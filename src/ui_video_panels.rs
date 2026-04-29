//! 動画フルスクリーン専用の左右サイドパネル (Phase 5.4)。
//!
//! 画像のフルスクリーンでは:
//! - 左: 画像補正パネル (`ui_adjustment_panel`)
//! - 右: メタデータパネル (EXIF / AI 情報)
//!
//! 動画では役割を入れ替えて:
//! - 左: ジャンプターゲット一覧 (ピン / ブックマーク / チャプター) — 5.4.1 で実装
//! - 右: 動画メタ情報 (タイトル / アーティスト / 説明 / コーデック / チャプター)
//!
//! 本モジュールは UI スレッドからのみ呼ばれる純粋な描画 + 入力ハンドラで、
//! 状態は VideoPlayer / Settings / video_bookmarks DB を通して取得する。

use eframe::egui;

use crate::app::App;
use crate::video::decoder::VideoInfo;

/// 右側パネルの幅 (画像と揃える)。
const VIDEO_PANEL_WIDTH: f32 = 380.0;
/// フルスクリーン上部バーの高さ (`ui_metadata_panel` の `TOP_BAR_H` と同期)。
const TOP_BAR_H: f32 = 44.0;
/// パネル内タイトル行の高さ。
const TITLE_BAR_H: f32 = 32.0;

const LABEL_COLOR: egui::Color32 = egui::Color32::from_rgb(140, 160, 200);
const TEXT_COLOR: egui::Color32 = egui::Color32::from_rgb(230, 230, 230);
const DIM_COLOR: egui::Color32 = egui::Color32::from_rgb(150, 150, 150);

impl App {
    /// 動画フルスクリーン中の **右パネル** (= メタ情報) を描画する。
    /// 戻り値: パネルが描画された (= 表示中) ら `true`、隠れていれば `false`。
    pub(crate) fn draw_video_metadata_panel(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        fs_idx: usize,
    ) -> bool {
        // 表示判定: TAB 固定 OR 画面右 1/4 にカーソル
        let panel_w = VIDEO_PANEL_WIDTH.min(full_rect.width() * 0.5);
        let hover_threshold = full_rect.max.x - full_rect.width() * 0.25;
        let hover_in_right = ctx.input(|i| {
            i.pointer
                .hover_pos()
                .map(|p| p.x > hover_threshold)
                .unwrap_or(false)
        });
        if !self.show_metadata_panel && !hover_in_right {
            return false;
        }

        // VideoPlayer から info() を取り出す。未着のうちはパネル自体は出すがコンテンツが
        // 「読み込み中...」になる (動画 metadata は open 直後に来るので一瞬だけ)。
        let info: Option<VideoInfo> = match self.fs_cache.get(&fs_idx) {
            Some(crate::fs_animation::FsCacheEntry::Video { player, .. }) => {
                player.info().cloned()
            }
            _ => None,
        };

        let panel_top = full_rect.min.y + TOP_BAR_H;
        let panel_rect = egui::Rect::from_min_max(
            egui::pos2(full_rect.max.x - panel_w, panel_top),
            full_rect.max,
        );

        let painter = ui.painter().clone();
        painter.rect_filled(
            panel_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(18, 18, 22, 230),
        );
        painter.line_segment(
            [panel_rect.left_top(), panel_rect.left_bottom()],
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40),
            ),
        );
        // 背景クリックを消費 (動画クリックの toggle_play に伝搬しない)
        let _ = ui.interact(
            panel_rect,
            egui::Id::new(("video_metadata_panel_bg", fs_idx)),
            egui::Sense::click(),
        );

        // ── タイトルバー ──
        let title_rect = egui::Rect::from_min_size(
            panel_rect.min,
            egui::vec2(panel_rect.width(), TITLE_BAR_H),
        );
        painter.rect_filled(
            title_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(30, 30, 38, 240),
        );
        painter.line_segment(
            [
                egui::pos2(title_rect.min.x, title_rect.max.y),
                egui::pos2(title_rect.max.x, title_rect.max.y),
            ],
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40),
            ),
        );
        painter.text(
            egui::pos2(panel_rect.min.x + 12.0, title_rect.center().y),
            egui::Align2::LEFT_CENTER,
            "動画メタ情報",
            egui::FontId::proportional(14.0),
            TEXT_COLOR,
        );

        // ── 内容: スクロール領域内に縦並びで表示 ──
        let content_rect = egui::Rect::from_min_max(
            egui::pos2(panel_rect.min.x, title_rect.max.y),
            panel_rect.max,
        );
        let mut seek_to: Option<f64> = None;
        let mut content_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(content_rect)
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
        );
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(&mut content_ui, |ui| {
                ui.add_space(8.0);

                if let Some(info) = info.as_ref() {
                    draw_kv_section(ui, info);
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);
                    seek_to = draw_chapters_section(ui, info);
                } else {
                    ui.add_space(8.0);
                    ui.colored_label(DIM_COLOR, "読み込み中...");
                }

                ui.add_space(16.0);
            });

        // クリックで seek (UI スレッド内のため借用衝突は出ない)
        if let Some(t) = seek_to {
            if let Some(crate::fs_animation::FsCacheEntry::Video { player, .. }) =
                self.fs_cache.get(&fs_idx)
            {
                // 5.1 と同じ流儀で apply_command(Play) 経由は user-seek に任せ、
                // ここではダイレクトに seek して current 状態を維持する
                // (チャプター ジャンプの一般的挙動)。
                player.seek(t);
            }
        }

        true
    }
}

