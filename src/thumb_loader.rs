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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};

// -----------------------------------------------------------------------
// 共通型
// -----------------------------------------------------------------------

/// サムネイル読み込み結果メッセージ。
///
/// `(item_idx, Option<ColorImage>, from_cache)`
/// - `from_cache = true`: WebP キャッシュから復元 (段階 E アップグレード対象)
/// - `from_cache = false`: 元画像から直接デコード (高画質) または動画 Shell API
pub type ThumbMsg = (usize, Option<egui::ColorImage>, bool);

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
    /// タスク 3: `Some(name)` なら ZIP エントリとして読む。
    /// `path` が ZIP ファイル、`name` が内部エントリ名。
    pub zip_entry: Option<String>,
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
    // cache_videos_always は動画が別パス (video_thumb) を通るため load_one_cached では使わない
}

impl CacheDecision {
    pub fn from_settings(s: &crate::settings::Settings) -> Self {
        Self {
            policy: s.cache_policy,
            threshold_ms: s.cache_threshold_ms,
            size_threshold: s.cache_size_threshold_bytes,
            webp_always: s.cache_webp_always,
        }
    }

    /// 指定画像をキャッシュに保存すべきか判定する。
    ///
    /// - `Always`: 常に true
    /// - `Off`   : 常に false
    /// - `Auto`  : 事前ヒューリスティック (ext==webp / サイズ) または
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
                // 事前ヒューリスティック
                if self.webp_always {
                    let ext = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|s| s.to_lowercase())
                        .unwrap_or_default();
                    if ext == "webp" {
                        return true;
                    }
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

/// 現在のセルサイズから表示用 ColorImage の画素数を算出する。
///
/// 論理ピクセル × DPI スケールで物理ピクセルを求め、256-2048 px にクランプする。
/// - 下限 256: 起動直後で cell_size が小さすぎる場合の最低品質保証
/// - 上限 2048: 4K 2列などの巨大セルで過大メモリを防ぐ (最大 16 MB/ColorImage)
pub fn compute_display_px(cell_w: f32, cell_h: f32, dpi: f32) -> u32 {
    let logical_max = cell_w.max(cell_h).max(1.0);
    let physical = (logical_max * dpi.max(0.5)).ceil();
    (physical as u32).clamp(256, 2048)
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
    cache_map: &std::collections::HashMap<String, crate::catalog::CacheEntry>,
    tx: &mpsc::Sender<ThumbMsg>,
    catalog: Option<&crate::catalog::CatalogDb>,
    thumb_px: u32,
    thumb_quality: u8,
    display_px: u32,
    cache_decision: CacheDecision,
    gen_done: &Arc<AtomicUsize>,
    stats: &Arc<Mutex<crate::stats::ThumbStats>>,
) {
    // カタログキー:
    // - 通常画像: ファイル名 (例: "foo.jpg")
    // - ZIP エントリ: エントリ名 (例: "work1/img01.jpg") 丸ごと
    //   ZIP ごとに別 DB が開かれるため、DB 内で一意
    let filename: &str = match &req.zip_entry {
        Some(name) => name.as_str(),
        None => req
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(""),
    };

    // 段階 E: skip_cache = true のときはキャッシュヒット判定を飛ばして
    // 必ず元画像からデコードする (アイドル時の画質アップグレード用)
    if !req.skip_cache {
        if let Some(entry) = cache_map.get(filename) {
            if entry.mtime == req.mtime && entry.file_size == req.file_size {
                let ci = crate::catalog::decode_thumb_to_color_image(&entry.jpeg_data);
                // from_cache = true: アップグレード対象
                let _ = tx.send((req.idx, ci, true));
                gen_done.fetch_add(1, Ordering::Relaxed);
                // 統計には記録しない: キャッシュヒットは 2-3 ms で
                // "キャッシュ無し時のコスト" を歪めるため
                return;
            }
        }
    }

    // キャッシュミス or skip_cache: フルデコード (+ 必要なら保存)
    // load_one_cached は from_cache = false を送信する
    load_one_cached(
        &req.path,
        req.zip_entry.as_deref(),
        req.idx, tx, catalog,
        req.mtime, req.file_size, gen_done,
        thumb_px, thumb_quality, display_px, cache_decision,
        stats,
    );
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
    idx: usize,
    tx: &mpsc::Sender<ThumbMsg>,
    catalog: Option<&crate::catalog::CatalogDb>,
    mtime: i64,
    file_size: i64,
    gen_done: &Arc<AtomicUsize>,
    thumb_px: u32,
    thumb_quality: u8,
    display_px: u32,
    cache_decision: CacheDecision,
    stats: &Arc<Mutex<crate::stats::ThumbStats>>,
) {
    // 表示名 (ログ用): ZIP エントリならエントリ名、通常ならファイル名
    let name = match zip_entry {
        Some(n) => n,
        None => path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
    };
    let t = std::time::Instant::now();

    // ── デコード経路 ──
    // 1. ZIP エントリの場合: ZIP を開いてエントリのバイト列を取り出してから decode
    // 2. 通常ファイル: 拡張子ベース → マジックバイトの二段構え
    let img_result = if let Some(entry_name) = zip_entry {
        crate::zip_loader::read_entry_bytes(path, entry_name)
            .map_err(image::ImageError::IoError)
            .and_then(|bytes| image::load_from_memory(&bytes))
    } else {
        image::open(path).or_else(|_| {
            use std::io::BufReader;
            let f = std::fs::File::open(path)?;
            image::ImageReader::new(BufReader::new(f))
                .with_guessed_format()
                .map_err(image::ImageError::IoError)?
                .decode()
        })
    };

    let img = match img_result {
        Ok(i) => i,
        Err(e) => {
            crate::logger::log(format!("    idx={idx:>4} FAIL {e}  {name}"));
            let _ = tx.send((idx, None, false));
            gen_done.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut s) = stats.lock() {
                s.record_failed();
            }
            return;
        }
    };
    let decode_ms = t.elapsed().as_secs_f64() * 1000.0;

