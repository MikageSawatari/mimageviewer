use mimageviewer_ipc::{RemoteWebFeatureStatus, SessionConnectionKind};
use qrcode::{Color, QrCode};

use super::session::{ActiveSessionSnapshot, SessionHandle};

#[derive(Default)]
pub(crate) struct RemoteSessionUiState {
    handle: Option<SessionHandle>,
    last_acquisition_sequence: u64,
    last_control_return_sequence: u64,
    pending_fullscreen_restore: Option<PendingFullscreenRestore>,
    paused_animation_restore_key: Option<String>,
    show_connection_dialog: bool,
}

struct PendingFullscreenRestore {
    item_key: String,
    view: ReloadedView,
    wait_frames: u8,
}

#[derive(Clone, Copy)]
enum ReloadedView {
    ReadingHistory,
    Rating,
    Bookmarks,
    SmartFolder(uuid::Uuid),
    Other,
}

impl crate::app::App {
    pub(crate) fn set_remote_session_handle(&mut self, handle: SessionHandle) {
        let snapshot = handle.snapshot();
        self.remote_session_ui.last_acquisition_sequence = if snapshot.active.is_some() {
            snapshot.acquisition_sequence.wrapping_sub(1)
        } else {
            snapshot.acquisition_sequence
        };
        self.remote_session_ui.last_control_return_sequence = snapshot.control_return_sequence;
        self.remote_session_ui.handle = Some(handle);
    }

    pub(crate) fn open_remote_connection_dialog(&mut self) {
        self.remote_session_ui.show_connection_dialog = true;
    }

    pub(crate) fn remote_connection_dialog_open(&self) -> bool {
        self.remote_session_ui.show_connection_dialog
    }

    pub(crate) fn consume_remote_animation_pause_restore(&mut self, index: usize) -> bool {
        let Some(expected) = self
            .remote_session_ui
            .paused_animation_restore_key
            .as_deref()
        else {
            return false;
        };
        let matches = self
            .items
            .get(index)
            .is_some_and(|item| item.perf_key() == expected);
        if matches {
            self.remote_session_ui.paused_animation_restore_key = None;
        }
        matches
    }

    pub(crate) fn remote_session_active(&self) -> bool {
        self.remote_session_ui
            .handle
            .as_ref()
            .is_some_and(|handle| handle.snapshot().active.is_some())
    }

