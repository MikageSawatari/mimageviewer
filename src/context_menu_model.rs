//! Right-click menu definition shared by the native Win32 and egui renderers.
//!
//! This module owns only pure data and predicates. Callers snapshot any App state
//! (pin state, associated applications, clipboard availability, and view flags)
//! before calling [`build_context_menu`].

use crate::external_tool::ExternalToolId;
use crate::grid_item::{GridItem, checked_virtual_selection_message};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContextMenuParentId {
    Root,
    OpenWith,
}

impl ContextMenuParentId {
    pub const ALL: &'static [Self] = &[Self::Root, Self::OpenWith];

    pub const fn stable_name(self) -> &'static str {
        match self {
            Self::Root => "Root",
            Self::OpenWith => "OpenWith",
        }
    }

    pub fn parse_stable_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|parent| parent.stable_name() == name)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Root => "最上位の項目",
            Self::OpenWith => "「アプリケーションで開く…」内の項目",
        }
    }
}

/// 右クリックメニューで利用者が表示・順序を設定できる静的項目の永続 ID。
///
/// 表示名や実行時 payload には依存しない。外部ツール、関連付けアプリ、
/// Open With サブメニュー、Windows Shell は固定枠であり、この catalog へ含めない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContextMenuItemId {
    CutFiles,
    CopyFiles,
    NewFolder,
    Paste,
    Rename,
    CopyRepresentativePath,
    CopyPath,
    CopyFileName,
    CopyPageName,
    CopyImageToClipboard,
    CopyEditBundle,
    PasteEditBundle,
    BulkPasteEditBundle,
    ResetPageEdits,
    OpenContainerAsPage,
    OpenContainerAsList,
    JumpToFolder,
    JumpToBookFolder,
    RotateLeft,
    RotateRight,
    SetCurrentVideoFrameThumbnail,
    ToggleRepresentativeThumb,
    OpenFolderInExplorer,
    OpenExternalToolSettings,
    MoveToRecycleBin,
    RemoveReadingHistory,
    Deselect,
}

