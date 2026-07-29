use std::collections::VecDeque;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use percent_encoding::percent_decode_str;
use serde::Deserialize;
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, StatusCode};
use uuid::Uuid;

use crate::auth::{AuthDecision, AuthInput, AuthSource, AuthToken, session_cookie};
use crate::diagnostics::{DiagnosticsLogger, RequestLog};
use crate::store::{Library, StoreError, ThumbnailMissReason};

const MAX_TELEMETRY_BODY_BYTES: usize = 64 * 1024;
const MAX_TELEMETRY_EVENTS: usize = 128;
const TELEMETRY_REQUESTS_PER_WINDOW: usize = 30;
const TELEMETRY_WINDOW: Duration = Duration::from_secs(60);

pub struct AppState {
    pub token: AuthToken,
    pub library: Library,
    pub logger: DiagnosticsLogger,
    pub telemetry_limiter: TelemetryLimiter,
    pub request_sequence: AtomicU64,
    pub web_root: PathBuf,
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
}

impl HttpResponse {
    fn bytes(status: u16, content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type,
            headers: Vec::new(),
            body,
            log_details: None,
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
    let mut response = route(&mut request, state);
    response
        .headers
        .push(("X-mIV-Request-Id", request_id.to_string()));
    let status = response.status;
    let response_bytes = response.body.len();
    let details = response.log_details.take();
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
        details,
    });
    if let Err(error) = response_result {
        eprintln!("remote-web: response write failed: {error}");
    }
}

