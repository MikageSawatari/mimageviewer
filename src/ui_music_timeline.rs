//! 音楽ビューの DJ 風カラー波形タイムライン描画 (Inc 3b)。
//!
//! ラボ (`tools/music_lab`) の row raster worker + タイムライン描画をそのまま本体へ移植した
//! もの。設計上の狙いはラボと同じ:
//!
//! - `TimelineAnalysis`(= `music-core` の純解析結果) から **1 行 (row) ずつ** `egui::ColorImage`
//!   を専用ワーカースレッドでラスタライズし、UI スレッドは 1 フレーム少量だけ texture upload する。
//! - cache key / generation / row version が現在の要求と一致しない結果は採用側で破棄する
//!   (黒待ち・古い行の混入を防ぐ最終防衛線)。
//! - 再生カーソル行を優先要求し、可視範囲・近傍の順にラスタライズする。
//!
//! `render_timeline_row_image` 以下のピクセル描画・カラーマッピング・key/bass root 検出は
//! ラボの実装を字面どおり移植している (ラボが機能の正本、`docs/music-integration-plan.md`
//! §2.1 の「再利用する」原則)。egui 依存 (`Color32` / `ColorImage`) があるため music-core には
//! 置けず、本体側モジュールとして持つ。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use music_core::{TimelineAnalysis, WaveformBin};

// ── レイアウト・解析定数 (ラボと同値) ──
const TIMELINE_WAVEFORM_H: f32 = 68.0;
const TIMELINE_METRICS_H: f32 = 44.0;
const TIMELINE_METRIC_LANE_COUNT: usize = 2;
const TIMELINE_LOUDNESS_ROOT_LANE_FRACTION: f32 = 0.81;
const TIMELINE_ROOT_MIN_SEGMENT_SECS: f64 = 0.050;
const TIMELINE_ROOT_MAX_SEGMENT_SECS: f64 = 0.100;
const TIMELINE_ROOT_TRANSIENT_THRESHOLD: f32 = 0.16;
const TIMELINE_KEY_WINDOW_SECS: f64 = 6.0;
const TIMELINE_KEY_TRANSIENT_PENALTY_START: f32 = 0.22;
const TIMELINE_KEY_TRANSIENT_PENALTY_END: f32 = 0.90;
const TIMELINE_KEY_DENSITY_PENALTY_START: f32 = 0.24;
const TIMELINE_KEY_DENSITY_PENALTY_END: f32 = 0.86;
const TIMELINE_KEY_TRANSIENT_REDUCTION: f32 = 0.56;
const TIMELINE_KEY_DENSITY_REDUCTION: f32 = 0.30;
const TIMELINE_KEY_DISPLAY_FLOOR: f32 = 0.18;
const TIMELINE_KEY_KRUMHANSL_WEIGHT: f32 = 0.64;
const TIMELINE_KEY_TEMPERLEY_WEIGHT: f32 = 0.36;
const TIMELINE_KEY_KRUMHANSL_MAJOR_PROFILE: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const TIMELINE_KEY_KRUMHANSL_MINOR_PROFILE: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];
const TIMELINE_KEY_TEMPERLEY_MAJOR_PROFILE: [f32; 12] =
    [5.0, 2.0, 3.5, 2.0, 4.5, 4.0, 2.0, 4.5, 2.0, 3.5, 1.5, 4.0];
const TIMELINE_KEY_TEMPERLEY_MINOR_PROFILE: [f32; 12] =
    [5.0, 2.0, 3.5, 4.5, 2.0, 4.0, 2.0, 4.5, 3.5, 2.0, 1.5, 4.0];
const TIMELINE_INNER_GAP: f32 = 4.0;
const TIMELINE_ROW_GAP: f32 = 12.0;
const TIMELINE_CONTENT_PADDING_Y: f32 = 8.0;
const TIMELINE_TEXTURE_MAX_WIDTH: usize = 4096;
const TIMELINE_ROW_TEXTURE_UPLOAD_BUDGET_PER_FRAME: usize = 1;
const BEAT_GRID_MIN_CONFIDENCE: f32 = 0.55;
const TRANSIENT_ACCENT_MIN: f32 = 0.42;

/// 音楽ビューの解析 bin 幅 (ラボと同じ 10ms)。App の解析ワーカーが `analyze_audio_file_with_config`
/// に渡す config と揃えて使う。
pub const MUSIC_ANALYSIS_BIN_SECS: f64 = 0.010;
/// Row 秒数 (1 行に詰め込む秒数) の選択肢。上情報バーの Row 切替で巡回する。
pub const MUSIC_ROW_SECS_CHOICES: [f64; 5] = [10.0, 15.0, 30.0, 60.0, 120.0];
/// 既定の Row 秒数。
pub const MUSIC_ROW_SECS_DEFAULT: f64 = 30.0;

pub(crate) fn timeline_bg() -> egui::Color32 {
    egui::Color32::BLACK
}

/// 音楽ビュー全体の背景色 (左右の隙間・ラベル列と揃える灰色)。ui_fullscreen の
/// `draw_fs_music_view` の背景と同値。タイムライン左の時間ラベル列をこの色で塗り、
/// 左右の gutter 隙間 (= 音楽ビュー背景が透ける部分) と色を揃える (実機 FB 2026-07)。
pub const MUSIC_VIEW_BG: egui::Color32 = egui::Color32::from_gray(18);

// ── row raster cache + worker ──

#[derive(Default)]
pub struct TimelineTextureCache {
    key: Option<TimelineTextureCacheKey>,
    rows: Vec<Option<TimelineRowTexture>>,
    pending: Vec<Option<TimelinePendingRow>>,
    row_versions: Vec<u64>,
    generation: u64,
    raster_tx: Option<mpsc::Sender<TimelineRasterRequest>>,
    raster_rx: Option<mpsc::Receiver<TimelineRasterResult>>,
    raster_cancel: Option<Arc<AtomicBool>>,
    /// 直近フレームで描いた解析の版数 (`App::music_analysis_version`、0 = 未取得)。
    /// progressive partial で解析が差し替わったら全 row を再ラスタするために使う。
    /// `Arc::as_ptr` の identity 比較はアドレス再利用 (ABA) で差し替えを見逃すため
    /// 版数で判定する (review-v2.3.0 P2-4)。
    last_analysis_version: u64,
}

struct TimelineRowTexture {
    key: TimelineTextureCacheKey,
    row_version: u64,
    texture: egui::TextureHandle,
    represented_bins: usize,
}

#[derive(Clone, Copy, Debug)]
struct TimelinePendingRow {
    key: TimelineTextureCacheKey,
    generation: u64,
    row_version: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TimelineTextureCacheKey {
    width_px: usize,
    waveform_h_px: usize,
    gap_px: usize,
    metrics_h_px: usize,
    row_secs_millis: u32,
    rows: usize,
    dark: bool,
}

struct TimelineRasterRequest {
    generation: u64,
    row_version: u64,
    row: usize,
    row_secs: f64,
    key: TimelineTextureCacheKey,
    /// 解析結果全体を `Arc` で共有する。UI スレッドは refcount を +1 するだけで、行ウィンドウの
    /// 切り出し (`timeline_bins_for_raster` 相当) はワーカー側で行う (Codex P2: UI スレッドで
    /// 数千 bin をコピーしない)。
    analysis: Arc<TimelineAnalysis>,
}

struct TimelineRasterResult {
    generation: u64,
    row_version: u64,
    row: usize,
    key: TimelineTextureCacheKey,
    image: egui::ColorImage,
    represented_bins: usize,
}

impl TimelineTextureCacheKey {
    fn height_px(self) -> usize {
        self.waveform_h_px + self.gap_px + self.metrics_h_px
    }
}

impl TimelineTextureCache {
    /// キャッシュを丸ごと破棄してワーカーを止める。開くファイルが変わったときに呼ぶ。
    pub fn clear(&mut self) {
        self.cancel_worker();
        self.key = None;
        self.rows.clear();
        self.pending.clear();
        self.row_versions.clear();
        self.last_analysis_version = 0;
    }

    /// 解析結果の版数が変わったら（progressive partial → 成長 / partial → final）、
    /// 既存の全 row を再ラスタ対象にする。`TimelineTextureCacheKey` は x 軸幅（= player duration）
    /// が不変なら同一なので、`ensure` の key 差分だけでは無効化されない（Codex P1）。key を保った
    /// まま全 `row_version` を進めて古い row texture を stale 化する（worker は残す）。partial は
    /// 幾何級数で高々十数回なので再ラスタ総コストは有界。`analysis_version` =
    /// `App::music_analysis_version`（代入ごとに単調増加、0 は「未取得」sentinel）。
    fn note_analysis_identity(&mut self, analysis_version: u64) {
        if analysis_version == 0 || self.last_analysis_version == analysis_version {
            return;
        }
        self.last_analysis_version = analysis_version;
        for v in self.row_versions.iter_mut() {
            *v = v.wrapping_add(1);
        }
    }

    fn ensure(&mut self, key: TimelineTextureCacheKey) {
        if self.key == Some(key) {
            return;
        }
        self.cancel_worker();
        self.generation = self.generation.wrapping_add(1);
        self.key = Some(key);
        self.rows.clear();
        self.rows.resize_with(key.rows, || None);
        self.pending.clear();
        self.pending.resize_with(key.rows, || None);
        self.row_versions.clear();
        self.row_versions.resize(key.rows, 0);
        self.spawn_worker();
    }

    fn cancel_worker(&mut self) {
        if let Some(cancel) = self.raster_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.raster_tx = None;
        self.raster_rx = None;
    }

    fn spawn_worker(&mut self) {
        let (request_tx, request_rx) = mpsc::channel::<TimelineRasterRequest>();
        let (result_tx, result_rx) = mpsc::channel::<TimelineRasterResult>();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let spawned = std::thread::Builder::new()
            .name("miv-music-timeline-raster".into())
            .spawn(move || run_timeline_raster_worker(request_rx, result_tx, worker_cancel));
        if spawned.is_ok() {
            self.raster_tx = Some(request_tx);
            self.raster_rx = Some(result_rx);
            self.raster_cancel = Some(cancel);
        }
    }

