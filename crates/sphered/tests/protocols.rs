//! Protocols against a real sphered: written, listed and run over the socket, started by the clock, and kept across a
//! restart. The tests do not wait for the clock: they write a protocol that is already due.

mod common;

use common::*;
use sphere_proto::protocol::{Outcome, Protocol, Schedule, Weekday};
use sphere_proto::{EventBody, TaskState};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn saved(e: &sphere_proto::Event) -> bool {
    matches!(e.body, EventBody::ProtocolSaved { .. })
}

/// A protocol that came due a minute ago, as sphered's store holds it while sphered is off.
fn overdue(repo: &std::path::Path) -> Protocol {
    let now = unix_now();
    Protocol {
        id: "p1".into(),
        text: format!("every day 9:00 {}: do the thing", repo.display()),
        schedule: Schedule::Weekly {
            days: Weekday::ALL.to_vec(),
            hour: 9,
            minute: 0,
        },
        repo: repo.display().to_string(),
        prompt: "do the thing".into(),
        enabled: true,
        created_at: now - 86_400,
        last_run: None,
        next_run: Some(now - 60),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_protocol_runs_as_an_ordinary_task_and_outlives_a_restart() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;

    let line = format!(
        "every weekday 9:00 {}: update the dependencies, run the tests",
        env.repo.display()
    );
    let p = c.protocol_add(&line, None).await.unwrap();
    assert_eq!(p.id, "p1");
    assert!(p.enabled && p.next_run.unwrap() > unix_now());
    // Clients hear of it as it is saved, as an event of no task.
    let ev = w.wait("the protocol saved", saved).await;
    assert_eq!(ev.task, "");
    assert_eq!(c.protocol_list().await.unwrap(), vec![p.clone()]);
    assert_eq!(c.snapshot().await.unwrap().protocols, vec![p.clone()]);

    // Run now: a task like any other, tagged with its protocol.
    let task = c.protocol_run_now("p1").await.unwrap();
    let created = w.wait("its task", is_kind("task_created")).await;
    assert_eq!(created.task, task);
    let EventBody::TaskCreated {
        title,
        prompt,
        protocol,
        ..
    } = created.body
    else {
        unreachable!()
    };
    assert_eq!(protocol.as_deref(), Some("p1"));
    assert_eq!(title, "Update the dependencies");
    assert_eq!(prompt, "update the dependencies, run the tests");
    w.wait("the run to finish", is_turn).await;
    // The run is recorded on the protocol, in the same step as the task's creation.
    let again = w
        .wait("the protocol saved with its run", |e| {
            matches!(&e.body, EventBody::ProtocolSaved { protocol } if protocol.last_run.is_some())
        })
        .await;
    assert_eq!(again.seq, created.seq + 1);

    let snap = c.snapshot().await.unwrap();
    let t = &snap.tasks[0];
    assert_eq!(
        (t.protocol.as_deref(), t.state),
        (Some("p1"), TaskState::Done)
    );
    let now = &snap.protocols[0];
    assert_eq!(
        now.next_run, p.next_run,
        "running it now leaves the schedule alone"
    );
    let last = now.last_run.as_ref().unwrap();
    assert_eq!(last.task.as_deref(), Some(task.as_str()));
    let state_of = |id: &str| snap.tasks.iter().find(|t| t.id == id).map(|t| t.state);
    assert_eq!(now.outcome(state_of), Outcome::Done);

    // sphered restarts: the protocol and its last run are still there.
    let before = c.protocol_list().await.unwrap();
    drop((c, w));
    env.kill_daemon();
    env.start_daemon().await;
    let c = env.client().await;
    assert_eq!(c.protocol_list().await.unwrap(), before);
    assert_eq!(c.snapshot().await.unwrap().protocols, before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_protocol_that_came_due_while_sphered_was_off_runs_once_when_it_is_back() {
    let mut env = Env::new("basic");
    std::fs::create_dir_all(&env.state).unwrap();
    {
        let mut store = sphered::store::Store::open(&env.state.join("sphere.db")).unwrap();
        let protocol = overdue(&env.repo);
        store
            .append("", &[EventBody::ProtocolSaved { protocol }], None)
            .unwrap();
    }
    env.start_daemon().await;
    let mut w = env.watcher(0, None).await;

    let created = w.wait("its task", is_kind("task_created")).await;
    let EventBody::TaskCreated { protocol, .. } = &created.body else {
        unreachable!()
    };
    assert_eq!(protocol.as_deref(), Some("p1"));
    w.wait("the run to finish", is_turn).await;

    let c = env.client().await;
    let p = c.protocol_list().await.unwrap().remove(0);
    assert!(
        p.next_run.unwrap() > unix_now(),
        "moved on to the next 09:00"
    );
    assert_eq!(
        p.last_run.unwrap().task.as_deref(),
        Some(created.task.as_str())
    );

    // Another restart does not run it again.
    drop((c, w));
    env.kill_daemon();
    env.start_daemon().await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let c = env.client().await;
    assert_eq!(c.snapshot().await.unwrap().tasks.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_protocol_missed_by_more_than_half_a_day_is_not_run_late() {
    let mut env = Env::new("basic");
    std::fs::create_dir_all(&env.state).unwrap();
    {
        let mut store = sphered::store::Store::open(&env.state.join("sphere.db")).unwrap();
        let mut protocol = overdue(&env.repo);
        protocol.next_run = Some(unix_now() - 13 * 3600);
        store
            .append("", &[EventBody::ProtocolSaved { protocol }], None)
            .unwrap();
    }
    env.start_daemon().await;
    let c = env.client().await;
    // The first look at the clock is at start-up: it has happened once the protocol has moved on.
    let p = eventually("the protocol to move on", 10, || async {
        let p = c.protocol_list().await.unwrap().remove(0);
        p.last_run.is_some().then_some(p)
    })
    .await;
    assert_eq!(
        p.last_run.unwrap().skipped.as_deref(),
        Some("missed while off")
    );
    assert!(p.next_run.unwrap() > unix_now());
    assert!(c.snapshot().await.unwrap().tasks.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_protocol_is_turned_off_and_removed_and_refuses_what_makes_no_sense() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let c = env.client().await;
    let line = format!("daily 9:00 {}: tidy up", env.repo.display());
    let p = c.protocol_add(&line, None).await.unwrap();

    let off = c.protocol_set_enabled(&p.id, false).await.unwrap();
    assert!(!off.enabled && off.next_run.is_none());
    assert_eq!(c.protocol_list().await.unwrap(), vec![off]);
    let on = c.protocol_set_enabled(&p.id, true).await.unwrap();
    assert!(on.enabled && on.next_run.is_some());

    // A line that is not a schedule says what to type; a repo that is not there says so.
    let e = c
        .protocol_add("evry weekday 9:00 x: y", None)
        .await
        .unwrap_err();
    assert_eq!(e.kind(), Some("bad_schedule"));
    assert!(e.to_string().contains("didn't understand 'evry'"), "{e}");
    let e = c.protocol_add("daily 9:00: y", None).await.unwrap_err();
    assert_eq!(e.kind(), Some("bad_repo"));
    // The default repo is used when the line names none.
    let there = env.repo.to_string_lossy().to_string();
    let q = c.protocol_add("daily 9:00: y", Some(&there)).await.unwrap();
    assert_eq!(q.id, "p2");

    assert_eq!(
        c.protocol_set_enabled("p9", true).await.unwrap_err().kind(),
        Some("not_found")
    );
    assert_eq!(
        c.protocol_run_now("p9").await.unwrap_err().kind(),
        Some("not_found")
    );
    c.protocol_remove(&p.id).await.unwrap();
    assert_eq!(
        c.protocol_remove(&p.id).await.unwrap_err().kind(),
        Some("not_found")
    );
    let left: Vec<String> = c
        .protocol_list()
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(left, ["p2"]);
}
