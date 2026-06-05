//! 編集用追加パック (オノマトペ向け OFL フォント + 被写体分離モデル) の
//! 配置・状態管理の基盤モジュール (ネットワーク非依存)。
//!
//! ## 役割
//!
//! 吹き出し / テキスト / オノマトペ機能と、補正レイヤーの被写体分離モデルは
//! 大きな追加ファイル (フォント ~62 MiB + BiRefNet fp16 ~490 MiB) を要するため、
//! 本体 exe に常時同梱せず、初回利用時に GitHub Releases からダウンロードして
//! `%APPDATA%/mimageviewer/addons/editing/` へ展開する (TensorRT pack と同方式)。
//!
//! 本モジュールは **パス解決・マニフェスト型・導入状態判定** のみを持ち、HTTP や
//! 展開は [`crate::editing_addon_download`] が担当する。ネットワークに触れない
//! 純ロジックなので単体テスト可能。`docs/editing-add-on-download-spec.md` を参照。
//!
//! ## ディスク構成
//!
//! ```text
//! %APPDATA%/mimageviewer/addons/editing/
//!   active.json                      ← 現在 active な pack version を指すポインタ
//!   downloads/                       ← *.zip.partial / 一時展開ディレクトリ
//!   packs/
//!     <version>/
//!       pack-manifest.json           ← pack 内容 (files + sha256 + license)
//!       INSTALL_OK                   ← 検証完了 sentinel (最後に atomic 書き込み)
//!       fonts/
//!         *.ttf
//!         *-OFL.txt
//!       models/
//!         birefnet_fp16.onnx
//!         BiRefNet-LICENSE.txt
//! ```
//!
//! ## 配布形態
//!
//! GitHub Releases に 2 アセットを置く (TRT pack 同様):
//!
//! - `editing-pack-index.json` … 利用可能 pack 一覧 ([`PackIndex`])。まず fetch。
//! - `editing-pack-<version>.zip` … pack 本体 (中に `pack-manifest.json` + fonts/ + models/)。
//!
//! ダウンロード処理は zip 全体の SHA-256 を検証 → `downloads/` で展開 →
//! 各ファイルの SHA-256 を pack-manifest と照合 → `packs/<version>/` へ atomic rename →
//! `active.json` を更新、という順で進む。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 対応する index スキーマ。`editing-pack-index.json` の `schema` がこれ以下なら読める。
pub const EXPECTED_INDEX_SCHEMA: u32 = 1;

/// 対応する pack マニフェストスキーマ。`pack-manifest.json` の `schema` がこれ以下なら読める。
pub const EXPECTED_PACK_SCHEMA: u32 = 1;

// ──────────────────────────────────────────────────────────────────────────
// パス解決
// ──────────────────────────────────────────────────────────────────────────

/// 編集用追加パックのルート。`%APPDATA%/mimageviewer/addons/editing/`
pub fn addon_root() -> PathBuf {
    crate::data_dir::get().join("addons").join("editing")
}

/// 現在 active な pack version を指すポインタファイル。`addons/editing/active.json`
pub fn active_pointer_path() -> PathBuf {
    addon_root().join("active.json")
}

/// 全 pack の親ディレクトリ。`addons/editing/packs/`
pub fn packs_root() -> PathBuf {
    addon_root().join("packs")
}

/// ダウンロード / 一時展開用ディレクトリ。`addons/editing/downloads/`
pub fn downloads_dir() -> PathBuf {
    addon_root().join("downloads")
}

/// 指定 version の pack ディレクトリ。`addons/editing/packs/<version>/`
///
/// `version` は信頼できない manifest 由来になり得るので、呼び出し前に
/// [`validate_version_dirname`] を通すこと (path traversal 防止)。
pub fn pack_dir(version: &str) -> PathBuf {
    packs_root().join(version)
}

