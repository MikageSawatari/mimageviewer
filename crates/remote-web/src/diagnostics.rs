use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Value, json};

/// Keep the diagnostics bundle bounded without rotating so frequently that a
/// single long-running remote session loses useful context. Five generations
/// cap the normal total at roughly 80 MiB.
const MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;
const MAX_LOG_GENERATIONS: usize = 5;

pub struct DiagnosticsLogger {
    writer: Mutex<DiagnosticsWriter>,
    permanent_secrets: Vec<String>,
    path: PathBuf,
    max_bytes: u64,
}

struct DiagnosticsWriter {
    writer: Option<BufWriter<File>>,
    bytes_written: u64,
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
    pub sensitive_values: Vec<String>,
}

impl DiagnosticsLogger {
    pub fn open(
        requested_path: &Path,
        protected_roots: &[PathBuf],
        permanent_secrets: &[String],
    ) -> Result<Self, String> {
        Self::open_with_max_bytes(
            requested_path,
            protected_roots,
            permanent_secrets,
            MAX_LOG_BYTES,
        )
    }

    fn open_with_max_bytes(
        requested_path: &Path,
        protected_roots: &[PathBuf],
        permanent_secrets: &[String],
        max_bytes: u64,
    ) -> Result<Self, String> {
        let resolved_path =
            resolve_external_write_file_path(requested_path, protected_roots, "診断ログ")?;

        // Deliberately no rotation on startup, unlike the core's per-run perf log.
        // The core restarts this child whenever the PIN changes, Serve is configured,
        // or every device is logged out, so rotating here would quietly discard the
        // history a few button presses in. Size is what needed bounding; timestamps
        // already separate one session from the next.
        let writer = DiagnosticsWriter::open(&resolved_path).map_err(|error| {
            format!(
                "診断ログを開けません ({}): {error}",
                resolved_path.display()
            )
        })?;
        Ok(Self {
            writer: Mutex::new(writer),
            permanent_secrets: permanent_secrets
                .iter()
                .filter(|secret| !secret.is_empty())
                .cloned()
                .collect(),
            path: resolved_path,
            max_bytes,
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
        let mut redacted = serialized;
        for secret in self
            .permanent_secrets
            .iter()
            .chain(record.sensitive_values.iter())
            .filter(|secret| !secret.is_empty())
        {
            redacted = redact_serialized_secret(redacted, secret);
        }
        let Ok(mut writer) = self.writer.lock() else {
            eprintln!("remote-web: request log lock is poisoned");
            return;
        };
        if let Err(error) = writer.write_line(&self.path, self.max_bytes, &redacted) {
            eprintln!("remote-web: request log write failed: {error}");
        }
    }
}

impl DiagnosticsWriter {
    fn open(path: &Path) -> io::Result<Self> {
        let (writer, bytes_written) = open_log_writer(path)?;
        Ok(Self {
            writer: Some(writer),
            bytes_written,
        })
    }

    fn reopen(&mut self, path: &Path) -> io::Result<()> {
        let (writer, bytes_written) = open_log_writer(path)?;
        self.writer = Some(writer);
        self.bytes_written = bytes_written;
        Ok(())
    }

    fn write_line(&mut self, path: &Path, max_bytes: u64, line: &str) -> io::Result<()> {
        if self.writer.is_none() {
            self.reopen(path)?;
        }
        let write_result = {
            let writer = self
                .writer
                .as_mut()
                .expect("diagnostics writer was reopened");
            writer
                .write_all(line.as_bytes())
                .and_then(|()| writer.write_all(b"\n"))
                .and_then(|()| writer.flush())
        };
        if let Err(error) = write_result {
            self.writer.take();
            self.bytes_written = 0;
            return Err(error);
        }

        self.bytes_written = self
            .bytes_written
            .saturating_add(line.len() as u64)
            .saturating_add(1);
        if self.bytes_written > max_bytes {
            self.rotate(path);
        }
        Ok(())
    }

    fn rotate(&mut self, path: &Path) {
        if let Some(mut writer) = self.writer.take() {
            if let Err(error) = writer.flush() {
                eprintln!(
                    "remote-web: request log rotation flush failed ({}): {error}",
                    path.display()
                );
            }
        }
        self.bytes_written = 0;
        rotate_logs(path);
        if let Err(error) = self.reopen(path) {
            eprintln!(
                "remote-web: request log rotation reopen failed ({}): {error}",
                path.display()
            );
        }
    }
}

fn open_log_writer(path: &Path) -> io::Result<(BufWriter<File>, u64)> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let bytes_written = file.metadata()?.len();
    Ok((BufWriter::new(file), bytes_written))
}

fn rotate_logs(path: &Path) {
    let mut first_error = None;
    remove_for_rotation(
        &rotated_log_path(path, MAX_LOG_GENERATIONS - 1),
        &mut first_error,
    );
    for generation in (0..MAX_LOG_GENERATIONS - 1).rev() {
        let from = rotated_log_path(path, generation);
        let to = rotated_log_path(path, generation + 1);
        if let Err(error) = std::fs::rename(&from, &to) {
            record_rotation_error(&mut first_error, "rename", &from, error);
        }
    }
    if let Some(error) = first_error {
        eprintln!(
            "remote-web: request log rotation failed ({}): {error}",
            path.display()
        );
    }
}

fn remove_for_rotation(path: &Path, first_error: &mut Option<String>) {
    if let Err(error) = std::fs::remove_file(path) {
        record_rotation_error(first_error, "remove", path, error);
    }
}

fn record_rotation_error(
    first_error: &mut Option<String>,
    operation: &str,
    path: &Path,
    error: io::Error,
) {
    if error.kind() != io::ErrorKind::NotFound && first_error.is_none() {
        *first_error = Some(format!("{operation} {}: {error}", path.display()));
    }
}

fn rotated_log_path(path: &Path, generation: usize) -> PathBuf {
    if generation == 0 {
        return path.to_owned();
    }

    let mut name = path
        .file_stem()
        .map(OsString::from)
        .or_else(|| path.file_name().map(OsString::from))
        .unwrap_or_else(|| OsString::from("remote-web-log"));
    name.push(format!(".{generation}"));
    if let Some(extension) = path.extension() {
        name.push(".");
        name.push(extension);
    }
    path.with_file_name(name)
}

fn redact_serialized_secret(mut serialized: String, secret: &str) -> String {
    if let Ok(json_string) = serde_json::to_string(secret) {
        let escaped = json_string
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(&json_string);
        if escaped != secret {
            serialized = serialized.replace(escaped, "[redacted-secret]");
        }
    }
    serialized.replace(secret, "[redacted-secret]")
}

pub fn duration_ms(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1000.0 * 1000.0).round() / 1000.0
}

