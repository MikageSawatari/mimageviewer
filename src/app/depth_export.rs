//! One process owner retains the frozen display and Remote lease until dismissal.
use super::*;
use crate::depth_export::{
    DepthExportEvent, DepthExportFormat, DepthExportJob, DepthExportOptions, DepthExportRequest,
    DepthExportUnderlay,
};
use crate::ui_depth_export::{self as ui, Action};

#[cfg(all(test, windows))]
#[path = "depth_export_main_window_tests.rs"]
mod main_window_tests;

#[derive(Clone)]
pub(crate) struct DisplaySnapshot {
    resource_id: u64,
    paint_pass: u64,
    page_idx: usize,
    items_generation: u64,
    uv: egui::Rect,
    quarter_turns: u32,
    paint_size: egui::Vec2,
    underlay: crate::ui_fullscreen::FsBgStyle,
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::app::depth_view_tests::{install_pages, install_ready_depth_for_export};

    fn acquire_request() -> mimageviewer_ipc::SessionAcquireRequest {
        mimageviewer_ipc::SessionAcquireRequest {
            client_id: "depth-export-test".into(),
            peer: mimageviewer_ipc::SessionPeerInfo {
                connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                device_name: None,
            },
        }
    }

    fn remember_display(app: &App, ctx: &egui::Context) {
        let crate::fs_exclusive_view::FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
            unreachable!()
        };
        let paint_pass = ctx.cumulative_pass_nr();
        ctx.data_mut(|data| {
            data.insert_temp(
                snapshot_id(),
                DisplaySnapshot {
                    resource_id: state.ready().unwrap().resource_id,
                    paint_pass,
                    page_idx: app.fullscreen_idx.unwrap(),
                    items_generation: app.items_generation,
                    uv: egui::Rect::from_min_max(egui::pos2(0.1, 0.2), egui::pos2(0.9, 0.8)),
                    quarter_turns: 1,
                    paint_size: egui::vec2(200.0, 300.0),
                    underlay: crate::ui_fullscreen::FsBgStyle::Solid(egui::Color32::WHITE),
                },
            )
        });
    }

    #[test]
    fn depth_export_completion_toast_includes_optional_worker_file_size() {
        for (bytes, expected) in [
            (Some(8_800_000), "保存しました: photo_3d.mp4 (8.4 MB)"),
            (None, "保存しました: photo_3d.mp4"),
        ] {
            let ctx = egui::Context::default();
            let mut app = setup_app_for_test();
            install_pages(&mut app, &ctx);
            install_ready_depth_for_export(&mut app);
            remember_display(&app, &ctx);
            app.open_depth_export(&ctx, 0);
            let (job, events, _) = DepthExportJob::controlled_for_test();
            app.depth_export_operation.as_mut().unwrap().phase = Phase::Running {
                job,
                completed: 1,
                total: 1,
                status: RunStatus::Active,
            };
            events
                .send(DepthExportEvent::Finished(Ok(
                    crate::depth_export::DepthExportSummary {
                        // This path need not exist: the UI uses only worker-supplied data.
                        output: PathBuf::from("photo_3d.mp4"),
                        bytes,
                    },
                )))
                .unwrap();
            app.poll_depth_export();
            let Phase::Finished { message, failed } =
                &app.depth_export_operation.as_ref().unwrap().phase
            else {
                panic!("completion event did not finish the dialog")
            };
            assert!(!failed);
            assert_eq!(message, expected);
            assert_eq!(app.fs_feedback_toast.as_ref().unwrap().0, expected);
        }
    }

    #[test]
    fn depth_export_freezes_display_and_retains_modal_admission_until_dismissal() {
        let ctx = egui::Context::default();
        let mut app = setup_app_for_test();
        install_pages(&mut app, &ctx);
        install_ready_depth_for_export(&mut app);
        let handle = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(handle.clone());
        app.settings.depth_parallax.motion = crate::depth_parallax::MotionKind::Mouse;
        app.settings.depth_parallax.strength = 0.03;
        assert!(!app.depth_export_available(&ctx));
        remember_display(&app, &ctx);
        assert!(app.depth_export_available(&ctx));
        app.open_depth_export(&ctx, 0);
        assert!(app.common_modal_dialog_open());
        assert!(app.document_open_modal_admission_blocked());
        assert_eq!(
            handle.acquire(acquire_request()).status,
            mimageviewer_ipc::SessionStatus::LocalInUse
        );
        let op = app.depth_export_operation.as_mut().unwrap();
        assert_eq!(op.settings.strength, 0.03);
        assert_eq!(op.display.quarter_turns, 1);
        assert_eq!(op.display.uv.min, egui::pos2(0.1, 0.2));
        let Phase::Settings(options) = &op.phase else {
            unreachable!()
        };
        assert_eq!(
            options.motion,
            crate::depth_parallax::MotionKind::Horizontal
        );
        assert_eq!(options.loops, 4);
        assert_eq!(options.long_edge, 2);
        op.phase = Phase::Finished {
            message: "完了".into(),
            failed: false,
        };
        assert!(app.common_modal_dialog_open());
        assert!(app.document_open_modal_admission_blocked());
        assert_eq!(
            handle.acquire(acquire_request()).status,
            mimageviewer_ipc::SessionStatus::LocalInUse
        );
        app.depth_export_operation.take();
        assert!(!app.common_modal_dialog_open());
        assert!(!app.document_open_modal_admission_blocked());
        assert_eq!(
            handle.acquire(acquire_request()).status,
            mimageviewer_ipc::SessionStatus::Active
        );
    }

    #[test]
    fn depth_export_rejects_remote_owner_and_stale_display_capture() {
        let ctx = egui::Context::default();
        let mut app = setup_app_for_test();
        install_pages(&mut app, &ctx);
        install_ready_depth_for_export(&mut app);
        remember_display(&app, &ctx);
        ctx.data_mut(|data| {
            let mut display = data.get_temp::<DisplaySnapshot>(snapshot_id()).unwrap();
            display.resource_id += 1;
            data.insert_temp(snapshot_id(), display);
        });
        assert!(!app.depth_export_available(&ctx));
        app.open_depth_export(&ctx, 0);
        assert!(app.depth_export_operation.is_none());
        remember_display(&app, &ctx);
        let handle = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(handle.clone());
        assert_eq!(
            handle.acquire(acquire_request()).status,
            mimageviewer_ipc::SessionStatus::Active
        );
        app.open_depth_export(&ctx, 0);
        assert!(app.depth_export_operation.is_none());
    }

    #[test]
    fn depth_export_rejects_previous_page_before_source_advance() {
        let ctx = egui::Context::default();
        let mut app = setup_app_for_test();
        install_pages(&mut app, &ctx);
        install_ready_depth_for_export(&mut app);
        remember_display(&app, &ctx);
        assert!(app.depth_export_available(&ctx));
        // The panel is drawn before advance_depth_source in this navigation frame.
        app.fullscreen_idx = Some(2);
        assert!(!app.depth_export_available(&ctx));
        app.open_depth_export(&ctx, 2);
        assert!(app.depth_export_operation.is_none());
        app.fullscreen_idx = Some(0);
        remember_display(&app, &ctx);
        assert!(app.depth_export_available(&ctx));
        app.open_depth_export(&ctx, 0);
        assert!(app.depth_export_operation.is_some());
        app.depth_export_operation.take();
        app.items_generation += 1;
        assert!(!app.depth_export_available(&ctx));
        app.open_depth_export(&ctx, 0);
        assert!(app.depth_export_operation.is_none());
        remember_display(&app, &ctx);
        assert!(app.depth_export_available(&ctx));
    }

    #[test]
    fn depth_export_close_cancels_only_own_worker_and_keeps_lease_until_terminal_dismissal() {
        let ctx = egui::Context::default();
        let mut app = setup_app_for_test();
        install_pages(&mut app, &ctx);
        install_ready_depth_for_export(&mut app);
        remember_display(&app, &ctx);
        let handle = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(handle.clone());
        app.open_depth_export(&ctx, 0);
        let (job, events, cancel) = DepthExportJob::controlled_for_test();
        app.depth_export_operation.as_mut().unwrap().phase = Phase::Running {
            job,
            completed: 1,
            total: 4,
            status: RunStatus::Active,
        };
        let sibling = app.build_window_context_for_test(91_101, |owner| {
            owner.fullscreen_idx = Some(0);
        });
        app.with_viewer_context(sibling, |owner| {
            owner.close_fullscreen_content();
        })
        .unwrap();
        assert!(!cancel.load(std::sync::atomic::Ordering::Relaxed));
        assert!(matches!(
            app.depth_export_operation.as_ref().unwrap().phase,
            Phase::Running {
                status: RunStatus::Active,
                ..
            }
        ));
        app.close_fullscreen_content();
        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
        assert!(matches!(
            app.depth_export_operation.as_ref().unwrap().phase,
            Phase::Running {
                status: RunStatus::Stopping,
                ..
            }
        ));
        assert!(app.common_modal_dialog_open());
        assert_eq!(
            handle.acquire(acquire_request()).status,
            mimageviewer_ipc::SessionStatus::LocalInUse
        );
        events.send(DepthExportEvent::Cancelled).unwrap();
        app.poll_depth_export();
        assert!(matches!(
            app.depth_export_operation.as_ref().unwrap().phase,
            Phase::Finished { .. }
        ));
        assert!(app.document_open_modal_admission_blocked());
        assert_eq!(
            handle.acquire(acquire_request()).status,
            mimageviewer_ipc::SessionStatus::LocalInUse
        );
        app.depth_export_operation.take();
        assert_eq!(
            handle.acquire(acquire_request()).status,
            mimageviewer_ipc::SessionStatus::Active
        );
    }

    #[test]
    fn depth_export_settings_and_terminal_close_release_only_own_operation() {
        for terminal in [false, true] {
            let ctx = egui::Context::default();
            let mut app = setup_app_for_test();
            install_pages(&mut app, &ctx);
            install_ready_depth_for_export(&mut app);
            remember_display(&app, &ctx);
            app.open_depth_export(&ctx, 0);
            if terminal {
                app.depth_export_operation.as_mut().unwrap().phase = Phase::Finished {
                    message: "完了".into(),
                    failed: false,
                };
            }
            let sibling =
                app.build_window_context_for_test(91_102, |owner| owner.fullscreen_idx = Some(0));
            app.with_viewer_context(sibling, |owner| owner.close_fullscreen_content())
                .unwrap();
            assert!(app.depth_export_dialog_open());
            app.close_fullscreen_content();
            assert!(!app.depth_export_dialog_open());
        }
    }

    fn paint_snapshot(
        app: &App,
        ctx: &egui::Context,
        rotation: crate::rotation_db::Rotation,
        viewport: egui::Vec2,
    ) {
        use crate::displayed_image_transform::{
            DisplayedImageTransform, DisplayedImageTransformInput, FullscreenFitScaleLimits,
            RectPixelFit, ResolvedDisplayPlacement,
        };
        let crate::fs_exclusive_view::FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
            unreachable!()
        };
        let transform = DisplayedImageTransform::resolve(DisplayedImageTransformInput {
            page_idx: app.fullscreen_idx.unwrap(),
            viewport_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, viewport),
            source_size: egui::vec2(1200.0, 800.0),
            texture_size: egui::vec2(1200.0, 800.0),
            rotation,
            free_rotation_rad: 0.0,
            content_bbox: None,
            fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
                crate::settings::FullscreenFitMode::Page,
            ),
            fit_scale_limits: FullscreenFitScaleLimits::default(),
            pixels_per_point: 1.0,
            placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
            pixel_fit: RectPixelFit::Texels,
        })
        .unwrap();
        record_display(
            ctx,
            state.ready().unwrap().resource_id,
            app.items_generation,
            &transform,
            &app.fs_checker_texture.as_ref().map_or(
                crate::ui_fullscreen::FsBgStyle::Solid(egui::Color32::WHITE),
                |texture| crate::ui_fullscreen::FsBgStyle::Checker(texture.clone()),
            ),
        );
    }

    #[test]
    fn depth_export_current_paint_capture_tracks_rotation_resize_and_new_resource_until_request_freeze()
     {
        let ctx = egui::Context::default();
        let mut app = setup_app_for_test();
        install_pages(&mut app, &ctx);
        install_ready_depth_for_export(&mut app);
        ctx.begin_pass(Default::default());
        app.ensure_checker_texture(&ctx);
        paint_snapshot(
            &app,
            &ctx,
            crate::rotation_db::Rotation::None,
            egui::vec2(900.0, 700.0),
        );
        let old_size = app
            .capture_depth_export(&ctx, true)
            .unwrap()
            .display
            .paint_size;
        ctx.end_pass().drop_without_applying_deltas();
        ctx.begin_pass(Default::default());
        assert!(
            app.capture_depth_export(&ctx, true).is_none(),
            "an old paint pass cannot admit an intent"
        );
        app.open_depth_export(&ctx, 0);
        assert!(!app.depth_export_dialog_open());
        paint_snapshot(
            &app,
            &ctx,
            crate::rotation_db::Rotation::Cw90,
            egui::vec2(540.0, 300.0),
        );
        app.open_depth_export(&ctx, 0);
        let initial_id = app
            .depth_export_operation
            .as_ref()
            .unwrap()
            .resource
            .resource_id;
        let capture = app.capture_depth_export(&ctx, true).unwrap();
        assert_eq!(capture.display.quarter_turns, 1);
        assert_ne!(capture.display.paint_size, old_size);
        // An AI refinement can replace the Ready resource while the settings modal stays open.
        install_ready_depth_for_export(&mut app);
        assert!(app.capture_depth_export(&ctx, true).is_none());
        paint_snapshot(
            &app,
            &ctx,
            crate::rotation_db::Rotation::Cw90,
            egui::vec2(700.0, 400.0),
        );
        let capture = app.capture_depth_export(&ctx, true).unwrap();
        assert_ne!(capture.resource.resource_id, initial_id);
        let op = app.depth_export_operation.as_ref().unwrap();
        assert!(capture.belongs_to(op));
        let requested_size = capture.display.paint_size;
        let resource_id = capture.resource.resource_id;
        let request = capture.into_request(
            op.settings.clone(),
            DepthExportOptions::default(),
            app.tmp.path().join("not-written.mp4"),
        );
        let DepthExportUnderlay::Checker { cell_size } = request.underlay else {
            panic!("shown checker must be frozen")
        };
        let size =
            request
                .options
                .dimensions(request.source_size, request.uv, request.quarter_turns);
        assert_eq!(
            cell_size,
            [
                8.0 * size[0] as f32 / requested_size.x,
                8.0 * size[1] as f32 / requested_size.y
            ]
        );
        paint_snapshot(
            &app,
            &ctx,
            crate::rotation_db::Rotation::None,
            egui::vec2(1200.0, 800.0),
        );
        assert_eq!(request.resource.resource_id, resource_id);
        assert_eq!(request.quarter_turns, 1);
        assert_ne!(
            app.capture_depth_export(&ctx, true)
                .unwrap()
                .display
                .paint_size,
            requested_size
        );
        // A navigation committed after paint must reject the frame-local intent.
        let capture = app.capture_depth_export(&ctx, true).unwrap();
        app.depth_export_operation.take();
        app.fullscreen_idx = Some(2);
        app.admit_depth_export(&ctx, capture);
        assert!(!app.depth_export_dialog_open());
        ctx.end_pass().drop_without_applying_deltas();
    }

    #[test]
    fn depth_export_modal_blocks_manual_and_slideshow_navigation_in_every_phase_then_resumes() {
        for phase in 0..3 {
            let ctx = egui::Context::default();
            let mut app = setup_app_for_test();
            install_pages(&mut app, &ctx);
            install_ready_depth_for_export(&mut app);
            remember_display(&app, &ctx);
            app.open_depth_export(&ctx, 0);
            if phase == 1 {
                let (job, _events, _cancel) = DepthExportJob::controlled_for_test();
                app.depth_export_operation.as_mut().unwrap().phase = Phase::Running {
                    job,
                    completed: 0,
                    total: 4,
                    status: RunStatus::Active,
                };
            } else if phase == 2 {
                app.depth_export_operation.as_mut().unwrap().phase = Phase::Finished {
                    message: "完了".into(),
                    failed: false,
                };
            }
            app.settings.page_turn_animation_enabled = false;
            app.slideshow_playing = true;
            app.slideshow_anchor_idx = Some(0);
            app.slideshow_next_at = std::time::Instant::now();
            let deadline = app.slideshow_next_at;
            for navigation in [
                crate::ui_fullscreen::FsPageNav::Target(2),
                crate::ui_fullscreen::FsPageNav::None,
            ] {
                app.handle_fs_navigation(&ctx, false, false, None, None, None, navigation, None, 0);
                assert_eq!(app.fullscreen_idx, Some(0));
                assert_eq!(app.slideshow_next_at, deadline);
            }
            app.depth_export_operation.take();
            app.slideshow_playing = false;
            app.handle_fs_navigation(
                &ctx,
                false,
                false,
                None,
                None,
                None,
                crate::ui_fullscreen::FsPageNav::Target(2),
                None,
                0,
            );
            assert_eq!(
                app.fullscreen_idx,
                Some(2),
                "ordinary navigation resumes after dismissal"
            );
        }
    }
}

