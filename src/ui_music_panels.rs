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

/// 左パネル (ブックマーク) の幅。画像補正パネル (`LEFT_PANEL_WIDTH`) と揃える。
pub(crate) const MUSIC_LEFT_PANEL_WIDTH: f32 = 292.0;
/// 右パネル (音楽情報 + タグ) の幅。
pub(crate) const MUSIC_RIGHT_PANEL_WIDTH: f32 = 340.0;
/// 下 HUD の高さ (seek 行 + コントロール行、常時表示、Inc 5 FB で動画寄りに)。
pub(crate) const MUSIC_HUD_HEIGHT: f32 = 62.0;
/// 再生速度プリセット (コントロール行の速度ボタンで巡回)。
const MUSIC_SPEED_PRESETS: [f64; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];

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

/// 左パネル (ブックマーク) の 1 フレームで発生する操作。リスト描画中に self を可変借用
/// できないので、収集してからクロージャ外で適用する。
enum BmAction {
    Seek(f64),
    Delete(i64),
    StartRename(i64, String),
    CommitRename,
    Add,
    Import,
    Export,
    ToggleImport,
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
        // 改名中の項目が消えていたら編集状態を解除する。
        if let Some((id, _)) = self.music_bookmark_rename.as_ref()
            && !self.music_bookmarks.iter().any(|b| b.id == *id)
        {
            self.music_bookmark_rename = None;
        }
    }

    fn music_player_position(&self, fs_idx: usize) -> Option<f64> {
        match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => Some(player.position().max(0.0)),
            _ => None,
        }
    }

    fn music_seek_to(&self, fs_idx: usize, secs: f64) {
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.seek(secs.max(0.0));
        }
    }

    fn music_audio_path(&self, fs_idx: usize) -> Option<std::path::PathBuf> {
        match self.items.get(fs_idx) {
            Some(GridItem::Audio(p)) => Some(p.clone()),
            _ => None,
        }
    }

    /// 現在の再生位置にブックマークを追加する (B キー / パネルボタン)。近接重複 (±1s) は避ける。
    pub(crate) fn add_music_bookmark_at_current(&mut self, fs_idx: usize) {
        let Some(path) = self.music_audio_path(fs_idx) else {
            return;
        };
        let Some(pos) = self.music_player_position(fs_idx) else {
            return;
        };
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
        if self.music_bookmark_rename.as_ref().map(|(rid, _)| *rid) == Some(id) {
            self.music_bookmark_rename = None;
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

    /// インポート欄のテキストを一括登録する (動画と同じ `parse_chapter_text` + 一括 add)。
    fn import_music_bookmarks(&mut self, fs_idx: usize) {
        let Some(path) = self.music_audio_path(fs_idx) else {
            return;
        };
        let text = self.music_bookmark_import_text.clone();
        let (entries, _errors) = crate::video_bookmarks_parser::parse_chapter_text(&text);
        if entries.is_empty() {
            self.show_feedback_toast("登録できる行がありません".to_string());
            return;
        }
        let refs: Vec<(f64, Option<&str>)> = entries
            .iter()
            .map(|e| (e.pts_secs, Some(e.title.as_str())))
            .collect();
        // 成功したときだけ貼り付けテキストをクリアしてインポート欄を閉じる。DB エラー / DB
        // 未オープンでは黙って捨てず、テキストを残してエラーを通知する (Codex P3)。
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
                self.music_bookmark_import_text.clear();
                self.music_bookmark_import_open = false;
            }
            Some(Err(e)) => {
                self.show_feedback_toast(format!("ブックマークの保存に失敗しました: {e}"));
            }
            None => {
                self.show_feedback_toast("ブックマーク DB を開けませんでした".to_string());
            }
        }
    }

    /// ブックマークをクリップボードへエクスポートする (動画と同じ `format_chapter_lines`)。
    fn export_music_bookmarks(&mut self, fs_idx: usize, ctx: &egui::Context) {
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
        let text = crate::video_bookmarks_parser::format_chapter_lines(
            &entries,
            self.music_bookmark_export_seconds_only,
        );
        ctx.copy_text(text);
        self.show_feedback_toast(format!(
            "{} 件をクリップボードへコピーしました",
            entries.len()
        ));
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

    /// 音楽ビュー左パネル (ブックマーク一覧) を描く。命名 / 追加 / 削除 / 改名 / ジャンプ /
    /// インポート / エクスポート。
    pub(crate) fn draw_fs_music_bookmarks_panel(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        panel_rect: egui::Rect,
        fs_idx: usize,
    ) {
        let Some(path) = self.music_audio_path(fs_idx) else {
            return;
        };
        self.ensure_music_bookmarks_loaded(&path);

        // 背景 + 右端区切り線 (左寄せパネル)。
        ui.painter().rect_filled(panel_rect, 0.0, PANEL_BG);
        ui.painter().line_segment(
            [panel_rect.right_top(), panel_rect.right_bottom()],
            egui::Stroke::new(1.0, PANEL_DIVIDER),
        );
        let _ = ui.interact(
            panel_rect,
            ui.id().with(("music_left_bg", fs_idx)),
            egui::Sense::click_and_drag(),
        );

        let title_rect =
            egui::Rect::from_min_size(panel_rect.min, egui::vec2(panel_rect.width(), TITLE_H));
        ui.painter().rect_filled(title_rect, 0.0, TITLE_BG);
        ui.painter().text(
            title_rect.left_center() + egui::vec2(10.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "ブックマーク",
            egui::FontId::proportional(13.0),
            egui::Color32::from_gray(205),
        );

        let content_rect = egui::Rect::from_min_max(
            egui::pos2(panel_rect.left(), title_rect.bottom()),
            panel_rect.max,
        );
        let inner = content_rect.shrink2(egui::vec2(10.0, 8.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        child.set_clip_rect(content_rect);
        *child.visuals_mut() = egui::Visuals::dark();

        // リスト描画中に self を可変借用できないので、行はスナップショットしてから描く。
        let rows: Vec<(i64, f64, Option<String>)> = self
            .music_bookmarks
            .iter()
            .map(|b| (b.id, b.pts_secs, b.title.clone()))
            .collect();
        let import_open = self.music_bookmark_import_open;
        let mut seconds_only = self.music_bookmark_export_seconds_only;
        // 改名バッファを取り出してクロージャ内で編集、末尾で書き戻す。
        let mut rename = self.music_bookmark_rename.take();
        let mut import_text = std::mem::take(&mut self.music_bookmark_import_text);
        // インポートプレビュー用に事前パース。
        let (parsed_entries, parse_errors) = if import_open {
            crate::video_bookmarks_parser::parse_chapter_text(&import_text)
        } else {
            (Vec::new(), Vec::new())
        };
        let mut act: Option<BmAction> = None;

        egui::ScrollArea::vertical()
            .id_salt(("music_bookmarks_scroll", fs_idx))
            .auto_shrink([false, false])
            .show(&mut child, |ui| {
                ui.set_width(ui.available_width());

                // ── 操作ボタン ──
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button("＋ 現在位置")
                        .on_hover_text("再生位置にブックマークを追加 [B]")
                        .clicked()
                    {
                        act = Some(BmAction::Add);
                    }
                    if ui.selectable_label(import_open, "インポート").clicked() {
                        act = Some(BmAction::ToggleImport);
                    }
                    if ui
                        .button("エクスポート")
                        .on_hover_text("一覧をクリップボードへコピー")
                        .clicked()
                    {
                        act = Some(BmAction::Export);
                    }
                });
                ui.add_space(6.0);

                // ── インポート欄 ──
                if import_open {
                    ui.label(
                        egui::RichText::new("mm:ss タイトル (1 行 1 件) を貼り付け")
                            .color(LABEL_COLOR)
                            .size(11.0),
                    );
                    ui.add(
                        egui::TextEdit::multiline(&mut import_text)
                            .desired_rows(4)
                            .desired_width(f32::INFINITY)
                            .hint_text("0:13 イントロ\n1:02 サビ"),
                    );
                    ui.horizontal(|ui| {
                        let n = parsed_entries.len();
                        let e = parse_errors.len();
                        let msg = if e > 0 {
                            format!("{n} 件 / エラー {e} 行")
                        } else {
                            format!("{n} 件")
                        };
                        ui.label(egui::RichText::new(msg).color(LABEL_COLOR).size(11.0));
                        if ui.add_enabled(n > 0, egui::Button::new("登録")).clicked() {
                            act = Some(BmAction::Import);
                        }
                    });
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(6.0);
                }

                // ── 一覧 ──
                if rows.is_empty() {
                    ui.label(
                        egui::RichText::new("ブックマークはありません")
                            .color(LABEL_COLOR)
                            .size(12.0),
                    );
                }
                for (id, secs, title) in &rows {
                    let editing = rename.as_ref().map(|(rid, _)| *rid) == Some(*id);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        // 時刻ボタン (クリックでジャンプ)。
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new(format_hms(*secs))
                                    .color(egui::Color32::from_rgb(150, 200, 245))
                                    .size(12.0),
                            ))
                            .on_hover_text("この位置へジャンプ")
                            .clicked()
                        {
                            act = Some(BmAction::Seek(*secs));
                        }
                        // タイトル (クリックで改名) / 改名中は TextEdit。
                        if editing {
                            if let Some((_, buf)) = rename.as_mut() {
                                let resp = ui.add(
                                    egui::TextEdit::singleline(buf)
                                        .desired_width(f32::INFINITY)
                                        .hint_text("名称"),
                                );
                                if !resp.has_focus() && !resp.lost_focus() {
                                    resp.request_focus();
                                }
                                // Enter (フォーカス喪失) / 別の場所クリックで確定。IME 変換中の
                                // Enter は lost_focus を発火しないので破壊しない。
                                if resp.lost_focus() {
                                    act = Some(BmAction::CommitRename);
                                }
                            }
                        } else {
                            let label = title.clone().unwrap_or_else(|| "(名称なし)".to_string());
                            let color = if title.is_some() {
                                VALUE_COLOR
                            } else {
                                egui::Color32::from_gray(140)
                            };
                            if ui
                                .add(
                                    egui::Label::new(
                                        egui::RichText::new(label).color(color).size(12.0),
                                    )
                                    .sense(egui::Sense::click())
                                    .truncate(),
                                )
                                .on_hover_text("クリックで名称を編集")
                                .clicked()
                            {
                                act = Some(BmAction::StartRename(
                                    *id,
                                    title.clone().unwrap_or_default(),
                                ));
                            }
                        }
                        // 削除 (右寄せ)。
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(egui::Button::new(
                                    egui::RichText::new("×").color(egui::Color32::from_gray(200)),
                                ))
                                .on_hover_text("削除")
                                .clicked()
                            {
                                act = Some(BmAction::Delete(*id));
                            }
                        });
                    });
                }

                // 秒単位トグル (エクスポート形式)。
                if !rows.is_empty() {
                    ui.add_space(6.0);
                    ui.checkbox(&mut seconds_only, "エクスポートは秒単位 (mm:ss)");
                }
            });

        // ── クロージャ外で状態を書き戻し + 操作を適用 ──
        self.music_bookmark_rename = rename;
        self.music_bookmark_import_text = import_text;
        self.music_bookmark_export_seconds_only = seconds_only;

        match act {
            Some(BmAction::Seek(s)) => self.music_seek_to(fs_idx, s),
            Some(BmAction::Delete(id)) => self.delete_music_bookmark(fs_idx, id),
            Some(BmAction::StartRename(id, t)) => {
                self.music_bookmark_rename = Some((id, t));
            }
            Some(BmAction::CommitRename) => {
                if let Some((id, text)) = self.music_bookmark_rename.take() {
                    self.rename_music_bookmark(fs_idx, id, &text);
                }
            }
            Some(BmAction::Add) => self.add_music_bookmark_at_current(fs_idx),
            Some(BmAction::Import) => self.import_music_bookmarks(fs_idx),
            Some(BmAction::Export) => self.export_music_bookmarks(fs_idx, ctx),
            Some(BmAction::ToggleImport) => {
                self.music_bookmark_import_open = !self.music_bookmark_import_open;
            }
            None => {}
        }
    }

    // ───────────────────────── 下 HUD (seek 行 + コントロール行) ─────────────────────────

    /// 音楽ビュー下 HUD (Inc 5 FB): seek 行 + コントロール行 (動画のレイアウトに寄せる)。
    /// 頭出し / 再生・一時停止 / 前後ブックマークジャンプ / ループ / 位置・長さ / 再生速度 /
    /// ミュート / 音量スライダー + シークバー上のブックマークマーカー。常時表示。
    ///
    /// (音量ノーマライズは動画のようなスキャン UI が必要なため、この HUD には載せていない。
    /// 音量正規化は開いた時点で `audio_normalize_db` キャッシュ値が適用される。)
    pub(crate) fn draw_music_bottom_hud(
        &mut self,
        ui: &mut egui::Ui,
        hud_rect: egui::Rect,
        fs_idx: usize,
        pos: f64,
        dur: f64,
        playing: bool,
        dark: bool,
    ) {
        // 現在状態を先に読む (player 借用を短く保つ)。
        let (cur_vol, muted) = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => (player.volume(), player.is_muted()),
            _ => (1.0, false),
        };
        let speed = self.video_playback_speed;
        let loop_on = self.music_loop_enabled;
        // マーカー / ジャンプ用に pts をスナップショット (self.music_bookmarks の借用回避)。
        let marker_secs: Vec<f64> = self.music_bookmarks.iter().map(|b| b.pts_secs).collect();

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
        let mut toggle_loop = false;
        let mut cycle_speed = false;
        let mut toggle_mute = false;
        let mut set_vol: Option<f64> = None;

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
        let btn_bg = |r: egui::Rect, hovered: bool, active: bool| {
            let c = if active {
                accent
            } else if hovered {
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40)
            } else {
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 16)
            };
            painter.rect_filled(r, 4.0, c);
        };

        // 頭出し (|◀)
        let r = alloc(&mut x, bsz);
        let resp = ui
            .interact(
                r,
                ui.id().with(("music_hud_start", fs_idx)),
                egui::Sense::click(),
            )
            .on_hover_text("頭出し");
        btn_bg(r, resp.hovered(), false);
        {
            let c = r.center();
            let t = bsz * 0.16;
            painter.rect_filled(
                egui::Rect::from_center_size(
                    egui::pos2(c.x - t * 1.4, c.y),
                    egui::vec2(bsz * 0.08, t * 2.0),
                ),
                1.0,
                fg,
            );
            painter.add(egui::Shape::convex_polygon(
                vec![
                    c + egui::vec2(t * 0.9, -t),
                    c + egui::vec2(t * 0.9, t),
                    c + egui::vec2(-t * 0.5, 0.0),
                ],
                fg,
                egui::Stroke::NONE,
            ));
        }
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
        btn_bg(r, resp.hovered(), false);
        {
            let c = r.center();
            if playing {
                let bw = bsz * 0.09;
                let bh = bsz * 0.3;
                painter.rect_filled(
                    egui::Rect::from_center_size(c - egui::vec2(bw * 1.5, 0.0), egui::vec2(bw, bh)),
                    1.0,
                    fg,
                );
                painter.rect_filled(
                    egui::Rect::from_center_size(c + egui::vec2(bw * 1.5, 0.0), egui::vec2(bw, bh)),
                    1.0,
                    fg,
                );
            } else {
                let t = bsz * 0.18;
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        c + egui::vec2(-t * 0.7, -t),
                        c + egui::vec2(-t * 0.7, t),
                        c + egui::vec2(t, 0.0),
                    ],
                    fg,
                    egui::Stroke::NONE,
                ));
            }
        }
        if resp.clicked() {
            toggle_play = true;
        }

        // 前ブックマーク (◀◀)
        let r = alloc(&mut x, bsz);
        let resp = ui
            .interact(
                r,
                ui.id().with(("music_hud_prevbm", fs_idx)),
                egui::Sense::click(),
            )
            .on_hover_text("前のブックマーク");
        btn_bg(r, resp.hovered(), false);
        draw_double_triangle(&painter, r, false, fg);
        if resp.clicked() {
            if let Some(&t) = marker_secs.iter().rev().find(|&&s| s < pos - 0.3) {
                seek_to = Some(t);
            } else if !marker_secs.is_empty() {
                seek_to = Some(0.0);
            }
        }

        // 次ブックマーク (▶▶)
        let r = alloc(&mut x, bsz);
        let resp = ui
            .interact(
                r,
                ui.id().with(("music_hud_nextbm", fs_idx)),
                egui::Sense::click(),
            )
            .on_hover_text("次のブックマーク");
        btn_bg(r, resp.hovered(), false);
        draw_double_triangle(&painter, r, true, fg);
        if resp.clicked()
            && let Some(&t) = marker_secs.iter().find(|&&s| s > pos + 0.3)
        {
            seek_to = Some(t);
        }

        // ループ
        let r = alloc(&mut x, bsz);
        let resp = ui
            .interact(
                r,
                ui.id().with(("music_hud_loop", fs_idx)),
                egui::Sense::click(),
            )
            .on_hover_text("ループ");
        btn_bg(r, resp.hovered(), loop_on);
        draw_loop_icon(
            &painter,
            r,
            if loop_on { egui::Color32::WHITE } else { dim },
        );
        if resp.clicked() {
            toggle_loop = true;
        }

        // ── コントロール行: 右クラスタ (右寄せ: 音量 / ミュート / 速度 / 時間) ──
        let mut rx = hud_rect.right() - 14.0;
        // 音量スライダー。動画と同じフェーダーマッピング (`video_volume_*_fader_pos`) を使い、
        // -80..+18dB の全域を一貫して扱う (Codex P3: 独自 dB マップだと高ブースト/微小音量が
        // つぶれ、動画のフェーダーと挙動が食い違う)。永続化はドラッグ確定時のみ (毎フレーム save 回避)。
        let vol_w = 120.0;
        let vol_rect = egui::Rect::from_min_max(
            egui::pos2(rx - vol_w, controls_cy - 4.0),
            egui::pos2(rx, controls_cy + 4.0),
        );
        rx -= vol_w + 10.0;
        let vol_frac =
            crate::settings::video_volume_linear_to_fader_pos(cur_vol).clamp(0.0, 1.0) as f32;
        painter.rect_filled(vol_rect, 3.0, egui::Color32::from_gray(70));
        painter.rect_filled(
            egui::Rect::from_min_max(
                vol_rect.min,
                egui::pos2(
                    vol_rect.left() + vol_rect.width() * vol_frac,
                    vol_rect.bottom(),
                ),
            ),
            3.0,
            accent,
        );
        painter.circle_filled(
            egui::pos2(
                vol_rect.left() + vol_rect.width() * vol_frac,
                vol_rect.center().y,
            ),
            6.0,
            fg,
        );
        let vol_resp = ui.interact(
            vol_rect.expand2(egui::vec2(0.0, 8.0)),
            ui.id().with(("music_hud_vol", fs_idx)),
            egui::Sense::click_and_drag(),
        );
        let mut vol_persist = false;
        if (vol_resp.clicked() || vol_resp.dragged())
            && let Some(p) = vol_resp.interact_pointer_pos()
        {
            let f = ((p.x - vol_rect.left()) / vol_rect.width()).clamp(0.0, 1.0) as f64;
            set_vol = Some(crate::settings::clamp_video_volume(
                crate::settings::video_volume_fader_pos_to_linear(f),
            ));
        }
        if vol_resp.drag_stopped() || vol_resp.clicked() {
            vol_persist = true;
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
        btn_bg(mute_r, mresp.hovered(), muted);
        draw_speaker_icon(
            &painter,
            mute_r,
            muted,
            if muted {
                egui::Color32::from_rgb(255, 150, 150)
            } else {
                fg
            },
        );
        if mresp.clicked() {
            toggle_mute = true;
        }
        // 速度 (クリックでプリセット巡回)
        let spd_w = 46.0;
        let spd_r = egui::Rect::from_min_size(
            egui::pos2(rx - spd_w, controls_cy - bsz * 0.5),
            egui::vec2(spd_w, bsz),
        );
        rx -= spd_w + 8.0;
        let sresp = ui
            .interact(
                spd_r,
                ui.id().with(("music_hud_speed", fs_idx)),
                egui::Sense::click(),
            )
            .on_hover_text("再生速度");
        btn_bg(spd_r, sresp.hovered(), (speed - 1.0).abs() > 1e-3);
        painter.text(
            spd_r.center(),
            egui::Align2::CENTER_CENTER,
            format!("{}x", format_speed(speed)),
            egui::FontId::proportional(12.0),
            fg,
        );
        if sresp.clicked() {
            cycle_speed = true;
        }
        // 時間 (速度ボタンの左に右寄せ)
        painter.text(
            egui::pos2(rx, controls_cy),
            egui::Align2::RIGHT_CENTER,
            format!("{} / {}", format_hms(pos), format_hms(dur)),
            egui::FontId::proportional(13.0),
            fg,
        );

        // ── 操作を適用 (self / player の可変借用を分離) ──
        let next_speed = if cycle_speed {
            let i = MUSIC_SPEED_PRESETS
                .iter()
                .position(|&s| (s - speed).abs() < 1e-3)
                .unwrap_or(2);
            MUSIC_SPEED_PRESETS[(i + 1) % MUSIC_SPEED_PRESETS.len()]
        } else {
            speed
        };
        if seek_start {
            seek_to = Some(0.0);
        }
        if toggle_loop {
            self.music_loop_enabled = !loop_on;
        }
        if cycle_speed {
            self.video_playback_speed = next_speed;
        }
        if toggle_mute {
            self.video_session_muted = !muted;
        }
        if let Some(v) = set_vol {
            self.settings.video_volume = v;
            // 動画の音量経路と同じく、確定時のみ永続化する (ドラッグ中の毎フレーム save を回避)。
            if vol_persist {
                self.settings.save();
            }
        }
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            if let Some(s) = seek_to {
                player.seek(s);
            }
            if seek_start {
                player.set_playing(true);
            }
            if toggle_play {
                player.toggle_play();
            }
            if toggle_loop {
                player.set_loop_enabled(!loop_on);
            }
            if cycle_speed {
                player.set_playback_speed(next_speed);
            }
            if toggle_mute {
                player.set_muted(!muted);
            }
            if let Some(v) = set_vol {
                player.set_volume(v);
            }
        }
    }
}

