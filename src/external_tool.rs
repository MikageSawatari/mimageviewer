use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use eframe::egui;

pub const DEFAULT_PDF_RENDER_LONG_EDGE: u32 = 4096;
const CONTEXT_MENU_DEFAULT_ON_THRESHOLD: usize = 10;
const ASSOCIATED_APP_DISPLAY_NAME: &str = "関連付けアプリ";
pub(crate) const VIRTUAL_EDITING_DISABLED_REASON: &str = "圧縮ファイル内のページは編集用ツールで開けません。書き出してから編集してください (フルスクリーンで Ctrl+E)";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const IMPLEMENTED_PLACEHOLDERS: &[&str] =
    &["{file}", "{dir}", "{name}", "{stem}", "{ext}", "{uri}"];
const DEFERRED_PLACEHOLDERS: &[&str] = &[
    "{files}",
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
    pub(crate) fn from_grid_item(item: &crate::grid_item::GridItem) -> Self {
        use crate::grid_item::GridItem;
        match item {
            GridItem::Image(_)
            | GridItem::Video(_)
            | GridItem::Audio(_)
            | GridItem::ZipFile(_)
            | GridItem::PdfFile(_)
            | GridItem::ConvertibleArchive { .. } => Self::RealFile,
            GridItem::ZipImage { .. } | GridItem::PdfPage { .. } | GridItem::Stack { .. } => {
                Self::VirtualPage
            }
            GridItem::Folder(_) | GridItem::ZipDir { .. } | GridItem::SearchContainer { .. } => {
                Self::Unsupported
            }
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
    UnsupportedVirtual,
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
            Some(_) => Self::UnsupportedVirtual,
            None => Self::None,
        }
    }

    pub fn real_file(&self) -> Result<&Path, String> {
        match self {
            Self::RealFile(path) => Ok(path),
            Self::UnsupportedVirtual => {
                Err("仮想ページはこの段階では外部ツールへ渡せません".to_string())
            }
            Self::None => Err("外部ツールへ渡す実ファイルが選択されていません".to_string()),
        }
    }
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
    pub file: PathBuf,
}

#[derive(Debug)]
pub(crate) struct ExternalLaunchResult {
    pub tool_name: String,
    pub result: Result<Option<AssociationHandlerRefresh>, String>,
}

#[derive(Debug)]
pub(crate) struct AssociationHandlerRefresh {
    previous_id: String,
    current_id: String,
}

