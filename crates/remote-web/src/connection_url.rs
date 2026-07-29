use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const PREFERRED_TAILSCALE_EXE: &str = r"C:\Program Files\Tailscale\tailscale.exe";
const TAILSCALE_COMMAND_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlSource {
    CommandLine,
    TailscaleServe,
    TailscaleStatus,
    BindFallback,
}

impl UrlSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::CommandLine => "--url",
            Self::TailscaleServe => "tailscale serve status --json",
            Self::TailscaleStatus => "tailscale status --json",
            Self::BindFallback => "bind fallback",
        }
    }
}

pub struct ConnectionUrl {
    pub base: String,
    pub source: UrlSource,
}

pub fn choose_connection_url(
    requested: Option<&str>,
    address: SocketAddr,
) -> Result<ConnectionUrl, String> {
    if let Some(requested) = requested {
        return Ok(ConnectionUrl {
            base: normalize_base_url(requested)?,
            source: UrlSource::CommandLine,
        });
    }

    if let Some(executable) = tailscale_executable() {
        if let Some(base) = tailscale_serve_url(&executable) {
            return Ok(ConnectionUrl {
                base,
                source: UrlSource::TailscaleServe,
            });
        }
        if let Some(base) = tailscale_status_url(&executable) {
            return Ok(ConnectionUrl {
                base,
                source: UrlSource::TailscaleStatus,
            });
        }
    }

    Ok(ConnectionUrl {
        base: format!("http://{address}/"),
        source: UrlSource::BindFallback,
    })
}

fn tailscale_executable() -> Option<PathBuf> {
    let preferred = PathBuf::from(PREFERRED_TAILSCALE_EXE);
    if preferred.is_file() {
        Some(preferred)
    } else {
        find_on_path("tailscale.exe").or_else(|| find_on_path("tailscale"))
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|directory| {
        let candidate = directory.join(name);
        candidate.is_file().then_some(candidate)
    })
}

fn command_json(executable: &Path, arguments: &[&str]) -> Option<Value> {
    let mut child = Command::new(executable)
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + TAILSCALE_COMMAND_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let bytes = reader.join().ok()?.ok()?;
    if !status?.success() {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn tailscale_serve_url(executable: &Path) -> Option<String> {
    let value = command_json(executable, &["serve", "status", "--json"])?;
    serve_hostname(&value).and_then(|host| normalize_base_url(&format!("https://{host}/")).ok())
}

fn tailscale_status_url(executable: &Path) -> Option<String> {
    let value = command_json(executable, &["status", "--json"])?;
    let dns_name = value.get("Self")?.get("DNSName")?.as_str()?;
    ts_hostname(dns_name).and_then(|host| normalize_base_url(&format!("https://{host}/")).ok())
}

fn serve_hostname(value: &Value) -> Option<String> {
    let web = value.get("Web")?.as_object()?;
    web.keys().find_map(|key| ts_hostname(key))
}

fn ts_hostname(candidate: &str) -> Option<String> {
    let without_scheme = candidate
        .strip_prefix("https://")
        .or_else(|| candidate.strip_prefix("http://"))
        .unwrap_or(candidate);
    let authority = without_scheme.split('/').next()?.trim_end_matches('.');
    let host = authority
        .rsplit_once(':')
        .filter(|(_, port)| port.bytes().all(|byte| byte.is_ascii_digit()))
        .map_or(authority, |(host, _)| host)
        .trim_end_matches('.');
    host.to_ascii_lowercase()
        .ends_with(".ts.net")
        .then(|| host.to_owned())
}

fn normalize_base_url(requested: &str) -> Result<String, String> {
    let trimmed = requested.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("--url は http:// または https:// の絶対 URL で指定してください".to_owned());
    }
    let authority_and_path = trimmed
        .split_once("://")
        .map(|(_, remainder)| remainder)
        .unwrap_or_default();
    let authority = authority_and_path.split('/').next().unwrap_or_default();
    if authority.is_empty()
        || authority.contains('@')
        || trimmed.contains('?')
        || trimmed.contains('#')
    {
        return Err("--url に認証情報・query・fragment は含められません".to_owned());
    }
    Ok(format!("{}/", trimmed.trim_end_matches('/')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_dns_name_becomes_https_url() {
        let value: Value =
            serde_json::from_str(r#"{"Self":{"DNSName":"desktop.taild260d0.ts.net."}}"#).unwrap();
        let dns_name = value["Self"]["DNSName"].as_str().unwrap();
        let host = ts_hostname(dns_name).unwrap();
        assert_eq!(
            normalize_base_url(&format!("https://{host}/")).unwrap(),
            "https://desktop.taild260d0.ts.net/"
        );
    }

    #[test]
    fn serve_web_key_is_read_from_json() {
        let value: Value =
            serde_json::from_str(r#"{"Web":{"desktop.taild260d0.ts.net:443":{"Handlers":{}}}}"#)
                .unwrap();
        assert_eq!(
            serve_hostname(&value).as_deref(),
            Some("desktop.taild260d0.ts.net")
        );
    }

    #[test]
    fn explicit_url_cannot_contain_secrets() {
        assert!(normalize_base_url("https://host.example/?t=secret").is_err());
        assert!(normalize_base_url("https://user:pass@host.example/").is_err());
        assert_eq!(
            normalize_base_url("http://127.0.0.1:8787").unwrap(),
            "http://127.0.0.1:8787/"
        );
    }
}
