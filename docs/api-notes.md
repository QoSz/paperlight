# Paperlight API notes (verified from crate sources)

All paths are relative to `R = ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`.
Cargo.lock pins: pdfium-render 0.9.4, wayland-backend 0.3.17, wayland-client 0.31.15,
wayland-protocols 0.32.13, winit 0.30.13, smithay-client-toolkit 0.19.2, raw-window-handle 0.6.2.
Each crate appears once in the lock, so winit/sctk and Paperlight share the same copies.

---

## 1. pdfium-render 0.9.4

### Features (`R/pdfium-render-0.9.4/Cargo.toml` `[features]`)
- `default = ["pdfium_latest", "image_latest", "thread_safe"]`.
- `pdfium_latest = ["pdfium_7881"]`. No other feature is required, so `default-features = false, features = ["pdfium_latest"]` is a valid set.
  `image_*` only gates `PdfBitmap::as_image()` (`src/pdf/bitmap.rs:328`, `#[cfg(feature = "image_api")]`).
- There is **no `sync` feature**. `static`, `libstdc++`, `libc++` and `core_graphics` all imply `static`. `static` removes
  `bind_to_system_library` / `bind_to_library` / `pdfium_platform_library_name*` (`#[cfg(not(feature = "static"))]`, `src/pdfium.rs:121-208`).
  Do not enable them.
- `pdfium_7881`: the bindings target the Chromium/7881 pdfium API. Ship a `libpdfium.so` of that build or newer (bblanchon pdfium-binaries).

### Thread safety (without `thread_safe`)
- `Pdfium` holds `Option<*mut FPDF_SYSFONTINFO>` (`src/pdfium.rs:88-93`). The `unsafe impl Send/Sync for Pdfium` is gated on `thread_safe` (`:625-629`), so here **`Pdfium` is `!Send + !Sync`**.
- `Send`/`Sync` for `PdfDocument` (`src/pdf/document.rs:449-453`), `PdfPages` (`pages.rs:584-587`), `PdfPage` (`page.rs:1027-1030`),
  `PdfPageText` (`text.rs:428-431`) and `PdfBitmap` (`bitmap.rs:444-447`) are all gated on `thread_safe` too. They all hold raw FPDF handles,
  so they are `!Send`.
- **Consequence:** create `Pdfium`, open documents and render, all on ONE dedicated worker thread. Send only owned data (`Vec<u8>`, sizes, `Vec<[f32;4]>`) across channels.
- Bindings are stored in a global `static BINDINGS: OnceCell<Box<dyn PdfiumLibraryBindings>>` (`src/pdfium.rs:70`). `trait PdfiumLibraryBindings: Send + Sync` (`src/bindings.rs:160`).
  You can bind only once per process. A second `bind_*` call returns `Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized)` (`src/pdfium.rs:183`).
  `Pdfium::new` calls `FPDF_InitLibrary` once (`:212-222`).

### Loading the library (`src/pdfium.rs`)
```rust
pub fn bind_to_library(path: impl AsRef<Path>) -> Result<Box<dyn PdfiumLibraryBindings>, PdfiumError>   // :173
pub fn bind_to_system_library() -> Result<Box<dyn PdfiumLibraryBindings>, PdfiumError>                 // :125
pub fn pdfium_platform_library_name() -> OsString        // :197, = libloading::library_filename("pdfium") -> "libpdfium.so" on Linux
pub fn pdfium_platform_library_name_at_path(path: &(impl AsRef<Path> + ?Sized)) -> PathBuf  // :206, = path.join("libpdfium.so")
pub fn new(bindings: Box<dyn PdfiumLibraryBindings>) -> Self   // :212
impl Default for Pdfium  // :600, bind_to_system_library().expect(..) -> PANICS, do not use
```
- `bind_to_system_library` calls `libloading::Library::new("libpdfium.so")`, which is dlopen with a bare name. It searches LD_LIBRARY_PATH, rpath, ldconfig cache and /usr/lib64.
- `bind_to_library(p)` calls dlopen on the exact path. Typical code:
  `Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&exe_dir)).or_else(|_| Pdfium::bind_to_system_library())`.