fn snapshot_id() -> egui::Id {
    egui::Id::unique("depth_export_display_snapshot")
}

pub(crate) fn record_display(
    ctx: &egui::Context,
    resource_id: u64,
    items_generation: u64,
    transform: &crate::displayed_image_transform::DisplayedImageTransform,
    underlay: &crate::ui_fullscreen::FsBgStyle,
) {
    let paint_pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|data| {
        data.insert_temp(
            snapshot_id(),
            DisplaySnapshot {
                resource_id,
                paint_pass,
                page_idx: transform.page_idx,
                items_generation,
                uv: transform.uv_rect,
                quarter_turns: transform.rotation.degrees() as u32 / 90,
                paint_size: transform.paint_rect.size(),
                underlay: underlay.clone(),
            },
        )
    });
}

enum Phase {
    Settings(DepthExportOptions),
    Running {
        job: DepthExportJob,
        completed: u32,
        total: u32,
        status: RunStatus,
    },
    Finished {
        message: String,
        failed: bool,
    },
}
enum RunStatus {
    Active,
    Stopping,
}

pub(crate) struct ExportCapture {
    resource: Arc<crate::depth_parallax_wgpu::DepthRenderResources>,
    display: DisplaySnapshot,
}
impl ExportCapture {
    fn belongs_to(&self, op: &Operation) -> bool {
        self.resource.context == op.resource.context
            && self.display.page_idx == op.display.page_idx
            && self.display.items_generation == op.display.items_generation
    }

