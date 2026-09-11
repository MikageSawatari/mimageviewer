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
//! | Tag real files    | `path.parent()`        | `"{filename_lower}"`                          |
//!
//! 相対キー → 絶対 DB キーへの再構成は [`reconstruct_adjust_key`] / [`reconstruct_mask_key`]。
//!
//! ## 動作の原則
//!
//! - 読み込み: `load_folder` 時に 1 度だけ、DB にエントリが無いものだけインポート。
//!   既に DB にあるエントリは無視 (中央が authoritative)。
//! - 書き込み: DB 更新と同じタイミングでメモリ上のサイドカーを更新 (`dirty = true`)。
//!   実ディスク書き込みは **編集ツールを抜けた時 / フォルダ切替 / アプリ終了 /
//!   [`PERIODIC_FLUSH_INTERVAL`]** のいずれかで、専用スレッドが行う。
//! - **書き出しは非同期**。UI スレッドは内容を writer へ渡すだけで、戻った時点では
//!   まだ書けていない。まだ届いていない内容は writer の pending に残り、[`SidecarFile::load`]
//!   がディスクより先にそれを見るので、読み直しても直前の編集は消えない。プロセスが
//!   止まる直前 (終了・トレイ退避) だけ [`wait_for_pending_writes`] で待つ。
//! - エラー処理: IO 失敗は黙ってログ 1 行、以降そのフォルダへは書きに行かない
//!   (読み取り専用メディア対策)。
//! - 設定 OFF 時: 読み書き両方スキップ。既存ファイルは削除しない。

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::adjustment::AdjustParams;
use crate::mask_db::Shape;

/// サイドカーファイル名。`.dat` は Windows 上で「アプリ内部データ」として広く認識される拡張子で、
/// ユーザが誤って編集・削除する心理的ハードルが高い。
pub const SIDECAR_FILENAME: &str = "mimageviewer.dat";

/// 現在のスキーマバージョン。互換性のない変更があったら上げる。
const CURRENT_VERSION: u32 = 1;

/// dirty のまま放置されたサイドカーを書き出す間隔。
///
/// 通常は編集ツールを抜けた時点・フォルダ切替・アプリ終了で書かれるので、この経路が
/// 使われるのは**クラッシュや電源断のとき**だけ。頻度を上げても守れる範囲は「最後の
/// 数分」しか変わらないのに、書き出しの回数だけが増える。
pub const PERIODIC_FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

// ── JSON 形式 ─────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Default)]
struct SidecarJson {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    saved_at: Option<String>,
    /// `SidecarFile::items` と同じ `Arc`。読み書きのどちらでも map を複製しない
    /// (serde の `rc` feature。復元時は共有を持ち越さず新しい `Arc` になる)。
    #[serde(default)]
    items: Arc<BTreeMap<String, SidecarEntry>>,
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
    /// 補正レイヤー配列。中央 `local_adjust.db` が authoritative で、サイドカーは
    /// フォルダ移動時の復元用バックアップとして扱う。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_adjust_layers: Option<local_adjust_core::LocalAdjustmentLayers>,
    /// 最後段 crop 設定。表示・コピー・書き出しの最終段でだけ適用する。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_crop: Option<crate::export_crop::CropSettings>,
    /// テキスト注釈ドキュメント (comic、Inc 2)。中央 `comic.db` が authoritative で、
    /// サイドカーはフォルダ移動時の復元用バックアップ。形式は `Vec<AnnotationObject>`
    /// (serde)。`local_adjust_layers` と同じ二層方式。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comic: Option<Vec<comic_core::AnnotationObject>>,
    /// mIV タグのフォルダ側バックアップ。中央 `tags.db` が authoritative で、
    /// インポートは item 単位の all-or-nothing (既存タグ/決定済み状態があれば skip)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

impl SidecarEntry {
    fn is_empty(&self) -> bool {
        self.adjust.is_none()
            && self.mask.is_none()
            && self.conceal.is_none()
            && self.local_adjust_layers.is_none()
            && self.export_crop.is_none()
            && self.comic.is_none()
            && self.tags.is_none()
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
    /// **writer と共有する不変スナップショット。** flush は `Arc` を 1 つ渡すだけで、
    /// map も、その中の全ページ分のラスターマスク (`Vec<f32>`、原寸なので 24MP なら
    /// 1 面 96 MB) も複製しない。書き換えは `items_mut` の `Arc::make_mut` を通り、
    /// writer がまだ前の snapshot を持っているときだけ 1 回複製する。
    ///
    /// 以前は flush・pending からの読み直し・worker の取り出しの 3 か所がそれぞれ
    /// map 全体を deep clone しており、フォルダ切替や編集終了のたびに数百 MB を
    /// UI スレッドで写していた (2026-08-29 レビュー R-14)。
    items: Arc<BTreeMap<String, SidecarEntry>>,
    dirty: bool,
    /// ディスク上の内容を**上書きしてはいけない**と分かっているフラグ。パース失敗や
    /// 新しいバージョンが書いたファイルがこれに当たる。書き込み自体の失敗 (読み取り
    /// 専用メディア等) は writer 側が folder 単位で覚えるので、ここには入らない。
    disabled: bool,
    /// clean → dirty になった時刻。定期フラッシュはここからの経過で判定するので、
    /// 編集を continuous に続けても「最大 N 分ぶんしか危険に晒さない」保証になる
    /// (最後の変更時刻で測ると、編集し続ける限り一度も書かれない)。
    dirty_since: Option<Instant>,
}

/// A disk snapshot used by the sidecar import path.
///
/// The digest identifies the bytes that were parsed.  The metadata values are
/// captured both before and after the read; a file whose metadata identity
/// changes during the read is not returned as [`SidecarImportLoad::Loaded`].
/// The digest is checked again before commit, including changes that preserve
/// length and mtime. `folder_key` prevents a token from one folder being reused
/// for another folder with coincidentally identical bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SidecarDiskToken {
    folder_key: String,
    byte_len: u64,
    modified_unix_nanos: i64,
    sha256: [u8; 32],
}

impl SidecarDiskToken {
    /// Stable identity stored in the existing INTEGER sync column.  It mixes
    /// byte length, nanosecond timestamp, and the digest so changed bytes do not
    /// look synchronized solely because their timestamps are equal. Old releases
    /// stored whole seconds, so the first new comparison intentionally misses once.
    pub(crate) fn sync_marker(&self) -> i64 {
        let mut hasher = Sha256::new();
        hasher.update(b"mimageviewer-sidecar-sync-v2");
        hasher.update(self.byte_len.to_le_bytes());
        hasher.update(self.modified_unix_nanos.to_le_bytes());
        hasher.update(self.sha256);
        let digest: [u8; 32] = hasher.finalize().into();
        i64::from_le_bytes(digest[..8].try_into().expect("eight-byte marker slice"))
    }

