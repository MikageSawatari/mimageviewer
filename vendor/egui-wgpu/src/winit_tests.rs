use super::*;
use crate::{WgpuError, WgpuSetup};
use epaint::{
    Color32, ColorImage, ImageDelta, TextureId,
    textures::{TextureFilter, TextureOptions, TexturesDelta},
};
use std::sync::atomic::{AtomicUsize, Ordering};

fn headless_painter() -> Option<Painter> {
    let mut config = WgpuConfiguration::default();
    let WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup else {
        unreachable!()
    };
    setup.instance_descriptor.backends = if cfg!(target_os = "windows") {
        wgpu::Backends::DX12
    } else {
        wgpu::Backends::PRIMARY
    };
    let options = RendererOptions {
        dithering: false,
        ..Default::default()
    };
    let mut painter = pollster::block_on(Painter::new(
        Context::default(),
        config.clone(),
        false,
        options,
    ));
    match pollster::block_on(RenderState::create(
        &config,
        &painter.instance,
        None,
        options,
    )) {
        Ok(state) => painter.render_state = Some(state),
        Err(WgpuError::RequestAdapterError(error)) => {
            eprintln!("skipping headless winit GPU regression: {error}");
            return None;
        }
        Err(error) => panic!("create headless render state: {error}"),
    }
    Some(painter)
}

fn full(id: TextureId, color: Color32) -> TexturesDelta {
    let mut delta = TexturesDelta::default();
    delta.push(
        id,
        ImageDelta::full(ColorImage::filled([2, 2], color), TextureOptions::NEAREST),
    );
    delta
}

fn paint_without_surface(painter: &mut Painter, delta: &mut TexturesDelta) {
    assert!(painter.surfaces.is_empty());
    painter.paint_and_update_textures_impl(
        ViewportId::ROOT,
        1.0,
        [0.0; 4],
        &[],
        delta,
        Vec::new(),
        None,
        Some(RootStartupFrameTag {
            frame: 1,
            pass: 1,
            normal: true,
        }),
    );
}

// This checks GPU buffer data as well as texture pixels below. Readback did not detect an
// omitted fallback submit in the local mutation check, so submission proof uses the
// returned-call counters rather than this map/readback result.
fn pending_write(state: &RenderState) -> wgpu::Buffer {
    let buffer = state.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("delivery buffer data"),
        size: 4,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    state
        .queue
        .write_buffer(&buffer, 0, &[0x21, 0x43, 0x65, 0x87]);
    buffer
}

fn assert_buffer_data(state: &RenderState, buffer: &wgpu::Buffer) {
    let (tx, rx) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });
    state
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .expect("poll delivery witness");
    rx.recv_timeout(std::time::Duration::from_secs(30))
        .expect("delivery map callback")
        .expect("delivery map result");
    let mapped = buffer
        .slice(..)
        .get_mapped_range()
        .expect("mapped delivery witness");
    assert_eq!(
        &*mapped,
        &[0x21, 0x43, 0x65, 0x87],
        "queued buffer data must reach the mapped buffer"
    );
    drop(mapped);
    buffer.unmap();
}

fn assert_submissions(before: SubmissionCounts, normal: usize, empty: usize) {
    let after = submission_counts();
    assert_eq!(
        after.normal - before.normal,
        normal,
        "returned normal Queue::submit calls"
    );
    assert_eq!(
        after.empty - before.empty,
        empty,
        "returned fallback Queue::submit([]) calls"
    );
}

// Records only returned operations; the second Begin/End must enclose both fallback submit
// and free. The callback may inspect the renderer at End because finalization releases its lock.
fn assert_delivery_spans(
    events: &[(bool, bool, SubmissionCounts, bool)],
    before: SubmissionCounts,
    created_and_freed: bool,
) {
    assert_eq!(
        events.len(),
        4,
        "set and finalization must each emit a paired stage"
    );
    assert_eq!(
        events
            .iter()
            .map(|event| (event.0, event.1))
            .collect::<Vec<_>>(),
        [(true, true), (false, true), (true, true), (false, true)]
    );
    assert_eq!(
        events[2].2, before,
        "fallback submit must remain inside finalization's stage"
    );
    assert_eq!(
        events[3].2,
        SubmissionCounts {
            normal: before.normal,
            empty: before.empty + 1
        },
        "finalization End must follow the returned submit"
    );
    if created_and_freed {
        assert!(events[1].3, "set End must precede free");
    }
    assert!(!events[3].3, "finalization End must follow free");
}

