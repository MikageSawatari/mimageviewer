//! フルスクリーン表示のレンダリング。
//!
//! `App::update()` から呼ばれる `render_fullscreen_viewport()` を実装する。
//! 元は `update()` 内にインラインで書かれていた ~460 行を独立メソッドに切り出したもの。

use eframe::egui;

use crate::app::App;
use crate::folder_tree::{navigate_folder_with_skip, next_folder_dfs, prev_folder_dfs};
use crate::fs_animation::FsCacheEntry;
use crate::grid_item::{GridItem, ThumbnailState};
use crate::ui_helpers::{draw_play_icon, format_bytes_small, open_external_player};

// ── 定数 ────────────────────────────────────────────────────────────────

/// メタデータパネルの最大幅
const METADATA_PANEL_WIDTH: f32 = 380.0;
/// ホバー時トップバーの高さ
const TOP_BAR_HEIGHT: f32 = 44.0;
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
}

/// フルスクリーンのキー入力結果。
struct FsKeyAction {
    close: bool,
    nav_delta: i32,
    ctrl_nav: Option<i32>,
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
        let fs_builder = egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_visible(false)
            .with_inner_size([1.0, 1.0]);
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("fullscreen_viewer"),
            fs_builder,
            |_ctx, _class| {},
        );
        self.fs_viewport_created = true;
    }

    pub(crate) fn render_fullscreen_viewport(&mut self, ctx: &egui::Context) {
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };

        // ── 状態の事前計算 ──
        self.advance_animation(ctx, fs_idx);
        let state = self.prepare_fullscreen_state(ctx, fs_idx);

        let mut close_fs = false;
        let mut nav_delta: i32 = 0;
        let mut ctrl_nav: Option<i32> = None;

        // ── ビューポート構築 ──
        let fs_builder = self.build_fullscreen_viewport_builder();
        let need_show = !self.fs_viewport_shown;

        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("fullscreen_viewer"),
            fs_builder,
            |ctx, _class| {
                if need_show {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }

                if ctx.input(|i| i.viewport().close_requested()) {
                    close_fs = true;
                }

                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(egui::Color32::BLACK))
                    .show(ctx, |ui| {
                        let full_rect = ui.max_rect();

                        // ── キー入力 ──
                        let key_action = self.handle_fs_key_input(ctx, fs_idx);
                        if key_action.close { close_fs = true; }
                        nav_delta = key_action.nav_delta;
                        ctrl_nav = key_action.ctrl_nav;

                        // ── ホイール & クリック ──
                        let (wheel_nav, click_close) = self.handle_fs_wheel_and_click(
                            ui, ctx, full_rect, &state,
                        );
                        if wheel_nav != 0 { nav_delta = wheel_nav; }
                        if click_close { close_fs = true; }

                        // ── 画像 / 動画 / セパレータ描画 ──
                        if let Some(sep) = state.separator_text.as_ref() {
                            Self::draw_fs_separator(ui, full_rect, sep);
                        } else {
                            let fs_rotation = self.get_rotation(fs_idx);
                            Self::draw_fs_image(
                                ui, full_rect,
                                state.tex.as_ref(), state.thumb_tex.as_ref(),
                                state.is_video, state.fs_load_failed, fs_rotation,
                            );
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
                        if state.is_loading && has_any_tex {
                            ui.painter().text(
                                full_rect.min + egui::vec2(16.0, 16.0),
                                egui::Align2::LEFT_TOP,
                                "高解像度 読込中...",
                                egui::FontId::proportional(14.0),
                                egui::Color32::from_rgba_unmultiplied(220, 220, 220, 180),
                            );
                        }

                        // ── メタデータパネル ──
                        let right_panel_visible =
                            self.draw_metadata_panel(ui, ctx, full_rect);

                        // ── ホバーバー ──
                        let mut bar_rotate_cw = false;
                        let mut bar_rotate_ccw = false;
                        Self::draw_fs_hover_bar(
                            ui, ctx, full_rect,
                            &state.folder_display, &state.filename,
                            state.image_dims, state.image_file_size,
                            &mut close_fs, &mut nav_delta,
                            &mut self.show_metadata_panel,
                            right_panel_visible,
                            &mut self.slideshow_playing,
                            &mut self.settings.slideshow_interval_secs,
                            &mut bar_rotate_cw, &mut bar_rotate_ccw,
                        );
                        if bar_rotate_cw { self.rotate_image_cw(fs_idx); }
                        if bar_rotate_ccw { self.rotate_image_ccw(fs_idx); }
                    });
            },
        );

        self.fs_viewport_created = true;
        self.fs_viewport_shown = true;

        // ── ナビゲーション & スライドショー処理 ──
        self.handle_fs_navigation(ctx, close_fs, ctrl_nav, nav_delta, fs_idx);
        self.handle_fs_repaint(ctx, fs_idx, state.is_video);
    }

    // ── 状態準備ヘルパー ────────────────────────────────────────────────

    /// アニメーションフレームを進める（メインコンテキストの時刻を使う）。
    fn advance_animation(&mut self, ctx: &egui::Context, fs_idx: usize) {
        let is_video = matches!(self.items.get(fs_idx), Some(GridItem::Video(_)));
        if is_video { return; }
        let now = ctx.input(|i| i.time);
        if let Some(FsCacheEntry::Animated {
            frames, current_frame, next_frame_at,
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
            match self.fs_cache.get(&fs_idx) {
                Some(FsCacheEntry::Static(h)) => Some(h.clone()),
                Some(FsCacheEntry::Animated { frames, current_frame, .. }) => {
                    frames.get(*current_frame).map(|(h, _)| h.clone())
                }
                Some(FsCacheEntry::Failed) | None => None,
            }
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
        let image_dims: Option<(u32, u32)> = tex.as_ref().map(|t| {
            let s = t.size_vec2();
            (s.x as u32, s.y as u32)
        });
        let image_file_size: Option<u64> = self.image_metas.get(fs_idx)
            .and_then(|m| m.map(|(_, sz)| sz.max(0) as u64));
        let is_loading =
            !is_video && !is_separator && !fs_load_failed && !self.fs_cache.contains_key(&fs_idx);

        FsFrameState {
            is_video, separator_text, video_path, tex, thumb_tex,
            filename, folder_display, image_dims, image_file_size,
            is_loading, fs_load_failed,
        }
    }

    /// フルスクリーンビューポートの ViewportBuilder を構築する。
    fn build_fullscreen_viewport_builder(&self) -> egui::ViewportBuilder {
        let center = self.last_outer_rect.map(|r| r.center());
        let ppp = self.last_pixels_per_point;
        crate::logger::log(format!(
            "[fullscreen] last_outer_rect center: {:?}  ppp={ppp:.2}",
            center.map(|c| format!("({:.1},{:.1})", c.x, c.y))
        ));

        let monitor_rect = center.and_then(|c| {
            crate::monitor::get_monitor_logical_rect_at(c.x * ppp, c.y * ppp)
        });

        let b = egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_transparent(true);
        match monitor_rect {
            Some(rect) => {
                crate::logger::log(format!(
                    "[fullscreen] using monitor rect: pos=({:.1},{:.1}) size={:.1}x{:.1}",
                    rect.min.x, rect.min.y, rect.width(), rect.height()
                ));
                b.with_position(rect.min)
                    .with_inner_size([rect.width(), rect.height()])
            }
            None => {
                crate::logger::log(
                    "[fullscreen] monitor rect not found, fallback to with_fullscreen".to_string(),
                );
                b.with_fullscreen(true)
            }
        }
    }

    // ── キー入力 ────────────────────────────────────────────────────────

    /// フルスクリーンのキー入力を処理し、アクションを返す。
    fn handle_fs_key_input(&mut self, ctx: &egui::Context, fs_idx: usize) -> FsKeyAction {
        let has_focus = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        let mut action = FsKeyAction { close: false, nav_delta: 0, ctrl_nav: None };

        if !has_focus { return action; }

        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let right = ctx.input(|i| {
            i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowDown)
        });
        let left = ctx.input(|i| {
            i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowUp)
        });
        let ctrl_d = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowDown));
        let ctrl_u = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowUp));
        let key_i = ctx.input(|i| i.key_pressed(egui::Key::I) || i.key_pressed(egui::Key::Tab));
        let key_s = ctx.input(|i| i.key_pressed(egui::Key::Space));
        let key_r = ctx.input(|i| i.key_pressed(egui::Key::R));
        let key_l = ctx.input(|i| i.key_pressed(egui::Key::L));

        if esc { action.close = true; }
        if key_i { self.show_metadata_panel = !self.show_metadata_panel; }

        // Space: スライドショー中→停止、停止中→画像をチェック
        if key_s {
            if self.slideshow_playing {
                self.slideshow_playing = false;
            } else {
                match self.items.get(fs_idx) {
                    Some(GridItem::Image(_))
                    | Some(GridItem::Video(_))
                    | Some(GridItem::ZipImage { .. }) => {
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
        if key_r { self.rotate_image_cw(fs_idx); }
        if key_l { self.rotate_image_ccw(fs_idx); }

        if right && !ctrl_d {
            action.nav_delta = 1;
            self.slideshow_playing = false;
        }
        if left && !ctrl_u {
            action.nav_delta = -1;
            self.slideshow_playing = false;
        }
        if ctrl_d { action.ctrl_nav = Some(1); }
        if ctrl_u { action.ctrl_nav = Some(-1); }

        action
    }

    // ── ホイール & クリック ──────────────────────────────────────────────

    /// ホイールとクリックを処理し、(nav_delta, close) を返す。
    fn handle_fs_wheel_and_click(
        &self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        state: &FsFrameState,
    ) -> (i32, bool) {
        let mut nav_delta = 0i32;
        let mut close = false;

        // ── ホイール ──
        let panel_w = METADATA_PANEL_WIDTH.min(full_rect.width() * 0.5);
        let panel_left = full_rect.max.x - panel_w;
        let hover_threshold = full_rect.max.x - full_rect.width() * 0.25;
        let cursor_in_panel = ctx.input(|i| {
            i.pointer.hover_pos().map(|p| {
                p.x > panel_left
                    && p.y >= 60.0
                    && (self.show_metadata_panel || p.x > hover_threshold)
            }).unwrap_or(false)
        });

        let wheel_y = ctx.input(|i| i.raw_scroll_delta.y);
        if wheel_y.abs() > 0.5 && !cursor_in_panel {
            ctx.input_mut(|i| {
                i.raw_scroll_delta = egui::Vec2::ZERO;
                i.smooth_scroll_delta = egui::Vec2::ZERO;
                i.events.retain(|e| !matches!(e, egui::Event::MouseWheel { .. }));
            });
            nav_delta = if wheel_y < 0.0 { 1 } else { -1 };
        }

        // ── クリック ──
        let fs_response = ui.interact(
            full_rect,
            egui::Id::new("fs_click"),
            egui::Sense::click(),
        );
        if state.is_video {
            if fs_response.clicked() {
                if let Some(ref vp) = state.video_path {
                    open_external_player(vp);
                }
            }
        } else if fs_response.clicked() {
            if let Some(pos) = fs_response.interact_pointer_pos() {
                let panel_threshold = full_rect.max.x - full_rect.width() * 0.25;
                let in_panel = pos.y >= 60.0
                    && (self.show_metadata_panel || pos.x > panel_threshold)
                    && pos.x > full_rect.max.x - METADATA_PANEL_WIDTH.min(full_rect.width() * 0.5);
                if !in_panel {
                    nav_delta = if pos.x > full_rect.center().x { 1 } else { -1 };
                }
            }
        }
        if fs_response.secondary_clicked() {
            close = true;
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
        fs_idx: usize,
    ) {
        if close_fs || ctrl_nav.is_some() {
            self.close_fullscreen();
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if let Some(delta) = ctrl_nav {
            if let Some(cur) = self.current_folder.clone() {
                let skip_limit = self.settings.folder_skip_limit;
                let next = if delta > 0 {
                    navigate_folder_with_skip(&cur, next_folder_dfs, skip_limit)
                } else {
                    navigate_folder_with_skip(&cur, prev_folder_dfs, skip_limit)
                };
                if let Some(p) = next {
                    self.load_folder(p);
                }
            }
        } else if !close_fs && nav_delta != 0 {
            if let Some(new_idx) = crate::ui_helpers::adjacent_navigable_idx(
                &self.items, &self.visible_indices, fs_idx, nav_delta,
            ) {
                self.open_fullscreen(new_idx);
                self.selected = Some(new_idx);
                self.scroll_to_selected = true;
                self.update_last_selected_image();
            }
        }

        // ── スライドショー タイマー ──
        if self.slideshow_playing && !close_fs {
            let now = std::time::Instant::now();
            if now >= self.slideshow_next_at {
                if let Some(cur) = self.fullscreen_idx {
                    let next = crate::ui_helpers::adjacent_navigable_idx(
                        &self.items, &self.visible_indices, cur, 1,
                    );
                    let target = match next {
                        Some(idx) => idx,
                        None => {
                            self.visible_indices.iter().copied()
                                .find(|&i| matches!(
                                    self.items.get(i),
                                    Some(GridItem::Image(_)) | Some(GridItem::ZipImage { .. })
                                ))
                                .unwrap_or(0)
                        }
                    };
                    self.open_fullscreen(target);
                    self.selected = Some(target);
                    self.scroll_to_selected = true;
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
        // 高解像度読み込み完了まで毎フレーム再描画
        let image_loading = !is_video
            && self.fullscreen_idx
                .map(|i| !self.fs_cache.contains_key(&i))
                .unwrap_or(false);
        if image_loading {
            ctx.request_repaint();
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
    fn draw_fs_image(
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        tex: Option<&egui::TextureHandle>,
        thumb_tex: Option<&egui::TextureHandle>,
        is_video: bool,
        fs_load_failed: bool,
        rotation: crate::rotation_db::Rotation,
    ) {
        let display_tex = tex.or(thumb_tex);
        if let Some(handle) = display_tex {
            let tex_size = handle.size_vec2();
            let display_size = match rotation {
                crate::rotation_db::Rotation::Cw90
                | crate::rotation_db::Rotation::Cw270 => {
                    egui::vec2(tex_size.y, tex_size.x)
                }
                _ => tex_size,
            };
            let scale =
                (full_rect.width() / display_size.x).min(full_rect.height() / display_size.y);
            let img_rect =
                egui::Rect::from_center_size(full_rect.center(), display_size * scale);
            if rotation.is_none() {
                ui.painter().image(
                    handle.id(),
                    img_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                crate::app::draw_rotated_image(
                    ui.painter(), handle.id(), img_rect, rotation,
                );
            }
        } else if fs_load_failed {
            ui.painter().text(
                full_rect.center(),
                egui::Align2::CENTER_CENTER,
                "読込失敗",
                egui::FontId::proportional(32.0),
                egui::Color32::from_rgb(255, 140, 140),
            );
            ui.painter().text(
                full_rect.center() + egui::vec2(0.0, 40.0),
                egui::Align2::CENTER_CENTER,
                "このファイルはデコードできませんでした",
                egui::FontId::proportional(16.0),
                egui::Color32::from_gray(180),
            );
        } else {
            ui.painter().text(
                full_rect.center(),
                egui::Align2::CENTER_CENTER,
                if is_video { "動画サムネイル 読込中..." } else { "読込中..." },
                egui::FontId::proportional(24.0),
                egui::Color32::from_gray(180),
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
    ) {
        let hover_in_top = ctx
            .input(|i| i.pointer.hover_pos().map(|p| p.y < 60.0).unwrap_or(false));
        if !hover_in_top && !force_show {
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
        if info_resp.clicked() { *show_info = !*show_info; }
        if info_resp.hovered() { *nav_delta = 0; }

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
        );
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
) {
    let mut parts: Vec<String> = Vec::new();
    if !filename.is_empty() { parts.push(filename.to_string()); }
    if let Some((w, h)) = image_dims { parts.push(format!("{w} × {h}")); }
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
    let _ = bar_rect; // bar_rect は配置参照のために引数に残す
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