    pub(crate) fn revalidate(&self, folder: &Path) -> Result<(), String> {
        if crate::adjustment_db::normalize_path(folder) != self.folder_key {
            return Err("sidecar token belongs to a different folder".to_string());
        }
        let path = folder.join(SIDECAR_FILENAME);
        let before = std::fs::metadata(&path)
            .map_err(|error| format!("cannot revalidate sidecar metadata: {error}"))
            .and_then(|metadata| disk_metadata_identity(&metadata))?;
        let data = std::fs::read(&path)
            .map_err(|error| format!("cannot revalidate sidecar bytes: {error}"))?;
        let after = std::fs::metadata(&path)
            .map_err(|error| format!("cannot revalidate sidecar metadata: {error}"))
            .and_then(|metadata| disk_metadata_identity(&metadata))?;
        let digest: [u8; 32] = Sha256::digest(&data).into();
        let expected = DiskMetadataIdentity {
            byte_len: self.byte_len,
            modified_unix_nanos: self.modified_unix_nanos,
        };
        if before != after
            || after != expected
            || data.len() as u64 != self.byte_len
            || digest != self.sha256
        {
            return Err("sidecar changed after its import snapshot was loaded".to_string());
        }
        Ok(())
    }
}

/// Identity of the immutable snapshot returned for import.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SidecarImportSource {
    Disk(SidecarDiskToken),
    /// A newer in-process writer snapshot.  Its sequence is meaningful only in
    /// this process and must never be persisted as a disk sync marker.
    PendingWriter {
        folder_key: String,
        sequence: u64,
        kind: PendingWriterKind,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingWriterKind {
    Write,
    Remove,
}

/// One immutable sidecar snapshot bound to the identity it was loaded from.
///
/// Its fields have no public constructor.  Consumers can inspect the sidecar,
/// but only [`crate::sidecar_import::prepare`] can split the snapshot from its
/// source token.  This prevents a token from one read being combined with the
/// bytes parsed by another read and then advancing the wrong sync marker.
pub struct LoadedSidecarImport {
    sidecar: SidecarFile,
    source: SidecarImportSource,
}

impl LoadedSidecarImport {
    pub fn sidecar(&self) -> &SidecarFile {
        &self.sidecar
    }

    pub(crate) fn into_parts(self) -> (SidecarFile, SidecarImportSource) {
        (self.sidecar, self.source)
    }
}

/// Typed load result for the sidecar import engine.
///
/// `SidecarFile::load` deliberately remains the forgiving UI/mirror API.  This
/// result exists so import code cannot confuse an empty valid sidecar with a
/// missing, unreadable, corrupt, future-version, or concurrently changing file
/// and then incorrectly advance a sync marker.
pub enum SidecarImportLoad {
    Missing { sidecar: SidecarFile },
    Loaded(LoadedSidecarImport),
    Unreadable { sidecar: SidecarFile, error: String },
    Corrupt { sidecar: SidecarFile, error: String },
    UnsupportedVersion { sidecar: SidecarFile, version: u32 },
    WriterFailed { sidecar: SidecarFile },
    ChangedDuringRead { sidecar: SidecarFile },
}

impl SidecarImportLoad {
    /// Preserve the historical forgiving load contract for existing callers.
    pub fn into_sidecar(self) -> SidecarFile {
        match self {
            Self::Loaded(loaded) => loaded.sidecar,
            Self::Missing { sidecar }
            | Self::Unreadable { sidecar, .. }
            | Self::Corrupt { sidecar, .. }
            | Self::UnsupportedVersion { sidecar, .. }
            | Self::WriterFailed { sidecar }
            | Self::ChangedDuringRead { sidecar } => sidecar,
        }
    }
}

pub(crate) fn revalidate_import_source(
    sidecar: &SidecarFile,
    source: &SidecarImportSource,
) -> Result<(), String> {
    revalidate_import_source_from(sidecar, source, &writer().state)
}

/// Confirm that the strict import source is still absent.
///
/// Missing has no byte identity to bind like [`SidecarDiskToken`].  Marker
/// clearing therefore uses this fail-closed check immediately before opening a
/// write transaction.  A pending in-process snapshot or a writer failure takes
/// precedence over the disk path exactly as it does in [`SidecarFile::load_for_import`].
pub(crate) fn revalidate_missing_import_source(folder: &Path) -> Result<(), String> {
    revalidate_missing_import_source_from(folder, &writer().state)
}

fn revalidate_missing_import_source_from(
    folder: &Path,
    writer_state: &WriterState,
) -> Result<(), String> {
    if writer_state.pending_import_snapshot(folder)?.is_some() {
        return Err("missing sidecar was superseded by a pending write".to_string());
    }
    if writer_state.is_failed_for_import(folder)? {
        return Err("sidecar writer failed while validating a missing source".to_string());
    }
    match std::fs::metadata(folder.join(SIDECAR_FILENAME)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => return Err("missing sidecar source now exists".to_string()),
        Err(error) => {
            return Err(format!("cannot validate missing sidecar metadata: {error}"));
        }
    }
    // Close the in-process window around the metadata check.  Stage 2 blocks
    // new producers before this call; the second lookup also makes this helper
    // fail closed when used independently in tests or future worker code.
    if writer_state.pending_import_snapshot(folder)?.is_some() {
        return Err("missing sidecar was superseded by a pending write".to_string());
    }
    if writer_state.is_failed_for_import(folder)? {
        return Err("sidecar writer failed while validating a missing source".to_string());
    }
    Ok(())
}

fn revalidate_import_source_from(
    sidecar: &SidecarFile,
    source: &SidecarImportSource,
    writer_state: &WriterState,
) -> Result<(), String> {
    let folder = sidecar.folder();
    let folder_key = crate::adjustment_db::normalize_path(folder);
    match source {
        SidecarImportSource::Disk(token) => {
            if writer_state.pending_import_snapshot(folder)?.is_some() {
                return Err("disk sidecar snapshot was superseded by a pending write".to_string());
            }
            if writer_state.is_failed_for_import(folder)? {
                return Err("sidecar writer failed after the disk snapshot was loaded".to_string());
            }
            token.revalidate(folder)
        }
        SidecarImportSource::PendingWriter {
            folder_key: expected_folder_key,
            sequence,
            kind,
        } => {
            if &folder_key != expected_folder_key {
                return Err("pending sidecar token belongs to a different folder".to_string());
            }
            let Some(current) = writer_state.pending_import_snapshot(folder)? else {
                return Err("pending sidecar snapshot is no longer current".to_string());
            };
            let same_snapshot = match (kind, current.kind) {
                (PendingWriterKind::Write, PendingWriterKind::Write) => {
                    Arc::ptr_eq(&current.items, &sidecar.items)
                }
                (PendingWriterKind::Remove, PendingWriterKind::Remove) => sidecar.items.is_empty(),
                _ => false,
            };
            if current.sequence != *sequence || !same_snapshot {
                return Err("pending sidecar snapshot was superseded".to_string());
            }
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DiskMetadataIdentity {
    byte_len: u64,
    modified_unix_nanos: i64,
}

fn disk_metadata_identity(metadata: &std::fs::Metadata) -> Result<DiskMetadataIdentity, String> {
    let modified = metadata
        .modified()
        .map_err(|error| format!("sidecar modified time is unavailable: {error}"))?;
    let nanos = modified
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("sidecar modified time predates UNIX epoch: {error}"))?
        .as_nanos();
    let modified_unix_nanos = i64::try_from(nanos)
        .map_err(|_| "sidecar modified time does not fit the sync marker".to_string())?;
    Ok(DiskMetadataIdentity {
        byte_len: metadata.len(),
        modified_unix_nanos,
    })
}

/// 実ファイル (フォルダ直下のメディア/コンテナ) のサイドカー相対キー。
/// `App::sidecar_relative_key` の Image 系と `tag_write_worker` のタグバックアップが
/// **同じ導出式を共有する** — 式が割れると同じ `.dat` 内でフィールドごとにキーが
/// 食い違い、インポートで検出不能な不整合になる。
pub(crate) fn real_file_rel_key(path: &Path) -> Option<String> {
    Some(path.file_name()?.to_string_lossy().to_lowercase())
}

impl SidecarFile {
    /// 空のサイドカーを新規作成する (ディスクからは読まない)。
    pub fn new(folder: PathBuf) -> Self {
        Self {
            folder,
            items: Arc::new(BTreeMap::new()),
            dirty: false,
            disabled: false,
            dirty_since: None,
        }
    }

    /// A fail-closed cache value for a source that could not reach a stable
    /// strict snapshot. It prevents a later UI-side forgiving load from doing
    /// the same potentially large read while ensuring no future flush can
    /// overwrite the changing or unreadable file.
    pub(crate) fn disabled_placeholder(folder: PathBuf) -> Self {
        let mut sidecar = Self::new(folder);
        sidecar.disabled = true;
        sidecar
    }

    /// Preserve a strict snapshot for display while preventing later edits from
    /// overwriting a source whose item data failed import validation.
    pub(crate) fn disable_writes_for_session(&mut self) {
        self.disabled = true;
    }

    /// 書き換え用の可変参照。writer がまだ前の snapshot を持っていれば、ここで
    /// **1 回だけ** 複製する (`Arc::make_mut`)。誰も持っていなければ複製しない。
    fn items_mut(&mut self) -> &mut BTreeMap<String, SidecarEntry> {
        Arc::make_mut(&mut self.items)
    }

    /// フォルダから `mimageviewer.dat` を読み込む。無ければ空のサイドカーを返す。
    /// パース失敗時もログ 1 行で空サイドカーを返す (古いバージョンや壊れたファイルで落ちない)。
    ///
    /// 書き出しは非同期なので、**まだディスクに届いていない内容が writer に残っていれば
    /// そちらを返す**。これが無いと、フォルダを離れて戻るだけで直前の編集が消えて見える。
    pub fn load(folder: &Path) -> Self {
        Self::load_from(folder, &writer().state)
    }

    /// Load a sidecar for central-DB import without collapsing terminal states.
    ///
    /// Pending writer data wins over disk, exactly as it does for [`Self::load`].
    /// A pending snapshot has no durable disk identity, so callers may import it
    /// but must leave the disk sync marker unchanged.
    pub fn load_for_import(folder: &Path) -> SidecarImportLoad {
        Self::load_for_import_from(folder, &writer().state)
    }

    fn load_for_import_from(folder: &Path, writer: &WriterState) -> SidecarImportLoad {
        let folder_key = crate::adjustment_db::normalize_path(folder);
        let pending = match writer.pending_import_snapshot(folder) {
            Ok(pending) => pending,
            Err(error) => {
                let mut sidecar = Self::new(folder.to_path_buf());
                sidecar.disabled = true;
                return SidecarImportLoad::Unreadable { sidecar, error };
            }
        };
        if let Some(pending) = pending {
            let mut sidecar = Self::new(folder.to_path_buf());
            sidecar.items = pending.items;
            return SidecarImportLoad::Loaded(LoadedSidecarImport {
                sidecar,
                source: SidecarImportSource::PendingWriter {
                    folder_key,
                    sequence: pending.sequence,
                    kind: pending.kind,
                },
            });
        }
        match writer.is_failed_for_import(folder) {
            Ok(false) => {}
            Ok(true) => {
                let mut sidecar = Self::new(folder.to_path_buf());
                sidecar.disabled = true;
                return SidecarImportLoad::WriterFailed { sidecar };
            }
            Err(error) => {
                let mut sidecar = Self::new(folder.to_path_buf());
                sidecar.disabled = true;
                return SidecarImportLoad::Unreadable { sidecar, error };
            }
        }

        let path = folder.join(SIDECAR_FILENAME);
        let before = match std::fs::metadata(&path) {
            Ok(metadata) => match disk_metadata_identity(&metadata) {
                Ok(identity) => identity,
                Err(error) => {
                    let mut sidecar = Self::new(folder.to_path_buf());
                    sidecar.disabled = true;
                    return SidecarImportLoad::Unreadable { sidecar, error };
                }
            },
            Err(ref error) if error.kind() == std::io::ErrorKind::NotFound => {
                return SidecarImportLoad::Missing {
                    sidecar: Self::new(folder.to_path_buf()),
                };
            }
            Err(error) => {
                let mut sidecar = Self::new(folder.to_path_buf());
                sidecar.disabled = true;
                return SidecarImportLoad::Unreadable {
                    sidecar,
                    error: error.to_string(),
                };
            }
        };

        let data = match std::fs::read(&path) {
            Ok(data) => data,
            Err(error) => {
                let mut sidecar = Self::new(folder.to_path_buf());
                sidecar.disabled = true;
                return SidecarImportLoad::Unreadable {
                    sidecar,
                    error: error.to_string(),
                };
            }
        };
        let after = match std::fs::metadata(&path)
            .map_err(|error| error.to_string())
            .and_then(|metadata| disk_metadata_identity(&metadata))
        {
            Ok(identity) => identity,
            Err(error) => {
                let mut sidecar = Self::new(folder.to_path_buf());
                sidecar.disabled = true;
                return SidecarImportLoad::Unreadable { sidecar, error };
            }
        };
        if before != after || after.byte_len != data.len() as u64 {
            let mut sidecar = Self::new(folder.to_path_buf());
            sidecar.disabled = true;
            return SidecarImportLoad::ChangedDuringRead { sidecar };
        }

        let digest: [u8; 32] = Sha256::digest(&data).into();
        let legacy_before = local_adjust_core::mask_codec::legacy_decode_count();
        let _mask_budget = local_adjust_core::mask_codec::DocumentBudget::open();
        let parsed: SidecarJson = match serde_json::from_slice(&data) {
            Ok(parsed) => parsed,
            Err(error) => {
                let mut sidecar = Self::new(folder.to_path_buf());
                sidecar.disabled = true;
                return SidecarImportLoad::Corrupt {
                    sidecar,
                    error: error.to_string(),
                };
            }
        };
        if parsed.version > CURRENT_VERSION {
            let mut sidecar = Self::new(folder.to_path_buf());
            sidecar.disabled = true;
            return SidecarImportLoad::UnsupportedVersion {
                sidecar,
                version: parsed.version,
            };
        }

        let mut sidecar = Self::new(folder.to_path_buf());
        sidecar.items = parsed.items;
        if local_adjust_core::mask_codec::legacy_decode_count() != legacy_before {
            sidecar.mark_dirty();
        }
        SidecarImportLoad::Loaded(LoadedSidecarImport {
            sidecar,
            source: SidecarImportSource::Disk(SidecarDiskToken {
                folder_key,
                byte_len: before.byte_len,
                modified_unix_nanos: before.modified_unix_nanos,
                sha256: digest,
            }),
        })
    }

    /// [`SidecarFile::load`] の実体。writer を明示的に渡すのはテストのためだけで、
    /// 本番の入口は 1 つ (`load`)。
    fn load_from(folder: &Path, writer: &WriterState) -> Self {
        let mut me = Self::new(folder.to_path_buf());
        if let Some(items) = writer.pending_items(folder) {
            me.items = items;
            return me;
        }
        let path = folder.join(SIDECAR_FILENAME);
        let data = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => return me,
            Err(e) => {
                crate::logger::log(format!("sidecar: read failed: {} ({})", path.display(), e));
                return me;
            }
        };
        let legacy_before = local_adjust_core::mask_codec::legacy_decode_count();
        // One sidecar covers every item in a folder, and each layer can hold four mask
        // buffers. Bound what the whole file may decode, not just its largest field.
        let _mask_budget = local_adjust_core::mask_codec::DocumentBudget::open();
        let parsed: SidecarJson = match serde_json::from_str(&data) {
            Ok(v) => v,
            Err(e) => {
                crate::logger::log(format!(
                    "sidecar: JSON parse failed: {} ({})",
                    path.display(),
                    e
                ));
                // parse 失敗 = 破損ファイル。空のまま返すと次の編集で flush() が単一エントリで
                // 上書きし、手動回復し得たデータを消す。newer-version 経路と同様に disabled に
                // して、このセッションでは上書きさせない (v1.0.0 データ整合性レビュー DI-7)。
                me.disabled = true;
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
        // v1.1.0〜v3.2.0 は補正レイヤーのマスクを 1 画素 1 数値の JSON 配列で書いていた。
        // 実測で 1 ページ 290MB になり、フォルダ切替のたびに UI を数秒止めていた。読めた
        // ものはメモリ上では新形式なので、dirty にしておけば次の書き出しで置き換わる。
        if local_adjust_core::mask_codec::legacy_decode_count() != legacy_before {
            crate::logger::log(format!(
                "sidecar: rewriting legacy mask arrays ({} bytes): {}",
                data.len(),
                path.display()
            ));
            me.mark_dirty();
        }
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

    /// clean → dirty になった時刻。定期フラッシュの判定に使う。
    pub fn dirty_since(&self) -> Option<Instant> {
        self.dirty_since
    }

    /// 画像編集の 6 系統だけを page bundle として全置換する。タグは編集 bundle の
    /// 対象外なので、同じ entry に保存済みの値を必ず引き継ぐ。
    pub fn replace_edit_bundle(&mut self, rel_key: &str, mut replacement: SidecarEntry) {
        replacement.tags = self.items.get(rel_key).and_then(|entry| entry.tags.clone());
        if replacement.is_empty() {
            self.items_mut().remove(rel_key);
        } else {
            self.items_mut().insert(rel_key.to_string(), replacement);
        }
        self.mark_dirty();
    }

    // ── 変更 ──────────────────────────────────────────────────────

    pub fn set_adjust(&mut self, rel_key: &str, params: AdjustParams) {
        let entry = self.items_mut().entry(rel_key.to_string()).or_default();
        entry.adjust = Some(params);
        self.mark_dirty();
    }

    pub fn remove_adjust(&mut self, rel_key: &str) {
        if let Some(entry) = self.items_mut().get_mut(rel_key) {
            if entry.adjust.is_some() {
                entry.adjust = None;
                if entry.is_empty() {
                    self.items_mut().remove(rel_key);
                }
                self.mark_dirty();
            }
        }
    }

    pub fn set_mask(&mut self, rel_key: &str, mask: SidecarMask) {
        let entry = self.items_mut().entry(rel_key.to_string()).or_default();
        entry.mask = Some(mask);
        self.mark_dirty();
    }

    pub fn remove_mask(&mut self, rel_key: &str) {
        if let Some(entry) = self.items_mut().get_mut(rel_key) {
            if entry.mask.is_some() {
                entry.mask = None;
                if entry.is_empty() {
                    self.items_mut().remove(rel_key);
                }
                self.mark_dirty();
            }
        }
    }

    /// 隠蔽加工マスクをセットする (Phase 4)。形式は `SidecarMask` と共通。
    pub fn set_conceal(&mut self, rel_key: &str, conceal: SidecarMask) {
        let entry = self.items_mut().entry(rel_key.to_string()).or_default();
        entry.conceal = Some(conceal);
        self.mark_dirty();
    }

    /// 隠蔽加工マスクを取り除く (Phase 4)。
    pub fn remove_conceal(&mut self, rel_key: &str) {
        if let Some(entry) = self.items_mut().get_mut(rel_key) {
            if entry.conceal.is_some() {
                entry.conceal = None;
                if entry.is_empty() {
                    self.items_mut().remove(rel_key);
                }
                self.mark_dirty();
            }
        }
    }

    /// 補正レイヤー配列をセットする。空配列は削除と同じ扱いにする。
    pub fn set_local_adjust_layers(
        &mut self,
        rel_key: &str,
        layers: local_adjust_core::LocalAdjustmentLayers,
    ) {
        if layers.is_empty() {
            self.remove_local_adjust_layers(rel_key);
            return;
        }
        let entry = self.items_mut().entry(rel_key.to_string()).or_default();
        entry.local_adjust_layers = Some(layers);
        self.mark_dirty();
    }

    /// 補正レイヤー配列を取り除く。
    pub fn remove_local_adjust_layers(&mut self, rel_key: &str) {
        if let Some(entry) = self.items_mut().get_mut(rel_key) {
            if entry.local_adjust_layers.is_some() {
                entry.local_adjust_layers = None;
                if entry.is_empty() {
                    self.items_mut().remove(rel_key);
                }
                self.mark_dirty();
            }
        }
    }

    /// テキスト注釈ドキュメントをセットする。空配列は削除と同じ扱い。
    pub fn set_comic(&mut self, rel_key: &str, objects: Vec<comic_core::AnnotationObject>) {
        if objects.is_empty() {
            self.remove_comic(rel_key);
            return;
        }
        let entry = self.items_mut().entry(rel_key.to_string()).or_default();
        entry.comic = Some(objects);
        self.mark_dirty();
    }

    /// テキスト注釈ドキュメントを取り除く。
    pub fn remove_comic(&mut self, rel_key: &str) {
        if let Some(entry) = self.items_mut().get_mut(rel_key) {
            if entry.comic.is_some() {
                entry.comic = None;
                if entry.is_empty() {
                    self.items_mut().remove(rel_key);
                }
                self.mark_dirty();
            }
        }
    }

    /// mIV タグをセットする。空/不正タグだけなら削除と同じ扱いにする。
    pub fn set_tags<I, S>(&mut self, rel_key: &str, tags: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let normalized = crate::tags_db::collapse_tags(tags, 0)
            .into_iter()
            .map(|tag| crate::tags_db::format_display_tag(&tag.tag))
            .collect::<Vec<_>>();
        if normalized.is_empty() {
            self.remove_tags(rel_key);
            return;
        }
        let entry = self.items_mut().entry(rel_key.to_string()).or_default();
        entry.tags = Some(normalized);
        self.mark_dirty();
    }

    /// mIV タグのバックアップを取り除く。
    pub fn remove_tags(&mut self, rel_key: &str) {
        if let Some(entry) = self.items_mut().get_mut(rel_key) {
            if entry.tags.is_some() {
                entry.tags = None;
                if entry.is_empty() {
                    self.items_mut().remove(rel_key);
                }
                self.mark_dirty();
            }
        }
    }

    /// 最後段 crop 設定をセットする。
    pub fn set_export_crop(&mut self, rel_key: &str, settings: crate::export_crop::CropSettings) {
        let entry = self.items_mut().entry(rel_key.to_string()).or_default();
        entry.export_crop = Some(settings);
        self.mark_dirty();
    }

    /// 最後段 crop 設定を取り除く。
    pub fn remove_export_crop(&mut self, rel_key: &str) {
        if let Some(entry) = self.items_mut().get_mut(rel_key) {
            if entry.export_crop.is_some() {
                entry.export_crop = None;
                if entry.is_empty() {
                    self.items_mut().remove(rel_key);
                }
                self.mark_dirty();
            }
        }
    }

    /// mIV 内削除成功時、実ファイル / コンテナ名に対応する全バックアップ項目を落とす。
    /// 通常画像は exact、ZIP/PDF は `<file>::...` 配下も対象。隣接名は巻き込まない。
    pub fn purge_deleted_root(&mut self, rel_root: &str) -> bool {
        let container_prefix = format!("{rel_root}::");
        let folder_prefix = format!("{rel_root}/");
        let before = self.items.len();
        self.items_mut().retain(|key, _| {
            key != rel_root
                && !key.starts_with(&container_prefix)
                && !key.starts_with(&folder_prefix)
        });
        if self.items.len() != before {
            self.mark_dirty();
            true
        } else {
            false
        }
    }

    /// 複数エントリの adjust を一括セット (「全画像に適用」用)。
    pub fn set_adjust_bulk<I>(&mut self, iter: I, params: &AdjustParams)
    where
        I: IntoIterator<Item = String>,
    {
        let mut changed = false;
        for rel_key in iter {
            let entry = self.items_mut().entry(rel_key).or_default();
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
            if let Some(entry) = self.items_mut().get_mut(rel_key) {
                if entry.adjust.is_some() {
                    entry.adjust = None;
                    changed = true;
                }
            }
        }
        if changed {
            self.items_mut().retain(|_, e| !e.is_empty());
            self.mark_dirty();
        }
    }

    fn mark_dirty(&mut self) {
        if !self.dirty {
            self.dirty_since = Some(Instant::now());
        }
        self.dirty = true;
    }

    // ── 書き込み ──────────────────────────────────────────────────

    /// dirty ならディスクへの書き出しを writer スレッドに積む。dirty でなければ何もしない。
    ///
    /// **戻ってきた時点ではまだ書けていない。**積んだ内容は writer の pending にも入り、
    /// 同じフォルダを読み直すとディスクより先にそちらが見えるので、書き終わるのを待たずに
    /// メモリから降ろしてよい。実際に書けたことを確かめたい呼び出し側は
    /// [`SidecarFile::flush_blocking`] を使う。
    pub fn queue_flush(&mut self) {
        if !self.dirty || self.disabled {
            return;
        }
        if writer().is_failed(&self.folder) {
            // 読み取り専用メディア等。既に一度失敗しているので積まない。
            self.dirty = false;
            self.dirty_since = None;
            return;
        }
        writer().enqueue(self.folder.clone(), self.write_request());
        self.dirty = false;
        self.dirty_since = None;
    }

    /// flush で積む内容。`items` の `Arc` をそのまま渡すので map は複製しない
    /// (`items` の doc comment 参照)。空なら削除要求。
    fn write_request(&self) -> WriteRequest {
        if self.items.is_empty() {
            WriteRequest::Remove
        } else {
            WriteRequest::Write(Arc::clone(&self.items))
        }
    }

    /// この内容がディスクに載っているか。積んだだけで**まだ待っていない**間は false
    /// なので、[`wait_for_pending_writes`] のあとに読むこと。
    pub fn written_to_disk(&self) -> bool {
        !self.dirty && !writer().is_failed(&self.folder)
    }

    #[cfg(test)]
    pub(crate) fn set_dirty_since_for_test(&mut self, since: std::time::Instant) {
        assert!(
            self.dirty,
            "only a dirty test owner may have its age overridden"
        );
        self.dirty_since = Some(since);
    }

    /// 書き出しを積み、writer が捌き終わるまで待って結果を返す。
    /// 削除移行のように「書けたか」を報告する必要がある稀な経路だけが使う。
    /// UI スレッドからは呼ばない。
    pub fn flush_blocking(&mut self) -> bool {
        self.queue_flush();
        writer().wait_until_idle();
        self.written_to_disk()
    }
}

// ── 書き出し worker ───────────────────────────────────────────────────

/// 1 フォルダぶんの書き出し要求。
#[derive(Clone)]
enum WriteRequest {
    /// 積んだ時点の内容そのもの。`SidecarFile::items` と同じ `Arc` を共有するので、
    /// 積む側も worker も map を複製しない。
    Write(Arc<BTreeMap<String, SidecarEntry>>),
    Remove,
}

/// UI-owned dirty sidecars retained while a worker flushes immutable snapshots.
///
/// Stage 2 keeps this owner outside the worker closure.  If thread spawn, queue,
/// write, or idle-fence validation fails, [`SidecarFlushOwners::resolve`] returns
/// the original dirty values so the App can put them back in its cache without
/// reconstructing them from disk.
pub(crate) struct SidecarFlushOwners {
    identity: Arc<SidecarFlushIdentity>,
    sidecars: Vec<SidecarFile>,
    expected_flushes: usize,
}

/// Immutable snapshots reserved from cache owners that remain available to the UI.
///
/// Holding each items `Arc` makes a later mutation use copy-on-write. Resolution can
/// therefore clear or evict only the exact owner that was written, while a sibling
/// context may keep using the cache without falling back to a synchronous disk load.
pub(crate) struct SidecarFlushReservations {
    identity: Arc<SidecarFlushIdentity>,
    owners: Vec<SidecarFlushReservation>,
    expected_flushes: usize,
}

struct SidecarFlushReservation {
    folder: PathBuf,
    items: Arc<BTreeMap<String, SidecarEntry>>,
    disabled: bool,
    requested: bool,
}

impl SidecarFlushReservations {
    /// Reconcile a worker result with cache owners that stayed in place.
    ///
    /// Errors leave every cache owner untouched. A successful request clears dirty
    /// state only when the folder, item `Arc`, and writable state still match the
    /// captured owner. Main-view cache eviction uses the same exact identity, so a
    /// clean cache inserted or replaced while the worker ran is preserved.
    pub(crate) fn resolve_in_place(
        self,
        sidecars: &mut HashMap<PathBuf, SidecarFile>,
        result: Result<SidecarFlushReport, String>,
        evict_verified_clean: bool,
    ) -> Result<SidecarFlushReport, String> {
        let report = validate_flush_result(&self.identity, self.expected_flushes, result)?;
        let mut evict = Vec::new();
        for reservation in self.owners {
            let Some(current) = sidecars.get_mut(&reservation.folder) else {
                continue;
            };
            let exact = Arc::ptr_eq(&current.items, &reservation.items)
                && current.disabled == reservation.disabled;
            if !exact {
                continue;
            }
            if reservation.requested && !current.disabled {
                current.dirty = false;
                current.dirty_since = None;
            }
            if evict_verified_clean && !current.dirty {
                evict.push(reservation.folder);
            }
        }
        for folder in evict {
            sidecars.remove(&folder);
        }
        Ok(report)
    }
}

impl SidecarFlushOwners {
    pub(crate) fn is_empty(&self) -> bool {
        self.sidecars.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.sidecars.len()
    }

    /// Rejoin the UI-owned values with the worker result.
    ///
    /// Only a verified success clears `dirty`; every error returns the exact
    /// original values unchanged so the App can restore their ownership.
    pub(crate) fn resolve(
        mut self,
        result: Result<SidecarFlushReport, String>,
    ) -> SidecarFlushCompletion {
        match validate_flush_result(&self.identity, self.expected_flushes, result) {
            Ok(report) => {
                for sidecar in &mut self.sidecars {
                    if !sidecar.disabled {
                        sidecar.dirty = false;
                        sidecar.dirty_since = None;
                    }
                }
                SidecarFlushCompletion::Flushed {
                    sidecars: self.sidecars,
                    report,
                }
            }
            Err(error) => SidecarFlushCompletion::Failed {
                sidecars: self.sidecars,
                error,
            },
        }
    }
}

fn validate_flush_result(
    identity: &Arc<SidecarFlushIdentity>,
    expected_flushes: usize,
    result: Result<SidecarFlushReport, String>,
) -> Result<SidecarFlushReport, String> {
    let report = result?;
    if !Arc::ptr_eq(identity, &report.identity) {
        return Err("sidecar flush result belongs to a different owner batch".to_string());
    }
    if report.flushed_folders != expected_flushes {
        return Err(format!(
            "sidecar flush reported {} folders for {} expected flushes",
            report.flushed_folders, expected_flushes
        ));
    }
    Ok(report)
}

#[derive(Clone)]
struct SidecarFlushRequest {
    folder: PathBuf,
    request: WriteRequest,
}

#[derive(Debug)]
struct SidecarFlushIdentity;

/// Immutable flush work that may be moved to a worker while
/// [`SidecarFlushOwners`] remains with the state owner.
pub(crate) struct SidecarFlushBatch {
    identity: Arc<SidecarFlushIdentity>,
    requests: Vec<SidecarFlushRequest>,
}

#[derive(Clone, Debug)]
pub(crate) struct SidecarFlushReport {
    identity: Arc<SidecarFlushIdentity>,
    pub(crate) flushed_folders: usize,
    pub(crate) elapsed: std::time::Duration,
}

pub(crate) enum SidecarFlushCompletion {
    Flushed {
        sidecars: Vec<SidecarFile>,
        report: SidecarFlushReport,
    },
    Failed {
        sidecars: Vec<SidecarFile>,
        error: String,
    },
}

/// Split sidecars into retained owners and an immutable dirty worker batch.
///
/// This function only moves `SidecarFile` values and clones their item `Arc`s;
/// it does no serialization, filesystem access, channel wait, or SQLite work.
/// Clean and write-disabled sidecars remain in the retained owner set, so a
/// caller may safely move its complete cache into this boundary without
/// silently dropping entries. A dirty disabled owner is deliberately omitted
/// from the batch and remains dirty after successful resolution: it represents
/// an edit that cannot safely replace the preserved source, not pending I/O
/// that should block recovery for an unrelated folder.
pub(crate) fn prepare_worker_flush(
    sidecars: impl IntoIterator<Item = SidecarFile>,
) -> (SidecarFlushOwners, SidecarFlushBatch) {
    let identity = Arc::new(SidecarFlushIdentity);
    let mut owners = Vec::new();
    let mut requests = Vec::new();
    for sidecar in sidecars {
        if sidecar.dirty && !sidecar.disabled {
            requests.push(SidecarFlushRequest {
                folder: sidecar.folder.clone(),
                request: sidecar.write_request(),
            });
        }
        owners.push(sidecar);
    }
    let expected_flushes = requests.len();
    (
        SidecarFlushOwners {
            identity: Arc::clone(&identity),
            sidecars: owners,
            expected_flushes,
        },
        SidecarFlushBatch { identity, requests },
    )
}

/// Capture an immutable worker batch without removing cache owners from the App.
///
/// The captured item `Arc`s are reservations: any edit after this call receives a
/// distinct `Arc` through `Arc::make_mut`, and resolution will leave that newer dirty
/// owner untouched. The batch still excludes dirty write-disabled owners and still
/// requires the process-global idle fence even when it is empty.
pub(crate) fn prepare_worker_flush_in_place<'a>(
    sidecars: impl IntoIterator<Item = &'a SidecarFile>,
) -> (SidecarFlushReservations, SidecarFlushBatch) {
    let identity = Arc::new(SidecarFlushIdentity);
    let mut owners = Vec::new();
    let mut requests = Vec::new();
    for sidecar in sidecars {
        let requested = sidecar.dirty && !sidecar.disabled;
        if requested {
            requests.push(SidecarFlushRequest {
                folder: sidecar.folder.clone(),
                request: sidecar.write_request(),
            });
        }
        owners.push(SidecarFlushReservation {
            folder: sidecar.folder.clone(),
            items: Arc::clone(&sidecar.items),
            disabled: sidecar.disabled,
            requested,
        });
    }
    let expected_flushes = requests.len();
    (
        SidecarFlushReservations {
            identity: Arc::clone(&identity),
            owners,
            expected_flushes,
        },
        SidecarFlushBatch { identity, requests },
    )
}

impl SidecarFlushBatch {
    /// Whether this batch has no new writable dirty snapshots to queue.
    ///
    /// This says nothing about the process-global writer: an earlier ordinary
    /// `queue_flush` may still be in flight after its cache owner became clean.
    /// The Stage 2 worker must call `run_on_worker` even for an empty batch so
    /// the strict global idle fence is not skipped.
    pub(crate) fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }

    /// Queue this batch and wait for the process-global sidecar writer.
    ///
    /// This is intentionally blocking and must only run on a worker.  Unlike
    /// changing the ordinary `queue_flush` contract, it preserves the existing
    /// coalescing queue and its synchronous channel-failure fallback while
    /// moving every potentially blocking operation off the UI thread.
    pub(crate) fn run_on_worker(
        self,
        timeout: std::time::Duration,
    ) -> Result<SidecarFlushReport, String> {
        self.run_with_writer(writer(), timeout)
    }

    fn run_with_writer(
        self,
        writer: &SidecarWriter,
        timeout: std::time::Duration,
    ) -> Result<SidecarFlushReport, String> {
        let started = Instant::now();
        for request in &self.requests {
            if writer.state.is_failed_for_import(&request.folder)? {
                return Err(format!(
                    "sidecar writer previously failed for {}",
                    request.folder.display()
                ));
            }
        }
        for request in &self.requests {
            if let Err(error) =
                writer.enqueue_for_worker_flush(request.folder.clone(), request.request.clone())
            {
                // Earlier requests may already be accepted.  Drain them when
                // possible before returning the retained owners to the App.
                let _ = writer.wait_until_idle_strict(timeout);
                return Err(error);
            }
        }
        writer.wait_until_idle_strict(timeout)?;
        for request in &self.requests {
            if writer
                .state
                .pending_import_snapshot(&request.folder)?
                .is_some()
            {
                return Err(format!(
                    "sidecar writer left pending data for {} after its idle fence",
                    request.folder.display()
                ));
            }
            if writer.state.is_failed_for_import(&request.folder)? {
                return Err(format!(
                    "sidecar writer failed for {}",
                    request.folder.display()
                ));
            }
        }
        Ok(SidecarFlushReport {
            identity: self.identity,
            flushed_folders: self.requests.len(),
            elapsed: started.elapsed(),
        })
    }
}

struct QueuedWrite {
    /// 積んだ順番。worker は書いた後、これが変わっていなければ pending から外す。
    seq: u64,
    request: WriteRequest,
}

struct PendingImportSnapshot {
    sequence: u64,
    items: Arc<BTreeMap<String, SidecarEntry>>,
    kind: PendingWriterKind,
}

/// サイドカーの書き出しを 1 本のスレッドへ集約する。
///
/// **プロセス内で 1 つだけ**存在する。読み手が pending を必ず見られることが
/// read-after-write の成立条件なので、context ごとに持たせて分裂させない。
pub struct SidecarWriter {
    tx: std::sync::mpsc::Sender<PathBuf>,
    state: std::sync::Arc<WriterState>,
}

#[derive(Default)]
struct WriterState {
    /// まだディスクに反映していない内容。読み手はディスクより先にここを見る。
    pending: std::sync::Mutex<std::collections::HashMap<PathBuf, QueuedWrite>>,
    /// 書き込みに失敗したフォルダ。以降そこへは書きに行かない (読み取り専用メディア対策)。
    failed: std::sync::Mutex<std::collections::HashSet<PathBuf>>,
    /// 未処理 + 実行中の件数と、0 になったことを知らせる condvar。
    inflight: std::sync::Mutex<usize>,
    idle: std::sync::Condvar,
    next_seq: std::sync::atomic::AtomicU64,
}

static WRITER: std::sync::OnceLock<SidecarWriter> = std::sync::OnceLock::new();

fn writer() -> &'static SidecarWriter {
    WRITER.get_or_init(SidecarWriter::spawn)
}

/// 積んである書き出しがすべてディスクに反映されるまで待つ。
/// アプリ終了・トレイ退避の直前に呼ぶ (ここを省くと最後の編集が落ちる)。
pub fn wait_for_pending_writes() {
    writer().wait_until_idle();
}

/// Strict, bounded idle fence for worker-owned recovery orchestration.
///
/// The historical exit path above deliberately keeps its forgiving contract.
/// Sidecar import must instead distinguish timeout and poisoned state from a
/// successful drain so it never reads stale disk bytes or advances a marker.
pub(crate) fn wait_for_pending_writes_strict(timeout: std::time::Duration) -> Result<(), String> {
    writer().wait_until_idle_strict(timeout)
}

impl SidecarWriter {
    fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<PathBuf>();
        let state = std::sync::Arc::new(WriterState::default());
        let worker_state = state.clone();
        // 失敗しても書き出しが同期に戻るだけなので、spawn 失敗はログして続行する。
        if let Err(error) = std::thread::Builder::new()
            .name("sidecar-writer".into())
            .spawn(move || {
                while let Ok(folder) = rx.recv() {
                    worker_state.write_one(&folder);
                    worker_state.finish_one();
                }
            })
        {
            crate::logger::log(format!("sidecar: writer thread spawn failed ({error})"));
        }
        Self { tx, state }
    }

    fn enqueue(&self, folder: PathBuf, request: WriteRequest) {
        self.state.queue(folder.clone(), request);
        if self.tx.send(folder.clone()).is_err() {
            // worker が居ない (spawn 失敗)。積んだままにすると読み手が永久に
            // pending を見続けるので、この場で同期に書いて整合させる。
            self.state.write_one(&folder);
            self.state.finish_one();
        }
    }

    /// Strict queue admission used only by a worker-owned restore flush.
    ///
    /// The normal enqueue path remains unchanged.  This variant fails before
    /// publishing anything if either queue mutex is poisoned.  A disconnected
    /// writer still uses the established synchronous fallback, but the caller
    /// is the recovery worker rather than the UI thread.
    fn enqueue_for_worker_flush(
        &self,
        folder: PathBuf,
        request: WriteRequest,
    ) -> Result<(), String> {
        self.state.queue_strict(folder.clone(), request)?;
        if self.tx.send(folder.clone()).is_err() {
            self.state.write_one(&folder);
            self.state.finish_one();
        }
        Ok(())
    }

    fn is_failed(&self, folder: &Path) -> bool {
        self.state.is_failed(folder)
    }

    fn wait_until_idle(&self) {
        let Ok(mut inflight) = self.state.inflight.lock() else {
            return;
        };
        while *inflight > 0 {
            let Ok(next) = self.state.idle.wait(inflight) else {
                return;
            };
            inflight = next;
        }
    }

    fn wait_until_idle_strict(&self, timeout: std::time::Duration) -> Result<(), String> {
        let started = Instant::now();
        let mut inflight = self
            .state
            .inflight
            .lock()
            .map_err(|_| "sidecar writer inflight state is unavailable".to_string())?;
        while *inflight > 0 {
            let remaining = timeout.checked_sub(started.elapsed()).ok_or_else(|| {
                format!(
                    "sidecar writer did not become idle within {} ms",
                    timeout.as_millis()
                )
            })?;
            let (next, wait) = self
                .state
                .idle
                .wait_timeout(inflight, remaining)
                .map_err(|_| "sidecar writer inflight state is unavailable".to_string())?;
            inflight = next;
            if wait.timed_out() && *inflight > 0 {
                return Err(format!(
                    "sidecar writer did not become idle within {} ms",
                    timeout.as_millis()
                ));
            }
        }
        Ok(())
    }
}

impl WriterState {
    /// 書き出し内容を pending へ載せ、未処理件数を 1 増やす。
    /// 実際に書くのは worker (または送信失敗時の呼び出し元)。
    fn queue(&self, folder: PathBuf, request: WriteRequest) {
        let seq = self
            .next_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(folder, QueuedWrite { seq, request });
        }
        if let Ok(mut inflight) = self.inflight.lock() {
            *inflight += 1;
        }
    }

    /// Queue one worker-owned flush without the forgiving poison fallbacks used
    /// by the historical UI enqueue path.  Both mutexes are acquired before
    /// either map or counter is changed, so a poison error cannot leave a
    /// half-published request.
    fn queue_strict(&self, folder: PathBuf, request: WriteRequest) -> Result<(), String> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| "sidecar writer pending state is unavailable".to_string())?;
        let mut inflight = self
            .inflight
            .lock()
            .map_err(|_| "sidecar writer inflight state is unavailable".to_string())?;
        let next_inflight = inflight
            .checked_add(1)
            .ok_or_else(|| "sidecar writer inflight count overflowed".to_string())?;
        let seq = self
            .next_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        pending.insert(folder, QueuedWrite { seq, request });
        *inflight = next_inflight;
        Ok(())
    }

    fn pending_items(&self, folder: &Path) -> Option<Arc<BTreeMap<String, SidecarEntry>>> {
        let pending = self.pending.lock().ok()?;
        match &pending.get(folder)?.request {
            WriteRequest::Write(items) => Some(Arc::clone(items)),
            WriteRequest::Remove => Some(Arc::new(BTreeMap::new())),
        }
    }

    /// Strict snapshot lookup for the import engine.  Unlike the forgiving UI
    /// lookup, lock poisoning is a terminal error instead of being collapsed to
    /// "nothing pending" and falling through to possibly stale disk bytes.
    fn pending_import_snapshot(
        &self,
        folder: &Path,
    ) -> Result<Option<PendingImportSnapshot>, String> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| "sidecar writer pending state is unavailable".to_string())?;
        let Some(queued) = pending.get(folder) else {
            return Ok(None);
        };
        let (items, kind) = match &queued.request {
            WriteRequest::Write(items) => (Arc::clone(items), PendingWriterKind::Write),
            WriteRequest::Remove => (Arc::new(BTreeMap::new()), PendingWriterKind::Remove),
        };
        Ok(Some(PendingImportSnapshot {
            sequence: queued.seq,
            items,
            kind,
        }))
    }

    fn is_failed(&self, folder: &Path) -> bool {
        self.failed
            .lock()
            .map(|failed| failed.contains(folder))
            .unwrap_or(false)
    }

    fn is_failed_for_import(&self, folder: &Path) -> Result<bool, String> {
        self.failed
            .lock()
            .map(|failed| failed.contains(folder))
            .map_err(|_| "sidecar writer failure state is unavailable".to_string())
    }

    fn write_one(&self, folder: &Path) {
        let Some((seq, request)) = self.take_for_write(folder) else {
            return;
        };
        let ok = write_sidecar_to_disk(folder, &request);
        self.finish_write(folder, seq, ok);
    }

    /// これから書く内容。同じフォルダに複数の job が積まれていても、常に**最新**を
    /// 書く。追い越された job はここで None になって空振りする (coalescing)。
    fn take_for_write(&self, folder: &Path) -> Option<(u64, WriteRequest)> {
        let pending = self.pending.lock().ok()?;
        let queued = pending.get(folder)?;
        Some((queued.seq, queued.request.clone()))
    }

    /// 書き終えた後始末。
    ///
    /// **書いている最中に積み直されていたら pending を消さない。**消すと、次の job が
    /// 走るまでの間だけ読み手がディスク (= 1 つ前の内容) を見てしまう。
    fn finish_write(&self, folder: &Path, seq: u64, ok: bool) {
        if !ok && let Ok(mut failed) = self.failed.lock() {
            failed.insert(folder.to_path_buf());
        }
        if let Ok(mut pending) = self.pending.lock()
            && pending.get(folder).is_some_and(|queued| queued.seq == seq)
        {
            pending.remove(folder);
        }
    }

    fn finish_one(&self) {
        if let Ok(mut inflight) = self.inflight.lock() {
            *inflight = inflight.saturating_sub(1);
            if *inflight == 0 {
                self.idle.notify_all();
            }
        }
    }
}

