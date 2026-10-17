//! What every app shares: the UI helpers its panes are drawn with (a pane header, a body, text).
use crate::fonts::UiFonts;
use crate::panel::*;
use bevy::prelude::*;
use bevy::text::LineHeight;

impl<'a> Ui<'a> {
    pub fn new(f: &'a UiFonts) -> Self {
        Ui { f, line: crate::util::role_color(crate::util::Role::Line), hot: crate::util::role_color(crate::util::Role::Hot), dim: crate::util::role_color(crate::util::Role::Dim) }
    }
}

pub struct Ui<'a> {
    pub f: &'a UiFonts,
    pub line: Color,
    pub hot: Color,
    pub dim: Color,
}

impl Ui<'_> {
    pub fn text(&self, c: &mut Commands, s: impl Into<String>, h: &Handle<Font>, size: f32, sp: f32, col: Color) -> Entity {
        c.spawn((Text::new(s.into()), font(h, size, sp), TextColor(col), LineHeight::Px(size * 1.25), TextLayout { linebreak: LineBreak::NoWrap, ..default() })).id()
    }
    pub fn row(&self, c: &mut Commands, gap: f32) -> Entity {
        c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(gap), ..default() }).id()
    }
    /// "■ TITLE ............ right"
    pub fn header(&self, c: &mut Commands, title: &str, right: &str, right_col: Color) -> Entity {
        let h = c
            .spawn((
                Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), padding: UiRect { left: px(14), right: px(14), top: px(11), bottom: px(8) }, margin: UiRect { left: px(4), right: px(4), ..default() }, border: UiRect { bottom: px(1), ..default() }, flex_shrink: 0.0, ..default() },
                BorderColor::all(with_alpha(self.line, 0.34)),
            ))
            .id();
        let sq = c.spawn((Node { width: px(6), height: px(6), ..default() }, BackgroundColor(self.hot))).id();
        let t = self.text(c, title.to_uppercase(), &self.f.ui600, 11.0, 3.3, self.hot);
        let r = c.spawn((Text::new(right.to_string()), font(&self.f.ui500, 11.0, 1.4), TextColor(right_col), LineHeight::Px(13.0), Node { margin: UiRect { left: auto(), ..default() }, ..default() })).id();
        c.entity(h).add_children(&[sq, t, r]);
        h
    }
    pub fn body(&self, c: &mut Commands, root: Entity) -> Entity {
        let b = c.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, overflow: Overflow::clip(), ..default() }).id();
        c.entity(root).add_child(b);
        b
    }
}

pub fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    } else {
        s.to_string()
    }
}

