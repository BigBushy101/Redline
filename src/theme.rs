//! Spotify-style dark theme with a red accent, plus the custom widgets the pages share.

use eframe::egui::{
    self, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, FontId, Frame,
    Margin, Mesh, Pos2, Rect, Response, RichText, Sense, Shadow, Stroke, StrokeKind, TextStyle, Ui,
    Vec2, pos2, vec2,
};
use redline_core::platform::Platform;
use std::sync::Arc;

pub const BLACK: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
pub const BG: Color32 = Color32::from_rgb(0x12, 0x12, 0x12);
pub const CARD: Color32 = Color32::from_rgb(0x18, 0x18, 0x18);
pub const CARD_HOVER: Color32 = Color32::from_rgb(0x28, 0x28, 0x28);
pub const INPUT: Color32 = Color32::from_rgb(0x24, 0x24, 0x24);
pub const ELEVATED: Color32 = Color32::from_rgb(0x28, 0x28, 0x28);
pub const TEXT: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
pub const MUTED: Color32 = Color32::from_rgb(0xB3, 0xB3, 0xB3);
pub const SUBTLE: Color32 = Color32::from_rgb(0x6A, 0x6A, 0x6A);
pub const TRACK: Color32 = Color32::from_rgb(0x4D, 0x4D, 0x4D);

/// The highlight color set: pills, toggles, progress, the active nav bar and the page
/// header. Redline red everywhere except a platform's Saved pages, which tint it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Accent {
    pub base: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub header_tint: Color32,
}

impl Accent {
    pub const REDLINE: Accent = Accent {
        base: Color32::from_rgb(0xE8, 0x26, 0x3A),
        hover: Color32::from_rgb(0xFF, 0x3B, 0x4E),
        pressed: Color32::from_rgb(0xC4, 0x1A, 0x2D),
        header_tint: Color32::from_rgb(0x6E, 0x12, 0x1D),
    };

    /// The set for a brand color: hover lighter, pressed darker, and the header tint about
    /// 45% of it mixed into the background.
    pub fn from_base(base: Color32) -> Accent {
        Accent {
            base,
            hover: lerp_color(base, Color32::WHITE, 0.15),
            pressed: lerp_color(base, BLACK, 0.18),
            header_tint: lerp_color(BG, base, 0.45),
        }
    }

    /// Fill, hover and pressed colors for pills with white labels, darkened where the base
    /// is too light for white text to read (at least 3:1).
    pub fn pill_colors(self) -> (Color32, Color32, Color32) {
        let readable = |mut c: Color32| {
            while contrast(TEXT, c) < 3.0 {
                c = lerp_color(c, BLACK, 0.04);
            }
            c
        };
        (readable(self.base), readable(self.hover), readable(self.pressed))
    }
}

/// WCAG contrast ratio between two opaque colors.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let lum = |c: Color32| {
        let ch = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    };
    let (la, lb) = (lum(a), lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// A platform's tint for its Saved pages.
pub fn accent_for(p: Platform) -> Accent {
    let rgb = |r, g, b| Accent::from_base(Color32::from_rgb(r, g, b));
    match p {
        Platform::YouTube => rgb(0xFF, 0x00, 0x33),
        Platform::SoundCloud => rgb(0xFF, 0x55, 0x00),
        Platform::Bandcamp => rgb(0x1D, 0xA0, 0xC3),
        // Lightened from the brand purple so it reads on the dark background.
        Platform::Mixcloud => rgb(0x7B, 0x5C, 0xFF),
        Platform::Vimeo => rgb(0x1A, 0xB7, 0xEA),
        Platform::Other => rgb(0x9A, 0x9A, 0x9A),
    }
}

/// Phosphor's logo glyph for the platform where it has one; a globe otherwise. No
/// official artwork is bundled.
pub fn platform_glyph(p: Platform) -> &'static str {
    use egui_phosphor::regular as icons;
    match p {
        Platform::YouTube => icons::YOUTUBE_LOGO,
        Platform::SoundCloud => icons::SOUNDCLOUD_LOGO,
        _ => icons::GLOBE,
    }
}

