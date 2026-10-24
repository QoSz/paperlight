# Paperlight: architecture and implementation contract

Phase 5 contract, kept as written. Later rounds (kinetic scrolling, text selection, rotation,
password prompt, system theme) are recorded in `implementation-notes.md`; where the two differ,
the code and the notes are current.

This document is binding for phase 5 (Implementation). Sources, in priority order:

1. The plan's **Decisions** table (`~/.claude/plans/goal-create-for-me-purring-lobster.md`). It is fixed.
2. `docs/api-notes.md`. Its API facts are verified.
3. `docs/design.md` and `src/ui/theme.rs`. The Design agent owns them. Do not edit them.

Anything in this document that differs from the plan's prose (not the Decisions table) is deliberate. Section 0.2 lists each difference and its reason.

---

## 0. Global rules

### 0.1 Conventions
- **Edition 2024 reserves `gen` as a keyword.** Name every generation field and variable `generation`.
- **`egui::Layout` already exists.** Our page geometry type is `PageLayout`.
- **Lints** (`Cargo.toml`): `unwrap_used`, `panic` and `unsafe_op_in_unsafe_fn` are deny. `expect_used` and pedantic are warn. Write code with zero warnings:
  - Use `?`, `let … else`, `unwrap_or*` and `if let`.
  - Convert numbers with `try_from(..).unwrap_or(..)`. Where a lossy cast is intended, put `#[expect(clippy::cast_precision_loss, reason = "…")]` (or the matching cast lint) on the smallest function.
  - Never compare floats with `==` (`clippy::float_cmp`). Use `to_bits()` or an epsilon.
- **Units**:
  - **pt**: PDF point. At zoom 1, 1 pt = 1 egui point.
  - **doc space**: ScrollArea content coordinates, in egui points, with the origin at the content's top-left.
  - **px**: physical pixels.
  - **`zoom`**: egui points per pt.
  - **`scale_px`**: px per pt that is actually rendered, `= min(scale_bucket(zoom, ppp), doc.max_px)`.
- **Threads**: the UI thread (eframe), `pdf-worker` (owns PDFium) and `pinch` (Wayland only, optional). Only owned data crosses threads. Every channel is `crossbeam_channel::unbounded()`.
- **Repaint discipline**: never repaint continuously. The UI wakes for exactly four reasons:
  1. Input.
  2. A worker `Msg` (the worker calls `request_repaint`).
  3. A pinch event (the pinch thread calls `request_repaint`).
  4. The zoom-settle timer (`request_repaint_after`).
- **Logging**: `tracing`. The env filter comes from `PAPERLIGHT_LOG` (default `warn`). Render latency is logged at `debug`.

### 0.2 Deliberate refinements to the requested interfaces
| Requested | Contract | Why |
|---|---|---|
| `zoom_about(anchor_doc, old_scale, new_scale, offset)` | `zoom_about(old: &PageLayout, new: &PageLayout, anchor_in_view, old_offset)` | `PAGE_GAP`/`PAGE_MARGIN` stay constant in points and pages are centred, so doc positions are **not linear** in scale. The linear formula drifts by `page_index × gap × (k−1)`: 120 pt per 1.1× step at page 50. Mapping through page-local coordinates is exact. |
| `gen` fields | `generation` | `gen` is reserved in edition 2024. |
| `Msg::Page{.., gen, rgba, w, h}` | `Msg::Page{page, scale_px, image: egui::ColorImage}` | The RGBA→`Color32` conversion (up to ~30 ms at 4×ppp) moves off the UI thread. The UI thread only calls `load_texture` (moves the image). `generation` is unused because doc identity is guaranteed by FIFO ordering (see A3). |
| `spawn(ctx, lib_path: Option<PathBuf>)` | `spawn(ctx, candidates: Vec<PathBuf>)` | The loader returns an ordered list. The worker tries each one, then the system library. |
| `get_best -> (TextureId, size, is_exact)` | `get_best -> (TextureId, is_exact)` | The draw rect comes from the layout, so the texture size is never needed. |
| `PageCache.best` map | none (scan) | The cache holds 15 entries or fewer, so a linear scan is simpler and fast. |
| `ToolbarState.search_open` | removed | `design.md`: the search field is always visible. |
| `eframe::App::update()` | `fn ui(&mut self, ui: &mut egui::Ui, frame)` | eframe 0.37's `App` trait has `ui` (required) and `logic` (optional). `update` does not exist (`epi.rs:152-185`). |
| texture mipmaps (api-notes suggestion) | `TextureOptions::LINEAR`, no mipmaps | Buckets are ≥ the request, so steady-state minification is at most 1.5×. Trilinear at 1.25× blends 32% of the half-res mip in, which makes text visibly soft. Bilinear stays crisp. |
| main.rs icon decode | `eframe::icon_data::from_png_bytes` | eframe 0.37 already depends on `image` (features `["png"]`, non-wasm, **not optional**: `eframe-0.37.0/Cargo.toml:266-269`). `icon_data` has no feature gate. It adds 0 new deps. |

### 0.3 Module tree and line budget (non-test Rust; budget ≤ 1400)
```
src/main.rs                      40   argv, tracing, viewport, run_native, exit
src/app.rs                      220   App, ui(): msgs → pinch → keys → toolbar → viewer → renders → retain
src/layout.rs                   140   PURE geometry + zoom math + parsing
src/search.rs                    70   PURE search state
src/render/mod.rs                 3   pub mod cache; pub mod pdfium_loader; pub mod worker;
src/render/worker.rs            200   pdf-worker thread
src/render/cache.rs              70   textures + in_flight
src/render/pdfium_loader.rs      30   candidate library paths
src/input/mod.rs                  1   pub mod pinch_wayland;
src/input/pinch_wayland.rs      120   zwp_pointer_gestures_v1 on winit's wl_display
src/ui/mod.rs                     3   pub mod theme; pub mod toolbar; pub mod viewer;
src/ui/toolbar.rs               130
src/ui/viewer.rs                170
src/ui/theme.rs               (~150, Design agent; do not touch)
                               ----
                              ~1350
```

---

## A. Module interfaces

### A1. `src/layout.rs` (pure, no egui state, only egui math types)