    pub(crate) fn poll_remote_session(&mut self, ctx: &egui::Context) {
        if let Some(handle) = self.remote_session_ui.handle.as_ref() {
            handle.install_repaint_context(ctx);
        }
        let snapshot = self
            .remote_session_ui
            .handle
            .as_ref()
            .map(SessionHandle::snapshot);
        let active = snapshot
            .as_ref()
            .is_some_and(|value| value.active.is_some());
        if let Some(snapshot) = snapshot.as_ref()
            && snapshot.acquisition_sequence != self.remote_session_ui.last_acquisition_sequence
        {
            self.remote_session_ui.last_acquisition_sequence = snapshot.acquisition_sequence;
            let (media, slideshow, animations, continuous) =
                self.pause_local_progress_for_remote_session();
            crate::logger::log(format!(
                "remote_ipc: local playback paused on session acquire media={media} slideshow={slideshow} animations={animations} continuous_pending={continuous}"
            ));
        }
        if let Some(snapshot) = snapshot
            && snapshot.control_return_sequence
                != self.remote_session_ui.last_control_return_sequence
        {
            self.remote_session_ui.last_control_return_sequence = snapshot.control_return_sequence;
            self.reload_after_remote_session_release();
        }
        self.poll_remote_fullscreen_restore();
        if active || self.remote_session_ui.pending_fullscreen_restore.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    pub(crate) fn show_remote_session_dialog(&mut self, ctx: &egui::Context) {
        let snapshot = self
            .remote_session_ui
            .handle
            .as_ref()
            .and_then(|handle| handle.snapshot().active);
        let Some(snapshot) = snapshot else {
            return;
        };
        let mut disconnect = false;
        egui::Modal::new(egui::Id::new("remote_session_modal")).show(ctx, |ui| {
            ui.heading("リモート接続中");
            ui.add_space(6.0);
            show_connection_summary(ui, &snapshot);
            ui.separator();
            ui.label(format!(
                "現在の処理: {}",
                snapshot.current_operation.as_deref().unwrap_or("待機中")
            ));
            ui.label(format!(
                "要求 {} 件 / 完了 {} 件 / 失敗 {} 件",
                snapshot.request_count, snapshot.completed_count, snapshot.failed_count
            ));
            ui.label(format!(
                "処理中 {} 件 / 待機 {} 件",
                snapshot.running_count, snapshot.queued_count
            ));
            ui.add_space(8.0);
            ui.label("切断するとローカル操作へ戻ります。リモートは次の操作時に再接続できます。");
            ui.add_space(8.0);
            if ui
                .add_sized([160.0, 34.0], egui::Button::new("切断する"))
                .clicked()
            {
                disconnect = true;
            }
        });
        if disconnect {
            if let Some(handle) = self.remote_session_ui.handle.as_ref() {
                handle.local_disconnect();
            }
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    pub(crate) fn show_remote_connection_dialog(&mut self, ctx: &egui::Context) {
        if !self.remote_session_ui.show_connection_dialog {
            return;
        }
        let info = self
            .remote_session_ui
            .handle
            .as_ref()
            .and_then(|handle| handle.snapshot().remote_web);
        let ipc_enabled = self.remote_session_ui.handle.is_some();
        let mut open = true;
        egui::Window::new("リモート接続")
            .id(egui::Id::new("remote_connection_dialog"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                if !ipc_enabled {
                    ui.heading("リモート接続は無効です");
                    ui.label("mIV 本体を --remote-ipc 付きで起動してください。");
                    return;
                }
                let Some(info) = info.as_ref() else {
                    ui.heading("remote-web を待っています");
                    ui.label("接続状態: remote-web 未接続");
                    ui.label("remote-web を起動すると、ここに接続 URL と QR コードを表示します。");
                    return;
                };

                ui.label("接続状態: remote-web 接続済み");
                ui.label(format!(
                    "tailscale serve: {}",
                    remote_feature_status_label(info.tailscale_serve)
                ));
                ui.label(format!(
                    "PIN: {}",
                    if info.pin_configured {
                        "設定済み"
                    } else {
                        "未設定"
                    }
                ));
                ui.add_space(6.0);
                paint_qr(ui, &info.public_url);
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.monospace(&info.public_url);
                    if ui.button("コピー").clicked() {
                        ui.ctx().copy_text(info.public_url.clone());
                    }
                });
                ui.small("QR コードには URL だけを含み、PIN や Bearer トークンは含みません。");
            });
        self.remote_session_ui.show_connection_dialog = open;
    }

    fn reload_after_remote_session_release(&mut self) {
        let paused_animation_key = self
            .fullscreen_idx
            .filter(|index| self.fs_entry_is_animated(*index))
            .and_then(|index| self.items.get(index))
            .map(crate::grid_item::GridItem::perf_key);
        let fullscreen_key = self
            .fullscreen_idx
            .and_then(|index| self.items.get(index))
            .map(crate::grid_item::GridItem::perf_key);
        let view = if self.items_are_reading_history_view {
            ReloadedView::ReadingHistory
        } else if self.items_are_rating_view {
            ReloadedView::Rating
        } else if self.items_are_bookmark_view {
            ReloadedView::Bookmarks
        } else if self.items_are_smart_folder_view {
            self.current_smart_folder_id
                .map(ReloadedView::SmartFolder)
                .unwrap_or(ReloadedView::Other)
        } else {
            ReloadedView::Other
        };

        // 既存の各「再読み込み」入口は start_loading_items で viewer を閉じる。
        // 先に identity を保持し、同じ入口が完了した後だけ open_fullscreen へ戻す。
        if fullscreen_key.is_some() {
            self.close_fullscreen();
        }
        match view {
            ReloadedView::ReadingHistory => self.enter_reading_history(),
            ReloadedView::Rating => self.reload_current_rating_view_preserving_sort(),
            ReloadedView::Bookmarks => self.refresh_bookmark_browser(),
            ReloadedView::SmartFolder(id) => self.open_smart_folder(id, true),
            ReloadedView::Other => self.reload_current_folder_preserving_override(),
        }
        self.remote_session_ui.pending_fullscreen_restore =
            fullscreen_key.map(|item_key| PendingFullscreenRestore {
                item_key,
                view,
                wait_frames: 0,
            });
        self.remote_session_ui.paused_animation_restore_key = paused_animation_key;
        crate::logger::log("remote_ipc: local control restored; current view reload requested");
    }

    fn poll_remote_fullscreen_restore(&mut self) {
        let Some(mut pending) = self.remote_session_ui.pending_fullscreen_restore.take() else {
            return;
        };
        pending.wait_frames = pending.wait_frames.saturating_add(1);
        let ready = match pending.view {
            ReloadedView::ReadingHistory => true,
            ReloadedView::Rating => self.rating_view_pending.is_none(),
            ReloadedView::Bookmarks => self.bookmark_browser_pending.is_none(),
            ReloadedView::SmartFolder(id) => {
                self.current_smart_folder_id == Some(id)
                    && self.smart_folder_pending.is_none()
                    && self.smart_folder_prepare_pending.is_none()
                    && self.smart_folder_confirm_pending.is_none()
            }
            // 通常フォルダの同期/非同期差を吸収し、旧 items を同フレームに拾わない。
            ReloadedView::Other => pending.wait_frames >= 2,
        };
        if ready {
            if let Some(index) = self
                .items
                .iter()
                .position(|item| item.perf_key() == pending.item_key)
            {
                self.selected = Some(index);
                if matches!(
                    self.items.get(index),
                    Some(crate::grid_item::GridItem::Video(_))
                ) {
                    self.fs_video_open_autoplay_override = Some(false);
                }
                self.open_fullscreen(index);
                crate::logger::log(
                    "remote_ipc: fullscreen position restored after session release",
                );
                return;
            }
            // 非同期一覧の install が直後のフレームに来る場合だけ有界に待つ。
            if pending.wait_frames < 120 {
                self.remote_session_ui.pending_fullscreen_restore = Some(pending);
            }
        } else {
            self.remote_session_ui.pending_fullscreen_restore = Some(pending);
        }
    }
}

fn remote_feature_status_label(status: RemoteWebFeatureStatus) -> &'static str {
    match status {
        RemoteWebFeatureStatus::Configured => "設定済み",
        RemoteWebFeatureStatus::NotConfigured => "未設定",
        RemoteWebFeatureStatus::Unknown => "確認できません",
    }
}

