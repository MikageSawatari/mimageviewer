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
use crate::mask_db::Shape;

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
    /// 隠蔽加工マスク (Phase 4 で追加)。`mask` (消しゴム) と並列のサブシステム。
    /// 形式は `SidecarMask` と同一 (1bit/pixel + deflate + base64 + Shape ベクタ群) で、
    /// 用途が異なるだけ。両者を 1 ファイルに同居させても容量影響は小さい。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conceal: Option<SidecarMask>,
}

impl SidecarEntry {
    fn is_empty(&self) -> bool {
        self.adjust.is_none() && self.mask.is_none() && self.conceal.is_none()
    }
}

/// 1bit/pixel に packed + deflate 圧縮されたマスクデータの base64。
/// mask_db と同じバイト列を base64 に掛けたもの。
/// `vectors` は Shape (Line / Rect / Ellipse) のベクタオブジェクト (未指定 = なし)。
///
/// JSON 互換性: フィールド名は歴史的経緯で `vectors` のまま (リリース済みデータ)。
/// 旧版が書いた `Vec<LineObject>` JSON は `Shape::deserialize` の legacy 経路で
/// `Shape::Line` として読める ([`crate::mask_db`] 参照)。新版は常にタグ付き
/// `Vec<Shape>` を書き戻す。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SidecarMask {
    pub w: u32,
    pub h: u32,
    pub data: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vectors: Vec<Shape>,
}

