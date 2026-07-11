//! グリッド要素のデータモデル。
//!
//! `GridItem` は一覧に表示される各セルの種別 (フォルダ・画像・動画・ZIP/PDF ファイル・
//! ZIP 内画像・ZIP 内サブディレクトリ境界・PDF ページ) を表す。
//! `ThumbnailState` は各セルのサムネイル読み込み状態。
//!
//! どちらも純粋なデータ型で、UI 状態や I/O は持たない。

use std::borrow::Cow;
use std::path::{Path, PathBuf};

fn path_display_name(path: &Path) -> Cow<'_, str> {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        return Cow::Borrowed(name);
    }
    let raw = path.as_os_str().to_string_lossy();
    let trimmed = raw.trim_end_matches(['\\', '/']);
    if !trimmed.is_empty() {
        let bytes = trimmed.as_bytes();
        if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            Cow::Owned(trimmed.to_ascii_uppercase())
        } else {
            Cow::Owned(trimmed.to_string())
        }
    } else if !raw.is_empty() {
        Cow::Owned(raw.into_owned())
    } else {
        Cow::Borrowed("")
    }
}

#[derive(Clone)]
pub enum GridItem {
    Folder(PathBuf),
    Image(PathBuf),
    Video(PathBuf),
    /// 音声ファイル (mp3 / flac / wav / m4a …)。フルスクリーンで音楽ビュー
    /// (波形タイムライン + スペクトラム、`docs/music-integration-plan.md`) を開いて再生する。
    /// サムネは固定の音楽アイコン (波形サムネは生成しない)。grid_item の各 helper では
    /// **Video と同じ扱い** (実ファイル・レーティング可・ページ補正なし・heavy I/O なし)。
    Audio(PathBuf),
    /// フォルダ一覧に表示される ZIP ファイル (1枚目のサムネイル + バッジ)
    ZipFile(PathBuf),
    /// フォルダ一覧に表示される PDF ファイル (1ページ目のサムネイル + バッジ)
    PdfFile(PathBuf),
    /// RAR / 7z / LZH など「ZIP に変換してから閲覧する」対象のアーカイブ。
    /// クリック時に変換ダイアログを表示し、変換済み ZIP を ZipFile 相当として開く。
    ConvertibleArchive {
        path: PathBuf,
        format: crate::archive_converter::ArchiveFormat,
    },
    /// タスク 3: ZIP ファイル内の画像エントリ
    ZipImage {
        zip_path: PathBuf,
        entry_name: String,
    },
    /// タスク 3: ZIP 内のサブディレクトリ境界を示す擬似アイテム
    /// (1 セル分を占め、作品名など大きな文字で表示される)
    ZipSeparator {
        /// 表示されるディレクトリ名 (ルート直下の場合は "(root)")
        dir_display: String,
    },
    /// ネスト ZIP ツリーナビ (v1.3.0、`docs/nested-zip-tree-plan.md` Strategy A) で、
    /// 開いている外側 ZIP の現在階層にある「入れる子ディレクトリ / 内側アーカイブ」を表す
    /// 1 セル。Enter / ダブルクリックでその階層へ降りる (ナビは `ZipNavState`)。
    /// 実ファイルパスを持たない仮想コンテナなので、ファイル整理 / D&D / チェックの対象外。
    /// レーティング / ピン / 見開き設定は zip_path + prefix の合成キーで扱う。
    ZipDir {
        /// 外側 ZIP の実ファイルパス (= 仮想フォルダのルート identity)。
        zip_path: PathBuf,
        /// この子コンテナの prefix。末尾 '/' 付き ("chapters/" や "chapters/ch01.zip/")。
        dir_prefix: String,
        /// セグメントが `.zip` / `.cbz` で終わるか (バッジ区別用の suffix 推定。
        /// 実アーカイブと同名フォルダは entry_name 文字列だけでは区別不能なので
        /// accepted ambiguity。計画書 §9 参照)。
        is_archive: bool,
        /// 代表サムネに使う部分木の画像 `entry_name` (フルパス、sort 準拠で materialize
        /// が選定)。画像 0 枚の階層では None (アイコン表示にフォールバック)。
        representative: Option<String>,
    },
    /// PDF ファイル内の 1 ページ
    PdfPage {
        pdf_path: PathBuf,
        /// ページ番号 (0-indexed)
        page_num: u32,
        /// ページのコンテンツ種別 (ベクター/ラスター)。
        /// 列挙時は None、フルスクリーンレンダリング完了時に確定。
        content_type: Option<crate::pdf_loader::PdfPageContentType>,
    },
    /// ファイル名 prefix スタック (v2.0.0、`docs/filename-stack-plan.md`) の集約ビューで、
    /// 同一 prefix の画像 2 枚以上を 1 セルに畳んだ「スタック」を表す。代表画像 + 枚数バッジで
    /// 描き、クリックでメンバーグリッドへドリルインする (ZipDir と同じ仮想コンテナ扱い)。
    /// 単独 (1 枚) の画像はスタックにせず通常 `Image` セルとして描く。
    /// 実パスを持たない仮想コンテナなので、ファイル整理 / D&D / チェック / レーティングの
    /// 対象外 (展開後のメンバーは実 `Image` なのでそちらで操作する。MVP は「丸ごと」非対応)。
    Stack {
        /// グループ化キー (prefix)。スタックの identity + 表示名。
        key: String,
        /// 代表画像の実パス (= グループ sort 先頭)。サムネ要求はこの実ファイルを読む
        /// (専用 cache key は不要 — 通常の per-file サムネをそのまま再利用する)。
        representative: PathBuf,
        /// メンバー数 (>= 2、バッジ表示用)。
        count: usize,
    },
    /// Ctrl+G グローバルメタ検索結果の集約コンテナ (v0.8.0)。
    /// トップレベル結果ビューで、ヒットを含む親フォルダ or ZIP を 1 セルで表現する。
    /// クリックでそのコンテナに入ると、drill-down ビューに遷移して階層を維持した
    /// まま絞り込み表示になる (docs/search-expansion-design.md §10.3)。
    SearchContainer {
        /// 親フォルダ or ZIP ファイルの絶対パス
        path: PathBuf,
        /// コンテナ種別
        kind: SearchContainerKind,
        /// ヒット件数 (バッジ表示用)
        hit_count: usize,
        /// 代表サムネ対象 (Option B、v0.8.1)。ヒット内の画像 1 枚を選び、コンテナ
        /// アイコンの代わりにサムネイルとして描画する。None ならアイコン表示のみ。
        representative: Option<ContainerRepresentative>,
    },
}

