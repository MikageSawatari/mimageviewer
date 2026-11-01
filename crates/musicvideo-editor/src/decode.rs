use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use mimageviewer::audio_decode::decode_audio_file_to_stereo_f32;
use mimageviewer::video::mvexport::default_audio_stream_index;

pub const OUTPUT_SAMPLE_RATE: u32 = 48_000;

pub use mimageviewer::video::mvexport::ExportPcm as AudioData;

/// 音声読込・Project 読込の worker と診断で共有する FFmpeg 経路。
/// 書き出しと同じ既定トラックと swresample を使い、PCM の識別は呼出側に保つ。
pub fn decode_audio(path: &Path, cancel: &AtomicBool) -> Result<AudioData, String> {
    let stream_index = default_audio_stream_index(path).map_err(|error| error.to_string())?;
    let decoded = decode_audio_file_to_stereo_f32(path, stream_index, cancel)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".to_owned());
    }
    if decoded.stereo_samples.is_empty() {
        return Err("音声サンプルをデコードできませんでした".to_owned());
    }
    Ok(AudioData {
        stereo_pcm: decoded.stereo_samples,
        sample_rate: decoded.info.sample_rate,
        duration_secs: decoded.info.duration_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mimageviewer::audio_decode::decode_audio_file_to_stereo_f32_streaming;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/audio-tracks")
            .join(name)
    }

    #[test]
    fn ffmpeg_decode_matches_export_default_track_and_stereo_resampling() {
        let cancel = AtomicBool::new(false);
        // Both containers mark the second audio track (880 Hz / 44.1 kHz) default.
        // MKV also exercises FFmpeg's 5.1 -> stereo downmix; neither selects 440 Hz.
        for (name, expected_index, first_audio_index) in
            [("multi-audio.m4a", 1, 0), ("multi.mkv", 2, 1)]
        {
            let path = fixture(name);
            let stream_index = default_audio_stream_index(&path).unwrap();
            assert_eq!(stream_index, expected_index, "{name}");
            let expected = decode_audio_file_to_stereo_f32(&path, stream_index, &cancel).unwrap();
            let first = decode_audio_file_to_stereo_f32(&path, first_audio_index, &cancel).unwrap();
            let actual = decode_audio(&path, &cancel).unwrap();
            assert_eq!(actual.stereo_pcm, expected.stereo_samples, "{name}");
            assert_ne!(actual.stereo_pcm, first.stereo_samples, "{name}");
            assert_eq!(actual.sample_rate, OUTPUT_SAMPLE_RATE);
            assert_eq!(actual.stereo_pcm.len() % 2, 0);
            assert_eq!(actual.duration_secs, expected.info.duration_secs);
            assert_eq!(
                actual.duration_secs,
                actual.stereo_pcm.len() as f64 / 2.0 / OUTPUT_SAMPLE_RATE as f64
            );
            assert!(actual.duration_secs > 5.0);
        }
    }

    #[test]
    fn ffmpeg_decode_receives_worker_cancellation() {
        let cancel = AtomicBool::new(true);
        assert_eq!(
            decode_audio(&fixture("multi-audio.m4a"), &cancel).unwrap_err(),
            "cancelled"
        );
    }

    #[test]
    fn shared_ffmpeg_decode_stops_when_cancelled_after_pcm_is_produced() {
        // The full decode API used above delegates to this streaming decoder.
        // Cancel at its first PCM callback, with no timing race or sleeping worker.
        let path = fixture("multi-audio.m4a");
        let stream_index = default_audio_stream_index(&path).unwrap();
        let cancel = AtomicBool::new(false);
        let mut decoded_frames = 0;
        let result = decode_audio_file_to_stereo_f32_streaming(
            &path,
            stream_index,
            &cancel,
            0.0,
            |pcm, sample_rate| {
                assert_eq!(sample_rate, OUTPUT_SAMPLE_RATE);
                decoded_frames = pcm.len() / 2;
                cancel.store(true, Ordering::Relaxed);
            },
        );
        assert!(decoded_frames >= 2 * OUTPUT_SAMPLE_RATE as usize);
        assert!(decoded_frames < 3 * OUTPUT_SAMPLE_RATE as usize);
        assert_eq!(result.unwrap_err(), "cancelled");
    }
}
