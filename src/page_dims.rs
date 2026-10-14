//! ページ寸法を GPU テクスチャの生存期間から切り離して保持する。

use std::collections::HashMap;

/// 注釈やクリック判定の正規ピクセル座標に使える、回転前のページ寸法。
/// サムネイル raster や PDF page box のレイアウト単位とは区別する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CanonicalPageDimensions {
    width: u32,
    height: u32,
}

impl CanonicalPageDimensions {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub const fn as_tuple(self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub const fn as_size(self) -> [usize; 2] {
        [self.width as usize, self.height as usize]
    }
}

/// ピクセル寸法の出所。小さい PDF サムネイルも形状判定には使えるが、
/// 正規ページ座標として公開してはいけない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PagePixelDimensions {
    Thumbnail((u32, u32)),
    Canonical(CanonicalPageDimensions),
}

impl PagePixelDimensions {
    pub const fn as_tuple(self) -> (u32, u32) {
        match self {
            Self::Thumbnail(dims) => dims,
            Self::Canonical(dims) => dims.as_tuple(),
        }
    }
}

/// 一度判明したページのピクセル寸法とレイアウト寸法を、テクスチャの生存期間から
/// 切り離して覚えておく。両者は単位と用途が異なるため別 map で所有する。
#[derive(Clone, Debug, Default)]
pub struct PageDimsCache {
    generation: u64,
    dims: HashMap<usize, PagePixelDimensions>,
    layout_dims: HashMap<usize, (u32, u32)>,
}

impl PageDimsCache {
    /// 出所つきピクセル寸法を記録する。
    ///
    /// `generation` が保持中のものと違えば、別の items 添字空間なので先に中身を捨てる。
    /// 同世代では Thumbnail は Canonical を上書きせず、Canonical の訂正は採用する。
    pub fn record(&mut self, generation: u64, idx: usize, dims: PagePixelDimensions) {
        if self.generation != generation {
            self.dims.clear();
            self.layout_dims.clear();
            self.generation = generation;
        }
        if !matches!(self.dims.get(&idx), Some(PagePixelDimensions::Canonical(_)))
            || matches!(dims, PagePixelDimensions::Canonical(_))
        {
            self.dims.insert(idx, dims);
        }
    }

    /// 現在の items 世代で判明済みの形状判定用寸法を返す。
    ///
    /// Thumbnail も含むため、注釈などの正規ピクセル座標には `get_canonical` を使う。
    /// 世代が違うときは、同じ idx の別 item を参照しないよう fail-closed で `None` を返す。
    pub fn get(&self, generation: u64, idx: usize) -> Option<(u32, u32)> {
        (self.generation == generation)
            .then(|| {
                self.dims
                    .get(&idx)
                    .copied()
                    .map(PagePixelDimensions::as_tuple)
            })
            .flatten()
    }

