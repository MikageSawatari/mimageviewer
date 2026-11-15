use super::*;

fn stats(timings: LanczosGenerationTimings) -> LanczosGenerationStats {
    LanczosGenerationStats {
        source_size: [884, 444],
        target_size: [3840, 1928],
        smoothing_percent: 0,
        blur_factor: 1.0,
        texture_fetches: 123,
        encode_submit_cpu_ms: timings.accounted_ms() + 5.0,
        timings,
        regeneration_count: 1,
        scale_branch: FullscreenPaintScaleBranch::UpscaleLanczos,
        visible_source_region: Some([0.0, 0.0, 884.0, 444.0]),
    }
}

#[test]
fn regeneration_perf_fields_include_phases_and_only_new_resampler_breakdown() {
    // Production lazy-construction helpers, no GPU or logger I/O.
    let mut slot = None;
    let visible = OnceCell::new();
    for first in [true, false] {
        let mut timings = LanczosGenerationTimings {
            prepare_job_ms: 1.0,
            register_texture_ms: 2.0,
            encode_ms: 3.0,
            submit_ms: 4.0,
            ..Default::default()
        };
        get_or_create_resampler(&mut slot, &mut timings, || {
            (
                (),
                ResamplerCreationTimings::Lanczos3(LanczosCreationTimings {
                    bind_group_layout_ms: Some(0.0),
                    pipeline_layout_ms: Some(0.0),
                    ..Default::default()
                }),
            )
        });
        get_or_create_pipeline(
            &visible,
            &mut timings,
            LanczosPipelineBranch::VisibleUpscale,
            |steps| {
                steps.visible_upscale_shader_module_ms = Some(0.0);
                steps.visible_upscale_vertical_pipeline_ms = Some(0.0);
                steps.visible_upscale_horizontal_pipeline_ms = Some(0.0);
            },
        );
        let fields = fields(stats(timings));
        assert_eq!(fields["resampler_created"], first);
        assert_eq!(fields["pipeline_created"], first);
        assert_eq!(fields["prepare_job_ms"], 1.0);
        assert_eq!(fields["register_texture_ms"], 2.0);
        assert_eq!(fields["encode_ms"], 3.0);
        assert_eq!(fields["submit_ms"], 4.0);
        assert!((fields["unaccounted_ms"].as_f64().unwrap() - 5.0).abs() < 1e-9);
        assert_eq!(fields["source_w"], 884);
        assert_eq!(fields["scale_branch"], "upscale");
        assert_eq!(fields["visible_source_w"], 884.0);
        if first {
            let breakdown = &fields["resampler_create_breakdown"];
            assert_eq!(breakdown["kind"], "lanczos3");
            assert_eq!(breakdown["steps"].as_object().unwrap().len(), 2);
            assert_eq!(breakdown["branch"]["branch"], "visible_upscale");
            assert_eq!(breakdown["branch"]["steps"].as_object().unwrap().len(), 3);
        } else {
            assert_eq!(fields["resampler_create_ms"], 0.0);
            assert_eq!(fields["pipeline_create_ms"], 0.0);
            assert!(!fields.contains_key("resampler_create_breakdown"));
        }
    }
    let mut timings = LanczosGenerationTimings::default();
    get_or_create_resampler(&mut slot, &mut timings, || {
        panic!("layout owner must be reused")
    });
    get_or_create_pipeline(
        &OnceCell::new(),
        &mut timings,
        LanczosPipelineBranch::Nis,
        |steps| {
            steps.nis_shader_module_ms = Some(0.0);
            steps.nis_upscale_pipeline_ms = Some(0.0);
        },
    );
    let fields = fields(stats(timings));
    assert_eq!(fields["resampler_created"], false);
    assert_eq!(fields["pipeline_created"], true);
    let breakdown = &fields["resampler_create_breakdown"];
    assert!(
        breakdown.get("steps").is_none(),
        "no layout was created this call"
    );
    assert_eq!(breakdown["branch"]["branch"], "nis");
    assert_eq!(breakdown["branch"]["steps"].as_object().unwrap().len(), 2);

    // The prewarm consumer keeps its existing metadata/total and appends the same
    // timing fields: it must not lose a family creation on a later normal cache hit.
    let actual = stats(timings);
    let mut prewarm_fields = vec![("encode_submit_cpu_ms", actual.encode_submit_cpu_ms.into())];
    actual.append_timing_fields(&mut prewarm_fields);
    let count = prewarm_fields.len();
    let prewarm_fields: serde_json::Map<String, serde_json::Value> = prewarm_fields
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    assert_eq!(prewarm_fields.len(), count, "no duplicate total field");
    for (key, value) in prewarm_fields {
        assert_eq!(fields[key.as_str()], value, "shared timing field: {key}");
    }
}

