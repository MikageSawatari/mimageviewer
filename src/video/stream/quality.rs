use std::fmt;

/// 正本 `docs/web-remote-video-streaming-plan.md` §6.4 の手動画質プリセット。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum QualityPreset {
    Minimum,
    Low,
    #[default]
    Standard,
    High,
}

impl From<crate::settings::RemoteVideoQuality> for QualityPreset {
    fn from(value: crate::settings::RemoteVideoQuality) -> Self {
        use crate::settings::RemoteVideoQuality;
        match value {
            RemoteVideoQuality::Minimum => Self::Minimum,
            RemoteVideoQuality::Low => Self::Low,
            RemoteVideoQuality::Standard => Self::Standard,
            RemoteVideoQuality::High => Self::High,
        }
    }
}

impl QualityPreset {
    #[cfg(test)]
    pub(crate) const ALL: [Self; 4] = [Self::Minimum, Self::Low, Self::Standard, Self::High];

    pub(crate) const fn parameters(self) -> QualityPresetParameters {
        match self {
            Self::Minimum => QualityPresetParameters {
                max_long_edge: 640,
                video_bitrate_bps: 400_000,
                audio_bitrate_bps: 64_000,
            },
            Self::Low => QualityPresetParameters {
                max_long_edge: 854,
                video_bitrate_bps: 800_000,
                audio_bitrate_bps: 96_000,
            },
            Self::Standard => QualityPresetParameters {
                max_long_edge: 1_280,
                video_bitrate_bps: 1_500_000,
                audio_bitrate_bps: 128_000,
            },
            Self::High => QualityPresetParameters {
                max_long_edge: 1_920,
                video_bitrate_bps: 3_000_000,
                audio_bitrate_bps: 160_000,
            },
        }
    }

