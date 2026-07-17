//! サムネイル生成ワーカーが使う型と関数群。
//!
//! - `LoadRequest`: UI スレッドが永続ワーカーに送る要求
//! - `CacheDecision`: Settings から派生する保存判定
//! - `process_load_request` / `load_one_cached`: 1 件ずつ処理する本体
//! - `build_and_save_one`: キャッシュ作成ダイアログから使う非対話版
//! - `compute_display_px`, `resize_to_display_color_image`: 表示用 ColorImage 生成
//!
//! どの関数も `App` 状態を直接触らない。スレッド境界を越えて使われるため、
//! 引数で必要な情報をすべて受け取る純粋な関数として設計されている。

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};

// -----------------------------------------------------------------------
// ワーカーキュー優先度
// -----------------------------------------------------------------------

/// ワーカーキューの優先度キー。可視範囲を最優先、先読みは距離順 (近い方から)、
/// 同距離では forward が先。サムネイルワーカー本体と bench で共有する。
/// 返り値は `(tier, distance, direction)` の tuple で、辞書順で小さいほど優先。
pub fn worker_priority_key(
    priority: bool,
    idx: usize,
    vis: usize,
    vis_end: usize,
) -> (usize, usize, usize) {
    if priority {
        let d = if idx < vis { vis - idx } else { idx - vis };
        (0, d, 0)
    } else if idx >= vis_end {
        (1, idx - vis_end + 1, 0)
    } else {
        (1, vis.saturating_sub(idx), 1)
    }
}

// -----------------------------------------------------------------------
// キャッシュキー定数 (app.rs / ベンチマーク bin から参照)
// -----------------------------------------------------------------------

/// カタログ内の ZipFile サムネイル用キャッシュキープレフィックス
pub const CACHE_KEY_ZIP: &str = "zipthumb:";
/// カタログ内の PdfFile サムネイル用キャッシュキープレフィックス
pub const CACHE_KEY_PDF: &str = "pdfthumb:";
/// カタログ内の変換済みアーカイブ (RAR/7z/LZH) サムネイル用キャッシュキープレフィックス
pub const CACHE_KEY_ARCHIVE: &str = "archivethumb:";
/// カタログ内のフォルダサムネイル用キャッシュキープレフィックス
pub const CACHE_KEY_FOLDER: &str = "folderthumb:";
/// フォルダ代表サムネの自動選定アルゴリズム世代。
///
/// cache key に含めることで、番号順などの選定ロジックを変えたときだけ古い代表
/// サムネを避けて再スキャンする。フォルダ内容の変更を毎回検査する目的ではない。
/// 旧 `folderthumb:{dirname}` 形式を v1 相当とみなし、明示版は v2 から始める。
pub const FOLDER_THUMB_AUTO_ALGO_VERSION: u32 = 2;

/// 親コンテナ (フォルダ / ZIP / PDF) に手動ピンが付いているときに、cache key の
/// 後ろに `#pin:` 区切りで pin の identity を埋め込む (docs/virtual-folders.md §3.1)。
/// pin の付け替え / target 変更で自然に key が変わって古い WebP を catch しない。
pub const CACHE_KEY_PIN_SUFFIX: &str = "#pin:";

/// pin 解決時の dispatch 戦略。
///
/// 通常 (pin なし) は `LoadRequest::cache_key_override` の prefix で is_folder_thumb /
/// is_zip_thumb を判定するが、pin がある場合は cache key を親 (folderthumb 等) に
/// 揃えるので prefix では target 種別が分からない。`LoadRequest::resolve_override` を
/// `Some` にしてここで明示する。
///
/// PdfFirstPage / PdfPage / ZipEntry は `pdf_page` / `zip_entry` フィールドで
/// 暗黙に dispatch されるので、ここには載せない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveStrategy {
    /// `req.path` を直接画像として decode
    DirectImage,
    /// `req.path` はフォルダ。`resolve_folder_thumb_image` で代表画像を選ぶ
    FolderRepresentative,
    /// `req.path` は ZIP。`zip_loader::read_first_image_bytes` で先頭画像を取り出す
    ZipFirstImage,
    /// `req.path` は ZIP。`LoadRequest::zip_dir_prefix` の部分木代表画像を取り出す
    ZipDirRepresentative,
}

/// Drive-list thumbnails are cache-only: the UI thread checks the local pin DB
/// and asks the worker to use an already-cataloged pinned thumbnail. Missing
/// cache falls back to the fixed drive icon instead of touching the drive.
pub struct PinnedOnlyRequest {
    pub cache_key_prefix: String,
}
/// Ctrl+G アグリゲートビューの「代表サムネ」用キャッシュキープレフィックス (v0.8.1)。
/// filename 単体だと別コンテナ同士の同名画像 (例: `cover.jpg`) でキャッシュ衝突し、
/// placeholder mtime=0 で相手の thumb を読み込んでしまうため、**コンテナ path 丸ごと**
/// をキーに含める。通常フォルダ閲覧とは別空間なので互いを上書きしない。
pub const CACHE_KEY_SEARCH_REP: &str = "searchrep:";

/// `LoadRequest` から catalog の検索キー (filename) を取り出す。
///
/// 優先順位:
/// 1. `cache_key_override` (フォルダ代表 / ZipFile / PdfFile / pin など)
/// 2. `pdf_page` → `pdf_page_cache_key(page_num)`
/// 3. `zip_entry`
/// 4. fallback: `req.path.file_name()`
///
/// `None` を返すのは `req.path` から file_name が取れない異常ケース。
/// ワーカー本体は空文字 fallback で動くが、`auto_aspect` の seed フェーズは
/// `None` を skip 対象として使うため Option で返している。
///
/// 詳細: [docs/auto-thumb-aspect-plan.md §4.1.2](../../docs/auto-thumb-aspect-plan.md)
pub fn cache_key_for_request(req: &LoadRequest) -> Option<std::borrow::Cow<'_, str>> {
    if let Some(ref key) = req.cache_key_override {
        Some(std::borrow::Cow::Borrowed(key.as_str()))
    } else if let Some(page_num) = req.pdf_page {
        Some(std::borrow::Cow::Owned(
            crate::grid_item::pdf_page_cache_key(page_num),
        ))
    } else if let Some(ref name) = req.zip_entry {
        Some(std::borrow::Cow::Borrowed(name.as_str()))
    } else {
        req.path
            .file_name()
            .and_then(|n| n.to_str())
            .map(std::borrow::Cow::Borrowed)
    }
}

/// フォルダ代表サムネの自動選定用 cache key を組み立てる。
///
/// `identity` はフォルダ名または full path。ソート種別・探索深度・アルゴリズム世代を
/// key に含め、設定やロジックが変わったときだけ古い自動代表サムネを読まないようにする。
pub fn folder_thumb_auto_cache_key(
    identity: &str,
    sort: crate::settings::SortOrder,
    depth: u32,
) -> String {
    let sort_token = match sort {
        crate::settings::SortOrder::FileName => "name",
        crate::settings::SortOrder::Numeric => "numeric",
        crate::settings::SortOrder::DateAsc => "date-asc",
        crate::settings::SortOrder::DateDesc => "date-desc",
    };
    format!(
        "{CACHE_KEY_FOLDER}auto-v{FOLDER_THUMB_AUTO_ALGO_VERSION}:{sort_token}:d{depth}:{identity}"
    )
}

// -----------------------------------------------------------------------
// 共通型
// -----------------------------------------------------------------------

/// 編集プレビュー由来サムネイルの、色調補正用に分離した下地と注釈。
pub struct ThumbEditPreviewAdjustment {
    pub base: egui::ColorImage,
    pub annotation_layers: Vec<crate::edit_preview_cache::CachedAnnotationLayer>,
}

/// サムネイル読み込み結果メッセージ。
///
/// ワーカースレッドが UI スレッドに送る。フィールドを位置に頼らず名前で判別できる
/// ように struct で保持している (`bool` が隣接するため tuple だと取り違えやすい)。
pub struct ThumbMsg {
    pub idx: usize,
    /// デコード成功時のピクセル。キャンセル / 失敗時は None。
    pub image: Option<egui::ColorImage>,
    /// true: WebP キャッシュから復元 (段階 E アップグレード対象)。
    /// false: 元画像から直接デコード (高画質) または動画 Shell API。
    pub from_cache: bool,
    /// true: 非破壊編集結果のプレビューキャッシュから復元。
    pub from_edit_preview: bool,
    /// `from_edit_preview` のとき、色調補正を下地だけへ掛けてから注釈を戻すためのデータ。
    pub edit_preview_adjustment: Option<ThumbEditPreviewAdjustment>,
    /// 元画像のピクセル寸法 (幅, 高さ)。取得できなかった場合は None。
    pub source_dims: Option<(u32, u32)>,
    /// ワーカーがロードを中断した場合 true (STALE: keep_range 外になった等)。
    /// `image` は必ず `None`。UI 側は `thumbnails[idx]` を `Evicted` に戻し、
    /// `requested` からも削除して **再試行可能** な状態にする (`Failed` にはしない)。
    pub canceled: bool,
    /// **第2シグナル**: デコード成功後にキャッシュ保存判定 (or 保存スキップ) が完了して
    /// `requested` から idx を抜くだけの通知 (`canceled` と排他、両方 true にはしない)。
    /// UI 側は `requested.remove(&idx)` のみ行い、**`thumbnails[idx]` の状態は変更しない**。
    ///
    /// 理由: 第1シグナル (image=Some) が `texture_backlog` に積まれて Pending のまま
    /// アップロード待ちになっているケースで、第2シグナルが Pending を Evicted に
    /// 書き換えると、次フレームに同じセルが再エンキュー → 重複デコード地獄になる。
    pub finalized: bool,
    /// エンキュー時の `LoadRequest::input_seq` を透過する。perf ログで enqueue /
    /// decode / ready を相関付けるのに使う。計装無効時や未設定時は 0。
    pub input_seq: u64,
    /// エンキュー時の `LoadRequest::items_gen` を透過する。UI 側は自分の
    /// `items_generation` と一致しないメッセージを破棄する (世代分離)。
    pub items_gen: u64,
}

/// 段階 B: サムネイル読み込み要求。
///
/// UI スレッドが `reload_queue` に push し、永続ワーカースレッドが pop して処理する。
/// ワーカーはまず `cache_map` を参照し、ヒットすれば WebP デコード、
/// ミスすれば `load_one_cached` に委譲する。
#[derive(Default)]
pub struct LoadRequest {
    pub idx: usize,
    /// 通常画像ならファイルパス、ZIP 画像なら ZIP ファイルのパス
    pub path: std::path::PathBuf,
    pub mtime: i64,
    pub file_size: i64,
    /// 非破壊編集プレビューのページキー。編集済み画像系アイテムだけに設定する。
    pub edit_preview_key: Option<String>,
    /// 段階 E: true の場合はキャッシュを無視して元画像から再デコードする
    pub skip_cache: bool,
    /// true = 画面上に見えている可視範囲のアイテム。ワーカーは priority 要求を
    /// 先読み要求より常に先に処理する。
    pub priority: bool,
    /// タスク 3: `Some(name)` なら ZIP エントリとして読む。
    /// `path` が ZIP ファイル、`name` が内部エントリ名。
    pub zip_entry: Option<String>,
    /// ZIP 内仮想ディレクトリ代表を遅延解決するときの prefix ("a/b/", root は空)。
    pub zip_dir_prefix: Option<String>,
    /// `Some(page_num)` なら PDF ページとしてレンダリングする。
    /// `path` が PDF ファイル、`page_num` が 0-indexed ページ番号。
    pub pdf_page: Option<u32>,
    /// PDF パスワード (パスワード付き PDF 用)
    pub pdf_password: Option<String>,
    /// フォルダ一覧の ZipFile/PdfFile 用: カタログキーを上書き。
    /// None の場合はファイル名 / エントリ名 / ページキーから自動生成。
    pub cache_key_override: Option<String>,
    /// フォルダサムネイル用: フォルダ内の画像を選ぶソート順
    pub folder_thumb_sort: Option<crate::settings::SortOrder>,
    /// フォルダサムネイル用: サブフォルダを探索する最大階層数
    pub folder_thumb_depth: u32,
    /// pin 解決時の dispatch 上書き。`Some` のときは `cache_key_override` の prefix
    /// 判定 (is_folder_thumb / is_zip_thumb) を無視してここで指定された戦略で
    /// resolve する。`None` のときは従来通り prefix で判定する。
    pub resolve_override: Option<ResolveStrategy>,
    /// 明示ピンだけを解決する特殊要求。未解決 / Folder leaf はアイコン fallback
    /// に倒し、通常フォルダ代表探索には進ませない。
    pub pinned_only: Option<PinnedOnlyRequest>,
    /// CachePolicy に関係なく、このリクエストの結果を catalog に保存する。
    /// ドライブ直下フォルダの明示ピン代表など、後段の cache-only 表示がユーザーの
    /// 明示操作に依存する場合だけ UI 側で true にする。
    pub force_cache: bool,
    /// パフォーマンス計装用: エンキュー時の input_seq (相関キー)。
    /// 0 は未設定を意味する。`--perf-log` 無効時は使われない。
    pub input_seq: u64,
    /// items 世代番号: `App::items_generation` のスナップショット。
    /// ワーカーは ThumbMsg にエコーバックし、UI 側は現行世代と一致しないメッセージを破棄する。
    pub items_gen: u64,
    /// PDF render pool の context epoch。**UI スレッドの enqueue 時点**で
    /// `pdf_loader::current_render_context_epoch()` を焼き付ける (TOCTOU 防止)。
    /// 0 = epoch チェック対象外 (background 経路の sentinel)。`Default::default()` は 0。
    pub context_epoch: u64,
}

/// キャッシュ生成判定用のパラメータ（段階 C）。
///
/// Settings から必要なフィールドのみを抽出した Copy 可能な構造体で、
/// 複数スレッドへ安価に配布できる。
#[derive(Clone, Copy)]
pub struct CacheDecision {
    pub policy: crate::settings::CachePolicy,
    pub threshold_ms: u32,
    pub size_threshold: u64,
    pub webp_always: bool,
    pub pdf_always: bool,
    pub zip_always: bool,
    // cache_videos_always は動画が別パス (video_thumb) を通るため load_one_cached では使わない
}

impl CacheDecision {
    pub fn from_settings(s: &crate::settings::Settings) -> Self {
        Self {
            policy: s.cache_policy,
            threshold_ms: s.cache_threshold_ms,
            size_threshold: s.cache_size_threshold_bytes,
            webp_always: s.cache_webp_always,
            pdf_always: s.cache_pdf_always,
            zip_always: s.cache_zip_always,
        }
    }

    /// 指定画像をキャッシュに保存すべきか判定する。
    ///
    /// - `Always`: 常に true
    /// - `Off`   : 常に false
    /// - `Auto`  : 事前ヒューリスティック (ext==webp/pdf/zip / サイズ) または
    ///             実測時間 (decode_ms + display_ms) がしきい値以上
    pub fn should_cache(
        &self,
        path: &Path,
        file_size: i64,
        decode_ms: f64,
        display_ms: f64,
    ) -> bool {
        use crate::settings::CachePolicy;
        match self.policy {
            CachePolicy::Always => true,
            CachePolicy::Off => false,
            CachePolicy::Auto => {
                // 事前ヒューリスティック: 拡張子ベースの無条件キャッシュ
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s.to_lowercase())
                    .unwrap_or_default();
                if self.webp_always && ext == "webp" {
                    return true;
                }
                if self.pdf_always && ext == "pdf" {
                    return true;
                }
                if self.zip_always && ext == "zip" {
                    return true;
                }
                if (file_size as u64) >= self.size_threshold {
                    return true;
                }
                // 実測判定
                (decode_ms + display_ms) >= self.threshold_ms as f64
            }
        }
    }
}

// -----------------------------------------------------------------------
// 表示用 ColorImage の生成
// -----------------------------------------------------------------------

/// DynamicImage を `display_px` 以下に収まるよう Lanczos3 でリサイズし、
/// egui::ColorImage に変換する。
///
/// 表示用パス (段階 A) で使用。WebP 量子化を通さず元画像から直接生成するため
/// 画質劣化が無く、キャッシュの WebP(q=75) より高品質。
///
/// SIMD 実装の `fast_image_resize` を使用。image crate の `imageops::resize`
/// (スカラー) に比べてサムネイル生成が 3-5 倍速い。フィルタは同じ Lanczos3。
pub fn resize_to_display_color_image(
    img: &image::DynamicImage,
    display_px: u32,
) -> egui::ColorImage {
    let resized = crate::fast_resize::resize_dynamic_fit(
        img,
        display_px,
        display_px,
        crate::fast_resize::Quality::Lanczos3,
    );
    let rgba = resized.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
}