/// 実際のディスク操作。成功なら true。
fn write_sidecar_to_disk(folder: &Path, request: &WriteRequest) -> bool {
    let path = folder.join(SIDECAR_FILENAME);
    let items = match request {
        WriteRequest::Remove => {
            return match std::fs::remove_file(&path) {
                Ok(()) => true,
                Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => true,
                Err(e) => {
                    crate::logger::log(format!(
                        "sidecar: remove failed: {} ({})",
                        path.display(),
                        e
                    ));
                    false
                }
            };
        }
        WriteRequest::Write(items) => items,
    };

    let json_value = SidecarJson {
        version: CURRENT_VERSION,
        app: Some(format!("mimageviewer {}", env!("CARGO_PKG_VERSION"))),
        saved_at: Some(current_timestamp()),
        items: Arc::clone(items),
    };
    // pretty ではなく compact。人が読むファイルではないし、マスクを packed 文字列に
    // したあとは pretty の改行とインデントが残りの大半になる。
    let json = match serde_json::to_string(&json_value) {
        Ok(s) => s,
        Err(e) => {
            crate::logger::log(format!("sidecar: serialize failed: {e}"));
            return false;
        }
    };

    // アトミック書き込み: temp → rename
    let tmp = folder.join(format!("{SIDECAR_FILENAME}.tmp"));
    if let Err(e) = std::fs::write(&tmp, &json) {
        crate::logger::log(format!("sidecar: write failed: {} ({})", tmp.display(), e));
        return false;
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
        return false;
    }
    #[cfg(windows)]
    mark_hidden_system(&path);
    true
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    pub imported_local_adjust: usize,
    pub imported_export_crop: usize,
    pub imported_comic: usize,
    pub imported_tags: usize,
    pub skipped_adjust: usize,
    pub skipped_mask: usize,
    pub skipped_conceal: usize,
    pub skipped_local_adjust: usize,
    pub skipped_export_crop: usize,
    pub skipped_comic: usize,
    pub skipped_tags: usize,
}

