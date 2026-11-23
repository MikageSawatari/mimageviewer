//! Agent reads and a shared UI admission bridge for state, open and metadata writes.
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mimageviewer_agent_ipc::*;
use serde::{Deserialize, Serialize};
pub(crate) mod changes;
pub(crate) mod favorite;
mod folder;
mod input_path;
mod item_info;
mod library;
mod metadata_response;
#[cfg(test)]
mod metadata_response_tests;
pub(crate) mod metadata_write;
pub(crate) mod open;
mod open_response;
mod rating;
mod recent;
mod stack_script;
#[cfg(test)]
mod stack_script_tests;
mod status;
pub(crate) use recent::{RecentApps, SharedRecentApps};
mod collection_write;
#[cfg(test)]
mod collection_write_tests;
mod thumbnail;
mod transcript;
mod work;
use work::*;

#[cfg(test)]
mod compatibility_tests;
#[cfg(test)]
mod favorite_tests;
#[cfg(test)]
mod logging_tests;
#[cfg(test)]
mod media_read_tests;
#[cfg(test)]
mod open_connection_tests;
#[cfg(test)]
mod permission_admission_tests;
#[cfg(windows)]
mod pipe;
#[cfg(test)]
mod read_tests;
#[cfg(test)]
mod recent_tests;
#[cfg(test)]
mod settings_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod view_state_tests;

pub(crate) struct Server {
    open: Arc<open::OpenController>,
    recent_apps: SharedRecentApps,
    changes: Arc<changes::ChangeController>,
    #[cfg(windows)]
    pipe: pipe::Server,
}

