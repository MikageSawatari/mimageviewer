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

use std::path::PathBuf;

use eframe::egui;

use crate::app::App;
use crate::video::decoder::{Chapter, VideoInfo};
use crate::video_bookmarks::VideoBookmark;

/// 右側パネルの幅 (画像と揃える)。
const VIDEO_PANEL_WIDTH: f32 = 380.0;
/// 左側ジャンプパネルの幅。
const VIDEO_JUMP_PANEL_WIDTH: f32 = 280.0;
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
        // Phase 6: チャプターは左ジャンプパネルに集約したため、右パネルからは削除。
        // 右パネルは「動画自体の固定情報」(タイトル / コーデック / 長さ等) のみ。
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
                } else {
                    ui.add_space(8.0);
                    ui.colored_label(DIM_COLOR, "読み込み中...");
                }
                ui.add_space(16.0);
            });

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

/// 左パネルでクリックされた行から発生する操作。
#[derive(Debug)]
pub(crate) enum JumpPanelAction {
    /// 指定秒に seek (ブックマーク / チャプター 共通)。
    Seek(f64),
    /// このブックマーク id を削除。
    DeleteBookmark(i64),
    /// 現在位置にブックマークを追加 (= 上部 🔖 ボタン)。
    AddBookmarkHere,
}

impl App {
    /// 動画フルスクリーン中の **左パネル** (= ジャンプ先サムネ) を描画する。
    /// 戻り値: パネルが描画された (= 表示中) ら `true`、隠れていれば `false`。
    /// クリック / 削除アクションは内部で処理する (= seek / bookmark 削除)。
    pub(crate) fn draw_video_jump_panel(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        fs_idx: usize,
    ) -> bool {
        // 表示判定: 画面左 1/4 にカーソル がある間表示。
        // (画像の adjustment_active と違って動画では常時固定モードを今は持たないが、
        //  今後 Phase 5.6 でホバーバーから固定可にする想定。)
        let panel_w = VIDEO_JUMP_PANEL_WIDTH.min(full_rect.width() * 0.4);
        let hover_threshold = full_rect.min.x + full_rect.width() * 0.25;
        let hover_in_left = ctx.input(|i| {
            i.pointer
                .hover_pos()
                .map(|p| p.x < hover_threshold)
                .unwrap_or(false)
        });
        if !hover_in_left {
            return false;
        }

        // 動画パスとピン/ブックマーク読み出し
        let video_path: Option<PathBuf> = match self.fs_cache.get(&fs_idx) {
            Some(crate::fs_animation::FsCacheEntry::Video { player, .. }) => {
                Some(player.path().clone())
            }
            _ => None,
        };
        let Some(video_path) = video_path else {
            return false;
        };
        let bookmarks: Vec<VideoBookmark> = self
            .video_bookmark_db
            .as_ref()
            .map(|db| db.list(&video_path))
            .unwrap_or_default();
        let chapters: Vec<Chapter> = match self.fs_cache.get(&fs_idx) {
            Some(crate::fs_animation::FsCacheEntry::Video { player, .. }) => {
                player.info().map(|i| i.chapters.clone()).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        // Phase 6: 動画モードの左パネルは常時表示候補 (= 🔖 ボタンがあるため、
        // 中身が空でもパネル自体は出す)。

        let panel_top = full_rect.min.y + TOP_BAR_H;
        let panel_rect = egui::Rect::from_min_max(
            egui::pos2(full_rect.min.x, panel_top),
            egui::pos2(full_rect.min.x + panel_w, full_rect.max.y),
        );

        let painter = ui.painter().clone();
        painter.rect_filled(
            panel_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(18, 18, 22, 230),
        );
        painter.line_segment(
            [panel_rect.right_top(), panel_rect.right_bottom()],
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40),
            ),
        );
        let _ = ui.interact(
            panel_rect,
            egui::Id::new(("video_jump_panel_bg", fs_idx)),
            egui::Sense::click(),
        );

        // タイトルバー
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
            "ジャンプ",
            egui::FontId::proportional(14.0),
            TEXT_COLOR,
        );

