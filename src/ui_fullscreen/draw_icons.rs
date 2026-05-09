use eframe::egui;

use crate::grid_item::GridItem;
use crate::pdf_loader::PdfPageContentType;
use crate::settings::SpreadMode;
use crate::ui_helpers::format_bytes_small;

use super::{BAR_BUTTON_SIZE, CHECKMARK_MARGIN, CHECKMARK_RADIUS};

// ── ホバーバーのアイコン描画関数 ────────────────────────────────────────

/// バーボタンの標準背景色を返す。
pub(super) fn bar_button_bg(hovered: bool, active: bool) -> egui::Color32 {
    if active {
        egui::Color32::from_rgba_unmultiplied(80, 140, 220, 200)
    } else if hovered {
        egui::Color32::from_rgba_unmultiplied(100, 100, 100, 200)
    } else {
        egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
    }
}

/// バーボタンの共通描画。位置とアイコン描画関数を受け取る。
pub(super) fn draw_bar_button(
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

/// "VST" ラベルを線分で自前描画する (= ホバーバーの VST3 管理ボタン用アイコン)。
/// proportional font だと S/T や V/S が重なって読みづらいため、line_segment で
/// 等幅 + 文字間 gap を確保する。3 文字を 32px のボタンに余裕を持って収める。
pub(super) fn draw_vst_text_label(painter: &egui::Painter, c: egui::Pos2) {
    let stroke = egui::Stroke::new(1.5, egui::Color32::WHITE);
    let char_w = 5.5_f32;
    let gap = 2.0_f32;
    let char_h = 10.0_f32;
    let group_w = 3.0 * char_w + 2.0 * gap;
    let top = c.y - char_h * 0.5;
    let bot = c.y + char_h * 0.5;
    let mid = c.y;
    let left = c.x - group_w * 0.5;

    // V
    let v_x0 = left;
    let v_x1 = v_x0 + char_w;
    let v_xc = (v_x0 + v_x1) * 0.5;
    painter.line_segment([egui::pos2(v_x0, top), egui::pos2(v_xc, bot)], stroke);
    painter.line_segment([egui::pos2(v_x1, top), egui::pos2(v_xc, bot)], stroke);

    // S (zigzag polyline)
    let s_x0 = v_x1 + gap;
    let s_x1 = s_x0 + char_w;
    painter.line_segment([egui::pos2(s_x1, top), egui::pos2(s_x0, top)], stroke);
    painter.line_segment([egui::pos2(s_x0, top), egui::pos2(s_x0, mid)], stroke);
    painter.line_segment([egui::pos2(s_x0, mid), egui::pos2(s_x1, mid)], stroke);
    painter.line_segment([egui::pos2(s_x1, mid), egui::pos2(s_x1, bot)], stroke);
    painter.line_segment([egui::pos2(s_x1, bot), egui::pos2(s_x0, bot)], stroke);

    // T
    let t_x0 = s_x1 + gap;
    let t_x1 = t_x0 + char_w;
    let t_xc = (t_x0 + t_x1) * 0.5;
    painter.line_segment([egui::pos2(t_x0, top), egui::pos2(t_x1, top)], stroke);
    painter.line_segment([egui::pos2(t_xc, top), egui::pos2(t_xc, bot)], stroke);
}

/// × アイコンを描画する。
pub(super) fn draw_close_icon(painter: &egui::Painter, c: egui::Pos2, _r: f32) {
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
pub(super) fn draw_pause_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let bar_w = r * 0.3;
    let gap = r * 0.35;
    let stroke = egui::Stroke::new(bar_w, egui::Color32::WHITE);
    painter.line_segment(
        [
            egui::pos2(c.x - gap, c.y - r),
            egui::pos2(c.x - gap, c.y + r),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(c.x + gap, c.y - r),
            egui::pos2(c.x + gap, c.y + r),
        ],
        stroke,
    );
}

/// 再生アイコン (右向き三角形) を描画する。
pub(super) fn draw_play_triangle(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let cx = c.x + r * 0.12;
    let points = vec![
        egui::pos2(cx - r * 0.5, c.y - r * 0.75),
        egui::pos2(cx - r * 0.5, c.y + r * 0.75),
        egui::pos2(cx + r * 0.7, c.y),
    ];
    painter.add(egui::Shape::convex_polygon(
        points,
        egui::Color32::WHITE,
        egui::Stroke::NONE,
    ));
}

/// 🔬 分析アイコン（虫眼鏡＋十字線）を描画する。
/// タイルモード アイコン: 2x2 の塗りつぶし正方形 (= ■ ■ / ■ ■)。
/// 旧 ▦ 文字は font glyph によっては tofu (□) に化けるため自前描画に切替。
pub(super) fn draw_tile_grid_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let white = egui::Color32::WHITE;
    // セル 1 個の半幅、セル間ギャップ
    let cell = r * 0.36;
    let gap = r * 0.10;
    let off = cell + gap * 0.5;
    for &(dx, dy) in &[(-1.0_f32, -1.0_f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let cx = c.x + dx * off;
        let cy = c.y + dy * off;
        painter.rect_filled(
            egui::Rect::from_center_size(egui::pos2(cx, cy), egui::vec2(cell, cell)),
            1.0,
            white,
        );
    }
}

pub(super) fn draw_analysis_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
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
        [
            egui::pos2(lens_cx - ch, lens_cy),
            egui::pos2(lens_cx + ch, lens_cy),
        ],
        egui::Stroke::new(1.2, white),
    );
    painter.line_segment(
        [
            egui::pos2(lens_cx, lens_cy - ch),
            egui::pos2(lens_cx, lens_cy + ch),
        ],
        egui::Stroke::new(1.2, white),
    );
}

