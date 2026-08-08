use std::collections::{HashMap, VecDeque};
#[cfg(not(feature = "embedded-web-assets"))]
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mimageviewer_ipc::{
    CollectionErrorCode, CollectionKind, FavoriteSearchIndexState, FavoriteSearchKind,
    FavoriteSearchRequest, MediaErrorCode, PagePriority, RemoteAddress, RemoteAiJobError,
    RemoteAiJobErrorCode, RemoteAiStartRequest, RemoteEntryKind, RemotePageRenderContext,
    RemoteReadingDirection, RemoteSessionIdentity, RemoteSpreadMode, RemoteSubresource,
    RemoteWriteErrorCode, RemoteWriteRequest, SessionResponse, SessionStatus, TagIndexState,
    TagItemKind, TagItemsRequest, VideoStreamControlAction, VideoStreamErrorCode,
    VideoStreamJumpThumbnailPayload, VideoStreamPlaylistKind, VideoStreamQuality,
    VideoStreamSegmentIndex, VideoStreamSegmentPayload, VideoStreamThumbnailPayload,
};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use serde::Deserialize;
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, StatusCode};
use uuid::Uuid;

use crate::auth::{
    AuthDecision, AuthInput, AuthService, AuthSessionIdentity, AuthSource, PinVerification,
};
use crate::diagnostics::{DiagnosticsLogger, RequestLog};
use crate::ipc_client::{ClientError as IpcClientError, ThumbnailClient};
use crate::store::{Library, StoreError};

const MAX_TELEMETRY_BODY_BYTES: usize = 64 * 1024;
const MAX_PIN_BODY_BYTES: usize = 4 * 1024;
const MAX_WRITE_BODY_BYTES: usize = 16 * 1024;
const MAX_VIDEO_BODY_BYTES: usize = 16 * 1024;
const MAX_AI_JOB_BODY_BYTES: usize = 32 * 1024;
const MAX_TELEMETRY_EVENTS: usize = 128;
const TELEMETRY_REQUESTS_PER_WINDOW: usize = 30;
const TELEMETRY_WINDOW: Duration = Duration::from_secs(60);
pub const HTTP_WORKER_COUNT: usize = 12;
pub const MAX_CONCURRENT_IPC: usize = 6;
pub const MAX_CONCURRENT_HEAVY_IPC: usize = 4;
pub const MAX_CONCURRENT_PAGE_PREFETCH: usize = 1;
pub const MAX_CONCURRENT_STREAM_IPC: usize = 4;
const IPC_RETRY_AFTER_SECONDS: u64 = 1;

include!(concat!(env!("OUT_DIR"), "/embedded_web_assets.rs"));

pub struct AppState {
    pub auth: AuthService,
    pub library: Library,
    pub thumbnail_client: Arc<ThumbnailClient>,
    pub session_activity: SessionActivityNotifier,
    pub ipc_admission: IpcAdmission,
    pub logger: DiagnosticsLogger,
    pub telemetry_limiter: TelemetryLimiter,
    pub request_sequence: AtomicU64,
    pub web_root: PathBuf,
    pub session_peers: Mutex<HashMap<std::net::IpAddr, mimageviewer_ipc::SessionPeerInfo>>,
    pub remote_client_identities: RemoteClientIdentities,
}

#[derive(Default)]
pub struct RemoteClientIdentities {
    cookie_owners: Mutex<HashMap<AuthSessionIdentity, RemoteCookieOwner>>,
    unidentified_sequence: AtomicU64,
}

#[derive(Clone)]
struct RemoteCookieOwner {
    client_id: String,
    session_id: Option<String>,
}

impl RemoteClientIdentities {
    fn resolve(&self, request: &Request, auth: AuthDecision) -> String {
        let header_client_id = remote_client_header(request);
        match auth {
            AuthDecision::Authorized(AuthSource::SessionCookie(identity)) => {
                let Ok(mut owners) = self.cookie_owners.lock() else {
                    return identity.fallback_client_id();
                };
                if let Some(owner) = owners.get(&identity) {
                    return owner.client_id.clone();
                }
                let owner = header_client_id
                    .map(str::to_owned)
                    .unwrap_or_else(|| identity.fallback_client_id());
                owners.insert(
                    identity,
                    RemoteCookieOwner {
                        client_id: owner.clone(),
                        session_id: None,
                    },
                );
                owner
            }
            AuthDecision::Authorized(AuthSource::Bearer) => {
                header_client_id.map(str::to_owned).unwrap_or_else(|| {
                    format!(
                        "bearer-unidentified-{}",
                        self.unidentified_sequence.fetch_add(1, Ordering::Relaxed)
                    )
                })
            }
            AuthDecision::Unauthorized => {
                unreachable!("unauthorized requests are rejected before client identity resolution")
            }
        }
    }

    fn bind_session(&self, auth: AuthDecision, owner: &RemoteSessionIdentity) {
        let AuthDecision::Authorized(AuthSource::SessionCookie(identity)) = auth else {
            return;
        };
        if let Ok(mut owners) = self.cookie_owners.lock() {
            owners.insert(
                identity,
                RemoteCookieOwner {
                    client_id: owner.client_id.clone(),
                    session_id: Some(owner.session_id.clone()),
                },
            );
        }
    }

    fn resolve_session(
        &self,
        request: &Request,
        auth: AuthDecision,
        client_id: &str,
    ) -> Option<RemoteSessionIdentity> {
        if let Some(session_id) = remote_session_header(request) {
            return Some(RemoteSessionIdentity {
                client_id: client_id.to_owned(),
                session_id: session_id.to_owned(),
            });
        }
        let AuthDecision::Authorized(AuthSource::SessionCookie(identity)) = auth else {
            return None;
        };
        let owners = self.cookie_owners.lock().ok()?;
        let owner = owners.get(&identity)?;
        (owner.client_id == client_id)
            .then(|| owner.session_id.clone())
            .flatten()
            .map(|session_id| RemoteSessionIdentity {
                client_id: client_id.to_owned(),
                session_id,
            })
    }
}

pub struct SessionActivityNotifier {
    tx: std::sync::mpsc::SyncSender<RemoteSessionIdentity>,
}

impl SessionActivityNotifier {
    pub fn start(client: Arc<ThumbnailClient>) -> Result<Self, String> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<RemoteSessionIdentity>(1);
        std::thread::Builder::new()
            .name("remote-session-activity".to_owned())
            .spawn(move || {
                while let Ok(owner) = rx.recv() {
                    let _ = client.session_activity(&owner);
                }
            })
            .map_err(|error| format!("session activity worker を開始できません: {error}"))?;
        Ok(Self { tx })
    }

    fn note(&self, owner: &RemoteSessionIdentity) {
        // Cheap HTTP routes must never wait for IPC. One pending notification is enough because
        // all notifications mean the same monotonic "active now" transition.
        let _ = self.tx.try_send(owner.clone());
    }
}

pub struct IpcAdmission {
    all: TrySemaphore,
    heavy: TrySemaphore,
    prefetch: TrySemaphore,
    stream: TrySemaphore,
}

#[derive(Clone, Copy)]
enum IpcClass {
    Browse,
    Home,
    Heavy,
    Prefetch,
    Stream,
}

#[derive(Clone, Copy, Debug)]
struct AdmissionBusy {
    all_in_flight: usize,
    all_limit: usize,
    heavy_in_flight: usize,
    heavy_limit: usize,
    prefetch_in_flight: usize,
    prefetch_limit: usize,
    stream_in_flight: usize,
    stream_limit: usize,
}

struct TrySemaphore {
    in_flight: AtomicUsize,
    limit: usize,
}

struct TryPermit<'a> {
    semaphore: &'a TrySemaphore,
}

struct IpcPermit<'a> {
    _all: Option<TryPermit<'a>>,
    _heavy: Option<TryPermit<'a>>,
    _prefetch: Option<TryPermit<'a>>,
    _stream: Option<TryPermit<'a>>,
}

impl IpcAdmission {
    pub fn new() -> Self {
        assert!(MAX_CONCURRENT_HEAVY_IPC < MAX_CONCURRENT_IPC);
        assert!(MAX_CONCURRENT_IPC < HTTP_WORKER_COUNT);
        assert!(MAX_CONCURRENT_IPC + MAX_CONCURRENT_STREAM_IPC < HTTP_WORKER_COUNT);
        Self {
            all: TrySemaphore::new(MAX_CONCURRENT_IPC),
            heavy: TrySemaphore::new(MAX_CONCURRENT_HEAVY_IPC),
            prefetch: TrySemaphore::new(MAX_CONCURRENT_PAGE_PREFETCH),
            stream: TrySemaphore::new(MAX_CONCURRENT_STREAM_IPC),
        }
    }

    fn try_enter(&self, class: IpcClass) -> Result<IpcPermit<'_>, AdmissionBusy> {
        if matches!(class, IpcClass::Stream) {
            return self
                .stream
                .try_acquire()
                .map(|stream| IpcPermit {
                    _all: None,
                    _heavy: None,
                    _prefetch: None,
                    _stream: Some(stream),
                })
                .ok_or_else(|| self.busy());
        }
        let prefetch = if matches!(class, IpcClass::Prefetch) {
            Some(self.prefetch.try_acquire().ok_or_else(|| self.busy())?)
        } else {
            None
        };
        // 先読みは all/heavy の最終 1 枠を使用しない。表示要求が queue 待ちではなく
        // admission を即取得できる余地を remote-web 側でも固定する。
        let all_limit = if matches!(class, IpcClass::Prefetch) {
            self.all.limit - 1
        } else {
            self.all.limit
        };
        let all = match self.all.try_acquire_below(all_limit) {
            Some(permit) => permit,
            None => {
                drop(prefetch);
                return Err(self.busy());
            }
        };
        let heavy = if matches!(class, IpcClass::Heavy | IpcClass::Prefetch) {
            let heavy_limit = if matches!(class, IpcClass::Prefetch) {
                self.heavy.limit - 1
            } else {
                self.heavy.limit
            };
            match self.heavy.try_acquire_below(heavy_limit) {
                Some(permit) => Some(permit),
                None => {
                    drop(all);
                    drop(prefetch);
                    return Err(self.busy());
                }
            }
        } else {
            None
        };
        Ok(IpcPermit {
            _all: Some(all),
            _heavy: heavy,
            _prefetch: prefetch,
            _stream: None,
        })
    }

    fn run<T>(&self, class: IpcClass, operation: impl FnOnce() -> T) -> Result<T, AdmissionBusy> {
        let _permit = self.try_enter(class)?;
        Ok(operation())
    }

    fn busy(&self) -> AdmissionBusy {
        AdmissionBusy {
            all_in_flight: self.all.in_flight(),
            all_limit: self.all.limit,
            heavy_in_flight: self.heavy.in_flight(),
            heavy_limit: self.heavy.limit,
            prefetch_in_flight: self.prefetch.in_flight(),
            prefetch_limit: self.prefetch.limit,
            stream_in_flight: self.stream.in_flight(),
            stream_limit: self.stream.limit,
        }
    }
}

impl TrySemaphore {
    fn new(limit: usize) -> Self {
        assert!(limit > 0);
        Self {
            in_flight: AtomicUsize::new(0),
            limit,
        }
    }

    fn try_acquire(&self) -> Option<TryPermit<'_>> {
        self.try_acquire_below(self.limit)
    }

    fn try_acquire_below(&self, limit: usize) -> Option<TryPermit<'_>> {
        let mut current = self.in_flight.load(Ordering::Acquire);
        loop {
            if current >= limit {
                return None;
            }
            match self.in_flight.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(TryPermit { semaphore: self }),
                Err(observed) => current = observed,
            }
        }
    }

    fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }
}

impl Drop for TryPermit<'_> {
    fn drop(&mut self) {
        self.semaphore.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct TelemetryLimiter {
    accepted: Mutex<VecDeque<Instant>>,
}

impl TelemetryLimiter {
    pub fn new() -> Self {
        Self {
            accepted: Mutex::new(VecDeque::new()),
        }
    }

    fn allow(&self, now: Instant) -> bool {
        let Ok(mut accepted) = self.accepted.lock() else {
            return false;
        };
        while accepted
            .front()
            .is_some_and(|instant| now.duration_since(*instant) >= TELEMETRY_WINDOW)
        {
            accepted.pop_front();
        }
        if accepted.len() >= TELEMETRY_REQUESTS_PER_WINDOW {
            return false;
        }
        accepted.push_back(now);
        true
    }
}

struct HttpResponse {
    status: u16,
    content_type: &'static str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
    log_details: Option<Value>,
    sensitive_values: Vec<String>,
}

impl HttpResponse {
    fn bytes(status: u16, content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type,
            headers: Vec::new(),
            body,
            log_details: None,
            sensitive_values: Vec::new(),
        }
    }

    fn text(status: u16, body: &'static str) -> Self {
        Self::bytes(
            status,
            "text/plain; charset=utf-8",
            body.as_bytes().to_vec(),
        )
    }

    fn json<T: serde::Serialize>(value: &T) -> Result<Self, serde_json::Error> {
        Ok(Self::bytes(
            200,
            "application/json; charset=utf-8",
            serde_json::to_vec(value)?,
        ))
    }

    fn with_header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    fn with_log_details(mut self, details: Value) -> Self {
        self.log_details = Some(details);
        self
    }

    fn with_body_error_code_log(mut self) -> Self {
        let error_code = serde_json::from_slice::<Value>(&self.body)
            .ok()
            .and_then(|body| body.get("error").and_then(Value::as_str).map(str::to_owned));
        let Some(error_code) = error_code else {
            return self;
        };
        let details = self.log_details.get_or_insert_with(|| json!({}));
        if !details.is_object() {
            *details = json!({});
        }
        let root = details.as_object_mut().expect("log details object");
        let video = root.entry("video_stream").or_insert_with(|| json!({}));
        if !video.is_object() {
            *video = json!({});
        }
        video
            .as_object_mut()
            .expect("video stream log details object")
            .insert("error_code".to_owned(), Value::String(error_code));
        self
    }

    fn with_sensitive_value(mut self, value: impl Into<String>) -> Self {
        self.sensitive_values.push(value.into());
        self
    }
}

pub fn handle(mut request: Request, state: &Arc<AppState>) {
    let request_id = state.request_sequence.fetch_add(1, Ordering::Relaxed);
    let started_at = Instant::now();
    let timestamp_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let method = request.method().to_string();
    let raw_url = request.url().to_owned();
    let proxy_details = request_proxy_details(&request);
    let mut response = route(&mut request, state);
    response
        .headers
        .push(("X-mIV-Request-Id", request_id.to_string()));
    let status = response.status;
    let response_bytes = response.body.len();
    let mut details = response.log_details.take().unwrap_or_else(|| json!({}));
    details["proxy"] = proxy_details;
    let sensitive_values = std::mem::take(&mut response.sensitive_values);
    let response_result = respond(request, response);
    state.logger.log_request(RequestLog {
        request_id,
        timestamp_unix_ms,
        method: &method,
        raw_url: &raw_url,
        status,
        duration: started_at.elapsed(),
        response_bytes,
        response_write_ok: response_result.is_ok(),
        details: Some(details),
        sensitive_values,
    });
    if let Err(error) = response_result {
        eprintln!("remote-web: response write failed: {error}");
    }
}

fn route(request: &mut Request, state: &AppState) -> HttpResponse {
    let (path, raw_query) = split_url(request.url());
    let video_route = path.starts_with("/api/video/") || path.starts_with("/stream/");
    let query_result = parse_query(raw_query);
    let query = match query_result {
        Ok(query) => query,
        Err(()) => {
            return HttpResponse::text(400, "Bad Request").with_header("Cache-Control", "no-store");
        }
    };

    let method = request.method().clone();
    let auth = state.auth.authorize(AuthInput {
        authorization: header_value(request, "Authorization"),
        cookie: header_value(request, "Cookie"),
    });
    let remote_client_id = if auth != AuthDecision::Unauthorized
        && (path.starts_with("/api/") || path.starts_with("/stream/"))
        && !path.starts_with("/api/auth/")
    {
        state.remote_client_identities.resolve(request, auth)
    } else {
        String::new()
    };
    let remote_owner =
        state
            .remote_client_identities
            .resolve_session(request, auth, &remote_client_id);
    if auth != AuthDecision::Unauthorized
        && route_requires_remote_session(path)
        && remote_owner.is_none()
    {
        return session_response_http(
            SessionResponse {
                status: SessionStatus::NotAcquired,
                message: "リモートセッションを取得してください。".to_owned(),
                session_id: None,
            },
            state,
        );
    }
    let remote_owner = remote_owner.as_ref();
    let web_asset = web_asset_route_name(path);
    let response = match (method, path) {
        (Method::Get, "/" | "/index.html") => static_index(state),
        (Method::Get, "/apple-touch-icon.png" | "/apple-touch-icon-precomposed.png") => {
            static_file(state, "icons/icon-180.png")
        }
        (Method::Get, "/favicon.ico") => HttpResponse::bytes(204, "image/x-icon", Vec::new()),
        (Method::Get, _) if web_asset.is_some() => static_file(
            state,
            web_asset.expect("match guard checked the generated manifest"),
        ),
        (Method::Get, "/api/auth/status") => api_auth_status(state, auth),
        (Method::Post, "/api/auth/pin") => api_auth_pin(request, state),
        (_, "/api/auth/status" | "/api/auth/pin") => {
            HttpResponse::text(405, "Method Not Allowed").with_header("Cache-Control", "no-store")
        }
        _ if auth == AuthDecision::Unauthorized => unauthorized(),
        (Method::Get, "/api/app-version") => api_app_version(state),
        (Method::Post, "/api/video/start") => api_video_start(
            request,
            state,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Post, "/api/video/control") => api_video_control(
            request,
            state,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Post, "/api/video/seek") => api_video_seek(
            request,
            state,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Post, "/api/video/thumbnail") => api_video_thumbnail(
            request,
            state,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Get, "/api/video/state") => api_video_state(
            state,
            &query,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Get, "/api/video/jumps") => api_video_jumps(
            state,
            &query,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Get, "/api/video/jump-thumbnail") => api_video_jump_thumbnail(
            state,
            &query,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Post, "/api/video/stop") => api_video_stop(
            request,
            state,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Get, path) if path.starts_with("/stream/") => stream_resource(
            state,
            path,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Post, "/api/session/acquire") => {
            api_session_acquire(request, state, auth, &remote_client_id)
        }
        (Method::Post, "/api/session/ping") => api_session_ping(
            request,
            state,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Get, "/api/remote-state") => with_session_activity(
            state,
            remote_owner.expect("route guard checked session"),
            || api_remote_state(state),
        ),
        (Method::Get, "/api/favorites") => with_session_activity(
            state,
            remote_owner.expect("route guard checked session"),
            || api_favorites(state),
        ),
        (Method::Get, "/api/home") => {
            api_home(state, remote_owner.expect("route guard checked session"))
        }
        (Method::Get, "/api/collection") => api_collection(
            state,
            &query,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Get, "/api/search/favorites") => api_favorite_search(
            state,
            &query,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Get, "/api/tags") => {
            api_tag_browse(state, remote_owner.expect("route guard checked session"))
        }
        (Method::Get, "/api/tags/items") => api_tag_items(
            state,
            &query,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Get, "/api/list") => with_session_activity(
            state,
            remote_owner.expect("route guard checked session"),
            || {
                api_list(
                    state,
                    &query,
                    remote_owner.expect("route guard checked session"),
                )
            },
        ),
        (Method::Get, "/api/container") => api_container(
            state,
            &query,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Post, "/api/ai/jobs") => api_ai_job_start(
            request,
            state,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Get, "/api/ai/jobs") => api_ai_jobs_recoverable(
            state,
            &query,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Get, path) if ai_result_job_id(path).is_some() => api_ai_job_result(
            state,
            ai_result_job_id(path).expect("guard checked result route"),
            &query,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Get, path) if ai_state_job_id(path).is_some() => api_ai_job_state(
            state,
            ai_state_job_id(path).expect("guard checked state route"),
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Delete, path) if ai_state_job_id(path).is_some() => api_ai_job_cancel(
            state,
            ai_state_job_id(path).expect("guard checked cancel route"),
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Post, "/api/write") => api_write(
            request,
            state,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Get, "/api/thumb") => api_thumb(
            state,
            &query,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Get, "/api/image-info") => with_session_activity(
            state,
            remote_owner.expect("route guard checked session"),
            || api_image_info(state, &query),
        ),
        (Method::Get, "/api/image") => with_session_activity(
            state,
            remote_owner.expect("route guard checked session"),
            || api_image(state, &query),
        ),
        (Method::Get, "/api/page") => api_page(
            state,
            &query,
            remote_owner.expect("route guard checked session"),
        ),
        (Method::Post, "/api/telemetry") => api_telemetry(request, state),
        (Method::Get, _) => HttpResponse::text(404, "Not Found"),
        (_, path) if path.starts_with("/stream/") => HttpResponse::text(405, "Method Not Allowed")
            .with_header("Allow", "GET")
            .with_header("Cache-Control", "no-store"),
        (_, "/api/ai/jobs") => HttpResponse::text(405, "Method Not Allowed")
            .with_header("Allow", "GET, POST")
            .with_header("Cache-Control", "no-store"),
        (_, path) if path.starts_with("/api/ai/jobs/") => {
            HttpResponse::text(405, "Method Not Allowed")
                .with_header("Allow", "GET, DELETE")
                .with_header("Cache-Control", "no-store")
        }
        (
            _,
            "/api/telemetry"
            | "/api/session/acquire"
            | "/api/session/ping"
            | "/api/write"
            | "/api/video/start"
            | "/api/video/control"
            | "/api/video/seek"
            | "/api/video/thumbnail"
            | "/api/video/stop",
        ) => HttpResponse::text(405, "Method Not Allowed")
            .with_header("Allow", "POST")
            .with_header("Cache-Control", "no-store"),
        _ => HttpResponse::text(405, "Method Not Allowed")
            .with_header("Allow", "GET")
            .with_header("Cache-Control", "no-store"),
    };

    let response = if video_route {
        response.with_body_error_code_log()
    } else {
        response
    };
    response
        .with_header("X-Content-Type-Options", "nosniff")
        .with_header("Referrer-Policy", "no-referrer")
}