/// 指定 version の pack マニフェストパス。`packs/<version>/pack-manifest.json`
pub fn pack_manifest_path(version: &str) -> PathBuf {
    pack_dir(version).join("pack-manifest.json")
}

/// 指定 version の導入完了 sentinel。`packs/<version>/INSTALL_OK`
pub fn install_sentinel_path(version: &str) -> PathBuf {
    pack_dir(version).join("INSTALL_OK")
}

// ──────────────────────────────────────────────────────────────────────────
// リモート index マニフェスト (editing-pack-index.json)
// ──────────────────────────────────────────────────────────────────────────

/// `editing-pack-index.json` のスキーマ。利用可能 pack の一覧を持つ。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PackIndex {
    /// index スキーマバージョン。`EXPECTED_INDEX_SCHEMA` 超は非対応。
    pub schema: u32,
    /// 配布されている pack 一覧 (新しい順とは限らない。選定は [`pick_pack`])。
    pub packs: Vec<IndexEntry>,
}

/// index 内の 1 pack エントリ。ダウンロード対象 zip とその検証情報。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IndexEntry {
    /// pack バージョン (= ディスク上のディレクトリ名 & active pointer の値)。
    pub version: String,
    /// この pack を入れるのに必要な mIV 最小バージョン (例: "1.1.0")。
    pub app_min_version: String,
    /// GitHub Releases のアセット名 (= zip ファイル名)。`<base>/<zip_name>` で DL。
    pub zip_name: String,
    /// zip 全体の SHA-256 (hex 64 文字)。
    pub zip_sha256: String,
    /// zip のサイズ (バイト)。進捗 UI の分母。
    pub zip_bytes: u64,
    /// 展開後の合計サイズ (バイト)。空き容量確認 / UI 表示用。
    #[serde(default)]
    pub uncompressed_bytes: u64,
    /// 同梱フォント数 (UI 表示用)。
    #[serde(default)]
    pub font_count: u32,
    /// 被写体分離モデルの表示名 (UI 表示用、例: "BiRefNet (fp16)")。
    #[serde(default)]
    pub subject_model: String,
}

impl IndexEntry {
    /// ダウンロードサイズの人間可読表記 (MB)。確認モーダルに出す。
    pub fn display_size_mb(&self) -> u64 {
        // 1 MiB 単位で四捨五入 (1_048_576 = 1 MiB)
        self.zip_bytes.div_ceil(1024 * 1024)
    }
}

/// 現在の mIV バージョンで導入可能な pack のうち最新を選ぶ。
///
/// `app_min_version <= 現在の mIV バージョン` を満たすものだけを候補にし、
/// その中で `version` が最大の pack を返す。候補が無ければ `None`。
pub fn pick_pack(index: &PackIndex) -> Option<&IndexEntry> {
    let app = env!("CARGO_PKG_VERSION");
    index
        .packs
        .iter()
        .filter(|p| version_at_least(app, &p.app_min_version))
        .max_by(|a, b| compare_versions(&a.version, &b.version))
}

// ──────────────────────────────────────────────────────────────────────────
// pack マニフェスト (pack-manifest.json、zip 内 + 展開後ディスク)
// ──────────────────────────────────────────────────────────────────────────

/// pack に含まれるファイルの種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    /// オノマトペ / テキスト用フォント (.ttf / .otf)。
    Font,
    /// 被写体分離 (foreground matte) ONNX モデル。
    SubjectMatteModel,
    /// ライセンス文面 (OFL / MIT 等のテキスト)。
    License,
    /// その他 (将来拡張用)。未知の文字列はこれに落ちる。
    #[serde(other)]
    Other,
}