impl Server {
    pub(crate) fn start(
        readers: Arc<crate::remote_ipc::SharedReaders>,
        producer: Option<crate::collection_store::CollectionRemoteProducerControl>,
        session: Option<crate::remote_ipc::session::SessionHandle>,
    ) -> std::io::Result<Self> {
        #[cfg(windows)]
        {
            let open = open::OpenController::new(session.clone());
            let recent_apps = Arc::new(Mutex::new(RecentApps::default()));
            let changes = changes::ChangeController::new();
            if let Some(lease) = producer
                .as_ref()
                .and_then(|producer| producer.begin_request())
            {
                readers.install_collection_store(lease.client().clone());
            }
            Ok(Self {
                open: open.clone(),
                recent_apps: recent_apps.clone(),
                changes: changes.clone(),
                pipe: pipe::Server::start(Arc::new(Handlers {
                    readers,
                    producer,
                    session,
                    open,
                    recent_apps,
                    changes,
                    active: AtomicUsize::new(0),
                    lane_log: Mutex::new(LaneLog::default()),
                    #[cfg(test)]
                    search_hooks: None,
                    #[cfg(test)]
                    read_hooks: None,
                    #[cfg(test)]
                    write_hooks: None,
                }))?,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = (readers, producer, session);
            Err(std::io::Error::other("Windows only"))
        }
    }

    pub(crate) fn open_controller(&self) -> Arc<open::OpenController> {
        self.open.clone()
    }

    pub(crate) fn recent_apps(&self) -> SharedRecentApps {
        self.recent_apps.clone()
    }

    pub(crate) fn changes_controller(&self) -> Arc<changes::ChangeController> {
        self.changes.clone()
    }
}

struct Handlers {
    readers: Arc<crate::remote_ipc::SharedReaders>,
    producer: Option<crate::collection_store::CollectionRemoteProducerControl>,
    session: Option<crate::remote_ipc::session::SessionHandle>,
    open: Arc<open::OpenController>,
    recent_apps: SharedRecentApps,
    changes: Arc<changes::ChangeController>,
    active: AtomicUsize,
    lane_log: Mutex<LaneLog>,
    #[cfg(test)]
    search_hooks: Option<Arc<SearchHooks>>,
    #[cfg(test)]
    read_hooks: Option<Arc<ReadHooks>>,
    #[cfg(test)]
    write_hooks: Option<Arc<collection_write::WriteHooks>>,
}

#[cfg(test)]
struct SearchHooks {
    now: Box<dyn Fn() -> Instant + Send + Sync>,
    worker: Box<
        dyn Fn(
                &str,
                &[uuid::Uuid],
                &Arc<AtomicBool>,
                &crossbeam_channel::Sender<crate::global_search::SearchStreamEvent>,
            ) + Send
            + Sync,
    >,
}

struct Running {
    id: u64,
    cancel: Arc<AtomicBool>,
}

struct Reply {
    response: Response,
    policy: ReplyPolicy,
}
type PermissionGrant = (PermissionSnapshot, u64);

/// Adapter calls use a fresh pipe and handshake, including each idle poll.
/// Observe server state across those short-lived connections instead of logging
/// their routine connect/EOF churn. Never retain client strings or read results.
#[derive(Default)]
struct LaneLog {
    hello: Option<Result<(), ErrorCode>>,
    status: Option<Result<(), ErrorCode>>,
    permission: Option<Result<PermissionGrant, ErrorCode>>,
    #[cfg(test)]
    lines: Vec<String>,
}

impl LaneLog {
    fn emit(&mut self, line: String) {
        #[cfg(test)]
        self.lines.push(line.clone());
        crate::logger::log(line);
    }

    fn permission(&mut self, observed: &Result<PermissionGrant, AgentError>) {
        let observed = observed.as_ref().copied().map_err(|error| error.code);
        if self.permission.as_ref() != Some(&observed) {
            self.permission = Some(observed);
            self.emit("agent_ipc: state=permission_changed".into());
        }
    }

    fn request(
        &mut self,
        tool: &'static str,
        started: Instant,
        result: &Result<Payload, AgentError>,
    ) -> bool {
        let previous = match tool {
            "hello" => &mut self.hello,
            "miv_status" => &mut self.status,
            _ => {
                self.emit(request_log_line(tool, started, result));
                return true;
            }
        };
        let observed = result.as_ref().map(|_| ()).map_err(|error| error.code);
        if previous.as_ref() != Some(&observed) {
            *previous = Some(observed);
            self.emit(request_log_line(tool, started, result));
            return true;
        }
        false
    }
}

enum ReplyPolicy {
    Read(PermissionGrant),
    Rejected,
    // Accepted operations retain their result after UI/actor acceptance.
    AcceptedOperation,
}
impl Reply {
    fn wire(mut self) -> Response {
        if let ReplyPolicy::Read(granted) = self.policy {
            self.response.result = permission().and_then(|current| {
                if current != granted {
                    Err(error(
                        ErrorCode::PermissionChanged,
                        "AI 連携の許可が変更されました。もう一度お試しください",
                    ))
                } else {
                    self.response.result
                }
            });
        }
        self.response
    }
}

/// The slot remains occupied until the actual read worker returns, including after
/// a deadline response. A connection can cancel only its own active identity.
struct Connection {
    handlers: Arc<Handlers>,
    running: Mutex<Option<Running>>,
    handshake: Mutex<HandshakeState>,
}

enum HandshakeState {
    Awaiting,
    Accepted(ClientRequest),
    Rejected,
}

#[derive(Clone)]
struct ClientRequest {
    identity: ClientIdentity,
    kind: RequestKind,
    adapter_protocol_version: u32,
}

struct Permit {
    connection: Arc<Connection>,
}
impl Drop for Permit {
    fn drop(&mut self) {
        self.connection.running.lock().unwrap().take();
        self.connection
            .handlers
            .active
            .fetch_sub(1, Ordering::AcqRel);
    }
}

fn error(code: ErrorCode, message: &str) -> AgentError {
    AgentError::new(code, message)
}
fn internal() -> AgentError {
    error(ErrorCode::Internal, "読み取りに失敗しました")
}
fn collection_error(cause: crate::collection_store::CollectionStoreError) -> AgentError {
    match cause {
        crate::collection_store::CollectionStoreError::Starting => {
            error(ErrorCode::NotReady, "コレクションを準備しています…")
        }
        _ => internal(),
    }
}
fn cancelled() -> AgentError {
    error(ErrorCode::Cancelled, "取り消しました")
}
fn permission() -> Result<PermissionGrant, AgentError> {
    enabled_permission(load_permission()?)
}
fn load_permission() -> Result<PermissionGrant, AgentError> {
    crate::settings_db::with_db_result(|db| db.load_agent_permission())
        .map_err(|_| error(ErrorCode::Busy, "設定を読み込めませんでした"))
}
const DISABLED_PERMISSION_MESSAGE: &str =
    "mIV の AI 連携が無効です。mIV の「設定」→「AI 連携…」で許可の範囲を選んでください";
const THUMBNAIL_PERMISSION_MESSAGE: &str =
    "mIV の「設定」→「AI 連携…」で「縮小画像を AI に見せる」を ON にしてください";
const TRANSCRIPT_PERMISSION_MESSAGE: &str =
    "mIV の「設定」→「AI 連携…」で「文字起こしを AI に見せる」を ON にしてください";
const OPEN_PERMISSION_MESSAGE: &str =
    "mIV の「設定」→「AI 連携…」で許可の範囲を「読み取りと、mIV で開く」以上にしてください";
pub(crate) const CURRENT_VIEW_PERMISSION_MESSAGE: &str =
    "mIV の「設定」→「AI 連携…」で「今開いている場所を AI に見せる」を ON にしてください";
pub(crate) const SETTINGS_PERMISSION_MESSAGE: &str =
    "mIV の「設定」→「AI 連携…」で「設定の確認と変更を AI に任せる」を ON にしてください";
const ORGANIZE_PERMISSION_MESSAGE: &str =
    "mIV の「設定」→「AI 連携…」で許可の範囲を「整理まで」にしてください";

/// 一覧は公開の機能説明として固定。要求の認可はcoreのこの受付で行う。
fn check_tool_permission(
    snapshot: PermissionSnapshot,
    command: &Command,
) -> Result<(), AgentError> {
    let denied = match command {
        Command::GetViewState if !snapshot.current_view => Some(CURRENT_VIEW_PERMISSION_MESSAGE),
        Command::GetSettings { .. }
        | Command::SetSettings { .. }
        | Command::SetFavorite { .. }
        | Command::StackScript { .. }
            if !snapshot.settings =>
        {
            Some(SETTINGS_PERMISSION_MESSAGE)
        }
        Command::GetThumbnail { .. } if !snapshot.thumbnails => Some(THUMBNAIL_PERMISSION_MESSAGE),
        Command::GetTranscript { .. } if !snapshot.transcripts => {
            Some(TRANSCRIPT_PERMISSION_MESSAGE)
        }
        Command::OpenInViewer { .. } if snapshot.level < AgentLevel::ReadAndOpen => {
            Some(OPEN_PERMISSION_MESSAGE)
        }
        _ if (collection_write::is_write(command) || metadata_write::is_write(command))
            && snapshot.level < AgentLevel::Organize =>
        {
            Some(ORGANIZE_PERMISSION_MESSAGE)
        }
        _ => None,
    };
    denied.map_or(Ok(()), |message| Err(error(ErrorCode::Disabled, message)))
}

fn enabled_permission((snapshot, epoch): PermissionGrant) -> Result<PermissionGrant, AgentError> {
    if snapshot.level == AgentLevel::Disabled {
        Err(error(ErrorCode::Disabled, DISABLED_PERMISSION_MESSAGE))
    } else {
        Ok((snapshot, epoch))
    }
}

impl Connection {
    fn new(handlers: Arc<Handlers>) -> Arc<Self> {
        Arc::new(Self {
            handlers,
            running: Mutex::new(None),
            handshake: Mutex::new(HandshakeState::Awaiting),
        })
    }

    /// The first frame is a stable envelope, not a business Request. In
    /// particular, unsupported versions may have completely different fields.
    fn read_handshake(
        self: &Arc<Self>,
        reader: &mut impl std::io::Read,
        replies: crossbeam_channel::Sender<Reply>,
    ) -> std::io::Result<()> {
        let invalid =
            || std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid agent hello");
        let frame: serde_json::Value = read_frame(reader)?.ok_or_else(invalid)?;
        let id = frame["id"].as_u64().ok_or_else(invalid)?;
        if frame["command"]["type"].as_str() != Some("hello") {
            return Err(invalid());
        }
        let version = frame["command"]["version"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(invalid)?;
        if !supports_adapter_version(version) {
            *self.handshake.lock().unwrap() = HandshakeState::Rejected;
            self.reply_hello(
                id,
                Ok(Payload::Hello {
                    version: AGENT_PROTOCOL_VERSION,
                }),
                replies,
            );
            return Ok(());
        }
        // Versions 3 and 4 share the remaining Hello fields. Only decode them
        // after recognizing a supported version; future versions need not do so.
        let request: Request = serde_json::from_value(frame).map_err(std::io::Error::other)?;
        let _ = self.request(request, replies); // Hello never creates a business worker.
        Ok(())
    }

    fn reply_hello(
        &self,
        id: u64,
        result: Result<Payload, AgentError>,
        replies: crossbeam_channel::Sender<Reply>,
    ) {
        self.record(None, "hello", Instant::now(), &result);
        let _ = replies.send(Reply {
            response: Response { id, result },
            policy: ReplyPolicy::Rejected,
        });
    }

    fn disconnect(&self) {
        if let Some(running) = self.running.lock().unwrap().as_ref() {
            running.cancel.store(true, Ordering::Release);
        }
    }

    fn acquire(self: &Arc<Self>, id: u64) -> Result<(Permit, Arc<AtomicBool>), AgentError> {
        let mut running = self.running.lock().unwrap();
        if running.is_some()
            || self
                .handlers
                .active
                .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                    (n < 2).then_some(n + 1)
                })
                .is_err()
        {
            return Err(error(
                ErrorCode::Busy,
                "処理中です。少し待ってから再試行してください",
            ));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *running = Some(Running {
            id,
            cancel: cancel.clone(),
        });
        Ok((
            Permit {
                connection: self.clone(),
            },
            cancel,
        ))
    }

    fn request(
        self: &Arc<Self>,
        request: Request,
        replies: crossbeam_channel::Sender<Reply>,
    ) -> Option<JoinHandle<()>> {
        let started = Instant::now();
        let tool = tool_name(&request.command);
        // Handshake has no permission lookup, worker, or admission slot. In
        // particular, a disabled/busy tool still reaches core and is recorded.
        if let Command::Hello {
            version,
            client_name,
            client_version,
            request_kind,
        } = &request.command
        {
            let mut handshake = self.handshake.lock().unwrap();
            // One Hello per connection. Repeated Hellos never renegotiate the
            // version or relabel the client, including while a job is running.
            let result = if !matches!(*handshake, HandshakeState::Awaiting) {
                Err(error(ErrorCode::BadRequest, "接続の版は確認済みです"))
            } else if supports_adapter_version(*version) {
                *handshake = HandshakeState::Accepted(ClientRequest {
                    identity: ClientIdentity::new(client_name, client_version),
                    kind: *request_kind,
                    adapter_protocol_version: *version,
                });
                Ok(Payload::Hello { version: *version })
            } else {
                *handshake = HandshakeState::Rejected;
                Ok(Payload::Hello {
                    version: AGENT_PROTOCOL_VERSION,
                })
            };
            drop(handshake);
            self.reply_hello(request.id, result, replies);
            return None;
        }
        let (client, handshake_error) = match &*self.handshake.lock().unwrap() {
            HandshakeState::Accepted(client) => (Some(client.clone()), None),
            HandshakeState::Awaiting => (
                None,
                Some(error(
                    ErrorCode::BadRequest,
                    "先に接続の版を確認してください",
                )),
            ),
            HandshakeState::Rejected => (None, Some(version_mismatch())),
        };
        let reject = |err| {
            let result = Err(err);
            self.record(client.as_ref(), tool, started, &result);
            let _ = replies.send(Reply {
                response: Response {
                    id: request.id,
                    result,
                },
                policy: ReplyPolicy::Rejected,
            });
        };
        // Gate every business request, including Cancel, before permissions,
        // admission slots, or handlers can observe an unnegotiated connection.
        if let Some(err) = handshake_error {
            reject(err);
            return None;
        }
        let observed = load_permission();
        self.handlers.lane_log.lock().unwrap().permission(&observed);
        let granted = match observed.and_then(enabled_permission) {
            Ok(p) => p,
            Err(err) => {
                reject(err);
                return None;
            }
        };
        if let Command::Cancel { request_id } = request.command {
            if let Some(running) = self
                .running
                .lock()
                .unwrap()
                .as_ref()
                .filter(|r| r.id == request_id)
            {
                running.cancel.store(true, Ordering::Release);
            }
            let result = permission().map(|_| Payload::Cancelled);
            self.record(client.as_ref(), tool, started, &result);
            let _ = replies.send(Reply {
                response: Response {
                    id: request.id,
                    result,
                },
                policy: ReplyPolicy::Read(granted),
            });
            return None;
        }
        if let Err(err) = check_tool_permission(granted.0, &request.command) {
            reject(err);
            return None;
        }
        let (permit, cancel) = match self.acquire(request.id) {
            Ok(value) => value,
            Err(err) => {
                reject(err);
                return None;
            }
        };
        let connection = self.clone();
        let fallback = replies.clone();
        let id = request.id;
        let context = ReadContext::for_adapter(
            &self.handlers,
            granted,
            cancel,
            client.as_ref().unwrap().adapter_protocol_version,
        );
        let record_client = client.clone();
        match std::thread::Builder::new()
            .name("agent-read".into())
            .spawn(move || {
                if collection_write::is_write(&request.command) {
                    collection_write::respond(
                        connection,
                        request.command,
                        permit,
                        context,
                        replies,
                        started,
                        id,
                        tool,
                        client,
                    );
                    return;
                }
                if metadata_write::is_write(&request.command)
                    || matches!(
                        request.command,
                        Command::SetSettings { .. }
                            | Command::SetFavorite { .. }
                            | Command::StackScript { .. }
                    )
                {
                    metadata_response::respond(
                        connection,
                        request.command,
                        permit,
                        context,
                        replies,
                        started,
                        id,
                        client,
                        tool,
                    );
                    return;
                }
                if let Command::OpenInViewer { path } = request.command {
                    open_response::respond(
                        connection, path, permit, context, replies, started, id, client,
                    );
                    return;
                }
                work::respond(
                    connection,
                    request.command,
                    granted,
                    permit,
                    context,
                    replies,
                    started,
                    id,
                    tool,
                    client,
                );
            }) {
            Ok(worker) => Some(worker),
            Err(_) => {
                // Spawn failure also completes a tool request. The closure's
                // captured attribution was moved, so use the admission copy.
                self.record(record_client.as_ref(), tool, started, &Err(internal()));
                let _ = fallback.send(Reply {
                    response: Response {
                        id,
                        result: Err(internal()),
                    },
                    policy: ReplyPolicy::Rejected,
                });
                None
            }
        }
    }

    fn record(
        &self,
        client: Option<&ClientRequest>,
        tool: &'static str,
        started: Instant,
        result: &Result<Payload, AgentError>,
    ) {
        // Normal and optional perf logging share the polling emission gate.
        if self
            .handlers
            .lane_log
            .lock()
            .unwrap()
            .request(tool, started, result)
        {
            record_perf(tool, started, result);
        }
        if tool != "hello" && tool != "cancel" {
            if let Some(client) = client.filter(|client| client.kind == RequestKind::Tool) {
                self.handlers
                    .recent_apps
                    .lock()
                    .unwrap()
                    .record(&client.identity);
                // The statement above releases the recent-apps lock before
                // acquiring the installed UI state or requesting a repaint.
                self.handlers.changes.request_ui_repaint();
            }
        }
    }
}

fn record_perf(tool: &'static str, started: Instant, result: &Result<Payload, AgentError>) {
    let (count, error_kind) = result_summary(result);
    if crate::perf::is_enabled() {
        crate::perf::event(
            "agent_ipc",
            tool,
            None,
            0,
            &[
                ("count", serde_json::json!(count)),
                (
                    "duration_ms",
                    serde_json::json!(started.elapsed().as_millis() as u64),
                ),
                ("error", serde_json::json!(error_kind)),
            ],
        );
    }
}
fn request_log_line(
    tool: &'static str,
    started: Instant,
    result: &Result<Payload, AgentError>,
) -> String {
    let (count, error_kind) = result_summary(result);
    let outcome = match result {
        Ok(Payload::Opened(_))
        | Ok(Payload::CollectionWrite(_))
        | Ok(Payload::TagWrite(_))
        | Ok(Payload::RatingWrite(_)) => "accepted",
        Err(_) => "refused",
        _ => "ok",
    };
    format!(
        "agent_ipc: tool={tool} duration_ms={} count={count} error={error_kind} outcome={outcome}",
        started.elapsed().as_millis()
    )
}

fn tool_name(command: &Command) -> &'static str {
    match command {
        Command::Hello { .. } => "hello",
        Command::Status => "miv_status",
        Command::GetViewState => "get_view_state",
        Command::GetSettings { .. } => "get_settings",
        Command::SetSettings { .. } => "set_settings",
        Command::SetFavorite { .. } => "set_favorite",
        Command::StackScript { .. } => "stack_script",
        Command::SearchItems { .. } => "search_items",
        Command::ListTags { .. } => "list_tags",
        Command::ItemsByTag { .. } => "items_by_tag",
        Command::ListCollections { .. } => "list_collections",
        Command::GetCollection { .. } => "get_collection",
        Command::ListLibrary { .. } => "list_library",
        Command::GetItemInfo { .. } => "get_item_info",
        Command::ListFolder { .. } => "list_folder",
        Command::SummarizeFolder { .. } => "summarize_folder",
        Command::GetThumbnail { .. } => "get_thumbnail",
        Command::GetTranscript { .. } => "get_transcript",
        Command::OpenInViewer { .. } => "open_in_viewer",
        Command::CreateCollection { .. } => "create_collection",
        Command::RenameCollection { .. } => "rename_collection",
        Command::AddToCollection { .. } => "add_to_collection",
        Command::RemoveFromCollection { .. } => "remove_from_collection",
        Command::AddTags { .. } => "add_tags",
        Command::RemoveTags { .. } => "remove_tags",
        Command::SetRating { .. } => "set_rating",
        Command::Cancel { .. } => "cancel",
    }
}
fn result_summary(result: &Result<Payload, AgentError>) -> (usize, &'static str) {
    match result {
        Ok(Payload::Items(p)) => (p.items.len(), "none"),
        Ok(Payload::Tags(p)) => (p.tags.len(), "none"),
        Ok(Payload::Collections(p)) => (p.collections.len(), "none"),
        Ok(Payload::Library(p)) => (p.items.len(), "none"),
        Ok(Payload::Info(p)) => (p.items.len(), "none"),
        Ok(Payload::Folder(p)) => (p.items.len(), "none"),
        Ok(Payload::FolderSummary(p)) => (p.total, "none"),
        Ok(Payload::Thumbnails(p)) => (p.page.items.len(), "none"),
        Ok(Payload::Transcript(TranscriptReadResult::Present(page))) => {
            (page.segments.len(), "none")
        }
        Ok(Payload::CollectionWrite(result)) => (
            match result {
                CollectionWriteResult::Created { .. } | CollectionWriteResult::Renamed { .. } => 1,
                CollectionWriteResult::Items { outcomes, .. } => outcomes
                    .iter()
                    .map(|outcome| match outcome {
                        CollectionItemOutcome::Added { indices }
                        | CollectionItemOutcome::Duplicate { indices }
                        | CollectionItemOutcome::Removed { indices }
                        | CollectionItemOutcome::NotPresent { indices }
                        | CollectionItemOutcome::CapacityRejected { indices }
                        | CollectionItemOutcome::Skipped { indices, .. } => indices.len(),
                    })
                    .sum(),
            },
            "none",
        ),
        Ok(Payload::TagWrite(result)) | Ok(Payload::RatingWrite(result)) => (
            result
                .outcomes
                .iter()
                .map(|outcome| match outcome {
                    MetadataItemOutcome::Queued { indices }
                    | MetadataItemOutcome::Changed { indices }
                    | MetadataItemOutcome::Unchanged { indices }
                    | MetadataItemOutcome::Skipped { indices, .. } => indices.len(),
                })
                .sum(),
            "none",
        ),
        Ok(Payload::Opened(_)) => (1, "none"),
        Ok(Payload::ViewState(_)) => (1, "none"),
        Ok(Payload::Settings(page)) => (page.items.len(), "none"),
        Ok(Payload::SettingsWrite(result)) => {
            (result.changed.len() + result.unchanged.len(), "none")
        }
        Ok(Payload::FavoriteWrite(_)) => (1, "none"),
        Ok(Payload::StackScript(_)) => (1, "none"),
        Ok(_) => (0, "none"),
        Err(e) => (
            0,
            match e.code {
                ErrorCode::Disabled => "disabled",
                ErrorCode::PermissionChanged => "permission_changed",
                ErrorCode::NotReady => "not_ready",
                ErrorCode::BadRequest => "bad_request",
                ErrorCode::VersionMismatch => "version_mismatch",
                ErrorCode::Busy => "busy",
                ErrorCode::Cancelled => "cancelled",
                ErrorCode::Timeout => "timeout",
                ErrorCode::Internal => "internal",
                ErrorCode::Conflict => "conflict",
            },
        ),
    }
}

impl Handlers {
    fn execute(
        &self,
        command: Command,
        ctx: &ReadContext,
    ) -> (Result<Payload, AgentError>, Option<JoinHandle<()>>) {
        let cancel = &ctx.cancel;
        if let Command::SearchItems {
            query,
            kind,
            limit,
            cursor,
        } = command
        {
            return self.search(query, kind, limit, cursor, ctx);
        }
        let result = (|| match command {
            // Handshake belongs to Connection, never to a business handler.
            Command::Hello { .. } => {
                Err(error(ErrorCode::BadRequest, "接続の確認は受付で行います"))
            }
            Command::Status => self.status(ctx),
            Command::GetViewState => self.open.execute_view_state(ctx).map(Payload::ViewState),
            Command::GetSettings { query, keys, page } => self
                .open
                .execute_settings_read(query, keys, page, ctx)
                .map(Payload::Settings),
            Command::ListTags {
                choice,
                limit,
                cursor,
            } => {
                let args = serde_json::json!([choice, "tag_menu_order"]);
                let offset = decode_cursor(cursor.as_deref(), "list_tags", &args, ctx.granted.1)?;
                let limit = limit_value(limit)?;
                let tags = self.readers.agent_tags(choice)?;
                let more = tags.len() > offset + limit;
                let (tags, cursor, truncated) = bounded_rows(
                    tags.into_iter().skip(offset).take(limit).collect(),
                    offset,
                    more,
                    "list_tags",
                    args,
                    "tags",
                    ctx.granted.1,
                )?;
                Ok(Payload::Tags(TagPage {
                    tags,
                    cursor,
                    truncated,
                }))
            }
            Command::ItemsByTag {
                tag,
                kind,
                limit,
                cursor,
            } => {
                let tag = tag.trim();
                validate_text(tag)?;
                let args = serde_json::json!([tag, kind, "path"]);
                let offset =
                    decode_cursor(cursor.as_deref(), "items_by_tag", &args, ctx.granted.1)?;
                let limit = limit_value(limit)?;
                let (items, truncated) =
                    self.readers
                        .agent_tag_items(tag, kind, offset + limit + 1, cancel)?;
                page(
                    items,
                    offset,
                    limit,
                    truncated,
                    "items_by_tag",
                    args,
                    ctx.granted.1,
                )
                .map(Payload::Items)
            }
            Command::ListCollections { limit, cursor } => self
                .collections(limit, cursor, ctx)
                .map(Payload::Collections),
            Command::GetCollection {
                id,
                name,
                limit,
                cursor,
            } => self
                .get_collection(id, name, limit, cursor, ctx)
                .map(Payload::Items),
            Command::ListLibrary {
                kind,
                stars,
                max_stars,
                limit,
                cursor,
            } => self
                .library(kind, stars, max_stars, limit, cursor, ctx)
                .map(Payload::Library),
            Command::GetItemInfo {
                items,
                limit,
                cursor,
            } => self.item_info(items, limit, cursor, ctx).map(Payload::Info),
            Command::ListFolder {
                path,
                subresource,
                min_stars,
                max_stars,
                kinds,
                recursive,
                max_depth,
                name_contains,
                extensions,
                tagged,
                limit,
                cursor,
            } => self.list_folder_with_options(
                path,
                subresource,
                min_stars,
                max_stars,
                kinds,
                folder::FolderOptions {
                    recursive,
                    max_depth,
                    name_contains,
                    extensions,
                    tagged,
                },
                limit,
                cursor,
                ctx,
            ),
            Command::SummarizeFolder {
                path,
                subresource,
                min_stars,
                max_stars,
                kinds,
                recursive,
                max_depth,
                name_contains,
                extensions,
                tagged,
            } => self.summarize_folder(
                path,
                subresource,
                min_stars,
                max_stars,
                kinds,
                folder::FolderOptions {
                    recursive,
                    max_depth,
                    name_contains,
                    extensions,
                    tagged,
                },
                ctx,
            ),
            Command::GetThumbnail {
                items,
                long_side,
                cursor,
            } => self.get_thumbnail(items, long_side, cursor, ctx),
            Command::GetTranscript {
                item,
                start_secs,
                cursor,
            } => self
                .get_transcript(item, start_secs, cursor, ctx)
                .map(Payload::Transcript),
            Command::Cancel { .. }
            | Command::SearchItems { .. }
            | Command::OpenInViewer { .. }
            | Command::CreateCollection { .. }
            | Command::RenameCollection { .. }
            | Command::AddToCollection { .. }
            | Command::RemoveFromCollection { .. }
            | Command::AddTags { .. }
            | Command::RemoveTags { .. }
            | Command::SetSettings { .. }
            | Command::SetFavorite { .. }
            | Command::StackScript { .. }
            | Command::SetRating { .. } => {
                unreachable!()
            }
        })();
        (result, None)
    }

