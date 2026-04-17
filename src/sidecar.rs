//! フォルダごとのサイドカーファイル (`mimageviewer.dat`) による補正・マスクバックアップ。
//!
//! 中央 DB (`adjustment.db` / `mask.db`) が authoritative で、サイドカーは移動耐性のための
//! バックアップ層。フォルダを丸ごと別ドライブへ移動すると中央 DB のパスキーが無効化されるが、
//! サイドカーは相対キーで保存されているため、新しい場所で初めて開いたときにインポートされて
//! 復元される。
//!
//! ## キー体系
//!
//! サイドカー内のキーは **フォルダ相対、小文字化**:
//!
//! | GridItem         | サイドカー置き場       | 相対キー                                      |
//! | ---------------- | ---------------------- | --------------------------------------------- |
//! | `Image(p)`       | `p.parent()`           | `"{filename_lower}"`                          |
//! | `ZipImage`       | `zip_path.parent()`    | `"{zip_filename_lower}::{entry_name_lower}"`  |
//! | `PdfPage`        | `pdf_path.parent()`    | `"{pdf_filename_lower}::page_{n}"`            |
//!
//! 相対キー → 絶対 DB キーへの再構成は [`reconstruct_adjust_key`] / [`reconstruct_mask_key`]。
//!
//! ## 動作の原則
//!
//! - 読み込み: `load_folder` 時に 1 度だけ、DB にエントリが無いものだけインポート。
//!   既に DB にあるエントリは無視 (中央が authoritative)。
//! - 書き込み: DB 更新と同じタイミングでメモリ上のサイドカーを更新 (`dirty = true`)。
//!   実ディスク書き込みは **フォルダ切替 / アプリ終了 / 5 秒アイドル** のいずれか。
//! - エラー処理: IO 失敗は黙ってログ 1 行、`disabled = true` で以降そのフォルダは無視
//!   (読み取り専用メディア対策)。
//! - 設定 OFF 時: 読み書き両方スキップ。既存ファイルは削除しない。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::adjustment::AdjustParams;

/// サイドカーファイル名。`.dat` は Windows 上で「アプリ内部データ」として広く認識される拡張子で、
/// ユーザが誤って編集・削除する心理的ハードルが高い。
pub const SIDECAR_FILENAME: &str = "mimageviewer.dat";

/// 現在のスキーマバージョン。互換性のない変更があったら上げる。
const CURRENT_VERSION: u32 = 1;

// ── JSON 形式 ─────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Default)]
struct SidecarJson {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    saved_at: Option<String>,
    #[serde(default)]
    items: BTreeMap<String, SidecarEntry>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct SidecarEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjust: Option<AdjustParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<SidecarMask>,
}

impl SidecarEntry {
    fn is_empty(&self) -> bool {
        self.adjust.is_none() && self.mask.is_none()
    }
}

/// 1bit/pixel に packed + deflate 圧縮されたマスクデータの base64。
/// mask_db と同じバイト列を base64 に掛けたもの。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SidecarMask {
    pub w: u32,
    pub h: u32,
    pub data: String,
}

impl SidecarMask {
    pub fn from_raw(raw: &[u8], w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            data: base64::engine::general_purpose::STANDARD.encode(raw),
        }
    }

    pub fn decode(&self) -> Option<Vec<u8>> {
        base64::engine::general_purpose::STANDARD
            .decode(self.data.as_bytes())
            .ok()
    }
}

// ── メモリ上のサイドカー ───────────────────────────────────────────────

/// フォルダごとに 1 個。dirty 管理と flush タイミングを保持する。
pub struct SidecarFile {
    folder: PathBuf,
    items: BTreeMap<String, SidecarEntry>,
    dirty: bool,
    /// 書き込み失敗後の再試行抑制フラグ。起動中一度失敗したら以降そのフォルダは書き込まない。
    disabled: bool,
    /// 最後に `mark_dirty` を呼ばれた時刻 (5 秒アイドル flush 判定用)。
    last_change: Option<Instant>,
}

impl SidecarFile {
    /// 空のサイドカーを新規作成する (ディスクからは読まない)。
    pub fn new(folder: PathBuf) -> Self {
        Self {
            folder,
            items: BTreeMap::new(),
            dirty: false,
            disabled: false,
            last_change: None,
        }
    }

