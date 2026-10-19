//! The swarm's feedback: what each `model::Events` entry sounds like, says, toasts and adds to the ticker
//! (SPEC section 6), the swarm ticker's items, the idle hint per mode, and the spoken status line.
use super::model::{self, Event, Model, State};
use super::{Mode, Swarm};
use crate::util::Role;
use bevy::prelude::*;

/// The swarm ticker's event items, newest first: (key, value, warn).
#[derive(Resource, Default)]
pub struct SwarmTicker(pub Vec<(String, String, bool)>);

/// SPEC 6.3: the list is capped at 14.
pub const TICKER_MAX: usize = 14;

/// The sounds an event makes, as (name, gain) (SPEC 6.1). `overview`: the swarm is in Overview mode;
/// `relevant`: the event's agent is hovered or in focus (only a landed step cares).
pub fn cues(e: &Event, overview: bool, relevant: bool) -> Vec<(&'static str, f32)> {
    match e {
        Event::Ready(_) => vec![("done", 0.8)],
        Event::Started(_) => vec![("open", 0.6)],
        // the request card opens by itself from the overview; elsewhere it is only a toast
        Event::NeedsYou(_) if !overview => vec![("toast", 1.0)],
        Event::NeedsYou(_) => vec![],
        Event::ConflictNew(_) => vec![("think", 0.6)],
        Event::ConflictCleared(_) => vec![],
        Event::Merged(_) => vec![("done", 0.8)],
        // the stop action plays `discard` itself
        Event::Stopped(_) => vec![],
        Event::StepLanded(_) if relevant => vec![("tick", 0.12)],
        Event::StepLanded(_) => vec![],
    }
}

/// How long a landed step's tick keeps other sounds out (SPEC `sfxQ(.., 600)`).
const TICK_GAP_MS: u128 = 600;

/// SPEC 3.4: the merge's impact (the shock, the sweep, the toast, the line) lands .62 s after the satellite starts
/// to collapse into the core, at the motion pace.
const MERGE_IMPACT_SECS: f32 = 0.62;

/// Events whose effects wait for something on screen (the merge's collapse): (seconds left, event).
#[derive(Default)]
pub struct Later(Vec<(f32, Event)>);

