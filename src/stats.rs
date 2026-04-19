//! サムネイル読み込み統計 (起動時から累計)。
//!
//! キャッシュ生成設定 (Auto モードのしきい値など) をユーザが決めやすくするため、
//! 読み込み時間・ファイルサイズ・フォーマットの分布を集計する。
//!
//! 複数ワーカースレッドから `Arc<Mutex<ThumbStats>>` で共有される。

use std::collections::BTreeMap;

// ── 読み込み時間のヒストグラム ─────────────────────────────
// 5 ms 刻みで [0-4, 5-9, 10-14, ..., 95-99, 100+] の 21 バケット
pub const LOAD_TIME_BUCKETS: usize = 21;
pub const LOAD_TIME_STEP_MS: f64 = 5.0;

// ── ファイルサイズのヒストグラム ───────────────────────────
// 1 MB 刻みで [0-1MB, 1-2MB, ..., 9-10MB, 10+MB] の 11 バケット
pub const SIZE_BUCKETS: usize = 11;
pub const SIZE_STEP_BYTES: u64 = 1_000_000;

/// どのデコーダ経路で画像が読み込まれたか。
/// thumb_loader が決定し、`record_image` に渡す。
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum DecodeSource {
    /// `image` クレート (PNG/JPEG/GIF/WebP/BMP) または TurboJPEG / PDFium / アニメーション GIF
    #[default]
    Native,
    /// Windows Imaging Component (HEIC/AVIF/JXL/RAW など)
    Wic,
    /// Susie プラグイン (32bit ワーカー経由、MAG/PI/PIC/Q4/MAKI など)
    Susie,
}

#[derive(Default, Clone)]
pub struct ThumbStats {
    /// 読み込み時間 (decode + display_resize) のヒストグラム
    /// 画像のみ記録。動画の Shell API は含まない (別パスのため)
    pub load_time_hist: [u64; LOAD_TIME_BUCKETS],

    /// ファイルサイズ (bytes) のヒストグラム
    /// 画像と動画の両方を記録
    pub size_hist: [u64; SIZE_BUCKETS],

    // ── フォーマット別件数 ──
    pub count_jpg: u64,
    pub count_png: u64,
    pub count_webp: u64,
    pub count_gif: u64,
    pub count_bmp: u64,
    pub count_video: u64,
    pub count_other: u64,

    // ── フォーマット別累計ロード時間 (ms) ──
    pub time_jpg: f64,
    pub time_png: f64,
    pub time_webp: f64,
    pub time_gif: f64,
    pub time_bmp: f64,
    pub time_other: f64,

    // ── ファイルサイズ別累計ロード時間 (ms) ──
    pub size_time_hist: [f64; SIZE_BUCKETS],

    /// 読み込みが FAIL した件数
    pub count_failed: u64,

    // ── デコーダ経路別 (Native/Wic/Susie) ──
    /// Susie プラグイン経由で読み込んだ件数
    pub count_susie: u64,
    /// Susie プラグイン経由の累計ロード時間 (ms)
    pub time_susie: f64,
    /// WIC 経由で読み込んだ件数
    pub count_wic: u64,
    /// WIC 経由の累計ロード時間 (ms)
    pub time_wic: f64,
    /// Susie プラグイン経由の拡張子別内訳 (lowercase ext → (件数, 累計 ms))
    pub susie_by_ext: BTreeMap<String, (u64, f64)>,
}

