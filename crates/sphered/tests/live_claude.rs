//! A smoke test against the real Claude Code. Ignored by default: it needs the
//! `claude` CLI installed and signed in, and spends a few cents.
//!
//! Run it with:
//!
//! ```text
//! cargo test -p sphered --test live_claude -- --ignored --nocapture
//! ```
//!
//! `SPHERE_LIVE_CLAUDE` names the command if it is not `claude` on PATH. The
//! test sets a budget of five cents and asks for one word, with no tools.

mod common;

use common::*;
use sphere_client::StartParams;
use sphere_proto::EventBody;

#[tokio::test]
#[ignore = "talks to the real Claude Code and spends a little money; see the module docs"]
async fn a_real_agent_answers_one_word_within_a_tiny_budget() {
    let command = std::env::var("SPHERE_LIVE_CLAUDE").unwrap_or_else(|_| "claude".into());
    let mut env = Env::new("basic")
        .set_str("agent_command", &command)
        .set("agent_args", "[]")
        .set("check_version", "true");
    env.start_daemon().await;
    let c = env.client().await;
    let st = c.status().await.unwrap();
    println!(
        "claude version: {:?}, signed in: {:?}, warnings: {:?}",
        st.claude_version, st.signed_in, st.warnings
    );
    assert_ne!(
        st.signed_in,
        Some(false),
        "sign in with `claude auth login` first"
    );

    let mut w = env.watcher(0, None).await;
    c.start_task(StartParams {
        repo: env.repo.to_string_lossy().to_string(),
        prompt: "Reply with the single word pong. Do not use any tools.".into(),
        title: Some("Live smoke test".into()),
        budget_usd: Some(0.05),
        ..Default::default()
    })
    .await
    .unwrap();
    w.wait("an init event", is_kind("init")).await;
    let said = w
        .wait(
            "the answer",
            |e| matches!(&e.body, EventBody::Say { text } if text.to_lowercase().contains("pong")),
        )
        .await;
    println!("agent said: {:?}", said.body);
    let turn = w.wait("the turn to end", is_turn).await;
    println!("turn: {:?}", turn.body);
    let snap = c.snapshot().await.unwrap();
    assert!(
        snap.tasks[0].session_id.is_some(),
        "a session id was recorded"
    );
    assert!(
        snap.tasks[0].cost_usd <= 0.05 + 1e-6 || snap.tasks[0].cost_usd < 0.2,
        "cost stayed small: {}",
        snap.tasks[0].cost_usd
    );
    c.stop("t1").await.unwrap();
}
