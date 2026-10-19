//! The Working view (prototype: Working), on a real agent's run: the core slides aside and turns toward the work,
//! the agent's steps stream along a 3D track (done steps trail back toward the core, the current one sits at the
//! head), a run panel, a live output panel and an edit panel show what it's doing. W toggles it, Esc goes home,
//! P interrupts the agent's turn, T brings the output forward. It opens when you start a task, or from the
//! Agents panel, its ring, or the Agents app.
use crate::core::{CoreCtl, CoreRoot};
use crate::fonts::UiFonts;
use crate::glow::{GlowMaterial, GlowSlots, Kind, ATTR_HOLO};
use crate::panel::*;
use crate::service::{Cmd, Run, RunStep, Service};
use crate::util::*;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;
use bevy::text::LineHeight;
use sphere_proto::{StepKind, TaskState, TaskView};

/// The track's control points (wide stage), from the core outward.
const CURVE: [[f32; 3]; 9] = [
    [0.0, 0.0, 0.0],
    [150.0, 50.0, 50.0],
    [290.0, 120.0, 100.0],
    [410.0, 180.0, 130.0],
    [520.0, 215.0, 140.0],
    [650.0, 232.0, 110.0],
    [790.0, 226.0, 50.0],
    [930.0, 250.0, -40.0],
    [1060.0, 300.0, -140.0],
];
const SAMPLES: usize = 420;
const DU: f32 = 0.10;
const CHIPS: usize = 5;
pub const RUN_W: f32 = 360.0;
pub const RUN_H: f32 = 430.0;
pub const OUT_W: f32 = 560.0;
pub const OUT_H: f32 = 300.0;
pub const EDIT_W: f32 = 420.0;
pub const EDIT_H: f32 = 210.0;
pub const CHIP_W: f32 = 250.0;
pub const CHIP_H: f32 = 56.0;

#[derive(Resource)]
pub struct Working {
    pub open: bool,
    /// the task on screen; `None` follows the latest
    pub task: Option<String>,
    pub version: u64,
    /// OUTPUT panel scroll, in lines up from the bottom
    pub out_scroll: usize,
    pub amt: Spring,
    pub big: Spring,
    pub head: Spring,
    reveal: f32,
    pts: Vec<Vec3>,
    head_u: f32,
    /// the step count the head last landed on (for the hand-off sounds)
    last_steps: usize,
    pulse: f32,
}

impl Default for Working {
    fn default() -> Self {
        let pts = sample_curve();
        // the head sits where the track passes x = 520
        let head_i = pts.iter().enumerate().min_by(|a, b| (a.1.x - 520.0).abs().total_cmp(&(b.1.x - 520.0).abs())).map(|(i, _)| i).unwrap_or(0);
        Working {
            open: false,
            task: None,
            version: 0,
            out_scroll: 0,
            amt: Spring::new(0.0, 24.0, 9.5),
            big: Spring::new(0.0, 40.0, 12.0),
            head: Spring::new(0.0, 30.0, 10.0),
            reveal: 0.0,
            head_u: head_i as f32 / (SAMPLES - 1) as f32,
            pts,
            last_steps: 0,
            pulse: 9.0,
        }
    }
}

impl Working {
    /// Opens the view on the most recent agent.
    pub fn open_latest(&mut self) {
        self.open = true;
        self.task = None;
        self.reveal = 0.0;
        self.version += 1;
    }
    pub fn close(&mut self) {
        self.open = false;
        self.big.t = 0.0;
        self.version += 1;
    }
    fn at(&self, u: f32) -> Vec3 {
        let f = clamp(u, 0.0, 1.0) * (SAMPLES - 1) as f32;
        let i = (f.floor() as usize).min(SAMPLES - 2);
        self.pts[i].lerp(self.pts[i + 1], f - i as f32)
    }
}

/// Centripetal Catmull-Rom through the control points, resampled evenly by length.
fn sample_curve() -> Vec<Vec3> {
    let p: Vec<Vec3> = CURVE.iter().map(|c| Vec3::new(c[0], c[1], c[2])).collect();
    let mut dense = Vec::new();
    for i in 0..p.len() - 1 {
        let p0 = if i == 0 { p[0] * 2.0 - p[1] } else { p[i - 1] };
        let (p1, p2) = (p[i], p[i + 1]);
        let p3 = if i + 2 < p.len() { p[i + 2] } else { p[i + 1] * 2.0 - p[i] };
        let tj = |a: Vec3, b: Vec3, t: f32| t + (b - a).length().sqrt().max(1e-4);
        let (t0, t1) = (0.0, tj(p0, p1, 0.0));
        let t2 = tj(p1, p2, t1);
        let t3 = tj(p2, p3, t2);
        for k in 0..60 {
            let t = lerp(t1, t2, k as f32 / 60.0);
            let a1 = p0 * ((t1 - t) / (t1 - t0)) + p1 * ((t - t0) / (t1 - t0));
            let a2 = p1 * ((t2 - t) / (t2 - t1)) + p2 * ((t - t1) / (t2 - t1));
            let a3 = p2 * ((t3 - t) / (t3 - t2)) + p3 * ((t - t2) / (t3 - t2));
            let b1 = a1 * ((t2 - t) / (t2 - t0)) + a2 * ((t - t0) / (t2 - t0));
            let b2 = a2 * ((t3 - t) / (t3 - t1)) + a3 * ((t - t1) / (t3 - t1));
            dense.push(b1 * ((t2 - t) / (t2 - t1)) + b2 * ((t - t1) / (t2 - t1)));
        }
    }
    dense.push(*p.last().unwrap());
    let mut len = vec![0.0f32];
    for i in 1..dense.len() {
        len.push(len[i - 1] + (dense[i] - dense[i - 1]).length());
    }
    let total = *len.last().unwrap();
    let mut out = Vec::with_capacity(SAMPLES);
    let mut j = 0;
    for k in 0..SAMPLES {
        let s = total * k as f32 / (SAMPLES - 1) as f32;
        while j + 1 < len.len() - 1 && len[j + 1] < s {
            j += 1;
        }
        let f = if len[j + 1] > len[j] { (s - len[j]) / (len[j + 1] - len[j]) } else { 0.0 };
        out.push(dense[j].lerp(dense[j + 1], clamp(f, 0.0, 1.0)));
    }
    out
}

