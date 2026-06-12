//! フルスクリーンのメタデータサイドパネル。
//!
//! AI 画像生成メタデータ (A1111/ComfyUI) と EXIF 撮影情報を右サイドパネルに表示する。

use eframe::egui;

use crate::app::App;
use crate::exif_reader::{self, ExifInfo};
use crate::png_metadata::{A1111Metadata, AiMetadata, ComfyUIMetadata};
use crate::xmp_reader::{self, XmpTweetInfo};

/// パネル幅 (ピクセル)
const PANEL_WIDTH: f32 = 380.0;

/// 上部ホバーバーの描画高さ
const TOP_BAR_H: f32 = 44.0;
/// パネルタイトルバーの高さ
const TITLE_BAR_H: f32 = 32.0;
const LINK_COLOR: egui::Color32 = egui::Color32::from_rgb(115, 180, 255);

impl App {
    /// フルスクリーンでメタデータパネルをオーバーレイ描画する。
    /// 画像は常に `full_rect` 全体に表示し、パネルは画像の上に重ねる。
    ///
    /// 表示条件:
    /// - `I` キーまたはピン留めで固定表示 ON/OFF
    /// - マウスカーソルが画面右 1/4 にあるときもホバー表示
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
        let panel_w = PANEL_WIDTH.min(full_rect.width() * 0.5);
        // 右パネルは常に上部バーの下から開始（上バーは常に同時表示される）
        let panel_top = full_rect.min.y + TOP_BAR_H;
        let panel_rect = egui::Rect::from_min_max(
            egui::pos2(full_rect.max.x - panel_w, panel_top),
            // 下端のページシークバーと重ならないよう、下端をシークバー分空ける。
            egui::pos2(
                full_rect.max.x,
                full_rect.max.y - crate::ui_fullscreen::FS_SEEK_BAR_HEIGHT,
            ),
        );

