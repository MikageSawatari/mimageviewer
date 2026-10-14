//! Parameter changes authored by one UI pass, applied to the latest accepted layer.
//! The before/after pair describes user intent; it is never a replacement page document.
//! Untouched parameters and shared raster buffers remain owned by the latest layer.

use local_adjust_core::{
    LocalAdjustmentLayer, LocalEffect, LocalMask, MaskAlpha, RasterVectorMask, SubjectMask,
};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct LocalLayerParameterEdit {
    pub before: LocalAdjustmentLayer,
    pub after: LocalAdjustmentLayer,
}

macro_rules! changed_fields {
    ($before:expr, $after:expr, $latest:expr; $([$($field:tt)+]),* $(,)?) => {
        $(if $before.$($field)+ != $after.$($field)+ {
            $latest.$($field)+ = $after.$($field)+.clone();
        })*
    };
}

impl LocalLayerParameterEdit {
    pub(crate) fn apply(self, latest: &mut LocalAdjustmentLayer) {
        let before = &self.before;
        let after = &self.after;
        changed_fields!(before, after, latest;
            [name], [enabled], [opacity], [mask_inverted], [mask_expand_px],
            [mask_feather_px], [mask_before_effect], [mask_after_effect]
        );
        apply_mask(&before.mask, &after.mask, &mut latest.mask);
        apply_manual_mask(
            &before.manual_override.add,
            &after.manual_override.add,
            &mut latest.manual_override.add,
        );
        apply_manual_mask(
            &before.manual_override.subtract,
            &after.manual_override.subtract,
            &mut latest.manual_override.subtract,
        );
        apply_effect(&before.effect, &after.effect, &mut latest.effect);
    }
}

fn alpha_changed(before: &MaskAlpha, after: &MaskAlpha) -> bool {
    // A normal parameter edit shares these large buffers. Do not scan their pixels.
    !Arc::ptr_eq(before, after) && before != after
}

fn apply_alpha(before: &MaskAlpha, after: &MaskAlpha, latest: &mut MaskAlpha) {
    if alpha_changed(before, after) {
        *latest = after.clone();
    }
}

fn source_alpha_changed(before: &Option<MaskAlpha>, after: &Option<MaskAlpha>) -> bool {
    match (before, after) {
        (Some(before), Some(after)) => alpha_changed(before, after),
        (None, None) => false,
        _ => true,
    }
}

fn apply_source_alpha(
    before: &Option<MaskAlpha>,
    after: &Option<MaskAlpha>,
    latest: &mut Option<MaskAlpha>,
) {
    match (before, after, latest) {
        (Some(before), Some(after), Some(latest)) => apply_alpha(before, after, latest),
        (None, None, _) => {}
        (Some(before), Some(after), latest) => {
            if alpha_changed(before, after) {
                *latest = Some(after.clone());
            }
        }
        (_, after, latest) => *latest = after.clone(),
    }
}

fn apply_raster_vector(
    before: &RasterVectorMask,
    after: &RasterVectorMask,
    latest: &mut RasterVectorMask,
) {
    changed_fields!(before, after, latest; [width], [height], [shapes]);
    apply_alpha(&before.alpha, &after.alpha, &mut latest.alpha);
}

fn raster_vector_changed(before: &RasterVectorMask, after: &RasterVectorMask) -> bool {
    before.width != after.width
        || before.height != after.height
        || before.shapes != after.shapes
        || alpha_changed(&before.alpha, &after.alpha)
}

fn apply_manual_mask(
    before: &Option<RasterVectorMask>,
    after: &Option<RasterVectorMask>,
    latest: &mut Option<RasterVectorMask>,
) {
    match (before, after, latest) {
        (Some(before), Some(after), Some(latest)) => apply_raster_vector(before, after, latest),
        (None, None, _) => {}
        (Some(before), Some(after), latest) => {
            if raster_vector_changed(before, after) {
                *latest = Some(after.clone());
            }
        }
        (_, after, latest) => *latest = after.clone(),
    }
}

fn apply_subject(before: &SubjectMask, after: &SubjectMask, latest: &mut SubjectMask) {
    let refinement_changed = before.refinement != after.refinement;
    // Older UI passes initialized the legacy source cache while refining. That
    // captures their original matte, rather than selecting a new source matte.
    let initialized_old_source = refinement_changed
        && before.source_alpha.is_none()
        && after
            .source_alpha
            .as_ref()
            .is_some_and(|source| !alpha_changed(&before.alpha, source));
    let source_changed =
        !initialized_old_source && source_alpha_changed(&before.source_alpha, &after.source_alpha);
    changed_fields!(before, after, latest;
        [width], [height], [refinement.enabled], [refinement.threshold],
        [refinement.expand_px], [refinement.feather_px]
    );
    if source_changed {
        apply_source_alpha(
            &before.source_alpha,
            &after.source_alpha,
            &mut latest.source_alpha,
        );
    }
    if refinement_changed || source_changed {
        // after.alpha was derived from the UI's old source and settings. The
        // authored operation is refinement, applied to the latest source matte.
        if latest.refinement.enabled {
            let refinement = latest.refinement;
            crate::ui_adjustment_panel::apply_local_adjust_subject_refinement(latest, refinement);
        } else {
            latest.alpha = latest.source_raster_mask().alpha;
        }
    } else {
        apply_alpha(&before.alpha, &after.alpha, &mut latest.alpha);
    }
}