- Errors: `PdfiumError` (`src/error.rs:48`) implements `Display` (`:256`) and `std::error::Error` (`:262`), so it works with `anyhow`.
  Relevant variants: `LoadLibraryError(libloading::Error)` (:59), `PdfiumLibraryBindingsAlreadyInitialized` (:69), `IoError(std::io::Error)` (:244),
  `PdfiumLibraryInternalError(PdfiumInternalError)` (:253), `TextSearchTargetIsEmpty`.
  `PdfiumInternalError` (`:26-44`): `FileError`, `FormatError`, `PasswordError`, `SecurityError`, `PageError`, `Unknown`.
  Map `PasswordError` to the "password required" UI.

### Opening (`src/pdfium.rs`)
```rust
pub fn load_pdf_from_file<'a>(&'a self, path: &(impl AsRef<Path> + ?Sized), password: Option<&str>) -> Result<PdfDocument<'a>, PdfiumError> // :371
pub fn load_pdf_from_byte_vec(&self, bytes: Vec<u8>, password: Option<&str>) -> Result<PdfDocument<'_>, PdfiumError>  // :330 (doc owns bytes)
pub fn load_pdf_from_byte_slice<'a>(&'a self, bytes: &'a [u8], password: Option<&str>) -> Result<PdfDocument<'a>, PdfiumError> // :313
```
- `PdfDocument<'a>` borrows `&'a Pdfium` (`document.rs:158,176`). Keep the `Pdfium` and its documents in the same owner. On the worker thread, use a local
  `let pdfium = ...;` and keep documents in a loop scope below it. Do not put both in one struct. That needs self-borrowing.
- `load_pdf_from_file` streams through a `File` reader (`:378` uses `load_pdf_from_reader`). It loads lazily.
- `PdfDocument::pages(&self) -> &PdfPages<'a>` (`document.rs:302`).
- `PdfPages` (`document/pages.rs`): `type PdfPageIndex = c_int` (i32) (:26). `len(&self) -> PdfPageIndex` (:97), `is_empty` (:103),
  `get(&self, index: PdfPageIndex) -> Result<PdfPage<'a>, PdfiumError>` (:124).
  `page_size(&self, index) -> Result<PdfRect, _>` (:151) and `page_sizes(&self) -> Result<Vec<PdfRect>, _>` (:180) do NOT load a full page.
  Use them for layout of all pages at open time.
- `PdfPage<'a>` (`document/page.rs:177`): `width(&self) -> PdfPoints` (:248), `height(&self) -> PdfPoints` (:255), `page_size(&self) -> PdfRect` (:261).
  These come from `FPDF_GetPageWidthF/HeightF`, which include the /Rotate rotation.
- `PdfPoints { pub value: f32 }` (`points.rs:18-19`). For f32 use `page.width().value`.
- `PdfRect` (`rect.rs:22`): private fields. Accessors are `left() right() bottom() top()` -> `PdfPoints` (:119-137), plus `width() height()` (:143,149).
  **Origin is bottom-left, y up** (doc comment `rect.rs:16-18`).
- `PdfPage<'a>` is not tied to the `&PdfPages` borrow, only to `'a` (the Pdfium lifetime). Still, drop pages before their document. The page keeps
  `FPDF_DOCUMENT`/`FPDF_PAGE` handles (`page.rs:178-179`).

