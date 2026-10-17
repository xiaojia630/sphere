//! Opening the eight apps (the rolodex, the core-open orbit, clicks on the home). Terminal and Agents open real
//! panels; the others say they come in a later pass, like the prototype.
use crate::homepanels::APPS;
use crate::live::Toasts;
use crate::panel::Focus;
use bevy::prelude::*;

#[derive(Resource, Default)]
pub struct Open {
    pending: Option<usize>,
    /// typed "apps" / "close": open or close the core
    pub toggle_orbit: Option<bool>,
}

impl Open {
    pub fn app(&mut self, i: usize, toasts: &mut Toasts) {
        match APPS.get(i) {
            Some((_, true)) => self.pending = Some(i),
            Some((name, false)) => toasts.push(format!("{name} is designed in a later pass")),
            None => {}
        }
    }
}

pub fn apply(mut open: ResMut<Open>, mut focus: ResMut<Focus>, mut working: ResMut<crate::working::Working>, mut sound: ResMut<crate::sound::Sound>, mut orb: ResMut<crate::orbit::Orb>) {
    if let Some(o) = open.toggle_orbit.take() {
        orb.set(o, &mut sound);
    }
    let Some(i) = open.pending.take() else { return };
    sound.play_with("open", 0.8, 0.0);
    match i {
        0 => focus.terminal = true,
        _ => working.open_latest(),
    }
}
