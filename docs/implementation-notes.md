# Paperlight: implementation notes (phase 5)

This file lists every place where the implementation differs from `docs/architecture.md`,
with the reason, and records the title-bar change requested during implementation.

## Deviations from the contract

| # | Contract | Implementation | Reason |
|---|---|---|---|
| 1 | `ViewerState::settled(now) -> bool`; `pub const ZOOM_SETTLE`; app.rs calls `request_repaint_after(ZOOM_SETTLE - (now - changed_at))` | `ViewerState::settle_remaining(now) -> Option<Duration>` (`None` = settled); `ZOOM_SETTLE` is private | app.rs needs the remaining delay, but `zoom_changed_at` is private to the viewer. One method returns both facts and keeps the settle rule in one place. |
| 2 | A9 steps 5–7: one `wanted` computed **before** `viewer::show` is used for painting, renders and retain | Painting uses the pre-show value. `request_renders` and `retain` use `wanted` recomputed **after** `viewer::show` | `viewer::show` applies the fit-width zoom. On every open, `viewer.zoom()` is still the old zoom (1.0 on the first open) until `show` runs. With the contract's order, the visible pages were requested at the old bucket and then again at the fit bucket 100 ms later: two renders per page and a visible soft-to-sharp flash. With the new order, the smoke run renders each page once, at the fit bucket (1.25). |
| 3 | `request_renders(&mut self, ctx, out)` | `request_renders(&mut self, ctx, out, wanted, pages)` | `wanted` comes from #2. `pages` is passed because `ui()` still borrows `self.doc` at the call. |
| 4 | `ViewerState::reset` sets `fit_width`, `layout`, `pending_offset`, `zoom_changed_at` | It also clears `anchor` | A pointer anchor left over from the previous document would otherwise be applied at the next relayout (a window resize, for example). |
| 5 | On `Opened`, app.rs does not change `current_page` | It sets `current_page = 0` | Without this, the toolbar showed the old document's page number for one frame, and a search started before the first canvas frame would use a stale `from_page`. |
| 6 | `TextEdit::horizontal_align(Align::Center)`, `Id::new(..)` | `.align(Align2::CENTER_CENTER)`, `Id::unique(..)` | Both are deprecated in egui 0.37 (`text_edit/builder.rs:416`, `id.rs:101`). With `-D warnings`, the build fails on deprecations. `Id::unique` is the same hash as `Id::new`. In a single-line field, vertical alignment has no visible effect. |
| 7 | File name label: `Label::new(name).truncate()` plus `.on_hover_text(name)` | `Label::new(name).truncate()` only | egui 0.37 labels already show the full text as a tooltip when it is elided (`show_tooltip_when_elided`, default true, `label.rs:284-292`). Adding `on_hover_text` would also show a tooltip on names that are not truncated. |
| 8 | `#[expect(clippy::cast_possible_truncation)]` on the pinch `Dispatch` impl | On the single `let zoom = … as f32;` statement | This is the smallest scope, as §0.1 asks. |
| 9 | `bind_pdfium` error text `tried {list}, system library` | `tried a, b, system library` built as one joined list | With an empty candidate list, the contract's format produced `(tried , system library)`. |
| 10 | Line budget ≤ 1400 non-test lines | 1940 non-test lines including `theme.rs` (142), measured with `cargo fmt` defaults (1621 without comments and blank lines) | The budget assumed the contract's compact style. Default rustfmt puts struct literals and long call chains on several lines (with `use_small_heuristics = "Max"` the same code is 1748 lines). No `rustfmt.toml` was added, to keep the streamline-desktop convention. The code has no dead items, and clippy pedantic is clean. |
| 11 | `ToolbarAction::None`; `toolbar::show -> ToolbarAction` | No `None` variant; `show -> Option<ToolbarAction>` | QA pass: the variant duplicated `Option`, which the toolbar's group functions already return. |
| 12 | `struct PinchEvent { zoom: f32 }`; `Receiver<PinchEvent>` | `Receiver<f32>` | QA pass: a one-field wrapper with a single consumer. The meaning is documented on `pinch_wayland::start`. |
| 13 | Pinch `Update` guard `scale > 0 && last_scale > 0` | `scale > 0` | QA pass: `last_scale` is only ever 1.0 or a previous positive `scale`, so the second check cannot fail. |
| 14 | `PageLayout::current_page`: page under the viewport centre | Page under the point one third down the viewport (`view.min.y + view.height() / 3`); the top and bottom rules are unchanged | GOD review (G1): with short pages, the centre is one page past the page that `go_to_page` puts at the top, so "go to page N" showed N+1. |