    fn collections(
        &self,
        limit: Option<u32>,
        cursor: Option<String>,
        ctx: &ReadContext,
    ) -> Result<CollectionPage, AgentError> {
        let cancel = &ctx.cancel;
        let args = serde_json::json!(["catalog_order"]);
        let offset = decode_cursor(cursor.as_deref(), "list_collections", &args, ctx.granted.1)?;
        let limit = limit_value(limit)?;
        let lease = self
            .producer
            .as_ref()
            .and_then(|p| p.begin_request())
            .ok_or_else(|| error(ErrorCode::NotReady, "コレクションを準備中です"))?;
        let catalog = lease
            .client()
            .list_catalog()
            .map_err(collection_error)?
            .recv()
            .map_err(|_| internal())?
            .map_err(collection_error)?;
        let more = catalog.definitions.len() > offset + limit;
        let mut collections = Vec::new();
        for definition in catalog.definitions.iter().skip(offset).take(limit) {
            if cancel.load(Ordering::Relaxed) {
                return Err(cancelled());
            }
            let snapshot = lease
                .client()
                .load_collection(definition.id)
                .map_err(collection_error)?
                .recv()
                .map_err(|_| internal())?
                .map_err(collection_error)?;
            collections.push(Collection {
                id: definition.id.to_string(),
                name: definition.name.clone(),
                count: snapshot.entries.len() as u64,
            });
        }
        let (collections, cursor, truncated) = bounded_rows(
            collections,
            offset,
            more,
            "list_collections",
            args,
            "collections",
            ctx.granted.1,
        )?;
        Ok(CollectionPage {
            collections,
            cursor,
            truncated,
        })
    }