/// 画像ファイルをデコードし、指定サイズにリサイズした ColorImage を返す。
/// 動画の同名画像サムネイルオーバーライド用。
pub fn decode_image_for_thumb(path: &std::path::Path, display_px: u32) -> Option<egui::ColorImage> {
    // JPEG なら TurboJPEG で DCT scale 付き高速デコードを試す。
    // この関数は cache 用 thumb_px を持たないので target = display_px を使う。
    let turbo_img: Option<image::DynamicImage> = if is_jpeg_ext(path) {
        match decode_jpeg_turbo_scaled_from_path(path, display_px) {
            Ok((img, _stats)) => Some(img),
            Err(DctDecodeError::TerminalRejection(msg)) => {
                // adversarial JPEG — fallback すると danger なので None で諦める。
                // この関数は cosmetic helper (動画 sidecar) なので致命的ではない。
                crate::logger::log(format!(
                    "decode_image_for_thumb: DCT terminal rejection {path:?}: {msg}"
                ));
                return None;
            }
            Err(DctDecodeError::Fallback(_)) => None,
        }
    } else {
        None
    };
    let img = turbo_img
        .or_else(|| image::open(path).ok())
        .or_else(|| crate::wic_decoder::decode_to_dynamic_image(path))?;
    Some(resize_to_display_color_image(&img, display_px))
}

/// EXIF Orientation に基づいて画像を回転・反転する。
/// デコード直後の DynamicImage に適用する。
pub fn apply_exif_orientation(
    img: image::DynamicImage,
    path: &std::path::Path,
) -> image::DynamicImage {
    let orientation = read_exif_orientation(path);
    apply_orientation(img, orientation)
}

/// バイト列から EXIF Orientation を読み取る（ZIP 内画像用）。
pub fn apply_exif_orientation_from_bytes(
    img: image::DynamicImage,
    bytes: &[u8],
) -> image::DynamicImage {
    let orientation = read_exif_orientation_from_bytes(bytes);
    apply_orientation(img, orientation)
}

fn read_exif_orientation(path: &std::path::Path) -> u16 {
    // まず rexif でファイルから EXIF を読む (JPEG, PNG, TIFF 等)
    if let Some(orient) = read_exif_orientation_from_file(path) {
        return orient;
    }

    // rexif が対応しない RAW 形式 (ORF, CR2, NEF 等) は
    // WIC のメタデータクエリリーダーで Orientation を取得する
    if let Some(orient) = crate::wic_decoder::read_wic_orientation(path) {
        return orient;
    }

    1 // デフォルト: 回転なし
}

fn read_exif_orientation_from_file(path: &std::path::Path) -> Option<u16> {
    rexif::parse_file(path.to_str()?).ok().and_then(|exif| {
        exif.entries
            .iter()
            .find(|e| e.ifd.tag == 274)
            .and_then(orientation_from_rexif_entry)
    })
}

pub(crate) fn read_exif_orientation_from_bytes(bytes: &[u8]) -> u16 {
    rexif::parse_buffer(bytes)
        .ok()
        .and_then(|exif| {
            exif.entries
                .iter()
                .find(|e| e.ifd.tag == 274)
                .and_then(orientation_from_rexif_entry)
        })
        .unwrap_or(1)
}

fn orientation_from_rexif_entry(entry: &rexif::ExifEntry) -> Option<u16> {
    entry
        .value
        .to_i64(0)
        .and_then(|v| u16::try_from(v).ok())
        .filter(|v| (1..=8).contains(v))
        .or_else(|| {
            entry
                .value_more_readable
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|v| (1..=8).contains(v))
        })
        .or_else(|| orientation_from_text(&entry.value_more_readable))
}

/// rexif の value_more_readable テキストから Orientation 値を推測する
fn orientation_from_text(text: &str) -> Option<u16> {
    let t = text.to_lowercase();
    if t.contains("straight") || t.contains("normal") {
        return Some(1);
    }
    if t.contains("rotated to left") || t.contains("90 cw") {
        return Some(6);
    }
    if t.contains("upside down") || t.contains("180") {
        return Some(3);
    }
    if t.contains("rotated to right") || t.contains("270 cw") || t.contains("90 ccw") {
        return Some(8);
    }
    if t.contains("mirrored horizontally") {
        return Some(2);
    }
    if t.contains("mirrored vertically") {
        return Some(4);
    }
    None
}

// -----------------------------------------------------------------------
// TurboJPEG 高速デコード + DCT スケール
// -----------------------------------------------------------------------
//
// JPEG ヘッダから真の寸法を読み、target_px と比較して DCT scale factor を選択する。
// libjpeg-turbo の `tj3SetScalingFactor` が `M/8` (M ∈ 1..=8) を受け付けるので、
// 「スケール後でも target を超える最小の M」を選ぶ。
//
// 設計詳細: docs/dct-scale-plan.md
// 実測: scripts/bench_dct_scale.py (Olympus PEN 20MP で 2.5× 高速化、PSNR 51dB)

/// `decode_jpeg_turbo_scaled_*` で許容する圧縮入力サイズの上限 (128 MB)。
/// これを超える JPEG は image::open / WIC chain に降ろす。
///
/// 値の根拠 (Codex 実装レビュー P2 対応):
/// - 通常のサムネ生成は複数ワーカー並列実行 (`src/app.rs::start_loading_items`、
///   `start_cache_creation` の rayon pool)。`std::fs::read(N MB)` × ワーカー数の
///   積算メモリ圧迫を考慮する必要がある。
/// - 100 MB 超の JPEG が複数あるフォルダで 8 ワーカー × 500MB = 4GB pre-decode
///   がピークに乗ると、16GB RAM クラスでスワップする可能性
/// - コンシューマー機の JPEG は通常 5-30 MB、ハイエンド mirrorless (Phase One
///   100MP RAW+JPEG 等) でも 50-100 MB が現実的上限。128 MB はそれを十分カバー
/// - 200 MB 超の JPEG (パノラマ stitch / 産業 / 航空) は image crate の 512MB
///   allocation guard へ素直に投げる方が、責務分離として綺麗
const MAX_TURBOJPEG_INPUT_SIZE: u64 = 128 * 1024 * 1024;

/// DCT スケール後の RGB 出力 buffer サイズ上限 (256 MB ≈ 9000×9000 px)。
/// adversarial JPEG (header に巨大寸法を埋め込んだもの) で巨大 allocation が
/// 発生するのを防ぐ。これを超えるケースは `TerminalRejection` で即拒否し、
/// 呼び出し側が image::open に fallback すると同じ問題が再発するため
/// **fallback してはならない**。
const MAX_DECODED_BYTES: usize = 256 * 1024 * 1024;

/// DCT スケール decode の結果メタデータ。
///
/// `src_w` / `src_h` は JPEG ヘッダから読んだ **元寸法** (EXIF orientation
/// 適用前)。catalog の `source_dims` に保存するには `source_dims_after_exif()`
/// で EXIF orientation を考慮してから使うこと。
///
/// `out_w` / `out_h` は DCT scale 適用後の decoded buffer 寸法 = 戻り値の
/// `DynamicImage::width()/height()` と一致。
#[derive(Copy, Clone, Debug)]
pub struct ScaleStats {
    pub src_w: u32,
    pub src_h: u32,
    /// DCT scale 分子 (1..=8)。分母は常に 8。M=8 は等倍 (scale 無し)。
    pub scale_num: u32,
    pub out_w: u32,
    pub out_h: u32,
}

impl ScaleStats {
    /// EXIF orientation を適用した後の元寸法を返す。orientation が 5-8
    /// (90° / 270° 系) なら w/h を swap。catalog の source_dims にこれを書く。
    pub fn source_dims_after_exif(&self, orientation: u16) -> (u32, u32) {
        match orientation {
            5..=8 => (self.src_h, self.src_w),
            _ => (self.src_w, self.src_h),
        }
    }
}

/// DCT スケール decode の失敗種別。
///
/// `Fallback` は呼び出し側が **image::open → WIC → Susie chain に降りてよい** ケース。
/// `TerminalRejection` は **降りてはいけない** ケース (= image::open 等がもっと
/// 巨大な allocation を要求して safety guard を回避する事故になる)。
#[derive(Debug)]
pub enum DctDecodeError {
    /// header read 失敗・I/O エラー・正常な subsampling 非対応・圧縮入力サイズ超過など。
    /// 呼び出し側は image::open / WIC chain に fallback してよい。
    Fallback(String),
    /// terminal: adversarial / 異常な header dims など。
    /// fallback すると danger なので、エラーを呼び出し元に伝播する。
    TerminalRejection(String),
}

/// 与えられた元寸法と target ピクセルから DCT scale 分子 M を選ぶ。
///
/// 戻り値は M ∈ 1..=8 (scale = M/8)。libjpeg-turbo の出力寸法は
/// `ceil(src * M / 8)` なので、これが `target` を超える最小の M を解く:
///
/// ```text
///   ceil(src * M / 8) >= target
///   ⇔ src * M + 7 >= 8 * target
///   ⇔ M >= (8 * target - 7) / src
///   ⇔ M = ceil((8 * target - 7) / src)
/// ```
///
/// u64 で計算して overflow を回避。`src_max_edge == 0` は 0 division を避けて
/// scale=1/1 (M=8) にフォールバック。
pub(crate) fn pick_dct_scale_num(src_max_edge: u32, target_px: u32) -> u32 {
    if src_max_edge == 0 {
        return 8;
    }
    let target = target_px as u64;
    if target == 0 {
        return 1;
    }
    let numer = (8u64).saturating_mul(target).saturating_sub(7);
    let m_raw = numer.div_ceil(src_max_edge as u64);
    // clamp は u64 上で先に行ってから u32 cast (target_px=u32::MAX 等の異常入力で
    // u32 wrap してから clamp すると意図しない値になる)。
    m_raw.clamp(1, 8) as u32
}

/// バイト列から JPEG を TurboJPEG で DCT scale 付きデコードする。
///
/// `target_px` は最終 thumbnail 表示の max-edge 目安。これに対して
/// `pick_dct_scale_num` で M/8 を選び、libjpeg-turbo の `set_scaling_factor`
/// 経由でデコード時にスケールダウンする。
///
/// 詳細: docs/dct-scale-plan.md §2-§3
pub fn decode_jpeg_turbo_scaled_from_bytes(
    data: &[u8],
    target_px: u32,
) -> Result<(image::DynamicImage, ScaleStats), DctDecodeError> {
    use DctDecodeError::*;
    use turbojpeg::{Decompressor, Image, PixelFormat, ScalingFactor};

    let mut dec = Decompressor::new().map_err(|e| Fallback(format!("Decompressor::new: {e}")))?;
    let header = dec
        .read_header(data)
        .map_err(|e| Fallback(format!("read_header: {e}")))?;

    // lossless JPEG は `set_scaling_factor != 1/1` がエラー。scale=1/1 強制で
    // TurboJPEG full decode を維持する (None で fallback すると遅くなるため)。
    let m = if header.is_lossless {
        8
    } else {
        let src_max = (header.width as u32).max(header.height as u32);
        pick_dct_scale_num(src_max, target_px)
    };
    let scale = ScalingFactor::new(m as usize, 8);
    dec.set_scaling_factor(scale)
        .map_err(|e| Fallback(format!("set_scaling_factor: {e}")))?;

    let scaled = header.scaled(scale);

    // allocation safety: checked_mul + max bound。失敗時は TerminalRejection
    // (= fallback NG)。adversarial JPEG が image::open でもっと巨大な allocation
    // を要求する事故を防ぐ。
    let byte_count = scaled
        .width
        .checked_mul(scaled.height)
        .and_then(|n| n.checked_mul(3))
        .ok_or_else(|| {
            TerminalRejection(format!(
                "decoded buffer overflow: {}x{}x3",
                scaled.width, scaled.height
            ))
        })?;
    if byte_count > MAX_DECODED_BYTES {
        return Err(TerminalRejection(format!(
            "decoded buffer too large: {} bytes > {} max",
            byte_count, MAX_DECODED_BYTES
        )));
    }
    let mut buf = vec![0u8; byte_count];

    let out = Image {
        pixels: buf.as_mut_slice(),
        width: scaled.width,
        pitch: scaled.width * 3,
        height: scaled.height,
        format: PixelFormat::RGB,
    };
    dec.decompress(data, out)
        .map_err(|e| Fallback(format!("decompress: {e}")))?;

    let rgb = image::RgbImage::from_raw(scaled.width as u32, scaled.height as u32, buf)
        .ok_or_else(|| Fallback("RgbImage::from_raw failed".into()))?;
    let img = image::DynamicImage::ImageRgb8(rgb);
    let stats = ScaleStats {
        src_w: header.width as u32,
        src_h: header.height as u32,
        scale_num: m,
        out_w: scaled.width as u32,
        out_h: scaled.height as u32,
    };
    Ok((img, stats))
}

/// JPEG ファイルパスから TurboJPEG で DCT scale 付きデコードする。
///
/// 圧縮ファイルサイズが `MAX_TURBOJPEG_INPUT_SIZE` を超える場合は
/// `Fallback` を返し、image::open / WIC chain に降ろす。
pub fn decode_jpeg_turbo_scaled_from_path(
    path: &Path,
    target_px: u32,
) -> Result<(image::DynamicImage, ScaleStats), DctDecodeError> {
    use DctDecodeError::*;
    let meta = std::fs::metadata(path).map_err(|e| Fallback(format!("metadata: {e}")))?;
    if meta.len() > MAX_TURBOJPEG_INPUT_SIZE {
        return Err(Fallback(format!(
            "input too large for TurboJPEG: {} bytes > {} max",
            meta.len(),
            MAX_TURBOJPEG_INPUT_SIZE
        )));
    }
    let data = std::fs::read(path).map_err(|e| Fallback(format!("read: {e}")))?;
    decode_jpeg_turbo_scaled_from_bytes(&data, target_px)
}

/// 拡張子 (小文字、先頭 `.` なし) が **Susie プラグイン専用** か判定する。
///
/// 「ネイティブ (image クレート / WIC) でデコード不可、かつ Susie が対応する」
/// 場合に true。MAG / PI / PIC / Q4 / MAKI 等が該当。
///
/// この場合 image::open + WIC を試行する分のオーバーヘッド (約 5ms/枚) を
/// 省くため、デコードチェーンを直接 Susie へショートカットする。
fn is_susie_only_ext(ext_lower: &str) -> bool {
    !crate::folder_tree::SUPPORTED_EXTENSIONS.contains(&ext_lower)
        && crate::susie_loader::supports_extension(ext_lower)
}

/// パスから小文字拡張子を抜き出すヘルパ。拡張子なしなら空文字列。
fn ext_lower(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// ZIP エントリ名から小文字拡張子を抜き出すヘルパ。
fn ext_lower_str(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// ZIP エントリのバイト列を image → WIC → Susie の順にフォールバックしてデコード。
/// 成功した経路を `decode_source` に書き戻す (Native はデフォルト、変更しない)。
///
/// 拡張子が Susie 専用 (MAG/PI 等) の場合は image::open + WIC をスキップし、
/// 直接 Susie へ送ることで約 5ms/枚のオーバーヘッドを削減する。
fn decode_zip_chain(
    bytes: &[u8],
    entry_name: &str,
    priority: bool,
    cancel: Option<Arc<AtomicBool>>,
    decode_source: &mut crate::stats::DecodeSource,
) -> Result<image::DynamicImage, image::ImageError> {
    // Susie 専用拡張子の高速パス (image / WIC で確実に失敗する分を省略)
    if is_susie_only_ext(&ext_lower_str(entry_name)) {
        return match crate::susie_loader::decode_bytes(entry_name, bytes, priority, cancel) {
            Ok(img) => {
                *decode_source = crate::stats::DecodeSource::Susie;
                Ok(img)
            }
            Err(e) => Err(image::ImageError::IoError(e)),
        };
    }
    match image::load_from_memory(bytes) {
        Ok(img) => Ok(img),
        Err(e) => match crate::wic_decoder::decode_to_dynamic_image_from_bytes(bytes) {
            Some(img) => {
                *decode_source = crate::stats::DecodeSource::Wic;
                Ok(img)
            }
            None => match crate::susie_loader::decode_bytes(entry_name, bytes, priority, cancel) {
                Ok(img) => {
                    *decode_source = crate::stats::DecodeSource::Susie;
                    Ok(img)
                }
                Err(_) => Err(e),
            },
        },
    }
}

const JPEG_EXTENSIONS: &[&str] = &["jpg", "jpeg", "jpe", "jfif"];

fn is_jpeg_extension(ext: &str) -> bool {
    let lower = ext.to_ascii_lowercase();
    JPEG_EXTENSIONS.iter().any(|&e| e == lower)
}

pub fn is_jpeg_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|s| is_jpeg_extension(s))
}

