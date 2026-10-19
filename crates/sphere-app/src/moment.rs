//! The Agent moment card (prototype: Agent moment): the oldest open request, centred over the core on a glass
//! panel. It shows the exact action (the command, the edit, the question, the plan), never only the agent's
//! description of it. Enter answers yes, E changes (type, Enter sends), L is later, Esc puts the card away
//! until something new arrives, and the arrows step through the others. A permission that comes with a rule
//! to remember also answers on A (yes, and the rule is saved), and when other agents ask the same, Shift+Enter
//! says yes to all of them at once. Only these keys answer: the service takes answers from the app and the CLI,
//! never from an agent.
use crate::appframe::{AppClicks, Click};
use crate::fonts::UiFonts;
use crate::live::{Live, Toasts};
use crate::panel::{font, with_alpha, Focus};
use crate::service::{Cmd, Service};
use crate::util::*;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::text::LineHeight;
use serde_json::Value;
use sphere_proto::{AnswerKind, RequestKind, RequestStatus, RequestView};

pub const CARD_W: f32 = 600.0;
/// Tall enough for the fullest card: a long summary and command, the same-ask line and the Always / Allow all row.
pub const CARD_H: f32 = 512.0;
/// The card's id in `AppClicks` (its quad carries an `AppPart` with it, so the app-pane click layer reaches it).
pub const CLICK_APP: u8 = 200;

#[derive(Resource, Default)]
pub struct Moment {
    /// The card is up and has the keyboard.
    pub shown: bool,
    /// Typing a `change` (or a question's answer).
    pub editing: bool,
    pub text: String,
    /// Which of the waiting requests is on the card.
    pub index: usize,
    /// Requests put away with Esc; the card comes back when one arrives that is not in here.
    dismissed: Vec<String>,
    /// Answered here, hidden until the next snapshot drops them.
    answered: Vec<String>,
    /// Put off here with Not now: they stay on the card's list (N brings them back) but don't raise it.
    put_off: Vec<String>,
    /// Bumps when anything on the card changes.
    pub version: u64,
    /// what the card shows now (for holding it when answered)
    pub current: Option<(RequestView, String)>,
    /// the answered card, kept on screen while it folds away (or dims into orbit)
    pub holding: Option<(RequestView, String)>,
    /// seconds since Send / Later, while it plays
    pub fold: Option<f32>,
    pub later: Option<f32>,
    /// bumps each time a card rises (the panel restarts its rise from the core)
    pub rise_seq: u64,
    /// everything answered: the halo turns green, Sphere says so, then the view closes
    pub all_done: Option<f32>,
    /// the streak's start: the card was here when it folded
    pub streak: Option<f32>,
    /// a key came while the answered card was leaving: finish its exit now and bring the next one up
    pub skip: bool,
    /// how many other agents' requests ask the same as the card drawn last (the answered card keeps its line and button
    /// while it folds)
    same: usize,
}

impl Moment {
    /// An answered card is still leaving, or everything is answered and the view is closing.
    pub fn busy(&self) -> bool {
        self.holding.is_some() || self.fold.is_some() || self.later.is_some() || self.all_done.is_some()
    }

    /// The task whose request is on the card now.
    pub fn current_task(&self, live: &Live) -> Option<String> {
        if !self.shown {
            return None;
        }
        waiting(live, self).get(self.index).map(|r| r.info.task.clone())
    }

    /// Brings the card up on `task`'s open request (the same sounds and rise as a request arriving; a swap from
    /// another request ticks instead). False when that task has no request waiting, or the card is busy leaving.
    pub fn show_task(&mut self, live: &Live, task: &str, sound: &mut crate::sound::Sound) -> bool {
        if self.busy() {
            return false;
        }
        let Some(i) = waiting(live, self).iter().position(|r| r.info.task == task) else { return false };
        if self.shown && self.index == i {
            return true;
        }
        if self.shown {
            sound.play_with("tick", 0.5, 0.0);
        } else {
            sound.play_with("whoosh", 0.7, 0.0);
            sound.play_with("open", 0.8, 0.0);
            sound.play_with("expand", 0.5, 0.0);
        }
        self.index = i;
        self.shown = true;
        self.editing = false;
        self.text.clear();
        self.rise_seq += 1;
        self.version += 1;
        true
    }

    /// Esc: the card goes back until something new arrives.
    pub fn put_back(&mut self, live: &Live, sound: &mut crate::sound::Sound) {
        if !self.shown || self.busy() {
            return;
        }
        let ids: Vec<String> = waiting(live, self).iter().map(|r| r.info.id.clone()).collect();
        self.dismissed.extend(ids);
        self.shown = false;
        self.editing = false;
        self.text.clear();
        self.version += 1;
        sound.play_with("collapse", 0.7, 0.0);
    }
}

/// The card's UI root (inside its panel texture).
#[derive(Resource)]
pub struct CardRoot(pub Entity);

/// Waiting for an answer and not put off: these raise the card.
fn pending(r: &RequestView, m: &Moment) -> bool {
    r.status == RequestStatus::Open && !m.answered.contains(&r.info.id) && !m.put_off.contains(&r.info.id)
}

/// What the card can show: every request still waiting that wasn't just answered here, the ones put off with
/// Not now last (so N, or clicking the agent, still finds them).
pub fn waiting<'a>(live: &'a Live, m: &Moment) -> Vec<&'a RequestView> {
    let mut w: Vec<&RequestView> = live.requests.iter().filter(|r| r.status.is_open() && !m.answered.contains(&r.info.id)).collect();
    w.sort_by_key(|r| !pending(r, m));
    w
}

/// Waiting requests not put off.
pub fn pending_count(live: &Live, m: &Moment) -> usize {
    live.requests.iter().filter(|r| pending(r, m)).count()
}

/// The allow rule a permission suggestion stands for, as sphered saves it to the policy (its `rule_from_suggestion`):
/// an "addRules" suggestion with behavior "allow", its first rule's tool name plus `(content)` when there is content.
fn rule_from_suggestion(s: &Value) -> Option<String> {
    if s.get("type")?.as_str()? != "addRules" || s.get("behavior").and_then(Value::as_str) != Some("allow") {
        return None;
    }
    let rule = s.get("rules")?.as_array()?.first()?;
    let tool = rule.get("toolName")?.as_str()?;
    Some(match rule.get("ruleContent").and_then(Value::as_str) {
        Some(c) if !c.is_empty() => format!("{tool}({c})"),
        _ => tool.to_string(),
    })
}

/// The rule Always would save for `r`: a permission's first suggestion (the only one sphered reads) when it is an allow
/// rule. Questions, plans and resume checks have none.
pub fn always_rule(r: &RequestView) -> Option<String> {
    if r.info.kind != RequestKind::Permission {
        return None;
    }
    rule_from_suggestion(r.info.suggestions.first()?)
}

/// Two requests ask the same when both are permissions for the same tool with exactly the same input. That is what the
/// person has seen on the card, so it is all that Allow all says yes to: the same suggested rule is not enough (`npm
/// install lodash` and `npm install react` share one, and so do `git push` and `git push --force`).
pub fn same_ask(a: &RequestView, b: &RequestView) -> bool {
    a.info.kind == RequestKind::Permission && b.info.kind == RequestKind::Permission && a.info.tool == b.info.tool && a.info.input == b.info.input
}

/// The other agents' requests that ask the same as `r`: waiting, not put off, from other tasks. The card counts them
/// ("+N more agents ask the same") and Shift+Enter answers them with `r`.
pub fn same_ask_others<'a>(live: &'a Live, m: &Moment, r: &RequestView) -> Vec<&'a RequestView> {
    waiting(live, m).into_iter().filter(|o| pending(o, m) && o.info.task != r.info.task && same_ask(r, o)).collect()
}