/// `GridItem::SearchContainer` のコンテナ種別 (v0.8.0)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SearchContainerKind {
    /// 通常フォルダ
    Folder,
    /// ZIP ファイル
    Zip,
}

/// `SearchContainer` の代表サムネ (v0.8.1)。
/// Ctrl+G アグリゲートビューで、コンテナ内のヒットから 1 件をサムネ表示するための参照。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ContainerRepresentative {
    /// 画像ファイル / PDF ファイルの絶対パス。ZIP エントリの場合は ZIP ファイル本体のパス。
    pub path: PathBuf,
    /// ZIP 内エントリ名。通常ファイルなら None。
    pub zip_entry: Option<String>,
    /// PDF ファイルのときの代表ページ番号 (0-indexed)。非 PDF なら None。
    /// ScanSnap 等で PDF だけのフォルダでもサムネが出るようにするため。
    pub pdf_page: Option<u32>,
}

impl GridItem {
    /// 補正プリセット / 消しゴムマスク / タグなど、ページ単位の永続データを持てるアイテムか。
    /// 通常画像 / ZIP 内画像 / PDF ページが対象。フォルダ・動画・ZIP/PDF ファイル本体・
    /// セパレータは対象外。
    ///
    /// レーティング判定とは別軸 — レーティングは [`Self::accepts_rating`] を使う。
    /// 動画はレーティング対象だがピクセル補正対象ではないので、ここには含めない。
    pub fn has_page_data(&self) -> bool {
        matches!(
            self,
            Self::Image(_) | Self::ZipImage { .. } | Self::PdfPage { .. }
        )
    }

    /// 単一ファイル単位でレーティング (★) を持てるアイテムか。
    /// [`Self::has_page_data`] との違いは Video を含むこと: 動画は色調補正は受けないが
    /// レーティングは付与可能。フォルダ/ZIP/PDF 本体はコンテナ扱いで
    /// [`Self::is_container_ratable`] 側。
    pub fn is_rating_leaf(&self) -> bool {
        matches!(
            self,
            Self::Image(_)
                | Self::Video(_)
                | Self::Audio(_)
                | Self::ZipImage { .. }
                | Self::PdfPage { .. }
        )
    }

    /// コンテナ (フォルダ / ZIP ファイル / PDF ファイル / 変換アーカイブ /
    /// ネスト ZIP の本) 自体へのレーティング対象か。一覧画面でコンテナを★絞り込み
    /// できるようにするための判定。
    /// ZipDir (v1.3.0 ツリーナビの本セル) は実パスを持たないが、ピン/見開きと同じ
    /// 合成パスキーでレーティングを持てる (`App::rating_path_key` 参照)。
    pub fn is_container_ratable(&self) -> bool {
        matches!(
            self,
            Self::Folder(_)
                | Self::ZipFile(_)
                | Self::PdfFile(_)
                | Self::ConvertibleArchive { .. }
                | Self::ZipDir { .. }
        )
    }