### Rendering to RGBA without `image`
```rust
// document/page.rs
pub fn render_with_config(&self, config: &PdfRenderConfig) -> Result<PdfBitmap<'_>, PdfiumError>              // :569
pub fn render_into_bitmap_with_config(&self, bitmap: &mut PdfBitmap, config: &PdfRenderConfig) -> Result<(), PdfiumError> // :620 (reuse buffer)
pub fn render(&self, width: Pixels, height: Pixels, rotation: Option<PdfPageRenderRotation>) -> Result<PdfBitmap<'_>, _>  // :541
```
`PdfRenderConfig` (`document/page/render_config.rs`). It is a builder and every method takes `self` by value:
- `new()` (:76). Defaults (:76-117): `format: PdfBitmapFormat::BGRA`, clear before render = true, clear color = `PdfColor::WHITE`,
  `do_render_form_data: true`, `render_annotations: true`, `use_lcd_text_rendering: false`, **`reverse_byte_order: true`**.
- Size: `set_target_width(Pixels)` (:197) and `set_target_height(Pixels)` (:210). Each one alone keeps the aspect ratio. Calling both (= `set_target_size`, :187) does not.
  `scale_page_by_factor(f32)` (:251): pixels = points × factor. Also `set_maximum_width/height` (:283,291) and `set_fixed_size` (:158).
  `scale_page_to_display_size` (:236) auto-rotates landscape pages. **Avoid it.**
- `set_format(PdfBitmapFormat)` (:360), `render_form_data(bool)` (:393), `render_annotations(bool)` (:402), `use_lcd_text_rendering(bool)` (:413),
  `set_reverse_byte_order(bool)` (:510), `set_clear_color(PdfColor)` (:380), `clear_before_rendering(bool)` (:370),
  `set_text_smoothing / set_image_smoothing / set_path_smoothing(bool)` (:478-496), `use_grayscale_rendering` (:433), `clip(...)` (:655).
- `type Pixels = i32` (`bitmap.rs:55`). `enum PdfBitmapFormat { Gray, BGR, BGRx, BGRA }`. The default is `BGRA` (`bitmap.rs:59-63,108`).
- `PdfBitmap`: `width()/height() -> Pixels` (:264,270), `format() -> Result<PdfBitmapFormat,_>` (:276),
  `as_raw_bytes(&self) -> Vec<u8>` (:288, copies the buffer, no conversion), `as_rgba_bytes(&self) -> Vec<u8>` (:294).
  **Pixel format:** with the defaults (BGRA + `reverse_byte_order = true`) pdfium already writes **RGBA**. `as_rgba_bytes` then returns the buffer as-is (:304-312).
  Buffer stride is pdfium-computed (`FPDFBitmap_CreateEx(.., stride 0)`, :140-146). For 4-byte formats that gives `width*4`, so `len == w*h*4`.
  Alpha is 255 (opaque white clear), so `ColorImage::from_rgba_premultiplied` and `from_rgba_unmultiplied` give the same result.
- Recipe (1 PDF pt = 1 egui pt at zoom 1):
```rust
let cfg = PdfRenderConfig::new().set_target_width((page_w_pt * zoom * ppp).round() as i32);
let bmp = page.render_with_config(&cfg)?;
let (w, h) = (bmp.width() as usize, bmp.height() as usize);
let rgba = bmp.as_rgba_bytes();          // len = w*h*4, RGBA
```

### Text search (`document/page/text.rs`, `document/page/text/*.rs`)
```rust
PdfPage::text(&self) -> Result<PdfPageText<'_>, PdfiumError>                                       // page.rs:377
PdfPageText::search(&self, text: &str, options: &PdfSearchOptions) -> Result<PdfPageTextSearch<'_>, PdfiumError> // text.rs:373
PdfPageText::search_from(&self, text, options, index: PdfPageTextCharIndex) -> Result<..>           // text.rs:384
PdfPageText::all(&self) -> String                                                                    // text.rs:246
```
- An empty `text` returns `Err(PdfiumError::TextSearchTargetIsEmpty)` (text.rs:391).
- `PdfSearchOptions` (search.rs:17): `new()` (:24, both false), `Default` (:61), `.match_case(bool)` (:33), `.match_whole_word(bool)` (:41). It is a by-value builder and `Copy`.
- `PdfSearchDirection::{SearchForward, SearchBackward}` (search.rs:70-72).
- `PdfPageTextSearch<'a>` (:77): `find_next()` / `find_previous()` -> `Option<PdfPageTextSegments<'_>>` (:104,111),
  `get_next_result(direction)` (:117), `iter(&self, direction) -> PdfPageTextSearchIterator<'_>` (:146) with `Item = PdfPageTextSegments<'a>` (:182).