Not deviations, but worth knowing:
- `render` and `search_page` in worker.rs return `Result<_, Box<dyn Error>>`, because each function mixes `TryFromIntError` (page index to `i32`) with `PdfiumError`. They were the crate's only use of `anyhow`, so the QA pass replaced it with the std type and removed the dependency.
- Tests return `Result` instead of calling `unwrap`/`expect`/`panic!`, because those lints are deny/warn for tests too. This matches the streamline-desktop convention.
- `paperlight.desktop` gets a `desktop-file-validate` hint: "more than one main category" (`Office;Viewer;Graphics;`). The categories are kept as specified. Evince's entry uses the same pair.

## Cargo.toml changes

| Change | Reason |
|---|---|
| `anyhow` removed (QA pass) | Its two uses became `Box<dyn std::error::Error>`; nothing else in the dependency tree uses it. |
| `wayland-client = { version = "0.31", features = ["system"] }` | `pinch_wayland.rs` calls `Backend::from_foreign_display`, which exists only with `client_system` (api-notes §4). Before this change, only winit's feature unification enabled it. |
| `winit = { version = "0.30", default-features = false, features = ["wayland-csd-adwaita"] }` | Title bar (next section). The crate is not used directly. Cargo.lock gains 9 packages: sctk-adwaita 0.10.1, tiny-skia 0.11.4, tiny-skia-path, ab_glyph 0.2.32, ab_glyph_rasterizer, owned_ttf_parser, ttf-parser, strict-num, arrayref. |

## Title bar on GNOME Wayland (requirement added during phase 5)

**Problem.** Mutter provides no server-side decorations (no `zxdg_decoration_manager_v1`). eframe's `wayland` feature does not enable winit's `wayland-csd-adwaita`, so the window had no title bar.

**Fix.** Enable `winit/wayland-csd-adwaita` from Cargo.toml (see above). `with_decorations` stays at its default (`true`).

**Theme.** No code was needed:
- `ViewportBuilder` has no theme field in egui 0.37. The runtime path is `ViewportCommand::SetTheme(SystemTheme)`. egui-winit maps it to `window.set_theme(..)` (`egui-winit/src/lib.rs:2122-2127`).
- egui's default `Options::sync_window_theme = true` sends `SetTheme(SystemDefault)` for `ThemePreference::System` (`egui/src/context.rs:2959-2975`). winit then calls `set_theme(None)`.
- For `None`, winit uses `sctk_adwaita::FrameConfig::auto()` (`winit/src/platform_impl/linux/wayland/window/state.rs:1214-1219`). `auto()` reads the XDG Settings portal key `org.freedesktop.appearance color-scheme` (`sctk-adwaita/src/config.rs:5-24`) and picks the dark frame when the value is 1 (prefer-dark).
- On this machine the portal returns `uint32 1`, so the frame is dark. Paperlight itself runs no subprocess. The portal query (through `dbus-send`) is internal to sctk-adwaita.

**Verification.** A `WAYLAND_DEBUG=1` run of the release binary on the sample PDF shows:
- five `wl_subcompositor.get_subsurface` calls (the header bar and the borders);
- `xdg_surface.set_window_geometry(0, -35, 900, …)`, which is the 35 px header;
- `xdg_toplevel.set_title("CA_Kenya_Engagement_Model.pdf — Paperlight")`, which is the text the header shows.

No screenshot was taken: no CLI screenshot tool is installed, and GNOME restricts the Shell screenshot D-Bus API. The look of the buttons and the dark header was not checked by eye.