thread_local! {
    static CURRENT: std::cell::Cell<Accent> = const { std::cell::Cell::new(Accent::REDLINE) };
}

/// The accent for whatever is being painted now. egui paints immediately, so a scope set
/// with [`with_accent`] covers everything drawn inside it, popups included.
pub fn accent() -> Accent {
    CURRENT.with(|c| c.get())
}

/// Paints `add` with `accent`, including the built-in egui widgets that read it from the
/// visuals (selection, links, slider fill and checkboxes).
pub fn with_accent<R>(ui: &mut Ui, accent: Accent, add: impl FnOnce(&mut Ui) -> R) -> R {
    let before = CURRENT.with(|c| c.replace(accent));
    let r = ui
        .scope(|ui| {
            apply_accent(ui.visuals_mut(), accent);
            add(ui)
        })
        .inner;
    CURRENT.with(|c| c.set(before));
    r
}

/// The visuals fields that follow the accent.
fn apply_accent(v: &mut egui::Visuals, accent: Accent) {
    v.hyperlink_color = accent.hover;
    v.selection.bg_fill = accent.base.gamma_multiply(0.55);
}

pub const BOLD: &str = "bold";

pub fn bold() -> FontFamily {
    FontFamily::Name(BOLD.into())
}

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(32.0, bold())),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        ]
        .into();

        let s = &mut style.spacing;
        s.item_spacing = vec2(8.0, 8.0);
        s.button_padding = vec2(14.0, 7.0);
        s.interact_size.y = 30.0;
        s.combo_width = 150.0;
        s.slider_width = 200.0;
        s.menu_margin = Margin::same(6);
        s.scroll = egui::style::ScrollStyle::floating();

        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = ELEVATED;
        v.window_stroke = Stroke::NONE;
        v.window_corner_radius = CornerRadius::same(8);
        v.menu_corner_radius = CornerRadius::same(6);
        v.window_shadow = Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(160),
        };
        v.popup_shadow = v.window_shadow;
        v.extreme_bg_color = INPUT;
        v.faint_bg_color = CARD;
        apply_accent(v, Accent::REDLINE);
        v.error_fg_color = Accent::REDLINE.hover;
        v.selection.stroke = Stroke::new(1.0, TEXT);
        v.slider_trailing_fill = true;
        v.override_text_color = None;

        let w = &mut v.widgets;
        w.noninteractive.fg_stroke = Stroke::new(1.0, MUTED);
        w.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_gray(0x2A));
        for (wv, fill) in [
            (&mut w.inactive, Color32::from_gray(0x2A)),
            (&mut w.hovered, Color32::from_gray(0x33)),
            (&mut w.active, Color32::from_gray(0x3E)),
            (&mut w.open, Color32::from_gray(0x33)),
        ] {
            wv.bg_fill = fill;
            wv.weak_bg_fill = fill;
            wv.bg_stroke = Stroke::NONE;
            wv.corner_radius = CornerRadius::same(6);
            wv.fg_stroke = Stroke::new(1.0, TEXT);
            wv.expansion = 0.0;
        }
        w.inactive.fg_stroke = Stroke::new(1.0, Color32::from_gray(0xE0));
        // Slider handles and checkbox ticks read the "active" accent via selection.
        w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_gray(0x55));
    });
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "Inter".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/Inter-Regular.ttf"))),
    );
    fonts.font_data.insert(
        "InterBold".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/Inter-Bold.ttf"))),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "Inter".into());

    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

    // Bold falls back to the full proportional stack (Inter, phosphor icons, emoji).
    let mut bold_stack = vec!["InterBold".to_owned()];
    bold_stack.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
    fonts.families.insert(bold(), bold_stack);

    ctx.set_fonts(fonts);
}

// ---------------------------------------------------------------------------
// Text helpers

pub fn heading(text: impl Into<String>) -> RichText {
    RichText::new(text).family(bold()).size(20.0).color(TEXT)
}