fn fields(stats: LanczosGenerationStats) -> serde_json::Map<String, serde_json::Value> {
    stats
        .perf_fields()
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

#[test]
fn regeneration_accounting_counts_layout_pipeline_and_job_phases_once() {
    let mut actual = stats(LanczosGenerationTimings {
        resampler_create_ms: 1.0,
        pipeline_create_ms: 2.0,
        prepare_job_ms: 3.0,
        register_texture_ms: 4.0,
        encode_ms: 5.0,
        submit_ms: 6.0,
        ..Default::default()
    });
    actual.encode_submit_cpu_ms = 29.0;
    assert_eq!(fields(actual)["unaccounted_ms"], 8.0);
}

fn emit(value: serde_json::Value) {
    println!("MEASURE776 {value}");
}

fn headless_gpu() -> Option<(wgpu::Instance, wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::DX12;
    descriptor.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
    let instance = wgpu::Instance::new(descriptor);
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    })) {
        Ok(adapter) => adapter,
        Err(error) => {
            emit(serde_json::json!({"phase": "skip", "reason": format!("DX12 adapter: {error}")}));
            return None;
        }
    };
    let info = adapter.get_info();
    if info.device_type == wgpu::DeviceType::Cpu {
        emit(
            serde_json::json!({"phase": "skip", "reason": "only a software adapter is available"}),
        );
        return None;
    }
    emit(serde_json::json!({
        "phase": "adapter", "name": info.name, "backend": format!("{:?}", info.backend),
        "device_type": format!("{:?}", info.device_type), "compiler": "Fxc",
    }));
    match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("measure_776 DX12 FXC"),
        ..Default::default()
    })) {
        Ok((device, queue)) => Some((instance, adapter, device, queue)),
        Err(error) => {
            emit(serde_json::json!({"phase": "skip", "reason": format!("device: {error}")}));
            None
        }
    }
}

fn work(
    source_size: [u32; 2],
    target: [u32; 2],
    scale: f32,
    filter: PostFilter,
) -> LanczosWorkPlan {
    let key = requested_lanczos_key(
        FullscreenPaintSourceId::Page(0),
        egui::TextureId::Managed(1),
        FullscreenPaintSourceGeneration { items: 1, input: 1 },
        source_size,
        scale,
        1.0,
        Some(VisibleRegionRequest::full(target)),
        filter,
        0,
        AnimeUpscaleSourceLimit::default(),
    )
    .unwrap();
    work_plan_for_key(source_size, key).unwrap()
}

fn prepare(
    resampler: &Lanczos3Resampler,
    device: &wgpu::Device,
    source: &wgpu::Texture,
    work: LanczosWorkPlan,
    timings: &mut LanczosGenerationTimings,
) -> LanczosWorkJob {
    match work {
        LanczosWorkPlan::Full(plan) => LanczosWorkJob::Full(
            resampler
                .prepare_job(device, source, plan, timings)
                .unwrap(),
        ),
        LanczosWorkPlan::VisibleUpscale(plan) => LanczosWorkJob::VisibleUpscale(
            resampler
                .prepare_visible_upscale_job(device, source, plan, timings)
                .unwrap(),
        ),
        LanczosWorkPlan::NisUpscale(plan) => LanczosWorkJob::NisUpscale(
            resampler
                .prepare_nis_upscale_job(device, source, plan, timings)
                .unwrap(),
        ),
        LanczosWorkPlan::PixelArtUpscale(plan) => LanczosWorkJob::PixelArtUpscale(
            resampler
                .prepare_pixel_aa_job(device, source, plan, timings)
                .unwrap(),
        ),
        LanczosWorkPlan::AnimeUpscale(_) => panic!("Anime4K is outside the lazy Lanczos families"),
    }
}

