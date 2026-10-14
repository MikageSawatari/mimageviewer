//! Shared pitch-class palette for the keyboard, bass marker and timeline.
//! The immutable table avoids first-frame OKLab/gamut work on the UI thread.

use serde::{Deserialize, Serialize};

#[cfg(test)]
#[path = "music_note_color/compute.rs"]
mod compute;
#[path = "music_note_color/table.rs"]
mod table;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub enum MusicNoteColorScheme {
    #[default]
    Fifths,
    Chromatic,
}

/// Strength is quantized to the nearest of 256 steps. MIDI octaves share colors.
pub fn note_color(midi: u8, value: f32, scheme: MusicNoteColorScheme) -> egui::Color32 {
    let strength = (value.clamp(0.0, 1.0) * 255.0).round() as usize;
    let scheme = match scheme {
        MusicNoteColorScheme::Fifths => 0,
        MusicNoteColorScheme::Chromatic => 1,
    };
    let [r, g, b] = table::NOTE_COLORS[scheme][usize::from(midi % 12)][strength];
    egui::Color32::from_rgb(r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    const L_TOLERANCE: f64 = 0.004;

    fn rgb(color: egui::Color32) -> [f64; 3] {
        [color.r(), color.g(), color.b()].map(|c| f64::from(c) / 255.0)
    }

    fn check_color(scheme: MusicNoteColorScheme, pc: u8, strength: f32) {
        let original = compute::original_colors(f64::from(strength));
        let index = match scheme {
            MusicNoteColorScheme::Fifths => usize::from(pc) * 7 % 12,
            MusicNoteColorScheme::Chromatic => usize::from(pc),
        };
        let actual = note_color(pc, strength, scheme);
        let l = compute::rgb_to_oklab(rgb(actual))[0];
        let base_l = compute::rgb_to_oklab(original[index])[0];
        let target = compute::median_lightness(&original);
        assert!(
            l + L_TOLERANCE >= base_l,
            "darker: {scheme:?} pc={pc} strength={strength} L={l} base={base_l}"
        );
        assert!(
            l + L_TOLERANCE >= target,
            "below median: {scheme:?} pc={pc} strength={strength} L={l} median={target}"
        );
    }

    #[test]
    fn every_table_entry_matches_direct_calculation_and_is_never_darker() {
        assert_eq!(table::NOTE_COLORS, compute::generate_table());
        for scheme in [
            MusicNoteColorScheme::Fifths,
            MusicNoteColorScheme::Chromatic,
        ] {
            for pc in 0..12 {
                for index in 0..256 {
                    check_color(scheme, pc, index as f32 / 255.0);
                }
            }
        }
    }

    #[test]
    fn all_step_boundaries_round_to_nearest_and_preserve_lightness() {
        for index in 0..255 {
            let boundary = (index as f32 + 0.5) / 255.0;
            for value in [
                f32::from_bits(boundary.to_bits() - 1),
                boundary,
                f32::from_bits(boundary.to_bits() + 1),
            ] {
                let step = (value.clamp(0.0, 1.0) * 255.0).round() as usize;
                for scheme in [
                    MusicNoteColorScheme::Fifths,
                    MusicNoteColorScheme::Chromatic,
                ] {
                    for pc in 0..12 {
                        let s = usize::from(scheme == MusicNoteColorScheme::Chromatic);
                        let color = table::NOTE_COLORS[s][usize::from(pc)][step];
                        assert_eq!(
                            note_color(pc, value, scheme),
                            egui::Color32::from_rgb(color[0], color[1], color[2])
                        );
                        check_color(scheme, pc, value);
                    }
                }
            }
        }
    }

    #[test]
    fn c_is_red_and_neighbor_order_matches_both_schemes() {
        for index in 0..256 {
            let value = index as f32 / 255.0;
            let c = note_color(60, value, MusicNoteColorScheme::Fifths);
            assert!(c.r() > c.g() && c.r() > c.b());
            assert_eq!(c, note_color(60, value, MusicNoteColorScheme::Chromatic));
            assert_eq!(
                note_color(67, value, MusicNoteColorScheme::Fifths),
                note_color(61, value, MusicNoteColorScheme::Chromatic)
            );
            for pc in 0..12 {
                assert_eq!(
                    note_color(pc, value, MusicNoteColorScheme::Fifths),
                    note_color((pc * 7) % 12, value, MusicNoteColorScheme::Chromatic)
                );
                assert_eq!(
                    note_color(pc, value, MusicNoteColorScheme::Fifths),
                    note_color(pc + 72, value, MusicNoteColorScheme::Fifths)
                );
            }
        }
    }

    #[test]
    fn gamut_mapping_preserves_target_lightness_hue_and_bright_colors() {
        for index in 0..256 {
            let colors = compute::original_colors(index as f64 / 255.0);
            let target = compute::median_lightness(&colors);
            for original in colors {
                let lab = compute::rgb_to_oklab(original);
                let corrected = compute::corrected_color(original, target);
                assert!(corrected.into_iter().all(|c| (0.0..=1.0).contains(&c)));
                if lab[0] >= target {
                    assert_eq!(corrected, original);
                } else {
                    let actual = compute::rgb_to_oklab(corrected);
                    assert!((actual[0] - target).abs() < 1.0e-6);
                    assert!((actual[2].atan2(actual[1]) - lab[2].atan2(lab[1])).abs() < 1.0e-5);
                    assert!(actual[1].hypot(actual[2]) <= lab[1].hypot(lab[2]) + 1.0e-6);
                }
            }
        }
    }

    #[test]
    fn strength_clamps_at_endpoints() {
        for scheme in [
            MusicNoteColorScheme::Fifths,
            MusicNoteColorScheme::Chromatic,
        ] {
            for pc in 0..12 {
                assert_eq!(note_color(pc, -1.0, scheme), note_color(pc, 0.0, scheme));
                assert_eq!(note_color(pc, 2.0, scheme), note_color(pc, 1.0, scheme));
            }
        }
    }
}