fn apply_mask(before_mask: &LocalMask, after_mask: &LocalMask, latest_mask: &mut LocalMask) {
    // Exhaustive on the original enum: adding a mask kind requires an intent rule.
    match before_mask {
        LocalMask::Full => {
            if !matches!(after_mask, LocalMask::Full) {
                *latest_mask = after_mask.clone();
            }
        }
        LocalMask::Raster(before) => match (after_mask, latest_mask) {
            (LocalMask::Raster(after), LocalMask::Raster(latest)) => {
                changed_fields!(before, after, latest; [width], [height]);
                apply_alpha(&before.alpha, &after.alpha, &mut latest.alpha);
            }
            (LocalMask::Raster(after), latest) => {
                if before.width != after.width
                    || before.height != after.height
                    || alpha_changed(&before.alpha, &after.alpha)
                {
                    *latest = LocalMask::Raster(after.clone());
                }
            }
            (after, latest) => *latest = after.clone(),
        },
        LocalMask::RasterVector(before) => match (after_mask, latest_mask) {
            (LocalMask::RasterVector(after), LocalMask::RasterVector(latest)) => {
                changed_fields!(before, after, latest; [width], [height], [shapes]);
                apply_alpha(&before.alpha, &after.alpha, &mut latest.alpha);
            }
            (LocalMask::RasterVector(after), latest) => {
                if raster_vector_changed(before, after) {
                    *latest = LocalMask::RasterVector(after.clone());
                }
            }
            (after, latest) => *latest = after.clone(),
        },
        LocalMask::LinearGradient(before) => match (after_mask, latest_mask) {
            (LocalMask::LinearGradient(after), LocalMask::LinearGradient(latest)) => {
                changed_fields!(before, after, latest; [initialized], [start[0]], [start[1]], [end[0]], [end[1]]);
            }
            (LocalMask::LinearGradient(after), latest) => {
                if before != after {
                    *latest = LocalMask::LinearGradient(after.clone());
                }
            }
            (after, latest) => *latest = after.clone(),
        },
        LocalMask::RadialGradient(before) => match (after_mask, latest_mask) {
            (LocalMask::RadialGradient(after), LocalMask::RadialGradient(latest)) => {
                changed_fields!(before, after, latest; [initialized], [center[0]], [center[1]], [inner_radius], [inner_radius_y], [outer_radius], [outer_radius_y]);
            }
            (LocalMask::RadialGradient(after), latest) => {
                if before != after {
                    *latest = LocalMask::RadialGradient(after.clone());
                }
            }
            (after, latest) => *latest = after.clone(),
        },
        LocalMask::LumaRange(before) => match (after_mask, latest_mask) {
            (LocalMask::LumaRange(after), LocalMask::LumaRange(latest)) => {
                changed_fields!(before, after, latest; [min], [max], [feather]);
            }
            (LocalMask::LumaRange(after), latest) => {
                if before != after {
                    *latest = LocalMask::LumaRange(after.clone());
                }
            }
            (after, latest) => *latest = after.clone(),
        },
        LocalMask::ColorRange(before) => match (after_mask, latest_mask) {
            (LocalMask::ColorRange(after), LocalMask::ColorRange(latest)) => {
                changed_fields!(before, after, latest; [initialized], [target_rgb[0]], [target_rgb[1]], [target_rgb[2]], [tolerance], [feather]);
            }
            (LocalMask::ColorRange(after), latest) => {
                if before != after {
                    *latest = LocalMask::ColorRange(after.clone());
                }
            }
            (after, latest) => *latest = after.clone(),
        },
        LocalMask::Subject(before) => match (after_mask, latest_mask) {
            (LocalMask::Subject(after), LocalMask::Subject(latest)) => {
                apply_subject(before, after, latest);
            }
            (LocalMask::Subject(after), latest) => {
                if before.width != after.width
                    || before.height != after.height
                    || before.refinement != after.refinement
                    || alpha_changed(&before.alpha, &after.alpha)
                    || source_alpha_changed(&before.source_alpha, &after.source_alpha)
                {
                    *latest = LocalMask::Subject(after.clone());
                }
            }
            (after, latest) => *latest = after.clone(),
        },
        LocalMask::Segmentation(before) => match (after_mask, latest_mask) {
            (LocalMask::Segmentation(after), LocalMask::Segmentation(latest)) => {
                changed_fields!(before, after, latest; [width], [height], [selected]);
                if !Arc::ptr_eq(&before.labels, &after.labels) && before.labels != after.labels {
                    latest.labels = after.labels.clone();
                }
            }
            (LocalMask::Segmentation(after), latest) => {
                if before.width != after.width
                    || before.height != after.height
                    || before.selected != after.selected
                    || (!Arc::ptr_eq(&before.labels, &after.labels)
                        && before.labels != after.labels)
                {
                    *latest = LocalMask::Segmentation(after.clone());
                }
            }
            (after, latest) => *latest = after.clone(),
        },
    }
}