pub fn strong(text: impl Into<String>) -> RichText {
    RichText::new(text).family(bold()).color(TEXT)
}

pub fn muted(text: impl Into<String>) -> RichText {
    RichText::new(text).color(MUTED)
}

pub fn eyebrow(text: &str) -> RichText {
    RichText::new(text.to_uppercase())
        .family(bold())
        .size(11.0)
        .color(MUTED)
}

// ---------------------------------------------------------------------------
// Containers

/// A rounded content card in the main panel.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(CARD)
        .corner_radius(8)
        .inner_margin(Margin::same(20))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Page title block shown above every page's content.
pub fn page_header(ui: &mut Ui, eyebrow_text: &str, title_text: &str, subtitle: &str) {
    ui.add_space(12.0);
    ui.label(eyebrow(eyebrow_text));
    ui.label(RichText::new(title_text).family(bold()).size(56.0).color(TEXT));
    ui.label(muted(subtitle).size(15.0));
    ui.add_space(20.0);
}

/// Paints the accent-to-background gradient at the top of the rounded main panel.
pub fn paint_header_gradient(painter: &egui::Painter, panel: Rect) {
    let r = 8u8;
    let top = Rect::from_min_size(panel.min, vec2(panel.width(), r as f32 * 2.0));
    painter.rect_filled(
        top,
        CornerRadius {
            nw: r,
            ne: r,
            sw: 0,
            se: 0,
        },
        accent().header_tint,
    );
    let grad = Rect::from_min_max(
        pos2(panel.min.x, panel.min.y + r as f32),
        pos2(panel.max.x, (panel.min.y + 340.0).min(panel.max.y)),
    );
    vertical_gradient(painter, grad, accent().header_tint, BG);
}

fn vertical_gradient(painter: &egui::Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 2, 3);
    painter.add(mesh);
}

// ---------------------------------------------------------------------------
// Widgets

fn pointer(resp: Response, enabled: bool) -> Response {
    if enabled {
        resp.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Big filled red pill, the primary call to action.
pub fn pill_button(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    pill(ui, label, enabled, 46.0, 16.0)
}

pub fn small_pill_button(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    pill(ui, label, enabled, 34.0, 14.0)
}

fn pill(ui: &mut Ui, label: &str, enabled: bool, height: f32, size: f32) -> Response {
    let text_color = if enabled { TEXT } else { Color32::from_gray(0xC8) };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), FontId::new(size, bold()), text_color);
    let desired = vec2(galley.size().x + height, height);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(desired, sense);
    if ui.is_rect_visible(rect) {
        let (base, hover, pressed) = accent().pill_colors();
        let fill = if !enabled {
            base.gamma_multiply(0.35)
        } else if resp.is_pointer_button_down_on() {
            pressed
        } else if resp.hovered() {
            hover
        } else {
            base
        };
        let draw = if enabled && resp.hovered() && !resp.is_pointer_button_down_on() {
            rect.expand(1.5)
        } else {
            rect
        };
        ui.painter().rect_filled(draw, CornerRadius::same(255), fill);
        ui.painter()
            .galley(rect.center() - galley.size() / 2.0, galley, text_color);
    }
    pointer(resp, enabled)
}

/// Outlined pill, for secondary actions.
pub fn ghost_button(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    let text_color = if enabled { TEXT } else { SUBTLE };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), FontId::new(14.0, bold()), text_color);
    let desired = vec2(galley.size().x + 32.0, 34.0);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(desired, sense);
    if ui.is_rect_visible(rect) {
        let stroke_color = if enabled && resp.hovered() {
            TEXT
        } else {
            Color32::from_gray(0x7C)
        };
        let draw = if enabled && resp.hovered() {
            rect.expand(1.0)
        } else {
            rect
        };
        ui.painter().rect_stroke(
            draw,
            CornerRadius::same(255),
            Stroke::new(1.0, stroke_color),
            StrokeKind::Inside,
        );
        ui.painter()
            .galley(rect.center() - galley.size() / 2.0, galley, text_color);
    }
    pointer(resp, enabled)
}

