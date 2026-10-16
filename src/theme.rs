//! Colours, fonts and the small set of painted widgets that give Sphere OS
//! its HUD look: thin borders with bright corner brackets, uppercase labels,
//! chips, progress bars and sparklines.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Mesh, Painter,
    Pos2, Rect, Response, RichText, Sense, Shadow, Stroke, StrokeKind, TextStyle, Ui,
};

pub const VOID: Color32 = Color32::from_rgb(3, 7, 13);
pub const BAR: Color32 = Color32::from_rgb(3, 10, 18);
pub const PANEL: Color32 = Color32::from_rgb(6, 17, 28);
pub const PANEL_HI: Color32 = Color32::from_rgb(10, 28, 44);
pub const LINE: Color32 = Color32::from_rgb(22, 52, 70);
pub const LINE_STRONG: Color32 = Color32::from_rgb(58, 122, 152);
pub const CYAN: Color32 = Color32::from_rgb(95, 212, 255);
pub const ICE: Color32 = Color32::from_rgb(220, 244, 255);
pub const TEXT: Color32 = Color32::from_rgb(185, 214, 232);
pub const DIM: Color32 = Color32::from_rgb(108, 144, 168);
pub const AMBER: Color32 = Color32::from_rgb(255, 181, 71);
pub const GREEN: Color32 = Color32::from_rgb(93, 255, 176);
pub const RED: Color32 = Color32::from_rgb(255, 107, 122);
pub const DEEP: Color32 = Color32::from_rgb(28, 100, 220);

/// `color` at `alpha` (0..=1), for glows and fills over the dark background.
///
/// Premultiplies in gamma space, as CSS does, so the values match the design
/// mockup. egui's `from_rgba_unmultiplied` premultiplies in linear space,
/// which makes faint colours far brighter and stacked glows blow out.
pub fn alpha(color: Color32, alpha: f32) -> Color32 {
    let a = alpha.clamp(0.0, 1.0);
    let m = |c: u8| (c as f32 * a).round() as u8;
    Color32::from_rgba_premultiplied(
        m(color.r()),
        m(color.g()),
        m(color.b()),
        (a * 255.0).round() as u8,
    )
}

pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("display".into()))
}

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}

pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

pub fn body(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// Small uppercase label in the display face, the HUD's caption style.
pub fn label(text: impl Into<String>) -> RichText {
    RichText::new(text.into().to_uppercase())
        .font(display(10.0))
        .extra_letter_spacing(1.6)
        .color(DIM)
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let faces: [(&str, &'static [u8]); 4] = [
        (
            "oxanium",
            include_bytes!("../assets/fonts/Oxanium-SemiBold.ttf"),
        ),
        (
            "plex",
            include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf"),
        ),
        (
            "plex-medium",
            include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf"),
        ),
        (
            "jetbrains",
            include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
        ),
    ];
    for (name, bytes) in faces {
        fonts
            .font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    let defaults_prop = fonts.families[&FontFamily::Proportional].clone();
    let defaults_mono = fonts.families[&FontFamily::Monospace].clone();
    let with_fallback = |first: &str, rest: &[String]| {
        let mut v = vec![first.to_string()];
        v.extend(rest.iter().cloned());
        v
    };
    fonts.families.insert(
        FontFamily::Proportional,
        with_fallback("plex", &defaults_prop),
    );
    fonts.families.insert(
        FontFamily::Monospace,
        with_fallback("jetbrains", &defaults_mono),
    );
    fonts.families.insert(
        FontFamily::Name("display".into()),
        with_fallback("oxanium", &defaults_prop),
    );
    fonts.families.insert(
        FontFamily::Name("medium".into()),
        with_fallback("plex-medium", &defaults_prop),
    );
    ctx.set_fonts(fonts);

    // Always dark, whatever the system theme says.
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, body(11.0)),
            (TextStyle::Body, body(13.0)),
            (TextStyle::Button, body(12.5)),
            (TextStyle::Heading, medium(16.0)),
            (TextStyle::Monospace, mono(12.0)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.scroll.bar_width = 6.0;
        style.interaction.selectable_labels = false;

        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = VOID;
        v.window_fill = PANEL;
        v.window_stroke = Stroke::new(1.0, LINE);
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        v.window_shadow = Shadow {
            offset: [0, 14],
            blur: 36,
            spread: 0,
            color: Color32::from_black_alpha(150),
        };
        v.popup_shadow = Shadow::NONE;
        v.extreme_bg_color = Color32::from_rgb(2, 6, 11);
        v.faint_bg_color = PANEL_HI;
        v.code_bg_color = Color32::from_rgb(2, 6, 11);
        v.hyperlink_color = CYAN;
        v.selection.bg_fill = Color32::from_rgb(18, 70, 100);
        v.selection.stroke = Stroke::new(1.0, CYAN);
        v.text_cursor.stroke = Stroke::new(1.5, CYAN);
        v.window_highlight_topmost = false;

        let w = &mut v.widgets;
        for (state, fill, stroke, fg) in [
            (&mut w.noninteractive, PANEL, LINE, TEXT),
            (&mut w.inactive, PANEL_HI, LINE, ICE),
            (
                &mut w.hovered,
                Color32::from_rgb(14, 42, 62),
                LINE_STRONG,
                ICE,
            ),
            (&mut w.active, Color32::from_rgb(20, 58, 84), CYAN, ICE),
            (&mut w.open, PANEL_HI, LINE_STRONG, ICE),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, stroke);
            state.fg_stroke = Stroke::new(1.0, fg);
            state.corner_radius = CornerRadius::ZERO;
            state.expansion = 0.0;
        }
    });
}

