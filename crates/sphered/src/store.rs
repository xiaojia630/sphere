//! The event store: SQLite in WAL mode. One `events` table with a global
//! sequence number, plus the read offsets into the host's output files, saved
//! in the same transaction as the events they produced. The user's notes for
//! agents have a table of their own, so a removed note is deleted, not left in
//! the log.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use sphere_proto::memory::NOTES_MAX;
use sphere_proto::{clean_note, now_rfc3339, Event, EventBody, Note, NoteSource};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub struct Store {
    conn: Connection,
}

/// Where sphered has read up to in a child's output file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffsetUpdate {
    pub child: String,
    pub stream: &'static str,
    pub offset: u64,
}

/// What [`Store::add_note`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteAdd {
    Added(Note),
    /// The same note was there already.
    Already(Note),
    /// Nothing was left of the text once it was cleaned up.
    Empty,
    /// The store holds [`NOTES_MAX`] notes already.
    Full,
}

/// How the notes for every repo are filed in the `scope` column. A repo is filed under its root, an absolute
/// path, which can't be this.
const ALL_SCOPE: &str = "all";

fn scope_of(repo: Option<&str>) -> &str {
    repo.filter(|r| *r != ALL_SCOPE).unwrap_or(ALL_SCOPE)
}

const NOTE_COLUMNS: &str = "id, scope, text, created_at, source";

