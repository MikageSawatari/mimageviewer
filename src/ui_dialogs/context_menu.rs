//! サムネイルグリッドの右クリックコンテキストメニュー。

use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc;

use crate::grid_item::GridItem;

impl crate::app::App {
    /// コンテキストメニューを表示する。
    pub(crate) fn show_context_menu(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        let idx = match self.context_menu_idx {
            Some(i) => i,
            None => return None,
        };

        // usize::MAX = 空フォルダでの右クリック（フォルダ操作のみ）
        let is_folder_context = idx == usize::MAX;
        let item = if is_folder_context {
            // 現在のフォルダをフォルダアイテムとして扱う
            match self.current_folder.clone() {
                Some(p) => GridItem::Folder(p),
                None => {
                    self.context_menu_idx = None;
                    return None;
                }
            }
        } else {
            match self.items.get(idx) {
                Some(item) => item.clone(),
                None => {
                    self.context_menu_idx = None;
                    return None;
                }
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
                    if ui
                        .button(format!("削除 (ゴミ箱) [{checked_count}件]"))
                        .clicked()
                    {
                        let targets: Vec<(usize, PathBuf)> = self.collect_checked_indexed_paths();
                        self.delete_targets = targets;
                        self.show_delete_confirm = true;
                        close = true;
                    }

                    // ペースト
                    if ui.button("ペースト (Ctrl+V)").clicked() {
                        if let Some(ref folder) = self.current_folder {
                            let rx = paste_files_from_clipboard(folder);
                            self.paste_pending.push(rx);
                        }
                        close = true;
                    }

                    ui.separator();
                    if ui.button("選択解除 (Ctrl+D)").clicked() {
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
                            if ui.button("ファイル名をコピー").clicked() {
                                let name = p
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("")
                                    .to_string();
                                ctx.copy_text(name);
                                close = true;
                            }
                            if matches!(item, GridItem::Image(_)) {
                                if ui.button("画像をクリップボードにコピー").clicked()
                                {
                                    copy_image_to_clipboard(p);
                                    close = true;
                                }
                            }
                            if matches!(item, GridItem::Video(_)) {
                                if ui.button("この動画をアップスケール登録").clicked()
                                {
                                    self.request_video_upscale(p.clone());
                                    close = true;
                                }
                                if ui.button("この動画のアップスケールを削除").clicked()
                                {
                                    self.request_video_upscale_artifact_delete(p.clone());
                                    close = true;
                                }
                                if ui.button("アップスケールタスク表示").clicked() {
                                    self.show_video_upscale_tasks = true;
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
                            // ── アプリケーションで開く ──
                            ui.separator();
                            let _ = self.render_open_with_menu(ui, p, &mut close);
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
                            ui.separator();
                            if ui.button("ペースト (Ctrl+V)").clicked() {
                                if let Some(ref folder) = self.current_folder {
                                    let rx = paste_files_from_clipboard(folder);
                                    self.paste_pending.push(rx);
                                }
                                close = true;
                            }
                        }
                        GridItem::Folder(p) | GridItem::ZipFile(p) | GridItem::PdfFile(p) => {
                            if ui.button("パスをコピー").clicked() {
                                ctx.copy_text(p.to_string_lossy().to_string());
                                close = true;
                            }
                            if matches!(item, GridItem::Folder(_)) {
                                if ui.button("エクスプローラで開く").clicked() {
                                    let _ = std::process::Command::new("explorer")
                                        .arg(p.as_os_str())
                                        .spawn();
                                    close = true;
                                }
                            } else {
                                if ui.button("フォルダを開く").clicked() {
                                    open_folder_in_explorer(p);
                                    close = true;
                                }
                                // ── アプリケーションで開く (ZipFile/PdfFile) ──
                                ui.separator();
                                let _ = self.render_open_with_menu(ui, p, &mut close);
                            }
                            ui.separator();
                            if ui.button("ペースト (Ctrl+V)").clicked() {
                                if let Some(ref folder) = self.current_folder {
                                    let rx = paste_files_from_clipboard(folder);
                                    self.paste_pending.push(rx);
                                }
                                close = true;
                            }
                        }
                        GridItem::ZipImage {
                            zip_path,
                            entry_name,
                        } => {
                            let display = format!("{}:{}", zip_path.display(), entry_name);
                            if ui.button("パスをコピー").clicked() {
                                ctx.copy_text(display);
                                close = true;
                            }
                            let basename = crate::zip_loader::entry_basename(entry_name);
                            if ui.button("ファイル名をコピー").clicked() {
                                ctx.copy_text(basename.to_string());
                                close = true;
                            }
                            if ui.button("画像をクリップボードにコピー").clicked() {
                                copy_zip_image_to_clipboard(zip_path, entry_name);
                                close = true;
                            }
                        }
                        GridItem::ZipSeparator { .. } => {
                            close = true;
                        }
                        GridItem::SearchContainer { path, .. } => {
                            // Ctrl+G 結果コンテナ: フォルダ扱いで最小限の操作を出す
                            if ui.button("パスをコピー").clicked() {
                                ctx.copy_text(path.to_string_lossy().to_string());
                                close = true;
                            }
                        }
                        GridItem::PdfPage {
                            pdf_path, page_num, ..
                        } => {
                            let display = format!("{}:Page {}", pdf_path.display(), page_num + 1);
                            if ui.button("パスをコピー").clicked() {
                                ctx.copy_text(display);
                                close = true;
                            }
                            if ui.button("ページ名をコピー").clicked() {
                                ctx.copy_text(format!("Page {}", page_num + 1));
                                close = true;
                            }
                        }
                        GridItem::ConvertibleArchive { path, .. } => {
                            if ui.button("パスをコピー").clicked() {
                                ctx.copy_text(path.to_string_lossy().to_string());
                                close = true;
                            }
                            if ui.button("フォルダを開く").clicked() {
                                open_folder_in_explorer(path);
                                close = true;
                            }
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
            self.cached_handlers = None;
        }

        nav
    }

    /// フルスクリーン表示中のコンテキストメニューを表示する。
    /// 右クリック長押しでトリガーされる。
    /// アプリケーション起動によりフルスクリーンを閉じるべき場合は true を返す。
    pub(crate) fn show_fs_context_menu(&mut self, ctx: &egui::Context) -> bool {
        let idx = match self.fs_context_menu_idx {
            Some(i) => i,
            None => return false,
        };

        let item = match self.items.get(idx) {
            Some(item) => item.clone(),
            None => {
                self.fs_context_menu_idx = None;
                return false;
            }
        };

        let mut close = false;
        let mut close_fullscreen = false;
        let pos = self.fs_context_menu_pos;

        let mut open = true;
        egui::Window::new("fs_context_menu")
            .id(egui::Id::new("fs_ctx_menu"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .fixed_pos(pos)
            .order(egui::Order::Tooltip)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(200.0);

                match &item {
                    GridItem::Image(p) | GridItem::Video(p) => {
                        if ui.button("パスをコピー").clicked() {
                            ctx.copy_text(p.to_string_lossy().to_string());
                            close = true;
                        }
                        if ui.button("ファイル名をコピー").clicked() {
                            let name = p
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("")
                                .to_string();
                            ctx.copy_text(name);
                            close = true;
                        }
                        if matches!(item, GridItem::Image(_)) {
                            if ui.button("画像をクリップボードにコピー").clicked() {
                                copy_image_to_clipboard(p);
                                close = true;
                            }
                        }
                        if ui.button("フォルダを開く").clicked() {
                            open_folder_in_explorer(p);
                            close = true;
                        }
                        // ── アプリケーションで開く ──
                        ui.separator();
                        close_fullscreen |= self.render_open_with_menu(ui, p, &mut close);
                    }
                    GridItem::ZipFile(p) | GridItem::PdfFile(p) => {
                        if ui.button("パスをコピー").clicked() {
                            ctx.copy_text(p.to_string_lossy().to_string());
                            close = true;
                        }
                        if ui.button("フォルダを開く").clicked() {
                            open_folder_in_explorer(p);
                            close = true;
                        }
                        // ── アプリケーションで開く ──
                        ui.separator();
                        close_fullscreen |= self.render_open_with_menu(ui, p, &mut close);
                    }
                    GridItem::ZipImage {
                        zip_path,
                        entry_name,
                    } => {
                        let display = format!("{}:{}", zip_path.display(), entry_name);
                        if ui.button("パスをコピー").clicked() {
                            ctx.copy_text(display);
                            close = true;
                        }
                        let basename = crate::zip_loader::entry_basename(entry_name);
                        if ui.button("ファイル名をコピー").clicked() {
                            ctx.copy_text(basename.to_string());
                            close = true;
                        }
                        if ui.button("画像をクリップボードにコピー").clicked() {
                            copy_zip_image_to_clipboard(zip_path, entry_name);
                            close = true;
                        }
                    }
                    GridItem::PdfPage {
                        pdf_path, page_num, ..
                    } => {
                        let display = format!("{}:Page {}", pdf_path.display(), page_num + 1);
                        if ui.button("パスをコピー").clicked() {
                            ctx.copy_text(display);
                            close = true;
                        }
                        if ui.button("ページ名をコピー").clicked() {
                            ctx.copy_text(format!("Page {}", page_num + 1));
                            close = true;
                        }
                    }
                    GridItem::Folder(_) | GridItem::ZipSeparator { .. } => {
                        close = true;
                    }
                    GridItem::ConvertibleArchive { path, .. } => {
                        if ui.button("パスをコピー").clicked() {
                            ctx.copy_text(path.to_string_lossy().to_string());
                            close = true;
                        }
                        if ui.button("フォルダを開く").clicked() {
                            open_folder_in_explorer(path);
                            close = true;
                        }
                    }
                    GridItem::SearchContainer { path, .. } => {
                        // Ctrl+G 結果ビューのコンテナ (v0.8.0): コピー系のみ最低限
                        if ui.button("パスをコピー").clicked() {
                            ctx.copy_text(path.to_string_lossy().to_string());
                            close = true;
                        }
                        if ui.button("フォルダを開く").clicked() {
                            open_folder_in_explorer(path);
                            close = true;
                        }
                    }
                }

                // メニュー外クリックで閉じる
                // 右クリック長押しからの遷移時、右ボタンのリリースで
                // secondary_clicked() が発火するため、左クリックのみで判定する
                if ui.input(|i| i.pointer.primary_clicked()) && !ui.ui_contains_pointer() {
                    close = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });

        if close || !open {
            self.fs_context_menu_idx = None;
            self.cached_handlers = None;
        }
        close_fullscreen
    }

    /// 「アプリケーションで開く」サブメニューを描画する。
    /// Image / ZipFile / PdfFile で共通のロジック。
    /// アプリが起動された場合は true を返す。
    fn render_open_with_menu(
        &mut self,
        ui: &mut egui::Ui,
        file_path: &std::path::Path,
        close: &mut bool,
    ) -> bool {
        let ext = file_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e.to_lowercase()))
            .unwrap_or_default();
        let file_path_owned = file_path.to_path_buf();
        let mut app_launched = false;

        // 直近使用アプリ（最大3件）
        for recent in self.settings.recent_open_with_apps.clone() {
            let label = format!("{}で開く", recent.display_name);
            if ui.button(&label).clicked() {
                crate::open_with::launch_with_app(&recent.exe_path, &file_path_owned);
                self.settings
                    .record_recent_open_with(recent.display_name, recent.exe_path);
                self.settings.save();
                *close = true;
                app_launched = true;
            }
        }

        // アプリ一覧（折りたたみ展開）
        egui::CollapsingHeader::new("アプリケーションで開く…").show(ui, |ui| {
            // カスタムアプリ
            let custom_apps = self.settings.custom_open_with_apps.clone();
            if !custom_apps.is_empty() {
                for app in &custom_apps {
                    if ui.button(&app.display_name).clicked() {
                        crate::open_with::launch_with_app(&app.exe_path, &file_path_owned);
                        self.settings.record_recent_open_with(
                            app.display_name.clone(),
                            app.exe_path.clone(),
                        );
                        self.settings.save();
                        *close = true;
                        app_launched = true;
                    }
                }
                ui.separator();
            }

            // システム関連付けアプリ（キャッシュ）
            let handlers = match &self.cached_handlers {
                Some((cached_ext, h)) if cached_ext == &ext => h.clone(),
                _ => {
                    let h = crate::open_with::enumerate_handlers(&ext);
                    self.cached_handlers = Some((ext.clone(), h.clone()));
                    h
                }
            };
            for handler in &handlers {
                if ui.button(&handler.display_name).clicked() {
                    crate::open_with::launch_with_app(&handler.exe_path, &file_path_owned);
                    self.settings.record_recent_open_with(
                        handler.display_name.clone(),
                        handler.exe_path.clone(),
                    );
                    self.settings.save();
                    *close = true;
                    app_launched = true;
                }
            }

            // アプリ追加ボタン
            ui.separator();
            if ui.button("アプリケーションを追加…").clicked() {
                if let Some(app) = crate::open_with::pick_exe_dialog() {
                    let already = self
                        .settings
                        .custom_open_with_apps
                        .iter()
                        .any(|a| a.exe_path.eq_ignore_ascii_case(&app.exe_path));
                    if !already {
                        self.settings
                            .custom_open_with_apps
                            .push(crate::settings::RecentApp {
                                display_name: app.display_name,
                                exe_path: app.exe_path,
                            });
                        self.settings.save();
                    }
                }
            }
        });
        app_launched
    }

    /// チェック済みアイテムのパスを収集する。
    pub(crate) fn collect_checked_paths(&self) -> Vec<PathBuf> {
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
        let mut do_start_delete = false;
        egui::Window::new("削除の確認")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(&label);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("削除").clicked() {
                        do_start_delete = true;
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

        if do_start_delete {
            // 削除確認は閉じ、path だけを worker に渡す (idx は完了時に再解決)。
            let paths: Vec<std::path::PathBuf> =
                self.delete_targets.iter().map(|(_, p)| p.clone()).collect();
            self.show_delete_confirm = false;
            self.delete_targets.clear();
            self.start_delete_files(paths);
        }
    }

    /// 削除進捗ダイアログ。`delete_pending` がある間だけ表示される。
    ///
    /// `egui::Modal` を使い、背景のグリッド / メニュー / ショートカット入力を
    /// 遮断する。スクロール自体は egui 側で吸われるが、背景描画の更新は
    /// `check_external_folder_changes` などを `delete_pending` でガードしているので
    /// 進まない (削除の競合状態を避けるため)。
    pub(crate) fn show_delete_progress_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.delete_pending.as_ref() else {
            return;
        };
        // worker 動作中は毎フレーム再描画して進捗が止まって見えないようにする。
        ctx.request_repaint();

        let total = pending.total;
        let processed = pending.processed();
        let succeeded = pending.succeeded.len();
        let failed = pending.failed.len();
        let canceling = pending.cancel.load(std::sync::atomic::Ordering::Relaxed);

        let mut cancel_requested = false;
        egui::Modal::new(egui::Id::new("delete_progress_modal")).show(ctx, |ui| {
            ui.set_min_width(280.0);
            ui.heading("削除中");
            ui.add_space(4.0);
            ui.label(format!("{processed} / {total}"));
            if failed > 0 {
                ui.colored_label(
                    egui::Color32::from_rgb(220, 80, 80),
                    format!("(失敗 {failed} 件)"),
                );
            } else {
                ui.label(format!("成功 {succeeded} 件"));
            }
            let ratio = if total > 0 {
                processed as f32 / total as f32
            } else {
                0.0
            };
            ui.add(egui::ProgressBar::new(ratio).show_percentage());
            ui.add_space(6.0);
            if canceling {
                ui.label("キャンセル中…");
            } else if ui.button("キャンセル").clicked() {
                cancel_requested = true;
            }
        });
        if cancel_requested {
            if let Some(p) = self.delete_pending.as_ref() {
                p.cancel();
            }
        }
    }

    /// DEL キーで選択画像を削除するハンドラ。
    pub(crate) fn handle_delete_key(&mut self, ctx: &egui::Context) {
        if self.fullscreen_idx.is_some() || self.address_has_focus || self.any_dialog_open() {
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
}

// ---------------------------------------------------------------------------
// OS 操作ヘルパー
// ---------------------------------------------------------------------------
// ゴミ箱移動は `src/delete_worker.rs` に集約 (バックグラウンド実行 + バッチ)。

/// クリップボード書き込みジョブの最新世代番号。発行時点で seq を取り、worker が
/// 実際にクリップボードに書く直前にこの値と比較、古ければ書き込みをスキップする。
/// これで「遅いコピー A の後に速いコピー B をクリック」ケースで A が B を
/// 上書きするのを防ぐ (画像 decode / PowerShell 起動いずれも対象)。
static CLIPBOARD_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// クリップボード書き込み (OpenClipboard/SetClipboardData と PowerShell copy/cut) を
/// 直列化するミューテックス。seq チェックは必ずこの lock 内で実行し、
/// 「チェック通過 → 実際の書き込み」の間に別の writer が割り込まないようにする。
/// 画像 clipboard の DIB 構築 / PowerShell プロセス実行 (`cmd.status()`) もこの lock の
/// 配下で行う — 古い writer が遅れて clipboard を上書きする race を閉じる。
/// paste (clipboard 読み出し + FS 操作) はここには入らない (書き込み競合しないため)。
static CLIPBOARD_WRITE_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 次世代の seq を発行する。発行値が最新 (= CLIPBOARD_SEQ の現在値) かをチェックするには
/// `CLIPBOARD_SEQ.load()` と比較する。
fn bump_clipboard_seq() -> u64 {
    CLIPBOARD_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

/// `my_seq` がまだ最新なら true。古ければ false — 書き込みをスキップすべき合図。
fn clipboard_seq_is_latest(my_seq: u64) -> bool {
    CLIPBOARD_SEQ.load(std::sync::atomic::Ordering::Relaxed) == my_seq
}

/// ファイルをクリップボードにコピー (エクスプローラのコピーと同等)。
pub fn copy_files_to_clipboard(paths: &[PathBuf]) {
    #[cfg(windows)]
    {
        if paths.is_empty() {
            return;
        }
        let my_seq = bump_clipboard_seq();
        let paths_str: Vec<String> = paths
            .iter()
            .map(|p| format!("'{}'", p.to_string_lossy().replace('\'', "''")))
            .collect();
        let arr = paths_str.join(",");
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms\n\
             $col = New-Object System.Collections.Specialized.StringCollection\n\
             @({arr}) | ForEach-Object {{ $col.Add($_) | Out-Null }}\n\
             [System.Windows.Forms.Clipboard]::SetFileDropList($col)\n"
        );
        run_ps_script_async_seq(script, None, my_seq);
    }
    #[cfg(not(windows))]
    {
        let _ = paths;
    }
}

/// ファイルをクリップボードにカット (移動操作用)。
pub fn cut_files_to_clipboard(paths: &[PathBuf]) {
    #[cfg(windows)]
    {
        if paths.is_empty() {
            return;
        }
        let my_seq = bump_clipboard_seq();
        let paths_str: Vec<String> = paths
            .iter()
            .map(|p| format!("'{}'", p.to_string_lossy().replace('\'', "''")))
            .collect();
        let arr = paths_str.join(",");
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms\n\
             $col = New-Object System.Collections.Specialized.StringCollection\n\
             @({arr}) | ForEach-Object {{ $col.Add($_) | Out-Null }}\n\
             $data = New-Object System.Windows.Forms.DataObject\n\
             $data.SetFileDropList($col)\n\
             $ms = New-Object System.IO.MemoryStream(4)\n\
             $ms.Write([BitConverter]::GetBytes(2), 0, 4)\n\
             $data.SetData('Preferred DropEffect', $ms)\n\
             [System.Windows.Forms.Clipboard]::SetDataObject($data, $true)\n"
        );
        run_ps_script_async_seq(script, None, my_seq);
    }
    #[cfg(not(windows))]
    {
        let _ = paths;
    }
}

/// 画像ファイルの内容をクリップボードにコピーする (Windows)。
/// 画像ファイルをデコードしてクリップボードにコピーする。
/// image クレートで非対応の形式は WIC にフォールバック。
///
/// decode + DIB 構築は worker スレッドで行う。20MP 超や巨大 RAW では
/// 数百ms〜秒単位かかるため、UI スレッドから同期実行すると右クリック操作で固まる。
/// 発行時に `CLIPBOARD_SEQ` を bump し、set 直前に最新 seq と比較、自分が古ければ
/// set をスキップする — 遅い A が速い B を追い越して上書きするのを防ぐ。
fn copy_image_to_clipboard(path: &std::path::Path) {
    let path = path.to_path_buf();
    let my_seq = bump_clipboard_seq();
    std::thread::Builder::new()
        .name("clipboard-image-copy".into())
        .spawn(move || {
            let img = match image::open(&path) {
                Ok(i) => i,
                Err(_) => {
                    #[cfg(windows)]
                    {
                        match crate::wic_decoder::decode_to_dynamic_image(&path) {
                            Some(i) => i,
                            None => return,
                        }
                    }
                    #[cfg(not(windows))]
                    return;
                }
            };
            // ここの pre-check は DIB 構築を省くだけの best-effort 短絡。
            // 正式な stale 判定は `set_image_to_clipboard` 内部で
            // `CLIPBOARD_WRITE_MUTEX` を握った状態で行われる。
            if !clipboard_seq_is_latest(my_seq) {
                return;
            }
            set_image_to_clipboard(&img, my_seq);
        })
        .ok();
}

/// バイト列から画像をデコードしてクリップボードにコピー (ZIP 内画像用)。
/// ZIP エントリ読み出し + decode + DIB 構築はまとめて worker に回す。
/// 巨大 ZIP の read_entry_bytes も I/O 待ちで UI を止めるため、そちらも含めてここで吸収する。
fn copy_zip_image_to_clipboard(zip_path: &std::path::Path, entry_name: &str) {
    let zip_path = zip_path.to_path_buf();
    let entry_name = entry_name.to_string();
    let my_seq = bump_clipboard_seq();
    std::thread::Builder::new()
        .name("clipboard-zip-image-copy".into())
        .spawn(move || {
            let Ok(bytes) = crate::zip_loader::read_entry_bytes(&zip_path, &entry_name) else {
                return;
            };
            let Ok(img) = image::load_from_memory(&bytes) else {
                return;
            };
            // pre-check は best-effort 短絡 (DIB 構築省略)。正式な stale 判定は
            // `set_image_to_clipboard` 側の mutex 内で行う。
            if !clipboard_seq_is_latest(my_seq) {
                return;
            }
            set_image_to_clipboard(&img, my_seq);
        })
        .ok();
}

/// DynamicImage をクリップボードに CF_DIB として設定する。
///
/// `my_seq` は発行時に取得した世代番号。`CLIPBOARD_WRITE_MUTEX` を保持した状態で
/// seq が最新であることを再チェックし、古ければ何もしない。これにより
/// 「decode/DIB 構築に時間がかかっている間に別のコピー B が先に完了して clipboard を
/// 更新 → 古い A が後から B を上書きする」race を回避する。
///
/// 重い `to_rgba8` + ピクセル並べ替えは lock 外のローカルバッファで完結させ、
/// mutex を握るのは OS クリップボード API 呼び出し区間に絞って他の writer を
/// 不要に待たせない。
fn set_image_to_clipboard(img: &image::DynamicImage, my_seq: u64) {
    let rgba = img.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();

    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::DataExchange::{
            CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
        };
        use windows::Win32::System::Memory::{
            GLOBAL_ALLOC_FLAGS, GlobalAlloc, GlobalLock, GlobalUnlock,
        };
        use windows::Win32::System::Ole::CF_DIB;

        let row_size = (width * 3 + 3) & !3;
        let pixel_size = row_size * height;
        let header_size: u32 = 40;
        let total_size = header_size as usize + pixel_size as usize;

        // 重い DIB 構築は lock 外でローカルバッファに済ませる。
        let mut buf = vec![0u8; total_size];
        buf[0..4].copy_from_slice(&header_size.to_le_bytes());
        buf[4..8].copy_from_slice(&(width as i32).to_le_bytes());
        buf[8..12].copy_from_slice(&(height as i32).to_le_bytes());
        buf[12..14].copy_from_slice(&1u16.to_le_bytes());
        buf[14..16].copy_from_slice(&24u16.to_le_bytes());
        let pixels = &rgba;
        for y in 0..height {
            let src_row = (height - 1 - y) as usize;
            let dst_offset = header_size as usize + (y * row_size) as usize;
            for x in 0..width {
                let src_idx = (src_row * width as usize + x as usize) * 4;
                let dst_idx = dst_offset + (x * 3) as usize;
                buf[dst_idx] = pixels.as_raw()[src_idx + 2];
                buf[dst_idx + 1] = pixels.as_raw()[src_idx + 1];
                buf[dst_idx + 2] = pixels.as_raw()[src_idx];
            }
        }

        // 実際のクリップボード書き込みは直列化 + seq 再確認。
        let Ok(_lock) = CLIPBOARD_WRITE_MUTEX.lock() else {
            return;
        };
        if !clipboard_seq_is_latest(my_seq) {
            return;
        }

        unsafe {
            let hmem = GlobalAlloc(GLOBAL_ALLOC_FLAGS(0x0042), total_size);
            let Ok(hmem) = hmem else { return };
            let ptr = GlobalLock(hmem);
            if ptr.is_null() {
                return;
            }
            std::ptr::copy_nonoverlapping(buf.as_ptr(), ptr as *mut u8, total_size);
            let _ = GlobalUnlock(hmem);

            if OpenClipboard(None).is_ok() {
                let _ = EmptyClipboard();
                let _ = SetClipboardData(CF_DIB.0 as u32, Some(HANDLE(hmem.0)));
                let _ = CloseClipboard();
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (width, height, my_seq);
    }
}

/// クリップボードにあるファイルを指定フォルダにペースト（コピーまたは移動）する。
///
/// PowerShell 起動 + Shell clipboard I/O が数百ms〜秒級になることがあるため、worker で走らせる。
/// 戻り値は「ペースト完了」を 1 回通知する `mpsc::Receiver`。App は pending に積み、
/// 完了したらフォルダを再読込する (docs/ui-responsiveness.md §4)。
pub fn paste_files_from_clipboard(dest_folder: &std::path::Path) -> mpsc::Receiver<()> {
    let (tx, rx) = mpsc::channel();
    #[cfg(windows)]
    {
        let dest = dest_folder.to_string_lossy().replace('\'', "''");
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms\n\
             $data = [System.Windows.Forms.Clipboard]::GetDataObject()\n\
             if ($data -eq $null -or -not $data.ContainsFileDropList()) {{ exit }}\n\
             $files = $data.GetFileDropList()\n\
             $effect = $data.GetData('Preferred DropEffect')\n\
             $isMove = $false\n\
             if ($effect -ne $null) {{\n\
               $bytes = New-Object byte[] 4\n\
               $null = $effect.Read($bytes, 0, 4)\n\
               if ([BitConverter]::ToInt32($bytes, 0) -eq 2) {{ $isMove = $true }}\n\
             }}\n\
             foreach ($f in $files) {{\n\
               if ($isMove) {{\n\
                 Move-Item -Path $f -Destination '{dest}' -Force\n\
               }} else {{\n\
                 Copy-Item -Path $f -Destination '{dest}' -Force -Recurse\n\
               }}\n\
             }}\n\
             if ($isMove) {{ [System.Windows.Forms.Clipboard]::Clear() }}\n"
        );
        run_ps_script_async(script, Some(tx));
    }
    #[cfg(not(windows))]
    {
        let _ = dest_folder;
        let _ = tx; // drop — receiver will get Disconnected
    }
    rx
}

/// PowerShell スクリプトを一時ファイル経由で worker スレッドで実行する共通ヘルパー。
/// -STA (クリップボード API 必須) / -ExecutionPolicy Bypass / CREATE_NO_WINDOW で実行。
/// スクリプトは UTF-8 BOM 付きで書き出す（日本語パス対応）。
///
/// 一時ファイル名は呼び出しごとにユニーク化して並行操作の衝突を避ける。
/// 完了を知りたい呼び出し元は `on_done` に `mpsc::Sender<()>` を渡す (paste 用)。
/// clipboard copy/cut はファイア & フォーゲットなので None を渡す。
///
/// paste 用 (クリップボードの書き込みではなく読み出し → FS 操作) には生 API の方を呼ぶ。
#[cfg(windows)]
fn run_ps_script_async(script: String, on_done: Option<mpsc::Sender<()>>) {
    run_ps_script_inner(script, on_done, None);
}

/// clipboard copy/cut (PowerShell で書き込み) 用。seq が古ければ PowerShell 起動を
/// スキップする。
#[cfg(windows)]
fn run_ps_script_async_seq(script: String, on_done: Option<mpsc::Sender<()>>, my_seq: u64) {
    run_ps_script_inner(script, on_done, Some(my_seq));
}

#[cfg(windows)]
fn run_ps_script_inner(
    script: String,
    on_done: Option<mpsc::Sender<()>>,
    clipboard_seq: Option<u64>,
) {
    // process id + atomic counter で temp ファイル衝突を回避。
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("miv_ps_{}_{}.ps1", std::process::id(), seq));
    std::thread::Builder::new()
        .name("powershell-clipboard-exec".into())
        .spawn(move || {
            // clipboard 書き込み系 (copy/cut) は `CLIPBOARD_WRITE_MUTEX` を握ったうえで
            // seq 再確認 → PowerShell 実行まで lock 内で直列化する。PowerShell プロセス内で
            // 実際の SetClipboardData が走るため、`cmd.status()` が返るまで lock を離さない
            // ことで「A の PS 起動後に B が割り込んで先に clipboard を書き、そのあと A が
            // 遅れて上書きする」race を閉じる。
            // paste (clipboard 読み出し + FS 操作) は clipboard_seq=None で呼ばれるので
            // lock を取らない — 書き込み系を不要にブロックしない。
            let _clipboard_lock = if clipboard_seq.is_some() {
                Some(
                    CLIPBOARD_WRITE_MUTEX
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()),
                )
            } else {
                None
            };
            if let Some(my_seq) = clipboard_seq {
                if !clipboard_seq_is_latest(my_seq) {
                    if let Some(tx) = on_done {
                        let _ = tx.send(());
                    }
                    return;
                }
            }
            // UTF-8 BOM (0xEF 0xBB 0xBF) + スクリプト本文
            let mut content = vec![0xEF, 0xBB, 0xBF];
            content.extend_from_slice(script.as_bytes());
            if std::fs::write(&tmp, &content).is_ok() {
                let mut cmd = std::process::Command::new("powershell");
                cmd.args([
                    "-NoProfile",
                    "-STA",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    &tmp.to_string_lossy(),
                ]);
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x08000000);
                }
                let _ = cmd.status();
                let _ = std::fs::remove_file(&tmp);
            }
            // ここで `_clipboard_lock` が drop されロック解放。
            if let Some(tx) = on_done {
                let _ = tx.send(());
            }
        })
        .ok();
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
