use std::collections::HashSet;
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

const IMPLEMENTED_PLACEHOLDERS: &[&str] = &[
    "{file}",
    "{files}",
    "{dir}",
    "{name}",
    "{stem}",
    "{ext}",
    "{uri}",
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
    targets: Vec<LaunchTarget>,
    target_kind: ExternalToolPickerTargetKind,
    items_generation: u64,
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

#[derive(Clone, Debug)]
pub struct PlaceholderContext {
    file: PathBuf,
    facts: PlaceholderFacts,
}

impl PlaceholderContext {
    pub fn for_file(file: impl Into<PathBuf>) -> Self {
        Self {
            file: file.into(),
            facts: PlaceholderFacts::default(),
        }
    }

    pub fn with_facts(file: impl Into<PathBuf>, facts: PlaceholderFacts) -> Self {
        Self {
            file: file.into(),
            facts,
        }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }
}

/// ツールへ伝える「対象が何だったか」。
///
/// **実体化した一時ファイルのパスからは復元できない。** 一時ファイルは書庫の外に置かれ、
/// ページ番号も再生位置も持たない。対象を解決した UI スレッドの時点で確定させ、
/// 実体化を挟んだ後の引数組み立てまで持ち回る。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaceholderFacts {
    /// 利用者から見た書庫 / PDF / 動画本体。実ファイルならそれ自身。
    /// 変換アーカイブでは cache ZIP ではなく元の RAR / 7z / LZH を指す。
    pub container: Option<PathBuf>,
    /// 書庫内エントリ名。
    pub entry: Option<String>,
    /// 1 始まりのページ番号。
    pub page: Option<u32>,
    /// 動画の再生位置 (秒)。
    pub time_secs: Option<f64>,
}

