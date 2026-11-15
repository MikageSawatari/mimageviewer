//! Production v3 outcomes, with optional additional evaluation experiments.
use super::*;

mod layout;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FallbackMode {
    Edge,
    Edge18,
    Density,
    Density18,
    Layout,
    LayoutSnap,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct FallbackConfig {
    mode: Option<FallbackMode>,
    richer: bool,
}

impl FallbackConfig {
    pub(super) fn parse(mode: Option<&str>, richer: bool) -> Result<Self, &'static str> {
        let mode = match mode {
            None => None,
            Some("edge") => Some(FallbackMode::Edge),
            Some("edge18") => Some(FallbackMode::Edge18),
            Some("density") => Some(FallbackMode::Density),
            Some("density18") => Some(FallbackMode::Density18),
            Some("layout") => Some(FallbackMode::Layout),
            Some("layout-snap") => Some(FallbackMode::LayoutSnap),
            Some(_) => {
                return Err(
                    "MIV_COVER_AUTO_EVAL_FALLBACK must be edge, edge18, density, density18, layout or layout-snap",
                );
            }
        };
        Ok(Self { mode, richer })
    }

    pub(super) fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        let mode = std::env::var("MIV_COVER_AUTO_EVAL_FALLBACK")
            .map(Some)
            .or_else(|error| match error {
                std::env::VarError::NotPresent => Ok(None),
                error => Err(error),
            })?;
        Ok(Self::parse(
            mode.as_deref(),
            std::env::var("MIV_COVER_AUTO_EVAL_FALLBACK_RICHER").as_deref() == Ok("1"),
        )?)
    }

    pub(super) fn name(self) -> Option<&'static str> {
        self.mode.map(|mode| match mode {
            FallbackMode::Edge => "edge",
            FallbackMode::Edge18 => "edge18",
            FallbackMode::Density => "density",
            FallbackMode::Density18 => "density18",
            FallbackMode::Layout => "layout",
            FallbackMode::LayoutSnap => "layout-snap",
        })
    }

    pub(super) fn includes_richer(self) -> bool {
        self.mode.is_some() && self.richer
    }

    fn permits(self, reason: AutoCropDecline) -> bool {
        self.mode.is_some()
            && match reason {
                AutoCropDecline::NoJacketEvidence
                | AutoCropDecline::NoLocalEvidence
                | AutoCropDecline::NoSpine
                | AutoCropDecline::WeakFront
                | AutoCropDecline::NoStructure
                | AutoCropDecline::TwoPanel { .. } => true,
                AutoCropDecline::RicherElsewhere => self.richer,
                AutoCropDecline::RawAspectGate
                | AutoCropDecline::PageAspectMissing
                | AutoCropDecline::PhotoLike { .. }
                | AutoCropDecline::CameraPageAspect
                | AutoCropDecline::NotWide => false,
            }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct FallbackDiagnostic {
    width: u32,
    inward: u32,
    score: Option<f64>,
    layout: Option<layout::LayoutDiagnostic>,
}

impl FallbackDiagnostic {
    pub(super) fn json(self, config: FallbackConfig) -> serde_json::Value {
        let mut value = json!({"mode": config.name(), "width": self.width, "inward": self.inward,
            "inward_fraction": f64::from(self.inward) / f64::from(self.width), "density_score": self.score});
        if let Some(layout) = self.layout {
            value["layout"] = layout.json();
        }
        value
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct AngleOutcome {
    pub(super) original: Result<AutoCropSuccess, AutoCropDecline>,
    pub(super) final_result: Result<CoverCrop, AutoCropDecline>,
    pub(super) fallback: Option<FallbackDiagnostic>,
}

impl AngleOutcome {
    fn new(
        original: Result<CoverCrop, AutoCropDecline>,
        image: &DynamicImage,
        native_dims: (u32, u32),
        aspect: Option<f64>,
        binding: ReadingDirection,
        rotation: Rotation,
        config: FallbackConfig,
    ) -> Self {
        let applied = original
            .err()
            .filter(|&reason| config.permits(reason))
            .and_then(|_| fallback_crop(image, native_dims, aspect, binding, rotation, config));
        Self {
            original: original.map(|crop| AutoCropSuccess::for_test(crop, AutoCropMethod::Current)),
            final_result: applied.map_or(original, |(crop, _)| Ok(crop)),
            fallback: applied.map(|(_, diagnostic)| diagnostic),
        }
    }

    fn from_production(success: AutoCropSuccess) -> Self {
        Self {
            original: Ok(success),
            final_result: Ok(success.crop),
            fallback: None,
        }
    }
}

pub(super) const EVAL_ROTATIONS: [Rotation; 4] = [
    Rotation::None,
    Rotation::Cw90,
    Rotation::Cw180,
    Rotation::Cw270,
];

/// Native saved-angle cover width / unrounded inferred page width. Also valid
/// outside the four layouts, so callers can count guarded/declined inputs.
pub(super) fn layout_ratio(
    native_dims: (u32, u32),
    aspect: Option<f64>,
    rotation: Rotation,
) -> Option<f64> {
    let (width, height) = match rotation {
        Rotation::Cw90 | Rotation::Cw270 => (native_dims.1, native_dims.0),
        _ => native_dims,
    };
    layout::ratio(f64::from(width), aspect? * f64::from(height))
}

pub(super) fn evaluate_angles(
    image: &DynamicImage,
    native_dims: (u32, u32),
    page_aspect: Option<f64>,
    shot_exif: bool,
    binding: ReadingDirection,
    config: FallbackConfig,
) -> [AngleOutcome; 4] {
    let initial = AngleOutcome {
        original: Err(AutoCropDecline::RawAspectGate),
        final_result: Err(AutoCropDecline::RawAspectGate),
        fallback: None,
    };
    let mut outcomes = [initial; 4];
    detect_cover_crops_with_photo_detailed_report(
        image,
        native_dims,
        page_aspect,
        shot_exif,
        binding,
        |rotation, result| {
            let index = EVAL_ROTATIONS
                .iter()
                .position(|&angle| angle == rotation)
                .unwrap();
            outcomes[index] = match result {
                Ok(success) => AngleOutcome::from_production(success),
                Err(reason) => AngleOutcome::new(
                    Err(reason),
                    image,
                    native_dims,
                    page_aspect,
                    binding,
                    rotation,
                    config,
                ),
            };
        },
    );
    outcomes
}

fn fallback_crop(
    image: &DynamicImage,
    native_dims: (u32, u32),
    aspect: Option<f64>,
    binding: ReadingDirection,
    rotation: Rotation,
    config: FallbackConfig,
) -> Option<(CoverCrop, FallbackDiagnostic)> {
    let mode = config.mode?;
    let aspect = aspect.filter(|value| value.is_finite() && *value > 0.0)?;
    let (width, height) = match rotation {
        Rotation::Cw90 | Rotation::Cw270 => (native_dims.1, native_dims.0),
        _ => native_dims,
    };
    if matches!(mode, FallbackMode::Layout | FallbackMode::LayoutSnap) {
        let (crop, diagnostic) = layout::crop(
            image,
            (width, height),
            aspect,
            binding,
            rotation,
            matches!(mode, FallbackMode::LayoutSnap),
        )?;
        return Some((
            crop,
            FallbackDiagnostic {
                width: (1.02 * aspect * f64::from(height)).round().max(1.0) as u32,
                inward: 0,
                score: None,
                layout: Some(diagnostic),
            },
        ));
    }
    let panel_width = (aspect * f64::from(height)).round();
    if width == 0 || height == 0 || panel_width < 1.0 || panel_width > 0.9 * f64::from(width) {
        return None;
    }
    if matches!(mode, FallbackMode::Edge18 | FallbackMode::Density18)
        && f64::from(width) < 1.8 * panel_width
    {
        return None;
    }
    let panel_width = panel_width as u32;
    let left_at = |inward| match binding {
        ReadingDirection::Rtl => width - panel_width - inward,
        ReadingDirection::Ltr => inward,
    };
    let mut inward = 0;
    let mut score = None;
    if matches!(mode, FallbackMode::Density | FallbackMode::Density18) {
        // Rotate before making the same bounded-height analysis raster used by
        // production. Window geometry remains in native saved-angle coordinates.
        let raster = analysis_raster(&rotated_image(image, rotation));
        if raster.width() == 0 || raster.height() == 0 {
            return None;
        }
        let density = DensityColumns::new(&raster);
        let maximum = ((0.3 * f64::from(panel_width)).floor() as u32).min(width - panel_width);
        let mut best = f64::NEG_INFINITY;
        for candidate in 0..=maximum {
            let left = left_at(candidate);
            let map_x =
                |x| (f64::from(x) * f64::from(raster.width()) / f64::from(width)).round() as usize;
            let value = density.score(map_x(left), map_x(left + panel_width));
            // A tie keeps the edge-most window: no arbitrary inward drift.
            if value > best + 1e-12 {
                best = value;
                inward = candidate;
            }
        }
        score = Some(best);
    }
    let left = left_at(inward);
    let ppm = |x| (f64::from(x) * 1_000_000.0 / f64::from(width)).round() as u32;
    let saved_crop = [ppm(left), 0, ppm(left + panel_width), 1_000_000];
    // The production rotate_rect/inverse path uses edge coordinates, not pixel
    // centers. Return EXIF-upright, before-saved-rotation CoverCrop coordinates.
    let [x0, y0, x1, y1] = saved_crop;
    let unrotated = match rotation {
        Rotation::None => saved_crop,
        Rotation::Cw90 => [y0, 1_000_000 - x1, y1, 1_000_000 - x0],
        Rotation::Cw180 => [
            1_000_000 - x1,
            1_000_000 - y1,
            1_000_000 - x0,
            1_000_000 - y0,
        ],
        Rotation::Cw270 => [1_000_000 - y1, x0, 1_000_000 - y0, x1],
    };
    let crop = CoverCrop::new(unrotated).ok().flatten()?;
    Some((
        crop,
        FallbackDiagnostic {
            width: panel_width,
            inward,
            score,
            layout: None,
        },
    ))
}

// Prefix moments make each candidate O(1). Score = mean absolute RGB adjacent
// gradient / 255 + 4 * mean RGB channel variance / 255^2. Constant white flaps
// contribute neither term; each window has equal full height and native width.
// No learned weights, omitted-strip penalty, or production detector scores.
struct DensityColumns {
    sums: Vec<[f64; 3]>,
    squares: Vec<[f64; 3]>,
    gradients: Vec<f64>,
    height: usize,
}

impl DensityColumns {
    fn new(image: &RgbImage) -> Self {
        let mut this = Self {
            sums: vec![[0.0; 3]],
            squares: vec![[0.0; 3]],
            gradients: vec![0.0],
            height: image.height() as usize,
        };
        for x in 0..image.width() {
            let mut sum = *this.sums.last().unwrap();
            let mut square = *this.squares.last().unwrap();
            let mut gradient = *this.gradients.last().unwrap();
            for y in 0..image.height() {
                let pixel = image.get_pixel(x, y);
                for channel in 0..3 {
                    let value = f64::from(pixel[channel]);
                    sum[channel] += value;
                    square[channel] += value * value;
                    if x > 0 {
                        gradient += (value - f64::from(image.get_pixel(x - 1, y)[channel])).abs();
                    }
                    if y > 0 {
                        gradient += (value - f64::from(image.get_pixel(x, y - 1)[channel])).abs();
                    }
                }
            }
            this.sums.push(sum);
            this.squares.push(square);
            this.gradients.push(gradient);
        }
        this
    }

    fn score(&self, left: usize, right: usize) -> f64 {
        if right <= left {
            return 0.0;
        }
        let count = ((right - left) * self.height) as f64;
        let variance = (0..3)
            .map(|channel| {
                let mean = (self.sums[right][channel] - self.sums[left][channel]) / count;
                ((self.squares[right][channel] - self.squares[left][channel]) / count - mean * mean)
                    .max(0.0)
            })
            .sum::<f64>()
            / 3.0;
        // At most two adjacent differences per channel/pixel.
        let gradient = (self.gradients[right] - self.gradients[left]) / (count * 6.0);
        gradient / 255.0 + 4.0 * variance / (255.0 * 255.0)
    }
}

#[derive(Default)]
pub(super) struct FallbackStats {
    pub(super) cropped: u32,
    pub(super) good: u32,
    pub(super) wrong: u32,
    pub(super) reasons: BTreeMap<String, u32>,
    inward_fractions: Vec<f64>,
    layouts: [LayoutCount; 4],
    layout_ratios: Vec<f64>,
}

#[derive(Clone, Copy, Default)]
struct LayoutCount {
    count: u32,
    good: u32,
    wrong: u32,
}

impl FallbackStats {
    pub(super) fn record(&mut self, outcome: AngleOutcome, iou: Option<f64>, has_truth: bool) {
        if let Some(diagnostic) = outcome.fallback {
            self.cropped += 1;
            if has_truth {
                if iou.is_some_and(|value| value >= 0.85) {
                    self.good += 1;
                } else {
                    self.wrong += 1;
                }
            }
            if let Err(reason) = outcome.original {
                *self
                    .reasons
                    .entry(reason.as_str().into_owned())
                    .or_default() += 1;
            }
            self.inward_fractions
                .push(f64::from(diagnostic.inward) / f64::from(diagnostic.width));
            if let Some(layout) = diagnostic.layout {
                let counts = &mut self.layouts[layout.kind];
                counts.count += 1;
                if has_truth {
                    if iou.is_some_and(|value| value >= 0.85) {
                        counts.good += 1;
                    } else {
                        counts.wrong += 1;
                    }
                }
                self.layout_ratios.push(layout.ratio);
            }
        }
    }

    pub(super) fn distribution(&self) -> serde_json::Value {
        let mut values = self.inward_fractions.clone();
        values.sort_by(f64::total_cmp);
        let quantile = |fraction: f64| {
            values
                .get(((values.len().saturating_sub(1)) as f64 * fraction).round() as usize)
                .copied()
        };
        json!({"count": values.len(), "min": quantile(0.0), "median": quantile(0.5), "p90": quantile(0.9), "max": quantile(1.0)})
    }

    pub(super) fn layout_summary(&self, has_truth: bool) -> serde_json::Value {
        let mut result = serde_json::Map::new();
        for (index, counts) in self.layouts.iter().enumerate() {
            result.insert(format!("T{}", index + 1), json!({"cropped": counts.count,
                "good": has_truth.then_some(counts.good), "wrong": has_truth.then_some(counts.wrong)}));
        }
        result.into()
    }

    pub(super) fn layout_ratio_distribution(&self) -> serde_json::Value {
        // Integer bin indices avoid accumulating decimal boundaries. These
        // counts cover applied layout fallbacks; parent owns all-case counts.
        let mut bins = [0_u32; 20];
        for ratio in &self.layout_ratios {
            bins[(*ratio * 5.0 + 1e-9).floor() as usize] += 1;
        }
        json!({"bin_width": 0.2, "count": self.layout_ratios.len(), "bins": bins.into_iter().enumerate().map(|(index, count)|
            json!({"lower": index as f64 / 5.0, "upper": (index + 1) as f64 / 5.0, "count": count})).collect::<Vec<_>>()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(mode: &str) -> FallbackConfig {
        FallbackConfig::parse(Some(mode), false).unwrap()
    }

    fn outcome(
        image: &DynamicImage,
        binding: ReadingDirection,
        rotation: Rotation,
        config: FallbackConfig,
    ) -> AngleOutcome {
        AngleOutcome::new(
            Err(AutoCropDecline::NoStructure),
            image,
            (image.width(), image.height()),
            Some(2.0 / 3.0),
            binding,
            rotation,
            config,
        )
    }

    #[test]
    fn evaluation_fallback_decline_allowlist_preserves_every_guard() {
        let image = DynamicImage::new_rgb8(240, 100);
        for mode in [
            "edge",
            "edge18",
            "density",
            "density18",
            "layout",
            "layout-snap",
        ] {
            for richer in [false, true] {
                let config = FallbackConfig::parse(Some(mode), richer).unwrap();
                let guards = [
                    AutoCropDecline::RawAspectGate,
                    AutoCropDecline::PageAspectMissing,
                    AutoCropDecline::NotWide,
                    AutoCropDecline::CameraPageAspect,
                    AutoCropDecline::PhotoLike {
                        reason: photo::PhotoLikeReason::ShotExif,
                    },
                ];
                for reason in guards {
                    let result = AngleOutcome::new(
                        Err(reason),
                        &image,
                        (240, 100),
                        Some(2.0 / 3.0),
                        ReadingDirection::Rtl,
                        Rotation::None,
                        config,
                    );
                    assert_eq!(result.final_result, Err(reason));
                    assert!(result.fallback.is_none());
                }
                for reason in [
                    AutoCropDecline::NoJacketEvidence,
                    AutoCropDecline::NoLocalEvidence,
                    AutoCropDecline::NoSpine,
                    AutoCropDecline::WeakFront,
                    AutoCropDecline::NoStructure,
                ] {
                    assert!(config.permits(reason));
                }
                for v5 in [
                    TwoPanelTrigger::NoJacketEvidence,
                    TwoPanelTrigger::NoLocalEvidence,
                    TwoPanelTrigger::NoSpine,
                    TwoPanelTrigger::WeakFront,
                ] {
                    for reason in [
                        TwoPanelDecline::Shape,
                        TwoPanelDecline::NoRange,
                        TwoPanelDecline::WeakBoundary,
                        TwoPanelDecline::OuterWide,
                        TwoPanelDecline::NotPaper,
                        TwoPanelDecline::WeakFront,
                        TwoPanelDecline::RicherElsewhere,
                    ] {
                        assert!(config.permits(AutoCropDecline::TwoPanel { v5, reason }));
                    }
                }
                assert_eq!(config.permits(AutoCropDecline::RicherElsewhere), richer);
                let richer_result = AngleOutcome::new(
                    Err(AutoCropDecline::RicherElsewhere),
                    &image,
                    (240, 100),
                    Some(2.0 / 3.0),
                    ReadingDirection::Rtl,
                    Rotation::None,
                    config,
                );
                assert_eq!(richer_result.fallback.is_some(), richer);
            }
        }
        assert!(FallbackConfig::parse(Some("invalid"), false).is_err());
    }

    #[test]
    fn evaluation_fallback_edge_binding_width_rounding_and_18_constraint() {
        let image = DynamicImage::new_rgb8(200, 100);
        let rtl = outcome(
            &image,
            ReadingDirection::Rtl,
            Rotation::None,
            config("edge"),
        );
        let ltr = outcome(
            &image,
            ReadingDirection::Ltr,
            Rotation::None,
            config("edge"),
        );
        assert_eq!(
            rtl.final_result.unwrap().values(),
            [665_000, 0, 1_000_000, 1_000_000]
        );
        assert_eq!(
            ltr.final_result.unwrap().values(),
            [0, 0, 335_000, 1_000_000]
        );
        assert_eq!(rtl.fallback.unwrap().width, 67);
        for mode in ["edge18", "density18"] {
            for (width, expected) in [(119, false), (120, false), (121, true)] {
                let image = DynamicImage::new_rgb8(width, 100);
                assert_eq!(
                    outcome(&image, ReadingDirection::Rtl, Rotation::None, config(mode))
                        .fallback
                        .is_some(),
                    expected
                );
            }
        }
        for mode in ["edge", "density"] {
            let image = DynamicImage::new_rgb8(100, 100);
            for (aspect, expected) in [(0.9, true), (0.91, false), (0.0, false), (f64::NAN, false)]
            {
                assert_eq!(
                    fallback_crop(
                        &image,
                        (100, 100),
                        Some(aspect),
                        ReadingDirection::Rtl,
                        Rotation::None,
                        config(mode)
                    )
                    .is_some(),
                    expected
                );
            }
            assert!(
                fallback_crop(
                    &image,
                    (100, 100),
                    None,
                    ReadingDirection::Rtl,
                    Rotation::None,
                    config(mode)
                )
                .is_none()
            );
        }
    }

    #[test]
    fn evaluation_fallback_density_moves_inward_past_white_flap_for_both_bindings() {
        let mut image = RgbImage::from_pixel(480, 256, Rgb([255, 255, 255]));
        for x in 272..432 {
            for y in 0..256 {
                let value = if (x / 4 + y / 4) % 2 == 0 { 0 } else { 255 };
                image.put_pixel(x, y, Rgb([value, value, value]));
            }
        }
        for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
            let image = DynamicImage::ImageRgb8(image.clone());
            let image = if binding == ReadingDirection::Ltr {
                image.fliph()
            } else {
                image
            };
            let apply = |mode| {
                fallback_crop(
                    &image,
                    (480, 256),
                    Some(0.625),
                    binding,
                    Rotation::None,
                    config(mode),
                )
                .unwrap()
            };
            let (edge, edge_diagnostic) = apply("edge");
            assert_eq!(edge_diagnostic.inward, 0);
            for mode in ["density", "density18"] {
                let (crop, diagnostic) = apply(mode);
                assert!(
                    diagnostic.inward >= 44 && diagnostic.inward <= 48,
                    "{binding:?}: {diagnostic:?}"
                );
                let expected = match binding {
                    ReadingDirection::Rtl => [272.0, 0.0, 432.0, 256.0],
                    ReadingDirection::Ltr => [48.0, 0.0, 208.0, 256.0],
                };
                let rect = |crop: CoverCrop| {
                    let [a, b, c, d] = crop.values();
                    [
                        f64::from(a) * 480.0 / 1e6,
                        f64::from(b) * 256.0 / 1e6,
                        f64::from(c) * 480.0 / 1e6,
                        f64::from(d) * 256.0 / 1e6,
                    ]
                };
                assert!(box_iou(rect(crop), expected) > 0.95);
                assert!(box_iou(rect(crop), expected) > box_iou(rect(edge), expected));
            }
        }
    }

    #[test]
    fn evaluation_fallback_physical_and_saved_rotations_keep_source_edge_coordinates() {
        let image = DynamicImage::new_rgb8(240, 100);
        for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
            let expected = outcome(&image, binding, Rotation::None, config("edge"))
                .final_result
                .unwrap()
                .values()
                .map(f64::from);
            for physical in EVAL_ROTATIONS {
                let input = rotated_image(&image, physical);
                let saved = Rotation::from_degrees(-physical.degrees());
                let result = outcome(&input, binding, saved, config("edge"));
                let actual = result.final_result.unwrap().values().map(f64::from);
                assert_eq!(
                    actual,
                    rotated_rect(expected, (1_000_000, 1_000_000), physical)
                );
            }
            // All four saved angles, independent of the physical input shape.
            for saved in EVAL_ROTATIONS {
                let input = rotated_image(&image, Rotation::from_degrees(-saved.degrees()));
                let result = outcome(&input, binding, saved, config("density"));
                assert_eq!(result.fallback.unwrap().inward, 0);
                assert_eq!(
                    result.final_result.unwrap().values().map(f64::from),
                    rotated_rect(
                        expected,
                        (1_000_000, 1_000_000),
                        Rotation::from_degrees(-saved.degrees())
                    )
                );
            }
            for physical in EVAL_ROTATIONS {
                let input = rotated_image(&image, physical);
                for saved in EVAL_ROTATIONS {
                    let effective = Rotation::from_degrees(physical.degrees() + saved.degrees());
                    for mode in ["edge", "density"] {
                        let base = fallback_crop(
                            &image,
                            (240, 100),
                            Some(2.0 / 3.0),
                            binding,
                            effective,
                            config(mode),
                        );
                        let actual = fallback_crop(
                            &input,
                            (input.width(), input.height()),
                            Some(2.0 / 3.0),
                            binding,
                            saved,
                            config(mode),
                        );
                        assert_eq!(actual.is_some(), base.is_some());
                        if let (Some((base, _)), Some((actual, _))) = (base, actual) {
                            assert_eq!(
                                actual.values().map(f64::from),
                                rotated_rect(
                                    base.values().map(f64::from),
                                    (1_000_000, 1_000_000),
                                    physical
                                )
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn evaluation_fallback_absent_matches_production_at_all_angles_and_preserves_success() {
        let image = DynamicImage::new_rgb8(240, 100);
        let page_aspect = Some(2.0 / 3.0);
        let outcomes = evaluate_angles(
            &image,
            (240, 100),
            page_aspect,
            false,
            ReadingDirection::Rtl,
            FallbackConfig::parse(None, true).unwrap(),
        );
        let mut original = Vec::new();
        detect_cover_crops_with_photo_report(
            &image,
            (240, 100),
            page_aspect,
            false,
            ReadingDirection::Rtl,
            |_, result| original.push(result),
        );
        for (outcome, result) in outcomes.into_iter().zip(original) {
            assert_eq!(outcome.original.map(|success| success.crop), result);
            assert_eq!(outcome.final_result, result);
            assert!(outcome.fallback.is_none());
        }
        let crop = CoverCrop::new([500_000, 0, 1_000_000, 1_000_000])
            .unwrap()
            .unwrap();
        let result = AngleOutcome::new(
            Ok(crop),
            &image,
            (240, 100),
            Some(2.0 / 3.0),
            ReadingDirection::Rtl,
            Rotation::None,
            config("edge"),
        );
        assert_eq!(result.final_result, Ok(crop));
        assert!(result.fallback.is_none());
    }

    #[test]
    fn evaluation_fallback_summary_counts_selected_angle_and_negative_truth() {
        let image = DynamicImage::new_rgb8(240, 100);
        let selected = outcome(
            &image,
            ReadingDirection::Rtl,
            Rotation::None,
            config("edge"),
        );
        let mut stats = FallbackStats::default();
        stats.record(selected, Some(0.9), true);
        stats.record(selected, Some(0.8), true);
        stats.record(selected, None, true); // negative truth: any crop is wrong
        stats.record(
            outcome(
                &image,
                ReadingDirection::Rtl,
                Rotation::None,
                FallbackConfig::default(),
            ),
            None,
            true,
        );
        assert_eq!((stats.cropped, stats.good, stats.wrong), (3, 1, 2));
        assert_eq!(stats.reasons["no-structure"], 3);
        assert_eq!(stats.distribution()["count"], 3);
        assert_eq!(stats.distribution()["max"], 0.0);
    }

    #[test]
    fn evaluation_layout_summary_keeps_all_types_truth_semantics_and_ratio_bins() {
        let mut stats = FallbackStats::default();
        for (index, width) in [150, 200, 270, 350].into_iter().enumerate() {
            let image = DynamicImage::new_rgb8(width, 150);
            let result = AngleOutcome::new(
                Err(AutoCropDecline::NoStructure),
                &image,
                (width, 150),
                Some(2.0 / 3.0),
                ReadingDirection::Rtl,
                Rotation::None,
                config("layout"),
            );
            let iou = [Some(0.9), Some(0.8), None, Some(0.9)][index];
            stats.record(result, iou, true);
            let diagnostic = result.fallback.unwrap().json(config("layout"));
            assert_eq!(diagnostic["layout"]["kind"], format!("T{}", index + 1));
            assert_eq!(diagnostic["layout"]["snap_applied"], false);
        }
        let summary = stats.layout_summary(true);
        let no_truth = stats.layout_summary(false);
        for index in 0..4 {
            let key = format!("T{}", index + 1);
            assert_eq!(summary[&key]["cropped"], 1);
            assert_eq!(summary[&key]["good"], u32::from(index == 0 || index == 3));
            assert_eq!(summary[&key]["wrong"], u32::from(index == 1 || index == 2));
            assert_eq!(no_truth[&key]["cropped"], 1);
            assert!(no_truth[&key]["good"].is_null());
            assert!(no_truth[&key]["wrong"].is_null());
        }
        assert_eq!((stats.cropped, stats.good, stats.wrong), (4, 2, 2));
        let histogram = stats.layout_ratio_distribution();
        assert_eq!(histogram["count"], 4);
        assert_eq!(
            histogram["bins"]
                .as_array()
                .unwrap()
                .iter()
                .map(|bin| bin["count"].as_u64().unwrap())
                .sum::<u64>(),
            4
        );
        for index in [7, 10, 13, 17] {
            assert_eq!(histogram["bins"][index]["count"], 1);
        }
        let empty = FallbackStats::default().layout_summary(true);
        assert_eq!(empty.as_object().unwrap().len(), 4);
        assert_eq!(empty["T4"]["cropped"], 0);
    }

    #[test]
    fn evaluation_layout_ratio_uses_native_saved_angle_and_accepts_outside_layout() {
        assert_eq!(
            layout_ratio((120, 80), Some(0.5), Rotation::None),
            Some(3.0)
        );
        assert_eq!(
            layout_ratio((120, 80), Some(0.5), Rotation::Cw180),
            Some(3.0)
        );
        for rotation in [Rotation::Cw90, Rotation::Cw270] {
            assert_eq!(
                layout_ratio((120, 80), Some(0.5), rotation),
                Some(4.0 / 3.0)
            );
        }
        assert_eq!(
            layout_ratio((50, 100), Some(1.0), Rotation::None),
            Some(0.5)
        );
        assert_eq!(
            layout_ratio((186, 150), Some(0.67), Rotation::None),
            Some(186.0 / 100.5)
        );
        for aspect in [
            None,
            Some(0.0),
            Some(-1.0),
            Some(f64::NAN),
            Some(f64::INFINITY),
        ] {
            assert!(layout_ratio((120, 80), aspect, Rotation::None).is_none());
        }
        assert!(layout_ratio((0, 80), Some(0.5), Rotation::None).is_none());
    }

    #[test]
    fn evaluation_layout_extension_preserves_legacy_crop_and_diagnostic_goldens() {
        let image = DynamicImage::new_rgb8(240, 100);
        for mode in ["edge", "edge18", "density", "density18"] {
            for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
                let result = outcome(&image, binding, Rotation::None, config(mode));
                let expected = match binding {
                    ReadingDirection::Rtl => [720_833, 0, 1_000_000, 1_000_000],
                    ReadingDirection::Ltr => [0, 0, 279_167, 1_000_000],
                };
                assert_eq!(result.final_result.unwrap().values(), expected);
                let density_score = mode.starts_with("density").then_some(0.0);
                assert_eq!(
                    result.fallback.unwrap().json(config(mode)),
                    json!({
                        "mode": mode, "width": 67, "inward": 0, "inward_fraction": 0.0, "density_score": density_score
                    })
                );
            }
        }
    }
}