fn encode(job: &LanczosWorkJob, encoder: &mut wgpu::CommandEncoder) {
    match job {
        LanczosWorkJob::Full(job) | LanczosWorkJob::VisibleUpscale(job) => job.encode(encoder),
        LanczosWorkJob::NisUpscale(job) => job.encode(encoder),
        LanczosWorkJob::PixelArtUpscale(job) => job.encode(encoder),
        LanczosWorkJob::AnimeUpscale(_) => panic!("Anime4K is outside the lazy Lanczos families"),
    }
}

fn source_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: [u32; 2],
    pixel: [u8; 4],
) -> wgpu::Texture {
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("measure_776 source"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &pixel.repeat((size[0] * size[1]) as usize),
    );
    queue.submit([]);
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .unwrap();
    texture
}

#[test]
#[ignore = "manual DX12/FXC real-GPU CPU timing probe; never launches the product"]
fn measure_776_dx12_fxc_first_resampler_and_upscale_jobs() {
    let Some((_instance, _adapter, device, queue)) = headless_gpu() else {
        return;
    };
    let source = source_texture(&device, &queue, [884, 444], [255; 4]);
    let mut slot = None;
    // The first row includes both layout-owner construction and only the default upscale family.
    for (branch, scale, filter, target, iteration) in [
        (
            "visible_upscale",
            3840.0 / 884.0,
            PostFilter::None,
            [3840, 1928],
            1,
        ),
        (
            "visible_upscale",
            3840.0 / 884.0,
            PostFilter::None,
            [3840, 1928],
            2,
        ),
        (
            "nis",
            3840.0 / 884.0,
            PostFilter::UpscaleSharp,
            [3840, 1928],
            1,
        ),
        ("downscale", 0.5, PostFilter::None, [442, 222], 1),
        (
            "pixel_art",
            3840.0 / 884.0,
            PostFilter::UpscalePixelArt,
            [3840, 1928],
            1,
        ),
    ] {
        let work = work([884, 444], target, scale, filter);
        let mut timings = LanczosGenerationTimings::default();
        let started = Instant::now();
        let resampler = get_or_create_resampler(&mut slot, &mut timings, || {
            let resampler = Lanczos3Resampler::new(&device);
            let creation = ResamplerCreationTimings::Lanczos3(resampler.creation_timings);
            (resampler, creation)
        });
        let job = prepare(resampler, &device, &source, work, &mut timings);
        let first_use_prepare_ms = started.elapsed().as_secs_f64() * 1000.0;
        // Keep the original per-job CPU/Wait measurements for the two default upscale jobs.
        let mut encode_ms = 0.0;
        let mut submit_ms = 0.0;
        let mut wait_ms = 0.0;
        if branch == "visible_upscale" {
            let commands = measure_resampler_cpu_ms(&mut encode_ms, || {
                let mut encoder = device.create_command_encoder(&Default::default());
                encode(&job, &mut encoder);
                encoder.finish()
            });
            let submission =
                measure_resampler_cpu_ms(&mut submit_ms, || queue.submit(Some(commands)));
            measure_resampler_cpu_ms(&mut wait_ms, || {
                device.poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: None,
                })
            })
            .expect("GPU completion for measure_776");
        }
        let mut value = serde_json::json!({
            "phase": "branch_first_use", "branch": branch, "iteration": iteration,
            "resampler_created": timings.creation.is_some(), "resampler_create_ms": timings.resampler_create_ms,
            "pipeline_created": timings.branch_creation.is_some(), "pipeline_create_ms": timings.pipeline_create_ms,
            "prepare_job_ms": timings.prepare_job_ms, "first_use_prepare_ms": first_use_prepare_ms,
            "unaccounted_ms": first_use_prepare_ms - timings.accounted_ms(),
            "source": work.source_size(), "target": work.target_size(), "source_region_px": work.visible_source_region(),
        });
        if let Some(breakdown) = timings.creation_breakdown() {
            value["resampler_create_breakdown"] = breakdown;
        }
        if branch == "visible_upscale" {
            value["encode_ms"] = encode_ms.into();
            value["submit_ms"] = submit_ms.into();
            value["gpu_completion_wait_ms"] = wait_ms.into();
        }
        emit(value);
    }
    let mut total_ms = 0.0;
    let anime = measure_resampler_cpu_ms(&mut total_ms, || {
        Anime4kResampler::new(&device, STILL_IMAGE_ANIME4K_VARIANT)
    });
    emit(
        serde_json::json!({"phase": "anime4k_new", "total_ms": total_ms, "steps": anime.creation_timings.perf_value()}),
    );
}