fn draw_kv_section(ui: &mut egui::Ui, info: &VideoInfo) {
    let put = |ui: &mut egui::Ui, label: &str, value: &str| {
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.colored_label(LABEL_COLOR, label);
        });
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.colored_label(TEXT_COLOR, value);
        });
        ui.add_space(2.0);
    };

    if let Some(t) = info.title.as_deref() {
        put(ui, "タイトル", t);
    }
    if let Some(a) = info.artist.as_deref() {
        put(ui, "作成者", a);
    }
    if let Some(d) = info.description.as_deref() {
        put(ui, "説明", d);
    }

    let dur_str = format_secs(info.duration_secs);
    put(ui, "長さ", &dur_str);

    let res_str = format!("{} × {} px", info.width, info.height);
    put(ui, "解像度", &res_str);

    if info.avg_fps > 0.0 {
        let fps_str = format!("{:.2} fps", info.avg_fps);
        put(ui, "フレームレート", &fps_str);
    }

    put(ui, "動画コーデック", &info.video_codec);

    if let Some(ac) = info.audio_codec.as_deref() {
        put(ui, "音声コーデック", ac);
    } else {
        put(ui, "音声", "なし");
    }

    let path_label = if info.gpu_path_active {
        "GPU (D3D11 zero-copy)"
    } else {
        "CPU (readback + swscale)"
    };
    put(ui, "経路", path_label);

    let decode_label = if info.hw_decode_active { "HW" } else { "SW" };
    put(ui, "デコーダ", decode_label);
}

fn draw_chapters_section(ui: &mut egui::Ui, info: &VideoInfo) -> Option<f64> {
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.colored_label(LABEL_COLOR, "チャプター");
    });
    ui.add_space(2.0);
    if info.chapters.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.colored_label(DIM_COLOR, "(なし)");
        });
        return None;
    }
    let mut clicked: Option<f64> = None;
    for c in info.chapters.iter() {
        let label_time = format_secs(c.start_secs);
        let label_text = c.title.as_deref().unwrap_or("(無題)");
        let resp = ui.horizontal(|ui| {
            ui.add_space(20.0);
            let r1 = ui.add(
                egui::Button::new(
                    egui::RichText::new(format!("{label_time}  {label_text}"))
                        .color(TEXT_COLOR)
                        .size(13.0),
                )
                .frame(false),
            );
            r1
        });
        let btn = resp.inner;
        if btn.clicked() {
            clicked = Some(c.start_secs);
        }
        ui.add_space(2.0);
    }
    clicked
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