macro_rules! effect_parameter_intents {
    ($($variant:ident { $([$($field:tt)+]),* $(,)? }),* $(,)?) => {
        fn apply_effect(before_effect: &LocalEffect, after_effect: &LocalEffect, latest_effect: &mut LocalEffect) {
            // Exhaustive variant inventory; each typed path is a UI parameter.
            match before_effect {
                LocalEffect::None => {
                    if !matches!(after_effect, LocalEffect::None) { *latest_effect = after_effect.clone(); }
                },
                $(LocalEffect::$variant(before) => match (after_effect, latest_effect) {
                    (LocalEffect::$variant(after), LocalEffect::$variant(latest)) => {
                        changed_fields!(before, after, latest; $([$($field)+]),*);
                    },
                    (LocalEffect::$variant(after), latest) => {
                        if before != after { *latest = LocalEffect::$variant(after.clone()); }
                    },
                    (after, latest) => *latest = after.clone(),
                }),*
            }
        }
    };
}

effect_parameter_intents! {
    Tone { [brightness], [contrast], [gamma], [saturation], [vibrance], [temperature], [tint] },
    ToneCurve { [points[0]], [points[1]], [points[2]], [points[3]], [points[4]] },
    RgbToneCurve { [master[0]], [master[1]], [master[2]], [master[3]], [master[4]], [red[0]], [red[1]], [red[2]], [red[3]], [red[4]], [green[0]], [green[1]], [green[2]], [green[3]], [green[4]], [blue[0]], [blue[1]], [blue[2]], [blue[3]], [blue[4]] },
    ColorBalance { [shadows.cyan_red], [shadows.magenta_green], [shadows.yellow_blue], [midtones.cyan_red], [midtones.magenta_green], [midtones.yellow_blue], [highlights.cyan_red], [highlights.magenta_green], [highlights.yellow_blue], [preserve_luma] },
    PhotoFilter { [preset], [color_rgb[0]], [color_rgb[1]], [color_rgb[2]], [density], [preserve_luminosity], [strength] },
    ThreeWayColorGrading { [shadows.hue_degrees], [shadows.saturation], [shadows.luminance], [midtones.hue_degrees], [midtones.saturation], [midtones.luminance], [highlights.hue_degrees], [highlights.saturation], [highlights.luminance], [balance] },
    SelectiveColor { [target_hue_degrees], [range_degrees], [feather_degrees], [hue_degrees], [saturation], [lightness] },
    PartColor { [target_rgb[0]], [target_rgb[1]], [target_rgb[2]], [range_degrees], [feather_degrees], [gray_strength], [selected_saturation], [selected_lightness] },
    ChannelMixer { [monochrome], [red_output[0]], [red_output[1]], [red_output[2]], [green_output[0]], [green_output[1]], [green_output[2]], [blue_output[0]], [blue_output[1]], [blue_output[2]], [mono_output[0]], [mono_output[1]], [mono_output[2]] },
    MonochromeMixer { [red], [yellow], [green], [cyan], [blue], [magenta], [contrast], [tint_rgb[0]], [tint_rgb[1]], [tint_rgb[2]], [tint_strength], [strength] },
    Clarity { [amount], [radius_px] },
    Texture { [amount], [radius_px] },
    HighPass { [amount], [radius_px], [contrast], [detail_only] },
    FrequencySeparation { [radius_px], [low_smoothing], [detail_amount], [detail_contrast], [strength] },
    HighlightsShadows { [shadows], [highlights] },
    Dehaze { [amount], [radius_px], [min_transmission], [saturation] },
    Blur { [radius_px] },
    MotionBlur { [distance_px], [angle_degrees], [strength] },
    Wind { [direction], [source], [distance_px], [threshold], [softness], [turbulence], [strength], [seed] },
    TiltShift { [mode], [mode_selected], [range_initialized], [center[0]], [center[1]], [angle_degrees], [focus_width], [falloff], [radius[0]], [radius[1]], [max_radius_px], [strength], [far_only] },
    LensBlur { [radius_px], [aperture], [rotation_degrees], [highlight_threshold], [highlight_boost], [strength] },
    BokehSprite { [shape], [threshold], [density], [size_px], [softness], [brightness], [color_strength], [seed], [strength] },
    LensDirt { [mode], [density], [size_px], [opacity], [softness], [highlight_response], [distortion_px], [seed], [strength] },
    RadialBlur { [mode], [center[0]], [center[1]], [zoom_px], [spin_degrees], [samples], [strength] },
    WaveDistortion { [mode], [amplitude_px], [wavelength_px], [phase_degrees], [center[0]], [center[1]], [strength] },
    HeatHaze { [amplitude_px], [wavelength_px], [rise_px], [turbulence], [blur_px], [phase_degrees], [strength] },
    PinchSpherize { [amount], [radius_px], [center[0]], [center[1]], [strength] },
    Twirl { [angle_degrees], [radius_px], [center[0]], [center[1]], [strength] },
    PolarCoordinates { [mode], [center[0]], [center[1]], [radius_px], [angle_offset_degrees], [invert_radius], [strength] },
    GlassDisplacement { [mode], [displacement_px], [scale_px], [detail], [angle_degrees], [seed], [strength] },
    LensCorrection { [distortion], [zoom], [center[0]], [center[1]], [vignette_correction], [strength] },
    LineExtract { [mode], [threshold], [softness], [thickness_px], [strength] },
    ArtisticMedia { [mode], [radius_px], [edge_strength], [texture], [color_amount], [strength], [seed] },
    BrushStroke { [mode], [length_px], [radius_px], [angle_degrees], [texture], [edge_strength], [color_amount], [strength], [seed] },
    Cutout { [levels], [radius_px], [edge_strength], [color_amount], [strength] },
    ToonShade { [bands], [softness], [preserve_hue], [shadow_tint_rgb[0]], [shadow_tint_rgb[1]], [shadow_tint_rgb[2]], [shadow_tint_strength], [light_tint_rgb[0]], [light_tint_rgb[1]], [light_tint_rgb[2]], [light_tint_strength], [outline_strength], [strength] },
    Emboss { [angle_degrees], [depth], [contrast], [color_amount], [strength] },
    PixelStylize { [mode], [cell_px], [edge_strength], [color_amount], [randomness], [strength], [seed] },
    Solarize { [threshold], [softness], [inversion], [contrast], [color_amount], [strength] },
    GlowingEdges { [threshold], [softness], [edge_width_px], [glow_radius_px], [edge_brightness], [glow_strength], [hue_degrees], [color_amount], [background_amount], [strength] },
    OilPaint { [radius_px], [saturation], [contrast], [strength] },
    SoftFocus { [radius_px], [strength] },
    Orton { [radius_px], [strength], [brightness], [contrast], [saturation] },
    Mosaic { [tile_mode], [boundary], [block_px] },
    Sharpen { [amount], [radius_px], [threshold] },
    SmartSharpen { [amount], [radius_px], [edge_threshold], [halo_suppression] },
    Hsl { [hue_degrees], [saturation], [lightness] },
    ColorMixer { [bands[0].hue_degrees], [bands[0].saturation], [bands[0].lightness], [bands[1].hue_degrees], [bands[1].saturation], [bands[1].lightness], [bands[2].hue_degrees], [bands[2].saturation], [bands[2].lightness], [bands[3].hue_degrees], [bands[3].saturation], [bands[3].lightness], [bands[4].hue_degrees], [bands[4].saturation], [bands[4].lightness], [bands[5].hue_degrees], [bands[5].saturation], [bands[5].lightness], [bands[6].hue_degrees], [bands[6].saturation], [bands[6].lightness], [bands[7].hue_degrees], [bands[7].saturation], [bands[7].lightness], [range_degrees] },
    Look { [preset], [strength] },
    CubeLut { [name], [size], [domain_min[0]], [domain_min[1]], [domain_min[2]], [domain_max[0]], [domain_max[1]], [domain_max[2]], [table], [strength] },
    Posterize { [levels], [strength] },
    RetroPalette { [mode], [dither], [strength] },
    CrtDisplay { [mode], [scanline_spacing_px], [scanline_depth], [mask_strength], [curvature], [bloom], [horizontal_blur], [brightness], [strength] },
    Threshold { [threshold], [invert], [strength] },
    Invert { [strength] },
    Duotone { [preset], [strength], [contrast] },
    Equalize { [strength], [preserve_color] },
    GradientMap { [preset], [strength], [contrast] },
    Repair { [mode], [color_source], [sampled_rgb[0]], [sampled_rgb[1]], [sampled_rgb[2]], [sample_radius_px], [search_radius_px], [texture_strength], [color_match_strength], [quality], [patch_size], [seed], [clone_source_uv], [clone_destination_uv] },
    ColorFill { [shape], [start_rgb[0]], [start_rgb[1]], [start_rgb[2]], [middle_rgb[0]], [middle_rgb[1]], [middle_rgb[2]], [end_rgb[0]], [end_rgb[1]], [end_rgb[2]], [middle_enabled], [midpoint], [angle_degrees], [linear_points_enabled], [linear_start[0]], [linear_start[1]], [linear_end[0]], [linear_end[1]], [center[0]], [center[1]], [radius], [softness], [opacity] },
    Frame { [mode], [color_rgb[0]], [color_rgb[1]], [color_rgb[2]], [line_rgb[0]], [line_rgb[1]], [line_rgb[2]], [opacity], [width_px], [use_individual_widths], [top_px], [right_px], [bottom_px], [left_px], [softness_px], [line_width_px], [line_opacity], [aspect_ratio], [corner_radius_px] },
    OutlineStroke { [placement], [width_px], [softness_px], [opacity], [color_rgb[0]], [color_rgb[1]], [color_rgb[2]] },
    RimLight { [light_angle_degrees], [width_px], [falloff], [strength], [color_rgb[0]], [color_rgb[1]], [color_rgb[2]], [wrap] },
    ContactShadow { [radius_px], [softness_px], [strength], [color_rgb[0]], [color_rgb[1]], [color_rgb[2]], [direction_degrees], [directionality] },
    ColorTrace { [strength], [line_threshold], [softness], [sample_radius_px], [darkness], [saturation] },
    ColorOverlay { [shape], [blend_mode], [start_rgb[0]], [start_rgb[1]], [start_rgb[2]], [end_rgb[0]], [end_rgb[1]], [end_rgb[2]], [angle_degrees], [linear_points_enabled], [linear_start[0]], [linear_start[1]], [linear_end[0]], [linear_end[1]], [center[0]], [center[1]], [radius], [softness], [opacity] },
    NeonGlow { [threshold], [by_saturation], [inner_radius_px], [outer_radius_px], [strength], [inner_amount], [outer_amount], [glow_saturation], [tint_rgb[0]], [tint_rgb[1]], [tint_rgb[2]], [tint_strength], [screen_blend], [source_color_enabled], [source_rgb[0]], [source_rgb[1]], [source_rgb[2]], [source_tolerance], [source_feather] },
    DiffuseGlow { [threshold], [radius_px], [strength], [white_mix], [grain], [seed] },
    Bloom { [threshold], [radius_px], [strength] },
    Halation { [threshold], [radius_px], [strength], [warmth], [tint_rgb[0]], [tint_rgb[1]], [tint_rgb[2]], [edge_bias], [screen_blend] },
    ColorDodgeGlow { [threshold], [radius_px], [strength], [dodge_amount], [color_rgb[0]], [color_rgb[1]], [color_rgb[2]], [color_strength] },
    GodRays { [center[0]], [center[1]], [threshold], [length_px], [decay], [strength], [warm_tint] },
    LensFlare { [center[0]], [center[1]], [radius_px], [strength], [core_strength], [halo_strength], [ghost_strength], [streak_strength], [warm_tint] },
    AnamorphicFlare { [threshold], [length_px], [thickness_px], [strength], [color_rgb[0]], [color_rgb[1]], [color_rgb[2]], [color_strength] },
    LightLeak { [center[0]], [center[1]], [color_rgb[0]], [color_rgb[1]], [color_rgb[2]], [radius], [intensity], [falloff], [haze], [streak_strength], [streak_angle_degrees], [strength], [seed] },
    BacklightHaze { [center[0]], [center[1]], [color_rgb[0]], [color_rgb[1]], [color_rgb[2]], [radius], [falloff], [haze], [glow], [shadow_lift], [contrast_fade], [saturation_fade], [strength] },
    SpeedLines { [mode], [center[0]], [center[1]], [angle_degrees], [line_count], [line_width_px], [length], [inner_radius], [outer_radius], [softness], [strength], [color_rgb[0]], [color_rgb[1]], [color_rgb[2]], [seed] },
    RadialFlash { [center[0]], [center[1]], [ray_count], [rotation_degrees], [inner_radius], [outer_radius], [softness], [white_amount], [black_amount], [invert], [strength] },
    CloudFog { [mode], [scale_px], [detail], [density], [contrast], [height_fade], [opacity], [color_rgb[0]], [color_rgb[1]], [color_rgb[2]], [seed] },
    Spotlight { [center[0]], [center[1]], [radius], [feather], [light_strength], [shadow_strength], [tint_rgb[0]], [tint_rgb[1]], [tint_rgb[2]], [tint_strength] },
    Vignette { [strength], [radius], [feather] },
    FilmGrain { [amount], [size_px], [seed] },
    Noise { [amount], [distribution], [monochrome], [seed] },
    ChromaticAberration { [offset_px] },
    Anaglyph3d { [mode], [disparity_px], [angle_degrees], [luma_mix], [strength] },
    Defringe { [radius_px], [edge_threshold], [color_threshold], [neutralize], [strength] },
    ScanlineGlitch { [line_spacing_px], [line_strength], [jitter_px], [rgb_shift_px], [block_strength], [noise], [seed], [strength] },
    Vhs { [chroma_bleed_px], [chroma_shift_px], [ghost_offset_px], [ghost_strength], [tracking_strength], [scanline_strength], [noise], [desaturation], [seed], [strength] },
    DataMosh { [block_size_px], [displacement_px], [direction_degrees], [low_threshold], [high_threshold], [freeze], [smear], [rgb_shift_px], [noise], [seed], [strength] },
    PixelSort { [direction], [order], [low_threshold], [high_threshold], [max_segment_px], [strength] },
    OldFilm { [sepia], [fade], [vignette], [grain], [dust], [scratches], [seed], [strength] },
    WaterCaustics { [scale_px], [intensity], [contrast], [tint], [depth], [phase], [seed], [strength] },
    ParticleOverlay { [mode], [density], [size_px], [length_px], [angle_degrees], [opacity], [color_rgb[0]], [color_rgb[1]], [color_rgb[2]], [seed], [strength] },
    Aurora { [band_count], [scale_px], [height], [waviness], [softness], [brightness], [color_rgb[0]], [color_rgb[1]], [color_rgb[2]], [secondary_rgb[0]], [secondary_rgb[1]], [secondary_rgb[2]], [phase], [seed], [strength] },
    Halftone { [cell_px], [strength] },
    ScreenTone { [mode], [cell_px], [angle_degrees], [density], [gradation], [softness], [strength] },
    ColorHalftone { [cell_px], [angle_offset_degrees], [dot_gain], [black_generation], [softness], [strength] },
    CmykPlateShift { [offset_px], [angle_degrees], [black_offset_px], [black_generation], [ink_gain], [strength] },
    Lithograph { [ink_a_rgb[0]], [ink_a_rgb[1]], [ink_a_rgb[2]], [ink_b_rgb[0]], [ink_b_rgb[1]], [ink_b_rgb[2]], [paper_rgb[0]], [paper_rgb[1]], [paper_rgb[2]], [ink_density], [posterization], [grain], [misregistration_px], [angle_degrees], [paper_texture], [strength], [seed] },
    Engraving { [ink_rgb[0]], [ink_rgb[1]], [ink_rgb[2]], [paper_rgb[0]], [paper_rgb[1]], [paper_rgb[2]], [line_spacing_px], [line_width], [angle_degrees], [crosshatch], [contour_strength], [tone_levels], [ink_density], [paper_texture], [strength], [seed] },
    NewspaperPrint { [cell_px], [dot_gain], [ink_bleed], [paper_age], [paper_texture], [contrast], [fade], [strength], [seed] },
    Textureizer { [mode], [scale_px], [depth], [contrast], [warmth], [strength], [seed] },
    StarGlow { [ray_count], [rotation_degrees], [threshold], [length_px], [strength] },
    DiffractionStarburst { [blade_count], [rotation_degrees], [threshold], [length_px], [width_px], [halo_radius_px], [chromatic_shift], [strength] },
    EdgeSmooth { [radius_px], [strength], [edge_threshold] },
    Despeckle { [radius_px], [threshold], [strength] },
    Median { [radius_px], [strength] },
}