/// ℹ アイコンを描画する。
pub(super) fn draw_info_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let white = egui::Color32::WHITE;
    painter.circle_stroke(c, r, egui::Stroke::new(1.5, white));
    let bar_w = r * 0.22;
    painter.line_segment(
        [
            egui::pos2(c.x, c.y - r * 0.05),
            egui::pos2(c.x, c.y + r * 0.55),
        ],
        egui::Stroke::new(bar_w, white),
    );
    painter.circle_filled(egui::pos2(c.x, c.y - r * 0.45), bar_w * 0.7, white);
}

/// チェックマーク（右上）を描画する。
pub(super) fn draw_fs_checkmark(ui: &mut egui::Ui, full_rect: egui::Rect) {
    let check_center = egui::pos2(
        full_rect.max.x - CHECKMARK_RADIUS - CHECKMARK_MARGIN,
        full_rect.min.y + CHECKMARK_RADIUS + CHECKMARK_MARGIN,
    );
    ui.painter().circle_filled(
        check_center,
        CHECKMARK_RADIUS,
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

/// 上部ホバーバー左側の表示文字列を組み立てる。
///
/// - `ZipImage`: `<archive-path> > <entry_name>`
/// - `PdfPage`:  `<pdf-path> > Page N`
/// - それ以外: `<folder>\<filename>` (Windows パス区切り)
///
/// `base_folder` は `effective_folder()` の表示文字列を想定 (変換済みアーカイブ
/// 閲覧中は元 7z/LZH のパスが渡ってくる)。空文字列なら基底パス部分を省略する。
pub(super) fn compute_location_display(
    item: Option<&GridItem>,
    base_folder: &str,
    filename: &str,
) -> String {
    match item {
        Some(GridItem::ZipImage { entry_name, .. }) => {
            if base_folder.is_empty() {
                entry_name.clone()
            } else {
                format!("{base_folder} > {entry_name}")
            }
        }
        Some(GridItem::PdfPage { page_num, .. }) => {
            if base_folder.is_empty() {
                format!("Page {}", page_num + 1)
            } else {
                format!("{base_folder} > Page {}", page_num + 1)
            }
        }
        _ => {
            // 通常画像・動画・ZipSeparator 等: folder + basename を連結。
            if base_folder.is_empty() {
                filename.to_string()
            } else if filename.is_empty() {
                base_folder.to_string()
            } else {
                let ends_with_sep =
                    base_folder.ends_with(std::path::MAIN_SEPARATOR) || base_folder.ends_with('/');
                if ends_with_sep {
                    format!("{base_folder}{filename}")
                } else {
                    format!("{base_folder}{}{filename}", std::path::MAIN_SEPARATOR)
                }
            }
        }
    }
}

const DOWNSCALE_MARKER: &str = " ⚠ ダウンスケール表示中";

/// ファイル情報テキスト (PDF 種別・寸法・AI・ファイルサイズ) を描画する。
/// ファイル名は左側 `location_display` に統合済みなのでここでは扱わない。
pub(super) fn draw_fs_bar_info_text(
    ui: &mut egui::Ui,
    bar_rect: egui::Rect,
    right_anchor: egui::Pos2,
    image_dims: Option<(u32, u32)>,
    image_file_size: Option<u64>,
    image_downscaled: bool,
    ai_upscale_info: Option<(&str, u32, u32)>,
    pdf_content_type: Option<PdfPageContentType>,
) {
    let text = build_info_text(
        image_dims,
        image_file_size,
        image_downscaled,
        ai_upscale_info,
        pdf_content_type,
    );
    if !text.is_empty() {
        // ダウンスケール警告だけ黄色で強調したいのでマーカー部分を切り分けて描画する。
        let (main_text, has_marker) = if image_downscaled && text.ends_with(DOWNSCALE_MARKER) {
            (&text[..text.len() - DOWNSCALE_MARKER.len()], true)
        } else {
            (text.as_str(), false)
        };
        let marker = DOWNSCALE_MARKER;
        let font = egui::FontId::proportional(15.0);
        if has_marker {
            // 右詰で書くので、まず marker を右端に、次に main_text をその左に置く。
            let marker_galley = ui.painter().layout_no_wrap(
                marker.to_string(),
                font.clone(),
                egui::Color32::from_rgb(255, 210, 80),
            );
            ui.painter().galley(
                egui::pos2(
                    right_anchor.x - marker_galley.size().x,
                    right_anchor.y - marker_galley.size().y * 0.5,
                ),
                marker_galley.clone(),
                egui::Color32::from_rgb(255, 210, 80),
            );
            let main_anchor = egui::pos2(right_anchor.x - marker_galley.size().x, right_anchor.y);
            ui.painter().text(
                main_anchor,
                egui::Align2::RIGHT_CENTER,
                main_text,
                font,
                egui::Color32::WHITE,
            );
        } else {
            ui.painter().text(
                right_anchor,
                egui::Align2::RIGHT_CENTER,
                text,
                font,
                egui::Color32::WHITE,
            );
        }
    }
    let _ = bar_rect;
}

/// 上部バー右側に表示する画像情報テキスト (PDF 種別 / 寸法 / AI / サイズ) を組み立てる。
/// `image_downscaled` が true の場合、dims の直後 (AI 情報がある場合はその後) に
/// `⚠ ダウンスケール表示中` マーカーを挿入する。
pub(super) fn build_info_text(
    image_dims: Option<(u32, u32)>,
    image_file_size: Option<u64>,
    image_downscaled: bool,
    ai_upscale_info: Option<(&str, u32, u32)>,
    pdf_content_type: Option<PdfPageContentType>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
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
        let mut dims_part = if let Some((model_name, ai_w, ai_h)) = ai_upscale_info {
            // AI アップスケール情報: "11 × 22 (漫画 44×88)"
            format!("{w} × {h} ({model_name} {ai_w}×{ai_h})")
        } else {
            format!("{w} × {h}")
        };
        if image_downscaled {
            dims_part.push_str(DOWNSCALE_MARKER);
        }
        parts.push(dims_part);
    }
    if let Some(bytes) = image_file_size {
        parts.push(format_bytes_small(bytes));
    }
    parts.join("    ")
}

/// 動画モード時の上ホバーバー右側情報を組み立てる (Phase 6)。
/// `(W × H、長さ mm:ss、平均 X.X Mbps、ファイルサイズ)` をスペース区切りで連結。
/// 各項目は値が無い (0 / None) ならスキップ。
pub(super) fn build_info_text_video(
    dims: Option<(u32, u32)>,
    file_size: Option<u64>,
    video_meta: Option<(f64, i64)>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some((w, h)) = dims {
        parts.push(format!("{w} × {h}"));
    }
    if let Some((dur, bitrate)) = video_meta {
        if dur > 0.0 {
            parts.push(crate::ui_helpers::format_hms(dur));
        }
        if bitrate > 0 {
            parts.push(crate::ui_helpers::format_bitrate_bps(bitrate));
        }
    }
    if let Some(bytes) = file_size {
        parts.push(format_bytes_small(bytes));
    }
    parts.join("    ")
}

/// 回転アイコンを自前描画する。
pub(super) fn draw_rotate_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    clockwise: bool,
) {
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
pub(super) fn draw_spread_icon(painter: &egui::Painter, c: egui::Pos2, r: f32, mode: SpreadMode) {
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
pub(super) fn draw_spread_direction_arrow(
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
            [
                egui::pos2(ax - alen, ay),
                egui::pos2(ax - alen + ahead, ay - ahead),
            ],
            arrow_stroke,
        );
        painter.line_segment(
            [
                egui::pos2(ax - alen, ay),
                egui::pos2(ax - alen + ahead, ay + ahead),
            ],
            arrow_stroke,
        );
    } else {
        // →
        painter.line_segment(
            [egui::pos2(ax - alen, ay), egui::pos2(ax + alen, ay)],
            arrow_stroke,
        );
        painter.line_segment(
            [
                egui::pos2(ax + alen, ay),
                egui::pos2(ax + alen - ahead, ay - ahead),
            ],
            arrow_stroke,
        );
        painter.line_segment(
            [
                egui::pos2(ax + alen, ay),
                egui::pos2(ax + alen - ahead, ay + ahead),
            ],
            arrow_stroke,
        );
    }
}