fn paint_qr(ui: &mut egui::Ui, url: &str) {
    let Ok(code) = QrCode::new(url.as_bytes()) else {
        ui.colored_label(egui::Color32::RED, "QR コードを生成できませんでした");
        return;
    };
    const QUIET_ZONE: usize = 4;
    const DISPLAY_PX: f32 = 240.0;
    let width = code.width();
    let modules = width + QUIET_ZONE * 2;
    let module_px = (DISPLAY_PX / modules as f32).floor().max(1.0);
    let side = module_px * modules as f32;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::WHITE);
    for (index, color) in code.to_colors().into_iter().enumerate() {
        if color != Color::Dark {
            continue;
        }
        let x = index % width + QUIET_ZONE;
        let y = index / width + QUIET_ZONE;
        let min = rect.min + egui::vec2(x as f32 * module_px, y as f32 * module_px);
        painter.rect_filled(
            egui::Rect::from_min_size(min, egui::vec2(module_px, module_px)),
            0.0,
            egui::Color32::BLACK,
        );
    }
}

fn show_connection_summary(ui: &mut egui::Ui, snapshot: &ActiveSessionSnapshot) {
    let connection = match snapshot.peer.connection_kind {
        SessionConnectionKind::Direct => "direct",
        SessionConnectionKind::Relay => "relay",
        SessionConnectionKind::Unknown => "取得できません",
    };
    ui.label(format!("接続種別: {connection}"));
    ui.label(format!(
        "対向端末: {}",
        snapshot
            .peer
            .device_name
            .as_deref()
            .unwrap_or("取得できません")
    ));
    ui.label(format!(
        "接続時刻: {} / 経過 {}",
        format_local_unix_ms(snapshot.connected_unix_ms),
        format_elapsed(snapshot.elapsed)
    ));
}

fn format_elapsed(elapsed: std::time::Duration) -> String {
    let total = elapsed.as_secs();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(windows)]
fn format_local_unix_ms(unix_ms: u64) -> String {
    const WINDOWS_TICKS_PER_MILLISECOND: u64 = 10_000;
    const UNIX_TO_WINDOWS_MILLISECONDS: u64 = 11_644_473_600_000;
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::Storage::FileSystem::FileTimeToLocalFileTime;
    use windows::Win32::System::Time::FileTimeToSystemTime;

    let Some(ticks) = unix_ms
        .checked_add(UNIX_TO_WINDOWS_MILLISECONDS)
        .and_then(|value| value.checked_mul(WINDOWS_TICKS_PER_MILLISECOND))
    else {
        return "取得できません".to_owned();
    };
    let filetime = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut local = FILETIME::default();
    let mut system = SYSTEMTIME::default();
    if unsafe { FileTimeToLocalFileTime(&filetime, &mut local) }.is_err()
        || unsafe { FileTimeToSystemTime(&local, &mut system) }.is_err()
    {
        return "取得できません".to_owned();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        system.wYear, system.wMonth, system.wDay, system.wHour, system.wMinute, system.wSecond
    )
}

#[cfg(not(windows))]
fn format_local_unix_ms(unix_ms: u64) -> String {
    format!("unix-ms {unix_ms}")
}
