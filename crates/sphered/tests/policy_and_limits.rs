//! Policy compiled into settings, the rule engine, limits, the queue, the
//! workspace, and the failure paths.

mod common;

use common::*;
use serde_json::Value;
use sphere_client::AnswerParams;
use sphere_proto::{AnswerKind, EventBody, RequestKind, TaskState};

fn settings_of(text: &str) -> Value {
    serde_json::from_str(text.split("settings: ").nth(1).expect("settings in say")).unwrap()
}

fn commit_all(env: &Env, msg: &str) {
    git(&env.repo, &["add", "-A"]);
    git(&env.repo, &["commit", "-q", "-m", msg]);
}

async fn run_settings(env: &Env) -> (Value, String) {
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Settings", "go").await;
    let s = say_text(&w.wait("settings", say_has("settings: ")).await);
    let a = say_text(&w.wait("args", say_has("args: ")).await);
    w.wait("turn", is_turn).await;
    (settings_of(&s), a)
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn policy_is_compiled_into_settings_with_the_sandbox_on() {
    let mut env = Env::new("settings");
    env.write_state_file(
        "policy.toml",
        "[sandbox]\nallowed_domains = [\"example.com\"]\n[rules]\nallow = [\"Bash(cargo test:*)\"]\ndeny = [\"Read(./.env)\"]\nask = [\"Bash(git push:*)\"]\n",
    );
    env.start_daemon().await;
    let (s, args) = run_settings(&env).await;
    assert_eq!(s["sandbox"]["enabled"], true);
    let domains = strs(&s["sandbox"]["network"]["allowedDomains"]);
    assert!(domains.contains(&"example.com".to_string()), "{domains:?}");
    assert!(
        domains.contains(&"crates.io".to_string()),
        "the package registries stay reachable: {domains:?}"
    );
    assert!(strs(&s["permissions"]["allow"]).contains(&"Bash(cargo test:*)".to_string()));
    assert!(strs(&s["permissions"]["deny"]).contains(&"Read(./.env)".to_string()));
    let deny = strs(&s["permissions"]["deny"]);
    assert!(
        deny.iter().any(|d| d.contains("--force")),
        "force pushes are denied by default: {deny:?}"
    );
    assert!(args.contains("--settings"));
}

#[tokio::test]
async fn allow_rewrites_lifts_only_the_rewrite_denies() {
    let mut env = Env::new("settings");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    c.start_task(sphere_client::StartParams {
        repo: env.repo.to_string_lossy().to_string(),
        prompt: "go".into(),
        title: Some("Rewrites".into()),
        allow_rewrites: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let s = settings_of(&say_text(&w.wait("settings", say_has("settings: ")).await));
    let deny = strs(&s["permissions"]["deny"]);
    assert!(!deny.iter().any(|d| d.contains("--force")), "{deny:?}");
    assert_eq!(s["sandbox"]["enabled"], true);
}

#[tokio::test]
async fn the_sandbox_can_be_turned_off_by_the_user_policy() {
    let mut env = Env::new("settings");
    env.write_state_file("policy.toml", "[sandbox]\nenabled = false\n");
    env.start_daemon().await;
    let (s, _) = run_settings(&env).await;
    assert_eq!(s["sandbox"]["enabled"], false);
}

#[tokio::test]
async fn a_repo_policy_only_restricts_unless_the_user_trusts_repos() {
    let repo_policy = "[sandbox]\nenabled = false\nallowed_domains = [\"repo.example\"]\n[rules]\nallow = [\"Bash(rm:*)\"]\ndeny = [\"Bash(curl:*)\"]\n";
    // Untrusted.
    let mut env = Env::new("settings");
    std::fs::create_dir_all(env.repo.join(".sphere")).unwrap();
    std::fs::write(env.repo.join(".sphere/policy.toml"), repo_policy).unwrap();
    commit_all(&env, "policy");
    env.start_daemon().await;
    let (s, _) = run_settings(&env).await;
    assert_eq!(
        s["sandbox"]["enabled"], true,
        "a repo cannot switch the sandbox off"
    );
    let domains = strs(&s["sandbox"]["network"]["allowedDomains"]);
    assert!(
        !domains.contains(&"repo.example".to_string()),
        "a repo cannot add hosts: {domains:?}"
    );
    assert!(domains.contains(&"crates.io".to_string()), "{domains:?}");
    assert!(!strs(&s["permissions"]["allow"]).contains(&"Bash(rm:*)".to_string()));
    assert!(
        strs(&s["permissions"]["deny"]).contains(&"Bash(curl:*)".to_string()),
        "but it can add denials"
    );
    drop(env);

    // Trusted.
    let mut env = Env::new("settings");
    env.write_state_file("policy.toml", "trust_repos = true\n");
    std::fs::create_dir_all(env.repo.join(".sphere")).unwrap();
    std::fs::write(env.repo.join(".sphere/policy.toml"), repo_policy).unwrap();
    commit_all(&env, "policy");
    env.start_daemon().await;
    let (s, _) = run_settings(&env).await;
    assert_eq!(s["sandbox"]["enabled"], false);
    assert!(strs(&s["sandbox"]["network"]["allowedDomains"]).contains(&"repo.example".to_string()));
    assert!(strs(&s["permissions"]["allow"]).contains(&"Bash(rm:*)".to_string()));
}

#[tokio::test]
async fn tier_two_rules_settle_a_request_by_branch_and_the_rest_still_ask() {
    let mut env = Env::new("tier2");
    env.write_state_file(
        "policy.toml",
        "[[auto]]\ntool = \"Bash\"\npattern = \"git push origin {branch}\"\ndecision = \"allow\"\nname = \"own branch\"\n",
    );
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Tier two", "go").await;
    // The push to the task's own branch is answered without anyone.
    let auto = w.wait("auto answer", is_kind("auto_answered")).await;
    assert!(
        matches!(&auto.body, EventBody::AutoAnswered { .. }),
        "{auto:?}"
    );
    // The push to main is not covered: it asks.
    let ev = w.wait("a request", is_request).await;
    let EventBody::Request { request } = ev.body else {
        unreachable!()
    };
    assert_eq!(request.kind, RequestKind::Permission);
    assert!(request.input["command"]
        .as_str()
        .unwrap()
        .contains("origin main"));
    assert_eq!(w.count(is_request), 1, "only one request reached the human");
    c.answer(AnswerParams {
        request: request.id,
        answer: AnswerKind::Yes,
        text: None,
        input: None,
        always: false,
    })
    .await
    .unwrap();
    w.wait("main allowed", say_has("main allowed")).await;
    w.wait("turn", is_turn).await;
}

#[tokio::test]
async fn a_tier_two_deny_rule_refuses_without_asking() {
    let mut env = Env::new("tier2");
    env.write_state_file(
        "policy.toml",
        "[[auto]]\ntool = \"Bash\"\npattern = \"git push origin main\"\ndecision = \"deny\"\n[[auto]]\ntool = \"Bash\"\npattern = \"git push origin sphere/*\"\ndecision = \"allow\"\n",
    );
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Tier two", "go").await;
    w.wait("main denied", say_has("main denied")).await;
    w.wait("turn", is_turn).await;
    assert_eq!(w.count(is_request), 0);
    assert_eq!(w.count(is_kind("auto_answered")), 2);
}

#[tokio::test]
async fn an_agent_cannot_answer_requests_through_the_socket() {
    let mut env = Env::new("rpc");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Rogue", "go").await;
    let said = say_text(&w.wait("rpc answer", say_has("rpc request.answer: ")).await);
    assert!(said.contains("forbidden"), "{said}");
    // Reading is fine.
    let snap = say_text(&w.wait("rpc snapshot", say_has("rpc snapshot: ")).await);
    assert!(snap.contains("\"result\""), "{snap}");
    w.wait("turn", is_turn).await;
}

#[tokio::test]
async fn the_assistant_client_kind_cannot_answer() {
    let mut env = Env::new("permission");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Ask", "go").await;
    let ev = w.wait("request", is_request).await;
    let EventBody::Request { request } = ev.body else {
        unreachable!()
    };
    let assistant = sphere_client::Client::connect(&env.socket).await.unwrap();
    assistant
        .hello(sphere_client::ClientKind::Assistant)
        .await
        .unwrap();
    let err = assistant.yes(&request.id).await.unwrap_err();
    assert_eq!(err.kind(), Some("forbidden"));
    c.yes(&request.id).await.unwrap();
}

#[tokio::test]
async fn the_queue_holds_tasks_beyond_the_limit_and_starts_them_in_order() {
    let mut env = Env::new("slow").set("max_concurrent", "1");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "First", "go").await;
    env.start_task(&c, "Second", "go").await;
    env.start_task(&c, "Third", "go").await;
    let c2 = &c;
    eventually("the first task to start", 10, || async move {
        let snap = c2.snapshot().await.unwrap();
        let states: Vec<TaskState> = snap.tasks.iter().map(|t| t.state).collect();
        (states.iter().filter(|s| **s == TaskState::Queued).count() == 2
            && states[0] == TaskState::Working)
            .then_some(())
    })
    .await;
    assert_eq!(c.status().await.unwrap().queued, 2);
    let finished = |task: &'static str| move |e: &sphere_proto::Event| e.task == task && is_turn(e);
    w.wait("first", finished("t1")).await;
    w.wait("second", finished("t2")).await;
    w.wait("third", finished("t3")).await;
    let started: Vec<String> = w
        .log
        .iter()
        .filter(|e| matches!(e.body, EventBody::AgentStarted { .. }))
        .map(|e| e.task.clone())
        .collect();
    assert_eq!(started, ["t1", "t2", "t3"]);
}

#[tokio::test]
async fn the_daily_cap_refuses_new_work_once_spent() {
    let mut env = Env::new("slow").set("daily_cap_usd", "0.4");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Spender", "go").await;
    w.wait("turn", is_turn).await;
    let st = c.status().await.unwrap();
    assert!(st.cost_today_usd >= 0.5 - 1e-9);
    let err = c
        .start_task(sphere_client::StartParams {
            repo: env.repo.to_string_lossy().to_string(),
            prompt: "more".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(err.kind(), Some("daily_cap"));
    let err = c.send("t1", "and more").await.unwrap_err();
    assert_eq!(err.kind(), Some("daily_cap"));
}

#[tokio::test]
async fn an_agent_that_dies_fails_the_task_with_its_stderr() {
    let mut env = Env::new("fail");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Doomed", "go").await;
    let ev = w.wait("failure", is_kind("failed")).await;
    let EventBody::Failed { message } = ev.body else {
        unreachable!()
    };
    assert!(message.contains("boom: something broke"), "{message}");
    assert_eq!(
        c.snapshot().await.unwrap().tasks[0].state,
        TaskState::Failed
    );
}

#[tokio::test]
async fn bad_starts_are_refused_with_clear_errors() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let c = env.client().await;
    let err = c
        .start_task(sphere_client::StartParams {
            repo: "/nonexistent/place".into(),
            prompt: "x".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(err.kind(), Some("bad_repo"));
    let plain = env.root().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let err = c
        .start_task(sphere_client::StartParams {
            repo: plain.to_string_lossy().to_string(),
            prompt: "x".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(err.kind(), Some("bad_repo"), "a worktree needs a git repo");
    let err = c
        .start_task(sphere_client::StartParams {
            repo: env.repo.to_string_lossy().to_string(),
            prompt: "  ".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(err.to_string().to_lowercase().contains("prompt"), "{err}");
    assert!(
        c.snapshot().await.unwrap().tasks.is_empty(),
        "nothing was created"
    );
    assert!(c.send("t9", "hi").await.unwrap_err().kind() == Some("not_found"));
    assert!(c.rewind("t9").await.is_err());
}

#[tokio::test]
async fn a_worktree_copies_files_runs_setup_and_reports_touched_files() {
    let mut env = Env::new("basic");
    std::fs::create_dir_all(env.repo.join(".sphere")).unwrap();
    std::fs::write(
        env.repo.join(".sphere/config.toml"),
        "copy = [\".env\"]\nsetup = \"echo ready > setup.out\"\n",
    )
    .unwrap();
    std::fs::write(env.repo.join(".gitignore"), ".env\nsetup.out\n").unwrap();
    commit_all(&env, "sphere config");
    std::fs::write(env.repo.join(".env"), "SECRET=1\n").unwrap();
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    let task = env.start_task(&c, "Fix the parser", "go").await;
    let ev = w.wait("workspace", is_kind("workspace_ready")).await;
    let EventBody::WorkspaceReady {
        workdir, branch, ..
    } = ev.body
    else {
        unreachable!()
    };
    assert!(workdir.contains("worktrees"), "{workdir}");
    assert!(
        workdir.ends_with(&task) || workdir.contains(&task),
        "{workdir}"
    );
    assert_eq!(branch, None, "no branch until files change");
    assert_eq!(
        std::fs::read_to_string(format!("{workdir}/.env")).unwrap(),
        "SECRET=1\n"
    );
    assert_eq!(
        std::fs::read_to_string(format!("{workdir}/setup.out"))
            .unwrap()
            .trim(),
        "ready"
    );
    w.wait("turn", is_turn).await;
    w.settle(300).await;
    let files = w.log.iter().rev().find_map(|e| match &e.body {
        EventBody::FilesTouched { files } => Some(files.clone()),
        _ => None,
    });
    let files = files.expect("files touched");
    let branches: Vec<Option<String>> = w
        .log
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::WorkspaceReady { branch, .. } => Some(branch.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        branches.last().cloned().flatten().as_deref(),
        Some("sphere/fix-the-parser"),
        "{branches:?}"
    );
    let names: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
    assert!(
        names.contains(&"notes.txt".to_string()) && names.contains(&"README.md".to_string()),
        "{names:?}"
    );
    assert_eq!(
        git(&env.repo, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "main",
        "the main checkout is untouched"
    );
    assert!(!env.repo.join("notes.txt").exists());
}

#[tokio::test]
async fn a_failing_setup_command_fails_the_task_before_any_agent_runs() {
    let mut env = Env::new("basic");
    std::fs::create_dir_all(env.repo.join(".sphere")).unwrap();
    std::fs::write(
        env.repo.join(".sphere/config.toml"),
        "setup = \"echo nope >&2; exit 4\"\n",
    )
    .unwrap();
    commit_all(&env, "sphere config");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Bad setup", "go").await;
    let ev = w.wait("failure", is_kind("failed")).await;
    let EventBody::Failed { message } = ev.body else {
        unreachable!()
    };
    assert!(message.contains("nope"), "{message}");
    assert_eq!(w.count(is_kind("agent_started")), 0);
}

#[tokio::test]
async fn in_place_runs_in_the_repo_itself() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    c.start_task(sphere_client::StartParams {
        repo: env.repo.to_string_lossy().to_string(),
        prompt: "go".into(),
        in_place: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let ev = w.wait("workspace", is_kind("workspace_ready")).await;
    let EventBody::WorkspaceReady {
        workdir, branch, ..
    } = ev.body
    else {
        unreachable!()
    };
    assert_eq!(
        std::fs::canonicalize(&workdir).unwrap(),
        std::fs::canonicalize(&env.repo).unwrap()
    );
    assert!(branch.is_none() || branch.as_deref() == Some("main"));
    w.wait("turn", is_turn).await;
    assert!(env.repo.join("notes.txt").exists());
}

#[tokio::test]
async fn budget_mode_and_model_reach_the_agent() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    c.start_task(sphere_client::StartParams {
        repo: env.repo.to_string_lossy().to_string(),
        prompt: "go".into(),
        budget_usd: Some(1.25),
        mode: Some("plan".into()),
        model: Some("some-model".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    let args = say_text(&w.wait("args", say_has("args: ")).await);
    assert!(args.contains("--max-budget-usd 1.25"), "{args}");
    assert!(args.contains("--permission-mode plan"), "{args}");
    assert!(args.contains("--model some-model"), "{args}");
}

#[tokio::test]
async fn auth_and_version_problems_are_warnings_not_failures() {
    let data = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(data.path(), r#"{"loggedIn":false}"#).unwrap();
    let bins = bins();
    let wrapper = tempfile::Builder::new()
        .prefix("claude-wrap")
        .tempdir_in("/tmp")
        .unwrap();
    let script = wrapper.path().join("claude");
    std::fs::write(&script, format!("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo '9.9.9 (Claude Code)'; exit 0; fi\nexec {:?} \"$@\"\n", bins.fake)).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let fake = bins.fake.to_string_lossy().to_string();
    let mut env = Env::new("basic")
        .set_str("agent_command", &script.to_string_lossy())
        .set(
            "auth_status_command",
            &format!(
                "[{fake:?}, \"auth\", \"status\", \"--data\", {:?}]",
                data.path().to_string_lossy()
            ),
        )
        .set("check_version", "true");
    env.start_daemon().await;
    let c = env.client().await;
    // the sign-in and version checks run in the background after start: wait for them
    let mut st = c.status().await.unwrap();
    for _ in 0..100 {
        if st.signed_in.is_some() && st.warnings.iter().any(|w| w.contains("9.9.9")) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        st = c.status().await.unwrap();
    }
    assert_eq!(st.signed_in, Some(false));
    assert!(
        st.warnings.iter().any(|w| w.contains("signed out")),
        "{:?}",
        st.warnings
    );
    assert!(
        st.warnings.iter().any(|w| w.contains("9.9.9")),
        "{:?}",
        st.warnings
    );
    assert_eq!(st.claude_version.as_deref(), Some("9.9.9 (Claude Code)"));
    // And a task still runs.
    let mut w = env.watcher(0, None).await;
    w.wait("the warning event", is_kind("warning")).await;
    env.start_task(&c, "Still works", "go").await;
    w.wait("turn", is_turn).await;
}

#[tokio::test]
async fn sessions_are_listed_and_a_running_one_cannot_be_brought_in() {
    let projects = tempfile::tempdir().unwrap();
    let dir = projects.path().join("-tmp-proj");
    std::fs::create_dir_all(&dir).unwrap();
    let mut env = Env::new("followup");
    let cwd = env.repo.to_string_lossy().to_string();
    std::fs::write(
        dir.join("sess-old.jsonl"),
        format!("{{\"type\":\"user\",\"cwd\":{cwd:?},\"message\":{{\"role\":\"user\",\"content\":\"Earlier work on the parser\"}}}}\n"),
    )
    .unwrap();
    let live_pid = std::process::id();
    let agents = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        agents.path(),
        format!("[{{\"sessionId\":\"sess-live\",\"pid\":{live_pid},\"cwd\":{cwd:?},\"state\":\"working\",\"name\":\"Live one\"}}]"),
    )
    .unwrap();
    let fake = bins().fake.to_string_lossy().to_string();
    env = env
        .set_str("claude_projects_dir", &projects.path().to_string_lossy())
        .set(
            "session_list_command",
            &format!(
                "[{fake:?}, \"agents\", \"--json\", \"--data\", {:?}]",
                agents.path().to_string_lossy()
            ),
        );
    env.start_daemon().await;
    let c = env.client().await;
    let list = c.session_list().await.unwrap();
    let old = list
        .iter()
        .find(|s| s.session_id == "sess-old")
        .expect("transcript session");
    assert!(!old.running);
    assert_eq!(old.title.as_deref(), Some("Earlier work on the parser"));
    let live = list
        .iter()
        .find(|s| s.session_id == "sess-live")
        .expect("agents session");
    assert!(live.running);

    let err = c
        .bring_in(sphere_client::BringInParams {
            session_id: "sess-live".into(),
            cwd: None,
            prompt: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.kind(), Some("session_running"));
    let err = c
        .bring_in(sphere_client::BringInParams {
            session_id: "nope".into(),
            cwd: None,
            prompt: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.kind(), Some("not_found"));

    let mut w = env.watcher(0, None).await;
    let task = c
        .bring_in(sphere_client::BringInParams {
            session_id: "sess-old".into(),
            cwd: None,
            prompt: Some("pick it up".into()),
        })
        .await
        .unwrap();
    let args = say_text(&w.wait("args", say_has("args: ")).await);
    assert!(args.contains("--resume sess-old"), "{args}");
    w.wait("turn", is_turn).await;
    let list = c.session_list().await.unwrap();
    assert_eq!(
        list.iter()
            .find(|s| s.session_id == "sess-old")
            .unwrap()
            .task
            .as_deref(),
        Some(task.as_str())
    );
}
