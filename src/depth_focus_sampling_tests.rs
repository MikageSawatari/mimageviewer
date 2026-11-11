use super::*;

fn params() -> DepthRenderParams {
    DepthRenderParams::from_settings(
        &crate::depth_parallax::DepthParallaxSettings::default(),
        0.5,
        egui::Vec2::ZERO,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
        0,
        1.,
    )
}

#[test]
fn depth_focus_cpu_rotation_crop_zoom_gamma_bilinear_and_fractional_fit() {
    let rect = egui::Rect::from_min_size(egui::pos2(10.25, 20.5), egui::vec2(123.5, 88.25));
    let values = [0., 0.25, 0.5, 1.].map(half::f16::from_f32);
    let mut p = params();
    p.zoom = 1.;
    let sample = |uv: egui::Vec2, p| {
        focus_depth_at(rect.min + rect.size() * uv, rect, p, 2, 2, &values).unwrap()
    };
    for (q, expected) in [0., 0.5, 1., 0.25].into_iter().enumerate() {
        p.quarter_turns = q as u32;
        assert!((sample(egui::vec2(0.25, 0.25), p) - expected).abs() < 1e-6);
    }
    p.quarter_turns = 0;
    assert!((sample(egui::Vec2::splat(0.5), p) - 0.4375).abs() < 1e-6);
    assert_eq!(sample(egui::Vec2::ZERO, p), 0.);
    assert_eq!(sample(egui::Vec2::splat(1.), p), 1.);
    p.zoom = 2.;
    assert!((sample(egui::Vec2::splat(0.25), p) - 0.203125).abs() < 1e-6);
    p.uv = egui::Rect::from_min_max(egui::pos2(0.25, 0.25), egui::pos2(0.75, 0.75));
    p.gamma = 2.;
    assert!((sample(egui::Vec2::splat(0.25), p) - 0.31640625_f32.powi(2)).abs() < 1e-6);
    assert!(focus_depth_at(rect.min - egui::vec2(0.01, 0.), rect, p, 2, 2, &values).is_none());
    // Crop clamping happens before the texture's clamp-to-edge sampling.
    p.zoom = 0.5;
    p.gamma = 1.;
    assert_eq!(sample(egui::Vec2::ZERO, p), 0.);
}

#[test]
fn depth_focus_export_params_never_inherit_highlight_or_change_focus_on_shaping() {
    let mut settings = crate::depth_parallax::DepthParallaxSettings::default();
    settings.focus = 0.37;
    for shape in [false, true] {
        settings.shape_depth = shape;
        settings.normalize();
        let p = DepthRenderParams::from_settings(
            &settings,
            0.25,
            egui::Vec2::ZERO,
            egui::Rect::EVERYTHING,
            0,
            1.,
        );
        assert_eq!(p.focus, 0.37);
        assert_eq!(p.focus_highlight, 0.);
        assert_eq!(
            f32::from_le_bytes(p.bytes()[44..48].try_into().unwrap()),
            0.
        );
        assert_eq!(p.with_focus_highlight(1.).focus_highlight, 1.);
        assert_eq!(settings.focus, 0.37);
    }
}

