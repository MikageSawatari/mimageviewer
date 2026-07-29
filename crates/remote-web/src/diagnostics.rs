use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Value, json};

pub struct DiagnosticsLogger {
    writer: Mutex<BufWriter<File>>,
    token: String,
    path: PathBuf,
}

pub struct RequestLog<'a> {
    pub request_id: u64,
    pub timestamp_unix_ms: u128,
    pub method: &'a str,
    pub raw_url: &'a str,
    pub status: u16,
    pub duration: Duration,
    pub response_bytes: usize,
    pub response_write_ok: bool,
    pub details: Option<Value>,
}

impl DiagnosticsLogger {
    pub fn open(
        requested_path: &Path,
        protected_roots: &[PathBuf],
        token: &str,
    ) -> Result<Self, String> {
        let resolved_path = resolve_output_path(requested_path)?;
        for root in protected_roots {
            let protected = resolve_for_comparison(root)?;
            if path_starts_with(&resolved_path, &protected) {
                return Err(format!(
                    "診断ログは mIV データディレクトリ配下へ出力できません: {}",
                    resolved_path.display()
                ));
            }
        }

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&resolved_path)
            .map_err(|error| {
                format!(
                    "診断ログを開けません ({}): {error}",
                    resolved_path.display()
                )
            })?;
        Ok(Self {
            writer: Mutex::new(BufWriter::new(file)),
            token: token.to_owned(),
            path: resolved_path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn log_request(&self, record: RequestLog<'_>) {
        let mut value = json!({
            "request_id": record.request_id,
            "timestamp_unix_ms": record.timestamp_unix_ms,
            "kind": "request",
            "method": record.method,
            "path": request_path(record.raw_url),
            "status": record.status,
            "duration_ms": duration_ms(record.duration),
            "response_bytes": record.response_bytes,
            "response_write_ok": record.response_write_ok,
        });
        if let Some(details) = record.details {
            value["details"] = details;
        }

        let Ok(serialized) = serde_json::to_string(&value) else {
            eprintln!("remote-web: request log serialization failed");
            return;
        };
        // The request path already drops its query, but telemetry error strings
        // are client-controlled. Redact the active secret from the complete JSON
        // line as a final fail-safe before it reaches disk.
        let redacted = serialized.replace(&self.token, "[redacted-token]");
        let Ok(mut writer) = self.writer.lock() else {
            eprintln!("remote-web: request log lock is poisoned");
            return;
        };
        if let Err(error) = writeln!(writer, "{redacted}").and_then(|()| writer.flush()) {
            eprintln!("remote-web: request log write failed: {error}");
        }
    }
}

pub fn duration_ms(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1000.0 * 1000.0).round() / 1000.0
}

fn request_path(raw_url: &str) -> &str {
    raw_url.split_once('?').map_or(raw_url, |(path, _)| path)
}

fn resolve_output_path(requested_path: &Path) -> Result<PathBuf, String> {
    let absolute = if requested_path.is_absolute() {
        requested_path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("カレントディレクトリを取得できません: {error}"))?
            .join(requested_path)
    };
    if absolute.try_exists().map_err(|error| {
        format!(
            "診断ログの出力先を確認できません ({}): {error}",
            absolute.display()
        )
    })? {
        return std::fs::canonicalize(&absolute).map_err(|error| {
            format!(
                "診断ログの出力先を解決できません ({}): {error}",
                absolute.display()
            )
        });
    }

    let parent = absolute
        .parent()
        .ok_or_else(|| "診断ログにはファイルパスを指定してください".to_owned())?;
    let filename = absolute
        .file_name()
        .ok_or_else(|| "診断ログにはファイル名を指定してください".to_owned())?;
    let canonical_parent = std::fs::canonicalize(parent).map_err(|error| {
        format!(
            "診断ログの親ディレクトリを解決できません ({}): {error}",
            parent.display()
        )
    })?;
    Ok(canonical_parent.join(filename))
}

fn resolve_for_comparison(path: &Path) -> Result<PathBuf, String> {
    if path.try_exists().map_err(|error| {
        format!(
            "保護対象ディレクトリを確認できません ({}): {error}",
            path.display()
        )
    })? {
        return std::fs::canonicalize(path).map_err(|error| {
            format!(
                "保護対象ディレクトリを解決できません ({}): {error}",
                path.display()
            )
        });
    }
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|error| format!("カレントディレクトリを取得できません: {error}"))
    }
}

#[cfg(windows)]
fn path_starts_with(path: &Path, root: &Path) -> bool {
    let mut path_components = path.components();
    for root_component in root.components() {
        let Some(path_component) = path_components.next() else {
            return false;
        };
        if !component_eq_ignore_ascii_case(path_component, root_component) {
            return false;
        }
    }
    true
}

#[cfg(windows)]
fn component_eq_ignore_ascii_case(left: Component<'_>, right: Component<'_>) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

#[cfg(not(windows))]
fn path_starts_with(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn request_log_never_contains_query_or_telemetry_token() {
        let temp = tempfile::tempdir().unwrap();
        let protected = temp.path().join("data");
        let output = temp.path().join("request.jsonl");
        std::fs::create_dir_all(&protected).unwrap();
        let logger = DiagnosticsLogger::open(&output, &[protected], TOKEN).unwrap();
        logger.log_request(RequestLog {
            request_id: 42,
            timestamp_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
            method: "POST",
            raw_url: &format!("/api/telemetry?t={TOKEN}"),
            status: 204,
            duration: Duration::from_millis(3),
            response_bytes: 0,
            response_write_ok: true,
            details: Some(json!({
                "telemetry": {"events": [{"message": format!("secret={TOKEN}")}]}
            })),
        });

        let written = std::fs::read_to_string(output).unwrap();
        assert!(!written.contains(TOKEN));
        assert!(!written.contains("?t="));
        assert!(written.contains("[redacted-token]"));
    }

    #[test]
    fn log_path_beneath_miv_data_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let protected = temp.path().join("mimageviewer");
        std::fs::create_dir_all(&protected).unwrap();
        let result =
            DiagnosticsLogger::open(&protected.join("remote-web-log.jsonl"), &[protected], TOKEN);
        assert!(result.is_err());
    }
}
