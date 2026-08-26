//! 動画の球面 (360 度) メタデータを mIV 自身のパーサで読んで表示する開発用ツール。
//!
//! `ffprobe` でも side data は見えるが、それは **FFmpeg の表示**であって
//! [`crate::video::spherical_metadata`] が実際に何を作るかではない。実素材の生バイトを
//! こちらのコードへ通し、投影種別 / 初期視点 / UV 変換 / ステレオ / 判定結果まで
//! 突き合わせるために使う (backlog §1.112)。
//!
//! ```text
//! cargo run --features dev-tools --bin probe_spherical -- <file|dir> [...]
//! ```
//!
//! ディレクトリを渡すと直下の動画を並べて表を出す。

use std::path::{Path, PathBuf};

use mimageviewer::video::display_metadata;
use mimageviewer::video::spherical_metadata::{
    self, VideoPanoramaRejection, VideoPanoramaTrigger, VideoStereoLayout,
};

const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "avi", "wmv", "mpg", "mpeg", "m4v", "ts",
];

fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn collect(inputs: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for input in inputs {
        let path = PathBuf::from(input);
        if path.is_dir() {
            let Ok(entries) = std::fs::read_dir(&path) else {
                eprintln!("cannot read dir: {}", path.display());
                continue;
            };
            let mut files: Vec<PathBuf> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file() && is_video(p))
                .collect();
            files.sort();
            out.extend(files);
        } else if path.is_file() {
            out.push(path);
        } else {
            eprintln!("not found: {}", path.display());
        }
    }
    out
}

struct Row {
    name: String,
    size: String,
    projection: String,
    pose: String,
    uv: String,
    stereo: String,
    verdict: String,
}

fn probe(path: &Path) -> Result<Row, String> {
    let input = ffmpeg_the_third::format::input(path).map_err(|e| e.to_string())?;
    let stream = input
        .streams()
        .best(ffmpeg_the_third::media::Type::Video)
        .ok_or_else(|| "no video stream".to_string())?;
    let params = stream.parameters();
    let decoder = ffmpeg_the_third::codec::context::Context::from_parameters(params)
        .map_err(|e| e.to_string())?;
    let video = decoder.decoder().video().map_err(|e| e.to_string())?;

    let orientation = display_metadata::orientation_from_stream(&stream);
    // SAR の取り方は thumbnail / seek strip と同じ経路に揃える。
    let sar = stream.parameters().sample_aspect_ratio();
    let (sar_num, sar_den) =
        mimageviewer::video::decoder::normalize_sar(sar.numerator(), sar.denominator());
    let (display_w, display_h) = display_metadata::display_dimensions(
        video.width(),
        video.height(),
        sar_num,
        sar_den,
        orientation,
    );
    let display_w = display_w.round().max(1.0) as u32;
    let display_h = display_h.round().max(1.0) as u32;

    let mapping = spherical_metadata::spherical_from_stream(&stream);
    let stereo = spherical_metadata::stereo_layout_from_stream(&stream);
    let verdict = spherical_metadata::detect(mapping.as_ref(), stereo, display_w, display_h);

    let projection = mapping
        .map(|m| m.projection.debug_name().to_string())
        .unwrap_or_else(|| "-".to_string());
    let pose = mapping
        .map(|m| {
            format!(
                "{:.0}/{:.0}/{:.0}",
                m.yaw_degrees, m.pitch_degrees, m.roll_degrees
            )
        })
        .unwrap_or_else(|| "-".to_string());
    let uv = mapping
        .map(|m| {
            if m.uv_transform.is_identity() {
                "full".to_string()
            } else {
                format!(
                    "off({:.3},{:.3}) scale({:.3},{:.3})",
                    m.uv_transform.u_offset,
                    m.uv_transform.v_offset,
                    m.uv_transform.u_scale,
                    m.uv_transform.v_scale
                )
            }
        })
        .unwrap_or_else(|| "-".to_string());
    let stereo_text = match stereo {
        VideoStereoLayout::Mono => "mono".to_string(),
        VideoStereoLayout::TopBottom => "top-bottom".to_string(),
        VideoStereoLayout::SideBySide => "side-by-side".to_string(),
        VideoStereoLayout::Other(v) => format!("other({v})"),
    };
    let verdict_text = match verdict {
        Ok(VideoPanoramaTrigger::Auto) => "AUTO".to_string(),
        Ok(VideoPanoramaTrigger::Hint) => "hint(2:1)".to_string(),
        Err(VideoPanoramaRejection::UnsupportedProjection(p)) => {
            format!("no: {}", p.debug_name())
        }
        Err(VideoPanoramaRejection::Stereoscopic(_)) => "no: stereo".to_string(),
        Err(VideoPanoramaRejection::NotPanoramic) => "no: flat".to_string(),
    };

    let raw = format!("{}x{}", video.width(), video.height());
    let size = if raw == format!("{display_w}x{display_h}") {
        raw
    } else {
        format!("{raw}->{display_w}x{display_h}")
    };

    Ok(Row {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        size,
        projection,
        pose,
        uv,
        stereo: stereo_text,
        verdict: verdict_text,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: probe_spherical <file|dir> [...]");
        std::process::exit(2);
    }
    if let Err(err) = ffmpeg_the_third::init() {
        eprintln!("ffmpeg init failed: {err}");
        std::process::exit(1);
    }

    let files = collect(&args);
    if files.is_empty() {
        eprintln!("no video files found");
        std::process::exit(1);
    }

    let mut rows = Vec::new();
    for path in &files {
        match probe(path) {
            Ok(row) => rows.push(row),
            Err(err) => eprintln!("{}: {err}", path.display()),
        }
    }

    let name_w = rows.iter().map(|r| r.name.len()).max().unwrap_or(4).max(4);
    let size_w = rows.iter().map(|r| r.size.len()).max().unwrap_or(4).max(4);
    let proj_w = rows
        .iter()
        .map(|r| r.projection.len())
        .max()
        .unwrap_or(10)
        .max(10);
    println!(
        "{:<name_w$}  {:<size_w$}  {:<proj_w$}  {:<10}  {:<12}  {:<28}  verdict",
        "file",
        "size",
        "projection",
        "pose y/p/r",
        "stereo",
        "uv",
        name_w = name_w,
        size_w = size_w,
        proj_w = proj_w,
    );
    for r in &rows {
        println!(
            "{:<name_w$}  {:<size_w$}  {:<proj_w$}  {:<10}  {:<12}  {:<28}  {}",
            r.name,
            r.size,
            r.projection,
            r.pose,
            r.stereo,
            r.uv,
            r.verdict,
            name_w = name_w,
            size_w = size_w,
            proj_w = proj_w,
        );
    }
}