    /// 代表サムネ生成に本物の同期 I/O を伴うか (heavy_io_queue 振り分け用)。
    /// Folder は `fs::read_dir` 再帰探索、ZipFile / ConvertibleArchive / ZipDir は
    /// ZIP セントラルディレクトリ読み込みや代表解決を伴う。PdfFile は別プロセス IPC
    /// 待ちでメインプロセス内 CPU を消費しないため通常 reload_queue に振る。
    pub fn is_heavy_io(&self) -> bool {
        matches!(
            self,
            Self::Folder(_)
                | Self::ZipFile(_)
                | Self::ConvertibleArchive { .. }
                | Self::ZipDir { .. }
        )
    }

    /// コンテナ系アイテム (Folder / ZipFile / PdfFile / ConvertibleArchive) のパスを返す。
    /// 画像・ページ系や Video / Separator / SearchContainer は対象外で `None`。
    /// 「開く」アクションのナビゲーション先抽出などで使う。
    pub fn container_path(&self) -> Option<&Path> {
        match self {
            Self::Folder(p) | Self::ZipFile(p) | Self::PdfFile(p) => Some(p),
            Self::ConvertibleArchive { path, .. } => Some(path),
            _ => None,
        }
    }

    /// レーティング★を付与できるアイテムかの総合判定。
    /// 単一ファイル (画像 / 動画 / ZIP 内画像 / PDF ページ) とコンテナ (フォルダ / ZIP /
    /// PDF / 変換アーカイブ / ネスト ZIP の本) の両方を含む。補正プリセット等のページ
    /// 専用データとは別物なので区別すること (補正用には [`Self::has_page_data`])。
    pub fn accepts_rating(&self) -> bool {
        self.is_rating_leaf() || self.is_container_ratable()
    }

    /// 表示用の名前を返す。
    /// - 通常: ファイル名
    /// - ZipImage: ZIP 内エントリのベース名
    /// - ZipSeparator: ディレクトリ表示名
    /// - PdfPage: "Page N" (1-indexed)
    pub fn name(&self) -> Cow<'_, str> {
        match self {
            GridItem::Folder(p)
            | GridItem::Image(p)
            | GridItem::Video(p)
            | GridItem::Audio(p)
            | GridItem::ZipFile(p)
            | GridItem::PdfFile(p) => path_display_name(p),
            GridItem::ConvertibleArchive { path, .. } => path_display_name(path),
            GridItem::ZipImage { entry_name, .. } => {
                Cow::Borrowed(crate::zip_loader::entry_basename(entry_name))
            }
            GridItem::ZipSeparator { dir_display } => Cow::Borrowed(dir_display),
            GridItem::ZipDir { dir_prefix, .. } => Cow::Borrowed(zipdir_display_name(dir_prefix)),
            GridItem::PdfPage { page_num, .. } => Cow::Owned(format!("Page {}", page_num + 1)),
            GridItem::SearchContainer { path, .. } => path_display_name(path),
            GridItem::Stack { key, .. } => Cow::Borrowed(key),
        }
    }

    /// 選択情報オーバーレイなどで表示するフルパス文字列。
    /// - 通常ファイル / フォルダ / コンテナ: パスそのまま
    /// - ZipImage: "<zip>:<entry>"
    /// - PdfPage: "<pdf>:Page N" (1-indexed)
    /// - ZipSeparator: ディレクトリ表示名
    pub fn display_path(&self) -> String {
        match self {
            GridItem::Folder(p)
            | GridItem::Image(p)
            | GridItem::Video(p)
            | GridItem::Audio(p)
            | GridItem::ZipFile(p)
            | GridItem::PdfFile(p) => p.display().to_string(),
            GridItem::ConvertibleArchive { path, .. } | GridItem::SearchContainer { path, .. } => {
                path.display().to_string()
            }
            GridItem::ZipImage {
                zip_path,
                entry_name,
            } => format!("{}:{}", zip_path.display(), entry_name),
            GridItem::PdfPage {
                pdf_path, page_num, ..
            } => format!("{}:Page {}", pdf_path.display(), page_num + 1),
            GridItem::ZipSeparator { dir_display } => dir_display.clone(),
            GridItem::ZipDir {
                zip_path,
                dir_prefix,
                ..
            } => format!("{}:{}", zip_path.display(), dir_prefix),
            GridItem::Stack { representative, .. } => representative.display().to_string(),
        }
    }

    /// ファイル整理系の操作のうち、チェック選択で扱う実ファイルのパス。
    ///
    /// 画像・動画・ZIP/PDF 本体・変換前アーカイブはディスク上に実体があるため対象。
    /// **フォルダはチェック対象外** — 単一選択の削除や Shell コピー/カット、D&D では
    /// [`Self::drag_source_path`] を使って扱う。ZIP/PDF 内ページなど仮想フォルダ内アイテムは
    /// 独立した実パスを持たないため対象外。
    pub fn file_operation_path(&self) -> Option<&Path> {
        match self {
            Self::Image(p)
            | Self::Video(p)
            | Self::Audio(p)
            | Self::ZipFile(p)
            | Self::PdfFile(p)
            | Self::ConvertibleArchive { path: p, .. } => Some(p),
            _ => None,
        }
    }

    /// Shell 操作 (D&D / コピー / カット / 単一選択削除) で送出できる実ファイル /
    /// 実フォルダのパス。
    ///
    /// [`Self::file_operation_path`] と同じく、実パスを持つフォルダ / ファイルだけを
    /// 対象にする。対象外:
    /// - `ZipImage` / `PdfPage` — 仮想フォルダ内でディスク上に実体がない
    /// - `ZipSeparator` — 擬似アイテム
    /// - `SearchContainer` — 検索集約 UI のコンテナ。`path` は実フォルダ / ZIP を
    ///   指すが、初版スコープ外 (`docs/file-drag-drop-design.md` §2)。将来含める
    ///   場合はここに 1 分岐足す。
    pub fn drag_source_path(&self) -> Option<&Path> {
        match self {
            Self::Folder(p)
            | Self::Image(p)
            | Self::Video(p)
            | Self::Audio(p)
            | Self::ZipFile(p)
            | Self::PdfFile(p)
            | Self::ConvertibleArchive { path: p, .. } => Some(p),
            _ => None,
        }
    }

    /// チェックボックスで選択できるアイテムか。
    ///
    /// 画像・動画・ZIP/PDF 本体・変換前アーカイブに加えて、ZIP/PDF 内ページも
    /// ページ操作用に対象にする。**フォルダ** (チェック対象外)・ZIP セパレータ・
    /// 検索集約コンテナは対象外。
    pub fn is_checkable(&self) -> bool {
        self.file_operation_path().is_some()
            || matches!(self, Self::ZipImage { .. } | Self::PdfPage { .. })
    }

    /// パフォーマンス計装用の相関キー文字列を返す。
    /// `perf::event` の `key` に渡すことで、解析ツールが同一画像に関する
    /// 一連のイベントを一意に紐付けられる。
    pub fn perf_key(&self) -> String {
        match self {
            GridItem::Folder(p) => format!("dir::{}", p.display()),
            GridItem::Image(p) | GridItem::Video(p) | GridItem::Audio(p) => {
                format!("{}", p.display())
            }
            GridItem::ZipFile(p) => format!("zipfile::{}", p.display()),
            GridItem::PdfFile(p) => format!("pdffile::{}", p.display()),
            GridItem::ZipImage {
                zip_path,
                entry_name,
            } => {
                format!("zip::{}#{}", zip_path.display(), entry_name)
            }
            GridItem::ZipSeparator { dir_display } => {
                format!("zipsep::{dir_display}")
            }
            GridItem::ZipDir {
                zip_path,
                dir_prefix,
                ..
            } => {
                format!("zipdir::{}#{}", zip_path.display(), dir_prefix)
            }
            GridItem::PdfPage {
                pdf_path, page_num, ..
            } => pdf_page_perf_key(pdf_path, *page_num),
            GridItem::ConvertibleArchive { path, format } => {
                format!("archive::{}::{}", format.label(), path.display())
            }
            GridItem::SearchContainer { path, kind, .. } => {
                let prefix = match kind {
                    SearchContainerKind::Folder => "searchdir",
                    SearchContainerKind::Zip => "searchzip",
                };
                format!("{prefix}::{}", path.display())
            }
            GridItem::Stack { representative, .. } => {
                format!("stack::{}", representative.display())
            }
        }
    }
}