        if !force_show {
            let hover_threshold = full_rect.max.x - full_rect.width() * 0.25;

            // ホバー判定: 画面右 1/4。カーソル非表示中は最後の座標が stale なので、
            // 実入力でカーソルが復帰するまでは passive hover でパネルを開かない。
            let pointer_pos = ctx.input(|i| {
                if self.cursor_hidden {
                    None
                } else {
                    i.pointer.hover_pos()
                }
            });

            let hover_in_right = pointer_pos.is_some_and(|p| p.x > hover_threshold);
            let hover_in_open_panel = self.metadata_panel_hover_active
                && pointer_pos.is_some_and(|p| panel_rect.contains(p));
            let hover_visible = hover_in_right || hover_in_open_panel;

            let visible = self.show_metadata_panel || hover_visible;
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

        // タグパネル用の情報を先に集める (child_ui の &mut ui closure 前に借用を解消するため)
        let taggable_path = self.current_tag_target_path();
        let current_tags: Vec<String> = if taggable_path.is_some() {
            self.get_current_tags_cached()
        } else {
            Vec::new()
        };
        let defined_tags: Vec<_> = self
            .settings
            .tags
            .iter()
            .filter(|tag| tag.show_shortcut)
            .cloned()
            .collect();

        // タグボタンクリックを closure 内で検出し、後段で request_tag_toggle を走らせる
        let mut clicked_tag: Option<String> = None;

        let inner_rect = content_rect.shrink2(egui::vec2(12.0, 8.0));
        let mut child_ui = ui.new_child(egui::UiBuilder::new().max_rect(inner_rect));
        child_ui.set_clip_rect(content_rect);
        // Metadata values often contain long CJK text, URLs, and hashes. Use a
        // solid scrollbar here so egui reserves a real gutter instead of
        // drawing the default floating bar on top of the text.
        child_ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();

        egui::ScrollArea::vertical()
            .id_salt("metadata_scroll")
            .auto_shrink([false, false])
            .show(&mut child_ui, |ui| {
                ui.set_width(ui.available_width());

                // ── タグパネル (最上段) ──
                // 登録タグを ON/OFF ボタンで並べる。対応形式外のファイルはグレーアウト。
                if !defined_tags.is_empty() {
                    draw_tag_panel(
                        ui,
                        &defined_tags,
                        &current_tags,
                        taggable_path.is_some(),
                        &mut clicked_tag,
                    );
                    if tweet_info.is_some()
                        || ai_metadata.is_some()
                        || exif_info.is_some()
                        || sidecar_info.is_some()
                    {
                        ui.add_space(8.0);
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
                        || !defined_tags.is_empty()
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
                    && defined_tags.is_empty()
                    && sidecar_info.is_none()
                {
                    draw_no_metadata(ui);
                }
            });

        // タグボタンクリックの後処理 (closure 外で self を可変借用する)
        if let Some(tag_name) = clicked_tag {
            self.request_tag_toggle_for_selection(&tag_name);
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

    /// 現在のフルスクリーン項目に対応する mIV タグ一覧を取得する。
    /// ZipImage / PdfPage はコンテナ本体のタグへフォールバックする。
    pub(crate) fn get_current_tags_cached(&mut self) -> Vec<String> {
        let Some(path) = self.current_tag_target_path() else {
            return Vec::new();
        };
        let key = crate::tags_db::item_key_for_path(&path);
        if let Some(cached) = self.tags_cache.get(&key) {
            return cached.clone();
        }
        let tags = self
            .tags_db
            .as_ref()
            .map(|db| db.display_tags_for_item(&key))
            .unwrap_or_default();
        self.tags_cache.insert(key, tags.clone());
        tags
    }

    /// 現在のフルスクリーン項目のタグ対象パスを返す。
    fn current_tag_target_path(&self) -> Option<std::path::PathBuf> {
        use crate::grid_item::GridItem;
        let idx = self.fullscreen_idx?;
        match self.items.get(idx)? {
            GridItem::Folder(p)
            | GridItem::Image(p)
            | GridItem::Video(p)
            | GridItem::ZipFile(p)
            | GridItem::PdfFile(p)
            | GridItem::ConvertibleArchive { path: p, .. } => Some(p.clone()),
            GridItem::ZipImage { zip_path, .. } => Some(zip_path.clone()),
            GridItem::PdfPage { pdf_path, .. } => Some(pdf_path.clone()),
            _ => None,
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

/// タグパネル描画 (docs/tag-feature.md §4.4)。
///
/// 登録タグを ON/OFF ボタンで横並び表示。各ボタンの外観:
/// - ON (現在のファイルに付与済み): 緑背景 + 強調
/// - OFF: 通常
/// - 対応形式外: グレーアウト (クリック不可)
///
/// クリック時は `clicked_tag` に `TagDef.name` を書き込む (closure 外でトグル実行)。
fn draw_tag_panel(
    ui: &mut egui::Ui,
    defined_tags: &[crate::settings::TagDef],
    current_tags: &[String],
    is_taggable: bool,
    clicked_tag: &mut Option<String>,
) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("タグ")
                .color(egui::Color32::WHITE)
                .size(14.0)
                .strong(),
        );
        if !is_taggable {
            ui.label(egui::RichText::new("(対象外)").size(10.0).color(DIM_COLOR))
                .on_hover_text("この項目にはタグを付けられません。");
        }
    });
    ui.add_space(4.0);

    // ボタンを折り返し配置。付与中は `#タグ名` を緑色で、未付与は通常色で表示する
    // (丸ドット等の装飾は付けない: ラベルの色とボタン背景で状態を伝える)。
    ui.horizontal_wrapped(|ui| {
        for def in defined_tags {
            let with_hash = format!("#{}", def.name);
            let is_on = current_tags
                .iter()
                .any(|t| crate::tags_db::normalize_tag_key(t) == def.tag_key);
            let label = egui::RichText::new(&with_hash).color(if is_on {
                egui::Color32::from_rgb(180, 255, 180)
            } else {
                TEXT_COLOR
            });
            let btn = egui::Button::new(label)
                .fill(if is_on {
                    egui::Color32::from_rgba_unmultiplied(60, 120, 70, 200)
                } else {
                    egui::Color32::from_rgba_unmultiplied(50, 50, 60, 180)
                })
                .stroke(egui::Stroke::new(
                    1.0,
                    if is_on {
                        egui::Color32::from_rgb(120, 200, 120)
                    } else {
                        egui::Color32::from_gray(80)
                    },
                ));
            let resp = ui.add_enabled(is_taggable, btn);
            let resp = resp.on_hover_text(if is_on {
                format!("クリックで `{with_hash}` を削除")
            } else {
                format!("クリックで `{with_hash}` を付与")
            });
            if resp.clicked() {
                *clicked_tag = Some(def.name.clone());
            }
        }
    });
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
