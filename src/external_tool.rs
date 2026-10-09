use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use eframe::egui;

pub const DEFAULT_PDF_RENDER_LONG_EDGE: u32 = 4096;
pub const DEFAULT_CONFIRMATION_THRESHOLD: u32 = 5;
pub const DEFAULT_MAX_TARGETS: u32 = 10;
#[cfg(windows)]
const CREATE_PROCESS_COMMAND_LINE_MAX_UTF16_UNITS: usize = 32_767;
const ASSOCIATED_APP_DISPLAY_NAME: &str = "関連付けアプリ";
pub(crate) const VIRTUAL_ORIGINAL_FILE_DISABLED_REASON: &str = "圧縮ファイル / PDF 内のページには元のファイルがありません。「一時ファイル」を渡す設定にするか、書き出してから編集してください (フルスクリーンで Ctrl+E)";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Argument placeholders are replaced within already split tokens.
const IMPLEMENTED_PLACEHOLDERS: &[&str] = &["{files}", "{file_list}"];

#[doc(hidden)]
pub fn draw_external_tool_placeholder_help(ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("{files} / {file_list}").strong());
    ui.add(egui::Label::new(egui::RichText::new(concat!(
        "{files} は渡すファイルのパスです。「1 件ずつ」なら 1 つ、「まとめて渡す」なら選んだ数だけ並びます。",
        "{file_list} は渡すファイルを 1 行 1 パスで記録した一覧ファイルのパスです (UTF-8、BOM なし、CRLF)。",
        "どちらも書かないときは {files} が 1 つ付きます。",
    )).weak()).wrap());
}

#[doc(hidden)]
pub fn draw_external_tool_preparing_body(ui: &mut egui::Ui, count: usize) -> bool {
    ui.heading("外部ツールへ渡す対象を準備中");
    ui.label(format!("{count} 件の準備ができました"));
    ui.spinner();
    ui.button("キャンセル").clicked()
}

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
/// 外部ツールへ何を渡すか。
///
/// **一時ファイルと元ファイルを 1 つの値の中で混ぜない** (§4.3 の 2026-09-02 決定)。
/// 混ぜると、同じ設定・同じツールでもページによって上書き保存の意味が変わる
/// (無加工のページだけ実ファイルが渡り、利用者が元データを壊す)。加工の有無は
/// ツール側から見えないので、保存する前に区別する手段が無い。
pub enum PayloadPolicy {
    /// 一時ファイル (編集を反映)。**常に再エンコードする** — 編集の有無で拡張子や
    /// EXIF の有無が変わると、受け取る側から区別できない (利用者判断 2026-09-02)。
    #[default]
    TempEdited,
    /// 一時ファイル (編集前)。**常に元バイト列そのまま。**
    TempOriginal,
    /// 元のファイルそのもの。仮想ページでは起動しない (§4.8)。
    OriginalFile,
}

impl PayloadPolicy {
    /// 元のバイト列をそのまま渡す約束か。
    ///
    /// **合成した見開きとは両立しない** — 2 ページを 1 枚にしたものに対応する「元の
    /// データ」はどこにも無い。`ExternalTool::effective_spread` がこの規則を持つ。
    pub fn passes_original_bytes(self) -> bool {
        matches!(self, Self::TempOriginal | Self::OriginalFile)
    }
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
    Single,
    #[default]
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
    #[serde(default)]
    pub show_console: bool,
    pub payload: PayloadPolicy,
    pub video: VideoPolicy,
    pub spread: SpreadPolicy,
    pub selection: SelectionPolicy,
    pub confirmation_threshold: u32,
    pub max_targets: u32,
    pub pdf_render_long_edge: u32,
    pub keep_temp: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ExternalToolMenuTarget {
    has_target: bool,
    has_virtual_page: bool,
    has_unsupported: bool,
}

impl ExternalToolMenuTarget {
    fn add_target(&mut self, target: &LaunchTarget) {
        let other = Self::from_launch_targets(std::slice::from_ref(target));
        self.has_target |= other.has_target;
        self.has_virtual_page |= other.has_virtual_page;
        self.has_unsupported |= other.has_unsupported;
    }

