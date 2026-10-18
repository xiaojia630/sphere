//! The `sphere` binary against a real sphered.

mod common;

use common::*;
use std::process::Command;

fn sphere(env: &Env, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(&bins().sphere)
        .arg("--socket")
        .arg(&env.socket)
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("run sphere");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_follow_prints_the_run_and_ls_lists_it() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let repo = env.repo.to_string_lossy().to_string();
    let (ok, out, err) = tokio::task::block_in_place(|| {
        sphere(
            &env,
            &[
                "run",
                &repo,
                "do the thing",
                "--title",
                "CLI run",
                "--follow",
            ],
        )
    });
    assert!(ok, "{out}\n{err}");
    assert!(out.contains("t1"), "{out}");
    assert!(out.contains("All done."), "{out}");
    let (ok, out, _) = tokio::task::block_in_place(|| sphere(&env, &["ls"]));
    assert!(ok);
    assert!(out.contains("CLI run") && out.contains("t1"), "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_is_answered_from_the_command_line() {
    let mut env = Env::new("permission");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Ask", "go").await;
    let ev = w.wait("request", is_request).await;
    let sphere_proto::EventBody::Request { request } = ev.body else {
        unreachable!()
    };
    let (_, ls, _) = tokio::task::block_in_place(|| sphere(&env, &["ls"]));
    assert!(ls.contains(&request.id), "ls shows what waits on you: {ls}");
    let (ok, out, err) =
        tokio::task::block_in_place(|| sphere(&env, &["answer", &request.id, "yes"]));
    assert!(ok, "{out}\n{err}");
    w.wait("the turn", is_turn).await;
    // A second answer says it was too late.
    let (ok, _, err) =
        tokio::task::block_in_place(|| sphere(&env, &["answer", &request.id, "yes"]));
    assert!(!ok);
    assert!(err.contains("already"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_and_stop_work_and_unknown_tasks_error() {
    let mut env = Env::new("followup");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Chat", "go").await;
    w.wait("turn", is_turn).await;
    let (ok, _, err) = tokio::task::block_in_place(|| sphere(&env, &["send", "t1", "next please"]));
    assert!(ok, "{err}");
    w.wait("second turn", say_has("second turn, you said: next please"))
        .await;
    let (ok, _, _) = tokio::task::block_in_place(|| sphere(&env, &["stop", "t1"]));
    assert!(ok);
    w.wait("stopped", is_kind("stopped")).await;
    let (ok, _, err) = tokio::task::block_in_place(|| sphere(&env, &["stop", "t42"]));
    assert!(!ok);
    assert!(err.contains("t42"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_reports_and_a_missing_daemon_is_explained() {
    let env = Env::new("basic");
    // No daemon yet.
    let (ok, out, err) = tokio::task::block_in_place(|| sphere(&env, &["ls"]));
    assert!(!ok);
    assert!(format!("{out}{err}").contains("sphered"), "{out}{err}");
    let agent = bins().fake.to_string_lossy().to_string();
    let state = env.state.to_string_lossy().to_string();
    let (_, out, err) = tokio::task::block_in_place(|| {
        sphere(&env, &["doctor", "--agent", &agent, "--state-dir", &state])
    });
    let all = format!("{out}{err}");
    assert!(all.contains("git"), "{all}");
    assert!(all.to_lowercase().contains("sphered"), "{all}");
}
