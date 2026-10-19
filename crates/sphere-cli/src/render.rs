//! Turning events and requests into text. Pure, so it is tested without a
//! terminal.

use serde_json::Value;
use sphere_proto::api::Snapshot;
use sphere_proto::{
    AnswerKind, Event, EventBody, RequestInfo, RequestKind, RequestStatus, TaskView,
};

pub struct Style {
    pub color: bool,
}

impl Style {
    fn bold(&self, s: &str) -> String {
        if self.color {
            format!("\x1b[1m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    fn dim(&self, s: &str) -> String {
        if self.color {
            format!("\x1b[2m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    fn red(&self, s: &str) -> String {
        if self.color {
            format!("\x1b[31m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
}

fn clip(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

fn indent(text: &str, by: &str, max_lines: usize) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = lines
        .iter()
        .take(max_lines)
        .map(|l| format!("{by}{}", clip(l, 200)))
        .collect();
    if lines.len() > max_lines {
        out.push(format!("{by}… ({} more lines)", lines.len() - max_lines));
    }
    out
}

/// The exact action a request asks for, as lines: the command, the edit, the
/// message. Never only the agent's description.
pub fn input_lines(tool: &str, input: &Value) -> Vec<String> {
    let s = |k: &str| input.get(k).and_then(|v| v.as_str());
    match tool {
        "Bash" => s("command")
            .map(|c| indent(c, "$ ", 12))
            .unwrap_or_default(),
        "Edit" => {
            let mut v = vec![s("file_path").unwrap_or("").to_string()];
            v.extend(indent(s("old_string").unwrap_or(""), "- ", 10));
            v.extend(indent(s("new_string").unwrap_or(""), "+ ", 10));
            v
        }
        "Write" => {
            let mut v = vec![format!("write {}", s("file_path").unwrap_or(""))];
            v.extend(indent(s("content").unwrap_or(""), "+ ", 10));
            v
        }
        "WebFetch" => vec![s("url").unwrap_or("").to_string()],
        "AskUserQuestion" => {
            let mut v = vec![];
            for q in input
                .get("questions")
                .and_then(|q| q.as_array())
                .into_iter()
                .flatten()
            {
                v.push(
                    q.get("question")
                        .and_then(|q| q.as_str())
                        .unwrap_or("")
                        .to_string(),
                );
                for o in q
                    .get("options")
                    .and_then(|o| o.as_array())
                    .into_iter()
                    .flatten()
                {
                    let label = o.get("label").and_then(|l| l.as_str()).unwrap_or("");
                    let desc = o.get("description").and_then(|l| l.as_str()).unwrap_or("");
                    v.push(if desc.is_empty() {
                        format!("  - {label}")
                    } else {
                        format!("  - {label}: {desc}")
                    });
                }
            }
            v
        }
        "ExitPlanMode" => indent(s("plan").unwrap_or(""), "", 30),
        "resume" => {
            let mut v = vec![];
            for c in input
                .get("cut_off")
                .and_then(|c| c.as_array())
                .into_iter()
                .flatten()
            {
                v.push(format!(
                    "cut off: {}",
                    c.get("title").and_then(|t| t.as_str()).unwrap_or("")
                ));
                if let Some(d) = c
                    .get("detail")
                    .and_then(|t| t.as_str())
                    .filter(|d| !d.is_empty())
                {
                    v.extend(indent(d, "  $ ", 3));
                }
            }
            v
        }
        _ => indent(
            &serde_json::to_string_pretty(input).unwrap_or_default(),
            "",
            15,
        ),
    }
}

fn kind_word(k: RequestKind) -> &'static str {
    match k {
        RequestKind::Permission => "permission",
        RequestKind::Question => "question",
        RequestKind::Plan => "plan",
        RequestKind::ResumeCheck => "resume check",
    }
}

/// A "needs you" card.
pub fn request_card(r: &RequestInfo, st: &Style) -> Vec<String> {
    let tag = format!("[{}]", r.task);
    let mut out = vec![format!(
        "{tag} {} {} ({}: {}){}",
        st.bold("NEEDS YOU"),
        st.bold(&r.id),
        kind_word(r.kind),
        r.display_name.as_deref().unwrap_or(&r.tool),
        r.revises
            .as_ref()
            .map(|o| format!(", revises {o}"))
            .unwrap_or_default()
    )];
    let pad = " ".repeat(tag.len() + 1);
    if !r.summary.is_empty() {
        out.push(format!("{pad}{}", clip(&r.summary, 200)));
    }
    for l in input_lines(&r.tool, &r.input) {
        out.push(format!("{pad}  {l}"));
    }
    let how = match r.kind {
        RequestKind::Question => format!("sphere answer {} yes \"<your answer>\"", r.id),
        _ => format!(
            "sphere answer {} yes | change \"<what to do instead>\" | later",
            r.id
        ),
    };
    out.push(format!("{pad}{}", st.dim(&how)));
    out
}

fn money(c: f64) -> String {
    format!("${c:.2}")
}

/// What an agent cost, as Claude Code reports it: an estimate at API prices (a subscription isn't billed it).
fn est(c: f64) -> String {
    format!("≈${c:.2}")
}

pub fn event_lines(e: &Event, st: &Style) -> Vec<String> {
    let tag = format!("[{}]", if e.task.is_empty() { "-" } else { &e.task });
    let one = |s: String| vec![format!("{tag} {s}")];
    match &e.body {
        EventBody::TaskCreated {
            title, budget_usd, ..
        } => one(format!(
            "task created: {title} (budget {})",
            money(*budget_usd)
        )),
        EventBody::WorkspaceReady {
            workdir, branch, ..
        } => match branch {
            Some(b) => one(format!("workspace {workdir} on branch {b}")),
            None => one(format!("working in place at {workdir}")),
        },
        EventBody::AgentStarted { pid, resumed, .. } => one(format!(
            "agent started (pid {pid}{})",
            if *resumed { ", resumed" } else { "" }
        )),
        EventBody::AgentExited { reason, .. } => one(format!("agent exited ({reason})")),
        EventBody::SessionId { .. } => vec![],
        EventBody::Init {
            model,
            permission_mode,
            ..
        } => one(format!(
            "agent ready{}{}",
            model
                .as_ref()
                .map(|m| format!(", model {m}"))
                .unwrap_or_default(),
            permission_mode
                .as_ref()
                .map(|m| format!(", mode {m}"))
                .unwrap_or_default()
        )),
        EventBody::UserMessage { text } => one(format!(
            "you: {}",
            clip(text.lines().next().unwrap_or(""), 200)
        )),
        EventBody::Say { text } => {
            let mut lines = text.lines();
            let mut out = vec![format!("{tag} {}", lines.next().unwrap_or(""))];
            out.extend(lines.map(|l| format!("{}{l}", " ".repeat(tag.len() + 1))));
            out
        }
        EventBody::Plan { items } => {
            let mut out = one("plan:".into());
            for i in items {
                let mark = match i.status.as_str() {
                    "completed" => "[x]",
                    "in_progress" => "[~]",
                    _ => "[ ]",
                };
                out.push(format!("{}{mark} {}", " ".repeat(tag.len() + 1), i.text));
            }
            out
        }
        EventBody::Step { title, parent, .. } => one(format!(
            "{}> {title}",
            if parent.is_some() { "  " } else { "" }
        )),
        EventBody::StepDone { ok, output, .. } => {
            let pad = " ".repeat(tag.len() + 3);
            let mut out = one(st.dim(if *ok { "  ok" } else { "  failed" }));
            if !*ok {
                out[0] = format!("{tag} {}", st.red("  failed"));
            }
            if let Some(o) = output {
                out.extend(indent(&o.head, &pad, 8));
                if let Some(t) = &o.tail {
                    out.push(format!("{pad}… ({} bytes in all)", o.bytes));
                    out.extend(indent(t, &pad, 4));
                }
            }
            out
        }
        EventBody::CutOff {
            title, outbound, ..
        } => one(format!(
            "cut off: {title}{}",
            if *outbound {
                " (it leaves your machine; check before continuing)"
            } else {
                ""
            }
        )),
        EventBody::FilesTouched { files } => one(format!(
            "files: {}",
            files
                .iter()
                .map(|f| format!("{} (+{} -{})", f.path, f.added, f.removed))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        EventBody::Request { request } => request_card(request, st),
        EventBody::RequestSnoozed { request } => {
            one(format!("{request} snoozed; the agent stays paused"))
        }
        EventBody::RequestAnswered {
            request,
            answer,
            text,
            always,
        } => one(format!(
            "{request} answered: {}{}{}",
            match answer {
                AnswerKind::Yes => "yes",
                AnswerKind::Change => "change",
                AnswerKind::Later => "later",
            },
            text.as_ref()
                .map(|t| format!(" \"{}\"", clip(t, 100)))
                .unwrap_or_default(),
            if *always { " (saved as a rule)" } else { "" }
        )),
        EventBody::RequestWithdrawn { request, reason } => {
            one(format!("{request} withdrawn: {reason}"))
        }
        EventBody::AutoAnswered { tool, allow, rule } => one(format!(
            "{} {tool} by rule {rule}",
            if *allow {
                "auto-allowed"
            } else {
                "auto-denied"
            }
        )),
        EventBody::ModeChanged { mode } => one(format!("mode is now {mode}")),
        EventBody::ModelChanged { model } => one(format!("model is now {model}")),
        EventBody::Turn {
            ok,
            interrupted,
            summary,
            turns,
            cost_usd,
            ..
        } => {
            let head = if *interrupted {
                "interrupted".to_string()
            } else if *ok {
                "finished".to_string()
            } else {
                st.red("failed")
            };
            one(format!(
                "{}: {}{}{}",
                st.bold(&head),
                clip(summary.lines().next().unwrap_or(""), 160),
                turns.map(|t| format!(" ({t} turns)")).unwrap_or_default(),
                cost_usd
                    .map(|c| format!(" total {} (API est.)", est(c)))
                    .unwrap_or_default()
            ))
        }
        EventBody::Stopped {} => one("stopped".into()),
        EventBody::Failed { message } => {
            one(format!("{}: {}", st.red("failed"), clip(message, 300)))
        }
        EventBody::Warning { message } => one(format!("warning: {message}")),
        EventBody::TakenOver { from } => one(format!("taken over from {from}")),
    }
}

/// `sphere ls`.
pub fn ls_table(snap: &Snapshot) -> Vec<String> {
    if snap.tasks.is_empty() {
        return vec!["no tasks".into()];
    }
    let mut out = vec![format!(
        "{:<5} {:<10} {:>7}  {:<28} {}",
        "ID", "STATE", "API EST", "BRANCH", "TITLE"
    )];
    for t in &snap.tasks {
        out.push(task_row(t));
        out.push(format!("      {}", clip(&t.status, 100)));
    }
    for r in &snap.requests {
        let tag = if r.status == RequestStatus::Snoozed {
            "snoozed"
        } else {
            "waiting"
        };
        out.push(format!(
            "{} {} for {} ({tag}): {}",
            "request",
            r.info.id,
            r.info.task,
            clip(&r.info.summary, 80)
        ));
    }
    out.push(format!(
        "today: {} (API estimate){}",
        est(snap.cost_today_usd),
        snap.daily_cap_usd
            .map(|c| format!(" of {} cap", money(c)))
            .unwrap_or_default()
    ));
    out
}

fn task_row(t: &TaskView) -> String {
    format!(
        "{:<5} {:<10} {:>7}  {:<28} {}",
        t.id,
        t.state.as_str(),
        est(t.cost_usd),
        clip(
            t.branch
                .as_deref()
                .unwrap_or(if t.in_place { "(in place)" } else { "-" }),
            28
        ),
        clip(&t.title, 60)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sphere_proto::{Excerpt, FileStat};

    fn plain() -> Style {
        Style { color: false }
    }

    fn ev(task: &str, body: EventBody) -> Event {
        Event {
            seq: 1,
            task: task.into(),
            at: "x".into(),
            body,
        }
    }

    fn request(tool: &str, input: Value) -> RequestInfo {
        RequestInfo {
            id: "r5".into(),
            task: "t3".into(),
            kind: RequestKind::for_tool(tool),
            tool: tool.into(),
            display_name: None,
            description: None,
            summary: "Push the branch".into(),
            input,
            suggestions: vec![],
            blocked_path: None,
            tool_use_id: None,
            agent_request: None,
            revises: Some("r2".into()),
        }
    }

    #[test]
    fn a_permission_card_shows_the_exact_command_and_how_to_answer() {
        let lines = request_card(
            &request(
                "Bash",
                json!({"command": "git push -u origin sphere/x\nrm -rf /tmp/y"}),
            ),
            &plain(),
        );
        let text = lines.join("\n");
        assert!(
            text.starts_with("[t3] NEEDS YOU r5 (permission: Bash), revises r2"),
            "{text}"
        );
        assert!(text.contains("Push the branch"));
        assert!(text.contains("$ git push -u origin sphere/x"));
        assert!(
            text.contains("$ rm -rf /tmp/y"),
            "every line of the command is shown"
        );
        assert!(text.contains("sphere answer r5 yes | change"));
    }

    #[test]
    fn cards_for_edits_questions_and_resume_checks() {
        let edit = input_lines(
            "Edit",
            &json!({"file_path": "/a.rs", "old_string": "a", "new_string": "b\nc"}),
        );
        assert_eq!(edit, vec!["/a.rs", "- a", "+ b", "+ c"]);
        let q = request_card(
            &request("AskUserQuestion", json!({"questions":[{"question":"Which db?","options":[{"label":"pg","description":"server"},{"label":"lite"}]}]})),
            &plain(),
        )
        .join("\n");
        assert!(q.contains("Which db?") && q.contains("- pg: server") && q.contains("- lite"));
        assert!(q.contains("sphere answer r5 yes \"<your answer>\""));
        let resume = input_lines(
            "resume",
            &json!({"cut_off":[{"title":"Running git push","detail":"git push origin x"}]}),
        );
        assert_eq!(resume[0], "cut off: Running git push");
        assert!(resume[1].contains("git push origin x"));
    }

    #[test]
    fn events_render_as_one_or_more_lines() {
        let st = plain();
        let say = event_lines(
            &ev(
                "t1",
                EventBody::Say {
                    text: "line one\nline two".into(),
                },
            ),
            &st,
        );
        assert_eq!(say, vec!["[t1] line one", "     line two"]);
        let step = event_lines(
            &ev(
                "t1",
                EventBody::Step {
                    id: "s".into(),
                    step_kind: sphere_proto::StepKind::Run,
                    tool: "Bash".into(),
                    title: "Running cargo test".into(),
                    detail: String::new(),
                    parent: None,
                },
            ),
            &st,
        );
        assert_eq!(step, vec!["[t1] > Running cargo test"]);
        let done = event_lines(
            &ev(
                "t1",
                EventBody::StepDone {
                    id: "s".into(),
                    ok: false,
                    output: Some(Excerpt {
                        head: "error: boom".into(),
                        tail: Some("tail line".into()),
                        bytes: 9000,
                        source: None,
                    }),
                },
            ),
            &st,
        );
        assert!(done[0].contains("failed"));
        assert!(done.iter().any(|l| l.contains("error: boom")));
        assert!(done.iter().any(|l| l.contains("9000 bytes")));
        assert!(done.iter().any(|l| l.contains("tail line")));
        let files = event_lines(
            &ev(
                "t1",
                EventBody::FilesTouched {
                    files: vec![FileStat {
                        path: "a".into(),
                        added: 2,
                        removed: 1,
                    }],
                },
            ),
            &st,
        );
        assert_eq!(files, vec!["[t1] files: a (+2 -1)"]);
        assert!(event_lines(
            &ev(
                "t1",
                EventBody::SessionId {
                    session_id: "s".into()
                }
            ),
            &st
        )
        .is_empty());
        let turn = event_lines(
            &ev(
                "t1",
                EventBody::Turn {
                    ok: true,
                    interrupted: false,
                    summary: "Added notes".into(),
                    turns: Some(3),
                    duration_ms: None,
                    cost_usd: Some(0.021),
                    cost_delta_usd: 0.0,
                },
            ),
            &st,
        );
        assert_eq!(
            turn,
            vec!["[t1] finished: Added notes (3 turns) total ≈$0.02 (API est.)"]
        );
        let answered = event_lines(
            &ev(
                "t1",
                EventBody::RequestAnswered {
                    request: "r5".into(),
                    answer: AnswerKind::Change,
                    text: Some("no".into()),
                    always: false,
                },
            ),
            &st,
        );
        assert_eq!(answered, vec!["[t1] r5 answered: change \"no\""]);
        let warn = event_lines(
            &ev(
                "",
                EventBody::Warning {
                    message: "careful".into(),
                },
            ),
            &st,
        );
        assert_eq!(warn, vec!["[-] warning: careful"]);
    }

    #[test]
    fn colours_only_when_asked() {
        let on = Style { color: true };
        assert!(on.bold("x").contains("\x1b["));
        assert_eq!(plain().bold("x"), "x");
    }
}
