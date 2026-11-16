//! Timestamp seekers in TS/PS do not promise a key packet. Verify a bounded
//! window and replay exactly those packets; never seek again after verification.
//! All callers run on their own demux/auxiliary worker, never the UI thread.

use std::collections::VecDeque;
use std::time::Instant;

use ffmpeg_the_third::{self as ffmpeg, Packet, format::context::Input};

const MAX_HELD_BYTES: usize = 64 * 1024 * 1024;
const MAX_READ_BYTES: u64 = 128 * 1024 * 1024;
const BACKOFF_SECONDS: [u32; 6] = [1, 2, 4, 8, 16, 32];

pub(crate) fn needs_verification(demuxer: &str) -> bool {
    // Deliberately separate from MediaTimeline's duration/origin policy: this
    // asks whether timestamp seek can land on a non-key video packet.
    matches!(demuxer, "mpegts" | "mpegtsraw" | "mpeg")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SeekOutcome {
    Direct,
    Verified { back: u32 },
    StartOfStream,
    Unverified,
    Aborted,
}

#[derive(Clone, Copy)]
struct Budget {
    held_bytes: usize,
    read_bytes: u64,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            held_bytes: MAX_HELD_BYTES,
            read_bytes: MAX_READ_BYTES,
        }
    }
}

#[derive(Default, Debug)]
struct SeekMetrics {
    attempts: u32,
    packets: u64,
    packet_bytes: u64,
    bytes_read: u64,
    held_bytes: usize,
    back: u32,
    elapsed_ms: f64,
}

/// One request's packet owner. A new seek or cancellation discards the old
/// window. Packet is moved, preserving payload, timestamps, flags and side data.
#[derive(Default)]
pub(crate) struct PacketReader {
    held: VecDeque<Packet>,
    metrics: SeekMetrics,
}

impl PacketReader {
    #[cfg(test)]
    pub(super) fn from_held_for_test(held: VecDeque<Packet>) -> Self {
        Self {
            held,
            metrics: SeekMetrics::default(),
        }
    }
    pub(crate) fn clear(&mut self) {
        self.held.clear();
    }

    pub(crate) fn next(
        &mut self,
        input: &mut Input,
    ) -> Option<Result<(usize, Packet), ffmpeg::Error>> {
        if let Some(packet) = self.held.pop_front() {
            return Some(Ok((packet.stream(), packet)));
        }
        let mut packet = Packet::empty();
        match packet.read(input) {
            Ok(()) => Some(Ok((packet.stream(), packet))),
            Err(ffmpeg::Error::Eof) => None,
            Err(error) => Some(Err(error)),
        }
    }

    /// Playback must expose all verified video packets to its existing audio
    /// backpressure drain BEFORE sending any held audio. Per-stream order stays
    /// intact; the caller queues these non-blockingly after accepting Flush.
    pub(crate) fn take_held_video(&mut self, video_stream: usize, mut enqueue: impl FnMut(Packet)) {
        let mut other = VecDeque::new();
        for packet in self.held.drain(..) {
            if packet.stream() == video_stream {
                enqueue(packet);
            } else {
                other.push_back(packet);
            }
        }
        self.held = other;
    }

    pub(crate) fn seek(
        &mut self,
        input: &mut Input,
        original_stream: i32,
        locator: i64,
        video_stream: Option<usize>,
        cancelled: impl FnMut() -> bool,
    ) -> Result<SeekOutcome, ffmpeg::Error> {
        self.seek_with_budget(
            input,
            original_stream,
            locator,
            video_stream,
            cancelled,
            Budget::default(),
        )
    }