#[derive(Component)]
pub struct Track;
#[derive(Component)]
pub struct TrackLine;
#[derive(Component)]
pub struct Head;
#[derive(Component)]
pub struct Chip(pub usize);
#[derive(Component)]
pub struct WorkPanel(pub u8);
#[derive(Resource)]
pub struct WorkRoots {
    run: Entity,
    out: Entity,
    edit: Entity,
    chips: [Entity; CHIPS],
}

fn line_mesh(pos: Vec<[f32; 3]>, k: Vec<f32>) -> Mesh {
    let mut m = Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::default());
    let at: Vec<[f32; 4]> = k.into_iter().map(|k| [k, 0.0, 0.0, 0.0]).collect();
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(ATTR_HOLO, at);
    m
}

#[allow(clippy::too_many_arguments)]
pub fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut gmats: ResMut<Assets<GlowMaterial>>,
    mut gslots: ResMut<GlowSlots>,
    mut atlas: ResMut<crate::panel::PanelAtlas>,
    mut pmats: ResMut<Assets<PanelMaterial>>,
    args: Res<crate::Args>,
) {
    let c = &mut commands;
    if args.work {
        let mut w = Working::default();
        w.open_latest();
        c.insert_resource(w);
    }
    // the track and the head live in world space, placed every frame from the core's position
    let track = c.spawn((Track, Transform::default(), Visibility::Hidden)).id();
    let lm = meshes.add(line_mesh(vec![[0.0; 3], [1.0, 0.0, 0.0]], vec![1.0, 1.0]));
    // the track's and the head's colour and alpha change every frame (see `update`), so each has a slot
    let mut m = GlowMaterial::new(Kind::Line, crate::theme::now_lin(Role::Line), 0.9);
    m.u.a.y = 0.0;
    gslots.attach(&mut m);
    let line = c.spawn((TrackLine, Mesh3d(lm), MeshMaterial3d(gmats.add(m)), Transform::default(), bevy::camera::visibility::NoFrustumCulling)).id();
    let mut g = crate::geo::Geo::new();
    crate::geo::ring(&mut g, 13.0, 2.2);
    crate::geo::ring(&mut g, 3.0, 6.0);
    crate::geo::dash_geo(&mut g, 24.0, 1.0, 3.0, 4.0);
    crate::geo::ticks_geo(&mut g, 30.0, 4, 5.0, 1.0, 0, 0.0);
    let mut hm = GlowMaterial::new(Kind::Ring, crate::theme::now_lin(Role::Hot), 1.0);
    hm.u.a.y = 0.0;
    gslots.attach(&mut hm);
    let head = c.spawn((Head, Mesh3d(meshes.add(g.into_mesh())), MeshMaterial3d(gmats.add(hm)), Transform::default())).id();
    c.entity(track).add_children(&[line, head]);
    // panels: run (left 54, top 54), output (right 1546, bottom 962), edit (right 1546, top 46)
    let mut mk = |w: f32, h: f32, home: Vec3, side: f32, k: u8| -> Entity {
        let ui = spawn_ui_panel(c, &mut atlas, w, h);
        let e = spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id: PanelId::Work, w, h, home, home_scale: 1.0, side, phase: 1.1 + k as f32, show_at: 0.0, unframed: false });
        c.entity(e).insert(WorkPanel(k));
        ui.root
    };
    let run = mk(RUN_W, RUN_H, crate::homepanels::anchor('l', 54.0, 't', 54.0, RUN_W, RUN_H), 1.0, 0);
    let out = mk(OUT_W, OUT_H, crate::homepanels::anchor('r', 1546.0, 'b', 962.0, OUT_W, OUT_H), -1.0, 1);
    let edit = mk(EDIT_W, EDIT_H, crate::homepanels::anchor('r', 1546.0, 't', 46.0, EDIT_W, EDIT_H), -1.0, 2);
    let mut chips = [Entity::PLACEHOLDER; CHIPS];
    for (i, ch) in chips.iter_mut().enumerate() {
        let ui = spawn_ui_panel(c, &mut atlas, CHIP_W, CHIP_H);
        let e = spawn_panel_quad(c, &mut meshes, &mut pmats, &ui, &PanelSpec { id: PanelId::Chip, w: CHIP_W, h: CHIP_H, home: Vec3::ZERO, home_scale: 1.0, side: 0.0, phase: 0.0, show_at: 0.0, unframed: true });
        c.entity(e).remove::<Panel>().insert((Chip(i), Visibility::Hidden));
        *ch = ui.root;
    }
    c.insert_resource(WorkRoots { run, out, edit, chips });
}

