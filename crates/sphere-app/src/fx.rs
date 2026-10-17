//! One-shot effects from the prototype: the shockwave ring off the core (mode and theme changes, reset), and the
//! sweep, a soft band of light that runs down the screen (theme change). Also flags for other systems.
use crate::core::{CoreRoot, CORE_Y};
use crate::geo::{ring, Geo};
use crate::glow::{GlowMaterial, Kind};
use crate::util::*;
use bevy::prelude::*;

#[derive(Resource)]
pub struct Fx {
    shock_t: f32,
    shock_role: Role,
    sweep_t: f32,
    /// Reset: unfold every single-folded ring
    pub unfold_all: bool,
    /// Reset: put dragged panels back home
    pub reset_panels: bool,
}

impl Default for Fx {
    fn default() -> Self {
        Fx { shock_t: 9.0, shock_role: Role::Hot, sweep_t: 9.0, unfold_all: false, reset_panels: false }
    }
}

impl Fx {
    pub fn shock(&mut self, r: Role) {
        self.shock_t = 0.0;
        self.shock_role = r;
    }
    pub fn sweep(&mut self) {
        self.sweep_t = 0.0;
    }
}

#[derive(Component)]
pub struct Shock;
#[derive(Component)]
pub struct Sweep;

pub fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut mats: ResMut<Assets<GlowMaterial>>, root: Res<crate::hud::OverlayRoot>, core: Query<Entity, With<CoreRoot>>) {
    let mut g = Geo::new();
    ring(&mut g, 100.0, 1.6);
    let mut m = GlowMaterial::new(Kind::Ring, crate::theme::now_lin(Role::Hot), 0.0);
    m.u.a.y = 0.0;
    let e = commands.spawn((Mesh3d(meshes.add(g.into_mesh())), MeshMaterial3d(mats.add(m)), Transform::from_xyz(0.0, 0.0, 120.0), Visibility::Hidden, Shock)).id();
    if let Ok(c) = core.single() {
        commands.entity(c).add_child(e);
    } else {
        commands.entity(e).insert(Transform::from_xyz(0.0, CORE_Y, 120.0));
    }
    // the sweep: transparent, g3 at 44%, g2 at 50%, g3 at 56%, transparent
    let line = role_color(Role::Line);
    let a = |x: f32| crate::panel::with_alpha(line, x);
    let band = commands
        .spawn((
            Sweep,
            Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: px(-240), height: px(220), ..default() },
            BackgroundGradient(vec![Gradient::Linear(LinearGradient::to_bottom(vec![
                ColorStop::percent(Color::NONE, 0.0),
                ColorStop::percent(a(0.13), 44.0),
                ColorStop::percent(a(0.34), 50.0),
                ColorStop::percent(a(0.13), 56.0),
                ColorStop::percent(Color::NONE, 100.0),
            ]))]),
            Visibility::Hidden,
        ))
        .id();
    commands.entity(root.0).add_child(band);
}

pub fn update(
    time: Res<Time<Real>>,
    mut fx: ResMut<Fx>,
    mut mats: ResMut<Assets<GlowMaterial>>,
    mut shock: Query<(&mut Transform, &mut Visibility, &MeshMaterial3d<GlowMaterial>), With<Shock>>,
    mut sweep: Query<(&mut Node, &mut Visibility, &mut UiTransform), (With<Sweep>, Without<Shock>)>,
    win: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    let dt = time.delta_secs().min(1.0 / 30.0);
    if fx.shock_t < 0.95 {
        fx.shock_t += dt;
        let t = (fx.shock_t / 0.95).min(1.0);
        for (mut tr, mut vis, h) in &mut shock {
            *vis = if t < 1.0 { Visibility::Inherited } else { Visibility::Hidden };
            tr.scale = Vec3::splat(lerp(1.1, 4.4, out_cubic(t)));
            if let Some(mut m) = mats.get_mut(&h.0) {
                m.u.color = crate::theme::now_lin(fx.shock_role);
                m.u.a.x = (1.0 - t) * 0.9;
            }
        }
    }
    if fx.sweep_t < 1.1 {
        fx.sweep_t += dt;
        let t = (fx.sweep_t / 1.1).min(1.0);
        // cubic-bezier(.3,.7,.3,1) is close to an out-cubic
        let e = out_cubic(t);
        let h = win.single().map(|w| w.height()).unwrap_or(1000.0);
        for (mut n, mut vis, mut ut) in &mut sweep {
            *vis = if t < 1.0 { Visibility::Inherited } else { Visibility::Hidden };
            n.top = px(lerp(-240.0, h, e));
            // opacity .85 -> 0 via scale of the band's alpha is not available on a node; fade by shrinking
            ut.scale = Vec2::new(1.0, lerp(1.0, 0.6, t));
        }
    }
}
