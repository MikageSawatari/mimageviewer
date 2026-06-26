//! フルスクリーンのメタデータサイドパネル。
//!
//! AI 画像生成メタデータ (A1111/ComfyUI) と EXIF 撮影情報を右サイドパネルに表示する。

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::app::App;
use crate::exif_reader::{self, ExifInfo};
use crate::png_metadata::{A1111Metadata, AiMetadata, ComfyUIMetadata};
use crate::tag_ops::TagTarget;
use crate::xmp_reader::{self, XmpTweetInfo};

/// パネルタイトルバーの高さ
const TITLE_BAR_H: f32 = 32.0;
const LINK_COLOR: egui::Color32 = egui::Color32::from_rgb(115, 180, 255);

#[derive(Clone)]
struct TagPanelRow {
    label: Option<String>,
    note: Option<String>,
    targets: Vec<TagTarget>,
    tags_by_target: Vec<Vec<String>>,
}

#[derive(Clone)]
struct TagPanelChoice {
    name: String,
    tag_key: String,
    count: usize,
    pinned: bool,
    last_applied_at: i64,
}

impl TagPanelRow {
    fn disabled() -> Self {
        Self {
            label: None,
            note: None,
            targets: Vec::new(),
            tags_by_target: Vec::new(),
        }
    }
}

impl App {
    /// フルスクリーンでメタデータパネルをオーバーレイ描画する。
    /// 画像は常に `full_rect` 全体に表示し、パネルは画像の上に重ねる。
    ///
    /// 表示条件:
    /// - `I` キーまたはピン留めで固定表示 ON/OFF
    /// - マウスカーソルが画面右端のパネル幅内にあるときもホバー表示
    ///
    /// 右パネル表示中は上部バーも常に同時表示する。
    /// 右パネルは常に上部バーの下から開始する。
    ///
    /// 戻り値: 右パネルが表示中なら true（上部バーの強制表示に使う）
    /// オーバーレイモード用: ホバー判定なしで強制表示する。
    pub(crate) fn draw_metadata_panel_forced(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
    ) {
        self.draw_metadata_panel_inner(ui, ctx, full_rect, true);
    }

    pub(crate) fn draw_metadata_panel(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
    ) -> bool {
        self.draw_metadata_panel_inner(ui, ctx, full_rect, false)
    }