    fn seek_with_budget(
        &mut self,
        input: &mut Input,
        original_stream: i32,
        locator: i64,
        video_stream: Option<usize>,
        mut cancelled: impl FnMut() -> bool,
        budget: Budget,
    ) -> Result<SeekOutcome, ffmpeg::Error> {
        self.clear();
        self.metrics = SeekMetrics::default();
        let started = Instant::now();
        let initial_bytes = bytes_read(input);
        let result = self.seek_inner(
            input,
            original_stream,
            locator,
            video_stream,
            &mut cancelled,
            budget,
            initial_bytes,
        );
        self.metrics.bytes_read = bytes_read(input).saturating_sub(initial_bytes);
        self.metrics.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        if !matches!(result, Ok(SeekOutcome::Verified { .. })) {
            self.clear();
        }
        let outcome = match result {
            Ok(SeekOutcome::Direct) => "Direct",
            Ok(SeekOutcome::Verified { .. }) => "Verified",
            Ok(SeekOutcome::StartOfStream) => "StartOfStream",
            Ok(SeekOutcome::Unverified) => "Unverified",
            Ok(SeekOutcome::Aborted) => "Aborted",
            Err(_) => "SeekFailed",
        };
        let m = &self.metrics;
        crate::logger::log(format!(
            "keyframe_seek: outcome={outcome} stream={original_stream} locator={locator} back={} attempts={} packets={} packet_bytes={} bytes_read={} held_bytes={} ms={:.3}",
            m.back, m.attempts, m.packets, m.packet_bytes, m.bytes_read, m.held_bytes, m.elapsed_ms,
        ));
        crate::perf::event(
            "video",
            "keyframe_seek",
            None,
            0,
            &[
                ("outcome", outcome.into()),
                ("stream", original_stream.into()),
                ("locator", locator.into()),
                ("back", m.back.into()),
                ("attempts", m.attempts.into()),
                ("packets", m.packets.into()),
                ("packet_bytes", m.packet_bytes.into()),
                ("bytes_read", m.bytes_read.into()),
                ("held_bytes", m.held_bytes.into()),
                ("ms", m.elapsed_ms.into()),
            ],
        );
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn seek_inner(
        &mut self,
        input: &mut Input,
        original_stream: i32,
        locator: i64,
        video_stream: Option<usize>,
        cancelled: &mut impl FnMut() -> bool,
        budget: Budget,
        initial_bytes: u64,
    ) -> Result<SeekOutcome, ffmpeg::Error> {
        if cancelled() {
            return Ok(SeekOutcome::Aborted);
        }
        let subject = needs_verification(input.format().name());
        let verification = video_stream.filter(|_| subject).and_then(|index| {
            let stream = input.stream(index)?;
            if stream.parameters().medium() != ffmpeg::media::Type::Video {
                return None;
            }
            let tb = stream.time_base();
            let source_tb = if original_stream == -1 {
                ffmpeg::ffi::AV_TIME_BASE_Q
            } else {
                input
                    .stream(usize::try_from(original_stream).ok()?)?
                    .time_base()
                    .into()
            };
            if tb.numerator() <= 0
                || tb.denominator() <= 0
                || source_tb.num <= 0
                || source_tb.den <= 0
            {
                return None;
            }
            // SAFETY: only integer rescaling of valid time bases; no source
            // origin is interpreted here. MediaTimeline owns that boundary.
            let target = unsafe { ffmpeg::ffi::av_rescale_q(locator, source_tb, tb.into()) };
            let mut start = stream.start_time();
            if start == ffmpeg::ffi::AV_NOPTS_VALUE {
                // SAFETY: context belongs to this borrowed Input.
                let format_start = unsafe { (*input.as_ptr()).start_time };
                if format_start == ffmpeg::ffi::AV_NOPTS_VALUE {
                    return None;
                }
                start = unsafe {
                    ffmpeg::ffi::av_rescale_q(format_start, ffmpeg::ffi::AV_TIME_BASE_Q, tb.into())
                };
            }
            let mpeg12 = matches!(
                stream.parameters().id(),
                ffmpeg::codec::Id::MPEG1VIDEO | ffmpeg::codec::Id::MPEG2VIDEO
            );
            Some((index, tb, target, start, mpeg12))
        });
        let Some((video_stream, time_base, target, stream_start, mpeg12)) = verification else {
            let result = self.seek_once(
                input,
                original_stream,
                locator,
                ffmpeg::ffi::AVSEEK_FLAG_BACKWARD as i32,
            );
            if cancelled() {
                return Ok(SeekOutcome::Aborted);
            }
            result?;
            return Ok(SeekOutcome::Direct);
        };

        for back in BACKOFF_SECONDS {
            self.metrics.back = back;
            if cancelled() {
                return Ok(SeekOutcome::Aborted);
            }
            let delta = unsafe {
                ffmpeg::ffi::av_rescale_q(
                    i64::from(back),
                    ffmpeg::ffi::AVRational { num: 1, den: 1 },
                    time_base.into(),
                )
            };
            let probe = target.saturating_sub(delta);
            if target <= stream_start || probe <= stream_start {
                // The only byte seek: resynchronize from file head, including
                // recordings which begin partway through a GOP. start_time is
                // PTS and must not be used as a head locator.
                let result = self.seek_once(input, -1, 0, ffmpeg::ffi::AVSEEK_FLAG_BYTE as i32);
                if cancelled() {
                    return Ok(SeekOutcome::Aborted);
                }
                if bytes_read(input).saturating_sub(initial_bytes) > budget.read_bytes {
                    break;
                }
                result?;
                return Ok(SeekOutcome::StartOfStream);
            }
            let seek = self.seek_once(
                input,
                video_stream as i32,
                probe,
                ffmpeg::ffi::AVSEEK_FLAG_BACKWARD as i32,
            );
            if cancelled() {
                return Ok(SeekOutcome::Aborted);
            }
            if bytes_read(input).saturating_sub(initial_bytes) > budget.read_bytes {
                break;
            }
            if let Err(error) = seek {
                crate::logger::log(format!("keyframe_seek: back={back} seek failed: {error}"));
                continue;
            }
            self.clear();
            self.metrics.held_bytes = 0;
            let mut keys = WindowKeys::default();
            loop {
                let mut packet = Packet::empty();
                let read = packet.read(input);
                if read.is_ok() {
                    self.metrics.packets += 1;
                    self.metrics.packet_bytes = self
                        .metrics
                        .packet_bytes
                        .saturating_add(packet.size() as u64);
                }
                if cancelled() {
                    return Ok(SeekOutcome::Aborted);
                }
                if bytes_read(input).saturating_sub(initial_bytes) > budget.read_bytes {
                    return self.fallback(input, original_stream, locator, cancelled);
                }
                if let Err(error) = read {
                    if error != ffmpeg::Error::Eof {
                        crate::logger::log(format!(
                            "keyframe_seek: back={back} read ended window: {error}"
                        ));
                    }
                    break; // EOF/read error ends this window; no recovery flow.
                }
                self.metrics.held_bytes = self.metrics.held_bytes.saturating_add(packet.size());
                if self.metrics.held_bytes > budget.held_bytes {
                    return self.fallback(input, original_stream, locator, cancelled);
                }
                let is_video = packet.stream() == video_stream;
                let beyond = is_video && packet.dts().is_some_and(|dts| dts > target);
                if is_video {
                    keys.observe(packet.is_key(), packet.pts(), packet.dts(), target);
                }
                self.held.push_back(packet); // includes the terminating packet
                if beyond {
                    break;
                }
            }
            if keys.verified(mpeg12) {
                return Ok(SeekOutcome::Verified { back });
            }
            // Release the failed window before the next FFmpeg seek, rather
            // than retaining it during that seek's own internal reads.
            self.clear();
            self.metrics.held_bytes = 0;
        }
        self.fallback(input, original_stream, locator, cancelled)
    }

    fn seek_once(
        &mut self,
        input: &mut Input,
        stream: i32,
        locator: i64,
        flags: i32,
    ) -> Result<(), ffmpeg::Error> {
        // av_seek_frame does not bounds-check a positive stream index before
        // dereferencing it. Keep this safe Rust entry point safe for bad args.
        if stream >= 0 && input.stream(stream as usize).is_none() {
            return Err(ffmpeg::Error::from(-22)); // AVERROR(EINVAL)
        }
        self.metrics.attempts += 1;
        // SAFETY: exclusive, open Input; locator is in the specified stream's
        // raw time base (or AV_TIME_BASE for -1, or byte zero for head).
        let result =
            unsafe { ffmpeg::ffi::av_seek_frame(input.as_mut_ptr(), stream, locator, flags) };
        if result >= 0 {
            Ok(())
        } else {
            Err(ffmpeg::Error::from(result))
        }
    }

    fn fallback(
        &mut self,
        input: &mut Input,
        stream: i32,
        locator: i64,
        cancelled: &mut impl FnMut() -> bool,
    ) -> Result<SeekOutcome, ffmpeg::Error> {
        self.clear();
        if cancelled() {
            return Ok(SeekOutcome::Aborted);
        }
        // Preserve the caller's ORIGINAL stream and units, particularly -1
        // microseconds for a 90 kHz stream with a large timeline origin.
        let result = self.seek_once(
            input,
            stream,
            locator,
            ffmpeg::ffi::AVSEEK_FLAG_BACKWARD as i32,
        );
        if cancelled() {
            return Ok(SeekOutcome::Aborted);
        }
        result?;
        Ok(SeekOutcome::Unverified)
    }
}

fn bytes_read(input: &Input) -> u64 {
    // SAFETY: Input owns the context and AVIO for this entire worker call.
    unsafe {
        let pb = (*input.as_ptr()).pb;
        if pb.is_null() {
            0
        } else {
            (*pb).bytes_read.max(0) as u64
        }
    }
}

#[derive(Default)]
struct WindowKeys {
    count: usize,
    known_pts: bool,
    at_or_before: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_uses_only_keys_in_the_dts_window_and_formula_a_or_b() {
        let mut keys = WindowKeys::default();
        keys.observe(false, Some(0), Some(0), 100);
        keys.observe(true, Some(0), None, 100);
        keys.observe(true, Some(0), Some(101), 100);
        assert!(!keys.verified(true));
        keys.observe(true, None, Some(10), 100);
        assert!(!keys.verified(true));
        keys.observe(true, None, Some(99), 100);
        assert!(keys.verified(true)); // B only for MPEG-1/2, never H264/HEVC
        assert!(!keys.verified(false));
        keys.observe(true, Some(150), Some(100), 100);
        assert!(!keys.verified(true)); // known future PTS vetoes B
        keys.observe(true, Some(100), Some(100), 100);
        assert!(keys.verified(false)); // A, including the exact target
    }