/// `ZipDir` の `dir_prefix` ("chapters/ch01.zip/") から表示名 (最後のセグメント、
/// 例 "ch01.zip") を取り出す。末尾 '/' を除去してから最後の '/' 以降を返す。
/// ルート ("") の場合は空文字列 (通常 ZipDir はルートを指さない)。
pub fn zipdir_display_name(dir_prefix: &str) -> &str {
    let trimmed = dir_prefix.strip_suffix('/').unwrap_or(dir_prefix);
    match trimmed.rfind('/') {
        Some(pos) => &trimmed[pos + 1..],
        None => trimmed,
    }
}

/// `ZipDir` 代表サムネのカタログキー。`zipdir:` プレフィックスで通常の `entry_name`
/// キー (ZIP 内パス) と衝突しない (entry_name に ':' を含む ZIP は稀)。additive。
pub fn zipdir_cache_key(dir_prefix: &str) -> String {
    format!("zipdir:{dir_prefix}")
}

/// PDF ファイル (フォルダ一覧上) 用の perf 相関キー。
/// `pdf_loader` など GridItem を直接持たない箇所から同じ形式のキーを作るために使う。
pub fn pdf_file_perf_key(pdf_path: &Path) -> String {
    format!("pdffile::{}", pdf_path.display())
}

/// PDF ページ用の perf 相関キー。
pub fn pdf_page_perf_key(pdf_path: &Path, page_num: u32) -> String {
    format!("pdf::{}#{}", pdf_path.display(), page_num)
}