    fn draw_metadata_panel_inner(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        force_show: bool,
    ) -> bool {
        let panel_rect = crate::ui_fullscreen::metadata_panel_rect(full_rect);

        if !force_show {
            let activation_rect =
                crate::ui_fullscreen::metadata_panel_hover_activation_rect(full_rect);

            // ホバー判定: 描画される右パネル幅内。カーソル非表示中は最後の座標が stale なので、
            // 実入力でカーソルが復帰するまでは passive hover でパネルを開かない。
            let pointer_pos = ctx.input(|i| {
                if self.cursor_hidden {
                    None
                } else {
                    i.pointer.hover_pos()
                }
            });

            let hover_in_right = pointer_pos.is_some_and(|p| activation_rect.contains(p));
            let hover_in_open_panel = self.metadata_panel_hover_active
                && pointer_pos.is_some_and(|p| panel_rect.contains(p));
            let hover_visible = hover_in_right || hover_in_open_panel;

            let visible =
                self.show_metadata_panel || self.fullscreen_tag_picker_open || hover_visible;
            self.metadata_panel_hover_active = !self.show_metadata_panel && hover_visible;
            if !visible {
                return false;
            }
        } else {
            self.metadata_panel_hover_active = false;
        }

        // パネル背景
        ui.painter().rect_filled(
            panel_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(18, 18, 22, 230),
        );
        // 左端に区切り線
        ui.painter().line_segment(
            [panel_rect.left_top(), panel_rect.left_bottom()],
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40),
            ),
        );

        // パネルのクリックイベントを消費
        let _ = ui.interact(
            panel_rect,
            egui::Id::new("metadata_panel_bg"),
            egui::Sense::click(),
        );

        // ── タイトルバー (ピン留めボタン付き) ──
        let title_rect =
            egui::Rect::from_min_size(panel_rect.min, egui::vec2(panel_rect.width(), TITLE_BAR_H));
        // タイトルバー背景 (やや明るめ)
        ui.painter().rect_filled(
            title_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(30, 30, 38, 240),
        );
        // 下端の区切り線
        ui.painter().line_segment(
            [
                egui::pos2(title_rect.min.x, title_rect.max.y),
                egui::pos2(title_rect.max.x, title_rect.max.y),
            ],
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 30),
            ),
        );

        // タイトルテキスト
        ui.painter().text(
            egui::pos2(title_rect.min.x + 10.0, title_rect.center().y),
            egui::Align2::LEFT_CENTER,
            "Image Info",
            egui::FontId::proportional(13.0),
            egui::Color32::from_gray(200),
        );

        // ピン留めボタン (右端)
        let pin_size = 22.0;
        let pin_margin = 5.0;
        let pin_rect = egui::Rect::from_min_size(
            egui::pos2(
                title_rect.max.x - pin_size - pin_margin,
                title_rect.min.y + (TITLE_BAR_H - pin_size) * 0.5,
            ),
            egui::vec2(pin_size, pin_size),
        );
        let pin_resp = ui.interact(
            pin_rect,
            egui::Id::new("metadata_pin_btn"),
            egui::Sense::click(),
        );
        let pin_bg = if self.show_metadata_panel {
            egui::Color32::from_rgba_unmultiplied(80, 140, 220, 200)
        } else if pin_resp.hovered() {
            egui::Color32::from_rgba_unmultiplied(100, 100, 100, 200)
        } else {
            egui::Color32::TRANSPARENT
        };
        ui.painter().rect_filled(pin_rect, 3.0, pin_bg);
        // ピンアイコン (📌 の代わりにシンプルなテキスト)
        ui.painter().text(
            pin_rect.center(),
            egui::Align2::CENTER_CENTER,
            if self.show_metadata_panel {
                "📌"
            } else {
                "📌"
            },
            egui::FontId::proportional(14.0),
            if self.show_metadata_panel {
                egui::Color32::WHITE
            } else {
                egui::Color32::from_gray(140)
            },
        );
        let pin_resp = pin_resp.on_hover_text(if self.show_metadata_panel {
            "常時表示を解除 [I / Tab]"
        } else {
            "常時表示に固定 [I / Tab]"
        });
        if pin_resp.clicked() {
            self.show_metadata_panel = !self.show_metadata_panel;
            self.metadata_panel_hover_active = !self.show_metadata_panel
                && ctx.input(|i| {
                    !self.cursor_hidden
                        && i.pointer
                            .hover_pos()
                            .is_some_and(|p| panel_rect.contains(p))
                });
        }

        // ── コンテンツ領域 (タイトルバーの下) ──
        let content_top = title_rect.max.y;
        let content_rect =
            egui::Rect::from_min_max(egui::pos2(panel_rect.min.x, content_top), panel_rect.max);

        let ai_metadata = self.get_current_ai_metadata();
        let exif_info = self.get_current_exif();
        let tweet_info = self.get_current_tweet_info();
        let sidecar_info = self.get_current_sidecar();
        let current_palette = self.current_fullscreen_color_palette();

        // タグパネル用の情報を先に集める (child_ui の &mut ui closure 前に借用を解消するため)
        let tag_rows = self.collect_fullscreen_tag_panel_rows();
        self.sync_fullscreen_tag_panel_state(&tag_rows);
        let tag_catalog: Vec<_> = self
            .cached_tag_choice_catalog()
            .into_iter()
            .map(|tag| TagPanelChoice {
                name: tag.name,
                tag_key: tag.tag_key,
                count: tag.count,
                pinned: tag.pinned,
                last_applied_at: tag.last_applied_at,
            })
            .collect();
        let pinned_tags: Vec<_> = self
            .settings
            .tags
            .iter()
            .filter(|tag| tag.show_shortcut)
            .map(|tag| TagPanelChoice {
                name: tag.name.clone(),
                tag_key: tag.tag_key.clone(),
                count: tag_catalog
                    .iter()
                    .find(|choice| choice.tag_key == tag.tag_key)
                    .map(|choice| choice.count)
                    .unwrap_or(0),
                pinned: true,
                last_applied_at: tag_catalog
                    .iter()
                    .find(|choice| choice.tag_key == tag.tag_key)
                    .map(|choice| choice.last_applied_at)
                    .unwrap_or(0),
            })
            .collect();
        let visible_tag_choices = tag_panel_visible_choices(
            &pinned_tags,
            &tag_rows,
            &self.fullscreen_tag_panel_sticky_tags,
            &tag_catalog,
        );
        let show_tag_panel =
            !visible_tag_choices.is_empty() || tag_rows.iter().any(|row| !row.targets.is_empty());

        // タグボタンクリックを closure 内で検出し、後段で self の操作に流す。
        let mut clicked_tag: Option<(String, Vec<TagTarget>)> = None;
        let mut set_tag: Option<(String, bool, Vec<TagTarget>)> = None;
        let mut searched_tag: Option<String> = None;
        let mut clicked_palette_rgb: Option<[u8; 3]> = None;
        let tag_picker_enter_pressed = self.dialog_enter_pressed(ctx);
        let tag_picker_escape_pressed = self.dialog_escape_pressed(ctx);
        let tag_picker_ime_active = self.ime_input_active();
        if self.fullscreen_tag_picker_open && tag_picker_escape_pressed {
            self.fullscreen_tag_picker_open = false;
            self.fullscreen_tag_picker_input.clear();
            self.fullscreen_tag_picker_row_key = None;
            self.fullscreen_tag_picker_focus_request = false;
            self.fullscreen_tag_picker_recent_tab = false;
        }

        let inner_rect = content_rect.shrink2(egui::vec2(12.0, 8.0));
        let mut child_ui = ui.new_child(egui::UiBuilder::new().max_rect(inner_rect));
        child_ui.set_clip_rect(content_rect);
        apply_metadata_panel_dark_widget_style(&mut child_ui);
        // Metadata values often contain long CJK text, URLs, and hashes. Use a
        // solid scrollbar here so egui reserves a real gutter instead of
        // drawing the default floating bar on top of the text.
        child_ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();

        egui::ScrollArea::vertical()
            .id_salt("metadata_scroll")
            .auto_shrink([false, false])
            .show(&mut child_ui, |ui| {
                ui.set_width(ui.available_width());

                if self.fullscreen_tag_picker_open {
                    draw_fullscreen_tag_picker_panel(
                        ui,
                        &tag_catalog,
                        &tag_rows,
                        &mut self.fullscreen_tag_picker_open,
                        &mut self.fullscreen_tag_picker_input,
                        &mut self.fullscreen_tag_picker_row_key,
                        &mut self.fullscreen_tag_picker_focus_request,
                        &mut self.fullscreen_tag_picker_recent_tab,
                        tag_picker_enter_pressed,
                        tag_picker_ime_active,
                        &mut set_tag,
                    );
                    return;
                }

                // ── タグパネル (最上段) ──
                // ピン留めタグと、表示対象に付いている未ピン留めタグを ON/OFF ボタンで並べる。
                if show_tag_panel {
                    draw_tag_panel(
                        ui,
                        &visible_tag_choices,
                        &tag_rows,
                        &mut self.fullscreen_tag_picker_open,
                        &mut self.fullscreen_tag_picker_input,
                        &mut self.fullscreen_tag_picker_row_key,
                        &mut self.fullscreen_tag_picker_focus_request,
                        &mut self.fullscreen_tag_picker_recent_tab,
                        &mut clicked_tag,
                        &mut searched_tag,
                    );
                    if tweet_info.is_some()
                        || ai_metadata.is_some()
                        || exif_info.is_some()
                        || sidecar_info.is_some()
                        || current_palette.is_some()
                    {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);
                    }
                }

                // 画像色パレット
                if let Some(ref palette) = current_palette {
                    draw_image_color_palette_section(ui, palette, &mut clicked_palette_rgb);
                    if tweet_info.is_some()
                        || ai_metadata.is_some()
                        || exif_info.is_some()
                        || sidecar_info.is_some()
                    {
                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(8.0);
                    }
                }

                // X ツイート情報 (mXD 由来)
                if let Some(ref t) = tweet_info {
                    draw_tweet_panel(ui, ctx, t);
                    if ai_metadata.is_some() || exif_info.is_some() {
                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(8.0);
                    }
                }

                // AI メタデータセクション
                match ai_metadata {
                    Some(AiMetadata::A1111(ref meta)) => {
                        draw_a1111_panel(ui, ctx, meta);
                    }
                    Some(AiMetadata::ComfyUI(ref meta)) => {
                        let show_raw_prompt = self.metadata_show_raw_prompt;
                        let show_raw_workflow = self.metadata_show_raw_workflow;
                        let (new_rp, new_rw) =
                            draw_comfyui_panel(ui, ctx, meta, show_raw_prompt, show_raw_workflow);
                        self.metadata_show_raw_prompt = new_rp;
                        self.metadata_show_raw_workflow = new_rw;
                    }
                    Some(AiMetadata::Unknown(ref chunks)) => {
                        draw_unknown_panel(ui, chunks);
                    }
                    None => {}
                }

                // EXIF セクション
                if let Some(ref exif) = exif_info {
                    if ai_metadata.is_some() {
                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(8.0);
                    }
                    draw_exif_panel(ui, exif, &mut self.exif_sections_open);
                }

                // 外部メタデータ (サイドカー) セクション (FS 画像のみ。docs §11)
                if let Some(ref sc) = sidecar_info {
                    if ai_metadata.is_some()
                        || exif_info.is_some()
                        || tweet_info.is_some()
                        || show_tag_panel
                    {
                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(8.0);
                    }
                    draw_sidecar_section(ui, sc);
                }

                // 何もない場合
                if ai_metadata.is_none()
                    && exif_info.is_none()
                    && tweet_info.is_none()
                    && !show_tag_panel
                    && sidecar_info.is_none()
                    && current_palette.is_none()
                {
                    draw_no_metadata(ui);
                }
            });

        // タグボタンクリックの後処理 (closure 外で self を可変借用する)
        if let Some((tag_name, targets)) = clicked_tag {
            self.request_tag_toggle_for_targets(&tag_name, targets);
        }
        if let Some((tag_name, add, targets)) = set_tag {
            if add {
                self.request_tag_add_for_targets(&tag_name, targets);
            } else {
                self.request_tag_remove_for_targets(&tag_name, targets);
            }
        }
        if let Some(tag_name) = searched_tag {
            self.open_tag_view_for_tag(&tag_name);
        }
        if let Some(rgb) = clicked_palette_rgb {
            self.apply_image_color_filter_from_swatch(rgb, ctx);
        }

        true
    }

    /// 現在のフルスクリーン画像の AI メタデータを取得する。
    fn get_current_ai_metadata(&self) -> Option<AiMetadata> {
        let idx = self.fullscreen_idx?;
        let key = self.metadata_cache_key(idx)?;
        self.metadata_cache.get(&key).cloned().flatten()
    }

    /// 現在のフルスクリーン画像の EXIF 情報を取得する。
    fn get_current_exif(&self) -> Option<ExifInfo> {
        let idx = self.fullscreen_idx?;
        let key = self.metadata_cache_key(idx)?;
        self.exif_cache.get(&key).cloned().flatten()
    }

    /// 現在のフルスクリーン画像の XMP (X/Twitter) 情報を取得する。
    fn get_current_tweet_info(&self) -> Option<XmpTweetInfo> {
        let idx = self.fullscreen_idx?;
        let key = self.metadata_cache_key(idx)?;
        self.xmp_cache.get(&key).cloned().flatten()
    }

    /// 現在のフルスクリーン画像の外部メタデータサイドカー (表示用) を取得する。
    /// worker (`run_metadata_load`) が FS 画像のみ埋めるので、動画 / ZIP / PDF は常に None。
    fn get_current_sidecar(&self) -> Option<crate::external_metadata::SidecarDisplay> {
        let idx = self.fullscreen_idx?;
        let key = self.metadata_cache_key(idx)?;
        self.sidecar_display_cache.get(&key).cloned().flatten()
    }

    fn collect_fullscreen_tag_panel_rows(&mut self) -> Vec<TagPanelRow> {
        let Some(idx) = self.fullscreen_idx else {
            return vec![TagPanelRow::disabled()];
        };

        if let crate::ui_fullscreen::SpreadPair::Double { left, right } =
            self.resolve_visible_spread_pair(idx)
        {
            if let Some(row) = self.container_spread_tag_panel_row(left, right) {
                return vec![row];
            }
            if let Some(rows) = self.normal_image_tag_panel_rows(&[left, right]) {
                return rows;
            }
        }

        if let Some(rows) = self.normal_image_tag_panel_rows(&[idx]) {
            return rows;
        }

        self.single_tag_panel_row(idx)
            .map(|row| vec![row])
            .unwrap_or_else(|| vec![TagPanelRow::disabled()])
    }

    fn single_tag_panel_row(&mut self, idx: usize) -> Option<TagPanelRow> {
        let target = self.tag_target_for_index(idx, true)?;
        let note = self.tag_target_note_for_item(idx, &target.path);
        Some(self.build_tag_panel_row(None, note, vec![target]))
    }

    fn container_spread_tag_panel_row(&mut self, left: usize, right: usize) -> Option<TagPanelRow> {
        let left_target = self.tag_target_for_index(left, true)?;
        let right_target = self.tag_target_for_index(right, true)?;
        if !crate::folder_tree::path_eq(&left_target.path, &right_target.path) {
            return None;
        }
        let note = self.tag_target_note_for_item(left, &left_target.path);
        Some(self.build_tag_panel_row(None, note, vec![left_target]))
    }

    fn normal_image_tag_panel_rows(&mut self, indices: &[usize]) -> Option<Vec<TagPanelRow>> {
        if indices.is_empty() {
            return None;
        }
        let mut image_paths = Vec::with_capacity(indices.len());
        for &idx in indices {
            match self.items.get(idx) {
                Some(crate::grid_item::GridItem::Image(path)) => image_paths.push(path.clone()),
                _ => return None,
            }
        }

        let shared_parent = shared_parent_folder(&image_paths)?;
        let folder_path = self
            .current_folder
            .clone()
            .filter(|path| crate::folder_tree::path_eq(path, &shared_parent))?;
        let folder_name = tag_path_display_name(&folder_path);
        let folder_target = self.tag_target_for_path(folder_path, false);
        let mut rows = vec![self.build_tag_panel_row(
            Some("フォルダ".to_string()),
            Some(format!("タグ対象: {folder_name}")),
            vec![folder_target],
        )];

        let mut page_targets = Vec::new();
        for &idx in indices {
            if let Some(target) = self.tag_target_for_index(idx, true) {
                page_targets.push(target);
            }
        }
        if !page_targets.is_empty() {
            let page_note = if page_targets.len() >= 2 {
                "タグ対象: 表示中の2ページ"
            } else {
                "タグ対象: 現在のページ"
            };
            rows.push(self.build_tag_panel_row(
                Some("ページ".to_string()),
                Some(page_note.to_string()),
                page_targets,
            ));
        }
        Some(rows)
    }

    fn build_tag_panel_row(
        &mut self,
        label: Option<String>,
        note: Option<String>,
        mut targets: Vec<TagTarget>,
    ) -> TagPanelRow {
        dedup_tag_targets(&mut targets);
        let paths: Vec<PathBuf> = targets.iter().map(|target| target.path.clone()).collect();
        self.hydrate_tags_cache_for_paths(&paths);
        let tags_by_target = paths
            .iter()
            .map(|path| {
                let key = crate::tags_db::item_key_for_path(path);
                self.tags_cache.get(&key).cloned().unwrap_or_default()
            })
            .collect();
        TagPanelRow {
            label,
            note,
            targets,
            tags_by_target,
        }
    }

    fn tag_target_note_for_item(&self, idx: usize, target_path: &Path) -> Option<String> {
        use crate::grid_item::GridItem;

        let target_kind = match self.items.get(idx)? {
            GridItem::ZipImage { .. }
            | GridItem::ZipFile(_)
            | GridItem::ConvertibleArchive { .. } => "この本",
            GridItem::PdfPage { .. } | GridItem::PdfFile(_) => "このPDF",
            _ => return None,
        };
        Some(format!(
            "タグ対象: {target_kind} ({})",
            tag_path_display_name(target_path)
        ))
    }

    fn sync_fullscreen_tag_panel_state(&mut self, rows: &[TagPanelRow]) {
        let target_key = tag_panel_target_key(rows);
        if self.fullscreen_tag_panel_target_key.as_deref() != Some(target_key.as_str()) {
            self.fullscreen_tag_panel_target_key = Some(target_key);
            self.fullscreen_tag_panel_sticky_tags.clear();
            self.fullscreen_tag_picker_open = false;
            self.fullscreen_tag_picker_input.clear();
            self.fullscreen_tag_picker_row_key = None;
            self.fullscreen_tag_picker_focus_request = false;
        }

        for row in rows {
            for tags in &row.tags_by_target {
                for tag in tags {
                    let tag_key = crate::tags_db::normalize_tag_key(tag);
                    if tag_key.is_empty()
                        || self
                            .settings
                            .tags
                            .iter()
                            .any(|def| def.show_shortcut && def.tag_key == tag_key)
                        || self
                            .fullscreen_tag_panel_sticky_tags
                            .iter()
                            .any(|(_, key)| key == &tag_key)
                    {
                        continue;
                    }
                    let name = self
                        .settings
                        .tags
                        .iter()
                        .find(|def| def.tag_key == tag_key)
                        .map(|def| def.name.clone())
                        .unwrap_or_else(|| crate::tags_db::strip_display_hash(tag).to_string());
                    self.fullscreen_tag_panel_sticky_tags.push((name, tag_key));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 描画ヘルパー
// ---------------------------------------------------------------------------

const LABEL_COLOR: egui::Color32 = egui::Color32::from_rgb(140, 160, 200);
const TEXT_COLOR: egui::Color32 = egui::Color32::from_rgb(230, 230, 230);
const DIM_COLOR: egui::Color32 = egui::Color32::from_rgb(150, 150, 150);
const JSON_COLOR: egui::Color32 = egui::Color32::from_rgb(190, 200, 210);
const SECTION_FONT: f32 = 14.0;
const BODY_FONT: f32 = 13.0;

fn apply_metadata_panel_dark_widget_style(ui: &mut egui::Ui) {
    let visuals = &mut ui.style_mut().visuals;
    visuals.extreme_bg_color = egui::Color32::from_rgb(28, 31, 38);
    visuals.selection.bg_fill = egui::Color32::from_rgb(58, 84, 116);
    visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(190, 215, 245));

    visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(42, 46, 56);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(48, 53, 64);
    visuals.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(88, 98, 118));
    visuals.widgets.inactive.fg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(232, 236, 244));

    visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(54, 60, 74);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(62, 70, 86);
    visuals.widgets.hovered.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(124, 144, 176));
    visuals.widgets.hovered.fg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(248, 250, 255));

    visuals.widgets.active.weak_bg_fill = egui::Color32::from_rgb(62, 72, 92);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(72, 86, 110);
    visuals.widgets.active.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(154, 178, 214));
    visuals.widgets.active.fg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 255, 255));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TagButtonState {
    Off,
    Mixed,
    On,
}

