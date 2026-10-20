//! The user's notes: stored in sphered, told to each agent it starts in their repo, and kept current for
//! every client.

mod common;

use common::*;
use serde_json::json;
use sphere_client::Client;
use sphere_proto::api::{self, method};
use sphere_proto::{Event, EventBody, NoteSource};
use std::path::Path;
use std::process::Command;

const HEADER: &str =
    "Notes from the user for this repository. Follow them unless the task says otherwise:";

fn started(e: &Event) -> bool {
    matches!(e.body, EventBody::AgentStarted { .. })
}

fn briefed(e: &Event) -> u32 {
    match e.body {
        EventBody::AgentStarted { briefed, .. } => briefed,
        _ => panic!("not an agent start: {e:?}"),
    }
}

fn notes_changed(e: &Event) -> bool {
    matches!(e.body, EventBody::MemoryChanged { .. })
}

/// A second repository next to the one the harness made.
fn other_repo(env: &Env) -> String {
    let dir = env.root().join("other");
    init_repo(&dir);
    dir.to_string_lossy().to_string()
}

async fn start_in(c: &Client, repo: &str, title: &str) -> String {
    c.start_task(sphere_client::StartParams {
        repo: repo.into(),
        prompt: "go".into(),
        title: Some(title.into()),
        ..Default::default()
    })
    .await
    .expect("start task")
}

#[tokio::test]
async fn an_agent_starts_with_the_notes_for_its_repo_and_for_every_repo() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let c = env.client().await;
    let repo = env.repo.to_string_lossy().to_string();
    c.memory_add(Some(&repo), "use pnpm, never npm", NoteSource::Typed)
        .await
        .unwrap();
    c.memory_add(None, "answer in plain words", NoteSource::Cli)
        .await
        .unwrap();
    c.memory_add(Some(&repo), "tests live in spec/", NoteSource::Change)
        .await
        .unwrap();
    let mut w = env.watcher(0, None).await;
    let task = env.start_task(&c, "Briefed", "go").await;

    let args = say_text(&w.wait("args", say_has("args: ")).await);
    // the repo's notes first, newest first, then the ones for every repo, all in one argument
    let want =
        format!("--append-system-prompt {HEADER}\n- tests live in spec/\n- use pnpm, never npm\n- answer in plain words");
    assert!(args.contains(&want), "{args}");
    w.wait("turn", is_turn).await;
    assert_eq!(briefed(w.find(started).unwrap()), 3);
    let snap = c.snapshot().await.unwrap();
    let t = snap.tasks.iter().find(|t| t.id == task).unwrap();
    assert_eq!(t.briefed, 3);
}

#[tokio::test]
async fn an_agent_in_another_repo_is_not_told_this_repos_notes() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let c = env.client().await;
    let repo = env.repo.to_string_lossy().to_string();
    let other = other_repo(&env);
    c.memory_add(Some(&repo), "use pnpm, never npm", NoteSource::Typed)
        .await
        .unwrap();
    c.memory_add(None, "answer in plain words", NoteSource::Typed)
        .await
        .unwrap();
    let mut w = env.watcher(0, None).await;
    let task = start_in(&c, &other, "Elsewhere").await;

    let args = say_text(&w.wait("args", say_has("args: ")).await);
    assert!(
        args.contains(&format!(
            "--append-system-prompt {HEADER}\n- answer in plain words"
        )),
        "{args}"
    );
    assert!(!args.contains("pnpm"), "{args}");
    w.wait("turn", is_turn).await;
    assert_eq!(briefed(w.find(started).unwrap()), 1);
    let snap = c.snapshot().await.unwrap();
    assert_eq!(snap.tasks.iter().find(|t| t.id == task).unwrap().briefed, 1);
}

