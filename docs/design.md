# Paperlight design spec

Direction: **lamplight on paper**. The chrome is warm graphite (dark) or warm
stone (light). One accent, ochre/amber, is used only where attention belongs:
focus, the text cursor, selected text and the current search hit. Everything
else stays quiet, so the white pages are the brightest thing on screen. The
layout is Adwaita-adjacent: flat header-bar buttons, filled entries and
hairline borders, with no heavy shadows on the chrome.

Code: `src/ui/theme.rs`. `theme::apply(ctx)` styles both egui themes once at
startup. `theme::palette(ui.visuals().dark_mode)` gives the colours for custom
painting (canvas, pages, highlights).

## Scale

| Token (Tailwind-ish) | px | Use |
|---|---|---|
| `space-0.5` | 2 | gap inside the zoom cluster `− 100% +` |
| `space-1` | 4 | icon to text, count to ▲▼ |
| `space-2` | 8 | `item_spacing.x`, toolbar vertical padding |
| `space-3` | 12 | toolbar horizontal padding |
| `space-4` | 16 | gap between toolbar groups |
| `space-6` | 24 | `PAGE_GAP` between pages |
| `space-8` | 32 | `PAGE_MARGIN` around the page column |

- egui spacing: `item_spacing` 8×6, `button_padding` 10×6, `interact_size.y` 28.
- Toolbar: `TOOLBAR_HEIGHT` 44. Controls are 28 high, with 8 above and 8 below.

**Radii:** `RADIUS_SM` 4 is for search-hit rects and the ▲▼ buttons inside the
search group. `RADIUS_MD` 6 is for all widgets (buttons, fields) and tooltips.
Pages have square corners (paper is not rounded).

**Type:** egui default fonts. Proportional is Ubuntu-Light; monospace is Hack.

| Style | Size | Use |
|---|---|---|
| Body / Button | 14 | file name, field text, page number, zoom % |
| Small | 12 | match count `n / m`, `/ 48` total, tooltips |

Glyph coverage with `default_fonts` only: Ubuntu-Light has `−` (U+2212) and
`…`, but not `▲▼`. Render ▲▼ with `RichText::monospace()`, because Hack has
them, or paint them as triangles. Paint the empty-state icon with the painter.

## Palettes

Hex values are sRGB. rgba alpha is straight (unpremultiplied) and goes into
`Color32::from_rgba_unmultiplied_const`.

| Role (`Palette` field) | Light | Dark | Notes |
|---|---|---|---|
| Canvas bg `canvas_bg` | `#E8E4DC` | `#1A1917` | also `panel_fill` |
| Toolbar bg `toolbar_bg` | `#F7F5F0` | `#262422` | also `window_fill` (tooltips) |
| Toolbar border `toolbar_border` | `#DCD6CC` | `#38352F` | 1 px under the toolbar; panel separator |
| Text primary `text_primary` | `#24211D` | `#EDE8DF` | labels, button text, icons |
| Text secondary `text_secondary` | `#6A645A` | `#A39D93` | `weak_text_color`: hints, count, empty state |
| Accent `accent` | `#8A570C` deep ochre | `#EBB04F` amber (accent on dark) | focus ring, cursor, links, selected text |
| Field bg `field_bg` | `#ECE8E1` | `#33302C` | filled entry, no resting border |
| Field border (focus) | = `accent`, 2 px | = `accent`, 2 px | via `selection.stroke` |
| Button hover `hover_bg` | `#ECE8E1` | `#33302C` | flat buttons gain a fill on hover |
| Button active `active_bg` | `#E1DCD3` | `#3E3A35` | pressed / keyboard focus, plus 1 px accent border |
| Selection bg `selection_bg` | `#F5E3BC` | `#55401A` | selected text is drawn in `accent` on this |
| Highlight hit `hit` | `rgba(255,196,46,0.38)` | `rgba(255,196,46,0.42)` | over white page: `#FFE9B0` / `#FFE6A7` |
| Highlight current `hit_current` | `rgba(240,134,18,0.58)` | `rgba(240,134,18,0.62)` | over white page: `#F6B976` / `#F6B46C` |
| Page shadow `page_shadow` | `rgba(48,36,20,0.16)` | `rgba(0,0,0,0.50)` | `Shadow { offset: [0, 2], blur: 12, spread: 0 }` |
| Page border `page_border` | `#CDC6BA` | `#0D0C0B` | 1 px hairline around each page |
| Error `error` | `#A8402F` | `#EC8C7D` | muted brick / coral; also `error_fg_color` |

