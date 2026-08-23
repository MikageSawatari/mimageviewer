//! Pure policy and persistent values for native-video Anime4K selection.

use serde::{Deserialize, Serialize};

pub const VIDEO_ANIME4K_MEASUREMENT_SCHEMA: u32 = 1;
pub const VIDEO_ANIME4K_SOURCE_MAX_PIXELS: u64 = 1920 * 1080;
pub const VIDEO_ANIME4K_MEASUREMENT_TOTAL: u32 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoAnime4kVariant {
    Small,
    Medium,
    Large,
    VeryLarge,
    UltraLarge,
}

impl VideoAnime4kVariant {
    pub const ALL: [Self; 5] = [
        Self::Small,
        Self::Medium,
        Self::Large,
        Self::VeryLarge,
        Self::UltraLarge,
    ];
    pub const MEASURED: [Self; 3] = [Self::Small, Self::Large, Self::UltraLarge];
    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "S",
            Self::Medium => "M",
            Self::Large => "L",
            Self::VeryLarge => "VL",
            Self::UltraLarge => "UL",
        }
    }
    pub fn intermediate_count(self) -> u64 {
        match self {
            Self::Small => 4,
            Self::Medium => 8,
            Self::Large => 9,
            Self::VeryLarge => 17,
            Self::UltraLarge => 24,
        }
    }
    fn work_units(self) -> f64 {
        match self {
            Self::Small => 11.0,
            Self::Medium => 24.0,
            Self::Large => 50.0,
            Self::VeryLarge => 100.0,
            Self::UltraLarge => 204.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoAnime4kBudgetPreset {
    Speed,
    #[default]
    Standard,
    Quality,
    FixedSmall,
    FixedMedium,
    FixedLarge,
    FixedVeryLarge,
    FixedUltraLarge,
}

impl VideoAnime4kBudgetPreset {
    pub const ALL: [Self; 8] = [
        Self::Speed,
        Self::Standard,
        Self::Quality,
        Self::FixedSmall,
        Self::FixedMedium,
        Self::FixedLarge,
        Self::FixedVeryLarge,
        Self::FixedUltraLarge,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Speed => "速度優先",
            Self::Standard => "標準",
            Self::Quality => "画質優先",
            Self::FixedSmall => "S に固定",
            Self::FixedMedium => "M に固定",
            Self::FixedLarge => "L に固定",
            Self::FixedVeryLarge => "VL に固定",
            Self::FixedUltraLarge => "UL に固定",
        }
    }
    pub fn budget_percent(self) -> Option<u32> {
        match self {
            Self::Speed => Some(20),
            Self::Standard => Some(40),
            Self::Quality => Some(60),
            _ => None,
        }
    }
    pub fn fixed_variant(self) -> Option<VideoAnime4kVariant> {
        match self {
            Self::FixedSmall => Some(VideoAnime4kVariant::Small),
            Self::FixedMedium => Some(VideoAnime4kVariant::Medium),
            Self::FixedLarge => Some(VideoAnime4kVariant::Large),
            Self::FixedVeryLarge => Some(VideoAnime4kVariant::VeryLarge),
            Self::FixedUltraLarge => Some(VideoAnime4kVariant::UltraLarge),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoAnime4kAdapterKey {
    pub luid_low: u32,
    pub luid_high: i32,
    pub vendor_id: u32,
    pub device_id: u32,
    pub subsystem_id: u32,
    pub revision: u32,
    pub driver_version: u64,
    pub dedicated_video_memory: u64,
    pub shared_system_memory: u64,
    pub description: String,
}

impl VideoAnime4kAdapterKey {
    pub fn anime4k_vram_budget_bytes(&self) -> u64 {
        self.dedicated_video_memory
            .max(self.shared_system_memory)
            .saturating_div(4)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoAnime4kMeasurementPoint {
    pub variant: VideoAnime4kVariant,
    pub source_width: u32,
    pub source_height: u32,
    pub output_width: u32,
    pub output_height: u32,
    pub gpu_time_us: u64,
}

impl VideoAnime4kMeasurementPoint {
    pub fn source_pixels(self) -> u64 {
        u64::from(self.source_width).saturating_mul(u64::from(self.source_height))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoAnime4kMeasurementCache {
    pub schema: u32,
    pub adapter: VideoAnime4kAdapterKey,
    pub points: Vec<VideoAnime4kMeasurementPoint>,
}

impl VideoAnime4kMeasurementCache {
    pub fn is_valid_for(&self, adapter: &VideoAnime4kAdapterKey) -> bool {
        self.schema == VIDEO_ANIME4K_MEASUREMENT_SCHEMA
            && self.adapter == *adapter
            && self.points.len() == VIDEO_ANIME4K_MEASUREMENT_TOTAL as usize
            && VideoAnime4kVariant::MEASURED.into_iter().all(|variant| {
                self.points
                    .iter()
                    .filter(|point| {
                        point.variant == variant
                            && point.gpu_time_us > 0
                            && point.gpu_time_us != u64::MAX
                    })
                    .count()
                    == 2
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoAnime4kFallbackReason {
    SourceTooLarge {
        source_pixels: u64,
        max_pixels: u64,
    },
    InsufficientVideoMemory {
        required_bytes: u64,
        budget_bytes: u64,
    },
    MeasuredTooSlow {
        predicted_us: u64,
        budget_us: u64,
    },
    MeasurementUnavailable,
    MeasurementFailed,
}

impl VideoAnime4kFallbackReason {
    pub fn perf_name(self) -> &'static str {
        match self {
            Self::SourceTooLarge { .. } => "anime_source_too_large",
            Self::InsufficientVideoMemory { .. } => "anime_insufficient_video_memory",
            Self::MeasuredTooSlow { .. } => "anime_measured_too_slow",
            Self::MeasurementUnavailable => "anime_measurement_unavailable",
            Self::MeasurementFailed => "anime_measurement_failed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoAnime4kSelection {
    Selected {
        variant: VideoAnime4kVariant,
        predicted_us: u64,
        budget_us: Option<u64>,
        required_vram_bytes: u64,
    },
    Fallback(VideoAnime4kFallbackReason),
}

pub fn anime4k_intermediate_vram_bytes(variant: VideoAnime4kVariant, source_pixels: u64) -> u64 {
    variant
        .intermediate_count()
        .saturating_mul(source_pixels)
        .saturating_mul(8)
}

pub fn select_video_anime4k_variant(
    measurements: &[VideoAnime4kMeasurementPoint],
    preset: VideoAnime4kBudgetPreset,
    source_pixels: u64,
    fps: f64,
    vram_budget_bytes: u64,
) -> VideoAnime4kSelection {
    if source_pixels == 0 || source_pixels > VIDEO_ANIME4K_SOURCE_MAX_PIXELS {
        return VideoAnime4kSelection::Fallback(VideoAnime4kFallbackReason::SourceTooLarge {
            source_pixels,
            max_pixels: VIDEO_ANIME4K_SOURCE_MAX_PIXELS,
        });
    }
    if let Some(variant) = preset.fixed_variant() {
        let required = anime4k_intermediate_vram_bytes(variant, source_pixels);
        if required > vram_budget_bytes {
            return VideoAnime4kSelection::Fallback(
                VideoAnime4kFallbackReason::InsufficientVideoMemory {
                    required_bytes: required,
                    budget_bytes: vram_budget_bytes,
                },
            );
        }
        return VideoAnime4kSelection::Selected {
            variant,
            predicted_us: predict_variant_time_us(measurements, variant, source_pixels)
                .unwrap_or_default(),
            budget_us: None,
            required_vram_bytes: required,
        };
    }
    let Some(percent) = preset.budget_percent() else {
        return VideoAnime4kSelection::Fallback(VideoAnime4kFallbackReason::MeasurementUnavailable);
    };
    if !fps.is_finite() || fps <= 0.0 {
        return VideoAnime4kSelection::Fallback(VideoAnime4kFallbackReason::MeasurementUnavailable);
    }
    let budget_us = ((1_000_000.0 / fps) * f64::from(percent) / 100.0)
        .round()
        .max(1.0) as u64;
    let mut smallest_prediction = None;
    let mut smallest_vram_failure = None;
    for variant in VideoAnime4kVariant::ALL.into_iter().rev() {
        let Some(predicted_us) = predict_variant_time_us(measurements, variant, source_pixels)
        else {
            return VideoAnime4kSelection::Fallback(
                VideoAnime4kFallbackReason::MeasurementUnavailable,
            );
        };
        if variant == VideoAnime4kVariant::Small {
            smallest_prediction = Some(predicted_us);
        }
        let required = anime4k_intermediate_vram_bytes(variant, source_pixels);
        if variant == VideoAnime4kVariant::Small && required > vram_budget_bytes {
            smallest_vram_failure = Some((required, vram_budget_bytes));
        }
        if predicted_us <= budget_us && required <= vram_budget_bytes {
            return VideoAnime4kSelection::Selected {
                variant,
                predicted_us,
                budget_us: Some(budget_us),
                required_vram_bytes: required,
            };
        }
    }
    if let Some((required_bytes, budget_bytes)) = smallest_vram_failure {
        return VideoAnime4kSelection::Fallback(
            VideoAnime4kFallbackReason::InsufficientVideoMemory {
                required_bytes,
                budget_bytes,
            },
        );
    }
    VideoAnime4kSelection::Fallback(VideoAnime4kFallbackReason::MeasuredTooSlow {
        predicted_us: smallest_prediction.unwrap_or_default(),
        budget_us,
    })
}

fn predict_variant_time_us(
    measurements: &[VideoAnime4kMeasurementPoint],
    variant: VideoAnime4kVariant,
    source_pixels: u64,
) -> Option<u64> {
    let anchor = |value| predict_anchor_time_us(measurements, value, source_pixels);
    let value = match variant {
        VideoAnime4kVariant::Small
        | VideoAnime4kVariant::Large
        | VideoAnime4kVariant::UltraLarge => anchor(variant)?,
        VideoAnime4kVariant::Medium => interpolate_work(
            anchor(VideoAnime4kVariant::Small)?,
            VideoAnime4kVariant::Small,
            anchor(VideoAnime4kVariant::Large)?,
            VideoAnime4kVariant::Large,
            variant,
        ),
        VideoAnime4kVariant::VeryLarge => interpolate_work(
            anchor(VideoAnime4kVariant::Large)?,
            VideoAnime4kVariant::Large,
            anchor(VideoAnime4kVariant::UltraLarge)?,
            VideoAnime4kVariant::UltraLarge,
            variant,
        ),
    };
    Some(value.round().max(1.0) as u64)
}

fn predict_anchor_time_us(
    measurements: &[VideoAnime4kMeasurementPoint],
    variant: VideoAnime4kVariant,
    source_pixels: u64,
) -> Option<f64> {
    let mut points = measurements
        .iter()
        .copied()
        .filter(|point| {
            point.variant == variant && point.gpu_time_us > 0 && point.gpu_time_us != u64::MAX
        })
        .collect::<Vec<_>>();
    points.sort_by_key(|point| point.source_pixels());
    if points.len() != 2 || points[0].source_pixels() == points[1].source_pixels() {
        return None;
    }
    let (x0, x1) = (
        points[0].source_pixels() as f64,
        points[1].source_pixels() as f64,
    );
    let (y0, y1) = (points[0].gpu_time_us as f64, points[1].gpu_time_us as f64);
    Some((y0 + (y1 - y0) * (source_pixels as f64 - x0) / (x1 - x0)).max(1.0))
}

fn interpolate_work(
    low_time: f64,
    low: VideoAnime4kVariant,
    high_time: f64,
    high: VideoAnime4kVariant,
    target: VideoAnime4kVariant,
) -> f64 {
    low_time
        + (high_time - low_time) * (target.work_units() - low.work_units())
            / (high.work_units() - low.work_units())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn table() -> Vec<VideoAnime4kMeasurementPoint> {
        [
            (VideoAnime4kVariant::Small, 1_000, 3_000),
            (VideoAnime4kVariant::Large, 2_000, 6_000),
            (VideoAnime4kVariant::UltraLarge, 4_000, 12_000),
        ]
        .into_iter()
        .flat_map(|(variant, low, high)| {
            [
                VideoAnime4kMeasurementPoint {
                    variant,
                    source_width: 960,
                    source_height: 540,
                    output_width: 1920,
                    output_height: 1080,
                    gpu_time_us: low,
                },
                VideoAnime4kMeasurementPoint {
                    variant,
                    source_width: 1920,
                    source_height: 1080,
                    output_width: 3840,
                    output_height: 2160,
                    gpu_time_us: high,
                },
            ]
        })
        .collect()
    }
    #[test]
    fn selection_uses_table_budget_pixels_and_fps() {
        assert!(matches!(
            select_video_anime4k_variant(
                &table(),
                VideoAnime4kBudgetPreset::Standard,
                1920 * 1080,
                60.0,
                u64::MAX
            ),
            VideoAnime4kSelection::Selected {
                variant: VideoAnime4kVariant::Large,
                predicted_us: 6_000,
                budget_us: Some(6_667),
                ..
            }
        ));
        assert!(matches!(
            select_video_anime4k_variant(
                &table(),
                VideoAnime4kBudgetPreset::Standard,
                1920 * 1080,
                120.0,
                u64::MAX
            ),
            VideoAnime4kSelection::Selected {
                variant: VideoAnime4kVariant::Small,
                ..
            }
        ));
    }
    #[test]
    fn selection_interpolates_unmeasured_variants() {
        assert!(matches!(
            select_video_anime4k_variant(
                &table(),
                VideoAnime4kBudgetPreset::Speed,
                960 * 540,
                60.0,
                u64::MAX
            ),
            VideoAnime4kSelection::Selected {
                variant: VideoAnime4kVariant::VeryLarge,
                ..
            }
        ));
    }
    #[test]
    fn selection_accounts_for_intermediate_vram() {
        let pixels = 1920 * 1080;
        let bytes = anime4k_intermediate_vram_bytes(VideoAnime4kVariant::Small, pixels);
        assert!(matches!(
            select_video_anime4k_variant(
                &table(),
                VideoAnime4kBudgetPreset::Quality,
                pixels,
                30.0,
                bytes - 1
            ),
            VideoAnime4kSelection::Fallback(
                VideoAnime4kFallbackReason::InsufficientVideoMemory { .. }
            )
        ));
    }
    #[test]
    fn selection_falls_back_when_even_small_is_too_slow() {
        assert!(matches!(
            select_video_anime4k_variant(
                &table(),
                VideoAnime4kBudgetPreset::Speed,
                1920 * 1080,
                240.0,
                u64::MAX
            ),
            VideoAnime4kSelection::Fallback(VideoAnime4kFallbackReason::MeasuredTooSlow { .. })
        ));
    }
    #[test]
    fn fixed_variant_still_obeys_safety_guards() {
        assert!(matches!(
            select_video_anime4k_variant(
                &[],
                VideoAnime4kBudgetPreset::FixedVeryLarge,
                960 * 540,
                60.0,
                u64::MAX
            ),
            VideoAnime4kSelection::Selected {
                variant: VideoAnime4kVariant::VeryLarge,
                budget_us: None,
                ..
            }
        ));
        assert!(matches!(
            select_video_anime4k_variant(
                &[],
                VideoAnime4kBudgetPreset::FixedSmall,
                VIDEO_ANIME4K_SOURCE_MAX_PIXELS + 1,
                60.0,
                u64::MAX
            ),
            VideoAnime4kSelection::Fallback(VideoAnime4kFallbackReason::SourceTooLarge { .. })
        ));
    }

    #[test]
    fn measurement_cache_is_invalidated_by_driver_or_failed_point() {
        let adapter = VideoAnime4kAdapterKey {
            luid_low: 1,
            luid_high: 2,
            vendor_id: 3,
            device_id: 4,
            subsystem_id: 5,
            revision: 6,
            driver_version: 7,
            dedicated_video_memory: 8,
            shared_system_memory: 9,
            description: "adapter".to_string(),
        };
        let cache = VideoAnime4kMeasurementCache {
            schema: VIDEO_ANIME4K_MEASUREMENT_SCHEMA,
            adapter: adapter.clone(),
            points: table(),
        };
        assert!(cache.is_valid_for(&adapter));

        let mut changed_driver = adapter.clone();
        changed_driver.driver_version += 1;
        assert!(!cache.is_valid_for(&changed_driver));

        let mut failed = cache;
        failed.points[0].gpu_time_us = u64::MAX;
        assert!(!failed.is_valid_for(&adapter));
    }
}
