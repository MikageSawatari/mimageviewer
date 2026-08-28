//! タスクトレイ常駐 / インデクサ throttle の App 側統合 (v0.9)。
//!
//! `App` に対する impl ブロックで、タスクトレイのライフサイクル管理、
//! ウィンドウ可視状態遷移、GPU リソース解放、メニューイベント処理を扱う。
//! ロジックを `src/app.rs` から切り出すことで、本体の肥大化を抑える。

use eframe::egui;

use crate::app::App;
use crate::tray::TrayEvent;

fn should_close_fullscreen_for_tray(
    fullscreen_open: bool,
    fs_cache_has_video: bool,
    native_video_pending: bool,
    detached_viewer_session: bool,
) -> bool {
    !detached_viewer_session && fullscreen_open && !fs_cache_has_video && !native_video_pending
}

impl App {
    /// Keep every already-owned egui host registered while changing only OS visibility.
    /// The fullscreen id covers the active F12 host; detached snapshots cover passive and
    /// ParkedLive hosts. Duplicate ids are collapsed before commands are queued.
    fn sync_retained_viewport_visibility_for_tray(
        &self,
        ctx: &egui::Context,
        visible: bool,
    ) -> usize {
        let mut viewport_ids = Vec::new();
        if self.fs_viewport_shown {
            viewport_ids.push(self.fullscreen_viewport_id());
        }
        #[cfg(windows)]
        {
            for window in &self.detached_image_windows {
                let viewport_id = Self::detached_image_window_viewport_id(window.id);
                if !viewport_ids.contains(&viewport_id) {
                    viewport_ids.push(viewport_id);
                }
            }
        }
        let transition_owned_viewport = (visible
            && self.video_presentation_transition.is_transitioning())
        .then(|| self.fullscreen_viewport_id());
        let mut commanded = 0;
        for viewport_id in &viewport_ids {
            if transition_owned_viewport == Some(*viewport_id) {
                continue;
            }
            ctx.send_viewport_cmd_to(*viewport_id, egui::ViewportCommand::Visible(visible));
            self.observe_viewport_presentation_command(
                *viewport_id,
                crate::presentation_observer::WindowAction::Visible,
                "tray::set_all_viewports_visible",
                if visible { "value=true" } else { "value=false" },
            );
            commanded += 1;
        }
        commanded
    }

    /// Request the same root viewport close as the main window's [x] button.
    /// The next update lets `maybe_intercept_close` apply tray residency rules.
    pub(crate) fn request_main_window_close(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
    }

    /// Explicitly quit the application, bypassing tray residency while still
    /// using eframe's normal close/on-exit persistence path.
    pub(crate) fn request_application_quit(&self, ctx: &egui::Context) {
        self.shutdown_requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.request_main_window_close(ctx);
    }

    /// 設定状態に応じてタスクトレイコントローラの起動 / 停止を同期する。
    /// - 設定 ON + tray_controller=None + HWND 取得済み → 起動
    /// - 設定 OFF + tray_controller=Some → 停止 (アイコンが消える)
    /// - 既に合致している → no-op
    ///
    /// HWND がまだ取得できていない (= 最初のフレーム前) 場合は起動を延期する。
    /// `ctx` はトレイスレッドに渡す clone 用。
    pub(crate) fn sync_tray_with_settings(&mut self, ctx: &egui::Context) {
        let should_be_on = self.settings.minimize_to_tray_on_close;
        match (should_be_on, self.tray_controller.is_some()) {
            (true, false) => {
                // HWND がまだなければ起動を延期 (最初のフレームで取得される)。
                let Some(hwnd_raw) = self.main_hwnd else {
                    return;
                };
                if let Some((rgba, w, h)) = crate::tray::load_embedded_icon_rgba() {
                    let io_sem = self.indexer_manager.as_ref().map(|m| m.io_sem());
                    // 配置共有スロットをまだ作っていなければ作成。
                    if self.placement_slot.is_none() {
                        self.placement_slot = Some(crate::tray::new_placement_slot());
                    }
                    let slot = self.placement_slot.clone().unwrap();
                    self.tray_controller = crate::tray::TrayController::start(
                        rgba,
                        w,
                        h,
                        ctx.clone(),
                        hwnd_raw,
                        std::sync::Arc::clone(&self.activity_gate),
                        io_sem,
                        slot,
                    );
                    if self.tray_controller.is_none() {
                        crate::logger::log(
                            "tray: controller start returned None (non-Windows or spawn failed)",
                        );
                    } else {
                        // checkmark は設定値 (pause_indexer_while_minimized) を反映する
                        // (activity_gate.is_paused() はウィンドウ表示中は常に false で、
                        // ダイアログのチェックボックスとズレるため)。
                        self.sync_tray_pause_check();
                        self.update_tray_tooltip();
                    }
                }
            }
            (false, true) => {
                // 停止: Drop 実装がスレッド shutdown を処理する。
                self.tray_controller = None;
                if let Some(mgr) = self.indexer_manager.as_ref() {
                    mgr.set_io_throttled(false);
                }
                self.activity_gate.set_paused(false);
            }
            _ => {}
        }
    }

