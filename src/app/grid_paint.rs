//! Grid cell painting helpers for the main thumbnail/details view.

use eframe::egui;

use super::{draw_rotated_image, drive_display_label, is_miv_upscaled_derivative};
use crate::grid_item::{GridItem, ThumbnailState};
use crate::ui_helpers::{draw_folder_badge, draw_pdf_badge, draw_play_icon, draw_zip_badge};

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

const BADGE_TEXT_TOP_PAD: f32 = 5.0;
const BADGE_TEXT_BOTTOM_PAD: f32 = 1.0;

/// `draw_cell` で **左下にコンテナバッジ** (フォルダ名 / "ZIP" / "PDF" / "7z" / "LZH")
/// が描かれるかを判定する純関数。
///
/// 用途: レーティング ★ バッジを左下に出すとき、コンテナバッジと重ねず縦に積む必要が
/// あるかを判定する (= ユーザー報告「フォルダ名と ★ が重なる」対策)。
///
/// 描画ロジックとの対応 (draw_cell 内):
/// - `Folder`: `ThumbnailState::Loaded` のときだけ `draw_folder_badge` を呼ぶ。
///   Pending / Evicted / Failed のときはセンターに 📁 アイコン + `draw_cell_filename`
///   なので左下バッジは描かれない。
/// - `ZipFile` / `PdfFile`: thumb の状態に関わらず常に badge_fn が呼ばれる
///   (Loaded はサムネ + ラベル、Pending はアイコンプレースホルダ + ラベル)。
/// - `ConvertibleArchive`: 常に `draw_archive_badge` が呼ばれる。
/// - `ZipDir` (v1.3.0): 内側アーカイブ (`is_archive`) は ZipFile と同じく常に
///   形式バッジ。ただのサブフォルダは Folder と同じく Loaded のときだけ
///   `draw_folder_badge` (Pending はセンター 📁 + ファイル名)。
/// - その他 (Image / Video / ZipImage / PdfPage / ZipSeparator): 左下にコンテナ
///   バッジは描かない (= false)。
pub(crate) fn cell_has_lower_left_container_badge(item: &GridItem, thumb: &ThumbnailState) -> bool {
    match item {
        GridItem::Folder(_) => matches!(thumb, ThumbnailState::Loaded { .. }),
        GridItem::ZipFile(_) | GridItem::PdfFile(_) | GridItem::ConvertibleArchive { .. } => true,
        GridItem::ZipDir { is_archive, .. } => {
            *is_archive || matches!(thumb, ThumbnailState::Loaded { .. })
        }
        _ => false,
    }
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
    has_page_override: bool, // true なら左上に補正済みバッジ「補」を表示
    has_local_adjust: bool,  // true なら左上に補正レイヤーバッジ「レ」を表示
    has_mask: bool,          // true なら左上に消しゴムマスクバッジ「消」を表示
    has_conceal: bool,       // true なら左上に隠蔽加工マスクバッジ「隠」を表示 (Phase 4)
    has_comic: bool,         // true なら左上にテキスト注釈バッジ「文」を表示
    rating: u8,              // 0 = 非表示, 1-5 = ★バッジ
    item: &GridItem,
    thumb: &ThumbnailState,
    rotation: crate::rotation_db::Rotation,
    // Some(tex) なら `ThumbnailState::Loaded.tex` の代わりにこちらを描画する
    // (色調補正済みサムネイルテクスチャ)。None または Loaded 以外なら生サムネ。
    adjusted_tex: Option<&egui::TextureHandle>,
    // 画像セルに表示する mIV タグ (`#原神` 等)。空なら非表示。
    tags: &[String],
    // コンテナセルに出す「フィルタ一致の子孫件数」。None ならバッジ非表示。
    filter_match_count: Option<u32>,
    // true なら **金色**の「📌」バッジを描画する (= ユーザーが Pin 操作した対象アイテム)。
    // 「現在表示中の親コンテナの pin source 先 = ユーザーがこのアイテムを選択して P
    // (or 📌) を押した」状態を示す。アイテム自身が pin 済みコンテナでも、ユーザーが
    // **そのアイテムに対して** Pin 操作したわけではないので badge は出さない (= 状態の
    // 二重提示を避け、「badge = 自分が Pin 操作した対象」を 1 対 1 で対応させる)。
    has_pin: bool,
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
        GridItem::Folder(path) => {
            let label;
            let name = if is_drive_list {
                label = drive_display_label(path).unwrap_or_else(|| path.display().to_string());
                label.as_str()
            } else {
                path.file_name().and_then(|n| n.to_str()).unwrap_or("")
            };
            match thumb {
                ThumbnailState::Loaded { tex, .. } => {
                    let use_tex = adjusted_tex.unwrap_or(tex);
                    draw_thumb_texture(painter, inner, use_tex, rotation);
                    draw_folder_badge(painter, inner, name);
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
                    crate::ui_helpers::draw_cell_filename(
                        painter,
                        inner,
                        name,
                        name_text_color,
                        dark,
                        0.0,
                    );
                }
            }
        }
        GridItem::Image(_) => {
            draw_thumb(painter, inner, thumb, rotation, dark, adjusted_tex);
        }
        GridItem::Video(path) => {
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
            if is_miv_upscaled_derivative(path) {
                draw_upscaled_video_badge(painter, inner);
            }
            // ファイル名
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            crate::ui_helpers::draw_cell_filename(painter, inner, name, name_text_color, dark, 0.0);
        }
        GridItem::ZipImage { .. } | GridItem::PdfPage { .. } => {
            draw_thumb(painter, inner, thumb, rotation, dark, adjusted_tex);
        }
        GridItem::ZipFile(path) | GridItem::PdfFile(path) => {
            let (icon, badge_fn): (&str, fn(&egui::Painter, egui::Rect)) =
                if matches!(item, GridItem::ZipFile(_)) {
                    ("📦", draw_zip_badge)
                } else {
                    ("📄", draw_pdf_badge)
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
            badge_fn(painter, inner);
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            crate::ui_helpers::draw_cell_filename(
                painter,
                inner,
                name,
                name_text_color,
                dark,
                crate::ui_helpers::estimated_file_badge_width(inner),
            );
        }
        GridItem::ConvertibleArchive { path, format } => {
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
            crate::ui_helpers::draw_archive_badge(painter, inner, format.label());
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            crate::ui_helpers::draw_cell_filename(
                painter,
                inner,
                name,
                name_text_color,
                dark,
                crate::ui_helpers::estimated_file_badge_width(inner),
            );
        }
        GridItem::ZipDir {
            is_archive,
            dir_prefix,
            ..
        } => {
            // ネスト ZIP ツリーの子コンテナ (v1.3.0)。内側アーカイブは ZipFile 風
            // (📦 + ZIP バッジ + 下部ファイル名)、ただのサブフォルダは Folder 風
            // (📁 + フォルダ名バッジ) に描く。代表サムネがロード済みならそれを使う。
            let name = crate::grid_item::zipdir_display_name(dir_prefix);
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
                // バッジは実フォーマットで出す: zip/cbz は青 ZIP バッジ、RAR/7z/LZH 由来の
                // セグメント (展開済み変換キャッシュのフラットパス) は ConvertibleArchive と
                // 同じ橙の形式バッジ (rar の本に "ZIP" と出る誤表示の修正、実機フィードバック)。
                let seg_ext = name.rsplit('.').next().unwrap_or("");
                match crate::archive_converter::ArchiveFormat::nested_from_extension(seg_ext) {
                    Some(fmt) if fmt != crate::archive_converter::ArchiveFormat::Zip => {
                        crate::ui_helpers::draw_archive_badge(painter, inner, fmt.label());
                    }
                    _ => crate::ui_helpers::draw_zip_badge(painter, inner),
                }
                crate::ui_helpers::draw_cell_filename(
                    painter,
                    inner,
                    name,
                    name_text_color,
                    dark,
                    crate::ui_helpers::estimated_file_badge_width(inner),
                );
            } else {
                match thumb {
                    ThumbnailState::Loaded { tex, .. } => {
                        draw_thumb_texture(painter, inner, tex, rotation);
                        crate::ui_helpers::draw_folder_badge(painter, inner, name);
                    }
                    ThumbnailState::Pending | ThumbnailState::Evicted | ThumbnailState::Failed => {
                        painter.text(
                            inner.center() - egui::vec2(0.0, 14.0),
                            egui::Align2::CENTER_CENTER,
                            "📁",
                            egui::FontId::proportional(42.0),
                            egui::Color32::from_rgb(220, 170, 30),
                        );
                        crate::ui_helpers::draw_cell_filename(
                            painter,
                            inner,
                            name,
                            name_text_color,
                            dark,
                            0.0,
                        );
                    }
                }
            }
        }
        GridItem::ZipSeparator { dir_display } => {
            // 作品境界のセパレータ: 1 セル全体に目立つ背景 + フォルダ名
            let (sep_bg, sep_stroke, sep_title, sep_small) = if dark {
                (
                    egui::Color32::from_rgb(35, 55, 85),
                    egui::Color32::from_rgb(100, 140, 200),
                    egui::Color32::from_rgb(200, 220, 250),
                    egui::Color32::from_gray(180),
                )
            } else {
                (
                    egui::Color32::from_rgb(235, 242, 252),
                    egui::Color32::from_rgb(120, 160, 220),
                    egui::Color32::from_rgb(40, 70, 140),
                    egui::Color32::from_gray(100),
                )
            };
            painter.rect_filled(inner, 6.0, sep_bg);
            painter.rect_stroke(
                inner,
                6.0,
                egui::Stroke::new(2.0, sep_stroke),
                egui::StrokeKind::Middle,
            );
            // 下部 "📁 作品の区切り" 用の予約幅 (タイトル領域算出にも使うため先に確定)
            let small = (inner.height() * 0.08).clamp(9.0, 16.0);
            let bottom_reserve = small * 1.4 + 6.0;
            // 階層フォルダは A/B ではなく `\n` 区切りで複数行に分け、Ctrl+G 検索結果と
            // 同じ自動縮小ロジック (draw_path_hierarchy) でセル幅にフィットさせる。
            const TOP_PAD: f32 = 6.0;
            const SIDE_PAD: f32 = 6.0;
            const MIN_TITLE_H: f32 = 14.0;
            let title_top = inner.min.y + TOP_PAD;
            let title_bottom_full = inner.max.y - bottom_reserve;
            let title_bottom = if title_bottom_full - title_top >= MIN_TITLE_H {
                title_bottom_full
            } else if (inner.max.y - TOP_PAD) - title_top >= MIN_TITLE_H {
                // 極小セルでは下部ラベル予約を諦める
                inner.max.y - TOP_PAD
            } else {
                inner.max.y
            };
            let title_rect = egui::Rect::from_min_max(
                egui::pos2(inner.min.x + SIDE_PAD, title_top),
                egui::pos2(inner.max.x - SIDE_PAD, title_bottom),
            );
            let components = crate::ui_helpers::split_path_components(dir_display);
            let max_font = (inner.height() * 0.14).clamp(14.0, 36.0);
            let min_font = 10.0;
            crate::ui_helpers::draw_path_hierarchy(
                painter,
                title_rect,
                &components,
                sep_title,
                max_font,
                min_font,
            );
            // 下部にフォルダアイコン的な記号
            painter.text(
                egui::pos2(inner.center().x, inner.max.y - 6.0),
                egui::Align2::CENTER_BOTTOM,
                "📁  作品の区切り",
                egui::FontId::proportional(small),
                sep_small,
            );
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

    // 左上バッジ列: 補 (ページ個別補正) → レ (補正レイヤー) →
    // 消 (消しゴムマスク) → 📌(金、pin) → タグバッジ。
    // 横並びで、収まらなければ末尾省略。
    // 並び順・表示条件・色・送り幅は `single_char_badges` / `single_char_badge_advance`
    // が正本 (タグバッジ当たり判定 `grid_tag_badge_start` と共有 — 片方だけ変えると
    // クリック判定がずれる)。
    {
        let font = egui::FontId::proportional(SINGLE_CHAR_BADGE_FONT_SIZE);
        let pad_x = SINGLE_CHAR_BADGE_PAD_X;
        let mut x = rect.min.x + SINGLE_CHAR_BADGE_START_OFFSET;
        let y = rect.min.y + SINGLE_CHAR_BADGE_START_OFFSET;
        // 1 文字バッジ (補/消/📌) を 1 個描いて x を進める。galley 実測ベースで高さ・幅を
        // 決め、固定 16px に CJK / 絵文字が収まらず上が欠ける問題 (タグ/★と同族) を避ける。
        // 📌 は emoji font fallback で metrics が違うが、独立に measure するので影響しない。
        for (glyph, bg, fg) in single_char_badges(
            has_page_override,
            has_local_adjust,
            has_mask,
            has_conceal,
            has_comic,
            has_pin,
        ) {
            let galley = painter.layout_no_wrap(glyph.to_string(), font.clone(), fg);
            let bg_w = galley.size().x + pad_x * 2.0;
            let bg_h = galley.size().y + BADGE_TEXT_TOP_PAD + BADGE_TEXT_BOTTOM_PAD;
            let badge_rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(bg_w, bg_h));
            painter.rect_filled(badge_rect, 3.0, bg);
            let text_pos = badge_rect.left_top() + egui::vec2(pad_x, BADGE_TEXT_TOP_PAD);
            painter.galley(text_pos, galley, fg);
            x += bg_w + SINGLE_CHAR_BADGE_GAP_X;
        }
        if !tags.is_empty() {
            // 残り幅 (右端 - 現在 x - 余白) に収まるだけ並べる。チェックマーク領域 (右上 24px)
            // と被らないように max_x を絞る。
            let max_x = rect.max.x - 28.0;
            draw_tag_badges(painter, egui::pos2(x, y), max_x, tags);
        }
    }

    // レーティングバッジ（1-5 ★、左下に半透明の背景付きで表示）
    // 画像系 (Image / ZipImage / PdfPage): 金色の ★
    // コンテナ系 (Folder / ZipFile / PdfFile): 銀青色の ★ + 先頭に 📁 アイコンを付与して
    //   「コンテナ自体への評価」であることを一目で区別できるようにする。
    //
    // コンテナ系で **左下バッジ** (folder 名 / "ZIP" / "PDF" / "7z" / "LZH") が既に
    // 描かれている場合、★ をその上に積んで重なりを回避する (= ユーザー報告対応)。
    // 判定は副作用ゼロの `cell_has_lower_left_container_badge` に集約。
    if rating >= 1 && rating <= 5 {
        let is_container = item.is_container_ratable();
        let star_color = if is_container {
            egui::Color32::from_rgb(180, 220, 255)
        } else {
            egui::Color32::from_rgb(255, 215, 50)
        };
        let text = if is_container {
            format!("📁{}", "★".repeat(rating as usize))
        } else {
            "★".repeat(rating as usize)
        };
        let font = egui::FontId::proportional(12.0);
        // ★ は fallback font (Segoe UI Symbol 等) 経由で描画され、固定 16px に
        // 収まらないことがある。実測ベースで背景矩形を作る (draw_filter_match_badge と同型)。
        let galley = painter.layout_no_wrap(text, font, star_color);
        let pad_x = 5.0;
        let bg_w = galley.size().x + pad_x * 2.0;
        let bg_h = galley.size().y + BADGE_TEXT_TOP_PAD + BADGE_TEXT_BOTTOM_PAD;
        // コンテナ系で左下バッジが描かれていれば 1 段上に積む。画像系 (Image /
        // ZipImage / PdfPage) や Folder の Pending 状態 (= ファイル名がセンター)
        // などは 0 (= 従来位置)。
        //
        // ★ も folder/zip/pdf/archive と同じ **inner** 基準で配置する:
        //   - x: `inner.min.x + 3.0` で folder badge と左端が揃う (rect ベースだと 4px 左にはみ出る)
        //   - y: `inner.max.y - bg_h - 3.0 - lift` で同じ anchor から計算
        // lift には gap 4px を追加して、folder badge との間に視覚的に余裕を出す
        // (実機フィードバック: container_badge_height の galley 近似 + anchor 差で
        // 単純な `+ 2.0` だと 1-2 px 重なるため、`+ 4.0` で安全側に倒す)。
        let star_y_lift = if cell_has_lower_left_container_badge(item, thumb) {
            crate::ui_helpers::container_badge_height(inner) + 4.0
        } else {
            0.0
        };
        let bg_rect = egui::Rect::from_min_size(
            egui::pos2(inner.min.x + 3.0, inner.max.y - bg_h - 3.0 - star_y_lift),
            egui::vec2(bg_w, bg_h),
        );
        painter.rect_filled(
            bg_rect,
            3.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 150),
        );
        let text_pos = bg_rect.left_top() + egui::vec2(pad_x, BADGE_TEXT_TOP_PAD);
        painter.galley(text_pos, galley, star_color);
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn grid_tag_badge_hit_rect(
    ui: &egui::Ui,
    rect: egui::Rect,
    has_page_override: bool,
    has_local_adjust: bool,
    has_mask: bool,
    has_conceal: bool,
    has_comic: bool,
    has_pin: bool,
    tags: &[String],
) -> Option<egui::Rect> {
    if tags.is_empty() {
        return None;
    }
    let painter = ui.painter();
    let start = grid_tag_badge_start(
        painter,
        rect,
        has_page_override,
        has_local_adjust,
        has_mask,
        has_conceal,
        has_comic,
        has_pin,
    );
    let max_x = rect.max.x - 28.0;
    layout_tag_badge(painter, start, max_x, tags).map(|(rect, _, _)| rect)
}

// ── 左上バッジ列のレイアウト定数 + 列定義 (描画と当たり判定の共有正本) ──────────

const SINGLE_CHAR_BADGE_FONT_SIZE: f32 = 11.0;
const SINGLE_CHAR_BADGE_PAD_X: f32 = 4.0;
const SINGLE_CHAR_BADGE_GAP_X: f32 = 2.0;
const SINGLE_CHAR_BADGE_START_OFFSET: f32 = 3.0;

/// 左上の 1 文字バッジ列の正本: 表示条件の順序 + (グリフ, 背景色, 前景色)。
/// `draw_cell` の描画と `grid_tag_badge_start` の当たり判定が**同じ列**を辿る。
/// バッジを追加・並べ替えるときはここだけ変えれば両方に効く。
fn single_char_badges(
    has_page_override: bool,
    has_local_adjust: bool,
    has_mask: bool,
    has_conceal: bool,
    has_comic: bool,
    has_pin: bool,
) -> Vec<(&'static str, egui::Color32, egui::Color32)> {
    let mut out = Vec::new();
    if has_page_override {
        out.push((
            "補",
            egui::Color32::from_rgb(50, 120, 220),
            egui::Color32::WHITE,
        ));
    }
    if has_local_adjust {
        out.push((
            "レ",
            egui::Color32::from_rgb(60, 150, 130),
            egui::Color32::WHITE,
        ));
    }
    if has_mask {
        out.push((
            "消",
            egui::Color32::from_rgb(200, 80, 40),
            egui::Color32::WHITE,
        ));
    }
    if has_conceal {
        // 隠蔽加工バッジ: 紫系 (補=青 / 消=オレンジと区別、`docs/conceal-feature-plan.md §12`)
        out.push((
            "隠",
            egui::Color32::from_rgb(153, 102, 204),
            egui::Color32::WHITE,
        ));
    }
    if has_comic {
        // テキスト注釈バッジ: ピンク系 (補=青 / レ=緑 / 消=オレンジ / 隠=紫 と区別)。
        out.push((
            "文",
            egui::Color32::from_rgb(210, 90, 160),
            egui::Color32::WHITE,
        ));
    }
    if has_pin {
        // 📌 (金色) — ユーザー設定の pin に関わるアイテムの目印。
        // アドレスバーの 📌 ボタンの色 (RGB 230,180,90) と統一する。
        out.push((
            "📌",
            egui::Color32::from_rgb(230, 180, 90),
            egui::Color32::from_rgb(60, 40, 10),
        ));
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn grid_tag_badge_start(
    painter: &egui::Painter,
    rect: egui::Rect,
    has_page_override: bool,
    has_local_adjust: bool,
    has_mask: bool,
    has_conceal: bool,
    has_comic: bool,
    has_pin: bool,
) -> egui::Pos2 {
    // draw_cell と同じ列定義 (`single_char_badges`) ・同じ送り幅式で進めるので、
    // 配置変更は正本側だけで済む。
    let font = egui::FontId::proportional(SINGLE_CHAR_BADGE_FONT_SIZE);
    let mut x = rect.min.x + SINGLE_CHAR_BADGE_START_OFFSET;
    for (glyph, _bg, fg) in single_char_badges(
        has_page_override,
        has_local_adjust,
        has_mask,
        has_conceal,
        has_comic,
        has_pin,
    ) {
        let galley = painter.layout_no_wrap(glyph.to_string(), font.clone(), fg);
        x += galley.size().x + SINGLE_CHAR_BADGE_PAD_X * 2.0 + SINGLE_CHAR_BADGE_GAP_X;
    }
    egui::pos2(x, rect.min.y + SINGLE_CHAR_BADGE_START_OFFSET)
}

fn layout_tag_badge(
    painter: &egui::Painter,
    start: egui::Pos2,
    max_x: f32,
    tags: &[String],
) -> Option<(egui::Rect, std::sync::Arc<egui::Galley>, egui::Color32)> {
    let font = egui::FontId::proportional(11.0);
    let pad_x = 5.0;
    let max_text_w = (max_x - start.x - pad_x * 2.0).max(0.0);
    if max_text_w < 8.0 {
        return None; // 領域不足 → 表示諦め
    }
    // `#` 始まり (mIV 付与) を優先、続いて他ソフト由来の裸タグを並べる。
    let mut combined = String::new();
    for t in tags.iter().filter(|t| t.starts_with('#')) {
        if !combined.is_empty() {
            combined.push(' ');
        }
        combined.push_str(t);
    }
    for t in tags.iter().filter(|t| !t.starts_with('#')) {
        if !combined.is_empty() {
            combined.push(' ');
        }
        combined.push_str(t);
    }
    if combined.is_empty() {
        return None;
    }

    // 実測ベースで省略する。CJK は 1 文字 ≒ 11px、ASCII は ≒ 6px と幅が大きく違うので、
    // 平均幅近似は使えない (`avg_char_w` で計算すると CJK が枠外にはみ出す)。
    let text_color = egui::Color32::from_rgb(180, 255, 180);
    let mut galley = painter.layout_no_wrap(combined.clone(), font.clone(), text_color);
    if galley.size().x > max_text_w {
        // 末尾から 1 文字ずつ削って `…` 付きで再 layout。最低 1 文字 + `…` は残す。
        let chars: Vec<char> = combined.chars().collect();
        for take in (1..chars.len()).rev() {
            let candidate: String = chars[..take].iter().collect::<String>() + "…";
            let g = painter.layout_no_wrap(candidate, font.clone(), text_color);
            if g.size().x <= max_text_w {
                galley = g;
                break;
            }
        }
        // それでも入らなければ `…` だけにする (極端に狭いセル)
        if galley.size().x > max_text_w {
            galley = painter.layout_no_wrap("…".to_string(), font.clone(), text_color);
            if galley.size().x > max_text_w {
                return None;
            }
        }
    }

    let bg_w = galley.size().x + pad_x * 2.0;
    // 高さも実測ベース。CJK が混じると 11pt でも 16px を超えることがあり、固定 16px だと
    // 中央寄せのオフセットが負になって文字がバッジ上端より上にはみ出していた。
    let bg_h = galley.size().y + BADGE_TEXT_TOP_PAD + BADGE_TEXT_BOTTOM_PAD;
    let bg_rect = egui::Rect::from_min_size(start, egui::vec2(bg_w, bg_h));
    Some((bg_rect, galley, text_color))
}

/// サムネイル左上 (補/消 バッジの右隣) にタグ (`#xxx #yyy`) を 1 つの緑バッジで描画する。
/// 幅は `painter.layout_no_wrap` で実測するので CJK / 絵文字でも正確に収まる。
/// `start.x` 以降、`max_x` まで使えるので、`max_x - start.x` を超える分は文字単位で削って
/// 末尾を `…` にする。空配列の呼び出しは `draw_cell` 側で弾かれている前提。
fn draw_tag_badges(painter: &egui::Painter, start: egui::Pos2, max_x: f32, tags: &[String]) {
    let Some((bg_rect, galley, text_color)) = layout_tag_badge(painter, start, max_x, tags) else {
        return;
    };
    painter.rect_filled(
        bg_rect,
        3.0,
        egui::Color32::from_rgba_unmultiplied(0, 40, 20, 170),
    );
    let pad_x = 5.0;
    let text_pos = bg_rect.left_top() + egui::vec2(pad_x, BADGE_TEXT_TOP_PAD);
    painter.galley(text_pos, galley, text_color);
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

/// Draws a compact badge for offline-upscaled video derivatives.
fn draw_upscaled_video_badge(painter: &egui::Painter, cell_rect: egui::Rect) {
    let font_size = (cell_rect.height() * 0.10).clamp(10.0, 14.0);
    let pad_x = font_size * 0.45;
    let pad_y = font_size * 0.22;
    let galley = painter.layout_no_wrap(
        "UP".to_owned(),
        egui::FontId::proportional(font_size),
        egui::Color32::WHITE,
    );
    let badge_rect = egui::Rect::from_min_size(
        cell_rect.min + egui::vec2(3.0, 3.0),
        egui::vec2(galley.size().x + pad_x * 2.0, galley.size().y + pad_y * 2.0),
    );
    painter.rect_filled(
        badge_rect,
        3.0,
        egui::Color32::from_rgba_unmultiplied(20, 120, 130, 215),
    );
    painter.galley(
        badge_rect.min + egui::vec2(pad_x, pad_y),
        galley,
        egui::Color32::WHITE,
    );
}