fn assert_pixels(state: &RenderState, id: TextureId, expected: &[Color32]) {
    let pixels = crate::renderer::tests::read_rendered_texture(
        &state.device,
        &state.queue,
        &mut state.renderer.write(),
        id,
        [2, 2],
    );
    assert_eq!(pixels, expected);
}

#[test]
fn no_surface_drains_ordered_full_partial_submits_uploads_then_frees() {
    let Some(mut painter) = headless_painter() else {
        return;
    };
    let state = painter.render_state().unwrap();
    let presented = Arc::new(AtomicUsize::new(0));
    let presented_callback = presented.clone();
    let delivery_events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let callback_events = delivery_events.clone();
    let observed_state = state.clone();
    painter.config.startup_diagnostics = Some(Arc::new(move |event| {
        let stage = match event {
            crate::StartupDiagnosticEvent::Begin(StartupGpuStage::TextureDelivery) => {
                Some((true, true))
            }
            crate::StartupDiagnosticEvent::End {
                stage: StartupGpuStage::TextureDelivery,
                success,
            } => Some((false, success)),
            _ => None,
        };
        if let Some((begin, success)) = stage {
            callback_events.lock().unwrap().push((
                begin,
                success,
                submission_counts(),
                observed_state
                    .renderer
                    .read()
                    .texture(&TextureId::Managed(1))
                    .is_some(),
            ));
        }
        if matches!(event, crate::StartupDiagnosticEvent::RootPresented { .. }) {
            presented_callback.fetch_add(1, Ordering::Relaxed);
        }
    }));
    // A replacement is followed by two overlapping partial writes under the same ID.
    let id = TextureId::Managed(0);
    let mut seed = TexturesDelta::default();
    seed.push(
        id,
        ImageDelta::full(
            ColorImage::filled([1, 1], Color32::WHITE),
            TextureOptions::NEAREST,
        ),
    );
    painter.apply_textures_delta(&mut seed);
    let mut delta = full(id, Color32::BLACK);
    delta.push(
        id,
        ImageDelta::partial(
            [0, 0],
            ColorImage::filled([2, 1], Color32::RED),
            TextureOptions::NEAREST,
        ),
    );
    delta.push(
        id,
        ImageDelta::partial(
            [1, 0],
            ColorImage::filled([1, 1], Color32::GREEN),
            TextureOptions::NEAREST,
        ),
    );
    let witness = pending_write(&state);
    let submissions = submission_counts();
    let events_before = delivery_events.lock().unwrap().len();
    paint_without_surface(&mut painter, &mut delta);
    assert_submissions(submissions, 0, 1);
    assert_delivery_spans(
        &delivery_events.lock().unwrap()[events_before..],
        submissions,
        false,
    );
    assert!(delta.is_empty());
    assert_buffer_data(&state, &witness);
    assert_pixels(
        &state,
        id,
        &[Color32::RED, Color32::GREEN, Color32::BLACK, Color32::BLACK],
    );

    // Creation and free in the same batch validates submit-before-destroy: destroying before
    // the queued write is submitted invalidates that write and produces a validation error.
    let validation = state.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let freed = TextureId::Managed(1);
    let mut create_and_free = full(freed, Color32::WHITE);
    create_and_free.free(freed);
    let submissions = submission_counts();
    let events_before = delivery_events.lock().unwrap().len();
    paint_without_surface(&mut painter, &mut create_and_free);
    assert_submissions(submissions, 0, 1);
    assert_delivery_spans(
        &delivery_events.lock().unwrap()[events_before..],
        submissions,
        true,
    );
    assert!(create_and_free.is_empty());
    assert!(state.renderer.read().texture(&freed).is_none());
    let error = pollster::block_on(validation.pop());
    assert!(error.is_none(), "free must follow submission: {error:?}");

    let witness = pending_write(&state);
    let mut free_only = TexturesDelta::default();
    free_only.free(id);
    let submissions = submission_counts();
    painter.apply_textures_delta(&mut free_only);
    assert_submissions(submissions, 0, 1);
    assert!(free_only.is_empty());
    assert_buffer_data(&state, &witness);
    assert!(state.renderer.read().texture(&id).is_none());
    assert_eq!(
        presented.load(Ordering::Relaxed),
        0,
        "texture-only delivery cannot prove presentation"
    );
}