    fn search(
        &self,
        query: String,
        kind: Option<ItemKind>,
        limit: Option<u32>,
        cursor: Option<String>,
        ctx: &ReadContext,
    ) -> (Result<Payload, AgentError>, Option<JoinHandle<()>>) {
        let cancel = &ctx.cancel;
        let setup = (|| {
            let query = query.trim().to_owned();
            validate_text(&query)?;
            let args = serde_json::json!([query, kind, "relevance"]);
            let offset = decode_cursor(cursor.as_deref(), "search_items", &args, ctx.granted.1)?;
            let limit = limit_value(limit)?;
            let fts = self
                .readers
                .fts
                .read()
                .unwrap()
                .clone()
                .ok_or_else(|| error(ErrorCode::NotReady, "検索索引を準備中です"))?;
            let favorites = self.readers.favorites()?;
            let ids: Vec<_> = favorites
                .iter()
                .filter(|f| f.auto_index_metadata)
                .map(|f| f.id)
                .collect();
            let settings = crate::settings_db::with_db_result(|db| db.load_into_settings())
                .map_err(|_| internal())?;
            let scope = crate::global_search::SearchScope {
                kinds: kind.map(|k| vec![index_kind(k)]),
                target: caption_search_target(ctx.granted.0),
                ignore_epub: settings.epub_file_handling_ignores_epub(),
                ignore_archives: settings.archive_file_handling_ignores_convertible(),
                visibility: settings.file_type_visibility,
                ..Default::default()
            };
            Ok((query, args, offset, limit, fts, ids, scope))
        })();
        let (query, args, offset, limit, fts, ids, scope) = match setup {
            Ok(s) => s,
            Err(e) => return (Err(e), None),
        };
        if kind == Some(ItemKind::Other) {
            return (
                Ok(Payload::Items(ItemPage {
                    items: vec![],
                    cursor: None,
                    truncated: false,
                })),
                None,
            );
        }
        let (tx, rx) = crossbeam_channel::bounded(4);
        let search_cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = search_cancel.clone();
        let now = || {
            #[cfg(test)]
            if let Some(hooks) = &self.search_hooks {
                return (hooks.now)();
            }
            Instant::now()
        };
        // Include worker startup in the response budget. Capture before spawning
        // so both the real worker and a controlled test worker see a fixed deadline.
        let deadline = now() + Duration::from_secs(20);
        #[cfg(test)]
        let hooks = self.search_hooks.clone();
        let worker = match std::thread::Builder::new()
            .name("agent-search".into())
            .spawn(move || {
                #[cfg(test)]
                if let Some(hooks) = hooks {
                    (hooks.worker)(&query, &ids, &cancel_worker, &tx);
                    return;
                }
                crate::global_search::run(&query, &ids, &scope, &fts, &cancel_worker, &tx, None)
            }) {
            Ok(w) => w,
            Err(_) => return (Err(internal()), None),
        };
        let collected = collect_search(&rx, cancel, offset + limit, deadline, &now, |items| {
            if items.len() > offset {
                let page = page(
                    items.to_vec(),
                    offset,
                    limit,
                    true,
                    "search_items",
                    args.clone(),
                    ctx.granted.1,
                )?;
                ctx.publish(Payload::Items(page));
            }
            Ok(())
        });
        search_cancel.store(true, Ordering::Release);
        drop(rx);
        (
            collected.and_then(|(items, truncated)| {
                if truncated && items.len() <= offset && now() >= deadline {
                    return Err(timeout());
                }
                page(
                    items,
                    offset,
                    limit,
                    truncated,
                    "search_items",
                    args,
                    ctx.granted.1,
                )
                .map(Payload::Items)
            }),
            Some(worker),
        )
    }