/// タグパネル描画 (docs/tag-feature.md §4.4)。
///
/// 登録タグを ON/OFF ボタンで横並び表示。各ボタンの外観:
/// - ON (現在のファイルに付与済み): 緑背景 + 強調
/// - Mixed (見開きの一部ページだけ付与済み): アンバー背景
/// - OFF: 通常
/// - 対応形式外: グレーアウト (クリック不可)
///
/// クリック時は `clicked_tag` に `TagDef.name` を書き込む (closure 外でトグル実行)。
fn draw_tag_panel(
    ui: &mut egui::Ui,
    visible_tags: &[TagPanelChoice],
    rows: &[TagPanelRow],
    picker_open: &mut bool,
    picker_input: &mut String,
    picker_row_key: &mut Option<String>,
    picker_focus_request: &mut bool,
    picker_recent_tab: &mut bool,
    clicked_tag: &mut Option<(String, Vec<TagTarget>)>,
    searched_tag: &mut Option<String>,
) {
    let any_taggable = rows.iter().any(|row| !row.targets.is_empty());
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("タグ")
                .color(egui::Color32::WHITE)
                .size(14.0)
                .strong(),
        );
        if !any_taggable {
            ui.label(egui::RichText::new("(対象外)").size(10.0).color(DIM_COLOR))
                .on_hover_text("この項目にはタグを付けられません。");
        }
    });
    ui.add_space(4.0);

    for (row_idx, row) in rows.iter().enumerate() {
        if row_idx > 0 {
            ui.add_space(7.0);
        }
        let row_key = tag_panel_row_key(row);
        ui.horizontal_wrapped(|ui| {
            if let Some(label) = row.label.as_deref() {
                ui.label(
                    egui::RichText::new(label)
                        .color(LABEL_COLOR)
                        .size(12.0)
                        .strong(),
                );
            }
            if let Some(note) = row.note.as_deref() {
                ui.label(egui::RichText::new(note).size(11.0).color(DIM_COLOR));
            }
            if !row.targets.is_empty() {
                let row_picker_open = *picker_open
                    && picker_row_key
                        .as_deref()
                        .is_some_and(|key| key == row_key.as_str());
                let plus_label = if row_picker_open { "×" } else { "＋" };
                let resp = ui
                    .small_button(plus_label)
                    .on_hover_text(if row_picker_open {
                        "タグ入力を閉じる"
                    } else {
                        "タグを検索/入力して付ける"
                    });
                if resp.clicked() {
                    if row_picker_open {
                        *picker_open = false;
                        picker_input.clear();
                        *picker_row_key = None;
                        *picker_focus_request = false;
                        *picker_recent_tab = false;
                    } else {
                        *picker_open = true;
                        picker_input.clear();
                        *picker_row_key = Some(row_key.clone());
                        *picker_focus_request = true;
                        *picker_recent_tab = false;
                    }
                }
            }
        });
        ui.add_space(3.0);

        // ボタンを折り返し配置。付与中は `#タグ名` を緑色で、未付与は通常色で表示する
        // (丸ドット等の装飾は付けない: ラベルの色とボタン背景で状態を伝える)。
        if visible_tags.is_empty() {
            ui.label(
                egui::RichText::new("（タグなし）")
                    .size(11.0)
                    .color(DIM_COLOR),
            );
        } else {
            ui.horizontal_wrapped(|ui| {
                for def in visible_tags {
                    let with_hash = format!("#{}", def.name);
                    let state = tag_button_state(row, &def.tag_key);
                    let (text_color, fill_color, stroke_color) = tag_button_visuals(state);
                    let label = egui::RichText::new(&with_hash).color(text_color).strong();
                    let btn = egui::Button::new(label)
                        .fill(fill_color)
                        .stroke(egui::Stroke::new(1.15, stroke_color));
                    let is_taggable = !row.targets.is_empty();
                    let resp = ui.add_enabled(is_taggable, btn);
                    let resp = resp.on_hover_text(match state {
                        TagButtonState::On => format!("クリックで `{with_hash}` を削除"),
                        TagButtonState::Mixed => {
                            format!("一部の対象に付与済み。クリックで `{with_hash}` を全対象に付与")
                        }
                        TagButtonState::Off => format!("クリックで `{with_hash}` を付与"),
                    });
                    let clicked = resp.clicked();
                    resp.context_menu(|ui| {
                        if ui.button("このタグで探す").clicked() {
                            *searched_tag = Some(def.name.clone());
                            ui.close();
                        }
                    });
                    if clicked {
                        *clicked_tag = Some((def.name.clone(), row.targets.clone()));
                    }
                }
            });
        }
    }
}

