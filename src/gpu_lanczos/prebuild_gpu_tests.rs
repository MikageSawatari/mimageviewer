//! Device-wide prebuild ownership, real context fallback, and queue overlap.
use super::*;
use crate::scaler_pipeline_store::{PipelineSource, ScalerKind, ScalerPipelineStore};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

const CASES: [([u32; 2], [f32; 4]); 3] = [
    ([126, 94], [0., 0., 63., 47.]),
    ([107, 80], [0., 0., 63., 47.]),
    ([91, 67], [3.25, 2.5, 48.75, 35.5]),
];

fn product_device(adapter: &wgpu::Adapter) -> (wgpu::Device, wgpu::Queue) {
    crate::test_gpu_group::assert_gpu_test_group();
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("scaler prebuild regression"),
        required_features: crate::gpu_still_shaders::request_passthrough(adapter.features()),
        ..Default::default()
    }))
    .expect("DX12 product scaler device")
}

fn context_pipelines(
    cache: &mut GpuLanczosCache,
    device: &wgpu::Device,
    store: &ScalerPipelineStore,
    expected: PipelineSource,
) -> (Arc<NisPipelineSet>, Arc<Anime4kResampler>) {
    let mut timings = LanczosGenerationTimings::default();
    let nis = cache
        .nis_pipeline_set(device, Some(store), &mut timings)
        .clone();
    assert_eq!(timings.pipeline_source, Some(expected));
    let mut timings = LanczosGenerationTimings::default();
    let anime = cache
        .anime_pipeline(device, Some(store), &mut timings)
        .clone();
    assert_eq!(timings.pipeline_source, Some(expected));
    (nis, anime)
}

fn draw_cases(
    cache: &mut GpuLanczosCache,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    input: &wgpu::Texture,
    store: Option<&ScalerPipelineStore>,
    kind: ScalerKind,
    first_source: Option<PipelineSource>,
) -> Vec<Vec<u8>> {
    let mut results = Vec::new();
    for (case, (target, region)) in CASES.into_iter().enumerate() {
        let mut timings = LanczosGenerationTimings::default();
        let mut encoder = device.create_command_encoder(&Default::default());
        let output = match kind {
            ScalerKind::Nis => {
                let pipelines = cache.nis_pipeline_set(device, store, &mut timings).clone();
                assert!(
                    pipelines.shader_uses_dxbc,
                    "shader variant: product DX12 NIS must use DXBC, both prebuilt and inline"
                );
                let plan = NisUpscalePlan {
                    source_size: [63, 47],
                    target_size: target,
                    source_region_px: region,
                    texture_fetches: 0,
                };
                let job = pipelines
                    .prepare_job(device, input, plan, &mut timings)
                    .unwrap();
                assert!(
                    Arc::ptr_eq(&pipelines, &job.pipelines),
                    "prepare and encode must own the same NIS Arc"
                );
                assert!(Arc::ptr_eq(&job.pipelines, cache.nis.as_ref().unwrap()));
                job.encode(&mut encoder);
                job.output_texture
            }
            ScalerKind::Anime4k => {
                let pipelines = cache.anime_pipeline(device, store, &mut timings).clone();
                assert!(
                    pipelines.shader_uses_dxbc,
                    "shader variant: product DX12 Anime4K must use DXBC, both prebuilt and inline"
                );
                let plan = Anime4kPlan::new([63, 47], target, region).unwrap();
                let job = pipelines.prepare_job(device, input, plan).unwrap();
                let encode_pipelines = cache.anime_resampler.as_ref().unwrap();
                assert!(
                    Arc::ptr_eq(&pipelines, encode_pipelines),
                    "prepare and encode must use the same Anime4K slot"
                );
                encode_pipelines.encode(&mut encoder, &job);
                job.into_output_texture()
            }
        };
        assert_eq!(
            timings.pipeline_source,
            if case == 0 { first_source } else { None }
        );
        queue.submit([encoder.finish()]);
        let rgba = read_rgba(device, queue, &output);
        assert!(rgba.chunks_exact(4).any(|pixel| pixel[..3] != [0, 0, 0]));
        results.push(rgba);
    }
    results
}

