//! Stop, restart, resume, idle agents and the controls: everything about an
//! agent's life that is not a single request.

mod common;

use common::*;
use serde_json::Value;
use sphere_client::AnswerParams;
use sphere_proto::{AnswerKind, EventBody, RequestKind, TaskState};
use std::time::Duration;

fn started(e: &sphere_proto::Event) -> bool {
    matches!(e.body, EventBody::AgentStarted { .. })
}

fn exited(reason: &'static str) -> impl Fn(&sphere_proto::Event) -> bool {
    move |e| matches!(&e.body, EventBody::AgentExited { reason: r, .. } if r == reason)
}

#[tokio::test]
async fn stop_withdraws_requests_interrupts_terminates_the_group_and_frees_the_slot() {
    let mut env = Env::new("stop");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    let task = env.start_task(&c, "Long job", "run for a long time").await;
    let sleeper = pid_from_say(
        &say_text(&w.wait("sleeper pid", say_has("sleeper: ")).await),
        "sleeper: ",
    );
    assert!(pid_alive(sleeper));
    let EventBody::Request { request } = w.wait("an ask", is_request).await.body else {
        unreachable!()
    };

    c.stop(&task).await.unwrap();

    // The pending ask is withdrawn; the agent exits as stopped.
    w.wait("withdrawal", |e| matches!(&e.body, EventBody::RequestWithdrawn { request: r, reason } if *r == request.id && reason.contains("stopped"))).await;
    w.wait("exit", exited("stopped")).await;
    w.wait("stopped", is_kind("stopped")).await;
    let snap = c.snapshot().await.unwrap();
    assert_eq!(snap.tasks[0].state, TaskState::Stopped);
    assert!(snap.requests.is_empty());
    assert_eq!(c.status().await.unwrap().active, 0);
    // Whatever it started went with it (the group was terminated).
    eventually("the sleeper to die", 10, || async {
        (!pid_alive(sleeper) || is_zombie(sleeper)).then_some(())
    })
    .await;
    // Answering a withdrawn request says so; stopping twice is harmless.
    let err = c.yes(&request.id).await.unwrap_err();
    assert_eq!(err.kind(), Some("withdrawn"), "{err}");
    c.stop(&task).await.unwrap();
    // A stopped task takes no more messages.
    assert_eq!(
        c.send(&task, "hello").await.unwrap_err().kind(),
        Some("invalid_state")
    );
}

fn is_zombie(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|s| s.contains(") Z "))
        .unwrap_or(false)
}

#[tokio::test]
async fn an_agent_that_ignores_sigterm_and_closed_input_is_killed_after_the_grace_period() {
    let mut env = Env::new("stubborn")
        .set("stop_grace_ms", "400")
        .set("interrupt_wait_ms", "100");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    let task = env.start_task(&c, "Stubborn", "go").await;
    let sleeper = pid_from_say(
        &say_text(&w.wait("sleeper pid", say_has("sleeper: ")).await),
        "sleeper: ",
    );
    let started_at = std::time::Instant::now();
    c.stop(&task).await.unwrap();
    assert!(
        started_at.elapsed() >= Duration::from_millis(400),
        "it waited out the grace period"
    );
    let ev = w.wait("exit", exited("stopped")).await;
    match ev.body {
        EventBody::AgentExited { signal, .. } => assert_eq!(signal, Some(9), "killed, not asked"),
        _ => unreachable!(),
    }
    eventually("the sleeper to die", 10, || async {
        (!pid_alive(sleeper) || is_zombie(sleeper)).then_some(())
    })
    .await;
    assert_eq!(
        c.snapshot().await.unwrap().tasks[0].state,
        TaskState::Stopped
    );
}