fn ai_state_job_id(path: &str) -> Option<&str> {
    let job_id = path.strip_prefix("/api/ai/jobs/")?;
    (!job_id.is_empty() && !job_id.contains('/')).then_some(job_id)
}

fn ai_result_job_id(path: &str) -> Option<&str> {
    let job_id = path
        .strip_prefix("/api/ai/jobs/")?
        .strip_suffix("/result")?;
    (!job_id.is_empty() && !job_id.contains('/')).then_some(job_id)
}

fn api_ai_job_start(
    request: &mut Request,
    state: &AppState,
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let body = match read_body_limited(request, MAX_AI_JOB_BODY_BYTES) {
        Ok(body) => body,
        Err(BodyReadError::TooLarge) => return HttpResponse::text(413, "Payload Too Large"),
        Err(BodyReadError::Read) => return HttpResponse::text(400, "Bad Request"),
    };
    let request: RemoteAiStartRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return HttpResponse::text(400, "Bad Request"),
    };
    for page in &request.pages {
        if let Err(error) = state.library.validate_remote_address(&page.address) {
            return store_error_response(error).with_header("Cache-Control", "no-store");
        }
        if let Some(render_context) = page.render_context.as_ref()
            && let Err(error) = state
                .library
                .validate_remote_address(&render_context.context_address)
        {
            return store_error_response(error).with_header("Cache-Control", "no-store");
        }
        if let Some(spread_partner) = page
            .render_context
            .as_ref()
            .and_then(|context| context.spread_partner.as_ref())
            && let Err(error) = state.library.validate_remote_address(spread_partner)
        {
            return store_error_response(error).with_header("Cache-Control", "no-store");
        }
    }
    let result = match state.ipc_admission.run(IpcClass::Home, || {
        state.thumbnail_client.remote_ai_start(owner, request)
    }) {
        Ok(result) => result,
        Err(_) => return ai_admission_busy_response(),
    };
    match result {
        Ok(success) => {
            let mut response = HttpResponse::json(&success.value)
                .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"));
            response.status = 202;
            response.with_header("Cache-Control", "no-store")
        }
        Err(failure) => ai_ipc_error_response(failure),
    }
}

fn api_ai_jobs_recoverable(
    state: &AppState,
    query: &[(String, String)],
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    if query_value(query, "recoverable") != Ok(Some("1")) {
        return HttpResponse::text(400, "Bad Request");
    }
    let result = match state.ipc_admission.run(IpcClass::Home, || {
        state.thumbnail_client.remote_ai_recoverable(owner)
    }) {
        Ok(result) => result,
        Err(_) => return ai_admission_busy_response(),
    };
    match result {
        Ok(success) => HttpResponse::json(&success.value)
            .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
            .with_header("Cache-Control", "no-store"),
        Err(failure) => ai_ipc_error_response(failure),
    }
}

fn api_ai_job_state(state: &AppState, job_id: &str, owner: &RemoteSessionIdentity) -> HttpResponse {
    let result = match state.ipc_admission.run(IpcClass::Home, || {
        state.thumbnail_client.remote_ai_state(owner, job_id)
    }) {
        Ok(result) => result,
        Err(_) => return ai_admission_busy_response(),
    };
    match result {
        Ok(success) => HttpResponse::json(&success.value)
            .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
            .with_header("Cache-Control", "no-store"),
        Err(failure) => ai_ipc_error_response(failure),
    }
}

fn api_ai_job_cancel(
    state: &AppState,
    job_id: &str,
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let result = match state.ipc_admission.run(IpcClass::Home, || {
        state.thumbnail_client.remote_ai_cancel(owner, job_id)
    }) {
        Ok(result) => result,
        Err(_) => return ai_admission_busy_response(),
    };
    match result {
        Ok(success) => {
            let terminal = success.value.state.is_terminal();
            let mut response = HttpResponse::json(&success.value)
                .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"));
            response.status = if terminal { 200 } else { 202 };
            response.with_header("Cache-Control", "no-store")
        }
        Err(failure) => ai_ipc_error_response(failure),
    }
}

fn remote_page_content_type(content_type: &str) -> Option<&'static str> {
    match content_type {
        "image/jpeg" => Some("image/jpeg"),
        _ => None,
    }
}

/// UTF-8 の relative path を HTTP header の ASCII 範囲へ閉じ込める。
/// 呼び出し側は core の `PagePayload.identity` だけを渡し、HTTP 要求値を echo しない。
fn remote_page_identity_header_value(identity: &RemoteAddress) -> Option<String> {
    let serialized = serde_json::to_string(identity).ok()?;
    Some(utf8_percent_encode(&serialized, NON_ALPHANUMERIC).to_string())
}

fn api_ai_job_result(
    state: &AppState,
    job_id: &str,
    query: &[(String, String)],
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let page = match required_query_value(query, "page")
        .and_then(|value| value.parse::<u32>().map_err(|_| ()))
    {
        Ok(page) => page,
        Err(()) => return HttpResponse::text(400, "Bad Request"),
    };
    let result = match state.ipc_admission.run(IpcClass::Heavy, || {
        state.thumbnail_client.remote_ai_result(owner, job_id, page)
    }) {
        Ok(result) => result,
        Err(_) => return ai_admission_busy_response(),
    };
    match result {
        Ok(success) => {
            let payload = success.value;
            if payload.identity.validate_syntax().is_err() {
                return HttpResponse::text(502, "Bad Gateway")
                    .with_header("Cache-Control", "no-store");
            }
            let Some(identity) = remote_page_identity_header_value(&payload.identity) else {
                return HttpResponse::text(502, "Bad Gateway")
                    .with_header("Cache-Control", "no-store");
            };
            let Some(content_type) = remote_page_content_type(&payload.content_type) else {
                return HttpResponse::text(502, "Bad Gateway")
                    .with_header("Cache-Control", "no-store");
            };
            HttpResponse::bytes(200, content_type, payload.bytes)
                .with_header("Cache-Control", "no-store")
                .with_header("X-mIV-Page-Identity", identity)
        }
        Err(failure) => ai_ipc_error_response(failure),
    }
}

fn ai_admission_busy_response() -> HttpResponse {
    HttpResponse::bytes(
        503,
        "application/json; charset=utf-8",
        serde_json::to_vec(&json!({
            "error": "ipc_busy",
            "message": "remote AI request admission is busy",
        }))
        .unwrap_or_default(),
    )
    .with_header("Retry-After", IPC_RETRY_AFTER_SECONDS.to_string())
    .with_header("Cache-Control", "no-store")
}

fn ai_ipc_error_response(failure: crate::ipc_client::ClientFailure) -> HttpResponse {
    let (status, code, message, terminal_code) = match failure.error {
        IpcClientError::RemoteAi(RemoteAiJobError {
            code,
            message,
            terminal_code,
        }) => {
            let status = match code {
                RemoteAiJobErrorCode::BadRequest => 400,
                RemoteAiJobErrorCode::StartExpired => 504,
                RemoteAiJobErrorCode::SessionClosing | RemoteAiJobErrorCode::NotReady => 409,
                RemoteAiJobErrorCode::PageNotApplicable => 422,
                RemoteAiJobErrorCode::NotFound | RemoteAiJobErrorCode::Forbidden => 404,
                RemoteAiJobErrorCode::JobGone => 410,
                RemoteAiJobErrorCode::PageOutOfRange => 416,
                RemoteAiJobErrorCode::Internal => 500,
            };
            (
                status,
                remote_ai_job_error_name(code).to_owned(),
                message,
                terminal_code,
            )
        }
        IpcClientError::Unavailable(_) => (
            503,
            "miv_not_running".to_owned(),
            "mIV core is not available".to_owned(),
            None,
        ),
        IpcClientError::VersionMismatch { .. } => (
            503,
            "protocol_version_mismatch".to_owned(),
            "IPC protocol versions do not match".to_owned(),
            None,
        ),
        IpcClientError::Protocol(_) => (
            502,
            "ipc_protocol_error".to_owned(),
            "IPC request failed".to_owned(),
            None,
        ),
        IpcClientError::SessionRemote(response) => (
            session_http_status(response.status),
            "session_required".to_owned(),
            response.message,
            None,
        ),
        other => (500, "ipc_error".to_owned(), other.to_string(), None),
    };
    HttpResponse::bytes(
        status,
        "application/json; charset=utf-8",
        serde_json::to_vec(&json!({
            "error": code,
            "message": message,
            "terminal_code": terminal_code,
        }))
        .unwrap_or_default(),
    )
    .with_header("Cache-Control", "no-store")
}

fn remote_ai_job_error_name(code: RemoteAiJobErrorCode) -> &'static str {
    match code {
        RemoteAiJobErrorCode::BadRequest => "bad_request",
        RemoteAiJobErrorCode::StartExpired => "start_expired",
        RemoteAiJobErrorCode::SessionClosing => "session_closing",
        RemoteAiJobErrorCode::NotFound => "not_found",
        RemoteAiJobErrorCode::Forbidden => "forbidden",
        RemoteAiJobErrorCode::JobGone => "job_gone",
        RemoteAiJobErrorCode::NotReady => "not_ready",
        RemoteAiJobErrorCode::PageNotApplicable => "page_not_applicable",
        RemoteAiJobErrorCode::PageOutOfRange => "page_out_of_range",
        RemoteAiJobErrorCode::Internal => "internal",
    }
}

#[derive(Deserialize)]
struct SessionPingBody {
    #[serde(default)]
    user_active: bool,
    #[serde(default)]
    media_playing: bool,
}

#[derive(Deserialize)]
struct VideoStartBody {
    root: String,
    path: String,
    quality: VideoStreamQuality,
}

#[derive(Deserialize)]
struct VideoControlBody {
    session: u64,
    #[serde(flatten)]
    action: VideoStreamControlAction,
}

#[derive(Deserialize)]
struct VideoSeekBody {
    session: u64,
    position_secs: f64,
}

#[derive(Deserialize)]
struct VideoThumbnailBody {
    session: u64,
    #[serde(default)]
    position_secs: Option<f64>,
}

#[derive(Deserialize)]
struct VideoStopBody {
    session: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamRouteResource {
    Playlist(VideoStreamPlaylistKind),
    Segment(VideoStreamSegmentIndex),
}

fn api_video_start(
    request: &mut Request,
    state: &AppState,
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let body: VideoStartBody = match read_video_json(request) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let address = RemoteAddress::file(body.root, body.path);
    if let Err(error) = state.library.validate_remote_file_video(&address) {
        return store_error_response(error).with_header("Cache-Control", "no-store");
    }
    state.session_activity.note(owner);
    let result = match state.ipc_admission.run(IpcClass::Stream, || {
        state
            .thumbnail_client
            .video_stream_start(owner, address, body.quality)
    }) {
        Ok(result) => result,
        Err(busy) => return video_admission_busy_response(busy),
    };
    match result {
        Ok(success) => {
            let payload = success.value;
            HttpResponse::json(&json!({
                "session": payload.session,
                "generation": payload.generation,
                "playlist": video_playlist_url(payload.session, payload.generation),
                "duration_secs": payload.duration_secs,
                "source_origin_secs": payload.source_origin_secs,
                "buffer_target_secs": payload.buffer_target_secs,
                "codec": payload.codecs,
                "encoder": payload.encoder,
                "video_size": payload.video_size,
                "audio_processing": payload.audio_processing,
                "end_behavior": payload.end_behavior,
            }))
            .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
            .with_header("Cache-Control", "no-store")
        }
        Err(failure) => video_ipc_error_response(failure),
    }
}

fn api_video_control(
    request: &mut Request,
    state: &AppState,
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let body: VideoControlBody = match read_video_json(request) {
        Ok(body) => body,
        Err(response) => return response,
    };
    state.session_activity.note(owner);
    let result = match state.ipc_admission.run(IpcClass::Stream, || {
        state
            .thumbnail_client
            .video_stream_control(owner, body.session, body.action)
    }) {
        Ok(result) => result,
        Err(busy) => return video_admission_busy_response(busy),
    };
    match result {
        Ok(success) => session_response_http(success.value, state),
        Err(failure) => video_ipc_error_response(failure),
    }
}

fn api_video_seek(
    request: &mut Request,
    state: &AppState,
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let body: VideoSeekBody = match read_video_json(request) {
        Ok(body) => body,
        Err(response) => return response,
    };
    if !body.position_secs.is_finite() || body.position_secs < 0.0 {
        return video_request_error_response(
            400,
            "stream_bad_request",
            "position_secs must be finite and non-negative",
        );
    }
    state.session_activity.note(owner);
    let result = match state.ipc_admission.run(IpcClass::Stream, || {
        state
            .thumbnail_client
            .video_stream_seek(owner, body.session, body.position_secs)
    }) {
        Ok(result) => result,
        Err(busy) => return video_admission_busy_response(busy),
    };
    match result {
        Ok(success) => HttpResponse::json(&json!({
            "generation": success.value.generation,
            "playlist": video_playlist_url(body.session, success.value.generation),
        }))
        .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
        .with_header("Cache-Control", "no-store"),
        Err(failure) => video_ipc_error_response(failure),
    }
}

fn api_video_thumbnail(
    request: &mut Request,
    state: &AppState,
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let body: VideoThumbnailBody = match read_video_json(request) {
        Ok(body) => body,
        Err(response) => return response,
    };
    if body
        .position_secs
        .is_some_and(|position| !position.is_finite() || position < 0.0)
    {
        return video_request_error_response(
            400,
            "stream_bad_request",
            "thumbnail position_secs must be finite and non-negative",
        );
    }
    state.session_activity.note(owner);
    let result = match state.ipc_admission.run(IpcClass::Stream, || {
        state
            .thumbnail_client
            .video_stream_thumbnail(owner, body.session, body.position_secs)
    }) {
        Ok(result) => result,
        Err(busy) => return video_admission_busy_response(busy),
    };
    match result {
        Ok(success) => video_thumbnail_http_response(success.value),
        Err(failure) => video_ipc_error_response(failure),
    }
}

fn video_thumbnail_http_response(payload: VideoStreamThumbnailPayload) -> HttpResponse {
    match payload {
        VideoStreamThumbnailPayload::Pending => HttpResponse::bytes(
            202,
            "application/json; charset=utf-8",
            serde_json::to_vec(&json!({"status": "pending"})).unwrap_or_default(),
        )
        .with_header("Cache-Control", "no-store"),
        VideoStreamThumbnailPayload::Ready {
            actual_pts_secs,
            width,
            height,
            webp_bytes,
        } => HttpResponse::bytes(200, "image/webp", webp_bytes)
            .with_header("Cache-Control", "no-store")
            .with_header("X-mIV-Video-Thumbnail-PTS", format!("{actual_pts_secs:.9}"))
            .with_header("X-mIV-Video-Thumbnail-Width", width.to_string())
            .with_header("X-mIV-Video-Thumbnail-Height", height.to_string()),
        VideoStreamThumbnailPayload::Cleared => {
            HttpResponse::bytes(204, "application/octet-stream", Vec::new())
                .with_header("Cache-Control", "no-store")
        }
    }
}

fn api_video_jumps(
    state: &AppState,
    query: &[(String, String)],
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let session = match video_query_session(query) {
        Ok(session) => session,
        Err(response) => return response,
    };
    state.session_activity.note(owner);
    let result = match state.ipc_admission.run(IpcClass::Heavy, || {
        state
            .thumbnail_client
            .video_stream_jump_list(owner, session)
    }) {
        Ok(result) => result,
        Err(busy) => return video_admission_busy_response(busy),
    };
    match result {
        Ok(success) => {
            let sections: Vec<_> = success
                .value
                .sections
                .into_iter()
                .map(|section| {
                    let entries: Vec<_> = section
                        .entries
                        .into_iter()
                        .map(|entry| {
                            let thumbnail_url = entry.thumbnail_token.map(|token| {
                                format!(
                                    "/api/video/jump-thumbnail?session={session}&token={}",
                                    utf8_percent_encode(&token, NON_ALPHANUMERIC)
                                )
                            });
                            json!({
                                "id": entry.id,
                                "position_secs": entry.position_secs,
                                "display_time": entry.display_time,
                                "title": entry.title,
                                "thumbnail_url": thumbnail_url,
                            })
                        })
                        .collect();
                    json!({
                        "kind": section.kind,
                        "label": section.label,
                        "entries": entries,
                    })
                })
                .collect();
            HttpResponse::json(&json!({"session": session, "sections": sections}))
                .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
                .with_header("Cache-Control", "no-store")
        }
        Err(failure) => video_ipc_error_response(failure),
    }
}

fn api_video_jump_thumbnail(
    state: &AppState,
    query: &[(String, String)],
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let session = match video_query_session(query) {
        Ok(session) => session,
        Err(response) => return response,
    };
    let token = match required_query_value(query, "token") {
        Ok(token) if !token.is_empty() && token.len() <= 256 => token,
        _ => {
            return video_request_error_response(
                400,
                "stream_bad_request",
                "jump thumbnail token query is invalid",
            );
        }
    };
    state.session_activity.note(owner);
    let result = match state.ipc_admission.run(IpcClass::Heavy, || {
        state
            .thumbnail_client
            .video_stream_jump_thumbnail(owner, session, token)
    }) {
        Ok(result) => result,
        Err(busy) => return video_admission_busy_response(busy),
    };
    match result {
        Ok(success) => video_jump_thumbnail_http_response(success.value),
        Err(failure) => video_ipc_error_response(failure),
    }
}

fn video_query_session(query: &[(String, String)]) -> Result<u64, HttpResponse> {
    required_query_value(query, "session")
        .and_then(|value| value.parse::<u64>().map_err(|_| ()))
        .map_err(|_| {
            video_request_error_response(400, "stream_bad_request", "session query is required")
        })
}

fn video_jump_thumbnail_http_response(payload: VideoStreamJumpThumbnailPayload) -> HttpResponse {
    match payload {
        VideoStreamJumpThumbnailPayload::Found { webp_bytes } => {
            HttpResponse::bytes(200, "image/webp", webp_bytes)
                .with_header("Cache-Control", "private, max-age=60")
        }
        VideoStreamJumpThumbnailPayload::Missing => HttpResponse::bytes(
            404,
            "application/json; charset=utf-8",
            serde_json::to_vec(&json!({
                "error": "jump_thumbnail_not_found",
                "message": "サムネイルが見つかりませんでした。",
            }))
            .unwrap_or_default(),
        )
        .with_header("Cache-Control", "no-store"),
    }
}

fn api_video_state(
    state: &AppState,
    query: &[(String, String)],
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let session = match required_query_value(query, "session")
        .and_then(|value| value.parse::<u64>().map_err(|_| ()))
    {
        Ok(session) => session,
        Err(()) => {
            return video_request_error_response(
                400,
                "stream_bad_request",
                "session query is required",
            );
        }
    };
    state.session_activity.note(owner);
    let result = match state.ipc_admission.run(IpcClass::Stream, || {
        state.thumbnail_client.video_stream_state(owner, session)
    }) {
        Ok(result) => result,
        Err(busy) => return video_admission_busy_response(busy),
    };
    match result {
        Ok(success) => HttpResponse::json(&success.value)
            .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
            .with_header("Cache-Control", "no-store"),
        Err(failure) => video_ipc_error_response(failure),
    }
}

fn api_video_stop(
    request: &mut Request,
    state: &AppState,
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let body: VideoStopBody = match read_video_json(request) {
        Ok(body) => body,
        Err(response) => return response,
    };
    state.session_activity.note(owner);
    let result = match state.ipc_admission.run(IpcClass::Stream, || {
        state
            .thumbnail_client
            .video_stream_stop(owner, body.session)
    }) {
        Ok(result) => result,
        Err(busy) => return video_admission_busy_response(busy),
    };
    match result {
        Ok(_) => HttpResponse::json(&json!({"stopped": true}))
            .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
            .with_header("Cache-Control", "no-store"),
        Err(failure) => video_ipc_error_response(failure),
    }
}

fn stream_resource(state: &AppState, path: &str, owner: &RemoteSessionIdentity) -> HttpResponse {
    let (session, generation, resource) = match parse_stream_path(path) {
        Ok(parsed) => parsed,
        Err(()) => return HttpResponse::text(404, "Not Found"),
    };
    state.session_activity.note(owner);
    match resource {
        StreamRouteResource::Playlist(kind) => {
            let result = match state.ipc_admission.run(IpcClass::Stream, || {
                state
                    .thumbnail_client
                    .video_stream_playlist(owner, session, generation, kind)
            }) {
                Ok(result) => result,
                Err(busy) => return video_admission_busy_response(busy),
            };
            match result {
                Ok(success) => playlist_http_response(success.value.body),
                Err(failure) => video_ipc_error_response(failure),
            }
        }
        StreamRouteResource::Segment(index) => {
            let result = match state.ipc_admission.run(IpcClass::Stream, || {
                state
                    .thumbnail_client
                    .video_stream_segment(owner, session, generation, index)
            }) {
                Ok(result) => result,
                Err(busy) => return video_admission_busy_response(busy),
            };
            match result {
                Ok(success) => segment_http_response(index, success.value),
                Err(failure) => video_ipc_error_response(failure),
            }
        }
    }
}

fn read_video_json<T: serde::de::DeserializeOwned>(
    request: &mut Request,
) -> Result<T, HttpResponse> {
    let body = match read_body_limited(request, MAX_VIDEO_BODY_BYTES) {
        Ok(body) => body,
        Err(BodyReadError::TooLarge) => {
            return Err(video_request_error_response(
                413,
                "stream_payload_too_large",
                "video request body is too large",
            ));
        }
        Err(BodyReadError::Read) => {
            return Err(video_request_error_response(
                400,
                "stream_bad_request",
                "video request body could not be read",
            ));
        }
    };
    serde_json::from_slice(&body).map_err(|_| {
        video_request_error_response(400, "stream_bad_request", "video request body is invalid")
    })
}

fn video_request_error_response(status: u16, code: &str, internal_message: &str) -> HttpResponse {
    let user_message = if status == 413 {
        "送信した内容が大きすぎます。内容を減らしてもう一度お試しください。"
    } else {
        "動画の指定内容を確認して、もう一度お試しください。"
    };
    HttpResponse::bytes(
        status,
        "application/json; charset=utf-8",
        serde_json::to_vec(&json!({"error": code, "message": user_message})).unwrap_or_default(),
    )
    .with_header("Cache-Control", "no-store")
    .with_log_details(json!({
        "video_stream": {
            "error_code": code,
            "internal_message": internal_message,
        }
    }))
}

fn parse_stream_path(path: &str) -> Result<(u64, u64, StreamRouteResource), ()> {
    let mut parts = path.strip_prefix("/stream/").ok_or(())?.split('/');
    let session = parts.next().ok_or(())?.parse::<u64>().map_err(|_| ())?;
    let generation = parts.next().ok_or(())?.parse::<u64>().map_err(|_| ())?;
    let name = parts.next().ok_or(())?;
    if parts.next().is_some() {
        return Err(());
    }
    let resource = match name {
        "index.m3u8" => StreamRouteResource::Playlist(VideoStreamPlaylistKind::Master),
        "media.m3u8" => StreamRouteResource::Playlist(VideoStreamPlaylistKind::Media),
        "init.mp4" => StreamRouteResource::Segment(VideoStreamSegmentIndex::Init),
        _ => {
            let sequence = name
                .strip_suffix(".m4s")
                .ok_or(())?
                .parse::<u64>()
                .map_err(|_| ())?;
            StreamRouteResource::Segment(VideoStreamSegmentIndex::Media { sequence })
        }
    };
    Ok((session, generation, resource))
}

fn playlist_http_response(body: String) -> HttpResponse {
    if body.is_empty() {
        video_not_ready_response("playlist is not ready")
    } else {
        HttpResponse::bytes(200, "application/vnd.apple.mpegurl", body.into_bytes())
            .with_header("Cache-Control", "no-store")
    }
}

fn segment_http_response(
    index: VideoStreamSegmentIndex,
    payload: VideoStreamSegmentPayload,
) -> HttpResponse {
    match payload {
        VideoStreamSegmentPayload::Found(bytes) if !bytes.is_empty() => {
            let (content_type, cache_control) = match index {
                VideoStreamSegmentIndex::Init => {
                    ("video/mp4", "private, max-age=31536000, immutable")
                }
                VideoStreamSegmentIndex::Media { .. } => ("video/iso.segment", "no-store"),
            };
            HttpResponse::bytes(200, content_type, bytes)
                .with_header("Cache-Control", cache_control)
        }
        VideoStreamSegmentPayload::Found(_) => video_not_ready_response("segment is not ready"),
        VideoStreamSegmentPayload::NotFound => {
            HttpResponse::text(404, "Not Found").with_header("Cache-Control", "no-store")
        }
        VideoStreamSegmentPayload::Gone => {
            HttpResponse::text(410, "Gone").with_header("Cache-Control", "no-store")
        }
    }
}

fn video_playlist_url(session: u64, generation: u64) -> String {
    format!("/stream/{session}/{generation}/index.m3u8")
}

fn video_admission_busy_response(busy: AdmissionBusy) -> HttpResponse {
    HttpResponse::bytes(
        503,
        "application/json; charset=utf-8",
        serde_json::to_vec(&json!({
            "error": "ipc_busy",
            "message": "ストリーミング要求が混み合っています。再試行してください。",
        }))
        .unwrap_or_default(),
    )
    .with_header("Retry-After", IPC_RETRY_AFTER_SECONDS.to_string())
    .with_header("Cache-Control", "no-store")
    .with_log_details(json!({
        "video_stream": {
            "error_code": "ipc_busy",
            "ipc_status": "admission_busy",
            "ipc_stream_in_flight": busy.stream_in_flight,
            "ipc_stream_limit": busy.stream_limit,
        }
    }))
}

fn video_not_ready_response(internal_message: &str) -> HttpResponse {
    HttpResponse::bytes(
        503,
        "application/json; charset=utf-8",
        serde_json::to_vec(&json!({
            "error": "stream_not_ready",
            "message": "動画を準備しています。しばらくお待ちください。",
        }))
        .unwrap_or_default(),
    )
    .with_header("Retry-After", IPC_RETRY_AFTER_SECONDS.to_string())
    .with_header("Cache-Control", "no-store")
    .with_log_details(json!({
        "video_stream": {
            "error_code": "stream_not_ready",
            "internal_message": internal_message,
        }
    }))
}

fn video_stream_user_message(code: VideoStreamErrorCode) -> &'static str {
    match code {
        VideoStreamErrorCode::StartQueueTimeout
        | VideoStreamErrorCode::StartUiTimeout
        | VideoStreamErrorCode::StartPlayerTimeout
        | VideoStreamErrorCode::StartSeekTimeout
        | VideoStreamErrorCode::StartEncoderTimeout
        | VideoStreamErrorCode::StartPlaylistTimeout => {
            "動画を開始できませんでした。もう一度お試しください。"
        }
        VideoStreamErrorCode::SessionMismatch | VideoStreamErrorCode::GenerationMismatch => {
            "動画の配信が終了しました。もう一度開いてください。"
        }
        VideoStreamErrorCode::NotReady
        | VideoStreamErrorCode::Busy
        | VideoStreamErrorCode::ResourceTimeout => {
            "動画を準備しています。しばらくしてからもう一度お試しください。"
        }
        VideoStreamErrorCode::BadRequest => "動画の指定内容を確認して、もう一度お試しください。",
        VideoStreamErrorCode::FavoriteNotFound
        | VideoStreamErrorCode::PathRejected
        | VideoStreamErrorCode::NotFound => "動画が見つかりませんでした。",
        VideoStreamErrorCode::Unsupported => "この動画は再生できません。",
        VideoStreamErrorCode::UiTimeout => {
            "動画の操作を完了できませんでした。もう一度お試しください。"
        }
        VideoStreamErrorCode::Failed | VideoStreamErrorCode::Internal => {
            "動画を処理できませんでした。もう一度お試しください。"
        }
    }
}

