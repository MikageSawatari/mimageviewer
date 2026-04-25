//! 起動時 / 定期的なバージョン更新チェック。
//!
//! GitHub Releases API (`/releases/latest`) を叩き、`tag_name` が現バージョンより
//! 新しければユーザーに通知する。通信は **バックグラウンドスレッドで非同期に実行**
//! し、UI スレッドは結果ハンドルを poll するだけ。失敗時は silent fail (オフライン
//! 環境でユーザーを煩わせないため)。
//!
//! ## バージョン比較
//! - mIV のリリースタグは `v0.8.1` 形式 (先頭 `v` + semver)。`semver::Version::parse`
//!   は `v` を受け付けないので strip してから比較する。
//! - リリース名 (`name`) は人間向けで信頼しない。判定は `tag_name` のみで行う。
//!
//! ## レート制限
//! - GitHub の未認証 API は IP あたり 60 req/h。1 ユーザーが 1 起動 + 24h ごとに 1 回
//!   なので余裕がある。`User-Agent` ヘッダ必須 (TOS 規定)。
//!
//! ## ユーザー設定
//! - `settings.update_check_enabled` (既定 ON) で全体 ON/OFF
//! - `settings.update_check_dismissed_version` で「このバージョンの通知は出さない」
//!   ユーザーが skip 選択した tag を覚える
//!
//! ## 「強制チェック」と auto チェックの違い
//! - auto: `update_check_enabled=false` なら走らない。失敗は silent
//! - manual (環境設定の「今すぐ確認」など): フラグ無視で常に走る。失敗を UI で表示

use std::sync::mpsc;
use std::time::Duration;

const RELEASES_LATEST_URL: &str =
    "https://api.github.com/repos/MikageSawatari/mimageviewer/releases/latest";
const RELEASES_PAGE_URL: &str =
    "https://github.com/MikageSawatari/mimageviewer/releases/latest";

/// 更新チェック結果。
#[derive(Clone, Debug)]
pub struct UpdateInfo {
    /// GitHub の `tag_name` (例: `"v0.8.2"`)
    pub latest_tag: String,
    /// `tag_name` から `v` を剥がして parse した semver
    pub latest_version: semver::Version,
    /// リリースページ URL (ブラウザで開く先)
    pub release_url: String,
    /// changelog 本文 (Markdown)。長い場合があるので UI で折りたたむ
    pub body: String,
    /// 現在の実行中バージョンより新しいか
    pub is_newer: bool,
}

/// バックグラウンドで GitHub に問い合わせる。結果は Receiver で 1 回だけ送信される。
///
/// `current_version` は `env!("CARGO_PKG_VERSION")` の値 (例: `"0.8.1"`) を渡す。
/// 内部で semver parse して比較する。
pub fn spawn_check(current_version: &str) -> mpsc::Receiver<Result<UpdateInfo, String>> {
    let (tx, rx) = mpsc::channel();
    let current = current_version.to_string();
    std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let result = perform_check(&current);
            let _ = tx.send(result);
        })
        .ok();
    rx
}

fn perform_check(current_version: &str) -> Result<UpdateInfo, String> {
    let current = semver::Version::parse(current_version)
        .map_err(|e| format!("current version parse: {e}"))?;
    let user_agent = format!("mImageViewer/{current_version}");
    let resp = ureq::get(RELEASES_LATEST_URL)
        .set("User-Agent", &user_agent)
        .set("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|e| format!("http: {e}"))?;
    let json: serde_json::Value = resp.into_json().map_err(|e| format!("json: {e}"))?;
    let tag = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or("missing tag_name")?
        .to_string();
    let url = json
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or(RELEASES_PAGE_URL)
        .to_string();
    let body = json
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let stripped = tag.strip_prefix('v').unwrap_or(&tag);
    let latest = semver::Version::parse(stripped)
        .map_err(|e| format!("tag '{tag}' parse: {e}"))?;
    let is_newer = latest > current;
    Ok(UpdateInfo {
        latest_tag: tag,
        latest_version: latest,
        release_url: url,
        body,
        is_newer,
    })
}

/// リリースページの URL (失敗時のフォールバック / 環境設定の「リリース履歴」リンク用)。
pub fn releases_page_url() -> &'static str {
    RELEASES_PAGE_URL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_version_parses() {
        // env!() の値が semver として valid であることを担保 (リリースで失敗しないように)
        let v = env!("CARGO_PKG_VERSION");
        semver::Version::parse(v).unwrap();
    }

    #[test]
    fn newer_tag_detection() {
        let cur = semver::Version::parse("0.8.1").unwrap();
        let newer = semver::Version::parse("0.8.2").unwrap();
        let same = semver::Version::parse("0.8.1").unwrap();
        let older = semver::Version::parse("0.7.9").unwrap();
        assert!(newer > cur);
        assert!(!(same > cur));
        assert!(!(older > cur));
    }

    #[test]
    fn tag_with_v_prefix_strips() {
        let tag = "v1.2.3";
        let stripped = tag.strip_prefix('v').unwrap_or(tag);
        semver::Version::parse(stripped).unwrap();
    }
}
