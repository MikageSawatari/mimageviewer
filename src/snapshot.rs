//! ★固定 (Snapshot Lock) 機能のデータ型と path 正規化純関数。
//!
//! 設計: [docs/star-lock-snapshot-design.md](../docs/star-lock-snapshot-design.md)
//!
//! 役割:
//! - 「現在の絞り込み結果」を一時的に凍結して、その範囲内でフルスクリーン /
//!   スライドショー巡回を可能にする
//! - 永続化しない (= アプリ再起動で消える)
//! - 検索 (Ctrl+F/S/G) と snapshot は scope mutual exclusion (= 同時 active 不可)
//!
//! 本ファイルでは:
//! - `SnapshotKey` (path 正規化済み identity)
//! - `SnapshotEntry` (snapshot に含まれる 1 件分)
//! - `SnapshotEntryKind` (Folder / Image / ... の種別)
//! - `SnapshotSourceLabel` (どんな絞り込み元から作られたか)
//! - `FilterState` (filter 変化検出用、★レベル限定)
//! - 正規化純関数 (`snapshot_key_from_path` / `snapshot_key_from_grid_item` /
//!   `normalize_fs` / `split_archive_path` / `is_inside_fs`)
//!
//! App との結合 (state 切替・UI・nav resolver) は app.rs / ui_main.rs 側。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::grid_item::GridItem;

// ─── SnapshotKey: path を normalize した hash 可能な identity ──────────

/// path 正規化済みの hash 可能な identity。
/// Windows の case-fold / separator / trailing sep / `\\?\` extended prefix の
/// 落とし穴を吸収する (詳細: docs/star-lock-snapshot-design.md §4.6 SnapshotKey 厳密定義)。
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub enum SnapshotKey {
    /// 通常の filesystem path (= Folder / Image / Video / ZipFile / PdfFile /
    /// ConvertibleArchive)
    Fs(String),
    /// アーカイブ inner path (= ZipImage / PdfPage)
    /// - container: archive ファイル自体の normalized path
    /// - inner: ZIP は entry path (例: "sub/image.png")、PDF は page 番号文字列 (例: "p:0")
    Archive { container: String, inner: String },
}

/// snapshot に含まれる 1 件分の entry。
///
/// `kind` で対応する操作 (folder enter / image fullscreen / page render) を切り替え、
/// `key` で identity 比較する。`target` は実際の open 経路で使う元 path 構造体
/// (= ZipImage は `<zip>+<entry_name>` 構造を保持、PdfPage は `<pdf>+<page_num>` 構造を保持)。
/// `display` は UI 表示用 (lazy 評価しない、構築時に決定)。
///
/// ⚠ 旧版では `display` を path round-trip に使っていたが、ZipImage の `<zip>:<entry>` /
/// PdfPage の `<pdf>:Page N` 形式が `snapshot_key_from_path` で解釈不能だったため、
/// `target` を分離して構造を保持する (Codex P1-1 fix)。
#[derive(Clone, Debug)]
pub struct SnapshotEntry {
    pub key: SnapshotKey,
    pub kind: SnapshotEntryKind,
    pub target: SnapshotTarget,
    /// UI 表示・log 用の文字列形式 path。`display_path()` を都度呼ぶより安価。
    pub display: String,
}

/// snapshot entry が指す元 path 構造 (= 再 open / reconstruct で使う)。
///
/// `display` 文字列からの round-trip では復元できない構造 (ZipImage / PdfPage) を
/// 保持するため、専用 enum で持つ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotTarget {
    /// 通常 filesystem path (= Folder / Image / Video / ZipFile / PdfFile /
    /// ConvertibleArchive)。GridItem への復元は kind と組み合わせて行う。
    Fs(PathBuf),
    /// ZIP 内 image entry
    ZipImage {
        zip_path: PathBuf,
        entry_name: String,
    },
    /// PDF page (0-indexed)
    PdfPage { pdf_path: PathBuf, page_num: u32 },
}