fn video_ipc_error_response(failure: crate::ipc_client::ClientFailure) -> HttpResponse {
    let (status, code, message, retryable, internal_message) = match failure.error {
        IpcClientError::Unavailable(_) => (
            503,
            "miv_not_running",
            "mIV 本体が起動していません。".to_owned(),
            true,
            None,
        ),
        IpcClientError::VersionMismatch { client, server } => (
            503,
            "protocol_version_mismatch",
            format!("IPC 版が一致しません (remote-web={client}, mIV={server})。"),
            false,
            None,
        ),
        IpcClientError::Protocol(detail) if detail.kind == "timeout" => (
            503,
            "ipc_timeout",
            "mIV 本体の応答が時間内に完了しませんでした。".to_owned(),
            true,
            None,
        ),
        IpcClientError::Protocol(_) => (
            502,
            "ipc_protocol_error",
            "mIV 本体との通信に失敗しました。".to_owned(),
            false,
            None,
        ),
        IpcClientError::VideoStreamRemote(error) => {
            let user_message = video_stream_user_message(error.code).to_owned();
            let internal_message = error.message;
            let (status, code, retryable) = match error.code {
                VideoStreamErrorCode::BadRequest => (400, "stream_bad_request", false),
                VideoStreamErrorCode::FavoriteNotFound => (404, "stream_favorite_not_found", false),
                VideoStreamErrorCode::PathRejected => (404, "stream_path_rejected", false),
                VideoStreamErrorCode::NotFound => (404, "stream_not_found", false),
                VideoStreamErrorCode::Unsupported => (415, "stream_unsupported", false),
                VideoStreamErrorCode::SessionMismatch => (409, "stream_session_mismatch", false),
                VideoStreamErrorCode::GenerationMismatch => {
                    (409, "stream_generation_mismatch", false)
                }
                VideoStreamErrorCode::NotReady => (503, "stream_not_ready", true),
                VideoStreamErrorCode::Busy => (503, "stream_busy", true),
                VideoStreamErrorCode::UiTimeout => (504, "stream_ui_timeout", false),
                VideoStreamErrorCode::Failed => (422, "stream_failed", false),
                VideoStreamErrorCode::StartQueueTimeout => {
                    (504, "stream_start_queue_timeout", false)
                }
                VideoStreamErrorCode::StartUiTimeout => (504, "stream_start_ui_timeout", false),
                VideoStreamErrorCode::StartPlayerTimeout => {
                    (504, "stream_start_player_timeout", false)
                }
                VideoStreamErrorCode::StartSeekTimeout => (504, "stream_start_seek_timeout", false),
                VideoStreamErrorCode::StartEncoderTimeout => {
                    (504, "stream_start_encoder_timeout", false)
                }
                VideoStreamErrorCode::StartPlaylistTimeout => {
                    (504, "stream_start_playlist_timeout", false)
                }
                VideoStreamErrorCode::ResourceTimeout => (503, "stream_resource_timeout", true),
                VideoStreamErrorCode::Internal => (500, "stream_internal", false),
            };
            (
                status,
                code,
                user_message,
                retryable,
                Some(internal_message),
            )
        }
        IpcClientError::SessionRemote(response) => (
            session_http_status(response.status),
            "session_required",
            response.message,
            false,
            None,
        ),
        IpcClientError::Remote(_)
        | IpcClientError::CollectionRemote(_)
        | IpcClientError::MediaRemote(_)
        | IpcClientError::WriteRemote(_)
        | IpcClientError::RemoteAi(_) => (
            502,
            "ipc_response_type_mismatch",
            "mIV 本体から予期しない応答を受信しました。".to_owned(),
            false,
            None,
        ),
    };
    let mut response = HttpResponse::bytes(
        status,
        "application/json; charset=utf-8",
        serde_json::to_vec(&json!({"error": code, "message": message})).unwrap_or_default(),
    )
    .with_header("Cache-Control", "no-store");
    if retryable {
        response = response.with_header("Retry-After", IPC_RETRY_AFTER_SECONDS.to_string());
    }
    if let Some(internal_message) = internal_message {
        response = response.with_log_details(json!({
            "video_stream": {
                "internal_message": internal_message,
            }
        }));
    }
    response.with_body_error_code_log()
}

fn api_session_acquire(
    request: &Request,
    state: &AppState,
    auth: AuthDecision,
    client_id: &str,
) -> HttpResponse {
    let source_ip =
        forwarded_source_ip(request).or_else(|| request.remote_addr().map(|address| address.ip()));
    let peer = source_ip
        .and_then(|source_ip| {
            state
                .session_peers
                .lock()
                .ok()
                .and_then(|peers| peers.get(&source_ip).cloned())
                .or_else(|| {
                    let peer = crate::connection_url::detect_peer_info(Some(source_ip));
                    if let Ok(mut peers) = state.session_peers.lock() {
                        peers.insert(source_ip, peer.clone());
                    }
                    Some(peer)
                })
        })
        .unwrap_or_else(|| crate::connection_url::detect_peer_info(None));
    match state.thumbnail_client.session_acquire(client_id, peer) {
        Ok(response) => {
            if response.status == SessionStatus::Active
                && let Some(session_id) = response.session_id.as_ref()
            {
                state.remote_client_identities.bind_session(
                    auth,
                    &RemoteSessionIdentity {
                        client_id: client_id.to_owned(),
                        session_id: session_id.clone(),
                    },
                );
            }
            session_acquire_response_http(response, state)
        }
        Err(failure) => session_failure_response(failure, state),
    }
}

fn api_session_ping(
    request: &mut Request,
    state: &AppState,
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let body = match read_body_limited(request, MAX_PIN_BODY_BYTES) {
        Ok(body) => body,
        Err(BodyReadError::TooLarge) => return HttpResponse::text(413, "Payload Too Large"),
        Err(BodyReadError::Read) => return HttpResponse::text(400, "Bad Request"),
    };
    let body: SessionPingBody = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => return HttpResponse::text(400, "Bad Request"),
    };
    match state
        .thumbnail_client
        .session_ping(owner, body.user_active, body.media_playing)
    {
        Ok(response) => session_response_http(response, state),
        Err(failure) => session_failure_response(failure, state),
    }
}

fn with_session_activity(
    state: &AppState,
    owner: &RemoteSessionIdentity,
    operation: impl FnOnce() -> HttpResponse,
) -> HttpResponse {
    match state.thumbnail_client.session_activity(owner) {
        Ok(response) if response.status == SessionStatus::Active => operation(),
        Ok(response) => session_response_http(response, state),
        Err(failure) => session_failure_response(failure, state),
    }
}

fn session_response_http(response: SessionResponse, state: &AppState) -> HttpResponse {
    session_response_http_inner(response, state, None)
}

fn session_acquire_response_http(response: SessionResponse, state: &AppState) -> HttpResponse {
    session_response_http_inner(response, state, Some(web_asset_token(&state.web_root)))
}

fn session_response_http_inner(
    response: SessionResponse,
    state: &AppState,
    asset_token: Option<String>,
) -> HttpResponse {
    let status = session_http_status(response.status);
    let remote_state_generation = state
        .library
        .remote_state()
        .ok()
        .map(|state| state.remote_state_generation);
    let mut body = json!({
        "status": response.status,
        "message": response.message,
        "session_id": response.session_id,
        "remote_state_generation": remote_state_generation,
    });
    if let Some(asset_token) = asset_token
        && let Some(body) = body.as_object_mut()
    {
        body.insert("asset_token".to_owned(), json!(asset_token));
    }
    HttpResponse::bytes(
        status,
        "application/json; charset=utf-8",
        serde_json::to_vec(&body).unwrap_or_default(),
    )
    .with_header("Cache-Control", "no-store")
    .with_log_details(json!({
        "session": { "status": response.status }
    }))
}

fn session_http_status(status: SessionStatus) -> u16 {
    match status {
        SessionStatus::Active => 200,
        SessionStatus::LocalInUse | SessionStatus::Superseded => 409,
        SessionStatus::NotAcquired | SessionStatus::Expired => 428,
    }
}

fn session_failure_response(
    failure: crate::ipc_client::ClientFailure,
    state: &AppState,
) -> HttpResponse {
    match failure.error {
        IpcClientError::SessionRemote(response) => session_response_http(response, state),
        IpcClientError::Unavailable(_) => HttpResponse::bytes(
            503,
            "application/json; charset=utf-8",
            serde_json::to_vec(&json!({
                "status": "unavailable",
                "message": "mIV 本体が起動していません。",
            }))
            .unwrap_or_default(),
        ),
        IpcClientError::VersionMismatch { .. } | IpcClientError::Protocol(_) => {
            HttpResponse::text(503, "Service Unavailable")
        }
        IpcClientError::Remote(_)
        | IpcClientError::CollectionRemote(_)
        | IpcClientError::MediaRemote(_)
        | IpcClientError::WriteRemote(_)
        | IpcClientError::VideoStreamRemote(_)
        | IpcClientError::RemoteAi(_) => HttpResponse::text(502, "Bad Gateway"),
    }
    .with_header("Cache-Control", "no-store")
}

fn unauthorized() -> HttpResponse {
    HttpResponse::text(401, "Unauthorized")
        .with_header("WWW-Authenticate", "Bearer")
        .with_header("Cache-Control", "no-store")
}

/// Identify the web assets currently served by this process.
///
/// The app is a single page whose screens are hash changes, so a tab that is already open never
/// re-fetches its own scripts. It keeps running the build it was loaded with until someone
/// reloads, which is invisible from the inside and has already cost one round of testing a fix
/// that was not in the running code. Assets are read per request and served `no-cache`, so the
/// only missing piece is telling the page that what it is running is no longer what is served.
///
/// Development serves files from disk, where size and mtime cheaply detect a deploy. Distribution
/// embeds immutable bytes, so its token hashes the embedded asset set and has no filesystem input.
#[cfg(not(feature = "embedded-web-assets"))]
fn web_asset_token(web_root: &std::path::Path) -> String {
    use std::hash::{Hash, Hasher};
    let files: Vec<(&str, Option<(u64, i64)>)> = WEB_ASSET_PATHS
        .iter()
        .map(|asset| {
            let metadata = fs::metadata(web_root.join(asset)).ok().map(|metadata| {
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |delta| delta.as_millis() as i64);
                (metadata.len(), modified)
            });
            (*asset, metadata)
        })
        .collect();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    files.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(feature = "embedded-web-assets")]
fn web_asset_token(_web_root: &std::path::Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    EMBEDDED_WEB_ASSETS.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn api_app_version(state: &AppState) -> HttpResponse {
    HttpResponse::json(&json!({ "asset_token": web_asset_token(&state.web_root) }))
        .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
        .with_header("Cache-Control", "no-store")
}

fn api_auth_status(state: &AppState, decision: AuthDecision) -> HttpResponse {
    let remaining = ceil_seconds(state.auth.lockout_remaining());
    HttpResponse::json(&json!({
        "authenticated": matches!(decision, AuthDecision::Authorized(_)),
        "lockout_remaining_seconds": remaining,
    }))
    .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
    .with_header("Cache-Control", "no-store")
}

#[derive(Deserialize)]
struct PinRequest {
    pin: String,
    #[serde(default = "remember_by_default")]
    remember: bool,
}

fn remember_by_default() -> bool {
    true
}

fn api_auth_pin(request: &mut Request, state: &AppState) -> HttpResponse {
    let body = match read_body_limited(request, MAX_PIN_BODY_BYTES) {
        Ok(body) => body,
        Err(BodyReadError::TooLarge) => {
            return HttpResponse::text(413, "Payload Too Large")
                .with_header("Cache-Control", "no-store");
        }
        Err(BodyReadError::Read) => {
            return HttpResponse::text(400, "Bad Request").with_header("Cache-Control", "no-store");
        }
    };
    let input: PinRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return HttpResponse::text(400, "Bad Request").with_header("Cache-Control", "no-store");
        }
    };
    let secure = request_is_https(request);
    match state.auth.verify_pin(&input.pin) {
        PinVerification::Success => {
            let cookie = state.auth.issue_session_cookie(input.remember, secure);
            HttpResponse::json(&json!({"authenticated": true}))
                .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
                .with_header("Set-Cookie", cookie.header)
                .with_header("Cache-Control", "no-store")
                .with_log_details(json!({
                    "pin_auth": {
                        "success": true,
                        "remember": input.remember,
                        "cookie_secure": secure,
                    }
                }))
                .with_sensitive_value(input.pin)
                .with_sensitive_value(cookie.sensitive_value)
        }
        PinVerification::Invalid {
            failure_count,
            lockout,
        } => {
            let remaining = lockout.map_or(0, ceil_seconds);
            let status = if lockout.is_some() { 429 } else { 401 };
            let mut response = HttpResponse::bytes(
                status,
                "application/json; charset=utf-8",
                serde_json::to_vec(&json!({
                    "authenticated": false,
                    "lockout_remaining_seconds": remaining,
                }))
                .unwrap_or_default(),
            )
            .with_header("Cache-Control", "no-store")
            .with_log_details(json!({
                "pin_auth": {
                    "success": false,
                    "failure_count": failure_count,
                    "lockout_remaining_seconds": remaining,
                }
            }))
            .with_sensitive_value(input.pin);
            if remaining > 0 {
                response = response.with_header("Retry-After", remaining.to_string());
            }
            response
        }
        PinVerification::Locked {
            failure_count,
            remaining,
        } => {
            let remaining = ceil_seconds(remaining);
            HttpResponse::bytes(
                429,
                "application/json; charset=utf-8",
                serde_json::to_vec(&json!({
                    "authenticated": false,
                    "lockout_remaining_seconds": remaining,
                }))
                .unwrap_or_default(),
            )
            .with_header("Retry-After", remaining.to_string())
            .with_header("Cache-Control", "no-store")
            .with_log_details(json!({
                "pin_auth": {
                    "success": false,
                    "failure_count": failure_count,
                    "lockout_remaining_seconds": remaining,
                    "attempt_skipped_during_lockout": true,
                }
            }))
            .with_sensitive_value(input.pin)
        }
    }
}

