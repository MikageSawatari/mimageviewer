//! Real shared shader, two material groups and private frame uniforms, without a window.
use super::*;
use crate::displayed_image_transform::*;
use crate::egui037_stage4_gpu_tests::OffscreenGpu;
use crate::plane_view::*;

fn full_uv() -> Rect {
    Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.))
}
fn transform(canvas: Rect, ppp: f32) -> DisplayedImageTransform {
    DisplayedImageTransform::resolve(DisplayedImageTransformInput {
        page_idx: 0,
        viewport_rect: canvas,
        source_size: Vec2::new(64., 48.),
        texture_size: Vec2::new(64., 48.),
        rotation: crate::rotation_db::Rotation::None,
        free_rotation_rad: 0.,
        content_bbox: None,
        fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
            crate::settings::FullscreenFitMode::Page,
        ),
        fit_scale_limits: Default::default(),
        pixels_per_point: ppp,
        placement: ResolvedDisplayPlacement::Normal {
            zoom_pan: Some((2., Vec2::ZERO)),
        },
        pixel_fit: RectPixelFit::Proportional,
    })
    .unwrap()
}
fn material(
    gpu: &OffscreenGpu,
    cache: &mut PlaneRendererCache,
    id: u64,
    alpha: bool,
) -> Arc<PlaneMaterial> {
    let source = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("plane test ramp"),
        size: wgpu::Extent3d {
            width: 64,
            height: 48,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let bytes: Vec<u8> = (0..48)
        .flat_map(|y| {
            (0..64).flat_map(move |x| {
                if alpha {
                    [0, 0, 0, 0]
                } else {
                    [x * 3, y * 4, 50, 255]
                }
            })
        })
        .collect();
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &source,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(256),
            rows_per_image: Some(48),
        },
        source.size(),
    );
    let copy = cache
        .copy(
            &gpu.device,
            &gpu.queue,
            egui::TextureId::Managed(id),
            &source,
            false,
        )
        .unwrap();
    PlaneMaterial::with_description(
        copy,
        PlaneMaterialDescription {
            source_size: [64, 48],
            texture_size: Vec2::new(64., 48.),
            coverage: full_uv(),
            sampler_roi: full_uv(),
            display_basis: crate::material_display_basis::MaterialDisplayBasis::from_sampler(
                [64, 48],
                crate::source_coverage::SourceCoverage::FullSource,
            ),
            material_kind: crate::gpu_lanczos::FullscreenPageMaterialKind::Complete,
        },
    )
}
fn request(
    gpu: &OffscreenGpu,
    cache: &mut PlaneRendererCache,
    material: Arc<PlaneMaterial>,
    canvas: Rect,
    ppp: f32,
    lens: PlaneLensKind,
    strength: f32,
) -> PlaneFrameRequest {
    let t = transform(canvas, ppp);
    request_for_transform(
        gpu,
        cache,
        material,
        &t,
        canvas,
        Vec2::splat(0.5),
        lens,
        strength,
    )
}
fn request_for_transform(
    gpu: &OffscreenGpu,
    cache: &mut PlaneRendererCache,
    material: Arc<PlaneMaterial>,
    t: &DisplayedImageTransform,
    canvas: Rect,
    center: Vec2,
    lens: PlaneLensKind,
    strength: f32,
) -> PlaneFrameRequest {
    let ppp = t.pixels_per_point;
    let (params, k_fill) = PlaneRenderParams::screen_with_fill_mode(
        t,
        canvas,
        center,
        strength,
        lens,
        PlaneTilt::default(),
        material.description,
        &crate::ui_fullscreen::FsBgStyle::Solid(egui::Color32::WHITE),
        egui::Color32::from_rgb(13, 23, 37),
        PlaneFillMode::Auto,
    );
    let snapshot = Arc::new(PlaneFrameSnapshot {
        context: crate::app::ViewerContextId::for_test(917),
        anchor: PlaneViewAnchor {
            fs_idx: 0,
            item: crate::item_identity::ItemId::NONE,
            items_generation: 1,
            slice: crate::page_split::PageSlice::Full,
        },
        material,
        params,
        k_fill,
    });
    PlaneFrameRequest {
        size: resolve_plane_output_size(
            canvas.size() * ppp,
            PlaneOutputChoice::Canvas,
            &gpu.device.limits(),
        ),
        snapshot,
        device: gpu.device.clone(),
        queue: gpu.queue.clone(),
        pipeline: cache.pipeline(&gpu.device, wgpu::TextureFormat::Rgba8Unorm),
    }
}

fn set_tilt(
    request: &mut PlaneFrameRequest,
    transform: &DisplayedImageTransform,
    canvas: Rect,
    lens: PlaneLensKind,
    strength: f32,
    tilt: PlaneTilt,
) {
    let old = &request.snapshot;
    let (params, k_fill) = PlaneRenderParams::screen_with_fill_mode(
        transform,
        canvas,
        Vec2::splat(0.5),
        strength,
        lens,
        tilt,
        old.material.description,
        &crate::ui_fullscreen::FsBgStyle::Solid(egui::Color32::WHITE),
        egui::Color32::from_rgb(13, 23, 37),
        PlaneFillMode::Locked(old.k_fill),
    );
    request.snapshot = Arc::new(PlaneFrameSnapshot {
        context: old.context,
        anchor: old.anchor,
        material: old.material.clone(),
        params,
        k_fill,
    });
}

