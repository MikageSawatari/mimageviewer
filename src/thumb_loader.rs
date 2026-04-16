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
use std::sync::{mpsc, Arc, Mutex};

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
/// カタログ内のフォルダサムネイル用キャッシュキープレフィックス
pub const CACHE_KEY_FOLDER: &str = "folderthumb:";

// -----------------------------------------------------------------------
// 共通型
// -----------------------------------------------------------------------

/// サムネイル読み込み結果メッセージ。
///
/// `(item_idx, Option<ColorImage>, from_cache, source_dims)`
/// - `from_cache = true`: WebP キャッシュから復元 (段階 E アップグレード対象)
/// - `from_cache = false`: 元画像から直接デコード (高画質) または動画 Shell API
/// - `source_dims`: 元画像のピクセル寸法 (幅, 高さ)。取得できなかった場合は None
pub type ThumbMsg = (usize, Option<egui::ColorImage>, bool, Option<(u32, u32)>);

/// 段階 B: サムネイル読み込み要求。
///
/// UI スレッドが `reload_queue` に push し、永続ワーカースレッドが pop して処理する。
/// ワーカーはまず `cache_map` を参照し、ヒットすれば WebP デコード、
/// ミスすれば `load_one_cached` に委譲する。
pub struct LoadRequest {
    pub idx: usize,
    /// 通常画像ならファイルパス、ZIP 画像なら ZIP ファイルのパス
    pub path: std::path::PathBuf,
    pub mtime: i64,
    pub file_size: i64,
    /// 段階 E: true の場合はキャッシュを無視して元画像から再デコードする
    pub skip_cache: bool,
    /// true = 画面上に見えている可視範囲のアイテム。ワーカーは priority 要求を
    /// 先読み要求より常に先に処理する。
    pub priority: bool,
    /// タスク 3: `Some(name)` なら ZIP エントリとして読む。
    /// `path` が ZIP ファイル、`name` が内部エントリ名。
    pub zip_entry: Option<String>,
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
            CachePolicy::Off    => false,
            CachePolicy::Auto   => {
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
pub fn resize_to_display_color_image(
    img: &image::DynamicImage,
    display_px: u32,
) -> egui::ColorImage {
    let resized = img.resize(
        display_px,
        display_px,
        image::imageops::FilterType::Lanczos3,
    );
    let rgba = resized.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
}

/// 画像ファイルをデコードし、指定サイズにリサイズした ColorImage を返す。
/// 動画の同名画像サムネイルオーバーライド用。
pub fn decode_image_for_thumb(
    path: &std::path::Path,
    display_px: u32,
) -> Option<egui::ColorImage> {
    // JPEG なら TurboJPEG で高速デコードを試す
    let img = if is_jpeg_ext(path) {
        decode_jpeg_turbo_from_path(path)
    } else {
        None
    };
    let img = img.or_else(|| image::open(path).ok())
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
    rexif::parse_file(path.to_str()?)
        .ok()
        .and_then(|exif| {
            exif.entries
                .iter()
                .find(|e| e.ifd.tag == 274)
                .and_then(|e| e.value_more_readable.trim().parse::<u16>().ok()
                    .or_else(|| orientation_from_text(&e.value_more_readable)))
        })
}

fn read_exif_orientation_from_bytes(bytes: &[u8]) -> u16 {
    rexif::parse_buffer(bytes)
        .ok()
        .and_then(|exif| {
            exif.entries
                .iter()
                .find(|e| e.ifd.tag == 274)
                .and_then(|e| e.value_more_readable.trim().parse::<u16>().ok()
                    .or_else(|| orientation_from_text(&e.value_more_readable)))
        })
        .unwrap_or(1)
}

/// rexif の value_more_readable テキストから Orientation 値を推測する
fn orientation_from_text(text: &str) -> Option<u16> {
    let t = text.to_lowercase();
    if t.contains("straight") || t.contains("normal") { return Some(1); }
    if t.contains("rotated to left") || t.contains("90 cw") { return Some(6); }
    if t.contains("upside down") || t.contains("180") { return Some(3); }
    if t.contains("rotated to right") || t.contains("270 cw") || t.contains("90 ccw") { return Some(8); }
    if t.contains("mirrored horizontally") { return Some(2); }
    if t.contains("mirrored vertically") { return Some(4); }
    None
}

// -----------------------------------------------------------------------
// TurboJPEG 高速デコード
// -----------------------------------------------------------------------

/// TurboJPEG を使うファイルサイズ上限 (5 MB)。
/// 大容量カメラ JPEG (10-30MB) では `std::fs::read()` の全読み込みコストが
/// `image::open()` のストリーミングデコードを上回るため、通常パスに任せる。
/// ZIP 内 JPEG は既にメモリ上にあるためこの制限は適用しない。
const TURBOJPEG_FILE_SIZE_LIMIT: u64 = 5 * 1024 * 1024;

/// JPEG ファイルを TurboJPEG (libjpeg-turbo) でデコードする。
/// SIMD 最適化により image クレートの純粋 Rust デコーダーより 2-4 倍高速。
/// ファイルサイズが `TURBOJPEG_FILE_SIZE_LIMIT` を超える場合は None を返し、
/// 呼び出し側で image クレートにフォールバックさせる。
fn decode_jpeg_turbo_from_path(path: &Path) -> Option<image::DynamicImage> {
    // 大容量ファイルは image::open() のストリーミングデコードに任せる
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > TURBOJPEG_FILE_SIZE_LIMIT {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    decode_jpeg_turbo_from_bytes(&data)
}

/// バイト列から JPEG を TurboJPEG でデコードする（ZIP 内 JPEG 用）。
fn decode_jpeg_turbo_from_bytes(data: &[u8]) -> Option<image::DynamicImage> {
    let img: image::RgbImage = turbojpeg::decompress_image(data).ok()?;
    Some(image::DynamicImage::ImageRgb8(img))
}

const JPEG_EXTENSIONS: &[&str] = &["jpg", "jpeg", "jpe", "jfif"];

fn is_jpeg_extension(ext: &str) -> bool {
    let lower = ext.to_ascii_lowercase();
    JPEG_EXTENSIONS.iter().any(|&e| e == lower)
}

fn is_jpeg_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|s| is_jpeg_extension(s))
}

fn is_jpeg_entry(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|s| is_jpeg_extension(s))
}