/// 二重三角形 (◀◀ / ▶▶) を painter で描く (前後ブックマークジャンプ用、フォント非依存)。
fn draw_double_triangle(
    painter: &egui::Painter,
    rect: egui::Rect,
    right: bool,
    color: egui::Color32,
) {
    let c = rect.center();
    let t = rect.width() * 0.14;
    let dir = if right { 1.0 } else { -1.0 };
    for k in [-1.0f32, 1.0] {
        let ox = k * t * 1.05 * dir;
        painter.add(egui::Shape::convex_polygon(
            vec![
                c + egui::vec2(ox - t * dir, -t),
                c + egui::vec2(ox - t * dir, t),
                c + egui::vec2(ox + t * dir, 0.0),
            ],
            color,
            egui::Stroke::NONE,
        ));
    }
}

/// ループアイコン (円 + 矢先) を painter で描く (フォント非依存、絵文字 tofu 回避)。
fn draw_loop_icon(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let c = rect.center();
    let r = rect.width() * 0.24;
    painter.circle_stroke(c, r, egui::Stroke::new(2.0, color));
    // 上部右に小さな矢先 (回転を示唆)。
    let tip = egui::pos2(c.x + r, c.y - r * 0.15);
    let s = r * 0.42;
    painter.add(egui::Shape::convex_polygon(
        vec![
            tip,
            tip + egui::vec2(-s, -s * 0.2),
            tip + egui::vec2(-s * 0.2, s),
        ],
        color,
        egui::Stroke::NONE,
    ));
}