pub fn is_jpeg_entry(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|s| is_jpeg_extension(s))
}

pub(crate) fn apply_orientation(img: image::DynamicImage, orientation: u16) -> image::DynamicImage {
    match orientation {
        1 => img,                    // 正常
        2 => img.fliph(),            // 左右反転
        3 => img.rotate180(),        // 180°
        4 => img.flipv(),            // 上下反転
        5 => img.rotate90().fliph(), // 転置
        6 => img.rotate90(),         // 90° CW
        7 => img.rotate90().flipv(), // 転置 + 反転
        8 => img.rotate270(),        // 270° CW
        _ => img,
    }
}

/// 表示用 ColorImage の最小ピクセル数 (起動直後で cell_size が小さすぎる場合の最低品質保証)
const DISPLAY_PX_MIN: u32 = 256;
/// 表示用 ColorImage の最大ピクセル数 (4K 2列などの巨大セルで過大メモリを防ぐ)
const DISPLAY_PX_MAX: u32 = 2048;

/// 現在のセルサイズから表示用 ColorImage の画素数を算出する。
///
/// 論理ピクセル × DPI スケールで物理ピクセルを求め、DISPLAY_PX_MIN-DISPLAY_PX_MAX px にクランプする。
/// - 下限 DISPLAY_PX_MIN: 起動直後で cell_size が小さすぎる場合の最低品質保証
/// - 上限 DISPLAY_PX_MAX: 4K 2列などの巨大セルで過大メモリを防ぐ (最大 16 MB/ColorImage)
pub fn compute_display_px(cell_w: f32, cell_h: f32, dpi: f32) -> u32 {
    let logical_max = cell_w.max(cell_h).max(1.0);
    let physical = (logical_max * dpi.max(0.5)).ceil();
    (physical as u32).clamp(DISPLAY_PX_MIN, DISPLAY_PX_MAX)
}

// -----------------------------------------------------------------------
// メインのリクエスト処理
// -----------------------------------------------------------------------

fn send_thumb_failed(req: &LoadRequest, tx: &mpsc::Sender<ThumbMsg>, gen_done: &Arc<AtomicUsize>) {
    let _ = tx.send(ThumbMsg {
        idx: req.idx,
        image: None,
        from_cache: false,
        from_edit_preview: false,
        edit_preview_adjustment: None,
        source_dims: None,
        canceled: false,
        finalized: false,
        input_seq: req.input_seq,
        items_gen: req.items_gen,
    });
    gen_done.fetch_add(1, Ordering::Relaxed);
}

fn send_pinned_only_cached(
    req: &LoadRequest,
    pin: &PinnedOnlyRequest,
    cache_map: &std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
    tx: &mpsc::Sender<ThumbMsg>,
    gen_done: &Arc<AtomicUsize>,
) -> bool {
    let cached = cache_map.read().ok().and_then(|map| {
        map.iter()
            .filter(|(key, _)| key.starts_with(&pin.cache_key_prefix))
            .max_by_key(|(_, entry)| (entry.mtime, entry.file_size))
            .map(|(key, entry)| {
                (
                    key.clone(),
                    entry.jpeg_data.clone(),
                    entry.source_dims,
                    entry.mtime,
                    entry.file_size,
                )
            })
    });

    let Some((filename, webp_data, source_dims, mtime, file_size)) = cached else {
        crate::logger::log(format!(
            "drive-list pin: cached thumbnail missing ({})",
            pin.cache_key_prefix
        ));
        return false;
    };

    let ci = crate::catalog::decode_thumb_to_color_image(&webp_data);
    let _ = tx.send(ThumbMsg {
        idx: req.idx,
        image: ci,
        from_cache: true,
        from_edit_preview: false,
        edit_preview_adjustment: None,
        source_dims,
        canceled: false,
        finalized: false,
        input_seq: req.input_seq,
        items_gen: req.items_gen,
    });
    gen_done.fetch_add(1, Ordering::Relaxed);
    crate::logger::log(format!(
        "    idx={:>4} drive_list_pin_cache_hit  {filename} ({mtime}/{file_size})",
        req.idx,
    ));
    true
}

