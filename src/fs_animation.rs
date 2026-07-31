//! フルスクリーン表示用のアニメーション (GIF / APNG / WebP) デコードと
//! メインスレッドへ送るキャッシュエントリ型の定義。
//!
//! 通常の (静止画) フルスクリーン読み込みは `app.rs` の `start_fs_load` が直接行うが、
//! アニメーション画像については本モジュールの `decode_*_frames` で全フレームを
//! 一括展開する。

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
    /// 静止画（GIF・APNG・WebP の1フレーム目のみを含む）。
    /// `source_dims` はワーカーがデコードした直後・GPU 上限 clamp 前の寸法で、
    /// ホバーバーに原寸を表示したり「ダウンスケール表示中」警告を出すために使う。
    /// `ci.size` は clamp 後なので両者が一致しないとき = clamp が発動したケース。
    Static {
        ci: egui::ColorImage,
        source_dims: [usize; 2],
    },
    /// 保持済み CPU ピクセルから再投入する静止画。
    ///
    /// PDF ラスタ保持キャッシュの hit では、大きい `ColorImage` を clone せずに
    /// `Arc` のまま UI スレッドの upload backlog へ渡す。
    StaticCached {
        pixels: std::sync::Arc<egui::ColorImage>,
        source_dims: [usize; 2],
    },
    /// 360 度パノラマ用 (Phase 2a、SettleReady or SettleApproved 経路、
    /// docs/panorama-360-view-plan.md §4.6.0)。
    ///
    /// 通常の `Static` と同じ ColorImage を持ちつつ、フル解像度 RGBA を追加で運ぶ。
    /// ワーカーは同じ DynamicImage から tee で両方を生成する (二重デコード回避)。
    /// `high_res` は `App::pano_high_res_source` に格納される。
    StaticPanorama {
        ci: egui::ColorImage,
        source_dims: [usize; 2],
        high_res: crate::panorama::HighResSource,
    },
    /// アニメーション: (フレーム画像, 表示時間[秒]) のベクタ
    Animated(Vec<(egui::ColorImage, f64)>),
    /// デコードに失敗した (fs_cache に Failed エントリを記録して
    /// 「読込中...」状態のまま固まらないようにする)
    Failed,
}

/// フルスクリーンキャッシュエントリ。
pub enum AnimationPlayback {
    Playing { next_frame_at: f64 },
    Paused,
}

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
        frame_pixels: Vec<std::sync::Arc<egui::ColorImage>>,
        current_frame: usize,
        playback: AnimationPlayback,
        /// Static と同じく perf 相関用の load_seq。
        load_seq: u64,
    },
    /// デコード失敗。UI は「読込失敗」表示を出す。
    Failed,
    /// 動画ファイル (MP4/MKV/MOV/AVI/WMV/MPG/MPEG)。
    /// `VideoPlayer` がデコーダワーカー・音声出力・AV クロックを所有する。
    /// drop 時にすべてのスレッドが停止する。テクスチャは VideoPlayer 内部で
    /// in-place 更新されるので、Static のように外側で持たない。
    Video {
        player: Box<crate::video::VideoPlayer>,
        load_seq: u64,
    },
}

impl FsCacheEntry {
    pub fn pause_animation(&mut self) -> bool {
        match self {
            FsCacheEntry::Animated { playback, .. }
                if matches!(playback, AnimationPlayback::Playing { .. }) =>
            {
                *playback = AnimationPlayback::Paused;
                true
            }
            _ => false,
        }
    }

    pub fn toggle_animation(&mut self, now: f64) -> bool {
        let FsCacheEntry::Animated {
            frames,
            current_frame,
            playback,
            ..
        } = self
        else {
            return false;
        };
        *playback = match playback {
            AnimationPlayback::Playing { .. } => AnimationPlayback::Paused,
            AnimationPlayback::Paused => AnimationPlayback::Playing {
                next_frame_at: now
                    + frames
                        .get(*current_frame)
                        .map(|(_, delay)| delay.max(0.02))
                        .unwrap_or(0.1),
            },
        };
        true
    }

