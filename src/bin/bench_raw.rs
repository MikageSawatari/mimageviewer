//! S1-only RAW measurement. All development is submitted to RawDevelopExecutor.
use image::{DynamicImage, GenericImage, GenericImageView, RgbaImage};
use mimageviewer::raw::brightness::{MatchDecision, median_linear_luma};
use mimageviewer::raw::raw_decoder::{
    self, RawBenchBrightness, RawDevelopScale, RawDevelopSupport, RawOwnedSource, RawSource,
};
use mimageviewer::raw::{RawDevelopExecutor, RawPriority};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{atomic::Ordering, mpsc};
use std::time::{Duration, Instant};

fn develop(
    executor: &RawDevelopExecutor,
    path: &Path,
    scale: RawDevelopScale,
    brightness: RawBenchBrightness,
) -> Result<(DynamicImage, f64), String> {
    let (sender, receiver) = mpsc::channel();
    let start = Instant::now();
    let _ticket = executor.submit_bench(
        RawOwnedSource::Path(path.to_path_buf()),
        scale,
        brightness,
        RawPriority::High,
        sender,
    );
    let result = receiver
        .recv()
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    Ok((result, start.elapsed().as_secs_f64() * 1000.0))
}

fn develop_match_preview(
    executor: &RawDevelopExecutor,
    path: &Path,
    preview_median: Option<f64>,
) -> Result<(raw_decoder::RawMatchPreviewOutput, f64), String> {
    let (sender, receiver) = mpsc::channel();
    let start = Instant::now();
    let _ticket = executor.submit_match_preview(
        RawOwnedSource::Path(path.to_path_buf()),
        RawDevelopScale::Full,
        preview_median,
        RawPriority::High,
        sender,
    );
    let result = receiver
        .recv()
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    Ok((result, start.elapsed().as_secs_f64() * 1000.0))
}

