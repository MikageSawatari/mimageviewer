//! フルスクリーン表示のレンダリング。
//!
//! `App::update()` から呼ばれる `render_fullscreen_viewport()` を実装する。
//! 元は `update()` 内にインラインで書かれていた ~460 行を独立メソッドに切り出したもの。

use eframe::egui;

use crate::app::App;
use crate::fs_animation::FsCacheEntry;
use crate::grid_item::{GridItem, ThumbnailState};
use crate::pdf_loader::PdfPageContentType;
use crate::settings::SpreadMode;
use crate::ui_helpers::{draw_play_icon, format_bytes_small, open_external_player};

// ── 定数 ────────────────────────────────────────────────────────────────

/// メタデータパネルの最大幅
const METADATA_PANEL_WIDTH: f32 = 380.0;
/// ホバー時トップバーの高さ
const TOP_BAR_HEIGHT: f32 = 44.0;
/// ホイール感度（raw_scroll_delta の除数）
const WHEEL_SENSITIVITY: f32 = 30.0;
/// ズーム倍率の下限
const ZOOM_MIN: f32 = 0.1;
/// ズーム倍率の上限
const ZOOM_MAX: f32 = 50.0;
/// ズームが 1.0 とみなせるしきい値
const ZOOM_NEAR_ONE: f32 = 1.001;
/// 回転・パンがゼロとみなせるしきい値
const TRANSFORM_EPSILON: f32 = 0.001;
/// パンがゼロとみなせるしきい値（length_sq）
const PAN_EPSILON_SQ: f32 = 0.25;
/// バー内ボタンのサイズ
const BAR_BUTTON_SIZE: f32 = 32.0;
/// バー内ボタンの上下マージン
const BAR_BUTTON_MARGIN: f32 = 6.0;
/// バー内ボタン間の隙間
const BAR_BUTTON_GAP: f32 = 4.0;
/// チェックマーク円の半径
const CHECKMARK_RADIUS: f32 = 18.0;
/// チェックマーク円のマージン（画面端からの距離）
const CHECKMARK_MARGIN: f32 = 16.0;
/// 見開き表示の区切り線の幅 (px)
const SPREAD_DIVIDER_WIDTH: f32 = 2.0;
/// フィードバックトースト表示時間（秒）
const FEEDBACK_TOAST_DURATION: f32 = 1.2;
/// 境界ヒント（最初/最後の画像に達した案内）の表示時間（秒）
const BOUNDARY_HINT_DURATION: f32 = 2.5;

// ── 見開きペアリング ──────────────────────────────────────────────────────

/// 見開き表示のペア解決結果。
#[derive(Copy, Clone)]
pub(crate) enum SpreadPair {
    /// 単独表示（1ページ表示 / 横長画像 / 表紙 / 末尾余り）
    Single,
    /// 見開き表示: left=画面左に表示するidx, right=画面右に表示するidx
    Double { left: usize, right: usize },
}

impl App {
    /// 分析モードを解除し、関連する状態をリセットする。
    pub(crate) fn reset_analysis_mode(&mut self) {
        self.analysis_mode = false;
        self.analysis_hover_color = None;
        self.analysis_pinned_color = None;
        self.analysis_grayscale = false;
        self.analysis_mosaic_grid = false;
        self.analysis_filter_mag = 0;
        self.analysis_guide_drag = None;
    }

    /// フルスクリーン通常モードのズーム/パンが有効なら返す。
    /// 閾値以下なら None (描画側で無変換パスに流れるよう明示するため)。
    pub(crate) fn fs_zoom_pan(&self) -> Option<(f32, egui::Vec2)> {
        if self.fs_zoom > ZOOM_NEAR_ONE || self.fs_pan.length_sq() > PAN_EPSILON_SQ {
            Some((self.fs_zoom, self.fs_pan))
        } else {
            None
        }
    }

    /// ホイールによるマウス位置固定ズームを適用する。ズームが変化したら true を返す。
    fn apply_wheel_zoom(
        zoom: &mut f32,
        pan: &mut egui::Vec2,
        wheel_y: f32,
        mouse: Option<egui::Pos2>,
        rect_center: egui::Pos2,
    ) -> bool {
        let factor = 1.1_f32.powf(wheel_y / WHEEL_SENSITIVITY);
        let old_zoom = *zoom;
        *zoom = (old_zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        if let Some(mouse) = mouse {
            let center = rect_center + *pan;
            let cx = mouse.x - center.x;
            let cy = mouse.y - center.y;
            let ratio = *zoom / old_zoom;
            pan.x += cx * (1.0 - ratio);
            pan.y += cy * (1.0 - ratio);
        }
        *zoom != old_zoom
    }

    /// 現在のフルスクリーン画像が PDF ページなら、指定ズームで再レンダリングを要求する。
    fn maybe_rerender_pdf(&mut self, zoom: f32) {
        if let Some(idx) = self.fullscreen_idx {
            if matches!(self.items.get(idx), Some(GridItem::PdfPage { .. })) {
                self.request_pdf_rerender(idx, zoom);
            }
        }
    }
}

/// 分析モード時の画像表示領域（パネル分を右側に確保した残り）を返す。
fn analysis_image_rect(full_rect: egui::Rect) -> egui::Rect {
    let panel_w = 360.0_f32.clamp(full_rect.width() * 0.20, full_rect.width() * 0.35);
    egui::Rect::from_min_max(
        full_rect.min,
        egui::pos2(full_rect.max.x - panel_w, full_rect.max.y),
    )
}

/// ナビゲーション可能アイテムのインデックスリストを作成する。
/// `adjacent_navigable_idx` と同じフィルタ条件。
fn build_nav_indices(items: &[GridItem], visible_indices: &[usize]) -> Vec<usize> {
    visible_indices
        .iter()
        .copied()
        .filter(|&i| {
            matches!(
                items.get(i),
                Some(GridItem::Image(_))
                    | Some(GridItem::Video(_))
                    | Some(GridItem::ZipImage { .. })
                    | Some(GridItem::ZipSeparator { .. })
                    | Some(GridItem::PdfPage { .. })
            )
        })
        .collect()
}

/// 指定インデックスの画像が横長（幅>高さ）かを判定する。
/// テクスチャサイズが不明な場合は false（縦長として扱う）。
fn is_landscape(
    idx: usize,
    fs_cache: &std::collections::HashMap<usize, FsCacheEntry>,
    thumbnails: &[ThumbnailState],
) -> bool {
    // フルサイズキャッシュから判定
    if let Some(entry) = fs_cache.get(&idx) {
        match entry {
            FsCacheEntry::Static { tex, .. } => {
                let s = tex.size_vec2();
                return s.x > s.y;
            }
            FsCacheEntry::Animated { frames, .. } => {
                if let Some((tex, _)) = frames.first() {
                    let s = tex.size_vec2();
                    return s.x > s.y;
                }
            }
            FsCacheEntry::Failed => {}
        }
    }
    // サムネイルから判定
    if let Some(ThumbnailState::Loaded { tex, .. }) = thumbnails.get(idx) {
        let s = tex.size_vec2();
        return s.x > s.y;
    }
    false
}

// ── フルスクリーン状態の中間構造体 ──────────────────────────────────────

/// フルスクリーン描画 1 フレーム分の事前計算済み状態。
struct FsFrameState {
    is_video: bool,
    separator_text: Option<String>,
    video_path: Option<std::path::PathBuf>,
    tex: Option<egui::TextureHandle>,
    thumb_tex: Option<egui::TextureHandle>,
    filename: String,
    folder_display: String,
    image_dims: Option<(u32, u32)>,
    image_file_size: Option<u64>,
    is_loading: bool,
    fs_load_failed: bool,
    /// PDF ページのコンテンツ種別 (非 PDF なら None)
    pdf_content_type: Option<PdfPageContentType>,
}

/// フルスクリーンのキー入力結果。
pub(crate) struct FsKeyAction {
    pub(crate) close: bool,
    pub(crate) nav_delta: i32,
    pub(crate) ctrl_nav: Option<i32>,
    /// Home/End などの絶対ジャンプ先 item index
    pub(crate) jump_to: Option<usize>,
}

impl App {
    /// フルスクリーンビューポートを描画し、終了後のナビゲーション処理も行う。
    /// フルスクリーン表示中でなければ何もしない。
    /// フルスクリーンが非アクティブでもビューポートを非表示で維持する。
    /// アプリ起動直後から呼ばれ、初回のフルスクリーン表示時のちらつきを防ぐ。
    pub(crate) fn keep_fullscreen_viewport_alive(&mut self, ctx: &egui::Context) {
        if self.fullscreen_idx.is_some() {
            return; // アクティブなときは render_fullscreen_viewport が担当
        }
        // 非表示でもフルスクリーンサイズを維持する。
        // 1x1 → フルサイズへのリサイズが Visible(true) と同時に発生すると
        // OS のウィンドウマネージャが中間状態を描画してちらつく。
        let fs_builder = self.build_fullscreen_viewport_builder()
            .with_visible(false);
        let fs_id = egui::ViewportId::from_hash_of("fullscreen_viewer");
        ctx.show_viewport_immediate(
            fs_id,
            fs_builder,
            |_ctx, _class| {},
        );
        // ViewportBuilder::with_visible(false) は「initial」可視性しか制御しないため、
        // 一度表示済みのビューポートを隠すには明示的に Visible(false) を送る必要がある。
        // 送信直前に DWM トランジションを無効化して Win11 のフェードアウトを抑止する。
        if self.fs_viewport_shown {
            crate::dwm_transitions::disable_transitions_for_thread_windows();
            ctx.send_viewport_cmd_to(fs_id, egui::ViewportCommand::Visible(false));
            self.fs_viewport_shown = false;
        }
    }

    pub(crate) fn render_fullscreen_viewport(&mut self, ctx: &egui::Context) {
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };

        // ── pending の PDF 再レンダリング結果を取り込む ──
        // show_viewport_immediate 内では &mut self が使えるので、
        // メインの update() を待たずにここで直接 poll する。
        self.poll_prefetch(ctx);

        // ── 状態の事前計算 ──
        self.advance_animation(ctx, fs_idx);
        // 見開きペアを 1 回だけ解決し、以降のフレーム処理で再利用する
        // (resolve_spread_pair は get_nav_indices 内で Vec<usize> をクローンするため、
        //  毎フレーム 3〜4 回呼ばれるのを避ける)
        let spread_pair = self.resolve_spread_pair(fs_idx);
        let is_spread_double = matches!(spread_pair, SpreadPair::Double { .. });
        // 見開きパートナーの事前読み込み + アニメーション進行
        if let SpreadPair::Double { left, right } = spread_pair {
            let partner = if left == fs_idx { right } else { left };
            self.advance_animation(ctx, partner);
            if !self.fs_cache.contains_key(&partner) && !self.fs_pending.contains_key(&partner) {
                self.start_fs_load(partner);
            }
        }
        let state = self.prepare_fullscreen_state(ctx, fs_idx);

        let mut close_fs = false;
        let mut nav_delta: i32 = 0;
        let mut ctrl_nav: Option<i32> = None;
        let mut jump_to: Option<usize> = None;
        // 境界ヒント即時消去のため、フレーム先頭の状態を捕捉する。
        // handle_fs_navigation 実行後に、ヒントが同じ start_time のまま残って
        // いれば「このフレームで再設定されていない」= 打ち切ってよい、と判定する。
        let hint_start_before = self.fs_boundary_hint.map(|(_, t)| t);
        let mut had_user_input_in_frame = false;

