//! サムネイルグリッドの右クリックコンテキストメニュー。

use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use crate::app::is_synthetic_view_path;
use crate::grid_item::GridItem;
use crate::native_context_menu::{
    NativeContextMenuRequest, NativeContextMenuResult, NativeMivCommand, NativeMivMenuItem,
};

/// `show_context_menu` の戻り値。単なる `Option<PathBuf>` ではなくアクション種別を
/// 表現することで、検索終了などの副作用を呼び出し側 (= 優先度判定後) で発火できる。
/// これにより別 nav 源 (キーボード等) が同じフレームで勝った場合に context_nav の副作用
/// だけが先に走るという順序の脆さを避ける (Codex P3)。
#[derive(Debug)]
pub(crate) enum ContextMenuAction {
    /// 検索結果 (Ctrl+G / Ctrl+S) から実フォルダへ着地。検索を明示終了し、検索前フォルダを
    /// 履歴に積んで `suppress_folder_nav_record_once` を立てる遷移。実適用は
    /// `apply_jump_from_search_to`。
    JumpFromSearch(PathBuf),
    /// ZIP/PDF/対応アーカイブを、グローバル設定ではなく明示モードで開く。
    OpenGridContainer {
        idx: usize,
        mode: crate::app::GridContainerOpenMode,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeleteConfirmKind {
    RecycleBin,
    MayPermanent,
}

#[derive(Debug)]
enum NativeGridContextMenuOutcome {
    Consumed(Option<ContextMenuAction>),
    Fallback,
}

#[derive(Clone)]
struct NativeGridContextMenuTarget {
    paths: Vec<PathBuf>,
    item: GridItem,
    item_index: Option<usize>,
    is_folder_context: bool,
    has_checked: bool,
}

fn native_grid_context_menu_target_kind(target: &NativeGridContextMenuTarget) -> &'static str {
    if target.is_folder_context {
        "background"
    } else if target.has_checked {
        "checked_paths"
    } else {
        "item_path"
    }
}

fn delete_confirm_label_for_targets(targets: &[(usize, PathBuf)]) -> String {
    let kind = if delete_targets_may_permanently_delete(targets) {
        DeleteConfirmKind::MayPermanent
    } else {
        DeleteConfirmKind::RecycleBin
    };
    let count = targets.len();
    let single_name = (count == 1).then(|| {
        targets[0]
            .1
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
    });
    build_delete_confirm_label(count, single_name, kind)
}

fn build_delete_confirm_label(
    count: usize,
    single_name: Option<&str>,
    kind: DeleteConfirmKind,
) -> String {
    match (kind, count, single_name) {
        (DeleteConfirmKind::RecycleBin, 1, Some(name)) => {
            format!("「{name}」をゴミ箱に移動しますか？")
        }
        (DeleteConfirmKind::RecycleBin, _, _) => {
            format!("{count} 件の項目をゴミ箱に移動しますか？")
        }
        (DeleteConfirmKind::MayPermanent, 1, Some(name)) => format!(
            "「{name}」はゴミ箱に移動できない場所にある可能性があります。\n\
             完全に削除される場合があります。削除しますか？"
        ),
        (DeleteConfirmKind::MayPermanent, _, _) => format!(
            "{count} 件のうち、ゴミ箱に移動できない場所の項目があります。\n\
             完全に削除される場合があります。削除しますか？"
        ),
    }
}

fn native_path_text(path: &Path) -> String {
    let text = path.to_string_lossy().to_string();
    #[cfg(windows)]
    {
        text.replace('/', "\\")
    }
    #[cfg(not(windows))]
    {
        text
    }
}

fn copy_path_text(ctx: &egui::Context, path: &Path) {
    ctx.copy_text(native_path_text(path));
}

fn legacy_xmp_context_path(item: &GridItem) -> Option<PathBuf> {
    match item {
        GridItem::Image(p) | GridItem::Video(p) => Some(p.clone()),
        _ => None,
    }
}

#[cfg(windows)]
fn delete_targets_may_permanently_delete(targets: &[(usize, PathBuf)]) -> bool {
    let mut checked_roots: std::collections::HashMap<String, Option<u64>> =
        std::collections::HashMap::new();
    for (_, path) in targets {
        let Some(root) = windows_path_root_for_file_operation(path) else {
            return true;
        };
        if !checked_roots.contains_key(&root) {
            if windows_root_may_permanently_delete(&root) {
                return true;
            }
            checked_roots.insert(root.clone(), windows_recycle_bin_max_capacity_bytes(&root));
        }
        if let Some(Some(max_capacity_bytes)) = checked_roots.get(&root) {
            if windows_file_exceeds_recycle_bin_capacity(path, *max_capacity_bytes) {
                return true;
            }
        }
    }
    false
}

#[cfg(not(windows))]
fn delete_targets_may_permanently_delete(_targets: &[(usize, PathBuf)]) -> bool {
    false
}

#[cfg(windows)]
fn windows_root_may_permanently_delete(root: &str) -> bool {
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;
    use windows::core::PCWSTR;

    let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
    let drive_type = unsafe { GetDriveTypeW(PCWSTR(wide.as_ptr())) };

    if windows_drive_type_may_permanently_delete(drive_type) {
        return true;
    }

    windows_recycle_bin_nuke_on_delete(root).unwrap_or(false)
}

#[cfg(windows)]
fn windows_drive_type_may_permanently_delete(drive_type: u32) -> bool {
    const DRIVE_UNKNOWN: u32 = 0;
    const DRIVE_NO_ROOT_DIR: u32 = 1;
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_CDROM: u32 = 5;
    const DRIVE_RAMDISK: u32 = 6;

    matches!(
        drive_type,
        DRIVE_REMOVABLE
            | DRIVE_REMOTE
            | DRIVE_CDROM
            | DRIVE_RAMDISK
            | DRIVE_NO_ROOT_DIR
            | DRIVE_UNKNOWN
    )
}

#[cfg(windows)]
fn windows_recycle_bin_nuke_on_delete(root: &str) -> Option<bool> {
    windows_recycle_bin_dword(root, "NukeOnDelete").map(|v| v != 0)
}

#[cfg(windows)]
fn windows_recycle_bin_max_capacity_bytes(root: &str) -> Option<u64> {
    windows_recycle_bin_dword(root, "MaxCapacity")
        .map(|mb| u64::from(mb).saturating_mul(1024 * 1024))
}

#[cfg(windows)]
fn windows_file_exceeds_recycle_bin_capacity(path: &Path, max_capacity_bytes: u64) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.len() > max_capacity_bytes)
        .unwrap_or(false)
}

#[cfg(windows)]
fn windows_recycle_bin_dword(root: &str, value_name: &str) -> Option<u32> {
    let volume_guid = windows_volume_guid_for_root(root)?;
    let subkey = format!(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\BitBucket\\Volume\\{volume_guid}"
    );
    windows_registry_dword(&subkey, value_name)
}

#[cfg(windows)]
fn windows_volume_guid_for_root(root: &str) -> Option<String> {
    use windows::Win32::Storage::FileSystem::GetVolumeNameForVolumeMountPointW;
    use windows::core::PCWSTR;

    let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buffer = vec![0u16; 64];
    unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(wide.as_ptr()), &mut buffer) }.ok()?;
    let end = buffer
        .iter()
        .position(|&ch| ch == 0)
        .unwrap_or(buffer.len());
    let volume_name = String::from_utf16_lossy(&buffer[..end]);
    let start = volume_name.find('{')?;
    let end = volume_name[start..].find('}')? + start + 1;
    Some(volume_name[start..end].to_owned())
}

#[cfg(windows)]
fn windows_registry_dword(subkey: &str, value_name: &str) -> Option<u32> {
    use std::ffi::c_void;
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_VALUE_TYPE, RRF_RT_REG_DWORD, RegGetValueW,
    };
    use windows::core::PCWSTR;

    let subkey: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let value_name: Vec<u16> = value_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let mut value_type = REG_VALUE_TYPE(0);
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_DWORD,
            Some(&mut value_type),
            Some(&mut data as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    result.is_ok().then_some(data)
}

