# Paperlight design spec

Direction: **macOS Preview**. One unified title bar holds the window buttons,
the document title and every control, so there is a single bar above the
pages. Neutral greys from Apple's palette, one accent (system blue) used only
for focus, the text cursor and selected text. Type is Inter, the closest open
match to SF Pro. Icons are thin vector lines in the spirit of SF Symbols. Pages
float on a quiet canvas with a soft shadow, and the white pages stay the
brightest thing on screen.

Code: `src/ui/theme.rs` (tokens, fonts, egui style), `src/ui/controls.rs`
(icons, icon buttons, fields), `src/ui/chrome.rs` (window chrome),
`src/ui/toolbar.rs` (title bar layout). `theme::apply(ctx)` installs the fonts
and styles both egui themes once at startup.
`theme::palette(ui.visuals().dark_mode)` gives the colours for custom painting.

## Scale

| Token | pt | Use |
|---|---|---|
| zoom cluster gap | 2 | between `−`, `100%` and `+` |
| page cluster gap | 4 | between `‹`, the page field, `/ 48` and `›`; count to ⌃⌄ |
| `item_spacing.x` | 8 | default gap |
| `GROUP_GAP` | 12 | between control groups |
| bar padding | 16 × 12 | title bar inner margin |
| `LIGHTS_GAP` | 16 | rotate button to window buttons |
| `PAGE_GAP` | 24 | between pages |
| `PAGE_MARGIN` | 32 | around the page column |

- egui spacing: `item_spacing` 8×6, `button_padding` 8×5, `interact_size.y` 28.
- Title bar: `TOOLBAR_HEIGHT` 52. Controls are 28 high, with 12 above and below.
- Scroll areas paint no edge fade (`scroll.fade.strength = 0`), and scroll bars
  sit `RESIZE_EDGE` (5) in from the window edge (`bar_outer_margin`), clear of
  the resize bands.
- Minimum window size 640 × 320: the narrowest width at which the title bar's
  controls leave room for the title.

**Radii:** `RADIUS_SM` 4 for search-hit rects. `RADIUS_MD` 7 for buttons,
fields, tooltips and menus. Pages have square corners (paper is not rounded).

**Type:** Inter 4.1 Regular and SemiBold, bundled (`assets/fonts/`, SIL OFL
1.1). egui's default fonts stay behind Inter as fallbacks for other scripts,
symbols and emoji. Labels are not selectable, so dragging over the title moves
the window.

| Style | Font | Size | Use |
|---|---|---|---|
| Heading | Inter SemiBold | 14 | document title |
| Body / Button | Inter | 14 | field text, page number, zoom %, status text |
| Small | Inter | 12 | subtitle "Page n of m", `/ 48`, match count |

**Icons** (`controls::Icon`): painted with 1.5 pt lines in a 10 pt box, in the
button's text colour: magnifier, chevrons ‹ › ⌃ ⌄, minus, plus, and a
clockwise arrow for rotate. No icon font is needed.

## Palettes

Hex values are sRGB. rgba alpha is straight (unpremultiplied).

