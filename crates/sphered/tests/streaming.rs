//! Subscriptions: replay without gaps, `since`, and dropping slow clients.

mod common;

use common::*;
use sphere_proto::EventBody;
use std::io::{BufRead, Write};
use std::os::unix::net::UnixStream;

#[tokio::test]
async fn a_late_subscriber_gets_history_then_live_with_no_gap_or_repeat() {
    let mut env = Env::new("followup");
    env.start_daemon().await;
    let c = env.client().await;
    let mut early = env.watcher(0, None).await;
    let task = env.start_task(&c, "Streaming", "go").await;
    early.wait("first turn", is_turn).await;

    // Subscribe from the start while more events are produced.
    let mut late = env.watcher(0, None).await;
    c.send(&task, "again").await.unwrap();
    early
        .wait_from(
            0,
            "second turn",
            |e| matches!(&e.body, EventBody::Say { text } if text.contains("second turn")),
        )
        .await;
    late.wait("second turn", say_has("second turn")).await;
    early.settle(300).await;
    late.settle(300).await;

    let seqs: Vec<u64> = late.log.iter().map(|e| e.seq).collect();
    assert!(
        seqs.windows(2).all(|w| w[1] == w[0] + 1),
        "no gaps or repeats: {seqs:?}"
    );
    let early_seqs: Vec<u64> = early.log.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, early_seqs, "both saw the same stream");
    assert_eq!(seqs[0], 1);
}

#[tokio::test]
async fn since_skips_what_the_client_already_has() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Since", "go").await;
    w.wait("turn", is_turn).await;
    let last = w.log.last().unwrap().seq;
    let mid = w.log[w.log.len() / 2].seq;
    let mut again = env.watcher(mid, None).await;
    again.settle(500).await;
    assert_eq!(again.log.first().unwrap().seq, mid + 1);
    assert!(again.log.last().unwrap().seq >= last);
    let mut other = env.watcher(0, Some("t99")).await;
    other.settle(300).await;
    assert!(other.log.is_empty(), "a task filter hides other tasks");
}

#[tokio::test]
async fn events_survive_a_restart_and_keep_counting() {
    let mut env = Env::new("followup");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    let task = env.start_task(&c, "Durable", "go").await;
    w.wait("turn", is_turn).await;
    let before = w.log.last().unwrap().seq;
    drop(c);
    env.kill_daemon();
    env.start_daemon().await;
    let mut again = env.watcher(0, None).await;
    again.settle(500).await;
    assert!(
        again.log.iter().any(|e| e.seq == before),
        "history is still there"
    );
    let snap = env.client().await.snapshot().await.unwrap();
    assert_eq!(snap.tasks[0].id, task);
    assert!(snap.seq >= before);
}

#[tokio::test]
async fn a_client_that_stops_reading_is_dropped_with_a_lagged_error() {
    // 1500 lines of 3 KB: far more than a socket buffer holds.
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("flood.jsonl");
    let mut text = String::from("{\"op\":\"init\"}\n{\"op\":\"wait_user\"}\n");
    let blob = "x".repeat(3000);
    for _ in 0..1500 {
        text.push_str(&format!("{{\"op\":\"say\",\"text\":\"{blob}\"}}\n"));
    }
    text.push_str("{\"op\":\"result\",\"text\":\"done\"}\n");
    std::fs::write(&fixture, text).unwrap();

    let mut env = Env::new("basic").set("subscriber_queue", "600").set(
        "agent_args",
        &format!("[\"--fixture\", {:?}]", fixture.to_string_lossy()),
    );
    env.start_daemon().await;

    let mut slow = UnixStream::connect(&env.socket).unwrap();
    writeln!(
        slow,
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"subscribe\",\"params\":{{\"since\":0}}}}"
    )
    .unwrap();
    // ...and now it never reads.

    let c = env.client().await;
    let mut healthy = env.watcher(0, None).await;
    env.start_task(&c, "Flood", "go").await;
    healthy.wait("the whole flood", is_turn).await;
    assert!(!healthy.lagged, "a client that keeps up is not dropped");
    assert!(healthy.count(say_has("xxxx")) >= 1500);

    slow.set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let mut lagged = false;
    for line in std::io::BufReader::new(slow).lines() {
        let Ok(line) = line else { break };
        if line.contains("lagged") {
            lagged = true;
            break;
        }
    }
    assert!(lagged, "the slow client was told it lagged");
    // sphered itself is fine.
    assert!(c.status().await.is_ok());
}
