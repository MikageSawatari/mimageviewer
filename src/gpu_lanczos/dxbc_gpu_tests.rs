//! Permanent DX12 contract tests: wgpu updates must catch register allocation,
//! special constants, shader interfaces and DXBC-in-dxil passthrough changes.
//! No adapter/device skip: a Windows GPU gate without DX12 is a failed gate.
use super::*;

#[path = "prebuild_gpu_tests.rs"]
mod prebuild;

#[test]
fn still_nis_dxbc_matches_wgsl_pixels() {
    compare(false);
}

#[test]
fn still_anime4k_vl_dxbc_matches_wgsl_pixels() {
    compare(true);
}

fn adapter(validation: bool) -> (wgpu::Instance, wgpu::Adapter) {
    crate::test_gpu_group::assert_gpu_test_group();
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::DX12;
    desc.flags = wgpu::InstanceFlags::from_build_config().with_env();
    if validation {
        desc.flags.insert(wgpu::InstanceFlags::VALIDATION);
    }
    let flags = desc.flags;
    desc.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
    let instance = wgpu::Instance::new(desc);
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .expect("DX12 adapter required for static-image DXBC regression");
    assert!(
        adapter
            .features()
            .contains(wgpu::Features::PASSTHROUGH_SHADERS),
        "DX12 PASSTHROUGH_SHADERS required; do not silently skip this gate"
    );
    println!("DXBC adapter={:?} flags={flags:?}", adapter.get_info(),);
    (instance, adapter)
}

fn compare(anime: bool) {
    let (_instance, adapter) = adapter(true);
    let mut outputs = Vec::new();
    // Exercise the real product choice, not test-only pipeline injection:
    // feature absent -> WGSL, feature requested -> embedded DXBC.
    for features in [wgpu::Features::empty(), wgpu::Features::PASSTHROUGH_SHADERS] {
        crate::test_gpu_group::assert_gpu_test_group();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("static-image parity"),
            required_features: features,
            ..Default::default()
        }))
        .expect("DX12 device required for static-image parity");
        let input = fixture(&device, &queue);
        let nis = (!anime).then(|| Arc::new(NisPipelineSet::new(&device)));
        let vl = anime.then(|| Anime4kResampler::new(&device, STILL_IMAGE_ANIME4K_VARIANT));
        // Pixel parity alone would also pass if both sides accidentally used
        // WGSL. Inspect the modules that built these exact rendering pipelines.
        let actual_dxbc = if let Some(vl) = &vl {
            vl.shader_uses_dxbc
        } else {
            nis.as_ref().unwrap().shader_uses_dxbc
        };
        assert_eq!(
            actual_dxbc,
            features.contains(wgpu::Features::PASSTHROUGH_SHADERS),
            "shader variant: DX12 with passthrough must use DXBC, without it WGSL (anime={anime})"
        );
        let mut results = Vec::new();
        // Integer/noninteger scales, clamped edges, fractional ROI phase.
        for (target, region) in [
            ([126, 94], [0., 0., 63., 47.]),
            ([107, 80], [0., 0., 63., 47.]),
            ([91, 67], [3.25, 2.5, 48.75, 35.5]),
        ] {
            let mut encoder = device.create_command_encoder(&Default::default());
            let output = if let Some(vl) = &vl {
                let plan = Anime4kPlan::new([63, 47], target, region).unwrap();
                let job = vl.prepare_job(&device, &input, plan).unwrap();
                vl.encode(&mut encoder, &job);
                job.into_output_texture()
            } else {
                let plan = NisUpscalePlan {
                    source_size: [63, 47],
                    target_size: target,
                    source_region_px: region,
                    texture_fetches: 0,
                };
                let job = nis
                    .as_ref()
                    .unwrap()
                    .prepare_job(&device, &input, plan, &mut Default::default())
                    .unwrap();
                job.encode(&mut encoder);
                job.output_texture
            };
            queue.submit([encoder.finish()]);
            let rgba = read_rgba(&device, &queue, &output);
            assert!(
                rgba.chunks_exact(4).any(|p| p[..3] != [0, 0, 0]),
                "nontrivial RGB output"
            );
            for alpha in [0, 128, 255] {
                assert!(
                    rgba.chunks_exact(4).any(|p| p[3] == alpha),
                    "alpha={alpha} coverage"
                );
            }
            results.push(rgba);
        }
        outputs.push(results);
    }
    for (case, (wgsl, dxbc)) in outputs[0].iter().zip(&outputs[1]).enumerate() {
        let different = wgsl
            .chunks_exact(4)
            .zip(dxbc.chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        let max = wgsl
            .iter()
            .zip(dxbc)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        println!(
            "DXBC parity anime={anime} case={case} pixels={} different={different} max={max}",
            wgsl.len() / 4
        );
        assert_eq!(wgsl.len(), dxbc.len());
        assert_eq!(
            different, 0,
            "anime={anime} case={case} maximum channel difference={max}"
        );
    }
}