#[tokio::test]
async fn an_agent_with_no_notes_gets_no_system_prompt_flag() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let c = env.client().await;
    // notes that are for a repo this task is not in
    c.memory_add(Some(&other_repo(&env)), "not for you", NoteSource::Typed)
        .await
        .unwrap();
    let mut w = env.watcher(0, None).await;
    let task = env.start_task(&c, "Plain", "go").await;

    let args = say_text(&w.wait("args", say_has("args: ")).await);
    assert!(!args.contains("--append-system-prompt"), "{args}");
    w.wait("turn", is_turn).await;
    assert_eq!(briefed(w.find(started).unwrap()), 0);
    let snap = c.snapshot().await.unwrap();
    assert_eq!(snap.tasks.iter().find(|t| t.id == task).unwrap().briefed, 0);
}

#[tokio::test]
async fn a_resumed_agent_is_briefed_again_with_the_notes_as_they_are_then() {
    let mut env = Env::new("resume");
    env.start_daemon().await;
    let c = env.client().await;
    let repo = env.repo.to_string_lossy().to_string();
    c.memory_add(Some(&repo), "use pnpm, never npm", NoteSource::Typed)
        .await
        .unwrap();
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Resume me", "begin").await;
    w.wait("started work", say_has("starting")).await;
    let first = say_text(w.find(say_has("args: ")).unwrap());
    assert!(first.contains("- use pnpm, never npm"), "{first}");
    assert!(!first.contains("keep commits small"), "{first}");
    assert_eq!(briefed(w.find(started).unwrap()), 1);
    w.wait("the Read to finish", |e| {
        matches!(e.body, EventBody::StepDone { .. })
    })
    .await;

    // a note is added while the agent works, then the host goes away and takes the agent with it
    c.memory_add(Some(&repo), "keep commits small", NoteSource::Change)
        .await
        .unwrap();
    env.host_call("shutdown");

    let resumed = w
        .wait("a second start", |e| {
            matches!(&e.body, EventBody::AgentStarted { resumed: true, .. })
        })
        .await;
    assert_eq!(briefed(&resumed), 2);
    let args = say_text(
        &w.wait("resumed args", |e| {
            matches!(&e.body, EventBody::Say { text } if text.contains("args: ") && text.contains("--resume"))
        })
        .await,
    );
    assert!(
        args.contains(&format!(
            "--append-system-prompt {HEADER}\n- keep commits small\n- use pnpm, never npm"
        )),
        "{args}"
    );
    w.wait("turn", is_turn).await;
    let snap = c.snapshot().await.unwrap();
    assert_eq!(snap.tasks[0].briefed, 2);
}

#[tokio::test]
async fn notes_reach_the_snapshot_and_every_subscriber_as_they_change() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let c = env.client().await;
    let mut w = env.watcher(0, None).await;
    let repo = env.repo.to_string_lossy().to_string();

    let added = c
        .memory_add(Some(&repo), "  use pnpm,\nnever npm ", NoteSource::Change)
        .await
        .unwrap();
    assert!(added.added);
    assert_eq!(added.count, 1);
    assert_eq!(added.note.text, "use pnpm, never npm", "cleaned up");
    assert_eq!(added.note.source, NoteSource::Change);
    let e = w.wait("the change", notes_changed).await;
    assert_eq!(e.body, EventBody::MemoryChanged { notes: 1 });
    assert_eq!(e.task, "", "a note belongs to no task");

    // the same note again changes nothing, and says so
    let again = c
        .memory_add(Some(&repo), "use pnpm, never npm", NoteSource::Typed)
        .await
        .unwrap();
    assert!(!again.added);
    assert_eq!(again.note.id, added.note.id);
    // the repo is its root, wherever in it the client stands
    let sub = env.repo.join("src");
    std::fs::create_dir_all(&sub).unwrap();
    let from_sub = c
        .memory_add(
            Some(&sub.to_string_lossy()),
            "tests live in spec/",
            NoteSource::Typed,
        )
        .await
        .unwrap();
    assert_eq!(from_sub.note.repo, added.note.repo);
    assert_eq!(from_sub.count, 2);
    let everywhere = c
        .memory_add(None, "answer in plain words", NoteSource::Cli)
        .await
        .unwrap();
    assert_eq!(everywhere.note.repo, None);
    assert_eq!(
        everywhere.count, 1,
        "counted among the notes for every repo"
    );
    w.wait_from(e.seq, "the other two", notes_changed).await;
    w.settle(300).await;
    assert_eq!(w.count(notes_changed), 3, "the repeat was not announced");

    // a snapshot has them, newest first
    let snap = c.snapshot().await.unwrap();
    let texts: Vec<&str> = snap.memory.iter().map(|n| n.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "answer in plain words",
            "tests live in spec/",
            "use pnpm, never npm"
        ]
    );
    assert!(snap.seq >= w.log.last().unwrap().seq);
    // a repo's list is what its agents are told: its own notes and the ones for every repo
    let other = other_repo(&env);
    assert_eq!(c.memory_list(None).await.unwrap().len(), 3);
    assert_eq!(c.memory_list(Some(&repo)).await.unwrap().len(), 3);
    let elsewhere = c.memory_list(Some(&other)).await.unwrap();
    assert_eq!(elsewhere.len(), 1);
    assert_eq!(elsewhere[0].text, "answer in plain words");

    // removing one is announced and gone for good
    c.memory_remove(added.note.id).await.unwrap();
    w.wait_from(w.log.last().unwrap().seq, "the removal", |e| {
        e.body == EventBody::MemoryChanged { notes: 2 }
    })
    .await;
    assert_eq!(c.snapshot().await.unwrap().memory.len(), 2);
    let err = c.memory_remove(added.note.id).await.unwrap_err();
    assert_eq!(err.kind(), Some("not_found"), "{err}");
}

