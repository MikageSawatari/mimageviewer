use super::*;
use crate::{WgpuError, WgpuSetup};
use epaint::{
    Color32, ColorImage, ImageDelta, TextureId,
    textures::{TextureFilter, TextureOptions, TexturesDelta},
};
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(windows)]
#[test]
fn surface_size_limit_constrains_initial_safe_unsafe_resize_and_recreate() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClientRect, GetWindowRect, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSENDCHANGING, SWP_NOZORDER,
        SetWindowPos,
    };
    use winit::{
        event_loop::EventLoop,
        platform::windows::EventLoopBuilderExtWindows as _,
        raw_window_handle::{HasWindowHandle as _, RawWindowHandle},
        window::Window,
    };
    const LIMIT: u32 = 1024;
    let event_loop = EventLoop::builder().with_any_thread(true).build().unwrap();
    #[expect(deprecated)] // Hidden standalone test windows, no application/event-loop run.
    let window = Arc::new(
        event_loop
            .create_window(
                Window::default_attributes()
                    .with_visible(false)
                    .with_inner_size(winit::dpi::PhysicalSize::new(1600, 1400)),
            )
            .unwrap(),
    );
    let RawWindowHandle::Win32(handle) = window.window_handle().unwrap().as_raw() else {
        panic!("expected Win32 window")
    };
    let hwnd = handle.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    assert!(
        window.inner_size().width > LIMIT,
        "fixture must exceed the limit before registration"
    );

    let mut config = WgpuConfiguration::default();
    let WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup else {
        unreachable!()
    };
    setup.instance_descriptor.backends = wgpu::Backends::DX12;
    // Match eframe's instance setup; wgpu 30's unsafe from_window target needs the
    // display on the instance, whereas safe create_surface also accepts it from Window.
    setup.display_handle = Some(Box::new(event_loop.owned_display_handle()));
    let descriptor = Arc::clone(&setup.device_descriptor);
    setup.device_descriptor = Arc::new(move |adapter| {
        let mut descriptor = descriptor(adapter);
        descriptor.required_limits.max_texture_dimension_2d = LIMIT;
        descriptor
    });
    let mut painter = pollster::block_on(Painter::new(
        Context::default(),
        config,
        false,
        RendererOptions {
            depth_stencil_format: Some(wgpu::TextureFormat::Depth24Plus),
            msaa_samples: 4,
            ..Default::default()
        },
    ));
    // Like the mandatory discard GPU regressions, unavailable prerequisites fail explicitly.
    // No adapter/device/window error is silently counted as a passing test.
    pollster::block_on(painter.set_window(ViewportId::ROOT, Some(Arc::clone(&window))))
        .expect("surface_size_limit requires a window-compatible wgpu adapter");
    assert_eq!(painter.max_texture_side(), Some(LIMIT as usize));
    let assert_bounded = |painter: &Painter| {
        let mut outer = windows_sys::Win32::Foundation::RECT::default();
        let mut client = windows_sys::Win32::Foundation::RECT::default();
        assert_ne!(unsafe { GetWindowRect(hwnd, &mut outer) }, 0);
        assert_ne!(unsafe { GetClientRect(hwnd, &mut client) }, 0);
        assert!(
            outer.right - outer.left <= LIMIT as i32 && outer.bottom - outer.top <= LIMIT as i32
        );
        let state = &painter.surfaces[&ViewportId::ROOT];
        assert_eq!(
            (state.width, state.height),
            (
                (client.right - client.left) as u32,
                (client.bottom - client.top) as u32
            )
        );
        assert!(state.width <= LIMIT && state.height <= LIMIT);
        assert!(painter.depth_texture_view.contains_key(&ViewportId::ROOT));
        assert!(painter.msaa_texture_view.contains_key(&ViewportId::ROOT));
    };
    assert_bounded(&painter);
    let bypass = || {
        assert_ne!(
            unsafe {
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    1800,
                    1500,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING,
                )
            },
            0
        );
        let mut client = windows_sys::Win32::Foundation::RECT::default();
        assert_ne!(unsafe { GetClientRect(hwnd, &mut client) }, 0);
        assert!(client.right > LIMIT as i32);
        (
            NonZeroU32::new(client.right as u32).unwrap(),
            NonZeroU32::new(client.bottom as u32).unwrap(),
        )
    };
    let (width, height) = bypass();
    painter.on_window_resized(ViewportId::ROOT, width, height);
    assert_bounded(&painter);
    // Status-based reconfiguration uses the same validated SurfaceState dimensions.
    let state = &painter.surfaces[&ViewportId::ROOT];
    Painter::configure_surface(
        state,
        painter.render_state.as_ref().unwrap(),
        &painter.config,
        ViewportId::ROOT,
        false,
    );
    painter.recreate_surface(ViewportId::ROOT, &window).unwrap();
    assert_bounded(&painter);
    pollster::block_on(painter.set_window(ViewportId::ROOT, None)).unwrap();
    // The subclass intentionally survives removal, and the unsafe attachment must reacquire
    // the corrected size after device initialization just like the safe attachment.
    bypass();
    unsafe { pollster::block_on(painter.set_window_unsafe(ViewportId::ROOT, Some(&window))) }
        .unwrap();
    assert_bounded(&painter);
    pollster::block_on(painter.set_window(ViewportId::ROOT, None)).unwrap();
}

