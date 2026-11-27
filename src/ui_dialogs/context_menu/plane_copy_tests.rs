use super::*;
use crate::capture::CapturePixelWork;
use crate::fs_exclusive_view::FsExclusiveView;
use crate::keymap::KeyAction;
use crate::plane_view::{PlaneViewPose, PlaneViewState};
use crate::ring_shortcut::{RingActionId, RingShortcutContext};
use std::cell::RefCell;

thread_local! {
    static CAPTURE_CLIPBOARD_SINK: RefCell<Option<Vec<CapturePixelWork>>> = const { RefCell::new(None) };
}

/// Takes ownership only while a test is explicitly capturing clipboard submissions.
pub(crate) fn intercept_capture_clipboard_work_for_test(
    work: CapturePixelWork,
) -> Option<CapturePixelWork> {
    CAPTURE_CLIPBOARD_SINK.with(|sink| {
        let mut sink = sink.borrow_mut();
        if let Some(submissions) = sink.as_mut() {
            submissions.push(work);
            None
        } else {
            Some(work)
        }
    })
}

struct ClipboardCapture;

impl ClipboardCapture {
    fn new() -> Self {
        CAPTURE_CLIPBOARD_SINK.with(|sink| {
            assert!(sink.borrow_mut().replace(Vec::new()).is_none());
        });
        Self
    }

    fn take(&self) -> Vec<CapturePixelWork> {
        CAPTURE_CLIPBOARD_SINK.with(|sink| std::mem::take(sink.borrow_mut().as_mut().unwrap()))
    }
}

impl Drop for ClipboardCapture {
    fn drop(&mut self) {
        CAPTURE_CLIPBOARD_SINK.with(|sink| *sink.borrow_mut() = None);
    }
}

fn dispatch(app: &mut crate::app::App, ctx: &egui::Context, entry: usize) {
    match entry {
        0 => {
            app.execute_copy_key_action(ctx, KeyAction::FsCopyImage);
        }
        1 => {
            let idx = app.fullscreen_idx.unwrap();
            let target = app.context_menu_target(
                idx,
                app.items[idx].clone(),
                false,
                false,
                None,
                ContextMenuSurface::Fullscreen,
                None,
            );
            app.dispatch_native_context_menu_result(
                ctx,
                NativeContextMenuResult::MivCommand(MenuCommand::CopyImageToClipboard),
                &target,
            );
        }
        2 => {
            app.apply_ring_action(
                ctx,
                RingShortcutContext::ImageFullscreen,
                RingActionId::ImageCopyToClipboard,
                "plane_copy_test",
            );
        }
        _ => unreachable!(),
    }
}

#[test]
fn plane_copy_action_menu_and_ring_reject_a_frame_that_has_not_been_drawn() {
    let _keys = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let capture = ClipboardCapture::new();
    for item in [
        GridItem::Image("plane-copy-unread-source.raw".into()),
        GridItem::ZipImage {
            zip_path: "plane-copy-unread-source.zip".into(),
            entry_name: "page.png".into(),
        },
        GridItem::PdfPage {
            pdf_path: "plane-copy-unread-source.pdf".into(),
            page_num: 0,
            content_type: None,
        },
    ] {
        app.items = vec![item];
        app.selected = Some(0);
        app.fullscreen_idx = Some(0);
        let pose = PlaneViewPose {
            anchor: app.plane_view_anchor(0),
            strength: 0.5,
            lens_center: egui::Vec2::splat(0.5),
        };
        app.set_fs_exclusive_view(
            FsExclusiveView::PlaneView(PlaneViewState::new(pose)),
            "plane_copy_fixture",
        );
        for entry in 0..3 {
            app.fs_feedback_toast = None;
            dispatch(&mut app, &ctx, entry);
            assert!(capture.take().is_empty());
            assert!(
                app.fs_feedback_toast
                    .as_ref()
                    .is_some_and(|(text, _, _)| text == "魚眼の表示の準備中です"),
                "copy entry {entry} must reject the missing snapshot for {:?}",
                app.items[0]
            );
        }
    }
}

#[cfg(windows)]
mod gpu_tests {
    use super::*;
    use crate::app::depth_main_window_tests::RootDepthViewer;
    use crate::plane_view_wgpu::{
        PlaneFrameSnapshot, PlaneOutputChoice, resolve_plane_output_size,
    };
    use std::sync::Arc;