- One match is one `PdfPageTextSegments` (segments.rs:28), because a match can span several lines/rects. Use `len() -> usize` (:59), `get(usize)` (:94),
  `iter()` (:158), with `Item = PdfPageTextSegment<'a>` (:180).
- `PdfPageTextSegment::bounds(&self) -> PdfRect` (segment.rs:33). Page points, **bottom-left origin**, so flip:
  `y_top_down = page_h - rect.top().value`.
- Prelude exports search/segment/segments (`lib.rs:176-178`). Use `use pdfium_render::prelude::*;`.
```rust
let text = page.text()?;
let search = text.search("hello", &PdfSearchOptions::new().match_case(false))?;
for m in search.iter(PdfSearchDirection::SearchForward) {
    let rects: Vec<PdfRect> = m.iter().map(|s| s.bounds()).collect();
}
```

---

## 2. egui 0.37 (`R/egui-0.37.0/src`, `R/epaint-0.37.0/src`)

### Input
- `InputState::zoom_delta(&self) -> f32` (input_state/mod.rs:565). This is the multiplicative factor for this frame: 1 = none, >1 = spread/zoom in.
  It uses `multi_touch().zoom_delta` if touch is active, else `zoom_factor_delta` (from ctrl+wheel or `Event::Zoom`).
  `zoom_delta_2d() -> Vec2` (:588) is the non-uniform version.
- Ctrl+wheel: `InputOptions.zoom_modifier` defaults to `Modifiers::COMMAND` (:83,117). The wheel delta is converted to zoom and NOT left in scroll. `scroll_zoom_speed = 1/200` (:113).
- `Event::Zoom(f32)` (data/input/event.rs:137, multiplicative). `Event::MouseWheel { unit, delta, .. }` (:171).
- `InputState.smooth_scroll_delta: Vec2` is a pub field (:242) and also has a `fn smooth_scroll_delta()` (:553). `ScrollArea` reads it and **zeroes** it once consumed.
  **No `raw_scroll_delta` in 0.37** (grep finds none).
- Dropped files: `RawInput.dropped_files: Vec<DroppedFileHandle>` (data/input/raw_input.rs:81). **This changed in 0.37:**
  `type DroppedFileHandle = Arc<dyn DroppedFile + Send + Sync>` (data/input/dropped_file.rs:49).
  `trait DroppedFile { fn path(&self) -> &Path; fn bytes(&self) -> Result<Vec<u8>, String>; }` (:11-35).
  `ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect::<Vec<_>>())`.
  `hovered_files: Vec<HoveredFile>` (raw_input.rs:64), `HoveredFile.path: Option<PathBuf>`.
- Keys (data/key.rs): `Key::PageUp` (:25), `Key::PageDown` (:26), `Key::Home` (:23), `Key::End` (:24), `Key::Escape` (:15), `Key::ArrowUp` (:13),
  `Key::Plus` (:77), `Key::Minus` (:71), `Key::Equals` (:80).
- `Modifiers { ctrl, command, .. }` (data/input/modifiers.rs:19,25,37). `command` = Ctrl on Linux. Constant `Modifiers::COMMAND`.
- `InputState::key_pressed(Key)` (:767), `consume_key(&mut self, Modifiers, Key) -> bool` (:743). Use it via `ctx.input_mut(|i| i.consume_key(..))`.
- `Context::input(|i: &InputState| ..)` (context.rs:1252), `memory_mut` (:1282), `options_mut` (:1404).