/// What Shift+Enter answers: `r` first, then every other request that asks the same, worked out when the key goes down.
fn allow_all_ids(live: &Live, m: &Moment, r: &RequestView) -> Vec<String> {
    std::iter::once(r.info.id.clone()).chain(same_ask_others(live, m, r).iter().map(|o| o.info.id.clone())).collect()
}

/// Shows the card when a request arrives that wasn't put away, hides it when nothing waits.
pub fn update(live: Res<Live>, mut m: ResMut<Moment>, mut seen: Local<u64>, mut sound: ResMut<crate::sound::Sound>, swarm: Res<crate::swarm::Swarm>) {
    if *seen == live.version {
        return;
    }
    *seen = live.version;
    let open: Vec<String> = live.requests.iter().map(|r| r.info.id.clone()).collect();
    m.answered.retain(|id| open.contains(id));
    m.dismissed.retain(|id| open.contains(id));
    m.put_off.retain(|id| open.contains(id));
    let w: Vec<String> = waiting(&live, &m).iter().map(|r| r.info.id.clone()).collect();
    let fresh = live.requests.iter().any(|r| pending(r, &m) && !m.dismissed.contains(&r.info.id));
    // a request that arrives while the swarm shows an agent's focus panel only toasts (the card waits for N)
    let held = swarm.open && matches!(swarm.mode, crate::swarm::Mode::Focus(_));
    let shown = !w.is_empty() && (m.shown || (fresh && !held));
    if shown != m.shown && m.holding.is_none() && m.all_done.is_none() {
        m.shown = shown;
        if shown {
            sound.play_with("whoosh", 0.7, 0.0);
            sound.play_with("open", 0.8, 0.0);
            sound.play_with("expand", 0.5, 0.0);
            m.rise_seq += 1;
        } else {
            sound.play_with("collapse", 0.7, 0.0);
            m.editing = false;
            m.text.clear();
        }
    }
    if m.index >= w.len() {
        m.index = 0;
    }
    m.version += 1;
}

#[allow(clippy::too_many_arguments)]
pub fn keys(
    mut ev: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    focus: Res<Focus>,
    mut m: ResMut<Moment>,
    live: Res<Live>,
    svc: Res<Service>,
    mut sound: ResMut<crate::sound::Sound>,
    mut toasts: ResMut<Toasts>,
    mut clicks: ResMut<AppClicks>,
) {
    // clicks on the card's buttons and option rows; they do what the same key does, and only on a card that is up and waiting
    let clicked = clicks.take(CLICK_APP);
    if !clicked.is_empty() && (m.fold.is_some() || m.later.is_some()) {
        m.skip = true;
    }
    if m.shown && m.holding.is_none() && m.fold.is_none() && m.later.is_none() && m.all_done.is_none() {
        for c in clicked {
            let w = waiting(&live, &m);
            let Some(req) = w.get(m.index).map(|r| (*r).clone()) else { break };
            let (n, id) = (w.len(), req.info.id.clone());
            let question = req.info.kind == RequestKind::Question;
            match c {
                Click::Key(KeyCode::Enter) if m.editing => {
                    let text = std::mem::take(&mut m.text);
                    m.editing = false;
                    if !text.trim().is_empty() {
                        answer(&mut m, &svc, &id, if question { AnswerKind::Yes } else { AnswerKind::Change }, Some(text.trim().to_string()));
                        sound.play_with("confirm", 0.7, 0.0);
                    }
                }
                Click::Key(KeyCode::Escape) if m.editing => {
                    m.editing = false;
                    m.text.clear();
                }
                _ if m.editing => {}
                Click::Key(KeyCode::Enter) if question => m.editing = true,
                Click::Key(KeyCode::Enter) => {
                    answer(&mut m, &svc, &id, AnswerKind::Yes, None);
                    sound.play_with("confirm", 0.7, 0.0);
                }
                Click::Key(KeyCode::KeyE) => m.editing = true,
                Click::Key(KeyCode::KeyL) => {
                    answer(&mut m, &svc, &id, AnswerKind::Later, None);
                    sound.play_with("whoosh", 0.6, 0.0);
                }
                Click::Key(KeyCode::KeyA) => {
                    if let Some(rule) = always_rule(&req) {
                        answer_always(&mut m, &svc, &mut toasts, &id, &rule);
                        sound.play_with("confirm", 0.7, 0.0);
                    }
                }
                Click::Alt(0) if !question => {
                    allow_all(&mut m, &svc, &live, &req);
                    sound.play_with("confirm", 0.7, 0.0);
                }
                Click::Key(KeyCode::ArrowRight) if n > 1 => m.index = (m.index + 1) % n,
                Click::Key(KeyCode::ArrowLeft) if n > 1 => m.index = (m.index + n - 1) % n,
                Click::Pick(i) if question => {
                    if let Some(label) = options(&req.info.input).get(i) {
                        answer(&mut m, &svc, &id, AnswerKind::Yes, Some(label.clone()));
                    }
                }
                _ => {}
            }
            m.version += 1;
        }
    }
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    // A saves a rule: never on a held key, and never as Cmd+A (select all) or the like
    let mods = keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::AltLeft, KeyCode::AltRight]);
    for e in ev.read() {
        if e.state != ButtonState::Pressed || !m.shown || focus.cmd || focus.terminal {
            continue;
        }
        if m.fold.is_some() || m.later.is_some() {
            // the answered card is still leaving: don't wait for it, and don't apply the key to the next request
            // before it is on screen
            m.skip = true;
            continue;
        }
        let w = waiting(&live, &m);
        let Some(req) = w.get(m.index).map(|r| (*r).clone()) else { continue };
        let n = w.len();
        let id = req.info.id.clone();
        if m.editing {
            match &e.logical_key {
                Key::Escape => {
                    m.editing = false;
                    m.text.clear();
                }
                Key::Enter => {
                    let text = std::mem::take(&mut m.text);
                    m.editing = false;
                    if !text.trim().is_empty() {
                        // A question's typed answer is a yes carrying the answer; anything else is a change.
                        let kind = if req.info.kind == RequestKind::Question { AnswerKind::Yes } else { AnswerKind::Change };
                        answer(&mut m, &svc, &id, kind, Some(text.trim().to_string()));
                        sound.play_with("confirm", 0.7, 0.0);
                    }
                }
                Key::Backspace => {
                    m.text.pop();
                }
                _ => {
                    if let Some(t) = &e.text {
                        m.text.extend(t.chars().filter(|c| !c.is_control()));
                    }
                }
            }
            m.version += 1;
            continue;
        }
        match &e.logical_key {
            // Shift+Enter: yes to this request and to every other agent's request that asks the same
            Key::Enter if shift && !mods && !e.repeat && req.info.kind != RequestKind::Question => {
                allow_all(&mut m, &svc, &live, &req);
                sound.play_with("confirm", 0.7, 0.0);
            }
            Key::Enter => {
                if req.info.kind == RequestKind::Question {
                    // A question needs an answer: pick an option by number, or type one.
                    m.editing = true;
                } else {
                    answer(&mut m, &svc, &id, AnswerKind::Yes, None);
                    sound.play_with("confirm", 0.7, 0.0);
                }
            }
            Key::Escape => {
                let ids: Vec<String> = waiting(&live, &m).iter().map(|r| r.info.id.clone()).collect();
                m.dismissed.extend(ids);
                m.shown = false;
            }
            Key::ArrowRight if n > 1 => m.index = (m.index + 1) % n,
            Key::ArrowLeft if n > 1 => m.index = (m.index + n - 1) % n,
            Key::Character(c) => match c.to_ascii_lowercase().as_str() {
                "e" => {
                    m.editing = true;
                    sound.play("click");
                }
                // Y is Enter's twin for a yes; a question needs its answer (a number, or E to type one)
                "y" if req.info.kind != RequestKind::Question => {
                    answer(&mut m, &svc, &id, AnswerKind::Yes, None);
                    sound.play_with("confirm", 0.7, 0.0);
                }
                "l" => {
                    answer(&mut m, &svc, &id, AnswerKind::Later, None);
                    sound.play_with("whoosh", 0.6, 0.0);
                }
                // A is a yes that also saves the rule the agent suggested (only when there is one)
                "a" if !mods && !e.repeat => {
                    if let Some(rule) = always_rule(&req) {
                        answer_always(&mut m, &svc, &mut toasts, &id, &rule);
                        sound.play_with("confirm", 0.7, 0.0);
                    }
                }
                d if req.info.kind == RequestKind::Question && d.len() == 1 && d.as_bytes()[0].is_ascii_digit() => {
                    let i = (d.as_bytes()[0] - b'0') as usize;
                    if let Some(label) = options(&req.info.input).get(i.wrapping_sub(1)) {
                        answer(&mut m, &svc, &id, AnswerKind::Yes, Some(label.clone()));
                    }
                }
                _ => {}
            },
            _ => {}
        }
        m.version += 1;
    }
}