```rust
use std::ops::Range;
use egui::{Rect, Vec2};

/// Smallest and largest zoom (egui points per PDF point).
pub const MIN_ZOOM: f32 = 0.25;
pub const MAX_ZOOM: f32 = 6.0;
/// Multiplicative step for Ctrl+= / Ctrl+- and the toolbar buttons.
pub const ZOOM_STEP: f32 = 1.25;
/// Render scales, as multiples of pixels-per-point.
const BUCKETS: [f32; 8] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0];

/// Page size in PDF points, /Rotate already applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSize { pub w: f32, pub h: f32 }

/// Page rectangles in doc space for one zoom and viewport width.
#[derive(Debug, Clone, Default)]
pub struct PageLayout { rects: Vec<Rect>, content: Vec2, scale: f32, gap: f32 }

impl PageLayout {
    /// Stacks pages top to bottom (`margin` above the first and below the last, `gap` between),
    /// each centred in `content.x = max(viewport_w, widest·scale + 2·margin)`.
    pub fn new(sizes: &[PageSize], scale: f32, viewport_w: f32, gap: f32, margin: f32) -> Self;
    /// Zoom this layout was built for.
    pub fn scale(&self) -> f32;
    /// Total scrollable size; `(viewport_w, 0)` when there are no pages.
    pub fn content_size(&self) -> Vec2;
    /// All page rects in doc space, indexed by page.
    pub fn rects(&self) -> &[Rect];
    /// Pages whose rect intersects `view` vertically (two `partition_point`s); empty range in gaps.
    pub fn visible_range(&self, view: Rect) -> Range<usize>;
    /// Page under the viewport centre; 0 if `view.min.y <= 0`, last if `view.max.y >= content.y - 0.5`.
    pub fn current_page(&self, view: Rect) -> usize;
    /// Scroll y that shows `page` with `gap/2` above it; 0 for page 0.
    pub fn page_scroll_y(&self, page: usize) -> f32;
    /// Maps a rect in page points (top-left origin) of `page` to doc space.
    pub fn to_doc(&self, page: usize, r: Rect) -> Rect;
    // private: fn page_at_y(&self, y: f32) -> usize  // partition_point(|r| r.max.y < y).min(len-1)
}

/// Zoom that fits the widest page in `viewport_w - 2·margin`, clamped; 1.0 if `sizes` is empty.
pub fn fit_width_scale(sizes: &[PageSize], viewport_w: f32, margin: f32) -> f32;
/// Clamps to `MIN_ZOOM..=MAX_ZOOM`.
pub fn clamp_zoom(zoom: f32) -> f32;
/// New scroll offset that keeps the doc point under `anchor_in_view` (relative to the viewport's
/// top-left) fixed across a relayout. It maps through page-local points of the page at that y
/// (`page_at_y`; gaps and margins extrapolate linearly). Not clamped. Returns `old_offset` if
/// either layout is empty or the page counts differ.
pub fn zoom_about(old: &PageLayout, new: &PageLayout, anchor_in_view: Vec2, old_offset: Vec2) -> Vec2;
/// Clamps a scroll offset to `0..=max(content - view, 0)` per axis (egui 0.37 rubber-bands forced offsets otherwise).
pub fn clamp_offset(offset: Vec2, content: Vec2, view: Vec2) -> Vec2;
/// Per axis: keep `view.min` if `target` fits inside `view` on that axis, else centre `target`.
/// Returns `None` when nothing changes. Both rects are in doc space. Not clamped.
pub fn reveal_offset(view: Rect, target: Rect) -> Option<Vec2>;
/// Smallest bucket `b >= zoom - 1e-3` (else 4.0), times `ppp`. Never below the requested size, so never blurry; capped at 4·ppp.
pub fn scale_bucket(zoom: f32, ppp: f32) -> f32;
/// Largest `scale_px` keeping every page's longer side under the GPU limit: `(max_side - 1) / max(w,h)`;
/// `f32::INFINITY` if `sizes` is empty. egui_glow asserts on oversize textures, so this guard is mandatory.
pub fn max_scale_px(sizes: &[PageSize], max_texture_side: usize) -> f32;
/// 1-based user text to a 0-based page index; trims whitespace; `None` if not in `1..=total`.
pub fn parse_page_input(s: &str, total: usize) -> Option<usize>;
```

**Tests** (`#[cfg(test)] mod tests`, written first). Fixture **F** = `PageLayout::new(&[PageSize{w:600.,h:800.}; 3], 1.0, 800.0, 20.0, 30.0)`. It gives rects x 100..700 and y `[30..830]`, `[850..1650]`, `[1670..2470]`, with content `(800, 2500)`. In the table, "view y a..b" means `Rect::from_min_max(pos2(0,a), pos2(800,b))`.

| test | input | expected |
|---|---|---|
| `new_centres_and_stacks` | F | rects as above; `content_size() == (800, 2500)` |
| `new_widens_content_for_wide_page` | `[600×800]`, scale 1, vw 500, gap 20, margin 30 | content.x 660; rect x 30..630 |
| `new_scales_pages` | F at scale 2 | rect0 = (30,30)–(1230,1630); content (1260, 4900) |
| `new_empty` | `[]`, vw 800 | `rects()` empty; content (800, 0) |
| `visible_range_spans_gap` | F, view y 800..900 | `0..2` |
| `visible_range_gap_only` | F, view y 835..845 | `1..1` |
| `visible_range_past_end` | F, view y 3000..3500 | `3..3` |
| `visible_range_empty_layout` | `PageLayout::default()` | `0..0` |
| `current_page_centre` | F, view y 600..1400 | 1 |
| `current_page_top_is_first` | F, view y 0..2000 | 0 |
| `current_page_bottom_is_last` | F, view y 700..2500 | 2 |
| `current_page_gap_picks_next` | F, view y 440..1240 (centre 840) | 1 |
| `page_scroll_y_values` | F | p0 → 0.0, p1 → 840.0, p2 → 1660.0 |
| `to_doc_maps_page_points` | F at scale 2, `to_doc(0, (10,20)–(30,40))` | (50,70)–(90,110) |
| `fit_width_basic` | `[612×792]`, 676, 32 | 1.0 |
| `fit_width_uses_widest` | `[600×800, 1200×800]`, 664, 32 | 0.5 |
| `fit_width_empty_is_one` | `[]`, 800, 32 | 1.0 |
| `fit_width_clamped` | `[100×100]`, 10000, 32 → 6.0; `[612×792]`, 100, 32 → 0.25 | as stated |
| `clamp_zoom_bounds` | 0.1 / 10.0 / 1.5 | 0.25 / 6.0 / 1.5 |
| `zoom_about_keeps_point_fixed` | old F (scale 1), new F at scale 2, anchor (400,300), offset (0,900) | (230, 2050) |
| `zoom_about_in_gap` | same layouts, anchor (400,300), offset (0,540) | (230, 1330) |
| `zoom_about_empty_returns_old` | default layouts | old offset |
| `clamp_offset_bounds` | (−5, 5000), content (800,2500), view (800,600) → (0,1900); content (500,500), view (800,600) → (0,0) | as stated |
| `reveal_offset_none_when_visible` | view (0,0)–(800,600), target (100,100)–(200,120) | `None` |
| `reveal_offset_centres_hidden_target` | same view, target (100,1000)–(200,1020) | `Some((0, 710))` |
| `scale_bucket_values` | (1.0,1)→1.0; (1.1,1)→1.25; (0.3,1)→0.5; (0.5,1)→0.5; (2.5,1)→3.0; (5.0,1)→4.0; (1.0,2)→2.0; (1.1,1.5)→1.875; (1.250_000_1,1)→1.25 | as stated (assert within 1e-6) |
| `max_scale_px_values` | `[612×792]`, 16384 → 16383/792 ≈ 20.6856; `[2384×3370]`, 8192 → 8191/3370 ≈ 2.43056; `[]` → ∞ | as stated |
| `parse_page_input_values` | ("3",10)→Some(2); (" 1 ",10)→Some(0); ("10",10)→Some(9); ("0",10), ("11",10), ("abc",10), ("",10), ("-1",10), ("1",0) → None | as stated |

### A2. `src/search.rs` (pure)

```rust
use egui::Rect;

/// One match. `rects` are in page points, TOP-left origin (the worker has already flipped them).
#[derive(Debug, Clone, PartialEq)]
pub struct Hit { pub page: usize, pub rects: Vec<Rect> }

/// Query, results and the selected hit. `hits` are sorted by page (worker order).
#[derive(Debug, Default)]
pub struct SearchState {
    pub query: String,
    pub generation: u64,
    pub hits: Vec<Hit>,
    pub current: Option<usize>,
    pub pending: bool,
}

impl SearchState {
    /// New query: `generation += 1`, clears hits/current, `pending = !query.is_empty()`.
    /// Returns the generation to send with `Cmd::Search` (always sent, including for "").
    pub fn start(&mut self, query: &str) -> u64;
    /// Drops the result if `generation != self.generation`. Otherwise stores the hits, sets `pending = false`,
    /// and selects the first hit with `page >= from_page` (else 0; `None` if empty). Returns `true` if accepted.
    pub fn set_results(&mut self, generation: u64, hits: Vec<Hit>, from_page: usize) -> bool;
    /// Selects the next hit, wrapping to the first. `None` if there are no hits.
    pub fn next(&mut self) -> Option<&Hit>;
    /// Selects the previous hit, wrapping to the last. `None` if there are no hits.
    pub fn prev(&mut self) -> Option<&Hit>;
    /// The selected hit.
    pub fn current_hit(&self) -> Option<&Hit>;
    /// `(index of first hit on page, hits on page)` via `partition_point`. Used for overlays.
    pub fn page_hits(&self, page: usize) -> (usize, &[Hit]);
    /// `self.start("")`: empties query and hits and makes in-flight results stale.
    pub fn clear(&mut self) -> u64;
    /// Toolbar count: "" (empty query), "…" (pending), "0 / 0", "3 / 17" (current+1 / len).
    pub fn label(&self) -> String;
    /// `true` when a finished non-empty query has no hits (toolbar paints the label in `error`).
    pub fn no_results(&self) -> bool;
}
```
`from_page` is an addition. Without it, a search typed on page 120 would jump to page 3.