fn assert_prepared_pixels(
    paint: &mut crate::egui037_stage4_gpu_tests::OffscreenGpu,
    texture: &wgpu::Texture,
    size: [u32; 2],
    color: [u8; 4],
) {
    let id = paint.renderer.register_native_texture(
        &paint.device,
        &texture.create_view(&Default::default()),
        wgpu::FilterMode::Linear,
    );
    let rect =
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0] as f32, size[1] as f32));
    let mut mesh = egui::Mesh::with_texture(id);
    mesh.add_rect_with_uv(
        rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
    let pixels = paint.render(
        &[egui::ClippedPrimitive {
            clip_rect: rect,
            primitive: egui::epaint::Primitive::Mesh(mesh.into()),
        }],
        size,
        [0; 4],
    );
    // NIS deliberately treats the exterior as opaque black; use an interior pixel beyond its support.
    crate::egui037_stage4_gpu_tests::assert_rgba(
        pixels[(size[1] / 2 * size[0] + size[0] / 2) as usize],
        color,
    );
    paint.renderer.free_texture(&id);
}

#[test]
fn lazy_776_default_only_then_other_families_through_prewarm_and_resolve() {
    let Some((instance, adapter, device, queue)) = headless_gpu() else {
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let rs = egui_wgpu::RenderState {
        renderer: Arc::new(egui::epaint::mutex::RwLock::new(egui_wgpu::Renderer::new(
            &device,
            format,
            Default::default(),
        ))),
        instance,
        adapter,
        available_adapters: Vec::new(),
        device,
        queue,
        target_format: format,
    };
    let mut paint = crate::egui037_stage4_gpu_tests::OffscreenGpu {
        device: rs.device.clone(),
        queue: rs.queue.clone(),
        renderer: egui_wgpu::Renderer::new(&rs.device, format, Default::default()),
    };
    let ctx = egui::Context::default();
    let color = [80, 120, 160, 255];
    let image = egui::ColorImage::filled(
        [32, 32],
        egui::Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]),
    );
    let handle = ctx.load_texture("lazy-776", image.clone(), egui::TextureOptions::LINEAR);
    let resource = FullscreenPaintResource::resampleable(
        0,
        handle,
        FullscreenPaintSourceGeneration { items: 1, input: 1 },
    );
    rs.renderer.write().update_texture(
        &rs.device,
        &rs.queue,
        resource.source_texture_id(),
        &egui::epaint::ImageDelta::full(image, egui::TextureOptions::LINEAR),
    );
    let mut cache = GpuLanczosCache::default();
    for (index, (scale, filter, target, branch)) in [
        (
            2.0,
            PostFilter::None,
            [64, 64],
            LanczosPipelineBranch::VisibleUpscale,
        ),
        (
            3.0,
            PostFilter::None,
            [96, 96],
            LanczosPipelineBranch::VisibleUpscale,
        ),
        (
            2.0,
            PostFilter::UpscaleSharp,
            [64, 64],
            LanczosPipelineBranch::Nis,
        ),
        (
            3.0,
            PostFilter::UpscaleSharp,
            [96, 96],
            LanczosPipelineBranch::Nis,
        ),
        (
            0.5,
            PostFilter::None,
            [16, 16],
            LanczosPipelineBranch::Downscale,
        ),
        (
            0.25,
            PostFilter::None,
            [8, 8],
            LanczosPipelineBranch::Downscale,
        ),
        (
            2.0,
            PostFilter::UpscalePixelArt,
            [64, 64],
            LanczosPipelineBranch::PixelArt,
        ),
        (
            3.0,
            PostFilter::UpscalePixelArt,
            [96, 96],
            LanczosPipelineBranch::PixelArt,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let visible = Some(VisibleRegionRequest::full(target));
        let query = cache.query_prewarm(
            Some(&rs),
            &resource,
            scale,
            1.0,
            visible,
            filter,
            0,
            AnimeUpscaleSourceLimit::default(),
        );
        if index == 0 {
            assert!(
                cache.resampler.is_none(),
                "query cannot initialize any GPU family"
            );
        }
        let (output, actual) = if index == 0 {
            let NativePrewarmQuery::Miss(plan) = query else {
                panic!("first prewarm miss")
            };
            let NativePrewarmExecution::Submitted { stats, .. } =
                cache.execute_prewarm(&rs, &resource, plan, |_| true)
            else {
                panic!("prewarm submitted")
            };
            (cache.entries[&plan.key].output.clone(), stats)
        } else {
            let (prepared, event) = cache.resolve(
                Some(&rs),
                &resource,
                scale,
                1.0,
                visible,
                filter,
                0,
                AnimeUpscaleSourceLimit::default(),
            );
            let Some(LanczosPerfEvent::Generated(stats)) = event else {
                panic!("new target must regenerate")
            };
            (prepared.lanczos_output().unwrap().clone(), stats)
        };
        assert_eq!(actual.timings.creation.is_some(), index == 0);
        assert_eq!(
            actual.timings.branch_creation.map(|c| c.branch),
            (index % 2 == 0).then_some(branch)
        );
        let resampler = cache.resampler.as_ref().unwrap();
        assert!(resampler.visible_upscale.get().is_some());
        assert_eq!(resampler.nis.get().is_some(), index >= 2);
        assert_eq!(resampler.downscale.get().is_some(), index >= 4);
        assert_eq!(resampler.pixel_art.get().is_some(), index >= 6);
        let f = fields(actual);
        assert_eq!(f["resampler_created"], index == 0);
        assert_eq!(f["pipeline_created"], index % 2 == 0);
        if index % 2 != 0 {
            assert!(!f.contains_key("resampler_create_breakdown"));
        }
        assert_prepared_pixels(&mut paint, output._texture.as_ref().unwrap(), target, color);
    }
}