    fn add_grid_item(&mut self, item: &crate::grid_item::GridItem) {
        use crate::grid_item::GridItem;
        match item {
            GridItem::Image(_)
            | GridItem::Video(_)
            | GridItem::Audio(_)
            | GridItem::ZipFile(_)
            | GridItem::PdfFile(_)
            | GridItem::ConvertibleArchive { .. }
            | GridItem::Stack { .. } => self.has_target = true,
            GridItem::ZipImage { .. } | GridItem::PdfPage { .. } => {
                self.has_target = true;
                self.has_virtual_page = true;
            }
            _ => self.has_unsupported = true,
        }
    }
    /// 解決済み集合からメニューの capability を決める。
    ///
    pub(crate) fn from_launch_targets(targets: &[LaunchTarget]) -> Self {
        let mut capability = Self::default();
        for target in targets {
            match target {
                LaunchTarget::RealFile(_) | LaunchTarget::ImagePage(_) | LaunchTarget::Stack(_) => {
                    capability.has_target = true;
                }
                LaunchTarget::ZipPage { .. } | LaunchTarget::PdfPage { .. } => {
                    capability.has_target = true;
                    capability.has_virtual_page = true;
                }
                LaunchTarget::Virtual(_) | LaunchTarget::Unsupported | LaunchTarget::None => {
                    capability.has_unsupported = true;
                }
            }
        }
        capability
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
struct ExternalToolPickerItem {
    tool_id: ExternalToolId,
    slot: usize,
    label: String,
    enabled: bool,
    disabled_reason: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchTarget {
    RealFile(PathBuf),
    ImagePage(PathBuf),
    ZipPage {
        zip_path: PathBuf,
        entry_name: String,
    },
    PdfPage {
        pdf_path: PathBuf,
        page_num: u32,
    },
    Stack(String),
    Virtual(crate::grid_item::FileOperationRefusal),
    Unsupported,
    None,
}

impl LaunchTarget {
    pub(crate) fn from_grid_item(item: Option<&crate::grid_item::GridItem>) -> Self {
        use crate::grid_item::GridItem;
        match item {
            Some(GridItem::Image(path)) => Self::ImagePage(path.clone()),
            Some(
                GridItem::Video(path)
                | GridItem::Audio(path)
                | GridItem::ZipFile(path)
                | GridItem::PdfFile(path),
            ) => Self::RealFile(path.clone()),
            Some(GridItem::ConvertibleArchive { path, .. }) => Self::RealFile(path.clone()),
            Some(GridItem::ZipImage {
                zip_path,
                entry_name,
            }) => Self::ZipPage {
                zip_path: zip_path.clone(),
                entry_name: entry_name.clone(),
            },
            Some(GridItem::PdfPage {
                pdf_path, page_num, ..
            }) => Self::PdfPage {
                pdf_path: pdf_path.clone(),
                page_num: *page_num,
            },
            Some(GridItem::Stack { key, .. }) => Self::Stack(key.clone()),
            Some(GridItem::ZipDir { .. }) => {
                Self::Virtual(crate::grid_item::FileOperationRefusal::ArchiveDirectory)
            }
            Some(_) => Self::Unsupported,
            None => Self::None,
        }
    }

    pub fn real_file(&self) -> Result<&Path, String> {
        match self {
            Self::RealFile(path) | Self::ImagePage(path) => Ok(path),
            Self::ZipPage { .. } | Self::PdfPage { .. } | Self::Stack(_) | Self::Virtual(_) => {
                Err("仮想ページは実体化してから外部ツールへ渡します".to_string())
            }
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

/// キー操作から開く外部ツール選択モーダルの対象種別。
///
/// 対象そのものは [`ExternalToolPickerRequest::targets`] にスナップショットする。
/// モーダル表示中にグリッドの選択や現在地が変わっても、別の項目を起動しないためである。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExternalToolPickerTargetKind {
    GridItems,
    Container,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExternalToolPickerRequest {
    locators: Vec<ExternalTargetLocator>,
    capability: ExternalToolMenuTarget,
    owner: crate::app::ViewerContextId,
    has_stack: bool,
    target_kind: ExternalToolPickerTargetKind,
    items_generation: u64,
}

/// Cheap event-time source descriptor. Bulk target snapshots are made only in Preparing.
#[derive(Clone, Debug)]
pub(crate) struct ExternalTargetDescriptor {
    owner: crate::app::ViewerContextId,
    items_generation: u64,
    pub(crate) source: ExternalTargetSource,
    pub(crate) capability: ExternalToolMenuTarget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExternalTargetLocator {
    Listed {
        owner: crate::app::ViewerContextId,
        items_generation: u64,
        index: usize,
        target: LaunchTarget,
    },
    OutsideList {
        target: LaunchTarget,
    },
}

impl ExternalTargetLocator {
    fn target(&self) -> &LaunchTarget {
        match self {
            Self::Listed { target, .. } | Self::OutsideList { target } => target,
        }
    }
}

enum ExternalPreparationPurpose {
    ForTool(ExternalTool),
    ForPicker(ExternalToolPickerTargetKind),
}

enum ExternalPreparationInput {
    Source(ExternalTargetSource),
    Captured,
    Legacy(Vec<LaunchTarget>),
}

enum ExternalPreparationStage {
    Enumerating { cursor: usize },
    LegacyIndex { cursor: usize },
    StackIndex { cursor: usize },
    Snapshot { cursor: usize, member_cursor: usize },
    Complete,
}

#[derive(Default)]
enum ExternalTargetValidation {
    #[default]
    Valid,
    Unsupported,
    Virtual(crate::grid_item::FileOperationRefusal),
    Missing,
}

impl ExternalTargetValidation {
    fn record(&mut self, target: &LaunchTarget) {
        match target {
            LaunchTarget::None => *self = Self::Missing,
            LaunchTarget::Virtual(refusal) if matches!(self, Self::Valid | Self::Unsupported) => {
                *self = Self::Virtual(*refusal)
            }
            LaunchTarget::Unsupported if matches!(self, Self::Valid) => *self = Self::Unsupported,
            _ => {}
        }
    }

    fn finish(&self) -> Result<(), String> {
        let target = match self {
            Self::Valid => return Ok(()),
            Self::Unsupported => LaunchTarget::Unsupported,
            Self::Virtual(refusal) => LaunchTarget::Virtual(*refusal),
            Self::Missing => LaunchTarget::None,
        };
        validate_materializable_targets(std::slice::from_ref(&target))
    }
}

#[derive(Hash, PartialEq, Eq)]
enum ExternalPageIdentity {
    Image(String),
    Zip(String, String),
    Pdf(String, u32),
}

fn external_page_identity(target: &LaunchTarget) -> Option<ExternalPageIdentity> {
    match target {
        LaunchTarget::ImagePage(path) => Some(ExternalPageIdentity::Image(
            crate::adjustment_db::normalize_path(path),
        )),
        LaunchTarget::ZipPage {
            zip_path,
            entry_name,
        } => Some(ExternalPageIdentity::Zip(
            crate::adjustment_db::normalize_path(zip_path),
            entry_name.clone(),
        )),
        LaunchTarget::PdfPage { pdf_path, page_num } => Some(ExternalPageIdentity::Pdf(
            crate::adjustment_db::normalize_path(pdf_path),
            *page_num,
        )),
        _ => None,
    }
}

struct ExternalRequestSnapshots {
    stage: crate::bake_stage::BakeStage,
    raw_brightness: crate::raw::RawBrightness,
    global_params: crate::adjustment::AdjustParams,
    conceal_preset: crate::conceal::ConcealPreset,
    erase_mono_tolerance: u8,
    creative_luts: crate::creative_lut::CreativeLutSnapshot,
    ai_materials: Option<crate::books::BookAiMaterials>,
    video_frame: Option<(PathBuf, u64)>,
    spread: Option<(usize, [usize; 2], [usize; 2])>,
}

pub(crate) struct ExternalPreparing {
    owner: crate::app::ViewerContextId,
    items_generation: u64,
    purpose: ExternalPreparationPurpose,
    input: ExternalPreparationInput,
    stage: ExternalPreparationStage,
    snapshots: ExternalRequestSnapshots,
    locators: Vec<ExternalTargetLocator>,
    requests: Vec<crate::materializer::MaterializeRequest>,
    page_indices: HashMap<ExternalPageIdentity, usize>,
    stack_indices: HashMap<String, usize>,
    capability: ExternalToolMenuTarget,
    validation: ExternalTargetValidation,
    has_stack: bool,
    last_frame: Option<u64>,
    scanned_entries: usize,
}

pub(crate) enum ExternalLaunching {
    Materialized(ExternalMaterializePending),
    Ready(ExternalLaunchPending),
}

/// Exactly one active request owns preparation, confirmation and launch. Retired workers
/// are drained separately; their presence never implies a visible modal.
#[derive(Default)]
pub(crate) enum ExternalOperation {
    #[default]
    Idle,
    Preparing(ExternalPreparing),
    Picker(ExternalToolPickerRequest),
    Confirmation(ExternalLaunchConfirmation),
    Materializing(ExternalMaterializePending),
    Launching(ExternalLaunching),
}

impl ExternalOperation {
    pub(crate) fn modal_reason(&self) -> Option<&'static str> {
        match self {
            Self::Preparing(_) => Some("external_tool_preparing"),
            Self::Picker(_) => Some("external_tool_picker"),
            Self::Confirmation(_) => Some("external_tool_launch_confirmation"),
            _ => None,
        }
    }
}

const EXTERNAL_PREPARATION_FRAME_ENTRIES: usize = 128;
const EXTERNAL_PREPARATION_FRAME_TIME: std::time::Duration = std::time::Duration::from_millis(2);

struct ExternalPreparationBudget {
    started: std::time::Instant,
    entries: usize,
}

impl ExternalPreparationBudget {
    fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            entries: 0,
        }
    }
    fn next(&mut self) -> bool {
        if self.entries >= EXTERNAL_PREPARATION_FRAME_ENTRIES
            || self.started.elapsed() >= EXTERNAL_PREPARATION_FRAME_TIME
        {
            return false;
        }
        self.entries += 1;
        true
    }
}

fn ordered_checked_indices(checked: &HashSet<usize>, display_order: &[usize]) -> Vec<usize> {
    display_order
        .iter()
        .copied()
        .filter(|index| checked.contains(index))
        .collect()
}

/// 発火面、checked、実際の表示順から対象集合を一度だけ解決する純関数。
///
/// `display_order` には `App::current_grid_order()` を渡す。詳細表示の列ソートを保ち、
/// item index 順へ並べ直したり、クリック / 現在項目を先頭へ移したりしてはならない。
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
                ordered_checked_indices(checked, display_order)
            }
        }
        ExternalTargetSource::GridKey { selected } => {
            if checked.is_empty() {
                selected.into_iter().collect()
            } else {
                ordered_checked_indices(checked, display_order)
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

/// 現在地から外部ツールへ渡すコンテナー 1 件を解決する純関数。
///
/// `unavailable` はドライブ一覧・検索結果一覧・スナップショット等、表示中の items が
/// `effective_folder` そのものの内容ではない場合に立てる。古い物理パスを誤って渡さない。
pub(crate) fn resolve_external_container_targets(
    effective_folder: Option<&Path>,
    unavailable: bool,
) -> Vec<LaunchTarget> {
    if unavailable {
        return Vec::new();
    }
    effective_folder
        .map(|path| LaunchTarget::RealFile(path.to_path_buf()))
        .into_iter()
        .collect()
}

/// 1-based の固定キースロットから登録済みツールを解決する純関数。
pub(crate) fn resolve_external_tool_slot(
    tools: &[ExternalTool],
    slot: usize,
) -> Result<&ExternalTool, String> {
    if !(1..=10).contains(&slot) {
        return Err(format!(
            "外部ツールスロット {slot} は使用できません (1〜10 を指定してください)"
        ));
    }
    let index = slot - 1;
    tools
        .get(index)
        .ok_or_else(|| format!("外部ツールスロット {slot} にはツールが登録されていません"))
}

#[derive(Debug)]
pub(crate) struct ExternalLaunchRequest {
    pub tool_name: String,
    pub launch: ExternalToolLaunch,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub show_console: bool,
    pub files: Vec<PathBuf>,
}

#[derive(Debug)]
pub(crate) struct ExternalLaunchOperation {
    tool_name: String,
    requests: Vec<ExternalLaunchRequest>,
    target_count: usize,
    target_count_decision: TargetCountDecision,
}

struct ExternalMaterializeOperation {
    tool: ExternalTool,
    targets: Vec<crate::materializer::MaterializeRequest>,
    target_count_decision: TargetCountDecision,
}

impl ExternalMaterializeOperation {
    fn tool_name(&self) -> String {
        self.tool.display_name()
    }

    fn target_count(&self) -> usize {
        self.targets.len()
    }
}

enum ExternalQueuedOperation {
    Ready(ExternalLaunchOperation),
    Materialize(ExternalMaterializeOperation),
}

impl std::fmt::Debug for ExternalQueuedOperation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExternalQueuedOperation")
            .field("tool_name", &self.tool_name())
            .field("target_count", &self.target_count())
            .finish()
    }
}

impl ExternalQueuedOperation {
    fn tool_name(&self) -> String {
        match self {
            Self::Ready(operation) => operation.tool_name.clone(),
            Self::Materialize(operation) => operation.tool_name(),
        }
    }

    fn target_count(&self) -> usize {
        match self {
            Self::Ready(operation) => operation.target_count,
            Self::Materialize(operation) => operation.target_count(),
        }
    }

    fn target_count_decision(&self) -> TargetCountDecision {
        match self {
            Self::Ready(operation) => operation.target_count_decision,
            Self::Materialize(operation) => operation.target_count_decision,
        }
    }

    fn launch(&self) -> &ExternalToolLaunch {
        match self {
            Self::Ready(operation) => &operation.requests[0].launch,
            Self::Materialize(operation) => &operation.tool.launch,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TargetCountDecision {
    Proceed,
    Confirm { target_count: usize },
}

#[derive(Debug)]
pub(crate) struct ExternalLaunchConfirmation {
    operation: ExternalQueuedOperation,
    network_executable: Option<PathBuf>,
}

struct MaterializeProgress {
    completed: AtomicUsize,
    total: usize,
    stage: std::sync::Mutex<String>,
}

impl MaterializeProgress {
    fn new(total: usize) -> Self {
        Self {
            completed: AtomicUsize::new(0),
            total,
            stage: std::sync::Mutex::new("準備を開始しています".to_string()),
        }
    }

    fn update(&self, completed: usize, stage: impl Into<String>) {
        self.completed.store(completed, Ordering::Release);
        *self.stage.lock().unwrap_or_else(|error| error.into_inner()) = stage.into();
    }

    fn snapshot(&self) -> (usize, usize, String) {
        (
            self.completed.load(Ordering::Acquire),
            self.total,
            self.stage
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone(),
        )
    }
}

pub(crate) struct ExternalMaterializePending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<ExternalLaunchCompletion>,
    launch_boundary_rx: mpsc::Receiver<()>,
    launch_decision_tx: Option<mpsc::Sender<MaterializeLaunchDecision>>,
    progress: Arc<MaterializeProgress>,
    generation: u64,
    worker: Option<std::thread::JoinHandle<()>>,
    user_cancelled: bool,
    /// この frame の progress UI で Cancel / Esc を処理済みなら true。
    /// frame tail の launch authorization が consume する。
    launch_ui_checkpoint_passed: bool,
}

impl ExternalMaterializePending {
    fn resolve_launch_boundary(&mut self, launch: bool) {
        let ready = match self.launch_boundary_rx.try_recv() {
            Ok(()) => true,
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => {
                // ここに来ると decision を返す口を閉じるので、worker は永久に待つ。
                crate::logger::log(
                    "external_tool: boundary channel disconnected; decision can no longer be sent"
                        .to_string(),
                );
                self.launch_decision_tx = None;
                false
            }
        };
        if ready && let Some(tx) = self.launch_decision_tx.take() {
            let decision = if launch {
                MaterializeLaunchDecision::Launch
            } else {
                MaterializeLaunchDecision::Cancel
            };
            let _ = tx.send(decision);
        }
    }

    fn poll_completion(&mut self) -> Option<ExternalLaunchCompletion> {
        match self.rx.try_recv() {
            Ok(completion) => {
                if let Some(worker) = self.worker.take() {
                    let _ = worker.join();
                }
                Some(completion)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                if let Some(worker) = self.worker.take() {
                    let _ = worker.join();
                }
                Some(ExternalLaunchCompletion {
                    tool_name: "外部ツール".to_string(),
                    target_count: self.progress.total,
                    succeeded_target_count: 0,
                    failures: vec!["外部ツールの準備結果を受け取れませんでした".to_string()],
                    refreshes: Vec::new(),
                })
            }
        }
    }

    fn can_cancel_before_launch(&self) -> bool {
        self.launch_decision_tx.is_some() && !self.cancel.load(Ordering::Acquire)
    }

    fn mark_launch_ui_checkpoint(&mut self) {
        self.launch_ui_checkpoint_passed = true;
    }

    fn take_launch_ui_checkpoint(&mut self) -> bool {
        std::mem::take(&mut self.launch_ui_checkpoint_passed)
    }

    fn cancel(&mut self, user_cancelled: bool) {
        self.cancel.store(true, Ordering::Release);
        if let Some(tx) = self.launch_decision_tx.take() {
            let _ = tx.send(MaterializeLaunchDecision::Cancel);
        }
        self.user_cancelled |= user_cancelled;
    }

    fn join_for_exit(&mut self) {
        self.cancel(false);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for ExternalMaterializePending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(tx) = self.launch_decision_tx.take() {
            let _ = tx.send(MaterializeLaunchDecision::Cancel);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MaterializeLaunchDecision {
    Launch,
    Cancel,
}

#[derive(Debug)]
struct ExternalLaunchAttemptResult {
    target_count: usize,
    target_label: String,
    pub result: Result<ExternalLaunchOutcome, String>,
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
                        Ok(outcome) => {
                            // 起動できなかった対象は成功に数えず、件ごとに理由を出す
                            // (v3.5.0 レビュー F02)。
                            self.succeeded_target_count += attempt
                                .target_count
                                .saturating_sub(outcome.failed_files.len());
                            for (path, reason) in &outcome.failed_files {
                                self.failures.push(format!("{}: {reason}", path.display()));
                            }
                            if let Some(refresh) = outcome.refresh {
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
    /// 実際に使う見開きの扱い。
    ///
    /// **元バイト列を渡す設定では合成しない。** 合成した 1 枚に対応する元のデータは
    /// 無いので、`TempOriginal` / `OriginalFile` と `Merged` は同時に成り立たない。
    /// 設定 UI はこの値を書き戻すので、通常は `spread` と一致する。保存済みの設定に
    /// 矛盾が残っていても、実行時に合成が起きないことをここで保証する
    /// (Codex Sol 指摘 #4)。
    pub fn effective_spread(&self) -> SpreadPolicy {
        if self.payload.passes_original_bytes() && self.spread == SpreadPolicy::Merged {
            SpreadPolicy::MainPageOnly
        } else {
            self.spread
        }
    }

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

    /// 一覧の行に出す短い要約。`launch_description` はフルパスを返すので、
    /// 一覧に置くと横幅がダイアログを突き抜ける (2026-08-31 利用者報告)。
    /// 一覧は「どれを選ぶか」が分かればよく、フルパスと引数は下の詳細に出ている。
    pub fn launch_summary(&self) -> String {
        match &self.launch {
            ExternalToolLaunch::Executable(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            ExternalToolLaunch::Association { .. } => "関連付けアプリ".to_string(),
            ExternalToolLaunch::OsDefault => "OS の関連付け".to_string(),
        }
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
            arguments: "{files}".to_string(),
            working_directory: None,
            show_console: false,
            payload: PayloadPolicy::TempEdited,
            video: VideoPolicy::File,
            spread: SpreadPolicy::Merged,
            selection: SelectionPolicy::Each,
            confirmation_threshold: DEFAULT_CONFIRMATION_THRESHOLD,
            max_targets: DEFAULT_MAX_TARGETS,
            pdf_render_long_edge: DEFAULT_PDF_RENDER_LONG_EDGE,
            keep_temp: false,
        }
    }

    pub fn defaults_for_editing() -> Self {
        Self {
            payload: PayloadPolicy::OriginalFile,
            spread: SpreadPolicy::MainPageOnly,
            ..Self::defaults_for_viewing()
        }
    }

    /// `payload` と両立しない下位設定を、保存前に揃える。
    ///
    /// 「効かない選択が設定画面に残っている」状態を作らないための正規化で、読み出し側に
    /// `effective_video()` のような派生を増やさない。保存値そのものを常に整合させる。
    pub fn normalize_for_payload(&mut self) {
        if self.payload != PayloadPolicy::OriginalFile {
            return;
        }
        // 一時 PNG のフレームは「元のファイル」ではない。
        self.video = VideoPolicy::File;
        // 合成した見開きに対応する元ファイルは存在しない。両ページは実ファイル 2 件
        // なので成立する。
        if self.spread == SpreadPolicy::Merged {
            self.spread = SpreadPolicy::MainPageOnly;
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

/// 外部ツールの右クリック項目を登録順に組み立てる。
///
/// ファイル存在確認や補正 DB 参照は行わず、項目種別とツール定義だけで判定する。
pub(crate) fn external_tool_menu_items(
    tools: &[ExternalTool],
    target: ExternalToolMenuTarget,
) -> Vec<ExternalToolMenuItem> {
    tools
        .iter()
        .filter_map(|tool| {
            let (enabled, disabled_reason) = external_tool_capability(tool, target)?;
            Some(ExternalToolMenuItem {
                tool_id: tool.id,
                label: tool.menu_label(),
                enabled,
                disabled_reason,
            })
        })
        .collect()
}

fn spread_both_pages_order(
    pages_in_reading_order: &[crate::ui_fullscreen::SpreadPageOccurrence],
) -> Option<[usize; 2]> {
    match pages_in_reading_order {
        [first, second] => Some([first.idx, second.idx]),
        _ => None,
    }
}

fn merged_spread_screen_order(left: usize, right: usize) -> [usize; 2] {
    [left, right]
}

/// この viewport が外部ツールの modal を描くか。
///
/// **所有者は「利用者がその操作をした窓」**で、先着ではない。viewer の viewport は
/// main の tail より前に描くので、先着にすると背面の F12 窓が main 由来の modal を
/// 攫い、前面の main は「ダイアログが無いのに入力だけ止まる」になる。
///
/// 所有 viewport は消えることがある (フルスクリーンを閉じる context menu からの起動、
/// native 動画の早期 return)。その frame は main が肩代わりする。main の tail は全 viewer
/// viewport より後に走るので、`owner_drawn_this_frame` はその時点で確定している。
fn external_tool_modal_viewport_draws(
    viewport: egui::ViewportId,
    owner: egui::ViewportId,
    owner_drawn_this_frame: bool,
) -> bool {
    viewport == owner || (viewport == egui::ViewportId::ROOT && !owner_drawn_this_frame)
}

fn external_tool_capability(
    tool: &ExternalTool,
    target: ExternalToolMenuTarget,
) -> Option<(bool, Option<&'static str>)> {
    if !target.has_target || target.has_unsupported {
        return None;
    }
    // 「元のファイル」のツールだけは、無効でも**隠さずグレーで理由を出す** (§4.8)。
    // 利用者は「このツールで編集できる」と思っているので、黙って消えるより理由が見えた
    // 方がよい。
    let virtual_refusal = target.has_virtual_page && tool.payload == PayloadPolicy::OriginalFile;
    Some((
        !virtual_refusal,
        virtual_refusal.then_some(VIRTUAL_ORIGINAL_FILE_DISABLED_REASON),
    ))
}

#[cfg(test)]
fn external_tool_picker_items(
    tools: &[ExternalTool],
    targets: &[LaunchTarget],
) -> Vec<ExternalToolPickerItem> {
    let target = ExternalToolMenuTarget::from_launch_targets(targets);
    external_tool_picker_items_for_capability(tools, target)
}

#[cfg(test)]
impl ExternalTargetDescriptor {
    pub(crate) fn for_test_item(app: &crate::app::App, item: &crate::grid_item::GridItem) -> Self {
        let mut capability = ExternalToolMenuTarget::default();
        capability.add_grid_item(item);
        Self {
            owner: app.edit_request_owner_context(),
            items_generation: app.items_generation,
            source: ExternalTargetSource::Viewer { current: Some(0) },
            capability,
        }
    }

    pub(crate) fn targets_for_test(&self, app: &crate::app::App) -> Vec<LaunchTarget> {
        resolve_external_targets(
            &app.items,
            app.current_grid_order(),
            &app.checked,
            self.source.clone(),
        )
    }
}

fn external_tool_picker_items_for_capability(
    tools: &[ExternalTool],
    target: ExternalToolMenuTarget,
) -> Vec<ExternalToolPickerItem> {
    tools
        .iter()
        .enumerate()
        .filter_map(|(index, tool)| {
            let (enabled, disabled_reason) = external_tool_capability(tool, target)?;
            Some(ExternalToolPickerItem {
                tool_id: tool.id,
                slot: index + 1,
                label: tool.display_name(),
                enabled,
                disabled_reason,
            })
        })
        .collect()
}

/// Windows の `CommandLineToArgvW` と同じ引用符・バックスラッシュ規則で、
/// 引数テンプレートを先にトークンへ分割する。
fn split_argument_template_with_default(template: &str) -> Vec<String> {
    // 渡すファイルの綴りは `{files}` 1 つ。**`{file}` は同義として受けるだけで、
    // 設定画面にも文書にも出さない。** 複数選択を「1 件ずつ」と「まとめて渡す」で
    // 切り替えるたびに引数を書き直すのは不便で、2 つの綴りを持つ理由が無い
    // (2026-09-02 利用者判断)。開発中に書いた古いテンプレートが literal `{file}` を
    // ツールへ渡してしまわないよう、ここで 1 つに揃える。
    let template = &template.replace("{file}", "{files}");
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
        result.push("{files}".to_string());
    }
    result
}

pub fn split_argument_template(template: &str) -> Vec<String> {
    split_argument_template_with_default(template)
}

/// `{files}` は「1 件ずつ」なら 1 引数、「まとめて渡す」なら対象数ぶんの引数へ広がる。
/// 綴りが 1 つなので、複数選択を切り替えても引数テンプレートは書き直さなくてよい。
fn split_argument_template_for_selection(
    template: &str,
    _selection: SelectionPolicy,
) -> Vec<String> {
    split_argument_template_with_default(template)
}

fn contains_known_placeholder(template: &str) -> bool {
    IMPLEMENTED_PLACEHOLDERS
        .iter()
        .any(|placeholder| template.contains(placeholder))
}

/// 1 トークン内の `{files}` を置き換える。
///
/// 知らない記法は文字どおり残す。利用者が書いたものを黙って変えない。
fn expand_token(token: &str, files_value: &OsStr, file_list: Option<&Path>) -> OsString {
    let mut expanded = OsString::new();
    let mut remainder = token;

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
            expanded.push(files_value);
        } else if placeholder == "{file_list}"
            && let Some(path) = file_list
        {
            expanded.push(path.as_os_str());
        } else {
            expanded.push(placeholder);
        }
        remainder = &after_open[close..];
    }
    expanded.push(remainder);
    expanded
}
/// 分割済みトークン内だけで `{files}` を置換する。
///
/// `{file_list}` の値は worker が一覧を準備してから専用 expansion へ渡す。
pub fn expand_arguments(tokens: &[String], file: &Path) -> Vec<OsString> {
    expand_arguments_for_files(tokens, std::slice::from_ref(&file.to_path_buf()))
}

/// `{files}` を含む 1 トークンを対象数ぶんの引数へ展開する。
///
/// パスを空白連結した文字列にはせず、各パスを独立した `OsString` として返す。
pub fn expand_arguments_for_files(tokens: &[String], files: &[PathBuf]) -> Vec<OsString> {
    expand_arguments_with_file_list(tokens, files, None)
}

fn expand_arguments_with_file_list(
    tokens: &[String],
    files: &[PathBuf],
    file_list: Option<&Path>,
) -> Vec<OsString> {
    let mut result: Vec<OsString> = Vec::new();
    for token in tokens {
        if token.contains("{files}") {
            // 1 トークンが対象数ぶんの引数へ広がる。空白連結した 1 文字列にはしない。
            for file in files {
                result.push(expand_token(token, file.as_os_str(), file_list));
            }
        } else {
            result.push(expand_token(token, OsStr::new(""), file_list));
        }
    }
    result
}

/// 設定画面の引数プレビュー用。**毎フレーム呼ばれる**ので計画をログに出さない。
pub(crate) fn build_launch_request_for_preview(
    tool: &ExternalTool,
    target: &LaunchTarget,
) -> Result<ExternalLaunchRequest, String> {
    let mut operation = build_launch_operation_inner(tool, std::slice::from_ref(target), false)?;
    operation
        .requests
        .pop()
        .ok_or_else(|| "外部ツールの起動要求を組み立てられませんでした".to_string())
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
            LaunchTarget::ZipPage { .. } | LaunchTarget::PdfPage { .. } => {
                Some(crate::grid_item::FileOperationRefusal::VirtualPage)
            }
            LaunchTarget::Stack(_) => Some(crate::grid_item::FileOperationRefusal::Stack),
            LaunchTarget::Virtual(refusal) => Some(*refusal),
            _ => None,
        })
        .collect();
    if refusals.is_empty() {
        return None;
    }
    // これは設定画面の「テスト起動」など、実体化済みの実ファイルだけを受け取る
    // ready-launch builder の境界。通常の登録ツール起動は materializer 経路を通る。
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
            LaunchTarget::RealFile(path) | LaunchTarget::ImagePage(path) => Some(path.clone()),
            _ => None,
        })
        .collect())
}

fn validate_materializable_targets(targets: &[LaunchTarget]) -> Result<(), String> {
    if targets.is_empty()
        || targets
            .iter()
            .any(|target| matches!(target, LaunchTarget::None))
    {
        return Err("外部ツールへ渡す対象が選択されていません".to_string());
    }
    if let Some(refusal) = targets.iter().find_map(|target| match target {
        LaunchTarget::Virtual(refusal) => Some(*refusal),
        _ => None,
    }) {
        return Err(refusal.message("外部ツールで開くことが"));
    }
    if targets
        .iter()
        .any(|target| matches!(target, LaunchTarget::Unsupported))
    {
        return Err("この項目は外部ツールへ渡せません".to_string());
    }
    Ok(())
}

fn materialize_policy(payload: PayloadPolicy) -> crate::materializer::MaterializePolicy {
    match payload {
        PayloadPolicy::TempEdited => crate::materializer::MaterializePolicy::TempEdited,
        PayloadPolicy::TempOriginal => crate::materializer::MaterializePolicy::TempOriginal,
        PayloadPolicy::OriginalFile => crate::materializer::MaterializePolicy::OriginalFile,
    }
}

/// 対象件数に対する起動可否と確認要否を決める。
///
/// `Single` は件数設定を使わない。1 件専用という policy 自体が上限なので、2 件以上を
/// 黙って先頭 1 件へ縮めず拒否する。`Each` / `Batch` は上限を先に適用し、確認閾値は
/// その範囲内にだけ適用する。
fn evaluate_target_count(
    selection: SelectionPolicy,
    target_count: usize,
    confirmation_threshold: u32,
    max_targets: u32,
) -> Result<TargetCountDecision, String> {
    if selection == SelectionPolicy::Single {
        return if target_count >= 2 {
            Err(format!(
                "複数選択の設定が「1 件」のため、{target_count} 件は起動できません"
            ))
        } else {
            Ok(TargetCountDecision::Proceed)
        };
    }

    if target_count > max_targets as usize {
        return Err(format!(
            "対象は {target_count} 件ですが、このツールの上限は {max_targets} 件です。環境設定の「起動と連携」→「外部ツール」で上限を変更できます"
        ));
    }
    if target_count > confirmation_threshold as usize {
        Ok(TargetCountDecision::Confirm { target_count })
    } else {
        Ok(TargetCountDecision::Proceed)
    }
}

/// Rust の `Command::args` と同じ規則で、通常引数 1 個を実際に quote / escape する。
#[cfg(windows)]
fn windows_quote_regular_argument(argument: &OsStr) -> OsString {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let units: Vec<u16> = argument.encode_wide().collect();
    let quoted = units.is_empty()
        || units
            .iter()
            .any(|unit| *unit == b' ' as u16 || *unit == b'\t' as u16);
    let mut output = Vec::with_capacity(units.len().saturating_add(2));
    if quoted {
        output.push(b'"' as u16);
    }
    let mut preceding_backslashes = 0usize;
    for unit in units {
        if unit == b'\\' as u16 {
            preceding_backslashes = preceding_backslashes.saturating_add(1);
            continue;
        }
        let count = if unit == b'"' as u16 {
            preceding_backslashes.saturating_mul(2).saturating_add(1)
        } else {
            preceding_backslashes
        };
        output.extend(std::iter::repeat_n(b'\\' as u16, count));
        output.push(unit);
        preceding_backslashes = 0;
    }
    let trailing_count = if quoted {
        preceding_backslashes.saturating_mul(2)
    } else {
        preceding_backslashes
    };
    output.extend(std::iter::repeat_n(b'\\' as u16, trailing_count));
    if quoted {
        output.push(b'"' as u16);
    }
    OsString::from_wide(&output)
}

#[cfg(all(windows, test))]
fn windows_regular_argument_utf16_len(argument: &OsStr) -> usize {
    use std::os::windows::ffi::OsStrExt;

    windows_quote_regular_argument(argument)
        .encode_wide()
        .count()
}

/// `ShellExecuteExW::lpParameters` へ渡す、quoted argument list。
#[cfg(windows)]
fn windows_create_process_parameters(arguments: &[OsString]) -> OsString {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let mut output = Vec::new();
    for (index, argument) in arguments.iter().enumerate() {
        if index > 0 {
            output.push(b' ' as u16);
        }
        output.extend(windows_quote_regular_argument(argument).encode_wide());
    }
    OsString::from_wide(&output)
}

/// Rust の `Command::args` が `CreateProcessW` へ渡す command line。
///
/// argv[0] の常時引用、引数間の空白、通常引数の引用・escape、終端 NUL をすべて含む。
#[cfg(windows)]
fn windows_create_process_command_line(executable: &OsStr, arguments: &[OsString]) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    let mut command_line = Vec::new();
    command_line.push(b'"' as u16);
    command_line.extend(executable.encode_wide());
    command_line.push(b'"' as u16);
    if !arguments.is_empty() {
        command_line.push(b' ' as u16);
        command_line.extend(windows_create_process_parameters(arguments).encode_wide());
    }
    command_line.push(0);
    command_line
}

/// Rust の `Command::args` が `CreateProcessW` へ渡す command line の UTF-16 長。
///
/// 実際の command line と長さ計算で quoting の実装が分かれないよう、上の結果から求める。
#[cfg(windows)]
fn windows_create_process_command_line_utf16_len(
    executable: &OsStr,
    arguments: &[OsString],
) -> usize {
    windows_create_process_command_line(executable, arguments).len()
}

fn build_request_for_files_with_list(
    tool: &ExternalTool,
    files: Vec<PathBuf>,
    file_list: Option<&Path>,
) -> Result<ExternalLaunchRequest, String> {
    let arguments = if tool.launch.uses_process_options() {
        let tokens = split_argument_template_for_selection(&tool.arguments, tool.selection);
        if tool.selection == SelectionPolicy::Batch {
            if !tokens.iter().any(|token| contains_known_placeholder(token)) {
                return Err(
                    "まとめて渡すには引数テンプレートに {files} または {file_list} を指定してください".to_string(),
                );
            }
        }
        if tokens.iter().any(|token| token.contains("{file_list}")) && file_list.is_none() {
            return Err("ファイル一覧は外部ツール起動 worker で準備します".to_string());
        }
        expand_arguments_with_file_list(&tokens, &files, file_list)
    } else {
        Vec::new()
    };
    #[cfg(windows)]
    if tool.selection == SelectionPolicy::Batch
        && let ExternalToolLaunch::Executable(executable) = &tool.launch
        && windows_create_process_command_line_utf16_len(executable.as_os_str(), &arguments)
            > CREATE_PROCESS_COMMAND_LINE_MAX_UTF16_UNITS
    {
        return Err(format!(
            "対象ファイルが多すぎるため起動できません。コマンドラインが Windows の上限（{CREATE_PROCESS_COMMAND_LINE_MAX_UTF16_UNITS} 文字）を超えます"
        ));
    }
    Ok(ExternalLaunchRequest {
        tool_name: tool.display_name(),
        launch: tool.launch.clone(),
        arguments,
        working_directory: tool
            .launch
            .uses_process_options()
            .then(|| tool.working_directory.clone())
            .flatten(),
        show_console: tool.launch.uses_process_options() && tool.show_console,
        files,
    })
}

pub(crate) fn build_launch_operation(
    tool: &ExternalTool,
    targets: &[LaunchTarget],
) -> Result<ExternalLaunchOperation, String> {
    build_launch_operation_inner(tool, targets, true)
}

fn build_launch_operation_inner(
    tool: &ExternalTool,
    targets: &[LaunchTarget],
    log_plan: bool,
) -> Result<ExternalLaunchOperation, String> {
    let files = real_files_from_targets(targets)?;
    let target_count = files.len();
    let target_count_decision = evaluate_target_count(
        tool.selection,
        target_count,
        tool.confirmation_threshold,
        tool.max_targets,
    )?;
    let preview_list =
        (!log_plan).then(|| Path::new(r"C:\Temp\mimageviewer\ext-1234\file-list-1.txt"));
    let build = |files| build_request_for_files_with_list(tool, files, preview_list);
    let requests = match tool.selection {
        SelectionPolicy::Single => vec![build(files)?],
        SelectionPolicy::Each => files
            .into_iter()
            .map(|file| build(vec![file]))
            .collect::<Result<Vec<_>, _>>()?,
        SelectionPolicy::Batch => match &tool.launch {
            ExternalToolLaunch::Executable(_) | ExternalToolLaunch::Association { .. } => {
                vec![build(files)?]
            }
            // OS 既定アプリへ複数パスをまとめて渡す API はないため Each と同じ。
            ExternalToolLaunch::OsDefault => files
                .into_iter()
                .map(|file| build(vec![file]))
                .collect::<Result<Vec<_>, _>>()?,
        },
    };
    // 起動計画をログに残す。Single / Each / Batch の違いは、起動したプロセス数と
    // 1 プロセスへ渡したファイル数にしか出ないので、外から確かめる手段がこれしかない
    // (2026-09-01 利用者要望)。
    if log_plan {
        crate::logger::log(format!(
            "external_tool: plan tool={:?} selection={:?} launch={} targets={} processes={} files_per_process={:?}",
            tool.display_name(),
            tool.selection,
            match &tool.launch {
                ExternalToolLaunch::Executable(_) => "Executable",
                ExternalToolLaunch::Association { .. } => "Association",
                ExternalToolLaunch::OsDefault => "OsDefault",
            },
            target_count,
            requests.len(),
            requests
                .iter()
                .map(|request| request.files.len())
                .collect::<Vec<_>>(),
        ));
    }
    Ok(ExternalLaunchOperation {
        tool_name: tool.display_name(),
        requests,
        target_count,
        target_count_decision,
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
        show_console: false,
        files: vec![file],
    }
}

fn launch_confirmation(
    operation: ExternalQueuedOperation,
) -> Result<ExternalQueuedOperation, ExternalLaunchConfirmation> {
    let network_executable = operation.launch().executable().filter(|path| {
        path.as_os_str()
            .as_encoded_bytes()
            .first()
            .is_some_and(|byte| *byte == b'\\')
    });
    let network_executable = network_executable.map(Path::to_path_buf);
    if network_executable.is_none()
        && operation.target_count_decision() == TargetCountDecision::Proceed
    {
        Ok(operation)
    } else {
        Err(ExternalLaunchConfirmation {
            operation,
            network_executable,
        })
    }
}

pub(crate) fn start_launch_worker(
    operation: ExternalLaunchOperation,
    owner_hwnd: Option<isize>,
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
                let result = launch_request(request, owner_hwnd);
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

/// `local_ai_activity` は worker が終わるまで持ち続ける借用。
///
/// 実体化は消しゴム (MI-GAN) や AI 拡大を回し得るので、リモートへ操作権を渡す前の静止確認
/// (`local_ai_remote_barrier_snapshot`) はこの worker を数えなければならない。数えていな
/// かったので、消しゴム付きの外部ツール起動中に接続するとローカル AI が走ったまま操作権が
/// 移り、リモート側の AI と GPU / モデルを取り合っていた (v3.5.0 レビュー F09)。
///
/// **どの対象が AI を回すかを起動前に決めない。** 何を回すかは worker がページ編集を DB から
/// 読みながら決めるので、先読みすると「AI を使うか」の綴りが 2 つになる。借用は worker の
/// 寿命そのもので、cancel でも panic でも drop で返る。
fn start_materialize_launch_worker(
    operation: ExternalMaterializeOperation,
    mut session: crate::materializer::MaterializeSession,
    generation: u64,
    owner_hwnd: Option<isize>,
    local_ai_activity: crate::app::LocalAiActivityLease,
) -> Result<ExternalMaterializePending, String> {
    let target_count = operation.target_count();
    let progress = Arc::new(MaterializeProgress::new(target_count));
    let progress_worker = Arc::clone(&progress);
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    let (launch_boundary_tx, launch_boundary_rx) = mpsc::channel();
    let (launch_decision_tx, launch_decision_rx) = mpsc::channel();
    let worker = std::thread::Builder::new()
        .name("external-tool-materialize".to_string())
        .spawn(move || {
            let _local_ai_activity = local_ai_activity;
            let completion = run_materialize_launch_operation(
                operation,
                &mut session,
                generation,
                owner_hwnd,
                &cancel_worker,
                &progress_worker,
                &launch_boundary_tx,
                &launch_decision_rx,
            );
            let _ = tx.send(completion);
        })
        .map_err(|error| format!("外部ツール準備 worker を開始できません: {error}"))?;
    Ok(ExternalMaterializePending {
        cancel,
        rx,
        launch_boundary_rx,
        launch_decision_tx: Some(launch_decision_tx),
        progress,
        generation,
        worker: Some(worker),
        user_cancelled: false,
        launch_ui_checkpoint_passed: false,
    })
}

struct PreparedInvocation {
    request: ExternalLaunchRequest,
    list: Option<crate::materializer::PreparedFileList>,
    media_indices: Vec<usize>,
}

/// All list I/O and argument validation precede the UI launch boundary. Until its ACK,
/// invocation and media leases are request-owned and the modal can still cancel them.
fn prepare_materialized_invocation(
    tool: &ExternalTool,
    files: Vec<PathBuf>,
    media_indices: Vec<usize>,
    session: &mut crate::materializer::MaterializeSession,
    generation: u64,
    cancel: &Arc<AtomicBool>,
) -> Result<PreparedInvocation, String> {
    let needs_list = tool.launch.uses_process_options()
        && split_argument_template(&tool.arguments)
            .iter()
            .any(|token| token.contains("{file_list}"));
    let list = if needs_list {
        Some(session.create_file_list(&files, cancel, generation)?)
    } else {
        None
    };
    let request =
        build_request_for_files_with_list(tool, files, list.as_ref().map(|list| list.path()))?;
    session.ensure_current(cancel.as_ref(), generation)?;
    Ok(PreparedInvocation {
        request,
        list,
        media_indices,
    })
}

fn run_materialize_launch_operation(
    operation: ExternalMaterializeOperation,
    session: &mut crate::materializer::MaterializeSession,
    generation: u64,
    owner_hwnd: Option<isize>,
    cancel: &Arc<AtomicBool>,
    progress: &MaterializeProgress,
    launch_boundary_tx: &mpsc::Sender<()>,
    launch_decision_rx: &mpsc::Receiver<MaterializeLaunchDecision>,
) -> ExternalLaunchCompletion {
    run_materialize_launch_operation_with_launcher(
        operation,
        session,
        generation,
        owner_hwnd,
        cancel,
        progress,
        launch_boundary_tx,
        launch_decision_rx,
        launch_request,
    )
}

fn run_materialize_launch_operation_with_launcher(
    operation: ExternalMaterializeOperation,
    session: &mut crate::materializer::MaterializeSession,
    generation: u64,
    owner_hwnd: Option<isize>,
    cancel: &Arc<AtomicBool>,
    progress: &MaterializeProgress,
    launch_boundary_tx: &mpsc::Sender<()>,
    launch_decision_rx: &mpsc::Receiver<MaterializeLaunchDecision>,
    mut launcher: impl FnMut(
        ExternalLaunchRequest,
        Option<isize>,
    ) -> Result<ExternalLaunchOutcome, String>,
) -> ExternalLaunchCompletion {
    let ExternalMaterializeOperation { tool, targets, .. } = operation;
    let tool_name = tool.display_name();
    let target_count = targets.len();
    let mut prepared = Vec::with_capacity(target_count);
    let mut failures = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        progress.update(
            index,
            format!("{} / {} 件目を準備しています", index + 1, target_count),
        );
        let label = target.source.display_label();
        match session.materialize(target, cancel, generation) {
            Ok(file) => prepared.push(Some(file)),
            Err(error) => {
                failures.push(format!("{label}: {error}"));
                prepared.push(None);
                if cancel.load(Ordering::Acquire) {
                    break;
                }
            }
        }
        progress.update(index + 1, "外部ツールの起動を準備しています");
    }
    let mut invocations = Vec::new();
    if session.ensure_current(cancel.as_ref(), generation).is_ok() {
        let ready_indices: Vec<_> = prepared
            .iter()
            .enumerate()
            .filter_map(|(index, file)| file.as_ref().map(|_| index))
            .collect();
        let batch = tool.selection == SelectionPolicy::Batch
            && !matches!(tool.launch, ExternalToolLaunch::OsDefault);
        let groups = if batch {
            if ready_indices.is_empty() {
                Vec::new()
            } else {
                vec![ready_indices]
            }
        } else {
            ready_indices.into_iter().map(|index| vec![index]).collect()
        };
        for (invocation_index, indices) in groups.into_iter().enumerate() {
            progress.update(
                target_count,
                format!(
                    "{} 件目の起動に渡すファイル一覧を準備しています",
                    invocation_index + 1
                ),
            );
            let files: Vec<_> = indices
                .iter()
                .map(|index| {
                    prepared[*index]
                        .as_ref()
                        .expect("ready index")
                        .path()
                        .to_path_buf()
                })
                .collect();
            let label = if files.len() == 1 {
                files[0].display().to_string()
            } else {
                format!("{} 件", files.len())
            };
            match prepare_materialized_invocation(
                &tool, files, indices, session, generation, cancel,
            ) {
                Ok(invocation) => invocations.push(invocation),
                Err(error) => failures.push(format!("{label}: {error}")),
            }
            if session.ensure_current(cancel.as_ref(), generation).is_err() {
                break;
            }
        }
    }
    let mut succeeded_target_count = 0;
    let mut refreshes = Vec::new();
    if session.ensure_current(cancel.as_ref(), generation).is_err() {
        if failures.is_empty() {
            failures.push("外部ツールの準備をキャンセルしました".to_string());
        }
    } else if !invocations.is_empty() {
        progress.update(target_count, "外部ツールの起動を確認しています");
        let launch_authorized = if launch_boundary_tx.send(()).is_err() {
            false
        } else {
            loop {
                if session.ensure_current(cancel.as_ref(), generation).is_err() {
                    break false;
                }
                match launch_decision_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                    Ok(MaterializeLaunchDecision::Launch) => break true,
                    Ok(MaterializeLaunchDecision::Cancel) => break false,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break false,
                }
            }
        };
        if !launch_authorized {
            failures.push("外部ツールの準備をキャンセルしました".to_string());
        } else {
            progress.update(target_count, "外部ツールを起動しています");
            for invocation in invocations {
                if session.ensure_current(cancel.as_ref(), generation).is_err() {
                    failures.push("古い起動要求を破棄しました".to_string());
                    break;
                }
                let PreparedInvocation {
                    request,
                    mut list,
                    media_indices,
                } = invocation;
                let label = if request.files.len() == 1 {
                    request.files[0].display().to_string()
                } else {
                    format!("{} 件", request.files.len())
                };
                match launcher(request, owner_hwnd) {
                    Ok(outcome) => {
                        let failed: HashMap<&Path, &str> = outcome
                            .failed_files
                            .iter()
                            .map(|(path, reason)| (path.as_path(), reason.as_str()))
                            .collect();
                        let mut launched = 0;
                        for index in media_indices {
                            let file = prepared[index].as_mut().expect("ready index");
                            if let Some(reason) = failed.get(file.path()) {
                                failures.push(format!("{}: {reason}", file.path().display()));
                            } else {
                                file.transfer_to_process_directory(tool.keep_temp);
                                launched += 1;
                            }
                        }
                        if launched > 0
                            && let Some(list) = list.as_mut()
                        {
                            list.transfer_to_process_directory(tool.keep_temp);
                        }
                        succeeded_target_count += launched;
                        if let Some(refresh) = outcome.refresh {
                            refreshes.push(refresh);
                        }
                    }
                    Err(error) => failures.push(format!("{label}: {error}")),
                }
            }
        }
    }
    progress.update(target_count, "完了しました");
    ExternalLaunchCompletion {
        tool_name,
        target_count,
        succeeded_target_count,
        failures,
        refreshes,
    }
}

/// 1 回の起動要求の結果。
///
/// `failed_files` は**渡したのに起動できなかった対象**。関連付けアプリへ複数ファイルを
/// 渡す経路は 1 件ずつ叩くので、一部だけ失敗し得る。ここを潰して「全件成功」にすると、
/// 失敗した対象が利用者に見えず、その一時ファイルも起動済みとして手放してしまう
/// (v3.5.0 レビュー F02)。
#[derive(Debug)]
struct ExternalLaunchOutcome {
    refresh: Option<AssociationHandlerRefresh>,
    failed_files: Vec<(PathBuf, String)>,
}

impl ExternalLaunchOutcome {
    fn all_launched(refresh: Option<AssociationHandlerRefresh>) -> Self {
        Self {
            refresh,
            failed_files: Vec::new(),
        }
    }
}

/// `CreateProcessW` が対象を実行形式として読み込めず、何も起動していない失敗か。
#[cfg(any(windows, test))]
fn is_bad_exe_format(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(193)
}

fn launch_request(
    request: ExternalLaunchRequest,
    owner_hwnd: Option<isize>,
) -> Result<ExternalLaunchOutcome, String> {
    let ExternalLaunchRequest {
        tool_name,
        launch,
        arguments,
        working_directory,
        show_console,
        files,
    } = request;
    match launch {
        ExternalToolLaunch::Executable(executable) => {
            crate::logger::log(format!(
                "external_tool: spawn {tool_name:?} exe={executable:?} files={} args={:?}",
                files.len(),
                arguments
            ));
            let mut command = Command::new(&executable);
            command.args(&arguments).stdin(Stdio::null());
            // 既定は標準出力を捨てる (mIV の stdio を子へ漏らさない)。
            //
            // **「コンソール窓を表示する」を ON にしたときだけ継承する。** `CREATE_NO_WINDOW`
            // を外すだけでは足りない: Rust の `Command` は `STARTF_USESTDHANDLES` を常に
            // 立てるので、NUL へ向けた handle が新しいコンソールの handle を上書きし、
            // **窓は出るのに中身が空**になる (2026-09-04 実測: 窓は visible、`GetConsoleMode`
            // は失敗)。継承にすると、コンソールを持たない mIV から起動した子は割り当てられた
            // 新しいコンソールへ、コンソール付きで起動された mIV の子はその同じコンソールへ
            // 出力する。どちらでも利用者は出力を読める。
            //
            // 継承するのは Windows でコンソールを見せるときだけ。非 Windows は従来どおり
            // 常に捨てる。
            if !(cfg!(windows) && show_console) {
                command.stdout(Stdio::null()).stderr(Stdio::null());
            }
            if let Some(directory) = &working_directory {
                command.current_dir(directory);
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                if !show_console {
                    command.creation_flags(CREATE_NO_WINDOW);
                }
            }
            let result = command.spawn();
            #[cfg(windows)]
            if let Err(error) = &result
                && is_bad_exe_format(error)
            {
                let parameters = windows_create_process_parameters(&arguments);
                let fallback = crate::open_with::shell_execute_open(
                    &executable,
                    (!arguments.is_empty()).then_some(parameters.as_os_str()),
                    working_directory.as_deref(),
                    show_console,
                    owner_hwnd,
                );
                crate::logger::log(format!(
                    "external_tool: ShellExecuteEx open fallback {tool_name:?} file={executable:?} result={:?}",
                    fallback.as_ref().map(|_| ())
                ));
                return fallback.map(|_| ExternalLaunchOutcome::all_launched(None));
            }
            #[cfg(not(windows))]
            let _ = show_console;
            result
                .map(|_| ExternalLaunchOutcome::all_launched(None))
                .map_err(|error| error.to_string())
        }
        ExternalToolLaunch::Association { handler_id } => {
            crate::logger::log(format!(
                "external_tool: launching {tool_name:?} via Association handler_id={handler_id:?} files={}",
                files.len()
            ));
            let outcome = if files.len() == 1 {
                crate::open_with::invoke_association_handler(
                    &handler_id,
                    &tool_name,
                    &files[0],
                    owner_hwnd,
                )
            } else {
                crate::open_with::invoke_association_handler_for_paths(
                    &handler_id,
                    &tool_name,
                    &files,
                    owner_hwnd,
                )
            };
            outcome.map(|outcome| ExternalLaunchOutcome {
                refresh: outcome
                    .refreshed_handler_id
                    .map(|current_id| AssociationHandlerRefresh {
                        previous_id: handler_id,
                        current_id,
                    }),
                failed_files: outcome.failed_paths,
            })
        }
        ExternalToolLaunch::OsDefault => {
            let Some(file) = files.into_iter().next() else {
                return Err("OS の関連付けへ渡す実ファイルがありません".to_string());
            };
            opener::open(file)
                .map(|_| ExternalLaunchOutcome::all_launched(None))
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

#[cfg(test)]
fn expand_stack_targets(
    targets: &[LaunchTarget],
    stack_view: Option<&crate::filename_stack::StackView>,
) -> Result<Vec<LaunchTarget>, String> {
    let mut expanded = Vec::new();
    for target in targets {
        let LaunchTarget::Stack(key) = target else {
            expanded.push(target.clone());
            continue;
        };
        let view = stack_view.ok_or_else(|| "スタックの内容を解決できませんでした".to_string())?;
        let group_index = view.group_index_by_key(key).ok_or_else(|| {
            "スタックの内容が更新されたため、もう一度実行してください".to_string()
        })?;
        let members = &view.groups[group_index].members;
        if members.is_empty() {
            return Err("スタックに外部ツールへ渡せるページがありません".to_string());
        }
        expanded.extend(members.iter().map(|member| {
            if member.is_video {
                LaunchTarget::RealFile(member.path.clone())
            } else {
                LaunchTarget::ImagePage(member.path.clone())
            }
        }));
    }
    Ok(expanded)
}

impl crate::app::App {
    #[cfg(test)]
    pub(crate) fn stack_member_default_params_for_test(
        &self,
        path: &Path,
    ) -> crate::adjustment::AdjustParams {
        self.stack_member_default_params(path)
    }

    fn stack_member_default_params(&self, path: &Path) -> crate::adjustment::AdjustParams {
        path.parent()
            .and_then(|folder| {
                crate::final_composite::active_favorite_default_id_for_path(
                    folder,
                    &self.settings.favorites,
                    None,
                    |id| self.adjustment_favorite_params.contains_key(&id),
                )
            })
            .and_then(|id| self.adjustment_favorite_params.get(&id))
            .cloned()
            .unwrap_or_else(|| self.settings.global_preset.clone())
    }

    /// 対象 1 件を「何を作るか」へ落とす。
    fn materialize_target(
        &self,
        tool: &ExternalTool,
        target: &LaunchTarget,
        index: Option<usize>,
        snapshots: &ExternalRequestSnapshots,
    ) -> Result<crate::materializer::MaterializeRequest, String> {
        use crate::materializer::{
            MaterializePageEdits, MaterializeRequest, MaterializeSource, PageEditContext,
        };

        let (source, page_key, fallback_path) = match target {
            // 再生中の動画に「表示中のフレーム」が設定されているときだけ、動画ファイル
            // ではなくフレームを渡す。再生していなければ従来どおり動画ファイル
            // (§4.3 の `VideoPolicy::CurrentFrame` は「再生中でなければ `File` に落ちる」)。
            LaunchTarget::RealFile(path)
                if tool.video == VideoPolicy::CurrentFrame
                    && snapshots
                        .video_frame
                        .as_ref()
                        .is_some_and(|(source, _)| crate::folder_tree::path_eq(source, path)) =>
            {
                (
                    MaterializeSource::VideoFrame {
                        path: path.clone(),
                        target_millis: snapshots
                            .video_frame
                            .as_ref()
                            .expect("checked in the guard")
                            .1,
                    },
                    None,
                    None,
                )
            }
            LaunchTarget::RealFile(path) => (
                MaterializeSource::File {
                    path: path.clone(),
                    image_page: false,
                },
                None,
                None,
            ),
            LaunchTarget::ImagePage(path) => (
                MaterializeSource::File {
                    path: path.clone(),
                    image_page: true,
                },
                Some(crate::adjustment_db::normalize_path(path)),
                Some(path.as_path()),
            ),
            LaunchTarget::ZipPage {
                zip_path,
                entry_name,
            } => (
                MaterializeSource::ZipEntry {
                    zip_path: zip_path.clone(),
                    entry_name: entry_name.clone(),
                },
                Some(crate::adjustment_db::zip_entry_key(zip_path, entry_name)),
                None,
            ),
            LaunchTarget::PdfPage { pdf_path, page_num } => (
                MaterializeSource::PdfPage {
                    pdf_path: pdf_path.clone(),
                    page_num: *page_num,
                    password: self.pdf_open_password(pdf_path),
                },
                Some(crate::adjustment_db::zip_entry_key(
                    pdf_path,
                    &format!("page_{page_num}"),
                )),
                None,
            ),
            LaunchTarget::Stack(_) => {
                return Err("スタックを展開できませんでした".to_string());
            }
            LaunchTarget::Virtual(refusal) => {
                return Err(refusal.message("外部ツールで開くことが"));
            }
            LaunchTarget::Unsupported => {
                return Err("この項目は外部ツールへ渡せません".to_string());
            }
            LaunchTarget::None => {
                return Err("外部ツールへ渡す対象が選択されていません".to_string());
            }
        };
        let page_edits = page_key.map(|page_key| {
            let (params, load_page_params_from_db) = if let Some(index) = index {
                (self.effective_params(index).clone(), false)
            } else if let Some(path) = fallback_path {
                (self.stack_member_default_params(path), true)
            } else {
                (snapshots.global_params.clone(), true)
            };
            PageEditContext {
                page_key,
                stage: snapshots.stage,
                // 実体の選択は worker が **確定した params** から行う (F10)。
                creative_luts: snapshots.creative_luts.clone(),
                ai_materials: snapshots.ai_materials.clone(),
                params,
                conceal_preset: snapshots.conceal_preset.clone(),
                erase_mono_tolerance: snapshots.erase_mono_tolerance,
                comic_source_dims: index.and_then(|index| {
                    self.source_dims_for_idx(index)
                        .map(|(width, height)| [width.round() as usize, height.round() as usize])
                }),
                ai_runtime_init: Arc::clone(&self.ai_runtime_init),
                ai_model_manager: Arc::clone(&self.ai_model_manager),
                load_page_params_from_db,
            }
        });
        Ok(MaterializeRequest {
            raw_brightness: snapshots.raw_brightness,
            source,
            policy: materialize_policy(tool.payload),
            page_edits: page_edits.map(MaterializePageEdits::Single),
            pdf_render_long_edge: if tool.pdf_render_long_edge == 0 {
                DEFAULT_PDF_RENDER_LONG_EDGE
            } else {
                tool.pdf_render_long_edge
            },
        })
    }

    /// フルスクリーンで見開きが見えているとき、ツールの `SpreadPolicy` に従って対象を
    /// 組み替える。**それ以外では `None` を返して従来どおりに扱う。**
    ///
    /// ここでしかできないのは、見えている組を `resolve_visible_spread_pair` が
    /// `&mut self` で解決するため。`Merged` の画素合成自体は materializer worker が行う。
    fn merged_spread_target(
        &self,
        tool: &ExternalTool,
        left: usize,
        right: usize,
        snapshots: &ExternalRequestSnapshots,
    ) -> Result<crate::materializer::MaterializeRequest, String> {
        let left_basename = self
            .capture_basename_for_idx(left)
            .unwrap_or_else(|| format!("page-{}", left + 1));
        let right_basename = self
            .capture_basename_for_idx(right)
            .unwrap_or_else(|| format!("page-{}", right + 1));
        let label =
            crate::capture::basename_from_text(&format!("{left_basename}_{right_basename}"));
        let left_target = LaunchTarget::from_grid_item(self.items.get(left));
        let right_target = LaunchTarget::from_grid_item(self.items.get(right));
        let left = self.materialize_target(tool, &left_target, Some(left), snapshots)?;
        let right = self.materialize_target(tool, &right_target, Some(right), snapshots)?;
        let left_edits = match left.page_edits {
            Some(crate::materializer::MaterializePageEdits::Single(edits)) => edits,
            _ => return Err("見開き左ページの編集情報を準備できません".to_string()),
        };
        let right_edits = match right.page_edits {
            Some(crate::materializer::MaterializePageEdits::Single(edits)) => edits,
            _ => return Err("見開き右ページの編集情報を準備できません".to_string()),
        };
        Ok(crate::materializer::MaterializeRequest {
            raw_brightness: snapshots.raw_brightness,
            source: crate::materializer::MaterializeSource::MergedSpread {
                label,
                left: Box::new(left.source),
                right: Box::new(right.source),
            },
            policy: materialize_policy(tool.payload),
            page_edits: Some(crate::materializer::MaterializePageEdits::Spread {
                left: left_edits,
                right: right_edits,
            }),
            pdf_render_long_edge: if tool.pdf_render_long_edge == 0 {
                DEFAULT_PDF_RENDER_LONG_EDGE
            } else {
                tool.pdf_render_long_edge
            },
        })
    }

    /// この対象がいま再生中の動画そのもので、フレームを切り出せるなら、その時刻 (ミリ秒)。
    ///
    /// **`{time}` とは別の値を使う。** こちらは「見えている絵」を切り出す用で、
    /// クリップボードコピー / <kbd>Ctrl+S</kbd> と同じ `screenshot_target_secs()`
    /// (最後に提示したフレームの PTS) を採る。一時停止中でも画面と一致させるため。
    fn external_tool_video_frame_millis(&self, path: &Path) -> Option<u64> {
        let fs_idx = self.fullscreen_idx?;
        let player = self.fs_video_player(fs_idx)?;
        if !crate::folder_tree::path_eq(player.path(), path) {
            return None;
        }
        // クリップボードコピーと同じ「まだ開けていない / 壊れている」判定。
        // **映像トラックの有無まで見る。** `RealFile` は音声ファイルも通るので、
        // ここを緩めると音声を切り出そうとして "video stream not found" で失敗する。
        // 音声はフレームを持たないので、従来どおりファイルそのものを渡すのが正しい
        // (Codex Sol 指摘 #12)。
        if player.error().is_some() {
            return None;
        }
        if !player.info().is_some_and(|info| info.has_video) {
            return None;
        }
        let secs = player.screenshot_target_secs();
        secs.is_finite()
            .then(|| (secs.max(0.0) * 1000.0).round() as u64)
    }

    pub(crate) fn external_tool_container_targets(&self) -> Vec<LaunchTarget> {
        let effective_folder = self.effective_folder();
        let unavailable = self.items_are_drive_list
            || self.items_are_global_search_view
            || self.items_are_tag_view
            || self.items_are_reading_history_view
            || self.items_are_bookmark_view
            || self.items_are_rating_view
            || self.is_snapshot_active()
            || self.favsearch.on_results_grid()
            || self.tag_view.on_results_grid()
            || effective_folder
                .as_deref()
                .is_some_and(crate::app::is_synthetic_view_path);
        resolve_external_container_targets(effective_folder.as_deref(), unavailable)
    }

    pub(crate) fn external_tool_context_menu_descriptor(
        &self,
        source: ExternalTargetSource,
    ) -> ExternalTargetDescriptor {
        let mut capability = ExternalToolMenuTarget::default();
        let primary = match &source {
            ExternalTargetSource::GridContext { clicked } => *clicked,
            ExternalTargetSource::GridKey { selected } => *selected,
            ExternalTargetSource::Viewer { current }
            | ExternalTargetSource::Playback { current } => *current,
            ExternalTargetSource::Container { path } => {
                capability.has_target = path.is_some();
                None
            }
        };
        if !matches!(source, ExternalTargetSource::Container { .. }) {
            if matches!(
                source,
                ExternalTargetSource::GridContext { .. } | ExternalTargetSource::GridKey { .. }
            ) && !self.checked.is_empty()
            {
                for index in &self.checked {
                    if let Some(item) = self.items.get(*index) {
                        capability.add_grid_item(item);
                    }
                }
            } else if let Some(item) = primary.and_then(|index| self.items.get(index)) {
                capability.add_grid_item(item);
            }
        }
        ExternalTargetDescriptor {
            owner: self.edit_request_owner_context(),
            items_generation: self.items_generation,
            source,
            capability,
        }
    }

    pub(crate) fn external_tool_target_descriptor(
        &self,
        source: ExternalTargetSource,
    ) -> ExternalTargetDescriptor {
        let capability = match &source {
            ExternalTargetSource::Container { path } => ExternalToolMenuTarget {
                has_target: path.is_some(),
                ..Default::default()
            },
            _ => ExternalToolMenuTarget::default(),
        };
        ExternalTargetDescriptor {
            owner: self.edit_request_owner_context(),
            items_generation: self.items_generation,
            source,
            capability,
        }
    }

    pub(crate) fn external_tool_container_descriptor(&self) -> ExternalTargetDescriptor {
        let path = self
            .external_tool_container_targets()
            .into_iter()
            .next()
            .and_then(|target| match target {
                LaunchTarget::RealFile(path) => Some(path),
                _ => None,
            });
        self.external_tool_target_descriptor(ExternalTargetSource::Container { path })
    }

    fn external_request_snapshots(&mut self) -> ExternalRequestSnapshots {
        let video_frame = self
            .fullscreen_idx
            .and_then(|index| self.items.get(index))
            .and_then(|item| match item {
                crate::grid_item::GridItem::Video(path) => Some(path.clone()),
                _ => None,
            })
            .and_then(|path| {
                self.external_tool_video_frame_millis(&path)
                    .map(|millis| (path, millis))
            });
        let spread = self.fullscreen_idx.and_then(|index| {
            let crate::ui_fullscreen::SpreadPair::Double { left, right } =
                self.resolve_visible_spread_pair(index)
            else {
                return None;
            };
            self.resolve_visible_spread_presentation_in_reading_order(index)
                .and_then(|pages| spread_both_pages_order(&pages))
                .map(|order| (index, merged_spread_screen_order(left, right), order))
        });
        ExternalRequestSnapshots {
            stage: self.settings.bake_stage_external_tool,
            raw_brightness: self.settings.raw_brightness,
            global_params: self.settings.global_preset.clone(),
            conceal_preset: self.current_conceal_preset_from_settings(),
            erase_mono_tolerance: self.settings.erase_inpaint_mono_tolerance,
            creative_luts: self.creative_lut_library.snapshot(),
            ai_materials: self
                .settings
                .bake_stage_external_tool
                .includes_ai()
                .then(|| self.book_ai_materials()),
            video_frame,
            spread,
        }
    }

    fn retire_external_operation(&mut self) {
        match std::mem::take(&mut self.external_tool_operation) {
            ExternalOperation::Materializing(mut pending)
            | ExternalOperation::Launching(ExternalLaunching::Materialized(mut pending)) => {
                pending.cancel(false);
                self.external_tool_retired_materialize.push(pending);
                self.external_tool_materializer.cancel_all();
            }
            ExternalOperation::Launching(ExternalLaunching::Ready(pending)) => {
                self.external_tool_retired_launch.push(pending)
            }
            _ => {}
        }
    }

    fn begin_external_preparation(
        &mut self,
        viewport: egui::ViewportId,
        descriptor: ExternalTargetDescriptor,
        purpose: ExternalPreparationPurpose,
    ) {
        self.retire_external_operation();
        self.external_tool_modal_viewport = viewport;
        let snapshots = self.external_request_snapshots();
        self.external_tool_operation = ExternalOperation::Preparing(ExternalPreparing {
            owner: descriptor.owner,
            items_generation: descriptor.items_generation,
            purpose,
            input: ExternalPreparationInput::Source(descriptor.source),
            stage: ExternalPreparationStage::Enumerating { cursor: 0 },
            snapshots,
            locators: Vec::new(),
            requests: Vec::new(),
            page_indices: HashMap::new(),
            stack_indices: HashMap::new(),
            capability: ExternalToolMenuTarget::default(),
            validation: ExternalTargetValidation::Valid,
            has_stack: false,
            last_frame: None,
            scanned_entries: 0,
        });
    }

    pub(crate) fn request_grid_external_tool_picker(&mut self) {
        let descriptor = self.external_tool_target_descriptor(ExternalTargetSource::GridKey {
            selected: self.selected,
        });
        self.begin_external_preparation(
            egui::ViewportId::ROOT,
            descriptor,
            ExternalPreparationPurpose::ForPicker(ExternalToolPickerTargetKind::GridItems),
        );
    }

    pub(crate) fn request_container_external_tool_picker(&mut self) {
        let descriptor = self.external_tool_container_descriptor();
        self.begin_external_preparation(
            egui::ViewportId::ROOT,
            descriptor,
            ExternalPreparationPurpose::ForPicker(ExternalToolPickerTargetKind::Container),
        );
    }

    pub(crate) fn launch_grid_external_tool_slot(&mut self, ctx: &egui::Context, slot: usize) {
        let tool = match resolve_external_tool_slot(&self.settings.external_tools, slot) {
            Ok(tool) => tool.clone(),
            Err(error) => {
                self.show_feedback_toast(error);
                return;
            }
        };
        let descriptor = self.external_tool_target_descriptor(ExternalTargetSource::GridKey {
            selected: self.selected,
        });
        self.begin_external_preparation(
            ctx.viewport_id(),
            descriptor,
            ExternalPreparationPurpose::ForTool(tool),
        );
    }

    pub(crate) fn queue_external_tool_launch_from_context_menu(
        &mut self,
        ctx: &egui::Context,
        tool: &ExternalTool,
        descriptor: &ExternalTargetDescriptor,
    ) {
        self.begin_external_preparation(
            ctx.viewport_id(),
            descriptor.clone(),
            ExternalPreparationPurpose::ForTool(tool.clone()),
        );
    }

    /// Preferences' internal single-target test launch has no source index. Build one
    /// frame-split page lookup for that legacy request; never scan for a RealFile.
    pub(crate) fn queue_external_tool_launch(
        &mut self,
        ctx: &egui::Context,
        tool: &ExternalTool,
        target: &LaunchTarget,
    ) {
        self.begin_external_legacy_preparation(
            ctx.viewport_id(),
            vec![target.clone()],
            ExternalPreparationPurpose::ForTool(tool.clone()),
        );
    }

    fn begin_external_legacy_preparation(
        &mut self,
        viewport: egui::ViewportId,
        targets: Vec<LaunchTarget>,
        purpose: ExternalPreparationPurpose,
    ) {
        self.retire_external_operation();
        self.external_tool_modal_viewport = viewport;
        let needs_lookup = targets
            .iter()
            .any(|target| external_page_identity(target).is_some());
        let snapshots = self.external_request_snapshots();
        self.external_tool_operation = ExternalOperation::Preparing(ExternalPreparing {
            owner: self.edit_request_owner_context(),
            items_generation: self.items_generation,
            purpose,
            input: ExternalPreparationInput::Legacy(targets),
            snapshots,
            stage: if needs_lookup {
                ExternalPreparationStage::LegacyIndex { cursor: 0 }
            } else {
                ExternalPreparationStage::Enumerating { cursor: 0 }
            },
            locators: Vec::new(),
            requests: Vec::new(),
            page_indices: HashMap::new(),
            stack_indices: HashMap::new(),
            capability: ExternalToolMenuTarget::default(),
            validation: ExternalTargetValidation::Valid,
            has_stack: false,
            last_frame: None,
            scanned_entries: 0,
        });
    }

    fn advance_external_preparation_mounted(
        &mut self,
        preparing: &mut ExternalPreparing,
    ) -> Result<bool, String> {
        if preparing.owner != self.edit_request_owner_context()
            || preparing.items_generation != self.items_generation
        {
            return Err("対象が更新されたため、外部ツールの準備をキャンセルしました".to_string());
        }
        let mut budget = ExternalPreparationBudget::new();
        while budget.next() {
            preparing.scanned_entries += 1;
            match &mut preparing.stage {
                ExternalPreparationStage::LegacyIndex { cursor } => {
                    if let Some(item) = self.items.get(*cursor) {
                        if let Some(identity) =
                            external_page_identity(&LaunchTarget::from_grid_item(Some(item)))
                        {
                            preparing.page_indices.entry(identity).or_insert(*cursor);
                        }
                        *cursor += 1;
                    } else {
                        preparing.stage = ExternalPreparationStage::Enumerating { cursor: 0 };
                    }
                }
                ExternalPreparationStage::Enumerating { cursor } => {
                    let locator = match &preparing.input {
                        ExternalPreparationInput::Legacy(targets) => {
                            if let Some(target) = targets.get(*cursor) {
                                *cursor += 1;
                                let target = target.clone();
                                let index = external_page_identity(&target).and_then(|identity| {
                                    preparing.page_indices.get(&identity).copied()
                                });
                                Some(if let Some(index) = index {
                                    ExternalTargetLocator::Listed {
                                        owner: preparing.owner,
                                        items_generation: preparing.items_generation,
                                        index,
                                        target,
                                    }
                                } else {
                                    ExternalTargetLocator::OutsideList { target }
                                })
                            } else {
                                None
                            }
                        }
                        ExternalPreparationInput::Source(ExternalTargetSource::Container {
                            path,
                        }) => {
                            if *cursor == 0 {
                                *cursor = 1;
                                path.as_ref()
                                    .map(|path| ExternalTargetLocator::OutsideList {
                                        target: LaunchTarget::RealFile(path.clone()),
                                    })
                            } else {
                                None
                            }
                        }
                        ExternalPreparationInput::Source(source) => {
                            let (primary, use_checked) = match source {
                                ExternalTargetSource::GridContext { clicked } => {
                                    (*clicked, !self.checked.is_empty())
                                }
                                ExternalTargetSource::GridKey { selected } => {
                                    (*selected, !self.checked.is_empty())
                                }
                                ExternalTargetSource::Viewer { current }
                                | ExternalTargetSource::Playback { current } => (*current, false),
                                ExternalTargetSource::Container { .. } => unreachable!(),
                            };
                            // One target has no ordering choice. Keep its direct lookup;
                            // multiple checked targets always follow the display cursor.
                            let single_current = !use_checked
                                || (self.checked.len() == 1
                                    && primary.is_some_and(|index| self.checked.contains(&index)));
                            let index = if single_current {
                                if *cursor == 0 {
                                    *cursor = 1;
                                    primary
                                } else {
                                    None
                                }
                            } else if preparing.locators.len() < self.checked.len() {
                                if let Some(index) = self.current_grid_order().get(*cursor).copied()
                                {
                                    *cursor += 1;
                                    if self.checked.contains(&index) {
                                        Some(index)
                                    } else {
                                        continue;
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            if let Some(index) = index {
                                self.items
                                    .get(index)
                                    .map(|item| ExternalTargetLocator::Listed {
                                        owner: preparing.owner,
                                        items_generation: preparing.items_generation,
                                        index,
                                        target: LaunchTarget::from_grid_item(Some(item)),
                                    })
                            } else if use_checked
                                && preparing.locators.len() < self.checked.len()
                                && *cursor < self.current_grid_order().len()
                            {
                                continue;
                            } else {
                                None
                            }
                        }
                        ExternalPreparationInput::Captured => None,
                    };
                    if let Some(locator) = locator {
                        preparing.validation.record(locator.target());
                        preparing.has_stack |= matches!(locator.target(), LaunchTarget::Stack(_));
                        preparing.capability.add_target(locator.target());
                        preparing.locators.push(locator);
                    } else {
                        if preparing.locators.is_empty() {
                            return Err("外部ツールへ渡す対象が選択されていません".to_string());
                        }
                        preparing.validation.finish()?;
                        if matches!(preparing.purpose, ExternalPreparationPurpose::ForPicker(_)) {
                            preparing.stage = ExternalPreparationStage::Complete;
                        } else if preparing.has_stack {
                            preparing.stage = ExternalPreparationStage::StackIndex { cursor: 0 };
                        } else {
                            preparing.stage = ExternalPreparationStage::Snapshot {
                                cursor: 0,
                                member_cursor: 0,
                            };
                        }
                    }
                }
                ExternalPreparationStage::StackIndex { cursor } => {
                    let view = self
                        .stack_view
                        .as_deref()
                        .ok_or_else(|| "スタックの内容を解決できませんでした".to_string())?;
                    if let Some(group) = view.groups.get(*cursor) {
                        preparing
                            .stack_indices
                            .entry(group.key.clone())
                            .or_insert(*cursor);
                        *cursor += 1;
                    } else {
                        preparing.stage = ExternalPreparationStage::Snapshot {
                            cursor: 0,
                            member_cursor: 0,
                        };
                    }
                }
                ExternalPreparationStage::Snapshot {
                    cursor,
                    member_cursor,
                } => {
                    let ExternalPreparationPurpose::ForTool(tool) = &preparing.purpose else {
                        unreachable!()
                    };
                    let Some(locator) = preparing.locators.get(*cursor) else {
                        preparing.stage = ExternalPreparationStage::Complete;
                        continue;
                    };
                    let index = match locator {
                        ExternalTargetLocator::Listed {
                            owner,
                            items_generation,
                            index,
                            target,
                        } => {
                            if *owner != preparing.owner
                                || *items_generation != self.items_generation
                                || LaunchTarget::from_grid_item(self.items.get(*index)) != *target
                            {
                                return Err(
                                    "対象が更新されたため、外部ツールの準備をキャンセルしました"
                                        .to_string(),
                                );
                            }
                            Some(*index)
                        }
                        ExternalTargetLocator::OutsideList { .. } => None,
                    };
                    if let LaunchTarget::Stack(key) = locator.target() {
                        let group = preparing
                            .stack_indices
                            .get(key)
                            .copied()
                            .ok_or_else(|| "スタックの内容が更新されました".to_string())?;
                        let members = &self
                            .stack_view
                            .as_deref()
                            .ok_or_else(|| "スタックの内容を解決できませんでした".to_string())?
                            .groups[group]
                            .members;
                        if members.is_empty() {
                            return Err(
                                "スタックに外部ツールへ渡せるページがありません".to_string()
                            );
                        }
                        let member = &members[*member_cursor];
                        let target = if member.is_video {
                            LaunchTarget::RealFile(member.path.clone())
                        } else {
                            LaunchTarget::ImagePage(member.path.clone())
                        };
                        preparing.requests.push(self.materialize_target(
                            tool,
                            &target,
                            None,
                            &preparing.snapshots,
                        )?);
                        *member_cursor += 1;
                        if *member_cursor == members.len() {
                            *cursor += 1;
                            *member_cursor = 0;
                        }
                    } else if preparing.locators.len() == 1
                        && preparing
                            .snapshots
                            .spread
                            .is_some_and(|(current, _, _)| Some(current) == index)
                        && tool.effective_spread() != SpreadPolicy::MainPageOnly
                    {
                        let (_, screen_order, reading_order) =
                            preparing.snapshots.spread.expect("checked above");
                        match tool.effective_spread() {
                            SpreadPolicy::Merged => {
                                preparing.requests.push(self.merged_spread_target(
                                    tool,
                                    screen_order[0],
                                    screen_order[1],
                                    &preparing.snapshots,
                                )?);
                                *cursor += 1;
                            }
                            SpreadPolicy::BothPages => {
                                let index = reading_order[*member_cursor];
                                let target = LaunchTarget::from_grid_item(self.items.get(index));
                                preparing.requests.push(self.materialize_target(
                                    tool,
                                    &target,
                                    Some(index),
                                    &preparing.snapshots,
                                )?);
                                *member_cursor += 1;
                                if *member_cursor == 2 {
                                    *cursor += 1;
                                    *member_cursor = 0;
                                }
                            }
                            SpreadPolicy::MainPageOnly => unreachable!(),
                        }
                    } else {
                        preparing.requests.push(self.materialize_target(
                            tool,
                            locator.target(),
                            index,
                            &preparing.snapshots,
                        )?);
                        *cursor += 1;
                    }
                }
                ExternalPreparationStage::Complete => break,
            }
        }
        if crate::perf::is_enabled() {
            crate::perf::event(
                "external_tool",
                "prepare_frame",
                None,
                self.input_seq,
                &[
                    (
                        "ms",
                        serde_json::Value::from(budget.started.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("entries", serde_json::Value::from(budget.entries)),
                    (
                        "total_entries",
                        serde_json::Value::from(preparing.scanned_entries),
                    ),
                ],
            );
        }
        Ok(matches!(
            preparing.stage,
            ExternalPreparationStage::Complete
        ))
    }

    fn advance_external_preparation(&mut self) {
        let ExternalOperation::Preparing(mut preparing) =
            std::mem::take(&mut self.external_tool_operation)
        else {
            return;
        };
        if preparing.last_frame == Some(self.frame_counter) {
            self.external_tool_operation = ExternalOperation::Preparing(preparing);
            return;
        }
        preparing.last_frame = Some(self.frame_counter);
        let result = if self.edit_request_owner_context() == preparing.owner {
            Some(self.advance_external_preparation_mounted(&mut preparing))
        } else {
            self.with_owner_viewer_context(preparing.owner, |app| {
                app.advance_external_preparation_mounted(&mut preparing)
            })
        };
        match result {
            Some(Ok(false)) => {
                self.external_tool_operation = ExternalOperation::Preparing(preparing)
            }
            Some(Ok(true)) => match preparing.purpose {
                ExternalPreparationPurpose::ForPicker(target_kind) => {
                    if self.settings.external_tools.is_empty() {
                        self.show_feedback_toast("外部ツールが登録されていません".to_string());
                        return;
                    }
                    self.external_tool_operation =
                        ExternalOperation::Picker(ExternalToolPickerRequest {
                            has_stack: preparing.has_stack,
                            locators: preparing.locators,
                            capability: preparing.capability,
                            owner: preparing.owner,
                            items_generation: preparing.items_generation,
                            target_kind,
                        });
                }
                ExternalPreparationPurpose::ForTool(tool) => {
                    match evaluate_target_count(
                        tool.selection,
                        preparing.requests.len(),
                        tool.confirmation_threshold,
                        tool.max_targets,
                    ) {
                        Ok(target_count_decision) => {
                            let operation = ExternalQueuedOperation::Materialize(
                                ExternalMaterializeOperation {
                                    tool,
                                    targets: preparing.requests,
                                    target_count_decision,
                                },
                            );
                            match launch_confirmation(operation) {
                                Ok(operation) => self.start_external_queued_operation(operation),
                                Err(confirmation) => {
                                    self.external_tool_operation =
                                        ExternalOperation::Confirmation(confirmation)
                                }
                            }
                        }
                        Err(error) => {
                            self.show_feedback_toast(format!("{}: {error}", tool.display_name()))
                        }
                    }
                }
            },
            Some(Err(error)) => self.show_feedback_toast(error),
            None => self.show_feedback_toast(
                "対象のウィンドウを閉じたため、外部ツールの準備をキャンセルしました".to_string(),
            ),
        }
    }

    fn show_external_tool_preparing(&mut self, ctx: &egui::Context) {
        let ExternalOperation::Preparing(preparing) = &self.external_tool_operation else {
            return;
        };
        let count = preparing.requests.len();
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("external_tool_preparing")).show(ctx, |ui| {
            ui.set_min_width(380.0);
            cancel = draw_external_tool_preparing_body(ui, count);
        });
        if cancel || response.should_close() || self.dialog_escape_pressed(ctx) {
            self.external_tool_operation = ExternalOperation::Idle;
        } else {
            self.advance_external_preparation();
            ctx.request_repaint();
        }
    }

    pub(crate) fn start_open_with(
        &mut self,
        ctx: &egui::Context,
        display_name: String,
        launch: ExternalToolLaunch,
        file: PathBuf,
    ) {
        // ネットワーク EXE の確認 modal も、操作した窓が所有する。
        self.retire_external_operation();
        self.external_tool_modal_viewport = ctx.viewport_id();
        let request = build_open_with_launch_request(display_name.clone(), launch, file);
        let operation = ExternalLaunchOperation {
            tool_name: display_name,
            target_count: request.files.len(),
            requests: vec![request],
            target_count_decision: TargetCountDecision::Proceed,
        };
        match launch_confirmation(ExternalQueuedOperation::Ready(operation)) {
            Ok(operation) => self.start_external_queued_operation(operation),
            Err(confirmation) => {
                self.external_tool_operation = ExternalOperation::Confirmation(confirmation);
            }
        }
    }

    fn start_external_launch_operation(&mut self, operation: ExternalLaunchOperation) {
        self.retire_external_operation();
        let tool_name = operation.tool_name.clone();
        let target_count = operation.target_count;
        match start_launch_worker(operation, self.main_hwnd) {
            Ok(pending) => {
                self.external_tool_operation =
                    ExternalOperation::Launching(ExternalLaunching::Ready(pending))
            }
            Err(error) if target_count == 1 => {
                self.show_feedback_toast(format!("{tool_name}: {error}"))
            }
            Err(error) => self.show_feedback_toast(format!(
                "{tool_name}: {target_count} 件の起動を開始できませんでした ({error})"
            )),
        }
    }

    fn start_external_queued_operation(&mut self, operation: ExternalQueuedOperation) {
        self.retire_external_operation();
        match operation {
            ExternalQueuedOperation::Ready(operation) => {
                self.start_external_launch_operation(operation)
            }
            ExternalQueuedOperation::Materialize(operation) => {
                let generation = self.external_tool_materializer.begin_generation();
                let session = self.external_tool_materializer.session();
                let tool_name = operation.tool_name();
                let target_count = operation.target_count();
                match start_materialize_launch_worker(
                    operation,
                    session,
                    generation,
                    self.main_hwnd,
                    self.local_ai_activity_lease(),
                ) {
                    Ok(pending) => {
                        self.external_tool_operation = ExternalOperation::Materializing(pending)
                    }
                    Err(error) if target_count == 1 => {
                        self.show_feedback_toast(format!("{tool_name}: {error}"))
                    }
                    Err(error) => self.show_feedback_toast(format!(
                        "{tool_name}: {target_count} 件の準備を開始できませんでした ({error})"
                    )),
                }
            }
        }
    }

    pub(crate) fn poll_external_tool_launch(&mut self, ctx: &egui::Context) {
        let mut finished = Vec::new();
        let operation = std::mem::take(&mut self.external_tool_operation);
        self.external_tool_operation = match operation {
            ExternalOperation::Materializing(mut pending) => {
                if !self
                    .external_tool_materializer
                    .generation_is_current(pending.generation)
                {
                    pending.cancel(false);
                }
                if let Some(completion) = pending.poll_completion() {
                    if !pending.user_cancelled {
                        finished.push(completion);
                    }
                    ExternalOperation::Idle
                } else {
                    ExternalOperation::Materializing(pending)
                }
            }
            ExternalOperation::Launching(ExternalLaunching::Materialized(mut pending)) => {
                if !self
                    .external_tool_materializer
                    .generation_is_current(pending.generation)
                {
                    pending.cancel(false);
                }
                if let Some(completion) = pending.poll_completion() {
                    if !pending.user_cancelled {
                        finished.push(completion);
                    }
                    ExternalOperation::Idle
                } else {
                    ExternalOperation::Launching(ExternalLaunching::Materialized(pending))
                }
            }
            ExternalOperation::Launching(ExternalLaunching::Ready(mut pending)) => {
                if let Some(completion) = pending.poll_completion() {
                    finished.push(completion);
                    ExternalOperation::Idle
                } else {
                    ExternalOperation::Launching(ExternalLaunching::Ready(pending))
                }
            }
            other => other,
        };
        let mut index = 0;
        while index < self.external_tool_retired_launch.len() {
            if let Some(completion) = self.external_tool_retired_launch[index].poll_completion() {
                self.external_tool_retired_launch.remove(index);
                finished.push(completion);
            } else {
                index += 1;
            }
        }
        let mut index = 0;
        while index < self.external_tool_retired_materialize.len() {
            if let Some(completion) =
                self.external_tool_retired_materialize[index].poll_completion()
            {
                let pending = self.external_tool_retired_materialize.remove(index);
                if !pending.user_cancelled {
                    finished.push(completion);
                }
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
            self.show_feedback_toast(external_launch_completion_summary(&finished));
        }
        if !matches!(self.external_tool_operation, ExternalOperation::Idle)
            || !self.external_tool_retired_launch.is_empty()
            || !self.external_tool_retired_materialize.is_empty()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    pub(crate) fn authorize_external_tool_launch_boundaries_after_ui(&mut self) {
        let operation = std::mem::take(&mut self.external_tool_operation);
        self.external_tool_operation = match operation {
            ExternalOperation::Materializing(mut pending) => {
                let current = self
                    .external_tool_materializer
                    .generation_is_current(pending.generation);
                if pending.take_launch_ui_checkpoint()
                    && current
                    && pending.can_cancel_before_launch()
                {
                    pending.resolve_launch_boundary(true);
                } else if !current {
                    pending.cancel(false);
                }
                if pending.launch_decision_tx.is_none() && !pending.cancel.load(Ordering::Acquire) {
                    ExternalOperation::Launching(ExternalLaunching::Materialized(pending))
                } else {
                    ExternalOperation::Materializing(pending)
                }
            }
            other => other,
        };
    }

    fn show_external_tool_launch_confirmation(&mut self, ctx: &egui::Context) {
        let ExternalOperation::Confirmation(confirmation) = &self.external_tool_operation else {
            return;
        };
        let tool_name = confirmation.operation.tool_name();
        let executable = confirmation
            .network_executable
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let confirmation_target_count = match confirmation.operation.target_count_decision() {
            TargetCountDecision::Proceed => None,
            TargetCountDecision::Confirm { target_count } => Some(target_count),
        };
        let mut launch = false;
        let mut cancel = false;
        let response =
            egui::Modal::new(egui::Id::new("external_tool_launch_confirmation")).show(ctx, |ui| {
                ui.set_min_width(440.0);
                if confirmation_target_count.is_some() {
                    ui.heading("複数の対象を外部ツールで開きますか？");
                } else {
                    ui.heading("ネットワーク上のツールを起動しますか？");
                }
                ui.add_space(8.0);
                ui.label(format!("ツール: {tool_name}"));
                if let Some(count) = confirmation_target_count {
                    ui.label(format!("{count} 件を外部ツールへ渡します。"));
                }
                if !executable.is_empty() {
                    ui.label(format!("プログラム / スクリプト: {executable}"));
                }
                ui.add_space(8.0);
                if !executable.is_empty() {
                    ui.label("信頼できる場所であることを確認してから起動してください。");
                } else {
                    ui.label(
                        "多数の対象を渡すため、外部ツールの動作に時間がかかる可能性があります。",
                    );
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
            if let ExternalOperation::Confirmation(confirmation) =
                std::mem::take(&mut self.external_tool_operation)
            {
                self.start_external_queued_operation(confirmation.operation);
            }
        } else if cancel {
            self.external_tool_operation = ExternalOperation::Idle;
        }
    }

    /// 準備進捗 modal がこの frame で描かれるか。**表示条件の唯一の持ち主**にして、
    /// 入力ブロック判定 (`modal_dialog_block_reason`) もここから導く。
    pub(crate) fn external_tool_materialize_progress_visible(&self) -> bool {
        match &self.external_tool_operation {
            ExternalOperation::Materializing(pending)
            | ExternalOperation::Launching(ExternalLaunching::Materialized(pending)) => self
                .external_tool_materializer
                .generation_is_current(pending.generation),
            _ => false,
        }
    }

    fn show_external_tool_materialize_progress(&mut self, ctx: &egui::Context) {
        let pending = match &self.external_tool_operation {
            ExternalOperation::Materializing(pending)
            | ExternalOperation::Launching(ExternalLaunching::Materialized(pending))
                if self
                    .external_tool_materializer
                    .generation_is_current(pending.generation) =>
            {
                pending
            }
            _ => return,
        };
        let (completed, total, stage) = pending.progress.snapshot();
        let can_cancel = pending.can_cancel_before_launch();
        let mut cancel = false;
        let response =
            egui::Modal::new(egui::Id::new("external_tool_materialize_progress")).show(ctx, |ui| {
                ui.set_min_width(380.0);
                ui.heading("外部ツールへ渡すファイルを準備中");
                ui.add_space(8.0);
                ui.label(stage);
                ui.add(
                    egui::ProgressBar::new(completed as f32 / total.max(1) as f32)
                        .text(format!("{completed} / {total}")),
                );
                ui.add_space(8.0);
                if can_cancel {
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                } else {
                    ui.label("外部ツールの起動を完了しています…");
                }
            });
        if can_cancel && (response.should_close() || self.dialog_escape_pressed(ctx)) {
            cancel = true;
        }
        if let ExternalOperation::Materializing(pending)
        | ExternalOperation::Launching(ExternalLaunching::Materialized(pending)) =
            &mut self.external_tool_operation
        {
            if cancel {
                pending.cancel(true);
                self.external_tool_materializer.cancel_all();
            } else if can_cancel {
                pending.mark_launch_ui_checkpoint();
            }
        }
    }

    pub(crate) fn shutdown_external_tool_materializer(&mut self) {
        self.retire_external_operation();
        self.external_tool_materializer.cancel_all();
        for pending in &mut self.external_tool_retired_materialize {
            pending.join_for_exit();
        }
        self.external_tool_retired_materialize.clear();
        self.external_tool_materializer.shutdown();
    }

    /// 外部ツールの modal 面をまとめて描く。**追加するときは必ずここへ足す。**
    ///
    /// 3 つとも `modal_dialog_block_reason` の対象なので、**描き漏らすと「ダイアログが
    /// 無いのに入力だけ止まる」**になる。実際に進捗 modal だけを複製して、ピッカーと
    /// 起動確認を落としていた (2026-09-02、Codex Sol の指摘)。
    ///
    /// 呼ぶ場所が 2 つあるのは、フルスクリーン中は main update の tail が飛ぶため
    /// (`app.rs` の early return)。専用 viewport では両方通るので、**この frame で先に
    /// 到達した方が描く**。frame 番号で持つのは、tail ごと飛ぶ frame があり bool では
    /// clear する場所が無いから。
    pub(crate) fn show_external_tool_modals(&mut self, ctx: &egui::Context) {
        let viewport = ctx.viewport_id();
        if self.external_tool_launch_ui_frame == Some((viewport, self.frame_counter)) {
            return;
        }
        self.external_tool_launch_ui_frame = Some((viewport, self.frame_counter));
        let owner = self.external_tool_modal_viewport;
        if viewport == owner {
            self.external_tool_modal_owner_drawn_frame = Some(self.frame_counter);
        }
        if !external_tool_modal_viewport_draws(
            viewport,
            owner,
            self.external_tool_modal_owner_drawn_frame == Some(self.frame_counter),
        ) {
            return;
        }
        self.show_external_tool_preparing(ctx);
        self.show_external_tool_picker(ctx);
        self.show_external_tool_launch_confirmation(ctx);
        self.show_external_tool_materialize_progress(ctx);
    }

    fn show_external_tool_picker(&mut self, ctx: &egui::Context) {
        let ExternalOperation::Picker(request) = &self.external_tool_operation else {
            return;
        };
        let target_kind = request.target_kind;
        let tools = external_tool_picker_items_for_capability(
            &self.settings.external_tools,
            request.capability,
        );
        let mut selected_tool = None;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("external_tool_picker")).show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.heading(match target_kind {
                ExternalToolPickerTargetKind::GridItems => "外部ツールを選択",
                ExternalToolPickerTargetKind::Container => "フォルダー / 本を開く外部ツールを選択",
            });
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for tool in &tools {
                        let response = ui.add_enabled_ui(tool.enabled, |ui| {
                            ui.add_sized(
                                [ui.available_width(), 28.0],
                                egui::Button::new(format!("{}. {}", tool.slot, tool.label)),
                            )
                        });
                        let response = if let Some(reason) = tool.disabled_reason {
                            response.inner.on_disabled_hover_text(reason)
                        } else {
                            response.inner
                        };
                        if tool.enabled && response.clicked() {
                            selected_tool = Some(tool.tool_id);
                        }
                    }
                });
            ui.add_space(8.0);
            if ui.button("キャンセル").clicked() {
                cancel = true;
            }
        });
        if response.should_close() || self.dialog_escape_pressed(ctx) {
            cancel = true;
        }
        if let Some(tool_id) = selected_tool {
            let tool = self
                .settings
                .external_tools
                .iter()
                .find(|tool| tool.id == tool_id)
                .cloned();
            let request = std::mem::take(&mut self.external_tool_operation);
            if let (Some(tool), ExternalOperation::Picker(request)) = (tool, request) {
                let snapshots = self
                    .with_owner_viewer_context(request.owner, |app| {
                        (app.items_generation == request.items_generation)
                            .then(|| app.external_request_snapshots())
                    })
                    .flatten();
                if let Some(snapshots) = snapshots {
                    let has_stack = request.has_stack;
                    self.external_tool_operation =
                        ExternalOperation::Preparing(ExternalPreparing {
                            owner: request.owner,
                            items_generation: request.items_generation,
                            purpose: ExternalPreparationPurpose::ForTool(tool),
                            input: ExternalPreparationInput::Captured,
                            stage: if has_stack {
                                ExternalPreparationStage::StackIndex { cursor: 0 }
                            } else {
                                ExternalPreparationStage::Snapshot {
                                    cursor: 0,
                                    member_cursor: 0,
                                }
                            },
                            snapshots,
                            locators: request.locators,
                            requests: Vec::new(),
                            page_indices: HashMap::new(),
                            stack_indices: HashMap::new(),
                            capability: request.capability,
                            validation: ExternalTargetValidation::Valid,
                            has_stack,
                            last_frame: None,
                            scanned_entries: 0,
                        });
                } else {
                    self.show_feedback_toast(
                        "対象が移動したため、外部ツールをもう一度選択してください".to_string(),
                    );
                }
            } else {
                self.show_feedback_toast("外部ツールを選択できませんでした".to_string());
            }
        } else if cancel {
            self.external_tool_operation = ExternalOperation::Idle;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finish_picker_preparation(app: &mut crate::app::App) {
        for _ in 0..1000 {
            if !matches!(app.external_tool_operation, ExternalOperation::Preparing(_)) {
                return;
            }
            app.frame_counter += 1;
            app.advance_external_preparation();
        }
        panic!("picker preparation did not finish");
    }

    #[test]
    fn file_list_template_occurrences_mixed_files_alias_and_unknown_literals() {
        let files = vec![
            PathBuf::from(r"C:\two words\一.jpg"),
            PathBuf::from(r"C:\media\二.jpg"),
        ];
        let list = Path::new(r"C:\list directory\一覧.txt");
        let tokens = split_argument_template(
            "--list={file_list} {file_list} --both={file}:{file_list} {unknown}",
        );
        let arguments = expand_arguments_with_file_list(&tokens, &files, Some(list));
        assert_eq!(
            arguments,
            vec![
                OsString::from(r"--list=C:\list directory\一覧.txt"),
                OsString::from(list),
                OsString::from(r"--both=C:\two words\一.jpg:C:\list directory\一覧.txt"),
                OsString::from(r"--both=C:\media\二.jpg:C:\list directory\一覧.txt"),
                OsString::from("{unknown}"),
            ]
        );
        assert_eq!(split_argument_template("{file_list}"), vec!["{file_list}"]);
        assert_eq!(
            split_argument_template("{unknown}"),
            vec!["{unknown}", "{files}"]
        );
    }

    #[test]
    fn file_list_preview_is_illustrative_and_association_ignores_templates() {
        let mut tool = ExternalTool::defaults_for_viewing();
        tool.launch = ExternalToolLaunch::Executable(PathBuf::from("fake.exe"));
        tool.arguments = "{file_list}".to_string();
        tool.selection = SelectionPolicy::Batch;
        let target = LaunchTarget::RealFile(PathBuf::from("example.jpg"));
        let preview = build_launch_request_for_preview(&tool, &target).unwrap();
        assert_eq!(preview.arguments.len(), 1);
        assert!(
            preview.arguments[0]
                .to_string_lossy()
                .ends_with("file-list-1.txt")
        );
        assert!(
            build_launch_request(&tool, &target)
                .unwrap_err()
                .contains("worker")
        );
        tool.launch = ExternalToolLaunch::Association {
            handler_id: "fake".to_string(),
        };
        let request = build_launch_request(&tool, &target).unwrap();
        assert!(request.arguments.is_empty());
    }

    fn file_list_test_tool(selection: SelectionPolicy) -> ExternalTool {
        let mut tool = ExternalTool::defaults_for_viewing();
        tool.launch = ExternalToolLaunch::Executable(PathBuf::from("fake.exe"));
        tool.arguments = "{file_list} {file_list}".to_string();
        tool.selection = selection;
        tool.payload = PayloadPolicy::TempOriginal;
        tool
    }

    fn file_list_materialize_request(
        path: PathBuf,
        image_page: bool,
    ) -> crate::materializer::MaterializeRequest {
        crate::materializer::MaterializeRequest {
            source: crate::materializer::MaterializeSource::File { path, image_page },
            raw_brightness: crate::raw::RawBrightness::default(),
            policy: crate::materializer::MaterializePolicy::TempOriginal,
            page_edits: None,
            pdf_render_long_edge: 4096,
        }
    }

    fn run_file_list_fake_worker(
        manager: &crate::materializer::Materializer,
        tool: ExternalTool,
        targets: Vec<crate::materializer::MaterializeRequest>,
        cancel: Arc<AtomicBool>,
        launcher: impl FnMut(
            ExternalLaunchRequest,
            Option<isize>,
        ) -> Result<ExternalLaunchOutcome, String>,
    ) -> ExternalLaunchCompletion {
        let generation = manager.begin_generation();
        let total = targets.len();
        let operation = ExternalMaterializeOperation {
            tool,
            targets,
            target_count_decision: TargetCountDecision::Proceed,
        };
        let (boundary_tx, _boundary_rx) = mpsc::channel();
        let (decision_tx, decision_rx) = mpsc::channel();
        decision_tx.send(MaterializeLaunchDecision::Launch).unwrap();
        run_materialize_launch_operation_with_launcher(
            operation,
            &mut manager.session(),
            generation,
            None,
            &cancel,
            &MaterializeProgress::new(total),
            &boundary_tx,
            &decision_rx,
            launcher,
        )
    }

    #[test]
    fn external_preparation_delivers_display_order_to_batch_args_lists_and_each_launches() {
        for selection in [SelectionPolicy::Each, SelectionPolicy::Batch] {
            for arguments in [
                "{files}",
                "--playlist={file_list}",
                "{files} --playlist={file_list}",
            ] {
                for source in [
                    ExternalTargetSource::GridContext { clicked: Some(2) },
                    ExternalTargetSource::GridKey { selected: Some(2) },
                ] {
                    let temp = tempfile::tempdir().unwrap();
                    let expected: Vec<_> = ["one.mp4", "two.mp4", "three.mp4"]
                        .into_iter()
                        .map(|name| {
                            let path = temp.path().join(name);
                            std::fs::write(&path, b"source").unwrap();
                            path
                        })
                        .collect();
                    let mut app = crate::app::setup_app_for_test();
                    app.items = [1, 0, 2]
                        .into_iter()
                        .map(|index| crate::grid_item::GridItem::Video(expected[index].clone()))
                        .collect();
                    app.visible_indices = vec![1, 0, 2];
                    app.checked.extend([2, 0, 1]);
                    app.selected = Some(2);
                    let mut tool = file_list_test_tool(selection);
                    tool.arguments = arguments.to_string();
                    tool.payload = PayloadPolicy::OriginalFile;
                    let descriptor = app.external_tool_target_descriptor(source);
                    app.begin_external_preparation(
                        egui::ViewportId::ROOT,
                        descriptor,
                        ExternalPreparationPurpose::ForTool(tool.clone()),
                    );
                    let mut preparing = take_preparing(&mut app);
                    finish_snapshot_for_test(&mut app, &mut preparing);
                    let manager = crate::materializer::Materializer::new_at_for_test(
                        temp.path().join("artifacts"),
                        513,
                    );
                    let mut delivered = Vec::new();
                    let mut launches = 0;
                    let completion = run_file_list_fake_worker(
                        &manager,
                        tool,
                        preparing.requests,
                        Arc::new(AtomicBool::new(false)),
                        |request, _| {
                            launches += 1;
                            let mut expected_args: Vec<OsString> = Vec::new();
                            if arguments.contains("{files}") {
                                expected_args
                                    .extend(request.files.iter().map(|p| p.as_os_str().to_owned()));
                            }
                            if arguments.contains("{file_list}") {
                                let argument = request.arguments.last().unwrap();
                                let list = PathBuf::from(
                                    argument
                                        .to_str()
                                        .unwrap()
                                        .strip_prefix("--playlist=")
                                        .unwrap(),
                                );
                                let rows: String = request
                                    .files
                                    .iter()
                                    .map(|path| format!("{}\r\n", path.to_str().unwrap()))
                                    .collect();
                                assert_eq!(std::fs::read(list).unwrap(), rows.as_bytes());
                                expected_args.push(argument.clone());
                            }
                            assert_eq!(request.arguments, expected_args);
                            delivered.extend(request.files);
                            Ok(ExternalLaunchOutcome::all_launched(None))
                        },
                    );
                    assert!(completion.failures.is_empty(), "{:?}", completion.failures);
                    assert_eq!(completion.succeeded_target_count, 3);
                    assert_eq!(
                        launches,
                        if selection == SelectionPolicy::Each {
                            3
                        } else {
                            1
                        }
                    );
                    assert_eq!(delivered, expected, "{selection:?}: {arguments}");
                }
            }
        }
    }

    #[test]
    fn file_list_worker_single_each_batch_use_exact_successful_paths_and_unique_each_lists() {
        for selection in [
            SelectionPolicy::Single,
            SelectionPolicy::Each,
            SelectionPolicy::Batch,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let mut manager = crate::materializer::Materializer::new_at_for_test(
                temp.path().join("artifacts"),
                501,
            );
            let count = if selection == SelectionPolicy::Single {
                1
            } else {
                3
            };
            let paths: Vec<_> = (0..count)
                .map(|index| {
                    let path = temp.path().join(format!("原動画 {index}.mp4"));
                    std::fs::write(&path, b"source").unwrap();
                    path
                })
                .collect();
            let mut lists = Vec::new();
            let completion = run_file_list_fake_worker(
                &manager,
                file_list_test_tool(selection),
                paths
                    .iter()
                    .cloned()
                    .map(|path| file_list_materialize_request(path, false))
                    .collect(),
                Arc::new(AtomicBool::new(false)),
                |request, _| {
                    assert_eq!(request.arguments.len(), 2);
                    assert_eq!(request.arguments[0], request.arguments[1]);
                    let list = PathBuf::from(&request.arguments[0]);
                    let expected: String = request
                        .files
                        .iter()
                        .map(|path| format!("{}\r\n", path.to_str().unwrap()))
                        .collect();
                    assert_eq!(std::fs::read(&list).unwrap(), expected.as_bytes());
                    lists.push(list);
                    Ok(ExternalLaunchOutcome::all_launched(None))
                },
            );
            assert_eq!(completion.succeeded_target_count, count);
            assert_eq!(
                lists.len(),
                if selection == SelectionPolicy::Batch {
                    1
                } else {
                    count
                }
            );
            assert_eq!(lists.iter().collect::<HashSet<_>>().len(), lists.len());
            assert!(lists.iter().all(|path| path.exists()));
            manager.shutdown();
            assert!(lists.iter().all(|path| !path.exists()));
        }
    }

    #[test]
    fn file_list_worker_partial_materialization_and_launch_failure_preserve_launched_lists_only() {
        let temp = tempfile::tempdir().unwrap();
        let mut manager =
            crate::materializer::Materializer::new_at_for_test(temp.path().join("artifacts"), 502);
        let first = temp.path().join("first.mp4");
        let last = temp.path().join("last.mp4");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&last, b"last").unwrap();
        let targets = vec![
            file_list_materialize_request(first.clone(), false),
            file_list_materialize_request(temp.path().join("missing.jpg"), true),
            file_list_materialize_request(last.clone(), false),
        ];
        let mut lists = Vec::new();
        let completion = run_file_list_fake_worker(
            &manager,
            file_list_test_tool(SelectionPolicy::Each),
            targets,
            Arc::new(AtomicBool::new(false)),
            |request, _| {
                let list = PathBuf::from(&request.arguments[0]);
                assert_eq!(
                    std::fs::read(&list).unwrap(),
                    format!("{}\r\n", request.files[0].to_str().unwrap()).as_bytes()
                );
                lists.push(list);
                if request.files[0] == last {
                    Err("fake spawn failed".to_string())
                } else {
                    Ok(ExternalLaunchOutcome::all_launched(None))
                }
            },
        );
        assert_eq!(completion.succeeded_target_count, 1);
        assert_eq!(completion.failures.len(), 2);
        assert_eq!(lists.len(), 2);
        assert!(lists[0].exists());
        assert!(!lists[1].exists());
        manager.shutdown();
        assert!(!lists[0].exists());
    }

    #[test]
    fn file_list_worker_batch_omits_failed_materialization_and_keeps_order() {
        let temp = tempfile::tempdir().unwrap();
        let mut manager =
            crate::materializer::Materializer::new_at_for_test(temp.path().join("artifacts"), 503);
        let paths: Vec<_> = ["three.mp4", "one.mp4"]
            .into_iter()
            .map(|name| {
                let path = temp.path().join(name);
                std::fs::write(&path, b"source").unwrap();
                path
            })
            .collect();
        let targets = vec![
            file_list_materialize_request(paths[0].clone(), false),
            file_list_materialize_request(temp.path().join("missing.png"), true),
            file_list_materialize_request(paths[1].clone(), false),
        ];
        let mut invocations = 0;
        let completion = run_file_list_fake_worker(
            &manager,
            file_list_test_tool(SelectionPolicy::Batch),
            targets,
            Arc::new(AtomicBool::new(false)),
            |request, _| {
                invocations += 1;
                assert_eq!(request.files, paths);
                assert_eq!(
                    std::fs::read(PathBuf::from(&request.arguments[0])).unwrap(),
                    format!(
                        "{}\r\n{}\r\n",
                        paths[0].to_str().unwrap(),
                        paths[1].to_str().unwrap()
                    )
                    .as_bytes()
                );
                Ok(ExternalLaunchOutcome::all_launched(None))
            },
        );
        assert_eq!(invocations, 1);
        assert_eq!(completion.succeeded_target_count, 2);
        assert_eq!(completion.failures.len(), 1);
        manager.shutdown();
    }

    #[test]
    fn file_list_worker_cancel_mid_each_stops_new_launch_without_deleting_prior_list() {
        let temp = tempfile::tempdir().unwrap();
        let mut manager =
            crate::materializer::Materializer::new_at_for_test(temp.path().join("artifacts"), 504);
        let cancel = Arc::new(AtomicBool::new(false));
        let targets = (0..2)
            .map(|index| {
                let path = temp.path().join(format!("{index}.mp4"));
                std::fs::write(&path, b"source").unwrap();
                file_list_materialize_request(path, false)
            })
            .collect();
        let mut lists = Vec::new();
        let completion = run_file_list_fake_worker(
            &manager,
            file_list_test_tool(SelectionPolicy::Each),
            targets,
            cancel.clone(),
            |request, _| {
                lists.push(PathBuf::from(&request.arguments[0]));
                cancel.store(true, Ordering::Release);
                Ok(ExternalLaunchOutcome::all_launched(None))
            },
        );
        assert_eq!(completion.succeeded_target_count, 1);
        assert_eq!(lists.len(), 1);
        assert!(lists[0].exists());
        manager.shutdown();
        assert!(!lists[0].exists());
    }

    #[test]
    fn file_list_worker_success_applies_keep_temp_to_list_and_media_together() {
        for keep_temp in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let mut manager = crate::materializer::Materializer::new_at_for_test(
                temp.path().join("artifacts"),
                505,
            );
            let original = temp.path().join("original.png");
            std::fs::write(&original, b"original bytes copied without decoding").unwrap();
            let mut tool = file_list_test_tool(SelectionPolicy::Single);
            tool.keep_temp = keep_temp;
            let mut launched = Vec::new();
            let completion = run_file_list_fake_worker(
                &manager,
                tool,
                vec![file_list_materialize_request(original.clone(), true)],
                Arc::new(AtomicBool::new(false)),
                |request, _| {
                    assert_ne!(request.files[0], original);
                    launched = vec![
                        PathBuf::from(&request.arguments[0]),
                        request.files[0].clone(),
                    ];
                    assert!(launched.iter().all(|path| path.exists()));
                    Ok(ExternalLaunchOutcome::all_launched(None))
                },
            );
            assert_eq!(completion.succeeded_target_count, 1);
            manager.shutdown();
            assert!(launched.iter().all(|path| path.exists() == keep_temp));
            assert!(original.exists());
        }
    }

    #[test]
    fn file_list_worker_prepares_closed_lists_before_ack_and_cancellation_drops_both_leases() {
        for selection in [SelectionPolicy::Each, SelectionPolicy::Batch] {
            for supersede in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let root = temp.path().join("artifacts");
                let mut manager =
                    crate::materializer::Materializer::new_at_for_test(root.clone(), 506);
                let generation = manager.begin_generation();
                let mut session = manager.session();
                let targets = (0..2)
                    .map(|index| {
                        let path = temp.path().join(format!("original-{index}.png"));
                        std::fs::write(&path, b"bytes copied without decoding").unwrap();
                        file_list_materialize_request(path, true)
                    })
                    .collect();
                let operation = ExternalMaterializeOperation {
                    tool: file_list_test_tool(selection),
                    targets,
                    target_count_decision: TargetCountDecision::Proceed,
                };
                let cancel = Arc::new(AtomicBool::new(false));
                let worker_cancel = cancel.clone();
                let attempts = Arc::new(AtomicUsize::new(0));
                let worker_attempts = attempts.clone();
                let (boundary_tx, boundary_rx) = mpsc::channel();
                let (decision_tx, decision_rx) = mpsc::channel();
                let worker = std::thread::spawn(move || {
                    run_materialize_launch_operation_with_launcher(
                        operation,
                        &mut session,
                        generation,
                        None,
                        &worker_cancel,
                        &MaterializeProgress::new(2),
                        &boundary_tx,
                        &decision_rx,
                        |_, _| {
                            worker_attempts.fetch_add(1, Ordering::Relaxed);
                            Ok(ExternalLaunchOutcome::all_launched(None))
                        },
                    )
                });
                boundary_rx
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .expect("worker did not reach ACK boundary");
                let process_dir = root.join("ext-506");
                let artifacts: Vec<_> = std::fs::read_dir(&process_dir)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .collect();
                let lists: Vec<_> = artifacts
                    .iter()
                    .filter(|path| path.extension().is_some_and(|extension| extension == "txt"))
                    .collect();
                assert_eq!(
                    lists.len(),
                    if selection == SelectionPolicy::Each {
                        2
                    } else {
                        1
                    }
                );
                assert_eq!(artifacts.len(), lists.len() + 2);
                for list in &lists {
                    // Complete list bytes must exist before the UI may ACK the launch.
                    let bytes = std::fs::read(list).unwrap();
                    assert!(bytes.ends_with(b"\r\n"));
                    let expected_rows = if selection == SelectionPolicy::Each {
                        1
                    } else {
                        2
                    };
                    assert_eq!(
                        String::from_utf8(bytes).unwrap().lines().count(),
                        expected_rows
                    );
                }
                assert_eq!(attempts.load(Ordering::Relaxed), 0);
                if supersede {
                    manager.begin_generation();
                } else {
                    cancel.store(true, Ordering::Release);
                }
                let _ = decision_tx.send(if supersede {
                    MaterializeLaunchDecision::Launch
                } else {
                    MaterializeLaunchDecision::Cancel
                });
                let completion = worker.join().unwrap();
                assert_eq!(completion.succeeded_target_count, 0);
                assert_eq!(attempts.load(Ordering::Relaxed), 0);
                assert!(artifacts.iter().all(|path| !path.exists()));
                assert!(temp.path().join("original-0.png").exists());
                assert!(temp.path().join("original-1.png").exists());
                manager.shutdown();
            }
        }
    }

    #[test]
    fn file_list_unrepresentable_path_refuses_invocation_before_ack_and_spawn() {
        let temp = tempfile::tempdir().unwrap();
        let mut manager =
            crate::materializer::Materializer::new_at_for_test(temp.path().join("artifacts"), 507);
        let generation = manager.begin_generation();
        let cancel = Arc::new(AtomicBool::new(false));
        let error = match prepare_materialized_invocation(
            &file_list_test_tool(SelectionPolicy::Batch),
            vec![temp.path().join("bad\npath.mp4")],
            vec![0],
            &mut manager.session(),
            generation,
            &cancel,
        ) {
            Err(error) => error,
            Ok(_) => panic!("unrepresentable list path accepted"),
        };
        assert!(error.contains("改行") || error.contains("CR") || error.contains("LF"));
        manager.shutdown();
    }

    #[cfg(windows)]
    #[test]
    fn file_list_batch_arguments_keep_command_length_limit_for_mixed_files_only() {
        let mut tool = file_list_test_tool(SelectionPolicy::Batch);
        let files = vec![PathBuf::from(
            "x".repeat(CREATE_PROCESS_COMMAND_LINE_MAX_UTF16_UNITS),
        )];
        let list = Path::new(r"C:\lists\short.txt");
        assert_eq!(
            build_request_for_files_with_list(&tool, files.clone(), Some(list))
                .unwrap()
                .arguments
                .len(),
            2
        );
        tool.arguments = "{file_list} {files}".to_string();
        assert!(
            build_request_for_files_with_list(&tool, files, Some(list))
                .unwrap_err()
                .contains("32767")
        );
    }

    fn take_preparing(app: &mut crate::app::App) -> ExternalPreparing {
        match std::mem::take(&mut app.external_tool_operation) {
            ExternalOperation::Preparing(preparing) => preparing,
            _ => panic!("expected Preparing"),
        }
    }

    #[test]
    fn external_preparation_stack_groups_use_one_split_index_and_outside_list_member_edits() {
        use crate::filename_stack::{StackGroup, StackMember, StackView};
        let mut app = crate::app::setup_app_for_test();
        let count = 512;
        let groups: Vec<_> = (0..count)
            .map(|index| StackGroup {
                key: format!("group-{index}"),
                members: vec![
                    StackMember {
                        path: PathBuf::from(format!("{index}-second.jpg")),
                        mtime: 0,
                        size: Some(1),
                        is_video: false,
                    },
                    StackMember {
                        path: PathBuf::from(format!("{index}-first.mp4")),
                        mtime: 0,
                        size: Some(1),
                        is_video: true,
                    },
                ],
            })
            .collect();
        app.items = groups
            .iter()
            .map(|group| crate::grid_item::GridItem::Stack {
                key: group.key.clone(),
                representative: group.members[0].path.clone(),
                count: 2,
            })
            .collect();
        app.visible_indices = (0..count).collect();
        app.checked.extend(0..count);
        app.selected = Some(7);
        app.stack_view = Some(Arc::new(StackView::from_groups(
            PathBuf::from("folder"),
            Vec::new(),
            Vec::new(),
            '_',
            crate::settings::SortOrder::FileName,
            groups,
        )));
        app.settings.external_tools = vec![file_list_test_tool(SelectionPolicy::Batch)];
        app.launch_grid_external_tool_slot(&egui::Context::default(), 1);
        let mut preparing = take_preparing(&mut app);
        let frames = finish_snapshot_for_test(&mut app, &mut preparing);
        assert!(frames.len() > 1);
        assert_eq!(preparing.stack_indices.len(), count);
        assert!(preparing.page_indices.is_empty());
        assert_eq!(preparing.requests.len(), count * 2);
        assert!(preparing.scanned_entries <= count * 4 + 10);
        let Some(crate::materializer::MaterializePageEdits::Single(edits)) =
            &preparing.requests[0].page_edits
        else {
            panic!("missing member edits")
        };
        assert!(edits.load_page_params_from_db);
        assert!(
            matches!(&preparing.requests[0].source, crate::materializer::MaterializeSource::File { path, image_page: true } if path == Path::new("0-second.jpg"))
        );
        assert!(
            matches!(&preparing.requests[1].source, crate::materializer::MaterializeSource::File { path, image_page: false } if path == Path::new("0-first.mp4"))
        );
        assert!(
            evaluate_target_count(SelectionPolicy::Batch, preparing.requests.len(), 5, 1000)
                .is_err()
        );
    }

    #[test]
    fn external_preparation_spread_preserves_screen_and_reading_indices() {
        for policy in [SpreadPolicy::BothPages, SpreadPolicy::Merged] {
            let mut app = crate::app::setup_app_for_test();
            app.items = vec![
                crate::grid_item::GridItem::Image(PathBuf::from("left.jpg")),
                crate::grid_item::GridItem::Image(PathBuf::from("right.jpg")),
            ];
            let mut tool = file_list_test_tool(SelectionPolicy::Each);
            tool.spread = policy;
            tool.payload = PayloadPolicy::TempEdited;
            let descriptor = app
                .external_tool_target_descriptor(ExternalTargetSource::Viewer { current: Some(1) });
            app.begin_external_preparation(
                egui::ViewportId::ROOT,
                descriptor,
                ExternalPreparationPurpose::ForTool(tool),
            );
            let mut preparing = take_preparing(&mut app);
            preparing.snapshots.spread = Some((1, [0, 1], [1, 0]));
            finish_snapshot_for_test(&mut app, &mut preparing);
            assert!(preparing.page_indices.is_empty());
            if policy == SpreadPolicy::BothPages {
                assert_eq!(preparing.requests.len(), 2);
                assert!(
                    matches!(&preparing.requests[0].source, crate::materializer::MaterializeSource::File { path, .. } if path == Path::new("right.jpg"))
                );
                assert!(
                    matches!(&preparing.requests[1].source, crate::materializer::MaterializeSource::File { path, .. } if path == Path::new("left.jpg"))
                );
            } else {
                assert_eq!(preparing.requests.len(), 1);
                let crate::materializer::MaterializeSource::MergedSpread { left, right, .. } =
                    &preparing.requests[0].source
                else {
                    panic!("expected merged spread")
                };
                assert!(
                    matches!(left.as_ref(), crate::materializer::MaterializeSource::File { path, .. } if path == Path::new("left.jpg"))
                );
                assert!(
                    matches!(right.as_ref(), crate::materializer::MaterializeSource::File { path, .. } if path == Path::new("right.jpg"))
                );
            }
        }
    }

    #[test]
    fn external_preparation_routes_keep_container_and_viewer_single_and_confirmation_exact() {
        let mut app = crate::app::setup_app_for_test();
        app.items = (0..300)
            .map(|index| crate::grid_item::GridItem::Video(PathBuf::from(format!("{index}.mp4"))))
            .collect();
        app.visible_indices = (0..300).rev().collect();
        app.checked.extend(0..300);
        app.selected = Some(10);
        let mut tool = file_list_test_tool(SelectionPolicy::Each);
        tool.max_targets = 500;
        tool.confirmation_threshold = 5;
        app.settings.external_tools = vec![tool.clone()];
        for source in [
            ExternalTargetSource::Viewer { current: Some(2) },
            ExternalTargetSource::Playback { current: Some(2) },
            ExternalTargetSource::Container {
                path: Some(PathBuf::from("book.zip")),
            },
        ] {
            let descriptor = app.external_tool_target_descriptor(source);
            app.queue_external_tool_launch_from_context_menu(
                &egui::Context::default(),
                &tool,
                &descriptor,
            );
            let mut preparing = take_preparing(&mut app);
            finish_snapshot_for_test(&mut app, &mut preparing);
            assert_eq!(preparing.requests.len(), 1);
            assert!(preparing.scanned_entries < 10);
        }
        app.launch_grid_external_tool_slot(&egui::Context::default(), 1);
        while matches!(app.external_tool_operation, ExternalOperation::Preparing(_)) {
            app.frame_counter += 1;
            app.advance_external_preparation();
        }
        let ExternalOperation::Confirmation(confirmation) = &app.external_tool_operation else {
            panic!("expected count confirmation before worker")
        };
        assert_eq!(confirmation.operation.target_count(), 300);
        assert!(app.external_tool_retired_materialize.is_empty());
        assert!(app.external_tool_retired_launch.is_empty());
    }

    fn finish_snapshot_for_test(
        app: &mut crate::app::App,
        preparing: &mut ExternalPreparing,
    ) -> Vec<usize> {
        let mut entries = Vec::new();
        loop {
            let before = preparing.scanned_entries;
            let done = app.advance_external_preparation_mounted(preparing).unwrap();
            entries.push(preparing.scanned_entries - before);
            assert!(entries.last().copied().unwrap() <= EXTERNAL_PREPARATION_FRAME_ENTRIES);
            if done {
                return entries;
            }
            assert!(entries.len() < 10000);
        }
    }

    #[test]
    fn external_preparation_grid_slot_begins_modal_before_any_bulk_capture_and_is_linear() {
        let mut app = crate::app::setup_app_for_test();
        let count = 4096;
        app.items = (0..count)
            .map(|index| {
                crate::grid_item::GridItem::Video(PathBuf::from(format!("video-{index}.mp4")))
            })
            .collect();
        app.visible_indices = (0..count).rev().collect();
        app.checked.extend(0..count);
        app.selected = Some(17);
        let mut tool = file_list_test_tool(SelectionPolicy::Batch);
        tool.max_targets = count as u32;
        app.settings.external_tools = vec![tool];
        app.launch_grid_external_tool_slot(&egui::Context::default(), 1);
        assert_eq!(
            app.modal_dialog_block_reason(),
            Some("external_tool_preparing")
        );
        let mut preparing = take_preparing(&mut app);
        assert!(preparing.locators.is_empty());
        assert!(preparing.requests.is_empty());
        assert!(preparing.page_indices.is_empty());
        let frames = finish_snapshot_for_test(&mut app, &mut preparing);
        assert!(frames.len() > 1);
        assert!(preparing.page_indices.is_empty());
        assert!(preparing.scanned_entries <= count * 2 + 8);
        let order: Vec<_> = preparing
            .locators
            .iter()
            .map(|locator| match locator {
                ExternalTargetLocator::Listed { index, .. } => *index,
                _ => panic!("listed index lost"),
            })
            .collect();
        let expected: Vec<_> = (0..count).rev().collect();
        assert_eq!(order, expected);
        assert_eq!(preparing.requests.len(), count);
    }

    #[test]
    fn external_preparation_single_checked_primary_does_not_scan_remaining_grid() {
        let mut app = crate::app::setup_app_for_test();
        app.items = (0..4096)
            .map(|index| {
                crate::grid_item::GridItem::Video(PathBuf::from(format!("video-{index}.mp4")))
            })
            .collect();
        app.visible_indices = (0..4096).rev().collect();
        app.checked.insert(17);
        app.selected = Some(17);
        app.settings.external_tools = vec![file_list_test_tool(SelectionPolicy::Batch)];
        app.launch_grid_external_tool_slot(&egui::Context::default(), 1);
        let mut preparing = take_preparing(&mut app);
        let frames = finish_snapshot_for_test(&mut app, &mut preparing);
        assert!(frames.iter().all(|entries| *entries <= 128));
        assert!(preparing.scanned_entries < 10);
        assert_eq!(preparing.requests.len(), 1);
        assert!(matches!(
            preparing.locators.as_slice(),
            [ExternalTargetLocator::Listed { index: 17, .. }]
        ));
    }

    #[test]
    fn external_preparation_budget_stops_at_entry_and_elapsed_limits() {
        let mut budget = ExternalPreparationBudget::new();
        budget.started = std::time::Instant::now() + std::time::Duration::from_secs(60);
        for _ in 0..128 {
            assert!(budget.next());
        }
        assert!(!budget.next());
        let mut budget = ExternalPreparationBudget::new();
        budget.started -= EXTERNAL_PREPARATION_FRAME_TIME;
        assert!(!budget.next());
        assert_eq!(budget.entries, 0);
    }

    #[test]
    fn external_preparation_listed_image_preserves_index_and_no_lookup_map() {
        let mut app = crate::app::setup_app_for_test();
        app.items = (0..1024)
            .map(|index| {
                crate::grid_item::GridItem::Image(PathBuf::from(format!("image-{index}.jpg")))
            })
            .collect();
        app.visible_indices = (0..1024).collect();
        app.selected = Some(777);
        let mut params = crate::adjustment::AdjustParams::default();
        params.brightness = 37.0;
        app.adjustment_page_params.insert(777, params.clone());
        app.settings.external_tools = vec![file_list_test_tool(SelectionPolicy::Single)];
        app.launch_grid_external_tool_slot(&egui::Context::default(), 1);
        let mut preparing = take_preparing(&mut app);
        finish_snapshot_for_test(&mut app, &mut preparing);
        assert!(preparing.page_indices.is_empty());
        assert!(preparing.scanned_entries < 10);
        let Some(crate::materializer::MaterializePageEdits::Single(edits)) =
            &preparing.requests[0].page_edits
        else {
            panic!("missing edits")
        };
        assert_eq!(edits.params, params);
        assert!(!edits.load_page_params_from_db);
    }

    #[test]
    fn external_preparation_legacy_single_page_lookup_is_split_once_and_real_file_never_scans() {
        let mut app = crate::app::setup_app_for_test();
        app.items = (0..4096)
            .map(|index| {
                crate::grid_item::GridItem::Image(PathBuf::from(format!("image-{index}.jpg")))
            })
            .collect();
        let target = LaunchTarget::from_grid_item(app.items.get(3333));
        let tool = file_list_test_tool(SelectionPolicy::Single);
        app.queue_external_tool_launch(&egui::Context::default(), &tool, &target);
        let mut preparing = take_preparing(&mut app);
        let frames = finish_snapshot_for_test(&mut app, &mut preparing);
        assert!(frames.len() > 1);
        assert_eq!(preparing.page_indices.len(), 4096);
        assert!(preparing.scanned_entries <= 4104);
        assert!(matches!(
            preparing.locators[0],
            ExternalTargetLocator::Listed { index: 3333, .. }
        ));
        app.queue_external_tool_launch(
            &egui::Context::default(),
            &tool,
            &LaunchTarget::RealFile(PathBuf::from("video.mp4")),
        );
        let mut preparing = take_preparing(&mut app);
        finish_snapshot_for_test(&mut app, &mut preparing);
        assert!(preparing.page_indices.is_empty());
        assert!(preparing.scanned_entries < 10);
    }

    #[test]
    fn external_preparation_owner_generation_and_typed_item_identity_cancel_without_retry() {
        let mut app = crate::app::setup_app_for_test();
        app.items = vec![crate::grid_item::GridItem::Image(PathBuf::from("old.jpg"))];
        app.visible_indices = vec![0];
        app.selected = Some(0);
        app.settings.external_tools = vec![file_list_test_tool(SelectionPolicy::Single)];
        app.launch_grid_external_tool_slot(&egui::Context::default(), 1);
        let mut preparing = take_preparing(&mut app);
        app.items_generation += 1;
        assert!(
            app.advance_external_preparation_mounted(&mut preparing)
                .is_err()
        );
        app.launch_grid_external_tool_slot(&egui::Context::default(), 1);
        let mut preparing = take_preparing(&mut app);
        preparing.locators.push(ExternalTargetLocator::Listed {
            owner: preparing.owner,
            items_generation: preparing.items_generation,
            index: 0,
            target: LaunchTarget::ImagePage(PathBuf::from("old.jpg")),
        });
        preparing.stage = ExternalPreparationStage::Snapshot {
            cursor: 0,
            member_cursor: 0,
        };
        app.items[0] = crate::grid_item::GridItem::Image(PathBuf::from("different.jpg"));
        assert!(
            app.advance_external_preparation_mounted(&mut preparing)
                .is_err()
        );
        assert!(preparing.requests.is_empty());
    }

    #[test]
    fn external_preparation_streamed_refusal_summary_preserves_original_priority_and_messages() {
        use crate::grid_item::FileOperationRefusal;
        for targets in [
            vec![
                LaunchTarget::Unsupported,
                LaunchTarget::Virtual(FileOperationRefusal::ArchiveDirectory),
            ],
            vec![
                LaunchTarget::Virtual(FileOperationRefusal::ArchiveDirectory),
                LaunchTarget::Unsupported,
            ],
            vec![
                LaunchTarget::Unsupported,
                LaunchTarget::Virtual(FileOperationRefusal::ArchiveDirectory),
                LaunchTarget::None,
            ],
            vec![
                LaunchTarget::None,
                LaunchTarget::Virtual(FileOperationRefusal::ArchiveDirectory),
                LaunchTarget::Unsupported,
            ],
            vec![
                LaunchTarget::Virtual(FileOperationRefusal::ArchiveDirectory),
                LaunchTarget::Virtual(FileOperationRefusal::VirtualPage),
            ],
        ] {
            let expected = validate_materializable_targets(&targets).unwrap_err();
            let mut app = crate::app::setup_app_for_test();
            app.begin_external_legacy_preparation(
                egui::ViewportId::ROOT,
                targets,
                ExternalPreparationPurpose::ForTool(file_list_test_tool(SelectionPolicy::Each)),
            );
            let mut preparing = take_preparing(&mut app);
            assert_eq!(
                app.advance_external_preparation_mounted(&mut preparing)
                    .unwrap_err(),
                expected
            );
            assert!(preparing.requests.is_empty());
        }
    }

    #[cfg(windows)]
    #[test]
    fn external_preparation_unrelated_context_mutation_keeps_owner_snapshot_and_missing_owner_cancels()
     {
        let mut app = crate::app::setup_app_for_test();
        app.settings.external_tools = vec![file_list_test_tool(SelectionPolicy::Each)];
        app.items = vec![crate::grid_item::GridItem::Video(PathBuf::from(
            "owner.mp4",
        ))];
        app.visible_indices = vec![0];
        app.selected = Some(0);
        app.request_grid_external_tool_picker();
        let owner = app.edit_request_owner_context();
        let owner_generation = app.items_generation;
        let sibling = app.build_window_context_for_test(9991, |app| {
            app.items = vec![crate::grid_item::GridItem::Video(PathBuf::from(
                "sibling.mp4",
            ))];
            app.items_generation += 50;
        });
        assert_eq!(app.edit_request_owner_context(), owner);
        assert_eq!(app.items_generation, owner_generation);
        app.with_window_context_for_test(sibling, |app| {
            app.items_generation += 1;
            let sibling_generation = app.items_generation;
            assert_eq!(app.edit_request_owner_context(), sibling);
            assert_ne!(sibling_generation, owner_generation);
            app.with_owner_viewer_context(owner, |app| {
                assert_eq!(app.edit_request_owner_context(), owner);
                assert_eq!(app.items_generation, owner_generation);
            })
            .expect("unchanged owner remains mountable");
            let mut frames = 0;
            while matches!(app.external_tool_operation, ExternalOperation::Preparing(_)) {
                app.frame_counter += 1;
                app.advance_external_preparation();
                assert_eq!(app.edit_request_owner_context(), sibling);
                assert_eq!(app.items_generation, sibling_generation);
                app.with_owner_viewer_context(owner, |app| {
                    assert_eq!(app.items_generation, owner_generation);
                })
                .expect("unchanged owner remains mountable");
                frames += 1;
                assert!(frames < 10000);
            }
        });
        assert_eq!(app.edit_request_owner_context(), owner);
        assert_eq!(app.items_generation, owner_generation);
        let ExternalOperation::Picker(request) = &app.external_tool_operation else {
            panic!("owner request cancelled by sibling mutation")
        };
        assert_eq!(request.owner, owner);
        assert_eq!(
            request.locators[0].target(),
            &LaunchTarget::RealFile(PathBuf::from("owner.mp4"))
        );
        app.request_grid_external_tool_picker();
        let ExternalOperation::Preparing(preparing) = &mut app.external_tool_operation else {
            unreachable!()
        };
        preparing.owner = crate::app::ViewerContextId::for_test(u64::MAX);
        app.frame_counter += 1;
        app.advance_external_preparation();
        assert!(matches!(
            app.external_tool_operation,
            ExternalOperation::Idle
        ));
    }

    fn launch_confirmation_ready(
        operation: ExternalLaunchOperation,
    ) -> Result<ExternalQueuedOperation, ExternalLaunchConfirmation> {
        launch_confirmation(ExternalQueuedOperation::Ready(operation))
    }

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
    fn editing_defaults_pass_the_real_file_and_one_page() {
        let tool = ExternalTool::defaults_for_editing();
        assert_eq!(tool.payload, PayloadPolicy::OriginalFile);
        assert_eq!(tool.spread, SpreadPolicy::MainPageOnly);
        assert_eq!(tool.arguments, "{files}");
        assert_eq!(tool.pdf_render_long_edge, DEFAULT_PDF_RENDER_LONG_EDGE);
    }

    #[test]
    fn viewing_defaults_use_displayed_each_file_policy_and_count_limits() {
        let tool = ExternalTool::defaults_for_viewing();
        assert_eq!(tool.payload, PayloadPolicy::TempEdited);
        assert_eq!(tool.video, VideoPolicy::File);
        assert_eq!(tool.spread, SpreadPolicy::Merged);
        assert_eq!(tool.selection, SelectionPolicy::Each);
        assert_eq!(tool.confirmation_threshold, DEFAULT_CONFIRMATION_THRESHOLD);
        assert_eq!(tool.max_targets, DEFAULT_MAX_TARGETS);
        assert_eq!(tool.launch, ExternalToolLaunch::OsDefault);
        assert!(!tool.show_console);
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

    fn menu_tool(id: u32, name: &str, payload: PayloadPolicy) -> ExternalTool {
        ExternalTool {
            id: ExternalToolId(id),
            name: name.to_string(),
            launch: ExternalToolLaunch::Executable(PathBuf::from(format!(r"C:\Tools\{name}.exe"))),
            payload,
            ..ExternalTool::defaults_for_viewing()
        }
    }

    /// 「元のデータをそのまま」と「見開きを 1 枚に合成」は同時に成り立たない。
    ///
    /// 合成した 1 枚に対応する元のデータはどこにも無い。設定 UI は合成を出さないが、
    /// 保存済みの設定に残っていても実行時に合成しないことを、同じ 1 つの規則で保証する
    /// (Codex Sol 指摘 #4)。
    #[test]
    fn passing_the_original_bytes_rules_out_a_merged_spread() {
        for payload in [PayloadPolicy::TempOriginal, PayloadPolicy::OriginalFile] {
            let mut tool = menu_tool(1, "merge", payload);
            tool.spread = SpreadPolicy::Merged;
            assert_eq!(
                tool.effective_spread(),
                SpreadPolicy::MainPageOnly,
                "{payload:?} で合成した PNG を作ってはいけない"
            );
        }

        let mut tool = menu_tool(2, "merge", PayloadPolicy::TempEdited);
        tool.spread = SpreadPolicy::Merged;
        assert_eq!(tool.effective_spread(), SpreadPolicy::Merged);
    }

    #[test]
    fn real_file_menu_keeps_every_registered_tool_in_order_without_a_cap() {
        let tools: Vec<_> = (0..12)
            .map(|index| {
                menu_tool(
                    index + 1,
                    &format!("tool-{index}"),
                    PayloadPolicy::TempEdited,
                )
            })
            .collect();

        let items = external_tool_menu_items(
            &tools,
            ExternalToolMenuTarget::from_launch_targets(&[LaunchTarget::ImagePage(PathBuf::from(
                "page.jpg",
            ))]),
        );

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
    fn virtual_page_menu_greys_original_file_tools_with_a_reason_and_keeps_temp_tools() {
        let displayed = menu_tool(1, "displayed", PayloadPolicy::TempEdited);
        let pre_edit = menu_tool(2, "pre-edit", PayloadPolicy::TempOriginal);
        let real_file = menu_tool(3, "real-file", PayloadPolicy::OriginalFile);
        let tools = vec![displayed, pre_edit, real_file];

        let target = ExternalToolMenuTarget::from_launch_targets(&[LaunchTarget::ZipPage {
            zip_path: PathBuf::from("book.zip"),
            entry_name: "page.jpg".to_string(),
        }]);
        let items = external_tool_menu_items(&tools, target);

        assert_eq!(
            items,
            vec![
                ExternalToolMenuItem {
                    tool_id: ExternalToolId(1),
                    label: "displayedで開く".to_string(),
                    enabled: true,
                    disabled_reason: None,
                },
                ExternalToolMenuItem {
                    tool_id: ExternalToolId(2),
                    label: "pre-editで開く".to_string(),
                    enabled: true,
                    disabled_reason: None,
                },
                // 隠さずグレーにする。利用者は「このツールで編集できる」と思っている。
                ExternalToolMenuItem {
                    tool_id: ExternalToolId(3),
                    label: "real-fileで開く".to_string(),
                    enabled: false,
                    disabled_reason: Some(VIRTUAL_ORIGINAL_FILE_DISABLED_REASON),
                },
            ]
        );
        assert!(
            external_tool_menu_items(
                &tools,
                ExternalToolMenuTarget::from_launch_targets(&[LaunchTarget::Unsupported])
            )
            .is_empty()
        );
    }

    #[test]
    fn virtual_page_picker_filters_and_disables_before_showing() {
        let displayed = menu_tool(1, "displayed", PayloadPolicy::TempEdited);
        let pre_edit = menu_tool(2, "pre-edit", PayloadPolicy::TempOriginal);
        let real_file = menu_tool(3, "real-file", PayloadPolicy::OriginalFile);

        let items = external_tool_picker_items(
            &[displayed, pre_edit, real_file],
            &[LaunchTarget::PdfPage {
                pdf_path: PathBuf::from("book.pdf"),
                page_num: 2,
            }],
        );

        assert_eq!(
            items,
            vec![
                ExternalToolPickerItem {
                    tool_id: ExternalToolId(1),
                    slot: 1,
                    label: "displayed".to_string(),
                    enabled: true,
                    disabled_reason: None,
                },
                ExternalToolPickerItem {
                    tool_id: ExternalToolId(2),
                    slot: 2,
                    label: "pre-edit".to_string(),
                    enabled: true,
                    disabled_reason: None,
                },
                ExternalToolPickerItem {
                    tool_id: ExternalToolId(3),
                    slot: 3,
                    label: "real-file".to_string(),
                    enabled: false,
                    disabled_reason: Some(VIRTUAL_ORIGINAL_FILE_DISABLED_REASON),
                },
            ]
        );
    }

    #[test]
    fn context_menu_target_classifies_real_virtual_and_unsupported_items() {
        assert_eq!(
            ExternalToolMenuTarget::from_launch_targets(&[LaunchTarget::RealFile(PathBuf::from(
                r"C:\Images\page.jpg",
            ))]),
            ExternalToolMenuTarget {
                has_target: true,
                has_virtual_page: false,
                has_unsupported: false,
            }
        );
        assert_eq!(
            ExternalToolMenuTarget::from_launch_targets(&[LaunchTarget::PdfPage {
                pdf_path: PathBuf::from("book.pdf"),
                page_num: 0,
            }]),
            ExternalToolMenuTarget {
                has_target: true,
                has_virtual_page: true,
                has_unsupported: false,
            }
        );
        assert_eq!(
            ExternalToolMenuTarget::from_launch_targets(&[LaunchTarget::Unsupported]),
            ExternalToolMenuTarget {
                has_target: false,
                has_virtual_page: false,
                has_unsupported: true,
            }
        );
        assert_eq!(
            ExternalToolMenuTarget::from_launch_targets(&[
                LaunchTarget::ZipPage {
                    zip_path: PathBuf::from("book.zip"),
                    entry_name: "page.jpg".to_string(),
                },
                LaunchTarget::RealFile(PathBuf::from(r"C:\Images\page.jpg")),
            ]),
            ExternalToolMenuTarget {
                has_target: true,
                has_virtual_page: true,
                has_unsupported: false,
            },
            "混在でも仮想ページを見失わず、OriginalFile を正しく判定する"
        );
    }

    /// 外部ツールの modal は**単一の描き手**からしか描かない。
    ///
    /// 個々の `show_*` を別の場所から呼ぶと、その場所は frame 所有権の判定を通らないので
    /// 二重描画するか、逆にフルスクリーンで描かれず「ダイアログが無いのに入力だけ止まる」
    /// になる。実際に 3 つのうち 1 つだけ複製して 2 つを落としていた (2026-09-02)。
    /// 目視のレビューでは 2 度取りこぼしたので、機械で見る。
    #[test]
    fn the_external_tool_modals_are_drawn_from_one_place_only() {
        let source = include_str!("external_tool.rs");
        let app = include_str!("app.rs");
        let fullscreen = include_str!("ui_fullscreen.rs");

        for name in [
            "show_external_tool_preparing",
            "show_external_tool_picker",
            "show_external_tool_launch_confirmation",
            "show_external_tool_materialize_progress",
        ] {
            let call = format!("self.{name}(ctx)");
            let inside_owner = source.matches(&call).count();
            assert_eq!(
                inside_owner, 1,
                "{name} は show_external_tool_modals からの 1 回だけであること"
            );
            assert!(
                !app.contains(&call) && !fullscreen.contains(&call),
                "{name} が単一の描き手の外から呼ばれている"
            );
        }

        let owner = "self.show_external_tool_modals(ctx)";
        assert_eq!(
            app.matches(owner).count() + fullscreen.matches(owner).count(),
            3,
            concat!(
                "描き手は update_frame の tail / fullscreen body / ",
                "`eframe::App::update` の取りこぼし拾いの 3 か所"
            )
        );
    }

    /// modal は「利用者がその操作をした窓」が描く。先着では決めない。
    ///
    /// viewer の viewport は main の tail より前に描くので、先着にすると背面の F12 窓が
    /// main 由来の modal を攫い、前面の main は「ダイアログが無いのに入力だけ止まる」に
    /// なる (Codex Sol 指摘 #3)。
    #[test]
    fn the_window_the_user_acted_in_owns_the_modal() {
        let root = egui::ViewportId::ROOT;
        let detached = egui::ViewportId::from_hash_of("detached-viewer");
        let fullscreen = egui::ViewportId::from_hash_of("fullscreen");

        // main 由来の要求: 先に描く背面 F12 窓は描かず、main が描く。
        assert!(!external_tool_modal_viewport_draws(detached, root, false));
        assert!(external_tool_modal_viewport_draws(root, root, false));

        // F12 窓由来の要求: その窓が描き、main は肩代わりしない。
        assert!(external_tool_modal_viewport_draws(
            detached, detached, false
        ));
        assert!(!external_tool_modal_viewport_draws(root, detached, true));

        // 所有 viewport が消えた frame (フルスクリーンを閉じる起動 / native 動画の早期
        // return) は main が肩代わりする。しないと ACK が来ず、起動が永久に始まらない。
        assert!(external_tool_modal_viewport_draws(root, fullscreen, false));
        assert!(!external_tool_modal_viewport_draws(
            detached, fullscreen, false
        ));
    }

    /// `update_frame` が早期 return した frame でも、modal の描画と spawn 境界の ACK は
    /// 落ちない。
    ///
    /// native 動画 backdrop / 静止画 viewport 抑止 / embedded 保留の 3 経路は、tail まで
    /// 到達しない。フルスクリーンで動画へ移動しただけで準備中の要求が ACK されなくなり、
    /// 動画を閉じるまで外部ツールが起動しない状態になっていた (Codex Sol 指摘 #2)。
    #[test]
    fn the_launch_boundary_is_acked_even_when_the_frame_body_returns_early() {
        let app = include_str!("app.rs");
        let wrapper = app
            .split_once("self.update_frame(ctx, frame);")
            .expect("`eframe::App::update` は本体を update_frame へ委譲すること")
            .1;
        let tail = wrapper
            .split_once("fn on_exit(")
            .expect("update の terminal tail は on_exit より前にあること")
            .0;
        assert!(
            tail.contains("self.show_external_tool_modals(ctx);"),
            "早期 return を飛び越える tail で modal を描いていない"
        );
        assert!(
            tail.contains("self.authorize_external_tool_launch_boundaries_after_ui();"),
            "早期 return を飛び越える tail で spawn 境界を ACK していない"
        );
    }

    /// supersede 済みの要求は進捗 modal に出ない。**入力ブロックも同じ述語から導く**ので、
    /// 「ダイアログが無いのにクリックが効かない」状態を作らない。
    ///
    /// この 2 つを別々の条件で書いていたのが実機の固着の一部だった (2026-09-02)。
    ///
    /// 前半 (準備中は入力が止まる) は、**世代をツールごとに分けない**根拠でもある。
    /// 準備中に利用者が別のツールを起動する経路が無いので、準備中の要求は常に高々
    /// 1 つ。ツール別の世代を持っても表せる状態は増えない (Codex Sol 指摘 #9)。
    #[test]
    fn a_superseded_materialize_request_blocks_no_input_because_it_draws_no_dialog() {
        let mut app = crate::app::setup_app_for_test();
        let generation = app.external_tool_materializer.begin_generation();
        app.external_tool_operation =
            ExternalOperation::Materializing(materialize_pending_for_test(generation));

        assert!(app.external_tool_materialize_progress_visible());
        assert_eq!(
            app.modal_dialog_block_reason(),
            Some("external_tool_materialize_progress")
        );

        // 次の要求が世代を進めると、この要求は描かれなくなる。入力も同時に解放される。
        app.external_tool_materializer.begin_generation();

        assert!(!app.external_tool_materialize_progress_visible());
        assert_eq!(app.modal_dialog_block_reason(), None);
        assert!(
            matches!(
                app.external_tool_operation,
                ExternalOperation::Materializing(_)
            ),
            "drain されるまで pending は残る。残っていても入力は止めない、が要件"
        );
    }

    /// この test は channel を触らない (見えるか / 入力を止めるか だけを見る) ので、
    /// 送信側は落としてよい。
    fn materialize_pending_for_test(generation: u64) -> ExternalMaterializePending {
        let (_, completion_rx) = mpsc::channel();
        let (_, boundary_rx) = mpsc::channel();
        let (decision_tx, _) = mpsc::channel();
        ExternalMaterializePending {
            cancel: Arc::new(AtomicBool::new(false)),
            rx: completion_rx,
            launch_boundary_rx: boundary_rx,
            launch_decision_tx: Some(decision_tx),
            progress: Arc::new(MaterializeProgress::new(1)),
            generation,
            worker: None,
            user_cancelled: false,
            launch_ui_checkpoint_passed: false,
        }
    }

    #[test]
    fn user_cancel_before_launch_boundary_authorization_wins() {
        let (_completion_tx, completion_rx) = mpsc::channel();
        let (boundary_tx, boundary_rx) = mpsc::channel();
        let (decision_tx, decision_rx) = mpsc::channel();
        let mut pending = ExternalMaterializePending {
            cancel: Arc::new(AtomicBool::new(false)),
            rx: completion_rx,
            launch_boundary_rx: boundary_rx,
            launch_decision_tx: Some(decision_tx),
            progress: Arc::new(MaterializeProgress::new(1)),
            generation: 1,
            worker: None,
            user_cancelled: false,
            launch_ui_checkpoint_passed: false,
        };
        boundary_tx.send(()).unwrap();

        assert!(!pending.take_launch_ui_checkpoint());
        pending.mark_launch_ui_checkpoint();
        assert!(pending.take_launch_ui_checkpoint());
        assert!(!pending.take_launch_ui_checkpoint());

        // App::update は progress UI の入力を処理してから authorization を試みる。
        pending.cancel(true);
        pending.resolve_launch_boundary(true);

        assert_eq!(
            decision_rx.recv().unwrap(),
            MaterializeLaunchDecision::Cancel
        );
        assert!(!pending.can_cancel_before_launch());
        assert!(pending.user_cancelled);
    }

    #[test]
    fn stack_expansion_preserves_outer_and_member_order_and_counts_expanded_targets() {
        use crate::filename_stack::{StackGroup, StackMember, StackView};

        let member = |path: &str, is_video| StackMember {
            path: PathBuf::from(path),
            mtime: 0,
            size: Some(1),
            is_video,
        };
        let view = StackView::from_groups(
            PathBuf::from("folder"),
            Vec::new(),
            Vec::new(),
            '_',
            crate::settings::SortOrder::FileName,
            vec![StackGroup {
                key: "stack".to_string(),
                members: vec![
                    member("page-2.jpg", false),
                    member("page-1.jpg", false),
                    member("clip.mp4", true),
                ],
            }],
        );
        let expanded = expand_stack_targets(
            &[
                LaunchTarget::ImagePage(PathBuf::from("before.jpg")),
                LaunchTarget::Stack("stack".to_string()),
                LaunchTarget::PdfPage {
                    pdf_path: PathBuf::from("after.pdf"),
                    page_num: 2,
                },
            ],
            Some(&view),
        )
        .unwrap();
        assert_eq!(
            expanded,
            vec![
                LaunchTarget::ImagePage(PathBuf::from("before.jpg")),
                LaunchTarget::ImagePage(PathBuf::from("page-2.jpg")),
                LaunchTarget::ImagePage(PathBuf::from("page-1.jpg")),
                LaunchTarget::RealFile(PathBuf::from("clip.mp4")),
                LaunchTarget::PdfPage {
                    pdf_path: PathBuf::from("after.pdf"),
                    page_num: 2,
                },
            ]
        );
        assert!(evaluate_target_count(SelectionPolicy::Single, expanded.len(), 5, 10).is_err());
        assert!(evaluate_target_count(SelectionPolicy::Each, expanded.len(), 5, 4).is_err());
        assert_eq!(
            evaluate_target_count(SelectionPolicy::Batch, expanded.len(), 4, 10).unwrap(),
            TargetCountDecision::Confirm { target_count: 5 }
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
                LaunchTarget::RealFile(path) | LaunchTarget::ImagePage(path) => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn target_resolver_prioritizes_checked_and_keeps_display_order() {
        let items = resolver_items();
        let checked = HashSet::from([0, 1, 2]);
        for source in [
            ExternalTargetSource::GridContext { clicked: Some(1) },
            ExternalTargetSource::GridKey { selected: Some(1) },
        ] {
            let targets = resolve_external_targets(&items, &[2, 0, 1], &checked, source);
            assert_eq!(
                target_paths(&targets),
                ["two.jpg", "zero.jpg", "one.jpg"]
                    .into_iter()
                    .map(PathBuf::from)
                    .collect::<Vec<_>>()
            );
        }

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
    fn external_tool_slot_reports_an_unregistered_slot() {
        let tools: Vec<_> = (1..=3)
            .map(|slot| {
                let mut tool = ExternalTool::defaults_for_viewing();
                tool.name = format!("Tool {slot}");
                tool
            })
            .collect();
        assert_eq!(
            resolve_external_tool_slot(&tools, 1).unwrap().name,
            "Tool 1"
        );
        assert_eq!(
            resolve_external_tool_slot(&tools, 3).unwrap().name,
            "Tool 3"
        );
        assert_eq!(
            resolve_external_tool_slot(&tools, 5).unwrap_err(),
            "外部ツールスロット 5 にはツールが登録されていません"
        );
        assert!(resolve_external_tool_slot(&tools, 0).is_err());
        let mut eleven_tools = tools.clone();
        eleven_tools.resize_with(11, ExternalTool::defaults_for_viewing);
        assert!(resolve_external_tool_slot(&eleven_tools, 11).is_err());
    }

    #[test]
    fn missing_external_tool_key_slot_is_a_reasoned_noop() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.external_tools = (1..=3)
            .map(|slot| {
                let mut tool = ExternalTool::defaults_for_viewing();
                tool.name = format!("Tool {slot}");
                tool
            })
            .collect();
        app.items = vec![crate::grid_item::GridItem::Image(PathBuf::from(
            "selected.jpg",
        ))];
        app.visible_indices = vec![0];
        app.selected = Some(0);

        app.launch_grid_external_tool_slot(&egui::Context::default(), 5);

        assert!(matches!(
            app.external_tool_operation,
            ExternalOperation::Idle
        ));
        assert!(app.fs_feedback_toast.as_ref().is_some_and(|(text, _, _)| {
            text == "外部ツールスロット 5 にはツールが登録されていません"
        }));
    }

    #[test]
    fn picker_accepts_materializable_pages_rejects_virtual_directories_and_snapshots_targets() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.external_tools = vec![ExternalTool::defaults_for_viewing()];
        app.begin_external_legacy_preparation(
            egui::ViewportId::ROOT,
            vec![LaunchTarget::ZipPage {
                zip_path: PathBuf::from("book.zip"),
                entry_name: "original.jpg".to_string(),
            }],
            ExternalPreparationPurpose::ForPicker(ExternalToolPickerTargetKind::GridItems),
        );
        finish_picker_preparation(&mut app);
        assert!(matches!(
            app.external_tool_operation,
            ExternalOperation::Picker(_)
        ));

        app.external_tool_operation = ExternalOperation::Idle;
        app.begin_external_legacy_preparation(
            egui::ViewportId::ROOT,
            vec![LaunchTarget::Virtual(
                crate::grid_item::FileOperationRefusal::ArchiveDirectory,
            )],
            ExternalPreparationPurpose::ForPicker(ExternalToolPickerTargetKind::GridItems),
        );
        finish_picker_preparation(&mut app);
        assert!(matches!(
            app.external_tool_operation,
            ExternalOperation::Idle
        ));
        assert!(
            app.fs_feedback_toast.as_ref().is_some_and(|(text, _, _)| {
                text.contains("圧縮ファイル内のフォルダ")
            })
        );

        app.begin_external_legacy_preparation(
            egui::ViewportId::ROOT,
            vec![LaunchTarget::RealFile(PathBuf::from("original.jpg"))],
            ExternalPreparationPurpose::ForPicker(ExternalToolPickerTargetKind::GridItems),
        );
        finish_picker_preparation(&mut app);
        app.items = vec![crate::grid_item::GridItem::Image(PathBuf::from(
            "changed.jpg",
        ))];
        assert_eq!(
            match &app.external_tool_operation {
                ExternalOperation::Picker(request) => Some(
                    request
                        .locators
                        .iter()
                        .map(|locator| locator.target().clone())
                        .collect::<Vec<_>>()
                ),
                _ => None,
            },
            Some(vec![LaunchTarget::RealFile(PathBuf::from("original.jpg"))])
        );
        assert_eq!(
            app.modal_dialog_block_reason(),
            Some("external_tool_picker")
        );
    }

    #[test]
    fn external_container_target_is_one_effective_folder_or_book() {
        for path in ["pictures", "book.zip", "book.pdf", "book.7z"] {
            assert_eq!(
                resolve_external_container_targets(Some(Path::new(path)), false),
                vec![LaunchTarget::RealFile(PathBuf::from(path))]
            );
        }
        assert!(resolve_external_container_targets(None, false).is_empty());
        assert!(
            resolve_external_container_targets(Some(Path::new("stale-origin")), true).is_empty()
        );
    }

    #[test]
    fn a_spread_is_handed_over_in_the_policy_specific_order() {
        let ordinary_rtl = [
            crate::ui_fullscreen::SpreadPageOccurrence::navigation(4, 5),
            crate::ui_fullscreen::SpreadPageOccurrence::navigation(5, 5),
        ];
        assert_eq!(
            spread_both_pages_order(&ordinary_rtl),
            Some([4, 5]),
            "ordinary RTL BothPages keeps the established reading order"
        );
        let final_cover = [
            crate::ui_fullscreen::SpreadPageOccurrence::navigation(9, 9),
            crate::ui_fullscreen::SpreadPageOccurrence::final_cover_supplement(0, 9),
        ];
        assert_eq!(spread_both_pages_order(&final_cover), Some([9, 0]));
        assert_eq!(
            merged_spread_screen_order(0, 9),
            [0, 9],
            "RTL Merged keeps the physical left-to-right composition"
        );
    }

    #[test]
    fn split_argument_template_handles_whitespace_quotes_and_escapes() {
        assert_eq!(
            split_argument_template(r#"  --mode edit   "two words" tail  "#),
            ["--mode", "edit", "two words", "tail", "{files}"]
        );
        assert_eq!(
            split_argument_template(r#""a""b" "c\\\"d" "#),
            ["a\"b", "c\\\"d", "{files}"]
        );
        assert_eq!(split_argument_template(r#""""#), ["", "{files}"]);
        assert_eq!(
            split_argument_template(r#""unterminated value"#),
            ["unterminated value", "{files}"]
        );
    }

    /// `{files}` が既にあれば足さない。無ければ足す。
    ///
    /// 知らない記法は「使える値」ではないので、あっても `{files}` を足す。
    /// これが無いと、書き間違えたテンプレートがファイルを渡さないまま起動する。
    #[test]
    fn the_file_is_appended_unless_the_template_already_asks_for_it() {
        // `{file}` は読み込み時点で `{files}` へ揃える (綴りは 1 つ)。
        assert_eq!(
            split_argument_template("--input={file}"),
            ["--input={files}"]
        );
        assert_eq!(
            split_argument_template("--input={files}"),
            ["--input={files}"]
        );
        assert_eq!(
            split_argument_template("--out {stem}.png"),
            ["--out", "{stem}.png", "{files}"]
        );
    }
    #[test]
    fn expand_preserves_unknown_braces_and_adds_file_when_no_keyword_exists() {
        let file = Path::new(r"C:\space dir\image.png");
        let tokens = split_argument_template("--pattern={unknown}");
        assert_eq!(
            expand_arguments(&tokens, file),
            [
                OsString::from("--pattern={unknown}"),
                OsString::from(r"C:\space dir\image.png")
            ]
        );
    }

    #[test]
    fn replacement_with_spaces_remains_one_argument() {
        let file = Path::new(r"C:\space dir\image.png");
        let tokens = split_argument_template("--input={files}");
        assert_eq!(
            expand_arguments(&tokens, file),
            [OsString::from(r"--input=C:\space dir\image.png")]
        );
    }

    #[test]
    fn files_placeholder_expands_one_token_to_distinct_os_arguments() {
        let files = [
            PathBuf::from(r"C:\space dir\one.png"),
            PathBuf::from(r"D:\other dir\two.png"),
        ];
        assert_eq!(
            expand_arguments_for_files(&["--input={files}".to_string()], &files),
            [
                OsString::from(r"--input=C:\space dir\one.png"),
                OsString::from(r"--input=D:\other dir\two.png"),
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
        tool.show_console = true;
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
            assert!(!request.show_console);
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
        assert!(request.show_console);
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

        tool.selection = SelectionPolicy::Single;
        let single = build_launch_operation(&tool, &targets[..1]).unwrap();
        assert_eq!(single.requests.len(), 1);
        assert_eq!(
            single.requests[0].files,
            [PathBuf::from(r"C:\Images\0.png")]
        );
        let single_error = build_launch_operation(&tool, &targets).unwrap_err();
        assert!(single_error.contains("3 件"));
        assert!(single_error.contains("起動できません"));

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

        // 綴りは 1 つなので、`{file}` と書いても「まとめて渡す」で動く。
        tool.arguments = "--input {file}".to_string();
        let spelled_singular = build_launch_operation(&tool, &targets).unwrap();
        assert_eq!(
            spelled_singular.requests[0].arguments,
            [
                OsString::from("--input"),
                OsString::from(r"C:\Images\0.png"),
                OsString::from(r"C:\Images\1.png"),
            ]
        );
    }

    /// 渡すファイルの綴りは `{files}` 1 つ。**複数選択を切り替えても引数は書き直さない。**
    ///
    /// 以前は「1 件ずつ」= `{file}` /「まとめて渡す」= `{files}` と綴りが分かれており、
    /// 切り替えるたびに引数テンプレートを直す必要があった (2026-09-02 利用者判断で統合)。
    #[test]
    fn the_same_template_serves_one_at_a_time_and_all_at_once() {
        let targets = [
            LaunchTarget::RealFile(PathBuf::from(r"C:\Images\0.png")),
            LaunchTarget::RealFile(PathBuf::from(r"C:\Images\1.png")),
        ];
        let mut tool = ExternalTool::defaults_for_viewing();
        tool.launch = ExternalToolLaunch::Executable(PathBuf::from(r"C:\Tools\viewer.exe"));
        tool.arguments = "--input {files}".to_string();

        tool.selection = SelectionPolicy::Each;
        let each = build_launch_operation(&tool, &targets).unwrap();
        assert_eq!(each.requests.len(), 2);
        assert_eq!(
            each.requests[0].arguments,
            [
                OsString::from("--input"),
                OsString::from(r"C:\Images\0.png")
            ]
        );
        assert_eq!(
            each.requests[1].arguments,
            [
                OsString::from("--input"),
                OsString::from(r"C:\Images\1.png")
            ]
        );

        tool.selection = SelectionPolicy::Batch;
        let batch = build_launch_operation(&tool, &targets).unwrap();
        assert_eq!(batch.requests.len(), 1);
        assert_eq!(
            batch.requests[0].arguments,
            [
                OsString::from("--input"),
                OsString::from(r"C:\Images\0.png"),
                OsString::from(r"C:\Images\1.png"),
            ]
        );
    }

    #[test]
    fn virtual_targets_reject_the_whole_set_before_single_policy() {
        let mut tool = ExternalTool::defaults_for_viewing();
        tool.selection = SelectionPolicy::Single;
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
    fn target_count_decision_applies_single_then_limit_then_confirmation() {
        assert_eq!(
            evaluate_target_count(SelectionPolicy::Single, 1, 0, 0).unwrap(),
            TargetCountDecision::Proceed
        );
        assert!(evaluate_target_count(SelectionPolicy::Single, 2, 100, 100).is_err());

        assert_eq!(
            evaluate_target_count(SelectionPolicy::Each, 5, 5, 10).unwrap(),
            TargetCountDecision::Proceed
        );
        assert_eq!(
            evaluate_target_count(SelectionPolicy::Each, 6, 5, 10).unwrap(),
            TargetCountDecision::Confirm { target_count: 6 }
        );
        assert!(evaluate_target_count(SelectionPolicy::Each, 11, 5, 10).is_err());

        assert_eq!(
            evaluate_target_count(SelectionPolicy::Batch, 6, 5, 10).unwrap(),
            TargetCountDecision::Confirm { target_count: 6 }
        );
        assert!(evaluate_target_count(SelectionPolicy::Batch, 11, 5, 10).is_err());
    }

    #[test]
    fn single_rejects_two_targets_and_ignores_count_settings_for_one() {
        let mut tool = ExternalTool::defaults_for_viewing();
        tool.selection = SelectionPolicy::Single;
        tool.confirmation_threshold = 0;
        tool.max_targets = 0;

        let one = build_launch_operation(&tool, &real_targets(1)).unwrap();
        assert_eq!(one.target_count_decision, TargetCountDecision::Proceed);
        assert!(launch_confirmation_ready(one).is_ok());

        let error = build_launch_operation(&tool, &real_targets(2)).unwrap_err();
        assert!(error.contains("2 件"));
        assert!(error.contains("起動できません"));
    }

    #[test]
    fn each_uses_per_tool_confirmation_and_upper_limit() {
        let mut each = ExternalTool::defaults_for_viewing();
        each.selection = SelectionPolicy::Each;
        each.confirmation_threshold = 5;
        each.max_targets = 10;

        assert!(
            launch_confirmation_ready(build_launch_operation(&each, &real_targets(5)).unwrap())
                .is_ok()
        );
        let confirmation =
            launch_confirmation_ready(build_launch_operation(&each, &real_targets(6)).unwrap())
                .unwrap_err();
        assert_eq!(
            confirmation.operation.target_count_decision(),
            TargetCountDecision::Confirm { target_count: 6 }
        );

        let error = build_launch_operation(&each, &real_targets(11)).unwrap_err();
        assert!(error.contains("対象は 11 件"));
        assert!(error.contains("上限は 10 件"));
        assert!(error.contains("起動と連携"));
    }

    #[test]
    fn batch_confirmation_counts_targets_for_executable_and_association() {
        let targets = real_targets(6);
        let mut tool = ExternalTool::defaults_for_viewing();
        tool.selection = SelectionPolicy::Batch;
        tool.confirmation_threshold = 5;
        tool.max_targets = 10;
        tool.launch = ExternalToolLaunch::Executable(PathBuf::from(r"C:\Tools\viewer.exe"));
        tool.arguments = "{files}".to_string();

        let executable_confirmation =
            launch_confirmation_ready(build_launch_operation(&tool, &targets).unwrap())
                .unwrap_err();
        assert_eq!(executable_confirmation.operation.target_count(), 6);
        assert_eq!(
            executable_confirmation.operation.target_count_decision(),
            TargetCountDecision::Confirm { target_count: 6 }
        );

        tool.launch = ExternalToolLaunch::Association {
            handler_id: "Photos.App".to_string(),
        };
        let association_confirmation =
            launch_confirmation_ready(build_launch_operation(&tool, &targets).unwrap())
                .unwrap_err();
        assert_eq!(association_confirmation.operation.target_count(), 6);
        assert_eq!(
            association_confirmation.operation.target_count_decision(),
            TargetCountDecision::Confirm { target_count: 6 }
        );
    }

    #[test]
    fn os_default_batch_uses_target_count_for_the_upper_limit() {
        let mut tool = ExternalTool::defaults_for_viewing();
        tool.selection = SelectionPolicy::Batch;
        tool.confirmation_threshold = 5;
        tool.max_targets = 10;
        tool.launch = ExternalToolLaunch::OsDefault;

        let error = build_launch_operation(&tool, &real_targets(11)).unwrap_err();
        assert!(error.contains("対象は 11 件"));
        assert!(error.contains("上限は 10 件"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_command_line_length_matches_rust_regular_argument_quoting() {
        use std::os::windows::ffi::OsStrExt;

        assert_eq!(
            windows_quote_regular_argument(OsStr::new("")),
            OsString::from(r#""""#)
        );
        assert_eq!(
            windows_quote_regular_argument(OsStr::new("plain")),
            OsString::from("plain")
        );
        assert_eq!(
            windows_quote_regular_argument(OsStr::new("two words")),
            OsString::from(r#""two words""#)
        );
        assert_eq!(
            windows_quote_regular_argument(OsStr::new("a\"b")),
            OsString::from(r#"a\"b"#)
        );
        let trailing_backslash = OsStr::new("C:\\two words\\");
        assert_eq!(
            windows_quote_regular_argument(trailing_backslash),
            OsString::from(r#""C:\two words\\""#)
        );
        let consecutive_backslashes = OsString::from(format!("a{}\"b", "\\".repeat(3)));
        assert_eq!(
            windows_quote_regular_argument(&consecutive_backslashes),
            OsString::from(format!("a{}\"b", "\\".repeat(7)))
        );

        assert_eq!(windows_regular_argument_utf16_len(OsStr::new("plain")), 5);
        assert_eq!(
            windows_regular_argument_utf16_len(OsStr::new("two words")),
            "two words".encode_utf16().count() + 2
        );
        assert_eq!(
            windows_regular_argument_utf16_len(trailing_backslash),
            trailing_backslash.encode_wide().count() + 3
        );
        assert_eq!(windows_regular_argument_utf16_len(OsStr::new("a\"b")), 4);
        assert_eq!(windows_regular_argument_utf16_len(OsStr::new("😀")), 2);

        assert_eq!(
            windows_create_process_command_line_utf16_len(OsStr::new("x"), &[]),
            4 // "x" + NUL
        );
    }

    #[test]
    fn bad_exe_format_is_only_windows_error_193() {
        assert!(is_bad_exe_format(&std::io::Error::from_raw_os_error(193)));
        assert!(!is_bad_exe_format(&std::io::Error::from_raw_os_error(2)));
        assert!(!is_bad_exe_format(&std::io::Error::other(
            "not an OS error"
        )));
    }

    #[cfg(windows)]
    #[test]
    fn windows_command_line_length_accepts_the_exact_limit_and_rejects_one_more() {
        // `"x"` + separator + argument + NUL = argument length + 5.
        let exact = OsString::from("a".repeat(CREATE_PROCESS_COMMAND_LINE_MAX_UTF16_UNITS - 5));
        let over = OsString::from("a".repeat(CREATE_PROCESS_COMMAND_LINE_MAX_UTF16_UNITS - 4));
        assert_eq!(
            windows_create_process_command_line_utf16_len(OsStr::new("x"), &[exact]),
            CREATE_PROCESS_COMMAND_LINE_MAX_UTF16_UNITS
        );
        assert_eq!(
            windows_create_process_command_line_utf16_len(OsStr::new("x"), &[over]),
            CREATE_PROCESS_COMMAND_LINE_MAX_UTF16_UNITS + 1
        );
    }

    #[cfg(windows)]
    #[test]
    fn executable_batch_rejects_an_overlong_command_line_but_association_does_not() {
        let long_path = PathBuf::from("a".repeat(CREATE_PROCESS_COMMAND_LINE_MAX_UTF16_UNITS - 4));
        let targets = [LaunchTarget::RealFile(long_path)];
        let mut tool = ExternalTool::defaults_for_viewing();
        tool.selection = SelectionPolicy::Batch;
        tool.launch = ExternalToolLaunch::Executable(PathBuf::from("x"));
        tool.arguments = "{files}".to_string();

        let error = build_launch_operation(&tool, &targets).unwrap_err();
        assert!(error.contains("対象ファイルが多すぎる"));
        assert!(error.contains("32767"));

        tool.launch = ExternalToolLaunch::Association {
            handler_id: "Photos.App".to_string(),
        };
        assert!(build_launch_operation(&tool, &targets).is_ok());
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
        let arguments = expand_arguments(&["{files}".to_string()], &path);
        assert_eq!(arguments[0].encode_wide().collect::<Vec<_>>(), raw.to_vec());
    }
}