fn draw_image_color_palette_section(
    ui: &mut egui::Ui,
    palette: &crate::color_search::Palette,
    clicked_rgb: &mut Option<[u8; 3]>,
) {
    if palette.colors.is_empty() {
        return;
    }

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("画像色")
                .color(egui::Color32::WHITE)
                .size(14.0)
                .strong(),
        )
        .on_hover_text("画像として扱える項目だけを、この色で絞り込みます。");
        ui.label(
            egui::RichText::new(format!("{} 色", palette.colors.len()))
                .color(DIM_COLOR)
                .size(11.0),
        );
    });
    ui.add_space(5.0);

    ui.horizontal_wrapped(|ui| {
        for color in &palette.colors {
            let rgb = color.rgb;
            let fill = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
            let swatch_size = egui::vec2(24.0, 24.0);
            let (rect, response) = ui.allocate_exact_size(swatch_size, egui::Sense::click());
            let stroke_color = if response.hovered() {
                egui::Color32::WHITE
            } else {
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 80)
            };
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(5), fill);
            ui.painter().rect_stroke(
                rect,
                egui::CornerRadius::same(5),
                egui::Stroke::new(1.0, stroke_color),
                egui::epaint::StrokeKind::Outside,
            );
            let tooltip = format!(
                "{} ({:.1}%)\nクリックで画像色フィルタに使用",
                crate::color_search::hex_rgb(rgb),
                color.ratio * 100.0
            );
            if response.on_hover_text(tooltip).clicked() {
                *clicked_rgb = Some(rgb);
            }
        }
    });
}

fn draw_fullscreen_tag_picker_panel(
    ui: &mut egui::Ui,
    tag_catalog: &[TagPanelChoice],
    rows: &[TagPanelRow],
    picker_open: &mut bool,
    picker_input: &mut String,
    picker_row_key: &mut Option<String>,
    picker_focus_request: &mut bool,
    picker_recent_tab: &mut bool,
    enter_pressed: bool,
    ime_active: bool,
    set_tag: &mut Option<(String, bool, Vec<TagTarget>)>,
) {
    let selected_idx = picker_row_key
        .as_deref()
        .and_then(|key| rows.iter().position(|row| tag_panel_row_key(row) == key))
        .or_else(|| rows.iter().position(|row| !row.targets.is_empty()));

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("タグを選択")
                .color(egui::Color32::WHITE)
                .size(15.0)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("戻る").clicked() {
                close_fullscreen_tag_picker(
                    picker_open,
                    picker_input,
                    picker_row_key,
                    picker_focus_request,
                    picker_recent_tab,
                );
            }
        });
    });
    ui.add_space(6.0);

    let Some(row_idx) = selected_idx else {
        ui.label(
            egui::RichText::new("タグを付けられる対象がありません。")
                .size(12.0)
                .color(DIM_COLOR),
        );
        return;
    };
    let row = &rows[row_idx];
    if let Some(label) = row.label.as_deref() {
        ui.label(
            egui::RichText::new(label)
                .color(LABEL_COLOR)
                .size(12.0)
                .strong(),
        );
    }
    if let Some(note) = row.note.as_deref() {
        ui.label(egui::RichText::new(note).size(11.0).color(DIM_COLOR));
    }
    ui.add_space(8.0);

    let mut close_after_apply = false;
    draw_fullscreen_tag_picker(
        ui,
        tag_catalog,
        row,
        picker_input,
        picker_focus_request,
        picker_recent_tab,
        enter_pressed,
        ime_active,
        set_tag,
        &mut close_after_apply,
    );
    if close_after_apply {
        close_fullscreen_tag_picker(
            picker_open,
            picker_input,
            picker_row_key,
            picker_focus_request,
            picker_recent_tab,
        );
    }
}

fn close_fullscreen_tag_picker(
    picker_open: &mut bool,
    picker_input: &mut String,
    picker_row_key: &mut Option<String>,
    picker_focus_request: &mut bool,
    picker_recent_tab: &mut bool,
) {
    *picker_open = false;
    picker_input.clear();
    *picker_row_key = None;
    *picker_focus_request = false;
    *picker_recent_tab = false;
}

