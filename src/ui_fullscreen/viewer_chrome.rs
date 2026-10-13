//! Shared physical-input and guide boundaries for ordinary viewer and page overview.
use super::*;

impl App {
    pub(super) fn dispatch_viewer_middle_short_click(&mut self, ctx: &egui::Context) {
        if let Some(nav) = self.apply_mouse_button(
            ctx,
            crate::ring_shortcut::MouseButtonSlot::Middle,
            crate::app::ActionSurface::Viewer,
            "fullscreen-middle-mouse",
        ) {
            self.mouse_ring_nav = Some(nav);
        }
    }
    pub(super) fn advance_viewer_primary_pointer_frame(
        &mut self,
        ctx: &egui::Context,
    ) -> FullscreenPrimarySuppressionDecision {
        const FOCUS_RESTORE_GRACE: std::time::Duration = std::time::Duration::from_millis(300);
        let (primary_pressed, pointer_primary_down, primary_released) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
            )
        });
        if self.cancel_mouse_right_drag_on_primary_press(
            crate::ring_shortcut::RightDragOwner::Root,
            primary_pressed,
        ) {
            self.fs_primary_suppression.arm_pointer_stream();
        }
        if let Some(t) = self.fs_focus_regained_at {
            if t.elapsed() >= FOCUS_RESTORE_GRACE {
                self.fs_focus_regained_at = None;
            } else if !self.fs_primary_suppression.is_active() && pointer_primary_down {
                self.fs_primary_suppression.arm_pointer_stream();
                self.fs_focus_regained_at = None;
            }
        }
        // Snapshot before releasing persistent ownership. The terminal frame
        // itself remains suppressed, while later early returns cannot strand
        // the owner because the viewer calls this before key-driven surface changes.
        let primary_suppression =
            self.fs_primary_suppression
                .apply(FullscreenPrimarySuppressionEvent::PointerFrame {
                    released: primary_released,
                });
        primary_suppression
    }

    pub(super) fn handle_viewer_right_drag_frame(
        &mut self,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        is_video: bool,
        secondary_in_overlay_chrome: bool,
        prev_foreground_hwnd: usize,
    ) -> bool {
        let mut close = false;
        // 分析モード中は右クリックを色固定に使うため、終了トリガーにしない
        // コンテキストメニュー表示中は右クリック処理をスキップ
        // 表示モード / フィットのポップアップ表示中も、右クリックはメニューを閉じる
        // 用途 (popup 側の外クリック判定) に専念させ、フルスクリーン終了 / コンテキスト
        // メニューを誤発火させない。handle_fs_wheel_and_click は hover bar 描画より前に
        // 走るため、ここで読む popup 状態はまだ閉じられていない。
        // Native video owns all of its own right-click handling (ring start/update/cancel,
        // close, long-press menu) through the presenter window — see
        // `handle_native_video_mouse_button`. The egui viewport behind the presenter never
        // receives the video's mouse events, so running this block for video would report
        // `secondary_down == false` every frame and cancel the presenter-created flick. In
        // the non-detached path `render_fullscreen_viewport` early-returns before reaching
        // here; in the detached path the egui body runs, so we must skip video explicitly.
        //
        // Exception (Inc 7 動画→音声モード): a video toggled into audio mode has its native
        // presenter *hidden* (SW_HIDE, consume-and-hold) with the egui music view on top, so
        // this viewport DOES receive the right-click and `handle_video_input` is gated off
        // (see `fs_music_view_active` at handle_fs_keyboard). The native path never runs, so we
        // must run this egui block instead — otherwise right-click (close) and the right-drag
        // ring shortcut do nothing (実機 FB 2026-07). The context is `current_right_drag_context()`
        // = VideoFullscreen (item is Video), so it shows the video ring, not the image ring.
        let dialog_open_for_right_drag = self.any_dialog_open();
        if dialog_open_for_right_drag {
            self.cancel_mouse_ring_flick();
            self.cancel_mouse_gesture();
            self.fs_secondary_press.cancel();
        }
        let video_in_audio_mode = is_video
            && self
                .fullscreen_idx
                .is_some_and(|idx| self.fs_music_view_active(idx));
        let right_drag_context = self.current_right_drag_context();
        let right_drag_mode = self
            .settings
            .ring_shortcuts
            .right_drag_mode(right_drag_context);
        let secondary_down = ctx.input(|i| i.pointer.secondary_down());
        let secondary_pressed = ctx.input(|i| i.pointer.secondary_pressed());
        let secondary_released = ctx.input(|i| i.pointer.secondary_released());
        let secondary_pos = fullscreen_pointer_pos_or(ctx, full_rect, full_rect.center());
        let passive_secondary_pos = fullscreen_pointer_pos_or(
            ctx,
            full_rect,
            self.fs_secondary_press
                .start_pos()
                .unwrap_or(full_rect.center()),
        );
        // 旧 egui HUD は撤去済 (native presenter overlay が右クリックも独自処理)。
        // egui main window 側に右クリックを吸収すべき HUD 矩形は無い。
        let popup_open = self.spread_popup_open
            || self.fit_popup_open
            || self.slideshow_popup_open
            || self.panorama_projection_popup_open
            || fs_rotation_popup_open(ctx)
            || fs_still_seek_strip_popup_open(ctx);
        let right_drag_gate_reject = if is_video && !video_in_audio_mode {
            Some("native_video")
        } else if self.analysis_mode {
            Some("analysis")
        } else if dialog_open_for_right_drag {
            Some("dialog")
        } else if self.fs_context_menu_idx.is_some() {
            Some("context_menu")
        } else if popup_open {
            Some("popup")
        } else {
            None
        };
        let passive_press_owner = fullscreen_secondary_press_owner_for_frame(
            self.fullscreen_idx.map(|fs_idx| {
                FullscreenSecondaryPressOwner::new(
                    // This existing helper is the cross-platform current projection owner;
                    // edit requests and physical pointer sequences need the same exact scope.
                    self.edit_request_owner_context(),
                    self.items_generation,
                    fs_idx,
                    right_drag_context,
                )
            }),
            &right_drag_mode,
            right_drag_context,
            right_drag_gate_reject.is_some(),
            secondary_in_overlay_chrome,
        );
        let passive_press_outcome = self.fs_secondary_press.advance(
            passive_press_owner,
            FullscreenSecondaryPressSample {
                pressed: secondary_pressed,
                down: secondary_down,
                released: secondary_released,
                pos: passive_secondary_pos,
                now: std::time::Instant::now(),
            },
        );
        #[cfg(windows)]
        if self.viewer_session_is_detached_or_switching()
            && Self::detached_image_window_debug_enabled()
        {
            let (
                focused,
                event_count,
                key_pressed_count,
                wheel_event_count,
                secondary_button_events,
            ) = ctx.input(|i| {
                let key_pressed_count = i
                    .events
                    .iter()
                    .filter(|event| matches!(event, egui::Event::Key { pressed: true, .. }))
                    .count();
                let wheel_event_count = i
                    .events
                    .iter()
                    .filter(|event| matches!(event, egui::Event::MouseWheel { .. }))
                    .count();
                let secondary_button_events = i
                    .events
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            egui::Event::PointerButton {
                                button: egui::PointerButton::Secondary,
                                ..
                            }
                        )
                    })
                    .count();
                (
                    i.viewport().focused.unwrap_or(false),
                    i.events.len(),
                    key_pressed_count,
                    wheel_event_count,
                    secondary_button_events,
                )
            });
            let right_drag_probe_event_frame =
                secondary_pressed || secondary_released || secondary_button_events > 0;
            if right_drag_probe_event_frame {
                let foreground = current_foreground_hwnd();
                let active_hwnd = self.detached_viewer_host_hwnd_raw() as usize;
                self.log_detached_image_window_debug(format!(
                    "right_drag_probe fs_idx={:?} window_id={:?} window_state={:?} \
                 session={:?} is_video={} video_audio_mode={} analysis={} \
                 dialog={} popup={} context_menu={} focused={} \
                 foreground=0x{foreground:x} active_hwnd=0x{active_hwnd:x} \
                 foreground_matches={} prev_foreground=0x{:x} \
                 secondary_down={} secondary_pressed={} secondary_released={} \
                 secondary_pos=({:.1},{:.1}) secondary_in_overlay_chrome={} \
                 right_drag_context={right_drag_context:?} right_drag_mode={right_drag_mode:?} \
                 gate={} events={} key_pressed={} wheel_events={} secondary_button_events={} \
                 frame={}",
                    self.fullscreen_idx,
                    self.active_detached_window_id(),
                    self.active_detached_window_state(),
                    self.active_detached_session,
                    is_video,
                    video_in_audio_mode,
                    self.analysis_mode,
                    dialog_open_for_right_drag,
                    popup_open,
                    self.fs_context_menu_idx.is_some(),
                    focused,
                    foreground != 0 && foreground == active_hwnd,
                    prev_foreground_hwnd,
                    secondary_down,
                    secondary_pressed,
                    secondary_released,
                    secondary_pos.x,
                    secondary_pos.y,
                    secondary_in_overlay_chrome,
                    right_drag_gate_reject.unwrap_or("ok"),
                    event_count,
                    key_pressed_count,
                    wheel_event_count,
                    secondary_button_events,
                    self.frame_counter
                ));
            }
        }
        if (!is_video || video_in_audio_mode)
            && !self.analysis_mode
            && !dialog_open_for_right_drag
            && self.fs_context_menu_idx.is_none()
            && !self.spread_popup_open
            && !self.fit_popup_open
            && !self.slideshow_popup_open
            && !self.panorama_projection_popup_open
            && !fs_rotation_popup_open(ctx)
            && !fs_still_seek_strip_popup_open(ctx)
        {
            match right_drag_mode {
                crate::ring_shortcut::RightDragMode::RingShortcut => {
                    let Some(ring_context) = right_drag_context.ring_context() else {
                        return close;
                    };
                    if secondary_in_overlay_chrome {
                        self.cancel_mouse_ring_flick();
                    } else if secondary_pressed {
                        self.start_mouse_ring_flick(
                            ctx,
                            crate::ring_shortcut::RightDragOwner::Root,
                            ring_context,
                            secondary_pos,
                            None,
                        );
                    }
                    match self.update_mouse_ring_flick_with_pos(
                        ctx,
                        crate::ring_shortcut::RightDragOwner::Root,
                        ring_context,
                        secondary_pos,
                        secondary_down,
                        secondary_released,
                    ) {
                        crate::ring_shortcut::MouseFlickOutcome::ShortTap => {
                            close |= self.apply_viewer_short_right_click_action(
                                right_drag_context,
                                self.fullscreen_idx,
                                secondary_pos,
                            );
                        }
                        crate::ring_shortcut::MouseFlickOutcome::Fired
                        | crate::ring_shortcut::MouseFlickOutcome::Cancelled
                        | crate::ring_shortcut::MouseFlickOutcome::None => {}
                    }
                }
                crate::ring_shortcut::RightDragMode::MouseGesture => {
                    if secondary_in_overlay_chrome {
                        self.cancel_mouse_gesture();
                    } else if secondary_pressed {
                        self.start_mouse_gesture(
                            ctx,
                            crate::ring_shortcut::RightDragOwner::Root,
                            right_drag_context,
                            secondary_pos,
                            None,
                        );
                    }
                    match self.update_mouse_gesture_with_pos(
                        ctx,
                        crate::ring_shortcut::RightDragOwner::Root,
                        right_drag_context,
                        secondary_pos,
                        secondary_down,
                        secondary_released,
                    ) {
                        crate::ring_shortcut::MouseFlickOutcome::ShortTap => {
                            close |= self.apply_viewer_short_right_click_action(
                                right_drag_context,
                                self.fullscreen_idx,
                                secondary_pos,
                            );
                        }
                        crate::ring_shortcut::MouseFlickOutcome::Fired
                        | crate::ring_shortcut::MouseFlickOutcome::Cancelled
                        | crate::ring_shortcut::MouseFlickOutcome::None => {}
                    }
                }
                crate::ring_shortcut::RightDragMode::Disabled
                | crate::ring_shortcut::RightDragMode::Unknown(_) => {
                    if right_drag_context == crate::ring_shortcut::RightDragContext::EditMode {
                        return close;
                    }
                    match passive_press_outcome {
                        FullscreenSecondaryPressOutcome::ShortTap(pos) => {
                            close |= self.apply_viewer_short_right_click_action(
                                right_drag_context,
                                self.fullscreen_idx,
                                pos,
                            );
                        }
                        FullscreenSecondaryPressOutcome::LongPress(pos) => {
                            if self.page_overview.session().is_none() {
                                self.fs_context_menu_idx = self.fullscreen_idx;
                                self.fs_context_menu_pos = pos;
                            }
                        }
                        FullscreenSecondaryPressOutcome::Cancelled
                        | FullscreenSecondaryPressOutcome::None => {}
                    }
                }
            }
        }
        close
    }

    pub(super) fn paint_viewer_presentation_interlude(
        &mut self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        ctx: &egui::Context,
        detached: bool,
        canvas_color: egui::Color32,
    ) -> bool {
        #[cfg(windows)]
        let blocked = detached && self.step_detached_viewer_borderless_transition(ctx);
        #[cfg(not(windows))]
        let blocked = {
            let _ = (ctx, detached);
            false
        };
        if blocked {
            ui.painter().rect_filled(full_rect, 0.0, canvas_color);
        }
        blocked
    }

    pub(super) fn draw_viewer_shared_overlays(
        &mut self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        ctx: &egui::Context,
    ) {
        // ── 右上フィードバックトースト ──
        let ring_surface_context =
            self.ring_shortcut_context_for_surface(crate::app::ActionSurface::Viewer);
        if self.mouse_ring_flick.is_some()
            && ring_surface_context == crate::ring_shortcut::RingShortcutContext::VideoFullscreen
        {
            self.sync_native_video_ring_guide_overlay_for_context(ctx, ring_surface_context);
        }
        self.draw_mouse_ring_flick_overlay(ui, full_rect, ring_surface_context);
        self.draw_mouse_gesture_overlay(ui, full_rect, self.current_right_drag_context());
        self.draw_gamepad_ring_overlay(ui, full_rect, crate::app::ActionSurface::Viewer);
        self.draw_gamepad_picker_overlay(ui, full_rect, crate::app::ActionSurface::Viewer);
        self.draw_gamepad_favorite_picker_overlay(ui, full_rect, crate::app::ActionSurface::Viewer);
        self.draw_gamepad_video_marker_picker_overlay(
            ui,
            full_rect,
            crate::app::ActionSurface::Viewer,
        );
        self.draw_feedback_toast(ui, full_rect, ctx, crate::app::ActionSurface::Viewer);

        self.draw_colorize_wait_overlay(ui, full_rect, ctx);

        // ── スタンプ埋め込み worker の進行表示 (中央「読み込み中」) ──
        self.draw_stamp_embed_overlay(ui, full_rect, ctx);

        // ── 注釈ベイク worker の進行表示 (中央「テキスト処理中…」) ──
        self.draw_comic_bake_overlay(ui, full_rect, ctx);

        // 動画ブックマーク名編集ダイアログは native presenter overlay の
        // 中で描画される (= `native_presenter/overlay_draw.rs::draw_native_*`)。
        // eframe ビューポートからは描画しない。

        // ── 中央の先頭 / 末尾ヒント ──
        self.draw_boundary_hint(ui, full_rect, ctx);
    }

    pub(super) fn draw_viewer_shared_dialogs(&mut self, ctx: &egui::Context) {
        // ── スロット保存ダイアログ ──
        self.draw_slot_save_dialog(ctx);
        self.draw_favorite_default_clear_confirm_dialog(ctx);
        self.draw_export_dialog(ctx);
        self.draw_export_progress_dialog(ctx);
    }
}

#[cfg(test)]
pub(crate) fn assert_prepared_viewer_frame_idx(ctx: &egui::Context, idx: usize) {
    let sample = ctx
        .data(|data| {
            data.get_temp::<FsOriginalPreviewFrameSample>(egui::Id::new((
                "fs_original_preview_active",
                ctx.viewport_id(),
            )))
        })
        .expect("production frame preparation must record its existing observation");
    assert_eq!(
        sample.idx, idx,
        "post-overview consumers must use the landed page frame"
    );
}