/// The HUD frame: flat panel, thin border.
pub fn hud_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, LINE))
        .inner_margin(egui::Margin::same(12))
}

/// Bright corner brackets on the top-left and bottom-right of `rect`.
pub fn brackets(painter: &Painter, rect: Rect) {
    let s = Stroke::new(1.0, CYAN);
    let l = 10.0;
    let (a, b) = (rect.left_top(), rect.right_bottom());
    painter.line_segment([a, a + egui::vec2(l, 0.0)], s);
    painter.line_segment([a, a + egui::vec2(0.0, l)], s);
    painter.line_segment([b, b - egui::vec2(l, 0.0)], s);
    painter.line_segment([b, b - egui::vec2(0.0, l)], s);
}

/// A framed HUD widget box with brackets.
pub fn widget_box<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let out = hud_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui)
    });
    brackets(ui.painter(), out.response.rect);
    out.inner
}

/// A small outlined uppercase status chip.
pub fn chip(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_uppercase(), display(9.0), color);
    let size = galley.size() + egui::vec2(12.0, 6.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_stroke(rect, 0.0, Stroke::new(1.0, color), StrokeKind::Inside);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

/// A HUD button: uppercase display text, optional highlighted fill.
pub fn button(ui: &mut Ui, text: &str, primary: bool) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_uppercase(), display(10.0), ICE);
    let size = galley.size() + egui::vec2(20.0, 14.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hovered = resp.hovered();
    let fill = match (primary, hovered) {
        (true, true) => alpha(CYAN, 0.32),
        (true, false) => alpha(CYAN, 0.18),
        (false, true) => alpha(CYAN, 0.12),
        (false, false) => Color32::TRANSPARENT,
    };
    let stroke = if primary { CYAN } else { LINE_STRONG };
    ui.painter().rect(
        rect,
        0.0,
        fill,
        Stroke::new(1.0, stroke),
        StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, ICE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A thin progress bar filling the available width.
pub fn progress(ui: &mut Ui, fraction: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 3.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, alpha(CYAN, 0.12));
    let mut fill = rect;
    fill.set_width(rect.width() * fraction.clamp(0.0, 1.0));
    ui.painter().rect_filled(fill, 0.0, color);
}

/// A sparkline with a soft area fill and an emphasised last point.
pub fn sparkline(painter: &Painter, rect: Rect, data: &[f32], grid: bool) {
    if grid {
        for i in 1..4 {
            let y = rect.top() + rect.height() * i as f32 / 4.0;
            painter.line_segment(
                [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                Stroke::new(1.0, alpha(CYAN, 0.07)),
            );
        }
        painter.rect_stroke(rect, 0.0, Stroke::new(1.0, LINE), StrokeKind::Inside);
    }
    if data.len() < 2 {
        return;
    }
    let cap = 60.0;
    let offset = cap - data.len() as f32;
    let x = |i: usize| rect.left() + (i as f32 + offset) / (cap - 1.0) * rect.width();
    let y = |v: f32| rect.bottom() - 2.0 - (v / 100.0).clamp(0.0, 1.0) * (rect.height() - 4.0);
    let points: Vec<Pos2> = data
        .iter()
        .enumerate()
        .map(|(i, &v)| Pos2::new(x(i), y(v)))
        .collect();

    let mut mesh = Mesh::default();
    for w in points.windows(2) {
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(w[0], alpha(CYAN, 0.22));
        mesh.colored_vertex(w[1], alpha(CYAN, 0.22));
        mesh.colored_vertex(Pos2::new(w[1].x, rect.bottom()), Color32::TRANSPARENT);
        mesh.colored_vertex(Pos2::new(w[0].x, rect.bottom()), Color32::TRANSPARENT);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    painter.add(mesh);
    let last = *points.last().unwrap();
    painter.add(egui::Shape::line(points, Stroke::new(1.4, CYAN)));
    painter.circle_filled(last, 2.2, ICE);
}

/// A square with a diagonal two-colour gradient, used as album art.
pub fn gradient_square(painter: &Painter, rect: Rect, a: Color32, b: Color32) {
    let mut mesh = Mesh::default();
    let mid = a.lerp_to_gamma(b, 0.5);
    mesh.colored_vertex(rect.left_top(), a);
    mesh.colored_vertex(rect.right_top(), mid);
    mesh.colored_vertex(rect.right_bottom(), b);
    mesh.colored_vertex(rect.left_bottom(), mid);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
}

/// Album-art colours derived from a string, so each track keeps its look.
pub fn art_colors(seed: &str) -> (Color32, Color32) {
    let mut h: u32 = 0;
    for c in seed.chars() {
        h = h.wrapping_mul(31).wrapping_add(c as u32) % 360;
    }
    let hue = |deg: u32, light: f32| {
        let c = egui::ecolor::Hsva::new((deg % 360) as f32 / 360.0, 0.75, light, 1.0);
        Color32::from(c)
    };
    (hue(h, 0.75), hue(h + 70, 0.3))
}
