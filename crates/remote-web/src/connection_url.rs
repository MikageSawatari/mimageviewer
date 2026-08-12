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
    pub tailscale_serve_unsupported_path: Option<String>,
    pub tailscale_https_certificate: RemoteWebFeatureStatus,
    pub tailscale_key_expiry_unix_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TailscaleServeState {
    status: RemoteWebFeatureStatus,
    url: Option<String>,
    conflict: Option<String>,
    unsupported_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TailscaleStatusState {
    url: Option<String>,
    https_certificate: RemoteWebFeatureStatus,
    key_expiry_unix_seconds: Option<i64>,
}

pub fn choose_connection_url(
    requested: Option<&str>,
    address: SocketAddr,
) -> Result<ConnectionUrl, String> {
    let executable = tailscale_executable();
    let serve = executable
        .as_deref()
        .map(|executable| tailscale_serve_status(executable, address))
        .unwrap_or_else(unknown_serve_state);
    let status = executable
        .as_deref()
        .map(tailscale_status)
        .unwrap_or_else(unknown_tailscale_status);
    let connection = |base, source| ConnectionUrl {
        base,
        source,
        tailscale_serve: serve.status,
        tailscale_serve_conflict: serve.conflict.clone(),
        tailscale_serve_unsupported_path: serve.unsupported_path.clone(),
        tailscale_https_certificate: status.https_certificate,
        tailscale_key_expiry_unix_seconds: status.key_expiry_unix_seconds,
    };

    if let Some(requested) = requested {
        return Ok(connection(
            normalize_base_url(requested)?,
            UrlSource::CommandLine,
        ));
    }
    if let Some(base) = serve.url.clone() {
        return Ok(connection(base, UrlSource::TailscaleServe));
    }
    if let Some(base) = status.url.clone() {
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
    let mut unsupported_path = None;
    let Some(web) = value.get("Web").and_then(Value::as_object) else {
        return TailscaleServeState {
            status: RemoteWebFeatureStatus::NotConfigured,
            url: None,
            conflict: None,
            unsupported_path: None,
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
            if proxy_targets(proxy, address) {
                if path == "/"
                    && let Some(url) = serve_url(&host, path)
                {
                    return TailscaleServeState {
                        status: RemoteWebFeatureStatus::Configured,
                        url: Some(url),
                        conflict: None,
                        unsupported_path: None,
                    };
                }
                if unsupported_path.is_none() {
                    unsupported_path = Some(path.to_owned());
                }
                continue;
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
        unsupported_path,
    }
}

fn unknown_serve_state() -> TailscaleServeState {
    TailscaleServeState {
        status: RemoteWebFeatureStatus::Unknown,
        url: None,
        conflict: None,
        unsupported_path: None,
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

fn tailscale_status(executable: &Path) -> TailscaleStatusState {
    let Ok(output) = run_tailscale_at(executable, &["status", "--json"]) else {
        return unknown_tailscale_status();
    };
    tailscale_status_json(&output.stdout)
}

fn tailscale_status_json(bytes: &[u8]) -> TailscaleStatusState {
    let Ok(value) = serde_json::from_slice(bytes) else {
        return unknown_tailscale_status();
    };
    inspect_tailscale_status(&value)
}

fn inspect_tailscale_status(value: &Value) -> TailscaleStatusState {
    let url = value
        .get("Self")
        .and_then(|self_node| self_node.get("DNSName"))
        .and_then(Value::as_str)
        .and_then(ts_hostname)
        .and_then(|host| normalize_base_url(&format!("https://{host}/")).ok());
    let https_certificate = if value
        .get("CertDomains")
        .and_then(Value::as_array)
        .is_some_and(|domains| !domains.is_empty())
    {
        RemoteWebFeatureStatus::Configured
    } else {
        RemoteWebFeatureStatus::NotConfigured
    };
    // status JSON は Expired=false をフィールドごと省略するため、欠落と false を区別できない。
    // 期限切れは Expired ではなく、常に KeyExpiry timestamp と現在時刻から本体側で導く。
    let key_expiry_unix_seconds = value
        .get("Self")
        .and_then(|self_node| self_node.get("KeyExpiry"))
        .and_then(Value::as_str)
        .and_then(parse_rfc3339_unix_seconds);
    TailscaleStatusState {
        url,
        https_certificate,
        key_expiry_unix_seconds,
    }
}

fn unknown_tailscale_status() -> TailscaleStatusState {
    TailscaleStatusState {
        url: None,
        https_certificate: RemoteWebFeatureStatus::Unknown,
        key_expiry_unix_seconds: None,
    }
}

fn parse_rfc3339_unix_seconds(value: &str) -> Option<i64> {
    let (date, time_and_zone) = value.split_once('T').or_else(|| value.split_once('t'))?;
    let (time, offset_seconds) = if let Some(time) = time_and_zone
        .strip_suffix('Z')
        .or_else(|| time_and_zone.strip_suffix('z'))
    {
        (time, 0_i64)
    } else {
        let offset_start = time_and_zone.rfind(['+', '-'])?;
        let (time, offset) = time_and_zone.split_at(offset_start);
        if offset.len() != 6 || offset.as_bytes().get(3) != Some(&b':') {
            return None;
        }
        let sign = if offset.starts_with('+') {
            1_i64
        } else {
            -1_i64
        };
        let hours = offset.get(1..3)?.parse::<i64>().ok()?;
        let minutes = offset.get(4..6)?.parse::<i64>().ok()?;
        if hours > 23 || minutes > 59 {
            return None;
        }
        (time, sign * (hours * 3_600 + minutes * 60))
    };

    if date.len() != 10
        || !date.is_ascii()
        || date.as_bytes().get(4) != Some(&b'-')
        || date.as_bytes().get(7) != Some(&b'-')
        || time.len() < 8
        || !time.is_ascii()
        || time.as_bytes().get(2) != Some(&b':')
        || time.as_bytes().get(5) != Some(&b':')
    {
        return None;
    }
    let year = date.get(0..4)?.parse::<i64>().ok()?;
    let month = date.get(5..7)?.parse::<u32>().ok()?;
    let day = date.get(8..10)?.parse::<u32>().ok()?;
    let hour = time.get(0..2)?.parse::<i64>().ok()?;
    let minute = time.get(3..5)?.parse::<i64>().ok()?;
    let second_part = time.get(6..)?;
    let second_text = second_part
        .split_once('.')
        .map_or(second_part, |(seconds, fraction)| {
            (!fraction.is_empty() && fraction.bytes().all(|byte| byte.is_ascii_digit()))
                .then_some(seconds)
                .unwrap_or("")
        });
    let second = second_text.parse::<i64>().ok()?;
    if hour > 23 || minute > 59 || second > 59 || !valid_date(year, month, day) {
        return None;
    }
    days_from_civil(year, month, day)
        .checked_mul(86_400)?
        .checked_add(hour * 3_600 + minute * 60 + second)?
        .checked_sub(offset_seconds)
}

fn valid_date(year: i64, month: u32, day: u32) -> bool {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    // Howard Hinnant の同じアルゴリズムの写しは
    // src/settings.rs の FacetCalendarDate::ordinal にもある (crate 境界のため現時点では共有しない)。
    let adjusted_year = year - i64::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
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
    fn status_cert_domains_with_an_entry_reports_https_configured() {
        let status =
            tailscale_status_json(br#"{"CertDomains":["desktop.taild260d0.ts.net"],"Self":{}}"#);
        assert_eq!(status.https_certificate, RemoteWebFeatureStatus::Configured);
    }

    #[test]
    fn status_empty_cert_domains_reports_https_not_configured() {
        let status = tailscale_status_json(br#"{"CertDomains":[],"Self":{}}"#);
        assert_eq!(
            status.https_certificate,
            RemoteWebFeatureStatus::NotConfigured
        );
    }

    #[test]
    fn status_missing_cert_domains_reports_https_not_configured() {
        let status = tailscale_status_json(br#"{"Self":{}}"#);
        assert_eq!(
            status.https_certificate,
            RemoteWebFeatureStatus::NotConfigured
        );
    }

    #[test]
    fn broken_status_json_makes_https_unknown_and_expiry_unavailable() {
        assert_eq!(
            tailscale_status_json(b"{not json"),
            unknown_tailscale_status()
        );
    }

    #[test]
    fn status_rfc3339_key_expiry_becomes_unix_seconds() {
        let status = tailscale_status_json(
            br#"{"CertDomains":[],"Self":{"KeyExpiry":"2026-02-08T09:00:00+09:00"}}"#,
        );
        assert_eq!(status.key_expiry_unix_seconds, Some(1_770_508_800));
    }

    #[test]
    fn observed_status_key_expiry_becomes_unix_seconds_without_an_expired_field() {
        // 実機の Self / Peer では Expired=false は出力されず、KeyExpiry だけが存在した。
        let status = tailscale_status_json(br#"{"Self":{"KeyExpiry":"2027-01-27T11:22:43Z"}}"#);
        assert_eq!(status.key_expiry_unix_seconds, Some(1_801_048_963));
    }

    #[test]
    fn status_null_key_expiry_is_unavailable() {
        let status = tailscale_status_json(br#"{"Self":{"KeyExpiry":null}}"#);
        assert_eq!(status.key_expiry_unix_seconds, None);
    }

    #[test]
    fn status_missing_key_expiry_is_unavailable() {
        let status = tailscale_status_json(br#"{"Self":{}}"#);
        assert_eq!(status.key_expiry_unix_seconds, None);
    }

    #[test]
    fn status_unparseable_key_expiry_is_unavailable() {
        for value in [
            br#"{"Self":{"KeyExpiry":"not-a-date"}}"#.as_slice(),
            br#"{"Self":{"KeyExpiry":"2026-02-30T00:00:00Z"}}"#.as_slice(),
        ] {
            assert_eq!(tailscale_status_json(value).key_expiry_unix_seconds, None);
        }
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
        assert_eq!(state.unsupported_path, None);
    }

    #[test]
    fn another_proxy_port_is_not_configured_and_reports_the_root_conflict() {
        let json = br#"{"Web":{"desktop4090.taild260d0.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:3000"}}}}}"#;
        let state = tailscale_serve_status_json(json, "127.0.0.1:8787".parse().unwrap());
        assert_eq!(state.status, RemoteWebFeatureStatus::NotConfigured);
        assert_eq!(state.url, None);
        assert_eq!(state.conflict.as_deref(), Some("http://127.0.0.1:3000"));
        assert_eq!(state.unsupported_path, None);
    }

    #[test]
    fn no_ts_net_entry_is_not_configured_without_a_conflict() {
        let json =
            br#"{"Web":{"localhost:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:3000"}}}}}"#;
        let state = tailscale_serve_status_json(json, "127.0.0.1:8787".parse().unwrap());
        assert_eq!(state.status, RemoteWebFeatureStatus::NotConfigured);
        assert_eq!(state.url, None);
        assert_eq!(state.conflict, None);
        assert_eq!(state.unsupported_path, None);
    }

    #[test]
    fn owned_non_root_handler_is_reported_as_unsupported_instead_of_configured() {
        let json = br#"{"Web":{"desktop4090.taild260d0.ts.net:443":{"Handlers":{"/miv":{"Proxy":"http://127.0.0.1:8787"}}}}}"#;
        let state = tailscale_serve_status_json(json, "127.0.0.1:8787".parse().unwrap());
        assert_eq!(state.status, RemoteWebFeatureStatus::NotConfigured);
        assert_eq!(state.url, None);
        assert_eq!(state.unsupported_path.as_deref(), Some("/miv"));
    }

    #[test]
    fn owned_root_handler_wins_when_an_owned_non_root_handler_also_exists() {
        let json = br#"{"Web":{"desktop4090.taild260d0.ts.net:443":{"Handlers":{"/miv":{"Proxy":"http://127.0.0.1:8787"},"/":{"Proxy":"http://127.0.0.1:8787"}}}}}"#;
        let state = tailscale_serve_status_json(json, "127.0.0.1:8787".parse().unwrap());
        assert_eq!(state.status, RemoteWebFeatureStatus::Configured);
        assert_eq!(
            state.url.as_deref(),
            Some("https://desktop4090.taild260d0.ts.net/")
        );
        assert_eq!(state.unsupported_path, None);
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
