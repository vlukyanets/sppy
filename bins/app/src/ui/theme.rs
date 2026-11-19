//! The app's look: one calm palette with a single accent colour, larger type, more air,
//! and a few small building blocks (section titles, pills, rows, segmented switches).

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Theme, vec2};

pub const ACCENT: Color32 = Color32::from_rgb(122, 140, 255);
pub const BG: Color32 = Color32::from_rgb(22, 24, 29);
pub const SIDE: Color32 = Color32::from_rgb(18, 20, 24);
pub const SURFACE: Color32 = Color32::from_rgb(30, 33, 40);
pub const RAISED: Color32 = Color32::from_rgb(38, 42, 51);
pub const SUNKEN: Color32 = Color32::from_rgb(13, 15, 18);
pub const TEXT: Color32 = Color32::from_rgb(220, 223, 230);
pub const MUTED: Color32 = Color32::from_rgb(138, 144, 156);

pub const RADIUS: u8 = 6;

/// Text sizes and spacing for both themes; colours for the dark one (the light one keeps
/// egui's colours with our accent).
pub fn apply(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(19.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.5, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.5, FontFamily::Monospace)),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = vec2(8.0, 6.0);
        s.button_padding = vec2(10.0, 4.0);
        s.interact_size.y = 26.0;
        s.window_margin = Margin::same(14);
        s.menu_margin = Margin::same(8);
        s.indent = 16.0;
        s.slider_rail_height = 6.0;
        let v = &mut style.visuals;
        let r = CornerRadius::same(RADIUS);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = r;
            w.expansion = 0.0;
        }
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);
        v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.hyperlink_color = ACCENT;
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Circle;
        v.indent_has_left_vline = false;
    });
    ctx.style_mut_of(Theme::Dark, |style| {
        let v = &mut style.visuals;
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.window_stroke = Stroke::new(1.0, RAISED);
        v.extreme_bg_color = SUNKEN;
        v.faint_bg_color = SURFACE;
        v.code_bg_color = SUNKEN;
        v.override_text_color = None;
        v.widgets.noninteractive.bg_fill = BG;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, RAISED);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.inactive.bg_fill = RAISED;
        v.widgets.inactive.weak_bg_fill = RAISED;
        v.widgets.inactive.bg_stroke = Stroke::NONE;
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.hovered.bg_fill = Color32::from_rgb(48, 53, 64);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(48, 53, 64);
        v.widgets.hovered.bg_stroke = Stroke::NONE;
        v.widgets.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        v.widgets.active.bg_fill = Color32::from_rgb(58, 64, 78);
        v.widgets.active.weak_bg_fill = Color32::from_rgb(58, 64, 78);
        v.widgets.active.bg_stroke = Stroke::NONE;
        v.widgets.open.weak_bg_fill = RAISED;
    });
}

/// A small uppercase title above a group of things.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title.to_uppercase()).small().strong().color(MUTED));
    ui.add_space(2.0);
}

/// A short status word on a tinted background.
pub fn pill(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.16))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::symmetric(7, 1))
        .show(ui, |ui| ui.label(egui::RichText::new(text.into()).small().color(color)))
        .response
}

/// A full-width row that lights up under the pointer; `current` marks it with the accent.
/// `add` gets whether the pointer is over the row, to show its actions only then.
pub fn row<R>(
    ui: &mut egui::Ui,
    id: egui::Id,
    current: bool,
    add: impl FnOnce(&mut egui::Ui, bool) -> R,
) -> R {
    let was_hovered = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let bg = ui.painter().add(egui::Shape::Noop);
    let inner = egui::Frame::new().inner_margin(Margin::symmetric(10, 5)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| add(ui, was_hovered)).inner
    });
    let rect = inner.response.rect;
    let hovered = ui.rect_contains_pointer(rect);
    if hovered != was_hovered {
        ui.ctx().data_mut(|d| d.insert_temp(id, hovered));
        ui.ctx().request_repaint();
    }
    let dark = ui.visuals().dark_mode;
    let fill = if current {
        ACCENT.gamma_multiply(if dark { 0.14 } else { 0.18 })
    } else if hovered {
        ui.visuals().widgets.inactive.weak_bg_fill.gamma_multiply(0.6)
    } else {
        Color32::TRANSPARENT
    };
    let mut shapes = vec![egui::Shape::rect_filled(rect, CornerRadius::same(RADIUS), fill)];
    if current {
        let bar =
            egui::Rect::from_min_size(rect.min + vec2(0.0, 4.0), vec2(3.0, rect.height() - 8.0));
        shapes.push(egui::Shape::rect_filled(bar, CornerRadius::same(2), ACCENT));
    }
    ui.painter().set(bg, egui::Shape::Vec(shapes));
    inner.inner
}

/// A frameless icon button for row actions.
pub fn icon(ui: &mut egui::Ui, glyph: &str, hint: &str) -> bool {
    ui.add(egui::Button::new(egui::RichText::new(glyph).color(MUTED)).frame(false))
        .on_hover_text(hint)
        .clicked()
}

