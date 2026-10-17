//! Ordered App phase fragments for the egui 0.33.3 update driver.
//!
//! The driver deliberately interleaves these calls: moving all service work to
//! the beginning would change modal gates and same-pass worker adoption.
//! Services never consume UI events; input/render and presentation ACKs stay
//! with their viewport. See docs/egui-0.37-stage1b-verification.md for ownership
//! and the future logic/ui connections. No scheduling or mount owner is added.
use super::*;

/// Navigation values captured during this root UI pass; never retained by App.
pub(super) struct MainFrameNavigation {
    pub(super) fav_nav: Option<PathBuf>,
    pub(super) toolbar_fav_nav: Option<PathBuf>,
    pub(super) fullscreen_close_nav: Option<crate::ui_main::AddressBarNav>,
    pub(super) keyboard_nav: Option<crate::ui_main::AddressBarNav>,
    pub(super) keyboard_nav_from_fullscreen_close: bool,
    pub(super) gamepad_nav: Option<crate::ui_main::AddressBarNav>,
    pub(super) address_nav: Option<crate::ui_main::AddressBarNav>,
    pub(super) open_folder_nav: Option<PathBuf>,
    pub(super) context_nav: Option<crate::ui_dialogs::context_menu::ContextMenuAction>,
    pub(super) folder_pane_nav: Option<PathBuf>,
    pub(super) grid_nav: Option<crate::ui_main::AddressBarNav>,
}

impl App {
    // Worker adoption can enter a UI-bound presentation continuation; see the ledger.
    pub(super) fn render_update_begin(&mut self, _ctx: &egui::Context) {
        #[cfg(test)]
        super::egui037_connection_tests::observe("update_begin", self, _ctx);
        self.acknowledge_tray_resident_media_wake();
        crate::record_ui_heartbeat_tick();
        // Remote service runs in logic. Display continuation stays at the old
        // root UI checkpoint before ordinary input and worker adoption.
        self.continue_remote_local_display();
        // Before any shortcut path, so a state that swallows every key says so once, in the
        // ordinary log, whether or not the reader happened to be running with --perf-log.
        self.note_modal_block_transition();
    }

    pub(super) fn input_update_begin(&mut self, ctx: &egui::Context) {
        self.consume_input_during_sidecar_restore(ctx);
        // Select and cache the root viewport owner before any shortcut path.
        // This also ages a root PendingFocus claim on every pass, including
        // passes that return before the normal keyboard handler.
        let _keyboard_owner = self.keyboard_owner_for_pass(ctx);
    }

