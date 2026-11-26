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
        placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
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
    let params = PlaneRenderParams::screen(
        &t,
        canvas,
        Vec2::splat(0.5),
        strength,
        lens,
        PlaneTilt::default(),
        material.description,
        &crate::ui_fullscreen::FsBgStyle::Solid(egui::Color32::WHITE),
        egui::Color32::from_rgb(13, 23, 37),
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
                let request = request(&gpu, &mut cache, material.clone(), canvas, ppp, lens, 0.7)
                    .with_choice(choice);
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
                        0.7,
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
        r.snapshot = Arc::new(PlaneFrameSnapshot {
            context: r.snapshot.context,
            anchor: r.snapshot.anchor,
            material: material.clone(),

            params: PlaneRenderParams::screen(
                &t,
                canvas,
                Vec2::splat(0.5),
                0.2,
                PlaneLensKind::Equidistant,
                PlaneTilt::default(),
                description,
                &crate::ui_fullscreen::FsBgStyle::Solid(egui::Color32::WHITE),
                egui::Color32::from_rgb(13, 23, 37),
            ),
        });
        let bytes = r.render().unwrap();
        for (x, y) in [(64, 48), (48, 35), (80, 60), (5, 5)] {
            let p = canvas.min + Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            let uv = camera_point(
                p,
                canvas,
                Vec2::splat(0.5),
                0.2,
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