/// snapshot entry の種別。
///
/// `Folder` / `ZipFile` / `PdfFile` / `ConvertibleArchive` は **コンテナ** (= owner-entry
/// になりうる)、`Image` / `Video` / `ZipImage` / `PdfPage` は **playable leaf**。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SnapshotEntryKind {
    Folder,
    Image,
    Video,
    ZipFile,
    PdfFile,
    ConvertibleArchive,
    ZipImage,
    PdfPage,
}

impl SnapshotEntryKind {
    /// owner-entry になりうるコンテナ (= 中に入って閲覧する対象) か。
    pub fn is_container(self) -> bool {
        matches!(
            self,
            Self::Folder | Self::ZipFile | Self::PdfFile | Self::ConvertibleArchive
        )
    }

    /// 直接再生可能な leaf (= fullscreen で 1 件として開ける) か。
    pub fn is_playable_leaf(self) -> bool {
        matches!(
            self,
            Self::Image | Self::Video | Self::ZipImage | Self::PdfPage
        )
    }
}

/// snapshot がどんな絞り込み元から作られたかのラベル。
///
/// tooltip / debug log 用。filter 変化検出には使わない (= それは [`FilterState`])。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotSourceLabel {
    /// ★ filter から (active な ★ レベルの一覧)
    RatingFilter { active_levels: Vec<u8> },
    /// Ctrl+F text search の query
    TextSearch { query: String },
    /// Ctrl+S favorite search の query
    FavSearch { query: String },
    /// Ctrl+G global search の query
    GlobalSearch { query: String },
    /// 複数 source の組み合わせ
    Mixed,
}

/// snapshot 中の filter 変化検出に使う state スナップショット。
///
/// 検索系 (Ctrl+F/S/G) は scope mutual exclusion で consume されるため含めない。
/// snapshot 中も操作可能な ★レベル filter のみ追跡する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterState {
    /// ★ filter (index 0=未評価, 1=★1, ..., 5=★5)
    pub rating_filter: [bool; 6],
}

impl FilterState {
    pub fn from_rating(rating_filter: [bool; 6]) -> Self {
        Self { rating_filter }
    }
}

// ─── path 正規化純関数 ────────────────────────────────────

/// `\\?\` extended prefix を取り除く。
///
/// Windows のロングパス対応で `\\?\C:\foo` 形式が来た場合に `C:\foo` に戻す。
/// UNC `\\?\UNC\server\share` は `\\server\share` に戻す。
fn strip_extended_prefix(s: &str) -> &str {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        // `\\?\UNC\server\share` → `\\server\share` だが、戻り値 `rest` は
        // 先頭 `\\` を失っているので caller 側で接頭辞を再構築する必要がある。
        // ここではシンプルに extended prefix 部分のみ取り除き、文字列の意味は
        // 「`server\share\...`」として normalize_fs に渡す (= 後段で
        // forward slash 統一されるので問題なし)。
        return rest;
    }
    s.strip_prefix(r"\\?\").unwrap_or(s)
}

/// 通常 filesystem path を normalize して `SnapshotKey::Fs` の中身となる文字列にする。
///
/// 規則:
/// 1. `\\?\` extended prefix を剥がす
/// 2. forward slash に統一 (= path_key::normalize_keep_drive と同じ規則)
/// 3. 小文字化 (= Windows case-insensitive)
/// 4. trailing `/` を剥がす
pub fn normalize_fs(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let stripped = strip_extended_prefix(&raw);
    let mut s = stripped.to_lowercase().replace('\\', "/");
    while s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    s
}