    fn item_info(
        &self,
        items: Vec<ItemRef>,
        limit: Option<u32>,
        cursor: Option<String>,
        ctx: &ReadContext,
    ) -> Result<InfoPage, AgentError> {
        if items.is_empty() || items.len() > MAX_INFO_LIMIT as usize {
            return Err(error(
                ErrorCode::BadRequest,
                "項目は1～50件で指定してください",
            ));
        }
        let normalized = items
            .iter()
            .map(|item| {
                let path = input_path::cursor_key(&item.path)?;
                Ok(serde_json::json!([
                    path,
                    item.subresource.as_ref().map(|s| s.replace('\\', "/"))
                ]))
            })
            .collect::<Result<Vec<_>, AgentError>>()?;
        let args = serde_json::json!([normalized, "input_order"]);
        let offset = decode_cursor(cursor.as_deref(), "get_item_info", &args, ctx.granted.1)?;
        if offset > items.len() {
            return Err(error(ErrorCode::BadRequest, "最初からやり直してください"));
        }
        let limit = limit_value(limit)?;
        let mut rows = Vec::new();
        for item in items.iter().skip(offset).take(limit) {
            ctx.checkpoint()?;
            rows.push(item_info::read(item, ctx.granted.0, &ctx.cancel)?);
            let (page_rows, cursor, truncated) = bounded_rows(
                rows.clone(),
                offset,
                true,
                "get_item_info",
                args.clone(),
                "items",
                ctx.granted.1,
            )?;
            let count = page_rows.len();
            ctx.publish(Payload::Info(InfoPage {
                items: page_rows,
                cursor,
                truncated,
            }));
            // Stop reading as soon as the page budget is full. Continuation
            // starts at this input offset, never at the start of the array.
            if count < rows.len() {
                rows.truncate(count);
                break;
            }
        }
        ctx.checkpoint()?;
        let more = offset + rows.len() < items.len();
        let (items, cursor, truncated) = bounded_rows(
            rows,
            offset,
            more,
            "get_item_info",
            args,
            "items",
            ctx.granted.1,
        )?;
        Ok(InfoPage {
            items,
            cursor,
            truncated,
        })
    }
}

fn caption_search_target(permission: PermissionSnapshot) -> crate::fts_index::SearchTarget {
    use crate::fts_index::{SearchTarget, SourceKind};
    // SearchScope shares this exact target between candidate lookup and the
    // STORED post-filter. Never fetch a second permission snapshot mid-read.
    SearchTarget::Only(
        SourceKind::ALL
            .iter()
            .copied()
            .filter(|source| permission.ai_captions || *source != SourceKind::AiCaption)
            .filter(|source| permission.transcripts || *source != SourceKind::Transcript)
            .collect(),
    )
}

fn collect_search(
    rx: &crossbeam_channel::Receiver<crate::global_search::SearchStreamEvent>,
    cancel: &AtomicBool,
    wanted: usize,
    deadline: Instant,
    now: impl Fn() -> Instant,
    mut prefix: impl FnMut(&[Item]) -> Result<(), AgentError>,
) -> Result<(Vec<Item>, bool), AgentError> {
    let mut items = Vec::new();
    let mut truncated = false;
    let outcome = loop {
        if cancel.load(Ordering::Acquire) {
            break Err(cancelled());
        }
        if now() >= deadline {
            truncated = true;
            break Ok(());
        }
        match rx.recv_timeout(
            deadline
                .saturating_duration_since(now())
                .min(Duration::from_millis(25)),
        ) {
            Ok(crate::global_search::SearchStreamEvent::Batch { hits, .. }) => {
                items.extend(hits.iter().map(item_from_hit));
                if let Err(error) = prefix(&items) {
                    break Err(error);
                }
                // Fetch only enough to establish whether another page exists.
                if items.len() > wanted {
                    truncated = true;
                    break Ok(());
                }
            }
            Ok(crate::global_search::SearchStreamEvent::Done {
                truncated: capped,
                reason,
            }) => {
                truncated = capped;
                break if matches!(reason, crate::global_search::DoneReason::RejectedQuery(_)) {
                    Err(error(ErrorCode::BadRequest, "検索条件を確認してください"))
                } else {
                    Ok(())
                };
            }
            Ok(crate::global_search::SearchStreamEvent::Error(_)) => break Err(internal()),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(_) => break Err(internal()),
        }
    };
    outcome.map(|_| (items, truncated))
}

fn index_kind(kind: ItemKind) -> crate::fts_index::IndexKind {
    use crate::fts_index::IndexKind as K;
    match kind {
        ItemKind::Image => K::Image,
        ItemKind::Pdf => K::Pdf,
        ItemKind::Video => K::Video,
        ItemKind::Audio => K::Audio,
        ItemKind::Archive => K::Zip,
        ItemKind::Folder | ItemKind::Other => K::Folder,
    }
}
fn item_from_hit(hit: &crate::global_search::GlobalHit) -> Item {
    use crate::fts_index::IndexKind as K;
    let path = hit.path.as_str();
    let (path, member) = path
        .split_once(crate::search_norm::ZIP_ENTRY_SEP)
        .map_or((path, None), |(p, m)| (p, Some(m)));
    let leaf = member.unwrap_or(path);
    let kind = match hit.kind {
        K::Image => ItemKind::Image,
        K::Pdf => ItemKind::Pdf,
        K::Video => ItemKind::Video,
        K::Audio => ItemKind::Audio,
        K::Zip => ItemKind::Archive,
        K::Folder => ItemKind::Folder,
    };
    Item {
        path: path.to_owned(),
        subresource: member.map(str::to_owned),
        name: leaf.rsplit(['/', '\\']).next().unwrap_or(leaf).to_owned(),
        kind,
        tags: if member.is_some() { None } else { Some(vec![]) },
        container_tags: None,
        rating: 0,
    }
}

// For projections without an indexed kind, callers resolve directories from
// their existing metadata/source kind first. This hint performs no filesystem I/O.
fn item_kind_from_extension(extension: &str) -> ItemKind {
    if crate::folder_tree::is_recognized_image_ext(extension) {
        ItemKind::Image
    } else if extension == "pdf" || extension == "epub" {
        ItemKind::Pdf
    } else if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&extension) {
        ItemKind::Video
    } else if crate::folder_tree::is_audio_ext(extension) {
        ItemKind::Audio
    } else if crate::folder_tree::is_zip_extension(extension)
        || crate::archive_converter::ArchiveFormat::from_extension(extension).is_some()
    {
        ItemKind::Archive
    } else {
        ItemKind::Other
    }
}

