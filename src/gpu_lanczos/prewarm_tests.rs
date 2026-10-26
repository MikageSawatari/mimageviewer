use super::*;

fn resource(ctx: &egui::Context, size: [usize; 2]) -> FullscreenPaintResource {
    FullscreenPaintResource::resampleable(
        3,
        ctx.load_texture(
            "prewarm-source",
            egui::ColorImage::filled(size, egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        ),
        FullscreenPaintSourceGeneration {
            items: 7,
            input: 11,
        },
    )
}

fn query(
    cache: &GpuLanczosCache,
    rs: Option<&egui_wgpu::RenderState>,
    resource: &FullscreenPaintResource,
    scale: f32,
    filter: PostFilter,
) -> NativePrewarmQuery {
    cache.query_prewarm(
        rs,
        resource,
        scale,
        1.0,
        None,
        filter,
        0,
        AnimeUpscaleSourceLimit::default(),
    )
}

fn plan(resource: &FullscreenPaintResource, scale: f32) -> NativePrewarmPlan {
    let key = resource_lanczos_key(
        resource,
        scale,
        1.0,
        None,
        PostFilter::None,
        0,
        AnimeUpscaleSourceLimit::default(),
    )
    .unwrap();
    NativePrewarmPlan {
        key,
        work: work_plan_for_key(resource.size().map(|size| size as u32), key).unwrap(),
        smoothing_percent: 0,
    }
}

#[test]
fn prewarm_query_preserves_direct_nearest_and_limit_decisions_without_mutating_cache() {
    let ctx = egui::Context::default();
    let source = resource(&ctx, [8, 12]);
    let cache = GpuLanczosCache::default();
    assert!(matches!(
        query(&cache, None, &source, 1.0, PostFilter::None),
        NativePrewarmQuery::NotNeeded
    ));
    assert!(matches!(
        query(&cache, None, &source, 2.0, PostFilter::Nearest),
        NativePrewarmQuery::NotNeeded
    ));
    assert!(matches!(
        query(&cache, None, &source, 0.5, PostFilter::Nearest),
        NativePrewarmQuery::Unavailable(NativePrewarmUnavailable::NoRenderer)
    ));
    let direct = FullscreenPaintResource::direct(source.source_texture().clone());
    assert!(matches!(
        query(&cache, None, &direct, 0.5, PostFilter::None),
        NativePrewarmQuery::NotNeeded
    ));
    assert!(matches!(
        cache.query_prewarm(
            None,
            &source,
            2.0,
            1.0,
            Some(VisibleRegionRequest::full([
                MAX_UPSCALE_TARGET_DIMENSION + 1,
                8
            ])),
            PostFilter::None,
            0,
            AnimeUpscaleSourceLimit::default(),
        ),
        NativePrewarmQuery::NotNeeded
    ));
    assert_eq!(cache.use_clock, 0);
    assert_eq!(cache.regeneration_count, 0);
    assert!(cache.entries.is_empty());
    assert!(cache.limit_fallback_sources.is_empty());
    assert!(cache.resampler.is_none());
    assert!(cache.anime_resampler.is_none());
}

#[test]
fn prewarm_allocation_uses_actual_temporary_texture_dimensions_and_variant_count() {
    let full = LanczosWorkPlan::Full(LanczosPlan::new([100, 200], [50, 100], 0).unwrap());
    assert_eq!(
        full.allocation_bytes(),
        NativeAllocationBytes {
            output_bytes: 50 * 100 * 4,
            temporary_bytes: 100 * 100 * 8,
        }
    );
    let roi = egui::Rect::from_min_max(egui::pos2(0.25, 0.25), egui::pos2(0.5, 0.5));
    let visible = VisibleUpscalePlan::new([400, 600], [200, 300], roi).unwrap();
    let allocation = LanczosWorkPlan::VisibleUpscale(visible).allocation_bytes();
    assert_eq!(allocation.output_bytes, 200 * 300 * 4);
    assert_eq!(
        allocation.temporary_bytes,
        u64::from(visible.intermediate_x_width) * 300 * 8
    );
    assert!(allocation.temporary_bytes < 400 * 300 * 8);
    for work in [
        LanczosWorkPlan::NisUpscale(NisUpscalePlan::new([400, 600], [200, 300], roi).unwrap()),
        LanczosWorkPlan::PixelArtUpscale(
            PixelArtUpscalePlan::new([400, 600], [200, 300], roi).unwrap(),
        ),
    ] {
        assert_eq!(
            work.allocation_bytes(),
            NativeAllocationBytes {
                output_bytes: 200 * 300 * 4,
                temporary_bytes: 0
            }
        );
    }
    let anime = Anime4kPlan::new([400, 600], [200, 300], [100.0, 150.0, 100.0, 150.0]).unwrap();
    let allocation = LanczosWorkPlan::AnimeUpscale(anime).allocation_bytes();
    assert_eq!(
        allocation.temporary_bytes,
        u64::from(anime.process_size[0])
            * u64::from(anime.process_size[1])
            * 8
            * STILL_IMAGE_ANIME4K_VARIANT.intermediate_count() as u64
    );
    assert_eq!(
        allocation.total(),
        allocation.output_bytes + allocation.temporary_bytes
    );
}

#[test]
fn prewarm_backing_requires_exact_size_and_rgba8_format() {
    assert!(backing_matches_plan(
        [8, 12],
        wgpu::TextureFormat::Rgba8Unorm,
        [8, 12]
    ));
    assert!(!backing_matches_plan(
        [8, 13],
        wgpu::TextureFormat::Rgba8Unorm,
        [8, 12]
    ));
    assert!(!backing_matches_plan(
        [8, 12],
        wgpu::TextureFormat::Rgba8UnormSrgb,
        [8, 12]
    ));
    assert!(!backing_matches_plan(
        [8, 12],
        wgpu::TextureFormat::Rgba16Float,
        [8, 12]
    ));
}

#[test]
fn prewarm_exact_hit_does_not_touch_recency_or_require_source_backing() {
    let ctx = egui::Context::default();
    let original = resource(&ctx, [8, 12]);
    let native = test_native_resource(original.clone(), [4, 6]);
    let output = native.lanczos_output().unwrap().clone();
    let mut cache = GpuLanczosCache::default();
    cache.entries.insert(
        output.key,
        LanczosCacheEntry {
            output,
            last_used: 123,
        },
    );
    cache.use_clock = 456;
    let NativePrewarmQuery::Hit(key) = query(&cache, None, &original, 0.5, PostFilter::None) else {
        panic!("exact cache hit does not need the source backing");
    };
    assert_eq!(cache.use_clock, 456);
    assert_eq!(cache.entries[&key].last_used, 123);
    assert!(cache.resampler.is_none());
    assert!(matches!(
        query(&cache, None, &original, 0.25, PostFilter::None),
        NativePrewarmQuery::Unavailable(NativePrewarmUnavailable::NoRenderer)
    ));
    assert!(matches!(
        cache.query_prewarm(
            None,
            &original,
            0.5,
            1.0,
            None,
            PostFilter::None,
            10,
            AnimeUpscaleSourceLimit::default()
        ),
        NativePrewarmQuery::Unavailable(NativePrewarmUnavailable::NoRenderer)
    ));
    cache.active_smoothing_percent = 10;
    // A held native with the exact requested percent remains reusable even if
    // the cache's most recently used setting differs, as in ordinary resolve.
    assert!(matches!(
        query(&cache, None, &native, 0.5, PostFilter::None),
        NativePrewarmQuery::Hit(_)
    ));
}

fn headless_render_state() -> egui_wgpu::RenderState {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
    let instance = wgpu::Instance::new(descriptor);
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .expect("Native prewarm regression requires a headless adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("Native prewarm regression"),
        ..Default::default()
    }))
    .expect("Native prewarm headless device");
    let target_format = wgpu::TextureFormat::Rgba8Unorm;
    let renderer = egui_wgpu::Renderer::new(&device, target_format, Default::default());
    egui_wgpu::RenderState {
        instance,
        adapter,
        available_adapters: Vec::new(),
        device,
        queue,
        target_format,
        renderer: Arc::new(egui::epaint::mutex::RwLock::new(renderer)),
    }
}

