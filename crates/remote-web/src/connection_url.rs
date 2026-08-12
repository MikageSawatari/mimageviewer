use std::net::{IpAddr, SocketAddr};
use std::path::Path;

use mimageviewer_ipc::{
    RemoteWebFeatureStatus, SessionConnectionKind, SessionPeerInfo, run_tailscale_at,
    tailscale_executable,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TailscaleServeState {
    status: RemoteWebFeatureStatus,
    url: Option<String>,
    conflict: Option<String>,
}

pub fn choose_connection_url(
    requested: Option<&str>,
    address: SocketAddr,
) -> Result<ConnectionUrl, String> {
    if let Some(requested) = requested {
        let serve = detect_tailscale_serve_status(address);
        return Ok(ConnectionUrl {
            base: normalize_base_url(requested)?,
            source: UrlSource::CommandLine,
            tailscale_serve: serve.status,
            tailscale_serve_conflict: serve.conflict,
        });
    }

    let mut tailscale_serve = RemoteWebFeatureStatus::Unknown;
    let mut tailscale_serve_conflict = None;
    if let Some(executable) = tailscale_executable() {
        let serve = tailscale_serve_status(&executable, address);
        tailscale_serve = serve.status;
        tailscale_serve_conflict.clone_from(&serve.conflict);
        if let Some(base) = serve.url {
            return Ok(ConnectionUrl {
                base,
                source: UrlSource::TailscaleServe,
                tailscale_serve,
                tailscale_serve_conflict,
            });
        }
        if let Some(base) = tailscale_status_url(&executable) {
            return Ok(ConnectionUrl {
                base,
                source: UrlSource::TailscaleStatus,
                tailscale_serve,
                tailscale_serve_conflict,
            });
        }
    }

    Ok(ConnectionUrl {
        base: format!("http://{address}/"),
        source: UrlSource::BindFallback,
        tailscale_serve,
        tailscale_serve_conflict,
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

fn command_json(executable: &Path, arguments: &[&str]) -> Option<Value> {
    let output = run_tailscale_at(executable, arguments).ok()?;
    serde_json::from_slice(&output.stdout).ok()
}

fn detect_tailscale_serve_status(address: SocketAddr) -> TailscaleServeState {
    tailscale_executable()
        .map(|executable| tailscale_serve_status(&executable, address))
        .unwrap_or_else(unknown_serve_state)
}

fn tailscale_serve_status(executable: &Path, address: SocketAddr) -> TailscaleServeState {
    let Ok(output) = run_tailscale_at(executable, &["serve", "status", "--json"]) else {
        return unknown_serve_state();
    };
    tailscale_serve_status_json(&output.stdout, address)
}

fn tailscale_serve_status_json(bytes: &[u8], address: SocketAddr) -> TailscaleServeState {
    let Ok(value) = serde_json::from_slice(bytes) else {
        return unknown_serve_state();
    };
    inspect_tailscale_serve(&value, address)
}

fn inspect_tailscale_serve(value: &Value, address: SocketAddr) -> TailscaleServeState {
    let mut conflict = None;
    let Some(web) = value.get("Web").and_then(Value::as_object) else {
        return TailscaleServeState {
            status: RemoteWebFeatureStatus::NotConfigured,
            url: None,
            conflict: None,
        };
    };
    for (entry, config) in web {
        let Some(host) = ts_hostname(entry) else {
            continue;
        };
        let Some(handlers) = config.get("Handlers").and_then(Value::as_object) else {
            continue;
        };
        for (path, handler) in handlers {
            let Some(proxy) = handler.get("Proxy").and_then(Value::as_str) else {
                continue;
            };
            if proxy_targets(proxy, address)
                && let Some(url) = serve_url(&host, path)
            {
                return TailscaleServeState {
                    status: RemoteWebFeatureStatus::Configured,
                    url: Some(url),
                    conflict: None,
                };
            }
            if path == "/" && conflict.is_none() {
                conflict = Some(proxy.to_owned());
            }
        }
    }
    TailscaleServeState {
        status: RemoteWebFeatureStatus::NotConfigured,
        url: None,
        conflict,
    }
}

fn unknown_serve_state() -> TailscaleServeState {
    TailscaleServeState {
        status: RemoteWebFeatureStatus::Unknown,
        url: None,
        conflict: None,
    }
}

fn proxy_targets(proxy: &str, address: SocketAddr) -> bool {
    let Some((scheme, remainder)) = proxy.split_once("://") else {
        return false;
    };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return false;
    }
    remainder
        .split('/')
        .next()
        .and_then(|authority| authority.parse::<SocketAddr>().ok())
        == Some(address)
}

fn serve_url(host: &str, path: &str) -> Option<String> {
    path.starts_with('/')
        .then(|| format!("https://{host}{path}"))
        .and_then(|url| normalize_base_url(&url).ok())
}

fn tailscale_status_url(executable: &Path) -> Option<String> {
    let value = command_json(executable, &["status", "--json"])?;
    let dns_name = value.get("Self")?.get("DNSName")?.as_str()?;
    ts_hostname(dns_name).and_then(|host| normalize_base_url(&format!("https://{host}/")).ok())
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
    fn real_serve_json_is_configured_only_for_the_owned_proxy_target() {
        let json = br#"{
            "TCP": { "443": { "HTTPS": true } },
            "Web": {
                "desktop4090.taild260d0.ts.net:443": {
                    "Handlers": { "/": { "Proxy": "http://127.0.0.1:8787" } }
                }
            }
        }"#;
        let state = tailscale_serve_status_json(json, "127.0.0.1:8787".parse().unwrap());
        assert_eq!(state.status, RemoteWebFeatureStatus::Configured);
        assert_eq!(
            state.url.as_deref(),
            Some("https://desktop4090.taild260d0.ts.net/")
        );
        assert_eq!(state.conflict, None);
    }

    #[test]
    fn another_proxy_port_is_not_configured_and_reports_the_root_conflict() {
        let json = br#"{"Web":{"desktop4090.taild260d0.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:3000"}}}}}"#;
        let state = tailscale_serve_status_json(json, "127.0.0.1:8787".parse().unwrap());
        assert_eq!(state.status, RemoteWebFeatureStatus::NotConfigured);
        assert_eq!(state.url, None);
        assert_eq!(state.conflict.as_deref(), Some("http://127.0.0.1:3000"));
    }

    #[test]
    fn no_ts_net_entry_is_not_configured_without_a_conflict() {
        let json =
            br#"{"Web":{"localhost:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:3000"}}}}}"#;
        let state = tailscale_serve_status_json(json, "127.0.0.1:8787".parse().unwrap());
        assert_eq!(state.status, RemoteWebFeatureStatus::NotConfigured);
        assert_eq!(state.url, None);
        assert_eq!(state.conflict, None);
    }

    #[test]
    fn configured_handler_path_is_part_of_the_connection_url() {
        let json = br#"{"Web":{"desktop4090.taild260d0.ts.net:443":{"Handlers":{"/miv":{"Proxy":"http://127.0.0.1:8787"}}}}}"#;
        let state = tailscale_serve_status_json(json, "127.0.0.1:8787".parse().unwrap());
        assert_eq!(state.status, RemoteWebFeatureStatus::Configured);
        assert_eq!(
            state.url.as_deref(),
            Some("https://desktop4090.taild260d0.ts.net/miv/")
        );
    }

    #[test]
    fn broken_serve_json_is_unknown() {
        let state = tailscale_serve_status_json(b"{not json", "127.0.0.1:8787".parse().unwrap());
        assert_eq!(state, unknown_serve_state());
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
