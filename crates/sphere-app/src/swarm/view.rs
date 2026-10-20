//! Opening and closing the swarm: S from home (or "swarm" in the command bar), Esc back, T for every track, the
//! springs that carry the view in and the home out, the dock's tilt group, and the brand's sub line.
use super::{Mode, Swarm, Tilt};
use crate::apps::Open;
use crate::core::{CoreCtl, Sim};
use crate::live::Toasts;
use crate::motion::{self, Pace};
use crate::panel::Focus;
use crate::sound::Sound;
use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::text::{LetterSpacing, LineHeight};

pub fn build(app: &mut App) {
    app.add_systems(Startup, start.after(crate::homepanels::setup));
    app.add_systems(Update, (keys, step, brand_sub, brand_look).chain().after(crate::home::keys).before(crate::home::apply));
    app.add_systems(Update, crate::visor::dock_groups);
}

/// `--swarm`: open at start, for screenshots.
fn start(args: Res<crate::Args>, mut swarm: ResMut<Swarm>) {
    if args.swarm {
        swarm.open();
    }
}

/// Orbit tilt from the dock or the command bar.
pub fn set_tilt(swarm: &mut Swarm, t: Tilt, sound: &mut Sound, toasts: &mut Toasts) {
    swarm.tilt = t;
    swarm.version += 1;
    sound.play_with("whoosh", 0.6, 0.0);
    toasts.push_quiet(format!("Orbit tilt · {}", match t { Tilt::Low => "low", Tilt::Mid => "mid", Tilt::High => "high" }));
}

/// T, the dock's Tracks button and "tracks".
pub fn toggle_tracks(swarm: &mut Swarm, sound: &mut Sound, toasts: &mut Toasts) {
    swarm.tracks = !swarm.tracks;
    sound.play("toggle");
    toasts.push_quiet(if swarm.tracks { "Step tracks on" } else { "Step tracks follow the pointer" });
}

/// S opens (from the home only), Esc goes back (focus and request to the overview, the overview closes), T tracks.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn keys(
    mut ev: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    focus: Res<Focus>,
    moment: Res<crate::moment::Moment>,
    working: Res<crate::working::Working>,
    mut swarm: ResMut<Swarm>,
    mut open: ResMut<Open>,
    mut sound: ResMut<Sound>,
    mut toasts: ResMut<Toasts>,
    apps: (Res<crate::codeapp::CodeApp>, Res<crate::gitapp::GitApp>, Res<crate::containers::ContainersApp>, Res<crate::ports::PortsApp>, Res<crate::sysapp::SysApp>, Res<crate::filesapp::FilesApp>),
) {
    let mods = keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::AltLeft, KeyCode::AltRight]);
    let app_open = apps.0.open || apps.1.open || apps.2.open || apps.3.open || apps.4.open || apps.5.open;
    for e in ev.read() {
        if e.state != ButtonState::Pressed || mods || focus.cmd || focus.terminal {
            continue;
        }
        if !swarm.open {
            if e.key_code == KeyCode::KeyS && !e.repeat && !app_open && !working.open && !moment.shown {
                open.swarm = true;
            }
            continue;
        }
        // the game has its own Esc
        if swarm.game {
            continue;
        }
        match e.key_code {
            KeyCode::Escape if !moment.shown || matches!(swarm.mode, Mode::Request(_)) => {
                if swarm.mode == Mode::Overview {
                    swarm.close();
                    sound.play_with("close", 0.7, 0.0);
                } else {
                    // a focus closes (`close`); a request card was already put back by its own Esc (`collapse`)
                    sound.play_with(if matches!(swarm.mode, Mode::Focus(_)) { "close" } else { "collapse" }, 0.7, 0.0);
                    swarm.mode = Mode::Overview;
                    swarm.version += 1;
                }
            }
            KeyCode::KeyT if !e.repeat && !moment.shown => toggle_tracks(&mut swarm, &mut sound, &mut toasts),
            _ => {}
        }
    }
}