#[test]
fn renderer_absence_retains_both_sets_and_frees_for_existing_pending_owner() {
    let config = WgpuConfiguration::default();
    let mut painter = pollster::block_on(Painter::new(
        Context::default(),
        config,
        false,
        RendererOptions::default(),
    ));
    let id = TextureId::Managed(0);
    let mut delta = full(id, Color32::WHITE);
    delta.free(id);
    let submissions = submission_counts();
    painter.apply_textures_delta(&mut delta);
    assert_submissions(submissions, 0, 0);
    assert_eq!(delta.set[&id].len(), 1);
    assert!(delta.free.contains(&id));
    let submissions = submission_counts();
    paint_without_surface(&mut painter, &mut delta);
    assert_submissions(submissions, 0, 0);
    assert_eq!(delta.set[&id].len(), 1);
    assert!(delta.free.contains(&id));
    // This headless test has established that no consumer exists, so end its logical batch.
    delta.clear();
}

#[test]
fn acquisition_failure_equivalent_and_normal_submission_share_completion_owner() {
    let Some(painter) = headless_painter() else {
        return;
    };
    let state = painter.render_state().unwrap();
    for normal_submit in [false, true] {
        let validation = state.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let id = TextureId::Managed(2);
        let mut delta = full(id, Color32::WHITE);
        delta.free(id);
        let witness = pending_write(&state);
        let submissions = submission_counts();
        {
            let mut delivery = TextureDelivery::begin(
                Some(&state),
                &mut delta,
                crate::atlas_diag::Site::AppliedPaint,
                None,
            )
            .unwrap();
            // This is the same owner/drop boundary as a failed acquire after buffer preparation.
            let mut encoder = state.device.create_command_encoder(&Default::default());
            let _ = state.renderer.write().update_buffers(
                &state.device,
                &state.queue,
                &mut encoder,
                &[],
                &renderer::ScreenDescriptor {
                    size_in_pixels: [2, 2],
                    pixels_per_point: 1.0,
                },
            );
            delivery.queue_needs_submit = true;
            if normal_submit {
                delivery.submit([encoder.finish()]);
                assert!(
                    !delivery.queue_needs_submit,
                    "normal submission suppresses fallback submission"
                );
            }
        }
        assert_submissions(
            submissions,
            usize::from(normal_submit),
            usize::from(!normal_submit),
        );
        assert!(delta.is_empty());
        assert_buffer_data(&state, &witness);
        assert!(state.renderer.read().texture(&id).is_none());
        assert!(pollster::block_on(validation.pop()).is_none());
    }
}

#[test]
fn mipmap_submission_tracks_later_writes_without_duplicate_empty_submit() {
    let Some(painter) = headless_painter() else {
        return;
    };
    let state = painter.render_state().unwrap();
    for modes in [[true, false], [false, true], [true, true]] {
        let mut delta = full(TextureId::Managed(10), Color32::WHITE);
        delta.push(
            TextureId::Managed(11),
            ImageDelta::full(
                ColorImage::filled([2, 2], Color32::WHITE),
                TextureOptions::NEAREST,
            ),
        );
        // Hash-map ID order is intentionally unspecified. Assign the desired upload order to
        // that actual iteration order instead of relying on insertion order or a fixed hasher.
        let ids: Vec<_> = delta.set.keys().copied().collect();
        for (id, mipmapped) in ids.iter().zip(modes) {
            delta.set.get_mut(id).unwrap()[0].options = TextureOptions::NEAREST
                .with_mipmap_mode(mipmapped.then_some(TextureFilter::Nearest));
        }
        let witness = pending_write(&state);
        let submissions = submission_counts();
        {
            let delivery = TextureDelivery::begin(
                Some(&state),
                &mut delta,
                crate::atlas_diag::Site::AppliedNoPaint,
                None,
            )
            .unwrap();
            assert_eq!(
                delivery.queue_needs_submit, !modes[1],
                "only writes after the most recent mipmap submit require fallback"
            );
        }
        assert_submissions(submissions, 0, usize::from(!modes[1]));
        assert!(delta.is_empty());
        assert_buffer_data(&state, &witness);
        for id in ids {
            assert_pixels(&state, id, &[Color32::WHITE; 4]);
        }
    }
}