impl ContextMenuItemId {
    pub const ALL: &'static [Self] = &[
        Self::CutFiles,
        Self::CopyFiles,
        Self::NewFolder,
        Self::Paste,
        Self::Rename,
        Self::CopyRepresentativePath,
        Self::CopyPath,
        Self::CopyFileName,
        Self::CopyPageName,
        Self::CopyImageToClipboard,
        Self::CopyEditBundle,
        Self::PasteEditBundle,
        Self::BulkPasteEditBundle,
        Self::ResetPageEdits,
        Self::OpenContainerAsPage,
        Self::OpenContainerAsList,
        Self::JumpToFolder,
        Self::JumpToBookFolder,
        Self::RotateLeft,
        Self::RotateRight,
        Self::SetCurrentVideoFrameThumbnail,
        Self::ToggleRepresentativeThumb,
        Self::OpenFolderInExplorer,
        Self::OpenExternalToolSettings,
        Self::MoveToRecycleBin,
        Self::RemoveReadingHistory,
        Self::Deselect,
    ];

    pub const fn stable_name(self) -> &'static str {
        match self {
            Self::CutFiles => "CutFiles",
            Self::CopyFiles => "CopyFiles",
            Self::NewFolder => "NewFolder",
            Self::Paste => "Paste",
            Self::Rename => "Rename",
            Self::CopyRepresentativePath => "CopyRepresentativePath",
            Self::CopyPath => "CopyPath",
            Self::CopyFileName => "CopyFileName",
            Self::CopyPageName => "CopyPageName",
            Self::CopyImageToClipboard => "CopyImageToClipboard",
            Self::CopyEditBundle => "CopyEditBundle",
            Self::PasteEditBundle => "PasteEditBundle",
            Self::BulkPasteEditBundle => "BulkPasteEditBundle",
            Self::ResetPageEdits => "ResetPageEdits",
            Self::OpenContainerAsPage => "OpenContainerAsPage",
            Self::OpenContainerAsList => "OpenContainerAsList",
            Self::JumpToFolder => "JumpToFolder",
            Self::JumpToBookFolder => "JumpToBookFolder",
            Self::RotateLeft => "RotateLeft",
            Self::RotateRight => "RotateRight",
            Self::SetCurrentVideoFrameThumbnail => "SetCurrentVideoFrameThumbnail",
            Self::ToggleRepresentativeThumb => "ToggleRepresentativeThumb",
            Self::OpenFolderInExplorer => "OpenFolderInExplorer",
            Self::OpenExternalToolSettings => "OpenExternalToolSettings",
            Self::MoveToRecycleBin => "MoveToRecycleBin",
            Self::RemoveReadingHistory => "RemoveReadingHistory",
            Self::Deselect => "Deselect",
        }
    }

    pub fn parse_stable_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|item| item.stable_name() == name)
    }

    pub const fn parent(self) -> ContextMenuParentId {
        match self {
            Self::OpenExternalToolSettings => ContextMenuParentId::OpenWith,
            _ => ContextMenuParentId::Root,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::CutFiles => "切り取り",
            Self::CopyFiles => "コピー",
            Self::NewFolder => "新しいフォルダ…",
            Self::Paste => "貼り付け",
            Self::Rename => "名前の変更…",
            Self::CopyRepresentativePath => "代表画像のパスをコピー",
            Self::CopyPath => "パスをコピー",
            Self::CopyFileName => "ファイル名をコピー",
            Self::CopyPageName => "ページ名をコピー",
            Self::CopyImageToClipboard => "画像をクリップボードにコピー",
            Self::CopyEditBundle => "編集内容をコピー",
            Self::PasteEditBundle => "編集内容を貼り付け",
            Self::BulkPasteEditBundle => "編集内容をまとめて貼り付け",
            Self::ResetPageEdits => "編集内容をリセット…",
            Self::OpenContainerAsPage => "ページを開く",
            Self::OpenContainerAsList => "一覧を開く",
            Self::JumpToFolder => "フォルダに移動",
            Self::JumpToBookFolder => "この本のフォルダに移動",
            Self::RotateLeft => "左に回転",
            Self::RotateRight => "右に回転",
            Self::SetCurrentVideoFrameThumbnail => "現在のフレームを動画サムネに設定",
            Self::ToggleRepresentativeThumb => "代表サムネに固定 / 解除",
            Self::OpenFolderInExplorer => "このフォルダをエクスプローラで開く",
            Self::OpenExternalToolSettings => "外部ツールの設定…",
            Self::MoveToRecycleBin => "ゴミ箱へ移動 (タグ・評価も整理)",
            Self::RemoveReadingHistory => "履歴から削除",
            Self::Deselect => "選択解除",
        }
    }

    fn from_command(command: &MenuCommand) -> Option<Self> {
        Some(match command {
            MenuCommand::NewFolder => Self::NewFolder,
            MenuCommand::Paste => Self::Paste,
            MenuCommand::Rename => Self::Rename,
            MenuCommand::CutFiles => Self::CutFiles,
            MenuCommand::CopyFiles => Self::CopyFiles,
            MenuCommand::CopyPath => Self::CopyPath,
            MenuCommand::CopyFileName => Self::CopyFileName,
            MenuCommand::CopyPageName => Self::CopyPageName,
            MenuCommand::CopyRepresentativePath => Self::CopyRepresentativePath,
            MenuCommand::CopyImageToClipboard => Self::CopyImageToClipboard,
            MenuCommand::CopyEditBundle => Self::CopyEditBundle,
            MenuCommand::PasteEditBundle => Self::PasteEditBundle,
            MenuCommand::BulkPasteEditBundle => Self::BulkPasteEditBundle,
            MenuCommand::ResetPageEdits => Self::ResetPageEdits,
            MenuCommand::JumpToFolder => Self::JumpToFolder,
            MenuCommand::JumpToBookFolder => Self::JumpToBookFolder,
            MenuCommand::OpenContainerAsPage => Self::OpenContainerAsPage,
            MenuCommand::OpenContainerAsList => Self::OpenContainerAsList,
            MenuCommand::RotateLeft => Self::RotateLeft,
            MenuCommand::RotateRight => Self::RotateRight,
            MenuCommand::ToggleRepresentativeThumb => Self::ToggleRepresentativeThumb,
            MenuCommand::SetCurrentVideoFrameThumbnail => Self::SetCurrentVideoFrameThumbnail,
            MenuCommand::OpenFolderInExplorer => Self::OpenFolderInExplorer,
            MenuCommand::OpenExternalToolSettings => Self::OpenExternalToolSettings,
            MenuCommand::MoveToRecycleBin => Self::MoveToRecycleBin,
            MenuCommand::Deselect => Self::Deselect,
            MenuCommand::RemoveReadingHistory => Self::RemoveReadingHistory,
            MenuCommand::ExternalTool(_) | MenuCommand::OpenWithAssociation { .. } => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenuOrderSettings {
    pub parent: String,
    #[serde(default)]
    pub items: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenuLayoutSettings {
    #[serde(default)]
    pub order: Vec<ContextMenuOrderSettings>,
    #[serde(default)]
    pub hidden_items: Vec<String>,
}

impl ContextMenuLayoutSettings {
    pub fn canonical_order(parent: ContextMenuParentId) -> Vec<ContextMenuItemId> {
        ContextMenuItemId::ALL
            .iter()
            .copied()
            .filter(|item| item.parent() == parent)
            .collect()
    }

    pub fn resolved_order(&self, parent: ContextMenuParentId) -> Vec<ContextMenuItemId> {
        let mut resolved = Self::canonical_order(parent);
        let Some(saved) = self
            .order
            .iter()
            .find(|order| ContextMenuParentId::parse_stable_name(&order.parent) == Some(parent))
        else {
            return resolved;
        };
        let mut explicit = Vec::new();
        for name in &saved.items {
            let Some(item) = ContextMenuItemId::parse_stable_name(name) else {
                continue;
            };
            if item.parent() == parent && !explicit.contains(&item) {
                explicit.push(item);
            }
        }
        if explicit.is_empty() {
            return resolved;
        }

        // 既知 ID が canonical 配列で占める slot だけを、保存済み順で置き換える。
        // これなら既存 ID の相対順を維持しつつ、将来追加された ID は既定位置に残る。
        let explicit_slots: Vec<_> = resolved
            .iter()
            .enumerate()
            .filter_map(|(index, item)| explicit.contains(item).then_some(index))
            .collect();
        for (slot, item) in explicit_slots.into_iter().zip(explicit) {
            resolved[slot] = item;
        }
        resolved
    }

    pub fn has_explicit_order(&self, parent: ContextMenuParentId) -> bool {
        self.order.iter().any(|order| {
            ContextMenuParentId::parse_stable_name(&order.parent) == Some(parent)
                && order.items.iter().any(|name| {
                    ContextMenuItemId::parse_stable_name(name)
                        .is_some_and(|item| item.parent() == parent)
                })
        })
    }

    pub fn is_visible(&self, item: ContextMenuItemId) -> bool {
        !self.hidden_items.iter().any(|name| {
            ContextMenuItemId::parse_stable_name(name).is_some_and(|hidden| hidden == item)
        })
    }

    pub fn set_order(&mut self, parent: ContextMenuParentId, items: &[ContextMenuItemId]) {
        self.order
            .retain(|entry| ContextMenuParentId::parse_stable_name(&entry.parent) != Some(parent));
        self.order.push(ContextMenuOrderSettings {
            parent: parent.stable_name().to_string(),
            items: items
                .iter()
                .copied()
                .filter(|item| item.parent() == parent)
                .map(|item| item.stable_name().to_string())
                .collect(),
        });
    }

    pub fn set_visible(&mut self, item: ContextMenuItemId, visible: bool) {
        self.hidden_items.retain(|name| {
            ContextMenuItemId::parse_stable_name(name).is_none_or(|hidden| hidden != item)
        });
        if !visible {
            self.hidden_items.push(item.stable_name().to_string());
        }
    }

    pub fn show_all(&mut self) {
        self.hidden_items.clear();
    }
}

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
    CutFiles,
    CopyFiles,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CheckedFileOperationSelection {
    #[default]
    Empty,
    RealOnly,
    VirtualOnly,
    Mixed,
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
    pub checked_file_operation_selection: CheckedFileOperationSelection,
    pub can_use_folder_commands: bool,
    pub can_paste_edit_bundle: bool,
    pub has_explorer_folder: bool,
    pub view: ContextMenuViewFlags,
    pub pin: Option<ContextMenuActionState>,
    pub external_tools: Vec<ExternalToolMenuEntry>,
    pub associated_apps: Vec<AssociatedAppMenuEntry>,
    pub shortcuts: ContextMenuShortcutLabels,
    pub layout: ContextMenuLayoutSettings,
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
    pub cut: Option<String>,
    pub copy: Option<String>,
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
        children: apply_context_menu_layout(children, ContextMenuParentId::OpenWith, &input.layout),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextMenuFixedSlotId {
    ExternalTools,
    OpenWithSubmenu,
    OpenWithAssociations,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayoutUnitId {
    Configurable(ContextMenuItemId),
    Fixed(ContextMenuFixedSlotId),
}

#[derive(Debug)]
struct LayoutUnit {
    id: LayoutUnitId,
    section: usize,
    last_section: usize,
    nodes: Vec<MenuNode>,
}

fn menu_node_layout_id(node: &MenuNode) -> LayoutUnitId {
    match node {
        MenuNode::Item {
            command: MenuCommand::ExternalTool(_),
            ..
        } => LayoutUnitId::Fixed(ContextMenuFixedSlotId::ExternalTools),
        MenuNode::Item {
            command: MenuCommand::OpenWithAssociation { .. },
            ..
        } => LayoutUnitId::Fixed(ContextMenuFixedSlotId::OpenWithAssociations),
        MenuNode::Item { command, .. } => LayoutUnitId::Configurable(
            ContextMenuItemId::from_command(command).expect("static context-menu command"),
        ),
        MenuNode::Submenu { .. } => LayoutUnitId::Fixed(ContextMenuFixedSlotId::OpenWithSubmenu),
        MenuNode::Separator => unreachable!("separators are handled before unit classification"),
    }
}

/// Capability-filtered tree と保存済み layout の積を、renderer 共通の MenuNode 列へ解決する。
fn apply_context_menu_layout(
    nodes: Vec<MenuNode>,
    parent: ContextMenuParentId,
    settings: &ContextMenuLayoutSettings,
) -> Vec<MenuNode> {
    let nodes = normalize_menu(nodes);
    let mut units: Vec<LayoutUnit> = Vec::new();
    let mut section = 0usize;
    for node in nodes {
        if matches!(node, MenuNode::Separator) {
            section += 1;
            continue;
        }
        let id = menu_node_layout_id(&node);
        if let LayoutUnitId::Configurable(item) = id {
            debug_assert_eq!(item.parent(), parent);
        }
        if let Some(existing) = units.iter_mut().find(|unit| unit.id == id) {
            // Fixed dynamic groups can contain their own separators (associated apps:
            // recommended / others). Keep that internal structure inside the fixed slot.
            if existing.last_section != section {
                existing.nodes.push(MenuNode::Separator);
            }
            existing.nodes.push(node);
            existing.last_section = section;
        } else {
            units.push(LayoutUnit {
                id,
                section,
                last_section: section,
                nodes: vec![node],
            });
        }
    }

    units.retain(|unit| match unit.id {
        LayoutUnitId::Configurable(item) => settings.is_visible(item),
        LayoutUnitId::Fixed(_) => true,
    });
    if settings.has_explicit_order(parent) {
        let order = settings.resolved_order(parent);
        let mut configurable: Vec<_> = units
            .iter()
            .filter_map(|unit| match unit.id {
                LayoutUnitId::Configurable(item) => Some((
                    order
                        .iter()
                        .position(|candidate| *candidate == item)
                        .unwrap_or(usize::MAX),
                    unit.nodes.clone(),
                )),
                LayoutUnitId::Fixed(_) => None,
            })
            .collect();
        configurable.sort_by_key(|(rank, _)| *rank);
        let mut configurable = configurable.into_iter().map(|(_, nodes)| nodes);
        for unit in &mut units {
            if matches!(unit.id, LayoutUnitId::Configurable(_)) {
                unit.nodes = configurable.next().expect("same configurable unit count");
            }
        }
    }

    let mut resolved = Vec::new();
    let mut previous_section = None;
    for unit in units {
        if previous_section.is_some_and(|previous| previous != unit.section) {
            resolved.push(MenuNode::Separator);
        }
        previous_section = Some(unit.section);
        resolved.extend(unit.nodes);
    }
    normalize_menu(resolved)
}

/// Build the complete mIV context-menu tree from an immutable snapshot.
pub fn build_context_menu(input: &ContextMenuInput) -> Vec<MenuNode> {
    let mut nodes = Vec::new();

    if input.has_checked {
        let file_clipboard_enabled =
            input.checked_file_operation_selection == CheckedFileOperationSelection::RealOnly;
        let cut_label = format!("切り取り [{}件]", input.checked_count);
        let copy_label = format!("コピー [{}件]", input.checked_count);
        let file_clipboard_disabled_reason = if file_clipboard_enabled {
            None
        } else {
            Some(checked_virtual_selection_message("コピー / カット"))
        };
        push_group(
            &mut nodes,
            [
                MenuNode::Item {
                    command: MenuCommand::CutFiles,
                    label: with_key(&cut_label, input.shortcuts.cut.as_ref()),
                    enabled: file_clipboard_enabled,
                    disabled_reason: file_clipboard_disabled_reason.clone(),
                },
                MenuNode::Item {
                    command: MenuCommand::CopyFiles,
                    label: with_key(&copy_label, input.shortcuts.copy.as_ref()),
                    enabled: file_clipboard_enabled,
                    disabled_reason: file_clipboard_disabled_reason,
                },
            ],
        );
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
        return apply_context_menu_layout(nodes, ContextMenuParentId::Root, &input.layout);
    }

    if !input.is_folder_context && input.kind.is_real_item() {
        push_group(
            &mut nodes,
            [
                item(
                    MenuCommand::CutFiles,
                    with_key("切り取り", input.shortcuts.cut.as_ref()),
                ),
                item(
                    MenuCommand::CopyFiles,
                    with_key("コピー", input.shortcuts.copy.as_ref()),
                ),
            ],
        );
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

    apply_context_menu_layout(nodes, ContextMenuParentId::Root, &input.layout)
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
            checked_file_operation_selection: CheckedFileOperationSelection::Empty,
            can_use_folder_commands: false,
            can_paste_edit_bundle: false,
            has_explorer_folder: false,
            view: ContextMenuViewFlags::default(),
            pin: None,
            external_tools: Vec::new(),
            associated_apps: Vec::new(),
            // 既存の期待値と揃うよう、既定の割り当てを解決した結果を渡す。
            shortcuts: ContextMenuShortcutLabels {
                cut: (surface == ContextMenuSurface::Grid).then(|| "Ctrl+X".to_string()),
                copy: (surface == ContextMenuSurface::Grid).then(|| "Ctrl+C".to_string()),
                rotate_left: Some("L".to_string()),
                rotate_right: Some("R".to_string()),
                deselect: Some("Ctrl+D".to_string()),
            },
            layout: ContextMenuLayoutSettings::default(),
        }
    }

    fn labels(nodes: &[MenuNode]) -> Vec<String> {
        let mut out = Vec::new();
        fn visit(nodes: &[MenuNode], out: &mut Vec<String>) {
            for node in nodes {
                match node {
                    MenuNode::Item { label, .. } => out.push(label.clone()),
                    MenuNode::Submenu {
                        label, children, ..
                    } => {
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

    fn command_state(
        nodes: &[MenuNode],
        expected: MenuCommand,
    ) -> Option<(String, bool, Option<String>)> {
        for node in nodes {
            match node {
                MenuNode::Item {
                    command,
                    label,
                    enabled,
                    disabled_reason,
                } if *command == expected => {
                    return Some((label.clone(), *enabled, disabled_reason.clone()));
                }
                MenuNode::Submenu { children, .. } => {
                    if let Some(found) = command_state(children, expected.clone()) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn root_layout_ids(nodes: &[MenuNode]) -> Vec<ContextMenuItemId> {
        nodes
            .iter()
            .filter_map(|node| match node {
                MenuNode::Separator => None,
                _ => match menu_node_layout_id(node) {
                    LayoutUnitId::Configurable(item) => Some(item),
                    LayoutUnitId::Fixed(_) => None,
                },
            })
            .collect()
    }

    fn menu_shape(nodes: &[MenuNode]) -> Vec<String> {
        nodes
            .iter()
            .map(|node| match node {
                MenuNode::Separator => "|".to_string(),
                MenuNode::Item { command, .. } => ContextMenuItemId::from_command(command)
                    .map(|id| id.stable_name().to_string())
                    .unwrap_or_else(|| match command {
                        MenuCommand::ExternalTool(_) => "<external-tools>".to_string(),
                        MenuCommand::OpenWithAssociation { .. } => "<associated-apps>".to_string(),
                        _ => unreachable!(),
                    }),
                MenuNode::Submenu { children, .. } => {
                    format!("<open-with>[{}]", menu_shape(children).join(","))
                }
            })
            .collect()
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
        input.checked_file_operation_selection = CheckedFileOperationSelection::RealOnly;
        input.shortcuts = ContextMenuShortcutLabels {
            cut: Some("Alt+X".to_string()),
            copy: Some("Alt+C".to_string()),
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
            shown.contains(&"切り取り [2件] (Alt+X)".to_string()),
            "{shown:?}"
        );
        assert!(
            shown.contains(&"コピー [2件] (Alt+C)".to_string()),
            "{shown:?}"
        );
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
        input.checked_file_operation_selection = CheckedFileOperationSelection::RealOnly;
        input.shortcuts = ContextMenuShortcutLabels::default();

        let shown = labels(&build_context_menu(&input));

        assert!(shown.contains(&"左に回転".to_string()), "{shown:?}");
        assert!(shown.contains(&"右に回転".to_string()), "{shown:?}");
        assert!(shown.contains(&"選択解除".to_string()), "{shown:?}");
        assert!(shown.contains(&"切り取り [2件]".to_string()), "{shown:?}");
        assert!(shown.contains(&"コピー [2件]".to_string()), "{shown:?}");
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
                    "切り取り (Ctrl+X)",
                    "コピー (Ctrl+C)",
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
                    "切り取り (Ctrl+X)",
                    "コピー (Ctrl+C)",
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
                    "切り取り (Ctrl+X)",
                    "コピー (Ctrl+C)",
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
                    "切り取り (Ctrl+X)",
                    "コピー (Ctrl+C)",
                    "名前の変更…",
                    "パスをコピー",
                    "ファイル名をコピー",
                    "ゴミ箱へ移動 (タグ・評価も整理)",
                ][..],
            ),
            (
                ContextMenuItemKind::ZipFile,
                &[
                    "切り取り (Ctrl+X)",
                    "コピー (Ctrl+C)",
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
                    "切り取り (Ctrl+X)",
                    "コピー (Ctrl+C)",
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
                    "切り取り (Ctrl+X)",
                    "コピー (Ctrl+C)",
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
                    checked.checked_file_operation_selection =
                        CheckedFileOperationSelection::RealOnly;
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
        checked.checked_file_operation_selection = CheckedFileOperationSelection::RealOnly;
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
        mixed.checked_file_operation_selection = CheckedFileOperationSelection::Mixed;
        assert_labels(
            mixed,
            &[
                "切り取り [5件] (Ctrl+X)",
                "コピー [5件] (Ctrl+C)",
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
    fn real_single_items_offer_file_cut_and_copy_first_on_both_surfaces() {
        let real_kinds = [
            ContextMenuItemKind::Image,
            ContextMenuItemKind::Video,
            ContextMenuItemKind::Audio,
            ContextMenuItemKind::Folder,
            ContextMenuItemKind::ZipFile,
            ContextMenuItemKind::PdfFile,
            ContextMenuItemKind::ConvertibleArchive,
        ];
        for kind in real_kinds {
            for surface in [ContextMenuSurface::Grid, ContextMenuSurface::Fullscreen] {
                let nodes = build_context_menu(&input(kind, surface));
                let expected_labels = if surface == ContextMenuSurface::Grid {
                    ["切り取り (Ctrl+X)", "コピー (Ctrl+C)"]
                } else {
                    ["切り取り", "コピー"]
                };
                for (index, (command, label)) in [
                    (MenuCommand::CutFiles, expected_labels[0]),
                    (MenuCommand::CopyFiles, expected_labels[1]),
                ]
                .into_iter()
                .enumerate()
                {
                    assert_eq!(
                        nodes.get(index),
                        Some(&MenuNode::Item {
                            command,
                            label: label.to_string(),
                            enabled: true,
                            disabled_reason: None,
                        }),
                        "kind={kind:?}, surface={surface:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn checked_cut_and_copy_use_count_and_real_virtual_classification() {
        let mut real = input(ContextMenuItemKind::Image, ContextMenuSurface::Grid);
        real.has_checked = true;
        real.checked_count = 3;
        real.checked_file_operation_selection = CheckedFileOperationSelection::RealOnly;
        for (command, expected_label) in [
            (MenuCommand::CutFiles, "切り取り [3件] (Ctrl+X)"),
            (MenuCommand::CopyFiles, "コピー [3件] (Ctrl+C)"),
        ] {
            assert_eq!(
                command_state(&build_context_menu(&real), command),
                Some((expected_label.to_string(), true, None))
            );
        }

        let reason = checked_virtual_selection_message("コピー / カット");
        for selection in [
            CheckedFileOperationSelection::Mixed,
            CheckedFileOperationSelection::VirtualOnly,
        ] {
            let mut unavailable = real.clone();
            unavailable.checked_file_operation_selection = selection;
            for command in [MenuCommand::CutFiles, MenuCommand::CopyFiles] {
                let (_, enabled, disabled_reason) =
                    command_state(&build_context_menu(&unavailable), command).unwrap();
                assert!(!enabled, "selection={selection:?}");
                assert_eq!(disabled_reason.as_deref(), Some(reason.as_str()));
            }
        }
    }

    #[test]
    fn virtual_single_items_and_folder_background_hide_file_cut_and_copy() {
        for kind in [
            ContextMenuItemKind::ZipImage,
            ContextMenuItemKind::PdfPage,
            ContextMenuItemKind::ZipDir,
            ContextMenuItemKind::Stack,
            ContextMenuItemKind::SearchContainer,
        ] {
            let nodes = build_context_menu(&input(kind, ContextMenuSurface::Grid));
            assert!(command_state(&nodes, MenuCommand::CutFiles).is_none());
            assert!(command_state(&nodes, MenuCommand::CopyFiles).is_none());
        }

        let mut background = input(ContextMenuItemKind::Folder, ContextMenuSurface::Grid);
        background.is_folder_context = true;
        background.can_use_folder_commands = true;
        let nodes = build_context_menu(&background);
        assert!(command_state(&nodes, MenuCommand::CutFiles).is_none());
        assert!(command_state(&nodes, MenuCommand::CopyFiles).is_none());
        assert!(command_state(&nodes, MenuCommand::Paste).is_some());
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
                    if has_checked {
                        case.checked_file_operation_selection = if kind.is_real_item() {
                            CheckedFileOperationSelection::RealOnly
                        } else {
                            CheckedFileOperationSelection::VirtualOnly
                        };
                    }
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

    #[test]
    fn default_layout_preserves_the_capability_filtered_tree_exactly() {
        let raw = vec![
            item(MenuCommand::CutFiles, "cut"),
            item(MenuCommand::CopyFiles, "copy"),
            MenuNode::Separator,
            item(MenuCommand::Rename, "rename"),
        ];
        assert_eq!(
            apply_context_menu_layout(
                raw.clone(),
                ContextMenuParentId::Root,
                &ContextMenuLayoutSettings::default()
            ),
            raw
        );
    }

    #[test]
    fn default_layout_preserves_representative_menu_sections_exactly() {
        let grid_image =
            build_context_menu(&input(ContextMenuItemKind::Image, ContextMenuSurface::Grid));
        let fullscreen_video = build_context_menu(&input(
            ContextMenuItemKind::Video,
            ContextMenuSurface::Fullscreen,
        ));
        let mut checked = input(ContextMenuItemKind::Image, ContextMenuSurface::Grid);
        checked.has_checked = true;
        checked.checked_count = 2;
        checked.checked_file_operation_selection = CheckedFileOperationSelection::RealOnly;

        assert_eq!(
            menu_shape(&grid_image),
            [
                "CutFiles",
                "CopyFiles",
                "|",
                "Rename",
                "|",
                "CopyPath",
                "CopyFileName",
                "CopyImageToClipboard",
                "CopyEditBundle",
                "PasteEditBundle",
                "|",
                "ResetPageEdits",
                "|",
                "RotateLeft",
                "RotateRight",
                "|",
                "<open-with>[OpenExternalToolSettings]",
                "|",
                "MoveToRecycleBin",
            ]
        );
        assert_eq!(
            menu_shape(&fullscreen_video),
            [
                "CutFiles",
                "CopyFiles",
                "|",
                "Rename",
                "|",
                "CopyPath",
                "CopyFileName",
                "|",
                "SetCurrentVideoFrameThumbnail",
                "|",
                "<open-with>[OpenExternalToolSettings]",
            ]
        );
        assert_eq!(
            menu_shape(&build_context_menu(&checked)),
            [
                "CutFiles",
                "CopyFiles",
                "|",
                "CopyPath",
                "|",
                "RotateLeft",
                "RotateRight",
                "|",
                "BulkPasteEditBundle",
                "ResetPageEdits",
                "|",
                "<open-with>[OpenExternalToolSettings]",
                "|",
                "MoveToRecycleBin",
                "|",
                "Deselect",
            ]
        );
    }

    #[test]
    fn missing_catalog_items_keep_their_canonical_slots_without_reordering_known_ids() {
        let canonical = ContextMenuLayoutSettings::canonical_order(ContextMenuParentId::Root);
        let mut saved: Vec<_> = canonical
            .iter()
            .copied()
            .filter(|item| *item != ContextMenuItemId::NewFolder)
            .collect();
        saved.swap(0, 1);
        let settings = ContextMenuLayoutSettings {
            order: vec![ContextMenuOrderSettings {
                parent: "Root".to_string(),
                items: saved
                    .iter()
                    .map(|item| item.stable_name().to_string())
                    .chain([
                        "FutureItem".to_string(),
                        ContextMenuItemId::CopyFiles.stable_name().to_string(),
                        "OpenWithAssociations".to_string(),
                    ])
                    .collect(),
            }],
            hidden_items: Vec::new(),
        };

        let resolved = settings.resolved_order(ContextMenuParentId::Root);
        assert_eq!(resolved[0], ContextMenuItemId::CopyFiles);
        assert_eq!(resolved[1], ContextMenuItemId::CutFiles);
        assert_eq!(resolved[2], ContextMenuItemId::NewFolder);
        let known_without_new: Vec<_> = resolved
            .iter()
            .copied()
            .filter(|item| *item != ContextMenuItemId::NewFolder)
            .collect();
        assert_eq!(known_without_new, saved);
    }

    #[test]
    fn custom_layout_reorders_only_available_items_on_grid_and_fullscreen() {
        let mut layout = ContextMenuLayoutSettings::default();
        let mut order = layout.resolved_order(ContextMenuParentId::Root);
        let right = order
            .iter()
            .position(|item| *item == ContextMenuItemId::RotateRight)
            .unwrap();
        let rotate_right = order.remove(right);
        order.insert(0, rotate_right);
        layout.set_order(ContextMenuParentId::Root, &order);
        layout.set_visible(ContextMenuItemId::CopyPath, false);

        for surface in [ContextMenuSurface::Grid, ContextMenuSurface::Fullscreen] {
            let mut case = input(ContextMenuItemKind::Image, surface);
            case.layout = layout.clone();
            let nodes = build_context_menu(&case);
            let ids = root_layout_ids(&nodes);
            assert_eq!(ids.first(), Some(&ContextMenuItemId::RotateRight));
            assert!(!ids.contains(&ContextMenuItemId::CopyPath));
            assert!(ids.contains(&ContextMenuItemId::RotateLeft));
            assert!(
                !ids.contains(&ContextMenuItemId::NewFolder),
                "layout must not resurrect an unavailable capability: {ids:?}"
            );
        }
    }

    #[test]
    fn dynamic_groups_keep_their_fixed_slots_and_internal_order() {
        let mut case = input(ContextMenuItemKind::Image, ContextMenuSurface::Grid);
        case.external_tools = vec![
            ExternalToolMenuEntry {
                tool_id: ExternalToolId(10),
                label: "tool-a".to_string(),
                enabled: true,
                disabled_reason: None,
            },
            ExternalToolMenuEntry {
                tool_id: ExternalToolId(11),
                label: "tool-b".to_string(),
                enabled: true,
                disabled_reason: None,
            },
        ];
        case.associated_apps = vec![
            AssociatedAppMenuEntry {
                display_name: "recommended".to_string(),
                handler_id: "recommended.app".to_string(),
                is_recommended: true,
            },
            AssociatedAppMenuEntry {
                display_name: "other".to_string(),
                handler_id: "other.app".to_string(),
                is_recommended: false,
            },
        ];
        let default_nodes = build_context_menu(&case);
        let default_labels = labels(&default_nodes);
        let fixed_positions = |labels: &[String]| {
            ["tool-a", "tool-b", "アプリケーションで開く…"]
                .map(|needle| labels.iter().position(|label| label == needle).unwrap())
        };

        let mut order = case.layout.resolved_order(ContextMenuParentId::Root);
        let rotate = order
            .iter()
            .position(|item| *item == ContextMenuItemId::RotateRight)
            .unwrap();
        let rotate = order.remove(rotate);
        order.insert(0, rotate);
        case.layout.set_order(ContextMenuParentId::Root, &order);

        let nodes = build_context_menu(&case);
        let shown = labels(&nodes);
        assert_eq!(fixed_positions(&shown), fixed_positions(&default_labels));
        assert!(
            shown.windows(2).any(|pair| pair == ["tool-a", "tool-b"]),
            "external tools keep runtime order: {shown:?}"
        );
        let MenuNode::Submenu { children, .. } = nodes
            .iter()
            .find(|node| matches!(node, MenuNode::Submenu { .. }))
            .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(
            labels(children),
            ["recommended", "other", "外部ツールの設定…"]
        );
        assert_eq!(
            children
                .iter()
                .filter(|node| matches!(node, MenuNode::Separator))
                .count(),
            2,
            "recommended/other/settings boundaries stay inside the fixed submenu"
        );
    }

    #[test]
    fn fixed_dynamic_groups_are_not_configurable_or_hidden_by_old_or_unknown_ids() {
        let mut case = input(ContextMenuItemKind::Image, ContextMenuSurface::Grid);
        case.associated_apps = vec![AssociatedAppMenuEntry {
            display_name: "viewer".to_string(),
            handler_id: "viewer.app".to_string(),
            is_recommended: true,
        }];
        case.layout.hidden_items.extend([
            "ExternalTools".to_string(),
            "OpenWithAssociations".to_string(),
            "OpenWithSubmenu".to_string(),
        ]);
        case.layout
            .set_visible(ContextMenuItemId::OpenExternalToolSettings, false);

        let nodes = build_context_menu(&case);
        let MenuNode::Submenu { children, .. } = nodes
            .iter()
            .find(|node| matches!(node, MenuNode::Submenu { .. }))
            .expect("fixed Open With slot remains while an association is available")
        else {
            unreachable!()
        };
        assert!(
            labels(children).contains(&"viewer".to_string()),
            "dynamic association cannot be hidden by settings: {nodes:?}"
        );
        assert_eq!(nodes, normalize_menu(nodes.clone()));
    }
}