#[test]
fn prebuilt_contexts_share_arcs_and_match_inline_all_pixels() {
    crate::test_gpu_group::assert_gpu_test_group();
    let (_instance, adapter) = adapter(true);
    let (device, queue) = product_device(&adapter);
    let input = fixture(&device, &queue);
    let store = Arc::new(ScalerPipelineStore::new(&device));
    for kind in [ScalerKind::Nis, ScalerKind::Anime4k] {
        store.request(kind, &device);
        store.wait_ready_for_test(kind);
    }
    let nis = store.get_nis(&device);
    let anime = store.get_anime4k(&device);
    assert_eq!(nis.source, PipelineSource::Prebuilt);
    assert_eq!(anime.source, PipelineSource::Prebuilt);
    let mut main = GpuLanczosCache::default();
    let mut detached = GpuLanczosCache::default();
    let main_arcs = context_pipelines(&mut main, &device, &store, PipelineSource::Prebuilt);
    let detached_arcs = context_pipelines(&mut detached, &device, &store, PipelineSource::Prebuilt);
    assert!(Arc::ptr_eq(&main_arcs.0, &detached_arcs.0));
    assert!(Arc::ptr_eq(&main_arcs.1, &detached_arcs.1));
    assert!(Arc::ptr_eq(nis.value.as_ref().unwrap(), &detached_arcs.0));
    assert!(Arc::ptr_eq(anime.value.as_ref().unwrap(), &detached_arcs.1));
    drop(main_arcs);
    drop(main);
    for kind in [ScalerKind::Nis, ScalerKind::Anime4k] {
        let shared = draw_cases(
            &mut detached,
            &device,
            &queue,
            &input,
            Some(&store),
            kind,
            None,
        );
        let mut inline = GpuLanczosCache::default();
        let direct = draw_cases(
            &mut inline,
            &device,
            &queue,
            &input,
            None,
            kind,
            Some(PipelineSource::Inline),
        );
        assert_eq!(
            shared,
            direct,
            "{} prebuilt/inline pixel parity",
            kind.as_str()
        );
    }
    assert!(Arc::ptr_eq(
        detached.nis.as_ref().unwrap(),
        &detached_arcs.0
    ));
    assert!(Arc::ptr_eq(
        detached.anime_resampler.as_ref().unwrap(),
        &detached_arcs.1
    ));
    assert!(Arc::ptr_eq(
        store.get_nis(&device).value.as_ref().unwrap(),
        &detached_arcs.0
    ));
    assert!(Arc::ptr_eq(
        store.get_anime4k(&device).value.as_ref().unwrap(),
        &detached_arcs.1
    ));
}

#[test]
fn failed_store_uses_one_context_slot_for_prepare_encode_and_reuse() {
    crate::test_gpu_group::assert_gpu_test_group();
    let (_instance, adapter) = adapter(true);
    let (device, queue) = product_device(&adapter);
    let input = fixture(&device, &queue);
    let store = ScalerPipelineStore::new(&device);
    store.fail_for_test(ScalerKind::Nis);
    store.fail_for_test(ScalerKind::Anime4k);
    let mut cache = GpuLanczosCache::default();
    let retained = context_pipelines(&mut cache, &device, &store, PipelineSource::Inline);
    for kind in [ScalerKind::Nis, ScalerKind::Anime4k] {
        let first = draw_cases(
            &mut cache,
            &device,
            &queue,
            &input,
            Some(&store),
            kind,
            None,
        );
        let repeated = draw_cases(
            &mut cache,
            &device,
            &queue,
            &input,
            Some(&store),
            kind,
            None,
        );
        assert_eq!(first, repeated);
    }
    assert!(Arc::ptr_eq(cache.nis.as_ref().unwrap(), &retained.0));
    assert!(Arc::ptr_eq(
        cache.anime_resampler.as_ref().unwrap(),
        &retained.1
    ));
    assert!(store.get_nis(&device).value.is_none());
    assert!(store.get_anime4k(&device).value.is_none());
}

