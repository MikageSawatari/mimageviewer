//! 動画タイル モード (Phase 5.5)。
//!
//! フルスクリーン動画再生中に S キーでトグルする一覧モード。動画の再生時間を
//! 一定間隔で区切ったサムネイルをタイル状に並べ、クリックでその時刻に seek する。
//!
//! ## 間隔の自動選択
//!
//! 候補リスト (1/2/5/10/20/30 秒、1/2/5/10/20/30 分) から、画面に切れずに最大数
//! 並ぶ最大の間隔を選ぶ。基本横 10 列、動画のアスペクト比をサムネに反映する。
//!
//! ## キャッシュ
//!
//! 抽出済タイル (= `TileThumbnailWorker.snapshot()`) は VideoPlayer 切替まで生存。
//! 同じ動画で同じ間隔を再選択した場合は worker を作り直さない (= 既存
//! VideoTileState を流用)。
//!
//! ## 描画
//!
//! 描画自体は `src/video/native_presenter/overlay_draw.rs::draw_native_tile_overlay`
//! が native overlay 上で行う。本モジュールは `VideoTileState` の生成 / トグル /
//! クローズと、間隔自動選択など描画と独立した算出ロジックだけを持つ。

use std::path::PathBuf;

use eframe::egui;

use crate::app::App;
use crate::fs_animation::FsCacheEntry;
use crate::video::tile_thumbnails::TileThumbnailWorker;

// 列数候補は `crate::settings::VIDEO_TILE_COLUMN_CANDIDATES` (= 4/6/10/16/20/26/30)。
// `Settings.video_tile_columns` が source of truth。Ctrl+Wheel で次/前の候補に
// 切替し、Setting に保存する。

/// 候補となる間隔 (秒)。タイル数が画面に収まる範囲で最大の間隔を選ぶ
/// (= タイル数最少 = 抽出待ち最短)。
const INTERVAL_CANDIDATES_SECS: &[f64] = &[
    1.0, 2.0, 5.0, 10.0, 20.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1200.0, 1800.0,
];

/// タイルモード 1 セッション分の状態。S キー初回押下で生成、再押下で None に戻す。
pub struct VideoTileState {
    /// この state がどの動画に紐づくか (動画切替を検知して捨てるため)。
    pub video_path: PathBuf,
    /// 採用した間隔 (秒)。
    pub interval_secs: f64,
    /// worker (= バックグラウンド抽出)。Drop で thread join。
    pub worker: TileThumbnailWorker,
    /// 各タイル先の pts_secs (worker spawn 時に与えた値、UI が直接 seek 先に使う)。
    pub timestamps: Vec<f64>,
    /// タイル幅 (px)、高さは動画 aspect から導出。
    pub tile_w: u32,
    pub tile_h: u32,
    pub columns: usize,
}

impl App {
    /// S キー押下時にトグル。フルスクリーン動画再生中であることを呼び出し側で
    /// 確認していること (= `state.is_video` 分岐内から呼ぶ)。
    #[cfg(windows)]
    pub(crate) fn toggle_video_tile_mode(&mut self, fs_idx: usize, screen_size: egui::Vec2) {
        if self.video_tile_state.is_some() {
            self.close_video_tile_mode();
            return;
        }
        self.video_tile_state = self.build_video_tile_state_for(fs_idx, screen_size);
    }

    #[cfg(windows)]
    pub(crate) fn close_video_tile_mode(&mut self) -> bool {
        let was_open = self.video_tile_state.is_some()
            || self.video_tile_swap_pending.is_some()
            || self.video_tile_reopen_pending;
        self.video_tile_state = None;
        self.video_tile_swap_pending = None;
        self.video_tile_reopen_pending = false;
        self.video_tile_reopen_deadline = None;
        was_open
    }