fn note_from_row(r: &rusqlite::Row) -> rusqlite::Result<Note> {
    let scope: String = r.get(1)?;
    let source: String = r.get(4)?;
    Ok(Note {
        id: r.get::<_, i64>(0)? as u64,
        repo: (scope != ALL_SCOPE).then_some(scope),
        text: r.get(2)?,
        created_at: r.get(3)?,
        source: NoteSource::parse(&source),
    })
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        Self::init(conn, Some(path))
    }

    pub fn open_in_memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?, None)
    }

    fn init(conn: Connection, path: Option<&Path>) -> Result<Store> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                 seq  INTEGER PRIMARY KEY AUTOINCREMENT,
                 task TEXT NOT NULL,
                 at   TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 body TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS events_by_task ON events(task, seq);
             CREATE TABLE IF NOT EXISTS offsets (
                 child  TEXT NOT NULL,
                 stream TEXT NOT NULL,
                 off    INTEGER NOT NULL,
                 PRIMARY KEY (child, stream)
             );
             CREATE TABLE IF NOT EXISTS memory (
                 id         INTEGER PRIMARY KEY AUTOINCREMENT,
                 scope      TEXT NOT NULL,
                 text       TEXT NOT NULL,
                 created_at TEXT NOT NULL,
                 source     TEXT NOT NULL,
                 UNIQUE (scope, text)
             );",
        )?;
        // a removed note's text is overwritten, not left in the free pages
        conn.pragma_update(None, "secure_delete", "ON")?;
        if let Some(path) = path {
            // The store is user-only.
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(Store { conn })
    }

    pub fn journal_mode(&self) -> Result<String> {
        Ok(self
            .conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))?)
    }

    /// Append events (and move an offset) atomically. Returns the stored events.
    pub fn append(
        &mut self,
        task: &str,
        bodies: &[EventBody],
        offset: Option<&OffsetUpdate>,
    ) -> Result<Vec<Event>> {
        let tx = self.conn.transaction()?;
        let at = now_rfc3339();
        let mut out = Vec::with_capacity(bodies.len());
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO events (task, at, kind, body) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for body in bodies {
                let json = serde_json::to_string(body)?;
                stmt.execute(params![task, at, body.kind_name(), json])?;
                out.push(Event {
                    seq: tx.last_insert_rowid() as u64,
                    task: task.to_string(),
                    at: at.clone(),
                    body: body.clone(),
                });
            }
        }
        if let Some(o) = offset {
            tx.execute(
                "INSERT INTO offsets (child, stream, off) VALUES (?1, ?2, ?3)
                 ON CONFLICT(child, stream) DO UPDATE SET off = excluded.off",
                params![o.child, o.stream, o.offset as i64],
            )?;
        }
        tx.commit()?;
        Ok(out)
    }

    pub fn last_seq(&self) -> Result<u64> {
        let n: Option<i64> = self
            .conn
            .query_row("SELECT MAX(seq) FROM events", [], |r| r.get(0))?;
        Ok(n.unwrap_or(0) as u64)
    }

    fn row_to_event(seq: i64, task: String, at: String, body: String) -> Option<Event> {
        match serde_json::from_str::<EventBody>(&body) {
            Ok(body) => Some(Event {
                seq: seq as u64,
                task,
                at,
                body,
            }),
            Err(e) => {
                tracing::warn!(seq, "skipping unreadable event: {e}");
                None
            }
        }
    }

    /// Events with `since < seq <= upto`, oldest first, at most `limit`.
    pub fn events_after(
        &self,
        since: u64,
        upto: u64,
        task: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT seq, task, at, body FROM events
             WHERE seq > ?1 AND seq <= ?2 AND (?3 IS NULL OR task = ?3)
             ORDER BY seq LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![since as i64, upto as i64, task, limit as i64],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let mut out = vec![];
        for row in rows {
            let (seq, task, at, body) = row?;
            out.extend(Self::row_to_event(seq, task, at, body));
        }
        Ok(out)
    }

    /// The newest `limit` events older than `before`, oldest first.
    pub fn history(
        &self,
        task: Option<&str>,
        before: Option<u64>,
        limit: usize,
    ) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT seq, task, at, body FROM events
             WHERE seq < ?1 AND (?2 IS NULL OR task = ?2)
             ORDER BY seq DESC LIMIT ?3",
        )?;
        let before = before.map(|b| b as i64).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![before, task, limit as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        let mut out = vec![];
        for row in rows {
            let (seq, task, at, body) = row?;
            out.extend(Self::row_to_event(seq, task, at, body));
        }
        out.reverse();
        Ok(out)
    }

    /// Keep a note for `repo` (`None`: every repo). The text is cleaned up first (see `clean_note`). The same
    /// note twice in one scope is kept once.
    pub fn add_note(
        &mut self,
        repo: Option<&str>,
        text: &str,
        source: NoteSource,
    ) -> Result<NoteAdd> {
        let Some(text) = clean_note(text) else {
            return Ok(NoteAdd::Empty);
        };
        let scope = scope_of(repo);
        let tx = self.conn.transaction()?;
        let same = tx
            .query_row(
                &format!("SELECT {NOTE_COLUMNS} FROM memory WHERE scope = ?1 AND text = ?2"),
                params![scope, text],
                note_from_row,
            )
            .optional()?;
        if let Some(note) = same {
            return Ok(NoteAdd::Already(note));
        }
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM memory", [], |r| r.get(0))?;
        if count as usize >= NOTES_MAX {
            return Ok(NoteAdd::Full);
        }
        let created_at = now_rfc3339();
        tx.execute(
            "INSERT INTO memory (scope, text, created_at, source) VALUES (?1, ?2, ?3, ?4)",
            params![scope, text, created_at, source.as_str()],
        )?;
        let id = tx.last_insert_rowid() as u64;
        tx.commit()?;
        Ok(NoteAdd::Added(Note {
            id,
            repo: repo.filter(|r| *r != ALL_SCOPE).map(str::to_string),
            text,
            created_at,
            source,
        }))
    }

    /// Every note, newest first.
    pub fn notes(&self) -> Result<Vec<Note>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {NOTE_COLUMNS} FROM memory ORDER BY id DESC"
        ))?;
        let rows = stmt.query_map([], note_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn note_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM memory", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// Delete a note. False when there is none with that id.
    pub fn remove_note(&mut self, id: u64) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM memory WHERE id = ?1", params![id as i64])?;
        Ok(n > 0)
    }

    pub fn offset(&self, child: &str, stream: &str) -> Result<u64> {
        let n: Option<i64> = self
            .conn
            .query_row(
                "SELECT off FROM offsets WHERE child = ?1 AND stream = ?2",
                params![child, stream],
                |r| r.get(0),
            )
            .optional()?;
        Ok(n.unwrap_or(0) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn say(t: &str) -> EventBody {
        EventBody::Say { text: t.into() }
    }

    #[test]
    fn append_assigns_global_increasing_seq() {
        let mut s = Store::open_in_memory().unwrap();
        let a = s.append("t1", &[say("a"), say("b")], None).unwrap();
        let b = s.append("t2", &[say("c")], None).unwrap();
        assert_eq!(a.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(b[0].seq, 3);
        assert_eq!(s.last_seq().unwrap(), 3);
        assert_eq!(a[0].at.len(), 24);
    }

    #[test]
    fn queries_filter_and_page() {
        let mut s = Store::open_in_memory().unwrap();
        for i in 0..10 {
            s.append(
                if i % 2 == 0 { "t1" } else { "t2" },
                &[say(&i.to_string())],
                None,
            )
            .unwrap();
        }
        let all = s.events_after(0, 100, None, 100).unwrap();
        assert_eq!(all.len(), 10);
        let t1 = s.events_after(0, 100, Some("t1"), 100).unwrap();
        assert_eq!(t1.len(), 5);
        let window = s.events_after(3, 6, None, 100).unwrap();
        assert_eq!(
            window.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![4, 5, 6]
        );
        let page = s.events_after(0, 100, None, 4).unwrap();
        assert_eq!(page.len(), 4);

        let hist = s.history(None, None, 3).unwrap();
        assert_eq!(
            hist.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![8, 9, 10]
        );
        let older = s.history(None, Some(8), 3).unwrap();
        assert_eq!(
            older.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![5, 6, 7]
        );
        let h2 = s.history(Some("t2"), None, 2).unwrap();
        assert!(h2.iter().all(|e| e.task == "t2") && h2.len() == 2);
    }

    #[test]
    fn offsets_move_with_the_events_in_one_transaction() {
        let mut s = Store::open_in_memory().unwrap();
        assert_eq!(s.offset("c1", "out").unwrap(), 0);
        let o = OffsetUpdate {
            child: "c1".into(),
            stream: "out",
            offset: 120,
        };
        s.append("t1", &[say("x")], Some(&o)).unwrap();
        assert_eq!(s.offset("c1", "out").unwrap(), 120);
        let o2 = OffsetUpdate { offset: 200, ..o };
        s.append("t1", &[], Some(&o2)).unwrap();
        assert_eq!(s.offset("c1", "out").unwrap(), 200);
        assert_eq!(s.offset("c1", "err").unwrap(), 0);
    }

    #[test]
    fn file_store_uses_wal_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sphere.db");
        {
            let mut s = Store::open(&path).unwrap();
            assert_eq!(s.journal_mode().unwrap(), "wal");
            s.append("t1", &[say("kept")], None).unwrap();
        }
        let s = Store::open(&path).unwrap();
        let evs = s.history(None, None, 10).unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].body, say("kept"));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    fn add_as(s: &mut Store, repo: Option<&str>, text: &str, source: NoteSource) -> Note {
        match s.add_note(repo, text, source).unwrap() {
            NoteAdd::Added(n) => n,
            other => panic!("{text}: {other:?}"),
        }
    }

    fn add(s: &mut Store, repo: Option<&str>, text: &str) -> Note {
        add_as(s, repo, text, NoteSource::Typed)
    }

    #[test]
    fn notes_round_trip_newest_first() {
        let mut s = Store::open_in_memory().unwrap();
        assert!(s.notes().unwrap().is_empty());
        let a = add(&mut s, Some("/code/sphere"), "use pnpm, never npm");
        let b = add_as(&mut s, None, "answer in plain words", NoteSource::Cli);
        let c = add_as(
            &mut s,
            Some("/code/other"),
            "tests live in spec/",
            NoteSource::Change,
        );
        assert!(a.id < b.id && b.id < c.id);
        assert_eq!(a.repo.as_deref(), Some("/code/sphere"));
        assert_eq!((b.repo.clone(), b.source), (None, NoteSource::Cli));
        assert_eq!(a.created_at.len(), 24);
        // what was returned is what is stored
        assert_eq!(s.notes().unwrap(), vec![c, b, a]);
        assert_eq!(s.note_count().unwrap(), 3);
    }

    #[test]
    fn notes_survive_reopening_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sphere.db");
        let kept = {
            let mut s = Store::open(&path).unwrap();
            add(&mut s, Some("/code/sphere"), "keep me")
        };
        let mut s = Store::open(&path).unwrap();
        assert_eq!(s.notes().unwrap(), vec![kept.clone()]);
        // ids go on from where they were
        assert!(add(&mut s, None, "and me").id > kept.id);
    }

    #[test]
    fn the_same_note_twice_is_kept_once_per_scope() {
        let mut s = Store::open_in_memory().unwrap();
        let first = add(&mut s, Some("/r"), "use pnpm");
        // after cleaning up, these are the same note
        for again in ["use pnpm", "  use   pnpm \n"] {
            assert_eq!(
                s.add_note(Some("/r"), again, NoteSource::Change).unwrap(),
                NoteAdd::Already(first.clone())
            );
        }
        assert_eq!(s.note_count().unwrap(), 1);
        // another repo, or every repo, can have the same words
        add(&mut s, Some("/other"), "use pnpm");
        add(&mut s, None, "use pnpm");
        assert_eq!(s.note_count().unwrap(), 3);
        // case counts: only exact duplicates are dropped
        add(&mut s, Some("/r"), "Use pnpm");
    }

    #[test]
    fn the_word_all_files_a_note_for_every_repo() {
        let mut s = Store::open_in_memory().unwrap();
        let n = add(&mut s, Some("all"), "for everyone");
        assert_eq!(n.repo, None);
        assert_eq!(s.notes().unwrap()[0].repo, None);
    }

    #[test]
    fn notes_are_cleaned_up_and_capped_in_length() {
        let mut s = Store::open_in_memory().unwrap();
        assert_eq!(
            s.add_note(None, "  \n ", NoteSource::Typed).unwrap(),
            NoteAdd::Empty
        );
        assert_eq!(s.note_count().unwrap(), 0);
        let n = add(&mut s, None, "  two\nlines  ");
        assert_eq!(n.text, "two lines");
        let long = add(&mut s, None, &"word ".repeat(200));
        assert_eq!(
            long.text.chars().count(),
            sphere_proto::memory::NOTE_MAX_CHARS
        );
        assert!(long.text.ends_with('…'));
        assert_eq!(s.notes().unwrap()[0].text, long.text);
    }

    #[test]
    fn the_store_holds_so_many_notes_and_no_more() {
        let mut s = Store::open_in_memory().unwrap();
        let mut first = None;
        for i in 0..NOTES_MAX {
            let n = add(&mut s, Some("/r"), &format!("note {i}"));
            first.get_or_insert(n.id);
        }
        assert_eq!(
            s.add_note(Some("/r"), "one too many", NoteSource::Typed)
                .unwrap(),
            NoteAdd::Full
        );
        // a note that is there already is not "too many"
        assert!(matches!(
            s.add_note(Some("/r"), "note 0", NoteSource::Typed).unwrap(),
            NoteAdd::Already(_)
        ));
        assert!(s.remove_note(first.unwrap()).unwrap());
        add(&mut s, Some("/r"), "one too many");
        assert_eq!(s.note_count().unwrap(), NOTES_MAX);
    }

    #[test]
    fn a_removed_note_is_gone_and_its_id_is_not_used_again() {
        let mut s = Store::open_in_memory().unwrap();
        let a = add(&mut s, Some("/r"), "first");
        let b = add(&mut s, Some("/r"), "second");
        assert!(s.remove_note(b.id).unwrap());
        assert!(!s.remove_note(b.id).unwrap());
        assert!(!s.remove_note(999).unwrap());
        assert_eq!(s.notes().unwrap(), vec![a]);
        // the id of the newest note is not handed out again
        let c = add(&mut s, Some("/r"), "third");
        assert!(c.id > b.id);
        // and the same words can be written again
        let again = add(&mut s, Some("/r"), "second");
        assert!(again.id > c.id);
    }

    #[test]
    fn unreadable_rows_are_skipped_not_fatal() {
        let mut s = Store::open_in_memory().unwrap();
        s.append("t1", &[say("a")], None).unwrap();
        s.conn
            .execute("INSERT INTO events (task, at, kind, body) VALUES ('t1','x','future','{\"kind\":\"from_the_future\"}')", [])
            .unwrap();
        s.append("t1", &[say("b")], None).unwrap();
        assert_eq!(s.events_after(0, 100, None, 100).unwrap().len(), 2);
    }
}
