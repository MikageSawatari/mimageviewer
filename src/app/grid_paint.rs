//! Grid cell painting helpers for the main thumbnail/details view.

use eframe::egui;

use super::{draw_rotated_image, drive_display_label, is_miv_upscaled_derivative};
use crate::grid_item::{GridItem, ThumbnailState};
use crate::thumb_overlay_layout::{
    BottomContainerInput, BottomContainerKind, EditBadgeFlags, ThumbnailOverlayLayout,
    ThumbnailOverlayLayoutInput, layout_thumbnail_overlays,
};
use crate::ui_helpers::draw_play_icon;

/// サムネイルテクスチャをアスペクト保持で中央配置して描画する（回転対応）。
fn draw_thumb_texture(
    painter: &egui::Painter,
    inner: egui::Rect,
    tex: &egui::TextureHandle,
    rotation: crate::rotation_db::Rotation,
) {
    let tex_size = tex.size_vec2();
    // 90°/270° 回転時は幅と高さが入れ替わる
    let display_size = match rotation {
        crate::rotation_db::Rotation::Cw90 | crate::rotation_db::Rotation::Cw270 => {
            egui::vec2(tex_size.y, tex_size.x)
        }
        _ => tex_size,
    };
    let scale = (inner.width() / display_size.x).min(inner.height() / display_size.y);
    let img_rect = egui::Rect::from_center_size(inner.center(), display_size * scale);

    // 透過画像の背景はフルスクリーンと同じ黒に揃える (v0.7.0 フィードバック反映)。
    // セル全体ではなく img_rect (実際に画像が描かれる領域) だけを塗るので、
    // フォルダラベルや letterbox の白背景は維持される。
    painter.rect_filled(img_rect, 0.0, egui::Color32::BLACK);

    if rotation.is_none() {
        painter.image(
            tex.id(),
            img_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        // 回転したテクスチャを Mesh で描画
        draw_rotated_image(painter, tex.id(), img_rect, rotation);
    }
}

/// 画像系アイテム (Image / ZipImage) のサムネイル状態に応じた描画。
fn draw_thumb(
    painter: &egui::Painter,
    inner: egui::Rect,
    thumb: &ThumbnailState,
    rotation: crate::rotation_db::Rotation,
    dark: bool,
    adjusted_tex: Option<&egui::TextureHandle>,
) {
    match thumb {
        ThumbnailState::Loaded { tex, .. } => {
            let use_tex = adjusted_tex.unwrap_or(tex);
            draw_thumb_texture(painter, inner, use_tex, rotation);
        }
        ThumbnailState::Pending | ThumbnailState::Evicted => {
            let bg = if dark {
                egui::Color32::from_gray(50)
            } else {
                egui::Color32::from_gray(220)
            };
            painter.rect_filled(inner, 2.0, bg);
            painter.text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                "読込中",
                egui::FontId::proportional(12.0),
                egui::Color32::from_gray(140),
            );
        }
        ThumbnailState::Failed => {
            let bg = if dark {
                egui::Color32::from_rgb(80, 30, 30)
            } else {
                egui::Color32::from_rgb(255, 220, 220)
            };
            let fg = if dark {
                egui::Color32::from_rgb(255, 160, 160)
            } else {
                egui::Color32::DARK_RED
            };
            painter.rect_filled(inner, 2.0, bg);
            painter.text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                "読込失敗",
                egui::FontId::proportional(12.0),
                fg,
            );
        }
    }
}

/// ファイル名スタックの集約セル右上に「N 枚」バッジを描く (v2.0.0)。
/// スタックは複数枚画像をまとめた仮想コンテナなので、通常画像との見分けを付ける。
/// スタックはチェック非対象なので右上のチェックオーバーレイとは衝突しない。
fn draw_stack_count_badge(painter: &egui::Painter, inner: egui::Rect, count: usize) {
    let text = format!("{count} 枚");
    let font = egui::FontId::proportional((inner.height() * 0.09).clamp(11.0, 16.0));
    let galley = painter.layout_no_wrap(text, font, egui::Color32::WHITE);
    let pad = egui::vec2(6.0, 3.0);
    let size = galley.size() + pad * 2.0;
    // バッジの右上をセル右上 (inner.max.x-4, inner.min.y+4) に合わせる。
    let anchor = egui::pos2(inner.max.x - 4.0, inner.min.y + 4.0);
    let badge_rect = egui::Rect::from_min_max(
        egui::pos2(anchor.x - size.x, anchor.y),
        egui::pos2(anchor.x, anchor.y + size.y),
    );
    painter.rect_filled(
        badge_rect,
        4.0,
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 190),
    );
    painter.galley(badge_rect.min + pad, galley, egui::Color32::WHITE);
}