    fn poll_finished_rows(&mut self, ctx: &egui::Context, upload_budget: usize) -> (usize, usize) {
        let Some(rx) = self.raster_rx.as_ref() else {
            return (0, 0);
        };
        let mut uploaded = 0;
        let mut represented_bins = 0;
        while uploaded < upload_budget {
            let result = match rx.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.raster_tx = None;
                    self.raster_rx = None;
                    self.raster_cancel = None;
                    break;
                }
            };
            if Some(result.key) != self.key || result.generation != self.generation {
                continue;
            }
            let Some(slot) = self.rows.get_mut(result.row) else {
                continue;
            };
            if let Some(pending_slot) = self.pending.get_mut(result.row)
                && pending_slot.as_ref().is_some_and(|pending| {
                    pending.generation == result.generation
                        && pending.row_version <= result.row_version
                })
            {
                *pending_slot = None;
            }
            if slot.as_ref().is_some_and(|row_texture| {
                row_texture.key == result.key && row_texture.row_version > result.row_version
            }) {
                continue;
            }
            let texture = ctx.load_texture(
                format!(
                    "music_timeline_row_{}_{}_{}_{}x{}_{}",
                    result.generation,
                    result.row_version,
                    result.row,
                    result.key.width_px,
                    result.key.height_px(),
                    result.key.dark as u8
                ),
                result.image,
                egui::TextureOptions::LINEAR,
            );
            *slot = Some(TimelineRowTexture {
                key: result.key,
                row_version: result.row_version,
                texture,
                represented_bins: result.represented_bins,
            });
            uploaded += 1;
            represented_bins += result.represented_bins;
        }
        (uploaded, represented_bins)
    }

    fn row_texture(
        &mut self,
        analysis: &Arc<TimelineAnalysis>,
        row: usize,
        row_secs: f64,
        key: TimelineTextureCacheKey,
    ) -> (Option<(&egui::TextureHandle, usize)>, bool) {
        self.ensure(key);
        if row >= self.rows.len() {
            return (None, false);
        }
        let stale = self.rows[row]
            .as_ref()
            .is_some_and(|row_texture| row_texture.key != key);
        let missing = self.rows[row].is_none() || stale;
        let row_version = self.row_versions.get(row).copied().unwrap_or(0);
        let mut request_sent = false;
        let needs_newer_texture = self.rows[row].as_ref().is_none_or(|row_texture| {
            row_texture.key != key || row_texture.row_version < row_version
        });
        if missing || needs_newer_texture {
            // `row_version` を含めて判定する: progressive partial で `note_analysis_identity` が
            // row_version を進めた直後、古い prefix 用 request が pending のままだと、それだけを
            // 見て「pending 済み」と誤判定して新 analysis 用 request が出ず、progressive 追従が
            // 古い raster 完了待ちになる (Codex P2)。pending が目標より古い version なら再要求する。
            let pending_current = self.pending[row].is_some_and(|pending| {
                pending.key == key
                    && pending.generation == self.generation
                    && pending.row_version >= row_version
            });
            if !pending_current && let Some(tx) = self.raster_tx.as_ref() {
                let request = TimelineRasterRequest {
                    generation: self.generation,
                    row_version,
                    row,
                    row_secs,
                    key,
                    analysis: Arc::clone(analysis),
                };
                if tx.send(request).is_ok() {
                    self.pending[row] = Some(TimelinePendingRow {
                        key,
                        generation: self.generation,
                        row_version,
                    });
                    request_sent = true;
                }
            }
        }
        let Some(row) = self.rows[row].as_ref() else {
            return (None, request_sent);
        };
        if row.key == key {
            (Some((&row.texture, row.represented_bins)), request_sent)
        } else {
            (None, request_sent)
        }
    }

    fn row_is_fresh(&self, row: usize, key: TimelineTextureCacheKey) -> bool {
        let row_version = self.row_versions.get(row).copied().unwrap_or(0);
        self.rows
            .get(row)
            .and_then(Option::as_ref)
            .is_some_and(|row_texture| {
                row_texture.key == key && row_texture.row_version >= row_version
            })
    }
}

impl Drop for TimelineTextureCache {
    fn drop(&mut self) {
        self.cancel_worker();
    }
}

fn run_timeline_raster_worker(
    request_rx: mpsc::Receiver<TimelineRasterRequest>,
    result_tx: mpsc::Sender<TimelineRasterResult>,
    cancel: Arc<AtomicBool>,
) {
    while !cancel.load(Ordering::Relaxed) {
        let request = match request_rx.recv() {
            Ok(request) => request,
            Err(_) => break,
        };
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let row_start = request.row as f64 * request.row_secs;
        // 行ウィンドウの切り出しはワーカー側で行う (UI スレッドは Arc を渡すだけ)。
        // 全 bin をコピーせず、部分スライスを直接 render に渡す (ゼロコピー)。
        let (start_idx, end_idx) =
            timeline_bins_window_range(&request.analysis.bins, row_start, request.row_secs);
        let (image, represented_bins) = render_timeline_row_image(
            row_start,
            request.row_secs,
            &request.analysis.bins[start_idx..end_idx],
            None,
            request.key.width_px,
            request.key.waveform_h_px,
            request.key.gap_px,
            request.key.metrics_h_px,
            request.key.dark,
        );
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if result_tx
            .send(TimelineRasterResult {
                generation: request.generation,
                row_version: request.row_version,
                row: request.row,
                key: request.key,
                image,
                represented_bins,
            })
            .is_err()
        {
            break;
        }
    }
}

// ── タイムライン描画エントリ ──

/// `draw_music_timeline` の結果。
#[derive(Default)]
pub struct MusicTimelineOutcome {
    /// クリック/ドラッグでシークを要求した位置 (呼び出し側が `player.seek`)。
    pub seek_request: Option<f64>,
    /// この frame に実際に描画できた timeline texture row 数。0 の場合、呼び出し側は
    /// 解析済みでもまだ texture が無い初回 frame と判断できる。
    pub displayed_texture_rows: usize,
    /// この frame にホイールで手動スクロールがあった。呼び出し側はこれを見て「追従クール
    /// ダウン」を張る (手動閲覧中に playhead へ引き戻さない)。
    pub manual_scroll: bool,
    /// この frame に追従 (`scroll_to_rect`) を発行した。呼び出し側が「offset 変化はプログラム
    /// 起因か scrollbar ドラッグか」を切り分けるのに使う (follow 由来の offset 変化を除外)。
    pub auto_scrolled: bool,
}