fn api_favorites(state: &AppState) -> HttpResponse {
    let favorites = match state.library.favorites() {
        Ok(favorites) => favorites,
        Err(error) => return store_error_response(error),
    };
    match HttpResponse::json(&favorites) {
        Ok(response) => response.with_header("Cache-Control", "no-store"),
        Err(error) => {
            eprintln!("remote-web: favorites JSON encoding failed: {error}");
            HttpResponse::text(500, "Internal Server Error")
        }
    }
}

fn api_remote_state(state: &AppState) -> HttpResponse {
    match state.library.remote_state() {
        Ok(remote_state) => HttpResponse::json(&remote_state)
            .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
            .with_header("Cache-Control", "no-store"),
        Err(error) => store_error_response(error),
    }
}

fn api_home(state: &AppState, owner: &RemoteSessionIdentity) -> HttpResponse {
    let started = Instant::now();
    let result = match state
        .ipc_admission
        .run(IpcClass::Home, || state.thumbnail_client.home(owner))
    {
        Ok(result) => result,
        Err(busy) => return collection_admission_busy_response(busy, "home"),
    };
    match result {
        Ok(result) => HttpResponse::json(&result.value)
            .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
            .with_header("Cache-Control", "no-store")
            .with_log_details(json!({
                "collection": {
                    "kind": "home",
                    "ipc_status": "ok",
                    "ipc_ms": crate::diagnostics::duration_ms(started.elapsed()),
                    "ipc_retry_count": result.retry_count,
                    "ipc_retry_statuses": result.retry_statuses,
                    "ipc_connection_id": result.connection_id,
                }
            })),
        Err(failure) => collection_ipc_error_response(failure, started.elapsed(), "home"),
    }
}

fn api_collection(
    state: &AppState,
    query: &[(String, String)],
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let kind_name = match required_query_value(query, "kind") {
        Ok(value) => value,
        Err(()) => return HttpResponse::text(400, "Bad Request"),
    };
    let kind = match kind_name {
        "reading_history" => CollectionKind::ReadingHistory,
        "bookmarks" => CollectionKind::Bookmarks,
        "bookshelf" => CollectionKind::Bookshelf,
        "rating" => {
            let stars = match required_query_value(query, "stars")
                .and_then(|value| value.parse::<u8>().map_err(|_| ()))
            {
                Ok(stars @ 1..=5) => stars,
                _ => return HttpResponse::text(400, "Bad Request"),
            };
            CollectionKind::Rating { stars }
        }
        "smart_folder" => {
            let definition_id = match required_query_value(query, "id") {
                Ok(value) if Uuid::parse_str(value).is_ok() => value.to_owned(),
                _ => return HttpResponse::text(400, "Bad Request"),
            };
            CollectionKind::SmartFolder { definition_id }
        }
        _ => return HttpResponse::text(400, "Bad Request"),
    };
    let started = Instant::now();
    let result = match state.ipc_admission.run(IpcClass::Heavy, || {
        state.thumbnail_client.collection(owner, kind)
    }) {
        Ok(result) => result,
        Err(busy) => return collection_admission_busy_response(busy, kind_name),
    };
    match result {
        Ok(mut result) => {
            state
                .library
                .retain_allowed_remote_entries(&mut result.value.entries);
            let entry_count = result.value.entries.len();
            HttpResponse::json(&result.value)
                .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
                .with_header("Cache-Control", "no-store")
                .with_log_details(json!({
                    "collection": {
                        "kind": kind_name,
                        "entry_count": entry_count,
                        "ipc_status": "ok",
                        "ipc_ms": crate::diagnostics::duration_ms(started.elapsed()),
                        "ipc_retry_count": result.retry_count,
                        "ipc_retry_statuses": result.retry_statuses,
                        "ipc_connection_id": result.connection_id,
                    }
                }))
        }
        Err(failure) => collection_ipc_error_response(failure, started.elapsed(), kind_name),
    }
}

fn api_favorite_search(
    state: &AppState,
    query: &[(String, String)],
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let search_query = match required_query_value(query, "q") {
        Ok(value) => value,
        Err(()) => {
            return HttpResponse::text(400, "Bad Request").with_header("Cache-Control", "no-store");
        }
    };
    let kind_name = match required_query_value(query, "kind") {
        Ok(value) => value,
        Err(()) => {
            return HttpResponse::text(400, "Bad Request").with_header("Cache-Control", "no-store");
        }
    };
    let kind = match kind_name {
        "all" => FavoriteSearchKind::All,
        "folder" => FavoriteSearchKind::Folder,
        "zip" => FavoriteSearchKind::Zip,
        "pdf" => FavoriteSearchKind::Pdf,
        _ => {
            return HttpResponse::text(400, "Bad Request").with_header("Cache-Control", "no-store");
        }
    };
    let query_length = search_query.chars().count();
    let started = Instant::now();
    let result = match state.ipc_admission.run(IpcClass::Heavy, || {
        state.thumbnail_client.favorite_search(
            owner,
            FavoriteSearchRequest {
                query: search_query.to_owned(),
                kind,
            },
        )
    }) {
        Ok(result) => result,
        Err(busy) => return collection_admission_busy_response(busy, "favorite_search"),
    };
    match result {
        Ok(mut result) => {
            state
                .library
                .retain_allowed_remote_entries(&mut result.value.listing.entries);
            let entry_count = result.value.listing.entries.len();
            let index_state = match result.value.index_state {
                FavoriteSearchIndexState::Ready => "ready",
                FavoriteSearchIndexState::Disabled => "disabled",
                FavoriteSearchIndexState::Unavailable => "unavailable",
            };
            HttpResponse::json(&result.value)
                .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
                .with_header("Cache-Control", "no-store")
                .with_log_details(json!({
                    "favorite_search": {
                        "query_length": query_length,
                        "kind": kind_name,
                        "entry_count": entry_count,
                        "index_state": index_state,
                        "ipc_status": "ok",
                        "ipc_ms": crate::diagnostics::duration_ms(started.elapsed()),
                        "ipc_retry_count": result.retry_count,
                        "ipc_retry_statuses": result.retry_statuses,
                        "ipc_connection_id": result.connection_id,
                    }
                }))
        }
        Err(failure) => {
            collection_ipc_error_response(failure, started.elapsed(), "favorite_search")
        }
    }
}

fn api_tag_browse(state: &AppState, owner: &RemoteSessionIdentity) -> HttpResponse {
    let started = Instant::now();
    let result = match state
        .ipc_admission
        .run(IpcClass::Heavy, || state.thumbnail_client.tag_browse(owner))
    {
        Ok(result) => result,
        Err(busy) => return collection_admission_busy_response(busy, "tag_browse"),
    };
    match result {
        Ok(result) => {
            let index_state = tag_index_state_name(result.value.state);
            let all_count = result.value.all.len();
            HttpResponse::json(&result.value)
                .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
                .with_header("Cache-Control", "no-store")
                .with_log_details(json!({
                    "tag_browse": {
                        "all_count": all_count,
                        "index_state": index_state,
                        "ipc_status": "ok",
                        "ipc_ms": crate::diagnostics::duration_ms(started.elapsed()),
                        "ipc_retry_count": result.retry_count,
                        "ipc_retry_statuses": result.retry_statuses,
                        "ipc_connection_id": result.connection_id,
                    }
                }))
        }
        Err(failure) => collection_ipc_error_response(failure, started.elapsed(), "tag_browse"),
    }
}

fn api_tag_items(
    state: &AppState,
    query: &[(String, String)],
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let tag = match required_query_value(query, "tag") {
        Ok(value) => value,
        Err(()) => {
            return HttpResponse::text(400, "Bad Request").with_header("Cache-Control", "no-store");
        }
    };
    let kind_name = match required_query_value(query, "kind") {
        Ok(value) => value,
        Err(()) => {
            return HttpResponse::text(400, "Bad Request").with_header("Cache-Control", "no-store");
        }
    };
    let kind = match kind_name {
        "all" => TagItemKind::All,
        "folder" => TagItemKind::Folder,
        "image" => TagItemKind::Image,
        "video" => TagItemKind::Video,
        "audio" => TagItemKind::Audio,
        "zip" => TagItemKind::Zip,
        "pdf" => TagItemKind::Pdf,
        "archive" => TagItemKind::Archive,
        _ => {
            return HttpResponse::text(400, "Bad Request").with_header("Cache-Control", "no-store");
        }
    };
    let tag_length = tag.chars().count();
    let started = Instant::now();
    let result = match state.ipc_admission.run(IpcClass::Heavy, || {
        state.thumbnail_client.tag_items(
            owner,
            TagItemsRequest {
                tag: tag.to_owned(),
                kind,
            },
        )
    }) {
        Ok(result) => result,
        Err(busy) => return collection_admission_busy_response(busy, "tag_items"),
    };
    match result {
        Ok(mut result) => {
            state
                .library
                .retain_allowed_remote_entries(&mut result.value.listing.entries);
            let entry_count = result.value.listing.entries.len();
            let index_state = tag_index_state_name(result.value.state);
            HttpResponse::json(&result.value)
                .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
                .with_header("Cache-Control", "no-store")
                .with_log_details(json!({
                    "tag_items": {
                        "tag_length": tag_length,
                        "kind": kind_name,
                        "entry_count": entry_count,
                        "index_state": index_state,
                        "ipc_status": "ok",
                        "ipc_ms": crate::diagnostics::duration_ms(started.elapsed()),
                        "ipc_retry_count": result.retry_count,
                        "ipc_retry_statuses": result.retry_statuses,
                        "ipc_connection_id": result.connection_id,
                    }
                }))
        }
        Err(failure) => collection_ipc_error_response(failure, started.elapsed(), "tag_items"),
    }
}

fn tag_index_state_name(state: TagIndexState) -> &'static str {
    match state {
        TagIndexState::Ready => "ready",
        TagIndexState::Empty => "empty",
        TagIndexState::Unavailable => "unavailable",
    }
}

fn collection_admission_busy_response(busy: AdmissionBusy, kind: &str) -> HttpResponse {
    HttpResponse::bytes(
        503,
        "application/json; charset=utf-8",
        serde_json::to_vec(&json!({
            "error": "ipc_busy",
            "message": "mIV 本体への要求が混み合っています。しばらく待って再試行してください。",
        }))
        .unwrap_or_default(),
    )
    .with_header("Retry-After", IPC_RETRY_AFTER_SECONDS.to_string())
    .with_header("Cache-Control", "no-store")
    .with_log_details(json!({
        "collection": {
            "kind": kind,
            "ipc_status": "admission_busy",
            "ipc_ms": 0,
            "ipc_retry_count": 0,
            "ipc_retry_statuses": [],
            "ipc_all_in_flight": busy.all_in_flight,
            "ipc_all_limit": busy.all_limit,
            "ipc_heavy_in_flight": busy.heavy_in_flight,
            "ipc_heavy_limit": busy.heavy_limit,
            "entry_count": 0,
        }
    }))
}

fn collection_ipc_error_response(
    failure: crate::ipc_client::ClientFailure,
    elapsed: Duration,
    kind: &str,
) -> HttpResponse {
    let ipc_status = failure.error.ipc_status();
    let protocol_stage = failure
        .error
        .protocol_failure()
        .map(|detail| detail.stage.to_owned());
    let protocol_error_kind = failure
        .error
        .protocol_failure()
        .map(|detail| detail.kind.to_owned());
    let timed_out = failure
        .error
        .protocol_failure()
        .is_some_and(|detail| detail.kind == "timeout");
    let (status, code, message) = match failure.error {
        IpcClientError::Unavailable(_) => (
            503,
            "miv_not_running",
            "mIV 本体が起動していません。mIV を起動してください。".to_owned(),
        ),
        IpcClientError::VersionMismatch { client, server } => (
            503,
            "protocol_version_mismatch",
            format!(
                "mIV 本体と remote-web の IPC 版が一致しません (remote-web={client}, mIV={server})。"
            ),
        ),
        IpcClientError::Protocol(detail) if detail.kind == "timeout" => (
            503,
            "ipc_timeout",
            "mIV 本体の応答が時間内に完了しませんでした。再試行してください。".to_owned(),
        ),
        IpcClientError::Protocol(_) => (
            502,
            "ipc_protocol_error",
            "mIV 本体との通信に失敗しました。".to_owned(),
        ),
        IpcClientError::CollectionRemote(error) => {
            let status = match error.code {
                CollectionErrorCode::BadRequest => 400,
                CollectionErrorCode::NotFound => 404,
                CollectionErrorCode::Busy => 503,
                CollectionErrorCode::Internal => 500,
            };
            (status, "miv_collection_error", error.message)
        }
        IpcClientError::MediaRemote(error) => (500, "miv_collection_error", error.message),
        IpcClientError::Remote(error) => (500, "miv_collection_error", error.message),
        IpcClientError::WriteRemote(error) => (500, "miv_collection_error", error.message),
        IpcClientError::VideoStreamRemote(error) => (500, "miv_collection_error", error.message),
        IpcClientError::RemoteAi(error) => (500, "miv_collection_error", error.message),
        IpcClientError::SessionRemote(response) => (
            session_http_status(response.status),
            "session_required",
            response.message,
        ),
    };
    let mut response = HttpResponse::bytes(
        status,
        "application/json; charset=utf-8",
        serde_json::to_vec(&json!({"error": code, "message": message})).unwrap_or_default(),
    )
    .with_header("Cache-Control", "no-store");
    if timed_out {
        response = response.with_header("Retry-After", IPC_RETRY_AFTER_SECONDS.to_string());
    }
    response.with_log_details(json!({
        "collection": {
            "kind": kind,
            "ipc_status": ipc_status,
            "ipc_ms": crate::diagnostics::duration_ms(elapsed),
            "ipc_retry_count": failure.retry_count,
            "ipc_retry_statuses": failure.retry_statuses,
            "ipc_stage": protocol_stage,
            "ipc_error_kind": protocol_error_kind,
            "entry_count": 0,
        }
    }))
}

fn api_list(
    state: &AppState,
    query: &[(String, String)],
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let Some((root, path)) = root_and_path(query) else {
        return HttpResponse::text(400, "Bad Request");
    };
    let address = RemoteAddress::file(root.to_string(), path);
    if let Err(error) = state.library.validate_remote_address(&address) {
        return store_error_response(error);
    }
    let started = Instant::now();
    let result = match state.ipc_admission.run(IpcClass::Browse, || {
        state.thumbnail_client.folder_list(owner, address)
    }) {
        Ok(result) => result,
        Err(busy) => return media_admission_busy_response(busy, "list"),
    };
    match result {
        Ok(mut result) => {
            if state
                .library
                .validate_remote_address(&result.value.effective_address)
                .is_err()
            {
                return HttpResponse::text(502, "Bad Gateway");
            }
            let core_entry_count = result.value.entries.len();
            state
                .library
                .retain_allowed_folder_list_entries(&mut result.value.entries);
            let payload = result.value;
            let entries = payload
                .entries
                .iter()
                .map(|entry| {
                    json!({
                        "kind": folder_list_entry_kind(entry.kind),
                        "name": entry.name,
                        "path": entry.address.relative_path,
                        "size": entry.size,
                        "mtime": entry.mtime,
                        "address": entry.address,
                        "thumbnail_address": entry.thumbnail_address,
                    })
                })
                .collect::<Vec<_>>();
            let entry_count = entries.len();
            let zip_count = payload
                .entries
                .iter()
                .filter(|entry| entry.kind == RemoteEntryKind::Zip)
                .count();
            let pdf_count = payload
                .entries
                .iter()
                .filter(|entry| entry.kind == RemoteEntryKind::Pdf)
                .count();
            let response = json!({
                "root_id": payload.effective_address.root_id,
                "path": payload.effective_address.relative_path,
                "thumb_aspect_height_ratio": payload.thumb_aspect_height_ratio,
                "sort_state": payload.sort_state,
                "entries": entries,
            });
            match HttpResponse::json(&response) {
                Ok(response) => response
                    .with_header("Cache-Control", "no-store")
                    .with_log_details(json!({
                        "list": {
                            "ipc_status": "ok",
                            "ipc_ms": crate::diagnostics::duration_ms(started.elapsed()),
                            "ipc_retry_count": result.retry_count,
                            "ipc_retry_statuses": result.retry_statuses,
                            "ipc_connection_id": result.connection_id,
                            "entry_count": entry_count,
                            "core_entry_count": core_entry_count,
                            "zip_count": zip_count,
                            "pdf_count": pdf_count,
                            "scan_ms": payload.scan_ms,
                            "materialize_ms": payload.materialize_ms,
                        }
                    })),
                Err(error) => {
                    eprintln!("remote-web: list JSON encoding failed: {error}");
                    HttpResponse::text(500, "Internal Server Error")
                }
            }
        }
        Err(failure) => media_ipc_error_response(failure, started.elapsed(), "list", None),
    }
}

fn folder_list_entry_kind(kind: RemoteEntryKind) -> &'static str {
    match kind {
        RemoteEntryKind::Folder => "dir",
        RemoteEntryKind::Image => "image",
        RemoteEntryKind::Video => "video",
        RemoteEntryKind::Audio => "audio",
        RemoteEntryKind::Zip => "zip",
        RemoteEntryKind::Pdf => "pdf",
        RemoteEntryKind::Archive => "archive",
        RemoteEntryKind::Other => "other",
    }
}

fn api_container(
    state: &AppState,
    query: &[(String, String)],
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let address = match remote_address_from_query(query) {
        Ok(address)
            if matches!(
                address.subresource,
                RemoteSubresource::File | RemoteSubresource::ZipDirectory { .. }
            ) =>
        {
            address
        }
        _ => return HttpResponse::text(400, "Bad Request"),
    };
    if let Err(error) = state.library.validate_remote_address(&address) {
        return store_error_response(error);
    }
    let spread_mode = match parse_spread_mode(query) {
        Ok(value) => value,
        Err(()) => return HttpResponse::text(400, "Bad Request"),
    };
    let force_single_page = match parse_force_single_page(query) {
        Ok(value) => value,
        Err(()) => return HttpResponse::text(400, "Bad Request"),
    };
    let reading_direction = match parse_reading_direction(query) {
        Ok(value) => value,
        Err(()) => return HttpResponse::text(400, "Bad Request"),
    };
    let started = Instant::now();
    let result = match state.ipc_admission.run(IpcClass::Heavy, || {
        state.thumbnail_client.container(
            owner,
            address.clone(),
            spread_mode,
            reading_direction,
            force_single_page,
        )
    }) {
        Ok(result) => result,
        Err(busy) => return media_admission_busy_response(busy, "container"),
    };
    match result {
        Ok(mut result) => {
            if state
                .library
                .validate_remote_address(&result.value.effective_address)
                .is_err()
            {
                return HttpResponse::text(502, "Bad Gateway");
            }
            state
                .library
                .retain_allowed_container_entries(&mut result.value.entries);
            if result.value.page_groups.iter().any(|group| {
                state
                    .library
                    .validate_remote_address(&group.anchor)
                    .is_err()
                    || group
                        .pages
                        .iter()
                        .any(|address| state.library.validate_remote_address(address).is_err())
            }) {
                return HttpResponse::text(502, "Bad Gateway");
            }
            let entry_count = result.value.entries.len();
            HttpResponse::json(&result.value)
                .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
                .with_header("Cache-Control", "no-store")
                .with_log_details(json!({
                    "container": {
                        "ipc_status": "ok",
                        "ipc_ms": crate::diagnostics::duration_ms(started.elapsed()),
                        "ipc_retry_count": result.retry_count,
                        "ipc_retry_statuses": result.retry_statuses,
                        "ipc_connection_id": result.connection_id,
                        "entry_count": entry_count,
                        "group_count": result.value.page_groups.len(),
                        "configured_spread": result.value.configured_spread_mode,
                        "effective_spread": result.value.effective_spread_mode,
                        "reading_direction": result.value.reading_direction,
                        "force_single_page": force_single_page,
                        "truncated": result.value.truncated,
                    }
                }))
        }
        Err(failure) => media_ipc_error_response(failure, started.elapsed(), "container", None),
    }
}