/// What the bottom-left lane shows for one item: a container badge, a filename plate, or both.
///
/// Split out of [`layout_cell_overlays`] so the item-kind rules stay unit-testable without a
/// `Painter`. The rule that a folder falls back to a plain filename until its thumbnail is
/// loaded predates the lane rework and had its own tests; keeping the mapping pure keeps them.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BottomLeftContent<'a> {
    pub container_kind: Option<BottomContainerKind>,
    pub container_label: Option<&'a str>,
    pub filename: Option<&'a str>,
}

pub(crate) fn bottom_left_content<'a>(
    item: &GridItem,
    thumb: &ThumbnailState,
    item_name: &'a str,
) -> BottomLeftContent<'a> {
    let mut container_kind = None;
    let mut container_label = None;
    let mut filename = None;
    match item {
        GridItem::Folder(_) => {
            if matches!(thumb, ThumbnailState::Loaded { .. }) {
                container_kind = Some(BottomContainerKind::Folder);
                container_label = Some(item_name);
            } else {
                filename = Some(item_name);
            }
        }
        GridItem::Video(_) | GridItem::Audio(_) => filename = Some(item_name),
        GridItem::ZipFile(_) => {
            container_kind = Some(BottomContainerKind::Format);
            container_label = Some("ZIP");
            filename = Some(item_name);
        }
        GridItem::PdfFile(_) => {
            container_kind = Some(BottomContainerKind::Format);
            container_label = Some("PDF");
            filename = Some(item_name);
        }
        GridItem::ConvertibleArchive { format, .. } => {
            container_kind = Some(BottomContainerKind::Format);
            container_label = Some(format.label());
            filename = Some(item_name);
        }
        GridItem::ZipDir { is_archive, .. } if *is_archive => {
            let extension = item_name.rsplit('.').next().unwrap_or_default();
            container_kind = Some(BottomContainerKind::Format);
            container_label = Some(
                crate::archive_converter::ArchiveFormat::nested_from_extension(extension)
                    .filter(|format| *format != crate::archive_converter::ArchiveFormat::Zip)
                    .map_or("ZIP", |format| format.label()),
            );
            filename = Some(item_name);
        }
        GridItem::ZipDir { .. } => {
            if matches!(thumb, ThumbnailState::Loaded { .. }) {
                container_kind = Some(BottomContainerKind::Folder);
                container_label = Some(item_name);
            } else {
                filename = Some(item_name);
            }
        }
        _ => {}
    }
    BottomLeftContent {
        container_kind,
        container_label,
        filename,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn layout_cell_overlays(
    painter: &egui::Painter,
    rect: egui::Rect,
    edit_badges: EditBadgeFlags,
    rating: u8,
    item: &GridItem,
    thumb: &ThumbnailState,
    tags: &[String],
    bookmark_time: Option<&str>,
    is_drive_list: bool,
) -> ThumbnailOverlayLayout {
    let inner = rect.shrink(4.0);
    let item_name = match item {
        GridItem::Folder(path) if is_drive_list => {
            drive_display_label(path).unwrap_or_else(|| path.display().to_string())
        }
        GridItem::Folder(path)
        | GridItem::Video(path)
        | GridItem::Audio(path)
        | GridItem::ZipFile(path)
        | GridItem::PdfFile(path) => path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned(),
        GridItem::ConvertibleArchive { path, .. } => path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned(),
        GridItem::ZipDir { dir_prefix, .. } => {
            crate::grid_item::zipdir_display_name(dir_prefix).to_owned()
        }
        _ => String::new(),
    };

    let BottomLeftContent {
        container_kind: bottom_kind,
        container_label: bottom_label,
        filename,
    } = bottom_left_content(item, thumb, &item_name);

    let rating_text = (1..=5).contains(&rating).then(|| {
        let stars = "★".repeat(rating as usize);
        if item.is_container_ratable() {
            format!("📁{stars}")
        } else {
            stars
        }
    });
    let bottom_container = bottom_kind
        .zip(bottom_label)
        .map(|(kind, label)| BottomContainerInput { kind, label });
    let upscaled_video = matches!(item, GridItem::Video(path) if is_miv_upscaled_derivative(path));

    layout_thumbnail_overlays(
        ThumbnailOverlayLayoutInput {
            cell: rect,
            inner,
            bookmark_time,
            upscaled_video,
            edit_badges,
            tags,
            bottom_container,
            rating_text: rating_text.as_deref(),
            filename,
        },
        |text, style| crate::ui_helpers::measure_thumbnail_badge_text(painter, text, style),
    )
}

fn draw_drive_icon(painter: &egui::Painter, inner: egui::Rect, dark: bool) {
    let side = inner.width().min(inner.height());
    let w = (side * 0.48).clamp(36.0, 72.0);
    let h = (side * 0.34).clamp(24.0, 48.0);
    let center = inner.center() - egui::vec2(0.0, 12.0);
    let body = egui::Rect::from_center_size(center, egui::vec2(w, h));
    let fill = if dark {
        egui::Color32::from_rgb(78, 88, 100)
    } else {
        egui::Color32::from_rgb(210, 218, 226)
    };
    let face = if dark {
        egui::Color32::from_rgb(48, 56, 66)
    } else {
        egui::Color32::from_rgb(244, 247, 250)
    };
    let stroke = if dark {
        egui::Color32::from_rgb(130, 145, 160)
    } else {
        egui::Color32::from_rgb(120, 134, 150)
    };
    painter.rect_filled(body, 5.0, fill);
    painter.rect_stroke(
        body,
        5.0,
        egui::Stroke::new(1.5, stroke),
        egui::StrokeKind::Middle,
    );
    let front = egui::Rect::from_min_max(
        egui::pos2(body.min.x + w * 0.12, body.center().y + h * 0.12),
        egui::pos2(body.max.x - w * 0.12, body.max.y - h * 0.16),
    );
    painter.rect_filled(front, 2.0, face);
    painter.line_segment(
        [
            egui::pos2(body.min.x + w * 0.18, body.min.y + h * 0.32),
            egui::pos2(body.max.x - w * 0.18, body.min.y + h * 0.32),
        ],
        egui::Stroke::new(1.4, stroke),
    );
    painter.circle_filled(
        egui::pos2(front.max.x - w * 0.10, front.center().y),
        (side * 0.025).clamp(2.0, 3.5),
        egui::Color32::from_rgb(70, 190, 120),
    );
}

pub(crate) fn draw_cell(
    ui: &egui::Ui,
    rect: egui::Rect,
    is_selected: bool,
    is_checked: bool,
    is_spread_pair_cursor: bool,
    overlay_layout: &ThumbnailOverlayLayout,
    item: &GridItem,
    thumb: &ThumbnailState,
    rotation: crate::rotation_db::Rotation,
    // Some(tex) なら `ThumbnailState::Loaded.tex` の代わりにこちらを描画する
    // (色調補正済みサムネイルテクスチャ)。None または Loaded 以外なら生サムネ。
    adjusted_tex: Option<&egui::TextureHandle>,
    // コンテナセルに出す「フィルタ一致の子孫件数」。None ならバッジ非表示。
    filter_match_count: Option<u32>,
    is_drive_list: bool,
) {
    if !ui.is_rect_visible(rect) {
        return;
    }

    let painter = ui.painter();
    let padding = 4.0;
    let inner = rect.shrink(padding);

    let dark = ui.visuals().dark_mode;
    let name_text_color = if dark {
        egui::Color32::from_gray(210)
    } else {
        egui::Color32::from_gray(30)
    };
    let pending_placeholder_bg = if dark {
        egui::Color32::from_gray(50)
    } else {
        egui::Color32::from_gray(230)
    };

    // カーソル位置 (selected) もマルチ選択チェック済み (checked) も同じ青背景に。
    // カーソル位置を示す太い青枠は selected のときだけ付く (下の border 判定で分岐)。
    let bg = if is_selected || is_checked {
        if dark {
            egui::Color32::from_rgb(40, 70, 110)
        } else {
            egui::Color32::from_rgb(180, 210, 255)
        }
    } else if dark {
        egui::Color32::from_gray(28)
    } else {
        egui::Color32::WHITE
    };
    painter.rect_filled(rect, 2.0, bg);

    match item {
        GridItem::Folder(_) => match thumb {
            ThumbnailState::Loaded { tex, .. } => {
                let use_tex = adjusted_tex.unwrap_or(tex);
                draw_thumb_texture(painter, inner, use_tex, rotation);
            }
            ThumbnailState::Pending | ThumbnailState::Evicted | ThumbnailState::Failed => {
                if is_drive_list {
                    draw_drive_icon(painter, inner, dark);
                } else {
                    painter.text(
                        inner.center() - egui::vec2(0.0, 14.0),
                        egui::Align2::CENTER_CENTER,
                        "📁",
                        egui::FontId::proportional(42.0),
                        egui::Color32::from_rgb(220, 170, 30),
                    );
                }
            }
        },
        GridItem::Image(_) => {
            draw_thumb(painter, inner, thumb, rotation, dark, adjusted_tex);
        }
        GridItem::Video(_) => {
            match thumb {
                ThumbnailState::Loaded { tex, .. } => {
                    // 動画サムネは補正対象外 (adjusted_tex は常に None)
                    draw_thumb_texture(painter, inner, tex, rotation);
                }
                ThumbnailState::Pending | ThumbnailState::Evicted => {
                    painter.rect_filled(inner, 2.0, egui::Color32::from_gray(40));
                    painter.text(
                        inner.center(),
                        egui::Align2::CENTER_CENTER,
                        "動画",
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_gray(160),
                    );
                }
                ThumbnailState::Failed => {
                    painter.rect_filled(inner, 2.0, egui::Color32::from_gray(40));
                }
            }
            // 再生ボタンオーバーレイ（常時表示）
            let r = (inner.width().min(inner.height()) * 0.18).max(10.0);
            draw_play_icon(painter, inner.center(), r);
        }
        GridItem::Audio(_) => {
            // 音声は固定の音楽アイコン (波形サムネは生成しない、D2)。サムネ状態に依らず
            // 常に同じアイコンを描く。
            painter.rect_filled(inner, 2.0, pending_placeholder_bg);
            crate::ui_helpers::draw_music_icon(painter, inner, dark);
        }
        GridItem::ZipImage { .. } | GridItem::PdfPage { .. } => {
            draw_thumb(painter, inner, thumb, rotation, dark, adjusted_tex);
        }
        GridItem::ZipFile(_) | GridItem::PdfFile(_) => {
            let icon = if matches!(item, GridItem::ZipFile(_)) {
                "📦"
            } else {
                "📄"
            };
            match thumb {
                ThumbnailState::Loaded { tex, .. } => {
                    // ZipFile/PdfFile の代表サムネは補正対象外 (adjusted_tex は常に None)
                    draw_thumb_texture(painter, inner, tex, rotation);
                }
                ThumbnailState::Pending | ThumbnailState::Evicted | ThumbnailState::Failed => {
                    painter.rect_filled(inner, 2.0, pending_placeholder_bg);
                    painter.text(
                        inner.center(),
                        egui::Align2::CENTER_CENTER,
                        icon,
                        egui::FontId::proportional(32.0),
                        egui::Color32::from_gray(120),
                    );
                }
            }
        }
        GridItem::ConvertibleArchive { .. } => {
            // RAR / 7z / LZH: 有効な変換キャッシュ ZIP があれば、その先頭画像 / ピン画像を
            // 表示する。未変換・キャッシュ失効時は汎用アーカイブアイコンへフォールバック。
            match thumb {
                ThumbnailState::Loaded { tex, .. } => {
                    draw_thumb_texture(painter, inner, tex, rotation);
                }
                ThumbnailState::Pending | ThumbnailState::Evicted | ThumbnailState::Failed => {
                    painter.rect_filled(inner, 2.0, pending_placeholder_bg);
                    painter.text(
                        inner.center(),
                        egui::Align2::CENTER_CENTER,
                        "🗜",
                        egui::FontId::proportional(32.0),
                        egui::Color32::from_gray(120),
                    );
                }
            }
        }
        GridItem::ZipDir { is_archive, .. } => {
            // ネスト ZIP ツリーの子コンテナ (v1.3.0)。内側アーカイブは ZipFile 風
            // (📦 + ZIP バッジ + 下部ファイル名)、ただのサブフォルダは Folder 風
            // (📁 + フォルダ名バッジ) に描く。代表サムネがロード済みならそれを使う。
            if *is_archive {
                match thumb {
                    ThumbnailState::Loaded { tex, .. } => {
                        draw_thumb_texture(painter, inner, tex, rotation);
                    }
                    ThumbnailState::Pending | ThumbnailState::Evicted | ThumbnailState::Failed => {
                        painter.rect_filled(inner, 2.0, pending_placeholder_bg);
                        painter.text(
                            inner.center(),
                            egui::Align2::CENTER_CENTER,
                            "📦",
                            egui::FontId::proportional(32.0),
                            egui::Color32::from_gray(120),
                        );
                    }
                }
            } else {
                match thumb {
                    ThumbnailState::Loaded { tex, .. } => {
                        draw_thumb_texture(painter, inner, tex, rotation);
                    }
                    ThumbnailState::Pending | ThumbnailState::Evicted | ThumbnailState::Failed => {
                        painter.text(
                            inner.center() - egui::vec2(0.0, 14.0),
                            egui::Align2::CENTER_CENTER,
                            "📁",
                            egui::FontId::proportional(42.0),
                            egui::Color32::from_rgb(220, 170, 30),
                        );
                    }
                }
            }
        }
        GridItem::SearchContainer {
            path,
            kind,
            hit_count,
            representative,
        } => {
            let (icon, label_color) = match kind {
                crate::grid_item::SearchContainerKind::Folder => (
                    "📁",
                    if dark {
                        egui::Color32::from_gray(220)
                    } else {
                        egui::Color32::from_gray(60)
                    },
                ),
                crate::grid_item::SearchContainerKind::Zip => (
                    "📦",
                    if dark {
                        egui::Color32::from_rgb(220, 200, 150)
                    } else {
                        egui::Color32::from_rgb(130, 90, 30)
                    },
                ),
            };

            // 代表サムネがあって GPU テクスチャがロード済みなら、セル上部にサムネ、
            // 下部に少し背景色の付いたボックスで「フォルダ階層 + ヒット件数」を出す。
            // 未ロード / 代表サムネなしのときは従来どおりアイコン + 階層パスで埋める
            // (サムネが読み込まれるまでの placeholder)。
            let thumb_loaded =
                representative.is_some() && matches!(thumb, ThumbnailState::Loaded { .. });

            if thumb_loaded {
                let thumb_h = inner.height() * 0.62;
                let thumb_rect = egui::Rect::from_min_max(
                    inner.min,
                    egui::pos2(inner.max.x, inner.min.y + thumb_h),
                );
                if let ThumbnailState::Loaded { tex, .. } = thumb {
                    // 代表サムネは色調補正対象外 (adjusted_tex は常に None)
                    draw_thumb_texture(painter, thumb_rect, tex, rotation);
                }
                // 種別アイコン (小) を左上隅に重ねて Folder/ZIP を示す
                let badge_size = (thumb_rect.height() * 0.22).clamp(14.0, 28.0);
                painter.text(
                    egui::pos2(thumb_rect.min.x + 4.0, thumb_rect.min.y + 4.0),
                    egui::Align2::LEFT_TOP,
                    icon,
                    egui::FontId::proportional(badge_size),
                    label_color,
                );

                // 下部の「少し背景色を付けたボックス」: ユーザー要望どおりフォルダ名を
                // サムネから切り離して読みやすくする。
                let label_rect = egui::Rect::from_min_max(
                    egui::pos2(inner.min.x, thumb_rect.max.y + 2.0),
                    inner.max,
                );
                let label_bg = if dark {
                    egui::Color32::from_rgb(38, 42, 50)
                } else {
                    egui::Color32::from_rgb(240, 240, 246)
                };
                painter.rect_filled(label_rect, 3.0, label_bg);

                let badge_font = (label_rect.height() * 0.19).clamp(10.0, 14.0);
                let text_rect = egui::Rect::from_min_max(
                    egui::pos2(label_rect.min.x + 4.0, label_rect.min.y + 2.0),
                    egui::pos2(label_rect.max.x - 4.0, label_rect.max.y - badge_font * 1.3),
                );
                let path_str = path.to_string_lossy();
                let components = crate::ui_helpers::split_path_components(&path_str);
                let max_font = (label_rect.height() * 0.24).clamp(10.0, 13.0);
                crate::ui_helpers::draw_path_hierarchy(
                    painter,
                    text_rect,
                    &components,
                    label_color,
                    max_font,
                    5.0,
                );
                let badge_text = format!("{} 枚", hit_count);
                let badge_color = if dark {
                    egui::Color32::from_rgb(240, 200, 100)
                } else {
                    egui::Color32::from_rgb(180, 80, 0)
                };
                painter.text(
                    egui::pos2(label_rect.max.x - 6.0, label_rect.max.y - 4.0),
                    egui::Align2::RIGHT_BOTTOM,
                    &badge_text,
                    egui::FontId::proportional(badge_font),
                    badge_color,
                );
            } else {
                // 代表サムネなし or 未ロード: 従来どおりアイコン + 階層パス + バッジ
                // (日付フォルダ `2025-01-01` 等を単独で識別できるよう階層を多行表示)
                let icon_size = (inner.height() * 0.18).clamp(22.0, 56.0);
                painter.text(
                    egui::pos2(inner.center().x, inner.min.y + icon_size * 0.75),
                    egui::Align2::CENTER_CENTER,
                    icon,
                    egui::FontId::proportional(icon_size),
                    label_color,
                );
                let badge_font = (inner.height() * 0.07).clamp(10.0, 14.0);
                let text_rect = egui::Rect::from_min_max(
                    egui::pos2(inner.min.x + 4.0, inner.min.y + icon_size * 1.35),
                    egui::pos2(inner.max.x - 4.0, inner.max.y - badge_font * 2.2),
                );
                let path_str = path.to_string_lossy();
                let components = crate::ui_helpers::split_path_components(&path_str);
                let max_font = (inner.height() * 0.075).clamp(11.0, 15.0);
                crate::ui_helpers::draw_path_hierarchy(
                    painter,
                    text_rect,
                    &components,
                    label_color,
                    max_font,
                    8.0,
                );
                let badge_text = format!("{} 枚", hit_count);
                let badge_color = if dark {
                    egui::Color32::from_rgb(240, 200, 100)
                } else {
                    egui::Color32::from_rgb(180, 80, 0)
                };
                painter.text(
                    egui::pos2(inner.max.x - 6.0, inner.max.y - 6.0),
                    egui::Align2::RIGHT_BOTTOM,
                    &badge_text,
                    egui::FontId::proportional(badge_font),
                    badge_color,
                );
            }
        }
        GridItem::Stack { count, .. } => {
            // ファイル名スタックの集約セル: 代表画像を通常サムネと同様に描き、
            // 右上に枚数バッジ (= スタックの目印)。単独グループは GridItem::Image で
            // 描かれるのでここには来ない (= count は常に 2 以上)。
            draw_thumb(painter, inner, thumb, rotation, dark, adjusted_tex);
            draw_stack_count_badge(painter, inner, *count);
        }
    }

    let border = if is_selected {
        egui::Stroke::new(2.0, egui::Color32::from_rgb(60, 120, 220))
    } else {
        egui::Stroke::new(
            1.0,
            if dark {
                egui::Color32::from_gray(70)
            } else {
                egui::Color32::from_gray(200)
            },
        )
    };
    painter.rect_stroke(rect, 2.0, border, egui::StrokeKind::Middle);
    if is_spread_pair_cursor && !is_selected {
        draw_spread_pair_cursor(painter, rect, ui.visuals());
    }

    // チェックマークオーバーレイ
    if is_checked {
        let check_r = 12.0;
        let check_center = egui::pos2(rect.max.x - check_r - 4.0, rect.min.y + check_r + 4.0);
        painter.circle_filled(check_center, check_r, egui::Color32::from_rgb(40, 140, 40));
        // チェックマーク (✓)
        let s = check_r * 0.55;
        let stroke = egui::Stroke::new(2.5, egui::Color32::WHITE);
        painter.line_segment(
            [
                egui::pos2(check_center.x - s * 0.6, check_center.y),
                egui::pos2(check_center.x - s * 0.1, check_center.y + s * 0.5),
            ],
            stroke,
        );
        painter.line_segment(
            [
                egui::pos2(check_center.x - s * 0.1, check_center.y + s * 0.5),
                egui::pos2(check_center.x + s * 0.7, check_center.y - s * 0.5),
            ],
            stroke,
        );
    }

    if let Some(placement) = overlay_layout.top_left.upscaled_video.as_ref() {
        crate::ui_helpers::draw_overlay_upscaled_video_badge(painter, placement);
    }
    for placement in &overlay_layout.top_left.edit_badges {
        let crate::thumb_overlay_layout::BadgeKind::Edit(kind) = placement.kind else {
            continue;
        };
        crate::ui_helpers::draw_overlay_edit_badge(painter, placement, kind);
    }
    if let Some(placement) = overlay_layout.top_left.tag.as_ref() {
        crate::ui_helpers::draw_overlay_tag_badge(painter, placement);
    }

    if let Some(placement) = overlay_layout.bottom_left.container.as_ref() {
        match placement.kind {
            crate::thumb_overlay_layout::BadgeKind::BottomContainer(
                BottomContainerKind::Folder,
            ) => crate::ui_helpers::draw_overlay_folder_badge(painter, placement),
            crate::thumb_overlay_layout::BadgeKind::BottomContainer(
                BottomContainerKind::Format,
            ) => crate::ui_helpers::draw_overlay_format_badge(painter, placement),
            _ => {}
        }
    }
    if let Some(placement) = overlay_layout.bottom_left.filename.as_ref() {
        crate::ui_helpers::draw_cell_filename(painter, placement, name_text_color, dark);
    }
    if let Some(placement) = overlay_layout.bottom_left.rating.as_ref() {
        crate::ui_helpers::draw_overlay_rating_badge(
            painter,
            placement,
            item.is_container_ratable(),
        );
    }

    if let Some(count) = filter_match_count {
        if item.is_container_ratable() && count > 0 {
            draw_filter_match_badge(painter, rect, count);
        }
    }
}

pub(crate) fn draw_spread_pair_cursor(
    painter: &egui::Painter,
    rect: egui::Rect,
    visuals: &egui::Visuals,
) {
    let rect = rect.shrink(3.0);
    if rect.width() <= 4.0 || rect.height() <= 4.0 {
        return;
    }
    let color = if visuals.dark_mode {
        egui::Color32::from_rgb(130, 185, 255)
    } else {
        egui::Color32::from_rgb(35, 95, 210)
    };
    let stroke = egui::Stroke::new(2.0, color);
    draw_dashed_segment(painter, rect.left_top(), rect.right_top(), stroke);
    draw_dashed_segment(painter, rect.right_top(), rect.right_bottom(), stroke);
    draw_dashed_segment(painter, rect.right_bottom(), rect.left_bottom(), stroke);
    draw_dashed_segment(painter, rect.left_bottom(), rect.left_top(), stroke);
}

fn draw_dashed_segment(
    painter: &egui::Painter,
    start: egui::Pos2,
    end: egui::Pos2,
    stroke: egui::Stroke,
) {
    let delta = end - start;
    let len = delta.length();
    if len <= 0.1 {
        return;
    }
    let dir = delta / len;
    let dash = 7.0;
    let gap = 5.0;
    let mut pos = 0.0;
    while pos < len {
        let next = (pos + dash).min(len);
        painter.line_segment([start + dir * pos, start + dir * next], stroke);
        pos += dash + gap;
    }
}

fn draw_filter_match_badge(painter: &egui::Painter, cell_rect: egui::Rect, count: u32) {
    let text = if count >= 1000 {
        "999+".to_string()
    } else {
        count.to_string()
    };
    let font = egui::FontId::proportional(11.0);
    let galley = painter.layout_no_wrap(text, font, egui::Color32::WHITE);
    let pad_x = 5.0;
    let pad_y = 2.0;
    let bg_w = galley.size().x + pad_x * 2.0;
    let bg_h = galley.size().y + pad_y * 2.0;
    let bg_rect = egui::Rect::from_min_size(
        egui::pos2(cell_rect.max.x - bg_w - 3.0, cell_rect.max.y - bg_h - 3.0),
        egui::vec2(bg_w, bg_h),
    );
    painter.rect_filled(bg_rect, 3.0, egui::Color32::from_rgb(0xE6, 0x7E, 0x22));
    let text_pos = bg_rect.left_top() + egui::vec2(pad_x, pad_y);
    painter.galley(text_pos, galley, egui::Color32::WHITE);
}

pub(crate) fn primary_grid_tag_for_badge(tags: &[String]) -> Option<&str> {
    tags.iter()
        .find(|tag| tag.starts_with('#'))
        .or_else(|| tags.first())
        .map(String::as_str)
}

pub(crate) fn grid_tag_badge_hit_rect(layout: &ThumbnailOverlayLayout) -> Option<egui::Rect> {
    layout.top_left.tag.as_ref().map(|placement| placement.rect)
}

/// サムネイル画質プレビュー用: 実グリッドと同じ `cell_w × cell_h` のセルを描画する。
/// 白背景 + 4px パディング、画像はアスペクト保持で中央配置（draw_cell と同じ方式）。
/// クリック可能で、クリック時は Response.clicked() が true になる。
pub(crate) fn tq_draw_preview(
    ui: &mut egui::Ui,
    tex: &Option<egui::TextureHandle>,
    cell_w: f32,
    cell_h: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(cell_w, cell_h), egui::Sense::click());
    let painter = ui.painter();
    // 白背景（選択状態ではないグリッドセルと同じ）
    painter.rect_filled(rect, 2.0, egui::Color32::WHITE);

    let padding = 4.0;
    let inner = rect.shrink(padding);

    match tex {
        Some(t) => {
            let tex_size = t.size_vec2();
            let scale = (inner.width() / tex_size.x).min(inner.height() / tex_size.y);
            let img_size = tex_size * scale;
            let img_rect = egui::Rect::from_center_size(inner.center(), img_size);
            painter.image(
                t.id(),
                img_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => {
            painter.text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                "エンコード失敗",
                egui::FontId::proportional(14.0),
                egui::Color32::from_gray(120),
            );
        }
    }

    // ホバー時にカーソル変更 + 縁を青くしてクリック可能さを示す
    if response.hovered() {
        painter.rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 150, 220)),
            egui::StrokeKind::Outside,
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    response
}