/// 段階 B: 1 つの `LoadRequest` を処理する。
///
/// - 通常: `cache_map` を参照しキャッシュヒットしていれば WebP を復号して送信する
///   (`from_cache = true`)
/// - ミスまたは `req.skip_cache = true`: `load_one_cached` に委譲してフルデコード
///   (`from_cache = false`、段階 E のアップグレード経路)
#[allow(clippy::too_many_arguments)]
pub fn process_load_request(
    req: &LoadRequest,
    cache_map: &std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
    tx: &mpsc::Sender<ThumbMsg>,
    // **v1.0.0**: `Arc` 経由で受け取る (= 内部の WebP cache hit catch-up worker が
    // `Arc::clone` で所有権を持ったまま background スレッドに移すため)。既存 `&CatalogDb`
    // を期待する内部関数 (`load_one_cached` 等) には `.as_ref()` で透過的に渡せる。
    catalog: Option<&Arc<crate::catalog::CatalogDb>>,
    thumb_px: u32,
    thumb_quality: u8,
    display_px: u32,
    cache_decision: CacheDecision,
    gen_done: &Arc<AtomicUsize>,
    stats: &Arc<Mutex<crate::stats::ThumbStats>>,
    cancel: Option<&Arc<AtomicBool>>,
    keep_start: &Arc<AtomicUsize>,
    keep_end: &Arc<AtomicUsize>,
    // pin-aware auto-pick 用の DB ハンドル。`resolve_folder_thumb_image` が
    // recursive auto-pick の各段でサブフォルダの pin を引いて leaf 画像へ
    // cascade 解決する。`None` のとき従来の純粋 auto-pick になる。
    pin_db: Option<&crate::folder_thumb_pins::FolderThumbPinDb>,
    edit_preview_db: Option<&Arc<crate::edit_preview_cache::EditPreviewCacheDb>>,
) {
    if let Some(pin) = req.pinned_only.as_ref() {
        if !send_pinned_only_cached(req, pin, cache_map, tx, gen_done) {
            send_thumb_failed(req, tx, gen_done);
        }
        return;
    }

    // 内部関数向けに `&CatalogDb` を取り出しておく (= 既存シグネチャ互換)
    let catalog_ref: Option<&crate::catalog::CatalogDb> = catalog.map(|a| a.as_ref());
    // カタログキーを共通 helper で組み立て (auto_aspect の seed フェーズと共有)。
    // None ケース (path に file_name が無い等の異常) は既存挙動に合わせて空文字 fallback。
    let key_cow = cache_key_for_request(req).unwrap_or(std::borrow::Cow::Borrowed(""));
    let filename: &str = key_cow.as_ref();

    // 編集済みページは通常 catalog より先に、source 解像度 edit-result 由来の
    // 永続プレビューを試す。これは最大辺 2048px / q=90 の完成済み派生画像なので、
    // 後段の idle quality-upgrade で元画像に差し替えてはいけない。
    if !req.skip_cache
        && let (Some(item_key), Some(db)) = (req.edit_preview_key.as_deref(), edit_preview_db)
        && let Some(preview) = db.load(item_key, req.mtime, req.file_size)
    {
        let _ = tx.send(ThumbMsg {
            idx: req.idx,
            image: Some(preview.image),
            from_cache: true,
            from_edit_preview: true,
            edit_preview_adjustment: Some(ThumbEditPreviewAdjustment {
                base: preview.adjustment_base,
                annotation_layers: preview.annotation_layers,
            }),
            source_dims: Some(preview.source_dims),
            canceled: false,
            finalized: false,
            input_seq: req.input_seq,
            items_gen: req.items_gen,
        });
        gen_done.fetch_add(1, Ordering::Relaxed);
        crate::logger::log(format!(
            "    idx={:>4} edit_preview_cache_hit  {filename}",
            req.idx,
        ));
        return;
    }

    let req_t0 = std::time::Instant::now();
    if crate::perf::is_enabled() {
        crate::perf::event(
            "thumb",
            "decode_begin",
            Some(filename),
            req.input_seq,
            &[
                ("idx", serde_json::Value::from(req.idx)),
                ("priority", serde_json::Value::from(req.priority)),
                ("skip_cache", serde_json::Value::from(req.skip_cache)),
            ],
        );
    }

    // 段階 E: skip_cache = true のときはキャッシュヒット判定を飛ばして
    // 必ず元画像からデコードする (アイドル時の画質アップグレード用)
    if !req.skip_cache {
        // read ロックは最短に保つ: エントリのデータだけ clone して即解放。
        // WebP デコード (2-3 ms) をロック外で実行することで、
        // 他ワーカーの write (キャッシュ保存) をブロックしない。
        let cached = cache_map.read().ok().and_then(|map| {
            let entry = map.get(filename)?;
            if entry.mtime == req.mtime && entry.file_size == req.file_size {
                Some((entry.jpeg_data.clone(), entry.source_dims))
            } else {
                None
            }
        });
        if let Some((webp_data, source_dims)) = cached {
            let ci = crate::catalog::decode_thumb_to_color_image(&webp_data);
            let cache_ms = req_t0.elapsed().as_secs_f64() * 1000.0;
            // from_cache = true: アップグレード対象
            // source_dims はカタログ由来 (旧バージョンで作成された
            // エントリには None が入っている)
            let _ = tx.send(ThumbMsg {
                idx: req.idx,
                image: ci,
                from_cache: true,
                from_edit_preview: false,
                edit_preview_adjustment: None,
                source_dims,
                canceled: false,
                finalized: false,
                input_seq: req.input_seq,
                items_gen: req.items_gen,
            });
            gen_done.fetch_add(1, Ordering::Relaxed);
            crate::logger::log(format!(
                "    idx={:>4} cache_hit={cache_ms:>5.1}ms  {filename}",
                req.idx,
            ));
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "thumb",
                    "decode_end",
                    Some(filename),
                    req.input_seq,
                    &[
                        ("idx", serde_json::Value::from(req.idx)),
                        ("ms", serde_json::Value::from(cache_ms)),
                        ("from_cache", serde_json::Value::from(true)),
                    ],
                );
            }
            // ── pdf_meta catch-up (v1.0.0) ──
            // PDF ファイルサムネが WebP cache hit したケース (= render_page を skip した
            // ケース) では `pdf_meta` テーブルが populate されない。v0.x 時代の WebP
            // サムネ済みユーザーがアップグレードした場合、初回 Enter で cache miss
            // になり 800ms 級の待ちが入る。サムネを表示した直後に裏で enumerate して
            // pdf_meta を埋めることで、その PDF の初回 Enter から瞬時にする。
            maybe_spawn_pdf_meta_catchup(req, catalog);
            return;
        }
    }

    // キャッシュミス or skip_cache: フルデコード (+ 必要なら保存)
    // load_one_cached は from_cache = false を送信する

    // 重い I/O (Folder / ZipFile / ConvertibleArchive / ZipDir) は専用 I/O ワーカーキューで処理されるため、
    // セマフォは不要。I/O ワーカー数 (1-2) で自然に同時実行数が制限される。
    //
    // pin 解決時 (`resolve_override` Some) は **prefix 判定をスキップ**し、
    // 明示された strategy で dispatch する。pin の cache key は親側の prefix
    // (folderthumb / zipthumb / pdfthumb) を保つので、prefix 判定だけだと
    // 「ZIP 内画像を親のフォルダ thumb として scan しようとして失敗」のような
    // ミスマッチが起きる。
    let is_folder_thumb = match req.resolve_override {
        Some(ResolveStrategy::FolderRepresentative) => true,
        Some(_) => false,
        None if req.zip_entry.is_none() && req.pdf_page.is_none() => req
            .cache_key_override
            .as_deref()
            .is_some_and(|k| k.starts_with(CACHE_KEY_FOLDER)),
        None => false,
    };
    let is_zip_thumb = match req.resolve_override {
        Some(ResolveStrategy::ZipFirstImage) => true,
        Some(_) => false,
        None if req.zip_entry.is_none() && req.pdf_page.is_none() => req
            .cache_key_override
            .as_deref()
            .is_some_and(|k| k.starts_with(CACHE_KEY_ZIP)),
        None => false,
    };
    let is_zip_dir_thumb = matches!(
        req.resolve_override,
        Some(ResolveStrategy::ZipDirRepresentative)
    );
    let needs_heavy_io = is_folder_thumb || is_zip_thumb || is_zip_dir_thumb;

    // フォルダサムネイル: フォルダ内の画像を探して代表画像のパスに差し替え。
    // pin-aware: 再帰中に見つけたサブフォルダに pin があれば cascade 解決して
    // leaf 画像を採用する (= auto-pick が経由する子フォルダの pin を尊重)。
    let resolved_folder_image = if is_folder_thumb {
        let t_resolve = std::time::Instant::now();
        let img = resolve_folder_thumb_image(
            &req.path,
            req.folder_thumb_sort
                .unwrap_or(crate::settings::SortOrder::Numeric),
            req.folder_thumb_depth,
            pin_db,
        );
        let resolve_ms = t_resolve.elapsed().as_secs_f64() * 1000.0;
        if resolve_ms > 10.0 {
            crate::logger::log(format!(
                "    idx={:>4} folder_resolve={resolve_ms:>6.1}ms  {}",
                req.idx,
                req.path.display(),
            ));
        }
        if img.is_none() {
            let _ = tx.send(ThumbMsg {
                idx: req.idx,
                image: None,
                from_cache: false,
                from_edit_preview: false,
                edit_preview_adjustment: None,
                source_dims: None,
                canceled: false,
                finalized: false,
                input_seq: req.input_seq,
                items_gen: req.items_gen,
            });
            gen_done.fetch_add(1, Ordering::Relaxed);
            return;
        }
        img
    } else {
        None
    };
    let load_path: &Path = resolved_folder_image.as_deref().unwrap_or(&req.path);

    // ZipFile (フォルダ一覧用サムネイル) の場合、UI スレッドでの ZIP I/O を避けるため
    // zip_entry が None のまま渡される。ワーカー側で遅延解決する。
    //
    // ネットワークドライブでは ZIP の open (セントラルディレクトリ読み取り) が高コスト
    // なため、first_image_entry + read_entry_bytes の 2 回 open を
    // read_first_image_bytes の 1 回 open に統合する。
    let resolved_zip_entry: Option<String>;
    let preloaded_zip_bytes: Option<Vec<u8>>;
    let zip_entry_ref: Option<&str> = if req.zip_entry.is_some() {
        preloaded_zip_bytes = None;
        req.zip_entry.as_deref()
    } else if is_zip_dir_thumb {
        let t_zip = std::time::Instant::now();
        let name = req.zip_dir_prefix.as_deref().and_then(|prefix| {
            let enumeration =
                crate::zip_loader::enumerate_image_entries_detailed(&req.path).ok()?;
            // CP932 名 ZIP のリリース済み per-page キー移行 (worker スレッド)。
            // フォルダ一覧のサムネ生成段階で済ませると、ZIP を開く前にピン/★ が直る。
            crate::zip_key_migration::migrate_if_needed(&req.path, &enumeration.legacy_renames);
            let entries = enumeration.entries;
            let tree = crate::zip_tree::ZipTree::build(req.path.clone(), entries);
            tree.representative_for_prefix_str(
                prefix,
                req.folder_thumb_sort
                    .unwrap_or(crate::settings::SortOrder::Numeric),
            )
            .map(|e| e.entry_name.clone())
        });
        let zip_ms = t_zip.elapsed().as_secs_f64() * 1000.0;
        // ZipDir 代表解決は enumerate_image_entries (ZIP セントラルディレクトリ読み = 重 I/O) +
        // ツリー構築を伴うことがあるため heavy_io_queue 側に振る。所要は regressions を
        // 追えるよう analyze_perf.py で集計できるイベントとして残す。
        if crate::perf::is_enabled() {
            crate::perf::event(
                "thumb",
                "zipdir_resolve",
                None,
                req.input_seq,
                &[
                    ("idx", serde_json::Value::from(req.idx)),
                    ("ms", serde_json::Value::from(zip_ms)),
                    ("resolved", serde_json::Value::from(name.is_some())),
                ],
            );
        }
        match name {
            Some(name) => {
                crate::logger::log(format!(
                    "    idx={:>4} zipdir_resolve={zip_ms:>6.1}ms  {}",
                    req.idx,
                    req.path.display(),
                ));
                resolved_zip_entry = Some(name);
                preloaded_zip_bytes = None;
                resolved_zip_entry.as_deref()
            }
            None => {
                let _ = tx.send(ThumbMsg {
                    idx: req.idx,
                    image: None,
                    from_cache: false,
                    from_edit_preview: false,
                    edit_preview_adjustment: None,
                    source_dims: None,
                    canceled: false,
                    finalized: false,
                    input_seq: req.input_seq,
                    items_gen: req.items_gen,
                });
                gen_done.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    } else if is_zip_thumb {
        // cache_key_override あり + pdf_page なし + フォルダでない = ZipFile サムネイル
        // ZIP を 1 回だけ開いてエントリ名 + バイト列を同時取得
        let t_zip = std::time::Instant::now();
        match crate::zip_loader::read_first_image_bytes(&req.path) {
            Some((name, bytes)) => {
                let zip_ms = t_zip.elapsed().as_secs_f64() * 1000.0;
                crate::logger::log(format!(
                    "    idx={:>4} zip_resolve={zip_ms:>6.1}ms  ({} bytes)  {}",
                    req.idx,
                    bytes.len(),
                    req.path.display(),
                ));
                resolved_zip_entry = Some(name);
                preloaded_zip_bytes = Some(bytes);
                resolved_zip_entry.as_deref()
            }
            None => {
                // ZIP 内に画像が無い場合は失敗として通知
                let _ = tx.send(ThumbMsg {
                    idx: req.idx,
                    image: None,
                    from_cache: false,
                    from_edit_preview: false,
                    edit_preview_adjustment: None,
                    source_dims: None,
                    canceled: false,
                    finalized: false,
                    input_seq: req.input_seq,
                    items_gen: req.items_gen,
                });
                gen_done.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    } else {
        preloaded_zip_bytes = None;
        None
    };

    // 重い I/O (ZIP/フォルダ) 完了後の stale チェック:
    // resolve に数秒かかった場合、スクロールで keep_range 外になっている可能性がある。
    // 不要な decode + send を省き、UI 側の requested 除去を早める。
    // canceled=true で送信 → poll_thumbnails で Evicted (retriable) に戻す。
    // Failed にしないのは、scroll 戻り時に再ロードできるようにするため。
    if needs_heavy_io {
        let ks = keep_start.load(Ordering::Relaxed);
        let ke = keep_end.load(Ordering::Relaxed);
        if req.idx < ks || req.idx >= ke {
            crate::logger::log(format!(
                "    idx={:>4} STALE (after I/O resolve)  {}",
                req.idx,
                req.path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
            ));
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "thumb",
                    "skip",
                    None,
                    req.input_seq,
                    &[
                        ("idx", serde_json::Value::from(req.idx)),
                        ("reason", serde_json::Value::from("stale_after_io")),
                    ],
                );
            }
            let _ = tx.send(ThumbMsg {
                idx: req.idx,
                image: None,
                from_cache: false,
                from_edit_preview: false,
                edit_preview_adjustment: None,
                source_dims: None,
                canceled: true,
                finalized: false,
                input_seq: req.input_seq,
                items_gen: req.items_gen,
            });
            gen_done.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }

    // PDF ページの stale チェック:
    // PDFium レンダは 1 枚 1 秒クラスの重処理なので、cache miss 時にも
    // 開始前に keep_range 外になっていれば中断する。これがないと
    // スクロール往復で同じページが複数回レンダされる事故が起きる。
    if req.pdf_page.is_some() && !req.skip_cache {
        let ks = keep_start.load(Ordering::Relaxed);
        let ke = keep_end.load(Ordering::Relaxed);
        if req.idx < ks || req.idx >= ke {
            crate::logger::log(format!(
                "    idx={:>4} STALE (pdf before render)  {}",
                req.idx,
                req.path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
            ));
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "thumb",
                    "skip",
                    None,
                    req.input_seq,
                    &[
                        ("idx", serde_json::Value::from(req.idx)),
                        ("reason", serde_json::Value::from("stale_pdf_before_render")),
                    ],
                );
            }
            let _ = tx.send(ThumbMsg {
                idx: req.idx,
                image: None,
                from_cache: false,
                from_edit_preview: false,
                edit_preview_adjustment: None,
                source_dims: None,
                canceled: true,
                finalized: false,
                input_seq: req.input_seq,
                items_gen: req.items_gen,
            });
            gen_done.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }

    // PDF render 経路 (pdf_page=Some) で cache 保存可能なら HarvestOnCancel に切替。
    // = 「PDFium が既に処理した結果を捨てない」ための投資回収。
    // 静的 gate: skip_cache / catalog 無し / cache_map 無し のいずれかが該当すれば
    // どうせ cache 保存しないので harvest 待ちの意味が無く AbortOnCancel に倒す。
    // (詳細は docs/pdf-pool-harvest-on-cancel-plan.md)
    let cancel_policy = if req.pdf_page.is_some() && !req.skip_cache && catalog_ref.is_some() {
        crate::pdf_loader::CancelWaitPolicy::HarvestOnCancel
    } else {
        crate::pdf_loader::CancelWaitPolicy::AbortOnCancel
    };

    load_one_cached(
        load_path,
        zip_entry_ref,
        preloaded_zip_bytes,
        req.pdf_page,
        req.pdf_password.as_deref(),
        req.cache_key_override.as_deref(),
        req.idx,
        tx,
        catalog_ref,
        Some(cache_map),
        req.mtime,
        req.file_size,
        gen_done,
        thumb_px,
        thumb_quality,
        display_px,
        cache_decision,
        stats,
        cancel,
        req.priority,
        req.input_seq,
        req.items_gen,
        req.context_epoch,
        cancel_policy,
        req.force_cache,
    );
    if crate::perf::is_enabled() {
        let total_ms = req_t0.elapsed().as_secs_f64() * 1000.0;
        crate::perf::event(
            "thumb",
            "decode_end",
            Some(filename),
            req.input_seq,
            &[
                ("idx", serde_json::Value::from(req.idx)),
                ("ms", serde_json::Value::from(total_ms)),
                ("from_cache", serde_json::Value::from(false)),
            ],
        );
    }
}

// ─────────────────────────────────────────────────────────────────────
// pdf_meta catch-up worker (v1.0.0)
// ─────────────────────────────────────────────────────────────────────
//
// WebP cache hit で PDF サムネが表示されたが、`pdf_meta` テーブルに行が
// 無いケースで裏で enumerate を実行して populate するためのヘルパー。
//
// 主な発動シーン:
//   - v0.x → v1.0.0 アップグレードユーザー: catalog 内の thumb は既に WebP で
//     キャッシュ済みなので render_page が走らず、`pdf_meta` は空のまま。初回
//     Enter で 800ms 級の待ちが発生する。
//   - サムネ表示時にこの catch-up が裏で走れば、ユーザーが「サムネを見て一拍
//     置いて Enter」する自然な間に pdf_meta が埋まる → 初回 Enter から瞬時化。
//
// 安全策 (Codex round 1-5 で確立したルール):
//   1. `pdf_password=None` で enumerate を試みる (= session pw を持ち込まない)。
//      成功すれば「password 不要」確定 → `set_pdf_meta(false)`。失敗すれば PDF が
//      暗号化されている可能性が高いので何も書かない (= 次回 Enter で
//      poll_pdf_enumerate がパスワードダイアログを出して正しい flag を書く)。
//   2. **CatchupQueue + pending HashSet** (= 旧「in-flight set」相当) で同 path の
//      重複 enqueue を防ぎ、優先度別 bounded VecDeque (high cap 16 / low cap 256)
//      で総作業量を抑える。lane が満杯のときだけ drop、lane 間は独立 (= low の flood
//      が high の neighbor prefetch を蝕まない)。詳細は下の `CatchupQueueState` 定義
//      とその周辺コメント参照。

use std::collections::{HashSet, VecDeque};
use std::sync::OnceLock;

// ── catch-up / neighbor prefetch ジョブの種類 ──

enum CatchupJobKind {
    /// WebP cache hit 経由: `enumerate_pages(password=None)` のみ、書き込みは pdf_meta のみ
    MetaOnly {
        catalog: Arc<crate::catalog::CatalogDb>,
    },
    /// load_pdf_as_folder の neighbor 経由: `render_page(page=0, password=None)` を実行し、
    /// pdf_meta + parent catalog の `pdfthumb:` WebP まで populate
    NeighborPrefetch {
        catalog: Arc<crate::catalog::CatalogDb>,
        thumb_px: u32,
        thumb_quality: u8,
    },
}

struct CatchupJob {
    path: std::path::PathBuf,
    kind: CatchupJobKind,
    /// **review #11/#13 対応**: enqueue 時点の `current_cancel` を握っておく。
    /// フォルダ移動 (`bump_catchup_epoch`) で worker / 各種 PDFium ジョブを
    /// 即中断するため、render_page に渡すと共に、job 取り出し時にも
    /// `is_cancelled()` で早期 skip 判定する。
    cancel: Arc<AtomicBool>,
}

// ── キュー本体 (Codex P2 round 2 対応) ──
//
// 旧設計 (request ごとに `std::thread::spawn`、in-flight HashSet サイズ cap=8 で drop)
// は PDF 多数フォルダで「8 件超 cache hit が永久に retry されない」「catch-up が
// 直近の neighbor prefetch 枠を食う」問題があった (Codex round 3 指摘)。
//
// 新設計: 単一 worker スレッド + 優先度別 bounded VecDeque + dedup HashSet:
//   - high (= neighbor prefetch、cap 16): load_pdf_as_folder 直後の Ctrl+↑↓ 想定
//   - low (= cache-hit catch-up、cap 256): scroll で滑り込む cache hit の大波対応
//   - worker は high → low の順で 1 件ずつ pop、PDF worker pool 経由で実作業
//   - 同一 path 重複は HashSet で 1 件に集約 (= dedup)
//   - cap 超過時のみ drop。drop しても機能不整合なし (Enter で必ず populate される)

struct CatchupQueueState {
    high: VecDeque<CatchupJob>,
    low: VecDeque<CatchupJob>,
    /// queue に入っている (or 現在 worker が処理中) の path 集合。dedup 用。
    pending: HashSet<std::path::PathBuf>,
    shutdown: bool,
    /// **review #11/#13 対応**: 現行 epoch の cancel flag。enqueue する job に焼き
    /// 付けて、フォルダ移動時に `bump_epoch` で旧 epoch を一括キャンセルする。
    current_cancel: Arc<AtomicBool>,
}

struct CatchupQueue {
    state: Mutex<CatchupQueueState>,
    cv: Condvar,
}

impl CatchupQueue {
    /// 現行 epoch の cancel flag を立てて、新しい epoch を開始する。フォルダ移動
    /// (= 新しい `load_folder` / `load_pdf_as_folder`) のときに呼ぶ。
    ///
    /// **挙動 (review #11/#13)**:
    ///   - 既存の `high` / `low` queue を全て drop。pending HashSet もクリア。
    ///     → 未処理ジョブを即座に捨てる。`Arc<CatalogDb>` への参照もここで切れる。
    ///   - 旧 cancel flag を true にセット。worker が今まさに走らせている
    ///     render_page / enumerate_pages は cancel を読んで Interrupted で抜ける。
    ///   - 新 cancel flag を生成。以降の enqueue はこれを焼き付ける。
    fn bump_epoch(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.current_cancel.store(true, Ordering::Relaxed);
        state.current_cancel = Arc::new(AtomicBool::new(false));
        state.high.clear();
        state.low.clear();
        state.pending.clear();
        // worker は wait 中なら新しい job を待っているだけなので notify 不要。
        // 進行中の job は cancel flag を読んで自発的に抜ける。
    }
}

/// 外部から呼ぶ public API。フォルダ移動の入口 (`App::load_folder` /
/// `load_pdf_as_folder` 等) で呼んで、前フォルダ向け catch-up を全部キャンセルする。
pub fn bump_catchup_epoch() {
    catchup_queue().bump_epoch();
}

/// high (neighbor prefetch) の queue 上限。Ctrl+↑↓ で連打しても 16 件先まで温める。
const MAX_NEIGHBOR_PENDING: usize = 16;
/// low (cache-hit catch-up) の queue 上限。100+ PDF フォルダの scroll でも収まる規模。
const MAX_CATCHUP_PENDING: usize = 256;

static CATCHUP_QUEUE: OnceLock<Arc<CatchupQueue>> = OnceLock::new();

fn catchup_queue() -> &'static Arc<CatchupQueue> {
    CATCHUP_QUEUE.get_or_init(|| {
        let queue = Arc::new(CatchupQueue {
            state: Mutex::new(CatchupQueueState {
                high: VecDeque::new(),
                low: VecDeque::new(),
                pending: HashSet::new(),
                shutdown: false,
                current_cancel: Arc::new(AtomicBool::new(false)),
            }),
            cv: Condvar::new(),
        });
        let q = Arc::clone(&queue);
        std::thread::Builder::new()
            .name("pdf-meta-catchup".into())
            .spawn(move || catchup_worker_loop(q))
            .expect("spawn pdf-meta-catchup worker");
        queue
    })
}

/// `catchup_worker_loop` が処理中の job について、関数を抜ける (= 正常 return も
/// panic unwind も) ときに pending HashSet から確実に取り除く RAII guard。
///
/// **review #2 対応**: 旧実装は `process_meta_only` / `process_neighbor_prefetch`
/// が成功 return したパスだけで pending を削除していたので、panic が起きると path が
/// 永続的に pending に残り、以降の `maybe_spawn_pdf_meta_catchup` がその path を
/// 永久に dedup でスキップ → cache hit 経由 catch-up が無音で停止していた。
/// Drop は unwind しても呼ばれるので、ここで除去すれば panic 後も catch-up 機能は
/// 1 件の path 漏れだけで済む。
struct PendingGuard<'a> {
    queue: &'a CatchupQueue,
    path: &'a std::path::PathBuf,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        // Mutex poisoning にも対応: poisoned でも内側の HashSet は触れる
        let mut state = match self.queue.state.lock() {
            Ok(s) => s,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.pending.remove(self.path);
    }
}

/// バックグラウンドワーカーループ。high → low の順で pop し、1 件ずつ実作業。
///
/// **panic 回復 (review #2 対応)**: 1 件の job 処理が panic しても worker thread が
/// 死なないように `catch_unwind` で囲う。worker が死ぬと `OnceLock` で再起動できず、
/// cache hit 経由 pdf_meta catch-up が無音で停止して PDF Enter latency が
/// 旧バージョン (≈700ms cold) に静かに退行する。
fn catchup_worker_loop(queue: Arc<CatchupQueue>) {
    loop {
        // queue から 1 件取り出す。空なら Condvar で寝る。
        let job = {
            let mut state = queue.state.lock().unwrap();
            loop {
                if state.shutdown {
                    return;
                }
                if let Some(j) = state.high.pop_front() {
                    break j;
                }
                if let Some(j) = state.low.pop_front() {
                    break j;
                }
                state = queue.cv.wait(state).unwrap();
            }
        };

        let path = job.path.clone();
        // Drop guard で pending 削除を保証 (panic unwind でも実行される)
        let _pending_guard = PendingGuard {
            queue: &queue,
            path: &path,
        };
        let kind = job.kind;
        let cancel = job.cancel;
        // **review #11/#13 対応**: job 取り出し時に cancel チェック。フォルダ移動で
        // bump_epoch されていれば、まだ走り出していない job をここで早期 skip
        // (= PDFium IPC を無駄に発行しない)。
        if cancel.load(Ordering::Relaxed) {
            continue;
        }
        let path_for_job = path.clone();
        let cancel_for_job = Arc::clone(&cancel);
        let panic_result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || match kind {
                CatchupJobKind::MetaOnly { catalog } => {
                    process_meta_only(&path_for_job, &catalog, &cancel_for_job)
                }
                CatchupJobKind::NeighborPrefetch {
                    catalog,
                    thumb_px,
                    thumb_quality,
                } => process_neighbor_prefetch(
                    &path_for_job,
                    &catalog,
                    thumb_px,
                    thumb_quality,
                    &cancel_for_job,
                ),
            }));
        if let Err(payload) = panic_result {
            let msg = panic_payload_to_string(&payload);
            crate::logger::log(format!(
                "pdf-meta-catchup: job panicked, worker survived (path={}): {msg}",
                path.display()
            ));
        }
        // _pending_guard ここで drop → pending HashSet から path 除去
    }
}

