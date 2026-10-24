//! Unified title bar: document title, search, page entry, zoom, rotate and window buttons.

use egui::{
    Align, Align2, Button, Id, Key, Layout, RichText, Sense, TextEdit, TextStyle, Ui,
    text::{LayoutJob, TextFormat, TextWrapping},
    vec2,
};

use super::{
    chrome,
    controls::{self, Icon},
    theme::{self, Palette},
};
use crate::{layout::parse_page_input, search::SearchState};

/// Horizontal gap between control groups.
const GROUP_GAP: f32 = 12.0;
/// Gap between the controls and the window buttons at the right end.
const LIGHTS_GAP: f32 = 16.0;
/// Side of the square toolbar buttons, and of the smaller search ⌃ ⌄ buttons.
const BUTTON: f32 = 28.0;
const SMALL_BUTTON: f32 = 24.0;
/// Search field width; it shrinks down to `SEARCH_MIN` so the right-hand controls leave the
/// first `TITLE_MIN` points of the title free.
const SEARCH_WIDTH: f32 = 180.0;
const SEARCH_MIN: f32 = 96.0;
const TITLE_MIN: f32 = 96.0;
/// Page field: text padding plus the width of one digit (Inter, Body size).
const PAGE_FIELD_PAD: f32 = 18.0;
const DIGIT_WIDTH: f32 = 9.0;

/// What the user did in the toolbar this frame (at most one).
#[derive(Debug)]
pub enum ToolbarAction {
    GoToPage(usize),
    /// ‹ or ›: the page before or after the scroll position (`forward`).
    StepPage(bool),
    ZoomIn,
    ZoomOut,
    FitWidth,
    RotateClockwise,
    SearchChanged(String),
    SearchNext,
    SearchPrev,
    SearchClosed,
}

/// Text-field buffers owned by the toolbar.
#[derive(Debug, Default)]
pub struct ToolbarState {
    page_text: String,
    pub search_text: String,
    /// Set by Ctrl+F in app.rs; consumed (focus requested) on the next `show()`.
    pub focus_search: bool,
}

/// Read-only data the toolbar displays.
pub struct DocInfo<'a> {
    /// `None` shows "Paperlight".
    pub name: Option<&'a str>,
    /// 0-based.
    pub current_page: usize,
    /// 0 = no document: page, zoom and search controls are disabled.
    pub total_pages: usize,
    pub zoom: f32,
    pub search: &'a SearchState,
}

/// Draws the bar contents (call inside the top panel). Layout per `docs/design.md`.
pub fn show(ui: &mut Ui, st: &mut ToolbarState, doc: &DocInfo<'_>) -> Option<ToolbarAction> {
    let pal = theme::palette(ui.visuals().dark_mode);
    chrome::title_bar(ui);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        // First, so they keep the right end however narrow the window gets.
        chrome::window_buttons(ui);
        ui.add_space(LIGHTS_GAP - ui.spacing().item_spacing.x);
        let action = ui
            .add_enabled_ui(doc.total_pages > 0, |ui| {
                let rotate = controls::icon_button(ui, Icon::RotateClockwise, BUTTON)
                    .on_hover_text("Rotate (Ctrl+→)")
                    .clicked()
                    .then_some(ToolbarAction::RotateClockwise);
                ui.add_space(GROUP_GAP - ui.spacing().item_spacing.x);
                let zoom = zoom_group(ui, doc.zoom);
                ui.add_space(GROUP_GAP - ui.spacing().item_spacing.x);
                let page = page_group(ui, st, doc, pal);
                ui.add_space(GROUP_GAP - ui.spacing().item_spacing.x);
                let search = search_group(ui, st, doc.search, pal);
                rotate.or(zoom).or(page).or(search)
            })
            .inner;
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            title(ui, doc, pal);
        });
        action
    })
    .inner
}

/// The file name (semibold) over "Page n of m", centred on the bar and truncated with `…`
/// (full name on hover). With no document: "Paperlight".
fn title(ui: &mut Ui, doc: &DocInfo<'_>, pal: &Palette) {
    let width = ui.available_width().max(0.0);
    let line = |text: String, style: &TextStyle, color| {
        let mut job =
            LayoutJob::single_section(text, TextFormat::simple(style.resolve(ui.style()), color));
        job.wrap = TextWrapping::truncate_at_width(width);
        ui.painter().layout_job(job)
    };
    let name = doc.name.unwrap_or("Paperlight");
    let heading = line(name.to_owned(), &TextStyle::Heading, pal.text_primary);
    let subtitle = (doc.total_pages > 0).then(|| {
        let text = format!("Page {} of {}", doc.current_page + 1, doc.total_pages);
        line(text, &TextStyle::Small, pal.text_secondary)
    });
    let height = heading.size().y + subtitle.as_ref().map_or(0.0, |g| g.size().y);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, ui.available_height()), Sense::hover());
    let mut pos = rect.left_center() - vec2(0.0, height / 2.0);
    let elided = heading.elided;
    let heading_h = heading.size().y;
    ui.painter().galley(pos, heading, pal.text_primary);
    if let Some(sub) = subtitle {
        pos.y += heading_h;
        ui.painter().galley(pos, sub, pal.text_secondary);
    }
    if elided {
        resp.on_hover_text(name);
    }
}