### Text field / focus
- `Ui::text_edit_singleline<S: TextBuffer>(&mut self, text: &mut S) -> Response` (ui.rs:1927).
- `Response::lost_focus()` (response.rs:428), `has_focus()` (:405), `request_focus()` (:433), `changed()` (:669).
- `Memory::request_focus(&mut self, id: Id)` (memory/mod.rs:923), `surrender_focus` (:937).
- Enter-to-search: `if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) { .. }`.

### ScrollArea (containers/scroll_area.rs)
- `ScrollArea::vertical()` (:445), `.id_salt(..)` (:552), `.auto_shrink(impl Into<Vec2b>)` (:675), `.animated(bool)` (:684),
  `.scroll_offset(Vec2)` (:565), `.vertical_scroll_offset(f32)` (:578). These force the offset only on the frame you pass them, so pass them only when jumping.
  Also `.scroll_source(ScrollSource)` (:652), `.scroll_bar_visibility(..)` (:535).
- `show_viewport<R>(self, ui: &mut Ui, add_contents: impl FnOnce(&mut Ui, Rect) -> R) -> ScrollAreaOutput<R>` (:1124).
  The `Rect` is the visible region in **content coordinates**: min = (0,0) when not scrolled.
- `ScrollAreaOutput { inner, id, state, content_size, inner_rect }` (:159-174). `state.clamped_offset()` (:121) and `unclamped_offset()` (:134). The `offset` field is private.
- Content size inside `show_viewport`: use `ui.set_min_size(vec2(w, h))` or `ui.allocate_space(..)`. These are long-standing Ui APIs but were not re-grepped in this pass.
  Then paint pages with absolute rects: `ui.min_rect().min + content_pos`.

### Textures & painting
- `Context::load_texture(&self, name: impl Into<String>, image: impl Into<ImageData>, options: TextureOptions) -> TextureHandle` (context.rs:2814).
- `ColorImage::from_rgba_unmultiplied(size: [usize;2], rgba: &[u8])` (epaint image.rs:113) and `from_rgba_premultiplied` (:128).
  Fields: `size: [usize;2]`, `source_size: Vec2`, `pixels: Vec<Color32>` (:50-56).
- `TextureOptions { magnification, minification, wrap_mode, mipmap_mode: Option<TextureFilter> }` (epaint textures.rs:160-178).
  `TextureOptions::LINEAR` (:183) = Linear/Linear/ClampToEdge with `mipmap_mode: None`. `NEAREST` (:191).
  For a downscaled preview during zoom-out, use `TextureOptions { mipmap_mode: Some(TextureFilter::Linear), ..TextureOptions::LINEAR }`.
- `TextureHandle::id()` (texture_handle.rs:64), `set(&mut self, image, options)` (:70, replaces the texture and keeps the id), `size() -> [usize;2]` (:90), `size_vec2()` (:98).
  When the handle is dropped, the texture is freed.
- `Painter::image(&self, texture_id: TextureId, rect: Rect, uv: Rect, tint: Color32) -> ShapeIdx` (painter.rs:447).
  Full uv: `Rect::from_min_max(pos2(0.,0.), pos2(1.,1.))`, tint `Color32::WHITE`.

### Context / threads / viewport
- `Context` is `Send + Sync` (compile-time assert in context.rs:4817-4818). Clone it into the worker thread or pinch thread.
  Call `request_repaint()` (:2128) or `request_repaint_after(Duration)` (:2179) from any thread.
- `ctx.send_viewport_cmd(ViewportCommand::Title(String))` (context.rs:4549; viewport.rs:1096). Also `ViewportCommand::Close` (:1090), `Icon(Option<Arc<IconData>>)` (:1162), `Focus` (:1175).
- `ViewportBuilder` (viewport.rs): `with_title` (:361), `with_icon(impl Into<Arc<IconData>>)` (:440), `with_inner_size` (:537), `with_min_inner_size` (:550),
  `with_drag_and_drop(bool)` (:606), `with_app_id(impl Into<String>)` (:651). This is the Wayland app_id. Match it to the `.desktop` file name / `StartupWMClass`.