fn panic_payload_to_string(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// 実作業: cache hit 経由の pdf_meta catch-up (enumerate のみ、確信値 false で書き込み)
fn process_meta_only(path: &Path, catalog: &crate::catalog::CatalogDb, cancel: &Arc<AtomicBool>) {
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    let Some(mtime) = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
    else {
        return;
    };
    let file_size = meta.len() as i64;

    // pdf_meta が既に populate 済みなら skip (= enqueue から処理開始までの間に他経路が
    // 書き込んだケース)
    if catalog
        .get_pdf_meta(filename, mtime, file_size)
        .ok()
        .flatten()
        .is_some()
    {
        return;
    }

    // password=None で試行。
    // **review #10 対応 (negative cache)**: 暗号化 PDF だと毎回 enumerate が失敗する
    // が、旧実装は失敗時に何も書かなかったため pdf_meta 行が無いまま残り、
    // scroll でまた同じ thumb が cache-hit するたびに maybe_spawn_pdf_meta_catchup
    // が同じ path を再 enqueue → 50 件保護 PDF だと毎スクロール 5-65 秒の IPC を
    // 無限にバーンしていた。失敗時は (page_count=0, password_required=true) で
    // negative cache 行を書いて re-enqueue を止める。
    // **Codex P3-2 対応**: cancel を enumerate にも伝搬する (cancel-aware 版を使う)。
    // `bump_catchup_epoch` によるフォルダ移動キャンセルが、走行中の MetaOnly enumerate も
    // Interrupted で抜けさせる (旧実装: cancel 非対応の `enumerate_pages` を呼んでいて
    // 走行中の MetaOnly は完走するまで PDF worker を占有していた)。
    match crate::pdf_loader::enumerate_pages_with_cancel(path, None, Some(Arc::clone(cancel))) {
        Ok(entries) => {
            if let Err(e) = catalog.set_pdf_meta(
                filename,
                mtime,
                file_size,
                entries.len() as u32,
                false, // password not required (enumerate succeeded without pw)
            ) {
                crate::logger::log(format!(
                    "pdf-meta-catchup: set_pdf_meta failed for {filename}: {e}"
                ));
            }
        }
        Err(e) => {
            let msg = format!("{e}");
            if is_password_required_error(&msg) {
                // negative cache: 0 ページ + password_required=true。
                // `try_apply_pdf_meta_cache` 側で has_saved_password=false なら
                // placeholder を出さずに通常 enumerate (= ユーザーへ password 入力
                // ダイアログ) に倒れる。
                if let Err(e2) = catalog.set_pdf_meta(filename, mtime, file_size, 0, true) {
                    crate::logger::log(format!(
                        "pdf-meta-catchup: negative cache write failed for {filename}: {e2}"
                    ));
                }
            } else {
                crate::logger::log(format!(
                    "pdf-meta-catchup: enumerate failed for {filename}: {msg}"
                ));
            }
        }
    }
}

/// PDFium のパスワード要求エラーかを判定する。`PdfiumError::PdfiumLibraryInternalError(
/// PdfiumInternalError::PasswordError)` の Display 文字列に "Password" が含まれる
/// ことを利用する (`src/app.rs:6376` と同じ pattern)。
fn is_password_required_error(msg: &str) -> bool {
    msg.contains("Password") || msg.contains("password")
}

/// 実作業: neighbor prefetch (render page 0 + pdf_meta + WebP サムネ)
fn process_neighbor_prefetch(
    path: &Path,
    parent_catalog: &crate::catalog::CatalogDb,
    thumb_px: u32,
    thumb_quality: u8,
    cancel: &Arc<AtomicBool>,
) {
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    let Some(mtime) = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
    else {
        return;
    };
    let file_size = meta.len() as i64;
    let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
        return;
    };

    // 既に pdf_meta + thumb 両方 cache 済みなら skip
    let meta = parent_catalog
        .get_pdf_meta(filename, mtime, file_size)
        .ok()
        .flatten();
    let meta_present = meta.is_some();
    // **review #10 対応**: negative cache (password_required=true) を持つ PDF は
    // None で render しても失敗確定。thumb がまだ無くても retry しない。
    // これを忘れると thumb_present=false で毎回 render_page → 同じ password エラーで
    // IPC を浪費する loop に陥る。
    if let Some((_, true)) = meta {
        return;
    }
    let folder_key = format!("{}{}", CACHE_KEY_PDF, filename);
    let thumb_present = parent_catalog
        .load_one(&folder_key)
        .ok()
        .flatten()
        .map(|e| e.mtime == mtime && e.file_size == file_size)
        .unwrap_or(false);
    if meta_present && thumb_present {
        return;
    }

    // render_page (page 0、Normal 優先度) — password=None で試行。
    // **review #10 対応**: 失敗時に password 起因かを判定して negative cache
    // (page_count=0, password_required=true) を書き、scroll で同じ neighbor が
    // 再 enqueue され続けるのを止める。
    // **review #11/#13 対応**: cancel をプール経由で render_page にも渡す。
    // フォルダ移動で bump_epoch されたら、走行中の PDFium 描画も Interrupted で
    // 抜けて Normal 枠を即座に解放する。
    // neighbor prefetch は background なので epoch=0 (UI nav の bump で巻き込まれない)
    // + AbortOnCancel (background は cancel=フォルダ移動意図、harvest 不要)
    let res = match crate::pdf_loader::render_page(
        path,
        0,
        thumb_px,
        None,
        Some(Arc::clone(cancel)),
        crate::pdf_loader::JobPriority::Normal,
        0,
        crate::pdf_loader::CancelWaitPolicy::AbortOnCancel,
    ) {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("{e}");
            if is_password_required_error(&msg) && !meta_present {
                if let Err(e2) = parent_catalog.set_pdf_meta(filename, mtime, file_size, 0, true) {
                    crate::logger::log(format!(
                        "pdf-neighbor-prefetch: negative cache write failed for {filename}: {e2}"
                    ));
                }
            } else {
                crate::logger::log(format!(
                    "pdf-neighbor-prefetch: render failed for {filename}: {msg}"
                ));
            }
            return;
        }
    };

    if !meta_present {
        if let Err(e) =
            parent_catalog.set_pdf_meta(filename, mtime, file_size, res.page_count, false)
        {
            crate::logger::log(format!(
                "pdf-neighbor-prefetch: set_pdf_meta failed for {filename}: {e}"
            ));
        }
    }
    if !thumb_present {
        if let Some(bytes) = encode_and_save(
            &res.image,
            &folder_key,
            parent_catalog,
            mtime,
            file_size,
            thumb_px,
            thumb_quality,
        ) {
            crate::logger::log(format!(
                "pdf-neighbor-prefetch: thumb saved for {filename} ({bytes} bytes)"
            ));
        }
    }
}

// ── enqueue API ──

/// WebP cache hit 経由の catch-up enqueue。`process_load_request` の cache hit 分岐から呼ばれる。
fn maybe_spawn_pdf_meta_catchup(
    req: &LoadRequest,
    catalog: Option<&Arc<crate::catalog::CatalogDb>>,
) {
    let Some(key) = req.cache_key_override.as_deref() else {
        return;
    };
    if !key.starts_with(CACHE_KEY_PDF) {
        return;
    }
    if req.pdf_page.is_none() {
        return;
    }
    let Some(cat_arc) = catalog else {
        return;
    };
    let Some(filename) = req.path.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    // pdf_meta が既に populate 済みなら enqueue 不要
    if cat_arc
        .get_pdf_meta(filename, req.mtime, req.file_size)
        .ok()
        .flatten()
        .is_some()
    {
        return;
    }

    let queue = catchup_queue();
    let mut state = queue.state.lock().unwrap();
    // dedup: 同 path が既に queue or 処理中なら skip
    if state.pending.contains(&req.path) {
        return;
    }
    // capacity guard: low queue 上限を超えていたら drop (= best-effort)
    if state.low.len() >= MAX_CATCHUP_PENDING {
        return;
    }
    state.pending.insert(req.path.clone());
    let cancel = Arc::clone(&state.current_cancel);
    state.low.push_back(CatchupJob {
        path: req.path.clone(),
        kind: CatchupJobKind::MetaOnly {
            catalog: Arc::clone(cat_arc),
        },
        cancel,
    });
    drop(state);
    queue.cv.notify_one();
}

/// 隣接 PDF prefetch enqueue。`load_pdf_as_folder` の最後で呼ばれる。
///
/// 重要: 呼び出し側 (UI スレッド) は **既に warm な catalog (= LRU hit) のみ** をここに
/// 渡すこと。cold catalog の `CatalogDb::open` を UI スレッドで走らせると Enter の体感が
/// 削れる (Codex P3 round 2)。`App::spawn_neighbor_pdf_prefetch_tasks` が `catalog_cache`
/// を `get` で見て hit したものだけ enqueue する。
///
/// **upgrade 動作 (Codex P2 round 3 対応)**: 同 path が既に low (= MetaOnly) に
/// **queue 中** (= 未処理) で居る場合、それを取り除いて high (= NeighborPrefetch) に
/// 積み直す。MetaOnly では page 0 render + WebP 温め + OS cache 温めが行われないので、
/// 用が大きいほうに昇格させる。すでに worker が処理中の path は low queue に居ない
/// ので upgrade できず skip するが、その場合 enumerate は完了済みなので Enter は
/// instant になる (= 失うのは render の事前温めだけ、許容範囲)。
pub fn spawn_pdf_neighbor_prefetch(
    pdf_path: std::path::PathBuf,
    parent_catalog: Arc<crate::catalog::CatalogDb>,
    thumb_px: u32,
    thumb_quality: u8,
) {
    let queue = catchup_queue();
    let mut state = queue.state.lock().unwrap();
    if state.pending.contains(&pdf_path) {
        // 既に queue にあるか、worker が処理中。
        // queue 中の MetaOnly なら NeighborPrefetch に昇格させる (upgrade)。
        // (high 側に同 path がある場合は何もしない、すでに NeighborPrefetch なので)
        //
        // **重要 (Codex P2 round 4 対応)**: 「low から remove する前に high の空きを
        // 確認する」順序にすること。逆順だと high 満杯時に既存の MetaOnly を破棄
        // するだけで終わってしまい、low と high が互いに影響しないという設計
        // メモの不変条件が崩れる (= catch-up を取りこぼす)。
        if state.high.len() < MAX_NEIGHBOR_PENDING {
            if let Some(pos) = state.low.iter().position(|j| j.path == pdf_path) {
                // queued MetaOnly + high に空きあり: 安全に upgrade できる
                state.low.remove(pos);
                let cancel = Arc::clone(&state.current_cancel);
                state.high.push_back(CatchupJob {
                    path: pdf_path,
                    kind: CatchupJobKind::NeighborPrefetch {
                        catalog: parent_catalog,
                        thumb_px,
                        thumb_quality,
                    },
                    cancel,
                });
                // pending は path がそのまま存在し続けるので insert/remove なし
                drop(state);
                queue.cv.notify_one();
                return;
            }
            // low にも居ない (= worker が処理中 or 既に high): upgrade 不可、skip
        }
        // それ以外 (high 満杯、または low に居ない既存 pending):
        //   - high 満杯時: 既存 MetaOnly はそのまま実行させる (catch-up は取りこぼさない)
        //   - 処理中 / high 既存: 既存 job がカバーする
        return;
    }
    if state.high.len() >= MAX_NEIGHBOR_PENDING {
        return;
    }
    state.pending.insert(pdf_path.clone());
    let cancel = Arc::clone(&state.current_cancel);
    state.high.push_back(CatchupJob {
        path: pdf_path,
        kind: CatchupJobKind::NeighborPrefetch {
            catalog: parent_catalog,
            thumb_px,
            thumb_quality,
        },
        cancel,
    });
    drop(state);
    queue.cv.notify_one();
}

/// フォルダ内をスキャンして代表画像のパスを返す。
/// `sort` で指定されたソート順でフォルダブロックと画像ブロックをそれぞれ並べ、
/// サムネイル一覧に近い順序 (フォルダ → 画像) で最初に見つかった画像を選ぶ。
/// サブフォルダ再帰は最大 `remaining_depth` 階層。
///
/// `pin_db` が `Some` のとき、サブフォルダ再帰の各段で「そのサブフォルダ自身に
/// folder_thumb_pin が設定されていないか」を確認する。設定されていれば cascade
/// 解決して leaf 画像があればそれを採用する (= 親 grid に対して**自分の pin を
/// 連鎖的に伝える**動作)。`None` のときは従来の純粋 auto-pick になる。
fn resolve_folder_thumb_image(
    folder: &Path,
    sort: crate::settings::SortOrder,
    remaining_depth: u32,
    pin_db: Option<&crate::folder_thumb_pins::FolderThumbPinDb>,
) -> Option<std::path::PathBuf> {
    let result = resolve_folder_thumb_image_inner(folder, sort, remaining_depth, pin_db);
    // pin 経路の切り分け用診断ログ (= 最上位 entry 点のみ。再帰ステップ内側は出さない)
    crate::logger::log(format!(
        "  resolve_folder_thumb_image: folder={} sort={:?} depth={} pin_aware={} -> {}",
        folder.display(),
        sort,
        remaining_depth,
        pin_db.is_some(),
        result
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<none>".to_string()),
    ));
    result
}

fn resolve_folder_thumb_image_inner(
    folder: &Path,
    sort: crate::settings::SortOrder,
    remaining_depth: u32,
    pin_db: Option<&crate::folder_thumb_pins::FolderThumbPinDb>,
) -> Option<std::path::PathBuf> {
    fn mtime_for_sort(entry: &std::fs::DirEntry, sort: crate::settings::SortOrder) -> i64 {
        match sort {
            crate::settings::SortOrder::DateAsc | crate::settings::SortOrder::DateDesc => entry
                .metadata()
                .ok()
                .map_or(0, |m| crate::ui_helpers::mtime_secs(&m)),
            crate::settings::SortOrder::FileName | crate::settings::SortOrder::Numeric => 0,
        }
    }

    let entries = std::fs::read_dir(folder).ok()?;
    let mut images: Vec<(std::path::PathBuf, i64)> = Vec::new();
    let mut subdirs: Vec<(std::path::PathBuf, i64)> = Vec::new();

    for entry in entries.flatten() {
        // entry.file_type() は FindFirstFile/FindNextFile の戻り値キャッシュを再利用するので
        // per-entry GetFileAttributes syscall が走らない (docs/ui-responsiveness.md §4)。
        // この関数は heavy I/O worker で動くが、大量フォルダで代表画像解決が詰まると
        // 可視サムネ処理が連鎖遅延するので file_type ベースにしておく。
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        let p = entry.path();
        let kind = crate::fs_entry::classify_dir_entry(&entry, &ft);
        if kind.is_directory() {
            let mtime = mtime_for_sort(&entry, sort);
            subdirs.push((p, mtime));
        } else if kind.is_file() {
            if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                if crate::folder_tree::is_recognized_image_ext(&ext.to_ascii_lowercase()) {
                    let mtime = mtime_for_sort(&entry, sort);
                    images.push((p, mtime));
                }
            }
        }
    }

    // サムネイル一覧はフォルダブロックを画像より先に出すため、代表サムネも
    // キャッシュミス時の自動選定ではサブフォルダを先に辿る。
    if remaining_depth > 0 {
        let mut keyed_subdirs: Vec<_> = subdirs
            .into_iter()
            .map(|(path, mtime)| {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let key = sort.name_key(name);
                (path, mtime, key)
            })
            .collect();
        keyed_subdirs
            .sort_by(|(_, a_mt, ak), (_, b_mt, bk)| sort.compare_name_keys(ak, *a_mt, bk, *b_mt));
        subdirs = keyed_subdirs
            .into_iter()
            .map(|(path, mtime, _)| (path, mtime))
            .collect();
        for (sub, _) in &subdirs {
            // pin-aware: サブフォルダ自身に pin があれば cascade 解決して
            // leaf 画像を優先採用する。`folder_thumb_depth` を cascade depth 上限と
            // 兼用する (= 設定値が両方の動作上限になる)。
            if let Some(db) = pin_db {
                if let Some(source) = db.lookup(sub) {
                    let lookup = |p: &std::path::Path| db.lookup(p);
                    if let Some(resolved) =
                        crate::folder_thumb_pins::resolve_pin_target_cascaded_via(
                            sub,
                            &source,
                            lookup,
                            remaining_depth as usize,
                        )
                    {
                        use crate::folder_thumb_pins::ResolvedKind;
                        match resolved.kind {
                            ResolvedKind::Image => {
                                return Some(resolved.abs_path);
                            }
                            ResolvedKind::Folder => {
                                // cascade が pin 無し Folder leaf に到達。
                                // そのフォルダで通常の auto-pick を続ける (pin-aware で)。
                                if let Some(img) = resolve_folder_thumb_image_inner(
                                    &resolved.abs_path,
                                    sort,
                                    remaining_depth - 1,
                                    pin_db,
                                ) {
                                    return Some(img);
                                }
                                // 見つからなければ次のサブフォルダへ
                                continue;
                            }
                            // Video / ZipEntry / PdfPage / ZipFirstImage / PdfFirstPage:
                            // PathBuf として返せないので、pin を尊重できない。
                            // 標準再帰にフォールバックする。
                            _ => {}
                        }
                    }
                }
            }
            // 標準再帰 (pin 無し or 非 Image/Folder pin)
            if let Some(img) =
                resolve_folder_thumb_image_inner(sub, sort, remaining_depth - 1, pin_db)
            {
                return Some(img);
            }
        }
    }

    if !images.is_empty() {
        let mut keyed_images: Vec<_> = images
            .into_iter()
            .map(|(path, mtime)| {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let key = sort.name_key(name);
                (path, mtime, key)
            })
            .collect();
        keyed_images
            .sort_by(|(_, a_mt, ak), (_, b_mt, bk)| sort.compare_name_keys(ak, *a_mt, bk, *b_mt));
        images = keyed_images
            .into_iter()
            .map(|(path, mtime, _)| (path, mtime))
            .collect();
        return Some(images.into_iter().next().unwrap().0);
    }

    None
}