fn draw_fullscreen_tag_picker(
    ui: &mut egui::Ui,
    tag_catalog: &[TagPanelChoice],
    row: &TagPanelRow,
    input: &mut String,
    focus_request: &mut bool,
    recent_tab: &mut bool,
    enter_pressed: bool,
    ime_active: bool,
    set_tag: &mut Option<(String, bool, Vec<TagTarget>)>,
    close_after_apply: &mut bool,
) {
    ui.add_space(5.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("タグ:").size(12.0).color(DIM_COLOR));
        let input_resp = ui.add_sized(
            [180.0, 22.0],
            egui::TextEdit::singleline(input)
                .hint_text("タグを検索/入力")
                .return_key(None::<egui::KeyboardShortcut>),
        );
        let restore_focus_for_ime_key = ime_active
            && input_resp.lost_focus()
            && ui.input(|i| {
                i.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::Enter | egui::Key::Escape,
                            pressed: true,
                            ..
                        }
                    )
                })
            });
        if *focus_request {
            input_resp.request_focus();
            *focus_request = false;
        } else if restore_focus_for_ime_key {
            input_resp.request_focus();
            *focus_request = true;
            ui.ctx().request_repaint();
        }
        let normalized = crate::tags_db::normalize_tag_display_name(input.trim());
        let valid = tag_panel_input_valid(&normalized);
        let add_clicked = ui.add_enabled(valid, egui::Button::new("付ける")).clicked();
        let enter_pressed = input_resp.has_focus() && enter_pressed;
        if valid && (add_clicked || enter_pressed) {
            *set_tag = Some((normalized.clone(), true, row.targets.clone()));
            input.clear();
            *focus_request = true;
            *close_after_apply = true;
        }
    });
    let normalized = crate::tags_db::normalize_tag_display_name(input.trim());
    let input_too_long = normalized.chars().count() > 64;
    let input_has_whitespace = crate::tags_db::tag_display_name_has_whitespace(&normalized);
    if input_too_long || input_has_whitespace {
        ui.label(
            egui::RichText::new(if input_too_long {
                "タグ名は64文字以内です。"
            } else {
                "タグ名に空白は使えません。"
            })
            .size(11.0)
            .color(egui::Color32::from_rgb(220, 120, 90)),
        );
    }

    let query_key = crate::tags_db::normalize_tag_key(&normalized);
    draw_tag_picker_tabs(ui, recent_tab);
    let mut choices = tag_panel_picker_choices(tag_catalog, &query_key, *recent_tab);
    choices.truncate(12);
    if choices.is_empty() {
        ui.label(egui::RichText::new("候補なし").size(11.0).color(DIM_COLOR));
        return;
    }

    for choice in choices {
        ui.horizontal(|ui| {
            let tag = format!("#{}", choice.name);
            let label_w = (ui.available_width() - 124.0).max(96.0);
            // タグ名は固定幅 label_w を確保しつつ左揃えにする。`add_sized` は中央寄せ +
            // 引き伸ばしになり、行ごとに `#` の x がずれて読みづらい。`set_min_width` で
            // label_w を必ず消費し、右側の件数 / ボタン列の右揃えを保つ。
            let resp = ui
                .allocate_ui_with_layout(
                    egui::vec2(label_w, 20.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_width(label_w);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&tag)
                                    .monospace()
                                    .color(egui::Color32::from_rgb(246, 248, 252)),
                            )
                            .truncate(),
                        )
                    },
                )
                .inner;
            resp.on_hover_text(tag);
            let meta = if choice.count > 0 {
                format!("{}件", choice.count)
            } else if choice.pinned {
                "ピン".to_string()
            } else {
                String::new()
            };
            ui.add_sized(
                [34.0, 20.0],
                egui::Label::new(
                    egui::RichText::new(meta)
                        .size(11.0)
                        .color(egui::Color32::from_rgb(188, 198, 214)),
                ),
            );
            let state = tag_button_state(row, &choice.tag_key);
            let add = state != TagButtonState::On;
            let label = if add { "付ける" } else { "外す" };
            if ui.button(label).clicked() {
                *set_tag = Some((choice.name.clone(), add, row.targets.clone()));
                *close_after_apply = true;
            }
        });
    }
}

fn tag_panel_input_valid(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 64
        && !crate::tags_db::tag_display_name_has_whitespace(name)
}

fn tag_button_visuals(state: TagButtonState) -> (egui::Color32, egui::Color32, egui::Color32) {
    match state {
        TagButtonState::On => (
            egui::Color32::from_rgb(236, 255, 238),
            egui::Color32::from_rgba_unmultiplied(26, 108, 62, 244),
            egui::Color32::from_rgb(132, 236, 156),
        ),
        TagButtonState::Mixed => (
            egui::Color32::from_rgb(255, 244, 214),
            egui::Color32::from_rgba_unmultiplied(118, 78, 20, 244),
            egui::Color32::from_rgb(255, 190, 92),
        ),
        TagButtonState::Off => (
            egui::Color32::from_rgb(248, 250, 255),
            egui::Color32::from_rgba_unmultiplied(35, 38, 50, 244),
            egui::Color32::from_rgb(132, 146, 174),
        ),
    }
}

fn draw_tag_picker_tabs(ui: &mut egui::Ui, recent_tab: &mut bool) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if tag_picker_tab_button(ui, "ピン留め", !*recent_tab).clicked() {
            *recent_tab = false;
        }
        if tag_picker_tab_button(ui, "最近", *recent_tab).clicked() {
            *recent_tab = true;
        }
    });
    ui.add_space(2.0);
}

fn tag_picker_tab_button(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let text = egui::RichText::new(label).color(if selected {
        egui::Color32::from_rgb(248, 252, 255)
    } else {
        egui::Color32::from_rgb(202, 210, 224)
    });
    ui.add(
        egui::Button::new(text)
            .fill(if selected {
                egui::Color32::from_rgb(58, 84, 116)
            } else {
                egui::Color32::from_rgb(42, 46, 56)
            })
            .stroke(egui::Stroke::new(
                1.0,
                if selected {
                    egui::Color32::from_rgb(142, 176, 218)
                } else {
                    egui::Color32::from_rgb(82, 92, 112)
                },
            )),
    )
}

fn tag_panel_picker_choices(
    tag_catalog: &[TagPanelChoice],
    query_key: &str,
    recent_tab: bool,
) -> Vec<TagPanelChoice> {
    let mut out = Vec::new();
    let mut seen = Vec::<String>::new();
    for choice in tag_catalog {
        if choice.tag_key.is_empty() || seen.iter().any(|key| key == &choice.tag_key) {
            continue;
        }
        if !query_key.is_empty() {
            if !choice.tag_key.starts_with(query_key) {
                continue;
            }
        } else if recent_tab {
            if choice.last_applied_at <= 0 {
                continue;
            }
        } else if !choice.pinned {
            continue;
        }
        seen.push(choice.tag_key.clone());
        out.push(choice.clone());
    }
    if !query_key.is_empty() {
        out.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| b.last_applied_at.cmp(&a.last_applied_at))
                .then_with(|| b.count.cmp(&a.count))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
    } else if recent_tab {
        out.sort_by(|a, b| {
            b.last_applied_at
                .cmp(&a.last_applied_at)
                .then_with(|| b.count.cmp(&a.count))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
    }
    out
}

fn tag_panel_visible_choices(
    pinned_tags: &[TagPanelChoice],
    rows: &[TagPanelRow],
    sticky_tags: &[(String, String)],
    tag_catalog: &[TagPanelChoice],
) -> Vec<TagPanelChoice> {
    let mut out = Vec::new();
    let mut seen = Vec::<String>::new();
    for choice in pinned_tags {
        push_tag_panel_choice(&mut out, &mut seen, choice.clone());
    }
    for (name, tag_key) in sticky_tags {
        push_tag_panel_choice(
            &mut out,
            &mut seen,
            tag_panel_choice_from_key(tag_catalog, tag_key, name, false),
        );
    }
    for row in rows {
        for tags in &row.tags_by_target {
            for tag in tags {
                let tag_key = crate::tags_db::normalize_tag_key(tag);
                if tag_key.is_empty() {
                    continue;
                }
                push_tag_panel_choice(
                    &mut out,
                    &mut seen,
                    tag_panel_choice_from_key(
                        tag_catalog,
                        &tag_key,
                        crate::tags_db::strip_display_hash(tag),
                        false,
                    ),
                );
            }
        }
    }
    out
}

fn push_tag_panel_choice(
    out: &mut Vec<TagPanelChoice>,
    seen: &mut Vec<String>,
    choice: TagPanelChoice,
) {
    if choice.tag_key.is_empty() || seen.iter().any(|key| key == &choice.tag_key) {
        return;
    }
    seen.push(choice.tag_key.clone());
    out.push(choice);
}

fn tag_panel_choice_from_key(
    tag_catalog: &[TagPanelChoice],
    tag_key: &str,
    fallback_name: &str,
    pinned: bool,
) -> TagPanelChoice {
    tag_catalog
        .iter()
        .find(|choice| choice.tag_key == tag_key)
        .cloned()
        .map(|mut choice| {
            choice.pinned |= pinned;
            choice
        })
        .unwrap_or_else(|| TagPanelChoice {
            name: fallback_name.to_string(),
            tag_key: tag_key.to_string(),
            count: 0,
            pinned,
            last_applied_at: 0,
        })
}

fn tag_panel_target_key(rows: &[TagPanelRow]) -> String {
    rows.iter()
        .map(tag_panel_row_key)
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

fn tag_panel_row_key(row: &TagPanelRow) -> String {
    row.targets
        .iter()
        .map(|target| crate::tags_db::item_key_for_path(&target.path))
        .collect::<Vec<_>>()
        .join("\u{1e}")
}

fn tag_button_state(row: &TagPanelRow, tag_key: &str) -> TagButtonState {
    let target_count = row.tags_by_target.len();
    if target_count == 0 {
        return TagButtonState::Off;
    }
    let tagged_count = row
        .tags_by_target
        .iter()
        .filter(|tags| {
            tags.iter()
                .any(|tag| crate::tags_db::normalize_tag_key(tag) == tag_key)
        })
        .count();
    if tagged_count == 0 {
        TagButtonState::Off
    } else if tagged_count == target_count {
        TagButtonState::On
    } else {
        TagButtonState::Mixed
    }
}

fn dedup_tag_targets(targets: &mut Vec<TagTarget>) {
    let mut seen: Vec<PathBuf> = Vec::new();
    targets.retain(|target| {
        if seen
            .iter()
            .any(|path| crate::folder_tree::path_eq(path, &target.path))
        {
            false
        } else {
            seen.push(target.path.clone());
            true
        }
    });
}

fn shared_parent_folder(paths: &[PathBuf]) -> Option<PathBuf> {
    let first_parent = paths.first()?.parent()?;
    if paths.iter().all(|path| {
        path.parent()
            .is_some_and(|p| crate::folder_tree::path_eq(p, first_parent))
    }) {
        Some(first_parent.to_path_buf())
    } else {
        None
    }
}

fn tag_path_display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

fn draw_a1111_panel(ui: &mut egui::Ui, ctx: &egui::Context, meta: &A1111Metadata) {
    // ヘッダー
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("AI Metadata")
                .color(egui::Color32::WHITE)
                .size(16.0)
                .strong(),
        );
        ui.label(
            egui::RichText::new("A1111")
                .color(egui::Color32::from_rgb(100, 180, 255))
                .size(12.0)
                .background_color(egui::Color32::from_rgba_unmultiplied(100, 180, 255, 30)),
        );
    });
    ui.add_space(8.0);

    // Prompt
    if !meta.prompt.is_empty() {
        draw_text_section(ui, ctx, "Prompt", &meta.prompt);
    }

    // Negative prompt
    if !meta.negative_prompt.is_empty() {
        ui.add_space(6.0);
        draw_text_section(ui, ctx, "Negative Prompt", &meta.negative_prompt);
    }

    // Parameters
    if !meta.params.is_empty() {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Parameters")
                .color(LABEL_COLOR)
                .size(SECTION_FONT),
        );
        ui.add_space(2.0);
        for (key, val) in &meta.params {
            draw_key_value_wrapped(ui, key, val);
        }
    }
}

