//! Worker-produced automatic results; the UI only selects the saved angle.
use crate::cover_auto_crop::{AutoCropDecline, TwoPanelDecline};
#[cfg(test)]
use crate::cover_auto_material::AutoCropResults;
use crate::cover_auto_sample::PageAspectDecline;
use crate::folder_thumb_pins::CoverCrop;
use crate::rotation_db::Rotation;
use crate::settings::ReadingDirection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AutoStop {
    Off,
    Manual,
    EditPreview,
    Unsupported,
    PageAspect(PageAspectDecline),
    AnalysisFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AutoDetection {
    Stopped(AutoStop),
    Analyzed {
        binding: ReadingDirection,
        reports: [Result<CoverCrop, AutoCropDecline>; 4],
    },
}

impl AutoDetection {
    pub(super) fn analyze(
        image: &image::DynamicImage,
        dimensions: (u32, u32),
        sample: &crate::cover_auto_sample::PageAspectSample,
        evidence: crate::cover_auto_crop::photo::PhotoEvidence,
        binding: ReadingDirection,
    ) -> Self {
        let mut reports = [Err(AutoCropDecline::NoStructure); 4];
        crate::cover_auto_crop::detect_cover_crops_with_photo_report(
            image,
            dimensions,
            sample,
            evidence,
            binding,
            |angle, result| reports[angle.degrees() as usize / 90] = result,
        );
        Self::Analyzed { binding, reports }
    }

    pub(super) fn enabled(self) -> bool {
        !matches!(self, Self::Stopped(AutoStop::Off | AutoStop::Unsupported))
    }

    pub(super) fn crop_for(self, rotation: Rotation) -> Option<CoverCrop> {
        match self {
            Self::Analyzed { reports, .. } => reports[rotation.degrees() as usize / 90].ok(),
            Self::Stopped(_) => None,
        }
    }

    #[cfg(test)]
    pub(super) fn crops(self) -> Option<AutoCropResults> {
        match self {
            Self::Analyzed { reports, .. } => Some(AutoCropResults::new(reports.map(Result::ok))),
            Self::Stopped(_) => None,
        }
    }

    #[cfg(test)]
    pub(super) fn from_crops(crops: AutoCropResults) -> Self {
        Self::Analyzed {
            binding: ReadingDirection::Rtl,
            reports: crops
                .crops()
                .map(|crop| crop.ok_or(AutoCropDecline::NoJacketEvidence)),
        }
    }

    pub(super) fn message(self, rotation: Rotation) -> String {
        let reason = match self {
            Self::Stopped(stop) => stop_text(stop),
            Self::Analyzed { binding, reports } => {
                match reports[rotation.degrees() as usize / 90] {
                    Ok(_) => {
                        return format!(
                            "自動の判定: 表表紙を切り出しました (綴じ方向: {})",
                            match binding {
                                ReadingDirection::Rtl => "右綴じ",
                                ReadingDirection::Ltr => "左綴じ",
                            }
                        );
                    }
                    Err(reason) => decline_text(reason),
                }
            }
        };
        format!("自動の判定: 切り出していません。{reason}")
    }
}

fn stop_text(stop: AutoStop) -> &'static str {
    match stop {
        AutoStop::Off => "自動の切り出しはオフです",
        AutoStop::Manual => "手動の範囲を使っています",
        AutoStop::EditPreview => "編集のプレビューがあるため自動は使いません",
        AutoStop::Unsupported => "この形式の本は自動の対象外です",
        AutoStop::AnalysisFailed => "表紙を解析できませんでした",
        AutoStop::PageAspect(PageAspectDecline::TooFew) => {
            "本文ページの形が決まりません (ページが少ない)"
        }
        AutoStop::PageAspect(PageAspectDecline::Landscape) => {
            "本文ページの形が決まりません (横長のページが多い)"
        }
        AutoStop::PageAspect(PageAspectDecline::Inconsistent) => {
            "本文ページの形が決まりません (大きさがそろわない)"
        }
    }
}