fn answer(m: &mut Moment, svc: &Service, id: &str, kind: AnswerKind, text: Option<String>) {
    answer_with(m, svc, id, kind, text, false);
}

/// A yes that also saves the rule the agent suggested; the toast names the rule.
fn answer_always(m: &mut Moment, svc: &Service, toasts: &mut Toasts, id: &str, rule: &str) {
    answer_with(m, svc, id, AnswerKind::Yes, None, true);
    toasts.push_ok(format!("Saved rule: {}", clip(rule, 60)));
}

/// Yes to `req` and to every other agent's request that asks the same. The card is the one answered request: the
/// others are marked answered (hidden until the snapshot drops them) and sent, so the fold, the next card and the
/// sounds go as for any yes.
fn allow_all(m: &mut Moment, svc: &Service, live: &Live, req: &RequestView) {
    let ids = allow_all_ids(live, m, req);
    answer(m, svc, &req.info.id, AnswerKind::Yes, None);
    for id in &ids[1..] {
        m.answered.push(id.clone());
        svc.send(Cmd::Answer { request: id.clone(), answer: AnswerKind::Yes, text: None, always: false });
    }
}

fn answer_with(m: &mut Moment, svc: &Service, id: &str, kind: AnswerKind, text: Option<String>, always: bool) {
    // keep the answered card on screen while it folds (yes) or dims into orbit (later)
    if kind != AnswerKind::Change {
        m.holding = m.current.clone();
        if kind == AnswerKind::Later {
            m.later = Some(0.0);
        } else {
            m.fold = Some(0.0);
        }
    }
    if kind == AnswerKind::Later {
        m.put_off.retain(|x| x != id);
        m.put_off.push(id.to_string());
    } else {
        m.answered.push(id.to_string());
    }
    m.text.clear();
    m.editing = false;
    svc.send(Cmd::Answer { request: id.to_string(), answer: kind, text, always });
}

// ------------------------------------------------------------------ what the card says

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn first_lines(t: &str, n: usize, w: usize) -> String {
    let mut out: Vec<String> = t
        .lines()
        .take(n)
        .map(|l| if l.chars().count() > w { l.chars().take(w - 1).collect::<String>() + "…" } else { l.to_string() })
        .collect();
    if t.lines().count() > n {
        out.push("…".into());
    }
    out.join("\n")
}

/// One paragraph, at most `n` characters (the text wraps in the card).
fn clip(t: &str, n: usize) -> String {
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > n {
        t.chars().take(n - 1).collect::<String>() + "…"
    } else {
        t
    }
}

fn options(input: &Value) -> Vec<String> {
    input["questions"][0]["options"]
        .as_array()
        .map(|a| a.iter().map(|o| s(o, "label")).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default()
}

/// (label for the action block, the exact action, monospace?)
fn action(r: &RequestView) -> (String, String, bool) {
    let i = &r.info.input;
    match r.info.kind {
        RequestKind::Question => {
            ("Question".into(), first_lines(&s(&i["questions"][0], "question"), 3, 64), false)
        }
        RequestKind::Plan => ("Plan".into(), first_lines(&s(i, "plan"), 7, 70), false),
        _ => match r.info.tool.as_str() {
            "Bash" => ("Command".into(), first_lines(&s(i, "command"), 5, 70), true),
            "Edit" | "MultiEdit" => {
                let mut t = s(i, "file_path");
                let (old, new) = (s(i, "old_string"), s(i, "new_string"));
                for l in old.lines().take(2) {
                    t.push_str(&format!("\n- {l}"));
                }
                for l in new.lines().take(3) {
                    t.push_str(&format!("\n+ {l}"));
                }
                ("Edit".into(), first_lines(&t, 6, 70), true)
            }
            "Write" => ("Write".into(), first_lines(&format!("{}\n{}", s(i, "file_path"), s(i, "content")), 6, 70), true),
            "WebFetch" => ("Fetch".into(), s(i, "url"), true),
            _ => {
                let j = serde_json::to_string_pretty(i).unwrap_or_default();
                ("Action".into(), first_lines(&j, 6, 70), true)
            }
        },
    }
}

fn yes_label(r: &RequestView) -> &'static str {
    match r.info.kind {
        RequestKind::Question => "Answer",
        RequestKind::Plan => "Approve",
        _ => match r.info.tool.as_str() {
            "Bash" => "Run",
            "Edit" | "MultiEdit" | "Write" => "Apply",
            _ => "Allow",
        },
    }
}

fn waited(opened_at: &str) -> String {
    waited_since(opened_at, crate::core::unix_now())
}

/// "waiting 12 min", "waiting 3 h 5 min" (RFC 3339 from the service; `now` is unix seconds), or "just now" under a minute.
fn waited_since(opened_at: &str, now: i64) -> String {
    match crate::working::parse_rfc3339(opened_at) {
        Some(t) if now - t >= 60 => format!("waiting {}", crate::live::fmt_waited((now - t) as u64)),
        _ => "just now".into(),
    }
}

// ------------------------------------------------------------------ drawing

/// Rebuilds the card's UI when the request, the selection or the typed text change.
pub fn draw(mut commands: Commands, mut m: ResMut<Moment>, live: Res<Live>, root: Res<CardRoot>, f: Res<UiFonts>, mut seen: Local<u64>) {
    if *seen == m.version {
        return;
    }
    *seen = m.version;
    commands.entity(root.0).despawn_children();
    // while folding, the answered card stays
    if let Some((r, title)) = m.holding.clone() {
        let body = build(&mut commands, &f, &r, &title, &m, 1, m.same);
        commands.entity(root.0).add_child(body);
        return;
    }
    let w = waiting(&live, &m);
    let Some(r) = w.get(m.index).map(|r| (*r).clone()) else { return };
    let title = live.title(&r.info.task).to_string();
    let n = w.len();
    let same = same_ask_others(&live, &m, &r).len();
    let body = build(&mut commands, &f, &r, &title, &m, n, same);
    commands.entity(root.0).add_child(body);
    let m = m.bypass_change_detection();
    m.current = Some((r, title));
    m.same = same;
}

/// "+2 more agents ask the same", the card's line when other agents wait on the same ask.
fn same_line(n: usize) -> String {
    if n == 1 {
        "+1 more agent asks the same".to_string()
    } else {
        format!("+{n} more agents ask the same")
    }
}