/// Round icon-only button that lights up on hover.
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
    let size = 32.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() {
            ui.painter()
                .circle_filled(rect.center(), size / 2.0, Color32::from_gray(0x33));
        }
        let color = if resp.hovered() { TEXT } else { MUTED };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icon,
            FontId::proportional(18.0),
            color,
        );
    }
    pointer(resp, true).on_hover_text(tooltip)
}

/// Row of pill "chips"; the selected one is white like Spotify's filter chips.
pub fn chips<T: PartialEq + Clone>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for (opt, label) in options {
            let selected = *value == *opt;
            let (fg, bg) = if selected {
                (BLACK, TEXT)
            } else {
                (TEXT, Color32::from_gray(0x2A))
            };
            let galley = ui
                .painter()
                .layout_no_wrap((*label).to_owned(), FontId::new(13.5, bold()), fg);
            let (rect, resp) =
                ui.allocate_exact_size(vec2(galley.size().x + 28.0, 32.0), Sense::click());
            let bg = if !selected && resp.hovered() {
                Color32::from_gray(0x36)
            } else {
                bg
            };
            ui.painter().rect_filled(rect, CornerRadius::same(255), bg);
            ui.painter()
                .galley(rect.center() - galley.size() / 2.0, galley, fg);
            if pointer(resp, true).clicked() && !selected {
                *value = opt.clone();
                changed = true;
            }
        }
    });
    changed
}

/// iOS/Spotify-style switch with the accent color when on.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    ui.horizontal(|ui| {
        let size = vec2(40.0, 22.0);
        let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
        if resp.clicked() {
            *on = !*on;
            resp.mark_changed();
        }
        let t = ui.ctx().animate_bool_responsive(resp.id, *on);
        let off_color = if resp.hovered() {
            Color32::from_gray(0x5A)
        } else {
            Color32::from_gray(0x4D)
        };
        let fill = lerp_color(off_color, accent().base, t);
        ui.painter().rect_filled(rect, CornerRadius::same(255), fill);
        let r = rect.height() / 2.0 - 3.0;
        let x = egui::lerp(rect.left() + r + 3.0..=rect.right() - r - 3.0, t);
        ui.painter()
            .circle_filled(pos2(x, rect.center().y), r, Color32::WHITE);
        let label_resp = ui.add(egui::Label::new(RichText::new(label).color(TEXT)).sense(Sense::click()));
        if label_resp.clicked() {
            *on = !*on;
            resp.mark_changed();
        }
        pointer(resp, true)
    })
    .inner
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Thin progress line: grey track, red fill (white while hovered, like Spotify's scrubber).
pub fn progress_line(ui: &mut Ui, width: f32, frac: Option<f32>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 12.0), Sense::hover());
    paint_progress(ui, rect, frac, resp.hovered());
    resp
}

pub fn paint_progress(ui: &Ui, rect: Rect, frac: Option<f32>, hovered: bool) {
    let track = Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
    let painter = ui.painter();
    painter.rect_filled(track, CornerRadius::same(2), TRACK);
    match frac {
        Some(f) => {
            let f = f.clamp(0.0, 1.0);
            let mut fill = track;
            fill.set_width(track.width() * f);
            painter.rect_filled(fill, CornerRadius::same(2), accent().base);
            if hovered && f > 0.0 {
                painter.circle_filled(pos2(fill.right(), fill.center().y), 6.0, TEXT);
            }
        }
        None => {
            // Indeterminate: a sliding segment.
            let t = ui.input(|i| i.time) as f32;
            let seg = track.width() * 0.25;
            let x = (t * 0.6).fract() * (track.width() + seg) - seg;
            let start = (track.left() + x).max(track.left());
            let end = (track.left() + x + seg).min(track.right());
            if end > start {
                painter.rect_filled(
                    Rect::from_min_max(pos2(start, track.top()), pos2(end, track.bottom())),
                    CornerRadius::same(2),
                    accent().base,
                );
            }
            ui.ctx().request_repaint();
        }
    }
}

