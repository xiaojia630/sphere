//! Opening the apps (the rolodex, the core-open orbit, clicks on the home). Terminal and Agents open real
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
    /// S or "swarm": open the Agent swarm (one view at a time)
    pub swarm: bool,
    /// the swarm's "Open in Code": the Code app opens on this task
    pub code_task: Option<String>,
    /// "code <path>", or O in Files and Git: the Code app opens on this folder
    pub code_dir: Option<std::path::PathBuf>,
    /// the command bar's words while the swarm is open (next, merge, stop, a number, back), for `swarm::input`
    pub swarm_cmds: Vec<crate::swarm::input::SwarmCmd>,
    /// typed "take over" / "elsewhere": `takeover::commands` opens the card
    pub takeover: Option<crate::takeover::Words>,
    /// Go to: the Working view opens on this task
    pub work_task: Option<String>,
    /// the bar's `go <filter>`, the NEW indicator: open Go to with this filter typed
    pub goto: Option<String>,
    /// typed "memory": `memory::commands` opens the card of notes
    pub memory: bool,
    /// a click on the Agents panel (panel px): an "elsewhere" row opens its card, anywhere else opens the Working view
    pub agents_click: Option<bevy::math::Vec2>,
    /// typed "protocols": `protocols::commands` opens the card of what runs on a schedule
    pub protocols: bool,
    /// typed "play" (or `--game`): Hold the Swarm opens on the swarm (`game::control`)
    pub game: bool,
    /// a click on the Terminal panel (panel px): FIX IT and DISMISS on a failed command's strip, anywhere else focuses
    /// the terminal (`fixit::apply` finds which)
    pub term_click: Option<bevy::math::Vec2>,
}

/// The Code app's tile.
pub const CODE: usize = 2;
/// The Files app's tile.
pub const FILES: usize = 6;
/// The System app has no tile: it opens from the bar ("system", "top").
const SYSTEM: usize = 100;

impl Open {
    /// The app that is about to open (tests).
    #[cfg(test)]
    pub fn pending(&self) -> Option<usize> {
        self.pending
    }
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
    /// Opens the Code app on a folder (and remembers it as the last repo when it is one, so `code .` finds it again).
    pub fn code_folder(&mut self, dir: std::path::PathBuf) {
        if dir.join(".git").exists() {
            crate::cmdbar::remember(&dir.to_string_lossy());
        }
        self.code_dir = Some(dir);
        self.pending = Some(CODE);
    }
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn apply(
    mut open: ResMut<Open>,
    mut focus: ResMut<Focus>,
    mut working: ResMut<crate::working::Working>,
    (mut code, mut git, mut ctr, mut ports, mut sys, mut files): (ResMut<crate::codeapp::CodeApp>, ResMut<crate::gitapp::GitApp>, ResMut<crate::containers::ContainersApp>, ResMut<crate::ports::PortsApp>, ResMut<crate::sysapp::SysApp>, ResMut<crate::filesapp::FilesApp>),
    mut sound: ResMut<crate::sound::Sound>,
    mut orb: ResMut<crate::orbit::Orb>,
    mut swarm: ResMut<crate::swarm::Swarm>,
    terms: Option<ResMut<crate::term::Terms>>,
) {
    // Helix quit: the app it was opened from shows again, as you left it
    if let Some(a) = terms.and_then(|mut t| t.returned.take()) {
        if a == CODE && !code.open {
            code.reshow();
        } else if a == FILES && !files.open {
            files.open();
        }
        focus.terminal = false;
        sound.play_with("open", 0.8, 0.0);
    }
    if let Some(o) = open.toggle_orbit.take() {
        orb.set(o, &mut sound);
    }
    if std::mem::take(&mut open.swarm) && !swarm.open {
        working.close();
        code.close();
        git.close();
        ctr.close();
        ports.close();
        sys.close();
        files.close();
        swarm.open();
        sound.play_with("open", 0.8, 0.0);
        sound.play_with("whoosh", 0.7, 0.0);
    }
    let Some(i) = open.pending.take() else { return };
    sound.play_with("open", 0.8, 0.0);
    // an app or the Working view takes over from the swarm
    if i != 0 && swarm.open {
        swarm.close();
    }
    // one app at a time
    if matches!(i, 2..=6 | SYSTEM) || (i == 1 && open.work_task.is_some()) {
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
        2 => {
            if let Some(d) = open.code_dir.take() {
                code.open_folder(d);
            } else {
                code.open();
                if let Some(t) = open.code_task.take() {
                    code.select_task(Some(t));
                }
            }
        }
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
        _ => match open.work_task.take() {
            Some(t) => working.open_task(t),
            None => working.open_latest(),
        },
    }
}
