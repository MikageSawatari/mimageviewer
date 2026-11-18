//! Worker-produced automatic results; the UI only selects the saved angle.
use crate::cover_auto_crop::{AutoCropDecline, AutoCropMethod, AutoCropSuccess, TwoPanelDecline};
#[cfg(test)]
use crate::cover_auto_material::AutoCropResults;
use crate::cover_auto_sample::PageAspectDecline;
use crate::folder_thumb_pins::CoverCrop;
use crate::rotation_db::Rotation;
use crate::settings::ReadingDirection;
use crate::thumb_loader::AutoCoverSkip;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AutoStop {
    /// Manual wins in the reason line even when auto is off. Retain the setting
    /// here solely for the existing clear-crop explanatory text.
    Manual {
        automatic_enabled: bool,
    },
    Source(AutoCoverSkip),
    EditPreview,
    PageAspect(PageAspectDecline),
    ReadFailed,
    AnalysisFailed,
}

impl AutoStop {
    pub(super) fn for_source(
        source: &Result<crate::thumb_loader::ResolvedAutoCover, AutoCoverSkip>,
        configured: bool,
    ) -> Option<Self> {
        // A manual crop is effective even with automatic cropping switched off.
        match source {
            Err(AutoCoverSkip::Manual {
                auto_eligible_after_clear,
            }) => Some(Self::Manual {
                automatic_enabled: configured && *auto_eligible_after_clear,
            }),
            _ if !configured => Some(Self::Source(AutoCoverSkip::Off)),
            Err(reason) => Some(Self::Source(*reason)),
            Ok(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AutoDetection {
    Stopped(AutoStop),
    Analyzed {
        binding: ReadingDirection,
        reports: [Result<AutoCropSuccess, AutoCropDecline>; 4],
    },
}

impl AutoDetection {
    pub(super) fn analyze(
        image: &image::DynamicImage,
        dimensions: (u32, u32),
        page_aspect: Option<f64>,
        shot_exif: bool,
        binding: ReadingDirection,
    ) -> Self {
        let mut reports = [Err(AutoCropDecline::NoStructure); 4];
        crate::cover_auto_crop::detect_cover_crops_with_photo_detailed_report(
            image,
            dimensions,
            page_aspect,
            shot_exif,
            binding,
            |angle, result| reports[angle.degrees() as usize / 90] = result,
        );
        Self::Analyzed { binding, reports }
    }

    pub(super) fn enabled(self) -> bool {
        if let Self::Stopped(AutoStop::Manual { automatic_enabled }) = self {
            return automatic_enabled;
        }
        !matches!(self, Self::Stopped(AutoStop::Source(_)))
    }

    pub(super) fn crop_for(self, rotation: Rotation) -> Option<CoverCrop> {
        match self {
            Self::Analyzed { reports, .. } => reports[rotation.degrees() as usize / 90]
                .ok()
                .map(|success| success.crop),
            Self::Stopped(_) => None,
        }
    }

    #[cfg(test)]
    pub(super) fn crops(self) -> Option<AutoCropResults> {
        match self {
            Self::Analyzed { reports, .. } => Some(AutoCropResults::new(
                reports.map(|result| result.ok().map(|success| success.crop)),
            )),
            Self::Stopped(_) => None,
        }
    }

    #[cfg(test)]
    pub(super) fn from_crops(crops: AutoCropResults) -> Self {
        Self::Analyzed {
            binding: ReadingDirection::Rtl,
            reports: crops.crops().map(|crop| {
                crop.map(|crop| AutoCropSuccess::for_test(crop, AutoCropMethod::Current))
                    .ok_or(AutoCropDecline::NoJacketEvidence)
            }),
        }
    }

    pub(super) fn message(self, rotation: Rotation) -> String {
        let reason = match self {
            Self::Stopped(stop) => stop_text(stop),
            Self::Analyzed { binding, reports } => {
                match reports[rotation.degrees() as usize / 90] {
                    Ok(success) => return success_text(success, binding),
                    Err(reason) => decline_text(reason),
                }
            }
        };
        format!("自動の判定: 切り出していません。{reason}")
    }
}

fn success_text(success: AutoCropSuccess, binding: ReadingDirection) -> String {
    let binding = match binding {
        ReadingDirection::Rtl => "右綴じ",
        ReadingDirection::Ltr => "左綴じ",
    };
    let (action, layout) = match success.method {
        AutoCropMethod::Current => {
            return format!("自動の判定: 表表紙を切り出しました (綴じ方向: {binding})");
        }
        AutoCropMethod::LayoutFallback(layout) => ("寸法の型から表表紙を切り出しました", layout),
        AutoCropMethod::LayoutReplaced(layout) => {
            ("寸法の型に合わせて切り出す位置を変更しました", layout)
        }
    };
    let layout = layout.as_str();
    format!("自動の判定: {action} (型: {layout}、綴じ方向: {binding})")
}

fn stop_text(stop: AutoStop) -> &'static str {
    match stop {
        AutoStop::Manual { .. } => "手動の範囲を使っています",
        AutoStop::Source(AutoCoverSkip::Off) => "自動の切り出しはオフです",
        AutoStop::Source(AutoCoverSkip::Manual { .. }) => "手動の範囲を使っています",
        AutoStop::EditPreview => "編集のプレビューがあるため自動は使いません",
        AutoStop::Source(AutoCoverSkip::Unsupported) => "この形式の本は自動の対象外です",
        AutoStop::Source(AutoCoverSkip::PinnedFolder) => {
            "代表のページを固定しているため、自動の判定は使いません"
        }
        AutoStop::Source(AutoCoverSkip::ChildRepresentative) => {
            "代表が子フォルダや中の本の画像なので、自動の判定は使いません"
        }
        AutoStop::Source(AutoCoverSkip::FolderDisabled) => {
            "画像フォルダの本は自動の対象にしていません (環境設定で変えられます)"
        }
        AutoStop::Source(AutoCoverSkip::NestedZip) => "ZIP の中の ZIP は自動の対象外です",
        AutoStop::ReadFailed => "本文のページを読めませんでした",
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
                    Ok(AutoCropSuccess::for_test(crop, AutoCropMethod::Current)),
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
            (
                AutoStop::Source(AutoCoverSkip::Off),
                "自動の切り出しはオフです",
            ),
            (
                AutoStop::Manual {
                    automatic_enabled: true,
                },
                "手動の範囲を使っています",
            ),
            (
                AutoStop::EditPreview,
                "編集のプレビューがあるため自動は使いません",
            ),
            (
                AutoStop::Source(AutoCoverSkip::Unsupported),
                "この形式の本は自動の対象外です",
            ),
            (
                AutoStop::Source(AutoCoverSkip::PinnedFolder),
                "代表のページを固定しているため、自動の判定は使いません",
            ),
            (
                AutoStop::Source(AutoCoverSkip::ChildRepresentative),
                "代表が子フォルダや中の本の画像なので、自動の判定は使いません",
            ),
            (
                AutoStop::Source(AutoCoverSkip::FolderDisabled),
                "画像フォルダの本は自動の対象にしていません (環境設定で変えられます)",
            ),
            (
                AutoStop::Source(AutoCoverSkip::NestedZip),
                "ZIP の中の ZIP は自動の対象外です",
            ),
            (AutoStop::AnalysisFailed, "表紙を解析できませんでした"),
            (AutoStop::ReadFailed, "本文のページを読めませんでした"),
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
