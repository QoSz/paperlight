//! Pure text-selection logic: char picking, reading-order ranges, highlight runs and copy text.

use std::{collections::HashMap, ops::Range};

use egui::{Pos2, Rect};

use crate::layout::PageLayout;

/// Farthest a drag may start from a char and still select, in page points.
pub const ANCHOR_CAP: f32 = 12.0;

/// One char of a page's text. `rect` is in page points, TOP-left origin (the worker has flipped it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharBox {
    pub rect: Rect,
    pub ch: char,
}

/// Chars of every page loaded so far, by page index.
pub type PageTexts = HashMap<usize, Box<[CharBox]>>;

/// A position between chars in reading order: before char `index` of `page`
/// (`index == len` is after the page's last char).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Caret {
    page: usize,
    index: usize,
}

/// Where a doc-space point lies relative to the pages.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Probe {
    /// Within the vertical span of `page`, at this point in page points (top-left origin).
    At(usize, Pos2),
    /// In the gap or margin above `page`, nearer to it: before its first char.
    Start(usize),
    /// In the gap or margin below `page`, nearer to it: after its last char.
    End(usize),
}

impl Probe {
    fn page(self) -> usize {
        match self {
            Self::At(page, _) | Self::Start(page) | Self::End(page) => page,
        }
    }
}

/// A drag selection: where it was pressed, where the pointer is, and the chars between them.
/// Stored as page points, so it survives zoom and resolves once the pages' chars arrive.
#[derive(Debug, Default)]
pub struct Selection {
    /// Press point: page and page points.
    anchor: Option<(usize, Pos2)>,
    head: Option<Probe>,
    /// Selected chars as `[start, end)` carets; `None` until resolved.
    range: Option<(Caret, Caret)>,
}

impl Selection {
    /// Starts a selection pressed at `at` (page points) on `page`. Call `resolve` after.
    pub fn begin(&mut self, page: usize, at: Pos2) {
        self.anchor = Some((page, at));
        self.head = Some(Probe::At(page, at));
        self.range = None;
    }

    /// Selects the chars `indices` of `page` outright (double- and triple-click). Not dragged, so
    /// `resolve` leaves it alone.
    pub fn select(&mut self, page: usize, indices: Range<usize>) {
        self.anchor = None;
        self.head = None;
        self.range = Some((caret(page, indices.start), caret(page, indices.end)));
    }

    /// Moves the dragged end. Call `resolve` after.
    pub fn drag_to(&mut self, head: Probe) {
        self.head = Some(head);
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// No selection started (an unresolved one is not empty).
    pub fn is_empty(&self) -> bool {
        self.anchor.is_none() && self.range.is_none()
    }

    /// Pages from the anchor's to the head's (or of the range), in order; empty without a selection.
    pub fn pages(&self) -> Range<usize> {
        match (self.anchor, self.head, self.range) {
            (Some((a, _)), Some(head), _) => a.min(head.page())..a.max(head.page()) + 1,
            (None, _, Some((lo, hi))) => lo.page..hi.page + 1,
            _ => 0..0,
        }
    }

    /// Selected chars as `[start, end)` carets, if resolved.
    pub fn range(&self) -> Option<(Caret, Caret)> {
        self.range
    }

    /// Recomputes a dragged range: the anchor's char (within `ANCHOR_CAP`) through the char nearest
    /// the head, both included. `None` while the anchor misses text or a needed page is not loaded.
    pub fn resolve(&mut self, texts: &PageTexts) {
        if self.anchor.is_some() {
            self.range = self.compute(texts);
        }
    }

    /// The selected text for the clipboard.
    pub fn text(&self, texts: &PageTexts) -> Option<String> {
        self.range.map(|(lo, hi)| copy_text(texts, lo, hi))
    }

    fn compute(&self, texts: &PageTexts) -> Option<(Caret, Caret)> {
        let (page, at) = self.anchor?;
        let index = nearest(texts.get(&page)?, at, ANCHOR_CAP)?;
        let head = self.head?;
        let chars = texts.get(&head.page())?;
        let (head_lo, head_hi) = match head {
            Probe::At(p, pt) => nearest(chars, pt, f32::INFINITY)
                .map_or((caret(p, 0), caret(p, 0)), |i| {
                    (caret(p, i), caret(p, i + 1))
                }),
            Probe::Start(p) => (caret(p, 0), caret(p, 0)),
            Probe::End(p) => (caret(p, chars.len()), caret(p, chars.len())),
        };
        Some((
            caret(page, index).min(head_lo),
            caret(page, index + 1).max(head_hi),
        ))
    }
}

fn caret(page: usize, index: usize) -> Caret {
    Caret { page, index }
}

/// Index of the char whose box is nearest to `p` (page points), if within `cap` points. Among
/// boxes at the same distance (overlapping loose boxes), the one with the nearest centre wins.
pub fn nearest(chars: &[CharBox], p: Pos2, cap: f32) -> Option<usize> {
    chars
        .iter()
        .enumerate()
        .map(|(i, c)| {
            (
                i,
                c.rect.distance_sq_to_pos(p),
                c.rect.center().distance_sq(p),
            )
        })
        .filter(|&(_, d, _)| d <= cap * cap)
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.2.total_cmp(&b.2)))
        .map(|(i, ..)| i)
}