#[cfg(windows)]
#[test]
fn zero_client_surface_skips_configure_and_acquire_then_resumes_rendering() {
    use crate::StartupDiagnosticEvent;
    use std::{sync::Mutex, time::Duration};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSENDCHANGING, SWP_NOZORDER, SetWindowPos,
    };
    use winit::{
        event_loop::EventLoop,
        platform::windows::EventLoopBuilderExtWindows as _,
        raw_window_handle::{HasWindowHandle as _, RawWindowHandle},
        window::Window,
    };
    const LIMIT: u32 = 1024;
    let event_loop = EventLoop::builder().with_any_thread(true).build().unwrap();
    #[expect(deprecated)] // Hidden test HWND; never run an application or activate the window.
    let window = Arc::new(
        event_loop
            .create_window(
                Window::default_attributes()
                    .with_visible(false)
                    .with_decorations(false)
                    .with_inner_size(winit::dpi::PhysicalSize::new(640, 480)),
            )
            .unwrap(),
    );
    let RawWindowHandle::Win32(handle) = window.window_handle().unwrap().as_raw() else {
        panic!("expected Win32 window")
    };
    let resize_native = |width, height| {
        assert_ne!(
            unsafe {
                SetWindowPos(
                    handle.hwnd.get() as _,
                    std::ptr::null_mut(),
                    0,
                    0,
                    width,
                    height,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING,
                )
            },
            0
        );
        let size = window.inner_size();
        assert_eq!((size.width, size.height), (width as u32, height as u32));
        size
    };
    // Observe existing native call boundaries, without adding production counters/state.
    let stages = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&stages);
    let mut config = WgpuConfiguration {
        startup_diagnostics: Some(Arc::new(move |event| {
            if let StartupDiagnosticEvent::Begin(stage) = event {
                observed.lock().unwrap().push(stage);
            }
        })),
        ..Default::default()
    };
    let WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup else {
        unreachable!()
    };
    setup.instance_descriptor.backends = wgpu::Backends::DX12;
    setup.display_handle = Some(Box::new(event_loop.owned_display_handle()));
    let descriptor = Arc::clone(&setup.device_descriptor);
    setup.device_descriptor = Arc::new(move |adapter| {
        let mut descriptor = descriptor(adapter);
        descriptor.required_limits.max_texture_dimension_2d = LIMIT;
        descriptor
    });
    let mut painter = pollster::block_on(Painter::new(
        Context::default(),
        config,
        false,
        RendererOptions {
            depth_stencil_format: Some(wgpu::TextureFormat::Depth24Plus),
            msaa_samples: 4,
            ..Default::default()
        },
    ));
    for (width, height) in [(0, 480), (640, 0), (0, 0)] {
        resize_native(width, height);
        pollster::block_on(painter.set_window(ViewportId::ROOT, Some(Arc::clone(&window))))
            .expect("zero-client regression requires a window-compatible wgpu adapter");
        assert!(
            painter.surfaces[&ViewportId::ROOT]
                .surface
                .get_configuration()
                .is_none()
        );
        // A stale oversize event must re-query the real zero-sized client, then return
        // before configure or depth/MSAA creation. Removing the resize guard must fail.
        painter.on_window_resized(
            ViewportId::ROOT,
            NonZeroU32::new(LIMIT + 1).unwrap(),
            NonZeroU32::new(LIMIT + 1).unwrap(),
        );
        let state = &painter.surfaces[&ViewportId::ROOT];
        assert_eq!((state.width, state.height), (width as u32, height as u32));
        assert!(!painter.depth_texture_view.contains_key(&ViewportId::ROOT));
        assert!(!painter.msaa_texture_view.contains_key(&ViewportId::ROOT));
        for needs_reconfigure in [false, true] {
            painter
                .surfaces
                .get_mut(&ViewportId::ROOT)
                .unwrap()
                .needs_reconfigure = needs_reconfigure;
            let mut delta = TexturesDelta::default();
            painter.paint_and_update_textures_impl(
                ViewportId::ROOT,
                1.0,
                [0.0, 0.0, 0.0, 1.0],
                &[],
                &mut delta,
                Vec::new(),
                Some(&window),
                Some(RootStartupFrameTag {
                    normal: true,
                    ..Default::default()
                }),
            );
        }
        let observed = stages.lock().unwrap();
        assert!(
            !observed.contains(&StartupGpuStage::SurfaceConfigure),
            "{observed:?}"
        );
        assert!(
            !observed.contains(&StartupGpuStage::SurfaceAcquire),
            "{observed:?}"
        );
        drop(observed);
        // Retain the last zero-sized surface for the zero -> nonzero recovery below.
        if (width, height) != (0, 0) {
            pollster::block_on(painter.set_window(ViewportId::ROOT, None)).unwrap();
        }
    }
    let size = resize_native(640, 480);
    painter.on_window_resized(
        ViewportId::ROOT,
        NonZeroU32::new(size.width).unwrap(),
        NonZeroU32::new(size.height).unwrap(),
    );
    let state = &painter.surfaces[&ViewportId::ROOT];
    let configured = state.surface.get_configuration().unwrap();
    assert_eq!((configured.width, configured.height), (640, 480));
    assert!(painter.depth_texture_view.contains_key(&ViewportId::ROOT));
    assert!(painter.msaa_texture_view.contains_key(&ViewportId::ROOT));
    let (tx, rx) = std::sync::mpsc::channel();
    // A hidden surface may be occluded. Read back the Painter's actual rendered frame
    // through its normal capture path rather than requiring a visible desktop window.
    painter.paint_and_update_textures_impl(
        ViewportId::ROOT,
        1.0,
        [0.0, 0.0, 0.0, 1.0],
        &[],
        &mut TexturesDelta::default(),
        vec![egui::ScreenshotCallback::new(move |image| {
            tx.send(image).unwrap()
        })],
        Some(&window),
        Some(RootStartupFrameTag {
            normal: true,
            ..Default::default()
        }),
    );
    painter
        .render_state
        .as_ref()
        .unwrap()
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(10)),
        })
        .unwrap();
    let image = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(image.size, [640, 480]);
    assert!(image.pixels.iter().all(|pixel| *pixel == Color32::BLACK));
    let observed = stages.lock().unwrap();
    for stage in [
        StartupGpuStage::SurfaceConfigure,
        StartupGpuStage::SurfaceAcquire,
        StartupGpuStage::Encode,
        StartupGpuStage::QueueSubmit,
    ] {
        assert!(observed.contains(&stage), "missing {stage:?}: {observed:?}");
    }
}

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