/// `{time_hms}` の書式。`00:01:23.456`。
fn format_hms(secs: f64) -> String {
    let secs = secs.max(0.0);
    let total_ms = (secs * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_seconds = total_ms / 1000;
    format!(
        "{:02}:{:02}:{:02}.{ms:03}",
        total_seconds / 3600,
        (total_seconds / 60) % 60,
        total_seconds % 60
    )
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
    target_count_decision: TargetCountDecision,
}

/// 1 対象ぶんの「何を作るか」と「ツールへ何と伝えるか」。
///
/// **並列 Vec にしない。** 実体化の結果と placeholder の値は index で突き合わせるので、
/// 別々の Vec にすると片方だけ長さが変わったときに黙ってずれる。
struct ExternalMaterializeTarget {
    request: crate::materializer::MaterializeRequest,
    facts: PlaceholderFacts,
}

struct ExternalMaterializeOperation {
    tool: ExternalTool,
    targets: Vec<ExternalMaterializeTarget>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MaterializeOperationOrigin {
    GridOrContainer,
    FullscreenContextMenu,
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
            arguments: "{file}".to_string(),
            working_directory: None,
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

/// 見開きを 2 件で渡すときの順序。
///
/// **画面の左右ではなく読み順で返す。** `resolve_visible_spread_pair` が返すのは画面上の
/// 左右で、右綴じでは右が先のページになる。ツールが受け取る順は綴じ方向によらず
/// 「先のページが先」であってほしいので、ページ番号の昇順に直す。
fn spread_reading_order(left: usize, right: usize) -> (usize, usize) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
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

fn external_tool_picker_items(
    tools: &[ExternalTool],
    targets: &[LaunchTarget],
) -> Vec<ExternalToolPickerItem> {
    let target = ExternalToolMenuTarget::from_launch_targets(targets);
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
        .any(|placeholder| template.contains(placeholder))
}

/// 空文字を返した placeholder は、そのトークンごと引数列から落ちる (`expand_token`)。
/// 「対象が持っていない情報」と「値が空文字」を区別しない — `-page {page}` に対して
/// `-page` だけを残さないことが要件なので、どちらも同じ扱いでよい。
fn placeholder_value(placeholder: &str, ctx: &PlaceholderContext) -> Option<OsString> {
    let facts = &ctx.facts;
    match placeholder {
        "{file}" => Some(ctx.file.as_os_str().to_os_string()),
        "{dir}" => Some(
            ctx.file
                .parent()
                .map(Path::as_os_str)
                .unwrap_or_else(|| OsStr::new(""))
                .to_os_string(),
        ),
        "{name}" => Some(
            ctx.file
                .file_name()
                .unwrap_or_else(|| OsStr::new(""))
                .to_os_string(),
        ),
        "{stem}" => Some(
            ctx.file
                .file_stem()
                .unwrap_or_else(|| OsStr::new(""))
                .to_os_string(),
        ),
        "{ext}" => Some(
            ctx.file
                .extension()
                .unwrap_or_else(|| OsStr::new(""))
                .to_os_string(),
        ),
        "{container}" => Some(
            facts
                .container
                .as_ref()
                .map(|path| path.as_os_str().to_os_string())
                .unwrap_or_default(),
        ),
        "{entry}" => Some(OsString::from(facts.entry.clone().unwrap_or_default())),
        "{page}" => Some(OsString::from(
            facts.page.map(|page| page.to_string()).unwrap_or_default(),
        )),
        "{time}" => Some(OsString::from(
            facts
                .time_secs
                .map(|secs| format!("{:.3}", secs.max(0.0)))
                .unwrap_or_default(),
        )),
        "{time_ms}" => Some(OsString::from(
            facts
                .time_secs
                .map(|secs| ((secs.max(0.0) * 1000.0).round() as u64).to_string())
                .unwrap_or_default(),
        )),
        "{time_hms}" => Some(OsString::from(
            facts.time_secs.map(format_hms).unwrap_or_default(),
        )),
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

#[cfg(windows)]
fn windows_regular_argument_utf16_len(argument: &OsStr) -> usize {
    use std::os::windows::ffi::OsStrExt;

    let units: Vec<u16> = argument.encode_wide().collect();
    let quoted = units.is_empty()
        || units
            .iter()
            .any(|unit| *unit == b' ' as u16 || *unit == b'\t' as u16);
    let mut length = units.len().saturating_add(usize::from(quoted) * 2);
    let mut preceding_backslashes = 0usize;
    for unit in &units {
        if *unit == b'\\' as u16 {
            preceding_backslashes = preceding_backslashes.saturating_add(1);
        } else {
            if *unit == b'"' as u16 {
                // `Command::args` は quote の直前にある n 個の backslash を 2n+1 個へ
                // するので、元の文字列に n+1 個ぶんが加わる。
                length = length.saturating_add(preceding_backslashes.saturating_add(1));
            }
            preceding_backslashes = 0;
        }
    }
    if quoted {
        // 引用された引数末尾の n 個は、閉じ quote の前で 2n 個になる。
        length = length.saturating_add(preceding_backslashes);
    }
    length
}

/// Rust の `Command::args` が `CreateProcessW` へ渡す command line の UTF-16 長。
///
/// argv[0] の常時引用、引数間の空白、通常引数の引用・escape、終端 NUL をすべて含む。
#[cfg(windows)]
fn windows_create_process_command_line_utf16_len(
    executable: &OsStr,
    arguments: &[OsString],
) -> usize {
    use std::os::windows::ffi::OsStrExt;

    let mut length = executable.encode_wide().count().saturating_add(2); // quoted argv[0]
    for argument in arguments {
        length = length
            .saturating_add(1) // separator
            .saturating_add(windows_regular_argument_utf16_len(argument));
    }
    length.saturating_add(1) // terminating NUL
}

fn build_request_for_files(
    tool: &ExternalTool,
    files: Vec<PathBuf>,
    facts: Vec<PlaceholderFacts>,
) -> Result<ExternalLaunchRequest, String> {
    // 対象ごとの facts が揃っていない呼び出しでも、パスの数だけ context を作る
    // (足りない分は既定 = 何も分からない)。placeholder は空へ展開され、そのトークンは落ちる。
    let mut facts = facts.into_iter();
    let contexts: Vec<_> = files
        .iter()
        .cloned()
        .map(|file| PlaceholderContext::with_facts(file, facts.next().unwrap_or_default()))
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
    // この経路は実ファイルしか扱わない (`real_files_from_targets`)。実ファイルの
    // `{container}` は自分自身で、ページ番号もエントリ名も再生位置も持たない。
    let facts = |files: &[PathBuf]| -> Vec<PlaceholderFacts> {
        files
            .iter()
            .map(|file| PlaceholderFacts {
                container: Some(file.clone()),
                ..PlaceholderFacts::default()
            })
            .collect()
    };
    let requests = match tool.selection {
        SelectionPolicy::Single => {
            let facts = facts(&files);
            vec![build_request_for_files(tool, files, facts)?]
        }
        SelectionPolicy::Each => files
            .into_iter()
            .map(|file| {
                let facts = facts(std::slice::from_ref(&file));
                build_request_for_files(tool, vec![file], facts)
            })
            .collect::<Result<Vec<_>, _>>()?,
        SelectionPolicy::Batch => match &tool.launch {
            ExternalToolLaunch::Executable(_) | ExternalToolLaunch::Association { .. } => {
                let facts = facts(&files);
                vec![build_request_for_files(tool, files, facts)?]
            }
            // OS 既定アプリへ複数パスをまとめて渡す API はないため Each と同じ。
            ExternalToolLaunch::OsDefault => files
                .into_iter()
                .map(|file| {
                    let facts = facts(std::slice::from_ref(&file));
                    build_request_for_files(tool, vec![file], facts)
                })
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

fn start_materialize_launch_worker(
    operation: ExternalMaterializeOperation,
    mut session: crate::materializer::MaterializeSession,
    generation: u64,
    owner_hwnd: Option<isize>,
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
        let label = target.request.source.display_label();
        match session.materialize(&target.request, cancel, generation) {
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
    while prepared.len() < target_count {
        prepared.push(None);
    }

    let mut succeeded_target_count = 0usize;
    let mut refreshes = Vec::new();
    if cancel.load(Ordering::Acquire)
        || session.ensure_current(cancel.as_ref(), generation).is_err()
    {
        if failures.is_empty() {
            failures.push("外部ツールの準備をキャンセルしました".to_string());
        }
        return ExternalLaunchCompletion {
            tool_name,
            target_count,
            succeeded_target_count,
            failures,
            refreshes,
        };
    }

    if prepared.iter().all(Option::is_none) {
        progress.update(target_count, "完了しました");
        return ExternalLaunchCompletion {
            tool_name,
            target_count,
            succeeded_target_count,
            failures,
            refreshes,
        };
    }

    progress.update(target_count, "対象が変わっていないか確認しています");
    if launch_boundary_tx.send(()).is_err() {
        failures.push("外部ツールの起動確認を完了できませんでした".to_string());
        return ExternalLaunchCompletion {
            tool_name,
            target_count,
            succeeded_target_count,
            failures,
            refreshes,
        };
    }
    let launch_authorized = loop {
        if cancel.load(Ordering::Acquire)
            || session.ensure_current(cancel.as_ref(), generation).is_err()
        {
            break false;
        }
        match launch_decision_rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(MaterializeLaunchDecision::Launch) => break true,
            Ok(MaterializeLaunchDecision::Cancel) => break false,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break false,
        }
    };
    if !launch_authorized {
        failures.push("対象が移動したため、古い起動要求を破棄しました".to_string());
        return ExternalLaunchCompletion {
            tool_name,
            target_count,
            succeeded_target_count,
            failures,
            refreshes,
        };
    }
    progress.update(target_count, "外部ツールを起動しています");

    match tool.selection {
        SelectionPolicy::Single | SelectionPolicy::Each => {
            for (index, file) in prepared.iter_mut().enumerate() {
                let Some(file) = file else { continue };
                if session.ensure_current(cancel.as_ref(), generation).is_err() {
                    failures.push("ページが移動したため、古い起動要求を破棄しました".to_string());
                    break;
                }
                let path = file.path().to_path_buf();
                let label = path.display().to_string();
                let facts = vec![targets[index].facts.clone()];
                match build_request_for_files(&tool, vec![path], facts)
                    .and_then(|request| launch_request(request, owner_hwnd))
                {
                    Ok(refresh) => {
                        file.transfer_to_process_directory(tool.keep_temp);
                        succeeded_target_count += 1;
                        if let Some(refresh) = refresh {
                            refreshes.push(refresh);
                        }
                    }
                    Err(error) => failures.push(format!("{label}: {error}")),
                }
            }
        }
        SelectionPolicy::Batch => match &tool.launch {
            ExternalToolLaunch::OsDefault => {
                // OS 既定アプリへ複数パスを一括で渡す API はないため、従来どおり Each 相当。
                for (index, file) in prepared.iter_mut().enumerate() {
                    let Some(file) = file else { continue };
                    if session.ensure_current(cancel.as_ref(), generation).is_err() {
                        failures
                            .push("ページが移動したため、古い起動要求を破棄しました".to_string());
                        break;
                    }
                    let path = file.path().to_path_buf();
                    let label = path.display().to_string();
                    let facts = vec![targets[index].facts.clone()];
                    match build_request_for_files(&tool, vec![path], facts)
                        .and_then(|request| launch_request(request, owner_hwnd))
                    {
                        Ok(refresh) => {
                            file.transfer_to_process_directory(tool.keep_temp);
                            succeeded_target_count += 1;
                            if let Some(refresh) = refresh {
                                refreshes.push(refresh);
                            }
                        }
                        Err(error) => failures.push(format!("{label}: {error}")),
                    }
                }
            }
            ExternalToolLaunch::Executable(_) | ExternalToolLaunch::Association { .. } => {
                if session.ensure_current(cancel.as_ref(), generation).is_ok() {
                    let (files, facts): (Vec<_>, Vec<_>) = prepared
                        .iter()
                        .enumerate()
                        .filter_map(|(index, file)| {
                            let file = file.as_ref()?;
                            Some((file.path().to_path_buf(), targets[index].facts.clone()))
                        })
                        .unzip();
                    let prepared_count = files.len();
                    match build_request_for_files(&tool, files, facts)
                        .and_then(|request| launch_request(request, owner_hwnd))
                    {
                        Ok(refresh) => {
                            for file in prepared.iter_mut().flatten() {
                                file.transfer_to_process_directory(tool.keep_temp);
                            }
                            succeeded_target_count = prepared_count;
                            if let Some(refresh) = refresh {
                                refreshes.push(refresh);
                            }
                        }
                        Err(error) => failures.push(error),
                    }
                } else {
                    failures.push("ページが移動したため、古い起動要求を破棄しました".to_string());
                }
            }
        },
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

fn launch_request(
    request: ExternalLaunchRequest,
    owner_hwnd: Option<isize>,
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
            crate::logger::log(format!(
                "external_tool: spawn {tool_name:?} exe={executable:?} files={} args={:?}",
                files.len(),
                arguments
            ));
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
    fn expand_external_stack_targets(
        &self,
        targets: &[LaunchTarget],
    ) -> Result<Vec<LaunchTarget>, String> {
        expand_stack_targets(targets, self.stack_view.as_ref())
    }

    fn launch_target_item_index(&self, target: &LaunchTarget) -> Option<usize> {
        self.items.iter().position(|item| match (target, item) {
            (LaunchTarget::ImagePage(target), crate::grid_item::GridItem::Image(path)) => {
                crate::folder_tree::path_eq(target, path)
            }
            (
                LaunchTarget::ZipPage {
                    zip_path,
                    entry_name,
                },
                crate::grid_item::GridItem::ZipImage {
                    zip_path: item_zip,
                    entry_name: item_entry,
                },
            ) => crate::folder_tree::path_eq(zip_path, item_zip) && entry_name == item_entry,
            (
                LaunchTarget::PdfPage { pdf_path, page_num },
                crate::grid_item::GridItem::PdfPage {
                    pdf_path: item_pdf,
                    page_num: item_page,
                    ..
                },
            ) => crate::folder_tree::path_eq(pdf_path, item_pdf) && page_num == item_page,
            _ => false,
        })
    }

    fn stack_member_default_params(&self, path: &Path) -> crate::adjustment::AdjustParams {
        path.parent()
            .and_then(|folder| self.find_nearest_favorite(folder))
            .and_then(|favorite| self.adjustment_favorite_params.get(&favorite.id))
            .cloned()
            .unwrap_or_else(|| self.settings.global_preset.clone())
    }

    /// 対象 1 件を「何を作るか」と「ツールへ何と伝えるか」の組へ落とす。
    ///
    /// facts はここでしか作れない。**書庫の元アーカイブも、ページ番号も、動画の再生位置も、
    /// 実体化後の一時ファイルからは復元できない。**
    fn materialize_target(
        &self,
        tool: &ExternalTool,
        target: &LaunchTarget,
    ) -> Result<ExternalMaterializeTarget, String> {
        use crate::materializer::{MaterializeRequest, MaterializeSource, PageEditContext};

        let index = self.launch_target_item_index(target);
        let facts = self.placeholder_facts_for_target(target, index);
        let (source, page_key, fallback_path) = match target {
            // 再生中の動画に「表示中のフレーム」が設定されているときだけ、動画ファイル
            // ではなくフレームを渡す。再生していなければ従来どおり動画ファイル
            // (§4.3 の `VideoPolicy::CurrentFrame` は「再生中でなければ `File` に落ちる」)。
            LaunchTarget::RealFile(path)
                if tool.video == VideoPolicy::CurrentFrame
                    && self.external_tool_video_frame_millis(path).is_some() =>
            {
                (
                    MaterializeSource::VideoFrame {
                        path: path.clone(),
                        target_millis: self
                            .external_tool_video_frame_millis(path)
                            .expect("checked in the guard"),
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
                    password: self.pdf_current_password.clone(),
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
                (self.settings.global_preset.clone(), true)
            };
            PageEditContext {
                page_key,
                stage: self.settings.bake_stage_external_tool,
                creative_lut: self
                    .settings
                    .bake_stage_external_tool
                    .includes_display_adjust()
                    .then(|| self.resolved_creative_lut(&params))
                    .flatten(),
                params,
                conceal_preset: self.current_conceal_preset_from_settings(),
                erase_mono_tolerance: self.settings.erase_inpaint_mono_tolerance,
                comic_source_dims: index.and_then(|index| {
                    self.source_dims_for_idx(index)
                        .map(|(width, height)| [width.round() as usize, height.round() as usize])
                }),
                ai_runtime: self.ai_runtime.clone(),
                ai_model_manager: Arc::clone(&self.ai_model_manager),
                load_page_params_from_db,
            }
        });
        Ok(ExternalMaterializeTarget {
            request: MaterializeRequest {
                source,
                policy: materialize_policy(tool.payload),
                page_edits,
                pdf_render_long_edge: if tool.pdf_render_long_edge == 0 {
                    DEFAULT_PDF_RENDER_LONG_EDGE
                } else {
                    tool.pdf_render_long_edge
                },
                rendered_pixels: None,
            },
            facts,
        })
    }

    /// フルスクリーンで見開きが見えているとき、ツールの `SpreadPolicy` に従って対象を
    /// 組み替える。**それ以外では `None` を返して従来どおりに扱う。**
    ///
    /// ここでしかできない理由は 2 つある。見えている組は `resolve_visible_spread_pair` が
    /// `&mut self` で解決するもので、`Merged` の合成には表示画素 (`ctx` が要る) が必要になる。
    fn external_tool_spread_expansion(
        &mut self,
        ctx: &egui::Context,
        tool: &ExternalTool,
        targets: &[LaunchTarget],
    ) -> Result<Option<Vec<ExternalMaterializeTarget>>, String> {
        if targets.len() != 1 {
            return Ok(None);
        }
        let Some(fs_idx) = self.fullscreen_idx else {
            return Ok(None);
        };
        // 「いまフルスクリーンで見えているページ」以外は見開きの話ではない。一覧から
        // 選んだ 1 件がたまたま同じページでも、そちらは 1 件のまま渡す。
        if self.launch_target_item_index(&targets[0]) != Some(fs_idx) {
            return Ok(None);
        }
        let crate::ui_fullscreen::SpreadPair::Double { left, right } =
            self.resolve_visible_spread_pair(fs_idx)
        else {
            return Ok(None);
        };
        match tool.spread {
            // 現在ページ 1 件。呼び出し側の従来経路をそのまま使う。
            SpreadPolicy::MainPageOnly => Ok(None),
            SpreadPolicy::BothPages => {
                let (first, second) = spread_reading_order(left, right);
                let mut expanded = Vec::with_capacity(2);
                for index in [first, second] {
                    let target = LaunchTarget::from_grid_item(self.items.get(index));
                    expanded.push(self.materialize_target(tool, &target)?);
                }
                Ok(Some(expanded))
            }
            SpreadPolicy::Merged => Ok(Some(vec![
                self.merged_spread_target(ctx, tool, left, right)?,
            ])),
        }
    }

    /// 見開き 2 ページを 1 枚へ合成した対象を作る。
    ///
    /// 合成そのものは <kbd>Ctrl+E</kbd> のエクスポートと**同じ経路**を通す
    /// ([`prepare_spread_export_dialog_target`] → [`render_export_pixels`])。
    /// 見えているものと書き出されるものが食い違わないよう、判断を二重に持たない。
    fn merged_spread_target(
        &mut self,
        ctx: &egui::Context,
        tool: &ExternalTool,
        left: usize,
        right: usize,
    ) -> Result<ExternalMaterializeTarget, String> {
        use sha2::Digest as _;

        let export = self.prepare_spread_export_dialog_target(ctx, left, right)?;
        // preset なしなので隠蔽の再合成は走らず、crop と回転と見開き結合だけ。UI スレッド
        // から同期で呼ぶ短い処理で中断させる相手がいないため、cancel は立てない。
        let never_cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let image = crate::export_dialog::render_export_pixels(
            &export.pixels,
            None,
            None,
            &never_cancelled,
        )
        .map_err(|error| error.to_string())?;
        let mut digest = sha2::Sha256::new();
        digest.update((image.size[0] as u64).to_le_bytes());
        digest.update((image.size[1] as u64).to_le_bytes());
        for pixel in &image.pixels {
            digest.update(pixel.to_array());
        }
        let fingerprint: [u8; 32] = digest.finalize().into();

        Ok(ExternalMaterializeTarget {
            request: crate::materializer::MaterializeRequest {
                source: crate::materializer::MaterializeSource::Rendered {
                    label: export.basename.clone(),
                    fingerprint,
                },
                policy: materialize_policy(tool.payload),
                // 表示画素は補正も注釈も反映済み。ここから更に焼き込むものは無い。
                page_edits: None,
                pdf_render_long_edge: if tool.pdf_render_long_edge == 0 {
                    DEFAULT_PDF_RENDER_LONG_EDGE
                } else {
                    tool.pdf_render_long_edge
                },
                rendered_pixels: Some(Arc::new(image.into_owned())),
            },
            // 合成物に固有のページ番号もエントリ名も無い。`{container}` は 2 ページが
            // 属するもの — 左ページの側から取る。
            facts: PlaceholderFacts {
                container: self
                    .placeholder_facts_for_target(
                        &LaunchTarget::from_grid_item(self.items.get(left)),
                        Some(left),
                    )
                    .container,
                ..PlaceholderFacts::default()
            },
        })
    }

    fn placeholder_facts_for_target(
        &self,
        target: &LaunchTarget,
        index: Option<usize>,
    ) -> PlaceholderFacts {
        match target {
            LaunchTarget::RealFile(path) | LaunchTarget::ImagePage(path) => PlaceholderFacts {
                container: Some(path.clone()),
                // 実ファイルの動画を渡すとき、再生中ならその位置を伝える。
                time_secs: self.external_tool_video_time_secs(path),
                ..PlaceholderFacts::default()
            },
            LaunchTarget::ZipPage {
                zip_path,
                entry_name,
            } => PlaceholderFacts {
                container: Some(self.external_tool_visible_container(zip_path)),
                entry: Some(entry_name.clone()),
                // 書庫内の「ページ番号」は、その書庫を開いて見ているときの並び順でしか
                // 意味を持たない。別フォルダーの検索結果などから渡した場合は付けない。
                page: self
                    .current_folder
                    .as_deref()
                    .filter(|current| crate::folder_tree::path_eq(zip_path, current))
                    .and(index)
                    .map(|index| index as u32 + 1),
                time_secs: None,
            },
            LaunchTarget::PdfPage { pdf_path, page_num } => PlaceholderFacts {
                container: Some(pdf_path.clone()),
                entry: None,
                page: Some(page_num.saturating_add(1)),
                time_secs: None,
            },
            LaunchTarget::Stack(_)
            | LaunchTarget::Virtual(_)
            | LaunchTarget::Unsupported
            | LaunchTarget::None => PlaceholderFacts::default(),
        }
    }

    /// 利用者から見た書庫本体。変換アーカイブでは cache ZIP ではなく元の RAR / 7z / LZH。
    fn external_tool_visible_container(&self, zip_path: &Path) -> PathBuf {
        self.archive_source_override
            .as_ref()
            .filter(|_| {
                self.current_folder
                    .as_deref()
                    .is_some_and(|current| crate::folder_tree::path_eq(zip_path, current))
            })
            .cloned()
            .unwrap_or_else(|| zip_path.to_path_buf())
    }

    /// この対象がいま再生中の動画そのものなら、その再生位置 (秒)。
    ///
    /// `{time}` 用。**別プレイヤーへ「ここから再生して」と伝える値**なので、提示済み
    /// フレームの PTS ではなく再生クロックの現在値を返す。
    fn external_tool_video_time_secs(&self, path: &Path) -> Option<f64> {
        let fs_idx = self.fullscreen_idx?;
        let player = self.fs_video_player(fs_idx)?;
        crate::folder_tree::path_eq(player.path(), path).then(|| player.position_secs())
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
        if player.error().is_some() || player.info().is_none() {
            return None;
        }
        let secs = player.screenshot_target_secs();
        secs.is_finite()
            .then(|| (secs.max(0.0) * 1000.0).round() as u64)
    }

    fn build_materialize_operation(
        &mut self,
        ctx: &egui::Context,
        tool: &ExternalTool,
        targets: &[LaunchTarget],
        origin: MaterializeOperationOrigin,
    ) -> Result<ExternalMaterializeOperation, String> {
        validate_materializable_targets(targets)?;
        let targets = self.expand_external_stack_targets(targets)?;
        validate_materializable_targets(&targets)?;
        // 見開き展開は stack 展開の隣。どちらも「利用者が 1 つ選んだものが、ツールから見ると
        // 何件になるか」を決める段で、件数の判定より前に済ませる必要がある。
        let targets = match self.external_tool_spread_expansion(ctx, tool, &targets)? {
            Some(expanded) => expanded,
            None => targets
                .iter()
                .map(|target| self.materialize_target(tool, target))
                .collect::<Result<Vec<_>, _>>()?,
        };
        let target_count_decision = evaluate_target_count(
            tool.selection,
            targets.len(),
            tool.confirmation_threshold,
            tool.max_targets,
        )?;
        // 発火面 (`origin`) は要求の組み立てに影響しない。以前はここで「フルスクリーンの
        // 現在ページ」を控え、実体化中に一致しなくなったら打ち切っていたが、その打ち切りを
        // やめた (2026-09-01 決定、正本 §4.7)。
        let _ = origin;
        Ok(ExternalMaterializeOperation {
            tool: tool.clone(),
            targets,
            target_count_decision,
        })
    }

    fn external_tool_grid_key_targets(&self) -> Vec<LaunchTarget> {
        resolve_external_targets(
            &self.items,
            self.current_grid_order(),
            &self.checked,
            ExternalTargetSource::GridKey {
                selected: self.selected,
            },
        )
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

    fn request_external_tool_picker(
        &mut self,
        targets: Vec<LaunchTarget>,
        target_kind: ExternalToolPickerTargetKind,
    ) {
        if let Err(error) = validate_materializable_targets(&targets) {
            self.show_feedback_toast(error);
            return;
        }
        if self.settings.external_tools.is_empty() {
            self.show_feedback_toast("外部ツールが登録されていません".to_string());
            return;
        }
        if external_tool_picker_items(&self.settings.external_tools, &targets).is_empty() {
            self.show_feedback_toast(
                "現在の対象で使用できる外部ツールが登録されていません".to_string(),
            );
            return;
        }
        self.external_tool_picker = Some(ExternalToolPickerRequest {
            targets,
            target_kind,
            items_generation: self.items_generation,
        });
    }

    pub(crate) fn request_grid_external_tool_picker(&mut self) {
        let targets = self.external_tool_grid_key_targets();
        self.request_external_tool_picker(targets, ExternalToolPickerTargetKind::GridItems);
    }

    pub(crate) fn request_container_external_tool_picker(&mut self) {
        let targets = self.external_tool_container_targets();
        self.request_external_tool_picker(targets, ExternalToolPickerTargetKind::Container);
    }

    pub(crate) fn launch_grid_external_tool_slot(&mut self, ctx: &egui::Context, slot: usize) {
        let tool = match resolve_external_tool_slot(&self.settings.external_tools, slot) {
            Ok(tool) => tool.clone(),
            Err(error) => {
                self.show_feedback_toast(error);
                return;
            }
        };
        let targets = self.external_tool_grid_key_targets();
        self.queue_external_tool_launch_targets(ctx, &tool, &targets);
    }

    pub(crate) fn queue_external_tool_launch(
        &mut self,
        ctx: &egui::Context,
        tool: &ExternalTool,
        target: &LaunchTarget,
    ) {
        self.queue_external_tool_launch_targets(ctx, tool, std::slice::from_ref(target));
    }

    pub(crate) fn queue_external_tool_launch_targets(
        &mut self,
        ctx: &egui::Context,
        tool: &ExternalTool,
        targets: &[LaunchTarget],
    ) {
        self.queue_external_tool_launch_targets_with_origin(
            ctx,
            tool,
            targets,
            MaterializeOperationOrigin::GridOrContainer,
        );
    }

    pub(crate) fn queue_external_tool_launch_targets_from_context_menu(
        &mut self,
        ctx: &egui::Context,
        tool: &ExternalTool,
        targets: &[LaunchTarget],
        fullscreen_will_close: bool,
    ) {
        let origin = if fullscreen_will_close {
            MaterializeOperationOrigin::FullscreenContextMenu
        } else {
            MaterializeOperationOrigin::GridOrContainer
        };
        self.queue_external_tool_launch_targets_with_origin(ctx, tool, targets, origin);
    }

    fn queue_external_tool_launch_targets_with_origin(
        &mut self,
        ctx: &egui::Context,
        tool: &ExternalTool,
        targets: &[LaunchTarget],
        origin: MaterializeOperationOrigin,
    ) {
        let operation = match self.build_materialize_operation(ctx, tool, targets, origin) {
            Ok(operation) => operation,
            Err(error) => {
                self.show_feedback_toast(format!("{}: {error}", tool.display_name()));
                return;
            }
        };
        match launch_confirmation(ExternalQueuedOperation::Materialize(operation)) {
            Ok(operation) => self.start_external_queued_operation(operation),
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
            target_count_decision: TargetCountDecision::Proceed,
        };
        match launch_confirmation(ExternalQueuedOperation::Ready(operation)) {
            Ok(operation) => self.start_external_queued_operation(operation),
            Err(confirmation) => {
                self.external_tool_launch_confirmation = Some(confirmation);
            }
        }
    }

    fn start_external_launch_operation(&mut self, operation: ExternalLaunchOperation) {
        let tool_name = operation.tool_name.clone();
        let target_count = operation.target_count;
        match start_launch_worker(operation, self.main_hwnd) {
            Ok(pending) => self.external_tool_launch_pending.push(pending),
            Err(error) if target_count == 1 => {
                self.show_feedback_toast(format!("{tool_name}: {error}"))
            }
            Err(error) => self.show_feedback_toast(format!(
                "{tool_name}: {target_count} 件の起動を開始できませんでした ({error})"
            )),
        }
    }

    fn start_external_queued_operation(&mut self, operation: ExternalQueuedOperation) {
        match operation {
            ExternalQueuedOperation::Ready(operation) => {
                self.start_external_launch_operation(operation)
            }
            ExternalQueuedOperation::Materialize(operation) => {
                for pending in &mut self.external_tool_materialize_pending {
                    pending.cancel(false);
                }
                let generation = self.external_tool_materializer.begin_generation();
                let session = self.external_tool_materializer.session();
                let tool_name = operation.tool_name();
                let target_count = operation.target_count();
                match start_materialize_launch_worker(
                    operation,
                    session,
                    generation,
                    self.main_hwnd,
                ) {
                    Ok(pending) => self.external_tool_materialize_pending.push(pending),
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
        let mut index = 0;
        while index < self.external_tool_launch_pending.len() {
            if let Some(completion) = self.external_tool_launch_pending[index].poll_completion() {
                self.external_tool_launch_pending.remove(index);
                finished.push(completion);
            } else {
                index += 1;
            }
        }
        let mut materialize_index = 0;
        while materialize_index < self.external_tool_materialize_pending.len() {
            // 打ち切るのは「同じツールの新しい要求に置き換えられたとき」だけ
            // (2026-09-01 決定、正本 §4.7)。一覧の差し替えやビューア位置では打ち切らない。
            let superseded = {
                let pending = &self.external_tool_materialize_pending[materialize_index];
                !self
                    .external_tool_materializer
                    .generation_is_current(pending.generation)
            };
            if superseded {
                let generation =
                    self.external_tool_materialize_pending[materialize_index].generation;
                crate::logger::log(format!(
                    "external_tool: materialize superseded (request={generation})"
                ));
                self.external_tool_materialize_pending[materialize_index].cancel(false);
            }
            if let Some(completion) =
                self.external_tool_materialize_pending[materialize_index].poll_completion()
            {
                let cancelled =
                    self.external_tool_materialize_pending[materialize_index].user_cancelled;
                self.external_tool_materialize_pending
                    .remove(materialize_index);
                if !cancelled {
                    finished.push(completion);
                }
            } else {
                materialize_index += 1;
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
        if !self.external_tool_launch_pending.is_empty()
            || !self.external_tool_materialize_pending.is_empty()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    /// 進捗 modal より後の navigation / items mutation もすべて終わった frame tail で、
    /// その frame に UI checkpoint を通った要求だけ spawn 境界を ACK する。
    pub(crate) fn authorize_external_tool_launch_boundaries_after_ui(&mut self) {
        let mut index = 0;
        while index < self.external_tool_materialize_pending.len() {
            let ui_checkpoint_passed =
                self.external_tool_materialize_pending[index].take_launch_ui_checkpoint();
            let current = {
                let pending = &self.external_tool_materialize_pending[index];
                self.external_tool_materializer
                    .generation_is_current(pending.generation)
            };
            if ui_checkpoint_passed
                && current
                && self.external_tool_materialize_pending[index].can_cancel_before_launch()
            {
                self.external_tool_materialize_pending[index].resolve_launch_boundary(true);
            } else if !current {
                let generation = self.external_tool_materialize_pending[index].generation;
                crate::logger::log(format!(
                    "external_tool: materialize superseded at launch boundary (request={generation})"
                ));
                self.external_tool_materialize_pending[index].cancel(false);
            }
            index += 1;
        }
    }

    fn show_external_tool_launch_confirmation(&mut self, ctx: &egui::Context) {
        let Some(confirmation) = self.external_tool_launch_confirmation.as_ref() else {
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
                    ui.label(format!("実行ファイル: {executable}"));
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
            if let Some(confirmation) = self.external_tool_launch_confirmation.take() {
                self.start_external_queued_operation(confirmation.operation);
            }
        } else if cancel {
            self.external_tool_launch_confirmation = None;
        }
    }

    /// 準備進捗 modal がこの frame で描かれるか。**表示条件の唯一の持ち主**にして、
    /// 入力ブロック判定 (`modal_dialog_block_reason`) もここから導く。
    pub(crate) fn external_tool_materialize_progress_visible(&self) -> bool {
        self.external_tool_materialize_pending
            .iter()
            .any(|pending| {
                self.external_tool_materializer
                    .generation_is_current(pending.generation)
            })
    }

    fn show_external_tool_materialize_progress(&mut self, ctx: &egui::Context) {
        let Some(index) = self
            .external_tool_materialize_pending
            .iter()
            .rposition(|pending| {
                self.external_tool_materializer
                    .generation_is_current(pending.generation)
            })
        else {
            return;
        };
        let (completed, total, stage) = self.external_tool_materialize_pending[index]
            .progress
            .snapshot();
        let can_cancel = self.external_tool_materialize_pending[index].can_cancel_before_launch();
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
        if cancel {
            self.external_tool_materialize_pending[index].cancel(true);
            self.external_tool_materializer.cancel_all();
        } else if can_cancel {
            self.external_tool_materialize_pending[index].mark_launch_ui_checkpoint();
        }
    }

    pub(crate) fn shutdown_external_tool_materializer(&mut self) {
        self.external_tool_materializer.cancel_all();
        for pending in &mut self.external_tool_materialize_pending {
            pending.join_for_exit();
        }
        self.external_tool_materialize_pending.clear();
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
        if self.external_tool_launch_ui_frame == Some(self.frame_counter) {
            return;
        }
        self.external_tool_launch_ui_frame = Some(self.frame_counter);
        self.show_external_tool_picker(ctx);
        self.show_external_tool_launch_confirmation(ctx);
        self.show_external_tool_materialize_progress(ctx);
    }

    fn show_external_tool_picker(&mut self, ctx: &egui::Context) {
        let Some(request) = self.external_tool_picker.as_ref() else {
            return;
        };
        let target_kind = request.target_kind;
        let tools = external_tool_picker_items(&self.settings.external_tools, &request.targets);
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
            let request = self.external_tool_picker.take();
            if let (Some(tool), Some(request)) = (tool, request) {
                if request.items_generation != self.items_generation {
                    self.show_feedback_toast(
                        "対象が移動したため、外部ツールをもう一度選択してください".to_string(),
                    );
                } else {
                    self.queue_external_tool_launch_targets(ctx, &tool, &request.targets);
                }
            } else {
                self.show_feedback_toast("外部ツールを選択できませんでした".to_string());
            }
        } else if cancel {
            self.external_tool_picker = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(tool.arguments, "{file}");
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
            2,
            "描き手は main update の tail と fullscreen body の 2 か所から呼ぶ              (フルスクリーン中は tail が飛ぶため)"
        );
    }

    /// supersede 済みの要求は進捗 modal に出ない。**入力ブロックも同じ述語から導く**ので、
    /// 「ダイアログが無いのにクリックが効かない」状態を作らない。
    ///
    /// この 2 つを別々の条件で書いていたのが実機の固着の一部だった (2026-09-02)。
    #[test]
    fn a_superseded_materialize_request_blocks_no_input_because_it_draws_no_dialog() {
        let mut app = crate::app::setup_app_for_test();
        let generation = app.external_tool_materializer.begin_generation();
        app.external_tool_materialize_pending
            .push(materialize_pending_for_test(generation));

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
            !app.external_tool_materialize_pending.is_empty(),
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
            size: 1,
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

        assert!(app.external_tool_launch_pending.is_empty());
        assert!(app.external_tool_launch_confirmation.is_none());
        assert!(app.external_tool_picker.is_none());
        assert!(app.fs_feedback_toast.as_ref().is_some_and(|(text, _, _)| {
            text == "外部ツールスロット 5 にはツールが登録されていません"
        }));
    }

    #[test]
    fn picker_accepts_materializable_pages_rejects_virtual_directories_and_snapshots_targets() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.external_tools = vec![ExternalTool::defaults_for_viewing()];
        app.request_external_tool_picker(
            vec![LaunchTarget::ZipPage {
                zip_path: PathBuf::from("book.zip"),
                entry_name: "original.jpg".to_string(),
            }],
            ExternalToolPickerTargetKind::GridItems,
        );
        assert!(app.external_tool_picker.is_some());

        app.external_tool_picker = None;
        app.request_external_tool_picker(
            vec![LaunchTarget::Virtual(
                crate::grid_item::FileOperationRefusal::ArchiveDirectory,
            )],
            ExternalToolPickerTargetKind::GridItems,
        );
        assert!(app.external_tool_picker.is_none());
        assert!(
            app.fs_feedback_toast.as_ref().is_some_and(|(text, _, _)| {
                text.contains("圧縮ファイル内のフォルダ")
            })
        );

        app.request_external_tool_picker(
            vec![LaunchTarget::RealFile(PathBuf::from("original.jpg"))],
            ExternalToolPickerTargetKind::GridItems,
        );
        app.items = vec![crate::grid_item::GridItem::Image(PathBuf::from(
            "changed.jpg",
        ))];
        assert_eq!(
            app.external_tool_picker
                .as_ref()
                .map(|request| request.targets.clone()),
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

    fn facts_for_pdf_page(container: &str, page: u32) -> PlaceholderFacts {
        PlaceholderFacts {
            container: Some(PathBuf::from(container)),
            entry: None,
            page: Some(page),
            time_secs: None,
        }
    }

    /// Acrobat / SumatraPDF に「PDF 本体 + ページ番号」を渡す形。**一時 PNG のパスでは
    /// ない**ので、実体化後のパスからは復元できない値になる。
    #[test]
    fn container_and_page_describe_the_source_not_the_materialized_file() {
        let tokens = split_argument_template(r#"-page {page} "{container}" {file}"#);
        let ctx = PlaceholderContext::with_facts(
            PathBuf::from(r"C:\Temp\ext-1\page_003.png"),
            facts_for_pdf_page(r"C:\Books\manual.pdf", 3),
        );

        assert_eq!(
            expand_arguments(&tokens, &ctx),
            vec![
                OsString::from("-page"),
                OsString::from("3"),
                OsString::from(r"C:\Books\manual.pdf"),
                OsString::from(r"C:\Temp\ext-1\page_003.png"),
            ]
        );
    }

    /// 対象が持っていない値は**そのトークンごと落ちる**。`-page` だけが残ると、受け取った
    /// 側は引数の解釈に失敗する。
    #[test]
    fn a_target_without_a_page_drops_the_option_and_its_value_together() {
        let tokens = split_argument_template(r#"-page {page} --at {time} "{file}""#);
        let ctx = PlaceholderContext::with_facts(
            PathBuf::from(r"C:\Images\photo.jpg"),
            PlaceholderFacts {
                container: Some(PathBuf::from(r"C:\Images\photo.jpg")),
                ..PlaceholderFacts::default()
            },
        );

        assert_eq!(
            expand_arguments(&tokens, &ctx),
            vec![OsString::from(r"C:\Images\photo.jpg")]
        );
    }

    #[test]
    fn zip_entry_name_and_video_time_reach_the_argument_list() {
        let tokens = split_argument_template("{entry} {time} {time_ms} {time_hms}");
        let ctx = PlaceholderContext::with_facts(
            PathBuf::from(r"C:\Temp\ext-1\page.jpg"),
            PlaceholderFacts {
                container: Some(PathBuf::from(r"C:\Books\comic.zip")),
                entry: Some("chapter1/page03.jpg".to_string()),
                page: None,
                time_secs: Some(83.4567),
            },
        );

        assert_eq!(
            expand_arguments(&tokens, &ctx),
            vec![
                OsString::from("chapter1/page03.jpg"),
                OsString::from("83.457"),
                OsString::from("83457"),
                OsString::from("00:01:23.457"),
            ]
        );
    }

    /// 右綴じ (画面の右が先のページ) でも、ツールが受け取る順は読み順。
    #[test]
    fn a_spread_is_handed_over_in_reading_order_not_screen_order() {
        assert_eq!(spread_reading_order(4, 5), (4, 5));
        assert_eq!(spread_reading_order(5, 4), (4, 5));
        assert_eq!(spread_reading_order(7, 7), (7, 7));
    }

    #[test]
    fn hms_carries_into_hours_and_never_goes_negative() {
        assert_eq!(format_hms(0.0), "00:00:00.000");
        assert_eq!(format_hms(-5.0), "00:00:00.000");
        assert_eq!(format_hms(59.9995), "00:01:00.000");
        assert_eq!(format_hms(3661.5), "01:01:01.500");
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

        tool.arguments = "--input {file}".to_string();
        assert!(
            build_launch_operation(&tool, &targets)
                .unwrap_err()
                .contains("{files}")
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

        assert_eq!(windows_regular_argument_utf16_len(OsStr::new("plain")), 5);
        assert_eq!(
            windows_regular_argument_utf16_len(OsStr::new("two words")),
            "two words".encode_utf16().count() + 2
        );
        let trailing_backslash = OsStr::new("C:\\two words\\");
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
        let arguments = expand_arguments(
            &["{file}".to_string()],
            &PlaceholderContext::for_file(&path),
        );
        assert_eq!(arguments[0].encode_wide().collect::<Vec<_>>(), raw.to_vec());
    }
}
