//! Application state and the per-frame flow: messages, input, toolbar, canvas, renders.

use std::{collections::HashSet, ops::Range, path::PathBuf, time::Instant};

use crossbeam_channel::{Receiver, Sender};
use egui::{
    CentralPanel, Event, Frame, Key, Margin, Modifiers, MouseWheelUnit, Panel, RawInput, Shape,
    Stroke, TextStyle, TextureOptions, TouchPhase, Ui, Vec2, ViewportCommand, pos2, vec2,
};

use crate::{
    fling::VelocityTracker,
    input::pinch_wayland,
    layout::{
        PageSize, ZOOM_STEP, max_scale_px, scale_bucket, touchpad_points_per_pixel,
        wheel_notch_points, wheel_zoom_speed,
    },
    render::{
        cache::{PageCache, TexKey},
        pdfium_loader,
        worker::{self, Cmd, Msg},
    },
    search::SearchState,
    selection::PageTexts,
    ui::{
        theme::{self, PAGE_GAP, Palette},
        toolbar::{self, DocInfo, ToolbarAction, ToolbarState},
        viewer::{self, ViewerOutput, ViewerState},
    },
};

/// Arrow-key scroll step, in points.
const LINE_STEP: f32 = 48.0;
/// Empty-state page glyph size, and its gap above the text.
const GLYPH: Vec2 = vec2(28.0, 36.0);
const GLYPH_GAP: f32 = 8.0;

pub struct App {
    tx: Sender<Cmd>,
    rx: Receiver<Msg>,
    /// Pinch zoom factors (Wayland only).
    pinch: Option<Receiver<f32>>,
    doc: Option<Doc>,
    /// What the canvas shows while `doc` is `None`.
    status: Status,
    cache: PageCache,
    render_gen: u64,
    search: SearchState,
    toolbar: ToolbarState,
    viewer: ViewerState,
    current_page: usize,
    /// Chars of the pages loaded for text selection.
    texts: PageTexts,
    /// Pages whose chars were requested and have not arrived.
    text_pending: HashSet<usize>,
    /// Touchpad scroll velocity, for the fling on finger lift.
    wheel: VelocityTracker,
}

/// The open document as the UI knows it.
struct Doc {
    name: String,
    sizes: Vec<PageSize>,
    /// Largest render scale whose textures fit the GPU limit.
    max_px: f32,
}