/// `− 100% +` (added right to left).
fn zoom_group(ui: &mut Ui, zoom: f32) -> Option<ToolbarAction> {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let zoom_in = controls::icon_button(ui, Icon::Plus, BUTTON)
            .on_hover_text("Zoom in (Ctrl++)")
            .clicked()
            .then_some(ToolbarAction::ZoomIn);
        let percent = format!("{:.0}%", (zoom * 100.0).round());
        let fit = ui
            .add(Button::new(percent).min_size(vec2(56.0, BUTTON)))
            .on_hover_text("Fit width (Ctrl+0)")
            .clicked()
            .then_some(ToolbarAction::FitWidth);
        let zoom_out = controls::icon_button(ui, Icon::Minus, BUTTON)
            .on_hover_text("Zoom out (Ctrl+−)")
            .clicked()
            .then_some(ToolbarAction::ZoomOut);
        zoom_in.or(fit).or(zoom_out)
    })
    .inner
}

/// `‹ [ 12 ] / 48 ›`: Enter commits, invalid input reverts (added right to left).
fn page_group(
    ui: &mut Ui,
    st: &mut ToolbarState,
    doc: &DocInfo<'_>,
    pal: &Palette,
) -> Option<ToolbarAction> {
    let id = Id::unique("page-entry");
    if !ui.memory(|m| m.has_focus(id)) {
        // "0 / 0" with no document or no pages.
        st.page_text = (doc.current_page + 1).min(doc.total_pages).to_string();
    }
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let next = controls::icon_button(ui, Icon::Right, BUTTON)
            .on_hover_text("Next page")
            .clicked()
            .then_some(ToolbarAction::StepPage(true));
        let total = RichText::new(format!("/ {}", doc.total_pages))
            .small()
            .color(pal.text_secondary);
        ui.label(total);
        let field = TextEdit::singleline(&mut st.page_text)
            .id(id)
            .desired_width(page_field_width(doc.total_pages))
            .align(Align2::CENTER_CENTER);
        let resp = controls::field(ui, field, None);
        let typed = (resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
            .then(|| parse_page_input(&st.page_text, doc.total_pages))
            .flatten()
            .map(ToolbarAction::GoToPage);
        let prev = controls::icon_button(ui, Icon::Left, BUTTON)
            .on_hover_text("Previous page")
            .clicked()
            .then_some(ToolbarAction::StepPage(false));
        next.or(typed).or(prev)
    })
    .inner
}

/// Width of the page field (`desired_width` includes the padding): room for the digits of
/// `total_pages`, at least two.
fn page_field_width(total_pages: usize) -> f32 {
    let digits = std::iter::successors(Some(total_pages.max(10)), |n| (*n >= 10).then(|| n / 10));
    digits.fold(PAGE_FIELD_PAD, |w, _| w + DIGIT_WIDTH)
}

/// Search field, then (with a query) the match count and ⌃ ⌄ (added right to left).
fn search_group(
    ui: &mut Ui,
    st: &mut ToolbarState,
    search: &SearchState,
    pal: &Palette,
) -> Option<ToolbarAction> {
    let mut action = None;
    if !search.query.is_empty() {
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let arrow = |ui: &mut Ui, icon, tip| {
                controls::icon_button(ui, icon, SMALL_BUTTON)
                    .on_hover_text(tip)
                    .clicked()
            };
            if arrow(ui, Icon::Down, "Next match (Enter)") {
                action = Some(ToolbarAction::SearchNext);
            }
            if arrow(ui, Icon::Up, "Previous match (Shift+Enter)") {
                action = Some(ToolbarAction::SearchPrev);
            }
            let color = if search.no_results() {
                pal.error
            } else {
                pal.text_secondary
            };
            ui.label(RichText::new(search.label()).small().color(color));
        });
    }
    let room = ui.available_width() - TITLE_MIN;
    let field = TextEdit::singleline(&mut st.search_text)
        .id(Id::unique("search"))
        .desired_width(room.clamp(SEARCH_MIN, SEARCH_WIDTH))
        .hint_text("Search");
    let resp = controls::field(ui, field, Some(Icon::Search));
    if std::mem::take(&mut st.focus_search) {
        resp.request_focus();
    }
    if resp.changed() {
        action = Some(ToolbarAction::SearchChanged(st.search_text.clone()));
    }
    if resp.lost_focus() {
        let (enter, escape, shift) = ui.input(|i| {
            (
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Escape),
                i.modifiers.shift,
            )
        });
        if enter {
            action = Some(if shift {
                ToolbarAction::SearchPrev
            } else {
                ToolbarAction::SearchNext
            });
            resp.request_focus();
        } else if escape {
            st.search_text.clear();
            action = Some(ToolbarAction::SearchClosed);
        }
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_field_fits_the_page_count() {
        let digits = |n: f32| PAGE_FIELD_PAD + n * DIGIT_WIDTH;
        for (total, want) in [
            (0, 2.0),
            (9, 2.0),
            (99, 2.0),
            (100, 3.0),
            (113, 3.0),
            (10_000, 5.0),
        ] {
            assert!(
                (page_field_width(total) - digits(want)).abs() < 1e-3,
                "{total}"
            );
        }
    }
}
