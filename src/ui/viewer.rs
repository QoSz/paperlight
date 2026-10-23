//! The page canvas: zoom, scroll, layout and painting of the visible pages.

use std::{
    ops::Range,
    time::{Duration, Instant},
};

use egui::{
    Color32, CursorIcon, PointerButton, Pos2, Rect, ScrollArea, Sense, Shadow, Stroke, StrokeKind,
    Vec2, emath::GuiRounding, pos2,
};

use super::theme::{self, PAGE_GAP, PAGE_MARGIN, RADIUS_SM};
use crate::{
    fling::Fling,
    layout::{
        PageLayout, PageSize, clamp_offset, clamp_zoom, fit_width_scale, reveal_offset, zoom_about,
    },
    render::cache::PageCache,
    search::SearchState,
    selection::{PageTexts, Probe, Selection, page_span, probe, runs},
};

/// Wait after the last zoom change before requesting new-bucket renders.
const ZOOM_SETTLE: Duration = Duration::from_millis(100);
/// Texture coordinates covering the whole texture.
const UV_FULL: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
/// Opacity of the selection highlight; the text under it stays readable.
const SELECTION_ALPHA: f32 = 0.55;

/// Zoom, scroll and layout of the page canvas. Survives across frames.
pub struct ViewerState {
    zoom: f32,
    /// True on open; any manual zoom clears it.
    fit_width: bool,
    layout: PageLayout,
    /// Viewport width `layout` was built for.
    layout_w: f32,
    /// Last frame's clamped scroll offset.
    offset: Vec2,
    /// Last frame's viewport (screen coords).
    view_rect: Rect,
    /// Zoom anchor relative to `view_rect.min`; `None` = viewport centre.
    anchor: Option<Vec2>,
    /// Forced into the `ScrollArea` on the next `show()`, then cleared.
    pending_offset: Option<Vec2>,
    zoom_changed_at: Option<Instant>,
    /// Kinetic scroll after a touchpad swipe; any other scroll input cancels it.
    fling: Option<Fling>,
    selection: Selection,
    /// Doc-space pointer position of the last hit test; `None` forces the next one.
    last_doc: Option<Pos2>,
    /// The pointer was over a char at the last hit test.
    over_text: bool,
}

/// Results of one viewer frame.
pub struct ViewerOutput {
    pub visible: Range<usize>,
    pub current_page: usize,
    /// Pages whose chars the viewer needs (pressed page, or the pages of a selection being dragged).
    pub want_text: Range<usize>,
}

impl Default for ViewerState {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            fit_width: true,
            layout: PageLayout::default(),
            layout_w: 0.0,
            offset: Vec2::ZERO,
            view_rect: Rect::ZERO,
            anchor: None,
            pending_offset: None,
            zoom_changed_at: None,
            fling: None,
            selection: Selection::default(),
            last_doc: None,
            over_text: false,
        }
    }
}

