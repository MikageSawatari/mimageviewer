use super::*;

fn write_embedded_png(path: &Path) {
    use image::ImageEncoder;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            &vec![90; 512 * 256 * 3],
            512,
            256,
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
    let mut picture = b"\x00image/png\x00\x03\x00".to_vec();
    picture.extend(png);
    let mut frame = b"APIC".to_vec();
    frame.extend((picture.len() as u32).to_be_bytes());
    frame.extend([0, 0]);
    frame.extend(picture);
    let size = frame.len() as u32;
    let mut tag = b"ID3\x03\x00\x00".to_vec();
    tag.extend([
        (size >> 21) as u8 & 127,
        (size >> 14) as u8 & 127,
        (size >> 7) as u8 & 127,
        size as u8 & 127,
    ]);
    tag.extend(frame);
    std::fs::write(path, tag).unwrap();
}

fn prepare_idle(app: &mut App, display_px: u32) {
    app.keep_set.insert(0);
    app.keep_range = (0, 1);
    app.keep_start_shared.store(0, Ordering::Relaxed);
    app.keep_end_shared.store(1, Ordering::Relaxed);
    app.visible_indices = vec![0];
    app.reload_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
    app.heavy_io_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
    app.settings.thumb_idle_upgrade = true;
    app.last_scroll_change_time = std::time::Instant::now() - std::time::Duration::from_secs(2);
    app.last_input_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(2));
    app.display_px_shared.store(display_px, Ordering::Relaxed);
    app.items_generation = 91_347;
    app.input_seq = 1347;
}

fn take_audio_request(app: &App) -> LoadRequest {
    assert!(
        app.reload_queue
            .as_ref()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .is_empty()
    );
    let mut queue = app.heavy_io_queue.as_ref().unwrap().0.lock().unwrap();
    assert_eq!(queue.len(), 1);
    let request = queue.pop().unwrap();
    assert!(
        matches!(
            request.raw_source,
            crate::thumb_loader::LoadRequestSource::AudioThumbnail(_)
        ),
        "Audio quality upgrades must use the shared album-art dispatch, never the generic image decoder"
    );
    assert_eq!(request.items_gen, app.items_generation);
    request
}

fn finish_audio_request(
    app: &mut App,
    ctx: &egui::Context,
    mut request: LoadRequest,
    cache_dir: &Path,
    display_px: u32,
    decision: CacheDecision,
) {
    let crate::thumb_loader::LoadRequestSource::AudioThumbnail(audio) = &mut request.raw_source
    else {
        panic!("Audio dispatch was lost before the worker");
    };
    // Use a disposable catalog, retaining the real handler's media/sidecar/policy contract.
    audio.cache_dir = cache_dir.to_owned();
    audio.admission = crate::catalog::CatalogAccess::for_cache_dir(cache_dir).admit();
    let cache_map = std::sync::RwLock::new(std::collections::HashMap::new());
    let raw_handoff = crate::thumb_loader::RawThumbHandoff::DedicatedWorker(Arc::new(
        crate::raw::RawDevelopExecutor::new(1).unwrap(),
    ));
    crate::thumb_loader::process_load_request(
        &mut request,
        &cache_map,
        &app.tx,
        None,
        64,
        75,
        display_px,
        decision,
        &app.cache_gen_done,
        &Arc::new(Mutex::new(crate::stats::ThumbStats::default())),
        Some(&app.cancel_token),
        &app.keep_start_shared,
        &app.keep_end_shared,
        None,
        None,
        None,
        None,
        Some(&raw_handoff),
    );
    app.poll_thumbnails(ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(app.requested.is_empty());
    assert!(matches!(
        app.thumbnails[0],
        ThumbnailState::Loaded {
            source_dims: Some((512, 256)),
            ..
        }
    ));
}

#[test]
fn audio_idle_upgrade_cache_revisit_keeps_embedded_art_and_source_policy() {
    let mut app = phase_c_support::setup_app();
    let path = app.tmp.path().join("song.mp3");
    let cache_dir = app.tmp.path().join("art-cache");
    write_embedded_png(&path);
    app.settings.cache_policy = crate::settings::CachePolicy::Always;
    let decision = CacheDecision::from_settings(&app.settings);
    let options = crate::audio_thumbnail::AudioThumbnailOptions {
        thumb_px: 64,
        thumb_quality: 75.0,
        cache_dir: cache_dir.clone(),
        cache_decision: decision,
        raw_executor: None,
    };
    crate::audio_thumbnail::generate_with_options(
        &path,
        None,
        &options,
        LoadSourcePolicy::CacheOrSource,
        crate::catalog::CatalogAccess::for_cache_dir(&cache_dir).admit(),
        64,
        &|| false,
    )
    .unwrap()
    .unwrap();
    app.items = vec![GridItem::Audio(path)];
    // History can revisit Audio without an acquired display stamp.
    app.image_metas = vec![None];
    app.thumbnails = vec![ThumbnailState::Evicted];
    prepare_idle(&mut app, 64);
    let ctx = egui::Context::default();
    app.enqueue_priority_thumbnail(0);
    let cached_request = take_audio_request(&app);
    assert_eq!(
        cached_request.source_policy,
        LoadSourcePolicy::CacheOrSource
    );
    finish_audio_request(&mut app, &ctx, cached_request, &cache_dir, 64, decision);
    assert!(matches!(
        app.thumbnails[0],
        ThumbnailState::Loaded {
            origin: crate::thumb_loader::ThumbLoadOrigin::UpgradeableCache,
            ..
        }
    ));
    app.enqueue_idle_upgrades();
    assert_eq!(app.requested.get(&0), Some(&true));
    let upgrade = take_audio_request(&app);
    assert_eq!(upgrade.input_seq, app.input_seq);
    assert_eq!(upgrade.source_policy, LoadSourcePolicy::SourceOnly);
    finish_audio_request(&mut app, &ctx, upgrade, &cache_dir, 64, decision);
    assert!(matches!(
        app.thumbnails[0],
        ThumbnailState::Loaded {
            origin: crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 64
            },
            ..
        }
    ));
    app.enqueue_idle_upgrades();
    assert!(
        app.requested.is_empty(),
        "the completed source request must converge"
    );
}

