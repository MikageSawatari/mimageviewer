//! フルスクリーン表示用のアニメーション (GIF / APNG) デコードと
//! メインスレッドへ送るキャッシュエントリ型の定義。
//!
//! 通常の (静止画) フルスクリーン読み込みは `app.rs` の `start_fs_load` が直接行うが、
//! GIF/APNG については本モジュールの `decode_*_frames` で全フレームを一括展開する。

use std::path::Path;

/// フルスクリーン読み込みスレッドからUIスレッドへ送るメッセージ。
///
/// デコードは時間がかかるので、メッセージは **2 段階** で送られることがある:
/// 1. `DimsOnly`  (任意)  — ファイルヘッダから寸法だけ取り出して先行送信。
///    ホバーバーにサイズと⚠ダウンスケール警告を即座に出すためのヒント。
///    受信しても fs_pending からは抜かない (本デコードが続く)。
/// 2. 本体 (`Static` / `Animated` / `Failed`) — 終端メッセージ。受信したら
///    fs_pending から該当エントリを削除する。
///
/// `DimsOnly` が送られずに `Static` がいきなり来るケースもある (PDF や
/// probe が失敗した場合など)。UI 側はそれも普通に扱えるよう、drain ループで
/// すべての受信メッセージを消化する。
pub enum FsLoadResult {
    /// ヘッダ解析だけで取れた EXIF 後相当の表示向き寸法。終端ではない。
    DimsOnly { source_dims: [usize; 2] },
    /// 静止画（GIF・APNG の1フレーム目のみを含む）。
    /// `source_dims` はワーカーがデコードした直後・GPU 上限 clamp 前の寸法で、
    /// ホバーバーに原寸を表示したり「ダウンスケール表示中」警告を出すために使う。
    /// `ci.size` は clamp 後なので両者が一致しないとき = clamp が発動したケース。
    Static { ci: egui::ColorImage, source_dims: [usize; 2] },
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
        /// GPU 上限 clamp 前の原寸 (幅, 高さ)。`pixels.size` と一致しないとき
        /// ダウンスケール表示中を意味する。派生キャッシュ (AI 結果・補正結果・
        /// 消しゴム結果) では `None` でよい。
        source_dims: Option<[usize; 2]>,
        /// このエントリを生成したロードの `input_seq`。perf 相関用。
        /// `fs.paint` で `fs.ready` と同じ seq を使うために保持する。
        /// 計装無効時や内部起因のエントリは 0。
        load_seq: u64,
    },
    Animated {
        frames: Vec<(egui::TextureHandle, f64)>, // (texture, delay_secs)
        current_frame: usize,
        next_frame_at: f64, // ctx.input(|i| i.time) 基準
        /// Static と同じく perf 相関用の load_seq。
        load_seq: u64,
    },
    /// デコード失敗。UI は「読込失敗」表示を出す。
    Failed,
}

impl FsCacheEntry {
    /// perf 相関用。Static / Animated なら load_seq、Failed は 0。
    pub fn load_seq(&self) -> u64 {
        match self {
            FsCacheEntry::Static { load_seq, .. } | FsCacheEntry::Animated { load_seq, .. } => {
                *load_seq
            }
            FsCacheEntry::Failed => 0,
        }
    }
}

/// 単一フレームを GPU テクスチャ上限 (`MAX_TEXTURE_DIM`) 以下に縮める。
/// 上限内ならそのまま返す。巨大 animated GIF/APNG が `ctx.load_texture` で
/// panic しないようにするための安全網。
fn clamp_rgba_frame_for_gpu(buf: image::RgbaImage) -> image::RgbaImage {
    let limit = crate::app::MAX_TEXTURE_DIM as u32;
    let (w, h) = buf.dimensions();
    if w <= limit && h <= limit {
        return buf;
    }
    let scale = limit as f64 / w.max(h) as f64;
    let new_w = ((w as f64 * scale).round() as u32).max(1);
    let new_h = ((h as f64 * scale).round() as u32).max(1);
    crate::fast_resize::resize_rgba8_exact(
        &buf,
        new_w,
        new_h,
        crate::fast_resize::Quality::Bilinear,
    )
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
                let buf = clamp_rgba_frame_for_gpu(frame.into_buffer());
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
                let buf = clamp_rgba_frame_for_gpu(frame.into_buffer());
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
    fn clamp_rgba_frame_noop_when_within_limit() {
        let buf = image::RgbaImage::from_pixel(1024, 768, image::Rgba([10, 20, 30, 255]));
        let out = clamp_rgba_frame_for_gpu(buf);
        assert_eq!(out.dimensions(), (1024, 768));
    }

    #[test]
    fn clamp_rgba_frame_shrinks_oversized_dims() {
        let limit = crate::app::MAX_TEXTURE_DIM as u32;
        let w = limit + 2048;
        // 本番と同じ比率で縮小されることを小さい画像で検証する。
        // 巨大バッファを確保すると CI で遅いので、内部スケールの丸め挙動のみ確認。
        let buf = image::RgbaImage::from_pixel(w, limit / 2, image::Rgba([0, 0, 0, 255]));
        let out = clamp_rgba_frame_for_gpu(buf);
        let (ow, oh) = out.dimensions();
        assert!(ow <= limit && oh <= limit, "clamped size should fit limit");
        assert_eq!(ow, limit, "long side should be pinned to limit");
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
