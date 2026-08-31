use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use eframe::egui;

pub const DEFAULT_PDF_RENDER_LONG_EDGE: u32 = 4096;
const CONTEXT_MENU_DEFAULT_ON_THRESHOLD: usize = 10;
const EACH_LAUNCH_CONFIRM_THRESHOLD: usize = 20;
const ASSOCIATED_APP_DISPLAY_NAME: &str = "関連付けアプリ";
pub(crate) const VIRTUAL_EDITING_DISABLED_REASON: &str = "圧縮ファイル内のページは編集用ツールで開けません。書き出してから編集してください (フルスクリーンで Ctrl+E)";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const IMPLEMENTED_PLACEHOLDERS: &[&str] = &[
    "{file}", "{files}", "{dir}", "{name}", "{stem}", "{ext}", "{uri}",
];
const DEFERRED_PLACEHOLDERS: &[&str] = &[
    "{container}",
    "{entry}",
    "{page}",
    "{time}",
    "{time_ms}",
    "{time_hms}",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ExternalToolId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExternalToolLaunch {
    Executable(PathBuf),
    Association { handler_id: String },
    OsDefault,
}

impl ExternalToolLaunch {
    pub fn executable(&self) -> Option<&Path> {
        match self {
            Self::Executable(path) => Some(path),
            Self::Association { .. } | Self::OsDefault => None,
        }
    }

    pub fn uses_process_options(&self) -> bool {
        matches!(self, Self::Executable(_))
    }

    pub(crate) fn same_target(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Executable(left), Self::Executable(right)) => left
                .as_os_str()
                .as_encoded_bytes()
                .eq_ignore_ascii_case(right.as_os_str().as_encoded_bytes()),
            (Self::Association { handler_id: left }, Self::Association { handler_id: right }) => {
                left.eq_ignore_ascii_case(right)
            }
            (Self::Executable(path), Self::Association { handler_id })
            | (Self::Association { handler_id }, Self::Executable(path)) => path
                .as_os_str()
                .as_encoded_bytes()
                .eq_ignore_ascii_case(handler_id.as_bytes()),
            (Self::OsDefault, Self::OsDefault) => true,
            _ => false,
        }
    }
}