#[cfg(windows)]
fn windows_path_root_for_file_operation(path: &Path) -> Option<String> {
    use std::path::{Component, Prefix};

    match path.components().next()? {
        Component::Prefix(prefix) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                Some(format!("{}:\\", (letter as char).to_ascii_uppercase()))
            }
            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => Some(format!(
                "\\\\{}\\{}\\",
                server.to_string_lossy(),
                share.to_string_lossy()
            )),
            _ => None,
        },
        _ => None,
    }
}

impl crate::app::App {
    /// コンテキストメニューを表示する。
    pub(crate) fn show_context_menu(&mut self, ctx: &egui::Context) -> Option<ContextMenuAction> {
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

        let has_checked = !is_folder_context && !self.checked.is_empty();
        let checked_count = self.checked.len();
        let mut nav: Option<ContextMenuAction> = None;
        // 検索結果ビュー中だけ「フォルダに移動」を出す。タグビューも対象 —
        // ディスク中に散在するタグ付きヒットから収納フォルダへ飛ぶのは
        // タグビューの主要動線そのもの (UX レビュー【🟧11】)。
        let in_search = self.items_are_global_search_view
            || self.global_search.drill.is_some()
            || self.favsearch.on_results_grid()
            || !self.favsearch.nav_stack.is_empty()
            || self.items_are_tag_view
            || !self.tag_view.nav_stack.is_empty();
        // 実フォルダ背景に対する mIV 管理操作 (新規フォルダ作成など) は
        // 「検索結果グリッドを表示中」だけ無効化する (外部 D&D と同じ判定)。
        // 検索前の実フォルダへ誤って作用しないよう、`active` ではなく on-results-grid で判定する。
        let on_search_results = self.items_are_global_search_view
            || self.favsearch.on_results_grid()
            || self.items_are_tag_view
            || self.tag_view.on_results_grid();
        let folder_command_target = if on_search_results {
            None
        } else {
            self.current_favorite_target()
        };
        let mut close = false;

        // 記録済みの座標に固定表示
        let pos = self.context_menu_pos;
        if !self.items_are_reading_history_view {
            match self.try_show_native_grid_context_menu(
                ctx,
                pos,
                idx,
                item.clone(),
                is_folder_context,
                has_checked,
                in_search,
                folder_command_target.clone(),
            ) {
                NativeGridContextMenuOutcome::Consumed(nav) => {
                    self.context_menu_idx = None;
                    self.cached_handlers = None;
                    ctx.request_repaint();
                    return nav;
                }
                NativeGridContextMenuOutcome::Fallback => {}
            }
        }

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
                    // ── 選択モード: チェック済みアイテムに対する操作 ──
                    ui.label(
                        egui::RichText::new(format!("{checked_count} 件選択中"))
                            .strong()
                            .size(13.0),
                    );
                    ui.separator();

                    // パスをコピー (disabled)
                    ui.add_enabled(false, egui::Button::new("パスをコピー"));

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

                    let legacy_xmp_paths = self
                        .checked
                        .iter()
                        .filter_map(|idx| self.items.get(*idx))
                        .filter_map(legacy_xmp_context_path)
                        .collect::<Vec<_>>();
                    self.draw_legacy_xmp_context_entries(ui, legacy_xmp_paths, &mut close);

                    // フォルダを開く (disabled)
                    ui.add_enabled(false, egui::Button::new("フォルダを開く"));

                    ui.separator();

                    // 削除 (ゴミ箱)
                    if ui
                        .button(format!("削除 (ゴミ箱) [{checked_count}件]"))
                        .clicked()
                    {
                        let targets: Vec<(usize, PathBuf)> = self.collect_checked_indexed_paths();
                        self.request_delete_confirm(targets);
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
                        GridItem::Image(p) | GridItem::Video(p) | GridItem::Audio(p) => {
                            if ui.button("パスをコピー").clicked() {
                                copy_path_text(ctx, p);
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
                                    let rotation = self.get_rotation(idx);
                                    copy_image_to_clipboard(p, rotation);
                                    close = true;
                                }
                            }
                            if ui.button("フォルダを開く").clicked() {
                                open_folder_in_explorer(p);
                                close = true;
                            }
                            if in_search && ui.button("フォルダに移動").clicked() {
                                nav =
                                    parent_folder_for_nav(p).map(ContextMenuAction::JumpFromSearch);
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
                            self.draw_legacy_xmp_context_entries(ui, vec![p.clone()], &mut close);
                            ui.separator();
                            if ui.button("削除 (ゴミ箱)").clicked() {
                                self.request_delete_confirm(vec![(idx, p.clone())]);
                                close = true;
                            }
                        }
                        GridItem::Folder(p) => {
                            let copy_label = if is_folder_context {
                                "このフォルダのパスをコピー"
                            } else {
                                "パスをコピー"
                            };
                            if ui.button(copy_label).clicked() {
                                copy_path_text(ctx, p);
                                close = true;
                            }
                            // フォルダのコピー / カット (クリップボード経由) は v1.1.0 で一旦
                            // 無効化 (データ破壊リスクのため将来へ延期)。ここには出さない。
                            let open_label = if is_folder_context {
                                "このフォルダを開く"
                            } else {
                                "エクスプローラで開く"
                            };
                            if ui.button(open_label).clicked() {
                                let _ = std::process::Command::new("explorer")
                                    .arg(native_path_text(p))
                                    .spawn();
                                close = true;
                            }
                            if in_search
                                && !is_folder_context
                                && ui.button("フォルダに移動").clicked()
                            {
                                nav = Some(ContextMenuAction::JumpFromSearch(native_nav_path(p)));
                                close = true;
                            }
                            if self.items_are_reading_history_view
                                && !is_folder_context
                                && ui.button("この本のフォルダに移動").clicked()
                            {
                                self.select_after_load = p
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .map(|s| s.to_string());
                                nav =
                                    parent_folder_for_nav(p).map(ContextMenuAction::JumpFromSearch);
                                close = true;
                            }
                            ui.separator();
                            if is_folder_context {
                                if ui
                                    .add_enabled(
                                        folder_command_target.is_some(),
                                        egui::Button::new("新しいフォルダ…"),
                                    )
                                    .clicked()
                                {
                                    if let Some(folder) = folder_command_target.clone() {
                                        self.request_new_folder_dialog(folder);
                                    }
                                    close = true;
                                }
                            } else {
                                if ui.button("削除 (ゴミ箱)").clicked() {
                                    self.request_delete_confirm(vec![(idx, p.clone())]);
                                    close = true;
                                }
                            }
                        }
                        GridItem::ZipFile(p) | GridItem::PdfFile(p) => {
                            if ui.button("パスをコピー").clicked() {
                                copy_path_text(ctx, p);
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
                            ui.separator();
                            if ui.button("ページを開く").clicked() {
                                nav = Some(ContextMenuAction::OpenGridContainer {
                                    idx,
                                    mode: crate::app::GridContainerOpenMode::PageFullscreen,
                                });
                                close = true;
                            }
                            if ui.button("一覧を開く").clicked() {
                                nav = Some(ContextMenuAction::OpenGridContainer {
                                    idx,
                                    mode: crate::app::GridContainerOpenMode::PageList,
                                });
                                close = true;
                            }
                            ui.separator();
                            if ui.button("フォルダを開く").clicked() {
                                open_folder_in_explorer(p);
                                close = true;
                            }
                            if in_search && ui.button("フォルダに移動").clicked() {
                                nav =
                                    parent_folder_for_nav(p).map(ContextMenuAction::JumpFromSearch);
                                close = true;
                            }
                            if self.items_are_reading_history_view
                                && ui.button("この本のフォルダに移動").clicked()
                            {
                                self.select_after_load = p
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .map(|s| s.to_string());
                                nav =
                                    parent_folder_for_nav(p).map(ContextMenuAction::JumpFromSearch);
                                close = true;
                            }
                            // ── アプリケーションで開く (ZipFile/PdfFile) ──
                            ui.separator();
                            let _ = self.render_open_with_menu(ui, p, &mut close);
                            ui.separator();
                            if ui.button("削除 (ゴミ箱)").clicked() {
                                self.request_delete_confirm(vec![(idx, p.clone())]);
                                close = true;
                            }
                        }
                        GridItem::ZipImage {
                            zip_path,
                            entry_name,
                        } => {
                            let display = format!("{}:{}", native_path_text(zip_path), entry_name);
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
                                let rotation = self.get_rotation(idx);
                                copy_zip_image_to_clipboard(zip_path, entry_name, rotation);
                                close = true;
                            }
                        }
                        GridItem::ZipSeparator { .. } => {
                            close = true;
                        }
                        GridItem::ZipDir {
                            zip_path,
                            dir_prefix,
                            ..
                        } => {
                            // ネスト ZIP の子コンテナ: 仮想パスのコピーのみ (実ファイル操作は不可)。
                            let display = format!("{}:{}", native_path_text(zip_path), dir_prefix);
                            if ui.button("パスをコピー").clicked() {
                                ctx.copy_text(display);
                                close = true;
                            }
                        }
                        GridItem::SearchContainer { path, .. } => {
                            // Ctrl+G 結果コンテナ: フォルダ扱いで最小限の操作を出す
                            if ui.button("パスをコピー").clicked() {
                                copy_path_text(ctx, path);
                                close = true;
                            }
                            if in_search && ui.button("フォルダに移動").clicked() {
                                nav =
                                    Some(ContextMenuAction::JumpFromSearch(native_nav_path(path)));
                                close = true;
                            }
                        }
                        GridItem::Stack { representative, .. } => {
                            // ファイル名スタック集約セル: 仮想コンテナなので最小限。展開 (読書) は
                            // 左ダブルクリック / Enter で行う。ここは代表画像のパスコピーのみ。
                            if ui.button("代表画像のパスをコピー").clicked() {
                                copy_path_text(ctx, representative);
                                close = true;
                            }
                        }
                        GridItem::PdfPage {
                            pdf_path, page_num, ..
                        } => {
                            let display =
                                format!("{}:Page {}", native_path_text(pdf_path), page_num + 1);
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
                                copy_path_text(ctx, path);
                                close = true;
                            }
                            if ui.button("ファイル名をコピー").clicked() {
                                let name = path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("")
                                    .to_string();
                                ctx.copy_text(name);
                                close = true;
                            }
                            ui.separator();
                            if ui.button("ページを開く").clicked() {
                                nav = Some(ContextMenuAction::OpenGridContainer {
                                    idx,
                                    mode: crate::app::GridContainerOpenMode::PageFullscreen,
                                });
                                close = true;
                            }
                            if ui.button("一覧を開く").clicked() {
                                nav = Some(ContextMenuAction::OpenGridContainer {
                                    idx,
                                    mode: crate::app::GridContainerOpenMode::PageList,
                                });
                                close = true;
                            }
                            ui.separator();
                            if ui.button("フォルダを開く").clicked() {
                                open_folder_in_explorer(path);
                                close = true;
                            }
                            if in_search && ui.button("フォルダに移動").clicked() {
                                nav = parent_folder_for_nav(path)
                                    .map(ContextMenuAction::JumpFromSearch);
                                close = true;
                            }
                            if self.items_are_reading_history_view
                                && ui.button("この本のフォルダに移動").clicked()
                            {
                                self.select_after_load = path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .map(|s| s.to_string());
                                nav = parent_folder_for_nav(path)
                                    .map(ContextMenuAction::JumpFromSearch);
                                close = true;
                            }
                            ui.separator();
                            if ui.button("削除 (ゴミ箱)").clicked() {
                                self.request_delete_confirm(vec![(idx, path.clone())]);
                                close = true;
                            }
                        }
                    }

