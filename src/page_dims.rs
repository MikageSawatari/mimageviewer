//! ページ寸法を GPU テクスチャの生存期間から切り離して保持する。

use std::collections::HashMap;

/// 一度判明したページのピクセル寸法とレイアウト寸法を、テクスチャの生存期間から
/// 切り離して覚えておく。両者は単位と用途が異なるため別 map で所有する。
#[derive(Clone, Debug, Default)]
pub struct PageDimsCache {
    generation: u64,
    dims: HashMap<usize, (u32, u32)>,
    layout_dims: HashMap<usize, (u32, u32)>,
}

impl PageDimsCache {
    /// 寸法を記録する。
    ///
    /// `generation` が保持中のものと違えば、別の items 添字空間なので先に中身を捨てる。
    pub fn record(&mut self, generation: u64, idx: usize, dims: (u32, u32)) {
        if self.generation != generation {
            self.dims.clear();
            self.layout_dims.clear();
            self.generation = generation;
        }
        self.dims.insert(idx, dims);
    }

    /// 現在の items 世代で判明済みの寸法を返す。
    ///
    /// 世代が違うときは、同じ idx の別 item を参照しないよう fail-closed で `None` を返す。
    pub fn get(&self, generation: u64, idx: usize) -> Option<(u32, u32)> {
        (self.generation == generation)
            .then(|| self.dims.get(&idx).copied())
            .flatten()
    }

    /// PDF page box など、pixel raster と独立したレイアウト寸法を記録する。
    pub fn record_layout(&mut self, generation: u64, idx: usize, dims: (u32, u32)) {
        if self.generation != generation {
            self.dims.clear();
            self.layout_dims.clear();
            self.generation = generation;
        }
        self.layout_dims.insert(idx, dims);
    }

    /// 現在の items 世代で判明済みのレイアウト寸法を返す。
    pub fn get_layout(&self, generation: u64, idx: usize) -> Option<(u32, u32)> {
        (self.generation == generation)
            .then(|| self.layout_dims.get(&idx).copied())
            .flatten()
    }

    /// 現在保持している寸法をすべて破棄する。
    pub fn clear(&mut self) {
        self.dims.clear();
        self.layout_dims.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::PageDimsCache;

    #[test]
    fn records_and_gets_dims_in_the_same_generation() {
        let mut cache = PageDimsCache::default();
        cache.record(7, 3, (1800, 1100));

        assert_eq!(cache.get(7, 3), Some((1800, 1100)));
    }

    #[test]
    fn generation_mismatch_fails_closed() {
        let mut cache = PageDimsCache::default();
        cache.record(7, 3, (1800, 1100));

        assert_eq!(cache.get(8, 3), None);
        assert_eq!(cache.get(7, 3), Some((1800, 1100)));
    }

    #[test]
    fn recording_a_new_generation_discards_old_indices() {
        let mut cache = PageDimsCache::default();
        cache.record(7, 3, (1800, 1100));
        cache.record(8, 4, (900, 1400));

        assert_eq!(cache.get(8, 3), None);
        assert_eq!(cache.get(8, 4), Some((900, 1400)));
    }

    #[test]
    fn clear_forgets_dims_without_changing_the_generation_contract() {
        let mut cache = PageDimsCache::default();
        cache.record(7, 3, (1800, 1100));
        cache.clear();

        assert_eq!(cache.get(7, 3), None);
    }

    #[test]
    fn layout_dims_survive_independently_from_pixel_dims() {
        let mut cache = PageDimsCache::default();
        cache.record(7, 3, (273, 416));
        cache.record_layout(7, 3, (468_600, 714_360));

        assert_eq!(cache.get(7, 3), Some((273, 416)));
        assert_eq!(cache.get_layout(7, 3), Some((468_600, 714_360)));
    }
}