/// The row above the actions with the other ways to say yes: Always (A), named by the rule it saves, and Allow all
/// (Shift+Enter) for the other agents asking the same.
fn more_ways(c: &mut Commands, f: &UiFonts, rule: Option<String>, same: usize) -> Entity {
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    let acc = role_color(Role::Acc);
    let mute = with_alpha(line, 0.62);
    let nowrap = TextLayout { linebreak: LineBreak::NoWrap, ..default() };
    let row = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), margin: UiRect { top: px(8), bottom: px(12), ..default() }, ..default() }).id();
    let chip = |c: &mut Commands, border: Color, click: Click| -> Entity {
        c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), flex_shrink: 0.0, padding: UiRect { left: px(14), right: px(12), top: px(8), bottom: px(7) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(border), click)).id()
    };
    let key = |c: &mut Commands, t: &str| -> Entity { c.spawn((Text::new(t), font(&f.mono_bold, 9.5, 0.0), TextColor(mute), LineHeight::Px(11.0), nowrap)).id() };
    if let Some(rule) = rule {
        let b = chip(c, with_alpha(line, 0.34), Click::Key(KeyCode::KeyA));
        let t = c.spawn((Text::new("ALWAYS ·"), font(&f.ui700, 11.5, 2.76), TextColor(hot), LineHeight::Px(12.5), nowrap)).id();
        // the rule as it will be saved, clipped only when the row is short of room
        let name = c.spawn((Text::new(clip(&rule, if same > 0 { 32 } else { 56 })), font(&f.mono, 12.0, 0.0), TextColor(hot), LineHeight::Px(14.0), nowrap)).id();
        let k = key(c, "A");
        c.entity(b).add_children(&[t, name, k]);
        c.entity(row).add_child(b);
    }
    if same > 0 {
        let b = chip(c, acc, Click::Alt(0));
        let t = c.spawn((Text::new(format!("ALLOW ALL {}", same + 1)), font(&f.ui700, 11.5, 2.76), TextColor(acc), LineHeight::Px(12.5), nowrap)).id();
        let k = key(c, "⇧ ENTER");
        c.entity(b).add_children(&[t, k]);
        c.entity(row).add_child(b);
    }
    row
}