    #[test]
    fn short_gop_boundary_key_is_held_but_not_counted_in_j() {
        let mut keys = WindowKeys::default();
        keys.observe(true, None, Some(90), 100);
        keys.observe(true, None, Some(101), 100);
        assert_eq!(keys.count, 1);
        assert!(!keys.verified(true));
    }

    #[test]
    fn demuxer_policy_is_independent_of_extension_and_timeline() {
        for name in ["mpegts", "mpegtsraw", "mpeg"] {
            assert!(needs_verification(name));
        }
        for name in [
            "ogg",
            "nut",
            "matroska,webm",
            "mov,mp4,m4a,3gp,3g2,mj2",
            "mpg",
            "mp3",
        ] {
            assert!(!needs_verification(name));
        }
    }
}

#[cfg(test)]
#[path = "keyframe_seek_tests.rs"]
mod material_tests;

impl WindowKeys {
    fn observe(&mut self, key: bool, pts: Option<i64>, dts: Option<i64>, target: i64) {
        if !key || !dts.is_some_and(|dts| dts <= target) {
            return;
        }
        self.count += 1;
        if let Some(pts) = pts {
            self.known_pts = true;
            self.at_or_before |= pts <= target;
        }
    }

    fn verified(&self, mpeg12: bool) -> bool {
        // A OR B (§2). B follows MPEG-1/2 anchor reordering (H.222.0
        // §2.4.3.7), including low_delay; never infer reorder from video_delay.
        // A known future PTS vetoes B even if many keys were observed.
        self.at_or_before || (mpeg12 && !self.known_pts && self.count >= 2)
    }
}