    /// Publish the latest media-owner projection to the existing tray thread.
    /// No App-side wake state is stored: visible/hidden, player transport, EOF navigation,
    /// and source-swap ownership remain the sources of truth.
    pub(crate) fn sync_tray_resident_media_wake(&self) {
        let Some(controller) = self.tray_controller.as_ref() else {
            return;
        };
        #[cfg(windows)]
        {
            let enabled = self.tray_resident_media_updates_needed();
            if controller.set_resident_media_wake_enabled(enabled) {
                let keep_heartbeat_alive = self.ui_heartbeat_should_stay_active_while_hidden();
                crate::set_ui_heartbeat_suspended(
                    !self.window_visible && !keep_heartbeat_alive,
                    if !self.window_visible && keep_heartbeat_alive {
                        "App::update heartbeat kept alive for active hidden viewer session"
                            .to_string()
                    } else if self.window_visible {
                        "App::update heartbeat follows visible window".to_string()
                    } else {
                        "App::update heartbeat suspended for idle tray residency".to_string()
                    },
                );
            }
        }
        #[cfg(not(windows))]
        let _ = controller.set_resident_media_wake_enabled(false);
    }

    /// Every `App::update` entry consumes the one wake claimed by the tray thread. A vendored
    /// scheduler pass and a queued `WM_PAINT` are serialized on the winit main thread; whichever
    /// enters first clears the claim so the 50 ms pump can post the next bounded media wake.
    pub(crate) fn acknowledge_tray_resident_media_wake(&self) {
        if let Some(controller) = self.tray_controller.as_ref() {
            controller.acknowledge_resident_media_wake();
        }
    }