/// path をアーカイブの container 部分と inner 部分に分割する。
///
/// 既存実装の `global_search_ui::split_zip_hit_path` は `search_norm::ZIP_ENTRY_SEP`
/// (= U+001F) 区切りの hit_path 専用なので、ここでは GridItem からの構築では使わない。
/// 本関数は **fullscreen で開かれた任意 path** から container/inner を逆算するために使う。
///
/// 戻り値:
/// - `Some((container, inner))` = path が `<container>.zip/<inner>` 形式
/// - `None` = 通常 path (= 拡張子 `.zip` / `.pdf` の境界が path 中に無い)
///
/// ⚠ ZipImage / PdfPage は GridItem 側で既に container と inner が分かれているので、
/// その場合は `snapshot_key_from_grid_item` で直接 `SnapshotKey::Archive` を構築する。
/// 本関数はあくまで「raw な path から逆算」用。
pub fn split_archive_path(path: &Path) -> Option<(PathBuf, String)> {
    let s = path.to_string_lossy().to_string();
    // forward slash 統一 (= 検索しやすくするため)
    let unified = s.replace('\\', "/");
    // 拡張子 `.zip` / `.pdf` の境界を探す (= 大文字小文字混在対応で小文字化済みを使う)
    let lower = unified.to_lowercase();
    for needle in [".zip/", ".pdf/"] {
        if let Some(idx) = lower.find(needle) {
            // needle は `.zip/` / `.pdf/` の 5 文字、container は idx + 4 (= `.zip` まで)
            let container_end = idx + 4;
            let container_str = &unified[..container_end];
            let inner_str = &unified[container_end + 1..];
            return Some((PathBuf::from(container_str), inner_str.to_string()));
        }
    }
    None
}

/// 任意 path から `SnapshotKey` を構築する (= 完全一致 lookup / owner_entry 用)。
pub fn snapshot_key_from_path(path: &Path) -> SnapshotKey {
    if let Some((container, inner)) = split_archive_path(path) {
        SnapshotKey::Archive {
            container: normalize_fs(&container),
            inner: normalize_archive_inner(&inner),
        }
    } else {
        SnapshotKey::Fs(normalize_fs(path))
    }
}

/// アーカイブ inner path の normalize (= ZIP entry path / PDF page 番号)。
///
/// 規則: forward slash 統一 + 小文字化 (= entry 名は通常 case-sensitive ではあるが、
/// owner_entry lookup の falseの positive を避けるため Fs 側と同じ規則で揃える)
fn normalize_archive_inner(s: &str) -> String {
    s.replace('\\', "/").to_lowercase()
}

/// PDF page 番号を `SnapshotKey::Archive` の `inner` 文字列にする。
///
/// page 番号文字列を ZIP entry path と区別するため、`p:<num>` という prefix を付ける。
pub fn pdf_page_inner_key(page_num: u32) -> String {
    format!("p:{page_num}")
}

/// `GridItem` から `SnapshotKey` を構築する。
///
/// 戻り値:
/// - `Some(key)` = snapshot 化可能なアイテム
/// - `None` = snapshot 対象外 (= `ZipSeparator`, `SearchContainer`)
pub fn snapshot_key_from_grid_item(item: &GridItem) -> Option<SnapshotKey> {
    match item {
        GridItem::Folder(p)
        | GridItem::Image(p)
        | GridItem::Video(p)
        | GridItem::ZipFile(p)
        | GridItem::PdfFile(p) => Some(SnapshotKey::Fs(normalize_fs(p))),
        GridItem::ConvertibleArchive { path, .. } => Some(SnapshotKey::Fs(normalize_fs(path))),
        GridItem::ZipImage {
            zip_path,
            entry_name,
        } => Some(SnapshotKey::Archive {
            container: normalize_fs(zip_path),
            inner: normalize_archive_inner(entry_name),
        }),
        GridItem::PdfPage {
            pdf_path, page_num, ..
        } => Some(SnapshotKey::Archive {
            container: normalize_fs(pdf_path),
            inner: pdf_page_inner_key(*page_num),
        }),
        // snapshot 対象外
        GridItem::ZipSeparator { .. } => None,
        // MVP では SearchContainer は §4.5 で disable 扱い (= 取り込まない)
        GridItem::SearchContainer { .. } => None,
    }
}

