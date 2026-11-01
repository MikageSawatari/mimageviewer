//! Headless T1 rendering measurement. Never opens an app/window or writes video.
//! Driven by python scripts/musicvideo/t1-measure.py (headless, release only).
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
            std::fs::read(std::env::var("T1_FONT").expect("T1_FONT")).unwrap(),
        )
        .unwrap(),
    );
    let assets = Assets::new(fonts);
    // Keep layers active at t=300 (end-exclusive spans), so the maximum outline
    // is actually baked on the final frame of the five-minute interval.
    let mut p = default_project(301.0);
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

fn animation(s: TextStyle, animated: bool, maximum: f32) -> Animatable<TextStyle> {
    if !animated {
        return Animatable::Const(s);
    }
    let mut end = s.clone();
    if let Some(outline) = &mut end.outline {
        outline.width_px = maximum;
    }
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
}
fn main() {
    let init = Instant::now();
    let case = std::env::args().nth(1).expect("scenario");
    let variant = std::env::args().nth(2).expect("none/current");
    assert!(matches!(variant.as_str(), "none" | "current"));
    let parts: Vec<_> = case.split('-').collect();
    assert_eq!(parts.len(), 3);
    let tiled = parts[0] == "tile";
    let animated = matches!(parts[1], "interp" | "mixed");
    let long = std::env::var("T1_LONG").as_deref() == Ok("1");
    let frames: usize = std::env::var("T1_FRAMES").unwrap().parse().unwrap();
    assert!(frames > 0);
    let (mut p, assets) = setup(variant == "current", animated);
    if tiled {
        p.layers.truncate(2);
        p.layers[1].rect = Animatable::Const(NormRect::FULL);
    }
    let mut s = if variant == "current" {
        style()
    } else {
        TextStyle::default()
    };
    if tiled && variant == "current" {
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
    }
    // Full-span retention stress reaches the UI maximum outline (128px).
    let anim = animation(s.clone(), animated, if long { 128.0 } else { 18.0 });
    for layer in p.layers.iter_mut().skip(1) {
        match &mut layer.kind {
            LayerKind::Text(t) => t.style = anim.clone(),
            LayerKind::TimeText(t) => t.style = anim.clone(),
            LayerKind::Lyrics(t) => t.style = anim.clone(),
            _ => unreachable!(),
        }
    }
    if parts[1] == "mixed" {
        let mut fixed = new_text_layer(&p);
        fixed.rect = Animatable::Const(NormRect::new(0.05, 0.45, 0.9, 0.2));
        if let LayerKind::Text(t) = &mut fixed.kind {
            t.text = "静的 + 補間".into();
            t.size_px = 64.0;
            t.style = Animatable::Const(s);
        }
        p.layers.push(fixed);
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
    let initialization_ms = init.elapsed().as_secs_f64() * 1000.0;
    let cold = Instant::now();
    let _ = render_frame(
        &evaluate(&p, 0.0, &SpectrumFrame::default()),
        &assets,
        &mut frame,
    )
    .unwrap();
    let first_frame_ms = cold.elapsed().as_secs_f64() * 1000.0;
    // Sum of preparation durations (layout/coverage + RGBA), excludes composition.
    // Across parallel callers this is aggregate time, not wall-clock subtraction.
    let initial_preparation_ms = assets.text_cache_stats().preparation_ns as f64 / 1e6;
    for _ in 0..30 {
        let _ = render_frame(
            &evaluate(&p, 0.0, &SpectrumFrame::default()),
            &assets,
            &mut frame,
        )
        .unwrap();
    }
    let wall = Instant::now();
    let mut times = Vec::with_capacity(frames);
    let mut maxima = [0_usize; 4];
    // Include cold/warm allocations in the retained-memory maxima too.
    let st = assets.text_cache_stats();
    maxima[0] = st.stable_bytes;
    maxima[1] = st.transient_bytes - st.prepared_bytes;
    maxima[2] = st.prepared_bytes;
    maxima[3] = st.max_raster_bytes;
    for n in 0..frames {
        let t = if long {
            n as f64 * 300.0 / (frames - 1).max(1) as f64
        } else {
            n as f64 / 30.0
        };
        let scene = evaluate(&p, t, &SpectrumFrame::default());
        let start = Instant::now();
        let _ = render_frame(&scene, &assets, &mut frame).unwrap();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
        let st = assets.text_cache_stats();
        let lanes = [
            st.stable_bytes,
            st.transient_bytes - st.prepared_bytes,
            st.prepared_bytes,
            st.max_raster_bytes,
        ];
        for (max, current) in maxima.iter_mut().zip(lanes) {
            *max = (*max).max(current);
        }
        if n % 30 == 0 || n + 1 == frames {
            println!(
                "{}",
                serde_json::json!({"type":"sample", "frame":n, "scene_secs":t,
                "elapsed_secs":wall.elapsed().as_secs_f64(), "stable_bytes":lanes[0],
                "transient_rgba_bytes":lanes[1], "prepared_bytes":lanes[2], "max_raster_bytes":lanes[3]})
            );
        }
        assert!(
            lanes[0] <= 24 * 1024 * 1024
                && lanes[1] <= 8 * 1024 * 1024
                && lanes[2] <= 32 * 1024 * 1024
        );
        assert!(lanes[3] <= MAX_TEXT_RASTER_BYTES);
    }
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    times.sort_by(f64::total_cmp);
    let hash = frame.rgba.iter().fold(0xcbf29ce484222325_u64, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x100000001b3)
    });
    println!(
        "{}",
        serde_json::json!({"type":"result", "case":case, "variant":variant,
        "frames":frames,"long":long, "initialization_ms":initialization_ms,
        "initial_preparation_ms":initial_preparation_ms, "first_frame_ms":first_frame_ms,
        "mean_ms":mean,"p95_ms":times[(frames*95).div_ceil(100)-1],"hash":format!("{hash:016x}"),
        "max_stable_bytes":maxima[0], "max_transient_rgba_bytes":maxima[1],
        "max_prepared_bytes":maxima[2], "max_raster_bytes":maxima[3],
        "measured_secs":wall.elapsed().as_secs_f64()})
    );
}