#[cfg(test)]
mod tests {
    use super::*;
    use local_adjust_core::{
        ColorBalanceParams, LinearGradientMask, MaskShape, ShapeOp, ToneParams,
    };

    fn shape_layer() -> LocalAdjustmentLayer {
        let mut mask = RasterVectorMask::empty(2, 2);
        mask.shapes.push(MaskShape::Rect {
            op: ShapeOp::Add,
            center: [0.25, 0.25],
            half_w: 0.1,
            half_h: 0.1,
            rotation_rad: 0.0,
        });
        LocalAdjustmentLayer::new("shape", LocalMask::RasterVector(mask), LocalEffect::None)
    }

    #[test]
    fn old_ui_opacity_and_mask_application_preserve_latest_key_edit_and_raster() {
        for toggle_mask_application in [false, true] {
            let before = shape_layer();
            let mut after = before.clone();
            if toggle_mask_application {
                after.mask_before_effect = !after.mask_before_effect;
            } else {
                after.opacity = 0.4;
            }
            let mut latest = before.clone();
            let LocalMask::RasterVector(mask) = &mut latest.mask else {
                unreachable!();
            };
            let MaskShape::Rect { center, .. } = &mut mask.shapes[0] else {
                unreachable!();
            };
            *center = [0.75, 0.6];
            mask.alpha = Arc::new(vec![0.5; 4]);
            let latest_mask = mask.clone();

            LocalLayerParameterEdit { before, after }.apply(&mut latest);

            let LocalMask::RasterVector(mask) = &latest.mask else {
                panic!("the latest manual mask must remain");
            };
            assert_eq!(mask.shapes, latest_mask.shapes);
            assert!(Arc::ptr_eq(&mask.alpha, &latest_mask.alpha));
            if toggle_mask_application {
                assert!(latest.mask_before_effect);
                assert_eq!(latest.opacity, 1.0);
            } else {
                assert_eq!(latest.opacity, 0.4);
            }
        }
    }