/// Which task the view follows: the one asked for, else the most recently updated.
pub fn pick<'a>(w: &Working, tasks: &'a [TaskView]) -> Option<&'a TaskView> {
    match &w.task {
        Some(id) => tasks.iter().find(|t| &t.id == id),
        None => tasks.iter().max_by(|a, b| a.updated_at.cmp(&b.updated_at)),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn keys(
    mut ev: MessageReader<bevy::input::keyboard::KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    focus: Res<Focus>,
    moment: Res<crate::moment::Moment>,
    mut w: ResMut<Working>,
    svc: Res<Service>,
    mut sound: ResMut<crate::sound::Sound>,
    mut toasts: ResMut<crate::live::Toasts>,
) {
    let mods = keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::AltLeft, KeyCode::AltRight]);
    for e in ev.read() {
        if e.state != bevy::input::ButtonState::Pressed || mods || focus.cmd || focus.terminal || moment.shown || e.repeat {
            continue;
        }
        match e.key_code {
            KeyCode::KeyW => {
                if w.open {
                    w.close();
                    sound.play_with("collapse", 0.7, 0.0);
                } else {
                    w.open_latest();
                    sound.play_with("whoosh", 0.7, 0.0);
                    sound.play_with("expand", 0.6, 0.0);
                }
            }
            KeyCode::Escape if w.open => {
                if w.big.t > 0.5 {
                    w.big.t = 0.0;
                    sound.play_with("close", 0.6, 0.0);
                } else {
                    w.close();
                    sound.play_with("collapse", 0.7, 0.0);
                }
            }
            KeyCode::KeyT if w.open => {
                let b = w.big.t < 0.5;
                w.big.t = if b { 1.0 } else { 0.0 };
                sound.play_with(if b { "open" } else { "close" }, 0.6, 0.0);
            }
            KeyCode::KeyP if w.open => {
                let task = { pick(&w, &svc.shared.lock().unwrap().tasks).filter(|t| matches!(t.state, TaskState::Working | TaskState::NeedsYou)).map(|t| t.id.clone()) };
                match task {
                    Some(t) => {
                        svc.send(Cmd::Interrupt { task: t });
                        sound.play_with("discard", 0.6, 0.0);
                        toasts.push_quiet("Interrupted · the agent stops this turn and waits");
                    }
                    None => toasts.push("Nothing running to interrupt"),
                }
            }
            _ => {}
        }
    }
}

/// Springs, the core's slide, the track, the head and the chips' places.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn update(
    sim: Res<crate::core::Sim>,
    mut w: ResMut<Working>,
    mut ctl: ResMut<CoreCtl>,
    svc: Res<Service>,
    mut sound: ResMut<crate::sound::Sound>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut gmats: ResMut<Assets<GlowMaterial>>,
    mut gslots: ResMut<GlowSlots>,
    mut pmats: ResMut<Assets<PanelMaterial>>,
    root: Query<&GlobalTransform, With<CoreRoot>>,
    mut sets: ParamSet<(
        Query<(&mut Transform, &mut Visibility), With<Track>>,
        Query<(&Mesh3d, &MeshMaterial3d<GlowMaterial>), With<TrackLine>>,
        Query<(&mut Transform, &MeshMaterial3d<GlowMaterial>), With<Head>>,
        Query<(&Chip, &mut Transform, &mut Visibility, &MeshMaterial3d<PanelMaterial>)>,
    )>,
) {
    let dt = crate::motion::dt(crate::motion::Pace::Big, sim.dt);
    w.amt.t = if w.open { 1.0 } else { 0.0 };
    w.amt.step(dt);
    w.big.step(dt);
    let amt = clamp(w.amt.x, 0.0, 1.0);
    ctl.work = amt;
    if w.open {
        w.reveal = (w.reveal + dt / 1.15).min(1.0);
    }
    if amt < 0.001 && !w.open {
        for (_, mut v) in &mut sets.p0() {
            v.set_if_neq(Visibility::Hidden);
        }
        for (_, _, mut v, _) in &mut sets.p3() {
            v.set_if_neq(Visibility::Hidden);
        }
        return;
    }
    let (task, n, prev_ok) = {
        let s = svc.shared.lock().unwrap();
        let t = pick(&w, &s.tasks).cloned();
        let r = t.as_ref().and_then(|t| s.runs.get(&t.id));
        let n = r.map(|r| r.steps.len()).unwrap_or(0);
        let prev_ok = r.and_then(|r| r.steps.get(n.wrapping_sub(2)).and_then(|s| s.ok));
        (t, n, prev_ok)
    };
    let running = task.as_ref().is_some_and(|t| t.state == TaskState::Working);
    ctl.work_spin = if amt > 0.1 && running { 1.45 } else { 1.0 };
    let done = task.as_ref().is_some_and(|t| t.state == TaskState::Done);
    let fail = task.as_ref().is_some_and(|t| matches!(t.state, TaskState::Failed | TaskState::Stopped));
    // the head springs to the current step; a new step plays the hand-off
    let cur = n.saturating_sub(1) as f32;
    w.head.t = if done { cur + 0.55 } else { cur };
    w.head.step(dt);
    if n != w.last_steps {
        if n > w.last_steps && amt > 0.5 {
            match prev_ok {
                Some(false) => sound.play_with("error", 0.4, 0.0),
                Some(true) => sound.play_with("tick", 0.5, 0.0),
                None => {}
            }
            w.pulse = 0.0;
        }
        w.last_steps = n;
    }
    w.pulse += dt;
    // track space: from the sphere's centre, scale .6 → 1 as the view opens
    let centre = root.single().map(|g| g.translation()).unwrap_or(Vec3::ZERO);
    let ts = lerp(0.6, 1.0, amt);
    for (mut tr, mut v) in &mut sets.p0() {
        tr.translation = centre;
        tr.scale = Vec3::splat(ts);
        v.set_if_neq(Visibility::Inherited);
    }
    // the ribbon: solid behind the head, dashed ahead, revealed outward; the ends fade
    let reveal = out_cubic(w.reveal);
    let hu = w.head_u;
    let mut pos = Vec::new();
    let mut ks = Vec::new();
    for i in 0..SAMPLES - 1 {
        let u = i as f32 / (SAMPLES - 1) as f32;
        if u > reveal {
            break;
        }
        let ahead = u > hu;
        if ahead && (i / 3) % 2 == 1 {
            continue;
        }
        let ends = clamp(u / 0.07, 0.0, 1.0) * (1.0 - clamp((u - 0.84) / 0.16, 0.0, 1.0));
        let k = ends * if ahead { 0.22 } else { 0.34 + 0.66 * (1.0 - (hu - u) * 0.8).max(0.3) };
        pos.push(w.pts[i].to_array());
        pos.push(w.pts[i + 1].to_array());
        ks.push(k);
        ks.push(k);
    }
    let role = if done { Role::Ok } else if fail { Role::Bad } else { Role::Line };
    let fade_big = 1.0 - 0.9 * w.big.x;
    for (m, h) in &sets.p1() {
        if let Some(mut mesh) = meshes.get_mut(&m.0) {
            *mesh = line_mesh(pos.clone(), ks.clone());
        }
        gslots.update(&mut gmats, &h.0, |u| {
            u.color = crate::theme::now_lin(role);
            u.a.x = 0.95 * amt * fade_big;
        });
    }
    // the head: at headU, beating; green when the run is done
    let head_p = w.at(hu);
    let beat = 0.8 + 0.2 * (4.0 * sim.t).sin() / 2.0;
    for (mut tr, h) in &mut sets.p2() {
        tr.translation = head_p;
        let land = if w.pulse < 0.8 { lerp(1.0, 1.25, 1.0 - w.pulse / 0.8) } else { 1.0 };
        tr.rotation = Quat::from_rotation_z(-0.9 * sim.t);
        tr.scale = Vec3::splat(land);
        gslots.update(&mut gmats, &h.0, |u| {
            u.color = crate::theme::now_lin(if done { Role::Ok } else if fail { Role::Bad } else { Role::Hot });
            u.a.x = beat * amt * reveal * fade_big;
        });
    }
    // chips: the current step at the head, done steps trailing back along the track
    let head_x = w.head.x;
    for (chip, mut tr, mut v, h) in &mut sets.p3() {
        let i = chip.0;
        let step_index = n as i64 - 1 - i as i64;
        let u = hu + (step_index as f32 - head_x) * DU;
        let show = step_index >= 0 && u > 0.02 && u < 0.97 && u <= reveal;
        let p = centre + w.at(u) * ts;
        let side = if step_index % 2 == 0 { 1.0 } else { -1.0 };
        let now = i == 0;
        tr.translation = p + Vec3::new(0.0, side * 48.0 + if now { 14.0 } else { 0.0 }, if now { 24.0 } else { 0.0 });
        tr.scale = Vec3::splat(if now { 1.12 } else { 0.86 } * ts);
        let ends = clamp((u - 0.02) / 0.08, 0.0, 1.0) * (1.0 - clamp((u - 0.86) / 0.11, 0.0, 1.0));
        let op = if show { amt * ends * if now { 1.0 } else { 0.82 } * fade_big } else { 0.0 };
        v.set_if_neq(if op > 0.004 { Visibility::Inherited } else { Visibility::Hidden });
        if pmats.get(&h.0).is_some_and(|m| m.u.size.w != op) {
            if let Some(mut m) = pmats.get_mut(&h.0) {
                m.u.size.w = op;
                m.u.q.x = 1.0;
            }
        }
    }
}