- `IconData { pub rgba: Vec<u8>, pub width: u32, pub height: u32 }` (viewport.rs:189-197).
- `eframe::NativeOptions.viewport: egui::ViewportBuilder` (eframe epi.rs:306).

### Theming (egui style.rs, epaint)
- `egui::Theme { Dark, Light }` (memory/theme.rs:6). `ThemePreference { Dark, Light, #[default] System }` (:67-74). `From<Theme> for ThemePreference`.
- `ctx.set_theme(impl Into<ThemePreference>)` (context.rs:2594), `ctx.theme() -> Theme` (:2582),
  `ctx.style_mut_of(Theme, impl FnOnce(&mut Style))` (:2661), `set_style_of(Theme, impl Into<Arc<Style>>)` (:2674), `set_visuals_of(Theme, Visuals)` (:2691).
- `Visuals` (style.rs:1099). Main pub fields: `dark_mode, override_text_color: Option<Color32>, widgets: Widgets, selection: Selection, hyperlink_color,
  faint_bg_color, extreme_bg_color, text_edit_bg_color: Option<Color32>, code_bg_color, warn_fg_color, error_fg_color, window_corner_radius: CornerRadius,
  window_shadow: Shadow, window_fill, window_stroke: Stroke, menu_corner_radius: CornerRadius, panel_fill, popup_shadow: Shadow, button_frame,
  striped, handle_shape, interact_cursor, disabled_alpha, weak_text_alpha, weak_text_color: Option<Color32>, ..`.
  Constructors: `Visuals::dark()` (:1608) and `Visuals::light()` (:1675).
- `Widgets { noninteractive, inactive, hovered, active, open: WidgetVisuals }` (:1360).
  `WidgetVisuals { bg_fill, weak_bg_fill, bg_stroke: Stroke, corner_radius: CornerRadius, fg_stroke: Stroke, expansion: f32 }` (:1400).
  Button background = `weak_bg_fill`.
- `Selection { bg_fill: Color32, stroke: Stroke }` (:1304).
- **`CornerRadius`** (not `Rounding`. There is no `Rounding` alias in epaint 0.37): `{ nw, ne, sw, se: u8 }` (epaint corner_radius.rs:13). `CornerRadius::same(u8)` (:59, const).
- `Shadow { offset: [i8;2], blur: u8, spread: u8, color: Color32 }` (epaint shadow.rs:10-26).
- `Spacing` (style.rs:386): `item_spacing: Vec2, button_padding: Vec2, window_margin: Margin, menu_margin: Margin, indent, interact_size: Vec2, text_edit_width, scroll: ScrollStyle, ..`.

---

## 3. eframe 0.37 `CreationContext` (`R/eframe-0.37.0/src/epi.rs`)
- Public: `egui_ctx: egui::Context` (:58), `integration_info` (:61), `storage: Option<&dyn Storage>` (:64), `gl: Option<Arc<glow::Context>>` (:71, glow), `get_proc_address` (:75).
- `raw_window_handle: Result<RawWindowHandle, HandleError>` (:92) and `raw_display_handle: Result<RawDisplayHandle, HandleError>` (:96) are **`pub(crate)`**. Your code cannot read these fields.
  Use the trait impls instead: `impl HasDisplayHandle for CreationContext` (:110-115) and `impl HasWindowHandle` (:101-105).
```rust
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
if let Ok(dh) = cc.display_handle() {
    if let RawDisplayHandle::Wayland(h) = dh.as_raw() {   // rwh lib.rs:312
        let display: *mut std::ffi::c_void = h.display.as_ptr();   // WaylandDisplayHandle { display: NonNull<c_void> } unix.rs:156-158
    }
}
```
- `run_native(app_name: &str, NativeOptions, AppCreator<'_>) -> eframe::Result` (lib.rs:288).
  `type AppCreator<'app> = Box<dyn 'app + FnOnce(&CreationContext<'_>) -> Result<Box<dyn 'app + App>, DynError>>` (epi.rs:49).

