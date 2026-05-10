use super::*;

impl App {
    #[cfg(windows)]
    pub(super) fn ensure_native_video_front(&mut self) {
        if self.fullscreen_idx.is_none() {
            self.native_video_front_synced_hwnd = 0;
            self.native_video_front_last_raise = None;
            return;
        }
        let hwnd = self
            .fullscreen_idx
            .and_then(|idx| self.fs_cache.get(&idx))
            .and_then(|entry| match entry {
                FsCacheEntry::Video { player, .. } => {
                    let hwnd = player.native_presenter_hwnd();
                    (hwnd != 0).then_some(hwnd)
                }
                _ => None,
            })
            .unwrap_or(0);
        if hwnd == 0 {
            self.native_video_front_synced_hwnd = 0;
            self.native_video_front_last_raise = None;
            return;
        }
        let is_new_hwnd = hwnd != self.native_video_front_synced_hwnd;
        if !is_new_hwnd {
            return;
        }

        // The native HWND is created and raised on the presenter thread. Calling
        // SetWindowPos on that HWND from the egui UI thread can synchronously
        // cross into the presenter thread / DWM while HUD seek and overlay input
        // are active, which has produced UI-thread hangs. Owner z-order plus the
        // fullscreen black backdrop now cover the startup race, so the UI thread
        // only records the new HWND and leaves z-order mutation to its owner
        // thread.
        self.native_video_front_synced_hwnd = hwnd;
        self.native_video_front_last_raise = Some(std::time::Instant::now());
        crate::video::native_window::log_state(hwnd, "synced");
        crate::logger::log(format!(
            "[native-video] synced fullscreen presenter hwnd=0x{hwnd:x}"
        ));
    }

    #[cfg(windows)]
    pub(super) fn native_video_presenter_hwnd_for_focus_guard(&self) -> bool {
        self.fullscreen_idx
            .and_then(|idx| self.fs_cache.get(&idx))
            .is_some_and(|entry| match entry {
                FsCacheEntry::Video { player, .. } => {
                    player.native_presenter_hwnd() != 0 || player.native_presenter_pending()
                }
                _ => false,
            })
    }

    #[cfg(windows)]
    pub(super) fn native_video_presenter_hwnd(&self) -> Option<u64> {
        self.fullscreen_idx
            .and_then(|idx| self.fs_cache.get(&idx))
            .and_then(|entry| match entry {
                FsCacheEntry::Video { player, .. } => {
                    let hwnd = player.native_presenter_hwnd();
                    (hwnd != 0).then_some(hwnd)
                }
                _ => None,
            })
    }

    #[cfg(windows)]
    pub(super) fn native_video_fullscreen_active_for_main_backdrop(&self) -> bool {
        let Some(fs_idx) = self.fullscreen_idx else {
            return false;
        };
        matches!(self.items.get(fs_idx), Some(GridItem::Video(_)))
    }

    #[cfg(windows)]
    pub(super) fn sync_native_video_main_chrome(&mut self, active: bool) {
        if active {
            self.native_video_main_chrome_restore_at = None;
        }
        if active == self.native_video_main_chrome_black {
            return;
        }
        let Some(hwnd_raw) = self.main_hwnd else {
            return;
        };
        let hwnd = windows::Win32::Foundation::HWND(hwnd_raw as *mut _);
        if active {
            crate::dwm_transitions::set_window_chrome_black(hwnd);
        } else {
            let dark = matches!(
                crate::os_theme::resolve(self.settings.ui_theme),
                crate::os_theme::ResolvedTheme::Dark
            );
            crate::dwm_transitions::restore_window_chrome_for_theme(hwnd, dark);
        }
        self.native_video_main_chrome_black = active;
    }