enum Status {
    Empty,
    Loading,
    Error(String),
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, path: Option<PathBuf>) -> Self {
        let ctx = cc.egui_ctx.clone();
        theme::apply(&ctx);
        // egui would otherwise eat Ctrl+= / Ctrl+- / Ctrl+0 as UI scaling.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let (tx, rx) = worker::spawn(ctx.clone(), pdfium_loader::default_candidates());
        let mut app = Self {
            tx,
            rx,
            pinch: pinch_wayland::start(cc, ctx),
            doc: None,
            status: Status::Empty,
            cache: PageCache::default(),
            render_gen: 0,
            search: SearchState::default(),
            toolbar: ToolbarState::default(),
            viewer: ViewerState::default(),
            current_page: 0,
            texts: PageTexts::default(),
            text_pending: HashSet::new(),
            wheel: VelocityTracker::default(),
        };
        if let Some(path) = path {
            app.open(path);
        }
        app
    }

    fn open(&mut self, path: PathBuf) {
        self.doc = None;
        self.status = Status::Loading;
        self.send(Cmd::Open(path));
    }

    /// The worker only stops when the UI is gone, so a failed send needs no handling.
    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    fn handle_msg(&mut self, ctx: &egui::Context, msg: Msg) {
        match msg {
            Msg::Opened { name, sizes } => {
                let max_px = max_scale_px(&sizes, ctx.input(|i| i.max_texture_side));
                ctx.send_viewport_cmd(ViewportCommand::Title(format!("{name} — Paperlight")));
                self.doc = Some(Doc {
                    name,
                    sizes,
                    max_px,
                });
                self.status = Status::Empty;
                self.forget_doc();
            }
            Msg::OpenFailed(e) | Msg::LibraryMissing(e) => {
                self.doc = None;
                self.status = Status::Error(e);
                self.forget_doc();
                ctx.send_viewport_cmd(ViewportCommand::Title("Paperlight".into()));
            }
            Msg::Page {
                page,
                scale_px,
                image,
            } => {
                if self.doc.is_some() {
                    let tex =
                        ctx.load_texture(format!("page-{page}"), image, TextureOptions::LINEAR);
                    self.cache.insert(TexKey::new(page, scale_px), tex);
                }
            }
            Msg::SearchDone { generation, hits } => {
                if self.search.set_results(generation, hits, self.current_page) {
                    self.reveal_current_hit();
                }
            }
            Msg::Text { page, chars } => {
                self.text_pending.remove(&page);
                if self.doc.is_some() {
                    self.texts.insert(page, chars.into());
                    self.viewer.text_loaded(&self.texts);
                }
            }
        }
    }

    /// Drops the previous document's textures, search, page number, scroll and zoom.
    fn forget_doc(&mut self) {
        self.current_page = 0;
        self.cache.clear();
        self.search.clear();
        self.toolbar.search_text.clear();
        self.viewer.reset();
        self.texts.clear();
        self.text_pending.clear();
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let free = ctx.memory(|m| m.focused().is_none());
        let pressed = |modifiers, key| ctx.input_mut(|i| i.consume_key(modifiers, key));
        if pressed(Modifiers::COMMAND, Key::F) {
            self.toolbar.focus_search = true;
        }
        let has_query = !self.search.query.is_empty();
        let has_selection = !self.viewer.selection().is_empty();
        if free && (has_query || has_selection) && pressed(Modifiers::NONE, Key::Escape) {
            if has_query {
                self.close_search();
            }
            self.viewer.clear_selection();
        }
        // egui-winit turns Ctrl+C into `Event::Copy`, not a key event.
        if free
            && ctx.input(|i| i.events.iter().any(|e| matches!(e, Event::Copy)))
            && let Some(text) = self.viewer.selection().text(&self.texts)
        {
            ctx.copy_text(text);
        }
        let Some(last) = self.doc.as_ref().map(|d| d.sizes.len().saturating_sub(1)) else {
            return;
        };
        if pressed(Modifiers::COMMAND, Key::Equals) || pressed(Modifiers::COMMAND, Key::Plus) {
            self.viewer.zoom_by(ZOOM_STEP, None);
        }
        if pressed(Modifiers::COMMAND, Key::Minus) {
            self.viewer.zoom_by(1.0 / ZOOM_STEP, None);
        }
        if pressed(Modifiers::COMMAND, Key::Num0) {
            self.viewer.fit_width();
        }
        if !free {
            return;
        }
        let screen = self.viewer.view_height() - PAGE_GAP;
        for (key, dy) in [
            (Key::PageDown, screen),
            (Key::PageUp, -screen),
            (Key::ArrowDown, LINE_STEP),
            (Key::ArrowUp, -LINE_STEP),
        ] {
            if pressed(Modifiers::NONE, key) {
                self.viewer.scroll_by(dy);
            }
        }
        for (key, page) in [
            (Key::ArrowRight, self.viewer.step_page(true)),
            (Key::ArrowLeft, self.viewer.step_page(false)),
            (Key::Home, 0),
            (Key::End, last),
        ] {
            if pressed(Modifiers::NONE, key) {
                self.viewer.go_to_page(page);
            }
        }
    }

    fn handle_toolbar(&mut self, action: ToolbarAction) {
        match action {
            ToolbarAction::GoToPage(page) => self.viewer.go_to_page(page),
            ToolbarAction::ZoomIn => self.viewer.zoom_by(ZOOM_STEP, None),
            ToolbarAction::ZoomOut => self.viewer.zoom_by(1.0 / ZOOM_STEP, None),
            ToolbarAction::FitWidth => self.viewer.fit_width(),
            ToolbarAction::SearchChanged(query) => {
                let generation = self.search.start(&query);
                self.send(Cmd::Search { query, generation });
            }
            ToolbarAction::SearchNext => {
                self.search.next();
                self.reveal_current_hit();
            }
            ToolbarAction::SearchPrev => {
                self.search.prev();
                self.reveal_current_hit();
            }
            ToolbarAction::SearchClosed => self.close_search(),
        }
    }

    /// Clears the query and hits and cancels any search in progress.
    fn close_search(&mut self) {
        let generation = self.search.clear();
        self.toolbar.search_text.clear();
        self.send(Cmd::Search {
            query: String::new(),
            generation,
        });
    }

    fn reveal_current_hit(&mut self) {
        if let Some(hit) = self.search.current_hit()
            && let Some(rect) = hit.rects.first()
        {
            self.viewer.reveal(hit.page, *rect);
        }
    }

    /// Requests the chars of `pages` not yet loaded or in flight.
    fn request_texts(&mut self, pages: Range<usize>) {
        for page in pages {
            if !self.texts.contains_key(&page) && self.text_pending.insert(page) {
                self.send(Cmd::Text { page });
            }
        }
    }

    /// Requests textures for the visible pages ±1 at `wanted` px per pt. While a zoom is
    /// unsettled, only visible pages with no texture at all are requested.
    fn request_renders(
        &mut self,
        ctx: &egui::Context,
        out: &ViewerOutput,
        wanted: f32,
        pages: usize,
    ) {
        let settle = self.viewer.settle_remaining(Instant::now());
        let settled = settle.is_none();
        let lo = out.visible.start.saturating_sub(1);
        let hi = (out.visible.end + 1).min(pages);
        let missing: Vec<TexKey> = (lo..hi)
            .filter(|&p| match self.cache.get_best(p, wanted) {
                Some((_, true)) => false,
                Some((_, false)) => settled,
                None => settled || out.visible.contains(&p),
            })
            .map(|p| TexKey::new(p, wanted))
            .collect();
        if let Some((new_generation, keys)) = self.cache.start_batch(&missing) {
            if new_generation {
                self.render_gen += 1;
            }
            for key in keys {
                self.send(Cmd::Render {
                    page: key.page,
                    scale_px: wanted,
                    generation: self.render_gen,
                });
            }
        }
        if let Some(delay) = settle {
            ctx.request_repaint_after(delay);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        while let Ok(msg) = self.rx.try_recv() {
            self.handle_msg(&ctx, msg);
        }
        // Drained even without a document, so a pinch on the empty window is not applied later.
        let pinch: f32 = self.pinch.iter().flat_map(Receiver::try_iter).product();
        if self.doc.is_some() {
            let (wheel_or_touch, hover) = ctx.input(|i| (i.zoom_delta(), i.pointer.hover_pos()));
            let factor = wheel_or_touch * pinch;
            if (factor - 1.0).abs() > 1e-4 {
                self.viewer.zoom_by(factor, hover);
            }
        }
        self.handle_keys(&ctx);
        if let Some(path) =
            ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()))
        {
            self.open(path);
        }

        let pal = theme::palette(ui.visuals().dark_mode);
        let info = DocInfo {
            name: self.doc.as_ref().map(|d| d.name.as_str()),
            current_page: self.current_page,
            total_pages: self.doc.as_ref().map_or(0, |d| d.sizes.len()),
            zoom: self.viewer.zoom(),
            search: &self.search,
        };
        let toolbar = Panel::top("toolbar")
            .exact_size(theme::TOOLBAR_HEIGHT)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(pal.toolbar_bg)
                    .inner_margin(Margin::symmetric(12, 8)),
            )
            .show(ui, |ui| toolbar::show(ui, &mut self.toolbar, &info))
            .inner;
        if let Some(action) = toolbar {
            self.handle_toolbar(action);
        }

        let ppp = ctx.pixels_per_point();
        let canvas = CentralPanel::default().frame(Frame::new().fill(pal.canvas_bg));
        let Some(doc) = &self.doc else {
            canvas.show(ui, |ui| paint_status(ui, &self.status, pal));
            return;
        };
        let wanted = |zoom| scale_bucket(zoom, ppp).min(doc.max_px);
        let out = canvas
            .show(ui, |ui| {
                let painted = wanted(self.viewer.zoom());
                viewer::show(
                    ui,
                    &mut self.viewer,
                    &doc.sizes,
                    &self.cache,
                    painted,
                    &self.search,
                    &self.texts,
                )
            })
            .inner;
        if out.current_page != self.current_page {
            // The toolbar was drawn before the canvas. Without this, a scroll that no input
            // event follows (a search reveal) left the old page number on screen.
            ctx.request_repaint();
        }
        self.current_page = out.current_page;
        let notch = wheel_notch_points(self.viewer.view_height());
        ctx.options_mut(|o| {
            o.input_options.line_scroll_speed = notch;
            o.input_options.scroll_zoom_speed = wheel_zoom_speed(notch);
        });
        // After `viewer::show`, so a fit-width zoom set this frame requests the right bucket.
        let wanted = wanted(self.viewer.zoom());
        let pages = doc.sizes.len();
        self.request_renders(&ctx, &out, wanted, pages);
        self.request_texts(out.want_text);
        self.cache.retain(out.visible, wanted);
    }

    /// Touchpad scrolling the GTK 3 (Evince) way, which egui lacks on Linux: pixel deltas scale
    /// to `page_size^(2/3) / 10` points per pixel, and their velocity flings on finger lift.
    /// Ctrl+touchpad (zoom) is left alone. Any wheel event cancels a fling.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut RawInput) {
        let time = raw_input.time.unwrap_or_default();
        let per_pixel = touchpad_points_per_pixel(self.viewer.view_height());
        for event in &mut raw_input.events {
            let Event::MouseWheel {
                unit,
                delta,
                phase,
                modifiers,
                ..
            } = event
            else {
                continue;
            };
            self.viewer.cancel_fling();
            if *unit != MouseWheelUnit::Point || modifiers.ctrl {
                continue;
            }
            *delta *= per_pixel;
            match phase {
                TouchPhase::Start => self.wheel.start(time),
                // The offset moves against the content.
                TouchPhase::Move => self.wheel.add(-delta.y, time),
                TouchPhase::End => {
                    if let Some(v) = self.wheel.end(time) {
                        let pointer = ctx.input(|i| i.pointer.latest_pos());
                        self.viewer.fling(v, time, pointer);
                    }
                }
                TouchPhase::Cancel => self.wheel = VelocityTracker::default(),
            }
        }
    }

    /// The pinch thread borrows winit's `wl_display`; exit before winit disconnects it.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        std::process::exit(0)
    }
}