#[test]
fn audio_idle_upgrade_enlarge_keeps_sidecar_and_rejects_old_generation() {
    let mut app = phase_c_support::setup_app();
    let path = app.tmp.path().join("song.flac");
    let sidecar = app.tmp.path().join("song.png");
    let cache_dir = app.tmp.path().join("art-cache");
    std::fs::write(&path, b"audio").unwrap();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        512,
        256,
        image::Rgb([50, 90, 160]),
    ))
    .save_with_format(&sidecar, image::ImageFormat::Png)
    .unwrap();
    app.video_thumb_overrides.insert(
        crate::path_key::normalize_keep_drive(&path),
        sidecar.clone(),
    );
    app.items = vec![GridItem::Audio(path)];
    app.image_metas = vec![Some((12345, 999))];
    app.thumbnails = vec![ThumbnailState::Pending];
    prepare_idle(&mut app, 64);
    let ctx = egui::Context::default();
    app.enqueue_priority_thumbnail(0);
    let request = take_audio_request(&app);
    finish_audio_request(
        &mut app,
        &ctx,
        request,
        &cache_dir,
        64,
        CacheDecision::without_thumbnail(),
    );
    app.display_px_shared.store(256, Ordering::Relaxed);
    app.enqueue_idle_upgrades();
    let mut upgrade = take_audio_request(&app);
    assert_eq!(upgrade.input_seq, app.input_seq);
    assert_eq!(upgrade.source_policy, LoadSourcePolicy::SourceOnly);
    let crate::thumb_loader::LoadRequestSource::AudioThumbnail(audio) = &upgrade.raw_source else {
        unreachable!()
    };
    assert_eq!(audio.sidecar.as_ref(), Some(&sidecar));
    let original_generation = upgrade.items_gen;
    let stale = upgrade.clone();
    finish_audio_request(
        &mut app,
        &ctx,
        upgrade.clone(),
        &cache_dir,
        256,
        CacheDecision::without_thumbnail(),
    );
    assert!(matches!(
        app.thumbnails[0],
        ThumbnailState::Loaded {
            rendered_at_px: 256,
            ..
        }
    ));
    app.enqueue_idle_upgrades();
    assert!(
        app.requested.is_empty(),
        "enlargement completes once at the evaluated size"
    );
    app.items_generation = original_generation.wrapping_add(1);
    app.thumbnails[0] = ThumbnailState::NoArt;
    upgrade = stale;
    let crate::thumb_loader::LoadRequestSource::AudioThumbnail(audio) = &mut upgrade.raw_source
    else {
        unreachable!()
    };
    audio.cache_dir = cache_dir;
    audio.admission = crate::catalog::CatalogAccess::for_cache_dir(&audio.cache_dir).admit();
    let cache_map = std::sync::RwLock::new(std::collections::HashMap::new());
    let raw_handoff = crate::thumb_loader::RawThumbHandoff::DedicatedWorker(Arc::new(
        crate::raw::RawDevelopExecutor::new(1).unwrap(),
    ));
    crate::thumb_loader::process_load_request(
        &mut upgrade,
        &cache_map,
        &app.tx,
        None,
        64,
        75,
        256,
        CacheDecision::without_thumbnail(),
        &app.cache_gen_done,
        &Arc::new(Mutex::new(crate::stats::ThumbStats::default())),
        None,
        &app.keep_start_shared,
        &app.keep_end_shared,
        None,
        None,
        None,
        None,
        Some(&raw_handoff),
    );
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(
        matches!(app.thumbnails[0], ThumbnailState::NoArt),
        "old generation artwork cannot replace the new terminal state"
    );
}
