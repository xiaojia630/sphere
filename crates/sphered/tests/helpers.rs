//! Agents handing work to helper agents through Sphere's own tools, driven
//! over the agent's pipe the way Claude Code does.

mod common;

use common::*;
use sphere_proto::{EventBody, StepKind, TaskState};

#[tokio::test]
async fn a_parent_starts_a_helper_waits_for_it_and_reads_its_result() {
    let mut env = Env::new("helpers");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    let parent = env.start_task(&c, "Parent", "go").await;
    assert_eq!(parent, "t1");

    // The agent is started with the in-process server, and its tools never ask.
    let args = say_text(&w.wait("the args", say_has("args: ")).await);
    assert!(args.contains("--mcp-config"), "{args}");
    assert!(args.contains(r#""type":"sdk""#), "{args}");
    let settings = say_text(&w.wait("the settings", say_has("settings: ")).await);
    assert!(settings.contains("mcp__sphere"), "{settings}");

    // The handshake and the tool list.
    let init = say_text(&w.wait("initialize", say_has("serverInfo")).await);
    assert!(
        init.contains("2025-06-18") && init.contains(r#""name":"sphere""#),
        "{init}"
    );
    let list = say_text(&w.wait("tools", say_has("wait_for_helpers")).await);
    assert!(
        list.contains("start_helper") && list.contains("stop_helper"),
        "{list}"
    );

    // The helper appears as its own task, under the parent.
    let created = w
        .wait("the helper", |e| {
            e.task == "t2" && matches!(&e.body, EventBody::TaskCreated { .. })
        })
        .await;
    match &created.body {
        EventBody::TaskCreated {
            parent_task,
            title,
            prompt,
            in_place,
            ..
        } => {
            assert_eq!(parent_task.as_deref(), Some("t1"));
            assert_eq!(
                (title.as_str(), prompt.as_str()),
                ("Write notes", "Write notes.txt")
            );
            assert!(!in_place, "a helper gets its own worktree");
        }
        other => panic!("{other:?}"),
    }
    let started = say_text(&w.wait("start answer", say_has("Started helper t2")).await);
    assert!(started.contains("wait_for_helpers"), "{started}");

    // The helper is offered no start_helper and is refused when it tries.
    let helper_tools = w
        .wait("the helper's tools", |e| {
            e.task == "t2"
                && matches!(&e.body, EventBody::Say { text } if text.contains("wait_for_helpers"))
        })
        .await;
    assert!(!say_text(&helper_tools).contains("start_helper"));
    w.wait("a refusal", |e| {
        e.task == "t2"
            && matches!(&e.body, EventBody::Say { text } if text.contains("cannot start helpers"))
    })
    .await;

    // The wait returns the helper's summary, branch and file count.
    let waited = say_text(&w.wait("the wait", say_has("The helper has finished")).await);
    for want in [
        "The helper has finished",
        "t2 \\\"Write notes\\\": done",
        "Wrote notes.txt for you",
        "1 file changed",
    ] {
        assert!(waited.contains(want), "missing {want:?} in {waited}");
    }
    w.wait("the parent's turn", |e| e.task == "t1" && is_turn(e))
        .await;
    w.settle(300).await;

    // The steps read as delegation, and nothing was put to the human.
    let step = |title: &str| {
        w.find(|e| matches!(&e.body, EventBody::Step { title: t, .. } if t == title))
            .map(|e| match &e.body {
                EventBody::Step {
                    step_kind, tool, ..
                } => (*step_kind, tool.clone()),
                _ => unreachable!(),
            })
    };
    assert_eq!(
        step("Start helper: Write notes"),
        Some((StepKind::Delegate, "mcp__sphere__start_helper".into()))
    );
    assert_eq!(
        step("Waiting for helpers").map(|s| s.0),
        Some(StepKind::Delegate)
    );
    assert_eq!(w.count(is_request), 0, "Sphere's tools never become cards");
    assert_eq!(
        w.count(|e| matches!(e.body, EventBody::AutoAnswered { .. })),
        0,
        "settings allow them before Sphere is asked"
    );

    // Views: the helper is the parent's, finished with a branch; only the parent is new.
    let snap = c.snapshot().await.unwrap();
    let helper = snap.tasks.iter().find(|t| t.id == "t2").unwrap();
    let parent_view = snap.tasks.iter().find(|t| t.id == "t1").unwrap();
    assert_eq!(helper.parent_task.as_deref(), Some("t1"));
    assert_eq!(helper.state, TaskState::Done);
    assert!(helper.branch.is_some() && helper.files.len() == 1);
    assert!(!helper.unseen);
    assert_eq!(parent_view.state, TaskState::Done);
    assert!(parent_view.unseen);

    // Looking at it clears the mark; looking again, or at a helper, changes nothing.
    c.seen("t1").await.unwrap();
    c.seen("t1").await.unwrap();
    c.seen("t2").await.unwrap();
    let snap = c.snapshot().await.unwrap();
    assert!(snap.tasks.iter().all(|t| !t.unseen));
    assert_eq!(c.seen("t99").await.unwrap_err().kind(), Some("not_found"));
    w.settle(200).await;
    assert_eq!(
        w.count(is_kind("seen")),
        1,
        "only the first look is an event"
    );
}

#[tokio::test]
async fn stopping_a_parent_stops_the_helpers_still_working() {
    let mut env = Env::new("helpers_stop");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    let parent = env.start_task(&c, "Parent", "go").await;
    w.wait("the helper's agent", |e| {
        e.task == "t2" && matches!(e.body, EventBody::AgentStarted { .. })
    })
    .await;
    c.stop(&parent).await.unwrap();
    w.wait("the parent stopped", |e| {
        e.task == "t1" && matches!(e.body, EventBody::Stopped {})
    })
    .await;
    w.wait("the helper stopped", |e| {
        e.task == "t2" && matches!(e.body, EventBody::Stopped {})
    })
    .await;
    let snap = c.snapshot().await.unwrap();
    assert!(snap.tasks.iter().all(|t| t.state == TaskState::Stopped));
}