/// Where the doc-space point `doc` lies; `None` without pages. Gaps resolve to the nearer page edge.
pub fn probe(layout: &PageLayout, doc: Pos2) -> Option<Probe> {
    let rects = layout.rects();
    let last = rects.len().checked_sub(1)?;
    let page = rects.partition_point(|r| r.max.y < doc.y);
    let Some(r) = rects.get(page) else {
        return Some(Probe::End(last));
    };
    if doc.y >= r.min.y {
        return Some(Probe::At(page, layout.to_page(page, doc)));
    }
    Some(match page.checked_sub(1) {
        Some(above) if doc.y - rects[above].max.y < r.min.y - doc.y => Probe::End(above),
        _ => Probe::Start(page),
    })
}

/// The part of `[lo, hi)` on `page`, as indices into its `len` chars (clamped).
pub fn page_span(lo: Caret, hi: Caret, page: usize, len: usize) -> Range<usize> {
    if page < lo.page || page > hi.page {
        return 0..0;
    }
    let start = if page == lo.page { lo.index } else { 0 }.min(len);
    let end = if page == hi.page { hi.index } else { len };
    start..end.min(len).max(start)
}

/// The chars of `[lo, hi)`. pdfium's own `\r\n` chars are kept; pages are joined with `\n`.
fn copy_text(texts: &PageTexts, lo: Caret, hi: Caret) -> String {
    let mut out = String::new();
    for page in lo.page..=hi.page {
        if page > lo.page {
            out.push('\n');
        }
        if let Some(chars) = texts.get(&page) {
            let span = page_span(lo, hi, page, chars.len());
            out.extend(chars[span].iter().map(|c| c.ch));
        }
    }
    out
}

/// The run of chars around `i` for which `keep` holds, as indices; `i` itself is always in it.
fn run_around(chars: &[CharBox], i: usize, keep: impl Fn(&CharBox) -> bool) -> Range<usize> {
    let start = chars[..i]
        .iter()
        .rposition(|c| !keep(c))
        .map_or(0, |p| p + 1);
    let end = chars[i..]
        .iter()
        .position(|c| !keep(c))
        .map_or(chars.len(), |p| i + p);
    start..end
}

/// pdfium's generated chars (spaces, line breaks) have no box.
fn is_boxless(c: &CharBox) -> bool {
    c.rect.width() <= 0.0 || c.rect.height() <= 0.0
}

/// The word around char `i`: the run of non-whitespace chars containing it. A whitespace char
/// selects just itself.
pub fn word_at(chars: &[CharBox], i: usize) -> Range<usize> {
    let Some(at) = chars.get(i) else {
        return chars.len()..chars.len();
    };
    if at.ch.is_whitespace() {
        return i..i + 1;
    }
    run_around(chars, i, |c| !c.ch.is_whitespace())
}

