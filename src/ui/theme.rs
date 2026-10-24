//! Paperlight design tokens and egui style. Spec: `docs/design.md`.

use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke,
    TextStyle, Theme, vec2,
};

/// Height of the unified title bar, in points.
pub const TOOLBAR_HEIGHT: f32 = 52.0;
/// Vertical gap between consecutive pages on the canvas, in points.
pub const PAGE_GAP: f32 = 24.0;
/// Padding between the canvas edges and the page column, in points.
pub const PAGE_MARGIN: f32 = 32.0;
/// Small corner radius: search-hit highlights.
pub const RADIUS_SM: u8 = 4;
/// Medium corner radius: buttons, text fields, tooltips.
pub const RADIUS_MD: u8 = 7;
/// Width of the window-resize band along each edge (`chrome::window_edges`), in points.
pub const RESIZE_EDGE: f32 = 5.0;
/// Opacity of `selection_bg` behind selected text: `#007AFF` at 30 % over white is macOS's
/// `#B3D7FF`, and the text under it stays readable.
pub const SELECTION_ALPHA: f32 = 0.3;

/// Inter (SIL OFL 1.1, `assets/fonts/Inter-LICENSE.txt`), the closest open match to SF Pro.
const INTER: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");
/// Font family of `TextStyle::Heading`: the document title.
const SEMIBOLD: &str = "semibold";

/// Semantic colours for one theme. Get the active one with [`palette`].
pub struct Palette {
    /// Background behind the pages.
    pub canvas_bg: Color32,
    /// Title bar surface; also tooltips and popups.
    pub toolbar_bg: Color32,
    /// Hairline under the title bar, around popups and around the window.
    pub toolbar_border: Color32,
    /// Body text and icons.
    pub text_primary: Color32,
    /// Muted text: placeholder, subtitle, match count, empty state.
    pub text_secondary: Color32,
    /// Focus ring, text cursor, links.
    pub accent: Color32,
    /// Resting fill of text fields.
    pub field_bg: Color32,
    /// Fill of a hovered button.
    pub hover_bg: Color32,
    /// Fill of a pressed or keyboard-focused button.
    pub active_bg: Color32,
    /// Selected text on the page and in fields, drawn at `SELECTION_ALPHA`.
    pub selection_bg: Color32,
    /// Translucent overlay on every search hit.
    pub hit: Color32,
    /// Translucent overlay on the current search hit (stronger than `hit`).
    pub hit_current: Color32,
    /// Translucent drop shadow under each page and popup.
    pub page_shadow: Color32,
    /// Hairline around each page.
    pub page_border: Color32,
    /// Inline error text.
    pub error: Color32,
}

/// Light theme: macOS window greys and system blue.
const LIGHT: Palette = Palette {
    canvas_bg: Color32::from_rgb(0xE5, 0xE5, 0xEA),
    toolbar_bg: Color32::from_rgb(0xF5, 0xF5, 0xF7),
    toolbar_border: Color32::from_rgb(0xD2, 0xD2, 0xD7),
    text_primary: Color32::from_rgb(0x1D, 0x1D, 0x1F),
    text_secondary: Color32::from_rgb(0x63, 0x63, 0x66),
    accent: Color32::from_rgb(0x00, 0x7A, 0xFF),
    field_bg: Color32::from_rgb(0xE8, 0xE8, 0xED),
    hover_bg: Color32::from_rgb(0xE8, 0xE8, 0xED),
    active_bg: Color32::from_rgb(0xD9, 0xD9, 0xDE),
    selection_bg: Color32::from_rgb(0x00, 0x7A, 0xFF),
    hit: Color32::from_rgba_unmultiplied_const(0xFF, 0xD6, 0x0A, 102),
    hit_current: Color32::from_rgba_unmultiplied_const(0xFF, 0x9F, 0x0A, 148),
    page_shadow: Color32::from_rgba_unmultiplied_const(0x00, 0x00, 0x00, 38),
    page_border: Color32::from_rgba_unmultiplied_const(0x00, 0x00, 0x00, 15),
    error: Color32::from_rgb(0xC4, 0x00, 0x1A),
};

/// Dark theme: macOS dark greys and system blue.
const DARK: Palette = Palette {
    canvas_bg: Color32::from_rgb(0x1C, 0x1C, 0x1E),
    toolbar_bg: Color32::from_rgb(0x2C, 0x2C, 0x2E),
    toolbar_border: Color32::from_rgb(0x3A, 0x3A, 0x3C),
    text_primary: Color32::from_rgb(0xF5, 0xF5, 0xF7),
    text_secondary: Color32::from_rgb(0xAE, 0xAE, 0xB2),
    accent: Color32::from_rgb(0x0A, 0x84, 0xFF),
    field_bg: Color32::from_rgb(0x36, 0x36, 0x38),
    hover_bg: Color32::from_rgb(0x3A, 0x3A, 0x3C),
    active_bg: Color32::from_rgb(0x48, 0x48, 0x4A),
    selection_bg: Color32::from_rgb(0x0A, 0x84, 0xFF),
    hit: Color32::from_rgba_unmultiplied_const(0xFF, 0xD6, 0x0A, 112),
    hit_current: Color32::from_rgba_unmultiplied_const(0xFF, 0x9F, 0x0A, 158),
    page_shadow: Color32::from_rgba_unmultiplied_const(0x00, 0x00, 0x00, 150),
    page_border: Color32::from_rgba_unmultiplied_const(0x00, 0x00, 0x00, 64),
    error: Color32::from_rgb(0xFF, 0x69, 0x61),
};