#[test]
fn removing_one_viewport_retires_only_its_surface_and_attachments() {
    struct Tracked(Arc<AtomicUsize>);
    impl Drop for Tracked {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    let root = ViewportId::ROOT;
    let target = ViewportId::from_hash_of("retired child");
    let sibling = ViewportId::from_hash_of("live child");
    let target_drops = Arc::new(AtomicUsize::new(0));
    let sibling_drops = Arc::new(AtomicUsize::new(0));
    let mut surfaces = ViewportIdMap::default();
    let mut depths = ViewportIdMap::default();
    let mut msaa = ViewportIdMap::default();
    for map in [&mut surfaces, &mut depths, &mut msaa] {
        map.insert(target, Tracked(target_drops.clone()));
        map.insert(sibling, Tracked(sibling_drops.clone()));
        map.insert(root, Tracked(sibling_drops.clone()));
    }
    // Both safe and unsafe set_window(None) call this exact resource retirement helper.
    remove_viewport_resources(target, &mut surfaces, &mut depths, &mut msaa);
    assert_eq!(target_drops.load(Ordering::Relaxed), 3);
    assert_eq!(sibling_drops.load(Ordering::Relaxed), 0);
    for map in [&surfaces, &depths, &msaa] {
        assert!(map.contains_key(&root));
        assert!(map.contains_key(&sibling));
        assert!(!map.contains_key(&target));
    }
    remove_viewport_resources(target, &mut surfaces, &mut depths, &mut msaa);
    assert_eq!(
        target_drops.load(Ordering::Relaxed),
        3,
        "repeated retirement cannot affect siblings"
    );
    assert_eq!(sibling_drops.load(Ordering::Relaxed), 0);
}

#[test]
fn public_window_removal_paths_keep_sibling_attachments_and_shared_renderer() {
    let Some(mut painter) = headless_painter() else {
        return;
    };
    let state = painter.render_state().unwrap();
    let renderer_id = state.renderer.read().diag_id();
    let shared_id = TextureId::Managed(15);
    let mut seed = full(shared_id, Color32::WHITE);
    painter.apply_textures_delta(&mut seed);
    let target = ViewportId::from_hash_of("removed child");
    let sibling = ViewportId::from_hash_of("surviving child");
    for unsafe_api in [false, true] {
        for viewport in [ViewportId::ROOT, target, sibling] {
            for (format, samples, map) in [
                (
                    wgpu::TextureFormat::Depth32Float,
                    1,
                    &mut painter.depth_texture_view,
                ),
                (
                    wgpu::TextureFormat::Rgba8Unorm,
                    4,
                    &mut painter.msaa_texture_view,
                ),
            ] {
                let texture = state.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("public viewport removal regression attachment"),
                    size: wgpu::Extent3d {
                        width: 2,
                        height: 2,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                });
                map.insert(viewport, texture.create_view(&Default::default()));
            }
        }
        assert!(
            painter.surfaces.is_empty(),
            "actual native surfaces require a window and are covered separately by the retirement helper's drop sentinels"
        );
        if unsafe_api {
            // No window is borrowed or registered on this branch.
            pollster::block_on(unsafe { painter.set_window_unsafe(target, None) }).unwrap();
        } else {
            pollster::block_on(painter.set_window(target, None)).unwrap();
        }
        for map in [&painter.depth_texture_view, &painter.msaa_texture_view] {
            assert!(!map.contains_key(&target));
            assert!(map.contains_key(&sibling));
            assert!(map.contains_key(&ViewportId::ROOT));
        }
        let retained = painter.render_state().unwrap();
        assert!(Arc::ptr_eq(&retained.renderer, &state.renderer));
        assert_eq!(retained.renderer.read().diag_id(), renderer_id);
        assert!(retained.renderer.read().texture(&shared_id).is_some());
    }
}
