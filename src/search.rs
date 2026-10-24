//! Pure search state: query, results, selected hit.

use egui::Rect;

/// One match. `rects` are in page points, TOP-left origin (the worker has already flipped them).
#[derive(Debug, PartialEq)]
pub struct Hit {
    pub page: usize,
    pub rects: Vec<Rect>,
}

/// Query, results and the selected hit. `hits` are sorted by page (worker order).
#[derive(Debug, Default)]
pub struct SearchState {
    pub query: String,
    generation: u64,
    hits: Vec<Hit>,
    pub current: Option<usize>,
    pending: bool,
}

impl SearchState {
    /// New query: bumps `generation`, clears hits and selection; pending unless `query` is empty.
    /// Returns the generation to send with `Cmd::Search` (always sent, including for "").
    pub fn start(&mut self, query: &str) -> u64 {
        query.clone_into(&mut self.query);
        self.generation += 1;
        self.hits.clear();
        self.current = None;
        self.pending = !query.is_empty();
        self.generation
    }

    /// Drops the result if `generation` is stale. Otherwise stores the hits, clears `pending` and
    /// selects the first hit with `page >= from_page` (else the first). Returns `true` if accepted.
    pub fn set_results(&mut self, generation: u64, hits: Vec<Hit>, from_page: usize) -> bool {
        if generation != self.generation {
            return false;
        }
        self.current =
            (!hits.is_empty()).then(|| hits.iter().position(|h| h.page >= from_page).unwrap_or(0));
        self.hits = hits;
        self.pending = false;
        true
    }

    /// Selects the next hit, wrapping to the first. `None` if there are no hits.
    pub fn next(&mut self) -> Option<&Hit> {
        let n = self.hits.len();
        if n == 0 {
            return None;
        }
        self.current = Some(self.current.map_or(0, |c| (c + 1) % n));
        self.current_hit()
    }

    /// Selects the previous hit, wrapping to the last. `None` if there are no hits.
    pub fn prev(&mut self) -> Option<&Hit> {
        let n = self.hits.len();
        if n == 0 {
            return None;
        }
        self.current = Some(self.current.map_or(n - 1, |c| (c + n - 1) % n));
        self.current_hit()
    }

    /// The selected hit.
    pub fn current_hit(&self) -> Option<&Hit> {
        self.hits.get(self.current?)
    }

    /// `(index of the first hit on page, hits on page)`. Used for overlays.
    pub fn page_hits(&self, page: usize) -> (usize, &[Hit]) {
        let start = self.hits.partition_point(|h| h.page < page);
        let end = self.hits.partition_point(|h| h.page <= page);
        (start, &self.hits[start..end])
    }

    /// Empties query and hits and makes in-flight results stale.
    pub fn clear(&mut self) -> u64 {
        self.start("")
    }

    /// Toolbar count: "" (empty query), "…" (pending), "0 / 0", "3 / 17" (current+1 / len).
    pub fn label(&self) -> String {
        if self.query.is_empty() {
            String::new()
        } else if self.pending {
            "…".to_owned()
        } else {
            format!(
                "{} / {}",
                self.current.map_or(0, |c| c + 1),
                self.hits.len()
            )
        }
    }

    /// `true` when a finished non-empty query has no hits.
    pub fn no_results(&self) -> bool {
        !self.query.is_empty() && !self.pending && self.hits.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(page: usize) -> Hit {
        Hit {
            page,
            rects: vec![],
        }
    }

    fn hits(pages: &[usize]) -> Vec<Hit> {
        pages.iter().copied().map(hit).collect()
    }

    /// State with finished results on `pages`, first hit selected.
    fn with_results(pages: &[usize]) -> SearchState {
        let mut s = SearchState::default();
        let g = s.start("q");
        assert!(s.set_results(g, hits(pages), 0));
        s
    }

    #[test]
    fn start_bumps_generation_and_clears() {
        let mut s = with_results(&[1, 2]);
        let g = s.generation;
        assert_eq!(s.start("x"), g + 1);
        assert_eq!(s.hits, []);
        assert_eq!(s.current, None);
        assert!(s.pending);
    }

    #[test]
    fn start_empty_not_pending() {
        let mut s = SearchState::default();
        let g = s.generation;
        assert_eq!(s.start(""), g + 1);
        assert!(!s.pending);
    }

    #[test]
    fn stale_results_ignored() {
        let mut s = SearchState::default();
        let g1 = s.start("a");
        let _g2 = s.start("ab");
        assert!(!s.set_results(g1, hits(&[0]), 0));
        assert_eq!(s.hits, []);
        assert!(s.pending);
    }

    #[test]
    fn results_select_first_at_or_after_page() {
        let mut s = SearchState::default();
        let g = s.start("a");
        assert!(s.set_results(g, hits(&[1, 4, 9]), 5));
        assert_eq!(s.current, Some(2));

        let g = s.start("a");
        assert!(s.set_results(g, hits(&[1, 4, 9]), 10));
        assert_eq!(s.current, Some(0));

        let g = s.start("a");
        assert!(s.set_results(g, vec![], 0));
        assert_eq!(s.current, None);
        assert!(!s.pending);
    }

    #[test]
    fn next_wraps() {
        let mut s = with_results(&[1, 4, 9]);
        s.current = Some(2);
        assert_eq!(s.next().map(|h| h.page), Some(1));
        assert_eq!(s.current, Some(0));
    }

    #[test]
    fn prev_wraps() {
        let mut s = with_results(&[1, 4, 9]);
        s.current = Some(0);
        assert_eq!(s.prev().map(|h| h.page), Some(9));
        assert_eq!(s.current, Some(2));
    }

    #[test]
    fn next_prev_empty_none() {
        let mut s = with_results(&[]);
        assert_eq!(s.next(), None);
        assert_eq!(s.prev(), None);
    }

    #[test]
    fn page_hits_slices() {
        let s = with_results(&[1, 1, 4]);
        assert_eq!(s.page_hits(1), (0, &s.hits[0..2]));
        assert_eq!(s.page_hits(4), (2, &s.hits[2..3]));
        assert_eq!(s.page_hits(2), (2, &[][..]));
    }

    #[test]
    fn clear_invalidates_in_flight() {
        let mut s = SearchState::default();
        let g = s.start("a");
        s.clear();
        assert!(!s.set_results(g, hits(&[0]), 0));
        assert_eq!(s.label(), "");
    }

    #[test]
    fn label_formats() {
        let mut s = SearchState::default();
        assert_eq!(s.label(), "");
        let g = s.start("a");
        assert_eq!(s.label(), "…");
        assert!(s.set_results(g, vec![], 0));
        assert_eq!(s.label(), "0 / 0");
        assert!(s.no_results());
        let mut s = with_results(&[1, 4, 9]);
        s.current = Some(1);
        assert_eq!(s.label(), "2 / 3");
        assert!(!s.no_results());
    }
}