pub(crate) fn json_chars(value: &impl Serialize) -> usize {
    serde_json::to_string(value).map_or(usize::MAX, |s| s.chars().count())
}
fn validate_text(text: &str) -> Result<(), AgentError> {
    if text.is_empty() || text.chars().count() > 200 {
        Err(error(
            ErrorCode::BadRequest,
            "条件は 1～200 文字で入力してください",
        ))
    } else {
        Ok(())
    }
}
fn limit_value(limit: Option<u32>) -> Result<usize, AgentError> {
    match limit.unwrap_or(DEFAULT_LIMIT) {
        0 => Err(error(
            ErrorCode::BadRequest,
            "件数は 1～200 件で指定してください",
        )),
        n => Ok(n.min(MAX_LIMIT) as usize),
    }
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    version: u32,
    tool: String,
    args_digest: String,
    returned: usize,
    #[serde(default)]
    permission_epoch: u64,
}
fn encode_cursor(
    tool: &str,
    args: serde_json::Value,
    returned: usize,
    permission_epoch: u64,
) -> String {
    // Hex JSON is opaque to clients and needs no dependency or process-local cursor state.
    serde_json::to_vec(&Cursor {
        version: 3,
        tool: tool.into(),
        args_digest: wire_digest(&args).expect("serializable cursor arguments"),
        returned,
        permission_epoch,
    })
    .unwrap()
    .iter()
    .map(|b| format!("{b:02x}"))
    .collect()
}
fn decode_cursor(
    cursor: Option<&str>,
    tool: &str,
    args: &serde_json::Value,
    permission_epoch: u64,
) -> Result<usize, AgentError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let bad = || error(ErrorCode::BadRequest, "最初からやり直してください");
    if cursor.len() > 8192 || !cursor.len().is_multiple_of(2) || !cursor.is_ascii() {
        return Err(bad());
    }
    let bytes = (0..cursor.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&cursor[i..i + 2], 16).map_err(|_| bad()))
        .collect::<Result<Vec<_>, _>>()?;
    let decoded: Cursor = serde_json::from_slice(&bytes).map_err(|_| bad())?;
    if decoded.version != 3
        || decoded.tool != tool
        || decoded.args_digest != wire_digest(args).map_err(|_| bad())?
        || decoded.returned > 100_000
    {
        return Err(bad());
    }
    if decoded.permission_epoch != permission_epoch {
        return Err(error(
            ErrorCode::PermissionChanged,
            "許可が変わりました。最初からやり直してください",
        ));
    }
    Ok(decoded.returned)
}