**Limitations.**
- The header theme is read when the frame is created and when egui sends its one-time `SetTheme`. It does not follow a live light/dark switch.
- winit 0.30 reports no system theme on Linux: both the X11 and Wayland `theme()` return `None` (`x11/window.rs:1905`), and `system_theme()` returns `None` (`linux/mod.rs:909`). egui therefore always uses `fallback_theme = Dark`, so the app's own light palette (`theme::LIGHT`) is never selected on Linux. This predates the title-bar change. Fixing it would need a D-Bus portal client, which the contract excludes (no new deps beyond the listed ones).

## Verification done in this phase

- `cargo test`: 55 tests pass. `worker_end_to_end` passes (it does not skip) with `PAPERLIGHT_PDFIUM` set, and skips cleanly without it.
- Smoke run (release, Wayland, `PAPERLIGHT_LOG=debug`, `~/Downloads/CA_Kenya_Engagement_Model.pdf`, 24 pages of 720×405 pt):
  - The document opens, and pages 0–2 render at `scale_px=1.25` in 5–28 ms.
  - Page 3 (prefetch) renders about 100 ms later, after the zoom settles.
  - No renders follow. Over 3 s idle, the process used 0 CPU ticks (`/proc/<pid>/stat` utime+stime), both on Wayland and on X11 (`WAYLAND_DISPLAY=`).
- `WAYLAND_DEBUG=1` shows `xdg_toplevel.set_app_id("paperlight")` and `zwp_pointer_gestures_v1.get_pinch_gesture`. On X11, the log shows `pinch: not on Wayland` and the app runs normally.

Not verified here (needs a person or system changes):
- Pinch zooming on a real touchpad.
- Drag and drop.
- The interactive toolbar flows: typing a page number, Ctrl+F, Enter/Shift+Enter/Esc.
- Running `install.sh` / `install.sh --uninstall` (not run, by instruction; only `bash -n` and `desktop-file-validate`).

## Verification fixes

Defects found in the regression and verification phase, each fixed at the narrowest layer.

