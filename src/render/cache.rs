//! GPU page textures and the render requests still in flight.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

use egui::{TextureHandle, TextureId};

/// Texture identity: page plus exact render scale (f32 bits, so it can be hashed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TexKey {
    pub page: usize,
    pub scale_px_bits: u32,
}

impl TexKey {
    pub fn new(page: usize, scale_px: f32) -> Self {
        Self {
            page,
            scale_px_bits: scale_px.to_bits(),
        }
    }

    pub fn scale_px(self) -> f32 {
        f32::from_bits(self.scale_px_bits)
    }
}

/// GPU page textures. Dropping a `TextureHandle` frees the texture.
#[derive(Default)]
pub struct PageCache {
    textures: HashMap<TexKey, TextureHandle>,
    /// Keys requested from the worker and not yet answered (maintained by `start_batch`).
    pub in_flight: HashSet<TexKey>,
}

impl PageCache {
    /// Stores a texture and removes `key` from `in_flight`.
    pub fn insert(&mut self, key: TexKey, tex: TextureHandle) {
        self.in_flight.remove(&key);
        self.textures.insert(key, tex);
    }

    /// Records `missing` as in flight and returns the keys to send, or `None` if all are already
    /// in flight. `true` means a new generation: an in-flight key is no longer wanted, so every key
    /// is resent (the worker drops older generations). Otherwise only the new keys are sent, under
    /// the current generation, so a page the worker is rendering right now is not queued again.
    pub fn start_batch(&mut self, missing: &[TexKey]) -> Option<(bool, Vec<TexKey>)> {
        let fresh: Vec<TexKey> = missing
            .iter()
            .filter(|k| !self.in_flight.contains(k))
            .copied()
            .collect();
        if fresh.is_empty() {
            return None;
        }
        if self.in_flight.iter().all(|k| missing.contains(k)) {
            self.in_flight.extend(&fresh);
            Some((false, fresh))
        } else {
            self.in_flight = missing.iter().copied().collect();
            Some((true, missing.to_vec()))
        }
    }

    /// Exact `(page, wanted_px)` texture → `(id, true)`; else this page's highest-scale texture → `(id, false)`.
    pub fn get_best(&self, page: usize, wanted_px: f32) -> Option<(TextureId, bool)> {
        if let Some(tex) = self.textures.get(&TexKey::new(page, wanted_px)) {
            return Some((tex.id(), true));
        }
        let key = self.highest_scale(page)?;
        self.textures.get(&key).map(|tex| (tex.id(), false))
    }

    /// Keeps exact-scale textures for pages in `visible.start-2 .. visible.end+2`, plus, for each
    /// visible page WITHOUT an exact texture, its single highest-scale fallback. Drops the rest.
    pub fn retain(&mut self, visible: Range<usize>, wanted_px: f32) {
        let window = visible.start.saturating_sub(2)..visible.end + 2;
        let fallbacks: Vec<TexKey> = visible
            .filter(|&p| !self.textures.contains_key(&TexKey::new(p, wanted_px)))
            .filter_map(|p| self.highest_scale(p))
            .collect();
        let wanted_bits = wanted_px.to_bits();
        self.textures.retain(|k, _| {
            (k.scale_px_bits == wanted_bits && window.contains(&k.page)) || fallbacks.contains(k)
        });
    }

    /// Drops all textures and in-flight keys (new document).
    pub fn clear(&mut self) {
        self.textures.clear();
        self.in_flight.clear();
    }

    /// Key of this page's highest-scale texture.
    fn highest_scale(&self, page: usize) -> Option<TexKey> {
        self.textures
            .keys()
            .filter(|k| k.page == page)
            .max_by(|a, b| a.scale_px().total_cmp(&b.scale_px()))
            .copied()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.textures.len()
    }
}

#[cfg(test)]
mod tests {
    use egui::{Color32, ColorImage, TextureOptions};

    use super::*;

