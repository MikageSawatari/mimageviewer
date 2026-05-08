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
    let frame_tolerance_secs = frame_tolerance_secs(video_stream.avg_frame_rate());
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
    let src_fmt = decoder.format();
    let mut scaler = ScaleContext::get(
        src_fmt,
        src_w,
        src_h,
        Pixel::RGBA,
        src_w,
        src_h,
        ScaleFlags::BILINEAR,
    )
    .map_err(|e| format!("sws_scale init failed: {e}"))?;

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

    let mut got_frame: Option<Video> = None;
    let mut fallback_frame: Option<Video> = None;
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
            let pts_secs = frame.pts().unwrap_or(0) as f64 * tb_num / tb_den;
            if pts_secs <= target_secs + frame_tolerance_secs {
                fallback_frame = Some(frame);
            } else if fallback_frame.is_none() {
                fallback_frame = Some(frame);
            }
            if pts_secs >= target_secs - frame_tolerance_secs || frames_seen >= 240 {
                got_frame = fallback_frame.take();
                break;
            }
            frame = Video::empty();
        }
        if got_frame.is_some() {
            break;
        }
    }

    let frame = got_frame
        .or(fallback_frame)
        .ok_or_else(|| format!("frame not found near {target_secs:.3}s"))?;
    let mut rgba = Video::empty();
    scaler
        .run(&frame, &mut rgba)
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
        target_secs,
        width: src_w,
        height: src_h,
        rgba: buf,
    })
}

fn frame_tolerance_secs(rate: ffmpeg_the_third::Rational) -> f64 {
    let num = rate.numerator();
    let den = rate.denominator();
    if num > 0 && den > 0 {
        (den as f64 / num as f64 * 0.55).clamp(0.001, 0.050)
    } else {
        0.020
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn frame_tolerance_falls_back_for_unknown_rate() {
        let tolerance = super::frame_tolerance_secs(ffmpeg_the_third::Rational(0, 1));
        assert!((tolerance - 0.020).abs() < 1.0e-9);
    }

    #[test]
    fn frame_tolerance_uses_about_half_frame() {
        let tolerance = super::frame_tolerance_secs(ffmpeg_the_third::Rational(30000, 1001));
        assert!(tolerance > 0.018);
        assert!(tolerance < 0.019);
    }
}
