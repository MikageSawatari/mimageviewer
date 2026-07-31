use std::collections::VecDeque;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mimageviewer_ipc::{CollectionErrorCode, CollectionKind};
use percent_encoding::percent_decode_str;
use serde::Deserialize;
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, StatusCode};
use uuid::Uuid;

use crate::auth::{AuthDecision, AuthInput, AuthService, PinVerification};
use crate::diagnostics::{DiagnosticsLogger, RequestLog};
use crate::ipc_client::{ClientError as IpcClientError, ThumbnailClient};
use crate::store::{Library, StoreError};

const MAX_TELEMETRY_BODY_BYTES: usize = 64 * 1024;
const MAX_PIN_BODY_BYTES: usize = 4 * 1024;
const MAX_TELEMETRY_EVENTS: usize = 128;
const TELEMETRY_REQUESTS_PER_WINDOW: usize = 30;
const TELEMETRY_WINDOW: Duration = Duration::from_secs(60);
pub const HTTP_WORKER_COUNT: usize = 12;
pub const MAX_CONCURRENT_IPC: usize = 6;
pub const MAX_CONCURRENT_HEAVY_IPC: usize = 4;
const IPC_RETRY_AFTER_SECONDS: u64 = 1;

pub struct AppState {
    pub auth: AuthService,
    pub library: Library,
    pub thumbnail_client: ThumbnailClient,
    pub ipc_admission: IpcAdmission,
    pub logger: DiagnosticsLogger,
    pub telemetry_limiter: TelemetryLimiter,
    pub request_sequence: AtomicU64,
    pub web_root: PathBuf,
}

pub struct IpcAdmission {
    all: TrySemaphore,
    heavy: TrySemaphore,
}

#[derive(Clone, Copy)]
enum IpcClass {
    Home,
    Heavy,
}

#[derive(Clone, Copy, Debug)]
struct AdmissionBusy {
    all_in_flight: usize,
    all_limit: usize,
    heavy_in_flight: usize,
    heavy_limit: usize,
}

struct TrySemaphore {
    in_flight: AtomicUsize,
    limit: usize,
}

struct TryPermit<'a> {
    semaphore: &'a TrySemaphore,
}

struct IpcPermit<'a> {
    _all: TryPermit<'a>,
    _heavy: Option<TryPermit<'a>>,
}

impl IpcAdmission {
    pub fn new() -> Self {
        assert!(MAX_CONCURRENT_HEAVY_IPC < MAX_CONCURRENT_IPC);
        assert!(MAX_CONCURRENT_IPC < HTTP_WORKER_COUNT);
        Self {
            all: TrySemaphore::new(MAX_CONCURRENT_IPC),
            heavy: TrySemaphore::new(MAX_CONCURRENT_HEAVY_IPC),
        }
    }