fn decline_text(reason: AutoCropDecline) -> &'static str {
    use AutoCropDecline::*;
    match reason {
        PhotoLike { .. } => "写真のように見えるため自動では切りません",
        RawAspectGate | NotWide => "表紙がつながった形 (横長) ではありません",
        // The dialog normally stops before analysis with the sampler's exact
        // typed cause. Keep a diagnostic for a detector-only missing aspect.
        PageAspectMissing => stop_text(AutoStop::PageAspect(PageAspectDecline::TooFew)),
        NoJacketEvidence | NoStructure | NoLocalEvidence | NoSpine => {
            "表表紙と背・袖の境目が見つかりません"
        }
        WeakFront | RicherElsewhere => {
            "表表紙の側が淡く、裏表紙と取り違えるおそれがあるため切りません"
        }
        TwoPanel { reason, .. } => match reason {
            TwoPanelDecline::WeakFront | TwoPanelDecline::RicherElsewhere => {
                "表表紙の側が淡く、裏表紙と取り違えるおそれがあるため切りません"
            }
            TwoPanelDecline::Shape
            | TwoPanelDecline::NoRange
            | TwoPanelDecline::WeakBoundary
            | TwoPanelDecline::OuterWide
            | TwoPanelDecline::NotPaper => "表表紙と背・袖の境目が見つかりません",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cover_auto_crop::TwoPanelTrigger;

    #[test]
    fn cover_crop_auto_success_uses_current_angle_and_binding() {
        let crop = CoverCrop::new([0, 0, 500_000, 1_000_000]).unwrap().unwrap();
        for (binding, label) in [
            (ReadingDirection::Rtl, "右綴じ"),
            (ReadingDirection::Ltr, "左綴じ"),
        ] {
            let detection = AutoDetection::Analyzed {
                binding,
                reports: [
                    Ok(crop),
                    Err(AutoCropDecline::RawAspectGate),
                    Err(AutoCropDecline::NoSpine),
                    Err(AutoCropDecline::WeakFront),
                ],
            };
            assert_eq!(detection.crop_for(Rotation::None), Some(crop));
            assert_eq!(detection.crop_for(Rotation::Cw90), None);
            assert_eq!(
                detection.message(Rotation::None),
                format!("自動の判定: 表表紙を切り出しました (綴じ方向: {label})")
            );
            assert_eq!(
                detection.message(Rotation::Cw90),
                "自動の判定: 切り出していません。表紙がつながった形 (横長) ではありません"
            );
        }
    }

    #[test]
    fn cover_crop_auto_reason_mapping_covers_all_detector_and_preanalysis_reasons() {
        use AutoCropDecline::*;
        let boundary = "表表紙と背・袖の境目が見つかりません";
        let color = "表表紙の側が淡く、裏表紙と取り違えるおそれがあるため切りません";
        for (reason, text) in [
            (RawAspectGate, "表紙がつながった形 (横長) ではありません"),
            (NotWide, "表紙がつながった形 (横長) ではありません"),
            (
                PageAspectMissing,
                "本文ページの形が決まりません (ページが少ない)",
            ),
            (NoJacketEvidence, boundary),
            (NoStructure, boundary),
            (NoLocalEvidence, boundary),
            (NoSpine, boundary),
            (WeakFront, color),
            (RicherElsewhere, color),
            (
                PhotoLike {
                    reason: crate::cover_auto_crop::photo::PhotoLikeReason::ShotExif,
                },
                "写真のように見えるため自動では切りません",
            ),
            (
                PhotoLike {
                    reason: crate::cover_auto_crop::photo::PhotoLikeReason::TransposedPage,
                },
                "写真のように見えるため自動では切りません",
            ),
        ] {
            assert_eq!(decline_text(reason), text);
        }
        for v5 in [
            TwoPanelTrigger::NoJacketEvidence,
            TwoPanelTrigger::NoLocalEvidence,
            TwoPanelTrigger::NoSpine,
            TwoPanelTrigger::WeakFront,
        ] {
            for (reason, text) in [
                (TwoPanelDecline::Shape, boundary),
                (TwoPanelDecline::NoRange, boundary),
                (TwoPanelDecline::WeakBoundary, boundary),
                (TwoPanelDecline::OuterWide, boundary),
                (TwoPanelDecline::NotPaper, boundary),
                (TwoPanelDecline::WeakFront, color),
                (TwoPanelDecline::RicherElsewhere, color),
            ] {
                assert_eq!(decline_text(TwoPanel { v5, reason }), text);
            }
        }
        for (stop, text) in [
            (AutoStop::Off, "自動の切り出しはオフです"),
            (AutoStop::Manual, "手動の範囲を使っています"),
            (
                AutoStop::EditPreview,
                "編集のプレビューがあるため自動は使いません",
            ),
            (AutoStop::Unsupported, "この形式の本は自動の対象外です"),
            (AutoStop::AnalysisFailed, "表紙を解析できませんでした"),
            (
                AutoStop::PageAspect(PageAspectDecline::TooFew),
                "本文ページの形が決まりません (ページが少ない)",
            ),
            (
                AutoStop::PageAspect(PageAspectDecline::Landscape),
                "本文ページの形が決まりません (横長のページが多い)",
            ),
            (
                AutoStop::PageAspect(PageAspectDecline::Inconsistent),
                "本文ページの形が決まりません (大きさがそろわない)",
            ),
        ] {
            assert!(
                AutoDetection::Stopped(stop)
                    .crop_for(Rotation::None)
                    .is_none()
            );
            assert_eq!(
                AutoDetection::Stopped(stop).message(Rotation::Cw90),
                format!("自動の判定: 切り出していません。{text}")
            );
        }
    }
}