/// The springs: open, the dim behind a focus or a request, the focus slide, the orbit tilt; and the core's.
#[allow(clippy::type_complexity)]
pub fn step(
    sim: Res<Sim>,
    mut swarm: ResMut<Swarm>,
    mut ctl: ResMut<CoreCtl>,
    working: Res<crate::working::Working>,
    apps: (Res<crate::codeapp::CodeApp>, Res<crate::gitapp::GitApp>, Res<crate::containers::ContainersApp>, Res<crate::ports::PortsApp>, Res<crate::sysapp::SysApp>, Res<crate::filesapp::FilesApp>),
) {
    // the Working view or an app opened by any route takes over
    if swarm.open && (working.open || apps.0.open || apps.1.open || apps.2.open || apps.3.open || apps.4.open || apps.5.open) {
        swarm.close();
    }
    let big = motion::dt(Pace::Big, sim.dt);
    let ui = motion::dt(Pace::Ui, sim.dt);
    let focus = matches!(swarm.mode, Mode::Focus(_));
    let dim = focus || matches!(swarm.mode, Mode::Request(_)) || (swarm.game && swarm.game_panel);
    swarm.amt.t = if swarm.open { 1.0 } else { 0.0 };
    swarm.dim.t = if dim { 1.0 } else { 0.0 };
    swarm.focus_amt.t = if focus { 1.0 } else { 0.0 };
    swarm.game_amt.t = if swarm.game { 1.0 } else { 0.0 };
    swarm.orbit_tilt.t = swarm.tilt.deg();
    swarm.amt.step(big);
    swarm.dim.step(ui);
    swarm.focus_amt.step(ui);
    swarm.game_amt.step(ui);
    swarm.orbit_tilt.step(ui);
    ctl.swarm = swarm.amt.x.clamp(0.0, 1.0);
    ctl.swarm_focus = swarm.focus_amt.x.clamp(0.0, 1.0);
    ctl.swarm_game = swarm.game_amt.x.clamp(0.0, 1.0);
}

/// The brand's sub line reads "Agent swarm" while the swarm is open.
fn brand_sub(swarm: Res<Swarm>, mut q: Query<&mut Text, With<crate::homepanels::BrandSub>>) {
    let want = if swarm.open && swarm.game { "HOLD THE SWARM" } else if swarm.open { "AGENT SWARM" } else { "AGENT OPERATING SYSTEM" };
    for mut t in &mut q {
        if t.0 != want {
            t.0 = want.into();
        }
    }
}