/// PDF ページのカタログキーを生成する。
/// サムネイルキャッシュの保存・参照で一致させるため、全箇所でこの関数を使うこと。
pub fn pdf_page_cache_key(page_num: u32) -> String {
    format!("page_{:04}", page_num)
}

/// サムネイルセルの読み込み状態。
///
/// `Clone` を実装しているのは、Ctrl+G の検索結果ストリーミング rebuild で
/// 同一パスのサムネを使い回してテクスチャ再アップロードによるちらつきを防ぐため。
/// `egui::TextureHandle` は内部 Arc なのでクローンは refcount inc だけ。
#[derive(Clone)]
pub enum ThumbnailState {
    /// まだロードされていない
    Pending,
    /// 読み込み済みで GPU テクスチャとして保持中
    ///
    /// `from_cache = true` の場合は WebP キャッシュ (q=75) から復元した状態で、
    /// 段階 E のアイドル時アップグレードで元画像から再デコードされる対象になる。
    /// `rendered_at_px` は生成時の長辺ピクセル数で、現在のセルサイズと比較して
    /// 著しく小さい場合 (列数変更後など) もアップグレード対象になる。
    /// `source_dims` は元画像のピクセル寸法 (旧カタログ由来は None)。
    Loaded {
        tex: egui::TextureHandle,
        from_cache: bool,
        rendered_at_px: u32,
        source_dims: Option<(u32, u32)>,
    },
    /// 読み込みに失敗した（再試行しない）
    Failed,
    /// 段階 B: 先読み範囲外に出て GPU テクスチャを破棄済み
    /// 再び範囲内に入ったら再ロードされる
    Evicted,
}

/// グリッド上段のフォルダ系ブロック (Folder / ZipFile / PdfFile / ConvertibleArchive 等)
/// を `SortOrder` に従って並べ替える。`folder_metas` は同インデックスの
/// `Some((mtime_secs, size))` (取得失敗時 `None`)。
///
/// `App::load_folder_inner` から呼ばれる本番経路。テストからも公開 API として直接呼んで
/// 「フォルダ系も sort_order に従う」「同 mtime は名前で安定」等の不変条件を検証する。
pub fn sort_folder_block(
    folders: &mut Vec<GridItem>,
    folder_metas: &mut Vec<Option<(i64, i64)>>,
    sort: crate::settings::SortOrder,
) {
    // pub fn の契約違反 (folders と folder_metas の長さ不一致) は release でも止める。
    // zip は短い方に合わせるので silently drop されると並びが壊れる。
    assert_eq!(folders.len(), folder_metas.len());
    let mut paired: Vec<_> = folders
        .drain(..)
        .zip(folder_metas.drain(..))
        .map(|(item, meta)| {
            let key = sort.name_key(&item.name());
            (item, meta, key)
        })
        .collect();
    paired.sort_by(|(_, ma, ak), (_, mb, bk)| {
        let a_mt = ma.map(|(mt, _)| mt).unwrap_or(0);
        let b_mt = mb.map(|(mt, _)| mt).unwrap_or(0);
        sort.compare_name_keys(ak, a_mt, bk, b_mt)
    });
    for (f, m, _) in paired {
        folders.push(f);
        folder_metas.push(m);
    }
}

/// 設定された 4 行のカテゴリ割り当てで items と同位置メタデータを並べ直す。
///
/// 各行は [`sort_folder_block`] で同じ `SortOrder` を適用する。`sort == None` は
/// レーティング設定時刻順など、呼び出し側が既に作った行内順序を安定保持する集約ビュー用。
/// 空行は出力を持たないため自動的に読み飛ばされる。
pub fn arrange_grid_items(
    items: &mut Vec<GridItem>,
    image_metas: &mut Vec<Option<(i64, i64)>>,
    display_order: &crate::settings::GridDisplayOrder,
    sort: Option<crate::settings::SortOrder>,
) {
    assert_eq!(items.len(), image_metas.len());
    let display_order = display_order.normalized();
    let mut row_items: [Vec<GridItem>; 4] = std::array::from_fn(|_| Vec::new());
    let mut row_metas: [Vec<Option<(i64, i64)>>; 4] = std::array::from_fn(|_| Vec::new());
    let mut other_items = Vec::new();
    let mut other_metas = Vec::new();

    for (item, meta) in items.drain(..).zip(image_metas.drain(..)) {
        if let Some(kind) = display_kind(&item) {
            let row = display_order.row_for(kind);
            row_items[row].push(item);
            row_metas[row].push(meta);
        } else {
            other_items.push(item);
            other_metas.push(meta);
        }
    }

    for row in 0..4 {
        if let Some(sort) = sort {
            sort_folder_block(&mut row_items[row], &mut row_metas[row], sort);
        }
        items.append(&mut row_items[row]);
        image_metas.append(&mut row_metas[row]);
    }
    // §1.6 の 4 カテゴリ外にある検索専用/レガシー疑似セルは、欠落させず末尾へ保つ。
    items.append(&mut other_items);
    image_metas.append(&mut other_metas);
}

