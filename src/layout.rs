//! Pure page geometry: stacking, visibility, zoom math and page-number parsing.

use std::ops::Range;

use egui::{Rangef, Rect, Vec2, pos2, vec2};

/// Smallest and largest zoom (egui points per PDF point).
pub const MIN_ZOOM: f32 = 0.25;
pub const MAX_ZOOM: f32 = 6.0;
/// Multiplicative step for Ctrl+= / Ctrl+- and the toolbar buttons.
pub const ZOOM_STEP: f32 = 1.25;
/// Render scales, as multiples of pixels-per-point.
const BUCKETS: [f32; 8] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0];

/// Page size in PDF points, /Rotate already applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSize {
    pub w: f32,
    pub h: f32,
}

/// Page rectangles in doc space for one zoom and viewport width.
#[derive(Debug, Default)]
pub struct PageLayout {
    rects: Vec<Rect>,
    content: Vec2,
    scale: f32,
    gap: f32,
}

impl PageLayout {
    /// Stacks pages top to bottom (`margin` above the first and below the last, `gap` between),
    /// each centred in `content.x = max(viewport_w, widest·scale + 2·margin)`.
    pub fn new(sizes: &[PageSize], scale: f32, viewport_w: f32, gap: f32, margin: f32) -> Self {
        if sizes.is_empty() {
            return Self {
                rects: Vec::new(),
                content: vec2(viewport_w, 0.0),
                scale,
                gap,
            };
        }
        let widest = sizes.iter().map(|s| s.w).fold(0.0, f32::max);
        let content_w = viewport_w.max(widest * scale + 2.0 * margin);
        let mut y = margin;
        let rects = sizes
            .iter()
            .map(|s| {
                let size = vec2(s.w, s.h) * scale;
                let r = Rect::from_min_size(pos2((content_w - size.x) / 2.0, y), size);
                y = r.max.y + gap;
                r
            })
            .collect();
        Self {
            rects,
            content: vec2(content_w, y - gap + margin),
            scale,
            gap,
        }
    }

    /// Zoom this layout was built for.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Total scrollable size; `(viewport_w, 0)` when there are no pages.
    pub fn content_size(&self) -> Vec2 {
        self.content
    }

    /// All page rects in doc space, indexed by page.
    pub fn rects(&self) -> &[Rect] {
        &self.rects
    }

    /// Pages whose rect intersects `view` vertically; empty range in gaps.
    pub fn visible_range(&self, view: Rect) -> Range<usize> {
        let start = self.rects.partition_point(|r| r.max.y <= view.min.y);
        let end = self.rects.partition_point(|r| r.min.y < view.max.y);
        start..end.max(start)
    }

    /// Page under the point one third down the viewport; the first page at the top, the last at
    /// the bottom. After scrolling to `page_scroll_y(n)` this is `n`, unless page `n` is shorter
    /// than about a third of the viewport (far zoomed out) or cannot reach the top (end of doc).
    pub fn current_page(&self, view: Rect) -> usize {
        if view.min.y <= 0.0 {
            0
        } else if view.max.y >= self.content.y - 0.5 {
            self.rects.len().saturating_sub(1)
        } else {
            self.page_at_y(view.min.y + view.height() / 3.0)
        }
    }

    /// Scroll y that shows `page` with `gap/2` above it; 0 for page 0.
    pub fn page_scroll_y(&self, page: usize) -> f32 {
        match self.rects.get(page) {
            Some(r) if page > 0 => r.min.y - self.gap / 2.0,
            _ => 0.0,
        }
    }

    /// Target of "next page" (`forward`) or "previous page" from scroll y `y`: the first page whose
    /// `page_scroll_y` is below `y` (else the last), or the last one above it (else the first).
    /// `current_page ± 1` skips or repeats pages when several pages fit in the viewport.
    pub fn step_page(&self, y: f32, forward: bool) -> usize {
        // Tolerates float drift between the offset we set and the one the ScrollArea reports.
        const SLACK: f32 = 1.0;
        let n = self.rects.len();
        if forward {
            (0..n)
                .find(|&p| self.page_scroll_y(p) > y + SLACK)
                .unwrap_or(n.saturating_sub(1))
        } else {
            (0..n)
                .rev()
                .find(|&p| self.page_scroll_y(p) < y - SLACK)
                .unwrap_or(0)
        }
    }

