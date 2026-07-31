use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use mimageviewer_ipc::{SessionConnectionKind, SessionPeerInfo};
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

/// Tailscale proxy が付けた X-Forwarded-For (無ければ TCP peer) を status JSON の
/// Peer.TailscaleIPs と照合する。CLI が無い/失敗/未照合でもセッション取得自体は止めない。
pub fn detect_peer_info(source_ip: Option<IpAddr>) -> SessionPeerInfo {
    let Some(source_ip) = source_ip else {
        return unknown_peer();
    };
    tailscale_executable()
        .and_then(|executable| command_json(&executable, &["status", "--json"]))
        .and_then(|value| peer_info_from_status(&value, source_ip))
        .unwrap_or_else(unknown_peer)
}

fn unknown_peer() -> SessionPeerInfo {
    SessionPeerInfo {
        connection_kind: SessionConnectionKind::Unknown,
        device_name: None,
    }
}

fn peer_info_from_status(value: &Value, source_ip: IpAddr) -> Option<SessionPeerInfo> {
    let peers = value.get("Peer")?.as_object()?;
    let peer = peers.values().find(|peer| {
        peer.get("TailscaleIPs")
            .and_then(Value::as_array)
            .is_some_and(|ips| {
                ips.iter().any(|ip| {
                    ip.as_str().and_then(|value| value.parse::<IpAddr>().ok()) == Some(source_ip)
                })
            })
    })?;
    let direct = peer
        .get("CurAddr")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty());
    let relay = peer
        .get("Relay")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty());
    let connection_kind = if direct {
        SessionConnectionKind::Direct
    } else if relay {
        SessionConnectionKind::Relay
    } else {
        SessionConnectionKind::Unknown
    };
    let device_name = peer
        .get("HostName")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .or_else(|| peer.get("DNSName").and_then(Value::as_str))
        .map(|value| value.trim_end_matches('.').to_owned());
    Some(SessionPeerInfo {
        connection_kind,
        device_name,
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

    #[test]
    fn peer_ip_resolves_direct_and_relay_from_status_json() {
        let value: Value = serde_json::from_str(
            r#"{"Peer":{"a":{"TailscaleIPs":["100.64.0.2"],"HostName":"phone","CurAddr":"192.0.2.1:123","Relay":""},"b":{"TailscaleIPs":["100.64.0.3"],"DNSName":"laptop.taild260d0.ts.net.","CurAddr":"","Relay":"tok"}}}"#,
        )
        .unwrap();
        let direct = peer_info_from_status(&value, "100.64.0.2".parse().unwrap()).unwrap();
        assert_eq!(direct.connection_kind, SessionConnectionKind::Direct);
        assert_eq!(direct.device_name.as_deref(), Some("phone"));
        let relay = peer_info_from_status(&value, "100.64.0.3".parse().unwrap()).unwrap();
        assert_eq!(relay.connection_kind, SessionConnectionKind::Relay);
        assert_eq!(
            relay.device_name.as_deref(),
            Some("laptop.taild260d0.ts.net")
        );
    }
}
