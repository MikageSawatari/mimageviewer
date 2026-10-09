//! Headless T1 rendering measurement. Never opens an app/window or writes video.
//! cargo build --release -p musicvideo-core --example t1_measure
//! target/release/examples/t1_measure.exe [composition|long]
use musicvideo_core::*;
use std::time::Instant;

fn style() -> TextStyle {
    TextStyle {
        outline: Some(StrokeStyle {
            color: TextColor::new(255, 255, 255, 255),
            width_px: 3.0,
        }),
        extra_outlines: vec![StrokeStyle {
            color: TextColor::new(30, 60, 180, 255),
            width_px: 7.0,
        }],
        shadow: Some(TextShadowStyle {
            color: TextColor::new(0, 0, 0, 180),
            offset: (8.0, 8.0),
            blur_px: 6.0,
            spread_px: 2.0,
        }),
        background: Some(TextBackgroundStyle {
            fill: TextColor::new(30, 50, 80, 200),
            padding_px: 12.0,
            corner_px: 8.0,
        }),
        ..Default::default()
    }
}
fn setup(decorated: bool, animated: bool) -> (Project, Assets) {
    let mut fonts = comic_core::FontSet::new();
    fonts.insert(
        comic_core::LoadedFont::from_bytes(
            DEFAULT_FONT_KEY,
            std::fs::read(r"C:\Windows\Fonts\YuGothM.ttc").unwrap(),
        )
        .unwrap(),
    );
    let assets = Assets::new(fonts);
    let mut p = default_project(300.0);
    p.output = Output {
        width: 1920,
        height: 1080,
        fps: 30.0,
    };
    p.layers.clear();
    let mut background = new_solid_layer(&p);
    background.rect = Animatable::Const(NormRect::FULL);
    if let LayerKind::Solid { fill } = &mut background.kind {
        *fill = Animatable::Const(Fill::Solid(Rgba8::new(35, 35, 35, 255)));
    }
    p.layers.push(background);
    let mut text = new_text_layer(&p);
    text.rect = Animatable::Const(NormRect::new(0.05, 0.1, 0.9, 0.2));
    p.layers.push(text);
    let mut time = new_time_text_layer(&p);
    time.rect = Animatable::Const(NormRect::new(0.05, 0.4, 0.9, 0.2));
    p.layers.push(time);
    let mut lyrics = new_lyrics_layer(&p);
    lyrics.rect = Animatable::Const(NormRect::new(0.05, 0.7, 0.9, 0.2));
    p.layers.push(lyrics);
    let s = if decorated {
        style()
    } else {
        TextStyle::default()
    };
    let mut end = s.clone();
    if let Some(outline) = &mut end.outline {
        outline.width_px = 18.0;
    }
    let animation = if animated {
        Animatable::Track(
            Track::new(vec![
                Keyframe {
                    t: 0.0,
                    value: s,
                    interp: Interp::Linear,
                },
                Keyframe {
                    t: 300.0,
                    value: end,
                    interp: Interp::Linear,
                },
            ])
            .unwrap(),
        )
    } else {
        Animatable::Const(s)
    };
    for layer in &mut p.layers {
        match &mut layer.kind {
            LayerKind::Text(t) => {
                t.text = "日本語 Text decoration".into();
                t.size_px = 64.0;
                t.style = animation.clone();
            }
            LayerKind::TimeText(t) => {
                t.size_px = 64.0;
                t.style = animation.clone();
            }
            LayerKind::Lyrics(t) => {
                t.size_px = 64.0;
                t.style = animation.clone();
                t.lines = (0..100)
                    .map(|i| LyricLine {
                        text: format!("歌詞の文字装飾 {i:03}"),
                        time_secs: Some(i as f64 * 3.0),
                    })
                    .collect();
            }
            _ => {}
        }
    }
    (p, assets)
}
fn composition(decorated: bool) {
    let (p, assets) = setup(decorated, false);
    let mut frame = Frame::new(1920, 1080).unwrap();
    for _ in 0..30 {
        let _ = render_frame(
            &evaluate(&p, 0.5, &SpectrumFrame::default()),
            &assets,
            &mut frame,
        )
        .unwrap();
    }
    let mut times = Vec::new();
    for n in 0..300 {
        let scene = evaluate(&p, 0.5 + n as f64 / 30.0, &SpectrumFrame::default());
        let start = Instant::now();
        let _ = render_frame(&scene, &assets, &mut frame).unwrap();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    times.sort_by(f64::total_cmp);
    println!(
        "composition decorated={decorated} frames=300 mean_ms={mean:.4} p95_ms={:.4} cache={:?}",
        times[284],
        assets.text_cache_stats()
    );
}
fn long() {
    let (p, assets) = setup(true, true);
    let mut frame = Frame::new(1920, 1080).unwrap();
    let start = Instant::now();
    let mut max_bytes = 0;
    let mut max_entries = 0;
    let mut max_transient = 0;
    for n in 0..9000 {
        let scene = evaluate(&p, n as f64 / 30.0, &SpectrumFrame::default());
        let _ = render_frame(&scene, &assets, &mut frame).unwrap();
        let stats = assets.text_cache_stats();
        max_bytes = max_bytes.max(stats.stable_bytes + stats.transient_bytes);
        max_transient = max_transient.max(stats.transient_bytes);
        max_entries = max_entries.max(stats.entries);
        if n % 900 == 0 {
            println!("long frame={n} cache={stats:?}");
        }
        assert!(stats.stable_bytes + stats.transient_bytes <= MAX_TEXT_CACHE_BYTES);
    }
    println!(
        "long frames=9000 duration_secs=300 fps=30 max_cache_bytes={max_bytes} max_transient_bytes={max_transient} max_entries={max_entries} elapsed_secs={:.3} final={:?}",
        start.elapsed().as_secs_f64(),
        assets.text_cache_stats()
    );
}
// One scenario per process makes the OS peak meaningful (including font/frame,
// layout, glyph-cache, LRU, rotated body and glow buffers). The Python driver
// samples Windows peak_wset / peak_pagefile and checks CPU competition.
fn matrix(case: &str) {
    let parts: Vec<_> = case.split('-').collect();
    assert_eq!(parts.len(), 3);
    let tiled = parts[0] == "tile";
    let animated = parts[1] == "interp";
    let (mut p, assets) = setup(true, animated);
    if tiled {
        p.layers.truncate(2); // opaque background + one full-HD text layer
        p.layers[1].rect = Animatable::Const(NormRect::FULL);
        let mut s = style();
        s.shadow = Some(TextShadowStyle {
            color: TextColor::new(0, 0, 0, 180),
            offset: (512.0, -512.0),
            blur_px: 48.0,
            spread_px: 16.0,
        });
        s.echo = Some(TextEchoStyle {
            color: TextColor::new(60, 130, 220, 150),
            offset: (512.0, 512.0),
            count: 12,
        });
        let mut end = s.clone();
        end.outline.as_mut().unwrap().width_px = 18.0;
        if let LayerKind::Text(t) = &mut p.layers[1].kind {
            t.style = if animated {
                Animatable::Track(
                    Track::new(vec![
                        Keyframe {
                            t: 0.0,
                            value: s,
                            interp: Interp::Linear,
                        },
                        Keyframe {
                            t: 300.0,
                            value: end,
                            interp: Interp::Linear,
                        },
                    ])
                    .unwrap(),
                )
            } else {
                Animatable::Const(s)
            };
        }
    }
    for layer in p.layers.iter_mut().skip(1) {
        if matches!(parts[2], "glow" | "both") {
            layer.glow = Animatable::Const(Some(Glow {
                color: Rgba8::new(120, 180, 240, 180),
                radius_px: 12.0,
                spread_px: 3.0,
            }));
        }
        if matches!(parts[2], "rotate" | "both") {
            layer.rotation = Animatable::Const(12.0);
        }
    }
    let mut frame = Frame::new(1920, 1080).unwrap();
    let frames: usize = std::env::var("T1_FRAMES")
        .ok()
        .map(|s| s.parse().unwrap())
        .unwrap_or(300);
    let mut times = Vec::with_capacity(frames);
    let mut max_cache = 0;
    let mut hash = 0;
    for n in 0..30 + frames {
        let scene = evaluate(&p, n as f64 / 30.0, &SpectrumFrame::default());
        let start = Instant::now();
        let _ = render_frame(&scene, &assets, &mut frame).unwrap();
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        if n >= 30 {
            times.push(elapsed);
        }
        let stats = assets.text_cache_stats();
        max_cache = max_cache.max(stats.stable_bytes + stats.transient_bytes);
        if n + 1 == 30 + frames {
            hash = frame.rgba.iter().fold(0xcbf29ce484222325_u64, |h, b| {
                (h ^ *b as u64).wrapping_mul(0x100000001b3)
            });
        }
    }
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    times.sort_by(f64::total_cmp);
    println!(
        "matrix case={case} frames={frames} mean_ms={mean:.4} p95_ms={:.4} max_cache_bytes={max_cache} hash={hash:016x} final_cache={:?}",
        times[(frames * 95).div_ceil(100) - 1],
        assets.text_cache_stats()
    );
}
fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("matrix") => matrix(&std::env::args().nth(2).expect("matrix case")),
        Some("long") => long(),
        _ => {
            composition(false);
            composition(true);
        }
    }
}