#[tokio::test]
async fn restarting_sphered_while_the_agent_waits_on_a_request_interrupts_nothing() {
    let mut env = Env::new("permission");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Survive", "push it").await;
    let EventBody::Request { request } = w.wait("request", is_request).await.body else {
        unreachable!()
    };
    let agent_pid = c.snapshot().await.unwrap().tasks[0].pid.unwrap() as i32;
    let host_pid = env.host_pid();
    let seq_before = c.snapshot().await.unwrap().seq;
    drop(w);
    drop(c);

    // kill -9: no shutdown, no goodbye.
    env.kill_daemon();
    assert!(
        pid_alive(agent_pid),
        "the agent keeps running without sphered"
    );
    assert_eq!(env.host_pid(), host_pid);
    env.start_daemon().await;
    assert_eq!(env.host_pid(), host_pid, "the same host");

    let c = env.client().await;
    let snap = c.snapshot().await.unwrap();
    assert_eq!(snap.tasks[0].state, TaskState::NeedsYou);
    assert_eq!(snap.tasks[0].pid, Some(agent_pid as u32));
    assert_eq!(snap.requests.len(), 1, "the request is still there");
    assert_eq!(
        snap.requests[0].info.id, request.id,
        "the same one, not a new one"
    );
    assert_eq!(
        snap.seq, seq_before,
        "nothing new was recorded by coming back"
    );

    // Answer it; the agent finishes without ever having been restarted.
    let mut w = env.watcher(0, None).await;
    c.yes(&request.id).await.unwrap();
    w.wait("Pushed.", say_has("Pushed.")).await;
    w.wait("turn", is_turn).await;
    assert_eq!(w.count(is_request), 1, "the ask was recorded once");
    assert_eq!(w.count(started), 1, "one start, no resume");
    assert!(w
        .find(|e| matches!(&e.body, EventBody::AgentStarted { resumed: true, .. }))
        .is_none());
    let args = say_text(w.find(say_has("args: ")).unwrap());
    assert!(!args.contains("--resume"), "{args}");
    assert_eq!(w.count(exited("lost")), 0);
    assert_eq!(c.snapshot().await.unwrap().tasks[0].state, TaskState::Done);
}

#[tokio::test]
async fn output_produced_while_sphered_is_down_is_picked_up_exactly_once() {
    let mut env = Env::new("slow");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "While away", "work").await;
    w.wait("working", say_has("slowly working")).await;
    drop(w);
    drop(c);
    env.kill_daemon();
    tokio::time::sleep(Duration::from_millis(2200)).await; // the agent finishes meanwhile
    env.start_daemon().await;
    let mut w = env.watcher(0, None).await;
    w.wait("the turn", is_turn).await;
    w.settle(500).await;
    assert_eq!(w.count(is_turn), 1);
    assert_eq!(
        w.count(say_has("slowly working")),
        1,
        "no line was read twice"
    );
    assert_eq!(w.count(started), 1);
    let c = env.client().await;
    let snap = c.snapshot().await.unwrap();
    assert_eq!(snap.tasks[0].state, TaskState::Done);
    assert!((snap.tasks[0].cost_usd - 0.5).abs() < 1e-9);
}

