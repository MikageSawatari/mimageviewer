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

impl App {
    /// フルスクリーンビューポートを描画し、終了後のナビゲーション処理も行う。
    /// フルスクリーン表示中でなければ何もしない。
    pub(crate) fn render_fullscreen_viewport(&mut self, ctx: &egui::Context) {
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };

        // 動画か否かを判定
        let is_video = matches!(self.items.get(fs_idx), Some(GridItem::Video(_)));
        // タスク 3: ZIP セパレータ (章タイトル表示)
        let separator_text: Option<String> = match self.items.get(fs_idx) {
            Some(GridItem::ZipSeparator { dir_display }) => Some(dir_display.clone()),
            _ => None,
        };
        let is_separator = separator_text.is_some();
        let video_path = if is_video {
            if let Some(GridItem::Video(p)) = self.items.get(fs_idx) {
                Some(p.clone())
            } else {
                None
            }
        } else {
            None
        };

        // アニメーションフレームを進める（メインコンテキストの時刻を使う）
        if !is_video {
            let now = ctx.input(|i| i.time);
            if let Some(FsCacheEntry::Animated {
                frames,
                current_frame,
                next_frame_at,
            }) = self.fs_cache.get_mut(&fs_idx)
            {
                if now >= *next_frame_at && !frames.is_empty() {
                    *current_frame = (*current_frame + 1) % frames.len();
                    let delay = frames[*current_frame].1.max(0.02);
                    *next_frame_at = now + delay;
                }
            }
        }

        // 表示テクスチャを取得（動画は None、画像はキャッシュエントリから）
        let tex: Option<egui::TextureHandle> = if is_video {
            None
        } else {
            match self.fs_cache.get(&fs_idx) {
                Some(FsCacheEntry::Static(h)) => Some(h.clone()),
                Some(FsCacheEntry::Animated {
                    frames,
                    current_frame,
                    ..
                }) => frames.get(*current_frame).map(|(h, _)| h.clone()),
                Some(FsCacheEntry::Failed) | None => None,
            }
        };

        // フルスクリーンデコードが失敗した場合を検出
        let fs_load_failed = matches!(self.fs_cache.get(&fs_idx), Some(FsCacheEntry::Failed));

        let thumb_tex = match self.thumbnails.get(fs_idx) {
            Some(ThumbnailState::Loaded { tex, .. }) => Some(tex.clone()),
            _ => None,
        };
        let filename = self
            .items
            .get(fs_idx)
            .map(|item| item.name().to_string())
            .unwrap_or_default();
        // トップバー用: フォルダ (current_folder そのまま、ZIP なら ZIP ファイルのパス)
        let folder_display = self
            .current_folder
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        // トップバー用: フルサイズ読込完了時のネイティブ画像サイズ
        let image_dims: Option<(u32, u32)> = tex.as_ref().map(|t| {
            let s = t.size_vec2();
            (s.x as u32, s.y as u32)
        });
        // トップバー用: ファイルサイズ (image_metas から)
        let image_file_size: Option<u64> = self
            .image_metas
            .get(fs_idx)
            .and_then(|m| m.map(|(_, sz)| sz.max(0) as u64));
        // 画像のみ「高解像度読込中」表示が必要（動画・セパレータ・失敗は不要）
        let is_loading =
            !is_video && !is_separator && !fs_load_failed && !self.fs_cache.contains_key(&fs_idx);

        let mut close_fs = false;
        let mut nav_delta: i32 = 0;
        let mut ctrl_nav: Option<i32> = None;

