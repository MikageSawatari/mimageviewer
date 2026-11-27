//! PlaneView adapters at existing viewer, material and output ownership boundaries.
use super::*;
use crate::fs_exclusive_view::FsExclusiveView;
use crate::plane_view::*;
use crate::plane_view_wgpu::*;

impl App {
    /// Capture the picture the user saw, before paint clears last_drawn.
    pub(crate) fn plane_fill_lock_candidate(
        &self,
    ) -> Option<crate::ui_fullscreen::PlaneFillLockCandidate> {
        let FsExclusiveView::PlaneView(state) = &self.fs_exclusive_view else {
            return None;
        };
        let idx = self.fullscreen_idx?;
        let anchor = self.plane_view_anchor(idx);
        let context = self.edit_request_owner_context();
        let frame = state.last_drawn.as_ref()?;
        (state.pose.anchor == anchor && frame.anchor == anchor && frame.context == context)
            .then_some(crate::ui_fullscreen::PlaneFillLockCandidate {
                context,
                anchor,
                k_fill: frame.k_fill,
            })
    }

    pub(crate) fn plane_fill_lock_candidate_matches(
        &self,
        candidate: crate::ui_fullscreen::PlaneFillLockCandidate,
    ) -> bool {
        self.edit_request_owner_context() == candidate.context
            && self
                .fullscreen_idx
                .is_some_and(|idx| self.plane_view_anchor(idx) == candidate.anchor)
            && matches!(&self.fs_exclusive_view, FsExclusiveView::PlaneView(state) if state.pose.anchor == candidate.anchor)
    }

    /// Every ordinary zoom writer freezes the displayed fill only when magnification changed.
    pub(crate) fn lock_plane_fill_for_zoom(
        &mut self,
        old_zoom: f32,
        new_zoom: f32,
        candidate: Option<crate::ui_fullscreen::PlaneFillLockCandidate>,
    ) {
        if old_zoom == new_zoom {
            return;
        }
        let Some(candidate) =
            candidate.filter(|candidate| self.plane_fill_lock_candidate_matches(*candidate))
        else {
            return;
        };
        if let FsExclusiveView::PlaneView(state) = &mut self.fs_exclusive_view
            && state.fill_mode == PlaneFillMode::Auto
        {
            state.fill_mode = PlaneFillMode::Locked(candidate.k_fill);
        }
    }