/// リリース済み `recent_open_with_apps.exe_path` の値を起動種別へ分類する。
///
/// ファイルシステム参照は呼び出し側が済ませ、結果だけを渡す。これにより分類規則を
/// migration と旧 JSON 読み込みで共有しつつ、純関数として検証できる。
pub(crate) fn classify_legacy_recent_launch(
    stored_value: &str,
    is_existing_file: bool,
) -> ExternalToolLaunch {
    if is_existing_file {
        ExternalToolLaunch::Executable(PathBuf::from(stored_value))
    } else {
        ExternalToolLaunch::Association {
            handler_id: stored_value.to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PayloadPolicy {
    #[default]
    AsDisplayed,
    Original,
    Container,
    RealFileOnly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VideoPolicy {
    #[default]
    File,
    CurrentFrame,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SpreadPolicy {
    #[default]
    Merged,
    BothPages,
    MainPageOnly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SelectionPolicy {
    #[default]
    Single,
    Each,
    Batch,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExternalTool {
    pub id: ExternalToolId,
    pub name: String,
    pub launch: ExternalToolLaunch,
    pub arguments: String,
    pub working_directory: Option<PathBuf>,
    pub payload: PayloadPolicy,
    pub video: VideoPolicy,
    pub spread: SpreadPolicy,
    pub selection: SelectionPolicy,
    pub pdf_render_long_edge: u32,
    pub for_editing: bool,
    pub show_in_context_menu: bool,
    pub keep_temp: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExternalToolMenuTarget {
    RealFile,
    VirtualPage,
    Unsupported,
}

impl ExternalToolMenuTarget {
    /// 解決済み集合からメニューの capability を決める。
    ///
    /// 実項目 + 仮想項目の混在では項目を残し、実行境界で選択全体を理由付き拒否する。
    /// ここで非表示にすると「起動せずトーストで断る」入口そのものが消えるためである。
    pub(crate) fn from_launch_targets(targets: &[LaunchTarget]) -> Self {
        if targets.iter().any(LaunchTarget::is_real_file) {
            Self::RealFile
        } else if targets.iter().any(LaunchTarget::is_virtual) {
            Self::VirtualPage
        } else {
            Self::Unsupported
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExternalToolMenuItem {
    pub tool_id: ExternalToolId,
    pub label: String,
    pub enabled: bool,
    pub disabled_reason: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchTarget {
    RealFile(PathBuf),
    Virtual(crate::grid_item::FileOperationRefusal),
    Unsupported,
    None,
}

impl LaunchTarget {
    pub(crate) fn from_grid_item(item: Option<&crate::grid_item::GridItem>) -> Self {
        use crate::grid_item::GridItem;
        match item {
            Some(
                GridItem::Image(path)
                | GridItem::Video(path)
                | GridItem::Audio(path)
                | GridItem::ZipFile(path)
                | GridItem::PdfFile(path),
            ) => Self::RealFile(path.clone()),
            Some(GridItem::ConvertibleArchive { path, .. }) => Self::RealFile(path.clone()),
            Some(GridItem::ZipImage { .. } | GridItem::PdfPage { .. }) => {
                Self::Virtual(crate::grid_item::FileOperationRefusal::VirtualPage)
            }
            Some(GridItem::Stack { .. }) => {
                Self::Virtual(crate::grid_item::FileOperationRefusal::Stack)
            }
            Some(GridItem::ZipDir { .. }) => {
                Self::Virtual(crate::grid_item::FileOperationRefusal::ArchiveDirectory)
            }
            Some(_) => Self::Unsupported,
            None => Self::None,
        }
    }

    fn is_real_file(&self) -> bool {
        matches!(self, Self::RealFile(_))
    }

    fn is_virtual(&self) -> bool {
        matches!(self, Self::Virtual(_))
    }

    pub fn real_file(&self) -> Result<&Path, String> {
        match self {
            Self::RealFile(path) => Ok(path),
            Self::Virtual(_) => Err("仮想ページはこの段階では外部ツールへ渡せません".to_string()),
            Self::Unsupported => Err("この項目は外部ツールへ渡せません".to_string()),
            Self::None => Err("外部ツールへ渡す実ファイルが選択されていません".to_string()),
        }
    }
}

/// 外部ツール起動の発火面。`fullscreen_idx` の有無から暗黙推定しない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalTargetSource {
    GridContext { clicked: Option<usize> },
    GridKey { selected: Option<usize> },
    Viewer { current: Option<usize> },
    Playback { current: Option<usize> },
    Container { path: Option<PathBuf> },
}

fn ordered_checked_indices(
    checked: &HashSet<usize>,
    primary: Option<usize>,
    display_order: &[usize],
) -> Vec<usize> {
    let mut result = Vec::with_capacity(checked.len());
    if let Some(primary) = primary.filter(|index| checked.contains(index)) {
        result.push(primary);
    }
    result.extend(
        display_order
            .iter()
            .copied()
            .filter(|index| checked.contains(index) && Some(*index) != primary),
    );
    result
}

/// 発火面、checked、実際の表示順から対象集合を一度だけ解決する純関数。
///
/// `display_order` には `App::current_grid_order()` を渡す。詳細表示の列ソートを保ち、
/// item index 順へ並べ直してはならない。checked に primary が含まれる場合だけ先頭へ移す。
pub fn resolve_external_targets(
    items: &[crate::grid_item::GridItem],
    display_order: &[usize],
    checked: &HashSet<usize>,
    source: ExternalTargetSource,
) -> Vec<LaunchTarget> {
    let indices = match source {
        ExternalTargetSource::GridContext { clicked } => {
            if checked.is_empty() {
                clicked.into_iter().collect()
            } else {
                ordered_checked_indices(checked, clicked, display_order)
            }
        }
        ExternalTargetSource::GridKey { selected } => {
            if checked.is_empty() {
                selected.into_iter().collect()
            } else {
                ordered_checked_indices(checked, selected, display_order)
            }
        }
        ExternalTargetSource::Viewer { current } | ExternalTargetSource::Playback { current } => {
            current.into_iter().collect()
        }
        ExternalTargetSource::Container { path } => {
            return path.map(LaunchTarget::RealFile).into_iter().collect();
        }
    };
    indices
        .into_iter()
        .filter_map(|index| items.get(index))
        .map(|item| LaunchTarget::from_grid_item(Some(item)))
        .collect()
}

#[derive(Clone, Debug)]
pub struct PlaceholderContext {
    file: PathBuf,
}

impl PlaceholderContext {
    pub fn for_file(file: impl Into<PathBuf>) -> Self {
        Self { file: file.into() }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }
}

#[derive(Debug)]
pub(crate) struct ExternalLaunchRequest {
    pub tool_name: String,
    pub launch: ExternalToolLaunch,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub files: Vec<PathBuf>,
}

#[derive(Debug)]
pub(crate) struct ExternalLaunchOperation {
    tool_name: String,
    requests: Vec<ExternalLaunchRequest>,
    target_count: usize,
}

#[derive(Debug)]
pub(crate) struct ExternalLaunchConfirmation {
    operation: ExternalLaunchOperation,
    network_executable: Option<PathBuf>,
    many_launch_count: Option<usize>,
}

#[derive(Debug)]
struct ExternalLaunchAttemptResult {
    target_count: usize,
    target_label: String,
    pub result: Result<Option<AssociationHandlerRefresh>, String>,
}

#[derive(Debug)]
pub(crate) struct AssociationHandlerRefresh {
    previous_id: String,
    current_id: String,
}

pub(crate) struct ExternalLaunchPending {
    pub cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<ExternalLaunchAttemptResult>,
    tool_name: String,
    expected_attempts: usize,
    completed_attempts: usize,
    target_count: usize,
    completed_target_count: usize,
    succeeded_target_count: usize,
    failures: Vec<String>,
    refreshes: Vec<AssociationHandlerRefresh>,
}

#[derive(Debug)]
struct ExternalLaunchCompletion {
    tool_name: String,
    target_count: usize,
    succeeded_target_count: usize,
    failures: Vec<String>,
    refreshes: Vec<AssociationHandlerRefresh>,
}

impl ExternalLaunchPending {
    fn poll_completion(&mut self) -> Option<ExternalLaunchCompletion> {
        let mut disconnected = false;
        loop {
            match self.rx.try_recv() {
                Ok(attempt) => {
                    self.completed_attempts += 1;
                    self.completed_target_count += attempt.target_count;
                    match attempt.result {
                        Ok(refresh) => {
                            self.succeeded_target_count += attempt.target_count;
                            if let Some(refresh) = refresh {
                                self.refreshes.push(refresh);
                            }
                        }
                        Err(error) => self
                            .failures
                            .push(format!("{}: {error}", attempt.target_label)),
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if self.completed_attempts < self.expected_attempts && !disconnected {
            return None;
        }
        if self.completed_target_count < self.target_count {
            self.failures
                .push("外部ツールの起動結果を受け取れませんでした".to_string());
        }
        Some(ExternalLaunchCompletion {
            tool_name: self.tool_name.clone(),
            target_count: self.target_count,
            succeeded_target_count: self.succeeded_target_count,
            failures: std::mem::take(&mut self.failures),
            refreshes: std::mem::take(&mut self.refreshes),
        })
    }
}

impl Drop for ExternalLaunchPending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl ExternalTool {
    /// 右クリックメニューに出す文言。
    ///
    /// 名前だけだと項目として認識されにくい (利用者が区切り線の下の項目を見落とした,
    /// 2026-08-30)。既存の「最近使ったアプリ」や「このフォルダをエクスプローラで開く」と
    /// 同じく、空白を入れずに `で開く` を付ける。表記の所有者はここ 1 か所にする。
    pub fn menu_label(&self) -> String {
        format!("{}で開く", self.display_name())
    }

    pub fn display_name(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        if let Some(stem) = self
            .launch
            .executable()
            .and_then(|path| path.file_stem())
            .filter(|stem| !stem.is_empty())
        {
            return stem.to_string_lossy().into_owned();
        }
        ASSOCIATED_APP_DISPLAY_NAME.to_string()
    }

    pub fn launch_description(&self) -> String {
        match &self.launch {
            ExternalToolLaunch::Executable(path) => path.display().to_string(),
            ExternalToolLaunch::Association { handler_id } => {
                let name = if self.name.is_empty() {
                    handler_id
                } else {
                    &self.name
                };
                format!("関連付けアプリ ({name})")
            }
            ExternalToolLaunch::OsDefault => "OS の関連付け".to_string(),
        }
    }

    pub fn defaults_for_viewing() -> Self {
        Self {
            id: ExternalToolId(1),
            name: String::new(),
            launch: ExternalToolLaunch::OsDefault,
            arguments: "{file}".to_string(),
            working_directory: None,
            payload: PayloadPolicy::AsDisplayed,
            video: VideoPolicy::File,
            spread: SpreadPolicy::Merged,
            selection: SelectionPolicy::Single,
            pdf_render_long_edge: DEFAULT_PDF_RENDER_LONG_EDGE,
            for_editing: false,
            show_in_context_menu: true,
            keep_temp: false,
        }
    }

    pub fn defaults_for_editing() -> Self {
        Self {
            payload: PayloadPolicy::Original,
            spread: SpreadPolicy::MainPageOnly,
            for_editing: true,
            ..Self::defaults_for_viewing()
        }
    }
}

/// 既存の最大 ID + 1 を返す。並べ替えでは変わらない。
///
/// ID 空間の上端に達している場合 (DB を手で編集した等) は panic せず、前から空きを探す。
/// 登録数は現実的に小さいので、この経路でもすぐ見つかる。
pub fn next_id(existing: &[ExternalTool]) -> ExternalToolId {
    let max_id = existing.iter().map(|tool| tool.id.0).max().unwrap_or(0);
    if let Some(next) = max_id.checked_add(1) {
        return ExternalToolId(next);
    }
    let used: std::collections::HashSet<u32> = existing.iter().map(|tool| tool.id.0).collect();
    let free = (1..=u32::MAX)
        .find(|candidate| !used.contains(candidate))
        .unwrap_or(u32::MAX);
    ExternalToolId(free)
}

/// 設定ページから新規登録するとき、右クリック表示を既定 ON にするかを返す。
///
/// 利用者指定どおり、既存の ON が 10 件なら新規も ON、10 件を超えている場合だけ OFF にする。
pub(crate) fn show_in_context_menu_by_default(existing: &[ExternalTool]) -> bool {
    existing
        .iter()
        .filter(|tool| tool.show_in_context_menu)
        .count()
        <= CONTEXT_MENU_DEFAULT_ON_THRESHOLD
}

/// 外部ツールの右クリック項目を登録順に組み立てる。
///
/// ファイル存在確認や補正 DB 参照は行わず、項目種別とツール定義だけで判定する。
pub(crate) fn external_tool_menu_items(
    tools: &[ExternalTool],
    target: ExternalToolMenuTarget,
) -> Vec<ExternalToolMenuItem> {
    tools
        .iter()
        .filter(|tool| tool.show_in_context_menu)
        .filter_map(|tool| match target {
            ExternalToolMenuTarget::RealFile => Some(ExternalToolMenuItem {
                tool_id: tool.id,
                label: tool.menu_label(),
                enabled: true,
                disabled_reason: None,
            }),
            ExternalToolMenuTarget::VirtualPage if tool.for_editing => Some(ExternalToolMenuItem {
                tool_id: tool.id,
                label: tool.menu_label(),
                enabled: false,
                disabled_reason: Some(VIRTUAL_EDITING_DISABLED_REASON),
            }),
            ExternalToolMenuTarget::VirtualPage | ExternalToolMenuTarget::Unsupported => None,
        })
        .collect()
}

/// Windows の `CommandLineToArgvW` と同じ引用符・バックスラッシュ規則で、
/// 引数テンプレートを先にトークンへ分割する。
fn split_argument_template_with_default(
    template: &str,
    default_placeholder: &'static str,
) -> Vec<String> {
    let mut result = Vec::new();
    let chars: Vec<char> = template.chars().collect();
    let mut index = 0;
    let mut current = String::new();
    let mut in_quotes = false;
    let mut token_started = false;

    while index < chars.len() {
        let ch = chars[index];
        if ch.is_whitespace() && !in_quotes {
            if token_started {
                result.push(std::mem::take(&mut current));
                token_started = false;
            }
            index += 1;
            continue;
        }

        if ch == '\\' {
            let start = index;
            while index < chars.len() && chars[index] == '\\' {
                index += 1;
            }
            let slash_count = index - start;
            if index < chars.len() && chars[index] == '"' {
                current.extend(std::iter::repeat_n('\\', slash_count / 2));
                token_started = true;
                if slash_count % 2 == 1 {
                    current.push('"');
                    index += 1;
                } else if in_quotes && index + 1 < chars.len() && chars[index + 1] == '"' {
                    current.push('"');
                    index += 2;
                } else {
                    in_quotes = !in_quotes;
                    index += 1;
                }
            } else {
                current.extend(std::iter::repeat_n('\\', slash_count));
            }
            continue;
        }

        if ch == '"' {
            token_started = true;
            if in_quotes && index + 1 < chars.len() && chars[index + 1] == '"' {
                current.push('"');
                index += 2;
            } else {
                in_quotes = !in_quotes;
                index += 1;
            }
            continue;
        }

        token_started = true;
        current.push(ch);
        index += 1;
    }

    if token_started {
        result.push(current);
    }

    if !contains_known_placeholder(template) {
        result.push(default_placeholder.to_string());
    }
    result
}

pub fn split_argument_template(template: &str) -> Vec<String> {
    split_argument_template_with_default(template, "{file}")
}

fn split_argument_template_for_selection(
    template: &str,
    selection: SelectionPolicy,
) -> Vec<String> {
    let default_placeholder = if selection == SelectionPolicy::Batch {
        "{files}"
    } else {
        "{file}"
    };
    split_argument_template_with_default(template, default_placeholder)
}

fn contains_known_placeholder(template: &str) -> bool {
    IMPLEMENTED_PLACEHOLDERS
        .iter()
        .chain(DEFERRED_PLACEHOLDERS)
        .any(|placeholder| template.contains(placeholder))
}

fn placeholder_value<'a>(placeholder: &str, ctx: &'a PlaceholderContext) -> Option<&'a OsStr> {
    match placeholder {
        "{file}" => Some(ctx.file.as_os_str()),
        "{dir}" => Some(
            ctx.file
                .parent()
                .map(Path::as_os_str)
                .unwrap_or_else(|| OsStr::new("")),
        ),
        "{name}" => Some(ctx.file.file_name().unwrap_or_else(|| OsStr::new(""))),
        "{stem}" => Some(ctx.file.file_stem().unwrap_or_else(|| OsStr::new(""))),
        "{ext}" => Some(ctx.file.extension().unwrap_or_else(|| OsStr::new(""))),
        _ => None,
    }
}

fn file_uri(path: &Path) -> OsString {
    let encoded = path.as_os_str().as_encoded_bytes();
    let is_unc = encoded.starts_with(b"\\\\");
    let mut uri = if is_unc {
        String::from("file:")
    } else {
        String::from("file:///")
    };
    for &byte in encoded {
        match byte {
            b'\\' => uri.push('/'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                uri.push(byte as char)
            }
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                uri.push('%');
                uri.push(HEX[(byte >> 4) as usize] as char);
                uri.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
    }
    OsString::from(uri)
}

fn expand_token(
    token: &str,
    ctx: &PlaceholderContext,
    files_value: Option<&OsStr>,
) -> Option<OsString> {
    let mut expanded = OsString::new();
    let mut remainder = token;
    let mut had_empty_placeholder = false;

    while let Some(open) = remainder.find('{') {
        expanded.push(&remainder[..open]);
        let after_open = &remainder[open..];
        let Some(relative_close) = after_open.find('}') else {
            expanded.push(after_open);
            remainder = "";
            break;
        };
        let close = relative_close + 1;
        let placeholder = &after_open[..close];
        if placeholder == "{files}" {
            if let Some(value) = files_value.filter(|value| !value.is_empty()) {
                expanded.push(value);
            } else {
                had_empty_placeholder = true;
            }
        } else if placeholder == "{uri}" {
            expanded.push(file_uri(&ctx.file));
        } else if let Some(value) = placeholder_value(placeholder, ctx) {
            if value.is_empty() {
                had_empty_placeholder = true;
            } else {
                expanded.push(value);
            }
        } else if DEFERRED_PLACEHOLDERS.contains(&placeholder) {
            had_empty_placeholder = true;
        } else {
            expanded.push(placeholder);
        }
        remainder = &after_open[close..];
    }
    expanded.push(remainder);

    if had_empty_placeholder || expanded.is_empty() {
        None
    } else {
        Some(expanded)
    }
}

/// 分割済みトークン内だけでプレースホルダを置換する。
///
/// 空のプレースホルダで単独の値トークンが消えた場合、直前の option トークンも除く。
/// これにより `-page {page}` が `-page` だけになることを防ぐ。
pub fn expand_arguments(tokens: &[String], ctx: &PlaceholderContext) -> Vec<OsString> {
    expand_arguments_for_files(tokens, std::slice::from_ref(ctx))
}

/// `{files}` を含む 1 トークンを対象数ぶんの引数へ展開する。
///
/// パスを空白連結した文字列にはせず、各パスを独立した `OsString` として返す。
pub fn expand_arguments_for_files(
    tokens: &[String],
    contexts: &[PlaceholderContext],
) -> Vec<OsString> {
    let Some(primary) = contexts.first() else {
        return Vec::new();
    };
    let mut result: Vec<OsString> = Vec::new();
    for token in tokens {
        let before = result.len();
        if token.contains("{files}") {
            result.extend(contexts.iter().filter_map(|ctx| {
                // `{file}` などの scalar は Batch でも先頭対象を指す。
                // N 件へ変化させるのは `{files}` の値だけにする。
                expand_token(token, primary, Some(ctx.file.as_os_str()))
            }));
        } else if let Some(expanded) = expand_token(token, primary, None) {
            result.push(expanded);
        }
        if result.len() == before {
            let placeholder_only = token.starts_with('{') && token.ends_with('}');
            if placeholder_only
                && result
                    .last()
                    .is_some_and(|previous| previous.as_encoded_bytes().starts_with(b"-"))
            {
                result.pop();
            }
        }
    }
    result
}

pub(crate) fn build_launch_request(
    tool: &ExternalTool,
    target: &LaunchTarget,
) -> Result<ExternalLaunchRequest, String> {
    let mut operation = build_launch_operation(tool, std::slice::from_ref(target))?;
    operation
        .requests
        .pop()
        .ok_or_else(|| "外部ツールの起動要求を組み立てられませんでした".to_string())
}

fn virtual_target_error(targets: &[LaunchTarget]) -> Option<String> {
    let refusals: Vec<_> = targets
        .iter()
        .filter_map(|target| match target {
            LaunchTarget::Virtual(refusal) => Some(*refusal),
            _ => None,
        })
        .collect();
    if refusals.is_empty() {
        return None;
    }
    // P3 の materializer が入るまでの暫定拒否。部分実行にすると、選択件数と
    // 外部アプリへ渡った件数が黙って食い違うため、SelectionPolicy 適用前に全体を止める。
    if refusals.contains(&crate::grid_item::FileOperationRefusal::VirtualPage) {
        Some(crate::grid_item::checked_virtual_selection_message(
            "外部ツールで開くことが",
        ))
    } else {
        Some(refusals[0].message("外部ツールで開くことが"))
    }
}

fn real_files_from_targets(targets: &[LaunchTarget]) -> Result<Vec<PathBuf>, String> {
    if targets.is_empty()
        || targets
            .iter()
            .any(|target| matches!(target, LaunchTarget::None))
    {
        return Err("外部ツールへ渡す実ファイルが選択されていません".to_string());
    }
    if let Some(error) = virtual_target_error(targets) {
        return Err(error);
    }
    if targets
        .iter()
        .any(|target| matches!(target, LaunchTarget::Unsupported))
    {
        return Err("この項目は外部ツールへ渡せません".to_string());
    }
    Ok(targets
        .iter()
        .filter_map(|target| match target {
            LaunchTarget::RealFile(path) => Some(path.clone()),
            _ => None,
        })
        .collect())
}

fn build_request_for_files(
    tool: &ExternalTool,
    files: Vec<PathBuf>,
) -> Result<ExternalLaunchRequest, String> {
    let contexts: Vec<_> = files
        .iter()
        .cloned()
        .map(PlaceholderContext::for_file)
        .collect();
    let arguments = if tool.launch.uses_process_options() {
        let tokens = split_argument_template_for_selection(&tool.arguments, tool.selection);
        if tool.selection == SelectionPolicy::Batch
            && !tokens.iter().any(|token| token.contains("{files}"))
        {
            return Err(
                "まとめて渡すには引数テンプレートに {files} を指定してください".to_string(),
            );
        }
        expand_arguments_for_files(&tokens, &contexts)
    } else {
        Vec::new()
    };
    Ok(ExternalLaunchRequest {
        tool_name: tool.display_name(),
        launch: tool.launch.clone(),
        arguments,
        working_directory: tool
            .launch
            .uses_process_options()
            .then(|| tool.working_directory.clone())
            .flatten(),
        files,
    })
}

pub(crate) fn build_launch_operation(
    tool: &ExternalTool,
    targets: &[LaunchTarget],
) -> Result<ExternalLaunchOperation, String> {
    let files = real_files_from_targets(targets)?;
    let requests = match tool.selection {
        SelectionPolicy::Single => vec![build_request_for_files(tool, vec![files[0].clone()])?],
        SelectionPolicy::Each => files
            .into_iter()
            .map(|file| build_request_for_files(tool, vec![file]))
            .collect::<Result<Vec<_>, _>>()?,
        SelectionPolicy::Batch => match &tool.launch {
            ExternalToolLaunch::Executable(_) | ExternalToolLaunch::Association { .. } => {
                vec![build_request_for_files(tool, files)?]
            }
            // OS 既定アプリへ複数パスをまとめて渡す API はないため Each と同じ。
            ExternalToolLaunch::OsDefault => files
                .into_iter()
                .map(|file| build_request_for_files(tool, vec![file]))
                .collect::<Result<Vec<_>, _>>()?,
        },
    };
    let target_count = requests.iter().map(|request| request.files.len()).sum();
    Ok(ExternalLaunchOperation {
        tool_name: tool.display_name(),
        requests,
        target_count,
    })
}

fn build_open_with_launch_request(
    tool_name: String,
    launch: ExternalToolLaunch,
    file: PathBuf,
) -> ExternalLaunchRequest {
    let arguments = match &launch {
        ExternalToolLaunch::Executable(_) => vec![file.as_os_str().to_os_string()],
        ExternalToolLaunch::Association { .. } | ExternalToolLaunch::OsDefault => Vec::new(),
    };
    ExternalLaunchRequest {
        tool_name,
        launch,
        arguments,
        working_directory: None,
        files: vec![file],
    }
}

fn executable_requires_confirmation(request: &ExternalLaunchRequest) -> bool {
    request.launch.executable().is_some_and(|path| {
        path.as_os_str()
            .as_encoded_bytes()
            .first()
            .is_some_and(|byte| *byte == b'\\')
    })
}

fn launch_confirmation(
    operation: ExternalLaunchOperation,
) -> Result<ExternalLaunchOperation, ExternalLaunchConfirmation> {
    let network_executable = operation
        .requests
        .iter()
        .find(|request| executable_requires_confirmation(request))
        .and_then(|request| request.launch.executable())
        .map(Path::to_path_buf);
    let many_launch_count = (operation.requests.len() > EACH_LAUNCH_CONFIRM_THRESHOLD)
        .then_some(operation.requests.len());
    if network_executable.is_none() && many_launch_count.is_none() {
        Ok(operation)
    } else {
        Err(ExternalLaunchConfirmation {
            operation,
            network_executable,
            many_launch_count,
        })
    }
}

pub(crate) fn start_launch_worker(
    operation: ExternalLaunchOperation,
) -> Result<ExternalLaunchPending, String> {
    let tool_name = operation.tool_name.clone();
    let expected_attempts = operation.requests.len();
    let target_count = operation.target_count;
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("external-tool-launch".to_string())
        .spawn(move || {
            // 1 操作 = 1 coordinator。要求順に spawn / Invoke するので、表示順が
            // scheduler によって逆転しない。spawn 済みの外部プロセス同士は並行して動く。
            for request in operation.requests {
                if cancel_worker.load(Ordering::Relaxed) {
                    return;
                }
                let target_count = request.files.len();
                let target_label = if target_count == 1 {
                    request.files[0].display().to_string()
                } else {
                    format!("{target_count} 件")
                };
                let result = launch_request(request);
                if cancel_worker.load(Ordering::Relaxed)
                    || tx
                        .send(ExternalLaunchAttemptResult {
                            target_count,
                            target_label,
                            result,
                        })
                        .is_err()
                {
                    return;
                }
            }
        })
        .map_err(|error| format!("外部ツール起動 worker を開始できません: {error}"))?;
    Ok(ExternalLaunchPending {
        cancel,
        rx,
        tool_name,
        expected_attempts,
        completed_attempts: 0,
        target_count,
        completed_target_count: 0,
        succeeded_target_count: 0,
        failures: Vec::new(),
        refreshes: Vec::new(),
    })
}

fn launch_request(
    request: ExternalLaunchRequest,
) -> Result<Option<AssociationHandlerRefresh>, String> {
    let ExternalLaunchRequest {
        tool_name,
        launch,
        arguments,
        working_directory,
        files,
    } = request;
    match launch {
        ExternalToolLaunch::Executable(executable) => {
            let mut command = Command::new(executable);
            command
                .args(arguments)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Some(directory) = working_directory {
                command.current_dir(directory);
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(CREATE_NO_WINDOW);
            }
            command
                .spawn()
                .map(|_| None)
                .map_err(|error| error.to_string())
        }
        ExternalToolLaunch::Association { handler_id } => {
            let outcome = if files.len() == 1 {
                crate::open_with::invoke_association_handler(&handler_id, &tool_name, &files[0])
            } else {
                crate::open_with::invoke_association_handler_for_paths(
                    &handler_id,
                    &tool_name,
                    &files,
                )
            };
            outcome.map(|outcome| {
                outcome
                    .refreshed_handler_id
                    .map(|current_id| AssociationHandlerRefresh {
                        previous_id: handler_id,
                        current_id,
                    })
            })
        }
        ExternalToolLaunch::OsDefault => {
            let Some(file) = files.into_iter().next() else {
                return Err("OS の関連付けへ渡す実ファイルがありません".to_string());
            };
            opener::open(file)
                .map(|_| None)
                .map_err(|error| error.to_string())
        }
    }
}

fn write_back_association_handler_id(
    settings: &mut crate::settings::Settings,
    refresh: &AssociationHandlerRefresh,
) {
    let update = |launch: &mut ExternalToolLaunch| {
        let ExternalToolLaunch::Association { handler_id } = launch else {
            return;
        };
        if *handler_id == refresh.previous_id {
            handler_id.clone_from(&refresh.current_id);
        }
    };
    for tool in &mut settings.external_tools {
        update(&mut tool.launch);
    }
}

fn external_launch_error_message(tool_name: &str, error: &str) -> String {
    format!("{tool_name}: {error}")
}

fn external_launch_completion_message(completion: &ExternalLaunchCompletion) -> String {
    let failed = completion
        .target_count
        .saturating_sub(completion.succeeded_target_count);
    match (completion.succeeded_target_count, failed) {
        (1, 0) => format!("{} を起動しました", completion.tool_name),
        (succeeded, 0) => format!("{}: {succeeded} 件を起動しました", completion.tool_name),
        (0, 1) => external_launch_error_message(
            &completion.tool_name,
            completion
                .failures
                .first()
                .map(String::as_str)
                .unwrap_or("起動に失敗しました"),
        ),
        (succeeded, failed) => {
            let detail = completion
                .failures
                .first()
                .map(|error| format!("。{error}"))
                .unwrap_or_default();
            format!(
                "{}: {succeeded} 件を起動し、{failed} 件の起動に失敗しました{detail}",
                completion.tool_name
            )
        }
    }
}

fn external_launch_completion_summary(completions: &[ExternalLaunchCompletion]) -> String {
    completions
        .iter()
        .map(external_launch_completion_message)
        .collect::<Vec<_>>()
        .join("\n")
}

impl crate::app::App {
    pub(crate) fn queue_external_tool_launch(
        &mut self,
        tool: &ExternalTool,
        target: &LaunchTarget,
    ) {
        self.queue_external_tool_launch_targets(tool, std::slice::from_ref(target));
    }

    pub(crate) fn queue_external_tool_launch_targets(
        &mut self,
        tool: &ExternalTool,
        targets: &[LaunchTarget],
    ) {
        let operation = match build_launch_operation(tool, targets) {
            Ok(operation) => operation,
            Err(error) => {
                self.show_feedback_toast(format!("{}: {error}", tool.display_name()));
                return;
            }
        };
        match launch_confirmation(operation) {
            Ok(operation) => self.start_external_launch_operation(operation),
            Err(confirmation) => {
                self.external_tool_launch_confirmation = Some(confirmation);
            }
        }
    }

    pub(crate) fn start_open_with(
        &mut self,
        display_name: String,
        launch: ExternalToolLaunch,
        file: PathBuf,
    ) {
        let request = build_open_with_launch_request(display_name.clone(), launch, file);
        let operation = ExternalLaunchOperation {
            tool_name: display_name,
            target_count: request.files.len(),
            requests: vec![request],
        };
        match launch_confirmation(operation) {
            Ok(operation) => self.start_external_launch_operation(operation),
            Err(confirmation) => {
                self.external_tool_launch_confirmation = Some(confirmation);
            }
        }
    }

    fn start_external_launch_operation(&mut self, operation: ExternalLaunchOperation) {
        let tool_name = operation.tool_name.clone();
        let target_count = operation.target_count;
        match start_launch_worker(operation) {
            Ok(pending) => self.external_tool_launch_pending.push(pending),
            Err(error) if target_count == 1 => {
                self.show_feedback_toast(format!("{tool_name}: {error}"))
            }
            Err(error) => self.show_feedback_toast(format!(
                "{tool_name}: {target_count} 件の起動を開始できませんでした ({error})"
            )),
        }
    }

    pub(crate) fn poll_external_tool_launch(&mut self, ctx: &egui::Context) {
        if self.external_tool_launch_pending.is_empty() {
            return;
        }
        let mut finished = Vec::new();
        let mut index = 0;
        while index < self.external_tool_launch_pending.len() {
            if let Some(completion) = self.external_tool_launch_pending[index].poll_completion() {
                self.external_tool_launch_pending.remove(index);
                finished.push(completion);
            } else {
                index += 1;
            }
        }
        for completion in &finished {
            for refresh in &completion.refreshes {
                write_back_association_handler_id(&mut self.settings, refresh);
            }
            for failure in &completion.failures {
                crate::logger::log(format!(
                    "external_tool: {} launch failure: {failure}",
                    completion.tool_name
                ));
            }
        }
        if !finished.is_empty() {
            // トースト owner は 1 枠なので、同じ frame に別操作が複数完了しても
            // 後着だけで上書きせず、全操作の結果を 1 通へまとめる。
            self.show_feedback_toast(external_launch_completion_summary(&finished));
        }
        if !self.external_tool_launch_pending.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    pub(crate) fn show_external_tool_launch_confirmation(&mut self, ctx: &egui::Context) {
        let Some(confirmation) = self.external_tool_launch_confirmation.as_ref() else {
            return;
        };
        let tool_name = confirmation.operation.tool_name.clone();
        let executable = confirmation
            .network_executable
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let many_launch_count = confirmation.many_launch_count;
        let mut launch = false;
        let mut cancel = false;
        let response =
            egui::Modal::new(egui::Id::new("external_tool_launch_confirmation")).show(ctx, |ui| {
                ui.set_min_width(440.0);
                if many_launch_count.is_some() {
                    ui.heading("外部ツールを複数起動しますか？");
                } else {
                    ui.heading("ネットワーク上のツールを起動しますか？");
                }
                ui.add_space(8.0);
                ui.label(format!("ツール: {tool_name}"));
                if let Some(count) = many_launch_count {
                    ui.label(format!("{count} 件を 1 件ずつ起動します。"));
                }
                if !executable.is_empty() {
                    ui.label(format!("実行ファイル: {executable}"));
                }
                ui.add_space(8.0);
                if !executable.is_empty() {
                    ui.label("信頼できる場所であることを確認してから起動してください。");
                } else {
                    ui.label("多数のアプリケーションウィンドウが開く可能性があります。");
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("起動する").clicked() {
                        launch = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
            });
        if response.should_close() || self.dialog_escape_pressed(ctx) {
            cancel = true;
        }
        if self.dialog_enter_pressed(ctx) {
            launch = true;
        }
        if launch {
            if let Some(confirmation) = self.external_tool_launch_confirmation.take() {
                self.start_external_launch_operation(confirmation.operation);
            }
        } else if cancel {
            self.external_tool_launch_confirmation = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_uses_name_then_executable_stem_then_association_label() {
        let mut tool = ExternalTool::defaults_for_viewing();
        tool.name = "画像編集".to_string();
        tool.launch = ExternalToolLaunch::Executable(PathBuf::from(r"C:\Tools\editor.exe"));
        assert_eq!(tool.display_name(), "画像編集");

        tool.name.clear();
        assert_eq!(tool.display_name(), "editor");

        tool.launch = ExternalToolLaunch::Association {
            handler_id: "Photos.App".to_string(),
        };
        assert_eq!(tool.display_name(), ASSOCIATED_APP_DISPLAY_NAME);
    }

    #[test]
    fn handler_refresh_updates_registered_tools() {
        let mut settings = crate::settings::Settings::default();
        let mut tool = ExternalTool::defaults_for_viewing();
        tool.launch = ExternalToolLaunch::Association {
            handler_id: "old.Paint".to_string(),
        };
        settings.external_tools = vec![tool];

        write_back_association_handler_id(
            &mut settings,
            &AssociationHandlerRefresh {
                previous_id: "old.Paint".to_string(),
                current_id: "current.Paint".to_string(),
            },
        );

        assert!(matches!(
            &settings.external_tools[0].launch,
            ExternalToolLaunch::Association { handler_id } if handler_id == "current.Paint"
        ));
    }

    #[test]
    fn editing_defaults_preserve_original_main_page() {
        let tool = ExternalTool::defaults_for_editing();
        assert_eq!(tool.payload, PayloadPolicy::Original);
        assert_eq!(tool.spread, SpreadPolicy::MainPageOnly);
        assert!(tool.for_editing);
        assert_eq!(tool.arguments, "{file}");
        assert_eq!(tool.pdf_render_long_edge, DEFAULT_PDF_RENDER_LONG_EDGE);
    }

    #[test]
    fn viewing_defaults_use_displayed_single_file_policy() {
        let tool = ExternalTool::defaults_for_viewing();
        assert_eq!(tool.payload, PayloadPolicy::AsDisplayed);
        assert_eq!(tool.video, VideoPolicy::File);
        assert_eq!(tool.spread, SpreadPolicy::Merged);
        assert_eq!(tool.selection, SelectionPolicy::Single);
        assert_eq!(tool.launch, ExternalToolLaunch::OsDefault);
        assert!(!tool.for_editing);
        assert!(tool.show_in_context_menu);
        assert!(!tool.keep_temp);
        assert_eq!(tool.pdf_render_long_edge, DEFAULT_PDF_RENDER_LONG_EDGE);
    }

    #[test]
    fn next_id_uses_maximum_regardless_of_order() {
        let mut tools = vec![
            ExternalTool {
                id: ExternalToolId(9),
                ..ExternalTool::defaults_for_viewing()
            },
            ExternalTool {
                id: ExternalToolId(2),
                ..ExternalTool::defaults_for_viewing()
            },
        ];
        assert_eq!(next_id(&tools), ExternalToolId(10));
        tools.reverse();
        assert_eq!(next_id(&tools), ExternalToolId(10));
        assert_eq!(next_id(&[]), ExternalToolId(1));
    }

    #[test]
    fn next_id_falls_back_to_a_free_slot_instead_of_panicking_at_the_top() {
        let tools = vec![
            ExternalTool {
                id: ExternalToolId(u32::MAX),
                ..ExternalTool::defaults_for_viewing()
            },
            ExternalTool {
                id: ExternalToolId(1),
                ..ExternalTool::defaults_for_viewing()
            },
        ];
        // max + 1 が溢れるので、前から空いている ID を返す。panic しないことが要件。
        assert_eq!(next_id(&tools), ExternalToolId(2));
    }

    fn menu_tool(id: u32, name: &str, for_editing: bool, shown: bool) -> ExternalTool {
        ExternalTool {
            id: ExternalToolId(id),
            name: name.to_string(),
            launch: ExternalToolLaunch::Executable(PathBuf::from(format!(r"C:\Tools\{name}.exe"))),
            for_editing,
            show_in_context_menu: shown,
            ..ExternalTool::defaults_for_viewing()
        }
    }

    #[test]
    fn context_menu_default_turns_off_only_after_more_than_ten_are_on() {
        let ten_on: Vec<_> = (0..10)
            .map(|index| menu_tool(index + 1, &format!("tool-{index}"), false, true))
            .collect();
        assert!(show_in_context_menu_by_default(&ten_on));

        let mut eleven_on = ten_on.clone();
        eleven_on.push(menu_tool(11, "tool-10", false, true));
        assert!(!show_in_context_menu_by_default(&eleven_on));

        eleven_on.extend((12..30).map(|id| menu_tool(id, &format!("hidden-{id}"), false, false)));
        assert!(!show_in_context_menu_by_default(&eleven_on));
    }

    #[test]
    fn real_file_menu_keeps_registration_order_and_does_not_cap_visible_tools() {
        let mut tools: Vec<_> = (0..12)
            .map(|index| menu_tool(index + 1, &format!("tool-{index}"), false, true))
            .collect();
        tools.insert(4, menu_tool(99, "hidden", false, false));

        let items = external_tool_menu_items(&tools, ExternalToolMenuTarget::RealFile);

        assert_eq!(items.len(), 12);
        assert_eq!(
            items
                .iter()
                .map(|item| (item.tool_id.0, item.label.clone()))
                .collect::<Vec<_>>(),
            (0..12)
                .map(|index| (index + 1, format!("tool-{index}で開く")))
                .collect::<Vec<_>>()
        );
        assert!(items.iter().all(|item| item.enabled));
        assert!(items.iter().all(|item| item.disabled_reason.is_none()));
    }

    #[test]
    fn menu_label_appends_the_open_verb_without_a_space() {
        let mut tool = ExternalTool::defaults_for_viewing();
        tool.name = "Photoshop".to_string();
        assert_eq!(tool.menu_label(), "Photoshopで開く");

        // 名前が空なら exe の file_stem に付く。
        tool.name = String::new();
        tool.launch = ExternalToolLaunch::Executable(PathBuf::from(r"C:	ools\Foo Bar.exe"));
        assert_eq!(tool.menu_label(), "Foo Barで開く");
    }

    #[test]
    fn virtual_page_menu_keeps_only_disabled_editing_tools_with_reason() {
        let tools = vec![
            menu_tool(1, "viewer", false, true),
            menu_tool(2, "editor", true, true),
            menu_tool(3, "hidden-editor", true, false),
        ];

        let items = external_tool_menu_items(&tools, ExternalToolMenuTarget::VirtualPage);

        assert_eq!(
            items,
            vec![ExternalToolMenuItem {
                tool_id: ExternalToolId(2),
                label: "editorで開く".to_string(),
                enabled: false,
                disabled_reason: Some(VIRTUAL_EDITING_DISABLED_REASON),
            }]
        );
        assert!(external_tool_menu_items(&tools, ExternalToolMenuTarget::Unsupported).is_empty());
    }

    #[test]
    fn context_menu_target_classifies_real_virtual_and_unsupported_items() {
        assert_eq!(
            ExternalToolMenuTarget::from_launch_targets(&[LaunchTarget::RealFile(PathBuf::from(
                r"C:\Images\page.jpg",
            ))]),
            ExternalToolMenuTarget::RealFile
        );
        assert_eq!(
            ExternalToolMenuTarget::from_launch_targets(&[LaunchTarget::Virtual(
                crate::grid_item::FileOperationRefusal::VirtualPage,
            )]),
            ExternalToolMenuTarget::VirtualPage
        );
        assert_eq!(
            ExternalToolMenuTarget::from_launch_targets(&[LaunchTarget::Unsupported]),
            ExternalToolMenuTarget::Unsupported
        );
        assert_eq!(
            ExternalToolMenuTarget::from_launch_targets(&[
                LaunchTarget::Virtual(crate::grid_item::FileOperationRefusal::VirtualPage),
                LaunchTarget::RealFile(PathBuf::from(r"C:\Images\page.jpg")),
            ]),
            ExternalToolMenuTarget::RealFile,
            "混在拒否のトーストへ到達できるようメニュー項目は残す"
        );
    }

    fn resolver_items() -> Vec<crate::grid_item::GridItem> {
        ["zero.jpg", "one.jpg", "two.jpg"]
            .into_iter()
            .map(|name| crate::grid_item::GridItem::Image(PathBuf::from(name)))
            .collect()
    }

    fn target_paths(targets: &[LaunchTarget]) -> Vec<PathBuf> {
        targets
            .iter()
            .filter_map(|target| match target {
                LaunchTarget::RealFile(path) => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn target_resolver_prioritizes_checked_and_keeps_primary_then_display_order() {
        let items = resolver_items();
        let checked = HashSet::from([0, 1, 2]);
        let targets = resolve_external_targets(
            &items,
            &[2, 0, 1],
            &checked,
            ExternalTargetSource::GridContext { clicked: Some(1) },
        );
        assert_eq!(
            target_paths(&targets),
            ["one.jpg", "two.jpg", "zero.jpg"]
                .into_iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>()
        );

        let checked = HashSet::from([0, 2]);
        let targets = resolve_external_targets(
            &items,
            &[2, 0, 1],
            &checked,
            ExternalTargetSource::GridContext { clicked: Some(1) },
        );
        assert_eq!(
            target_paths(&targets),
            ["two.jpg", "zero.jpg"]
                .into_iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>(),
            "checked 外の右クリック項目を勝手に対象へ足さない"
        );
    }

    #[test]
    fn target_resolver_uses_selected_without_checks_and_viewer_ignores_stale_checks() {
        let items = resolver_items();
        assert_eq!(
            target_paths(&resolve_external_targets(
                &items,
                &[2, 0, 1],
                &HashSet::new(),
                ExternalTargetSource::GridKey { selected: Some(0) },
            )),
            [PathBuf::from("zero.jpg")]
        );
        assert_eq!(
            target_paths(&resolve_external_targets(
                &items,
                &[2, 0, 1],
                &HashSet::from([1, 2]),
                ExternalTargetSource::Viewer { current: Some(0) },
            )),
            [PathBuf::from("zero.jpg")]
        );
        assert_eq!(
            target_paths(&resolve_external_targets(
                &items,
                &[2, 0, 1],
                &HashSet::from([0, 1]),
                ExternalTargetSource::Playback { current: Some(2) },
            )),
            [PathBuf::from("two.jpg")]
        );
        assert_eq!(
            target_paths(&resolve_external_targets(
                &items,
                &[2, 0, 1],
                &HashSet::new(),
                ExternalTargetSource::Container {
                    path: Some(PathBuf::from("book.zip")),
                },
            )),
            [PathBuf::from("book.zip")]
        );
    }

    #[test]
    fn split_argument_template_handles_whitespace_quotes_and_escapes() {
        assert_eq!(
            split_argument_template(r#"  --mode edit   "two words" tail  "#),
            ["--mode", "edit", "two words", "tail", "{file}"]
        );
        assert_eq!(
            split_argument_template(r#""a""b" "c\\\"d" "#),
            ["a\"b", "c\\\"d", "{file}"]
        );
        assert_eq!(split_argument_template(r#""""#), ["", "{file}"]);
        assert_eq!(
            split_argument_template(r#""unterminated value"#),
            ["unterminated value", "{file}"]
        );
    }

    #[test]
    fn split_does_not_append_file_when_a_known_keyword_exists() {
        assert_eq!(
            split_argument_template("--input={file}"),
            ["--input={file}"]
        );
        assert_eq!(split_argument_template("-page {page}"), ["-page", "{page}"]);
    }

    #[test]
    fn expand_arguments_supports_every_p1_placeholder_without_loss_of_boundaries() {
        let ctx = PlaceholderContext::for_file(r"C:\My Images\photo.final.JPG");
        let tokens = vec![
            "{file}".into(),
            "{dir}".into(),
            "{name}".into(),
            "{stem}".into(),
            "{ext}".into(),
            "{uri}".into(),
        ];
        assert_eq!(
            expand_arguments(&tokens, &ctx),
            vec![
                OsString::from(r"C:\My Images\photo.final.JPG"),
                OsString::from(r"C:\My Images"),
                OsString::from("photo.final.JPG"),
                OsString::from("photo.final"),
                OsString::from("JPG"),
                OsString::from("file:///C:/My%20Images/photo.final.JPG"),
            ]
        );
    }

    #[test]
    fn expand_drops_empty_placeholder_tokens_and_their_option() {
        let ctx = PlaceholderContext::for_file(r"C:\image");
        let tokens = split_argument_template("-page {page} --entry={entry} literal");
        assert_eq!(expand_arguments(&tokens, &ctx), [OsString::from("literal")]);
    }

    #[test]
    fn expand_preserves_unknown_braces_and_adds_file_when_no_keyword_exists() {
        let ctx = PlaceholderContext::for_file(r"C:\space dir\image.png");
        let tokens = split_argument_template("--pattern={unknown}");
        assert_eq!(
            expand_arguments(&tokens, &ctx),
            [
                OsString::from("--pattern={unknown}"),
                OsString::from(r"C:\space dir\image.png")
            ]
        );
    }

    #[test]
    fn replacement_with_spaces_remains_one_argument() {
        let ctx = PlaceholderContext::for_file(r"C:\space dir\image.png");
        let tokens = split_argument_template("--input={file}");
        assert_eq!(
            expand_arguments(&tokens, &ctx),
            [OsString::from(r"--input=C:\space dir\image.png")]
        );
    }

    #[test]
    fn files_placeholder_expands_one_token_to_distinct_os_arguments() {
        let contexts = [
            PlaceholderContext::for_file(r"C:\space dir\one.png"),
            PlaceholderContext::for_file(r"D:\other dir\two.png"),
        ];
        assert_eq!(
            expand_arguments_for_files(&["--input={files}".to_string()], &contexts),
            [
                OsString::from(r"--input=C:\space dir\one.png"),
                OsString::from(r"--input=D:\other dir\two.png"),
            ]
        );
    }

    #[test]
    fn files_placeholder_keeps_other_placeholders_on_the_primary_target() {
        let contexts = [
            PlaceholderContext::for_file(r"C:\images\first one.jpg"),
            PlaceholderContext::for_file(r"D:\other\second.png"),
        ];
        assert_eq!(
            expand_arguments_for_files(&["--pair={file}:{files}".to_string()], &contexts),
            [
                OsString::from(r"--pair=C:\images\first one.jpg:C:\images\first one.jpg"),
                OsString::from(r"--pair=C:\images\first one.jpg:D:\other\second.png"),
            ]
        );
    }

    #[test]
    fn build_request_rejects_virtual_targets_and_ignores_shell_launch_arguments() {
        let mut tool = ExternalTool::defaults_for_viewing();
        assert!(
            build_launch_request(
                &tool,
                &LaunchTarget::Virtual(crate::grid_item::FileOperationRefusal::VirtualPage)
            )
            .is_err()
        );
        tool.arguments = "--ignored {file}".to_string();
        tool.working_directory = Some(PathBuf::from(r"C:\ignored"));
        let target = LaunchTarget::RealFile(PathBuf::from(r"C:\image.png"));

        for launch in [
            ExternalToolLaunch::Association {
                handler_id: "Photos.App".to_string(),
            },
            ExternalToolLaunch::OsDefault,
        ] {
            tool.launch = launch.clone();
            let request = build_launch_request(&tool, &target).unwrap();
            assert_eq!(request.launch, launch);
            assert!(request.arguments.is_empty());
            assert!(request.working_directory.is_none());
        }

        tool.launch = ExternalToolLaunch::Executable(PathBuf::from(r"C:\Tools\viewer.exe"));
        let request = build_launch_request(&tool, &target).unwrap();
        assert_eq!(request.files, [PathBuf::from(r"C:\image.png")]);
        assert_eq!(
            request.arguments,
            [OsString::from("--ignored"), OsString::from(r"C:\image.png")]
        );
        assert_eq!(
            request.working_directory,
            Some(PathBuf::from(r"C:\ignored"))
        );
    }

    fn real_targets(count: usize) -> Vec<LaunchTarget> {
        (0..count)
            .map(|index| LaunchTarget::RealFile(PathBuf::from(format!(r"C:\Images\{index}.png"))))
            .collect()
    }

    #[test]
    fn selection_policy_builds_single_each_and_executable_batch_plans() {
        let targets = real_targets(3);
        let mut tool = ExternalTool::defaults_for_viewing();
        tool.launch = ExternalToolLaunch::Executable(PathBuf::from(r"C:\Tools\viewer.exe"));

        let single = build_launch_operation(&tool, &targets).unwrap();
        assert_eq!(single.requests.len(), 1);
        assert_eq!(
            single.requests[0].files,
            [PathBuf::from(r"C:\Images\0.png")]
        );

        tool.selection = SelectionPolicy::Each;
        let each = build_launch_operation(&tool, &targets).unwrap();
        assert_eq!(each.requests.len(), 3);
        assert!(each.requests.iter().all(|request| request.files.len() == 1));

        tool.selection = SelectionPolicy::Batch;
        tool.arguments = "--input {files}".to_string();
        let batch = build_launch_operation(&tool, &targets).unwrap();
        assert_eq!(batch.requests.len(), 1);
        assert_eq!(batch.requests[0].files.len(), 3);
        assert_eq!(
            batch.requests[0].arguments,
            [
                OsString::from("--input"),
                OsString::from(r"C:\Images\0.png"),
                OsString::from(r"C:\Images\1.png"),
                OsString::from(r"C:\Images\2.png"),
            ]
        );
    }

    #[test]
    fn batch_launch_kind_controls_process_fanout() {
        let targets = real_targets(3);
        let mut tool = ExternalTool::defaults_for_viewing();
        tool.selection = SelectionPolicy::Batch;

        tool.launch = ExternalToolLaunch::Association {
            handler_id: "Photos.App".to_string(),
        };
        let association = build_launch_operation(&tool, &targets).unwrap();
        assert_eq!(association.requests.len(), 1);
        assert_eq!(association.requests[0].files.len(), 3);

        tool.launch = ExternalToolLaunch::OsDefault;
        let os_default = build_launch_operation(&tool, &targets).unwrap();
        assert_eq!(os_default.requests.len(), 3);
        assert!(
            os_default
                .requests
                .iter()
                .all(|request| request.files.len() == 1)
        );
    }

    #[test]
    fn batch_auto_adds_files_but_rejects_an_explicit_single_file_template() {
        let targets = real_targets(2);
        let mut tool = ExternalTool::defaults_for_viewing();
        tool.launch = ExternalToolLaunch::Executable(PathBuf::from(r"C:\Tools\viewer.exe"));
        tool.selection = SelectionPolicy::Batch;
        tool.arguments = "--readonly".to_string();
        let automatic = build_launch_operation(&tool, &targets).unwrap();
        assert_eq!(
            automatic.requests[0].arguments,
            [
                OsString::from("--readonly"),
                OsString::from(r"C:\Images\0.png"),
                OsString::from(r"C:\Images\1.png"),
            ]
        );

        tool.arguments = "--input {file}".to_string();
        assert!(
            build_launch_operation(&tool, &targets)
                .unwrap_err()
                .contains("{files}")
        );
    }

    #[test]
    fn virtual_targets_reject_the_whole_set_before_single_policy() {
        let tool = ExternalTool::defaults_for_viewing();
        for refusal in [
            crate::grid_item::FileOperationRefusal::VirtualPage,
            crate::grid_item::FileOperationRefusal::ArchiveDirectory,
            crate::grid_item::FileOperationRefusal::Stack,
        ] {
            let targets = [
                LaunchTarget::RealFile(PathBuf::from(r"C:\Images\real.png")),
                LaunchTarget::Virtual(refusal),
            ];
            assert!(build_launch_operation(&tool, &targets).is_err());
        }
    }

    #[test]
    fn each_confirmation_starts_after_twenty_and_os_default_batch_shares_the_limit() {
        let mut each = ExternalTool::defaults_for_viewing();
        each.selection = SelectionPolicy::Each;
        assert!(
            launch_confirmation(build_launch_operation(&each, &real_targets(20)).unwrap()).is_ok()
        );
        let confirmation =
            launch_confirmation(build_launch_operation(&each, &real_targets(21)).unwrap())
                .unwrap_err();
        assert_eq!(confirmation.many_launch_count, Some(21));

        let mut batch_default = each;
        batch_default.selection = SelectionPolicy::Batch;
        assert!(
            launch_confirmation(build_launch_operation(&batch_default, &real_targets(21)).unwrap())
                .is_err()
        );

        batch_default.launch = ExternalToolLaunch::Association {
            handler_id: "Photos.App".to_string(),
        };
        assert!(
            launch_confirmation(build_launch_operation(&batch_default, &real_targets(21)).unwrap())
                .is_ok()
        );
    }

    #[test]
    fn partial_completion_message_reports_success_failure_count_and_os_error() {
        let completion = ExternalLaunchCompletion {
            tool_name: "Viewer".to_string(),
            target_count: 3,
            succeeded_target_count: 2,
            failures: vec!["C:\\bad.png: access denied".to_string()],
            refreshes: Vec::new(),
        };
        let message = external_launch_completion_message(&completion);
        assert!(message.contains("2 件を起動"));
        assert!(message.contains("1 件の起動に失敗"));
        assert!(message.contains("access denied"));
    }

    #[test]
    fn simultaneous_operation_completions_are_combined_without_dropping_the_first() {
        let completions = vec![
            ExternalLaunchCompletion {
                tool_name: "first".to_string(),
                target_count: 1,
                succeeded_target_count: 0,
                failures: vec!["first.jpg: denied".to_string()],
                refreshes: Vec::new(),
            },
            ExternalLaunchCompletion {
                tool_name: "second".to_string(),
                target_count: 2,
                succeeded_target_count: 2,
                failures: Vec::new(),
                refreshes: Vec::new(),
            },
        ];

        let message = external_launch_completion_summary(&completions);
        assert!(message.contains("first: first.jpg: denied"));
        assert!(message.contains("second: 2 件を起動しました"));
        assert_eq!(message.lines().count(), 2);
    }

    #[test]
    fn legacy_recent_classification_is_pure_for_all_three_reported_shapes() {
        assert_eq!(
            classify_legacy_recent_launch(r"C:\Tools\viewer.exe", true),
            ExternalToolLaunch::Executable(PathBuf::from(r"C:\Tools\viewer.exe"))
        );
        assert_eq!(
            classify_legacy_recent_launch("フォト", false),
            ExternalToolLaunch::Association {
                handler_id: "フォト".to_string()
            }
        );
        assert_eq!(
            classify_legacy_recent_launch(r"C:\Missing\viewer.exe", false),
            ExternalToolLaunch::Association {
                handler_id: r"C:\Missing\viewer.exe".to_string()
            }
        );
    }

    #[test]
    fn launch_descriptions_make_non_executable_kinds_explicit() {
        let mut tool = ExternalTool::defaults_for_viewing();
        assert_eq!(tool.launch_description(), "OS の関連付け");
        tool.name = "フォト".to_string();
        tool.launch = ExternalToolLaunch::Association {
            handler_id: "Photos.App".to_string(),
        };
        assert_eq!(tool.launch_description(), "関連付けアプリ (フォト)");
        tool.launch = ExternalToolLaunch::Executable(PathBuf::from(r"C:\Tools\viewer.exe"));
        assert_eq!(tool.launch_description(), r"C:\Tools\viewer.exe");
    }

    #[test]
    fn external_launch_error_message_includes_the_tool_name() {
        assert_eq!(
            external_launch_error_message("フォト", "関連付けアプリが見つかりません"),
            "フォト: 関連付けアプリが見つかりません"
        );
    }

    #[test]
    fn recent_identity_replaces_a_legacy_executable_with_the_same_handler_id() {
        let executable = ExternalToolLaunch::Executable(PathBuf::from(r"C:\Tools\viewer.exe"));
        let association = ExternalToolLaunch::Association {
            handler_id: r"c:\tools\VIEWER.exe".to_string(),
        };
        assert!(executable.same_target(&association));
        assert!(association.same_target(&executable));
        assert!(!executable.same_target(&ExternalToolLaunch::OsDefault));
    }

    #[cfg(windows)]
    #[test]
    fn file_placeholder_preserves_non_unicode_windows_path() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        let raw = [
            b'C' as u16,
            b':' as u16,
            b'\\' as u16,
            0xd800,
            b'.' as u16,
            b'p' as u16,
        ];
        let path = PathBuf::from(OsString::from_wide(&raw));
        let arguments = expand_arguments(
            &["{file}".to_string()],
            &PlaceholderContext::for_file(&path),
        );
        assert_eq!(arguments[0].encode_wide().collect::<Vec<_>>(), raw.to_vec());
    }
}