        // ── ビューポート構築 ──
        let fs_builder = self.build_fullscreen_viewport_builder();
        let need_show = !self.fs_viewport_shown;

        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("fullscreen_viewer"),
            fs_builder,
            |ctx, _class| {
                // フルスクリーンビューポート内のイベントで IME 状態を更新する
                // (メインビューポートとは別のイベントキューなのでここで呼ぶ必要がある)
                self.update_ime_state(ctx);
                if need_show {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }

                // event consume される前に捕捉 (handle_fs_key_input が矢印等を
                // 消費するとイベントが見えなくなるため)。マウス移動は操作と見なさない。
                if self.fs_boundary_hint.is_some() {
                    had_user_input_in_frame = ctx.input(|i| {
                        i.events.iter().any(|e| matches!(
                            e,
                            egui::Event::Key { pressed: true, .. }
                                | egui::Event::PointerButton { pressed: true, .. }
                                | egui::Event::MouseWheel { .. }
                        ))
                    });
                }

                if ctx.input(|i| i.viewport().close_requested()) {
                    close_fs = true;
                }

                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(egui::Color32::BLACK))
                    .show(ctx, |ui| {
                        let full_rect = ui.max_rect();

                        // ── キー入力 ──
                        let key_action = self.handle_fs_key_input(ctx, fs_idx, is_spread_double);
                        if key_action.close { close_fs = true; }
                        nav_delta = key_action.nav_delta;
                        ctrl_nav = key_action.ctrl_nav;
                        jump_to = key_action.jump_to;
                        // perf: キー起因のナビはここで input_seq を進める
                        if nav_delta != 0 {
                            self.bump_input_seq("fs_key", Some(&format!("delta={nav_delta}")));
                        } else if ctrl_nav.is_some() {
                            self.bump_input_seq("fs_ctrl_nav", None);
                        } else if key_action.close {
                            self.bump_input_seq("fs_close_key", None);
                        }

                        // ── ホイール & クリック ──
                        let (wheel_nav, click_close) = self.handle_fs_wheel_and_click(
                            ui, ctx, full_rect, &state, is_spread_double,
                        );
                        if wheel_nav != 0 { nav_delta = wheel_nav; }
                        if click_close { close_fs = true; }
                        // perf: ホイール/クリック起因のナビ
                        if wheel_nav != 0 {
                            self.bump_input_seq("fs_wheel", Some(&format!("delta={wheel_nav}")));
                        } else if click_close {
                            self.bump_input_seq("fs_close_click", None);
                        }
                        // ホイール/キーで nav_delta が確定済みなら、
                        // ホバーバーのボタンホバーで上書きされないよう保護
                        let nav_locked = nav_delta != 0;

                        // ── 分析/補正モード: 見開き中は無効 ──
                        // 分析モードは画像エリアを左側に制限する（右パネルと重ならないよう）。
                        // 補正モードは左パネルを画像の上にオーバーレイする（画像位置は移動しない）。
                        let analysis_active = self.analysis_mode && !is_spread_double;
                        let adjustment_active = self.adjustment_mode && !is_spread_double;
                        let image_rect = if analysis_active {
                            analysis_image_rect(full_rect)
                        } else {
                            full_rect
                        };

                        // ── 画像 / 動画 / セパレータ描画 ──
                        if let Some(sep) = state.separator_text.as_ref() {
                            Self::draw_fs_separator(ui, image_rect, sep);
                        } else {
                            match spread_pair {
                                SpreadPair::Single => {
                                    let fs_rotation = self.get_rotation(fs_idx);
                                    let zp = if analysis_active {
                                        Some((self.analysis_zoom, self.analysis_pan))
                                    } else {
                                        self.fs_zoom_pan()
                                    };
                                    let free_rot = if analysis_active { 0.0 } else { self.fs_free_rotation };
                                    // 前フレームと異なる (idx, テクスチャ) の最初の描画で paint を emit。
                                    // seq はエントリ自身の `load_seq` を使う (self.input_seq だと
                                    // paint 時点で別操作に更新されていて load→ready→paint の相関が崩れる)。
                                    if crate::perf::is_enabled()
                                        && let Some(tex) = state.tex.as_ref()
                                    {
                                        let cur_id = tex.id();
                                        let prev = self.fs_painted_last;
                                        let is_new = !matches!(
                                            prev,
                                            Some((prev_idx, prev_id, _)) if prev_idx == fs_idx && prev_id == cur_id
                                        );
                                        if is_new {
                                            let key = self.perf_item_key(fs_idx);
                                            let entry_seq = self
                                                .fs_cache
                                                .get(&fs_idx)
                                                .map(|e| e.load_seq())
                                                .unwrap_or(0);
                                            crate::perf::event(
                                                "fs",
                                                "paint",
                                                key.as_deref(),
                                                entry_seq,
                                                &[("idx", serde_json::Value::from(fs_idx))],
                                            );
                                            self.fs_painted_last = Some((fs_idx, cur_id, entry_seq));
                                        }
                                    }
                                    Self::draw_fs_image(
                                        ui, image_rect,
                                        state.tex.as_ref(), state.thumb_tex.as_ref(),
                                        state.is_video, state.fs_load_failed, fs_rotation, zp,
                                        free_rot,
                                    );
                                }
                                SpreadPair::Double { left, right } => {
                                    self.draw_fs_spread(ui, image_rect, left, right);
                                }
                            }
                        }

                        // ── 消しゴムモード: マスク塗り＋オーバーレイ描画 ──
                        if self.erase_mode && !is_spread_double {
                            let zp = self.fs_zoom_pan();
                            self.handle_erase_paint(ctx, image_rect, zp);
                            self.draw_erase_overlay(ui, ctx, image_rect, zp);
                            ctx.request_repaint();
                        }

                        // ── 動画: 再生ボタン + Enter ──
                        if state.is_video {
                            draw_play_icon(ui.painter(), full_rect.center(), 56.0);
                            if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                                if let Some(ref vp) = state.video_path {
                                    open_external_player(vp);
                                }
                            }
                        }

                        // ── チェックマーク ──
                        if self.checked.contains(&fs_idx) {
                            draw_fs_checkmark(ui, full_rect);
                        }

                        // ── 高解像度読込中インジケーター ──
                        let has_any_tex = state.tex.is_some() || state.thumb_tex.is_some();
                        let pdf_rerendering = self.fs_pending.contains_key(&fs_idx);
                        if state.is_loading && has_any_tex && !pdf_rerendering {
                            ui.painter().text(
                                image_rect.min + egui::vec2(16.0, 16.0),
                                egui::Align2::LEFT_TOP,
                                "高解像度 読込中...",
                                egui::FontId::proportional(14.0),
                                egui::Color32::from_rgba_unmultiplied(220, 220, 220, 180),
                            );
                        }

                        // ── PDF 再レンダリング進捗 ──
                        if self.fs_pending.contains_key(&fs_idx) {
                            let label = if matches!(self.items.get(fs_idx), Some(GridItem::PdfPage { .. })) {
                                "PDF 再レンダリング中..."
                            } else {
                                "読込中..."
                            };
                            let font = egui::FontId::proportional(13.0);
                            let pos = egui::pos2(image_rect.min.x + 12.0, image_rect.max.y - 12.0);
                            let galley = ui.painter().layout_no_wrap(
                                label.to_string(), font.clone(), egui::Color32::WHITE,
                            );
                            let text_rect = egui::Align2::LEFT_BOTTOM
                                .anchor_size(pos, galley.size());
                            let bg = text_rect.expand(4.0);
                            ui.painter().rect_filled(
                                bg, 4.0,
                                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200),
                            );
                            ui.painter().galley(text_rect.min, galley, egui::Color32::WHITE);
                        }

                        // ── 分析パネル（分析モード時、見開き中は無効）──
                        if analysis_active {
                            let pixels = match self.fs_cache.get(&fs_idx) {
                                Some(FsCacheEntry::Static { pixels, .. }) => {
                                    Some(std::sync::Arc::clone(pixels))
                                }
                                _ => None,
                            };
                            let close_analysis = self.draw_analysis_panel(
                                ui, ctx, full_rect, image_rect, pixels.as_deref(),
                            );
                            if close_analysis {
                                self.reset_analysis_mode();
                            }
                        } else if adjustment_active {
                            // ── オーバーレイモード: 左パネル + 右パネル 同時表示 ──
                            // 上部ホバーバーと重ならないよう、左パネルは上部バーの下から開始する。
                            let panel_w = crate::ui_adjustment_panel::LEFT_PANEL_WIDTH.min(full_rect.width() * 0.3);
                            let panel_rect = egui::Rect::from_min_max(
                                egui::pos2(full_rect.min.x, full_rect.min.y + TOP_BAR_HEIGHT),
                                egui::pos2(full_rect.min.x + panel_w, full_rect.max.y),
                            );
                            self.draw_adjustment_panel(ui, panel_rect, state.image_dims);
                            // 右側にメタデータパネルも同時表示（show_metadata_panel の状態に関係なく）
                            if !is_spread_double {
                                self.draw_metadata_panel_forced(ui, ctx, full_rect);
                            }
                        } else if !is_spread_double {
                            // ── メタデータパネル（通常モード：TABキー固定 or 右端ホバー）──
                            let right_panel_visible =
                                self.draw_metadata_panel(ui, ctx, full_rect);
                            let _ = right_panel_visible;
                        }

                        // ── ホバーバー ──
                        let mut bar_rotate_cw = false;
                        let mut bar_rotate_ccw = false;
                        let spread_before = self.spread_mode;
                        // AI 処理情報を計算（ホバーバーのファイル情報に表示）
                        let ai_info_model_name: String;
                        let ai_upscale_info = if self.ai_upscale_enabled || self.ai_denoise_model.is_some() {
                            ai_info_model_name = self.ai_model_label(fs_idx, false);
                            // 処理後のサイズ
                            if let Some(crate::fs_animation::FsCacheEntry::Static { tex, .. }) =
                                self.ai_upscale_cache.get(&fs_idx)
                            {
                                let s = tex.size_vec2();
                                Some((ai_info_model_name.as_str(), s.x as u32, s.y as u32))
                            } else if self.ai_upscale_enabled {
                                if let Some((w, h)) = state.image_dims {
                                    if crate::ai::upscale::should_process(w, h, self.settings.ai_upscale_skip_px) {
                                        Some((ai_info_model_name.as_str(), w * 4, h * 4))
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        // 消しゴムモード中は上部バーを抑制 (自前の消しゴムパネルと競合させない)。
                        if !self.erase_mode {
                            let saved_nav = nav_delta;
                            let has_page_override = self.adjustment_page_params.contains_key(&fs_idx);
                            Self::draw_fs_hover_bar(
                                ui, ctx, full_rect,
                                &state.folder_display, &state.filename,
                                state.image_dims, state.image_file_size,
                                &mut close_fs, &mut nav_delta,
                                &mut self.show_metadata_panel,
                                false,
                                &mut self.slideshow_playing,
                                &mut self.settings.slideshow_interval_secs,
                                &mut bar_rotate_cw, &mut bar_rotate_ccw,
                                &mut self.analysis_mode,
                                &mut self.spread_mode, &mut self.spread_popup_open,
                                is_spread_double,
                                ai_upscale_info,
                                &mut self.adjustment_mode,
                                has_page_override,
                                state.pdf_content_type,
                            );
                            // ホイール/キーで確定した nav_delta を保護
                            if nav_locked { nav_delta = saved_nav; }
                        }
                        if bar_rotate_cw { self.rotate_image_cw(fs_idx); }
                        if bar_rotate_ccw { self.rotate_image_ccw(fs_idx); }

                        // ── フルスクリーン用コンテキストメニュー ──
                        if self.show_fs_context_menu(ctx) {
                            close_fs = true;
                        }

                        // ── フルスクリーン左下ステータス表示 ──
                        if self.ai_upscale_enabled || self.ai_denoise_model.is_some() {
                            self.draw_fs_ai_status(ui, fs_idx);
                        }

                        // ── 右上フィードバックトースト ──
                        self.draw_feedback_toast(ui, full_rect, ctx);

                        // ── 中央の境界ヒント (最初/最後の画像です…) ──
                        self.draw_boundary_hint(ui, full_rect, ctx);

                        // ── スロット保存ダイアログ ──
                        self.draw_slot_save_dialog(ctx);

                        // ホバーバーのポップアップからモードが変更された場合
                        if self.spread_mode != spread_before {
                            if let (Some(db), Some(folder)) = (&self.spread_db, &self.current_folder) {
                                let _ = db.set(folder, self.spread_mode, self.settings.default_spread_mode);
                            }
                            if self.spread_mode.is_spread() && self.analysis_mode {
                                self.reset_analysis_mode();
                            }
                            self.normalize_spread_position();
                        }
                    });
            },
        );

        self.fs_viewport_shown = true;

        // ── ナビゲーション & スライドショー処理 ──
        self.handle_fs_navigation(ctx, close_fs, ctrl_nav, nav_delta, jump_to, fs_idx);

        // hint_start_before と一致 = このフレームで再設定されていない
        // (= 境界でない方向への移動、別キー入力、等)。操作があれば即消去。
        // 再設定されていた場合 (= 引き続き境界に突き当たった) はそのまま残す。
        if had_user_input_in_frame {
            let hint_now = self.fs_boundary_hint.map(|(_, t)| t);
            if hint_now.is_some() && hint_now == hint_start_before {
                self.fs_boundary_hint = None;
            }
        }
        self.handle_fs_repaint(ctx, fs_idx, state.is_video);
    }

    // ── 状態準備ヘルパー ────────────────────────────────────────────────

    /// アニメーションフレームを進める（メインコンテキストの時刻を使う）。
    fn advance_animation(&mut self, ctx: &egui::Context, fs_idx: usize) {
        let is_video = matches!(self.items.get(fs_idx), Some(GridItem::Video(_)));
        if is_video { return; }
        let now = ctx.input(|i| i.time);
        if let Some(FsCacheEntry::Animated {
            frames, current_frame, next_frame_at, ..
        }) = self.fs_cache.get_mut(&fs_idx)
        {
            if now >= *next_frame_at && !frames.is_empty() {
                *current_frame = (*current_frame + 1) % frames.len();
                let delay = frames[*current_frame].1.max(0.02);
                *next_frame_at = now + delay;
            }
        }
    }

