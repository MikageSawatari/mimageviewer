//! 動画の現在フレームをクリップボード用に抽出する one-shot helper。
//!
//! メイン再生デコーダとは独立した FFmpeg input を開き、指定 pts 近傍の 1 フレームを
//! RGBA8 に変換して返す。呼び出し側は必ず worker thread から呼ぶこと。

use std::path::Path;

#[derive(Debug)]
pub struct CapturedVideoFrame {
    pub target_secs: f64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn capture_frame(path: &Path, target_secs: f64) -> Result<CapturedVideoFrame, String> {
    use ffmpeg::format::Pixel;
    use ffmpeg::media::Type as MediaType;
    use ffmpeg::software::scaling::{Context as ScaleContext, Flags as ScaleFlags};
    use ffmpeg::util::frame::video::Video;
    use ffmpeg_the_third as ffmpeg;

    if !target_secs.is_finite() {
        return Err("target secs is not finite".to_string());
    }
    let target_secs = target_secs.max(0.0);

    ffmpeg::init().map_err(|e| format!("ffmpeg init failed: {e}"))?;

    let mut input = ffmpeg::format::input(path)
        .map_err(|e| format!("open input failed: {}: {e}", path.display()))?;
    let video_stream = input
        .streams()
        .best(MediaType::Video)
        .ok_or_else(|| "video stream not found".to_string())?;
    let stream_idx = video_stream.index();
    let time_base = video_stream.time_base();
    let tb_num = time_base.numerator() as f64;
    let tb_den = time_base.denominator() as f64;
    let timestamp_epsilon_secs = timestamp_epsilon_secs(tb_num, tb_den);
    let params = video_stream.parameters();

    let codec_ctx = ffmpeg::codec::context::Context::from_parameters(params)
        .map_err(|e| format!("codec ctx failed: {e}"))?;
    let mut decoder = codec_ctx
        .decoder()
        .video()
        .map_err(|e| format!("decoder open failed: {e}"))?;
    let src_w = decoder.width();
    let src_h = decoder.height();
    if src_w == 0 || src_h == 0 {
        return Err("video frame size is zero".to_string());
    }
    // `decoder.format()` は HW accel attach 時に `Pixel::D3D11` を返し swscale
    // `av_assert0` → `abort()` を踏むので、scaler は **frame 取得後に lazy 構築**
    // する。HW frame は `prepare_frame_for_swscale` で SW download してから渡す。
    // (詳細は `src/video/swscale_helpers.rs`)

    let target_pts = (target_secs * 1_000_000.0) as i64;
    let seek_ok = unsafe {
        use ffmpeg::ffi::{AVSEEK_FLAG_BACKWARD, av_seek_frame};
        av_seek_frame(
            input.as_mut_ptr(),
            -1,
            target_pts,
            AVSEEK_FLAG_BACKWARD as i32,
        ) >= 0
    };
    if !seek_ok {
        return Err(format!("seek failed: target={target_secs:.3}s"));
    }
    decoder.flush();

    let mut got_frame: Option<(Video, f64)> = None;
    let mut before_frame: Option<(Video, f64)> = None;
    let mut video_packets_seen = 0;
    let mut frames_seen = 0;
    for item in input.packets() {
        let (stream, packet) = match item {
            Ok(sp) => sp,
            Err(e) => return Err(format!("read packet failed: {e}")),
        };
        if stream.index() != stream_idx {
            continue;
        }
        video_packets_seen += 1;
        if video_packets_seen > 240 {
            break;
        }
        if decoder.send_packet(&packet).is_err() {
            continue;
        }
        let mut frame = Video::empty();
        while decoder.receive_frame(&mut frame).is_ok() {
            frames_seen += 1;
            let Some(raw_pts) = crate::video::decoder::video_frame_timestamp(&frame) else {
                frame = Video::empty();
                continue;
            };
            let pts_secs = raw_pts as f64 * tb_num / tb_den;
            if (pts_secs - target_secs).abs() <= timestamp_epsilon_secs {
                got_frame = Some((frame, pts_secs));
                break;
            }
            if pts_secs < target_secs {
                before_frame = Some((frame, pts_secs));
            } else {
                let choose_after = before_frame
                    .as_ref()
                    .map(|(_, before_pts)| {
                        (pts_secs - target_secs).abs() <= (target_secs - *before_pts).abs()
                    })
                    .unwrap_or(true);
                got_frame = if choose_after {
                    Some((frame, pts_secs))
                } else {
                    before_frame.take()
                };
                break;
            }
            if frames_seen >= 240 {
                got_frame = before_frame.take();
                break;
            }
            frame = Video::empty();
        }
        if got_frame.is_some() {
            break;
        }
    }

    let (frame, decoded_pts_secs) = got_frame
        .or(before_frame)
        .ok_or_else(|| format!("frame not found near {target_secs:.3}s"))?;
    // HW (D3D11) frame は SW download してから scaler に渡す。
    let mut sw_holder: Option<Video> = None;
    let frame_for_scaler =
        crate::video::swscale_helpers::prepare_frame_for_swscale(&frame, &mut sw_holder)
            .map_err(|e| format!("screenshot: {e}"))?;
    let cur_src_fmt = frame_for_scaler.format();
    let mut scaler = ScaleContext::get(
        cur_src_fmt,
        src_w,
        src_h,
        Pixel::RGBA,
        src_w,
        src_h,
        ScaleFlags::BILINEAR,
    )
    .map_err(|e| format!("sws_scale init failed: {e}"))?;
    let mut rgba = Video::empty();
    scaler
        .run(frame_for_scaler, &mut rgba)
        .map_err(|e| format!("sws_scale failed: {e}"))?;
    let stride = rgba.stride(0);
    let needed = (src_w * 4) as usize;
    let plane = rgba.data(0);
    let buf = if stride == needed {
        plane[..needed * src_h as usize].to_vec()
    } else {
        let mut out = Vec::with_capacity(needed * src_h as usize);
        for row in 0..src_h as usize {
            let start = row * stride;
            out.extend_from_slice(&plane[start..start + needed]);
        }
        out
    };

    Ok(CapturedVideoFrame {
        target_secs: decoded_pts_secs,
        width: src_w,
        height: src_h,
        rgba: buf,
    })
}

fn timestamp_epsilon_secs(tb_num: f64, tb_den: f64) -> f64 {
    if tb_num.is_finite() && tb_den.is_finite() && tb_num > 0.0 && tb_den > 0.0 {
        (tb_num / tb_den * 0.5).clamp(1.0e-9, 1.0e-6)
    } else {
        1.0e-6
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn timestamp_epsilon_is_tiny_even_for_millisecond_timebase() {
        let eps = super::timestamp_epsilon_secs(1.0, 1000.0);
        assert!(eps <= 1.0e-6);
    }
}