fn route(request: &mut Request, state: &AppState) -> HttpResponse {
    let (path, raw_query) = split_url(request.url());
    let query_result = parse_query(raw_query);
    let authorization = header_value(request, "Authorization");
    let cookie = header_value(request, "Cookie");
    let query_token = query_result
        .as_ref()
        .ok()
        .and_then(|query| query_value(query, "t").ok().flatten());
    let decision = state.token.authorize(AuthInput {
        authorization,
        cookie,
        query_token,
    });

    let source = match decision {
        AuthDecision::Authorized(source) => source,
        AuthDecision::Unauthorized => {
            return HttpResponse::text(decision.http_status(), "Unauthorized")
                .with_header("WWW-Authenticate", "Bearer")
                .with_header("Cache-Control", "no-store")
                .with_header("X-Content-Type-Options", "nosniff")
                .with_header("Referrer-Policy", "no-referrer");
        }
    };

    let query = match query_result {
        Ok(query) => query,
        Err(()) => return HttpResponse::text(400, "Bad Request"),
    };

    if source == AuthSource::Query {
        let location = url_without_token(path, &query);
        return HttpResponse::text(303, "See Other")
            .with_header("Location", location)
            .with_header("Set-Cookie", session_cookie(&state.token))
            .with_header("Cache-Control", "no-store")
            .with_header("X-Content-Type-Options", "nosniff")
            .with_header("Referrer-Policy", "no-referrer");
    }

    let method = request.method().clone();
    let response = match (method, path) {
        (Method::Get, "/api/favorites") => api_favorites(state),
        (Method::Get, "/api/list") => api_list(state, &query),
        (Method::Get, "/api/thumb") => api_thumb(state, &query),
        (Method::Get, "/api/image") => api_image(state, &query),
        (Method::Post, "/api/telemetry") => api_telemetry(request, state),
        (Method::Get, "/") => static_file(state, "index.html", "text/html; charset=utf-8"),
        (Method::Get, "/app.js") => static_file(state, "app.js", "text/javascript; charset=utf-8"),
        (Method::Get, "/styles.css") => static_file(state, "styles.css", "text/css; charset=utf-8"),
        (Method::Get, "/favicon.ico") => HttpResponse::bytes(204, "image/x-icon", Vec::new()),
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

fn api_favorites(state: &AppState) -> HttpResponse {
    match HttpResponse::json(&state.library.favorites()) {
        Ok(response) => response.with_header("Cache-Control", "no-store"),
        Err(error) => {
            eprintln!("remote-web: favorites JSON encoding failed: {error}");
            HttpResponse::text(500, "Internal Server Error")
        }
    }
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
    match state.library.thumbnail(favorite, path) {
        Ok(bytes) => {
            let blob_bytes = bytes.len();
            HttpResponse::bytes(200, "image/webp", bytes)
                .with_header("Cache-Control", "private, max-age=60")
                .with_log_details(json!({
                    "thumb": {
                        "hit": true,
                        "miss_reason": null,
                        "blob_bytes": blob_bytes,
                    }
                }))
        }
        Err(StoreError::ThumbnailMiss(reason)) => {
            HttpResponse::text(404, "Not Found").with_log_details(thumbnail_miss_details(reason))
        }
        Err(error) => store_error_response(error).with_log_details(json!({
            "thumb": {
                "hit": false,
                "miss_reason": "request_error",
                "blob_bytes": 0,
            }
        })),
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
        Ok(value) => HttpResponse::bytes(200, "image/webp", value.bytes)
            .with_header("Cache-Control", "private, max-age=60")
            .with_log_details(json!({"image": value.metrics})),
        Err(error) => store_error_response(error),
    }
}

fn thumbnail_miss_details(reason: ThumbnailMissReason) -> Value {
    json!({
        "thumb": {
            "hit": false,
            "miss_reason": reason,
            "blob_bytes": 0,
        }
    })
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
    let body = match read_limited_body(request) {
        Ok(body) => body,
        Err(TelemetryBodyError::TooLarge) => {
            return HttpResponse::text(413, "Payload Too Large")
                .with_header("Cache-Control", "no-store");
        }
        Err(TelemetryBodyError::Read) => {
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

enum TelemetryBodyError {
    TooLarge,
    Read,
}

fn read_limited_body(request: &mut Request) -> Result<Vec<u8>, TelemetryBodyError> {
    if request
        .body_length()
        .is_some_and(|length| length > MAX_TELEMETRY_BODY_BYTES)
    {
        return Err(TelemetryBodyError::TooLarge);
    }
    let mut body = Vec::new();
    request
        .as_reader()
        .take((MAX_TELEMETRY_BODY_BYTES + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|_| TelemetryBodyError::Read)?;
    if body.len() > MAX_TELEMETRY_BODY_BYTES {
        return Err(TelemetryBodyError::TooLarge);
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
        StoreError::ThumbnailMiss(_) => HttpResponse::text(404, "Not Found"),
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

fn url_without_token(path: &str, query: &[(String, String)]) -> String {
    let retained = query
        .iter()
        .filter(|(key, _)| key != "t")
        .map(|(key, value)| {
            format!(
                "{}={}",
                encode_query_component(key),
                encode_query_component(value)
            )
        })
        .collect::<Vec<_>>();
    if retained.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{}", retained.join("&"))
    }
}

fn encode_query_component(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::COOKIE_NAME;
    use tiny_http::TestRequest;

    const TEST_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn test_state(temp: &tempfile::TempDir) -> AppState {
        let protected = temp.path().join("data");
        std::fs::create_dir_all(&protected).unwrap();
        AppState {
            token: AuthToken::from_printable_for_test(TEST_TOKEN),
            library: Library::empty_for_test(temp.path().join("cache")),
            logger: DiagnosticsLogger::open(
                &temp.path().join("request.jsonl"),
                &[protected],
                TEST_TOKEN,
            )
            .unwrap(),
            telemetry_limiter: TelemetryLimiter::new(),
            request_sequence: AtomicU64::new(1),
            web_root: temp.path().to_owned(),
        }
    }

    fn cookie_header() -> Header {
        Header::from_bytes("Cookie", format!("{COOKIE_NAME}={TEST_TOKEN}").as_bytes()).unwrap()
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
    fn token_is_removed_from_redirect_location() {
        let query = parse_query("t=secret&fav=abc&path=A%2FB").unwrap();
        let location = url_without_token("/api/list", &query);
        assert!(!location.contains("secret"));
        assert!(!location.contains("t="));
        assert!(location.starts_with("/api/list?"));
    }

    #[test]
    fn cookie_name_is_not_ambiguous() {
        assert_eq!(COOKIE_NAME, "miv_remote_token");
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
            .with_header(cookie_header())
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
            .with_header(cookie_header())
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
            .with_header(cookie_header())
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
}