#[test]
fn different_device_falls_back_without_replacing_original_store_arcs() {
    crate::test_gpu_group::assert_gpu_test_group();
    let (_instance, adapter) = adapter(true);
    let (original, _original_queue) = product_device(&adapter);
    let (device, queue) = product_device(&adapter);
    let store = ScalerPipelineStore::new(&original);
    let retained_nis = store.get_nis(&original).value.unwrap();
    let retained_anime = store.get_anime4k(&original).value.unwrap();
    let wrong_nis = store.get_nis(&device);
    let wrong_anime = store.get_anime4k(&device);
    assert!(wrong_nis.value.is_none());
    assert!(wrong_anime.value.is_none());
    assert_eq!(wrong_nis.source, PipelineSource::Inline);
    assert_eq!(wrong_anime.source, PipelineSource::Inline);
    let input = fixture(&device, &queue);
    let mut cache = GpuLanczosCache::default();
    let fallback = context_pipelines(&mut cache, &device, &store, PipelineSource::Inline);
    for kind in [ScalerKind::Nis, ScalerKind::Anime4k] {
        draw_cases(
            &mut cache,
            &device,
            &queue,
            &input,
            Some(&store),
            kind,
            None,
        );
    }
    assert!(Arc::ptr_eq(cache.nis.as_ref().unwrap(), &fallback.0));
    assert!(Arc::ptr_eq(
        cache.anime_resampler.as_ref().unwrap(),
        &fallback.1
    ));
    assert!(!Arc::ptr_eq(&retained_nis, &fallback.0));
    assert!(!Arc::ptr_eq(&retained_anime, &fallback.1));
    assert!(Arc::ptr_eq(
        store.get_nis(&original).value.as_ref().unwrap(),
        &retained_nis
    ));
    assert!(Arc::ptr_eq(
        store.get_anime4k(&original).value.as_ref().unwrap(),
        &retained_anime
    ));
}

#[derive(Clone, Copy)]
struct ApiCall {
    begin: Instant,
    end: Instant,
}

#[derive(Default)]
struct QueueCalls {
    write_buffer: Vec<ApiCall>,
    write_texture: Vec<ApiCall>,
    submit: Vec<ApiCall>,
    poll: Vec<ApiCall>,
}

fn timed_api<T>(calls: &mut Vec<ApiCall>, call: impl FnOnce() -> T) -> T {
    let begin = Instant::now();
    let result = call();
    calls.push(ApiCall {
        begin,
        end: Instant::now(),
    });
    result
}

struct QueueFixture {
    upload: wgpu::Buffer,
    copied: wgpu::Buffer,
    texture: wgpu::Texture,
    copied_texture: wgpu::Texture,
    bytes: [u8; 256],
}

impl QueueFixture {
    fn new(device: &wgpu::Device) -> Self {
        let upload = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlap UI upload"),
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let copied = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlap UI copy"),
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("overlap UI texture upload"),
            size: wgpu::Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let copied_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("overlap UI texture copy"),
            size: texture.size(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        Self {
            upload,
            copied,
            texture,
            copied_texture,
            bytes: [127; 256],
        }
    }

    fn iteration(&self, device: &wgpu::Device, queue: &wgpu::Queue, calls: &mut QueueCalls) {
        timed_api(&mut calls.write_buffer, || {
            queue.write_buffer(&self.upload, 0, &self.bytes)
        });
        timed_api(&mut calls.write_texture, || {
            queue.write_texture(
                self.texture.as_image_copy(),
                &self.bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(32),
                    rows_per_image: Some(8),
                },
                self.texture.size(),
            )
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.upload, 0, &self.copied, 0, 256);
        encoder.copy_texture_to_texture(
            self.texture.as_image_copy(),
            self.copied_texture.as_image_copy(),
            self.texture.size(),
        );
        let command = encoder.finish();
        let submission = timed_api(&mut calls.submit, || queue.submit([command]));
        timed_api(&mut calls.poll, || {
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: None,
                })
                .expect("overlap UI poll")
        });
    }
}

fn overlap_stats(
    calls: &[ApiCall],
    worker_begin: Instant,
    worker_end: Instant,
) -> serde_json::Value {
    let intersecting = calls
        .iter()
        .filter(|call| call.begin < worker_end && call.end > worker_begin)
        .collect::<Vec<_>>();
    let contained = intersecting
        .iter()
        .filter(|call| call.begin >= worker_begin && call.end <= worker_end)
        .count();
    assert!(
        contained > 0,
        "each API needs a call fully inside pipeline construction"
    );
    let mut durations = intersecting
        .iter()
        .map(|call| (call.end - call.begin).as_secs_f64() * 1000.0)
        .collect::<Vec<_>>();
    durations.sort_by(f64::total_cmp);
    let median = if durations.len() % 2 == 0 {
        (durations[durations.len() / 2 - 1] + durations[durations.len() / 2]) / 2.0
    } else {
        durations[durations.len() / 2]
    };
    serde_json::json!({
        "intersecting_calls": durations.len(), "fully_contained_calls": contained,
        "median_ms": median, "max_ms": durations.last().unwrap(),
    })
}

