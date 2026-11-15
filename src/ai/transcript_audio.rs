//! Worker-only streaming extraction. No audio device, GPU, or temporary PCM file.
//! A synchronous sink provides pipe backpressure; killing its worker unblocks writes.

use std::ffi::{CString, c_void};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ffmpeg::util::frame::Audio;
use ffmpeg_the_third as ffmpeg;

use crate::video::audio_track_selection::{SavedAudioTrackChoice, resolve_initial_audio_track};

pub const SAMPLE_RATE: u32 = 16_000;
pub const MAX_PCM_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
pub enum AudioError {
    Cancelled,
    Ffmpeg(String),
    Sink(String),
}
impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "transcript audio: {self:?}")
    }
}
impl std::error::Error for AudioError {}
impl From<ffmpeg::Error> for AudioError {
    fn from(error: ffmpeg::Error) -> Self {
        Self::Ffmpeg(error.to_string())
    }
}

pub struct ExtractionStats {
    pub audio_duration_ms: u64,
    pub pcm_samples: u64,
    /// Includes sink backpressure. Extract-only benchmarks use a no-op sink.
    pub elapsed: Duration,
}

pub struct PreparedAudio {
    // Input must be dropped before cancel: its interrupt callback borrows the Arc allocation.
    input: ffmpeg::format::context::Input,
    cancel: Arc<AtomicBool>,
    decoder: ffmpeg::decoder::Audio,
    time_base: ffmpeg::Rational,
    timeline: crate::video::media_timeline::MediaTimeline,
    pub stream_index: usize,
    /// Playback/container duration (zero if unknown); actual PCM length is in stats.
    pub duration_secs: f64,
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), AudioError> {
    if cancel.load(Ordering::Acquire) {
        Err(AudioError::Cancelled)
    } else {
        Ok(())
    }
}

extern "C" fn interrupt(opaque: *mut c_void) -> i32 {
    // Arc::as_ptr remains live throughout open, probe, decode and Input::drop.
    i32::from(unsafe { &*(opaque.cast::<AtomicBool>()) }.load(Ordering::Acquire))
}

fn open_input(
    path: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<ffmpeg::format::context::Input, AudioError> {
    check_cancel(cancel)?;
    // Only real files are in scope. Do not let FFmpeg interpret arbitrary URL schemes.
    if !path.is_file() {
        return Err(AudioError::Ffmpeg("source is not a regular file".into()));
    }
    let name = CString::new(path.to_string_lossy().as_bytes())
        .map_err(|_| AudioError::Ffmpeg("source path contains NUL".into()))?;
    unsafe {
        let mut raw = ffmpeg::ffi::avformat_alloc_context();
        if raw.is_null() {
            return Err(AudioError::Ffmpeg("avformat allocation failed".into()));
        }
        (*raw).interrupt_callback = ffmpeg::ffi::AVIOInterruptCB {
            callback: Some(interrupt),
            opaque: Arc::as_ptr(cancel).cast_mut().cast(),
        };
        let opened = ffmpeg::ffi::avformat_open_input(
            &mut raw,
            name.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
        );
        if opened < 0 {
            // avformat_open_input frees its context on failure.
            check_cancel(cancel)?;
            return Err(ffmpeg::Error::from(opened).into());
        }
        let probed = ffmpeg::ffi::avformat_find_stream_info(raw, std::ptr::null_mut());
        if probed < 0 {
            ffmpeg::ffi::avformat_close_input(&mut raw);
            check_cancel(cancel)?;
            return Err(ffmpeg::Error::from(probed).into());
        }
        Ok(ffmpeg::format::context::Input::wrap(raw))
    }
}

/// Worker-only entry. The accepted job supplies the same current in-memory
/// choice snapshot that playback uses; a delayed settings flush cannot change it.
pub fn prepare_with_saved(
    path: &Path,
    saved: Option<&SavedAudioTrackChoice>,
    cancel: Arc<AtomicBool>,
) -> Result<Option<PreparedAudio>, AudioError> {
    crate::audio_decode::ensure_ffmpeg_init();
    let mut input = open_input(path, &cancel)?;
    check_cancel(&cancel)?;
    let timeline = crate::video::media_timeline::MediaTimeline::from_input(&input);
    let tracks = crate::video::decoder::enumerate_audio_tracks_with_timeline(&input, timeline);
    let default = input
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .map(|s| s.index());
    let Some(stream_index) = resolve_initial_audio_track(&tracks, default, saved) else {
        return Ok(None);
    };
    let time_base = input
        .stream(stream_index)
        .ok_or_else(|| AudioError::Ffmpeg("selected stream missing".into()))?
        .time_base();
    let duration_secs = input.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);
    let duration_secs = if duration_secs.is_finite() && duration_secs > 0.0 {
        duration_secs
    } else {
        0.0
    };
    let decoder = crate::audio_decode::open_audio_stream_decoder(&input, stream_index)
        .map_err(AudioError::Ffmpeg)?;
    crate::audio_decode::discard_unselected_streams(&mut input, stream_index);
    Ok(Some(PreparedAudio {
        input,
        cancel,
        decoder,
        time_base,
        timeline,
        stream_index,
        duration_secs,
    }))
}