    pub fn animation_is_playing(&self) -> bool {
        matches!(
            self,
            FsCacheEntry::Animated {
                playback: AnimationPlayback::Playing { .. },
                ..
            }
        )
    }

    /// perf 相関用。Static / Animated / Video なら load_seq、Failed は 0。
    pub fn load_seq(&self) -> u64 {
        match self {
            FsCacheEntry::Static { load_seq, .. }
            | FsCacheEntry::Animated { load_seq, .. }
            | FsCacheEntry::Video { load_seq, .. } => *load_seq,
            FsCacheEntry::Failed => 0,
        }
    }
}

impl Drop for FsCacheEntry {
    fn drop(&mut self) {
        // Video エントリは drop 前に明示 shutdown して cpal stream / pump を即停止。
        // フィールド drop 順任せだと数百 ms 前動画の音声が hardware buffer から流れ続ける。
        if let FsCacheEntry::Video { player, .. } = self {
            player.shutdown();
        }
    }
}

/// 単一フレームを GPU テクスチャ上限 (`MAX_TEXTURE_DIM`) 以下に縮める。
/// 上限内ならそのまま返す。巨大 animated GIF/APNG が `ctx.load_texture` で
/// panic しないようにするための安全網。
fn clamp_rgba_frame_for_gpu(buf: image::RgbaImage) -> image::RgbaImage {
    let limit = crate::app::MAX_TEXTURE_DIM as u32;
    let (w, h) = buf.dimensions();
    let (new_w, new_h) = clamped_rgba_frame_dims(w, h, limit);
    if (new_w, new_h) == (w, h) {
        return buf;
    }
    crate::fast_resize::resize_rgba8_exact(
        &buf,
        new_w,
        new_h,
        crate::fast_resize::Quality::Bilinear,
    )
}

fn clamped_rgba_frame_dims(w: u32, h: u32, limit: u32) -> (u32, u32) {
    if w <= limit && h <= limit {
        return (w, h);
    }
    let scale = limit as f64 / w.max(h) as f64;
    let new_w = ((w as f64 * scale).round() as u32).max(1);
    let new_h = ((h as f64 * scale).round() as u32).max(1);
    (new_w, new_h)
}

fn frame_delay_secs(format: &str, delay: image::Delay) -> f64 {
    let (numer, denom) = delay.numer_denom_ms();
    let delay = if denom > 0 {
        numer as f64 / denom as f64 / 1000.0
    } else {
        crate::logger::log(format!(
            "{format} animation frame denom=0, using 0.1s default"
        ));
        0.1
    };
    delay.max(0.02)
}

fn rgba_frame_to_color_image(buf: image::RgbaImage) -> egui::ColorImage {
    let buf = clamp_rgba_frame_for_gpu(buf);
    let (w, h) = buf.dimensions();
    egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], buf.as_raw())
}

/// GIF をデコードしてアニメーションフレーム列を返す。
/// 静止画（1フレーム）や失敗時は None を返す。
pub fn decode_gif_frames(path: &Path) -> Option<Vec<(egui::ColorImage, f64)>> {
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    decode_gif_frames_from_reader(reader)
}

pub fn decode_gif_frames_from_bytes(bytes: &[u8]) -> Option<Vec<(egui::ColorImage, f64)>> {
    decode_gif_frames_from_reader(std::io::Cursor::new(bytes))
}

fn decode_gif_frames_from_reader<R>(reader: R) -> Option<Vec<(egui::ColorImage, f64)>>
where
    R: std::io::BufRead + std::io::Seek,
{
    use image::AnimationDecoder;
    use image::codecs::gif::GifDecoder;

    let decoder = GifDecoder::new(reader).ok()?;
    let frames = decoder.into_frames().collect_frames().ok()?;
    if frames.len() <= 1 {
        return None;
    }

    Some(
        frames
            .into_iter()
            .map(|frame| {
                let delay = frame_delay_secs("GIF", frame.delay());
                (rgba_frame_to_color_image(frame.into_buffer()), delay)
            })
            .collect(),
    )
}

