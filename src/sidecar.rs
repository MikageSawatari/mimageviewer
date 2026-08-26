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
    /// 補正レイヤー配列。中央 `local_adjust.db` が authoritative で、サイドカーは
    /// フォルダ移動時の復元用バックアップとして扱う。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_adjust_layers: Option<Vec<local_adjust_core::LocalAdjustmentLayer>>,
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
    items: BTreeMap<String, SidecarEntry>,
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
            items: BTreeMap::new(),
            dirty: false,
            disabled: false,
            dirty_since: None,
        }
    }

    /// フォルダから `mimageviewer.dat` を読み込む。無ければ空のサイドカーを返す。
    /// パース失敗時もログ 1 行で空サイドカーを返す (古いバージョンや壊れたファイルで落ちない)。
    ///
    /// 書き出しは非同期なので、**まだディスクに届いていない内容が writer に残っていれば
    /// そちらを返す**。これが無いと、フォルダを離れて戻るだけで直前の編集が消えて見える。
    pub fn load(folder: &Path) -> Self {
        Self::load_from(folder, &writer().state)
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
            self.items.remove(rel_key);
        } else {
            self.items.insert(rel_key.to_string(), replacement);
        }
        self.mark_dirty();
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

    /// 補正レイヤー配列をセットする。空配列は削除と同じ扱いにする。
    pub fn set_local_adjust_layers(
        &mut self,
        rel_key: &str,
        layers: Vec<local_adjust_core::LocalAdjustmentLayer>,
    ) {
        if layers.is_empty() {
            self.remove_local_adjust_layers(rel_key);
            return;
        }
        let entry = self.items.entry(rel_key.to_string()).or_default();
        entry.local_adjust_layers = Some(layers);
        self.mark_dirty();
    }

    /// 補正レイヤー配列を取り除く。
    pub fn remove_local_adjust_layers(&mut self, rel_key: &str) {
        if let Some(entry) = self.items.get_mut(rel_key) {
            if entry.local_adjust_layers.is_some() {
                entry.local_adjust_layers = None;
                if entry.is_empty() {
                    self.items.remove(rel_key);
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
        let entry = self.items.entry(rel_key.to_string()).or_default();
        entry.comic = Some(objects);
        self.mark_dirty();
    }

    /// テキスト注釈ドキュメントを取り除く。
    pub fn remove_comic(&mut self, rel_key: &str) {
        if let Some(entry) = self.items.get_mut(rel_key) {
            if entry.comic.is_some() {
                entry.comic = None;
                if entry.is_empty() {
                    self.items.remove(rel_key);
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
        let entry = self.items.entry(rel_key.to_string()).or_default();
        entry.tags = Some(normalized);
        self.mark_dirty();
    }

    /// mIV タグのバックアップを取り除く。
    pub fn remove_tags(&mut self, rel_key: &str) {
        if let Some(entry) = self.items.get_mut(rel_key) {
            if entry.tags.is_some() {
                entry.tags = None;
                if entry.is_empty() {
                    self.items.remove(rel_key);
                }
                self.mark_dirty();
            }
        }
    }

    /// 最後段 crop 設定をセットする。
    pub fn set_export_crop(&mut self, rel_key: &str, settings: crate::export_crop::CropSettings) {
        let entry = self.items.entry(rel_key.to_string()).or_default();
        entry.export_crop = Some(settings);
        self.mark_dirty();
    }

    /// 最後段 crop 設定を取り除く。
    pub fn remove_export_crop(&mut self, rel_key: &str) {
        if let Some(entry) = self.items.get_mut(rel_key) {
            if entry.export_crop.is_some() {
                entry.export_crop = None;
                if entry.is_empty() {
                    self.items.remove(rel_key);
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
        self.items.retain(|key, _| {
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
        let request = if self.items.is_empty() {
            WriteRequest::Remove
        } else {
            WriteRequest::Write(self.items.clone())
        };
        writer().enqueue(self.folder.clone(), request);
        self.dirty = false;
        self.dirty_since = None;
    }

    /// この内容がディスクに載っているか。積んだだけで**まだ待っていない**間は false
    /// なので、[`wait_for_pending_writes`] のあとに読むこと。
    pub fn written_to_disk(&self) -> bool {
        !self.dirty && !writer().is_failed(&self.folder)
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
    Write(BTreeMap<String, SidecarEntry>),
    Remove,
}

struct QueuedWrite {
    /// 積んだ順番。worker は書いた後、これが変わっていなければ pending から外す。
    seq: u64,
    request: WriteRequest,
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

    fn pending_items(&self, folder: &Path) -> Option<BTreeMap<String, SidecarEntry>> {
        let pending = self.pending.lock().ok()?;
        match &pending.get(folder)?.request {
            WriteRequest::Write(items) => Some(items.clone()),
            WriteRequest::Remove => Some(BTreeMap::new()),
        }
    }

    fn is_failed(&self, folder: &Path) -> bool {
        self.failed
            .lock()
            .map(|failed| failed.contains(folder))
            .unwrap_or(false)
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
        items: items.clone(),
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
        s.set_local_adjust_layers("img.jpg", vec![sample_local_adjust_layer("layer")]);
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
        s.set_local_adjust_layers("img.jpg", vec![sample_local_adjust_layer("look")]);
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
                vec![
                    sample_local_adjust_layer("base"),
                    sample_local_adjust_layer("finish"),
                ],
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
        s.set_local_adjust_layers("fresh.png", imported_layers.clone());
        s.set_local_adjust_layers(
            "existing.png",
            vec![sample_local_adjust_layer("stale sidecar")],
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
        mask.alpha[5] = 1.0;
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

        let mut items = BTreeMap::new();
        items.insert("img.jpg".to_string(), SidecarEntry::default());
        state.queue(folder.clone(), WriteRequest::Write(items.clone()));

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

    fn items_named(names: &[&str]) -> BTreeMap<String, SidecarEntry> {
        names
            .iter()
            .map(|name| (name.to_string(), SidecarEntry::default()))
            .collect()
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
        for y in 100..140 {
            for x in 100..180 {
                mask.alpha[y * 1000 + x] = 1.0;
            }
        }
        sidecar.set_local_adjust_layers(
            "img.jpg",
            vec![local_adjust_core::LocalAdjustmentLayer::new(
                "layer",
                local_adjust_core::LocalMask::RasterVector(mask),
                local_adjust_core::LocalEffect::None,
            )],
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

    #[test]
    fn the_periodic_deadline_is_measured_from_when_editing_started() {
        // 最後の変更時刻で測ると、編集し続けている間は一度も書かれない。
        let mut sidecar = SidecarFile::new(std::path::PathBuf::from("nowhere"));
        sidecar.set_local_adjust_layers("img.jpg", vec![sample_layer()]);
        let first = sidecar.dirty_since().expect("dirty since the first edit");
        sidecar.set_local_adjust_layers("other.jpg", vec![sample_layer()]);
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