    #[cfg(all(windows, feature = "test-script"))]
    pub(super) fn input_test_script_actions(&mut self, ctx: &egui::Context) {
        #[cfg(all(windows, feature = "test-script"))]
        {
            let snapshot = self.test_script_snapshot(ctx);
            if crate::test_script::ui_update(ctx, snapshot) {
                if crate::test_script::release_checks::take_quit_after_finish() {
                    if let Some(tray) = self.tray_controller.as_ref() {
                        if let Err(error) = tray.quit_for_smoke() {
                            crate::logger::log(format!(
                                "[release-checks] environment: normal tray Quit failed: {error}"
                            ));
                        }
                    }
                } else {
                    ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
                }
            }
            self.poll_release_check_commands(ctx);
            if crate::test_script::take_smoke_action(
                crate::test_script::UiSmokeAction::OpenThumbnailPreferences,
            ) {
                self.open_preferences_page(
                    crate::ui_dialogs::preferences::PreferencesPage::Thumbnail,
                );
            }
            for (action, enabled) in [
                (crate::test_script::UiSmokeAction::ClipboardMonitorsOn, true),
                (
                    crate::test_script::UiSmokeAction::ClipboardMonitorsOff,
                    false,
                ),
            ] {
                if crate::test_script::take_smoke_action(action) {
                    self.settings.clipboard_capture_image_enabled = enabled;
                    self.settings.clipboard_capture_html_enabled = enabled;
                    ctx.request_repaint();
                }
            }
            if crate::test_script::take_smoke_action(
                crate::test_script::UiSmokeAction::OpenFirstSmartFolder,
            ) {
                if let Some(id) = self.settings.smart_folders.first().map(|folder| folder.id) {
                    self.open_smart_folder_staged(id, false);
                }
            }
            if crate::test_script::take_smoke_action(
                crate::test_script::UiSmokeAction::OpenSeededCollection,
            ) {
                if let Err(error) = self.open_seeded_collection_for_smoke() {
                    crate::logger::log(format!(
                        "[test-script] seeded Collection open rejected: {error}"
                    ));
                }
            }
            if crate::test_script::take_smoke_action(
                crate::test_script::UiSmokeAction::OpenRatingOne,
            ) {
                self.enter_rating_view_from_menu(1);
            }
            if crate::test_script::take_smoke_action(
                crate::test_script::UiSmokeAction::AlwaysOnTopOn,
            ) {
                self.set_always_on_top(ctx, true, ActionSurface::MainWindow);
            }
            if crate::test_script::take_smoke_action(
                crate::test_script::UiSmokeAction::AlwaysOnTopOff,
            ) {
                self.set_always_on_top(ctx, false, ActionSurface::MainWindow);
            }
            if crate::test_script::take_smoke_action(
                crate::test_script::UiSmokeAction::MinimizeRoot,
            ) {
                ctx.send_viewport_cmd_to(
                    egui::ViewportId::ROOT,
                    egui::ViewportCommand::Minimized(true),
                );
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
            if crate::test_script::take_smoke_action(crate::test_script::UiSmokeAction::RestoreRoot)
            {
                ctx.send_viewport_cmd_to(
                    egui::ViewportId::ROOT,
                    egui::ViewportCommand::Minimized(false),
                );
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
            if crate::test_script::take_smoke_action(crate::test_script::UiSmokeAction::HideToTray)
            {
                self.hide_to_tray(ctx);
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
            if crate::test_script::take_smoke_action(
                crate::test_script::UiSmokeAction::RestoreFromTray,
            ) {
                if let Some(controller) = self.tray_controller.as_ref() {
                    if let Err(error) = controller.open_for_smoke() {
                        crate::logger::log(format!("[test-script] {error}"));
                    }
                } else {
                    crate::logger::log(
                        "[test-script] tray restore requested without a tray controller",
                    );
                }
            }
            if crate::test_script::take_smoke_action(
                crate::test_script::UiSmokeAction::CloseFullscreen,
            ) {
                self.close_fullscreen_to_completion(ctx);
            }
            if crate::test_script::take_smoke_action(
                crate::test_script::UiSmokeAction::ToggleDetachedMode,
            ) {
                self.toggle_detached_viewer_mode();
            }
            if crate::test_script::take_smoke_action(
                crate::test_script::UiSmokeAction::EnableIndependentWindows,
            ) {
                self.settings.detached_viewer_open_images_in_window = true;
            }
        }
    }

    pub(super) fn service_creative_luts(&mut self, ctx: &egui::Context) {
        self.edit_preview_repaint_ctx = Some(ctx.clone());
        if self
            .creative_lut_library
            .poll(&self.settings.creative_luts, ctx)
        {
            self.clear_all_final_pipeline_caches();
            #[cfg(windows)]
            self.sync_native_video_grade();
        }
    }

    pub(super) fn service_edit_completions(
        &mut self,
        ctx: &egui::Context,
        now: std::time::Instant,
    ) {
        let had_deferred_local_adjust_brush = self.local_adjust_brush_deferred_render.is_some();
        if let Some(delay) = self.poll_deferred_local_adjust_brush_render_at(now) {
            ctx.request_repaint_after(delay);
        } else if had_deferred_local_adjust_brush {
            ctx.request_repaint();
        }
        // 補正レイヤーの保存結果とキー編集セッションは、**グリッド描画より前**で回す。
        // in-window フルスクリーンは下の `embedded_fs_active` gate で無条件に return
        // するので、そこより後ろに置くと編集中は一度も走らない (2026-08-31 Codex P1)。
        // 補正の編集はまさにフルスクリーン中に行われる。
        self.poll_local_adjust_write_results();
        if !self.local_adjust_write_pending.is_empty() {
            // 最後の保存の直後に egui が眠ると、失敗トーストとサイドカーミラーが
            // 次の入力まで出ない。
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if self.settle_local_adjust_shape_key_edit() {
            // セッションが開いている間はフレームを回す。眠ると無操作判定が来ない。
            ctx.request_repaint_after(Self::LOCAL_ADJUST_SHAPE_KEY_IDLE);
        }
    }

    pub(super) fn service_capture_main_window(
        &mut self,
        ctx: &egui::Context,
        frame: &mut eframe::Frame,
    ) {
        // メインウィンドウの HWND を最初のフレームで取得 (Win32 ShowWindow 用)。
        // eframe::Frame::window_handle() は raw_window_handle::WindowHandle を返す。
        // Windows では Win32WindowHandle の hwnd フィールドに HWND が入る。
        #[cfg(windows)]
        if self.main_hwnd.is_none() {
            use eframe::wgpu::rwh::{HasWindowHandle, RawWindowHandle};
            if let Ok(wh) = frame.window_handle() {
                if let RawWindowHandle::Win32(h) = wh.as_raw() {
                    let hwnd_raw = h.hwnd.get();
                    self.main_hwnd = Some(hwnd_raw);
                    crate::startup_windows_diag::mark(
                        "app.tray_main_hwnd.capture",
                        hwnd_raw as usize,
                        || serde_json::json!({}),
                    );
                    crate::presentation_observer::register(
                        crate::presentation_observer::WindowRole::Main,
                        hwnd_raw as u64,
                    );
                    // マウスドライバが WM_APPCOMMAND_BROWSER_BACKWARD/FORWARD で送るか、
                    // AHK 等が VK_BROWSER_BACK/FORWARD keystroke で送るルートを WH_GETMESSAGE
                    // フックで拾い、進む/戻るボタンを Ctrl+↑/↓ と等価に扱う。詳細は
                    // main.rs の `install_mouse_nav_hook` のコメント参照。
                    crate::install_mouse_nav_hook();
                    crate::key_input::install_main_window_subclass(hwnd_raw as u64);
                    self.effetune.set_main_hwnd(hwnd_raw as u64);
                    #[cfg(windows)]
                    self.dsp_bridge.set_main_hwnd(hwnd_raw as u64);
                    #[cfg(windows)]
                    if let Some(bridge) = self.effetune.bridge() {
                        bridge.set_main_hwnd(hwnd_raw as u64);
                    }
                    // watchdog に HWND を共有 (IsHungAppWindow チェック用、
                    // intentional idle と actual hang を区別するため)。
                    crate::set_ui_heartbeat_main_hwnd(hwnd_raw as u64);
                    crate::logger::log(format!("tray: captured main HWND = {hwnd_raw:#x}"));
                    // アクティベーションリスナーに placement_slot を共有するため、
                    // ここでスロットを作成しておく (sync_tray_with_settings での遅延作成と
                    // 競合しないよう、どちらも is_none チェック付き)。
                    if self.placement_slot.is_none() {
                        self.placement_slot = Some(crate::tray::new_placement_slot());
                    }
                    let slot = self.placement_slot.clone().unwrap();
                    // 2 重起動時に既存インスタンスを復帰させるリスナーを起動。
                    // placement_slot もトレイ Open と同じ経路で SetWindowPlacement するため共有。
                    self.activation_listener = crate::single_instance::spawn_activation_listener(
                        hwnd_raw,
                        ctx.clone(),
                        slot,
                        self.shutdown_requested.clone(),
                        self.activation_open_path_tx.clone(),
                        self.remote_session_handle().cloned(),
                    );
                }
            }
        }
    }

    pub(super) fn service_tray_requests(&mut self, ctx: &egui::Context) {
        // 設定変更反映 + メニューイベントをポーリング + 閉じるボタンの乗っ取り。
        self.sync_tray_with_settings(ctx);
        self.poll_tray_events(ctx);
    }

    pub(super) fn input_main_focus_resume(&mut self, ctx: &egui::Context) {
        // フォーカス復帰検出 (Alt+Tab で他アプリから mIV に戻った等) で、
        // 外部 (ComfyUI 等) による current_folder のファイル追加を自動反映する。
        // トレイ復帰は `sync_after_restore` 側で別経路で呼ぶので、tray_active に限らず
        // 全ユーザで動かす。
        let main_focused_now = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        if main_focused_now && !self.last_main_focused && self.window_visible {
            self.check_external_folder_changes(ctx, ExternalChangeCheck::Resumed);
        }
        self.last_main_focused = main_focused_now;
    }

    pub(super) fn service_folder_watch(&mut self, ctx: &egui::Context) {
        self.poll_current_folder_watch(ctx);
    }

    pub(super) fn service_periodic_folder_work(&mut self, ctx: &egui::Context) {
        self.poll_association_handler_prewarm(ctx);

        // dirty のまま一定時間が経ったサイドカーを書き出す (電源断や強制終了への保険)。
        // 頻繁なフレームで呼ばれるが is_dirty 判定で大半は no-op になる。
        self.flush_periodic_sidecars();
    }

    pub(super) fn service_startup_notifications(&mut self, ctx: &egui::Context) {
        self.poll_indexer_notifications(
            ctx.input(|input| input.viewport().minimized.unwrap_or(false)),
        );
        self.poll_housekeeping_arm();
        // VST3 起動時の bridge enable + 自動ロードは専用 worker で走らせる。
        // 画像閲覧だけの起動では通常 UI を先に出し、動画を開いた時点でまだロード中なら
        // フルスクリーン側で待機表示にする。
        // VST3 プラグイン GUI の TOPMOST 切替 + フルスクリーン解除時の cleanup。
        // - 遷移時 (= フルスクリーン入退) のみ操作 (毎フレーム呼ぶと余計な SetWindowPos
        //   が発生してプラグイン GUI のフォーカスを乱す)。
        // - **フルスクリーン解除**: 動画閲覧をやめたら全プラグイン GUI を隠して
        //   管理パネルも閉じる (= ユーザー要望「動画再生中のみの表示」を満たす)。
        //   ついでに動画 compact mode (右上 1/4) も自動解除 (= 次に動画見るときの
        //   既定をフルサイズに戻す)。
        #[cfg(windows)]
        {
            let is_fs = self.fullscreen_idx.is_some();
            if is_fs != self.vst3_was_fullscreen {
                self.dsp_bridge.set_all_guis_topmost(is_fs);
                if !is_fs {
                    // フルスクリーン解除 → VST 関連 UI を全部畳む。
                    // 動画 compact の設定値は保持し、次に VST を開いた時に復元する。
                    self.native_video_owner_synced_hwnd = 0;
                    self.dsp_bridge.set_existing_guis_owner_to_main();
                    self.dsp_bridge.set_all_guis_visible(false);
                    self.show_vst3_manager = false;
                    if self.settings.vst3_gui_visible {
                        self.settings.vst3_gui_visible = false;
                        self.settings.save();
                    }
                }
                self.vst3_was_fullscreen = is_fs;
            }
        }
    }

    // Worker adoption can enter a UI-bound presentation continuation; see the ledger.
    pub(super) fn render_startup_workers(&mut self, ctx: &egui::Context) {
        #[cfg(test)]
        super::egui037_connection_tests::observe("startup_before", self, ctx);
        #[cfg(windows)]
        self.poll_vst3_startup_load(ctx);
        #[cfg(windows)]
        self.poll_effetune(ctx);
        self.poll_play_test(ctx);

        // バージョン更新通知: 起動完了後の初回 + 24h 周期で auto kick。
        self.maybe_auto_update_check();
        self.poll_update_check();
        // worker の結果を取りこぼさないよう、pending 中はアイドルでも repaint を要求。
        // 通常 update() はマウス操作等が無いと走らないため (Codex P2)。
        if self.update_check_pending.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        #[cfg(test)]
        super::egui037_connection_tests::observe("startup_after", self, ctx);
    }

    pub(super) fn input_pointer_and_ime(&mut self, ctx: &egui::Context) {
        // 直前フレームで native ファイル D&D を実行した場合、egui のポインタ状態を
        // リセットする。SHDoDragDrop はドラッグ終了の WM_LBUTTONUP を内部消費し
        // winit に届けないため、放置すると egui が「左ボタン押下中」と誤認して
        // 幽霊ドラッグが起きる (docs/file-drag-drop-design.md §6.1)。
        // フラグ自体はこのフレーム末尾で消費する (= ドラッグ検出も 1 フレーム抑止)。
        if self.native_drag_just_finished {
            ctx.input_mut(|i| i.pointer = egui::PointerState::default());
            // PointerState は raw 入力状態のみ。egui の interaction 層
            // (dragged_id / potential_drag_id) は別管理なので明示的に止める。
            ctx.stop_dragging();
        }

        // メインビューポートの IME 状態を更新 (ここで Ime イベントを拾う)。
        // フルスクリーンビューポートは別イベントキューなので render_fullscreen_viewport 内で別途呼ぶ。
        self.update_ime_state(ctx);
    }

    pub(super) fn service_page_edit_completion(&mut self, _ctx: &egui::Context) {
        // 編集ツールを抜けたのに見開きが戻っていない状態を、毎フレーム 1 か所で解消する。
        self.reconcile_page_edit_spread_pivots();
        self.flush_sidecars_when_page_edit_ends();
    }

    pub(super) fn input_drop_and_activity(&mut self, ctx: &egui::Context) {
        // エクスプローラ等から mIV へドロップされたファイル / フォルダを現在のフォルダへコピーする
        // (mIV → 外部 のドラッグ送出と対称の受け取り方向)。毎フレーム走るので、
        // ドロップが無い通常フレームでは空チェックだけで抜ける (Vec を確保しない)。
        if ctx.input(|i| !i.raw.dropped_files.is_empty()) {
            if self.sidecar_restore_active() {
                let count = ctx.input(|i| i.raw.dropped_files.len());
                self.internal_drop_pending = false;
                crate::logger::log(format!(
                    "file_drop: discarded {count} path(s) during sidecar restore"
                ));
            // mIV 自身のドラッグが mIV ウィンドウ上へ落ちた跳ね返り (グリッド上での
            // クリックずれ等) なら受け取らない。フラグの消費は dropped_files が来た
            // 時点で必ず行う — パスが空に潰れても stale で残さない (Codex P2)。
            } else if std::mem::take(&mut self.internal_drop_pending) {
                crate::logger::log("file_drop: ignored (mIV-internal drag bounce-back)");
            } else {
                let dropped_paths: Vec<PathBuf> = ctx.input(|i| {
                    i.raw
                        .dropped_files
                        .iter()
                        .map(|f| f.path().to_path_buf())
                        .collect()
                });
                self.handle_external_file_drop(dropped_paths);
            }
        }

        // 入力があればバックグラウンドインデクサを一時停止させる。
        // `keys_down` は「今押されている」セットなので、Ctrl や Shift を指で押しっぱなしに
        // したまま読んでいると毎フレーム true になり、indexer が永久に再開できなくなる
        // (ユーザー報告される前に /simplify で発見、2026-04)。代わりに「今フレームで
        // 発生したイベント」だけを見る。マウス移動・フォーカス変化・Ime 等は除外。
        let has_activity = ctx.input(|i| {
            i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::PointerButton { .. }
                        | egui::Event::MouseWheel { .. }
                        | egui::Event::Touch { .. }
                )
            })
        });
        if has_activity {
            self.activity_gate.bump();
        }
    }

    pub(super) fn render_apply_theme(&mut self, ctx: &egui::Context) {
        // UI テーマを適用 (変化したときだけ set_visuals を呼ぶ)。
        // `UiTheme::System` 選択時は設定値が変わらなくても Windows の Light/Dark
        // 切替に追従する必要があるので、毎フレーム resolve して解決後の値で比較する。
        let resolved_theme = crate::os_theme::resolve(self.settings.ui_theme);
        let text_contrast = self.settings.text_contrast;
        if self.applied_ui_theme != Some(resolved_theme)
            || self.applied_text_contrast != Some(text_contrast)
        {
            crate::os_theme::apply_resolved_with_contrast(ctx, resolved_theme, text_contrast);
            self.applied_ui_theme = Some(resolved_theme);
            self.applied_text_contrast = Some(text_contrast);
            #[cfg(windows)]
            if !self.native_video_main_chrome_black
                && let Some(hwnd_raw) = self.main_hwnd
            {
                crate::dwm_transitions::restore_window_chrome_for_theme(
                    windows::Win32::Foundation::HWND(hwnd_raw as *mut _),
                    matches!(resolved_theme, crate::os_theme::ResolvedTheme::Dark),
                );
            }
        }
    }

    // Worker adoption can enter a UI-bound presentation continuation; see the ledger.
    pub(super) fn render_initial_open(&mut self, ctx: &egui::Context) {
        #[cfg(test)]
        super::egui037_connection_tests::observe("initial_open", self, ctx);
        // 初回フレームで設定された開始フォルダを開く。起動引数の explicit path は
        // UI スレッド外で解決し、前回フォルダ / Desktop / 指定フォルダの優先順位と
        // fallback は known_folders::startup_folder に集約する。
        if !self.initialized {
            self.initialized = true;

            // テーマは Settings::ui_theme が `System` であれば OS 設定に追従し、
            // 明示的な Light / Dark 選択はそのまま使う。初回セットアップダイアログでは
            // テーマ / AI 利用範囲 / ZIP/PDF の開き方をまとめて確認する。

            if self.startup_open_path.is_some() {
                self.poll_deferred_startup_open_path(ctx);
            } else {
                self.open_default_startup_target();
            }

            // AI ランタイムを初期化
            self.ensure_ai_runtime();
        }

        self.poll_deferred_startup_open_path(ctx);

        #[cfg(windows)]
        self.poll_activation_open_paths(ctx);

        self.poll_startup_open_path_resolve(ctx);
    }

    pub(super) fn service_initial_worker_results(
        &mut self,
        ctx: &egui::Context,
        update_perf: &mut Option<UpdatePerfRecorder>,
    ) {
        #[cfg(test)]
        super::egui037_connection_tests::observe("initial_workers", self, ctx);
        self.poll_edit_preview_cache(ctx);
        self.poll_content_identity_detection(ctx);
        self.poll_content_identity_restore(ctx);
        self.reconcile_pinned_adjustment_refreshes();
        self.poll_smart_folder_metadata_refresh(ctx);
        mark_update_perf(update_perf, UpdatePerfStage::InitialWorkerPolls);
    }

    pub(super) fn render_media_and_edit_results(&mut self, ctx: &egui::Context) {
        #[cfg(test)]
        super::egui037_connection_tests::observe("media_before", self, ctx);
        self.poll_main_video_context(ctx);
        self.poll_ai_upscale(ctx);
        self.poll_final_ai(ctx);
        self.poll_erase_inpaint(ctx);
        self.poll_local_adjust_render(ctx);
        self.poll_local_adjust_layer_bypass_preview(ctx);
        self.poll_local_adjust_prefix_preview(ctx);
        self.poll_local_adjust_lut_load(ctx);
        self.poll_local_adjust_segmentation(ctx);
        #[cfg(test)]
        super::egui037_connection_tests::observe("media_after", self, ctx);
    }

    pub(super) fn render_search_and_metadata_results(&mut self, ctx: &egui::Context) {
        #[cfg(test)]
        super::egui037_connection_tests::observe("search_before", self, ctx);
        self.poll_search(ctx);
        self.poll_facet_name_debounce(ctx);
        self.poll_facet_name_cache(ctx);
        self.poll_color_scan(ctx);
        self.poll_favsearch();
        self.poll_tag_view();
        self.poll_rating_view();
        self.poll_book_bookmarks();
        self.poll_bookmark_presence(ctx);
        self.poll_bookmark_browser(ctx);
        if self.book_bookmark_projections_pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.poll_metadata_load();
        #[cfg(test)]
        super::egui037_connection_tests::observe("search_after", self, ctx);
    }

    // Worker adoption can enter a UI-bound presentation continuation; see the ledger.
    pub(super) fn render_other_worker_results(&mut self, ctx: &egui::Context) {
        #[cfg(test)]
        super::egui037_connection_tests::observe("other_workers", self, ctx);
        // スタックスクリプトは items generation を差し替える。復旧中は receiver を保持し、
        // terminal 後に既存 poll へ一度だけ戻す。
        if !self.sidecar_restore_active() {
            self.poll_stack_script(ctx);
        }
        self.poll_subfolder_expansion(ctx);
        self.poll_smart_folder(ctx);
        self.poll_details_meta_load(ctx);
        // 360 度パノラマビュー Phase 2a (docs/panorama-360-view-plan.md §4.6.3):
        // 1. NeedsUserConfirmation → SettleApproved 経路の追加 worker 結果取り込み
        // 2. settle 静止検出 + render spawn + 結果ポーリング
        self.poll_pano_high_res(ctx);
        self.update_pano_refinement(ctx);
        self.poll_tag_prewarm_results();
        self.poll_book_resume_meters(ctx);
        self.poll_delete_pending();
        self.poll_batch_convert();
        self.poll_epub_batch_convert();
        self.poll_file_drop_pending();
        self.poll_external_tool_launch(ctx);
        self.poll_new_folder_pending(ctx);
        self.poll_rename_pending(ctx);
        self.poll_file_organize(ctx);
        self.poll_rename_migration_pending(ctx);
        self.poll_delete_purge_retry(ctx);
        self.poll_capture_pending(ctx);
        self.poll_book_op_pending(ctx);
        self.poll_saved_group_open(ctx);
        self.poll_pipeline_debug_export_pending(ctx);
        self.poll_export_pending(ctx);
        self.poll_compare_pin_load_pending(ctx);
        self.poll_compare_pin_pending(ctx);
        self.poll_compare_prepare_pending(ctx);
        if !self.drop_copy_pending.is_empty()
            || self.new_folder_pending.is_some()
            || self.rename_pending.is_some()
        {
            ctx.request_repaint();
        }
        if self.capture_pending.is_some()
            || self.book_op_pending.is_some()
            || self.pipeline_debug_export_pending.is_some()
            || self.export_pending.is_some()
            || self.compare_pin_load_pending.is_some()
            || self.compare_pin_pending.is_some()
            || (self.compare_preparation.is_pending()
                && !matches!(self.compare_view_mode, CompareViewMode::Off))
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.poll_cache_maint_pending();
        self.poll_reading_history_thumbnail_sources(ctx);
        if let Some(detail) =
            crate::catalog::CatalogAccess::for_cache_dir(&crate::catalog::default_cache_dir())
                .take_audio_art_notice()
        {
            crate::logger::log(format!("audio art cache notice: {detail}"));
            self.show_feedback_toast(
                "音声画像のキャッシュを保存・参照できませんでした。次回に再生成します".to_owned(),
            );
        }
        self.poll_metadata_cleanup(ctx);
        self.poll_archive_cache_maint_pending();
        self.poll_epub_cache_maint_pending();
        self.poll_converted_archive_cache_paths(ctx);
        self.ensure_folder_rating_counter();
        self.poll_folder_rating_counts();
        // Ctrl+G (docs §10.4): debounce 後に spawn、streaming 受信 → items 更新
        self.poll_global_search_debounce(ctx);
        self.poll_global_search_events(ctx);
        if self.global_search.is_searching() {
            // Ctrl+G の検索そのものもインタラクティブ操作として扱う。
            // 入力イベントの bump だけだと DEFAULT_QUIET_MS 経過後にインデクサが
            // 再開し、Tantivy/post-filter の初回結果と I/O/CPU が競合することがある。
            // pending 中は毎フレーム bump して、検索完了後に自然再開させる。
            self.activity_gate.bump();
        }
        if self.tag_view_pending.is_some()
            || self.rating_view_pending.is_some()
            || self
                .top_level_grid_view
                .history_navigation_transition()
                .is_some()
        {
            self.activity_gate.bump();
        }
        if self.rating_view_pending.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // review #9 対応: ensure_container_mtime_populated が worker thread に
        // 投げた mtime 取得結果を drain する。完了で rebuild が走る。
        self.poll_container_mtime_pending(ctx);
        // 非同期 pending が走っている間は次フレームも poll させる (egui アイドル寝防止)。
        // tag_prewarm_pending は常駐 handle になったので `is_some()` ではなく
        // 実ジョブ残数 (is_busy) を見る。アイドル時の無限 repaint を避けるため。
        if self.search_pending.is_some()
            || self.favsearch_pending.is_some()
            || self.tag_view_pending.is_some()
            || self.metadata_pending.is_some()
            || self.details_meta_pending.is_some()
            || !self.local_adjust_pending.is_empty()
            || self.local_adjust_layer_bypass_pending.is_some()
            || self.local_adjust_prefix_preview_pending.is_some()
            || self.local_adjust_segmentation_pending.is_some()
            || !self.final_ai_pending.is_empty()
            || !self.retained_final_ai_orphans.is_empty()
            || self
                .tag_prewarm_pending
                .as_ref()
                .is_some_and(|p| p.is_busy())
            || self.converted_archive_cache_paths_pending.is_some()
            || (self.folder_rating_counter_handle.is_some() && !self.folder_rating_counts_loaded)
        {
            ctx.request_repaint();
        }
    }

    pub(super) fn render_main_title(&mut self, ctx: &egui::Context) {
        // タイトルバーに現在のフォルダパスを表示する。
        // フォルダ未選択時や読み込み途中はアプリ名のみ。
        // 変換済みアーカイブを開いているときは元 (RAR/7z/LZH) のパスを表示する。
        //
        // 名前索引 / メタ索引の supervisor が `in_full_scan=true` を示している間は
        // 「(インデックス更新中)」をサフィックスに付け、検索結果が不完全である
        // 可能性をユーザーに示唆する (notify-rs の watcher 待ちだけなら付けない)。
        let smart_folder_name = self.current_smart_folder_id.and_then(|id| {
            self.settings
                .smart_folders
                .iter()
                .find(|definition| definition.id == id)
                .map(|definition| definition.name.as_str())
        });
        let title = main_window_title(
            self.effective_folder().as_deref(),
            self.items_are_reading_history_view,
            self.items_are_bookmark_view,
            self.items_are_subfolder_expansion_view,
            self.items_are_smart_folder_view
                .then_some(smart_folder_name)
                .flatten(),
            self.top_level_grid_view
                .collection_session()
                .filter(|session| {
                    matches!(
                        session.position,
                        top_level_grid_view::CollectionGridPosition::Root
                    )
                })
                .map(|session| {
                    session.prepared().map_or("読み込み中", |prepared| {
                        prepared.collection_name.as_str()
                    })
                }),
            self.any_indexer_in_full_scan(),
        );
        // タイトルが変わったときだけ送信する (2026-05-10 修正)。
        // 無条件に send_viewport_cmd(Title(...)) すると egui 内部の `request_repaint_of`
        // が毎フレーム発火し、App::update が 60fps で回り続けて他アプリ FPS を奪う。
        if self.last_window_title.as_deref() != Some(title.as_str()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_window_title = Some(title);
        }
    }

    pub(super) fn render_main_overlays(&mut self, ctx: &egui::Context) {
        // ── 進捗バー (左下フローティングオーバーレイ) ────────────────
        self.render_progress_overlay(ctx);
        self.render_subfolder_expansion_install_overlay(ctx);
        self.render_smart_folder_overlay(ctx);
        self.render_saved_group_open_modal(ctx);

        // ── PDF / ZIP コンテナ列挙待ちバッジ (左下、進捗バーの上に積む) ──
        // PDFium 開封 + 列挙の 100ms〜1.3 秒の間「親フォルダのまま動かない」状態に
        // 「読み込み中…」を出す。先読み N/M と同じ場所で進行中セマンティクスを統一。
        self.render_container_enumerate_overlay(ctx);
        self.render_collection_grid_error_overlay(ctx);
    }

    // Worker adoption can enter a UI-bound presentation continuation; see the ledger.
    pub(super) fn render_menu_worker_results(&mut self, ctx: &egui::Context) {
        // タグ書き込み worker の結果ポーリング (docs/archive/search-metadata/tag-feature.md §5.6)
        self.poll_tag_write_results();
        self.poll_tag_maintenance_results();
        self.poll_rating_write_results();
        self.poll_video_upscale_queue(ctx);
    }

    pub(super) fn render_main_dialogs_before_open(
        &mut self,
        ctx: &egui::Context,
        main_viewer_blocked: bool,
    ) {
        // ── ダイアログ群 ─────────────────────────────────────────────
        self.show_favorites_editor_dialog(ctx);
        if !main_viewer_blocked && !self.sidecar_restore_active() {
            self.draw_favorite_default_clear_confirm_dialog(ctx);
        }
        self.show_smart_folder_editor_dialog(ctx);
        self.show_tag_editor_dialog(ctx);
        self.show_tag_apply_dialog(ctx);
        self.show_content_restore_dialog(ctx);
        self.show_fav_add_dialog_window(ctx);
    }

    pub(super) fn render_main_file_dialogs(&mut self, ctx: &egui::Context) {
        self.show_subfolder_expansion_dialog_window(ctx);
        self.show_new_folder_dialog_window(ctx);
        self.show_rename_dialog_window(ctx);
        self.show_file_organize_dialog(ctx);
        self.show_rename_migration_recovery_dialog(ctx);
        self.draw_book_manager(ctx);
        self.draw_book_reorder(ctx);
        self.draw_collection_reorder(ctx);
        self.show_cache_manager_dialog(ctx);
        self.show_metadata_cleanup_dialog(ctx);
    }

    pub(super) fn render_main_conversion_dialogs(&mut self, ctx: &egui::Context) {
        self.show_metadata_transfer_dialog(ctx);
        self.show_sidecar_restore_dialog(ctx);
        self.show_archive_cache_manager_dialog(ctx);
        self.show_epub_cache_manager_dialog(ctx);
        self.show_cache_creator_dialog(ctx);
        self.show_archive_convert_dialog(ctx);
        self.show_video_upscale_dialog(ctx);
        self.show_video_upscale_tasks_window(ctx);
    }

    pub(super) fn service_thumb_quality_results(&mut self, ctx: &egui::Context) {
        self.poll_thumb_quality_pending(ctx);
        self.poll_tq_encode_pending(ctx);
    }

    pub(super) fn render_main_settings_dialogs(
        &mut self,
        ctx: &egui::Context,
        main_viewer_blocked: bool,
    ) {
        self.show_thumb_quality_dialog_window(ctx);
        self.show_thumb_quality_fullscreen_overlay(ctx);
        self.show_settings_boot_problem_dialog(ctx);
        self.show_first_setup_dialog(ctx);
        self.show_mouse_nav_migration_prompt_dialog(ctx);
        self.show_preferences_dialog(ctx);
        self.show_external_tool_modals(ctx);
        self.show_operation_customize_dialog(ctx);
        self.show_settings_restore_dialog(ctx);
        self.show_collection_manager(ctx);
        // VST3 プラグイン管理ウィンドウ + チェーンエディタ。
        // ⚠️ フルスクリーン中はフルスクリーンビューポート側で描画する (= ui_fullscreen.rs)。
        //    両方のビューポートで描画すると egui::Window の位置が二重管理になり、
        //    ビューポート切替時に位置がリセットされたように見える (ユーザー報告 2026-04)。
        // pump_gui_signals は close/resize シグナルを 1 回 drain するだけなので
        // どちらでも 1 回呼べば OK (= ここでは fullscreen 中は呼ばない)。
        #[cfg(windows)]
        if !main_viewer_blocked {
            self.show_vst3_manager(ctx);
            self.vst3_pump_gui_signals();
        }
    }

    pub(super) fn service_pdf_worker_notices(&mut self, _ctx: &egui::Context) {
        // PDF pool の遅延初期化失敗 notice。poll は pool 自体を初期化しない。
        self.poll_pdf_worker_notice();
        if crate::pdf_loader::take_render_epoch_maintenance_failure() {
            crate::logger::log("PDF render epoch maintenance worker disconnected".to_owned());
            self.show_feedback_toast_on(
                "PDFサムネイルの旧要求を整理できませんでした。再起動してください".to_owned(),
                ActionSurface::MainWindow,
            );
        }
    }

    pub(super) fn render_main_dialogs_after_context(
        &mut self,
        ctx: &egui::Context,
        main_viewer_blocked: bool,
    ) {
        self.show_delete_confirm_dialog(ctx);
        self.show_edit_bundle_paste_confirm_dialog(ctx);
        self.show_bulk_page_edit_dialog(ctx);
        self.show_delete_progress_dialog(ctx);
        self.show_batch_convert_progress_dialog(ctx);
        self.show_epub_batch_convert_dialog(ctx);
        self.show_about_dialog_window(ctx);
        self.show_update_dialog_window(ctx);
        self.show_whats_new_dialog(ctx);
        self.show_network_data_dir_notice_dialog(ctx);
        if !main_viewer_blocked {
            self.show_context_shortcuts_dialog(ctx);
        }
        self.show_toolbar_reset_confirm_dialog(ctx);
        self.show_tray_enabled_notice_dialog(ctx);
        if self.main_window_should_draw_export_dialogs() {
            self.draw_export_dialog(ctx);
            self.draw_export_batch_dialog(ctx);
            self.draw_export_progress_dialog(ctx);
        }
    }

    pub(super) fn input_main_grid_shortcuts(
        &mut self,
        ctx: &egui::Context,
        main_viewer_blocked: bool,
    ) {
        // ── F12: 画像・動画ビューア別ウィンドウモード ───────────────
        if !self.address_has_focus
            && !self.search_has_focus
            && !self.favsearch.has_focus
            && !self.global_search.has_focus
            && !self.tag_view.has_focus
            && !main_viewer_blocked
            && !self.any_dialog_open()
            && self
                .keymap
                .consume_action_no_repeat(ctx, KeyAction::ToggleAlwaysOnTop)
        {
            self.toggle_always_on_top(ctx, ActionSurface::MainWindow);
        }

        if !self.address_has_focus
            && !self.search_has_focus
            && !self.favsearch.has_focus
            && !self.global_search.has_focus
            && !self.tag_view.has_focus
            && !main_viewer_blocked
            && !self.any_dialog_open()
            && self
                .keymap
                .consume_action_no_repeat(ctx, KeyAction::ToggleDetachedViewerMode)
        {
            self.toggle_detached_viewer_mode();
        }

        // ── Ctrl+F: 検索バー表示 ─────────────────────────────────────
        // Ctrl+G (グローバルメタ検索) と相互排他 (docs §10.3):
        //   - Ctrl+G active 中は Ctrl+F を無効化する (Codex round-8 Must-fix #3)
        //     理由: 結果ビューに SearchContainer が並ぶ状態で Ctrl+F を使うと、
        //     run_metadata_search が SearchContainer を常に一致扱いするためフィルタ不能になる
        //   - Codex P2 #2: `has_focus` だけだと Ctrl+G active でグリッドにフォーカスが
        //     落ちた状態で Ctrl+F が通ってしまう → `active` も条件に入れる
        // §4.1.1: PDF のページ表示中は Ctrl+F 自体を無効化する (連番ページ名を
        // 絞っても得るものが無いため)。グリッドが PdfPage で構成されるときは
        // ショートカットを無反応にする (検索バーは render_search_bar 側で閉じる)。
        if !self.address_has_focus
            && !main_viewer_blocked
            && !self.any_dialog_open()
            && !self.favsearch.has_focus
            && !self.global_search.has_focus
            && !self.tag_view.has_focus
            && !self.global_search.active
            && !self.grid_is_pdf_pages()
        {
            let ctrl_f = self
                .keymap
                .pressed_action(ctx, KeyAction::GlobalLocalSearch);
            if ctrl_f {
                // 他の検索バーが開いていれば閉じて Ctrl+F に切り替え (相互排他)
                self.open_local_metadata_search();
            }
        }

        // ── Ctrl+A: 表示中のチェック可能なアイテムをチェック ─────────
        // Ctrl+D / Ctrl+Shift+A: 選択解除 (右クリックメニュー「選択解除」と同等)。
        // Ctrl+D を primary、Ctrl+Shift+A を alias として両方受け付ける。メニュー側の
        // ヘルプ表記は Ctrl+D に統一 (3 キー同時押しより 2 キーの方が提示しやすい)。
        // address/search にフォーカスがあるときはテキスト選択を優先する。
        if !self.address_has_focus
            && !self.search_has_focus
            && !self.favsearch.has_focus
            && !self.tag_view.has_focus
            && !main_viewer_blocked
            && !self.any_dialog_open()
            && self.grid_item_input_allowed()
        {
            let ctrl_a = self.keymap.pressed_action(ctx, KeyAction::GridSelectAll);
            let deselect = self.keymap.pressed_action(ctx, KeyAction::GridDeselect);
            if ctrl_a {
                for &idx in &self.visible_indices {
                    if self.items.get(idx).is_some_and(|it| it.is_checkable()) {
                        self.checked.insert(idx);
                    }
                }
            }
            if deselect {
                self.checked.clear();
            }
        }

        // ── P: 代表サムネ固定トグル (グリッドモード) ───────────────
        // アドレスバー 📌 ボタン左クリックと同等。pin 不能アイテム / 検索アグリゲート /
        // 変換キャッシュ drill-down / 空フォルダ等は `toggle_folder_pin_from_selection`
        // 内のガードで silent no-op になる。
        // フルスクリーン中の P は fullscreen key handler 側で現在表示中アイテムの
        // 代表サムネ固定として処理する。ここではグリッド表示中だけ反応する。
        if !self.address_has_focus
            && !self.search_has_focus
            && !self.favsearch.has_focus
            && !self.global_search.has_focus
            && !self.tag_view.has_focus
            && !main_viewer_blocked
            && !self.any_dialog_open()
            && self.grid_item_input_allowed()
        {
            let pressed_p = self.keymap.consume_action(ctx, KeyAction::GridPin);
            if pressed_p {
                self.toggle_folder_pin_from_selection();
            }
            if self
                .keymap
                .consume_action(ctx, KeyAction::GridAddToActiveBook)
            {
                self.add_grid_selection_to_active_book(ctx);
            }
            self.handle_grid_collection_add_shortcut(ctx);
            // グリッドの Ctrl+E = 選択の一括エクスポート。既定キーはフルスクリーンの
            // `FsExport` と同じだが action は別で、割り当ても別々に変えられる。
            //
            // `consume_action` は chord しか見ず `KeyContext` を強制しないので、既定の
            // まま使うとフルスクリーン中の Ctrl+E もここで拾えてしまう。Grid の action
            // であることをこの述語で明示する。
            if self.fullscreen_idx.is_none()
                && self
                    .keymap
                    .consume_action(ctx, KeyAction::GridExportSelection)
            {
                self.open_export_batch_dialog(ctx);
            }
        }

        // ── Ctrl+S: お気に入り検索バー表示 ───────────────────────────
        if !self.address_has_focus
            && !main_viewer_blocked
            && !self.any_dialog_open()
            && !self.search_has_focus
            && !self.favsearch.has_focus
            && !self.global_search.has_focus
            && !self.tag_view.has_focus
        {
            let ctrl_s = self.keymap.pressed_action(ctx, KeyAction::GlobalFavSearch);
            if ctrl_s {
                // 他の検索バーを閉じるのは open_favsearch 内で行う (相互排他)
                self.open_favsearch();
            }
        }

        // ── Ctrl+G: グローバルメタ検索バー表示 (docs §10.3) ──────────
        self.dispatch_everything_search_shortcut(ctx);

        if !self.address_has_focus
            && !main_viewer_blocked
            && !self.any_dialog_open()
            && !self.search_has_focus
            && !self.favsearch.has_focus
            && !self.tag_view.has_focus
        {
            let ctrl_g = self
                .keymap
                .pressed_action(ctx, KeyAction::GlobalMetadataSearch);
            if ctrl_g {
                // 相互排他は toggle_global_search → open_global_search 内で行う
                self.toggle_global_search();
            }
        }

        // ── Ctrl+T: タグビュー表示 ─────────────────────────────────
        if !self.address_has_focus
            && !main_viewer_blocked
            && !self.any_dialog_open()
            && !self.search_has_focus
            && !self.favsearch.has_focus
            && !self.global_search.has_focus
            && !self.tag_view.has_focus
        {
            let ctrl_t = self.keymap.pressed_action(ctx, KeyAction::GridTagView);
            if ctrl_t {
                self.toggle_tag_view();
            }
        }

        // ── T: タグを付ける/外す ───────────────────────────────────
        if !self.address_has_focus
            && !main_viewer_blocked
            && !self.any_dialog_open()
            && !self.search_has_focus
            && !self.favsearch.has_focus
            && !self.global_search.has_focus
            && !self.tag_view.has_focus
            && self.grid_item_input_allowed()
            && self.keymap.pressed_action(ctx, KeyAction::GridTagApply)
        {
            self.open_tag_apply_dialog();
        }

        // ── Ctrl+O: フォルダを開く ───────────────────────────────────
        if !main_viewer_blocked
            && !self.any_dialog_open()
            && !self.address_has_focus
            && !self.search_has_focus
            && !self.favsearch.has_focus
            && !self.tag_view.has_focus
        {
            let ctrl_o = self.keymap.pressed_action(ctx, KeyAction::GlobalOpenFolder);
            if ctrl_o {
                self.open_folder_input = self
                    .current_folder
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.show_open_folder_dialog = true;
            }
        }

        // ── 列数切り替え (1〜10 は Alt+数字) / Alt+-: 詳細表示切り替え ─────────
        if !self.address_has_focus
            && !self.search_has_focus
            && !self.favsearch.has_focus
            && !self.tag_view.has_focus
            && !main_viewer_blocked
            && !self.any_dialog_open()
        {
            if self
                .keymap
                .pressed_action(ctx, KeyAction::GridToggleDetailsView)
            {
                self.toggle_grid_details_view();
            }
            let selected_cols = [
                (KeyAction::GridColumnCount1, 1),
                (KeyAction::GridColumnCount2, 2),
                (KeyAction::GridColumnCount3, 3),
                (KeyAction::GridColumnCount4, 4),
                (KeyAction::GridColumnCount5, 5),
                (KeyAction::GridColumnCount6, 6),
                (KeyAction::GridColumnCount7, 7),
                (KeyAction::GridColumnCount8, 8),
                (KeyAction::GridColumnCount9, 9),
                (KeyAction::GridColumnCount10, 10),
                (KeyAction::GridColumnCount11, 11),
                (KeyAction::GridColumnCount12, 12),
                (KeyAction::GridColumnCount13, 13),
                (KeyAction::GridColumnCount14, 14),
                (KeyAction::GridColumnCount15, 15),
                (KeyAction::GridColumnCount16, 16),
                (KeyAction::GridColumnCount17, 17),
                (KeyAction::GridColumnCount18, 18),
                (KeyAction::GridColumnCount19, 19),
                (KeyAction::GridColumnCount20, 20),
            ]
            .into_iter()
            .find(|(action, _)| self.keymap.pressed_action(ctx, *action))
            .map(|(_, cols)| cols);
            if let Some(cols) = selected_cols {
                self.set_grid_view_mode(crate::settings::GridViewMode::Thumbnail);
                if cols != self.settings.grid_cols {
                    self.settings.grid_cols = cols;
                    self.settings.save();
                }
            }
        }
    }

    pub(super) fn post_render_main_navigation(
        &mut self,
        ctx: &egui::Context,
        navigation: update_phases::MainFrameNavigation,
    ) {
        let MainFrameNavigation {
            fav_nav,
            toolbar_fav_nav,
            fullscreen_close_nav,
            keyboard_nav,
            keyboard_nav_from_fullscreen_close,
            gamepad_nav,
            address_nav,
            open_folder_nav,
            context_nav,
            folder_pane_nav,
            grid_nav,
        } = navigation;
        // ── 非同期フォルダナビゲーションのポーリング ────────────────
        // 優先度 (旧来踏襲): fav_nav > toolbar_fav_nav > keyboard/gamepad_nav > folder_nav
        //                     > address_nav > open_folder_nav > context_nav
        //                     > folder_pane_open > grid_nav
        // folder_nav は fav/toolbar/keyboard/gamepad より後、address 以下より先。
        let mouse_ring_nav = self.mouse_ring_nav.take();
        // A prepared saved-group open is a candidate, not a poll-time side effect. A
        // different requested navigation wins without consuming its payload or history.
        if fav_nav.is_some()
            || toolbar_fav_nav.is_some()
            || fullscreen_close_nav.is_some()
            || keyboard_nav.is_some()
            || gamepad_nav.is_some()
            || mouse_ring_nav.is_some()
            || address_nav.is_some()
            || open_folder_nav.is_some()
            || context_nav.is_some()
            || folder_pane_nav.is_some()
            || grid_nav.is_some()
        {
            self.cancel_saved_group_open_for_other_nav();
        }
        self.retire_saved_group_open_if_replaced();
        let (input_nav, input_nav_from_fullscreen_close) = if fullscreen_close_nav.is_some() {
            (fullscreen_close_nav, true)
        } else if keyboard_nav.is_some() {
            (keyboard_nav, keyboard_nav_from_fullscreen_close)
        } else {
            (
                gamepad_nav
                    .or(mouse_ring_nav)
                    .or_else(|| self.saved_group_ready_nav()),
                false,
            )
        };
        let folder_nav_result = self.poll_folder_nav();
        let folder_nav_wins = folder_nav_result.is_some()
            && fav_nav.is_none()
            && toolbar_fav_nav.is_none()
            && input_nav.is_none();
        let higher_priority_than_folder_pane = fav_nav.is_some()
            || toolbar_fav_nav.is_some()
            || input_nav.is_some()
            || folder_nav_wins
            || address_nav.is_some()
            || open_folder_nav.is_some()
            || context_nav.is_some();
        if higher_priority_than_folder_pane {
            self.cancel_folder_pane_open(PaneOpenRestoreExit::Abandoned);
        }
        // フォルダツリーペインの worker scan 完了は、通常 nav 優先順位の候補として回収する。
        let mut folder_pane_open_ready = if higher_priority_than_folder_pane {
            None
        } else {
            self.poll_folder_pane_open(ctx)
        };

        if folder_nav_wins {
            // folder_nav 勝利: モードに応じて load_folder / close+load+open_fullscreen /
            // favsearch 処理に分岐。address_nav 以下の低優先度 nav は破棄する
            // (旧実装の `.or()` 短絡と同じ挙動)。
            if let Some(result) = folder_nav_result {
                self.apply_folder_nav_result(ctx, result);
            }
        } else {
            // folder_nav が未完了 or 他の高優先 nav 源が勝ったケース
            let higher_priority_direct_nav = fav_nav.or(toolbar_fav_nav);
            let mut navigation = None;
            let mut navigate_pre_scan: Option<ScannedDir> = None;
            let mut pane_epub_restore = None;
            let mut navigate_owner = OpenRequestOwner::Navigation;
            let mut navigate_restore_intent = StartupListIntent::ExplicitList;
            let navigate = if higher_priority_direct_nav.is_some() {
                higher_priority_direct_nav
            } else if let Some(nav) = input_nav.or(address_nav) {
                match nav {
                    crate::ui_main::AddressBarNav::Direct(path, restore_intent) => {
                        navigate_restore_intent = restore_intent;
                        Some(path)
                    }
                    crate::ui_main::AddressBarNav::GridVirtual(intent) => {
                        self.start_grid_virtual_open(intent);
                        None
                    }
                    crate::ui_main::AddressBarNav::CollectionSource {
                        path,
                        owner,
                        restore_intent,
                    } => {
                        navigate_restore_intent = restore_intent;
                        navigate_owner = OpenRequestOwner::CollectionGridPhysical(owner);
                        Some(path)
                    }
                    crate::ui_main::AddressBarNav::RatingSource {
                        owner,
                        restore_intent,
                        ..
                    } => {
                        self.start_rating_physical_open(owner, restore_intent);
                        None
                    }
                    crate::ui_main::AddressBarNav::DriveList(origin) => {
                        self.enter_drive_list_from_navigation(origin);
                        None
                    }
                    crate::ui_main::AddressBarNav::ReadingHistory => {
                        self.enter_reading_history_from_menu();
                        None
                    }
                    crate::ui_main::AddressBarNav::Bookmarks => {
                        self.enter_bookmark_view();
                        None
                    }
                    crate::ui_main::AddressBarNav::RatingViewBack => {
                        self.rating_view_parent_nav();
                        None
                    }
                    crate::ui_main::AddressBarNav::BooksRoot => {
                        self.open_books_root();
                        None
                    }
                    crate::ui_main::AddressBarNav::Collection(restore) => {
                        self.apply_collection_input_nav(restore, input_nav_from_fullscreen_close);
                        None
                    }
                    crate::ui_main::AddressBarNav::CollectionOpen(id) => {
                        self.bump_input_seq("grid-collection-key", Some(&format!("{id:?}")));
                        self.open_collection_grid_from_navigation(id);
                        None
                    }
                    crate::ui_main::AddressBarNav::SavedGroupReady(id) => {
                        self.adopt_saved_group_ready(id);
                        None
                    }
                    crate::ui_main::AddressBarNav::HistoryBack => {
                        self.dispatch_main_folder_history_input(FolderHistoryDirection::Back)
                    }
                    crate::ui_main::AddressBarNav::HistoryForward => {
                        self.dispatch_main_folder_history_input(FolderHistoryDirection::Forward)
                    }
                }
            } else if let Some(p) = open_folder_nav {
                Some(p)
            } else if let Some(action) = context_nav {
                // context_nav が実際に勝ったときだけ source surface の終了と typed
                // destination/selection の dispatch を適用する。show_context_menu 内で
                // 副作用を起こすと別 nav 源が同フレームで勝ったときに順序が脆くなる。
                match action {
                    crate::ui_dialogs::context_menu::ContextMenuAction::JumpToFolder(request) => {
                        self.begin_context_jump_to_folder(request)
                    }
                    crate::ui_dialogs::context_menu::ContextMenuAction::OpenGridContainer {
                        idx,
                        mode,
                    } => self
                        .open_grid_container_with_mode(ctx, idx, mode, "grid_context_menu")
                        .and_then(|nav| match nav {
                            crate::ui_main::AddressBarNav::Direct(path, restore_intent) => {
                                navigate_restore_intent = restore_intent;
                                Some(path)
                            }
                            crate::ui_main::AddressBarNav::GridVirtual(intent) => {
                                self.start_grid_virtual_open(intent);
                                None
                            }
                            crate::ui_main::AddressBarNav::CollectionSource {
                                path,
                                owner,
                                restore_intent,
                            } => {
                                navigate_restore_intent = restore_intent;
                                navigate_owner = OpenRequestOwner::CollectionGridPhysical(owner);
                                Some(path)
                            }
                            crate::ui_main::AddressBarNav::RatingSource {
                                owner,
                                restore_intent,
                                ..
                            } => {
                                self.start_rating_physical_open(owner, restore_intent);
                                None
                            }
                            crate::ui_main::AddressBarNav::DriveList(origin) => {
                                self.enter_drive_list_from_navigation(origin);
                                None
                            }
                            crate::ui_main::AddressBarNav::ReadingHistory => {
                                self.enter_reading_history();
                                None
                            }
                            crate::ui_main::AddressBarNav::Bookmarks => {
                                self.enter_bookmark_view();
                                None
                            }
                            crate::ui_main::AddressBarNav::RatingViewBack => {
                                self.rating_view_parent_nav();
                                None
                            }
                            crate::ui_main::AddressBarNav::BooksRoot => {
                                self.open_books_root();
                                None
                            }
                            crate::ui_main::AddressBarNav::Collection(restore) => {
                                self.apply_collection_input_nav(restore, false);
                                None
                            }
                            crate::ui_main::AddressBarNav::CollectionOpen(id) => {
                                self.bump_input_seq(
                                    "grid-collection-key",
                                    Some(&format!("{id:?}")),
                                );
                                self.open_collection_grid_from_navigation(id);
                                None
                            }
                            crate::ui_main::AddressBarNav::HistoryBack
                            | crate::ui_main::AddressBarNav::HistoryForward
                            | crate::ui_main::AddressBarNav::SavedGroupReady(_) => None,
                        }),
                }
            } else if let Some(p) = folder_pane_nav {
                // フォルダツリーペインのクリック/Enter ナビは worker で scan_directory を
                // 済ませてから開く (UI スレッドの read_dir が大/遅/ネットワークフォルダで
                // 固まるのを防ぐ)。同期 load はせず、完了後に通常 nav 優先順位で裁定する。
                // 対象は実ディレクトリ前提。
                self.replace_ready_folder_pane_open(p, folder_pane_open_ready.take());
                None
            } else if let Some(ready) = folder_pane_open_ready.take() {
                #[cfg(windows)]
                {
                    self.resolve_main_folder_open_ready(ctx, ready)
                        .map(|ready| {
                            navigate_restore_intent = ready.restore_intent;
                            navigate_pre_scan = Some(ready.scan);
                            pane_epub_restore = ready.epub_restore;
                            navigation = ready.navigation;
                            if let Some(owner) = ready.collection_owner {
                                navigate_owner = OpenRequestOwner::CollectionGridPhysical(owner);
                            }
                            ready.path
                        })
                }
                #[cfg(not(windows))]
                {
                    let FolderPaneOpenReady {
                        path,
                        scan,
                        purpose,
                        epub_restore,
                    } = ready;
                    match (purpose, scan) {
                        (
                            FolderOpenScanPurpose::JumpToPhysicalFolder { selection, origin },
                            Ok(scan),
                        ) => {
                            self.apply_jump_to_physical_folder_ready(path, scan, selection, origin);
                            None
                        }
                        (
                            FolderOpenScanPurpose::CurrentViewOrderRefresh {
                                order,
                                reload_owner,
                            },
                            Ok(scan),
                        ) => {
                            self.apply_current_view_order_refresh(path, scan, order, *reload_owner);
                            None
                        }
                        (
                            FolderOpenScanPurpose::GridFolderCandidate { collection_owner },
                            Ok(scan),
                        ) => {
                            navigate_pre_scan = Some(scan);
                            if let Some(owner) = collection_owner {
                                navigate_owner = OpenRequestOwner::CollectionGridPhysical(owner);
                            }
                            Some(path)
                        }
                        (_, Ok(scan)) => {
                            navigate_pre_scan = Some(scan);
                            pane_epub_restore = epub_restore;
                            Some(path)
                        }
                        (_, Err(error)) => {
                            self.finish_pane_open_restore(
                                epub_restore,
                                PaneOpenRestoreExit::Abandoned,
                            );
                            crate::logger::log(format!(
                                "folder open scan failed path={} error={error}",
                                path.display()
                            ));
                            self.show_feedback_toast("フォルダを読み取れませんでした".to_string());
                            None
                        }
                    }
                }
            } else {
                match grid_nav {
                    Some(crate::ui_main::AddressBarNav::Direct(path, restore_intent)) => {
                        navigate_restore_intent = restore_intent;
                        Some(path)
                    }
                    Some(crate::ui_main::AddressBarNav::GridVirtual(intent)) => {
                        self.start_grid_virtual_open(intent);
                        None
                    }
                    Some(crate::ui_main::AddressBarNav::CollectionSource {
                        path,
                        owner,
                        restore_intent,
                    }) => {
                        navigate_restore_intent = restore_intent;
                        navigate_owner = OpenRequestOwner::CollectionGridPhysical(owner);
                        Some(path)
                    }
                    Some(crate::ui_main::AddressBarNav::RatingSource {
                        owner,
                        restore_intent,
                        ..
                    }) => {
                        self.start_rating_physical_open(owner, restore_intent);
                        None
                    }
                    Some(crate::ui_main::AddressBarNav::DriveList(origin)) => {
                        self.enter_drive_list_from_navigation(origin);
                        None
                    }
                    Some(crate::ui_main::AddressBarNav::ReadingHistory) => {
                        self.enter_reading_history();
                        None
                    }
                    Some(crate::ui_main::AddressBarNav::Bookmarks) => {
                        self.enter_bookmark_view();
                        None
                    }
                    Some(crate::ui_main::AddressBarNav::RatingViewBack) => {
                        self.rating_view_parent_nav();
                        None
                    }
                    Some(crate::ui_main::AddressBarNav::BooksRoot) => {
                        self.open_books_root();
                        None
                    }
                    Some(crate::ui_main::AddressBarNav::Collection(restore)) => {
                        self.apply_collection_input_nav(restore, false);
                        None
                    }
                    Some(crate::ui_main::AddressBarNav::CollectionOpen(id)) => {
                        self.bump_input_seq("grid-collection-key", Some(&format!("{id:?}")));
                        self.open_collection_grid_from_navigation(id);
                        None
                    }
                    Some(crate::ui_main::AddressBarNav::SavedGroupReady(_)) => None,
                    Some(
                        crate::ui_main::AddressBarNav::HistoryBack
                        | crate::ui_main::AddressBarNav::HistoryForward,
                    )
                    | None => None,
                }
            };
            if let Some(p) = navigate {
                self.open_direct_navigation_target(
                    p,
                    navigate_pre_scan.take(),
                    navigate_owner,
                    navigation,
                    pane_epub_restore.take(),
                    navigate_restore_intent,
                );
            }
        }
    }

    pub(super) fn post_render_native_drag(&mut self, ctx: &egui::Context) {
        // ── native ファイル D&D の実行 (docs/file-drag-drop-design.md §5.5b) ──
        // 全パネル描画後・perf ログ後に実行する。SHDoDragDrop はモーダルブロック
        // するので、frame_total の計測外に置いて perf ログを汚さない。
        if let Some(PendingNativeDrag {
            paths,
            post_drag_toast,
        }) = self.pending_native_drag.take()
        {
            match self.main_hwnd {
                Some(hwnd) => {
                    // SHDoDragDrop が戻る (ドロップ完了 or キャンセル) までブロックする。
                    let outcome = crate::file_drag::start_file_drag(hwnd, &paths);
                    // SHDoDragDrop に到達したときだけポインタリセットが要る (§6.1)。
                    // 未到達ならフラグを立てない (winit が WM_LBUTTONUP を正常受信)。
                    if outcome.started {
                        self.native_drag_just_finished = true;
                        // ドロップが成立し、かつ落下先が mIV 自身のウィンドウなら、
                        // 直後の dropped_files (跳ね返り) を受け取らないよう印を付ける。
                        if outcome.dropped && Self::cursor_over_window(hwnd) {
                            self.internal_drop_pending = true;
                        }
                    }
                    if let Some(err) = &outcome.error {
                        crate::logger::log(format!("file_drag: outcome error: {err:?}"));
                    }
                    // 結果トーストはここ (= SHDoDragDrop 完了後) で出す。drag_started
                    // 時点で出すとブロック中に表示期限が切れる (§5.4.2)。失敗・未開始時は
                    // emit_drag_result_toasts 側が post_drag_toast を抑止する (Codex P2)。
                    self.emit_drag_result_toasts(Some(&outcome), post_drag_toast);
                }
                None => {
                    // 初フレーム前など main_hwnd 未取得。ドラッグは走らない。
                    crate::logger::log("file_drag: main_hwnd unavailable, drag skipped");
                    self.emit_drag_result_toasts(None, post_drag_toast);
                }
            }
            ctx.request_repaint();
            // 連続ドラッグ (このフレームでまた drag が積まれた場合) は started で
            // フラグが再び立つ。else 節に来ないので消費されず、次フレームも冒頭
            // リセットが走る — これは意図どおり。
        } else if self.native_drag_just_finished {
            // 直前フレームが「ドラッグ実行フレーム」だった。このフレーム冒頭で
            // ポインタリセットを済ませたのでフラグを消費する (§6.1)。
            self.native_drag_just_finished = false;
        }
    }

    pub(super) fn post_render_scroll_bookkeeping(&mut self, ctx: &egui::Context) {
        // ── スクロール後の可視サムネ計装 (docs/scroll-visibility-priority-plan.md Phase 1) ──
        // `render_grid` が `scroll_to_selected` を反映済みのこの位置で scroll_offset_y を
        // 観測する。変化があれば last_scroll_event_at を更新し、300ms 経過後の最初の
        // フレームで scroll_settle を発火。
        self.update_scroll_settle_state();
        // PDF pool snapshot を 1 秒に 1 回 emit (pool 未初期化なら no-op)。
        self.maybe_emit_pool_queue_snapshot();

        // auto_aspect: 時間ゲート (streak / input idle / cooldown) で
        // `maybe_apply_auto_aspect` が早期 return すると、UI イベントが無いと再評価
        // されない (= マウス動かすまで反応しない問題)。ゲートのうち最短の残り時間で
        // 再描画を予約する (Codex P2 + 2026-05 ユーザー報告)。
        if self.settings.thumb_aspect_auto && self.auto_aspect.switches_done < 2 {
            const STREAK_DURATION: std::time::Duration = std::time::Duration::from_millis(750);
            const INPUT_IDLE_GUARD: std::time::Duration = std::time::Duration::from_millis(500);
            const SWITCH_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(2);

            let now = std::time::Instant::now();
            let mut next_wait: Option<std::time::Duration> = None;
            let mut update_wait = |dur: std::time::Duration| match next_wait {
                None => next_wait = Some(dur),
                Some(cur) if dur < cur => next_wait = Some(dur),
                _ => {}
            };

            // streak は **残り時間が正のときだけ**予約候補に入れる。expired (= 0ms)
            // を候補に入れると、他 gate (input idle / cooldown) がまだブロック中の
            // 状況で「streak 0ms → 即時 repaint → 次フレでも streak 0ms」のループに
            // なる (Codex P2 2026-05)。expired ならどうせ他 gate 解除時の評価で
            // 拾われるか、新サムネ着弾時の poll_thumbnails で拾われる。
            if let Some((_, since, _)) = self.auto_aspect.streak {
                let elapsed = since.elapsed();
                if elapsed < STREAK_DURATION {
                    update_wait(STREAK_DURATION - elapsed);
                }
            }
            if let Some(t) = self.last_input_at {
                let elapsed = now.duration_since(t);
                if elapsed < INPUT_IDLE_GUARD {
                    update_wait(INPUT_IDLE_GUARD - elapsed);
                }
            }
            if let Some(t) = self.auto_aspect.last_switch_at {
                let elapsed = now.duration_since(t);
                if elapsed < SWITCH_COOLDOWN {
                    update_wait(SWITCH_COOLDOWN - elapsed);
                }
            }

            // この時点で next_wait は必ず正の値 (= 各 gate で `elapsed < GUARD` のとき
            // だけ update_wait を呼んでいる)。0 値は streak expired を除外する修正で
            // 通らなくなったので、常に request_repaint_after で OK。
            if let Some(d) = next_wait {
                ctx.request_repaint_after(d);
            }
        }
    }

    pub(super) fn service_update_registered_writers(&mut self, ctx: &egui::Context) {
        crate::page_edit_write_epoch::PAGE_EDIT_WRITES.register_repaint_context(ctx);
        crate::rating_db::RATING_WRITES.register_repaint_context(ctx);
        crate::tags_db::TAG_WRITES.register_repaint_context(ctx);
        self.poll_page_edit_reconciliation(ctx);
    }

    pub(super) fn render_update_before_frame(&mut self, ctx: &egui::Context) {
        #[cfg(test)]
        super::egui037_connection_tests::observe("before_frame", self, ctx);
        // Retired asset requests must cancel even when presentation returns early.
        self.prune_current_view_pin_refreshes();
        // Collection startup/revision/worker responses must likewise progress even when
        // update_frame returns through a fullscreen or native-video presentation path.
        self.poll_collection_ui(ctx);
        self.poll_mounted_document_open_owners(ctx);
    }

    // Worker adoption can enter a UI-bound presentation continuation; see the ledger.
    pub(super) fn render_update_after_frame(&mut self, ctx: &egui::Context) {
        #[cfg(test)]
        super::egui037_connection_tests::observe("after_frame", self, ctx);
        // Keep sent-ticket receipts before UI preparation and publication.
        // The receipt entry is split, while legacy owner mounts remain until 1b-3b.
        // Run at the old post-presentation position, including early-return frames.
        self.poll_composition_editor_operations(ctx);
        self.poll_book_composition_service(ctx);
    }

    pub(super) fn render_update_owner_dialogs(&mut self, ctx: &egui::Context) {
        if self.fullscreen_idx.is_none() || self.fullscreen_embedded_still_active() {
            self.show_local_composition_editor(ctx);
            self.show_image_folder_zip(ctx);
        }
        #[cfg(windows)]
        self.sync_clipboard_capture(ctx);
        self.show_capture_selection_dialog(ctx);
        // Dedicated viewer viewports draw their mounted owner's dialog in their callback.
        // Root and embedded presentations draw here; parked contexts are not serviced.
        if self.fullscreen_idx.is_none()
            || self.fullscreen_embedded_still_active()
            || self.current_viewer_context_contains_video()
        {
            #[cfg(windows)]
            viewer_context_registry::ContextAsyncOwner::EpubConversion.service(
                self,
                ctx,
                viewer_context_registry::ContextAsyncServicePhase::Dialog,
            );
            #[cfg(not(windows))]
            self.show_epub_convert_dialog(ctx);
        }
        // `update_frame` can return before its menu/dialog section. Keep the global password
        // input visible through those paths; a dedicated viewport draws it in its own callback.
        #[cfg(windows)]
        if !self.active_detached_owns_pdf_password_prompt() {
            viewer_context_registry::ContextAsyncOwner::PdfPassword.service(
                self,
                ctx,
                viewer_context_registry::ContextAsyncServicePhase::Dialog,
            );
            // Smart-folder password preflight has its own global typed owner.
            if !self.pdf_password_request_pending_in_any_context()
                && self.smart_pdf_password_dialog_path().is_some()
            {
                self.show_pdf_password_dialog_window(ctx);
            }
        }
        #[cfg(not(windows))]
        self.show_pdf_password_dialog_window(ctx);
    }

    pub(super) fn post_render_favorite_writes(&mut self, ctx: &egui::Context) {
        // UI の代入箇所を列挙せず、frame 終端で有効値との差分を一括検出する。
        if !self.sidecar_restore_active() {
            self.reconcile_favorite_view_for_current_context_at(std::time::Instant::now());
            self.poll_favorite_view_writes(ctx);
        }
    }

    pub(super) fn render_update_external_tool_checkpoint(&mut self, ctx: &egui::Context) {
        // `update_frame` は native 動画 backdrop / 静止画 viewport 抑止 / embedded 保留の
        // 3 経路で早期 return する。その frame では外部ツールの modal も spawn 境界の
        // ACK も落ちるので、**早期 return では飛ばせないここ**で拾う。
        //
        // 落とすと、準備中の要求を抱えたままフルスクリーンで動画へ移動した利用者は、
        // 動画を閉じるまで外部ツールが起動しない (Codex Sol 指摘 #2)。
        if self.external_tool_launch_ui_frame != Some((egui::ViewportId::ROOT, self.frame_counter))
        {
            self.show_external_tool_modals(ctx);
            self.authorize_external_tool_launch_boundaries_after_ui();
        }
    }

    pub(super) fn input_frame_diagnostics(&mut self, ctx: &egui::Context) {
        #[cfg(test)]
        super::egui037_connection_tests::observe("rescan_after", self, ctx);
        // パフォーマンス計装: フレーム境界。--perf-log 無効時は is_enabled() 読みのみ
        self.frame_counter = self.frame_counter.wrapping_add(1);
        #[cfg(windows)]
        self.log_main_flash_probe("update_begin", "after_frame_counter_increment");
        if crate::perf::is_enabled() {
            let frame_begin_now = std::time::Instant::now();
            if let Some(prev_begin) = self.perf_last_frame_begin {
                let gap = frame_begin_now.saturating_duration_since(prev_begin);
                if gap > std::time::Duration::from_millis(30) {
                    let video_snapshot =
                        self.fullscreen_idx
                            .and_then(|idx| match self.fs_cache.get(&idx) {
                                Some(FsCacheEntry::Video { player, .. }) => Some((
                                    idx,
                                    player.is_playing(),
                                    player.is_paused_or_seeking(),
                                    player.pending_frames(),
                                    player.engine_state_code(),
                                    player.displayed_frame_seq(),
                                )),
                                _ => None,
                            });
                    let (
                        video_idx,
                        video_playing,
                        video_freeze,
                        video_pending_frames,
                        video_state,
                        video_seq,
                    ) = video_snapshot
                        .map(|(idx, playing, freeze, pending, state, seq)| {
                            (
                                idx as i64,
                                playing,
                                freeze,
                                pending as i64,
                                state as i64,
                                seq as i64,
                            )
                        })
                        .unwrap_or((-1, false, false, -1, -1, -1));
                    #[cfg(windows)]
                    let vst3_active_slots = self.dsp_bridge.active_slot_count() as i64;
                    #[cfg(not(windows))]
                    let vst3_active_slots = 0_i64;
                    // 計装: window state と repaint 駆動条件を frame_gap に同梱する
                    // (2026-05-10、最小化中に App::update が止まらない問題の調査用)。
                    let is_minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
                    let visible_video_thumb_pending =
                        self.requested.video_image_pending(&self.keep_set);
                    let thumbnail_work_active = self.requested.blocks_idle_upgrade()
                        || self.requested.images_len() > 0
                        || visible_video_thumb_pending;
                    crate::perf::event(
                        "ui",
                        "frame_gap",
                        None,
                        self.input_seq,
                        &[
                            (
                                "gap_ms",
                                serde_json::Value::from(gap.as_secs_f64() * 1000.0),
                            ),
                            ("n", serde_json::Value::from(self.frame_counter as i64)),
                            (
                                "fullscreen",
                                serde_json::Value::from(self.fullscreen_idx.is_some()),
                            ),
                            ("video_idx", serde_json::Value::from(video_idx)),
                            ("video_playing", serde_json::Value::from(video_playing)),
                            ("video_freeze", serde_json::Value::from(video_freeze)),
                            (
                                "video_pending_frames",
                                serde_json::Value::from(video_pending_frames),
                            ),
                            ("video_state", serde_json::Value::from(video_state)),
                            ("video_seq", serde_json::Value::from(video_seq)),
                            (
                                "vst3_manager",
                                serde_json::Value::from(self.show_vst3_manager),
                            ),
                            (
                                "vst3_visible",
                                serde_json::Value::from(self.settings.vst3_gui_visible),
                            ),
                            (
                                "vst3_active_slots",
                                serde_json::Value::from(vst3_active_slots),
                            ),
                            ("is_minimized", serde_json::Value::from(is_minimized)),
                            (
                                "window_visible",
                                serde_json::Value::from(self.window_visible),
                            ),
                            (
                                "thumbnail_work_active",
                                serde_json::Value::from(thumbnail_work_active),
                            ),
                            (
                                "requested_len",
                                serde_json::Value::from(self.requested.legacy_len() as i64),
                            ),
                            (
                                "texture_backlog_len",
                                serde_json::Value::from(self.requested.images_len() as i64),
                            ),
                            (
                                "folder_nav_pending",
                                serde_json::Value::from(self.folder_nav_pending.is_some()),
                            ),
                            (
                                "pdf_enumerate_pending",
                                serde_json::Value::from(self.pdf_enumerate_pending.is_some()),
                            ),
                            (
                                "zip_enumerate_pending",
                                serde_json::Value::from(self.zip_enumerate_pending.is_some()),
                            ),
                            (
                                "video_upscale_running",
                                serde_json::Value::from(self.video_upscale_running.is_some()),
                            ),
                            (
                                "fs_viewport_shown",
                                serde_json::Value::from(self.fs_viewport_shown),
                            ),
                        ],
                    );
                }
            }
            self.perf_last_frame_begin = Some(frame_begin_now);
            let prev_update_ms = self.perf_prev_update_ms.unwrap_or(0.0);
            // frame.begin is emitted partway through update_frame. Reuse the
            // previous outer endpoint instead of treating that event as its start.
            let prev_update_end_t = self.perf_last_update_end.map(crate::perf::stall::seconds);
            let prev_update_start_t = prev_update_end_t
                .zip(self.perf_prev_update_ms)
                .map(|(end, ms)| end - ms / 1000.0);
            let prev_outside_ms = self
                .perf_last_update_end
                .map(|end| frame_begin_now.saturating_duration_since(end).as_secs_f64() * 1000.0)
                .unwrap_or(0.0);
            crate::perf::event(
                "frame",
                "begin",
                None,
                self.input_seq,
                &[
                    ("n", serde_json::Value::from(self.frame_counter)),
                    // The previous frame, split: what App::update spent, and what eframe
                    // spent rendering and presenting after it returned.
                    ("prev_update_ms", serde_json::Value::from(prev_update_ms)),
                    (
                        "prev_update_start_t",
                        serde_json::Value::from(prev_update_start_t),
                    ),
                    (
                        "prev_update_end_t",
                        serde_json::Value::from(prev_update_end_t),
                    ),
                    ("prev_outside_ms", serde_json::Value::from(prev_outside_ms)),
                    (
                        "prev_update_cycles",
                        serde_json::Value::from(self.perf_prev_update_cycles.unwrap_or(0)),
                    ),
                    // Near the machine's clock rate means the frame computed; a fraction of
                    // it means the thread spent that wall time waiting or preempted.
                    (
                        "prev_update_cycles_per_ms",
                        serde_json::Value::from(if prev_update_ms > 0.0 {
                            self.perf_prev_update_cycles.unwrap_or(0) as f64 / prev_update_ms
                        } else {
                            0.0
                        }),
                    ),
                ],
            );
            // 起動時間計測: 最初の update() 呼び出し = winit が初回描画に入った瞬間。
            // `total_ms` に main() 入口からの累計経過を載せる。creator_exit との差分が
            // wgpu パイプライン構築 + winit 初期描画準備の時間になる。
            if self.frame_counter == 1
                && let Some(prog) = crate::perf::program_start()
            {
                crate::perf::event(
                    "startup",
                    "first_frame",
                    None,
                    0,
                    &[(
                        "total_ms",
                        serde_json::Value::from(prog.elapsed().as_secs_f64() * 1000.0),
                    )],
                );
            }
            // 約 1 秒に 1 回 flush (BufWriter のデータをディスクへ)
            let now = std::time::Instant::now();
            let should_flush = self
                .perf_last_flush
                .map(|t| now.duration_since(t).as_millis() >= 1000)
                .unwrap_or(true);
            if should_flush {
                crate::perf::flush();
                self.perf_last_flush = Some(now);
            }
        }
    }

    pub(super) fn render_prepare_fullscreen_materials(&mut self, ctx: &egui::Context) {
        #[cfg(test)]
        super::egui037_connection_tests::observe("materials_before", self, ctx);
        // フルスクリーン表示中なら AI アップスケール + 画像補正を検討
        if let Some(fs_idx) = self.fullscreen_idx {
            // 360 の意図と実際の状態を突き合わせる。素材が 360 だと分かる時刻が
            // 静止画 / 動画で違うため、イベントではなくここで毎フレーム見る
            // (`reconcile_panorama_session_intent` の doc 参照)。描画より前。
            self.reconcile_panorama_session_intent(fs_idx);
            // プリセットに基づいてアップスケール設定を同期
            self.sync_upscale_from_preset(fs_idx);
            let page_turn_decision = self.fs_page_turn_decision_for_frame(ctx, fs_idx);
            if page_turn_decision.admits_navigation_target_materialization() {
                let continuous_keep_set = if !self.reading_flow.is_paged()
                    && self.fs_vertical_cache_keep_set.contains(&fs_idx)
                {
                    Some(self.fs_vertical_cache_keep_set.clone())
                } else {
                    None
                };
                // 見開き相方ページ (Double のとき)。reconcile と local_adjust の両方で使う。
                let spread_other = self.displayed_spread_partner(fs_idx);
                // PDF raster の display-fit 結果が AI 用 native 入力と異なるなら、native
                // 解像度へ再レンダして AI を起動する。
                // sync_upscale_from_preset の **直後** = fs_idx の AI 設定が最新の地点で評価。
                // 判定自体はページ個別 effective_params なので、見開き相方・連続表示の可視
                // ページ (= ui_fullscreen が final AI をかける全ページ) にも個別に適用する。
                {
                    let mut targets: Vec<usize> = Vec::with_capacity(2);
                    targets.push(fs_idx);
                    if let Some(other) = spread_other {
                        targets.push(other);
                    }
                    if let Some(keep_set) = continuous_keep_set.as_ref() {
                        targets.extend(keep_set.iter().copied());
                    }
                    targets.sort_unstable();
                    targets.dedup();
                    for idx in targets {
                        self.maybe_native_rerender_pdf_for_ai(idx);
                    }
                }
                if let Some(keep_set) = continuous_keep_set.as_ref() {
                    self.evict_final_pipeline_cache_for_keep_set(keep_set);
                } else {
                    self.evict_final_pipeline_cache(fs_idx);
                }
                if let Some(other) = spread_other {
                    self.maybe_start_local_adjust_render(other);
                }
                self.maybe_start_local_adjust_render(fs_idx);
                if let Some(keep_set) = continuous_keep_set.as_ref() {
                    self.evict_adjustment_cache_for_keep_set(keep_set);
                } else {
                    self.evict_adjustment_cache(fs_idx);
                }
                // AI 先読み (新パイプライン v1.1.0)。現在ページの final_ai が完了 / 不要
                // のときだけ隣接ページを起動するため、毎フレーム呼んでも spawn の競合を
                // 起こさない (= 内部で has_uncancelled_final_ai_pending を gate に使う)。
                // Pipeline P1 リファクタで dead code 化していた旧 `prefetch_ai_upscale` の
                // 後継 (= ページ送り時 AI を待たされる退行を直す)。
                if page_turn_decision.admits_non_target_full_resolution_work() {
                    self.reconcile_paged_prefetch_window(fs_idx);
                    self.prefetch_final_ai(ctx, fs_idx);
                } else {
                    // Target final-effect results are part of materialization; unrelated
                    // prefetch results stay in their channels until the sequence settles.
                    self.poll_final_effects(ctx);
                }
            }
        }
    }

    pub(super) fn input_root_keyboard(
        &mut self,
        ctx: &egui::Context,
    ) -> (
        Option<crate::ui_main::AddressBarNav>,
        bool,
        Option<crate::ui_fullscreen::FullscreenRootFrameInput>,
    ) {
        #[cfg(test)]
        super::egui037_connection_tests::observe("materials_after", self, ctx);
        // フルスクリーンのキーが main/root 側に届いた場合は、main focus guard より先に
        // 処理する。F11 の window/fullscreen 切替など、main が focused になること自体が
        // 操作の副作用であるキーを、一覧へ戻りたい意図と誤認しないため。
        let main_viewport_explicit_input = ctx.input(|input| {
            input
                .events
                .iter()
                .any(main_viewport_event_is_explicit_input)
        });
        let (fullscreen_root_key_handled, fullscreen_root_input) =
            self.fullscreen_root_input_for_frame(ctx);

        // ── フルスクリーン中にメインウィンドウへフォーカスが来たら閉じる ──
        // ボーダーレスウィンドウなので Alt-Tab 等でメインに戻れるが、
        // そのままだと両方のウィンドウがキー入力を無視して操作不能に見える。
        // 既定ではメインにフォーカスが来た = ユーザーがサムネイル一覧に戻りたい意図と
        // 解釈し、フルスクリーンを閉じてメインウィンドウで通常操作を再開する。
        // `fullscreen_keep_on_app_switch` が ON のときは、他アプリから mIV main へ戻った
        // 場合にも fullscreen session を維持し、main に来た focus を fullscreen 側へ
        // 戻す。メインも並行操作したい場合は detached viewer (F12) を使う。
        //
        // ただし open_fullscreen() 直後はフルスクリーンビューポートへの
        // ViewportCommand::Focus が反映されるまで数フレームかかるため、
        // 500ms のグレース期間中はチェックをスキップする。
        let main_viewport_focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
        self.reconcile_fullscreen_after_main_focus(
            ctx,
            main_viewport_focused,
            main_viewport_explicit_input,
            fullscreen_root_key_handled,
        );
        // A root-key or focus close can happen above. Restore the retained aggregate before
        // ordinary keyboard routing sees an index from the flat fullscreen order.
        self.stack_reconcile_after_fullscreen_close(ctx);

        if !fullscreen_root_key_handled {
            self.handle_clipboard_shortcuts(ctx);
        }

        let keyboard_nav_from_fullscreen_close =
            !fullscreen_root_key_handled && self.pending_return_to_parent.is_pending();
        let keyboard_nav = if fullscreen_root_key_handled {
            None
        } else {
            self.handle_keyboard(ctx)
        };
        let keyboard_nav_from_fullscreen_close =
            keyboard_nav_from_fullscreen_close && keyboard_nav.is_some();
        (
            keyboard_nav,
            keyboard_nav_from_fullscreen_close,
            fullscreen_root_input,
        )
    }

    pub(super) fn render_main_bars(
        &mut self,
        ui: &mut egui::Ui,
        pre_grid_perf: &mut Option<PreGridPerfRecorder>,
    ) -> (Option<PathBuf>, Option<egui::Rect>) {
        // ── 検索バー ─────────────────────────────────────────────────
        self.render_search_bar(ui);
        mark_pre_grid_perf(pre_grid_perf, PreGridPerfStage::SearchBar);

        // ── お気に入り検索バー (ツールバー直下の 2 行目相当) ─────────
        self.render_favsearch_bar(ui);
        mark_pre_grid_perf(pre_grid_perf, PreGridPerfStage::FavSearchBar);

        // ── Ctrl+G グローバルメタ検索バー (docs §10.3) ────────────────
        self.render_global_search_bar(ui);
        mark_pre_grid_perf(pre_grid_perf, PreGridPerfStage::GlobalSearchBar);

        // ── Ctrl+T タグビュー ───────────────────────────────────────
        self.render_tag_view_bar(ui);
        mark_pre_grid_perf(pre_grid_perf, PreGridPerfStage::TagViewBar);

        // ── スマートフィルタバー (サムネ/詳細共通) ───────────────────
        self.render_facet_filter_bar(ui);
        mark_pre_grid_perf(pre_grid_perf, PreGridPerfStage::FacetFilterBar);
        self.render_details_lazy_status_bar(ui);
        mark_pre_grid_perf(pre_grid_perf, PreGridPerfStage::DetailsLazyStatusBar);

        // 下部情報バーは CentralPanel のグリッドより先に確保する。egui が残り領域を
        // render_grid へ渡すため、last_viewport_h と仮想スクロール範囲にも反映される。
        self.render_selection_info_bar(ui);
        mark_pre_grid_perf(pre_grid_perf, PreGridPerfStage::SelectionInfoBar);
        let (folder_pane_nav, folder_pane_rect) = self.render_folder_pane(ui);
        mark_pre_grid_perf(pre_grid_perf, PreGridPerfStage::FolderPane);

        // チェック件数は一覧の右上へ常時表示する。Foreground Area をグリッドより先に
        // 登録し、解除クリックが背面セルの選択操作へ漏れないようにする。
        self.render_checked_selection_overlay(ui);
        mark_pre_grid_perf(pre_grid_perf, PreGridPerfStage::CheckedSelectionOverlay);

        (folder_pane_nav, folder_pane_rect)
    }

    /// Must run even after update_frame returns, after this pass's modal/input gate.
    pub(super) fn post_render_sidecar_restore(&mut self, ctx: &egui::Context) {
        self.poll_sidecar_restore(ctx);
    }
}