    /// Inserts a 1×1 texture for `(page, scale_px)` and returns its id.
    fn add(cache: &mut PageCache, ctx: &egui::Context, page: usize, scale_px: f32) -> TextureId {
        let image = ColorImage::filled([1, 1], Color32::WHITE);
        let tex = ctx.load_texture("t", image, TextureOptions::LINEAR);
        let id = tex.id();
        cache.insert(TexKey::new(page, scale_px), tex);
        id
    }

    #[test]
    fn get_best_prefers_exact() {
        let (ctx, mut c) = (egui::Context::default(), PageCache::default());
        let one = add(&mut c, &ctx, 0, 1.0);
        add(&mut c, &ctx, 0, 2.0);
        assert_eq!(c.get_best(0, 1.0), Some((one, true)));
    }

    #[test]
    fn get_best_falls_back_to_highest_scale() {
        let (ctx, mut c) = (egui::Context::default(), PageCache::default());
        add(&mut c, &ctx, 0, 1.0);
        let two = add(&mut c, &ctx, 0, 2.0);
        assert_eq!(c.get_best(0, 1.5), Some((two, false)));
    }

    #[test]
    fn get_best_missing_page_none() {
        let (ctx, mut c) = (egui::Context::default(), PageCache::default());
        add(&mut c, &ctx, 0, 1.0);
        assert_eq!(c.get_best(5, 1.0), None);
    }

    #[test]
    fn insert_clears_in_flight() {
        let (ctx, mut c) = (egui::Context::default(), PageCache::default());
        c.in_flight.insert(TexKey::new(3, 1.0));
        add(&mut c, &ctx, 3, 1.0);
        assert!(c.in_flight.is_empty());
    }

    #[test]
    fn start_batch_sends_only_new_keys() {
        let mut c = PageCache::default();
        let k = |page| TexKey::new(page, 1.0);
        assert_eq!(c.start_batch(&[k(0)]), Some((false, vec![k(0)])));
        // Regression: k(0) may be rendering right now; resending it rendered it twice.
        assert_eq!(c.start_batch(&[k(0), k(1)]), Some((false, vec![k(1)])));
        assert_eq!(c.start_batch(&[k(0), k(1)]), None);
        assert_eq!(c.in_flight.len(), 2);
    }

    #[test]
    fn start_batch_new_generation_when_key_unwanted() {
        let mut c = PageCache::default();
        let k = |page| TexKey::new(page, 1.0);
        c.start_batch(&[k(0), k(1)]);
        assert_eq!(c.start_batch(&[k(1), k(2)]), Some((true, vec![k(1), k(2)])));
        assert_eq!(c.in_flight, HashSet::from([k(1), k(2)]));
    }

    #[test]
    fn retain_keeps_exact_in_window() {
        let (ctx, mut c) = (egui::Context::default(), PageCache::default());
        for p in 0..10 {
            add(&mut c, &ctx, p, 1.0);
        }
        c.retain(4..6, 1.0);
        assert_eq!(c.len(), 6);
        assert!((2..=7).all(|p| c.get_best(p, 1.0).is_some()));
    }

    #[test]
    fn retain_keeps_one_fallback_for_visible() {
        let (ctx, mut c) = (egui::Context::default(), PageCache::default());
        add(&mut c, &ctx, 4, 1.0);
        let best = add(&mut c, &ctx, 4, 1.5);
        c.retain(4..5, 2.0);
        assert_eq!(c.len(), 1);
        assert_eq!(c.get_best(4, 2.0), Some((best, false)));
    }

    #[test]
    fn retain_drops_fallback_when_exact_present() {
        let (ctx, mut c) = (egui::Context::default(), PageCache::default());
        add(&mut c, &ctx, 4, 1.0);
        let exact = add(&mut c, &ctx, 4, 2.0);
        c.retain(4..5, 2.0);
        assert_eq!(c.len(), 1);
        assert_eq!(c.get_best(4, 2.0), Some((exact, true)));
    }

    #[test]
    fn retain_drops_offscreen_fallback() {
        let (ctx, mut c) = (egui::Context::default(), PageCache::default());
        add(&mut c, &ctx, 8, 1.0);
        c.retain(4..5, 2.0);
        assert_eq!(c.len(), 0);
    }
}