| # | Defect | Evidence | Fix | Regression test |
|---|---|---|---|---|
| V1 | ←/→ skipped pages forward and stuck backward when pages are shorter than half the viewport (landscape slides such as `CA_Kenya_Engagement_Model.pdf`, or zoomed out). The contract's `go_to_page(current ± 1)` uses the centre page, which is one past the page `go_to_page` puts at the top. From page 1 at the top: → went to 3, ← stayed on 1. | Layout maths on 720×405 pages in a ~1000 pt viewport; the test asserts `current_page == 2` with page 1 at the top. | `PageLayout::step_page(y, forward)` (`layout.rs:107`): the first page whose `page_scroll_y` is below the scroll y, or the last one above it. `ViewerState::step_page` (`ui/viewer.rs:105`) feeds it `pending_offset.unwrap_or(offset)`. `app.rs:177-178` uses it for ArrowRight/ArrowLeft. Deviates from the A9 key table on purpose. The toolbar counter no longer follows the viewport centre; see G1 below. | `step_page_values`, `step_page_short_pages_never_skips` (`layout.rs:346`, `:361`) |
| V2 | The page that the worker was rendering got queued again, so it rendered twice. Every new batch resent every in-flight key, including the one being rendered. On open this happens every time the first render takes longer than `ZOOM_SETTLE` (100 ms): the settle batch adds the prefetch page and resends page 0. | Largest PDF in ~/Downloads (40 pages of 1440×810 pt, 58 MB): `rendered page=0 scale_px=0.75` twice (707 ms + 588 ms) before pages 1–3. | `PageCache::start_batch` (`render/cache.rs:49`). If every in-flight key is still wanted, it sends only the new keys under the current generation. It bumps the generation and resends everything only when an in-flight key is no longer wanted (scrolled away or zoomed). `app.rs:249` uses it. After the fix, page 0 renders once (590 ms) and pages 1–3 follow at once. A duplicate is still possible on a cancelling batch (README limitation). | `start_batch_sends_only_new_keys`, `start_batch_new_generation_when_key_unwanted` (`render/cache.rs:158`, `:169`) |
| V3 | The open-error text shown on the canvas was a 7-line Rust debug dump: `PdfiumError`'s `Display` is `{:#?}`, and the contract formats `"Cannot open {name}: {e}"`. | `Cannot open missing.pdf: IoError(\n    Os {\n        code: 2, …` | `open_error_text` (`render/worker.rs:224`): `IoError` shows the io message, `FormatError` shows "not a PDF, or damaged", anything else shows compact `{e:?}`. The password message is unchanged. | `worker_end_to_end` step (d) now asserts `"Cannot open nonexistent.pdf: No such file or directory (os error 2)"` (`render/worker.rs:497`) |
| V4 | A failed open was not logged anywhere (only shown on the canvas), so it could not be diagnosed from a log. | `PAPERLIGHT_LOG=debug` runs on a missing path or a text file printed nothing from paperlight. | `render/worker.rs:126-128` logs the message at `warn` (like `LibraryMissing` and `render failed`) before sending `OpenFailed`. | Manual: `WARN paperlight::render::worker: Cannot open notes.txt: not a PDF, or damaged` |
| V5 | Pinch events were drained only while a document was open. A pinch on the empty or loading window stayed queued and was applied as a zoom on the next document's first frame (it also cleared fit-width). | Code trace (`app.rs` `ui()`). | `app.rs:273-274`: drain the pinch channel every frame; apply only with a document. | none (needs a touchpad) |
| V6 | `install.sh` failed on a HOME without `~/.config`: `xdg-mime default` writes `$XDG_CONFIG_HOME/mimeapps.list` but does not create the directory (`touch: cannot touch …/.config/mimeapps.list`), so the final check aborted the install. | Sandboxed run with a fresh `HOME`. | `install.sh:94`: `mkdir -p "${XDG_CONFIG_HOME:-$HOME/.config}"` before `xdg-mime default`. | Sandboxed install/uninstall run |

## GOD review

Final independent review against the plan, the brief and this file. Each defect is fixed at the
narrowest layer.

| # | Defect | Evidence | Fix | Regression test |
|---|---|---|---|---|
| G1 | After "go to page N" (page field, →, Home) on short pages, the toolbar showed N+1. The counter used the viewport centre, which lies one page below the page at the top when pages are shorter than half the viewport (`CA_Kenya_Engagement_Model.pdf`: 720×405 pt slides at fit width). | Layout maths: default window, page 1 at the top, centre probe in page 2. The new test fails on the old rule (`left: 2, right: 1`). | `PageLayout::current_page` (`layout.rs:88`) probes `view.min.y + view.height() / 3`. Contract change: deviation #14. Pages that cannot reach the top at the end of the document still show the last page (the bottom rule, so End shows N / N). When a page is shorter than about a third of the viewport (zoomed far out), go-to can still show a later page. | `current_page_after_go_to_short_pages` (all 24 pages of the CA_Kenya geometry), `current_page_one_third_down`, `current_page_gap_picks_next` updated (`layout.rs:327-370`) |
| G2 | A failed open (bad file, missing file, dropped non-PDF) kept the previous document's search query, match label ("3 / 17"), textures and scroll state; only a successful open cleared them. | Code trace: the reset ran only in the `Opened` arm. | `App::forget_doc` (`app.rs:141`) holds the reset that `Opened` did and is also called for `OpenFailed`/`LibraryMissing` (`app.rs:113`, `:118`). The reset cannot move into `open()`: when two drops race, the first file's `Opened` and textures arrive after the second `open()`. | none (needs an `eframe` window) |
| G3 | Ctrl+mouse-wheel zoomed about 1.7× per notch. egui zooms by `exp(scroll_zoom_speed × points)` and the GTK wheel speed made a notch about 100 pt (`exp(100 / 200)` = 1.65 at a 1000 pt viewport, more on a taller one). | egui 0.37 `input_state/mod.rs:465`, `wheel_state.rs:145`. | `layout::wheel_zoom_speed` (`layout.rs:166`): `ln(ZOOM_STEP) / notch`, set next to `line_scroll_speed` (`app.rs:348`). One notch is now one zoom step (1.25×, the same as Ctrl+=). Touchpad Ctrl+scroll (pixel deltas) is slower than egui's default; pinch is the touchpad path. | `wheel_zoom_notch_is_one_step` (`layout.rs:448`) |
| G4 | The page counter could stay stale after a scroll that no input event follows. The toolbar is drawn before the canvas, and egui only runs a follow-up frame after input. A search result arriving from the worker scrolled to the hit's page while the counter kept the old number until the next mouse move or cursor blink. | egui 0.37 `input_state/mod.rs:676-685` (repaint only for events, pointer or wheel). | `app.rs:338-343`: `request_repaint()` when `current_page` changed this frame. This is one extra frame per page change, and it cannot loop. | none (frame-loop behaviour) |
| G5 | With no document, or a PDF with 0 pages, the page field showed "1 / 0". | Code: `(current_page + 1).to_string()`. | `toolbar.rs:101`: `(current_page + 1).min(total_pages)`, so both cases show "0 / 0". The controls stay disabled. | none (UI) |