#[cfg(test)]
mod bottom_left_content_tests {
    use super::{BottomContainerKind, bottom_left_content};
    use crate::grid_item::{GridItem, ThumbnailState};
    use std::path::PathBuf;

    // 左下レーンのコンテナバッジ / ファイル名プレートの出し分け。レーン共通化 (§2.2) の前は
    // `cell_has_lower_left_container_badge` として同じ規則を検証していた。ユーザー報告
    // 「フォルダ名と ★ が重なる」の退行ガードなので、レイアウト層へ移した後も残す。

    fn loaded() -> ThumbnailState {
        let ctx = egui::Context::default();
        ThumbnailState::Loaded {
            tex: ctx.load_texture(
                "dummy",
                egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
                Default::default(),
            ),
            from_cache: false,
            from_edit_preview: false,
            rendered_at_px: 128,
            source_dims: None,
        }
    }

    #[test]
    fn folder_shows_its_name_badge_once_the_thumbnail_is_loaded() {
        let folder = GridItem::Folder(PathBuf::from("c:/x"));
        let content = bottom_left_content(&folder, &loaded(), "x");
        assert_eq!(content.container_kind, Some(BottomContainerKind::Folder));
        assert_eq!(content.container_label, Some("x"));
        assert_eq!(content.filename, None);
    }