    /// 閉じるボタン [×] が押されたか検出し、設定 ON + トレイ起動中なら hide に差し替える。
    /// 返り値は「hide に差し替えた (= アプリを終了させない)」かどうか。
    pub(crate) fn maybe_intercept_close(&mut self, ctx: &egui::Context) -> bool {
        // トレイメニュー「終了」 / インストーラからの shutdown 要求は常に通す。
        let tray_wants_quit = self
            .tray_controller
            .as_ref()
            .is_some_and(|tc| tc.is_quit_requested());
        let installer_wants_quit = self
            .shutdown_requested
            .load(std::sync::atomic::Ordering::SeqCst);
        if tray_wants_quit || installer_wants_quit {
            return false;
        }
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if !close_requested {
            return false;
        }
        if !self.settings.minimize_to_tray_on_close || self.tray_controller.is_none() {
            return false;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.hide_to_tray(ctx);
        true
    }

    /// ウィンドウを非表示にしてタスクトレイ状態へ遷移する。
    ///
    /// **重要**: `ViewportCommand::Visible(false)` は使わない。Win32
    /// `ShowWindow(hwnd, SW_HIDE)` を直接使うことで、トレイスレッドは App を経由せず復帰できる。
    /// hidden 中は vendored eframe scheduler がアプリ要求済み repaint だけを 100 ms 以上の間隔で
    /// direct UI pass として消化し、要求が無ければ sleep する。active media は別途、同スレッドの
    /// bounded 50 ms `WM_PAINT` bridge が EOF / 次トラック遷移を駆動する。両入口は winit main
    /// thread 上で直列化され、各 `App::update` 入口が bridge の pending claim を ack する。
    ///
    /// サイズ保存について: hide の直前に `GetWindowPlacement` で rect を丸ごと捕獲しておき、
    /// 復帰時に `SetWindowPlacement` で完全復元する。eframe/winit の DPI 丸めを完全に
    /// バイパスできるため、マルチモニタ DPI 環境でも開閉でサイズが変わらない。
    fn hide_to_tray(&mut self, ctx: &egui::Context) {
        if !self.window_visible {
            return;
        }
        // Preserve the media session and transport. Native presenters enter their existing
        // consume-and-hold state; egui hosts remain registered but become explicitly hidden.
        let routed_presenters = self.prepare_media_session_for_tray_residency();
        let retained_viewports = self.sync_retained_viewport_visibility_for_tray(ctx, false);
        self.window_visible = false;
        self.sync_tray_resident_media_wake();
        let keep_heartbeat_alive = self.ui_heartbeat_should_stay_active_while_hidden();
        crate::set_ui_heartbeat_suspended(
            !keep_heartbeat_alive,
            if keep_heartbeat_alive {
                "App::update heartbeat kept alive for active viewer session while hidden to tray"
            } else {
                "App::update heartbeat suspended for idle tray residency"
            }
            .to_string(),
        );

        // 先に WINDOWPLACEMENT を共有スロットに保存してから hide する。
        // トレイスレッドの復帰処理が ShowWindow より**前**にこれを読み取って
        // SetWindowPlacement することで、表示直後のジャンプや黒フラッシュを抑える。
        #[cfg(windows)]
        if let Some(hwnd_raw) = self.main_hwnd {
            if let Some(slot) = &self.placement_slot {
                let captured = crate::tray::capture_window_placement(hwnd_raw);
                *slot.lock().unwrap() = captured;
            }
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
            unsafe {
                let _ = crate::presentation_observer::show_window(
                    HWND(hwnd_raw as *mut _),
                    SW_HIDE,
                    crate::presentation_observer::WindowRole::Main,
                    "tray_integration::hide_to_tray",
                );
            }
        }

        // I/O throttle: 他アプリへの帯域影響を抑える
        if let Some(mgr) = self.indexer_manager.as_ref() {
            mgr.set_io_throttled(true);
        }

        // ユーザー明示の pause フラグ (既定 OFF)
        if self.settings.pause_indexer_while_minimized {
            self.activity_gate.set_paused(true);
        }

        // GPU リソース解放 (best-effort)
        self.release_gpu_resources();
        if self.viewer_session_is_detached_or_switching() {
            ctx.request_repaint();
        }
        // settings / sidecar は従来どおり hide 時にも保存する。トレイ「終了」は DWM cloak 後に
        // 通常 close を通るため、リネーム journal writer の終了 flush は on_exit が担当する。
        self.persist_window_state_and_flush();

        // トレイの表示を更新
        self.update_tray_tooltip();
        crate::logger::log(format!(
            "tray: window hidden to tray (Win32 SW_HIDE, placement saved); \
             retained_viewports={retained_viewports} routed_presenters={routed_presenters}"
        ));
    }

    /// タスクトレイから復帰した後の **App 側事後処理**。
    /// トレイスレッドの OpenRequested 経路と、外部 ShowWindow 検出経路 (アクティベーション
    /// リスナー等) の両方から呼ばれる。
    pub(crate) fn sync_after_restore(&mut self, ctx: &egui::Context) {
        if self.window_visible {
            return;
        }
        self.window_visible = true;
        self.sync_tray_resident_media_wake();
        let restored_viewports = self.sync_retained_viewport_visibility_for_tray(ctx, true);
        crate::set_ui_heartbeat_suspended(
            false,
            "App::update heartbeat resumed after tray restore".to_string(),
        );
        if let Some(mgr) = self.indexer_manager.as_ref() {
            mgr.set_io_throttled(false);
        }
        self.activity_gate.set_paused(false);
        self.apply_smart_folder_editor_deferred_commit_after_restore();
        self.update_tray_tooltip();
        // 外部 (ComfyUI 等) がトレイ常駐中に current_folder へファイルを追加していたら
        // 自動で反映する。stat 1 回の軽量チェックで、変化が無ければ no-op。
        self.check_external_folder_changes();
        #[cfg(windows)]
        let restored_presenters = self.sync_media_presenter_visibility_for_tray(true);
        #[cfg(not(windows))]
        let restored_presenters = 0;
        // Transport state was never changed. Restore the retained surface and let the existing
        // focus path bring it forward without synthesizing Play or recreating the session.
        self.restore_media_surface_after_tray(ctx);
        crate::logger::log(format!(
            "tray: App state synced after restore; \
             restored_viewports={restored_viewports} routed_presenters={restored_presenters}"
        ));
    }

    /// Apply the media half of the tray transition without changing playback transport.
    /// Tests call this ownership boundary directly to pin session identity and play intent.
    pub(crate) fn prepare_media_session_for_tray_residency(&mut self) -> usize {
        #[cfg(windows)]
        let routed_presenters = self.sync_media_presenter_visibility_for_tray(false);
        #[cfg(not(windows))]
        let routed_presenters = 0;
        self.save_all_video_resume_positions();
        #[cfg(windows)]
        self.save_detached_video_resume_positions_for_exit();
        // Media and pending-open sessions survive regardless of play/pause state. A plain
        // still fullscreen has no background work to preserve and follows the existing close path.
        self.release_media_session_for_tray();
        routed_presenters
    }

    /// Keep media/pending/detached ownership intact and close only a plain still fullscreen.
    fn release_media_session_for_tray(&mut self) {
        let fs_cache_has_video = self
            .fs_cache
            .values()
            .any(|entry| matches!(entry, crate::fs_animation::FsCacheEntry::Video { .. }));
        #[cfg(windows)]
        let native_video_pending = self.native_video_open_pending.is_some()
            || self.native_video_source_swap_pending.is_some()
            || self.native_video_fast_swap_pending.is_some()
            || self.video_tile_swap_pending.is_some();
        #[cfg(not(windows))]
        let native_video_pending = false;

        if should_close_fullscreen_for_tray(
            self.fullscreen_idx.is_some(),
            fs_cache_has_video,
            native_video_pending,
            self.viewer_session_is_detached_or_switching(),
        ) {
            crate::logger::log(format!(
                "tray: closing fullscreen/media session before residency \
                 fullscreen={:?} fs_video={} native_pending={}",
                self.fullscreen_idx, fs_cache_has_video, native_video_pending
            ));
            self.close_fullscreen();
        } else if fs_cache_has_video
            || native_video_pending
            || self.viewer_session_is_detached_or_switching()
        {
            crate::logger::log(format!(
                "tray: keeping media/detached session during residency \
                 fullscreen={:?} fs_video={} native_pending={}",
                self.fullscreen_idx, fs_cache_has_video, native_video_pending
            ));
        }
    }

    /// GPU テクスチャキャッシュを破棄する (VRAM 解放目的)。
    ///
    /// ウィンドウ復帰後は通常のロード経路で再取得されるので、描画には影響なし
    /// (短時間の再ロードオーバーヘッドが発生する)。
    fn release_gpu_resources(&mut self) {
        let keep_detached_viewer_alive = self.viewer_session_is_detached_or_switching();
        // グリッドサムネ: Loaded → Evicted で TextureHandle を drop。
        // 動画サムネは Windows Shell API 経由で復帰後の再 spawn 経路が無く、
        // Evicted のまま暗灰背景が固定表示されてしまう (= 「全動画黒背景」報告) ので除外。
        for (i, state) in self.thumbnails.iter_mut().enumerate() {
            if matches!(state, crate::grid_item::ThumbnailState::Loaded { .. }) {
                if matches!(
                    self.items.get(i),
                    Some(crate::grid_item::GridItem::Video(_))
                ) {
                    continue;
                }
                *state = crate::grid_item::ThumbnailState::Evicted;
            }
        }
        // The decoder/presenter keeps its own leases. Reclaim only process-wide pool entries
        // that are not currently leased, including when the active session is detached.
        #[cfg(windows)]
        if let Some(device) = self.gpu_video_device.as_ref() {
            device.release_idle_pools();
        }
        if keep_detached_viewer_alive {
            crate::logger::log(
                "tray: skipped active viewer GPU/cache release for detached viewer session",
            );
            return;
        }
        // フルスクリーン画像キャッシュ (最大サイズ源、20MP RGBA ≈ 80MB/枚)。
        // tray へ入った media session は稼働中の VideoPlayer を保持し、
        // それ以外の texture entry は従来どおり解放する。
        let active_media_entries = self
            .fs_cache
            .values()
            .filter(|entry| matches!(entry, crate::fs_animation::FsCacheEntry::Video { .. }))
            .count();
        if active_media_entries == 0 {
            self.fs_cache.clear();
        } else {
            self.fs_cache.retain(|_, entry| {
                matches!(entry, crate::fs_animation::FsCacheEntry::Video { .. })
            });
            crate::logger::log(format!(
                "tray: retained {active_media_entries} active media player(s) while releasing GPU caches"
            ));
        }
        self.ai_upscale_cache.clear();
        self.adjustment_cache.clear();
        // 補正済みサムネテクスチャ
        self.thumb_adjust_tex.clear();
        // アップロード待ち (CPU 側 ColorImage データ)
        self.texture_backlog.clear();
        self.fs_upload_backlog.clear();
        // 単発テクスチャ各種 (None にして drop)
        self.erase_mask_texture = None;
        self.fs_checker_texture = None;
        self.analysis_sv_cache = None;
        // fs_pending の worker は cancel せず、結果が届いても fs_cache に
        // 入らず破棄されるだけ (= 少量の無駄 CPU だが整合性は保てる)。
    }

    /// トレイのツールチップを現在の状態で更新する。
    fn update_tray_tooltip(&self) {
        let Some(tc) = &self.tray_controller else {
            return;
        };
        let tooltip = if self.activity_gate.is_paused() {
            "mImageViewer — インデックス一時停止中".to_string()
        } else if !self.window_visible {
            "mImageViewer — タスクトレイ常駐中".to_string()
        } else {
            "mImageViewer".to_string()
        };
        tc.set_tooltip(tooltip);
    }

    /// 「常駐時のスキャンを一時停止」のトレイ checkmark を設定値で同期する。
    /// ダイアログ (お気に入り編集 / 環境設定) からの設定変更時、およびトレイ起動時に呼ぶ。
    pub(crate) fn sync_tray_pause_check(&self) {
        if let Some(tc) = &self.tray_controller {
            tc.set_paused_check(self.settings.pause_indexer_while_minimized);
        }
    }

    /// トレイ pause checkmark の atomic 状態を読み、設定値と differ なら設定を更新する。
    /// `TogglePauseRequested` イベントが bounded channel overflow で drop されたケースの
    /// safety net (Codex P3)。events drain 後に必ず呼ぶ。
    fn reconcile_pause_state(&mut self) {
        let Some(tc) = &self.tray_controller else {
            return;
        };
        let atomic = tc.pause_checked_snapshot();
        if atomic != self.settings.pause_indexer_while_minimized {
            self.settings.pause_indexer_while_minimized = atomic;
            self.settings.save();
            if self.window_visible {
                // ウィンドウ表示中は「実行状態は止めない」の不変を保つ。設定は次回 minimize で効く。
                self.activity_gate.set_paused(false);
            }
            self.update_tray_tooltip();
        }
    }

    /// 「タスクトレイに常駐」にチェックを入れた瞬間に表示する案内ダイアログ。
    /// ユーザーが OK を押すと閉じる。`App::update` から毎フレーム呼ぶ。
    pub(crate) fn show_tray_enabled_notice_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_tray_enabled_notice {
            return;
        }
        let mut open = true;
        let mut close_req = false;
        let dialog_pos = ctx.content_rect().min + egui::vec2(80.0, 60.0);
        egui::Window::new("タスクトレイ常駐を有効にしました")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(460.0);
                ui.label(
                    egui::RichText::new(
                        "以降、ウィンドウの [×] ボタンを押すとアプリは終了せず、\n\
                         タスクトレイ (通知領域) に常駐します。\n\n\
                         常駐中もファイル監視は続くため、次回ウィンドウを開いた\n\
                         ときに溜まっていた変更が自動で反映されます。\n\n\
                         完全に終了するには、タスクトレイのアイコンを右クリックして\n\
                         「終了」を選んでください。",
                    )
                    .size(12.0),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        close_req = true;
                    }
                });
            });
        if close_req || !open {
            self.show_tray_enabled_notice = false;
        }
    }

    /// タスクトレイのメニュー / クリックイベントを処理する。
    /// `App::update` から毎フレーム呼ぶ。
    ///
    /// トレイスレッド側が先に Win32 操作 (`ShowWindow` / `PostMessage(WM_CLOSE)` 等) と
    /// 共有 Arc 経由の状態反転 (`activity_gate` / `io_sem`) を済ませているので、
    /// ここでは App 側の状態同期と設定永続化だけ行う。
    pub(crate) fn poll_tray_events(&mut self, ctx: &egui::Context) {
        // borrow 分離: 先にイベントを drain してから self のメソッドを呼ぶ。
        let events: Vec<TrayEvent> = {
            let Some(tc) = self.tray_controller.as_ref() else {
                return;
            };
            let mut buf = Vec::new();
            while let Some(ev) = tc.try_recv() {
                buf.push(ev);
            }
            buf
        };
        for ev in events {
            match ev {
                TrayEvent::OpenRequested => {
                    self.sync_after_restore(ctx);
                }
                TrayEvent::TogglePauseRequested { .. } => {
                    // 設定反映・保存・tooltip 更新は下の reconcile で一括処理する
                    // (bounded channel overflow でこのイベントが drop されても、atomic
                    //  snapshot 経由で同じ状態に収束する)。
                }
                TrayEvent::QuitRequested => {
                    // 可視状態で Quit が押されたケース: トレイスレッドが既に
                    // PostMessage(WM_CLOSE) 済み。`maybe_intercept_close` が
                    // tc.is_quit_requested() を見て close を通すのでここは何もしない。
                    // hidden 状態でも DWM cloak + SW_SHOWNOACTIVATE で通常 close を通す。
                }
            }
        }
        // イベント drop を考慮し、必ず atomic から最新状態を reconcile する (Codex P3)。
        self.reconcile_pause_state();
    }
}

#[cfg(test)]
mod tests {
    use super::should_close_fullscreen_for_tray;

    #[test]
    fn tray_residency_closes_plain_still_fullscreen_sessions() {
        assert!(should_close_fullscreen_for_tray(true, false, false, false));
    }

    #[test]
    fn tray_residency_keeps_media_resources_without_fullscreen_flag() {
        assert!(!should_close_fullscreen_for_tray(false, true, false, false));
        assert!(!should_close_fullscreen_for_tray(false, false, true, false));
        assert!(!should_close_fullscreen_for_tray(true, true, false, false));
        assert!(!should_close_fullscreen_for_tray(true, false, true, false));
    }

    #[test]
    fn tray_residency_leaves_plain_grid_sessions_open() {
        assert!(!should_close_fullscreen_for_tray(
            false, false, false, false
        ));
    }

    #[test]
    fn tray_residency_keeps_detached_viewer_sessions_open() {
        assert!(!should_close_fullscreen_for_tray(true, false, false, true));
        assert!(!should_close_fullscreen_for_tray(false, true, false, true));
        assert!(!should_close_fullscreen_for_tray(false, false, true, true));
    }
}