    /// フォルダから `mimageviewer.dat` を読み込む。無ければ空のサイドカーを返す。
    /// パース失敗時もログ 1 行で空サイドカーを返す (古いバージョンや壊れたファイルで落ちない)。
    pub fn load(folder: &Path) -> Self {
        let mut me = Self::new(folder.to_path_buf());
        let path = folder.join(SIDECAR_FILENAME);
        let data = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => return me,
            Err(e) => {
                crate::logger::log(format!(
                    "sidecar: read failed: {} ({})",
                    path.display(),
                    e
                ));
                return me;
            }
        };
        let parsed: SidecarJson = match serde_json::from_str(&data) {
            Ok(v) => v,
            Err(e) => {
                crate::logger::log(format!(
                    "sidecar: JSON parse failed: {} ({})",
                    path.display(),
                    e
                ));
                return me;
            }
        };
        if parsed.version > CURRENT_VERSION {
            crate::logger::log(format!(
                "sidecar: skipping newer-version file: {} (v{})",
                path.display(),
                parsed.version
            ));
            // 上書きすると新バージョンのデータを失うので disabled にしておく
            me.disabled = true;
            return me;
        }
        me.items = parsed.items;
        me
    }

    // ── アクセッサ ────────────────────────────────────────────────

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn items(&self) -> &BTreeMap<String, SidecarEntry> {
        &self.items
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn last_change(&self) -> Option<Instant> {
        self.last_change
    }

    // ── 変更 ──────────────────────────────────────────────────────

    pub fn set_adjust(&mut self, rel_key: &str, params: AdjustParams) {
        let entry = self.items.entry(rel_key.to_string()).or_default();
        entry.adjust = Some(params);
        self.mark_dirty();
    }

    pub fn remove_adjust(&mut self, rel_key: &str) {
        if let Some(entry) = self.items.get_mut(rel_key) {
            if entry.adjust.is_some() {
                entry.adjust = None;
                if entry.is_empty() {
                    self.items.remove(rel_key);
                }
                self.mark_dirty();
            }
        }
    }

    pub fn set_mask(&mut self, rel_key: &str, mask: SidecarMask) {
        let entry = self.items.entry(rel_key.to_string()).or_default();
        entry.mask = Some(mask);
        self.mark_dirty();
    }

    pub fn remove_mask(&mut self, rel_key: &str) {
        if let Some(entry) = self.items.get_mut(rel_key) {
            if entry.mask.is_some() {
                entry.mask = None;
                if entry.is_empty() {
                    self.items.remove(rel_key);
                }
                self.mark_dirty();
            }
        }
    }

    /// 複数エントリの adjust を一括セット (「全画像に適用」用)。
    pub fn set_adjust_bulk<I>(&mut self, iter: I, params: &AdjustParams)
    where
        I: IntoIterator<Item = String>,
    {
        let mut changed = false;
        for rel_key in iter {
            let entry = self.items.entry(rel_key).or_default();
            entry.adjust = Some(params.clone());
            changed = true;
        }
        if changed {
            self.mark_dirty();
        }
    }

    /// 複数エントリの adjust を一括削除 (「全画像から削除」用)。
    pub fn remove_adjust_bulk<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = String>,
    {
        let mut changed = false;
        let keys: Vec<String> = iter.into_iter().collect();
        for rel_key in &keys {
            if let Some(entry) = self.items.get_mut(rel_key) {
                if entry.adjust.is_some() {
                    entry.adjust = None;
                    changed = true;
                }
            }
        }
        if changed {
            self.items.retain(|_, e| !e.is_empty());
            self.mark_dirty();
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.last_change = Some(Instant::now());
    }

    // ── 書き込み ──────────────────────────────────────────────────

    /// dirty ならディスクに書き出す (または空なら削除する)。dirty でなければ何もしない。
    /// 書き込み失敗時は `disabled = true` にして以降の書き込みをスキップ。
    pub fn flush(&mut self) {
        if !self.dirty || self.disabled {
            return;
        }
        let path = self.folder.join(SIDECAR_FILENAME);

        // 空なら削除
        if self.items.is_empty() {
            match std::fs::remove_file(&path) {
                Ok(_) => {
                    self.dirty = false;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => {
                    self.dirty = false;
                }
                Err(e) => {
                    crate::logger::log(format!(
                        "sidecar: remove failed: {} ({})",
                        path.display(),
                        e
                    ));
                    self.disabled = true;
                }
            }
            return;
        }

        let json_value = SidecarJson {
            version: CURRENT_VERSION,
            app: Some(format!("mimageviewer {}", env!("CARGO_PKG_VERSION"))),
            saved_at: Some(current_timestamp()),
            items: self.items.clone(),
        };
        let json = match serde_json::to_string_pretty(&json_value) {
            Ok(s) => s,
            Err(e) => {
                crate::logger::log(format!("sidecar: serialize failed: {e}"));
                self.disabled = true;
                return;
            }
        };

        // アトミック書き込み: temp → rename
        let tmp = self.folder.join(format!("{SIDECAR_FILENAME}.tmp"));
        if let Err(e) = std::fs::write(&tmp, &json) {
            crate::logger::log(format!(
                "sidecar: write failed: {} ({})",
                tmp.display(),
                e
            ));
            self.disabled = true;
            return;
        }
        // 既存ファイルの属性を一度クリアしないと rename が失敗するケースがあるため、
        // 既存ファイルがあれば属性を NORMAL に戻してから rename する。
        #[cfg(windows)]
        clear_hidden_system(&path);
        if let Err(e) = std::fs::rename(&tmp, &path) {
            crate::logger::log(format!(
                "sidecar: rename failed: {} -> {} ({})",
                tmp.display(),
                path.display(),
                e
            ));
            let _ = std::fs::remove_file(&tmp);
            self.disabled = true;
            return;
        }
        #[cfg(windows)]
        mark_hidden_system(&path);
        self.dirty = false;
    }
}

// ── キー再構成ヘルパー ─────────────────────────────────────────────────

/// Image 用の絶対 DB キー (= `adjustment_db::normalize_path` と同形式) を再構成する。
///
/// `folder` にサイドカーが置いてあるフォルダ、`rel_key` にサイドカー内の相対キー。
pub fn reconstruct_image_key(folder: &Path, rel_key: &str) -> String {
    let abs = folder.join(rel_key);
    crate::adjustment_db::normalize_path(&abs)
}

/// ZipImage / PdfPage 用の絶対 DB キー (`App::page_path_key` と同形式) を再構成する。
///
/// `rel_key` が `"archive.zip::entry.jpg"` または `"doc.pdf::page_5"` の形式であることが前提。
/// 不正な形式なら `None`。
pub fn reconstruct_virtual_key(folder: &Path, rel_key: &str) -> Option<String> {
    let (container, tail) = rel_key.split_once("::")?;
    let abs_container = folder.join(container);
    let container_norm = crate::adjustment_db::normalize_path(&abs_container);
    Some(format!("{container_norm}::{tail}"))
}

/// 相対キーの形が Image / ZipImage / PdfPage のどれかを判別する。
pub enum RelKeyKind {
    Image,
    ZipImage,
    PdfPage,
}

pub fn classify_rel_key(rel_key: &str) -> RelKeyKind {
    if let Some((_, tail)) = rel_key.split_once("::") {
        if tail.starts_with("page_") {
            RelKeyKind::PdfPage
        } else {
            RelKeyKind::ZipImage
        }
    } else {
        RelKeyKind::Image
    }
}

/// インポート結果の集計値。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ImportStats {
    pub imported_adjust: usize,
    pub imported_mask: usize,
    pub skipped_adjust: usize,
    pub skipped_mask: usize,
}

/// サイドカーの各エントリを中央 DB へインポートする (純粋関数、テスト用に App から分離)。
///
/// 中央 DB に既にエントリがあるものは **上書きしない** (中央が authoritative)。
/// `adjust_db` / `mask_db` に None を渡した場合、その DB 種別へのインポートはスキップ。
/// `folder` はサイドカーファイルが置かれているフォルダの絶対パス。
/// 絶対 DB キーの再構成は [`reconstruct_image_key`] / [`reconstruct_virtual_key`] に従う。
pub fn import_to_dbs(
    folder: &Path,
    sidecar: &SidecarFile,
    adjust_db: Option<&crate::adjustment_db::AdjustmentDb>,
    mask_db: Option<&crate::mask_db::MaskDb>,
) -> ImportStats {
    let mut stats = ImportStats::default();
    for (rel_key, entry) in sidecar.items() {
        let abs_key = match classify_rel_key(rel_key) {
            RelKeyKind::Image => reconstruct_image_key(folder, rel_key),
            RelKeyKind::ZipImage | RelKeyKind::PdfPage => {
                match reconstruct_virtual_key(folder, rel_key) {
                    Some(k) => k,
                    None => continue,
                }
            }
        };

        if let (Some(db), Some(params)) = (adjust_db, &entry.adjust) {
            if db.get_page_params(&abs_key).is_none() {
                if db.set_page_params(&abs_key, params).is_ok() {
                    stats.imported_adjust += 1;
                }
            } else {
                stats.skipped_adjust += 1;
            }
        }

        if let (Some(db), Some(mask)) = (mask_db, &entry.mask) {
            let w = mask.w as usize;
            let h = mask.h as usize;
            if w == 0 || h == 0 {
                continue;
            }
            if db.get(&abs_key, w, h).is_none() {
                if let Some(raw) = mask.decode() {
                    if db.set_raw(&abs_key, &raw, w, h).is_ok() {
                        stats.imported_mask += 1;
                    }
                }
            } else {
                stats.skipped_mask += 1;
            }
        }
    }
    stats
}

// ── Windows 隠し+システム属性 ─────────────────────────────────────────

#[cfg(windows)]
fn mark_hidden_system(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM,
    };
    use windows::core::PCWSTR;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        let _ = SetFileAttributesW(
            PCWSTR(wide.as_ptr()),
            FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM,
        );
    }
}