Dark highlights are slightly stronger: next to a dark canvas, the white page
looks brighter and pale overlays wash out.

### Contrast (WCAG 2.x, checked with a script)

| Pair | Light | Dark | Need |
|---|---|---|---|
| text primary on toolbar / canvas / field | 14.7 / 12.6 / 13.1 | 12.7 / 14.4 / 10.8 | 4.5 |
| text primary on active bg | 11.7 | 9.2 | 4.5 |
| text secondary on toolbar / canvas / field | 5.4 / 4.6 / 4.8 | 5.8 / 6.5 / 4.9 | 4.5 |
| accent on toolbar | 5.6 | 8.0 | 4.5 |
| focus ring (accent) on field bg | 5.0 | 6.8 | 3.0 (UI) |
| selected text: accent on selection bg | 4.8 | 5.1 | 4.5 |
| error on toolbar / canvas | 5.6 / 4.8 | 6.3 / 7.2 | 4.5 |
| black PDF text under hit / current hit | 17.5 / 12.1 | 17.1 / 11.7 | 4.5 |

Hover fills (1.12 to 1.18 against the toolbar) are cues, as in Adwaita. They
are not the only state signal: the pointer also changes, and focus always adds
the 2 px accent ring.

## Widget states (`apply()` mapping)

| egui state | fill (`weak_bg_fill` for buttons) | border | text |
|---|---|---|---|
| noninteractive | `toolbar_bg` | 1 px `toolbar_border` (separators) | `text_primary` |
| inactive | transparent, so buttons are flat | none | `text_primary` |
| hovered | `hover_bg` | none | `text_primary` |
| active (pressed / focused) | `active_bg` | 1 px `accent` | `text_primary` |
| TextEdit at rest | `field_bg` (`extreme_bg_color`) | none | `text_primary` |
| TextEdit focused | `field_bg` | **2 px `accent` focus ring** | `text_primary`, cursor 2 px `accent` |

Window and popup shadows are `Shadow::NONE`. Tooltips use `toolbar_bg` with a
1 px `toolbar_border`.

## Toolbar (44 px, `toolbar_bg`, bottom hairline)

Inner margin is 12 × 8. Left to right:

1. **File name**: Body 14 `text_primary`, vertically centred. It takes the
   leftover width and truncates with `…` via `Label::truncate()`; the full name
   shows in the tooltip. With no file it shows "No document" in
   `text_secondary`.
2. **Flexible gap.**
3. **Search**: field 220 wide, hint "Search" (`text_secondary`). Ctrl+F
   focuses it. Enter goes to the next hit, Shift+Enter to the previous one,
   and Esc clears the query and leaves the field. With a query, the field is
   followed by the count `n / m` (Small 12 `text_secondary`; `0 / 0` in
   `error`) and the ▲ ▼ buttons (24 × 24, `RADIUS_SM`, gap 4).
4. 16 px gap. **Page entry**: `[ 12 ]` is a field 44 wide with the number
   centred. It is followed by ` / 48` in Small 12 `text_secondary`. Enter
   commits; invalid input reverts.
5. 16 px gap. **Zoom**: `−` · `100%` · `+`, with a 2 px gap. The `−` and `+`
   buttons are 28 × 28. `100%` is a flat button at least 56 wide; clicking it
   fits the width (tooltip "Fit width").

## Canvas

- Fill: `canvas_bg`. Pages are stacked vertically and centred horizontally,
  with `PAGE_GAP` 24 between them and `PAGE_MARGIN` 32 around the column.
- Each page: shadow (`page_shadow`, offset `[0, 2]`, blur 12), then the page
  image, then a 1 px `page_border` drawn inside the edge
  (`StrokeKind::Inside`). Square corners.
- Search hits: `rect_filled(hit_rect.expand(1.0), RADIUS_SM, hit)`. The
  current hit uses `hit_current` instead of `hit`; it does not stack on top of
  `hit`.
- **Empty state**: centred in the canvas. A painted page glyph (28 × 36,
  folded top-right corner, 1.5 px stroke in `text_secondary`) sits 8 px above
  "Drop a PDF here" (Body 14, `text_secondary`).
- **Error state**: the same centred slot as the empty state. It shows inline
  text in `error` (Body 14), wrapped at 420 px max width, with no icon and no
  box.