impl Later {
    pub fn push(&mut self, secs: f32, e: Event) {
        self.0.push((secs, e));
    }
    /// Advances `dt` and returns the events that are due, in the order they were queued.
    pub fn tick(&mut self, dt: f32) -> Vec<Event> {
        let mut due = Vec::new();
        self.0.retain_mut(|(t, e)| {
            *t -= dt;
            if *t <= 0.0 {
                due.push(e.clone());
            }
            *t > 0.0
        });
        due
    }
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The toast an event makes here. A stop has none: its action toasts at once itself.
pub fn toast_for(e: &Event, m: &Model) -> Option<(String, model::ToastKind)> {
    if matches!(e, Event::Stopped(_)) {
        return None;
    }
    model::toast(e, m)
}

/// Drains `Events` while the swarm is open (and drops them when it is closed).
#[allow(clippy::too_many_arguments)]
pub fn feedback(
    swarm: Res<Swarm>,
    sim: Res<crate::core::Sim>,
    mut events: ResMut<model::Events>,
    mut toasts: ResMut<crate::live::Toasts>,
    mut sound: ResMut<crate::sound::Sound>,
    mut say: ResMut<crate::homepanels::Say>,
    mut fx: ResMut<crate::fx::Fx>,
    mut ctl: ResMut<crate::core::CoreCtl>,
    mut ticker: ResMut<SwarmTicker>,
    mut later: Local<Later>,
) {
    if !swarm.open {
        events.0.clear();
        later.0.clear();
        return;
    }
    let due = later.tick(sim.dt);
    if events.0.is_empty() && due.is_empty() {
        return;
    }
    let overview = swarm.mode == Mode::Overview;
    let m = &swarm.model;
    // (event, whether it is a delayed one coming due)
    let work: Vec<(Event, bool)> = std::mem::take(&mut events.0).into_iter().map(|e| (e, false)).chain(due.into_iter().map(|e| (e, true))).collect();
    for (e, was_late) in work {
        if matches!(e, Event::Merged(_)) && !was_late {
            // the ticker has it now; the rest waits for the collapse
            if let Some(t) = model::ticker(&e, m) {
                ticker.0.insert(0, t);
                ticker.0.truncate(TICKER_MAX);
            }
            later.push(crate::motion::delay(crate::motion::Pace::Big, MERGE_IMPACT_SECS), e);
            continue;
        }
        let id = match &e {
            Event::StepLanded(id) => Some(id.as_str()),
            _ => None,
        };
        let relevant = id.is_some_and(|id| swarm.hover.as_deref() == Some(id) || swarm.current() == Some(id));
        if let Some((s, kind)) = toast_for(&e, m) {
            toasts.push_kind(s, kind);
        }
        for (name, gain) in cues(&e, overview, relevant) {
            if matches!(e, Event::StepLanded(_)) {
                sound.play_q_with(name, gain, TICK_GAP_MS);
            } else {
                sound.play_with(name, gain, 0.0);
            }
        }
        match &e {
            Event::Ready(_) => {
                fx.shock(Role::Ok);
                ctl.flash = ctl.flash.max(0.3);
            }
            Event::Merged(_) => {
                fx.sweep();
                fx.shock(Role::Ok);
                ctl.flash = ctl.flash.max(0.55);
            }
            _ => {}
        }
        if overview {
            let busy = say.shown < say.text.chars().count();
            if !busy {
                if let Some(line) = model::say(&e, m) {
                    sound.speak(&line);
                    say.say(line);
                }
            }
        }
        if !was_late {
            if let Some(t) = model::ticker(&e, m) {
                ticker.0.insert(0, t);
                ticker.0.truncate(TICKER_MAX);
            }
        }
    }
}

/// The ticker's items while the swarm is open: its events, then the slots and the cost (SPEC 6.3).
pub fn ticker_items(events: &[(String, String, bool)], m: &Model) -> Vec<String> {
    let mut items: Vec<String> = events.iter().map(|(k, v, _)| format!("{k} {v}")).collect();
    items.push(format!("SWARM {} of {} slots busy · {} queued", m.running().len(), m.limit, m.count(State::Queued)));
    items.push(format!("COST ≈ ${:.2} today · API est.", m.cost_today));
    items
}

/// The Say line's idle hint for the swarm's mode (SPEC 4.3).
pub fn idle_hint(mode: &Mode) -> &'static str {
    match mode {
        Mode::Overview => "1-9 JUMP · TAB CYCLES · N NEXT THAT NEEDS YOU · ESC BACK · F FIT",
        Mode::Focus(_) => "ENTER MERGES OR OPENS IN CODE · A TO ASK · TAB FOR THE NEXT AGENT · ESC BACK",
        Mode::Request(_) => "ENTER TO SEND · E TO CHANGE · L FOR LATER · ← → FOR THE OTHERS · ESC BACK",
    }
}

/// SPEC 6.2: "6 agents: 3 working, 1 ready to merge, 2 queued. About $1.89 today, API estimate." (the clauses after
/// "working" only when there are some), then "Agents waited 12 min on you today." once `waited_secs` (what agents have
/// waited on you today, `Live::waited_today_secs`) is a minute or more.
pub fn status_line(m: &Model, waited_secs: u64) -> String {
    let n = m.agents.iter().filter(|a| !matches!(a.state, State::Merged | State::Stopped)).count();
    let mut s = format!("{n} {}: {} working", if n == 1 { "agent" } else { "agents" }, m.count(State::Working));
    for (c, what) in [(m.count(State::Needs), "needs you"), (m.count(State::Done), "ready to merge"), (m.count(State::Queued), "queued")] {
        if c > 0 {
            s.push_str(&format!(", {c} {what}"));
        }
    }
    s.push_str(&format!(". About ${:.2} today, API estimate.", m.cost_today));
    if waited_secs >= 60 {
        s.push_str(&format!(" Agents waited {} on you today.", crate::live::fmt_waited(waited_secs)));
    }
    s
}

