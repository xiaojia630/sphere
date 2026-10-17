//! Depth's layer callouts (prototype v2): each ring gets a leader from a point on its rim (52°, .97 of its outer
//! radius) out to a label in a column right of the stack, `L1`..`L6` with the layer's name and a live line.
use crate::core::{CoreCtl, Decks, Mode};
use crate::fonts::UiFonts;
use crate::panel::{font, with_alpha};
use crate::util::*;
use crate::CamCore;
use bevy::prelude::*;
use bevy::text::LineHeight;

const NAMES: [&str; 6] = ["Timeline", "System load", "Agents", "Signal", "Voice", "Core"];

#[derive(Component)]
pub struct Callout(pub usize);
#[derive(Component)]
pub struct CalloutSub(pub usize);
#[derive(Component)]
pub struct Leader(pub usize, pub u8);
#[derive(Component)]
pub struct Datum(pub usize);

pub fn setup(mut commands: Commands, f: Res<UiFonts>, root: Res<crate::hud::OverlayRoot>) {
    let c = &mut commands;
    let line = role_color(Role::Line);
    let hot = role_color(Role::Hot);
    for (i, layer) in NAMES.iter().enumerate() {
        let lab = c
            .spawn((Callout(i), Node { position_type: PositionType::Absolute, flex_direction: FlexDirection::Column, row_gap: px(4), ..default() }, Visibility::Hidden))
            .id();
        let top = c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), ..default() }).id();
        let tag = c.spawn((Node { padding: UiRect { left: px(5), right: px(5), top: px(2), bottom: px(1) }, ..default() }, BackgroundColor(role_color(Role::Fill)))).id();
        let tt = c.spawn((Text::new(format!("L{}", i + 1)), font(&f.ui700, 10.5, 1.26), TextColor(Color::BLACK), LineHeight::Px(13.0))).id();
        c.entity(tag).add_child(tt);
        let name = c.spawn((Text::new(layer.to_uppercase()), font(&f.ui700, 10.5, 2.73), TextColor(hot), LineHeight::Px(13.0))).id();
        c.entity(top).add_children(&[tag, name]);
        let sub = c.spawn((CalloutSub(i), Text::new(""), font(&f.ui500, 11.0, 1.32), TextColor(line), LineHeight::Px(13.0))).id();
        c.entity(lab).add_children(&[top, sub]);
        let mut parts = vec![lab];
        for k in 0..2u8 {
            parts.push(c.spawn((Leader(i, k), Node { position_type: PositionType::Absolute, height: px(1), ..default() }, BackgroundColor(with_alpha(line, 0.8)), UiTransform::default(), Visibility::Hidden)).id());
        }
        parts.push(c.spawn((Datum(i), Node { position_type: PositionType::Absolute, width: px(5.2), height: px(5.2), border_radius: BorderRadius::MAX, ..default() }, BackgroundColor(role_color(Role::Fill)), Visibility::Hidden)).id());
        c.entity(root.0).add_children(&parts);
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn update(
    ctl: Res<CoreCtl>,
    decks: Res<Decks>,
    agents: Res<crate::core::Agents>,
    stats: Res<crate::sysstats::SysStats>,
    moment: Res<crate::moment::Moment>,
    cams: Query<(&Camera, &GlobalTransform), With<CamCore>>,
    gts: Query<&GlobalTransform>,
    win: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut sets: ParamSet<(
        Query<(&Callout, &mut Node, &mut Visibility)>,
        Query<(&Leader, &mut Node, &mut UiTransform, &mut Visibility, &mut BackgroundColor)>,
        Query<(&Datum, &mut Node, &mut Visibility)>,
        Query<(&CalloutSub, &mut Text)>,
    )>,
) {
    let amt = if ctl.mode == Mode::Depth && !moment.shown && ctl.orb < 0.05 && ctl.work < 0.05 && ctl.app < 0.05 { ctl.depth_amt } else { 0.0 };
    let vis = if amt > 0.02 { Visibility::Inherited } else { Visibility::Hidden };
    let Ok((cam, cgt)) = cams.single() else { return };
    let w = win.single().map(|w| w.width()).unwrap_or(1600.0);
    // anchors on screen
    let mut anchors: Vec<(usize, Vec2)> = Vec::new();
    for (i, d) in decks.list.iter().enumerate() {
        let Ok(gt) = gts.get(d.fold) else { continue };
        let a = 52.0 * DEG;
        let r = 0.97 * d.r_max.max(40.0);
        let p = gt.transform_point(Vec3::new(r * a.sin(), r * a.cos(), 0.0));
        if let Ok(v) = cam.world_to_viewport(cgt, p) {
            anchors.push((i, v));
        }
    }
    let max_x = anchors.iter().map(|a| a.1.x).fold(0.0, f32::max);
    let col_x = (w - 230.0).min(max_x + 48.0);
    // labels in a column sorted by height, at least 38 px apart
    let mut order: Vec<(usize, Vec2)> = anchors.clone();
    order.sort_by(|a, b| a.1.y.total_cmp(&b.1.y));
    let mut ys: Vec<(usize, f32)> = Vec::new();
    let mut last = f32::MIN;
    for (i, p) in &order {
        let y = p.y.max(last + 38.0);
        ys.push((*i, y));
        last = y;
    }
    let ly = |i: usize| ys.iter().find(|y| y.0 == i).map(|y| y.1).unwrap_or(0.0);
    let an = |i: usize| anchors.iter().find(|a| a.0 == i).map(|a| a.1).unwrap_or(Vec2::ZERO);
    let line = crate::theme::now(Role::Line);
    // hidden: nothing to place (writing the nodes anyway would relayout the UI every frame)
    if vis == Visibility::Hidden {
        for (_, _, mut v) in &mut sets.p0() {
            v.set_if_neq(vis);
        }
        for (_, _, _, mut v, _) in &mut sets.p1() {
            v.set_if_neq(vis);
        }
        for (_, _, mut v) in &mut sets.p2() {
            v.set_if_neq(vis);
        }
        return;
    }
    for (c, mut n, mut v) in &mut sets.p0() {
        v.set_if_neq(vis);
        n.left = px(col_x);
        n.top = px(ly(c.0) - 8.0);
    }
    for (l, mut n, mut tr, mut v, mut bg) in &mut sets.p1() {
        v.set_if_neq(vis);
        let a = an(l.0);
        let (p0, p1) = if l.1 == 0 { (a, Vec2::new(col_x - 20.0, ly(l.0))) } else { (Vec2::new(col_x - 20.0, ly(l.0)), Vec2::new(col_x - 6.0, ly(l.0))) };
        let d = p1 - p0;
        let len = d.length();
        let mid = (p0 + p1) / 2.0;
        n.width = px(len);
        n.left = px(mid.x - len / 2.0);
        n.top = px(mid.y);
        tr.rotation = Rot2::radians(d.y.atan2(d.x));
        bg.0 = with_alpha(line, 0.8 * amt);
    }
    for (dt, mut n, mut v) in &mut sets.p2() {
        v.set_if_neq(vis);
        let a = an(dt.0);
        n.left = px(a.x - 2.6);
        n.top = px(a.y - 2.6);
    }
    let s = stats.get();
    for (cs, mut t) in &mut sets.p3() {
        let want = match cs.0 {
            0 => "Today, 24 hours".to_string(),
            1 => format!("CPU {:.0}% · memory {:.0}%", s.cpu * 100.0, s.mem * 100.0),
            2 => agents.summary(),
            3 => "sphered · services".to_string(),
            4 => "/ to type".to_string(),
            _ => "Sphere".to_string(),
        };
        if t.0 != want {
            t.0 = want;
        }
    }
}
