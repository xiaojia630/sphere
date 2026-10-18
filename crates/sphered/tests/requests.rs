//! Runs, requests and answers, end to end: sphered and sphere-host as real
//! processes, fake-claude as the agent, sphere-client as the user.

mod common;

use common::*;
use serde_json::{json, Value};
use sphere_client::AnswerParams;
use sphere_proto::api::{BlobReadParams, HistoryParams};
use sphere_proto::{AnswerKind, EventBody, RequestKind, RequestStatus, TaskState};

fn response_in(text: &str) -> Value {
    serde_json::from_str(
        text.split("response: ")
            .nth(1)
            .unwrap_or_else(|| panic!("no response in {text:?}")),
    )
    .unwrap()
}

#[tokio::test]
async fn a_run_streams_steps_output_files_and_cost() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    let task = env.start_task(&c, "Basic run", "do the thing").await;
    assert_eq!(task, "t1");
    let turn = w.wait("the turn to finish", is_turn).await;
    w.settle(300).await; // let the files refresh land

    // The command line the agent got.
    let args = say_text(&w.wait("args", say_has("args: ")).await);
    for want in [
        "-p",
        "--input-format stream-json",
        "--output-format stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-prompt-tool stdio",
        "--permission-mode acceptEdits",
        "--max-budget-usd 5",
        "--settings",
    ] {
        assert!(args.contains(want), "missing {want:?} in {args}");
    }
    assert!(!args.contains("--resume"));
    assert!(say_text(&w.wait("env", say_has("SPHERE_TASK=")).await).contains("SPHERE_TASK=t1"));

    // The story, in order.
    let kinds: Vec<&str> = w
        .log
        .iter()
        .filter(|e| e.task == "t1")
        .map(|e| e.body.kind_name())
        .collect();
    let pos = |k: &str| {
        kinds
            .iter()
            .position(|x| *x == k)
            .unwrap_or_else(|| panic!("no {k} in {kinds:?}"))
    };
    assert!(pos("task_created") < pos("workspace_ready"));
    assert!(pos("workspace_ready") < pos("agent_started"));
    assert!(pos("agent_started") < pos("init"));
    assert!(pos("init") < pos("step"));
    assert!(pos("step") < pos("step_done"));
    assert!(pos("step_done") < pos("turn"));
    assert!(
        w.log.windows(2).all(|p| p[0].seq < p[1].seq),
        "seq is strictly increasing"
    );

    // Init carried the slash commands; the session id was kept.
    let init = w.find(is_kind("init")).unwrap();
    match &init.body {
        EventBody::Init {
            slash_commands,
            model,
            ..
        } => {
            assert!(slash_commands.contains(&"/compact".to_string()));
            assert_eq!(model.as_deref(), Some("test-model"));
        }
        _ => unreachable!(),
    }
    assert!(w.find(is_kind("session_id")).is_some());

    // Typing deltas were broadcast live and never stored.
    let typed: String = w.live.iter().map(|f| f.text.as_str()).collect();
    assert_eq!(typed, "Let me look around.");
    let history = c
        .history(HistoryParams {
            task: Some("t1".into()),
            limit: Some(1000),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        history
            .iter()
            .all(|e| !format!("{:?}", e.body).contains("look around")),
        "deltas are not stored"
    );

    // Steps and their results, including the odd lines fake-claude threw in.
    let titles: Vec<String> = w
        .log
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::Step { title, .. } => Some(title.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        titles,
        vec![
            "Reading README.md",
            "Searching hello",
            "Editing notes.txt",
            "Editing README.md",
            "Running cargo test",
            "Running false"
        ]
    );
    assert!(
        w.find(say_has("I'll read the README")).is_some(),
        "an unknown block next to text is ignored"
    );
    let failed = w
        .log
        .iter()
        .filter(|e| matches!(&e.body, EventBody::StepDone { ok: false, .. }))
        .count();
    assert_eq!(failed, 1);

    // Big output: head and tail in the event, the rest by reference.
    let big = w
        .log
        .iter()
        .find_map(|e| match &e.body {
            EventBody::StepDone {
                id,
                output: Some(o),
                ..
            } if o.bytes == 6000 => Some((id.clone(), o.clone())),
            _ => None,
        })
        .expect("the 6000 byte output");
    assert!(big.1.tail.is_some() && big.1.head.len() < 3000);
    let src = big.1.source.expect("a reference to the full output");
    let full = c
        .blob_read(BlobReadParams {
            child: src.child,
            offset: src.offset,
            len: src.len,
            tool_use_id: Some(big.0),
        })
        .await
        .unwrap();
    assert_eq!(full.len(), 6000);
    assert!(full.starts_with("abcdefghij"));

    // Files touched come from git, so they include what the shell did too.
    let files = w.log.iter().rev().find_map(|e| match &e.body {
        EventBody::FilesTouched { files } => Some(files.clone()),
        _ => None,
    });
    let files = files.expect("files touched");
    let by_path = |p: &str| {
        files
            .iter()
            .find(|f| f.path == p)
            .unwrap_or_else(|| panic!("no {p} in {files:?}"))
            .clone()
    };
    assert_eq!(by_path("notes.txt").added, 2);
    assert_eq!(by_path("README.md").added, 1);

    match turn.body {
        EventBody::Turn {
            ok,
            cost_usd,
            turns,
            summary,
            ..
        } => {
            assert!(ok);
            assert_eq!(cost_usd, Some(0.02));
            assert_eq!(turns, Some(3));
            assert_eq!(summary, "Added notes.txt");
        }
        _ => unreachable!(),
    }

    // The view: done, on its own branch, in a worktree outside the repo.
    let snap = c.snapshot().await.unwrap();
    let t = &snap.tasks[0];
    assert_eq!(t.state, TaskState::Done);
    assert_eq!(t.branch.as_deref(), Some("sphere/basic-run"));
    assert!((t.cost_usd - 0.02).abs() < 1e-9 && (snap.cost_today_usd - 0.02).abs() < 1e-9);
    assert!(
        t.status.starts_with("Done: Added notes.txt"),
        "{}",
        t.status
    );
    let wt = std::path::PathBuf::from(t.workdir.as_ref().unwrap());
    // on macOS /tmp is a link to /private/tmp: compare real paths
    assert_eq!(
        wt.canonicalize().unwrap_or(wt.clone()),
        env.state
            .join("worktrees/repo/t1")
            .canonicalize()
            .unwrap_or(wt.clone())
    );
    assert!(wt.join("notes.txt").exists());
    assert!(
        !env.repo.join("notes.txt").exists(),
        "the checkout is untouched"
    );
    assert_eq!(
        git(&env.repo, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "main"
    );
    assert_eq!(
        git(&wt, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "sphere/basic-run"
    );
    assert_eq!(t.files.len(), 2);

    // Views survive a restart of sphered, rebuilt from the store.
    env.kill_daemon();
    env.start_daemon().await;
    let c2 = env.client().await;
    let snap2 = c2.snapshot().await.unwrap();
    assert_eq!(snap2.tasks[0].state, TaskState::Done);
    assert_eq!(snap2.tasks[0].cost_usd, t.cost_usd);
    assert!(snap2.seq >= snap.seq);
    let st = c2.status().await.unwrap();
    assert_eq!(st.tasks.len(), 1);
    assert!(st.host_pid.is_some());
}

#[tokio::test]
async fn a_permission_ask_becomes_a_request_and_yes_with_always_saves_the_rule() {
    let mut env = Env::new("permission");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    let task = env.start_task(&c, "Push work", "push it").await;

    let ev = w.wait("a request", is_request).await;
    let EventBody::Request { request } = &ev.body else {
        unreachable!()
    };
    assert_eq!(request.kind, RequestKind::Permission);
    assert_eq!(request.tool, "Bash");
    assert_eq!(
        request.input,
        json!({"command": "git push -u origin sphere/work"}),
        "the exact input"
    );
    assert_eq!(request.summary, "Push the branch");
    assert_eq!(request.display_name.as_deref(), Some("Run shell command"));
    assert_eq!(request.suggestions.len(), 1);
    assert_eq!(request.revises, None);

    // While it waits, the task needs you and the status line says what for.
    let snap = c.snapshot().await.unwrap();
    assert_eq!(snap.tasks[0].state, TaskState::NeedsYou);
    assert_eq!(
        snap.tasks[0].status,
        "Needs you: Bash git push -u origin sphere/work"
    );
    assert_eq!(snap.requests.len(), 1);
    assert_eq!(snap.requests[0].info.id, request.id);
    let st = c.status().await.unwrap();
    assert_eq!(st.needs_you, 1);
    assert_eq!(st.requests[0].id, request.id);
    assert_eq!(st.active, 1, "a task waiting on you still holds its slot");

    // Answer: yes, and remember it.
    c.answer(AnswerParams {
        request: request.id.clone(),
        answer: AnswerKind::Yes,
        text: None,
        input: None,
        always: true,
    })
    .await
    .unwrap();
    let said = say_text(&w.wait("the response echo", say_has("response: ")).await);
    let resp = response_in(&said);
    assert_eq!(resp["behavior"], "allow");
    assert_eq!(
        resp["updatedInput"]["command"],
        "git push -u origin sphere/work"
    );
    assert_eq!(
        resp["updatedPermissions"][0]["destination"], "session",
        "the rule is for this session only"
    );
    assert_eq!(
        resp["updatedPermissions"][0]["rules"][0]["ruleContent"],
        "git push:*"
    );
    w.wait("Pushed.", say_has("Pushed.")).await;
    w.wait("the turn", is_turn).await;

    // First answer wins: a second gets `already_answered`.
    let err = c.yes(&request.id).await.unwrap_err();
    assert!(err.is_already_answered(), "{err}");
    let err = c.change(&request.id, "too late").await.unwrap_err();
    assert!(err.is_already_answered());
    let err = c.yes("r999").await.unwrap_err();
    assert_eq!(err.kind(), Some("not_found"));

    // The rule landed in Sphere's policy file.
    let policy = std::fs::read_to_string(env.state.join("policy.toml")).unwrap();
    assert!(policy.contains("Bash(git push:*)"), "{policy}");
    let answered = w.find(is_kind("request_answered")).unwrap();
    assert!(matches!(
        &answered.body,
        EventBody::RequestAnswered {
            always: true,
            answer: AnswerKind::Yes,
            ..
        }
    ));
    let snap = c.snapshot().await.unwrap();
    assert!(snap.requests.is_empty());
    assert_eq!(snap.tasks[0].state, TaskState::Done);
    let _ = task;
}

#[tokio::test]
async fn yes_can_edit_the_input() {
    let mut env = Env::new("permission");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Edit it", "go").await;
    let EventBody::Request { request } = w.wait("a request", is_request).await.body else {
        unreachable!()
    };
    c.answer(AnswerParams {
        request: request.id,
        answer: AnswerKind::Yes,
        text: None,
        input: Some(json!({"command": "git push origin sphere/other"})),
        always: false,
    })
    .await
    .unwrap();
    let resp = response_in(&say_text(&w.wait("echo", say_has("response: ")).await));
    assert_eq!(
        resp["updatedInput"]["command"],
        "git push origin sphere/other"
    );
    assert!(resp.get("updatedPermissions").is_none());
    assert!(!std::fs::read_to_string(env.state.join("policy.toml"))
        .unwrap_or_default()
        .contains("git push"));
}

#[tokio::test]
async fn change_denies_with_the_users_words_and_the_next_ask_revises_it() {
    let mut env = Env::new("change");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Change it", "go").await;

    let EventBody::Request { request: first } = w.wait("first request", is_request).await.body
    else {
        unreachable!()
    };
    assert_eq!(
        first.input["command"],
        "git push --set-upstream origin sphere/work"
    );
    c.change(&first.id, "Don't set the upstream; push the plain branch.")
        .await
        .unwrap();

    let resp = response_in(&say_text(&w.wait("deny echo", say_has("response: ")).await));
    assert_eq!(resp["behavior"], "deny");
    assert_eq!(
        resp["message"],
        "Don't set the upstream; push the plain branch."
    );

    // The revised ask arrives as a new request that points at the old one.
    let second = w
        .wait_from(
            0,
            "the revised request",
            |e| matches!(&e.body, EventBody::Request { request } if request.id != first.id),
        )
        .await;
    let EventBody::Request { request: second } = second.body else {
        unreachable!()
    };
    assert_eq!(second.revises.as_deref(), Some(first.id.as_str()));
    assert_eq!(second.input["command"], "git push origin sphere/work");
    let snap = c.snapshot().await.unwrap();
    assert_eq!(
        snap.requests.len(),
        1,
        "the old card is gone, the new one stands in"
    );
    assert_eq!(snap.requests[0].info.id, second.id);
    assert_eq!(
        snap.requests[0].info.revises.as_deref(),
        Some(first.id.as_str())
    );

    c.yes(&second.id).await.unwrap();
    w.wait("revised push", say_has("Pushed the revised command."))
        .await;
    w.wait("turn", is_turn).await;
    // A change with no words still tells the agent something.
    assert!(c
        .change(&first.id, "x")
        .await
        .unwrap_err()
        .is_already_answered());
}

#[tokio::test]
async fn later_keeps_the_request_open_and_the_agent_paused() {
    let mut env = Env::new("permission");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Later", "go").await;
    let EventBody::Request { request } = w.wait("request", is_request).await.body else {
        unreachable!()
    };

    c.later(&request.id).await.unwrap();
    w.wait("snooze", is_kind("request_snoozed")).await;
    w.settle(500).await;
    assert!(
        w.find(say_has("response: ")).is_none(),
        "the agent got no answer"
    );
    let snap = c.snapshot().await.unwrap();
    assert_eq!(snap.requests.len(), 1);
    assert_eq!(snap.requests[0].status, RequestStatus::Snoozed);
    assert_eq!(snap.tasks[0].state, TaskState::NeedsYou);
    // Later again is fine.
    c.later(&request.id).await.unwrap();

    // Its later; now yes.
    c.yes(&request.id).await.unwrap();
    w.wait("Pushed.", say_has("Pushed.")).await;
    w.wait("turn", is_turn).await;
    assert!(c.snapshot().await.unwrap().requests.is_empty());
}

#[tokio::test]
async fn questions_and_plans_are_requests_with_their_own_kinds() {
    let mut env = Env::new("question");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Questions", "go").await;

    let EventBody::Request { request: q } = w.wait("question", is_request).await.body else {
        unreachable!()
    };
    assert_eq!(q.kind, RequestKind::Question);
    assert_eq!(q.input["questions"][0]["question"], "Which database?");
    c.answer(AnswerParams {
        request: q.id.clone(),
        answer: AnswerKind::Yes,
        text: Some("SQLite".into()),
        input: None,
        always: false,
    })
    .await
    .unwrap();
    let resp = response_in(&say_text(
        &w.wait("answer echo", say_has("response: ")).await,
    ));
    assert_eq!(resp["updatedInput"]["answers"]["Which database?"], "SQLite");
    assert_eq!(
        resp["updatedInput"]["questions"][0]["header"], "DB",
        "the rest of the input is kept"
    );

    let plan = w
        .wait_from(0, "plan request", |e| matches!(&e.body, EventBody::Request { request } if request.kind == RequestKind::Plan))
        .await;
    let EventBody::Request { request: p } = plan.body else {
        unreachable!()
    };
    assert!(p.input["plan"].as_str().unwrap().contains("Migrate"));
    c.change(&p.id, "Also add an index.").await.unwrap();
    w.wait(
        "plan deny echo",
        |e| matches!(&e.body, EventBody::Say { text } if text.contains("Also add an index")),
    )
    .await;
    w.wait("turn", is_turn).await;
}