/// Scrubber-style bar with two handles selecting `start..=end` out of `0..=total` seconds.
/// Drags snap to whole seconds. Returns true when either end moved.
pub fn range_bar(ui: &mut Ui, start: &mut f64, end: &mut f64, total: f64) -> bool {
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), vec2(rect.width() - 16.0, 4.0));
    let to_x = |t: f64| track.left() + track.width() * (t / total.max(1e-6)).clamp(0.0, 1.0) as f32;
    let to_t = |x: f32| (((x - track.left()) / track.width()).clamp(0.0, 1.0) as f64 * total).round();

    // Grab whichever handle is nearer the press and remember it for the rest of the drag.
    let id = resp.id.with("handle");
    let mut changed = false;
    if let Some(pos) = resp.interact_pointer_pos() {
        let grabbing_end = if resp.drag_started() || resp.clicked() {
            let near_end = (pos.x - to_x(*end)).abs() < (pos.x - to_x(*start)).abs()
                || (to_x(*start) - to_x(*end)).abs() < 1.0 && pos.x > to_x(*end);
            ui.data_mut(|d| d.insert_temp(id, near_end));
            near_end
        } else {
            ui.data(|d| d.get_temp(id)).unwrap_or(false)
        };
        let t = to_t(pos.x);
        if grabbing_end {
            let v = t.max(*start + 1.0).min(total);
            changed |= v != *end;
            *end = v;
        } else {
            let v = t.min(*end - 1.0).max(0.0);
            changed |= v != *start;
            *start = v;
        }
    }

    let painter = ui.painter();
    painter.rect_filled(track, CornerRadius::same(2), TRACK);
    let sel = Rect::from_min_max(pos2(to_x(*start), track.top()), pos2(to_x(*end), track.bottom()));
    painter.rect_filled(sel, CornerRadius::same(2), accent().base);
    let hot = resp.hovered() || resp.dragged();
    for x in [sel.left(), sel.right()] {
        painter.circle_filled(pos2(x, track.center().y), if hot { 8.0 } else { 7.0 }, TEXT);
    }
    pointer(resp, true);
    changed
}

/// Sidebar navigation entry.
pub fn nav_item(ui: &mut Ui, icon: &str, label: &str, selected: bool, badge: Option<usize>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::same(6), Color32::from_gray(0x24));
        let bar = Rect::from_min_size(rect.left_top() + vec2(0.0, 10.0), vec2(3.0, 24.0));
        painter.rect_filled(bar, CornerRadius::same(2), accent().base);
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), Color32::from_gray(0x1C));
    }
    let color = if selected || resp.hovered() { TEXT } else { MUTED };
    painter.text(
        pos2(rect.left() + 18.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        icon,
        FontId::proportional(22.0),
        if selected { accent().base } else { color },
    );
    painter.text(
        pos2(rect.left() + 52.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::new(15.0, bold()),
        color,
    );
    if let Some(n) = badge.filter(|n| *n > 0) {
        let text = n.to_string();
        let galley = painter.layout_no_wrap(text, FontId::new(11.0, bold()), TEXT);
        let w = galley.size().x.max(10.0) + 10.0;
        let pill = Rect::from_center_size(pos2(rect.right() - 12.0 - w / 2.0, rect.center().y), vec2(w, 18.0));
        painter.rect_filled(pill, CornerRadius::same(255), accent().base);
        painter.galley(pill.center() - galley.size() / 2.0, galley, TEXT);
    }
    pointer(resp, true)
}

/// Indented sidebar entry under a [`nav_item`], with a count on the right. The glyph and
/// the selection bar take the current accent.
pub fn nav_sub_item(ui: &mut Ui, icon: &str, label: &str, selected: bool, count: usize) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::same(6), Color32::from_gray(0x24));
        let bar = Rect::from_min_size(rect.left_top() + vec2(0.0, 7.0), vec2(3.0, 18.0));
        painter.rect_filled(bar, CornerRadius::same(2), accent().base);
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), Color32::from_gray(0x1C));
    }
    let color = if selected || resp.hovered() { TEXT } else { MUTED };
    painter.text(
        pos2(rect.left() + 40.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        icon,
        FontId::proportional(17.0),
        accent().base,
    );
    painter.text(
        pos2(rect.left() + 64.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::new(14.0, bold()),
        color,
    );
    painter.text(
        pos2(rect.right() - 12.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        FontId::proportional(12.0),
        SUBTLE,
    );
    pointer(resp, true)
}

/// Square "album art" tile with an icon, used for jobs without a thumbnail.
pub fn art_tile(ui: &mut Ui, size: f32, icon: &str, tint: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(4), Color32::from_gray(0x28));
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        FontId::proportional(size * 0.45),
        tint,
    );
    resp
}

