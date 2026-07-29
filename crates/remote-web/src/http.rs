use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use percent_encoding::percent_decode_str;
use tiny_http::{Header, Method, Request, Response, StatusCode};
use uuid::Uuid;

use crate::auth::{AuthDecision, AuthInput, AuthSource, AuthToken, session_cookie};
use crate::store::{Library, StoreError};

pub struct AppState {
    pub token: AuthToken,
    pub library: Library,
    pub web_root: PathBuf,
}

struct HttpResponse {
    status: u16,
    content_type: &'static str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn bytes(status: u16, content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type,
            headers: Vec::new(),
            body,
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
}

pub fn handle(request: Request, state: &Arc<AppState>) {
    let response = route(&request, state);
    if let Err(error) = respond(request, response) {
        eprintln!("remote-web: response write failed: {error}");
    }
}

fn route(request: &Request, state: &AppState) -> HttpResponse {
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

    if request.method() != &Method::Get {
        return HttpResponse::text(405, "Method Not Allowed")
            .with_header("Allow", "GET")
            .with_header("Cache-Control", "no-store");
    }

    if source == AuthSource::Query {
        let location = url_without_token(path, &query);
        return HttpResponse::text(303, "See Other")
            .with_header("Location", location)
            .with_header("Set-Cookie", session_cookie(&state.token))
            .with_header("Cache-Control", "no-store")
            .with_header("X-Content-Type-Options", "nosniff")
            .with_header("Referrer-Policy", "no-referrer");
    }

    let response = match path {
        "/api/favorites" => api_favorites(state),
        "/api/list" => api_list(state, &query),
        "/api/thumb" => api_thumb(state, &query),
        "/api/image" => api_image(state, &query),
        "/" => static_file(state, "index.html", "text/html; charset=utf-8"),
        "/app.js" => static_file(state, "app.js", "text/javascript; charset=utf-8"),
        "/styles.css" => static_file(state, "styles.css", "text/css; charset=utf-8"),
        "/favicon.ico" => HttpResponse::bytes(204, "image/x-icon", Vec::new()),
        _ => HttpResponse::text(404, "Not Found"),
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
        Ok(value) => match HttpResponse::json(&value) {
            Ok(response) => response.with_header("Cache-Control", "no-store"),
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
        Ok(bytes) => HttpResponse::bytes(200, "image/webp", bytes)
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
        Ok(bytes) => HttpResponse::bytes(200, "image/webp", bytes)
            .with_header("Cache-Control", "private, max-age=60"),
        Err(error) => store_error_response(error),
    }
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
}
