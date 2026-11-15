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
    // Exercise the production lazy-construction boundary without a GPU or logger I/O.
    let mut slot = None;
    let mut creations = 0;
    for first in [true, false] {
        let mut timings = LanczosGenerationTimings {
            prepare_job_ms: 1.0,
            register_texture_ms: 2.0,
            encode_ms: 3.0,
            submit_ms: 4.0,
            ..Default::default()
        };
        get_or_create_resampler(&mut slot, &mut timings, || {
            creations += 1;
            (
                (),
                ResamplerCreationTimings::Lanczos3(LanczosCreationTimings::default()),
            )
        });
        let fields: serde_json::Map<String, serde_json::Value> = stats(timings)
            .perf_fields()
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect();
        assert_eq!(fields["resampler_created"], first);
        assert_eq!(fields["prepare_job_ms"], 1.0);
        assert_eq!(fields["register_texture_ms"], 2.0);
        assert_eq!(fields["encode_ms"], 3.0);
        assert_eq!(fields["submit_ms"], 4.0);
        assert!((fields["unaccounted_ms"].as_f64().unwrap() - 5.0).abs() < 1e-9);
        assert!(fields["resampler_create_ms"].is_number());
        assert_eq!(fields["source_w"], 884);
        assert_eq!(fields["scale_branch"], "upscale");
        assert_eq!(fields["visible_source_w"], 884.0);
        if first {
            let breakdown = &fields["resampler_create_breakdown"];
            assert_eq!(breakdown["kind"], "lanczos3");
            let steps = breakdown["steps"].as_object().unwrap();
            assert_eq!(steps.len(), 12);
            for name in [
                "bind_group_layout_ms",
                "pipeline_layout_ms",
                "lanczos3_shader_module_ms",
                "visible_upscale_shader_module_ms",
                "nis_shader_module_ms",
                "pixel_aa_shader_module_ms",
                "vertical_pipeline_ms",
                "horizontal_pipeline_ms",
                "visible_upscale_vertical_pipeline_ms",
                "visible_upscale_horizontal_pipeline_ms",
                "nis_upscale_pipeline_ms",
                "pixel_aa_pipeline_ms",
            ] {
                assert!(steps[name].is_number(), "{name}");
            }
        } else {
            assert_eq!(fields["resampler_create_ms"], 0.0);
            assert!(!fields.contains_key("resampler_create_breakdown"));
        }
    }
    assert_eq!(creations, 1);
}

fn emit(value: serde_json::Value) {
    println!("MEASURE776 {value}");
}

fn measure_new(
    device: &wgpu::Device,
    device_number: u32,
    creation_number: u32,
) -> Lanczos3Resampler {
    let mut total_ms = 0.0;
    let resampler = measure_resampler_cpu_ms(&mut total_ms, || Lanczos3Resampler::new(device));
    let steps = serde_json::to_value(resampler.creation_timings).unwrap();
    let accounted_ms: f64 = steps
        .as_object()
        .unwrap()
        .values()
        .map(|v| v.as_f64().unwrap())
        .sum();
    emit(serde_json::json!({
        "phase": "lanczos3_new", "device": device_number, "creation": creation_number,
        "total_ms": total_ms, "steps": steps, "unaccounted_ms": total_ms - accounted_ms,
    }));
    resampler
}

#[test]
#[ignore = "manual DX12/FXC real-GPU CPU timing probe; never launches the product"]
fn measure_776_dx12_fxc_first_resampler_and_upscale_jobs() {
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
            return;
        }
    };
    let info = adapter.get_info();
    if info.device_type == wgpu::DeviceType::Cpu {
        emit(
            serde_json::json!({"phase": "skip", "reason": "only a software adapter is available"}),
        );
        return;
    }
    emit(serde_json::json!({
        "phase": "adapter", "name": info.name, "backend": format!("{:?}", info.backend),
        "device_type": format!("{:?}", info.device_type), "compiler": "Fxc",
    }));
    let request_device = || {
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("measure_776 DX12 FXC"),
            ..Default::default()
        }))
    };
    let (device, queue) = match request_device() {
        Ok(pair) => pair,
        Err(error) => {
            emit(serde_json::json!({"phase": "skip", "reason": format!("first device: {error}")}));
            return;
        }
    };
    let first = measure_new(&device, 1, 1);
    let _second = measure_new(&device, 1, 2);
    let (device2, _queue2) = match request_device() {
        Ok(pair) => pair,
        Err(error) => {
            emit(serde_json::json!({"phase": "skip", "reason": format!("second device: {error}")}));
            return;
        }
    };
    let _on_device2 = measure_new(&device2, 2, 1);

    // The same product request/key/work-plan functions choose the visible-upscale branch.
    let key = requested_lanczos_key(
        FullscreenPaintSourceId::Page(0),
        egui::TextureId::Managed(1),
        FullscreenPaintSourceGeneration { items: 1, input: 1 },
        [884, 444],
        3840.0 / 884.0,
        1.0,
        Some(VisibleRegionRequest::full([3840, 1928])),
        PostFilter::None,
        0,
        AnimeUpscaleSourceLimit::default(),
    )
    .unwrap();
    let work = work_plan_for_key([884, 444], key).unwrap();
    let LanczosWorkPlan::VisibleUpscale(plan) = work else {
        panic!("measurement must follow the product visible Lanczos upscale plan");
    };
    let source = device.create_texture_with_data(
        &queue,
        &wgpu::TextureDescriptor {
            label: Some("measure_776 884x444 source"),
            size: wgpu::Extent3d {
                width: 884,
                height: 444,
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
        &vec![255; 884 * 444 * 4],
    );
    // Upload/initialization is outside the measured jobs, as for an already-resident product source.
    queue.submit([]);
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("source upload completion for measure_776");
    for iteration in 1..=2 {
        let mut prepare_ms = 0.0;
        let job = measure_resampler_cpu_ms(&mut prepare_ms, || {
            first.prepare_visible_upscale_job(&device, &source, plan)
        })
        .unwrap();
        let mut encode_ms = 0.0;
        let commands = measure_resampler_cpu_ms(&mut encode_ms, || {
            let mut encoder = device.create_command_encoder(&Default::default());
            first.encode_visible_upscale(&mut encoder, &job);
            encoder.finish()
        });
        let mut submit_ms = 0.0;
        let submission = measure_resampler_cpu_ms(&mut submit_ms, || queue.submit(Some(commands)));
        let mut wait_ms = 0.0;
        measure_resampler_cpu_ms(&mut wait_ms, || {
            device.poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
        })
        .expect("GPU completion for measure_776");
        emit(serde_json::json!({
            "phase": "upscale_job", "device": 1, "iteration": iteration,
            "source": work.source_size(), "target": work.target_size(),
            "scale_branch": key.scale_branch.as_str(),
            "source_region_px": work.visible_source_region(),
            "prepare_job_ms": prepare_ms, "encode_ms": encode_ms,
            "submit_ms": submit_ms, "gpu_completion_wait_ms": wait_ms,
        }));
    }
    let mut total_ms = 0.0;
    let anime = measure_resampler_cpu_ms(&mut total_ms, || {
        Anime4kResampler::new(&device, STILL_IMAGE_ANIME4K_VARIANT)
    });
    emit(serde_json::json!({
        "phase": "anime4k_new", "device": 1, "total_ms": total_ms,
        "steps": anime.creation_timings.perf_value(),
    }));
}