fn draw_comfyui_panel(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    meta: &ComfyUIMetadata,
    show_raw_prompt: bool,
    show_raw_workflow: bool,
) -> (bool, bool) {
    let mut rp = show_raw_prompt;
    let mut rw = show_raw_workflow;

    // ヘッダー
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("AI Metadata")
                .color(egui::Color32::WHITE)
                .size(16.0)
                .strong(),
        );
        ui.label(
            egui::RichText::new("ComfyUI")
                .color(egui::Color32::from_rgb(120, 220, 120))
                .size(12.0)
                .background_color(egui::Color32::from_rgba_unmultiplied(120, 220, 120, 30)),
        );
    });
    ui.add_space(8.0);

    // Extracted prompts
    if !meta.extracted_prompts.is_empty() {
        let combined = meta.extracted_prompts.join("\n---\n");
        draw_text_section(ui, ctx, "Prompt", &combined);
    }

    // Extracted negatives
    if !meta.extracted_negatives.is_empty() {
        ui.add_space(6.0);
        let combined = meta.extracted_negatives.join("\n---\n");
        draw_text_section(ui, ctx, "Negative Prompt", &combined);
    }

    // Sampler parameters
    if !meta.sampler_params.is_empty() {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Parameters")
                .color(LABEL_COLOR)
                .size(SECTION_FONT),
        );
        ui.add_space(2.0);
        for (key, val) in &meta.sampler_params {
            draw_key_value_wrapped(ui, key, val);
        }
    }

    // Raw JSON sections (collapsible)
    ui.add_space(10.0);
    {
        let json_str = serde_json::to_string_pretty(&meta.prompt_json).unwrap_or_default();
        draw_collapsible_json_section(ui, ctx, "Raw Prompt JSON", &json_str, &mut rp);
    }

    if let Some(ref wf) = meta.workflow_json {
        ui.add_space(4.0);
        let json_str = serde_json::to_string_pretty(wf).unwrap_or_default();
        draw_collapsible_json_section(ui, ctx, "Raw Workflow JSON", &json_str, &mut rw);
    }

    (rp, rw)
}

fn draw_unknown_panel(ui: &mut egui::Ui, chunks: &[(String, String)]) {
    ui.label(
        egui::RichText::new("Metadata")
            .color(egui::Color32::WHITE)
            .size(16.0)
            .strong(),
    );
    ui.add_space(8.0);

    for (key, val) in chunks {
        ui.label(
            egui::RichText::new(key)
                .color(LABEL_COLOR)
                .size(SECTION_FONT),
        );
        ui.add_space(2.0);
        let display = if val.len() > 2000 {
            format!("{}...", &val[..2000])
        } else {
            val.clone()
        };
        ui.label(
            egui::RichText::new(display)
                .color(TEXT_COLOR)
                .font(crate::ui_fonts::user_text_font(BODY_FONT)),
        );
        ui.add_space(8.0);
    }
}

fn draw_no_metadata(ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new("Image Info")
            .color(egui::Color32::WHITE)
            .size(16.0)
            .strong(),
    );
    ui.add_space(20.0);
    ui.label(
        egui::RichText::new("No metadata found.")
            .color(DIM_COLOR)
            .size(BODY_FONT),
    );
}

fn draw_exif_panel(
    ui: &mut egui::Ui,
    exif: &ExifInfo,
    sections_open: &mut std::collections::HashMap<String, bool>,
) {
    ui.label(
        egui::RichText::new("EXIF")
            .color(egui::Color32::WHITE)
            .size(16.0)
            .strong(),
    );
    ui.add_space(6.0);

    for (group, fields) in &exif.sections {
        let key = format!("{:?}", group);
        let open = sections_open.entry(key).or_insert(true);
        let display_section = group.display_name();
        let header = if *open {
            format!("▼ {display_section}")
        } else {
            format!("▶ {display_section}")
        };
        if ui
            .selectable_label(
                *open,
                egui::RichText::new(&header)
                    .color(LABEL_COLOR)
                    .size(SECTION_FONT),
            )
            .clicked()
        {
            *open = !*open;
        }

        if *open {
            ui.add_space(2.0);
            for (tag_name, value) in fields {
                let display_tag = exif_reader::tag_display_name(tag_name);
                draw_key_value_wrapped(ui, display_tag, value);
            }
            ui.add_space(4.0);
        }
    }
}

/// キー: 値 を1つの LayoutJob で描画し、長い値も確実に折り返す。
fn draw_key_value_wrapped(ui: &mut egui::Ui, key: &str, val: &str) {
    if !crate::ui_text_links::find_http_urls(val).is_empty() {
        ui.horizontal_top(|ui| {
            ui.label(
                egui::RichText::new(format!("{key}:  "))
                    .font(egui::FontId::proportional(BODY_FONT))
                    .color(DIM_COLOR),
            );
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                draw_user_text_with_links(ui, val, BODY_FONT);
            });
        });
        return;
    }

    let mut job = egui::text::LayoutJob::default();
    job.wrap = egui::text::TextWrapping {
        max_width: ui.available_width(),
        ..Default::default()
    };
    job.append(
        &format!("{key}:  "),
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(BODY_FONT),
            color: DIM_COLOR,
            ..Default::default()
        },
    );
    job.append(
        val,
        0.0,
        egui::TextFormat {
            font_id: crate::ui_fonts::user_text_font(BODY_FONT),
            color: TEXT_COLOR,
            ..Default::default()
        },
    );
    ui.label(job);
}

fn draw_user_text_with_links(ui: &mut egui::Ui, text: &str, font_size: f32) {
    if let Some(url) = crate::ui_text_links::draw_text_with_links(
        ui,
        text,
        crate::ui_fonts::user_text_font(font_size),
        TEXT_COLOR,
        LINK_COLOR,
    ) {
        crate::ui_helpers::open_url(&url);
    }
}

/// 折りたたみ可能な JSON セクションを描画する。
fn draw_collapsible_json_section(
    ui: &mut egui::Ui,
    _ctx: &egui::Context,
    label: &str,
    json: &str,
    open: &mut bool,
) {
    if ui
        .selectable_label(
            *open,
            egui::RichText::new(if *open {
                format!("▼ {label}")
            } else {
                format!("▶ {label}")
            })
            .color(DIM_COLOR)
            .size(BODY_FONT),
        )
        .clicked()
    {
        *open = !*open;
    }
    if *open {
        egui::ScrollArea::vertical()
            .id_salt(label)
            .max_height(300.0)
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(json)
                        .color(JSON_COLOR)
                        .size(11.0)
                        .monospace(),
                );
            });
    }
}

