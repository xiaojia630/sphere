//! Moving a Claude session that runs in a terminal into Sphere.
//!
//! The "terminal session" is a copy of fake-claude named `claude`, started
//! with `--idle-forever --session-id <id>` in the test repo and detached from
//! the test process (so its parent chain ends at init: "a terminal").

mod common;
use common::*;
use sphere_proto::{EventBody, TaskState};
use std::path::Path;
use std::process::{Command, Stdio};

/// A fresh id per run: tests run in parallel and each sees the other's fake
/// session (and any left by an earlier run that was cut short).
fn new_sid(kind: u32) -> String {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!(
        "{:08x}-{kind:04x}-4000-8000-{:012x}",
        std::process::id(),
        ns & 0xffff_ffff_ffff
    )
}

fn line(
    sid: &str,
    kind: &str,
    content: serde_json::Value,
    cwd: &Path,
    stop: Option<&str>,
) -> String {
    serde_json::json!({"type": kind, "cwd": cwd, "sessionId": sid,
        "message": {"role": kind, "content": content, "stop_reason": stop}})
    .to_string()
        + "\n"
}

/// Write the transcript where Claude Code would: `<projects>/<encoded cwd>/<id>.jsonl`.
fn write_transcript(projects: &Path, cwd: &Path, sid: &str, idle: bool) {
    let enc: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let dir = projects.join(enc);
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = line(
        sid,
        "user",
        serde_json::json!("Refactor the parser"),
        cwd,
        None,
    );
    if idle {
        body += &line(
            sid,
            "assistant",
            serde_json::json!([{"type":"text","text":"Parser refactored.\nAll tests pass."}]),
            cwd,
            Some("end_turn"),
        );
    } else {
        body += &line(
            sid,
            "assistant",
            serde_json::json!([{"type":"tool_use","id":"t1","name":"Bash","input":{}}]),
            cwd,
            Some("tool_use"),
        );
    }
    std::fs::write(dir.join(format!("{sid}.jsonl")), body).unwrap();
}

/// Start the fake terminal session (its pid comes from sphered's listing).
fn start_outside(root: &Path, cwd: &Path, sid: &str) {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let claude = bin.join("claude");
    std::fs::copy(&bins().fake, &claude).unwrap();
    // `sh` exits at once, so the session is orphaned like one in a long-gone shell
    Command::new("sh")
        .arg("-c")
        .arg("\"$0\" --idle-forever --session-id \"$1\" </dev/null >/dev/null 2>&1 &")
        .arg(&claude)
        .arg(sid)
        .current_dir(cwd)
        .stdout(Stdio::null())
        .status()
        .unwrap();
}

struct Setup {
    env: Env,
    sid: String,
    pid: i32,
}

async fn setup(idle: bool) -> Setup {
    let mut env = Env::new("takeover");
    let projects = env.root().join("projects");
    let repo = env.repo.canonicalize().unwrap();
    let sid = new_sid(if idle { 1 } else { 2 });
    write_transcript(&projects, &repo, &sid, idle);
    start_outside(env.root(), &repo, &sid);
    let fake = bins().fake.to_string_lossy().to_string();
    env = env
        .set_str("claude_projects_dir", &projects.to_string_lossy())
        .set(
            "session_list_command",
            &format!("[{fake:?}, \"agents\", \"--json\"]"),
        )
        .set("session_stop_command", &format!("[{fake:?}, \"stop\"]"));
    env.start_daemon().await;
    let c = env.client().await;
    let pid = eventually("the fake session to show up", 10, || async {
        let l = c.session_outside().await.unwrap();
        l.iter()
            .find(|o| o.session_id.as_deref() == Some(sid.as_str()))
            .and_then(|o| o.pid)
    })
    .await as i32;
    Setup { env, sid, pid }
}

impl Drop for Setup {
    fn drop(&mut self) {
        unsafe { libc::kill(self.pid, libc::SIGKILL) };
    }
}

