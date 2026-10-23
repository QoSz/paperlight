# Follow-up brief: kinetic touchpad scrolling + text selection

User feedback after the first install (2026-10-23):

1. "The scroll still feels really slow." The user scrolls with the XPS touchpad on GNOME Wayland.
2. "I can't select text."

## Verified facts (crate sources, do not re-derive)

- winit 0.30.13 Wayland (`platform_impl/linux/wayland/seat/pointer/mod.rs:176-210`): touchpad
  scrolling arrives as `MouseScrollDelta::PixelDelta` with `TouchPhase::Started/Moved`, and
  `wl_pointer.axis_stop` (finger lift) arrives as `TouchPhase::Ended`. Mouse wheels arrive as
  `LineDelta`.
- egui-winit converts these to `egui::Event::MouseWheel { unit: Point | Line, delta, modifiers, phase }`.
  Pixel deltas are already divided by `pixels_per_point`.
- egui 0.37 applies `Point` deltas 1:1 (no inertia on Linux; momentum detection is macOS-only, see
  `egui/src/input_state/wheel_state.rs`). GTK apps have kinetic scrolling after finger lift. That
  missing inertia is why scrolling feels slow and dead compared to Evince.
- `eframe::App::raw_input_hook(&mut self, ctx, raw_input: &mut RawInput)` (`eframe/src/epi.rs:282`)
  runs before egui processes events; `raw_input.events` can be read and edited there.
- `App` already sets `line_scroll_speed` per frame from `layout::wheel_notch_points` (GTK notch size).
- pdfium-render 0.9.4: `PdfPage::text()? -> PdfPageText`, `text.chars()` iterates `PdfPageTextChar`
  with `unicode_char() -> Option<char>` and `loose_bounds() -> Result<PdfRect>` (bottom-left origin,
  page points; flip exactly as the search code does). `chars().len()` gives the count.
- Clipboard: enable eframe feature `clipboard` (arboard, with `wayland-data-control` via the
  `wayland` feature). Then `ctx.copy_text(String)`.

## Measured on the user's XPS touchpad (2026-10-23)

Same swipe logged in a GTK4 window and an egui window: per-event deltas are identical (6-8 pt per
event at ~125 Hz, ~200 pt per half-second swipe). On finger lift GTK's `EventControllerScroll`
emits `decelerate` with |vy| = 1000-1500 px/s and the scrolled window coasts; egui stops dead.
The velocity to use is the EMA of `dy/dt` over the last ~50 ms before `End`.

## Deliverable 1: kinetic (fling) scrolling

- In `raw_input_hook`, for each `Event::MouseWheel { unit: MouseWheelUnit::Point, delta, phase, modifiers }`
  with no Ctrl: track vertical velocity (points/s) as an exponential moving average of
  `delta.y / dt` using `raw_input.time`. Keep the last ~3 samples' worth; ignore the first sample
  after `Started`.
- On `phase == TouchPhase::End` with |velocity| above ~80 pt/s: start a fling in `ViewerState`
  (`fling_v: Option<f32>`). Any new `MouseWheel` event, key navigation, `go_to_page`, `reveal`,
  or zoom cancels the fling.
- Each frame while flinging (in `viewer::show`): `dy = v * dt`, `v *= exp(-DECEL * dt)` with
  `DECEL = 4.0 /s` (GTK's kinetic friction; coast distance = v0/4), stop when |v| < 10 pt/s or when the
  offset hits the clamp. Apply through the existing `pending_offset` path and `ctx.request_repaint()`
  while flinging. Must not repaint when idle.
- Touchpad speed itself stays 1:1 with the system. Add `pub const TOUCHPAD_SCROLL_MULTIPLIER: f32 = 1.0`
  in `layout.rs` applied to Point deltas in the hook only if it is cheap (one multiply); keep 1.0.
- Tests: pure velocity/decay math in `layout.rs` (or a tiny `fling.rs`): decay reaches < 10 pt/s
  within ~1.6 s from 500 pt/s; cancelled fling is None; EMA ignores the first sample.

## Deliverable 2: text selection + copy

- Worker: `Cmd::Text { page }` → `Msg::Text { page, chars: Vec<CharBox> }`,
  `CharBox { rect: egui::Rect /* page points, top-left origin */, ch: char }`. Chars without a
  unicode char are skipped. Use `loose_bounds`. Requested lazily by the viewer the first time the
  pointer presses on a page (and for every page in a multi-page selection); cached in `App` as
  `HashMap<usize, Arc<[CharBox]>>`, cleared on open. No eviction (a page's chars are a few KB).
- Viewer: primary-button drag without Ctrl, starting over a page rect, starts a selection:
  `anchor = (page, index of nearest char by distance from point to char rect, capped at ~12 pt
  else None)`. While dragging, `head = (page under pointer, nearest char)`; if the pointer is in
  the gap between pages, use the nearest page edge (last char of the page above, or first char of
  the page below). Selection = reading-order range from min to max of (page, index). A click
  without movement clears the selection. Keyboard: Ctrl+C copies, Esc clears (Esc already clears
  search; do both).
- Painting: for selected chars, fill `pal.selection_bg` at ~55% alpha over each char rect merged
  per text line (merge chars whose rects vertically overlap > 50% and are adjacent in index into
  one run rect; this avoids ragged per-glyph boxes). Selected text stays readable.
- Copy string: for each page in range, chars in range joined; pdfium already includes `\r\n` /
  `\n` chars in the stream, keep them; insert `\n` between pages. `ctx.copy_text`.
- Cursor: `CursorIcon::Text` when hovering a char; default otherwise. Cheap: hit test only when
  the pointer moved.
- Keep selection in doc space independent of zoom (it is char indices, so it survives zoom).
- Tests: pure selection range logic (`search`-style module `selection.rs`): ordering across pages,
  nearest-char pick, run merging, copy string assembly.

## Non-goals

Double-click word select, triple-click line select, right-click menu, selection in rotated pages.
