//! タスクトレイ常駐 / インデクサ throttle の App 側統合 (v0.9)。
//!
//! `App` に対する impl ブロックで、タスクトレイのライフサイクル管理、
//! ウィンドウ可視状態遷移、GPU リソース解放、メニューイベント処理を扱う。
//! ロジックを `src/app.rs` から切り出すことで、本体の肥大化を抑える。

use eframe::egui;

use crate::app::App;
use crate::tray::TrayEvent;

impl App {
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
                        // checkmark は設定値 (pause_indexer_while_minimized) を反映する。
                        // 以前は activity_gate.is_paused() を使っていたが、ウィンドウ表示中は
                        // 必ず false で、ダイアログの設定チェックボックスと表示がズレていた。
                        // ユーザーの自然なメンタルモデル「常駐時に一時停止するか」の設定値を
                        // 直接出すのが正。
                        if let Some(tc) = &self.tray_controller {
                            tc.set_paused_check(self.settings.pause_indexer_while_minimized);
                        }
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
        self.hide_to_tray();
        true
    }

    /// ウィンドウを非表示にしてタスクトレイ状態へ遷移する。
    ///
    /// **重要**: `ViewportCommand::Visible(false)` は使わない。それを使うと eframe/winit が
    /// `App::update` を呼ばなくなり、トレイメニューから復帰できなくなる (request_repaint が
    /// 効かない)。代わりに Win32 `ShowWindow(hwnd, SW_HIDE)` を直接呼ぶ。
    ///
    /// サイズ保存について: hide の直前に `GetWindowPlacement` で rect を丸ごと捕獲しておき、
    /// 復帰時に `SetWindowPlacement` で完全復元する。eframe/winit の DPI 丸めを完全に
    /// バイパスできるため、マルチモニタ DPI 環境でも開閉でサイズが変わらない。
    fn hide_to_tray(&mut self) {
        if !self.window_visible {
            return;
        }
        self.window_visible = false;

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
            use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
            unsafe {
                let _ = ShowWindow(HWND(hwnd_raw as *mut _), SW_HIDE);
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
        // 終了時と同じ永続化処理を hide のタイミングでも走らせる。
        // トレイメニュー「終了」は hidden 状態では std::process::exit(0) で抜けるため
        // on_exit が呼ばれない可能性があり、ここで確実に flush しておくことでデータロスを防ぐ。
        self.persist_window_state_and_flush();

        // トレイの表示を更新
        self.update_tray_tooltip();
        crate::logger::log("tray: window hidden to tray (Win32 SW_HIDE, placement saved)");
    }

    /// タスクトレイから復帰した後の **App 側事後処理**。
    /// トレイスレッドの OpenRequested 経路と、外部 ShowWindow 検出経路 (アクティベーション
    /// リスナー等) の両方から呼ばれる。
    pub(crate) fn sync_after_restore(&mut self) {
        if self.window_visible {
            return;
        }
        self.window_visible = true;
        if let Some(mgr) = self.indexer_manager.as_ref() {
            mgr.set_io_throttled(false);
        }
        self.activity_gate.set_paused(false);
        self.update_tray_tooltip();
        // 外部 (ComfyUI 等) がトレイ常駐中に current_folder へファイルを追加していたら
        // 自動で反映する。stat 1 回の軽量チェックで、変化が無ければ no-op。
        self.check_external_folder_changes();
        crate::logger::log("tray: App state synced after restore");
    }

    /// GPU テクスチャキャッシュを破棄する (VRAM 解放目的)。
    ///
    /// ウィンドウ復帰後は通常のロード経路で再取得されるので、描画には影響なし
    /// (短時間の再ロードオーバーヘッドが発生する)。
    fn release_gpu_resources(&mut self) {
        // グリッドサムネ: Loaded → Evicted で TextureHandle を drop
        for state in &mut self.thumbnails {
            if matches!(state, crate::grid_item::ThumbnailState::Loaded { .. }) {
                *state = crate::grid_item::ThumbnailState::Evicted;
            }
        }
        // フルスクリーン画像キャッシュ (最大サイズ源、20MP RGBA ≈ 80MB/枚)
        self.fs_cache.clear();
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
    pub(crate) fn poll_tray_events(&mut self) {
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
                    self.sync_after_restore();
                }
                TrayEvent::TogglePauseRequested { new_checked } => {
                    // トレイ checkmark = 設定 `pause_indexer_while_minimized` の新値
                    // (muda が auto-toggle 済み、activity_gate もトレイスレッドが反映済み)。
                    // ここでは設定を保存し、ウィンドウが表示中ならランタイム activity_gate は
                    // 「ウィンドウ表示中は止めない」の不変に戻す (設定は次回 minimize で効く)。
                    self.settings.pause_indexer_while_minimized = new_checked;
                    self.settings.save();
                    if self.window_visible {
                        self.activity_gate.set_paused(false);
                    }
                    self.update_tray_tooltip();
                }
                TrayEvent::QuitRequested => {
                    // 可視状態で Quit が押されたケース: トレイスレッドが既に
                    // PostMessage(WM_CLOSE) 済み。`maybe_intercept_close` が
                    // tc.is_quit_requested() を見て close を通すのでここは何もしない。
                    // (hidden 状態のときはトレイスレッドが直接 std::process::exit(0)
                    //  するので、そもそもこの event は届かない)
                }
            }
        }
    }
}
