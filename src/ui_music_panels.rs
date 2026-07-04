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
// 音楽ビューはメイン egui ctx (アプリテーマ = Light になり得る) に描くため、egui 既定の
// `on_hover_text` はツールチップが明色になる。動画 native HUD (専用ダーク ctx) と見た目を
// 揃えるため、ctx テーマに依らずダーク枠で描く `hover_tip_dark` を使う。
use crate::ui_helpers::HoverTipExt;
use crate::video::native_presenter::overlay_draw::{
    NativeJumpPanelOptions, draw_native_bookmark_title_editor, draw_native_bulk_bookmark_dialog,
    draw_native_jump_panel_body, draw_overlay_arrow_icon, draw_overlay_bookmark_icon,
    draw_overlay_button_bg, draw_overlay_continuous_icon, draw_overlay_loop_icon,
    draw_overlay_pause_icon, draw_overlay_play_icon, draw_overlay_replay_icon,
    draw_overlay_skip_to_marker_icon, draw_overlay_speaker_icon, draw_overlay_speed_control,
    draw_overlay_volume_slider,
};
use crate::video::native_presenter::{
    NativeOverlayCommand, NativeOverlayJumpEntry, NativeOverlayTimelineMarkerKind,
};

/// 左パネル (ブックマーク) の幅。画像補正パネル (`LEFT_PANEL_WIDTH`) と揃える。
pub(crate) const MUSIC_LEFT_PANEL_WIDTH: f32 = 292.0;
/// 右パネル (音楽情報 + タグ) の幅。動画 native の右メタデータパネル
/// (`native_metadata_panel_width()` = 430) に揃え、video↔audio 切替で幅が
/// ジャンプしないようにする (Inc 7 ④)。
pub(crate) const MUSIC_RIGHT_PANEL_WIDTH: f32 = 430.0;
/// 下 HUD の高さ (seek 行 + コントロール行、常時表示、Inc 5 FB で動画寄りに)。
pub(crate) const MUSIC_HUD_HEIGHT: f32 = 62.0;

const PANEL_BG: egui::Color32 = egui::Color32::from_rgba_premultiplied(16, 16, 20, 235);
const PANEL_DIVIDER: egui::Color32 = egui::Color32::from_rgba_premultiplied(255, 255, 255, 40);
const TITLE_BG: egui::Color32 = egui::Color32::from_rgba_premultiplied(28, 28, 36, 240);
const LABEL_COLOR: egui::Color32 = egui::Color32::from_rgb(150, 168, 205);
const VALUE_COLOR: egui::Color32 = egui::Color32::from_rgb(228, 230, 236);
const TITLE_H: f32 = 30.0;

/// ツールチップにショートカットを併記する ("ミュート" + Some("M") → "ミュート [M]")。
/// 動画 native HUD の `native_label_with_shortcut` 相当。未割り当て (None / 空) はラベルのみ。
fn label_with_shortcut(label: &str, chord: Option<&str>) -> String {
    match chord {
        Some(c) if !c.trim().is_empty() => format!("{label} [{c}]"),
        _ => label.to_string(),
    }
}

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

/// J/K マーカーナビの seek 結果。`Marker` = 具体的なブックマーク秒へ、`Start` = 前ブックマークが
/// 無い J キーのフォールバックで先頭 0.0 へ。
#[derive(Debug, Clone, Copy, PartialEq)]
enum MusicMarkerJump {
    Marker(f64),
    Start,
}