#[tokio::test]
async fn a_note_that_is_empty_or_unplaced_is_refused() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let c = env.client().await;
    let repo = env.repo.to_string_lossy().to_string();

    let err = c
        .memory_add(Some(&repo), "  \n ", NoteSource::Typed)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("empty"), "{err}");
    // a folder that is not in a repo has no notes of its own
    let nowhere = env.root().to_string_lossy().to_string();
    let err = c
        .memory_add(Some(&nowhere), "x", NoteSource::Typed)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), Some("bad_repo"), "{err}");
    // for a repo or for all of them, not both and not neither
    let both: Result<api::MemoryAdded, _> = c
        .call(
            method::MEMORY_ADD,
            json!({"repo": repo, "all": true, "text": "x"}),
        )
        .await;
    assert!(both.unwrap_err().to_string().contains("not both"));
    let neither: Result<api::MemoryAdded, _> =
        c.call(method::MEMORY_ADD, json!({"text": "x"})).await;
    assert!(neither.unwrap_err().to_string().contains("which repo"));
    assert!(c.memory_list(None).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_long_note_is_cut_and_the_notes_are_capped() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let c = env.client().await;
    let long = c
        .memory_add(None, &"word ".repeat(200), NoteSource::Typed)
        .await
        .unwrap();
    assert_eq!(long.note.text.chars().count(), 300);
    assert!(long.note.text.ends_with('…'));
    for i in 1..sphere_proto::memory::NOTES_MAX {
        c.memory_add(None, &format!("note {i}"), NoteSource::Typed)
            .await
            .unwrap();
    }
    let err = c
        .memory_add(None, "one too many", NoteSource::Typed)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), Some("memory_full"), "{err}");
}

#[tokio::test]
async fn notes_survive_a_restart_of_sphered() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let c = env.client().await;
    let repo = env.repo.to_string_lossy().to_string();
    let kept = c
        .memory_add(Some(&repo), "use pnpm, never npm", NoteSource::Change)
        .await
        .unwrap()
        .note;
    drop(c);
    env.kill_daemon();
    env.start_daemon().await;
    let c = env.client().await;
    assert_eq!(c.snapshot().await.unwrap().memory, vec![kept.clone()]);
    // and the next one goes on from its id
    let next = c
        .memory_add(None, "and this", NoteSource::Typed)
        .await
        .unwrap();
    assert!(next.note.id > kept.id);
}

