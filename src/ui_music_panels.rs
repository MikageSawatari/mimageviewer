//! 音楽ビュー (Inc 5) の右パネル (音楽情報 + ★ レーティング + タグ) と
//! 左パネル (ブックマーク一覧)。
//!
//! - 右パネルのタグ UI は画像メタデータパネルと同一のものを再利用する
//!   (`App::draw_music_tag_section`、`ui_metadata_panel.rs`)。★ は `get_rating` / `set_rating`
//!   を共有する (音声も `is_rating_leaf`)。音楽情報は解析ワーカーの軽量 probe
//!   (`music_probe`) を表示する。
//! - 左パネルのブックマークは動画の `VideoBookmarkDb` を **path キーで共有** する
//!   (docs/music-integration-plan.md D5.1)。フォーマット (parse/format) も動画と同じ
//!   `video_bookmarks_parser` を使う。
//!
//! フルスクリーン内は黒背景ベース統一なので、両パネルとも常にダーク配色で描く。

use std::path::Path;

use crate::app::App;
use crate::fs_animation::FsCacheEntry;
use crate::grid_item::GridItem;
use crate::video::native_presenter::overlay_draw::{
    NativeJumpPanelOptions, draw_native_bookmark_title_editor, draw_native_bulk_bookmark_dialog,
    draw_native_jump_panel_body, draw_overlay_button_bg, draw_overlay_continuous_icon,
    draw_overlay_loop_icon, draw_overlay_pause_icon, draw_overlay_play_icon,
    draw_overlay_replay_icon, draw_overlay_skip_to_marker_icon, draw_overlay_speaker_icon,
    draw_overlay_speed_control, draw_overlay_volume_slider,
};
use crate::video::native_presenter::{
    NativeOverlayCommand, NativeOverlayJumpEntry, NativeOverlayTimelineMarkerKind,
};

/// 左パネル (ブックマーク) の幅。画像補正パネル (`LEFT_PANEL_WIDTH`) と揃える。
pub(crate) const MUSIC_LEFT_PANEL_WIDTH: f32 = 292.0;
/// 右パネル (音楽情報 + タグ) の幅。
pub(crate) const MUSIC_RIGHT_PANEL_WIDTH: f32 = 340.0;
/// 下 HUD の高さ (seek 行 + コントロール行、常時表示、Inc 5 FB で動画寄りに)。
pub(crate) const MUSIC_HUD_HEIGHT: f32 = 62.0;

const PANEL_BG: egui::Color32 = egui::Color32::from_rgba_premultiplied(16, 16, 20, 235);
const PANEL_DIVIDER: egui::Color32 = egui::Color32::from_rgba_premultiplied(255, 255, 255, 40);
const TITLE_BG: egui::Color32 = egui::Color32::from_rgba_premultiplied(28, 28, 36, 240);
const LABEL_COLOR: egui::Color32 = egui::Color32::from_rgb(150, 168, 205);
const VALUE_COLOR: egui::Color32 = egui::Color32::from_rgb(228, 230, 236);
const TITLE_H: f32 = 30.0;

/// 秒を `mm:ss` / `h:mm:ss` に整形する (負値は 0)。
fn format_hms(secs: f64) -> String {
    let total = secs.max(0.0).floor() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// bitrate (bps) を "320 kbps" / "1.4 Mbps" 風に整形する。
fn format_bitrate(bit_rate_bps: i64) -> String {
    if bit_rate_bps <= 0 {
        return "-".to_string();
    }
    if bit_rate_bps >= 1_000_000 {
        format!("{:.1} Mbps", bit_rate_bps as f64 / 1_000_000.0)
    } else {
        format!("{} kbps", (bit_rate_bps as f64 / 1000.0).round() as i64)
    }
}

fn format_sample_rate(sr: u32) -> String {
    if sr == 0 {
        return "-".to_string();
    }
    if sr % 1000 == 0 {
        format!("{} kHz", sr / 1000)
    } else {
        format!("{:.1} kHz", sr as f64 / 1000.0)
    }
}

fn format_channels(ch: u16) -> String {
    match ch {
        0 => "-".to_string(),
        1 => "1 (モノラル)".to_string(),
        2 => "2 (ステレオ)".to_string(),
        n => format!("{n} ch"),
    }
}

/// ラベル + 値の 1 行を描く (値は折り返し可)。
fn info_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(egui::RichText::new(label).color(LABEL_COLOR).size(12.0));
        ui.label(egui::RichText::new(value).color(VALUE_COLOR).size(13.0));
    });
    ui.add_space(2.0);
}

impl App {
    // ───────────────────────── ブックマークのデータ操作 ─────────────────────────

    /// 現在の音声パスのブックマークをキャッシュへ読み込む (path 変化時のみ再取得)。
    /// 動画と同じ `VideoBookmarkDb` を path キーで共有する (D5.1)。
    pub(crate) fn ensure_music_bookmarks_loaded(&mut self, path: &Path) {
        if self.music_bookmarks_loaded_for.as_deref() == Some(path) {
            return;
        }
        self.reload_music_bookmarks(path);
    }

    fn reload_music_bookmarks(&mut self, path: &Path) {
        self.music_bookmarks = self
            .video_bookmark_db
            .as_ref()
            .map(|db| db.list_marker_entries(path))
            .unwrap_or_default();
        self.music_bookmarks_loaded_for = Some(path.to_path_buf());
        // 改名ダイアログ中の項目が消えていたら編集状態を解除する。
        if let Some(edit) = self.music_bookmark_title_edit.as_ref()
            && !self.music_bookmarks.iter().any(|b| b.id == edit.id)
        {
            self.music_bookmark_title_edit = None;
        }
    }

