//! Opt-in, windowless §1.809 B measurements. No native window, surface or event loop.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Capture, Scene, distribution, representatives, write_json};

const WARMUP_ITERATIONS: usize = 30;
const MEASURED_ITERATIONS: usize = 240;
const FORMAT: wgpu::TextureFormat = crate::egui037_stage4_gpu_tests::FORMAT;
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(30);

struct Timestamps {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
}

impl Timestamps {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("hover 809 render-pass timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hover 809 timestamp resolve"),
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hover 809 timestamp readback"),
                size: 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
        }
    }

    fn read_us(&self, device: &wgpu::Device, period_ns: f32) -> f64 {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                sender.send(result).expect("hover 809 timestamp receiver");
            });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(COMPLETION_TIMEOUT),
            })
            .expect("hover 809 timestamp mapping completion");
        receiver
            .recv_timeout(COMPLETION_TIMEOUT)
            .expect("hover 809 timestamp callback")
            .expect("hover 809 timestamp mapping");
        let mapped = self
            .readback
            .slice(..)
            .get_mapped_range()
            .expect("hover 809 mapped timestamps");
        let begin = u64::from_ne_bytes(mapped[0..8].try_into().unwrap());
        let end = u64::from_ne_bytes(mapped[8..16].try_into().unwrap());
        drop(mapped);
        self.readback.unmap();
        assert!(end >= begin, "hover 809 timestamp order: {begin}..{end}");
        (end - begin) as f64 * f64::from(period_ns) / 1_000.0
    }
}

struct OffscreenMeasurement {
    device: wgpu::Device,
    queue: wgpu::Queue,
    timestamps: Option<Timestamps>,
    adapter: Value,
    options: Value,
}