/// pack 内の 1 ファイル。展開後の検証 + ライセンス表示に使う。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PackFile {
    /// pack ディレクトリからの相対パス (例: "fonts/OtomanopeeOne-Regular.ttf")。
    /// サブディレクトリ可だが `..` / 絶対パスは [`validate_pack_relpath`] で拒否。
    pub path: String,
    /// ファイル種別。
    pub kind: FileKind,
    /// ライセンス識別子 (例: "OFL-1.1" / "MIT")。
    #[serde(default)]
    pub license: String,
    /// SHA-256 (hex 64 文字)。展開後に照合。
    pub sha256: String,
    /// ファイルサイズ (バイト)。
    #[serde(default)]
    pub bytes: u64,
    /// モデル識別子 (kind == SubjectMatteModel のときのみ。例: "birefnet_fp16")。
    #[serde(default)]
    pub model_id: Option<String>,
}

/// pack マニフェスト本体。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PackManifest {
    /// pack スキーマバージョン。`EXPECTED_PACK_SCHEMA` 超は非対応。
    pub schema: u32,
    /// pack 識別子 (例: "editing-base")。
    pub pack_id: String,
    /// pack バージョン (= index の `version` と一致する)。
    pub version: String,
    /// 必要な mIV 最小バージョン。
    #[serde(default)]
    pub app_min_version: String,
    /// 含まれるファイル一覧。
    pub files: Vec<PackFile>,
}

impl PackManifest {
    /// フォントファイル (kind == Font) のみを列挙する。
    pub fn fonts(&self) -> impl Iterator<Item = &PackFile> {
        self.files.iter().filter(|f| f.kind == FileKind::Font)
    }

    /// 被写体分離モデル (kind == SubjectMatteModel) の最初のエントリを返す。
    pub fn subject_matte_model(&self) -> Option<&PackFile> {
        self.files
            .iter()
            .find(|f| f.kind == FileKind::SubjectMatteModel)
    }
}

/// `active.json` の中身。現在有効な pack version を 1 つだけ保持する。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ActivePointer {
    /// ポインタスキーマ (将来拡張用、現状 1 固定)。
    #[serde(default = "default_pointer_schema")]
    pub schema: u32,
    /// 現在 active な pack version。`packs/<active_version>/` を指す。
    pub active_version: String,
}

fn default_pointer_schema() -> u32 {
    1
}

// ──────────────────────────────────────────────────────────────────────────
// 導入状態の判定
// ──────────────────────────────────────────────────────────────────────────

/// 編集用追加パックの導入状態。
///
/// TRT pack の `PackStatus` と同じ思想で、UI / 機能側が「未導入なので DL を促す」
/// 「壊れているので手動削除を案内」を分岐できるよう細分化する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddonStatus {
    /// active pointer + sentinel + pack-manifest が揃い、schema も対応範囲内。
    Valid {
        /// 導入済み pack のバージョン。
        version: String,
    },
    /// active pointer が無い / sentinel が無い (= 未導入)。
    Missing,
    /// active pointer や pack-manifest のパース失敗、schema 非対応など。
    Corrupt(String),
}

impl AddonStatus {
    /// 導入済み (Valid) かどうか。
    pub fn is_installed(&self) -> bool {
        matches!(self, AddonStatus::Valid { .. })
    }
}