    /// Maps a rect in page points (top-left origin) of `page` to doc space.
    pub fn to_doc(&self, page: usize, r: Rect) -> Rect {
        let origin = self.rects[page].min;
        Rect::from_min_max(
            origin + r.min.to_vec2() * self.scale,
            origin + r.max.to_vec2() * self.scale,
        )
    }

    /// First page whose bottom is at or below `y` (the next page when `y` is in a gap), else the last.
    fn page_at_y(&self, y: f32) -> usize {
        self.rects
            .partition_point(|r| r.max.y < y)
            .min(self.rects.len().saturating_sub(1))
    }
}

/// Zoom that fits the widest page in `viewport_w - 2·margin`, clamped; 1.0 if `sizes` is empty.
pub fn fit_width_scale(sizes: &[PageSize], viewport_w: f32, margin: f32) -> f32 {
    let widest = sizes.iter().map(|s| s.w).fold(0.0, f32::max);
    if widest > 0.0 {
        clamp_zoom((viewport_w - 2.0 * margin) / widest)
    } else {
        1.0
    }
}

/// Clamps to `MIN_ZOOM..=MAX_ZOOM`.
pub fn clamp_zoom(zoom: f32) -> f32 {
    zoom.clamp(MIN_ZOOM, MAX_ZOOM)
}

/// Wayland surface pixels per mouse-wheel notch (the `wl_pointer.axis` convention GTK 3 uses to
/// turn touchpad pixel deltas into wheel units).
const SURFACE_PX_PER_NOTCH: f32 = 10.0;

/// Points scrolled per mouse-wheel notch: GTK's `min(page_size^(2/3), page_size / 2)`, so the
/// wheel feels like Evince and other GNOME apps.
pub fn wheel_notch_points(view_h: f32) -> f32 {
    let h = view_h.max(0.0);
    h.powf(2.0 / 3.0).min(h / 2.0)
}

/// Points scrolled per touchpad pixel. GTK 3 (Evince) treats [`SURFACE_PX_PER_NOTCH`] touchpad
/// pixels as one wheel notch, so a swipe moves `page_size^(2/3) / 10` points per pixel: about ×9
/// on a laptop screen. GTK 4 scrolls touchpads 1:1, which felt slow next to Evince.
pub fn touchpad_points_per_pixel(view_h: f32) -> f32 {
    wheel_notch_points(view_h) / SURFACE_PX_PER_NOTCH
}

/// egui `scroll_zoom_speed` for a wheel notch of `notch` points. egui zooms Ctrl+wheel by
/// `exp(speed · points)`, so with GTK-sized notches its default (1/200) zoomed ~1.7× per notch;
/// this makes one notch one [`ZOOM_STEP`].
pub fn wheel_zoom_speed(notch: f32) -> f32 {
    ZOOM_STEP.ln() / notch.max(1.0)
}

/// New scroll offset that keeps the doc point under `anchor_in_view` (relative to the viewport's
/// top-left) fixed across a relayout. It maps through page-local points of the page at that y
/// (gaps and margins extrapolate linearly). Not clamped. Returns `old_offset` if either layout is
/// empty or the page counts differ.
pub fn zoom_about(
    old: &PageLayout,
    new: &PageLayout,
    anchor_in_view: Vec2,
    old_offset: Vec2,
) -> Vec2 {
    if old.rects.is_empty() || old.rects.len() != new.rects.len() {
        return old_offset;
    }
    let anchor = (old_offset + anchor_in_view).to_pos2();
    let page = old.page_at_y(anchor.y);
    let local = (anchor - old.rects[page].min) / old.scale;
    let moved = new.rects[page].min + local * new.scale;
    moved.to_vec2() - anchor_in_view
}

/// Clamps a scroll offset to `0..=max(content - view, 0)` per axis.
pub fn clamp_offset(offset: Vec2, content: Vec2, view: Vec2) -> Vec2 {
    offset.clamp(Vec2::ZERO, (content - view).max(Vec2::ZERO))
}

/// Per axis: keep `view.min` if `target` fits inside `view` on that axis, else centre `target`.
/// Returns `None` when nothing changes. Both rects are in doc space. Not clamped.
pub fn reveal_offset(view: Rect, target: Rect) -> Option<Vec2> {
    let axis = |v: Rangef, t: Rangef| {
        if v.min <= t.min && t.max <= v.max {
            v.min
        } else {
            t.center() - v.span() / 2.0
        }
    };
    let offset = vec2(
        axis(view.x_range(), target.x_range()),
        axis(view.y_range(), target.y_range()),
    );
    (offset != view.min.to_vec2()).then_some(offset)
}