fn apply_orientation(img: image::DynamicImage, orientation: u16) -> image::DynamicImage {
    match orientation {
        1 => img,                                          // 正常
        2 => img.fliph(),                                  // 左右反転
        3 => img.rotate180(),                              // 180°
        4 => img.flipv(),                                  // 上下反転
        5 => img.rotate90().fliph(),                       // 転置
        6 => img.rotate90(),                               // 90° CW
        7 => img.rotate90().flipv(),                       // 転置 + 反転
        8 => img.rotate270(),                              // 270° CW
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
    catalog: Option<&crate::catalog::CatalogDb>,
    thumb_px: u32,
    thumb_quality: u8,
    display_px: u32,
    cache_decision: CacheDecision,
    gen_done: &Arc<AtomicUsize>,
    stats: &Arc<Mutex<crate::stats::ThumbStats>>,
    cancel: Option<&Arc<AtomicBool>>,
    keep_start: &Arc<AtomicUsize>,
    keep_end: &Arc<AtomicUsize>,
) {
    // カタログキー:
    // - 通常画像: ファイル名 (例: "foo.jpg")
    // - ZIP エントリ: エントリ名 (例: "work1/img01.jpg") 丸ごと
    //   ZIP ごとに別 DB が開かれるため、DB 内で一意
    // - cache_key_override: フォルダ一覧の ZipFile/PdfFile 用
    // PDF ページの場合はキー名を生成する必要があるので owned を保持
    let auto_key: String;
    let filename: &str = if let Some(ref key) = req.cache_key_override {
        key.as_str()
    } else if let Some(page_num) = req.pdf_page {
        auto_key = crate::grid_item::pdf_page_cache_key(page_num);
        &auto_key
    } else if let Some(ref name) = req.zip_entry {
        name.as_str()
    } else {
        req.path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
    };

    let req_t0 = std::time::Instant::now();

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
            let _ = tx.send((req.idx, ci, true, source_dims));
            gen_done.fetch_add(1, Ordering::Relaxed);
            crate::logger::log(format!(
                "    idx={:>4} cache_hit={cache_ms:>5.1}ms  {filename}",
                req.idx,
            ));
            return;
        }
    }

    // キャッシュミス or skip_cache: フルデコード (+ 必要なら保存)
    // load_one_cached は from_cache = false を送信する

    // 重い I/O (ZIP/Folder) は専用 I/O ワーカーキューで処理されるため、
    // セマフォは不要。I/O ワーカー数 (1-2) で自然に同時実行数が制限される。
    let is_folder_thumb = req.cache_key_override.as_deref()
        .is_some_and(|k| k.starts_with(CACHE_KEY_FOLDER));
    let is_zip_thumb = !is_folder_thumb
        && req.zip_entry.is_none()
        && req.cache_key_override.is_some()
        && req.pdf_page.is_none();
    let needs_heavy_io = is_folder_thumb || is_zip_thumb;

    // フォルダサムネイル: フォルダ内の画像を探して代表画像のパスに差し替え
    let resolved_folder_image = if is_folder_thumb {
        let t_resolve = std::time::Instant::now();
        let img = resolve_folder_thumb_image(
            &req.path,
            req.folder_thumb_sort.unwrap_or(crate::settings::SortOrder::Numeric),
            req.folder_thumb_depth,
        );
        let resolve_ms = t_resolve.elapsed().as_secs_f64() * 1000.0;
        if resolve_ms > 10.0 {
            crate::logger::log(format!(
                "    idx={:>4} folder_resolve={resolve_ms:>6.1}ms  {}",
                req.idx, req.path.display(),
            ));
        }
        if img.is_none() {
            let _ = tx.send((req.idx, None, false, None));
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
    } else if is_zip_thumb {
        // cache_key_override あり + pdf_page なし + フォルダでない = ZipFile サムネイル
        // ZIP を 1 回だけ開いてエントリ名 + バイト列を同時取得
        let t_zip = std::time::Instant::now();
        match crate::zip_loader::read_first_image_bytes(&req.path) {
            Some((name, bytes)) => {
                let zip_ms = t_zip.elapsed().as_secs_f64() * 1000.0;
                crate::logger::log(format!(
                    "    idx={:>4} zip_resolve={zip_ms:>6.1}ms  ({} bytes)  {}",
                    req.idx, bytes.len(), req.path.display(),
                ));
                resolved_zip_entry = Some(name);
                preloaded_zip_bytes = Some(bytes);
                resolved_zip_entry.as_deref()
            }
            None => {
                // ZIP 内に画像が無い場合は失敗として通知
                let _ = tx.send((req.idx, None, false, None));
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
    if needs_heavy_io {
        let ks = keep_start.load(Ordering::Relaxed);
        let ke = keep_end.load(Ordering::Relaxed);
        if req.idx < ks || req.idx >= ke {
            crate::logger::log(format!(
                "    idx={:>4} STALE (after I/O resolve)  {}",
                req.idx, req.path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
            ));
            // None を送信して poll_thumbnails で requested を除去させる
            let _ = tx.send((req.idx, None, false, None));
            gen_done.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }

    load_one_cached(
        load_path,
        zip_entry_ref,
        preloaded_zip_bytes,
        req.pdf_page,
        req.pdf_password.as_deref(),
        req.cache_key_override.as_deref(),
        req.idx, tx, catalog,
        Some(cache_map),
        req.mtime, req.file_size, gen_done,
        thumb_px, thumb_quality, display_px, cache_decision,
        stats,
        cancel,
    );
}

/// フォルダ内をスキャンして代表画像のパスを返す。
/// `sort` で指定されたソート順で並べ、先頭の画像を選ぶ。
/// 直接の子に画像がなければサブフォルダを再帰的に探索する（最大 `remaining_depth` 階層）。
fn resolve_folder_thumb_image(
    folder: &Path,
    sort: crate::settings::SortOrder,
    remaining_depth: u32,
) -> Option<std::path::PathBuf> {
    use crate::folder_tree::SUPPORTED_EXTENSIONS;

    let entries = std::fs::read_dir(folder).ok()?;
    let mut images: Vec<(std::path::PathBuf, i64)> = Vec::new();
    let mut subdirs: Vec<std::path::PathBuf> = Vec::new();

    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            subdirs.push(p);
        } else if p.is_file() {
            if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                if SUPPORTED_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()) {
                    let mtime = entry.metadata().ok()
                        .map_or(0, |m| crate::ui_helpers::mtime_secs(&m));
                    images.push((p, mtime));
                }
            }
        }
    }

    // このフォルダに画像があればソートして先頭を返す
    if !images.is_empty() {
        images.sort_by(|(a, a_mt), (b, b_mt)| {
            let an = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let bn = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
            sort.compare(an, *a_mt, bn, *b_mt, crate::ui_helpers::natural_sort_key)
        });
        return Some(images.into_iter().next().unwrap().0);
    }

    // 画像がなく、まだ深く探索できるならサブフォルダを再帰探索
    if remaining_depth > 0 {
        // サブフォルダを名前順にソートして、最初に画像が見つかったものを採用
        subdirs.sort_by(|a, b| {
            a.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase()
                .cmp(&b.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase())
        });
        for sub in &subdirs {
            if let Some(img) = resolve_folder_thumb_image(sub, sort, remaining_depth - 1) {
                return Some(img);
            }
        }
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
    cache_map: Option<&std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>>,
    mtime: i64,
    file_size: i64,
    gen_done: &Arc<AtomicUsize>,
    thumb_px: u32,
    thumb_quality: u8,
    display_px: u32,
    cache_decision: CacheDecision,
    stats: &Arc<Mutex<crate::stats::ThumbStats>>,
    cancel: Option<&Arc<AtomicBool>>,
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
    // 2. 通常ファイル:    image クレート (拡張子 → マジックバイトの二段構え)
    //                     失敗時は WIC にフォールバック (HEIC / AVIF / JXL / RAW 等)
    let img_result = if let Some(page_num) = pdf_page {
        crate::pdf_loader::render_page(path, page_num, display_px, pdf_password, cancel.map(Arc::clone))
            .map(|(img, _ct)| img)
            .map_err(|e| image::ImageError::IoError(e))
    } else if let Some(entry_name) = zip_entry {
        // プリロード済みバイト列があれば ZIP を再度 open せずにデコード
        let bytes_result = if let Some(bytes) = preloaded_zip_bytes {
            Ok(bytes)
        } else {
            crate::zip_loader::read_entry_bytes(path, entry_name)
                .map_err(image::ImageError::IoError)
        };
        bytes_result.and_then(|bytes| {
            // JPEG なら TurboJPEG で高速デコードを試す
            if is_jpeg_entry(entry_name) {
                if let Some(img) = decode_jpeg_turbo_from_bytes(&bytes) {
                    return Ok(img);
                }
            }
            image::load_from_memory(&bytes)
        })
    } else {
        // 通常ファイル: JPEG なら TurboJPEG を最初に試す
        let turbo_result = if is_jpeg_ext(path) {
            decode_jpeg_turbo_from_path(path)
        } else {
            None
        };
        if let Some(img) = turbo_result {
            Ok(img)
        } else {
            let primary = image::open(path).or_else(|_| {
                use std::io::BufReader;
                let f = std::fs::File::open(path)?;
                image::ImageReader::new(BufReader::new(f))
                    .with_guessed_format()
                    .map_err(image::ImageError::IoError)?
                    .decode()
            });
            // image クレートが失敗した場合に WIC を試す
            // (HEIC / AVIF / JPEG XL / RAW 等は image クレート非対応のため)
            match primary {
                Ok(img) => Ok(img),
                Err(e) => match crate::wic_decoder::decode_to_dynamic_image(path) {
                    Some(img) => Ok(img),
                    None => Err(e),
                },
            }
        }
    };

    let img = match img_result {
        Ok(i) => i,
        Err(e) => {
            // キャンセル済みなら失敗を報告しない (フォルダ切替で旧アイテムが
            // 一瞬ピンク表示になるのを防ぐ)
            if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                crate::logger::log(format!("    idx={idx:>4} cancelled  {display_name}"));
                gen_done.fetch_add(1, Ordering::Relaxed);
                return;
            }
            crate::logger::log(format!("    idx={idx:>4} FAIL {e}  {display_name}"));
            let _ = tx.send((idx, None, false, None));
            gen_done.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut s) = stats.lock() {
                s.record_failed();
            }
            return;
        }
    };

    // EXIF Orientation に基づいて自動回転
    let img = if pdf_page.is_some() || zip_entry.is_some() {
        // PDF ページ / ZIP 内画像: EXIF 回転は不要
        img
    } else {
        apply_exif_orientation(img, path)
    };

    let decode_ms = t.elapsed().as_secs_f64() * 1000.0;

    // 元画像のピクセル寸法 (EXIF 回転適用後)
    let source_dims: Option<(u32, u32)> = Some((img.width(), img.height()));

    // (A) 表示用パス: 元画像から直接セルサイズにリサイズして UI へ送信
    //     WebP 量子化を経由しないため画質劣化なし、かつ WebP encode を待たない
    //     from_cache = false: 元画像由来の高画質 (段階 E アップグレード不要)
    let t_display = std::time::Instant::now();
    let display_ci = resize_to_display_color_image(&img, display_px);
    let display_ms = t_display.elapsed().as_secs_f64() * 1000.0;
    let _ = tx.send((idx, Some(display_ci), false, source_dims));

    // 統計: 画像のフルデコード時間・サイズ・フォーマットを記録
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
            s.record_image(decode_ms + display_ms, file_size.max(0) as u64, ext);
        }
    }

    // (B) キャッシュ保存判定 (段階 C)
    //     catalog 未指定時は保存不可
    //     それ以外は CacheDecision の判定に従う
    let should_save = catalog.is_some()
        && cache_decision.should_cache(path, file_size, decode_ms, display_ms);

    if should_save {
        let cat = catalog.expect("should_save => catalog is Some");
        let t_enc = std::time::Instant::now();
        match crate::catalog::encode_thumb_webp(&img, thumb_px, thumb_quality as f32) {
            Some((webp_data, w, h)) => {
                let encode_ms = t_enc.elapsed().as_secs_f64() * 1000.0;
                if let Err(e) =
                    cat.save(name, mtime, file_size, w, h, source_dims, &webp_data)
                {
                    crate::logger::log(format!("    idx={idx:>4} catalog save: {e}"));
                } else if let Some(cm) = cache_map {
                    // DB 保存成功 → in-memory cache_map にも反映する。
                    // Evicted → 再ロード時にキャッシュヒットさせるために必要。
                    if let Ok(mut map) = cm.write() {
                        map.insert(name.to_owned(), crate::catalog::CacheEntry {
                            mtime,
                            file_size,
                            jpeg_data: webp_data,
                            source_dims,
                        });
                    }
                }
                crate::logger::log(format!(
                    "    idx={idx:>4} decode={decode_ms:>6.1}ms display={display_ms:>5.1}ms encode={encode_ms:>5.1}ms  {display_name}"
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
    use crate::settings::CachePolicy;
    use std::path::PathBuf;

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
    // 拡張子ベース → マジックバイト fallback（load_one_cached と同じ方針）
    let img = image::open(path)
        .or_else(|_| {
            use std::io::BufReader;
            let f = std::fs::File::open(path)?;
            image::ImageReader::new(BufReader::new(f))
                .with_guessed_format()
                .map_err(image::ImageError::IoError)?
                .decode()
        })
        .ok()?;

    let name = path.file_name()?.to_str()?;
    encode_and_save(&img, name, catalog, mtime, file_size, thumb_px, thumb_quality)
}

/// デコード済み画像を WebP エンコードしてカタログに保存する共通ヘルパー。
pub fn encode_and_save(
    img: &image::DynamicImage,
    key: &str,
    catalog: &crate::catalog::CatalogDb,
    mtime: i64,
    file_size: i64,
    thumb_px: u32,
    thumb_quality: u8,
) -> Option<usize> {
    let source_dims = Some((img.width(), img.height()));
    let (webp_data, w, h) =
        crate::catalog::encode_thumb_webp(img, thumb_px, thumb_quality as f32)?;
    catalog
        .save(key, mtime, file_size, w, h, source_dims, &webp_data)
        .ok()?;
    Some(webp_data.len())
}

/// ZIP 内の画像エントリ1つをデコードしてキャッシュに保存する。
/// バッチキャッシュ作成用。
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
    let img = image::load_from_memory(&bytes).ok()?;
    encode_and_save(&img, entry_name, catalog, mtime, file_size, thumb_px, thumb_quality)
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
    let (img, _) = crate::pdf_loader::render_page(pdf_path, page_num, thumb_px, password, None).ok()?;
    let key = crate::grid_item::pdf_page_cache_key(page_num);
    encode_and_save(&img, &key, catalog, mtime, file_size, thumb_px, thumb_quality)
}
