//! Read-only agent lane. No display state, UI dispatch, or metadata writers.
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mimageviewer_agent_ipc::*;
use serde::{Deserialize, Serialize};
mod folder;
mod item_info;
mod library;
mod thumbnail;
mod work;
use work::*;

#[cfg(test)]
mod media_read_tests;
#[cfg(windows)]
mod pipe;
#[cfg(test)]
mod read_tests;
#[cfg(test)]
mod tests;

pub(crate) struct Server {
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
            Ok(Self {
                pipe: pipe::Server::start(Arc::new(Handlers {
                    readers,
                    producer,
                    session,
                    active: AtomicUsize::new(0),
                    #[cfg(test)]
                    search_hooks: None,
                    #[cfg(test)]
                    read_hooks: None,
                }))?,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = (readers, producer, session);
            Err(std::io::Error::other("Windows only"))
        }
    }
}

struct Handlers {
    readers: Arc<crate::remote_ipc::SharedReaders>,
    producer: Option<crate::collection_store::CollectionRemoteProducerControl>,
    session: Option<crate::remote_ipc::session::SessionHandle>,
    active: AtomicUsize,
    #[cfg(test)]
    search_hooks: Option<Arc<SearchHooks>>,
    #[cfg(test)]
    read_hooks: Option<Arc<ReadHooks>>,
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
enum ReplyPolicy {
    Read(PermissionGrant),
    Rejected,
    // Future accepted operations retain their result after the acceptance point.
    #[allow(dead_code)]
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
    hello: AtomicBool,
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
    let (snapshot, epoch) = crate::settings_db::with_db_result(|db| db.load_agent_permission())
        .map_err(|_| error(ErrorCode::Busy, "設定を読み込めませんでした"))?;
    if snapshot.level == AgentLevel::Disabled {
        Err(error(ErrorCode::Disabled, "AI 連携 (MCP) は無効です"))
    } else {
        Ok((snapshot, epoch))
    }
}

impl Connection {
    fn new(handlers: Arc<Handlers>) -> Arc<Self> {
        Arc::new(Self {
            handlers,
            running: Mutex::new(None),
            hello: AtomicBool::new(false),
        })
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
        let reject = |err| {
            let result = Err(err);
            record(tool, started, &result);
            let _ = replies.send(Reply {
                response: Response {
                    id: request.id,
                    result,
                },
                policy: ReplyPolicy::Rejected,
            });
        };
        let granted = match permission() {
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
            record(tool, started, &result);
            let _ = replies.send(Reply {
                response: Response {
                    id: request.id,
                    result,
                },
                policy: ReplyPolicy::Read(granted),
            });
            return None;
        }
        if !self.hello.load(Ordering::Acquire) && !matches!(request.command, Command::Hello { .. })
        {
            reject(error(
                ErrorCode::BadRequest,
                "先に接続の版を確認してください",
            ));
            return None;
        }
        if matches!(request.command, Command::GetThumbnail { .. }) && !granted.0.thumbnails {
            reject(error(
                ErrorCode::Disabled,
                "AI に縮小画像を見せる設定は無効です",
            ));
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
        let context = ReadContext::new(&self.handlers, granted.0, cancel);
        match std::thread::Builder::new()
            .name("agent-read".into())
            .spawn(move || {
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
                );
            }) {
            Ok(worker) => Some(worker),
            Err(_) => {
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
}

fn record(tool: &'static str, started: Instant, result: &Result<Payload, AgentError>) {
    let (count, error_kind) = result_summary(result);
    crate::logger::log(format!(
        "agent_ipc: tool={tool} duration_ms={} count={count} error={error_kind}",
        started.elapsed().as_millis()
    ));
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

fn tool_name(command: &Command) -> &'static str {
    match command {
        Command::Hello { .. } => "hello",
        Command::Status => "miv_status",
        Command::SearchItems { .. } => "search_items",
        Command::ListTags { .. } => "list_tags",
        Command::ItemsByTag { .. } => "items_by_tag",
        Command::ListCollections { .. } => "list_collections",
        Command::GetCollection { .. } => "get_collection",
        Command::ListLibrary { .. } => "list_library",
        Command::GetItemInfo { .. } => "get_item_info",
        Command::ListFolder { .. } => "list_folder",
        Command::GetThumbnail { .. } => "get_thumbnail",
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
        Ok(Payload::Thumbnails(p)) => (p.page.items.len(), "none"),
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
            Command::Hello { version, .. } => {
                check_version(version).map(|_| Payload::Hello { version })
            }
            Command::Status => Ok(Payload::Status(StatusPayload {
                app_version: env!("CARGO_PKG_VERSION").into(),
                permission: ctx.permission,
                fts_ready: self.readers.fts.read().unwrap().is_some(),
                remote_active: self.session.as_ref().is_some_and(|s| {
                    s.snapshot().phase != crate::remote_ipc::session::RemoteControlPhase::Local
                }),
            })),
            Command::ListTags {
                choice,
                limit,
                cursor,
            } => {
                let args = serde_json::json!([choice, "tag_menu_order"]);
                let offset = decode_cursor(cursor.as_deref(), "list_tags", &args)?;
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
                let offset = decode_cursor(cursor.as_deref(), "items_by_tag", &args)?;
                let limit = limit_value(limit)?;
                let (items, truncated) =
                    self.readers
                        .agent_tag_items(tag, kind, offset + limit + 1, cancel)?;
                page(items, offset, limit, truncated, "items_by_tag", args).map(Payload::Items)
            }
            Command::ListCollections { limit, cursor } => self
                .collections(limit, cursor, cancel)
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
                limit,
                cursor,
            } => self
                .library(kind, stars, limit, cursor, ctx)
                .map(Payload::Library),
            Command::GetItemInfo {
                items,
                limit,
                cursor,
            } => self.item_info(items, limit, cursor, ctx).map(Payload::Info),
            Command::ListFolder {
                path,
                subresource,
                limit,
                cursor,
            } => self.list_folder(path, subresource, limit, cursor, ctx),
            Command::GetThumbnail {
                items,
                long_side,
                cursor,
            } => self.get_thumbnail(items, long_side, cursor, ctx),
            Command::Cancel { .. } | Command::SearchItems { .. } => unreachable!(),
        })();
        (result, None)
    }