/// 1枚の画像をデコードしてサムネイルを生成し、(条件を満たせば) カタログに保存して
/// チャネルへ送信する。
/// catalog が None の場合はカタログへの保存をスキップする。
/// gen_done は処理完了時にインクリメントする進捗カウンタ。
///
/// 段階 A 以降のフロー:
/// 1. `image::open` でフルデコード
/// 2. **表示用 ColorImage を直接生成してチャネル送信** (UI を先に更新)
/// 3. 段階 C: `CacheDecision` で保存要否を判定
/// 4. 保存対象かつ catalog が指定されていれば WebP エンコード → DB 保存
///
/// 2 → 3/4 の順にすることで、UI 応答性を優先しつつキャッシュも作成する。
/// 表示は元画像から直接生成するため WebP 量子化の画質劣化が無い。
#[allow(clippy::too_many_arguments)]
pub fn load_one_cached(
    path: &Path,
    zip_entry: Option<&str>,
    // プリロード済み ZIP エントリバイト列。Some の場合 read_entry_bytes を省略する。
    // `read_first_image_bytes` で ZIP 1 回 open に統合した場合に使用。
    preloaded_zip_bytes: Option<Vec<u8>>,
    pdf_page: Option<u32>,
    pdf_password: Option<&str>,
    cache_key_override: Option<&str>,
    idx: usize,
    tx: &mpsc::Sender<ThumbMsg>,
    catalog: Option<&crate::catalog::CatalogDb>,
    cache_map: Option<
        &std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
    >,
    mtime: i64,
    file_size: i64,
    gen_done: &Arc<AtomicUsize>,
    thumb_px: u32,
    thumb_quality: u8,
    display_px: u32,
    cache_decision: CacheDecision,
    stats: &Arc<Mutex<crate::stats::ThumbStats>>,
    cancel: Option<&Arc<AtomicBool>>,
    // 可視セルの要求は true。Susie プールへ priority=true で渡され、キュー先頭に挿入される。
    priority: bool,
    // エンキュー元の `LoadRequest::input_seq`。perf 相関用に ThumbMsg にそのまま載せる。
    // 0 は未設定 (計装無効時)。
    input_seq: u64,
    // エンキュー元の `LoadRequest::items_gen`。世代不一致検出用 (Codex P2)。
    items_gen: u64,
    // PDF render pool の context epoch。`req.context_epoch` をそのまま流す。
    // 0 = epoch チェック対象外 (background)。
    context_epoch: u64,
    // PDF render の cancel 時挙動。HarvestOnCancel なら in-flight IPC を harvest して
    // cache 保存に進む (cache savable な PDF render thumbnail のみ)。
    // 詳細は `pdf_loader::CancelWaitPolicy` doc 参照。
    cancel_policy: crate::pdf_loader::CancelWaitPolicy,
    // CachePolicy に関係なく catalog に残す。ユーザー明示ピンなど cache-only 復元が
    // 後続で必要なリクエストに限って使う。
    force_cache: bool,
) {
    // カタログキー (保存・参照で一致させる) と表示名 (ログ用) を分離。
    // process_load_request 側と同じキー形式を使うこと��
    // cache_key_override が Some のとき: フォルダ一覧の ZipFile/PdfFile 用キーを優先。
    let auto_key_buf: String;
    let display_buf: String;
    let (name, display_name): (&str, &str) = if let Some(key) = cache_key_override {
        let dn = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        (key, dn)
    } else if let Some(page_num) = pdf_page {
        auto_key_buf = crate::grid_item::pdf_page_cache_key(page_num);
        display_buf = format!("Page {}", page_num + 1);
        (&auto_key_buf, &display_buf)
    } else if let Some(n) = zip_entry {
        (n, n)
    } else {
        let n = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        (n, n)
    };
    let t = std::time::Instant::now();

    // ── デコード経路 ──
    // 0. PDF ページ:     PDFium でラスタライズ → DynamicImage
    // 1. ZIP エントリ:    ZIP を開いてエントリのバイト列を取り出してから image クレートで decode
    //                     失敗時は WIC (SHCreateMemStream + CreateDecoderFromStream) にフォールバック
    // 2. 通常ファイル:    image クレート (拡張子 → マジックバイトの二段構え)
    //                     失敗時は WIC にフォールバック (HEIC / AVIF / JXL / RAW 等)
    //
    // どのデコーダ経路で成功したかを `decode_source` に記録し、後段の統計に渡す。
    // JPEG パスでは TurboJPEG DCT scale も試み、成功時は `dct_stats` を立てて
    // source_dims を **元寸法** で保存する (DCT scaled buffer ではない)。
    let mut decode_source = crate::stats::DecodeSource::Native;
    let mut dct_stats: Option<ScaleStats> = None;
    let mut zip_orientation: u16 = 1;
    let img_result = if let Some(page_num) = pdf_page {
        // サムネイル用 PDF レンダ: 可視セル (priority=true) は HighNormal、
        // 先読みは Normal。プールの予約ワーカーはフルスクリーン現在ページ
        // (Critical) 用に空けておく。cancel_policy は caller (process_load_request)
        // が cache savable かどうかを判定して渡す。
        let pdf_priority = if priority {
            crate::pdf_loader::JobPriority::HighNormal
        } else {
            crate::pdf_loader::JobPriority::Normal
        };
        crate::pdf_loader::render_page(
            path,
            page_num,
            display_px,
            pdf_password,
            cancel.map(Arc::clone),
            pdf_priority,
            context_epoch,
            cancel_policy,
        )
        .map(|res| {
            // C-thumb (v1.0.0): 親フォルダ内の PDF サムネ render の場合、ついでに
            // pdf_meta テーブルへページ数を書き込む。`cache_key_override` が
            // "pdfthumb:" 始まりのときが「親フォルダ catalog 経路」の signature。
            // PDF 自身を仮想フォルダとして開いている経路 (= cache_key_override None
            // で pdf_page Some) は catalog が PDF 内サムネ用なので skip。
            //
            // **password_required の決定** (Codex P1/P2 follow-up 対応):
            //   - pdf_password=None で render 成功 = 「パスワード不要」確信あり
            //     → `set_pdf_meta_safe` (新規行 OK、既存 password_required は保持)
            //   - pdf_password=Some = session-level pw が居座っているだけかもしれず、
            //     PDF が本当に保護されているか確信できない (= unknown)
            //     → `set_pdf_meta_thumb` (UPDATE only、新規行は作らない)
            // unknown を false-default で挿入すると、保護 PDF が永続的に
            // 「非保護」記録されて次回 placeholder で page 数が露出する bypass を
            // 生むので、新規行は確信できる経路 (= enumerate 成功時) のみで作る。
            if let (Some(cat), Some(key)) = (catalog, cache_key_override) {
                if key.starts_with(CACHE_KEY_PDF) {
                    if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                        let write_result = if pdf_password.is_none() {
                            cat.set_pdf_meta_safe(filename, mtime, file_size, res.page_count)
                        } else {
                            cat.set_pdf_meta_thumb(filename, mtime, file_size, res.page_count)
                        };
                        if let Err(e) = write_result {
                            crate::logger::log(format!(
                                "thumb_loader: pdf_meta write failed for {filename}: {e}"
                            ));
                        }
                    }
                }
            }
            res.image
        })
        .map_err(|e| image::ImageError::IoError(e))
    } else if let Some(entry_name) = zip_entry {
        // プリロード済みバイト列があれば ZIP を再度 open せずにデコード
        let bytes_result = if let Some(bytes) = preloaded_zip_bytes {
            Ok(bytes)
        } else {
            crate::zip_loader::read_entry_bytes(path, entry_name)
                .map_err(image::ImageError::IoError)
        };
        match bytes_result {
            Err(e) => Err(e),
            Ok(bytes) => {
                // ZIP 内 RAW/WIC 系の orientation は rexif で読めないため 1 扱いになる。
                // JPEG 等、rexif が読める EXIF は通常ファイルと同じ向きに揃える。
                zip_orientation = read_exif_orientation_from_bytes(&bytes);
                // JPEG なら TurboJPEG DCT scale で高速デコードを試す
                if is_jpeg_entry(entry_name) {
                    let target_px = display_px.max(thumb_px);
                    match decode_jpeg_turbo_scaled_from_bytes(&bytes, target_px) {
                        Ok((img, stats)) => {
                            dct_stats = Some(stats);
                            Ok(img)
                        }
                        Err(DctDecodeError::TerminalRejection(msg)) => {
                            // ZIP 内 adversarial — fallback すると danger なのでエラー返却
                            crate::logger::log(format!(
                                "DCT terminal rejection ZIP {path:?}/{entry_name}: {msg}"
                            ));
                            Err(image::ImageError::Limits(
                                image::error::LimitError::from_kind(
                                    image::error::LimitErrorKind::InsufficientMemory,
                                ),
                            ))
                        }
                        Err(DctDecodeError::Fallback(_)) => decode_zip_chain(
                            &bytes,
                            entry_name,
                            priority,
                            cancel.cloned(),
                            &mut decode_source,
                        ),
                    }
                } else {
                    decode_zip_chain(
                        &bytes,
                        entry_name,
                        priority,
                        cancel.cloned(),
                        &mut decode_source,
                    )
                }
            }
        }
    } else if is_susie_only_ext(&ext_lower(path)) {
        // Susie 専用拡張子の高速パス: image::open + WIC をスキップして直接 Susie へ。
        // MAG / PI / PIC / Q4 / MAKI 等で 1 枚あたり約 5ms 短縮。
        match crate::susie_loader::decode_file(path, priority, cancel.cloned()) {
            Ok(img) => {
                decode_source = crate::stats::DecodeSource::Susie;
                Ok(img)
            }
            Err(e) => Err(image::ImageError::IoError(e)),
        }
    } else {
        // 通常ファイル: JPEG なら TurboJPEG DCT scale を最初に試す
        let turbo_img: Option<Result<image::DynamicImage, image::ImageError>> = if is_jpeg_ext(path)
        {
            let target_px = display_px.max(thumb_px);
            match decode_jpeg_turbo_scaled_from_path(path, target_px) {
                Ok((img, stats)) => {
                    dct_stats = Some(stats);
                    Some(Ok(img))
                }
                Err(DctDecodeError::TerminalRejection(msg)) => {
                    // adversarial — fallback すると danger。エラーを caller に伝播。
                    crate::logger::log(format!("DCT terminal rejection {path:?}: {msg}"));
                    Some(Err(image::ImageError::Limits(
                        image::error::LimitError::from_kind(
                            image::error::LimitErrorKind::InsufficientMemory,
                        ),
                    )))
                }
                Err(DctDecodeError::Fallback(_)) => None,
            }
        } else {
            None
        };
        if let Some(result) = turbo_img {
            result
        } else {
            let primary = image::open(path).or_else(|_| {
                use std::io::BufReader;
                let f = std::fs::File::open(path)?;
                image::ImageReader::new(BufReader::new(f))
                    .with_guessed_format()
                    .map_err(image::ImageError::IoError)?
                    .decode()
            });
            // image クレートが失敗した場合に WIC → Susie プラグインの順にフォールバック
            // (HEIC / AVIF / JPEG XL / RAW 等は image クレート非対応のため WIC、
            //  PI / MAG / Q0 / PIC / MAKI 等のレトロ形式は Susie プラグインで対応)
            match primary {
                Ok(img) => Ok(img),
                Err(e) => match crate::wic_decoder::decode_to_dynamic_image(path) {
                    Some(img) => {
                        decode_source = crate::stats::DecodeSource::Wic;
                        Ok(img)
                    }
                    None => match crate::susie_loader::decode_file(path, priority, cancel.cloned())
                    {
                        Ok(img) => {
                            decode_source = crate::stats::DecodeSource::Susie;
                            Ok(img)
                        }
                        Err(_) => Err(e),
                    },
                },
            }
        }
    };

    let img = match img_result {
        Ok(i) => i,
        Err(e) => {
            // **PDF render pool の context epoch prune** (`pool_prune_stale_epoch` /
            // `pool_stale_epoch_skip`) や **dispatcher pop 時 cancel skip** は
            // `image::ImageError::IoError(io)` で wrap した `io::ErrorKind::Interrupted`
            // を返す。これらは「systemic な諦め」シグナルなので Failed 化せず silent
            // 経路に乗せる。Susie / ZIP / native decode 由来の Interrupted を巻き込まないよう
            // `pdf_page.is_some()` でガードする (= PDF 経路のみ対象)。
            let pdf_interrupted = pdf_page.is_some()
                && matches!(
                    &e,
                    image::ImageError::IoError(io) if io.kind() == std::io::ErrorKind::Interrupted
                );
            let cancelled = cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed));

            // **Codex P2 対応 (2026-05)**:
            // `pdf_interrupted` は cancel_token を flip せず epoch だけ進めた経路
            // (= `load_folder` 入口の `bump_render_context_epoch` から `start_loading_items` の
            // `invalidate_idx_state_and_queues` までの window) でも発生し得る。この window では
            // 旧フォルダの items_generation が生きており、`requested` が wipe されていない。
            // ここで silent return すると `requested` に dangling entry が残り、Pending のまま
            // 再エンキューが弾かれて (`requested.contains_key=true` で skip) サムネが固着する。
            // STALE 経路 ([docs/async-architecture.md §3.4](docs/async-architecture.md)) と同じく
            // `canceled=true` を送って UI 側 (`poll_thumbnails`) に `requested.remove + Evicted`
            // で掃除させる (Failed 化しない、retriable)。
            //
            // 純粋 cancel (= folder change) 経由はその後の `invalidate_idx_state_and_queues` で
            // `requested` が wipe される上、items_generation も bump されるので `canceled=true`
            // を送る必要は本来ない。ただし両者を区別せず常に送る方がシンプルで、
            // items_gen mismatch で UI 側が無視するので副作用も無い。
            if cancelled || pdf_interrupted {
                let reason = if pdf_interrupted {
                    "pdf-interrupted"
                } else {
                    "cancelled"
                };
                crate::logger::log(format!("    idx={idx:>4} {reason}  {display_name}"));
                let _ = tx.send(ThumbMsg {
                    idx,
                    image: None,
                    from_cache: false,
                    from_edit_preview: false,
                    edit_preview_adjustment: None,
                    source_dims: None,
                    canceled: true,
                    finalized: false,
                    input_seq,
                    items_gen,
                });
                gen_done.fetch_add(1, Ordering::Relaxed);
                return;
            }
            crate::logger::log(format!("    idx={idx:>4} FAIL {e}  {display_name}"));
            let _ = tx.send(ThumbMsg {
                idx,
                image: None,
                from_cache: false,
                from_edit_preview: false,
                edit_preview_adjustment: None,
                source_dims: None,
                canceled: false,
                finalized: false,
                input_seq,
                items_gen,
            });
            gen_done.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut s) = stats.lock() {
                s.record_failed();
            }
            return;
        }
    };

    // EXIF Orientation に基づいて自動回転。
    // ZIP はエントリのバイト列から読み、PDF はレンダ済みページなので常に 1。
    let orientation: u16 = if pdf_page.is_some() {
        1
    } else if zip_entry.is_some() {
        zip_orientation
    } else {
        read_exif_orientation(path)
    };
    let img = apply_orientation(img, orientation);

    let decode_ms = t.elapsed().as_secs_f64() * 1000.0;

    // 元画像のピクセル寸法。
    // DCT scale 経由なら `img.width()/height()` は scaled buffer の寸法なので
    // 使わず、`dct_stats.src_w/src_h` から EXIF 適用後寸法を算出する。
    // 非 DCT 経路 (image::open / WIC / Susie / PDF / ZIP image::load_from_memory)
    // は img の現寸法 = 元寸法 (EXIF 適用済み) なので従来通り。
    let source_dims: Option<(u32, u32)> = if let Some(stats) = dct_stats {
        Some(stats.source_dims_after_exif(orientation))
    } else {
        Some((img.width(), img.height()))
    };

    // DCT スケール経由なら perf event を発火 (`thumb/dct_scale`)。
    // `decode_ms` を含めることで analyze_perf.py で scale_num 別の所要時間を集計可能。
    if let Some(stats) = dct_stats {
        if crate::perf::is_enabled() {
            crate::perf::event(
                "thumb",
                "dct_scale",
                Some(name),
                input_seq,
                &[
                    ("scale_num", serde_json::Value::from(stats.scale_num)),
                    ("src_w", serde_json::Value::from(stats.src_w)),
                    ("src_h", serde_json::Value::from(stats.src_h)),
                    ("out_w", serde_json::Value::from(stats.out_w)),
                    ("out_h", serde_json::Value::from(stats.out_h)),
                    ("decode_ms", serde_json::Value::from(decode_ms)),
                ],
            );
        }
    }

    // (A) 表示用パス: 元画像から直接セルサイズにリサイズして UI へ送信
    //     WebP 量子化を経由しないため画質劣化なし、かつ WebP encode を待たない
    //     from_cache = false: 元画像由来の高画質 (段階 E アップグレード不要)
    let t_display = std::time::Instant::now();
    let display_ci = resize_to_display_color_image(&img, display_px);
    let display_ms = t_display.elapsed().as_secs_f64() * 1000.0;
    // 第 1 シグナル: display ColorImage を UI に送る。UI は Loaded 化するが、
    // from_cache=false のこの経路では `requested` を抜かない (下の cache save が
    // 完了するまで保持する → cache save 中に同じ idx が再エンキューされて
    // 二重レンダする事故を防ぐ)。
    let _ = tx.send(ThumbMsg {
        idx,
        image: Some(display_ci),
        from_cache: false,
        from_edit_preview: false,
        edit_preview_adjustment: None,
        source_dims,
        canceled: false,
        finalized: false,
        input_seq,
        items_gen,
    });

    // 統計: 画像のフルデコード時間・サイズ・フォーマット・デコーダ経路を記録
    {
        // 拡張子の取得元: PDF ページなら "pdf"、ZIP エントリならエントリ名、通常ならファイルパス
        let ext_source: &str = if pdf_page.is_some() {
            "page.pdf"
        } else if let Some(n) = zip_entry {
            n
        } else {
            path.to_str().unwrap_or("")
        };
        let ext = ext_source.rsplit('.').next().unwrap_or("");
        if let Ok(mut s) = stats.lock() {
            s.record_image(
                decode_ms + display_ms,
                file_size.max(0) as u64,
                ext,
                decode_source,
            );
        }
    }

    // (B) キャッシュ保存判定 (段階 C)
    //     catalog 未指定時は保存不可
    //     それ以外は CacheDecision の判定に従う
    let policy_should_save = cache_decision.should_cache(path, file_size, decode_ms, display_ms);
    let should_save = catalog.is_some() && (force_cache || policy_should_save);

    if should_save {
        let cat = catalog.expect("should_save => catalog is Some");
        let t_enc = std::time::Instant::now();
        match crate::catalog::encode_thumb_webp(&img, thumb_px, thumb_quality as f32) {
            Some((webp_data, w, h)) => {
                let encode_ms = t_enc.elapsed().as_secs_f64() * 1000.0;
                if let Err(e) = cat.save(name, mtime, file_size, w, h, source_dims, &webp_data) {
                    crate::logger::log(format!("    idx={idx:>4} catalog save: {e}"));
                } else if let Some(cm) = cache_map {
                    // DB 保存成功 → in-memory cache_map にも反映する。
                    // Evicted → 再ロード時にキャッシュヒットさせるために必要。
                    if let Ok(mut map) = cm.write() {
                        map.insert(
                            name.to_owned(),
                            crate::catalog::CacheEntry {
                                mtime,
                                file_size,
                                jpeg_data: webp_data,
                                source_dims,
                            },
                        );
                    }
                    // **HarvestOnCancel ROI 計測**: cache_map.insert 完了後に
                    // cancel が立っているか確認する (encode/save 中に flip した
                    // ケースも拾う、Codex round 1 P3-1 対応)。立っていれば「投資回収
                    // に成功した」イベントを発火 — pdf_page 経路のみ意味がある。
                    if pdf_page.is_some()
                        && crate::perf::is_enabled()
                        && cancel.is_some_and(|c| c.load(Ordering::Relaxed))
                    {
                        crate::perf::event(
                            "pdf",
                            "pdf_thumb_cache_saved_after_cancel",
                            Some(name),
                            input_seq,
                            &[("idx", serde_json::Value::from(idx))],
                        );
                    }
                }
                let force_note = if force_cache && !policy_should_save {
                    " force-cache"
                } else {
                    ""
                };
                crate::logger::log(format!(
                    "    idx={idx:>4} decode={decode_ms:>6.1}ms display={display_ms:>5.1}ms encode={encode_ms:>5.1}ms{force_note}  {display_name}  -> save_key=`{name}`"
                ));
            }
            None => {
                crate::logger::log(format!("    idx={idx:>4} WebP encode FAIL  {display_name}"));
            }
        }
    } else {
        crate::logger::log(format!(
            "    idx={idx:>4} decode={decode_ms:>6.1}ms display={display_ms:>5.1}ms (skip cache)  {display_name}"
        ));
    }

    // 成功・失敗を問わず完了としてカウント（タイトルバーの進捗に反映）
    gen_done.fetch_add(1, Ordering::Relaxed);

    // 第 2 シグナル: cache save (or skip) 完了を UI に通知し `requested` から抜く。
    // `finalized=true` を立てることで poll_thumbnails 側は **状態を変更せず** requested
    // からの削除のみ行う (texture_backlog で Pending アップロード待ちのケースを保護)。
    let _ = tx.send(ThumbMsg {
        idx,
        image: None,
        from_cache: false,
        from_edit_preview: false,
        edit_preview_adjustment: None,
        source_dims: None,
        canceled: false,
        finalized: true,
        input_seq,
        items_gen,
    });
}