/// Empty, loading or error state, centred in the canvas.
fn paint_status(ui: &Ui, status: &Status, pal: &Palette) {
    let (text, color, icon) = match status {
        Status::Empty => ("Drop a PDF here", pal.text_secondary, true),
        Status::Loading => ("Opening…", pal.text_secondary, false),
        Status::Error(e) => (e.as_str(), pal.error, false),
    };
    let painter = ui.painter();
    let font = TextStyle::Body.resolve(ui.style());
    let galley = painter.layout(text.to_owned(), font, color, 420.0);
    let icon_h = if icon { GLYPH.y + GLYPH_GAP } else { 0.0 };
    let centre = ui.max_rect().center();
    let block_h = icon_h + galley.size().y;
    let top = centre.y - block_h / 2.0;
    if icon {
        paint_page_glyph(
            painter,
            pos2(centre.x - GLYPH.x / 2.0, top),
            Stroke::new(1.5, pal.text_secondary),
        );
    }
    painter.galley(
        pos2(centre.x - galley.size().x / 2.0, top + icon_h),
        galley,
        color,
    );
}

/// Page outline with a folded top-right corner.
fn paint_page_glyph(painter: &egui::Painter, min: egui::Pos2, stroke: Stroke) {
    const FOLD: f32 = 8.0;
    let r = egui::Rect::from_min_size(min, GLYPH);
    let (fold_top, fold_side) = (
        pos2(r.right() - FOLD, r.top()),
        pos2(r.right(), r.top() + FOLD),
    );
    painter.add(Shape::closed_line(
        vec![
            r.left_top(),
            fold_top,
            fold_side,
            r.right_bottom(),
            r.left_bottom(),
        ],
        stroke,
    ));
    painter.add(Shape::line(
        vec![fold_top, pos2(fold_top.x, fold_side.y), fold_side],
        stroke,
    ));
}