#[test]
fn lazy_776_prepared_jobs_encode_after_resampler_drop_for_all_families() {
    let Some((_instance, _adapter, device, queue)) = headless_gpu() else {
        return;
    };
    let color = [80, 120, 160, 255];
    let source = source_texture(&device, &queue, [32, 32], color);
    let jobs = {
        let resampler = Lanczos3Resampler::new(&device);
        [
            work([32, 32], [64, 64], 2.0, PostFilter::None),
            work([32, 32], [64, 64], 2.0, PostFilter::UpscaleSharp),
            work([32, 32], [16, 16], 0.5, PostFilter::None),
            work([32, 32], [64, 64], 2.0, PostFilter::UpscalePixelArt),
        ]
        .map(|work| {
            prepare(
                &resampler,
                &device,
                &source,
                work,
                &mut LanczosGenerationTimings::default(),
            )
        })
    }; // no resampler (or cell lookup) is available at encode
    let mut paint = crate::egui037_stage4_gpu_tests::OffscreenGpu {
        renderer: egui_wgpu::Renderer::new(
            &device,
            wgpu::TextureFormat::Rgba8Unorm,
            Default::default(),
        ),
        device,
        queue,
    };
    for job in jobs {
        let mut encoder = paint.device.create_command_encoder(&Default::default());
        encode(&job, &mut encoder);
        paint.queue.submit([encoder.finish()]);
        let size = match &job {
            LanczosWorkJob::Full(job) | LanczosWorkJob::VisibleUpscale(job) => {
                [job.output_texture.width(), job.output_texture.height()]
            }
            LanczosWorkJob::NisUpscale(job) => {
                [job.output_texture.width(), job.output_texture.height()]
            }
            LanczosWorkJob::PixelArtUpscale(job) => {
                [job.output_texture.width(), job.output_texture.height()]
            }
            LanczosWorkJob::AnimeUpscale(_) => unreachable!(),
        };
        assert_prepared_pixels(&mut paint, &job.into_output_texture(), size, color);
    }
}