/// A label/control row used in the option cards.
pub fn option_row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(34.0);
        let (rect, _) = ui.allocate_exact_size(vec2(150.0, 20.0), Sense::hover());
        ui.painter().text(
            Pos2::new(rect.left(), rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            FontId::proportional(14.0),
            MUTED,
        );
        add(ui);
    });
}

/// Spotify-style scrubber over `progress` (0..=1). While dragging it follows the pointer;
/// returns the fraction to seek to once the press is released. `markers` are the A-B loop's
/// points (fractions too), drawn like the trim bar's handles with the span between them lit.
pub fn seek_bar(ui: &mut Ui, width: f32, progress: Option<f32>, markers: (Option<f32>, Option<f32>)) -> Option<f32> {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 16.0), Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
    let frac_at = |x: f32| ((x - track.left()) / track.width().max(1.0)).clamp(0.0, 1.0);
    let held = resp.is_pointer_button_down_on() || resp.dragged();
    let shown = match resp.interact_pointer_pos() {
        Some(p) if held && progress.is_some() => Some(frac_at(p.x)),
        _ => progress,
    };
    paint_progress(ui, rect, shown, resp.hovered() || held);
    let x_at = |f: f32| track.left() + track.width() * f.clamp(0.0, 1.0);
    if let (Some(a), Some(b)) = markers {
        let span = Rect::from_min_max(pos2(x_at(a), track.top() - 2.0), pos2(x_at(b), track.bottom() + 2.0));
        ui.painter().rect_filled(span, CornerRadius::same(2), accent().base.gamma_multiply(0.35));
    }
    for f in [markers.0, markers.1].into_iter().flatten() {
        let tick = Rect::from_center_size(pos2(x_at(f), track.center().y), vec2(3.0, 12.0));
        ui.painter().rect_filled(tick, CornerRadius::same(1), TEXT);
    }
    let released = resp.drag_stopped() || resp.clicked();
    let pos = resp.interact_pointer_pos().or(resp.hover_pos());
    pointer(resp, progress.is_some());
    match (released, pos, progress) {
        (true, Some(p), Some(_)) => Some(frac_at(p.x)),
        _ => None,
    }
}

/// White round play/pause button, the centre of the player bar.
pub fn play_button(ui: &mut Ui, playing: bool, enabled: bool) -> Response {
    let size = 36.0;
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), sense);
    let r = if enabled && resp.hovered() { size / 2.0 + 1.5 } else { size / 2.0 };
    let fill = if enabled { TEXT } else { Color32::from_gray(0x7C) };
    ui.painter().circle_filled(rect.center(), r, fill);
    // Drawn rather than taken from the icon font, which only has outlined glyphs.
    let c = rect.center();
    if playing {
        for dx in [-4.0, 4.0] {
            let bar = Rect::from_center_size(c + vec2(dx, 0.0), vec2(4.0, 14.0));
            ui.painter().rect_filled(bar, CornerRadius::same(1), BLACK);
        }
    } else {
        let pts = vec![c + vec2(-4.5, -7.5), c + vec2(8.0, 0.0), c + vec2(-4.5, 7.5)];
        ui.painter()
            .add(egui::Shape::convex_polygon(pts, BLACK, Stroke::NONE));
    }
    pointer(resp, enabled)
}