**Tests**. Helper `hit(p) = Hit{page:p, rects: vec![]}`.

| test | expected |
|---|---|
| `start_bumps_generation_and_clears` | after results, `start("x")` returns g+1; hits empty; current None; pending true |
| `start_empty_not_pending` | `start("")` returns g+1, pending false |
| `stale_results_ignored` | g1=start("a"), g2=start("ab"); `set_results(g1,[hit(0)],0)` → false, hits empty, pending true |
| `results_select_first_at_or_after_page` | pages [1,4,9]: from 5 → Some(2); from 10 → Some(0); `[]` → None and pending false |
| `next_wraps` | 3 hits, current 2 → `next()` page of hits[0]; current Some(0) |
| `prev_wraps` | current 0 → `prev()` → current Some(2) |
| `next_prev_empty_none` | no hits → both None |
| `page_hits_slices` | pages [1,1,4]: p1 → (0, len 2); p4 → (2, len 1); p2 → (2, len 0) |
| `clear_invalidates_in_flight` | g=start("a"); clear(); `set_results(g,..)` → false; label "" |
| `label_formats` | "" / "…" / "0 / 0" / "2 / 3" |

### A3. `src/render/worker.rs`

```rust
use std::{collections::VecDeque, path::PathBuf};
use crossbeam_channel::{Receiver, Sender};
use crate::{layout::PageSize, search::Hit};

/// UI → worker.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    /// Replace the open document.
    Open(PathBuf),
    /// Render `page` at `scale_px` px per pt (ppp already included). Older `generation`s are dropped.
    Render { page: usize, scale_px: f32, generation: u64 },
    /// Find `query` (case-insensitive) in all pages. "" cancels and replies with no hits.
    Search { query: String, generation: u64 },
}

/// Worker → UI. Each send is followed by `ctx.request_repaint()`.
pub enum Msg {
    Opened { name: String, sizes: Vec<PageSize> },
    OpenFailed(String),
    /// No libpdfium could be bound. Sent at startup and again for every later `Open`.
    LibraryMissing(String),
    Page { page: usize, scale_px: f32, image: egui::ColorImage },
    SearchDone { generation: u64, hits: Vec<Hit> },
}

/// Starts the `pdf-worker` thread. If the spawn fails, `LibraryMissing("cannot start render thread: …")` is queued.
pub fn spawn(ctx: egui::Context, candidates: Vec<PathBuf>) -> (Sender<Cmd>, Receiver<Msg>);

// private
struct SearchJob { query: String, generation: u64, next_page: usize, hits: Vec<Hit> }
fn run(ctx: &egui::Context, candidates: &[PathBuf], rx: &Receiver<Cmd>, tx: &Sender<Msg>);
fn bind_pdfium(candidates: &[PathBuf]) -> Result<Pdfium, String>;
fn compact(queue: &mut VecDeque<Cmd>);
fn flip(left: f32, bottom: f32, right: f32, top: f32, page_h: f32) -> egui::Rect;
```

**Thread function `run`.** PDFium objects are `!Send` and `PdfDocument<'a>` borrows `&'a Pdfium`. They therefore live as locals in this one function, with `pdfium` declared first:
```rust
let pdfium = match bind_pdfium(candidates) {
    Ok(p) => p,
    Err(e) => {               // reply to every Open so a dropped file also shows the hint
        send(Msg::LibraryMissing(e.clone()));
        for cmd in rx { if matches!(cmd, Cmd::Open(_)) { send(Msg::LibraryMissing(e.clone())); } }
        return;
    }
};
let mut doc: Option<PdfDocument<'_>> = None;    // borrows `pdfium`; re-created on Open
let mut queue: VecDeque<Cmd> = VecDeque::new();
let mut job: Option<SearchJob> = None;
loop {
    if queue.is_empty() && job.is_none() {
        let Ok(cmd) = rx.recv() else { return };      // UI gone
        queue.push_back(cmd);
    }
    queue.extend(rx.try_iter());                      // drain everything pending
    compact(&mut queue);
    if let Some(cmd) = queue.pop_front() {            // commands pre-empt search, one at a time
        match cmd {
            Cmd::Open(path) => { job = None; doc = None; /* load, send Opened | OpenFailed */ }
            Cmd::Render { page, scale_px, .. } => { /* if doc: render, send Page; on Err: warn!, send nothing */ }
            Cmd::Search { query, generation } => {
                // "" or no doc: job = None, send SearchDone{generation, hits: vec![]}
                // else job = Some(SearchJob{query, generation, next_page: 0, hits: vec![]})
            }
        }
    } else if let (Some(j), Some(d)) = (job.as_mut(), doc.as_ref()) {
        // search ONE page: j.hits.extend(page hits); j.next_page += 1;
        // when j.next_page == page count: send SearchDone{generation, hits: take(j.hits)}; job = None
    } else {
        job = None;
    }
}
```
- `send` is a closure: `|m| { let _ = tx.send(m); ctx.request_repaint(); }`.
- A newer `Search` replaces `job`. `Open` clears it. A `Render` runs between two search pages. That pre-emption check is the "peek" between pages: `rx.try_iter()` runs into `queue` at the top of every iteration.

**`compact(queue)`.** Pure. Order is preserved.
1. If any `Open` exists, drop everything before the **last** `Open`.
2. Keep only the **last** `Search`.
3. Let `g` = max `Render.generation` in the queue. Drop every `Render` with `generation < g`. Dedupe `Render`s by `(page, scale_px.to_bits())`, keeping the first.

**Open.**
- Call `pdfium.load_pdf_from_file(&path, None)`.
- Get sizes from `doc.pages().page_sizes()?` (they do not load pages), as `PageSize{w: r.width().value, h: r.height().value}`.
- `name` = `path.file_name()` lossy, falling back to `path.display()`.
- Error text:
  - `PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)` → `"{name} is password-protected (not supported)"`.
  - Any other error → `"Cannot open {name}: {e}"`.

**Render.**
- Get the page with `doc.pages().get(i32::try_from(page)?)`. If it is out of range, return Err, then warn and skip.
- Call `page.render_with_config(&PdfRenderConfig::new().scale_page_by_factor(scale_px))`. The `new()` defaults are exactly the required config: BGRA plus `reverse_byte_order = true` gives **RGBA**, annotations on, form data on, LCD text off and white clear. Add no other setters.
- Build the image with `ColorImage::from_rgba_unmultiplied([w, h], &bmp.as_rgba_bytes())` where `w, h = usize::try_from(bmp.width()/height())`.
- Log `debug!(page, scale_px, ms, "rendered")` (the Verification phase uses this latency log).
- Drop the page before the next command.

**Search a page.**
- Steps:
  1. `let h = page.height().value;`
  2. `let text = page.text()?;`
  3. `text.search(&query, &PdfSearchOptions::new())?` (case-insensitive, substring).
  4. `iter(PdfSearchDirection::SearchForward)` gives one `Hit` per `PdfPageTextSegments`.
  5. `rects = segs.iter().map(|s| { let b = s.bounds(); flip(b.left().value, b.bottom().value, b.right().value, b.top().value, h) })`.
- `flip` = `Rect::from_min_max(pos2(l, h - t), pos2(r, h - b))`.
- If a page errors, skip it (`debug!`).

**`bind_pdfium`.**
- Try `Pdfium::bind_to_library(p)` for each candidate in order. On the first `Ok(b)`, return `Ok(Pdfium::new(b))`. Log each failure at `debug`.
- Then try `Pdfium::bind_to_system_library()`.
- On total failure, return `Err(format!("libpdfium.so not found (tried {list}, system library). Run ./install.sh"))`.
- Never use `Pdfium::default()` (it panics).
- Bindings are a process-global `OnceCell`, so bind only once per process.