impl ViewerState {
    /// New document: fit the width and start at the top.
    pub fn reset(&mut self) {
        self.fit_width = true;
        self.layout = PageLayout::default();
        self.anchor = None;
        self.pending_offset = Some(Vec2::ZERO);
        self.zoom_changed_at = None;
        self.fling = None;
        self.selection.clear();
        self.last_doc = None;
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// Multiplies the zoom, anchored at `pointer` if it is over the viewport (else the centre).
    pub fn zoom_by(&mut self, factor: f32, pointer: Option<Pos2>) {
        self.zoom = clamp_zoom(self.zoom * factor);
        self.fit_width = false;
        self.fling = None;
        self.anchor = pointer
            .filter(|p| self.view_rect.contains(*p))
            .map(|p| p - self.view_rect.min);
        self.zoom_changed_at = Some(Instant::now());
    }

    /// Returns to fit-width mode, anchored at the viewport centre.
    pub fn fit_width(&mut self) {
        self.fit_width = true;
        self.anchor = None;
        self.zoom_changed_at = Some(Instant::now());
        self.fling = None;
    }

    /// Scrolls so that `page` starts just below the top of the viewport.
    pub fn go_to_page(&mut self, page: usize) {
        let target = Vec2::new(self.offset.x, self.layout.page_scroll_y(page));
        self.pending_offset = Some(self.clamp(target));
        self.fling = None;
    }

    /// Page that "next page" (`forward`) or "previous page" goes to from the scroll position.
    pub fn step_page(&self, forward: bool) -> usize {
        let y = self.pending_offset.unwrap_or(self.offset).y;
        self.layout.step_page(y, forward)
    }

    /// Scrolls vertically by `dy` points.
    pub fn scroll_by(&mut self, dy: f32) {
        let from = self.pending_offset.unwrap_or(self.offset);
        self.pending_offset = Some(self.clamp(from + Vec2::new(0.0, dy)));
        self.fling = None;
    }

    /// Scrolls so that `rect_pt` (page points, top-left origin) on `page` is visible.
    pub fn reveal(&mut self, page: usize, rect_pt: Rect) {
        let view = Rect::from_min_size(self.offset.to_pos2(), self.view_rect.size());
        if let Some(offset) = reveal_offset(view, self.layout.to_doc(page, rect_pt)) {
            self.pending_offset = Some(self.clamp(offset));
        }
        self.fling = None;
    }

    /// Starts coasting at `v` points per second (scroll-offset direction) from frame time `time`,
    /// if `pointer` is over the viewport (egui only scrolled the canvas then).
    pub fn fling(&mut self, v: f32, time: f64, pointer: Option<Pos2>) {
        if pointer.is_some_and(|p| self.view_rect.contains(p)) {
            self.fling = Some(Fling::new(v, time));
        }
    }

    pub fn cancel_fling(&mut self) {
        self.fling = None;
    }

    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    /// A page's chars arrived: resolves the selection and redoes the cursor hit test.
    pub fn text_loaded(&mut self, texts: &PageTexts) {
        self.selection.resolve(texts);
        self.last_doc = None;
    }

    /// Viewport height of the last frame.
    pub fn view_height(&self) -> f32 {
        self.view_rect.height()
    }

    /// Time left until the last zoom change settles; `None` once settled.
    pub fn settle_remaining(&self, now: Instant) -> Option<Duration> {
        let elapsed = now.saturating_duration_since(self.zoom_changed_at?);
        ZOOM_SETTLE.checked_sub(elapsed).filter(|d| !d.is_zero())
    }

    fn clamp(&self, offset: Vec2) -> Vec2 {
        clamp_offset(offset, self.layout.content_size(), self.view_rect.size())
    }
}

/// Lays out and paints the visible pages; consumes `pending_offset`.
pub fn show(
    ui: &mut egui::Ui,
    st: &mut ViewerState,
    sizes: &[PageSize],
    cache: &PageCache,
    wanted_px: f32,
    search: &SearchState,
    texts: &PageTexts,
) -> ViewerOutput {
    let vw = ui.available_width();
    if st.fit_width {
        let zoom = fit_width_scale(sizes, vw, PAGE_MARGIN);
        if zoom.to_bits() != st.zoom.to_bits() {
            st.zoom = zoom;
            st.zoom_changed_at = Some(Instant::now());
        }
    }
    if st.layout.scale().to_bits() != st.zoom.to_bits()
        || st.layout_w.to_bits() != vw.to_bits()
        || st.layout.rects().len() != sizes.len()
    {
        let new = PageLayout::new(sizes, st.zoom, vw, PAGE_GAP, PAGE_MARGIN);
        let old = std::mem::replace(&mut st.layout, new);
        st.layout_w = vw;
        if st.pending_offset.is_none() && !old.rects().is_empty() {
            let anchor = st.anchor.take().unwrap_or(st.view_rect.size() / 2.0);
            let offset = zoom_about(&old, &st.layout, anchor, st.offset);
            st.pending_offset = Some(st.clamp(offset));
        }
    }
    if let Some(fling) = st.fling.take() {
        let (dy, next) = fling.step(ui.input(|i| i.time));
        let target = st.pending_offset.unwrap_or(st.offset) + Vec2::new(0.0, dy);
        let clamped = st.clamp(target);
        st.pending_offset = Some(clamped);
        // Stops once the clamp holds it at the top or bottom.
        st.fling = next.filter(|_| (clamped.y - target.y).abs() < 1e-3);
        if st.fling.is_some() {
            ui.ctx().request_repaint();
        }
    }

    let mut area = ScrollArea::both().id_salt("pages").auto_shrink(false);
    if let Some(offset) = st.pending_offset.take() {
        area = area.scroll_offset(offset);
    }
    let layout = &st.layout;
    let (selection, last_doc, over_text) = (&mut st.selection, &mut st.last_doc, &mut st.over_text);
    let out = area.show_viewport(ui, |ui, view| {
        ui.set_min_size(layout.content_size());
        let want_text = select(ui, layout, texts, selection, last_doc, over_text);
        paint(ui, layout, view, cache, wanted_px, search);
        paint_selection(ui, layout, view, texts, selection);
        ViewerOutput {
            visible: layout.visible_range(view),
            current_page: layout.current_page(view),
            want_text,
        }
    });
    st.offset = out.state.clamped_offset();
    st.view_rect = out.inner_rect;
    out.inner
}

/// Paints each visible page: shadow, white placeholder, best texture, border, search hits.
fn paint(
    ui: &egui::Ui,
    layout: &PageLayout,
    view: Rect,
    cache: &PageCache,
    wanted_px: f32,
    search: &SearchState,
) {
    let pal = theme::palette(ui.visuals().dark_mode);
    let painter = ui.painter();
    let origin = ui.max_rect().min.to_vec2();
    let ppp = ui.pixels_per_point();
    let shadow = Shadow {
        offset: [0, 2],
        blur: 12,
        spread: 0,
        color: pal.page_shadow,
    };
    for i in layout.visible_range(view) {
        let r = layout.rects()[i].translate(origin).round_to_pixels(ppp);
        painter.add(shadow.as_shape(r, 0.0));
        painter.rect_filled(r, 0.0, Color32::WHITE);
        if let Some((id, _)) = cache.get_best(i, wanted_px) {
            painter.image(id, r, UV_FULL, Color32::WHITE);
        }
        painter.rect_stroke(
            r,
            0.0,
            Stroke::new(1.0, pal.page_border),
            StrokeKind::Inside,
        );
        let (first, hits) = search.page_hits(i);
        for (k, hit) in hits.iter().enumerate() {
            let fill = if search.current == Some(first + k) {
                pal.hit_current
            } else {
                pal.hit
            };
            for rect in &hit.rects {
                let r = layout.to_doc(i, *rect).translate(origin).expand(1.0);
                painter.rect_filled(r, RADIUS_SM, fill);
            }
        }
    }
}

/// Pointer input on the canvas: a primary drag without Ctrl that starts on a page selects text, a
/// click clears the selection, and the cursor is a text cursor over chars. Hit tests run only when
/// the pointer moved in doc space. Returns the pages whose chars are needed.
fn select(
    ui: &egui::Ui,
    layout: &PageLayout,
    texts: &PageTexts,
    sel: &mut Selection,
    last_doc: &mut Option<Pos2>,
    over_text: &mut bool,
) -> Range<usize> {
    let origin = ui.max_rect().min.to_vec2();
    // Not focusable: a focused canvas would disable the navigation keys.
    let resp = ui.interact(
        ui.clip_rect(),
        ui.scope_id().with("select"),
        Sense::CLICK | Sense::DRAG,
    );
    let mut want_text = 0..0;
    if resp.clicked() {
        sel.clear();
    }
    if ui.input(|i| i.pointer.primary_pressed())
        && let Some(p) = resp.hover_pos()
        && let Some(Probe::At(page, _)) = probe(layout, p - origin)
    {
        want_text = page..page + 1;
    }
    let doc = ui.input(|i| i.pointer.latest_pos()).map(|p| p - origin);
    let mut moved = doc != *last_doc;
    if resp.drag_started_by(PointerButton::Primary) {
        sel.clear();
        let press = ui.input(|i| i.pointer.press_origin()).map(|p| p - origin);
        if !ui.input(|i| i.modifiers.ctrl)
            && let Some(press) = press
            && let Some(Probe::At(page, at)) = probe(layout, press)
            && layout.rects()[page].contains(press)
        {
            sel.begin(page, at);
            // egui only unfocuses a text field on clicks; Ctrl+C must reach the selection.
            ui.memory_mut(egui::Memory::stop_text_input);
            moved = true;
        }
    }
    let dragging = resp.dragged_by(PointerButton::Primary) && !sel.is_empty();
    if moved {
        *last_doc = doc;
        let at = doc.and_then(|d| probe(layout, d));
        if dragging && let Some(head) = at {
            sel.drag_to(head);
            sel.resolve(texts);
        }
        *over_text = matches!(at, Some(Probe::At(page, pt))
            if texts.get(&page).is_some_and(|chars| chars.iter().any(|c| c.rect.contains(pt))));
    }
    if dragging {
        want_text = sel.pages();
    }
    if dragging || (resp.hovered() && *over_text) {
        ui.ctx().set_cursor_icon(CursorIcon::Text);
    }
    want_text
}

/// Selection highlight over the visible pages: one translucent rect per text line.
fn paint_selection(
    ui: &egui::Ui,
    layout: &PageLayout,
    view: Rect,
    texts: &PageTexts,
    sel: &Selection,
) {
    let Some((lo, hi)) = sel.range() else {
        return;
    };
    let fill = theme::palette(ui.visuals().dark_mode)
        .selection_bg
        .gamma_multiply(SELECTION_ALPHA);
    let origin = ui.max_rect().min.to_vec2();
    for page in layout.visible_range(view) {
        let Some(chars) = texts.get(&page) else {
            continue;
        };
        for run in runs(&chars[page_span(lo, hi, page, chars.len())]) {
            ui.painter()
                .rect_filled(layout.to_doc(page, run).translate(origin), 0.0, fill);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_fling_is_none() {
        let over = Some(pos2(10.0, 10.0));
        let mut st = ViewerState {
            view_rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(200.0, 200.0)),
            layout: PageLayout::new(
                &[PageSize { w: 100.0, h: 100.0 }; 2],
                1.0,
                200.0,
                24.0,
                32.0,
            ),
            ..ViewerState::default()
        };
        let cancels: [fn(&mut ViewerState); 6] = [
            |st| st.zoom_by(1.25, None),
            |st| st.fit_width(),
            |st| st.go_to_page(1),
            |st| st.scroll_by(48.0),
            |st| st.reveal(1, Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 10.0))),
            ViewerState::cancel_fling,
        ];
        for cancel in cancels {
            st.fling(500.0, 0.0, over);
            assert!(st.fling.is_some());
            cancel(&mut st);
            assert!(st.fling.is_none());
        }
        st.fling(500.0, 0.0, over);
        st.reset();
        assert!(st.fling.is_none());
        // A swipe outside the canvas (over the toolbar) did not scroll it, so it does not fling.
        st.fling(500.0, 0.0, Some(pos2(10.0, -20.0)));
        assert!(st.fling.is_none());
    }
}
