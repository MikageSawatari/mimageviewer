//! PDF パスワードの DPAPI 暗号化永続化モジュール。
//!
//! パスワードは Windows DPAPI (`Scope::User`) で暗号化され、
//! `%APPDATA%/mimageviewer/pdf_passwords.json` に保存される。
//! キーは PDF パスの SHA-256 ハッシュ（パス自体は保存しない）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use sha2::{Digest, Sha256};
// windows-dpapi クレートは中身がすべて #[cfg(windows)] で、非 Windows ターゲット
// では空クレートになる (= この use が unresolved import でコンパイル不能になる)。
// 非 Windows ビルド (CI の ubuntu cargo check) を通すため use と暗号化経路を
// cfg(windows) でガードし、非 Windows では get=None / set=no-op にする。
#[cfg(windows)]
use windows_dpapi::{Scope, decrypt_data, encrypt_data};

// -----------------------------------------------------------------------
// PdfPasswordStore
// -----------------------------------------------------------------------

/// PDF パスワードの暗号化ストア。
///
/// `Clone` は Ctrl+F の PDF document info 照合 (`run_metadata_search`) が
/// ワーカースレッドへストアごと渡せるようにするため。中身は小さな
/// `HashMap<String, String>` 1 個なので clone コストは無視できる。
#[derive(Clone)]
pub struct PdfPasswordStore {
    /// path_hash → base64-encoded DPAPI-encrypted password
    entries: HashMap<String, String>,
}

impl PdfPasswordStore {
    /// ストアファイルから読み込む。ファイルが無ければ空のストアを返す。
    pub fn load() -> Self {
        Self::load_at(&Self::store_path())
    }

    fn load_at(path: &Path) -> Self {
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { entries }
    }

    /// delete worker 用。保存済みパスワードが 1 件でもある場合だけ、削除前の PDF
    /// path 列挙が必要になる。ファイル不在・空・壊れた JSON は load_at と同じく空扱い。
    pub(crate) fn has_entries_at(data_dir: &Path) -> bool {
        !Self::load_at(&data_dir.join("pdf_passwords.json"))
            .entries
            .is_empty()
    }

    /// ストアファイルに保存する。
    pub fn save(&self) {
        let path = Self::store_path();
        if let Err(e) = self.save_at(&path) {
            eprintln!("pdf_passwords save failed: {e}");
        }
    }

    fn save_at(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&self.entries)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(path, json)
    }

    /// 指定 PDF パスの保存済みパスワードを DPAPI で復号して返す。
    /// 非 Windows では DPAPI が無いので常に None (保存済みパスワード機能は無効)。
    pub fn get(&self, pdf_path: &Path) -> Option<String> {
        #[cfg(windows)]
        {
            let hash = Self::path_hash(pdf_path);
            let b64 = self.entries.get(&hash)?;
            let encrypted = BASE64.decode(b64).ok()?;
            let decrypted = decrypt_data(&encrypted, Scope::User, None).ok()?;
            String::from_utf8(decrypted).ok()
        }
        #[cfg(not(windows))]
        {
            let _ = pdf_path;
            None
        }
    }

    /// パスワードを DPAPI で暗号化して保存する。非 Windows では no-op。
    pub fn set(&mut self, pdf_path: &Path, password: &str) {
        #[cfg(windows)]
        {
            let hash = Self::path_hash(pdf_path);
            match encrypt_data(password.as_bytes(), Scope::User, None) {
                Ok(encrypted) => {
                    self.entries.insert(hash, BASE64.encode(&encrypted));
                }
                Err(e) => {
                    eprintln!("pdf_passwords DPAPI encrypt failed: {e}");
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (pdf_path, password);
        }
    }

    /// 保存済みパスワードを削除する。
    pub fn remove(&mut self, pdf_path: &Path) {
        let hash = Self::path_hash(pdf_path);
        self.entries.remove(&hash);
    }

    // ── 内部 ────────────────────────────────────────────────

    fn store_path() -> PathBuf {
        crate::data_dir::get().join("pdf_passwords.json")
    }

    pub(crate) fn path_hash(pdf_path: &Path) -> String {
        let normalized = crate::path_key::normalize(pdf_path);
        let mut hasher = Sha256::new();
        hasher.update(normalized.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// delete worker 用。削除前に列挙した PDF path のハッシュ行だけを hard purge する。
    /// path 自体を保存しない既存形式のため、フォルダ削除では worker が削除前に配下 PDF を
    /// 列挙してからこの関数へ渡す。
    pub(crate) fn purge_paths_at(data_dir: &Path, pdf_paths: &[PathBuf]) -> std::io::Result<usize> {
        if pdf_paths.is_empty() {
            return Ok(0);
        }
        let path = data_dir.join("pdf_passwords.json");
        if !path.exists() {
            return Ok(0);
        }
        let mut store = Self::load_at(&path);
        let mut removed = 0usize;
        for pdf_path in pdf_paths {
            removed += usize::from(store.entries.remove(&Self::path_hash(pdf_path)).is_some());
        }
        if removed > 0 {
            store.save_at(&path)?;
        }
        Ok(removed)
    }
}

#[cfg(test)]
impl PdfPasswordStore {
    /// テスト用: ディスク (data_dir) に触れない空ストア。
    pub fn empty_for_test() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}