    fn try_enter(&self, class: IpcClass) -> Result<IpcPermit<'_>, AdmissionBusy> {
        let all = self.all.try_acquire().ok_or_else(|| self.busy())?;
        let heavy = if matches!(class, IpcClass::Heavy) {
            match self.heavy.try_acquire() {
                Some(permit) => Some(permit),
                None => {
                    drop(all);
                    return Err(self.busy());
                }
            }
        } else {
            None
        };
        Ok(IpcPermit {
            _all: all,
            _heavy: heavy,
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
        let mut current = self.in_flight.load(Ordering::Acquire);
        loop {
            if current >= self.limit {
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
    let query_result = parse_query(raw_query);
    let query = match query_result {
        Ok(query) => query,
        Err(()) => return HttpResponse::text(400, "Bad Request"),
    };

    let method = request.method().clone();
    let auth = state.auth.authorize(AuthInput {
        authorization: header_value(request, "Authorization"),
        cookie: header_value(request, "Cookie"),
    });
    let response = match (method, path) {
        (Method::Get, "/") => static_file(state, "index.html", "text/html; charset=utf-8"),
        (Method::Get, "/app.js") => static_file(state, "app.js", "text/javascript; charset=utf-8"),
        (Method::Get, "/command-core.mjs") => {
            static_file(state, "command-core.mjs", "text/javascript; charset=utf-8")
        }
        (Method::Get, "/styles.css") => static_file(state, "styles.css", "text/css; charset=utf-8"),
        (Method::Get, "/favicon.ico") => HttpResponse::bytes(204, "image/x-icon", Vec::new()),
        (Method::Get, "/api/auth/status") => api_auth_status(state, auth),
        (Method::Post, "/api/auth/pin") => api_auth_pin(request, state),
        (_, "/api/auth/status" | "/api/auth/pin") => {
            HttpResponse::text(405, "Method Not Allowed").with_header("Cache-Control", "no-store")
        }
        _ if auth == AuthDecision::Unauthorized => unauthorized(),
        (Method::Get, "/api/favorites") => api_favorites(state),
        (Method::Get, "/api/home") => api_home(state),
        (Method::Get, "/api/collection") => api_collection(state, &query),
        (Method::Get, "/api/list") => api_list(state, &query),
        (Method::Get, "/api/thumb") => api_thumb(state, &query),
        (Method::Get, "/api/image-info") => api_image_info(state, &query),
        (Method::Get, "/api/image") => api_image(state, &query),
        (Method::Post, "/api/telemetry") => api_telemetry(request, state),
        (Method::Get, _) => HttpResponse::text(404, "Not Found"),
        (_, "/api/telemetry") => HttpResponse::text(405, "Method Not Allowed")
            .with_header("Allow", "POST")
            .with_header("Cache-Control", "no-store"),
        _ => HttpResponse::text(405, "Method Not Allowed")
            .with_header("Allow", "GET")
            .with_header("Cache-Control", "no-store"),
    };

    response
        .with_header("X-Content-Type-Options", "nosniff")
        .with_header("Referrer-Policy", "no-referrer")
}

fn unauthorized() -> HttpResponse {
    HttpResponse::text(401, "Unauthorized")
        .with_header("WWW-Authenticate", "Bearer")
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
    match HttpResponse::json(&state.library.favorites()) {
        Ok(response) => response.with_header("Cache-Control", "no-store"),
        Err(error) => {
            eprintln!("remote-web: favorites JSON encoding failed: {error}");
            HttpResponse::text(500, "Internal Server Error")
        }
    }
}

fn api_home(state: &AppState) -> HttpResponse {
    let started = Instant::now();
    let result = match state
        .ipc_admission
        .run(IpcClass::Home, || state.thumbnail_client.home())
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

fn api_collection(state: &AppState, query: &[(String, String)]) -> HttpResponse {
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
    let result = match state
        .ipc_admission
        .run(IpcClass::Heavy, || state.thumbnail_client.collection(kind))
    {
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
            "mIV 本体が起動していません。mIV を --remote-ipc 付きで起動してください。".to_owned(),
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
        IpcClientError::Remote(error) => (500, "miv_collection_error", error.message),
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

fn api_list(state: &AppState, query: &[(String, String)]) -> HttpResponse {
    let Some((favorite, path)) = favorite_and_path(query) else {
        return HttpResponse::text(400, "Bad Request");
    };
    match state.library.list(favorite, path) {
        Ok(value) => match HttpResponse::json(&value.response) {
            Ok(response) => response
                .with_header("Cache-Control", "no-store")
                .with_log_details(json!({"list": value.metrics})),
            Err(error) => {
                eprintln!("remote-web: list JSON encoding failed: {error}");
                HttpResponse::text(500, "Internal Server Error")
            }
        },
        Err(error) => store_error_response(error),
    }
}

fn api_thumb(state: &AppState, query: &[(String, String)]) -> HttpResponse {
    let Some((favorite, path)) = favorite_and_path(query) else {
        return HttpResponse::text(400, "Bad Request");
    };
    let target_px = match required_query_value(query, "w").and_then(|value| {
        value
            .parse::<u32>()
            .map_err(|_| ())
            .and_then(|width| (width > 0).then_some(width).ok_or(()))
    }) {
        Ok(width) => width,
        Err(()) => return HttpResponse::text(400, "Bad Request"),
    };
    let started = Instant::now();
    let result = match state.ipc_admission.run(IpcClass::Heavy, || {
        state
            .thumbnail_client
            .thumbnail(&favorite.to_string(), path, target_px)
    }) {
        Ok(result) => result,
        Err(busy) => return thumbnail_admission_busy_response(busy, target_px),
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
        ),
    }
}

fn thumbnail_admission_busy_response(busy: AdmissionBusy, target_px: u32) -> HttpResponse {
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
            "mIV 本体が起動していません。mIV を --remote-ipc 付きで起動してください。".to_owned(),
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
                ThumbnailErrorCode::Internal => 500,
            };
            (status, "miv_thumbnail_error", remote.message)
        }
        IpcClientError::CollectionRemote(remote) => (500, "miv_thumbnail_error", remote.message),
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
            "blob_bytes": 0,
        }
    }))
}