    fn into_request(
        self,
        settings: crate::depth_parallax::DepthParallaxSettings,
        options: DepthExportOptions,
        output: PathBuf,
    ) -> DepthExportRequest {
        let source_size = [self.resource.color.width, self.resource.color.height];
        let dimensions =
            options.dimensions(source_size, self.display.uv, self.display.quarter_turns);
        let underlay = match &self.display.underlay {
            crate::ui_fullscreen::FsBgStyle::Solid(color) => {
                DepthExportUnderlay::Solid([color.r(), color.g(), color.b()])
            }
            crate::ui_fullscreen::FsBgStyle::Checker(_) => DepthExportUnderlay::Checker {
                cell_size: [
                    8.0 * dimensions[0] as f32 / self.display.paint_size.x.max(1.0),
                    8.0 * dimensions[1] as f32 / self.display.paint_size.y.max(1.0),
                ],
            },
        };
        DepthExportRequest {
            resource: self.resource,
            settings,
            options,
            output,
            source_size,
            uv: self.display.uv,
            quarter_turns: self.display.quarter_turns,
            underlay,
        }
    }
}

pub(crate) struct Operation {
    resource: Arc<crate::depth_parallax_wgpu::DepthRenderResources>,
    display: DisplaySnapshot,
    settings: crate::depth_parallax::DepthParallaxSettings,
    name: String,
    _lease: Option<crate::remote_ipc::session::LocalModalLease>,
    phase: Phase,
}