pub(crate) fn optional_tags(path: &Path) -> Result<Option<crate::tags_db::TagsDb>, AgentError> {
    if !path.try_exists().map_err(|_| internal())? {
        return Ok(None);
    }
    crate::tags_db::TagsDb::open_readonly(path)
        .map(Some)
        .map_err(|_| internal())
}

fn enrich(items: &mut [Item]) -> Result<(), AgentError> {
    let tags = optional_tags(&crate::tags_db::TagsDb::db_path())?;
    let rating_path = crate::rating_db::RatingDb::db_path();
    let ratings = if rating_path.try_exists().map_err(|_| internal())? {
        Some(crate::rating_db::RatingDb::open_readonly(rating_path).map_err(|_| internal())?)
    } else {
        None
    };
    for item in items {
        let logical = item.subresource.as_ref().map_or_else(
            || item.path.clone(),
            |m| format!("{}{}{m}", item.path, crate::search_norm::ZIP_ENTRY_SEP),
        );
        let key = crate::global_search_ui::hit_rating_key(&logical);
        item.rating = ratings.as_ref().map_or(0, |db| db.get(&key));
        let stored_tags = tags.as_ref().map_or_else(Vec::new, |db| {
            db.display_tags_for_item(&crate::tags_db::item_key_for_path(Path::new(&item.path)))
        });
        if item.subresource.is_some() {
            item.tags = None;
            item.container_tags = Some(stored_tags);
        } else {
            item.tags = Some(stored_tags);
            item.container_tags = None;
        }
    }
    Ok(())
}