#[cfg(windows)]
fn clear_hidden_system(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_NORMAL};
    use windows::core::PCWSTR;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        // 存在しないパスに対する呼び出しは単にエラーになるだけ (TOCTOU 回避のため exists() チェックなし)。
        let _ = SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_ATTRIBUTE_NORMAL);
    }
}

// ── タイムスタンプ (ISO8601、タイムゾーン非依存の簡易版) ────────────────

fn current_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // ざっくり UTC のエポック秒ベースで表記 (タイムゾーン計算は避ける)
    format!("epoch:{secs}")
}

// ── テスト ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_params() -> AdjustParams {
        let mut p = AdjustParams::default();
        p.brightness = 10.0;
        p.contrast = -5.0;
        p
    }

    #[test]
    fn set_and_remove_adjust() {
        let mut s = SidecarFile::new(PathBuf::from("C:/tmp/nonexistent"));
        assert!(!s.is_dirty());
        s.set_adjust("img.jpg", sample_params());
        assert!(s.is_dirty());
        assert_eq!(s.items().len(), 1);
        s.remove_adjust("img.jpg");
        assert!(s.items().is_empty());
    }

    #[test]
    fn entry_empty_after_removing_both() {
        let mut s = SidecarFile::new(PathBuf::from("C:/tmp/nonexistent"));
        s.set_adjust("img.jpg", sample_params());
        s.set_mask("img.jpg", SidecarMask { w: 2, h: 2, data: String::new() });
        assert_eq!(s.items().len(), 1);
        s.remove_adjust("img.jpg");
        assert_eq!(s.items().len(), 1, "mask still present");
        s.remove_mask("img.jpg");
        assert!(s.items().is_empty(), "entry dropped when both gone");
    }

    #[test]
    fn reconstruct_image_key_matches_normalize() {
        let folder = PathBuf::from("C:\\Users\\Foo\\Pictures");
        let key = reconstruct_image_key(&folder, "photo.jpg");
        assert_eq!(key, "c:/users/foo/pictures/photo.jpg");
    }

    #[test]
    fn reconstruct_virtual_key_zip() {
        let folder = PathBuf::from("C:\\Books");
        let key = reconstruct_virtual_key(&folder, "vol1.zip::001.jpg").unwrap();
        assert_eq!(key, "c:/books/vol1.zip::001.jpg");
    }

    #[test]
    fn reconstruct_virtual_key_pdf() {
        let folder = PathBuf::from("C:\\Docs");
        let key = reconstruct_virtual_key(&folder, "manual.pdf::page_5").unwrap();
        assert_eq!(key, "c:/docs/manual.pdf::page_5");
    }

    #[test]
    fn classify_rel_key_works() {
        assert!(matches!(classify_rel_key("img.jpg"), RelKeyKind::Image));
        assert!(matches!(
            classify_rel_key("v.zip::a.jpg"),
            RelKeyKind::ZipImage
        ));
        assert!(matches!(
            classify_rel_key("d.pdf::page_0"),
            RelKeyKind::PdfPage
        ));
    }

    #[test]
    fn json_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().to_path_buf();
        {
            let mut s = SidecarFile::new(folder.clone());
            s.set_adjust("img.jpg", sample_params());
            s.set_mask(
                "book.zip::001.jpg",
                SidecarMask::from_raw(&[1, 2, 3, 4], 8, 8),
            );
            s.flush();
            assert!(!s.is_dirty());
        }
        let s2 = SidecarFile::load(&folder);
        assert_eq!(s2.items().len(), 2);
        let adj = s2.items().get("img.jpg").unwrap().adjust.as_ref().unwrap();
        assert_eq!(adj.brightness, 10.0);
        let mask = s2
            .items()
            .get("book.zip::001.jpg")
            .unwrap()
            .mask
            .as_ref()
            .unwrap();
        assert_eq!(mask.w, 8);
        assert_eq!(mask.decode().unwrap(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn flush_removes_file_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().to_path_buf();
        let path = folder.join(SIDECAR_FILENAME);

        let mut s = SidecarFile::new(folder.clone());
        s.set_adjust("img.jpg", sample_params());
        s.flush();
        assert!(path.exists());

        s.remove_adjust("img.jpg");
        s.flush();
        assert!(!path.exists(), "file should be removed when empty");
    }

    #[test]
    fn load_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let s = SidecarFile::load(dir.path());
        assert!(s.items().is_empty());
        assert!(!s.is_dirty());
    }
}