/// サイドカーの各エントリを中央 DB へインポートする (純粋関数、テスト用に App から分離)。
///
/// 中央 DB に既にエントリがあるものは **上書きしない** (中央が authoritative)。
/// `adjust_db` / `mask_db` / `conceal_db` / `tags_db` に None を渡した場合、その DB 種別への
/// インポートはスキップ。`folder` はサイドカーファイルが置かれているフォルダの絶対パス。
/// 絶対 DB キーの再構成は [`reconstruct_image_key`] / [`reconstruct_virtual_key`] に従う。
pub fn import_to_dbs(
    folder: &Path,
    sidecar: &SidecarFile,
    adjust_db: Option<&crate::adjustment_db::AdjustmentDb>,
    mask_db: Option<&crate::mask_db::MaskDb>,
    conceal_db: Option<&crate::conceal_db::ConcealDb>,
    local_adjust_db: Option<&crate::local_adjust_db::LocalAdjustDb>,
    export_crop_db: Option<&crate::export_crop::CropDb>,
    comic_db: Option<&crate::comic_db::ComicDb>,
    mut tags_db: Option<&mut crate::tags_db::TagsDb>,
) -> ImportStats {
    let mut stats = ImportStats::default();
    for (rel_key, entry) in sidecar.items() {
        let rel_kind = classify_rel_key(rel_key);
        let abs_key = match rel_kind {
            RelKeyKind::Image => reconstruct_image_key(folder, rel_key),
            RelKeyKind::ZipImage | RelKeyKind::PdfPage => {
                match reconstruct_virtual_key(folder, rel_key) {
                    Some(k) => k,
                    None => continue,
                }
            }
        };

        if let (Some(db), Some(tags)) = (tags_db.as_deref_mut(), &entry.tags) {
            if matches!(rel_kind, RelKeyKind::Image) {
                if db.has_item_state(&abs_key) || !db.display_tags_for_item(&abs_key).is_empty() {
                    stats.skipped_tags += 1;
                } else if !tags.is_empty()
                    && db
                        .set_item_tags(
                            &abs_key,
                            tags.iter()
                                .map(|tag| crate::tags_db::strip_display_hash(tag)),
                            crate::tags_db::source::SIDECAR,
                        )
                        .is_ok()
                {
                    stats.imported_tags += 1;
                }
            }
        }

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

        if let (Some(db), Some(layers)) = (local_adjust_db, &entry.local_adjust_layers) {
            if !layers.is_empty() {
                if db.get_layers(&abs_key).is_none() {
                    if db.set_layers(&abs_key, layers).is_ok() {
                        stats.imported_local_adjust += 1;
                    }
                } else {
                    stats.skipped_local_adjust += 1;
                }
            }
        }

        if let (Some(db), Some(crop)) = (export_crop_db, entry.export_crop) {
            if db.get(&abs_key).is_none() {
                if db.set(&abs_key, crop).is_ok() {
                    stats.imported_export_crop += 1;
                }
            } else {
                stats.skipped_export_crop += 1;
            }
        }

        if let (Some(db), Some(objects)) = (comic_db, &entry.comic) {
            if !objects.is_empty() {
                // get_raw で「行の有無」を判定する。get() は壊れ JSON も None を返すため、
                // それで判定すると壊れた/将来非互換の中央行をサイドカーで上書きしてしまう
                // (Codex P2)。seed ガードと同じく get_raw に揃え、既存行は一切上書きしない。
                if db.get_raw(&abs_key).is_none() {
                    if db.set(&abs_key, objects).is_ok() {
                        stats.imported_comic += 1;
                    }
                } else {
                    stats.skipped_comic += 1;
                }
            }
        }
    }
    stats
}