#[test]
#[ignore = "manual DX12 product prebuild/UI queue overlap timing; use dev-runtime"]
fn measure_776b_anime4k_ui_queue_overlap() {
    crate::test_gpu_group::assert_gpu_test_group();
    assert!(!cfg!(debug_assertions), "use --cargo-profile dev-runtime");
    let flags = wgpu::InstanceFlags::from_build_config().with_env();
    assert!(
        !flags.intersects(wgpu::InstanceFlags::DEBUG | wgpu::InstanceFlags::VALIDATION),
        "unset WGPU_DEBUG / WGPU_VALIDATION for product-flags timing"
    );
    let (_instance, adapter) = adapter(false);
    let (device, queue) = product_device(&adapter);
    let fixture = QueueFixture::new(&device);
    let mut warmup = QueueCalls::default();
    for _ in 0..4 {
        fixture.iteration(&device, &queue, &mut warmup);
    }
    drop(warmup);
    let store = Arc::new(ScalerPipelineStore::new(&device));
    let done = Arc::new(AtomicBool::new(false));
    let (start_tx, start_rx) = mpsc::sync_channel(0);
    let origin = Instant::now();
    let worker = {
        let store = store.clone();
        let device = device.clone();
        let done = done.clone();
        std::thread::Builder::new()
            .name("miv-scaler-prebuild-anime4k-measure".into())
            .spawn(move || {
                crate::similar_index::lower_current_thread_priority();
                // Rendezvous is outside the timed construction interval. The worker
                // never waits on a test barrier during the pipeline constructor.
                start_tx.send(()).unwrap();
                let begin = Instant::now();
                // Publish completion even on a test failure, so the UI loop
                // terminates and reports the worker panic instead of spinning.
                let acquired = catch_unwind(AssertUnwindSafe(|| store.get_anime4k(&device)));
                let end = Instant::now();
                done.store(true, Ordering::Release);
                (begin, end, acquired)
            })
            .unwrap()
    };
    let mut calls = QueueCalls::default();
    start_rx.recv().unwrap();
    while !done.load(Ordering::Acquire) {
        fixture.iteration(&device, &queue, &mut calls);
    }
    // Joining is done only after the UI-equivalent loop has finished.
    let (begin, end, acquired) = worker.join().unwrap();
    let acquired = acquired.expect("overlap pipeline worker panicked");
    assert_eq!(acquired.source, PipelineSource::Inline);
    assert!(
        acquired.value.is_some(),
        "product scoped Anime4K creation must succeed"
    );
    let all_calls = [
        &calls.write_buffer,
        &calls.write_texture,
        &calls.submit,
        &calls.poll,
    ];
    let overlapping = all_calls
        .into_iter()
        .flatten()
        .filter(|call| call.begin < end && call.end > begin)
        .collect::<Vec<_>>();
    let overlap_begin = overlapping
        .iter()
        .map(|call| call.begin.max(begin))
        .min()
        .expect("overlapping queue work");
    let overlap_end = overlapping
        .iter()
        .map(|call| call.end.min(end))
        .max()
        .unwrap();
    let report = serde_json::json!({
        "backend": "Dx12", "compiler": "Fxc", "passthrough": device.features().contains(wgpu::Features::PASSTHROUGH_SHADERS),
        "flags": format!("{flags:?}"), "interval": "store.get_anime4k (includes product error scopes; not individual PSO API timing)",
        "worker_create_ms": (end - begin).as_secs_f64() * 1000.0,
        "worker_begin_ms": (begin - origin).as_secs_f64() * 1000.0,
        "worker_end_ms": (end - origin).as_secs_f64() * 1000.0,
        "overlap_window_begin_ms": (overlap_begin - origin).as_secs_f64() * 1000.0,
        "overlap_window_end_ms": (overlap_end - origin).as_secs_f64() * 1000.0,
        "overlap_window_ms": (overlap_end - overlap_begin).as_secs_f64() * 1000.0,
        "write_buffer": overlap_stats(&calls.write_buffer, begin, end),
        "write_texture": overlap_stats(&calls.write_texture, begin, end),
        "submit": overlap_stats(&calls.submit, begin, end),
        "poll": overlap_stats(&calls.poll, begin, end),
    });
    println!("MEASURE776B_OVERLAP {report}");
}