/// Thin horizontal slider styled like the progress line, for volume. The response is
/// marked changed while it changes `value` (0..=1).
pub fn line_slider(ui: &mut Ui, width: f32, value: &mut f32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 16.0), Sense::click_and_drag());
    if let Some(p) = resp.interact_pointer_pos()
        && (resp.dragged() || resp.clicked() || resp.is_pointer_button_down_on())
    {
        let v = ((p.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
        if v != *value {
            *value = v;
            resp.mark_changed();
        }
    }
    let hot = resp.hovered() || resp.dragged();
    let track = Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
    let painter = ui.painter();
    painter.rect_filled(track, CornerRadius::same(2), TRACK);
    let mut fill = track;
    fill.set_width(track.width() * value.clamp(0.0, 1.0));
    painter.rect_filled(fill, CornerRadius::same(2), if hot { accent().base } else { TEXT });
    if hot {
        painter.circle_filled(pos2(fill.right(), fill.center().y), 6.0, TEXT);
    }
    pointer(resp, true)
}

/// Square cover art cropped from the middle of `url` (a thumbnail URL or file), or the
/// `icon` tile while it loads or if it can't.
pub fn cover(ui: &mut Ui, size: f32, url: Option<&str>, icon: &str) -> Response {
    let Some(url) = url else {
        return art_tile(ui, size, icon, MUTED);
    };
    let image = egui::Image::new(url.to_owned());
    let loaded = image.load_for_size(ui.ctx(), Vec2::splat(size * 4.0));
    let Ok(egui::load::TexturePoll::Ready { texture }) = loaded else {
        return art_tile(ui, size, icon, MUTED);
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    // Video thumbnails are wide; keep the centre square rather than squashing them.
    let (w, h) = (texture.size.x.max(1.0), texture.size.y.max(1.0));
    let uv = if w > h {
        let margin = (1.0 - h / w) / 2.0;
        Rect::from_min_max(pos2(margin, 0.0), pos2(1.0 - margin, 1.0))
    } else {
        let margin = (1.0 - w / h) / 2.0;
        Rect::from_min_max(pos2(0.0, margin), pos2(1.0, 1.0 - margin))
    };
    egui::Image::from_texture(texture)
        .uv(uv)
        .corner_radius(4)
        .paint_at(ui, rect);
    resp
}

/// Icon button that stays red with a dot under it while `on`, like Spotify's shuffle.
pub fn toggle_icon(ui: &mut Ui, icon: &str, on: bool, tooltip: &str) -> Response {
    let size = 32.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if ui.is_rect_visible(rect) {
        let color = match (on, resp.hovered()) {
            (true, true) => accent().hover,
            (true, false) => accent().base,
            (false, true) => TEXT,
            (false, false) => MUTED,
        };
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon, FontId::proportional(18.0), color);
        if on {
            ui.painter().circle_filled(pos2(rect.center().x, rect.bottom() - 2.0), 2.0, accent().base);
        }
    }
    pointer(resp, true).on_hover_text(tooltip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_reads_on_every_pill() {
        for p in Platform::ALL.into_iter().map(accent_for).chain([Accent::REDLINE]) {
            let (fill, hover, pressed) = p.pill_colors();
            for c in [fill, hover, pressed] {
                assert!(contrast(TEXT, c) >= 3.0, "{p:?}: {c:?} is {}", contrast(TEXT, c));
            }
        }
        assert_eq!(Accent::REDLINE.pill_colors().0, Accent::REDLINE.base, "red is unchanged");
        let vimeo = accent_for(Platform::Vimeo);
        assert_ne!(vimeo.pill_colors().0, vimeo.base, "too light for white text: darkened");
    }

    #[test]
    fn tint_is_between_background_and_base() {
        let a = accent_for(Platform::SoundCloud);
        assert!(a.header_tint.r() > BG.r() && a.header_tint.r() < a.base.r());
        assert!(contrast(a.hover, BG) > contrast(a.pressed, BG));
    }
}