    /// 現在の items 世代で判明済みの正規ピクセル寸法だけを返す。
    pub fn get_canonical(&self, generation: u64, idx: usize) -> Option<CanonicalPageDimensions> {
        if self.generation != generation {
            return None;
        }
        match self.dims.get(&idx) {
            Some(PagePixelDimensions::Canonical(dims)) => Some(*dims),
            Some(PagePixelDimensions::Thumbnail(_)) | None => None,
        }
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
    use super::{CanonicalPageDimensions, PageDimsCache, PagePixelDimensions};

    fn canonical(width: u32, height: u32) -> PagePixelDimensions {
        PagePixelDimensions::Canonical(CanonicalPageDimensions::new(width, height))
    }

    #[test]
    fn records_and_gets_dims_in_the_same_generation() {
        let mut cache = PageDimsCache::default();
        cache.record(7, 3, canonical(1800, 1100));

        assert_eq!(cache.get(7, 3), Some((1800, 1100)));
        let dims = cache.get_canonical(7, 3).unwrap();
        assert_eq!(dims.as_tuple(), (1800, 1100));
        assert_eq!(dims.as_size(), [1800, 1100]);
    }

    #[test]
    fn generation_mismatch_fails_closed() {
        let mut cache = PageDimsCache::default();
        cache.record(7, 3, canonical(1800, 1100));

        assert_eq!(cache.get(8, 3), None);
        assert_eq!(cache.get_canonical(8, 3), None);
        assert_eq!(cache.get(7, 3), Some((1800, 1100)));
        assert_eq!(cache.get_canonical(7, 3).unwrap().as_tuple(), (1800, 1100));
    }

    #[test]
    fn recording_a_new_generation_discards_old_indices() {
        let mut cache = PageDimsCache::default();
        cache.record(7, 3, canonical(1800, 1100));
        cache.record_layout(7, 3, (468_600, 714_360));
        cache.record(8, 4, PagePixelDimensions::Thumbnail((900, 1400)));

        assert_eq!(cache.get(8, 3), None);
        assert_eq!(cache.get_canonical(8, 3), None);
        assert_eq!(cache.get_layout(8, 3), None);
        assert_eq!(cache.get(8, 4), Some((900, 1400)));
        assert_eq!(cache.get_canonical(8, 4), None);
    }

    #[test]
    fn clear_forgets_dims_without_changing_the_generation_contract() {
        let mut cache = PageDimsCache::default();
        cache.record(7, 3, canonical(1800, 1100));
        cache.record_layout(7, 3, (468_600, 714_360));
        cache.clear();

        assert_eq!(cache.get(7, 3), None);
        assert_eq!(cache.get_canonical(7, 3), None);
        assert_eq!(cache.get_layout(7, 3), None);

        // clear 後は同じ世代・idx の新しい thumbnail も Canonical に妨げられない。
        cache.record(7, 3, PagePixelDimensions::Thumbnail((273, 416)));
        assert_eq!(cache.get(7, 3), Some((273, 416)));
        assert_eq!(cache.get_canonical(7, 3), None);
    }

    #[test]
    fn layout_dims_survive_independently_from_pixel_dims() {
        let mut cache = PageDimsCache::default();
        cache.record(7, 3, PagePixelDimensions::Thumbnail((273, 416)));
        cache.record_layout(7, 3, (468_600, 714_360));

        assert_eq!(cache.get(7, 3), Some((273, 416)));
        assert_eq!(cache.get_canonical(7, 3), None);
        assert_eq!(cache.get_layout(7, 3), Some((468_600, 714_360)));
    }

    #[test]
    fn pdf_thumbnail_cannot_supply_canonical_page_coordinates() {
        let mut cache = PageDimsCache::default();
        cache.record(7, 3, PagePixelDimensions::Thumbnail((273, 416)));

        assert_eq!(cache.get(7, 3), Some((273, 416)));
        assert_eq!(cache.get_canonical(7, 3), None);

        cache.record(7, 3, canonical(1800, 2744));
        assert_eq!(cache.get(7, 3), Some((1800, 2744)));
        assert_eq!(cache.get_canonical(7, 3).unwrap().as_tuple(), (1800, 2744));
    }

    #[test]
    fn later_thumbnail_preserves_known_canonical_dimensions() {
        let mut cache = PageDimsCache::default();
        cache.record(7, 3, canonical(1800, 2744));
        cache.record(7, 3, PagePixelDimensions::Thumbnail((273, 416)));

        assert_eq!(cache.get(7, 3), Some((1800, 2744)));
        assert_eq!(cache.get_canonical(7, 3).unwrap().as_tuple(), (1800, 2744));
    }

    #[test]
    fn same_generation_can_update_thumbnail_and_canonical_dimensions() {
        let mut cache = PageDimsCache::default();
        cache.record(7, 3, PagePixelDimensions::Thumbnail((273, 416)));
        cache.record(7, 3, PagePixelDimensions::Thumbnail((546, 832)));
        assert_eq!(cache.get(7, 3), Some((546, 832)));
        assert_eq!(cache.get_canonical(7, 3), None);

        cache.record(7, 3, canonical(1800, 2744));
        cache.record(7, 3, canonical(2400, 3658));
        assert_eq!(cache.get(7, 3), Some((2400, 3658)));
        assert_eq!(cache.get_canonical(7, 3).unwrap().as_tuple(), (2400, 3658));
    }

    #[test]
    fn new_generation_thumbnail_replaces_old_canonical_at_same_index() {
        let mut cache = PageDimsCache::default();
        cache.record(7, 3, canonical(1800, 2744));
        cache.record(8, 3, PagePixelDimensions::Thumbnail((273, 416)));

        assert_eq!(cache.get(8, 3), Some((273, 416)));
        assert_eq!(cache.get_canonical(8, 3), None);
        assert_eq!(cache.get_canonical(7, 3), None);
    }

    #[test]
    fn pdf_layout_updates_never_change_pixel_provenance() {
        let mut cache = PageDimsCache::default();
        cache.record_layout(7, 3, (468_600, 714_360));
        assert_eq!(cache.get(7, 3), None);
        assert_eq!(cache.get_canonical(7, 3), None);
        assert_eq!(cache.get_layout(8, 3), None);

        cache.record(7, 3, canonical(1800, 2744));
        cache.record_layout(7, 3, (480_000, 731_600));
        assert_eq!(cache.get_canonical(7, 3).unwrap().as_tuple(), (1800, 2744));
        assert_eq!(cache.get_layout(7, 3), Some((480_000, 731_600)));

        cache.record_layout(8, 3, (468_600, 714_360));
        assert_eq!(cache.get(8, 3), None);
        assert_eq!(cache.get_canonical(8, 3), None);
        assert_eq!(cache.get_layout(8, 3), Some((468_600, 714_360)));
    }

    #[test]
    fn cloned_context_keeps_independent_provenance_and_layout() {
        let mut original = PageDimsCache::default();
        original.record(7, 3, canonical(1800, 2744));
        original.record_layout(7, 3, (468_600, 714_360));
        let mut sibling = original.clone();

        sibling.record(7, 3, canonical(2400, 3658));
        sibling.record_layout(7, 3, (480_000, 731_600));
        assert_eq!(
            original.get_canonical(7, 3).unwrap().as_tuple(),
            (1800, 2744)
        );
        assert_eq!(original.get_layout(7, 3), Some((468_600, 714_360)));

        original.clear();
        assert_eq!(
            sibling.get_canonical(7, 3).unwrap().as_tuple(),
            (2400, 3658)
        );
        assert_eq!(sibling.get_layout(7, 3), Some((480_000, 731_600)));

        original.record(8, 3, PagePixelDimensions::Thumbnail((273, 416)));
        assert_eq!(
            sibling.get_canonical(7, 3).unwrap().as_tuple(),
            (2400, 3658)
        );
        assert_eq!(sibling.get_canonical(8, 3), None);
    }
}