/// `GridItem` から `SnapshotEntryKind` を取り出す。
pub fn snapshot_entry_kind(item: &GridItem) -> Option<SnapshotEntryKind> {
    match item {
        GridItem::Folder(_) => Some(SnapshotEntryKind::Folder),
        GridItem::Image(_) => Some(SnapshotEntryKind::Image),
        GridItem::Video(_) => Some(SnapshotEntryKind::Video),
        GridItem::ZipFile(_) => Some(SnapshotEntryKind::ZipFile),
        GridItem::PdfFile(_) => Some(SnapshotEntryKind::PdfFile),
        GridItem::ConvertibleArchive { .. } => Some(SnapshotEntryKind::ConvertibleArchive),
        GridItem::ZipImage { .. } => Some(SnapshotEntryKind::ZipImage),
        GridItem::PdfPage { .. } => Some(SnapshotEntryKind::PdfPage),
        GridItem::ZipSeparator { .. } | GridItem::SearchContainer { .. } => None,
    }
}

/// `GridItem` から `SnapshotEntry` を構築する。snapshot 対象外なら `None`。
pub fn snapshot_entry_from_grid_item(item: &GridItem) -> Option<SnapshotEntry> {
    let key = snapshot_key_from_grid_item(item)?;
    let kind = snapshot_entry_kind(item)?;
    let target = snapshot_target_from_grid_item(item)?;
    Some(SnapshotEntry {
        key,
        kind,
        target,
        display: item.display_path(),
    })
}

/// `GridItem` から `SnapshotTarget` を構築する。
///
/// `Fs` variant は通常 PathBuf、Archive 系 (ZipImage/PdfPage) は構造化して保持。
pub fn snapshot_target_from_grid_item(item: &GridItem) -> Option<SnapshotTarget> {
    match item {
        GridItem::Folder(p)
        | GridItem::Image(p)
        | GridItem::Video(p)
        | GridItem::ZipFile(p)
        | GridItem::PdfFile(p) => Some(SnapshotTarget::Fs(p.clone())),
        GridItem::ConvertibleArchive { path, .. } => Some(SnapshotTarget::Fs(path.clone())),
        GridItem::ZipImage {
            zip_path,
            entry_name,
        } => Some(SnapshotTarget::ZipImage {
            zip_path: zip_path.clone(),
            entry_name: entry_name.clone(),
        }),
        GridItem::PdfPage {
            pdf_path, page_num, ..
        } => Some(SnapshotTarget::PdfPage {
            pdf_path: pdf_path.clone(),
            page_num: *page_num,
        }),
        GridItem::ZipSeparator { .. } | GridItem::SearchContainer { .. } => None,
    }
}

/// `SnapshotEntry` から `GridItem` を復元する (= snapshot list view への帰還で使う)。
///
/// `target` 構造を保持しているので ZipImage/PdfPage も正しく復元できる。
/// `PdfPage::content_type` は復元できない (= 元 GridItem 構築時に None で再構築、
/// fullscreen で再描画されると更新される)。
pub fn reconstruct_grid_item(entry: &SnapshotEntry) -> Option<GridItem> {
    match (&entry.target, entry.kind) {
        (SnapshotTarget::Fs(p), SnapshotEntryKind::Folder) => Some(GridItem::Folder(p.clone())),
        (SnapshotTarget::Fs(p), SnapshotEntryKind::Image) => Some(GridItem::Image(p.clone())),
        (SnapshotTarget::Fs(p), SnapshotEntryKind::Video) => Some(GridItem::Video(p.clone())),
        (SnapshotTarget::Fs(p), SnapshotEntryKind::ZipFile) => Some(GridItem::ZipFile(p.clone())),
        (SnapshotTarget::Fs(p), SnapshotEntryKind::PdfFile) => Some(GridItem::PdfFile(p.clone())),
        // ConvertibleArchive は format 情報を失うので restore できない。Fs 扱いで近似:
        // 実用上は snapshot 内に ConvertibleArchive を含めるケースは稀なので MVP として許容。
        (SnapshotTarget::Fs(p), SnapshotEntryKind::ConvertibleArchive) => {
            Some(GridItem::Folder(p.clone()))
        }
        (
            SnapshotTarget::ZipImage {
                zip_path,
                entry_name,
            },
            SnapshotEntryKind::ZipImage,
        ) => Some(GridItem::ZipImage {
            zip_path: zip_path.clone(),
            entry_name: entry_name.clone(),
        }),
        (SnapshotTarget::PdfPage { pdf_path, page_num }, SnapshotEntryKind::PdfPage) => {
            Some(GridItem::PdfPage {
                pdf_path: pdf_path.clone(),
                page_num: *page_num,
                content_type: None,
            })
        }
        // target と kind が不整合のケースは構築不能 (= 設計バグ、debug log のみ)
        _ => None,
    }
}

