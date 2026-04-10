//! サムネイル読み込み統計 (起動時から累計)。
//!
//! キャッシュ生成設定 (Auto モードのしきい値など) をユーザが決めやすくするため、
//! 読み込み時間・ファイルサイズ・フォーマットの分布を集計する。
//!
//! 複数ワーカースレッドから `Arc<Mutex<ThumbStats>>` で共有される。

// ── 読み込み時間のヒストグラム ─────────────────────────────
// 5 ms 刻みで [0-4, 5-9, 10-14, ..., 95-99, 100+] の 21 バケット
pub const LOAD_TIME_BUCKETS: usize = 21;
pub const LOAD_TIME_STEP_MS: f64 = 5.0;

// ── ファイルサイズのヒストグラム ───────────────────────────
// 1 MB 刻みで [0-1MB, 1-2MB, ..., 9-10MB, 10+MB] の 11 バケット
pub const SIZE_BUCKETS: usize = 11;
pub const SIZE_STEP_BYTES: u64 = 1_000_000;

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

    /// 読み込みが FAIL した件数
    pub count_failed: u64,
}

impl ThumbStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// 画像の読み込み結果を記録する。
    /// `total_ms` は decode + display リサイズの合計時間。
    pub fn record_image(&mut self, total_ms: f64, file_size: u64, ext: &str) {
        // 読み込み時間ヒストグラム
        let bucket = ((total_ms / LOAD_TIME_STEP_MS) as usize).min(LOAD_TIME_BUCKETS - 1);
        self.load_time_hist[bucket] += 1;

        // ファイルサイズヒストグラム
        let size_bucket = ((file_size / SIZE_STEP_BYTES) as usize).min(SIZE_BUCKETS - 1);
        self.size_hist[size_bucket] += 1;

        // フォーマット
        let ext_lower = ext.to_ascii_lowercase();
        match ext_lower.as_str() {
            "jpg" | "jpeg" => self.count_jpg += 1,
            "png" => self.count_png += 1,
            "webp" => self.count_webp += 1,
            "gif" => self.count_gif += 1,
            "bmp" => self.count_bmp += 1,
            _ => self.count_other += 1,
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