fn deliver(rs: &egui_wgpu::RenderState, source: &FullscreenPaintResource, size: [usize; 2]) {
    let delta = egui::epaint::ImageDelta::full(
        egui::ColorImage::filled(size, egui::Color32::WHITE),
        egui::TextureOptions::LINEAR,
    );
    rs.renderer
        .write()
        .update_texture(&rs.device, &rs.queue, source.source_texture_id(), &delta);
    // Queue::write_texture is ordered before the later Native submit. This does
    // not claim GPU completion or require a diagnostic completion ticket.
}

#[test]
fn prewarm_delivery_claim_cache_hit_and_output_lease_use_normal_ownership() {
    let rs = headless_render_state();
    let ctx = egui::Context::default();
    let source = resource(&ctx, [8, 12]);
    let mut cache = GpuLanczosCache::default();
    let expected = plan(&source, 0.5);
    assert!(matches!(
        query(&cache, Some(&rs), &source, 0.5, PostFilter::None),
        NativePrewarmQuery::Unavailable(NativePrewarmUnavailable::MissingBacking)
    ));
    assert!(matches!(
        cache.execute_prewarm(&rs, &source, expected, |_| panic!(
            "missing backing must not claim"
        )),
        NativePrewarmExecution::Unavailable(NativePrewarmUnavailable::MissingBacking)
    ));
    deliver(&rs, &source, [8, 13]);
    assert!(matches!(
        query(&cache, Some(&rs), &source, 0.5, PostFilter::None),
        NativePrewarmQuery::Unavailable(NativePrewarmUnavailable::BackingMismatch)
    ));
    assert!(matches!(
        cache.execute_prewarm(&rs, &source, expected, |_| panic!(
            "wrong backing must not claim"
        )),
        NativePrewarmExecution::Unavailable(NativePrewarmUnavailable::BackingMismatch)
    ));
    deliver(&rs, &source, [8, 12]);
    let NativePrewarmQuery::Miss(planned) =
        query(&cache, Some(&rs), &source, 0.5, PostFilter::None)
    else {
        panic!("delivered source must be a native miss")
    };
    assert_eq!(planned.key(), expected.key());
    assert_eq!(cache.use_clock, 0);
    assert!(cache.resampler.is_none());
    assert!(matches!(
        cache.execute_prewarm(&rs, &source, planned, |_| false),
        NativePrewarmExecution::Unavailable(NativePrewarmUnavailable::ClaimDenied)
    ));
    assert_eq!(cache.regeneration_count, 0);
    assert!(cache.resampler.is_none());
    let mut claims = 0;
    let result = cache.execute_prewarm(&rs, &source, planned, |allocation| {
        // Taking the renderer write lock here also checks the no-lock callback
        // contract. The closure does not visit App contexts or reenter UI.
        drop(rs.renderer.write());
        assert_eq!(allocation, planned.allocation());
        claims += 1;
        true
    });
    let NativePrewarmExecution::Submitted {
        output_texture_id,
        stats,
    } = result
    else {
        panic!("prewarm must submit")
    };
    assert_eq!(claims, 1);
    assert_eq!(stats.target_size, [4, 6]);
    assert_eq!(stats.regeneration_count, 1);
    let NativePrewarmQuery::Hit(hit_key) = query(&cache, Some(&rs), &source, 0.5, PostFilter::None)
    else {
        panic!("normal cache must contain prewarm")
    };
    assert_eq!(hit_key, planned.key());
    assert!(matches!(
        cache.execute_prewarm(&rs, &source, planned, |_| panic!("hit must not claim")),
        NativePrewarmExecution::Hit
    ));
    let (prepared, event) = cache.resolve(
        Some(&rs),
        &source,
        0.5,
        1.0,
        None,
        PostFilter::None,
        0,
        AnimeUpscaleSourceLimit::default(),
    );
    assert!(event.is_none());
    assert_eq!(prepared.paint_texture_id(), output_texture_id);
    assert_eq!(cache.regeneration_count, 1);
    let held = prepared.capture_presentation(&ctx);
    cache.clear();
    drop(prepared);
    assert!(
        rs.renderer.read().texture(&output_texture_id).is_some(),
        "presentation retains native lease after cache eviction"
    );
    drop(held);
    assert!(
        rs.renderer.read().texture(&output_texture_id).is_none(),
        "last native lease unregisters output"
    );
}

#[test]
fn prewarm_execution_rejects_changed_source_and_freed_backing_before_claim() {
    let rs = headless_render_state();
    let ctx = egui::Context::default();
    let source = resource(&ctx, [8, 12]);
    let other = resource(&ctx, [8, 12]);
    let mut cache = GpuLanczosCache::default();
    deliver(&rs, &source, [8, 12]);
    let NativePrewarmQuery::Miss(planned) =
        query(&cache, Some(&rs), &source, 0.5, PostFilter::None)
    else {
        panic!("delivered source")
    };
    assert!(matches!(
        cache.execute_prewarm(&rs, &other, planned, |_| panic!(
            "changed source must not claim"
        )),
        NativePrewarmExecution::Unavailable(NativePrewarmUnavailable::SourceChanged)
    ));
    rs.renderer
        .write()
        .free_texture(&source.source_texture_id());
    assert!(matches!(
        cache.execute_prewarm(&rs, &source, planned, |_| panic!(
            "freed backing must not claim"
        )),
        NativePrewarmExecution::Unavailable(NativePrewarmUnavailable::MissingBacking)
    ));
    assert_eq!(cache.regeneration_count, 0);
    assert!(cache.entries.is_empty());
}