    /// フルスクリーン描画に必要な状態を事前計算する。
    fn prepare_fullscreen_state(&self, _ctx: &egui::Context, fs_idx: usize) -> FsFrameState {
        let is_video = matches!(self.items.get(fs_idx), Some(GridItem::Video(_)));
        let separator_text = match self.items.get(fs_idx) {
            Some(GridItem::ZipSeparator { dir_display }) => Some(dir_display.clone()),
            _ => None,
        };
        let is_separator = separator_text.is_some();
        let video_path = if let Some(GridItem::Video(p)) = self.items.get(fs_idx) {
            Some(p.clone())
        } else {
            None
        };

        let tex: Option<egui::TextureHandle> = if is_video {
            None
        } else {
            // 補正済みキャッシュ（フル解像度）
            let adj_tex = match self.adjustment_cache.get(&fs_idx) {
                Some(FsCacheEntry::Static { tex, .. }) => Some(tex.clone()),
                _ => None,
            };

            // AI 処理有効時（アップスケール or デノイズ）: 処理済みテクスチャ
            let ai_tex = if adj_tex.is_none() && (self.ai_upscale_enabled || self.ai_denoise_model.is_some()) {
                match self.ai_upscale_cache.get(&fs_idx) {
                    Some(FsCacheEntry::Static { tex, .. }) => Some(tex.clone()),
                    _ => None,
                }
            } else {
                None
            };

            adj_tex
                .or(ai_tex)
                .or_else(|| {
                    match self.fs_cache.get(&fs_idx) {
                        Some(FsCacheEntry::Static { tex, .. }) => Some(tex.clone()),
                        Some(FsCacheEntry::Animated { frames, current_frame, .. }) => {
                            frames.get(*current_frame).map(|(h, _)| h.clone())
                        }
                        Some(FsCacheEntry::Failed) | None => None,
                    }
                })
        };

        let fs_load_failed = matches!(self.fs_cache.get(&fs_idx), Some(FsCacheEntry::Failed));

        let thumb_tex = match self.thumbnails.get(fs_idx) {
            Some(ThumbnailState::Loaded { tex, .. }) => Some(tex.clone()),
            _ => None,
        };

        let filename = self.items.get(fs_idx)
            .map(|item| item.name().to_string())
            .unwrap_or_default();
        let folder_display = self.current_folder.as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        // image_dims は常に元画像のサイズを表示する（AI アップスケール後のサイズではない）。
        // AI テクスチャが選ばれている場合でも、元画像のサイズを使う。
        let image_dims: Option<(u32, u32)> = {
            // まず元画像のキャッシュからサイズを取得
            let original_dims = match self.fs_cache.get(&fs_idx) {
                Some(FsCacheEntry::Static { tex, .. }) => {
                    let s = tex.size_vec2();
                    Some((s.x as u32, s.y as u32))
                }
                Some(FsCacheEntry::Animated { frames, current_frame, .. }) => {
                    frames.get(*current_frame).map(|(h, _)| {
                        let s = h.size_vec2();
                        (s.x as u32, s.y as u32)
                    })
                }
                _ => None,
            };
            // 元画像がまだロードされていなければ、表示中のテクスチャから取得
            original_dims.or_else(|| tex.as_ref().map(|t| {
                let s = t.size_vec2();
                (s.x as u32, s.y as u32)
            }))
        };
        let image_file_size: Option<u64> = self.image_metas.get(fs_idx)
            .and_then(|m| m.map(|(_, sz)| sz.max(0) as u64));
        let is_loading =
            !is_video && !is_separator && !fs_load_failed && !self.fs_cache.contains_key(&fs_idx);

        let pdf_content_type = match self.items.get(fs_idx) {
            Some(GridItem::PdfPage { content_type, .. }) => *content_type,
            _ => None,
        };

        FsFrameState {
            is_video, separator_text, video_path, tex, thumb_tex,
            filename, folder_display, image_dims, image_file_size,
            is_loading, fs_load_failed, pdf_content_type,
        }
    }

    /// フルスクリーンビューポートの ViewportBuilder を構築する。
    fn build_fullscreen_viewport_builder(&self) -> egui::ViewportBuilder {
        let center = self.last_outer_rect.map(|r| r.center());
        let ppp = self.last_pixels_per_point;

        let monitor_rect = center.and_then(|c| {
            crate::monitor::get_monitor_logical_rect_at(c.x * ppp, c.y * ppp)
        });

        let b = egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_transparent(true)
            .with_taskbar(false);
        match monitor_rect {
            Some(rect) => {
                b.with_position(rect.min)
                    .with_inner_size([rect.width(), rect.height()])
            }
            None => {
                b.with_fullscreen(true)
            }
        }
    }

    // ── 見開きペアリング ────────────────────────────────────────────────

    /// build_nav_indices の結果をキャッシュして返す。
    fn get_nav_indices(&mut self) -> Vec<usize> {
        if let Some(ref cached) = self.cached_nav_indices {
            return cached.clone();
        }
        let nav = build_nav_indices(&self.items, &self.visible_indices);
        self.cached_nav_indices = Some(nav.clone());
        nav
    }

    /// 現在の見開きモードとインデックスからペア表示を解決する。
    pub(crate) fn resolve_spread_pair(&mut self, idx: usize) -> SpreadPair {
        if !self.spread_mode.is_spread() {
            return SpreadPair::Single;
        }

        let nav = self.get_nav_indices();
        let Some(pos) = nav.iter().position(|&i| i == idx) else {
            return SpreadPair::Single;
        };

        // 表紙モード: pos=0 は常に単独
        if self.spread_mode.has_cover() && pos == 0 {
            return SpreadPair::Single;
        }

        // 横長画像は単独
        if is_landscape(idx, &self.fs_cache, &self.thumbnails) {
            return SpreadPair::Single;
        }

        // ペアリング開始位置: 表紙ありなら pos=1 から、なしなら pos=0 から
        let pair_start = if self.spread_mode.has_cover() { 1 } else { 0 };

        // ペア内の位置を計算 (0-indexed from pair_start)
        let relative = pos - pair_start;
        let is_first_of_pair = relative % 2 == 0;

        // ペア相手の pos を決定
        let partner_pos = if is_first_of_pair {
            pos + 1
        } else {
            pos - 1
        };

        // パートナーが存在しない or 横長の場合は単独
        let partner_idx = match nav.get(partner_pos) {
            Some(&pidx) => pidx,
            None => return SpreadPair::Single,
        };
        if is_landscape(partner_idx, &self.fs_cache, &self.thumbnails) {
            return SpreadPair::Single;
        }

        // 小さい pos のインデックスと大きい pos のインデックス
        let (small_idx, large_idx) = if is_first_of_pair {
            (idx, partner_idx)
        } else {
            (partner_idx, idx)
        };

        // LTR: 左=小, 右=大  /  RTL: 左=大, 右=小
        if self.spread_mode.is_rtl() {
            SpreadPair::Double { left: large_idx, right: small_idx }
        } else {
            SpreadPair::Double { left: small_idx, right: large_idx }
        }
    }

    /// 見開きモードでの nav_delta を計算する。
    /// 見開き表示中は2ページ送り、Single表示中は1ページ送り。
    /// Shift が押されている場合は常に1ページ送り。
    pub(crate) fn spread_nav_delta(&mut self, base_delta: i32, shift_held: bool) -> i32 {
        if !self.spread_mode.is_spread() || shift_held {
            return base_delta;
        }
        let fs_idx = match self.fullscreen_idx {
            Some(i) => i,
            None => return base_delta,
        };
        // 現在の表示が Single（横長等）なら1ページ送り
        match self.resolve_spread_pair(fs_idx) {
            SpreadPair::Single => base_delta,
            SpreadPair::Double { .. } => base_delta * 2,
        }
    }

    /// 見開きモード切替後、fullscreen_idx をペアの先頭に正規化する。
    pub(crate) fn normalize_spread_position(&mut self) {
        if !self.spread_mode.is_spread() {
            return;
        }
        let Some(idx) = self.fullscreen_idx else { return };
        let nav = self.get_nav_indices();
        let Some(pos) = nav.iter().position(|&i| i == idx) else { return };

        let pair_start = if self.spread_mode.has_cover() { 1 } else { 0 };
        if pos < pair_start {
            return; // 表紙位置
        }
        let relative = pos - pair_start;
        if relative % 2 != 0 {
            // ペアの2番目にいるので1番目に戻す
            let new_idx = nav[pos - 1];
            self.open_fullscreen(new_idx);
            self.selected = Some(new_idx);
        }
    }

    // ── キー入力 ────────────────────────────────────────────────────────