/// Palette for the current theme; pass `ctx.style().visuals.dark_mode`.
pub fn palette(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}

/// Installs Paperlight's fonts and its style into both egui themes. Call once at startup;
/// egui then switches between them as the system theme changes.
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    for (theme, p) in [(Theme::Light, &LIGHT), (Theme::Dark, &DARK)] {
        ctx.style_mut_of(theme, |s| {
            let ts = &mut s.text_styles;
            ts.insert(TextStyle::Body, FontId::proportional(14.0));
            ts.insert(TextStyle::Button, FontId::proportional(14.0));
            ts.insert(TextStyle::Small, FontId::proportional(12.0));
            ts.insert(
                TextStyle::Heading,
                FontId::new(14.0, FontFamily::Name(SEMIBOLD.into())),
            );

            s.spacing.item_spacing = vec2(8.0, 6.0);
            s.spacing.button_padding = vec2(8.0, 5.0);
            s.spacing.interact_size.y = 28.0;
            // No fade over the pages at the scroll edges: macOS shows content edge to edge.
            s.spacing.scroll.fade.strength = 0.0;
            // Keep scroll bars clear of the resize bands, which would take their drags.
            s.spacing.scroll.bar_outer_margin = RESIZE_EDGE;
            // Labels are display text; dragging over the title must move the window.
            s.interaction.selectable_labels = false;

            let v = &mut s.visuals;
            v.panel_fill = p.canvas_bg;
            v.window_fill = p.toolbar_bg;
            v.window_stroke = Stroke::new(1.0, p.toolbar_border);
            v.window_corner_radius = CornerRadius::same(RADIUS_MD);
            v.menu_corner_radius = CornerRadius::same(RADIUS_MD);
            v.window_shadow = Shadow::NONE;
            v.popup_shadow = Shadow {
                offset: [0, 2],
                blur: 8,
                spread: 0,
                color: p.page_shadow,
            };
            v.extreme_bg_color = p.field_bg; // text-field fill, scrollbar track
            v.weak_text_color = Some(p.text_secondary);
            v.hyperlink_color = p.accent;
            v.error_fg_color = p.error;
            v.text_cursor.stroke = Stroke::new(2.0, p.accent);
            // egui draws selected field text in `selection.stroke`; keep it the text colour.
            // Fields draw their own focus ring (`controls::field`).
            v.selection.bg_fill = p.selection_bg.gamma_multiply(SELECTION_ALPHA);
            v.selection.stroke = Stroke::new(1.0, p.text_primary);

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

/// egui's default fonts with Inter first for proportional text, plus Inter `SemiBold` as the
/// `SEMIBOLD` family. The defaults stay as fallbacks (symbols, emoji, other scripts).
fn fonts() -> FontDefinitions {
    let mut f = FontDefinitions::default();
    for (name, bytes) in [("Inter", INTER), ("Inter-SemiBold", INTER_SEMIBOLD)] {
        f.font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    let fallbacks = f
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let family = |first: &str| [vec![first.to_owned()], fallbacks.clone()].concat();
    f.families.insert(FontFamily::Proportional, family("Inter"));
    f.families
        .insert(FontFamily::Name(SEMIBOLD.into()), family("Inter-SemiBold"));
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG 2.x contrast ratio of two opaque colours.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let luminance = |c: Color32| {
            let lin = |v: u8| {
                let v = f32::from(v) / 255.0;
                if v <= 0.040_45 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
        };
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn text_meets_wcag_aa_on_every_surface() {
        for p in [&LIGHT, &DARK] {
            for bg in [p.toolbar_bg, p.canvas_bg, p.field_bg, p.hover_bg] {
                for fg in [p.text_primary, p.text_secondary] {
                    let ratio = contrast(fg, bg);
                    assert!(ratio >= 4.5, "{fg:?} on {bg:?}: {ratio:.2}");
                }
            }
            assert!(contrast(p.text_primary, p.active_bg) >= 4.5);
            for bg in [p.toolbar_bg, p.canvas_bg] {
                assert!(contrast(p.error, bg) >= 4.5, "error on {bg:?}");
            }
            // The text cursor (UI component, 3:1).
            assert!(contrast(p.accent, p.field_bg) >= 3.0);
        }
    }

    #[test]
    fn inter_leads_both_families() {
        let f = fonts();
        let first = |family: FontFamily| f.families.get(&family).and_then(|v| v.first().cloned());
        assert_eq!(first(FontFamily::Proportional).as_deref(), Some("Inter"));
        assert_eq!(
            first(FontFamily::Name(SEMIBOLD.into())).as_deref(),
            Some("Inter-SemiBold")
        );
        // Defaults stay as fallbacks.
        assert!(f.families[&FontFamily::Proportional].len() > 1);
        assert!(f.font_data.contains_key("Inter-SemiBold"));
    }
}