    #[cfg(windows)]
    pub(super) fn schedule_native_video_main_chrome_restore(&mut self) {
        if self.native_video_main_chrome_black {
            self.native_video_main_chrome_restore_at =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(80));
        } else {
            self.native_video_main_chrome_restore_at = None;
        }
    }

    #[cfg(windows)]
    pub(super) fn process_native_video_main_chrome_restore(&mut self, ctx: &egui::Context) {
        let Some(deadline) = self.native_video_main_chrome_restore_at else {
            return;
        };
        let now = std::time::Instant::now();
        if self.fullscreen_idx.is_some() || self.fs_viewport_shown {
            self.native_video_main_chrome_restore_at =
                Some(now + std::time::Duration::from_millis(80));
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
            return;
        }
        if now >= deadline {
            self.native_video_main_chrome_restore_at = None;
            self.sync_native_video_main_chrome(false);
        } else {
            ctx.request_repaint_after(
                deadline
                    .saturating_duration_since(now)
                    .min(std::time::Duration::from_millis(16)),
            );
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_output_event(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        source_epoch: u64,
        event: crate::video::NativeVideoOutputEvent,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            crate::logger::log(format!(
                "[native-video] stale overlay event ignored: event_idx={fs_idx} current={:?}",
                self.fullscreen_idx
            ));
            return;
        }
        let current_epoch = self.fs_cache.get(&fs_idx).and_then(|entry| match entry {
            FsCacheEntry::Video { player, .. } => player.native_source_epoch(),
            _ => None,
        });
        if current_epoch != Some(source_epoch) {
            crate::logger::log(format!(
                "[native-video] stale overlay event ignored: event_idx={fs_idx} event_epoch={source_epoch} current_epoch={current_epoch:?}"
            ));
            return;
        }
        match event {
            crate::video::NativeVideoOutputEvent::Window(event) => {
                self.handle_native_video_window_event(ctx, fs_idx, event);
            }
            crate::video::NativeVideoOutputEvent::Seek { target_secs } => {
                self.handle_native_video_seek_command(ctx, fs_idx, target_secs);
            }
            crate::video::NativeVideoOutputEvent::TileSeek { target_secs } => {
                self.handle_native_video_tile_seek_command(ctx, fs_idx, target_secs);
            }
            crate::video::NativeVideoOutputEvent::WheelNavigate { delta } => {
                self.navigate_native_video_fullscreen(ctx, fs_idx, delta);
            }
            crate::video::NativeVideoOutputEvent::TileColumnsDelta { delta } => {
                self.adjust_native_video_tile_columns(ctx, fs_idx, delta);
            }
            crate::video::NativeVideoOutputEvent::RequestSeekThumbnail { target_secs } => {
                self.handle_native_video_request_seek_thumbnail(fs_idx, target_secs);
            }
            crate::video::NativeVideoOutputEvent::ToggleTileMode => {
                let screen = ctx.content_rect().size();
                self.toggle_video_tile_mode(fs_idx, screen);
                self.sync_native_video_tile_overlay(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::TogglePerfOverlay => {
                self.toggle_native_video_perf_overlay(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::ToggleVst3Gui => {
                self.toggle_native_video_vst3_gui();
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::CloseFullscreen => {
                self.close_fullscreen();
            }
            crate::video::NativeVideoOutputEvent::SetVst3PanelVisible { visible } => {
                self.set_native_video_vst3_panel_visible(visible);
                self.sync_native_video_vst3_panel(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::SetVst3VideoCompact { compact } => {
                self.set_native_video_vst3_compact(compact);
                self.sync_native_video_vst3_panel(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::Vst3ShowSlotGui { idx, path } => {
                self.show_native_video_vst3_slot_gui(idx, path);
                self.sync_native_video_vst3_panel(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::Vst3HideSlotGui { idx, path } => {
                self.hide_native_video_vst3_slot_gui(idx, path);
                self.sync_native_video_vst3_panel(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::Vst3SetBypass { idx, path, bypass } => {
                self.set_native_video_vst3_slot_bypass(idx, path, bypass);
                self.sync_native_video_vst3_panel(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::Vst3LoadChainSlot { slot_idx } => {
                self.load_vst3_chain_slot(slot_idx);
                self.sync_native_video_vst3_panel(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::Vst3SaveChainSlot { slot_idx } => {
                self.save_vst3_chain_slot(slot_idx);
                self.sync_native_video_vst3_panel(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::SeekToStartAndPlay => {
                self.handle_native_video_seek_to_start_command(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::TogglePlay => {
                self.handle_native_video_toggle_play_command(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::ToggleMute => {
                self.handle_native_video_toggle_mute_command(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::ToggleLoop => {
                self.handle_native_video_toggle_loop_command(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::SetVolume { volume, persist } => {
                self.handle_native_video_set_volume_command(ctx, fs_idx, volume, persist);
            }
            crate::video::NativeVideoOutputEvent::SetPlaybackSpeed { speed } => {
                self.handle_video_playback_speed_command(ctx, fs_idx, speed);
            }
            crate::video::NativeVideoOutputEvent::CopyFrameToClipboard => {
                self.copy_video_frame_to_clipboard(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::FrameStep { direction } => {
                self.step_video_frame(ctx, fs_idx, direction);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::AddBookmarkAt { target_secs } => {
                self.handle_native_video_add_bookmark_command(ctx, fs_idx, target_secs);
            }
            crate::video::NativeVideoOutputEvent::TogglePinAt { target_secs } => {
                self.handle_native_video_toggle_pin_command(ctx, fs_idx, target_secs);
            }
            crate::video::NativeVideoOutputEvent::SetBookmarkTitle { id, title } => {
                self.handle_native_video_set_bookmark_title_command(ctx, fs_idx, id, title);
            }
            crate::video::NativeVideoOutputEvent::DeleteBookmark { id } => {
                self.handle_native_video_delete_bookmark_command(ctx, fs_idx, id);
            }
            crate::video::NativeVideoOutputEvent::OpenExternalUrl { url } => {
                self.handle_native_video_open_external_url_command(ctx, fs_idx, url);
            }
            crate::video::NativeVideoOutputEvent::ToggleNormalize => {
                self.handle_toggle_normalize(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::DisableNormalize => {
                self.handle_disable_normalize(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::CancelNormalizeScan => {
                self.handle_cancel_normalize_scan(ctx, fs_idx);
            }
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_window_event(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        event: crate::video::native_window::NativeVideoWindowEvent,
    ) {
        if self.fullscreen_idx != Some(fs_idx) || self.ime_input_active() {
            return;
        }
        match event {
            crate::video::native_window::NativeVideoWindowEvent::KeyDown(key) => {
                self.handle_native_video_key_event(ctx, fs_idx, key);
            }
            crate::video::native_window::NativeVideoWindowEvent::KeyUp(_) => {}
            crate::video::native_window::NativeVideoWindowEvent::Text(_) => {}
            crate::video::native_window::NativeVideoWindowEvent::Ime(_) => {}
            crate::video::native_window::NativeVideoWindowEvent::MouseMove(mouse) => {
                if mouse.x < 340 {
                    self.sync_native_video_timeline_markers(fs_idx);
                }
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::native_window::NativeVideoWindowEvent::MouseButton(button) => {
                self.handle_native_video_mouse_button(ctx, fs_idx, button);
            }
            crate::video::native_window::NativeVideoWindowEvent::MouseWheel(wheel) => {
                if wheel.ctrl && self.video_tile_state.is_some() {
                    let delta = if wheel.delta > 0 { -1 } else { 1 };
                    self.adjust_native_video_tile_columns(ctx, fs_idx, delta);
                } else if !wheel.ctrl {
                    let delta = if wheel.delta < 0 { 1 } else { -1 };
                    self.navigate_native_video_fullscreen(ctx, fs_idx, delta);
                }
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::native_window::NativeVideoWindowEvent::MouseLeave => {
                self.native_video_pointer_down = None;
            }
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_seek_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        target_secs: f64,
    ) {
        if self.fullscreen_idx != Some(fs_idx) || self.ime_input_active() {
            return;
        }
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.seek(target_secs);
            self.mark_native_video_hud_activity(ctx);
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_tile_seek_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        target_secs: f64,
    ) {
        if self.fullscreen_idx != Some(fs_idx) || self.ime_input_active() {
            return;
        }
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            crate::logger::log(format!(
                "[native-video] tile seek command: idx={fs_idx} target={target_secs:.3} engine_state={} seek_serial={} playing={} pos={:.3} video_rx_len={} audio_rx_len={}",
                player.engine_state_name(),
                player.current_seek_serial(),
                player.is_playing(),
                player.position(),
                player.video_rx_len(),
                player.audio_rx_len()
            ));
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "native_presenter",
                    "tile_seek_command",
                    None,
                    0,
                    &[
                        ("idx", serde_json::Value::from(fs_idx as i64)),
                        ("target", serde_json::Value::from(target_secs)),
                        (
                            "engine_state",
                            serde_json::Value::from(player.engine_state_name()),
                        ),
                        (
                            "seek_serial",
                            serde_json::Value::from(player.current_seek_serial() as i64),
                        ),
                        ("playing", serde_json::Value::from(player.is_playing())),
                        ("position", serde_json::Value::from(player.position())),
                        (
                            "video_rx_len",
                            serde_json::Value::from(player.video_rx_len() as i64),
                        ),
                        (
                            "audio_rx_len",
                            serde_json::Value::from(player.audio_rx_len() as i64),
                        ),
                    ],
                );
            }
            player.seek(target_secs);
            self.video_tile_state = None;
            self.video_tile_swap_pending = None;
            player.set_native_tile_overlay(None);
            self.mark_native_video_hud_activity(ctx);
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_seek_to_start_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) {
        if self.fullscreen_idx != Some(fs_idx) || self.ime_input_active() {
            return;
        }
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.seek(0.0);
            if !player.is_playing() {
                player.toggle_play();
            }
            self.mark_native_video_hud_activity(ctx);
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_toggle_play_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) {
        if self.fullscreen_idx != Some(fs_idx) || self.ime_input_active() {
            return;
        }
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.toggle_play();
            self.mark_native_video_hud_activity(ctx);
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_open_external_url_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        url: String,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_playing(false);
        }
        crate::ui_helpers::open_url(&url);
        self.mark_native_video_hud_activity(ctx);
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_toggle_mute_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) {
        if self.fullscreen_idx != Some(fs_idx) || self.ime_input_active() {
            return;
        }
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_muted(!player.is_muted());
            self.mark_native_video_hud_activity(ctx);
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_toggle_loop_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) {
        if self.fullscreen_idx != Some(fs_idx) || self.ime_input_active() {
            return;
        }
        self.settings.video_loop = !self.settings.video_loop;
        self.settings.save();
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_loop_enabled(self.settings.video_loop);
            player.set_native_loop_enabled(self.settings.video_loop);
        }
        self.mark_native_video_hud_activity(ctx);
    }

    /// 音量ノーマライズ ボタン左クリック (3 状態モデル: Off → ON 化 / OnApplied → OFF 化 /
    /// OnUnmeasured → スキャン起動)。
    #[cfg(windows)]
    pub(super) fn handle_toggle_normalize(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        // [Scanning] 中はクリック無効
        if self.normalize_state.is_some() {
            return;
        }
        use crate::video::normalize_types::NormalizeUiState;
        // ── snapshot phase: self の借用を短くする ──
        let current_state = self
            .normalize_ui_states
            .get(&fs_idx)
            .copied()
            .unwrap_or(NormalizeUiState::Off);
        let target_milli = self.settings.clamped_audio_normalize_target_lufs_milli();
        let current_path: Option<PathBuf> = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => Some(player.path().to_path_buf()),
            _ => None,
        };
        let Some(current_path) = current_path else {
            return;
        };

        match current_state {
            NormalizeUiState::OnApplied { .. } => {
                // [OnApplied] → [Off]: グローバル OFF + 全 player に gain=1.0 即時適用
                self.disable_normalize_globally();
            }
            NormalizeUiState::OnUnmeasured => {
                // [OnUnmeasured] → [Scanning]: グローバル ON は維持、スキャン起動
                self.start_normalize_scan(fs_idx);
            }
            NormalizeUiState::Off => {
                // [Off] → [OnApplied] or [Scanning]: グローバル ON 化、現在動画 DB lookup
                self.settings.audio_normalize_enabled = true;
                self.settings.save();
                let lookup = self
                    .audio_normalize_db
                    .as_ref()
                    .and_then(|db| db.lookup(&current_path, target_milli));
                if let Some(result) = lookup {
                    self.apply_normalize_gain_db_to_player(fs_idx, result.gain_db);
                    self.normalize_ui_states.insert(
                        fs_idx,
                        NormalizeUiState::OnApplied {
                            gain_db: result.gain_db,
                        },
                    );
                } else {
                    self.start_normalize_scan(fs_idx);
                }
                // 他の動画にも反映 (ヒットしたものから順に適用)
                self.apply_normalize_to_all_videos_except(fs_idx, target_milli);
            }
            NormalizeUiState::Scanning => {
                // is_some() ガードで通常到達しない
            }
        }
        self.mark_native_video_hud_activity(ctx);
    }

    /// 音量ノーマライズ ボタン右クリック (どの状態からでもグローバル OFF 化、救済経路)。
    #[cfg(windows)]
    pub(super) fn handle_disable_normalize(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        // [Scanning] 中は無効
        if self.normalize_state.is_some() {
            return;
        }
        self.disable_normalize_globally();
        self.mark_native_video_hud_activity(ctx);
    }

    /// 進捗パネル × ボタン or ESC でキャンセル。
    /// take() で state を捨てて新規スキャン即開始可能にする。
    #[cfg(windows)]
    pub(super) fn handle_cancel_normalize_scan(&mut self, ctx: &egui::Context, fs_idx: usize) {
        let should_drop = self
            .normalize_state
            .as_ref()
            .map(|s| s.fs_idx == fs_idx)
            .unwrap_or(false);
        if !should_drop {
            return;
        }
        if let Some(state) = self.normalize_state.take() {
            state.cancel();
            // 元再生状態に復帰
            if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&state.fs_idx) {
                if state.was_playing {
                    player.set_playing(true);
                }
            }
            self.normalize_ui_states.insert(
                state.fs_idx,
                crate::video::normalize_types::NormalizeUiState::OnUnmeasured,
            );
            // worker は cancel atomic を見て早期 return、_join + rx も drop で解放される
        }
        self.mark_native_video_hud_activity(ctx);
    }

    /// 全 fs_cache の VideoPlayer に gain=1.0 を即時適用 + Settings 保存。
    /// DB エントリは残す (= 次回 ON 復帰で即適用できる)。
    #[cfg(windows)]
    pub(super) fn disable_normalize_globally(&mut self) {
        use crate::video::normalize_types::NormalizeUiState;
        self.settings.audio_normalize_enabled = false;
        self.settings.save();
        let fs_idxs: Vec<usize> = self.fs_cache.keys().copied().collect();
        for idx in fs_idxs {
            if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&idx) {
                // Codex P2: set → clear の順 (clear → set だと clear と set の間に
                // pump が回って旧 gain で processed 再生成される小窓がある)。
                player.set_normalize_gain(1.0);
                player.clear_audio_output_buffer();
                self.normalize_ui_states.insert(idx, NormalizeUiState::Off);
            }
        }
    }

    /// 1 player に gain_db を線形変換して適用。clear_audio_output_buffer も呼ぶ。
    #[cfg(windows)]
    pub(super) fn apply_normalize_gain_db_to_player(&mut self, fs_idx: usize, gain_db: f32) {
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            let linear = 10.0_f64.powf(gain_db as f64 / 20.0);
            // Codex P2: set → clear の順 (旧 gain processed の残存防止)
            player.set_normalize_gain(linear);
            player.clear_audio_output_buffer();
        }
    }

    /// 他の fs_cache entry (= except_fs_idx 以外) について DB lookup → ヒットなら適用、
    /// ミスなら OnUnmeasured 設定。トグル ON 時の同期適用に使う。
    #[cfg(windows)]
    pub(super) fn apply_normalize_to_all_videos_except(
        &mut self,
        except_fs_idx: usize,
        target_milli: i32,
    ) {
        use crate::video::normalize_types::NormalizeUiState;
        let other_idxs: Vec<usize> = self
            .fs_cache
            .keys()
            .copied()
            .filter(|i| *i != except_fs_idx)
            .collect();
        for idx in other_idxs {
            let path = match self.fs_cache.get(&idx) {
                Some(FsCacheEntry::Video { player, .. }) => Some(player.path().to_path_buf()),
                _ => None,
            };
            let Some(path) = path else { continue };
            let lookup = self
                .audio_normalize_db
                .as_ref()
                .and_then(|db| db.lookup(&path, target_milli));
            match lookup {
                Some(result) => {
                    self.apply_normalize_gain_db_to_player(idx, result.gain_db);
                    self.normalize_ui_states.insert(
                        idx,
                        NormalizeUiState::OnApplied {
                            gain_db: result.gain_db,
                        },
                    );
                }
                None => {
                    self.normalize_ui_states
                        .insert(idx, NormalizeUiState::OnUnmeasured);
                }
            }
        }
    }

    /// スキャン worker thread を起動。再生中なら一時停止 → スキャン → poll で完了検知。
    #[cfg(windows)]
    pub(super) fn start_normalize_scan(&mut self, fs_idx: usize) {
        use crate::video::normalize_types::NormalizeUiState;
        let (path, was_playing) = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                (player.path().to_path_buf(), player.is_playing())
            }
            _ => return,
        };
        // 既存 state を捨てる (cancel を立てておく) — 通常は is_some() で弾かれているが defensive
        if let Some(prev) = self.normalize_state.take() {
            prev.cancel();
        }
        // 再生中なら一時停止
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            if was_playing {
                player.set_playing(false);
            }
        }
        let target_milli = self.settings.clamped_audio_normalize_target_lufs_milli();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(crate::video::normalize_scanner::NormalizeScanProgress::default());
        let (tx, rx) = mpsc::channel();
        let cancel_clone = cancel.clone();
        let progress_clone = progress.clone();
        let path_clone = path.clone();
        let join = std::thread::Builder::new()
            .name("normalize-scan".to_string())
            .spawn(move || {
                let result = crate::video::normalize_scanner::scan_audio_loudness(
                    &path_clone,
                    target_milli,
                    cancel_clone,
                    progress_clone,
                );
                let _ = tx.send(crate::app::normalize::NormalizeMessage::from(result));
            });
        let join = match join {
            Ok(j) => j,
            Err(e) => {
                crate::logger::log(format!("normalize-scan thread spawn failed: {e}"));
                // Codex P2: spawn 失敗時は元再生状態に戻し、UI 状態も OnUnmeasured に
                if was_playing {
                    if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                        player.set_playing(true);
                    }
                }
                self.normalize_ui_states
                    .insert(fs_idx, NormalizeUiState::OnUnmeasured);
                return;
            }
        };
        self.normalize_state = Some(crate::app::normalize::NormalizeScanState {
            fs_idx,
            cancel,
            progress,
            rx,
            was_playing,
            file_path: path,
            target_lufs_milli: target_milli,
            _join: join,
        });
        self.normalize_ui_states
            .insert(fs_idx, NormalizeUiState::Scanning);
    }

    /// スキャン完了 / キャンセル / エラーを検知して後処理する。`App::update` から毎フレーム呼ぶ。
    #[cfg(windows)]
    pub(super) fn poll_normalize_scan(&mut self, _ctx: &egui::Context) {
        use crate::video::normalize_types::NormalizeUiState;
        // 1. メッセージ peek (try_recv)
        let msg = match self.normalize_state.as_ref() {
            Some(state) => match state.rx.try_recv() {
                Ok(msg) => Some(Ok(msg)),
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(())),
            },
            None => return,
        };
        // 2. 完了確定: state を所有してから後処理
        let Some(state) = self.normalize_state.take() else {
            return;
        };
        let target_milli = state.target_lufs_milli;
        // 3. stale fs_idx 復活防止
        let still_valid = match self.fs_cache.get(&state.fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player.path() == state.file_path.as_path(),
            _ => false,
        };
        match msg {
            Some(Ok(crate::app::normalize::NormalizeMessage::Done(result))) => {
                // 測定値はファイル単位なので、stale でも DB に保存しておく (= 次回開いたとき即適用)
                if let Some(db) = self.audio_normalize_db.as_ref() {
                    let _ = db.upsert(&state.file_path, &result);
                }
                if still_valid {
                    if let Some(FsCacheEntry::Video { player, .. }) =
                        self.fs_cache.get(&state.fs_idx)
                    {
                        let linear = 10.0_f64.powf(result.gain_db as f64 / 20.0);
                        // Codex P2: set → clear の順 (旧 gain processed の残存防止)
                        player.set_normalize_gain(linear);
                        player.clear_audio_output_buffer();
                        if state.was_playing {
                            player.set_playing(true);
                        }
                    }
                    self.normalize_ui_states.insert(
                        state.fs_idx,
                        NormalizeUiState::OnApplied {
                            gain_db: result.gain_db,
                        },
                    );
                }
                let _ = target_milli; // suppress unused warning
            }
            Some(Ok(crate::app::normalize::NormalizeMessage::Cancelled))
            | Some(Ok(crate::app::normalize::NormalizeMessage::Error(_)))
            | Some(Err(())) => {
                if let Some(Ok(crate::app::normalize::NormalizeMessage::Error(ref m))) = msg {
                    crate::logger::log(format!("normalize-scan error: {m}"));
                }
                // DB に書かない、グローバル ON は維持、UI 状態を OnUnmeasured に戻す
                if still_valid {
                    if let Some(FsCacheEntry::Video { player, .. }) =
                        self.fs_cache.get(&state.fs_idx)
                    {
                        if state.was_playing {
                            player.set_playing(true);
                        }
                    }
                    self.normalize_ui_states
                        .insert(state.fs_idx, NormalizeUiState::OnUnmeasured);
                }
            }
            None => {
                // unreachable - try_recv が Empty なら return 済み、Disconnected なら Some(Err(()))
            }
        }
    }

    /// fs_idx 単位の normalize state を cleanup (close_fullscreen / fs_cache evict 時に呼ぶ)。
    #[cfg(windows)]
    pub(super) fn cleanup_normalize_state_for_fs_idx(&mut self, fs_idx: usize) {
        self.normalize_ui_states.remove(&fs_idx);
        // 同 fs_idx のスキャン中なら state を持ち去って捨てる (= 新規スキャン即開始可能に)
        let should_drop = self
            .normalize_state
            .as_ref()
            .map(|s| s.fs_idx == fs_idx)
            .unwrap_or(false);
        if should_drop {
            if let Some(state) = self.normalize_state.take() {
                state.cancel();
            }
        }
    }

    /// 動画 open 時の自動適用。Settings ON + DB ヒットなら gain を即適用、ミスなら
    /// OnUnmeasured 表示。OFF なら Off 状態で初期化。
    #[cfg(windows)]
    pub(super) fn init_normalize_state_for_opened_video(&mut self, fs_idx: usize) {
        use crate::video::normalize_types::NormalizeUiState;
        let path = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player.path().to_path_buf(),
            _ => return,
        };
        let target_milli = self.settings.clamped_audio_normalize_target_lufs_milli();
        let ui_state = if self.settings.audio_normalize_enabled {
            let lookup = self
                .audio_normalize_db
                .as_ref()
                .and_then(|db| db.lookup(&path, target_milli));
            if let Some(result) = lookup {
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    let linear = 10.0_f64.powf(result.gain_db as f64 / 20.0);
                    // 再生開始前なので flush 不要
                    player.set_normalize_gain(linear);
                }
                NormalizeUiState::OnApplied {
                    gain_db: result.gain_db,
                }
            } else {
                NormalizeUiState::OnUnmeasured
            }
        } else {
            NormalizeUiState::Off
        };
        self.normalize_ui_states.insert(fs_idx, ui_state);
    }

    /// native overlay にノーマライズ UI 状態 + 進捗 snapshot を配信する。
    /// `App::update` から毎フレーム呼ぶ。
    #[cfg(windows)]
    pub(super) fn sync_native_video_normalize_state(&self, fs_idx: usize) {
        use crate::video::normalize_types::{
            NormalizeOverlayState, NormalizeProgressSnapshot, NormalizeUiState,
        };
        let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) else {
            return;
        };
        let ui_state = self
            .normalize_ui_states
            .get(&fs_idx)
            .copied()
            .unwrap_or(NormalizeUiState::Off);
        let progress = self
            .normalize_state
            .as_ref()
            .filter(|s| s.fs_idx == fs_idx)
            .map(|s| NormalizeProgressSnapshot {
                pts_processed_ms: s
                    .progress
                    .pts_processed_ms
                    .load(std::sync::atomic::Ordering::Acquire),
                duration_ms: s
                    .progress
                    .duration_ms
                    .load(std::sync::atomic::Ordering::Acquire),
                indeterminate: s
                    .progress
                    .indeterminate
                    .load(std::sync::atomic::Ordering::Acquire),
            });
        player.set_native_normalize_state(NormalizeOverlayState { ui_state, progress });
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_set_volume_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        volume: f64,
        persist: bool,
    ) {
        if self.fullscreen_idx != Some(fs_idx) || self.ime_input_active() {
            return;
        }
        let volume = if volume.is_finite() {
            crate::settings::clamp_video_volume(volume)
        } else {
            return;
        };
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_volume(volume);
            self.settings.video_volume = volume;
            if persist {
                self.settings.save();
            }
            self.mark_native_video_hud_activity(ctx);
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_video_playback_speed_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        speed: f64,
    ) {
        if self.fullscreen_idx != Some(fs_idx) || self.ime_input_active() {
            return;
        }
        let speed = crate::video::clock::clamp_playback_speed(speed);
        self.video_playback_speed = speed;
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_playback_speed(speed);
            self.mark_native_video_hud_activity(ctx);
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_request_seek_thumbnail(
        &mut self,
        fs_idx: usize,
        target_secs: f64,
    ) {
        if self.fullscreen_idx != Some(fs_idx) || self.ime_input_active() {
            return;
        }
        let path = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. })
                if player.error().is_none()
                    && player.info().is_some()
                    && target_secs.is_finite() =>
            {
                let target_secs = target_secs.max(0.0);
                player.request_native_hover_thumbnail(target_secs);
                Some(player.path().clone())
            }
            _ => None,
        };
        let Some(path) = path else {
            return;
        };
        self.ensure_fullscreen_video_marker_cache(fs_idx);
        let pinned = self
            .fullscreen_video_marker_snapshot(fs_idx, &path)
            .0
            .is_some();
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_native_hover_preview_pinned(pinned);
        }
        self.sync_native_video_timeline_markers(fs_idx);
    }

    #[cfg(windows)]
    pub(crate) fn sync_native_video_timeline_markers(&mut self, fs_idx: usize) {
        self.ensure_fullscreen_video_marker_cache(fs_idx);
        let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) else {
            return;
        };
        if player.error().is_some() {
            return;
        };
        let path = player.path().clone();
        let chapters = player
            .info()
            .map(|info| info.chapters.clone())
            .unwrap_or_default();
        let mut markers: Vec<crate::video::native_presenter::NativeOverlayTimelineMarker> =
            Vec::new();
        let mut entries: Vec<crate::video::native_presenter::NativeOverlayJumpEntry> = Vec::new();
        let (pin_pts, bookmarks) = self.fullscreen_video_marker_snapshot(fs_idx, &path);

        let requested_thumb = std::cell::Cell::new(false);
        let make_thumbnail =
            |pts_secs: f64| -> Option<crate::video::native_presenter::NativeOverlayThumbnail> {
                if let Some(thumb) = player.nearest_seek_thumbnail(pts_secs) {
                    Some(crate::video::native_presenter::NativeOverlayThumbnail {
                        target_secs: thumb.target_secs,
                        width: thumb.width,
                        height: thumb.height,
                        rgba: thumb.rgba,
                    })
                } else {
                    if !requested_thumb.replace(true) {
                        player.request_seek_thumbnail(pts_secs);
                    }
                    None
                }
            };

        if let Some(pts_secs) = pin_pts {
            markers.push(
                crate::video::native_presenter::NativeOverlayTimelineMarker {
                    pts_secs,
                    kind: crate::video::native_presenter::NativeOverlayTimelineMarkerKind::Pin,
                },
            );
            entries.push(crate::video::native_presenter::NativeOverlayJumpEntry {
                pts_secs,
                kind: crate::video::native_presenter::NativeOverlayTimelineMarkerKind::Pin,
                title: Some("代表フレーム".to_string()),
                bookmark_id: None,
                thumbnail: make_thumbnail(pts_secs),
            });
        }
        for bookmark in bookmarks {
            markers.push(
                crate::video::native_presenter::NativeOverlayTimelineMarker {
                    pts_secs: bookmark.pts_secs,
                    kind: crate::video::native_presenter::NativeOverlayTimelineMarkerKind::Bookmark,
                },
            );
            entries.push(crate::video::native_presenter::NativeOverlayJumpEntry {
                pts_secs: bookmark.pts_secs,
                kind: crate::video::native_presenter::NativeOverlayTimelineMarkerKind::Bookmark,
                title: bookmark.title.clone(),
                bookmark_id: Some(bookmark.id),
                thumbnail: make_thumbnail(bookmark.pts_secs),
            });
        }
        for chapter in chapters {
            markers.push(
                crate::video::native_presenter::NativeOverlayTimelineMarker {
                    pts_secs: chapter.start_secs,
                    kind: crate::video::native_presenter::NativeOverlayTimelineMarkerKind::Chapter,
                },
            );
            entries.push(crate::video::native_presenter::NativeOverlayJumpEntry {
                pts_secs: chapter.start_secs,
                kind: crate::video::native_presenter::NativeOverlayTimelineMarkerKind::Chapter,
                title: chapter.title.clone(),
                bookmark_id: None,
                thumbnail: make_thumbnail(chapter.start_secs),
            });
        }
        markers.retain(|marker| marker.pts_secs.is_finite() && marker.pts_secs >= 0.0);
        markers.sort_by(|a, b| {
            a.pts_secs
                .partial_cmp(&b.pts_secs)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        entries.retain(|entry| entry.pts_secs.is_finite() && entry.pts_secs >= 0.0);
        entries.sort_by(|a, b| {
            a.pts_secs
                .partial_cmp(&b.pts_secs)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        player.set_native_timeline_markers(markers);
        player.set_native_jump_entries(entries);
    }

    #[cfg(windows)]
    pub(super) fn sync_native_video_metadata(&self, fs_idx: usize) {
        let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) else {
            return;
        };
        let metadata = player.info().map(|info| {
            let file_name = player
                .path()
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("video")
                .to_string();
            crate::video::native_presenter::NativeOverlayMetadata {
                file_name,
                title: info.title.clone(),
                artist: info.artist.clone(),
                original_url: info.original_url.clone(),
                description: info.description.clone(),
                width: info.width,
                height: info.height,
                duration_secs: info.duration_secs,
                video_codec: info.video_codec.clone(),
                video_decoder: info.video_decoder.clone(),
                audio_codec: info.audio_codec.clone(),
                avg_fps: info.avg_fps,
                bit_rate_bps: info.bit_rate_bps,
                chapter_count: info.chapters.len(),
                hw_decode_active: info.hw_decode_active,
                gpu_path_active: info.gpu_path_active,
                d3d11va_supported: info.d3d11va_supported,
            }
        });
        player.set_native_metadata(metadata);
    }

    #[cfg(windows)]
    pub(super) fn sync_native_video_vst3_available(&self, fs_idx: usize) {
        let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) else {
            return;
        };
        player.set_native_vst3_available(self.settings.vst3_enabled);
        player.set_native_video_compact(
            self.settings.vst3_enabled
                && self.settings.vst3_gui_visible
                && self.settings.vst3_video_compact,
        );
    }

    #[cfg(windows)]
    pub(super) fn sync_native_video_vst3_panel(&self, fs_idx: usize) {
        let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) else {
            return;
        };
        let panel = self.build_native_video_vst3_panel();
        player.set_native_vst3_panel(panel);
    }

    #[cfg(windows)]
    pub(super) fn build_native_video_vst3_panel(
        &self,
    ) -> Option<crate::video::native_presenter::NativeOverlayVst3Panel> {
        if !self.settings.vst3_enabled || !self.show_vst3_manager {
            return None;
        }
        use crate::video::dsp::{DspState, SlotState};
        use crate::video::native_presenter::{
            NativeOverlayVst3ChainSlot, NativeOverlayVst3Panel, NativeOverlayVst3Slot,
            NativeOverlayVst3SlotState,
        };

        let bridge_state = self.dsp_bridge.state();
        let state_text = match bridge_state {
            DspState::Disabled => "disabled".to_string(),
            DspState::Enabled => "enabled".to_string(),
            DspState::Error(err) => format!("error: {err}"),
        };
        let disabled_reason = if bridge_state == DspState::Disabled {
            self.dsp_bridge.session_disabled_reason()
        } else {
            None
        };
        let sample_rate = self.dsp_bridge.sample_rate();
        let bridge_slots = self.dsp_bridge.slots();
        let display_count = bridge_slots.len().max(self.settings.vst3_plugins.len());
        let plugin_label = |path: &str| -> String {
            std::path::Path::new(path)
                .file_stem()
                .or_else(|| std::path::Path::new(path).file_name())
                .map(|name| name.to_string_lossy().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "(unknown)".to_string())
        };
        let mut slots = Vec::with_capacity(display_count);
        for idx in 0..display_count {
            if let Some(slot) = bridge_slots.get(idx) {
                let state = match slot.state {
                    SlotState::Loading => NativeOverlayVst3SlotState::Loading,
                    SlotState::Loaded => NativeOverlayVst3SlotState::Loaded,
                    SlotState::Error => NativeOverlayVst3SlotState::Error,
                };
                let latency_ms = if sample_rate > 0 && slot.latency_samples > 0 {
                    Some(slot.latency_samples as f64 / sample_rate as f64 * 1000.0)
                } else {
                    None
                };
                slots.push(NativeOverlayVst3Slot {
                    idx,
                    path: slot.plugin_path.clone(),
                    name: slot
                        .plugin_name
                        .clone()
                        .unwrap_or_else(|| plugin_label(&slot.plugin_path)),
                    state,
                    bypass: slot.bypass,
                    gui_visible: slot.gui_visible,
                    latency_ms,
                    auto_bypassed_for_latency: slot.auto_bypassed_for_latency,
                    placeholder: false,
                });
            } else if let Some(entry) = self.settings.vst3_plugins.get(idx) {
                slots.push(NativeOverlayVst3Slot {
                    idx,
                    path: entry.path.clone(),
                    name: plugin_label(&entry.path),
                    state: NativeOverlayVst3SlotState::Placeholder,
                    bypass: entry.bypass,
                    gui_visible: !entry.user_hidden,
                    latency_ms: None,
                    auto_bypassed_for_latency: false,
                    placeholder: true,
                });
            }
        }

        let chain_slots = self
            .settings
            .vst3_chain_slots
            .slots
            .iter()
            .enumerate()
            .map(|(idx, slot)| NativeOverlayVst3ChainSlot {
                idx,
                key_label: crate::adjustment::slot_key_label(idx),
                name: slot.as_ref().map(|slot| slot.name.clone()),
                plugin_count: slot.as_ref().map(|slot| slot.plugins.len()).unwrap_or(0),
            })
            .collect();

        Some(NativeOverlayVst3Panel {
            visible: true,
            video_compact: self.settings.vst3_video_compact,
            state_text,
            disabled_reason,
            slots,
            chain_slots,
        })
    }

    #[cfg(windows)]
    pub(super) fn poll_video_tile_swap(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.video_tile_swap_pending.as_ref() else {
            return;
        };
        let target_idx = pending.target_idx;
        let target_path = pending.target_path.clone();
        let source_epoch = pending.source_epoch;
        let started_at = pending.started_at;
        let deadline = pending.deadline;
        if self.fullscreen_idx != Some(target_idx) {
            self.video_tile_swap_pending = None;
            return;
        }
        let now = std::time::Instant::now();
        enum SwapStatus {
            Ready,
            Pending,
            Timeout,
            Error,
            Missing,
        }
        let status = match self.fs_cache.get(&target_idx) {
            Some(FsCacheEntry::Video { player, .. }) if player.path() == &target_path => {
                if player.error().is_some() {
                    SwapStatus::Error
                } else if player.info().is_some() {
                    SwapStatus::Ready
                } else if now >= deadline {
                    SwapStatus::Timeout
                } else {
                    SwapStatus::Pending
                }
            }
            _ => SwapStatus::Missing,
        };
        match status {
            SwapStatus::Ready => {
                let screen = ctx.content_rect().size();
                self.video_tile_state = self.build_video_tile_state_for(target_idx, screen);
                self.video_tile_swap_pending = None;
                self.video_tile_reopen_pending = false;
                self.video_tile_reopen_deadline = None;
                self.sync_native_video_tile_overlay(ctx, target_idx);
                crate::logger::log(format!(
                    "[native-video] fast tile swap ready: idx={target_idx} epoch={source_epoch} elapsed_ms={:.1}",
                    started_at.elapsed().as_secs_f64() * 1000.0
                ));
                ctx.request_repaint();
            }
            SwapStatus::Pending => {
                self.set_native_video_tile_preparing_overlay(target_idx);
                ctx.request_repaint_after(std::time::Duration::from_millis(33));
            }
            SwapStatus::Timeout => {
                self.video_tile_swap_pending = None;
                self.video_tile_state = None;
                self.video_tile_reopen_pending = true;
                self.video_tile_reopen_deadline = Some(now + std::time::Duration::from_secs(3));
                self.set_native_video_tile_preparing_overlay(target_idx);
                crate::logger::log(format!(
                    "[native-video] fast tile swap timeout: idx={target_idx} epoch={source_epoch} elapsed_ms={:.1}",
                    started_at.elapsed().as_secs_f64() * 1000.0
                ));
                ctx.request_repaint();
            }
            SwapStatus::Error | SwapStatus::Missing => {
                self.video_tile_swap_pending = None;
                self.video_tile_state = None;
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&target_idx) {
                    player.set_native_tile_overlay(None);
                }
                ctx.request_repaint();
            }
        }
    }

    #[cfg(windows)]
    pub(super) fn sync_native_video_tile_overlay(&mut self, ctx: &egui::Context, fs_idx: usize) {
        let current_path = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) if player.error().is_none() => {
                Some(player.path().clone())
            }
            _ => None,
        };
        let Some(current_path) = current_path else {
            return;
        };

        if self.video_tile_reopen_pending && self.video_tile_state.is_none() {
            let now = std::time::Instant::now();
            let deadline = *self
                .video_tile_reopen_deadline
                .get_or_insert_with(|| now + std::time::Duration::from_secs(3));
            if now >= deadline {
                self.video_tile_reopen_pending = false;
                self.video_tile_reopen_deadline = None;
            } else {
                let screen = ctx.content_rect().size();
                self.toggle_video_tile_mode(fs_idx, screen);
                if self.video_tile_state.is_some() {
                    self.video_tile_reopen_pending = false;
                    self.video_tile_reopen_deadline = None;
                } else {
                    ctx.request_repaint_after(
                        deadline
                            .saturating_duration_since(now)
                            .min(std::time::Duration::from_millis(80)),
                    );
                }
            }
        }

        let swap_pending_for_current =
            self.video_tile_swap_pending
                .as_ref()
                .is_some_and(|pending| {
                    pending.target_idx == fs_idx && pending.target_path == current_path
                });
        let mut clear_state = false;
        let tile_overlay = if let Some(state) = self.video_tile_state.as_ref() {
            if state.video_path != current_path {
                if swap_pending_for_current {
                    Some(Self::native_video_tile_preparing_overlay())
                } else {
                    clear_state = true;
                    None
                }
            } else {
                let snapshot = state.worker.snapshot();
                let (progress_done, progress_total) = state.worker.progress();
                let finished = state.worker.is_finished();
                if !finished {
                    ctx.request_repaint_after(std::time::Duration::from_millis(80));
                }
                let tiles = snapshot
                    .into_iter()
                    .map(|slot| {
                        slot.map(|thumb| {
                            crate::video::native_presenter::NativeOverlayTileThumbnail {
                                target_secs: thumb.pts_secs,
                                width: thumb.width,
                                height: thumb.height,
                                rgba: thumb.rgba,
                            }
                        })
                    })
                    .collect();
                Some(crate::video::native_presenter::NativeOverlayTileOverlay {
                    interval_secs: state.interval_secs,
                    timestamps: state.timestamps.clone(),
                    tile_w: state.tile_w,
                    tile_h: state.tile_h,
                    columns: state.columns,
                    progress_done,
                    progress_total,
                    finished,
                    tiles,
                })
            }
        } else if swap_pending_for_current || self.video_tile_reopen_pending {
            Some(Self::native_video_tile_preparing_overlay())
        } else {
            None
        };

        if clear_state {
            self.video_tile_state = None;
            self.video_tile_swap_pending = None;
        }
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_native_tile_overlay(tile_overlay);
        }
    }

    #[cfg(windows)]
    pub(super) fn native_video_tile_preparing_overlay()
    -> crate::video::native_presenter::NativeOverlayTileOverlay {
        crate::video::native_presenter::NativeOverlayTileOverlay::preparing()
    }

    #[cfg(windows)]
    pub(super) fn set_native_video_tile_preparing_overlay(&self, fs_idx: usize) {
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_native_tile_overlay(Some(Self::native_video_tile_preparing_overlay()));
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_add_bookmark_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        target_secs: f64,
    ) {
        if self.fullscreen_idx != Some(fs_idx) || self.ime_input_active() {
            return;
        }
        self.add_native_video_bookmark(fs_idx, Some(target_secs));
        self.mark_native_video_hud_activity(ctx);
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_toggle_pin_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        target_secs: f64,
    ) {
        if self.fullscreen_idx != Some(fs_idx) || self.ime_input_active() {
            return;
        }
        self.toggle_native_video_pin(fs_idx, target_secs);
        self.mark_native_video_hud_activity(ctx);
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_delete_bookmark_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        id: i64,
    ) {
        if self.fullscreen_idx != Some(fs_idx) || self.ime_input_active() {
            return;
        }
        if let Some(db) = self.video_bookmark_db.as_ref() {
            if let Err(e) = db.remove(id) {
                crate::logger::log(format!("video bookmark remove failed: {e}"));
            } else {
                self.refresh_fullscreen_video_marker_cache(fs_idx);
                self.sync_native_video_timeline_markers(fs_idx);
            }
        }
        self.mark_native_video_hud_activity(ctx);
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_set_bookmark_title_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        id: i64,
        title: String,
    ) {
        if self.fullscreen_idx != Some(fs_idx) || self.ime_input_active() {
            return;
        }
        if let Some(db) = self.video_bookmark_db.as_ref() {
            if let Err(e) = db.update_title(id, Some(&title)) {
                crate::logger::log(format!("video bookmark title update failed: {e}"));
            } else {
                self.refresh_fullscreen_video_marker_cache(fs_idx);
                self.sync_native_video_timeline_markers(fs_idx);
            }
        }
        self.mark_native_video_hud_activity(ctx);
    }

    #[cfg(windows)]
    pub(super) fn add_native_video_bookmark(&mut self, fs_idx: usize, target_secs: Option<f64>) {
        let snapshot = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                if player.error().is_some() || player.info().is_none() {
                    None
                } else {
                    let pts = target_secs.unwrap_or_else(|| player.position());
                    Some((
                        player.path().clone(),
                        finite_video_target_secs(pts, player.duration()),
                    ))
                }
            }
            _ => None,
        };
        if let (Some((path, pts)), Some(db)) = (snapshot, self.video_bookmark_db.as_ref()) {
            if let Err(e) = db.add(&path, pts, None, &[]) {
                crate::logger::log(format!("video bookmark add failed: {e}"));
            } else {
                crate::logger::log(format!(
                    "video bookmark added: pts={pts:.2}s {}",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                ));
                self.refresh_fullscreen_video_marker_cache(fs_idx);
                self.sync_native_video_timeline_markers(fs_idx);
            }
        }
    }

    #[cfg(windows)]
    pub(super) fn toggle_native_video_pin(&mut self, fs_idx: usize, target_secs: f64) {
        let snapshot = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                if player.error().is_some() || player.info().is_none() {
                    None
                } else {
                    let pts = finite_video_target_secs(target_secs, player.duration());
                    player.request_seek_thumbnail(pts);
                    let thumb = player.nearest_seek_thumbnail(pts);
                    Some((player.path().clone(), pts, thumb))
                }
            }
            _ => None,
        };
        let Some((path, pts, thumb)) = snapshot else {
            return;
        };
        let Some(db) = self.video_pin_db.as_ref() else {
            crate::logger::log("video pin: DB not open".to_string());
            return;
        };
        if db.lookup(&path).is_some() {
            match db.remove(&path) {
                Ok(()) => {
                    self.video_thumb_overrides_dirty = true;
                    if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                        player.set_native_hover_preview_pinned(false);
                    }
                    self.refresh_fullscreen_video_marker_cache(fs_idx);
                    self.sync_native_video_timeline_markers(fs_idx);
                    crate::logger::log(format!(
                        "video pin removed: {}",
                        path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                    ));
                }
                Err(e) => crate::logger::log(format!("video pin remove failed: {e}")),
            }
            return;
        }
        let webp = thumb
            .as_ref()
            .map(|t| {
                let encoder = webp::Encoder::from_rgba(&t.rgba, t.width, t.height);
                encoder.encode(75.0).to_vec()
            })
            .unwrap_or_default();
        let webp_len = webp.len();
        match db.set_pin(&path, pts, &webp) {
            Ok(()) => {
                self.video_thumb_overrides_dirty = true;
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    player.set_native_hover_preview_pinned(true);
                }
                self.refresh_fullscreen_video_marker_cache(fs_idx);
                self.sync_native_video_timeline_markers(fs_idx);
                crate::logger::log(format!(
                    "video pin set: pts={pts:.2}s webp={}B {}",
                    webp_len,
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                ));
            }
            Err(e) => crate::logger::log(format!("video pin set failed: {e}")),
        }
    }

    #[cfg(windows)]
    pub(crate) fn handle_native_video_key_event(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        key: crate::video::native_window::NativeVideoKeyEvent,
    ) {
        if key.alt {
            return;
        }
        // Codex 5周目 P1: ノーマライズスキャン中はモーダル動作のため、ESC (cancel) 以外の
        // キー入力 (Enter で再生再開、S で tile mode、B でブックマーク等) を全て遮断する。
        // ESC だけ下の match に流す。
        if self
            .normalize_state
            .as_ref()
            .map(|s| s.fs_idx == fs_idx)
            .unwrap_or(false)
            && !(key.virtual_key == 0x1B && !key.repeat)
        {
            return;
        }
        let mut hud_activity = true;
        match key.virtual_key {
            // Shift+Enter: open in external player, matching the legacy egui
            // fullscreen video path.
            0x0D if key.shift && !key.ctrl && !key.repeat => {
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    crate::ui_helpers::open_external_player(player.path());
                }
            }
            // Enter: play / pause.
            0x0D if !key.shift && !key.ctrl && !key.repeat => {
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    player.toggle_play();
                }
            }
            // Escape: close native fullscreen. If the native overlay has a text
            // editor focused this key is not forwarded here, so dialog editing
            // does not accidentally close the fullscreen window.
            // Codex P1 反映: ノーマライズスキャン中の ESC は cancel に優先ルーティング。
            // (overlay 側 progress UI のキャンセルボタン押下と等価)
            0x1B if !key.repeat => {
                if self
                    .normalize_state
                    .as_ref()
                    .map(|s| s.fs_idx == fs_idx)
                    .unwrap_or(false)
                {
                    self.handle_cancel_normalize_scan(ctx, fs_idx);
                } else if self.close_video_tile_mode() {
                    self.sync_native_video_tile_overlay(ctx, fs_idx);
                } else {
                    self.close_fullscreen();
                }
            }
            // W: seek to start and play.
            0x57 if !key.shift && !key.ctrl && !key.repeat => {
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    player.seek(0.0);
                }
            }
            // Ctrl+Shift+Left / Right: frame step and pause.
            0x25 if key.ctrl && key.shift => {
                self.step_video_frame(ctx, fs_idx, -1);
            }
            0x27 if key.ctrl && key.shift => {
                self.step_video_frame(ctx, fs_idx, 1);
            }
            // Left / Right: same seek granularity as the egui fullscreen path.
            0x25 => {
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    let delta = if key.ctrl {
                        -30.0
                    } else if key.shift {
                        -1.0
                    } else {
                        -5.0
                    };
                    player.seek_relative(delta);
                }
            }
            0x27 => {
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    let delta = if key.ctrl {
                        30.0
                    } else if key.shift {
                        1.0
                    } else {
                        5.0
                    };
                    player.seek_relative(delta);
                }
            }
            // Plain Up / Down: navigate files, matching the egui fullscreen path.
            0x26 if !key.shift && !key.ctrl => {
                self.navigate_native_video_fullscreen(ctx, fs_idx, -1);
            }
            0x28 if !key.shift && !key.ctrl => {
                self.navigate_native_video_fullscreen(ctx, fs_idx, 1);
            }
            // Home / End: jump to the first / last visible navigable item.
            0x24 if !key.shift && !key.ctrl && !key.repeat => {
                if let Some(idx) = crate::ui_helpers::boundary_navigable_idx(
                    &self.items,
                    &self.visible_indices,
                    false,
                ) {
                    if idx != fs_idx {
                        self.open_native_video_fullscreen_from_navigation(ctx, idx);
                    }
                }
            }
            0x23 if !key.shift && !key.ctrl && !key.repeat => {
                if let Some(idx) = crate::ui_helpers::boundary_navigable_idx(
                    &self.items,
                    &self.visible_indices,
                    true,
                ) {
                    if idx != fs_idx {
                        self.open_native_video_fullscreen_from_navigation(ctx, idx);
                    }
                }
            }
            // Shift+Up / Shift+Down: volume. Plain Up/Down remains for the
            // future full overlay phase as well, but the native HWND can already
            // perform the same item navigation without involving egui input.
            0x26 if key.shift && !key.ctrl => {
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    let v = (player.volume() + 0.20).min(crate::settings::VIDEO_VOLUME_MAX);
                    player.set_volume(v);
                    self.settings.video_volume = v;
                    self.settings.save();
                }
            }
            0x28 if key.shift && !key.ctrl => {
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    let v = (player.volume() - 0.20).max(0.0);
                    player.set_volume(v);
                    self.settings.video_volume = v;
                    self.settings.save();
                }
            }
            // M: mute
            0x4D if !key.shift && !key.ctrl && !key.repeat => {
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    player.set_muted(!player.is_muted());
                }
            }
            // L: loop
            0x4C if !key.shift && !key.ctrl && !key.repeat => {
                self.settings.video_loop = !self.settings.video_loop;
                self.settings.save();
            }
            // J / K: previous / next chapter, bookmark, or pin marker.
            0x4A if !key.shift && !key.ctrl && !key.repeat => {
                self.jump_native_video_marker(fs_idx, false);
            }
            0x4B if !key.shift && !key.ctrl && !key.repeat => {
                self.jump_native_video_marker(fs_idx, true);
            }
            // Space: check/uncheck the current item, matching normal fullscreen.
            0x20 if !key.shift && !key.ctrl && !key.repeat => {
                let checked = if self.checked.contains(&fs_idx) {
                    self.checked.remove(&fs_idx);
                    false
                } else {
                    self.checked.insert(fs_idx);
                    true
                };
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    player.set_native_checked(checked);
                }
            }
            // P: perf overlay
            0x50 if !key.shift && !key.ctrl && !key.repeat => {
                self.video_perf_overlay_visible = !self.video_perf_overlay_visible;
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    player.set_native_perf_overlay_visible(self.video_perf_overlay_visible);
                }
            }
            // S: tile mode toggle. This still uses the egui context for screen
            // size until the native overlay owns layout.
            0x53 if !key.shift && !key.ctrl && !key.repeat => {
                let screen = ctx.content_rect().size();
                self.toggle_video_tile_mode(fs_idx, screen);
                self.sync_native_video_tile_overlay(ctx, fs_idx);
            }
            // B: add video bookmark.
            0x42 if !key.shift && !key.ctrl && !key.repeat => {
                self.add_native_video_bookmark(fs_idx, None);
            }
            _ => {
                hud_activity = false;
            }
        }
        if hud_activity {
            self.mark_native_video_hud_activity(ctx);
        }
    }

    #[cfg(windows)]
    pub(super) fn jump_native_video_marker(&mut self, fs_idx: usize, next: bool) {
        const NAV_MARKER_EPSILON: f64 = 0.5;
        let markers = self.collect_video_nav_markers(fs_idx);
        let current = self
            .fs_video_player(fs_idx)
            .map(|p| p.position())
            .unwrap_or(0.0);
        let target = if next {
            markers
                .iter()
                .find(|marker| marker.pts > current + NAV_MARKER_EPSILON)
                .cloned()
        } else {
            markers
                .iter()
                .rev()
                .find(|marker| marker.pts < current - NAV_MARKER_EPSILON)
                .cloned()
        };
        let Some(marker) = target else {
            return;
        };
        if let Some(player) = self.fs_video_player(fs_idx) {
            player.seek(marker.pts);
        }
        let direction = if next { "次の" } else { "前の" };
        let kind_label = match marker.kind {
            crate::ui_fullscreen::NavMarkerKind::Chapter => "チャプター",
            crate::ui_fullscreen::NavMarkerKind::Bookmark => "ブックマーク",
            crate::ui_fullscreen::NavMarkerKind::Pin => "ピン",
        };
        let toast = match (marker.kind, marker.title.as_deref()) {
            (crate::ui_fullscreen::NavMarkerKind::Chapter, Some(title))
            | (crate::ui_fullscreen::NavMarkerKind::Bookmark, Some(title))
                if !title.is_empty() =>
            {
                format!(
                    "{} {}{}: {}",
                    crate::ui_helpers::format_hms(marker.pts),
                    direction,
                    kind_label,
                    title
                )
            }
            _ => format!(
                "{} {}{}",
                crate::ui_helpers::format_hms(marker.pts),
                direction,
                kind_label
            ),
        };
        self.show_feedback_toast(toast);
    }

    #[cfg(windows)]
    pub(super) fn toggle_native_video_vst3_gui(&mut self) {
        let opening = !self.show_vst3_manager;
        self.show_vst3_manager = opening;
        if let Some(hwnd) = self.native_video_presenter_hwnd() {
            self.dsp_bridge.set_existing_guis_owner_to_hwnd(hwnd);
            self.native_video_owner_synced_hwnd = hwnd;
        }
        self.dsp_bridge.set_all_guis_topmost(opening);
        std::sync::Arc::clone(&self.dsp_bridge).set_all_guis_visible_async(opening);
        self.settings.vst3_gui_visible = opening;
        self.settings.save();
    }

    #[cfg(windows)]
    pub(super) fn set_native_video_vst3_panel_visible(&mut self, visible: bool) {
        self.show_vst3_manager = visible;
    }

    #[cfg(windows)]
    pub(super) fn set_native_video_vst3_compact(&mut self, compact: bool) {
        if self.settings.vst3_video_compact == compact {
            return;
        }
        self.settings.vst3_video_compact = compact;
        self.settings.save();
    }

    #[cfg(windows)]
    pub(super) fn show_native_video_vst3_slot_gui(&mut self, idx: usize, path: String) {
        std::sync::Arc::clone(&self.dsp_bridge).show_slot_gui_async(idx);
        let mut changed = !self.settings.vst3_gui_visible;
        self.settings.vst3_gui_visible = true;
        if let Some(entry) = self.find_vst3_entry_mut(&path)
            && entry.user_hidden
        {
            entry.user_hidden = false;
            changed = true;
        }
        if changed {
            self.settings.save();
        }
    }

    #[cfg(windows)]
    pub(super) fn hide_native_video_vst3_slot_gui(&mut self, idx: usize, path: String) {
        self.dsp_bridge.user_hide_slot_gui(idx);
        if let Some(entry) = self.find_vst3_entry_mut(&path)
            && !entry.user_hidden
        {
            entry.user_hidden = true;
            self.settings.save();
        }
    }

    #[cfg(windows)]
    pub(super) fn set_native_video_vst3_slot_bypass(
        &mut self,
        idx: usize,
        path: String,
        bypass: bool,
    ) {
        self.dsp_bridge.set_bypass(idx, bypass);
        if let Some(entry) = self.find_vst3_entry_mut(&path)
            && entry.bypass != bypass
        {
            entry.bypass = bypass;
            self.settings.save();
        }
    }

    #[cfg(windows)]
    pub(super) fn show_native_video_overlay_toast(&self, text: String, centered: bool) {
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.show_native_overlay_toast(text, centered);
        }
    }

    #[cfg(windows)]
    pub(super) fn native_boundary_hint_text(hint: crate::ui_fullscreen::FsBoundaryHint) -> String {
        match hint {
            crate::ui_fullscreen::FsBoundaryHint::Edge { at_end, .. } => {
                if at_end {
                    "最後の画像です".to_string()
                } else {
                    "最初の画像です".to_string()
                }
            }
            crate::ui_fullscreen::FsBoundaryHint::NoImageFolder { forward, .. } => {
                if forward {
                    "次の画像フォルダが見つかりません".to_string()
                } else {
                    "前の画像フォルダが見つかりません".to_string()
                }
            }
            crate::ui_fullscreen::FsBoundaryHint::SearchEnd { forward, .. } => {
                if forward {
                    "これ以上先の検索結果はありません".to_string()
                } else {
                    "これ以上前の検索結果はありません".to_string()
                }
            }
        }
    }

    #[cfg(windows)]
    pub(super) fn navigate_native_video_fullscreen(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        base_delta: i32,
    ) {
        if self.video_tile_swap_pending.is_some() {
            return;
        }
        if self.fs_nav_is_locked() {
            return;
        }
        let nav_delta = self.spread_nav_delta(base_delta, false);
        if let Some(new_idx) = crate::ui_helpers::adjacent_navigable_idx(
            &self.items,
            &self.visible_indices,
            fs_idx,
            nav_delta,
        ) {
            self.open_native_video_fullscreen_from_navigation(ctx, new_idx);
        } else {
            let hint = crate::ui_fullscreen::FsBoundaryHint::Edge {
                at_end: nav_delta > 0,
                at: std::time::Instant::now(),
            };
            self.show_native_video_overlay_toast(Self::native_boundary_hint_text(hint), true);
            self.fs_boundary_hint = Some(hint);
            self.mark_native_video_hud_activity(ctx);
        }
    }

    #[cfg(windows)]
    pub(super) fn adjust_native_video_tile_columns(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        delta: i32,
    ) {
        if self.video_tile_swap_pending.is_some() {
            return;
        }
        if self.fullscreen_idx != Some(fs_idx) || self.ime_input_active() || delta == 0 {
            return;
        }
        let candidates = crate::settings::VIDEO_TILE_COLUMN_CANDIDATES;
        let current = self.settings.video_tile_columns;
        let current_idx = candidates
            .iter()
            .position(|&cols| cols == current)
            .unwrap_or_else(|| {
                candidates
                    .iter()
                    .position(|&cols| cols >= current)
                    .unwrap_or(candidates.len().saturating_sub(1))
            });
        let next_idx = (current_idx as i32 + delta)
            .clamp(0, candidates.len().saturating_sub(1) as i32) as usize;
        let next_cols = candidates[next_idx];
        if next_cols == current {
            return;
        }
        self.settings.video_tile_columns = next_cols;
        self.settings.save();
        let was_open = self.video_tile_state.is_some();
        self.video_tile_state = None;
        self.video_tile_swap_pending = None;
        if was_open {
            let screen = ctx.content_rect().size();
            self.toggle_video_tile_mode(fs_idx, screen);
            self.sync_native_video_tile_overlay(ctx, fs_idx);
        }
        self.mark_native_video_hud_activity(ctx);
    }

    #[cfg(windows)]
    pub(super) fn toggle_native_video_perf_overlay(&mut self, fs_idx: usize) {
        self.video_perf_overlay_visible = !self.video_perf_overlay_visible;
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_native_perf_overlay_visible(self.video_perf_overlay_visible);
        }
    }

    #[cfg(windows)]
    pub(super) fn next_native_video_source_epoch(&mut self) -> u64 {
        let epoch = self.native_video_source_epoch_next.max(1);
        self.native_video_source_epoch_next = epoch.wrapping_add(1).max(1);
        epoch
    }

    #[cfg(windows)]
    pub(super) fn try_start_video_tile_fast_swap(
        &mut self,
        ctx: &egui::Context,
        target_idx: usize,
    ) -> bool {
        if self.video_tile_swap_pending.is_some() {
            return true;
        }
        let Some(from_idx) = self.fullscreen_idx else {
            return false;
        };
        if from_idx == target_idx {
            return true;
        }
        if self.video_tile_state.is_none() {
            return false;
        }
        let Some(GridItem::Video(target_path)) = self.items.get(target_idx).cloned() else {
            return false;
        };
        if !matches!(self.items.get(from_idx), Some(GridItem::Video(_))) {
            return false;
        }

        self.save_all_video_resume_positions();
        let native_output = match self.fs_cache.get_mut(&from_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                player.pause_audio_output();
                player.set_playing(false);
                player.clear_audio_output_buffer();
                player.take_native_output()
            }
            _ => None,
        };
        let Some(native_output) = native_output else {
            return false;
        };

        let source_epoch = self.next_native_video_source_epoch();
        let mut new_player = self.build_video_player_for_open(
            target_idx,
            target_path.clone(),
            false,
            Some(false),
            None,
        );
        new_player.attach_native_output(native_output);
        let payload = new_player.build_switch_source_payload(source_epoch, true);
        new_player.switch_native_source(payload);

        self.fs_cache.insert(
            target_idx,
            FsCacheEntry::Video {
                player: Box::new(new_player),
                load_seq: self.input_seq,
            },
        );
        self.video_tile_swap_pending = Some(VideoTileSwapPending {
            target_idx,
            target_path,
            source_epoch,
            started_at: std::time::Instant::now(),
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
        });
        self.video_tile_reopen_pending = false;
        self.video_tile_reopen_deadline = None;

        self.open_fullscreen(target_idx);
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&target_idx) {
            player.set_playing(false);
            crate::logger::log(format!(
                "[video-debug] post-swap state: idx={target_idx} engine_state={} seek_serial={} clock_is_playing={} pos={:.3} video_rx_len={} audio_rx_len={} pending_frames={}",
                player.engine_state_name(),
                player.current_seek_serial(),
                player.is_playing(),
                player.position(),
                player.video_rx_len(),
                player.audio_rx_len(),
                player.pending_frames()
            ));
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "native_presenter",
                    "post_swap_state",
                    None,
                    0,
                    &[
                        ("idx", serde_json::Value::from(target_idx as i64)),
                        (
                            "engine_state",
                            serde_json::Value::from(player.engine_state_name()),
                        ),
                        (
                            "seek_serial",
                            serde_json::Value::from(player.current_seek_serial() as i64),
                        ),
                        ("playing", serde_json::Value::from(player.is_playing())),
                        ("position", serde_json::Value::from(player.position())),
                        (
                            "video_rx_len",
                            serde_json::Value::from(player.video_rx_len() as i64),
                        ),
                        (
                            "audio_rx_len",
                            serde_json::Value::from(player.audio_rx_len() as i64),
                        ),
                        (
                            "pending_frames",
                            serde_json::Value::from(player.pending_frames() as i64),
                        ),
                    ],
                );
            }
        }
        self.set_native_video_tile_preparing_overlay(target_idx);
        self.sync_native_video_metadata(target_idx);
        self.sync_native_video_timeline_markers(target_idx);
        self.sync_native_video_vst3_available(target_idx);
        self.sync_native_video_vst3_panel(target_idx);

        if from_idx != target_idx {
            self.fs_cache.remove(&from_idx);
        }
        crate::logger::log(format!(
            "[native-video] fast tile swap: from={from_idx} to={target_idx} epoch={source_epoch}"
        ));
        ctx.request_repaint();
        true
    }

    #[cfg(windows)]
    pub(super) fn open_native_video_fullscreen_from_navigation(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
    ) {
        if self.try_start_video_tile_fast_swap(ctx, idx) {
            return;
        }
        let started = std::time::Instant::now();
        let from_idx = self.fullscreen_idx;
        let restore_video_tile = self.video_tile_state.is_some();
        let restore_target_is_video = matches!(self.items.get(idx), Some(GridItem::Video(_)));
        crate::logger::log(format!(
            "[native-video] wheel navigation open: from={from_idx:?} to={idx} tile_restore={restore_video_tile} target_video={restore_target_is_video}"
        ));
        if restore_video_tile {
            self.video_tile_state = None;
            self.video_tile_swap_pending = None;
            if let Some(current_idx) = self.fullscreen_idx {
                self.set_native_video_tile_preparing_overlay(current_idx);
            }
            if restore_target_is_video {
                self.video_tile_reopen_pending = true;
                self.video_tile_reopen_deadline =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(3));
            } else {
                self.video_tile_reopen_pending = false;
                self.video_tile_reopen_deadline = None;
            }
        }

        self.open_fullscreen(idx);

        if restore_video_tile && restore_target_is_video {
            self.set_native_video_tile_preparing_overlay(idx);
        } else if restore_video_tile {
            self.video_tile_reopen_pending = false;
            self.video_tile_reopen_deadline = None;
        }
        crate::logger::log(format!(
            "[native-video] wheel navigation open queued: to={idx} elapsed_ms={:.1}",
            started.elapsed().as_secs_f64() * 1000.0
        ));
        ctx.request_repaint();
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_mouse_button(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        event: crate::video::native_window::NativeVideoMouseButtonEvent,
    ) {
        use crate::video::native_window::NativeVideoMouseButton;

        self.mark_native_video_hud_activity(ctx);
        if event.button == NativeVideoMouseButton::Right && !event.down && !event.double_click {
            self.close_fullscreen();
            return;
        }
        if event.button != NativeVideoMouseButton::Left {
            return;
        }

        if event.double_click {
            self.native_video_pointer_down = None;
            return;
        }

        if event.down {
            self.native_video_pointer_down = Some(NativeVideoPointerDown {
                fs_idx,
                x: event.x,
                y: event.y,
                at: std::time::Instant::now(),
            });
            return;
        }

        let Some(start) = self.native_video_pointer_down.take() else {
            return;
        };
        if start.fs_idx != fs_idx {
            return;
        }
        let dx = event.x - start.x;
        let dy = event.y - start.y;
        let moved_sq = dx.saturating_mul(dx) + dy.saturating_mul(dy);
        let click_like =
            moved_sq <= 36 && start.at.elapsed() <= std::time::Duration::from_millis(500);
        if !click_like || self.settings.vst3_gui_visible {
            return;
        }
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.toggle_play();
        }
    }

    #[cfg(windows)]
    pub(super) fn mark_native_video_hud_activity(&mut self, ctx: &egui::Context) {
        let now = std::time::Instant::now();
        // ネイティブビデオウィンドウのマウス/キー入力は eframe フルスクリーンビューポートの
        // input には現れないため、カーソル auto-hide のアクティビティタイマもここで更新する。
        // 動画 HUD 活動 = ユーザーがマウスを動かしたかキー操作した = カーソル可視。
        self.cursor_last_activity = Some(now);
        self.cursor_hidden = false;
        // eframe 経由のキー入力 (Space で pause/resume 等) は native presenter HWND の
        // `push_native_event` を経由しないので、NativeEguiOverlay 側の cursor タイマが
        // ズレる (= 一時停止前にカーソルが隠れていたまま再開しても消えたままになる)。
        // current の player に明示的にアクティビティを伝搬してリセットさせる。
        if let Some(idx) = self.fullscreen_idx
            && let Some(crate::fs_animation::FsCacheEntry::Video { player, .. }) =
                self.fs_cache.get(&idx)
        {
            player.mark_cursor_activity();
        }
        ctx.request_repaint();
    }
}