// -----------------------------------------------------------------------
// キャッシュ作成ダイアログ用の非対話版
// -----------------------------------------------------------------------

// -----------------------------------------------------------------------
// テスト
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{CachePolicy, SortOrder};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn make_decision(policy: CachePolicy, threshold_ms: u32, size_bytes: u64) -> CacheDecision {
        CacheDecision {
            policy,
            threshold_ms,
            size_threshold: size_bytes,
            webp_always: true,
            pdf_always: true,
            zip_always: true,
        }
    }

    fn orientation_exif_payload(value: u16) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"Exif\0\0");
        payload.extend_from_slice(b"II");
        payload.extend_from_slice(&0x002A_u16.to_le_bytes());
        payload.extend_from_slice(&8_u32.to_le_bytes());
        payload.extend_from_slice(&1_u16.to_le_bytes());
        payload.extend_from_slice(&0x0112_u16.to_le_bytes());
        payload.extend_from_slice(&3_u16.to_le_bytes());
        payload.extend_from_slice(&1_u32.to_le_bytes());
        payload.extend_from_slice(&value.to_le_bytes());
        payload.extend_from_slice(&0_u16.to_le_bytes());
        payload.extend_from_slice(&0_u32.to_le_bytes());
        payload
    }

    fn jpeg_with_orientation(value: u16) -> Vec<u8> {
        let rgb = image::RgbImage::from_fn(8, 8, |x, y| {
            image::Rgb([(x * 31) as u8, (y * 29) as u8, 128])
        });
        let jpeg = turbojpeg::compress_image(&rgb, 85, turbojpeg::Subsamp::Sub2x2)
            .expect("compress")
            .to_vec();
        let payload = orientation_exif_payload(value);
        let len = payload.len() + 2;
        let mut out = Vec::with_capacity(jpeg.len() + payload.len() + 4);
        out.extend_from_slice(&jpeg[..2]);
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.push((len >> 8) as u8);
        out.push((len & 0xff) as u8);
        out.extend_from_slice(&payload);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[test]
    fn read_exif_orientation_from_bytes_reads_all_values() {
        for orientation in 1..=8 {
            let bytes = jpeg_with_orientation(orientation);
            assert_eq!(super::read_exif_orientation_from_bytes(&bytes), orientation);
        }
    }

    #[test]
    fn apply_exif_orientation_from_bytes_swaps_dimensions() {
        let bytes = jpeg_with_orientation(6);
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(2, 3, |x, y| {
            image::Rgba([(x * 80) as u8, (y * 80) as u8, 0, 255])
        }));

        let rotated = super::apply_exif_orientation_from_bytes(img, &bytes);

        assert_eq!((rotated.width(), rotated.height()), (3, 2));
    }

    #[test]
    fn compute_display_px_clamps_low() {
        // セルサイズ 50 → 50 だが 256 で下限クランプ
        assert_eq!(compute_display_px(50.0, 50.0, 1.0), 256);
        // 0 や負も 256 にクランプ
        assert_eq!(compute_display_px(0.0, 0.0, 1.0), 256);
    }

    #[test]
    fn compute_display_px_clamps_high() {
        // 巨大セル → 2048 で上限クランプ
        assert_eq!(compute_display_px(5000.0, 5000.0, 1.0), 2048);
        // DPI 倍率込みでも上限
        assert_eq!(compute_display_px(2000.0, 2000.0, 2.0), 2048);
    }

    #[test]
    fn compute_display_px_normal_range() {
        // 通常のセルは そのまま物理ピクセル化
        assert_eq!(compute_display_px(400.0, 400.0, 1.0), 400);
        assert_eq!(compute_display_px(400.0, 400.0, 1.5), 600);
        // cell_w と cell_h の最大値を取る
        assert_eq!(compute_display_px(300.0, 500.0, 1.0), 500);
    }

    // ── DCT スケール: 詳細は docs/dct-scale-plan.md ───────────────────────

    #[test]
    fn pick_dct_scale_num_clamps_low() {
        // src 巨大、target 普通 → 最小 scale 1/8
        assert_eq!(super::pick_dct_scale_num(50000, 512), 1);
        assert_eq!(super::pick_dct_scale_num(10000, 512), 1);
    }

    #[test]
    fn pick_dct_scale_num_picks_smallest_above_target() {
        // 5184*1/8 = ceil(648) = 648 >= 512 → M=1
        assert_eq!(super::pick_dct_scale_num(5184, 512), 1);
        // 4000*1/8 = 500 < 512、4000*2/8 = 1000 >= 512 → M=2
        assert_eq!(super::pick_dct_scale_num(4000, 512), 2);
        // 6000*2/8 = 1500 < 2048、6000*3/8 = 2250 >= 2048 → M=3
        assert_eq!(super::pick_dct_scale_num(6000, 2048), 3);
        // turbojpeg の ceil rounding 境界: 1023*4/8 = ceil(511.5) = 512 = target → M=4
        // (旧 formula `ceil(8*target/src)` は M=5 を返してしまう)
        assert_eq!(super::pick_dct_scale_num(1023, 512), 4);
        // 4095*1/8 = ceil(511.875) = 512 >= 512 → M=1
        assert_eq!(super::pick_dct_scale_num(4095, 512), 1);
    }

    #[test]
    fn pick_dct_scale_num_clamps_high() {
        // src が target 未満 → scaling 不可、M=8 (= 1/1)
        assert_eq!(super::pick_dct_scale_num(500, 512), 8);
        assert_eq!(super::pick_dct_scale_num(100, 512), 8);
    }

    #[test]
    fn pick_dct_scale_num_exact_match() {
        // src == target → M=8 (scaling 不要)
        assert_eq!(super::pick_dct_scale_num(512, 512), 8);
        // src = 8*target → 1/8 でちょうど = target
        assert_eq!(super::pick_dct_scale_num(4096, 512), 1);
    }

    #[test]
    fn pick_dct_scale_num_safe_against_overflow() {
        // u32::MAX 入力 → overflow せず安全
        assert_eq!(super::pick_dct_scale_num(u32::MAX, 2048), 1);
        // src=0 → 0-division 回避、M=8
        assert_eq!(super::pick_dct_scale_num(0, 512), 8);
        // target=0 → M=1 (最小)
        assert_eq!(super::pick_dct_scale_num(5184, 0), 1);
    }

    #[test]
    fn scale_stats_source_dims_after_exif_no_swap() {
        let s = super::ScaleStats {
            src_w: 5184,
            src_h: 3888,
            scale_num: 1,
            out_w: 648,
            out_h: 486,
        };
        // orientation 1-4 は w/h 維持
        assert_eq!(s.source_dims_after_exif(1), (5184, 3888));
        assert_eq!(s.source_dims_after_exif(2), (5184, 3888));
        assert_eq!(s.source_dims_after_exif(3), (5184, 3888));
        assert_eq!(s.source_dims_after_exif(4), (5184, 3888));
    }

    #[test]
    fn scale_stats_source_dims_after_exif_swap() {
        let s = super::ScaleStats {
            src_w: 5184,
            src_h: 3888,
            scale_num: 1,
            out_w: 648,
            out_h: 486,
        };
        // orientation 5-8 は 90°/270° 系で w/h swap
        assert_eq!(s.source_dims_after_exif(5), (3888, 5184));
        assert_eq!(s.source_dims_after_exif(6), (3888, 5184));
        assert_eq!(s.source_dims_after_exif(7), (3888, 5184));
        assert_eq!(s.source_dims_after_exif(8), (3888, 5184));
    }

    #[test]
    fn decode_jpeg_turbo_scaled_normal_jpeg_returns_orig_dims() {
        // 5184x3888 baseline JPEG を runtime 生成 → DCT 1/8 で decode、source_dims
        // が元寸法と一致することを検証 (Codex 2nd round P1 対応: source_dims 契約)。
        let src_w = 5184u32;
        let src_h = 3888u32;
        let rgb = image::RgbImage::from_fn(src_w, src_h, |x, y| {
            image::Rgb([
                ((x ^ y) & 0xff) as u8,
                ((x.wrapping_mul(2)) & 0xff) as u8,
                ((y.wrapping_mul(2)) & 0xff) as u8,
            ])
        });
        let bytes =
            turbojpeg::compress_image(&rgb, 85, turbojpeg::Subsamp::Sub2x2).expect("compress");
        let (img, stats) =
            super::decode_jpeg_turbo_scaled_from_bytes(&bytes, 512).expect("DCT decode ok");
        // 元寸法は header から正しく取れる
        assert_eq!(stats.src_w, src_w);
        assert_eq!(stats.src_h, src_h);
        // pick_dct_scale_num(5184, 512) = 1 → 1/8 scale
        assert_eq!(stats.scale_num, 1);
        // ceil(5184 * 1 / 8) = 648, ceil(3888 * 1 / 8) = 486
        assert_eq!(stats.out_w, 648);
        assert_eq!(stats.out_h, 486);
        assert_eq!(img.width(), 648);
        assert_eq!(img.height(), 486);
        // EXIF=1 (no swap) で source_dims が元寸法
        assert_eq!(stats.source_dims_after_exif(1), (src_w, src_h));
    }

    #[test]
    fn decode_jpeg_turbo_scaled_target_above_src_uses_full_decode() {
        // src < target なら scale=1/1 (M=8) で full decode
        let src_w = 400u32;
        let src_h = 300u32;
        let rgb = image::RgbImage::from_fn(src_w, src_h, |x, y| {
            image::Rgb([(x & 0xff) as u8, (y & 0xff) as u8, 128])
        });
        let bytes =
            turbojpeg::compress_image(&rgb, 85, turbojpeg::Subsamp::Sub2x2).expect("compress");
        let (img, stats) =
            super::decode_jpeg_turbo_scaled_from_bytes(&bytes, 1024).expect("DCT decode ok");
        assert_eq!(stats.scale_num, 8); // M=8 = 1/1
        assert_eq!(img.width(), src_w);
        assert_eq!(img.height(), src_h);
    }

    /// 小さな real JPEG を生成してから SOF0 マーカー (FF C0) の width/height
    /// フィールドを書き換える test fixture helper。
    ///
    /// libjpeg-turbo の `read_header` は SOF を parse して dims を返すので、
    /// この helper で「巨大な header dims を主張する偽 JPEG」を作れる。
    /// decode 自体は body と不整合で失敗するが、本来は MAX_DECODED_BYTES guard が
    /// それより先に発火して TerminalRejection を返すべき。
    ///
    /// JPEG SOF0 構造 (RFC):
    /// `FF C0 <len:2> <precision:1> <height:2 big-endian> <width:2 big-endian> ...`
    fn mutate_jpeg_sof_dims(bytes: &mut Vec<u8>, new_w: u16, new_h: u16) {
        // SOF0 (FF C0) または SOF3 (FF C3) マーカーを探す
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i] == 0xFF && (bytes[i + 1] == 0xC0 || bytes[i + 1] == 0xC3) {
                // marker(2) + len(2) + precision(1) = 5 bytes 後に height
                let h_off = i + 5;
                let w_off = i + 7;
                if w_off + 1 < bytes.len() {
                    bytes[h_off] = (new_h >> 8) as u8;
                    bytes[h_off + 1] = (new_h & 0xff) as u8;
                    bytes[w_off] = (new_w >> 8) as u8;
                    bytes[w_off + 1] = (new_w & 0xff) as u8;
                    return;
                }
            }
            i += 1;
        }
        panic!("SOF marker not found");
    }

    #[test]
    fn decode_bytes_rejects_oversized_output_with_terminal() {
        // 小さな実 JPEG を作って SOF dims を 65500x65500 (= libjpeg-turbo の最大
        // サポート寸法、JPEG 仕様の 65535 ではなく実装上の上限) に書き換える。
        // target_px=10000 にすると pick_dct_scale_num = 2 (= 1/4)、出力は
        // ceil(65500*2/8) = 16375 px square = 16375*16375*3 ≈ 804 MB >
        // MAX_DECODED_BYTES (256MB)。allocation 前に TerminalRejection で弾かれること。
        //
        // (target_px=512 の場合は M=1 で出力 ≈8188 px square ≈ 201 MB で guard 内に
        //  収まるので、ここでは target_px=10000 にして M=2 を強制する。)
        let rgb = image::RgbImage::from_fn(16, 16, |x, y| {
            image::Rgb([(x * 16) as u8, (y * 16) as u8, 128])
        });
        let mut bytes = turbojpeg::compress_image(&rgb, 85, turbojpeg::Subsamp::Sub2x2)
            .expect("compress")
            .to_vec();
        mutate_jpeg_sof_dims(&mut bytes, 65500, 65500);

        let result = super::decode_jpeg_turbo_scaled_from_bytes(&bytes, 10000);
        assert!(
            matches!(result, Err(super::DctDecodeError::TerminalRejection(_))),
            "expected TerminalRejection, got {:?}",
            result.as_ref().map(|_| "Ok").or_else(|e| Err(e))
        );
    }

    #[test]
    fn decode_path_rejects_oversized_input_with_fallback() {
        // 圧縮入力 > MAX_TURBOJPEG_INPUT_SIZE (128 MB) は Fallback を返す。
        // tempfile を sparse に拡張 (set_len) して metadata.len() の guard 経路だけ
        // 確認する。実バイトは書き込まないので disk I/O / AV scanner と無関係に走る。
        // read_header には到達せず、早期 Fallback で抜けることを検証。
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        tmp.as_file()
            .set_len(super::MAX_TURBOJPEG_INPUT_SIZE + 1)
            .expect("set_len");

        let result = super::decode_jpeg_turbo_scaled_from_path(tmp.path(), 512);
        match result {
            Err(super::DctDecodeError::Fallback(msg)) => {
                assert!(
                    msg.contains("too large"),
                    "expected size-related Fallback, got: {msg}"
                );
            }
            other => panic!("expected Fallback, got {other:?}"),
        }
    }

    #[test]
    fn cache_decision_always_returns_true() {
        let d = make_decision(CachePolicy::Always, 25, 2_000_000);
        let p = PathBuf::from("foo.jpg");
        assert!(d.should_cache(&p, 100, 0.0, 0.0));
        assert!(d.should_cache(&p, 0, 0.0, 0.0));
    }

    #[test]
    fn cache_decision_off_returns_false() {
        let d = make_decision(CachePolicy::Off, 25, 2_000_000);
        let p = PathBuf::from("huge.jpg");
        assert!(!d.should_cache(&p, 100_000_000, 999.0, 999.0));
    }

    #[test]
    fn force_cache_saves_even_when_policy_off() {
        let tmp = TempDir::new().expect("tempdir");
        let img_path = tmp.path().join("img.png");
        let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([32, 64, 128, 255]));
        image::DynamicImage::ImageRgba8(img)
            .save(&img_path)
            .expect("write image");

        let cache_dir = tmp.path().join("cache");
        let catalog = crate::catalog::CatalogDb::open(&cache_dir, tmp.path()).unwrap();
        let cache_map = std::sync::RwLock::new(std::collections::HashMap::new());
        let (tx, _rx) = std::sync::mpsc::channel();
        let gen_done = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stats = std::sync::Arc::new(std::sync::Mutex::new(crate::stats::ThumbStats::default()));

        load_one_cached(
            &img_path,
            None,
            None,
            None,
            None,
            None,
            0,
            &tx,
            Some(&catalog),
            Some(&cache_map),
            123,
            0,
            &gen_done,
            64,
            75,
            64,
            make_decision(CachePolicy::Off, 25, 2_000_000),
            &stats,
            None,
            true,
            1,
            1,
            0,
            crate::pdf_loader::CancelWaitPolicy::AbortOnCancel,
            true,
        );

        let entry = catalog
            .load_one("img.png")
            .unwrap()
            .expect("force_cache should write catalog entry");
        assert_eq!(entry.mtime, 123);
        assert_eq!(entry.file_size, 0);
        assert!(cache_map.read().unwrap().contains_key("img.png"));
    }

    #[test]
    fn cache_decision_auto_uses_size_threshold() {
        let d = make_decision(CachePolicy::Auto, 25, 2_000_000);
        let p = PathBuf::from("foo.jpg");
        // サイズが 2 MB 以上ならキャッシュ
        assert!(d.should_cache(&p, 2_000_000, 0.0, 0.0));
        assert!(d.should_cache(&p, 5_000_000, 0.0, 0.0));
        // サイズが小さく、時間も短ければキャッシュなし
        assert!(!d.should_cache(&p, 100_000, 5.0, 5.0));
    }

    #[test]
    fn cache_decision_auto_uses_time_threshold() {
        let d = make_decision(CachePolicy::Auto, 25, 100_000_000);
        let p = PathBuf::from("foo.jpg");
        // 合計時間 < 25 ms → キャッシュなし
        assert!(!d.should_cache(&p, 100, 10.0, 10.0));
        // 合計時間 == 25 ms → キャッシュ
        assert!(d.should_cache(&p, 100, 12.0, 13.0));
        // 合計時間 > 25 ms → キャッシュ
        assert!(d.should_cache(&p, 100, 30.0, 0.0));
    }

    #[test]
    fn cache_decision_auto_webp_always_caches() {
        let d = make_decision(CachePolicy::Auto, 25, 100_000_000);
        let webp = PathBuf::from("img.webp");
        // .webp は常にキャッシュ (size/time 関係なし)
        assert!(d.should_cache(&webp, 100, 0.0, 0.0));
        // 大文字 .WEBP も同じ
        let webp_upper = PathBuf::from("IMG.WEBP");
        assert!(d.should_cache(&webp_upper, 100, 0.0, 0.0));
    }

    #[test]
    fn cache_decision_auto_webp_can_be_disabled() {
        let mut d = make_decision(CachePolicy::Auto, 25, 100_000_000);
        d.webp_always = false;
        let webp = PathBuf::from("img.webp");
        assert!(!d.should_cache(&webp, 100, 0.0, 0.0));
    }

    #[test]
    fn folder_thumb_auto_cache_key_includes_policy_version() {
        let numeric = folder_thumb_auto_cache_key("folder", SortOrder::Numeric, 3);
        let name = folder_thumb_auto_cache_key("folder", SortOrder::FileName, 3);
        let depth = folder_thumb_auto_cache_key("folder", SortOrder::Numeric, 4);

        assert!(numeric.starts_with(CACHE_KEY_FOLDER));
        assert!(numeric.contains("auto-v2:numeric:d3:folder"));
        assert_ne!(numeric, name);
        assert_ne!(numeric, depth);
    }

    #[test]
    fn resolve_folder_thumb_sorts_subdirs_numerically() {
        let tmp = TempDir::new().unwrap();
        let dir10 = tmp.path().join("dir10");
        let dir2 = tmp.path().join("dir2");
        std::fs::create_dir_all(&dir10).unwrap();
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::write(dir10.join("a.jpg"), b"not decoded").unwrap();
        let expected = dir2.join("a.jpg");
        std::fs::write(&expected, b"not decoded").unwrap();

        let picked = resolve_folder_thumb_image(tmp.path(), SortOrder::Numeric, 1, None);

        assert_eq!(picked, Some(expected));
    }

    #[test]
    fn resolve_folder_thumb_sorts_subdirs_by_date_desc() {
        let tmp = TempDir::new().unwrap();
        let old_dir = tmp.path().join("old");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("a.jpg"), b"not decoded").unwrap();

        // mtime_secs は秒精度なので、日付順の差が確実に出るまで待つ。
        std::thread::sleep(std::time::Duration::from_millis(1_100));

        let new_dir = tmp.path().join("new");
        std::fs::create_dir_all(&new_dir).unwrap();
        let expected = new_dir.join("a.jpg");
        std::fs::write(&expected, b"not decoded").unwrap();

        let picked = resolve_folder_thumb_image(tmp.path(), SortOrder::DateDesc, 1, None);

        assert_eq!(picked, Some(expected));
    }

    #[test]
    fn resolve_folder_thumb_prefers_folder_block_before_direct_images() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("01-sub");
        std::fs::create_dir_all(&sub).unwrap();
        let expected = sub.join("09.jpg");
        std::fs::write(&expected, b"not decoded").unwrap();
        std::fs::write(tmp.path().join("00.jpg"), b"not decoded").unwrap();

        let picked = resolve_folder_thumb_image(tmp.path(), SortOrder::Numeric, 1, None);

        assert_eq!(picked, Some(expected));
    }

    #[test]
    fn resolve_folder_thumb_depth_zero_uses_direct_images() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("01-sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("00.jpg"), b"not decoded").unwrap();
        let expected = tmp.path().join("01.jpg");
        std::fs::write(&expected, b"not decoded").unwrap();

        let picked = resolve_folder_thumb_image(tmp.path(), SortOrder::Numeric, 0, None);

        assert_eq!(picked, Some(expected));
    }
}