    #[test]
    fn same_effect_edit_preserves_a_different_latest_parameter() {
        let before = LocalAdjustmentLayer::new(
            "tone",
            LocalMask::Full,
            LocalEffect::Tone(ToneParams::default()),
        );
        let mut after = before.clone();
        let LocalEffect::Tone(params) = &mut after.effect else {
            unreachable!()
        };
        params.brightness = 20.0;
        let mut latest = before.clone();
        let LocalEffect::Tone(params) = &mut latest.effect else {
            unreachable!()
        };
        params.gamma = 1.8;

        LocalLayerParameterEdit { before, after }.apply(&mut latest);

        let LocalEffect::Tone(params) = latest.effect else {
            unreachable!()
        };
        assert_eq!(params.brightness, 20.0);
        assert_eq!(params.gamma, 1.8);
    }

    #[test]
    fn same_gradient_edit_preserves_latest_other_endpoint_and_coordinate() {
        let before = LocalAdjustmentLayer::new(
            "gradient",
            LocalMask::LinearGradient(LinearGradientMask::default()),
            LocalEffect::None,
        );
        let mut after = before.clone();
        let LocalMask::LinearGradient(params) = &mut after.mask else {
            unreachable!()
        };
        params.start[0] = 0.2;
        let mut latest = before.clone();
        let LocalMask::LinearGradient(params) = &mut latest.mask else {
            unreachable!()
        };
        params.start[1] = 0.7;
        params.end = [0.9, 0.8];

        LocalLayerParameterEdit { before, after }.apply(&mut latest);

        let LocalMask::LinearGradient(params) = latest.mask else {
            unreachable!()
        };
        assert_eq!(params.start, [0.2, 0.7]);
        assert_eq!(params.end, [0.9, 0.8]);
    }