/// 子 path が親 fs path 配下にあるか判定する (= sibling false positive 防止)。
///
/// `C:\foo` は `C:\foobar\baz` を **own しない** ように、separator 境界で確認する。
pub fn is_inside_fs(child: &SnapshotKey, parent_fs: &str) -> bool {
    match child {
        SnapshotKey::Fs(c) => path_starts_with_dir(c, parent_fs),
        SnapshotKey::Archive { container, .. } => path_starts_with_dir(container, parent_fs),
    }
}

/// `child` が `parent_dir` 配下にあるか (= prefix 一致 + separator 境界)。
///
/// 両者とも `normalize_fs` 経由で構築されている前提 (= forward slash, 小文字, trailing sep なし)。
fn path_starts_with_dir(child: &str, parent_dir: &str) -> bool {
    if parent_dir.is_empty() {
        return false;
    }
    if !child.starts_with(parent_dir) {
        return false;
    }
    let rest = &child[parent_dir.len()..];
    // 完全一致は inside 扱いしない (= 同じ path は完全一致 lookup 側で解決すべき)
    rest.starts_with('/')
}

// ─── 既定値 ────────────────────────────────────

impl Default for FilterState {
    fn default() -> Self {
        // 全 ★ レベルを許可 (= filter なし状態と等価)
        Self {
            rating_filter: [true; 6],
        }
    }
}

/// `App` が snapshot active 中に保持する state。`App.snapshot: Option<SnapshotState>` で
/// active/inactive を表現する (= `is_some()` で判定)。
///
/// snapshot ON 時に既存の `App.items` / `App.thumbnails` / `App.visible_indices` /
/// `App.scroll_offset_y` を `saved_*` field に退避し、snapshot subset で置き換える。
/// snapshot OFF 時に `saved_*` から復元 (= 元のフォルダ表示に戻る)。
///
/// 設計: docs/star-lock-snapshot-design.md §4.2 / §4.6 / §5
///
/// `Debug` 派生していないのは `GridItem` / `ThumbnailState` が `Debug` 未実装のため
/// (= GUI 系の型は Debug 出力されることを想定していない)。snapshot state を
/// log に出したい場合は `origin` / `items.len()` 等の具体的 field を個別に出す。
#[derive(Clone)]
pub struct SnapshotState {
    // ── snapshot 本体 ──
    /// snapshot に含まれる entry list (= top-level grid 表示順を保つ)
    pub items: Vec<SnapshotEntry>,
    /// O(1) membership / owner_entry lookup 用。`items` の index を指す。
    pub membership: HashMap<SnapshotKey, usize>,
    /// snapshot 起点となった base path (= 表示・解除時の reference)
    pub origin: PathBuf,
    /// ★レベル filter の capture 時 snapshot (= 後で current と比較して
    /// 「filter 変更後」suffix 判定に使う)
    pub filter_at_capture: FilterState,
    /// 絞り込み元のラベル (tooltip / debug 用)
    pub source_label: SnapshotSourceLabel,
    /// pending folder nav の世代識別 (= snapshot OFF 後に旧 nav 結果が来ても無視)
    pub generation_id: u64,

