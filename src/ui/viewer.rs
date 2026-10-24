//! The page canvas: zoom, scroll, layout and painting of the visible pages.

use std::{
    ops::Range,
    time::{Duration, Instant},
};

use egui::{
    Color32, CursorIcon, Mesh, PointerButton, Pos2, Rect, ScrollArea, Sense, Shadow, Shape, Stroke,
    StrokeKind, TextStyle, TextureId, Vec2, emath::GuiRounding, epaint::Vertex,
};

use super::theme::{self, PAGE_GAP, PAGE_MARGIN, RADIUS_SM, SELECTION_ALPHA};
use crate::{
    fling::Fling,
    layout::{
        PageLayout, PageSize, Rotation, clamp_offset, clamp_zoom, fit_width_scale, reveal_offset,
        zoom_about, zoomed_in,
    },
    render::cache::PageCache,
    search::SearchState,
    selection::{
        ANCHOR_CAP, PageTexts, Probe, Selection, line_at, nearest, page_span, probe, runs, word_at,
    },
};

/// Wait after the last zoom change before requesting new-bucket renders.
const ZOOM_SETTLE: Duration = Duration::from_millis(100);
/// Zoomed in (`zoomed_in`), off-screen pages are prefetched (±1) and kept (±2) only within this many
/// view heights of the viewport: fewer far-off textures once a page plus its gap is taller than 2
/// view heights. At fit width's render scale and below, the page windows always apply.
const PREFETCH_VIEWPORTS: f32 = 2.0;
const KEEP_VIEWPORTS: f32 = 4.0;

/// Zoom, scroll and layout of the page canvas. Survives across frames.
pub struct ViewerState {
    zoom: f32,
    /// True on open; any manual zoom clears it.
    fit_width: bool,
    /// Rotation of every page, clockwise.
    rotation: Rotation,
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
    /// Page to put at the top once the next `show()` has rebuilt the layout.
    pending_page: Option<usize>,
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
    /// Pages to render: the visible ones ±1 (zoomed in: within `PREFETCH_VIEWPORTS`).
    pub prefetch: Range<usize>,
    /// Pages whose exact-scale textures are kept: the visible ones ±2 (zoomed in: within
    /// `KEEP_VIEWPORTS`).
    pub keep: Range<usize>,
    /// Rendering above fit width's render scale, so renders are of a zoomed-in page.
    pub zoomed: bool,
    pub current_page: usize,
    /// Pages whose chars the viewer needs (pressed page, or the pages of a selection being dragged).
    pub want_text: Range<usize>,
}

impl Default for ViewerState {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            fit_width: true,
            rotation: Rotation::R0,
            layout: PageLayout::default(),
            layout_w: 0.0,
            offset: Vec2::ZERO,
            view_rect: Rect::ZERO,
            anchor: None,
            pending_offset: None,
            pending_page: None,
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
        self.rotation = Rotation::R0;
        self.layout = PageLayout::default();
        self.anchor = None;
        self.pending_offset = Some(Vec2::ZERO);
        self.pending_page = None;
        self.zoom_changed_at = None;
        self.fling = None;
        self.selection.clear();
        self.last_doc = None;
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// Rotates every page a quarter turn and keeps `page` at the top of the viewport.
    pub fn rotate(&mut self, clockwise: bool, page: usize) {
        self.rotation = self.rotation.turned(clockwise);
        self.pending_page = Some(page);
        self.zoom_changed_at = Some(Instant::now());
        self.fling = None;
        self.last_doc = None;
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
    let fit = fit_width_scale(sizes, st.rotation, vw, PAGE_MARGIN);
    if st.fit_width && fit.to_bits() != st.zoom.to_bits() {
        st.zoom = fit;
        st.zoom_changed_at = Some(Instant::now());
    }
    // Fit width is never zoomed in; its `wanted_px` can still be last frame's (a resize).
    let zoomed = !st.fit_width && zoomed_in(wanted_px, fit, ui.pixels_per_point());
    // Taken even without a rebuild (a rotation undone within one frame), so it cannot go stale.
    let pending_page = st.pending_page.take();
    if st.layout.scale().to_bits() != st.zoom.to_bits()
        || st.layout.rotation() != st.rotation
        || st.layout_w.to_bits() != vw.to_bits()
        || st.layout.rects().len() != sizes.len()
    {
        let new = PageLayout::new(sizes, st.zoom, st.rotation, vw, PAGE_GAP, PAGE_MARGIN);
        let old = std::mem::replace(&mut st.layout, new);
        st.layout_w = vw;
        if let Some(page) = pending_page {
            st.go_to_page(page);
        } else if st.pending_offset.is_none() && !old.rects().is_empty() {
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
        // egui checks a forced offset against last frame's content size and rubber-bands it back
        // when it lies past that edge (zooming in grows the content). It is already clamped to this
        // frame's layout, so no rubber band this frame.
        ui.spacing_mut().scroll.kinetic.rubber_band = false;
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
            prefetch: if zoomed {
                layout.window(view, 1, PREFETCH_VIEWPORTS)
            } else {
                layout.around(view, 1)
            },
            keep: if zoomed {
                layout.window(view, 2, KEEP_VIEWPORTS)
            } else {
                layout.around(view, 2)
            },
            zoomed,
            current_page: layout.current_page(view),
            want_text,
        }
    });
    st.offset = out.state.clamped_offset();
    st.view_rect = out.inner_rect;
    out.inner
}