    #[cfg(windows)]
    pub(crate) fn build_video_tile_state_for(
        &self,
        fs_idx: usize,
        screen_size: egui::Vec2,
    ) -> Option<VideoTileState> {
        let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) else {
            return None;
        };
        let Some(info) = player.info().cloned() else {
            return None;
        };
        if info.duration_secs <= 0.5 {
            return None;
        }
        let path = player.path().clone();
        // SAR (sample aspect ratio) を反映した表示アスペクト。
        // anamorphic 動画 (NTSC DVD 等) では encoded `width/height` (= 1.5:1) と
        // 表示比 (= 1.819:1 など) が異なるため、SAR を掛けないとタイルセル比が
        // ずれてサムネが letterbox 表示される。
        let aspect = if info.height > 0 && info.sar_den > 0 {
            (info.width as f64 * info.sar_num as f64) / (info.height as f64 * info.sar_den as f64)
        } else {
            16.0 / 9.0
        };

        // タイルサイズと列数: Setting の video_tile_columns を優先。範囲外なら 10 に
        // 戻して保存。サムネ幅は画面幅 / 列数 (横向き)、高さは aspect で逆算。
        let mut columns = self.settings.video_tile_columns;
        if !crate::settings::VIDEO_TILE_COLUMN_CANDIDATES.contains(&columns) {
            columns = 10;
        }
        // 画面幅 (left/right に余白 16px を確保)。
        let usable_w = (screen_size.x - 32.0).max(200.0);
        let tile_w = (usable_w / columns as f32).floor().max(40.0) as u32;
        let tile_h = ((tile_w as f64) / aspect).round().max(30.0) as u32;
        // 抽出幅は **接続モニター中の最大幅 / `VIDEO_TILE_EXTRACT_MIN_COLUMNS`** を
        // 基準にし、現在の tile_w を下回らないようにする (`.max(tile_w)`)。
        // - 6 列以上は extract_w が定数になり、列数を変えても同じキャッシュ行を共有
        // - tile_w が基準を超える粗いモード (4 列など) では extract_w = tile_w となり、
        //   そのモードを使ったときだけ専用サイズで抽出 (拡大ぼやけは出ない)
        // - 基準を候補の最小値ではなく専用定数にしているのは、候補に 4 を足しただけで
        //   既定 10 列の抽出解像度・メモリまで上がるのを避けるため (Codex P3)
        // - マルチモニターで FS 幅が違ってもキャッシュ再利用可
        // native overlay の描画スケールは tile_rect に合わせて適用するので描画側は
        // 再抽出不要。モニター情報が取れない場合のフォールバックは 640px (4K/6 ≈ 640 相当)。
        let max_screen_w = crate::monitor::max_monitor_pixel_width().unwrap_or(3840) as f32;
        let extract_min_columns = crate::settings::VIDEO_TILE_EXTRACT_MIN_COLUMNS;
        let extract_w = ((max_screen_w / extract_min_columns as f32).floor() as u32).max(tile_w);
        let extract_h = ((extract_w as f64) / aspect).round().max(tile_h as f64) as u32;

        // 画面に収まる最大行数を計算。上下に余白 + ファイル名行 (任意) を引く。
        let usable_h = (screen_size.y - 80.0).max(200.0);
        let row_h = (tile_h as f32 + 8.0 + 16.0).max(80.0); // 8: gap, 16: label
        let max_rows = (usable_h / row_h).floor().max(2.0) as usize;
        let max_tiles = columns.saturating_mul(max_rows).max(8);

        // 候補から「タイル数 <= max_tiles」になる最小の interval を選ぶ。
        // → タイル数最大 = 間隔最短 = より細かいスクラブを優先。
        let dur = info.duration_secs;
        let interval = pick_interval(dur, max_tiles);

        // Codex P5.5 M4 反映: `pick_interval` は溢れる場合に最大候補 (1800 秒) を
        // 返すため、超長時間動画では duration / 1800 が依然 max_tiles を超える可能性が
        // ある (= 100 時間動画で 200 タイル等)。ここで明示的に max_tiles に切り詰めて
        // メモリ / GPU テクスチャ確保を抑える。
        let timestamps: Vec<f64> = generate_timestamps(dur, interval, max_tiles);
        if timestamps.is_empty() {
            return None;
        }