                    // ── 代表サムネ固定 (pin) エントリ (separator 込み) ──
                    // 空フォルダの右クリックで合成された `GridItem::Folder(current_folder)`
                    // は rel="" で pin できないので呼ばない (Codex Phase D P3 指摘: 単独の
                    // separator が残るのを防ぐため呼び出し自体を skip)。
                    // pin 不能 / アグリゲートビュー / drill-down 等の条件分岐とそれに伴う
                    // separator 描画は helper 側に集約 (Codex Phase D 再指摘)。
                    if !is_folder_context && !self.items_are_reading_history_view {
                        if self.render_folder_pin_menu_entry(ui, &item) {
                            close = true;
                        }
                    }
                    if self.items_are_reading_history_view && !is_folder_context {
                        ui.separator();
                        if ui.button("履歴から削除").clicked() {
                            self.remove_reading_history_entry_for_idx(idx);
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
            self.cached_handlers = None;
        }

        nav
    }

    pub(crate) fn copy_item_path_to_clipboard(&mut self, ctx: &egui::Context, idx: usize) -> bool {
        let Some(item) = self.items.get(idx).cloned() else {
            return false;
        };
        let text = match item {
            GridItem::Image(path)
            | GridItem::Video(path)
            | GridItem::Audio(path)
            | GridItem::Folder(path)
            | GridItem::ZipFile(path)
            | GridItem::PdfFile(path) => native_path_text(&path),
            GridItem::ConvertibleArchive { path, .. } | GridItem::SearchContainer { path, .. } => {
                native_path_text(&path)
            }
            GridItem::ZipImage {
                zip_path,
                entry_name,
            }
            | GridItem::ZipDir {
                zip_path,
                dir_prefix: entry_name,
                ..
            } => format!("{}:{}", native_path_text(&zip_path), entry_name),
            GridItem::PdfPage {
                pdf_path, page_num, ..
            } => format!("{}:Page {}", native_path_text(&pdf_path), page_num + 1),
            // ファイル名スタック: 代表画像の実パスをコピー。
            GridItem::Stack { representative, .. } => native_path_text(&representative),
            GridItem::ZipSeparator { .. } => return false,
        };
        ctx.copy_text(text);
        true
    }

    pub(crate) fn copy_item_file_name_to_clipboard(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
    ) -> bool {
        let Some(item) = self.items.get(idx) else {
            return false;
        };
        let name = item.name().to_string();
        if name.is_empty() {
            return false;
        }
        ctx.copy_text(name);
        true
    }