Checked and rejected (no change):
- **Several `Page` messages uploaded in one frame.** At fit width a texture is about 2 MB and uploads in well under 1 ms. At high zoom a render takes 100 ms or more, so the messages arrive in separate frames anyway. A per-frame upload limit would add state for no visible gain.
- **`std::process::exit` in `on_exit`.** eframe calls `on_exit` from `save_and_destroy` on every exit path (close, error, `LoopExiting`) while winit is still alive, so the pinch thread never sees a closed display. Nothing needs flushing. `tracing_subscriber::fmt()` writes to **stdout**, not stderr. Rust's stdout is a `LineWriter`, each event is one complete line, and `process::exit` runs the std stdout cleanup. Threads end with the process, so no zombie is possible. One side effect: a run that eframe ends with an error exits with code 0 (the error is still logged).
- **Kinetic scroll momentum continues after Home/End/go-to.** egui's `scroll_offset` does not reset the velocity. This is egui behaviour, and resetting it would mean editing egui's stored `ScrollArea` state.
- **A pending offset applied after a relayout in the same frame** (a key press in the exact frame of a window resize) lands relative to the old layout. It is a one-frame race with no panic, and the next scroll corrects it.
- **`PageCache::in_flight` and some `SearchState` fields are `pub` but only used inside their modules.** This is an encapsulation nit, not a defect, so it is left as is.
- **install.sh** is idempotent (`install -D`, hash-checked skip of the download) and uses `set -euo pipefail`. Downloads go to `mktemp -d`, removed by an `EXIT` trap. Both the archive and the `.so` are SHA-256 checked before use; the pinned hashes match the bblanchon `chromium/8111` files. Missing tools are reported before any change. `rm -rf` only touches `~/.local/lib/paperlight` and the temp dir. `--uninstall` restores Evince through `xdg-mime` and never deletes `mimeapps.list`. The desktop entry has `%f`, an absolute `Exec`, `StartupWMClass=paperlight` and `MimeType=application/pdf;`.

Residual risks:
- Touchpad pinch on a real touchpad is still unverified by hand.
- Memory at extreme zoom: one page is one texture (no tiling). An A0 page at 600% is about 0.5 GB of RGBA, and on a 2× HiDPI screen a Letter page at 600% is about 125 MB. The GPU-size cap (`max_scale_px`) prevents the egui_glow assert but not the memory use.
- Doc-space coordinates are `f32` (egui). On documents thousands of pages long at high zoom, positions near the end lose sub-point precision.
- `install.sh` writes `Exec=<absolute path> %f` unquoted. A `$HOME` that contains spaces would need quoting.


## Round 2: kinetic scrolling and text selection

Scope: `docs/followup-brief.md` (two deliverables). New pure modules: `src/fling.rs` (velocity
tracker and decay) and `src/selection.rs` (char picking, ranges, highlight runs, copy text).