    #[test]
    fn nested_effect_parameter_edit_preserves_other_channels_in_the_same_range() {
        let before = LocalAdjustmentLayer::new(
            "balance",
            LocalMask::Full,
            LocalEffect::ColorBalance(ColorBalanceParams::default()),
        );
        let mut after = before.clone();
        let LocalEffect::ColorBalance(params) = &mut after.effect else {
            unreachable!()
        };
        params.shadows.cyan_red = 0.25;
        let mut latest = before.clone();
        let LocalEffect::ColorBalance(params) = &mut latest.effect else {
            unreachable!()
        };
        params.shadows.yellow_blue = 0.7;

        LocalLayerParameterEdit { before, after }.apply(&mut latest);

        let LocalEffect::ColorBalance(params) = latest.effect else {
            unreachable!()
        };
        assert_eq!(params.shadows.cyan_red, 0.25);
        assert_eq!(params.shadows.yellow_blue, 0.7);
    }

    #[test]
    fn removing_add_override_preserves_latest_subtract_override() {
        let mut before = shape_layer();
        before.manual_override.add = Some(RasterVectorMask::empty(2, 2));
        before.manual_override.subtract = Some(RasterVectorMask::empty(2, 2));
        let mut after = before.clone();
        after.manual_override.add = None;
        let mut latest = before.clone();
        latest.manual_override.subtract.as_mut().unwrap().alpha = Arc::new(vec![0.6; 4]);
        let latest_subtract = latest.manual_override.subtract.clone().unwrap();

        LocalLayerParameterEdit { before, after }.apply(&mut latest);

        assert!(latest.manual_override.add.is_none());
        let subtract = latest.manual_override.subtract.unwrap();
        assert!(Arc::ptr_eq(&subtract.alpha, &latest_subtract.alpha));
    }

