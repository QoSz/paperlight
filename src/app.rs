//! Application state and the per-frame flow: messages, input, toolbar, canvas, renders.

use std::{
    collections::HashSet,
    ops::Range,
    path::PathBuf,
    sync::mpsc::{Receiver, Sender},
    time::Instant,
};

use egui::{
    Align, CentralPanel, Event, Frame, Key, Layout, Margin, Modifiers, MouseWheelUnit, Panel,
    RawInput, Rect, Shape, Stroke, TextEdit, TextStyle, TextureOptions, TouchPhase, Ui, UiBuilder,
    Vec2, ViewportCommand, pos2, vec2,
};

use crate::{
    fling::VelocityTracker,
    input::pinch_wayland,
    layout::{
        PageSize, ZOOM_STEP, max_scale_px, scale_bucket, touchpad_points_per_pixel, visible_first,
        wheel_notch_points, wheel_zoom_speed,
    },
    render::{
        cache::{PageCache, TexKey},
        pdfium_loader,
        worker::{self, Cmd, Msg},
    },
    search::SearchState,
    selection::PageTexts,
    system_theme,
    ui::{
        chrome, controls,
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
/// Width of the password field.
const PASSWORD_WIDTH: f32 = 240.0;

pub struct App {
    tx: Sender<Cmd>,
    rx: Receiver<Msg>,
    /// Pinch zoom factors (Wayland only).
    pinch: Option<Receiver<f32>>,
    doc: Option<Doc>,
    /// What the canvas shows while `doc` is `None`.
    status: Status,
    cache: PageCache,
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
    /// `path` is encrypted: the canvas shows a password field. `wrong` after a rejected try.
    Password {
        path: PathBuf,
        wrong: bool,
        text: String,
    },
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, path: Option<PathBuf>) -> Self {
        let ctx = cc.egui_ctx.clone();
        theme::apply(&ctx);
        // egui would otherwise eat Ctrl+= / Ctrl+- / Ctrl+0 as UI scaling.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let (tx, rx) = worker::spawn(ctx.clone(), pdfium_loader::default_candidates());
        system_theme::watch(&ctx);
        let mut app = Self {
            tx,
            rx,
            pinch: pinch_wayland::start(cc, ctx),
            doc: None,
            status: Status::Empty,
            cache: PageCache::default(),
            search: SearchState::default(),
            toolbar: ToolbarState::default(),
            viewer: ViewerState::default(),
            current_page: 0,
            texts: PageTexts::default(),
            text_pending: HashSet::new(),
            wheel: VelocityTracker::default(),
        };
        if let Some(path) = path {
            app.open(path, None);
        }
        app
    }

    fn open(&mut self, path: PathBuf, password: Option<String>) {
        self.doc = None;
        // The old document's textures are never drawn again.
        self.cache.clear();
        self.status = Status::Loading;
        self.send(Cmd::Open { path, password });
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
            Msg::OpenFailed(e) | Msg::LibraryMissing(e) => self.no_doc(ctx, Status::Error(e)),
            Msg::NeedsPassword { path, wrong } => {
                let text = String::new();
                self.no_doc(ctx, Status::Password { path, wrong, text });
            }
            Msg::Page {
                page,
                scale_px,
                image,
            } => {
                // A render queued for the previous document arrives at its scale, which can exceed
                // this document's GPU limit (egui_glow asserts).
                if self.doc.as_ref().is_some_and(|d| scale_px <= d.max_px) {
                    let tex =
                        ctx.load_texture(format!("page-{page}"), image, TextureOptions::LINEAR);
                    self.cache.insert(TexKey::new(page, scale_px), tex);
                }
            }
            Msg::RenderFailed { page, scale_px } => {
                self.cache.fail(TexKey::new(page, scale_px));
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

    /// No document after an open: `status` fills the canvas.
    fn no_doc(&mut self, ctx: &egui::Context, status: Status) {
        self.doc = None;
        self.status = status;
        self.forget_doc();
        ctx.send_viewport_cmd(ViewportCommand::Title("Paperlight".into()));
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
        if pressed(Modifiers::COMMAND, Key::W) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
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
        for (key, clockwise) in [(Key::ArrowRight, true), (Key::ArrowLeft, false)] {
            if pressed(Modifiers::COMMAND, key) {
                self.viewer.rotate(clockwise, self.current_page);
            }
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
            ToolbarAction::StepPage(forward) => {
                self.viewer.go_to_page(self.viewer.step_page(forward));
            }
            ToolbarAction::ZoomIn => self.viewer.zoom_by(ZOOM_STEP, None),
            ToolbarAction::ZoomOut => self.viewer.zoom_by(1.0 / ZOOM_STEP, None),
            ToolbarAction::FitWidth => self.viewer.fit_width(),
            ToolbarAction::RotateClockwise => self.viewer.rotate(true, self.current_page),
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

    /// Requests the chars of the visible pages that show something at `wanted_px` (a texture or
    /// its failure note) and of `wanted`, if not loaded or in flight, and drops the chars of pages
    /// far from the viewport unless they are part of the selection.
    fn request_texts(&mut self, visible: &Range<usize>, wanted: Range<usize>, wanted_px: f32) {
        let painted = visible
            .clone()
            .filter(|&p| self.cache.painted(p, wanted_px));
        for page in painted.chain(wanted) {
            if !self.texts.contains_key(&page) && self.text_pending.insert(page) {
                self.send(Cmd::Text { page });
            }
        }
        let keep = visible.start.saturating_sub(2)..visible.end + 2;
        let selected = self.viewer.selection().pages();
        self.texts
            .retain(|page, _| keep.contains(page) || selected.contains(page));
    }

    /// Requests textures for `out.prefetch` (the visible pages ±1 near the viewport) at `wanted`
    /// px per pt, visible pages first. While a zoom is unsettled, only visible pages with no texture
    /// at all are requested.
    fn request_renders(&mut self, ctx: &egui::Context, out: &ViewerOutput, wanted: f32) {
        let settle = self.viewer.settle_remaining(Instant::now());
        let settled = settle.is_none();
        let missing: Vec<TexKey> = visible_first(out.visible.clone(), out.prefetch.clone())
            .filter(|&p| !self.cache.failed(p, wanted))
            .filter(|&p| match self.cache.get_best(p, wanted) {
                Some((_, true)) => false,
                Some((_, false)) => settled,
                None => settled || out.visible.contains(&p),
            })
            .map(|p| TexKey::new(p, wanted))
            .collect();
        if let Some((generation, keys)) = self.cache.start_batch(&missing) {
            for key in keys {
                self.send(Cmd::Render {
                    page: key.page,
                    scale_px: wanted,
                    generation,
                    zoomed: out.zoomed,
                });
            }
        }
        if let Some(delay) = settle {
            ctx.request_repaint_after(delay);
        }
    }

    /// One frame: messages, input, the title bar and the canvas.
    fn show(&mut self, ui: &mut Ui) {
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
            self.open(path, None);
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
                    .inner_margin(Margin::symmetric(16, 12)),
            )
            .show(ui, |ui| toolbar::show(ui, &mut self.toolbar, &info))
            .inner;
        if let Some(action) = toolbar {
            self.handle_toolbar(action);
        }

        let ppp = ctx.pixels_per_point();
        let canvas = CentralPanel::default().frame(Frame::new().fill(pal.canvas_bg));
        let Some(doc) = &self.doc else {
            let submit = canvas
                .show(ui, |ui| paint_status(ui, &mut self.status, pal))
                .inner;
            if let Some((path, password)) = submit {
                self.open(path, Some(password));
            }
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
        self.request_renders(&ctx, &out, wanted);
        self.request_texts(&out.visible, out.want_text, wanted);
        self.cache.retain(out.keep, out.visible, wanted);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
        // After the panels, so the resize bands sit on top of them.
        chrome::window_edges(ui);
    }

    /// Touchpad scrolling the GTK 3 (Evince) way, which egui lacks on Linux: pixel deltas scale
    /// to `page_size^(2/3) / 10` points per pixel, and their velocity flings on finger lift.
    /// Ctrl+touchpad (zoom) is left alone. Any wheel event cancels a fling.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut RawInput) {
        chrome::release_after_grab(ctx, raw_input);
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

/// Empty, loading, error or password state, centred in the canvas. Returns the path and
/// password to open when the password field is submitted.
fn paint_status(ui: &mut Ui, status: &mut Status, pal: &Palette) -> Option<(PathBuf, String)> {
    let (text, color, icon) = match status {
        Status::Empty => ("Drop a PDF here".to_owned(), pal.text_secondary, true),
        Status::Loading => ("Opening…".to_owned(), pal.text_secondary, false),
        Status::Error(e) => (e.clone(), pal.error, false),
        Status::Password { path, wrong, text } => {
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            return password_form(ui, &name, *wrong, text, pal)
                .then(|| (path.clone(), text.clone()));
        }
    };
    let painter = ui.painter();
    let font = TextStyle::Body.resolve(ui.style());
    let galley = painter.layout(text, font, color, 420.0);
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
    None
}

/// "name is password-protected", a password field (focused) and, after a rejected try, "Wrong
/// password". Returns `true` when Enter submits a non-empty password.
fn password_form(ui: &mut Ui, name: &str, wrong: bool, text: &mut String, pal: &Palette) -> bool {
    let area = Rect::from_center_size(ui.max_rect().center(), vec2(420.0, 120.0));
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(area)
            .layout(Layout::top_down(Align::Center)),
        |ui| {
            ui.label(format!("{name} is password-protected"));
            ui.add_space(GLYPH_GAP);
            let field = controls::field(
                ui,
                TextEdit::singleline(text)
                    .password(true)
                    .hint_text("Password")
                    .desired_width(PASSWORD_WIDTH),
                None,
            );
            let submit =
                field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !text.is_empty();
            // Enter drops the focus; refocusing then would hide `lost_focus`.
            if !submit && ui.memory(|m| m.focused().is_none()) {
                field.request_focus();
            }
            if wrong {
                ui.add_space(GLYPH_GAP);
                ui.colored_label(pal.error, "Wrong password");
            }
            submit
        },
    )
    .inner
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