/// Paints each visible page: shadow, white placeholder, best texture (rotated), border, search hits.
/// A page whose render failed shows a note instead of its texture.
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
        offset: [0, 4],
        blur: 18,
        spread: 0,
        color: pal.page_shadow,
    };
    for i in layout.visible_range(view) {
        let r = layout.rects()[i].translate(origin).round_to_pixels(ppp);
        painter.add(shadow.as_shape(r, 0.0));
        painter.rect_filled(r, 0.0, Color32::WHITE);
        if let Some((id, _)) = cache.get_best(i, wanted_px) {
            painter.add(page_mesh(id, r, layout.rotation()));
        } else if cache.failed(i, wanted_px) {
            painter.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                format!("Page {} could not be rendered", i + 1),
                TextStyle::Body.resolve(ui.style()),
                pal.error,
            );
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

/// The page texture `id` drawn into `r`, its corners rotated by `rotation`.
fn page_mesh(id: TextureId, r: Rect, rotation: Rotation) -> Shape {
    let corners = [
        r.left_top(),
        r.right_top(),
        r.left_bottom(),
        r.right_bottom(),
    ];
    let mut mesh = Mesh::with_texture(id);
    mesh.indices.extend_from_slice(&[0, 1, 2, 2, 1, 3]);
    for (pos, uv) in corners.into_iter().zip(rotation.uv_corners()) {
        mesh.vertices.push(Vertex {
            pos,
            uv,
            color: Color32::WHITE,
        });
    }
    Shape::mesh(mesh)
}

/// Pointer input on the canvas: a primary drag without Ctrl that starts on a page selects text, a
/// click clears the selection, a double click selects a word, a triple click a line, and the cursor
/// is a text cursor over chars. Hit tests run only when the pointer moved in doc space. Returns the
/// pages whose chars are needed.
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
    let clicks = if resp.triple_clicked_by(PointerButton::Primary) {
        3
    } else if resp.double_clicked_by(PointerButton::Primary) {
        2
    } else {
        0
    };
    if clicks > 0
        && let Some(p) = resp.interact_pointer_pos()
        && let Some(Probe::At(page, at)) = probe(layout, p - origin)
        && let Some(chars) = texts.get(&page)
        && let Some(i) = nearest(chars, at, ANCHOR_CAP)
    {
        let range = if clicks == 3 {
            line_at(chars, i)
        } else {
            word_at(chars, i)
        };
        sel.select(page, range);
        ui.memory_mut(egui::Memory::stop_text_input);
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
    use egui::pos2;

    use super::*;

    #[test]
    fn cancelled_fling_is_none() {
        let over = Some(pos2(10.0, 10.0));
        let mut st = ViewerState {
            view_rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(200.0, 200.0)),
            layout: PageLayout::new(
                &[PageSize { w: 100.0, h: 100.0 }; 2],
                1.0,
                Rotation::R0,
                200.0,
                24.0,
                32.0,
            ),
            ..ViewerState::default()
        };
        let cancels: [fn(&mut ViewerState); 7] = [
            |st| st.zoom_by(1.25, None),
            |st| st.rotate(true, 0),
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

    #[test]
    fn page_mesh_rotates_texture_corners() -> Result<(), String> {
        let r = Rect::from_min_max(pos2(0.0, 0.0), pos2(20.0, 10.0));
        let Shape::Mesh(mesh) = page_mesh(TextureId::default(), r, Rotation::R90) else {
            return Err("not a mesh".into());
        };
        assert_eq!(mesh.indices, [0, 1, 2, 2, 1, 3]);
        let uvs: Vec<Pos2> = mesh.vertices.iter().map(|v| v.uv).collect();
        assert_eq!(uvs, Rotation::R90.uv_corners());
        assert_eq!(mesh.vertices[3].pos, pos2(20.0, 10.0));
        Ok(())
    }

    /// One headless frame of `show` in a `size` canvas at ppp 1, painted at the zoom's bucket as
    /// app.rs does; returns the viewer output and whether egui asked for another frame.
    fn run(
        ctx: &egui::Context,
        st: &mut ViewerState,
        sizes: &[PageSize],
        size: Vec2,
        time: f64,
    ) -> (Option<ViewerOutput>, bool) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(time),
            ..Default::default()
        };
        let (cache, search, texts) = (
            PageCache::default(),
            SearchState::default(),
            PageTexts::default(),
        );
        let wanted_px = crate::layout::scale_bucket(st.zoom, 1.0);
        let mut viewer = None;
        let mut out = ctx.run_ui(input, |ui| {
            viewer = Some(show(ui, st, sizes, &cache, wanted_px, &search, &texts));
        });
        out.textures_delta.clear();
        let repaint = out
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .is_some_and(|v| v.repaint_delay.is_zero());
        (viewer, repaint)
    }

    /// One headless frame of `show` in a 900×1048 canvas; returns whether egui asked for another.
    fn frame(ctx: &egui::Context, st: &mut ViewerState, sizes: &[PageSize], time: f64) -> bool {
        run(ctx, st, sizes, egui::vec2(900.0, 1048.0), time).1
    }

    /// Fit width in a maximized 1080p window: A4 pages (2626 pt) are taller than 2 view heights
    /// (996 pt), yet prefetch and keep stay the page windows (±1 / ±2) at every scroll position,
    /// also one zoom step in (still fit width's render scale).
    #[test]
    fn fit_width_keeps_page_windows_in_a_maximized_window() -> Result<(), String> {
        let ctx = egui::Context::default();
        let sizes = [PageSize { w: 595.0, h: 842.0 }; 12];
        let mut st = ViewerState::default();
        let mut t = 0.0;
        let mut step = |st: &mut ViewerState| {
            t += 1.0 / 60.0;
            run(&ctx, st, &sizes, egui::vec2(1920.0, 996.0), t).0
        };
        step(&mut st);
        for zoom_steps in 0..2 {
            for _ in 0..90 {
                st.scroll_by(97.0);
                let out = step(&mut st).ok_or("no output")?;
                let v = &out.visible;
                let around = |k: usize| v.start.saturating_sub(k)..(v.end + k).min(sizes.len());
                let at = (zoom_steps, st.offset.y);
                assert!(!out.zoomed, "zoomed at {at:?}");
                assert_eq!(out.prefetch, around(1), "prefetch at {at:?}");
                assert_eq!(out.keep, around(2), "keep at {at:?}");
            }
            st.zoom_by(crate::layout::ZOOM_STEP, None);
            step(&mut st);
        }
        Ok(())
    }

    #[test]
    fn zoom_in_keeps_centre_without_sliding() {
        let ctx = egui::Context::default();
        let sizes = [PageSize { w: 612.0, h: 792.0 }; 20];
        let mut st = ViewerState::default();
        let mut t = 0.0;
        let mut step = |st: &mut ViewerState, n: usize| {
            let mut repaint = false;
            for _ in 0..n {
                t += 1.0 / 60.0;
                repaint = frame(&ctx, st, &sizes, t);
            }
            repaint
        };
        step(&mut st, 3);
        for _ in 0..2 {
            st.zoom_by(crate::layout::ZOOM_STEP, None);
            step(&mut st, 30);
        }
        let want = (st.layout.content_size().x - st.view_rect.width()) / 2.0;
        assert!(
            (st.offset.x - want).abs() < 0.5,
            "offset.x {} want {want}",
            st.offset.x
        );
        assert!(!step(&mut st, 1), "still repainting");
    }

    /// Zooming in at the end of a long document: the forced offset lies far past last frame's bottom
    /// edge, and egui's rubber band used to throw it back several pages over seconds.
    #[test]
    fn zoom_in_at_document_end_stays_put() {
        let ctx = egui::Context::default();
        let sizes = [PageSize { w: 612.0, h: 792.0 }; 30];
        let mut st = ViewerState::default();
        let mut t = 0.0;
        let mut step = |st: &mut ViewerState, n: usize| {
            let mut repaint = false;
            for _ in 0..n {
                t += 1.0 / 60.0;
                repaint = frame(&ctx, st, &sizes, t);
            }
            repaint
        };
        step(&mut st, 3);
        st.go_to_page(29);
        step(&mut st, 30);
        for _ in 0..4 {
            st.zoom_by(crate::layout::ZOOM_STEP, None);
            step(&mut st, 1);
            let placed = st.offset;
            assert!(!step(&mut st, 30), "still repainting");
            assert_eq!(st.offset, placed);
        }
    }
}
