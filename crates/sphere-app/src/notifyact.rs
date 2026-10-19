//! What an actionable notification offers and what its buttons answer. Pure: no OS calls here, so it is all tested.
//! `plan` turns the open requests into notifications (one per group of identical permission asks, one per other
//! request) with their buttons; `answers` turns a pressed button into the same `Cmd::Answer`s the Agent moment card
//! sends (Allow, Always, Change, a question's option or typed answer). The objc glue is in `summon`.
use crate::moment::{always_rule, options, same_ask};
use serde_json::Value;
use sphere_proto::{AnswerKind, RequestKind, RequestStatus, RequestView};

/// The longest body that still shows the whole command: Allow and Always are offered only when it fits.
pub const BODY_LIMIT: usize = 200;
/// A body too long (or too many lines) to trust is cut to this, with an ellipsis.
const CLIP: usize = 180;
/// The most options of a question that get a button each; more are answered with Reply or on the card.
pub const MAX_PICKS: usize = 3;
pub const DENY_TEXT: &str = "Denied from the notification. Don't do this; find another way or ask me.";

/// The action identifier macOS reports for a click on the notification itself, and for dismissing it.
pub const DEFAULT_ACTION: &str = "com.apple.UNNotificationDefaultActionIdentifier";

/// A button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    Allow,
    Always,
    /// Change with the fixed deny text.
    Deny,
    /// A text field for a permission: sent as a Change.
    Reply,
    /// A text field for a question: sent as the answer.
    Answer,
    /// Bring Sphere forward on the request.
    Open,
    /// A question's option, by its label.
    Pick(String),
}

impl Act {
    /// The identifier macOS hands back when the button is pressed.
    pub fn id(&self) -> String {
        match self {
            Act::Allow => "allow".into(),
            Act::Always => "always".into(),
            Act::Deny => "deny".into(),
            Act::Reply => "reply".into(),
            Act::Answer => "answer".into(),
            Act::Open => "open".into(),
            Act::Pick(l) => format!("pick:{l}"),
        }
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn title(&self) -> String {
        match self {
            Act::Allow => "Allow".into(),
            Act::Always => "Always allow".into(),
            Act::Deny => "Deny".into(),
            Act::Reply | Act::Answer => "Reply…".into(),
            Act::Open => "Open".into(),
            Act::Pick(l) => l.clone(),
        }
    }

    /// The text field's placeholder, for the two actions that have one.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn placeholder(&self) -> Option<&'static str> {
        match self {
            Act::Reply => Some("Tell the agent what to do instead"),
            Act::Answer => Some("Your answer"),
            _ => None,
        }
    }

    fn parse(id: &str) -> Option<Act> {
        Some(match id {
            "allow" => Act::Allow,
            "always" => Act::Always,
            "deny" => Act::Deny,
            "reply" => Act::Reply,
            "answer" => Act::Answer,
            "open" => Act::Open,
            _ => Act::Pick(id.strip_prefix("pick:")?.to_string()),
        })
    }
}

/// One notification.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    /// The notification identifier: a new note with the same one replaces the delivered one.
    pub id: String,
    /// The category (the buttons) it is posted under; see `categories`.
    pub category: String,
    pub title: String,
    pub body: String,
    /// The requests its buttons act on (and only these).
    pub ids: Vec<String>,
    pub actions: Vec<Act>,
}

/// What a pressed button sends (a `Cmd::Answer`).
#[derive(Debug, Clone, PartialEq)]
pub struct AnswerCmd {
    pub request: String,
    pub answer: AnswerKind,
    pub text: Option<String>,
    pub always: bool,
}

const PERM_FULL: &str = "perm-full";
const PERM_FULL_ALWAYS: &str = "perm-full-always";
const PERM_OPEN: &str = "perm-open";
const ASK_OPEN: &str = "ask-open";
const OPEN_ONLY: &str = "open-only";