| Role (`Palette` field) | Light | Dark | Notes |
|---|---|---|---|
| Canvas `canvas_bg` | `#E5E5EA` | `#1C1C1E` | also `panel_fill` |
| Title bar `toolbar_bg` | `#F5F5F7` | `#2C2C2E` | also `window_fill` (tooltips) |
| Hairline `toolbar_border` | `#D2D2D7` | `#3A3A3C` | under the bar, around popups, window outline |
| Text primary `text_primary` | `#1D1D1F` | `#F5F5F7` | title, labels, icons, selected field text |
| Text secondary `text_secondary` | `#636366` | `#AEAEB2` | subtitle, hints, count, empty state |
| Accent `accent` | `#007AFF` | `#0A84FF` | system blue: focus ring, text cursor, links |
| Field `field_bg` | `#E8E8ED` | `#363638` | filled fields, no resting border |
| Hover `hover_bg` | `#E8E8ED` | `#3A3A3C` | flat buttons gain a fill on hover |
| Pressed `active_bg` | `#D9D9DE` | `#48484A` | pressed / keyboard focus (+ 1 pt accent border) |
| Selection `selection_bg` | `#007AFF` | `#0A84FF` | at `SELECTION_ALPHA` 30 %: `#B2D7FF` over a white page (macOS's `#B3D7FF`) |
| Search hit `hit` | `rgba(255,214,10,0.40)` | `rgba(255,214,10,0.44)` | system yellow; over white: `#FFEF9D` |
| Current hit `hit_current` | `rgba(255,159,10,0.58)` | `rgba(255,159,10,0.62)` | system orange; over white: `#FFC771` |
| Shadow `page_shadow` | `rgba(0,0,0,0.15)` | `rgba(0,0,0,0.59)` | pages: offset `[0, 4]`, blur 18; popups: `[0, 2]`, blur 8 |
| Page edge `page_border` | `rgba(0,0,0,0.06)` | `rgba(0,0,0,0.25)` | 1 pt, inside the page edge |
| Error `error` | `#C4001A` | `#FF6961` | also `error_fg_color` |

Window buttons follow libadwaita: their fill is `text_primary` at 10 % (rest),
15 % (hovered) and 30 % (pressed); their icons are `text_primary`, or
`text_secondary` while the window is unfocused.

### Contrast (WCAG 2.x)

`theme::tests::text_meets_wcag_aa_on_every_surface` enforces the 4.5 and 3.0
floors below; the figures are from the same formula.

| Pair | Light | Dark | Need |
|---|---|---|---|
| text primary on bar / canvas / field / hover | 15.5 / 13.4 / 13.8 / 13.8 | 12.8 / 15.6 / 11.1 / 10.4 | 4.5 |
| text primary on pressed | 12.0 | 8.4 | 4.5 |
| text secondary on bar / canvas / field / hover | 5.5 / 4.8 / 4.9 / 4.9 | 6.3 / 7.7 / 5.5 / 5.1 | 4.5 |
| error on bar / canvas | 5.7 / 5.0 | 4.9 / 6.0 | 4.5 |
| text cursor (accent) on field | 3.3 | 3.3 | 3.0 (UI) |
| black PDF text under selection / hit / current hit | 14.1 / 18.1 / 13.7 | 14.5 / 17.8 / 13.3 | 4.5 |
| selected field text on selection | 9.6 | 8.0 | 4.5 |

The focus ring is a translucent halo, as on macOS (accent at 50 %, 3 pt,
outside the field). It is a cue, not the only one: the 2 pt accent text cursor
also marks the focused field.

## Widget states (`apply()` mapping)

| egui state | fill (`weak_bg_fill` for buttons) | border | text |
|---|---|---|---|
| noninteractive | `toolbar_bg` | 1 pt `toolbar_border` (separators) | `text_primary` |
| inactive | transparent, so buttons are flat | none | `text_primary` |
| hovered | `hover_bg` | none | `text_primary` |
| active (pressed / focused) | `active_bg` | 1 pt `accent` | `text_primary` |

Fields (`controls::field`) bypass the theme frame: `field_bg` fill,
`RADIUS_MD`, padding 8 × 5 (26 on the left when a leading icon is drawn), no
border at rest, and the accent halo while focused. Selected field text keeps
`text_primary` (`selection.stroke`).

## Title bar (52 pt, `toolbar_bg`, bottom hairline)

Inner margin is 16 × 12. Empty parts of the bar move the window when dragged
and maximize or restore it on double-click (`chrome::title_bar`, added before
the controls so they keep their clicks). The bar, the window buttons and the resize
bands are not focusable, so Tab never lands on them. Left to right:

1. **Title**: the file name in Heading over "Page n of m" in Small
   `text_secondary`, the pair centred vertically. It takes the leftover width
   and truncates with `…`; the full name shows on hover. With no file it shows
   "Paperlight" and no subtitle.
2. **Flexible gap.**
3. **Search**: field 180 wide with a magnifier, hint "Search". It shrinks to
   no less than 96 so the controls leave 96 pt of title free. Ctrl+F focuses
   it. Enter goes to the next hit, Shift+Enter to the previous one, and Esc
   clears the query and leaves the field. With a query it is followed by the
   count `n / m` (Small `text_secondary`; `0 / 0` in `error`) and ⌃ ⌄ buttons
   (24 × 24).
4. 12 pt gap. **Page**: `‹` · field, number centred · `/ 48` in Small
   `text_secondary` · `›`. The field is 18 + 9 per digit of the page count
   (at least two digits) wide. Enter in the field commits; invalid input
   reverts. `‹ ›` step from the scroll position like `←` / `→`.
5. 12 pt gap. **Zoom**: `−` · `100%` · `+`, 2 pt apart. `−` and `+` are
   28 × 28; `100%` is a flat button at least 56 wide that fits the width.
6. 12 pt gap. **Rotate**: a 28 × 28 clockwise-arrow button.
7. 16 pt gap. **Window buttons**, GNOME style: three 24 pt round buttons, 6 pt
   apart, in GNOME's default order (`button-layout`
   `appmenu:minimize,maximize,close`): minimize, maximize/restore, and close at
   the far right. Symbolic icons in an 8 pt box, 1.2 pt lines, always shown: a
   low bar, a square (two stacked squares while maximized), a cross. No
   colour-coding. They are laid out first, so they keep the right end however
   narrow the window is.

Page, zoom, search and rotate are disabled while no document is open.

## Window

The window is undecorated (`with_decorations(false)`); `chrome::window_edges`
runs after the panels and, unless the window is maximized or fullscreen:

- strokes a 1 pt `toolbar_border` outline inside the window edge;
- adds 5 pt resize bands on the four edges and 12 pt squares at the corners
  (corners win where they overlap an edge). A press starts the compositor
  resize at once (`Sense::drag` only, so there is no drag threshold), and the
  pointer shows the matching resize cursor. Only the primary button resizes,
  and only while the window is focused (as egui-winit guards `StartDrag`).
- The compositor keeps the button release of a move or resize, so
  `chrome::release_after_grab` (called from `raw_input_hook`) feeds egui the
  missing release at the start of the next input.

## Canvas

- Fill: `canvas_bg`. Pages are stacked vertically and centred horizontally,
  with `PAGE_GAP` between them and `PAGE_MARGIN` around the column.
- Each page: shadow (`page_shadow`, offset `[0, 4]`, blur 18), the page image,
  then a 1 pt `page_border` inside the edge. Square corners.
- Search hits: `rect_filled(hit_rect.expand(1.0), RADIUS_SM, hit)`. The
  current hit uses `hit_current` instead; it does not stack on `hit`.
- **Empty state**: centred in the canvas. A painted page glyph (28 × 36,
  folded top-right corner, 1.5 pt stroke in `text_secondary`) sits 8 pt above
  "Drop a PDF here" (Body, `text_secondary`).
- **Password state**: "name is password-protected" (Body), then a 240 wide
  field (focused), then "Wrong password" in `error` after a rejected try.
- **Error state**: the same centred slot. Inline text in `error` (Body),
  wrapped at 420 pt, with no icon and no box.