impl SidecarMask {
    pub fn from_raw(raw: &[u8], shapes: &[Shape], w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            data: base64::engine::general_purpose::STANDARD.encode(raw),
            vectors: shapes.to_vec(),
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
                crate::logger::log(format!("sidecar: read failed: {} ({})", path.display(), e));
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

    /// 隠蔽加工マスクをセットする (Phase 4)。形式は `SidecarMask` と共通。
    pub fn set_conceal(&mut self, rel_key: &str, conceal: SidecarMask) {
        let entry = self.items.entry(rel_key.to_string()).or_default();
        entry.conceal = Some(conceal);
        self.mark_dirty();
    }

    /// 隠蔽加工マスクを取り除く (Phase 4)。
    pub fn remove_conceal(&mut self, rel_key: &str) {
        if let Some(entry) = self.items.get_mut(rel_key) {
            if entry.conceal.is_some() {
                entry.conceal = None;
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
            crate::logger::log(format!("sidecar: write failed: {} ({})", tmp.display(), e));
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
    pub imported_conceal: usize,
    pub skipped_adjust: usize,
    pub skipped_mask: usize,
    pub skipped_conceal: usize,
}

/// サイドカーの各エントリを中央 DB へインポートする (純粋関数、テスト用に App から分離)。
///
/// 中央 DB に既にエントリがあるものは **上書きしない** (中央が authoritative)。
/// `adjust_db` / `mask_db` / `conceal_db` に None を渡した場合、その DB 種別へのインポートは
/// スキップ。`folder` はサイドカーファイルが置かれているフォルダの絶対パス。
/// 絶対 DB キーの再構成は [`reconstruct_image_key`] / [`reconstruct_virtual_key`] に従う。
pub fn import_to_dbs(
    folder: &Path,
    sidecar: &SidecarFile,
    adjust_db: Option<&crate::adjustment_db::AdjustmentDb>,
    mask_db: Option<&crate::mask_db::MaskDb>,
    conceal_db: Option<&crate::conceal_db::ConcealDb>,
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
            if w > 0 && h > 0 {
                if db.get(&abs_key, w, h).is_none() {
                    if let Some(raw) = mask.decode() {
                        let vectors_json = crate::mask_db::shapes_to_json(&mask.vectors);
                        if db
                            .set_raw(&abs_key, &raw, vectors_json.as_deref(), w, h)
                            .is_ok()
                        {
                            stats.imported_mask += 1;
                        }
                    }
                } else {
                    stats.skipped_mask += 1;
                }
            }
        }

        if let (Some(db), Some(conceal)) = (conceal_db, &entry.conceal) {
            let w = conceal.w as usize;
            let h = conceal.h as usize;
            if w > 0 && h > 0 {
                if db.get_full(&abs_key, w, h).is_none() {
                    if let Some(raw) = conceal.decode() {
                        let shapes_json = crate::mask_db::shapes_to_json(&conceal.vectors);
                        if db
                            .set_raw(&abs_key, &raw, shapes_json.as_deref(), w, h)
                            .is_ok()
                        {
                            stats.imported_conceal += 1;
                        }
                    }
                } else {
                    stats.skipped_conceal += 1;
                }
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
        FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM, SetFileAttributesW,
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
    use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_NORMAL, SetFileAttributesW};
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
        s.set_mask(
            "img.jpg",
            SidecarMask {
                w: 2,
                h: 2,
                data: String::new(),
                vectors: Vec::new(),
            },
        );
        assert_eq!(s.items().len(), 1);
        s.remove_adjust("img.jpg");
        assert_eq!(s.items().len(), 1, "mask still present");
        s.remove_mask("img.jpg");
        assert!(s.items().is_empty(), "entry dropped when both gone");
    }

    #[test]
    fn entry_empty_after_removing_all_three_kinds() {
        // Phase 4: adjust / mask / conceal の 3 種類すべてに対する is_empty 連動
        let mut s = SidecarFile::new(PathBuf::from("C:/tmp/nonexistent"));
        s.set_adjust("img.jpg", sample_params());
        s.set_mask(
            "img.jpg",
            SidecarMask {
                w: 2,
                h: 2,
                data: String::new(),
                vectors: Vec::new(),
            },
        );
        s.set_conceal(
            "img.jpg",
            SidecarMask {
                w: 4,
                h: 4,
                data: String::from("xxxx"),
                vectors: Vec::new(),
            },
        );
        assert_eq!(s.items().len(), 1);
        s.remove_adjust("img.jpg");
        assert_eq!(s.items().len(), 1, "mask + conceal still present");
        s.remove_mask("img.jpg");
        assert_eq!(s.items().len(), 1, "conceal still present");
        s.remove_conceal("img.jpg");
        assert!(s.items().is_empty(), "entry dropped when all three gone");
    }

    #[test]
    fn set_conceal_is_independent_of_mask() {
        // Phase 4: mask と conceal は別フィールドなので、片方だけセットされた状態を
        // ラウンドトリップしても干渉しない (= mask が空でも conceal は保持される)
        let mut s = SidecarFile::new(PathBuf::from("C:/tmp/nonexistent"));
        s.set_conceal(
            "img.jpg",
            SidecarMask {
                w: 8,
                h: 8,
                data: String::from("zzzz"),
                vectors: Vec::new(),
            },
        );
        let entry = s.items().get("img.jpg").unwrap();
        assert!(entry.mask.is_none());
        assert!(entry.conceal.is_some());
        assert_eq!(entry.conceal.as_ref().unwrap().w, 8);
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
                SidecarMask::from_raw(&[1, 2, 3, 4], &[], 8, 8),
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

    // ── 旧サイドカー (Vec<LineObject> JSON) backward-compat テスト ──
    //
    // Phase 2b で SidecarMask.vectors の型を `Vec<LineObject>` → `Vec<Shape>` に
    // 変更した。旧版が書いた `vectors: [{"kind":"diag","p0":...,...}]` JSON が
    // 現行の `Vec<Shape>` 型でそのまま読めることを確認する (`Shape::Deserialize`
    // の legacy 経路、mask_db.rs §"Shape 拡張" 参照)。

    #[test]
    fn legacy_sidecar_vectors_json_parses_as_shape() {
        // 旧版 mIV が書いたサイドカーの JSON 文字列を直接構築する。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SIDECAR_FILENAME);
        let legacy_json = r#"{
            "version": 1,
            "items": {
                "img.png": {
                    "mask": {
                        "w": 100,
                        "h": 100,
                        "data": "AAEC",
                        "vectors": [
                            {"kind":"diag","p0":[10.0,20.0],"p1":[90.0,20.0],"thickness":4.0},
                            {"kind":"vert","p0":[50.0,0.0],"p1":[50.0,100.0],"thickness":2.0}
                        ]
                    }
                }
            }
        }"#;
        std::fs::write(&path, legacy_json).unwrap();
        let s = SidecarFile::load(dir.path());
        let entry = s.items().get("img.png").expect("entry present");
        let mask = entry.mask.as_ref().expect("mask present");
        assert_eq!(mask.vectors.len(), 2);
        match mask.vectors[0] {
            crate::mask_db::Shape::Line {
                kind: crate::mask_db::LineKind::Diagonal,
                p0,
                p1,
                thickness,
                ..
            } => {
                assert_eq!(p0, (10.0, 20.0));
                assert_eq!(p1, (90.0, 20.0));
                assert!((thickness - 4.0).abs() < 1e-4);
            }
            other => panic!("expected Line(Diagonal), got {:?}", other),
        }
        assert!(matches!(
            mask.vectors[1],
            crate::mask_db::Shape::Line {
                kind: crate::mask_db::LineKind::Vertical,
                ..
            }
        ));
    }

    #[test]
    fn legacy_sidecar_with_mixed_line_kinds_parses_as_shape() {
        // 縦/横/直線が混在する旧サイドカー JSON を Vec<Shape> として読めることを確認。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SIDECAR_FILENAME);
        let legacy_json = r#"{
            "version": 1,
            "items": {
                "page.jpg": {
                    "mask": {
                        "w": 200,
                        "h": 200,
                        "data": "",
                        "vectors": [
                            {"kind":"horiz","p0":[0.0,100.0],"p1":[200.0,100.0],"thickness":5.0},
                            {"kind":"vert","p0":[100.0,0.0],"p1":[100.0,200.0],"thickness":5.0},
                            {"kind":"diag","p0":[10.0,10.0],"p1":[190.0,190.0],"thickness":8.0}
                        ]
                    }
                }
            }
        }"#;
        std::fs::write(&path, legacy_json).unwrap();
        let s = SidecarFile::load(dir.path());
        let mask = s
            .items()
            .get("page.jpg")
            .and_then(|e| e.mask.as_ref())
            .expect("mask present");
        assert_eq!(mask.vectors.len(), 3);
        assert!(matches!(
            mask.vectors[0],
            crate::mask_db::Shape::Line {
                kind: crate::mask_db::LineKind::Horizontal,
                ..
            }
        ));
        assert!(matches!(
            mask.vectors[1],
            crate::mask_db::Shape::Line {
                kind: crate::mask_db::LineKind::Vertical,
                ..
            }
        ));
        assert!(matches!(
            mask.vectors[2],
            crate::mask_db::Shape::Line {
                kind: crate::mask_db::LineKind::Diagonal,
                ..
            }
        ));
    }

    #[test]
    fn rect_and_ellipse_sidecar_roundtrip() {
        // 新規 Phase 2b: Rect / Ellipse もサイドカーで保存・読込できる
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().to_path_buf();
        let mut s = SidecarFile::new(folder.clone());
        s.set_mask(
            "shape.png",
            SidecarMask {
                w: 100,
                h: 100,
                data: String::new(),
                vectors: vec![
                    crate::mask_db::Shape::Rect {
                        op: crate::mask_db::ShapeOp::Add,
                        center: (50.0, 50.0),
                        half_w: 20.0,
                        half_h: 10.0,
                        rotation_rad: 0.5,
                    },
                    crate::mask_db::Shape::Ellipse {
                        op: crate::mask_db::ShapeOp::Add,
                        center: (30.0, 70.0),
                        rx: 15.0,
                        ry: 8.0,
                        rotation_rad: 0.0,
                    },
                ],
            },
        );
        s.flush();
        // 読み戻し
        let s2 = SidecarFile::load(dir.path());
        let mask = s2
            .items()
            .get("shape.png")
            .and_then(|e| e.mask.as_ref())
            .expect("mask");
        assert_eq!(mask.vectors.len(), 2);
        match mask.vectors[0] {
            crate::mask_db::Shape::Rect {
                center,
                half_w,
                half_h,
                rotation_rad,
                ..
            } => {
                assert_eq!(center, (50.0, 50.0));
                assert!((half_w - 20.0).abs() < 1e-3);
                assert!((half_h - 10.0).abs() < 1e-3);
                assert!((rotation_rad - 0.5).abs() < 1e-3);
            }
            other => panic!("expected Rect, got {:?}", other),
        }
        match mask.vectors[1] {
            crate::mask_db::Shape::Ellipse { rx, ry, .. } => {
                assert!((rx - 15.0).abs() < 1e-3);
                assert!((ry - 8.0).abs() < 1e-3);
            }
            other => panic!("expected Ellipse, got {:?}", other),
        }
    }

    #[test]
    fn empty_vectors_roundtrip_after_migration() {
        // ベクタ 0 件 (= 筆/囲みでビットマップのみ作ったケース) も問題なく読める
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().to_path_buf();
        let mut s = SidecarFile::new(folder.clone());
        s.set_mask(
            "brush_only.png",
            SidecarMask {
                w: 50,
                h: 50,
                data: base64::engine::general_purpose::STANDARD.encode([0xFFu8, 0, 0xFF, 0]),
                vectors: Vec::new(),
            },
        );
        s.flush();

        let s2 = SidecarFile::load(dir.path());
        let mask = s2
            .items()
            .get("brush_only.png")
            .and_then(|e| e.mask.as_ref())
            .expect("mask");
        assert!(mask.vectors.is_empty());
        assert_eq!(mask.w, 50);
    }
}
