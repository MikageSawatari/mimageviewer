//! Extract-only ASR benchmark. No application startup or settings writes.
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: transcript_extract_bench <media-file>")?;
    let cancel = Arc::new(AtomicBool::new(false));
    let total_started = std::time::Instant::now();
    let Some(prepared) =
        mimageviewer::ai::transcript_audio::prepare_with_saved(&path, None, cancel.clone())?
    else {
        println!("{{\"no_audio\":true}}");
        return Ok(());
    };
    let stream = prepared.stream_index;
    let declared_duration_secs = prepared.duration_secs;
    let prepare_s = total_started.elapsed().as_secs_f64();
    let stats = prepared.extract(&cancel, |_| Ok(()))?;
    let total_s = total_started.elapsed().as_secs_f64();
    let audio_s = stats.audio_duration_ms as f64 / 1000.0;
    println!(
        "{}",
        serde_json::json!({
            "audio_stream": stream, "declared_duration_s":declared_duration_secs,
            "audio_duration_s": audio_s, "pcm_samples":stats.pcm_samples,
            "extract_s":stats.elapsed.as_secs_f64(),
        "prepare_s":prepare_s,"total_s":total_s,
        "total_wall_audio_ratio":total_s/audio_s,"total_realtime_multiple":audio_s/total_s,
            "realtime_multiple":audio_s/stats.elapsed.as_secs_f64(),
            "wall_audio_ratio":stats.elapsed.as_secs_f64()/audio_s,
        })
    );
    Ok(())
}