    // ── 退避 state (= snapshot OFF 時に復元) ──
    /// snapshot ON 時点の `App.items` (= 通常フォルダの GridItem 一覧)
    pub saved_items: Vec<crate::grid_item::GridItem>,
    /// snapshot ON 時点の `App.thumbnails` (= GPU texture 状態含む)
    pub saved_thumbnails: Vec<crate::grid_item::ThumbnailState>,
    /// snapshot ON 時点の `App.visible_indices` (= filter 適用後の indices)
    pub saved_visible_indices: Vec<usize>,
    /// snapshot ON 時点の `App.scroll_offset_y`
    pub saved_scroll_offset_y: f32,
    /// snapshot ON 時点の `App.selected` (= 選択中セル idx、復元用)
    pub saved_selected: Option<usize>,

    // ── snapshot list view 用の固定 state (= BS で復帰したときに使う) ──
    /// snapshot list view を構成する GridItem 一覧 (= snapshot subset の clone)。
    /// `reconstruct_grid_item` での再構築だと ZipImage/PdfPage の細部 (= content_type 等)
    /// が失われるため、初回 activate 時の clone を保持する。
    pub list_view_items: Vec<crate::grid_item::GridItem>,
    /// 同上、サムネイル状態 (= GPU texture 含む、ロード済みフォルダ代表サムネ保持)。
    /// 子フォルダから BS で戻った際にサムネが Pending に戻らないために必要。
    pub list_view_thumbnails: Vec<crate::grid_item::ThumbnailState>,
}

