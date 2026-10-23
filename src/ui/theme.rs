//! Paperlight design tokens and egui style. Spec: `docs/design.md`.

use egui::{Color32, CornerRadius, FontId, Shadow, Stroke, TextStyle, Theme, vec2};

/// Height of the top toolbar, in points.
pub const TOOLBAR_HEIGHT: f32 = 44.0;
/// Vertical gap between consecutive pages on the canvas, in points.
pub const PAGE_GAP: f32 = 24.0;
/// Padding between the canvas edges and the page column, in points.
pub const PAGE_MARGIN: f32 = 32.0;
/// Small corner radius: search-hit highlights, controls nested in a field.
pub const RADIUS_SM: u8 = 4;
/// Medium corner radius: buttons, text fields, tooltips.
pub const RADIUS_MD: u8 = 6;

/// Semantic colours for one theme. Get the active one with [`palette`].
pub struct Palette {
    /// Background behind the pages.
    pub canvas_bg: Color32,
    /// Toolbar surface; also tooltips and popups.
    pub toolbar_bg: Color32,
    /// Hairline under the toolbar and around popups.
    pub toolbar_border: Color32,
    /// Body text and icons.
    pub text_primary: Color32,
    /// Muted text: placeholder, match count, empty state, "no file" name.
    pub text_secondary: Color32,
    /// Focus ring, text cursor, selected text, links.
    pub accent: Color32,
    /// Resting fill of text fields.
    pub field_bg: Color32,
    /// Fill of a hovered button.
    pub hover_bg: Color32,
    /// Fill of a pressed or keyboard-focused button.
    pub active_bg: Color32,
    /// Background behind selected text.
    pub selection_bg: Color32,
    /// Translucent overlay on every search hit.
    pub hit: Color32,
    /// Translucent overlay on the current search hit (stronger than `hit`).
    pub hit_current: Color32,
    /// Translucent drop shadow under each page.
    pub page_shadow: Color32,
    /// 1 px hairline around each page.
    pub page_border: Color32,
    /// Inline error text.
    pub error: Color32,
}

/// Warm paper palette for the light theme.
pub const LIGHT: Palette = Palette {
    canvas_bg: Color32::from_rgb(0xE8, 0xE4, 0xDC),
    toolbar_bg: Color32::from_rgb(0xF7, 0xF5, 0xF0),
    toolbar_border: Color32::from_rgb(0xDC, 0xD6, 0xCC),
    text_primary: Color32::from_rgb(0x24, 0x21, 0x1D),
    text_secondary: Color32::from_rgb(0x6A, 0x64, 0x5A),
    accent: Color32::from_rgb(0x8A, 0x57, 0x0C),
    field_bg: Color32::from_rgb(0xEC, 0xE8, 0xE1),
    hover_bg: Color32::from_rgb(0xEC, 0xE8, 0xE1),
    active_bg: Color32::from_rgb(0xE1, 0xDC, 0xD3),
    selection_bg: Color32::from_rgb(0xF5, 0xE3, 0xBC),
    hit: Color32::from_rgba_unmultiplied_const(0xFF, 0xC4, 0x2E, 97),
    hit_current: Color32::from_rgba_unmultiplied_const(0xF0, 0x86, 0x12, 148),
    page_shadow: Color32::from_rgba_unmultiplied_const(0x30, 0x24, 0x14, 41),
    page_border: Color32::from_rgb(0xCD, 0xC6, 0xBA),
    error: Color32::from_rgb(0xA8, 0x40, 0x2F),
};

/// Lamplit graphite palette for the dark theme.
pub const DARK: Palette = Palette {
    canvas_bg: Color32::from_rgb(0x1A, 0x19, 0x17),
    toolbar_bg: Color32::from_rgb(0x26, 0x24, 0x22),
    toolbar_border: Color32::from_rgb(0x38, 0x35, 0x2F),
    text_primary: Color32::from_rgb(0xED, 0xE8, 0xDF),
    text_secondary: Color32::from_rgb(0xA3, 0x9D, 0x93),
    accent: Color32::from_rgb(0xEB, 0xB0, 0x4F),
    field_bg: Color32::from_rgb(0x33, 0x30, 0x2C),
    hover_bg: Color32::from_rgb(0x33, 0x30, 0x2C),
    active_bg: Color32::from_rgb(0x3E, 0x3A, 0x35),
    selection_bg: Color32::from_rgb(0x55, 0x40, 0x1A),
    hit: Color32::from_rgba_unmultiplied_const(0xFF, 0xC4, 0x2E, 107),
    hit_current: Color32::from_rgba_unmultiplied_const(0xF0, 0x86, 0x12, 158),
    page_shadow: Color32::from_rgba_unmultiplied_const(0x00, 0x00, 0x00, 128),
    page_border: Color32::from_rgb(0x0D, 0x0C, 0x0B),
    error: Color32::from_rgb(0xEC, 0x8C, 0x7D),
};

/// Palette for the current theme; pass `ctx.style().visuals.dark_mode`.
pub fn palette(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}

/// Installs Paperlight's style into both egui themes. Call once at startup;
/// egui then switches between them as the system theme changes.
pub fn apply(ctx: &egui::Context) {
    for (theme, p) in [(Theme::Light, &LIGHT), (Theme::Dark, &DARK)] {
        ctx.style_mut_of(theme, |s| {
            let ts = &mut s.text_styles;
            ts.insert(TextStyle::Body, FontId::proportional(14.0));
            ts.insert(TextStyle::Button, FontId::proportional(14.0));
            ts.insert(TextStyle::Small, FontId::proportional(12.0));

            s.spacing.item_spacing = vec2(8.0, 6.0);
            s.spacing.button_padding = vec2(10.0, 6.0);
            s.spacing.interact_size.y = 28.0;

            let v = &mut s.visuals;
            v.panel_fill = p.canvas_bg;
            v.window_fill = p.toolbar_bg;
            v.window_stroke = Stroke::new(1.0, p.toolbar_border);
            v.window_shadow = Shadow::NONE;
            v.popup_shadow = Shadow::NONE;
            v.extreme_bg_color = p.field_bg; // text-field fill, scrollbar track
            v.weak_text_color = Some(p.text_secondary);
            v.hyperlink_color = p.accent;
            v.error_fg_color = p.error;
            v.text_cursor.stroke = Stroke::new(2.0, p.accent);
            // Selected text is drawn in `selection.stroke`; a focused TextEdit
            // is outlined with it too, so it doubles as the field focus ring.
            v.selection.bg_fill = p.selection_bg;
            v.selection.stroke = Stroke::new(2.0, p.accent);

            // Buttons paint `weak_bg_fill`: transparent at rest = flat toolbar.
            let hairline = Stroke::new(1.0, p.toolbar_border);
            let ring = Stroke::new(1.0, p.accent);
            let (bar, clear) = (p.toolbar_bg, Color32::TRANSPARENT);
            let w = &mut v.widgets;
            for (wv, weak_fill, fill, border, fg_width) in [
                (&mut w.noninteractive, bar, bar, hairline, 1.0),
                (&mut w.inactive, clear, p.field_bg, Stroke::NONE, 1.0),
                (&mut w.hovered, p.hover_bg, p.hover_bg, Stroke::NONE, 1.5),
                (&mut w.active, p.active_bg, p.active_bg, ring, 2.0),
            ] {
                wv.weak_bg_fill = weak_fill;
                wv.bg_fill = fill;
                wv.bg_stroke = border;
                wv.fg_stroke = Stroke::new(fg_width, p.text_primary);
                wv.corner_radius = CornerRadius::same(RADIUS_MD);
            }
        });
    }
}