    // (A) 表示用パス: 元画像から直接セルサイズにリサイズして UI へ送信
    //     WebP 量子化を経由しないため画質劣化なし、かつ WebP encode を待たない
    //     from_cache = false: 元画像由来の高画質 (段階 E アップグレード不要)
    let t_display = std::time::Instant::now();
    let display_ci = resize_to_display_color_image(&img, display_px);
    let display_ms = t_display.elapsed().as_secs_f64() * 1000.0;
    let _ = tx.send((idx, Some(display_ci), false));

    // 統計: 画像のフルデコード時間・サイズ・フォーマットを記録
    {
        // 拡張子の取得元: ZIP エントリならエントリ名、通常ならファイルパス
        let ext_source: &str = match zip_entry {
            Some(n) => n,
            None => path.to_str().unwrap_or(""),
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
                if let Err(e) = cat.save(name, mtime, file_size, w, h, &webp_data) {
                    crate::logger::log(format!("    idx={idx:>4} catalog save: {e}"));
                }
                crate::logger::log(format!(
                    "    idx={idx:>4} decode={decode_ms:>6.1}ms display={display_ms:>5.1}ms encode={encode_ms:>5.1}ms  {name}"
                ));
            }
            None => {
                crate::logger::log(format!("    idx={idx:>4} WebP encode FAIL  {name}"));
            }
        }
    } else {
        crate::logger::log(format!(
            "    idx={idx:>4} decode={decode_ms:>6.1}ms display={display_ms:>5.1}ms (skip cache)  {name}"
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

    let (webp_data, w, h) =
        crate::catalog::encode_thumb_webp(&img, thumb_px, thumb_quality as f32)?;
    let name = path.file_name()?.to_str()?;
    catalog.save(name, mtime, file_size, w, h, &webp_data).ok()?;
    Some(webp_data.len())
}
