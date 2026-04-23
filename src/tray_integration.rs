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
                        let paused = self.activity_gate.is_paused();
                        if let Some(tc) = &self.tray_controller {
                            tc.set_paused_check(paused);
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
        // メニュー「終了」経由の強制終了要求は常に通す (tray_quit_requested フラグ、または
        // トレイスレッドが直接立てた quit_flag のいずれかで判定)。
        let tray_wants_quit = self
            .tray_controller
            .as_ref()
            .is_some_and(|tc| tc.is_quit_requested());
        if self.tray_quit_requested || tray_wants_quit {
            self.tray_quit_requested = true; // 次回フレームでも通す
            return false;
        }
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if !close_requested {
            return false;
        }
        if !self.settings.minimize_to_tray_on_close || self.tray_controller.is_none() {
            return false;
        }
        // インターセプト: close をキャンセルしてウィンドウを Win32 レベルで非表示に。
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
        // 終了時と同じく、dirty なサイドカー・設定をこのタイミングで flush する
        // (電源断・クラッシュ時に失わないため)。inner_size が不明なら前回保存値を維持。
        // トレイメニューの「終了」は WM_QUIT 経路で on_exit を呼ばずに終わる可能性があるため、
        // hide のタイミングで確実に全 flush しておくことでデータロスを防ぐ。
        if let Some(rect) = self.last_outer_rect {
            self.settings.window_pos = Some([rect.min.x, rect.min.y]);
        }
        if let Some(size) = self.last_inner_size {
            self.settings.window_size = Some(size);
        }
        self.settings.save();
        self.flush_all_sidecars();

        // トレイの表示を更新
        self.update_tray_tooltip();
        crate::logger::log("tray: window hidden to tray (Win32 SW_HIDE, placement saved)");
    }

    /// タスクトレイから復帰した後の **App 側事後処理**。
    /// トレイスレッドの OpenRequested 経路と、外部 ShowWindow 検出経路の両方から呼ばれる。
    fn sync_after_restore(&mut self) {
        self.sync_after_restore_internal();
    }

    /// `update` から直接呼べる版。borrow チェッカ回避で `&mut self` のみ要求。
    pub(crate) fn sync_after_restore_internal(&mut self) {
        if self.window_visible {
            return;
        }
        self.window_visible = true;
        if let Some(mgr) = self.indexer_manager.as_ref() {
            mgr.set_io_throttled(false);
        }
        self.activity_gate.set_paused(false);
        self.update_tray_tooltip();
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
    pub(crate) fn poll_tray_events(&mut self, ctx: &egui::Context) {
        // borrow 分離: 一旦イベントを drain してから self のメソッドを呼ぶ。
        // controller は `self` の一部なので、ループ内で `&mut self` を触りたいと
        // immutable borrow と衝突する。
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
                    // トレイスレッドが既に Win32 ShowWindow + SetForegroundWindow を実行済み。
                    // App 側は状態同期だけ行う。
                    self.sync_after_restore();
                }
                TrayEvent::TogglePauseRequested => {
                    // トレイスレッドが既に activity_gate を反転済み。
                    // App 側は設定保存 + ツールチップ更新のみ。
                    let new_state = self.activity_gate.is_paused();
                    self.settings.pause_indexer_while_minimized = new_state;
                    self.settings.save();
                    self.update_tray_tooltip();
                }
                TrayEvent::QuitRequested => {
                    // トレイスレッドが quit_flag + PostMessage(WM_CLOSE) を既に実行済み。
                    // maybe_intercept_close が quit_flag を見て close を通すのでここは何もしない。
                    self.tray_quit_requested = true;
                    let _ = ctx; // ctx unused here (kept for signature uniformity)
                }
            }
        }
    }
}