    /// フルスクリーンのキー入力を処理し、アクションを返す。
    fn handle_fs_key_input(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        is_spread_double: bool,
    ) -> FsKeyAction {
        let has_focus = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        let mut action = FsKeyAction { close: false, nav_delta: 0, ctrl_nav: None, jump_to: None };

        if !has_focus { return action; }
        // モーダルダイアログ表示中はキー入力を奪わない
        // (テキスト入力やダイアログ内の Enter/Esc 処理を優先)
        if self.any_dialog_open() { return action; }

        // 消しゴムモード中は専用ショートカットのみ有効にし、通常のフルスクリーンショートカット
        // (矢印ナビ、R/L 回転、I メタデータ等) を無効化する。
        if self.erase_mode {
            return self.handle_erase_keys(ctx, fs_idx);
        }

        // ナビゲーションキーは input_mut で消費して、パネル内ウィジェット（スライダー等）に
        // 奪われないようにする
        let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let shift_held = ctx.input(|i| i.modifiers.shift);
        // 左右キーは上下と分離して処理（RTL 反転のため）
        // Shift+矢印（スプレッドナビ）にも対応するため、修飾キーを問わず消費
        let ctrl_d = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowDown));
        let ctrl_u = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowUp));
        let arrow_right = ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)
            || i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowRight)
        });
        let arrow_left = ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)
            || i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowLeft)
        });
        let arrow_down = ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
            || i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowDown)
        });
        let arrow_up = ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
            || i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowUp)
        });
        let key_home = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Home));
        let key_end = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::End));
        let key_i = ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::NONE, egui::Key::I)
            || i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
        });
        let key_s = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space));
        let key_r = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::R));
        let key_l = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::L));
        let key_z = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Z));
        let key_g = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::G));
        let key_m = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::M));
        let key_e = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::E));
        let key_p = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::P));

        // F1-F5: レーティング 1〜5 / F6: レーティング解除
        let rating_key: Option<u8> = ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::F1) { Some(1) }
            else if i.consume_key(egui::Modifiers::NONE, egui::Key::F2) { Some(2) }
            else if i.consume_key(egui::Modifiers::NONE, egui::Key::F3) { Some(3) }
            else if i.consume_key(egui::Modifiers::NONE, egui::Key::F4) { Some(4) }
            else if i.consume_key(egui::Modifiers::NONE, egui::Key::F5) { Some(5) }
            else if i.consume_key(egui::Modifiers::NONE, egui::Key::F6) { Some(0) }
            else { None }
        });
        if let Some(stars) = rating_key {
            self.set_rating(fs_idx, stars);
            if stars == 0 {
                self.show_feedback_toast("[★解除]".to_string());
            } else {
                self.show_feedback_toast(format!("[{}]", "★".repeat(stars as usize)));
            }
        }

        // F7/F8: マスクスロット 1/2 をフルスクリーン表示のまま現ページに適用
        // (消しゴムモードに入らず、1 キーで inpaint までを一気に実行)
        let key_f7 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F7));
        let key_f8 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F8));
        if key_f7 {
            self.apply_slot_in_viewing_mode(ctx, 1);
        }
        if key_f8 {
            self.apply_slot_in_viewing_mode(ctx, 2);
        }

        // 見開きモード切替 (1-5 キー)
        let key_1 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Num1));
        let key_2 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Num2));
        let key_3 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Num3));
        let key_4 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Num4));
        let key_5 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Num5));

        // U キー: AI アップスケールサイクル
        let key_u = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::U));
        // N キー: AI デノイズサイクル
        let key_n = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::N));

        // Ctrl+数字キー: 保存スロットからロード
        // (Shift+数字はキー配列によって記号化され egui::Key::Num1 等にマッチしないため CTRL を採用)
        let slot_keys: [bool; 10] = [
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num1)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num2)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num3)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num4)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num5)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num6)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num7)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num8)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num9)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num0)),
        ];

        // Ctrl+Backspace: 現在ページの個別補正設定を解除 (標準値に戻す)
        let clear_page_key = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Backspace));

        // 見開きモード切替 + フィードバック表示
        let new_spread = if key_1 { Some(SpreadMode::Single) }
            else if key_2 { Some(SpreadMode::Ltr) }
            else if key_3 { Some(SpreadMode::LtrCover) }
            else if key_4 { Some(SpreadMode::Rtl) }
            else if key_5 { Some(SpreadMode::RtlCover) }
            else { None };

        if let Some(mode) = new_spread {
            if mode != self.spread_mode {
                self.spread_mode = mode;
                self.spread_popup_open = false;
                // DB に保存
                if let (Some(db), Some(folder)) = (&self.spread_db, &self.current_folder) {
                    let _ = db.set(folder, mode, self.settings.default_spread_mode);
                }
                // 分析モードを解除
                if mode.is_spread() && self.analysis_mode {
                    self.analysis_mode = false;
                    self.analysis_hover_color = None;
                    self.analysis_pinned_color = None;
                }
                // ページ位置を正規化
                self.normalize_spread_position();
            }
            // フィードバック表示
            let key_num = if key_1 { 1 } else if key_2 { 2 } else if key_3 { 3 } else if key_4 { 4 } else { 5 };
            self.show_feedback_toast(format!("[{}:{}]", key_num, mode.label()));
        }

        // U キー: AI アップスケールモデルをサイクル (現在ページの有効パラメータに対して)
        if key_u {
            let mut params = self.effective_params(fs_idx).clone();
            let items = crate::adjustment::upscale_menu_items();
            let cur = items.iter().position(|(_, k)| {
                match (k, params.upscale_model.as_deref()) {
                    (None, None) => true,
                    (Some(a), Some(b)) => *a == b,
                    _ => false,
                }
            }).unwrap_or(0);
            let next = (cur + 1) % items.len();
            let (label, key) = items[next];
            params.upscale_model = key.map(|s| s.to_string());
            self.show_feedback_toast(format!("[U:アップスケール {}]", label));
            self.set_page_params(fs_idx, params);
            self.clear_all_adjustment_and_ai_caches(fs_idx);
        }

        // N キー: AI デノイズをトグル (現在ページの有効パラメータに対して)
        if key_n {
            let mut params = self.effective_params(fs_idx).clone();
            if params.denoise_model.is_some() {
                params.denoise_model = None;
                self.show_feedback_toast("[N:デノイズ OFF]".to_string());
            } else {
                params.denoise_model = Some(crate::ai::ModelKind::DenoiseRealplksr.as_str().to_string());
                self.show_feedback_toast("[N:デノイズ ON]".to_string());
            }
            self.set_page_params(fs_idx, params);
            self.clear_all_adjustment_and_ai_caches(fs_idx);
        }

        // Ctrl+数字キー: 保存スロットを現在ページに適用 (= ページ個別化)
        for (slot_idx, &pressed) in slot_keys.iter().enumerate() {
            if pressed {
                self.apply_slot_to_current_page(slot_idx);
            }
        }

        // Ctrl+Backspace: 個別設定があれば解除、なければフィードバックのみ
        if clear_page_key {
            if self.adjustment_page_params.contains_key(&fs_idx) {
                self.clear_page_params(fs_idx);
                self.show_feedback_toast("[個別設定を解除]".to_string());
            } else {
                self.show_feedback_toast("[個別設定なし]".to_string());
            }
        }

        if esc { action.close = true; }
        // 見開きダブル表示中は I/Z/R/L を無効化
        if key_i && !is_spread_double {
            self.show_metadata_panel = !self.show_metadata_panel;
        }
        if key_z && !is_spread_double {
            if self.analysis_mode {
                // 分析→通常: ズーム/パンを引き継ぐ
                self.fs_zoom = self.analysis_zoom;
                self.fs_pan = self.analysis_pan;
                self.reset_analysis_mode();
            } else {
                // 通常→分析: ズーム/パンを引き継ぐ
                self.analysis_zoom = self.fs_zoom;
                self.analysis_pan = self.fs_pan;
                self.analysis_mode = true;
                // 補正パネルと排他
                self.adjustment_mode = false;
            }
        }
        if self.analysis_mode && !is_spread_double {
            if key_g { self.analysis_grayscale = !self.analysis_grayscale; }
            if key_m {
                self.analysis_mosaic_grid = !self.analysis_mosaic_grid;
                if self.analysis_mosaic_grid {
                    self.analysis_guide_drag = None;
                }
            }
        }

        // E: 消しゴムモード切り替え（見開き・分析・補正中は無効）
        if key_e && !is_spread_double && !self.analysis_mode && !self.adjustment_mode {
            if self.erase_mode {
                // 2回目のE: inpaint実行
                self.execute_erase_inpaint(ctx, fs_idx);
            } else {
                // 1回目のE: マスクモード開始
                self.enter_erase_mode(fs_idx);
            }
        }

        // P: スライドショー開始/停止トグル
        // 開始時のみ、現在ページが画像系アイテム (Image/ZipImage/PdfPage) かを確認する。
        // ZipSeparator など非画像アイテム上では開始させない (停止操作は常に許可)。
        if key_p {
            if self.slideshow_playing {
                self.slideshow_playing = false;
            } else if matches!(
                self.items.get(fs_idx),
                Some(GridItem::Image(_)) | Some(GridItem::ZipImage { .. }) | Some(GridItem::PdfPage { .. })
            ) {
                self.slideshow_playing = true;
                self.slideshow_next_at = std::time::Instant::now()
                    + std::time::Duration::from_secs_f32(self.settings.slideshow_interval_secs);
            }
        }

        // Space: スライドショー中→停止、停止中→画像をチェック
        if key_s {
            if self.slideshow_playing {
                self.slideshow_playing = false;
            } else {
                match self.items.get(fs_idx) {
                    Some(GridItem::Image(_))
                    | Some(GridItem::Video(_))
                    | Some(GridItem::ZipImage { .. })
                    | Some(GridItem::PdfPage { .. }) => {
                        if self.checked.contains(&fs_idx) {
                            self.checked.remove(&fs_idx);
                        } else {
                            self.checked.insert(fs_idx);
                        }
                    }
                    _ => {}
                }
            }
        }
        if key_r && !is_spread_double { self.rotate_image_cw(fs_idx); }
        if key_l && !is_spread_double { self.rotate_image_ccw(fs_idx); }

        // ── ナビゲーション ──
        // RTL モードでは左右キーの意味を反転
        let rtl = self.spread_mode.is_rtl();
        let nav_next = (arrow_right && !rtl) || (arrow_left && rtl) || arrow_down;
        let nav_prev = (arrow_left && !rtl) || (arrow_right && rtl) || arrow_up;

        if nav_next && !ctrl_d {
            action.nav_delta = self.spread_nav_delta(1, shift_held);
            self.slideshow_playing = false;
        }
        if nav_prev && !ctrl_u {
            action.nav_delta = self.spread_nav_delta(-1, shift_held);
            self.slideshow_playing = false;
        }
        if ctrl_d { action.ctrl_nav = Some(1); }
        if ctrl_u { action.ctrl_nav = Some(-1); }

        if key_home {
            if let Some(first) = crate::ui_helpers::boundary_navigable_idx(
                &self.items, &self.visible_indices, false,
            ) {
                if first != fs_idx {
                    action.jump_to = Some(first);
                    self.slideshow_playing = false;
                } else {
                    self.fs_boundary_hint = Some((false, std::time::Instant::now()));
                }
            }
        }
        if key_end {
            if let Some(last) = crate::ui_helpers::boundary_navigable_idx(
                &self.items, &self.visible_indices, true,
            ) {
                if last != fs_idx {
                    action.jump_to = Some(last);
                    self.slideshow_playing = false;
                } else {
                    self.fs_boundary_hint = Some((true, std::time::Instant::now()));
                }
            }
        }

        action
    }

    // ── ホイール & クリック ──────────────────────────────────────────────

    /// ホイールとクリックを処理し、(nav_delta, close) を返す。
    fn handle_fs_wheel_and_click(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        state: &FsFrameState,
        is_spread_double: bool,
    ) -> (i32, bool) {
        let mut nav_delta = 0i32;
        let mut close = false;

        // ── ホイール ──
        // パネル領域内ではホイールナビゲーションを抑制
        let panel_w = METADATA_PANEL_WIDTH.min(full_rect.width() * 0.5);
        let panel_left = full_rect.max.x - panel_w;
        let hover_threshold = full_rect.max.x - full_rect.width() * 0.25;
        let has_right_panel = self.show_metadata_panel;
        let left_panel_w = crate::ui_adjustment_panel::LEFT_PANEL_WIDTH.min(full_rect.width() * 0.3);
        let cursor_in_panel = ctx.input(|i| {
            i.pointer.hover_pos().map(|p| {
                let in_right = p.x > panel_left
                    && p.y >= 60.0
                    && (has_right_panel || p.x > hover_threshold);
                let in_left = self.adjustment_mode
                    && p.x < full_rect.min.x + left_panel_w
                    && p.y >= 60.0;
                in_right || in_left
            }).unwrap_or(false)
        });

        // 左端・上端・右端のホバーでオーバーレイ（上バー＋左パネル＋右パネル）を同時表示/非表示
        // 消しゴムモード中は自前のパネルを左端に描いているためエッジ発火を抑制する。
        // 加えて、消しゴムモードに入る前から adjustment_mode が立っていると、消しゴムパネルが
        // 左端を占有している間 edge_hover が常に true 扱いになり off へ遷移できないので、
        // 強制的に落とす。
        if self.erase_mode {
            self.adjustment_mode = false;
        } else {
            let edge_hover = ctx.input(|i| {
                i.pointer.hover_pos().map(|p| {
                    p.y < 60.0  // 上端
                    || p.x < full_rect.min.x + full_rect.width() * 0.05  // 左端5%
                    || p.x > full_rect.max.x - full_rect.width() * 0.05  // 右端5%
                }).unwrap_or(false)
            });
            if edge_hover && !self.analysis_mode {
                self.adjustment_mode = true;
            } else if !cursor_in_panel && !edge_hover && self.adjustment_mode && !self.adjustment_dragging {
                self.adjustment_mode = false;
            }
        }

        let wheel_y = ctx.input(|i| i.raw_scroll_delta.y);
        if wheel_y.abs() > 0.5 && !cursor_in_panel {
            ctx.input_mut(|i| {
                i.raw_scroll_delta = egui::Vec2::ZERO;
                i.smooth_scroll_delta = egui::Vec2::ZERO;
                i.events.retain(|e| !matches!(e, egui::Event::MouseWheel { .. }));
            });
            // 消しゴムモード: 筆/直線ツールでは修飾なしホイールで太さ調整
            // (Ctrl+ホイールは通常のズームに残す)
            let ctrl_held = ctx.input(|i| i.modifiers.ctrl);
            if !ctrl_held
                && self.erase_mode
                && matches!(self.erase_tool, crate::app::EraseTool::Brush | crate::app::EraseTool::Line)
            {
                let max_r = self.erase_mask_size[0].max(self.erase_mask_size[1]) as f32 / 20.0;
                let factor = if wheel_y > 0.0 { 1.1 } else { 1.0 / 1.1 };
                match self.erase_tool {
                    crate::app::EraseTool::Brush => {
                        self.erase_brush_radius = (self.erase_brush_radius * factor).clamp(1.0, max_r);
                    }
                    crate::app::EraseTool::Line => {
                        self.erase_line_width = (self.erase_line_width * factor).clamp(1.0, max_r);
                    }
                    _ => {}
                }
                return (0, false); // ホイールを消費したので終了
            }
            if self.analysis_mode {
                // 分析モード: ホイールでズーム
                let mouse = ctx.input(|i| i.pointer.hover_pos());
                let image_rect = analysis_image_rect(full_rect);
                let changed = Self::apply_wheel_zoom(
                    &mut self.analysis_zoom, &mut self.analysis_pan,
                    wheel_y, mouse, image_rect.center(),
                );
                if changed { self.maybe_rerender_pdf(self.analysis_zoom); }
            } else {
                let ctrl_held = ctx.input(|i| i.modifiers.ctrl);
                if ctrl_held {
                    // 通常モード: Ctrl+ホイールでズーム
                    let mouse = ctx.input(|i| i.pointer.hover_pos());
                    let changed = Self::apply_wheel_zoom(
                        &mut self.fs_zoom, &mut self.fs_pan,
                        wheel_y, mouse, full_rect.center(),
                    );
                    if changed { self.maybe_rerender_pdf(self.fs_zoom); }
                } else if !self.erase_mode {
                    let base = if wheel_y < 0.0 { 1 } else { -1 };
                    nav_delta = self.spread_nav_delta(base, false);
                }
            }
        }

        // ── クリック & ドラッグ ──
        let fs_response = ui.interact(
            full_rect,
            egui::Id::new("fs_click"),
            egui::Sense::click_and_drag(),
        );
        if self.erase_mode {
            // 消しゴムモード: 左クリック/ドラッグはマスク塗りに使うためナビ無効化
        } else if self.analysis_mode {
            // 分析モード: 左クリックでのナビを無効化（パン用のドラッグは analysis_panel 側）
            // ダブルクリックでズームリセット
            if fs_response.double_clicked() {
                self.analysis_zoom = 1.0;
                self.analysis_pan = egui::Vec2::ZERO;
                self.maybe_rerender_pdf(1.0);
            }
            // 右クリックは analysis_panel 側で処理
        } else {
            // ── 通常モード: ドラッグ操作 ──
            let mods = ctx.input(|i| i.modifiers);
            let primary_pressed = fs_response.drag_started_by(egui::PointerButton::Primary);
            let primary_down = fs_response.dragged_by(egui::PointerButton::Primary);
            let primary_released = fs_response.drag_stopped_by(egui::PointerButton::Primary);
            let pointer_pos = ctx.input(|i| i.pointer.hover_pos());

            // 見開き 2 ページ表示中はフリー回転が描画に反映されないため、Ctrl+ドラッグ回転を無効化する
            if mods.ctrl && !is_spread_double {
                // Ctrl+ドラッグ → 回転
                if primary_pressed {
                    if let Some(pos) = pointer_pos {
                        self.fs_rotation_drag_start = Some((pos, self.fs_free_rotation));
                    }
                } else if primary_down {
                    if let Some((start_pos, start_rot)) = self.fs_rotation_drag_start {
                        if let Some(pos) = pointer_pos {
                            let center = full_rect.center() + self.fs_pan;
                            let start_angle = (start_pos.y - center.y).atan2(start_pos.x - center.x);
                            let cur_angle = (pos.y - center.y).atan2(pos.x - center.x);
                            self.fs_free_rotation = start_rot + (cur_angle - start_angle);
                        }
                    }
                }
            } else if self.fs_zoom > ZOOM_NEAR_ONE || self.fs_free_rotation.abs() > TRANSFORM_EPSILON {
                // ズームまたは回転中: ドラッグでパン
                if primary_pressed {
                    if let Some(pos) = pointer_pos {
                        self.fs_pan_drag_start = Some((pos, self.fs_pan));
                    }
                } else if primary_down {
                    if let Some((start_pos, start_pan)) = self.fs_pan_drag_start {
                        if let Some(pos) = pointer_pos {
                            self.fs_pan = start_pan + (pos - start_pos);
                        }
                    }
                }
            }
            if primary_released {
                self.fs_pan_drag_start = None;
                self.fs_rotation_drag_start = None;
            }

            // ダブルクリック → ズーム/パン/回転リセット
            let has_transform = self.fs_zoom > ZOOM_NEAR_ONE
                || self.fs_free_rotation.abs() > TRANSFORM_EPSILON
                || self.fs_pan.length_sq() > PAN_EPSILON_SQ;
            if fs_response.double_clicked() && has_transform {
                self.fs_zoom = 1.0;
                self.fs_pan = egui::Vec2::ZERO;
                self.fs_free_rotation = 0.0;
                self.maybe_rerender_pdf(1.0);
            } else if !has_transform && self.fs_context_menu_idx.is_none() {
                // 変形なし: 従来の動画/画像クリック動作（コンテキストメニュー表示中は無効）
                let was_dragging = fs_response.dragged() && fs_response.drag_delta().length() > 3.0;
                if !was_dragging {
                    if state.is_video {
                        if fs_response.clicked() {
                            if let Some(ref vp) = state.video_path {
                                open_external_player(vp);
                            }
                        }
                    } else if fs_response.clicked() {
                        // ポップアップ表示中はクリックでのページ送りを抑制
                        let any_popup = self.spread_popup_open;
                        if !any_popup {
                            if let Some(pos) = fs_response.interact_pointer_pos() {
                                let panel_threshold = full_rect.max.x - full_rect.width() * 0.25;
                                let in_right_panel = pos.y >= 60.0
                                    && (self.show_metadata_panel || pos.x > panel_threshold)
                                    && pos.x > full_rect.max.x - METADATA_PANEL_WIDTH.min(full_rect.width() * 0.5);
                                let in_left_panel = self.adjustment_mode
                                    && pos.x < full_rect.min.x + crate::ui_adjustment_panel::LEFT_PANEL_WIDTH.min(full_rect.width() * 0.3)
                                    && pos.y >= 60.0;
                                if !in_right_panel && !in_left_panel {
                                    let base = if pos.x > full_rect.center().x { 1 } else { -1 };
                                    nav_delta = self.spread_nav_delta(base, false);
                                }
                            }
                        }
                    }
                }
            }
        }
        // 分析モード中は右クリックを色固定に使うため、終了トリガーにしない
        // コンテキストメニュー表示中は右クリック処理をスキップ
        if !self.analysis_mode && self.fs_context_menu_idx.is_none() {
            let secondary_down = ctx.input(|i| i.pointer.secondary_down());
            let secondary_released = ctx.input(|i| i.pointer.secondary_released());

            if secondary_down && self.fs_secondary_press_start.is_none() {
                // 押下開始を記録
                let pos = ctx.input(|i| i.pointer.interact_pos().unwrap_or_default());
                self.fs_secondary_press_start = Some((std::time::Instant::now(), pos));
            }

            if let Some((start_time, start_pos)) = self.fs_secondary_press_start {
                let elapsed = start_time.elapsed();
                let current_pos = ctx.input(|i| {
                    i.pointer.interact_pos().unwrap_or(start_pos)
                });
                let moved = current_pos.distance(start_pos);

                if !secondary_released && elapsed >= std::time::Duration::from_millis(400) && moved < 20.0 {
                    // 長押ししきい値超過 → 押下中にコンテキストメニューを即表示
                    self.fs_context_menu_idx = self.fullscreen_idx;
                    self.fs_context_menu_pos = current_pos;
                    self.fs_secondary_press_start = None;
                } else if secondary_released {
                    if moved < 20.0 && elapsed < std::time::Duration::from_millis(400) {
                        // 短押し → 従来通り閉じる
                        close = true;
                    }
                    self.fs_secondary_press_start = None;
                } else if moved >= 20.0 {
                    // マウスが動きすぎた → キャンセル
                    self.fs_secondary_press_start = None;
                }
            }
        }

        (nav_delta, close)
    }

    // ── ナビゲーション & スライドショー ─────────────────────────────────

    /// フルスクリーン終了・ナビゲーション・スライドショーを処理する。
    fn handle_fs_navigation(
        &mut self,
        ctx: &egui::Context,
        close_fs: bool,
        ctrl_nav: Option<i32>,
        nav_delta: i32,
        jump_to: Option<usize>,
        fs_idx: usize,
    ) {
        if close_fs {
            self.close_fullscreen();
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        // Ctrl+↑↓ はフルスクリーンを保ったまま前後フォルダへ飛び、先頭/末尾の
        // 画像系アイテムを開く。self.selected も合わせて更新するので、ここから
        // フルスクリーンを閉じたときグリッド側のカーソルが最後に観た画像に残る。
        //
        // 実装上: `navigate_folder_with_skip` は DFS + `read_dir` で UI スレッドを
        // ブロックし得るので (深い階層だと 100ms 級)、ここでは発火だけ行い、
        // 実際の close_fullscreen / load_folder / open_fullscreen は
        // `apply_folder_nav_result` (FolderNavMode::Fullscreen ブランチ) に任せる。
        if let Some(delta) = ctrl_nav {
            if let Some(cur) = self.current_folder.clone() {
                let forward = delta > 0;
                self.start_folder_nav(cur, forward, crate::app::FolderNavMode::Fullscreen);
            }
        } else if !close_fs {
            if let Some(new_idx) = jump_to {
                self.open_fullscreen(new_idx);
                self.selected = Some(new_idx);
                self.scroll_to_selected = true;
                self.update_last_selected_image();
            } else if nav_delta != 0 {
                if let Some(new_idx) = crate::ui_helpers::adjacent_navigable_idx(
                    &self.items, &self.visible_indices, fs_idx, nav_delta,
                ) {
                    self.open_fullscreen(new_idx);
                    self.selected = Some(new_idx);
                    self.scroll_to_selected = true;
                    self.update_last_selected_image();
                } else {
                    // 境界到達: 中央にヒントを出す (nav_delta > 0 なら末尾)
                    self.fs_boundary_hint = Some((nav_delta > 0, std::time::Instant::now()));
                    crate::logger::log(format!(
                        "[NAV] adjacent_navigable_idx returned None: fs_idx={fs_idx}, delta={nav_delta}, items={}, visible={}",
                        self.items.len(), self.visible_indices.len()
                    ));
                }
            }
        }

        // ── スライドショー タイマー ──
        if self.slideshow_playing && !close_fs {
            let now = std::time::Instant::now();
            if now >= self.slideshow_next_at {
                if let Some(cur) = self.fullscreen_idx {
                    let slide_delta = self.spread_nav_delta(1, false);
                    let next = crate::ui_helpers::adjacent_navigable_idx(
                        &self.items, &self.visible_indices, cur, slide_delta,
                    );
                    // 末尾到達時は先頭の画像系アイテムへループ。
                    // 画像系がひとつも無い場合はスライドショーを停止 (安全側、
                    // 旧実装の `unwrap_or(0)` で非画像アイテムへ飛ぶ事故を防ぐ)。
                    let target = next.or_else(|| {
                        self.visible_indices.iter().copied()
                            .find(|&i| matches!(
                                self.items.get(i),
                                Some(GridItem::Image(_)) | Some(GridItem::ZipImage { .. }) | Some(GridItem::PdfPage { .. })
                            ))
                    });
                    match target {
                        Some(idx) => {
                            self.open_fullscreen(idx);
                            self.selected = Some(idx);
                            self.scroll_to_selected = true;
                        }
                        None => {
                            self.slideshow_playing = false;
                        }
                    }
                }
                self.slideshow_next_at = now
                    + std::time::Duration::from_secs_f32(self.settings.slideshow_interval_secs);
            }
            let remaining = self.slideshow_next_at.saturating_duration_since(now);
            ctx.request_repaint_after(remaining);
        }
    }

    /// フルスクリーンの再描画リクエストを管理する。
    fn handle_fs_repaint(&self, ctx: &egui::Context, fs_idx: usize, is_video: bool) {
        // 高解像度読み込み完了まで、またはPDF再レンダリング中は毎フレーム再描画
        let image_loading = !is_video
            && self.fullscreen_idx
                .map(|i| !self.fs_cache.contains_key(&i))
                .unwrap_or(false);
        let pdf_rerendering = self.fs_pending.contains_key(&fs_idx);
        if image_loading || pdf_rerendering {
            ctx.request_repaint();
        }

        // 右クリック長押し検出中: しきい値チェックのため再描画をリクエスト
        if let Some((start_time, _)) = self.fs_secondary_press_start {
            let remaining = std::time::Duration::from_millis(400)
                .saturating_sub(start_time.elapsed());
            if remaining.is_zero() {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(remaining);
            }
        }

        // アニメーション: 次フレームの時刻まで待ってから再描画
        if !is_video {
            if let Some(FsCacheEntry::Animated { next_frame_at, .. }) = self.fs_cache.get(&fs_idx) {
                let delay = (next_frame_at - ctx.input(|i| i.time)).max(0.0);
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay));
            }
        }
    }

    // ── フルスクリーン描画ヘルパー ──────────────────────────────────────

    /// ZIP セパレータの章タイトル画面を描画する。
    fn draw_fs_separator(ui: &mut egui::Ui, full_rect: egui::Rect, sep: &str) {
        let title_size = (full_rect.height() * 0.12).clamp(48.0, 120.0);
        let sub_size = (full_rect.height() * 0.030).clamp(20.0, 36.0);

        ui.painter().rect_filled(
            egui::Rect::from_center_size(
                full_rect.center(),
                egui::vec2(full_rect.width() * 0.85, title_size * 2.2),
            ),
            16.0,
            egui::Color32::from_rgba_unmultiplied(30, 45, 80, 180),
        );
        ui.painter().text(
            full_rect.center(),
            egui::Align2::CENTER_CENTER,
            sep,
            egui::FontId::proportional(title_size),
            egui::Color32::WHITE,
        );
        ui.painter().text(
            egui::pos2(full_rect.center().x, full_rect.max.y - 48.0),
            egui::Align2::CENTER_BOTTOM,
            "── 作品の区切り ──",
            egui::FontId::proportional(sub_size),
            egui::Color32::from_rgb(150, 180, 220),
        );
    }

    /// フルスクリーンの画像 / 動画 / 読込中 / 失敗 表示を描画する。
    /// zoom/pan が Some のとき分析モードのズーム/パンを適用する。
    fn draw_fs_image(
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        tex: Option<&egui::TextureHandle>,
        thumb_tex: Option<&egui::TextureHandle>,
        is_video: bool,
        fs_load_failed: bool,
        rotation: crate::rotation_db::Rotation,
        zoom_pan: Option<(f32, egui::Vec2)>,
        free_rotation_rad: f32,
    ) {
        let display_tex = tex.or(thumb_tex);
        if let Some(handle) = display_tex {
            let tex_size = handle.size_vec2();
            let display_size = match rotation {
                crate::rotation_db::Rotation::Cw90
                | crate::rotation_db::Rotation::Cw270 => egui::vec2(tex_size.y, tex_size.x),
                _ => tex_size,
            };
            let fit_scale =
                (full_rect.width() / display_size.x).min(full_rect.height() / display_size.y);
            let (total_scale, center) = match zoom_pan {
                Some((zoom, pan)) => (fit_scale * zoom, full_rect.center() + pan),
                None => (fit_scale, full_rect.center()),
            };
            let img_rect = egui::Rect::from_center_size(center, display_size * total_scale);
            let needs_clip = zoom_pan.is_some() || free_rotation_rad.abs() > TRANSFORM_EPSILON;
            let painter = if needs_clip {
                ui.painter().with_clip_rect(full_rect)
            } else {
                ui.painter().clone()
            };
            if rotation.is_none() && free_rotation_rad.abs() <= TRANSFORM_EPSILON {
                painter.image(
                    handle.id(), img_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                crate::app::draw_rotated_image_ex(
                    &painter, handle.id(), img_rect, rotation,
                    free_rotation_rad, center,
                );
            }
        } else if fs_load_failed {
            ui.painter().text(
                full_rect.center(), egui::Align2::CENTER_CENTER,
                "読込失敗", egui::FontId::proportional(32.0),
                egui::Color32::from_rgb(255, 140, 140),
            );
            ui.painter().text(
                full_rect.center() + egui::vec2(0.0, 40.0), egui::Align2::CENTER_CENTER,
                "このファイルはデコードできませんでした",
                egui::FontId::proportional(16.0), egui::Color32::from_gray(180),
            );
        } else {
            ui.painter().text(
                full_rect.center(), egui::Align2::CENTER_CENTER,
                if is_video { "動画サムネイル 読込中..." } else { "読込中..." },
                egui::FontId::proportional(24.0), egui::Color32::from_gray(180),
            );
        }
    }

    /// 見開きモードの2ページ描画。
    /// 2枚の画像を隙間なく中央に配置し、境界に薄い黒線を描画する。
    fn draw_fs_spread(
        &mut self,
        ui: &mut egui::Ui,
        image_rect: egui::Rect,
        left_idx: usize,
        right_idx: usize,
    ) {
        let zoom_pan = self.fs_zoom_pan();
        let left_rot = self.get_rotation(left_idx);
        let right_rot = self.get_rotation(right_idx);

        // 各ページの表示サイズを計算して、全体をフィットさせる
        // 片方だけフルサイズだとアスペクト比の微小差でレイアウトがジャンプするため、
        // 両方フルサイズが揃うまではサムネイルサイズに統一する
        let both_in_fs_cache = self.fs_cache.contains_key(&left_idx)
            && self.fs_cache.contains_key(&right_idx);
        let (left_size, right_size) = if both_in_fs_cache {
            (
                Self::get_display_size(left_idx, left_rot, &self.fs_cache, &self.thumbnails),
                Self::get_display_size(right_idx, right_rot, &self.fs_cache, &self.thumbnails),
            )
        } else {
            // サムネイルのみ使用（fs_cache を空マップとして渡す）
            let empty = std::collections::HashMap::new();
            (
                Self::get_display_size(left_idx, left_rot, &empty, &self.thumbnails),
                Self::get_display_size(right_idx, right_rot, &empty, &self.thumbnails),
            )
        };

        // ズーム/パンが有効な場合は image_rect でクリップする
        // (ズーム時にページが image_rect 外へはみ出して他の UI を覆わないようにするため)
        let painter = if zoom_pan.is_some() {
            ui.painter().with_clip_rect(image_rect)
        } else {
            ui.painter().clone()
        };

        if let (Some(ls), Some(rs)) = (left_size, right_size) {
            // 両ページの高さを揃える（高い方に合わせる）
            let combined_h = ls.y.max(rs.y);
            let left_w = ls.x * (combined_h / ls.y);
            let right_w = rs.x * (combined_h / rs.y);

            let combined_w = left_w + right_w;

            // 画面にフィットするスケール
            let fit_scale = (image_rect.width() / combined_w)
                .min(image_rect.height() / combined_h);

            let (total_scale, center) = match zoom_pan {
                Some((zoom, pan)) => (fit_scale * zoom, image_rect.center() + pan),
                None => (fit_scale, image_rect.center()),
            };

            let scaled_lw = left_w * total_scale;
            let scaled_rw = right_w * total_scale;
            let scaled_h = combined_h * total_scale;

            // 全体を中央に配置
            let total_w = scaled_lw + scaled_rw;
            let start_x = center.x - total_w * 0.5;
            let start_y = center.y - scaled_h * 0.5;

            let left_rect = egui::Rect::from_min_size(
                egui::pos2(start_x, start_y),
                egui::vec2(scaled_lw, scaled_h),
            );
            let right_rect = egui::Rect::from_min_size(
                egui::pos2(start_x + scaled_lw, start_y),
                egui::vec2(scaled_rw, scaled_h),
            );

            Self::draw_fs_spread_page(&painter, left_rect, left_idx, left_rot, &self.fs_cache, &self.thumbnails);
            Self::draw_fs_spread_page(&painter, right_rect, right_idx, right_rot, &self.fs_cache, &self.thumbnails);

            // 区切り線（2px 黒線）
            let divider_x = start_x + scaled_lw;
            painter.line_segment(
                [
                    egui::pos2(divider_x, start_y),
                    egui::pos2(divider_x, start_y + scaled_h),
                ],
                egui::Stroke::new(SPREAD_DIVIDER_WIDTH, egui::Color32::BLACK),
            );
        } else {
            // サイズ不明の場合は均等分割フォールバック
            // (ズーム/パンはサイズが分かってからでないと正しく計算できないため適用しない)
            let half_w = image_rect.width() / 2.0;
            let left_rect = egui::Rect::from_min_size(
                image_rect.min,
                egui::vec2(half_w, image_rect.height()),
            );
            let right_rect = egui::Rect::from_min_size(
                egui::pos2(image_rect.min.x + half_w, image_rect.min.y),
                egui::vec2(half_w, image_rect.height()),
            );
            Self::draw_fs_spread_page(&painter, left_rect, left_idx, left_rot, &self.fs_cache, &self.thumbnails);
            Self::draw_fs_spread_page(&painter, right_rect, right_idx, right_rot, &self.fs_cache, &self.thumbnails);
        }
    }

    /// テクスチャの表示サイズ（回転考慮）を返す。テクスチャ未取得なら None。
    fn get_display_size(
        idx: usize,
        rotation: crate::rotation_db::Rotation,
        fs_cache: &std::collections::HashMap<usize, FsCacheEntry>,
        thumbnails: &[ThumbnailState],
    ) -> Option<egui::Vec2> {
        let tex = match fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { tex, .. }) => Some(tex.size_vec2()),
            Some(FsCacheEntry::Animated { frames, current_frame, .. }) => {
                frames.get(*current_frame).map(|(h, _)| h.size_vec2())
            }
            _ => None,
        };
        let size = tex.or_else(|| {
            if let Some(ThumbnailState::Loaded { tex, .. }) = thumbnails.get(idx) {
                Some(tex.size_vec2())
            } else {
                None
            }
        })?;
        Some(match rotation {
            crate::rotation_db::Rotation::Cw90
            | crate::rotation_db::Rotation::Cw270 => egui::vec2(size.y, size.x),
            _ => size,
        })
    }

    /// 見開きモードの1ページ分を指定領域に描画。
    /// `painter` は呼び出し側でクリップ済みのものを渡すことで、ズーム時のはみ出しを防ぐ。
    fn draw_fs_spread_page(
        painter: &egui::Painter,
        rect: egui::Rect,
        idx: usize,
        rotation: crate::rotation_db::Rotation,
        fs_cache: &std::collections::HashMap<usize, FsCacheEntry>,
        thumbnails: &[ThumbnailState],
    ) {
        // テクスチャ取得（フルサイズ or サムネイル）
        let tex = match fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { tex, .. }) => Some(tex.clone()),
            Some(FsCacheEntry::Animated { frames, current_frame, .. }) => {
                frames.get(*current_frame).map(|(h, _)| h.clone())
            }
            _ => None,
        };
        let thumb_tex = match thumbnails.get(idx) {
            Some(ThumbnailState::Loaded { tex, .. }) => Some(tex.clone()),
            _ => None,
        };
        let display_tex = tex.as_ref().or(thumb_tex.as_ref());

        if let Some(handle) = display_tex {
            let tex_size = handle.size_vec2();
            let display_size = match rotation {
                crate::rotation_db::Rotation::Cw90
                | crate::rotation_db::Rotation::Cw270 => egui::vec2(tex_size.y, tex_size.x),
                _ => tex_size,
            };
            let fit_scale =
                (rect.width() / display_size.x).min(rect.height() / display_size.y);
            let img_rect = egui::Rect::from_center_size(
                rect.center(),
                display_size * fit_scale,
            );
            if rotation.is_none() {
                painter.image(
                    handle.id(), img_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                crate::app::draw_rotated_image(painter, handle.id(), img_rect, rotation);
            }
        } else {
            // 読込中
            painter.text(
                rect.center(), egui::Align2::CENTER_CENTER,
                "読込中...",
                egui::FontId::proportional(18.0), egui::Color32::from_gray(150),
            );
        }
    }

    /// フルスクリーンのホバー時トップバーを描画する。
    #[allow(clippy::too_many_arguments)]
    fn draw_fs_hover_bar(
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        folder_display: &str,
        filename: &str,
        image_dims: Option<(u32, u32)>,
        image_file_size: Option<u64>,
        close_fs: &mut bool,
        nav_delta: &mut i32,
        show_info: &mut bool,
        force_show: bool,
        slideshow_playing: &mut bool,
        _slideshow_interval: &mut f32,
        rotate_cw: &mut bool,
        rotate_ccw: &mut bool,
        show_analysis: &mut bool,
        spread_mode: &mut SpreadMode,
        spread_popup_open: &mut bool,
        is_spread_double: bool,
        // AI アップスケール後のサイズとモデル名（表示用）
        ai_upscale_info: Option<(&str, u32, u32)>,
        // 画像補正パネル表示トグル
        adjustment_mode: &mut bool,
        // 現在ページに個別補正が適用されているか (ボタン点灯用)
        has_page_override: bool,
        // PDF ページのコンテンツ種別 (非 PDF なら None)
        pdf_content_type: Option<PdfPageContentType>,
    ) {
        let hover_in_top = ctx
            .input(|i| i.pointer.hover_pos().map(|p| p.y < 60.0).unwrap_or(false));
        // adjustment_mode がオンならオーバーレイとして常に表示
        if !hover_in_top && !force_show && !*spread_popup_open && !*adjustment_mode {
            return;
        }

        let bar_rect = egui::Rect::from_min_size(
            full_rect.min,
            egui::vec2(full_rect.width(), TOP_BAR_HEIGHT),
        );
        ui.painter().rect_filled(
            bar_rect, 0.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200),
        );
        ui.painter().line_segment(
            [
                egui::pos2(bar_rect.min.x, bar_rect.max.y),
                egui::pos2(bar_rect.max.x, bar_rect.max.y),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 60)),
        );

        // ── ボタン群（右端から左に並べる）──
        let mut next_x = bar_rect.max.x - BAR_BUTTON_SIZE - BAR_BUTTON_MARGIN;

        // × 閉じるボタン
        let close_resp = draw_bar_button(
            ui, next_x, bar_rect.min.y + BAR_BUTTON_MARGIN,
            "fs_close_btn",
            |hovered| if hovered {
                egui::Color32::from_rgba_unmultiplied(220, 50, 50, 230)
            } else {
                egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
            },
            false, // active 状態なし
            |p, c, r| draw_close_icon(p, c, r),
        );
        let close_resp = close_resp.on_hover_text("閉じる [Esc]");
        if close_resp.clicked() { *close_fs = true; }
        if close_resp.hovered() { *nav_delta = 0; }
        next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;

        // ▶/⏸ スライドショーボタン
        let play_resp = draw_bar_button(
            ui, next_x, bar_rect.min.y + BAR_BUTTON_MARGIN,
            "fs_play_btn",
            |hovered| if *slideshow_playing {
                egui::Color32::from_rgba_unmultiplied(60, 180, 60, 200)
            } else if hovered {
                egui::Color32::from_rgba_unmultiplied(100, 100, 100, 200)
            } else {
                egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
            },
            false,
            |p, c, r| {
                if *slideshow_playing {
                    draw_pause_icon(p, c, r);
                } else {
                    draw_play_triangle(p, c, r);
                }
            },
        );
        let play_resp = if *slideshow_playing {
            play_resp.on_hover_text("スライドショー停止")
        } else {
            play_resp.on_hover_text("スライドショー")
        };
        if play_resp.clicked() { *slideshow_playing = !*slideshow_playing; }
        if play_resp.hovered() { *nav_delta = 0; }
        next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;

        // ↷ 右回転ボタン
        let rcw_resp = draw_bar_button(
            ui, next_x, bar_rect.min.y + BAR_BUTTON_MARGIN,
            "fs_rcw_btn",
            |hovered| bar_button_bg(hovered, false),
            false,
            |p, c, r| draw_rotate_icon(p, c, r, true),
        );
        let rcw_resp = rcw_resp.on_hover_text("右回転 [R]");
        if rcw_resp.clicked() { *rotate_cw = true; }
        if rcw_resp.hovered() { *nav_delta = 0; }
        next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;

        // ↶ 左回転ボタン
        let rccw_resp = draw_bar_button(
            ui, next_x, bar_rect.min.y + BAR_BUTTON_MARGIN,
            "fs_rccw_btn",
            |hovered| bar_button_bg(hovered, false),
            false,
            |p, c, r| draw_rotate_icon(p, c, r, false),
        );
        let rccw_resp = rccw_resp.on_hover_text("左回転 [L]");
        if rccw_resp.clicked() { *rotate_ccw = true; }
        if rccw_resp.hovered() { *nav_delta = 0; }
        next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;

        // ℹ Info ボタン
        let info_resp = draw_bar_button(
            ui, next_x, bar_rect.min.y + BAR_BUTTON_MARGIN,
            "fs_info_btn",
            |hovered| bar_button_bg(hovered, *show_info),
            *show_info,
            |p, c, r| draw_info_icon(p, c, r),
        );
        let info_resp = info_resp.on_hover_text("メタデータ [I / Tab]");
        if info_resp.clicked() { *show_info = !*show_info; }
        if info_resp.hovered() { *nav_delta = 0; }
        next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;

        // 🔬 分析ボタン（見開きダブル中は非表示）
        if !is_spread_double {
            let analysis_resp = draw_bar_button(
                ui, next_x, bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_analysis_btn",
                |hovered| bar_button_bg(hovered, *show_analysis),
                *show_analysis,
                |p, c, r| draw_analysis_icon(p, c, r),
            );
            let analysis_resp = analysis_resp.on_hover_text("分析ツール [Z]");
            if analysis_resp.clicked() { *show_analysis = !*show_analysis; }
            if analysis_resp.hovered() { *nav_delta = 0; }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // 📖 見開きモードボタン (クリックでポップアップ)
        let spread_active = spread_mode.is_spread();
        let sm = *spread_mode;
        let spread_resp = draw_bar_button(
            ui, next_x, bar_rect.min.y + BAR_BUTTON_MARGIN,
            "fs_spread_btn",
            |hovered| bar_button_bg(hovered, spread_active),
            spread_active,
            |p, c, r| draw_spread_icon(p, c, r, sm),
        );
        let spread_resp = spread_resp.on_hover_text("見開き設定 [5-9]");
        if spread_resp.clicked() {
            *spread_popup_open = !*spread_popup_open;
        }
        if spread_resp.hovered() { *nav_delta = 0; }

        // 見開きポップアップ
        if *spread_popup_open {
            let popup_x = next_x;
            let popup_y = bar_rect.max.y + 4.0;
            let popup_w = 200.0_f32;
            let popup_h = 5.0 * 36.0 + 8.0; // 5 items + padding
            let popup_rect = egui::Rect::from_min_size(
                egui::pos2(popup_x, popup_y),
                egui::vec2(popup_w, popup_h),
            );

            // 背景
            ui.painter().rect_filled(
                popup_rect, 6.0,
                egui::Color32::from_rgba_unmultiplied(30, 30, 30, 240),
            );
            ui.painter().rect_stroke(
                popup_rect, 6.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(100, 100, 100, 180)),
                egui::StrokeKind::Outside,
            );

            let mut item_y = popup_rect.min.y + 4.0;
            for &mode in SpreadMode::all() {
                let item_rect = egui::Rect::from_min_size(
                    egui::pos2(popup_rect.min.x + 4.0, item_y),
                    egui::vec2(popup_w - 8.0, 32.0),
                );
                let item_resp = ui.interact(
                    item_rect,
                    egui::Id::new(format!("spread_popup_{}", mode.to_int())),
                    egui::Sense::click(),
                );
                let is_current = *spread_mode == mode;
                let bg = if is_current {
                    egui::Color32::from_rgba_unmultiplied(80, 140, 220, 200)
                } else if item_resp.hovered() {
                    egui::Color32::from_rgba_unmultiplied(80, 80, 80, 200)
                } else {
                    egui::Color32::TRANSPARENT
                };
                ui.painter().rect_filled(item_rect, 4.0, bg);

                // アイコン (左側)
                let icon_center = egui::pos2(item_rect.min.x + 20.0, item_rect.center().y);
                draw_spread_icon(ui.painter(), icon_center, 7.0, mode);

                // ラベル (右側)
                ui.painter().text(
                    egui::pos2(item_rect.min.x + 44.0, item_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    mode.label(),
                    egui::FontId::proportional(13.0),
                    egui::Color32::from_gray(220),
                );

                let shortcut_label = match mode.to_int() {
                    0 => "[5]", 1 => "[6]", 2 => "[7]", 3 => "[8]", _ => "[9]",
                };
                ui.painter().text(
                    egui::pos2(item_rect.max.x - 8.0, item_rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    shortcut_label,
                    egui::FontId::proportional(11.0),
                    egui::Color32::from_gray(140),
                );

                if item_resp.clicked() {
                    *spread_mode = mode;
                    *spread_popup_open = false;
                }
                item_y += 36.0;
            }

            // ポップアップ外クリックで閉じる
            let pointer_pos = ctx.input(|i| i.pointer.press_origin());
            if let Some(pos) = pointer_pos {
                if !popup_rect.contains(pos) && !spread_resp.rect.contains(pos) {
                    if ctx.input(|i| i.pointer.any_pressed()) {
                        *spread_popup_open = false;
                    }
                }
            }
        }

        next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;

        // 🎨 画像補正パネルトグルボタン
        {
            let btn_rect = egui::Rect::from_min_size(
                egui::pos2(next_x, bar_rect.min.y + BAR_BUTTON_MARGIN),
                egui::vec2(BAR_BUTTON_SIZE, BAR_BUTTON_SIZE),
            );
            let resp = ui.interact(
                btn_rect,
                egui::Id::new("fs_adjust_btn"),
                egui::Sense::click(),
            );
            let bg = if *adjustment_mode {
                egui::Color32::from_rgba_unmultiplied(80, 140, 220, 220)
            } else if has_page_override {
                // 個別設定が効いているときは薄い警告色でヒント
                egui::Color32::from_rgba_unmultiplied(120, 100, 60, 200)
            } else if resp.hovered() {
                egui::Color32::from_rgba_unmultiplied(100, 100, 100, 200)
            } else {
                egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
            };
            ui.painter().rect_filled(btn_rect, 4.0, bg);
            ui.painter().text(
                btn_rect.center(),
                egui::Align2::CENTER_CENTER,
                "🎨",
                egui::FontId::proportional(16.0),
                egui::Color32::WHITE,
            );
            let tooltip = if has_page_override {
                "画像補正 (このページは個別設定あり)"
            } else {
                "画像補正"
            };
            let resp = resp.on_hover_text(tooltip);
            if resp.clicked() { *adjustment_mode = !*adjustment_mode; }
            if resp.hovered() { *nav_delta = 0; }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // ── 左側: フォルダパス ──
        if !folder_display.is_empty() {
            ui.painter().text(
                egui::pos2(bar_rect.min.x + 12.0, bar_rect.center().y),
                egui::Align2::LEFT_CENTER,
                folder_display,
                egui::FontId::proportional(13.0),
                egui::Color32::from_gray(180),
            );
        }

        // ── ファイル情報テキスト ──
        draw_fs_bar_info_text(
            ui, bar_rect,
            egui::pos2(next_x - 12.0, bar_rect.center().y),
            filename, image_dims, image_file_size,
            ai_upscale_info, pdf_content_type,
        );
    }
}

// ── フルスクリーン AI ステータスオーバーレイ ────────────────────────────

impl App {
    /// 現在有効な AI 処理のモデル名を結合して返す。
    /// `show_auto_prefix` が true の場合、自動選択時に「自動: 」プレフィックスを付ける。
    fn ai_model_label(&self, fs_idx: usize, show_auto_prefix: bool) -> String {
        let mut labels = Vec::new();
        if let Some(denoise_kind) = self.ai_denoise_model {
            labels.push(denoise_kind.display_label().to_string());
        }
        if self.ai_upscale_enabled {
            let upscale_label = match self.ai_upscale_model_override {
                Some(k) => k.display_label().to_string(),
                None => self.ai_classify_cache.get(&fs_idx)
                    .map(|c| if show_auto_prefix {
                        format!("自動: {}", c.display_label())
                    } else {
                        c.display_label().to_string()
                    })
                    .unwrap_or_else(|| "自動".to_string()),
            };
            labels.push(upscale_label);
        }
        labels.join(" + ")
    }

    /// フルスクリーン左下に AI 処理ステータスを表示する。
    fn draw_fs_ai_status(&mut self, ui: &mut egui::Ui, fs_idx: usize) {
        let is_upscaling = self.ai_upscale_pending.contains_key(&fs_idx);
        let is_upscaled = self.ai_upscale_cache.contains_key(&fs_idx);
        let is_loading = self.fs_pending.contains_key(&fs_idx);
        let any_busy = is_loading || is_upscaling || !self.ai_upscale_pending.is_empty();

        let mut lines: Vec<(String, egui::Color32)> = Vec::new();

        if is_loading {
            lines.push(("読込中...".to_string(), egui::Color32::from_gray(210)));
        }

        if is_upscaling {
            let label = self.ai_model_label(fs_idx, true);
            lines.push((format!("AI 処理中 ({})", label), egui::Color32::from_rgb(255, 200, 80)));
        } else if is_upscaled {
            let label = self.ai_model_label(fs_idx, false);
            lines.push((format!("AI 処理完了 ({})", label), egui::Color32::from_rgb(80, 220, 80)));
        }

        if self.erase_base_cache.contains_key(&fs_idx) && !self.erase_mode {
            lines.push(("消去補完済み".to_string(), egui::Color32::from_rgb(180, 140, 255)));
        }

        // AI 機能が完全に無効なら先読みバーを出さない。
        // 以前は AI off でも target があれば「0/N」バーが表示されて
        // 進捗が進まないように見える UX 不具合があった。
        let ai_feature_active = self.ai_upscale_enabled || self.ai_denoise_model.is_some();

        let prefetch_progress: Option<(usize, usize)> = if is_upscaling || !ai_feature_active {
            None
        } else {
            let targets = self.ai_prefetch_targets(fs_idx);
            let total = targets.len();
            if total == 0 {
                None
            } else {
                // 「done」の判定: cache 済み / failed / サイズ閾値で skip 確定。
                // 高解像度スキャン (2048px 超等) は maybe_start_ai_upscale で
                // should_process に弾かれて AI が走らないが、従来は cache にも
                // failed にも入らないため「0/N」バーが永久に残った。
                // ここでサイズを見て「この画像は AI 対象外」と判別できるものは done
                // 扱いにする。fs_cache に Static が無いものはまだ判定不能なので undone。
                let upscale_px = self.settings.ai_upscale_skip_px;
                let denoise_px = self.settings.ai_denoise_skip_px;
                let upscale_enabled = self.ai_upscale_enabled;
                let denoise_enabled = self.ai_denoise_model.is_some();
                let done = targets.iter()
                    .filter(|&&i| {
                        if self.ai_upscale_cache.contains_key(&i)
                            || self.ai_upscale_failed.contains(&i)
                        {
                            return true;
                        }
                        // fs_cache の dims でサイズ閾値判定
                        if let Some(FsCacheEntry::Static { pixels, .. }) = self.fs_cache.get(&i) {
                            let w = pixels.size[0] as u32;
                            let h = pixels.size[1] as u32;
                            let upscale_skip = !upscale_enabled
                                || !crate::ai::upscale::should_process(w, h, upscale_px);
                            let denoise_skip = !denoise_enabled
                                || !crate::ai::upscale::should_process(w, h, denoise_px);
                            if upscale_skip && denoise_skip {
                                return true;
                            }
                        }
                        false
                    })
                    .count();
                (done < total).then_some((done, total))
            }
        };

        if lines.is_empty() && prefetch_progress.is_none() {
            self.ai_status_done_at = None;
            return;
        }

        // 全処理完了後の自動非表示: 完了から 1 秒フル表示、続く 1 秒でフェードアウト。
        const FADE_START_SECS: f32 = 1.0;
        const FADE_DURATION_SECS: f32 = 1.0;
        if any_busy {
            self.ai_status_done_at = None;
        } else {
            let done_at = *self.ai_status_done_at.get_or_insert_with(std::time::Instant::now);
            if done_at.elapsed().as_secs_f32() > FADE_START_SECS + FADE_DURATION_SECS {
                return;
            }
        }

        let alpha = if let Some(done_at) = self.ai_status_done_at {
            let elapsed = done_at.elapsed().as_secs_f32();
            if elapsed < FADE_START_SECS { 1.0 }
            else { (1.0 - (elapsed - FADE_START_SECS) / FADE_DURATION_SECS).clamp(0.0, 1.0) }
        } else {
            1.0
        };

        // Area の available width が 0 のまま描画されるとラベルが 1 文字幅で
        // 縦に折り返される。min_width で横方向を確保する。
        const MIN_WIDTH: f32 = 260.0;
        const BAR_WIDTH: f32 = 180.0;
        const FONT_SIZE: f32 = 13.0;

        let ctx = ui.ctx().clone();
        egui::Area::new("fs_ai_status_overlay".into())
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(12.0, -12.0))
            .show(&ctx, |ui| {
                ui.set_opacity(alpha);
                ui.set_min_width(MIN_WIDTH);
                egui::Frame::popup(ui.style())
                    .fill(crate::ui_helpers::PROGRESS_BG_COLOR)
                    .show(ui, |ui| {
                        for (text, color) in &lines {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(text).color(*color).size(FONT_SIZE),
                                )
                                .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        }
                        if let Some((done, total)) = prefetch_progress {
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new("先読み AI")
                                            .monospace()
                                            .color(crate::ui_helpers::PROGRESS_LABEL_COLOR),
                                    )
                                    .wrap_mode(egui::TextWrapMode::Extend),
                                );
                                ui.add(
                                    egui::ProgressBar::new(done as f32 / total as f32)
                                        .desired_width(BAR_WIDTH)
                                        .fill(crate::ui_helpers::PROGRESS_UPGRADE_COLOR)
                                        .text(
                                            egui::RichText::new(format!("{} / {}", done, total))
                                                .color(egui::Color32::BLACK),
                                        ),
                                );
                            });
                        }
                    });
            });

        // フェードアウト中のみ毎フレーム再描画。処理中の進捗更新は
        // poll_ai_upscale / poll_prefetch 側が完了時に repaint を要求するので
        // ここでの busy-loop repaint は不要。
        if self.ai_status_done_at.is_some() {
            ctx.request_repaint();
        }
    }
}