    pub(crate) fn copy_item_image_to_clipboard(&mut self, idx: usize) -> bool {
        let Some(item) = self.items.get(idx).cloned() else {
            return false;
        };
        let rotation = self.get_rotation(idx);
        match item {
            GridItem::Image(path) => {
                copy_image_to_clipboard(&path, rotation);
                true
            }
            GridItem::ZipImage {
                zip_path,
                entry_name,
            } => {
                copy_zip_image_to_clipboard(&zip_path, &entry_name, rotation);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn open_item_folder_in_explorer(&mut self, idx: usize) -> bool {
        let Some(item) = self.items.get(idx).cloned() else {
            return false;
        };
        let path = match item {
            GridItem::Image(path)
            | GridItem::Video(path)
            | GridItem::Audio(path)
            | GridItem::Folder(path)
            | GridItem::ZipFile(path)
            | GridItem::PdfFile(path) => path,
            GridItem::ConvertibleArchive { path, .. } | GridItem::SearchContainer { path, .. } => {
                path
            }
            GridItem::ZipImage { zip_path, .. } | GridItem::ZipDir { zip_path, .. } => zip_path,
            GridItem::PdfPage { pdf_path, .. } => pdf_path,
            // ファイル名スタック: 代表画像を含むフォルダを開く。
            GridItem::Stack { representative, .. } => representative,
            GridItem::ZipSeparator { .. } => return false,
        };
        open_folder_in_explorer(&path);
        true
    }

    fn try_show_native_grid_context_menu(
        &mut self,
        ctx: &egui::Context,
        pos: egui::Pos2,
        idx: usize,
        item: GridItem,
        is_folder_context: bool,
        has_checked: bool,
        in_search: bool,
        folder_command_target: Option<PathBuf>,
    ) -> NativeGridContextMenuOutcome {
        if !self.settings.use_native_shell_context_menu {
            return NativeGridContextMenuOutcome::Fallback;
        }
        let Some(hwnd) = self.main_hwnd else {
            return NativeGridContextMenuOutcome::Fallback;
        };
        let prepare_t0 = std::time::Instant::now();
        let Some(target) = self.native_grid_context_menu_target(
            idx,
            item,
            is_folder_context,
            has_checked,
            folder_command_target,
        ) else {
            return NativeGridContextMenuOutcome::Fallback;
        };
        let miv_items = self.native_grid_context_menu_items(&target, in_search);
        let target_kind = native_grid_context_menu_target_kind(&target);
        let prepare_ms = prepare_t0.elapsed().as_secs_f64() * 1000.0;
        if crate::perf::is_enabled() {
            crate::perf::event(
                "native_menu",
                "app_prepare",
                Some(target_kind),
                self.input_seq,
                &[
                    ("ms", serde_json::Value::from(prepare_ms)),
                    (
                        "path_count",
                        serde_json::Value::from(target.paths.len() as u64),
                    ),
                    ("miv_count", serde_json::Value::from(miv_items.len() as u64)),
                    (
                        "folder_context",
                        serde_json::Value::from(target.is_folder_context),
                    ),
                    ("checked", serde_json::Value::from(target.has_checked)),
                ],
            );
        }
        if prepare_ms >= 80.0 {
            crate::logger::log(format!(
                "native_context_menu: slow app_prepare {prepare_ms:.1}ms target={target_kind} paths={} miv_items={}",
                target.paths.len(),
                miv_items.len()
            ));
        }
        let background_folder = target.is_folder_context.then(|| target.paths[0].clone());
        let request = NativeContextMenuRequest {
            hwnd,
            screen_pos: (pos.x.round() as i32, pos.y.round() as i32),
            background_folder,
            paths: if target.is_folder_context {
                Vec::new()
            } else {
                target.paths.clone()
            },
            miv_items,
        };
        let native_result = crate::native_context_menu::show_native_context_menu(request);
        Self::resync_egui_modifiers_from_os(ctx);
        match native_result {
            NativeContextMenuResult::Canceled | NativeContextMenuResult::ShellCommandInvoked => {
                NativeGridContextMenuOutcome::Consumed(None)
            }
            NativeContextMenuResult::MivCommand(command) => {
                let nav = self.dispatch_native_grid_context_command(ctx, command, &target);
                NativeGridContextMenuOutcome::Consumed(nav)
            }
            NativeContextMenuResult::Fallback { reason } => {
                crate::logger::log(format!(
                    "native_context_menu: fallback to egui menu: {reason}"
                ));
                NativeGridContextMenuOutcome::Fallback
            }
        }
    }

    fn native_grid_context_menu_target(
        &self,
        idx: usize,
        item: GridItem,
        is_folder_context: bool,
        has_checked: bool,
        folder_command_target: Option<PathBuf>,
    ) -> Option<NativeGridContextMenuTarget> {
        let paths = if is_folder_context {
            vec![folder_command_target?]
        } else if has_checked {
            if self.checked.iter().any(|&idx| {
                self.items
                    .get(idx)
                    .and_then(GridItem::file_operation_path)
                    .is_none()
            }) {
                return None;
            }
            self.collect_checked_paths()
        } else {
            vec![item.drag_source_path()?.to_path_buf()]
        };
        if paths.is_empty() {
            return None;
        }
        Some(NativeGridContextMenuTarget {
            paths,
            item,
            item_index: (!is_folder_context).then_some(idx),
            is_folder_context,
            has_checked,
        })
    }

    fn native_grid_context_menu_items(
        &self,
        target: &NativeGridContextMenuTarget,
        in_search: bool,
    ) -> Vec<NativeMivMenuItem> {
        let mut items = Vec::new();
        if target.has_checked {
            items.push(NativeMivMenuItem {
                command: NativeMivCommand::CopyPath,
                label: "選択項目のパスをコピー".to_string(),
            });
            items.push(NativeMivMenuItem {
                command: NativeMivCommand::RotateLeft,
                label: "左に回転 (L)".to_string(),
            });
            items.push(NativeMivMenuItem {
                command: NativeMivCommand::RotateRight,
                label: "右に回転 (R)".to_string(),
            });
            return items;
        }

        if target.is_folder_context
            && target.paths.first().is_some_and(|path| {
                self.current_favorite_target()
                    .as_ref()
                    .is_some_and(|folder| crate::folder_tree::path_eq(folder, path))
            })
        {
            items.push(NativeMivMenuItem {
                command: NativeMivCommand::NewFolder,
                label: "新しいフォルダ...".to_string(),
            });
            items.push(NativeMivMenuItem {
                command: NativeMivCommand::Paste,
                label: "貼り付け".to_string(),
            });
        }

        if !target.is_folder_context && target.paths.len() == 1 {
            items.push(NativeMivMenuItem {
                command: NativeMivCommand::Rename,
                label: "名前の変更...".to_string(),
            });
        }

        let copy_path_label = if target.is_folder_context {
            "このフォルダのパスをコピー"
        } else {
            "パスをコピー"
        };
        items.push(NativeMivMenuItem {
            command: NativeMivCommand::CopyPath,
            label: copy_path_label.to_string(),
        });
        if !target.is_folder_context
            && target
                .paths
                .first()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .is_some_and(|name| !name.is_empty())
        {
            items.push(NativeMivMenuItem {
                command: NativeMivCommand::CopyFileName,
                label: "ファイル名をコピー".to_string(),
            });
        }
        if matches!(target.item, GridItem::Image(_)) {
            items.push(NativeMivMenuItem {
                command: NativeMivCommand::CopyImageToClipboard,
                label: "画像をクリップボードにコピー".to_string(),
            });
        }
        if !target.is_folder_context
            && !target.has_checked
            && matches!(
                target.item,
                GridItem::ZipFile(_) | GridItem::PdfFile(_) | GridItem::ConvertibleArchive { .. }
            )
        {
            items.push(NativeMivMenuItem {
                command: NativeMivCommand::OpenContainerAsPage,
                label: "ページを開く".to_string(),
            });
            items.push(NativeMivMenuItem {
                command: NativeMivCommand::OpenContainerAsList,
                label: "一覧を開く".to_string(),
            });
        }
        if in_search && !target.is_folder_context {
            items.push(NativeMivMenuItem {
                command: NativeMivCommand::JumpToFolder,
                label: "フォルダに移動".to_string(),
            });
        }
        if matches!(target.item, GridItem::Image(_) | GridItem::Video(_)) {
            items.push(NativeMivMenuItem {
                command: NativeMivCommand::RotateLeft,
                label: "左に回転 (L)".to_string(),
            });
            items.push(NativeMivMenuItem {
                command: NativeMivCommand::RotateRight,
                label: "右に回転 (R)".to_string(),
            });
        }
        if let Some(label) = self.native_folder_pin_context_label(target) {
            items.push(NativeMivMenuItem {
                command: NativeMivCommand::ToggleRepresentativeThumb,
                label,
            });
        }
        items
    }

    fn native_folder_pin_context_label(
        &self,
        target: &NativeGridContextMenuTarget,
    ) -> Option<String> {
        if target.is_folder_context
            || target.item_index.is_none()
            || self.items_are_global_search_view
            || self.items_are_tag_view
            || self.items_are_reading_history_view
            || self.items_are_rating_view
            || self.archive_source_override.is_some() && self.zip_nav.is_none()
        {
            return None;
        }
        let container = self.current_folder.as_ref()?;
        if is_synthetic_view_path(container) {
            return None;
        }
        let source = crate::folder_thumb_pins::source_from_grid_item(container, &target.item)?;
        let existing = self.folder_thumb_pin_for(container);
        let label = if existing == Some(&source) {
            "📌 代表サムネ固定を解除"
        } else {
            "📌 代表サムネに固定"
        };
        Some(label.to_string())
    }

    fn dispatch_native_grid_context_command(
        &mut self,
        ctx: &egui::Context,
        command: NativeMivCommand,
        target: &NativeGridContextMenuTarget,
    ) -> Option<ContextMenuAction> {
        match command {
            NativeMivCommand::NewFolder => {
                if target.is_folder_context
                    && let Some(folder) = target.paths.first().cloned()
                {
                    self.request_new_folder_dialog(folder);
                }
                None
            }
            NativeMivCommand::Paste => {
                if target.is_folder_context
                    && let (Some(hwnd), Some(folder)) =
                        (self.main_hwnd, target.paths.first().cloned())
                {
                    let result = crate::native_context_menu::invoke_shell_folder_background_verb(
                        hwnd,
                        &folder,
                        crate::native_context_menu::ShellClipboardVerb::Paste,
                    );
                    Self::resync_egui_modifiers_from_os(ctx);
                    if let Err(err) = result {
                        crate::logger::log(format!("native_context_menu: mIV Paste failed: {err}"));
                    }
                }
                None
            }
            NativeMivCommand::Rename => {
                if !target.is_folder_context
                    && target.paths.len() == 1
                    && let Some(path) = target.paths.first().cloned()
                {
                    self.request_rename_dialog(path);
                }
                None
            }
            NativeMivCommand::CopyPath => {
                let text = target
                    .paths
                    .iter()
                    .map(|p| native_path_text(p))
                    .collect::<Vec<_>>()
                    .join("\n");
                ctx.copy_text(text);
                None
            }
            NativeMivCommand::CopyFileName => {
                let name = target
                    .paths
                    .first()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                ctx.copy_text(name);
                None
            }
            NativeMivCommand::CopyImageToClipboard => {
                if let GridItem::Image(path) = &target.item {
                    let rotation = target
                        .item_index
                        .map(|idx| self.get_rotation(idx))
                        .unwrap_or(crate::rotation_db::Rotation::None);
                    copy_image_to_clipboard(path, rotation);
                }
                None
            }
            NativeMivCommand::JumpToFolder => match &target.item {
                GridItem::Folder(path) => {
                    Some(ContextMenuAction::JumpFromSearch(native_nav_path(path)))
                }
                _ => target
                    .paths
                    .first()
                    .and_then(|path| parent_folder_for_nav(path))
                    .map(ContextMenuAction::JumpFromSearch),
            },
            NativeMivCommand::OpenContainerAsPage => {
                target
                    .item_index
                    .map(|idx| ContextMenuAction::OpenGridContainer {
                        idx,
                        mode: crate::app::GridContainerOpenMode::PageFullscreen,
                    })
            }
            NativeMivCommand::OpenContainerAsList => {
                target
                    .item_index
                    .map(|idx| ContextMenuAction::OpenGridContainer {
                        idx,
                        mode: crate::app::GridContainerOpenMode::PageList,
                    })
            }
            NativeMivCommand::RotateLeft => {
                if target.has_checked {
                    for idx in self.checked.clone() {
                        self.rotate_image_ccw(idx);
                    }
                } else if let Some(idx) = target.item_index {
                    self.rotate_image_ccw(idx);
                }
                None
            }
            NativeMivCommand::RotateRight => {
                if target.has_checked {
                    for idx in self.checked.clone() {
                        self.rotate_image_cw(idx);
                    }
                } else if let Some(idx) = target.item_index {
                    self.rotate_image_cw(idx);
                }
                None
            }
            NativeMivCommand::ToggleRepresentativeThumb => {
                if let Some(idx) = target.item_index {
                    self.toggle_folder_pin_for_idx(idx);
                }
                None
            }
        }
    }

    /// `ContextMenuAction::JumpFromSearch` の副作用を適用する。検索終了 (Ctrl+G /
    /// Ctrl+S) と、検索前フォルダを back stack に積んだうえで履歴の二重 push を抑止する。
    ///
    /// **呼び出しは context_nav が優先度判定で実際に勝ったあとに限る** (Codex P3): 副作用を
    /// show_context_menu 内で発火すると、同フレームに別 nav 源 (キーボード等) が勝った
    /// ときに、別ナビが意図せず検索終了済み・suppress 立て済みの状態を引き継いでしまう。
    pub(crate) fn apply_jump_from_search_to(&mut self, target: &Path) {
        // 検索を抜ける前に、検索開始時の実フォルダ C を捕捉する。検索中は
        // current_folder がドリルイン先や合成パスを指しうるので、確実に検索前の
        // 実フォルダを保持している saved_folder から取る。
        let pre_search_folder = if self.global_search.active {
            self.global_search.saved_folder.clone()
        } else if self.favsearch.active {
            self.favsearch.saved_folder.clone()
        } else if self.tag_view.active {
            self.tag_view.saved_folder.clone()
        } else {
            None
        };
        // saved_folder の復帰で旧フォルダへ無駄なロードが走らないよう、先に
        // saved_folder を捨てる (toolbar_fav_nav と同じ手順)。
        if self.global_search.active {
            self.global_search.saved_folder = None;
            self.close_global_search();
        }
        if self.favsearch.active {
            self.favsearch.saved_folder = None;
            self.close_favsearch();
        }
        if self.tag_view.active {
            self.tag_view.saved_folder = None;
            self.close_tag_view();
        }
        // 「フォルダに移動」は検索を明示終了して実フォルダへ着地する正当な
        // ナビゲーションなので「検索前フォルダ C → 移動先 X」を履歴に残す。
        // X で ← を押すと検索前の C に戻れる。C == X のときは無意味なので積まない。
        if let Some(c) = pre_search_folder {
            if !crate::folder_tree::path_eq(&c, target) {
                self.push_nav_history_entry(c);
            }
        }
        // 直後に呼び出し元が行う load_folder(X) では back_stack への二重 push を
        // 避ける (移動元 C は上で明示的に積み済み。X の recent 追加は
        // record_folder_nav_transition 側で行われる)。
        self.set_active_folder_nav_suppress_record_once(true);
    }

    /// フルスクリーン表示中のコンテキストメニューを表示する。
    /// 移動なし右クリックでトリガーされる。
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

        match self.try_show_native_grid_context_menu(
            ctx,
            pos,
            idx,
            item.clone(),
            false,
            false,
            false,
            None,
        ) {
            NativeGridContextMenuOutcome::Consumed(_) => {
                self.fs_context_menu_idx = None;
                self.cached_handlers = None;
                ctx.request_repaint();
                return false;
            }
            NativeGridContextMenuOutcome::Fallback => {}
        }

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
                    GridItem::Image(p) | GridItem::Video(p) | GridItem::Audio(p) => {
                        if ui.button("パスをコピー").clicked() {
                            copy_path_text(ctx, p);
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
                                let rotation = self.get_rotation(idx);
                                copy_image_to_clipboard(p, rotation);
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
                        self.draw_legacy_xmp_context_entries(ui, vec![p.clone()], &mut close);
                    }
                    GridItem::ZipFile(p) | GridItem::PdfFile(p) => {
                        if ui.button("パスをコピー").clicked() {
                            copy_path_text(ctx, p);
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
                        let display = format!("{}:{}", native_path_text(zip_path), entry_name);
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
                            let rotation = self.get_rotation(idx);
                            copy_zip_image_to_clipboard(zip_path, entry_name, rotation);
                            close = true;
                        }
                    }
                    GridItem::PdfPage {
                        pdf_path, page_num, ..
                    } => {
                        let display =
                            format!("{}:Page {}", native_path_text(pdf_path), page_num + 1);
                        if ui.button("パスをコピー").clicked() {
                            ctx.copy_text(display);
                            close = true;
                        }
                        if ui.button("ページ名をコピー").clicked() {
                            ctx.copy_text(format!("Page {}", page_num + 1));
                            close = true;
                        }
                    }
                    // ZipDir / Stack はフルスクリーン対象外 (仮想ナビコンテナ) なので FS では
                    // 最小限 (そもそも FS の items にはメンバーの実 Image しか入らない)。
                    GridItem::Folder(_)
                    | GridItem::ZipSeparator { .. }
                    | GridItem::ZipDir { .. }
                    | GridItem::Stack { .. } => {
                        close = true;
                    }
                    GridItem::ConvertibleArchive { path, .. } => {
                        if ui.button("パスをコピー").clicked() {
                            copy_path_text(ctx, path);
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
                            copy_path_text(ctx, path);
                            close = true;
                        }
                        if ui.button("フォルダを開く").clicked() {
                            open_folder_in_explorer(path);
                            close = true;
                        }
                    }
                }

                // ── 代表サムネ固定 (pin) エントリ (separator 込み) ──
                // 条件分岐とそれに伴う separator 描画は helper 側に集約。
                if self.render_folder_pin_menu_entry(ui, &item) {
                    close = true;
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

    fn draw_legacy_xmp_context_entries(
        &mut self,
        ui: &mut egui::Ui,
        mut paths: Vec<PathBuf>,
        close: &mut bool,
    ) {
        paths.retain(|path| {
            crate::xmp_writer::is_writable_format(path)
                || crate::xmp_writer::is_video_for_sidecar(path)
        });
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            return;
        }
        let count = paths.len();
        ui.separator();
        if ui
            .button(format!("旧XMPタグを取り込む ({count})"))
            .on_hover_text("ファイル内に残っている旧mIVの #タグをアプリ内タグへ取り込みます。")
            .clicked()
        {
            self.request_legacy_xmp_import_for_paths(
                paths.clone(),
                crate::tag_legacy_xmp_worker::LegacyXmpImportMode::ImportOnly,
            );
            *close = true;
        }
        if ui
            .button(format!("旧XMPタグを取り込んでファイルから削除 ({count})"))
            .on_hover_text("取り込み後、ファイル内の旧mIV #タグだけを削除します。")
            .clicked()
        {
            self.request_legacy_xmp_import_for_paths(
                paths,
                crate::tag_legacy_xmp_worker::LegacyXmpImportMode::ImportAndRemove,
            );
            *close = true;
        }
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
            if let Some(path) = self.items.get(idx).and_then(GridItem::file_operation_path) {
                paths.push(path.to_path_buf());
            }
        }
        paths
    }

    /// チェック済みアイテムの削除対象 (idx, path) を収集する (降順ソート)。
    ///
    /// 通常の UI ではフォルダをチェックできないが、削除は単一選択フォルダも対象にするため、
    /// ここも実ファイル / 実フォルダを返す `drag_source_path` に揃える。
    pub(crate) fn collect_checked_indexed_paths(&self) -> Vec<(usize, PathBuf)> {
        let mut targets: Vec<(usize, PathBuf)> = Vec::new();
        for &idx in &self.checked {
            if let Some(path) = self.items.get(idx).and_then(GridItem::drag_source_path) {
                targets.push((idx, path.to_path_buf()));
            }
        }
        // 降順ソート (削除時にインデックスがずれないよう後ろから削除)
        targets.sort_by(|a, b| b.0.cmp(&a.0));
        targets
    }

    pub(crate) fn request_delete_confirm(&mut self, targets: Vec<(usize, PathBuf)>) {
        self.delete_targets = targets;
        self.delete_confirm_label = None;
        self.show_delete_confirm = true;
    }

    /// 削除確認ダイアログを表示する。
    pub(crate) fn show_delete_confirm_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_delete_confirm {
            return;
        }

        if self.delete_targets.is_empty() {
            self.show_delete_confirm = false;
            self.delete_confirm_label = None;
            return;
        }

        if self.delete_confirm_label.is_none() {
            self.delete_confirm_label =
                Some(delete_confirm_label_for_targets(&self.delete_targets));
        }
        let label = self.delete_confirm_label.clone().unwrap_or_default();

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
                        self.delete_confirm_label = None;
                    }
                });
            });

        if !open {
            self.show_delete_confirm = false;
            self.delete_targets.clear();
            self.delete_confirm_label = None;
        }

        if do_start_delete {
            // 削除確認は閉じ、path だけを worker に渡す (idx は完了時に再解決)。
            let paths: Vec<std::path::PathBuf> =
                self.delete_targets.iter().map(|(_, p)| p.clone()).collect();
            self.show_delete_confirm = false;
            self.delete_targets.clear();
            self.delete_confirm_label = None;
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

    /// DEL キーで選択中またはチェック済みの実ファイル / 実フォルダを削除するハンドラ。
    pub(crate) fn handle_delete_key(&mut self, ctx: &egui::Context) {
        // detached viewer 中はグリッドが操作可能なので Delete も生かす
        // (`fullscreen_idx.is_some()` の旧述語だと detached 窓を開いている間
        // グリッドの Delete キーが無効化される)。GridDelete は Grid コンテキストの
        // KeyAction なので、detached 窓側のキー入力がここへ流れることはない。
        if self.viewer_session_blocks_main_window()
            || self.address_has_focus
            || self.any_dialog_open()
        {
            return;
        }
        let del = self
            .keymap
            .pressed_action(ctx, crate::keymap::KeyAction::GridDelete);
        if !del {
            return;
        }

        if !self.checked.is_empty() {
            // チェック済みがある → まとめて削除
            let targets = self.collect_checked_indexed_paths();
            self.request_delete_confirm(targets);
        } else if let Some(idx) = self.selected {
            // 単一選択
            let Some(path) = self
                .items
                .get(idx)
                .and_then(GridItem::drag_source_path)
                .map(|p| p.to_path_buf())
            else {
                return;
            };
            self.request_delete_confirm(vec![(idx, path)]);
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
/// 上書きするのを防ぐ (画像 decode / Shell ファイルコピーの先行予約が対象)。
static CLIPBOARD_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 画像クリップボード書き込み (OpenClipboard/SetClipboardData) を直列化するミューテックス。
/// seq チェックは必ずこの lock 内で実行し、
/// 「チェック通過 → 実際の書き込み」の間に別の writer が割り込まないようにする。
/// 古い writer が遅れて clipboard を上書きする race を閉じる。
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

/// Path を PowerShell の単一引用符文字列リテラル (`'...'`、内部の `'` を `''` へ
/// エスケープ) に変換する。外部 D&D 受け取りのスクリプト生成で使う。
#[cfg(windows)]
fn ps_quote(path: &std::path::Path) -> String {
    format!("'{}'", native_path_text(path).replace('\'', "''"))
}

/// 画像ファイルの内容をクリップボードにコピーする (Windows)。
/// 画像ファイルをデコードしてクリップボードにコピーする。
/// image クレートで非対応の形式は WIC にフォールバック。
///
/// decode + DIB 構築は worker スレッドで行う。20MP 超や巨大 RAW では
/// 数百ms〜秒単位かかるため、UI スレッドから同期実行すると右クリック操作で固まる。
/// 発行時に `CLIPBOARD_SEQ` を bump し、set 直前に最新 seq と比較、自分が古ければ
/// set をスキップする — 遅い A が速い B を追い越して上書きするのを防ぐ。
fn copy_image_to_clipboard(path: &std::path::Path, rotation: crate::rotation_db::Rotation) {
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
                        match crate::wic_decoder::decode_to_dynamic_image(&path)
                            .or_else(|| crate::susie_loader::decode_file(&path, true, None).ok())
                        {
                            Some(i) => i,
                            None => return,
                        }
                    }
                    #[cfg(not(windows))]
                    {
                        match crate::susie_loader::decode_file(&path, true, None) {
                            Ok(i) => i,
                            Err(_) => return,
                        }
                    }
                }
            };
            let img = crate::thumb_loader::apply_exif_orientation(img, &path);
            let img = crate::capture::rotate_dynamic_image(img, rotation);
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
fn copy_zip_image_to_clipboard(
    zip_path: &std::path::Path,
    entry_name: &str,
    rotation: crate::rotation_db::Rotation,
) {
    let zip_path = zip_path.to_path_buf();
    let entry_name = entry_name.to_string();
    let my_seq = bump_clipboard_seq();
    std::thread::Builder::new()
        .name("clipboard-zip-image-copy".into())
        .spawn(move || {
            let Ok(bytes) = crate::zip_loader::read_entry_bytes(&zip_path, &entry_name) else {
                return;
            };
            let Some(img) = image::load_from_memory(&bytes)
                .ok()
                .or_else(|| crate::wic_decoder::decode_to_dynamic_image_from_bytes(&bytes))
                .or_else(|| {
                    crate::susie_loader::decode_bytes(&entry_name, &bytes, true, None).ok()
                })
            else {
                return;
            };
            let img = crate::thumb_loader::apply_exif_orientation_from_bytes(img, &bytes);
            let img = crate::capture::rotate_dynamic_image(img, rotation);
            // pre-check は best-effort 短絡 (DIB 構築省略)。正式な stale 判定は
            // `set_image_to_clipboard` 側の mutex 内で行う。
            if !clipboard_seq_is_latest(my_seq) {
                return;
            }
            set_image_to_clipboard(&img, my_seq);
        })
        .ok();
}

/// 遅い decode が後続の clipboard 操作を上書きしないよう、呼び出し開始時点で
/// clipboard 書き込み世代を予約する。
pub fn reserve_clipboard_write_sequence() -> u64 {
    bump_clipboard_seq()
}

/// 予約済み clipboard 世代で RGBA8 画像をコピーする。
///
/// 動画フレームのスクリーンショットなど、呼び出し側がピクセルを持っている場合に使う。
/// DIB 構築と OS clipboard I/O は画像ファイルコピーと同じく worker thread で行い、
/// UI スレッドを止めない。
pub fn copy_rgba_image_to_clipboard_async_seq(width: u32, height: u32, rgba: Vec<u8>, my_seq: u64) {
    if width == 0 || height == 0 || rgba.len() != width as usize * height as usize * 4 {
        return;
    }
    std::thread::Builder::new()
        .name("clipboard-rgba-copy".into())
        .spawn(move || {
            if !clipboard_seq_is_latest(my_seq) {
                return;
            }
            set_rgba_to_clipboard(width, height, &rgba, my_seq);
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
    set_rgba_to_clipboard(width, height, rgba.as_raw(), my_seq);
}

/// RGBA8 pixels をクリップボードに CF_DIB として設定する。
fn set_rgba_to_clipboard(width: u32, height: u32, rgba: &[u8], my_seq: u64) {
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
        for y in 0..height {
            let src_row = (height - 1 - y) as usize;
            let dst_offset = header_size as usize + (y * row_size) as usize;
            for x in 0..width {
                let src_idx = (src_row * width as usize + x as usize) * 4;
                let dst_idx = dst_offset + (x * 3) as usize;
                buf[dst_idx] = rgba[src_idx + 2];
                buf[dst_idx + 1] = rgba[src_idx + 1];
                buf[dst_idx + 2] = rgba[src_idx];
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
        let _ = (width, height, rgba, my_seq);
    }
}

/// `copy_paths_into_folder` の完了結果。`failed > 0` のとき呼び出し側はトースト等で
/// ユーザーに通知すべき。エラー詳細は `first_errors` (先頭最大 5 件) を見る。
///
/// `notice` は「失敗ではないが事後通知が要る」局面で使う (Codex P2-2 対応)。
/// 例: 自己再帰除外で全件落ちて実コピー自体が走らなかったケース。
#[derive(Debug, Default, Clone)]
pub struct CopyOutcome {
    pub attempted: usize,
    pub failed: usize,
    pub first_errors: Vec<String>,
    pub notice: Option<String>,
}

/// 指定パス群を `dest_folder` へコピーする（エクスプローラ → mIV のドロップ受け取り用）。
///
/// PowerShell worker で実行し、完了を `rx` で 1 回通知する。
///
/// **フォルダは v1.1.0 で一旦無効化したため、防御層としてここでもディレクトリは skip し
/// ファイルのみコピーする** (呼び出し側 `handle_external_file_drop` も folder を除外済みだが、
/// 将来の呼び出し追加で再帰コピーが復活しないよう二重化)。`-Recurse` は付けない (ファイル
/// 専用)。コピー先に同名ファイルが既存なら上書き（`-Force`）。
///
/// **review #15 対応**: 旧実装は `-ErrorAction SilentlyContinue` で全エラーを握りつぶし、
/// `()` 完了通知だけを返していた。Locked file / 権限拒否 / disk full 等の per-file 失敗が
/// UI から完全に見えない問題があった。本実装は try/catch で失敗カウントとメッセージを
/// stdout に書き出し、worker 側で parse して `CopyOutcome` で返す。
pub fn copy_paths_into_folder(
    paths: Vec<PathBuf>,
    dest_folder: &std::path::Path,
) -> mpsc::Receiver<CopyOutcome> {
    let (tx, rx) = mpsc::channel();
    #[cfg(windows)]
    {
        if paths.is_empty() {
            let _ = tx.send(CopyOutcome::default());
            return rx;
        }
        let attempted = paths.len();
        let dest = ps_quote(dest_folder);
        let list = paths
            .iter()
            .map(|p| ps_quote(p))
            .collect::<Vec<_>>()
            .join(",");
        // 各 Copy-Item を try/catch で囲んで失敗を数える。エラー詳細は先頭 5 件まで
        // ::ERR:: マーカー付きで stdout に流す。スクリプト全体としては常に exit 0
        // (= worker の `cmd.output()` 成功) を保つ。
        let script = format!(
            "$dest = {dest}\n\
             $failed = 0\n\
             $errs = New-Object System.Collections.ArrayList\n\
             foreach ($f in @({list})) {{\n\
            \x20 if ([System.IO.Directory]::Exists($f)) {{ continue }}\n\
            \x20 try {{\n\
            \x20   Copy-Item -LiteralPath $f -Destination $dest -Force -ErrorAction Stop\n\
            \x20 }} catch {{\n\
            \x20   $failed++\n\
            \x20   if ($errs.Count -lt 5) {{ [void]$errs.Add(\"$($_.Exception.Message): $f\") }}\n\
            \x20 }}\n\
             }}\n\
             Write-Output \"::FAILED::$failed\"\n\
             foreach ($e in $errs) {{ Write-Output \"::ERR::$e\" }}\n"
        );
        run_ps_script_with_outcome(script, tx, attempted);
    }
    #[cfg(not(windows))]
    {
        let _ = (paths, dest_folder);
        let _ = tx; // drop — receiver will get Disconnected
    }
    rx
}

impl CopyOutcome {
    /// 「全件失敗 + 原因 1 件」の outcome を作るヘルパー。spawn 失敗 / PowerShell 起動失敗 /
    /// 非ゼロ終了 / `::FAILED::` マーカー欠落 等で worker が結果を確定できないときに使う
    /// (Codex P2-1 対応: 旧実装はこれらを全て `failed=0` の成功扱いに潰していた)。
    pub fn all_failed(attempted: usize, reason: impl Into<String>) -> Self {
        Self {
            attempted,
            failed: attempted,
            first_errors: vec![reason.into()],
            notice: None,
        }
    }
}

/// stdout を parse して `CopyOutcome` を作って送る `run_ps_script_async` の変種。
/// `attempted` は paths.len() を呼び出し側が知っているのでそれを使う。
///
/// **失敗ハンドリング (Codex P2-1 対応)**: 失敗ポイントを明示的に列挙して `failed=attempted`
/// で報告する。
///   - thread::Builder::spawn 失敗 → ここで即 `all_failed` 送出。
///   - tmp ps1 書き込み失敗 → `all_failed`。
///   - powershell 起動失敗 (`cmd.output()` Err) → `all_failed`。
///   - 非ゼロ exit code → `all_failed` (stderr 先頭 3 行も付ける)。
///   - `::FAILED::` マーカー欠落 → スクリプトが途中でクラッシュした可能性。`all_failed`。
#[cfg(windows)]
fn run_ps_script_with_outcome(
    script: String,
    on_done: mpsc::Sender<CopyOutcome>,
    attempted: usize,
) {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("miv_ps_{}_{}.ps1", std::process::id(), seq));
    // spawn が失敗したときに後段の `all_failed` 送出で `on_done` を再利用するため
    // closure には clone を渡す (`mpsc::Sender` は Clone 可能、各 clone は独立した
    // ハンドル)。
    let tx_for_worker = on_done.clone();
    let spawn_result = std::thread::Builder::new()
        .name("powershell-copy-with-outcome".into())
        .spawn(move || {
            let outcome = execute_copy_script(&tmp, &script, attempted);
            let _ = std::fs::remove_file(&tmp);
            let _ = tx_for_worker.send(outcome);
        });
    if let Err(e) = spawn_result {
        crate::logger::log(format!(
            "run_ps_script_with_outcome: thread spawn failed: {e}"
        ));
        let _ = on_done.send(CopyOutcome::all_failed(
            attempted,
            format!("worker thread spawn failed: {e}"),
        ));
    }
}

#[cfg(windows)]
fn execute_copy_script(tmp: &std::path::Path, script: &str, attempted: usize) -> CopyOutcome {
    let mut content = vec![0xEF, 0xBB, 0xBF];
    content.extend_from_slice(script.as_bytes());
    if let Err(e) = std::fs::write(tmp, &content) {
        return CopyOutcome::all_failed(attempted, format!("script file write failed: {e}"));
    }
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
    let out = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            return CopyOutcome::all_failed(attempted, format!("powershell execution failed: {e}"));
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut parsed_failed: Option<usize> = None;
    let mut first_errors: Vec<String> = Vec::new();
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("::FAILED::") {
            if let Ok(n) = rest.trim().parse::<usize>() {
                parsed_failed = Some(n);
            }
        } else if let Some(rest) = line.strip_prefix("::ERR::") {
            first_errors.push(rest.to_string());
        }
    }
    // 非ゼロ exit code: スクリプトが完走しなかった可能性が高い → 全件失敗扱い。
    if !out.status.success() {
        let mut errs = vec![format!("powershell exit code {:?}", out.status.code())];
        for line in stderr.lines().take(3) {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                errs.push(trimmed.to_string());
            }
        }
        errs.extend(first_errors.into_iter());
        return CopyOutcome {
            attempted,
            failed: attempted,
            first_errors: errs.into_iter().take(5).collect(),
            notice: None,
        };
    }
    // exit=0 だが ::FAILED:: マーカーが無い: スクリプトが try/catch ループの外でエラー終了。
    // (例: 構文/解析エラー、外部 cmdlet が見つからない、CLR が落ちる等)
    let Some(failed) = parsed_failed else {
        let mut errs = vec![
            "powershell did not emit ::FAILED:: marker (script crashed before completion)"
                .to_string(),
        ];
        for line in stderr.lines().take(3) {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                errs.push(trimmed.to_string());
            }
        }
        return CopyOutcome {
            attempted,
            failed: attempted,
            first_errors: errs.into_iter().take(5).collect(),
            notice: None,
        };
    };
    CopyOutcome {
        attempted,
        failed,
        first_errors,
        notice: None,
    }
}

