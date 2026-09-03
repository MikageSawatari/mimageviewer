//! Right-click menu definition shared by the native Win32 and egui renderers.
//!
//! This module owns only pure data and predicates. Callers snapshot any App state
//! (pin state, associated applications, clipboard availability, and view flags)
//! before calling [`build_context_menu`].

use crate::external_tool::ExternalToolId;
use crate::grid_item::GridItem;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuNode {
    Item {
        command: MenuCommand,
        label: String,
        enabled: bool,
        disabled_reason: Option<String>,
    },
    Submenu {
        label: String,
        children: Vec<MenuNode>,
    },
    Separator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuCommand {
    NewFolder,
    Paste,
    Rename,
    CopyPath,
    CopyFileName,
    CopyPageName,
    CopyRepresentativePath,
    CopyImageToClipboard,
    CopyEditBundle,
    PasteEditBundle,
    BulkPasteEditBundle,
    ResetPageEdits,
    JumpToFolder,
    JumpToBookFolder,
    OpenContainerAsPage,
    OpenContainerAsList,
    RotateLeft,
    RotateRight,
    ToggleRepresentativeThumb,
    SetCurrentVideoFrameThumbnail,
    OpenFolderInExplorer,
    ExternalTool(ExternalToolId),
    OpenWithAssociation {
        display_name: String,
        handler_id: String,
    },
    OpenExternalToolSettings,
    MoveToRecycleBin,
    Deselect,
    RemoveReadingHistory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMenuSurface {
    Grid,
    Fullscreen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMenuItemKind {
    Folder,
    Image,
    Video,
    Audio,
    ZipFile,
    PdfFile,
    ConvertibleArchive,
    ZipImage,
    PdfPage,
    Stack,
    ZipDir,
    SearchContainer,
}

impl ContextMenuItemKind {
    pub fn from_grid_item(item: &GridItem) -> Self {
        match item {
            GridItem::Folder(_) => Self::Folder,
            GridItem::Image(_) => Self::Image,
            GridItem::Video(_) => Self::Video,
            GridItem::Audio(_) => Self::Audio,
            GridItem::ZipFile(_) => Self::ZipFile,
            GridItem::PdfFile(_) => Self::PdfFile,
            GridItem::ConvertibleArchive { .. } => Self::ConvertibleArchive,
            GridItem::ZipImage { .. } => Self::ZipImage,
            GridItem::PdfPage { .. } => Self::PdfPage,
            GridItem::Stack { .. } => Self::Stack,
            GridItem::ZipDir { .. } => Self::ZipDir,
            GridItem::SearchContainer { .. } => Self::SearchContainer,
        }
    }

    fn is_real_item(self) -> bool {
        matches!(
            self,
            Self::Folder
                | Self::Image
                | Self::Video
                | Self::Audio
                | Self::ZipFile
                | Self::PdfFile
                | Self::ConvertibleArchive
        )
    }

    fn has_file_name(self) -> bool {
        self.is_real_item() || matches!(self, Self::ZipImage)
    }

    fn supports_page_edits(self) -> bool {
        matches!(self, Self::Image | Self::ZipImage | Self::PdfPage)
    }

    fn supports_container_open(self) -> bool {
        matches!(
            self,
            Self::ZipFile | Self::PdfFile | Self::ConvertibleArchive
        )
    }

    fn supports_open_with(self) -> bool {
        matches!(
            self,
            Self::Image
                | Self::Video
                | Self::Audio
                | Self::ZipFile
                | Self::PdfFile
                | Self::ConvertibleArchive
                | Self::ZipImage
                | Self::PdfPage
                | Self::Stack
        )
    }

    fn supports_delete(self) -> bool {
        self.is_real_item()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContextMenuViewFlags {
    pub in_search: bool,
    pub search: bool,
    pub tag: bool,
    pub rating: bool,
    pub reading_history: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenuActionState {
    pub label: String,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalToolMenuEntry {
    pub tool_id: ExternalToolId,
    pub label: String,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssociatedAppMenuEntry {
    pub display_name: String,
    pub handler_id: String,
    /// Windows が「おすすめ」に分類しているか。区切り線を入れる位置を決めるだけで、
    /// 候補を絞る条件には使わない。
    pub is_recommended: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenuInput {
    pub kind: ContextMenuItemKind,
    pub surface: ContextMenuSurface,
    pub is_folder_context: bool,
    pub has_checked: bool,
    pub checked_count: usize,
    pub can_use_folder_commands: bool,
    pub can_paste_edit_bundle: bool,
    pub has_explorer_folder: bool,
    pub view: ContextMenuViewFlags,
    pub pin: Option<ContextMenuActionState>,
    pub external_tools: Vec<ExternalToolMenuEntry>,
    pub associated_apps: Vec<AssociatedAppMenuEntry>,
    pub shortcuts: ContextMenuShortcutLabels,
}

/// メニューに併記するキーの表示。**実際の割り当てから作った文字列**を呼び出し側が入れる。
///
/// 既定キーをこのモジュール内へ書き直すと、操作カスタマイズで割り当てを変えたり解除したり
/// しても以前のキーが出続ける。native / egui の両描画がこの 1 つのモデルを見るので、
/// 書き直した瞬間に両方へ同じずれが固定される (v3.5.0 レビュー F16)。
///
/// `None` は「そのキーは割り当てられていない」= 併記しない。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextMenuShortcutLabels {
    pub rotate_left: Option<String>,
    pub rotate_right: Option<String>,
    pub deselect: Option<String>,
}

/// `label` に、割り当てがあるときだけ ` (キー)` を足す。
fn with_key(label: &str, key: Option<&String>) -> String {
    match key {
        Some(key) => format!("{label} ({key})"),
        None => label.to_string(),
    }
}

fn item(command: MenuCommand, label: impl Into<String>) -> MenuNode {
    MenuNode::Item {
        command,
        label: label.into(),
        enabled: true,
        disabled_reason: None,
    }
}

fn state_item(command: MenuCommand, state: ContextMenuActionState) -> MenuNode {
    MenuNode::Item {
        command,
        label: state.label,
        enabled: state.enabled,
        disabled_reason: state.disabled_reason,
    }
}

fn push_group(nodes: &mut Vec<MenuNode>, group: impl IntoIterator<Item = MenuNode>) {
    let mut group: Vec<_> = group.into_iter().collect();
    if group.is_empty() {
        return;
    }
    if !nodes.is_empty() {
        nodes.push(MenuNode::Separator);
    }
    nodes.append(&mut group);
}

fn external_tool_nodes(entries: &[ExternalToolMenuEntry]) -> Vec<MenuNode> {
    entries
        .iter()
        .map(|entry| MenuNode::Item {
            command: MenuCommand::ExternalTool(entry.tool_id),
            label: entry.label.clone(),
            enabled: entry.enabled,
            disabled_reason: entry.disabled_reason.clone(),
        })
        .collect()
}

fn open_with_submenu(input: &ContextMenuInput) -> Option<MenuNode> {
    if !input.kind.supports_open_with() {
        return None;
    }
    // Windows の「プログラムから開く」と同じく、おすすめとその他を区切って見せる。
    // 一覧は絞らない (絞ると OS に出るアプリが mIV に出ないことになる) が、
    // 区切りが無いと利用者にはどこまでがおすすめか分からない (2026-09-01 指摘)。
    let mut children = Vec::new();
    let mut previous_recommended: Option<bool> = None;
    for app in &input.associated_apps {
        if previous_recommended == Some(true) && !app.is_recommended {
            children.push(MenuNode::Separator);
        }
        previous_recommended = Some(app.is_recommended);
        children.push(item(
            MenuCommand::OpenWithAssociation {
                display_name: app.display_name.clone(),
                handler_id: app.handler_id.clone(),
            },
            app.display_name.clone(),
        ));
    }
    push_group(
        &mut children,
        [item(
            MenuCommand::OpenExternalToolSettings,
            "外部ツールの設定…",
        )],
    );
    Some(MenuNode::Submenu {
        label: "アプリケーションで開く…".to_string(),
        children,
    })
}

/// Build the complete mIV context-menu tree from an immutable snapshot.
pub fn build_context_menu(input: &ContextMenuInput) -> Vec<MenuNode> {
    let mut nodes = Vec::new();

    if input.has_checked {
        push_group(
            &mut nodes,
            [item(
                MenuCommand::CopyPath,
                format!("選択項目のパスをコピー [{}件]", input.checked_count),
            )],
        );
        if input.surface == ContextMenuSurface::Grid {
            push_group(
                &mut nodes,
                [
                    item(
                        MenuCommand::RotateLeft,
                        with_key("左に回転", input.shortcuts.rotate_left.as_ref()),
                    ),
                    item(
                        MenuCommand::RotateRight,
                        with_key("右に回転", input.shortcuts.rotate_right.as_ref()),
                    ),
                ],
            );
        }
        if input.surface == ContextMenuSurface::Grid {
            // 対象外 (動画 / 音声 / フォルダ) が混じっていても出す。回転と同じで、
            // 選択の中身ではなく操作の有無で決める。実際に何件へ効くかは確認
            // ダイアログが「対象 N 件 / 対象外 M 件」として出す。
            push_group(
                &mut nodes,
                [
                    MenuNode::Item {
                        command: MenuCommand::BulkPasteEditBundle,
                        label: format!("編集内容をまとめて貼り付け [{}件]", input.checked_count),
                        enabled: input.can_paste_edit_bundle,
                        disabled_reason: (!input.can_paste_edit_bundle)
                            .then(|| "コピーされた編集内容がありません".to_string()),
                    },
                    item(
                        MenuCommand::ResetPageEdits,
                        format!("編集内容をリセット… [{}件]", input.checked_count),
                    ),
                ],
            );
        }
        if input.has_explorer_folder {
            push_group(
                &mut nodes,
                [item(
                    MenuCommand::OpenFolderInExplorer,
                    "このフォルダをエクスプローラで開く",
                )],
            );
        }
        push_group(&mut nodes, external_tool_nodes(&input.external_tools));
        if let Some(open_with) = open_with_submenu(input) {
            push_group(&mut nodes, [open_with]);
        }
        if input.surface == ContextMenuSurface::Grid {
            push_group(
                &mut nodes,
                [item(
                    MenuCommand::MoveToRecycleBin,
                    format!(
                        "ゴミ箱へ移動 (タグ・評価も整理) [{}件]",
                        input.checked_count
                    ),
                )],
            );
        }
        if input.surface == ContextMenuSurface::Grid {
            push_group(
                &mut nodes,
                [item(
                    MenuCommand::Deselect,
                    with_key("選択解除", input.shortcuts.deselect.as_ref()),
                )],
            );
        }
        return normalize_menu(nodes);
    }

    if input.is_folder_context && input.can_use_folder_commands {
        push_group(
            &mut nodes,
            [
                item(MenuCommand::NewFolder, "新しいフォルダ…"),
                item(MenuCommand::Paste, "貼り付け"),
            ],
        );
    }

    if !input.is_folder_context && input.kind.is_real_item() {
        push_group(&mut nodes, [item(MenuCommand::Rename, "名前の変更…")]);
    }

    let mut copy_group = Vec::new();
    if input.kind == ContextMenuItemKind::Stack {
        copy_group.push(item(
            MenuCommand::CopyRepresentativePath,
            "代表画像のパスをコピー",
        ));
    } else {
        copy_group.push(item(
            MenuCommand::CopyPath,
            if input.is_folder_context {
                "このフォルダのパスをコピー"
            } else {
                "パスをコピー"
            },
        ));
    }
    if input.kind.has_file_name() && !input.is_folder_context {
        copy_group.push(item(MenuCommand::CopyFileName, "ファイル名をコピー"));
    } else if input.kind == ContextMenuItemKind::PdfPage {
        copy_group.push(item(MenuCommand::CopyPageName, "ページ名をコピー"));
    }
    if matches!(
        input.kind,
        ContextMenuItemKind::Image | ContextMenuItemKind::ZipImage
    ) {
        copy_group.push(item(
            MenuCommand::CopyImageToClipboard,
            "画像をクリップボードにコピー",
        ));
    }
    if input.kind.supports_page_edits() {
        copy_group.push(item(MenuCommand::CopyEditBundle, "編集内容をコピー"));
        copy_group.push(MenuNode::Item {
            command: MenuCommand::PasteEditBundle,
            label: "編集内容を貼り付け".to_string(),
            enabled: input.can_paste_edit_bundle,
            disabled_reason: (!input.can_paste_edit_bundle)
                .then(|| "コピーされた編集内容がありません".to_string()),
        });
    }
    push_group(&mut nodes, copy_group);

    if input.kind.supports_page_edits() {
        // コピー系とは別グループにして、消す操作を並びで見分けられるようにする。
        push_group(
            &mut nodes,
            [item(MenuCommand::ResetPageEdits, "編集内容をリセット…")],
        );
    }

    if input.surface == ContextMenuSurface::Grid && input.kind.supports_container_open() {
        push_group(
            &mut nodes,
            [
                item(MenuCommand::OpenContainerAsPage, "ページを開く"),
                item(MenuCommand::OpenContainerAsList, "一覧を開く"),
            ],
        );
    }

    let can_jump_to_folder = input.view.in_search
        && !input.is_folder_context
        && matches!(
            input.kind,
            ContextMenuItemKind::Folder
                | ContextMenuItemKind::Image
                | ContextMenuItemKind::Video
                | ContextMenuItemKind::Audio
                | ContextMenuItemKind::ZipFile
                | ContextMenuItemKind::PdfFile
                | ContextMenuItemKind::ConvertibleArchive
                | ContextMenuItemKind::SearchContainer
        );
    let can_jump_to_book = input.surface == ContextMenuSurface::Grid
        && input.view.reading_history
        && !input.is_folder_context
        && matches!(
            input.kind,
            ContextMenuItemKind::Folder
                | ContextMenuItemKind::ZipFile
                | ContextMenuItemKind::PdfFile
                | ContextMenuItemKind::ConvertibleArchive
        );
    let mut navigation_group = Vec::new();
    if can_jump_to_folder {
        navigation_group.push(item(MenuCommand::JumpToFolder, "フォルダに移動"));
    }
    if can_jump_to_book {
        navigation_group.push(item(
            MenuCommand::JumpToBookFolder,
            "この本のフォルダに移動",
        ));
    }
    push_group(&mut nodes, navigation_group);

    let can_rotate = match input.surface {
        ContextMenuSurface::Grid => matches!(
            input.kind,
            ContextMenuItemKind::Image
                | ContextMenuItemKind::Video
                | ContextMenuItemKind::Audio
                | ContextMenuItemKind::ZipImage
                | ContextMenuItemKind::PdfPage
        ),
        ContextMenuSurface::Fullscreen => matches!(
            input.kind,
            ContextMenuItemKind::Image
                | ContextMenuItemKind::ZipImage
                | ContextMenuItemKind::PdfPage
        ),
    };
    if can_rotate {
        push_group(
            &mut nodes,
            [
                item(
                    MenuCommand::RotateLeft,
                    with_key("左に回転", input.shortcuts.rotate_left.as_ref()),
                ),
                item(
                    MenuCommand::RotateRight,
                    with_key("右に回転", input.shortcuts.rotate_right.as_ref()),
                ),
            ],
        );
    }

    if input.surface == ContextMenuSurface::Fullscreen && input.kind == ContextMenuItemKind::Video {
        push_group(
            &mut nodes,
            [item(
                MenuCommand::SetCurrentVideoFrameThumbnail,
                "📌 現在のフレームを動画サムネに設定",
            )],
        );
    } else if !input.view.search
        && !input.view.tag
        && !input.view.rating
        && !input.view.reading_history
        && let Some(pin) = input.pin.clone()
    {
        push_group(
            &mut nodes,
            [state_item(MenuCommand::ToggleRepresentativeThumb, pin)],
        );
    }

    if input.has_explorer_folder {
        push_group(
            &mut nodes,
            [item(
                MenuCommand::OpenFolderInExplorer,
                "このフォルダをエクスプローラで開く",
            )],
        );
    }

    push_group(&mut nodes, external_tool_nodes(&input.external_tools));
    if let Some(open_with) = open_with_submenu(input) {
        push_group(&mut nodes, [open_with]);
    }

    if input.surface == ContextMenuSurface::Grid
        && !input.is_folder_context
        && input.kind.supports_delete()
    {
        push_group(
            &mut nodes,
            [item(
                MenuCommand::MoveToRecycleBin,
                "ゴミ箱へ移動 (タグ・評価も整理)",
            )],
        );
    }

    if input.surface == ContextMenuSurface::Grid
        && input.view.reading_history
        && !input.is_folder_context
    {
        push_group(
            &mut nodes,
            [item(MenuCommand::RemoveReadingHistory, "履歴から削除")],
        );
    }

    normalize_menu(nodes)
}

/// Remove empty submenus and collapse leading, trailing, and repeated separators.
pub fn normalize_menu(nodes: Vec<MenuNode>) -> Vec<MenuNode> {
    let mut normalized = Vec::new();
    for node in nodes {
        let node = match node {
            MenuNode::Submenu { label, children } => {
                let children = normalize_menu(children);
                if children.is_empty() {
                    continue;
                }
                MenuNode::Submenu { label, children }
            }
            other => other,
        };
        if matches!(node, MenuNode::Separator)
            && (normalized.is_empty() || matches!(normalized.last(), Some(MenuNode::Separator)))
        {
            continue;
        }
        normalized.push(node);
    }
    if matches!(normalized.last(), Some(MenuNode::Separator)) {
        normalized.pop();
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(kind: ContextMenuItemKind, surface: ContextMenuSurface) -> ContextMenuInput {
        ContextMenuInput {
            kind,
            surface,
            is_folder_context: false,
            has_checked: false,
            checked_count: 0,
            can_use_folder_commands: false,
            can_paste_edit_bundle: false,
            has_explorer_folder: false,
            view: ContextMenuViewFlags::default(),
            pin: None,
            external_tools: Vec::new(),
            associated_apps: Vec::new(),
            // 既存の期待値と揃うよう、既定の割り当てを解決した結果を渡す。
            shortcuts: ContextMenuShortcutLabels {
                rotate_left: Some("L".to_string()),
                rotate_right: Some("R".to_string()),
                deselect: Some("Ctrl+D".to_string()),
            },
        }
    }

    fn labels(nodes: &[MenuNode]) -> Vec<String> {
        let mut out = Vec::new();
        fn visit(nodes: &[MenuNode], out: &mut Vec<String>) {
            for node in nodes {
                match node {
                    MenuNode::Item { label, .. } => out.push(label.clone()),
                    MenuNode::Submenu { label, children } => {
                        out.push(label.clone());
                        visit(children, out);
                    }
                    MenuNode::Separator => {}
                }
            }
        }
        visit(nodes, &mut out);
        out
    }

    /// キー併記は snapshot が渡した**実際の割り当て**をそのまま出す。
    ///
    /// 既定キーをモデル内へ書いていたので、操作カスタマイズで割り当てを変えても
    /// native / egui の両メニューに以前のキーが出ていた (F16)。
    #[test]
    fn the_menu_shows_the_key_that_is_actually_assigned() {
        let mut input = input(ContextMenuItemKind::Image, ContextMenuSurface::Grid);
        input.has_checked = true;
        input.checked_count = 2;
        input.shortcuts = ContextMenuShortcutLabels {
            rotate_left: Some("Shift+F1".to_string()),
            rotate_right: Some("Shift+F2".to_string()),
            deselect: Some("Alt+Q".to_string()),
        };

        let shown = labels(&build_context_menu(&input));

        assert!(
            shown.contains(&"左に回転 (Shift+F1)".to_string()),
            "{shown:?}"
        );
        assert!(
            shown.contains(&"右に回転 (Shift+F2)".to_string()),
            "{shown:?}"
        );
        assert!(shown.contains(&"選択解除 (Alt+Q)".to_string()), "{shown:?}");
        assert!(
            !shown.iter().any(|label| label.contains("(L)")
                || label.contains("(R)")
                || label.contains("(Ctrl+D)")),
            "既定キーが残っている: {shown:?}"
        );
    }

    /// 割り当てを解除したら、括弧ごと出さない (存在しないキーを案内しない)。
    #[test]
    fn the_menu_drops_the_suffix_when_the_action_has_no_key() {
        let mut input = input(ContextMenuItemKind::Image, ContextMenuSurface::Grid);
        input.has_checked = true;
        input.checked_count = 2;
        input.shortcuts = ContextMenuShortcutLabels::default();

        let shown = labels(&build_context_menu(&input));

        assert!(shown.contains(&"左に回転".to_string()), "{shown:?}");
        assert!(shown.contains(&"右に回転".to_string()), "{shown:?}");
        assert!(shown.contains(&"選択解除".to_string()), "{shown:?}");
    }

    fn assert_labels(input: ContextMenuInput, expected: &[&str]) {
        assert_eq!(
            labels(&build_context_menu(&input)),
            expected
                .iter()
                .map(|label| (*label).to_string())
                .collect::<Vec<_>>(),
            "kind={:?}, surface={:?}, checked={}",
            input.kind,
            input.surface,
            input.has_checked
        );
    }

    #[test]
    fn every_grid_item_kind_has_a_stable_single_item_label_set() {
        let cases = [
            (
                ContextMenuItemKind::Image,
                &[
                    "名前の変更…",
                    "パスをコピー",
                    "ファイル名をコピー",
                    "画像をクリップボードにコピー",
                    "編集内容をコピー",
                    "編集内容を貼り付け",
                    "編集内容をリセット…",
                    "左に回転 (L)",
                    "右に回転 (R)",
                    "アプリケーションで開く…",
                    "外部ツールの設定…",
                    "ゴミ箱へ移動 (タグ・評価も整理)",
                ][..],
            ),
            (
                ContextMenuItemKind::Video,
                &[
                    "名前の変更…",
                    "パスをコピー",
                    "ファイル名をコピー",
                    "左に回転 (L)",
                    "右に回転 (R)",
                    "アプリケーションで開く…",
                    "外部ツールの設定…",
                    "ゴミ箱へ移動 (タグ・評価も整理)",
                ][..],
            ),
            (
                ContextMenuItemKind::Audio,
                &[
                    "名前の変更…",
                    "パスをコピー",
                    "ファイル名をコピー",
                    "左に回転 (L)",
                    "右に回転 (R)",
                    "アプリケーションで開く…",
                    "外部ツールの設定…",
                    "ゴミ箱へ移動 (タグ・評価も整理)",
                ][..],
            ),
            (
                ContextMenuItemKind::Folder,
                &[
                    "名前の変更…",
                    "パスをコピー",
                    "ファイル名をコピー",
                    "ゴミ箱へ移動 (タグ・評価も整理)",
                ][..],
            ),
            (
                ContextMenuItemKind::ZipFile,
                &[
                    "名前の変更…",
                    "パスをコピー",
                    "ファイル名をコピー",
                    "ページを開く",
                    "一覧を開く",
                    "アプリケーションで開く…",
                    "外部ツールの設定…",
                    "ゴミ箱へ移動 (タグ・評価も整理)",
                ][..],
            ),
            (
                ContextMenuItemKind::PdfFile,
                &[
                    "名前の変更…",
                    "パスをコピー",
                    "ファイル名をコピー",
                    "ページを開く",
                    "一覧を開く",
                    "アプリケーションで開く…",
                    "外部ツールの設定…",
                    "ゴミ箱へ移動 (タグ・評価も整理)",
                ][..],
            ),
            (
                ContextMenuItemKind::ConvertibleArchive,
                &[
                    "名前の変更…",
                    "パスをコピー",
                    "ファイル名をコピー",
                    "ページを開く",
                    "一覧を開く",
                    "アプリケーションで開く…",
                    "外部ツールの設定…",
                    "ゴミ箱へ移動 (タグ・評価も整理)",
                ][..],
            ),
            (
                ContextMenuItemKind::ZipImage,
                &[
                    "パスをコピー",
                    "ファイル名をコピー",
                    "画像をクリップボードにコピー",
                    "編集内容をコピー",
                    "編集内容を貼り付け",
                    "編集内容をリセット…",
                    "左に回転 (L)",
                    "右に回転 (R)",
                    "アプリケーションで開く…",
                    "外部ツールの設定…",
                ][..],
            ),
            (
                ContextMenuItemKind::PdfPage,
                &[
                    "パスをコピー",
                    "ページ名をコピー",
                    "編集内容をコピー",
                    "編集内容を貼り付け",
                    "編集内容をリセット…",
                    "左に回転 (L)",
                    "右に回転 (R)",
                    "アプリケーションで開く…",
                    "外部ツールの設定…",
                ][..],
            ),
            (
                ContextMenuItemKind::Stack,
                &[
                    "代表画像のパスをコピー",
                    "アプリケーションで開く…",
                    "外部ツールの設定…",
                ][..],
            ),
            (ContextMenuItemKind::ZipDir, &["パスをコピー"][..]),
            (ContextMenuItemKind::SearchContainer, &["パスをコピー"][..]),
        ];
        for (kind, expected) in cases {
            assert_labels(input(kind, ContextMenuSurface::Grid), expected);
        }
    }

    #[test]
    fn surface_and_virtual_page_rotation_are_intentional() {
        let video_fs = labels(&build_context_menu(&input(
            ContextMenuItemKind::Video,
            ContextMenuSurface::Fullscreen,
        )));
        assert!(video_fs.contains(&"📌 現在のフレームを動画サムネに設定".to_string()));
        assert!(!video_fs.contains(&"左に回転 (L)".to_string()));

        for kind in [ContextMenuItemKind::ZipImage, ContextMenuItemKind::PdfPage] {
            for surface in [ContextMenuSurface::Grid, ContextMenuSurface::Fullscreen] {
                let actual = labels(&build_context_menu(&input(kind, surface)));
                assert!(actual.contains(&"左に回転 (L)".to_string()));
                assert!(actual.contains(&"右に回転 (R)".to_string()));
            }
        }
    }

    /// 貼り付け系は単一も一括も「コピー済みの編集内容があるか」だけで決まる。
    /// 片方だけ理由なしで灰色になると、なぜ押せないのかが入口ごとに変わる。
    #[test]
    fn both_paste_entries_share_one_reason_for_being_unavailable() {
        for (mut case, label) in [
            (
                input(ContextMenuItemKind::Image, ContextMenuSurface::Grid),
                "編集内容を貼り付け",
            ),
            (
                {
                    let mut checked = input(ContextMenuItemKind::Image, ContextMenuSurface::Grid);
                    checked.has_checked = true;
                    checked.checked_count = 3;
                    checked
                },
                "編集内容をまとめて貼り付け [3件]",
            ),
        ] {
            case.can_paste_edit_bundle = false;
            let node = build_context_menu(&case)
                .into_iter()
                .find(|node| matches!(node, MenuNode::Item { label: found, .. } if found == label))
                .unwrap_or_else(|| panic!("{label} が出ていない"));
            let MenuNode::Item {
                enabled,
                disabled_reason,
                ..
            } = node
            else {
                panic!("{label} は Item のはず");
            };
            assert!(!enabled, "{label}: クリップボードが空なら押せない");
            assert_eq!(
                disabled_reason.as_deref(),
                Some("コピーされた編集内容がありません"),
                "{label}: 理由は入口によらず同じ"
            );
        }
    }

    /// リセットは単一 (カーソル) でもチェック複数でも同じ入口を通る。ページ編集を
    /// 持てない種別には出さない。
    #[test]
    fn reset_is_offered_for_page_items_on_both_surfaces_and_for_a_checked_set() {
        for kind in [
            ContextMenuItemKind::Image,
            ContextMenuItemKind::ZipImage,
            ContextMenuItemKind::PdfPage,
        ] {
            for surface in [ContextMenuSurface::Grid, ContextMenuSurface::Fullscreen] {
                assert!(
                    labels(&build_context_menu(&input(kind, surface)))
                        .contains(&"編集内容をリセット…".to_string()),
                    "kind={kind:?}, surface={surface:?}"
                );
            }
        }
        for kind in [
            ContextMenuItemKind::Video,
            ContextMenuItemKind::Audio,
            ContextMenuItemKind::Folder,
            ContextMenuItemKind::ZipFile,
            ContextMenuItemKind::PdfFile,
            ContextMenuItemKind::Stack,
        ] {
            assert!(
                !labels(&build_context_menu(&input(kind, ContextMenuSurface::Grid)))
                    .contains(&"編集内容をリセット…".to_string()),
                "kind={kind:?} はページ編集を持たない"
            );
        }

        let mut checked = input(ContextMenuItemKind::Image, ContextMenuSurface::Grid);
        checked.has_checked = true;
        checked.checked_count = 4;
        assert!(
            labels(&build_context_menu(&checked))
                .contains(&"編集内容をリセット… [4件]".to_string())
        );
    }

    #[test]
    fn checked_virtual_mix_displays_the_full_selection_count() {
        let mut mixed = input(ContextMenuItemKind::ZipImage, ContextMenuSurface::Grid);
        mixed.has_checked = true;
        mixed.checked_count = 5;
        assert_labels(
            mixed,
            &[
                "選択項目のパスをコピー [5件]",
                "左に回転 (L)",
                "右に回転 (R)",
                "編集内容をまとめて貼り付け [5件]",
                "編集内容をリセット… [5件]",
                "アプリケーションで開く…",
                "外部ツールの設定…",
                "ゴミ箱へ移動 (タグ・評価も整理) [5件]",
                "選択解除 (Ctrl+D)",
            ],
        );
    }

    #[test]
    fn view_flags_control_search_history_and_pin_entries() {
        let mut search = input(
            ContextMenuItemKind::SearchContainer,
            ContextMenuSurface::Grid,
        );
        search.view.in_search = true;
        assert!(labels(&build_context_menu(&search)).contains(&"フォルダに移動".to_string()));

        let mut history = input(ContextMenuItemKind::ZipFile, ContextMenuSurface::Grid);
        history.view.reading_history = true;
        let actual = labels(&build_context_menu(&history));
        assert!(actual.contains(&"この本のフォルダに移動".to_string()));
        assert!(actual.contains(&"履歴から削除".to_string()));

        let mut pin = input(ContextMenuItemKind::Image, ContextMenuSurface::Grid);
        pin.pin = Some(ContextMenuActionState {
            label: "📌 代表サムネに固定".to_string(),
            enabled: true,
            disabled_reason: None,
        });
        assert!(labels(&build_context_menu(&pin)).contains(&"📌 代表サムネに固定".to_string()));
        for excluded in ["search", "tag", "rating", "history"] {
            let mut excluded_pin = pin.clone();
            match excluded {
                "search" => excluded_pin.view.search = true,
                "tag" => excluded_pin.view.tag = true,
                "rating" => excluded_pin.view.rating = true,
                "history" => excluded_pin.view.reading_history = true,
                _ => unreachable!(),
            }
            assert!(
                !labels(&build_context_menu(&excluded_pin))
                    .contains(&"📌 代表サムネに固定".to_string()),
                "pin must be hidden in {excluded} view"
            );
        }
    }

    #[test]
    fn major_kind_surface_checked_matrix_is_normalized_and_contains_no_removed_entries() {
        let kinds = [
            ContextMenuItemKind::Image,
            ContextMenuItemKind::Video,
            ContextMenuItemKind::Audio,
            ContextMenuItemKind::Folder,
            ContextMenuItemKind::ZipFile,
            ContextMenuItemKind::PdfFile,
            ContextMenuItemKind::ConvertibleArchive,
            ContextMenuItemKind::ZipImage,
            ContextMenuItemKind::PdfPage,
            ContextMenuItemKind::Stack,
            ContextMenuItemKind::ZipDir,
            ContextMenuItemKind::SearchContainer,
        ];
        for kind in kinds {
            for surface in [ContextMenuSurface::Grid, ContextMenuSurface::Fullscreen] {
                for has_checked in [false, true] {
                    if has_checked && surface == ContextMenuSurface::Fullscreen {
                        continue;
                    }
                    let mut case = input(kind, surface);
                    case.has_checked = has_checked;
                    case.checked_count = usize::from(has_checked) * 3;
                    let nodes = build_context_menu(&case);
                    assert_eq!(nodes, normalize_menu(nodes.clone()));
                    let actual = labels(&nodes);
                    assert!(!actual.iter().any(|label| label.contains("旧XMP")));
                    assert!(!actual.iter().any(|label| label.contains("最近使った")));
                }
            }
        }
    }

    #[test]
    fn folder_background_and_open_with_tree_use_unified_ellipsis() {
        let mut folder = input(ContextMenuItemKind::Folder, ContextMenuSurface::Grid);
        folder.is_folder_context = true;
        folder.can_use_folder_commands = true;
        assert_labels(
            folder,
            &["新しいフォルダ…", "貼り付け", "このフォルダのパスをコピー"],
        );

        let image = labels(&build_context_menu(&input(
            ContextMenuItemKind::Image,
            ContextMenuSurface::Grid,
        )));
        assert!(image.contains(&"名前の変更…".to_string()));
        assert!(!image.contains(&"アプリケーションを追加…".to_string()));
        assert!(image.contains(&"外部ツールの設定…".to_string()));
    }

    #[test]
    fn removed_entries_never_appear() {
        for kind in [
            ContextMenuItemKind::Image,
            ContextMenuItemKind::Video,
            ContextMenuItemKind::ZipImage,
            ContextMenuItemKind::PdfPage,
        ] {
            let actual = labels(&build_context_menu(&input(kind, ContextMenuSurface::Grid)));
            assert!(!actual.iter().any(|label| label.contains("最近使った")));
            assert!(!actual.iter().any(|label| label.contains("旧XMP")));
            assert!(
                !actual
                    .iter()
                    .any(|label| label.contains("アプリケーションを追加"))
            );
        }
    }

    #[test]
    fn normalization_removes_empty_submenus_and_bad_separators() {
        let nodes = normalize_menu(vec![
            MenuNode::Separator,
            MenuNode::Submenu {
                label: "empty".to_string(),
                children: vec![MenuNode::Separator],
            },
            item(MenuCommand::CopyPath, "copy"),
            MenuNode::Separator,
            MenuNode::Separator,
        ]);
        assert_eq!(nodes, vec![item(MenuCommand::CopyPath, "copy")]);
    }
}
