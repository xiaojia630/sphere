//! The home's actions (view modes, collapse, fit, reset, themes, mute) and their keys. Keys, the dock buttons and
//! typed commands all push an `Act`; `apply` carries it out with the prototype's sounds, flashes and toasts.
use crate::core::{CoreCtl, Mode};
use crate::live::Toasts;
use crate::panel::Focus;
use crate::sound::Sound;
use crate::theme::{Theme, ThemeId};
use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    Mode(Mode),
    ToggleDepth,
    Fit,
    /// `None` cycles to the next theme
    Theme(Option<ThemeId>),
    Reset,
    /// `None` toggles
    Collapse(Option<bool>),
    Mute,
}

#[derive(Resource, Default)]
pub struct Acts(pub Vec<Act>);

/// Camera pan (zoom lives in `CoreCtl`), in stage units; springs S(80,18) like the zoom.
#[derive(Resource)]
pub struct View {
    pub px: crate::util::Spring,
    pub py: crate::util::Spring,
}

impl Default for View {
    fn default() -> Self {
        View { px: crate::util::Spring::new(0.0, 80.0, 18.0), py: crate::util::Spring::new(0.0, 80.0, 18.0) }
    }
}

pub fn step_view(sim: Res<crate::core::Sim>, mut v: ResMut<View>) {
    let dt = crate::motion::dt(crate::motion::Pace::Ui, sim.dt);
    v.px.step(dt);
    v.py.step(dt);
}