/// `active.json` を読む。存在しない / 壊れている場合は `None`。
pub fn read_active_pointer() -> Option<ActivePointer> {
    let raw = std::fs::read_to_string(active_pointer_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 指定 version の `pack-manifest.json` を読む。存在しない / 壊れている場合は `None`。
pub fn read_pack_manifest(version: &str) -> Option<PackManifest> {
    let raw = std::fs::read_to_string(pack_manifest_path(version)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 編集用追加パックの現状を返す。
///
/// 軽量な status チェック (active.json と pack-manifest.json は数 KB、sentinel は stat
/// のみ) なので UI スレッドから呼んでよい。フォント列挙のような走査は別関数。
pub fn addon_status() -> AddonStatus {
    let pointer = match read_active_pointer() {
        Some(p) => p,
        None => return AddonStatus::Missing,
    };
    if pointer.schema > 1 {
        return AddonStatus::Corrupt(format!("active.json schema {} 非対応", pointer.schema));
    }
    let version = pointer.active_version;
    if version.is_empty() {
        return AddonStatus::Corrupt("active.json の active_version が空です".to_string());
    }
    // sentinel が無ければ未導入扱い (= DL 途中で中断した残骸かもしれない)。
    if !install_sentinel_path(&version).exists() {
        return AddonStatus::Missing;
    }
    let manifest = match read_pack_manifest(&version) {
        Some(m) => m,
        None => {
            return AddonStatus::Corrupt(format!("pack-manifest.json ({}) を読めません", version));
        }
    };
    if manifest.schema > EXPECTED_PACK_SCHEMA {
        return AddonStatus::Corrupt(format!(
            "pack schema {} は本バージョンの mIV では非対応です (対応上限 {})",
            manifest.schema, EXPECTED_PACK_SCHEMA
        ));
    }
    AddonStatus::Valid { version }
}

/// 導入済み (Valid) かどうかの簡易判定。
pub fn is_installed() -> bool {
    addon_status().is_installed()
}

/// 現在 active な pack ディレクトリ。未導入なら `None`。
pub fn active_pack_dir() -> Option<PathBuf> {
    match addon_status() {
        AddonStatus::Valid { version } => Some(pack_dir(&version)),
        _ => None,
    }
}

/// 導入済みフォントファイル (.ttf / .otf) の絶対パス一覧を返す。
/// 未導入なら空。`fonts/` ディレクトリを走査する。
///
/// CLAUDE.md の指針に従い `Path::is_file()` を呼ばず、拡張子フィルタのみで判定する
/// (per-entry の `GetFileAttributes` syscall を避ける)。
pub fn installed_fonts() -> Vec<PathBuf> {
    let Some(dir) = active_pack_dir() else {
        return Vec::new();
    };
    let fonts_dir = dir.join("fonts");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&fonts_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let lower = name.to_ascii_lowercase();
            if lower.ends_with(".ttf") || lower.ends_with(".otf") {
                out.push(fonts_dir.join(name.as_ref()));
            }
        }
    }
    out.sort();
    out
}

/// 被写体分離モデルの絶対パス。導入済みかつ manifest にモデルがあれば `Some`。
pub fn subject_matte_model_path() -> Option<PathBuf> {
    let AddonStatus::Valid { version } = addon_status() else {
        return None;
    };
    let manifest = read_pack_manifest(&version)?;
    let model = manifest.subject_matte_model()?;
    // path は manifest 由来なので念のため検証してから join。
    if validate_pack_relpath(&model.path).is_err() {
        return None;
    }
    let p = pack_dir(&version).join(&model.path);
    if p.exists() { Some(p) } else { None }
}

// ──────────────────────────────────────────────────────────────────────────
// バージョン比較 (semver 簡易版)
// ──────────────────────────────────────────────────────────────────────────

/// "1.2.3" / "2026.06.0" のようなドット区切りバージョンを数値ベクタにパースする。
/// 数値でない / 欠けた要素は 0 扱い。prerelease タグ (`-rc1` 等) は無視する。
fn parse_version(v: &str) -> Vec<u64> {
    v.split('-')
        .next()
        .unwrap_or("")
        .split('.')
        .map(|c| c.trim().parse::<u64>().unwrap_or(0))
        .collect()
}

/// 2 つのバージョンを比較する (a <=> b)。長さが違う場合は不足分を 0 で補う。
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let va = parse_version(a);
    let vb = parse_version(b);
    let n = va.len().max(vb.len());
    for i in 0..n {
        let x = va.get(i).copied().unwrap_or(0);
        let y = vb.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

/// `have` が `need` 以上か (have >= need)。app バージョン互換性判定に使う。
pub fn version_at_least(have: &str, need: &str) -> bool {
    if need.trim().is_empty() {
        return true; // app_min_version 未指定 = 制約なし
    }
    compare_versions(have, need) != std::cmp::Ordering::Less
}

// ──────────────────────────────────────────────────────────────────────────
// 信頼できない入力の検証 (path traversal 防止)
// ──────────────────────────────────────────────────────────────────────────

/// pack version をディレクトリ名に使う前に検証する。
///
/// SECURITY: index / pointer 由来の `version` は `packs/<version>/` の一部になるため、
/// path separator / `..` / 絶対パス指定を拒否する。許容するのは英数 + `.` `-` `_` のみ。
pub fn validate_version_dirname(version: &str) -> Result<(), String> {
    if version.is_empty() {
        return Err("pack version が空です".to_string());
    }
    if version.len() > 64 {
        return Err("pack version が長すぎます".to_string());
    }
    if version.contains('/') || version.contains('\\') || version.contains(':') {
        return Err(format!(
            "pack version に path separator が含まれます: {version:?}"
        ));
    }
    if version.contains("..") {
        return Err(format!("pack version に `..` が含まれます: {version:?}"));
    }
    if !version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err(format!(
            "pack version に使用できない文字が含まれます (英数 . - _ のみ可): {version:?}"
        ));
    }
    Ok(())
}

/// pack 内ファイルの相対パス (`fonts/X.ttf` 等) を pack_dir に join する前に検証する。
///
/// SECURITY: zip 内 / manifest 内のパスは `..` や絶対パスを含み得るので、pack_dir の
/// 外に書き出されないよう各コンポーネントを検証する。サブディレクトリは許可するが:
/// - 空でない
/// - 絶対パスでない (先頭が `/` `\` やドライブレターでない)
/// - どのコンポーネントも `..` / `.` でない
/// - Windows drive separator `:` を含まない
pub fn validate_pack_relpath(rel: &str) -> Result<(), String> {
    if rel.is_empty() {
        return Err("pack file path が空です".to_string());
    }
    if rel.contains(':') {
        return Err(format!("pack file path に `:` が含まれます: {rel:?}"));
    }
    if rel.starts_with('/') || rel.starts_with('\\') {
        return Err(format!("pack file path が絶対パスです: {rel:?}"));
    }
    // `/` と `\` の両方を separator として分解し、各コンポーネントを検査。
    for comp in rel.split(['/', '\\']) {
        if comp.is_empty() {
            return Err(format!(
                "pack file path に空コンポーネントがあります: {rel:?}"
            ));
        }
        if comp == ".." || comp == "." {
            return Err(format!("pack file path に `.`/`..` があります: {rel:?}"));
        }
    }
    Ok(())
}

/// 検証済み相対パスを base に安全に join する。`validate_pack_relpath` を内部で呼ぶ。
pub fn join_pack_relpath(base: &Path, rel: &str) -> Result<PathBuf, String> {
    validate_pack_relpath(rel)?;
    Ok(base.join(rel.replace('\\', "/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parse_and_compare() {
        assert_eq!(parse_version("1.2.3"), vec![1, 2, 3]);
        assert_eq!(parse_version("2026.06.0"), vec![2026, 6, 0]);
        assert_eq!(parse_version("1.1.0-rc2"), vec![1, 1, 0]);
        assert_eq!(parse_version(""), vec![0]);

        use std::cmp::Ordering;
        assert_eq!(compare_versions("1.1.0", "1.1.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.2.0", "1.1.9"), Ordering::Greater);
        assert_eq!(compare_versions("1.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0.0", "1.0.1"), Ordering::Less);
        assert_eq!(compare_versions("2.0", "10.0"), Ordering::Less);
    }

    #[test]
    fn version_at_least_basics() {
        assert!(version_at_least("1.1.0", "1.1.0"));
        assert!(version_at_least("1.2.0", "1.1.0"));
        assert!(!version_at_least("1.0.0", "1.1.0"));
        // 制約なし
        assert!(version_at_least("1.0.0", ""));
        assert!(version_at_least("1.0.0", "   "));
    }

    #[test]
    fn pick_pack_selects_latest_compatible() {
        // 現在の app バージョンに依存しないよう app_min_version を 0.0.0 にして
        // 「互換のうち最大 version が選ばれる」ことだけを検証する。
        let index = PackIndex {
            schema: 1,
            packs: vec![
                IndexEntry {
                    version: "2026.05.0".to_string(),
                    app_min_version: "0.0.0".to_string(),
                    zip_name: "a.zip".to_string(),
                    zip_sha256: "0".repeat(64),
                    zip_bytes: 1,
                    uncompressed_bytes: 0,
                    font_count: 0,
                    subject_model: String::new(),
                },
                IndexEntry {
                    version: "2026.06.0".to_string(),
                    app_min_version: "0.0.0".to_string(),
                    zip_name: "b.zip".to_string(),
                    zip_sha256: "0".repeat(64),
                    zip_bytes: 1,
                    uncompressed_bytes: 0,
                    font_count: 0,
                    subject_model: String::new(),
                },
            ],
        };
        let picked = pick_pack(&index).expect("should pick");
        assert_eq!(picked.version, "2026.06.0");
    }

    #[test]
    fn pick_pack_skips_incompatible_app() {
        // app_min_version が天文学的に高い pack は選ばれない。
        let index = PackIndex {
            schema: 1,
            packs: vec![IndexEntry {
                version: "9999.0.0".to_string(),
                app_min_version: "9999.0.0".to_string(),
                zip_name: "future.zip".to_string(),
                zip_sha256: "0".repeat(64),
                zip_bytes: 1,
                uncompressed_bytes: 0,
                font_count: 0,
                subject_model: String::new(),
            }],
        };
        assert!(pick_pack(&index).is_none());
    }

    #[test]
    fn display_size_rounds_up() {
        let e = IndexEntry {
            version: "1".to_string(),
            app_min_version: "0".to_string(),
            zip_name: "x".to_string(),
            zip_sha256: "0".repeat(64),
            zip_bytes: 1024 * 1024 + 1,
            uncompressed_bytes: 0,
            font_count: 0,
            subject_model: String::new(),
        };
        assert_eq!(e.display_size_mb(), 2);
        let e0 = IndexEntry {
            zip_bytes: 0,
            ..e.clone()
        };
        assert_eq!(e0.display_size_mb(), 0);
        let e_exact = IndexEntry {
            zip_bytes: 5 * 1024 * 1024,
            ..e
        };
        assert_eq!(e_exact.display_size_mb(), 5);
    }

    #[test]
    fn validate_version_accepts_normal() {
        assert!(validate_version_dirname("2026.06.0").is_ok());
        assert!(validate_version_dirname("1.1.0-rc1").is_ok());
        assert!(validate_version_dirname("editing_base_v1").is_ok());
    }

    #[test]
    fn validate_version_rejects_traversal() {
        assert!(validate_version_dirname("").is_err());
        assert!(validate_version_dirname("..").is_err());
        assert!(validate_version_dirname("../evil").is_err());
        assert!(validate_version_dirname("a/b").is_err());
        assert!(validate_version_dirname("a\\b").is_err());
        assert!(validate_version_dirname("C:evil").is_err());
        assert!(validate_version_dirname("a b").is_err()); // space not allowed
        assert!(validate_version_dirname(&"x".repeat(65)).is_err());
    }

    #[test]
    fn validate_relpath_accepts_subdirs() {
        assert!(validate_pack_relpath("fonts/OtomanopeeOne-Regular.ttf").is_ok());
        assert!(validate_pack_relpath("models/birefnet_fp16.onnx").is_ok());
        assert!(validate_pack_relpath("pack-manifest.json").is_ok());
    }

    #[test]
    fn validate_relpath_rejects_traversal() {
        assert!(validate_pack_relpath("").is_err());
        assert!(validate_pack_relpath("../secret").is_err());
        assert!(validate_pack_relpath("fonts/../../etc/passwd").is_err());
        assert!(validate_pack_relpath("/abs/path").is_err());
        assert!(validate_pack_relpath("\\abs\\path").is_err());
        assert!(validate_pack_relpath("C:/Windows/system32").is_err());
        assert!(validate_pack_relpath("fonts//double").is_err());
        assert!(validate_pack_relpath("./hidden").is_err());
    }

    #[test]
    fn join_pack_relpath_normalizes_backslash() {
        let base = Path::new("C:/packs/v1");
        let joined = join_pack_relpath(base, "fonts\\X.ttf").expect("ok");
        assert_eq!(joined, base.join("fonts/X.ttf"));
        assert!(join_pack_relpath(base, "../evil").is_err());
    }

    #[test]
    fn manifest_helpers_filter_by_kind() {
        let m = PackManifest {
            schema: 1,
            pack_id: "editing-base".to_string(),
            version: "2026.06.0".to_string(),
            app_min_version: "1.1.0".to_string(),
            files: vec![
                PackFile {
                    path: "fonts/A.ttf".to_string(),
                    kind: FileKind::Font,
                    license: "OFL-1.1".to_string(),
                    sha256: "0".repeat(64),
                    bytes: 1,
                    model_id: None,
                },
                PackFile {
                    path: "fonts/B.ttf".to_string(),
                    kind: FileKind::Font,
                    license: "OFL-1.1".to_string(),
                    sha256: "0".repeat(64),
                    bytes: 1,
                    model_id: None,
                },
                PackFile {
                    path: "models/birefnet_fp16.onnx".to_string(),
                    kind: FileKind::SubjectMatteModel,
                    license: "MIT".to_string(),
                    sha256: "0".repeat(64),
                    bytes: 1,
                    model_id: Some("birefnet_fp16".to_string()),
                },
            ],
        };
        assert_eq!(m.fonts().count(), 2);
        assert_eq!(
            m.subject_matte_model().and_then(|f| f.model_id.clone()),
            Some("birefnet_fp16".to_string())
        );
    }

    #[test]
    fn pack_manifest_parses_spec_example() {
        // docs/editing-add-on-download-spec.md §6 のマニフェスト例が現スキーマで読めること。
        let json = r#"{
            "schema": 1,
            "pack_id": "editing-base",
            "version": "2026.06.0",
            "app_min_version": "1.1.0",
            "files": [
                {"path":"fonts/OtomanopeeOne-Regular.ttf","kind":"font","license":"OFL-1.1","sha256":"abc","bytes":123},
                {"path":"models/birefnet_fp16.onnx","kind":"subject_matte_model","model_id":"birefnet_fp16","license":"MIT","sha256":"def","bytes":456}
            ]
        }"#;
        let m: PackManifest = serde_json::from_str(json).expect("parse");
        assert_eq!(m.pack_id, "editing-base");
        assert_eq!(m.files.len(), 2);
        assert_eq!(m.files[0].kind, FileKind::Font);
        assert_eq!(m.files[1].kind, FileKind::SubjectMatteModel);
    }

    #[test]
    fn file_kind_unknown_falls_back_to_other() {
        let json = r#"{"path":"x","kind":"weird_future_kind","sha256":"0"}"#;
        let f: PackFile = serde_json::from_str(json).expect("parse");
        assert_eq!(f.kind, FileKind::Other);
    }

    #[test]
    fn active_pointer_roundtrip() {
        let p = ActivePointer {
            schema: 1,
            active_version: "2026.06.0".to_string(),
        };
        let s = serde_json::to_string(&p).expect("ser");
        let back: ActivePointer = serde_json::from_str(&s).expect("de");
        assert_eq!(back.active_version, "2026.06.0");
        // schema 省略時のデフォルト
        let back2: ActivePointer = serde_json::from_str(r#"{"active_version":"x"}"#).expect("de2");
        assert_eq!(back2.schema, 1);
    }
}