fn cancel_latency(executor: &RawDevelopExecutor, path: &Path) -> Option<f64> {
    let (sender, receiver) = mpsc::channel();
    let ticket = executor.submit_bench(
        RawOwnedSource::Path(path.to_path_buf()),
        RawDevelopScale::Full,
        RawBenchBrightness::Auto001,
        RawPriority::High,
        sender,
    );
    let progress = ticket.progress();
    loop {
        if progress.load(Ordering::Acquire) >= 30 {
            let start = Instant::now();
            ticket.cancel();
            let _ = receiver.recv();
            return Some(start.elapsed().as_secs_f64() * 1000.0);
        }
        match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn mean_luma(image: &DynamicImage) -> f64 {
    let rgb = image.to_rgb8();
    let count = rgb.pixels().len();
    let stride = (count / 100_000).max(1);
    let mut total = 0.0;
    let mut measured = 0usize;
    for pixel in rgb.pixels().step_by(stride) {
        total += (0.2126 * f64::from(pixel[0])
            + 0.7152 * f64::from(pixel[1])
            + 0.0722 * f64::from(pixel[2]))
            / 255.0;
        measured += 1;
    }
    total / measured.max(1) as f64
}

fn write_comparison(path: &Path, images: &[&DynamicImage]) -> Result<(), String> {
    let panels: Vec<RgbaImage> = images
        .iter()
        .map(|image| image.thumbnail(800, 600).into_rgba8())
        .collect();
    let width = panels.iter().map(|p| p.width()).sum::<u32>() + 3 * (panels.len() as u32 - 1);
    let height = panels.iter().map(|p| p.height()).max().unwrap_or(0);
    let mut canvas = RgbaImage::new(width, height);
    let mut x = 0;
    for panel in &panels {
        canvas
            .copy_from(panel, x, 0)
            .map_err(|error| error.to_string())?;
        x += panel.width() + 3;
    }
    canvas.save(path).map_err(|error| error.to_string())
}

fn benchmark(sample: &Value, executor: &RawDevelopExecutor, output: &Path) -> Value {
    let id = sample["id"].as_str().unwrap();
    let file = sample["file"].as_str().unwrap();
    let path = PathBuf::from("vendor/raw-samples").join(file);
    let start = Instant::now();
    let info = match raw_decoder::info(RawSource::Path(&path)) {
        Ok(info) => info,
        Err(error) => return json!({"id":id,"error":error.to_string()}),
    };
    let info_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let preview = raw_decoder::preview(RawSource::Path(&path));
    let preview_ms = start.elapsed().as_secs_f64() * 1000.0;
    let preview_error = preview.as_ref().err().map(ToString::to_string);
    let preview_dims = preview
        .as_ref()
        .ok()
        .map(|preview| preview.image.dimensions());
    let preview_size = preview_dims.map(|(w, h)| [w, h]);
    let zero_dim_cr3_thumb = info
        .previews
        .iter()
        .any(|p| p.recorded_dims == [0, 0] && p.dims[0] > 0 && p.dims[1] > 0);
    if info.develop_support != RawDevelopSupport::Supported {
        return json!({"id":id,"case":sample["case"],"info_ms":info_ms,
            "preview_ms":preview_ms,"preview_dims":preview_size,
            "preview_error":preview_error,
            "develop_support":format!("{:?}",info.develop_support),
            "flip":info.flip,"previews":info.previews.len()});
    }
    let (full, full_ms) = match develop(
        executor,
        &path,
        RawDevelopScale::Full,
        RawBenchBrightness::Auto001,
    ) {
        Ok(value) => value,
        Err(error) => {
            return json!({"id":id,"error":error,"info_ms":info_ms,"preview_ms":preview_ms});
        }
    };
    let (half, half_ms) = match develop(
        executor,
        &path,
        RawDevelopScale::Half,
        RawBenchBrightness::Auto001,
    ) {
        Ok(value) => value,
        Err(error) => return json!({"id":id,"error":error,"full_ms":full_ms}),
    };
    let (auto0001, auto0001_ms) = match develop(
        executor,
        &path,
        RawDevelopScale::Full,
        RawBenchBrightness::Auto0001,
    ) {
        Ok(value) => value,
        Err(error) => return json!({"id":id,"error":error,"full_ms":full_ms}),
    };
    let preview_median_linear = preview
        .as_ref()
        .ok()
        .and_then(|preview| median_linear_luma(&preview.image));
    let (matched_output, match_preview_ms) =
        match develop_match_preview(executor, &path, preview_median_linear) {
            Ok(value) => value,
            Err(error) => return json!({"id":id,"error":error,"full_ms":full_ms}),
        };
    let no_auto = &matched_output.no_auto;
    let matched = &matched_output.matched;
    let matched_median_linear = median_linear_luma(matched);
    let (gain, unclamped_gain, gain_clamped, match_fallback) = match matched_output.decision {
        MatchDecision::Gain {
            gain,
            unclamped,
            clamped,
        } => (Some(gain), Some(unclamped), clamped, None),
        MatchDecision::Fallback(reason) => (None, None, false, Some(format!("{reason:?}"))),
    };
    let cancel_ms = cancel_latency(executor, &path);
    let aspect_diff_percent = preview_dims.map(|(w, h)| {
        let preview_ratio = f64::from(w) / f64::from(h);
        let full_ratio = f64::from(full.width()) / f64::from(full.height());
        (preview_ratio / full_ratio - 1.0).abs() * 100.0
    });
    let preview_luma = preview.as_ref().ok().map(|image| mean_luma(&image.image));
    let comparison = preview
        .as_ref()
        .ok()
        .map(|_| output.join(format!("{id}.png")));
    if let (Some(preview), Some(path)) = (preview.as_ref().ok(), comparison.as_ref())
        && let Err(error) =
            write_comparison(path, &[&preview.image, &full, &auto0001, no_auto, matched])
    {
        eprintln!("{}: comparison PNG: {}", id, error);
    }
    let wic_dims = if sample["format"] == "DNG" {
        mimageviewer::wic_decoder::decode_to_dynamic_image(&path)
            .map(|image| [image.width(), image.height()])
    } else {
        None
    };
    json!({
        "id":id,"case":sample["case"],"camera":sample["camera"],
        "info_dims":info.developed_dims,"full_dims":[full.width(),full.height()],
        "half_dims":[half.width(),half.height()],"dims_match":info.developed_dims==[full.width(),full.height()],
        "flip":info.flip,"preview_dims":preview_size,"preview_count":info.previews.len(),
        "preview_error":preview_error,
        "selected_preview_tflip":preview.as_ref().ok().and_then(|preview|preview.info.tflip),
        "zero_dim_cr3_thumb_candidate":zero_dim_cr3_thumb,
        "info_ms":info_ms,"preview_ms":preview_ms,"full_ms":full_ms,"half_ms":half_ms,
        "auto0001_ms":auto0001_ms,"match_preview_ms":match_preview_ms,"cancel_ms":cancel_ms,
        "aspect_diff_percent":aspect_diff_percent,"preview_luma":preview_luma,
        "auto001_luma_diff":preview_luma.map(|value|(mean_luma(&full)-value).abs()),
        "auto0001_luma_diff":preview_luma.map(|value|(mean_luma(&auto0001)-value).abs()),
        "no_auto_luma_diff":preview_luma.map(|value|(mean_luma(no_auto)-value).abs()),
        "match_mean_luma_diff":preview_luma.map(|value|(mean_luma(matched)-value).abs()),
        "preview_median_linear":preview_median_linear,
        "developed_median_linear":matched_output.developed_median,
        "matched_median_linear":matched_median_linear,
        "match_median_luma_diff_linear":preview_median_linear.zip(matched_median_linear)
            .map(|(preview, developed)| (preview-developed).abs()),
        "match_gain":gain,"match_unclamped_gain":unclamped_gain,
        "match_gain_clamped":gain_clamped,"match_fallback":match_fallback,
        "comparison_png":comparison.map(|p|p.display().to_string()),"wic_dims":wic_dims,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest: Value = serde_json::from_slice(&std::fs::read("tests/raw-samples.json")?)?;
    let output = Path::new("target/raw-compare");
    std::fs::create_dir_all(output)?;
    let executor = RawDevelopExecutor::new(3)?;
    let only = std::env::var("RAW_BENCH_ONLY").ok();
    let result_file = output.join("results.json");
    let mut results: Vec<Value> = if only.is_some() && result_file.exists() {
        serde_json::from_slice(&std::fs::read(&result_file)?)?
    } else {
        Vec::new()
    };
    for sample in manifest["samples"].as_array().ok_or("missing samples")? {
        if only
            .as_deref()
            .is_some_and(|id| sample["id"].as_str() != Some(id))
        {
            continue;
        }
        eprintln!("Measuring {}", sample["case"]);
        let result = benchmark(sample, &executor, output);
        println!("{}", serde_json::to_string(&result)?);
        if let Some(existing) = results.iter_mut().find(|entry| entry["id"] == sample["id"]) {
            *existing = result;
        } else {
            results.push(result);
        }
    }
    std::fs::write(result_file, serde_json::to_vec_pretty(&results)?)?;
    Ok(())
}