fn display_kind(item: &GridItem) -> Option<crate::settings::GridItemDisplayKind> {
    use crate::settings::GridItemDisplayKind;
    match item {
        GridItem::Folder(_) => Some(GridItemDisplayKind::Folder),
        GridItem::ZipFile(_)
        | GridItem::PdfFile(_)
        | GridItem::ConvertibleArchive { .. }
        | GridItem::ZipDir { .. } => Some(GridItemDisplayKind::Archive),
        GridItem::Image(_)
        | GridItem::ZipImage { .. }
        | GridItem::PdfPage { .. }
        | GridItem::Stack { .. } => Some(GridItemDisplayKind::Image),
        GridItem::Video(_) | GridItem::Audio(_) => Some(GridItemDisplayKind::VideoAudio),
        GridItem::ZipSeparator { .. } | GridItem::SearchContainer { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_name() {
        let item = GridItem::Folder(PathBuf::from(r"C:\foo\bar"));
        assert_eq!(item.name(), "bar");
    }

    #[test]
    fn image_name() {
        let item = GridItem::Image(PathBuf::from(r"D:\photos\sunset.jpg"));
        assert_eq!(item.name(), "sunset.jpg");
    }

    #[test]
    fn drive_root_name_uses_drive_label() {
        let item = GridItem::Folder(PathBuf::from(r"c:\"));
        assert_eq!(item.name(), "C:");
    }

    #[test]
    fn video_name() {
        let item = GridItem::Video(PathBuf::from(r"E:\videos\clip.mp4"));
        assert_eq!(item.name(), "clip.mp4");
    }

    #[test]
    fn audio_name() {
        let item = GridItem::Audio(PathBuf::from(r"E:\music\track.mp3"));
        assert_eq!(item.name(), "track.mp3");
    }

    #[test]
    fn audio_behaves_like_video_leaf() {
        // 音声は grid_item の helper では Video と同じ扱い: 実ファイル・レーティング可・
        // ページ補正データなし・コンテナでない。
        let item = GridItem::Audio(PathBuf::from(r"E:\music\track.flac"));
        assert!(item.is_rating_leaf());
        assert!(item.accepts_rating());
        assert!(!item.has_page_data());
        assert!(!item.is_container_ratable());
        assert!(!item.is_heavy_io());
        assert!(item.is_checkable());
        assert!(item.file_operation_path().is_some());
        assert!(item.drag_source_path().is_some());
        assert!(item.container_path().is_none());
        assert_eq!(item.perf_key(), r"E:\music\track.flac");
    }

    #[test]
    fn zip_image_name() {
        let item = GridItem::ZipImage {
            zip_path: PathBuf::from(r"C:\archive.zip"),
            entry_name: "chapter1/page01.jpg".to_string(),
        };
        assert_eq!(item.name(), "page01.jpg");
    }

    #[test]
    fn zip_separator_name() {
        let item = GridItem::ZipSeparator {
            dir_display: "Chapter 1".to_string(),
        };
        assert_eq!(item.name(), "Chapter 1");
    }

    fn zipdir(prefix: &str, is_archive: bool) -> GridItem {
        GridItem::ZipDir {
            zip_path: PathBuf::from(r"C:\books\vol.zip"),
            dir_prefix: prefix.to_string(),
            is_archive,
            representative: None,
        }
    }

    #[test]
    fn zipdir_display_name_helper() {
        assert_eq!(zipdir_display_name("chapters/ch01.zip/"), "ch01.zip");
        assert_eq!(zipdir_display_name("chapters/"), "chapters");
        assert_eq!(zipdir_display_name("a/b/c/"), "c");
        // 末尾 '/' なしでも動く
        assert_eq!(zipdir_display_name("chapters/ch01.zip"), "ch01.zip");
        assert_eq!(zipdir_display_name(""), "");
    }

    #[test]
    fn zipdir_name_is_last_segment() {
        assert_eq!(zipdir(r"chapters/ch01.zip/", true).name(), "ch01.zip");
        assert_eq!(zipdir(r"chapters/", false).name(), "chapters");
    }

    #[test]
    fn zipdir_display_path_and_perf_key() {
        let item = zipdir("chapters/ch01.zip/", true);
        assert_eq!(item.display_path(), r"C:\books\vol.zip:chapters/ch01.zip/");
        assert_eq!(
            item.perf_key(),
            r"zipdir::C:\books\vol.zip#chapters/ch01.zip/"
        );
    }

    #[test]
    fn zipdir_is_virtual_non_actionable_container() {
        let item = zipdir("chapters/ch01.zip/", true);
        // 仮想コンテナ: 実パス系 helper は全て None / false。
        assert!(item.container_path().is_none());
        assert!(item.file_operation_path().is_none());
        assert!(item.drag_source_path().is_none());
        assert!(!item.is_checkable());
        assert!(!item.has_page_data());
        assert!(!item.is_rating_leaf());
        // コンテナレーティングは対象 (実機フィードバック: 本にも★を付けたい)。
        // キーは実パスではなく合成パス (App::rating_path_key の ZipDir arm)。
        assert!(item.is_container_ratable());
        assert!(item.accepts_rating());
        // サムネ代表解決で ZIP 列挙が走ることがあるため、通常画像/PDF の
        // regular queue を塞がないよう heavy I/O queue に振る。
        assert!(item.is_heavy_io());
    }

    #[test]
    fn convertible_archive_is_ratable_container() {
        let item = GridItem::ConvertibleArchive {
            path: PathBuf::from(r"C:\books\a.rar"),
            format: crate::archive_converter::ArchiveFormat::Rar,
        };

        assert!(item.is_container_ratable());
        assert!(item.accepts_rating());
        assert!(!item.is_rating_leaf());
    }

    #[test]
    fn zipdir_cache_key_is_prefixed() {
        assert_eq!(
            zipdir_cache_key("chapters/ch01.zip/"),
            "zipdir:chapters/ch01.zip/"
        );
    }

    #[test]
    fn name_root_path() {
        // ルートパスの場合もドライブ名を一覧表示名として使う。
        let item = GridItem::Folder(PathBuf::from(r"C:\"));
        assert_eq!(item.name(), "C:");
    }

    #[test]
    fn checkable_includes_real_files_except_folders() {
        // フォルダは単一選択の Shell 操作で扱い、複数チェック対象にはしない。
        assert!(!GridItem::Folder(PathBuf::from(r"C:\books")).is_checkable());
        assert!(GridItem::Image(PathBuf::from(r"C:\books\a.jpg")).is_checkable());
        assert!(GridItem::Video(PathBuf::from(r"C:\books\a.mp4")).is_checkable());
        assert!(GridItem::ZipFile(PathBuf::from(r"C:\books\a.zip")).is_checkable());
        assert!(GridItem::PdfFile(PathBuf::from(r"C:\books\a.pdf")).is_checkable());
        assert!(
            GridItem::ConvertibleArchive {
                path: PathBuf::from(r"C:\books\a.7z"),
                format: crate::archive_converter::ArchiveFormat::SevenZ,
            }
            .is_checkable()
        );
        assert!(
            GridItem::ZipImage {
                zip_path: PathBuf::from(r"C:\books\a.zip"),
                entry_name: "p001.jpg".to_owned(),
            }
            .is_checkable()
        );
        assert!(
            GridItem::PdfPage {
                pdf_path: PathBuf::from(r"C:\books\a.pdf"),
                page_num: 0,
                content_type: None,
            }
            .is_checkable()
        );
        assert!(
            !GridItem::ZipSeparator {
                dir_display: "chapter".to_owned(),
            }
            .is_checkable()
        );
        assert!(
            !GridItem::SearchContainer {
                path: PathBuf::from(r"C:\books"),
                kind: SearchContainerKind::Folder,
                hit_count: 3,
                representative: None,
            }
            .is_checkable()
        );
    }

    #[test]
    fn file_operation_path_is_only_for_movable_files() {
        // フォルダは複数チェックの file_operation_path ではなく drag_source_path で扱う。
        assert!(
            GridItem::Folder(PathBuf::from(r"C:\books"))
                .file_operation_path()
                .is_none()
        );
        assert!(
            GridItem::ZipImage {
                zip_path: PathBuf::from(r"C:\books\a.zip"),
                entry_name: "p001.jpg".to_owned(),
            }
            .file_operation_path()
            .is_none()
        );
        assert!(
            GridItem::PdfPage {
                pdf_path: PathBuf::from(r"C:\books\a.pdf"),
                page_num: 0,
                content_type: None,
            }
            .file_operation_path()
            .is_none()
        );

        let zip = GridItem::ZipFile(PathBuf::from(r"C:\books\a.zip"));
        assert_eq!(
            zip.file_operation_path()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str()),
            Some("a.zip")
        );
        let pdf = GridItem::PdfFile(PathBuf::from(r"C:\books\a.pdf"));
        assert_eq!(
            pdf.file_operation_path()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str()),
            Some("a.pdf")
        );
    }

    #[test]
    fn drag_source_path_includes_folders_and_real_files() {
        // 実フォルダ / 実ファイルは Some。
        assert!(
            GridItem::Folder(PathBuf::from(r"C:\books"))
                .drag_source_path()
                .is_some()
        );
        assert!(
            GridItem::Image(PathBuf::from(r"C:\books\a.jpg"))
                .drag_source_path()
                .is_some()
        );
        assert!(
            GridItem::Video(PathBuf::from(r"C:\books\a.mp4"))
                .drag_source_path()
                .is_some()
        );
        assert!(
            GridItem::ZipFile(PathBuf::from(r"C:\books\a.zip"))
                .drag_source_path()
                .is_some()
        );
        assert!(
            GridItem::PdfFile(PathBuf::from(r"C:\books\a.pdf"))
                .drag_source_path()
                .is_some()
        );
        assert!(
            GridItem::ConvertibleArchive {
                path: PathBuf::from(r"C:\books\a.7z"),
                format: crate::archive_converter::ArchiveFormat::SevenZ,
            }
            .drag_source_path()
            .is_some()
        );
    }

    #[test]
    fn drag_source_path_excludes_virtual_and_pseudo_items() {
        // 仮想フォルダ内 / 擬似アイテム / 検索集約コンテナは None。
        assert!(
            GridItem::ZipImage {
                zip_path: PathBuf::from(r"C:\books\a.zip"),
                entry_name: "p001.jpg".to_owned(),
            }
            .drag_source_path()
            .is_none()
        );
        assert!(
            GridItem::PdfPage {
                pdf_path: PathBuf::from(r"C:\books\a.pdf"),
                page_num: 0,
                content_type: None,
            }
            .drag_source_path()
            .is_none()
        );
        assert!(
            GridItem::ZipSeparator {
                dir_display: "chapter".to_owned(),
            }
            .drag_source_path()
            .is_none()
        );
        // SearchContainer は path を持つが初版スコープ外 (docs §2)。
        assert!(
            GridItem::SearchContainer {
                path: PathBuf::from(r"C:\books"),
                kind: SearchContainerKind::Folder,
                hit_count: 3,
                representative: None,
            }
            .drag_source_path()
            .is_none()
        );
    }

    fn arranged_names(
        mut items: Vec<GridItem>,
        order: crate::settings::GridDisplayOrder,
    ) -> Vec<String> {
        let mut metas = vec![Some((0, 0)); items.len()];
        arrange_grid_items(
            &mut items,
            &mut metas,
            &order,
            Some(crate::settings::SortOrder::FileName),
        );
        items.iter().map(|item| item.name().into_owned()).collect()
    }

    fn mixed_outer_items() -> Vec<GridItem> {
        vec![
            GridItem::Image(PathBuf::from(r"C:\grid\z.jpg")),
            GridItem::Folder(PathBuf::from(r"C:\grid\b-folder")),
            GridItem::Video(PathBuf::from(r"C:\grid\c.mp4")),
            GridItem::ZipFile(PathBuf::from(r"C:\grid\a.zip")),
        ]
    }

    #[test]
    fn arrange_default_reproduces_container_then_media_blocks() {
        assert_eq!(
            arranged_names(
                mixed_outer_items(),
                crate::settings::GridDisplayOrder::default()
            ),
            ["a.zip", "b-folder", "c.mp4", "z.jpg"]
        );
    }

    #[test]
    fn arrange_supports_folder_first_and_folder_last() {
        use crate::settings::GridItemDisplayKind::{Archive, Folder, Image, VideoAudio};
        let folder_first = crate::settings::GridDisplayOrder::from_rows([
            vec![Folder],
            vec![Archive, Image, VideoAudio],
            vec![],
            vec![],
        ]);
        let folder_last = crate::settings::GridDisplayOrder::from_rows([
            vec![Archive, Image, VideoAudio],
            vec![],
            vec![],
            vec![Folder],
        ]);
        assert_eq!(
            arranged_names(mixed_outer_items(), folder_first),
            ["b-folder", "a.zip", "c.mp4", "z.jpg"]
        );
        assert_eq!(
            arranged_names(mixed_outer_items(), folder_last),
            ["a.zip", "c.mp4", "z.jpg", "b-folder"]
        );
    }

    #[test]
    fn arrange_merges_archives_into_media_row() {
        use crate::settings::GridItemDisplayKind::{Archive, Folder, Image, VideoAudio};
        let order = crate::settings::GridDisplayOrder::from_rows([
            vec![Folder],
            vec![Archive, Image, VideoAudio],
            vec![],
            vec![],
        ]);
        assert_eq!(
            arranged_names(
                vec![
                    GridItem::Image(PathBuf::from(r"C:\grid\a.jpg")),
                    GridItem::ZipFile(PathBuf::from(r"C:\grid\b.zip")),
                    GridItem::Video(PathBuf::from(r"C:\grid\c.mp4")),
                ],
                order,
            ),
            ["a.jpg", "b.zip", "c.mp4"]
        );
    }

    #[test]
    fn arrange_skips_empty_rows() {
        use crate::settings::GridItemDisplayKind::{Archive, Folder, Image, VideoAudio};
        let order = crate::settings::GridDisplayOrder::from_rows([
            vec![],
            vec![Image],
            vec![],
            vec![Folder, Archive, VideoAudio],
        ]);
        assert_eq!(
            arranged_names(mixed_outer_items(), order),
            ["z.jpg", "a.zip", "b-folder", "c.mp4"]
        );
    }
}
