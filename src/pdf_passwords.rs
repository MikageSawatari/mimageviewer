//! PDF パスワードの DPAPI 暗号化永続化モジュール。
//!
//! パスワードは Windows DPAPI (`Scope::User`) で暗号化され、
//! `%APPDATA%/mimageviewer/pdf_passwords.json` に保存される。
//! キーは PDF パスの SHA-256 ハッシュ（パス自体は保存しない）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use sha2::{Digest, Sha256};
use windows_dpapi::{encrypt_data, decrypt_data, Scope};

// -----------------------------------------------------------------------
// PdfPasswordStore
// -----------------------------------------------------------------------

/// PDF パスワードの暗号化ストア。
pub struct PdfPasswordStore {
    /// path_hash → base64-encoded DPAPI-encrypted password
    entries: HashMap<String, String>,
}

impl PdfPasswordStore {
    /// ストアファイルから読み込む。ファイルが無ければ空のストアを返す。
    pub fn load() -> Self {
        let path = Self::store_path();
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { entries }
    }

    /// ストアファイルに保存する。
    pub fn save(&self) {
        let path = Self::store_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.entries) {
            if let Err(e) = std::fs::write(&path, json) {
                eprintln!("pdf_passwords save failed: {e}");
            }
        }
    }

    /// 指定 PDF パスの保存済みパスワードを DPAPI で復号して返す。
    pub fn get(&self, pdf_path: &Path) -> Option<String> {
        let hash = Self::path_hash(pdf_path);
        let b64 = self.entries.get(&hash)?;
        let encrypted = BASE64.decode(b64).ok()?;
        let decrypted = decrypt_data(&encrypted, Scope::User, None).ok()?;
        String::from_utf8(decrypted).ok()
    }

    /// パスワードを DPAPI で暗号化して保存する。
    pub fn set(&mut self, pdf_path: &Path, password: &str) {
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

    /// 保存済みパスワードを削除する。
    pub fn remove(&mut self, pdf_path: &Path) {
        let hash = Self::path_hash(pdf_path);
        self.entries.remove(&hash);
    }

    // ── 内部 ────────────────────────────────────────────────

    fn store_path() -> PathBuf {
        crate::data_dir::get().join("pdf_passwords.json")
    }

    fn path_hash(pdf_path: &Path) -> String {
        let normalized = crate::path_key::normalize(pdf_path);
        let mut hasher = Sha256::new();
        hasher.update(normalized.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}