/// J/K マーカーナビの seek 先を決める純関数 (副作用なし・テスト用)。`starts` は昇順・filter/dedup
/// 済みのブックマーク秒。現在位置 `pos` の ± epsilon を境にした最近接探索で「現在マーカーで足踏み」
/// を防ぐ (動画 J/K と同じ規約)。返り値 `None` は no-op。
/// - `forward=true` (K): 次ブックマーク (pos+EPS 超)、無ければ `None`。
/// - `forward=false` (J): 前ブックマーク (pos-EPS 未満)、無ければ、まだ先頭でなければ `Start`
///   (= 先頭 0.0 へ)、既に先頭なら `None`。
fn music_marker_target(starts: &[f64], pos: f64, forward: bool) -> Option<MusicMarkerJump> {
    // 「現在マーカーで足踏み」を防ぐ許容。音声は下 HUD の前後ブックマークボタン (0.3s) と
    // 揃える (= J/K キーとボタンで同じジャンプ挙動)。動画 J/K は NAV_MARKER_EPSILON=0.5 だが、
    // 音声は HUD との内部一貫性を優先して 0.3 にする (0.3-0.5s の差は実用上ほぼ不可視、Codex P3)。
    const EPSILON: f64 = 0.3;
    const ALREADY_AT_START_TOL: f64 = 0.05;
    if forward {
        starts
            .iter()
            .copied()
            .find(|&s| s > pos + EPSILON)
            .map(MusicMarkerJump::Marker)
    } else if let Some(t) = starts.iter().copied().rev().find(|&s| s < pos - EPSILON) {
        Some(MusicMarkerJump::Marker(t))
    } else if pos > ALREADY_AT_START_TOL {
        Some(MusicMarkerJump::Start)
    } else {
        None
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

    /// 相対シーク (現在位置 + delta を [0, duration] にクランプして seek)。←→ シークキーの実体で、
    /// 動画 `seek_relative` の音声版。seek 後のループ target 再計算のため `music_seek_to` に集約する。
    pub(crate) fn music_seek_relative(&mut self, fs_idx: usize, delta_secs: f64) {
        let (pos, dur) = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                (player.position().max(0.0), player.duration().max(0.0))
            }
            _ => return,
        };
        let target = if dur > 0.0 {
            (pos + delta_secs).clamp(0.0, dur)
        } else {
            (pos + delta_secs).max(0.0)
        };
        self.music_seek_to(fs_idx, target);
    }

    /// 頭出し (先頭 0.0 へ seek + 即再生)。下 HUD の 頭出しボタンと W キー (VideoSeekStart) の
    /// 共通実体。動画の rewind (= seek(0.0) + play) の音声版で、明示 `set_playing(true)` で
    /// 一時停止中でも頭から再生を開始する (seek 自体の autoplay intent に依存しない)。
    pub(crate) fn music_seek_start(&mut self, fs_idx: usize) {
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_playing(true);
        }
        self.music_seek_to(fs_idx, 0.0);
    }

    /// 音楽ビューの下 HUD の前/次ファイルボタンの実体。動画 HUD の ↑↓ ボタン
    /// (`VideoPrevFile` / `VideoNextFile`) およびキーボードの ↑↓ と同一挙動で、表示順
    /// (`current_grid_order`) の隣接する移動可能アイテム (画像 / 動画 / 音声) へ移動する。
    /// キーボード経路 (ui_fullscreen.rs の `nav_delta` → `adjacent_navigable_idx` →
    /// `open_fullscreen_from_fs_navigation`) をそのまま踏襲する。境界では中央にヒントを出す。
    /// `delta`: -1 = 前, +1 = 次。
    pub(crate) fn music_navigate_file(&mut self, ctx: &egui::Context, fs_idx: usize, delta: i32) {
        // ユーザー入力起点のナビなので input_seq を bump する (キーボード nav の "fs_key" と同様、
        // perf 帰属 / last_input_at ベースの idle 判定を今回の操作に紐付ける、Codex P3)。
        self.bump_input_seq("music_nav_file", Some(&format!("delta={delta}")));
        let display_order = self.current_grid_order().to_vec();
        if let Some(new_idx) =
            crate::ui_helpers::adjacent_navigable_idx(&self.items, &display_order, fs_idx, delta)
        {
            self.open_fullscreen_from_fs_navigation(ctx, new_idx);
        } else {
            self.fs_boundary_hint = Some(crate::ui_fullscreen::FsBoundaryHint::Edge {
                at_end: delta > 0,
                at: std::time::Instant::now(),
            });
        }
    }

    /// J/K マーカーナビ (ブックマーク間の前後ジャンプ)。動画 `VideoMarkerPrev`/`VideoMarkerNext`
    /// の音声版。音声はチャプター/ピンを持たないのでブックマークのみ対象。seek 先の決定は副作用
    /// ゼロの純関数 `music_marker_target` に集約 (= unit test 可能)。マーカー集合は freshness gate
    /// 付きの `music_bookmark_starts_for` (現在の音声パスに読み込み済みのブックマークだけ、昇順・
    /// filter/dedup 済み) を使う。
    pub(crate) fn music_marker_jump(&mut self, fs_idx: usize, forward: bool) {
        let starts = self.music_bookmark_starts_for(fs_idx);
        let pos = self.music_player_position(fs_idx).unwrap_or(0.0);
        match music_marker_target(&starts, pos, forward) {
            Some(MusicMarkerJump::Marker(t)) => {
                self.music_seek_to(fs_idx, t);
                let dir = if forward { "次の" } else { "前の" };
                self.show_feedback_toast(format!("{} {dir}ブックマーク", format_hms(t)));
            }
            Some(MusicMarkerJump::Start) => {
                self.music_seek_to(fs_idx, 0.0);
                self.show_feedback_toast(format!("{} 先頭", format_hms(0.0)));
            }
            None => {}
        }
    }

    /// 音楽ビューで解析 / タイムライン / スペクトラム / ブックマークの対象にする「音源」のパス
    /// (3 概念分離のうち解析ソース、[`Self::fs_music_view_active`] は表示/ゲート判定)。
    ///
    /// 音声ファイルはそのパスを返す。Inc 7 (動画→音声モード) では、音声モードにトグルされた
    /// 動画 (`video_audio_mode == Some(fs_idx)`) の場合にその動画ファイルのパスを返す
    /// (その動画の音声トラックを解析する)。名前が「audio_path」ではなく「source」なのは、
    /// 動画パスも返し得るため。stale index 対策で動画アームは `fullscreen_idx == Some(fs_idx)`
    /// も確認する ([`Self::fs_music_view_active`] と揃える、Codex 7c 設計レビュー)。
    pub(crate) fn fs_music_source_for_idx(&self, fs_idx: usize) -> Option<std::path::PathBuf> {
        match self.items.get(fs_idx) {
            Some(GridItem::Audio(p)) => Some(p.clone()),
            Some(GridItem::Video(p))
                if self.video_audio_mode == Some(fs_idx) && self.fullscreen_idx == Some(fs_idx) =>
            {
                Some(p.clone())
            }
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
        let Some(path) = self.fs_music_source_for_idx(fs_idx) else {
            return;
        };
        let pos = secs.max(0.0);
        let added = self
            .video_bookmark_db
            .as_ref()
            .and_then(|db| db.add_if_no_duplicate(&path, pos, None, 1.0).ok())
            .flatten();
        self.reload_music_bookmarks(&path);
        // 境界集合が変わったのでブックマーク区間ループの target / baseline を再計算する
        // (でないと初回ブックマーク追加時に stale baseline で誤ループする、Codex P2)。
        self.apply_music_loop_mode(fs_idx);
        match added {
            Some(_) => self.show_feedback_toast(format!("ブックマークを追加: {}", format_hms(pos))),
            None => self.show_feedback_toast("既存のブックマークと近すぎます".to_string()),
        }
    }

    fn delete_music_bookmark(&mut self, fs_idx: usize, id: i64) {
        let Some(path) = self.fs_music_source_for_idx(fs_idx) else {
            return;
        };
        if let Some(db) = self.video_bookmark_db.as_ref() {
            let _ = db.remove(id);
        }
        if self.music_bookmark_title_edit.as_ref().map(|e| e.id) == Some(id) {
            self.music_bookmark_title_edit = None;
        }
        self.reload_music_bookmarks(&path);
        self.apply_music_loop_mode(fs_idx);
    }

    fn rename_music_bookmark(&mut self, fs_idx: usize, id: i64, title: &str) {
        let Some(path) = self.fs_music_source_for_idx(fs_idx) else {
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
        let Some(path) = self.fs_music_source_for_idx(fs_idx) else {
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
                self.apply_music_loop_mode(fs_idx);
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
        let Some(path) = self.fs_music_source_for_idx(fs_idx) else {
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
        let Some(path) = self.fs_music_source_for_idx(fs_idx) else {
            return;
        };
        let result = self
            .video_bookmark_db
            .as_ref()
            .map(|db| db.clear_for(&path));
        self.music_bookmark_title_edit = None;
        self.reload_music_bookmarks(&path);
        self.apply_music_loop_mode(fs_idx);
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
        let Some(path) = self.fs_music_source_for_idx(fs_idx) else {
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
        // リミッター作動インジケータ (動画 native HUD と同じ挙動): player の
        // limiter_ceiling_hit_seq の増加を検知したら赤ドットを一定時間点灯する。
        // normalize が有効なときだけ右クラスタにスロットを確保する (点灯有無で幅が
        // ジッタしないように、有効時は常にスロット確保・ドットは作動時のみ描画)。
        let now = std::time::Instant::now();
        let cur_limiter_seq = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player.limiter_ceiling_hit_seq(),
            _ => self.music_limiter_last_seq,
        };
        if cur_limiter_seq > self.music_limiter_last_seq {
            self.music_limiter_visible_until =
                now.checked_add(std::time::Duration::from_millis(1500));
        } else if cur_limiter_seq < self.music_limiter_last_seq {
            // 別プレイヤー (曲切替) で seq が 0 に巻き戻ったら stale な赤ドットを消灯する
            // (動画 native HUD と同じ、Codex P3)。
            self.music_limiter_visible_until = None;
        }
        self.music_limiter_last_seq = cur_limiter_seq;
        let limiter_visible = self
            .music_limiter_visible_until
            .is_some_and(|until| now < until);
        let show_limiter_slot = self.settings.audio_normalize_enabled;
        // 各ボタンのツールチップに併記するショートカット (動画 HUD と同じく共有 Video* アクションの
        // keymap chord を使う)。連続再生/速度/limiter/Norm はキーボードショートカットが無いので対象外。
        let sc_seek_start = self
            .keymap
            .first_chord_label(crate::keymap::KeyAction::VideoSeekStart);
        let sc_play = self
            .keymap
            .first_chord_label(crate::keymap::KeyAction::VideoPlayPause);
        let sc_marker_prev = self
            .keymap
            .first_chord_label(crate::keymap::KeyAction::VideoMarkerPrev);
        let sc_marker_next = self
            .keymap
            .first_chord_label(crate::keymap::KeyAction::VideoMarkerNext);
        let sc_loop = self
            .keymap
            .first_chord_label(crate::keymap::KeyAction::VideoLoop);
        let sc_mute = self
            .keymap
            .first_chord_label(crate::keymap::KeyAction::VideoMute);
        let sc_prev_file = self
            .keymap
            .first_chord_label(crate::keymap::KeyAction::VideoPrevFile);
        let sc_next_file = self
            .keymap
            .first_chord_label(crate::keymap::KeyAction::VideoNextFile);
        let speed = self.video_playback_speed;
        // ループ / 連続再生モードは動画と共有 (video_loop_mode / video_continuous_mode)。
        // 音声はチャプター無しなので effective は Off/Full/Bookmark のみ。
        let continuous_mode = self.video_continuous_mode;
        // マーカー / ジャンプ用に pts をスナップショット (self.music_bookmarks の借用回避)。
        let marker_secs: Vec<f64> = self.music_bookmarks.iter().map(|b| b.pts_secs).collect();
        let has_bm_now = self.music_bookmarks_loaded_for.is_some() && !marker_secs.is_empty();
        let loop_eff =
            crate::settings::effective_loop_mode(self.settings.video_loop_mode, false, has_bm_now);

        let painter = ui.painter_at(hud_rect);
        painter.rect_filled(
            hud_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, if dark { 150 } else { 60 }),
        );

        let seek_row_h = 22.0;
        let controls_cy = (hud_rect.top() + seek_row_h + hud_rect.bottom()) * 0.5;
        // 動画 native HUD と同じ 2 系統: アイコン/ドット/スライダーは幾何中心 (controls_cy)、
        // テキストは +4.0 した baseline (text_center_y) に置く。egui の CENTER 揃えは text bbox の
        // 中心を合わせるので、13px フォントだと光学的に上寄りに見え、ドット/アイコンと縦がずれる。
        // 動画側 `text_center_y = center_y + 4.0` (native_presenter/mod.rs) と同値。
        let text_center_y = controls_cy + 4.0;
        // レイアウト寸法は動画 native HUD (native_presenter/mod.rs) と同値にする (Inc 7 ③):
        // 端 padding = side_pad、ボタン間 = gap、グループ境界 = gap + group_gap_extra。
        // シークバー・左クラスタ・右クラスタの端揃え / グループ間隔を動画に一致させる。
        let side_pad = 10.0;
        let gap = 8.0;
        let group_gap_extra = 8.0;

        // 収集する操作 (描画中は self を可変借用しないため、末尾でまとめて適用)。
        let mut seek_to: Option<f64> = None;
        let mut toggle_play = false;
        let mut seek_start = false;
        let mut cycle_loop = false;
        let mut cycle_continuous = false;
        let mut toggle_mute = false;
        let mut set_vol: Option<f64> = None;
        // 前/次ファイル移動 intent (-1 = 前, +1 = 次)。動画 HUD の ↑↓ = VideoPrevFile/NextFile
        // と同一挙動 (末尾でまとめて適用)。
        let mut nav_file: Option<i32> = None;
        // 音量ノーマライズボタン (Norm) の左クリック intent。スキャン機構が windows 限定の
        // ため windows でのみ収集・適用する。右クリックは音楽 HUD では使わない (背後の
        // フルスクリーン右クリックハンドラにも届いて二重動作するため。音量/速度と同方針、
        // 実機 FB 2026-07-02)。
        #[cfg(windows)]
        let mut toggle_normalize = false;
        // `set_speed` は速度ボタン描画時に `draw_overlay_speed_control` の返り値で一度だけ
        // 束縛する (他フラグと違い条件付き更新ではないため、代入時点で宣言する)。

        // ── seek 行 (バー + ブックマークマーカー + クリック/ドラッグ seek) ──
        // シークバーの見た目は動画 native HUD (native_presenter/mod.rs) に揃える (Inc 7 ③):
        // トラック gray(74) / 角丸 2.0 / 太さ 8、fill は白 (228,228,228)。動画と video↔audio
        // 切替で視覚ジャンプしないようにする。
        let bar_margin = side_pad;
        let bar_h = 8.0;
        let bar_cy = hud_rect.top() + seek_row_h * 0.5;
        let bar_rect = egui::Rect::from_min_max(
            egui::pos2(hud_rect.left() + bar_margin, bar_cy - bar_h * 0.5),
            egui::pos2(hud_rect.right() - bar_margin, bar_cy + bar_h * 0.5),
        );
        painter.rect_filled(bar_rect, 2.0, egui::Color32::from_gray(74));
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
            painter.rect_filled(filled, 2.0, egui::Color32::from_rgb(228, 228, 228));
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

        // ── コントロール行: 左クラスタ ──
        // 並び順・グループ間隔・左端揃えを動画 native HUD に完全一致させる (Inc 7 ③ 実機 FB):
        //   [頭出し][再生] | [ループ][連続][前ファイル][次ファイル] | [前マーカー][次マーカー]
        // 前/次ファイル (↑↓ = VideoPrevFile/NextFile) は音声でも表示する (実機 FB: 動画と揃える)。
        // 動画のキャプチャパレット (コマ送り ◀▶ / スクショ / 保存) だけは音声では非表示
        // (§5.8「音楽は無視」)。グループ内 = gap、境界 = gap + group_gap_extra、始点 = side_pad。
        let bsz = 28.0;
        let mut x = hud_rect.left() + side_pad;
        let alloc = |x: &mut f32, w: f32| -> egui::Rect {
            let r = egui::Rect::from_min_size(
                egui::pos2(*x, controls_cy - bsz * 0.5),
                egui::vec2(w, bsz),
            );
            *x += w + gap;
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
            .hover_tip_dark(label_with_shortcut("頭出し", sc_seek_start.as_deref()));
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
            .hover_tip_dark(label_with_shortcut(
                if playing { "一時停止" } else { "再生" },
                sc_play.as_deref(),
            ));
        draw_overlay_button_bg(&painter, r, resp.hovered(), false);
        if playing {
            draw_overlay_pause_icon(&painter, r.center(), bsz * 0.30);
        } else {
            draw_overlay_play_icon(&painter, r.center(), bsz * 0.38);
        }
        if resp.clicked() {
            toggle_play = true;
        }

        // グループ境界: [頭出し][再生] | [ループ][連続]
        x += group_gap_extra;

        // ループ (Off → 全体 → ブックマーク間 → Off で循環、動画 L キーと共有)。アイコン描画・
        // 配色は動画 HUD (native_presenter) と揃える: 連続再生中は淡色 + no-op、mode_active は
        // 水色、ブックマークモードは「ブックマークアイコン + 小さめループアイコン」の合成表示。
        use crate::settings::VideoLoopMode;
        let continuous_active = continuous_mode.is_enabled();
        let mode_active = !continuous_active && !matches!(loop_eff, VideoLoopMode::Off);
        let loop_icon_color = if continuous_active {
            egui::Color32::from_gray(120)
        } else if mode_active {
            egui::Color32::from_rgb(170, 230, 255)
        } else {
            egui::Color32::from_rgb(238, 238, 238)
        };
        let loop_tooltip = if continuous_active {
            "連続再生中はループ無効"
        } else {
            match loop_eff {
                VideoLoopMode::Off => "ループ再生",
                VideoLoopMode::Full => "ループ: 全体",
                VideoLoopMode::Bookmark => "ループ: ブックマーク",
                VideoLoopMode::Chapter => "ループ: 全体",
            }
        };
        let r = alloc(&mut x, bsz);
        let resp = ui
            .interact(
                r,
                ui.id().with(("music_hud_loop", fs_idx)),
                egui::Sense::click(),
            )
            .hover_tip_dark(label_with_shortcut(loop_tooltip, sc_loop.as_deref()));
        draw_overlay_button_bg(
            &painter,
            r,
            resp.hovered() && !continuous_active,
            mode_active,
        );
        let ir = bsz * 0.36;
        let ic = r.center();
        match loop_eff {
            VideoLoopMode::Bookmark => {
                // 動画 HUD と同じ「上=ブックマーク / 下=小さめループ」の合成 (Chapter は音声に無い)。
                draw_overlay_loop_icon(
                    &painter,
                    egui::pos2(ic.x, ic.y + ir * 0.18),
                    ir * 0.65,
                    loop_icon_color,
                );
                draw_overlay_bookmark_icon(
                    &painter,
                    egui::pos2(ic.x, ic.y - ir * 0.55),
                    ir * 0.32,
                    egui::Color32::from_rgb(255, 220, 82),
                );
            }
            _ => {
                draw_overlay_loop_icon(&painter, ic, ir, loop_icon_color);
            }
        }
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
            .hover_tip_dark(cont_tooltip);
        draw_overlay_button_bg(&painter, r, resp.hovered(), continuous_mode.is_enabled());
        draw_overlay_continuous_icon(&painter, r, continuous_mode);
        if resp.clicked() {
            cycle_continuous = true;
        }

        // 前ファイル (↑ = 前の項目、動画 HUD の ↑ = VideoPrevFile と同一)。continuous と同じ
        // group B に含める (gap のみ、境界なし)。前/次フレーム (コマ送り) とキャプチャは非表示。
        let r = alloc(&mut x, bsz);
        let resp = ui
            .interact(
                r,
                ui.id().with(("music_hud_prevfile", fs_idx)),
                egui::Sense::click(),
            )
            .hover_tip_dark(label_with_shortcut("前の項目", sc_prev_file.as_deref()));
        draw_overlay_button_bg(&painter, r, resp.hovered(), false);
        draw_overlay_arrow_icon(&painter, r, -1);
        if resp.clicked() {
            nav_file = Some(-1);
        }

        // 次ファイル (↓ = 次の項目、動画 HUD の ↓ = VideoNextFile と同一)。
        let r = alloc(&mut x, bsz);
        let resp = ui
            .interact(
                r,
                ui.id().with(("music_hud_nextfile", fs_idx)),
                egui::Sense::click(),
            )
            .hover_tip_dark(label_with_shortcut("次の項目", sc_next_file.as_deref()));
        draw_overlay_button_bg(&painter, r, resp.hovered(), false);
        draw_overlay_arrow_icon(&painter, r, 1);
        if resp.clicked() {
            nav_file = Some(1);
        }

        // グループ境界: [ループ][連続][前ファイル][次ファイル] | [前マーカー][次マーカー]
        x += group_gap_extra;

        // 前ブックマーク (|◀)
        let r = alloc(&mut x, bsz);
        let resp = ui
            .interact(
                r,
                ui.id().with(("music_hud_prevbm", fs_idx)),
                egui::Sense::click(),
            )
            .hover_tip_dark(label_with_shortcut(
                "前のブックマーク",
                sc_marker_prev.as_deref(),
            ));
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
            .hover_tip_dark(label_with_shortcut(
                "次のブックマーク",
                sc_marker_next.as_deref(),
            ));
        draw_overlay_button_bg(&painter, r, resp.hovered(), false);
        draw_overlay_skip_to_marker_icon(&painter, r, 1, markers_present);
        if resp.clicked()
            && let Some(&t) = marker_secs.iter().find(|&&s| s > pos + 0.3)
        {
            seek_to = Some(t);
        }

        // ── コントロール行: 右クラスタ (右寄せ: リミッター / dB ラベル / 音量 / Norm /
        // ミュート / 速度 / 時間) ──
        // 右端 padding は動画 native HUD の side_pad に揃える (旧 14 → 10、音量バー位置ズレ修正)。
        let mut rx = hud_rect.right() - side_pad;
        // リミッター作動ドット (最右、動画 HUD の vol_label の右に置くのと同じ)。normalize
        // 有効時のみスロットを確保し、直近作動時だけ赤ドットを描く。
        let limiter_slot_w = 14.0;
        if show_limiter_slot {
            if limiter_visible {
                let dot_c = egui::pos2(rx - limiter_slot_w * 0.5, controls_cy);
                let lim_rect = egui::Rect::from_center_size(dot_c, egui::vec2(limiter_slot_w, bsz));
                let lim_resp = ui.interact(
                    lim_rect,
                    ui.id().with(("music_hud_limiter", fs_idx)),
                    egui::Sense::hover(),
                );
                painter.circle_filled(
                    dot_c,
                    if lim_resp.hovered() { 4.5 } else { 4.0 },
                    egui::Color32::from_rgb(255, 72, 72),
                );
                lim_resp.hover_tip_dark("出力リミッターが作動しました");
                ui.ctx().request_repaint(); // 点灯期限まで消灯を反映するため repaint
            }
            // ドット中心は rx - limiter_slot_w*0.5。スロットぶん詰めると dB ラベル右端が
            // ドット中心の limiter_slot_w*0.5 (=7px) 左に来て、動画 HUD の vol_label→limiter
            // 間隔と一致する。
            rx -= limiter_slot_w;
        }
        // 現在音量の dB 表示ラベル (最右、動画 HUD の「スライダーの右」配置に合わせる)。
        // ミュート状態に関わらず実効音量を dB で示す。共有の
        // `format_video_volume_db_compact` を使い動画と表記を揃える (-∞dB / 0.0dB / +3.0dB)。
        let vol_label_w = 60.0;
        let vol_db_label = crate::video::native_presenter::format_video_volume_db_compact(cur_vol);
        let vol_label_color = if cur_vol > 1.0 {
            egui::Color32::from_rgb(255, 210, 80)
        } else {
            egui::Color32::from_rgb(238, 238, 238)
        };
        painter.text(
            egui::pos2(rx, text_center_y),
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
        let vol_w = 144.0;
        let vol_rect = egui::Rect::from_min_max(
            egui::pos2(rx - vol_w, controls_cy - 4.0),
            egui::pos2(rx, controls_cy + 4.0),
        );
        rx -= vol_w + 8.0;
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
        // 音量ノーマライズボタン (Norm、音量スライダーとミュートの間)。動画 native HUD と
        // 同じ 5 状態・配色・ラベルで描く。左クリックのみ (右クリックは背後 FS と二重動作の
        // ため不使用): Off→ON / OnApplied・ProvisionalApplied→OFF / OnUnmeasured→OFF (末尾の
        // apply 参照)。測定は open 時の自動スキャンが担う。スキャン機構が windows 限定のため
        // windows でのみ描く。
        #[cfg(windows)]
        {
            use crate::video::normalize_types::NormalizeUiState;
            let norm_ui_state = self
                .normalize_ui_states
                .get(&fs_idx)
                .copied()
                .unwrap_or_default();
            // Norm ボタン幅は動画 native HUD の norm_w (= btn_size) に揃える (Inc 7 ③)。
            let norm_w = bsz;
            let norm_rect = egui::Rect::from_min_size(
                egui::pos2(rx - norm_w, controls_cy - bsz * 0.5),
                egui::vec2(norm_w, bsz),
            );
            rx -= norm_w + 8.0;
            let is_scanning = matches!(norm_ui_state, NormalizeUiState::Scanning);
            let norm_active = matches!(
                norm_ui_state,
                NormalizeUiState::OnApplied { .. } | NormalizeUiState::ProvisionalApplied { .. }
            );
            let norm_unmeasured = matches!(norm_ui_state, NormalizeUiState::OnUnmeasured);
            let norm_tooltip = match norm_ui_state {
                NormalizeUiState::Off => "音量ノーマライズ (-14 LUFS)。クリックで ON".to_string(),
                NormalizeUiState::OnApplied { gain_db } => {
                    format!("音量ノーマライズ ON ({gain_db:+.1}dB / -14 LUFS)。クリックで OFF")
                }
                NormalizeUiState::ProvisionalApplied { gain_db } => {
                    format!("音量ノーマライズ ON (仮 {gain_db:+.1}dB / 確定測定中)。クリックで OFF")
                }
                NormalizeUiState::OnUnmeasured => {
                    "音量ノーマライズ ON (未測定)。クリックで OFF".to_string()
                }
                NormalizeUiState::Scanning => "ノーマライズ中…".to_string(),
            };
            let norm_resp = ui
                .interact(
                    norm_rect,
                    ui.id().with(("music_hud_normalize", fs_idx)),
                    egui::Sense::click(),
                )
                .hover_tip_dark(norm_tooltip);
            draw_overlay_button_bg(
                &painter,
                norm_rect,
                norm_resp.hovered() && !is_scanning,
                norm_active,
            );
            let norm_color = if is_scanning {
                egui::Color32::from_gray(120)
            } else if norm_active {
                egui::Color32::from_rgb(255, 198, 62)
            } else if norm_unmeasured {
                // 半透明 blink (時間ベースで alpha 変動、動画と同じ)。
                let t = ui.ctx().input(|i| i.time);
                let blink = (((t * 2.0).sin() + 1.0) * 0.5) as f32;
                let alpha = (180.0 + blink * 75.0) as u8;
                egui::Color32::from_rgba_unmultiplied(255, 150, 60, alpha)
            } else {
                egui::Color32::from_gray(180)
            };
            painter.text(
                egui::pos2(norm_rect.center().x, text_center_y),
                egui::Align2::CENTER_CENTER,
                "Norm",
                egui::FontId::proportional(11.0),
                norm_color,
            );
            if !is_scanning && norm_resp.clicked() {
                toggle_normalize = true;
            }
            if norm_unmeasured {
                ui.ctx().request_repaint(); // blink アニメーション
            }
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
            .hover_tip_dark(label_with_shortcut(
                if muted {
                    "ミュート解除"
                } else {
                    "ミュート"
                },
                sc_mute.as_deref(),
            ));
        draw_overlay_button_bg(&painter, mute_r, mresp.hovered(), muted);
        draw_overlay_speaker_icon(&painter, mute_r.center(), bsz * 0.46, muted);
        if mresp.clicked() {
            toggle_mute = true;
        }
        // 再生速度: 動画/音楽共有の speed ボタン + プリセット popup (Inc 5c-B2)。
        // 左クリックで popup をトグル、右クリック / ダブルクリックで x1。動画と同じ 11
        // プリセット (`PLAYBACK_SPEED_CHOICES`) / ラベル形式 (`format_playback_speed`) に揃う。
        // 速度ボタン幅は動画 native HUD の speed_w (= btn_size * 1.55) に揃える (Inc 7 ③)。
        let spd_w = bsz * 1.55;
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
            text_center_y,
            speed,
            ui.id().with(("music_hud_speed", fs_idx)),
            ui.id().with(("music_hud_speed_popup", fs_idx)),
            hud_rect.left(),
            hud_rect.width(),
            hud_rect.top(),
            speed_popup_open,
            &mut speed_popup_rect_sink,
        );
        // 時間表示は動画 native HUD に揃える (Inc 7 ③): 速度ボタンの左に time_w=132 の固定
        // スロットを取り、その左端に LEFT_CENTER・14px・白(238) で置く。旧実装は速度ボタンに
        // 右寄せで密着していて、動画 (左寄せ・スロット左端) と再生時間の x がズレていた。
        let time_w = 132.0;
        painter.text(
            egui::pos2(rx - time_w, text_center_y),
            egui::Align2::LEFT_CENTER,
            format!("{} / {}", format_hms(pos), format_hms(dur)),
            egui::FontId::proportional(14.0),
            egui::Color32::from_rgb(238, 238, 238),
        );

        // ── 操作を適用 (self / player の可変借用を分離) ──
        // モーダル表示中は上で描いたホバー/レスポンスを無視し、一切適用しない。
        if !interactive {
            return;
        }
        let ctx = ui.ctx().clone();
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
        // (全 seek 経路を単一化、Codex 設計 P2)。頭出し (seek_start) は seek(0.0) + 即再生を
        // 束ねた `music_seek_start` に寄せ、W キー (VideoSeekStart) と実体を共有する。
        if seek_start {
            self.music_seek_start(fs_idx);
        } else if let Some(s) = seek_to {
            self.music_seek_to(fs_idx, s);
        }
        // ループ / 連続再生の切替 (共有 video_loop_mode / video_continuous_mode)。
        if cycle_loop {
            self.cycle_music_loop_mode(&ctx, fs_idx);
        }
        if cycle_continuous {
            self.cycle_music_continuous_mode(&ctx, fs_idx);
        }
        // 音量ノーマライズ (Norm ボタン、左クリックのみ)。動画と共有のハンドラを呼ぶ
        // (音声も FsCacheEntry::Video なので lookup / gain 適用 / スキャンが同じ経路で動く)。
        // ただし音楽 HUD は右クリックが使えない (背後 FS 右クリックと二重動作) ため、動画の
        // 「右クリックで OFF」救済経路が無い。そこで OnUnmeasured からの左クリックは測定ではなく
        // OFF にする (測定は open 時の自動スキャンで走る。再測定したいときは一度 OFF→ON すれば
        // 未測定判定で再スキャンされる)。それ以外の状態は動画共有ハンドラに委ねる (Codex P2)。
        #[cfg(windows)]
        if toggle_normalize {
            use crate::video::normalize_types::NormalizeUiState;
            let st = self
                .normalize_ui_states
                .get(&fs_idx)
                .copied()
                .unwrap_or_default();
            if matches!(st, NormalizeUiState::OnUnmeasured) {
                self.handle_disable_normalize(&ctx, fs_idx);
            } else {
                self.handle_toggle_normalize(&ctx, fs_idx);
            }
        }

        // 前/次ファイル移動 (動画 HUD の ↑↓ = VideoPrevFile/NextFile と同じ経路)。表示順の
        // 隣接する移動可能アイテム (Audio/Video/画像) へ移動する。ビューを切り替えるので他 intent
        // とは排他 (単一クリック)。
        if let Some(delta) = nav_file {
            self.music_navigate_file(&ctx, fs_idx, delta);
        }
    }

    /// 再生前ノーマライズスキャンがモーダル段階 (= 仮 gain 適用前) で、かつ現在の音楽ビュー
    /// (fs_idx) のスキャンかどうか。true の間は左右パネル / timeline seek / HUD 操作 / FS
    /// ショートカットを抑止し、中央にモーダル進捗を出す。スキャン機構は windows 限定なので
    /// 非 windows では常に false。
    pub(crate) fn music_normalize_modal_active(&self, fs_idx: usize) -> bool {
        #[cfg(windows)]
        {
            self.normalize_scan_is_modal_for_current_player(fs_idx)
        }
        #[cfg(not(windows))]
        {
            let _ = fs_idx;
            false
        }
    }

    /// 音楽ビューの再生前スキャン中に出すモーダル進捗パネル (動画 native の
    /// `draw_native_normalize_progress` の egui 版)。背後の操作は
    /// `music_normalize_modal_active` 経由で別途抑止済みだが、ここでも全面 backdrop で
    /// 入力を吸収し、× / ESC でキャンセルできる。`draw_fs_music_view` の最後 (最前面) で呼ぶ。
    #[cfg(windows)]
    pub(crate) fn draw_music_normalize_modal(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        rect: egui::Rect,
        fs_idx: usize,
    ) {
        use crate::video::normalize_types::NormalizeProgressSnapshot;
        use std::sync::atomic::Ordering;
        if !self.normalize_scan_is_modal_for_current_player(fs_idx) {
            return;
        }
        let progress = self
            .normalize_state
            .as_ref()
            .filter(|s| s.fs_idx == fs_idx)
            .map(|s| NormalizeProgressSnapshot {
                pts_processed_ms: s.progress.pts_processed_ms.load(Ordering::Acquire),
                duration_ms: s.progress.duration_ms.load(Ordering::Acquire),
                indeterminate: s.progress.indeterminate.load(Ordering::Acquire),
            })
            .unwrap_or_default();

        let painter = ui.painter_at(rect);
        // 全面 backdrop: 背後の HUD / timeline / パネルへの click / hover を吸収する。
        let _block = ui.interact(
            rect,
            ui.id().with(("music_normalize_backdrop", fs_idx)),
            egui::Sense::click_and_drag(),
        );
        painter.rect_filled(
            rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 120),
        );

        // 中央パネル
        let panel_w = 420.0_f32.min((rect.width() - 40.0).max(120.0));
        let panel_h = 110.0_f32;
        let panel_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(panel_w, panel_h));
        painter.rect_filled(
            panel_rect,
            10.0,
            egui::Color32::from_rgba_unmultiplied(20, 20, 24, 236),
        );
        painter.text(
            egui::pos2(panel_rect.center().x, panel_rect.min.y + 22.0),
            egui::Align2::CENTER_CENTER,
            "音量ノーマライズ中…",
            egui::FontId::proportional(16.0),
            egui::Color32::from_rgb(238, 238, 238),
        );
        // プログレスバー / スピナー
        let bar_pad_x = 24.0;
        let bar_y = panel_rect.center().y + 6.0;
        let bar_rect = egui::Rect::from_min_max(
            egui::pos2(panel_rect.min.x + bar_pad_x, bar_y - 4.0),
            egui::pos2(panel_rect.max.x - bar_pad_x, bar_y + 4.0),
        );
        painter.rect_filled(bar_rect, 2.0, egui::Color32::from_gray(60));
        if progress.indeterminate || progress.duration_ms == 0 {
            let t = ctx.input(|i| i.time as f32);
            let frac = (t * 0.7).fract().clamp(0.0, 1.0);
            let lo = (frac - 0.18).clamp(0.0, 1.0);
            let hi = (frac + 0.18).clamp(0.0, 1.0);
            let chunk = egui::Rect::from_min_max(
                egui::pos2(bar_rect.min.x + bar_rect.width() * lo, bar_rect.min.y),
                egui::pos2(bar_rect.min.x + bar_rect.width() * hi, bar_rect.max.y),
            );
            painter.rect_filled(chunk, 2.0, egui::Color32::from_rgb(255, 198, 62));
        } else {
            let frac =
                (progress.pts_processed_ms as f32 / progress.duration_ms as f32).clamp(0.0, 1.0);
            let filled = egui::Rect::from_min_max(
                bar_rect.min,
                egui::pos2(bar_rect.min.x + bar_rect.width() * frac, bar_rect.max.y),
            );
            painter.rect_filled(filled, 2.0, egui::Color32::from_rgb(255, 198, 62));
            painter.text(
                egui::pos2(panel_rect.center().x, bar_y + 18.0),
                egui::Align2::CENTER_CENTER,
                format!("{:.0}%", frac * 100.0),
                egui::FontId::proportional(12.0),
                egui::Color32::from_gray(200),
            );
        }
        // スキャン中はプログレス更新のため毎フレーム repaint。
        ctx.request_repaint();
        // キャンセル × (右上)
        let cancel_size = 24.0;
        let cancel_rect = egui::Rect::from_min_size(
            egui::pos2(panel_rect.max.x - cancel_size - 8.0, panel_rect.min.y + 8.0),
            egui::vec2(cancel_size, cancel_size),
        );
        let cancel_resp = ui
            .interact(
                cancel_rect,
                ui.id().with(("music_normalize_cancel", fs_idx)),
                egui::Sense::click(),
            )
            .hover_tip_dark("キャンセル [ESC]");
        let cancel_color = if cancel_resp.hovered() {
            egui::Color32::from_rgb(255, 120, 120)
        } else {
            egui::Color32::from_gray(180)
        };
        painter.text(
            cancel_rect.center(),
            egui::Align2::CENTER_CENTER,
            "\u{00D7}", // U+00D7 multiplication sign (ANSI 安全、glyph lint 通過)
            egui::FontId::proportional(20.0),
            cancel_color,
        );
        // ESC は消費して背後の FS 閉じ経路へ流さない (音声キー入力側でもモーダル中は
        // early-return するが、二重防御で consume する)。
        let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if cancel_resp.clicked() || esc {
            self.handle_cancel_normalize_scan(ctx, fs_idx);
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

    #[test]
    fn marker_target_next_finds_first_after_pos() {
        let starts = [10.0, 30.0, 60.0];
        // 15s から K → 次は 30s
        assert_eq!(
            music_marker_target(&starts, 15.0, true),
            Some(MusicMarkerJump::Marker(30.0))
        );
        // 直前 (epsilon 内) のマーカーでは足踏みしない: 30.0 ちょうどから K → 60s
        assert_eq!(
            music_marker_target(&starts, 30.0, true),
            Some(MusicMarkerJump::Marker(60.0))
        );
        // 末尾以降は no-op
        assert_eq!(music_marker_target(&starts, 60.0, true), None);
        assert_eq!(music_marker_target(&starts, 100.0, true), None);
    }

    #[test]
    fn marker_target_prev_finds_last_before_pos() {
        let starts = [10.0, 30.0, 60.0];
        // 45s から J → 前は 30s
        assert_eq!(
            music_marker_target(&starts, 45.0, false),
            Some(MusicMarkerJump::Marker(30.0))
        );
        // 30.0 ちょうどから J → 足踏みせず 10s
        assert_eq!(
            music_marker_target(&starts, 30.0, false),
            Some(MusicMarkerJump::Marker(10.0))
        );
    }

    #[test]
    fn marker_target_prev_falls_back_to_start() {
        let starts = [10.0, 30.0];
        // 最初のマーカーより手前 (かつ先頭でない) → 先頭へ
        assert_eq!(
            music_marker_target(&starts, 5.0, false),
            Some(MusicMarkerJump::Start)
        );
        // 既に先頭 (許容 0.05 以内) → no-op
        assert_eq!(music_marker_target(&starts, 0.0, false), None);
        assert_eq!(music_marker_target(&starts, 0.02, false), None);
        // ブックマーク皆無 + 先頭でない → 先頭へ
        assert_eq!(
            music_marker_target(&[], 42.0, false),
            Some(MusicMarkerJump::Start)
        );
        // ブックマーク皆無 + 先頭 → no-op
        assert_eq!(music_marker_target(&[], 0.0, false), None);
    }
}
