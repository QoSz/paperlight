//! Toolbar: file name, search, page entry and zoom controls.

use egui::{Align, Align2, Button, Id, Key, Label, Layout, RichText, TextEdit, Ui, vec2};

use super::theme::{self, Palette, RADIUS_SM};
use crate::{layout::parse_page_input, search::SearchState};

/// Horizontal gap between control groups.
const GROUP_GAP: f32 = 16.0;

/// What the user did in the toolbar this frame (at most one).
#[derive(Debug)]
pub enum ToolbarAction {
    GoToPage(usize),
    ZoomIn,
    ZoomOut,
    FitWidth,
    SearchChanged(String),
    SearchNext,
    SearchPrev,
    SearchClosed,
}

/// Text-field buffers owned by the toolbar.
#[derive(Debug, Default)]
pub struct ToolbarState {
    pub page_text: String,
    pub search_text: String,
    /// Set by Ctrl+F in app.rs; consumed (focus requested) on the next `show()`.
    pub focus_search: bool,
}

/// Read-only data the toolbar displays.
pub struct DocInfo<'a> {
    /// `None` shows "No document".
    pub name: Option<&'a str>,
    /// 0-based.
    pub current_page: usize,
    /// 0 = no document: page, zoom and search controls are disabled.
    pub total_pages: usize,
    pub zoom: f32,
    pub search: &'a SearchState,
}

/// Draws the toolbar contents (call inside the top panel). Layout per `docs/design.md`.
pub fn show(ui: &mut Ui, st: &mut ToolbarState, doc: &DocInfo<'_>) -> Option<ToolbarAction> {
    let pal = theme::palette(ui.visuals().dark_mode);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let action = ui
            .add_enabled_ui(doc.total_pages > 0, |ui| {
                let zoom = zoom_group(ui, doc.zoom);
                ui.add_space(GROUP_GAP - ui.spacing().item_spacing.x);
                let page = page_group(ui, st, doc, pal);
                ui.add_space(GROUP_GAP - ui.spacing().item_spacing.x);
                let search = search_group(ui, st, doc.search, pal);
                zoom.or(page).or(search)
            })
            .inner;
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| match doc.name {
            Some(name) => ui.add(Label::new(name).truncate()),
            None => ui.label(RichText::new("No document").color(pal.text_secondary)),
        });
        action
    })
    .inner
}

/// `− 100% +` (added right to left).
fn zoom_group(ui: &mut Ui, zoom: f32) -> Option<ToolbarAction> {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let square = vec2(28.0, 28.0);
        let zoom_in = ui.add(Button::new("+").min_size(square)).clicked();
        let percent = format!("{:.0}%", (zoom * 100.0).round());
        let fit = ui
            .add(Button::new(percent).min_size(vec2(56.0, 28.0)))
            .on_hover_text("Fit width")
            .clicked();
        let zoom_out = ui.add(Button::new("−").min_size(square)).clicked();
        if zoom_in {
            Some(ToolbarAction::ZoomIn)
        } else if fit {
            Some(ToolbarAction::FitWidth)
        } else {
            zoom_out.then_some(ToolbarAction::ZoomOut)
        }
    })
    .inner
}

/// `[ 12 ] / 48`: Enter commits, invalid input reverts (added right to left).
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
    let total = RichText::new(format!("/ {}", doc.total_pages))
        .small()
        .color(pal.text_secondary);
    ui.label(total);
    let field = TextEdit::singleline(&mut st.page_text)
        .id(id)
        .desired_width(44.0)
        .align(Align2::CENTER_CENTER);
    let resp = ui.add(field);
    if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
        parse_page_input(&st.page_text, doc.total_pages).map(ToolbarAction::GoToPage)
    } else {
        None
    }
}

/// Search field, then (with a query) the match count and ▲ ▼ (added right to left).
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
            let arrow = |glyph: &str| {
                Button::new(RichText::new(glyph).monospace())
                    .min_size(vec2(24.0, 24.0))
                    .corner_radius(RADIUS_SM)
            };
            if ui.add(arrow("▼")).clicked() {
                action = Some(ToolbarAction::SearchNext);
            }
            if ui.add(arrow("▲")).clicked() {
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
    let field = TextEdit::singleline(&mut st.search_text)
        .id(Id::unique("search"))
        .desired_width(220.0)
        .hint_text("Search");
    let resp = ui.add(field);
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