/// The work panels follow the view (and the output panel comes forward with T).
pub fn place_panels(w: Res<Working>, mut q: Query<(&WorkPanel, &mut Panel)>) {
    let amt = clamp(w.amt.x, 0.0, 1.0);
    let big = clamp(w.big.x, 0.0, 1.0);
    for (k, mut p) in &mut q {
        p.work = Some((amt, if k.0 == 1 { big } else { 0.0 }));
    }
}

fn kind_word(k: StepKind) -> &'static str {
    match k {
        StepKind::Read => "READ",
        StepKind::Search => "SEARCH",
        StepKind::Edit => "PATCH",
        StepKind::Run => "RUN",
        StepKind::Web => "WEB",
        StepKind::Delegate => "DELEGATE",
        StepKind::Plan => "PLAN",
        StepKind::Other => "STEP",
    }
}

fn clip(s: &str, n: usize) -> String {
    let s = s.lines().next().unwrap_or("").trim();
    if s.chars().count() > n {
        s.chars().take(n - 1).collect::<String>() + "…"
    } else {
        s.to_string()
    }
}

/// Word-wraps every non-empty line of `s` at `n` chars; only words longer than `n` are split.
fn wrap(s: &str, n: usize) -> Vec<String> {
    let n = n.max(1);
    let mut out = Vec::new();
    for l in s.lines().filter(|l| !l.trim().is_empty()) {
        let mut cur = String::new();
        let mut len = 0;
        for word in l.split_whitespace() {
            let mut w: Vec<char> = word.chars().collect();
            if len > 0 && len + 1 + w.len() <= n {
                cur.push(' ');
                cur.extend(w.iter());
                len += 1 + w.len();
                continue;
            }
            if len > 0 {
                out.push(std::mem::take(&mut cur));
            }
            while w.len() > n {
                out.push(w.drain(..n).collect());
            }
            len = w.len();
            cur = w.into_iter().collect();
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    out
}

/// Rebuilds the work panels' UI when the run changes (and once a second for the clock).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn draw(mut commands: Commands, svc: Res<Service>, roots: Option<Res<WorkRoots>>, f: Res<UiFonts>, time: Res<Time<Real>>, sim: Res<crate::core::Sim>, mut w: ResMut<Working>, mut seen: Local<(u64, Option<String>, u64, i64, usize)>, mut last: Local<(String, usize)>) {
    let Some(roots) = roots else { return };
    if w.amt.x < 0.001 && !w.open {
        return;
    }
    let (task, run) = {
        let s = svc.shared.lock().unwrap();
        let t = pick(&w, &s.tasks).cloned();
        let r = t.as_ref().and_then(|t| s.runs.get(&t.id).cloned()).unwrap_or_default();
        (t, r)
    };
    // the clock ticks once a second (a frozen look test stops it, so its screenshot can settle)
    let clock = if sim.frozen { seen.3 } else { time.elapsed_secs() as i64 };
    // a new task or a new reply snaps the output back to the bottom
    let cur = (task.as_ref().map(|t| t.id.clone()).unwrap_or_default(), run.says.len());
    if *last != cur {
        *last = cur;
        w.out_scroll = 0;
    }
    let key = (run.version, task.as_ref().map(|t| format!("{}{:?}{}", t.id, t.state, t.status)), w.version, clock, w.out_scroll);
    if *seen == key {
        return;
    }
    *seen = key;
    let c = &mut commands;
    for r in [roots.run, roots.out, roots.edit] {
        c.entity(r).despawn_children();
    }
    for r in roots.chips {
        c.entity(r).despawn_children();
    }
    let Some(t) = task else {
        let b = c.spawn((Text::new("NO AGENT YET · START ONE WITH /"), font(&f.ui600, 12.0, 3.0), TextColor(role_color(Role::Dim)), Node { margin: UiRect::all(px(18)), ..default() })).id();
        c.entity(roots.run).add_child(b);
        return;
    };
    build_run(c, roots.run, &f, &t, &run);
    let max = build_out(c, roots.out, &f, &t, &run, w.out_scroll);
    if w.out_scroll > max {
        w.out_scroll = max; // wheel overshoot; the next frame redraws at the clamped spot
    }
    build_edit(c, roots.edit, &f, &run);
    let n = run.steps.len();
    for (i, root) in roots.chips.iter().enumerate() {
        if i < n {
            build_chip(c, *root, &f, &run.steps[n - 1 - i], n - i, i == 0, &t);
        }
    }
}

fn state_chip(c: &mut Commands, f: &UiFonts, t: &TaskView) -> Entity {
    let (label, bg, fg, border) = match t.state {
        TaskState::Working | TaskState::Queued => ("WORKING", Color::NONE, role_color(Role::Line), with_alpha(role_color(Role::Line), 0.72)),
        TaskState::NeedsYou => ("NEEDS YOU", role_color(Role::Acc), Color::BLACK, role_color(Role::Acc)),
        TaskState::Done => ("DONE", Color::NONE, role_color(Role::Ok), with_alpha(role_color(Role::Ok), 0.72)),
        TaskState::Failed => ("FAILED", Color::NONE, role_color(Role::Bad), with_alpha(role_color(Role::Bad), 0.72)),
        TaskState::Stopped => ("STOPPED", Color::NONE, role_color(Role::Dim), with_alpha(role_color(Role::Dim), 0.72)),
    };
    let chip = c
        .spawn((Node { min_width: px(78), padding: UiRect { left: px(6), right: px(6), top: px(4), bottom: px(3) }, border: UiRect::all(px(1)), justify_content: JustifyContent::Center, flex_shrink: 0.0, ..default() }, BackgroundColor(bg), BorderColor::all(border)))
        .id();
    let tx = c.spawn((Text::new(label), font(&f.ui700, 10.0, 2.0), TextColor(fg), LineHeight::Px(10.0))).id();
    c.entity(chip).add_child(tx);
    chip
}

fn elapsed(t: &TaskView) -> String {
    let start = parse_rfc3339(&t.started_at);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let end = if matches!(t.state, TaskState::Working | TaskState::NeedsYou | TaskState::Queued) { now } else { parse_rfc3339(&t.updated_at).unwrap_or(now) };
    match start {
        Some(s) => {
            let d = (end - s).max(0);
            format!("{:02}:{:02}", d / 60, d % 60)
        }
        None => "--:--".into(),
    }
}

/// The head's tooltip: the task, and where it stands. `now` is unix seconds.
pub fn head_tip(t: &TaskView, run: &Run, now: i64) -> (String, String) {
    let live = matches!(t.state, TaskState::Working | TaskState::NeedsYou | TaskState::Queued);
    let end = if live { now } else { parse_rfc3339(&t.updated_at).unwrap_or(now) };
    let dur = parse_rfc3339(&t.started_at).map_or(String::new(), |s| {
        let d = (end - s).max(0);
        if d < 60 { format!("{d}s") } else if d < 3600 { format!("{}m", d / 60) } else { format!("{}h {}m", d / 3600, d % 3600 / 60) }
    });
    let n = run.steps.last().map_or(t.steps as u64, |s| s.seq + 1);
    let l2 = match t.state {
        TaskState::Working => format!("Step {n} · {} · {dur}", clip(run.steps.last().map_or("", |s| s.title.as_str()), 40)),
        TaskState::NeedsYou => "Needs you · waiting on your answer".to_string(),
        TaskState::Done if t.files.is_empty() => format!("Answered · {n} steps · {dur} · ≈${:.2}", t.cost_usd),
        TaskState::Done => format!("Done · {} files changed · click to review", t.files.len()),
        TaskState::Failed => format!("Failed · {}", clip(t.status.strip_prefix("Failed: ").unwrap_or(&t.status), 60)),
        TaskState::Stopped => "Stopped".to_string(),
        TaskState::Queued => "Queued".to_string(),
    };
    (t.title.to_uppercase(), l2)
}

/// "2026-10-17T00:55:33.309Z" (or with an offset) as unix seconds.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    if s.len() < 19 {
        return None;
    }
    let n = |a: usize, l: usize| s.get(a..a + l)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (n(0, 4)?, n(5, 2)?, n(8, 2)?, n(11, 2)?, n(14, 2)?, n(17, 2)?);
    // days from civil (Howard Hinnant's algorithm)
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let mut secs = days * 86400 + h * 3600 + mi * 60 + se;
    // an explicit offset like +01:00
    let tail = &s[19..];
    if let Some(i) = tail.find(['+', '-']) {
        let off = &tail[i..];
        if off.len() >= 6 {
            let sign = if off.starts_with('-') { -1 } else { 1 };
            let oh = off[1..3].parse::<i64>().unwrap_or(0);
            let om = off[4..6].parse::<i64>().unwrap_or(0);
            secs -= sign * (oh * 3600 + om * 60);
        }
    }
    Some(secs)
}