impl OffscreenMeasurement {
    fn new() -> Self {
        // Keep this before any device/driver execution; the gpu_tests module
        // name also assigns the ignored test to nextest's existing gpu group.
        crate::test_gpu_group::assert_gpu_test_group();
        let mut descriptor = wgpu::InstanceDescriptor {
            backends: if cfg!(target_os = "windows") {
                wgpu::Backends::DX12
            } else {
                wgpu::Backends::PRIMARY
            },
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        };
        descriptor.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
        let instance = wgpu::Instance::new(descriptor);
        let request = wgpu::RequestAdapterOptions::default();
        let adapter = pollster::block_on(instance.request_adapter(&request))
            .expect("hover 809 prerequisite: no offscreen GPU adapter (not a pass/skip)");
        let info = adapter.get_info();
        let features = adapter.features();
        let timestamp_supported = features.contains(wgpu::Features::TIMESTAMP_QUERY);
        let required_features = if timestamp_supported {
            wgpu::Features::TIMESTAMP_QUERY
        } else {
            wgpu::Features::empty()
        };
        let device_descriptor = wgpu::DeviceDescriptor {
            label: Some("hover 809 measurement device"),
            required_features,
            ..Default::default()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&device_descriptor))
            .expect("hover 809 prerequisite: create offscreen GPU device");
        let timestamps = timestamp_supported.then(|| Timestamps::new(&device));
        let renderer_options = egui_wgpu::RendererOptions::default();
        let options = json!({
            "requested_backends": if cfg!(target_os = "windows") { "DX12" } else { "PRIMARY" },
            "dx12_shader_compiler": "Fxc",
            "power_preference": format!("{:?}", request.power_preference),
            "force_fallback_adapter": request.force_fallback_adapter,
            "compatible_surface": false,
            "required_features": format!("{required_features:?}"),
            "required_limits": format!("{:?}", device_descriptor.required_limits),
            "format": format!("{FORMAT:?}"),
            "renderer": {
                "msaa_samples": renderer_options.msaa_samples,
                "depth_stencil_format": format!("{:?}", renderer_options.depth_stencil_format),
                "dithering": renderer_options.dithering,
                "predictable_texture_filtering": renderer_options.predictable_texture_filtering,
            },
            "timestamp_query_supported": timestamp_supported,
            "timestamp_period_ns": timestamp_supported.then(|| queue.get_timestamp_period()),
        });
        Self {
            device,
            queue,
            timestamps,
            adapter: json!({
                "name": info.name,
                "vendor": info.vendor,
                "device": info.device,
                "device_type": format!("{:?}", info.device_type),
                "backend": format!("{:?}", info.backend),
                "driver": info.driver,
                "driver_info": info.driver_info,
                "available_features": format!("{features:?}"),
            }),
            options,
        }
    }

    fn target(&self, size: [u32; 2]) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hover 809 persistent offscreen target"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    }

    fn frame(
        &self,
        renderer: &mut egui_wgpu::Renderer,
        jobs: &[egui::ClippedPrimitive],
        screen: &egui_wgpu::ScreenDescriptor,
        target: &wgpu::TextureView,
        measured: bool,
    ) -> (f64, Option<f64>) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("hover 809 measured frame"),
            });
        // Use the real prepare/finish_prepare path and submit callback buffers
        // in their normal order. Preparation/encoding is outside the GPU timer.
        let mut submissions =
            renderer.update_buffers(&self.device, &self.queue, &mut encoder, jobs, screen);
        let timestamps = self.timestamps.as_ref().filter(|_| measured);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("hover 809 clear and egui pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    timestamp_writes: timestamps.map(|timestamps| {
                        wgpu::RenderPassTimestampWrites {
                            query_set: &timestamps.queries,
                            beginning_of_pass_write_index: Some(0),
                            end_of_pass_write_index: Some(1),
                        }
                    }),
                    ..Default::default()
                })
                .forget_lifetime();
            renderer.render(&mut pass, jobs, screen);
        }
        if let Some(timestamps) = timestamps {
            // Only the two timestamps are read back. Resolving/copying them is
            // outside the measured pass, and no target pixels are copied.
            encoder.resolve_query_set(&timestamps.queries, 0..2, &timestamps.resolve, 0);
            encoder.copy_buffer_to_buffer(&timestamps.resolve, 0, &timestamps.readback, 0, 16);
        }
        submissions.push(encoder.finish());
        let submitted = Instant::now();
        let submission = self.queue.submit(submissions);
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(COMPLETION_TIMEOUT),
            })
            .expect("hover 809 frame completion");
        let cpu_submit_to_completion_us = submitted.elapsed().as_secs_f64() * 1_000_000.0;
        // Mapping and the mapping-completion poll are outside both timers.
        let gpu_render_pass_us = timestamps
            .map(|timestamps| timestamps.read_us(&self.device, self.queue.get_timestamp_period()));
        (cpu_submit_to_completion_us, gpu_render_pass_us)
    }

    fn measure(
        &self,
        scene: &str,
        representative: &str,
        capture: &Capture,
        screen: &egui_wgpu::ScreenDescriptor,
        target: &wgpu::TextureView,
    ) -> Value {
        let mut renderer =
            egui_wgpu::Renderer::new(&self.device, FORMAT, egui_wgpu::RendererOptions::default());
        // Capture includes every full/partial atlas delta, including the CPU
        // fixture warmup. Preserve order, including repeated TextureIds.
        for (id, delta) in &capture.texture_sets {
            renderer.update_texture(&self.device, &self.queue, *id, delta);
        }
        let mut vertices = 0;
        let mut indices = 0;
        let mut meshes = 0;
        let mut callbacks = 0;
        for job in &capture.jobs {
            match &job.primitive {
                egui::epaint::Primitive::Mesh(mesh) => {
                    assert!(
                        renderer.texture(&mesh.texture_id).is_some(),
                        "hover 809 {scene}/{representative}: missing texture {:?}",
                        mesh.texture_id
                    );
                    vertices += mesh.vertices.len();
                    indices += mesh.indices.len();
                    meshes += 1;
                }
                egui::epaint::Primitive::Callback(_) => callbacks += 1,
            }
        }
        for _ in 0..WARMUP_ITERATIONS {
            self.frame(&mut renderer, &capture.jobs, screen, target, false);
        }
        let mut cpu_times = Vec::with_capacity(MEASURED_ITERATIONS);
        let mut gpu_times = Vec::with_capacity(MEASURED_ITERATIONS);
        for _ in 0..MEASURED_ITERATIONS {
            let (cpu_us, gpu_us) = self.frame(&mut renderer, &capture.jobs, screen, target, true);
            cpu_times.push(cpu_us);
            if let Some(gpu_us) = gpu_us {
                gpu_times.push(gpu_us);
            }
        }
        json!({
            "scene": scene,
            "representative": representative,
            "size_in_pixels": screen.size_in_pixels,
            "pixels_per_point": screen.pixels_per_point,
            "warmup_iterations": WARMUP_ITERATIONS,
            "measured_iterations": MEASURED_ITERATIONS,
            "primitive_count": capture.jobs.len(),
            "mesh_count": meshes,
            "callback_count": callbacks,
            "vertices": vertices,
            "indices": indices,
            "texture_delta_count": capture.texture_sets.len(),
            "source_cpu_sample": capture.source,
            "gpu_render_pass_us": if gpu_times.is_empty() { Value::Null } else { distribution(gpu_times) },
            "cpu_submit_to_completion_us": distribution(cpu_times),
        })
    }
}

