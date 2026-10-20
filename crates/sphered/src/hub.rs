//! The hub: one lock around "assign seq, persist, update the views, broadcast".
//! Durable events are stored before anyone hears of them; live frames are only
//! broadcast, and may be dropped under load.

use crate::store::{NoteAdd, OffsetUpdate, Store};
use anyhow::Result;
use sphere_proto::api::LiveFrame;
use sphere_proto::{Event, EventBody, Note, NoteSource, Views};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)] // events dominate; boxing them would only add an allocation per message
pub enum Msg {
    Event(Event),
    Live(LiveFrame),
}

struct Sub {
    tx: mpsc::Sender<Msg>,
    task: Option<String>,
    lagged: Arc<AtomicBool>,
}

struct Inner {
    store: Store,
    views: Views,
    subs: Vec<Sub>,
}

pub struct Hub {
    inner: Mutex<Inner>,
    next_task: AtomicU64,
    next_request: AtomicU64,
    next_protocol: AtomicU64,
    queue_cap: usize,
}

/// A live subscription. Events after `replay_upto` arrive on `rx`; everything
/// up to it is read from the store with [`Hub::replay_page`].
pub struct Subscription {
    pub rx: mpsc::Receiver<Msg>,
    /// Set when the hub dropped this subscriber because its queue overflowed.
    pub lagged: Arc<AtomicBool>,
    pub replay_upto: u64,
}

impl Hub {
    /// Open the store and rebuild the views from it.
    pub fn open(path: &Path, queue_cap: usize) -> Result<Hub> {
        Self::with_store(Store::open(path)?, queue_cap)
    }

    pub fn with_store(store: Store, queue_cap: usize) -> Result<Hub> {
        let mut views = Views::default();
        let mut since = 0;
        loop {
            let page = store.events_after(since, i64::MAX as u64, None, 1000)?;
            let Some(last) = page.last().map(|e| e.seq) else {
                break;
            };
            for ev in &page {
                views.apply(ev);
            }
            since = last;
        }
        views.seq = views.seq.max(store.last_seq()?);
        Ok(Hub {
            next_task: AtomicU64::new(views.max_task()),
            next_request: AtomicU64::new(views.max_request()),
            next_protocol: AtomicU64::new(views.max_protocol()),
            inner: Mutex::new(Inner {
                store,
                views,
                subs: vec![],
            }),
            queue_cap,
        })
    }

    pub fn new_task_id(&self) -> String {
        format!("t{}", self.next_task.fetch_add(1, Ordering::SeqCst) + 1)
    }

    pub fn new_request_id(&self) -> String {
        format!("r{}", self.next_request.fetch_add(1, Ordering::SeqCst) + 1)
    }

    pub fn new_protocol_id(&self) -> String {
        format!("p{}", self.next_protocol.fetch_add(1, Ordering::SeqCst) + 1)
    }

    /// Read the views under the lock.
    pub fn view<R>(&self, f: impl FnOnce(&Views) -> R) -> R {
        f(&self.inner.lock().unwrap().views)
    }

    pub fn offset(&self, child: &str, stream: &str) -> Result<u64> {
        self.inner.lock().unwrap().store.offset(child, stream)
    }

    pub fn last_seq(&self) -> u64 {
        self.view(|v| v.seq)
    }

    /// Persist `bodies` (and `offset`) in one transaction, then update the
    /// views and tell the subscribers.
    pub fn commit(
        &self,
        task: &str,
        bodies: Vec<EventBody>,
        offset: Option<OffsetUpdate>,
    ) -> Result<Vec<Event>> {
        let mut g = self.inner.lock().unwrap();
        Self::commit_in(&mut g, task, bodies, offset)
    }

    fn commit_in(
        g: &mut Inner,
        task: &str,
        bodies: Vec<EventBody>,
        offset: Option<OffsetUpdate>,
    ) -> Result<Vec<Event>> {
        let events = g.store.append(task, &bodies, offset.as_ref())?;
        Self::deliver(g, &events);
        Ok(events)
    }

    /// Like [`Hub::commit`] for events that belong to different tasks (the task
    /// is empty for one that belongs to none): all of them are stored, or none.
    pub fn commit_many(&self, items: Vec<(String, EventBody)>) -> Result<Vec<Event>> {
        let mut g = self.inner.lock().unwrap();
        let events = g
            .store
            .append_to(items.iter().map(|(t, b)| (t.as_str(), b)), None)?;
        Self::deliver(&mut g, &events);
        Ok(events)
    }