pub fn build(app: &mut App) {
    app.init_resource::<SwarmTicker>().add_systems(Last, feedback).add_systems(Update, crate::visor::edge_text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::Conflict;

    fn m(states: &[State], cost: f64) -> Model {
        Model { agents: states.iter().enumerate().map(|(i, s)| model::Agent { id: i.to_string(), state: *s, ..default() }).collect(), limit: 3, cost_today: cost, ..default() }
    }

    #[test]
    fn status_line_matches_the_spec() {
        use State::*;
        assert_eq!(status_line(&m(&[Working, Working, Working, Done, Queued, Queued], 1.89), 0), "6 agents: 3 working, 1 ready to merge, 2 queued. About $1.89 today, API estimate.");
        assert_eq!(status_line(&m(&[Working, Needs, Done, Queued], 0.0), 0), "4 agents: 1 working, 1 needs you, 1 ready to merge, 1 queued. About $0.00 today, API estimate.");
        assert_eq!(status_line(&m(&[Working], 12.5), 0), "1 agent: 1 working. About $12.50 today, API estimate.");
        assert_eq!(status_line(&m(&[Working, Merged, Stopped], 1.0), 0), "1 agent: 1 working. About $1.00 today, API estimate.");
        assert_eq!(status_line(&Model::default(), 0), "0 agents: 0 working. About $0.00 today, API estimate.");
    }

    #[test]
    fn status_line_says_how_long_agents_waited_on_you_once_it_is_a_minute() {
        use State::*;
        let two = m(&[Working, Needs], 1.89);
        let base = "2 agents: 1 working, 1 needs you. About $1.89 today, API estimate.";
        // under a minute says nothing
        assert_eq!(status_line(&two, 59), base);
        assert_eq!(status_line(&two, 60), format!("{base} Agents waited 1 min on you today."));
        assert_eq!(status_line(&two, 12 * 60 + 59), format!("{base} Agents waited 12 min on you today."));
        assert_eq!(status_line(&two, 65 * 60), format!("{base} Agents waited 1 h 5 min on you today."));
        assert_eq!(status_line(&Model::default(), 3 * 3600), "0 agents: 0 working. About $0.00 today, API estimate. Agents waited 3 h on you today.");
    }

    #[test]
    fn events_map_to_the_specs_sounds() {
        let id = || "a".to_string();
        let c = Conflict { a: id(), b: "b".into(), file: "f".into() };
        assert_eq!(cues(&Event::Ready(id()), true, false), vec![("done", 0.8)]);
        assert_eq!(cues(&Event::Started(id()), true, false), vec![("open", 0.6)]);
        assert!(cues(&Event::NeedsYou(id()), true, false).is_empty());
        assert_eq!(cues(&Event::NeedsYou(id()), false, false), vec![("toast", 1.0)]);
        assert_eq!(cues(&Event::ConflictNew(c.clone()), true, false), vec![("think", 0.6)]);
        assert!(cues(&Event::ConflictCleared(c), true, false).is_empty());
        assert_eq!(cues(&Event::Merged(id()), true, false), vec![("done", 0.8)]);
        assert!(cues(&Event::Stopped(id()), true, false).is_empty());
        assert_eq!(cues(&Event::StepLanded(id()), true, true), vec![("tick", 0.12)]);
        assert!(cues(&Event::StepLanded(id()), true, false).is_empty());
    }

    #[test]
    fn a_merges_effects_wait_for_the_collapse() {
        let mut l = Later::default();
        l.push(0.62, Event::Merged("a".into()));
        l.push(0.2, Event::Merged("b".into()));
        assert!(l.tick(0.1).is_empty());
        assert_eq!(l.tick(0.15), vec![Event::Merged("b".into())]);
        assert!(l.tick(0.3).is_empty());
        assert_eq!(l.tick(0.1), vec![Event::Merged("a".into())]);
        assert!(l.is_empty());
        assert!(l.tick(5.0).is_empty());
    }

    #[test]
    fn a_stop_is_toasted_by_its_action_not_by_the_event() {
        let model = m(&[State::Stopped, State::Done], 0.0);
        assert_eq!(toast_for(&Event::Stopped("0".into()), &model), None);
        assert_eq!(toast_for(&Event::Ready("1".into()), &model).map(|t| t.1), Some(model::ToastKind::Ok));
    }

    #[test]
    fn the_swarm_ticker_ends_with_slots_and_cost() {
        use State::*;
        let model = m(&[Working, Needs, Queued], 1.84);
        let ev = vec![("API-SERVER".to_string(), "x · y".to_string(), true)];
        assert_eq!(ticker_items(&ev, &model), vec!["API-SERVER x · y", "SWARM 2 of 3 slots busy · 1 queued", "COST ≈ $1.84 today · API est."]);
    }

    #[test]
    fn idle_hints_follow_the_mode() {
        assert!(idle_hint(&Mode::Overview).starts_with("1-9 JUMP"));
        assert!(idle_hint(&Mode::Focus("a".into())).starts_with("ENTER MERGES"));
        assert!(idle_hint(&Mode::Request("a".into())).starts_with("ENTER TO SEND"));
        // every hint fits one line of the 780 px say panel at its letter spacing (about .55em + spacing per char at 12 px)
        for m in [Mode::Overview, Mode::Focus("a".into()), Mode::Request("a".into())] {
            let n = idle_hint(&m).chars().count();
            assert!(n as f32 * (12.0 * 0.55 + crate::homepanels::idle_spacing(n)) <= 780.0, "{m:?}: {n} chars");
        }
    }
}