        // メインウィンドウがあるモニターの論理ピクセル矩形を取得し、
        // そのモニターを完全に覆う borderless ウィンドウを作成する。
        // with_fullscreen(true) はプライマリモニター固定になるため使わない。
        let fs_builder = {
            let center = self.last_outer_rect.map(|r| r.center());
            let ppp = self.last_pixels_per_point;
            crate::logger::log(format!(
                "[fullscreen] last_outer_rect center: {:?}  ppp={ppp:.2}",
                center.map(|c| format!("({:.1},{:.1})", c.x, c.y))
            ));

            // MonitorFromPoint は物理座標を要求するため論理座標に ppp を乗算する
            let monitor_rect = center.and_then(|c| {
                crate::monitor::get_monitor_logical_rect_at(c.x * ppp, c.y * ppp)
            });

            let b = egui::ViewportBuilder::default().with_decorations(false);
            match monitor_rect {
                Some(rect) => {
                    crate::logger::log(format!(
                        "[fullscreen] using monitor rect: pos=({:.1},{:.1}) size={:.1}x{:.1}",
                        rect.min.x,
                        rect.min.y,
                        rect.width(),
                        rect.height()
                    ));
                    b.with_position(rect.min)
                        .with_inner_size([rect.width(), rect.height()])
                }
                None => {
                    crate::logger::log(
                        "[fullscreen] monitor rect not found, fallback to with_fullscreen"
                            .to_string(),
                    );
                    b.with_fullscreen(true)
                }
            }
        };

        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("fullscreen_viewer"),
            fs_builder,
            |ctx, _class| {
                // プラットフォームの閉じるリクエスト（Alt+F4 など）
                if ctx.input(|i| i.viewport().close_requested()) {
                    close_fs = true;
                }

                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(egui::Color32::BLACK))
                    .show(ctx, |ui| {
                        let full_rect = ui.max_rect();

                        // ── キー入力（ctx はこのビューポートのコンテキスト）
                        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
                        let right = ctx.input(|i| {
                            i.key_pressed(egui::Key::ArrowRight)
                                || i.key_pressed(egui::Key::ArrowDown)
                        });
                        let left = ctx.input(|i| {
                            i.key_pressed(egui::Key::ArrowLeft)
                                || i.key_pressed(egui::Key::ArrowUp)
                        });
                        let ctrl_d = ctx.input(|i| {
                            i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowDown)
                        });
                        let ctrl_u = ctx
                            .input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowUp));
                        let key_i = ctx.input(|i| i.key_pressed(egui::Key::I));

                        let key_s = ctx.input(|i| i.key_pressed(egui::Key::Space));
                        let key_r = ctx.input(|i| i.key_pressed(egui::Key::R));
                        let key_l = ctx.input(|i| i.key_pressed(egui::Key::L));

                        if esc {
                            close_fs = true;
                        }
                        if key_i {
                            self.show_metadata_panel = !self.show_metadata_panel;
                        }
                        // Space: スライドショー再生/一時停止
                        if key_s {
                            self.slideshow_playing = !self.slideshow_playing;
                            if self.slideshow_playing {
                                self.slideshow_next_at = std::time::Instant::now()
                                    + std::time::Duration::from_secs_f32(
                                        self.settings.slideshow_interval_secs,
                                    );
                            }
                        }
                        // R: 時計回り 90° 回転、L: 反時計回り 90° 回転
                        if key_r {
                            self.rotate_image_cw(fs_idx);
                        }
                        if key_l {
                            self.rotate_image_ccw(fs_idx);
                        }

                        if right && !ctrl_d {
                            nav_delta = 1;
                            self.slideshow_playing = false; // 手動操作で停止
                        }
                        if left && !ctrl_u {
                            nav_delta = -1;
                            self.slideshow_playing = false;
                        }
                        if ctrl_d {
                            ctrl_nav = Some(1);
                        }
                        if ctrl_u {
                            ctrl_nav = Some(-1);
                        }

                        // ── ホイール操作 ──────────────────────────
                        // メタデータパネル上ではパネル内スクロール、それ以外は画像ナビゲーション
                        let panel_w = 380.0_f32.min(full_rect.width() * 0.5);
                        let panel_left = full_rect.max.x - panel_w;
                        let hover_threshold = full_rect.max.x - full_rect.width() * 0.25;
                        let cursor_in_panel = ctx.input(|i| {
                            i.pointer.hover_pos().map(|p| {
                                p.x > panel_left
                                    && p.y >= 60.0 // 上部バー領域は除外
                                    && (self.show_metadata_panel || p.x > hover_threshold)
                            }).unwrap_or(false)
                        });

                        let wheel_y = ctx.input(|i| i.raw_scroll_delta.y);
                        if wheel_y.abs() > 0.5 && !cursor_in_panel {
                            // パネル外: ホイールで画像前後移動
                            ctx.input_mut(|i| {
                                i.raw_scroll_delta = egui::Vec2::ZERO;
                                i.smooth_scroll_delta = egui::Vec2::ZERO;
                                i.events
                                    .retain(|e| !matches!(e, egui::Event::MouseWheel { .. }));
                            });
                            nav_delta = if wheel_y < 0.0 { 1 } else { -1 };
                        }
                        // パネル上: ホイールイベントを消費せず、ScrollArea に委ねる

                        // ── マウスクリック操作 ────────────────────
                        let fs_response = ui.interact(
                            full_rect,
                            egui::Id::new("fs_click"),
                            egui::Sense::click(),
                        );
                        if is_video {
                            // 動画: クリックで外部プレイヤー起動
                            if fs_response.clicked() {
                                if let Some(ref vp) = video_path {
                                    open_external_player(vp);
                                }
                            }
                        } else {
                            // 画像 / セパレータ: 左半分 → 前、右半分 → 次
                            // パネル表示領域のクリックはナビゲーションしない
                            if fs_response.clicked() {
                                if let Some(pos) = fs_response.interact_pointer_pos() {
                                    let panel_threshold = full_rect.max.x - full_rect.width() * 0.25;
                                    let in_panel = pos.y >= 60.0
                                        && (self.show_metadata_panel
                                            || pos.x > panel_threshold)
                                        && pos.x > full_rect.max.x - 380.0_f32.min(full_rect.width() * 0.5);
                                    if !in_panel {
                                        if pos.x > full_rect.center().x {
                                            nav_delta = 1;
                                        } else {
                                            nav_delta = -1;
                                        }
                                    }
                                }
                            }
                        }
                        // 右クリックでフルスクリーン終了 → サムネイル一覧に戻る
                        if fs_response.secondary_clicked() {
                            close_fs = true;
                        }

                        // ── 画像 / 動画 / セパレータ表示 ──────────
                        if let Some(sep) = separator_text.as_ref() {
                            Self::draw_fs_separator(ui, full_rect, sep);
                        } else {
                            let fs_rotation = self.get_rotation(fs_idx);
                            Self::draw_fs_image(
                                ui,
                                full_rect,
                                tex.as_ref(),
                                thumb_tex.as_ref(),
                                is_video,
                                fs_load_failed,
                                fs_rotation,
                            );
                        }

                        // ── 動画: 再生ボタンオーバーレイ ─────────
                        if is_video {
                            draw_play_icon(ui.painter(), full_rect.center(), 56.0);
                            // Enter キーでも起動
                            let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
                            if enter {
                                if let Some(ref vp) = video_path {
                                    open_external_player(vp);
                                }
                            }
                        }

                        // サムネイル仮表示中 → 高解像度読み込み中インジケーター（画像のみ）
                        // セパレータでない + 何らかのテクスチャが表示されている場合のみ表示
                        let has_any_tex = tex.is_some() || thumb_tex.is_some();
                        if is_loading && has_any_tex {
                            ui.painter().text(
                                full_rect.min + egui::vec2(16.0, 16.0),
                                egui::Align2::LEFT_TOP,
                                "高解像度 読込中...",
                                egui::FontId::proportional(14.0),
                                egui::Color32::from_rgba_unmultiplied(220, 220, 220, 180),
                            );
                        }

                        // ── メタデータパネル (画像の上にオーバーレイ) ──
                        // 右パネル表示中は上部バーも強制表示する
                        let right_panel_visible =
                            self.draw_metadata_panel(ui, ctx, full_rect);

                        // ── ホバー時のトップバー ──
                        let mut bar_rotate_cw = false;
                        let mut bar_rotate_ccw = false;
                        Self::draw_fs_hover_bar(
                            ui,
                            ctx,
                            full_rect,
                            &folder_display,
                            &filename,
                            image_dims,
                            image_file_size,
                            &mut close_fs,
                            &mut nav_delta,
                            &mut self.show_metadata_panel,
                            right_panel_visible,
                            &mut self.slideshow_playing,
                            &mut self.settings.slideshow_interval_secs,
                            &mut bar_rotate_cw,
                            &mut bar_rotate_ccw,
                        );
                        // ホバーバーの回転ボタンが押された場合
                        if bar_rotate_cw {
                            self.rotate_image_cw(fs_idx);
                        }
                        if bar_rotate_ccw {
                            self.rotate_image_ccw(fs_idx);
                        }
                    });
            },
        );

        // ── フルスクリーン終了・ナビゲーション処理 ────────────────
        if close_fs || ctrl_nav.is_some() {
            self.close_fullscreen();
        }
        if let Some(delta) = ctrl_nav {
            // Ctrl+↑↓: フォルダを移動してサムネイルモードに戻る（仕様 §7.2）
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
            // ←→↑↓: 画像・動画を前後に切り替え
            if let Some(new_idx) =
                crate::ui_helpers::adjacent_navigable_idx(&self.items, &self.visible_indices, fs_idx, nav_delta)
            {
                self.open_fullscreen(new_idx);
                self.selected = Some(new_idx);
                self.scroll_to_selected = true;
                self.update_last_selected_image();
            }
        }

        // ── スライドショー タイマー ─────────────────────────────────
        if self.slideshow_playing && !close_fs {
            let now = std::time::Instant::now();
            if now >= self.slideshow_next_at {
                // 次の画像に切り替え（ループ）
                if let Some(cur) = self.fullscreen_idx {
                    let next = crate::ui_helpers::adjacent_navigable_idx(&self.items, &self.visible_indices, cur, 1);
                    let target = match next {
                        Some(idx) => idx,
                        None => {
                            // 末尾に到達 → visible_indices 内の先頭画像に戻る（ループ）
                            self.visible_indices
                                .iter()
                                .copied()
                                .find(|&i| {
                                    matches!(
                                        self.items.get(i),
                                        Some(GridItem::Image(_)) | Some(GridItem::ZipImage { .. })
                                    )
                                })
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
            // タイマーが発火するまで再描画を継続
            let remaining = self.slideshow_next_at.saturating_duration_since(now);
            ctx.request_repaint_after(remaining);
        }

        // 高解像度読み込み完了まで毎フレーム再描画（画像のみ）
        let image_loading = !is_video
            && self
                .fullscreen_idx
                .map(|i| !self.fs_cache.contains_key(&i))
                .unwrap_or(false);
        if image_loading {
            ctx.request_repaint();
        }

        // アニメーション: 次フレームの時刻まで待ってから再描画
        if !is_video {
            if let Some(FsCacheEntry::Animated {
                next_frame_at, ..
            }) = self.fs_cache.get(&fs_idx)
            {
                let delay = (next_frame_at - ctx.input(|i| i.time)).max(0.0);
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay));
            }
        }
    }

    // ── フルスクリーン描画ヘルパー ──────────────────────────────────

    /// ZIP セパレータの章タイトル画面を描画する。
    fn draw_fs_separator(ui: &mut egui::Ui, full_rect: egui::Rect, sep: &str) {
        let title_size = (full_rect.height() * 0.12).clamp(48.0, 120.0);
        let sub_size = (full_rect.height() * 0.030).clamp(20.0, 36.0);

        // 控えめな背景ハイライト (フォルダ名の周囲のみ)
        ui.painter().rect_filled(
            egui::Rect::from_center_size(
                full_rect.center(),
                egui::vec2(full_rect.width() * 0.85, title_size * 2.2),
            ),
            16.0,
            egui::Color32::from_rgba_unmultiplied(30, 45, 80, 180),
        );
        // フォルダ名 (中央、大きく)
        ui.painter().text(
            full_rect.center(),
            egui::Align2::CENTER_CENTER,
            sep,
            egui::FontId::proportional(title_size),
            egui::Color32::WHITE,
        );
        // 「作品の区切り」案内 (画面下部)
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
        // 動画はサムネイルのみ表示。画像はフルサイズ優先。
        let display_tex = tex.or(thumb_tex);
        if let Some(handle) = display_tex {
            let tex_size = handle.size_vec2();
            // 90°/270° 回転時は幅と高さが入れ替わる
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
                    ui.painter(),
                    handle.id(),
                    img_rect,
                    rotation,
                );
            }
        } else if fs_load_failed {
            // デコード失敗
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
            // テクスチャ未ロード（サムネイルも未完了）
            ui.painter().text(
                full_rect.center(),
                egui::Align2::CENTER_CENTER,
                if is_video {
                    "動画サムネイル 読込中..."
                } else {
                    "読込中..."
                },
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
        // 画面上部 60px にマウスがあるとき、または右パネル表示中は常に表示
        let hover_in_top = ctx
            .input(|i| i.pointer.hover_pos().map(|p| p.y < 60.0).unwrap_or(false));
        if !hover_in_top && !force_show {
            return;
        }

        let bar_h = 44.0;
        let bar_rect =
            egui::Rect::from_min_size(full_rect.min, egui::vec2(full_rect.width(), bar_h));
        // 半透明の黒背景 (文字が読みやすいよう alpha 高め)
        ui.painter().rect_filled(
            bar_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200),
        );
        // 下端に薄い区切り線
        ui.painter().line_segment(
            [
                egui::pos2(bar_rect.min.x, bar_rect.max.y),
                egui::pos2(bar_rect.max.x, bar_rect.max.y),
            ],
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 60),
            ),
        );

        // ── × 閉じるボタン (右端) ──
        let btn_size = 32.0;
        let btn_margin = 6.0;
        let btn_rect = egui::Rect::from_min_size(
            egui::pos2(
                bar_rect.max.x - btn_size - btn_margin,
                bar_rect.min.y + btn_margin,
            ),
            egui::vec2(btn_size, btn_size),
        );
        let btn_resp = ui.interact(
            btn_rect,
            egui::Id::new("fs_close_btn"),
            egui::Sense::click(),
        );
        let bg = if btn_resp.hovered() {
            egui::Color32::from_rgba_unmultiplied(220, 50, 50, 230)
        } else {
            egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
        };
        ui.painter().rect_filled(btn_rect, 4.0, bg);
        let c = btn_rect.center();
        let r = btn_size * 0.25;
        let stroke = egui::Stroke::new(2.5, egui::Color32::WHITE);
        ui.painter().line_segment(
            [
                egui::pos2(c.x - r, c.y - r),
                egui::pos2(c.x + r, c.y + r),
            ],
            stroke,
        );
        ui.painter().line_segment(
            [
                egui::pos2(c.x + r, c.y - r),
                egui::pos2(c.x - r, c.y + r),
            ],
            stroke,
        );
        if btn_resp.clicked() {
            *close_fs = true;
        }
        // ×ボタンのクリックが背面の nav_delta に漏れないように上書き
        if btn_resp.hovered() {
            *nav_delta = 0;
        }

        // ── ▶/⏸ スライドショーボタン (× の左隣) ──
        let play_rect = egui::Rect::from_min_size(
            egui::pos2(
                btn_rect.min.x - btn_size - 4.0,
                bar_rect.min.y + btn_margin,
            ),
            egui::vec2(btn_size, btn_size),
        );
        let play_resp = ui.interact(
            play_rect,
            egui::Id::new("fs_play_btn"),
            egui::Sense::click(),
        );
        let play_bg = if *slideshow_playing {
            egui::Color32::from_rgba_unmultiplied(60, 180, 60, 200)
        } else if play_resp.hovered() {
            egui::Color32::from_rgba_unmultiplied(100, 100, 100, 200)
        } else {
            egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
        };
        ui.painter().rect_filled(play_rect, 4.0, play_bg);
        {
            let c = play_rect.center();
            let r = btn_size * 0.28;
            let p = ui.painter();
            if *slideshow_playing {
                // 一時停止: 2 本の縦線
                let bar_w = r * 0.3;
                let gap = r * 0.35;
                let stroke = egui::Stroke::new(bar_w, egui::Color32::WHITE);
                p.line_segment(
                    [egui::pos2(c.x - gap, c.y - r), egui::pos2(c.x - gap, c.y + r)],
                    stroke,
                );
                p.line_segment(
                    [egui::pos2(c.x + gap, c.y - r), egui::pos2(c.x + gap, c.y + r)],
                    stroke,
                );
            } else {
                // 再生: 右向き三角形
                let cx = c.x + r * 0.12;
                let points = vec![
                    egui::pos2(cx - r * 0.5, c.y - r * 0.75),
                    egui::pos2(cx - r * 0.5, c.y + r * 0.75),
                    egui::pos2(cx + r * 0.7, c.y),
                ];
                p.add(egui::Shape::convex_polygon(
                    points,
                    egui::Color32::WHITE,
                    egui::Stroke::NONE,
                ));
            }
        }
        if play_resp.clicked() {
            *slideshow_playing = !*slideshow_playing;
        }
        if play_resp.hovered() {
            *nav_delta = 0;
        }

        // ── ↷ 右回転ボタン (▶ の左隣) ──
        let rcw_rect = egui::Rect::from_min_size(
            egui::pos2(play_rect.min.x - btn_size - 4.0, bar_rect.min.y + btn_margin),
            egui::vec2(btn_size, btn_size),
        );
        let rcw_resp = ui.interact(rcw_rect, egui::Id::new("fs_rcw_btn"), egui::Sense::click());
        let rcw_bg = if rcw_resp.hovered() {
            egui::Color32::from_rgba_unmultiplied(100, 100, 100, 200)
        } else {
            egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
        };
        ui.painter().rect_filled(rcw_rect, 4.0, rcw_bg);
        draw_rotate_icon(ui.painter(), rcw_rect.center(), btn_size * 0.28, true);
        if rcw_resp.clicked() { *rotate_cw = true; }
        if rcw_resp.hovered() { *nav_delta = 0; }

        // ── ↶ 左回転ボタン (↷ の左隣) ──
        let rccw_rect = egui::Rect::from_min_size(
            egui::pos2(rcw_rect.min.x - btn_size - 4.0, bar_rect.min.y + btn_margin),
            egui::vec2(btn_size, btn_size),
        );
        let rccw_resp = ui.interact(rccw_rect, egui::Id::new("fs_rccw_btn"), egui::Sense::click());
        let rccw_bg = if rccw_resp.hovered() {
            egui::Color32::from_rgba_unmultiplied(100, 100, 100, 200)
        } else {
            egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
        };
        ui.painter().rect_filled(rccw_rect, 4.0, rccw_bg);
        draw_rotate_icon(ui.painter(), rccw_rect.center(), btn_size * 0.28, false);
        if rccw_resp.clicked() { *rotate_ccw = true; }
        if rccw_resp.hovered() { *nav_delta = 0; }

        // ── ℹ Info ボタン (↶ の左隣) ──
        let info_rect = egui::Rect::from_min_size(
            egui::pos2(
                rccw_rect.min.x - btn_size - 4.0,
                bar_rect.min.y + btn_margin,
            ),
            egui::vec2(btn_size, btn_size),
        );
        let info_resp = ui.interact(
            info_rect,
            egui::Id::new("fs_info_btn"),
            egui::Sense::click(),
        );
        let info_bg = if *show_info {
            egui::Color32::from_rgba_unmultiplied(80, 140, 220, 200)
        } else if info_resp.hovered() {
            egui::Color32::from_rgba_unmultiplied(100, 100, 100, 200)
        } else {
            egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
        };
        ui.painter().rect_filled(info_rect, 4.0, info_bg);
        {
            // ℹ アイコンの自前描画: 丸 + 縦線 + 点
            let c = info_rect.center();
            let r = btn_size * 0.28;
            let white = egui::Color32::WHITE;
            // 外円
            ui.painter().circle_stroke(c, r, egui::Stroke::new(1.5, white));
            // 縦線 (i の棒部分)
            let bar_w = r * 0.22;
            ui.painter().line_segment(
                [egui::pos2(c.x, c.y - r * 0.05), egui::pos2(c.x, c.y + r * 0.55)],
                egui::Stroke::new(bar_w, white),
            );
            // 点 (i の上の点)
            ui.painter().circle_filled(
                egui::pos2(c.x, c.y - r * 0.45),
                bar_w * 0.7,
                white,
            );
        }
        if info_resp.clicked() {
            *show_info = !*show_info;
        }
        if info_resp.hovered() {
            *nav_delta = 0;
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

        // ── 中央寄り (× の左): ファイル名 | 寸法 | サイズ ──
        let mut info_parts: Vec<String> = Vec::new();
        if !filename.is_empty() {
            info_parts.push(filename.to_string());
        }
        if let Some((w, h)) = image_dims {
            info_parts.push(format!("{w} × {h}"));
        }
        if let Some(bytes) = image_file_size {
            info_parts.push(format_bytes_small(bytes));
        }
        if !info_parts.is_empty() {
            let info_text = info_parts.join("    ");
            // info ボタンの左に配置
            let right_edge = info_rect.min.x - 12.0;
            ui.painter().text(
                egui::pos2(right_edge, bar_rect.center().y),
                egui::Align2::RIGHT_CENTER,
                info_text,
                egui::FontId::proportional(15.0),
                egui::Color32::WHITE,
            );
        }
    }
}

