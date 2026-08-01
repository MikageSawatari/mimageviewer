use std::fmt;

use ffmpeg::format::sample::{Sample, Type as SampleType};
use ffmpeg::util::frame::audio::Audio as AudioFrame;
use ffmpeg_the_third as ffmpeg;

use super::encoder::{AUDIO_ENCODER_NAME, AUDIO_PROFILE, AUDIO_PROFILE_ID};
use super::timeline::{StreamTimeline, StreamTimelineError};
use crate::video::audio::ProcessedChunk;

const CHANNELS: usize = 2;
const AAC_FRAME_SAMPLES: usize = 1024;
const SWR_OUTPUT_SAFETY_SAMPLES: u64 = 32;
const FFMPEG_EAGAIN: i32 = 11;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AacEncoderError(String);

impl AacEncoderError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for AacEncoderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AacEncoderError {}

impl From<StreamTimelineError> for AacEncoderError {
    fn from(error: StreamTimelineError) -> Self {
        Self::new(error.to_string())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AacEncoderStats {
    pub(crate) stale_seek_chunks: u64,
    pub(crate) rejected_speed_chunks: u64,
    pub(crate) input_samples_per_channel: u64,
    pub(crate) output_samples_per_channel: u64,
    pub(crate) encoded_frames: u64,
}

struct PlanarPcmAssembler {
    left: Vec<f32>,
    right: Vec<f32>,
    offset: usize,
    next_pts: i64,
}

impl PlanarPcmAssembler {
    fn new(first_pts: i64) -> Self {
        Self {
            left: Vec::new(),
            right: Vec::new(),
            offset: 0,
            next_pts: first_pts,
        }
    }

    fn available(&self) -> usize {
        self.left.len().saturating_sub(self.offset)
    }

    fn append(&mut self, left: &[f32], right: &[f32]) -> Result<(), AacEncoderError> {
        if left.len() != right.len() {
            return Err(AacEncoderError::new("planar stereo channel lengths differ"));
        }
        self.left.extend_from_slice(left);
        self.right.extend_from_slice(right);
        Ok(())
    }

    fn pop(&mut self, samples: usize, pad: bool) -> Option<(i64, Vec<f32>, Vec<f32>, usize)> {
        let available = self.available();
        if available == 0 || (!pad && available < samples) {
            return None;
        }
        let valid = available.min(samples);
        let mut left = vec![0.0; samples];
        let mut right = vec![0.0; samples];
        left[..valid].copy_from_slice(&self.left[self.offset..self.offset + valid]);
        right[..valid].copy_from_slice(&self.right[self.offset..self.offset + valid]);
        self.offset += valid;
        let pts = self.next_pts;
        self.next_pts = self.next_pts.saturating_add(samples as i64);
        if self.offset == self.left.len() {
            self.left.clear();
            self.right.clear();
            self.offset = 0;
        } else if self.offset >= 8 * AAC_FRAME_SAMPLES {
            self.left.drain(..self.offset);
            self.right.drain(..self.offset);
            self.offset = 0;
        }
        Some((pts, left, right, valid))
    }
}

pub(crate) struct OpenedAacEncoder {
    pub(crate) encoder: ffmpeg::codec::encoder::audio::Encoder,
    input_sample_rate: u32,
    output_sample_rate: u32,
    effective_bitrate_bps: u64,
    expected_seek_serial: u64,
    timeline: StreamTimeline,
    resampler: Option<ffmpeg::software::resampling::Context>,
    assembler: Option<PlanarPcmAssembler>,
    expected_next_source_pts_secs: Option<f64>,
    stats: AacEncoderStats,
    finished: bool,
}

unsafe impl Send for OpenedAacEncoder {}

fn choose_aac_sample_rate(input_rate: u32, supported: &[u32]) -> Option<u32> {
    if input_rate == 0 {
        return None;
    }
    if supported.is_empty() || supported.contains(&input_rate) {
        return Some(input_rate);
    }
    supported
        .iter()
        .copied()
        .filter(|rate| *rate > 0)
        .min_by_key(|rate| {
            (
                rate.abs_diff(input_rate),
                u8::from(*rate != 48_000),
                rate.abs_diff(48_000),
            )
        })
}

pub(crate) fn open_aac_encoder(
    input_sample_rate: u32,
    audio_bitrate_bps: u32,
    expected_seek_serial: u64,
    timeline: StreamTimeline,
) -> Result<OpenedAacEncoder, AacEncoderError> {
    ffmpeg::init()
        .map_err(|error| AacEncoderError::new(format!("FFmpeg initialization failed: {error}")))?;
    let codec = ffmpeg::codec::encoder::find_by_name(AUDIO_ENCODER_NAME)
        .ok_or_else(|| AacEncoderError::new("AAC encoder was not found"))?;
    let audio_codec = codec
        .audio()
        .ok_or_else(|| AacEncoderError::new("configured AAC encoder is not an audio encoder"))?;
    let fltp = Sample::F32(SampleType::Planar);
    if audio_codec
        .formats()
        .is_some_and(|mut formats| !formats.any(|format| format == fltp))
    {
        return Err(AacEncoderError::new(
            "AAC encoder does not accept planar f32 input",
        ));
    }
    let supported_rates = audio_codec
        .rates()
        .map(|rates| {
            rates
                .filter_map(|rate| u32::try_from(rate).ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let output_sample_rate = choose_aac_sample_rate(input_sample_rate, &supported_rates)
        .ok_or_else(|| AacEncoderError::new("AAC sample rate selection failed"))?;

    let mut context = ffmpeg::codec::context::Context::new_with_codec(codec)
        .encoder()
        .audio()
        .map_err(|error| AacEncoderError::new(format!("AAC encoder context: {error}")))?;
    context.set_rate(output_sample_rate as i32);
    context.set_format(fltp);
    context.set_bit_rate(audio_bitrate_bps as usize);
    context.set_time_base((1, output_sample_rate as i32));
    context.set_ch_layout(ffmpeg::ChannelLayout::STEREO);
    unsafe {
        let raw = context.as_mut_ptr();
        (*raw).flags |= ffmpeg::ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
        (*raw).profile = AUDIO_PROFILE_ID;
    }
    let encoder = context
        .open_as(codec)
        .map_err(|error| AacEncoderError::new(format!("open {AUDIO_ENCODER_NAME}: {error}")))?;
    if encoder.frame_size() as usize != AAC_FRAME_SAMPLES {
        return Err(AacEncoderError::new(format!(
            "AAC encoder frame size is {}, expected {AAC_FRAME_SAMPLES}",
            encoder.frame_size()
        )));
    }
    let effective_bitrate_bps = unsafe { (*encoder.as_ptr()).bit_rate.max(0) as u64 };
    let resampler = if input_sample_rate == output_sample_rate {
        None
    } else {
        Some(
            ffmpeg::software::resampling::Context::get2(
                Sample::F32(SampleType::Packed),
                ffmpeg::ChannelLayout::STEREO,
                input_sample_rate,
                fltp,
                ffmpeg::ChannelLayout::STEREO,
                output_sample_rate,
            )
            .map_err(|error| AacEncoderError::new(format!("AAC resampler init: {error}")))?,
        )
    };
    crate::logger::log(format!(
        "remote-stream audio encoder: encoder={AUDIO_ENCODER_NAME} profile={AUDIO_PROFILE} input_rate={input_sample_rate} output_rate={output_sample_rate} bitrate_bps={effective_bitrate_bps} resample={}",
        input_sample_rate != output_sample_rate
    ));
    Ok(OpenedAacEncoder {
        encoder,
        input_sample_rate,
        output_sample_rate,
        effective_bitrate_bps,
        expected_seek_serial,
        timeline,
        resampler,
        assembler: None,
        expected_next_source_pts_secs: None,
        stats: AacEncoderStats::default(),
        finished: false,
    })
}

impl OpenedAacEncoder {
    #[cfg(test)]
    pub(crate) fn input_sample_rate(&self) -> u32 {
        self.input_sample_rate
    }

    pub(crate) fn output_sample_rate(&self) -> u32 {
        self.output_sample_rate
    }

    pub(crate) fn effective_bitrate_bps(&self) -> u64 {
        self.effective_bitrate_bps
    }

    #[cfg(test)]
    pub(crate) fn stats(&self) -> AacEncoderStats {
        self.stats
    }

    /// post-DSP interleaved stereo chunk を AAC packet 列へ進める。
    /// seek 世代違いは明示的に捨て、非等速は歪んだ音を送らないため error にする。
    pub(crate) fn push_chunk(
        &mut self,
        chunk: ProcessedChunk,
    ) -> Result<Vec<ffmpeg::Packet>, AacEncoderError> {
        if self.finished {
            return Err(AacEncoderError::new(
                "cannot push audio after encoder finish",
            ));
        }
        if chunk.seek_serial != self.expected_seek_serial {
            self.stats.stale_seek_chunks = self.stats.stale_seek_chunks.saturating_add(1);
            return Ok(Vec::new());
        }
        if !chunk.source_secs_per_output_sec.is_finite()
            || (chunk.source_secs_per_output_sec - 1.0).abs() > 1.0e-6
        {
            self.stats.rejected_speed_chunks = self.stats.rejected_speed_chunks.saturating_add(1);
            return Err(AacEncoderError::new(format!(
                "streaming audio requires 1.0x playback, got source_secs_per_output_sec={}",
                chunk.source_secs_per_output_sec
            )));
        }
        if chunk.samples.len() % CHANNELS != 0 {
            return Err(AacEncoderError::new(
                "processed audio chunk is not interleaved stereo",
            ));
        }
        if !chunk.audible_pts_secs.is_finite()
            || !chunk.duration_secs.is_finite()
            || !chunk.pdc_latency_secs_at_process.is_finite()
            || chunk.duration_secs < 0.0
        {
            return Err(AacEncoderError::new("processed audio metadata is invalid"));
        }
        let input_samples = chunk.samples.len() / CHANNELS;
        let measured_duration = input_samples as f64 / f64::from(self.input_sample_rate);
        if (measured_duration - chunk.duration_secs).abs() > 1.5 / f64::from(self.input_sample_rate)
        {
            return Err(AacEncoderError::new(
                "processed audio duration does not match its sample count",
            ));
        }
        if let Some(expected) = self.expected_next_source_pts_secs {
            let tolerance = 1.5 / f64::from(self.input_sample_rate);
            if (chunk.audible_pts_secs - expected).abs() > tolerance {
                return Err(AacEncoderError::new(format!(
                    "processed audio source timeline is discontinuous: expected {expected:.9}, got {:.9}",
                    chunk.audible_pts_secs
                )));
            }
        }
        if self.assembler.is_none() {
            let first_pts = self
                .timeline
                .relative_ticks(chunk.audible_pts_secs, self.output_sample_rate)?;
            self.assembler = Some(PlanarPcmAssembler::new(first_pts));
        }

        let (left, right) = if let Some(resampler) = self.resampler.as_mut() {
            resample_interleaved(
                resampler,
                &chunk.samples,
                self.input_sample_rate,
                self.output_sample_rate,
            )?
        } else {
            deinterleave(&chunk.samples)
        };
        self.stats.input_samples_per_channel = self
            .stats
            .input_samples_per_channel
            .saturating_add(input_samples as u64);
        self.stats.output_samples_per_channel = self
            .stats
            .output_samples_per_channel
            .saturating_add(left.len() as u64);
        self.assembler.as_mut().unwrap().append(&left, &right)?;
        self.expected_next_source_pts_secs = Some(chunk.audible_pts_secs + chunk.duration_secs);
        self.encode_available(false)
    }

    fn encode_available(&mut self, pad_tail: bool) -> Result<Vec<ffmpeg::Packet>, AacEncoderError> {
        let mut packets = Vec::new();
        loop {
            let Some((pts, left, right, _valid)) = self
                .assembler
                .as_mut()
                .and_then(|assembler| assembler.pop(AAC_FRAME_SAMPLES, pad_tail))
            else {
                break;
            };
            let mut frame = AudioFrame::new(
                Sample::F32(SampleType::Planar),
                AAC_FRAME_SAMPLES,
                ffmpeg::ChannelLayoutMask::STEREO,
            );
            frame.set_rate(self.output_sample_rate);
            frame.set_pts(Some(pts));
            frame.plane_mut::<f32>(0).copy_from_slice(&left);
            frame.plane_mut::<f32>(1).copy_from_slice(&right);
            self.encoder
                .send_frame(&frame)
                .map_err(|error| AacEncoderError::new(format!("AAC send_frame: {error}")))?;
            self.stats.encoded_frames = self.stats.encoded_frames.saturating_add(1);
            drain_aac_packets(&mut self.encoder, &mut packets)?;
        }
        Ok(packets)
    }
}

fn deinterleave(samples: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let frames = samples.len() / CHANNELS;
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    for stereo in samples.chunks_exact(CHANNELS) {
        left.push(stereo[0]);
        right.push(stereo[1]);
    }
    (left, right)
}

impl OpenedAacEncoder {
    /// swresample の delay と 1024-sample 未満の末尾を吐き、AAC encoder を drain する。
    #[cfg(test)]
    pub(crate) fn finish(&mut self) -> Result<Vec<ffmpeg::Packet>, AacEncoderError> {
        if self.finished {
            return Ok(Vec::new());
        }
        if let Some(resampler) = self.resampler.as_mut() {
            let (left, right) = flush_resampler(resampler, self.output_sample_rate)?;
            self.stats.output_samples_per_channel = self
                .stats
                .output_samples_per_channel
                .saturating_add(left.len() as u64);
            if !left.is_empty() {
                let assembler = self.assembler.as_mut().ok_or_else(|| {
                    AacEncoderError::new("resampler produced output without an audio timeline")
                })?;
                assembler.append(&left, &right)?;
            }
        }
        let mut packets = self.encode_available(true)?;
        self.encoder
            .send_eof()
            .map_err(|error| AacEncoderError::new(format!("AAC send_eof: {error}")))?;
        drain_aac_packets(&mut self.encoder, &mut packets)?;
        self.finished = true;
        Ok(packets)
    }
}

fn resample_interleaved(
    resampler: &mut ffmpeg::software::resampling::Context,
    samples: &[f32],
    input_rate: u32,
    output_rate: u32,
) -> Result<(Vec<f32>, Vec<f32>), AacEncoderError> {
    let input_samples = samples.len() / CHANNELS;
    if input_samples == 0 {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut input = AudioFrame::new(
        Sample::F32(SampleType::Packed),
        input_samples,
        ffmpeg::ChannelLayoutMask::STEREO,
    );
    input.set_rate(input_rate);
    for (output, stereo) in input
        .plane_mut::<(f32, f32)>(0)
        .iter_mut()
        .zip(samples.chunks_exact(CHANNELS))
    {
        *output = (stereo[0], stereo[1]);
    }
    let delay_output = resampler_delay_output(resampler, output_rate);
    let converted = (input_samples as u64 * u64::from(output_rate)).div_ceil(u64::from(input_rate));
    let output_capacity = (converted + delay_output + SWR_OUTPUT_SAFETY_SAMPLES) as usize;
    let mut output = AudioFrame::empty();
    unsafe {
        output.alloc(
            Sample::F32(SampleType::Planar),
            output_capacity,
            ffmpeg::ChannelLayoutMask::STEREO,
        );
    }
    output.set_rate(output_rate);
    resampler
        .run(&input, &mut output)
        .map_err(|error| AacEncoderError::new(format!("AAC resample: {error}")))?;
    Ok((
        output.plane::<f32>(0).to_vec(),
        output.plane::<f32>(1).to_vec(),
    ))
}

#[cfg(test)]
fn flush_resampler(
    resampler: &mut ffmpeg::software::resampling::Context,
    output_rate: u32,
) -> Result<(Vec<f32>, Vec<f32>), AacEncoderError> {
    let mut left = Vec::new();
    let mut right = Vec::new();
    loop {
        let delay_output = resampler_delay_output(resampler, output_rate);
        if delay_output == 0 {
            break;
        }
        let mut output = AudioFrame::empty();
        unsafe {
            output.alloc(
                Sample::F32(SampleType::Planar),
                (delay_output + SWR_OUTPUT_SAFETY_SAMPLES) as usize,
                ffmpeg::ChannelLayoutMask::STEREO,
            );
        }
        output.set_rate(output_rate);
        resampler
            .flush(&mut output)
            .map_err(|error| AacEncoderError::new(format!("AAC resampler flush: {error}")))?;
        if output.samples() == 0 {
            break;
        }
        left.extend_from_slice(output.plane::<f32>(0));
        right.extend_from_slice(output.plane::<f32>(1));
    }
    Ok((left, right))
}

fn resampler_delay_output(
    resampler: &ffmpeg::software::resampling::Context,
    output_rate: u32,
) -> u64 {
    // The wrapper's delay() first probes base=1 and returns None when that rounded value is
    // zero, even when sub-second filter delay still contains output samples. Query the output
    // sample base directly so EOF draining cannot lose that tail.
    unsafe {
        ffmpeg::ffi::swr_get_delay(resampler.as_ptr() as *mut _, i64::from(output_rate)).max(0)
            as u64
    }
}

fn drain_aac_packets(
    encoder: &mut ffmpeg::codec::encoder::audio::Encoder,
    packets: &mut Vec<ffmpeg::Packet>,
) -> Result<(), AacEncoderError> {
    loop {
        let mut packet = ffmpeg::Packet::empty();
        match encoder.receive_packet(&mut packet) {
            Ok(()) => packets.push(packet),
            Err(ffmpeg::Error::Other { errno }) if errno == FFMPEG_EAGAIN => return Ok(()),
            Err(ffmpeg::Error::Eof) => return Ok(()),
            Err(error) => {
                return Err(AacEncoderError::new(format!("AAC receive_packet: {error}")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(
        samples_per_channel: usize,
        sample_rate: u32,
        pts: f64,
        seek_serial: u64,
    ) -> ProcessedChunk {
        let mut samples = Vec::with_capacity(samples_per_channel * CHANNELS);
        for index in 0..samples_per_channel {
            samples.push(index as f32 / samples_per_channel.max(1) as f32);
            samples.push(-(index as f32) / samples_per_channel.max(1) as f32);
        }
        ProcessedChunk {
            samples,
            audible_pts_secs: pts,
            duration_secs: samples_per_channel as f64 / f64::from(sample_rate),
            source_secs_per_output_sec: 1.0,
            seek_serial,
            pdc_latency_secs_at_process: 0.0,
        }
    }

    #[test]
    fn assembler_carries_misaligned_chunk_boundaries_without_loss_or_duplication() {
        let total = 3 * AAC_FRAME_SAMPLES + 137;
        let source_left = (0..total).map(|value| value as f32).collect::<Vec<_>>();
        let source_right = source_left.iter().map(|value| -*value).collect::<Vec<_>>();
        let mut assembler = PlanarPcmAssembler::new(17);
        let mut reconstructed_left = Vec::new();
        let mut reconstructed_right = Vec::new();
        let mut observed_pts = Vec::new();
        let mut start = 0;
        for length in [333, 901, 17, total - 1_251] {
            let end = start + length;
            assembler
                .append(&source_left[start..end], &source_right[start..end])
                .unwrap();
            while let Some((pts, left, right, valid)) = assembler.pop(AAC_FRAME_SAMPLES, false) {
                observed_pts.push(pts);
                reconstructed_left.extend_from_slice(&left[..valid]);
                reconstructed_right.extend_from_slice(&right[..valid]);
            }
            start = end;
        }
        let (pts, left, right, valid) = assembler.pop(AAC_FRAME_SAMPLES, true).unwrap();
        observed_pts.push(pts);
        reconstructed_left.extend_from_slice(&left[..valid]);
        reconstructed_right.extend_from_slice(&right[..valid]);

        assert_eq!(reconstructed_left, source_left);
        assert_eq!(reconstructed_right, source_right);
        assert_eq!(observed_pts, vec![17, 1_041, 2_065, 3_089]);
    }

    #[test]
    fn sample_rate_selection_resamples_only_when_input_is_unsupported() {
        assert_eq!(
            choose_aac_sample_rate(48_000, &[44_100, 48_000]),
            Some(48_000)
        );
        assert_eq!(
            choose_aac_sample_rate(50_000, &[44_100, 48_000]),
            Some(48_000)
        );
        assert_eq!(choose_aac_sample_rate(96_000, &[]), Some(96_000));
        assert_eq!(choose_aac_sample_rate(0, &[48_000]), None);
    }

    #[test]
    fn unsupported_input_rate_is_resampled_and_drained_into_aac_frames() {
        let timeline = StreamTimeline::new(0.0).unwrap();
        let mut encoder = open_aac_encoder(50_000, 96_000, 4, timeline).unwrap();
        assert_eq!(encoder.input_sample_rate(), 50_000);
        assert_eq!(encoder.output_sample_rate(), 48_000);
        let mut packets = encoder.push_chunk(chunk(50_000, 50_000, 0.0, 4)).unwrap();
        packets.extend(encoder.finish().unwrap());
        assert!(!packets.is_empty());
        assert_eq!(encoder.stats().input_samples_per_channel, 50_000);
        assert_eq!(encoder.stats().output_samples_per_channel, 48_000);
    }

    #[test]
    fn changed_seek_serial_chunk_is_discarded_before_pcm_assembly() {
        let timeline = StreamTimeline::new(5.0).unwrap();
        let mut encoder = open_aac_encoder(48_000, 96_000, 11, timeline).unwrap();
        let packets = encoder.push_chunk(chunk(1_000, 48_000, 5.0, 12)).unwrap();
        assert!(packets.is_empty());
        assert!(encoder.assembler.is_none());
        assert_eq!(encoder.stats().stale_seek_chunks, 1);
        assert_eq!(encoder.stats().input_samples_per_channel, 0);
    }

    #[test]
    fn non_unity_speed_is_rejected_instead_of_encoded_with_wrong_timing() {
        let timeline = StreamTimeline::new(0.0).unwrap();
        let mut encoder = open_aac_encoder(48_000, 96_000, 3, timeline).unwrap();
        let mut input = chunk(1_024, 48_000, 0.0, 3);
        input.source_secs_per_output_sec = 1.25;
        let error = match encoder.push_chunk(input) {
            Ok(_) => panic!("non-unity audio unexpectedly encoded"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("requires 1.0x playback"));
        assert_eq!(encoder.stats().rejected_speed_chunks, 1);
        assert!(encoder.assembler.is_none());
    }
}