---

## 4. Wayland pinch on winit's connection

### Backend / features
- winit 0.30.13 already depends on `wayland-backend` with `features = ["client_system"]` (`R/winit-0.30.13/Cargo.toml:275-278`). The sys (libwayland) backend is therefore
  already the active backend for the single shared wayland-backend 0.3.17. Cargo features unify, so the backend is chosen once per crate copy.
  Nothing changes for winit.
- To be explicit, set `wayland-client = { version = "0.31", features = ["system"] }` (wayland-client Cargo.toml: `system = ["wayland-backend/client_system"]`).
  You need no direct wayland-backend dep. `wayland_client::backend::Backend` re-exports `wayland_backend::client::Backend` (wayland-client lib.rs:186-190).
- Linking: egui-winit uses `winit` with `default-features = false` (egui-winit Cargo.toml:133-135), and eframe's `wayland` feature does not enable `wayland-dlopen`.
  So wayland-sys 0.31.11 links libwayland-client at build time via pkg-config (wayland-sys build.rs:1). That needs `wayland-devel`, and `/usr/lib64/libwayland-client.so` exists.
  At runtime it needs `libwayland-client.so.0`, which Fedora has.

### Foreign display
```rust
// wayland-backend sys/mod.rs:85-104, cfg(feature = "client_system")
pub unsafe fn from_foreign_display(display: *mut wayland_sys::client::wl_display) -> Backend
```
- "Guest" mode: dropping it does not close the connection (sys/mod.rs:92-96).
  **Safety:** the `wl_display` must outlive the Backend and all its clones (:98-101).
- It creates its **own** libwayland event queue (`wl_display_create_queue`, sys/client_impl/mod.rs:274-277). It never dispatches winit's queue.
- Call it as `Backend::from_foreign_display(h.display.as_ptr().cast())`. `.cast()` infers the `wl_display` type, so you need no wayland-sys import.
- `NonNull<c_void>` is `!Send`. Build the `Connection` in the eframe creator closure (on the UI thread), then move the `Connection` into the thread. `Backend` is Arc+Mutex and Send.

### wayland-client API (`R/wayland-client-0.31.15/src`)
- `Connection::from_backend(backend: Backend) -> Connection` (conn.rs:111), `display() -> WlDisplay` (:100), `new_event_queue<State>() -> EventQueue<State>` (:106),
  `flush()` (:124), `roundtrip()` (:148).
- `EventQueue::handle() -> QueueHandle<State>` (event_queue.rs:375), `dispatch_pending(&mut State)` (:387),
  `blocking_dispatch(&mut self, &mut State) -> Result<usize, DispatchError>` (:398, flushes and then blocks), `roundtrip(&mut State)` (:420).
- **Multi-thread multi-queue is supported:** see the event_queue.rs doc "Multi-thread multi-queue app … one EventQueue per thread … each thread can use
  `EventQueue::blocking_dispatch()` on its own event loop, and everything will 'Just Work'". `ReadEventsGuard` (backend client_api.rs:308-323) makes sure only one thread does the socket read,
  using libwayland `prepare_read_queue`/`read_events`. winit/calloop reads through the same mechanism.
  Running `blocking_dispatch` in a dedicated thread is therefore safe.
- `delegate_noop!(State: Iface)` (event_queue.rs:847) implements `Dispatch<Iface, ()>` and **`unreachable!()`s on any event** (doc :845).
  Use it only for `ZwpPointerGesturesV1`, which has no events. Write `wl_pointer`, `wl_seat`, `wl_registry` and pinch `Dispatch` impls by hand.
- `Dispatch::event(state: &mut Self, proxy: &I, event: I::Event, udata: &U, conn: &Connection, qh: &QueueHandle<Self>)` (doc example event_queue.rs:73-80).