impl App {
    /// The process owner will join after run_native; no waiting in this callback.
    pub(crate) fn retire_depth_export_for_exit(&self) {
        if let Some(workers) = &self.depth_export_workers {
            workers.retire();
        }
    }
    /// A read-only sibling close must never retire the main viewer's export.
    pub(crate) fn cancel_depth_export_for_fullscreen_close(&mut self) {
        let context = self.edit_request_owner_context();
        let Some(op) = self
            .depth_export_operation
            .as_mut()
            .filter(|op| op.resource.context == context)
        else {
            return;
        };
        match &mut op.phase {
            Phase::Running { job, status, .. } => {
                job.cancel();
                *status = RunStatus::Stopping;
            }
            Phase::Settings(_) | Phase::Finished { .. } => {
                self.depth_export_operation = None;
            }
        }
    }

    pub(crate) fn depth_export_dialog_open(&self) -> bool {
        self.depth_export_operation.is_some()
    }

    pub(crate) fn depth_export_available(&self, ctx: &egui::Context) -> bool {
        self.capture_depth_export(ctx, false).is_some()
    }

    pub(crate) fn capture_depth_export(
        &self,
        ctx: &egui::Context,
        same_pass: bool,
    ) -> Option<ExportCapture> {
        let crate::fs_exclusive_view::FsExclusiveView::Depth(state) = &self.fs_exclusive_view
        else {
            return None;
        };
        let resource = state.ready()?;
        let display = self
            .current_depth_export_display(ctx, resource.resource_id)
            .filter(|display| !same_pass || display.paint_pass == ctx.cumulative_pass_nr())?;
        Some(ExportCapture { resource, display })
    }

