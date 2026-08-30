//! サムネイルグリッドの右クリックコンテキストメニュー。

use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use crate::app::is_synthetic_view_path;
use crate::context_menu_model::{
    AssociatedAppMenuEntry, ContextMenuActionState, ContextMenuInput, ContextMenuItemKind,
    ContextMenuSurface, ContextMenuViewFlags, ExternalToolMenuEntry, MenuCommand, MenuNode,
};
use crate::grid_item::GridItem;
use crate::native_context_menu::{NativeContextMenuRequest, NativeContextMenuResult};

#[cfg(windows)]
fn primary_mouse_button_physically_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};

    // Mouse state is outside the keyboard-only synthetic timeline.
    unsafe { (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0 }
}

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

impl DeleteConfirmKind {
    fn aggregate(kinds: impl IntoIterator<Item = Self>) -> Self {
        if kinds.into_iter().any(|kind| kind == Self::MayPermanent) {
            Self::MayPermanent
        } else {
            Self::RecycleBin
        }
    }

    fn initial_selection(self) -> DeleteConfirmSelection {
        match self {
            Self::RecycleBin => DeleteConfirmSelection::Delete,
            Self::MayPermanent => DeleteConfirmSelection::Cancel,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeleteConfirmSelection {
    Delete,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeleteConfirmAction {
    None,
    Delete,
    Cancel,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct DeleteConfirmModalResponse {
    delete_clicked: bool,
    cancel_clicked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeleteConfirmContent {
    label: String,
    kind: DeleteConfirmKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DeleteConfirmKeyResponse {
    selection: DeleteConfirmSelection,
    action: DeleteConfirmAction,
}

const DELETE_CONFIRM_VISIBLE_TARGET_LIMIT: usize = 10;
const DELETE_CONFIRM_SELECTION_ID: &str = "delete_confirm_selection";

/// 削除確認をモーダル表示し、背面 UI への pointer 入力を backdrop で遮断する。
///
/// backdrop のクリックは閉じる操作にせず吸収する。破壊的操作の確認結果は、明示的な
/// ボタンまたは `consume_delete_confirm_action` が扱う固定キーだけで決める。
fn show_delete_confirm_modal(
    ctx: &egui::Context,
    label: &str,
    selection: DeleteConfirmSelection,
) -> DeleteConfirmModalResponse {
    let mut response = DeleteConfirmModalResponse::default();
    let content_rect = ctx.content_rect();
    let dialog_width = (content_rect.width() - 32.0).clamp(240.0, 520.0);
    let target_list_height = (content_rect.height() - 180.0).clamp(100.0, 320.0);
    let has_target_list = label.contains("\n\n対象:");
    egui::Modal::new(egui::Id::new("delete_confirm_modal")).show(ctx, |ui| {
        ui.set_min_width(dialog_width.min(320.0));
        ui.set_max_width(dialog_width);
        ui.heading("削除の確認");
        ui.add_space(8.0);
        if has_target_list {
            ui.scope(|ui| {
                ui.spacing_mut().scroll =
                    super::non_overlapping_dialog_scroll_style(ui.spacing().scroll);
                egui::ScrollArea::vertical()
                    .id_salt("delete_confirm_targets")
                    .max_height(target_list_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.add(egui::Label::new(label).wrap());
                    });
            });
        } else {
            ui.add(egui::Label::new(label).wrap());
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let delete_response = ui.button("削除[Y]");
            if selection == DeleteConfirmSelection::Delete {
                delete_response.request_focus();
            }
            if delete_response.clicked() {
                response.delete_clicked = true;
            }
            let cancel_response = ui.button("キャンセル[N]");
            if selection == DeleteConfirmSelection::Cancel {
                cancel_response.request_focus();
            }
            if cancel_response.clicked() {
                response.cancel_clicked = true;
            }
        });
    });
    response
}

fn resolve_delete_confirm_action(
    selection: DeleteConfirmSelection,
    y_pressed: bool,
    n_pressed: bool,
    escape_pressed: bool,
    enter_pressed: bool,
    ime_active: bool,
) -> DeleteConfirmAction {
    if ime_active {
        DeleteConfirmAction::None
    } else if n_pressed || escape_pressed {
        // 削除とキャンセルが同じ frame に来た場合も、安全側のキャンセルを優先する。
        DeleteConfirmAction::Cancel
    } else if y_pressed {
        DeleteConfirmAction::Delete
    } else if enter_pressed {
        match selection {
            DeleteConfirmSelection::Delete => DeleteConfirmAction::Delete,
            DeleteConfirmSelection::Cancel => DeleteConfirmAction::Cancel,
        }
    } else {
        DeleteConfirmAction::None
    }
}

fn move_delete_confirm_selection(
    selection: DeleteConfirmSelection,
    toward_delete: bool,
    toward_cancel: bool,
) -> DeleteConfirmSelection {
    if toward_cancel {
        // 両方向が同じ frame に来た場合も、安全側のキャンセルを優先する。
        DeleteConfirmSelection::Cancel
    } else if toward_delete {
        DeleteConfirmSelection::Delete
    } else {
        selection
    }
}

/// 削除確認専用の固定キーを判定し、同じ frame の後段 keymap へ漏れないよう event を消費する。
/// Escape の意味判定自体は呼び出し側が IME-safe helper で先に済ませる。
fn consume_delete_confirm_action(
    ctx: &egui::Context,
    selection: DeleteConfirmSelection,
    ime_active: bool,
    escape_pressed: bool,
    enter_pressed: bool,
) -> DeleteConfirmKeyResponse {
    let (y_pressed, n_pressed, toward_delete, toward_cancel) = ctx.input_mut(|input| {
        let y_pressed = input.consume_key(egui::Modifiers::NONE, egui::Key::Y);
        let n_pressed = input.consume_key(egui::Modifiers::NONE, egui::Key::N);
        let modifiers = input.modifiers;
        let toward_delete = input.consume_key(modifiers, egui::Key::ArrowLeft)
            | input.consume_key(modifiers, egui::Key::ArrowUp);
        let toward_cancel = input.consume_key(modifiers, egui::Key::ArrowRight)
            | input.consume_key(modifiers, egui::Key::ArrowDown);
        let _ = input.consume_key(modifiers, egui::Key::Escape);
        let _ = input.consume_key(modifiers, egui::Key::Enter);
        (y_pressed, n_pressed, toward_delete, toward_cancel)
    });
    let selection = move_delete_confirm_selection(selection, toward_delete, toward_cancel);
    let action = resolve_delete_confirm_action(
        selection,
        y_pressed,
        n_pressed,
        escape_pressed,
        enter_pressed,
        ime_active,
    );
    DeleteConfirmKeyResponse { selection, action }
}

/// ダイアログ状態を閉じ、Delete のときだけ worker へ渡す path 列を返す。
fn apply_delete_confirm_action(
    action: DeleteConfirmAction,
    show_delete_confirm: &mut bool,
    delete_targets: &mut Vec<(usize, PathBuf)>,
    delete_confirm_label: &mut Option<String>,
) -> Option<Vec<PathBuf>> {
    match action {
        DeleteConfirmAction::None => None,
        DeleteConfirmAction::Delete => {
            let paths = delete_targets
                .iter()
                .map(|(_, path)| path.clone())
                .collect();
            *show_delete_confirm = false;
            delete_targets.clear();
            *delete_confirm_label = None;
            Some(paths)
        }
        DeleteConfirmAction::Cancel => {
            *show_delete_confirm = false;
            delete_targets.clear();
            *delete_confirm_label = None;
            None
        }
    }
}

#[derive(Debug)]
enum NativeGridContextMenuOutcome {
    Consumed {
        nav: Option<ContextMenuAction>,
        close_fullscreen: bool,
    },
    Fallback,
}

fn native_external_tool_closes_fullscreen(surface: ContextMenuSurface, tool_exists: bool) -> bool {
    surface == ContextMenuSurface::Fullscreen && tool_exists
}

#[derive(Clone)]
struct NativeGridContextMenuTarget {
    shell_paths: Option<Vec<PathBuf>>,
    real_paths: Vec<PathBuf>,
    delete_targets: Vec<(usize, PathBuf)>,
    item: GridItem,
    item_index: Option<usize>,
    is_folder_context: bool,
    has_checked: bool,
    surface: ContextMenuSurface,
    explorer_folder: Option<PathBuf>,
    folder_command_target: Option<PathBuf>,
}

impl NativeGridContextMenuTarget {
    fn is_fullscreen_video(&self) -> bool {
        self.surface == ContextMenuSurface::Fullscreen && matches!(self.item, GridItem::Video(_))
    }
}

fn native_grid_context_menu_target_kind(target: &NativeGridContextMenuTarget) -> &'static str {
    if target.is_folder_context {
        "background"
    } else if target.has_checked {
        "checked_paths"
    } else if target.shell_paths.is_none() {
        "virtual_item"
    } else {
        "item_path"
    }
}

fn menu_leaf_count(nodes: &[MenuNode]) -> usize {
    nodes
        .iter()
        .map(|node| match node {
            MenuNode::Item { .. } => 1,
            MenuNode::Submenu { children, .. } => menu_leaf_count(children),
            MenuNode::Separator => 0,
        })
        .sum()
}

fn render_egui_menu_nodes(ui: &mut egui::Ui, nodes: &[MenuNode]) -> Option<MenuCommand> {
    let mut selected = None;
    for node in nodes {
        match node {
            MenuNode::Separator => {
                ui.separator();
            }
            MenuNode::Item {
                command,
                label,
                enabled,
                disabled_reason,
            } => {
                let response = ui.add_enabled(*enabled, egui::Button::new(label));
                let response = if let Some(reason) = disabled_reason {
                    response.on_disabled_hover_text(reason)
                } else {
                    response
                };
                if response.clicked() {
                    selected = Some(command.clone());
                    break;
                }
            }
            MenuNode::Submenu { label, children } => {
                egui::CollapsingHeader::new(label).show(ui, |ui| {
                    if selected.is_none() {
                        selected = render_egui_menu_nodes(ui, children);
                    }
                });
                if selected.is_some() {
                    break;
                }
            }
        }
    }
    selected
}

const DELETE_FOLDER_OMITTED_FILES_WARNING: &str =
    "フォルダの中には一覧に表示していないファイルも含まれます";

fn delete_confirm_label_for_targets(
    targets: &[(usize, PathBuf)],
    items: &[GridItem],
) -> DeleteConfirmContent {
    let kind = delete_confirm_kind_for_targets(targets);
    let count = targets.len();
    let single_name = (count == 1).then(|| {
        targets[0]
            .1
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
    });
    // 対象決定時の GridItem だけを参照する。確認表示のための metadata/read_dir は行わない。
    let includes_folder = targets
        .iter()
        .any(|(idx, _)| matches!(items.get(*idx), Some(GridItem::Folder(_))));
    let mut label = build_delete_confirm_label(count, single_name, kind, includes_folder);
    if count > 1 {
        label.push_str("\n\n対象:");
        for (_, path) in targets.iter().take(DELETE_CONFIRM_VISIBLE_TARGET_LIMIT) {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_else(|| path.as_os_str().to_string_lossy());
            label.push_str("\n・");
            label.push_str(&name);
        }
        let remaining = count.saturating_sub(DELETE_CONFIRM_VISIBLE_TARGET_LIMIT);
        if remaining > 0 {
            label.push_str(&format!("\n・他 {remaining} 件"));
        }
    }
    DeleteConfirmContent { label, kind }
}

fn should_skip_delete_confirmation(
    skip_recycle_bin_delete_confirmation: bool,
    content: &DeleteConfirmContent,
) -> bool {
    skip_recycle_bin_delete_confirmation && content.kind == DeleteConfirmKind::RecycleBin
}

fn build_delete_confirm_label(
    count: usize,
    single_name: Option<&str>,
    kind: DeleteConfirmKind,
    includes_folder: bool,
) -> String {
    let mut label = match (kind, count, single_name) {
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
    };
    if includes_folder {
        label.push_str("\n\n");
        label.push_str(DELETE_FOLDER_OMITTED_FILES_WARNING);
    }
    label
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

fn context_item_path_text(item: &GridItem) -> String {
    match item {
        GridItem::Folder(path)
        | GridItem::Image(path)
        | GridItem::Video(path)
        | GridItem::Audio(path)
        | GridItem::ZipFile(path)
        | GridItem::PdfFile(path)
        | GridItem::ConvertibleArchive { path, .. }
        | GridItem::SearchContainer { path, .. }
        | GridItem::Stack {
            representative: path,
            ..
        } => native_path_text(path),
        GridItem::ZipImage {
            zip_path,
            entry_name,
        }
        | GridItem::ZipDir {
            zip_path,
            dir_prefix: entry_name,
            ..
        } => format!("{}:{}", native_path_text(zip_path), entry_name),
        GridItem::PdfPage {
            pdf_path, page_num, ..
        } => format!("{}:Page {}", native_path_text(pdf_path), page_num + 1),
    }
}

#[cfg(windows)]
fn delete_confirm_kind_for_targets(targets: &[(usize, PathBuf)]) -> DeleteConfirmKind {
    let mut checked_roots: std::collections::HashMap<String, Option<u64>> =
        std::collections::HashMap::new();
    DeleteConfirmKind::aggregate(targets.iter().map(|(_, path)| {
        let Some(root) = windows_path_root_for_file_operation(path) else {
            return DeleteConfirmKind::MayPermanent;
        };
        let max_capacity_bytes = match checked_roots.entry(root.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                if windows_root_may_permanently_delete(&root) {
                    return DeleteConfirmKind::MayPermanent;
                }
                *entry.insert(windows_recycle_bin_max_capacity_bytes(&root))
            }
        };
        if max_capacity_bytes
            .is_some_and(|capacity| windows_file_exceeds_recycle_bin_capacity(path, capacity))
        {
            DeleteConfirmKind::MayPermanent
        } else {
            DeleteConfirmKind::RecycleBin
        }
    }))
}

#[cfg(not(windows))]
fn delete_confirm_kind_for_targets(_targets: &[(usize, PathBuf)]) -> DeleteConfirmKind {
    DeleteConfirmKind::RecycleBin
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
    fn show_bookmark_grid_context_menu(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
    ) -> Option<ContextMenuAction> {
        let row = self.bookmark_browser_rows.get(idx)?.clone();
        let source = row.source_path().to_path_buf();
        let explorer_target = match &row.source {
            crate::bookmark_browser::BookmarkRowSource::Book(bookmark)
                if matches!(
                    bookmark.container_kind,
                    crate::book_bookmarks::BookContainerKind::CompiledBook
                        | crate::book_bookmarks::BookContainerKind::ImageFolder
                ) =>
            {
                Some(source.clone())
            }
            _ => source.parent().map(Path::to_path_buf),
        };
        let mut open = true;
        let mut close = false;
        let mut open_bookmark = false;
        let mut delete_bookmark = false;
        let mut open_explorer = false;
        let checked_count = self.checked.len();
        let pos = self.context_menu_pos;
        egui::Window::new("bookmark-grid-context")
            .id(egui::Id::new("bookmark_grid_context_menu"))
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_pos(pos)
            .open(&mut open)
            .show(ctx, |ui| {
                if ui
                    .add_enabled(!row.missing, egui::Button::new("ブックマーク位置を開く"))
                    .clicked()
                {
                    open_bookmark = true;
                    close = true;
                }
                if ui.button("パスをコピー").clicked() {
                    copy_path_text(ctx, &source);
                    close = true;
                }
                if explorer_target.is_some()
                    && ui.button("このフォルダをエクスプローラで開く").clicked()
                {
                    open_explorer = true;
                    close = true;
                }
                ui.separator();
                if ui
                    .add_enabled(
                        self.bookmark_delete_pending.is_none(),
                        egui::Button::new(if checked_count > 0 {
                            format!("ブックマークを {checked_count} 件削除")
                        } else {
                            "ブックマークを削除".to_string()
                        }),
                    )
                    .clicked()
                {
                    delete_bookmark = true;
                    close = true;
                }
                if ui.input(|input| input.pointer.any_click()) && !ui.ui_contains_pointer() {
                    close = true;
                }
                if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });
        if open_bookmark {
            self.open_bookmark_browser_row(ctx, &row);
        }
        if open_explorer && let Some(folder) = explorer_target {
            open_directory_in_explorer(&folder);
        }
        if delete_bookmark {
            if self.checked.is_empty() {
                self.delete_bookmark_browser_rows(std::slice::from_ref(&row));
            } else {
                self.delete_selected_bookmarks();
            }
        }
        if close || !open {
            self.context_menu_idx = None;
            self.cached_handlers = None;
        }
        None
    }

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
        if self.items_are_bookmark_view && !is_folder_context {
            return self.show_bookmark_grid_context_menu(ctx, idx);
        }
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
        let explorer_folder = context_explorer_folder(
            &item,
            is_folder_context,
            has_checked,
            folder_command_target.as_deref(),
        );
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
                ContextMenuSurface::Grid,
            ) {
                NativeGridContextMenuOutcome::Consumed { nav, .. } => {
                    self.context_menu_idx = None;
                    self.cached_handlers = None;
                    ctx.request_repaint();
                    return nav;
                }
                NativeGridContextMenuOutcome::Fallback => {}
            }
        }
        let target = self.context_menu_target(
            idx,
            item,
            is_folder_context,
            has_checked,
            folder_command_target,
            ContextMenuSurface::Grid,
            explorer_folder,
        );
        let nodes = self.context_menu_nodes(&target, in_search);
        let mut selected_command = None;
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
                selected_command = render_egui_menu_nodes(ui, &nodes);

                if ui.input(|i| i.pointer.any_click()) && !ui.ui_contains_pointer() {
                    close = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });
        if let Some(command) = selected_command {
            nav = self.dispatch_native_grid_context_command(ctx, command, &target);
            close = true;
        }
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
        surface: ContextMenuSurface,
    ) -> NativeGridContextMenuOutcome {
        if !self.settings.use_native_shell_context_menu {
            return NativeGridContextMenuOutcome::Fallback;
        }
        let Some(hwnd) = self.main_hwnd else {
            return NativeGridContextMenuOutcome::Fallback;
        };
        let prepare_t0 = std::time::Instant::now();
        let explorer_folder = context_explorer_folder(
            &item,
            is_folder_context,
            has_checked,
            folder_command_target.as_deref(),
        );
        let target = self.context_menu_target(
            idx,
            item,
            is_folder_context,
            has_checked,
            folder_command_target,
            surface,
            explorer_folder,
        );
        let Some(shell_paths) = target.shell_paths.clone() else {
            return NativeGridContextMenuOutcome::Fallback;
        };
        let miv_items = self.context_menu_nodes(&target, in_search);
        let miv_count = menu_leaf_count(&miv_items);
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
                        serde_json::Value::from(shell_paths.len() as u64),
                    ),
                    ("miv_count", serde_json::Value::from(miv_count as u64)),
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
                shell_paths.len(),
                miv_count
            ));
        }
        let background_folder = target.is_folder_context.then(|| shell_paths[0].clone());
        let request = NativeContextMenuRequest {
            hwnd,
            screen_pos: (pos.x.round() as i32, pos.y.round() as i32),
            background_folder,
            paths: if target.is_folder_context {
                Vec::new()
            } else {
                shell_paths
            },
            miv_items,
        };
        let native_result = crate::native_context_menu::show_native_context_menu(request);
        Self::resync_egui_modifiers_from_os(ctx);
        #[cfg(windows)]
        if matches!(&native_result, NativeContextMenuResult::Canceled)
            && target.is_fullscreen_video()
            && primary_mouse_button_physically_down()
        {
            // TrackPopupMenuEx を動画 HWND 上の左クリックで閉じると、その同じ click
            // sequence が presenter に遅れて届く。menu dismissal の down 時刻を所有し、
            // native video input 側で対応する down/up だけを消費する。
            self.begin_native_video_context_menu_dismiss_click();
        }
        match native_result {
            NativeContextMenuResult::Canceled | NativeContextMenuResult::ShellCommandInvoked => {
                NativeGridContextMenuOutcome::Consumed {
                    nav: None,
                    close_fullscreen: false,
                }
            }
            NativeContextMenuResult::MivCommand(command) => {
                let close_fullscreen = match command {
                    MenuCommand::ExternalTool(id) => native_external_tool_closes_fullscreen(
                        target.surface,
                        self.settings
                            .external_tools
                            .iter()
                            .any(|tool| tool.id == id),
                    ),
                    MenuCommand::OpenWithAssociation { .. } => {
                        target.surface == ContextMenuSurface::Fullscreen
                    }
                    _ => false,
                };
                let nav = self.dispatch_native_grid_context_command(ctx, command, &target);
                NativeGridContextMenuOutcome::Consumed {
                    nav,
                    close_fullscreen,
                }
            }
            NativeContextMenuResult::Fallback { reason } => {
                crate::logger::log(format!(
                    "native_context_menu: fallback to egui menu: {reason}"
                ));
                NativeGridContextMenuOutcome::Fallback
            }
        }
    }

    fn context_menu_target(
        &self,
        idx: usize,
        item: GridItem,
        is_folder_context: bool,
        has_checked: bool,
        folder_command_target: Option<PathBuf>,
        surface: ContextMenuSurface,
        explorer_folder: Option<PathBuf>,
    ) -> NativeGridContextMenuTarget {
        let checked_targets = has_checked
            .then(|| self.collect_checked_indexed_paths())
            .unwrap_or_default();
        let real_paths: Vec<PathBuf> = if is_folder_context {
            folder_command_target.iter().cloned().collect()
        } else if has_checked {
            self.collect_checked_paths()
        } else {
            item.drag_source_path()
                .map(Path::to_path_buf)
                .into_iter()
                .collect()
        };
        let shell_paths = if is_folder_context {
            folder_command_target
                .as_ref()
                .map(|folder| vec![folder.clone()])
        } else if has_checked {
            if self.checked.is_empty()
                || self.checked.iter().any(|&idx| {
                    self.items
                        .get(idx)
                        .and_then(GridItem::file_operation_path)
                        .is_none()
                })
            {
                None
            } else {
                Some(real_paths.clone())
            }
        } else {
            item.drag_source_path().map(|path| vec![path.to_path_buf()])
        };
        let delete_targets = if has_checked {
            checked_targets
        } else if is_folder_context {
            Vec::new()
        } else {
            item.drag_source_path()
                .map(|path| vec![(idx, path.to_path_buf())])
                .unwrap_or_default()
        };
        NativeGridContextMenuTarget {
            shell_paths,
            real_paths,
            delete_targets,
            item,
            item_index: (!is_folder_context).then_some(idx),
            is_folder_context,
            has_checked,
            surface,
            explorer_folder,
            folder_command_target,
        }
    }

    fn context_menu_nodes(
        &mut self,
        target: &NativeGridContextMenuTarget,
        in_search: bool,
    ) -> Vec<MenuNode> {
        let external_items = crate::external_tool::external_tool_menu_items(
            &self.settings.external_tools,
            crate::external_tool::ExternalToolMenuTarget::from_grid_item(&target.item),
        );
        let labels: Vec<_> = external_items
            .iter()
            .map(|item| item.label.as_str())
            .collect();
        crate::logger::log(format!(
            "native_context_menu: external_tools count={} labels={labels:?}",
            external_items.len()
        ));
        let external_tools = external_items
            .into_iter()
            .map(|entry| ExternalToolMenuEntry {
                tool_id: entry.tool_id,
                label: entry.label,
                enabled: entry.enabled,
                disabled_reason: entry.disabled_reason.map(str::to_string),
            })
            .collect();
        let associated_apps = self.context_menu_associated_apps(&target.item);
        let view = ContextMenuViewFlags {
            in_search,
            search: self.items_are_global_search_view
                || self.global_search.drill.is_some()
                || self.favsearch.on_results_grid()
                || !self.favsearch.nav_stack.is_empty(),
            tag: self.items_are_tag_view || !self.tag_view.nav_stack.is_empty(),
            rating: self.items_are_rating_view,
            reading_history: self.items_are_reading_history_view,
        };
        let input = ContextMenuInput {
            kind: ContextMenuItemKind::from_grid_item(&target.item),
            surface: target.surface,
            is_folder_context: target.is_folder_context,
            has_checked: target.has_checked,
            checked_count: if target.has_checked {
                self.checked.len()
            } else {
                0
            },
            real_checked_count: if target.has_checked {
                target.real_paths.len()
            } else {
                0
            },
            can_use_folder_commands: target.folder_command_target.is_some(),
            can_paste_edit_bundle: self.has_page_edit_bundle_clipboard(),
            has_explorer_folder: target.explorer_folder.is_some(),
            view,
            pin: self.context_menu_pin_state(target, view),
            external_tools,
            associated_apps,
        };
        crate::context_menu_model::build_context_menu(&input)
    }

    fn context_menu_associated_apps(&mut self, item: &GridItem) -> Vec<AssociatedAppMenuEntry> {
        let launch_target = crate::external_tool::LaunchTarget::from_grid_item(Some(item));
        let Some(extension) = launch_target
            .real_file()
            .ok()
            .and_then(Path::extension)
            .and_then(|extension| extension.to_str())
            .map(|extension| format!(".{}", extension.to_lowercase()))
        else {
            return Vec::new();
        };
        let handlers = match &self.cached_handlers {
            Some((cached_extension, handlers)) if cached_extension == &extension => {
                handlers.clone()
            }
            _ => {
                let handlers = crate::open_with::enumerate_handlers(&extension);
                self.cached_handlers = Some((extension, handlers.clone()));
                handlers
            }
        };
        handlers
            .into_iter()
            .map(|handler| AssociatedAppMenuEntry {
                display_name: handler.display_name,
                handler_id: handler.handler_id,
            })
            .collect()
    }

    fn context_menu_pin_state(
        &self,
        target: &NativeGridContextMenuTarget,
        view: ContextMenuViewFlags,
    ) -> Option<ContextMenuActionState> {
        if target.is_folder_context
            || target.has_checked
            || view.search
            || view.tag
            || view.rating
            || view.reading_history
            || self.archive_source_override.is_some() && self.zip_nav.is_none()
        {
            return None;
        }
        let idx = target.item_index?;
        let container = self.current_folder.as_ref()?;
        if is_synthetic_view_path(container) {
            return None;
        }
        if let GridItem::ConvertibleArchive { path, .. } = &target.item
            && !self
                .converted_archive_cache_paths
                .get(&crate::path_key::normalize_keep_drive(path))
                .is_some_and(crate::app::ConvertedArchiveSourceState::is_available)
        {
            return Some(ContextMenuActionState {
                label: "📌 代表サムネに固定".to_string(),
                enabled: false,
                disabled_reason: Some(
                    "変換後に設定可能 (アーカイブを ZIP に変換すると指定できます)".to_string(),
                ),
            });
        }
        let (pin_container, source) = if self.zip_nav.is_some() {
            (
                self.pin_container_key()?,
                self.folder_pin_selected_source(idx)?,
            )
        } else {
            (
                container.clone(),
                crate::folder_thumb_pins::source_from_grid_item(container, &target.item)?,
            )
        };
        let label = if self.folder_thumb_pin_for(&pin_container) == Some(&source) {
            "📌 代表サムネ固定を解除"
        } else {
            "📌 代表サムネに固定"
        };
        Some(ContextMenuActionState {
            label: label.to_string(),
            enabled: true,
            disabled_reason: None,
        })
    }

    fn dispatch_native_grid_context_command(
        &mut self,
        ctx: &egui::Context,
        command: MenuCommand,
        target: &NativeGridContextMenuTarget,
    ) -> Option<ContextMenuAction> {
        match command {
            MenuCommand::NewFolder => {
                if target.is_folder_context
                    && let Some(folder) = target.folder_command_target.clone()
                {
                    self.request_new_folder_dialog(folder);
                }
                None
            }
            MenuCommand::Paste => {
                if target.is_folder_context
                    && let (Some(hwnd), Some(folder)) =
                        (self.main_hwnd, target.folder_command_target.clone())
                {
                    // Ctrl+V と同じ約束。Shell が何を作るかは分からないので、呼ぶ前の
                    // 一覧を控えて差分で拾う ([`crate::post_operation_selection`])。
                    self.request_post_operation_selection_for_added_items(folder.clone());
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
            MenuCommand::Rename => {
                if !target.is_folder_context
                    && let Some(path) = target.item.drag_source_path().map(Path::to_path_buf)
                {
                    self.request_rename_dialog(path);
                }
                None
            }
            MenuCommand::CopyPath | MenuCommand::CopyRepresentativePath => {
                let text = if target.has_checked {
                    target
                        .real_paths
                        .iter()
                        .map(|path| native_path_text(path))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    context_item_path_text(&target.item)
                };
                ctx.copy_text(text);
                None
            }
            MenuCommand::CopyFileName | MenuCommand::CopyPageName => {
                ctx.copy_text(target.item.name().to_string());
                None
            }
            MenuCommand::CopyImageToClipboard => {
                if let Some(idx) = target.item_index {
                    let rotation = self.get_rotation(idx);
                    match &target.item {
                        GridItem::Image(path) => copy_image_to_clipboard(path, rotation),
                        GridItem::ZipImage {
                            zip_path,
                            entry_name,
                        } => copy_zip_image_to_clipboard(zip_path, entry_name, rotation),
                        _ => {}
                    }
                }
                None
            }
            MenuCommand::CopyEditBundle => {
                if let Some(idx) = target.item_index {
                    self.copy_page_edit_bundle(idx);
                }
                None
            }
            MenuCommand::PasteEditBundle => {
                if let Some(idx) = target.item_index {
                    self.request_paste_page_edit_bundle(idx);
                }
                None
            }
            MenuCommand::JumpToFolder => match &target.item {
                GridItem::Folder(path) => {
                    Some(ContextMenuAction::JumpFromSearch(native_nav_path(path)))
                }
                GridItem::SearchContainer { path, .. } => {
                    Some(ContextMenuAction::JumpFromSearch(native_nav_path(path)))
                }
                _ => target.item.drag_source_path().and_then(|path| {
                    parent_folder_for_nav(path).map(ContextMenuAction::JumpFromSearch)
                }),
            },
            MenuCommand::JumpToBookFolder => {
                self.select_after_load = Some(target.item.name().to_string());
                target.item.container_path().and_then(|path| {
                    parent_folder_for_nav(path).map(ContextMenuAction::JumpFromSearch)
                })
            }
            MenuCommand::OpenContainerAsPage => {
                target
                    .item_index
                    .map(|idx| ContextMenuAction::OpenGridContainer {
                        idx,
                        mode: crate::app::GridContainerOpenMode::PageFullscreen,
                    })
            }
            MenuCommand::OpenContainerAsList => {
                target
                    .item_index
                    .map(|idx| ContextMenuAction::OpenGridContainer {
                        idx,
                        mode: crate::app::GridContainerOpenMode::PageList,
                    })
            }
            MenuCommand::RotateLeft => {
                if target.has_checked {
                    for idx in self.checked.clone() {
                        self.rotate_image_ccw(idx);
                    }
                } else if let Some(idx) = target.item_index {
                    self.rotate_image_ccw(idx);
                }
                None
            }
            MenuCommand::RotateRight => {
                if target.has_checked {
                    for idx in self.checked.clone() {
                        self.rotate_image_cw(idx);
                    }
                } else if let Some(idx) = target.item_index {
                    self.rotate_image_cw(idx);
                }
                None
            }
            MenuCommand::ToggleRepresentativeThumb => {
                if let Some(idx) = target.item_index {
                    self.toggle_folder_pin_for_idx(idx);
                }
                None
            }
            MenuCommand::SetCurrentVideoFrameThumbnail => {
                #[cfg(windows)]
                if target.is_fullscreen_video()
                    && let Some(idx) = target.item_index
                {
                    self.pin_current_native_video_frame_for_input(ctx, idx);
                }
                None
            }
            MenuCommand::OpenFolderInExplorer => {
                if let Some(folder) = target.explorer_folder.as_deref() {
                    open_directory_in_explorer(folder);
                }
                None
            }
            MenuCommand::ExternalTool(id) => {
                let Some(tool) = self
                    .settings
                    .external_tools
                    .iter()
                    .find(|tool| tool.id == id)
                    .cloned()
                else {
                    self.show_feedback_toast(format!("外部ツールが見つかりません (ID: {})", id.0));
                    return None;
                };
                let launch_target =
                    crate::external_tool::LaunchTarget::from_grid_item(Some(&target.item));
                self.queue_external_tool_launch(&tool, &launch_target);
                None
            }
            MenuCommand::OpenWithAssociation {
                display_name,
                handler_id,
            } => {
                let launch_target =
                    crate::external_tool::LaunchTarget::from_grid_item(Some(&target.item));
                if let Ok(file) = launch_target.real_file() {
                    self.start_open_with(
                        display_name,
                        crate::external_tool::ExternalToolLaunch::Association { handler_id },
                        file.to_path_buf(),
                    );
                }
                None
            }
            MenuCommand::AddApplication => {
                if let Some(app) = crate::open_with::pick_exe_dialog() {
                    let executable = app.executable;
                    let already = self
                        .settings
                        .external_tools
                        .iter()
                        .filter_map(|tool| tool.launch.executable())
                        .any(|path| {
                            path.as_os_str()
                                .as_encoded_bytes()
                                .eq_ignore_ascii_case(executable.as_os_str().as_encoded_bytes())
                        });
                    if !already {
                        let mut tool = crate::external_tool::ExternalTool::defaults_for_viewing();
                        tool.id = crate::external_tool::next_id(&self.settings.external_tools);
                        tool.name = executable
                            .file_stem()
                            .map(|stem| stem.to_string_lossy().into_owned())
                            .unwrap_or(app.display_name);
                        tool.launch =
                            crate::external_tool::ExternalToolLaunch::Executable(executable);
                        tool.show_in_context_menu = true;
                        self.settings.external_tools.push(tool);
                        self.settings.save();
                    }
                }
                None
            }
            MenuCommand::OpenExternalToolSettings => {
                self.open_preferences_page(
                    crate::ui_dialogs::preferences::PreferencesPage::ExternalTools,
                );
                None
            }
            MenuCommand::MoveToRecycleBin => {
                if !target.delete_targets.is_empty() {
                    self.request_delete_confirm(target.delete_targets.clone());
                }
                None
            }
            MenuCommand::Deselect => {
                self.checked.clear();
                None
            }
            MenuCommand::RemoveReadingHistory => {
                if let Some(idx) = target.item_index {
                    self.remove_reading_history_entry_for_idx(idx);
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
        let explorer_folder = context_explorer_folder(&item, false, false, None);

        match self.try_show_native_grid_context_menu(
            ctx,
            pos,
            idx,
            item.clone(),
            false,
            false,
            false,
            None,
            ContextMenuSurface::Fullscreen,
        ) {
            NativeGridContextMenuOutcome::Consumed {
                close_fullscreen, ..
            } => {
                self.fs_context_menu_idx = None;
                self.cached_handlers = None;
                ctx.request_repaint();
                return close_fullscreen;
            }
            NativeGridContextMenuOutcome::Fallback => {}
        }
        let target = self.context_menu_target(
            idx,
            item,
            false,
            false,
            None,
            ContextMenuSurface::Fullscreen,
            explorer_folder,
        );
        let nodes = self.context_menu_nodes(&target, false);
        let mut selected_command = None;
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
                selected_command = render_egui_menu_nodes(ui, &nodes);

                if ui.input(|i| i.pointer.primary_clicked()) && !ui.ui_contains_pointer() {
                    close = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });
        if let Some(command) = selected_command {
            close_fullscreen = match &command {
                MenuCommand::ExternalTool(id) => native_external_tool_closes_fullscreen(
                    target.surface,
                    self.settings
                        .external_tools
                        .iter()
                        .any(|tool| tool.id == *id),
                ),
                MenuCommand::OpenWithAssociation { .. } => true,
                _ => false,
            };
            let _ = self.dispatch_native_grid_context_command(ctx, command, &target);
            close = true;
        }
        if close || !open {
            self.fs_context_menu_idx = None;
            self.cached_handlers = None;
        }
        close_fullscreen
    }
    /// チェック済みアイテムの Shell ファイル操作対象を収集する。
    pub(crate) fn collect_checked_paths(&self) -> Vec<PathBuf> {
        self.checked
            .iter()
            .filter_map(|&idx| {
                self.items
                    .get(idx)
                    .and_then(GridItem::file_operation_path)
                    .map(Path::to_path_buf)
            })
            .collect()
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
            let content = delete_confirm_label_for_targets(&self.delete_targets, &self.items);
            if should_skip_delete_confirmation(
                self.settings.skip_recycle_bin_delete_confirmation,
                &content,
            ) {
                if let Some(paths) = apply_delete_confirm_action(
                    DeleteConfirmAction::Delete,
                    &mut self.show_delete_confirm,
                    &mut self.delete_targets,
                    &mut self.delete_confirm_label,
                ) {
                    // Shell の最終判断は worker 側に委ねる。FOF_WANTNUKEWARNING は維持する。
                    self.start_delete_files(ctx, paths);
                }
                return;
            }
            ctx.data_mut(|data| {
                data.insert_temp(
                    egui::Id::new(DELETE_CONFIRM_SELECTION_ID),
                    content.kind.initial_selection(),
                );
            });
            self.delete_confirm_label = Some(content.label);
        }
        let label = self.delete_confirm_label.clone().unwrap_or_default();
        let selection = ctx
            .data(|data| data.get_temp(egui::Id::new(DELETE_CONFIRM_SELECTION_ID)))
            .unwrap_or(DeleteConfirmSelection::Cancel);

        // CLAUDE.md の IME 定型どおり Modal closure の前で capture する。
        let ime_active = self.ime_input_active(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let key_response = consume_delete_confirm_action(
            ctx,
            selection,
            ime_active,
            escape_pressed,
            enter_pressed,
        );
        ctx.data_mut(|data| {
            data.insert_temp(
                egui::Id::new(DELETE_CONFIRM_SELECTION_ID),
                key_response.selection,
            );
        });
        let modal_response = show_delete_confirm_modal(ctx, &label, key_response.selection);

        let action = if modal_response.cancel_clicked
            || key_response.action == DeleteConfirmAction::Cancel
        {
            DeleteConfirmAction::Cancel
        } else if modal_response.delete_clicked
            || key_response.action == DeleteConfirmAction::Delete
        {
            DeleteConfirmAction::Delete
        } else {
            DeleteConfirmAction::None
        };
        if let Some(paths) = apply_delete_confirm_action(
            action,
            &mut self.show_delete_confirm,
            &mut self.delete_targets,
            &mut self.delete_confirm_label,
        ) {
            // path だけを worker に渡す (idx は完了時に再解決)。
            self.start_delete_files(ctx, paths);
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

        if self.items_are_bookmark_view {
            self.delete_selected_bookmarks();
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

/// 右クリック対象に対して「このフォルダ」が指す実ディレクトリを返す。
/// 単一フォルダはそのフォルダ自身、ファイルや仮想ページは元コンテナの親、複数選択と
/// 背景メニューは現在表示中の実フォルダを使う。検索結果の複数選択のように単一の
/// 実フォルダを決められない場合は項目を出さない。
fn context_explorer_folder(
    item: &GridItem,
    is_folder_context: bool,
    has_checked: bool,
    current_real_folder: Option<&Path>,
) -> Option<PathBuf> {
    if is_folder_context || has_checked {
        return current_real_folder.map(Path::to_path_buf);
    }
    match item {
        GridItem::Folder(path) => Some(path.clone()),
        GridItem::SearchContainer {
            path,
            kind: crate::grid_item::SearchContainerKind::Folder,
            ..
        } => Some(path.clone()),
        GridItem::Image(path)
        | GridItem::Video(path)
        | GridItem::Audio(path)
        | GridItem::ZipFile(path)
        | GridItem::PdfFile(path)
        | GridItem::ConvertibleArchive { path, .. }
        | GridItem::SearchContainer { path, .. } => path.parent().map(Path::to_path_buf),
        GridItem::ZipImage { zip_path, .. } | GridItem::ZipDir { zip_path, .. } => {
            zip_path.parent().map(Path::to_path_buf)
        }
        GridItem::PdfPage { pdf_path, .. } => pdf_path.parent().map(Path::to_path_buf),
        GridItem::Stack { representative, .. } => representative.parent().map(Path::to_path_buf),
    }
}

fn open_directory_in_explorer(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer")
            .arg(native_path_text(path))
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
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

    fn target(item: GridItem, surface: ContextMenuSurface) -> NativeGridContextMenuTarget {
        let path = item.drag_source_path().map(Path::to_path_buf);
        NativeGridContextMenuTarget {
            shell_paths: path.clone().map(|path| vec![path]),
            real_paths: path.into_iter().collect(),
            delete_targets: Vec::new(),
            item,
            item_index: Some(0),
            is_folder_context: false,
            has_checked: false,
            surface,
            explorer_folder: Some(PathBuf::from(r"C:\media")),
            folder_command_target: None,
        }
    }

    fn menu_commands(
        app: &mut crate::app::App,
        item: GridItem,
        surface: ContextMenuSurface,
    ) -> Vec<MenuCommand> {
        fn visit(nodes: &[MenuNode], commands: &mut Vec<MenuCommand>) {
            for node in nodes {
                match node {
                    MenuNode::Item { command, .. } => commands.push(command.clone()),
                    MenuNode::Submenu { children, .. } => visit(children, commands),
                    MenuNode::Separator => {}
                }
            }
        }
        let target = target(item, surface);
        let mut commands = Vec::new();
        visit(&app.context_menu_nodes(&target, false), &mut commands);
        commands
    }

    #[test]
    fn native_menu_appends_all_visible_external_tools_after_one_separator() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.external_tools = (0..12)
            .map(|index| {
                let mut tool = crate::external_tool::ExternalTool::defaults_for_viewing();
                tool.id = crate::external_tool::ExternalToolId(index + 1);
                tool.name = format!("tool-{index}");
                tool
            })
            .collect();
        let mut hidden = crate::external_tool::ExternalTool::defaults_for_viewing();
        hidden.id = crate::external_tool::ExternalToolId(99);
        hidden.name = "hidden".to_string();
        hidden.show_in_context_menu = false;
        app.settings.external_tools.insert(4, hidden);

        let target = NativeGridContextMenuTarget {
            // checked の Shell paths と外部ツール対象 item は別 ownership。P1b は item 側。
            shell_paths: Some(vec![PathBuf::from(r"C:\media\checked-other.jpg")]),
            real_paths: vec![PathBuf::from(r"C:\media\checked-other.jpg")],
            delete_targets: vec![(0, PathBuf::from(r"C:\media\checked-other.jpg"))],
            item: GridItem::Image(PathBuf::from(r"C:\media\clicked.jpg")),
            item_index: Some(0),
            is_folder_context: false,
            has_checked: true,
            surface: ContextMenuSurface::Grid,
            explorer_folder: Some(PathBuf::from(r"C:\media")),
            folder_command_target: None,
        };

        let items = app.context_menu_nodes(&target, false);
        let external: Vec<_> = items
            .iter()
            .filter_map(|item| match item {
                MenuNode::Item {
                    command: MenuCommand::ExternalTool(id),
                    ..
                } => Some(*id),
                _ => None,
            })
            .collect();

        assert_eq!(external.len(), 12, "表示時に 10 件で打ち切らない");
        assert_eq!(
            external.iter().map(|id| id.0).collect::<Vec<_>>(),
            (1..=12).collect::<Vec<_>>()
        );
        let first_external = items.iter().position(|node| {
            matches!(
                node,
                MenuNode::Item {
                    command: MenuCommand::ExternalTool(_),
                    ..
                }
            )
        });
        assert!(first_external.is_some_and(|index| index > 0));
        assert!(matches!(
            items[first_external.unwrap() - 1],
            MenuNode::Separator
        ));
    }

    #[test]
    fn missing_external_tool_id_is_reported_instead_of_ignored() {
        let mut app = crate::app::setup_app_for_test();
        let target = target(
            GridItem::Image(PathBuf::from(r"C:\media\clicked.jpg")),
            ContextMenuSurface::Grid,
        );

        let action = app.dispatch_native_grid_context_command(
            &egui::Context::default(),
            MenuCommand::ExternalTool(crate::external_tool::ExternalToolId(404)),
            &target,
        );

        assert!(action.is_none());
        assert!(app.fs_feedback_toast.as_ref().is_some_and(|(text, _, _)| {
            text.contains("外部ツールが見つかりません") && text.contains("404")
        }));
    }

    #[test]
    fn native_external_tool_closes_only_fullscreen_surface_after_lookup() {
        assert!(native_external_tool_closes_fullscreen(
            ContextMenuSurface::Fullscreen,
            true
        ));
        assert!(!native_external_tool_closes_fullscreen(
            ContextMenuSurface::Grid,
            true
        ));
        assert!(!native_external_tool_closes_fullscreen(
            ContextMenuSurface::Fullscreen,
            false
        ));
    }

    #[test]
    fn fullscreen_video_menu_uses_frame_thumbnail_action_without_image_only_actions() {
        let mut app = crate::app::setup_app_for_test();
        let commands = menu_commands(
            &mut app,
            GridItem::Video(PathBuf::from("movie.mp4")),
            ContextMenuSurface::Fullscreen,
        );

        for required in [
            MenuCommand::Rename,
            MenuCommand::CopyPath,
            MenuCommand::CopyFileName,
            MenuCommand::SetCurrentVideoFrameThumbnail,
        ] {
            assert!(commands.contains(&required), "missing {required:?}");
        }
        for invalid in [
            MenuCommand::RotateLeft,
            MenuCommand::RotateRight,
            MenuCommand::ToggleRepresentativeThumb,
        ] {
            assert!(!commands.contains(&invalid), "unexpected {invalid:?}");
        }
        assert!(commands.contains(&MenuCommand::OpenFolderInExplorer));
    }

    #[test]
    fn context_menu_surface_keeps_grid_video_and_fullscreen_image_rotation_actions() {
        let mut app = crate::app::setup_app_for_test();
        let grid_video = menu_commands(
            &mut app,
            GridItem::Video(PathBuf::from("movie.mp4")),
            ContextMenuSurface::Grid,
        );
        assert!(grid_video.contains(&MenuCommand::RotateLeft));
        assert!(grid_video.contains(&MenuCommand::RotateRight));
        assert!(!grid_video.contains(&MenuCommand::SetCurrentVideoFrameThumbnail));

        let fullscreen_image = menu_commands(
            &mut app,
            GridItem::Image(PathBuf::from("image.png")),
            ContextMenuSurface::Fullscreen,
        );
        assert!(fullscreen_image.contains(&MenuCommand::RotateLeft));
        assert!(fullscreen_image.contains(&MenuCommand::RotateRight));
        assert!(!fullscreen_image.contains(&MenuCommand::SetCurrentVideoFrameThumbnail));
        assert!(fullscreen_image.contains(&MenuCommand::OpenFolderInExplorer));
    }

    #[test]
    fn context_explorer_folder_resolves_real_and_virtual_targets_without_ambiguity() {
        let current = Path::new(r"C:\library");
        assert_eq!(
            context_explorer_folder(
                &GridItem::Image(PathBuf::from(r"C:\library\page.jpg")),
                false,
                false,
                Some(current),
            ),
            Some(current.to_path_buf())
        );
        assert_eq!(
            context_explorer_folder(
                &GridItem::Folder(PathBuf::from(r"C:\library\book")),
                false,
                false,
                Some(current),
            ),
            Some(PathBuf::from(r"C:\library\book"))
        );
        assert_eq!(
            context_explorer_folder(
                &GridItem::ZipImage {
                    zip_path: PathBuf::from(r"C:\library\book.cbz"),
                    entry_name: "001.jpg".to_owned(),
                },
                false,
                false,
                None,
            ),
            Some(current.to_path_buf())
        );
        assert_eq!(
            context_explorer_folder(
                &GridItem::Image(PathBuf::from(r"C:\one\page.jpg")),
                false,
                true,
                None,
            ),
            None,
            "cross-folder checked results have no single Explorer folder"
        );
    }

    fn delete_content(kind: DeleteConfirmKind) -> DeleteConfirmContent {
        DeleteConfirmContent {
            label: String::new(),
            kind,
        }
    }

    #[test]
    fn ordinary_local_file_skips_confirmation_when_setting_is_on() {
        let content = delete_content(DeleteConfirmKind::RecycleBin);

        assert!(should_skip_delete_confirmation(true, &content));
        assert!(!should_skip_delete_confirmation(false, &content));
    }

    #[test]
    fn may_permanent_item_still_confirms_with_cancel_preselected() {
        let content = delete_content(DeleteConfirmKind::MayPermanent);

        assert!(!should_skip_delete_confirmation(true, &content));
        assert_eq!(
            content.kind.initial_selection(),
            DeleteConfirmSelection::Cancel
        );
    }

    #[test]
    fn mixed_selection_uses_may_permanent_kind_and_does_not_skip() {
        let kind = DeleteConfirmKind::aggregate([
            DeleteConfirmKind::RecycleBin,
            DeleteConfirmKind::MayPermanent,
            DeleteConfirmKind::RecycleBin,
        ]);
        let content = delete_content(kind);

        assert_eq!(kind, DeleteConfirmKind::MayPermanent);
        assert!(!should_skip_delete_confirmation(true, &content));
        assert_eq!(kind.initial_selection(), DeleteConfirmSelection::Cancel);
    }

    #[test]
    fn folders_and_archive_containers_skip_confirmation_when_setting_is_on() {
        let items = vec![
            GridItem::Folder(PathBuf::from(r"C:\pictures\album")),
            GridItem::ZipFile(PathBuf::from(r"C:\pictures\book.zip")),
            GridItem::ZipFile(PathBuf::from(r"C:\pictures\book.cbz")),
            GridItem::PdfFile(PathBuf::from(r"C:\pictures\book.pdf")),
            GridItem::ConvertibleArchive {
                path: PathBuf::from(r"C:\pictures\book.rar"),
                format: crate::archive_converter::ArchiveFormat::Rar,
            },
            GridItem::ConvertibleArchive {
                path: PathBuf::from(r"C:\pictures\book.cbr"),
                format: crate::archive_converter::ArchiveFormat::Rar,
            },
            GridItem::ConvertibleArchive {
                path: PathBuf::from(r"C:\pictures\book.7z"),
                format: crate::archive_converter::ArchiveFormat::SevenZ,
            },
            GridItem::ConvertibleArchive {
                path: PathBuf::from(r"C:\pictures\book.cb7"),
                format: crate::archive_converter::ArchiveFormat::SevenZ,
            },
            GridItem::ConvertibleArchive {
                path: PathBuf::from(r"C:\pictures\book.lzh"),
                format: crate::archive_converter::ArchiveFormat::Lzh,
            },
            GridItem::ConvertibleArchive {
                path: PathBuf::from(r"C:\pictures\book.lha"),
                format: crate::archive_converter::ArchiveFormat::Lzh,
            },
        ];

        for item in items {
            assert!(
                item.drag_source_path().is_some(),
                "container test item should have a real delete target"
            );
            let content = delete_content(DeleteConfirmKind::RecycleBin);
            assert!(should_skip_delete_confirmation(true, &content));
            assert!(!should_skip_delete_confirmation(false, &content));
        }
    }

    #[test]
    fn disabled_setting_keeps_confirmation_for_every_delete_kind() {
        for kind in [
            DeleteConfirmKind::RecycleBin,
            DeleteConfirmKind::MayPermanent,
        ] {
            let content = delete_content(kind);
            assert!(!should_skip_delete_confirmation(false, &content));
            assert_eq!(content.kind.initial_selection(), kind.initial_selection());
        }
    }

    #[test]
    fn delete_confirm_label_keeps_recycle_bin_wording_for_normal_targets() {
        let label =
            build_delete_confirm_label(1, Some("sample.jpg"), DeleteConfirmKind::RecycleBin, false);
        assert_eq!(label, "「sample.jpg」をゴミ箱に移動しますか？");

        let label = build_delete_confirm_label(3, None, DeleteConfirmKind::RecycleBin, false);
        assert_eq!(label, "3 件の項目をゴミ箱に移動しますか？");
    }

    #[test]
    fn delete_confirm_label_warns_when_delete_may_be_permanent() {
        let label = build_delete_confirm_label(
            1,
            Some("sample.jpg"),
            DeleteConfirmKind::MayPermanent,
            false,
        );
        assert!(
            label.contains("完全に削除される場合があります"),
            "single-target warning should mention permanent deletion: {label}"
        );
        assert!(
            label.contains("sample.jpg"),
            "single-target warning should include the file name: {label}"
        );

        let label = build_delete_confirm_label(2, None, DeleteConfirmKind::MayPermanent, false);
        assert!(
            label.contains("ゴミ箱に移動できない場所"),
            "multi-target warning should mention non-recyclable locations: {label}"
        );
    }

    #[test]
    fn delete_confirm_always_warns_for_folders_but_not_file_only_targets() {
        for kind in [
            DeleteConfirmKind::RecycleBin,
            DeleteConfirmKind::MayPermanent,
        ] {
            let folder_label = build_delete_confirm_label(1, Some("album"), kind, true);
            assert!(
                folder_label.contains(DELETE_FOLDER_OMITTED_FILES_WARNING),
                "folder warning is required for every delete route: {folder_label}"
            );

            let file_label = build_delete_confirm_label(1, Some("photo.jpg"), kind, false);
            assert!(
                !file_label.contains(DELETE_FOLDER_OMITTED_FILES_WARNING),
                "file-only deletion must not show the folder warning: {file_label}"
            );
        }
    }

    #[test]
    fn delete_target_kind_is_derived_from_the_existing_grid_item_without_io() {
        let folder = PathBuf::from(r"C:\pictures\album");
        let targets = vec![(0, folder.clone())];
        let folder_label =
            delete_confirm_label_for_targets(&targets, &[GridItem::Folder(folder.clone())]);
        assert!(
            folder_label
                .label
                .contains(DELETE_FOLDER_OMITTED_FILES_WARNING)
        );

        let file_label = delete_confirm_label_for_targets(&targets, &[GridItem::Image(folder)]);
        assert!(
            !file_label
                .label
                .contains(DELETE_FOLDER_OMITTED_FILES_WARNING)
        );
    }

    #[test]
    fn delete_confirm_lists_first_ten_targets_then_remaining_count() {
        let targets = (0..13)
            .map(|idx| {
                (
                    idx,
                    PathBuf::from(format!(r"C:\pictures\image_{idx:02}.jpg")),
                )
            })
            .collect::<Vec<_>>();

        let items = targets
            .iter()
            .map(|(_, path)| GridItem::Image(path.clone()))
            .collect::<Vec<_>>();
        let label = delete_confirm_label_for_targets(&targets, &items).label;

        for idx in 0..DELETE_CONFIRM_VISIBLE_TARGET_LIMIT {
            assert!(
                label.contains(&format!("image_{idx:02}.jpg")),
                "先頭 10 件の名前を列挙する: {label}"
            );
        }
        assert!(!label.contains("image_10.jpg"));
        assert_eq!(label.matches("\n・").count(), 11);
        assert!(label.contains("・他 3 件"));
    }

    #[test]
    fn delete_confirm_single_target_keeps_natural_filename_wording() {
        let targets = vec![(0, PathBuf::from(r"C:\pictures\only_one.jpg"))];

        let items = vec![GridItem::Image(targets[0].1.clone())];
        let label = delete_confirm_label_for_targets(&targets, &items).label;

        assert!(label.contains("only_one.jpg"));
        assert!(!label.contains("\n\n対象:"));
    }

    fn begin_key_pass(ctx: &egui::Context, key: egui::Key) {
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        });
    }

    #[test]
    fn delete_confirm_initial_selection_depends_on_delete_kind() {
        assert_eq!(
            DeleteConfirmKind::RecycleBin.initial_selection(),
            DeleteConfirmSelection::Delete
        );
        assert_eq!(
            DeleteConfirmKind::MayPermanent.initial_selection(),
            DeleteConfirmSelection::Cancel
        );
    }

    #[test]
    fn delete_confirm_fixed_keys_resolve_y_n_escape_and_selected_enter() {
        assert_eq!(
            resolve_delete_confirm_action(
                DeleteConfirmSelection::Cancel,
                true,
                false,
                false,
                false,
                false,
            ),
            DeleteConfirmAction::Delete
        );
        assert_eq!(
            resolve_delete_confirm_action(
                DeleteConfirmSelection::Delete,
                false,
                true,
                false,
                false,
                false,
            ),
            DeleteConfirmAction::Cancel
        );
        assert_eq!(
            resolve_delete_confirm_action(
                DeleteConfirmSelection::Delete,
                false,
                false,
                true,
                false,
                false,
            ),
            DeleteConfirmAction::Cancel
        );
        assert_eq!(
            resolve_delete_confirm_action(
                DeleteConfirmSelection::Delete,
                false,
                false,
                false,
                true,
                false,
            ),
            DeleteConfirmAction::Delete
        );
        assert_eq!(
            resolve_delete_confirm_action(
                DeleteConfirmSelection::Cancel,
                false,
                false,
                false,
                true,
                false,
            ),
            DeleteConfirmAction::Cancel
        );
    }

    #[test]
    fn delete_confirm_ignores_y_n_enter_during_ime_composition() {
        assert_eq!(
            resolve_delete_confirm_action(
                DeleteConfirmSelection::Delete,
                true,
                false,
                false,
                false,
                true,
            ),
            DeleteConfirmAction::None
        );
        assert_eq!(
            resolve_delete_confirm_action(
                DeleteConfirmSelection::Cancel,
                false,
                true,
                false,
                false,
                true,
            ),
            DeleteConfirmAction::None
        );
        assert_eq!(
            resolve_delete_confirm_action(
                DeleteConfirmSelection::Delete,
                false,
                false,
                false,
                true,
                true,
            ),
            DeleteConfirmAction::None
        );
    }

    #[test]
    fn delete_confirm_arrow_directions_select_delete_or_cancel() {
        assert_eq!(
            move_delete_confirm_selection(DeleteConfirmSelection::Cancel, true, false),
            DeleteConfirmSelection::Delete
        );
        assert_eq!(
            move_delete_confirm_selection(DeleteConfirmSelection::Delete, false, true),
            DeleteConfirmSelection::Cancel
        );
    }

    #[test]
    fn delete_confirm_modal_blocks_background_pointer_action() {
        use egui_kittest::{Harness, kittest::Queryable};
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let background_clicked = Arc::new(AtomicBool::new(false));
        let clicked_in_ui = Arc::clone(&background_clicked);
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(480.0, 300.0))
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Light);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    if ui.button("Background action").clicked() {
                        clicked_in_ui.store(true, Ordering::Relaxed);
                    }
                });
                let _ = show_delete_confirm_modal(
                    ctx,
                    "sample.jpg をゴミ箱に移動しますか？",
                    DeleteConfirmSelection::Delete,
                );
            });

        harness.get_by_label("Background action").click();
        harness.run();
        assert!(
            !background_clicked.load(Ordering::Relaxed),
            "削除確認の backdrop は背面ボタンへの pointer 操作を遮断する"
        );
        harness.snapshot("delete_confirm_modal_blocks_background_light");
    }

    #[test]
    fn delete_confirm_state_transition_returns_delete_worker_seam_or_cancels() {
        let mut shown = true;
        let mut targets = vec![(3, PathBuf::from("a.jpg")), (1, PathBuf::from("b.jpg"))];
        let mut label = Some("confirm".to_owned());
        let paths = apply_delete_confirm_action(
            DeleteConfirmAction::Delete,
            &mut shown,
            &mut targets,
            &mut label,
        )
        .expect("Delete は start_delete_files へ渡す path 列を返す");
        assert_eq!(paths, vec![PathBuf::from("a.jpg"), PathBuf::from("b.jpg")]);
        assert!(!shown);
        assert!(targets.is_empty());
        assert!(label.is_none());

        for action in [DeleteConfirmAction::Cancel, DeleteConfirmAction::None] {
            let mut shown = true;
            let mut targets = vec![(0, PathBuf::from("keep.jpg"))];
            let mut label = Some("confirm".to_owned());
            let paths = apply_delete_confirm_action(action, &mut shown, &mut targets, &mut label);
            assert!(paths.is_none());
            if action == DeleteConfirmAction::Cancel {
                assert!(!shown);
                assert!(targets.is_empty());
                assert!(label.is_none());
            } else {
                assert!(shown);
                assert_eq!(targets.len(), 1);
                assert!(label.is_some());
            }
        }
    }

    #[test]
    fn delete_confirm_consumes_y_before_background_keymap_dispatch() {
        let keymap = crate::keymap::Keymap::from_ini_str("[Grid]\nGridPin = Y\n");
        let ctx = egui::Context::default();
        begin_key_pass(&ctx, egui::Key::Y);
        let response = consume_delete_confirm_action(
            &ctx,
            DeleteConfirmSelection::Cancel,
            false,
            false,
            false,
        );
        assert_eq!(response.action, DeleteConfirmAction::Delete);
        assert!(
            !keymap.consume_action(&ctx, crate::keymap::KeyAction::GridPin),
            "削除確認で消費した Y は同 frame の背面 KeyAction に届かない"
        );
        let _ = ctx.end_pass();
    }

    #[test]
    fn delete_confirm_consumes_n_even_when_ime_guard_ignores_it() {
        let keymap = crate::keymap::Keymap::from_ini_str("[Grid]\nGridPin = N\n");
        let ctx = egui::Context::default();
        begin_key_pass(&ctx, egui::Key::N);
        let response =
            consume_delete_confirm_action(&ctx, DeleteConfirmSelection::Delete, true, false, false);
        assert_eq!(response.action, DeleteConfirmAction::None);
        assert!(!keymap.consume_action(&ctx, crate::keymap::KeyAction::GridPin));
        let _ = ctx.end_pass();
    }

    #[test]
    fn delete_confirm_consumes_escape_after_ime_safe_capture() {
        let ctx = egui::Context::default();
        begin_key_pass(&ctx, egui::Key::Escape);
        let response =
            consume_delete_confirm_action(&ctx, DeleteConfirmSelection::Delete, false, true, false);
        assert_eq!(response.action, DeleteConfirmAction::Cancel);
        assert!(ctx.input(|input| input.events.is_empty()));
        let _ = ctx.end_pass();
    }

    #[test]
    fn delete_confirm_consumes_enter_and_uses_current_selection() {
        let ctx = egui::Context::default();
        begin_key_pass(&ctx, egui::Key::Enter);
        let response =
            consume_delete_confirm_action(&ctx, DeleteConfirmSelection::Cancel, false, false, true);
        assert_eq!(response.action, DeleteConfirmAction::Cancel);
        assert!(ctx.input(|input| input.events.is_empty()));
        let _ = ctx.end_pass();
    }

    #[test]
    fn delete_confirm_consumes_horizontal_and_vertical_arrow_synonyms() {
        for (key, expected) in [
            (egui::Key::ArrowLeft, DeleteConfirmSelection::Delete),
            (egui::Key::ArrowUp, DeleteConfirmSelection::Delete),
            (egui::Key::ArrowRight, DeleteConfirmSelection::Cancel),
            (egui::Key::ArrowDown, DeleteConfirmSelection::Cancel),
        ] {
            let ctx = egui::Context::default();
            begin_key_pass(&ctx, key);
            let response = consume_delete_confirm_action(
                &ctx,
                DeleteConfirmSelection::Cancel,
                false,
                false,
                false,
            );
            assert_eq!(response.selection, expected, "{key:?}");
            assert_eq!(response.action, DeleteConfirmAction::None);
            assert!(ctx.input(|input| input.events.is_empty()));
            let _ = ctx.end_pass();
        }
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