// Test-only non-cancelling receive, matching the eframe shared owner contract.
fn receive_discard_delta(pending: &mut TexturesDelta, mut newer: TexturesDelta) {
    for (id, updates) in newer.set.drain() {
        for update in updates {
            pending.push(id, update);
        }
    }
    pending.free.extend(core::mem::take(&mut newer.free));
}

#[test]
fn retirement_texture_delivery_offscreen_startup_preserves_first_full_atlas_and_user_texture() {
    use std::{cell::RefCell, rc::Rc};

    for scenario in ["empty", "discard", "immediate"] {
        let painter = headless_painter()
            .expect("startup retirement regression requires a headless wgpu adapter");
        let ctx = painter.context.clone();
        let state = painter.render_state().unwrap();
        let painter = Rc::new(RefCell::new(painter));
        let pending = Rc::new(RefCell::new(TexturesDelta::default()));

        // Exercise the actual ready Painter before any Full. In the broken
        // implementation this submits the reserved 0x0 atlas to the real GPU;
        // inspect validation only after the consumer has run.
        let validation = state.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut initial = ctx.take_viewport_retirement_texture_delta();
        painter.borrow_mut().apply_textures_delta(&mut initial);
        let startup_error = pollster::block_on(validation.pop());
        initial.clear();
        assert!(startup_error.is_none(), "{scenario}: {startup_error:?}");
        assert!(
            state
                .renderer
                .read()
                .texture(&TextureId::default())
                .is_none()
        );

        let texture = ctx.load_texture(
            "pre-Full user texture",
            ColorImage::filled([2, 2], Color32::RED),
            TextureOptions::NEAREST,
        );
        let id = texture.id();
        let validation = state.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut before_full = ctx.take_viewport_retirement_texture_delta();
        painter.borrow_mut().apply_textures_delta(&mut before_full);
        assert!(pollster::block_on(validation.pop()).is_none());
        assert!(before_full.is_empty());
        assert!(state.renderer.read().texture(&id).is_none());

        let validation = state.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let resource = Rc::downgrade(&painter);
        let owner = Rc::downgrade(&pending);
        let _scope = ctx.enter_discard_texture_delivery(move |delta| {
            let owner = owner.upgrade().unwrap();
            receive_discard_delta(&mut owner.borrow_mut(), delta);
            resource
                .upgrade()
                .unwrap()
                .borrow_mut()
                .apply_textures_delta(&mut owner.borrow_mut());
        });
        let child = ViewportId::from_hash_of("startup first immediate");
        if scenario == "immediate" {
            ctx.set_embed_viewports(false);
            let resource = Rc::downgrade(&painter);
            let owner = Rc::downgrade(&pending);
            Context::set_immediate_viewport_renderer(move |ctx, mut immediate| {
                let mut input = egui::RawInput {
                    viewport_id: immediate.ids.this,
                    ..Default::default()
                };
                input
                    .viewports
                    .entry(immediate.ids.this)
                    .or_default()
                    .parent = Some(immediate.ids.parent);
                let mut output = ctx.run_ui(input, |ui| (immediate.viewport_ui_cb)(ui));
                let owner = owner.upgrade().unwrap();
                receive_discard_delta(
                    &mut owner.borrow_mut(),
                    core::mem::take(&mut output.textures_delta),
                );
                resource
                    .upgrade()
                    .unwrap()
                    .borrow_mut()
                    .apply_textures_delta(&mut owner.borrow_mut());
                output.drop_without_applying_deltas();
            });
        }
        let mut passes = 0;
        let mut output = ctx.run_ui(Default::default(), |ui| {
            if scenario == "discard" && passes == 0 {
                ui.ctx().request_discard("first Full atlas publication");
            }
            if scenario == "immediate" {
                ui.ctx()
                    .show_viewport_immediate(child, Default::default(), |_, _| {});
            }
            passes += 1;
        });
        assert_eq!(passes, if scenario == "discard" { 2 } else { 1 });
        receive_discard_delta(
            &mut pending.borrow_mut(),
            core::mem::take(&mut output.textures_delta),
        );
        output.drop_without_applying_deltas();
        painter
            .borrow_mut()
            .apply_textures_delta(&mut pending.borrow_mut());
        assert!(pending.borrow().is_empty());
        let atlas_size = state
            .renderer
            .read()
            .texture_size(&TextureId::default())
            .expect("first Full must publish the real font atlas even without text");
        assert!(atlas_size[0] > 0 && atlas_size[1] > 0);
        assert_pixels(&state, id, &[Color32::RED; 4]);
        assert!(pollster::block_on(validation.pop()).is_none());

        // Once the atlas is published, an ordinary handle drop must still
        // reach the same no-paint consumer as a free-only batch.
        drop(texture);
        let validation = state.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut free_only = ctx.take_viewport_retirement_texture_delta();
        assert!(free_only.set.is_empty());
        assert!(free_only.free.contains(&id));
        painter.borrow_mut().apply_textures_delta(&mut free_only);
        assert!(free_only.is_empty());
        assert!(state.renderer.read().texture(&id).is_none());
        assert!(
            state
                .renderer
                .read()
                .texture(&TextureId::default())
                .is_some()
        );
        assert!(painter.borrow().surfaces.is_empty());
        assert!(pollster::block_on(validation.pop()).is_none());
    }
}

