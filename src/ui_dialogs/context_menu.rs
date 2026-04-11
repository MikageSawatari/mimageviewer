//! サムネイルグリッドの右クリックコンテキストメニュー。

use std::path::PathBuf;
use eframe::egui;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::grid_item::GridItem;

impl crate::app::App {
    /// コンテキストメニューを表示する。
    pub(crate) fn show_context_menu(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        let idx = match self.context_menu_idx {
            Some(i) => i,
            None => return None,
        };

        let item = match self.items.get(idx) {
            Some(item) => item.clone(),
            None => {
                self.context_menu_idx = None;
                return None;
            }
        };

        let has_checked = !self.checked.is_empty();
        let checked_count = self.checked.len();
        let nav: Option<PathBuf> = None;
        let mut close = false;

        // 記録済みの座標に固定表示
        let pos = self.context_menu_pos;

        let mut open = true;
        egui::Window::new("context_menu")
            .id(egui::Id::new("grid_ctx_menu"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .fixed_pos(pos)
            .order(egui::Order::Tooltip)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(200.0);

                if has_checked {
                    // ── 選択モード: チェック済みファイルに対する操作 ──
                    ui.label(
                        egui::RichText::new(format!("{checked_count} 件選択中"))
                            .strong()
                            .size(13.0),
                    );
                    ui.separator();

                    // パスをコピー (disabled)
                    ui.add_enabled(false, egui::Button::new("パスをコピー"));

                    // コピー
                    if ui.button("コピー").clicked() {
                        let paths = self.collect_checked_paths();
                        copy_files_to_clipboard(&paths);
                        close = true;
                    }

                    // カット
                    if ui.button("カット").clicked() {
                        let paths = self.collect_checked_paths();
                        cut_files_to_clipboard(&paths);
                        close = true;
                    }

                    // 回転
                    ui.horizontal(|ui| {
                        if ui.button("左に回転 (L)").clicked() {
                            for &i in &self.checked.clone() {
                                self.rotate_image_ccw(i);
                            }
                            close = true;
                        }
                        if ui.button("右に回転 (R)").clicked() {
                            for &i in &self.checked.clone() {
                                self.rotate_image_cw(i);
                            }
                            close = true;
                        }
                    });

                    // フォルダを開く (disabled)
                    ui.add_enabled(false, egui::Button::new("フォルダを開く"));

                    ui.separator();

                    // 削除 (ゴミ箱)
                    if ui.button(format!("削除 (ゴミ箱) [{checked_count}件]")).clicked() {
                        let targets: Vec<(usize, PathBuf)> = self.collect_checked_indexed_paths();
                        self.delete_targets = targets;
                        self.show_delete_confirm = true;
                        close = true;
                    }

                    ui.separator();
                    if ui.button("選択解除").clicked() {
                        self.checked.clear();
                        close = true;
                    }
                } else {
                    // ── 通常モード: 単一アイテムに対する操作 ──
                    match &item {
                        GridItem::Image(p) | GridItem::Video(p) => {
                            if ui.button("パスをコピー").clicked() {
                                ctx.copy_text(p.to_string_lossy().to_string());
                                close = true;
                            }
                            if matches!(item, GridItem::Image(_)) {
                                if ui.button("画像をクリップボードにコピー").clicked() {
                                    copy_image_to_clipboard(p);
                                    close = true;
                                }
                            }
                            if ui.button("コピー").clicked() {
                                copy_files_to_clipboard(&[p.clone()]);
                                close = true;
                            }
                            if ui.button("カット").clicked() {
                                cut_files_to_clipboard(&[p.clone()]);
                                close = true;
                            }
                            if ui.button("フォルダを開く").clicked() {
                                open_folder_in_explorer(p);
                                close = true;
                            }
                            ui.separator();
                            ui.horizontal(|ui| {
                                if ui.button("左に回転 (L)").clicked() {
                                    self.rotate_image_ccw(idx);
                                    close = true;
                                }
                                if ui.button("右に回転 (R)").clicked() {
                                    self.rotate_image_cw(idx);
                                    close = true;
                                }
                            });
                            ui.separator();
                            if ui.button("削除 (ゴミ箱)").clicked() {
                                self.delete_targets = vec![(idx, p.clone())];
                                self.show_delete_confirm = true;
                                close = true;
                            }
                        }
                        GridItem::Folder(p) => {
                            if ui.button("パスをコピー").clicked() {
                                ctx.copy_text(p.to_string_lossy().to_string());
                                close = true;
                            }
                            if ui.button("エクスプローラで開く").clicked() {
                                let _ = std::process::Command::new("explorer")
                                    .arg(p.as_os_str())
                                    .spawn();
                                close = true;
                            }
                        }
                        GridItem::ZipImage { zip_path, entry_name } => {
                            let display = format!("{}:{}", zip_path.display(), entry_name);
                            if ui.button("パスをコピー").clicked() {
                                ctx.copy_text(display);
                                close = true;
                            }
                        }
                        GridItem::ZipSeparator { .. } => {
                            close = true;
                        }
                    }
                }

                // メニュー外クリックで閉じる
                if ui.input(|i| i.pointer.any_click()) && !ui.ui_contains_pointer() {
                    close = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });

        if close || !open {
            self.context_menu_idx = None;
        }

        nav
    }

    /// チェック済みアイテムのパスを収集する。
    fn collect_checked_paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for &idx in &self.checked {
            match self.items.get(idx) {
                Some(GridItem::Image(p)) | Some(GridItem::Video(p)) => {
                    paths.push(p.clone());
                }
                _ => {}
            }
        }
        paths
    }

