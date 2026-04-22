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

        if !force_show {
            let hover_threshold = full_rect.max.x - full_rect.width() * 0.25;

            // ホバー判定: 画面右 1/4
            let hover_in_right = ctx.input(|i| {
                i.pointer
                    .hover_pos()
                    .map(|p| p.x > hover_threshold)
                    .unwrap_or(false)
            });

            let visible = self.show_metadata_panel || hover_in_right;
            if !visible {
                return false;
            }
        }

        // 右パネルは常に上部バーの下から開始（上バーは常に同時表示される）
        let panel_top = full_rect.min.y + TOP_BAR_H;

        let panel_rect = egui::Rect::from_min_max(
            egui::pos2(full_rect.max.x - panel_w, panel_top),
            full_rect.max,
        );

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
        }

        // ── コンテンツ領域 (タイトルバーの下) ──
        let content_top = title_rect.max.y;
        let content_rect =
            egui::Rect::from_min_max(egui::pos2(panel_rect.min.x, content_top), panel_rect.max);

        let ai_metadata = self.get_current_ai_metadata();
        let exif_info = self.get_current_exif();
        let tweet_info = self.get_current_tweet_info();

        // タグパネル用の情報を先に集める (child_ui の &mut ui closure 前に借用を解消するため)
        let taggable_path = self.current_taggable_image_path();
        let current_tags: Vec<String> = if taggable_path.is_some() {
            self.get_current_tags_cached()
        } else {
            Vec::new()
        };
        let defined_tags = self.settings.tags.clone();

        // タグボタンクリックを closure 内で検出し、後段で request_tag_toggle を走らせる
        let mut clicked_tag: Option<String> = None;

        let inner_rect = content_rect.shrink2(egui::vec2(12.0, 8.0));
        let mut child_ui = ui.new_child(egui::UiBuilder::new().max_rect(inner_rect));
        child_ui.set_clip_rect(content_rect);

        egui::ScrollArea::vertical()
            .id_salt("metadata_scroll")
            .show(&mut child_ui, |ui| {
                ui.set_width(inner_rect.width());

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
                    if tweet_info.is_some() || ai_metadata.is_some() || exif_info.is_some() {
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

                // 何もない場合
                if ai_metadata.is_none()
                    && exif_info.is_none()
                    && tweet_info.is_none()
                    && defined_tags.is_empty()
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

    /// 現在のフルスクリーン画像のタグ一覧 (XMP dc:subject) を取得する。
    /// キャッシュにあればそれを返し、なければ同期的に読み込んでキャッシュする。
    /// 対応形式外 (ZIP 内画像 / PDF ページ / HEIC 等) は空 Vec を返す。
    pub(crate) fn get_current_tags_cached(&mut self) -> Vec<String> {
        let Some(idx) = self.fullscreen_idx else {
            return Vec::new();
        };
        let Some(key) = self.metadata_cache_key(idx) else {
            return Vec::new();
        };
        if let Some(cached) = self.tags_cache.get(&key) {
            return cached.clone();
        }
        // タグ対象外 (ZIP/PDF/フォルダ等) は空 Vec をキャッシュして終わり
        let Some(path) = self.current_taggable_image_path() else {
            self.tags_cache.insert(key, Vec::new());
            return Vec::new();
        };
        let tags = crate::xmp_reader::read_dc_subject(&path);
        self.tags_cache.insert(key, tags.clone());
        tags
    }

    /// 現在のフルスクリーン画像がタグ付与対象 (通常画像 JPEG/PNG/WebP) なら Path を返す。
    /// ZIP 内・PDF ページ・フォルダなどは None。
    fn current_taggable_image_path(&self) -> Option<std::path::PathBuf> {
        use crate::grid_item::GridItem;
        let idx = self.fullscreen_idx?;
        if let Some(GridItem::Image(p)) = self.items.get(idx) {
            // 拡張子判定は tag_ops と同じ基準
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase())?;
            if matches!(ext.as_str(), "jpg" | "jpeg" | "jfif" | "png" | "webp") {
                return Some(p.clone());
            }
        }
        None
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
            ui.label(
                egui::RichText::new("(対応形式外)")
                    .size(10.0)
                    .color(DIM_COLOR),
            )
            .on_hover_text(
                "このファイルはタグ書き込みに対応していません。\n\
                 JPEG / PNG / WebP のみ対応しています。",
            );
        }
    });
    ui.add_space(4.0);

    // ボタンを折り返し配置。付与中は `#タグ名` を緑色で、未付与は通常色で表示する
    // (丸ドット等の装飾は付けない: ラベルの色とボタン背景で状態を伝える)。
    ui.horizontal_wrapped(|ui| {
        for def in defined_tags {
            let with_hash = format!("#{}", def.name);
            let is_on = current_tags.iter().any(|t| *t == with_hash);
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
                .size(BODY_FONT),
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
            font_id: egui::FontId::proportional(BODY_FONT),
            color: TEXT_COLOR,
            ..Default::default()
        },
    );
    ui.label(job);
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
    ui.label(egui::RichText::new(text).color(TEXT_COLOR).size(BODY_FONT));
}