/// 外部メタデータ (サイドカー) セクション。JSON は汎用 key/value ツリー、
/// TXT はテキスト表示 (docs/sidecar-metadata-ingest.md §11)。
/// 特定スキーマの代表フィールドをハードコードしない (どんな JSON でも同一ロジック)。
fn draw_sidecar_section(ui: &mut egui::Ui, sc: &crate::external_metadata::SidecarDisplay) {
    ui.label(
        egui::RichText::new("外部メタデータ")
            .color(egui::Color32::WHITE)
            .size(16.0)
            .strong(),
    );
    ui.add_space(4.0);
    match sc {
        crate::external_metadata::SidecarDisplay::Json(v) => {
            draw_json_node(ui, None, v, 0);
        }
        crate::external_metadata::SidecarDisplay::Text(t) => {
            draw_user_text_with_links(ui, t.as_str(), 11.0);
        }
    }
}

/// JSON 値がスカラ (Null/Bool/Number/String) かを **非アロケート** で判定する。
/// 配列分類の毎フレーム走査で `json_scalar_str` (String 生成) を避けるために使う。
fn is_json_scalar(v: &serde_json::Value) -> bool {
    matches!(
        v,
        serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_)
    )
}

/// JSON 値がスカラ (Null/Bool/Number/String) ならその表示文字列を返す。
fn json_scalar_str(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Null => Some("null".to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn json_dim_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(DIM_COLOR).size(BODY_FONT));
}

/// JSON 値を 1 ノード描画する。スカラは `key: value`、スカラ配列は 1 行、
/// ネストした配列/オブジェクトはインデントして再帰。深さ上限で打ち切る。
///
/// egui は immediate-mode なのでパネル表示中は毎フレーム再描画される。サイドカーは最大 2MB
/// 許容するため、巨大配列/オブジェクトを毎フレーム全件走査 / join / widget 化すると重い (Codex P2)。
/// 分類も描画も **先頭 `MAX_ITEMS` 件で完結** させ、残りは件数表示にする
/// (分類は非アロケートの `is_json_scalar` で先頭 MAX_ITEMS 件のみ判定する)。
fn draw_json_node(ui: &mut egui::Ui, key: Option<&str>, v: &serde_json::Value, depth: usize) {
    const MAX_DEPTH: usize = 8;
    const MAX_ITEMS: usize = 100;
    if let Some(s) = json_scalar_str(v) {
        draw_key_value_wrapped(ui, key.unwrap_or("-"), &s);
        return;
    }
    match v {
        serde_json::Value::Array(a) => {
            // 分類は先頭 MAX_ITEMS 件のみ・非アロケート判定 (巨大配列の毎フレーム全件走査を回避)。
            // 先頭が全てスカラならスカラ配列としてプレビュー表示する (101 件目以降に非スカラが
            // 混ざっていても、表示はあくまで先頭 100 件 + 残り件数なので実用上問題ない)。
            if a.iter().take(MAX_ITEMS).all(is_json_scalar) {
                // スカラのみの配列は 1 行に連結 (先頭 MAX_ITEMS 件まで)
                let mut joined = a
                    .iter()
                    .take(MAX_ITEMS)
                    .filter_map(json_scalar_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                if a.len() > MAX_ITEMS {
                    joined.push_str(&format!(" … (他 {} 件)", a.len() - MAX_ITEMS));
                }
                draw_key_value_wrapped(ui, key.unwrap_or("-"), &joined);
            } else if depth >= MAX_DEPTH {
                draw_key_value_wrapped(ui, key.unwrap_or("-"), &format!("[{} 件]", a.len()));
            } else {
                if let Some(k) = key {
                    json_dim_label(ui, &format!("{k}:"));
                }
                ui.indent(("sc_arr", depth, key.unwrap_or("")), |ui| {
                    for (i, e) in a.iter().take(MAX_ITEMS).enumerate() {
                        draw_json_node(ui, Some(&format!("[{i}]")), e, depth + 1);
                    }
                    if a.len() > MAX_ITEMS {
                        json_dim_label(ui, &format!("… (他 {} 件)", a.len() - MAX_ITEMS));
                    }
                });
            }
        }
        serde_json::Value::Object(m) => {
            if depth >= MAX_DEPTH {
                draw_key_value_wrapped(ui, key.unwrap_or("-"), &format!("{{{} キー}}", m.len()));
            } else if depth == 0 {
                // トップレベルはインデントせず並べる (キー数も先頭 MAX_ITEMS 件で打ち切る)
                for (k, val) in m.iter().take(MAX_ITEMS) {
                    draw_json_node(ui, Some(k), val, depth + 1);
                }
                if m.len() > MAX_ITEMS {
                    json_dim_label(ui, &format!("… (他 {} 件)", m.len() - MAX_ITEMS));
                }
            } else {
                if let Some(k) = key {
                    json_dim_label(ui, &format!("{k}:"));
                }
                ui.indent(("sc_obj", depth, key.unwrap_or("")), |ui| {
                    for (k, val) in m.iter().take(MAX_ITEMS) {
                        draw_json_node(ui, Some(k), val, depth + 1);
                    }
                    if m.len() > MAX_ITEMS {
                        json_dim_label(ui, &format!("… (他 {} 件)", m.len() - MAX_ITEMS));
                    }
                });
            }
        }
        // Null/Bool/Number/String は上の scalar 経路で処理済み
        _ => {}
    }
}

/// 「X ツイート情報」セクションを描画する。
/// `xtw:TweetId` がある画像 (mXD が保存したもの) のときだけ呼ばれる。
fn draw_tweet_panel(ui: &mut egui::Ui, ctx: &egui::Context, t: &XmpTweetInfo) {
    // ヘッダー
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("X ツイート情報")
                .color(egui::Color32::WHITE)
                .size(16.0)
                .strong(),
        );
        if let Some(src) = &t.source {
            let (label, color) = match src.as_str() {
                "Likes" => ("いいね", egui::Color32::from_rgb(240, 100, 140)),
                "Bookmarks" => ("ブックマーク", egui::Color32::from_rgb(100, 160, 240)),
                other => (other, egui::Color32::from_rgb(180, 180, 180)),
            };
            ui.label(
                egui::RichText::new(label)
                    .color(color)
                    .size(12.0)
                    .background_color(egui::Color32::from_rgba_unmultiplied(
                        color.r(),
                        color.g(),
                        color.b(),
                        30,
                    )),
            );
        }
    });
    ui.add_space(8.0);

    // 投稿者
    if t.author_display_name.is_some() || t.author_screen_name.is_some() {
        let display = t
            .author_display_name
            .as_deref()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let screen = t
            .author_screen_name
            .as_deref()
            .map(|s| format!("@{s}"))
            .unwrap_or_default();
        let combined = match (display.is_empty(), screen.is_empty()) {
            (false, false) => format!("{display} ({screen})"),
            (true, false) => screen,
            (false, true) => display,
            _ => String::new(),
        };
        draw_key_value_wrapped(ui, "投稿者", &combined);
    }

    if let Some(ts) = t.posted_at.as_deref() {
        draw_key_value_wrapped(ui, "投稿日時", &format_xmp_datetime(ts));
    }
    if let Some(ts) = t.discovered_at.as_deref() {
        draw_key_value_wrapped(ui, "発見日時", &format_xmp_datetime(ts));
    }

    // スレッド / メディア位置 (単独投稿・単枚は省略)
    let thread_interesting = t.thread_part.map(|n| n > 1).unwrap_or(false);
    let media_interesting = t.media_count.map(|n| n > 1).unwrap_or(false);
    if thread_interesting || media_interesting {
        let mut parts = Vec::new();
        if let Some(tp) = t.thread_part {
            parts.push(format!("スレッド {tp} 番目"));
        }
        if let (Some(mi), Some(mc)) = (t.media_index, t.media_count) {
            parts.push(format!("メディア {mi}/{mc}"));
        }
        if !parts.is_empty() {
            draw_key_value_wrapped(ui, "位置", &parts.join(" / "));
        }
    }

    // 本文
    if let Some(body) = t.description.as_deref().filter(|s| !s.is_empty()) {
        ui.add_space(4.0);
        draw_text_section(ui, ctx, "本文", body);
    }

    // アクションボタン
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        if let Some(url) = t.tweet_url.as_deref() {
            if xmp_reader::is_safe_tweet_url(url)
                && ui.button("元ツイートを開く").on_hover_text(url).clicked()
            {
                let _ = opener::open(url);
            }
        }
        if let Some(url) = t.author_url.as_deref() {
            if xmp_reader::is_safe_tweet_url(url)
                && ui
                    .button("投稿者のタイムラインを開く")
                    .on_hover_text(url)
                    .clicked()
            {
                let _ = opener::open(url);
            }
        }
        if let Some(url) = t.tweet_url.as_deref() {
            if xmp_reader::is_safe_tweet_url(url) && ui.small_button("URL コピー").clicked() {
                ctx.copy_text(url.to_string());
            }
        }
    });

    // 引用元 (別ツイートが RT/引用したことで保存された場合のみ)
    if let Some(qurl) = t.quoted_by_url.as_deref() {
        ui.add_space(4.0);
        let by = t
            .quoted_by_author_display_name
            .as_deref()
            .unwrap_or("")
            .to_string();
        let handle = t
            .quoted_by_screen_name
            .as_deref()
            .map(|s| format!("@{s}"))
            .unwrap_or_default();
        let label = match (by.is_empty(), handle.is_empty()) {
            (false, false) => format!("{by} ({handle}) が引用"),
            (_, false) => format!("{handle} が引用"),
            (false, _) => format!("{by} が引用"),
            _ => "別ツイートから引用".to_string(),
        };
        ui.label(egui::RichText::new(label).color(DIM_COLOR).size(BODY_FONT));
        if xmp_reader::is_safe_tweet_url(qurl)
            && ui
                .button("引用した投稿を開く")
                .on_hover_text(qurl)
                .clicked()
        {
            let _ = opener::open(qurl);
        }
    }
}

