//! 音楽可視化で共有する配色判断。
//!
//! egui や音楽ビデオ側の色型には依存せず、本体の音楽ビューと書き出しがこの同じ純関数を
//! それぞれの描画 API へ渡す。

/// 描画バックエンドに依存しない非 premultiplied RGBA8。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisualizationColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl VisualizationColor {
    pub const fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn from_rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

/// 消灯時の鍵ベース色 (黒/白 × real/非表示レンジ)。
pub fn unlit_key_base(black: bool, real: bool) -> VisualizationColor {
    match (black, real) {
        (false, true) => VisualizationColor::from_rgb(208, 214, 219),
        (false, false) => VisualizationColor::from_rgb(58, 64, 70),
        (true, true) => VisualizationColor::from_rgb(13, 16, 19),
        (true, false) => VisualizationColor::from_rgb(34, 39, 44),
    }
}

/// 五度圏上のピッチクラスと点灯強度から、鍵の点灯色を返す。
pub fn key_color(midi: u8, value: f32) -> VisualizationColor {
    let pitch_class = midi % 12;
    let fifth_index = ((pitch_class as usize * 7) % 12) as f32;
    let hue = (fifth_index / 12.0 + 0.57) % 1.0;
    let saturation = 0.62 + value.clamp(0.0, 1.0) * 0.28;
    let brightness = 0.38 + value.clamp(0.0, 1.0) * 0.58;
    hsv_to_rgb(hue, saturation, brightness)
}

/// 本体鍵盤と同じ、消灯色から点灯色までの最終ブレンド色を返す。
pub fn key_fill(midi: u8, value: f32, black: bool) -> VisualizationColor {
    let active = key_color(midi, value);
    key_fill_with_color(value, black, active)
}

/// Blend an explicitly selected note palette with the shared unlit key base.
pub fn key_fill_with_color(
    value: f32,
    black: bool,
    active: VisualizationColor,
) -> VisualizationColor {
    let base = unlit_key_base(black, true);
    let blend = (value * if black { 0.96 } else { 0.94 }).clamp(0.0, 1.0);
    lerp_color(base, active, blend)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> VisualizationColor {
    let h = h.rem_euclid(1.0) * 6.0;
    let i = h.floor() as i32;
    let f = h - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    let (r, g, b) = match i.rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    VisualizationColor::from_rgb(
        (r.clamp(0.0, 1.0) * 255.0) as u8,
        (g.clamp(0.0, 1.0) * 255.0) as u8,
        (b.clamp(0.0, 1.0) * 255.0) as u8,
    )
}

/// 本体スペクトラムと同じ帯域位置・点灯強度の色を返す。
pub fn spectrum_color(index: usize, total: usize, value: f32) -> VisualizationColor {
    let t = if total <= 1 {
        0.0
    } else {
        index as f32 / (total - 1) as f32
    };
    let base = if t < 0.20 {
        lerp_color(
            VisualizationColor::from_rgb(188, 58, 34),
            VisualizationColor::from_rgb(236, 132, 34),
            t / 0.20,
        )
    } else if t < 0.52 {
        lerp_color(
            VisualizationColor::from_rgb(255, 216, 28),
            VisualizationColor::from_rgb(70, 232, 76),
            (t - 0.20) / 0.32,
        )
    } else if t < 0.78 {
        lerp_color(
            VisualizationColor::from_rgb(38, 230, 190),
            VisualizationColor::from_rgb(44, 138, 255),
            (t - 0.52) / 0.26,
        )
    } else {
        lerp_color(
            VisualizationColor::from_rgb(44, 138, 255),
            VisualizationColor::from_rgb(245, 82, 210),
            (t - 0.78) / 0.22,
        )
    };
    let alpha = (92.0 + 150.0 * value.clamp(0.0, 1.0)) as u8;
    with_alpha(
        brighten_color(base, 0.55 + value.clamp(0.0, 1.0) * 0.65),
        alpha,
    )
}

pub fn lerp_color(
    from: VisualizationColor,
    to: VisualizationColor,
    amount: f32,
) -> VisualizationColor {
    let amount = amount.clamp(0.0, 1.0);
    let lerp = |from: u8, to: u8| from as f32 + (to as f32 - from as f32) * amount;
    VisualizationColor::from_rgb(
        lerp(from.r, to.r) as u8,
        lerp(from.g, to.g) as u8,
        lerp(from.b, to.b) as u8,
    )
}

pub fn brighten_color(color: VisualizationColor, scale: f32) -> VisualizationColor {
    VisualizationColor::from_rgb(
        ((color.r as f32 * scale).min(255.0)) as u8,
        ((color.g as f32 * scale).min(255.0)) as u8,
        ((color.b as f32 * scale).min(255.0)) as u8,
    )
}

pub const fn with_alpha(color: VisualizationColor, alpha: u8) -> VisualizationColor {
    VisualizationColor::from_rgba(color.r, color.g, color.b, alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_keyboard_palette_matches_the_shipped_music_view() {
        assert_eq!(
            [
                unlit_key_base(false, true),
                unlit_key_base(false, false),
                unlit_key_base(true, true),
                unlit_key_base(true, false),
            ],
            [
                VisualizationColor::from_rgb(208, 214, 219),
                VisualizationColor::from_rgb(58, 64, 70),
                VisualizationColor::from_rgb(13, 16, 19),
                VisualizationColor::from_rgb(34, 39, 44),
            ]
        );
        assert_eq!(
            [
                key_color(21, 0.0),
                key_color(60, 0.5),
                key_fill(60, 0.75, false),
                key_fill(61, 0.25, true),
            ],
            [
                VisualizationColor::from_rgb(92, 36, 96),
                VisualizationColor::from_rgb(41, 116, 170),
                VisualizationColor::from_rgb(86, 158, 210),
                VisualizationColor::from_rgb(41, 42, 24),
            ]
        );
    }

    #[test]
    fn fixed_spectrum_palette_matches_the_shipped_music_view() {
        assert_eq!(
            [
                spectrum_color(0, 5, 0.0),
                spectrum_color(1, 5, 0.5),
                spectrum_color(4, 5, 1.0),
                spectrum_color(0, 1, 0.25),
            ],
            [
                VisualizationColor::from_rgba(103, 31, 18, 92),
                VisualizationColor::from_rgba(197, 190, 30, 167),
                VisualizationColor::from_rgba(255, 98, 252, 242),
                VisualizationColor::from_rgba(133, 41, 24, 129),
            ]
        );
    }
}