    #[test]
    fn explicit_variant_switch_replaces_the_selected_mask_and_effect() {
        let before = shape_layer();
        let mut after = before.clone();
        after.mask = LocalMask::LinearGradient(LinearGradientMask::default());
        after.effect = LocalEffect::Tone(ToneParams::default());
        let mut latest = before.clone();

        LocalLayerParameterEdit {
            before,
            after: after.clone(),
        }
        .apply(&mut latest);

        assert_eq!(latest.mask, after.mask);
        assert_eq!(latest.effect, after.effect);
    }

    #[test]
    fn untouched_old_ui_variants_do_not_replace_newer_mask_or_effect() {
        let before = shape_layer();
        let mut after = before.clone();
        after.opacity = 0.4;
        let mut latest = before.clone();
        latest.mask = LocalMask::LinearGradient(LinearGradientMask::default());
        latest.effect = LocalEffect::Tone(ToneParams::default());
        let latest_mask = latest.mask.clone();
        let latest_effect = latest.effect.clone();

        LocalLayerParameterEdit { before, after }.apply(&mut latest);

        assert_eq!(latest.mask, latest_mask);
        assert_eq!(latest.effect, latest_effect);
        assert_eq!(latest.opacity, 0.4);
    }

    fn subject_layer(alpha: &[f32], enabled: bool) -> LocalAdjustmentLayer {
        let mut subject = SubjectMask::from_raster(local_adjust_core::RasterMask {
            width: alpha.len(),
            height: 1,
            alpha: Arc::new(alpha.to_vec()),
        });
        subject.refinement.threshold = 0.5;
        subject.refinement.expand_px = 0;
        subject.refinement.feather_px = 0;
        if enabled {
            let refinement = subject.refinement;
            crate::ui_adjustment_panel::apply_local_adjust_subject_refinement(
                &mut subject,
                refinement,
            );
        }
        LocalAdjustmentLayer::new("subject", LocalMask::Subject(subject), LocalEffect::None)
    }