/// 回転アイコンを自前描画する。
/// 上部が開いた 270° の円弧 + 端に矢印。
/// `clockwise`: true=時計回り (R), false=反時計回り (L)。
fn draw_rotate_icon(painter: &egui::Painter, center: egui::Pos2, radius: f32, clockwise: bool) {
    let stroke = egui::Stroke::new(2.0, egui::Color32::WHITE);
    let n = 24;

    // 円弧: 角度系 (0°=右, 90°=下, 時計回り)
    // 開口部は下。start=225° (左下), end=315° (右下) の短い方ではなく、
    // start=315° → end=315°+270°=585°(=225°) で上を通る 270° の弧。
    let start_rad = 315.0_f32.to_radians(); // 右下
    let end_rad = (315.0 + 270.0_f32).to_radians(); // = 585° = 225° (左下)
    let arc_span = end_rad - start_rad; // 270°

    // 円弧の点を計算（常に時計回り方向 = 角度増加方向に描画）
    let mut points = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let angle = start_rad + arc_span * t;
        points.push(egui::pos2(
            center.x + radius * angle.cos(),
            center.y + radius * angle.sin(),
        ));
    }

    // 円弧を描画
    for i in 0..n {
        painter.line_segment([points[i], points[i + 1]], stroke);
    }

    // 矢印: どちらの端に付けるか
    // 開口部は下。start=315° (右下), end=225° (左下)。
    let (arrow_pt, tangent_x, tangent_y) = if clockwise {
        // 終端 (225° = 左下) に矢印。進行方向 = 時計回りの接線 = (sin θ, -cos θ)。
        let angle = end_rad;
        let tx = angle.sin();
        let ty = -angle.cos();
        (points[n], tx, ty)
    } else {
        // 始端 (315° = 右下) に矢印。進行方向 = 反時計回りの接線 = (-sin θ, cos θ)。
        let angle = start_rad;
        let tx = -angle.sin();
        let ty = angle.cos();
        (points[0], tx, ty)
    };

    // 矢印の 2 辺
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