#[tokio::test]
async fn an_agent_cannot_change_the_notes_through_the_socket() {
    let mut env = Env::new("rpc_notes");
    env.start_daemon().await;
    let c = env.client().await;
    let repo = env.repo.to_string_lossy().to_string();
    let mine = c
        .memory_add(Some(&repo), "mine", NoteSource::Typed)
        .await
        .unwrap();
    let mut w = env.watcher(0, None).await;
    env.start_task(&c, "Rogue", "go").await;

    let add = say_text(&w.wait("rpc add", say_has("rpc memory.add: ")).await);
    assert!(add.contains("forbidden"), "{add}");
    let remove = say_text(&w.wait("rpc remove", say_has("rpc memory.remove: ")).await);
    assert!(remove.contains("forbidden"), "{remove}");
    // reading is fine
    let list = say_text(&w.wait("rpc list", say_has("rpc memory.list: ")).await);
    assert!(
        list.contains("\"result\"") && list.contains("mine"),
        "{list}"
    );
    w.wait("turn", is_turn).await;
    assert_eq!(c.memory_list(None).await.unwrap(), vec![mine.note]);
    assert_eq!(w.count(notes_changed), 1, "only the user's own addition");
}

#[tokio::test]
async fn the_assistant_client_kind_cannot_change_the_notes_either() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let assistant = Client::connect(&env.socket).await.unwrap();
    assistant
        .hello(sphere_client::ClientKind::Assistant)
        .await
        .unwrap();
    let err = assistant
        .memory_add(None, "x", NoteSource::Typed)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), Some("forbidden"), "{err}");
    assert!(assistant.memory_list(None).await.unwrap().is_empty());
}

// ---- the sphere command ----------------------------------------------------------

fn sphere_in(env: &Env, dir: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(&bins().sphere)
        .arg("--socket")
        .arg(&env.socket)
        .args(args)
        .current_dir(dir)
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
async fn remember_and_memory_work_from_the_command_line() {
    let mut env = Env::new("basic");
    env.start_daemon().await;
    let repo = env.repo.clone();
    let inside = repo.join("src");
    std::fs::create_dir_all(&inside).unwrap();
    let run =
        |dir: &Path, args: &[&str]| tokio::task::block_in_place(|| sphere_in(&env, dir, args));

    // the repo is the one the command is run in, from anywhere inside it
    let (ok, out, err) = run(&inside, &["remember", "use", "pnpm,", "never", "npm"]);
    assert!(ok, "{out}\n{err}");
    assert_eq!(out.trim(), "Remembered for repo · 1 note");
    let (ok, out, err) = run(&repo, &["remember", "--all", "answer in plain words"]);
    assert!(ok, "{out}\n{err}");
    assert_eq!(out.trim(), "Remembered for all repos · 1 note");
    let (_, out, _) = run(&repo, &["remember", "use pnpm, never npm"]);
    assert_eq!(out.trim(), "Already remembered for repo · 1 note");
    // outside a repo, a note needs --all
    let (ok, _, err) = run(env.root(), &["remember", "nowhere"]);
    assert!(!ok);
    assert!(
        err.contains("not in a git repository") && err.contains("--all"),
        "{err}"
    );

    let (ok, out, err) = run(&repo, &["memory"]);
    assert!(ok, "{out}\n{err}");
    assert!(out.contains("repo  "), "{out}");
    assert!(
        out.lines()
            .any(|l| l.contains("#1") && l.contains(" cli ") && l.ends_with("use pnpm, never npm")),
        "{out}"
    );
    assert!(out.contains("All repos") && out.contains("#2"), "{out}");

    let (ok, out, err) = run(&repo, &["memory", "rm", "1"]);
    assert!(ok, "{out}\n{err}");
    let (ok, _, err) = run(&repo, &["memory", "rm", "1"]);
    assert!(!ok);
    assert!(err.contains("no such note"), "{err}");
    let (_, out, _) = run(&repo, &["memory"]);
    assert!(
        !out.contains("pnpm") && out.contains("answer in plain words"),
        "{out}"
    );
    let c = env.client().await;
    assert_eq!(c.memory_list(None).await.unwrap().len(), 1);
}