fn build_run(c: &mut Commands, root: Entity, f: &UiFonts, t: &TaskView, run: &Run) {
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    let dim = role_color(Role::Dim);
    let b = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, row_gap: px(8), padding: UiRect { left: px(18), right: px(18), top: px(14), bottom: px(16) }, ..default() }).id();
    c.entity(root).add_child(b);
    let hdr = c
        .spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), padding: UiRect { bottom: px(8), ..default() }, border: UiRect { bottom: px(1), ..default() }, ..default() }, BorderColor::all(with_alpha(line, 0.72))))
        .id();
    let chip = state_chip(c, f, t);
    let title = c
        .spawn((Text::new(clip(&t.title, 22).to_uppercase()), font(&f.ui600, 12.0, 2.4), TextColor(hot), LineHeight::Px(14.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { flex_grow: 1.0, flex_shrink: 1.0, overflow: Overflow::clip(), ..default() }))
        .id();
    let clk = c.spawn((Text::new(elapsed(t)), font(&f.mono, 11.5, 0.46), TextColor(hot), LineHeight::Px(14.0))).id();
    c.entity(hdr).add_children(&[chip, title, clk]);
    let repo = std::path::Path::new(&t.repo).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let meta = format!("Agent · Claude Code · {} · {}", t.branch.clone().unwrap_or_else(|| "in place".into()), repo);
    let m = c.spawn((Text::new(meta), font(&f.ui500, 12.0, 0.96), TextColor(line), LineHeight::Px(15.0))).id();
    // progress: the plan's completed share when there is a plan, else a curve on the step count
    let done_plan = t.plan.iter().filter(|p| p.status == "completed").count();
    let p = if !t.plan.is_empty() {
        done_plan as f32 / t.plan.len() as f32
    } else if t.state == TaskState::Done {
        1.0
    } else {
        1.0 - (-(run.steps.len() as f32) / 14.0).exp()
    };
    let pr = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), ..default() }).id();
    let track = c.spawn((Node { flex_grow: 1.0, height: px(3), ..default() }, BackgroundColor(with_alpha(line, 0.13)))).id();
    let fill_c = if t.state == TaskState::Done { role_color(Role::Ok) } else { role_color(Role::Fill) };
    let fill = c.spawn((Node { width: percent(p * 100.0), height: px(3), ..default() }, BackgroundColor(fill_c))).id();
    c.entity(track).add_child(fill);
    // a plan gives a real share; without one, only the number of steps is true
    let label = if !t.plan.is_empty() || t.state == TaskState::Done { format!("{:.0}%", p * 100.0) } else { format!("{} STEP{}", run.steps.len(), if run.steps.len() == 1 { "" } else { "S" }) };
    let pct = c.spawn((Text::new(label), font(&f.mono, 11.5, 0.46), TextColor(hot), LineHeight::Px(12.0))).id();
    c.entity(pr).add_children(&[track, pct]);
    c.entity(b).add_children(&[hdr, m, pr]);
    // steps: the last eight
    let list = c.spawn(Node { flex_direction: FlexDirection::Column, margin: UiRect { top: px(4), ..default() }, ..default() }).id();
    let n = run.steps.len();
    let first = n.saturating_sub(8);
    for (k, s) in run.steps.iter().enumerate().skip(first) {
        let now = k + 1 == n && s.ok.is_none();
        let row = c.spawn(Node { height: px(27), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), ..default() }).id();
        let (nc, nb) = match s.ok {
            Some(true) => (role_color(Role::Ok), role_color(Role::Ok)),
            Some(false) => (role_color(Role::Bad), role_color(Role::Bad)),
            None if now => (role_color(Role::Fill), role_color(Role::Fill)),
            None => (Color::NONE, dim),
        };
        let node = c.spawn((Node { width: px(7), height: px(7), border: UiRect::all(px(1)), ..default() }, BackgroundColor(nc), BorderColor::all(nb), UiTransform { rotation: Rot2::degrees(if now { 45.0 } else { 0.0 }), ..default() })).id();
        let idx = c.spawn((Text::new(format!("{:02}", k + 1)), font(&f.mono, 11.0, 0.0), TextColor(dim), LineHeight::Px(12.0))).id();
        let nm = c
            .spawn((Text::new(clip(&s.title, 34)), font(&f.ui500, 13.0, 0.5), TextColor(if now { hot } else { line }), LineHeight::Px(16.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { overflow: Overflow::clip(), flex_shrink: 1.0, ..default() }))
            .id();
        c.entity(row).add_children(&[node, idx, nm]);
        c.entity(list).add_child(row);
    }
    if n == 0 {
        let e = c.spawn((Text::new("Starting up…"), font(&f.ui500, 13.0, 0.5), TextColor(line), LineHeight::Px(16.0))).id();
        c.entity(list).add_child(e);
    }
    c.entity(b).add_child(list);
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    c.entity(b).add_child(sp);
    if !run.files.is_empty() {
        let fl = c.spawn((Text::new(format!("FILES TOUCHED · {}", run.files.len())), font(&f.ui700, 9.5, 3.0), TextColor(dim), LineHeight::Px(12.0))).id();
        let fs = c.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(6), row_gap: px(6), ..default() }).id();
        for fi in run.files.iter().take(4) {
            let name = std::path::Path::new(&fi.path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let ch = c.spawn((Node { padding: UiRect { left: px(7), right: px(7), top: px(3), bottom: px(2) }, border: UiRect::all(px(1)), ..default() }, BorderColor::all(with_alpha(line, 0.34)))).id();
            let tx = c.spawn((Text::new(name), font(&f.mono, 11.0, 0.0), TextColor(line), LineHeight::Px(13.0))).id();
            c.entity(ch).add_child(tx);
            c.entity(fs).add_child(ch);
        }
        c.entity(b).add_children(&[fl, fs]);
    }
    let keys = c.spawn((Node { padding: UiRect { top: px(10), ..default() }, border: UiRect { top: px(1), ..default() }, ..default() }, BorderColor::all(with_alpha(line, 0.13)))).id();
    let kt = c.spawn((Text::new("P INTERRUPT   ·   T OUTPUT   ·   W OR ESC HOME"), font(&f.ui700, 9.5, 1.9), TextColor(dim), LineHeight::Px(10.0))).id();
    c.entity(keys).add_child(kt);
    c.entity(b).add_child(keys);
}