### Deviations from the brief

| # | Brief | Implementation | Reason |
|---|---|---|---|
| R1 | eframe feature `clipboard` (arboard with `wayland-data-control`) | eframe features unchanged. A direct `egui-winit = { version = "0.37", default-features = false, features = ["smithay-clipboard"] }` enables only the smithay backend (the implicit optional-dependency feature). | GNOME has no data-control protocol. With `clipboard`, every launch logged `WARN arboard::platform::linux: Tried to initialize the wayland data control protocol clipboard, but failed. Falling back to the X11 clipboard protocol`. arboard then opened an X11 connection to Xwayland and started a server thread (`arboard-3.7.0/src/platform/linux/x11.rs:938-955`). On Wayland, egui-winit's `set_text` uses smithay-clipboard first and never reaches arboard (`egui-winit-0.37.0/src/clipboard.rs`, `set_text`), so arboard was unused. Trade-off: in an X11 session, `copy_text` falls back to egui-winit's in-app clipboard (README limitation). To restore X11 copy, add `"clipboard"` to the eframe features; the startup warning comes back with it. |
| R2 | `ViewerState.fling_v: Option<f32>` | `fling: Option<Fling>` (velocity plus the frame time of the last step) | `dt` comes from frame times, so the first step runs in the lift frame with `dt = 0`. Using `unstable_dt` there would replay the last finger frame (about 20 pt at 1200 pt/s). |
| R3 | `dy = v·dt`, then `v *= exp(-DECEL·dt)` | `dy = v·(1 − exp(−DECEL·dt)) / DECEL` (exact integral), same `v` update | The total coast is exactly `v0 / DECEL`, independent of frame rate. Euler adds about `v0·dt/2` and overshoots after a late frame. Test: `step_is_frame_rate_independent`. |
| R4 | EMA of `delta.y / dt` per event; ignore the first sample after `Started` | One sample per frame: the summed `Move` deltas of a frame over the time since the previous sampled frame. Weight 0.5 (about 3 frames, about 50 ms). The `Start` frame only sets the time base. `Start`/`End` deltas are not sampled. | eframe stamps every event of a frame with the same `raw_input.time` (`epi_integration.rs:421`), so per-event `dt` is 0. egui ignores the deltas of `Start` and `End` events when scrolling (`wheel_state.rs`), so they are not part of the motion. |
| R5 | Fling on `End` when `|v| > ~80 pt/s` | Also no fling when the lift comes more than 100 ms after the last motion | A finger that rests before lifting must not fling. GTK gets the same effect from its 150 ms scroll history window (`scroll_history_finish`). Test: `slow_paused_or_unstarted_lift_does_not_fling`. |
| R6 | (not specified) | A fling starts only if the pointer is over the canvas viewport | egui scrolls the `ScrollArea` only while the pointer is over it. A swipe over the toolbar did not scroll the pages, so it must not fling them. Test: `cancelled_fling_is_none`, last case. |
| R7 | Anchor and head as `(page, char index)` | The anchor is stored as a page point and the head as a `Probe` (`At(page, point)`, `Start(page)`, `End(page)`). `resolve` turns them into half-open carets `[lo, hi)`. Inclusive semantics are kept: the anchor char and the head char are both selected. | A drag can start before the page's chars arrive (they are requested on press). The range then resolves when `Msg::Text` arrives (`ViewerState::text_loaded`). Carets also cover gap edges and pages with no chars without special cases. Selection survives zoom (page points and indices). |
| R8 | Nearest char "capped at ~12 pt else None" | Cap only for the anchor (`ANCHOR_CAP = 12`). The head takes the nearest char on its page without a cap. | With a capped head, dragging into the margin past a line end would drop the selection. |
| R9 | (not specified) | Overlapping loose boxes (consecutive lines) tie-break on the nearest box centre | Without it, a point in the overlap always picked the upper line. Test: `nearest_prefers_closer_centre_in_overlapping_lines`. |
| R10 | Merge runs of chars adjacent in index with > 50% vertical overlap | Zero-size boxes are skipped when painting and do not split a run. They are kept for copy and picking. | PDFium generates spaces and line breaks with a zero-size box at the baseline (measured: 64 of 406 chars on page 1 of `CA_Kenya_Engagement_Model.pdf`). Without the skip, the overlap test divides by a zero height and every generated space splits the line. Test: `runs_merge_per_line`. |
| R11 | "Chars without a unicode char are skipped" | Also skips U+0000 | `unicode_char()` is `char::from_u32(FPDFText_GetUnicode)`, and PDFium returns 0 when there is no mapping. NUL must not reach the clipboard. |
| R12 | A click without movement clears the selection | A drag that starts off a page (or with Ctrl) also clears it | One rule: every new gesture on the canvas replaces the selection. |
| R13 | (not specified) | Starting a selection calls `Memory::stop_text_input` | egui's default `SurrenderFocusOn::Clicks` keeps a focused text field focused through a drag. Ctrl+C would then go to the search field, and "free" (no focus) would stay false, so the selection could never be copied. |
| R14 | Ctrl+C | Read as `Event::Copy`, only when no text field has focus | egui-winit turns Ctrl+C into `Event::Copy` and sends no `Key::C` event (`egui-winit-0.37.0/src/lib.rs:1133-1135`). |
| R15 | (widget not specified) | `ui.interact(clip_rect, …, Sense::CLICK \| Sense::DRAG)` inside the `ScrollArea` | `Sense::click_and_drag()` includes `FOCUSABLE`. A focused canvas would make "free" false and disable the navigation keys. Scroll bars are added after the content, so they stay on top and keep their drags. |
| R16 | Text requested on first press and for every page of a multi-page selection | `ViewerOutput.want_text: Range<usize>` (the pressed page on the press frame, the selection's pages while dragging). `App.text_pending: HashSet<usize>` stops repeat requests. | Keeps the worker channel the only path, and keeps the per-frame work to an empty range when idle. |
| R17 | Worker replies `Msg::Text` | A failed extraction replies with empty chars and logs `warn`. `compact` keeps every `Cmd::Text` queued after the last `Open`. `page_chars` logs inside, so the `run` loop stays within clippy's 100-line limit. | Without a reply the page would stay pending until the next open. Test: `compact_keeps_text_after_last_open`; `worker_end_to_end` now also checks `Cmd::Text` (chars "Hello", flipped top in 40..72). |
| R18 | Text cursor when hovering a char | Only over pages whose chars are loaded, that is, after the first press on that page | Follows the brief's press-only loading. Loading on hover would extract the text of every page the pointer passes over while scrolling, with no eviction. README limitation. |
| R19 | Selection painted with the page | Separate `paint_selection` pass after `paint` (over the search hits) | Adding two parameters to `paint` exceeds clippy's argument limit. |
| R20 | `ui.id()` | `ui.scope_id()` | `Ui::id` is deprecated in egui 0.37 (fails `-D warnings`). |

Not deviations, but worth knowing:
- Hit tests (cursor, drag head) run when the pointer's doc-space position changes. That is when the pointer moves, or when content scrolls under a still pointer. They also run once after a page's chars arrive. Each one is a linear scan of one page's chars. There is no per-frame allocation in the new code. Highlight runs come from an iterator.
- Cancel paths: any `MouseWheel` event (in `raw_input_hook`), `scroll_by`, `go_to_page` (arrows, Page keys, Home/End, page field), `reveal` (search), `zoom_by`, `fit_width` and `reset`. A fling also stops below 10 pt/s or at the top or bottom clamp. It calls `request_repaint` only while it continues.
- A touchscreen drag on the canvas now selects text. Our widget is on top of the `ScrollArea`'s drag-to-scroll area, which egui enables only after it detects a touch device. Touchpad and mouse wheel scrolling are not affected. The brief says nothing about touchscreens. This was not tested (no touchscreen here).

### Cargo.toml / Cargo.lock

| Change | Reason |
|---|---|
| `egui-winit = { version = "0.37", default-features = false, features = ["smithay-clipboard"] }` | R1. It unifies with eframe's egui-winit 0.37.0. Cargo.lock gains 6 packages: smithay-clipboard 0.7.3, smithay-client-toolkit 0.20.0 (winit keeps 0.19.2), calloop 0.14.5, calloop-wayland-source 0.4.1, wayland-protocols-experimental, wayland-protocols-misc. wayland-client and wayland-backend stay single copies, so `pinch_wayland` is unaffected. arboard is not compiled (`cargo tree -i arboard`: nothing). |

### Verification (round 2)

- `cargo test` with `PAPERLIGHT_PDFIUM` set: 84 pass (62 before + 7 fling + 13 selection + 1 compact + 1 viewer). `worker_end_to_end` runs; it does not skip.
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` and `cargo build --release` are clean.
- Smoke run (release, Wayland, `PAPERLIGHT_LOG=debug`, `CA_Kenya_Engagement_Model.pdf`, 6 s):
  - The document opens. Pages 0–2 render once at `scale_px=1.25` (5–27 ms), and page 3 follows after the settle.
  - No WARN or ERROR lines.
  - Threads: `pdf-worker`, `pinch`, `smithay-clipboa`.
  - 0 CPU ticks between 2 s and 5 s (idle, no repaint loop).
  - An earlier run showed renders of pages 4–10 at 4–6 s. A `WAYLAND_DEBUG=1` rerun showed only user pointer motion and no axis events, with 1 tick while idle. The renders came from real input on the window during that run.
- One-off probe (removed afterwards) of `Cmd::Text` on the real PDF: pages 1, 2 and 6 gave 406, 1075 and 2091 chars and 7, 31 and 59 line runs. The text read correctly, with PDFium's `\r\n`.

Not verified (needs a person at the touchpad):
- How the fling feels against Evince. The velocity weight, the 100 ms rest rule and the 80 pt/s threshold are first guesses.
- Drag selection, the highlight look, the text cursor and Ctrl+C into another app through smithay-clipboard on GNOME.
- Esc clearing both the search and the selection.
- Selection across pages while scrolling with the touchpad mid-drag.

## Round 3: touchpad scroll distance

User feedback after round 2: "The scroll speed still feels really slow."

Round 2 matched touchpad deltas 1:1 with a GTK 4 window and added coasting. The user compares
with Evince, which is GTK 3 (`ldd /usr/bin/evince` lists `libgtk-3.so.0`; evince 48.1 on Fedora 44).
GTK 3 scrolls touchpads very differently from GTK 4 (sources `gtk-3-24`):

- `gdk/wayland/gdkdevice-wayland.c`: `pointer_frame->delta_y = wl_fixed_to_double (value) / 10.0`.
  Ten Wayland surface pixels become one wheel unit.
- `gtk/gtkscrolledwindow.c`: smooth deltas are applied as `delta_y * get_scroll_unit()`, with
  `scroll_unit = MIN (pow (page_size, 2.0 / 3.0), page_size / 2.0)`. The kinetic velocity in
  `scroll_history_finish` is scaled by the same unit, then decelerated with `DECELERATION_FRICTION 4`.

So in Evince one touchpad pixel scrolls `page^(2/3) / 10` points: ×9.7 for a 950 pt view. GTK 4
(`main`) only applies the unit to `GDK_SCROLL_UNIT_WHEEL` deltas and scrolls surface deltas 1:1,
which is what Paperlight did and why it felt slow.

Change: `layout::TOUCHPAD_SCROLL_MULTIPLIER` (1.0) is replaced by `touchpad_points_per_pixel(view_h)
= wheel_notch_points(view_h) / 10`, applied to Ctrl-free `Point` deltas in `App::raw_input_hook`
before egui and the velocity tracker see them. The fling velocity therefore scales too, as in GTK 3.
`wheel_notch_points` gains GTK's `min(_, page / 2)` cap (only matters for viewports under 8 pt).
Tests: 85 pass. Not verified by hand (same reason as before).
