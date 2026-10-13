//! One UI pass's parameter changes, applied after prior page edits are accepted.
//! The old UI values describe intent; untouched values come from the latest draft.

use crate::adjustment::AdjustParams;
use crate::colorize::{ColorizeControlPoint, ColorizeParams};

#[derive(Debug, Clone)]
pub(crate) struct AdjustmentParameterEdit {
    pub before: AdjustParams,
    pub after: AdjustParams,
}

macro_rules! changed_fields {
    ($before:expr, $after:expr, $latest:expr; $($field:ident),* $(,)?) => {
        $(if $before.$field != $after.$field {
            $latest.$field = $after.$field.clone();
        })*
    };
}

impl AdjustmentParameterEdit {
    pub(crate) fn apply(self, latest: &mut AdjustParams) {
        let before = &self.before;
        let after = &self.after;
        // This exhaustive inventory makes newly added UI parameters visible here.
        let AdjustParams {
            brightness: _,
            contrast: _,
            gamma: _,
            saturation: _,
            temperature: _,
            black_point: _,
            white_point: _,
            midtone: _,
            auto_mode: _,
            upscale_model: _,
            denoise_model: _,
            post_filter: _,
            creative_lut: _,
            colorize: _,
            smart_sharpen: _,
        } = after;
        // Model/filter selections are explicit choices when they change.
        changed_fields!(before, after, latest;
            brightness, contrast, gamma, saturation, temperature, black_point,
            white_point, midtone, auto_mode, upscale_model, denoise_model,
            post_filter, smart_sharpen
        );
        let crate::creative_lut::CreativeLutSelection { id: _, strength: _ } = &after.creative_lut;
        changed_fields!(&before.creative_lut, &after.creative_lut, &mut latest.creative_lut;
            id, strength
        );
        apply_colorize(&before.colorize, &after.colorize, &mut latest.colorize);
    }
}

fn apply_colorize(before: &ColorizeParams, after: &ColorizeParams, latest: &mut ColorizeParams) {
    let ColorizeParams {
        mode: _,
        mono_tolerance: _,
        palette: _,
        control_points: _,
        luminance_weight: _,
        density_normalization_strength: _,
        tone_method: _,
        tone_radius: _,
        tone_strength: _,
    } = after;
    changed_fields!(before, after, latest;
        mode, mono_tolerance, palette, luminance_weight,
        density_normalization_strength, tone_method, tone_radius, tone_strength
    );
    apply_control_points(
        &before.control_points,
        &after.control_points,
        &mut latest.control_points,
    );
}

fn apply_control_points(
    before: &[ColorizeControlPoint],
    after: &[ColorizeControlPoint],
    latest: &mut Vec<ColorizeControlPoint>,
) {
    if before == after {
        return;
    }
    if before.len() != after.len() || after.len() != latest.len() {
        // Adding/removing points is an explicit palette composition change.
        *latest = after.to_vec();
        return;
    }
    for ((before, after), latest) in before.iter().zip(after).zip(latest) {
        for channel in 0..3 {
            if before.color[channel] != after.color[channel] {
                latest.color[channel] = after.color[channel];
            }
        }
        changed_fields!(before, after, latest; strength);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colorize::{ColorizeMode, ColorizePalette};

    #[test]
    fn stale_ui_brightness_change_preserves_latest_contrast_and_colorize() {
        let before = AdjustParams::default();
        let mut after = before.clone();
        after.brightness = 25.0;
        let mut latest = before.clone();
        latest.contrast = 40.0;
        latest.colorize.mode = ColorizeMode::AllImages;
        latest.colorize.palette = ColorizePalette::LegacySkin;
        latest.colorize.tone_strength = 70;
        let latest_colorize = latest.colorize.clone();

        AdjustmentParameterEdit { before, after }.apply(&mut latest);

        assert_eq!(latest.brightness, 25.0);
        assert_eq!(latest.contrast, 40.0);
        assert_eq!(latest.colorize, latest_colorize);
    }

    #[test]
    fn changing_one_colorize_field_preserves_latest_other_field_and_control_point() {
        let before = AdjustParams::default();
        let mut after = before.clone();
        after.colorize.tone_radius = 2.5;
        after.colorize.control_points[0].strength = 2.0;
        let mut latest = before.clone();
        latest.colorize.mono_tolerance = 28;
        latest.colorize.control_points[0].color = [30, 40, 50];
        latest.colorize.control_points[1].strength = 1.5;

        AdjustmentParameterEdit { before, after }.apply(&mut latest);

        assert_eq!(latest.colorize.tone_radius, 2.5);
        assert_eq!(latest.colorize.mono_tolerance, 28);
        assert_eq!(latest.colorize.control_points[0].strength, 2.0);
        assert_eq!(latest.colorize.control_points[0].color, [30, 40, 50]);
        assert_eq!(latest.colorize.control_points[1].strength, 1.5);
    }

    #[test]
    fn lut_strength_and_selection_are_independent_ui_parameters() {
        let old_id = uuid::Uuid::from_u128(1);
        let new_id = uuid::Uuid::from_u128(2);
        let mut before = AdjustParams::default();
        before.creative_lut.id = Some(old_id);
        let mut after = before.clone();
        after.creative_lut.strength = 0.4;
        let mut latest = before.clone();
        latest.creative_lut.id = Some(new_id);

        AdjustmentParameterEdit {
            before: before.clone(),
            after,
        }
        .apply(&mut latest);

        assert_eq!(latest.creative_lut.id, Some(new_id));
        assert_eq!(latest.creative_lut.strength, 0.4);

        let mut after = before.clone();
        after.creative_lut.id = None;
        let mut latest = before.clone();
        latest.creative_lut.strength = 0.6;

        AdjustmentParameterEdit { before, after }.apply(&mut latest);

        assert_eq!(latest.creative_lut.id, None);
        assert_eq!(latest.creative_lut.strength, 0.6);
    }
}
