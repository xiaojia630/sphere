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
    /// Containers opens on this one (from Ports' "tail logs")
    pub containers_focus: Option<String>,
}

/// The System app has no tile: it opens from the bar ("system", "top").
const SYSTEM: usize = 100;

impl Open {
    pub fn app(&mut self, i: usize, toasts: &mut Toasts) {
        match APPS.get(i) {
            Some((_, true)) => self.pending = Some(i),
            Some((name, false)) => toasts.push(format!("{name} is designed in a later pass")),
            None => {}
        }
    }
    pub fn system(&mut self) {
        self.pending = Some(SYSTEM);
    }
}

#[allow(clippy::type_complexity)]
pub fn apply(
    mut open: ResMut<Open>,
    mut focus: ResMut<Focus>,
    mut working: ResMut<crate::working::Working>,
    (mut code, mut git, mut ctr, mut ports, mut sys, mut files): (ResMut<crate::codeapp::CodeApp>, ResMut<crate::gitapp::GitApp>, ResMut<crate::containers::ContainersApp>, ResMut<crate::ports::PortsApp>, ResMut<crate::sysapp::SysApp>, ResMut<crate::filesapp::FilesApp>),
    mut sound: ResMut<crate::sound::Sound>,
    mut orb: ResMut<crate::orbit::Orb>,
) {
    if let Some(o) = open.toggle_orbit.take() {
        orb.set(o, &mut sound);
    }
    let Some(i) = open.pending.take() else { return };
    sound.play_with("open", 0.8, 0.0);
    // one app at a time
    if matches!(i, 2..=6 | SYSTEM) {
        working.close();
        code.close();
        git.close();
        ctr.close();
        ports.close();
        sys.close();
        files.close();
        sound.play_with("whoosh", 0.7, 0.0);
    }
    match i {
        0 => focus.terminal = true,
        2 => code.open(),
        3 => git.open(),
        4 => {
            ctr.open();
            if let Some(n) = open.containers_focus.take() {
                ctr.focus(n);
            }
        }
        5 => ports.open(),
        6 => files.open(),
        SYSTEM => sys.open(),
        _ => working.open_latest(),
    }
}