/// スピーカーアイコン (ミュート時は赤い斜線) を painter で描く。
fn draw_speaker_icon(painter: &egui::Painter, rect: egui::Rect, muted: bool, color: egui::Color32) {
    let c = rect.center();
    let s = rect.width() * 0.16;
    // スピーカー本体 (小さな四角 + 三角のホーン)。
    painter.rect_filled(
        egui::Rect::from_center_size(egui::pos2(c.x - s * 1.1, c.y), egui::vec2(s, s * 1.1)),
        1.0,
        color,
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(c.x - s * 0.6, c.y - s * 0.9),
            egui::pos2(c.x - s * 0.6, c.y + s * 0.9),
            egui::pos2(c.x + s * 0.3, c.y),
        ],
        color,
        egui::Stroke::NONE,
    ));
    if muted {
        painter.line_segment(
            [
                egui::pos2(c.x + s * 0.5, c.y - s * 0.9),
                egui::pos2(c.x + s * 1.3, c.y + s * 0.9),
            ],
            egui::Stroke::new(2.0, color),
        );
    } else {
        // 音波 (小さな 2 本の弧の代わりに短い縦 dash)。
        painter.line_segment(
            [
                egui::pos2(c.x + s * 0.8, c.y - s * 0.5),
                egui::pos2(c.x + s * 0.8, c.y + s * 0.5),
            ],
            egui::Stroke::new(2.0, color),
        );
    }
}

/// 再生速度を "0.5" / "1" / "1.25" 風に整形する (末尾ゼロを落とす)。
fn format_speed(speed: f64) -> String {
    let s = format!("{speed:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
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
