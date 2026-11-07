use super::*;
use crate::depth_parallax::{
    DepthEntryBlock, DepthEntryInput, DepthSourceMaterial, DepthSourceWait, DepthViewState,
};
use crate::fs_exclusive_view::{ExclusiveViewKind, FsExclusiveView};

impl App {
    pub(crate) fn panorama_state(&self) -> Option<&crate::panorama::PanoramaState> {
        self.fs_exclusive_view.panorama_state()
    }

    pub(crate) fn panorama_state_mut(&mut self) -> Option<&mut crate::panorama::PanoramaState> {
        self.fs_exclusive_view.panorama_state_mut()
    }

    /// All live transitions retire the old owner before publishing its replacement.
    pub(crate) fn set_fs_exclusive_view(&mut self, target: FsExclusiveView, _cause: &'static str) {
        let leaving_panorama = matches!(self.fs_exclusive_view, FsExclusiveView::Panorama(_));
        if leaving_panorama {
            self.panorama_projection_popup_open = false;
            self.clear_pano_upload();
            self.clear_pano_refinement();
            for (_, request) in self.pano_high_res_pending.drain() {
                request
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        // Depth's Drop cancels the per-request receiver and clears only its context's slot.
        let previous = std::mem::take(&mut self.fs_exclusive_view);
        drop(previous);
        if matches!(target, FsExclusiveView::Depth(_)) {
            self.panorama_intent.on = false;
        }
        self.fs_exclusive_view = target;
        #[cfg(windows)]
        if leaving_panorama && let Some(idx) = self.fullscreen_idx {
            if matches!(self.fs_cache.get(&idx), Some(FsCacheEntry::Video { .. })) {
                self.sync_native_video_panorama_state(idx);
            }
        }
    }

    pub(crate) fn fs_exclusive_view_active(&self, fs_idx: usize) -> Option<ExclusiveViewKind> {
        match &self.fs_exclusive_view {
            FsExclusiveView::Depth(_) => Some(ExclusiveViewKind::Depth),
            FsExclusiveView::Panorama(_) if self.is_panorama_mode_active(fs_idx) => {
                Some(ExclusiveViewKind::Panorama)
            }
            _ => None,
        }
    }

    pub(crate) fn depth_entry_allowed(&mut self, fs_idx: usize) -> Result<(), DepthEntryBlock> {
        if self.fullscreen_idx != Some(fs_idx) {
            return Err(DepthEntryBlock::NotImageFullscreen);
        }
        #[cfg(windows)]
        let (main_context, main_presentation, wgpu_available) = (
            self.projected_viewer_context_id() == self.viewer_context_main(),
            self.viewer_presentation == ViewerPresentation::Fullscreen,
            self.wgpu_render_state.is_some(),
        );
        #[cfg(not(windows))]
        let (main_context, main_presentation, wgpu_available) = (true, true, false);
        crate::depth_parallax::entry_allowed(DepthEntryInput {
            image_fullscreen: self.fullscreen_idx == Some(fs_idx)
                && self.items.get(fs_idx).is_some_and(GridItem::has_page_data),
            is_video: matches!(
                self.items.get(fs_idx),
                Some(GridItem::Video(_) | GridItem::Audio(_))
            ),
            is_animation: self.fs_entry_is_animated(fs_idx),
            spread_double: matches!(
                self.resolve_visible_spread_pair(fs_idx),
                crate::ui_fullscreen::SpreadPair::Double { .. }
            ),
            continuous_reading: !self.reading_flow.is_paged(),
            main_context,
            main_presentation,
            wgpu_available,
            editing: self.is_overlay_edit_mode_active(),
            page_turning: self.page_turn_barrier_active() || self.page_turn_completion_active(),
        })
    }

    pub(crate) fn depth_view_may_continue(&mut self, fs_idx: usize) -> bool {
        if !self.fs_exclusive_view.is_depth() {
            return false;
        }
        if let Err(reason) = self.depth_entry_allowed(fs_idx) {
            self.set_fs_exclusive_view(FsExclusiveView::None, "continuation_rejected");
            self.show_feedback_toast(format!("3D 写真を終了しました: {}", reason.message()));
            return false;
        }
        true
    }

    /// Presentation is a context-owned fact; committing a non-main-fullscreen target
    /// retires Depth before the next viewport can observe the context.
    #[cfg(windows)]
    pub(crate) fn commit_viewer_presentation(
        &mut self,
        target: ViewerPresentation,
        cause: &'static str,
    ) {
        if self.fs_exclusive_view.is_depth()
            && (target != ViewerPresentation::Fullscreen
                || self.projected_viewer_context_id() != self.viewer_context_main())
        {
            self.set_fs_exclusive_view(FsExclusiveView::None, cause);
        }
        self.viewer_presentation = target;
    }

    pub(crate) fn toggle_depth_parallax(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.fs_exclusive_view.is_depth() {
            self.set_fs_exclusive_view(FsExclusiveView::None, "depth_toggle_off");
            ctx.request_repaint();
            return;
        }
        if let Err(reason) = self.depth_entry_allowed(fs_idx) {
            self.show_feedback_toast(reason.message().to_owned());
            return;
        }
        // Setup owns its lazy scan. Unknown installation is Checking, never Missing.
        if self.depth_setup.installed_model_path().is_none() {
            self.request_depth_addon_setup(ctx);
            return;
        }
        let mut state = DepthViewState::new(self.edit_request_owner_context());
        #[cfg(windows)]
        if let Some(renderer) = &self.wgpu_render_state {
            state.attach_renderer(renderer);
        }
        self.set_fs_exclusive_view(FsExclusiveView::Depth(state), "depth_toggle_on");
        self.close_fs_exclusive_conflicts();
        ctx.request_repaint();
    }

    pub(crate) fn toggle_depth_panel(&mut self) {
        if let FsExclusiveView::Depth(state) = &mut self.fs_exclusive_view {
            state.panel_visible = !state.panel_visible;
        }
    }

    pub(crate) fn try_paint_depth(
        &mut self,
        painter: &egui::Painter,
        ctx: &egui::Context,
        payload: &crate::gpu_lanczos::FullscreenPaintPayload,
        transform: &crate::displayed_image_transform::DisplayedImageTransform,
        underlay: &crate::ui_fullscreen::FsBgStyle,
    ) -> bool {
        #[cfg(windows)]
        {
            let Some(render) = self.wgpu_render_state.as_ref() else {
                return false;
            };
            let FsExclusiveView::Depth(state) = &mut self.fs_exclusive_view else {
                return false;
            };
            let Some(resource) = state.ready() else {
                return false;
            };
            if payload
                .source_texture()
                .is_none_or(|texture| texture.id() != resource.key.texture_id)
            {
                return false;
            }
            let panel = state
                .panel_visible
                .then(|| {
                    ctx.data(|data| {
                        data.get_temp::<egui::Rect>(egui::Id::unique("depth_panel_rect"))
                    })
                })
                .flatten();
            let (pointer, dt, visible) = ctx.input(|input| {
                (
                    input.pointer.hover_pos(),
                    input.stable_dt,
                    input.viewport().minimized != Some(true),
                )
            });
            let settings = &self.settings.depth_parallax;
            let (offset, moving) = if settings.motion == crate::depth_parallax::MotionKind::Mouse {
                let moving = state.mouse.update(
                    pointer,
                    transform.paint_rect,
                    pointer.is_some_and(|point| panel.is_some_and(|rect| rect.contains(point))),
                    settings.strength,
                    dt,
                );
                (state.mouse.current, moving)
            } else {
                (
                    crate::depth_parallax::motion_offset(
                        settings.motion,
                        settings.strength,
                        state.started_at.elapsed().as_secs_f32() / settings.period_secs,
                    ),
                    true,
                )
            };
            if moving && self.window_visible && visible {
                ctx.request_repaint();
            }
            let params = crate::depth_parallax_wgpu::DepthRenderParams::from_settings(
                settings,
                resource.depth.median,
                offset,
                transform.uv_rect,
                transform.rotation.degrees() as u32 / 90,
                transform.paint_rect.aspect_ratio(),
            );
            let callback = crate::depth_parallax_wgpu::DepthShaderCallback {
                context: resource.context,
                key: resource.key,
                resource_id: resource.resource_id,
                params,
                target_format: render.target_format,
            };
            state.publish(&mut render.renderer.write().callback_resources);
            crate::ui_fullscreen::paint_image_underlay(painter, transform.paint_quad(), underlay);
            painter.add(egui::Shape::Callback(
                egui_wgpu::Callback::new_paint_callback(transform.paint_rect, callback),
            ));
            return true;
        }
        #[cfg(not(windows))]
        false
    }

    pub(crate) fn draw_depth_panel(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.depth_entry_allowed(fs_idx).is_err() {
            return;
        }
        let FsExclusiveView::Depth(state) = &self.fs_exclusive_view else {
            return;
        };
        if !state.panel_visible {
            return;
        }
        let status = match &state.source {
            crate::depth_parallax::DepthSourcePhase::Failed { reason, .. } => {
                format!("失敗しました ({reason})")
            }
            crate::depth_parallax::DepthSourcePhase::Ready { .. } => String::new(),
            _ if matches!(
                self.ai_runtime_init.snapshot(),
                crate::ai::runtime::AiRuntimeInitSnapshot::Dormant
                    | crate::ai::runtime::AiRuntimeInitSnapshot::Initializing
            ) =>
            {
                "モデルを準備中".to_owned()
            }
            _ => "奥行きを計算中".to_owned(),
        };
        let response = crate::ui_depth_parallax::draw_depth_parallax_panel(
            ctx,
            &mut self.settings.depth_parallax,
            &status,
        );
        ctx.data_mut(|data| data.insert_temp(egui::Id::unique("depth_panel_rect"), response.rect));
        if response.changed {
            self.settings.save();
        }
        if response.close_requested {
            self.set_fs_exclusive_view(FsExclusiveView::None, "depth_panel_close");
        }
    }

    #[cfg(windows)]
    pub(crate) fn advance_depth_source(
        &mut self,
        ctx: &egui::Context,
        material: DepthSourceMaterial,
    ) {
        // A normal show pass resumes polling and adoption; hidden passes stay idle.
        if !self.window_visible || ctx.input(|input| input.viewport().minimized == Some(true)) {
            return;
        }
        let Some(render) = self.wgpu_render_state.clone() else {
            return;
        };
        let backing = match &material {
            DepthSourceMaterial::Ready { texture } => render
                .renderer
                .read()
                .texture(&texture.id())
                .and_then(|texture| texture.texture.clone()),
            _ => None,
        };
        self.ensure_ai_runtime();
        let runtime = self.ai_runtime_init.clone();
        let activity = self.local_ai_activity_lease();
        let context = self.edit_request_owner_context();
        let viewport = ctx.viewport_id();
        let input_seq = self.input_seq;
        let FsExclusiveView::Depth(state) = &mut self.fs_exclusive_view else {
            return;
        };
        let failed_before = match &state.source {
            crate::depth_parallax::DepthSourcePhase::Failed { key, .. } => Some(*key),
            _ => None,
        };
        let selected = match material {
            DepthSourceMaterial::Ready { texture } => match backing {
                Some(backing) => state.select_source(
                    &render.device,
                    &render.queue,
                    &backing,
                    crate::depth_estimation::DepthSourceKey {
                        texture_id: texture.id(),
                    },
                ),
                None => {
                    state.wait_for_source(DepthSourceWait::TexturePending);
                    // Managed textures are registered by the renderer after UI.
                    // Consume that registration next pass even when the pointer is idle.
                    ctx.request_repaint_of(viewport);
                    Ok(())
                }
            },
            DepthSourceMaterial::NotYet(reason) => {
                state.wait_for_source(reason);
                Ok(())
            }
        };
        if let Err(error) = selected {
            crate::logger::log(format!("[Depth] {error}"));
        }
        state.advance(
            &render.device,
            &render.queue,
            ctx,
            viewport,
            |key, input, guide| {
                crate::depth_estimation::DepthWorker::shared()?.submit(
                    context,
                    viewport,
                    key,
                    input,
                    guide,
                    runtime,
                    activity,
                    ctx.clone(),
                    input_seq,
                )
            },
        );
        if let crate::depth_parallax::DepthSourcePhase::Failed { key, reason } = &state.source
            && failed_before != Some(*key)
        {
            let message = format!("3D 写真の処理に失敗しました: {reason}");
            self.show_feedback_result(message, crate::feedback::FeedbackKind::Error);
        }
    }
}