/// Smallest bucket `b >= zoom - 1e-3` (else the largest), times `ppp`. Never below the requested
/// size, so never blurry; capped at 4·ppp.
pub fn scale_bucket(zoom: f32, ppp: f32) -> f32 {
    let largest = BUCKETS[BUCKETS.len() - 1];
    BUCKETS
        .into_iter()
        .find(|&b| b >= zoom - 1e-3)
        .unwrap_or(largest)
        * ppp
}

/// Largest `scale_px` keeping every page's longer side under the GPU limit;
/// `f32::INFINITY` if `sizes` is empty. `egui_glow` asserts on oversize textures.
#[expect(
    clippy::cast_precision_loss,
    reason = "texture sides are far below 2^24"
)]
pub fn max_scale_px(sizes: &[PageSize], max_texture_side: usize) -> f32 {
    let longest = sizes.iter().map(|s| s.w.max(s.h)).fold(0.0, f32::max);
    if longest > 0.0 {
        max_texture_side.saturating_sub(1) as f32 / longest
    } else {
        f32::INFINITY
    }
}

/// 1-based user text to a 0-based page index; trims whitespace; `None` if not in `1..=total`.
pub fn parse_page_input(s: &str, total: usize) -> Option<usize> {
    s.trim()
        .parse()
        .ok()
        .filter(|n| (1..=total).contains(n))
        .map(|n| n - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A4ISH: PageSize = PageSize { w: 600.0, h: 800.0 };

    /// Fixture F: three 600×800 pages at zoom `scale`, viewport 800, gap 20, margin 30.
    fn f(scale: f32) -> PageLayout {
        PageLayout::new(&[A4ISH; 3], scale, 800.0, 20.0, 30.0)
    }

    /// Viewport rect `0..800` wide, `a..b` tall.
    fn view(a: f32, b: f32) -> Rect {
        Rect::from_min_max(pos2(0.0, a), pos2(800.0, b))
    }

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn new_centres_and_stacks() {
        let l = f(1.0);
        assert_eq!(
            l.rects(),
            [
                rect(100.0, 30.0, 700.0, 830.0),
                rect(100.0, 850.0, 700.0, 1650.0),
                rect(100.0, 1670.0, 700.0, 2470.0),
            ]
        );
        assert_eq!(l.content_size(), vec2(800.0, 2500.0));
    }

    #[test]
    fn new_widens_content_for_wide_page() {
        let l = PageLayout::new(&[A4ISH], 1.0, 500.0, 20.0, 30.0);
        assert!(close(l.content_size().x, 660.0));
        assert_eq!(l.rects()[0].x_range(), 30.0..=630.0);
    }

    #[test]
    fn new_scales_pages() {
        let l = f(2.0);
        assert_eq!(l.rects()[0], rect(30.0, 30.0, 1230.0, 1630.0));
        assert_eq!(l.content_size(), vec2(1260.0, 4900.0));
    }

    #[test]
    fn new_empty() {
        let l = PageLayout::new(&[], 1.0, 800.0, 20.0, 30.0);
        assert_eq!(l.rects(), []);
        assert_eq!(l.content_size(), vec2(800.0, 0.0));
    }

    #[test]
    fn visible_range_spans_gap() {
        assert_eq!(f(1.0).visible_range(view(800.0, 900.0)), 0..2);
    }

    #[test]
    fn visible_range_gap_only() {
        assert_eq!(f(1.0).visible_range(view(835.0, 845.0)), 1..1);
    }

    #[test]
    fn visible_range_past_end() {
        assert_eq!(f(1.0).visible_range(view(3000.0, 3500.0)), 3..3);
    }

    #[test]
    fn visible_range_empty_layout() {
        assert_eq!(PageLayout::default().visible_range(view(0.0, 600.0)), 0..0);
    }

    #[test]
    fn current_page_one_third_down() {
        // Probe at 300 + 1200/3 = 700 (page 0); the centre, 900, would be page 1.
        assert_eq!(f(1.0).current_page(view(300.0, 1500.0)), 0);
        assert_eq!(f(1.0).current_page(view(600.0, 1400.0)), 1);
    }

    #[test]
    fn current_page_top_is_first() {
        assert_eq!(f(1.0).current_page(view(0.0, 2000.0)), 0);
    }

    #[test]
    fn current_page_bottom_is_last() {
        assert_eq!(f(1.0).current_page(view(700.0, 2500.0)), 2);
    }

    #[test]
    fn current_page_gap_picks_next() {
        // Probe at 640 + 600/3 = 840, in the 830..850 gap.
        assert_eq!(f(1.0).current_page(view(640.0, 1240.0)), 1);
    }

    /// Regression: the counter followed the viewport centre, so after "go to page N" on short
    /// pages (landscape slides such as `CA_Kenya_Engagement_Model.pdf`) it showed N+1.
    #[test]
    fn current_page_after_go_to_short_pages() {
        let slide = PageSize { w: 720.0, h: 405.0 };
        // Default 900×1100 window minus the 44 pt toolbar, at fit width.
        let (vw, vh) = (900.0, 1056.0);
        let l = PageLayout::new(
            &[slide; 24],
            fit_width_scale(&[slide], vw, 32.0),
            vw,
            24.0,
            32.0,
        );
        let max_y = l.content_size().y - vh;
        for n in 0..24 {
            let y = l.page_scroll_y(n);
            // Pages that cannot reach the top (clamped at the end) show the last page.
            let want = if y <= max_y { n } else { 23 };
            let view = Rect::from_min_size(pos2(0.0, y.min(max_y)), vec2(vw, vh));
            assert_eq!(l.current_page(view), want, "go to page {n}");
        }
    }

    #[test]
    fn page_scroll_y_values() {
        let l = f(1.0);
        assert!(close(l.page_scroll_y(0), 0.0));
        assert!(close(l.page_scroll_y(1), 840.0));
        assert!(close(l.page_scroll_y(2), 1660.0));
    }

    #[test]
    fn step_page_values() {
        let l = f(1.0);
        assert_eq!(l.step_page(0.0, true), 1);
        assert_eq!(l.step_page(840.0, true), 2);
        assert_eq!(l.step_page(1660.0, true), 2);
        assert_eq!(l.step_page(0.0, false), 0);
        assert_eq!(l.step_page(840.0, false), 0);
        // Part-way into page 1: back goes to its start.
        assert_eq!(l.step_page(1000.0, false), 1);
        assert_eq!(PageLayout::default().step_page(0.0, true), 0);
    }

    /// Regression: with pages shorter than half the viewport, the (then centre-based) current page
    /// was one past the page `go_to_page` puts at the top, so `current_page ± 1` skipped forward
    /// and stuck backward.
    #[test]
    fn step_page_short_pages_never_skips() {
        let slide = PageSize { w: 720.0, h: 405.0 };
        let l = PageLayout::new(&[slide; 5], 1.0, 800.0, 24.0, 32.0);
        let y = l.page_scroll_y(1);
        assert_eq!(l.step_page(y, true), 2);
        assert_eq!(l.step_page(y, false), 0);
    }

    #[test]
    fn to_doc_maps_page_points() {
        let r = f(2.0).to_doc(0, rect(10.0, 20.0, 30.0, 40.0));
        assert_eq!(r, rect(50.0, 70.0, 90.0, 110.0));
    }

    #[test]
    fn fit_width_basic() {
        let letter = PageSize { w: 612.0, h: 792.0 };
        assert!(close(fit_width_scale(&[letter], 676.0, 32.0), 1.0));
    }

    #[test]
    fn fit_width_uses_widest() {
        let wide = PageSize {
            w: 1200.0,
            h: 800.0,
        };
        assert!(close(fit_width_scale(&[A4ISH, wide], 664.0, 32.0), 0.5));
    }

    #[test]
    fn fit_width_empty_is_one() {
        assert!(close(fit_width_scale(&[], 800.0, 32.0), 1.0));
    }

    #[test]
    fn fit_width_clamped() {
        let tiny = PageSize { w: 100.0, h: 100.0 };
        let letter = PageSize { w: 612.0, h: 792.0 };
        assert!(close(fit_width_scale(&[tiny], 10_000.0, 32.0), 6.0));
        assert!(close(fit_width_scale(&[letter], 100.0, 32.0), 0.25));
    }

    #[test]
    fn wheel_notch_follows_gtk_formula() {
        assert!(close(wheel_notch_points(1000.0), 100.0));
        assert!(close(wheel_notch_points(0.0), 0.0));
        assert!(close(wheel_notch_points(-5.0), 0.0));
        // Tiny viewports: GTK caps the notch at half a page.
        assert!(close(wheel_notch_points(4.0), 2.0));
    }

    #[test]
    fn touchpad_scrolls_a_tenth_of_a_notch_per_pixel() {
        assert!(close(touchpad_points_per_pixel(1000.0), 10.0));
        assert!(close(touchpad_points_per_pixel(0.0), 0.0));
    }

    #[test]
    fn wheel_zoom_notch_is_one_step() {
        for notch in [40.0, 100.0, 250.0] {
            assert!(close((wheel_zoom_speed(notch) * notch).exp(), ZOOM_STEP));
        }
        assert!(wheel_zoom_speed(0.0).is_finite());
    }

    #[test]
    fn clamp_zoom_bounds() {
        assert!(close(clamp_zoom(0.1), 0.25));
        assert!(close(clamp_zoom(10.0), 6.0));
        assert!(close(clamp_zoom(1.5), 1.5));
    }

    #[test]
    fn zoom_about_keeps_point_fixed() {
        let o = zoom_about(&f(1.0), &f(2.0), vec2(400.0, 300.0), vec2(0.0, 900.0));
        assert_eq!(o, vec2(230.0, 2050.0));
    }

    #[test]
    fn zoom_about_in_gap() {
        let o = zoom_about(&f(1.0), &f(2.0), vec2(400.0, 300.0), vec2(0.0, 540.0));
        assert_eq!(o, vec2(230.0, 1330.0));
    }

    #[test]
    fn zoom_about_empty_returns_old() {
        let (d, offset) = (PageLayout::default(), vec2(12.0, 34.0));
        assert_eq!(zoom_about(&d, &d, vec2(400.0, 300.0), offset), offset);
    }

    #[test]
    fn clamp_offset_bounds() {
        let view = vec2(800.0, 600.0);
        assert_eq!(
            clamp_offset(vec2(-5.0, 5000.0), vec2(800.0, 2500.0), view),
            vec2(0.0, 1900.0)
        );
        assert_eq!(
            clamp_offset(vec2(-5.0, 5000.0), vec2(500.0, 500.0), view),
            Vec2::ZERO
        );
    }

    #[test]
    fn reveal_offset_none_when_visible() {
        let target = rect(100.0, 100.0, 200.0, 120.0);
        assert_eq!(reveal_offset(view(0.0, 600.0), target), None);
    }

    #[test]
    fn reveal_offset_centres_hidden_target() {
        let target = rect(100.0, 1000.0, 200.0, 1020.0);
        assert_eq!(
            reveal_offset(view(0.0, 600.0), target),
            Some(vec2(0.0, 710.0))
        );
    }

    #[test]
    fn scale_bucket_values() {
        for (zoom, ppp, want) in [
            (1.0, 1.0, 1.0),
            (1.1, 1.0, 1.25),
            (0.3, 1.0, 0.5),
            (0.5, 1.0, 0.5),
            (2.5, 1.0, 3.0),
            (5.0, 1.0, 4.0),
            (1.0, 2.0, 2.0),
            (1.1, 1.5, 1.875),
            (1.250_000_1, 1.0, 1.25),
        ] {
            let got = scale_bucket(zoom, ppp);
            assert!(
                (got - want).abs() < 1e-6,
                "scale_bucket({zoom}, {ppp}) = {got}"
            );
        }
    }

    #[test]
    fn max_scale_px_values() {
        let letter = PageSize { w: 612.0, h: 792.0 };
        let a0 = PageSize {
            w: 2384.0,
            h: 3370.0,
        };
        assert!(close(max_scale_px(&[letter], 16384), 16383.0 / 792.0));
        assert!(close(max_scale_px(&[a0], 8192), 8191.0 / 3370.0));
        assert!(max_scale_px(&[], 8192).is_infinite());
    }

    #[test]
    fn parse_page_input_values() {
        assert_eq!(parse_page_input("3", 10), Some(2));
        assert_eq!(parse_page_input(" 1 ", 10), Some(0));
        assert_eq!(parse_page_input("10", 10), Some(9));
        for (s, total) in [
            ("0", 10),
            ("11", 10),
            ("abc", 10),
            ("", 10),
            ("-1", 10),
            ("1", 0),
        ] {
            assert_eq!(parse_page_input(s, total), None, "{s:?} of {total}");
        }
    }
}