/// The line around char `i`: the neighbours whose boxes share its line (see [`runs`]). Boxless
/// chars inside the line are kept, and the line breaks ending it too, so a copied line keeps its
/// newline; boxless chars before its first glyph are not.
pub fn line_at(chars: &[CharBox], i: usize) -> Range<usize> {
    let Some(at) = chars.get(i) else {
        return chars.len()..chars.len();
    };
    if is_boxless(at) {
        return i..i + 1;
    }
    let line = run_around(chars, i, |c| is_boxless(c) || same_line(at.rect, c.rect));
    let lead = chars[line.clone()]
        .iter()
        .take_while(|c| is_boxless(c))
        .count();
    line.start + lead..line.end
}

/// Highlight rects for consecutive selected chars: one per text line. A char joins the run when
/// its box overlaps the previous box vertically by more than half the smaller height.
/// Zero-size boxes (pdfium's generated spaces and line breaks) are skipped. Allocation-free.
pub fn runs(chars: &[CharBox]) -> impl Iterator<Item = Rect> + '_ {
    let mut rects = chars
        .iter()
        .map(|c| c.rect)
        .filter(|r| r.width() > 0.0 && r.height() > 0.0)
        .peekable();
    std::iter::from_fn(move || {
        let mut last = rects.next()?;
        let mut run = last;
        while let Some(next) = rects.next_if(|r| same_line(last, *r)) {
            run = run.union(next);
            last = next;
        }
        Some(run)
    })
}