    pub(crate) fn output_parameters(
        self,
        source_width: u32,
        source_height: u32,
    ) -> Result<StreamOutputParameters, OutputDimensionsError> {
        let preset = self.parameters();
        Ok(StreamOutputParameters {
            dimensions: calculate_output_dimensions(
                source_width,
                source_height,
                preset.max_long_edge,
            )?,
            video_bitrate_bps: preset.video_bitrate_bps,
            audio_bitrate_bps: preset.audio_bitrate_bps,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QualityPresetParameters {
    pub(crate) max_long_edge: u32,
    pub(crate) video_bitrate_bps: u32,
    pub(crate) audio_bitrate_bps: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StreamOutputParameters {
    pub(crate) dimensions: OutputDimensions,
    pub(crate) video_bitrate_bps: u32,
    pub(crate) audio_bitrate_bps: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OutputDimensions {
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputDimensionsError {
    ZeroSourceDimension,
    LongEdgeBelowH264Minimum,
    SourceDimensionBelowH264Minimum,
}

impl fmt::Display for OutputDimensionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ZeroSourceDimension => "source dimensions must be non-zero",
            Self::LongEdgeBelowH264Minimum => "maximum long edge must be at least 2 pixels",
            Self::SourceDimensionBelowH264Minimum => {
                "source dimensions cannot be represented as non-upscaled even H.264 dimensions"
            }
        })
    }
}

impl std::error::Error for OutputDimensionsError {}

/// アスペクト比を保って比例縮小し、各辺を偶数へ切り下げる。短辺の比例値が 2 未満に
/// なる極端な比率だけ、H.264 で表現できる最小値 2 へ飽和する。
pub(crate) fn calculate_output_dimensions(
    source_width: u32,
    source_height: u32,
    max_long_edge: u32,
) -> Result<OutputDimensions, OutputDimensionsError> {
    if source_width == 0 || source_height == 0 {
        return Err(OutputDimensionsError::ZeroSourceDimension);
    }
    if max_long_edge < 2 {
        return Err(OutputDimensionsError::LongEdgeBelowH264Minimum);
    }
    if source_width < 2 || source_height < 2 {
        return Err(OutputDimensionsError::SourceDimensionBelowH264Minimum);
    }

    let source_long_edge = source_width.max(source_height);
    let target_long_edge = source_long_edge.min(max_long_edge);
    let scaled_width =
        u64::from(source_width) * u64::from(target_long_edge) / u64::from(source_long_edge);
    let scaled_height =
        u64::from(source_height) * u64::from(target_long_edge) / u64::from(source_long_edge);
    Ok(OutputDimensions {
        width: even_down_or_minimum(scaled_width),
        height: even_down_or_minimum(scaled_height),
    })
}

fn even_down_or_minimum(value: u64) -> u32 {
    if value < 2 { 2 } else { (value as u32) & !1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_preset_table_matches_canonical_section_6_4() {
        let expected = [
            (QualityPreset::Minimum, 640, 400_000, 64_000),
            (QualityPreset::Low, 854, 800_000, 96_000),
            (QualityPreset::Standard, 1_280, 1_500_000, 128_000),
            (QualityPreset::High, 1_920, 3_000_000, 160_000),
        ];
        assert_eq!(QualityPreset::ALL.len(), expected.len());
        for (preset, max_long_edge, video_bitrate_bps, audio_bitrate_bps) in expected {
            assert_eq!(
                preset.parameters(),
                QualityPresetParameters {
                    max_long_edge,
                    video_bitrate_bps,
                    audio_bitrate_bps,
                }
            );
        }
        assert_eq!(QualityPreset::default(), QualityPreset::Standard);
    }

    #[test]
    fn output_dimensions_never_upscale_and_preserve_aspect() {
        assert_eq!(
            calculate_output_dimensions(640, 360, 1_280),
            Ok(OutputDimensions {
                width: 640,
                height: 360,
            })
        );
        assert_eq!(
            calculate_output_dimensions(3_840, 2_160, 1_280),
            Ok(OutputDimensions {
                width: 1_280,
                height: 720,
            })
        );
        assert_eq!(
            calculate_output_dimensions(1_920, 1_080, 854),
            Ok(OutputDimensions {
                width: 854,
                height: 480,
            })
        );
    }

    #[test]
    fn output_dimensions_treat_portrait_height_as_long_edge() {
        assert_eq!(
            calculate_output_dimensions(1_080, 1_920, 854),
            Ok(OutputDimensions {
                width: 480,
                height: 854,
            })
        );
    }

    #[test]
    fn output_dimensions_round_both_edges_down_to_even() {
        assert_eq!(
            calculate_output_dimensions(853, 479, 1_920),
            Ok(OutputDimensions {
                width: 852,
                height: 478,
            })
        );
        assert_eq!(
            calculate_output_dimensions(1_919, 1_079, 1_280),
            Ok(OutputDimensions {
                width: 1_280,
                height: 718,
            })
        );
    }

    #[test]
    fn output_dimensions_handle_extreme_ratios_without_zero_edges() {
        assert_eq!(
            calculate_output_dimensions(8, 8_192, 640),
            Ok(OutputDimensions {
                width: 2,
                height: 640,
            })
        );
        assert_eq!(
            calculate_output_dimensions(8_192, 8, 640),
            Ok(OutputDimensions {
                width: 640,
                height: 2,
            })
        );
    }

    #[test]
    fn output_dimensions_reject_impossible_inputs() {
        assert_eq!(
            calculate_output_dimensions(0, 1_080, 1_280),
            Err(OutputDimensionsError::ZeroSourceDimension)
        );
        assert_eq!(
            calculate_output_dimensions(1, 1_080, 1_280),
            Err(OutputDimensionsError::SourceDimensionBelowH264Minimum)
        );
        assert_eq!(
            calculate_output_dimensions(1_920, 1_080, 1),
            Err(OutputDimensionsError::LongEdgeBelowH264Minimum)
        );
    }
}