    fn current_depth_export_display(
        &self,
        ctx: &egui::Context,
        resource_id: u64,
    ) -> Option<DisplaySnapshot> {
        ctx.data(|data| data.get_temp::<DisplaySnapshot>(snapshot_id()))
            .filter(|display| {
                display.resource_id == resource_id
                    && self.fullscreen_idx == Some(display.page_idx)
                    && self.items_generation == display.items_generation
            })
    }

    pub(crate) fn open_depth_export(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        let Some(capture) = self.capture_depth_export(ctx, true) else {
            return;
        };
        self.admit_depth_export(ctx, capture);
    }

    pub(crate) fn admit_depth_export(&mut self, ctx: &egui::Context, capture: ExportCapture) {
        if self.common_modal_dialog_open() || self.document_open_modal_admission_blocked() {
            return;
        }
        // Navigation may commit after the viewport has painted.
        let Some(current) = self.capture_depth_export(ctx, false) else {
            return;
        };
        if current.resource.resource_id != capture.resource.resource_id
            || current.resource.context != capture.resource.context
            || current.display.page_idx != capture.display.page_idx
            || current.display.items_generation != capture.display.items_generation
        {
            return;
        }
        let ExportCapture { resource, display } = capture;
        let fs_idx = display.page_idx;
        let lease = if let Some(handle) = self.remote_session_handle() {
            let Some(lease) = handle.try_local_modal_lease_with_notice(
                "PC で3D写真を書き出しています。完了してから接続してください。",
            ) else {
                self.show_feedback_toast("リモート閲覧中のため、3D 写真を書き出せません".into());
                return;
            };
            Some(lease)
        } else {
            None
        };
        let settings = self.settings.depth_parallax.clone();
        let long_edge = ui::default_long_edge(
            DepthExportFormat::Mp4,
            resource.color.width.max(resource.color.height),
        );
        let motion = if settings.motion == crate::depth_parallax::MotionKind::Mouse {
            crate::depth_parallax::MotionKind::Horizontal
        } else {
            settings.motion
        };
        self.depth_export_operation = Some(Box::new(Operation {
            resource,
            display,
            settings,
            name: self
                .items
                .get(fs_idx)
                .map_or_else(|| "image".into(), |item| item.name().into_owned()),
            _lease: lease,
            phase: Phase::Settings(DepthExportOptions {
                format: DepthExportFormat::Mp4,
                long_edge,
                motion,
                loops: 4,
            }),
        }));
        ctx.request_repaint();
    }