/// The categories that never change.
pub fn static_categories() -> Vec<(String, Vec<Act>)> {
    let c = |id: &str, a: &[Act]| (id.to_string(), a.to_vec());
    vec![
        c(PERM_FULL, &[Act::Allow, Act::Deny, Act::Reply]),
        c(PERM_FULL_ALWAYS, &[Act::Allow, Act::Always, Act::Deny, Act::Reply]),
        c(PERM_OPEN, &[Act::Open, Act::Deny, Act::Reply]),
        c(ASK_OPEN, &[Act::Answer, Act::Open]),
        c(OPEN_ONLY, &[Act::Open]),
    ]
}

/// Every category the notifications in `notes` need: the static ones, then one per question with a button per option.
/// Registering this whole set again replaces what was registered, so questions that are gone drop out.
pub fn categories(notes: &[Note]) -> Vec<(String, Vec<Act>)> {
    let mut out = static_categories();
    for n in notes {
        if n.category.starts_with("q-") && !out.iter().any(|(id, _)| *id == n.category) {
            out.push((n.category.clone(), n.actions.clone()));
        }
    }
    out
}

fn compact(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(t: &str, n: usize) -> String {
    let t = compact(t);
    if t.chars().count() > n {
        t.chars().take(n - 1).collect::<String>() + "…"
    } else {
        t
    }
}

/// The exact action of a permission, as text: a command is itself, anything else its tool and exact input.
fn exact(r: &RequestView) -> String {
    if r.info.tool == "Bash" {
        if let Some(c) = r.info.input.get("command").and_then(Value::as_str) {
            return c.trim().to_string();
        }
    }
    format!("{} {}", r.info.tool, serde_json::to_string(&r.info.input).unwrap_or_default())
}

/// FNV-1a: a hash that stays the same between runs and versions (the notification identifier must).
fn fnv(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for p in parts {
        for b in p.bytes().chain([0u8]) {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
    }
    h
}

/// The identifier of the notification for a permission: its tool and exact input.
pub fn perm_key(r: &RequestView) -> String {
    format!("sphere-perm-{:016x}", fnv(&[&r.info.tool, &serde_json::to_string(&r.info.input).unwrap_or_default()]))
}

/// The notifications for the requests waiting for an answer (snoozed ones are not waiting), in the order they arrived.
/// `title_of` gives a task's title.
pub fn plan(requests: &[RequestView], title_of: impl Fn(&str) -> String) -> Vec<Note> {
    let waiting: Vec<&RequestView> = requests.iter().filter(|r| r.status == RequestStatus::Open).collect();
    let mut out: Vec<Note> = Vec::new();
    let mut done: Vec<&str> = Vec::new();
    for r in &waiting {
        if done.contains(&r.info.id.as_str()) {
            continue;
        }
        let title = format!("Needs you · {}", title_of(&r.info.task));
        match r.info.kind {
            RequestKind::Permission => {
                let group: Vec<&RequestView> = waiting.iter().copied().filter(|o| same_ask(r, o)).collect();
                done.extend(group.iter().map(|o| o.info.id.as_str()));
                let (title, body) = if group.len() > 1 {
                    (format!("Needs you · {} agents", group.len()), format!("{} agents ask: {}", group.len(), exact(r)))
                } else {
                    (title, exact(r))
                };
                let fits = !body.contains('\n') && !exact(r).is_empty() && body.chars().count() <= BODY_LIMIT;
                let (category, actions) = if !fits {
                    (PERM_OPEN, vec![Act::Open, Act::Deny, Act::Reply])
                } else if always_rule(r).is_some() {
                    (PERM_FULL_ALWAYS, vec![Act::Allow, Act::Always, Act::Deny, Act::Reply])
                } else {
                    (PERM_FULL, vec![Act::Allow, Act::Deny, Act::Reply])
                };
                out.push(Note {
                    id: perm_key(r),
                    category: category.into(),
                    title,
                    body: if fits { body } else { clip(&body, CLIP) },
                    ids: group.iter().map(|o| o.info.id.clone()).collect(),
                    actions,
                });
            }
            RequestKind::Question => {
                let q = &r.info.input["questions"][0];
                let text = q.get("question").and_then(Value::as_str).unwrap_or("");
                let opts = options(&r.info.input);
                let (category, actions) = if (1..=MAX_PICKS).contains(&opts.len()) {
                    (format!("q-{}", r.info.id), opts.into_iter().map(Act::Pick).collect())
                } else {
                    (ASK_OPEN.to_string(), vec![Act::Answer, Act::Open])
                };
                out.push(Note { id: format!("sphere-req-{}", r.info.id), category, title, body: clip(text, CLIP), ids: vec![r.info.id.clone()], actions });
            }
            RequestKind::Plan | RequestKind::ResumeCheck => {
                let text = if r.info.kind == RequestKind::Plan { r.info.input.get("plan").and_then(Value::as_str).unwrap_or("") } else { "" };
                let text = if text.trim().is_empty() { if r.info.summary.is_empty() { &r.info.tool } else { &r.info.summary } } else { text };
                out.push(Note { id: format!("sphere-req-{}", r.info.id), category: OPEN_ONLY.into(), title, body: clip(text, CLIP), ids: vec![r.info.id.clone()], actions: vec![Act::Open] });
            }
        }
    }
    out
}

/// Whether a button (or the click on the notification) brings Sphere forward instead of answering.
pub fn opens(action_id: &str) -> bool {
    action_id == DEFAULT_ACTION || action_id == "open"
}

/// The task to bring up on the card for these ids: the first one still waiting.
pub fn focus_task(ids: &[String], requests: &[RequestView]) -> Option<String> {
    ids.iter().find_map(|id| requests.iter().find(|r| r.info.id == *id && r.status.is_open())).map(|r| r.info.task.clone())
}

/// What `action_id` sends for `ids` (the ones in the notification, and no others). Open, the click on the notification
/// and anything unknown send nothing. Answering a request that is already answered is harmless (the service says so
/// and the app ignores it), so ids that are no longer in `requests` are still sent; `requests` only decides whether
/// Always is plain (the first request has no rule to save).
pub fn answers(action_id: &str, ids: &[String], text: Option<&str>, requests: &[RequestView]) -> Vec<AnswerCmd> {
    let Some(act) = Act::parse(action_id) else { return Vec::new() };
    let typed = text.map(str::trim).filter(|t| !t.is_empty());
    let all = |answer: AnswerKind, text: Option<String>| -> Vec<AnswerCmd> {
        ids.iter().map(|id| AnswerCmd { request: id.clone(), answer, text: text.clone(), always: false }).collect()
    };
    match act {
        Act::Allow => all(AnswerKind::Yes, None),
        Act::Always => {
            let mut cmds = all(AnswerKind::Yes, None);
            if let Some(first) = cmds.first_mut() {
                // only with a rule to save: a request gone from the list gets a plain yes
                first.always = requests.iter().find(|r| r.info.id == first.request).is_some_and(|r| always_rule(r).is_some());
            }
            cmds
        }
        Act::Deny => all(AnswerKind::Change, Some(DENY_TEXT.into())),
        Act::Reply => typed.map(|t| all(AnswerKind::Change, Some(t.to_string()))).unwrap_or_default(),
        // a question has one request; its typed answer is a yes carrying the text, like on the card
        Act::Answer => typed.map(|t| all(AnswerKind::Yes, Some(t.to_string()))).unwrap_or_default(),
        Act::Pick(label) => all(AnswerKind::Yes, Some(label)),
        Act::Open => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sphere_proto::RequestInfo;

    fn req(id: &str, task: &str, kind: RequestKind, tool: &str, input: Value, suggestions: Vec<Value>) -> RequestView {
        RequestView {
            info: RequestInfo {
                id: id.into(),
                task: task.into(),
                kind,
                tool: tool.into(),
                display_name: None,
                description: None,
                summary: String::new(),
                input,
                suggestions,
                blocked_path: None,
                tool_use_id: None,
                agent_request: None,
                revises: None,
            },
            status: RequestStatus::Open,
            opened_at: "2026-10-19T08:00:00Z".into(),
        }
    }

    fn rule(c: &str) -> Value {
        json!({"type": "addRules", "behavior": "allow", "rules": [{"toolName": "Bash", "ruleContent": c}]})
    }

    fn bash(id: &str, task: &str, cmd: &str, always: bool) -> RequestView {
        req(id, task, RequestKind::Permission, "Bash", json!({"command": cmd}), if always { vec![rule("npm install:*")] } else { vec![] })
    }

    fn question(id: &str, opts: &[&str]) -> RequestView {
        let o: Vec<Value> = opts.iter().map(|l| json!({"label": l})).collect();
        req(id, "t1", RequestKind::Question, "AskUserQuestion", json!({"questions": [{"question": "Which one?", "options": o}]}), vec![])
    }

    fn titles(t: &str) -> String {
        format!("Title {t}")
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn cmd(request: &str, answer: AnswerKind, text: Option<&str>, always: bool) -> AnswerCmd {
        AnswerCmd { request: request.into(), answer, text: text.map(String::from), always }
    }

    #[test]
    fn a_single_permission_that_fits_offers_allow_with_the_command_in_the_body() {
        let n = plan(&[bash("r1", "t1", "npm install left-pad", false)], titles);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].title, "Needs you · Title t1");
        assert_eq!(n[0].body, "npm install left-pad");
        assert_eq!(n[0].category, "perm-full");
        assert_eq!(n[0].actions, vec![Act::Allow, Act::Deny, Act::Reply]);
        assert_eq!(n[0].ids, ids(&["r1"]));
    }

    #[test]
    fn always_is_offered_only_when_there_is_a_rule() {
        let n = plan(&[bash("r1", "t1", "npm install left-pad", true)], titles);
        assert_eq!(n[0].category, "perm-full-always");
        assert_eq!(n[0].actions, vec![Act::Allow, Act::Always, Act::Deny, Act::Reply]);
    }

    #[test]
    fn a_command_that_does_not_fit_gets_open_deny_and_reply_but_no_allow() {
        let long = format!("echo {}", "x".repeat(BODY_LIMIT));
        for cmd in [long.as_str(), "echo a\necho b", ""] {
            let n = plan(&[bash("r1", "t1", cmd, true)], titles);
            assert_eq!(n[0].category, "perm-open", "{cmd:?}");
            assert_eq!(n[0].actions, vec![Act::Open, Act::Deny, Act::Reply]);
            assert!(!n[0].body.contains('\n') && n[0].body.chars().count() <= CLIP);
        }
        // right at the limit still fits
        let edge = format!("echo {}", "x".repeat(BODY_LIMIT - 5));
        assert_eq!(plan(&[bash("r1", "t1", &edge, false)], titles)[0].category, "perm-full");
        let over = format!("echo {}", "x".repeat(BODY_LIMIT - 4));
        assert_eq!(plan(&[bash("r1", "t1", &over, false)], titles)[0].category, "perm-open");
    }

    #[test]
    fn the_same_ask_from_several_agents_is_one_notification_with_all_the_ids() {
        let rs = [bash("r1", "t1", "npm install left-pad", true), bash("r2", "t2", "make", false), bash("r3", "t3", "npm install left-pad", true)];
        let n = plan(&rs, titles);
        assert_eq!(n.len(), 2);
        assert_eq!(n[0].ids, ids(&["r1", "r3"]));
        assert_eq!(n[0].title, "Needs you · 2 agents");
        assert_eq!(n[0].body, "2 agents ask: npm install left-pad");
        assert_eq!(n[1].ids, ids(&["r2"]));
        assert_eq!(n[1].body, "make");
        // another command is another notification, and the same command keeps its identifier as agents come and go
        assert_ne!(n[0].id, n[1].id);
        assert_eq!(n[0].id, plan(&rs[..1], titles)[0].id);
        assert_eq!(n[0].id, plan(&rs[2..], titles)[0].id);
        assert_eq!(n[0].id, perm_key(&rs[0]));
    }

    #[test]
    fn the_group_counts_in_the_fit() {
        // the bare command fits, but not behind "N agents ask: "
        let cmd = format!("echo {}", "x".repeat(BODY_LIMIT - 10));
        assert_eq!(plan(&[bash("r1", "t1", &cmd, false)], titles)[0].category, "perm-full");
        let n = plan(&[bash("r1", "t1", &cmd, false), bash("r2", "t2", &cmd, false)], titles);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].category, "perm-open");
    }

    #[test]
    fn same_tool_with_other_input_is_not_the_same_ask() {
        let rs = [bash("r1", "t1", "git push", false), bash("r2", "t2", "git push --force", false)];
        assert_eq!(plan(&rs, titles).len(), 2);
    }

    #[test]
    fn other_tools_show_their_exact_input() {
        let r = req("r1", "t1", RequestKind::Permission, "Write", json!({"file_path": "a.txt"}), vec![]);
        let n = plan(&[r], titles);
        assert_eq!(n[0].body, r#"Write {"file_path":"a.txt"}"#);
        assert_eq!(n[0].category, "perm-full");
    }

    #[test]
    fn snoozed_requests_are_not_notified() {
        let mut r = bash("r1", "t1", "make", false);
        r.status = RequestStatus::Snoozed;
        assert!(plan(&[r], titles).is_empty());
    }

    #[test]
    fn a_question_with_up_to_three_options_gets_a_button_each() {
        let n = plan(&[question("q1", &["Yes", "No", "Maybe"])], titles);
        assert_eq!(n[0].category, "q-q1");
        assert_eq!(n[0].actions, vec![Act::Pick("Yes".into()), Act::Pick("No".into()), Act::Pick("Maybe".into())]);
        assert_eq!(n[0].body, "Which one?");
        assert_eq!(n[0].id, "sphere-req-q1");
        // the whole set to register holds the static categories and this question's
        let cats = categories(&n);
        assert_eq!(cats.len(), static_categories().len() + 1);
        assert_eq!(cats.last().unwrap().0, "q-q1");
    }

    #[test]
    fn a_question_with_more_options_or_none_gets_reply_and_open() {
        for opts in [&["a", "b", "c", "d"][..], &[][..]] {
            let n = plan(&[question("q1", opts)], titles);
            assert_eq!(n[0].category, "ask-open");
            assert_eq!(n[0].actions, vec![Act::Answer, Act::Open]);
            assert_eq!(categories(&n).len(), static_categories().len());
        }
    }

    #[test]
    fn plans_and_resume_checks_only_open() {
        let p = req("p1", "t1", RequestKind::Plan, "ExitPlanMode", json!({"plan": "1. do it\n2. test it"}), vec![]);
        let c = req("c1", "t2", RequestKind::ResumeCheck, "Bash", json!({"command": "rm -rf x"}), vec![]);
        let n = plan(&[p, c], titles);
        assert_eq!(n.len(), 2);
        for x in &n {
            assert_eq!(x.category, "open-only");
            assert_eq!(x.actions, vec![Act::Open]);
        }
        assert_eq!(n[0].body, "1. do it 2. test it");
        assert_eq!(n[1].ids, ids(&["c1"]));
    }

    #[test]
    fn every_note_category_is_registered() {
        let rs = [bash("r1", "t1", "make", true), bash("r2", "t2", "echo a\nb", false), question("q1", &["A"]), question("q2", &["A", "B", "C", "D"])];
        let n = plan(&rs, titles);
        let cats = categories(&n);
        for x in &n {
            let (_, acts) = cats.iter().find(|(id, _)| *id == x.category).unwrap_or_else(|| panic!("{} missing", x.category));
            assert_eq!(*acts, x.actions);
        }
    }

    #[test]
    fn action_ids_round_trip() {
        for a in [Act::Allow, Act::Always, Act::Deny, Act::Reply, Act::Answer, Act::Open, Act::Pick("Use: pick:1".into())] {
            assert_eq!(Act::parse(&a.id()), Some(a));
        }
        assert_eq!(Act::parse("nope"), None);
        assert!(Act::Reply.placeholder().is_some() && Act::Allow.placeholder().is_none());
    }

    #[test]
    fn allow_says_yes_to_every_id() {
        let rs = [bash("r1", "t1", "make", true), bash("r2", "t2", "make", true)];
        assert_eq!(
            answers("allow", &ids(&["r1", "r2"]), None, &rs),
            vec![cmd("r1", AnswerKind::Yes, None, false), cmd("r2", AnswerKind::Yes, None, false)]
        );
    }

    #[test]
    fn always_saves_the_rule_on_the_first_id_only() {
        let rs = [bash("r1", "t1", "make", true), bash("r2", "t2", "make", true)];
        assert_eq!(
            answers("always", &ids(&["r1", "r2"]), None, &rs),
            vec![cmd("r1", AnswerKind::Yes, None, true), cmd("r2", AnswerKind::Yes, None, false)]
        );
        // a first request with no rule to save is a plain yes; one that has left the list is still answered
        let plain = [bash("r1", "t1", "make", false)];
        assert_eq!(answers("always", &ids(&["r1"]), None, &plain), vec![cmd("r1", AnswerKind::Yes, None, false)]);
        assert_eq!(answers("always", &ids(&["r9"]), None, &[]), vec![cmd("r9", AnswerKind::Yes, None, false)]);
    }

    #[test]
    fn deny_changes_every_id_with_the_fixed_text() {
        assert_eq!(
            answers("deny", &ids(&["r1", "r2"]), None, &[]),
            vec![cmd("r1", AnswerKind::Change, Some(DENY_TEXT), false), cmd("r2", AnswerKind::Change, Some(DENY_TEXT), false)]
        );
    }

    #[test]
    fn a_reply_is_a_change_for_a_permission_and_the_answer_for_a_question() {
        assert_eq!(answers("reply", &ids(&["r1"]), Some("  use yarn "), &[]), vec![cmd("r1", AnswerKind::Change, Some("use yarn"), false)]);
        assert_eq!(answers("answer", &ids(&["q1"]), Some("blue"), &[]), vec![cmd("q1", AnswerKind::Yes, Some("blue"), false)]);
        // nothing typed, nothing sent
        assert!(answers("reply", &ids(&["r1"]), Some("  "), &[]).is_empty());
        assert!(answers("reply", &ids(&["r1"]), None, &[]).is_empty());
        assert!(answers("answer", &ids(&["q1"]), None, &[]).is_empty());
    }

    #[test]
    fn a_pick_answers_with_the_option_label() {
        assert_eq!(answers("pick:No", &ids(&["q1"]), None, &[]), vec![cmd("q1", AnswerKind::Yes, Some("No"), false)]);
    }

    #[test]
    fn open_the_click_and_unknown_actions_send_nothing() {
        let rs = [bash("r1", "t1", "make", true)];
        assert!(answers("open", &ids(&["r1"]), None, &rs).is_empty());
        assert!(answers(DEFAULT_ACTION, &ids(&["r1"]), None, &rs).is_empty());
        assert!(answers("com.apple.UNNotificationDismissActionIdentifier", &ids(&["r1"]), None, &rs).is_empty());
        assert!(opens("open") && opens(DEFAULT_ACTION) && !opens("allow"));
        assert!(answers("allow", &[], None, &rs).is_empty());
    }

    #[test]
    fn focus_goes_to_the_first_id_still_waiting() {
        let rs = [bash("r2", "t2", "make", false), bash("r3", "t3", "make", false)];
        assert_eq!(focus_task(&ids(&["r1", "r3", "r2"]), &rs).as_deref(), Some("t3"));
        assert_eq!(focus_task(&ids(&["r1"]), &rs), None);
    }
}
