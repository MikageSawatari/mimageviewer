//! mImageViewer 全体の GPU テクスチャ予算・会計に使う共通ロジック。
//!
//! 予算は RGBA8 相当の bytes / texels で扱い、テクスチャ会計は実寸と mip chain、
//! `TextureId` の重複排除を単一の実装へ集約する。

use std::collections::HashSet;

pub(crate) const RGBA8_BYTES_PER_TEXEL: u64 = 4;
pub(crate) const VRAM_PRIMARY_SHARE_PERCENT: u64 = 80;
pub(crate) const VRAM_SECONDARY_SHARE_PERCENT: u64 = 20;
pub(crate) const VRAM_LOW_WATERMARK_NUMERATOR: u64 = 3;
pub(crate) const VRAM_LOW_WATERMARK_DENOMINATOR: u64 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VramBudgetMode {
    Grid,
    Fullscreen,
}

impl VramBudgetMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Grid => "grid",
            Self::Fullscreen => "fullscreen",
        }
    }
}

/// `None` は設定値 0% による無制限を表す。0 bytes と混同しない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VramPoolBudget {
    pub(crate) pool_bytes: Option<u64>,
    pub(crate) thumbnail_bytes: Option<u64>,
    pub(crate) fullscreen_bytes: Option<u64>,
}

impl VramPoolBudget {
    pub(crate) fn from_percent(percent: u32, mode: VramBudgetMode) -> Self {
        if percent == 0 {
            return Self::from_pool_bytes(None, mode);
        }
        Self::from_pool_bytes(Some(crate::gpu_info::vram_cap_from_percent(percent)), mode)
    }

    /// VRAM 検出値を注入して予算を組む。実機の DXGI 問い合わせに依存せず
    /// 予算計算だけを検証するためのテスト用シーム。
    #[cfg(test)]
    pub(crate) fn from_detected_vram(
        percent: u32,
        mode: VramBudgetMode,
        detected_vram_bytes: Option<u64>,
    ) -> Self {
        if percent == 0 {
            return Self::from_pool_bytes(None, mode);
        }
        Self::from_pool_bytes(
            Some(crate::gpu_info::vram_cap_from_percent_for_total(
                percent,
                detected_vram_bytes,
            )),
            mode,
        )
    }

    pub(crate) fn from_pool_bytes(pool_bytes: Option<u64>, mode: VramBudgetMode) -> Self {
        let (thumbnail_percent, fullscreen_percent) = match mode {
            VramBudgetMode::Grid => (VRAM_PRIMARY_SHARE_PERCENT, VRAM_SECONDARY_SHARE_PERCENT),
            VramBudgetMode::Fullscreen => {
                (VRAM_SECONDARY_SHARE_PERCENT, VRAM_PRIMARY_SHARE_PERCENT)
            }
        };
        Self {
            pool_bytes,
            thumbnail_bytes: pool_bytes.map(|bytes| percent_of(bytes, thumbnail_percent)),
            fullscreen_bytes: pool_bytes.map(|bytes| percent_of(bytes, fullscreen_percent)),
        }
    }

    pub(crate) fn fullscreen_watermarks(self) -> VramWatermarks {
        let high_texels = self.fullscreen_bytes.map(rgba8_bytes_to_texels);
        VramWatermarks {
            high_texels,
            low_texels: high_texels.map(|high| {
                high.saturating_mul(VRAM_LOW_WATERMARK_NUMERATOR as usize)
                    / VRAM_LOW_WATERMARK_DENOMINATOR as usize
            }),
        }
    }
}