pub(crate) struct ExternalLaunchPending {
    pub cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<ExternalLaunchResult>,
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
pub fn split_argument_template(template: &str) -> Vec<String> {
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
        result.push("{file}".to_string());
    }
    result
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

fn expand_token(token: &str, ctx: &PlaceholderContext) -> Option<OsString> {
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
        if placeholder == "{uri}" {
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
    let mut result: Vec<OsString> = Vec::new();
    for token in tokens {
        match expand_token(token, ctx) {
            Some(expanded) => result.push(expanded),
            None => {
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
    }
    result
}

pub(crate) fn build_launch_request(
    tool: &ExternalTool,
    target: &LaunchTarget,
) -> Result<ExternalLaunchRequest, String> {
    let file = target.real_file()?.to_path_buf();
    let arguments = if tool.launch.uses_process_options() {
        let tokens = split_argument_template(&tool.arguments);
        expand_arguments(&tokens, &PlaceholderContext::for_file(&file))
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
        file,
    })
}

pub(crate) fn build_open_with_launch_request(
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
        file,
    }
}

pub(crate) fn executable_requires_confirmation(request: &ExternalLaunchRequest) -> bool {
    request.launch.executable().is_some_and(|path| {
        path.as_os_str()
            .as_encoded_bytes()
            .first()
            .is_some_and(|byte| *byte == b'\\')
    })
}

pub(crate) fn start_launch_worker(
    request: ExternalLaunchRequest,
) -> Result<ExternalLaunchPending, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("external-tool-launch".to_string())
        .spawn(move || {
            if cancel_worker.load(Ordering::Relaxed) {
                return;
            }
            let tool_name = request.tool_name.clone();
            let result = launch_request(request);
            if !cancel_worker.load(Ordering::Relaxed) {
                let _ = tx.send(ExternalLaunchResult { tool_name, result });
            }
        })
        .map_err(|error| format!("外部ツール起動 worker を開始できません: {error}"))?;
    Ok(ExternalLaunchPending { cancel, rx })
}

fn launch_request(
    request: ExternalLaunchRequest,
) -> Result<Option<AssociationHandlerRefresh>, String> {
    match request.launch {
        ExternalToolLaunch::Executable(executable) => {
            let mut command = Command::new(executable);
            command
                .args(request.arguments)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Some(directory) = request.working_directory {
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
            crate::open_with::invoke_association_handler(
                &handler_id,
                &request.tool_name,
                &request.file,
            )
            .map(|outcome| {
                outcome
                    .refreshed_handler_id
                    .map(|current_id| AssociationHandlerRefresh {
                        previous_id: handler_id,
                        current_id,
                    })
            })
        }
        ExternalToolLaunch::OsDefault => opener::open(request.file)
            .map(|_| None)
            .map_err(|error| error.to_string()),
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

impl crate::app::App {
    pub(crate) fn queue_external_tool_launch(
        &mut self,
        tool: &ExternalTool,
        target: &LaunchTarget,
    ) {
        let request = match build_launch_request(tool, target) {
            Ok(request) => request,
            Err(error) => {
                self.show_feedback_toast(format!("{}: {error}", tool.display_name()));
                return;
            }
        };
        if executable_requires_confirmation(&request) {
            self.external_tool_launch_confirmation = Some(request);
        } else {
            self.start_external_launch_request(request);
        }
    }

    pub(crate) fn start_open_with(
        &mut self,
        display_name: String,
        launch: ExternalToolLaunch,
        file: PathBuf,
    ) {
        let request = build_open_with_launch_request(display_name.clone(), launch, file);
        if executable_requires_confirmation(&request) {
            self.external_tool_launch_confirmation = Some(request);
        } else {
            self.start_external_launch_request(request);
        }
    }

    fn start_external_launch_request(&mut self, request: ExternalLaunchRequest) {
        let tool_name = request.tool_name.clone();
        match start_launch_worker(request) {
            Ok(pending) => self.external_tool_launch_pending.push(pending),
            Err(error) => self.show_feedback_toast(format!("{tool_name}: {error}")),
        }
    }

    pub(crate) fn poll_external_tool_launch(&mut self, ctx: &egui::Context) {
        if self.external_tool_launch_pending.is_empty() {
            return;
        }
        // 全 slot を見る。1 つだけ見て早期 return すると、同時に走った別の起動の結果が
        // 次の frame まで残り、失敗 toast が遅れる。
        let mut finished: Vec<Option<ExternalLaunchResult>> = Vec::new();
        self.external_tool_launch_pending
            .retain(|pending| match pending.rx.try_recv() {
                Ok(result) => {
                    finished.push(Some(result));
                    false
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    finished.push(None);
                    false
                }
                Err(mpsc::TryRecvError::Empty) => true,
            });
        for outcome in finished {
            match outcome {
                Some(result) => match result.result {
                    Ok(refresh) => {
                        if let Some(refresh) = refresh {
                            write_back_association_handler_id(&mut self.settings, &refresh);
                        }
                        self.show_feedback_toast(format!("{} を起動しました", result.tool_name))
                    }
                    Err(error) => self.show_feedback_toast(external_launch_error_message(
                        &result.tool_name,
                        &error,
                    )),
                },
                // worker が結果を送らずに落ちた (panic 等)。黙って消すと「押したのに
                // 何も起きない」になるので、起きたことは伝える。
                None => self
                    .show_feedback_toast("外部ツールの起動結果を受け取れませんでした".to_string()),
            }
        }
        if !self.external_tool_launch_pending.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    pub(crate) fn show_external_tool_launch_confirmation(&mut self, ctx: &egui::Context) {
        let Some(request) = self.external_tool_launch_confirmation.as_ref() else {
            return;
        };
        let tool_name = request.tool_name.clone();
        let executable = request
            .launch
            .executable()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let mut launch = false;
        let mut cancel = false;
        let response =
            egui::Modal::new(egui::Id::new("external_tool_network_confirmation")).show(ctx, |ui| {
                ui.set_min_width(440.0);
                ui.heading("ネットワーク上のツールを起動しますか？");
                ui.add_space(8.0);
                ui.label(format!("ツール: {tool_name}"));
                ui.label(format!("実行ファイル: {executable}"));
                ui.add_space(8.0);
                ui.label("信頼できる場所であることを確認してから起動してください。");
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
            if let Some(request) = self.external_tool_launch_confirmation.take() {
                self.start_external_launch_request(request);
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
            ExternalToolMenuTarget::from_grid_item(&crate::grid_item::GridItem::Image(
                PathBuf::from(r"C:\Images\page.jpg"),
            )),
            ExternalToolMenuTarget::RealFile
        );
        assert_eq!(
            ExternalToolMenuTarget::from_grid_item(&crate::grid_item::GridItem::ZipImage {
                zip_path: PathBuf::from(r"C:\Books\book.zip"),
                entry_name: "page.jpg".to_string(),
            }),
            ExternalToolMenuTarget::VirtualPage
        );
        assert_eq!(
            ExternalToolMenuTarget::from_grid_item(&crate::grid_item::GridItem::Folder(
                PathBuf::from(r"C:\Images"),
            )),
            ExternalToolMenuTarget::Unsupported
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
    fn build_request_rejects_virtual_targets_and_ignores_shell_launch_arguments() {
        let mut tool = ExternalTool::defaults_for_viewing();
        assert!(build_launch_request(&tool, &LaunchTarget::UnsupportedVirtual).is_err());
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
        assert_eq!(
            request.arguments,
            [OsString::from("--ignored"), OsString::from(r"C:\image.png")]
        );
        assert_eq!(
            request.working_directory,
            Some(PathBuf::from(r"C:\ignored"))
        );
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