    pub(crate) fn handle_plane_lens_drag(
        &mut self,
        ctx: &egui::Context,
        canvas: egui::Rect,
        can_start: bool,
    ) -> bool {
        let Some(pose) = self.plane_view_pose() else {
            self.fs_canvas_drag.retire_lens();
            return false;
        };
        if pose.anchor != self.plane_view_anchor(pose.anchor.fs_idx) {
            self.fs_canvas_drag.retire_lens();
            return false;
        }
        let press_origin =
            ctx.input(|i| crate::ui_helpers::button_press_origin(i, egui::PointerButton::Primary));
        let (pressed, down, pos, _mods) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.interact_pos(),
                i.modifiers,
            )
        });
        #[cfg(windows)]
        let (shift, ctrl) =
            crate::keyboard_input::focused_key_state_permit(ctx).map_or((false, false), |permit| {
                (
                    crate::keymap::modifier_held_via_os(permit, crate::keymap::ModKind::Shift),
                    crate::keymap::modifier_held_via_os(permit, crate::keymap::ModKind::Ctrl),
                )
            });
        #[cfg(not(windows))]
        let (shift, ctrl) = (_mods.shift, _mods.ctrl);
        if pressed
            && can_start
            && shift
            && !ctrl
            && let Some(start) = press_origin.filter(|p| canvas.contains(*p))
        {
            self.fs_canvas_drag = FsCanvasDrag::PlaneLens {
                anchor: pose.anchor,
                start,
                center: pose.lens_center,
            };
        }
        let owned = self.fs_canvas_drag.is_lens();
        if owned {
            if let Some(center) =
                pos.and_then(|p| self.fs_canvas_drag.lens_center(pose.anchor, p, canvas))
                && let FsExclusiveView::PlaneView(state) = &mut self.fs_exclusive_view
            {
                state.pose.lens_center = center;
            }
            let frame_key =
                egui::Id::unique(("plane_lens_used", self.edit_request_owner_context()));
            let frame_nr = ctx.cumulative_frame_nr();
            ctx.data_mut(|data| data.insert_temp(frame_key, frame_nr));
            if !down {
                self.fs_canvas_drag.retire_lens();
            }
        }
        owned
    }
    pub(crate) fn plane_view_anchor(&self, idx: usize) -> PlaneViewAnchor {
        PlaneViewAnchor {
            fs_idx: idx,
            item: self.item_id(idx),
            items_generation: self.items_generation,
            slice: self.fullscreen_page_slice,
        }
    }
    pub(crate) fn plane_view_pose(&self) -> Option<PlaneViewPose> {
        match &self.fs_exclusive_view {
            FsExclusiveView::PlaneView(state) => Some(state.pose),
            _ => None,
        }
    }
    pub(crate) fn plane_view_entry_allowed(
        &mut self,
        idx: usize,
    ) -> Result<(), PlaneViewEntryBlock> {
        use PlaneViewEntryBlock::*;
        if self.fullscreen_idx != Some(idx)
            || !self.items.get(idx).is_some_and(GridItem::has_page_data)
            || matches!(
                self.items.get(idx),
                Some(GridItem::Video(_) | GridItem::Audio(_))
            )
        {
            return Err(NotImage);
        }
        if self.fs_entry_is_animated(idx) {
            return Err(Animation);
        }
        if matches!(
            self.resolve_visible_spread_pair(idx),
            crate::ui_fullscreen::SpreadPair::Double { .. }
        ) {
            return Err(Spread);
        }
        if !self.reading_flow.is_paged() {
            return Err(Continuous);
        }
        #[cfg(windows)]
        if self.wgpu_render_state.is_none() {
            return Err(NoGpu);
        }
        #[cfg(not(windows))]
        {
            return Err(NoGpu);
        }
        #[allow(unreachable_code)]
        {
            if self.is_overlay_edit_mode_active() {
                return Err(Editing);
            }
            if self.view_trim_mode {
                return Err(Trim);
            }
            if self.adjustment_mode.is_open() {
                return Err(Adjustment);
            }
            if self.analysis_mode {
                return Err(Analysis);
            }
            if !matches!(self.compare_view_mode, CompareViewMode::Off) {
                return Err(Compare);
            }
            if self.fs_zoom_mode_engaged() {
                return Err(ZoomMode);
            }
            if self.page_turn_barrier_active() || self.page_turn_completion_active() {
                return Err(PageTurn);
            }
            if self.fs_exclusive_view.is_depth() || self.is_panorama_mode_active(idx) {
                return Err(OtherProjection);
            }
            Ok(())
        }
    }
    pub(crate) fn plane_view_may_continue(&mut self, idx: usize) -> bool {
        let Some(pose) = self.plane_view_pose() else {
            return false;
        };
        if self.fullscreen_idx != Some(idx) || pose.anchor != self.plane_view_anchor(idx) {
            self.set_fs_exclusive_view(FsExclusiveView::None, "plane_page_changed");
            return false;
        }
        if let Err(reason) = self.plane_view_entry_allowed(idx) {
            self.set_fs_exclusive_view(FsExclusiveView::None, "plane_continuation_rejected");
            self.show_feedback_toast(format!("魚眼を終了しました: {}", reason.message()));
            return false;
        }
        true
    }
    pub(crate) fn plane_view_wheel_input(
        &mut self,
        canvas: egui::Rect,
        pointer: Option<egui::Pos2>,
        notches: f32,
        zoom: f32,
        pan: egui::Vec2,
        current: Option<PlaneViewPose>,
        cover: Option<PlaneCoverInput>,
    ) -> Option<PlaneViewWheelInput> {
        let idx = self.fullscreen_idx?;
        Some(PlaneViewWheelInput {
            current,
            anchor: self.plane_view_anchor(idx),
            zoom,
            pan,
            pointer,
            canvas,
            notches,
            allowed: self.plane_view_entry_allowed(idx),
            step: self.settings.fullscreen_wheel_zoom_step,
            cover,
        })
    }
    /// Owner changes retire live projections together; panorama's intent stays on its old path.
    pub(crate) fn retire_exclusive_view_for_owner_change(&mut self, cause: &'static str) -> bool {
        let depth = self.fs_exclusive_view.is_depth();
        if self.fs_exclusive_view.retires_for_owner_change() {
            self.set_fs_exclusive_view(FsExclusiveView::None, cause);
        }
        depth
    }
    pub(crate) fn clear_plane_last_drawn(&mut self) {
        if let FsExclusiveView::PlaneView(state) = &mut self.fs_exclusive_view {
            state.last_drawn = None;
        }
    }
    pub(crate) fn plane_material_description(
        resource: &crate::gpu_lanczos::FullscreenPaintPayload,
        source_size: egui::Vec2,
        texture_size: egui::Vec2,
    ) -> Option<PlaneMaterialDescription> {
        Some(PlaneMaterialDescription {
            source_size: [source_size.x as usize, source_size.y as usize],
            texture_size,
            coverage: resource.source_coverage().source_uv_rect()?,
            sampler_roi: resource
                .visible_source_uv_rect()
                .unwrap_or(egui::Rect::from_min_max(
                    egui::Pos2::ZERO,
                    egui::pos2(1., 1.),
                )),
            display_basis: resource.display_basis(),
            material_kind: resource.page_material_kind(),
        })
    }
    pub(crate) fn select_plane_material(
        &mut self,
        ctx: &egui::Context,
        resource: &crate::gpu_lanczos::FullscreenPaintPayload,
        source_size: egui::Vec2,
        texture_size: egui::Vec2,
    ) -> Option<Arc<PlaneMaterial>> {
        let FsExclusiveView::PlaneView(state) = &mut self.fs_exclusive_view else {
            return None;
        };
        state.last_drawn = None;
        let texture = resource.source_texture()?;
        let description = Self::plane_material_description(resource, source_size, texture_size)?;
        let previous = match &state.color {
            PlaneColorPhase::Ready(material) => Some(material.clone()),
            PlaneColorPhase::Pending { previous } => previous.clone(),
            PlaneColorPhase::Empty => None,
        };
        if let Some(previous) = &previous
            && previous.copy.texture_id == texture.id()
        {
            let material = if previous.description == description {
                previous.clone()
            } else {
                PlaneMaterial::with_description(previous.copy.clone(), description)
            };
            state.color = PlaneColorPhase::Ready(material.clone());
            return Some(material);
        }
        #[cfg(windows)]
        if let Some(render) = self.wgpu_render_state.clone() {
            let mut renderer = render.renderer.write();
            let source = renderer
                .texture(&texture.id())
                .and_then(|entry| entry.texture.clone());
            if let Some(source) = source {
                if renderer
                    .callback_resources
                    .get::<PlaneRendererCache>()
                    .is_none()
                {
                    renderer
                        .callback_resources
                        .insert(PlaneRendererCache::default());
                }
                let cache = renderer
                    .callback_resources
                    .get_mut::<PlaneRendererCache>()
                    .expect("installed plane pipelines");
                match cache.copy(
                    &render.device,
                    &render.queue,
                    texture.id(),
                    &source,
                    render
                        .adapter
                        .get_downlevel_capabilities()
                        .flags
                        .contains(wgpu::DownlevelFlags::ANISOTROPIC_FILTERING),
                ) {
                    Ok(copy) => {
                        let material = PlaneMaterial::with_description(copy, description);
                        state.color = PlaneColorPhase::Ready(material.clone());
                        return Some(material);
                    }
                    Err(error) => {
                        drop(renderer);
                        crate::logger::log(format!("[PlaneView] {error}"));
                        self.set_fs_exclusive_view(FsExclusiveView::None, "plane_copy_failed");
                        self.show_feedback_error(format!("魚眼を終了しました: {error}"));
                        return None;
                    }
                }
            }
            state.color = PlaneColorPhase::Pending {
                previous: previous.clone(),
            };
            // Managed texture registration occurs after UI; request its owning next pass.
            ctx.request_repaint_of(ctx.viewport_id());
        }
        previous
    }
    pub(crate) fn paint_plane_view(
        &mut self,
        painter: &egui::Painter,
        material: Arc<PlaneMaterial>,
        transform: &crate::displayed_image_transform::DisplayedImageTransform,
        canvas: egui::Rect,
        underlay: &crate::ui_fullscreen::FsBgStyle,
    ) -> bool {
        #[cfg(windows)]
        {
            let Some(render) = self.wgpu_render_state.clone() else {
                return false;
            };
            let context = self.edit_request_owner_context();
            let FsExclusiveView::PlaneView(state) = &mut self.fs_exclusive_view else {
                return false;
            };
            let rgb = self.settings.fullscreen_image_margin_color;
            let (params, k_fill) = PlaneRenderParams::screen_with_fill_mode(
                transform,
                canvas,
                state.pose.lens_center,
                state.pose.strength,
                self.settings.plane_view_lens,
                state.tilt,
                material.description,
                underlay,
                egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
                state.fill_mode,
            );
            let snapshot = Arc::new(PlaneFrameSnapshot {
                context,
                anchor: state.pose.anchor,
                material,
                params,
                k_fill,
            });
            let pipeline = render
                .renderer
                .write()
                .callback_resources
                .get_mut::<PlaneRendererCache>()
                .expect("material installed the cache")
                .pipeline(&render.device, render.target_format);
            let clip = canvas.intersect(painter.clip_rect());
            if !clip.is_positive() {
                state.last_drawn = None;
                return false;
            }
            painter.add(egui::Shape::Callback(
                egui_wgpu::Callback::new_paint_callback(
                    clip,
                    PlaneShaderCallback::new(&render.device, pipeline, snapshot.clone()),
                ),
            ));
            state.last_drawn = Some(snapshot);
            return true;
        }
        #[cfg(not(windows))]
        false
    }
    pub(crate) fn plane_frame_request(
        &mut self,
        choice: PlaneOutputChoice,
    ) -> Result<PlaneFrameRequest, String> {
        let not_ready = || "魚眼の表示の準備中です".to_owned();
        let idx = self.fullscreen_idx.ok_or_else(not_ready)?;
        let context = self.edit_request_owner_context();
        let anchor = self.plane_view_anchor(idx);
        let FsExclusiveView::PlaneView(state) = &self.fs_exclusive_view else {
            return Err(not_ready());
        };
        let snapshot = state
            .last_drawn
            .as_ref()
            .filter(|frame| {
                frame.context == context && frame.anchor == anchor && state.pose.anchor == anchor
            })
            .cloned()
            .ok_or_else(not_ready)?;
        #[cfg(windows)]
        {
            let render = self.wgpu_render_state.as_ref().ok_or_else(not_ready)?;
            let size = resolve_plane_output_size(
                snapshot.params.canvas().size(),
                choice,
                &render.device.limits(),
            );
            let pipeline = render
                .renderer
                .write()
                .callback_resources
                .get_mut::<PlaneRendererCache>()
                .ok_or_else(not_ready)?
                .pipeline(&render.device, wgpu::TextureFormat::Rgba8Unorm);
            Ok(PlaneFrameRequest {
                snapshot,
                size,
                device: render.device.clone(),
                queue: render.queue.clone(),
                pipeline,
            })
        }
        #[cfg(not(windows))]
        Err(not_ready())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    fn install(app: &mut App) {
        app.items = vec![GridItem::Image("plane-owner.png".into())];
        app.fullscreen_idx = Some(0);
        let pose = PlaneViewPose {
            anchor: app.plane_view_anchor(0),
            strength: 0.5,
            lens_center: egui::Vec2::splat(0.5),
        };
        app.set_fs_exclusive_view(
            FsExclusiveView::PlaneView(PlaneViewState::new(pose)),
            "plane_owner_fixture",
        );
    }
    #[test]
    fn plane_actual_presentation_pause_and_terminal_close_leave_sibling_unchanged() {
        let _keys = crate::key_input::lock_test_input();
        let mut app = setup_app_for_test();
        let sibling = app.build_window_context_for_test(917_001, install);
        for from in [
            ViewerPresentation::MainWindow,
            ViewerPresentation::Fullscreen,
            ViewerPresentation::DetachedWindow,
        ] {
            for to in [
                ViewerPresentation::MainWindow,
                ViewerPresentation::Fullscreen,
                ViewerPresentation::DetachedWindow,
            ] {
                app.viewer_presentation = from;
                install(&mut app);
                let feedback = app.commit_viewer_session_deferred_feedback(
                    to,
                    false,
                    "plane_presentation_test",
                );
                assert_eq!(app.fs_exclusive_view.is_plane_view(), from == to);
                assert!(
                    feedback.is_none(),
                    "PlaneView does not publish Depth's reentry notification"
                );
                app.with_viewer_context(sibling, |owner| {
                    assert!(owner.fs_exclusive_view.is_plane_view())
                })
                .unwrap();
            }
        }
        install(&mut app);
        app.pause_mounted_background_work_keep_current_frame();
        assert!(!app.fs_exclusive_view.is_plane_view());
        app.with_viewer_context(sibling, |owner| {
            assert!(owner.fs_exclusive_view.is_plane_view())
        })
        .unwrap();
        app.close_and_retire_context(
            sibling,
            "plane_close_test",
            |owner| owner.close_fullscreen(),
            |owner| {
                assert!(
                    !owner
                        .as_ref()
                        .texture_caches()
                        .fs_exclusive_view
                        .is_plane_view()
                );
            },
        )
        .unwrap();
    }
    #[test]
    fn plane_actual_f11_f12_and_live_pass_handoff_use_existing_session_boundaries() {
        let _keys = crate::key_input::lock_test_input();
        let mut app = setup_app_for_test();
        for in_window in [true, false] {
            app.settings.detached_viewer_enabled = false;
            app.settings.detached_viewer_open_images_in_window = false;
            app.settings.video_in_window_mode = in_window;
            app.native_video_in_window_active = in_window;
            app.viewer_presentation = if in_window {
                ViewerPresentation::MainWindow
            } else {
                ViewerPresentation::Fullscreen
            };
            install(&mut app);
            app.toggle_still_window_mode();
            assert!(!app.fs_exclusive_view.is_plane_view());
            install(&mut app);
            app.toggle_detached_viewer_mode();
            assert_eq!(app.viewer_presentation, ViewerPresentation::DetachedWindow);
            assert!(!app.fs_exclusive_view.is_plane_view());
            assert!(!app.fs_feedback_toast.as_ref().unwrap().0.contains("3D"));
            install(&mut app);
            app.toggle_detached_viewer_mode();
            assert!(!app.fs_exclusive_view.is_plane_view());
        }
        for embedded in [false, true] {
            app.viewer_presentation = if embedded {
                ViewerPresentation::MainWindow
            } else {
                ViewerPresentation::Fullscreen
            };
            app.settings.video_in_window_mode = embedded;
            app.native_video_in_window_active = embedded;
            install(&mut app);
            let destination = app.build_window_context_for_test(917_004, install);
            app.retire_main_depth_for_independent_activation(destination);
            assert_eq!(app.fs_exclusive_view.is_plane_view(), embedded);
            app.with_viewer_context(destination, |owner| {
                assert!(owner.fs_exclusive_view.is_plane_view())
            })
            .unwrap();
            app.close_and_retire_context(
                destination,
                "plane_handoff_test_end",
                |owner| owner.close_fullscreen(),
                |_| (),
            )
            .unwrap();
        }
    }

    #[test]
    fn plane_actual_forks_retire_original_and_child_without_retiring_sibling() {
        let _keys = crate::key_input::lock_test_input();
        for live in [false, true] {
            let mut app = setup_app_for_test();
            install(&mut app);
            let folder = app.tmp.path().join("plane-fork");
            app.current_folder = Some(folder.clone());
            app.items[0] = GridItem::Image(folder.join("page.jpg"));
            let sibling = app.build_window_context_for_test(917_002, install);
            let main = app.viewer_context_main();
            let child = if live {
                app.bind_window(main, 917_003).unwrap();
                app.fork_mounted_live_media_context(917_003)
            } else {
                app.fork_materialized_still_context(&folder, 0)
            };
            assert!(!app.fs_exclusive_view.is_plane_view());
            app.with_viewer_context(child, |owner| {
                assert!(!owner.fs_exclusive_view.is_plane_view())
            })
            .unwrap();
            app.with_viewer_context(sibling, |owner| {
                assert!(owner.fs_exclusive_view.is_plane_view())
            })
            .unwrap();
        }
    }
}