#[test]
fn discard_texture_delivery_offscreen_actual_retry_immediate_callback_drop_readback() {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };
    // Prerequisite failure is a test failure, never a successful skip.
    let painter =
        headless_painter().expect("discard_texture_delivery requires a headless wgpu adapter");
    let state = painter.render_state().unwrap();
    let validation = state.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let painter = Rc::new(RefCell::new(painter));
    let pending = Rc::new(RefCell::new(TexturesDelta::default()));
    let ctx = Context::default();
    ctx.set_embed_viewports(false);
    let mut texture = ctx.load_texture(
        "old native callback",
        ColorImage::filled([2, 2], Color32::BLACK),
        TextureOptions::NEAREST,
    );
    let id = texture.id();
    texture.set_partial(
        [1, 0],
        ColorImage::filled([1, 1], Color32::RED),
        TextureOptions::NEAREST,
    );
    // This callback is the last TextureHandle owner, retained independently of
    // Context registration, exactly like a stale native viewport callback.
    let old_callback: Rc<RefCell<Option<Arc<dyn Fn(&mut egui::Ui) + Send + Sync>>>> =
        Rc::new(RefCell::new(Some(Arc::new(move |_| {
            let _ = &texture;
        }))));
    let older = TextureId::User(1000);
    receive_discard_delta(&mut pending.borrow_mut(), full(older, Color32::WHITE));
    let deliveries = Rc::new(Cell::new(0));
    let got = deliveries.clone();
    let resource = Rc::downgrade(&painter);
    let owner = Rc::downgrade(&pending);
    let observed_state = state.clone();
    let _scope = ctx.enter_discard_texture_delivery(move |delta| {
        assert_eq!(delta.set[&id].len(), 2);
        let resource = resource.upgrade().unwrap();
        let owner = owner.upgrade().unwrap();
        receive_discard_delta(&mut owner.borrow_mut(), delta);
        let before = submission_counts();
        resource
            .borrow_mut()
            .apply_textures_delta(&mut owner.borrow_mut());
        assert_submissions(before, 0, 1); // returned consumer upload submit, before readback
        assert!(owner.borrow().is_empty());
        assert_pixels(&observed_state, older, &[Color32::WHITE; 4]);
        assert_pixels(
            &observed_state,
            id,
            &[Color32::BLACK, Color32::RED, Color32::BLACK, Color32::BLACK],
        );
        got.set(got.get() + 1);
    });
    let observed = deliveries.clone();
    let resource = Rc::downgrade(&painter);
    let owner = Rc::downgrade(&pending);
    let old = old_callback.clone();
    let observed_state = state.clone();
    let child = ViewportId::from_hash_of("discard-resource-immediate");
    Context::set_immediate_viewport_renderer(move |ctx, mut immediate| {
        assert_eq!(
            observed.get(),
            1,
            "readback and upload submit preceded replacement"
        );
        assert!(observed_state.renderer.read().texture(&id).is_some());
        drop(old.borrow_mut().take()); // last actual TextureHandle drop, no fabricated free
        let delta = ctx.tex_manager().write().take_delta();
        assert!(delta.free.contains(&id));
        let owner = owner.upgrade().unwrap();
        receive_discard_delta(&mut owner.borrow_mut(), delta);
        let before = submission_counts();
        resource
            .upgrade()
            .unwrap()
            .borrow_mut()
            .apply_textures_delta(&mut owner.borrow_mut());
        assert_submissions(before, 0, 1);
        assert!(observed_state.renderer.read().texture(&id).is_none());
        assert!(
            observed_state.renderer.read().texture(&older).is_some(),
            "sibling resource survives"
        );
        let mut input = egui::RawInput {
            viewport_id: immediate.ids.this,
            ..Default::default()
        };
        input
            .viewports
            .entry(immediate.ids.this)
            .or_default()
            .parent = Some(immediate.ids.parent);
        let mut output = ctx.run_ui(input, |ui| (immediate.viewport_ui_cb)(ui));
        assert!(!output.textures_delta.set.contains_key(&id));
        receive_discard_delta(
            &mut owner.borrow_mut(),
            core::mem::take(&mut output.textures_delta),
        );
        if !owner.borrow().is_empty() {
            resource
                .upgrade()
                .unwrap()
                .borrow_mut()
                .apply_textures_delta(&mut owner.borrow_mut());
        }
        output.drop_without_applying_deltas();
    });
    let mut pass = 0;
    let mut output = ctx.run_ui(Default::default(), |ui| {
        if pass == 0 {
            ui.ctx().request_discard("resource ordering retry");
        } else {
            ui.ctx()
                .show_viewport_immediate(child, Default::default(), |_, _| {});
        }
        pass += 1;
    });
    assert_eq!(pass, 2);
    assert_eq!(deliveries.get(), 1);
    assert!(old_callback.borrow().is_none());
    assert!(!output.textures_delta.set.contains_key(&id));
    receive_discard_delta(
        &mut pending.borrow_mut(),
        core::mem::take(&mut output.textures_delta),
    );
    if !pending.borrow().is_empty() {
        painter
            .borrow_mut()
            .apply_textures_delta(&mut pending.borrow_mut());
    }
    output.drop_without_applying_deltas();
    assert!(
        painter.borrow().surfaces.is_empty(),
        "no surface, paint or present used"
    );
    assert!(pollster::block_on(validation.pop()).is_none());
}