// ── ホバーバーのアイコン描画関数 ────────────────────────────────────────

/// バーボタンの標準背景色を返す。
fn bar_button_bg(hovered: bool, active: bool) -> egui::Color32 {
    if active {
        egui::Color32::from_rgba_unmultiplied(80, 140, 220, 200)
    } else if hovered {
        egui::Color32::from_rgba_unmultiplied(100, 100, 100, 200)
    } else {
        egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
    }
}

/// バーボタンの共通描画。位置とアイコン描画関数を受け取る。
fn draw_bar_button(
    ui: &mut egui::Ui,
    x: f32,
    y: f32,
    id: &str,
    bg_fn: impl FnOnce(bool) -> egui::Color32,
    _active: bool,
    icon_fn: impl FnOnce(&egui::Painter, egui::Pos2, f32),
) -> egui::Response {
    let rect = egui::Rect::from_min_size(
        egui::pos2(x, y),
        egui::vec2(BAR_BUTTON_SIZE, BAR_BUTTON_SIZE),
    );
    let resp = ui.interact(rect, egui::Id::new(id), egui::Sense::click());
    let bg = bg_fn(resp.hovered());
    ui.painter().rect_filled(rect, 4.0, bg);
    let r = BAR_BUTTON_SIZE * 0.28;
    icon_fn(ui.painter(), rect.center(), r);
    resp
}