#[test]
fn plane_tilt_gpu_all_pixels_match_two_stage_cpu_including_backfaces_and_edge_on() {
    let gpu = OffscreenGpu::new();
    let mut cache = PlaneRendererCache::default();
    let material = material(&gpu, &mut cache, 948, false);
    let mut poses = vec![
        PlaneTilt::default(),
        PlaneTilt {
            q: [0., 0., 1., 0.],
        },
        PlaneTilt {
            q: [0., 1., 0., 0.],
        },
        PlaneTilt::from_rotation_vector([0.3, 0.5, -0.2]),
        PlaneTilt::from_rotation_vector([0.005_f32.to_radians(), 0., 0.]),
    ];
    for degrees in [45_f32, 89., 90., 91., 100., 180., 270.] {
        poses.push(PlaneTilt::from_rotation_vector([
            0.,
            degrees.to_radians(),
            0.,
        ]));
    }
    poses.push(PlaneTilt::from_rotation_vector([
        270_f32.to_radians(),
        0.,
        0.,
    ]));
    for lens in PlaneLensKind::ALL {
        for density in [1., 1.5] {
            let canvas = Rect::from_min_size(egui::pos2(23., 41.), Vec2::new(48., 36.));
            let transform = transform(canvas, density);
            for strength in [0., 0.75] {
                for tilt in &poses {
                    let mut request = request_for_transform(
                        &gpu,
                        &mut cache,
                        material.clone(),
                        &transform,
                        canvas,
                        Vec2::splat(0.5),
                        lens,
                        strength,
                    );
                    set_tilt(&mut request, &transform, canvas, lens, strength, *tilt);
                    let rgba = request.render().unwrap();
                    let [width, height] = request.size;
                    for y in 0..height {
                        for x in 0..width {
                            let p = canvas.min * density
                                + Vec2::new(
                                    (x as f32 + 0.5) * canvas.width() * density / width as f32,
                                    (y as f32 + 0.5) * canvas.height() * density / height as f32,
                                );
                            let (q, valid) = camera_sample(
                                p,
                                canvas * density,
                                Vec2::splat(0.5),
                                request.snapshot.params.values[2][0],
                                lens,
                                *tilt,
                            );
                            assert!(
                                q.is_finite(),
                                "masked rays must remain finite before derivatives"
                            );
                            let uv = valid
                                .then(|| {
                                    texture_point(q / density, &transform, full_uv(), full_uv())
                                })
                                .flatten();
                            let pixel = &rgba[((y * width + x) * 4) as usize
                                ..((y * width + x) * 4 + 4) as usize];
                            // CPU/WGSL f32 may choose opposite sides within 1/64 physical pixel of a material edge.
                            let bounds = transform.paint_rect;
                            let local = q / density;
                            let edge_distance = [
                                (local.x - bounds.left()).abs(),
                                (local.x - bounds.right()).abs(),
                                (local.y - bounds.top()).abs(),
                                (local.y - bounds.bottom()).abs(),
                            ]
                            .into_iter()
                            .fold(f32::INFINITY, f32::min)
                                * density;
                            if valid && edge_distance <= 1. / 64. {
                                continue;
                            }
                            if uv.is_some() {
                                assert!(
                                    pixel[2].abs_diff(50) <= 1,
                                    "{lens:?}, {tilt:?}, {density}, {strength}, ({x},{y}), {pixel:?}"
                                );
                            } else {
                                assert_eq!(
                                    pixel,
                                    &[13, 23, 37, 255],
                                    "{lens:?}, {tilt:?}, ({x},{y})"
                                );
                            }
                            assert_eq!(pixel[3], 255);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn plane_tilt_snapshot_matrix_preserves_screen_copy_and_export_pixels() {
    let mut gpu = OffscreenGpu::new();
    let mut cache = PlaneRendererCache::default();
    let material = material(&gpu, &mut cache, 949, false);
    let canvas = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(128., 96.));
    let transform = transform(canvas, 1.);
    for tilt in [
        PlaneTilt::from_rotation_vector([0.3, 0.5, -0.2]),
        PlaneTilt {
            q: [0., 0., 1., 0.],
        },
        PlaneTilt {
            q: [0., 1., 0., 0.],
        },
        PlaneTilt::from_rotation_vector([0., 90_f32.to_radians(), 0.]),
    ] {
        for lens in PlaneLensKind::ALL {
            for strength in [0., 0.75] {
                let mut request = request_for_transform(
                    &gpu,
                    &mut cache,
                    material.clone(),
                    &transform,
                    canvas,
                    Vec2::splat(0.5),
                    lens,
                    strength,
                );
                set_tilt(&mut request, &transform, canvas, lens, strength, tilt);
                let job = crate::egui037_stage4_gpu_tests::callback(
                    canvas,
                    PlaneShaderCallback::new(
                        &gpu.device,
                        request.pipeline.clone(),
                        request.snapshot.clone(),
                    ),
                );
                let screen: Vec<u8> = gpu
                    .render(&[job], [128, 96], [13, 23, 37, 255])
                    .into_iter()
                    .flatten()
                    .collect();
                assert_eq!(screen, request.render().unwrap());
                for scale in [
                    crate::export_dialog::ExportScale::Full,
                    crate::export_dialog::ExportScale::Half,
                ] {
                    let output = request.with_choice(PlaneOutputChoice::Export(scale));
                    assert!(Arc::ptr_eq(&request.snapshot, &output.snapshot));
                    let params = output.snapshot.params.for_output(output.size);
                    for column in 13..16 {
                        assert_eq!(
                            params.values[column],
                            request.snapshot.params.values[column]
                        );
                    }
                    assert_eq!(
                        params.values[2][0..3],
                        request.snapshot.params.values[2][0..3]
                    );
                    let pixels = output.render().unwrap();
                    if output.size == request.size {
                        assert_eq!(pixels, screen);
                    }
                    if tilt.matrix()[2][2].abs() <= 1.0e-6 {
                        assert!(pixels.chunks_exact(4).all(|p| p == [13, 23, 37, 255]));
                    }
                }
            }
        }
    }
}
#[test]
fn plane_export_gpu_matches_cpu_camera_at_multiple_origins_dpi_and_output_sizes() {
    let gpu = OffscreenGpu::new();
    let mut cache = PlaneRendererCache::default();
    let material = material(&gpu, &mut cache, 917, false);
    for lens in PlaneLensKind::ALL {
        for ppp in [1., 1.5] {
            let canvas = Rect::from_min_size(egui::pos2(23., 41.), Vec2::new(128., 96.));
            for choice in [
                PlaneOutputChoice::Canvas,
                PlaneOutputChoice::Export(crate::export_dialog::ExportScale::Full),
                PlaneOutputChoice::Export(crate::export_dialog::ExportScale::Half),
                PlaneOutputChoice::Export(crate::export_dialog::ExportScale::Quarter),
                PlaneOutputChoice::Export(crate::export_dialog::ExportScale::LongEdge(100)),
            ] {
                let screen_request =
                    request(&gpu, &mut cache, material.clone(), canvas, ppp, lens, 0.7);
                let request = screen_request.with_choice(choice);
                assert!(Arc::ptr_eq(&screen_request.snapshot, &request.snapshot));
                assert!(request.snapshot.params.values[2][0] > 0.);
                assert_eq!(
                    request.snapshot.params.for_output(request.size).values[2][0],
                    request.snapshot.params.values[2][0],
                    "output resizing keeps the displayed coefficient"
                );
                let rgba = request.render().unwrap();
                let [w, h] = request.size;
                for (x, y) in [
                    (w / 2, h / 2),
                    (w / 3, h / 3),
                    (w * 2 / 3, h * 2 / 3),
                    (0, 0),
                ] {
                    let p = canvas.min * ppp
                        + Vec2::new(
                            (x as f32 + 0.5) * canvas.width() * ppp / w as f32,
                            (y as f32 + 0.5) * canvas.height() * ppp / h as f32,
                        );
                    let expected = camera_point(
                        p,
                        canvas * ppp,
                        Vec2::splat(0.5),
                        request.snapshot.params.values[2][0],
                        lens,
                        PlaneTilt::default(),
                    )
                    .and_then(|q| {
                        texture_point(q / ppp, &transform(canvas, ppp), full_uv(), full_uv())
                    });
                    let pixel = &rgba[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize];
                    match expected {
                        Some(uv) => {
                            let r = ((uv.x * 64. - 0.5).clamp(0., 63.) * 3.).round() as u8;
                            let g = ((uv.y * 48. - 0.5).clamp(0., 47.) * 4.).round() as u8;
                            assert!(
                                pixel[0].abs_diff(r) <= 4 && pixel[1].abs_diff(g) <= 4,
                                "{lens:?} {ppp} {choice:?} ({x},{y}) {pixel:?} != {r},{g}"
                            );
                            assert_eq!(pixel[3], 255);
                        }
                        None => assert_eq!(pixel, &[13, 23, 37, 255]),
                    }
                }
            }
        }
    }
}
#[test]
fn plane_gpu_pipeline_formats_and_sibling_callback_uniforms_are_independent() {
    let mut gpu = OffscreenGpu::new();
    let mut cache = PlaneRendererCache::default();
    let sibling_material = material(&gpu, &mut cache, 919, true);
    let material = material(&gpu, &mut cache, 918, false);
    let rgba = cache.pipeline(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    let bgra = cache.pipeline(&gpu.device, wgpu::TextureFormat::Bgra8Unorm);
    assert_eq!(bgra.format, wgpu::TextureFormat::Bgra8Unorm);
    assert!(!Arc::ptr_eq(&rgba, &bgra));
    assert!(Arc::ptr_eq(
        &rgba,
        &cache.pipeline(&gpu.device, wgpu::TextureFormat::Rgba8Unorm)
    ));
    let canvas = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(128., 96.));
    let a = request(
        &gpu,
        &mut cache,
        material.clone(),
        canvas,
        1.,
        PlaneLensKind::Equidistant,
        0.2,
    );
    let mut b = request(
        &gpu,
        &mut cache,
        sibling_material,
        canvas,
        1.,
        PlaneLensKind::Stereographic,
        1.,
    );
    let old = &b.snapshot;
    b.snapshot = Arc::new(PlaneFrameSnapshot {
        context: crate::app::ViewerContextId::for_test(918),
        anchor: old.anchor,
        material: old.material.clone(),
        params: old.params,
        k_fill: old.k_fill,
    });
    let device = gpu.device.clone();
    let make = |r: &PlaneFrameRequest| {
        crate::egui037_stage4_gpu_tests::callback(
            canvas,
            PlaneShaderCallback::new(&device, rgba.clone(), r.snapshot.clone()),
        )
    };
    let job_a = make(&a);
    let job_b = make(&b);
    let expected = gpu.render(&[job_a], [128, 96], [13, 23, 37, 255]);
    let actual = gpu.render(&[job_b, make(&a)], [128, 96], [13, 23, 37, 255]);
    assert_eq!(
        actual, expected,
        "later callback writes cannot replace its sibling's uniform"
    );
}

#[test]
fn plane_locked_snapshot_preserves_displayed_fill_and_gpu_pixels_for_copy_and_export() {
    let mut gpu = OffscreenGpu::new();
    let mut cache = PlaneRendererCache::default();
    let material = material(&gpu, &mut cache, 942, false);
    let canvas = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(128., 96.));
    for lens in PlaneLensKind::ALL {
        let original = transform(canvas, 1.);
        let (_, fixed) = PlaneRenderParams::screen_with_fill_mode(
            &original,
            canvas,
            Vec2::splat(0.5),
            0.75,
            lens,
            PlaneTilt::default(),
            material.description,
            &crate::ui_fullscreen::FsBgStyle::Solid(egui::Color32::WHITE),
            egui::Color32::from_rgb(13, 23, 37),
            PlaneFillMode::Auto,
        );
        assert!(fixed > 0.);
        let shrunk = DisplayedImageTransform::resolve(DisplayedImageTransformInput {
            page_idx: 0,
            viewport_rect: canvas,
            source_size: Vec2::new(64., 48.),
            texture_size: Vec2::new(64., 48.),
            rotation: crate::rotation_db::Rotation::None,
            free_rotation_rad: 0.,
            content_bbox: None,
            fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
                crate::settings::FullscreenFitMode::Page,
            ),
            fit_scale_limits: Default::default(),
            pixels_per_point: 1.,
            placement: ResolvedDisplayPlacement::Normal {
                zoom_pan: Some((0.5, Vec2::ZERO)),
            },
            pixel_fit: RectPixelFit::Proportional,
        })
        .unwrap();
        let auto = PlaneRenderParams::screen(
            &shrunk,
            canvas,
            Vec2::splat(0.5),
            0.75,
            lens,
            PlaneTilt::default(),
            material.description,
            &crate::ui_fullscreen::FsBgStyle::Solid(egui::Color32::WHITE),
            egui::Color32::from_rgb(13, 23, 37),
        );
        assert_eq!(auto.values[2][0], 0.);
        let (params, k_fill) = PlaneRenderParams::screen_with_fill_mode(
            &shrunk,
            canvas,
            Vec2::splat(0.5),
            0.75,
            lens,
            PlaneTilt::default(),
            material.description,
            &crate::ui_fullscreen::FsBgStyle::Solid(egui::Color32::WHITE),
            egui::Color32::from_rgb(13, 23, 37),
            PlaneFillMode::Locked(fixed),
        );
        assert_eq!(k_fill, fixed);
        assert_eq!(params.values[2][0], 0.75 * fixed);
        let mut screen = request_for_transform(
            &gpu,
            &mut cache,
            material.clone(),
            &shrunk,
            canvas,
            Vec2::splat(0.5),
            lens,
            0.75,
        );
        screen.snapshot = Arc::new(PlaneFrameSnapshot {
            context: screen.snapshot.context,
            anchor: screen.snapshot.anchor,
            material: material.clone(),
            params,
            k_fill,
        });
        let callback = crate::egui037_stage4_gpu_tests::callback(
            canvas,
            PlaneShaderCallback::new(
                &gpu.device,
                screen.pipeline.clone(),
                screen.snapshot.clone(),
            ),
        );
        let screenshot: Vec<u8> = gpu
            .render(&[callback], [128, 96], [13, 23, 37, 255])
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(
            screenshot,
            screen.render().unwrap(),
            "screen and copied canvas keep the same locked k"
        );
        for choice in [
            PlaneOutputChoice::Canvas,
            PlaneOutputChoice::Export(crate::export_dialog::ExportScale::Full),
            PlaneOutputChoice::Export(crate::export_dialog::ExportScale::Half),
        ] {
            let output = screen.with_choice(choice);
            assert!(Arc::ptr_eq(&screen.snapshot, &output.snapshot));
            assert_eq!(output.snapshot.k_fill, fixed);
            assert_eq!(
                output.snapshot.params.for_output(output.size).values[2][0],
                0.75 * fixed
            );
            let rgba = output.render().unwrap();
            let [w, h] = output.size;
            for (x, y) in [(w / 2, h / 2), (w / 3, h / 3), (0, 0)] {
                let p = egui::pos2(
                    (x as f32 + 0.5) * 128. / w as f32,
                    (y as f32 + 0.5) * 96. / h as f32,
                );
                let uv = camera_point(
                    p,
                    canvas,
                    Vec2::splat(0.5),
                    params.values[2][0],
                    lens,
                    PlaneTilt::default(),
                )
                .and_then(|q| texture_point(q, &shrunk, full_uv(), full_uv()));
                let pixel = &rgba[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize];
                match uv {
                    Some(uv) => {
                        let r = ((uv.x * 64. - 0.5).clamp(0., 63.) * 3.).round() as u8;
                        let g = ((uv.y * 48. - 0.5).clamp(0., 47.) * 4.).round() as u8;
                        assert!(pixel[0].abs_diff(r) <= 4 && pixel[1].abs_diff(g) <= 4);
                    }
                    None => assert_eq!(pixel, &[13, 23, 37, 255]),
                }
            }
        }
    }
}

#[test]
fn plane_checker_underlay_is_frozen_in_canvas_coordinates_at_double_output() {
    let gpu = OffscreenGpu::new();
    let mut cache = PlaneRendererCache::default();
    let material = material(&gpu, &mut cache, 925, true);
    let canvas = Rect::from_min_size(egui::pos2(31., 47.), Vec2::new(128., 96.));
    let mut request = request(
        &gpu,
        &mut cache,
        material,
        canvas,
        1.,
        PlaneLensKind::Equidistant,
        0.1,
    );
    let old = &request.snapshot;
    let mut params = old.params;
    params.values[9][3] = 1.;
    params.values[10] = [31., 47., 8., 8.];
    request.snapshot = Arc::new(PlaneFrameSnapshot {
        context: old.context,
        anchor: old.anchor,
        material: old.material.clone(),
        params,
        k_fill: old.k_fill,
    });
    for choice in [
        PlaneOutputChoice::Canvas,
        PlaneOutputChoice::Export(crate::export_dialog::ExportScale::Full),
    ] {
        let r = request.with_choice(choice);
        let bytes = r.render().unwrap();
        let [w, h] = r.size;
        for (nx, ny) in [(0.4, 0.4), (0.5, 0.4), (0.6, 0.6)] {
            let x = (nx * w as f32) as u32;
            let y = (ny * h as f32) as u32;
            let cx = (x as f32 + 0.5) * 128. / w as f32;
            let cy = (y as f32 + 0.5) * 96. / h as f32;
            let gray = if ((cx / 8.).floor() as u32 + (cy / 8.).floor() as u32) % 2 == 0 {
                224
            } else {
                176
            };
            assert_eq!(
                &bytes[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize],
                &[gray, gray, gray, 255]
            );
        }
    }
}
#[test]
fn plane_material_metadata_reuses_copy_and_three_retained_generations_count_once() {
    let gpu = OffscreenGpu::new();
    let mut cache = PlaneRendererCache::default();
    let baseline = resident_bytes();
    let mut all = Vec::new();
    let mut weak = Vec::new();
    let mut bytes = 0;
    for id in 0..3 {
        let first = material(&gpu, &mut cache, 920 + id, false);
        bytes += first.copy.gpu._color.byte_size();
        let mut description = first.description;
        description.coverage = Rect::from_min_max(egui::pos2(0.25, 0.), egui::pos2(0.75, 1.));
        let updated = PlaneMaterial::with_description(first.copy.clone(), description);
        assert!(Arc::ptr_eq(&first.copy, &updated.copy));
        assert_ne!(first.description, updated.description);
        weak.push(Arc::downgrade(&first.copy));
        all.push((first, updated));
    }
    assert_eq!(resident_bytes(), baseline + bytes);
    drop(all);
    assert!(weak.iter().all(|p| p.upgrade().is_none()));
    assert_eq!(resident_bytes(), baseline);
}

#[test]
fn plane_gpu_copy_rejects_unsupported_formats() {
    let gpu = OffscreenGpu::new();
    let source = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("plane invalid color"),
        size: wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    assert!(
        PlaneRendererCache::default()
            .copy(
                &gpu.device,
                &gpu.queue,
                egui::TextureId::Managed(930),
                &source,
                false
            )
            .is_err()
    );
}

#[test]
fn plane_gpu_partial_material_roi_trim_and_quarter_rotation_match_cpu() {
    let gpu = OffscreenGpu::new();
    let mut cache = PlaneRendererCache::default();
    let base = material(&gpu, &mut cache, 931, false);
    let mut description = base.description;
    description.coverage = Rect::from_min_max(egui::pos2(0.25, 0.), egui::pos2(0.75, 1.));
    description.sampler_roi = Rect::from_min_max(egui::pos2(0.1, 0.1), egui::pos2(0.9, 0.9));
    let material = PlaneMaterial::with_description(base.copy.clone(), description);
    let canvas = Rect::from_min_size(egui::pos2(19., 29.), Vec2::new(128., 96.));
    for rotation in [
        crate::rotation_db::Rotation::None,
        crate::rotation_db::Rotation::Cw90,
    ] {
        let t = DisplayedImageTransform::resolve(DisplayedImageTransformInput {
            page_idx: 0,
            viewport_rect: canvas,
            source_size: Vec2::new(64., 48.),
            texture_size: Vec2::new(64., 48.),
            rotation,
            free_rotation_rad: 0.2,
            content_bbox: Some(Rect::from_min_max(
                egui::pos2(0.1, 0.2),
                egui::pos2(0.8, 0.9),
            )),
            fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
                crate::settings::FullscreenFitMode::Page,
            ),
            fit_scale_limits: Default::default(),
            pixels_per_point: 1.,
            placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
            pixel_fit: RectPixelFit::Proportional,
        })
        .unwrap();
        let mut r = request(
            &gpu,
            &mut cache,
            material.clone(),
            canvas,
            1.,
            PlaneLensKind::Equidistant,
            0.2,
        );
        let (params, k_fill) = PlaneRenderParams::screen_with_fill_mode(
            &t,
            canvas,
            Vec2::splat(0.5),
            0.2,
            PlaneLensKind::Equidistant,
            PlaneTilt::default(),
            description,
            &crate::ui_fullscreen::FsBgStyle::Solid(egui::Color32::WHITE),
            egui::Color32::from_rgb(13, 23, 37),
            PlaneFillMode::Auto,
        );
        r.snapshot = Arc::new(PlaneFrameSnapshot {
            context: r.snapshot.context,
            anchor: r.snapshot.anchor,
            material: material.clone(),
            params,
            k_fill,
        });
        let bytes = r.render().unwrap();
        for (x, y) in [(64, 48), (48, 35), (80, 60), (5, 5)] {
            let p = canvas.min + Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            let uv = camera_point(
                p,
                canvas,
                Vec2::splat(0.5),
                r.snapshot.params.values[2][0],
                PlaneLensKind::Equidistant,
                PlaneTilt::default(),
            )
            .and_then(|q| texture_point(q, &t, description.coverage, description.sampler_roi));
            let i = ((y * 128 + x) * 4) as usize;
            if let Some(uv) = uv {
                let red = ((uv.x * 64. - 0.5).clamp(0., 63.) * 3.).round() as u8;
                let green = ((uv.y * 48. - 0.5).clamp(0., 47.) * 4.).round() as u8;
                assert!(bytes[i].abs_diff(red) <= 4 && bytes[i + 1].abs_diff(green) <= 4);
            } else {
                assert_eq!(&bytes[i..i + 4], &[13, 23, 37, 255]);
            }
        }
    }
}

#[test]
fn plane_gpu_fill_has_no_background_with_partial_trim_rotation_and_offcenter_lenses() {
    let gpu = OffscreenGpu::new();
    let mut cache = PlaneRendererCache::default();
    let base = material(&gpu, &mut cache, 933, false);
    let ppp = 1.25;
    for canvas_size in [Vec2::new(128., 72.), Vec2::new(72., 128.)] {
        let canvas = Rect::from_min_size(egui::pos2(19., 29.), canvas_size);
        for (source_size, rotation, angle, partial) in [
            (
                canvas_size,
                crate::rotation_db::Rotation::None,
                0.0_f32,
                false,
            ),
            (
                Vec2::new(48., 96.),
                crate::rotation_db::Rotation::Cw90,
                0.,
                true,
            ),
            (
                Vec2::new(160., 48.),
                crate::rotation_db::Rotation::None,
                17.,
                true,
            ),
            (
                Vec2::new(48., 96.),
                crate::rotation_db::Rotation::Cw90,
                17.,
                true,
            ),
        ] {
            let mut description = base.description;
            description.source_size = [source_size.x as usize, source_size.y as usize];
            description.texture_size = source_size;
            description.display_basis =
                crate::material_display_basis::MaterialDisplayBasis::from_sampler(
                    description.source_size,
                    crate::source_coverage::SourceCoverage::FullSource,
                );
            if partial {
                description.coverage =
                    Rect::from_min_max(egui::pos2(0.15, 0.1), egui::pos2(0.85, 0.9));
                description.sampler_roi =
                    Rect::from_min_max(egui::pos2(0.1, 0.05), egui::pos2(0.9, 0.95));
            }
            let material = PlaneMaterial::with_description(base.copy.clone(), description);
            let resolve = |zoom, pan, pixel_fit| {
                DisplayedImageTransform::resolve(DisplayedImageTransformInput {
                    page_idx: 0,
                    viewport_rect: canvas,
                    source_size,
                    texture_size: source_size,
                    rotation,
                    free_rotation_rad: angle.to_radians(),
                    content_bbox: Some(Rect::from_min_max(
                        egui::pos2(0.2, 0.15),
                        egui::pos2(0.8, 0.85),
                    )),
                    fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
                        crate::settings::FullscreenFitMode::Page,
                    ),
                    fit_scale_limits: Default::default(),
                    pixels_per_point: ppp,
                    placement: ResolvedDisplayPlacement::Normal {
                        zoom_pan: Some((zoom, pan)),
                    },
                    pixel_fit,
                })
                .unwrap()
            };
            let unit = resolve(1., Vec2::ZERO, RectPixelFit::Proportional);
            let quad = plane_effective_quad(&unit, description).unwrap();
            let x = quad[1] - quad[0];
            let y = quad[3] - quad[0];
            let x_axis = x.normalized();
            let y_axis = y.normalized();
            // Cover with four physical pixels to spare before testing the real Texels path.
            let cover_size = canvas.size() + Vec2::splat(8. / ppp);
            let cover_zoom = ((cover_size.x * x_axis.x.abs() + cover_size.y * x_axis.y.abs())
                / x.length())
            .max((cover_size.x * y_axis.x.abs() + cover_size.y * y_axis.y.abs()) / y.length());
            for center in [
                Vec2::splat(0.5),
                Vec2::new(0.04, 0.5),
                Vec2::new(0.04, 0.06),
                Vec2::ZERO,
            ] {
                let c = canvas.min + center * canvas.size();
                for scale in [1., 1.1, 2., 5.] {
                    let zoom = cover_zoom * scale;
                    let unpanned = resolve(zoom, Vec2::ZERO, RectPixelFit::Proportional);
                    let q = plane_effective_quad(&unpanned, description).unwrap();
                    let q_center = q[0] + (q[2] - q[0]) * 0.5;
                    let desired_center = c + (canvas.center() - c) * scale;
                    let t = resolve(zoom, desired_center - q_center, RectPixelFit::Texels);
                    for lens in PlaneLensKind::ALL {
                        let r = request_for_transform(
                            &gpu,
                            &mut cache,
                            material.clone(),
                            &t,
                            canvas,
                            center,
                            lens,
                            1.,
                        );
                        let k = r.snapshot.params.values[2][0];
                        assert!(
                            k > 0.,
                            "{canvas_size:?} {rotation:?} {angle} {scale} {center:?}"
                        );
                        let bytes = r.render().unwrap();
                        let [width, height] = r.size;
                        for py in 0..height {
                            for px in 0..width {
                                let p = canvas.min * ppp
                                    + Vec2::new(
                                        (px as f32 + 0.5) * canvas.width() * ppp / width as f32,
                                        (py as f32 + 0.5) * canvas.height() * ppp / height as f32,
                                    );
                                let q = camera_point(
                                    p,
                                    canvas * ppp,
                                    center,
                                    k,
                                    lens,
                                    PlaneTilt::default(),
                                )
                                .unwrap();
                                let uv = texture_point(
                                    q / ppp,
                                    &t,
                                    description.coverage,
                                    description.sampler_roi,
                                )
                                .expect("every pixel center stays in the effective material");
                                let i = ((py * width + px) * 4) as usize;
                                let red = ((uv.x * 64. - 0.5).clamp(0., 63.) * 3.).round() as u8;
                                let green = ((uv.y * 48. - 0.5).clamp(0., 47.) * 4.).round() as u8;
                                assert!(
                                    bytes[i].abs_diff(red) <= 4
                                        && bytes[i + 1].abs_diff(green) <= 4
                                        && bytes[i + 2].abs_diff(50) <= 1
                                        && bytes[i + 3] == 255,
                                    "{lens:?} {canvas_size:?} {rotation:?} {angle} {scale} {center:?} ({px},{py})"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn plane_gpu_extreme_aspects_use_drawn_quad_for_fill_and_identity() {
    let gpu = OffscreenGpu::new();
    let mut cache = PlaneRendererCache::default();
    let base = material(&gpu, &mut cache, 934, false);
    let canvas = Rect::from_min_size(egui::pos2(19., 29.), Vec2::new(160., 90.));
    let center = Vec2::splat(0.5);
    let anchor = PlaneViewAnchor {
        fs_idx: 0,
        item: crate::item_identity::ItemId::NONE,
        items_generation: 1,
        slice: crate::page_split::PageSlice::Full,
    };
    for source_size in [
        Vec2::new(4096., 1.),
        Vec2::new(8192., 3.),
        Vec2::new(8192., 1.),
    ] {
        let mut description = base.description;
        description.source_size = [source_size.x as usize, source_size.y as usize];
        description.texture_size = source_size;
        description.display_basis =
            crate::material_display_basis::MaterialDisplayBasis::from_sampler(
                description.source_size,
                crate::source_coverage::SourceCoverage::FullSource,
            );
        let material = PlaneMaterial::with_description(base.copy.clone(), description);
        for ppp in [1., 1.25, 1.75] {
            for angle in [0.0_f32, std::f32::consts::FRAC_PI_2] {
                let cover = PlaneCoverInput {
                    transform: DisplayedImageTransformInput {
                        page_idx: 0,
                        viewport_rect: canvas,
                        source_size,
                        texture_size: source_size,
                        rotation: crate::rotation_db::Rotation::None,
                        free_rotation_rad: angle,
                        content_bbox: None,
                        fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
                            crate::settings::FullscreenFitMode::Height,
                        ),
                        fit_scale_limits: Default::default(),
                        pixels_per_point: ppp,
                        placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
                        pixel_fit: RectPixelFit::Texels,
                    },
                    layout_source_size: None,
                    material: description,
                };
                // The ordinary cover transition also handles these unsupported aspect ratios.
                let out = plane_view_wheel_transition(PlaneViewWheelInput {
                    current: None,
                    anchor,
                    zoom: 49.,
                    pan: Vec2::ZERO,
                    pointer: Some(canvas.center()),
                    canvas,
                    notches: 1.,
                    allowed: Ok(()),
                    step: crate::settings::FullscreenWheelZoomStep::Percent10,
                    tilt_is_identity: true,
                    tilt_drag_owned: false,
                    cover: Some(cover),
                });
                assert!(out.pose.is_some());
                let filled = cover
                    .resolve(out.zoom, out.pan, RectPixelFit::Texels)
                    .unwrap();
                let quad = plane_effective_quad(&filled, description).unwrap();
                // A later ordinary pan exposes the material edge, including very large offsets.
                let exposed_pan = out.pan + (canvas.center() - quad[0]);
                let exposed = cover
                    .resolve(out.zoom, exposed_pan, RectPixelFit::Texels)
                    .unwrap();
                for (t, should_cover) in [(filled, true), (exposed, false)] {
                    let q = plane_effective_quad(&t, description).unwrap();
                    let corners = [
                        canvas.left_top(),
                        canvas.right_top(),
                        canvas.right_bottom(),
                        canvas.left_bottom(),
                    ];
                    let actually_covers = corners.into_iter().all(|p| {
                        (0..4).all(|j| {
                            let a = q[j];
                            let b = q[(j + 1) % 4];
                            (f64::from(b.x) - f64::from(a.x)) * (f64::from(p.y) - f64::from(a.y))
                                - (f64::from(b.y) - f64::from(a.y))
                                    * (f64::from(p.x) - f64::from(a.x))
                                >= 0.
                        })
                    });
                    assert_eq!(
                        actually_covers, should_cover,
                        "{source_size:?} {ppp} {angle}"
                    );
                    for lens in PlaneLensKind::ALL {
                        let r = request_for_transform(
                            &gpu,
                            &mut cache,
                            material.clone(),
                            &t,
                            canvas,
                            center,
                            lens,
                            1.,
                        );
                        let k = r.snapshot.params.values[2][0];
                        let bytes = r.render().unwrap();
                        if actually_covers {
                            assert!(k > 0.);
                            let [width, height] = r.size;
                            for py in 0..height {
                                for px in 0..width {
                                    let p = canvas.min * ppp
                                        + Vec2::new(
                                            (px as f32 + 0.5) * canvas.width() * ppp / width as f32,
                                            (py as f32 + 0.5) * canvas.height() * ppp
                                                / height as f32,
                                        );
                                    let sample = camera_point(
                                        p,
                                        canvas * ppp,
                                        center,
                                        k,
                                        lens,
                                        PlaneTilt::default(),
                                    )
                                    .unwrap();
                                    assert!(
                                        texture_point(
                                            sample / ppp,
                                            &t,
                                            description.coverage,
                                            description.sampler_roi
                                        )
                                        .is_some()
                                    );
                                }
                            }
                            assert!(
                                bytes
                                    .chunks_exact(4)
                                    .all(|pixel| pixel[2].abs_diff(50) <= 1 && pixel[3] == 255)
                            );
                        } else {
                            assert_eq!(k, 0.);
                            let identity = request_for_transform(
                                &gpu,
                                &mut cache,
                                material.clone(),
                                &t,
                                canvas,
                                center,
                                lens,
                                0.,
                            );
                            assert_eq!(
                                bytes,
                                identity.render().unwrap(),
                                "outside support keeps the 2D pixels"
                            );
                            assert!(
                                bytes
                                    .chunks_exact(4)
                                    .any(|pixel| pixel == [13, 23, 37, 255])
                            );
                            assert!(
                                bytes
                                    .chunks_exact(4)
                                    .any(|pixel| pixel[2].abs_diff(50) <= 1)
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn plane_callback_and_output_request_keep_copy_alive_after_state_is_dropped() {
    let gpu = OffscreenGpu::new();
    let mut cache = PlaneRendererCache::default();
    let baseline = resident_bytes();
    let material = material(&gpu, &mut cache, 932, false);
    let bytes = material.copy.gpu._color.byte_size();
    let weak = Arc::downgrade(&material.copy);
    let r = request(
        &gpu,
        &mut cache,
        material.clone(),
        Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(128., 96.)),
        1.,
        PlaneLensKind::Equidistant,
        0.2,
    );
    let callback = PlaneShaderCallback::new(&gpu.device, r.pipeline.clone(), r.snapshot.clone());
    drop(material);
    assert_eq!(resident_bytes(), baseline + bytes);
    drop(r);
    assert!(weak.upgrade().is_some());
    drop(callback);
    assert!(weak.upgrade().is_none());
    assert_eq!(resident_bytes(), baseline);
}