    fn ready() -> (crate::app::AppTestEnvForTest, RootDepthViewer) {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = RootDepthViewer::new(&mut app);
        viewer.ready(&mut app);
        app.set_fs_exclusive_view(FsExclusiveView::None, "plane_copy_fixture");
        app.settings.fullscreen_navigator_visible = false;
        app.settings.fullscreen_fit_mode = crate::settings::FullscreenFitMode::Page;
        viewer.frame(&mut app, vec![]);
        viewer.frame(
            &mut app,
            vec![
                egui::Event::PointerMoved(viewer.size.to_pos2() * 0.5),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0., 40.),
                    modifiers: egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                },
            ],
        );
        let render = app.wgpu_render_state.as_ref().unwrap().clone();
        let mut delta = viewer.ctx.tex_manager().write().take_delta();
        for (id, deltas) in std::mem::take(&mut delta.set) {
            for delta in deltas {
                render
                    .renderer
                    .write()
                    .update_texture(&render.device, &render.queue, id, &delta);
            }
        }
        viewer.frame(&mut app, vec![]);
        (app, viewer)
    }

    #[test]
    fn plane_copy_action_menu_and_ring_submit_the_same_frozen_canvas_frame() {
        let _keys = crate::key_input::lock_test_input();
        let (mut app, viewer) = ready();
        let capture = ClipboardCapture::new();
        let FsExclusiveView::PlaneView(state) = &app.fs_exclusive_view else {
            panic!("PlaneView fixture")
        };
        let frozen = state.last_drawn.clone().expect("drawn plane frame");
        let expected_size = resolve_plane_output_size(
            frozen.params.canvas().size(),
            PlaneOutputChoice::Canvas,
            &app.wgpu_render_state.as_ref().unwrap().device.limits(),
        );
        for entry in 0..3 {
            dispatch(&mut app, &viewer.ctx, entry);
            let mut work = capture.take();
            assert_eq!(
                work.len(),
                1,
                "copy entry {entry} must submit one worker request"
            );
            let CapturePixelWork::PlaneView { basename, request } = work.remove(0) else {
                panic!("copy must use PlaneView rather than original-source pixels")
            };
            assert!(basename.ends_with("_fisheye"));
            assert!(Arc::ptr_eq(&request.snapshot, &frozen));
            assert_eq!(request.size, expected_size);
        }
    }

    #[test]
    fn plane_copy_locked_zoom_action_menu_and_ring_keep_the_displayed_distortion() {
        let _keys = crate::key_input::lock_test_input();
        let (mut app, mut viewer) = ready();
        // Revision 3's first inward notch leaves this portrait Page-fit image
        // uncovered (k_fill=0). Continue real inward input before testing a nonzero lock.
        viewer.frame(
            &mut app,
            vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0., 120.),
                modifiers: egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
            }],
        );
        let before = match &app.fs_exclusive_view {
            FsExclusiveView::PlaneView(state) => state.last_drawn.clone().unwrap(),
            _ => panic!("PlaneView fixture"),
        };
        assert!(
            before.k_fill > 0. && before.params.values[2][0] > 0.,
            "the lock fixture must start with actual visible distortion"
        );
        app.settings.fullscreen_wheel_zoom_step =
            crate::settings::FullscreenWheelZoomStep::Percent10;
        viewer.frame(
            &mut app,
            vec![
                egui::Event::PointerMoved(viewer.size.to_pos2() * 0.5),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0., -480.),
                    modifiers: egui::Modifiers::CTRL,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                },
            ],
        );
        let FsExclusiveView::PlaneView(state) = &app.fs_exclusive_view else {
            panic!("PlaneView fixture")
        };
        assert_eq!(
            state.fill_mode,
            crate::plane_view::PlaneFillMode::Locked(before.k_fill)
        );
        let frozen = state.last_drawn.clone().unwrap();
        assert_eq!(frozen.k_fill, before.k_fill);
        assert_eq!(frozen.params.values[2][0], before.params.values[2][0]);
        assert!(frozen.params.values[2][0] > 0.);
        let capture = ClipboardCapture::new();
        for entry in 0..3 {
            dispatch(&mut app, &viewer.ctx, entry);
            let mut work = capture.take();
            assert_eq!(work.len(), 1, "copy entry {entry}");
            let CapturePixelWork::PlaneView { request, .. } = work.remove(0) else {
                panic!("PlaneView copy")
            };
            assert!(Arc::ptr_eq(&request.snapshot, &frozen));
            assert_eq!(request.snapshot.k_fill, before.k_fill);
            assert_eq!(
                request.snapshot.params.for_output(request.size).values[2][0],
                frozen.params.values[2][0]
            );
        }
    }

    #[test]
    fn plane_copy_action_rejects_stale_context_and_page_slice_snapshots() {
        let _keys = crate::key_input::lock_test_input();
        let (mut app, viewer) = ready();
        let capture = ClipboardCapture::new();
        let FsExclusiveView::PlaneView(state) = &app.fs_exclusive_view else {
            panic!("PlaneView fixture")
        };
        let frozen = state.last_drawn.clone().unwrap();
        for entry in 0..3 {
            if let FsExclusiveView::PlaneView(state) = &mut app.fs_exclusive_view {
                state.last_drawn = Some(Arc::new(PlaneFrameSnapshot {
                    context: crate::app::ViewerContextId::for_test(917_999),
                    anchor: frozen.anchor,
                    material: frozen.material.clone(),
                    params: frozen.params,
                    k_fill: frozen.k_fill,
                }));
            }
            app.fs_feedback_toast = None;
            dispatch(&mut app, &viewer.ctx, entry);
            assert!(capture.take().is_empty());
            assert!(
                app.fs_feedback_toast
                    .as_ref()
                    .is_some_and(|(text, _, _)| text == "魚眼の表示の準備中です")
            );
            if let FsExclusiveView::PlaneView(state) = &mut app.fs_exclusive_view {
                state.last_drawn = Some(frozen.clone());
            }
            app.fullscreen_page_slice = crate::page_split::PageSlice::Left;
            app.fs_feedback_toast = None;
            dispatch(&mut app, &viewer.ctx, entry);
            assert!(capture.take().is_empty());
            assert!(
                app.fs_feedback_toast
                    .as_ref()
                    .is_some_and(|(text, _, _)| text == "魚眼の表示の準備中です")
            );
            app.fullscreen_page_slice = crate::page_split::PageSlice::Full;
        }
    }
}
