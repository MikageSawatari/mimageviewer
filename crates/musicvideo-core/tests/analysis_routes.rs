//! Repository boundary checks for H3a A4. These intentionally inspect production
//! consumers: a second lattice or legacy adapter can pass core math tests while
//! silently putting preview and export back on different analysis paths.
use std::path::Path;

fn source(relative: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::read_to_string(root.join(relative))
        .unwrap()
        .replace("\r\n", "\n")
        .split("\n#[cfg(test)]\nmod tests")
        .next()
        .unwrap()
        .to_owned()
}

fn function<'a>(source: &'a str, name: &str) -> &'a str {
    let signature = format!("fn {name}");
    let start = source
        .match_indices(&signature)
        .find(|(index, _)| {
            matches!(
                source.as_bytes().get(index + signature.len()),
                Some(b'(' | b'<')
            )
        })
        .map(|(index, _)| index)
        .unwrap_or_else(|| panic!("missing {name}"));
    let start = start + source[start..].find('{').unwrap();
    let mut depth = 0;
    for (offset, character) in source[start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..=start + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unclosed {name}")
}

fn uses(body: &str, required: &[&str]) {
    for token in required {
        assert!(
            body.contains(token),
            "missing shared analysis route: {token}"
        );
    }
}

#[test]
fn export_driver_and_cli_share_the_frame_grid_view_and_timeline() {
    let driver = source("src/video/mvexport/driver.rs");
    uses(
        function(&driver, "validate_request"),
        &[
            "FrameGrid::new",
            "grid.frames()",
            "grid.numerator()",
            "grid.denominator()",
        ],
    );
    uses(
        function(&driver, "export"),
        &[
            "pcm_view(",
            "AnalysisKey::new(",
            "AnalysisTimeline::default()",
            "plan.grid.time(frame_index)",
            ".frame(&key, view, frame_index",
            "calibrate_modulators_view(",
            "source_duration_secs",
        ],
    );
    assert!(!driver.contains("rational_frame_rate("));
    assert!(!driver.contains("SpectrumSource"));
    assert!(!driver.contains("full_frame_at("));
    let cli = source("src/bin/mvexport_cli.rs");
    uses(function(&cli, "run"), &["export_music_video("]);
    for legacy in [
        "SpectrumSource",
        "full_frame_at(",
        "rational_frame_rate(",
        "duration_secs * output.fps",
    ] {
        assert!(!cli.contains(legacy), "CLI duplicates analysis: {legacy}");
    }
}

#[test]
fn lab_preview_png_and_benchmark_share_analysis_inputs() {
    let app = source("crates/musicvideo-editor/src/app.rs");
    let export = source("crates/musicvideo-editor/src/export.rs");
    uses(
        function(&app, "start_preview_if_needed"),
        &[
            "FrameGrid::new(",
            "grid.index_at(",
            "grid.time(",
            "pcm_view(",
            "AnalysisKey::new(",
            "analyze_preview(",
        ],
    );
    uses(
        function(&app, "analyze_preview"),
        &[".frame(key, pcm, n,", ".cloned()"],
    );
    let analysis = function(&app, "analyze_preview");
    let lock = analysis.find("timeline.lock()").unwrap();
    let cancelled = analysis.find("if cancel.load(Ordering::Relaxed)").unwrap();
    let frame = analysis.find(".frame(key, pcm, n,").unwrap();
    assert!(
        lock < cancelled && cancelled < frame,
        "host must check cancellation immediately after lock, before timeline input changes"
    );
    uses(
        function(&export, "new"),
        &[
            "FrameGrid::new(",
            "pcm_view(",
            "AnalysisKey::new(",
            "AnalysisTimeline::default()",
            "calibrate_modulators_view(",
        ],
    );
    uses(
        function(&export, "frame_count"),
        &["FrameGrid::new(", ".frames()"],
    );
    uses(
        function(&export, "export_png_sequence"),
        &[
            "LabAnalysis::new(",
            "spectrum.grid.time(",
            "spectrum.frame(",
        ],
    );
    uses(
        function(&export, "benchmark_once"),
        &[
            "LabAnalysis::new(",
            "spectrum.grid.time(",
            "spectrum.frame(",
        ],
    );
    let benchmark = function(&export, "benchmark_once");
    assert_eq!(
        benchmark.matches("spectrum.frame(").count(),
        2,
        "both warmup and measured frames must use the timeline"
    );
    for route in [&app, &export] {
        for legacy in [
            "SpectrumSource",
            "full_frame_at(",
            "previous_spectrum",
            "rational_frame_rate(",
            "as f64 / project.output.fps",
        ] {
            assert!(!route.contains(legacy), "lab duplicates analysis: {legacy}");
        }
    }
}