// ── Windows 隠し+システム属性 ─────────────────────────────────────────

#[cfg(windows)]
pub(crate) fn mark_hidden_system(path: &Path) {
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

#[cfg(not(windows))]
pub(crate) fn mark_hidden_system(_path: &Path) {}

#[cfg(windows)]
fn attributes_without_hidden_system(attributes: u32) -> u32 {
    use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM};
    attributes & !(FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0)
}

#[cfg(windows)]
pub(crate) fn clear_hidden_system_preserving_other_attributes(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAGS_AND_ATTRIBUTES, GetFileAttributesW, INVALID_FILE_ATTRIBUTES, SetFileAttributesW,
    };
    use windows::core::PCWSTR;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let attributes = unsafe { GetFileAttributesW(PCWSTR(wide.as_ptr())) };
    if attributes == INVALID_FILE_ATTRIBUTES {
        crate::logger::log(format!(
            "metadata bundle attribute read failed {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
        return;
    }
    let cleared = attributes_without_hidden_system(attributes);
    if cleared == attributes {
        return;
    }
    if let Err(error) =
        unsafe { SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(cleared)) }
    {
        crate::logger::log(format!(
            "metadata bundle attribute clear failed {}: {error}",
            path.display()
        ));
    }
}

#[cfg(not(windows))]
pub(crate) fn clear_hidden_system_preserving_other_attributes(_path: &Path) {}

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

    #[cfg(windows)]
    #[test]
    fn attributes_without_hidden_system_preserves_unrelated_bits() {
        use windows::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_COMPRESSED, FILE_ATTRIBUTE_HIDDEN,
            FILE_ATTRIBUTE_NOT_CONTENT_INDEXED, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_SYSTEM,
        };
        let preserved = FILE_ATTRIBUTE_READONLY.0
            | FILE_ATTRIBUTE_ARCHIVE.0
            | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED.0
            | FILE_ATTRIBUTE_COMPRESSED.0;
        let attributes = preserved | FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0;

        assert_eq!(attributes_without_hidden_system(attributes), preserved);
    }

    #[test]
    fn purge_deleted_root_removes_exact_and_container_entries_only() {
        let temp = tempfile::tempdir().unwrap();
        let mut sidecar = SidecarFile::new(temp.path().to_path_buf());
        sidecar.set_adjust("book.pdf", AdjustParams::default());
        sidecar.set_adjust("book.pdf::page_1", AdjustParams::default());
        sidecar.set_adjust("book.pdf2::page_1", AdjustParams::default());

        assert!(sidecar.purge_deleted_root("book.pdf"));
        assert!(!sidecar.items().contains_key("book.pdf"));
        assert!(!sidecar.items().contains_key("book.pdf::page_1"));
        assert!(sidecar.items().contains_key("book.pdf2::page_1"));
    }
    use local_adjust_core::{LocalAdjustmentLayer, LocalEffect, LocalMask};
    use std::path::PathBuf;

    fn sample_params() -> AdjustParams {
        let mut p = AdjustParams::default();
        p.brightness = 10.0;
        p.contrast = -5.0;
        p
    }

    fn sample_local_adjust_layer(name: &str) -> LocalAdjustmentLayer {
        LocalAdjustmentLayer::new(name, LocalMask::Full, LocalEffect::None)
    }

    fn sample_export_crop() -> crate::export_crop::CropSettings {
        crate::export_crop::CropSettings {
            rect: crate::export_crop::CropRect {
                min_x: 10.0,
                min_y: 12.0,
                max_x: 90.0,
                max_y: 70.0,
            },
            aspect_mode: crate::export_crop::CropAspectMode::Ratio4x3,
            source_size: Some([100, 80]),
        }
    }

    #[test]
    fn worker_flush_batch_runs_disconnected_writer_fallback_off_the_owner_thread() {
        let dir = tempfile::tempdir().unwrap();
        let mut sidecar = SidecarFile::new(dir.path().to_path_buf());
        sidecar.set_adjust("a.jpg", sample_params());
        let (owners, batch) = prepare_worker_flush([sidecar]);
        assert_eq!(owners.len(), 1);
        assert!(!batch.is_empty());

        let state = Arc::new(WriterState::default());
        let (tx, rx) = std::sync::mpsc::channel();
        drop(rx);
        let writer = SidecarWriter { tx, state };
        let worker = std::thread::Builder::new()
            .name("sidecar-flush-batch-test".to_string())
            .spawn(move || {
                let name = std::thread::current().name().map(str::to_owned);
                (
                    batch.run_with_writer(&writer, std::time::Duration::from_secs(1)),
                    name,
                )
            })
            .unwrap();
        let (result, thread_name) = worker.join().unwrap();
        let SidecarFlushCompletion::Flushed { sidecars, report } = owners.resolve(result) else {
            panic!("worker flush did not resolve as success");
        };
        assert_eq!(report.flushed_folders, 1);
        assert_eq!(sidecars.len(), 1);
        assert!(!sidecars[0].is_dirty());
        assert_eq!(thread_name.as_deref(), Some("sidecar-flush-batch-test"));
        assert!(dir.path().join(SIDECAR_FILENAME).is_file());
    }

    #[test]
    fn worker_flush_batch_failure_returns_the_original_dirty_owner() {
        let dir = tempfile::tempdir().unwrap();
        let mut sidecar = SidecarFile::new(dir.path().to_path_buf());
        sidecar.set_adjust("a.jpg", sample_params());
        let (owners, batch) = prepare_worker_flush([sidecar]);

        let state = Arc::new(WriterState::default());
        state
            .failed
            .lock()
            .unwrap()
            .insert(dir.path().to_path_buf());
        let (tx, _rx) = std::sync::mpsc::channel();
        let writer = SidecarWriter { tx, state };
        let result = batch.run_with_writer(&writer, std::time::Duration::from_secs(1));
        assert!(result.as_ref().unwrap_err().contains("previously failed"));
        let SidecarFlushCompletion::Failed {
            sidecars: retained,
            error,
        } = owners.resolve(result)
        else {
            panic!("failed worker flush did not retain dirty owners");
        };
        assert!(error.contains("previously failed"));
        assert_eq!(retained.len(), 1);
        assert!(retained[0].is_dirty());
        assert_eq!(retained[0].items().len(), 1);
        assert!(!dir.path().join(SIDECAR_FILENAME).exists());
    }

    #[test]
    fn worker_flush_write_failure_returns_the_original_dirty_owner() {
        let root = tempfile::tempdir().unwrap();
        let missing_folder = root.path().join("removed-before-worker-flush");
        let mut sidecar = SidecarFile::new(missing_folder.clone());
        sidecar.set_adjust("a.jpg", sample_params());
        let (owners, batch) = prepare_worker_flush([sidecar]);

        let state = Arc::new(WriterState::default());
        let (tx, rx) = std::sync::mpsc::channel();
        drop(rx);
        let writer = SidecarWriter { tx, state };
        let result = batch.run_with_writer(&writer, std::time::Duration::from_secs(1));
        assert!(result.as_ref().unwrap_err().contains("failed for"));
        let SidecarFlushCompletion::Failed {
            sidecars: retained,
            error,
        } = owners.resolve(result)
        else {
            panic!("write failure did not retain dirty owners");
        };
        assert!(error.contains("failed for"));
        assert_eq!(retained.len(), 1);
        assert!(retained[0].is_dirty());
        assert_eq!(retained[0].folder(), missing_folder);
        assert!(!missing_folder.join(SIDECAR_FILENAME).exists());
    }

    #[test]
    fn flush_owner_resolution_rejects_an_inconsistent_success_report() {
        let dir = tempfile::tempdir().unwrap();
        let mut sidecar = SidecarFile::new(dir.path().to_path_buf());
        sidecar.set_adjust("a.jpg", sample_params());
        let (owners, _batch) = prepare_worker_flush([sidecar]);
        let identity = Arc::clone(&owners.identity);

        let SidecarFlushCompletion::Failed { sidecars, error } =
            owners.resolve(Ok(SidecarFlushReport {
                identity,
                flushed_folders: 0,
                elapsed: std::time::Duration::ZERO,
            }))
        else {
            panic!("inconsistent worker success must fail closed");
        };
        assert!(error.contains("0 folders for 1 expected flushes"));
        assert_eq!(sidecars.len(), 1);
        assert!(sidecars[0].is_dirty());
    }

    #[test]
    fn flush_owner_resolution_rejects_a_foreign_batch_result() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let mut first_sidecar = SidecarFile::new(first.path().to_path_buf());
        first_sidecar.set_adjust("a.jpg", sample_params());
        let (first_owners, _first_batch) = prepare_worker_flush([first_sidecar]);
        let mut second_sidecar = SidecarFile::new(second.path().to_path_buf());
        second_sidecar.set_adjust("b.jpg", sample_params());
        let (second_owners, _second_batch) = prepare_worker_flush([second_sidecar]);

        let SidecarFlushCompletion::Failed { sidecars, error } =
            first_owners.resolve(Ok(SidecarFlushReport {
                identity: Arc::clone(&second_owners.identity),
                flushed_folders: 1,
                elapsed: std::time::Duration::ZERO,
            }))
        else {
            panic!("a foreign worker result must not clean another batch's owners");
        };
        assert!(error.contains("different owner batch"));
        assert_eq!(sidecars.len(), 1);
        assert!(sidecars[0].is_dirty());
    }

    #[test]
    fn worker_flush_keeps_clean_cache_owners_without_queueing_them() {
        let dir = tempfile::tempdir().unwrap();
        let clean = SidecarFile::new(dir.path().to_path_buf());
        let (owners, batch) = prepare_worker_flush([clean]);
        assert_eq!(owners.len(), 1);
        assert!(batch.is_empty());

        let state = Arc::new(WriterState::default());
        let (tx, _rx) = std::sync::mpsc::channel();
        let writer = SidecarWriter { tx, state };
        let result = batch.run_with_writer(&writer, std::time::Duration::from_secs(1));
        let SidecarFlushCompletion::Flushed { sidecars, report } = owners.resolve(result) else {
            panic!("empty worker batch must preserve its clean owners");
        };
        assert_eq!(report.flushed_folders, 0);
        assert_eq!(sidecars.len(), 1);
        assert!(!sidecars[0].is_dirty());
    }

    #[test]
    fn dirty_disabled_owner_does_not_block_an_unrelated_valid_worker_flush() {
        let root = tempfile::tempdir().unwrap();
        let invalid_folder = root.path().join("invalid");
        let valid_folder = root.path().join("valid");
        std::fs::create_dir_all(&invalid_folder).unwrap();
        std::fs::create_dir_all(&valid_folder).unwrap();
        let invalid_path = invalid_folder.join(SIDECAR_FILENAME);
        std::fs::write(&invalid_path, b"preserve-invalid-source").unwrap();

        let mut invalid = SidecarFile::disabled_placeholder(invalid_folder.clone());
        invalid.set_tags("page.jpg", ["unsaved"]);
        let mut valid = SidecarFile::new(valid_folder.clone());
        valid.set_adjust("page.jpg", sample_params());
        let (owners, batch) = prepare_worker_flush([invalid, valid]);
        assert_eq!(owners.len(), 2);
        assert_eq!(batch.requests.len(), 1);

        let state = Arc::new(WriterState::default());
        let (tx, rx) = std::sync::mpsc::channel();
        drop(rx);
        let writer = SidecarWriter { tx, state };
        let result = batch.run_with_writer(&writer, std::time::Duration::from_secs(1));
        let SidecarFlushCompletion::Flushed { sidecars, report } = owners.resolve(result) else {
            panic!("a disabled owner must not fail the valid folder's worker batch");
        };
        assert_eq!(report.flushed_folders, 1);
        let invalid = sidecars
            .iter()
            .find(|sidecar| sidecar.folder() == invalid_folder)
            .unwrap();
        let valid = sidecars
            .iter()
            .find(|sidecar| sidecar.folder() == valid_folder)
            .unwrap();
        assert!(invalid.is_dirty());
        assert!(!valid.is_dirty());
        assert_eq!(
            std::fs::read(invalid_path).unwrap(),
            b"preserve-invalid-source"
        );
        assert!(valid_folder.join(SIDECAR_FILENAME).is_file());
    }

    fn reservation_success(
        reservations: &SidecarFlushReservations,
    ) -> Result<SidecarFlushReport, String> {
        Ok(SidecarFlushReport {
            identity: Arc::clone(&reservations.identity),
            flushed_folders: reservations.expected_flushes,
            elapsed: std::time::Duration::ZERO,
        })
    }

    #[test]
    fn in_place_flush_clears_only_the_exact_writable_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("book");
        let mut sidecar = SidecarFile::new(folder.clone());
        sidecar.set_tags("page.jpg", ["before"]);
        let mut cache = HashMap::from([(folder.clone(), sidecar)]);
        let (reservations, _batch) = prepare_worker_flush_in_place(cache.values());

        cache
            .get_mut(&folder)
            .unwrap()
            .set_tags("page.jpg", ["after"]);
        let result = reservation_success(&reservations);
        reservations
            .resolve_in_place(&mut cache, result, true)
            .unwrap();

        let current = cache.get(&folder).expect("newer dirty owner retained");
        assert!(current.is_dirty());
        assert_eq!(
            current
                .items()
                .get("page.jpg")
                .and_then(|entry| entry.tags.as_ref()),
            Some(&vec!["#after".to_string()])
        );
    }

    #[test]
    fn in_place_flush_success_evicts_only_exact_clean_main_cache_owners() {
        let root = tempfile::tempdir().unwrap();
        let old_clean_folder = root.path().join("old-clean");
        let flushed_folder = root.path().join("flushed");
        let replaced_folder = root.path().join("replaced-clean");
        let disabled_folder = root.path().join("disabled-dirty");
        let mut flushed = SidecarFile::new(flushed_folder.clone());
        flushed.set_tags("page.jpg", ["saved"]);
        let mut disabled = SidecarFile::disabled_placeholder(disabled_folder.clone());
        disabled.set_tags("page.jpg", ["unsaved"]);
        let mut cache = HashMap::from([
            (
                old_clean_folder.clone(),
                SidecarFile::new(old_clean_folder.clone()),
            ),
            (flushed_folder.clone(), flushed),
            (
                replaced_folder.clone(),
                SidecarFile::new(replaced_folder.clone()),
            ),
            (disabled_folder.clone(), disabled),
        ]);
        let (reservations, _batch) = prepare_worker_flush_in_place(cache.values());
        cache.insert(
            replaced_folder.clone(),
            SidecarFile::new(replaced_folder.clone()),
        );

        let result = reservation_success(&reservations);
        reservations
            .resolve_in_place(&mut cache, result, true)
            .unwrap();

        assert!(!cache.contains_key(&old_clean_folder));
        assert!(!cache.contains_key(&flushed_folder));
        assert!(cache.contains_key(&replaced_folder));
        assert!(cache.get(&disabled_folder).unwrap().is_dirty());
    }

    #[test]
    fn in_place_flush_failure_and_write_disable_leave_live_owner_untouched() {
        let root = tempfile::tempdir().unwrap();
        let failed_folder = root.path().join("failed");
        let mut failed = SidecarFile::new(failed_folder.clone());
        failed.set_tags("page.jpg", ["pending"]);
        let mut cache = HashMap::from([(failed_folder.clone(), failed)]);
        let (reservations, _batch) = prepare_worker_flush_in_place(cache.values());
        assert_eq!(
            reservations
                .resolve_in_place(&mut cache, Err("injected".into()), true)
                .unwrap_err(),
            "injected"
        );
        assert!(cache.get(&failed_folder).unwrap().is_dirty());

        let (reservations, _batch) = prepare_worker_flush_in_place(cache.values());
        cache
            .get_mut(&failed_folder)
            .unwrap()
            .disable_writes_for_session();
        let result = reservation_success(&reservations);
        reservations
            .resolve_in_place(&mut cache, result, true)
            .unwrap();
        assert!(cache.get(&failed_folder).unwrap().is_dirty());
    }

    #[test]
    fn empty_worker_flush_batch_still_requires_the_global_idle_fence() {
        let dir = tempfile::tempdir().unwrap();
        let clean = SidecarFile::new(dir.path().to_path_buf());
        let (owners, batch) = prepare_worker_flush([clean]);
        assert!(batch.is_empty());

        let state = Arc::new(WriterState::default());
        *state.inflight.lock().unwrap() = 1;
        let (tx, _rx) = std::sync::mpsc::channel();
        let writer = SidecarWriter { tx, state };
        let result = batch.run_with_writer(&writer, std::time::Duration::from_millis(5));
        assert!(result.as_ref().unwrap_err().contains("did not become idle"));
        let SidecarFlushCompletion::Failed { sidecars, error } = owners.resolve(result) else {
            panic!("empty batch must not report success while the global writer is busy");
        };
        assert!(error.contains("did not become idle"));
        assert_eq!(sidecars.len(), 1);
        assert!(!sidecars[0].is_dirty());
    }

    #[test]
    fn strict_queue_poison_fails_before_publishing_partial_state() {
        let state = Arc::new(WriterState::default());
        let poisoned = Arc::clone(&state);
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.pending.lock().unwrap();
            panic!("poison pending state for strict admission test");
        })
        .join();

        let error = state
            .queue_strict(PathBuf::from("C:/strict-queue-test"), WriteRequest::Remove)
            .unwrap_err();
        assert!(error.contains("pending state is unavailable"));
        assert_eq!(*state.inflight.lock().unwrap(), 0);
        assert_eq!(state.next_seq.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[test]
    fn strict_idle_fence_reports_timeout_instead_of_success() {
        let state = Arc::new(WriterState::default());
        *state.inflight.lock().unwrap() = 1;
        let (tx, _rx) = std::sync::mpsc::channel();
        let writer = SidecarWriter { tx, state };
        let error = writer
            .wait_until_idle_strict(std::time::Duration::from_millis(5))
            .unwrap_err();
        assert!(error.contains("did not become idle"));
    }

    #[test]
    fn missing_import_source_revalidation_rejects_a_pending_remove() {
        let dir = tempfile::tempdir().unwrap();
        let state = WriterState::default();
        state.queue(dir.path().to_path_buf(), WriteRequest::Remove);
        let error = revalidate_missing_import_source_from(dir.path(), &state).unwrap_err();
        assert!(error.contains("pending write"));
    }

    #[test]
    fn corrupt_sidecar_is_disabled_and_not_overwritten() {
        // DI-7: 現行版だが破損した sidecar を load → disabled になり、次の編集 + flush でも
        // 上書きされない (newer-version 経路と同じ防御。空 load → 上書き消去の回避)。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SIDECAR_FILENAME);
        std::fs::write(&path, b"{ this is not valid json ]").unwrap();
        let original = std::fs::read(&path).unwrap();

        let mut s = SidecarFile::load(dir.path());
        assert!(s.disabled, "corrupt sidecar must be disabled");

        // disabled なら dirty でも flush は書き込まない。
        s.set_adjust("img.jpg", sample_params());
        s.flush_blocking();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            original,
            "corrupt sidecar must not be overwritten while disabled"
        );
    }

    #[test]
    fn sidecar_import_load_keeps_disk_identity_and_terminal_states_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let missing = SidecarFile::load_for_import(dir.path());
        assert!(matches!(missing, SidecarImportLoad::Missing { .. }));

        let path = dir.path().join(SIDECAR_FILENAME);
        let bytes = br#"{"version":1,"items":{}}"#;
        std::fs::write(&path, bytes).unwrap();
        let loaded = SidecarFile::load_for_import(dir.path());
        let SidecarImportLoad::Loaded(loaded) = loaded else {
            panic!("valid disk sidecar must produce a disk token");
        };
        let (sidecar, source) = loaded.into_parts();
        let SidecarImportSource::Disk(token) = source else {
            panic!("valid disk sidecar must produce a disk token");
        };
        assert!(sidecar.items().is_empty());
        assert_eq!(token.byte_len, bytes.len() as u64);
        assert_eq!(token.sha256, <[u8; 32]>::from(Sha256::digest(bytes)));
        assert_eq!(
            token.folder_key,
            crate::adjustment_db::normalize_path(dir.path())
        );

        std::fs::write(&path, b"not json").unwrap();
        let corrupt = SidecarFile::load_for_import(dir.path());
        let SidecarImportLoad::Corrupt { sidecar, .. } = corrupt else {
            panic!("invalid JSON must remain distinguishable");
        };
        assert!(sidecar.disabled);

        std::fs::write(&path, br#"{"version":999,"items":{}}"#).unwrap();
        let future = SidecarFile::load_for_import(dir.path());
        let SidecarImportLoad::UnsupportedVersion { sidecar, version } = future else {
            panic!("future schema must remain distinguishable");
        };
        assert_eq!(version, 999);
        assert!(sidecar.disabled);
    }

    #[test]
    fn sidecar_import_pending_snapshot_has_no_disk_token() {
        let dir = tempfile::tempdir().unwrap();
        let state = WriterState::default();
        let mut items = BTreeMap::new();
        items.insert("a.jpg".to_string(), SidecarEntry::default());
        state.queue(
            dir.path().to_path_buf(),
            WriteRequest::Write(Arc::new(items)),
        );

        let loaded = SidecarFile::load_for_import_from(dir.path(), &state);
        let SidecarImportLoad::Loaded(loaded) = loaded else {
            panic!("pending writer snapshot must win over disk");
        };
        let (sidecar, source) = loaded.into_parts();
        let SidecarImportSource::PendingWriter {
            folder_key,
            sequence,
            kind,
        } = &source
        else {
            panic!("pending writer snapshot must produce a writer token");
        };
        assert_eq!(*sequence, 0);
        assert_eq!(*kind, PendingWriterKind::Write);
        assert_eq!(
            folder_key,
            &crate::adjustment_db::normalize_path(dir.path())
        );
        assert_eq!(sidecar.items().len(), 1);
        assert!(revalidate_import_source_from(&sidecar, &source, &state).is_ok());

        let mut replacement = BTreeMap::new();
        replacement.insert("b.jpg".to_string(), SidecarEntry::default());
        state.queue(
            dir.path().to_path_buf(),
            WriteRequest::Write(Arc::new(replacement)),
        );
        assert!(revalidate_import_source_from(&sidecar, &source, &state).is_err());
    }

    #[test]
    fn sidecar_import_pending_remove_remains_current_until_superseded() {
        let dir = tempfile::tempdir().unwrap();
        let state = WriterState::default();
        state.queue(dir.path().to_path_buf(), WriteRequest::Remove);

        let loaded = SidecarFile::load_for_import_from(dir.path(), &state);
        let SidecarImportLoad::Loaded(loaded) = loaded else {
            panic!("pending remove must be represented as a current empty snapshot");
        };
        let (sidecar, source) = loaded.into_parts();
        assert!(sidecar.items().is_empty());
        assert!(matches!(
            &source,
            SidecarImportSource::PendingWriter {
                kind: PendingWriterKind::Remove,
                ..
            }
        ));
        assert!(revalidate_import_source_from(&sidecar, &source, &state).is_ok());

        state.queue(
            dir.path().to_path_buf(),
            WriteRequest::Write(Arc::new(BTreeMap::new())),
        );
        assert!(revalidate_import_source_from(&sidecar, &source, &state).is_err());
    }

    #[test]
    fn sidecar_import_failed_writer_blocks_stale_disk() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let mut disk_sidecar = SidecarFile::new(media.path().to_path_buf());
        disk_sidecar.set_adjust("old.jpg", sample_params());
        assert!(disk_sidecar.flush_blocking());

        let state = WriterState::default();
        let mut replacement = BTreeMap::new();
        replacement.insert("new.jpg".to_string(), SidecarEntry::default());
        state.queue(
            media.path().to_path_buf(),
            WriteRequest::Write(Arc::new(replacement)),
        );
        state.finish_write(media.path(), 0, false);

        let loaded = SidecarFile::load_for_import_from(media.path(), &state);
        let SidecarImportLoad::WriterFailed { sidecar } = loaded else {
            panic!("a failed writer must stop strict import before stale disk is read");
        };
        assert!(sidecar.disabled);

        let db = crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
            .unwrap();
        let key = reconstruct_image_key(media.path(), "old.jpg");
        assert!(db.get_page_params(&key).is_none());
        assert_eq!(
            db.sidecar_sync_get(&crate::adjustment_db::normalize_path(media.path())),
            None
        );
    }

    #[test]
    fn sidecar_import_fails_closed_when_writer_state_is_poisoned() {
        let dir = tempfile::tempdir().unwrap();
        let pending_state = WriterState::default();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = pending_state.pending.lock().unwrap();
            panic!("poison pending state");
        }));
        let SidecarImportLoad::Unreadable { sidecar, .. } =
            SidecarFile::load_for_import_from(dir.path(), &pending_state)
        else {
            panic!("poisoned pending state must fail closed");
        };
        assert!(sidecar.disabled);

        let failed_state = WriterState::default();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = failed_state.failed.lock().unwrap();
            panic!("poison failed state");
        }));
        let SidecarImportLoad::Unreadable { sidecar, .. } =
            SidecarFile::load_for_import_from(dir.path(), &failed_state)
        else {
            panic!("poisoned failure state must fail closed");
        };
        assert!(sidecar.disabled);
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
    fn replace_edit_bundle_preserves_tags_and_replaces_all_edit_fields() {
        let mut s = SidecarFile::new(PathBuf::from("C:/tmp/nonexistent"));
        s.set_adjust("img.jpg", sample_params());
        s.set_mask(
            "img.jpg",
            SidecarMask {
                w: 2,
                h: 2,
                data: "old".to_string(),
                vectors: Vec::new(),
            },
        );
        s.set_tags("img.jpg", ["keep"]);

        s.replace_edit_bundle(
            "img.jpg",
            SidecarEntry {
                conceal: Some(SidecarMask {
                    w: 4,
                    h: 4,
                    data: "new".to_string(),
                    vectors: Vec::new(),
                }),
                ..SidecarEntry::default()
            },
        );

        let entry = s.items().get("img.jpg").unwrap();
        assert!(entry.adjust.is_none());
        assert!(entry.mask.is_none());
        assert_eq!(entry.conceal.as_ref().unwrap().data, "new");
        assert_eq!(
            entry.tags.as_deref(),
            Some(["#keep".to_string()].as_slice())
        );
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
    fn entry_empty_after_removing_all_kinds() {
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
        s.set_local_adjust_layers(
            "img.jpg",
            Arc::new(vec![sample_local_adjust_layer("layer")]),
        );
        s.set_export_crop("img.jpg", sample_export_crop());
        s.set_tags("img.jpg", ["#tag"]);
        assert_eq!(s.items().len(), 1);
        s.remove_tags("img.jpg");
        assert_eq!(
            s.items().len(),
            1,
            "adjust + mask + conceal + local adjust + crop still present"
        );
        s.remove_export_crop("img.jpg");
        assert_eq!(
            s.items().len(),
            1,
            "adjust + mask + conceal + local adjust still present"
        );
        s.remove_local_adjust_layers("img.jpg");
        assert_eq!(s.items().len(), 1, "adjust + mask + conceal still present");
        s.remove_adjust("img.jpg");
        assert_eq!(s.items().len(), 1, "mask + conceal still present");
        s.remove_mask("img.jpg");
        assert_eq!(s.items().len(), 1, "conceal still present");
        s.remove_conceal("img.jpg");
        assert!(
            s.items().is_empty(),
            "entry dropped when all fields are gone"
        );
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
    fn set_local_adjust_layers_is_independent_of_other_fields() {
        let mut s = SidecarFile::new(PathBuf::from("C:/tmp/nonexistent"));
        s.set_local_adjust_layers("img.jpg", Arc::new(vec![sample_local_adjust_layer("look")]));
        let entry = s.items().get("img.jpg").unwrap();
        assert!(entry.adjust.is_none());
        assert!(entry.mask.is_none());
        assert!(entry.conceal.is_none());
        assert_eq!(entry.local_adjust_layers.as_ref().unwrap().len(), 1);

        s.remove_local_adjust_layers("img.jpg");
        assert!(s.items().is_empty());
    }

    #[test]
    fn set_export_crop_is_independent_of_other_fields() {
        let mut s = SidecarFile::new(PathBuf::from("C:/tmp/nonexistent"));
        s.set_export_crop("img.jpg", sample_export_crop());
        let entry = s.items().get("img.jpg").unwrap();
        assert!(entry.adjust.is_none());
        assert!(entry.mask.is_none());
        assert!(entry.conceal.is_none());
        assert!(entry.local_adjust_layers.is_none());
        assert_eq!(entry.export_crop, Some(sample_export_crop()));

        s.remove_export_crop("img.jpg");
        assert!(s.items().is_empty());
    }

    #[test]
    fn set_tags_normalizes_and_removes_empty_entries() {
        let mut s = SidecarFile::new(PathBuf::from("C:/tmp/nonexistent"));
        s.set_tags("img.jpg", ["#Cat", "dog"]);
        let entry = s.items().get("img.jpg").unwrap();
        assert_eq!(
            entry.tags.as_ref().unwrap(),
            &vec!["#Cat".to_string(), "#dog".to_string()]
        );

        s.set_tags("img.jpg", ["   "]);
        assert!(s.items().is_empty());
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
            s.set_local_adjust_layers(
                "img.jpg",
                Arc::new(vec![
                    sample_local_adjust_layer("base"),
                    sample_local_adjust_layer("finish"),
                ]),
            );
            s.set_export_crop("img.jpg", sample_export_crop());
            s.set_tags("img.jpg", ["#Cat", "dog"]);
            s.flush_blocking();
            assert!(!s.is_dirty());
        }
        let s2 = SidecarFile::load(&folder);
        assert_eq!(s2.items().len(), 2);
        let adj = s2.items().get("img.jpg").unwrap().adjust.as_ref().unwrap();
        assert_eq!(adj.brightness, 10.0);
        assert_eq!(
            s2.items()
                .get("img.jpg")
                .unwrap()
                .local_adjust_layers
                .as_ref()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            s2.items().get("img.jpg").unwrap().export_crop,
            Some(sample_export_crop())
        );
        assert_eq!(
            s2.items().get("img.jpg").unwrap().tags.as_ref().unwrap(),
            &vec!["#Cat".to_string(), "#dog".to_string()]
        );
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
    fn import_to_dbs_imports_local_adjust_without_overwriting_db() {
        let sidecar_dir = tempfile::tempdir().unwrap();
        let folder = sidecar_dir.path();
        let mut s = SidecarFile::new(folder.to_path_buf());
        let imported_layers = vec![sample_local_adjust_layer("from sidecar")];
        let existing_layers = vec![sample_local_adjust_layer("already in db")];
        s.set_local_adjust_layers("fresh.png", Arc::new(imported_layers.clone()));
        s.set_local_adjust_layers(
            "existing.png",
            Arc::new(vec![sample_local_adjust_layer("stale sidecar")]),
        );

        let db_dir = tempfile::tempdir().unwrap();
        let db = crate::local_adjust_db::LocalAdjustDb::open_at(&db_dir.path().join("local.db"))
            .unwrap();
        let fresh_key = reconstruct_image_key(folder, "fresh.png");
        let existing_key = reconstruct_image_key(folder, "existing.png");
        db.set_layers(&existing_key, &existing_layers).unwrap();

        let stats = import_to_dbs(folder, &s, None, None, None, Some(&db), None, None, None);

        assert_eq!(stats.imported_local_adjust, 1);
        assert_eq!(stats.skipped_local_adjust, 1);
        assert_eq!(db.get_layers(&fresh_key), Some(imported_layers));
        assert_eq!(db.get_layers(&existing_key), Some(existing_layers));
    }

    #[test]
    fn import_to_dbs_imports_export_crop_without_overwriting_db() {
        let sidecar_dir = tempfile::tempdir().unwrap();
        let folder = sidecar_dir.path();
        let mut s = SidecarFile::new(folder.to_path_buf());
        let imported_crop = sample_export_crop();
        let existing_crop = crate::export_crop::CropSettings {
            rect: crate::export_crop::CropRect {
                min_x: 1.0,
                min_y: 2.0,
                max_x: 30.0,
                max_y: 40.0,
            },
            aspect_mode: crate::export_crop::CropAspectMode::Square,
            source_size: Some([50, 50]),
        };
        s.set_export_crop("fresh.png", imported_crop);
        s.set_export_crop("existing.png", sample_export_crop());

        let db_dir = tempfile::tempdir().unwrap();
        let db = crate::export_crop::CropDb::open_at(&db_dir.path().join("crop.db")).unwrap();
        let fresh_key = reconstruct_image_key(folder, "fresh.png");
        let existing_key = reconstruct_image_key(folder, "existing.png");
        db.set(&existing_key, existing_crop).unwrap();

        let stats = import_to_dbs(folder, &s, None, None, None, None, Some(&db), None, None);

        assert_eq!(stats.imported_export_crop, 1);
        assert_eq!(stats.skipped_export_crop, 1);
        assert_eq!(db.get(&fresh_key), Some(imported_crop));
        assert_eq!(db.get(&existing_key), Some(existing_crop));
    }

    #[test]
    fn import_to_dbs_imports_comic_without_overwriting_db() {
        use comic_core::{AnnotationObject, TextBlock};
        let mk = |t: &str| {
            vec![AnnotationObject::new_text(
                1,
                (10.0, 20.0),
                TextBlock {
                    text: t.to_string(),
                    ..TextBlock::default()
                },
            )]
        };
        let sidecar_dir = tempfile::tempdir().unwrap();
        let folder = sidecar_dir.path();
        let mut s = SidecarFile::new(folder.to_path_buf());
        let imported = mk("from sidecar");
        let existing = mk("already in db");
        s.set_comic("fresh.png", imported.clone());
        s.set_comic("existing.png", mk("stale sidecar"));

        let db_dir = tempfile::tempdir().unwrap();
        let db = crate::comic_db::ComicDb::open_at(&db_dir.path().join("comic.db")).unwrap();
        let fresh_key = reconstruct_image_key(folder, "fresh.png");
        let existing_key = reconstruct_image_key(folder, "existing.png");
        db.set(&existing_key, &existing).unwrap();

        let stats = import_to_dbs(folder, &s, None, None, None, None, None, Some(&db), None);

        // 中央 DB に無い fresh は import、既に有る existing は上書きしない。
        assert_eq!(stats.imported_comic, 1);
        assert_eq!(stats.skipped_comic, 1);
        assert_eq!(db.get(&fresh_key), Some(imported));
        assert_eq!(db.get(&existing_key), Some(existing));
    }

    #[test]
    fn import_to_dbs_imports_tags_without_resurrecting_decided_items() {
        let sidecar_dir = tempfile::tempdir().unwrap();
        let folder = sidecar_dir.path();
        let mut s = SidecarFile::new(folder.to_path_buf());
        s.set_tags("fresh.png", ["#Cat", "dog"]);
        s.set_tags("existing.png", ["stale"]);
        s.set_tags("deleted.png", ["resurrect"]);
        s.set_tags("book.zip::001.jpg", ["virtual"]);

        let db_dir = tempfile::tempdir().unwrap();
        let mut db = crate::tags_db::TagsDb::open_at(&db_dir.path().join("tags.db")).unwrap();
        let fresh_key = reconstruct_image_key(folder, "fresh.png");
        let existing_key = reconstruct_image_key(folder, "existing.png");
        let deleted_key = reconstruct_image_key(folder, "deleted.png");
        let virtual_key = reconstruct_virtual_key(folder, "book.zip::001.jpg").unwrap();
        db.set_item_tags(&existing_key, ["current"], crate::tags_db::source::EDIT)
            .unwrap();
        db.upsert_item_state(&deleted_key, crate::tags_db::source::EDIT)
            .unwrap();

        let stats = import_to_dbs(
            folder,
            &s,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&mut db),
        );

        assert_eq!(stats.imported_tags, 1);
        assert_eq!(stats.skipped_tags, 2);
        assert_eq!(
            db.display_tags_for_item(&fresh_key),
            vec!["#Cat".to_string(), "#dog".to_string()]
        );
        assert_eq!(
            db.display_tags_for_item(&existing_key),
            vec!["#current".to_string()]
        );
        assert!(db.display_tags_for_item(&deleted_key).is_empty());
        assert!(
            db.display_tags_for_item(&virtual_key).is_empty(),
            "virtual rel keys are edit-data only and must not become item tags"
        );
    }

    #[test]
    fn flush_removes_file_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().to_path_buf();
        let path = folder.join(SIDECAR_FILENAME);

        let mut s = SidecarFile::new(folder.clone());
        s.set_adjust("img.jpg", sample_params());
        s.flush_blocking();
        assert!(path.exists());

        s.remove_adjust("img.jpg");
        s.flush_blocking();
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
        s.flush_blocking();
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
        s.flush_blocking();

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

// ── 非同期書き出しと旧形式の移行 ─────────────────────────────────────────

#[cfg(test)]
mod writer_tests {
    use super::*;

    fn sample_layer() -> local_adjust_core::LocalAdjustmentLayer {
        let mut mask = local_adjust_core::RasterVectorMask::empty(4, 4);
        mask.alpha_mut()[5] = 1.0;
        local_adjust_core::LocalAdjustmentLayer::new(
            "layer",
            local_adjust_core::LocalMask::RasterVector(mask),
            local_adjust_core::LocalEffect::None,
        )
    }

    #[test]
    fn a_queued_write_is_readable_before_the_worker_reaches_it() {
        // worker を回さずに state だけを動かすので、「まだ書いていない」状態を
        // 確実に作れる。ディスクには何も無いのに読めることが要件。
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().to_path_buf();
        let state = WriterState::default();

        let items = items_named(&["img.jpg"]);
        state.queue(folder.clone(), WriteRequest::Write(Arc::clone(&items)));

        assert!(
            !folder.join(SIDECAR_FILENAME).exists(),
            "the test is meaningless if the file is already on disk"
        );
        assert_eq!(
            state.pending_items(&folder).map(|items| items.len()),
            Some(1),
            "a folder switch that reloads now must see the queued content"
        );

        state.write_one(&folder);
        assert!(folder.join(SIDECAR_FILENAME).exists());
        assert!(
            state.pending_items(&folder).is_none(),
            "once it is on disk the queue entry has to go, or the file can never be re-read"
        );
    }

    /// flush は writer へ **同じ割り当て** を渡す。map もその中の原寸ラスターマスクも
    /// 複製しない。
    ///
    /// 欠陥は `queue_flush` / `pending_items` / 書き出しの 3 か所がそれぞれ map 全体を
    /// deep clone していたことで、`local_adjust_layers` の `Vec<f32>` は画像原寸
    /// (24MP なら 1 面 96 MB) なので、フォルダ切替や編集終了のたびに数百 MB を
    /// UI スレッドで写していた (2026-08-29 レビュー R-14)。
    #[test]
    fn queueing_a_flush_hands_the_writer_the_same_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = SidecarFile::new(dir.path().to_path_buf());
        file.set_adjust("a.jpg", crate::adjustment::AdjustParams::default());

        let WriteRequest::Write(queued) = file.write_request() else {
            panic!("中身があるのに削除要求になった");
        };
        assert!(
            Arc::ptr_eq(&queued, &file.items),
            "flush のたびに map 全体を複製している"
        );

        let state = WriterState::default();
        state.queue(
            dir.path().to_path_buf(),
            WriteRequest::Write(Arc::clone(&queued)),
        );
        let read_back = state.pending_items(dir.path()).expect("積んだものが読める");
        assert!(
            Arc::ptr_eq(&read_back, &queued),
            "pending から読み直すたびに map 全体を複製している"
        );
    }

    /// 積んだ後の編集は、積んだ内容に混ざらない。
    ///
    /// `Arc` を共有するので、ここを `get_mut` の unwrap などにすると、writer が
    /// 「積んだ時点の内容」ではなく後から足した編集まで書いてしまう。
    #[test]
    fn an_edit_after_the_flush_does_not_reach_the_queued_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = SidecarFile::new(dir.path().to_path_buf());
        file.set_adjust("a.jpg", crate::adjustment::AdjustParams::default());
        let WriteRequest::Write(queued) = file.write_request() else {
            panic!("中身があるのに削除要求になった");
        };

        file.set_adjust("b.jpg", crate::adjustment::AdjustParams::default());

        assert!(
            !queued.contains_key("b.jpg"),
            "積んだ後の編集が、積んだ内容に混ざった"
        );
        assert!(
            file.items.contains_key("b.jpg"),
            "編集が手元にも入っていない"
        );
        assert!(
            !Arc::ptr_eq(&queued, &file.items),
            "共有したまま書き換えている"
        );
    }

    fn items_named(names: &[&str]) -> Arc<BTreeMap<String, SidecarEntry>> {
        Arc::new(
            names
                .iter()
                .map(|name| (name.to_string(), SidecarEntry::default()))
                .collect(),
        )
    }

    #[test]
    fn an_edit_made_during_a_write_is_not_lost_when_that_write_finishes() {
        // worker が書いている最中に UI スレッドが積み直す並び。実際に起きる順番を
        // 手で再現する (write_one 1 回では、この隙間を作れない)。
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().to_path_buf();
        let state = WriterState::default();

        state.queue(
            folder.clone(),
            WriteRequest::Write(items_named(&["one.jpg"])),
        );
        let (seq, request) = state.take_for_write(&folder).expect("the first job");

        // ここで編集が入る。
        state.queue(
            folder.clone(),
            WriteRequest::Write(items_named(&["one.jpg", "two.jpg"])),
        );

        // 1 件目の書き込みが完了する。
        write_sidecar_to_disk(&folder, &request);
        state.finish_write(&folder, seq, true);

        assert_eq!(
            state.pending_items(&folder).map(|items| items.len()),
            Some(2),
            "the newer edit must stay queued, or a reload sees the pre-edit disk copy"
        );
        assert_eq!(SidecarFile::load_from(&folder, &state).items().len(), 2);

        // 2 件目の job が走ると、ようやくディスクと一致して pending が空になる。
        state.write_one(&folder);
        assert!(state.pending_items(&folder).is_none());
        assert_eq!(SidecarFile::load_from(&folder, &state).items().len(), 2);
    }

    #[test]
    fn a_job_that_was_overtaken_does_not_write_the_older_content() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().to_path_buf();
        let state = WriterState::default();

        state.queue(
            folder.clone(),
            WriteRequest::Write(items_named(&["one.jpg"])),
        );
        state.queue(
            folder.clone(),
            WriteRequest::Write(items_named(&["one.jpg", "two.jpg"])),
        );

        // 2 つ積まれているが、最初の job が既に最新を書いてしまう。
        state.write_one(&folder);
        assert_eq!(SidecarFile::load_from(&folder, &state).items().len(), 2);
        // 2 つめの job は空振りする。ここで 1 件目の内容へ巻き戻ってはいけない。
        state.write_one(&folder);
        assert_eq!(SidecarFile::load_from(&folder, &state).items().len(), 2);
    }

    #[test]
    fn a_queued_removal_reads_back_as_an_empty_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().to_path_buf();
        std::fs::write(
            folder.join(SIDECAR_FILENAME),
            br##"{"version":1,"items":{"img.jpg":{"tags":["#a"]}}}"##,
        )
        .unwrap();
        let state = WriterState::default();
        state.queue(folder.clone(), WriteRequest::Remove);
        assert_eq!(
            state.pending_items(&folder).map(|items| items.len()),
            Some(0),
            "the file is still on disk, but it is already logically gone"
        );
    }

    #[test]
    fn the_released_number_array_form_is_read_and_then_rewritten_packed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SIDECAR_FILENAME);
        // v1.1.0〜v3.2.0 が書いていた形。
        std::fs::write(
            &path,
            br#"{"version":1,"items":{"img.jpg":{"local_adjust_layers":[{
                "name":"layer","enabled":true,"opacity":1.0,
                "mask":{"RasterVector":{"width":2,"height":2,"alpha":[0.0,1.0,0.0,0.0],"shapes":[]}},
                "mask_inverted":false,"mask_expand_px":0.0,"mask_feather_px":0.0,
                "effect":"None"}]}}}"#,
        )
        .unwrap();

        let mut sidecar = SidecarFile::load(dir.path());
        let layers = sidecar
            .items()
            .get("img.jpg")
            .and_then(|entry| entry.local_adjust_layers.as_ref())
            .expect("the old form still has to load");
        assert_eq!(layers.len(), 1);
        assert!(
            sidecar.is_dirty(),
            "reading the old form has to schedule the rewrite, or the file stays huge forever"
        );

        assert!(sidecar.flush_blocking());
        let rewritten = std::fs::read_to_string(&path).unwrap();
        assert!(
            !rewritten.contains(r#""alpha":["#),
            "the number array must be gone after the rewrite: {rewritten}"
        );
        assert!(rewritten.contains(r#""alpha":"q8z:"#));

        // ...and the second read finds nothing to migrate.
        let reloaded = SidecarFile::load(dir.path());
        assert!(!reloaded.is_dirty());
    }

    #[test]
    fn a_page_worth_of_mask_no_longer_dominates_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut sidecar = SidecarFile::new(dir.path().to_path_buf());
        // 1000x1000 は実データ (3082x4486) より小さいが、旧形式なら 1 枚で
        // 4 MB を超える。packed ならページ数を増やしても効かない。
        let mut mask = local_adjust_core::RasterVectorMask::empty(1000, 1000);
        let alpha = mask.alpha_mut();
        for y in 100..140 {
            for x in 100..180 {
                alpha[y * 1000 + x] = 1.0;
            }
        }
        sidecar.set_local_adjust_layers(
            "img.jpg",
            Arc::new(vec![local_adjust_core::LocalAdjustmentLayer::new(
                "layer",
                local_adjust_core::LocalMask::RasterVector(mask),
                local_adjust_core::LocalEffect::None,
            )]),
        );
        assert!(sidecar.flush_blocking());
        let size = std::fs::metadata(dir.path().join(SIDECAR_FILENAME))
            .unwrap()
            .len();
        assert!(
            size < 64 * 1024,
            "one million mask pixels should not cost {size} bytes"
        );
    }

    /// サイドカーは出荷済みのディスク形式で、フォルダを移動したときの復元源になる。
    /// 補正レイヤーを共有所有 (`Arc`) で持つようにしても、書き出す形は**素の配列**の
    /// ままでなければならない。ここが包まれると、旧版が書いたファイルを新版が読めず
    /// (逆も) 移動したフォルダの補正が黙って消える。
    #[test]
    fn local_adjust_layers_are_still_written_as_a_plain_array() {
        let dir = tempfile::tempdir().unwrap();
        let mut sidecar = SidecarFile::new(dir.path().to_path_buf());
        sidecar.set_local_adjust_layers("img.jpg", Arc::new(vec![sample_layer()]));
        assert!(sidecar.flush_blocking());

        let written = std::fs::read_to_string(dir.path().join(SIDECAR_FILENAME)).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        let layers = &parsed["items"]["img.jpg"]["local_adjust_layers"];
        assert!(
            layers.is_array(),
            "補正レイヤーは配列として書かれること: {written}"
        );
        assert_eq!(layers.as_array().unwrap().len(), 1);
    }

    #[test]
    fn the_periodic_deadline_is_measured_from_when_editing_started() {
        // 最後の変更時刻で測ると、編集し続けている間は一度も書かれない。
        let mut sidecar = SidecarFile::new(std::path::PathBuf::from("nowhere"));
        sidecar.set_local_adjust_layers("img.jpg", Arc::new(vec![sample_layer()]));
        let first = sidecar.dirty_since().expect("dirty since the first edit");
        sidecar.set_local_adjust_layers("other.jpg", Arc::new(vec![sample_layer()]));
        assert_eq!(
            sidecar.dirty_since(),
            Some(first),
            "a later edit must not push the deadline back"
        );
    }
}

