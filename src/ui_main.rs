//! メイン画面の UI コンポーネント描画。
//!
//! `App::update()` から呼ばれるメニューバー・ツールバー・アドレスバー・
//! グリッド・進捗オーバーレイ・選択情報オーバーレイの描画メソッドを集約。

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use eframe::egui;

use crate::app::App;
use crate::grid_item::{GridItem, ThumbnailState};
use crate::ui_helpers::open_external_player;

// ── 進捗バー定数 ──
const PROGRESS_LABEL_COLOR: egui::Color32 = egui::Color32::from_rgb(235, 240, 250);
const PROGRESS_BG_COLOR: egui::Color32 = egui::Color32::from_rgba_premultiplied(20, 25, 35, 230);
const PROGRESS_NORMAL_COLOR: egui::Color32 = egui::Color32::from_rgb(60, 130, 220);
const PROGRESS_UPGRADE_COLOR: egui::Color32 = egui::Color32::from_rgb(100, 170, 240);

impl App {
    // ── メニューバー ─────────────────────────────────────────────────

    /// メニューバーを描画し、ナビゲーション先とソート変更の有無を返す。
    pub(crate) fn render_menubar(
        &mut self,
        ctx: &egui::Context,
    ) -> (Option<PathBuf>, bool) {
        let mut fav_nav: Option<PathBuf> = None;
        let mut settings_changed = false;
        let mut sort_changed = false;

        egui::TopBottomPanel::top("menubar").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("ファイル", |ui| {
                    if ui.button("フォルダを開く…").clicked() {
                        // 既に現在フォルダが設定されていれば初期値として補完
                        self.open_folder_input = self
                            .current_folder
                            .as_ref()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default();
                        self.show_open_folder_dialog = true;
                        ui.close();
                    }
                    if ui.button("メタデータ検索… (Ctrl+F)").clicked() {
                        self.show_search_bar = true;
                        self.search_focus_request = true;
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("終了").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                ui.menu_button("お気に入り", |ui| {
                    // このフォルダを追加 (クリック時は名称入力ダイアログを開く)
                    let can_add = self.current_folder.is_some();
                    if ui
                        .add_enabled(can_add, egui::Button::new("このフォルダを追加…"))
                        .clicked()
                    {
                        if let Some(ref folder) = self.current_folder.clone() {
                            // 既定の名前はフォルダ名から補完
                            let default_name = folder
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("")
                                .to_string();
                            self.fav_add_name_input = default_name;
                            self.fav_add_target = Some(folder.clone());
                            self.show_fav_add_dialog = true;
                        }
                        ui.close();
                    }

                    // 編集
                    if ui.button("編集").clicked() {
                        self.show_favorites_editor = true;
                        ui.close();
                    }

                    // キャッシュ作成
                    if ui.button("キャッシュ作成").clicked() {
                        self.cc.checked =
                            vec![false; self.settings.favorites.len()];
                        self.cc.running = false;
                        self.cc.result = None;
                        self.cc.total.store(0, Ordering::Relaxed);
                        self.cc.done.store(0, Ordering::Relaxed);
                        self.cc.cache_size.store(0, Ordering::Relaxed);
                        self.cc.finished.store(false, Ordering::Relaxed);
                        *self.cc.current.lock().unwrap() = String::new();
                        self.cc.show = true;
                        ui.close();
                    }

                    // 区切り線
                    ui.separator();

                    // 登録済みお気に入り一覧
                    if self.settings.favorites.is_empty() {
                        ui.label(egui::RichText::new("（未登録）").weak());
                    } else {
                        let favorites = self.settings.favorites.clone();
                        for fav in &favorites {
                            if ui.button(&fav.name).clicked() {
                                fav_nav = Some(fav.path.clone());
                                ui.close();
                            }
                        }
                    }
                });

                ui.menu_button("設定", |ui| {
                    ui.menu_button("サムネイル列数", |ui| {
                        for cols in crate::settings::MIN_GRID_COLS..=crate::settings::MAX_GRID_COLS {
                            let checked = self.settings.grid_cols == cols;
                            let prefix = if checked { "✓ " } else { "  " };
                            if ui.button(format!("{prefix}{cols} 列")).clicked() {
                                self.settings.grid_cols = cols;
                                settings_changed = true;
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button("サムネイル比率", |ui| {
                        for &aspect in crate::settings::ThumbAspect::all() {
                            let checked = self.settings.thumb_aspect == aspect;
                            let prefix = if checked { "✓ " } else { "  " };
                            if ui.button(format!("{prefix}{}", aspect.label())).clicked() {
                                self.settings.thumb_aspect = aspect;
                                settings_changed = true;
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button("ソート順", |ui| {
                        for &order in crate::settings::SortOrder::all() {
                            let checked = self.settings.sort_order == order;
                            let prefix = if checked { "✓ " } else { "  " };
                            if ui.button(format!("{prefix}{}", order.label())).clicked() {
                                self.settings.sort_order = order;
                                sort_changed = true;
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("キャッシュ管理").clicked() {
                        let cache_dir = crate::catalog::default_cache_dir();
                        self.cache_manager_stats =
                            Some(crate::catalog::cache_stats(&cache_dir));
                        self.cache_manager_result = None;
                        self.show_cache_manager = true;
                        ui.close();
                    }
                    if ui.button("サムネイル画質…").clicked() {
                        self.open_thumb_quality_dialog(ctx);
                        ui.close();
                    }
                    if ui.button("統計…").clicked() {
                        self.show_stats_dialog = true;
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("回転情報をリセット…").clicked() {
                        self.show_rotation_reset_confirm = true;
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("環境設定…").clicked() {
                        self.show_preferences = true;
                        ui.close();
                    }
                });

                ui.menu_button("ヘルプ", |ui| {
                    if ui.button("ヘルプサイトを開く").clicked() {
                        let url = format!(
                            "https://www.mikage.to/mimageviewer/manual/index.html?version={}",
                            env!("CARGO_PKG_VERSION"),
                        );
                        crate::ui_helpers::open_url(&url);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("ログフォルダを開く").clicked() {
                        let dir = crate::data_dir::logs_dir();
                        let _ = std::fs::create_dir_all(&dir);
                        crate::ui_helpers::open_external_player(&dir);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("バージョン情報").clicked() {
                        self.show_about_dialog = true;
                        ui.close();
                    }
                });
            });
        });

        if settings_changed {
            self.settings.save();
        }
        if sort_changed {
            self.settings.save();
            if let Some(path) = self.current_folder.clone() {
                // スクロール履歴を捨てて先頭から再ロード
                self.folder_history.remove(&path);
                self.load_folder(path);
            }
        }

        (fav_nav, sort_changed)
    }

    // ── 進捗バー ─────────────────────────────────────────────────────

    /// 進捗バーオーバーレイ（左下フローティング）を描画する。
    pub(crate) fn render_progress_overlay(&self, ctx: &egui::Context) {
        let ((cur_normal, peak_normal), (cur_upgrade, peak_upgrade)) =
            self.progress_snapshot();
        if peak_normal == 0 && peak_upgrade == 0 {
            return;
        }

        egui::Area::new("progress_overlay".into())
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(8.0, -8.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(PROGRESS_BG_COLOR)
                    .show(ui, |ui| {
                        if peak_normal > 0 {
                            let done = peak_normal.saturating_sub(cur_normal);
                            let progress = done as f32 / peak_normal as f32;
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("先読み    ")
                                        .monospace()
                                        .color(PROGRESS_LABEL_COLOR),
                                );
                                ui.add(
                                    egui::ProgressBar::new(progress)
                                        .desired_width(220.0)
                                        .fill(PROGRESS_NORMAL_COLOR)
                                        .text(
                                            egui::RichText::new(format!(
                                                "{} / {}",
                                                done, peak_normal
                                            ))
                                            .color(egui::Color32::BLACK),
                                        ),
                                );
                            });
                        }
                        if peak_upgrade > 0 {
                            let done = peak_upgrade.saturating_sub(cur_upgrade);
                            let progress = done as f32 / peak_upgrade as f32;
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("高画質化  ")
                                        .monospace()
                                        .color(PROGRESS_LABEL_COLOR),
                                );
                                ui.add(
                                    egui::ProgressBar::new(progress)
                                        .desired_width(220.0)
                                        .fill(PROGRESS_UPGRADE_COLOR)
                                        .text(
                                            egui::RichText::new(format!(
                                                "{} / {}",
                                                done, peak_upgrade
                                            ))
                                            .color(egui::Color32::BLACK),
                                        ),
                                );
                            });
                        }
                    });
            });
        // 進行中は毎フレーム再描画してバーをスムーズに更新
        ctx.request_repaint();
    }

    // ── ツールバー ───────────────────────────────────────────────────

    /// ツールバーを描画し、お気に入りナビゲーション先を返す。
    /// ソート変更があった場合はフォルダの再ロードも行う。
    pub(crate) fn render_toolbar(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        // Vec を先にクローンして borrow checker の制約を回避
        let tb_cols = self.settings.toolbar_cols_items.clone();
        let tb_aspects = self.settings.toolbar_aspect_items.clone();
        let tb_sorts = self.settings.toolbar_sort_items.clone();
        let show_cols = !tb_cols.is_empty();
        let show_aspect = !tb_aspects.is_empty();
        let show_sort = !tb_sorts.is_empty();
        let show_favs = self.settings.show_toolbar_favorites;
        let show_parent = self.settings.show_toolbar_parent_button;
        let any_toolbar_section = show_cols || show_aspect || show_sort || show_favs || show_parent;

        if !any_toolbar_section {
            return None;
        }

        let mut toolbar_fav_nav: Option<PathBuf> = None;
        let mut toolbar_sort_changed = false;
        let mut toolbar_parent_nav = false;

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                let mut first_section = true;
                if show_parent {
                    let has_parent = self
                        .current_folder
                        .as_ref()
                        .and_then(|p| p.parent())
                        .is_some();
                    if ui
                        .add_enabled(has_parent, egui::Button::new("⬆"))
                        .on_hover_text("上のフォルダへ (BS)")
                        .clicked()
                    {
                        toolbar_parent_nav = true;
                    }
                    first_section = false;
                }
                if show_cols {
                    if !first_section {
                        ui.separator();
                    }
                    ui.label("列:");
                    for &cols in &tb_cols {
                        let selected = self.settings.grid_cols == cols;
                        if ui
                            .selectable_label(selected, format!(" {cols} "))
                            .clicked()
                        {
                            self.settings.grid_cols = cols;
                            self.settings.save();
                        }
                    }
                    first_section = false;
                }
                if show_aspect {
                    if !first_section {
                        ui.separator();
                    }
                    ui.label("比率:");
                    for &aspect in &tb_aspects {
                        let selected = self.settings.thumb_aspect == aspect;
                        if ui.selectable_label(selected, aspect.label()).clicked() {
                            self.settings.thumb_aspect = aspect;
                            self.settings.save();
                        }
                    }
                    first_section = false;
                }
                if show_sort {
                    if !first_section {
                        ui.separator();
                    }
                    ui.label("ソート:");
                    for &order in &tb_sorts {
                        let selected = self.settings.sort_order == order;
                        if ui
                            .selectable_label(selected, order.short_label())
                            .clicked()
                            && !selected
                        {
                            self.settings.sort_order = order;
                            self.settings.save();
                            toolbar_sort_changed = true;
                        }
                    }
                    first_section = false;
                }
                if show_favs {
                    if !first_section {
                        ui.separator();
                    }
                    ui.label("お気に入り:");
                    if self.settings.favorites.is_empty() {
                        ui.label(egui::RichText::new("(未登録)").weak());
                    } else {
                        // 現在のフォルダと一致するお気に入りをハイライト
                        let current = self.current_folder.clone();
                        for fav in &self.settings.favorites {
                            let selected = current
                                .as_ref()
                                .map(|c| c == &fav.path)
                                .unwrap_or(false);
                            if ui
                                .selectable_label(selected, &fav.name)
                                .on_hover_text(fav.path.to_string_lossy())
                                .clicked()
                            {
                                toolbar_fav_nav = Some(fav.path.clone());
                            }
                        }
                    }
                }
            });
            ui.add_space(2.0);
        });

        // 親フォルダへ移動
        if toolbar_parent_nav {
            if let Some(ref cur) = self.current_folder.clone() {
                if let Some(parent) = cur.parent() {
                    self.select_after_load = cur
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|s| s.to_string());
                    return Some(parent.to_path_buf());
                }
            }
        }

        // ツールバーのソート変更は borrow の関係で遅延実行
        if toolbar_sort_changed {
            if let Some(path) = self.current_folder.clone() {
                self.folder_history.remove(&path);
                self.load_folder(path);
            }
        }

        toolbar_fav_nav
    }

    // ── アドレスバー ─────────────────────────────────────────────────

    /// アドレスバーを描画し、Enter で確定されたパスを返す。
    pub(crate) fn render_address_bar(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        if !self.settings.show_toolbar_folder {
            self.address_has_focus = false;
            return None;
        }

        egui::TopBottomPanel::top("address_bar")
            .show(ctx, |ui| -> Option<PathBuf> {
                ui.add_space(3.0);
                let mut result = None;
                ui.horizontal(|ui| {
                    ui.label("フォルダ:");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.address)
                            .desired_width(f32::INFINITY),
                    );
                    self.address_has_focus = resp.has_focus();
                    if resp.lost_focus()
                        && ctx.input(|i| i.key_pressed(egui::Key::Enter))
                    {
                        let p = PathBuf::from(&self.address);
                        if let Some(resolved) =
                            crate::folder_tree::resolve_openable_path(&p)
                        {
                            result = Some(resolved);
                        }
                    }
                });
                ui.add_space(3.0);
                result
            })
            .inner
    }

    // ── 検索バー ─────────────────────────────────────────────────────

    /// メタデータ検索バーを描画する。
    pub(crate) fn render_search_bar(&mut self, ctx: &egui::Context) {
        if !self.show_search_bar {
            return;
        }

        egui::TopBottomPanel::top("search_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("検索:");
                let response = ui.add_sized(
                    [240.0, 18.0],
                    egui::TextEdit::singleline(&mut self.search_query)
                        .hint_text("プロンプト・ファイル名…"),
                );

                // フォーカスリクエスト
                if self.search_focus_request {
                    self.search_focus_request = false;
                    response.request_focus();
                }

                // フォーカス状態を追跡
                self.search_has_focus = response.has_focus();

                // Enter で検索実行
                // TextEdit は Enter でフォーカスを失うので lost_focus() で検知
                if response.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    self.execute_search();
                    // フォーカスを外してカーソルキーでグリッド操作できるようにする
                    response.surrender_focus();
                    self.search_has_focus = false;
                }

                // × ボタン
                if ui.small_button("×").clicked() {
                    self.show_search_bar = false;
                    self.search_query.clear();
                    self.search_filter = None;
                    self.search_has_focus = false;
                    self.rebuild_visible_indices();
                }

                // Esc で検索解除（ダイアログが開いていない場合のみ）
                if !self.any_dialog_open()
                    && ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    self.show_search_bar = false;
                    self.search_query.clear();
                    self.search_filter = None;
                    self.search_has_focus = false;
                    self.rebuild_visible_indices();
                }

                // マッチ件数を同じ行に表示
                if let Some(ref filter) = self.search_filter {
                    let image_count = filter
                        .iter()
                        .filter(|&&i| {
                            matches!(
                                self.items.get(i),
                                Some(crate::grid_item::GridItem::Image(_))
                            )
                        })
                        .count();
                    let total_images = self
                        .items
                        .iter()
                        .filter(|it| matches!(it, crate::grid_item::GridItem::Image(_)))
                        .count();
                    ui.label(
                        egui::RichText::new(format!("{image_count}/{total_images} 件"))
                            .size(11.0)
                            .color(egui::Color32::from_gray(140)),
                    );
                }
            });
        });
    }

    // ── セルインタラクション ─────────────────────────────────────────

    /// グリッドセルのクリック・ダブルクリック・右クリックを処理する。
    /// ダブルクリックでフォルダに入る場合はそのパスを返す。
    fn handle_cell_interaction(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        cell_rect: egui::Rect,
        idx: usize,
    ) -> Option<PathBuf> {
        let response = ui.interact(
            cell_rect,
            ui.id().with(idx),
            egui::Sense::click(),
        );
        let mut nav = None;
        if response.clicked() {
            let ctrl = ctx.input(|i| i.modifiers.ctrl);
            if ctrl {
                // Ctrl+クリック: チェック ON/OFF トグル + 選択移動
                match self.items.get(idx) {
                    Some(GridItem::Image(_))
                    | Some(GridItem::Video(_))
                    | Some(GridItem::ZipImage { .. }) => {
                        if self.checked.contains(&idx) {
                            self.checked.remove(&idx);
                        } else {
                            self.checked.insert(idx);
                        }
                    }
                    _ => {}
                }
            }
            self.selected = Some(idx);
            self.update_last_selected_image();
        }
        if response.double_clicked() {
            match self.items.get(idx) {
                Some(GridItem::Folder(p))
                | Some(GridItem::ZipFile(p))
                | Some(GridItem::PdfFile(p)) => {
                    nav = Some(p.clone())
                }
                Some(GridItem::Image(_))
                | Some(GridItem::ZipImage { .. })
                | Some(GridItem::ZipSeparator { .. })
                | Some(GridItem::PdfPage { .. }) => {
                    self.open_fullscreen(idx)
                }
                Some(GridItem::Video(p)) => {
                    let vp = p.clone();
                    open_external_player(&vp);
                }
                None => {}
            }
        }
        // 右クリック → コンテキストメニュー
        if response.secondary_clicked() {
            self.selected = Some(idx);
            self.update_last_selected_image();
            self.context_menu_idx = Some(idx);
            self.context_menu_pos = ctx.input(|i| {
                i.pointer.interact_pos().unwrap_or_default()
            });
        }
        nav
    }

    // ── サムネイルグリッド ───────────────────────────────────────────

    /// サムネイルグリッドを描画し、フォルダナビゲーション先を返す。
    pub(crate) fn render_grid(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        let scroll_to = self.scroll_to_selected;
        self.scroll_to_selected = false;

        egui::CentralPanel::default()
            .show(ctx, |ui| -> Option<PathBuf> {
                if self.items.is_empty() {
                    let msg = if self.current_folder.is_some() {
                        "表示するファイルがありません"
                    } else {
                        "フォルダを入力して Enter キーを押してください"
                    };
                    let r = ui.centered_and_justified(|ui| {
                        ui.label(msg)
                    });
                    // 空フォルダでも右クリックでフォルダ操作可能にする
                    if r.inner.secondary_clicked() {
                        if self.current_folder.is_some() {
                            self.context_menu_idx = Some(usize::MAX); // 特殊値: フォルダ操作
                            self.context_menu_pos = ctx.input(|i| {
                                i.pointer.interact_pos().unwrap_or_default()
                            });
                        }
                    }
                    return None;
                }

                if self.visible_indices.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label("検索結果なし");
                    });
                    return None;
                }

                let cols = self.settings.grid_cols.max(1);
                let avail_w = ui.available_width();
                let cell_w = (avail_w / cols as f32).floor();
                let cell_h =
                    (cell_w * self.settings.thumb_aspect.height_ratio()).round().max(1.0);

                // ウィンドウリサイズやアスペクト比変更でセルサイズが変わった場合スナップし直す
                if (cell_w - self.last_cell_size).abs() > 0.5
                    || (cell_h - self.last_cell_h).abs() > 0.5
                {
                    self.scroll_offset_y =
                        (self.scroll_offset_y / cell_h).round() * cell_h;
                    self.last_cell_size = cell_w;
                    self.last_cell_h = cell_h;
                }

                if scroll_to {
                    self.apply_scroll_to_selected(cols, cell_h);
                }

                let total_rows = self.visible_indices.len().div_ceil(cols);
                let natural_h = total_rows as f32 * cell_h;

                // egui 内部の max offset = total_h - viewport_h が行境界に揃うよう、
                // total_h を拡張する。これにより egui と自前の行スナップが一致し振動を防ぐ。
                // 拡張量は最大 cell_h 未満（端数の補正のみ）。
                let total_h = if natural_h <= self.last_viewport_h {
                    natural_h
                } else {
                    let raw_max = natural_h - self.last_viewport_h;
                    let snapped_max = (raw_max / cell_h).ceil() * cell_h;
                    snapped_max + self.last_viewport_h
                };

                let max_offset = if total_h <= self.last_viewport_h {
                    0.0
                } else {
                    total_h - self.last_viewport_h
                };
                self.scroll_offset_y = self.scroll_offset_y.clamp(0.0, max_offset);

                let mut nav: Option<PathBuf> = None;

                // egui にスクロールを管理させず、自前の offset を毎フレーム注入する。
                // ただしスクロールバードラッグ時は egui 側のオフセットを読み戻す。
                let scroll_output = egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .vertical_scroll_offset(self.scroll_offset_y)
                    .show_viewport(ui, |ui, viewport| {
                        // ビューポート高さを記録（次フレームのスクロール計算に使う）
                        self.last_viewport_h = viewport.height();

                        let (content_rect, _) = ui.allocate_exact_size(
                            egui::vec2(avail_w, total_h),
                            egui::Sense::hover(),
                        );

                        let first_row = (viewport.min.y / cell_h) as usize;
                        let last_row =
                            ((viewport.max.y / cell_h) as usize + 2).min(total_rows);

                        // Phase 2b ワーカーへ現在の可視先頭アイテムを通知
                        let vis_first_idx = self
                            .visible_indices
                            .get(first_row * cols)
                            .copied()
                            .unwrap_or(0);
                        self.scroll_hint
                            .store(vis_first_idx, Ordering::Relaxed);

                        for row in first_row..last_row {
                            for col in 0..cols {
                                let vis_pos = row * cols + col;
                                if vis_pos >= self.visible_indices.len() {
                                    break;
                                }
                                let idx = self.visible_indices[vis_pos];

                                let cell_rect = egui::Rect::from_min_size(
                                    content_rect.min
                                        + egui::vec2(
                                            col as f32 * cell_w,
                                            row as f32 * cell_h,
                                        ),
                                    egui::vec2(cell_w, cell_h),
                                );

                                if let Some(n) =
                                    self.handle_cell_interaction(ui, ctx, cell_rect, idx)
                                {
                                    nav = Some(n);
                                }

                                let rot = self.get_rotation(idx);
                                crate::app::draw_cell(
                                    ui,
                                    cell_rect,
                                    self.selected == Some(idx),
                                    self.checked.contains(&idx),
                                    &self.items[idx],
                                    &self.thumbnails[idx],
                                    rot,
                                );

                                // 選択中セルの矩形を記録 (オーバーレイ配置用)
                                if self.selected == Some(idx) {
                                    self.selected_cell_rect = Some(cell_rect);
                                }
                            }
                        }

                        // グリッドの空白部分で右クリック → フォルダメニュー
                        // Sense::click() だと左クリックも消費するので、
                        // ポインタ位置を直接チェックする
                        let bg_right_clicked = ctx.input(|i| {
                            i.pointer.secondary_clicked()
                        });
                        if bg_right_clicked && self.context_menu_idx.is_none() {
                            if self.current_folder.is_some() {
                                self.context_menu_idx = Some(usize::MAX);
                                self.context_menu_pos = ctx.input(|i| {
                                    i.pointer.interact_pos().unwrap_or_default()
                                });
                            }
                        }
                    });

                // スクロールバードラッグによるオフセット変化を読み戻す。
                // egui が内部で管理するオフセットと自前オフセットを同期させる。
                // ただし行スナップによる端数差分で毎フレーム振動するのを防ぐため、
                // 1 行分 (cell_h) 以上ずれた場合のみ同期する。
                let egui_offset = scroll_output.state.offset.y;
                if (egui_offset - self.scroll_offset_y).abs() > cell_h * 0.5 {
                    self.scroll_offset_y = (egui_offset / cell_h).round() * cell_h;
                }

                nav
            })
            .inner
    }

    // ── 選択情報オーバーレイ ─────────────────────────────────────────

    /// 選択中アイテムの情報をセル直下に表示する。
    pub(crate) fn render_selection_info(&self, ctx: &egui::Context) {
        // フルスクリーン中は出さない (独自のホバーヘッダーを持つため)。
        if self.fullscreen_idx.is_some() {
            return;
        }

        let (Some(idx), Some(cell_rect)) = (self.selected, self.selected_cell_rect) else {
            return;
        };

        // ZipSeparator はスキップ
        if matches!(self.items.get(idx), Some(GridItem::ZipSeparator { .. }) | None) {
            return;
        }

        let name = self
            .items
            .get(idx)
            .map(|it| it.name().to_string())
            .unwrap_or_default();
        // 元画像のピクセル寸法 (ThumbnailState::Loaded.source_dims から取得)
        let dims_str = match self.thumbnails.get(idx) {
            Some(ThumbnailState::Loaded {
                source_dims: Some((w, h)),
                ..
            }) => Some(format!("{} × {}", w, h)),
            _ => None,
        };
        let text = match dims_str {
            Some(d) => format!("{}   {}", d, name),
            None => name,
        };

        // セル幅で配置: セルの左下を基点、セル幅に合わせる
        let cell_w = cell_rect.width();
        let area_pos = cell_rect.left_bottom() + egui::vec2(0.0, 4.0);

        egui::Area::new("selection_info".into())
            .order(egui::Order::Middle)
            .fixed_pos(area_pos)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(egui::Color32::from_rgba_unmultiplied(20, 25, 35, 230))
                    .show(ui, |ui| {
                        let inner_width = (cell_w - 12.0).max(40.0);
                        ui.set_min_width(inner_width);
                        ui.set_max_width(inner_width);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(text)
                                    .color(egui::Color32::WHITE)
                                    .monospace(),
                            )
                            .truncate(),
                        );
                    });
            });
    }
}
