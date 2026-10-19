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

/// Drains `Events` while the swarm is open (and drops them when it is closed).
#[allow(clippy::too_many_arguments)]
pub fn feedback(
    swarm: Res<Swarm>,
    mut events: ResMut<model::Events>,
    mut toasts: ResMut<crate::live::Toasts>,
    mut sound: ResMut<crate::sound::Sound>,
    mut say: ResMut<crate::homepanels::Say>,
    mut fx: ResMut<crate::fx::Fx>,
    mut ctl: ResMut<crate::core::CoreCtl>,
    mut ticker: ResMut<SwarmTicker>,
) {
    if !swarm.open {
        events.0.clear();
        return;
    }
    if events.0.is_empty() {
        return;
    }
    let overview = swarm.mode == Mode::Overview;
    for e in std::mem::take(&mut events.0) {
        let m = &swarm.model;
        let id = match &e {
            Event::StepLanded(id) => Some(id.as_str()),
            _ => None,
        };
        let relevant = id.is_some_and(|id| swarm.hover.as_deref() == Some(id) || swarm.current() == Some(id));
        if let Some((s, _kind)) = model::toast(&e, m) {
            toasts.push_quiet(s);
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
        if let Some(t) = model::ticker(&e, m) {
            ticker.0.insert(0, t);
            ticker.0.truncate(TICKER_MAX);
        }
    }
}

/// The ticker's items while the swarm is open: its events, then the slots and the cost (SPEC 6.3).
pub fn ticker_items(events: &[(String, String, bool)], m: &Model) -> Vec<String> {
    let mut items: Vec<String> = events.iter().map(|(k, v, _)| format!("{k} {v}")).collect();
    items.push(format!("SWARM {} of {} slots busy · {} queued", m.running().len(), m.limit, m.count(State::Queued)));
    items.push(format!("COST ${:.2} today", m.cost_today));
    items
}

/// The Say line's idle hint for the swarm's mode (SPEC 4.3).
pub fn idle_hint(mode: &Mode) -> &'static str {
    match mode {
        Mode::Overview => "1 TO 9 JUMP TO AN AGENT · TAB CYCLES · N FOR THE NEXT ONE THAT NEEDS YOU · ESC BACK · F TO FIT",
        Mode::Focus(_) => "ENTER MERGES OR OPENS IN CODE · A TO ASK · TAB FOR THE NEXT AGENT · ESC BACK",
        Mode::Request(_) => "ENTER TO SEND · E TO CHANGE · L FOR LATER · ← → FOR THE OTHERS · ESC BACK",
    }
}

/// SPEC 6.2: "6 agents: 3 working, 1 ready to merge, 2 queued. Spent $1.89 today." (the clauses after
/// "working" only when there are some).
#[allow(dead_code)] // the command bar's "status" hooks it
pub fn status_line(m: &Model) -> String {
    let n = m.agents.iter().filter(|a| !matches!(a.state, State::Merged | State::Stopped)).count();
    let mut s = format!("{n} {}: {} working", if n == 1 { "agent" } else { "agents" }, m.count(State::Working));
    for (c, what) in [(m.count(State::Needs), "needs you"), (m.count(State::Done), "ready to merge"), (m.count(State::Queued), "queued")] {
        if c > 0 {
            s.push_str(&format!(", {c} {what}"));
        }
    }
    s.push_str(&format!(". Spent ${:.2} today.", m.cost_today));
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
        assert_eq!(status_line(&m(&[Working, Working, Working, Done, Queued, Queued], 1.89)), "6 agents: 3 working, 1 ready to merge, 2 queued. Spent $1.89 today.");
        assert_eq!(status_line(&m(&[Working, Needs, Done, Queued], 0.0)), "4 agents: 1 working, 1 needs you, 1 ready to merge, 1 queued. Spent $0.00 today.");
        assert_eq!(status_line(&m(&[Working], 12.5)), "1 agent: 1 working. Spent $12.50 today.");
        assert_eq!(status_line(&m(&[Working, Merged, Stopped], 1.0)), "1 agent: 1 working. Spent $1.00 today.");
        assert_eq!(status_line(&Model::default()), "0 agents: 0 working. Spent $0.00 today.");
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
    fn the_swarm_ticker_ends_with_slots_and_cost() {
        use State::*;
        let model = m(&[Working, Needs, Queued], 1.84);
        let ev = vec![("API-SERVER".to_string(), "x · y".to_string(), true)];
        assert_eq!(ticker_items(&ev, &model), vec!["API-SERVER x · y", "SWARM 2 of 3 slots busy · 1 queued", "COST $1.84 today"]);
    }

    #[test]
    fn idle_hints_follow_the_mode() {
        assert!(idle_hint(&Mode::Overview).starts_with("1 TO 9"));
        assert!(idle_hint(&Mode::Focus("a".into())).starts_with("ENTER MERGES"));
        assert!(idle_hint(&Mode::Request("a".into())).starts_with("ENTER TO SEND"));
    }
}