**Doc identity without generations.** The UI clears its cache when it receives `Opened`. The worker sends every `Page` of the old document before it processes `Open` (FIFO). `compact` drops old Renders queued before `Open`. The UI sends no Renders while `doc == None` (Loading). So every `Page` after `Opened` belongs to the new document.

**Tests.**

| test | expected |
|---|---|
| `compact_keeps_last_open_and_after` | `[R(p1,g1), Open(a), R(p2,g1), Open(b), S("x",g2)]` → `[Open(b), S("x",g2)]` |
| `compact_keeps_newest_render_generation` | `[R(p1,g1), R(p2,g2), R(p3,g2)]` → `[R(p2,g2), R(p3,g2)]` |
| `compact_dedupes_same_key` | `[R(p1,1.0,g2), R(p1,1.0,g2), R(p1,2.0,g2)]` → first + `R(p1,2.0,g2)` |
| `compact_keeps_only_last_search` | `[S("a",g1), R(p0,g1), S("ab",g2)]` → `[R(p0,g1), S("ab",g2)]` |
| `flip_converts_bottom_left_origin` | `flip(72., 715., 140., 740., 792.)` → (72,52)–(140,77) |
| `worker_end_to_end` | Write `tiny_pdf(&["Hello","World"])` (api-notes §5, test-only helper) to `temp_dir()/paperlight-test-{pid}.pdf`. `spawn(egui::Context::default(), default_candidates())`. Use `recv_timeout(10 s)` for each step. (a) `Open` → `Opened{name == file name, sizes == [612×792; 2]}`; if `LibraryMissing` arrives instead, `eprintln!` and return (skip). (b) `Render{page:0, scale_px:0.5, generation:1}` → `Page{image.size == [306, 396]}`. (c) `Search{"world",1}` → `SearchDone{generation:1, hits.len()==1, hits[0].page==1, rect.min.y in 40.0..72.0}`. (d) `Open("/nonexistent.pdf")` → `OpenFailed`. |

**This is the only test in the crate that binds PDFium.** A second binding in the same test binary returns `PdfiumLibraryBindingsAlreadyInitialized`. Run it with `PAPERLIGHT_PDFIUM=<path> cargo test`. The library is already extracted at `/tmp/claude-1000/-home-yash-Documents-Coding-Projects-Gemini/5f317c08-d80c-4689-9642-ad94f864b307/scratchpad/pdfium/x/lib/libpdfium.so`, or use `~/.local/lib/paperlight/libpdfium.so` after `install.sh`.

### A4. `src/render/cache.rs`

```rust
use std::{collections::{HashMap, HashSet}, ops::Range};
use egui::{TextureHandle, TextureId};

/// Texture identity: page plus exact render scale (f32 bits, so it can be hashed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TexKey { pub page: usize, pub scale_px_bits: u32 }
impl TexKey {
    pub fn new(page: usize, scale_px: f32) -> Self;
    pub fn scale_px(self) -> f32;
}

/// GPU page textures. Dropping a `TextureHandle` frees the texture.
#[derive(Default)]
pub struct PageCache {
    textures: HashMap<TexKey, TextureHandle>,
    /// Keys of the latest Render batch not yet answered (replaced wholesale per batch by app.rs).
    pub in_flight: HashSet<TexKey>,
}
impl PageCache {
    /// Stores a texture and removes `key` from `in_flight`.
    pub fn insert(&mut self, key: TexKey, tex: TextureHandle);
    /// Exact `(page, wanted_px)` texture → `(id, true)`; else this page's highest-scale texture → `(id, false)`.
    pub fn get_best(&self, page: usize, wanted_px: f32) -> Option<(TextureId, bool)>;
    /// Keeps exact-scale textures for pages in `visible.start-2 .. visible.end+2`, plus, for each
    /// visible page WITHOUT an exact texture, its single highest-scale fallback. Drops the rest.
    pub fn retain(&mut self, visible: Range<usize>, wanted_px: f32);
    /// Drops all textures and in-flight keys (new document).
    pub fn clear(&mut self);
}
```
Tests use a real headless context:
```rust
let ctx = egui::Context::default();
let tex = |c| ctx.load_texture("t", ColorImage::filled([1, 1], Color32::WHITE), TextureOptions::LINEAR);
```
Compare ids with `handle.id()` captured before `insert`. Add `#[cfg(test)] fn len(&self) -> usize`.

| test | expected |
|---|---|
| `get_best_prefers_exact` | p0@1.0, p0@2.0 → `get_best(0,1.0) == (id@1.0, true)` |
| `get_best_falls_back_to_highest_scale` | same → `get_best(0,1.5) == (id@2.0, false)` |
| `get_best_missing_page_none` | `get_best(5,1.0) == None` |
| `insert_clears_in_flight` | key in `in_flight`, insert → removed |
| `retain_keeps_exact_in_window` | p0..p9 @1.0, `retain(4..6, 1.0)` → len 6 (p2..=p7) |
| `retain_keeps_one_fallback_for_visible` | p4@1.0, p4@1.5, `retain(4..5, 2.0)` → only p4@1.5 |
| `retain_drops_fallback_when_exact_present` | p4@1.0, p4@2.0, `retain(4..5, 2.0)` → only p4@2.0 |
| `retain_drops_offscreen_fallback` | p8@1.0, `retain(4..5, 2.0)` → empty |

### A5. `src/render/pdfium_loader.rs` (pure plus one 5-line wrapper)

```rust
use std::{ffi::OsStr, path::{Path, PathBuf}};
pub const LIB_NAME: &str = "libpdfium.so";
/// Ordered libpdfium files to try before the system library:
/// `env_override` (a file path; ignored if empty), `exe_dir/libpdfium.so`, `home/.local/lib/paperlight/libpdfium.so`.
pub fn candidate_paths(env_override: Option<&OsStr>, exe_dir: Option<&Path>, home: Option<&Path>) -> Vec<PathBuf>;
/// `candidate_paths` fed from `$PAPERLIGHT_PDFIUM`, `current_exe()`'s parent and `$HOME` (var_os, not home_dir).
pub fn default_candidates() -> Vec<PathBuf>;
```
The system library (`dlopen("libpdfium.so")`) is the implicit last step in `bind_pdfium`.

| test | expected |
|---|---|
| `order_is_env_exe_home` | (`/x/p.so`, `/opt/pl`, `/home/u`) → [`/x/p.so`, `/opt/pl/libpdfium.so`, `/home/u/.local/lib/paperlight/libpdfium.so`] |
| `empty_env_ignored` | (`""`, None, `/home/u`) → [`/home/u/.local/lib/paperlight/libpdfium.so`] |
| `missing_inputs_skipped` | (None, None, None) → `[]` |

### A6. `src/input/pinch_wayland.rs`