/// ファイルの親フォルダをエクスプローラで開き、ファイルを選択する。
/// 検索結果由来のパスは正規化形 (小文字・スラッシュ区切り) なので、グリッド側の
/// 通常パスと揃うよう区切り文字をバックスラッシュに変換する。
fn native_nav_path(path: &std::path::Path) -> PathBuf {
    PathBuf::from(native_path_text(path))
}

/// 検索結果アイテムのパスから、ネイティブ形式 (バックスラッシュ区切り) の親フォルダを返す。
fn parent_folder_for_nav(path: &std::path::Path) -> Option<PathBuf> {
    Some(native_nav_path(path.parent()?))
}

fn open_folder_in_explorer(path: &std::path::Path) {
    #[cfg(windows)]
    {
        // `explorer /select,` は区切り文字にバックスラッシュを要求する。検索結果由来の
        // パスは正規化形 (スラッシュ区切り) のことがあり、その場合 explorer がパスを
        // 解決できず既定フォルダ (OneDrive 等) を開いてしまう。
        let native = native_path_text(path);
        let _ = std::process::Command::new("explorer")
            .arg("/select,")
            .arg(&native)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod delete_confirm_tests {
    use super::*;

    #[test]
    fn delete_confirm_label_keeps_recycle_bin_wording_for_normal_targets() {
        let label =
            build_delete_confirm_label(1, Some("sample.jpg"), DeleteConfirmKind::RecycleBin);
        assert_eq!(label, "「sample.jpg」をゴミ箱に移動しますか？");

        let label = build_delete_confirm_label(3, None, DeleteConfirmKind::RecycleBin);
        assert_eq!(label, "3 件の項目をゴミ箱に移動しますか？");
    }

    #[test]
    fn delete_confirm_label_warns_when_delete_may_be_permanent() {
        let label =
            build_delete_confirm_label(1, Some("sample.jpg"), DeleteConfirmKind::MayPermanent);
        assert!(
            label.contains("完全に削除される場合があります"),
            "single-target warning should mention permanent deletion: {label}"
        );
        assert!(
            label.contains("sample.jpg"),
            "single-target warning should include the file name: {label}"
        );

        let label = build_delete_confirm_label(2, None, DeleteConfirmKind::MayPermanent);
        assert!(
            label.contains("ゴミ箱に移動できない場所"),
            "multi-target warning should mention non-recyclable locations: {label}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_root_for_file_operation_extracts_drive_and_unc_roots() {
        assert_eq!(
            windows_path_root_for_file_operation(Path::new(r"j:\folder\sample.jpg")),
            Some("J:\\".to_owned())
        );
        assert_eq!(
            windows_path_root_for_file_operation(Path::new(r"\\server\share\dir\sample.jpg")),
            Some(r"\\server\share\".to_owned())
        );
    }

    #[cfg(windows)]
    #[test]
    fn native_path_text_converts_normalized_search_paths() {
        assert_eq!(
            native_path_text(Path::new(r"g:/home/comfyui/eagle/a.png")),
            r"g:\home\comfyui\eagle\a.png"
        );
        assert_eq!(
            native_path_text(Path::new(r"//server/share/folder/a.jpg")),
            r"\\server\share\folder\a.jpg"
        );
    }

    #[cfg(windows)]
    #[test]
    fn ps_quote_uses_native_separators_and_escapes_quotes() {
        assert_eq!(
            ps_quote(Path::new(r"g:/home/O'Brien/a.png")),
            r"'g:\home\O''Brien\a.png'"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_type_marks_removable_and_remote_as_permanent_risk() {
        assert!(windows_drive_type_may_permanently_delete(2));
        assert!(windows_drive_type_may_permanently_delete(4));
        assert!(!windows_drive_type_may_permanently_delete(3));
    }
}