/// Two or three exclusive choices drawn as one control.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    value: &mut T,
    options: &[(T, &str)],
) -> egui::Response {
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(CornerRadius::same(RADIUS + 2))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // The frame adds 2 px around: keep the whole control as tall as a button.
            ui.spacing_mut().interact_size.y -= 4.0;
            ui.spacing_mut().button_padding.y -= 2.0;
            ui.horizontal(|ui| {
                for (v, label) in options {
                    let on = *value == *v;
                    let text = egui::RichText::new(*label);
                    let button =
                        egui::Button::new(if on { text.color(Color32::WHITE) } else { text })
                            .fill(if on {
                                ACCENT.gamma_multiply(0.75)
                            } else {
                                Color32::TRANSPARENT
                            })
                            .corner_radius(CornerRadius::same(RADIUS));
                    if ui.add(button).clicked() {
                        *value = *v;
                    }
                }
            });
        })
        .response
}

/// The main call to action (Play, Connect).
pub fn primary(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text.into()).color(Color32::WHITE))
        .fill(ACCENT.gamma_multiply(0.85))
}

/// Background of the side and top panels: a shade darker than the content.
pub fn panel_frame(ui: &egui::Ui, margin: Margin) -> egui::Frame {
    let fill = if ui.visuals().dark_mode { SIDE } else { ui.visuals().faint_bg_color };
    egui::Frame::new().fill(fill).inner_margin(margin)
}

/// The content area.
pub fn content_frame(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(Margin::symmetric(18, 14))
}

/// A note that needs attention, tinted with `color`.
pub fn callout<R>(ui: &mut egui::Ui, color: Color32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let r = egui::Frame::new()
        .fill(color.gamma_multiply(0.10))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner;
    ui.add_space(8.0);
    r
}

/// A titled group of settings.
pub fn card<R>(
    ui: &mut egui::Ui,
    title: &str,
    about: Option<&str>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let r = egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(title).size(16.0).strong());
            if let Some(about) = about {
                muted(ui, about);
            }
            ui.add_space(6.0);
            add(ui)
        })
        .inner;
    ui.add_space(10.0);
    r
}

/// Secondary text that wraps.
pub fn muted(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).color(MUTED)).wrap());
}

/// Transport icons, drawn rather than taken from fonts so that they line up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Previous,
    Next,
    Play,
    Pause,
    Restart,
}

/// A round transport button of `size`; `main` ones are filled with the accent.
pub fn transport(ui: &mut egui::Ui, icon: Transport, size: f32, main: bool) -> egui::Response {
    use egui::{Pos2, Shape, pos2};
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let enabled = ui.is_enabled();
    let hovered = response.hovered() && enabled;
    let painter = ui.painter();
    let c = rect.center();
    let r = size / 2.0;
    if main {
        let fill = if hovered { ACCENT } else { ACCENT.gamma_multiply(0.85) };
        painter.circle_filled(c, r, if enabled { fill } else { fill.gamma_multiply(0.4) });
    } else if hovered {
        painter.circle_filled(c, r, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let fg = if main {
        Color32::WHITE
    } else if !enabled {
        MUTED.gamma_multiply(0.5)
    } else if hovered {
        Color32::WHITE
    } else {
        ui.visuals().text_color()
    };
    let s = size * if main { 0.36 } else { 0.30 };
    let triangle = |tip_right: bool, x0: f32, x1: f32| {
        let (back, tip) = if tip_right { (x0, x1) } else { (x1, x0) };
        Shape::convex_polygon(
            vec![pos2(back, c.y - s * 0.55), pos2(tip, c.y), pos2(back, c.y + s * 0.55)],
            fg,
            Stroke::NONE,
        )
    };
    let bar = |x: f32, w: f32| {
        Shape::rect_filled(
            egui::Rect::from_min_max(pos2(x, c.y - s * 0.55), pos2(x + w, c.y + s * 0.55)),
            CornerRadius::same(1),
            fg,
        )
    };
    let shapes: Vec<Shape> = match icon {
        Transport::Play => vec![triangle(true, c.x - s * 0.38, c.x + s * 0.55)],
        Transport::Pause => vec![bar(c.x - s * 0.42, s * 0.28), bar(c.x + s * 0.14, s * 0.28)],
        Transport::Next => {
            vec![triangle(true, c.x - s * 0.5, c.x + s * 0.3), bar(c.x + s * 0.32, s * 0.16)]
        }
        Transport::Previous => {
            vec![bar(c.x - s * 0.48, s * 0.16), triangle(false, c.x - s * 0.3, c.x + s * 0.5)]
        }
        Transport::Restart => {
            let radius = s * 0.55;
            let points: Vec<Pos2> = (0..=24)
                .map(|i| {
                    let a = -0.6 + (i as f32 / 24.0) * 5.0;
                    pos2(c.x + radius * a.cos(), c.y + radius * a.sin())
                })
                .collect();
            let end = points[0];
            let head = vec![
                pos2(end.x - s * 0.05, end.y - s * 0.38),
                pos2(end.x + s * 0.30, end.y + s * 0.06),
                pos2(end.x - s * 0.20, end.y + s * 0.10),
            ];
            vec![
                Shape::line(points, Stroke::new(size * 0.07, fg)),
                Shape::convex_polygon(head, fg, Stroke::NONE),
            ]
        }
    };
    painter.extend(shapes);
    response
}