    fn collections(
        &self,
        limit: Option<u32>,
        cursor: Option<String>,
        cancel: &AtomicBool,
    ) -> Result<CollectionPage, AgentError> {
        let args = serde_json::json!(["catalog_order"]);
        let offset = decode_cursor(cursor.as_deref(), "list_collections", &args)?;
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
            let offset = decode_cursor(cursor.as_deref(), "search_items", &args)?;
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
                ignore_epub: settings.epub_file_handling_ignores_epub(),
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
                page(items, offset, limit, truncated, "search_items", args).map(Payload::Items)
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
                let path = resolve_data_dir(Path::new(&item.path))
                    .map_err(|_| error(ErrorCode::BadRequest, "絶対パスを指定してください"))?;
                Ok(serde_json::json!([
                    path.to_string_lossy().replace('/', "\\").to_lowercase(),
                    item.subresource.as_ref().map(|s| s.replace('\\', "/"))
                ]))
            })
            .collect::<Result<Vec<_>, AgentError>>()?;
        let args = serde_json::json!([normalized, "input_order"]);
        let offset = decode_cursor(cursor.as_deref(), "get_item_info", &args)?;
        if offset > items.len() {
            return Err(error(ErrorCode::BadRequest, "最初からやり直してください"));
        }
        let limit = limit_value(limit)?;
        let mut rows = Vec::new();
        for item in items.iter().skip(offset).take(limit) {
            ctx.checkpoint()?;
            rows.push(item_info::read(item, ctx.permission, &ctx.cancel)?);
            let (page_rows, cursor, truncated) = bounded_rows(
                rows.clone(),
                offset,
                true,
                "get_item_info",
                args.clone(),
                "items",
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
        let (items, cursor, truncated) =
            bounded_rows(rows, offset, more, "get_item_info", args, "items")?;
        Ok(InfoPage {
            items,
            cursor,
            truncated,
        })
    }
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
                items.extend(hits.into_iter().map(|h| item_from_hit(&h.path)));
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
fn item_from_hit(path: &str) -> Item {
    let (path, member) = path
        .split_once(crate::search_norm::ZIP_ENTRY_SEP)
        .map_or((path, None), |(p, m)| (p, Some(m)));
    let leaf = member.unwrap_or(path);
    let extension = leaf.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let kind = if member.is_some() || crate::folder_tree::is_recognized_image_ext(&extension) {
        ItemKind::Image
    } else if extension == "pdf" || extension == "epub" {
        ItemKind::Pdf
    } else if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&extension.as_str()) {
        ItemKind::Video
    } else if crate::folder_tree::is_audio_ext(&extension) {
        ItemKind::Audio
    } else if crate::folder_tree::is_zip_extension(&extension) {
        ItemKind::Archive
    } else if Path::new(path).is_dir() {
        ItemKind::Folder
    } else {
        ItemKind::Other
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
}
fn encode_cursor(tool: &str, args: serde_json::Value, returned: usize) -> String {
    // Hex JSON is opaque to clients and needs no dependency or process-local cursor state.
    serde_json::to_vec(&Cursor {
        version: 2,
        tool: tool.into(),
        args_digest: wire_digest(&args).expect("serializable cursor arguments"),
        returned,
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
    if decoded.version != 2
        || decoded.tool != tool
        || decoded.args_digest != wire_digest(args).map_err(|_| bad())?
        || decoded.returned > 100_000
    {
        return Err(bad());
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
            .then(|| encode_cursor(tool, args.clone(), offset + rows.len()));
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