        let content_rect = egui::Rect::from_min_max(
            egui::pos2(panel_rect.min.x, title_rect.max.y),
            panel_rect.max,
        );
        let mut action: Option<JumpPanelAction> = None;
        let mut content_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(content_rect)
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
        );
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(&mut content_ui, |ui| {
                ui.add_space(6.0);
                // 🔖 ブックマーク追加ボタン (Phase 6 — 上部固定)
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    let resp = ui.add(
                        egui::Button::new(
                            egui::RichText::new("🔖 現在位置にブックマーク追加 [B]")
                                .color(TEXT_COLOR)
                                .size(13.0),
                        ),
                    );
                    if resp.clicked() {
                        action = Some(JumpPanelAction::AddBookmarkHere);
                    }
                });
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                if !bookmarks.is_empty() {
                    section_label(ui, "🔖 ブックマーク");
                    for b in bookmarks.iter() {
                        if let Some(act) = draw_jump_row(
                            ui,
                            "🔖",
                            b.pts_secs,
                            b.title.as_deref(),
                            Some(b.id),
                        ) {
                            action = Some(act);
                        }
                    }
                    ui.add_space(8.0);
                }

                if !chapters.is_empty() {
                    section_label(ui, "📑 チャプター");
                    for c in chapters.iter() {
                        if let Some(act) = draw_jump_row(
                            ui,
                            "📑",
                            c.start_secs,
                            c.title.as_deref(),
                            None,
                        ) {
                            action = Some(act);
                        }
                    }
                    ui.add_space(8.0);
                }

                if bookmarks.is_empty() && chapters.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(20.0);
                        ui.colored_label(
                            DIM_COLOR,
                            "ブックマーク・チャプターはまだありません",
                        );
                    });
                }

                ui.add_space(16.0);
            });

        if let Some(act) = action {
            match act {
                JumpPanelAction::Seek(t) => {
                    if let Some(crate::fs_animation::FsCacheEntry::Video { player, .. }) =
                        self.fs_cache.get(&fs_idx)
                    {
                        player.seek(t);
                    }
                }
                JumpPanelAction::DeleteBookmark(id) => {
                    if let Some(db) = self.video_bookmark_db.as_ref() {
                        let _ = db.remove(id);
                    }
                }
                JumpPanelAction::AddBookmarkHere => {
                    self.add_video_bookmark_at_current(fs_idx);
                }
            }
        }

        true
    }

    /// 現在の再生位置に新規ブックマークを追加する。B キー / 🔖 ボタンの両経路から呼ばれる。
    /// info() が無い (= Loading 中) や error 状態では no-op にする
    /// (= Codex Phase 5.4 M2 の guard を再利用)。
    pub(crate) fn add_video_bookmark_at_current(&mut self, fs_idx: usize) {
        let snapshot = match self.fs_cache.get(&fs_idx) {
            Some(crate::fs_animation::FsCacheEntry::Video { player, .. }) => {
                if player.error().is_some() || player.info().is_none() {
                    None
                } else {
                    Some((player.path().clone(), player.position()))
                }
            }
            _ => None,
        };
        if let (Some((path, pts)), Some(db)) =
            (snapshot, self.video_bookmark_db.as_ref())
        {
            if let Err(e) = db.add(&path, pts, None, &[]) {
                crate::logger::log(format!("video bookmark add failed: {e}"));
            } else {
                crate::logger::log(format!(
                    "video bookmark added: pts={pts:.2}s {}",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                ));
            }
        }
    }
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.colored_label(LABEL_COLOR, text);
    });
    ui.add_space(2.0);
}

/// ジャンプ行を 1 つ描画する。アイコン + mm:ss + タイトル + (削除可なら × ボタン)。
/// 戻り値はクリック / 削除発生時のみ。
fn draw_jump_row(
    ui: &mut egui::Ui,
    icon: &str,
    pts_secs: f64,
    title: Option<&str>,
    delete_id: Option<i64>,
) -> Option<JumpPanelAction> {
    let mut out: Option<JumpPanelAction> = None;
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        let mm = format_secs(pts_secs);
        let txt = if let Some(t) = title {
            format!("{icon}  {mm}  {t}")
        } else {
            format!("{icon}  {mm}")
        };
        let r = ui.add(
            egui::Button::new(
                egui::RichText::new(txt).color(TEXT_COLOR).size(13.0),
            )
            .frame(false),
        );
        if r.clicked() {
            out = Some(JumpPanelAction::Seek(pts_secs));
        }
        if let Some(id) = delete_id {
            ui.add_space(4.0);
            let x = ui.add(
                egui::Button::new(
                    egui::RichText::new("×")
                        .color(DIM_COLOR)
                        .size(13.0),
                )
                .frame(false),
            );
            if x.clicked() {
                out = Some(JumpPanelAction::DeleteBookmark(id));
            }
        }
    });
    ui.add_space(2.0);
    out
}