// ═══════════════════════════════════════════════════════════
// unit tests (= path 正規化と純関数群の振る舞い検証)
// ═══════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ─── normalize_fs ──────────────────────────────────

    #[test]
    fn normalize_fs_lowercases_drive_and_segments() {
        assert_eq!(normalize_fs(Path::new(r"C:\Foo\Bar")), "c:/foo/bar");
        assert_eq!(
            normalize_fs(Path::new(r"E:\Photos\IMG.jpg")),
            "e:/photos/img.jpg"
        );
    }

    #[test]
    fn normalize_fs_case_only_difference_collapses() {
        let a = normalize_fs(Path::new(r"E:\Foo"));
        let b = normalize_fs(Path::new(r"e:\foo"));
        assert_eq!(a, b);
    }

    #[test]
    fn normalize_fs_unifies_separators() {
        assert_eq!(
            normalize_fs(Path::new(r"C:\Mixed/Slash\Path")),
            "c:/mixed/slash/path"
        );
    }

    #[test]
    fn normalize_fs_strips_trailing_separator() {
        assert_eq!(normalize_fs(Path::new(r"C:\Foo\")), "c:/foo");
        assert_eq!(normalize_fs(Path::new("C:/Foo/")), "c:/foo");
    }

    #[test]
    fn normalize_fs_strips_extended_prefix() {
        assert_eq!(normalize_fs(Path::new(r"\\?\C:\foo")), "c:/foo");
        // `\\?\UNC\server\share\file` → 後段で forward slash 化されて
        // `server/share/file` 相当になる (= 元の `\\server\share` 表記とは別形式だが、
        // snapshot 内で同じ正規化を通すので owner lookup で一致する)
        let unc_extended = normalize_fs(Path::new(r"\\?\UNC\server\share\file"));
        assert_eq!(unc_extended, "server/share/file");
    }

    #[test]
    fn normalize_fs_root_path_is_preserved() {
        // root だけ `/` の場合は剥がさない (= pop 後に空にならないようにする)
        assert_eq!(normalize_fs(Path::new("/")), "/");
        assert_eq!(normalize_fs(Path::new(r"\")), "/");
    }

    // ─── split_archive_path ──────────────────────────────────

    #[test]
    fn split_archive_path_detects_zip_inner_image() {
        let (container, inner) =
            split_archive_path(Path::new(r"E:\foo.zip\sub\image.png")).unwrap();
        assert_eq!(container, PathBuf::from(r"E:\foo.zip"));
        assert_eq!(inner, "sub/image.png");
    }

    #[test]
    fn split_archive_path_detects_pdf_inner_page() {
        let (container, inner) = split_archive_path(Path::new(r"E:\doc.pdf\p:42")).unwrap();
        assert_eq!(container, PathBuf::from(r"E:\doc.pdf"));
        assert_eq!(inner, "p:42");
    }

    #[test]
    fn split_archive_path_normal_path_returns_none() {
        assert!(split_archive_path(Path::new(r"E:\Photos\image.png")).is_none());
        assert!(split_archive_path(Path::new(r"E:\foo.zip")).is_none()); // archive 自体、inner なし
    }

    #[test]
    fn split_archive_path_case_insensitive_extension() {
        // 拡張子は大文字小文字混在でも検出
        let (container, _) = split_archive_path(Path::new(r"E:\Foo.ZIP\inner.png")).unwrap();
        // container は元の大小を保持 (= normalize_fs 側で小文字化する責務)
        assert_eq!(container, PathBuf::from(r"E:\Foo.ZIP"));
    }

    // ─── snapshot_key_from_path ──────────────────────────────────

    #[test]
    fn snapshot_key_from_path_normal_file() {
        let key = snapshot_key_from_path(Path::new(r"E:\Photos\IMG.png"));
        assert_eq!(key, SnapshotKey::Fs("e:/photos/img.png".into()));
    }

    #[test]
    fn snapshot_key_from_path_zip_inner() {
        let key = snapshot_key_from_path(Path::new(r"E:\Foo.ZIP\Sub\IMG.png"));
        assert_eq!(
            key,
            SnapshotKey::Archive {
                container: "e:/foo.zip".into(),
                inner: "sub/img.png".into(),
            }
        );
    }

    #[test]
    fn snapshot_key_case_only_difference_collapses_in_hashmap() {
        let mut map: HashMap<SnapshotKey, usize> = HashMap::new();
        map.insert(snapshot_key_from_path(Path::new(r"E:\Foo\Bar.png")), 0);
        // 取り出し時に大文字小文字の差があっても同じ entry
        let hit = map.get(&snapshot_key_from_path(Path::new(r"e:\FOO\bar.PNG")));
        assert_eq!(hit, Some(&0));
    }

    // ─── snapshot_key_from_grid_item ──────────────────────────────────

    #[test]
    fn snapshot_key_from_grid_item_folder() {
        let item = GridItem::Folder(PathBuf::from(r"E:\Photos"));
        let key = snapshot_key_from_grid_item(&item).unwrap();
        assert_eq!(key, SnapshotKey::Fs("e:/photos".into()));
    }

    #[test]
    fn snapshot_key_from_grid_item_zipimage() {
        let item = GridItem::ZipImage {
            zip_path: PathBuf::from(r"E:\Foo.zip"),
            entry_name: "Sub/Image.PNG".into(),
        };
        let key = snapshot_key_from_grid_item(&item).unwrap();
        assert_eq!(
            key,
            SnapshotKey::Archive {
                container: "e:/foo.zip".into(),
                inner: "sub/image.png".into(),
            }
        );
    }

    #[test]
    fn snapshot_key_from_grid_item_pdfpage() {
        let item = GridItem::PdfPage {
            pdf_path: PathBuf::from(r"E:\Doc.pdf"),
            page_num: 7,
            content_type: None,
        };
        let key = snapshot_key_from_grid_item(&item).unwrap();
        assert_eq!(
            key,
            SnapshotKey::Archive {
                container: "e:/doc.pdf".into(),
                inner: "p:7".into(),
            }
        );
    }

    #[test]
    fn snapshot_key_from_grid_item_returns_none_for_separator() {
        let item = GridItem::ZipSeparator {
            dir_display: "Some Title".into(),
        };
        assert!(snapshot_key_from_grid_item(&item).is_none());
    }

    #[test]
    fn snapshot_key_from_grid_item_returns_none_for_search_container() {
        let item = GridItem::SearchContainer {
            path: PathBuf::from(r"E:\Photos"),
            kind: crate::grid_item::SearchContainerKind::Folder,
            hit_count: 5,
            representative: None,
        };
        // MVP では SearchContainer は §4.5 で disable 扱い (= 取り込まない)
        assert!(snapshot_key_from_grid_item(&item).is_none());
    }

    // ─── is_inside_fs (= sibling false positive 防止) ──────────────────

    #[test]
    fn is_inside_fs_true_for_immediate_child() {
        let child = snapshot_key_from_path(Path::new(r"C:\foo\bar.png"));
        assert!(is_inside_fs(&child, "c:/foo"));
    }

    #[test]
    fn is_inside_fs_true_for_nested_child() {
        let child = snapshot_key_from_path(Path::new(r"C:\foo\a\b\c.png"));
        assert!(is_inside_fs(&child, "c:/foo"));
    }

    #[test]
    fn is_inside_fs_false_for_sibling_with_common_prefix() {
        // P1-1 重要観点: `c:/foo` が `c:/foobar/baz` を own しない
        let sibling = snapshot_key_from_path(Path::new(r"C:\foobar\baz.png"));
        assert!(!is_inside_fs(&sibling, "c:/foo"));
    }

    #[test]
    fn is_inside_fs_false_for_exact_match() {
        // 完全一致は inside 扱いしない (= 完全一致 lookup 側で解決)
        let same = snapshot_key_from_path(Path::new(r"C:\foo"));
        assert!(!is_inside_fs(&same, "c:/foo"));
    }

    #[test]
    fn is_inside_fs_true_for_archive_container_inside_folder() {
        // archive entry (= ZipImage / PdfPage) も親 folder の中身として扱う
        let archive = snapshot_key_from_path(Path::new(r"C:\foo\bar.zip\inner.png"));
        assert!(is_inside_fs(&archive, "c:/foo"));
    }

    #[test]
    fn is_inside_fs_false_for_empty_parent() {
        let child = snapshot_key_from_path(Path::new(r"C:\foo\bar.png"));
        // 空 parent は誰も own しない (= guard)
        assert!(!is_inside_fs(&child, ""));
    }

    // ─── SnapshotEntryKind::is_container / is_playable_leaf ──────────

    #[test]
    fn entry_kind_classification() {
        assert!(SnapshotEntryKind::Folder.is_container());
        assert!(SnapshotEntryKind::ZipFile.is_container());
        assert!(SnapshotEntryKind::PdfFile.is_container());
        assert!(SnapshotEntryKind::ConvertibleArchive.is_container());
        assert!(!SnapshotEntryKind::Image.is_container());
        assert!(!SnapshotEntryKind::ZipImage.is_container());

        assert!(SnapshotEntryKind::Image.is_playable_leaf());
        assert!(SnapshotEntryKind::Video.is_playable_leaf());
        assert!(SnapshotEntryKind::ZipImage.is_playable_leaf());
        assert!(SnapshotEntryKind::PdfPage.is_playable_leaf());
        assert!(!SnapshotEntryKind::Folder.is_playable_leaf());
    }

    // ─── FilterState ──────────────────────────────────

    #[test]
    fn filter_state_default_allows_all_ratings() {
        let f = FilterState::default();
        assert_eq!(f.rating_filter, [true; 6]);
    }

    #[test]
    fn filter_state_equality_detects_rating_change() {
        let captured = FilterState::from_rating([true, false, false, false, false, true]); // ★5 のみ
        let current_same = FilterState::from_rating([true, false, false, false, false, true]);
        let current_diff = FilterState::from_rating([true, true, true, true, true, true]); // 全部
        assert_eq!(captured, current_same);
        assert_ne!(captured, current_diff);
    }

    // ─── snapshot_entry_from_grid_item の display 保持 ──────────────────

    #[test]
    fn entry_from_grid_item_preserves_display_path() {
        let item = GridItem::Image(PathBuf::from(r"E:\photos\img.png"));
        let entry = snapshot_entry_from_grid_item(&item).unwrap();
        // display_path() は OS-specific 表現を保つ (= normalize と別軸)
        assert!(entry.display.contains("img.png"));
        assert_eq!(entry.kind, SnapshotEntryKind::Image);
    }
}
