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

                        if esc {
                            close_fs = true;
                        }
                        if right && !ctrl_d {
                            nav_delta = 1;
                        }
                        if left && !ctrl_u {
                            nav_delta = -1;
                        }
                        if ctrl_d {
                            ctrl_nav = Some(1);
                        }
                        if ctrl_u {
                            ctrl_nav = Some(-1);
                        }

                        // ── ホイール操作 ──────────────────────────
                        // 下スクロール(delta<0) → 次の画像、上スクロール(delta>0) → 前の画像
                        let wheel_y = ctx.input(|i| i.raw_scroll_delta.y);
                        if wheel_y.abs() > 0.5 {
                            ctx.input_mut(|i| {
                                i.raw_scroll_delta = egui::Vec2::ZERO;
                                i.smooth_scroll_delta = egui::Vec2::ZERO;
                                i.events
                                    .retain(|e| !matches!(e, egui::Event::MouseWheel { .. }));
                            });
                            nav_delta = if wheel_y < 0.0 { 1 } else { -1 };
                        }

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
                            if fs_response.clicked() {
                                if let Some(pos) = fs_response.interact_pointer_pos() {
                                    if pos.x > full_rect.center().x {
                                        nav_delta = 1;
                                    } else {
                                        nav_delta = -1;
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
                            Self::draw_fs_image(
                                ui,
                                full_rect,
                                tex.as_ref(),
                                thumb_tex.as_ref(),
                                is_video,
                                fs_load_failed,
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

                        // ── ホバー時のトップバー ──
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
                        );
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
                crate::ui_helpers::adjacent_navigable_idx(&self.items, fs_idx, nav_delta)
            {
                self.open_fullscreen(new_idx);
                self.selected = Some(new_idx);
                self.scroll_to_selected = true;
                self.update_last_selected_image();
            }
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
    ) {
        // 動画はサムネイルのみ表示。画像はフルサイズ優先。
        let display_tex = tex.or(thumb_tex);
        if let Some(handle) = display_tex {
            let tex_size = handle.size_vec2();
            let scale = (full_rect.width() / tex_size.x).min(full_rect.height() / tex_size.y);
            let img_rect =
                egui::Rect::from_center_size(full_rect.center(), tex_size * scale);
            ui.painter().image(
                handle.id(),
                img_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
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
    ) {
        // 画面上部 60px にマウスがあるときだけ表示する
        let hover_in_top = ctx
            .input(|i| i.pointer.hover_pos().map(|p| p.y < 60.0).unwrap_or(false));
        if !hover_in_top {
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
            // ×ボタンの左に配置
            let right_edge = btn_rect.min.x - 12.0;
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