/// 音楽ビュー中央のタイムラインを描画する。ラボの `draw_timeline` を本体向けに移植した
/// もの (再生は `VideoPlayer` に委ねるので、この関数はシーク要求を返すだけに留める)。
///
/// `ui` は音楽ビュー中央領域に張った `ScrollArea` 内の子 UI。`auto_scroll` が true かつ
/// 再生中で、playhead 行が画面内にあって画面外へ出かけている (部分クリップ) ときだけ追従
/// スクロールする。手動スクロール中 (呼び出し側のクールダウン) は false で渡される。
#[allow(clippy::too_many_arguments)]
pub fn draw_music_timeline(
    ui: &mut egui::Ui,
    analysis: &Arc<TimelineAnalysis>,
    // `App::music_analysis_version` (解析差し替えごとに進む。row 再ラスタ判定に使う)。
    analysis_version: u64,
    duration_secs: f64,
    position_secs: f64,
    playing: bool,
    auto_scroll: bool,
    cache: &mut TimelineTextureCache,
    row_secs: f64,
    dark: bool,
    // 左の時間ラベル (0:30 等) 列の幅 (px)。呼び出し側が「左 5% gutter」を渡すことで、
    // ラベルをパネルトリガ帯 (= seek 不要領域) に載せ、波形は中央領域を全幅使う
    // (実機 FB 2026-07)。ラベルは波形左端に右寄せで描く。
    left_label_w: f32,
) -> MusicTimelineOutcome {
    let mut seek_request = None;
    let row_secs = row_secs.max(1.0);
    let rows = timeline_row_count(duration_secs, row_secs);
    let row_gap = TIMELINE_ROW_GAP;
    let row_h = TIMELINE_WAVEFORM_H + TIMELINE_INNER_GAP + TIMELINE_METRICS_H;
    let content_h = TIMELINE_CONTENT_PADDING_Y * 2.0
        + rows as f32 * row_h
        + rows.saturating_sub(1) as f32 * row_gap;
    let available = egui::vec2(ui.available_width(), ui.available_height().max(content_h));
    let (rect, response) = ui.allocate_exact_size(available, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, timeline_bg());

    let label_w = left_label_w.max(0.0);
    let graph_rect = egui::Rect::from_min_max(
        rect.min + egui::vec2(label_w, TIMELINE_CONTENT_PADDING_Y),
        egui::pos2(
            rect.max.x - TIMELINE_CONTENT_PADDING_Y,
            rect.min.y + content_h - TIMELINE_CONTENT_PADDING_Y,
        ),
    );
    // 左の時間ラベル列 (gutter) は音楽ビュー背景の灰色で塗り、左右の隙間と色を揃える
    // (実機 FB 2026-07)。波形グラフ部 (graph_rect 以降) は timeline_bg (黒) のまま。
    if label_w > 0.0 {
        painter.rect_filled(
            egui::Rect::from_min_max(rect.min, egui::pos2(graph_rect.left(), rect.max.y)),
            0.0,
            MUSIC_VIEW_BG,
        );
    }
    let ppp = ui.ctx().pixels_per_point().clamp(1.0, 3.0);
    let width_px =
        ((graph_rect.width() * ppp).round() as usize).clamp(1, TIMELINE_TEXTURE_MAX_WIDTH);
    let waveform_h_px = ((TIMELINE_WAVEFORM_H * ppp).round() as usize).max(1);
    let gap_px = ((TIMELINE_INNER_GAP * ppp).round() as usize).max(1);
    let metrics_h_px = ((TIMELINE_METRICS_H * ppp).round() as usize).max(1);
    let texture_key = TimelineTextureCacheKey {
        width_px,
        waveform_h_px,
        gap_px,
        metrics_h_px,
        row_secs_millis: (row_secs * 1000.0).round() as u32,
        rows,
        dark,
    };
    cache.ensure(texture_key);
    // progressive partial で解析が差し替わっていたら全 row を再ラスタ対象にする（key は
    // player duration 基準の全幅なので変わらず、ensure では無効化されない。Codex P1）。
    cache.note_analysis_identity(analysis_version);
    cache.poll_finished_rows(ui.ctx(), TIMELINE_ROW_TEXTURE_UPLOAD_BUDGET_PER_FRAME);

    let text_color = ui.visuals().text_color();
    let clip_rect = ui.clip_rect();
    // ホイール手動スクロールを検出 (scrollbar ドラッグは呼び出し側が offset 変化で拾う)。
    let mut manual_scroll = timeline_manual_scroll_requested(ui, &response, clip_rect);
    // ホイール (中) ボタンをドラッグしたらグラフを縦パンする (ハンドツール、実機 FB)。左クリック
    // シークとは別ボタンなので誤爆しない。egui の drag-sense がどのボタンを拾うかに依存せず、
    // 中ボタン押下 + ポインタがタイムライン上 + 移動、をポインタ状態から直接判定する。パン中は
    // クールダウンを張るため manual_scroll 扱いにする。
    let middle_pan = ui.input(|i| {
        i.pointer.button_down(egui::PointerButton::Middle)
            && i.pointer
                .hover_pos()
                // 可視 viewport (clip_rect) かつ content (rect) 内 = 可視タイムライン上のみ。
                // rect は content rect でスクロール後は viewport 外まで伸びるので clip_rect と AND
                // を取る (上バー / 下 HUD で誤発火しない、Codex P3)。
                .is_some_and(|p| clip_rect.contains(p) && rect.contains(p))
    });
    if middle_pan {
        let dy = ui.input(|i| i.pointer.delta().y);
        if dy != 0.0 {
            // 正の delta.y = 先頭方向 (上) へスクロール = ドラッグ下方向で内容が下に動く (ハンドパン)。
            ui.scroll_with_delta_animation(
                egui::vec2(0.0, dy),
                egui::style::ScrollAnimation::none(),
            );
            manual_scroll = true;
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    let mut auto_scrolled = false;
    if let Some(playhead_rect) =
        timeline_playhead_row_rect(graph_rect, position_secs, row_secs, row_h, row_gap, rows)
    {
        let vertically_visible = clip_rect.intersects(playhead_rect);
        let fully_visible = clip_rect_vertically_contains(clip_rect, playhead_rect);
        // 追従スクロールは「再生中 + クールダウンなし(auto_scroll) + playhead が画面内にあって
        // 画面外へ出かけている(部分クリップ)」ときだけ発火する (実機 FB 2026-07-03)。完全に
        // 画面外 (ユーザーが離れて閲覧中) のときは引き戻さない。毎フレーム再センタリングも
        // しない (fully_visible の間は何もしない)。
        if playing && auto_scroll && vertically_visible && !fully_visible {
            // 即時スクロール (ScrollAnimation::none)。既定の 0.1-0.3s アニメだと offset が数フレーム
            // かけて変化し、その末尾フレームが「scrollbar ドラッグ」に誤検出されて追従が jerky 化
            // する (Codex P2、egui 0.33.3)。即時なら offset 変化が当フレームに収まり除外できる。
            ui.scroll_to_rect_animation(
                playhead_rect.expand(row_gap),
                None,
                egui::style::ScrollAnimation::none(),
            );
            auto_scrolled = true;
        }
    }

    let mut visible_rows = Vec::new();
    for row in 0..rows {
        let row_top = graph_rect.min.y + row as f32 * (row_h + row_gap);
        let row_rect = egui::Rect::from_min_size(
            egui::pos2(graph_rect.min.x, row_top),
            egui::vec2(graph_rect.width(), row_h),
        );
        if !clip_rect.intersects(row_rect.expand(row_gap)) {
            continue;
        }
        visible_rows.push((row, row_rect));
    }

    let focus_row = timeline_focus_row(position_secs, row_secs, rows);
    // 追従中 (auto_scroll) は playhead 行を off-screen でも先読みラスタしておく (じき画面内へ)。
    // 手動閲覧中 (クールダウン) は可視行を優先し、off-screen の playhead 行は競合させない。
    let include_offscreen_focus = playing && auto_scroll;
    let visible_row_indices = visible_rows.iter().map(|(row, _)| *row).collect::<Vec<_>>();
    let request_rows = prioritized_timeline_request_rows(
        &visible_row_indices,
        focus_row,
        include_offscreen_focus,
        rows,
    );

    let mut pending_raster = false;
    for row in request_rows {
        if !cache.row_is_fresh(row, texture_key) {
            pending_raster = true;
        }
        let (_, request_sent) = cache.row_texture(analysis, row, row_secs, texture_key);
        if request_sent {
            pending_raster = true;
        }
    }

    let mut displayed_texture_rows = 0_usize;
    for (row, row_rect) in visible_rows {
        let row_start = row as f64 * row_secs;
        if !cache.row_is_fresh(row, texture_key) {
            pending_raster = true;
        }
        let (row_texture, request_sent) = cache.row_texture(analysis, row, row_secs, texture_key);
        if request_sent {
            pending_raster = true;
        }
        if let Some((texture, _represented_bins)) = row_texture {
            displayed_texture_rows += 1;
            painter.image(
                texture.id(),
                row_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        // 時間ラベルは波形左端 (graph_rect.left) の直左に右寄せで描く。左 gutter が広くても
        // ラベルが波形に隣接して読みやすく、seek 帯 (graph_rect) には食い込まない。
        painter.text(
            egui::pos2(graph_rect.min.x - 6.0, row_rect.center().y),
            egui::Align2::RIGHT_CENTER,
            format_time(row_start),
            egui::FontId::monospace(12.0),
            text_color,
        );
        draw_beat_grid(&painter, row_rect, row_start, row_secs, analysis);
    }
    if pending_raster {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(1));
    }

    draw_playhead(
        &painter,
        graph_rect,
        position_secs,
        row_secs,
        row_h,
        row_gap,
    );

    if let Some(pos) = response.hover_pos() {
        draw_timeline_metric_hover(
            &painter,
            pos,
            graph_rect,
            row_secs,
            row_h,
            row_gap,
            rows,
            &analysis.bins,
        );
    }

    // シークは **左クリック / 左ドラッグのみ** (実機 FB)。右クリック・ホイール(中)クリックは
    // egui の `dragged()` が全ボタンで true になり、わずかな移動でシーク誤爆 → 不快音が出ていた。
    // `dragged_by(Primary)` に限定する (中ボタンは上のパン、右はメニュー等でシークしない)。
    if (response.clicked() || response.dragged_by(egui::PointerButton::Primary))
        && let Some(pos) = response.interact_pointer_pos()
    {
        seek_request = timeline_seek_target_secs(
            pos,
            graph_rect,
            row_h,
            row_gap,
            rows,
            row_secs,
            duration_secs,
        );
    }
    MusicTimelineOutcome {
        seek_request,
        displayed_texture_rows,
        manual_scroll,
        auto_scrolled,
    }
}

/// タイムライン上のクリック/ドラッグ位置 `pos` から seek 先秒を計算する。**グラフ範囲外は
/// `None`** (= seek しない)。棄却するケース:
/// - x が波形 `graph_rect` の外 (左ラベル列 / 左右 gutter 隙間) → frac が 0/1 に張り付くのを防ぐ。
/// - `row >= rows` (最終行より下の余白。viewport > content の空き領域や row_gap)。
/// - 部分行 (曲末尾が行途中) の末尾以降空白 → 計算 seek 秒が `duration_secs` 以上。
///
/// これらを弾かないと余白/末尾余白クリックで曲末尾へ飛び、連続再生でも次曲に進まず停止する
/// (実機 FB 2026-07-03)。
fn timeline_seek_target_secs(
    pos: egui::Pos2,
    graph_rect: egui::Rect,
    row_h: f32,
    row_gap: f32,
    rows: usize,
    row_secs: f64,
    duration_secs: f64,
) -> Option<f64> {
    let local_y = pos.y - graph_rect.min.y;
    let row = (local_y / (row_h + row_gap)).floor().max(0.0) as usize;
    let row_top = graph_rect.min.y + row as f32 * (row_h + row_gap);
    if row < rows
        && pos.y >= row_top
        && pos.y <= row_top + row_h
        && pos.x >= graph_rect.left()
        && pos.x <= graph_rect.right()
    {
        let frac = ((pos.x - graph_rect.min.x) / graph_rect.width()).clamp(0.0, 1.0);
        let t = row as f64 * row_secs + frac as f64 * row_secs;
        if t < duration_secs {
            return Some(t);
        }
    }
    None
}

/// タイムライン行数の上限。壊れた音声ファイルはコンテナ duration に i64::MAX 級の
/// ゴミを報告することがあり (probe は上限なしで秒に変換する)、無制限だと
/// `TimelineTextureCache::ensure` が兆単位の `resize_with` を試みて OOM abort /
/// フリーズする (review-v2.3.0 hunt P1)。row_secs は描画側で 1.0 秒以上に clamp
/// されるので、100_000 行 ≒ 27 時間超の音声までは実表示を維持できる (それ以上は
/// 末尾が切れるだけで、正常ファイルでは実質到達しない)。
const TIMELINE_MAX_ROWS: usize = 100_000;

fn timeline_row_count(duration_secs: f64, row_secs: f64) -> usize {
    if !duration_secs.is_finite()
        || duration_secs <= 0.0
        || !row_secs.is_finite()
        || row_secs <= 0.0
    {
        1
    } else {
        ((duration_secs / row_secs).ceil().max(1.0) as usize).min(TIMELINE_MAX_ROWS)
    }
}

fn clip_rect_vertically_contains(clip_rect: egui::Rect, target: egui::Rect) -> bool {
    target.top() >= clip_rect.top() && target.bottom() <= clip_rect.bottom()
}

fn timeline_manual_scroll_requested(
    ui: &egui::Ui,
    response: &egui::Response,
    clip_rect: egui::Rect,
) -> bool {
    let pointer_over_timeline = ui
        .input(|input| input.pointer.hover_pos())
        .is_some_and(|pos| clip_rect.contains(pos) || response.rect.contains(pos));
    if !pointer_over_timeline {
        return false;
    }
    ui.input(|input| {
        input.raw_scroll_delta.y.abs() > 0.0 || input.smooth_scroll_delta.y.abs() > 0.5
    })
}

fn timeline_focus_row(position_secs: f64, row_secs: f64, rows: usize) -> Option<usize> {
    if row_secs <= 0.0 || rows == 0 || !position_secs.is_finite() {
        return None;
    }
    Some(
        (position_secs.max(0.0) / row_secs)
            .floor()
            .max(0.0)
            .min(rows.saturating_sub(1) as f64) as usize,
    )
}

fn prioritized_timeline_request_rows(
    visible_rows: &[usize],
    focus_row: Option<usize>,
    include_offscreen_focus: bool,
    rows: usize,
) -> Vec<usize> {
    let mut ordered = Vec::with_capacity(visible_rows.len() + usize::from(include_offscreen_focus));
    if include_offscreen_focus && let Some(row) = focus_row.filter(|row| *row < rows) {
        ordered.push(row);
    }

    let mut visible = visible_rows
        .iter()
        .copied()
        .filter(|row| *row < rows && !ordered.contains(row))
        .collect::<Vec<_>>();
    if let Some(focus) = focus_row
        && (include_offscreen_focus || visible_rows.contains(&focus))
    {
        visible.sort_by_key(|row| (row.abs_diff(focus), *row));
    }
    ordered.extend(visible);
    ordered
}

fn timeline_playhead_row_rect(
    graph_rect: egui::Rect,
    position_secs: f64,
    row_secs: f64,
    row_h: f32,
    row_gap: f32,
    rows: usize,
) -> Option<egui::Rect> {
    let row = timeline_focus_row(position_secs, row_secs, rows)?;
    let row_top = graph_rect.min.y + row as f32 * (row_h + row_gap);
    Some(egui::Rect::from_min_size(
        egui::pos2(graph_rect.min.x, row_top),
        egui::vec2(graph_rect.width(), row_h),
    ))
}

#[allow(clippy::too_many_arguments)]
fn draw_timeline_metric_hover(
    painter: &egui::Painter,
    pos: egui::Pos2,
    graph_rect: egui::Rect,
    row_secs: f64,
    row_h: f32,
    row_gap: f32,
    rows: usize,
    bins: &[WaveformBin],
) {
    if row_secs <= 0.0 || rows == 0 || bins.is_empty() || !graph_rect.contains(pos) {
        return;
    }
    let row_stride = row_h + row_gap;
    let local_y = pos.y - graph_rect.top();
    if local_y < 0.0 {
        return;
    }
    let row = (local_y / row_stride).floor() as usize;
    if row >= rows {
        return;
    }
    let row_top = graph_rect.top() + row as f32 * row_stride;
    let metrics_rect = egui::Rect::from_min_max(
        egui::pos2(
            graph_rect.left(),
            row_top + TIMELINE_WAVEFORM_H + TIMELINE_INNER_GAP,
        ),
        egui::pos2(graph_rect.right(), row_top + row_h),
    );
    if !metrics_rect.contains(pos) {
        return;
    }

    let loudness_bottom =
        metrics_rect.top() + metrics_rect.height() * TIMELINE_LOUDNESS_ROOT_LANE_FRACTION;
    let lane_idx = if pos.y < loudness_bottom { 0 } else { 1 };
    let kind = TIMELINE_METRIC_KINDS[lane_idx];
    let row_start = row as f64 * row_secs;
    let frac = ((pos.x - graph_rect.left()) / graph_rect.width()).clamp(0.0, 1.0);
    let time_secs = row_start + frac as f64 * row_secs;
    let Some(bin) = timeline_bin_at_time(bins, time_secs) else {
        return;
    };
    let value = timeline_metric_hover_value(kind, bins, bin, time_secs).clamp(0.0, 1.0);
    let x = graph_rect.left() + frac * graph_rect.width();
    let (lane_top, lane_bottom) =
        timeline_metric_lane_bounds_f32(metrics_rect.top(), metrics_rect.height(), lane_idx);
    let lane_rect = egui::Rect::from_min_max(
        egui::pos2(metrics_rect.left(), lane_top),
        egui::pos2(metrics_rect.right(), lane_bottom),
    );
    painter.rect_stroke(
        lane_rect,
        0.0,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(210, 230, 250, 150),
        ),
        egui::StrokeKind::Inside,
    );
    painter.line_segment(
        [
            egui::pos2(x, lane_rect.top()),
            egui::pos2(x, lane_rect.bottom()),
        ],
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(230, 240, 255, 150),
        ),
    );

    let label = format!(
        "{}{} {:.0}%  {}  {}",
        timeline_metric_name(kind),
        timeline_metric_extra(kind, bins, bin, time_secs),
        value * 100.0,
        format_time(time_secs),
        timeline_metric_description(kind)
    );
    let label_w = (label.chars().count() as f32 * 7.0 + 14.0).min(graph_rect.width() - 8.0);
    let label_h = 22.0;
    let label_x = if x + 10.0 + label_w <= graph_rect.right() {
        x + 10.0
    } else {
        (x - 10.0 - label_w).max(graph_rect.left() + 4.0)
    };
    let label_y = if lane_rect.top() - label_h - 6.0 >= graph_rect.top() {
        lane_rect.top() - label_h - 6.0
    } else {
        lane_rect.bottom() + 6.0
    };
    let label_rect =
        egui::Rect::from_min_size(egui::pos2(label_x, label_y), egui::vec2(label_w, label_h));
    painter.rect_filled(
        label_rect,
        3.0,
        egui::Color32::from_rgba_unmultiplied(5, 8, 12, 230),
    );
    painter.rect_stroke(
        label_rect,
        3.0,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(100, 150, 190, 180),
        ),
        egui::StrokeKind::Inside,
    );
    painter.text(
        label_rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::monospace(11.0),
        egui::Color32::from_rgb(234, 244, 252),
    );
}