fn request_path(raw_url: &str) -> &str {
    raw_url.split_once('?').map_or(raw_url, |(path, _)| path)
}

pub fn resolve_external_write_file_path(
    requested_path: &Path,
    protected_roots: &[PathBuf],
    purpose: &str,
) -> Result<PathBuf, String> {
    let resolved = resolve_file_path(requested_path, purpose)?;
    reject_protected_write_path(&resolved, protected_roots, purpose)?;
    Ok(resolved)
}

pub fn resolve_file_path(requested_path: &Path, purpose: &str) -> Result<PathBuf, String> {
    let absolute = if requested_path.is_absolute() {
        requested_path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("カレントディレクトリを取得できません: {error}"))?
            .join(requested_path)
    };
    if absolute.try_exists().map_err(|error| {
        format!(
            "{purpose}の出力先を確認できません ({}): {error}",
            absolute.display()
        )
    })? {
        let resolved = std::fs::canonicalize(&absolute).map_err(|error| {
            format!(
                "{purpose}の出力先を解決できません ({}): {error}",
                absolute.display()
            )
        })?;
        return Ok(resolved);
    }

    let parent = absolute
        .parent()
        .ok_or_else(|| format!("{purpose}にはファイルパスを指定してください"))?;
    let filename = absolute
        .file_name()
        .ok_or_else(|| format!("{purpose}にはファイル名を指定してください"))?;
    let canonical_parent = std::fs::canonicalize(parent).map_err(|error| {
        format!(
            "{purpose}の親ディレクトリを解決できません ({}): {error}",
            parent.display()
        )
    })?;
    Ok(canonical_parent.join(filename))
}