fn same_line(a: Rect, b: Rect) -> bool {
    let overlap = a.max.y.min(b.max.y) - a.min.y.max(b.min.y);
    overlap > 0.5 * a.height().min(b.height())
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    use crate::layout::{PageSize, Rotation};

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
    }

    /// One line of 10×10 chars starting at `(0, y)`; `\r`/`\n` get a zero-size box at the baseline.
    fn line(text: &str, y: f32) -> Vec<CharBox> {
        let mut x = 0.0;
        text.chars()
            .map(|ch| {
                if ch == '\r' || ch == '\n' {
                    return CharBox {
                        rect: rect(x, y + 8.0, x, y + 8.0),
                        ch,
                    };
                }
                x += 10.0;
                CharBox {
                    rect: rect(x - 10.0, y, x, y + 10.0),
                    ch,
                }
            })
            .collect()
    }

    fn texts(pages: &[(usize, Vec<CharBox>)]) -> PageTexts {
        pages
            .iter()
            .map(|(p, chars)| (*p, chars.as_slice().into()))
            .collect()
    }

    #[test]
    fn nearest_picks_char_and_caps_distance() {
        let chars = line("ab", 0.0);
        assert_eq!(nearest(&chars, pos2(15.0, 5.0), ANCHOR_CAP), Some(1));
        assert_eq!(nearest(&chars, pos2(25.0, 5.0), ANCHOR_CAP), Some(1));
        assert_eq!(nearest(&chars, pos2(40.0, 5.0), ANCHOR_CAP), None);
        assert_eq!(nearest(&chars, pos2(40.0, 5.0), f32::INFINITY), Some(1));
        assert_eq!(nearest(&[], pos2(0.0, 0.0), f32::INFINITY), None);
    }

    #[test]
    fn nearest_prefers_closer_centre_in_overlapping_lines() {
        // Loose boxes of consecutive lines overlap in 10..12.
        let chars = [
            CharBox {
                rect: rect(0.0, 0.0, 10.0, 12.0),
                ch: 'a',
            },
            CharBox {
                rect: rect(0.0, 10.0, 10.0, 22.0),
                ch: 'b',
            },
        ];
        assert_eq!(nearest(&chars, pos2(5.0, 10.5), 0.0), Some(0));
        assert_eq!(nearest(&chars, pos2(5.0, 11.5), 0.0), Some(1));
    }

    #[test]
    fn range_is_ordered_across_pages() {
        let t = texts(&[(1, line("abc", 0.0)), (2, line("def", 0.0))]);
        let mut sel = Selection::default();
        // Backward drag: from 'e' on page 2 up to 'c' on page 1.
        sel.begin(2, pos2(15.0, 5.0));
        sel.drag_to(Probe::At(1, pos2(25.0, 5.0)));
        sel.resolve(&t);
        assert_eq!(sel.range(), Some((caret(1, 2), caret(2, 2))));
        assert_eq!(sel.pages(), 1..3);
        // Forward drag from 'a' to 'e'.
        sel.begin(1, pos2(5.0, 5.0));
        sel.drag_to(Probe::At(2, pos2(15.0, 5.0)));
        sel.resolve(&t);
        assert_eq!(sel.range(), Some((caret(1, 0), caret(2, 2))));
    }

    #[test]
    fn range_in_one_char_selects_it() {
        let t = texts(&[(0, line("abc", 0.0))]);
        let mut sel = Selection::default();
        sel.begin(0, pos2(12.0, 5.0));
        sel.drag_to(Probe::At(0, pos2(18.0, 5.0)));
        sel.resolve(&t);
        assert_eq!(sel.range(), Some((caret(0, 1), caret(0, 2))));
    }

    #[test]
    fn range_needs_anchor_near_text() {
        let t = texts(&[(0, line("abc", 0.0))]);
        let mut sel = Selection::default();
        sel.begin(0, pos2(100.0, 5.0));
        sel.drag_to(Probe::At(0, pos2(5.0, 5.0)));
        sel.resolve(&t);
        assert_eq!(sel.range(), None);
        assert!(!sel.is_empty());
    }

    #[test]
    fn range_head_in_gap_uses_page_edges() {
        let t = texts(&[(1, line("abc", 0.0)), (2, line("def", 0.0))]);
        let mut sel = Selection::default();
        sel.begin(1, pos2(15.0, 5.0));
        sel.drag_to(Probe::Start(2));
        sel.resolve(&t);
        assert_eq!(sel.range(), Some((caret(1, 1), caret(2, 0))));
        sel.drag_to(Probe::End(1));
        sel.resolve(&t);
        assert_eq!(sel.range(), Some((caret(1, 1), caret(1, 3))));
    }

    #[test]
    fn range_waits_for_chars() {
        let mut t = texts(&[(0, line("abc", 0.0))]);
        let mut sel = Selection::default();
        sel.begin(0, pos2(5.0, 5.0));
        sel.drag_to(Probe::At(1, pos2(5.0, 5.0)));
        sel.resolve(&t);
        assert_eq!(sel.range(), None);
        t.insert(1, line("de", 0.0).into());
        sel.resolve(&t);
        assert_eq!(sel.range(), Some((caret(0, 0), caret(1, 1))));
    }

    #[test]
    fn clear_empties_selection() {
        let t = texts(&[(0, line("abc", 0.0))]);
        let mut sel = Selection::default();
        sel.begin(0, pos2(5.0, 5.0));
        sel.resolve(&t);
        sel.clear();
        assert!(sel.is_empty());
        assert_eq!(sel.range(), None);
        assert_eq!(sel.pages(), 0..0);
        assert_eq!(sel.text(&t), None);
    }

    #[test]
    fn select_sets_range_without_anchor() {
        let t = texts(&[(2, line("ab cd", 0.0))]);
        let mut sel = Selection::default();
        sel.select(2, 3..5);
        assert!(!sel.is_empty());
        assert_eq!(sel.range(), Some((caret(2, 3), caret(2, 5))));
        assert_eq!(sel.pages(), 2..3);
        // Chars arriving later must not disturb a click selection.
        sel.resolve(&t);
        assert_eq!(sel.text(&t), Some("cd".to_owned()));
    }

    #[test]
    fn word_at_values() {
        let chars = line("ab cd\r\n", 0.0);
        assert_eq!(word_at(&chars, 0), 0..2);
        assert_eq!(word_at(&chars, 4), 3..5);
        assert_eq!(word_at(&chars, 2), 2..3);
        assert_eq!(word_at(&chars, 9), 7..7);
        assert_eq!(word_at(&[], 0), 0..0);
    }

    #[test]
    fn line_at_values() {
        let mut chars = line("ab cd\r\n", 0.0);
        chars.extend(line("ef", 12.0));
        assert_eq!(line_at(&chars, 1), 0..7);
        assert_eq!(line_at(&chars, 8), 7..9);
        assert_eq!(line_at(&chars, 20), 9..9);
        // A boxless char selects only itself.
        assert_eq!(line_at(&chars, 5), 5..6);
    }

    #[test]
    fn line_at_spans_generated_spaces() {
        // pdfium gives generated spaces a zero-size box; they must not split the line.
        let mut chars = line("ab", 0.0);
        chars.push(CharBox {
            rect: rect(20.0, 8.0, 20.0, 8.0),
            ch: ' ',
        });
        chars.extend(line("cd", 0.0));
        assert_eq!(line_at(&chars, 4), 0..5);
        assert_eq!(line_at(&chars, 0), 0..5);
    }

    #[test]
    fn probe_maps_pages_and_gaps() {
        // Pages at y 30..830, 850..1650, 1670..2470; x 100..700.
        let l = PageLayout::new(
            &[PageSize { w: 600.0, h: 800.0 }; 3],
            1.0,
            Rotation::R0,
            800.0,
            20.0,
            30.0,
        );
        assert_eq!(
            probe(&l, pos2(400.0, 100.0)),
            Some(Probe::At(0, pos2(300.0, 70.0)))
        );
        assert_eq!(probe(&l, pos2(400.0, 835.0)), Some(Probe::End(0)));
        assert_eq!(probe(&l, pos2(400.0, 845.0)), Some(Probe::Start(1)));
        assert_eq!(probe(&l, pos2(400.0, 10.0)), Some(Probe::Start(0)));
        assert_eq!(probe(&l, pos2(400.0, 3000.0)), Some(Probe::End(2)));
        assert_eq!(probe(&PageLayout::default(), pos2(0.0, 0.0)), None);
        let zoomed = PageLayout::new(
            &[PageSize { w: 600.0, h: 800.0 }],
            2.0,
            Rotation::R0,
            800.0,
            20.0,
            30.0,
        );
        assert_eq!(
            probe(&zoomed, pos2(50.0, 70.0)),
            Some(Probe::At(0, pos2(10.0, 20.0)))
        );
    }

    #[test]
    fn runs_merge_per_line() {
        // "abc\r\n" on line 0, "de" on line 12; the zero-size `\r\n` boxes neither paint nor split.
        let mut chars = line("abc\r\n", 0.0);
        chars.extend(line("de", 12.0));
        let got: Vec<Rect> = runs(&chars).collect();
        assert_eq!(
            got,
            [rect(0.0, 0.0, 30.0, 10.0), rect(0.0, 12.0, 20.0, 22.0)]
        );
    }

    #[test]
    fn runs_split_on_small_vertical_overlap() {
        // A superscript overlapping its neighbour by 40% starts a new run.
        let chars = [
            CharBox {
                rect: rect(0.0, 10.0, 10.0, 20.0),
                ch: 'x',
            },
            CharBox {
                rect: rect(10.0, 4.0, 20.0, 14.0),
                ch: '2',
            },
        ];
        assert_eq!(runs(&chars).count(), 2);
        assert_eq!(runs(&[]).count(), 0);
    }

    #[test]
    fn page_span_values() {
        let (lo, hi) = (caret(1, 2), caret(3, 4));
        assert_eq!(page_span(lo, hi, 0, 10), 0..0);
        assert_eq!(page_span(lo, hi, 1, 10), 2..10);
        assert_eq!(page_span(lo, hi, 2, 10), 0..10);
        assert_eq!(page_span(lo, hi, 3, 10), 0..4);
        assert_eq!(page_span(lo, hi, 4, 10), 0..0);
        // Indices past a page's end are clamped.
        assert_eq!(page_span(lo, hi, 3, 2), 0..2);
        assert_eq!(page_span(caret(1, 5), caret(1, 9), 1, 3), 3..3);
    }

    #[test]
    fn copy_text_keeps_newlines_and_joins_pages() {
        let mut first = line("ab\r\n", 0.0);
        first.extend(line("cd", 12.0));
        let t = texts(&[(1, first), (3, line("ef", 0.0))]);
        assert_eq!(copy_text(&t, caret(1, 1), caret(3, 1)), "b\r\ncd\n\ne");
        assert_eq!(copy_text(&t, caret(1, 0), caret(1, 2)), "ab");
    }
}