#[test]
#[ignore = "manual product DXBC first-use CPU timing; never launches the product"]
fn measure_776_nis_product_dxbc_first() {
    measure(false);
}

#[test]
#[ignore = "manual product DXBC first-use CPU timing; never launches the product"]
fn measure_776_anime4k_product_dxbc_first() {
    measure(true);
}

fn measure(anime: bool) {
    assert!(!cfg!(debug_assertions), "use --cargo-profile dev-runtime");
    let flags = wgpu::InstanceFlags::from_build_config().with_env();
    assert!(
        !flags.intersects(wgpu::InstanceFlags::DEBUG | wgpu::InstanceFlags::VALIDATION),
        "measure product flags: unset WGPU_DEBUG / WGPU_VALIDATION"
    );
    let (_instance, adapter) = adapter(false);
    crate::test_gpu_group::assert_gpu_test_group();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("measure_776 product DXBC"),
        required_features: crate::gpu_still_shaders::request_passthrough(adapter.features()),
        ..Default::default()
    }))
    .expect("product DXBC measurement device");
    let input = fixture(&device, &queue);
    let started = Instant::now();
    let (retained, details) = if anime {
        let resampler = Anime4kResampler::new(&device, STILL_IMAGE_ANIME4K_VARIANT);
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let details = serde_json::json!({"family":"anime4k", "first_create_ms":ms,
            "steps":resampler.creation_timings.perf_value()});
        (Some(resampler), details)
    } else {
        let resampler = NisPipelineSet::new(&device);
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let details = serde_json::json!({"family":"nis", "first_create_ms":ms,
            "steps":resampler.creation_timings});
        (None, details)
    };
    println!("MEASURE776_PRODUCT_FLAGS {flags:?}");
    println!("MEASURE776_PRODUCT {details}");
    // Keep the resources live through the timing report, never a second creation.
    drop((retained, input));
}

fn fixture(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::Texture {
    let mut rgba = Vec::with_capacity(63 * 47 * 4);
    for y in 0..47 {
        for x in 0..63 {
            let alpha = if x + y < 28 {
                0
            } else if x > y {
                128
            } else {
                255
            };
            let detail = if (x * 3 + y * 5) % 11 < 3 {
                255
            } else {
                (x * 17 + y * 31) % 256
            };
            rgba.extend(
                [
                    detail * alpha / 255,
                    (255 - detail) * alpha / 255,
                    ((x ^ y) * 7 % 256) * alpha / 255,
                    alpha,
                ]
                .map(|v| v as u8),
            );
        }
    }
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("diagonal/detail/alpha fixture"),
            size: wgpu::Extent3d {
                width: 63,
                height: 47,
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
        &rgba,
    )
}

fn read_rgba(device: &wgpu::Device, queue: &wgpu::Queue, source: &wgpu::Texture) -> Vec<u8> {
    // Product targets intentionally have no COPY_SRC. A lossless textureLoad
    // blit tests their real preparation/encode without changing resource usage.
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("parity readback"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(
            r#"
        @group(0) @binding(0) var source: texture_2d<f32>;
        @vertex fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
            let p = array<vec2<f32>, 3>(vec2(-1., -1.), vec2(3., -1.), vec2(-1., 3.));
            return vec4(p[i], 0., 1.);
        }
        @fragment fn fs_copy(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
            return textureLoad(source, vec2<i32>(p.xy), 0);
        }"#,
        )),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = create_pipeline(
        device,
        &pipeline_layout,
        &shader,
        "fs_copy",
        wgpu::TextureFormat::Rgba8Unorm,
    );
    let view = source.create_view(&Default::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&view),
        }],
    });
    let copy = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: source.size(),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let row = (source.width() * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row) * u64::from(source.height()),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let view = copy.create_view(&Default::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
    encoder.copy_texture_to_buffer(
        copy.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        source.size(),
    );
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .unwrap();
    rx.recv().unwrap().unwrap();
    let mapped = buffer.slice(..).get_mapped_range().unwrap();
    mapped
        .chunks_exact(row as usize)
        .flat_map(|row| row[..source.width() as usize * 4].iter().copied())
        .collect()
}