    #[test]
    fn folder_falls_back_to_a_filename_plate_until_it_is_loaded() {
        let folder = GridItem::Folder(PathBuf::from("c:/x"));
        for (label, thumb) in [
            ("pending", ThumbnailState::Pending),
            ("evicted", ThumbnailState::Evicted),
            ("failed", ThumbnailState::Failed),
        ] {
            let content = bottom_left_content(&folder, &thumb, "x");
            assert_eq!(content.container_kind, None, "{label}");
            assert_eq!(content.filename, Some("x"), "{label}");
        }
    }

    #[test]
    fn archive_types_always_show_a_format_badge() {
        let zip = GridItem::ZipFile(PathBuf::from("c:/x.zip"));
        let pdf = GridItem::PdfFile(PathBuf::from("c:/x.pdf"));
        let archive = GridItem::ConvertibleArchive {
            path: PathBuf::from("c:/x.7z"),
            format: crate::archive_converter::ArchiveFormat::SevenZ,
        };
        for thumb in [
            ThumbnailState::Pending,
            ThumbnailState::Evicted,
            ThumbnailState::Failed,
        ] {
            for (item, label) in [(&zip, "ZIP"), (&pdf, "PDF"), (&archive, "7z")] {
                let content = bottom_left_content(item, &thumb, "x");
                assert_eq!(
                    content.container_kind,
                    Some(BottomContainerKind::Format),
                    "{label}"
                );
                assert_eq!(content.container_label, Some(label));
            }
        }
    }

    #[test]
    fn image_like_items_have_no_container_badge() {
        let image = GridItem::Image(PathBuf::from("c:/x.jpg"));
        let zip_image = GridItem::ZipImage {
            zip_path: PathBuf::from("c:/x.zip"),
            entry_name: "p1.jpg".to_string(),
        };
        let pdf_page = GridItem::PdfPage {
            pdf_path: PathBuf::from("c:/x.pdf"),
            page_num: 0,
            content_type: None,
        };
        for item in [&image, &zip_image, &pdf_page] {
            let content = bottom_left_content(item, &ThumbnailState::Pending, "x");
            assert_eq!(content.container_kind, None);
        }
        // 動画・音声はコンテナではないが、ファイル名プレートは出る。
        let video = GridItem::Video(PathBuf::from("c:/x.mp4"));
        let content = bottom_left_content(&video, &ThumbnailState::Pending, "x.mp4");
        assert_eq!(content.container_kind, None);
        assert_eq!(content.filename, Some("x.mp4"));
    }
}
