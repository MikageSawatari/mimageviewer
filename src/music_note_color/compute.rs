//! Pure table generation math, shared by the offline generator and regression tests.

pub fn original_colors(value: f64) -> [[f64; 3]; 12] {
    std::array::from_fn(|index| {
        let s = 0.62 + 0.28 * value;
        let v = 0.38 + 0.58 * value;
        let h = index as f64 / 12.0 * 6.0;
        let f = h.fract();
        let p = v * (1.0 - s);
        let q = v * (1.0 - s * f);
        let t = v * (1.0 - s * (1.0 - f));
        match h.floor() as usize {
            0 => [v, t, p],
            1 => [q, v, p],
            2 => [p, v, t],
            3 => [p, q, v],
            4 => [t, p, v],
            _ => [v, p, q],
        }
    })
}

pub fn linearize(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn encode(c: f64) -> f64 {
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

pub fn rgb_to_oklab(rgb: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = rgb.map(linearize);
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

pub fn oklab_to_linear_rgb([l, a, b]: [f64; 3]) -> [f64; 3] {
    let lp = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let mp = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let sp = (l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    [
        4.0767416621 * lp - 3.3077115913 * mp + 0.2309699292 * sp,
        -1.2684380046 * lp + 2.6097574011 * mp - 0.3413193965 * sp,
        -0.0041960863 * lp - 0.7034186147 * mp + 1.7076147010 * sp,
    ]
}

pub fn median_lightness(colors: &[[f64; 3]; 12]) -> f64 {
    let mut lightness = colors.map(|rgb| rgb_to_oklab(rgb)[0]);
    lightness.sort_by(f64::total_cmp);
    (lightness[5] + lightness[6]) * 0.5
}

fn in_gamut(rgb: [f64; 3]) -> bool {
    rgb.into_iter().all(|c| (0.0..=1.0).contains(&c))
}

/// Raise only L, keeping the OKLCH hue fixed. Reduce chroma by binary search,
/// never by clipping RGB components. Bright colors keep their original RGB.
pub fn corrected_color(rgb: [f64; 3], target: f64) -> [f64; 3] {
    let [l, a, b] = rgb_to_oklab(rgb);
    if l >= target {
        return rgb;
    }
    let chroma = a.hypot(b);
    let hue = b.atan2(a);
    let to_rgb = |c| oklab_to_linear_rgb([target, c * hue.cos(), c * hue.sin()]);
    let mut c = chroma;
    if !in_gamut(to_rgb(c)) {
        let mut low = 0.0;
        let mut high = chroma;
        for _ in 0..32 {
            let mid = (low + high) * 0.5;
            if in_gamut(to_rgb(mid)) {
                low = mid;
            } else {
                high = mid;
            }
        }
        c = low;
    }
    let linear_rgb = to_rgb(c);
    assert!(in_gamut(linear_rgb));
    linear_rgb.map(encode)
}

pub fn colors_at_strength(value: f64) -> [[u8; 3]; 12] {
    let original = original_colors(value);
    let target = median_lightness(&original);
    original.map(|rgb| corrected_color(rgb, target).map(|c| (c * 255.0).round() as u8))
}

pub fn generate_table() -> [[[[u8; 3]; 256]; 12]; 2] {
    let by_strength: [[[u8; 3]; 12]; 256] =
        std::array::from_fn(|index| colors_at_strength(index as f64 / 255.0));
    std::array::from_fn(|scheme| {
        std::array::from_fn(|pc| {
            let hue_index = if scheme == 0 { pc * 7 % 12 } else { pc };
            std::array::from_fn(|strength| by_strength[strength][hue_index])
        })
    })
}