    /// Fold stored events into the views and hand them to the subscribers.
    fn deliver(g: &mut Inner, events: &[Event]) {
        let Inner { views, subs, .. } = g;
        for ev in events {
            views.apply(ev);
        }
        subs.retain(|s| {
            for ev in events {
                if s.task.as_deref().is_some_and(|t| t != ev.task) {
                    continue;
                }
                match s.tx.try_send(Msg::Event(ev.clone())) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        s.lagged.store(true, Ordering::SeqCst);
                        return false;
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => return false,
                }
            }
            true
        });
    }

    pub fn publish(&self, task: &str, body: EventBody) -> Result<Event> {
        Ok(self.commit(task, vec![body], None)?.remove(0))
    }

    // ---- the user's notes ----------------------------------------------------------

    /// Keep a note (see [`Store::add_note`]). The subscribers hear of a new one with a `MemoryChanged` event.
    pub fn add_note(&self, repo: Option<&str>, text: &str, source: NoteSource) -> Result<NoteAdd> {
        let mut g = self.inner.lock().unwrap();
        let added = g.store.add_note(repo, text, source)?;
        if matches!(added, NoteAdd::Added(_)) {
            Self::announce_notes(&mut g)?;
        }
        Ok(added)
    }

    /// Delete a note. False when there is none with that id.
    pub fn remove_note(&self, id: u64) -> Result<bool> {
        let mut g = self.inner.lock().unwrap();
        let removed = g.store.remove_note(id)?;
        if removed {
            Self::announce_notes(&mut g)?;
        }
        Ok(removed)
    }

    /// Every note, newest first.
    pub fn notes(&self) -> Result<Vec<Note>> {
        self.inner.lock().unwrap().store.notes()
    }

    /// Read the views and the notes together, so a snapshot's `seq` and its notes are from the same moment: a
    /// client that subscribes after `seq` hears of every later change.
    pub fn view_with_notes<R>(&self, f: impl FnOnce(&Views, Vec<Note>) -> R) -> R {
        let g = self.inner.lock().unwrap();
        let notes = g.store.notes().unwrap_or_else(|e| {
            tracing::warn!("could not read the notes: {e:#}");
            vec![]
        });
        f(&g.views, notes)
    }

    /// Tell everyone the notes changed. The event holds no text, only how many notes there are.
    fn announce_notes(g: &mut Inner) -> Result<()> {
        let notes = g.store.note_count()? as u32;
        Self::commit_in(g, "", vec![EventBody::MemoryChanged { notes }], None)?;
        Ok(())
    }

    /// Broadcast a live frame to subscribers. Never stored; dropped when a
    /// subscriber's queue is full.
    pub fn live(&self, task: &str, text: String) {
        let g = self.inner.lock().unwrap();
        for s in &g.subs {
            if s.task.as_deref().is_some_and(|t| t != task) {
                continue;
            }
            let _ = s.tx.try_send(Msg::Live(LiveFrame {
                task: task.to_string(),
                text: text.clone(),
            }));
        }
    }

    /// Register a subscriber. Under the lock, so nothing is missed or doubled
    /// between the events to replay (`..=replay_upto`) and the live ones.
    pub fn subscribe(&self, task: Option<String>) -> Subscription {
        let mut g = self.inner.lock().unwrap();
        let (tx, rx) = mpsc::channel(self.queue_cap);
        let lagged = Arc::new(AtomicBool::new(false));
        let replay_upto = g.views.seq;
        g.subs.push(Sub {
            tx,
            task,
            lagged: lagged.clone(),
        });
        Subscription {
            rx,
            lagged,
            replay_upto,
        }
    }

    pub fn replay_page(
        &self,
        since: u64,
        upto: u64,
        task: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Event>> {
        self.inner
            .lock()
            .unwrap()
            .store
            .events_after(since, upto, task, limit)
    }

    pub fn history(
        &self,
        task: Option<&str>,
        before: Option<u64>,
        limit: usize,
    ) -> Result<Vec<Event>> {
        self.inner
            .lock()
            .unwrap()
            .store
            .history(task, before, limit)
    }

    pub fn subscriber_count(&self) -> usize {
        self.inner.lock().unwrap().subs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sphere_proto::TaskState;

    fn hub(cap: usize) -> Hub {
        Hub::with_store(Store::open_in_memory().unwrap(), cap).unwrap()
    }

    fn created() -> EventBody {
        EventBody::TaskCreated {
            title: "t".into(),
            prompt: "p".into(),
            repo: "/r".into(),
            in_place: false,
            budget_usd: 1.0,
            mode: None,
            model: None,
            allow_rewrites: false,
            resume_session: None,
            parent_task: None,
            protocol: None,
        }
    }

    fn saved(id: &str) -> EventBody {
        EventBody::ProtocolSaved {
            protocol: sphere_proto::Protocol {
                id: id.into(),
                text: "in 2 hours sphere: x".into(),
                schedule: sphere_proto::protocol::Schedule::Once { at: 100 },
                repo: "/r".into(),
                prompt: "x".into(),
                enabled: true,
                created_at: 1,
                last_run: None,
                next_run: Some(100),
            },
        }
    }

    fn say(t: &str) -> EventBody {
        EventBody::Say { text: t.into() }
    }

    #[test]
    fn commit_persists_before_broadcast_and_updates_views() {
        let h = hub(16);
        let mut sub = h.subscribe(None);
        let id = h.new_task_id();
        assert_eq!(id, "t1");
        let ev = h.publish(&id, created()).unwrap();
        assert_eq!(ev.seq, 1);
        // What the subscriber heard is already in the store.
        match sub.rx.try_recv().unwrap() {
            Msg::Event(e) => {
                assert_eq!(e.seq, 1);
                assert_eq!(h.history(None, None, 10).unwrap().len(), 1);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            h.view(|v| v.task("t1").map(|t| t.state)),
            Some(TaskState::Queued)
        );
        assert_eq!(h.new_task_id(), "t2");
    }

    #[test]
    fn views_are_rebuilt_from_the_store_and_ids_continue() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        {
            let h = Hub::open(&path, 16).unwrap();
            let id = h.new_task_id();
            h.publish(&id, created()).unwrap();
            h.publish(&id, say("hi")).unwrap();
        }
        let h = Hub::open(&path, 16).unwrap();
        assert_eq!(h.last_seq(), 2);
        assert_eq!(
            h.view(|v| v.task("t1").map(|t| t.last_say.clone())),
            Some("hi".into())
        );
        assert_eq!(h.new_task_id(), "t2");
    }

    #[test]
    fn subscribe_replays_then_goes_live_with_no_gap() {
        let h = hub(16);
        h.publish("t1", created()).unwrap();
        h.publish("t1", say("a")).unwrap();
        let mut sub = h.subscribe(None);
        assert_eq!(sub.replay_upto, 2);
        h.publish("t1", say("b")).unwrap();
        let replay = h.replay_page(0, sub.replay_upto, None, 100).unwrap();
        assert_eq!(replay.len(), 2);
        match sub.rx.try_recv().unwrap() {
            Msg::Event(e) => assert_eq!(e.seq, 3),
            other => panic!("{other:?}"),
        }
        assert!(sub.rx.try_recv().is_err());
    }

    #[test]
    fn a_slow_subscriber_is_dropped_and_flagged_but_live_frames_never_drop_it() {
        let h = hub(2);
        let mut slow = h.subscribe(None);
        let mut other = h.subscribe(Some("t2".into()));
        h.publish("t1", created()).unwrap();
        h.publish("t1", say("1")).unwrap();
        assert!(!slow.lagged.load(Ordering::SeqCst));
        // Live frames beyond the queue are discarded without penalty.
        for _ in 0..5 {
            h.live("t1", "x".into());
        }
        assert!(!slow.lagged.load(Ordering::SeqCst));
        h.publish("t1", say("2")).unwrap();
        assert!(
            slow.lagged.load(Ordering::SeqCst),
            "queue overflow drops the client"
        );
        assert_eq!(
            h.subscriber_count(),
            1,
            "the filtered subscriber is unaffected"
        );
        // The queue holds what fitted, then reports closed.
        assert!(slow.rx.try_recv().is_ok());
        assert!(other.rx.try_recv().is_err());
    }

    #[test]
    fn a_new_or_removed_note_is_announced_once_and_a_repeat_is_not() {
        let h = hub(16);
        let mut all = h.subscribe(None);
        let mut one_task = h.subscribe(Some("t1".into()));
        let changed = |sub: &mut Subscription| match sub.rx.try_recv() {
            Ok(Msg::Event(e)) => {
                assert_eq!(e.task, "", "a note belongs to no task");
                Some(e.body)
            }
            Ok(other) => panic!("{other:?}"),
            Err(_) => None,
        };
        let NoteAdd::Added(note) = h
            .add_note(Some("/r"), "use pnpm", NoteSource::Typed)
            .unwrap()
        else {
            panic!("not added")
        };
        assert_eq!(
            changed(&mut all),
            Some(EventBody::MemoryChanged { notes: 1 })
        );
        let again = h
            .add_note(Some("/r"), "use pnpm", NoteSource::Change)
            .unwrap();
        assert!(matches!(again, NoteAdd::Already(_)));
        assert_eq!(changed(&mut all), None, "nothing changed, nothing to tell");
        assert!(h.remove_note(note.id).unwrap());
        assert_eq!(
            changed(&mut all),
            Some(EventBody::MemoryChanged { notes: 0 })
        );
        assert!(!h.remove_note(note.id).unwrap());
        assert_eq!(changed(&mut all), None);
        assert_eq!(changed(&mut one_task), None, "a task's watcher is not told");
        // the changes are durable: a client that was away replays them
        assert_eq!(h.replay_page(0, 10, None, 10).unwrap().len(), 2);
    }

    #[test]
    fn a_snapshot_reads_its_seq_and_its_notes_together() {
        let h = hub(16);
        h.publish("t1", created()).unwrap();
        h.add_note(None, "for everyone", NoteSource::Cli).unwrap();
        let (seq, notes) = h.view_with_notes(|v, notes| (v.seq, notes));
        assert_eq!(seq, 2, "the task, then the note's announcement");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].text, "for everyone");
        // after the seq it names, everything else is a later event
        h.remove_note(notes[0].id).unwrap();
        assert_eq!(h.replay_page(seq, 10, None, 10).unwrap().len(), 1);
    }

    #[test]
    fn task_filtered_subscribers_only_hear_their_task() {
        let h = hub(16);
        let mut sub = h.subscribe(Some("t2".into()));
        h.publish("t1", say("no")).unwrap();
        h.publish("t2", say("yes")).unwrap();
        h.live("t1", "no".into());
        h.live("t2", "yes".into());
        let mut got = vec![];
        while let Ok(m) = sub.rx.try_recv() {
            got.push(m);
        }
        assert_eq!(got.len(), 2);
        assert!(matches!(&got[0], Msg::Event(e) if e.task == "t2"));
        assert!(matches!(&got[1], Msg::Live(f) if f.task == "t2"));
    }

    #[test]
    fn events_of_several_tasks_commit_together_and_reach_the_right_subscribers() {
        let h = hub(16);
        let (mut all, mut t1, mut t9) = (
            h.subscribe(None),
            h.subscribe(Some("t1".into())),
            h.subscribe(Some("t9".into())),
        );
        let evs = h
            .commit_many(vec![("t1".into(), created()), (String::new(), saved("p1"))])
            .unwrap();
        assert_eq!(
            evs.iter()
                .map(|e| (e.seq, e.task.as_str()))
                .collect::<Vec<_>>(),
            vec![(1, "t1"), (2, "")]
        );
        assert!(h.view(|v| v.task("t1").is_some() && v.protocol("p1").is_some()));
        assert_eq!(h.history(None, None, 10).unwrap().len(), 2);
        let seqs = |sub: &mut Subscription| {
            let mut got = vec![];
            while let Ok(Msg::Event(e)) = sub.rx.try_recv() {
                got.push(e.seq);
            }
            got
        };
        assert_eq!(seqs(&mut all), [1, 2]);
        assert_eq!(seqs(&mut t1), [1]);
        assert!(seqs(&mut t9).is_empty());
    }

    #[test]
    fn protocol_ids_continue_after_a_restart_and_are_not_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        {
            let h = Hub::open(&path, 16).unwrap();
            assert_eq!(h.new_protocol_id(), "p1");
            assert_eq!(h.new_protocol_id(), "p2");
            h.publish("", saved("p1")).unwrap();
            h.publish("", saved("p2")).unwrap();
            h.publish("", EventBody::ProtocolRemoved { id: "p2".into() })
                .unwrap();
        }
        let h = Hub::open(&path, 16).unwrap();
        assert_eq!(h.view(|v| v.protocols().len()), 1);
        assert_eq!(h.new_protocol_id(), "p3");
    }
}