/// × アイコンを描画する。
fn draw_close_icon(painter: &egui::Painter, c: egui::Pos2, _r: f32) {
    let r = BAR_BUTTON_SIZE * 0.25;
    let stroke = egui::Stroke::new(2.5, egui::Color32::WHITE);
    painter.line_segment(
        [egui::pos2(c.x - r, c.y - r), egui::pos2(c.x + r, c.y + r)],
        stroke,
    );
    painter.line_segment(
        [egui::pos2(c.x + r, c.y - r), egui::pos2(c.x - r, c.y + r)],
        stroke,
    );
}

/// 一時停止アイコン (2本の縦線) を描画する。
fn draw_pause_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let bar_w = r * 0.3;
    let gap = r * 0.35;
    let stroke = egui::Stroke::new(bar_w, egui::Color32::WHITE);
    painter.line_segment(
        [egui::pos2(c.x - gap, c.y - r), egui::pos2(c.x - gap, c.y + r)],
        stroke,
    );
    painter.line_segment(
        [egui::pos2(c.x + gap, c.y - r), egui::pos2(c.x + gap, c.y + r)],
        stroke,
    );
}

/// 再生アイコン (右向き三角形) を描画する。
fn draw_play_triangle(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let cx = c.x + r * 0.12;
    let points = vec![
        egui::pos2(cx - r * 0.5, c.y - r * 0.75),
        egui::pos2(cx - r * 0.5, c.y + r * 0.75),
        egui::pos2(cx + r * 0.7, c.y),
    ];
    painter.add(egui::Shape::convex_polygon(
        points, egui::Color32::WHITE, egui::Stroke::NONE,
    ));
}