fn build_out(c: &mut Commands, root: Entity, f: &UiFonts, t: &TaskView, run: &Run, scroll: usize) -> usize {
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    let b = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, padding: UiRect { left: px(18), right: px(18), top: px(14), bottom: px(14) }, ..default() }).id();
    c.entity(root).add_child(b);
    let live = matches!(t.state, TaskState::Working | TaskState::NeedsYou);
    let hdr = c
        .spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(9), padding: UiRect { bottom: px(8), ..default() }, margin: UiRect { bottom: px(6), ..default() }, border: UiRect { bottom: px(1), ..default() }, ..default() }, BorderColor::all(with_alpha(line, 0.72))))
        .id();
    let sq = c.spawn((Node { width: px(6), height: px(6), ..default() }, BackgroundColor(if live { role_color(Role::Fill) } else { role_color(Role::Dim) }))).id();
    let tt = c.spawn((Text::new("OUTPUT"), font(&f.ui600, 12.0, 3.84), TextColor(hot), LineHeight::Px(14.4))).id();
    let st = match t.state {
        TaskState::Done => "EXITED 0",
        TaskState::Failed => "FAILED",
        TaskState::Stopped => "STOPPED",
        _ => "LIVE · AGENT",
    };
    let r = c.spawn((Text::new(st), font(&f.ui500, 12.0, 2.16), TextColor(role_color(Role::Dim)), LineHeight::Px(14.4), Node { margin: UiRect { left: auto(), ..default() }, ..default() })).id();
    c.entity(hdr).add_children(&[sq, tt, r]);
    c.entity(b).add_child(hdr);
    // the last lines: each step as a command, then its output (errors in red), newest at the bottom
    let mut lines: Vec<(String, Role)> = Vec::new();
    for s in run.steps.iter().rev().take(6).collect::<Vec<_>>().into_iter().rev() {
        lines.push((format!("$ {}", clip(&s.title, 70)), Role::Hot));
        let out = if !s.output.is_empty() { s.output.clone() } else { s.detail.clone() };
        for l in wrap(&out, 74).into_iter().take(3) {
            lines.push((l, if s.ok == Some(false) { Role::Bad } else { Role::Line }));
        }
    }
    // the whole latest reply, wrapped; continuation lines indent under the first
    if let Some(say) = run.says.last() {
        for (i, l) in wrap(say, 70).into_iter().enumerate() {
            lines.push((if i == 0 { format!("» {l}") } else { format!("  {l}") }, Role::Fill));
        }
    }
    // the window of 12 lines, `scroll` lines above the bottom (never past the top)
    let end = lines.len().saturating_sub(scroll.min(lines.len().saturating_sub(12)));
    let start = end.saturating_sub(12);
    let shown = &lines[start..end];
    for (k, (text, role)) in shown.iter().enumerate() {
        let fade = if (k == 0 && start > 0) || (k + 1 == shown.len() && end < lines.len()) { 0.45 } else { 1.0 };
        let e = c
            .spawn((Text::new(text.clone()), font(&f.mono, 12.0, 0.0), TextColor(with_alpha(role_color(*role), fade)), LineHeight::Px(19.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { overflow: Overflow::clip(), ..default() }))
            .id();
        c.entity(b).add_child(e);
    }
    lines.len().saturating_sub(12)
}