fn make_graph(
    frame: &Audio,
    time_base: ffmpeg::Rational,
) -> Result<ffmpeg::filter::Graph, AudioError> {
    let mut graph = ffmpeg::filter::Graph::new();
    let source = ffmpeg::filter::find("abuffer")
        .ok_or_else(|| AudioError::Ffmpeg("abuffer unavailable".into()))?;
    let sink = ffmpeg::filter::find("abuffersink")
        .ok_or_else(|| AudioError::Ffmpeg("abuffersink unavailable".into()))?;
    let layout = crate::audio_decode::normalize_layout(frame.ch_layout());
    let args = format!(
        "time_base={}/{}:sample_rate={}:sample_fmt={}:channel_layout={}",
        time_base.numerator(),
        time_base.denominator(),
        frame.rate(),
        frame.format().name(),
        layout.description()
    );
    graph.add(&source, "in", &args)?;
    graph.add(&sink, "out", "")?;
    // メディア時刻0から埋め、0より前のsampleは捨てる。
    graph.output("in", 0)?.input("out", 0)?.parse(
        "aresample=16000:async=1:first_pts=0,aformat=sample_fmts=s16:channel_layouts=mono",
    )?;
    graph.validate()?;
    Ok(graph)
}

struct PcmSink<'a, F> {
    callback: F,
    cancel: &'a AtomicBool,
    bytes: Vec<u8>,
    samples: u64,
}
impl<F: FnMut(&[u8]) -> Result<(), AudioError>> PcmSink<'_, F> {
    fn append(&mut self, data: &[u8]) -> Result<(), AudioError> {
        for chunk in data.chunks(MAX_PCM_BYTES) {
            check_cancel(self.cancel)?;
            let available = MAX_PCM_BYTES - self.bytes.len();
            let split = available.min(chunk.len());
            self.bytes.extend_from_slice(&chunk[..split]);
            if self.bytes.len() == MAX_PCM_BYTES {
                self.flush()?;
            }
            self.bytes.extend_from_slice(&chunk[split..]);
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<(), AudioError> {
        check_cancel(self.cancel)?;
        if !self.bytes.is_empty() {
            (self.callback)(&self.bytes)?;
            self.samples += (self.bytes.len() / 2) as u64;
            self.bytes.clear();
        }
        check_cancel(self.cancel)
    }
}

fn pull_pcm<F: FnMut(&[u8]) -> Result<(), AudioError>>(
    graph: &mut ffmpeg::filter::Graph,
    sink: &mut PcmSink<'_, F>,
) -> Result<(), AudioError> {
    loop {
        check_cancel(sink.cancel)?;
        let mut frame = Audio::empty();
        match graph.get("out").unwrap().sink().frame(&mut frame) {
            Ok(()) => {
                let len = frame
                    .samples()
                    .checked_mul(2)
                    .ok_or_else(|| AudioError::Ffmpeg("PCM size overflow".into()))?;
                if len > 0 {
                    // FFmpeg's linesize includes padding: read only samples * mono * s16.
                    let data =
                        unsafe { std::slice::from_raw_parts((*frame.as_ptr()).data[0], len) };
                    sink.append(data)?;
                }
            }
            Err(ffmpeg::Error::Eof) | Err(ffmpeg::Error::Other { errno: 11 }) => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    }
}

fn receive_decoded<F: FnMut(&[u8]) -> Result<(), AudioError>>(
    decoder: &mut ffmpeg::decoder::Audio,
    graph: &mut Option<ffmpeg::filter::Graph>,
    time_base: ffmpeg::Rational,
    timeline: crate::video::media_timeline::MediaTimeline,
    sink: &mut PcmSink<'_, F>,
) -> Result<(), AudioError> {
    loop {
        check_cancel(sink.cancel)?;
        let mut frame = Audio::empty();
        match decoder.receive_frame(&mut frame) {
            Ok(()) => {
                let layout = crate::audio_decode::normalize_layout(frame.ch_layout());
                frame.set_ch_layout(layout);
                // 再生と同じメディア時刻。選択したトラックの開始へ原点を付け替えない。
                let pts = frame
                    .timestamp()
                    .or_else(|| frame.pts())
                    .map(|pts| {
                        timeline
                            .raw_stream_ticks_to_media(pts, time_base)
                            .ok_or_else(|| AudioError::Ffmpeg("timestamp overflow".into()))
                    })
                    .transpose()?;
                frame.set_pts(pts);
                if graph.is_none() {
                    *graph = Some(make_graph(&frame, time_base)?);
                }
                let graph = graph.as_mut().unwrap();
                graph.get("in").unwrap().source().add(&frame)?;
                pull_pcm(graph, sink)?;
            }
            Err(ffmpeg::Error::Eof) | Err(ffmpeg::Error::Other { errno: 11 }) => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    }
}

impl PreparedAudio {
    pub fn extract(
        mut self,
        cancel: &AtomicBool,
        on_pcm: impl FnMut(&[u8]) -> Result<(), AudioError>,
    ) -> Result<ExtractionStats, AudioError> {
        let started = Instant::now();
        let mut sink = PcmSink {
            callback: on_pcm,
            cancel,
            bytes: Vec::with_capacity(MAX_PCM_BYTES),
            samples: 0,
        };
        let mut graph = None;
        let result: Result<(), AudioError> = (|| {
            loop {
                check_cancel(cancel)?;
                check_cancel(&self.cancel)?;
                let mut packet = ffmpeg::Packet::empty();
                match packet.read(&mut self.input) {
                    Ok(()) => {}
                    Err(ffmpeg::Error::Eof) => break,
                    Err(e) => {
                        check_cancel(cancel)?;
                        return Err(e.into());
                    }
                }
                if packet.stream() != self.stream_index {
                    continue;
                }
                self.decoder.send_packet(&packet)?;
                receive_decoded(
                    &mut self.decoder,
                    &mut graph,
                    self.time_base,
                    self.timeline,
                    &mut sink,
                )?;
            }
            self.decoder.send_eof()?;
            receive_decoded(
                &mut self.decoder,
                &mut graph,
                self.time_base,
                self.timeline,
                &mut sink,
            )?;
            if let Some(graph) = graph.as_mut() {
                graph.get("in").unwrap().source().flush()?;
                pull_pcm(graph, &mut sink)?;
            }
            sink.flush()?;
            Ok(())
        })();
        let elapsed = started.elapsed();
        if crate::perf::is_enabled() {
            crate::perf::event(
                "transcript",
                "extract",
                None,
                0,
                &[
                    (
                        "audio_duration_ms",
                        serde_json::json!(sink.samples * 1000 / SAMPLE_RATE as u64),
                    ),
                    ("pcm_samples", serde_json::json!(sink.samples)),
                    ("elapsed_s", serde_json::json!(elapsed.as_secs_f64())),
                    ("elapsed_ms", serde_json::json!(elapsed.as_millis() as u64)),
                    ("ok", serde_json::json!(result.is_ok())),
                ],
            );
        }
        result?;
        Ok(ExtractionStats {
            audio_duration_ms: sink.samples * 1000 / SAMPLE_RATE as u64,
            pcm_samples: sink.samples,
            elapsed,
        })
    }
}

#[cfg(test)]
mod tests;
