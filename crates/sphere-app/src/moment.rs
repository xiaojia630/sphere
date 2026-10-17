//! The Agent moment card (prototype: Agent moment): the oldest open request, centred over the core on a glass
//! panel. It shows the exact action (the command, the edit, the question, the plan), never only the agent's
//! description of it. Enter answers yes, E changes (type, Enter sends), L is later, Esc puts the card away
//! until something new arrives, and the arrows step through the others. Only these keys answer: the service
//! takes answers from the app and the CLI, never from an agent.
use crate::appframe::{AppClicks, Click};
use crate::fonts::UiFonts;
use crate::live::Live;
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
pub const CARD_H: f32 = 452.0;
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
}

/// The card's UI root (inside its panel texture).
#[derive(Resource)]
pub struct CardRoot(pub Entity);

/// What the card can show: open requests (not snoozed) that weren't just answered here.
pub fn waiting<'a>(live: &'a Live, m: &Moment) -> Vec<&'a RequestView> {
    live.requests.iter().filter(|r| r.status == RequestStatus::Open && !m.answered.contains(&r.info.id)).collect()
}

/// Shows the card when a request arrives that wasn't put away, hides it when nothing waits.
pub fn update(live: Res<Live>, mut m: ResMut<Moment>, mut seen: Local<u64>, mut sound: ResMut<crate::sound::Sound>) {
    if *seen == live.version {
        return;
    }
    *seen = live.version;
    let open: Vec<String> = live.requests.iter().map(|r| r.info.id.clone()).collect();
    m.answered.retain(|id| open.contains(id));
    m.dismissed.retain(|id| open.contains(id));
    let w: Vec<String> = waiting(&live, &m).iter().map(|r| r.info.id.clone()).collect();
    let fresh = w.iter().any(|id| !m.dismissed.contains(id));
    let shown = !w.is_empty() && (m.shown || fresh);
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

pub fn keys(mut ev: MessageReader<KeyboardInput>, focus: Res<Focus>, mut m: ResMut<Moment>, live: Res<Live>, svc: Res<Service>, mut sound: ResMut<crate::sound::Sound>, mut clicks: ResMut<AppClicks>) {
    // clicks on the card's buttons and option rows; they do what the same key does, and only on a card that is up and waiting
    let clicked = clicks.take(CLICK_APP);
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
    for e in ev.read() {
        if e.state != ButtonState::Pressed || !m.shown || focus.cmd || focus.terminal {
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
    // keep the answered card on screen while it folds (yes) or dims into orbit (later)
    if kind != AnswerKind::Change {
        m.holding = m.current.clone();
        if kind == AnswerKind::Later {
            m.later = Some(0.0);
        } else {
            m.fold = Some(0.0);
        }
    }
    m.answered.push(id.to_string());
    m.text.clear();
    m.editing = false;
    svc.send(Cmd::Answer { request: id.to_string(), answer: kind, text });
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
    // RFC 3339 from the service; minutes since then, from the system clock.
    let secs = crate::working::parse_rfc3339(opened_at);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    match secs {
        Some(t) if now - t >= 60 => format!("waiting {} min", (now - t) / 60),
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
        let body = build(&mut commands, &f, &r, &title, &m, 1);
        commands.entity(root.0).add_child(body);
        return;
    }
    let w = waiting(&live, &m);
    let Some(r) = w.get(m.index).map(|r| (*r).clone()) else { return };
    let title = live.title(&r.info.task).to_string();
    let n = w.len();
    let body = build(&mut commands, &f, &r, &title, &m, n);
    commands.entity(root.0).add_child(body);
    m.bypass_change_detection().current = Some((r, title));
}

fn build(c: &mut Commands, f: &UiFonts, r: &RequestView, title: &str, m: &Moment, n: usize) -> Entity {
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