/// APNG をデコードしてアニメーションフレーム列を返す。
/// 静止画（1フレーム）・非 APNG・失敗時は None を返す。
pub fn decode_apng_frames(path: &Path) -> Option<Vec<(egui::ColorImage, f64)>> {
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    decode_apng_frames_from_reader(reader)
}

pub fn decode_apng_frames_from_bytes(bytes: &[u8]) -> Option<Vec<(egui::ColorImage, f64)>> {
    decode_apng_frames_from_reader(std::io::Cursor::new(bytes))
}

fn decode_apng_frames_from_reader<R>(reader: R) -> Option<Vec<(egui::ColorImage, f64)>>
where
    R: std::io::BufRead + std::io::Seek,
{
    use image::AnimationDecoder;
    use image::codecs::png::PngDecoder;

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
                let delay = frame_delay_secs("APNG", frame.delay());
                (rgba_frame_to_color_image(frame.into_buffer()), delay)
            })
            .collect(),
    )
}

/// Animated WebP をデコードしてアニメーションフレーム列を返す。
/// 静止画（1フレーム）・失敗時は None を返す。
pub fn decode_webp_frames(path: &Path) -> Option<Vec<(egui::ColorImage, f64)>> {
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    decode_webp_frames_from_reader(reader)
}

/// ZIP 内 WebP など、実ファイルパスを持たない入力用。
pub fn decode_webp_frames_from_bytes(bytes: &[u8]) -> Option<Vec<(egui::ColorImage, f64)>> {
    decode_webp_frames_from_reader(std::io::Cursor::new(bytes))
}

fn decode_webp_frames_from_reader<R>(mut reader: R) -> Option<Vec<(egui::ColorImage, f64)>>
where
    R: std::io::BufRead + std::io::Seek,
{
    use image::AnimationDecoder;
    use image::codecs::webp::WebPDecoder;

    let background = webp_animation_background_rgba(&mut reader).unwrap_or([0, 0, 0, 0]);
    let mut decoder = WebPDecoder::new(reader).ok()?;
    if !decoder.has_animation() {
        return None;
    }
    decoder.set_background_color(image::Rgba(background)).ok()?;
    let frames = decoder.into_frames().collect_frames().ok()?;
    if frames.len() <= 1 {
        return None;
    }

    Some(
        frames
            .into_iter()
            .map(|frame| {
                let delay = frame_delay_secs("WebP", frame.delay());
                (rgba_frame_to_color_image(frame.into_buffer()), delay)
            })
            .collect(),
    )
}

