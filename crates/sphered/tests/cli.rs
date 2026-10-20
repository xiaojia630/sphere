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
async fn protocols_are_written_listed_run_turned_off_and_removed_from_the_command_line() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    let repo = env.repo.to_string_lossy().to_string();
    let run = |args: &[&str]| tokio::task::block_in_place(|| sphere(&env, args));

    let line = format!("every weekday 9:00 {repo}: update the dependencies, run the tests");
    let (ok, out, err) = run(&["protocol", "add", &line]);
    assert!(ok, "{out}\n{err}");
    assert!(
        out.starts_with("saved p1: every weekday 09:00 · next ")
            && out.trim_end().ends_with(" · repo"),
        "{out}"
    );
    // A line with no repo takes the one given beside it.
    let (ok, out, err) = run(&["protocol", "add", "--repo", &repo, "every 2 hours: tidy up"]);
    assert!(ok, "{out}\n{err}");
    assert!(out.starts_with("saved p2: every 2 hours · next "), "{out}");

    let (ok, out, _) = run(&["protocol", "list"]);
    assert!(ok);
    assert!(
        out.contains("p1")
            && out.contains("every weekday 09:00")
            && out.contains("update the dependencies"),
        "{out}"
    );
    assert!(out.contains("last: not run yet"), "{out}");

    let (ok, out, _) = run(&["protocol", "off", "p2"]);
    assert!(ok && out.contains("p2 is off"), "{out}");
    let (_, out, _) = run(&["protocol", "ls"]);
    assert!(out.contains("p2   off"), "{out}");
    let (ok, out, _) = run(&["protocol", "on", "p2"]);
    assert!(ok && out.starts_with("saved p2: every 2 hours"), "{out}");

    // Run now: a task like any other, and `ls` says which protocol started it.
    let (ok, out, err) = run(&["protocol", "run", "p1"]);
    assert!(ok, "{out}\n{err}");
    assert!(out.contains("started t1"), "{out}");
    w.wait("the run to finish", is_turn).await;
    let (_, ls, _) = run(&["ls"]);
    assert!(ls.contains("(protocol p1)"), "{ls}");
    let (_, out, _) = run(&["protocol", "list"]);
    assert!(out.contains("last: done (t1) ·"), "{out}");

    // What cannot be done says why.
    let (ok, _, err) = run(&["protocol", "add", "evry day 9:00 x: y"]);
    assert!(!ok);
    assert!(err.contains("didn't understand 'evry'"), "{err}");
    let (ok, _, err) = run(&["protocol", "run", "p9"]);
    assert!(!ok && err.contains("p9"), "{err}");

    let (ok, out, _) = run(&["protocol", "rm", "p1"]);
    assert!(ok && out.contains("removed p1"), "{out}");
    let (_, out, _) = run(&["protocol", "list"]);
    assert!(!out.contains("p1 ") && out.contains("p2"), "{out}");
    assert_eq!(c.protocol_list().await.unwrap().len(), 1);
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