    /// チェック済みアイテムの (idx, path) を収集する (降順ソート)。
    fn collect_checked_indexed_paths(&self) -> Vec<(usize, PathBuf)> {
        let mut targets: Vec<(usize, PathBuf)> = Vec::new();
        for &idx in &self.checked {
            match self.items.get(idx) {
                Some(GridItem::Image(p)) | Some(GridItem::Video(p)) => {
                    targets.push((idx, p.clone()));
                }
                _ => {}
            }
        }
        // 降順ソート (削除時にインデックスがずれないよう後ろから削除)
        targets.sort_by(|a, b| b.0.cmp(&a.0));
        targets
    }

    /// 削除確認ダイアログを表示する。
    pub(crate) fn show_delete_confirm_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_delete_confirm {
            return;
        }

        if self.delete_targets.is_empty() {
            self.show_delete_confirm = false;
            return;
        }

        let count = self.delete_targets.len();
        let label = if count == 1 {
            let name = self.delete_targets[0]
                .1
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string();
            format!("「{name}」をゴミ箱に移動しますか？")
        } else {
            format!("{count} 件のファイルをゴミ箱に移動しますか？")
        };

        let mut open = true;
        egui::Window::new("削除の確認")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(&label);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("削除").clicked() {
                        // 降順で削除（インデックスずれ防止）
                        let mut targets = std::mem::take(&mut self.delete_targets);
                        targets.sort_by(|a, b| b.0.cmp(&a.0));
                        for (idx, path) in &targets {
                            if move_to_recycle_bin(path) {
                                self.remove_item_after_delete(*idx);
                            }
                        }
                        self.checked.clear();
                        self.show_delete_confirm = false;
                        self.delete_targets.clear();
                    }
                    if ui.button("キャンセル").clicked() {
                        self.show_delete_confirm = false;
                        self.delete_targets.clear();
                    }
                });
            });

        if !open {
            self.show_delete_confirm = false;
            self.delete_targets.clear();
        }
    }

    /// DEL キーで選択画像を削除するハンドラ。
    pub(crate) fn handle_delete_key(&mut self, ctx: &egui::Context) {
        if self.fullscreen_idx.is_some() || self.address_has_focus {
            return;
        }
        let del = ctx.input(|i| i.key_pressed(egui::Key::Delete));
        if !del {
            return;
        }

        if !self.checked.is_empty() {
            // チェック済みがある → まとめて削除
            let targets = self.collect_checked_indexed_paths();
            self.delete_targets = targets;
            self.show_delete_confirm = true;
        } else if let Some(idx) = self.selected {
            // 単一選択
            let path = match self.items.get(idx) {
                Some(GridItem::Image(p)) | Some(GridItem::Video(p)) => p.clone(),
                _ => return,
            };
            self.delete_targets = vec![(idx, path)];
            self.show_delete_confirm = true;
        }
    }

    /// 削除後にアイテムリストから除去し、選択を調整する。
    fn remove_item_after_delete(&mut self, idx: usize) {
        if idx < self.items.len() {
            self.items.remove(idx);
            self.thumbnails.remove(idx);
            if idx < self.image_metas.len() {
                self.image_metas.remove(idx);
            }
            let n = self.items.len();
            if n == 0 {
                self.selected = None;
            } else if let Some(sel) = self.selected {
                if sel >= n {
                    self.selected = Some(n - 1);
                }
            }
            self.requested.clear();
        }
    }
}