fn build_edit(c: &mut Commands, root: Entity, f: &UiFonts, run: &Run) {
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    let b = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, row_gap: px(4), padding: UiRect { left: px(18), right: px(18), top: px(14), bottom: px(14) }, ..default() }).id();
    c.entity(root).add_child(b);
    let last_edit: Option<&RunStep> = run.steps.iter().rev().find(|s| s.kind == StepKind::Edit);
    let file = last_edit.map(|s| clip(&s.title, 40)).unwrap_or_else(|| "no edits yet".into());
    let hdr = c
        .spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(9), padding: UiRect { bottom: px(8), ..default() }, margin: UiRect { bottom: px(6), ..default() }, border: UiRect { bottom: px(1), ..default() }, ..default() }, BorderColor::all(with_alpha(line, 0.72))))
        .id();
    let sq = c.spawn((Node { width: px(6), height: px(6), ..default() }, BackgroundColor(role_color(Role::Fill)))).id();
    let tt = c.spawn((Text::new("EDIT"), font(&f.ui600, 12.0, 3.84), TextColor(hot), LineHeight::Px(14.4))).id();
    let r = c.spawn((Text::new(file), font(&f.mono, 11.0, 0.0), TextColor(role_color(Role::Dim)), LineHeight::Px(14.4), Node { margin: UiRect { left: auto(), ..default() }, ..default() })).id();
    c.entity(hdr).add_children(&[sq, tt, r]);
    c.entity(b).add_child(hdr);
    let (mut add, mut del) = (0u64, 0u64);
    for fi in run.files.iter().take(5) {
        add += fi.added;
        del += fi.removed;
        let row = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(10), align_items: AlignItems::Center, height: px(21), ..default() }).id();
        let p = c.spawn((Text::new(clip(&fi.path, 38)), font(&f.mono, 11.0, 0.0), TextColor(line), LineHeight::Px(13.0), Node { flex_grow: 1.0, overflow: Overflow::clip(), ..default() })).id();
        let a = c.spawn((Text::new(format!("+{}", fi.added)), font(&f.mono, 11.0, 0.0), TextColor(role_color(Role::Ok)), LineHeight::Px(13.0))).id();
        let d = c.spawn((Text::new(format!("−{}", fi.removed)), font(&f.mono, 11.0, 0.0), TextColor(role_color(Role::Bad)), LineHeight::Px(13.0))).id();
        c.entity(row).add_children(&[p, a, d]);
        c.entity(b).add_child(row);
    }
    if run.files.is_empty() {
        let e = c.spawn((Text::new("The agent hasn't changed any files yet."), font(&f.ui500, 13.0, 0.5), TextColor(line), LineHeight::Px(16.0))).id();
        c.entity(b).add_child(e);
    }
    let sp = c.spawn(Node { flex_grow: 1.0, ..default() }).id();
    let foot = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(10), ..default() }).id();
    let a = c.spawn((Text::new(format!("+{add}")), font(&f.mono, 11.0, 0.0), TextColor(role_color(Role::Ok)), LineHeight::Px(13.0))).id();
    let d = c.spawn((Text::new(format!("−{del}")), font(&f.mono, 11.0, 0.0), TextColor(role_color(Role::Bad)), LineHeight::Px(13.0))).id();
    c.entity(foot).add_children(&[a, d]);
    c.entity(b).add_children(&[sp, foot]);
}