/// `n`: how many requests wait; `same`: how many other agents' requests ask the same as `r`.
fn build(c: &mut Commands, f: &UiFonts, r: &RequestView, title: &str, m: &Moment, n: usize, same: usize) -> Entity {
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    let acc = role_color(Role::Acc);
    let mute = with_alpha(line, 0.62);
    let g1 = with_alpha(line, 0.72);
    let g2 = with_alpha(line, 0.34);
    let g3 = with_alpha(line, 0.13);
    let body = c
        .spawn(Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            padding: UiRect { left: px(28), right: px(28), top: px(16), bottom: px(17) },
            ..default()
        })
        .id();
    // header: chip, title, source
    let hdr = c
        .spawn((
            Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), padding: UiRect { bottom: px(14), ..default() }, border: UiRect { bottom: px(1), ..default() }, ..default() },
            BorderColor::all(with_alpha(line, 0.4)),
        ))
        .id();
    let chip = c
        .spawn((Node { padding: UiRect { left: px(8), right: px(9), top: px(5), bottom: px(4) }, column_gap: px(6), align_items: AlignItems::Center, flex_shrink: 0.0, ..default() }, BackgroundColor(acc)))
        .id();
    let dot = c.spawn((Node { width: px(5), height: px(5), ..default() }, BackgroundColor(Color::BLACK))).id();
    let nowrap = TextLayout { linebreak: LineBreak::NoWrap, ..default() };
    let ct = c.spawn((Text::new("NEEDS YOU"), font(&f.ui700, 10.5, 2.31), TextColor(Color::BLACK), LineHeight::Px(10.5), nowrap)).id();
    c.entity(chip).add_children(&[dot, ct]);
    let mut tt = title.to_uppercase();
    if tt.chars().count() > 22 {
        tt = tt.chars().take(21).collect::<String>() + "…";
    }
    let t = c
        .spawn((Text::new(tt), font(&f.ui600, 15.0, 3.0), TextColor(hot), LineHeight::Px(18.0), nowrap, Node { flex_grow: 1.0, flex_shrink: 1.0, overflow: Overflow::clip(), ..default() }))
        .id();
    let src = format!("{} · {}", r.info.tool, waited(&r.opened_at)).to_uppercase();
    let st = c.spawn((Text::new(src), font(&f.ui500, 11.0, 1.76), TextColor(mute), LineHeight::Px(13.0), nowrap, Node { flex_shrink: 0.0, ..default() })).id();
    c.entity(hdr).add_children(&[chip, t, st]);
    // the amber tick on the header rule
    let tick = c
        .spawn((Node { position_type: PositionType::Absolute, left: px(0), bottom: px(-1), width: px(44), height: px(2), ..default() }, BackgroundColor(acc)))
        .id();
    c.entity(hdr).add_child(tick);
    c.entity(body).add_child(hdr);

    // the ask: the agent's own words
    let ask = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(14), margin: UiRect { top: px(16), ..default() }, ..default() }).id();
    let av = c
        .spawn((
            Node { width: px(40), height: px(40), border: UiRect::all(px(1)), border_radius: BorderRadius::MAX, justify_content: JustifyContent::Center, align_items: AlignItems::Center, flex_shrink: 0.0, ..default() },
            BorderColor::all(g1),
        ))
        .id();
    let avt = c.spawn((Text::new("A"), font(&f.display, 14.0, 0.0), TextColor(hot))).id();
    c.entity(av).add_child(avt);
    let msg = c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(6), flex_grow: 1.0, flex_shrink: 1.0, ..default() }).id();
    let who = c.spawn((Text::new("AGENT"), font(&f.ui700, 12.5, 2.5), TextColor(hot), LineHeight::Px(15.0))).id();
    let summary = if r.info.summary.is_empty() { r.info.description.clone().unwrap_or_default() } else { r.info.summary.clone() };
    let said = c.spawn((Text::new(clip(&summary, 150)), font(&f.ui500, 17.0, 0.17), TextColor(line), LineHeight::Px(23.8))).id();
    c.entity(msg).add_children(&[who, said]);
    c.entity(ask).add_children(&[av, msg]);
    c.entity(body).add_child(ask);

    // the exact action
    let (lbl, text, mono) = action(r);
    let draft = c
        .spawn((
            Node { flex_direction: FlexDirection::Column, row_gap: px(9), margin: UiRect { top: px(16), ..default() }, padding: UiRect { left: px(20), top: px(10), bottom: px(4), ..default() }, border: UiRect { left: px(1), ..default() }, ..default() },
            BorderColor::all(g2),
        ))
        .id();
    let bar = c.spawn((Node { position_type: PositionType::Absolute, left: px(-2), top: px(0), width: px(3), height: px(22), ..default() }, BackgroundColor(acc))).id();
    let dl = c.spawn((Text::new(lbl.to_uppercase()), font(&f.ui700, 9.5, 3.04), TextColor(line), LineHeight::Px(9.5))).id();
    let dt = if mono {
        c.spawn((Text::new(text), font(&f.mono, 13.5, 0.0), TextColor(hot), LineHeight::Px(20.0))).id()
    } else {
        c.spawn((Text::new(text), font(&f.ui500, 18.0, 0.14), TextColor(hot), LineHeight::Px(25.0))).id()
    };
    c.entity(draft).add_children(&[bar, dl, dt]);
    if r.info.kind == RequestKind::Question {
        for (n, o) in options(&r.info.input).iter().take(5).enumerate() {
            let row = c
                .spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(12), padding: UiRect { left: px(8), right: px(10), top: px(4), bottom: px(3) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(g3), Click::Pick(n)))
                .id();
            let k = c.spawn((Text::new((n + 1).to_string()), font(&f.mono, 13.5, 0.0), TextColor(acc), LineHeight::Px(20.0))).id();
            let l = c.spawn((Text::new(clip(o, 64)), font(&f.ui500, 16.0, 0.16), TextColor(hot), LineHeight::Px(20.0))).id();
            c.entity(row).add_children(&[k, l]);
            c.entity(draft).add_child(row);
        }
    }
    // the same ask from other agents (the card's Allow all answers them too)
    if same > 0 && !m.editing {
        let also = c.spawn((Text::new(same_line(same)), font(&f.ui600, 12.5, 0.6), TextColor(acc), LineHeight::Px(15.0), nowrap)).id();
        c.entity(draft).add_child(also);
    }
    c.entity(body).add_child(draft);

    // what you're typing
    if m.editing {
        let you = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(6), margin: UiRect { top: px(10), ..default() }, ..default() }).id();
        let yl = c.spawn((Text::new("YOU"), font(&f.ui700, 9.5, 3.04), TextColor(mute), LineHeight::Px(22.0), Node { width: px(32), ..default() })).id();
        let prompt = if m.text.is_empty() {
            if r.info.kind == RequestKind::Question { "Type an answer, or a number…".to_string() } else { "Say what to change…".to_string() }
        } else {
            format!("\u{201C}{}\u{201D}", m.text)
        };
        let yt = c.spawn((Text::new(prompt), font(&f.ui500, 16.0, 0.16), TextColor(if m.text.is_empty() { mute } else { hot }), LineHeight::Px(22.0))).id();
        c.entity(you).add_children(&[yl, yt]);
        c.entity(body).add_child(you);
    }

    // why: the evidence
    let why = c.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(6), row_gap: px(6), align_items: AlignItems::Center, margin: UiRect { top: px(14), ..default() }, ..default() }).id();
    let wl = c.spawn((Text::new("WHY"), font(&f.ui700, 9.5, 3.04), TextColor(mute), LineHeight::Px(9.5), Node { width: px(32), ..default() })).id();
    c.entity(why).add_child(wl);
    let mut ev = vec![r.info.tool.clone()];
    if let Some(p) = &r.info.blocked_path {
        ev.push(format!("outside the worktree · {p}"));
    }
    if r.info.revises.is_some() {
        ev.push("revised after your change".into());
    }
    for e in ev {
        let chip = c.spawn((Node { padding: UiRect { left: px(8), right: px(7), top: px(4), bottom: px(3) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(g2))).id();
        let t = c.spawn((Text::new(first_lines(&e, 1, 60)), font(&f.mono, 11.0, 0.0), TextColor(line), LineHeight::Px(13.0))).id();
        c.entity(chip).add_child(t);
        c.entity(why).add_child(chip);
    }
    c.entity(body).add_child(why);

    // spacer pushes the actions to the bottom
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    c.entity(body).add_child(sp);

    // other ways to say yes
    let rule = always_rule(r);
    if !m.editing && (rule.is_some() || same > 0) {
        let more = more_ways(c, f, rule, same);
        c.entity(body).add_child(more);
    }

    // actions
    let acts = c
        .spawn((
            Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(14), padding: UiRect { top: px(16), ..default() }, border: UiRect { top: px(1), ..default() }, ..default() },
            BorderColor::all(g3),
        ))
        .id();
    let btn = |c: &mut Commands, label: &str, fill: bool, ghost: bool, click: Click| -> Entity {
        let b = c
            .spawn((
                Node { padding: if fill { UiRect { left: px(24), right: px(24), top: px(13), bottom: px(12) } } else { UiRect { left: px(17), right: px(17), top: px(12), bottom: px(11) } }, border: UiRect::all(px(1)), ..default() },
                BackgroundColor(if fill { acc } else { Color::NONE }),
                BorderColor::all(if fill { acc } else if ghost { Color::NONE } else { g2 }),
                click,
            ))
            .id();
        let t = c
            .spawn((Text::new(label.to_uppercase()), font(&f.ui700, if fill { 12.5 } else { 11.5 }, if fill { 3.5 } else { 2.76 }), TextColor(if fill { Color::BLACK } else if ghost { line } else { hot }), LineHeight::Px(12.5)))
            .id();
        c.entity(b).add_child(t);
        b
    };
    let mut row = if m.editing {
        vec![btn(c, "Send", true, false, Click::Key(KeyCode::Enter)), btn(c, "Cancel", false, true, Click::Key(KeyCode::Escape))]
    } else {
        vec![btn(c, yes_label(r), true, false, Click::Key(KeyCode::Enter)), btn(c, "Change", false, false, Click::Key(KeyCode::KeyE)), btn(c, "Not now", false, true, Click::Key(KeyCode::KeyL))]
    };
    let keys = c
        .spawn((Node { margin: UiRect { left: auto(), ..default() }, padding: UiRect { left: px(7), right: px(7), top: px(5), bottom: px(4) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(g3)))
        .id();
    let kt = c
        .spawn((Text::new(if m.editing { "ENTER · ESC" } else { "ENTER · E · L · ESC" }), font(&f.ui700, 9.5, 1.9), TextColor(mute), LineHeight::Px(9.5)))
        .id();
    c.entity(keys).add_child(kt);
    row.push(keys);
    c.entity(acts).add_children(&row);
    c.entity(body).add_child(acts);

    // foot
    let foot = c.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, margin: UiRect { top: px(14), ..default() }, ..default() }).id();
    let count = c.spawn((Text::new(format!("{} of {}", m.index + 1, n)), font(&f.mono, 11.0, 0.66), TextColor(hot), LineHeight::Px(11.0))).id();
    let nav = c.spawn((Text::new(if n > 1 { "← →  for the others" } else { "" }), font(&f.mono, 10.0, 0.5), TextColor(mute), LineHeight::Px(10.0))).id();
    c.entity(foot).add_children(&[count, nav]);
    c.entity(body).add_child(foot);
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: &str, status: RequestStatus) -> RequestView {
        let info: sphere_proto::RequestInfo = serde_json::from_value(serde_json::json!({
            "id": id, "task": format!("t-{id}"), "kind": "permission", "tool": "Bash", "summary": "", "input": {},
        }))
        .unwrap();
        RequestView { info, status, opened_at: "2026-10-19T00:00:00Z".into() }
    }

    #[test]
    fn the_header_says_how_long_a_request_has_waited_in_minutes_then_hours() {
        let at = "2026-10-19T12:00:00Z";
        let opened = crate::working::parse_rfc3339(at).unwrap();
        assert_eq!(waited_since(at, opened), "just now");
        assert_eq!(waited_since(at, opened + 59), "just now");
        assert_eq!(waited_since(at, opened + 60), "waiting 1 min");
        assert_eq!(waited_since(at, opened + 12 * 60 + 30), "waiting 12 min");
        assert_eq!(waited_since(at, opened + 3600), "waiting 1 h");
        assert_eq!(waited_since(at, opened + 3 * 3600 + 5 * 60), "waiting 3 h 5 min");
        // a long wait is not a count of minutes, and a clock behind the service's or a time that can't be read is just now
        assert_eq!(waited_since(at, opened + 4271 * 60), "waiting 71 h 11 min");
        assert_eq!(waited_since(at, opened - 5), "just now");
        assert_eq!(waited_since("not a time", opened), "just now");
    }

    #[test]
    fn a_request_put_off_stays_findable_but_does_not_raise_the_card() {
        let mut live = Live::default();
        live.requests = vec![req("r1", RequestStatus::Snoozed), req("r2", RequestStatus::Open), req("r3", RequestStatus::Open)];
        let mut m = Moment::default();
        let ids = |m: &Moment| waiting(&live, m).iter().map(|r| r.info.id.clone()).collect::<Vec<_>>();
        // put off before (sphered says Snoozed): still on the list, last
        assert_eq!(ids(&m), ["r2", "r3", "r1"]);
        assert_eq!(pending_count(&live, &m), 2);
        // put off here: the same, before sphered's snapshot catches up
        m.put_off.push("r2".into());
        assert_eq!(ids(&m), ["r3", "r1", "r2"]);
        assert_eq!(pending_count(&live, &m), 1);
        // answered here: gone
        m.answered.push("r3".into());
        assert_eq!(ids(&m), ["r1", "r2"]);
        assert_eq!(pending_count(&live, &m), 0);
        // the agent's own card (swarm, show_task) still finds a put-off request
        assert!(waiting(&live, &m).iter().any(|r| r.info.task == "t-r1"));
    }

    /// A permission from `task` for `tool` with `input`; `rule` is the content of the allow rule it suggests, if any.
    fn ask(id: &str, task: &str, tool: &str, input: Value, rule: Option<&str>) -> RequestView {
        let suggestions: Vec<Value> = rule.map(|c| serde_json::json!({"type": "addRules", "rules": [{"toolName": tool, "ruleContent": c}], "behavior": "allow", "destination": "localSettings"})).into_iter().collect();
        let info: sphere_proto::RequestInfo = serde_json::from_value(serde_json::json!({"id": id, "task": task, "kind": "permission", "tool": tool, "summary": "", "input": input, "suggestions": suggestions})).unwrap();
        RequestView { info, status: RequestStatus::Open, opened_at: "2026-10-19T00:00:00Z".into() }
    }

    fn npm(id: &str, task: &str, pkg: &str) -> RequestView {
        ask(id, task, "Bash", serde_json::json!({"command": format!("npm install {pkg}")}), Some("npm install:*"))
    }

    #[test]
    fn a_suggestion_becomes_the_rule_sphered_saves() {
        use serde_json::json;
        let bash = json!({"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "npm install:*"}], "behavior": "allow", "destination": "localSettings"});
        assert_eq!(rule_from_suggestion(&bash).as_deref(), Some("Bash(npm install:*)"));
        // no content, or empty content: the bare tool
        assert_eq!(rule_from_suggestion(&json!({"type": "addRules", "rules": [{"toolName": "WebFetch"}], "behavior": "allow"})).as_deref(), Some("WebFetch"));
        assert_eq!(rule_from_suggestion(&json!({"type": "addRules", "rules": [{"toolName": "WebFetch", "ruleContent": ""}], "behavior": "allow"})).as_deref(), Some("WebFetch"));
        // the first rule only
        assert_eq!(rule_from_suggestion(&json!({"type": "addRules", "rules": [{"toolName": "Read", "ruleContent": "/a/**"}, {"toolName": "Bash", "ruleContent": "ls:*"}], "behavior": "allow"})).as_deref(), Some("Read(/a/**)"));
        // not an allow rule: a mode change, a deny, no rules, no tool name, not even an object
        assert_eq!(rule_from_suggestion(&json!({"type": "setMode", "mode": "acceptEdits"})), None);
        assert_eq!(rule_from_suggestion(&json!({"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "x"}], "behavior": "deny"})), None);
        assert_eq!(rule_from_suggestion(&json!({"type": "addRules", "rules": [], "behavior": "allow"})), None);
        assert_eq!(rule_from_suggestion(&json!({"type": "addRules", "rules": [{"ruleContent": "x"}], "behavior": "allow"})), None);
        assert_eq!(rule_from_suggestion(&json!("addRules")), None);
    }

    #[test]
    fn always_is_for_a_permission_whose_first_suggestion_is_a_rule() {
        use serde_json::json;
        assert_eq!(always_rule(&npm("r1", "t1", "left-pad")).as_deref(), Some("Bash(npm install:*)"));
        // nothing suggested
        assert_eq!(always_rule(&ask("r2", "t2", "Bash", json!({}), None)), None);
        // sphered reads the first suggestion only, so a rule behind a mode change is not offered
        let mut behind = npm("r3", "t3", "x");
        behind.info.suggestions.insert(0, json!({"type": "setMode", "mode": "acceptEdits"}));
        assert_eq!(always_rule(&behind), None);
        // questions, plans and resume checks never get one, whatever they carry
        for kind in [RequestKind::Question, RequestKind::Plan, RequestKind::ResumeCheck] {
            let mut r = npm("r4", "t4", "x");
            r.info.kind = kind;
            assert_eq!(always_rule(&r), None, "{kind:?}");
        }
    }

    #[test]
    fn two_requests_ask_the_same_only_when_the_tool_and_the_input_are_exactly_the_same() {
        use serde_json::json;
        // the same command, from whichever agent
        assert!(same_ask(&npm("r1", "t1", "left-pad"), &npm("r2", "t2", "left-pad")));
        // the same rule is not the same ask: Allow all says yes to what was seen, not to what a rule would cover
        assert!(!same_ask(&npm("r1", "t1", "lodash"), &npm("r2", "t2", "react")));
        let push = |id: &str, cmd: &str| ask(id, "t", "Bash", json!({"command": cmd}), Some("git push:*"));
        assert!(!same_ask(&push("a", "git push"), &push("b", "git push --force")));
        // with a rule or without one (or another), the input decides
        let make = |id: &str, rule: Option<&str>| ask(id, "t", "Bash", json!({"command": "make test"}), rule);
        assert!(same_ask(&make("a", None), &make("b", None)));
        assert!(same_ask(&make("a", Some("make test:*")), &make("b", None)));
        assert!(same_ask(&make("a", Some("make test:*")), &make("b", Some("make:*"))));
        let lint = ask("b", "t", "Bash", json!({"command": "make lint"}), None);
        assert!(!same_ask(&make("a", None), &lint));
        // a word more in the input is another ask
        let said = |id: &str, why: &str| ask(id, "t", "Bash", json!({"command": "make test", "description": why}), None);
        assert!(same_ask(&said("a", "run the tests"), &said("b", "run the tests")));
        assert!(!same_ask(&said("a", "run the tests"), &said("b", "run them again")));
        // another tool is another ask, input or no input
        assert!(!same_ask(&ask("a", "t", "Edit", json!({"x": 1}), None), &ask("b", "t", "Write", json!({"x": 1}), None)));
        // only permissions ask the same
        let mut q1 = make("a", None);
        let mut q2 = make("b", None);
        q1.info.kind = RequestKind::Question;
        q2.info.kind = RequestKind::Question;
        assert!(!same_ask(&q1, &q2));
        assert!(!same_ask(&q1, &make("b", None)));
    }

    /// Twelve open requests, one per task but t1's two (r1 and r9). The `npm install left-pad` asks (r1, r2, r3, r5, r6, r9,
    /// r11) are the same; r4 is one of them put off before (Snoozed), r7 asks for another rule, r8 another tool, r10 is a
    /// question, and r12 installs another package under the very same rule as the rest.
    fn crowd() -> Live {
        use serde_json::json;
        let mut live = Live::default();
        let mut snoozed = npm("r4", "t4", "left-pad");
        snoozed.status = RequestStatus::Snoozed;
        let mut question = npm("r10", "t10", "left-pad");
        question.info.kind = RequestKind::Question;
        live.requests = vec![
            npm("r1", "t1", "left-pad"),
            npm("r2", "t2", "left-pad"),
            npm("r3", "t3", "left-pad"),
            snoozed,
            npm("r5", "t5", "left-pad"),
            npm("r6", "t6", "left-pad"),
            ask("r7", "t7", "Bash", json!({"command": "git push"}), Some("git push:*")),
            ask("r8", "t8", "Edit", json!({"file_path": "/a"}), None),
            npm("r9", "t1", "left-pad"),
            question,
            npm("r11", "t11", "left-pad"),
            npm("r12", "t12", "react"),
        ];
        live
    }

    #[test]
    fn allow_all_is_the_card_and_exactly_the_waiting_others_that_ask_the_same() {
        let live = crowd();
        let mut m = Moment::default();
        // r5 was put off here, r6 answered here, r4 was put off before (sphered says Snoozed)
        m.put_off.push("r5".into());
        m.answered.push("r6".into());
        let r1 = live.requests[0].clone();
        // not r4, r5, r6 (not waiting, or put off), r7, r8 (another ask), r9 (r1's own task, not another agent), r10 (a question),
        // and not r12, which installs another package under the same rule
        assert_eq!(allow_all_ids(&live, &m, &r1), ["r1", "r2", "r3", "r11"].map(String::from));
        // the card counts the same agents, other than itself
        assert_eq!(same_ask_others(&live, &m, &r1).len(), 3);
        // from r2's card r9 is another agent's (and r1, which is from the same task as r9, is too)
        let r2 = live.requests[1].clone();
        assert_eq!(allow_all_ids(&live, &m, &r2), ["r2", "r1", "r3", "r9", "r11"].map(String::from));
        // a card whose ask nobody else shares: just itself, even when others share its rule
        let r7 = live.requests[6].clone();
        assert_eq!(allow_all_ids(&live, &m, &r7), ["r7"].map(String::from));
        assert!(same_ask_others(&live, &m, &r7).is_empty());
        let r12 = live.requests[11].clone();
        assert_eq!(allow_all_ids(&live, &m, &r12), ["r12"].map(String::from));
        assert!(same_ask_others(&live, &m, &r12).is_empty());
        // worked out again after an answer here: the answered one drops out of the count
        m.answered.push("r2".into());
        assert_eq!(allow_all_ids(&live, &m, &r1), ["r1", "r3", "r11"].map(String::from));
    }

    // ---- the keys and the buttons, on a bare App

    use crate::live::ToastKind;
    use tokio::sync::mpsc::UnboundedReceiver;

    /// A bare App with the card up on `live`'s first request (as `draw` leaves it) and the service's commands readable.
    fn card(live: Live) -> (App, UnboundedReceiver<Cmd>) {
        let (svc, rx) = Service::with_receiver();
        let m = Moment { shown: true, current: Some((live.requests[0].clone(), "Install it".into())), ..default() };
        let mut app = App::new();
        app.add_message::<KeyboardInput>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Focus>()
            .init_resource::<AppClicks>()
            .init_resource::<Toasts>()
            .init_resource::<crate::sound::Sound>()
            .insert_resource(live)
            .insert_resource(m)
            .insert_resource(svc)
            .add_systems(Update, keys);
        (app, rx)
    }

    fn press_key(app: &mut App, key: Key, code: KeyCode, repeat: bool) {
        let text = if let Key::Character(c) = &key { Some(c.clone()) } else { None };
        app.world_mut().write_message(KeyboardInput { key_code: code, logical_key: key, state: ButtonState::Pressed, text, repeat, window: Entity::PLACEHOLDER });
        app.update();
    }

    fn press(app: &mut App, key: Key, code: KeyCode) {
        press_key(app, key, code, false);
    }

    fn hold(app: &mut App, code: KeyCode) {
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(code);
    }

    fn a_key() -> Key {
        Key::Character("a".into())
    }

    /// The answers sent so far: (request, answer, always).
    fn sent(rx: &mut UnboundedReceiver<Cmd>) -> Vec<(String, AnswerKind, bool)> {
        let mut out = Vec::new();
        while let Ok(Cmd::Answer { request, answer, always, .. }) = rx.try_recv() {
            out.push((request, answer, always));
        }
        out
    }

    fn yes(ids: &[&str], always: bool) -> Vec<(String, AnswerKind, bool)> {
        ids.iter().map(|i| (i.to_string(), AnswerKind::Yes, always)).collect()
    }

    /// `card`, and the drawing too: `draw` builds the card under a root node, so what the card says can be read back.
    fn drawn(live: Live) -> (App, UnboundedReceiver<Cmd>) {
        let (mut app, rx) = card(live);
        let root = app.world_mut().spawn(Node::default()).id();
        app.insert_resource(CardRoot(root))
            .insert_resource(UiFonts { ui500: default(), ui600: default(), ui700: default(), display: default(), mono: default(), mono_bold: default() })
            .add_systems(Update, draw.after(keys));
        app.world_mut().resource_mut::<Moment>().version = 1;
        (app, rx)
    }

    /// Every text on the card, sorted.
    fn texts(app: &mut App) -> Vec<String> {
        let world = app.world_mut();
        let mut said: Vec<String> = world.query::<&Text>().iter(world).map(|t| t.0.clone()).collect();
        said.sort();
        said
    }

    fn says(said: &[String], t: &str) -> bool {
        said.iter().any(|s| s == t)
    }

    #[test]
    fn the_card_offers_always_and_allow_all_only_where_they_apply() {
        use serde_json::json;
        let first_card = |requests: Vec<RequestView>| {
            let mut live = Live::default();
            live.requests = requests;
            let (mut app, _rx) = drawn(live);
            app.update();
            texts(&mut app)
        };
        let no_allow_all = |said: &[String]| !said.iter().any(|s| s.starts_with("ALLOW ALL") || s.contains("ask the same") || s.contains("asks the same"));
        // a rule, and two other agents asking the same: both ways, named and counted (the card is one of the three)
        let said = first_card(vec![npm("r1", "t1", "left-pad"), npm("r2", "t2", "left-pad"), npm("r3", "t3", "left-pad")]);
        assert!(says(&said, "ALWAYS ·") && says(&said, "Bash(npm install:*)"), "{said:?}");
        assert!(says(&said, "ALLOW ALL 3") && says(&said, "⇧ ENTER") && says(&said, "+2 more agents ask the same"), "{said:?}");
        // one other agent: the line is singular
        let said = first_card(vec![npm("r1", "t1", "left-pad"), npm("r2", "t2", "left-pad")]);
        assert!(says(&said, "ALLOW ALL 2") && says(&said, "+1 more agent asks the same"), "{said:?}");
        // another package under the same rule is not the same ask: Always only
        let said = first_card(vec![npm("r1", "t1", "lodash"), npm("r2", "t2", "react")]);
        assert!(says(&said, "ALWAYS ·") && no_allow_all(&said), "{said:?}");
        // a rule and nobody else: Always only
        let said = first_card(vec![npm("r1", "t1", "left-pad"), ask("r2", "t2", "Bash", json!({"command": "git push"}), Some("git push:*"))]);
        assert!(says(&said, "ALWAYS ·") && no_allow_all(&said), "{said:?}");
        // no rule but the very same input as another agent's: Allow all only
        let cmd = json!({"command": "make test"});
        let said = first_card(vec![ask("r1", "t1", "Bash", cmd.clone(), None), ask("r2", "t2", "Bash", cmd, None)]);
        assert!(says(&said, "ALLOW ALL 2") && !says(&said, "ALWAYS ·"), "{said:?}");
        // a question has neither, even with another agent's question beside it
        let (mut q1, mut q2) = (npm("r1", "t1", "left-pad"), npm("r2", "t2", "left-pad"));
        q1.info.kind = RequestKind::Question;
        q2.info.kind = RequestKind::Question;
        let said = first_card(vec![q1, q2]);
        assert!(!said.iter().any(|s| s.starts_with("ALWAYS")) && no_allow_all(&said), "{said:?}");
    }

    #[test]
    fn the_answered_card_keeps_its_line_and_buttons_while_it_folds() {
        let kept = |said: &[String]| says(said, "+5 more agents ask the same") && says(said, "ALLOW ALL 6") && says(said, "ALWAYS ·");
        // a plain yes answers only the card, but the card on its way out still reads as it did
        let (mut app, mut rx) = drawn(crowd());
        app.update();
        let before = texts(&mut app);
        assert!(kept(&before), "{before:?}");
        press(&mut app, Key::Enter, KeyCode::Enter);
        assert_eq!(sent(&mut rx), yes(&["r1"], false));
        assert!(app.world().resource::<Moment>().holding.is_some());
        let folding = texts(&mut app);
        assert!(kept(&folding), "{folding:?}");
        // Allow all takes the other five off the list at once, yet the card it answered still counts them
        let (mut app, mut rx) = drawn(crowd());
        app.update();
        hold(&mut app, KeyCode::ShiftLeft);
        press(&mut app, Key::Enter, KeyCode::Enter);
        assert_eq!(sent(&mut rx).len(), 6);
        let folding = texts(&mut app);
        assert!(kept(&folding), "{folding:?}");
    }

    #[test]
    fn shift_enter_says_yes_to_the_card_and_every_other_agent_that_asks_the_same() {
        let (mut app, mut rx) = card(crowd());
        hold(&mut app, KeyCode::ShiftLeft);
        press(&mut app, Key::Enter, KeyCode::Enter);
        // a plain yes (never Always) for the card and each of the five that ask the same, the card first
        assert_eq!(sent(&mut rx), yes(&["r1", "r2", "r3", "r5", "r6", "r11"], false));
        // the card folds like any yes, and everything answered is hidden until the snapshot drops it
        let (m, live) = (app.world().resource::<Moment>(), app.world().resource::<Live>());
        assert!(m.fold.is_some() && m.holding.as_ref().is_some_and(|(r, _)| r.info.id == "r1"));
        assert_eq!(m.answered, ["r1", "r2", "r3", "r5", "r6", "r11"]);
        // the next card is the first of what is left; r4 (put off before) comes last and does not raise the card
        assert_eq!(waiting(live, m).iter().map(|r| r.info.id.as_str()).collect::<Vec<_>>(), ["r7", "r8", "r9", "r10", "r12", "r4"]);
        assert_eq!(pending_count(live, m), 5);
        // a held Enter does not answer again while the card is folding (it only skips the fold)
        press_key(&mut app, Key::Enter, KeyCode::Enter, true);
        assert!(sent(&mut rx).is_empty());
    }

    #[test]
    fn answering_all_the_requests_that_wait_ends_the_moment() {
        let mut live = Live::default();
        live.requests = vec![npm("r1", "t1", "left-pad"), npm("r2", "t2", "left-pad"), npm("r3", "t3", "left-pad")];
        let (mut app, mut rx) = card(live);
        hold(&mut app, KeyCode::ShiftRight);
        press(&mut app, Key::Enter, KeyCode::Enter);
        assert_eq!(sent(&mut rx), yes(&["r1", "r2", "r3"], false));
        let (m, live) = (app.world().resource::<Moment>(), app.world().resource::<Live>());
        // nothing is left to raise a card: the fold ends in the all-done flow
        assert_eq!(pending_count(live, m), 0);
        assert!(waiting(live, m).is_empty());
    }

    #[test]
    fn enter_answers_only_the_card_and_shift_enter_alone_is_just_a_yes() {
        let (mut app, mut rx) = card(crowd());
        press(&mut app, Key::Enter, KeyCode::Enter);
        assert_eq!(sent(&mut rx), yes(&["r1"], false));
        // Shift+Enter on a card nobody else shares
        let mut live = Live::default();
        live.requests = vec![ask("r7", "t7", "Bash", serde_json::json!({"command": "git push"}), Some("git push:*")), npm("r1", "t1", "left-pad")];
        let (mut app, mut rx) = card(live);
        hold(&mut app, KeyCode::ShiftLeft);
        press(&mut app, Key::Enter, KeyCode::Enter);
        assert_eq!(sent(&mut rx), yes(&["r7"], false));
        // the same rule for another package is not the same ask: only the card is answered
        let mut live = Live::default();
        live.requests = vec![npm("r1", "t1", "lodash"), npm("r2", "t2", "react")];
        let (mut app, mut rx) = card(live);
        hold(&mut app, KeyCode::ShiftLeft);
        press(&mut app, Key::Enter, KeyCode::Enter);
        assert_eq!(sent(&mut rx), yes(&["r1"], false));
    }

    #[test]
    fn shift_enter_on_a_question_still_asks_for_the_answer() {
        let mut live = Live::default();
        let mut q = npm("r1", "t1", "a");
        q.info.kind = RequestKind::Question;
        live.requests = vec![q, npm("r2", "t2", "b")];
        let (mut app, mut rx) = card(live);
        hold(&mut app, KeyCode::ShiftLeft);
        press(&mut app, Key::Enter, KeyCode::Enter);
        assert!(sent(&mut rx).is_empty());
        assert!(app.world().resource::<Moment>().editing);
    }

    #[test]
    fn a_says_yes_saves_the_rule_and_says_so() {
        let (mut app, mut rx) = card(crowd());
        press(&mut app, a_key(), KeyCode::KeyA);
        assert_eq!(sent(&mut rx), yes(&["r1"], true));
        let toasts: Vec<(String, ToastKind)> = app.world().resource::<Toasts>().list.iter().map(|t| (t.0.clone(), t.2)).collect();
        assert_eq!(toasts, [("Saved rule: Bash(npm install:*)".to_string(), ToastKind::Ok)]);
        // it folds like any yes, for this request only
        let m = app.world().resource::<Moment>();
        assert!(m.fold.is_some() && m.holding.is_some());
        assert_eq!(m.answered, ["r1"]);
    }

    #[test]
    fn a_does_nothing_without_a_rule_or_on_a_held_key_or_with_cmd() {
        use serde_json::json;
        let silent = |app: &mut App, rx: &mut UnboundedReceiver<Cmd>| {
            assert!(sent(rx).is_empty());
            assert!(app.world().resource::<Toasts>().list.is_empty());
            assert!(app.world().resource::<Moment>().fold.is_none());
        };
        // nothing suggested
        let mut live = Live::default();
        live.requests = vec![ask("r1", "t1", "Bash", json!({"command": "ls"}), None)];
        let (mut app, mut rx) = card(live);
        press(&mut app, a_key(), KeyCode::KeyA);
        silent(&mut app, &mut rx);
        // a question, a plan or a resume check, whatever they carry
        for kind in [RequestKind::Question, RequestKind::Plan, RequestKind::ResumeCheck] {
            let mut live = Live::default();
            let mut r = npm("r1", "t1", "a");
            r.info.kind = kind;
            live.requests = vec![r];
            let (mut app, mut rx) = card(live);
            press(&mut app, a_key(), KeyCode::KeyA);
            silent(&mut app, &mut rx);
        }
        // a held A, and Cmd+A (select all) or Ctrl+A on a card that does have a rule
        let (mut app, mut rx) = card(crowd());
        press_key(&mut app, a_key(), KeyCode::KeyA, true);
        silent(&mut app, &mut rx);
        for code in [KeyCode::SuperLeft, KeyCode::ControlLeft, KeyCode::AltLeft] {
            let (mut app, mut rx) = card(crowd());
            hold(&mut app, code);
            press(&mut app, a_key(), KeyCode::KeyA);
            silent(&mut app, &mut rx);
        }
        // while typing a change the key is a letter
        let (mut app, mut rx) = card(crowd());
        app.world_mut().resource_mut::<Moment>().editing = true;
        press(&mut app, a_key(), KeyCode::KeyA);
        assert!(sent(&mut rx).is_empty());
        assert_eq!(app.world().resource::<Moment>().text, "a");
    }

    #[test]
    fn the_buttons_do_what_their_keys_do() {
        let (mut app, mut rx) = card(crowd());
        app.world_mut().resource_mut::<AppClicks>().push(CLICK_APP, Click::Alt(0));
        app.update();
        assert_eq!(sent(&mut rx), yes(&["r1", "r2", "r3", "r5", "r6", "r11"], false));
        let (mut app, mut rx) = card(crowd());
        app.world_mut().resource_mut::<AppClicks>().push(CLICK_APP, Click::Key(KeyCode::KeyA));
        app.update();
        assert_eq!(sent(&mut rx), yes(&["r1"], true));
        assert_eq!(app.world().resource::<Toasts>().list.len(), 1);
    }
}