/// Enforce the managed server's read-only boundary for files it opens for writing.
///
/// Reading the core-owned authentication file below the data directory is allowed; diagnostic
/// logs and any other server-owned output must remain outside that directory.
fn reject_protected_write_path(
    resolved_path: &Path,
    protected_roots: &[PathBuf],
    purpose: &str,
) -> Result<(), String> {
    for root in protected_roots {
        let protected = resolve_for_comparison(root)?;
        if path_starts_with(resolved_path, &protected) {
            return Err(format!(
                "{purpose}の書き込み先は mIV データディレクトリ配下へ配置できません: {}",
                resolved_path.display()
            ));
        }
    }
    Ok(())
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
    const PIN: &str = r#"84"62\91"#;
    const DERIVED: &str = "$argon2id$v=19$m=19456,t=2,p=1$derived";

    fn log_test_request(logger: &DiagnosticsLogger, request_id: u64) {
        logger.log_request(RequestLog {
            request_id,
            timestamp_unix_ms: request_id as u128,
            method: "GET",
            raw_url: "/api/test",
            status: 200,
            duration: Duration::from_millis(1),
            response_bytes: 10,
            response_write_ok: true,
            details: None,
            sensitive_values: Vec::new(),
        });
    }

    #[test]
    fn opening_appends_to_the_existing_log_instead_of_rotating_it() {
        // The core restarts this child on a PIN change, on Serve setup, and on
        // logging every device out. Rotating here would throw the history away a
        // few button presses in, so only size rotates the file.
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("request.jsonl");
        std::fs::write(&output, "previous session\n").unwrap();

        let logger = DiagnosticsLogger::open(&output, &[], &[]).unwrap();
        assert_eq!(
            logger.path(),
            resolve_file_path(&output, "diagnostics test").unwrap()
        );
        assert!(!rotated_log_path(&output, 1).exists());

        log_test_request(&logger, 1);
        let written = std::fs::read_to_string(&output).unwrap();
        assert!(written.starts_with("previous session\n"));
        assert!(written.contains("\"request_id\":1"));
    }

    #[test]
    fn logger_rotates_immediately_after_crossing_the_size_limit() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("request.jsonl");
        let logger = DiagnosticsLogger::open_with_max_bytes(&output, &[], &[], 1).unwrap();

        log_test_request(&logger, 1);

        assert_eq!(
            logger.path(),
            resolve_file_path(&output, "diagnostics test").unwrap()
        );
        assert!(rotated_log_path(&output, 1).is_file());
        assert!(std::fs::metadata(&output).unwrap().len() <= 1);
    }

    #[test]
    fn logger_keeps_only_five_generations() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("request.jsonl");
        let logger = DiagnosticsLogger::open_with_max_bytes(&output, &[], &[], 1).unwrap();

        for request_id in 0..7 {
            log_test_request(&logger, request_id);
        }

        let retained = (0..MAX_LOG_GENERATIONS)
            .filter_map(|generation| {
                std::fs::read_to_string(rotated_log_path(&output, generation)).ok()
            })
            .collect::<String>();
        assert!(!retained.contains("\"request_id\":0"));
        assert!(!retained.contains("\"request_id\":1"));
        assert!(!retained.contains("\"request_id\":2"));
        assert!(retained.contains("\"request_id\":3"));
        assert!(retained.contains("\"request_id\":6"));
        assert!(!rotated_log_path(&output, MAX_LOG_GENERATIONS).exists());
    }

    #[test]
    fn rotated_request_log_still_redacts_secrets() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("request.jsonl");
        let logger = DiagnosticsLogger::open_with_max_bytes(
            &output,
            &[],
            &[TOKEN.to_owned(), DERIVED.to_owned()],
            1,
        )
        .unwrap();
        logger.log_request(RequestLog {
            request_id: 41,
            timestamp_unix_ms: 0,
            method: "POST",
            raw_url: "/api/telemetry",
            status: 204,
            duration: Duration::from_millis(1),
            response_bytes: 0,
            response_write_ok: true,
            details: Some(json!({"message": format!("{TOKEN} {PIN} {DERIVED}")})),
            sensitive_values: vec![PIN.to_owned()],
        });

        let written = std::fs::read_to_string(rotated_log_path(&output, 1)).unwrap();
        assert!(!written.contains(TOKEN));
        assert!(!written.contains(PIN));
        assert!(!written.contains(DERIVED));
        assert!(written.contains("[redacted-secret]"));
    }

    #[test]
    fn request_log_never_contains_pin_bearer_or_pin_derived_hash() {
        let temp = tempfile::tempdir().unwrap();
        let protected = temp.path().join("data");
        let output = temp.path().join("request.jsonl");
        std::fs::create_dir_all(&protected).unwrap();
        let logger = DiagnosticsLogger::open(
            &output,
            &[protected],
            &[TOKEN.to_owned(), DERIVED.to_owned()],
        )
        .unwrap();
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
                "telemetry": {"events": [{"message": format!("secret={TOKEN} pin={PIN} hash={DERIVED}")}]}
            })),
            sensitive_values: vec![PIN.to_owned()],
        });

        let written = std::fs::read_to_string(output).unwrap();
        assert!(!written.contains(TOKEN));
        assert!(!written.contains(PIN));
        assert!(!written.contains(DERIVED));
        assert!(!written.contains("?t="));
        assert!(written.contains("[redacted-secret]"));
    }

    #[test]
    fn request_log_drops_absolute_path_query_values() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("request.jsonl");
        let logger = DiagnosticsLogger::open(&output, &[], &[]).unwrap();
        logger.log_request(RequestLog {
            request_id: 43,
            timestamp_unix_ms: 0,
            method: "GET",
            raw_url: "/api/image?path=C%3A%5CUsers%5CAlice%5Csecret.jpg&w=1200",
            status: 200,
            duration: Duration::from_millis(1),
            response_bytes: 10,
            response_write_ok: true,
            details: None,
            sensitive_values: Vec::new(),
        });

        let written = std::fs::read_to_string(output).unwrap();
        assert!(written.contains("/api/image"));
        assert!(!written.contains("Alice"));
        assert!(!written.contains("secret.jpg"));
        assert!(!written.contains("?path="));
    }

    #[test]
    fn log_path_beneath_miv_data_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let protected = temp.path().join("mimageviewer");
        std::fs::create_dir_all(&protected).unwrap();
        let result = DiagnosticsLogger::open(
            &protected.join("remote-web-log.jsonl"),
            &[protected],
            &[TOKEN.to_owned()],
        );
        assert!(result.is_err());
    }

    #[test]
    fn auth_file_beneath_miv_data_is_allowed_for_reading() {
        let temp = tempfile::tempdir().unwrap();
        let protected = temp.path().join("mimageviewer");
        std::fs::create_dir_all(&protected).unwrap();
        let auth_path = protected.join("auth.json");
        mimageviewer_ipc::set_pin_file(&auth_path, "123456").unwrap();

        let resolved = resolve_file_path(&auth_path, "認証ファイル").unwrap();
        mimageviewer_ipc::load_pin_file(&resolved).unwrap();
        assert!(resolve_external_write_file_path(&auth_path, &[protected], "診断ログ").is_err());
    }
}