#[tokio::test]
async fn sphered_and_the_host_are_separate_processes_and_the_host_is_detached() {
    let mut env = Env::new("slow");
    env.start_daemon().await;
    let c = env.client().await;
    let st = c.status().await.unwrap();
    let host = st.host_pid.unwrap();
    assert_ne!(host, env.daemon_pid());
    let stat = std::fs::read_to_string(format!("/proc/{host}/stat")).unwrap();
    let rest = &stat[stat.rfind(')').unwrap() + 2..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let (ppid, sid): (u32, u32) = (fields[1].parse().unwrap(), fields[3].parse().unwrap());
    assert_ne!(
        ppid,
        env.daemon_pid(),
        "double fork: sphered is not its parent"
    );
    let mine = std::fs::read_to_string(format!("/proc/{}/stat", env.daemon_pid())).unwrap();
    let msid: u32 = mine[mine.rfind(')').unwrap() + 2..]
        .split_whitespace()
        .nth(3)
        .unwrap()
        .parse()
        .unwrap();
    assert_ne!(sid, msid, "the host is in its own session");
    drop(c);
    env.kill_daemon();
    assert!(
        pid_alive(host as i32),
        "killing sphered does not kill the host"
    );
    assert!(env.host_call("ping").is_some());
}

#[tokio::test]
async fn a_lost_host_means_resume_from_the_saved_session() {
    let mut env = Env::new("resume");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Resume me", "begin").await;
    w.wait("started work", say_has("starting")).await;
    let first_args = say_text(w.find(say_has("args: ")).unwrap());
    assert!(!first_args.contains("--resume"));
    let session = c.snapshot().await.unwrap().tasks[0]
        .session_id
        .clone()
        .expect("session id was saved");
    let old_host = env.host_pid();

    // The host goes away and takes the agent with it (an update, a crash).
    env.host_call("shutdown");

    w.wait("the agent to be flagged lost", exited("lost")).await;
    w.wait("a second start", |e| {
        matches!(&e.body, EventBody::AgentStarted { resumed: true, .. })
    })
    .await;
    let resumed_args = say_text(&w.wait("resumed args", |e| matches!(&e.body, EventBody::Say { text } if text.contains("args: ") && text.contains("--resume"))).await);
    assert!(
        resumed_args.contains(&format!("--resume {session}")),
        "{resumed_args}"
    );
    w.wait("continue message", say_has("resumed with: continue"))
        .await;
    w.wait("turn", is_turn).await;
    assert_eq!(w.count(started), 2);
    assert_ne!(env.host_pid(), old_host, "a new host");
    assert_eq!(c.snapshot().await.unwrap().tasks[0].state, TaskState::Done);
    assert_eq!(
        w.count(|e| matches!(e.body, EventBody::CutOff { .. })),
        0,
        "the Read had finished"
    );
}

#[tokio::test]
async fn a_cut_off_push_becomes_a_resume_check_before_anything_continues() {
    let mut env = Env::new("cutoff");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Cut off", "push").await;
    w.wait(
        "the push step",
        |e| matches!(&e.body, EventBody::Step { title, .. } if title.contains("git push")),
    )
    .await;
    env.host_call("shutdown");

    let cut = w.wait("cut-off flag", is_kind("cut_off")).await;
    assert!(
        matches!(&cut.body, EventBody::CutOff { outbound: true, title, .. } if title.contains("git push"))
    );
    let ev = w.wait("resume check", is_request).await;
    let EventBody::Request { request } = ev.body else {
        unreachable!()
    };
    assert_eq!(request.kind, RequestKind::ResumeCheck);
    assert!(request.input["cut_off"][0]["detail"]
        .as_str()
        .unwrap()
        .contains("git push origin sphere/x"));
    let snap = c.snapshot().await.unwrap();
    assert_eq!(snap.tasks[0].state, TaskState::NeedsYou);
    assert!(snap.tasks[0].child.is_none());

    // Nothing runs until the human says so.
    w.settle(1000).await;
    assert_eq!(w.count(started), 1);

    c.answer(AnswerParams {
        request: request.id.clone(),
        answer: AnswerKind::Change,
        text: Some("do not push, just report".into()),
        input: None,
        always: false,
    })
    .await
    .unwrap();
    let said = say_text(&w.wait("resumed", say_has("resumed with: ")).await);
    assert!(said.contains("continue"), "{said}");
    assert!(
        said.contains("Running git push origin sphere/x"),
        "the agent is told what was cut off: {said}"
    );
    assert!(
        said.contains("The user says: do not push, just report"),
        "{said}"
    );
    w.wait("turn", is_turn).await;
    assert_eq!(w.count(started), 2);
    assert!(c.yes(&request.id).await.unwrap_err().is_already_answered());
}

#[tokio::test]
async fn a_cut_off_resume_check_can_wait_or_be_stopped() {
    let mut env = Env::new("cutoff");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    let task = env.start_task(&c, "Cut off", "push").await;
    w.wait("the push step", |e| {
        matches!(&e.body, EventBody::Step { .. })
    })
    .await;
    env.host_call("shutdown");
    let EventBody::Request { request } = w.wait("resume check", is_request).await.body else {
        unreachable!()
    };
    c.later(&request.id).await.unwrap();
    w.settle(500).await;
    assert_eq!(w.count(started), 1, "later keeps it paused");
    // Restarting sphered while it waits does not ask twice.
    drop(w);
    env.kill_daemon();
    env.start_daemon().await;
    let c = env.client().await;
    let snap = c.snapshot().await.unwrap();
    assert_eq!(snap.requests.len(), 1);
    assert_eq!(snap.requests[0].info.id, request.id);
    c.stop(&task).await.unwrap();
    let snap = c.snapshot().await.unwrap();
    assert_eq!(snap.tasks[0].state, TaskState::Stopped);
    assert!(snap.requests.is_empty(), "stopping withdraws the check");
}

#[tokio::test]
async fn a_cut_off_test_run_just_continues() {
    let mut env = Env::new("cutoff_harmless");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Harmless", "test").await;
    w.wait(
        "the test step",
        |e| matches!(&e.body, EventBody::Step { title, .. } if title.contains("cargo test")),
    )
    .await;
    env.host_call("shutdown");
    let cut = w.wait("cut-off flag", is_kind("cut_off")).await;
    assert!(matches!(
        &cut.body,
        EventBody::CutOff {
            outbound: false,
            ..
        }
    ));
    let said = say_text(&w.wait("resumed", say_has("resumed with: ")).await);
    assert!(said.contains("Running cargo test"), "{said}");
    w.wait("turn", is_turn).await;
    assert_eq!(w.count(is_request), 0, "no question for something harmless");
    assert_eq!(w.count(started), 2);
}

#[tokio::test]
async fn follow_ups_idle_exit_and_idle_agents_do_not_hold_a_slot() {
    let mut env = Env::new("followup")
        .set("max_concurrent", "1")
        .set("idle_ttl_ms", "800");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    let a = env.start_task(&c, "Task A", "first").await;
    w.wait("first turn", is_turn).await;
    assert_eq!(c.snapshot().await.unwrap().tasks[0].state, TaskState::Done);
    assert!(
        c.snapshot().await.unwrap().tasks[0].child.is_some(),
        "the process stays warm"
    );

    // A finished task holds no slot: a second task starts at once with a limit of one.
    let b = env.start_task(&c, "Task B", "other").await;
    w.wait("B starts", |e| e.task == b && started(e)).await;

    // A follow-up goes to the live process.
    c.send(&a, "again please").await.unwrap();
    w.wait(
        "second turn text",
        say_has("second turn, you said: again please"),
    )
    .await;
    let second = w
        .wait_from(0, "A's second turn", |e| {
            e.task == a
                && matches!(&e.body, EventBody::Turn { summary, .. } if summary == "second done")
        })
        .await;
    assert!(
        matches!(second.body, EventBody::Turn { cost_usd: Some(c), .. } if (c - 0.03).abs() < 1e-9),
        "cumulative cost"
    );
    assert_eq!(w.count(|e| e.task == a && started(e)), 1, "same process");
    let user_msgs = w.count(|e| {
        e.task == a && matches!(&e.body, EventBody::UserMessage { text } if text == "again please")
    });
    assert_eq!(user_msgs, 1);

    // After the idle time the process exits, and the task is still just done.
    w.wait("idle exit of A", |e| e.task == a && exited("idle")(e))
        .await;
    let snap = c.snapshot().await.unwrap();
    let ta = snap.tasks.iter().find(|t| t.id == a).unwrap();
    assert_eq!(ta.state, TaskState::Done);
    assert!(ta.child.is_none());
    assert!((ta.cost_usd - 0.03).abs() < 1e-9, "{}", ta.cost_usd);

    // The next message resumes it.
    c.send(&a, "back again").await.unwrap();
    let restarted = w
        .wait_from(0, "A restarts", |e| {
            e.task == a && matches!(&e.body, EventBody::AgentStarted { resumed: true, .. })
        })
        .await;
    let args = w
        .wait_from(restarted.seq, "resume args", say_has("--resume"))
        .await;
    assert!(say_text(&args).contains("--resume fake-session-"));
    let third = w
        .wait_from(restarted.seq, "A's turn after resume", |e| {
            e.task == a && is_turn(e)
        })
        .await;
    assert!(matches!(third.body, EventBody::Turn { .. }));
    let snap = c.snapshot().await.unwrap();
    let ta = snap.tasks.iter().find(|t| t.id == a).unwrap();
    assert!(
        (ta.cost_usd - 0.04).abs() < 1e-9,
        "a new process starts its total at zero; only the new cost is added: {}",
        ta.cost_usd
    );
}

#[tokio::test]
async fn interrupt_ends_the_turn_and_the_agent_takes_the_next_message() {
    let mut env = Env::new("interrupt");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    let task = env.start_task(&c, "Interrupt me", "go").await;
    w.wait("working", say_has("working")).await;
    c.interrupt(&task).await.unwrap();
    let turn = w.wait("interrupted turn", is_turn).await;
    assert!(
        matches!(
            turn.body,
            EventBody::Turn {
                ok: false,
                interrupted: true,
                ..
            }
        ),
        "{:?}",
        turn.body
    );
    let t = c.snapshot().await.unwrap().tasks[0].clone();
    assert_eq!(t.state, TaskState::Done, "an interrupt is not a failure");
    assert_eq!(t.status, "Interrupted");
    assert!(t.child.is_some(), "the process lives on");

    c.send(&task, "go on").await.unwrap();
    w.wait(
        "next message handled",
        say_has("after the interrupt: go on"),
    )
    .await;
    let second = w
        .wait_from(turn.seq, "recovered", |e| {
            matches!(&e.body, EventBody::Turn { ok: true, .. })
        })
        .await;
    assert!(matches!(second.body, EventBody::Turn { ok: true, .. }));
}

#[tokio::test]
async fn interrupt_withdraws_open_requests() {
    let mut env = Env::new("stop"); // asks and then waits
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    let task = env.start_task(&c, "Interrupt an ask", "go").await;
    let EventBody::Request { request } = w.wait("an ask", is_request).await.body else {
        unreachable!()
    };
    c.interrupt(&task).await.unwrap();
    w.wait("withdrawn", |e| matches!(&e.body, EventBody::RequestWithdrawn { request: r, reason } if *r == request.id && reason == "interrupted")).await;
    assert!(c.snapshot().await.unwrap().requests.is_empty());
    assert_eq!(
        c.yes(&request.id).await.unwrap_err().kind(),
        Some("withdrawn")
    );
}

#[tokio::test]
async fn mode_and_model_changes_go_to_the_agent_as_control_requests() {
    let mut env = Env::new("controls");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    let task = env.start_task(&c, "Controls", "go").await;
    w.wait("ready", is_turn).await;

    c.set_mode(&task, "plan").await.unwrap();
    c.set_model(&task, "some-model").await.unwrap();
    w.wait(
        "mode event",
        |e| matches!(&e.body, EventBody::ModeChanged { mode } if mode == "plan"),
    )
    .await;
    w.wait(
        "model event",
        |e| matches!(&e.body, EventBody::ModelChanged { model } if model == "some-model"),
    )
    .await;
    c.send(&task, "report").await.unwrap();
    let said = say_text(&w.wait("control report", say_has("ctl: ")).await);
    let log: Value = serde_json::from_str(said.trim_start_matches("ctl: ")).unwrap();
    assert_eq!(
        log[0],
        serde_json::json!({"subtype": "set_permission_mode", "mode": "plan"})
    );
    assert_eq!(
        log[1],
        serde_json::json!({"subtype": "set_model", "model": "some-model"})
    );
    let t = c.snapshot().await.unwrap().tasks[0].clone();
    assert_eq!(t.mode.as_deref(), Some("plan"));
    assert_eq!(t.model.as_deref(), Some("some-model"));

    // Bad input and the unfinished rewind say so plainly.
    let err = c.set_mode(&task, "bypassPermissions").await.unwrap_err();
    assert!(format!("{err}").contains("unknown mode"), "{err}");
    assert_eq!(
        c.rewind(&task).await.unwrap_err().kind(),
        Some("not_implemented")
    );
    assert_eq!(
        c.set_mode("t99", "plan").await.unwrap_err().kind(),
        Some("not_found")
    );
}