#[test]
fn depth_focus_gpu_clicked_surface_stays_fixed_for_planes_and_continuous_depth() {
    use crate::egui037_stage4_gpu_tests::{FORMAT, OffscreenGpu, callback};
    let mut gpu = OffscreenGpu::new();
    let ctx = egui::Context::default();
    let size = [64, 48];
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(64., 48.));
    let source = ctx.load_texture(
        "focus patterned color",
        egui::ColorImage::new(
            [64, 48],
            (0..48)
                .flat_map(|y| {
                    (0..64).map(move |x| {
                        egui::Color32::from_rgb(
                            20 + x * 3,
                            30 + y * 4,
                            if (x / 8 + y / 8) % 2 == 0 { 40 } else { 180 },
                        )
                    })
                })
                .collect(),
        ),
        egui::TextureOptions::LINEAR,
    );
    // Register the managed source in the real renderer.
    super::tests::render_egui_frame(&mut gpu, &ctx, size, |_| {});
    let key = DepthSourceKey {
        texture_id: source.id(),
    };
    for continuous in [false, true] {
        let backing = gpu
            .renderer
            .texture(&source.id())
            .unwrap()
            .texture
            .as_ref()
            .unwrap();
        let color = Arc::new(
            DepthGpu::new(&gpu.device)
                .copy_color(&gpu.device, &gpu.queue, backing)
                .unwrap(),
        );
        let depth = Arc::new(
            DepthTexture::upload(
                &gpu.device,
                &gpu.queue,
                DepthMap {
                    key,
                    width: 64,
                    height: 48,
                    median: 0.25,
                    values: (0..48)
                        .flat_map(|y| {
                            (0..64).map(move |x| {
                                half::f16::from_f32(if continuous {
                                    0.15 + 0.5 * x as f32 / 63. + 0.15 * y as f32 / 47.
                                } else if x < 32 {
                                    0.25
                                } else {
                                    0.75
                                })
                            })
                        })
                        .collect(),
                },
            )
            .unwrap(),
        );
        let resource = Arc::new(DepthRenderResources::new(
            &gpu.device,
            ViewerContextId::for_test(921),
            key,
            color,
            depth.clone(),
        ));
        gpu.renderer
            .callback_resources
            .insert(DepthRenderResourcesRef(resource.clone()));
        for q in 0..4 {
            for crop in [false, true] {
                let mut p = params();
                p.quarter_turns = q;
                p.aspect = rect.aspect_ratio();
                p.gamma = depth_gamma(true, 0.25);
                if crop {
                    p.uv = egui::Rect::from_min_max(egui::pos2(0.1, 0.2), egui::pos2(0.9, 0.8));
                }
                // Deliberately away from depth discontinuities and color-pattern edges.
                let (x, y) = (20, 17);
                p.focus = focus_depth_at(
                    egui::pos2(x as f32 + 0.5, y as f32 + 0.5),
                    rect,
                    p,
                    depth.width,
                    depth.height,
                    &depth.values,
                )
                .unwrap();
                let job = DepthShaderCallback {
                    context: resource.context,
                    key,
                    resource_id: resource.resource_id,
                    params: p,
                    target_format: FORMAT,
                };
                let still = gpu.render(&[callback(rect, job.clone())], size, [0, 0, 0, 0]);
                for sign in [-1., 1.] {
                    let mut moving = job.clone();
                    moving.params.offset = egui::vec2(0.04 * sign, -0.02 * sign);
                    let pixels = gpu.render(&[callback(rect, moving)], size, [0, 0, 0, 0]);
                    for (a, b) in pixels[y * 64 + x].into_iter().zip(still[y * 64 + x]) {
                        assert!(
                            a.abs_diff(b) <= 2,
                            "q={q}, crop={crop}, continuous={continuous}: {:?} != {:?}",
                            pixels[y * 64 + x],
                            still[y * 64 + x]
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn depth_focus_gpu_highlight_band_preserves_premultiplied_alpha() {
    use crate::egui037_stage4_gpu_tests::{OffscreenGpu, callback};
    let mut gpu = OffscreenGpu::new();
    let ctx = egui::Context::default();
    let size = [64, 48];
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(64., 48.));
    for alpha in [0, 128, 255] {
        let c = egui::Color32::from_rgba_premultiplied(alpha / 2, alpha / 4, alpha / 8, alpha);
        let source = ctx.load_texture(
            "highlight alpha",
            egui::ColorImage::filled([64, 48], c),
            egui::TextureOptions::LINEAR,
        );
        super::tests::render_egui_frame(&mut gpu, &ctx, size, |_| {});
        let transform = super::tests::cropped_quarter_turn(rect, params().uv);
        let mut job = super::tests::install_two_depth_planes(&mut gpu, source.id(), &transform);
        job.params.quarter_turns = 0;
        job.params.focus = 0.25;
        let still = gpu.render(&[callback(rect, job.clone())], size, [0, 0, 0, 0]);
        job.params = job.params.with_focus_highlight(1.);
        let highlighted = gpu.render(&[callback(rect, job)], size, [0, 0, 0, 0]);
        for (x, band) in [(10, true), (50, false)] {
            let a = alpha as f32;
            let base = [a / 2., (alpha / 4) as f32, (alpha / 8) as f32];
            let target = [0., 0.85 * a, a];
            let expected = std::array::from_fn::<_, 3, _>(|i| {
                if band {
                    base[i] * 0.8 * 0.45 + target[i] * 0.55
                } else {
                    base[i] * 0.8
                }
            });
            let pixel = highlighted[20 * 64 + x];
            assert_eq!(pixel[3], alpha);
            assert_eq!(still[20 * 64 + x], c.to_array());
            for i in 0..3 {
                assert!((pixel[i] as f32 - expected[i]).abs() <= 1.1);
                assert!(pixel[i] <= alpha);
            }
        }
    }
}