fn api_write(
    request: &mut Request,
    state: &AppState,
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let body = match read_body_limited(request, MAX_WRITE_BODY_BYTES) {
        Ok(body) => body,
        Err(BodyReadError::TooLarge) => return HttpResponse::text(413, "Payload Too Large"),
        Err(BodyReadError::Read) => return HttpResponse::text(400, "Bad Request"),
    };
    let write_request: RemoteWriteRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return HttpResponse::text(400, "Bad Request"),
    };
    if let Some(address) = write_request.address()
        && let Err(error) = state.library.validate_remote_address(address)
    {
        return store_error_response(error);
    }
    if let Some(context_address) = write_request.context_address()
        && let Err(error) = state.library.validate_remote_address(context_address)
    {
        return store_error_response(error);
    }
    let write_kind = write_request.kind_name();
    let started = Instant::now();
    let result = match state.ipc_admission.run(IpcClass::Home, || {
        state.thumbnail_client.write(owner, write_request)
    }) {
        Ok(result) => result,
        Err(busy) => return write_admission_busy_response(busy, write_kind),
    };
    match result {
        Ok(result) => {
            let remote_state_generation = match state.library.remote_state() {
                Ok(state) => state.remote_state_generation,
                Err(error) => return store_error_response(error),
            };
            HttpResponse::json(&json!({
                "applied": true,
                "item_state": result.value.item_state,
                "adjustment_state": result.value.adjustment_state,
                "book_bookmarks": result.value.book_bookmarks,
                "view_trim_state": result.value.view_trim_state,
                "sort_state": result.value.sort_state,
                "remote_state_generation": remote_state_generation,
            }))
            .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
            .with_header("Cache-Control", "no-store")
            .with_log_details(json!({
                "write": {
                    "kind": write_kind,
                    "ipc_status": "ok",
                    "ipc_ms": crate::diagnostics::duration_ms(started.elapsed()),
                    "ipc_connection_id": result.connection_id,
                }
            }))
        }
        Err(failure) => write_ipc_error_response(failure, started.elapsed(), write_kind),
    }
}

fn api_thumb(
    state: &AppState,
    query: &[(String, String)],
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let address = match remote_address_from_query(query) {
        Ok(address) => address,
        Err(()) => return HttpResponse::text(400, "Bad Request"),
    };
    if let Err(error) = state.library.validate_remote_address(&address) {
        return store_error_response(error);
    }
    let target_px = match requested_width(query) {
        Ok(width) => width,
        Err(()) => return HttpResponse::text(400, "Bad Request"),
    };
    let source_kind = remote_source_kind(&address);
    let started = Instant::now();
    let result = match state.ipc_admission.run(IpcClass::Heavy, || {
        state
            .thumbnail_client
            .thumbnail_address(owner, address.clone(), target_px)
    }) {
        Ok(result) => result,
        Err(busy) => {
            return thumbnail_admission_busy_response(busy, target_px, source_kind);
        }
    };
    match result {
        Ok(result) => {
            let ipc_ms = crate::diagnostics::duration_ms(started.elapsed());
            let blob_bytes = result.bytes.len();
            HttpResponse::bytes(200, "image/webp", result.bytes)
                .with_header("Cache-Control", "private, max-age=60")
                .with_log_details(json!({
                    "thumb": {
                        "cache_tier": "miv_ipc",
                        "ipc_status": "ok",
                        "ipc_ms": ipc_ms,
                        "ipc_retry_count": result.retry_count,
                        "ipc_retry_statuses": result.retry_statuses,
                        "ipc_connection_id": result.connection_id,
                        "target_px": target_px,
                        "address_kind": remote_address_kind(&address),
                        "source_kind": source_kind,
                        "blob_bytes": blob_bytes,
                    }
                }))
        }
        Err(failure) => ipc_error_response(
            failure.error,
            failure.retry_count,
            failure.retry_statuses,
            started.elapsed(),
            target_px,
            source_kind,
        ),
    }
}

fn api_page(
    state: &AppState,
    query: &[(String, String)],
    owner: &RemoteSessionIdentity,
) -> HttpResponse {
    let remote_state_generation = match query_value(query, "generation") {
        Ok(Some(value))
            if (1..=128).contains(&value.len())
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') =>
        {
            value.to_owned()
        }
        _ => return HttpResponse::text(400, "Bad Request"),
    };
    if let Err(error) = state
        .library
        .require_remote_state_generation(&remote_state_generation)
    {
        return store_error_response(error);
    }
    let address = match remote_address_from_query(query) {
        Ok(address) => address,
        _ => return HttpResponse::text(400, "Bad Request"),
    };
    if let Err(error) = validate_page_address(&state.library, &address) {
        return store_error_response(error);
    }
    let target_px = match requested_width(query) {
        Ok(width) => width,
        Err(()) => return HttpResponse::text(400, "Bad Request"),
    };
    let priority = match query_value(query, "prefetch") {
        Ok(None) => PagePriority::Foreground,
        Ok(Some("1")) => PagePriority::Prefetch,
        _ => return HttpResponse::text(400, "Bad Request"),
    };
    let adjustment_preview = match query_value(query, "adjustment_preview") {
        Ok(None) => None,
        Ok(Some(value)) => match serde_json::from_str(value) {
            Ok(preview) => Some(preview),
            Err(_) => return HttpResponse::text(400, "Bad Request"),
        },
        Err(()) => return HttpResponse::text(400, "Bad Request"),
    };
    let render_context = match query_value(query, "render_context") {
        Ok(None) => None,
        Ok(Some(value)) => match serde_json::from_str::<RemotePageRenderContext>(value) {
            Ok(context) => Some(context),
            Err(_) => return HttpResponse::text(400, "Bad Request"),
        },
        Err(()) => return HttpResponse::text(400, "Bad Request"),
    };
    if let Some(render_context) = render_context.as_ref()
        && let Err(error) = state
            .library
            .validate_remote_address(&render_context.context_address)
    {
        return store_error_response(error);
    }
    if let Some(spread_partner) = render_context
        .as_ref()
        .and_then(|context| context.spread_partner.as_ref())
        && let Err(error) = state.library.validate_remote_address(spread_partner)
    {
        return store_error_response(error);
    }
    let ipc_class = if priority == PagePriority::Prefetch {
        IpcClass::Prefetch
    } else {
        IpcClass::Heavy
    };
    let started = Instant::now();
    let result = match state.ipc_admission.run(ipc_class, || {
        state.thumbnail_client.page(
            owner,
            address.clone(),
            target_px,
            priority,
            render_context.clone(),
            adjustment_preview.clone(),
        )
    }) {
        Ok(result) => result,
        Err(busy) => return media_admission_busy_response(busy, "page"),
    };
    if let Err(error) = state
        .library
        .require_remote_state_generation(&remote_state_generation)
    {
        return store_error_response(error);
    }
    match result {
        Ok(result) => {
            let payload = result.value;
            let blob_bytes = payload.bytes.len();
            if payload.identity.validate_syntax().is_err() {
                return HttpResponse::text(502, "Bad Gateway");
            }
            let Some(identity) = remote_page_identity_header_value(&payload.identity) else {
                return HttpResponse::text(502, "Bad Gateway");
            };
            let Some(content_type) = remote_page_content_type(&payload.content_type) else {
                return HttpResponse::text(502, "Bad Gateway");
            };
            HttpResponse::bytes(200, content_type, payload.bytes)
                .with_header(
                    "Cache-Control",
                    if adjustment_preview.is_some() {
                        "no-store"
                    } else {
                        "private, max-age=60"
                    },
                )
                .with_header("X-mIV-Image-Width", payload.width.to_string())
                .with_header("X-mIV-Image-Height", payload.height.to_string())
                .with_header("X-mIV-Page-Identity", identity)
                .with_header("X-mIV-Remote-State-Generation", remote_state_generation)
                .with_header("X-mIV-Remote-Session", owner.session_id.clone())
                .with_header("Vary", "X-mIV-Remote-Session")
                .with_log_details(json!({
                    "page": {
                        "ipc_status": "ok",
                        "ipc_ms": crate::diagnostics::duration_ms(started.elapsed()),
                        "ipc_retry_count": result.retry_count,
                        "ipc_retry_statuses": result.retry_statuses,
                        "ipc_connection_id": result.connection_id,
                        "address_kind": remote_address_kind(&address),
                        "priority": if priority == PagePriority::Prefetch { "prefetch" } else { "foreground" },
                        "target_px": target_px,
                        "output_width": payload.width,
                        "output_height": payload.height,
                        "content_type": payload.content_type,
                        "blob_bytes": blob_bytes,
                    }
                }))
        }
        Err(failure) => {
            media_ipc_error_response(failure, started.elapsed(), "page", Some(target_px))
        }
    }
}

fn validate_page_address(library: &Library, address: &RemoteAddress) -> Result<(), StoreError> {
    // Every accepted page, including an ordinary file, stays inside the existing favorite-root
    // path guard. The core performs the format decode; remote-web only admits file kinds that the
    // same list endpoint classifies as images.
    library.validate_remote_address(address)?;
    match &address.subresource {
        RemoteSubresource::File => library.validate_remote_file_image(address),
        RemoteSubresource::ZipEntry { .. } | RemoteSubresource::PdfPage { .. } => Ok(()),
        RemoteSubresource::ZipDirectory { .. } => Err(StoreError::BadRequest),
    }
}

fn write_admission_busy_response(busy: AdmissionBusy, write_kind: &str) -> HttpResponse {
    HttpResponse::bytes(
        503,
        "application/json; charset=utf-8",
        serde_json::to_vec(&json!({
            "error": "ipc_busy",
            "message": "mIV 本体への書き込み要求が混み合っています。再試行してください。",
        }))
        .unwrap_or_default(),
    )
    .with_header("Retry-After", IPC_RETRY_AFTER_SECONDS.to_string())
    .with_header("Cache-Control", "no-store")
    .with_log_details(json!({
        "write": {
            "kind": write_kind,
            "ipc_status": "admission_busy",
            "ipc_ms": 0,
            "ipc_all_in_flight": busy.all_in_flight,
            "ipc_all_limit": busy.all_limit,
        }
    }))
}

fn media_admission_busy_response(busy: AdmissionBusy, operation: &str) -> HttpResponse {
    HttpResponse::bytes(
        503,
        "application/json; charset=utf-8",
        serde_json::to_vec(&json!({
            "error": "ipc_busy",
            "message": "mIV 本体への要求が混み合っています。自動的に再試行します。",
        }))
        .unwrap_or_default(),
    )
    .with_header("Retry-After", IPC_RETRY_AFTER_SECONDS.to_string())
    .with_header("Cache-Control", "no-store")
    .with_log_details(json!({
        (operation): {
            "ipc_status": "admission_busy",
            "ipc_ms": 0,
            "ipc_retry_count": 0,
            "ipc_all_in_flight": busy.all_in_flight,
            "ipc_all_limit": busy.all_limit,
            "ipc_heavy_in_flight": busy.heavy_in_flight,
            "ipc_heavy_limit": busy.heavy_limit,
            "ipc_prefetch_in_flight": busy.prefetch_in_flight,
            "ipc_prefetch_limit": busy.prefetch_limit,
        }
    }))
}

fn write_ipc_error_response(
    failure: crate::ipc_client::ClientFailure,
    elapsed: Duration,
    write_kind: &str,
) -> HttpResponse {
    let ipc_status = failure.error.ipc_status();
    let (status, code, message) = match failure.error {
        IpcClientError::Unavailable(_) => (
            503,
            "miv_not_running",
            "mIV 本体が起動していません。".to_owned(),
        ),
        IpcClientError::VersionMismatch { client, server } => (
            503,
            "protocol_version_mismatch",
            format!("IPC 版が一致しません (remote-web={client}, mIV={server})。"),
        ),
        IpcClientError::Protocol(detail) if detail.kind == "timeout" => (
            504,
            "ipc_timeout",
            "書き込み結果を時間内に確認できませんでした。".to_owned(),
        ),
        IpcClientError::Protocol(_) => (
            502,
            "ipc_protocol_error",
            "mIV 本体との通信に失敗しました。".to_owned(),
        ),
        IpcClientError::WriteRemote(error) => {
            let status = match error.code {
                RemoteWriteErrorCode::BadRequest => 400,
                RemoteWriteErrorCode::FavoriteNotFound
                | RemoteWriteErrorCode::PathRejected
                | RemoteWriteErrorCode::NotFound => 404,
                RemoteWriteErrorCode::Unsupported => 415,
                RemoteWriteErrorCode::Busy => 503,
                RemoteWriteErrorCode::UiTimeout => 504,
                RemoteWriteErrorCode::PersistenceFailed | RemoteWriteErrorCode::Internal => 500,
            };
            (status, "miv_write_error", error.message)
        }
        IpcClientError::SessionRemote(response) => (
            session_http_status(response.status),
            "session_required",
            response.message,
        ),
        IpcClientError::Remote(_)
        | IpcClientError::CollectionRemote(_)
        | IpcClientError::MediaRemote(_)
        | IpcClientError::VideoStreamRemote(_)
        | IpcClientError::RemoteAi(_) => (
            500,
            "miv_write_error",
            "書き込みに失敗しました。".to_owned(),
        ),
    };
    HttpResponse::bytes(
        status,
        "application/json; charset=utf-8",
        serde_json::to_vec(&json!({"error": code, "message": message})).unwrap_or_default(),
    )
    .with_header("Cache-Control", "no-store")
    .with_log_details(json!({
        "write": {
            "kind": write_kind,
            "ipc_status": ipc_status,
            "ipc_ms": crate::diagnostics::duration_ms(elapsed),
        }
    }))
}

fn media_ipc_error_response(
    failure: crate::ipc_client::ClientFailure,
    elapsed: Duration,
    operation: &str,
    target_px: Option<u32>,
) -> HttpResponse {
    let ipc_status = failure.error.ipc_status();
    let protocol_stage = failure
        .error
        .protocol_failure()
        .map(|detail| detail.stage.to_owned());
    let protocol_error_kind = failure
        .error
        .protocol_failure()
        .map(|detail| detail.kind.to_owned());
    let mut retryable = failure
        .error
        .protocol_failure()
        .is_some_and(|detail| detail.kind == "timeout");
    let (status, code, message) = match failure.error {
        IpcClientError::Unavailable(_) => (
            503,
            "miv_not_running",
            "mIV 本体が起動していません。mIV を起動してください。".to_owned(),
        ),
        IpcClientError::VersionMismatch { client, server } => (
            503,
            "protocol_version_mismatch",
            format!(
                "mIV 本体と remote-web の IPC 版が一致しません (remote-web={client}, mIV={server})。"
            ),
        ),
        IpcClientError::Protocol(detail) if detail.kind == "timeout" => (
            503,
            "ipc_timeout",
            "mIV 本体の応答が時間内に完了しませんでした。自動的に再試行します。".to_owned(),
        ),
        IpcClientError::Protocol(_) => (
            502,
            "ipc_protocol_error",
            "mIV 本体との通信に失敗しました。".to_owned(),
        ),
        IpcClientError::MediaRemote(error) => {
            let status = match error.code {
                MediaErrorCode::BadRequest => 400,
                MediaErrorCode::FavoriteNotFound
                | MediaErrorCode::PathRejected
                | MediaErrorCode::NotFound => 404,
                MediaErrorCode::Unsupported => 415,
                MediaErrorCode::PasswordRequired => 423,
                MediaErrorCode::PageOutOfRange => 416,
                MediaErrorCode::Busy => {
                    retryable = true;
                    503
                }
                MediaErrorCode::RenderFailed => 422,
                MediaErrorCode::Internal => 500,
            };
            (status, "miv_media_error", error.message)
        }
        IpcClientError::Remote(error) => (500, "miv_media_error", error.message),
        IpcClientError::CollectionRemote(error) => (500, "miv_media_error", error.message),
        IpcClientError::WriteRemote(error) => (500, "miv_media_error", error.message),
        IpcClientError::VideoStreamRemote(error) => (500, "miv_media_error", error.message),
        IpcClientError::RemoteAi(error) => (500, "miv_media_error", error.message),
        IpcClientError::SessionRemote(response) => (
            session_http_status(response.status),
            "session_required",
            response.message,
        ),
    };
    let mut response = HttpResponse::bytes(
        status,
        "application/json; charset=utf-8",
        serde_json::to_vec(&json!({"error": code, "message": message})).unwrap_or_default(),
    )
    .with_header("Cache-Control", "no-store");
    if retryable {
        response = response.with_header("Retry-After", IPC_RETRY_AFTER_SECONDS.to_string());
    }
    response.with_log_details(json!({
        (operation): {
            "ipc_status": ipc_status,
            "ipc_ms": crate::diagnostics::duration_ms(elapsed),
            "ipc_retry_count": failure.retry_count,
            "ipc_retry_statuses": failure.retry_statuses,
            "ipc_stage": protocol_stage,
            "ipc_error_kind": protocol_error_kind,
            "target_px": target_px,
        }
    }))
}

fn thumbnail_admission_busy_response(
    busy: AdmissionBusy,
    target_px: u32,
    source_kind: &str,
) -> HttpResponse {
    HttpResponse::bytes(
        503,
        "application/json; charset=utf-8",
        serde_json::to_vec(&json!({
            "error": "ipc_busy",
            "message": "サムネイル生成が混み合っています。自動的に再試行します。",
        }))
        .unwrap_or_default(),
    )
    .with_header("Retry-After", IPC_RETRY_AFTER_SECONDS.to_string())
    .with_header("Cache-Control", "no-store")
    .with_log_details(json!({
        "thumb": {
            "cache_tier": "miv_ipc",
            "ipc_status": "admission_busy",
            "ipc_ms": 0,
            "ipc_retry_count": 0,
            "ipc_retry_statuses": [],
            "ipc_all_in_flight": busy.all_in_flight,
            "ipc_all_limit": busy.all_limit,
            "ipc_heavy_in_flight": busy.heavy_in_flight,
            "ipc_heavy_limit": busy.heavy_limit,
            "target_px": target_px,
            "source_kind": source_kind,
            "blob_bytes": 0,
        }
    }))
}

fn ipc_error_response(
    error: IpcClientError,
    retry_count: u32,
    retry_statuses: Vec<String>,
    elapsed: Duration,
    target_px: u32,
    source_kind: &str,
) -> HttpResponse {
    use mimageviewer_ipc::ThumbnailErrorCode;

    let ipc_ms = crate::diagnostics::duration_ms(elapsed);
    let ipc_status = error.ipc_status();
    let protocol_stage = error
        .protocol_failure()
        .map(|failure| failure.stage.to_owned());
    let protocol_error_kind = error
        .protocol_failure()
        .map(|failure| failure.kind.to_owned());
    let protocol_os_error = error
        .protocol_failure()
        .and_then(|failure| failure.os_error);
    let timed_out = error
        .protocol_failure()
        .is_some_and(|failure| failure.kind == "timeout");
    let (status, code, message) = match error {
        IpcClientError::Unavailable(_) => (
            503,
            "miv_not_running",
            "mIV 本体が起動していません。mIV を起動してください。".to_owned(),
        ),
        IpcClientError::VersionMismatch { client, server } => (
            503,
            "protocol_version_mismatch",
            format!(
                "mIV 本体と remote-web の IPC 版が一致しません (remote-web={client}, mIV={server})。"
            ),
        ),
        IpcClientError::Protocol(detail) if detail.kind == "timeout" => (
            503,
            "ipc_timeout",
            "mIV 本体の応答が時間内に完了しませんでした。自動的に再試行します。".to_owned(),
        ),
        IpcClientError::Protocol(detail) => {
            eprintln!(
                "remote-web: thumbnail IPC error stage={} kind={} os_error={:?}",
                detail.stage, detail.kind, detail.os_error
            );
            (
                502,
                "ipc_protocol_error",
                "mIV 本体との通信に失敗しました。".to_owned(),
            )
        }
        IpcClientError::Remote(remote) => {
            let status = match remote.code {
                ThumbnailErrorCode::BadRequest => 400,
                ThumbnailErrorCode::FavoriteNotFound
                | ThumbnailErrorCode::PathRejected
                | ThumbnailErrorCode::NotFound => 404,
                ThumbnailErrorCode::Unsupported => 415,
                ThumbnailErrorCode::Busy => 503,
                ThumbnailErrorCode::GenerationFailed => 422,
                ThumbnailErrorCode::PasswordRequired => 423,
                ThumbnailErrorCode::PageOutOfRange => 416,
                ThumbnailErrorCode::Internal => 500,
            };
            (status, "miv_thumbnail_error", remote.message)
        }
        IpcClientError::CollectionRemote(remote) => (500, "miv_thumbnail_error", remote.message),
        IpcClientError::MediaRemote(remote) => (500, "miv_thumbnail_error", remote.message),
        IpcClientError::WriteRemote(remote) => (500, "miv_thumbnail_error", remote.message),
        IpcClientError::VideoStreamRemote(remote) => (500, "miv_thumbnail_error", remote.message),
        IpcClientError::RemoteAi(remote) => (500, "miv_thumbnail_error", remote.message),
        IpcClientError::SessionRemote(response) => (
            session_http_status(response.status),
            "session_required",
            response.message,
        ),
    };
    let body = serde_json::to_vec(&json!({"error": code, "message": message}))
        .unwrap_or_else(|_| b"{\"error\":\"ipc_error\"}".to_vec());
    let mut response = HttpResponse::bytes(status, "application/json; charset=utf-8", body)
        .with_header("Cache-Control", "no-store");
    if timed_out {
        response = response.with_header("Retry-After", IPC_RETRY_AFTER_SECONDS.to_string());
    }
    response.with_log_details(json!({
        "thumb": {
            "cache_tier": "miv_ipc",
            "ipc_status": ipc_status,
            "ipc_ms": ipc_ms,
            "ipc_retry_count": retry_count,
            "ipc_retry_statuses": retry_statuses,
            "ipc_stage": protocol_stage,
            "ipc_error_kind": protocol_error_kind,
            "ipc_os_error": protocol_os_error,
            "target_px": target_px,
            "source_kind": source_kind,
            "blob_bytes": 0,
        }
    }))
}