fn build_chip(c: &mut Commands, root: Entity, f: &UiFonts, s: &RunStep, index: usize, now: bool, t: &TaskView) {
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    let b = c
        .spawn((
            Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, justify_content: JustifyContent::Center, row_gap: px(3), padding: UiRect { left: px(12), right: px(10), ..default() }, border: UiRect { left: px(if now { 2.0 } else { 1.0 }), ..default() }, ..default() },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            BorderColor::all(if now { role_color(Role::Fill) } else { with_alpha(line, 0.34) }),
        ))
        .id();
    c.entity(root).add_child(b);
    let top = c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(8), align_items: AlignItems::Center, ..default() }).id();
    let tag = c.spawn((Node { padding: UiRect { left: px(4), right: px(4), top: px(2), bottom: px(1) }, ..default() }, BackgroundColor(if now { role_color(Role::Fill) } else { Color::NONE }))).id();
    let tn = c.spawn((Text::new(format!("{index:02}")), font(&f.ui700, 10.5, 1.3), TextColor(if now { Color::BLACK } else { line }), LineHeight::Px(11.0))).id();
    c.entity(tag).add_child(tn);
    let kd = c.spawn((Text::new(kind_word(s.kind)), font(&f.ui700, if now { 12.0 } else { 11.0 }, 2.9), TextColor(hot), LineHeight::Px(13.0))).id();
    c.entity(top).add_children(&[tag, kd]);
    let value = match s.ok {
        Some(false) => format!("failed · {}", clip(&s.title, 26)),
        _ if now && t.state == TaskState::NeedsYou => "waiting on you".to_string(),
        _ => clip(&s.title, 32),
    };
    let col = if s.ok == Some(false) { role_color(Role::Bad) } else { line };
    let v = c.spawn((Text::new(value), font(&f.ui500, if now { 14.0 } else { 12.5 }, 0.3), TextColor(col), LineHeight::Px(16.0), TextLayout { linebreak: LineBreak::NoWrap, ..default() }, Node { overflow: Overflow::clip(), ..default() })).id();
    c.entity(b).add_children(&[top, v]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tv(state: &str, status: &str, files: usize) -> TaskView {
        serde_json::from_value(serde_json::json!({
            "id": "t1", "title": "Fix it", "prompt": "", "state": state, "status": status, "repo": "/r", "in_place": false,
            "budget_usd": 0.0, "cost_usd": 0.5, "started_at": "2026-10-19T00:00:00Z", "updated_at": "2026-10-19T00:03:05Z",
            "files": (0..files).map(|i| serde_json::json!({"path": format!("f{i}"), "added": 1, "removed": 0})).collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    #[test]
    fn the_head_tip_follows_the_state() {
        let run = Run::default();
        let now = parse_rfc3339("2026-10-19T00:00:45Z").unwrap();
        assert_eq!(head_tip(&tv("working", "", 0), &run, now), ("FIX IT".into(), "Step 0 ·  · 45s".into()));
        assert_eq!(head_tip(&tv("needs_you", "", 0), &run, now).1, "Needs you · waiting on your answer");
        assert_eq!(head_tip(&tv("done", "Done", 0), &run, now).1, "Answered · 0 steps · 3m · ≈$0.50");
        assert_eq!(head_tip(&tv("done", "Done", 2), &run, now).1, "Done · 2 files changed · click to review");
        assert_eq!(head_tip(&tv("failed", "Failed: boom\nmore", 0), &run, now).1, "Failed · boom");
        assert_eq!(head_tip(&tv("stopped", "", 0), &run, now).1, "Stopped");
        assert_eq!(head_tip(&tv("queued", "", 0), &run, now).1, "Queued");
    }

    #[test]
    fn rfc3339_reads_utc_and_offsets() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-10-17T00:55:33.309Z"), Some(1_792_198_533));
        assert_eq!(parse_rfc3339("2026-10-17T01:55:33+01:00"), Some(1_792_198_533));
    }

    #[test]
    fn wrap_breaks_at_spaces_and_splits_long_words() {
        assert_eq!(wrap("aa bb cc dd", 5), ["aa bb", "cc dd"]);
        assert_eq!(wrap("one\n\n  \ntwo three", 20), ["one", "two three"]);
        assert_eq!(wrap("abcdefghij kl", 4), ["abcd", "efgh", "ij", "kl"]);
        assert!(wrap("", 10).is_empty());
    }

    #[test]
    fn the_track_is_even_and_starts_at_the_core() {
        let p = sample_curve();
        assert_eq!(p.len(), SAMPLES);
        assert!(p[0].length() < 1.0);
        let d0 = (p[1] - p[0]).length();
        let d1 = (p[200] - p[199]).length();
        assert!((d0 - d1).abs() < d0 * 0.2);
    }
}