fn page(
    items: Vec<Item>,
    offset: usize,
    limit: usize,
    source_truncated: bool,
    tool: &str,
    args: serde_json::Value,
    permission_epoch: u64,
) -> Result<ItemPage, AgentError> {
    let total = items.len();
    let mut items = items
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    enrich(&mut items)?;
    let (items, cursor, truncated) = bounded_rows(
        items,
        offset,
        source_truncated || total > offset + limit,
        tool,
        args,
        "items",
        permission_epoch,
    )?;
    Ok(ItemPage {
        items,
        cursor,
        truncated,
    })
}

/// One character/cursor boundary for every paginated projection. The cursor
/// advances by the rows actually returned, including a character-limited page.
fn bounded_rows<T: Serialize>(
    rows: Vec<T>,
    offset: usize,
    mut truncated: bool,
    tool: &str,
    args: serde_json::Value,
    field: &str,
    permission_epoch: u64,
) -> Result<(Vec<PageRow<T>>, Option<String>, bool), AgentError> {
    let mut rows = rows
        .into_iter()
        .map(|row| {
            if mcp_content_chars(&row).map_err(|_| internal())? > MAX_ITEM_CHARS {
                PageRow::too_long(&row).map_err(|_| internal())
            } else {
                Ok(PageRow::Value(row))
            }
        })
        .collect::<Result<Vec<_>, AgentError>>()?;
    loop {
        let cursor = (truncated && !rows.is_empty())
            .then(|| encode_cursor(tool, args.clone(), offset + rows.len(), permission_epoch));
        if mcp_content_chars(
            &serde_json::json!({ (field): &rows, "cursor": &cursor, "truncated": truncated }),
        )
        .map_err(|_| internal())?
            <= MAX_PAGE_CHARS
        {
            return Ok((rows, cursor, truncated));
        }
        rows.pop();
        truncated = true;
        if rows.is_empty() {
            return Err(error(
                ErrorCode::BadRequest,
                "項目が応答の上限を超えています",
            ));
        }
    }
}

impl crate::app::App {
    pub(crate) fn install_agent_readers(&mut self, readers: Arc<crate::remote_ipc::SharedReaders>) {
        if let Some(service) = &self.book_composition_service {
            readers.install_book_composition(service.clone());
        }
        self.agent_readers = Some(readers);
        self.publish_agent_index();
    }
    pub(crate) fn publish_agent_index(&self) {
        if let Some(readers) = &self.agent_readers {
            *readers.fts.write().unwrap() = self
                .indexer_init
                .as_ref()
                .map(|manager| manager.clone_fts_index());
        }
    }
}