fn percent_of(bytes: u64, percent: u64) -> u64 {
    bytes.saturating_mul(percent) / 100
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VramWatermarks {
    pub(crate) high_texels: Option<usize>,
    pub(crate) low_texels: Option<usize>,
}

pub(crate) fn rgba8_bytes_to_texels(bytes: u64) -> usize {
    usize::try_from(bytes / RGBA8_BYTES_PER_TEXEL).unwrap_or(usize::MAX)
}

pub(crate) fn rgba8_texels_to_bytes(texels: usize) -> u64 {
    u64::try_from(texels)
        .unwrap_or(u64::MAX)
        .saturating_mul(RGBA8_BYTES_PER_TEXEL)
}

pub(crate) fn texture_size_texels(size: [usize; 2], mipmapped: bool) -> usize {
    let width = u32::try_from(size[0].max(1)).unwrap_or(u32::MAX);
    let height = u32::try_from(size[1].max(1)).unwrap_or(u32::MAX);
    if mipmapped {
        usize::try_from(egui_wgpu::mip_chain_texel_count(width, height)).unwrap_or(usize::MAX)
    } else {
        size[0].max(1).saturating_mul(size[1].max(1))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub(crate) enum VramSubsystem {
    FsCache,
    FinalCompositeCache,
    AdjustmentCache,
    AiUpscaleCache,
    EraseResultCache,
    LocalAdjustCache,
    ConcealCache,
    ComicCache,
    EditResultCache,
    ContinuousPageTransitions,
    Thumbnails,
    ThumbTextures,
    ThumbAdjustTex,
}

impl VramSubsystem {
    pub(crate) const ALL: [Self; 13] = [
        Self::FsCache,
        Self::FinalCompositeCache,
        Self::AdjustmentCache,
        Self::AiUpscaleCache,
        Self::EraseResultCache,
        Self::LocalAdjustCache,
        Self::ConcealCache,
        Self::ComicCache,
        Self::EditResultCache,
        Self::ContinuousPageTransitions,
        Self::Thumbnails,
        Self::ThumbTextures,
        Self::ThumbAdjustTex,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::FsCache => "fs_cache",
            Self::FinalCompositeCache => "final_composite_cache",
            Self::AdjustmentCache => "adjustment_cache",
            Self::AiUpscaleCache => "ai_upscale_cache",
            Self::EraseResultCache => "erase_result_cache",
            Self::LocalAdjustCache => "local_adjust_cache",
            Self::ConcealCache => "conceal_cache",
            Self::ComicCache => "comic_cache",
            Self::EditResultCache => "edit_result_cache",
            Self::ContinuousPageTransitions => "continuous_page_transitions",
            Self::Thumbnails => "thumbnails",
            Self::ThumbTextures => "thumb_textures",
            Self::ThumbAdjustTex => "thumb_adjust_tex",
        }
    }
}

const VRAM_SUBSYSTEM_COUNT: usize = VramSubsystem::ALL.len();

#[derive(Default)]
pub(crate) struct VramAccountant {
    seen: HashSet<egui::TextureId>,
    subsystem_texels: [usize; VRAM_SUBSYSTEM_COUNT],
}

impl VramAccountant {
    pub(crate) fn add_texture(
        &mut self,
        subsystem: VramSubsystem,
        texture: &egui::TextureHandle,
        mipmapped: bool,
    ) {
        self.add_texture_id(subsystem, texture.id(), texture.size(), mipmapped);
    }

    fn add_texture_id(
        &mut self,
        subsystem: VramSubsystem,
        texture_id: egui::TextureId,
        size: [usize; 2],
        mipmapped: bool,
    ) {
        if !self.seen.insert(texture_id) {
            return;
        }
        let texels = texture_size_texels(size, mipmapped);
        let slot = &mut self.subsystem_texels[subsystem as usize];
        *slot = slot.saturating_add(texels);
    }

    pub(crate) fn finish(self) -> VramAccountingSnapshot {
        let total_texels = self
            .subsystem_texels
            .iter()
            .fold(0usize, |total, texels| total.saturating_add(*texels));
        VramAccountingSnapshot {
            subsystem_texels: self.subsystem_texels,
            total_texels,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VramAccountingSnapshot {
    subsystem_texels: [usize; VRAM_SUBSYSTEM_COUNT],
    pub(crate) total_texels: usize,
}

impl VramAccountingSnapshot {
    pub(crate) fn subsystem_texels(&self, subsystem: VramSubsystem) -> usize {
        self.subsystem_texels[subsystem as usize]
    }

    pub(crate) fn total_bytes(&self) -> u64 {
        rgba8_texels_to_bytes(self.total_texels)
    }

    pub(crate) fn subsystems_json(&self) -> serde_json::Value {
        let mut subsystems = serde_json::Map::with_capacity(VRAM_SUBSYSTEM_COUNT);
        for subsystem in VramSubsystem::ALL {
            let texels = self.subsystem_texels(subsystem);
            let mut values = serde_json::Map::with_capacity(2);
            values.insert("texels".to_string(), (texels as u64).into());
            values.insert("bytes".to_string(), rgba8_texels_to_bytes(texels).into());
            subsystems.insert(subsystem.as_str().to_string(), values.into());
        }
        subsystems.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounting_deduplicates_texture_id_and_aggregates_by_subsystem() {
        let mut accountant = VramAccountant::default();
        accountant.add_texture_id(
            VramSubsystem::FsCache,
            egui::TextureId::Managed(1),
            [4, 3],
            true,
        );
        accountant.add_texture_id(
            VramSubsystem::FinalCompositeCache,
            egui::TextureId::Managed(1),
            [4, 3],
            true,
        );
        accountant.add_texture_id(
            VramSubsystem::FinalCompositeCache,
            egui::TextureId::Managed(2),
            [4, 3],
            false,
        );

        let snapshot = accountant.finish();
        assert_eq!(snapshot.subsystem_texels(VramSubsystem::FsCache), 15);
        assert_eq!(
            snapshot.subsystem_texels(VramSubsystem::FinalCompositeCache),
            12
        );
        assert_eq!(snapshot.total_texels, 27);
        assert_eq!(snapshot.total_bytes(), 108);
    }

    #[test]
    fn texture_texels_include_complete_mip_chain() {
        assert_eq!(texture_size_texels([1, 1], true), 1);
        assert_eq!(texture_size_texels([4, 3], true), 15);
        assert_eq!(texture_size_texels([4, 3], false), 12);
        assert_eq!(texture_size_texels([3, 1], true), 4);
    }

    #[test]
    fn mode_allocations_swap_primary_and_secondary_shares() {
        let pool = 1_000;
        let grid = VramPoolBudget::from_pool_bytes(Some(pool), VramBudgetMode::Grid);
        let fullscreen = VramPoolBudget::from_pool_bytes(Some(pool), VramBudgetMode::Fullscreen);

        assert_eq!(grid.thumbnail_bytes, Some(800));
        assert_eq!(grid.fullscreen_bytes, Some(200));
        assert_eq!(fullscreen.thumbnail_bytes, Some(200));
        assert_eq!(fullscreen.fullscreen_bytes, Some(800));
    }

    #[test]
    fn zero_percent_is_explicitly_unlimited() {
        let budget =
            VramPoolBudget::from_detected_vram(0, VramBudgetMode::Fullscreen, Some(24 << 30));
        assert_eq!(budget.pool_bytes, None);
        assert_eq!(budget.thumbnail_bytes, None);
        assert_eq!(budget.fullscreen_bytes, None);
        assert_eq!(
            budget.fullscreen_watermarks(),
            VramWatermarks {
                high_texels: None,
                low_texels: None,
            }
        );
    }

    #[test]
    fn fullscreen_watermarks_derive_from_pool_and_low_is_three_quarters() {
        let budget = VramPoolBudget::from_pool_bytes(Some(2_000), VramBudgetMode::Fullscreen);
        let watermarks = budget.fullscreen_watermarks();
        assert_eq!(watermarks.high_texels, Some(400));
        assert_eq!(watermarks.low_texels, Some(300));
    }

    #[test]
    fn vram_detection_failure_uses_four_gib_fallback_without_shrinking_below_legacy_high() {
        let budget = VramPoolBudget::from_detected_vram(50, VramBudgetMode::Fullscreen, None);
        let watermarks = budget.fullscreen_watermarks();
        assert_eq!(budget.pool_bytes, Some(2 * 1024 * 1024 * 1024));
        assert!(
            watermarks
                .high_texels
                .is_some_and(|high| high >= 320_000_000)
        );
    }
}