/// mXD / ExifTool が書く日時を視認しやすい形に整える。
/// 入力例: `"2026:04:16 04:09:58.0000000+00:00"` (ExifTool の `:` 区切り)
///         `"2026-04-16T04:09:58.0000000+00:00"` (ISO-8601)
/// 出力例: `"2026-04-16 04:09:58 UTC"` / `"2026-04-16 04:09:58 +09:00"`
/// パターンに合わなければ原文を返す。
fn format_xmp_datetime(raw: &str) -> String {
    // 日付部の `:` を `-` に (ExifTool 形式対策)。先頭 10 文字内の最初の 2 個が対象。
    // 10 文字境界で切るのは日付と時刻を混同しないため ("YYYY:MM:DD" の 10 文字)。
    let date_part: String = raw
        .chars()
        .take(10)
        .scan(0u8, |replaced, c| {
            let out = if c == ':' && *replaced < 2 {
                *replaced += 1;
                '-'
            } else {
                c
            };
            Some(out)
        })
        .collect();
    let rest: String = raw.chars().skip(10).collect();
    // ISO-8601 の `T` を空白に揃える (UI の読みやすさ)
    let rest = rest.replacen('T', " ", 1);

    // タイムゾーンを先に切り出す: `Z` / `+HH:MM` / `-HH:MM` のいずれか。
    // 小数秒(.0000000) の位置より後ろの `+`/`-`/`Z` を採用する。
    let tz_search_start = rest.find('.').map(|d| d + 1).unwrap_or(0);
    let tz_pos = rest[tz_search_start..]
        .find(|c: char| c == '+' || c == '-' || c == 'Z')
        .map(|i| tz_search_start + i);
    let (body, tz_suffix) = match tz_pos {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest.as_str(), ""),
    };
    // 小数秒を捨てる
    let time = match body.find('.') {
        Some(dot) => &body[..dot],
        None => body,
    };
    let tz_label = match tz_suffix {
        "+00:00" | "Z" => " UTC".to_string(),
        "" => String::new(),
        other => format!(" {other}"),
    };
    format!("{date_part}{time}{tz_label}")
}

#[cfg(test)]
mod format_datetime_tests {
    use super::format_xmp_datetime;

    #[test]
    fn utc_iso8601_collapses_offset() {
        assert_eq!(
            format_xmp_datetime("2026-04-16T04:09:58.0000000+00:00"),
            "2026-04-16 04:09:58 UTC"
        );
    }

    #[test]
    fn utc_exiftool_colon_date_converted() {
        assert_eq!(
            format_xmp_datetime("2026:04:16 04:09:58.0000000+00:00"),
            "2026-04-16 04:09:58 UTC"
        );
    }

    #[test]
    fn non_utc_offset_preserved() {
        assert_eq!(
            format_xmp_datetime("2026-04-16T04:09:58.500+09:00"),
            "2026-04-16 04:09:58 +09:00"
        );
    }

    #[test]
    fn no_fractional_seconds() {
        assert_eq!(
            format_xmp_datetime("2026-04-16T04:09:58Z"),
            "2026-04-16 04:09:58 UTC"
        );
    }

    #[test]
    fn unparseable_returns_near_original() {
        // `:` 置換は走るが壊れた文字列はそのまま (日付部 10 文字を超える位置は保持)
        let out = format_xmp_datetime("not a date");
        assert_eq!(out, "not a date");
    }
}

/// テキストセクション (ラベル + コピーボタン + テキスト) を描画する。
fn draw_text_section(ui: &mut egui::Ui, ctx: &egui::Context, label: &str, text: &str) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .color(LABEL_COLOR)
                .size(SECTION_FONT),
        );
        if ui
            .small_button("Copy")
            .on_hover_text("Copy to clipboard")
            .clicked()
        {
            ctx.copy_text(text.to_string());
        }
    });
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(text)
            .color(TEXT_COLOR)
            .font(crate::ui_fonts::user_text_font(BODY_FONT)),
    );
}

#[cfg(test)]
mod tag_panel_tests {
    use std::path::PathBuf;

    use super::{
        TagButtonState, TagPanelChoice, TagPanelRow, tag_button_state, tag_panel_picker_choices,
        tag_panel_visible_choices,
    };
    use crate::tag_ops::TagTarget;

    fn row(path: &str, tags: &[&str]) -> TagPanelRow {
        TagPanelRow {
            label: None,
            note: None,
            targets: vec![TagTarget {
                path: PathBuf::from(path),
                tag_sidecar: None,
            }],
            tags_by_target: vec![tags.iter().map(|tag| (*tag).to_string()).collect()],
        }
    }

    fn choice(name: &str, pinned: bool) -> TagPanelChoice {
        TagPanelChoice {
            name: name.to_string(),
            tag_key: crate::tags_db::normalize_tag_key(name),
            count: 0,
            pinned,
            last_applied_at: 0,
        }
    }

    fn recent_choice(name: &str, pinned: bool, last_applied_at: i64) -> TagPanelChoice {
        TagPanelChoice {
            name: name.to_string(),
            tag_key: crate::tags_db::normalize_tag_key(name),
            count: 0,
            pinned,
            last_applied_at,
        }
    }

    #[test]
    fn visible_choices_include_current_unpinned_tags() {
        let rows = vec![row("C:/media/a.mp4", &["#旅行"])];
        let visible = tag_panel_visible_choices(&[choice("人物", true)], &rows, &[], &[]);
        let names: Vec<_> = visible.iter().map(|choice| choice.name.as_str()).collect();
        assert_eq!(names, vec!["人物", "旅行"]);
    }

    #[test]
    fn sticky_unpinned_tag_remains_as_off_button_after_removal() {
        let rows = vec![row("C:/media/a.mp4", &[])];
        let sticky = vec![(
            "旅行".to_string(),
            crate::tags_db::normalize_tag_key("旅行"),
        )];
        let visible = tag_panel_visible_choices(&[], &rows, &sticky, &[]);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].name, "旅行");
        assert_eq!(
            tag_button_state(&rows[0], &visible[0].tag_key),
            TagButtonState::Off
        );
    }

    #[test]
    fn picker_pinned_tab_shows_only_pinned_tags_when_input_empty() {
        let catalog = vec![
            recent_choice("人物", true, 10),
            recent_choice("旅行", false, 20),
        ];
        let choices = tag_panel_picker_choices(&catalog, "", false);
        let names: Vec<_> = choices.iter().map(|choice| choice.name.as_str()).collect();
        assert_eq!(names, vec!["人物"]);
    }

    #[test]
    fn picker_recent_tab_uses_last_applied_order_without_removed_only_tags() {
        let catalog = vec![
            recent_choice("人物", true, 10),
            recent_choice("旅行", false, 30),
            recent_choice("未使用", false, 0),
        ];
        let choices = tag_panel_picker_choices(&catalog, "", true);
        let names: Vec<_> = choices.iter().map(|choice| choice.name.as_str()).collect();
        assert_eq!(names, vec!["旅行", "人物"]);
    }

    #[test]
    fn picker_search_ignores_current_tab() {
        let catalog = vec![
            recent_choice("人物", true, 10),
            recent_choice("旅行", false, 30),
        ];
        let query_key = crate::tags_db::normalize_tag_key("旅");
        let choices = tag_panel_picker_choices(&catalog, &query_key, false);
        let names: Vec<_> = choices.iter().map(|choice| choice.name.as_str()).collect();
        assert_eq!(names, vec!["旅行"]);
    }
}