    pub(crate) fn poll_depth_export(&mut self) {
        let Some(op) = self.depth_export_operation.as_mut() else {
            return;
        };
        let Phase::Running {
            job,
            completed,
            total,
            ..
        } = &mut op.phase
        else {
            return;
        };
        let mut terminal = None;
        loop {
            match job.try_recv() {
                Ok(DepthExportEvent::Progress {
                    completed: done,
                    total: all,
                }) => {
                    *completed = done;
                    *total = all;
                }
                Ok(DepthExportEvent::Finished(result)) => {
                    terminal = Some(match result {
                        Ok(summary) => {
                            let size = summary.bytes.map_or_else(String::new, |bytes| {
                                format!(" ({})", crate::ui_helpers::format_bytes(bytes))
                            });
                            (
                                format!("保存しました: {}{size}", summary.output.display()),
                                false,
                            )
                        }
                        Err(error) => (format!("3D 写真を書き出せませんでした: {error}"), true),
                    });
                    break;
                }
                Ok(DepthExportEvent::Cancelled) => {
                    terminal = Some(("書き出しをキャンセルしました".into(), false));
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    terminal = Some((
                        "3D 写真の書き出し処理が終了しました。詳しくはログを確認してください。"
                            .into(),
                        true,
                    ));
                    break;
                }
            }
        }
        if let Some((message, failed)) = terminal {
            op.phase = Phase::Finished {
                message: message.clone(),
                failed,
            };
            if failed {
                crate::logger::log(format!("[DepthExport] {message}"));
                self.show_feedback_error(message);
            } else {
                self.show_feedback_toast(message);
            }
        }
    }

