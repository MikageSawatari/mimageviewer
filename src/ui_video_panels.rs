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
/// 左側ジャンプパネルの幅 (Phase 6: サムネ表示のため広げる)。
const VIDEO_JUMP_PANEL_WIDTH: f32 = 320.0;
/// 左側ジャンプパネル内のサムネサイズ (16:9 想定、実際の動画 aspect で再計算)。
const JUMP_THUMB_W: u32 = 120;
const JUMP_THUMB_H: u32 = 68;
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
        // 表示判定: TAB 固定 OR 「entry zone (画面右端 80px 以内)」にカーソルが
        // 入った時点でパネル表示開始、表示中はカーソルが「パネル矩形全体」内にあれば
        // 継続表示。これでパネル上をホイール スクロール / 行クリックしている間に
        // パネルが消える問題を解消 (Phase 7: シークバー / 音量等の操作中に右パネルが
        // 出てくる問題は entry zone の幅で別途抑止する)。
        let panel_w = VIDEO_PANEL_WIDTH.min(full_rect.width() * 0.5);
        let hover_y_max = full_rect.max.y - 48.0; // HUD 44px + 4px 余白
        let entry_x_threshold = full_rect.max.x - 80.0;
        let panel_x_threshold = full_rect.max.x - panel_w;
        let was_visible = self.show_video_metadata_panel_visible;
        let hover_in_zone = ctx.input(|i| {
            i.pointer
                .hover_pos()
                .map(|p| {
                    let x_min = if was_visible {
                        panel_x_threshold
                    } else {
                        entry_x_threshold
                    };
                    p.x > x_min && p.y < hover_y_max
                })
                .unwrap_or(false)
        });
        if !self.show_metadata_panel && !hover_in_zone {
            self.show_video_metadata_panel_visible = false;
            return false;
        }
        self.show_video_metadata_panel_visible = true;

        // VideoPlayer から info() を取り出す。未着のうちはパネル自体は出すがコンテンツが
        // 「読み込み中...」になる (動画 metadata は open 直後に来るので一瞬だけ)。
        let info: Option<VideoInfo> = match self.fs_cache.get(&fs_idx) {
            Some(crate::fs_animation::FsCacheEntry::Video { player, .. }) => {
                player.info().cloned()
            }
            _ => None,
        };

        let panel_top = full_rect.min.y + TOP_BAR_H;
        // Phase 7: HUD (下部 44px) と重ならないようパネルをそこで終わらせる。
        let panel_bottom = full_rect.max.y - 44.0;
        let panel_rect = egui::Rect::from_min_max(
            egui::pos2(full_rect.max.x - panel_w, panel_top),
            egui::pos2(full_rect.max.x, panel_bottom),
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

    let dur_str = crate::ui_helpers::format_hms(info.duration_secs);
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


/// 左パネルでクリックされた行から発生する操作。
#[derive(Debug)]
pub(crate) enum JumpPanelAction {
    /// 指定秒に seek (ブックマーク / チャプター 共通)。
    Seek(f64),
    /// このブックマーク id を削除。
    DeleteBookmark(i64),
    /// 現在位置にブックマークを追加 (= 上部 🔖 ボタン)。
    AddBookmarkHere,
    /// 現在のフレームを動画グリッドサムネに固定 / 解除トグル (= 上部 📌 ボタン)。
    SetPinAtCurrent,
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
        // 表示判定: entry zone (画面左端 80px 以内) で表示開始、表示中は
        // 「パネル矩形全体」内なら継続表示 (パネル上ホイール / 行クリックで消えない)。
        let panel_w = VIDEO_JUMP_PANEL_WIDTH.min(full_rect.width() * 0.4);
        let hover_y_max = full_rect.max.y - 48.0;
        let entry_x_threshold = full_rect.min.x + 80.0;
        let panel_x_threshold = full_rect.min.x + panel_w;
        let was_visible = self.show_video_jump_panel_visible;
        let hover_in_zone = ctx.input(|i| {
            i.pointer
                .hover_pos()
                .map(|p| {
                    let x_max = if was_visible {
                        panel_x_threshold
                    } else {
                        entry_x_threshold
                    };
                    p.x < x_max && p.y < hover_y_max
                })
                .unwrap_or(false)
        });
        if !hover_in_zone {
            self.show_video_jump_panel_visible = false;
            return false;
        }
        self.show_video_jump_panel_visible = true;

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

        // Phase 6.C / 7.G: 各行のサムネを順番にリクエストする。
        // ThumbnailWorker は MAX_ENTRIES=32 の LRU + 最新リクエスト 1 件のみ処理する
        // drain semantics。
        // - フレームごとに 1 行だけ worker に request、他行は cache から読む。
        // - 一度 GPU テクスチャ化 (= self.video_jump_textures に bucket キーで載った)
        //   ら、worker LRU で evicted されても **再 request しない** (Phase 7.G):
        //   = チャプター多数動画 (> 32 件) で worker LRU が thrash して古い行サムネが
        //     何度も再生成 + 再 upload → 「再描画されているような動作」を解消する。
        //     UI は video_jump_textures の TextureHandle を直接読むので、worker の
        //     RGBA キャッシュが残っているかどうかは関係なく安定描画できる。
        let pending_pts: Option<f64> = {
            let pin_pts: Vec<f64> = bookmarks.iter().map(|b| b.pts_secs).collect();
            let chap_pts: Vec<f64> = chapters.iter().map(|c| c.start_secs).collect();
            let all_pts = pin_pts.into_iter().chain(chap_pts.into_iter());
            let mut found: Option<f64> = None;
            if let Some(crate::fs_animation::FsCacheEntry::Video { player, .. }) =
                self.fs_cache.get(&fs_idx)
            {
                for pts in all_pts {
                    let bucket = crate::video::thumbnail::bucket_key(pts);
                    // 既にテクスチャ アップロード済みならスキップ (= worker LRU 状態に
                    // 関わらず行は表示できる)。
                    if self.video_jump_textures.contains_key(&bucket) {
                        continue;
                    }
                    if player.nearest_seek_thumbnail(pts).is_none() {
                        found = Some(pts);
                        break;
                    }
                }
            }
            found
        };
        if let Some(pts) = pending_pts {
            if let Some(crate::fs_animation::FsCacheEntry::Video { player, .. }) =
                self.fs_cache.get(&fs_idx)
            {
                player.request_seek_thumbnail(pts);
            }
        }

        // Phase 6: 動画モードの左パネルは常時表示候補 (= 🔖 ボタンがあるため、
        // 中身が空でもパネル自体は出す)。

        let panel_top = full_rect.min.y + TOP_BAR_H;
        // Phase 7: HUD (下部 44px) と重ならないようパネルをそこで終わらせる。
        let panel_bottom = full_rect.max.y - 44.0;
        let panel_rect = egui::Rect::from_min_max(
            egui::pos2(full_rect.min.x, panel_top),
            egui::pos2(full_rect.min.x + panel_w, panel_bottom),
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
        // 各行を描画。サムネは self.video_jump_textures を介して player の thumb cache から
        // 取得する。借用衝突を避けるため、サムネ取得 + texture upload は内部で完結させる。
        let mut scroll_actions: Vec<JumpPanelAction> = Vec::new();
        let scroll_height = content_rect.height();
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .max_height(scroll_height)
            .show(&mut content_ui, |ui| {
                ui.add_space(6.0);
                // 上部ボタン群: 📌 ピン (= 現フレームを動画グリッドサムネに) と
                // 🔖 ブックマーク追加。テキスト色はパネル暗背景に合わせて
                // 明示的に明色固定 + 視認性確保のため explicit fill を入れる。
                let btn_bg = egui::Color32::from_rgba_unmultiplied(50, 60, 90, 240);
                let btn_bg_hover = egui::Color32::from_rgba_unmultiplied(80, 100, 150, 255);
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    // 📌 ピン (Phase 7.B で実装予定の handler を呼ぶ)
                    let pin_resp = ui.add(
                        egui::Button::new(
                            egui::RichText::new("📌 現フレームをサムネ固定")
                                .color(egui::Color32::WHITE)
                                .size(12.0),
                        )
                        .fill(btn_bg)
                        .stroke(egui::Stroke::new(1.0, btn_bg_hover)),
                    );
                    if pin_resp.clicked() {
                        scroll_actions.push(JumpPanelAction::SetPinAtCurrent);
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    let bm_resp = ui.add(
                        egui::Button::new(
                            egui::RichText::new("🔖 現在位置にブックマーク追加 [B]")
                                .color(egui::Color32::WHITE)
                                .size(12.0),
                        )
                        .fill(btn_bg)
                        .stroke(egui::Stroke::new(1.0, btn_bg_hover)),
                    );
                    if bm_resp.clicked() {
                        scroll_actions.push(JumpPanelAction::AddBookmarkHere);
                    }
                });
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);

                let mut had_section = false;
                if !bookmarks.is_empty() {
                    section_label(ui, "🔖 ブックマーク");
                    for b in bookmarks.iter() {
                        let acts = self.draw_video_jump_row(
                            ui,
                            "🔖",
                            b.pts_secs,
                            b.title.as_deref(),
                            Some(b.id),
                            fs_idx,
                        );
                        scroll_actions.extend(acts);
                    }
                    ui.add_space(8.0);
                    had_section = true;
                }

                if !chapters.is_empty() {
                    section_label(ui, "📑 チャプター");
                    for c in chapters.iter() {
                        let acts = self.draw_video_jump_row(
                            ui,
                            "📑",
                            c.start_secs,
                            c.title.as_deref(),
                            None,
                            fs_idx,
                        );
                        scroll_actions.extend(acts);
                    }
                    ui.add_space(8.0);
                    had_section = true;
                }

                if !had_section {
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
        // 最初のアクションだけ採用 (= 1 フレーム 1 アクション)。
        if let Some(a) = scroll_actions.into_iter().next() {
            action = Some(a);
        }

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
                JumpPanelAction::SetPinAtCurrent => {
                    self.toggle_video_pin_at_current(fs_idx);
                }
            }
        }

        true
    }

    /// Phase 6.C: 1 行分のジャンプ行を描画する (サムネ + 時間 + タイトル + 削除 ×)。
    /// インスタンスメソッド版で、サムネは player の seek thumb cache + 自前の
    /// texture cache 経由で表示する。クリック / 削除アクションは Vec で返す。
    fn draw_video_jump_row(
        &mut self,
        ui: &mut egui::Ui,
        icon: &str,
        pts_secs: f64,
        title: Option<&str>,
        delete_id: Option<i64>,
        fs_idx: usize,
    ) -> Vec<JumpPanelAction> {
        let mut out: Vec<JumpPanelAction> = Vec::new();
        let bucket: i64 = crate::video::thumbnail::bucket_key(pts_secs);

        // 既に video_jump_textures にアップロード済ならそれを使う (Phase 7.G:
        // worker LRU evict 後でも texture が安定して残るので、サムネ「再描画」が消える)。
        let tex_id: Option<egui::TextureId> = if let Some((_, tex)) =
            self.video_jump_textures.get(&bucket)
        {
            Some(tex.id())
        } else {
            // 未アップロードなら worker から最新サムネを取り出して texture を作る。
            let thumb_data = match self.fs_cache.get(&fs_idx) {
                Some(crate::fs_animation::FsCacheEntry::Video { player, .. }) => {
                    player.nearest_seek_thumbnail(pts_secs)
                }
                _ => None,
            };
            if let Some(t) = thumb_data {
                let key = (t.target_secs.to_bits(), t.width, t.height);
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [t.width as usize, t.height as usize],
                    &t.rgba,
                );
                let tex = ui.ctx().load_texture(
                    format!("video_jump_thumb:{bucket}"),
                    img,
                    egui::TextureOptions::LINEAR,
                );
                let id = tex.id();
                self.video_jump_textures.insert(bucket, (key, tex));
                Some(id)
            } else {
                None
            }
        };

        // 1 行 = サムネ列 + 時間/タイトル列 + × 列。サムネ size は固定 120x68。
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            // サムネプレースホルダ rect
            let thumb_size = egui::vec2(JUMP_THUMB_W as f32, JUMP_THUMB_H as f32);
            let (thumb_rect, _r) = ui.allocate_exact_size(thumb_size, egui::Sense::click());
            let painter = ui.painter();
            painter.rect_filled(
                thumb_rect,
                3.0,
                egui::Color32::from_rgba_unmultiplied(35, 35, 40, 255),
            );
            if let Some(tid) = tex_id {
                painter.image(
                    tid,
                    thumb_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                painter.text(
                    thumb_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "...",
                    egui::FontId::proportional(14.0),
                    DIM_COLOR,
                );
            }
            painter.rect_stroke(
                thumb_rect,
                3.0,
                egui::Stroke::new(1.0, egui::Color32::from_gray(80)),
                egui::StrokeKind::Inside,
            );
            // クリック判定 (サムネ + 時刻ラベル両方)
            let click_resp = ui.interact(
                thumb_rect,
                egui::Id::new(("video_jump_thumb_click", bucket, delete_id.unwrap_or(0))),
                egui::Sense::click(),
            );
            if click_resp.clicked() {
                out.push(JumpPanelAction::Seek(pts_secs));
            }
            ui.add_space(6.0);
            // 縦 2 行: 時間 + タイトル
            ui.vertical(|ui| {
                ui.add_space(4.0);
                let time_label = crate::ui_helpers::format_hms(pts_secs);
                let time_resp = ui.add(
                    egui::Button::new(
                        egui::RichText::new(format!("{icon}  {time_label}"))
                            .color(TEXT_COLOR)
                            .size(13.0),
                    )
                    .frame(false),
                );
                if time_resp.clicked() {
                    out.push(JumpPanelAction::Seek(pts_secs));
                }
                if let Some(t) = title {
                    ui.colored_label(
                        DIM_COLOR,
                        egui::RichText::new(t).size(11.0),
                    );
                }
            });
            // 削除ボタン (右端)
            if let Some(id) = delete_id {
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let x_resp = ui.add(
                            egui::Button::new(
                                egui::RichText::new("×")
                                    .color(DIM_COLOR)
                                    .size(14.0),
                            )
                            .frame(false),
                        );
                        if x_resp.clicked() {
                            out.push(JumpPanelAction::DeleteBookmark(id));
                        }
                    },
                );
            }
        });
        ui.add_space(2.0);
        out
    }

    /// 現在のフレームをピン留め (= 動画グリッドサムネに固定) するトグル。
    /// 既存ピンがあれば削除、なければ現在位置を set_pin。`thumb_webp` は今回は空
    /// で書く (= グリッド側は WebP が空なら現状動作 = sidecar / shell に fall-through、
    /// 後続フェーズで本物のフレーム抽出を入れる予定)。
    pub(crate) fn toggle_video_pin_at_current(&mut self, fs_idx: usize) {
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
        let (Some((path, pts)), Some(db)) = (snapshot, self.video_pin_db.as_ref()) else {
            return;
        };
        let already_pinned = db.lookup(&path).is_some();
        if already_pinned {
            if let Err(e) = db.remove(&path) {
                crate::logger::log(format!("video pin remove failed: {e}"));
            } else {
                crate::logger::log("video pin removed".to_string());
            }
        } else {
            if let Err(e) = db.set_pin(&path, pts, &[]) {
                crate::logger::log(format!("video pin set failed: {e}"));
            } else {
                crate::logger::log(format!("video pin set: pts={pts:.2}s"));
            }
        }
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