        // Phase 6.D-2: 永続キャッシュを worker に渡す。動画 mtime をキーに mismatch
        // 検出して古いキャッシュを自動破棄する。
        // Phase 8.C: キャッシュキーを (path, tile_w, timestamp_ms) に変更したため
        // worker には interval_ms を渡さない (= 個別 timestamp で lookup する)。
        let video_mtime = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let cache = self.video_tile_cache.clone();
        // worker には extract サイズ (= 最大列幅) を渡す。描画用の tile_w/tile_h は
        // VideoTileState に持って native overlay 側の描画でスケーリングに使う。
        let worker = TileThumbnailWorker::spawn(
            path.clone(),
            timestamps.clone(),
            extract_w,
            extract_h,
            cache,
            video_mtime,
        );
        Some(VideoTileState {
            video_path: path,
            interval_secs: interval,
            worker,
            timestamps,
            tile_w,
            tile_h,
            columns,
        })
    }
}

fn pick_interval(duration_secs: f64, max_tiles: usize) -> f64 {
    // 候補を昇順に試して、タイル数 (= ceil(duration / interval)) が max_tiles 以下に
    // 収まる **最初の** 値を採用 (= 一番細かい間隔)。
    for &c in INTERVAL_CANDIDATES_SECS {
        let count = (duration_secs / c).ceil() as usize;
        if count <= max_tiles {
            return c;
        }
    }
    // 全候補が溢れる場合は最大候補を返す (= 最低でも何タイルか並ぶ)。
    *INTERVAL_CANDIDATES_SECS.last().unwrap_or(&30.0)
}

fn generate_timestamps(duration_secs: f64, interval_secs: f64, max_count: usize) -> Vec<f64> {
    if duration_secs <= 0.0 || interval_secs <= 0.0 || max_count == 0 {
        return Vec::new();
    }
    let mut out: Vec<f64> = Vec::with_capacity(max_count.min(1024));
    let mut t = 0.0;
    while t < duration_secs - 0.01 && out.len() < max_count {
        out.push(t);
        t += interval_secs;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_interval_short_video_gets_small_step() {
        // 30 秒動画、最大 100 タイル → 1 秒間隔 (= 30 タイル)
        let i = pick_interval(30.0, 100);
        assert!((i - 1.0).abs() < 1e-9);
    }

    #[test]
    fn pick_interval_long_video_steps_up() {
        // 1 時間動画 (3600 秒)、最大 100 タイル → 30 秒で 120 → over → 60 秒で 60 →ok
        let i = pick_interval(3600.0, 100);
        assert!((i - 60.0).abs() < 1e-9);
    }

    #[test]
    fn pick_interval_overflow_returns_largest() {
        // 100 時間動画、最大 5 タイル → どの候補でも溢れる → 最大候補 (1800)
        let i = pick_interval(360_000.0, 5);
        assert!((i - 1800.0).abs() < 1e-9);
    }

    #[test]
    fn generate_timestamps_basic() {
        let ts = generate_timestamps(30.0, 5.0, 100);
        assert_eq!(ts.len(), 6); // 0,5,10,15,20,25
        assert!((ts[0] - 0.0).abs() < 1e-9);
        assert!((ts[5] - 25.0).abs() < 1e-9);
    }

    #[test]
    fn generate_timestamps_zero_dur() {
        assert!(generate_timestamps(0.0, 5.0, 100).is_empty());
        assert!(generate_timestamps(5.0, 0.0, 100).is_empty());
        assert!(generate_timestamps(30.0, 5.0, 0).is_empty());
    }

    #[test]
    fn generate_timestamps_capped_by_max_count() {
        // 1 時間 / 1 秒間隔 = 3600 タイルになるが、max_count=10 で切り詰める
        let ts = generate_timestamps(3600.0, 1.0, 10);
        assert_eq!(ts.len(), 10);
        assert!((ts[9] - 9.0).abs() < 1e-9);
    }
}