#[test]
fn discard_texture_delivery_offscreen_unavailable_pending_later_free_not_cancelled() {
    use std::{cell::RefCell, rc::Rc};
    let ready =
        headless_painter().expect("discard_texture_delivery requires a headless wgpu adapter");
    let state = ready.render_state().unwrap();
    let validation = state.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let ctx = Context::default();
    let painter = Rc::new(RefCell::new(pollster::block_on(Painter::new(
        ctx.clone(),
        Default::default(),
        false,
        Default::default(),
    ))));
    let pending = Rc::new(RefCell::new(TexturesDelta::default()));
    let owner = pending.clone();
    let resource = painter.clone();
    let _scope = ctx.enter_discard_texture_delivery(move |delta| {
        receive_discard_delta(&mut owner.borrow_mut(), delta);
        resource
            .borrow_mut()
            .apply_textures_delta(&mut owner.borrow_mut());
        assert!(
            !owner.borrow().is_empty(),
            "existing pending owns unavailable resources"
        );
    });
    let mut handle = Some(ctx.load_texture(
        "unavailable whole/partial",
        ColorImage::filled([2, 2], Color32::BLACK),
        TextureOptions::NEAREST,
    ));
    handle.as_mut().unwrap().set_partial(
        [1, 0],
        ColorImage::filled([1, 1], Color32::RED),
        TextureOptions::NEAREST,
    );
    let id = handle.as_ref().unwrap().id();
    let mut pass = 0;
    let mut output = ctx.run_ui(Default::default(), |ui| {
        if pass == 0 {
            ui.ctx().request_discard("unavailable renderer");
        } else {
            drop(handle.take());
        }
        pass += 1;
    });
    receive_discard_delta(
        &mut pending.borrow_mut(),
        core::mem::take(&mut output.textures_delta),
    );
    output.drop_without_applying_deltas();
    assert_eq!(pending.borrow().set[&id].len(), 2);
    assert!(pending.borrow().free.contains(&id));
    painter.borrow_mut().render_state = Some(state.clone());
    let before = submission_counts();
    painter
        .borrow_mut()
        .apply_textures_delta(&mut pending.borrow_mut());
    assert_submissions(before, 0, 1);
    assert!(pending.borrow().is_empty());
    assert!(state.renderer.read().texture(&id).is_none());
    assert!(
        pollster::block_on(validation.pop()).is_none(),
        "whole/partial submitted before later free"
    );
}