    #[test]
    fn subject_threshold_intent_derives_from_the_latest_source_matte() {
        let before = subject_layer(&[0.2, 0.4, 0.6, 0.9], true);
        let mut after = before.clone();
        let LocalMask::Subject(subject) = &mut after.mask else {
            unreachable!()
        };
        let mut refinement = subject.refinement;
        refinement.threshold = 0.7;
        crate::ui_adjustment_panel::apply_local_adjust_subject_refinement(subject, refinement);
        assert_eq!(subject.alpha.as_slice(), &[0.0, 0.0, 0.0, 1.0]);
        let mut latest = subject_layer(&[0.9, 0.1, 0.8, 0.6], true);
        let LocalMask::Subject(subject) = &latest.mask else {
            unreachable!()
        };
        let source = subject.source_alpha.clone().unwrap();

        LocalLayerParameterEdit { before, after }.apply(&mut latest);

        let LocalMask::Subject(subject) = &latest.mask else {
            unreachable!()
        };
        assert_eq!(subject.refinement.threshold, 0.7);
        assert_eq!(subject.alpha.as_slice(), &[1.0, 0.0, 1.0, 0.0]);
        assert!(Arc::ptr_eq(subject.source_alpha.as_ref().unwrap(), &source));
    }

    #[test]
    fn subject_refinement_on_off_uses_latest_source_and_ignores_old_legacy_source_cache() {
        for enable in [true, false] {
            let mut before = subject_layer(&[0.2, 0.4, 0.6, 0.9], !enable);
            let LocalMask::Subject(subject) = &mut before.mask else {
                unreachable!()
            };
            if enable {
                // Refining a legacy matte creates a source cache in the old UI.
                subject.source_alpha = None;
            }
            let mut after = before.clone();
            let LocalMask::Subject(subject) = &mut after.mask else {
                unreachable!()
            };
            if enable {
                let refinement = subject.refinement;
                crate::ui_adjustment_panel::apply_local_adjust_subject_refinement(
                    subject, refinement,
                );
            } else {
                subject.alpha = subject.source_raster_mask().alpha;
                subject.refinement.enabled = false;
            }
            let mut latest = subject_layer(&[0.9, 0.1, 0.8, 0.6], !enable);
            let LocalMask::Subject(subject) = &latest.mask else {
                unreachable!()
            };
            let source = subject.source_alpha.clone().unwrap();

            LocalLayerParameterEdit { before, after }.apply(&mut latest);

            let LocalMask::Subject(subject) = &latest.mask else {
                unreachable!()
            };
            assert_eq!(subject.refinement.enabled, enable);
            assert!(Arc::ptr_eq(subject.source_alpha.as_ref().unwrap(), &source));
            if enable {
                assert_eq!(subject.alpha.as_slice(), &[1.0, 0.0, 1.0, 1.0]);
            } else {
                assert!(Arc::ptr_eq(&subject.alpha, &source));
            }
        }
    }

    #[test]
    fn an_explicit_subject_source_change_is_preserved_and_refined_with_latest_settings() {
        let before = subject_layer(&[0.2, 0.4, 0.6, 0.9], false);
        let after = subject_layer(&[0.9, 0.1, 0.8, 0.6], false);
        let LocalMask::Subject(subject) = &after.mask else {
            unreachable!()
        };
        let requested_source = subject.source_alpha.clone().unwrap();
        let mut latest = before.clone();
        let LocalMask::Subject(subject) = &mut latest.mask else {
            unreachable!()
        };
        let mut refinement = subject.refinement;
        refinement.threshold = 0.7;
        crate::ui_adjustment_panel::apply_local_adjust_subject_refinement(subject, refinement);

        LocalLayerParameterEdit { before, after }.apply(&mut latest);

        let LocalMask::Subject(subject) = &latest.mask else {
            unreachable!()
        };
        assert!(Arc::ptr_eq(
            subject.source_alpha.as_ref().unwrap(),
            &requested_source
        ));
        assert!(subject.refinement.enabled);
        assert_eq!(subject.refinement.threshold, 0.7);
        assert_eq!(subject.alpha.as_slice(), &[1.0, 0.0, 1.0, 0.0]);
    }
}