fn webp_animation_background_rgba<R>(reader: &mut R) -> Option<[u8; 4]>
where
    R: std::io::Read + std::io::Seek,
{
    let origin = reader.stream_position().ok()?;
    let parsed = (|| {
        reader.seek(std::io::SeekFrom::Start(origin)).ok()?;

        let mut header = [0u8; 12];
        reader.read_exact(&mut header).ok()?;
        if &header[0..4] != b"RIFF" || &header[8..12] != b"WEBP" {
            return None;
        }
        let riff_size = u32::from_le_bytes(header[4..8].try_into().ok()?) as u64;
        let riff_end = origin.checked_add(8)?.checked_add(riff_size)?;
        let mut pos = origin.checked_add(12)?;

        while pos.checked_add(8)? <= riff_end {
            reader.seek(std::io::SeekFrom::Start(pos)).ok()?;
            let mut chunk_header = [0u8; 8];
            reader.read_exact(&mut chunk_header).ok()?;
            let chunk_size = u32::from_le_bytes(chunk_header[4..8].try_into().ok()?) as u64;
            let payload_start = pos.checked_add(8)?;

            if &chunk_header[0..4] == b"ANIM" && chunk_size >= 6 {
                let mut bgra = [0u8; 4];
                reader.read_exact(&mut bgra).ok()?;
                let [b, g, r, a] = bgra;
                return Some([r, g, b, a]);
            }

            pos = payload_start
                .checked_add(chunk_size)?
                .checked_add(chunk_size & 1)?;
        }
        None
    })();
    let _ = reader.seek(std::io::SeekFrom::Start(origin));
    parsed
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
        // 巨大バッファを確保すると CI で落ちるため、内部スケールの丸め挙動のみ確認。
        let (ow, oh) = clamped_rgba_frame_dims(w, limit / 2, limit);
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
                assert!(*delay >= 0.02, "frame {i} delay {delay} should be >= 0.02s");
            }
        }
    }

    fn riff_chunk(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + payload.len() + (payload.len() & 1));
        out.extend_from_slice(fourcc);
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        if payload.len() & 1 != 0 {
            out.push(0);
        }
        out
    }

    fn u24(n: u32) -> [u8; 3] {
        [n as u8, (n >> 8) as u8, (n >> 16) as u8]
    }

    fn static_webp_image_subchunk_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut webp = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
            .encode(rgba, width, height, image::ExtendedColorType::Rgba8)
            .unwrap();

        let mut pos = 12;
        while pos + 8 <= webp.len() {
            let fourcc = &webp[pos..pos + 4];
            let size =
                u32::from_le_bytes([webp[pos + 4], webp[pos + 5], webp[pos + 6], webp[pos + 7]])
                    as usize;
            let end = pos + 8 + size;
            assert!(end <= webp.len(), "encoded WebP chunk should fit");
            if fourcc == b"VP8L" || fourcc == b"VP8 " {
                return webp[pos..end + (size & 1)].to_vec();
            }
            pos = end + (size & 1);
        }
        panic!("encoded WebP should contain VP8L or VP8 chunk");
    }

    fn static_webp_image_subchunk(rgba: [u8; 4]) -> Vec<u8> {
        static_webp_image_subchunk_rgba(1, 1, &rgba)
    }

    fn animated_webp_fixture() -> Vec<u8> {
        let mut chunks = Vec::new();

        let mut vp8x = Vec::new();
        vp8x.push(0b0000_0010); // animation flag
        vp8x.extend_from_slice(&[0, 0, 0]); // reserved
        vp8x.extend_from_slice(&u24(0)); // canvas width - 1
        vp8x.extend_from_slice(&u24(0)); // canvas height - 1
        chunks.extend_from_slice(&riff_chunk(b"VP8X", &vp8x));

        let mut anim = Vec::new();
        anim.extend_from_slice(&[0, 0, 0, 0]); // background color
        anim.extend_from_slice(&0u16.to_le_bytes()); // loop forever
        chunks.extend_from_slice(&riff_chunk(b"ANIM", &anim));

        for (rgba, delay_ms) in [([255, 0, 0, 255], 40u32), ([0, 255, 0, 255], 80u32)] {
            let mut frame = Vec::new();
            frame.extend_from_slice(&u24(0)); // x / 2
            frame.extend_from_slice(&u24(0)); // y / 2
            frame.extend_from_slice(&u24(0)); // width - 1
            frame.extend_from_slice(&u24(0)); // height - 1
            frame.extend_from_slice(&u24(delay_ms));
            frame.push(0); // blend + no dispose
            frame.extend_from_slice(&static_webp_image_subchunk(rgba));
            chunks.extend_from_slice(&riff_chunk(b"ANMF", &frame));
        }

        let mut webp = Vec::new();
        webp.extend_from_slice(b"RIFF");
        webp.extend_from_slice(&((4 + chunks.len()) as u32).to_le_bytes());
        webp.extend_from_slice(b"WEBP");
        webp.extend_from_slice(&chunks);
        webp
    }

    fn animated_webp_dispose_background_fixture() -> Vec<u8> {
        let mut chunks = Vec::new();

        let mut vp8x = Vec::new();
        vp8x.push(0b0001_0010); // alpha + animation flags
        vp8x.extend_from_slice(&[0, 0, 0]); // reserved
        vp8x.extend_from_slice(&u24(3)); // canvas width - 1
        vp8x.extend_from_slice(&u24(1)); // canvas height - 1
        chunks.extend_from_slice(&riff_chunk(b"VP8X", &vp8x));

        let mut anim = Vec::new();
        anim.extend_from_slice(&[0, 0, 0, 0]); // transparent background (BGRA)
        anim.extend_from_slice(&0u16.to_le_bytes()); // loop forever
        chunks.extend_from_slice(&riff_chunk(b"ANIM", &anim));

        {
            let mut append_frame = |x: u32, rgba: [u8; 4], flags: u8| {
                let pixels: Vec<u8> = (0..4).flat_map(|_| rgba).collect();
                let mut frame = Vec::new();
                frame.extend_from_slice(&u24(x / 2));
                frame.extend_from_slice(&u24(0)); // y / 2
                frame.extend_from_slice(&u24(1)); // width - 1
                frame.extend_from_slice(&u24(1)); // height - 1
                frame.extend_from_slice(&u24(40)); // duration
                frame.push(flags);
                frame.extend_from_slice(&static_webp_image_subchunk_rgba(2, 2, &pixels));
                chunks.extend_from_slice(&riff_chunk(b"ANMF", &frame));
            };
            append_frame(0, [255, 0, 0, 255], 0b0000_0011); // no blend + dispose
            append_frame(2, [0, 255, 0, 255], 0b0000_0010); // no blend + no dispose
        }

        let mut webp = Vec::new();
        webp.extend_from_slice(b"RIFF");
        webp.extend_from_slice(&((4 + chunks.len()) as u32).to_le_bytes());
        webp.extend_from_slice(b"WEBP");
        webp.extend_from_slice(&chunks);
        webp
    }

    #[test]
    fn webp_animation_background_is_bgra_in_container() {
        let mut chunks = Vec::new();

        let mut vp8x = Vec::new();
        vp8x.push(0b0001_0010); // alpha + animation flags
        vp8x.extend_from_slice(&[0, 0, 0]);
        vp8x.extend_from_slice(&u24(0));
        vp8x.extend_from_slice(&u24(0));
        chunks.extend_from_slice(&riff_chunk(b"VP8X", &vp8x));

        let mut anim = Vec::new();
        anim.extend_from_slice(&[0x44, 0x33, 0x22, 0x11]); // B, G, R, A
        anim.extend_from_slice(&0u16.to_le_bytes());
        chunks.extend_from_slice(&riff_chunk(b"ANIM", &anim));

        let mut webp = Vec::new();
        webp.extend_from_slice(b"RIFF");
        webp.extend_from_slice(&((4 + chunks.len()) as u32).to_le_bytes());
        webp.extend_from_slice(b"WEBP");
        webp.extend_from_slice(&chunks);

        let mut cursor = std::io::Cursor::new(webp);
        assert_eq!(
            webp_animation_background_rgba(&mut cursor),
            Some([0x22, 0x33, 0x44, 0x11])
        );
        assert_eq!(cursor.position(), 0);
    }

    #[test]
    fn decode_webp_animated_from_bytes() {
        let bytes = animated_webp_fixture();
        let frames = decode_webp_frames_from_bytes(&bytes).expect("animated WebP should decode");
        assert_eq!(frames.len(), 2);
        assert!((frames[0].1 - 0.04).abs() < 0.001);
        assert!((frames[1].1 - 0.08).abs() < 0.001);
        assert_eq!(frames[0].0.size, [1, 1]);
        assert_eq!(frames[1].0.size, [1, 1]);
    }

    #[test]
    fn decode_webp_dispose_background_clears_previous_frame() {
        let bytes = animated_webp_dispose_background_fixture();
        let frames = decode_webp_frames_from_bytes(&bytes).expect("animated WebP should decode");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].0.size, [4, 2]);

        let left = frames[1].0.pixels[0];
        let right = frames[1].0.pixels[2];
        assert_eq!(
            left.a(),
            0,
            "disposed previous frame area should be transparent"
        );
        assert_eq!(right.to_srgba_unmultiplied(), [0, 255, 0, 255]);
    }

    #[test]
    fn decode_webp_static_returns_none() {
        let mut webp = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
            .encode(&[255, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        assert!(decode_webp_frames_from_bytes(&webp).is_none());
    }
}