/// 🔬 分析アイコン（虫眼鏡＋十字線）を描画する。
fn draw_analysis_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let white = egui::Color32::WHITE;
    let stroke = egui::Stroke::new(1.8, white);
    // 虫眼鏡の円
    let lens_r = r * 0.62;
    let lens_cx = c.x - r * 0.12;
    let lens_cy = c.y - r * 0.12;
    painter.circle_stroke(egui::pos2(lens_cx, lens_cy), lens_r, stroke);
    // 虫眼鏡のハンドル
    let angle = std::f32::consts::FRAC_PI_4;
    let handle_start = egui::pos2(
        lens_cx + lens_r * angle.cos(),
        lens_cy + lens_r * angle.sin(),
    );
    let handle_end = egui::pos2(c.x + r * 0.72, c.y + r * 0.72);
    painter.line_segment([handle_start, handle_end], egui::Stroke::new(2.2, white));
    // 十字線（レンズ内）
    let ch = lens_r * 0.55;
    painter.line_segment(
        [egui::pos2(lens_cx - ch, lens_cy), egui::pos2(lens_cx + ch, lens_cy)],
        egui::Stroke::new(1.2, white),
    );
    painter.line_segment(
        [egui::pos2(lens_cx, lens_cy - ch), egui::pos2(lens_cx, lens_cy + ch)],
        egui::Stroke::new(1.2, white),
    );
}