### Protocol objects
- Module: `wayland_protocols::wp::pointer_gestures::zv1::client::{zwp_pointer_gestures_v1::ZwpPointerGesturesV1, zwp_pointer_gesture_pinch_v1::{self, ZwpPointerGesturePinchV1}}`
  (wp.rs:333-341). This needs wayland-protocols features `client` + `unstable`, which Cargo.toml already has.
- Global `zwp_pointer_gestures_v1` is at version 3 (xml:28). `get_pinch_gesture(pointer: wl_pointer)` (xml:57-63). Generated as
  `gestures.get_pinch_gesture(&pointer, &qh, ())`.
- Pinch events (xml:174-219). In the generated code `fixed` becomes f64, `int` becomes i32 and `uint` becomes u32:
  - `Event::Begin { serial: u32, time: u32, surface: WlSurface, fingers: u32 }`
  - `Event::Update { time: u32, dx: f64, dy: f64, scale: f64, rotation: f64 }`. **`scale` is absolute relative to Begin**: 2.0 means the fingers are twice as far apart (xml:193-195).
    `rotation` is a relative angle in degrees (xml:197-198). Per-event factor = `scale / last_scale`. Reset `last_scale = 1.0` on Begin.
  - `Event::End { serial: u32, time: u32, cancelled: i32 }`
- `Begin.surface` is winit's surface. Its proxy carries the shared `RUST_MANAGED` marker (sys/client_impl/mod.rs:961), so it resolves to winit's object without panicking.
  Ignore that field.
- You need your own `wl_pointer`: `seat.get_pointer(&qh, ())` after `wl_seat::Event::Capabilities` contains `Capability::Pointer`.
  A second `wl_pointer` on the same seat is allowed by the core protocol, and each pointer object receives the events. Your pointer also gets enter/leave/motion/button/axis/frame,
  so ignore them in `Dispatch<WlPointer, ()>`.
- Bind flow: `conn.display().get_registry(&qh, ())`, then `queue.roundtrip(&mut st)`. In the registry `Global{name, interface, version}` handler, bind
  `"wl_seat"` and `"zwp_pointer_gestures_v1"` with `registry.bind::<I, _, _>(name, version.min(N), qh, ())`.
  `registry.bind` is generated code and was not re-grepped this pass. Once both the pointer and the gestures object exist, call `get_pinch_gesture`.
  Then run `loop { queue.blocking_dispatch(&mut st)?; }`.
  Forward the factor to the UI through a channel or `Arc<Mutex<f32>>` plus `ctx.request_repaint()`.
- Shutdown caveat: once winit drops its connection at exit, the display pointer dangles. Keep the pinch thread detached and return or `exit` right after `run_native`.
  Never touch the `Connection` after that.

---

## 5. Test PDF fixture
- `R/pdfium-render-0.9.4/test/` contains only `Lato-Regular.ttf`. There are no PDF fixtures to copy.
- pdfium can rebuild a broken xref (parser fallback). That is pdfium C++ behaviour and cannot be verified in this crate. Instead, generate correct offsets in the test helper:
```rust
fn tiny_pdf(pages: &[&str]) -> Vec<u8> {
    let n = pages.len();
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        format!("<< /Type /Pages /Kids [{}] /Count {n} >>",
            (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect::<Vec<_>>().join(" ")),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    ];
    for (i, text) in pages.iter().enumerate() {
        let content = format!("BT /F1 24 Tf 72 720 Td ({text}) Tj ET");
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
            /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>", 5 + 2 * i));
        objs.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
    }
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::with_capacity(objs.len());
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes()); // exactly 20 bytes per entry
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objs.len() + 1).as_bytes());
    out
}
// tiny_pdf(&["Hello", "World"]) -> load_pdf_from_byte_vec(bytes, None)
// Expect 2 pages, each 612x792 pt, and search "Hello" -> 1 hit on page 0.
```
- Helvetica is a standard-14 font. pdfium has the standard-14 fonts built in, so rendering and text extraction need no system fonts.
- Tests need `libpdfium.so` at runtime. Skip them when binding fails, so CI without the lib passes.