fn api_image_info(state: &AppState, query: &[(String, String)]) -> HttpResponse {
    let Some((root, path)) = root_and_path(query) else {
        return HttpResponse::text(400, "Bad Request");
    };
    match state.library.image_info(root, path) {
        Ok(value) => HttpResponse::json(&value)
            .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
            .with_header("Cache-Control", "private, max-age=60"),
        Err(error) => store_error_response(error),
    }
}

fn api_image(state: &AppState, query: &[(String, String)]) -> HttpResponse {
    let Some((root, path)) = root_and_path(query) else {
        return HttpResponse::text(400, "Bad Request");
    };
    let width = match required_query_value(query, "w").and_then(|value| {
        value
            .parse::<u32>()
            .map_err(|_| ())
            .and_then(|width| (width > 0).then_some(width).ok_or(()))
    }) {
        Ok(width) => width,
        Err(()) => return HttpResponse::text(400, "Bad Request"),
    };

    match state.library.image(root, path, width) {
        Ok(value) => HttpResponse::bytes(200, value.content_type, value.bytes)
            .with_header("Cache-Control", "private, max-age=60")
            .with_log_details(json!({"image": value.metrics})),
        Err(error) => store_error_response(error),
    }
}

#[derive(Deserialize)]
struct TelemetryBatch {
    #[serde(default)]
    client_timestamp_ms: Option<u64>,
    #[serde(default)]
    connection: Option<Value>,
    events: Vec<Value>,
}

fn api_telemetry(request: &mut Request, state: &AppState) -> HttpResponse {
    if !state.telemetry_limiter.allow(Instant::now()) {
        return HttpResponse::text(429, "Too Many Requests")
            .with_header("Retry-After", "5")
            .with_header("Cache-Control", "no-store");
    }
    let body = match read_body_limited(request, MAX_TELEMETRY_BODY_BYTES) {
        Ok(body) => body,
        Err(BodyReadError::TooLarge) => {
            return HttpResponse::text(413, "Payload Too Large")
                .with_header("Cache-Control", "no-store");
        }
        Err(BodyReadError::Read) => {
            return HttpResponse::text(400, "Bad Request").with_header("Cache-Control", "no-store");
        }
    };
    let batch: TelemetryBatch = match serde_json::from_slice(&body) {
        Ok(batch) => batch,
        Err(_) => {
            return HttpResponse::text(400, "Bad Request").with_header("Cache-Control", "no-store");
        }
    };
    if batch.events.len() > MAX_TELEMETRY_EVENTS {
        return HttpResponse::text(400, "Bad Request").with_header("Cache-Control", "no-store");
    }
    let event_count = batch.events.len();
    let mut telemetry = json!({
        "client_timestamp_ms": batch.client_timestamp_ms,
        "event_count": event_count,
        "events": batch.events,
    });
    if let Some(connection) = batch.connection {
        telemetry["connection"] = connection;
    }
    HttpResponse::bytes(204, "application/json; charset=utf-8", Vec::new())
        .with_header("Cache-Control", "no-store")
        .with_log_details(json!({"telemetry": telemetry}))
}

enum BodyReadError {
    TooLarge,
    Read,
}

fn read_body_limited(request: &mut Request, limit: usize) -> Result<Vec<u8>, BodyReadError> {
    if request.body_length().is_some_and(|length| length > limit) {
        return Err(BodyReadError::TooLarge);
    }
    let mut body = Vec::new();
    request
        .as_reader()
        .take((limit + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|_| BodyReadError::Read)?;
    if body.len() > limit {
        return Err(BodyReadError::TooLarge);
    }
    Ok(body)
}

fn root_and_path(query: &[(String, String)]) -> Option<(Uuid, &str)> {
    let root = required_query_value(query, "root")
        .ok()?
        .parse::<Uuid>()
        .ok()?;
    let path = required_query_value(query, "path").ok()?;
    Some((root, path))
}

fn remote_address_from_query(query: &[(String, String)]) -> Result<RemoteAddress, ()> {
    let (root, path) = root_and_path(query).ok_or(())?;
    let entry = query_value(query, "entry")?;
    let prefix = query_value(query, "prefix")?;
    let page = query_value(query, "page")?;
    if usize::from(entry.is_some()) + usize::from(prefix.is_some()) + usize::from(page.is_some())
        > 1
    {
        return Err(());
    }
    let subresource = if let Some(entry_name) = entry {
        RemoteSubresource::ZipEntry {
            entry_name: entry_name.to_owned(),
        }
    } else if let Some(prefix) = prefix {
        RemoteSubresource::ZipDirectory {
            prefix: prefix.to_owned(),
        }
    } else if let Some(page) = page {
        RemoteSubresource::PdfPage {
            page_number: page.parse::<u32>().map_err(|_| ())?,
        }
    } else {
        RemoteSubresource::File
    };
    let address = RemoteAddress {
        root_id: root.to_string(),
        relative_path: path.to_owned(),
        subresource,
    };
    address.validate_syntax().map_err(|_| ())?;
    Ok(address)
}

fn requested_width(query: &[(String, String)]) -> Result<u32, ()> {
    required_query_value(query, "w").and_then(|value| {
        value
            .parse::<u32>()
            .map_err(|_| ())
            .and_then(|width| (width > 0).then_some(width).ok_or(()))
    })
}

fn parse_spread_mode(query: &[(String, String)]) -> Result<Option<RemoteSpreadMode>, ()> {
    match query_value(query, "spread")? {
        None => Ok(None),
        Some("single") => Ok(Some(RemoteSpreadMode::Single)),
        Some("ltr") => Ok(Some(RemoteSpreadMode::Ltr)),
        Some("ltr_cover") => Ok(Some(RemoteSpreadMode::LtrCover)),
        Some("rtl") => Ok(Some(RemoteSpreadMode::Rtl)),
        Some("rtl_cover") => Ok(Some(RemoteSpreadMode::RtlCover)),
        Some(_) => Err(()),
    }
}

fn parse_force_single_page(query: &[(String, String)]) -> Result<bool, ()> {
    match query_value(query, "single")? {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(_) => Err(()),
    }
}

fn parse_reading_direction(
    query: &[(String, String)],
) -> Result<Option<RemoteReadingDirection>, ()> {
    match query_value(query, "direction")? {
        None => Ok(None),
        Some("ltr") => Ok(Some(RemoteReadingDirection::Ltr)),
        Some("rtl") => Ok(Some(RemoteReadingDirection::Rtl)),
        Some(_) => Err(()),
    }
}

fn remote_address_kind(address: &RemoteAddress) -> &'static str {
    match address.subresource {
        RemoteSubresource::File => "file",
        RemoteSubresource::ZipDirectory { .. } => "zip_directory",
        RemoteSubresource::ZipEntry { .. } => "zip_entry",
        RemoteSubresource::PdfPage { .. } => "pdf_page",
    }
}

fn remote_source_kind(address: &RemoteAddress) -> &'static str {
    match address.subresource {
        RemoteSubresource::ZipDirectory { .. } | RemoteSubresource::ZipEntry { .. } => "zip",
        RemoteSubresource::PdfPage { .. } => "pdf",
        RemoteSubresource::File => match PathBuf::from(&address.relative_path)
            .extension()
            .and_then(|value| value.to_str())
        {
            Some(extension) if extension.eq_ignore_ascii_case("zip") => "zip",
            Some(extension) if extension.eq_ignore_ascii_case("pdf") => "pdf",
            _ => "file",
        },
    }
}

fn web_asset_route_name(path: &str) -> Option<&'static str> {
    let name = path.strip_prefix('/')?;
    if !crate::web_assets::is_distribution_asset(name) {
        return None;
    }
    WEB_ASSET_PATHS
        .binary_search(&name)
        .ok()
        .map(|index| WEB_ASSET_PATHS[index])
}

#[cfg(not(feature = "embedded-web-assets"))]
fn load_web_asset(web_root: &std::path::Path, name: &str) -> Result<Vec<u8>, String> {
    fs::read(web_root.join(name)).map_err(|error| error.to_string())
}

#[cfg(feature = "embedded-web-assets")]
fn load_web_asset(_web_root: &std::path::Path, name: &str) -> Result<Vec<u8>, String> {
    EMBEDDED_WEB_ASSETS
        .iter()
        .find_map(|(asset_name, bytes)| (*asset_name == name).then(|| bytes.to_vec()))
        .ok_or_else(|| format!("embedded asset is missing: {name}"))
}

fn static_file(state: &AppState, name: &str) -> HttpResponse {
    let Some(content_type) = crate::web_assets::content_type(name) else {
        eprintln!("remote-web: static asset {name} has no Content-Type mapping");
        return HttpResponse::text(500, "Internal Server Error");
    };
    match load_web_asset(&state.web_root, name) {
        Ok(bytes) => {
            HttpResponse::bytes(200, content_type, bytes).with_header("Cache-Control", "no-cache")
        }
        Err(error) => {
            eprintln!("remote-web: static asset {name} could not be read: {error}");
            HttpResponse::text(500, "Internal Server Error")
        }
    }
}

const WEB_ASSET_TOKEN_PLACEHOLDER: &str = "__MIV_REMOTE_ASSET_TOKEN__";

/// Bind the shell to the asset tree from which it was loaded. The app-version endpoint describes
/// the tree currently served and therefore cannot identify an already-running script after a
/// deploy.
fn static_index(state: &AppState) -> HttpResponse {
    match load_web_asset(&state.web_root, "index.html")
        .and_then(|bytes| String::from_utf8(bytes).map_err(|error| error.to_string()))
    {
        Ok(source) => {
            let asset_token = web_asset_token(&state.web_root);
            if asset_token.is_empty() || !source.contains(WEB_ASSET_TOKEN_PLACEHOLDER) {
                eprintln!("remote-web: index shell has no usable asset token placeholder");
                return HttpResponse::text(500, "Internal Server Error");
            }
            HttpResponse::bytes(
                200,
                "text/html; charset=utf-8",
                source
                    .replacen(WEB_ASSET_TOKEN_PLACEHOLDER, &asset_token, 1)
                    .into_bytes(),
            )
            .with_header("Cache-Control", "no-cache")
        }
        Err(error) => {
            eprintln!("remote-web: static asset index.html could not be read: {error}");
            HttpResponse::text(500, "Internal Server Error")
        }
    }
}

fn store_error_response(error: StoreError) -> HttpResponse {
    match error {
        StoreError::BadRequest => HttpResponse::text(400, "Bad Request"),
        StoreError::Busy => HttpResponse::text(503, "Service Unavailable")
            .with_header("Retry-After", IPC_RETRY_AFTER_SECONDS.to_string())
            .with_header("Cache-Control", "no-store"),
        StoreError::NotFound => HttpResponse::text(404, "Not Found"),
        StoreError::StaleGeneration(remote_state_generation) => HttpResponse::bytes(
            409,
            "application/json; charset=utf-8",
            serde_json::to_vec(&json!({
                "error": "remote_state_generation_mismatch",
                "message": "本体の状態が変わりました。ページを読み直します。",
                "remote_state_generation": remote_state_generation,
            }))
            .unwrap_or_default(),
        )
        .with_header("Cache-Control", "no-store"),
        StoreError::Decode => HttpResponse::text(415, "Unsupported Media Type"),
        StoreError::Io(error) => {
            eprintln!("remote-web: filesystem request failed: {error}");
            HttpResponse::text(500, "Internal Server Error")
        }
        StoreError::Db(error) => {
            eprintln!("remote-web: read-only database request failed: {error}");
            HttpResponse::text(500, "Internal Server Error")
        }
    }
}

fn respond(request: Request, response: HttpResponse) -> std::io::Result<()> {
    let mut tiny = Response::from_data(response.body).with_status_code(StatusCode(response.status));
    tiny.add_header(
        Header::from_bytes("Content-Type", response.content_type)
            .expect("static response header is valid"),
    );
    for (name, value) in response.headers {
        if let Ok(header) = Header::from_bytes(name, value.as_bytes()) {
            tiny.add_header(header);
        }
    }
    request.respond(tiny)
}

fn header_value<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request.headers().iter().find_map(|header| {
        header
            .field
            .as_str()
            .as_str()
            .eq_ignore_ascii_case(name)
            .then(|| header.value.as_str())
    })
}

fn remote_client_header(request: &Request) -> Option<&str> {
    header_value(request, "X-mIV-Remote-Client").filter(|value| {
        (8..=128).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    })
}

fn remote_session_header(request: &Request) -> Option<&str> {
    header_value(request, "X-mIV-Remote-Session")
        .filter(|value| value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn route_requires_remote_session(path: &str) -> bool {
    (path.starts_with("/api/") || path.starts_with("/stream/"))
        && !path.starts_with("/api/auth/")
        && !matches!(
            path,
            "/api/session/acquire" | "/api/app-version" | "/api/telemetry"
        )
}

fn request_is_https(request: &Request) -> bool {
    request.secure()
        || header_value(request, "X-Forwarded-Proto")
            .and_then(|value| value.split(',').next())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("https"))
}

fn forwarded_source_ip(request: &Request) -> Option<std::net::IpAddr> {
    header_value(request, "X-Forwarded-For")?
        .split(',')
        .next()?
        .trim()
        .parse()
        .ok()
}

fn request_proxy_details(request: &Request) -> Value {
    let forwarded_proto = header_value(request, "X-Forwarded-Proto").map(limit_log_value);
    let https_source = if request.secure() {
        "direct_tls"
    } else if request_is_https(request) {
        "x_forwarded_proto"
    } else {
        "plain_http"
    };
    let mut tailscale_user_headers = serde_json::Map::new();
    for header in request.headers() {
        let name = header.field.as_str().as_str();
        if name
            .get(..15)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Tailscale-User-"))
        {
            tailscale_user_headers.insert(
                name.to_owned(),
                Value::String(limit_log_value(header.value.as_str())),
            );
        }
    }
    json!({
        "remote_addr": request.remote_addr().map(ToString::to_string),
        "x_forwarded_for": header_value(request, "X-Forwarded-For").map(limit_log_value),
        "x_forwarded_proto": forwarded_proto,
        "tailscale_user_headers": tailscale_user_headers,
        "https_detected": request_is_https(request),
        "https_source": https_source,
    })
}

fn limit_log_value(value: &str) -> String {
    const MAX_CHARS: usize = 1024;
    let mut chars = value.chars();
    let result: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{result}…")
    } else {
        result
    }
}

fn ceil_seconds(duration: Duration) -> u64 {
    duration.as_secs() + u64::from(duration.subsec_nanos() > 0)
}

fn split_url(url: &str) -> (&str, &str) {
    url.split_once('?').unwrap_or((url, ""))
}

fn parse_query(raw_query: &str) -> Result<Vec<(String, String)>, ()> {
    if raw_query.is_empty() {
        return Ok(Vec::new());
    }
    raw_query
        .split('&')
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            Ok((decode_query_component(key)?, decode_query_component(value)?))
        })
        .collect()
}

fn decode_query_component(value: &str) -> Result<String, ()> {
    let form_value = value.replace('+', " ");
    percent_decode_str(&form_value)
        .decode_utf8()
        .map(|value| value.into_owned())
        .map_err(|_| ())
}

fn required_query_value<'a>(query: &'a [(String, String)], key: &str) -> Result<&'a str, ()> {
    query_value(query, key)?.ok_or(())
}