impl ThumbStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// 画像の読み込み結果を記録する。
    /// `total_ms` は decode + display リサイズの合計時間。
    /// `source` はどのデコーダ経路で読み込まれたか (フォーマット別カウントとは独立に集計)。
    pub fn record_image(&mut self, total_ms: f64, file_size: u64, ext: &str, source: DecodeSource) {
        // 読み込み時間ヒストグラム
        let bucket = ((total_ms / LOAD_TIME_STEP_MS) as usize).min(LOAD_TIME_BUCKETS - 1);
        self.load_time_hist[bucket] += 1;

        // ファイルサイズヒストグラム
        let size_bucket = ((file_size / SIZE_STEP_BYTES) as usize).min(SIZE_BUCKETS - 1);
        self.size_hist[size_bucket] += 1;

        // ファイルサイズ別ロード時間
        self.size_time_hist[size_bucket] += total_ms;

        // フォーマット (拡張子)
        let ext_lower = ext.to_ascii_lowercase();
        match ext_lower.as_str() {
            "jpg" | "jpeg" => { self.count_jpg += 1; self.time_jpg += total_ms; }
            "png"          => { self.count_png += 1; self.time_png += total_ms; }
            "webp"         => { self.count_webp += 1; self.time_webp += total_ms; }
            "gif"          => { self.count_gif += 1; self.time_gif += total_ms; }
            "bmp"          => { self.count_bmp += 1; self.time_bmp += total_ms; }
            _              => { self.count_other += 1; self.time_other += total_ms; }
        }

        // デコーダ経路 (フォーマット集計とは独立。同じ画像は両方に 1 件ずつ加算される。)
        match source {
            DecodeSource::Native => {}
            DecodeSource::Wic => {
                self.count_wic += 1;
                self.time_wic += total_ms;
            }
            DecodeSource::Susie => {
                self.count_susie += 1;
                self.time_susie += total_ms;
                let entry = self.susie_by_ext.entry(ext_lower).or_insert((0, 0.0));
                entry.0 += 1;
                entry.1 += total_ms;
            }
        }
    }

    /// 動画サムネイル (Shell API) の結果を記録する。
    pub fn record_video(&mut self, file_size: u64) {
        let size_bucket = ((file_size / SIZE_STEP_BYTES) as usize).min(SIZE_BUCKETS - 1);
        self.size_hist[size_bucket] += 1;
        self.count_video += 1;
    }

    /// 失敗を記録する。
    pub fn record_failed(&mut self) {
        self.count_failed += 1;
    }

    /// 全件リセット。
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 読み込み時間ヒストグラムのバケット範囲ラベル (例: "  0- 4 ms", "100+  ms")
    pub fn load_time_label(bucket: usize) -> String {
        if bucket >= LOAD_TIME_BUCKETS - 1 {
            format!("{:>3}+   ms", ((LOAD_TIME_BUCKETS - 1) as f64 * LOAD_TIME_STEP_MS) as u32)
        } else {
            let lo = (bucket as f64 * LOAD_TIME_STEP_MS) as u32;
            let hi = lo + LOAD_TIME_STEP_MS as u32 - 1;
            format!("{:>3}-{:>2} ms", lo, hi)
        }
    }

    /// ファイルサイズヒストグラムのバケット範囲ラベル (例: " 0- 1 MB", "10+  MB")
    pub fn size_label(bucket: usize) -> String {
        if bucket >= SIZE_BUCKETS - 1 {
            format!("{:>2}+   MB", SIZE_BUCKETS - 1)
        } else {
            format!("{:>2}-{:>2} MB", bucket, bucket + 1)
        }
    }

    /// 画像のみの合計件数 (動画・失敗を除く)
    pub fn total_images(&self) -> u64 {
        self.count_jpg
            + self.count_png
            + self.count_webp
            + self.count_gif
            + self.count_bmp
            + self.count_other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_image_buckets_load_time() {
        let mut s = ThumbStats::new();
        // 0-4 ms バケット
        s.record_image(0.0, 0, "jpg", DecodeSource::Native);
        s.record_image(4.9, 0, "jpg", DecodeSource::Native);
        // 5-9 ms バケット
        s.record_image(5.0, 0, "jpg", DecodeSource::Native);
        s.record_image(9.9, 0, "jpg", DecodeSource::Native);
        // 100+ ms (オーバーフローバケット)
        s.record_image(150.0, 0, "jpg", DecodeSource::Native);
        s.record_image(9999.0, 0, "jpg", DecodeSource::Native);

        assert_eq!(s.load_time_hist[0], 2);
        assert_eq!(s.load_time_hist[1], 2);
        assert_eq!(s.load_time_hist[LOAD_TIME_BUCKETS - 1], 2);
    }

    #[test]
    fn record_image_buckets_size() {
        let mut s = ThumbStats::new();
        // 0-1 MB
        s.record_image(0.0, 0, "jpg", DecodeSource::Native);
        s.record_image(0.0, 999_999, "jpg", DecodeSource::Native);
        // 1-2 MB
        s.record_image(0.0, 1_500_000, "jpg", DecodeSource::Native);
        // 10+ MB (オーバーフロー)
        s.record_image(0.0, 50_000_000, "jpg", DecodeSource::Native);

        assert_eq!(s.size_hist[0], 2);
        assert_eq!(s.size_hist[1], 1);
        assert_eq!(s.size_hist[SIZE_BUCKETS - 1], 1);
    }

    #[test]
    fn record_image_format_counts() {
        let mut s = ThumbStats::new();
        s.record_image(0.0, 0, "jpg", DecodeSource::Native);
        s.record_image(0.0, 0, "JPEG", DecodeSource::Native); // 大文字 + 拡張形式
        s.record_image(0.0, 0, "png", DecodeSource::Native);
        s.record_image(0.0, 0, "webp", DecodeSource::Native);
        s.record_image(0.0, 0, "gif", DecodeSource::Native);
        s.record_image(0.0, 0, "bmp", DecodeSource::Native);
        s.record_image(0.0, 0, "tiff", DecodeSource::Wic); // → other (count_other), Wic にも 1 件

        assert_eq!(s.count_jpg, 2);
        assert_eq!(s.count_png, 1);
        assert_eq!(s.count_webp, 1);
        assert_eq!(s.count_gif, 1);
        assert_eq!(s.count_bmp, 1);
        assert_eq!(s.count_other, 1);
        assert_eq!(s.total_images(), 7);
    }

    #[test]
    fn record_video_increments_count_and_size() {
        let mut s = ThumbStats::new();
        s.record_video(2_500_000); // 2-3 MB バケット
        s.record_video(800_000);   // 0-1 MB バケット
        s.record_video(0);

        assert_eq!(s.count_video, 3);
        assert_eq!(s.size_hist[2], 1);
        assert_eq!(s.size_hist[0], 2);
        // 動画は load_time_hist には記録されない
        assert!(s.load_time_hist.iter().all(|&n| n == 0));
        // 動画は total_images() には含まれない
        assert_eq!(s.total_images(), 0);
    }

    #[test]
    fn reset_clears_everything() {
        let mut s = ThumbStats::new();
        s.record_image(50.0, 1_000_000, "jpg", DecodeSource::Native);
        s.record_image(20.0, 100_000, "mag", DecodeSource::Susie);
        s.record_video(2_000_000);
        s.record_failed();
        assert!(s.total_images() > 0 || s.count_video > 0 || s.count_failed > 0);
        assert_eq!(s.count_susie, 1);

        s.reset();
        assert_eq!(s.total_images(), 0);
        assert_eq!(s.count_video, 0);
        assert_eq!(s.count_failed, 0);
        assert_eq!(s.count_susie, 0);
        assert_eq!(s.count_wic, 0);
        assert!(s.susie_by_ext.is_empty());
        assert!(s.load_time_hist.iter().all(|&n| n == 0));
        assert!(s.size_hist.iter().all(|&n| n == 0));
    }

    #[test]
    fn record_image_susie_classification() {
        let mut s = ThumbStats::new();
        s.record_image(10.0, 0, "mag", DecodeSource::Susie);
        s.record_image(15.0, 0, "mag", DecodeSource::Susie);
        s.record_image(8.0, 0, "pi", DecodeSource::Susie);
        s.record_image(2.0, 0, "heic", DecodeSource::Wic);
        s.record_image(1.0, 0, "jpg", DecodeSource::Native);

        // Source-based counters
        assert_eq!(s.count_susie, 3);
        assert!((s.time_susie - 33.0).abs() < 1e-6);
        assert_eq!(s.count_wic, 1);
        assert!((s.time_wic - 2.0).abs() < 1e-6);

        // Per-extension Susie breakdown
        assert_eq!(s.susie_by_ext.get("mag"), Some(&(2u64, 25.0)));
        assert_eq!(s.susie_by_ext.get("pi"), Some(&(1u64, 8.0)));

        // Format counts unchanged by source (mag/pi/heic all → other)
        assert_eq!(s.count_other, 4);
        assert_eq!(s.count_jpg, 1);
    }

    #[test]
    fn load_time_label_format() {
        assert_eq!(ThumbStats::load_time_label(0), "  0- 4 ms");
        assert_eq!(ThumbStats::load_time_label(1), "  5- 9 ms");
        // 最後のバケットは "100+ ms" 形式
        let last = ThumbStats::load_time_label(LOAD_TIME_BUCKETS - 1);
        assert!(last.contains('+'));
        assert!(last.contains("ms"));
    }
}
