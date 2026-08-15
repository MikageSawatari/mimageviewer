use std::net::SocketAddr;
use std::path::Path;

use serde_json::Value;

use crate::{RemoteWebFeatureStatus, run_tailscale_at, tailscale_executable};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailnetProbe {
    /// tailscale CLI を見つけられたか。false の場合、残りの状態は Unknown / None になる。
    pub cli_found: bool,
    pub serve: RemoteWebFeatureStatus,
    pub serve_url: Option<String>,
    pub serve_conflict: Option<String>,
    pub serve_unsupported_path: Option<String>,
    pub status_url: Option<String>,
    pub https_certificate: RemoteWebFeatureStatus,
    pub key_expiry_unix_seconds: Option<i64>,
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

pub fn probe_tailnet(address: SocketAddr) -> TailnetProbe {
    let executable = tailscale_executable();
    probe_tailnet_with_executable(executable.as_deref(), address)
}

fn probe_tailnet_with_executable(executable: Option<&Path>, address: SocketAddr) -> TailnetProbe {
    let Some(executable) = executable else {
        return TailnetProbe {
            cli_found: false,
            serve: RemoteWebFeatureStatus::Unknown,
            serve_url: None,
            serve_conflict: None,
            serve_unsupported_path: None,
            status_url: None,
            https_certificate: RemoteWebFeatureStatus::Unknown,
            key_expiry_unix_seconds: None,
        };
    };
    let serve = tailscale_serve_status(executable, address);
    let status = tailscale_status(executable);
    TailnetProbe {
        cli_found: true,
        serve: serve.status,
        serve_url: serve.url,
        serve_conflict: serve.conflict,
        serve_unsupported_path: serve.unsupported_path,
        status_url: status.url,
        https_certificate: status.https_certificate,
        key_expiry_unix_seconds: status.key_expiry_unix_seconds,
    }
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
    let url = path
        .starts_with('/')
        .then(|| format!("https://{host}{path}"))?;
    let authority = url
        .split_once("://")
        .map(|(_, remainder)| remainder)
        .unwrap_or_default()
        .split('/')
        .next()
        .unwrap_or_default();
    (!authority.is_empty() && !authority.contains('@') && !url.contains('?') && !url.contains('#'))
        .then(|| format!("{}/", url.trim_end_matches('/')))
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
        .and_then(|host| serve_url(&host, "/"));
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

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_ADDRESS: &str = "127.0.0.1:8787";

    fn proxy(port: &str) -> String {
        ["http", "://127.0.0.1:", port].concat()
    }

    #[test]
    fn missing_cli_has_one_unambiguous_unknown_result() {
        let probe = probe_tailnet_with_executable(None, TEST_ADDRESS.parse().unwrap());
        assert!(!probe.cli_found);
        assert_eq!(probe.serve, RemoteWebFeatureStatus::Unknown);
        assert_eq!(probe.serve_url, None);
        assert_eq!(probe.serve_conflict, None);
        assert_eq!(probe.serve_unsupported_path, None);
        assert_eq!(probe.status_url, None);
        assert_eq!(probe.https_certificate, RemoteWebFeatureStatus::Unknown);
        assert_eq!(probe.key_expiry_unix_seconds, None);
    }

    #[test]
    fn status_dns_name_becomes_https_url() {
        let value = serde_json::json!({"Self": {"DNSName": "desktop.taild260d0.ts.net."}});
        let status = inspect_tailscale_status(&value);
        let expected = ["https", "://", "desktop.taild260d0.ts.net/"].concat();
        assert_eq!(status.url.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn generated_tailnet_url_rejects_credentials() {
        assert_eq!(serve_url("user@desktop.taild260d0.ts.net", "/"), None);
    }

    #[test]
    fn status_cert_domains_with_an_entry_reports_https_configured() {
        let value = serde_json::json!({"CertDomains": ["desktop.taild260d0.ts.net"], "Self": {}});
        let status = inspect_tailscale_status(&value);
        assert_eq!(status.https_certificate, RemoteWebFeatureStatus::Configured);
    }

    #[test]
    fn status_empty_cert_domains_reports_https_not_configured() {
        let status = inspect_tailscale_status(&serde_json::json!({"CertDomains": [], "Self": {}}));
        assert_eq!(
            status.https_certificate,
            RemoteWebFeatureStatus::NotConfigured
        );
    }

    #[test]
    fn status_missing_cert_domains_reports_https_not_configured() {
        let status = inspect_tailscale_status(&serde_json::json!({"Self": {}}));
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
        let value = serde_json::json!({
            "CertDomains": [],
            "Self": {"KeyExpiry": "2026-02-08T09:00:00+09:00"}
        });
        let status = inspect_tailscale_status(&value);
        assert_eq!(status.key_expiry_unix_seconds, Some(1_770_508_800));
    }

    #[test]
    fn observed_status_key_expiry_becomes_unix_seconds_without_an_expired_field() {
        // 実機の Self / Peer では Expired=false は出力されず、KeyExpiry だけが存在した。
        let value = serde_json::json!({"Self": {"KeyExpiry": "2027-01-27T11:22:43Z"}});
        let status = inspect_tailscale_status(&value);
        assert_eq!(status.key_expiry_unix_seconds, Some(1_801_048_963));
    }

    #[test]
    fn status_null_key_expiry_is_unavailable() {
        let status = inspect_tailscale_status(&serde_json::json!({"Self": {"KeyExpiry": null}}));
        assert_eq!(status.key_expiry_unix_seconds, None);
    }

    #[test]
    fn status_missing_key_expiry_is_unavailable() {
        let status = inspect_tailscale_status(&serde_json::json!({"Self": {}}));
        assert_eq!(status.key_expiry_unix_seconds, None);
    }

    #[test]
    fn status_unparseable_key_expiry_is_unavailable() {
        for key_expiry in ["not-a-date", "2026-02-30T00:00:00Z"] {
            let value = serde_json::json!({"Self": {"KeyExpiry": key_expiry}});
            assert_eq!(
                inspect_tailscale_status(&value).key_expiry_unix_seconds,
                None
            );
        }
    }

    #[test]
    fn real_serve_json_is_configured_only_for_the_owned_proxy_target() {
        let value = serde_json::json!({
            "TCP": {"443": {"HTTPS": true}},
            "Web": {
                "desktop4090.taild260d0.ts.net:443": {
                    "Handlers": {"/": {"Proxy": proxy("8787")}}
                }
            }
        });
        let state = inspect_tailscale_serve(&value, TEST_ADDRESS.parse().unwrap());
        assert_eq!(state.status, RemoteWebFeatureStatus::Configured);
        let expected = ["https", "://", "desktop4090.taild260d0.ts.net/"].concat();
        assert_eq!(state.url.as_deref(), Some(expected.as_str()));
        assert_eq!(state.conflict, None);
        assert_eq!(state.unsupported_path, None);
    }

    #[test]
    fn another_proxy_port_is_not_configured_and_reports_the_root_conflict() {
        let conflict = proxy("3000");
        let value = serde_json::json!({
            "Web": {
                "desktop4090.taild260d0.ts.net:443": {
                    "Handlers": {"/": {"Proxy": conflict}}
                }
            }
        });
        let state = inspect_tailscale_serve(&value, TEST_ADDRESS.parse().unwrap());
        assert_eq!(state.status, RemoteWebFeatureStatus::NotConfigured);
        assert_eq!(state.url, None);
        assert_eq!(state.conflict.as_deref(), Some(conflict.as_str()));
        assert_eq!(state.unsupported_path, None);
    }

    #[test]
    fn no_ts_net_entry_is_not_configured_without_a_conflict() {
        let value = serde_json::json!({
            "Web": {
                "localhost:443": {
                    "Handlers": {"/": {"Proxy": proxy("3000")}}
                }
            }
        });
        let state = inspect_tailscale_serve(&value, TEST_ADDRESS.parse().unwrap());
        assert_eq!(state.status, RemoteWebFeatureStatus::NotConfigured);
        assert_eq!(state.url, None);
        assert_eq!(state.conflict, None);
        assert_eq!(state.unsupported_path, None);
    }

    #[test]
    fn owned_non_root_handler_is_reported_as_unsupported_instead_of_configured() {
        let value = serde_json::json!({
            "Web": {
                "desktop4090.taild260d0.ts.net:443": {
                    "Handlers": {"/miv": {"Proxy": proxy("8787")}}
                }
            }
        });
        let state = inspect_tailscale_serve(&value, TEST_ADDRESS.parse().unwrap());
        assert_eq!(state.status, RemoteWebFeatureStatus::NotConfigured);
        assert_eq!(state.url, None);
        assert_eq!(state.unsupported_path.as_deref(), Some("/miv"));
    }

    #[test]
    fn owned_root_handler_wins_when_an_owned_non_root_handler_also_exists() {
        let value = serde_json::json!({
            "Web": {
                "desktop4090.taild260d0.ts.net:443": {
                    "Handlers": {
                        "/miv": {"Proxy": proxy("8787")},
                        "/": {"Proxy": proxy("8787")}
                    }
                }
            }
        });
        let state = inspect_tailscale_serve(&value, TEST_ADDRESS.parse().unwrap());
        assert_eq!(state.status, RemoteWebFeatureStatus::Configured);
        assert_eq!(state.unsupported_path, None);
    }

    #[test]
    fn broken_serve_json_is_unknown() {
        let state = tailscale_serve_status_json(b"{not json", TEST_ADDRESS.parse().unwrap());
        assert_eq!(state, unknown_serve_state());
    }
}