/// Home keys (prototype v2): 1 Flat, 2/D Depth, 3/O Orbit, E Depth↔Flat, F fit, C theme, R/0 reset, X collapse,
/// M mute. Not while the command bar, the terminal or the Agent moment card has the keyboard, nor with Cmd/Ctrl/Alt,
/// nor while an app is open: the apps use the same letters (M merges, D discards, E asks, R runs the tests).
#[allow(clippy::type_complexity)]
pub fn keys(
    mut ev: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    focus: Res<Focus>,
    moment: Res<crate::moment::Moment>,
    swarm: Res<crate::swarm::Swarm>,
    mut acts: ResMut<Acts>,
    apps: (Res<crate::codeapp::CodeApp>, Res<crate::gitapp::GitApp>, Res<crate::containers::ContainersApp>, Res<crate::ports::PortsApp>, Res<crate::sysapp::SysApp>, Res<crate::filesapp::FilesApp>),
) {
    if apps.0.open || apps.1.open || apps.2.open || apps.3.open || apps.4.open || apps.5.open || swarm.open {
        ev.clear();
        return;
    }
    let mods = keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::AltLeft, KeyCode::AltRight]);
    for e in ev.read() {
        if e.state != ButtonState::Pressed || mods || focus.cmd || focus.terminal || moment.shown {
            continue;
        }
        let a = match e.key_code {
            KeyCode::Digit1 => Act::Mode(Mode::Flat),
            KeyCode::Digit2 | KeyCode::KeyD => Act::Mode(Mode::Depth),
            KeyCode::Digit3 | KeyCode::KeyO => Act::Mode(Mode::Orbit),
            KeyCode::KeyE => Act::ToggleDepth,
            KeyCode::KeyF => Act::Fit,
            KeyCode::KeyC => Act::Theme(None),
            KeyCode::KeyR | KeyCode::Digit0 => Act::Reset,
            KeyCode::KeyX if !e.repeat => Act::Collapse(None),
            KeyCode::KeyM if !e.repeat => Act::Mute,
            _ => continue,
        };
        acts.0.push(a);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn apply(
    mut acts: ResMut<Acts>,
    mut ctl: ResMut<CoreCtl>,
    mut theme: ResMut<Theme>,
    mut sound: ResMut<Sound>,
    mut toasts: ResMut<Toasts>,
    mut view: ResMut<View>,
    mut focus: ResMut<Focus>,
    mut fx: ResMut<crate::fx::Fx>,
) {
    for a in std::mem::take(&mut acts.0) {
        match a {
            Act::Mode(m) => set_mode(&mut ctl, m, &mut sound, &mut toasts, &mut fx),
            Act::ToggleDepth => {
                let m = if ctl.mode == Mode::Depth { Mode::Flat } else { Mode::Depth };
                set_mode(&mut ctl, m, &mut sound, &mut toasts, &mut fx);
            }
            Act::Fit => {
                ctl.zoom.t = 1.0;
                view.px.t = 0.0;
                view.py.t = 0.0;
                sound.play_with("whoosh", 0.6, 0.0);
                toasts.push_quiet("Fit to screen");
            }
            Act::Theme(t) => {
                let next = t.unwrap_or_else(|| theme.id.next());
                theme.set(next);
                sound.play("theme");
                fx.sweep();
                fx.shock(crate::util::Role::Fill);
                toasts.push_quiet(format!("Theme · {}", next.label()));
            }
            Act::Reset => {
                ctl.set_mode(Mode::Flat);
                ctl.zoom.t = 1.0;
                view.px.t = 0.0;
                view.py.t = 0.0;
                if ctl.collapsed {
                    ctl.collapsed = false;
                    ctl.collapse_t = 0.0;
                }
                focus.terminal = false;
                fx.unfold_all = true;
                fx.reset_panels = true;
                sound.play_with("boot", 0.7, 0.0);
                fx.shock(crate::util::Role::Hot);
                ctl.flash = 0.6;
                toasts.push_quiet("Everything reset");
            }
            Act::Collapse(want) => {
                let c = want.unwrap_or(!ctl.collapsed);
                if c != ctl.collapsed {
                    ctl.collapsed = c;
                    ctl.collapse_t = 0.0;
                    sound.play(if c { "collapse" } else { "expand" });
                    toasts.push_quiet(if c { "Collapsed" } else { "Expanded" });
                }
            }
            Act::Mute => {
                let m = !sound.muted;
                sound.set_muted(m);
                if !m {
                    sound.play("toggle");
                }
                toasts.push_quiet(if m { "Sound off" } else { "Sound on" });
            }
        }
    }
}

fn set_mode(ctl: &mut CoreCtl, m: Mode, sound: &mut Sound, toasts: &mut Toasts, fx: &mut crate::fx::Fx) {
    if ctl.mode == m {
        ctl.set_mode(m);
        return;
    }
    ctl.set_mode(m);
    sound.play("whoosh");
    fx.shock(crate::util::Role::Hot);
    ctl.flash = 0.7;
    toasts.push_quiet(match m {
        Mode::Flat => "Flat",
        Mode::Depth => "Depth · layers pulled apart",
        Mode::Orbit => "Orbit · rings folded around the core",
    });
}

/// The drop when every collapsed ring has landed.
pub fn landed_sound(ctl: Res<CoreCtl>, mut was: Local<bool>, mut sound: ResMut<Sound>) {
    if ctl.landed && !*was {
        sound.play_with("drop", 0.5, 0.0);
    }
    *was = ctl.landed;
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::keyboard::Key;

    fn press(app: &mut App, code: KeyCode) {
        app.world_mut().write_message(KeyboardInput { key_code: code, logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified), state: ButtonState::Pressed, text: None, repeat: false, window: Entity::PLACEHOLDER });
        app.update();
    }

    fn home() -> App {
        let mut app = App::new();
        app.add_message::<KeyboardInput>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Focus>()
            .init_resource::<crate::moment::Moment>()
            .init_resource::<crate::swarm::Swarm>()
            .init_resource::<Acts>()
            .init_resource::<crate::codeapp::CodeApp>()
            .init_resource::<crate::gitapp::GitApp>()
            .init_resource::<crate::containers::ContainersApp>()
            .init_resource::<crate::ports::PortsApp>()
            .init_resource::<crate::sysapp::SysApp>()
            .init_resource::<crate::filesapp::FilesApp>()
            .add_systems(Update, keys);
        app
    }

    const LETTERS: [KeyCode; 10] = [KeyCode::KeyM, KeyCode::KeyD, KeyCode::KeyE, KeyCode::KeyR, KeyCode::KeyF, KeyCode::KeyC, KeyCode::KeyX, KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3];

    #[test]
    fn home_keys_work_on_the_home() {
        let mut app = home();
        press(&mut app, KeyCode::KeyM);
        assert_eq!(app.world().resource::<Acts>().0, vec![Act::Mute]);
    }

    #[test]
    fn home_keys_stay_quiet_in_every_app() {
        type Open = fn(&mut World, bool);
        let opens: [(&str, Open); 6] = [
            ("Code", |w, o| w.resource_mut::<crate::codeapp::CodeApp>().open = o),
            ("Git", |w, o| w.resource_mut::<crate::gitapp::GitApp>().open = o),
            ("Containers", |w, o| w.resource_mut::<crate::containers::ContainersApp>().open = o),
            ("Ports", |w, o| w.resource_mut::<crate::ports::PortsApp>().open = o),
            ("System", |w, o| w.resource_mut::<crate::sysapp::SysApp>().open = o),
            ("Files", |w, o| w.resource_mut::<crate::filesapp::FilesApp>().open = o),
        ];
        for (name, set) in opens {
            let mut app = home();
            set(app.world_mut(), true);
            for k in LETTERS {
                press(&mut app, k);
            }
            assert!(app.world().resource::<Acts>().0.is_empty(), "{name}: home keys fired: {:?}", app.world().resource::<Acts>().0);
            // and a key pressed in the app doesn't fire later, once it closes
            set(app.world_mut(), false);
            app.update();
            assert!(app.world().resource::<Acts>().0.is_empty(), "{name}: a key leaked after closing");
        }
    }
}