fn timeline_bin_at_time(bins: &[WaveformBin], time_secs: f64) -> Option<&WaveformBin> {
    if bins.is_empty() || !time_secs.is_finite() {
        return None;
    }
    // progressive ロード中、まだ解析されていない未来領域（最終 bin の終端より先）を hover
    // したとき、`bins.last()` を返すと古い metric が未来位置に表示される（Codex P2）。
    // 最終 bin の終端より先は None にする。
    if let Some(last) = bins.last()
        && time_secs > last.start_secs + last.duration_secs
    {
        return None;
    }
    let idx = bins.partition_point(|bin| bin.start_secs + bin.duration_secs <= time_secs);
    bins.get(idx).or_else(|| bins.last())
}

/// 1 行 (row) をラスタライズするのに必要な bin スライスの範囲 `[start, end)` を返す。
/// 行の秒範囲 + key 検出用の前後パディングを含む。ワーカー側でこの範囲を使って全 bin から
/// 部分スライスを取り、コピーせずに render へ渡す。
fn timeline_bins_window_range(
    bins: &[WaveformBin],
    row_start: f64,
    row_secs: f64,
) -> (usize, usize) {
    if bins.is_empty() {
        return (0, 0);
    }
    let row_end = row_start + row_secs;
    let pad = TIMELINE_KEY_WINDOW_SECS * 0.5;
    let copy_start = (row_start - pad).max(0.0);
    let copy_end = row_end + pad;
    let start_idx = bins.partition_point(|bin| bin.start_secs + bin.duration_secs < copy_start);
    let end_idx = bins.partition_point(|bin| bin.start_secs <= copy_end);
    (start_idx, end_idx)
}

#[cfg(test)]
fn timeline_bins_for_raster(
    bins: &[WaveformBin],
    row_start: f64,
    row_secs: f64,
) -> Vec<WaveformBin> {
    let (start_idx, end_idx) = timeline_bins_window_range(bins, row_start, row_secs);
    bins[start_idx..end_idx].to_vec()
}

// ── row ラスタライズ (ピクセル描画、ワーカースレッドで実行) ──

/// シークストリップの帯としてこの行を描くときの素材情報。音楽ビューは `None` を渡し、
/// 見た目は従来のまま変わらない。
pub(crate) struct SeekStripRowStyle<'a> {
    /// 解析済みの時間範囲。ここから外れた列は「まだ解析していない」として暗く描き、
    /// 中心線も引かない。無音 (中心線だけが見える) と区別が付く必要がある。
    pub(crate) analyzed_spans: &'a [(f64, f64)],
    /// 素材の長さ。0 秒と末尾に境界線を引く。帯は中心固定なので、端へ寄ると窓が素材の
    /// 外へはみ出す。
    pub(crate) duration_secs: f64,
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_timeline_row_image(
    row_start: f64,
    row_secs: f64,
    bins: &[WaveformBin],
    // `Some` はシークストリップの帯。粗い列と窓復号の**どちらも** `Some` を渡す。片方だけ
    // 渡すと、同じ帯なのに中心線の見え方が経路で変わってしまう。`None` は音楽ビューで、
    // 見た目は従来のまま (raster スナップショットが一致し続ける必要がある)。
    seek_strip: Option<&SeekStripRowStyle>,
    width: usize,
    waveform_h: usize,
    gap_h: usize,
    metrics_h: usize,
    _dark: bool,
) -> (egui::ColorImage, usize) {
    let height = waveform_h + gap_h + metrics_h;
    let bg = timeline_bg();
    let mut pixels = vec![bg; width * height];

    fill_rect_px(
        &mut pixels,
        width,
        height,
        0,
        0,
        width,
        waveform_h,
        egui::Color32::BLACK,
    );
    if let Some(strip) = seek_strip {
        for &(start_secs, end_secs) in strip.analyzed_spans {
            let visible_start = start_secs.max(row_start);
            let visible_end = end_secs.min(row_start + row_secs);
            if visible_end <= visible_start {
                continue;
            }
            let x0 = ((visible_start - row_start) / row_secs) as f32 * width as f32;
            let x1 = ((visible_end - row_start) / row_secs) as f32 * width as f32;
            fill_rect_f32(
                &mut pixels,
                width,
                height,
                x0,
                0.0,
                x1,
                waveform_h as f32,
                egui::Color32::from_rgb(6, 8, 10),
            );
        }
    }
    let metrics_top = waveform_h + gap_h;
    fill_rect_px(
        &mut pixels,
        width,
        height,
        0,
        metrics_top,
        width,
        height,
        egui::Color32::from_rgb(3, 5, 7),
    );
    let center_y = waveform_h as f32 * 0.5;
    match seek_strip {
        None => fill_rect_f32(
            &mut pixels,
            width,
            height,
            0.0,
            center_y - 0.5,
            width as f32,
            center_y + 0.5,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 28),
        ),
        Some(strip) => {
            // 帯の中心線は 1 物理ピクセルしかないうえ、`center_y - 0.5 .. center_y + 0.5` は
            // 2 行に半分ずつ乗るので、音楽ビューと同じ 11% 白では**無音の動画でほぼ見えない**
            // (利用者報告 2026-08-26)。無音とは「中心線だけが見えている状態」なので、
            // ここが見えないと無音と未解析の区別も付かない。行境界へ丸めて 1 行を満たし、
            // 濃さも上げる。
            let line_top = center_y.floor();
            for &(start_secs, end_secs) in strip.analyzed_spans {
                let visible_start = start_secs.max(row_start);
                let visible_end = end_secs.min(row_start + row_secs);
                if visible_end <= visible_start {
                    continue;
                }
                let x0 = ((visible_start - row_start) / row_secs) as f32 * width as f32;
                let x1 = ((visible_end - row_start) / row_secs) as f32 * width as f32;
                fill_rect_f32(
                    &mut pixels,
                    width,
                    height,
                    x0,
                    line_top,
                    x1,
                    line_top + 1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 170),
                );
            }
            // 素材の先頭と末尾に境界線を引く。ストリップは中心固定なので、端に寄ると窓が
            // 素材の外へはみ出す。そこを未解析と同じ見た目のままにすると、**末尾より先へ
            // シークして次の動画へ移ってしまう** (利用者指摘 2026-08-26)。
            for boundary_secs in [0.0_f64, strip.duration_secs] {
                if boundary_secs < row_start || boundary_secs > row_start + row_secs {
                    continue;
                }
                let x = ((boundary_secs - row_start) / row_secs) as f32 * width as f32;
                let x0 = x.floor().clamp(0.0, (width.saturating_sub(1)) as f32);
                fill_rect_f32(
                    &mut pixels,
                    width,
                    height,
                    x0,
                    0.0,
                    x0 + 1.0,
                    waveform_h as f32,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200),
                );
            }
        }
    }
    let mut drawn_bins = 0;
    let mut visible_bins = Vec::new();
    for (index, bin) in bins.iter().enumerate() {
        let bin_end = bin.start_secs + bin.duration_secs;
        if bin_end < row_start || bin.start_secs > row_start + row_secs {
            continue;
        }
        let visible_start = bin.start_secs.max(row_start);
        let visible_end = bin_end.min(row_start + row_secs);
        let x0 = ((visible_start - row_start) / row_secs) as f32 * width as f32;
        let x1 = (((visible_end - row_start) / row_secs) as f32 * width as f32)
            .max(x0 + 1.0)
            .min(width as f32);
        visible_bins.push(TimelineVisibleBin { index, x0, x1 });
    }

    // A compact waveform surface has no metrics lane. Avoid doing the metric-only
    // pitch/chroma voting in that case; the positive-height music-view path is
    // intentionally unchanged so its raster snapshots remain identical.
    let (bass_hints, key_hints) = if metrics_h == 0 {
        (
            vec![TimelinePitchHint::default(); visible_bins.len()],
            vec![TimelineKeyHint::default(); visible_bins.len()],
        )
    } else {
        let rhythm_segments = build_timeline_rhythm_segments(bins, &visible_bins);
        (
            build_bass_root_display_hints(bins, &visible_bins, &rhythm_segments),
            build_key_display_hints(bins, &visible_bins, &rhythm_segments),
        )
    };

    for (visible_idx, visible) in visible_bins.iter().copied().enumerate() {
        let bin = &bins[visible.index];
        drawn_bins += 1;
        // L=上 / R=下 のステレオ波形。L/R が全てゼロ (既定構築 / 旧 bin) で mono があるなら
        // mono にフォールバックして従来どおり対称に描く (Codex P2: 既定 bin が無音/非対称に
        // ならないように)。
        let (peak_up, rms_up, peak_down, rms_down) =
            if bin.peak_l == 0.0 && bin.rms_l == 0.0 && bin.peak_r == 0.0 && bin.rms_r == 0.0 {
                (bin.peak, bin.rms, bin.peak, bin.rms)
            } else {
                (bin.peak_l, bin.rms_l, bin.peak_r, bin.rms_r)
            };
        let (outer_up, core_up) = spectral_waveform_half_heights(waveform_h, peak_up, rms_up);
        let (outer_down, core_down) =
            spectral_waveform_half_heights(waveform_h, peak_down, rms_down);
        draw_spectral_waveform_bin_pixels(
            &mut pixels,
            width,
            height,
            center_y,
            visible.x0,
            visible.x1,
            outer_up,
            core_up,
            outer_down,
            core_down,
            bin.band_energy,
        );
        if bin.transient > TRANSIENT_ACCENT_MIN {
            let transient = ((bin.transient - TRANSIENT_ACCENT_MIN) / (1.0 - TRANSIENT_ACCENT_MIN))
                .sqrt()
                .clamp(0.0, 1.0);
            let accent_half_h = waveform_h as f32 * (0.12 + transient * 0.22);
            let accent_center = ((visible.x0 + visible.x1) * 0.5).clamp(0.0, width as f32);
            let accent_half_w =
                ((visible.x1 - visible.x0).max(1.5) * (0.30 + transient * 0.45)).min(6.0);
            let accent = transient_color(bin.transient_band, transient);
            fill_rect_f32(
                &mut pixels,
                width,
                height,
                accent_center - accent_half_w,
                center_y - accent_half_h,
                accent_center + accent_half_w,
                center_y + accent_half_h,
                color_with_alpha(accent, (34.0 + transient * 76.0) as u8),
            );
            fill_rect_f32(
                &mut pixels,
                width,
                height,
                accent_center - 0.5,
                center_y - accent_half_h,
                accent_center + 0.5,
                center_y + accent_half_h,
                color_with_alpha(
                    brighten_color(accent, 1.18),
                    (54.0 + transient * 96.0) as u8,
                ),
            );
        }

        draw_timeline_metric_bins(
            &mut pixels,
            width,
            height,
            metrics_top,
            metrics_h,
            visible.x0,
            visible.x1,
            bin,
            bass_hints[visible_idx],
            key_hints[visible_idx],
        );
    }

    fill_rect_px(
        &mut pixels,
        width,
        height,
        0,
        waveform_h,
        width,
        (waveform_h + gap_h).min(height),
        egui::Color32::from_rgba_unmultiplied(36, 48, 60, 120),
    );
    draw_rect_stroke_px(
        &mut pixels,
        width,
        height,
        0,
        0,
        width,
        height,
        egui::Color32::from_rgba_unmultiplied(92, 112, 134, 112),
    );
    draw_rect_stroke_px(
        &mut pixels,
        width,
        height,
        0,
        0,
        width,
        waveform_h,
        egui::Color32::from_rgba_unmultiplied(74, 96, 120, 118),
    );
    if metrics_h > 0 {
        draw_rect_stroke_px(
            &mut pixels,
            width,
            height,
            0,
            metrics_top,
            width,
            height,
            egui::Color32::from_rgba_unmultiplied(74, 96, 120, 88),
        );
    }

    (egui::ColorImage::new([width, height], pixels), drawn_bins)
}

