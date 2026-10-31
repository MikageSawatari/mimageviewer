use super::*;

#[test]
fn p5a_radial_geometry_uses_ratio_and_available_cap_space() {
    // D17.6: default geometry is the entire 1100px square, not 1100x1080.
    // The 168px centerline plus the 2px outer cap occupies the full 170px gap.
    let reference = radial_spectrum_bars(
        PixelRect {
            x0: 0,
            y0: 0,
            x1: 1100,
            y1: 1100,
        },
        &[1.0],
        1.0,
        380.0 / 550.0,
        4.0,
        0.0,
        0.0,
        SpectrumDirection::Clockwise,
    );
    assert_eq!(reference[0].start, (930.0, 550.0));
    assert_eq!(reference[0].end, (1098.0, 550.0));
    assert_eq!(reference[0].end.0 + 2.0 - reference[0].start.0, 170.0);
    let rect = PixelRect {
        x0: 0,
        y0: 0,
        x1: 120,
        y1: 80,
    };
    let bars = radial_spectrum_bars(
        rect,
        &[0.0, 1.0],
        1.0,
        0.5,
        4.0,
        0.0,
        0.0,
        SpectrumDirection::Clockwise,
    );
    assert_eq!(bars[0].start, (80.0, 40.0));
    assert_eq!(bars[0].end, (84.0, 40.0));
    assert_eq!(bars[1].end, (98.0, 40.0));
    // Static box edits leave thickness in px: the visible cap still ends at 100.
    let doubled = PixelRect {
        x0: 0,
        y0: 0,
        x1: 240,
        y1: 160,
    };
    let fixed_thickness = radial_spectrum_bars(
        doubled,
        &[1.0],
        1.0,
        0.5,
        4.0,
        0.0,
        0.0,
        SpectrumDirection::Clockwise,
    );
    assert_eq!(fixed_thickness[0].start, (160.0, 80.0));
    assert_eq!(
        fixed_thickness[0].end.0 + fixed_thickness[0].thickness * 0.5,
        200.0
    );
    // Whole-layer Scale doubles the cap as well: every endpoint and length doubles.
    let scaled = radial_spectrum_bars(
        doubled,
        &[0.0, 1.0],
        2.0,
        0.5,
        8.0,
        0.0,
        0.0,
        SpectrumDirection::Clockwise,
    );
    for (base, scaled) in bars.iter().zip(scaled) {
        assert_eq!(scaled.start, (base.start.0 * 2.0, base.start.1 * 2.0));
        assert_eq!(scaled.end, (base.end.0 * 2.0, base.end.1 * 2.0));
        assert_eq!(scaled.thickness, base.thickness * 2.0);
    }
    for (ratio, expected) in [(-1.0, 60.0), (0.0, 60.0), (1.0, 98.0), (2.0, 98.0)] {
        let bars = radial_spectrum_bars(
            rect,
            &[1.0],
            1.0,
            ratio,
            4.0,
            0.0,
            0.0,
            SpectrumDirection::Clockwise,
        );
        assert_eq!(bars[0].start.0, expected);
        assert_eq!(bars[0].end.0 + 2.0, 100.0);
    }
}