fn api_image_info(state: &AppState, query: &[(String, String)]) -> HttpResponse {
    let Some((favorite, path)) = favorite_and_path(query) else {
        return HttpResponse::text(400, "Bad Request");
    };
    match state.library.image_info(favorite, path) {
        Ok(value) => HttpResponse::json(&value)
            .unwrap_or_else(|_| HttpResponse::text(500, "Internal Server Error"))
            .with_header("Cache-Control", "private, max-age=60"),
        Err(error) => store_error_response(error),
    }
}

fn api_image(state: &AppState, query: &[(String, String)]) -> HttpResponse {
    let Some((favorite, path)) = favorite_and_path(query) else {
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

    match state.library.image(favorite, path, width) {
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

fn favorite_and_path(query: &[(String, String)]) -> Option<(Uuid, &str)> {
    let favorite = required_query_value(query, "fav")
        .ok()?
        .parse::<Uuid>()
        .ok()?;
    let path = required_query_value(query, "path").ok()?;
    Some((favorite, path))
}

fn static_file(state: &AppState, name: &str, content_type: &'static str) -> HttpResponse {
    match fs::read(state.web_root.join(name)) {
        Ok(bytes) => {
            HttpResponse::bytes(200, content_type, bytes).with_header("Cache-Control", "no-cache")
        }
        Err(error) => {
            eprintln!("remote-web: static asset {name} could not be read: {error}");
            HttpResponse::text(500, "Internal Server Error")
        }
    }
}

fn store_error_response(error: StoreError) -> HttpResponse {
    match error {
        StoreError::BadRequest => HttpResponse::text(400, "Bad Request"),
        StoreError::NotFound => HttpResponse::text(404, "Not Found"),
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

fn request_is_https(request: &Request) -> bool {
    request.secure()
        || header_value(request, "X-Forwarded-Proto")
            .and_then(|value| value.split(',').next())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("https"))
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
        AppState {
            auth,
            library: Library::empty_for_test(temp.path().join("cache")),
            thumbnail_client: ThumbnailClient::new(),
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
        let query = parse_query("fav=a&fav=b").unwrap();
        assert!(query_value(&query, "fav").is_err());
    }

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
            .with_path("/api/favorites")
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
        let started = Instant::now();
        let busy = match admission.try_enter(IpcClass::Heavy) {
            Ok(_) => panic!("heavy IPC admission unexpectedly succeeded"),
            Err(busy) => busy,
        };
        assert!(started.elapsed() < Duration::from_millis(50));
        let response = thumbnail_admission_busy_response(busy, 256);
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
        drop(permits);
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
}