/// ℹ アイコンを描画する。
fn draw_info_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let white = egui::Color32::WHITE;
    painter.circle_stroke(c, r, egui::Stroke::new(1.5, white));
    let bar_w = r * 0.22;
    painter.line_segment(
        [egui::pos2(c.x, c.y - r * 0.05), egui::pos2(c.x, c.y + r * 0.55)],
        egui::Stroke::new(bar_w, white),
    );
    painter.circle_filled(egui::pos2(c.x, c.y - r * 0.45), bar_w * 0.7, white);
}

/// チェックマーク（右上）を描画する。
fn draw_fs_checkmark(ui: &mut egui::Ui, full_rect: egui::Rect) {
    let check_center = egui::pos2(
        full_rect.max.x - CHECKMARK_RADIUS - CHECKMARK_MARGIN,
        full_rect.min.y + CHECKMARK_RADIUS + CHECKMARK_MARGIN,
    );
    ui.painter().circle_filled(
        check_center, CHECKMARK_RADIUS,
        egui::Color32::from_rgb(40, 160, 40),
    );
    let s = CHECKMARK_RADIUS * 0.55;
    let stroke = egui::Stroke::new(3.0, egui::Color32::WHITE);
    ui.painter().line_segment(
        [
            egui::pos2(check_center.x - s * 0.6, check_center.y),
            egui::pos2(check_center.x - s * 0.1, check_center.y + s * 0.5),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            egui::pos2(check_center.x - s * 0.1, check_center.y + s * 0.5),
            egui::pos2(check_center.x + s * 0.7, check_center.y - s * 0.5),
        ],
        stroke,
    );
}

/// ファイル情報テキスト（ファイル名・寸法・サイズ）を描画する。
fn draw_fs_bar_info_text(
    ui: &mut egui::Ui,
    bar_rect: egui::Rect,
    right_anchor: egui::Pos2,
    filename: &str,
    image_dims: Option<(u32, u32)>,
    image_file_size: Option<u64>,
    ai_upscale_info: Option<(&str, u32, u32)>,
    pdf_content_type: Option<PdfPageContentType>,
) {
    let mut parts: Vec<String> = Vec::new();
    if !filename.is_empty() { parts.push(filename.to_string()); }
    // PDF コンテンツ種別
    if let Some(ct) = pdf_content_type {
        match ct {
            PdfPageContentType::Raster { w, h } => {
                parts.push(format!("PDF Raster {w}×{h}"));
            }
            PdfPageContentType::Vector => {
                parts.push("PDF Vector".to_string());
            }
        }
    }
    if let Some((w, h)) = image_dims {
        if let Some((model_name, ai_w, ai_h)) = ai_upscale_info {
            // AI アップスケール情報を表示: "11 × 22 (漫画 44×88)"
            parts.push(format!("{w} × {h} ({model_name} {ai_w}×{ai_h})"));
        } else {
            parts.push(format!("{w} × {h}"));
        }
    }
    if let Some(bytes) = image_file_size { parts.push(format_bytes_small(bytes)); }
    if !parts.is_empty() {
        ui.painter().text(
            right_anchor,
            egui::Align2::RIGHT_CENTER,
            parts.join("    "),
            egui::FontId::proportional(15.0),
            egui::Color32::WHITE,
        );
    }
    let _ = bar_rect;
}

/// 回転アイコンを自前描画する。
fn draw_rotate_icon(painter: &egui::Painter, center: egui::Pos2, radius: f32, clockwise: bool) {
    let stroke = egui::Stroke::new(2.0, egui::Color32::WHITE);
    let n = 24;

    let start_rad = 315.0_f32.to_radians();
    let end_rad = (315.0 + 270.0_f32).to_radians();
    let arc_span = end_rad - start_rad;

    let mut points = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let angle = start_rad + arc_span * t;
        points.push(egui::pos2(
            center.x + radius * angle.cos(),
            center.y + radius * angle.sin(),
        ));
    }

    for i in 0..n {
        painter.line_segment([points[i], points[i + 1]], stroke);
    }

    let (arrow_pt, tangent_x, tangent_y) = if clockwise {
        let angle = end_rad;
        (points[n], angle.sin(), -angle.cos())
    } else {
        let angle = start_rad;
        (points[0], -angle.sin(), angle.cos())
    };

    let nx = -tangent_y;
    let ny = tangent_x;
    let a = radius * 0.55;
    let p1 = egui::pos2(
        arrow_pt.x + tangent_x * a + nx * a * 0.45,
        arrow_pt.y + tangent_y * a + ny * a * 0.45,
    );
    let p2 = egui::pos2(
        arrow_pt.x + tangent_x * a - nx * a * 0.45,
        arrow_pt.y + tangent_y * a - ny * a * 0.45,
    );
    painter.line_segment([arrow_pt, p1], stroke);
    painter.line_segment([arrow_pt, p2], stroke);
}

/// 見開きモードアイコンを描画する。
fn draw_spread_icon(painter: &egui::Painter, c: egui::Pos2, r: f32, mode: SpreadMode) {
    let white = egui::Color32::WHITE;
    let stroke = egui::Stroke::new(1.5, white);
    let page_w = r * 0.7;
    let page_h = r * 1.1;

    match mode {
        SpreadMode::Single => {
            // 単独ページ: 1枚の矩形
            let rect = egui::Rect::from_center_size(c, egui::vec2(page_w, page_h));
            painter.rect_stroke(rect, 1.0, stroke, egui::StrokeKind::Outside);
        }
        SpreadMode::Ltr | SpreadMode::Rtl => {
            // 見開き（表紙なし）: 2枚の矩形
            let gap = r * 0.15;
            let left_rect = egui::Rect::from_center_size(
                egui::pos2(c.x - page_w * 0.5 - gap * 0.5, c.y),
                egui::vec2(page_w, page_h),
            );
            let right_rect = egui::Rect::from_center_size(
                egui::pos2(c.x + page_w * 0.5 + gap * 0.5, c.y),
                egui::vec2(page_w, page_h),
            );
            painter.rect_stroke(left_rect, 1.0, stroke, egui::StrokeKind::Outside);
            painter.rect_stroke(right_rect, 1.0, stroke, egui::StrokeKind::Outside);
            // 方向矢印
            draw_spread_direction_arrow(painter, c, r, mode.is_rtl());
        }
        SpreadMode::LtrCover | SpreadMode::RtlCover => {
            // 表紙あり: 小さい表紙 + 見開き2枚
            let small_w = page_w * 0.55;
            let gap = r * 0.12;
            let total = small_w + gap + page_w * 2.0 + gap;
            let start_x = c.x - total * 0.5;

            // 表紙（小さい矩形）
            let cover_rect = egui::Rect::from_center_size(
                egui::pos2(start_x + small_w * 0.5, c.y),
                egui::vec2(small_w, page_h),
            );
            painter.rect_stroke(cover_rect, 1.0, stroke, egui::StrokeKind::Outside);

            // 見開き2枚
            let spread_x = start_x + small_w + gap;
            let left_rect = egui::Rect::from_center_size(
                egui::pos2(spread_x + page_w * 0.5, c.y),
                egui::vec2(page_w, page_h),
            );
            let right_rect = egui::Rect::from_center_size(
                egui::pos2(spread_x + page_w * 1.5 + gap, c.y),
                egui::vec2(page_w, page_h),
            );
            painter.rect_stroke(left_rect, 1.0, stroke, egui::StrokeKind::Outside);
            painter.rect_stroke(right_rect, 1.0, stroke, egui::StrokeKind::Outside);
            // 方向矢印
            draw_spread_direction_arrow(painter, c, r, mode.is_rtl());
        }
    }
}

/// 見開きモードの方向矢印（→ or ←）を描画する。
fn draw_spread_direction_arrow(
    painter: &egui::Painter,
    c: egui::Pos2,
    r: f32,
    rtl: bool,
) {
    let white = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 180);
    let arrow_stroke = egui::Stroke::new(1.2, white);
    let ay = c.y + r * 1.4; // 矩形の下
    let ax = c.x;
    let alen = r * 0.6;
    let ahead = r * 0.3;

    if rtl {
        // ←
        painter.line_segment(
            [egui::pos2(ax + alen, ay), egui::pos2(ax - alen, ay)],
            arrow_stroke,
        );
        painter.line_segment(
            [egui::pos2(ax - alen, ay), egui::pos2(ax - alen + ahead, ay - ahead)],
            arrow_stroke,
        );
        painter.line_segment(
            [egui::pos2(ax - alen, ay), egui::pos2(ax - alen + ahead, ay + ahead)],
            arrow_stroke,
        );
    } else {
        // →
        painter.line_segment(
            [egui::pos2(ax - alen, ay), egui::pos2(ax + alen, ay)],
            arrow_stroke,
        );
        painter.line_segment(
            [egui::pos2(ax + alen, ay), egui::pos2(ax + alen - ahead, ay - ahead)],
            arrow_stroke,
        );
        painter.line_segment(
            [egui::pos2(ax + alen, ay), egui::pos2(ax + alen - ahead, ay + ahead)],
            arrow_stroke,
        );
    }
}

impl App {
    /// 右上にフィードバックトーストを描画する。
    fn draw_feedback_toast(&mut self, ui: &mut egui::Ui, full_rect: egui::Rect, ctx: &egui::Context) {
        let Some((ref text, start_time)) = self.fs_feedback_toast else { return; };
        let elapsed = start_time.elapsed().as_secs_f32();
        if elapsed > FEEDBACK_TOAST_DURATION {
            self.fs_feedback_toast = None;
            return;
        }

        // フェードアウト (最後の0.3秒)
        let alpha = if elapsed > FEEDBACK_TOAST_DURATION - 0.3 {
            ((FEEDBACK_TOAST_DURATION - elapsed) / 0.3).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let alpha_u8 = (alpha * 220.0) as u8;

        let font = egui::FontId::proportional(18.0);
        let galley = ui.painter().layout_no_wrap(text.clone(), font.clone(), egui::Color32::WHITE);
        let text_size = galley.size();
        let padding = egui::vec2(16.0, 10.0);
        let toast_size = text_size + padding * 2.0;

        let toast_rect = egui::Rect::from_min_size(
            egui::pos2(full_rect.max.x - toast_size.x - 20.0, full_rect.min.y + 60.0),
            toast_size,
        );

        ui.painter().rect_filled(
            toast_rect, 8.0,
            egui::Color32::from_rgba_unmultiplied(30, 30, 30, alpha_u8),
        );
        ui.painter().text(
            toast_rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            font,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, (alpha * 255.0) as u8),
        );

        // フェードアウト中は 30fps で再描画
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }

    /// 画面中央に境界ヒント (最初/最後の画像です…) を描画する。
    fn draw_boundary_hint(&mut self, ui: &mut egui::Ui, full_rect: egui::Rect, ctx: &egui::Context) {
        let Some((at_end, start_time)) = self.fs_boundary_hint else { return; };
        let elapsed = start_time.elapsed().as_secs_f32();
        if elapsed > BOUNDARY_HINT_DURATION {
            self.fs_boundary_hint = None;
            return;
        }

        let alpha = if elapsed > BOUNDARY_HINT_DURATION - 0.4 {
            ((BOUNDARY_HINT_DURATION - elapsed) / 0.4).clamp(0.0, 1.0)
        } else {
            1.0
        };

        let (title, line1, line2) = if at_end {
            ("最後の画像です", "[Home] 最初に戻る", "[Ctrl]+[↓] 次のフォルダへ")
        } else {
            ("最初の画像です", "[End] 最後に移動", "[Ctrl]+[↑] 前のフォルダへ")
        };

        let title_font = egui::FontId::proportional(32.0);
        let body_font = egui::FontId::proportional(22.0);
        let white = egui::Color32::from_rgba_unmultiplied(255, 255, 255, (alpha * 255.0) as u8);
        let accent = egui::Color32::from_rgba_unmultiplied(255, 220, 120, (alpha * 255.0) as u8);

        let painter = ui.painter();
        let title_galley = painter.layout_no_wrap(title.to_string(), title_font.clone(), white);
        let line1_galley = painter.layout_no_wrap(line1.to_string(), body_font.clone(), white);
        let line2_galley = painter.layout_no_wrap(line2.to_string(), body_font.clone(), white);

        let line_gap = 10.0;
        let padding = egui::vec2(32.0, 24.0);
        let content_w = title_galley.size().x
            .max(line1_galley.size().x)
            .max(line2_galley.size().x);
        let content_h = title_galley.size().y
            + line_gap * 1.5
            + line1_galley.size().y
            + line_gap
            + line2_galley.size().y;
        let box_size = egui::vec2(content_w, content_h) + padding * 2.0;

        let center = full_rect.center();
        let box_rect = egui::Rect::from_center_size(center, box_size);

        let bg_alpha = (alpha * 210.0) as u8;
        painter.rect_filled(
            box_rect, 12.0,
            egui::Color32::from_rgba_unmultiplied(20, 20, 20, bg_alpha),
        );
        painter.rect_stroke(
            box_rect, 12.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(200, 200, 200, (alpha * 120.0) as u8)),
            egui::StrokeKind::Outside,
        );

        let mut y = box_rect.min.y + padding.y;
        painter.text(
            egui::pos2(center.x, y),
            egui::Align2::CENTER_TOP,
            title,
            title_font,
            accent,
        );
        y += title_galley.size().y + line_gap * 1.5;
        painter.text(
            egui::pos2(center.x, y),
            egui::Align2::CENTER_TOP,
            line1,
            body_font.clone(),
            white,
        );
        y += line1_galley.size().y + line_gap;
        painter.text(
            egui::pos2(center.x, y),
            egui::Align2::CENTER_TOP,
            line2,
            body_font,
            white,
        );

        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