#[test]
#[ignore = "opt-in §1.809 B offscreen GPU measurement; no window, surface, or present"]
fn hover_809_offscreen_gpu_measurement() {
    let gpu = OffscreenMeasurement::new();
    let mut rows = Vec::new();
    let mut omitted_representatives = Vec::new();
    for size in [[2560, 1440], [3840, 2160]] {
        // Reuse target and timestamp/query buffers for every measured frame.
        let target = gpu.target(size);
        let view = target.create_view(&Default::default());
        for ppp in [1.0, 1.5] {
            let screen = egui_wgpu::ScreenDescriptor {
                size_in_pixels: size,
                pixels_per_point: ppp,
            };
            let clear = Capture {
                jobs: Vec::new(),
                texture_sets: Vec::new(),
                source: Value::Null,
            };
            rows.push(gpu.measure("clear", "clear", &clear, &screen, &view));
            for scene in Scene::ALL.into_iter().chain(Scene::COMPONENTS) {
                let captures = representatives(scene, size, ppp);
                assert!(
                    !captures.is_empty(),
                    "hover 809 {} has no representatives",
                    scene.name()
                );
                for expected in ["same", "changed"] {
                    if !captures.iter().any(|(name, _)| *name == expected) {
                        omitted_representatives.push(json!({
                            "scene": scene.name(),
                            "representative": expected,
                            "size_in_pixels": size,
                            "pixels_per_point": ppp,
                            "reason": if expected == "same" {
                                "no same frame in moving or stationary capture intervals; no synthetic frame substituted"
                            } else {
                                "no changed frame in the moving capture interval; no synthetic frame substituted"
                            },
                        }));
                    }
                }
                for (representative, capture) in captures {
                    rows.push(gpu.measure(scene.name(), representative, &capture, &screen, &view));
                }
            }
        }
    }
    let report = json!({
        "section": "1.809",
        "measurement": "B",
        "units": "microseconds",
        "adapter": gpu.adapter,
        "configuration": gpu.options,
        "method": {
            "primary": if gpu.timestamps.is_some() { "gpu_render_pass_timestamp" } else { "cpu_submit_to_completion_upper_bound" },
            "gpu_render_pass_includes": "attachment clear, egui drawing, attachment store",
            "gpu_render_pass_excludes": "CPU frame construction/tessellation, update_buffers preparation, texture upload, query resolve/copy/map, surface acquisition, present, DWM, VSync, pacing",
            "cpu_submit_to_completion": "serialized submit through device.poll completion; includes queue/driver/synchronization overhead and queued buffer work; upper bound, not GPU-only time",
            "texture_uploads": "all captured initialization/warmup deltas applied in order before GPU warmup; no recurring image/atlas updates",
            "samples": "one completed submission per frame; 30 separate warmup frames then 240 measured frames per representative",
            "target_readback": false,
            "timestamp_readback_bytes_per_measured_frame": if gpu.timestamps.is_some() { 16 } else { 0 },
            "surface_created": false,
            "present_measured": false,
            "product_rendering_changed": false,
        },
        "rows": rows,
        "omitted_representatives": omitted_representatives,
    });
    write_json("MIV_GPU_809_JSON", &report);
}
