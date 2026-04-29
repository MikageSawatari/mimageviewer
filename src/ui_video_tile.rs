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

use std::path::PathBuf;

use eframe::egui;

use crate::app::App;
use crate::fs_animation::FsCacheEntry;
use crate::video::tile_thumbnails::{TileThumbnail, TileThumbnailWorker};

// 列数候補は `crate::settings::VIDEO_TILE_COLUMN_CANDIDATES` (= 6/10/16/20/26/30)。
// `Settings.video_tile_columns` が source of truth。Ctrl+Wheel で次/前の候補に
// 切替し、Setting に保存する。

/// 候補となる間隔 (秒)。タイル数が画面に収まる範囲で最大の間隔を選ぶ
/// (= タイル数最少 = 抽出待ち最短)。
const INTERVAL_CANDIDATES_SECS: &[f64] = &[
    1.0, 2.0, 5.0, 10.0, 20.0, 30.0,
    60.0, 120.0, 300.0, 600.0, 1200.0, 1800.0,
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
            // Codex P5.5 H2 反映: state Drop だけでは texture cache がクリアされない
            // (Drop impl が無いため)。閉じる側で明示的にクリアしてリーク防止。
            self.video_tile_state = None;
            self.video_tile_textures.clear();
            return;
        }
        // 古い texture が残っていれば再 open 前にもクリア (= 異なる動画 / 異なる
        // 列数の grid から切り替えるとき、古いキーで残った texture が誤マッチする
        // のを防ぐ)。
        self.video_tile_textures.clear();
        let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx)
        else {
            return;
        };
        let Some(info) = player.info().cloned() else {
            return;
        };
        if info.duration_secs <= 0.5 {
            return;
        }
        let path = player.path().clone();
        let aspect = if info.height > 0 {
            info.width as f64 / info.height as f64
        } else {
            16.0 / 9.0
        };

        // タイルサイズと列数: Setting の video_tile_columns を優先。範囲外なら 10 に
        // 戻して保存。サムネ幅は画面幅 / 列数 (横向き)、高さは aspect で逆算。
        let mut columns = self.settings.video_tile_columns;
        if !crate::settings::VIDEO_TILE_COLUMN_CANDIDATES.contains(&columns) {
            columns = 10;
            self.settings.video_tile_columns = 10;
        }
        // 画面幅 (left/right に余白 16px を確保)。
        let usable_w = (screen_size.x - 32.0).max(200.0);
        let tile_w = (usable_w / columns as f32).floor().max(40.0) as u32;
        let tile_h = ((tile_w as f64) / aspect).round().max(30.0) as u32;

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
            return;
        }

        let worker = TileThumbnailWorker::spawn(
            path.clone(),
            timestamps.clone(),
            tile_w,
            tile_h,
        );
        self.video_tile_state = Some(VideoTileState {
            video_path: path,
            interval_secs: interval,
            worker,
            timestamps,
            tile_w,
            tile_h,
            columns,
        });
    }

    /// 動画タイル モードのオーバーレイを描画する。再生中の他の入力 (= クリック →
    /// toggle_play、ジャンプパネル等) は **タイルモード中は抑止** する想定で、
    /// 呼び出し側 (ui_fullscreen) で early-return する。
    /// 戻り値: モードが描画された (= active) ら true。
    #[cfg(windows)]
    pub(crate) fn draw_video_tile_overlay(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        fs_idx: usize,
    ) -> bool {
        // state から必要な値を **コピー** して借用を切る (= 後で self を mutable borrow
        // するため)。snapshot / progress / is_finished は全て Arc<Mutex> 越しのコピーなので
        // 軽い。
        let (
            state_video_path,
            state_interval_secs,
            state_timestamps,
            state_tile_w,
            state_tile_h,
            state_columns,
            snapshot,
            progress_done,
            progress_total,
            worker_finished,
        ) = {
            let Some(state) = self.video_tile_state.as_ref() else {
                return false;
            };
            let (done, total) = state.worker.progress();
            (
                state.video_path.clone(),
                state.interval_secs,
                state.timestamps.clone(),
                state.tile_w,
                state.tile_h,
                state.columns,
                state.worker.snapshot(),
                done,
                total,
                state.worker.is_finished(),
            )
        };

        // 動画切替を検知 → 自動 close
        let cur_path = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => Some(player.path().clone()),
            _ => None,
        };
        if cur_path.as_ref() != Some(&state_video_path) {
            self.video_tile_state = None;
            self.video_tile_textures.clear();
            return false;
        }

        // 黒背景で全画面を覆う
        let painter = ui.painter().clone();
        painter.rect_filled(
            full_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 230),
        );
        // 背景クリックを消費 (= toggle_play などの catch-all を抑止)
        let _ = ui.interact(
            full_rect,
            egui::Id::new(("video_tile_bg", fs_idx)),
            egui::Sense::click(),
        );

        // タイトル + 進捗
        let header = format!(
            "タイル モード — 間隔 {} / 進捗 {progress_done}/{progress_total} (S または ESC で閉じる)",
            format_interval(state_interval_secs)
        );
        painter.text(
            egui::pos2(full_rect.min.x + 16.0, full_rect.min.y + 24.0),
            egui::Align2::LEFT_CENTER,
            header,
            egui::FontId::proportional(14.0),
            egui::Color32::from_rgb(220, 220, 220),
        );

        // タイルグリッド
        let columns = state_columns;
        let tile_w = state_tile_w as f32;
        let tile_h = state_tile_h as f32;
        let label_h = 16.0;
        let gap_x = 6.0;
        let gap_y = 6.0;
        let total_grid_w = (tile_w + gap_x) * columns as f32 - gap_x;
        let grid_left = full_rect.min.x + (full_rect.width() - total_grid_w) * 0.5;
        let grid_top = full_rect.min.y + 56.0;

        let mut clicked_pts: Option<f64> = None;

        for (idx, slot) in snapshot.iter().enumerate() {
            let col = idx % columns;
            let row = idx / columns;
            let x0 = grid_left + (tile_w + gap_x) * col as f32;
            let y0 = grid_top + (tile_h + label_h + gap_y) * row as f32;
            let tile_rect = egui::Rect::from_min_size(
                egui::pos2(x0, y0),
                egui::vec2(tile_w, tile_h),
            );
            // 画面下端を超えるタイルはスキップ (描画しない、列の続きが画面外に出る場合)
            if tile_rect.max.y > full_rect.max.y - 20.0 {
                continue;
            }
            // 背景 (黒くより淡い灰色) + サムネ画像 + 枠
            painter.rect_filled(
                tile_rect,
                4.0,
                egui::Color32::from_rgba_unmultiplied(35, 35, 40, 255),
            );
            if let Some(t) = slot {
                let tex_id = self.upload_video_tile_texture(ctx, idx, t);
                painter.image(
                    tex_id,
                    tile_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                painter.text(
                    tile_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "...",
                    egui::FontId::proportional(20.0),
                    egui::Color32::from_gray(120),
                );
            }
            painter.rect_stroke(
                tile_rect,
                4.0,
                egui::Stroke::new(1.0, egui::Color32::from_gray(80)),
                egui::StrokeKind::Inside,
            );
            // mm:ss ラベル (タイル直下)
            let pts = state_timestamps.get(idx).copied().unwrap_or(0.0);
            painter.text(
                egui::pos2(tile_rect.center().x, tile_rect.max.y + label_h * 0.5),
                egui::Align2::CENTER_CENTER,
                format_secs(pts),
                egui::FontId::proportional(12.0),
                egui::Color32::from_rgb(220, 220, 220),
            );

            // クリック判定
            let resp = ui.interact(
                tile_rect,
                egui::Id::new(("video_tile", fs_idx, idx)),
                egui::Sense::click(),
            );
            if resp.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.clicked() {
                clicked_pts = Some(pts);
            }
        }

        // Ctrl+Wheel で列数候補を切替 (Phase 6.D)。タイル中のみ有効。
        let wheel_y = ctx.input(|i| {
            if i.modifiers.ctrl {
                i.smooth_scroll_delta.y
            } else {
                0.0
            }
        });
        if wheel_y.abs() > 0.5 {
            let cur = self.settings.video_tile_columns;
            let cands = crate::settings::VIDEO_TILE_COLUMN_CANDIDATES;
            let idx = cands.iter().position(|&v| v == cur).unwrap_or(1);
            // wheel_y > 0 = 上回転 = 列数を **減らす** (= 1 タイルが大きくなる、直感的)
            // wheel_y < 0 = 下回転 = 列数を **増やす**
            let new_idx = if wheel_y > 0.0 {
                idx.saturating_sub(1)
            } else {
                (idx + 1).min(cands.len() - 1)
            };
            if new_idx != idx {
                let new_cols = cands[new_idx];
                self.settings.video_tile_columns = new_cols;
                self.settings.save();
                // 列数変わると tile_w/tile_h と timestamps が変わるので、現在の
                // state を捨てて再 spawn。
                let video_path = state_video_path.clone();
                let cur_path = match self.fs_cache.get(&fs_idx) {
                    Some(FsCacheEntry::Video { player, .. }) => Some(player.path().clone()),
                    _ => None,
                };
                if cur_path.as_ref() == Some(&video_path) {
                    self.video_tile_state = None;
                    self.video_tile_textures.clear();
                    let screen = ctx.content_rect().size();
                    self.toggle_video_tile_mode(fs_idx, screen);
                }
                return true;
            }
        }

        // タイル抽出が進行中なら毎フレーム repaint (= 進捗を反映)。
        if !worker_finished {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }

        if let Some(pts) = clicked_pts {
            // クリック → seek + close
            if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                player.seek(pts);
            }
            self.video_tile_state = None;
            self.video_tile_textures.clear();
        }

        true
    }

    /// タイル 1 個分のテクスチャをアップロード (キャッシュは簡易: id ごとに 1 件保持
    /// で、別 idx で上書き)。Phase 5.5 では VideoTileState 内に専用キャッシュを
    /// 持たないシンプル実装。再描画ごとに texture が増える可能性があるため、
    /// 期間が長いタスクで GPU メモリを食う場合は将来的にキャッシュ追加。
    #[cfg(windows)]
    fn upload_video_tile_texture(
        &mut self,
        ctx: &egui::Context,
        slot_idx: usize,
        thumb: &TileThumbnail,
    ) -> egui::TextureId {
        let key = (slot_idx as u64, thumb.pts_secs.to_bits(), thumb.width, thumb.height);
        // 既存キャッシュにヒットすればそのまま返す
        if let Some((k, tex)) = self.video_tile_textures.get(&slot_idx) {
            if *k == key {
                return tex.id();
            }
        }
        let img = egui::ColorImage::from_rgba_unmultiplied(
            [thumb.width as usize, thumb.height as usize],
            &thumb.rgba,
        );
        let tex = ctx.load_texture(
            format!("video_tile:{slot_idx}"),
            img,
            egui::TextureOptions::LINEAR,
        );
        let id = tex.id();
        self.video_tile_textures.insert(slot_idx, (key, tex));
        id
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

fn format_interval(secs: f64) -> String {
    if secs >= 60.0 {
        let m = (secs / 60.0).round() as i64;
        format!("{m} 分")
    } else {
        format!("{} 秒", secs.round() as i64)
    }
}

fn format_secs(s: f64) -> String {
    let total = s.max(0.0).round() as i64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let sec = total % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{sec:02}")
    } else {
        format!("{m:02}:{sec:02}")
    }
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