fn query_value<'a>(query: &'a [(String, String)], key: &str) -> Result<Option<&'a str>, ()> {
    let mut values = query
        .iter()
        .filter_map(|(candidate, value)| (candidate == key).then_some(value.as_str()));
    let first = values.next();
    if values.next().is_some() {
        return Err(());
    }
    Ok(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthService, AuthToken, COOKIE_NAME, load_pin_file, set_pin_file};
    use tiny_http::TestRequest;

    const TEST_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn test_state(temp: &tempfile::TempDir) -> AppState {
        let protected = temp.path().join("data");
        std::fs::create_dir_all(&protected).unwrap();
        let auth_path = temp.path().join("auth.json");
        set_pin_file(&auth_path, &[protected.clone()], "123456").unwrap();
        let loaded = load_pin_file(&auth_path, &[protected.clone()]).unwrap();
        let auth = AuthService::new(
            loaded.record,
            AuthToken::from_printable_for_test(TEST_TOKEN),
        )
        .unwrap();
        let log_secrets = auth.permanent_log_secrets();
        let thumbnail_client = Arc::new(ThumbnailClient::new());
        let session_activity =
            SessionActivityNotifier::start(Arc::clone(&thumbnail_client)).unwrap();
        AppState {
            auth,
            library: Library::empty_for_test(temp.path().join("cache")),
            thumbnail_client,
            session_activity,
            ipc_admission: IpcAdmission::new(),
            logger: DiagnosticsLogger::open(
                &temp.path().join("request.jsonl"),
                &[protected],
                &log_secrets,
            )
            .unwrap(),
            telemetry_limiter: TelemetryLimiter::new(),
            request_sequence: AtomicU64::new(1),
            web_root: temp.path().to_owned(),
            session_peers: Mutex::new(HashMap::new()),
            remote_client_identities: RemoteClientIdentities::default(),
        }
    }

    fn cookie_header(state: &AppState) -> Header {
        let cookie = state.auth.issue_session_cookie(true, false);
        Header::from_bytes(
            "Cookie",
            format!("{COOKIE_NAME}={}", cookie.sensitive_value).as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn query_parser_decodes_form_encoding_and_rejects_invalid_utf8() {
        let query = parse_query("path=A%2FB+set&name=%E5%A4%8F").unwrap();
        assert_eq!(
            query,
            vec![
                ("path".to_owned(), "A/B set".to_owned()),
                ("name".to_owned(), "夏".to_owned())
            ]
        );
        assert!(parse_query("path=%ff").is_err());
    }

    #[test]
    fn duplicate_security_parameters_are_rejected() {
        let query = parse_query("root=a&root=b").unwrap();
        assert!(query_value(&query, "root").is_err());
    }

    #[test]
    fn container_spread_query_accepts_only_protocol_modes_and_explicit_orientation() {
        let query = parse_query("spread=rtl_cover&direction=rtl&single=1").unwrap();
        assert_eq!(
            parse_spread_mode(&query).unwrap(),
            Some(RemoteSpreadMode::RtlCover)
        );
        assert_eq!(
            parse_reading_direction(&query).unwrap(),
            Some(RemoteReadingDirection::Rtl)
        );
        assert!(parse_force_single_page(&query).unwrap());
        assert!(parse_spread_mode(&parse_query("spread=vertical").unwrap()).is_err());
        assert!(parse_force_single_page(&parse_query("single=true").unwrap()).is_err());
        assert!(parse_reading_direction(&parse_query("direction=vertical").unwrap()).is_err());
        assert!(parse_spread_mode(&parse_query("spread=ltr&spread=rtl").unwrap()).is_err());
    }

    #[test]
    fn remote_address_query_rejects_zip_traversal_and_mixed_targets() {
        let id = "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2";
        let traversal =
            parse_query(&format!("root={id}&path=book.zip&entry=..%2Fsecret.jpg")).unwrap();
        assert!(remote_address_from_query(&traversal).is_err());

        let mixed = parse_query(&format!("root={id}&path=book.pdf&page=1&entry=page.jpg")).unwrap();
        assert!(remote_address_from_query(&mixed).is_err());

        let valid = parse_query(&format!(
            "root={id}&path=book.zip&entry=chapter.zip%2F001.jpg"
        ))
        .unwrap();
        assert!(matches!(
            remote_address_from_query(&valid).unwrap().subresource,
            RemoteSubresource::ZipEntry { entry_name }
                if entry_name == "chapter.zip/001.jpg"
        ));
    }

    #[test]
    fn page_identity_header_is_ascii_and_round_trips_unicode_relative_paths() {
        let identity = RemoteAddress {
            root_id: "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2".to_owned(),
            relative_path: "本棚/第一巻.pdf".to_owned(),
            subresource: RemoteSubresource::PdfPage { page_number: 1 },
        };
        let header = remote_page_identity_header_value(&identity).unwrap();
        assert!(header.is_ascii());
        let decoded = percent_decode_str(&header).decode_utf8().unwrap();
        assert_eq!(
            serde_json::from_str::<RemoteAddress>(&decoded).unwrap(),
            identity
        );
        assert!(!header.contains("session"));
        assert!(!header.contains("token"));
    }

    #[cfg(not(feature = "embedded-web-assets"))]
    #[test]
    fn command_core_module_is_public_for_the_pin_screen_bootstrap() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        std::fs::write(temp.path().join("command-core.mjs"), b"export {};").unwrap();
        let mut request: Request = TestRequest::new()
            .with_method(Method::Get)
            .with_path("/command-core.mjs")
            .into();
        let response = route(&mut request, &state);
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"export {};");
    }

    #[cfg(not(feature = "embedded-web-assets"))]
    #[test]
    fn local_settings_module_is_public_for_the_app_bootstrap() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        std::fs::write(temp.path().join("local-settings.mjs"), b"export {};").unwrap();
        let mut request: Request = TestRequest::new()
            .with_method(Method::Get)
            .with_path("/local-settings.mjs")
            .into();
        let response = route(&mut request, &state);
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"export {};");
    }

    #[cfg(not(feature = "embedded-web-assets"))]
    #[test]
    fn video_stream_modules_are_public_shell_assets_with_exact_routes() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        std::fs::create_dir_all(temp.path().join("vendor")).unwrap();
        std::fs::write(temp.path().join("video-stream.mjs"), b"export {};").unwrap();
        std::fs::write(temp.path().join("vendor/hls.min.js"), b"window.Hls={};").unwrap();

        for (path, expected) in [
            ("/video-stream.mjs", b"export {};".as_slice()),
            ("/vendor/hls.min.js", b"window.Hls={};".as_slice()),
        ] {
            let mut request: Request = TestRequest::new()
                .with_method(Method::Get)
                .with_path(path)
                .into();
            let response = route(&mut request, &state);
            assert_eq!(response.status, 200, "{path}");
            assert_eq!(response.content_type, "text/javascript; charset=utf-8");
            assert_eq!(response.body, expected);
        }

        let mut prefix_request: Request = TestRequest::new()
            .with_method(Method::Get)
            .with_path("/vendor/hls.min.js/extra")
            .into();
        assert_eq!(route(&mut prefix_request, &state).status, 401);
    }

    #[cfg(not(feature = "embedded-web-assets"))]
    #[test]
    fn generated_web_assets_are_public_before_and_after_authentication() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let source_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("web");
        for asset in WEB_ASSET_PATHS {
            let destination = temp.path().join(asset);
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            std::fs::copy(source_root.join(asset), destination).unwrap();
        }

        for authenticated in [false, true] {
            for asset in WEB_ASSET_PATHS {
                let path = if *asset == "index.html" {
                    "/".to_owned()
                } else {
                    format!("/{asset}")
                };
                let mut request = TestRequest::new().with_method(Method::Get).with_path(&path);
                if authenticated {
                    request = request.with_header(cookie_header(&state));
                }
                let mut request: Request = request.into();
                let response = route(&mut request, &state);
                assert_eq!(response.status, 200, "{path} authenticated={authenticated}");
                assert_eq!(
                    response.content_type,
                    crate::web_assets::content_type(asset).unwrap(),
                    "{path}"
                );
                assert!(!response.body.is_empty(), "{path}");
                assert!(
                    response
                        .headers
                        .iter()
                        .any(|(name, value)| { *name == "Cache-Control" && value == "no-cache" })
                );
            }
        }

        for alias in [
            "/index.html",
            "/apple-touch-icon.png",
            "/apple-touch-icon-precomposed.png",
        ] {
            let mut request: Request = TestRequest::new()
                .with_method(Method::Get)
                .with_path(alias)
                .into();
            assert_eq!(route(&mut request, &state).status, 200, "{alias}");
        }

        let mut protected_request: Request = TestRequest::new()
            .with_method(Method::Get)
            .with_path("/api/favorites")
            .into();
        assert_eq!(route(&mut protected_request, &state).status, 401);
    }

    #[cfg(feature = "embedded-web-assets")]
    #[test]
    fn distribution_serves_the_complete_embedded_shell_without_a_web_directory() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        assert_eq!(
            EMBEDDED_WEB_ASSETS
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>(),
            WEB_ASSET_PATHS
        );
        for asset in WEB_ASSET_PATHS {
            let path = if *asset == "index.html" {
                "/".to_owned()
            } else {
                format!("/{asset}")
            };
            let mut request: Request = TestRequest::new()
                .with_method(Method::Get)
                .with_path(&path)
                .into();
            let response = route(&mut request, &state);
            assert_eq!(response.status, 200, "{path}");
            assert_eq!(
                response.content_type,
                crate::web_assets::content_type(asset).unwrap(),
                "{path}"
            );
            assert!(!response.body.is_empty(), "{path}");
        }

        let token = web_asset_token(&state.web_root);
        assert!(!token.is_empty());
        let mut index_request: Request = TestRequest::new()
            .with_method(Method::Get)
            .with_path("/")
            .into();
        let index = route(&mut index_request, &state);
        let index = String::from_utf8(index.body).unwrap();
        assert!(index.contains(&format!(r#"content="{token}""#)));
        assert!(!index.contains(WEB_ASSET_TOKEN_PLACEHOLDER));
    }

    #[test]
    fn page_entry_guard_accepts_folder_zip_and_pdf_pages_but_rejects_non_images() {
        let temp = tempfile::tempdir().unwrap();
        let favorite_id = Uuid::from_u128(0x1234567890abcdef1234567890abcdef);
        for path in [
            "page.jpg",
            "book.zip",
            "book.pdf",
            "movie.mp4",
            "song.mp3",
            "notes.txt",
        ] {
            std::fs::write(temp.path().join(path), b"fixture").unwrap();
        }
        std::fs::create_dir(temp.path().join("not-a-page.jpg")).unwrap();
        let library = Library::with_favorite_for_test(favorite_id, temp.path().to_owned());
        let root_id = favorite_id.to_string();

        let folder_page = RemoteAddress::file(root_id.clone(), "page.jpg");
        let zip_page = RemoteAddress {
            root_id: root_id.clone(),
            relative_path: "book.zip".to_owned(),
            subresource: RemoteSubresource::ZipEntry {
                entry_name: "001.jpg".to_owned(),
            },
        };
        let pdf_page = RemoteAddress {
            root_id: root_id.clone(),
            relative_path: "book.pdf".to_owned(),
            subresource: RemoteSubresource::PdfPage { page_number: 0 },
        };

        assert!(validate_page_address(&library, &folder_page).is_ok());
        assert!(validate_page_address(&library, &zip_page).is_ok());
        assert!(validate_page_address(&library, &pdf_page).is_ok());
        assert!(matches!(
            validate_page_address(&library, &RemoteAddress::file(root_id.clone(), "movie.mp4")),
            Err(StoreError::BadRequest)
        ));
        assert!(matches!(
            validate_page_address(&library, &RemoteAddress::file(root_id.clone(), "song.mp3")),
            Err(StoreError::BadRequest)
        ));
        assert!(matches!(
            validate_page_address(&library, &RemoteAddress::file(root_id.clone(), "notes.txt")),
            Err(StoreError::BadRequest)
        ));
        assert!(matches!(
            validate_page_address(
                &library,
                &RemoteAddress::file(root_id.clone(), "not-a-page.jpg")
            ),
            Err(StoreError::BadRequest)
        ));
        assert!(matches!(
            validate_page_address(&library, &RemoteAddress::file(root_id, "../outside.jpg")),
            Err(StoreError::BadRequest)
        ));
    }

    #[test]
    fn video_entry_guard_accepts_only_contained_video_files() {
        let temp = tempfile::tempdir().unwrap();
        let favorite_id = Uuid::from_u128(0x1234567890abcdef1234567890abcdef);
        std::fs::write(temp.path().join("movie.mp4"), b"fixture").unwrap();
        std::fs::write(temp.path().join("page.jpg"), b"fixture").unwrap();
        std::fs::create_dir(temp.path().join("folder.mp4")).unwrap();
        let library = Library::with_favorite_for_test(favorite_id, temp.path().to_owned());
        let root_id = favorite_id.to_string();

        assert!(
            library
                .validate_remote_file_video(&RemoteAddress::file(root_id.clone(), "movie.mp4",))
                .is_ok()
        );
        for path in ["page.jpg", "folder.mp4", "../movie.mp4"] {
            assert!(
                library
                    .validate_remote_file_video(&RemoteAddress::file(root_id.clone(), path,))
                    .is_err(),
                "{path}",
            );
        }
    }

    #[test]
    fn video_seek_thumbnail_http_preserves_pending_and_actual_frame_pts() {
        let pending = video_thumbnail_http_response(VideoStreamThumbnailPayload::Pending);
        assert_eq!(pending.status, 202);

        let ready = video_thumbnail_http_response(VideoStreamThumbnailPayload::Ready {
            actual_pts_secs: 12.466,
            width: 320,
            height: 180,
            webp_bytes: vec![1, 2, 3],
        });
        assert_eq!(ready.status, 200);
        assert_eq!(ready.content_type, "image/webp");
        assert_eq!(ready.body, vec![1, 2, 3]);
        assert!(ready.headers.iter().any(|(name, value)| {
            *name == "X-mIV-Video-Thumbnail-PTS" && value == "12.466000000"
        }));
    }

    #[test]
    fn video_jump_thumbnail_is_private_for_60_seconds_and_missing_is_not_cached() {
        let found = video_jump_thumbnail_http_response(VideoStreamJumpThumbnailPayload::Found {
            webp_bytes: vec![4, 5, 6],
        });
        assert_eq!(found.status, 200);
        assert_eq!(found.content_type, "image/webp");
        assert_eq!(found.body, vec![4, 5, 6]);
        assert!(
            found.headers.iter().any(|(name, value)| {
                *name == "Cache-Control" && value == "private, max-age=60"
            })
        );

        let missing = video_jump_thumbnail_http_response(VideoStreamJumpThumbnailPayload::Missing);
        assert_eq!(missing.status, 404);
        assert!(
            missing
                .headers
                .iter()
                .any(|(name, value)| { *name == "Cache-Control" && value == "no-store" })
        );
    }

    #[test]
    fn remote_page_and_ai_result_http_accept_only_the_jpeg_payload_contract() {
        assert_eq!(remote_page_content_type("image/jpeg"), Some("image/jpeg"));
        assert_eq!(remote_page_content_type("image/webp"), None);
        assert_eq!(remote_page_content_type("image/png"), None);
    }

    #[test]
    fn write_ui_timeout_is_an_explicit_gateway_timeout_without_retry() {
        let response = write_ipc_error_response(
            crate::ipc_client::ClientFailure {
                error: IpcClientError::WriteRemote(mimageviewer_ipc::RemoteWriteError::new(
                    RemoteWriteErrorCode::UiTimeout,
                    "本体 UI が応答しませんでした",
                )),
                retry_count: 0,
                retry_statuses: Vec::new(),
            },
            Duration::from_secs(2),
            "set_spread",
        );
        assert_eq!(response.status, 504);
        assert!(
            !response
                .headers
                .iter()
                .any(|(name, _)| *name == "Retry-After")
        );
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["error"], "miv_write_error");
        assert!(body["message"].as_str().unwrap().contains("UI"));
    }

    #[test]
    fn telemetry_rejects_an_unauthenticated_request() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let mut request: Request = TestRequest::new()
            .with_method(Method::Post)
            .with_path("/api/telemetry")
            .with_body(r#"{"events":[]}"#)
            .into();
        let response = route(&mut request, &state);
        assert_eq!(response.status, 401);
        assert_eq!(response.body, b"Unauthorized");
    }

    #[cfg(not(feature = "embedded-web-assets"))]
    #[test]
    fn the_asset_token_changes_when_an_asset_changes_and_needs_authentication() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        std::fs::write(
            state.web_root.join("index.html"),
            format!(
                r#"<!doctype html><meta name="miv-remote-asset-token" content="{WEB_ASSET_TOKEN_PLACEHOLDER}">"#
            ),
        )
        .unwrap();
        std::fs::write(state.web_root.join("app.js"), b"const build = 1;").unwrap();
        let before = web_asset_token(&state.web_root);
        assert!(!before.is_empty());
        assert_eq!(
            before,
            web_asset_token(&state.web_root),
            "an unchanged tree keeps its token, so polling does not cry wolf"
        );

        std::fs::write(state.web_root.join("app.js"), b"const build = 2222;").unwrap();
        let after_top_level_change = web_asset_token(&state.web_root);
        assert_ne!(before, after_top_level_change);

        std::fs::create_dir(state.web_root.join("vendor")).unwrap();
        std::fs::write(
            state.web_root.join("vendor").join("hls.VERSION.txt"),
            b"1.0",
        )
        .unwrap();
        assert_ne!(
            web_asset_token(&state.web_root),
            after_top_level_change,
            "a nested generated asset must participate in the running version"
        );

        let expected_shell_token = web_asset_token(&state.web_root);
        let acquire_response = session_acquire_response_http(
            SessionResponse {
                status: SessionStatus::Active,
                message: String::new(),
                session_id: Some("0123456789abcdef0123456789abcdef".to_owned()),
            },
            &state,
        );
        let acquire_body: serde_json::Value =
            serde_json::from_slice(&acquire_response.body).unwrap();
        assert_eq!(acquire_body["asset_token"], expected_shell_token);
        let ordinary_response = session_response_http(
            SessionResponse {
                status: SessionStatus::Active,
                message: String::new(),
                session_id: None,
            },
            &state,
        );
        let ordinary_body: serde_json::Value =
            serde_json::from_slice(&ordinary_response.body).unwrap();
        assert!(
            ordinary_body.get("asset_token").is_none(),
            "asset metadata reads belong to acquisition, not every video/session response"
        );

        let mut shell_request: Request = TestRequest::new()
            .with_method(Method::Get)
            .with_path("/")
            .into();
        let shell_response = route(&mut shell_request, &state);
        let shell = String::from_utf8(shell_response.body).unwrap();
        assert_eq!(shell_response.status, 200);
        assert!(shell.contains(&format!(r#"content="{expected_shell_token}""#)));
        assert!(!shell.contains(WEB_ASSET_TOKEN_PLACEHOLDER));

        let mut request: Request = TestRequest::new()
            .with_method(Method::Get)
            .with_path("/api/app-version")
            .into();
        assert_eq!(route(&mut request, &state).status, 401);
    }

    #[test]
    fn every_video_stream_and_ai_route_is_below_the_fail_closed_auth_guard() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let requests = [
            (
                Method::Post,
                "/api/video/start",
                r#"{"root":"00000000-0000-0000-0000-000000000000","path":"movie.mp4","quality":"standard"}"#,
            ),
            (
                Method::Post,
                "/api/video/control",
                r#"{"session":1,"action":"play"}"#,
            ),
            (
                Method::Post,
                "/api/video/seek",
                r#"{"session":1,"position_secs":12.5}"#,
            ),
            (
                Method::Post,
                "/api/video/thumbnail",
                r#"{"session":1,"position_secs":12.5}"#,
            ),
            (Method::Get, "/api/video/state?session=1", ""),
            (Method::Get, "/api/video/jumps?session=1", ""),
            (
                Method::Get,
                "/api/video/jump-thumbnail?session=1&token=v1%3Apin%3A1%3Aabc",
                "",
            ),
            (Method::Post, "/api/video/stop", r#"{"session":1}"#),
            (Method::Get, "/stream/1/1/index.m3u8", ""),
            (Method::Get, "/stream/1/1/media.m3u8", ""),
            (Method::Get, "/stream/1/1/init.mp4", ""),
            (Method::Get, "/stream/1/1/0.m4s", ""),
            // AI job 経路も同じ guard の下に置く。開始は GPU 推論を起こし、state /
            // result は画像そのものを返すため、video / stream と同じ扱いにする。
            (
                Method::Post,
                "/api/ai/jobs",
                r#"{"request_id":"r1","pages":[]}"#,
            ),
            (Method::Get, "/api/ai/jobs?recoverable=1", ""),
            (Method::Get, "/api/ai/jobs/1-1", ""),
            (Method::Get, "/api/ai/jobs/1-1/result?page=0", ""),
            (Method::Delete, "/api/ai/jobs/1-1", ""),
        ];

        for (method, path, body) in requests {
            let mut request: Request = TestRequest::new()
                .with_method(method)
                .with_path(path)
                .with_body(body)
                .into();
            let response = route(&mut request, &state);
            assert_eq!(response.status, 401, "{path}");
            assert_eq!(response.body, b"Unauthorized", "{path}");
        }
    }

    #[test]
    fn stream_request_without_client_header_uses_same_cookie_session_owner() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let cookie = cookie_header(&state);
        let client_header =
            Header::from_bytes("X-mIV-Remote-Client", "browser-client-1234").unwrap();
        let start_request: Request = TestRequest::new()
            .with_method(Method::Post)
            .with_path("/api/video/start")
            .with_header(cookie.clone())
            .with_header(client_header)
            .into();
        let start_auth = state.auth.authorize(AuthInput {
            authorization: header_value(&start_request, "Authorization"),
            cookie: header_value(&start_request, "Cookie"),
        });
        assert_eq!(
            state
                .remote_client_identities
                .resolve(&start_request, start_auth),
            "browser-client-1234"
        );
        let owner = RemoteSessionIdentity {
            client_id: "browser-client-1234".to_owned(),
            session_id: "0123456789abcdef0123456789abcdef".to_owned(),
        };
        state
            .remote_client_identities
            .bind_session(start_auth, &owner);

        let stream_request: Request = TestRequest::new()
            .with_method(Method::Get)
            .with_path("/stream/1/1/index.m3u8")
            .with_header(cookie)
            .into();
        let stream_auth = state.auth.authorize(AuthInput {
            authorization: header_value(&stream_request, "Authorization"),
            cookie: header_value(&stream_request, "Cookie"),
        });
        assert_eq!(
            state
                .remote_client_identities
                .resolve(&stream_request, stream_auth),
            "browser-client-1234"
        );
        assert_eq!(
            state.remote_client_identities.resolve_session(
                &stream_request,
                stream_auth,
                "browser-client-1234",
            ),
            Some(owner)
        );
    }

    #[test]
    fn authenticated_dynamic_request_without_acquisition_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let mut request: Request = TestRequest::new()
            .with_method(Method::Get)
            .with_path("/api/favorites")
            .with_header(cookie_header(&state))
            .into();
        let response = route(&mut request, &state);
        assert_eq!(response.status, 428);
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["status"], "not_acquired");
    }

    #[test]
    fn stream_request_without_authentication_remains_unauthorized() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let mut request: Request = TestRequest::new()
            .with_method(Method::Get)
            .with_path("/stream/1/1/index.m3u8")
            .into();
        let response = route(&mut request, &state);
        assert_eq!(response.status, 401);
        assert_eq!(response.body, b"Unauthorized");
    }

    #[test]
    fn video_stream_http_statuses_keep_not_found_gone_conflict_and_unavailable_distinct() {
        assert_eq!(
            segment_http_response(
                VideoStreamSegmentIndex::Media { sequence: 8 },
                VideoStreamSegmentPayload::NotFound,
            )
            .status,
            404
        );
        assert_eq!(
            segment_http_response(
                VideoStreamSegmentIndex::Media { sequence: 8 },
                VideoStreamSegmentPayload::Gone,
            )
            .status,
            410
        );

        for (code, expected_error) in [
            (
                VideoStreamErrorCode::SessionMismatch,
                "stream_session_mismatch",
            ),
            (
                VideoStreamErrorCode::GenerationMismatch,
                "stream_generation_mismatch",
            ),
        ] {
            let mismatch = video_ipc_error_response(crate::ipc_client::ClientFailure {
                error: IpcClientError::VideoStreamRemote(mimageviewer_ipc::VideoStreamError::new(
                    code, "mismatch",
                )),
                retry_count: 0,
                retry_statuses: Vec::new(),
            });
            assert_eq!(mismatch.status, 409);
            let body: Value = serde_json::from_slice(&mismatch.body).unwrap();
            assert_eq!(body["error"], expected_error);
            assert_eq!(
                mismatch.log_details.as_ref().unwrap()["video_stream"]["error_code"],
                expected_error
            );
        }
        for (code, expected_error) in [
            (
                VideoStreamErrorCode::StartPlayerTimeout,
                "stream_start_player_timeout",
            ),
            (
                VideoStreamErrorCode::StartEncoderTimeout,
                "stream_start_encoder_timeout",
            ),
        ] {
            let timeout = video_ipc_error_response(crate::ipc_client::ClientFailure {
                error: IpcClientError::VideoStreamRemote(mimageviewer_ipc::VideoStreamError::new(
                    code,
                    "start stage deadline",
                )),
                retry_count: 0,
                retry_statuses: Vec::new(),
            });
            assert_eq!(timeout.status, 504);
            let body: Value = serde_json::from_slice(&timeout.body).unwrap();
            assert_eq!(body["error"], expected_error);
            assert_eq!(
                body["message"],
                "動画を開始できませんでした。もう一度お試しください。"
            );
            assert!(
                !body["message"]
                    .as_str()
                    .unwrap()
                    .contains("start stage deadline")
            );
            assert_eq!(
                timeout.log_details.as_ref().unwrap()["video_stream"]["error_code"],
                expected_error
            );
            assert_eq!(
                timeout.log_details.as_ref().unwrap()["video_stream"]["internal_message"],
                "start stage deadline"
            );
        }

        let worker_reason = "video tap disconnected";
        let worker_failure = video_ipc_error_response(crate::ipc_client::ClientFailure {
            error: IpcClientError::VideoStreamRemote(mimageviewer_ipc::VideoStreamError::new(
                VideoStreamErrorCode::Failed,
                worker_reason,
            )),
            retry_count: 0,
            retry_statuses: Vec::new(),
        });
        assert_eq!(worker_failure.status, 422);
        let body: Value = serde_json::from_slice(&worker_failure.body).unwrap();
        assert_eq!(body["error"], "stream_failed");
        assert!(
            !body["message"].as_str().unwrap().contains(worker_reason),
            "the worker reason belongs in diagnostics, not the user-facing response"
        );
        assert_eq!(
            worker_failure.log_details.as_ref().unwrap()["video_stream"]["internal_message"],
            worker_reason
        );

        let unavailable = video_ipc_error_response(crate::ipc_client::ClientFailure {
            error: IpcClientError::Unavailable(crate::ipc_client::ProtocolFailure::new(
                "connect",
                "not_found",
                Some(2),
                "not found",
            )),
            retry_count: 0,
            retry_statuses: Vec::new(),
        });
        assert_eq!(unavailable.status, 503);
        let body: Value = serde_json::from_slice(&unavailable.body).unwrap();
        assert_eq!(body["error"], "miv_not_running");

        let not_ready = video_ipc_error_response(crate::ipc_client::ClientFailure {
            error: IpcClientError::VideoStreamRemote(mimageviewer_ipc::VideoStreamError::new(
                VideoStreamErrorCode::NotReady,
                "playlist not ready",
            )),
            retry_count: 0,
            retry_statuses: Vec::new(),
        });
        assert_eq!(not_ready.status, 503);
        assert!(
            not_ready
                .headers
                .iter()
                .any(|(name, value)| { *name == "Retry-After" && value == "1" })
        );
    }

    #[test]
    fn video_stream_user_messages_never_expose_internal_start_terms() {
        let codes = [
            VideoStreamErrorCode::BadRequest,
            VideoStreamErrorCode::FavoriteNotFound,
            VideoStreamErrorCode::PathRejected,
            VideoStreamErrorCode::NotFound,
            VideoStreamErrorCode::Unsupported,
            VideoStreamErrorCode::SessionMismatch,
            VideoStreamErrorCode::GenerationMismatch,
            VideoStreamErrorCode::NotReady,
            VideoStreamErrorCode::Busy,
            VideoStreamErrorCode::UiTimeout,
            VideoStreamErrorCode::Failed,
            VideoStreamErrorCode::StartQueueTimeout,
            VideoStreamErrorCode::StartUiTimeout,
            VideoStreamErrorCode::StartPlayerTimeout,
            VideoStreamErrorCode::StartSeekTimeout,
            VideoStreamErrorCode::StartEncoderTimeout,
            VideoStreamErrorCode::StartPlaylistTimeout,
            VideoStreamErrorCode::ResourceTimeout,
            VideoStreamErrorCode::Internal,
        ];
        let forbidden = [
            "player",
            "seek",
            "encoder",
            "playlist",
            "deadline",
            "budget",
            "内部状態",
            "秒以内",
            "状態が一致",
        ];

        for code in codes {
            let message = video_stream_user_message(code);
            for term in forbidden {
                assert!(
                    !message.contains(term),
                    "{code:?} user message exposed internal term {term:?}: {message}"
                );
            }
        }
    }

    #[test]
    fn video_stream_success_responses_can_never_have_an_empty_body() {
        for response in [
            playlist_http_response(String::new()),
            segment_http_response(
                VideoStreamSegmentIndex::Init,
                VideoStreamSegmentPayload::Found(Vec::new()),
            ),
            segment_http_response(
                VideoStreamSegmentIndex::Media { sequence: 1 },
                VideoStreamSegmentPayload::Found(Vec::new()),
            ),
        ] {
            assert_eq!(response.status, 503);
            assert!(!response.body.is_empty());
        }

        let init = segment_http_response(
            VideoStreamSegmentIndex::Init,
            VideoStreamSegmentPayload::Found(vec![1, 2, 3]),
        );
        assert_eq!(init.status, 200);
        assert!(!init.body.is_empty());
        assert!(
            init.headers
                .iter()
                .any(|(name, value)| { *name == "Cache-Control" && value.contains("immutable") })
        );
        let media = segment_http_response(
            VideoStreamSegmentIndex::Media { sequence: 1 },
            VideoStreamSegmentPayload::Found(vec![4, 5, 6]),
        );
        assert_eq!(media.status, 200);
        assert!(
            media
                .headers
                .iter()
                .any(|(name, value)| { *name == "Cache-Control" && value == "no-store" })
        );
    }

    #[test]
    fn saturated_streaming_leaves_thumbnail_and_folder_list_capacity_available() {
        let admission = IpcAdmission::new();
        let stream_permits = (0..MAX_CONCURRENT_STREAM_IPC)
            .map(|_| admission.try_enter(IpcClass::Stream).unwrap())
            .collect::<Vec<_>>();
        let thumbnail = admission.try_enter(IpcClass::Heavy).unwrap();
        let folder_list = admission.try_enter(IpcClass::Browse).unwrap();

        drop(folder_list);
        drop(thumbnail);
        drop(stream_permits);
    }
    #[test]
    fn disconnected_thumbnail_ipc_returns_a_user_visible_service_error() {
        let response = ipc_error_response(
            IpcClientError::Unavailable(crate::ipc_client::ProtocolFailure::new(
                "connect",
                "not_found",
                Some(2),
                "not found",
            )),
            2,
            vec![
                "connect_not_found".to_owned(),
                "connect_not_found".to_owned(),
            ],
            Duration::from_millis(12),
            256,
            "zip",
        );
        assert_eq!(response.status, 503);
        assert_eq!(response.content_type, "application/json; charset=utf-8");
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["error"], "miv_not_running");
        assert!(body["message"].as_str().unwrap().contains("mIV 本体"));
        let details = response.log_details.unwrap();
        assert_eq!(details["thumb"]["ipc_status"], "connect_not_found");
        assert_eq!(details["thumb"]["ipc_retry_count"], 2);
        assert_eq!(
            details["thumb"]["ipc_retry_statuses"],
            json!(["connect_not_found", "connect_not_found"])
        );
        assert_eq!(details["thumb"]["ipc_stage"], "connect");
        assert_eq!(details["thumb"]["ipc_error_kind"], "not_found");
        assert_eq!(details["thumb"]["target_px"], 256);
    }

    #[test]
    fn media_password_required_is_preserved_as_http_423() {
        let response = media_ipc_error_response(
            crate::ipc_client::ClientFailure {
                error: IpcClientError::MediaRemote(mimageviewer_ipc::MediaError::new(
                    MediaErrorCode::PasswordRequired,
                    "この PDF はパスワード保護されています",
                )),
                retry_count: 0,
                retry_statuses: Vec::new(),
            },
            Duration::from_millis(5),
            "page",
            Some(2048),
        );
        assert_eq!(response.status, 423);
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert!(body["message"].as_str().unwrap().contains("パスワード保護"));
    }

    #[test]
    fn thumbnail_diagnostics_distinguish_container_source_without_logging_a_path() {
        let root_id = Uuid::nil().to_string();
        let zip = RemoteAddress::file(root_id.clone(), "books/volume.ZIP");
        let pdf = RemoteAddress::file(root_id, "books/volume.pdf");
        assert_eq!(remote_source_kind(&zip), "zip");
        assert_eq!(remote_source_kind(&pdf), "pdf");
        assert_eq!(remote_address_kind(&zip), "file");
    }

    #[test]
    fn saturated_ipc_does_not_block_an_ipc_free_endpoint() {
        let temp = tempfile::tempdir().unwrap();
        let state = Arc::new(test_state(&temp));
        let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let mut blocked = Vec::new();
        for index in 0..MAX_CONCURRENT_IPC {
            let state = Arc::clone(&state);
            let release = Arc::clone(&release);
            let ready_tx = ready_tx.clone();
            blocked.push(std::thread::spawn(move || {
                let class = if index < MAX_CONCURRENT_HEAVY_IPC {
                    IpcClass::Heavy
                } else {
                    IpcClass::Home
                };
                state
                    .ipc_admission
                    .run(class, || {
                        ready_tx.send(()).unwrap();
                        let (lock, ready) = &*release;
                        let mut released = lock.lock().unwrap();
                        while !*released {
                            released = ready.wait(released).unwrap();
                        }
                    })
                    .unwrap();
            }));
        }
        for _ in 0..MAX_CONCURRENT_IPC {
            ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        }

        let mut request: Request = TestRequest::new()
            .with_method(Method::Get)
            .with_path("/api/app-version")
            .with_header(cookie_header(&state))
            .into();
        let started = Instant::now();
        let response = route(&mut request, &state);
        assert_eq!(response.status, 200);
        assert!(started.elapsed() < Duration::from_millis(50));

        let (lock, ready) = &*release;
        *lock.lock().unwrap() = true;
        ready.notify_all();
        for thread in blocked {
            thread.join().unwrap();
        }
    }

    #[test]
    fn excess_thumbnail_ipc_is_rejected_immediately_as_retryable() {
        let admission = IpcAdmission::new();
        let permits = (0..MAX_CONCURRENT_HEAVY_IPC)
            .map(|_| admission.try_enter(IpcClass::Heavy).unwrap())
            .collect::<Vec<_>>();
        let _browse = admission.try_enter(IpcClass::Browse).unwrap();
        let started = Instant::now();
        let busy = match admission.try_enter(IpcClass::Heavy) {
            Ok(_) => panic!("heavy IPC admission unexpectedly succeeded"),
            Err(busy) => busy,
        };
        assert!(started.elapsed() < Duration::from_millis(50));
        let response = thumbnail_admission_busy_response(busy, 256, "zip");
        assert_eq!(response.status, 503);
        assert!(
            response
                .headers
                .iter()
                .any(|(name, value)| *name == "Retry-After" && value == "1")
        );
        assert_eq!(
            response.log_details.unwrap()["thumb"]["ipc_status"],
            "admission_busy"
        );

        let response = media_admission_busy_response(busy, "container");
        let details = response.log_details.unwrap();
        assert_eq!(details["container"]["ipc_status"], "admission_busy");
        assert!(details.get("operation").is_none());
        drop(permits);
    }

    #[test]
    fn page_prefetch_keeps_one_heavy_slot_for_foreground() {
        let admission = IpcAdmission::new();
        let existing = (0..MAX_CONCURRENT_HEAVY_IPC - 2)
            .map(|_| admission.try_enter(IpcClass::Heavy).unwrap())
            .collect::<Vec<_>>();
        let prefetch = admission.try_enter(IpcClass::Prefetch).unwrap();
        assert!(admission.try_enter(IpcClass::Prefetch).is_err());
        let foreground = admission.try_enter(IpcClass::Heavy).unwrap();
        assert!(admission.try_enter(IpcClass::Heavy).is_err());
        drop(foreground);
        drop(prefetch);
        drop(existing);
    }

    #[test]
    fn ipc_timeout_is_retryable_and_releases_the_admission_slot() {
        let admission = IpcAdmission::new();
        let result = admission.run(IpcClass::Heavy, || {
            Err::<(), _>(IpcClientError::Protocol(
                crate::ipc_client::ProtocolFailure::new(
                    "response_read",
                    "timeout",
                    None,
                    "test timeout",
                ),
            ))
        });
        assert!(matches!(result, Ok(Err(IpcClientError::Protocol(_)))));
        let permit = admission.try_enter(IpcClass::Heavy).unwrap();
        drop(permit);

        let response = ipc_error_response(
            IpcClientError::Protocol(crate::ipc_client::ProtocolFailure::new(
                "response_read",
                "timeout",
                None,
                "test timeout",
            )),
            0,
            Vec::new(),
            Duration::from_secs(10),
            256,
            "file",
        );
        assert_eq!(response.status, 503);
        assert!(
            response
                .headers
                .iter()
                .any(|(name, value)| *name == "Retry-After" && value == "1")
        );
        assert_eq!(
            response.log_details.unwrap()["thumb"]["ipc_status"],
            "response_read_timeout"
        );

        let response = media_ipc_error_response(
            crate::ipc_client::ClientFailure {
                error: IpcClientError::Protocol(crate::ipc_client::ProtocolFailure::new(
                    "response_read",
                    "timeout",
                    None,
                    "test timeout",
                )),
                retry_count: 2,
                retry_statuses: vec!["response_read_timeout".to_owned()],
            },
            Duration::from_secs(10),
            "page",
            Some(2048),
        );
        let details = response.log_details.unwrap();
        assert_eq!(details["page"]["ipc_status"], "response_read_timeout");
        assert_eq!(details["page"]["target_px"], 2048);
        assert!(details.get("operation").is_none());
    }

    #[test]
    fn telemetry_rejects_a_declared_body_over_64_kib() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let content_length = Header::from_bytes(
            "Content-Length",
            (MAX_TELEMETRY_BODY_BYTES + 1).to_string().as_bytes(),
        )
        .unwrap();
        let mut request: Request = TestRequest::new()
            .with_method(Method::Post)
            .with_path("/api/telemetry")
            .with_header(cookie_header(&state))
            .with_header(content_length)
            .with_body(r#"{"events":[]}"#)
            .into();
        let response = route(&mut request, &state);
        assert_eq!(response.status, 413);
    }

    #[test]
    fn telemetry_accepts_cookie_auth_used_by_send_beacon() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let mut request: Request = TestRequest::new()
            .with_method(Method::Post)
            .with_path("/api/telemetry")
            .with_header(cookie_header(&state))
            .with_body(r#"{"client_timestamp_ms":1,"events":[{"type":"image"}]}"#)
            .into();
        let response = route(&mut request, &state);
        assert_eq!(response.status, 204);
        assert_eq!(response.body.len(), 0);
        let details = response.log_details.unwrap();
        assert_eq!(details["telemetry"]["event_count"], 1);
        assert!(details["telemetry"].get("connection").is_none());
    }

    #[test]
    fn telemetry_rejects_invalid_json() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let mut request: Request = TestRequest::new()
            .with_method(Method::Post)
            .with_path("/api/telemetry")
            .with_header(cookie_header(&state))
            .with_body("not json")
            .into();
        let response = route(&mut request, &state);
        assert_eq!(response.status, 400);
    }

    #[test]
    fn telemetry_limiter_caps_requests_per_minute() {
        let limiter = TelemetryLimiter::new();
        let now = Instant::now();
        for _ in 0..TELEMETRY_REQUESTS_PER_WINDOW {
            assert!(limiter.allow(now));
        }
        assert!(!limiter.allow(now));
        assert!(limiter.allow(now + TELEMETRY_WINDOW));
    }

    #[test]
    fn pin_login_issues_persistent_cookie_and_authenticates_client() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let forwarded_proto = Header::from_bytes("X-Forwarded-Proto", "https").unwrap();
        let mut request: Request = TestRequest::new()
            .with_method(Method::Post)
            .with_path("/api/auth/pin")
            .with_header(forwarded_proto)
            .with_body(r#"{"pin":"123456","remember":true}"#)
            .into();
        let response = route(&mut request, &state);
        assert_eq!(response.status, 200);
        let set_cookie = response
            .headers
            .iter()
            .find(|(name, _)| *name == "Set-Cookie")
            .map(|(_, value)| value)
            .unwrap();
        assert!(set_cookie.contains("Max-Age=7776000"));
        assert!(set_cookie.contains("; Secure"));
        assert!(!set_cookie.contains("123456"));
    }

    #[test]
    fn session_only_pin_login_omits_max_age() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let mut request: Request = TestRequest::new()
            .with_method(Method::Post)
            .with_path("/api/auth/pin")
            .with_body(r#"{"pin":"123456","remember":false}"#)
            .into();
        let response = route(&mut request, &state);
        let set_cookie = response
            .headers
            .iter()
            .find(|(name, _)| *name == "Set-Cookie")
            .map(|(_, value)| value)
            .unwrap();
        assert!(!set_cookie.contains("Max-Age"));
        assert!(!set_cookie.contains("; Secure"));
    }

    #[test]
    fn failed_pin_attempt_log_contains_count_and_source_but_not_pin() {
        let temp = tempfile::tempdir().unwrap();
        let state = Arc::new(test_state(&temp));
        let request: Request = TestRequest::new()
            .with_method(Method::Post)
            .with_path("/api/auth/pin")
            .with_body(r#"{"pin":"654321","remember":true}"#)
            .into();
        handle(request, &state);
        let log = std::fs::read_to_string(temp.path().join("request.jsonl")).unwrap();
        assert!(log.contains(r#""failure_count":1"#));
        assert!(log.contains(r#""remote_addr":"127.0.0.1:23456""#));
        assert!(!log.contains("654321"));
    }

    #[test]
    fn proxy_headers_and_remote_address_are_exposed_to_request_log_details() {
        let forwarded_for = Header::from_bytes("X-Forwarded-For", "100.64.0.42").unwrap();
        let forwarded_proto = Header::from_bytes("X-Forwarded-Proto", "https").unwrap();
        let tailscale_user =
            Header::from_bytes("Tailscale-User-Login", "alice@example.com").unwrap();
        let request: Request = TestRequest::new()
            .with_remote_addr("127.0.0.1:54321".parse().unwrap())
            .with_header(forwarded_for)
            .with_header(forwarded_proto)
            .with_header(tailscale_user)
            .into();
        let details = request_proxy_details(&request);
        assert_eq!(details["remote_addr"], "127.0.0.1:54321");
        assert_eq!(details["x_forwarded_for"], "100.64.0.42");
        assert_eq!(details["x_forwarded_proto"], "https");
        assert_eq!(
            details["tailscale_user_headers"]["Tailscale-User-Login"],
            "alice@example.com"
        );
        assert_eq!(details["https_detected"], true);
        assert_eq!(details["https_source"], "x_forwarded_proto");
    }

    #[test]
    fn remote_ai_dynamic_routes_are_exact_and_keep_result_page_separate() {
        assert_eq!(ai_state_job_id("/api/ai/jobs/7-1"), Some("7-1"));
        assert_eq!(ai_result_job_id("/api/ai/jobs/7-1/result"), Some("7-1"));
        assert_eq!(ai_state_job_id("/api/ai/jobs/7-1/result"), None);
        assert_eq!(ai_result_job_id("/api/ai/jobs/7-1/result/extra"), None);
        assert_eq!(ai_state_job_id("/api/ai/jobs/"), None);
    }

    #[test]
    fn remote_ai_typed_errors_map_to_stable_http_statuses() {
        let cases = [
            (RemoteAiJobErrorCode::BadRequest, 400, "bad_request"),
            (RemoteAiJobErrorCode::StartExpired, 504, "start_expired"),
            (RemoteAiJobErrorCode::SessionClosing, 409, "session_closing"),
            (RemoteAiJobErrorCode::NotReady, 409, "not_ready"),
            (
                RemoteAiJobErrorCode::PageNotApplicable,
                422,
                "page_not_applicable",
            ),
            (RemoteAiJobErrorCode::NotFound, 404, "not_found"),
            (RemoteAiJobErrorCode::Forbidden, 404, "forbidden"),
            (RemoteAiJobErrorCode::JobGone, 410, "job_gone"),
            (
                RemoteAiJobErrorCode::PageOutOfRange,
                416,
                "page_out_of_range",
            ),
            (RemoteAiJobErrorCode::Internal, 500, "internal"),
        ];
        for (code, expected_status, expected_name) in cases {
            let response = ai_ipc_error_response(crate::ipc_client::ClientFailure {
                error: IpcClientError::RemoteAi(RemoteAiJobError {
                    code,
                    message: "typed failure".to_owned(),
                    terminal_code: Some(mimageviewer_ipc::RemoteAiTerminalCode::VectorPdf),
                }),
                retry_count: 0,
                retry_statuses: Vec::new(),
            });
            assert_eq!(response.status, expected_status, "{code:?}");
            let body: Value = serde_json::from_slice(&response.body).unwrap();
            assert_eq!(body["error"], expected_name);
            assert_eq!(body["terminal_code"], "vector_pdf");
        }
    }
}