// ---------------------------------------------------------------------------
// OS 操作ヘルパー
// ---------------------------------------------------------------------------

/// ファイルを Windows ゴミ箱に移動する。
fn move_to_recycle_bin(path: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::UI::Shell::{
            SHFileOperationW, SHFILEOPSTRUCTW, FO_DELETE,
            FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_SILENT,
        };

        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .chain(std::iter::once(0))
            .collect();

        let flags = (FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0 | FOF_SILENT.0) as u16;
        let mut op = SHFILEOPSTRUCTW {
            wFunc: FO_DELETE,
            pFrom: windows::core::PCWSTR(wide.as_ptr()),
            fFlags: flags,
            ..Default::default()
        };

        let result = unsafe { SHFileOperationW(&mut op) };
        result == 0
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

/// ファイルをクリップボードにコピー (エクスプローラのコピーと同等)。
fn copy_files_to_clipboard(paths: &[PathBuf]) {
    #[cfg(windows)]
    {
        if paths.is_empty() {
            return;
        }
        let paths_str: Vec<String> = paths
            .iter()
            .map(|p| format!("'{}'", p.to_string_lossy().replace('\'', "''")))
            .collect();
        let arr = paths_str.join(",");
        let script = format!(
            "$files = @({arr}); \
             $col = New-Object System.Collections.Specialized.StringCollection; \
             foreach($f in $files){{ $col.Add($f) | Out-Null }}; \
             Add-Type -AssemblyName System.Windows.Forms; \
             [System.Windows.Forms.Clipboard]::SetFileDropList($col)"
        );
        let mut cmd = std::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-Command", &script]);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);
        let _ = cmd.spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = paths;
    }
}

/// ファイルをクリップボードにカット (移動操作用)。
fn cut_files_to_clipboard(paths: &[PathBuf]) {
    #[cfg(windows)]
    {
        if paths.is_empty() {
            return;
        }
        // Windows ではカットはコピー+PreferredDropEffect=MOVE
        let paths_str: Vec<String> = paths
            .iter()
            .map(|p| format!("'{}'", p.to_string_lossy().replace('\'', "''")))
            .collect();
        let arr = paths_str.join(",");
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; \
             $files = @({arr}); \
             $col = New-Object System.Collections.Specialized.StringCollection; \
             foreach($f in $files){{ $col.Add($f) | Out-Null }}; \
             $data = New-Object System.Windows.Forms.DataObject; \
             $data.SetFileDropList($col); \
             $ms = New-Object System.IO.MemoryStream(4); \
             $ms.Write([BitConverter]::GetBytes(2), 0, 4); \
             $data.SetData('Preferred DropEffect', $ms); \
             [System.Windows.Forms.Clipboard]::SetDataObject($data, $true)"
        );
        let mut cmd = std::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-Command", &script]);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);
        let _ = cmd.spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = paths;
    }
}

/// 画像ファイルの内容をクリップボードにコピーする (Windows)。
fn copy_image_to_clipboard(path: &std::path::Path) {
    #[cfg(windows)]
    {
        let path_str = path.to_string_lossy().replace('\'', "''");
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; \
             $img = [System.Drawing.Image]::FromFile('{}'); \
             [System.Windows.Forms.Clipboard]::SetImage($img); \
             $img.Dispose()",
            path_str
        );
        let mut cmd = std::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-Command", &script]);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);
        let _ = cmd.spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
}

/// ファイルの親フォルダをエクスプローラで開き、ファイルを選択する。
fn open_folder_in_explorer(path: &std::path::Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer")
            .arg("/select,")
            .arg(path.as_os_str())
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
}