#[derive(Clone, Copy, Debug)]
struct TimelineVisibleBin {
    index: usize,
    x0: f32,
    x1: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct TimelinePitchHint {
    pitch_class: u8,
    confidence: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimelineKeyMode {
    Major,
    Minor,
}

#[derive(Clone, Copy, Debug)]
struct TimelineKeyHint {
    pitch_class: u8,
    mode: TimelineKeyMode,
    confidence: f32,
}

impl Default for TimelineKeyHint {
    fn default() -> Self {
        Self {
            pitch_class: 0,
            mode: TimelineKeyMode::Major,
            confidence: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct TimelineRhythmSegment {
    start: usize,
    end: usize,
}

fn build_timeline_rhythm_segments(
    bins: &[WaveformBin],
    visible_bins: &[TimelineVisibleBin],
) -> Vec<TimelineRhythmSegment> {
    let mut segments = Vec::new();
    if visible_bins.is_empty() {
        return segments;
    }

    let bin_secs = bins[visible_bins[0].index].duration_secs.max(0.005);
    let min_bins = (TIMELINE_ROOT_MIN_SEGMENT_SECS / bin_secs).ceil().max(1.0) as usize;
    let max_bins = (TIMELINE_ROOT_MAX_SEGMENT_SECS / bin_secs)
        .ceil()
        .max(min_bins as f64) as usize;

    let mut start = 0usize;
    while start < visible_bins.len() {
        let min_end = (start + min_bins).min(visible_bins.len());
        let max_end = (start + max_bins).min(visible_bins.len());
        let mut end = max_end.max(start + 1).min(visible_bins.len());

        if min_end < max_end {
            let mut best_idx = None;
            let mut best_score = TIMELINE_ROOT_TRANSIENT_THRESHOLD;
            for idx in min_end..max_end {
                let bin = &bins[visible_bins[idx].index];
                let score =
                    bin.transient * (0.55 + normalize_range(bin.band_energy[0], 0.05, 0.45) * 0.45);
                if score > best_score {
                    best_score = score;
                    best_idx = Some(idx);
                }
            }
            if let Some(idx) = best_idx {
                end = (idx + 1).max(start + 1);
            }
        }

        segments.push(TimelineRhythmSegment { start, end });
        start = end;
    }

    segments
}

fn build_bass_root_display_hints(
    bins: &[WaveformBin],
    visible_bins: &[TimelineVisibleBin],
    rhythm_segments: &[TimelineRhythmSegment],
) -> Vec<TimelinePitchHint> {
    let mut hints = vec![TimelinePitchHint::default(); visible_bins.len()];
    for segment in rhythm_segments {
        let hint = vote_bass_root_hint(bins, &visible_bins[segment.start..segment.end]);
        for slot in &mut hints[segment.start..segment.end] {
            *slot = hint;
        }
    }

    hints
}

fn build_key_display_hints(
    bins: &[WaveformBin],
    visible_bins: &[TimelineVisibleBin],
    rhythm_segments: &[TimelineRhythmSegment],
) -> Vec<TimelineKeyHint> {
    let mut hints = vec![TimelineKeyHint::default(); visible_bins.len()];
    for segment in rhythm_segments {
        if segment.start >= segment.end {
            continue;
        }
        let center_visible = &visible_bins[(segment.start + segment.end - 1) / 2];
        let center_bin = &bins[center_visible.index];
        let center_secs = center_bin.start_secs + center_bin.duration_secs * 0.5;
        let window_start = center_secs - TIMELINE_KEY_WINDOW_SECS * 0.5;
        let window_end = center_secs + TIMELINE_KEY_WINDOW_SECS * 0.5;
        let start_idx =
            bins.partition_point(|bin| bin.start_secs + bin.duration_secs <= window_start);
        let end_idx = bins.partition_point(|bin| bin.start_secs < window_end);
        let hint = vote_key_hint(&bins[start_idx..end_idx]);
        for slot in &mut hints[segment.start..segment.end] {
            *slot = hint;
        }
    }

    hints
}

fn vote_bass_root_hint(
    bins: &[WaveformBin],
    visible_bins: &[TimelineVisibleBin],
) -> TimelinePitchHint {
    let mut votes = [0.0_f32; 12];
    let mut count = 0usize;
    for visible in visible_bins {
        let bin = &bins[visible.index];
        let weight = timeline_metric_value(TimelineMetricKind::LoudnessBassRoot, bin).powi(2);
        votes[bin.bass_pitch_class as usize % 12] += weight;
        count += 1;
    }
    best_pitch_hint(votes, count)
}

fn vote_key_hint(bins: &[WaveformBin]) -> TimelineKeyHint {
    let mut chroma = [0.0_f32; 12];
    let mut presence = [0.0_f32; 12];
    let mut total_weight = 0.0_f32;
    for bin in bins {
        let weight = key_chroma_bin_weight(bin);
        if weight <= 1.0e-5 {
            continue;
        }
        let bin_max = bin.chroma.iter().copied().fold(0.0_f32, f32::max);
        for (pc, value) in bin.chroma.iter().copied().enumerate() {
            let value = value.max(0.0);
            chroma[pc] += value.powf(1.08) * weight;
            if bin_max > 1.0e-6 {
                presence[pc] += smoothstep(bin_max * 0.36, bin_max * 0.70, value) * weight.sqrt();
            }
        }
        total_weight += weight;
    }
    profile_key_hint(chroma, presence, total_weight)
}

fn key_hint_at_time(bins: &[WaveformBin], time_secs: f64) -> TimelineKeyHint {
    if bins.is_empty() || !time_secs.is_finite() {
        return TimelineKeyHint::default();
    }
    let window_start = time_secs - TIMELINE_KEY_WINDOW_SECS * 0.5;
    let window_end = time_secs + TIMELINE_KEY_WINDOW_SECS * 0.5;
    let start_idx = bins.partition_point(|bin| bin.start_secs + bin.duration_secs <= window_start);
    let end_idx = bins.partition_point(|bin| bin.start_secs < window_end);
    vote_key_hint(&bins[start_idx..end_idx])
}

fn key_chroma_bin_weight(bin: &WaveformBin) -> f32 {
    // Lightweight HPSS-style gate: keep sustained harmonic chroma and discount percussive bins.
    let loudness = timeline_loudness_value(bin).powf(1.18);
    let transient_keep = 1.0
        - TIMELINE_KEY_TRANSIENT_REDUCTION
            * smoothstep(
                TIMELINE_KEY_TRANSIENT_PENALTY_START,
                TIMELINE_KEY_TRANSIENT_PENALTY_END,
                bin.transient,
            );
    let density_keep = 1.0
        - TIMELINE_KEY_DENSITY_REDUCTION
            * smoothstep(
                TIMELINE_KEY_DENSITY_PENALTY_START,
                TIMELINE_KEY_DENSITY_PENALTY_END,
                bin.transient_density,
            );
    let confidence = 0.36 + bin.key_confidence.clamp(0.0, 1.0) * 0.64;
    (loudness * transient_keep * density_keep * confidence).clamp(0.0, 1.0)
}

fn profile_key_hint(chroma: [f32; 12], presence: [f32; 12], total_weight: f32) -> TimelineKeyHint {
    if total_weight <= 1.0e-6 || chroma.iter().copied().sum::<f32>() <= 1.0e-6 {
        return TimelineKeyHint::default();
    }

    let mut best = TimelineKeyCandidate::default();
    let mut second = TimelineKeyCandidate::default();
    for root in 0..12 {
        for mode in [TimelineKeyMode::Major, TimelineKeyMode::Minor] {
            let score = blended_key_profile_score(&chroma, &presence, root, mode);
            let candidate = TimelineKeyCandidate {
                pitch_class: root as u8,
                mode,
                score,
            };
            if candidate.score > best.score {
                second = best;
                best = candidate;
            } else if candidate.score > second.score {
                second = candidate;
            }
        }
    }

    let energy =
        (chroma.iter().copied().sum::<f32>() / total_weight.max(1.0e-6)).clamp(0.0, 4.0) / 4.0;
    let margin = (best.score - second.score).max(0.0);
    let score_support = smoothstep(-0.10, 0.32, best.score);
    let margin_support = smoothstep(0.0, 0.16, margin);
    let confidence =
        (score_support * (0.22 + margin_support * 0.78) * (0.45 + 0.55 * energy.sqrt()))
            .clamp(0.0, 1.0);

    TimelineKeyHint {
        pitch_class: best.pitch_class,
        mode: best.mode,
        confidence,
    }
}

fn blended_key_profile_score(
    chroma: &[f32; 12],
    presence: &[f32; 12],
    root: usize,
    mode: TimelineKeyMode,
) -> f32 {
    let (krumhansl, temperley) = match mode {
        TimelineKeyMode::Major => (
            TIMELINE_KEY_KRUMHANSL_MAJOR_PROFILE,
            TIMELINE_KEY_TEMPERLEY_MAJOR_PROFILE,
        ),
        TimelineKeyMode::Minor => (
            TIMELINE_KEY_KRUMHANSL_MINOR_PROFILE,
            TIMELINE_KEY_TEMPERLEY_MINOR_PROFILE,
        ),
    };
    let chroma_score = profile_correlation(chroma, &krumhansl, root);
    let presence_score = profile_correlation(presence, &temperley, root);
    chroma_score * TIMELINE_KEY_KRUMHANSL_WEIGHT + presence_score * TIMELINE_KEY_TEMPERLEY_WEIGHT
}

#[derive(Clone, Copy, Debug)]
struct TimelineKeyCandidate {
    pitch_class: u8,
    mode: TimelineKeyMode,
    score: f32,
}

impl Default for TimelineKeyCandidate {
    fn default() -> Self {
        Self {
            pitch_class: 0,
            mode: TimelineKeyMode::Major,
            score: f32::NEG_INFINITY,
        }
    }
}

fn profile_correlation(chroma: &[f32; 12], profile: &[f32; 12], root: usize) -> f32 {
    let chroma_mean = chroma.iter().copied().sum::<f32>() / 12.0;
    let profile_mean = profile.iter().copied().sum::<f32>() / 12.0;
    let mut numerator = 0.0_f32;
    let mut chroma_norm = 0.0_f32;
    let mut profile_norm = 0.0_f32;
    for pc in 0..12 {
        let chroma_value = chroma[pc] - chroma_mean;
        let degree = (pc + 12 - root) % 12;
        let profile_value = profile[degree] - profile_mean;
        numerator += chroma_value * profile_value;
        chroma_norm += chroma_value * chroma_value;
        profile_norm += profile_value * profile_value;
    }
    if chroma_norm <= 1.0e-8 || profile_norm <= 1.0e-8 {
        0.0
    } else {
        numerator / (chroma_norm.sqrt() * profile_norm.sqrt())
    }
}

fn best_pitch_hint(votes: [f32; 12], count: usize) -> TimelinePitchHint {
    let mut best_idx = 0usize;
    let mut best = 0.0_f32;
    let mut sum = 0.0_f32;
    for (idx, value) in votes.iter().copied().enumerate() {
        sum += value;
        if value > best {
            best = value;
            best_idx = idx;
        }
    }
    if sum <= 1.0e-6 || count == 0 {
        return TimelinePitchHint::default();
    }
    let dominance = (best / sum).clamp(0.0, 1.0);
    let support = (best / count as f32).sqrt().clamp(0.0, 1.0);
    TimelinePitchHint {
        pitch_class: best_idx as u8,
        confidence: (support * (0.42 + dominance * 0.58)).clamp(0.0, 1.0),
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_timeline_metric_bins(
    pixels: &mut [egui::Color32],
    width: usize,
    height: usize,
    metrics_top: usize,
    metrics_h: usize,
    x0: f32,
    x1: f32,
    bin: &WaveformBin,
    bass_hint: TimelinePitchHint,
    key_hint: TimelineKeyHint,
) {
    if metrics_h == 0 {
        return;
    }
    for (idx, kind) in TIMELINE_METRIC_KINDS.into_iter().enumerate() {
        let (top, bottom) = timeline_metric_lane_bounds(metrics_top, metrics_h, idx);
        let lane_bottom = bottom - 0.6;
        let lane_top = top + 0.6;
        let available_h = (lane_bottom - lane_top).max(1.0);
        let value = timeline_metric_display_value(kind, bin, bass_hint, key_hint).clamp(0.0, 1.0);
        let full_pitch_lane = matches!(kind, TimelineMetricKind::Key) && value > 0.03;
        let bar_top = if full_pitch_lane {
            lane_top
        } else {
            lane_bottom - available_h * value.max(0.025)
        };
        fill_rect_f32(
            pixels,
            width,
            height,
            x0,
            bar_top,
            x1,
            lane_bottom,
            timeline_metric_color(kind, value, bin, bass_hint, key_hint),
        );
        if idx > 0 {
            fill_rect_f32(
                pixels,
                width,
                height,
                0.0,
                top,
                width as f32,
                top + 0.5,
                egui::Color32::from_rgba_unmultiplied(80, 98, 118, 42),
            );
        }
    }
}

fn timeline_metric_lane_bounds(
    metrics_top: usize,
    metrics_h: usize,
    lane_idx: usize,
) -> (f32, f32) {
    timeline_metric_lane_bounds_f32(metrics_top as f32, metrics_h as f32, lane_idx)
}

fn timeline_metric_lane_bounds_f32(
    metrics_top: f32,
    metrics_h: f32,
    lane_idx: usize,
) -> (f32, f32) {
    let top = metrics_top;
    let bottom = metrics_top + metrics_h;
    let loudness_bottom = top + metrics_h * TIMELINE_LOUDNESS_ROOT_LANE_FRACTION;
    if lane_idx == 0 {
        (top, loudness_bottom)
    } else {
        (loudness_bottom, bottom)
    }
}

fn timeline_metric_display_value(
    kind: TimelineMetricKind,
    bin: &WaveformBin,
    bass_hint: TimelinePitchHint,
    key_hint: TimelineKeyHint,
) -> f32 {
    match kind {
        TimelineMetricKind::LoudnessBassRoot => {
            timeline_loudness_value(bin) * (0.72 + 0.28 * bass_hint.confidence.clamp(0.0, 1.0))
        }
        TimelineMetricKind::Key if key_hint.confidence > 0.0 => {
            TIMELINE_KEY_DISPLAY_FLOOR + key_hint.confidence * (1.0 - TIMELINE_KEY_DISPLAY_FLOOR)
        }
        TimelineMetricKind::Key => 0.0,
    }
}

fn timeline_metric_hover_value(
    kind: TimelineMetricKind,
    bins: &[WaveformBin],
    bin: &WaveformBin,
    time_secs: f64,
) -> f32 {
    match kind {
        TimelineMetricKind::LoudnessBassRoot => timeline_loudness_value(bin),
        TimelineMetricKind::Key => key_hint_at_time(bins, time_secs).confidence,
    }
}

fn timeline_metric_value(kind: TimelineMetricKind, bin: &WaveformBin) -> f32 {
    match kind {
        TimelineMetricKind::LoudnessBassRoot => (bin.bass_pitch_confidence
            * normalize_range(bin.band_energy[0], 0.06, 0.48)
            * timeline_loudness_value(bin).powf(0.90))
        .sqrt()
        .clamp(0.0, 1.0),
        TimelineMetricKind::Key => {
            (bin.key_confidence * timeline_loudness_value(bin).powf(0.72)).clamp(0.0, 1.0)
        }
    }
}

fn timeline_loudness_value(bin: &WaveformBin) -> f32 {
    ((bin.loudness_db + 52.0) / 52.0).clamp(0.0, 1.0).powf(0.72)
}

fn normalize_range(value: f32, low: f32, high: f32) -> f32 {
    if high <= low {
        return if value >= high { 1.0 } else { 0.0 };
    }
    ((value - low) / (high - low)).clamp(0.0, 1.0)
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    if (edge1 - edge0).abs() <= f32::EPSILON {
        return if value >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn timeline_metric_name(kind: TimelineMetricKind) -> &'static str {
    match kind {
        TimelineMetricKind::LoudnessBassRoot => "Loudness+Bass",
        TimelineMetricKind::Key => "Key",
    }
}

fn timeline_metric_description(kind: TimelineMetricKind) -> &'static str {
    match kind {
        TimelineMetricKind::LoudnessBassRoot => "Height RMS, color bass root",
        TimelineMetricKind::Key => "Transient-suppressed chroma profile",
    }
}

fn timeline_metric_extra(
    kind: TimelineMetricKind,
    bins: &[WaveformBin],
    bin: &WaveformBin,
    time_secs: f64,
) -> String {
    match kind {
        TimelineMetricKind::LoudnessBassRoot if bin.bass_pitch_confidence > 0.08 => {
            format!(" {}", pitch_class_name(bin.bass_pitch_class))
        }
        TimelineMetricKind::Key => {
            let hint = key_hint_at_time(bins, time_secs);
            if hint.confidence > 0.01 {
                format!(
                    " {} {}",
                    pitch_class_name(hint.pitch_class),
                    key_mode_label(hint.mode)
                )
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

fn timeline_metric_color(
    kind: TimelineMetricKind,
    value: f32,
    _bin: &WaveformBin,
    bass_hint: TimelinePitchHint,
    key_hint: TimelineKeyHint,
) -> egui::Color32 {
    let value = value.clamp(0.0, 1.0);
    let base = match kind {
        TimelineMetricKind::LoudnessBassRoot if bass_hint.confidence > 0.06 => {
            let color_value = (bass_hint.confidence * 0.78).clamp(0.32, 0.74);
            return color_with_alpha(
                brighten_color(
                    key_color(60 + bass_hint.pitch_class % 12, color_value),
                    0.86 + value * 0.16 + bass_hint.confidence * 0.12,
                ),
                (86.0 + value * 70.0 + bass_hint.confidence * 28.0).min(202.0) as u8,
            );
        }
        TimelineMetricKind::LoudnessBassRoot => egui::Color32::from_rgb(188, 198, 92),
        TimelineMetricKind::Key if key_hint.confidence > 0.0 => {
            let color_value = (0.36 + key_hint.confidence * 0.38).min(0.74);
            return color_with_alpha(
                brighten_color(
                    key_color(60 + key_hint.pitch_class % 12, color_value),
                    0.78 + key_hint.confidence * 0.30,
                ),
                (70.0 + key_hint.confidence * 120.0).min(190.0) as u8,
            );
        }
        TimelineMetricKind::Key => egui::Color32::from_rgb(104, 214, 186),
    };
    color_with_alpha(
        brighten_color(base, 0.42 + value * 0.72),
        (58.0 + value * 162.0) as u8,
    )
}

fn key_mode_label(mode: TimelineKeyMode) -> &'static str {
    match mode {
        TimelineKeyMode::Major => "maj",
        TimelineKeyMode::Minor => "min",
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimelineMetricKind {
    LoudnessBassRoot,
    Key,
}

const TIMELINE_METRIC_KINDS: [TimelineMetricKind; TIMELINE_METRIC_LANE_COUNT] = [
    TimelineMetricKind::LoudnessBassRoot,
    TimelineMetricKind::Key,
];

#[allow(clippy::too_many_arguments)]
/// 1 チャンネル分の (outer, core) 波形高さ (px) を peak / rms から求める。左右で同じスケール
/// を使うため関数化した。従来の mono 計算 (`amp=(peak.max(rms*2)).sqrt()`、core=0.42+rms.sqrt()*0.45)
/// をそのまま片チャンネルに適用する。
fn spectral_waveform_half_heights(waveform_h: usize, peak: f32, rms: f32) -> (f32, f32) {
    let amp = (peak.max(rms * 2.0)).sqrt().clamp(0.025, 1.0);
    let outer = (waveform_h as f32 * 0.46 * amp).max(1.0);
    let core_scale = 0.42 + rms.sqrt().clamp(0.0, 1.0) * 0.45;
    let core = (outer * core_scale).max(1.0).min(outer);
    (outer, core)
}

/// 上半分 = 左チャンネル (`outer_up` / `core_up`)、下半分 = 右チャンネル (`outer_down` /
/// `core_down`) の非対称波形を描く。モノラル素材は L==R で対称になり従来と同じ見た目になる。
#[allow(clippy::too_many_arguments)]
fn draw_spectral_waveform_bin_pixels(
    pixels: &mut [egui::Color32],
    width: usize,
    height: usize,
    center_y: f32,
    x0: f32,
    x1: f32,
    outer_up: f32,
    core_up: f32,
    outer_down: f32,
    core_down: f32,
    band: [f32; 3],
) {
    let weights = spectral_weights(band);
    let x0 = (x0 - 0.35).max(0.0);
    let x1 = (x1 + 0.35).min(width as f32).max(x0 + 0.75);
    fill_rect_f32(
        pixels,
        width,
        height,
        x0,
        center_y - outer_up,
        x1,
        center_y + outer_down,
        egui::Color32::from_rgba_unmultiplied(126, 104, 62, 52),
    );

    // 上 = L (direction -1.0)、下 = R (direction +1.0)。outer → core の順で重ねる。
    draw_spectral_half_pixels(
        pixels, width, height, x0, x1, center_y, -1.0, outer_up, weights, 88,
    );
    draw_spectral_half_pixels(
        pixels, width, height, x0, x1, center_y, 1.0, outer_down, weights, 88,
    );
    draw_spectral_half_pixels(
        pixels, width, height, x0, x1, center_y, -1.0, core_up, weights, 218,
    );
    draw_spectral_half_pixels(
        pixels, width, height, x0, x1, center_y, 1.0, core_down, weights, 218,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_spectral_half_pixels(
    pixels: &mut [egui::Color32],
    width: usize,
    height: usize,
    x0: f32,
    x1: f32,
    center_y: f32,
    direction: f32,
    half_h: f32,
    weights: [f32; 3],
    alpha: u8,
) {
    let colors = [
        egui::Color32::from_rgb(222, 154, 58),
        egui::Color32::from_rgb(126, 210, 90),
        egui::Color32::from_rgb(78, 186, 236),
    ];
    let mut cursor = center_y;
    for (idx, weight) in weights.into_iter().enumerate() {
        let h = (half_h * weight).max(0.0);
        if h < 0.35 {
            continue;
        }
        let next = cursor + direction * h;
        fill_rect_f32(
            pixels,
            width,
            height,
            x0,
            cursor.min(next),
            x1,
            cursor.max(next),
            color_with_alpha(colors[idx], alpha),
        );
        cursor = next;
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_rect_stroke_px(
    pixels: &mut [egui::Color32],
    width: usize,
    height: usize,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    color: egui::Color32,
) {
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    fill_rect_px(pixels, width, height, x0, y0, x1, (y0 + 1).min(y1), color);
    fill_rect_px(
        pixels,
        width,
        height,
        x0,
        y1.saturating_sub(1),
        x1,
        y1,
        color,
    );
    fill_rect_px(pixels, width, height, x0, y0, (x0 + 1).min(x1), y1, color);
    fill_rect_px(
        pixels,
        width,
        height,
        x1.saturating_sub(1),
        y0,
        x1,
        y1,
        color,
    );
}

#[allow(clippy::too_many_arguments)]
fn fill_rect_f32(
    pixels: &mut [egui::Color32],
    width: usize,
    height: usize,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    color: egui::Color32,
) {
    let x0 = x0.floor().clamp(0.0, width as f32) as usize;
    let y0 = y0.floor().clamp(0.0, height as f32) as usize;
    let x1 = x1.ceil().clamp(0.0, width as f32) as usize;
    let y1 = y1.ceil().clamp(0.0, height as f32) as usize;
    fill_rect_px(pixels, width, height, x0, y0, x1, y1, color);
}

#[allow(clippy::too_many_arguments)]
fn fill_rect_px(
    pixels: &mut [egui::Color32],
    width: usize,
    height: usize,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    color: egui::Color32,
) {
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let x1 = x1.min(width);
    let y1 = y1.min(height);
    for y in y0.min(height)..y1 {
        let row = y * width;
        for x in x0.min(width)..x1 {
            blend_pixel(&mut pixels[row + x], color);
        }
    }
}

fn blend_pixel(dst: &mut egui::Color32, src: egui::Color32) {
    let alpha = src.a() as u32;
    if alpha >= 255 {
        *dst = egui::Color32::from_rgb(src.r(), src.g(), src.b());
        return;
    }
    if alpha == 0 {
        return;
    }
    let inv = 255 - alpha;
    let blend = |s: u8, d: u8| ((s as u32 * alpha + d as u32 * inv + 127) / 255) as u8;
    *dst = egui::Color32::from_rgb(
        blend(src.r(), dst.r()),
        blend(src.g(), dst.g()),
        blend(src.b(), dst.b()),
    );
}

fn draw_beat_grid(
    painter: &egui::Painter,
    rect: egui::Rect,
    row_start: f64,
    row_secs: f64,
    analysis: &TimelineAnalysis,
) {
    if analysis.beat_grid.confidence < BEAT_GRID_MIN_CONFIDENCE {
        return;
    }
    let row_end = row_start + row_secs;
    let alpha_scale = ((analysis.beat_grid.confidence - BEAT_GRID_MIN_CONFIDENCE)
        / (1.0 - BEAT_GRID_MIN_CONFIDENCE))
        .clamp(0.0, 1.0);
    for beat in &analysis.beat_grid.beats {
        if beat.time_secs < row_start || beat.time_secs > row_end {
            continue;
        }
        let x = rect.left() + ((beat.time_secs - row_start) / row_secs) as f32 * rect.width();
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(
                0.5,
                egui::Color32::from_rgba_unmultiplied(
                    150,
                    190,
                    220,
                    (24.0 + alpha_scale * 54.0) as u8,
                ),
            ),
        );
    }
    let mut last_bar_label_x = f32::NEG_INFINITY;
    for bar in &analysis.beat_grid.bars {
        if bar.time_secs < row_start || bar.time_secs > row_end {
            continue;
        }
        let x = rect.left() + ((bar.time_secs - row_start) / row_secs) as f32 * rect.width();
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(
                1.5,
                egui::Color32::from_rgba_unmultiplied(
                    55,
                    170,
                    255,
                    (72.0 + alpha_scale * 92.0) as u8,
                ),
            ),
        );
        if x - last_bar_label_x >= 32.0 {
            painter.text(
                egui::pos2(x + 3.0, rect.top() + 3.0),
                egui::Align2::LEFT_TOP,
                (bar.index + 1).to_string(),
                egui::FontId::monospace(10.0),
                egui::Color32::from_rgba_unmultiplied(95, 185, 255, 210),
            );
            last_bar_label_x = x;
        }
    }
}

fn draw_playhead(
    painter: &egui::Painter,
    graph_rect: egui::Rect,
    position_secs: f64,
    row_secs: f64,
    row_h: f32,
    row_gap: f32,
) {
    if position_secs <= 0.0 || row_secs <= 0.0 {
        return;
    }
    let row = (position_secs / row_secs).floor() as usize;
    let row_start = row as f64 * row_secs;
    let row_top = graph_rect.min.y + row as f32 * (row_h + row_gap);
    let row_rect = egui::Rect::from_min_size(
        egui::pos2(graph_rect.min.x, row_top),
        egui::vec2(graph_rect.width(), row_h),
    );
    if !graph_rect.intersects(row_rect) {
        return;
    }
    let x = row_rect.left() + ((position_secs - row_start) / row_secs) as f32 * row_rect.width();
    painter.line_segment(
        [
            egui::pos2(x, row_rect.top()),
            egui::pos2(x, row_rect.bottom()),
        ],
        egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 230, 80)),
    );
}

// ── カラーヘルパー ──

fn pitch_class_name(pitch_class: u8) -> &'static str {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    NAMES[pitch_class as usize % 12]
}

fn key_color(midi: u8, value: f32) -> egui::Color32 {
    let pitch_class = midi % 12;
    let fifth_index = ((pitch_class as usize * 7) % 12) as f32;
    let hue = (fifth_index / 12.0 + 0.57) % 1.0;
    let saturation = 0.62 + value.clamp(0.0, 1.0) * 0.28;
    let brightness = 0.38 + value.clamp(0.0, 1.0) * 0.58;
    hsv_to_rgb(hue, saturation, brightness)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> egui::Color32 {
    let h = h.rem_euclid(1.0) * 6.0;
    let i = h.floor() as i32;
    let f = h - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    let (r, g, b) = match i.rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    egui::Color32::from_rgb(
        (r.clamp(0.0, 1.0) * 255.0) as u8,
        (g.clamp(0.0, 1.0) * 255.0) as u8,
        (b.clamp(0.0, 1.0) * 255.0) as u8,
    )
}

fn spectral_weights(band: [f32; 3]) -> [f32; 3] {
    let low = (band[0] * 0.95).sqrt().clamp(0.0, 1.0);
    let mid = (band[1] * 0.88).sqrt().clamp(0.0, 1.0);
    let high = (band[2] * 1.22).sqrt().clamp(0.0, 1.0);
    let sum = (low + mid + high).max(1.0e-6);
    [low / sum, mid / sum, high / sum]
}

fn transient_color(band: [f32; 3], strength: f32) -> egui::Color32 {
    let weights = spectral_weights(band);
    let low = weights[0];
    let mid = weights[1];
    let high = weights[2];
    let color = egui::Color32::from_rgb(
        ((238.0 * low + 205.0 * mid + 70.0 * high).min(255.0)) as u8,
        ((126.0 * low + 92.0 * mid + 220.0 * high).min(255.0)) as u8,
        ((32.0 * low + 210.0 * mid + 255.0 * high).min(255.0)) as u8,
    );
    brighten_color(color, 1.0 + strength.clamp(0.0, 1.0) * 0.24)
}

fn brighten_color(color: egui::Color32, scale: f32) -> egui::Color32 {
    egui::Color32::from_rgb(
        ((color.r() as f32 * scale).min(255.0)) as u8,
        ((color.g() as f32 * scale).min(255.0)) as u8,
        ((color.b() as f32 * scale).min(255.0)) as u8,
    )
}

fn color_with_alpha(color: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// タイムライン行ラベル / ホバー用の時刻整形 (h:mm:ss / m:ss)。
fn format_time(secs: f64) -> String {
    if !secs.is_finite() {
        return "--:--".to_string();
    }
    let secs = secs.max(0.0).round() as u64;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Row 秒数を次の選択肢へ巡回する (旧クリック巡回、後方互換用に残す)。
pub fn next_row_secs(current: f64) -> f64 {
    let idx = MUSIC_ROW_SECS_CHOICES
        .iter()
        .position(|c| (c - current).abs() < 0.5)
        .unwrap_or(0);
    MUSIC_ROW_SECS_CHOICES[(idx + 1) % MUSIC_ROW_SECS_CHOICES.len()]
}

/// Row 秒数を `delta` (±1) 段ずつ選択肢内で動かす (端でクランプ、巡回しない)。
/// 上情報バーの − / + ステッパー用。`delta < 0` で短く (詳細寄り)、`delta > 0` で長く。
pub fn step_row_secs(current: f64, delta: i32) -> f64 {
    let idx = MUSIC_ROW_SECS_CHOICES
        .iter()
        .position(|c| (c - current).abs() < 0.5)
        .unwrap_or(0) as i32;
    let n = MUSIC_ROW_SECS_CHOICES.len() as i32;
    let new_idx = (idx + delta).clamp(0, n - 1) as usize;
    MUSIC_ROW_SECS_CHOICES[new_idx]
}

pub fn music_timeline_playhead_center_scroll_offset(
    position_secs: f64,
    duration_secs: f64,
    row_secs: f64,
    viewport_h: f32,
) -> Option<f32> {
    let row_h = TIMELINE_WAVEFORM_H + TIMELINE_INNER_GAP + TIMELINE_METRICS_H;
    let rows = timeline_row_count(duration_secs, row_secs);
    music_timeline_playhead_center_scroll_offset_for_layout(
        position_secs,
        row_secs,
        viewport_h,
        row_h,
        TIMELINE_ROW_GAP,
        rows,
    )
}

pub fn music_timeline_playhead_center_scroll_offset_for_layout(
    position_secs: f64,
    row_secs: f64,
    viewport_h: f32,
    row_h: f32,
    row_gap: f32,
    rows: usize,
) -> Option<f32> {
    if rows == 0 || viewport_h <= 0.0 || row_h <= 0.0 || !viewport_h.is_finite() {
        return None;
    }
    let row = timeline_focus_row(position_secs, row_secs, rows)?;
    let stride = row_h + row_gap.max(0.0);
    let row_top = TIMELINE_CONTENT_PADDING_Y + row as f32 * stride;
    let row_center = row_top + row_h * 0.5;
    let content_h = TIMELINE_CONTENT_PADDING_Y * 2.0
        + rows as f32 * row_h
        + rows.saturating_sub(1) as f32 * row_gap.max(0.0);
    let max_offset = (content_h - viewport_h).max(0.0);
    Some((row_center - viewport_h * 0.5).clamp(0.0, max_offset))
}

/// Row 秒数の表示ラベル ("30s" / "2m")。
pub fn format_row_secs(secs: f64) -> String {
    if secs < 60.0 {
        format!("{secs:.0}s")
    } else {
        let minutes = secs / 60.0;
        if minutes.fract().abs() <= f64::EPSILON {
            format!("{minutes:.0}m")
        } else {
            format!("{minutes:.1}m")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_count_handles_edge_durations() {
        assert_eq!(timeline_row_count(0.0, 30.0), 1);
        assert_eq!(timeline_row_count(f64::NAN, 30.0), 1);
        assert_eq!(timeline_row_count(90.0, 30.0), 3);
        assert_eq!(timeline_row_count(91.0, 30.0), 4);
    }

    #[test]
    fn row_count_is_capped_for_garbage_durations() {
        // review-v2.3.0 hunt P1: 壊れたファイルのコンテナ duration (i64::MAX 級) で
        // 兆単位の row を確保して OOM/フリーズしない。
        assert_eq!(
            timeline_row_count(9_223_372_036_854.0, 1.0),
            TIMELINE_MAX_ROWS
        );
        assert_eq!(timeline_row_count(f64::MAX, 1.0), TIMELINE_MAX_ROWS);
        // 正常域はキャップの影響を受けない (24h 音声 @ row_secs=1.0)。
        assert_eq!(timeline_row_count(86_400.0, 1.0), 86_400);
    }

    #[test]
    fn timeline_seek_rejects_outside_graph() {
        // graph_rect: x=[100,300] (幅200), y=[0..]。row_h=40, row_gap=4, stride=44。
        // row_secs=30, duration=50 → rows=2 (0-30, 30-60。ただし曲は 50s で 2 行目は部分行)。
        let g = egui::Rect::from_min_max(egui::pos2(100.0, 0.0), egui::pos2(300.0, 400.0));
        let (row_h, row_gap, rows, row_secs, dur) = (40.0_f32, 4.0_f32, 2usize, 30.0_f64, 50.0_f64);
        let seek = |x: f32, y: f32| {
            timeline_seek_target_secs(egui::pos2(x, y), g, row_h, row_gap, rows, row_secs, dur)
        };
        // 行 0 の中央 (x=200 → frac 0.5) → 15s。
        assert_eq!(seek(200.0, 20.0), Some(15.0));
        // 行 0 の左端 → 0s。
        assert_eq!(seek(100.0, 20.0), Some(0.0));
        // x がラベル列側 (graph_rect 左端より左) → None。
        assert_eq!(seek(80.0, 20.0), None);
        // x が右 gutter 側 (graph_rect 右端より右) → None。
        assert_eq!(seek(320.0, 20.0), None);
        // 最終行より下の余白 (row=3 相当、y=140) → row >= rows で None。
        assert_eq!(seek(200.0, 140.0), None);
        // 行間 gap (row 0 の下端 40〜44) → pos.y > row_top+row_h で None。
        assert_eq!(seek(200.0, 42.0), None);
        // 部分行 (行 1 = 30-60s、曲は 50s)。行 1 の右端 (frac→1.0 → 60s ≥ duration 50) → None。
        assert_eq!(seek(300.0, 44.0 + 20.0), None);
        // 部分行の曲内 (行 1 の左端 x=100 → 30s < 50) → Some(30)。
        assert_eq!(seek(100.0, 44.0 + 20.0), Some(30.0));
    }

    #[test]
    fn next_row_secs_cycles_choices() {
        assert_eq!(next_row_secs(30.0), 60.0);
        assert_eq!(next_row_secs(120.0), 10.0);
        // 未知の値は先頭 → 2 番目へ。
        assert_eq!(next_row_secs(999.0), MUSIC_ROW_SECS_CHOICES[1]);
    }

    #[test]
    fn step_row_secs_clamps_at_ends() {
        assert_eq!(step_row_secs(30.0, 1), 60.0);
        assert_eq!(step_row_secs(30.0, -1), 15.0);
        // 端でクランプ (巡回しない)。
        assert_eq!(step_row_secs(10.0, -1), 10.0);
        assert_eq!(step_row_secs(120.0, 1), 120.0);
    }

    #[test]
    fn playhead_center_scroll_offset_keeps_zoomed_row_visible() {
        let row_h = 116.0;
        let row_gap = 12.0;
        let rows = 80;
        let viewport_h = 180.0;
        let position_secs = 305.0;
        let row_secs = 10.0;
        let offset = music_timeline_playhead_center_scroll_offset_for_layout(
            position_secs,
            row_secs,
            viewport_h,
            row_h,
            row_gap,
            rows,
        )
        .expect("valid playhead row");
        let row = timeline_focus_row(position_secs, row_secs, rows).unwrap();
        let row_top = TIMELINE_CONTENT_PADDING_Y + row as f32 * (row_h + row_gap);
        let row_bottom = row_top + row_h;

        assert!(
            offset <= row_top && offset + viewport_h >= row_bottom,
            "target offset {offset} should keep row {row} [{row_top}, {row_bottom}] inside viewport"
        );
    }

    #[test]
    fn playhead_center_scroll_offset_clamps_edges() {
        let first = music_timeline_playhead_center_scroll_offset_for_layout(
            0.0, 10.0, 240.0, 116.0, 12.0, 5,
        )
        .unwrap();
        assert_eq!(first, 0.0);

        let last = music_timeline_playhead_center_scroll_offset_for_layout(
            49.0, 10.0, 240.0, 116.0, 12.0, 5,
        )
        .unwrap();
        let content_h = TIMELINE_CONTENT_PADDING_Y * 2.0 + 5.0 * 116.0 + 4.0 * 12.0;
        assert_eq!(last, content_h - 240.0);
    }

    #[test]
    fn format_row_secs_labels() {
        assert_eq!(format_row_secs(30.0), "30s");
        assert_eq!(format_row_secs(60.0), "1m");
        assert_eq!(format_row_secs(90.0), "1.5m");
    }

    #[test]
    fn prioritized_rows_put_focus_first() {
        let rows = prioritized_timeline_request_rows(&[2, 3, 4], Some(3), false, 10);
        assert_eq!(rows[0], 3);
    }

    #[test]
    fn bins_for_raster_windows_around_row() {
        let bins: Vec<WaveformBin> = (0..100)
            .map(|i| WaveformBin {
                start_secs: i as f64,
                duration_secs: 1.0,
                ..WaveformBin::default()
            })
            .collect();
        let sub = timeline_bins_for_raster(&bins, 30.0, 30.0);
        // 30..60 の窓 + 前後 3s (KEY_WINDOW/2) パディング → 27..63 くらい。
        assert!(sub.first().unwrap().start_secs <= 27.0);
        assert!(sub.last().unwrap().start_secs >= 60.0);
    }

    fn strip_style(analyzed: &[(f64, f64)], duration_secs: f64) -> SeekStripRowStyle<'_> {
        SeekStripRowStyle {
            analyzed_spans: analyzed,
            duration_secs,
        }
    }

    /// 10x6 の帯では、波形レーンの実体は x=1..8 / y=1..4 (外周 1px は timeline 背景)。
    /// 窓 [-2, 8) に素材 [0, 5) を置くと、境界は x=2 と x=7、素材内は x=2..7。
    fn strip_probe() -> egui::ColorImage {
        let analyzed = [(0.0, 5.0)];
        let style = strip_style(&analyzed, 5.0);
        render_timeline_row_image(-2.0, 10.0, &[], Some(&style), 10, 6, 0, 0, true).0
    }

    #[test]
    fn unanalyzed_waveform_columns_are_dark_and_have_no_center_line() {
        let image = strip_probe();
        let pixel = |x: usize, y: usize| image.pixels[y * 10 + x];
        // 素材の中 (解析済み) は僅かに明るい背景 + 中心線。
        assert_eq!(pixel(4, 1), egui::Color32::from_rgb(6, 8, 10));
        assert!(
            pixel(4, 3).r() > 60,
            "analyzed center line {:?}",
            pixel(4, 3)
        );
        // 素材の外は素の黒のまま、中心線も引かない。無音 (中心線が見える) と区別が付く。
        assert_eq!(pixel(8, 1), egui::Color32::BLACK);
        assert_eq!(pixel(8, 3), egui::Color32::BLACK);
    }

    /// 無音の動画では中心線だけが見える。`center_y +- 0.5` は 2 行に半分ずつ乗るので、
    /// 音楽ビューと同じ 11% 白では**帯でほぼ見えなかった** (利用者報告 2026-08-26)。
    /// 行境界へ丸めて 1 行を満たす。音楽ビューの見た目は変えない。
    #[test]
    fn seek_strip_center_line_is_far_brighter_than_the_music_view_one() {
        let strip = strip_probe();
        let music = render_timeline_row_image(-2.0, 10.0, &[], None, 10, 6, 0, 0, true).0;
        let strip_line = strip.pixels[3 * 10 + 4];
        let music_line = music.pixels[3 * 10 + 4];
        assert!(
            u32::from(strip_line.r()) >= u32::from(music_line.r()) * 8,
            "strip center line {strip_line:?} must clearly outshine the music view one {music_line:?}"
        );
        // 音楽ビューは 2 行に分かれたまま。ここが変わるとスナップショットが動く。
        assert_eq!(music.pixels[2 * 10 + 4], music_line);
    }

    /// 帯は中心固定なので、端へ寄ると窓が素材の外へはみ出す。境界が分からないと、
    /// **末尾より先へシークして次の動画へ移ってしまう** (利用者指摘 2026-08-26)。
    #[test]
    fn seek_strip_marks_the_start_and_end_of_the_material() {
        let image = strip_probe();
        let pixel = |x: usize, y: usize| image.pixels[y * 10 + x];
        let center = pixel(4, 3);
        for y in 1..5 {
            for x in [2usize, 7] {
                assert!(
                    pixel(x, y).r() > center.r(),
                    "boundary at ({x},{y}) = {:?} must stand out over the center line {center:?}",
                    pixel(x, y)
                );
            }
        }
        // 境界線はレーンの全高に伸びる (中心行だけではない)。
        assert!(pixel(2, 1).r() > 100 && pixel(2, 4).r() > 100);
    }

    /// 音楽ビューには境界線を引かない (行は常に素材の内側)。
    #[test]
    fn music_view_rows_have_no_material_boundary() {
        let image = render_timeline_row_image(-2.0, 10.0, &[], None, 10, 6, 0, 0, true).0;
        for x in 1..9 {
            let p = image.pixels[1 * 10 + x];
            assert_eq!(p, egui::Color32::BLACK, "music view x={x} must stay plain");
        }
    }
}