/// In the swarm the wordmark moves to the top left (the panel does, `panel::update_panels`): left aligned, the
/// wordmark 19 px (23 on the home) and the sub line 10 px (SPEC 1.3 p-brand).
#[allow(clippy::type_complexity)]
fn brand_look(
    swarm: Res<Swarm>,
    subs: Query<(Entity, &ChildOf), With<crate::homepanels::BrandSub>>,
    kids: Query<&Children>,
    mut nodes: Query<&mut Node>,
    mut texts: Query<(&mut TextFont, &mut LetterSpacing, &mut LineHeight)>,
    mut was: Local<Option<bool>>,
) {
    if *was == Some(swarm.open) {
        return;
    }
    let Ok((sub, parent)) = subs.single() else { return };
    *was = Some(swarm.open);
    let sw = swarm.open;
    let style = |texts: &mut Query<(&mut TextFont, &mut LetterSpacing, &mut LineHeight)>, e: Entity, size: f32, spacing: f32| {
        if let Ok((mut f, mut l, mut h)) = texts.get_mut(e) {
            f.font_size = FontSize::Px(size);
            *l = LetterSpacing::Px(spacing);
            *h = LineHeight::Px(size);
        }
    };
    if let Ok(mut n) = nodes.get_mut(parent.parent()) {
        n.align_items = if sw { AlignItems::FlexStart } else { AlignItems::Center };
    }
    style(&mut texts, sub, if sw { 10.0 } else { 11.0 }, if sw { 4.4 } else { 5.72 });
    // the wordmark row is the other child of the sub line's parent; its first child is the "SPHERE" text
    let Ok(sibs) = kids.get(parent.parent()) else { return };
    for row in sibs.iter().filter(|e| *e != sub) {
        if let Ok(mut n) = nodes.get_mut(row) {
            n.padding.left = px(if sw { 0.0 } else { 14.3 });
        }
        if let Some(word) = kids.get(row).ok().and_then(|c| c.first().copied()) {
            style(&mut texts, word, if sw { 19.0 } else { 23.0 }, if sw { 9.5 } else { 14.26 });
        }
    }
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
            .init_resource::<Swarm>()
            .init_resource::<Open>()
            .init_resource::<Sound>()
            .init_resource::<Toasts>()
            .init_resource::<crate::home::Acts>()
            .init_resource::<crate::working::Working>()
            .init_resource::<crate::orbit::Orb>()
            .init_resource::<crate::codeapp::CodeApp>()
            .init_resource::<crate::gitapp::GitApp>()
            .init_resource::<crate::containers::ContainersApp>()
            .init_resource::<crate::ports::PortsApp>()
            .init_resource::<crate::sysapp::SysApp>()
            .init_resource::<crate::filesapp::FilesApp>()
            .add_systems(Update, (crate::home::keys, keys, crate::apps::apply).chain());
        app
    }

    #[test]
    fn s_opens_and_esc_closes() {
        let mut app = home();
        press(&mut app, KeyCode::KeyS);
        assert!(app.world().resource::<Swarm>().open);
        press(&mut app, KeyCode::Escape);
        assert!(!app.world().resource::<Swarm>().open);
    }

    #[test]
    fn s_does_nothing_in_an_app_or_the_working_view_or_the_command_bar() {
        let mut app = home();
        app.world_mut().resource_mut::<crate::codeapp::CodeApp>().open = true;
        press(&mut app, KeyCode::KeyS);
        assert!(!app.world().resource::<Swarm>().open);
        app.world_mut().resource_mut::<crate::codeapp::CodeApp>().open = false;
        app.world_mut().resource_mut::<Focus>().cmd = true;
        press(&mut app, KeyCode::KeyS);
        assert!(!app.world().resource::<Swarm>().open);
    }

    #[test]
    fn opening_closes_the_working_view_and_esc_steps_back_from_focus() {
        let mut app = home();
        app.world_mut().resource_mut::<crate::working::Working>().open_latest();
        // S is quiet in the Working view; the command does it
        press(&mut app, KeyCode::KeyS);
        assert!(!app.world().resource::<Swarm>().open);
        app.world_mut().resource_mut::<Open>().swarm = true;
        app.update();
        assert!(app.world().resource::<Swarm>().open && !app.world().resource::<crate::working::Working>().open);
        app.world_mut().resource_mut::<Swarm>().mode = Mode::Focus("a".into());
        press(&mut app, KeyCode::Escape);
        let s = app.world().resource::<Swarm>();
        assert!(s.open && s.mode == Mode::Overview);
    }

    #[test]
    fn home_keys_stay_quiet_while_the_swarm_is_open() {
        let mut app = home();
        press(&mut app, KeyCode::KeyS);
        for k in [KeyCode::KeyM, KeyCode::KeyD, KeyCode::KeyE, KeyCode::KeyR, KeyCode::KeyF, KeyCode::KeyC, KeyCode::KeyX, KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3] {
            press(&mut app, k);
        }
        assert!(app.world().resource::<crate::home::Acts>().0.is_empty());
        press(&mut app, KeyCode::Escape);
        press(&mut app, KeyCode::KeyM);
        assert_eq!(app.world().resource::<crate::home::Acts>().0, vec![crate::home::Act::Mute]);
    }

    #[test]
    fn t_toggles_tracks_and_tilt_sets_with_a_toast() {
        let mut app = home();
        press(&mut app, KeyCode::KeyS);
        press(&mut app, KeyCode::KeyT);
        assert!(app.world().resource::<Swarm>().tracks);
        press(&mut app, KeyCode::KeyT);
        assert!(!app.world().resource::<Swarm>().tracks);
        let mut sw = Swarm::default();
        let (mut sound, mut toasts) = (Sound::default(), Toasts::default());
        set_tilt(&mut sw, Tilt::High, &mut sound, &mut toasts);
        assert_eq!(sw.tilt, Tilt::High);
    }

    #[test]
    fn swarm_commands_open_and_tilt() {
        let mut open = Open::default();
        let (mut acts, mut toasts, mut sw, mut sound) = (crate::home::Acts::default(), Toasts::default(), Swarm::default(), Sound::default());
        assert!(crate::cmdbar::home_command("all agents", &mut acts, &mut open, &mut toasts, &mut sw, &mut sound) && open.swarm);
        // tilt words are only the swarm's while it is open
        assert!(!crate::cmdbar::home_command("low", &mut acts, &mut open, &mut toasts, &mut sw, &mut sound));
        sw.open = true;
        assert!(crate::cmdbar::home_command("low", &mut acts, &mut open, &mut toasts, &mut sw, &mut sound) && sw.tilt == Tilt::Low);
        assert!(crate::cmdbar::home_command("tracks", &mut acts, &mut open, &mut toasts, &mut sw, &mut sound) && sw.tracks);
    }
}
