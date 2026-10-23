<p align="center">
  <img src="assets/icon-128.png" alt="Paperlight icon" width="96">
</p>

<h1 align="center">Paperlight</h1>

<p align="center">
  A lightweight, fast PDF viewer for Linux. Rust, egui and PDFium.<br>
  It does one job: show a PDF, quickly, with nothing in the way.
</p>

<p align="center">
  <img alt="Rust 1.95+" src="https://img.shields.io/badge/rust-1.95%2B-orange?logo=rust">
  <img alt="Edition 2024" src="https://img.shields.io/badge/edition-2024-blue">
  <img alt="License MIT" src="https://img.shields.io/badge/license-MIT-green">
  <img alt="Linux Wayland" src="https://img.shields.io/badge/platform-Linux%20%7C%20Wayland-lightgrey?logo=linux">
</p>

---

## Why

Most PDF viewers carry sidebars, thumbnails, annotation tools and printing dialogs
that you never open when you only want to read a file. Paperlight keeps search,
text selection, a page counter and zoom, and drops the rest. It opens instantly,
uses no CPU while idle, and scrolls the way a GTK app on a touchpad should.

## Features

- **Fast open.** The window appears at once and pages stream in from a
  background render thread.
- **Smooth zoom.** Pages render at a few fixed scales. In between, the GPU scales
  the nearest texture, so zooming never stalls.
