//! フルスクリーン表示用のアニメーション (GIF / APNG) デコードと
//! メインスレッドへ送るキャッシュエントリ型の定義。
//!
//! 通常の (静止画) フルスクリーン読み込みは `app.rs` の `start_fs_load` が直接行うが、
//! GIF/APNG については本モジュールの `decode_*_frames` で全フレームを一括展開する。

use std::path::Path;

/// フルスクリーン読み込みスレッドからUIスレッドへ送るメッセージ。
pub enum FsLoadResult {
    /// 静止画（GIF・APNG の1フレーム目のみを含む）
    Static(egui::ColorImage),
    /// アニメーション: (フレーム画像, 表示時間[秒]) のベクタ
    Animated(Vec<(egui::ColorImage, f64)>),
    /// デコードに失敗した (fs_cache に Failed エントリを記録して
    /// 「読込中...」状態のまま固まらないようにする)
    Failed,
}

/// フルスクリーンキャッシュエントリ。
pub enum FsCacheEntry {
    /// 静止画。GPU テクスチャと CPU 側ピクセルデータ（分析パネル用）を保持する。
    Static {
        tex: egui::TextureHandle,
        pixels: std::sync::Arc<egui::ColorImage>,
    },
    Animated {
        frames: Vec<(egui::TextureHandle, f64)>, // (texture, delay_secs)
        current_frame: usize,
        next_frame_at: f64, // ctx.input(|i| i.time) 基準
    },
    /// デコード失敗。UI は「読込失敗」表示を出す。
    Failed,
}

/// GIF をデコードしてアニメーションフレーム列を返す。
/// 静止画（1フレーム）や失敗時は None を返す。
pub fn decode_gif_frames(path: &Path) -> Option<Vec<(egui::ColorImage, f64)>> {
    use image::codecs::gif::GifDecoder;
    use image::AnimationDecoder;

    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    let decoder = GifDecoder::new(reader).ok()?;
    let frames = decoder.into_frames().collect_frames().ok()?;
    if frames.len() <= 1 {
        return None;
    }

    Some(
        frames
            .into_iter()
            .map(|frame| {
                let (numer, denom) = frame.delay().numer_denom_ms();
                let delay = if denom > 0 {
                    numer as f64 / denom as f64 / 1000.0
                } else {
                    crate::logger::log("GIF animation frame denom=0, using 0.1s default".to_string());
                    0.1
                };
                let delay = delay.max(0.02); // 最低 20ms（Chrome 互換）
                let buf = frame.into_buffer();
                let (w, h) = buf.dimensions();
                let ci = egui::ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    buf.as_raw(),
                );
                (ci, delay)
            })
            .collect(),
    )
}

/// APNG をデコードしてアニメーションフレーム列を返す。
/// 静止画（1フレーム）・非 APNG・失敗時は None を返す。
pub fn decode_apng_frames(path: &Path) -> Option<Vec<(egui::ColorImage, f64)>> {
    use image::codecs::png::PngDecoder;
    use image::AnimationDecoder;

    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    let decoder = PngDecoder::new(reader).ok()?;
    if !decoder.is_apng().ok()? {
        return None;
    }

    let frames = decoder.apng().ok()?.into_frames().collect_frames().ok()?;
    if frames.len() <= 1 {
        return None;
    }

    Some(
        frames
            .into_iter()
            .map(|frame| {
                let (numer, denom) = frame.delay().numer_denom_ms();
                let delay = if denom > 0 {
                    numer as f64 / denom as f64 / 1000.0
                } else {
                    crate::logger::log("APNG animation frame denom=0, using 0.1s default".to_string());
                    0.1
                };
                let delay = delay.max(0.02);
                let buf = frame.into_buffer();
                let (w, h) = buf.dimensions();
                let ci = egui::ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    buf.as_raw(),
                );
                (ci, delay)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn decode_gif_animated() {
        let path = Path::new("testimage/rotating_earth.gif");
        if !path.exists() {
            eprintln!("skipping: testimage/rotating_earth.gif not found");
            return;
        }
        let frames = decode_gif_frames(path);
        assert!(frames.is_some(), "animated GIF should return Some");
        let frames = frames.unwrap();
        assert!(frames.len() > 1, "animated GIF should have multiple frames");
    }

    #[test]
    fn decode_gif_static_returns_none() {
        let dir = std::env::temp_dir().join("mimageviewer_test");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("static_1frame.gif");

        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
        img.save(&path).unwrap();

        let result = decode_gif_frames(&path);
        assert!(result.is_none(), "single-frame GIF should return None");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn decode_apng_static_png_returns_none() {
        let path = Path::new("testimage/bouncing_ball.png");
        if !path.exists() {
            eprintln!("skipping: testimage/bouncing_ball.png not found");
            return;
        }
        let result = decode_apng_frames(path);
        if let Some(frames) = &result {
            assert!(frames.len() > 1, "if APNG, should have multiple frames");
        }
    }

    #[test]
    fn gif_frame_delay_minimum() {
        let path = Path::new("testimage/rotating_earth.gif");
        if !path.exists() {
            eprintln!("skipping: testimage/rotating_earth.gif not found");
            return;
        }
        if let Some(frames) = decode_gif_frames(path) {
            for (i, (_img, delay)) in frames.iter().enumerate() {
                assert!(
                    *delay >= 0.02,
                    "frame {i} delay {delay} should be >= 0.02s"
                );
            }
        }
    }
}