```rust
/// Multiplicative zoom since the previous pinch event (>1 = fingers spreading).
#[derive(Debug, Clone, Copy)]
pub struct PinchEvent { pub zoom: f32 }

/// Binds zwp_pointer_gestures_v1 on winit's wl_display and starts the detached `pinch` thread.
/// Returns `None` (logged at debug) on X11, without the global, or on any error.
pub fn start(cc: &eframe::CreationContext<'_>, ctx: egui::Context) -> Option<Receiver<PinchEvent>>;
```
State and dispatch:
```rust
struct State {
    tx: Sender<PinchEvent>,
    ctx: egui::Context,
    seat: Option<WlSeat>,
    pointer: Option<WlPointer>,
    gestures: Option<ZwpPointerGesturesV1>,
    pinch: Option<ZwpPointerGesturePinchV1>,
    last_scale: f64,
}
impl State {
    /// Creates the pinch object once both the pointer and the gestures global exist.
    fn try_create_pinch(&mut self, qh: &QueueHandle<Self>);
}
```
| impl | behaviour |
|---|---|
| `Dispatch<WlRegistry, ()>` | `Global{name, interface, version}`. For `"wl_seat"` with `seat` None: `seat = Some(registry.bind::<WlSeat,_,_>(name, version.min(1), qh, ()))`. For `"zwp_pointer_gestures_v1"`: `gestures = Some(registry.bind(name, version.min(1), qh, ()))`, then `try_create_pinch`. Ignore `GlobalRemove`. |
| `Dispatch<WlSeat, ()>` | `Capabilities{capabilities: WEnum::Value(c)}` with `c.contains(Capability::Pointer)` and `pointer` None: `pointer = Some(seat.get_pointer(qh, ()))`, then `try_create_pinch`. Ignore everything else. |
| `delegate_noop!(State: ignore WlPointer)` | Our second pointer receives enter/motion/button/axis/frame. Ignore them. **Must use the `ignore` form**: plain `delegate_noop!` calls `unreachable!()` on events. |
| `delegate_noop!(State: ZwpPointerGesturesV1)` | The interface has no events. |
| `Dispatch<ZwpPointerGesturePinchV1, ()>` | `Begin{..}` sets `last_scale = 1.0` (ignore `surface`: it is winit's object). `Update{scale, ..}`: if `scale > 0 && last_scale > 0`, send `PinchEvent{zoom: (scale / last_scale) as f32}` (`#[expect(clippy::cast_possible_truncation)]`), set `last_scale = scale`, call `ctx.request_repaint()`. `End{..}` sets `last_scale = 1.0`. |

`start` steps:
1. `let RawDisplayHandle::Wayland(h) = cc.display_handle().ok()?.as_raw() else { debug!(..); return None };` (needs `use raw_window_handle::HasDisplayHandle`).
2. `// SAFETY: winit's wl_display outlives this Backend: App::on_exit calls process::exit before winit disconnects.`
   `let backend = unsafe { Backend::from_foreign_display(h.display.as_ptr().cast()) };` This runs on the UI thread because `NonNull` is `!Send`.
3. Create the connection and queue: `let conn = Connection::from_backend(backend); let mut queue = conn.new_event_queue::<State>(); let qh = queue.handle(); conn.display().get_registry(&qh, ());`
4. Run `queue.roundtrip(&mut state)` **twice**. The first binds globals. The second delivers seat capabilities, which create the pointer and the pinch object. On `Err`, log at debug and return None.
5. If `state.pinch.is_none()`, log at debug ("no pointer gestures") and return None.
6. Start the thread: `thread::Builder::new().name("pinch".into()).spawn(move || { let _conn = conn; while queue.blocking_dispatch(&mut state).is_ok() {} })`. Use `.ok()?` on the spawn result. The thread is detached (never joined).

**Exit safety.** The pinch thread borrows winit's `wl_display`. If an event wakes it after winit's `wl_display_disconnect`, libwayland reads freed memory. Contract:
- `impl eframe::App for App { fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) { std::process::exit(0) } }`.
- `on_exit` runs while winit's display is still connected (`glow_integration.rs:517`, before the event loop drops). The process ends before any disconnect.
- `main` also calls `std::process::exit(code)` right after `run_native` returns (error paths).
- No `save` runs (no persistence feature), so nothing is lost.

### A7. `src/ui/toolbar.rs`

```rust
/// What the user did in the toolbar this frame (at most one).
#[derive(Debug, Clone, PartialEq)]
pub enum ToolbarAction { None, GoToPage(usize), ZoomIn, ZoomOut, FitWidth, SearchChanged(String), SearchNext, SearchPrev, SearchClosed }

/// Text-field buffers owned by the toolbar.
#[derive(Debug, Default)]
pub struct ToolbarState {
    pub page_text: String,
    pub search_text: String,
    /// Set by Ctrl+F in app.rs; consumed (request_focus) on the next show().
    pub focus_search: bool,
}

/// Read-only data the toolbar displays.
pub struct DocInfo<'a> {
    pub name: Option<&'a str>,     // None → "No document" in text_secondary
    pub current_page: usize,       // 0-based
    pub total_pages: usize,        // 0 = no document: page/zoom/search controls disabled
    pub zoom: f32,
    pub search: &'a SearchState,   // label() and no_results()
}

/// Draws the 44-pt toolbar contents (call inside `egui::Panel::top`). Layout per design.md §Toolbar.
pub fn show(ui: &mut egui::Ui, st: &mut ToolbarState, doc: &DocInfo<'_>) -> ToolbarAction;
```
Behaviour:
- **Layout**: `ui.with_layout(Layout::right_to_left(Align::Center), ..)`. Add zoom first, then the page entry, then search. Finish with a nested `left_to_right` holding `Label::new(name).truncate()`, with the full name as hover text.
- **Ids**: `Id::new("page-entry")` and `Id::new("search")`. Set them with `TextEdit::id`.
- **Page entry**:
  - Before drawing: if `!ui.memory(|m| m.has_focus(page_id))`, set `st.page_text = (current_page + 1).to_string()`. This syncs the field with scroll and reverts invalid input.
  - Use `TextEdit::singleline(..).desired_width(44.).horizontal_align(Align::Center)`.
  - On `resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))`: `parse_page_input(&st.page_text, total).map_or(ToolbarAction::None, ToolbarAction::GoToPage)`.
  - Then show `/ {total}` in Small `text_secondary`.
- **Search field** (220 wide, hint "Search"):
  - If `st.focus_search`, call `request_focus()` and clear the flag.
  - `changed()` → `SearchChanged(st.search_text.clone())` (search as you type; the worker supersedes stale searches).
  - `lost_focus() && Enter` → `SearchPrev` if `shift` is held, else `SearchNext`, then `resp.request_focus()` so repeated Enter keeps cycling.
  - `lost_focus() && Escape` → `st.search_text.clear()`, `SearchClosed`.
  - With a non-empty query, show `label()` (Small; `error` colour if `no_results()`) and ▲/▼ buttons (24×24, both `RADIUS_SM`) that return `SearchPrev`/`SearchNext`.
- **Zoom**: `−` → `ZoomOut`. A flat `"{round(zoom·100)}%"` button (min width 56, tooltip "Fit width") → `FitWidth`. `+` → `ZoomIn`.

### A8. `src/ui/viewer.rs`

```rust
use std::{ops::Range, time::{Duration, Instant}};

/// Wait after the last zoom change before requesting new-bucket renders.
pub const ZOOM_SETTLE: Duration = Duration::from_millis(100);

/// Zoom, scroll and layout of the page canvas. Survives across frames.
pub struct ViewerState {
    zoom: f32,
    fit_width: bool,                // true on open; any manual zoom clears it
    layout: PageLayout,
    layout_w: f32,                  // viewport width `layout` was built for
    offset: Vec2,                   // last frame's clamped scroll offset
    view_rect: Rect,                // last frame's viewport (screen coords)
    anchor: Option<Vec2>,           // zoom anchor relative to view_rect.min; None = viewport centre
    pending_offset: Option<Vec2>,   // forced into ScrollArea on the next show(), then cleared
    zoom_changed_at: Option<Instant>,
}

/// Results of one viewer frame.
pub struct ViewerOutput { pub visible: Range<usize>, pub current_page: usize }

impl Default for ViewerState { /* zoom 1.0, fit_width true, rest empty/None */ }
impl ViewerState {
    /// New document: fit_width = true, layout = default, pending_offset = Some(ZERO), zoom_changed_at = None.
    pub fn reset(&mut self);
    pub fn zoom(&self) -> f32;
    /// zoom = clamp_zoom(zoom·factor); fit_width = false; anchor from `pointer` if it is inside view_rect; zoom_changed_at = now.
    pub fn zoom_by(&mut self, factor: f32, pointer: Option<Pos2>);
    /// fit_width = true; anchor = None; zoom_changed_at = now.
    pub fn fit_width(&mut self);
    /// pending_offset = clamp(x, layout.page_scroll_y(page)).
    pub fn go_to_page(&mut self, page: usize);
    /// pending_offset = clamp(pending_offset.unwrap_or(offset) + (0, dy)).
    pub fn scroll_by(&mut self, dy: f32);
    /// Scrolls so that `rect_pt` (page points, top-left origin) on `page` is visible (layout::reveal_offset), clamped.
    pub fn reveal(&mut self, page: usize, rect_pt: Rect);
    /// Viewport height of the last frame (PageUp/PageDown step = height − PAGE_GAP).
    pub fn view_height(&self) -> f32;
    /// `zoom_changed_at.is_none_or(|t| now - t >= ZOOM_SETTLE)`.
    pub fn settled(&self, now: Instant) -> bool;
}

/// Lays out and paints the visible pages; consumes `pending_offset`.
pub fn show(ui: &mut egui::Ui, st: &mut ViewerState, sizes: &[PageSize],
            cache: &PageCache, wanted_px: f32, search: &SearchState) -> ViewerOutput;
```
Every "clamp" above uses `layout::clamp_offset(o, layout.content_size(), view_rect.size())`.

`show` algorithm:
1. Set `let vw = ui.available_width();`. Floating scroll bars (egui default) do not take width.
2. `if st.fit_width { let z = fit_width_scale(sizes, vw, PAGE_MARGIN); if bits differ { st.zoom = z; st.zoom_changed_at = Some(now) } }`. Resizing in fit mode is debounced like any other zoom change.
3. Rebuild if `layout.scale()` and `zoom` have different bits, or `layout_w` and `vw` have different bits, or `layout.rects().len() != sizes.len()`:
   - Build `new = PageLayout::new(sizes, zoom, vw, PAGE_GAP, PAGE_MARGIN)`.
   - If `pending_offset.is_none() && !layout.rects().is_empty()`, set `pending_offset = Some(clamp(zoom_about(&layout, &new, anchor.take().unwrap_or(view_rect.size()/2), offset)))`. Clamp against `new.content_size()`.
   - Then set `layout = new; layout_w = vw`.
4. Show the ScrollArea:
   ```rust
   let mut area = ScrollArea::both().id_salt("pages").auto_shrink(false);
   if let Some(o) = st.pending_offset.take() { area = area.scroll_offset(o); }
   let out = area.show_viewport(ui, |ui, view| { ui.set_min_size(layout.content_size()); paint(..) ; (visible, current) });
   ```
   Then `st.offset = out.state.clamped_offset(); st.view_rect = out.inner_rect;`.
5. Paint (inside the closure, `origin = ui.max_rect().min`; `pal = theme::palette(ui.visuals().dark_mode)`). For `i in layout.visible_range(view)`, with `r = layout.rects()[i].translate(origin.to_vec2()).round_to_pixels(ppp)` (`egui::emath::GuiRounding`):
   1. `painter.add(Shadow{offset:[0,2], blur:12, spread:0, color: pal.page_shadow}.as_shape(r, 0))`.
   2. `painter.rect_filled(r, 0, Color32::WHITE)` (placeholder; never a blank hole).
   3. `if let Some((id, _)) = cache.get_best(i, wanted_px) { painter.image(id, r, UV_FULL, Color32::WHITE) }`, where `UV_FULL = Rect::from_min_max(pos2(0,0), pos2(1,1))`.
   4. `painter.rect_stroke(r, 0, Stroke::new(1.0, pal.page_border), StrokeKind::Inside)`.
   5. Hits: let `(first, hits) = search.page_hits(i)`. For each hit `k` and each rect, `painter.rect_filled(layout.to_doc(i, rect).translate(origin.to_vec2()).expand(1.0), RADIUS_SM, if Some(first+k) == search.current { pal.hit_current } else { pal.hit })`.
6. Return `ViewerOutput{ visible: layout.visible_range(view), current_page: layout.current_page(view) }`.

Horizontal scrolling happens only when content is wider than the viewport. `ScrollArea::both()` shows a bar only when needed, and centring is built into `PageLayout`.

### A9. `src/app.rs` and `src/main.rs`

```rust
pub struct App {
    tx: Sender<Cmd>,
    rx: Receiver<Msg>,
    pinch: Option<Receiver<PinchEvent>>,
    doc: Option<Doc>,
    status: Status,                 // what the canvas shows while doc is None
    cache: PageCache,
    render_gen: u64,
    search: SearchState,
    toolbar: ToolbarState,
    viewer: ViewerState,
    current_page: usize,
}
struct Doc { name: String, sizes: Vec<PageSize>, max_px: f32 }
enum Status { Empty, Loading, Error(String) }

impl App {
    /// theme::apply; ctx.options_mut(|o| o.zoom_with_keyboard = false) (egui would otherwise eat Ctrl+=/-/0 as UI scaling);
    /// worker::spawn(ctx, pdfium_loader::default_candidates()); pinch_wayland::start(cc, ctx); open(path) if Some.
    pub fn new(cc: &eframe::CreationContext<'_>, path: Option<PathBuf>) -> Self;
    // private
    fn open(&mut self, path: PathBuf);   // doc = None; status = Loading; tx.send(Cmd::Open(path))
    fn handle_msg(&mut self, ctx: &egui::Context, msg: Msg);
    fn handle_keys(&mut self, ctx: &egui::Context);
    fn handle_toolbar(&mut self, action: ToolbarAction);
    fn request_renders(&mut self, ctx: &egui::Context, out: &ViewerOutput);
}
impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame);
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) { std::process::exit(0) } // see A6
}
```

**`ui()` order** (all steps on the UI thread):
1. **Messages**: `while let Ok(m) = self.rx.try_recv() { self.handle_msg(&ctx, m) }`.
   - `Opened{name,sizes}`:
     - `max_px = max_scale_px(&sizes, ctx.input(|i| i.max_texture_side))`.
     - `doc = Some(..)`, `status = Empty`.
     - `cache.clear()`, `search.clear()`, `toolbar.search_text.clear()`, `viewer.reset()`.
     - `ctx.send_viewport_cmd(ViewportCommand::Title(format!("{name} — Paperlight")))`.
   - `OpenFailed(e)` and `LibraryMissing(e)`: `doc = None`, `status = Error(e)`, title `"Paperlight"`.
   - `Page{page, scale_px, image}`: if `doc.is_some()`, `cache.insert(TexKey::new(page, scale_px), ctx.load_texture(format!("page-{page}"), image, TextureOptions::LINEAR))`.
   - `SearchDone{generation, hits}`: if `search.set_results(generation, hits, current_page)` returns true and there is a `current_hit()`, call `viewer.reveal(hit.page, hit.rects[0])` (skip if `rects` is empty).
2. **Zoom input** (only if `doc.is_some()`):
   - `let (dz, hover) = ctx.input(|i| (i.zoom_delta(), i.pointer.hover_pos()));` This covers Ctrl+wheel and `Event::Zoom`.
   - `let f = dz * pinch.try_iter().map(|e| e.zoom).product::<f32>();`
   - `if (f - 1.0).abs() > 1e-4 { viewer.zoom_by(f, hover) }`.
3. **Keys and drop**: `handle_keys` (table below). Dropped file: `ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()))` → `open`.
4. **Toolbar**: draw with `egui::Panel::top("toolbar").exact_size(theme::TOOLBAR_HEIGHT).resizable(false).frame(Frame::new().fill(pal.toolbar_bg).inner_margin(Margin::symmetric(12, 8)))`, then `.show(ui, |ui| toolbar::show(ui, &mut self.toolbar, &info)).inner`, then `handle_toolbar`. Action handling:
   - `GoToPage(p)` → `viewer.go_to_page(p)`.
   - `ZoomIn`/`ZoomOut` → `viewer.zoom_by(ZOOM_STEP or 1/ZOOM_STEP, None)`.
   - `FitWidth` → `viewer.fit_width()`.
   - `SearchChanged(q)` → `let g = search.start(&q); tx.send(Cmd::Search{query: q, generation: g})`.
   - `SearchNext`/`SearchPrev` → `search.next()/prev()`, then `viewer.reveal(..)`.
   - `SearchClosed` → `let g = search.clear(); tx.send(Cmd::Search{query: String::new(), generation: g})`.
5. **Canvas**: `egui::CentralPanel::default().frame(Frame::new().fill(pal.canvas_bg)).show(ui, ..)`.
   - If there is a `doc`: compute `wanted = min(scale_bucket(viewer.zoom(), ctx.pixels_per_point()), doc.max_px)`. Then `out = viewer::show(ui, &mut self.viewer, &doc.sizes, &self.cache, wanted, &self.search)` and `current_page = out.current_page`.
   - Otherwise draw the empty, loading or error state per design.md §Canvas. Loading text: "Opening…".
6. **Renders** (`request_renders`, only with a doc). Let `now = Instant::now()`, `settled = viewer.settled(now)`, and `wanted` be the same value as in step 5.
   - Build the wanted pages:
     ```rust
     let n = sizes.len();
     let lo = out.visible.start.saturating_sub(1);
     let hi = (out.visible.end + 1).min(n);
     let missing: Vec<TexKey> = (lo..hi).filter(|&p| match cache.get_best(p, wanted) {
         Some((_, true)) => false,
         Some((_, false)) => settled,
         None => settled || out.visible.contains(&p),
     }).map(|p| TexKey::new(p, wanted)).collect();
     ```
     While a zoom is unsettled, only visible pages with no texture at all are requested.
   - If `missing.iter().any(|k| !cache.in_flight.contains(k))`, start a new batch: `render_gen += 1`, `cache.in_flight = missing.iter().copied().collect()`, then send `Cmd::Render{page, scale_px: wanted, generation: render_gen}` for **every** key in `missing`. Keys already in flight are resent so they survive the worker's stale-generation drop.
   - If `!settled`, call `ctx.request_repaint_after(ZOOM_SETTLE.saturating_sub(now - changed_at))`. Without this wake-up, no new-bucket render is ever requested after a gesture.
7. **Retain**: `cache.retain(out.visible, wanted)`.

**Keyboard** (`handle_keys`, via `ctx.input_mut(|i| i.consume_key(..))`). "Free" means `ctx.memory(|m| m.focused().is_none())`, i.e. no text field has focus.

| key | when | effect |
|---|---|---|
| Ctrl+F | always | `toolbar.focus_search = true` |
| Ctrl+= or Ctrl+Plus | doc | `viewer.zoom_by(ZOOM_STEP, None)` |
| Ctrl+Minus | doc | `viewer.zoom_by(1.0/ZOOM_STEP, None)` |
| Ctrl+0 | doc | `viewer.fit_width()` |
| PageDown / PageUp | free | `scroll_by(±(view_height() − PAGE_GAP))` |
| ArrowDown / ArrowUp | free | `scroll_by(±48.0)` |
| ArrowRight / ArrowLeft | free | `go_to_page(current ± 1)` (saturating, clamped to n−1) |
| Home / End | free | `go_to_page(0)` / `go_to_page(n−1)` |
| Escape | free, query non-empty | same as `SearchClosed` |

**`main.rs`**:
```rust
mod app; mod input; mod layout; mod render; mod search; mod ui;
fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_env("PAPERLIGHT_LOG").unwrap_or_else(|_| EnvFilter::new("warn")))
        .init();
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Paperlight").with_app_id("paperlight")
        .with_inner_size([900.0, 1100.0]).with_min_inner_size([420.0, 320.0])
        .with_drag_and_drop(true);
    match eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")) {
        Ok(icon) => viewport = viewport.with_icon(Arc::new(icon)),
        Err(e) => tracing::warn!("bad icon: {e}"),
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };   // glow is the only renderer compiled in
    let code = match eframe::run_native("paperlight", options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, path))))) {
        Ok(()) => 0,
        Err(e) => { tracing::error!("{e}"); 1 }
    };
    std::process::exit(code);
}
```

---

## B. Message and ownership diagram

```
 argv[1] / drag-drop                                      ┌───────────── pdf-worker thread ─────────────┐
        │                                                 │ owns (locals in run()):                     │
┌───────▼──────────── UI thread (eframe) ───────────┐     │   pdfium: Pdfium            (!Send)         │
│ App (owns everything below)                       │     │   doc: Option<PdfDocument<'_>> (borrows ↑)  │
│  ToolbarState ─ toolbar::show ─▶ ToolbarAction    │ Cmd │   queue: VecDeque<Cmd> → compact()          │
│  ViewerState  ─ viewer::show  ─▶ ViewerOutput     │────▶│   job: Option<SearchJob>  (1 page per step) │
│  PageCache {TexKey→TextureHandle, in_flight}      │     │                                             │
│  SearchState {query, generation, hits, current}   │◀────│ Msg + ctx.request_repaint() after each send │
│  Doc {name, sizes, max_px}   render_gen: u64      │ Msg └─────────────────────────────────────────────┘
│                                                   │
│                                                   │◀── PinchEvent{zoom} + request_repaint ──┐
└──────────────────────────┬────────────────────────┘                                         │
                           │ egui::Context clones (request_repaint only)    ┌─────── pinch thread (Wayland only) ───────┐
                           └───────────────────────────────────────────────▶│ EventQueue<State>, own libwayland queue   │
                                                                            │ on winit's wl_display (guest Backend),    │
                                                                            │ blocking_dispatch loop, detached          │
                                                                            └───────────────────────────────────────────┘
```

Flows:
- **Open**: UI `open()` → `Cmd::Open` (doc=None, Loading) → worker load → `Msg::Opened{name,sizes}` → cache cleared, `viewer.reset()` → next frame: layout (fit width), renders for visible±1 → `Msg::Page` → `load_texture` → painted.
- **Scroll**: `visible` changes → new missing keys → new batch with `render_gen+1` → worker drops older-generation Renders still queued.
- **Zoom** (pinch / Ctrl+wheel / keys):
  1. `zoom_by` updates the zoom at once.
  2. The layout is rebuilt and `zoom_about` sets the offset, so the GPU scales existing textures this frame.
  3. Visible pages with no texture are requested immediately.
  4. After 100 ms without change, `settled` becomes true and the bucket renders go out.
  5. `retain` keeps one fallback per visible page until its exact texture arrives.
- **Search**: keystroke → `search.start` → `Cmd::Search{generation}` → worker searches page by page, yielding to Renders and to newer Search/Open → `Msg::SearchDone` → stale generations ignored → first hit at or after the current page is revealed.

---

## C. Ordered task list (TDD)

Each task ends with the gate: `cargo test` green, `cargo clippy --all-targets -- -D warnings` clean and `cargo fmt --check`.

| # | files | work (tests first) | success criterion | est. lines |
|---|---|---|---|---|
| 1 | `main.rs`, `render/mod.rs`, `input/mod.rs`, `ui/mod.rs` | module skeleton. `main` from A9 with a stub App showing "Drop a PDF here". Keep `theme.rs` as is. | window opens with icon and title "Paperlight"; `WAYLAND_DEBUG=1` shows `xdg_toplevel.set_app_id("paperlight")` | 60 |
| 2 | `layout.rs` | write all A1 tests, then implement | 28 tests pass | 140 |
| 3 | `search.rs` | A2 tests, then implement | 10 tests pass | 70 |
| 4 | `render/pdfium_loader.rs` | A5 tests, then implement | 3 tests pass | 30 |
| 5 | `render/cache.rs` | A4 tests, then implement | 8 tests pass | 70 |
| 6 | `render/worker.rs` | `compact_*` + `flip_*` tests, then `worker_end_to_end`, then implement | all pass; `worker_end_to_end` **passes** (does not skip) with `PAPERLIGHT_PDFIUM` set | 200 |
| 7 | `ui/viewer.rs`, `app.rs` (open, msgs, viewer, renders, retain, drag-drop, title) | none (UI). Logic is covered by 2/3/5/6. | `cargo run -- ~/Downloads/<50+ page pdf>`: pages render, idle CPU ≈ 0 (`PAPERLIGHT_LOG=debug` shows no renders while idle), smooth scroll | 300 |
| 8 | `ui/toolbar.rs`, `app.rs` (toolbar actions, keys, search) | none (UI) | plan Verification §5 items: filename, page counter, typed page + Enter, Ctrl+F/Enter/Shift+Enter/Esc, zoom ±/fit, % label | 170 |
| 9 | `input/pinch_wayland.rs`, hook in `App::new` + `on_exit` | none (needs a compositor) | `WAYLAND_DEBUG=1 cargo run` shows `zwp_pointer_gestures_v1#N.get_pinch_gesture`; pinch `update` events move the zoom; X11 (`WAYLAND_DISPLAY= cargo run`) starts fine with pinch None; closing the window exits cleanly (no core in `coredumpctl list paperlight`) | 120 |
| 10 | `install.sh`, `paperlight.desktop` | spec D | plan Verification §2–4 and §7 | 90 (shell) |
| 11 | none | regression and verification phases 6–7 of the plan | all checklists pass | 0 |

Rust total ≈ 1160 plus theme ≈ 150, within the ≤ 1400 budget.

**YAGNI traps (do not build):**
- No `Arc<Mutex<…>>` shared state, async runtime, LRU crate, generic cache, tiling, progressive rendering or bitmap buffer reuse.
- No `image` dependency, `thread_safe` feature or `static` feature.
- No second PDFium test (a second bind fails), and no `Pdfium::default()`.
- No password prompt, rotation commands, text selection or copy, search options UI, persistence (last page/zoom), settings, file dialog, single-instance IPC, thumbnails or outline.
- No scaling of gap/margin with zoom. No `Msg` generation for pages. No separate "best fallback" map.
- No texture creation on the worker thread. No mipmaps.
- No plain `delegate_noop!` for `WlPointer` (it panics on events). Keep egui's keyboard zoom disabled.
- No custom key handling inside the ScrollArea. egui 0.37 ScrollArea handles no keys (grep: no `Key::` in `scroll_area.rs`), so app.rs owns all navigation keys.

**Accepted limitations** (document them in the README; do not fix them now):
- Search highlights on pages with `/Rotate ≠ 0` or a non-zero MediaBox origin are misplaced. PDFium text boxes are in unrotated user space. A future fix would use `FPDF_PageToDevice`.
- At most 1.5× bilinear downsampling of text between buckets.
- A render that fails leaves a white page.
- A ~1-frame race can render one page twice.

---

## D. `install.sh` specification

Pinned PDFium values. Verified 2026-10-22: the tarball was downloaded and hashed locally, and the hash matches GitHub's API asset `digest`.

| item | value |
|---|---|
| release tag | `chromium/8111` (latest as of 2026-10-22, published 2026-10-19; `VERSION`: 158.0.8111.0) |
| URL | `https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/8111/pdfium-linux-x64.tgz` |
| tgz size / sha256 | 3828739 B / `bdc60126399f39fe768566a18d4be055a7b3681bc9e248e8a6656b4f6cae0aa2` |
| member path | `lib/libpdfium.so` (8049792 B) |
| `libpdfium.so` sha256 | `dca41ec4a2496ea7378b144cc4bad2d9e050692aaac75b9cc22411c1835f7f6e` |
| runtime deps (`ldd`) | libpthread, libm, libgcc_s, libc only |
| ABI check | every symbol that pdfium-render 0.9.4 (`pdfium_7881`) binds eagerly is exported by this build. The 11 symbols absent from it are all cfg-gated to older API versions or to xfa/v8/skia/win32. |

Constants and paths:
```bash
#!/usr/bin/env bash
# Build Paperlight, install it for the current user and make it the default PDF viewer.
# Undo: ./install.sh --uninstall
set -euo pipefail
cd "$(dirname "$0")"
PDFIUM_TAG="chromium/8111"
PDFIUM_URL="https://github.com/bblanchon/pdfium-binaries/releases/download/${PDFIUM_TAG}/pdfium-linux-x64.tgz"
PDFIUM_TGZ_SHA256="bdc60126399f39fe768566a18d4be055a7b3681bc9e248e8a6656b4f6cae0aa2"
PDFIUM_SO_SHA256="dca41ec4a2496ea7378b144cc4bad2d9e050692aaac75b9cc22411c1835f7f6e"
BIN="$HOME/.local/bin/paperlight"
LIB_DIR="$HOME/.local/lib/paperlight"
APPS="$HOME/.local/share/applications"
DESKTOP="$APPS/paperlight.desktop"
ICONS="$HOME/.local/share/icons/hicolor"
SIZES=(16 32 48 64 128 256 512)
FALLBACK="org.gnome.Evince.desktop"
```

**Install** (default), in order. Each step prints one line.
1. **Preflight**:
   - `command -v cargo curl tar sha256sum xdg-mime >/dev/null`. If anything is missing, exit 1 and name it.
   - `pkg-config --exists wayland-client`. If it fails, exit 1 with "Missing wayland-devel: sudo dnf install wayland-devel". wayland-sys links libwayland-client at build time (api-notes §4).
2. **libpdfium** (idempotent): skip if `$LIB_DIR/libpdfium.so` exists and its sha256 equals `PDFIUM_SO_SHA256`. Otherwise:
   ```bash
   tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
   curl -fsSL --retry 3 -o "$tmp/pdfium.tgz" "$PDFIUM_URL"
   echo "$PDFIUM_TGZ_SHA256  $tmp/pdfium.tgz" | sha256sum -c --quiet -
   tar -xzf "$tmp/pdfium.tgz" -C "$tmp" lib/libpdfium.so
   echo "$PDFIUM_SO_SHA256  $tmp/lib/libpdfium.so" | sha256sum -c --quiet -
   install -Dm644 "$tmp/lib/libpdfium.so" "$LIB_DIR/libpdfium.so"
   ```
   A hash mismatch aborts through `set -e`. Nothing is installed in that case.
3. **Binary**: `cargo build --release --locked`, then `install -Dm755 target/release/paperlight "$BIN"`.
4. **Icons**: `install -Dm644 assets/icon.svg "$ICONS/scalable/apps/paperlight.svg"`, then for each `s` in `SIZES`, `install -Dm644 "assets/icon-$s.png" "$ICONS/${s}x${s}/apps/paperlight.png"`.
5. **Desktop entry**: `mkdir -p "$APPS"; sed "s|^Exec=paperlight |Exec=$BIN |" paperlight.desktop > "$DESKTOP"; chmod 644 "$DESKTOP"`. The absolute Exec path removes any dependence on the GNOME session's PATH. Then run `desktop-file-validate "$DESKTOP"` if the command exists (warn only).
6. **Caches and default**:
   ```bash
   update-desktop-database "$APPS" 2>/dev/null || true
   gtk-update-icon-cache -q -t -f "$ICONS" 2>/dev/null || true
   xdg-mime default paperlight.desktop application/pdf
   ```
7. **Verify**: `[[ "$(xdg-mime query default application/pdf)" == paperlight.desktop ]]`, then print the installed paths.

**`--uninstall`**:
1. If `xdg-mime query default application/pdf` prints `paperlight.desktop` and `/usr/share/applications/$FALLBACK` exists, run `xdg-mime default "$FALLBACK" application/pdf`. If the file is missing, warn and leave the default as is.
2. Remove the files: `rm -f "$BIN" "$DESKTOP" "$ICONS/scalable/apps/paperlight.svg"`, then `rm -f "$ICONS/${s}x${s}/apps/paperlight.png"` for each size, then `rm -rf "$LIB_DIR"`.
3. Refresh both caches as in install step 6. Print `Removed Paperlight; default PDF viewer: $(xdg-mime query default application/pdf)`.
4. Exit 0.

**`paperlight.desktop`** (repo root; install.sh rewrites only the `Exec=` binary path):
```ini
[Desktop Entry]
Type=Application
Name=Paperlight
GenericName=PDF Viewer
Comment=Lightweight, fast PDF viewer
Exec=paperlight %f
Icon=paperlight
Terminal=false
Categories=Office;Viewer;Graphics;
MimeType=application/pdf;
Keywords=pdf;document;viewer;reader;
StartupWMClass=paperlight
StartupNotify=true
```
`Icon=paperlight` resolves through hicolor to the files from step 4. `StartupWMClass` matches `with_app_id("paperlight")`. That match is what makes GNOME group the window under this icon.