#[tokio::test]
async fn an_idle_terminal_session_moves_in_and_waits_for_a_message() {
    let s = setup(true).await;
    let sid = s.sid.as_str();
    let env = &s.env;
    let c = env.client().await;

    let outside = c.session_outside().await.unwrap();
    let o = outside
        .iter()
        .find(|o| o.session_id.as_deref() == Some(sid))
        .unwrap_or_else(|| panic!("not listed: {outside:?}"));
    assert_eq!(o.where_, "a terminal");
    assert_eq!(o.state, "idle");
    assert_eq!(o.pid, Some(s.pid as u32));
    assert!(!o.ambiguous);
    assert_eq!(o.title.as_deref(), Some("Refactor the parser"));
    assert_eq!(o.last_message.as_deref(), Some("Parser refactored."));
    assert_eq!(
        o.cwd.as_deref(),
        Some(env.repo.canonicalize().unwrap().to_str().unwrap())
    );

    let list = c.session_list().await.unwrap();
    let l = list.iter().find(|x| x.session_id == sid).unwrap();
    assert!(l.running);
    assert_eq!(l.where_.as_deref(), Some("a terminal"));
    assert_eq!(l.state.as_deref(), Some("idle"));
    let err = c
        .bring_in(sphere_client::BringInParams {
            session_id: sid.into(),
            cwd: None,
            prompt: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.kind(), Some("session_running"));

    let mut w = env.watcher(0, None).await;
    let r = c.take_over(sid).await.unwrap();
    assert_eq!(r.from, "a terminal");
    assert!(r.idle);
    assert_eq!(r.title, "Refactor the parser");
    eventually("the old process to exit", 5, || async {
        (!pid_alive(s.pid)).then_some(())
    })
    .await;

    let args = say_text(&w.wait("args", say_has("args: ")).await);
    assert!(args.contains(&format!("--resume {sid}")), "{args}");
    let taken = w
        .find(|e| matches!(e.body, EventBody::TakenOver { .. }))
        .unwrap()
        .seq;
    let started = w.find(is_kind("agent_started")).unwrap().seq;
    let created = w.find(is_kind("task_created")).unwrap().seq;
    assert!(
        created < taken && taken < started,
        "{created} {taken} {started}"
    );
    w.settle(300).await;
    assert_eq!(w.count(is_kind("user_message")), 0, "no first message");

    let t = c.snapshot().await.unwrap().tasks.remove(0);
    assert_eq!(t.id, r.task);
    assert_eq!(t.state, TaskState::Done);
    assert_eq!(t.status, "Waiting for your message");
    assert_eq!(t.taken_over_from.as_deref(), Some("a terminal"));
    assert!(t.in_place);
    assert_eq!(t.session_id.as_deref(), Some(sid));
    assert_eq!(t.repo, env.repo.canonicalize().unwrap().to_string_lossy());

    // it is now Sphere's: not listed outside, and a second take-over is refused
    assert!(c
        .session_outside()
        .await
        .unwrap()
        .iter()
        .all(|o| o.session_id.as_deref() != Some(sid)));
    let err = c.take_over(sid).await.unwrap_err();
    assert!(
        matches!(err.kind(), Some("invalid_state" | "not_found")),
        "{err:?}"
    );

    // and the user's message gets through
    c.send(&r.task, "hello there").await.unwrap();
    w.wait("echo", say_has("first message: hello there")).await;
}

#[tokio::test]
async fn a_session_stopped_mid_turn_is_told_to_continue() {
    let s = setup(false).await;
    let sid = s.sid.as_str();
    let c = s.env.client().await;
    let o = c.session_outside().await.unwrap();
    let o = o
        .iter()
        .find(|o| o.session_id.as_deref() == Some(sid))
        .unwrap();
    assert_eq!(o.state, "waiting");

    let mut w = s.env.watcher(0, None).await;
    let r = c.take_over(sid).await.unwrap();
    assert!(!r.idle);
    let m = w.wait("first message", is_kind("user_message")).await;
    match m.body {
        EventBody::UserMessage { text } => {
            assert!(text.starts_with("continue"), "{text}");
            assert!(text.contains("moved into Sphere from a terminal"), "{text}");
        }
        _ => unreachable!(),
    }
    w.wait("echo", say_has("first message: continue")).await;
    assert!(!pid_alive(s.pid));
}