/// 画像1枚をデコード・エンコード・カタログ保存する。成功時は WebP バイト数を返す。
/// load_one_cached と違い、mpsc 送信・ログ出力・進捗更新は行わないバッチ処理専用版。
pub fn build_and_save_one(
    path: &Path,
    catalog: &crate::catalog::CatalogDb,
    mtime: i64,
    file_size: i64,
    thumb_px: u32,
    thumb_quality: u8,
) -> Option<usize> {
    // JPEG なら TurboJPEG DCT scale を先に試す (load_one_cached と同じ方針)。
    // cache creator は UI セルサイズに依存しない bulk worker なので target は thumb_px。
    let mut dct_stats: Option<ScaleStats> = None;
    let turbo_img = if is_jpeg_ext(path) {
        match decode_jpeg_turbo_scaled_from_path(path, thumb_px) {
            Ok((img, stats)) => {
                dct_stats = Some(stats);
                Some(img)
            }
            Err(DctDecodeError::TerminalRejection(msg)) => {
                crate::logger::log(format!(
                    "build_and_save_one: DCT terminal rejection {path:?}: {msg}"
                ));
                return None;
            }
            Err(DctDecodeError::Fallback(_)) => None,
        }
    } else {
        None
    };
    // 拡張子ベース → マジックバイト fallback（load_one_cached と同じ方針）
    let img = match turbo_img {
        Some(img) => img,
        None => image::open(path)
            .or_else(|_| {
                use std::io::BufReader;
                let f = std::fs::File::open(path)?;
                image::ImageReader::new(BufReader::new(f))
                    .with_guessed_format()
                    .map_err(image::ImageError::IoError)?
                    .decode()
            })
            .ok()?,
    };

    // EXIF orientation を適用 (load_one_cached と挙動を揃える)。
    // 旧 build_and_save_one は orientation 未適用だったが、catalog に保存する
    // source_dims が load_one_cached 経由と不一致になる既存バグ。本プランで修正。
    let orientation = read_exif_orientation(path);
    let img = apply_orientation(img, orientation);
    let source_dims = dct_stats.map(|s| s.source_dims_after_exif(orientation));

    let name = path.file_name()?.to_str()?;
    encode_and_save_with_source_dims(
        &img,
        source_dims,
        name,
        catalog,
        mtime,
        file_size,
        thumb_px,
        thumb_quality,
    )
}

/// デコード済み画像を WebP エンコードしてカタログに保存する共通ヘルパー。
///
/// `source_dims` は `img` の現寸法 (= DCT scale 適用前提なら元寸法ではないので注意)。
/// DCT scale 経由で渡される caller は `encode_and_save_with_source_dims` を使う。
pub fn encode_and_save(
    img: &image::DynamicImage,
    key: &str,
    catalog: &crate::catalog::CatalogDb,
    mtime: i64,
    file_size: i64,
    thumb_px: u32,
    thumb_quality: u8,
) -> Option<usize> {
    encode_and_save_with_source_dims(
        img,
        None,
        key,
        catalog,
        mtime,
        file_size,
        thumb_px,
        thumb_quality,
    )
}

/// `encode_and_save` の source_dims override 付き版。
///
/// DCT スケール経由で `img` が縮小済みの場合、catalog の `source_dims` は
/// **元寸法** (decode 前の寸法) を保存する必要があるため、caller が
/// `ScaleStats.source_dims_after_exif()` で算出した override 値を渡す。
///
/// `source_dims_override = None` のときは旧 `encode_and_save` と同じ動作で
/// `(img.width(), img.height())` を保存する。
pub fn encode_and_save_with_source_dims(
    img: &image::DynamicImage,
    source_dims_override: Option<(u32, u32)>,
    key: &str,
    catalog: &crate::catalog::CatalogDb,
    mtime: i64,
    file_size: i64,
    thumb_px: u32,
    thumb_quality: u8,
) -> Option<usize> {
    let source_dims = source_dims_override.or(Some((img.width(), img.height())));
    let (webp_data, w, h) = crate::catalog::encode_thumb_webp(img, thumb_px, thumb_quality as f32)?;
    catalog
        .save(key, mtime, file_size, w, h, source_dims, &webp_data)
        .ok()?;
    Some(webp_data.len())
}

/// ZIP 内の画像エントリ1つをデコードしてキャッシュに保存する。
/// バッチキャッシュ作成用。**現状 unused** だが、将来再利用に備えて
/// DCT scale 経路と source_dims override 対応を入れてある (= source 監査時の整合性維持)。
pub fn build_and_save_one_zip(
    zip_path: &Path,
    entry_name: &str,
    catalog: &crate::catalog::CatalogDb,
    mtime: i64,
    file_size: i64,
    thumb_px: u32,
    thumb_quality: u8,
) -> Option<usize> {
    let bytes = crate::zip_loader::read_entry_bytes(zip_path, entry_name).ok()?;
    // JPEG なら DCT scale を試す + 元寸法を source_dims に保存
    let orientation = read_exif_orientation_from_bytes(&bytes);
    let (img, source_dims) = if is_jpeg_entry(entry_name) {
        match decode_jpeg_turbo_scaled_from_bytes(&bytes, thumb_px) {
            Ok((img, stats)) => (Some(img), Some(stats.source_dims_after_exif(orientation))),
            Err(DctDecodeError::TerminalRejection(_)) => return None,
            Err(DctDecodeError::Fallback(_)) => (None, None),
        }
    } else {
        (None, None)
    };
    let img = img.or_else(|| image::load_from_memory(&bytes).ok())?;
    let img = apply_orientation(img, orientation);
    encode_and_save_with_source_dims(
        &img,
        source_dims,
        entry_name,
        catalog,
        mtime,
        file_size,
        thumb_px,
        thumb_quality,
    )
}

/// PDF の1ページをレンダリングしてキャッシュに保存する。
/// バッチキャッシュ作成用。
pub fn build_and_save_one_pdf(
    pdf_path: &Path,
    page_num: u32,
    password: Option<&str>,
    catalog: &crate::catalog::CatalogDb,
    mtime: i64,
    file_size: i64,
    thumb_px: u32,
    thumb_quality: u8,
) -> Option<usize> {
    // バッチキャッシュ作成は Normal 優先度 + 非 UI 経路なので epoch=0:
    // フルスクリーン操作より優先されない、かつ UI nav の bump で巻き込まれない
    // + AbortOnCancel (bulk は cancel=明示中断意図)
    let res = crate::pdf_loader::render_page(
        pdf_path,
        page_num,
        thumb_px,
        password,
        None,
        crate::pdf_loader::JobPriority::Normal,
        0,
        crate::pdf_loader::CancelWaitPolicy::AbortOnCancel,
    )
    .ok()?;
    let key = crate::grid_item::pdf_page_cache_key(page_num);
    encode_and_save(
        &res.image,
        &key,
        catalog,
        mtime,
        file_size,
        thumb_px,
        thumb_quality,
    )
}