    pub(crate) fn draw_depth_export_dialog(&mut self, ctx: &egui::Context) {
        let escape = self.dialog_escape_pressed(ctx);
        let capture = self.capture_depth_export(ctx, true);
        let Some(op) = self.depth_export_operation.as_mut() else {
            return;
        };
        let capture = capture.filter(|capture| capture.belongs_to(op));
        let save_enabled = capture.is_some();
        if matches!(op.phase, Phase::Settings(_)) {
            if let Some(capture) = capture {
                op.resource = capture.resource;
                op.display = capture.display;
                let choices =
                    ui::long_edge_choices(op.resource.color.width.max(op.resource.color.height));
                if let Phase::Settings(options) = &mut op.phase {
                    if !choices.contains(&options.long_edge) {
                        options.long_edge = choices
                            .into_iter()
                            .filter(|&edge| edge <= options.long_edge)
                            .next_back()
                            .unwrap_or(
                                op.resource
                                    .color
                                    .width
                                    .max(op.resource.color.height)
                                    .min(720),
                            );
                    }
                }
            }
        }
        let phase = match &mut op.phase {
            Phase::Settings(options) => ui::Phase::Settings {
                options,
                source_long_edge: op.resource.color.width.max(op.resource.color.height),
                period_secs: op.settings.period_secs,
                save_enabled,
            },
            Phase::Running {
                completed,
                total,
                status,
                ..
            } => ui::Phase::Progress {
                completed: *completed,
                total: *total,
                stopping: matches!(status, RunStatus::Stopping),
            },
            Phase::Finished { message, failed } => ui::Phase::Finished {
                message,
                failed: *failed,
            },
        };
        match ui::draw(ctx, phase, escape).action {
            Some(Action::Close) => {
                self.depth_export_operation = None;
            }
            Some(Action::Cancel) => match &mut op.phase {
                Phase::Running { job, status, .. } => {
                    job.cancel();
                    *status = RunStatus::Stopping;
                }
                _ => {
                    self.depth_export_operation = None;
                }
            },
            Some(Action::Save) => self.save_depth_export(ctx),
            None => {}
        }
    }

    fn save_depth_export(&mut self, ctx: &egui::Context) {
        let capture = self.capture_depth_export(ctx, true);
        let Some(op) = self.depth_export_operation.as_ref() else {
            return;
        };
        let Some(capture) = capture.filter(|capture| capture.belongs_to(op)) else {
            return;
        };
        let Phase::Settings(options) = &op.phase else {
            return;
        };
        let extension = options.format.extension();
        let stem = std::path::Path::new(&op.name)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        // The OS save dialog is deliberately modal, matching the existing core export path.
        let Some(output) = rfd::FileDialog::new()
            .set_file_name(format!("{stem}_3d.{extension}"))
            .add_filter(extension.to_ascii_uppercase(), &[extension])
            .save_file()
        else {
            return;
        };
        let request = capture.into_request(op.settings.clone(), options.clone(), output);
        let total = request.options.total_frames(request.settings.period_secs);
        #[cfg(windows)]
        let result = match (&self.wgpu_render_state, &self.depth_export_workers) {
            (Some(render), Some(workers)) => workers.start(
                request,
                render.device.clone(),
                render.queue.clone(),
                ctx.clone(),
                // Root owns polling even after the source fullscreen closes.
                egui::ViewportId::ROOT,
            ),
            _ => Err("GPU または書き出し worker を利用できません".into()),
        };
        #[cfg(not(windows))]
        let result: Result<DepthExportJob, String> = {
            let _ = request;
            Err("GPU を利用できません".into())
        };
        if let Some(op) = self.depth_export_operation.as_mut() {
            op.phase = match result {
                Ok(job) => Phase::Running {
                    job,
                    completed: 0,
                    total,
                    status: RunStatus::Active,
                },
                Err(error) => {
                    crate::logger::log(format!("[DepthExport] {error}"));
                    Phase::Finished {
                        message: format!("3D 写真を書き出せませんでした: {error}"),
                        failed: true,
                    }
                }
            };
        }
    }
}