- **Touchpad that feels right.** Scroll distance matches Evince (GTK 3 scrolls
  `page^(2/3)/10` points per finger pixel, about nine times GTK 4's 1:1) and
  keeps coasting after you lift your fingers, slowing down with GTK's friction.
- **Search** with a match counter and ▲ ▼ navigation, highlights on the page.
- **Text selection** by drag, across page boundaries, with Ctrl+C to copy.
- **Pinch to zoom** on Wayland touchpads, Ctrl+scroll everywhere.
- **Idle means idle.** No CPU use when nothing changes.
- **Opens as your default viewer.** One script installs the binary, icons and
  desktop entry and registers it for `application/pdf`.

Not included, on purpose: sidebar, thumbnails, annotations, forms editing,
printing.

## The window

```
┌──────────────────────────────────────────────────────────────────────────┐
│ report.pdf        [ search…        ] 3 / 12 ▲ ▼      [ 12 ] / 48   − 100% + │
├──────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│                         ┌──────────────────┐                             │
│                         │                  │                             │
│                         │      page 12     │                             │
│                         │                  │                             │
│                         └──────────────────┘                             │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
```

Open a file from the command line (`paperlight file.pdf`), drop one onto the
window, or double-click a PDF in your file manager once Paperlight is the
default handler.

## Keyboard and pointer

| Input | Action |
|---|---|
| `Ctrl+F` | Focus the search field |
| `Enter` / `Shift+Enter` (in search) | Next / previous match |
| Drag over text | Select text (may span pages); a click clears it |
| `Ctrl+C` | Copy the selected text |
| `Esc` | Clear the search and the selection |
| `Ctrl+=` or `Ctrl++` / `Ctrl+-` | Zoom in / out |
| `Ctrl+0`, or click the zoom % | Fit width (the default on open) |
| `Ctrl+scroll`, touchpad pinch | Zoom at the pointer |
| `PageDown` / `PageUp` | Scroll one screen |
| `↓` / `↑` | Scroll a little |
| `→` / `←` | Next / previous page |
| `Home` / `End` | First / last page |
| Page field + `Enter` | Go to that page |

## Install

### Requirements

- Rust 1.95 or newer (edition 2024)
- `curl`, `tar`, `sha256sum`, `xdg-utils`, `pkg-config`
- Wayland headers: `sudo dnf install wayland-devel` (Fedora) or
  `sudo apt install libwayland-dev` (Debian/Ubuntu)

### One command

```bash
git clone https://github.com/QoSz/paperlight.git
cd paperlight
./install.sh
```

The script builds the release binary and installs, for the current user only:

| Path | What |
|---|---|
| `~/.local/bin/paperlight` | The binary |
| `~/.local/lib/paperlight/libpdfium.so` | Pinned [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries) release `chromium/8111`, SHA-256 verified before install |
| `~/.local/share/icons/hicolor/…` | Icons, 16 px to 512 px plus SVG |
| `~/.local/share/applications/paperlight.desktop` | Desktop entry |

It then registers Paperlight as the default handler for `application/pdf`
with `xdg-mime`. Nothing is written outside your home directory and no
`sudo` is needed.

### Uninstall

```bash
./install.sh --uninstall
```

Removes every installed file and, if Paperlight is still the default PDF
viewer, restores `org.gnome.Evince.desktop`.

## Configuration

Paperlight has no config file. Two environment variables cover what you might
want to change:

| Variable | Meaning |
|---|---|
| `PAPERLIGHT_PDFIUM` | Path to a `libpdfium.so` to use instead of the installed one. The default search order is the executable's directory, then `~/.local/lib/paperlight/`, then the system library path. |
| `PAPERLIGHT_LOG` | `tracing` filter, for example `debug` or `paperlight=debug` (default `warn`). At `debug`, every page render is logged with its latency. |

## How it works

```
 main thread (egui)                     render worker thread
 ───────────────────                    ────────────────────
 input, layout, painting   ── jobs ──▶  PDFium: open, render page @ scale,
 texture cache per page    ◀─ RGBA ──   extract text, search
 fling + pinch handling
```

- **One PDFium thread.** `pdfium-render` handles are not `Send`, so the
  document lives on a single worker. Only owned buffers cross the channel.
- **Scale ladder.** Pages render at fixed scales. The viewer paints the nearest
  ready texture, GPU-scaled, and requests the exact scale about 100 ms after
  zooming stops.
- **Wayland pinch.** winit 0.30 does not expose touchpad gestures on Wayland,
  so Paperlight binds `zwp_pointer_gestures_v1` on winit's `wl_display`
  itself (`src/input/pinch_wayland.rs`).
- **Kinetic scroll.** Finger lift arrives as `TouchPhase::Ended`; velocity is
  tracked with an EMA and decayed with GTK's friction (`src/fling.rs`).

Design notes, the API facts verified against crate sources, and every
deviation from the original plan are in [`docs/`](docs/).

## Development

```bash
cargo test                                   # the PDFium end-to-end test skips without the library
PAPERLIGHT_PDFIUM=/path/to/libpdfium.so cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Lints are strict: `unwrap_used` and `panic` are denied, pedantic is on.

Source layout:

```
src/
├── main.rs          entry, logging, PDFium lookup
├── app.rs           eframe App: state, events, frame loop
├── layout.rs        page geometry, scroll math, touchpad scaling
├── fling.rs         kinetic scrolling
├── search.rs        search state and match navigation
├── selection.rs     text selection and clipboard
├── input/           Wayland pinch gesture binding
├── render/          worker thread, texture cache, PDFium loader
└── ui/              toolbar, viewer, theme
```

## Known limitations

- Password-protected PDFs are not supported (an error is shown).
- Search highlights and text selection are misplaced on pages with `/Rotate`
  other than 0 or a MediaBox that does not start at the origin.
- A page's text is read the first time you press on it, so the text cursor
  appears over a page only after the first click or drag there. No double-click
  word or triple-click line selection.
- Between render scales, text is downsampled by at most 1.5× (bilinear), so it
  can look slightly soft until the exact scale arrives.
- A page that fails to render stays white.
- The app always uses its dark palette: winit 0.30 does not report the system
  light/dark setting on Linux. The window title bar does follow the GNOME
  setting, read when the window opens.
- Touchpad pinch on Wayland uses `zwp_pointer_gestures_v1` (GNOME, KDE and most
  compositors have it). On X11, use Ctrl+scroll.
- Copy uses the Wayland clipboard. In an X11 session, copied text stays inside
  Paperlight. Kinetic scrolling also needs Wayland: X11 reports no finger lift.

## License

MIT. See [LICENSE](LICENSE).
