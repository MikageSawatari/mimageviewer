use std::net::{IpAddr, SocketAddr};
use std::path::Path;

use mimageviewer_ipc::{
    RemoteWebFeatureStatus, SessionConnectionKind, SessionPeerInfo, probe_tailnet,
    run_tailscale_at, tailscale_executable,
};
use serde_json::Value;

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
    pub tailscale_serve: RemoteWebFeatureStatus,
    pub tailscale_serve_conflict: Option<String>,
    pub tailscale_serve_unsupported_path: Option<String>,
    pub tailscale_https_certificate: RemoteWebFeatureStatus,
    pub tailscale_key_expiry_unix_seconds: Option<i64>,
}

pub fn choose_connection_url(
    requested: Option<&str>,
    address: SocketAddr,
) -> Result<ConnectionUrl, String> {
    let probe = probe_tailnet(address);
    let connection = |base, source| ConnectionUrl {
        base,
        source,
        tailscale_serve: probe.serve,
        tailscale_serve_conflict: probe.serve_conflict.clone(),
        tailscale_serve_unsupported_path: probe.serve_unsupported_path.clone(),
        tailscale_https_certificate: probe.https_certificate,
        tailscale_key_expiry_unix_seconds: probe.key_expiry_unix_seconds,
    };

    if let Some(requested) = requested {
        return Ok(connection(
            normalize_base_url(requested)?,
            UrlSource::CommandLine,
        ));
    }
    if let Some(base) = probe.serve_url.clone() {
        return Ok(connection(base, UrlSource::TailscaleServe));
    }
    if let Some(base) = probe.status_url.clone() {
        return Ok(connection(base, UrlSource::TailscaleStatus));
    }
    Ok(connection(
        format!("http://{address}/"),
        UrlSource::BindFallback,
    ))
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

fn command_json(executable: &Path, arguments: &[&str]) -> Option<Value> {
    let output = run_tailscale_at(executable, arguments).ok()?;
    serde_json::from_slice(&output.stdout).ok()
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
        let value = serde_json::json!({
            "Peer": {
                "a": {
                    "TailscaleIPs": ["100.64.0.2"],
                    "HostName": "phone",
                    "CurAddr": "192.0.2.1:123",
                    "Relay": ""
                },
                "b": {
                    "TailscaleIPs": ["100.64.0.3"],
                    "DNSName": "laptop.taild260d0.ts.net.",
                    "CurAddr": "",
                    "Relay": "tok"
                }
            }
        });
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