#[cfg(test)]
mod real_data_dry_run {
    use super::*;

    /// 実データでの一度きりの確認用。`MIV_SIDECAR_DRY_RUN=<folder>` を指定したときだけ走る。
    #[test]
    fn migrate_a_copy_of_a_real_sidecar() {
        let Ok(folder) = std::env::var("MIV_SIDECAR_DRY_RUN") else {
            return;
        };
        let folder = std::path::PathBuf::from(folder);
        let before = std::fs::metadata(folder.join(SIDECAR_FILENAME))
            .unwrap()
            .len();

        let read_at = std::time::Instant::now();
        let mut sidecar = SidecarFile::load(&folder);
        let read_secs = read_at.elapsed().as_secs_f64();
        assert!(
            sidecar.is_dirty(),
            "the copy should still be in the old form"
        );

        // UI スレッドが払うのは queue_flush まで。ここが重いと非同期化の意味が薄れる。
        let queue_at = std::time::Instant::now();
        sidecar.queue_flush();
        let queue_ms = queue_at.elapsed().as_secs_f64() * 1000.0;

        let write_at = std::time::Instant::now();
        assert!(sidecar.flush_blocking());
        let after = std::fs::metadata(folder.join(SIDECAR_FILENAME))
            .unwrap()
            .len();
        println!(
            "{} items, {:.1}MB -> {:.1}MB, read {:.1}s, queue {:.0}ms (UI thread), write {:.1}s",
            sidecar.items().len(),
            before as f64 / 1e6,
            after as f64 / 1e6,
            read_secs,
            queue_ms,
            write_at.elapsed().as_secs_f64(),
        );

        let again = SidecarFile::load(&folder);
        assert!(
            !again.is_dirty(),
            "the second read must find nothing to migrate"
        );
        assert_eq!(again.items().len(), sidecar.items().len());
    }
}