    fn music_player_position(&self, fs_idx: usize) -> Option<f64> {
        match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => Some(player.position().max(0.0)),
            _ => None,
        }
    }

    /// 音楽ビューの全 seek はこの helper に集約する (HUD シークバー / ブックマークジャンプ /
    /// timeline クリック / コマンド翻訳)。seek 後にブックマーク区間ループの `loop_target_secs`
    /// を再計算するため `apply_music_loop_mode` を必ず通す (旧区間の target 残留を防ぐ、Codex P2)。
    pub(crate) fn music_seek_to(&mut self, fs_idx: usize, secs: f64) {
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.seek(secs.max(0.0));
        }
        self.apply_music_loop_mode(fs_idx);
    }

    fn music_audio_path(&self, fs_idx: usize) -> Option<std::path::PathBuf> {
        match self.items.get(fs_idx) {
            Some(GridItem::Audio(p)) => Some(p.clone()),
            _ => None,
        }
    }

    /// 現在の再生位置にブックマークを追加する (B キー)。近接重複 (±1s) は避ける。
    pub(crate) fn add_music_bookmark_at_current(&mut self, fs_idx: usize) {
        let Some(pos) = self.music_player_position(fs_idx) else {
            return;
        };
        self.add_music_bookmark_at(fs_idx, pos);
    }

    /// 指定秒にブックマークを追加する。近接重複 (±1s) は避ける。パネルヘッダの
    /// ブックマークボタン (`AddBookmarkAt { target_secs }` コマンド) からも使う。
    fn add_music_bookmark_at(&mut self, fs_idx: usize, secs: f64) {
        let Some(path) = self.music_audio_path(fs_idx) else {
            return;
        };
        let pos = secs.max(0.0);
        let added = self
            .video_bookmark_db
            .as_ref()
            .and_then(|db| db.add_if_no_duplicate(&path, pos, None, 1.0).ok())
            .flatten();
        self.reload_music_bookmarks(&path);
        match added {
            Some(_) => self.show_feedback_toast(format!("ブックマークを追加: {}", format_hms(pos))),
            None => self.show_feedback_toast("既存のブックマークと近すぎます".to_string()),
        }
    }

    fn delete_music_bookmark(&mut self, fs_idx: usize, id: i64) {
        let Some(path) = self.music_audio_path(fs_idx) else {
            return;
        };
        if let Some(db) = self.video_bookmark_db.as_ref() {
            let _ = db.remove(id);
        }
        if self.music_bookmark_title_edit.as_ref().map(|e| e.id) == Some(id) {
            self.music_bookmark_title_edit = None;
        }
        self.reload_music_bookmarks(&path);
    }

    fn rename_music_bookmark(&mut self, fs_idx: usize, id: i64, title: &str) {
        let Some(path) = self.music_audio_path(fs_idx) else {
            return;
        };
        let trimmed = title.trim();
        let new_title = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        };
        if let Some(db) = self.video_bookmark_db.as_ref() {
            let _ = db.update_title(id, new_title);
        }
        self.reload_music_bookmarks(&path);
    }

    /// 一括ブックマーク登録 (中央モーダルの `BulkAddBookmarks { entries }` コマンドを翻訳)。
    /// 動画と同じ重複判定 (±1s) で追加する。
    fn bulk_add_music_bookmarks(&mut self, fs_idx: usize, entries: Vec<(f64, String)>) {
        let Some(path) = self.music_audio_path(fs_idx) else {
            return;
        };
        if entries.is_empty() {
            self.show_feedback_toast("登録できる行がありません".to_string());
            return;
        }
        let refs: Vec<(f64, Option<&str>)> = entries
            .iter()
            .map(|(secs, title)| (*secs, Some(title.as_str())))
            .collect();
        // DB エラー / DB 未オープンは黙って捨てずに通知する (Codex P3、動画と同流儀)。
        let result = self
            .video_bookmark_db
            .as_mut()
            .map(|db| db.bulk_add_if_no_duplicate(&path, &refs, 1.0));
        match result {
            Some(Ok(s)) => {
                self.reload_music_bookmarks(&path);
                self.show_feedback_toast(format!(
                    "一括登録: {} 件追加 / 重複 {} / エラー {}",
                    s.added, s.skipped_duplicates, s.errors
                ));
            }
            Some(Err(e)) => {
                self.show_feedback_toast(format!("ブックマークの保存に失敗しました: {e}"));
            }
            None => {
                self.show_feedback_toast("ブックマーク DB を開けませんでした".to_string());
            }
        }
    }

    /// ブックマークをクリップボードへエクスポート (`ExportBookmarksToClipboard { seconds_only }`
    /// コマンドを翻訳、動画と同じ `format_chapter_lines`)。
    fn export_music_bookmarks(&mut self, fs_idx: usize, ctx: &egui::Context, seconds_only: bool) {
        let Some(path) = self.music_audio_path(fs_idx) else {
            return;
        };
        let entries: Vec<(f64, Option<String>)> = self
            .video_bookmark_db
            .as_ref()
            .map(|db| db.list_marker_meta(&path))
            .unwrap_or_default();
        if entries.is_empty() {
            self.show_feedback_toast("ブックマークがありません".to_string());
            return;
        }
        let text = crate::video_bookmarks_parser::format_chapter_lines(&entries, seconds_only);
        ctx.copy_text(text);
        self.show_feedback_toast(format!(
            "{} 件をクリップボードへコピーしました",
            entries.len()
        ));
    }

    /// この音声のブックマークを全削除 (`ClearAllBookmarksForCurrent` コマンドを翻訳)。
    fn clear_all_music_bookmarks(&mut self, fs_idx: usize) {
        let Some(path) = self.music_audio_path(fs_idx) else {
            return;
        };
        let result = self
            .video_bookmark_db
            .as_ref()
            .map(|db| db.clear_for(&path));
        self.music_bookmark_title_edit = None;
        self.reload_music_bookmarks(&path);
        match result {
            Some(Ok(())) => {
                self.show_feedback_toast("ブックマークをすべて削除しました".to_string())
            }
            Some(Err(e)) => self.show_feedback_toast(format!("削除に失敗しました: {e}")),
            None => self.show_feedback_toast("ブックマーク DB を開けませんでした".to_string()),
        }
    }

    // ───────────────────────── 右パネル (音楽情報 + ★ + タグ) ─────────────────────────

    /// 音楽ビュー右パネルを描く。音楽情報 (probe) + ★ レーティング + タグ。
    pub(crate) fn draw_fs_music_right_panel(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        panel_rect: egui::Rect,
        fs_idx: usize,
    ) {
        // 背景 + 左端区切り線。
        ui.painter().rect_filled(panel_rect, 0.0, PANEL_BG);
        ui.painter().line_segment(
            [panel_rect.left_top(), panel_rect.left_bottom()],
            egui::Stroke::new(1.0, PANEL_DIVIDER),
        );
        // 背面のタイムライン/スペクトラムへクリック/ドラッグを漏らさない。
        let _ = ui.interact(
            panel_rect,
            ui.id().with(("music_right_bg", fs_idx)),
            egui::Sense::click_and_drag(),
        );

        // タイトルバー。
        let title_rect =
            egui::Rect::from_min_size(panel_rect.min, egui::vec2(panel_rect.width(), TITLE_H));
        ui.painter().rect_filled(title_rect, 0.0, TITLE_BG);
        ui.painter().text(
            title_rect.left_center() + egui::vec2(10.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "音楽情報",
            egui::FontId::proportional(13.0),
            egui::Color32::from_gray(205),
        );

        let content_rect = egui::Rect::from_min_max(
            egui::pos2(panel_rect.left(), title_rect.bottom()),
            panel_rect.max,
        );
        let inner = content_rect.shrink2(egui::vec2(12.0, 8.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        child.set_clip_rect(content_rect);
        *child.visuals_mut() = egui::Visuals::dark();

        let probe = self.music_probe.clone();
        // probe がまだ届いていない理由が「解析ワーカーが動作中」か「終了したが probe 失敗」かで
        // メッセージを変える (Codex P3: probe 失敗時に「取得しています…」で固着しないように)。
        let still_probing = self.music_analysis_pending.is_some();
        let name = self
            .items
            .get(fs_idx)
            .map(|it| it.name().to_string())
            .unwrap_or_default();
        let stars = self.get_rating(fs_idx);
        let mut set_rating: Option<u8> = None;

        egui::ScrollArea::vertical()
            .id_salt(("music_right_scroll", fs_idx))
            .auto_shrink([false, false])
            .show(&mut child, |ui| {
                ui.set_width(ui.available_width());

                // 統一順序 (画像/動画/音声共通): ★ → タグ → 内容 (★ は固定高で先頭、
                // 可変高のタグを中間に置く。ユーザー確定 2026-07-02)。
                // ── ★ レーティング ──
                set_rating = crate::ui_helpers::draw_rating_stars(ui, stars);

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // ── タグ (画像パネルと同一 UI を再利用) ──
                self.draw_music_tag_section(ui, ctx);

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // ── 内容 (音楽情報) ──
                ui.label(
                    egui::RichText::new(&name)
                        .color(VALUE_COLOR)
                        .size(14.0)
                        .strong(),
                );
                ui.add_space(6.0);
                if let Some(p) = probe.as_ref() {
                    if !p.format_name.is_empty() {
                        info_row(ui, "形式", &p.format_name);
                    }
                    if !p.codec_name.is_empty() {
                        info_row(ui, "コーデック", &p.codec_name);
                    }
                    if p.duration_secs > 0.0 {
                        info_row(ui, "長さ", &format_hms(p.duration_secs));
                    }
                    info_row(ui, "サンプルレート", &format_sample_rate(p.sample_rate));
                    info_row(ui, "チャンネル", &format_channels(p.channels));
                    if p.bit_rate_bps > 0 {
                        info_row(ui, "ビットレート", &format_bitrate(p.bit_rate_bps));
                    }
                    if !p.tags.is_empty() {
                        ui.add_space(4.0);
                        for (label, value) in &p.tags {
                            info_row(ui, label, value);
                        }
                    }
                } else {
                    let msg = if still_probing {
                        "情報を取得しています…"
                    } else {
                        "情報を取得できませんでした"
                    };
                    ui.label(egui::RichText::new(msg).color(LABEL_COLOR).size(12.0));
                }
            });

        if let Some(new_stars) = set_rating {
            // draw_rating_stars が「同★再クリック=0」を解決済み。
            self.set_rating(fs_idx, new_stars);
        }
    }

    // ───────────────────────── 左パネル (ブックマーク一覧) ─────────────────────────

    /// 音楽ビューのブックマーク UI (Inc 5c-A、動画のジャンプ/ブックマークパネルを共有)。
    ///
    /// - 左端ホバーで出す一覧パネル本体 (`show_panel`) は `draw_native_jump_panel_body` を
    ///   音声オプション (ピン/チャプター/サムネなし・種別見出しなし) で呼ぶ。
    /// - 改名ダイアログ / 一括登録ダイアログは動画と同一の中央モーダル
    ///   (`draw_native_bookmark_title_editor` / `draw_native_bulk_bookmark_dialog`) を使う。
    ///   IME・貼り付けも動画実装に揃う (Inc 5 FB のインポート欄 IME 不具合を解消)。
    /// - パネル本体・ダイアログが発行する `NativeOverlayCommand` を music の実操作へ翻訳する。
    ///
    /// `panel_rect` = 左端ホバー領域。`full_rect` = 音楽ビュー全域 (中央モーダルの中心決めと、
    /// 背後 timeline/パネルへのクリック漏れを防ぐバックドロップ用)。改名/一括ダイアログは
    /// ホバーに依らず開いている間常に描画する。
    pub(crate) fn draw_music_bookmark_ui(
        &mut self,
        ctx: &egui::Context,
        panel_rect: egui::Rect,
        full_rect: egui::Rect,
        fs_idx: usize,
        show_panel: bool,
    ) {
        let Some(path) = self.music_audio_path(fs_idx) else {
            return;
        };
        self.ensure_music_bookmarks_loaded(&path);

        let position_secs = self.music_player_position(fs_idx).unwrap_or(0.0);
        // 一覧 → 共有 body 用のジャンプエントリ (全て Bookmark 種別、サムネなし)。
        let entries: Vec<NativeOverlayJumpEntry> = self
            .music_bookmarks
            .iter()
            .map(|b| NativeOverlayJumpEntry {
                pts_secs: b.pts_secs,
                kind: NativeOverlayTimelineMarkerKind::Bookmark,
                title: b.title.clone(),
                bookmark_id: Some(b.id),
                thumbnail: None,
            })
            .collect();

        // 借用衝突を避けるためダイアログ state を self から取り出し、末尾で書き戻す。
        let mut title_edit = self.music_bookmark_title_edit.take();
        let mut bulk = self.music_bulk_bookmark_dialog.take();
        let mut commands: Vec<NativeOverlayCommand> = Vec::new();
        // 音声はサムネを持たないので空の texture マップを渡す (show_thumbnails=false で未参照)。
        let empty_tex: std::collections::HashMap<usize, egui::TextureId> =
            std::collections::HashMap::new();

        // ── 左端ホバー一覧パネル本体 ──
        if show_panel {
            let opts = NativeJumpPanelOptions {
                title: "ブックマーク",
                empty_text: "ブックマークはありません",
                show_pin_button: false,
                show_bulk_button: true,
                show_pins: false,
                show_chapters: false,
                show_section_headers: false,
                show_thumbnails: false,
            };
            egui::Area::new(egui::Id::new(("music_jump_panel", fs_idx)))
                .order(egui::Order::Foreground)
                .fixed_pos(panel_rect.min)
                .show(ctx, |ui| {
                    ui.set_min_size(panel_rect.size());
                    // ScrollArea スクロールバー等の ambient テーマ依存要素を、動画 overlay
                    // (既定ダーク ctx) と揃えてダーク描画する。メイン ctx が Light テーマでも
                    // 暗いパネル上に明色スクロールバーが出ないようにする。
                    *ui.visuals_mut() = egui::Visuals::dark();
                    draw_native_jump_panel_body(
                        ui,
                        panel_rect,
                        &opts,
                        position_secs,
                        &entries,
                        &empty_tex,
                        None,
                        &mut title_edit,
                        &mut bulk,
                        &mut commands,
                    );
                });
        }

        // ── 中央モーダル (改名 / 一括登録) は開いている間常に描く ──
        // 背後 timeline / パネルへクリックが漏れないよう半透明バックドロップで吸収する。
        // バックドロップは `Order::Middle` (base の timeline/HUD より上、ダイアログの
        // `Order::Foreground` より下) に置き、ダイアログのクリックを塞がないようにする。
        let modal_open = title_edit.is_some() || bulk.is_some();
        if modal_open {
            egui::Area::new(egui::Id::new(("music_modal_backdrop", fs_idx)))
                .order(egui::Order::Middle)
                .fixed_pos(full_rect.min)
                .show(ctx, |ui| {
                    ui.painter().rect_filled(
                        full_rect,
                        0.0,
                        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 120),
                    );
                    let _ = ui.interact(
                        full_rect,
                        egui::Id::new(("music_modal_backdrop_sink", fs_idx)),
                        egui::Sense::click_and_drag(),
                    );
                });
        }
        if title_edit.is_some() {
            draw_native_bookmark_title_editor(
                ctx,
                full_rect.width(),
                full_rect.height(),
                &mut title_edit,
                &mut commands,
            );
        }
        if bulk.is_some() {
            draw_native_bulk_bookmark_dialog(
                ctx,
                full_rect.width(),
                full_rect.height(),
                "音声",
                &mut bulk,
                &mut commands,
            );
        }

        // 書き戻し。
        self.music_bookmark_title_edit = title_edit;
        self.music_bulk_bookmark_dialog = bulk;

        // ── 発行コマンドを music の実操作へ翻訳する ──
        for cmd in commands {
            match cmd {
                NativeOverlayCommand::Seek { target_secs } => {
                    self.music_seek_to(fs_idx, target_secs)
                }
                NativeOverlayCommand::AddBookmarkAt { target_secs } => {
                    self.add_music_bookmark_at(fs_idx, target_secs)
                }
                NativeOverlayCommand::DeleteBookmark { id } => {
                    self.delete_music_bookmark(fs_idx, id)
                }
                NativeOverlayCommand::SetBookmarkTitle { id, title } => {
                    self.rename_music_bookmark(fs_idx, id, &title)
                }
                NativeOverlayCommand::BulkAddBookmarks { entries } => {
                    self.bulk_add_music_bookmarks(fs_idx, entries)
                }
                NativeOverlayCommand::ExportBookmarksToClipboard { seconds_only } => {
                    self.export_music_bookmarks(fs_idx, ctx, seconds_only)
                }
                NativeOverlayCommand::ClearAllBookmarksForCurrent => {
                    self.clear_all_music_bookmarks(fs_idx)
                }
                // ピン留めや動画専用コマンドは音声パネルでは発行されない。
                _ => {}
            }
        }
    }

    /// 音楽ビューでブックマーク改名 / 一括登録の中央モーダルが開いているか
    /// (キー・シーク入力を抑止するモーダル判定用)。
    pub(crate) fn music_bookmark_modal_open(&self) -> bool {
        self.music_bookmark_title_edit.is_some() || self.music_bulk_bookmark_dialog.is_some()
    }

    // ───────────────────────── 下 HUD (seek 行 + コントロール行) ─────────────────────────

    /// 音楽ビュー下 HUD (Inc 5 FB): seek 行 + コントロール行 (動画のレイアウトに寄せる)。
    /// 頭出し / 再生・一時停止 / 前後ブックマークジャンプ / ループ / 位置・長さ / 再生速度 /
    /// ミュート / 音量スライダー + シークバー上のブックマークマーカー。常時表示。
    ///
    /// (音量ノーマライズは動画のようなスキャン UI が必要なため、この HUD には載せていない。
    /// 音量正規化は開いた時点で `audio_normalize_db` キャッシュ値が適用される。)
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_music_bottom_hud(
        &mut self,
        ui: &mut egui::Ui,
        hud_rect: egui::Rect,
        fs_idx: usize,
        pos: f64,
        dur: f64,
        playing: bool,
        dark: bool,
        // 改名 / 一括登録の中央モーダル表示中は false。HUD の見た目は描くが、操作 (seek /
        // play / volume / bookmark ジャンプ) は適用しない。半透明バックドロップも入力を吸収
        // するが、モーダル仕様として HUD 側でも明示的に止める (Codex 5c-A P2、多層防御)。
        interactive: bool,
    ) {
        // 現在状態を先に読む (player 借用を短く保つ)。
        let (cur_vol, muted) = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => (player.volume(), player.is_muted()),
            _ => (1.0, false),
        };
        let speed = self.video_playback_speed;
        // ループ / 連続再生モードは動画と共有 (video_loop_mode / video_continuous_mode)。
        // 音声はチャプター無しなので effective は Off/Full/Bookmark のみ。
        let continuous_mode = self.video_continuous_mode;
        // マーカー / ジャンプ用に pts をスナップショット (self.music_bookmarks の借用回避)。
        let marker_secs: Vec<f64> = self.music_bookmarks.iter().map(|b| b.pts_secs).collect();
        let has_bm_now = self.music_bookmarks_loaded_for.is_some() && !marker_secs.is_empty();
        let loop_eff =
            crate::settings::effective_loop_mode(self.settings.video_loop_mode, false, has_bm_now);

        let accent = egui::Color32::from_rgb(90, 150, 220);
        let fg = egui::Color32::from_gray(220);
        let dim = egui::Color32::from_gray(150);
        let painter = ui.painter_at(hud_rect);
        painter.rect_filled(
            hud_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, if dark { 150 } else { 60 }),
        );

        let seek_row_h = 22.0;
        let controls_cy = (hud_rect.top() + seek_row_h + hud_rect.bottom()) * 0.5;

        // 収集する操作 (描画中は self を可変借用しないため、末尾でまとめて適用)。
        let mut seek_to: Option<f64> = None;
        let mut toggle_play = false;
        let mut seek_start = false;
        let mut cycle_loop = false;
        let mut cycle_continuous = false;
        let mut toggle_mute = false;
        let mut set_vol: Option<f64> = None;
        // `set_speed` は速度ボタン描画時に `draw_overlay_speed_control` の返り値で一度だけ
        // 束縛する (他フラグと違い条件付き更新ではないため、代入時点で宣言する)。

        // ── seek 行 (バー + ブックマークマーカー + クリック/ドラッグ seek) ──
        let bar_margin = 16.0;
        let bar_h = 6.0;
        let bar_cy = hud_rect.top() + seek_row_h * 0.5;
        let bar_rect = egui::Rect::from_min_max(
            egui::pos2(hud_rect.left() + bar_margin, bar_cy - bar_h * 0.5),
            egui::pos2(hud_rect.right() - bar_margin, bar_cy + bar_h * 0.5),
        );
        painter.rect_filled(bar_rect, bar_h * 0.5, egui::Color32::from_gray(70));
        let frac = if dur > 0.0 {
            (pos / dur).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        if frac > 0.0 {
            let filled = egui::Rect::from_min_max(
                bar_rect.min,
                egui::pos2(bar_rect.left() + bar_rect.width() * frac, bar_rect.bottom()),
            );
            painter.rect_filled(filled, bar_h * 0.5, accent);
        }
        if dur > 0.0 {
            let marker_color = egui::Color32::from_rgb(255, 220, 82);
            for &s in &marker_secs {
                let f = (s / dur).clamp(0.0, 1.0) as f32;
                let mx = bar_rect.left() + bar_rect.width() * f;
                painter.line_segment(
                    [
                        egui::pos2(mx, bar_rect.top() - 4.0),
                        egui::pos2(mx, bar_rect.bottom() + 4.0),
                    ],
                    egui::Stroke::new(2.0, marker_color),
                );
            }
        }
        let seek_resp = ui.interact(
            bar_rect.expand2(egui::vec2(0.0, 7.0)),
            ui.id().with(("music_hud_seek", fs_idx)),
            egui::Sense::click_and_drag(),
        );
        if (seek_resp.clicked() || seek_resp.dragged())
            && dur > 0.0
            && let Some(p) = seek_resp.interact_pointer_pos()
        {
            let f = ((p.x - bar_rect.left()) / bar_rect.width()).clamp(0.0, 1.0) as f64;
            seek_to = Some(f * dur);
        }

        // ── コントロール行: 左クラスタ (頭出し / 再生 / 前後ブックマーク / ループ) ──
        let bsz = 26.0;
        let mut x = hud_rect.left() + 14.0;
        let alloc = |x: &mut f32, w: f32| -> egui::Rect {
            let r = egui::Rect::from_min_size(
                egui::pos2(*x, controls_cy - bsz * 0.5),
                egui::vec2(w, bsz),
            );
            *x += w + 6.0;
            r
        };
        // ボタン描画は動画 HUD と共有の primitive を使う (Inc 5c-B3):
        // 背景 = `draw_overlay_button_bg` (hover / active blue)、アイコン = 各
        // `draw_overlay_*_icon`。これで頭出し / 再生 / 前後マーカー / ループの見た目が
        // 動画 HUD と揃う (頭出し = 動画 replay ↺、前後 = |◀ / ▶| skip-to-marker)。
        let markers_present = !marker_secs.is_empty();

        // 頭出し (動画 replay = 頭出し + 即再生と同義)
        let r = alloc(&mut x, bsz);
        let resp = ui
            .interact(
                r,
                ui.id().with(("music_hud_start", fs_idx)),
                egui::Sense::click(),
            )
            .on_hover_text("頭出し");
        draw_overlay_button_bg(&painter, r, resp.hovered(), false);
        draw_overlay_replay_icon(&painter, r.center(), bsz * 0.36);
        if resp.clicked() {
            seek_start = true;
        }

        // 再生 / 一時停止
        let r = alloc(&mut x, bsz);
        let resp = ui
            .interact(
                r,
                ui.id().with(("music_hud_play", fs_idx)),
                egui::Sense::click(),
            )
            .on_hover_text(if playing { "一時停止" } else { "再生" });
        draw_overlay_button_bg(&painter, r, resp.hovered(), false);
        if playing {
            draw_overlay_pause_icon(&painter, r.center(), bsz * 0.30);
        } else {
            draw_overlay_play_icon(&painter, r.center(), bsz * 0.38);
        }
        if resp.clicked() {
            toggle_play = true;
        }

        // 前ブックマーク (|◀)
        let r = alloc(&mut x, bsz);
        let resp = ui
            .interact(
                r,
                ui.id().with(("music_hud_prevbm", fs_idx)),
                egui::Sense::click(),
            )
            .on_hover_text("前のブックマーク");
        draw_overlay_button_bg(&painter, r, resp.hovered(), false);
        draw_overlay_skip_to_marker_icon(&painter, r, -1, markers_present);
        if resp.clicked() {
            if let Some(&t) = marker_secs.iter().rev().find(|&&s| s < pos - 0.3) {
                seek_to = Some(t);
            } else if markers_present {
                seek_to = Some(0.0);
            }
        }

        // 次ブックマーク (▶|)
        let r = alloc(&mut x, bsz);
        let resp = ui
            .interact(
                r,
                ui.id().with(("music_hud_nextbm", fs_idx)),
                egui::Sense::click(),
            )
            .on_hover_text("次のブックマーク");
        draw_overlay_button_bg(&painter, r, resp.hovered(), false);
        draw_overlay_skip_to_marker_icon(&painter, r, 1, markers_present);
        if resp.clicked()
            && let Some(&t) = marker_secs.iter().find(|&&s| s > pos + 0.3)
        {
            seek_to = Some(t);
        }

        // ループ (Off → 全体 → ブックマーク間 → Off で循環、動画 L キーと共有)。
        // 連続再生中は動画同様に無効表示 (クリックは no-op + トースト)。
        let loop_active = !matches!(loop_eff, crate::settings::VideoLoopMode::Off);
        let loop_color = if continuous_mode.is_enabled() {
            egui::Color32::from_gray(90)
        } else {
            match loop_eff {
                crate::settings::VideoLoopMode::Off => dim,
                crate::settings::VideoLoopMode::Bookmark => {
                    egui::Color32::from_rgb(255, 220, 82) // ブックマークマーカーと同色
                }
                _ => egui::Color32::WHITE, // Full (Chapter は音声で Full 降格)
            }
        };
        let loop_tooltip = if continuous_mode.is_enabled() {
            "ループ (連続再生中は無効)"
        } else {
            match loop_eff {
                crate::settings::VideoLoopMode::Off => "ループしない",
                crate::settings::VideoLoopMode::Full => "全体ループ",
                crate::settings::VideoLoopMode::Bookmark => "ブックマークループ",
                crate::settings::VideoLoopMode::Chapter => "全体ループ",
            }
        };
        let r = alloc(&mut x, bsz);
        let resp = ui
            .interact(
                r,
                ui.id().with(("music_hud_loop", fs_idx)),
                egui::Sense::click(),
            )
            .on_hover_text(loop_tooltip);
        draw_overlay_button_bg(
            &painter,
            r,
            resp.hovered(),
            loop_active && !continuous_mode.is_enabled(),
        );
        draw_overlay_loop_icon(&painter, r.center(), bsz * 0.36, loop_color);
        if resp.clicked() {
            cycle_loop = true;
        }

        // 連続再生 (Off → 連続 → 連続+ループ で循環、動画と共有)。
        let cont_tooltip = match continuous_mode {
            crate::video::VideoContinuousMode::Off => "連続再生: OFF",
            crate::video::VideoContinuousMode::Continuous => "連続再生",
            crate::video::VideoContinuousMode::ContinuousLoop => "連続再生 + ループ",
        };
        let r = alloc(&mut x, bsz);
        let resp = ui
            .interact(
                r,
                ui.id().with(("music_hud_continuous", fs_idx)),
                egui::Sense::click(),
            )
            .on_hover_text(cont_tooltip);
        draw_overlay_button_bg(&painter, r, resp.hovered(), continuous_mode.is_enabled());
        draw_overlay_continuous_icon(&painter, r, continuous_mode);
        if resp.clicked() {
            cycle_continuous = true;
        }

        // ── コントロール行: 右クラスタ (右寄せ: dB ラベル / 音量 / ミュート / 速度 / 時間) ──
        let mut rx = hud_rect.right() - 14.0;
        // 現在音量の dB 表示ラベル (最右、動画 HUD の「スライダーの右」配置に合わせる)。
        // ミュート状態に関わらず実効音量を dB で示す。共有の
        // `format_video_volume_db_compact` を使い動画と表記を揃える (-∞dB / 0.0dB / +3.0dB)。
        let vol_label_w = 52.0;
        let vol_db_label = crate::video::native_presenter::format_video_volume_db_compact(cur_vol);
        let vol_label_color = if cur_vol > 1.0 {
            egui::Color32::from_rgb(255, 210, 80)
        } else {
            fg
        };
        painter.text(
            egui::pos2(rx, controls_cy),
            egui::Align2::RIGHT_CENTER,
            vol_db_label,
            egui::FontId::proportional(13.0),
            vol_label_color,
        );
        rx -= vol_label_w + 8.0;
        // 音量 dB フェーダーは動画/音楽共有の `draw_overlay_volume_slider` を使う
        // (Inc 5c-B1)。トラック + fill (0dB 未満グレー / 0dB 超ブースト黄) + dB 目盛り +
        // クリック/ドラッグ/ダブルクリック (0dB リセット) が動画 HUD と完全に揃う。
        // フェーダーマッピングは `video_volume_*_fader_pos` (-80..+18dB) を共有し、独自 dB
        // マップだと高ブースト/微小音量がつぶれる問題 (Codex P3) も解消済み。永続化は
        // ドラッグ確定 / クリック / ダブルクリック時のみ (`persist=true`、毎フレーム save 回避)。
        let vol_w = 120.0;
        let vol_rect = egui::Rect::from_min_max(
            egui::pos2(rx - vol_w, controls_cy - 4.0),
            egui::pos2(rx, controls_cy + 4.0),
        );
        rx -= vol_w + 10.0;
        // 音量ツールチップに Shift+↑↓ (`VideoVolumeUp/Down`) のショートカットを併記する
        // (動画 HUD と揃える、実機 FB 2026-07-02)。`&mut self` を握る `vol_target` より前に
        // owned String を作っておき、借用衝突を避ける。
        let vol_shortcut = {
            let up = self
                .keymap
                .first_chord_label(crate::keymap::KeyAction::VideoVolumeUp);
            let down = self
                .keymap
                .first_chord_label(crate::keymap::KeyAction::VideoVolumeDown);
            match (up, down) {
                (Some(u), Some(d)) => format!(" [{u} / {d}]"),
                (Some(s), None) | (None, Some(s)) => format!(" [{s}]"),
                (None, None) => String::new(),
            }
        };
        let vol_tooltip = format!("音量 (ダブルクリックで 0dB){vol_shortcut}");
        let mut vol_persist = false;
        // モーダル表示中 (`!interactive`) は HUD 操作を一切自 state に反映しない不変条件を
        // 守るため、ドラッグ確定用の frame 跨ぎ state をダミーに逃がす (Codex 5c-B1 P3)。
        // set_vol / vol_persist は末尾の early-return で捨てられるが、`last_volume_target` は
        // 自 state なので明示的にガードしないと汚れる (5c-A の多層防御と同趣旨)。
        let mut dummy_vol_target = None;
        let vol_target = if interactive {
            &mut self.music_hud_last_volume_target
        } else {
            &mut dummy_vol_target
        };
        if let Some((v, persist)) = draw_overlay_volume_slider(
            ui,
            &painter,
            vol_rect,
            cur_vol,
            ui.id().with(("music_hud_vol", fs_idx)),
            Some(vol_tooltip),
            vol_target,
        ) {
            set_vol = Some(crate::settings::clamp_video_volume(v));
            vol_persist = persist;
        }
        // ミュート
        let mute_r = egui::Rect::from_min_size(
            egui::pos2(rx - bsz, controls_cy - bsz * 0.5),
            egui::vec2(bsz, bsz),
        );
        rx -= bsz + 8.0;
        let mresp = ui
            .interact(
                mute_r,
                ui.id().with(("music_hud_mute", fs_idx)),
                egui::Sense::click(),
            )
            .on_hover_text(if muted {
                "ミュート解除"
            } else {
                "ミュート"
            });
        draw_overlay_button_bg(&painter, mute_r, mresp.hovered(), muted);
        draw_overlay_speaker_icon(&painter, mute_r.center(), bsz * 0.46, muted);
        if mresp.clicked() {
            toggle_mute = true;
        }
        // 再生速度: 動画/音楽共有の speed ボタン + プリセット popup (Inc 5c-B2)。
        // 左クリックで popup をトグル、右クリック / ダブルクリックで x1。動画と同じ 11
        // プリセット (`PLAYBACK_SPEED_CHOICES`) / ラベル形式 (`format_playback_speed`) に揃う。
        let spd_w = 46.0;
        let spd_r = egui::Rect::from_min_size(
            egui::pos2(rx - spd_w, controls_cy - bsz * 0.5),
            egui::vec2(spd_w, bsz),
        );
        rx -= spd_w + 8.0;
        // モーダル中 (`!interactive`) は popup 開閉 (自 state) を汚さないようダミーに逃がす
        // (B1 の音量と同趣旨、多層防御)。popup rect は音楽では使わないので sink に捨てる。
        let mut dummy_speed_popup = false;
        let speed_popup_open = if interactive {
            &mut self.music_speed_popup_open
        } else {
            &mut dummy_speed_popup
        };
        let mut speed_popup_rect_sink = None;
        let set_speed = draw_overlay_speed_control(
            ui.ctx(),
            ui,
            &painter,
            spd_r,
            spd_r.center().y,
            speed,
            ui.id().with(("music_hud_speed", fs_idx)),
            ui.id().with(("music_hud_speed_popup", fs_idx)),
            hud_rect.left(),
            hud_rect.width(),
            hud_rect.top(),
            speed_popup_open,
            &mut speed_popup_rect_sink,
        );
        // 時間 (速度ボタンの左に右寄せ)
        painter.text(
            egui::pos2(rx, controls_cy),
            egui::Align2::RIGHT_CENTER,
            format!("{} / {}", format_hms(pos), format_hms(dur)),
            egui::FontId::proportional(13.0),
            fg,
        );

        // ── 操作を適用 (self / player の可変借用を分離) ──
        // モーダル表示中は上で描いたホバー/レスポンスを無視し、一切適用しない。
        if !interactive {
            return;
        }
        let ctx = ui.ctx().clone();
        if seek_start {
            seek_to = Some(0.0);
        }
        if let Some(s) = set_speed {
            self.video_playback_speed = s;
        }
        if toggle_mute {
            self.video_session_muted = !muted;
        }
        if let Some(v) = set_vol {
            self.settings.video_volume = v;
        }
        // 確定時 (drag_stopped / click) のみ save する (毎フレーム save を回避)。release frame は
        // set_vol=None だが settings.video_volume は直前 frame までに更新済みなので、ここで
        // save すればドラッグ確定分も永続化される (Codex P3)。
        if vol_persist {
            self.settings.save();
        }
        // player 直操作 (seek はループ target 再計算のため music_seek_to に集約するので除く)。
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            if seek_start {
                player.set_playing(true);
            }
            if toggle_play {
                player.toggle_play();
            }
            if let Some(s) = set_speed {
                player.set_playback_speed(s);
            }
            if toggle_mute {
                player.set_muted(!muted);
            }
            if let Some(v) = set_vol {
                player.set_volume(v);
            }
        }
        // seek はブックマーク区間ループの target 再計算を伴うので専用 helper に寄せる
        // (全 seek 経路を単一化、Codex 設計 P2)。
        if let Some(s) = seek_to {
            self.music_seek_to(fs_idx, s);
        }
        // ループ / 連続再生の切替 (共有 video_loop_mode / video_continuous_mode)。
        if cycle_loop {
            self.cycle_music_loop_mode(&ctx, fs_idx);
        }
        if cycle_continuous {
            self.cycle_music_continuous_mode(&ctx, fs_idx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hms_formats() {
        assert_eq!(format_hms(0.0), "0:00");
        assert_eq!(format_hms(13.7), "0:13");
        assert_eq!(format_hms(62.0), "1:02");
        assert_eq!(format_hms(3608.0), "1:00:08");
        assert_eq!(format_hms(-5.0), "0:00");
    }

    #[test]
    fn bitrate_formats() {
        assert_eq!(format_bitrate(0), "-");
        assert_eq!(format_bitrate(320_000), "320 kbps");
        assert_eq!(format_bitrate(1_411_000), "1.4 Mbps");
    }

    #[test]
    fn sample_rate_formats() {
        assert_eq!(format_sample_rate(0), "-");
        assert_eq!(format_sample_rate(48_000), "48 kHz");
        assert_eq!(format_sample_rate(44_100), "44.1 kHz");
    }

    #[test]
    fn channel_labels() {
        assert_eq!(format_channels(1), "1 (モノラル)");
        assert_eq!(format_channels(2), "2 (ステレオ)");
        assert_eq!(format_channels(6), "6 ch");
        assert_eq!(format_channels(0), "-");
    }
}
