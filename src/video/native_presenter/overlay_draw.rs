use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{
    NativeBookmarkTitleEdit, NativeFrameStepHold, NativeOverlayCommand, NativeOverlayJumpEntry,
    NativeOverlayMetadata, NativeOverlayNavigationPreview, NativeOverlayPerfSample,
    NativeOverlayPerfSnapshot, NativeOverlayThumbnail, NativeOverlayTileOverlay,
    NativeOverlayTimelineMarker, NativeOverlayTimelineMarkerKind, NativeOverlayToast,
    NativeOverlayVst3ChainSlot, NativeOverlayVst3Panel, NativeOverlayVst3Slot,
    NativeOverlayVst3SlotState,
};

const NATIVE_PERF_GRAPH_SECS: f32 = 6.0;
const NATIVE_PERF_AV_OFFSET_NORMAL_MS: f32 = 100.0;
const NATIVE_PERF_AV_OFFSET_SEVERE_MS: f32 = 500.0;

pub(super) fn draw_native_perf_overlay(
    ctx: &egui::Context,
    overlay_width_points: f32,
    _overlay_height_points: f32,
    history: &[NativeOverlayPerfSample],
    latest: NativeOverlayPerfSnapshot,
    origin: egui::Pos2,
) {
    egui::Area::new(egui::Id::new("native_video_perf_overlay"))
        .order(egui::Order::Middle)
        .fixed_pos(origin)
        .show(ctx, |ui| {
            let width = overlay_width_points.min(460.0).max(300.0);
            let panel_rect =
                egui::Rect::from_min_size(ui.min_rect().min, egui::vec2(width, 158.0));
            ui.set_min_size(panel_rect.size());
            let painter = ui.painter().clone();
            painter.rect_filled(
                panel_rect,
                5.0,
                egui::Color32::from_rgba_unmultiplied(8, 10, 14, 218),
            );
            painter.rect_stroke(
                panel_rect,
                5.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 42)),
                egui::StrokeKind::Inside,
            );

            let graph = egui::Rect::from_min_max(
                panel_rect.min + egui::vec2(10.0, 48.0),
                panel_rect.max - egui::vec2(10.0, 34.0),
            );
            let display_fps = native_perf_visible_fps(history).unwrap_or(0.0);
            let title = format!(
                "native {:.1} fps  frames {}  GPU {} CPU {}",
                display_fps, latest.presented, latest.gpu, latest.cpu
            );
            painter.text(
                panel_rect.min + egui::vec2(10.0, 9.0),
                egui::Align2::LEFT_TOP,
                title,
                egui::FontId::monospace(12.0),
                egui::Color32::from_rgb(235, 238, 244),
            );
            let warn = if latest.late_drop > 0 || latest.wait_timeout > 0 {
                egui::Color32::from_rgb(255, 112, 112)
            } else {
                egui::Color32::from_rgb(154, 236, 178)
            };
            painter.text(
                panel_rect.min + egui::vec2(panel_rect.width() - 10.0, 9.0),
                egui::Align2::RIGHT_TOP,
                format!("drop {} timeout {}", latest.late_drop, latest.wait_timeout),
                egui::FontId::monospace(12.0),
                warn,
            );
            // ── ヘッダ 2 行目: clock 統計 + A/V offset + audio lead + UNDERRUN ──
            // `A/V` は **体感の音映像差** (av_offset_ms = video_pts − audio_audible_pts) を
            // 主指標とする。当初使っていた `av_drift_ms` (= video_pts − master_clock) は
            // Norm clear バグなど audio が clock から乖離した場面で値が変わらず、
            // 「音と映像がズレているのに数値が動かない」という体感と乖離する。
            // 詳細は `docs/video-architecture.md` の「A/V drift 計装」節参照。
            const LEAD_VISIBLE_MIN_WIDTH: f32 = 380.0;
            const GROUP_GAP: f32 = 24.0; // A/V グループと lead / UNDERRUN グループの間
            let audio_active = latest.audio_active;
            let underrun_active = audio_active && latest.audio_underrun_active;
            let wide_enough_for_status = panel_rect.width() >= LEAD_VISIBLE_MIN_WIDTH;
            let clock_text = if underrun_active && !wide_enough_for_status {
                format!("late {:.1}ms", latest.max_late_ms)
            } else {
                format!(
                    "clock late {:.1}ms  max dt {:.1}ms",
                    latest.max_late_ms, latest.max_interval_ms
                )
            };
            painter.text(
                panel_rect.min + egui::vec2(10.0, 25.0),
                egui::Align2::LEFT_TOP,
                clock_text,
                egui::FontId::monospace(10.0),
                egui::Color32::from_rgb(168, 176, 188),
            );

            // ── 右端の値表示: label + value を分けて描く ──
            //
            // **桁ぶれ問題への対応** (= 2026-05-11):
            // 旧版は `format!("A/V {:>+8.1}ms", v)` の 1 行で右寄せしていたが、mIV の
            // monospace family は `ui_fonts.rs` で先頭に Yu Gothic Medium (proportional)
            // が挿入されているため、leading space と digit の advance 幅が違って
            // 桁数変化のたびにテキスト全体が左右に動いていた。
            //
            // 対策: label 部分と value 部分を **別の painter.text** で描き、value 部分は
            // **固定 right edge** で右寄せする。これで value 文字数が変わっても
            // value の右端 / label の位置は動かない (= 視覚的に「動かない」)。
            // 各 value slot は ~10 char (~70px) で「±9999.9ms」までを含意する余裕。
            const VALUE_SLOT_W: f32 = 70.0; // 約 10 char @ 7px (monospace)
            const LABEL_VALUE_GAP: f32 = 4.0;

            let av_offset_finite = latest.av_offset_ms.is_finite();
            let display_value = if av_offset_finite {
                latest.av_offset_ms
            } else {
                // audio inactive または seek 直後など offset 未確定のときは
                // video pacing drift にフォールバックする。
                latest.av_drift_ms
            };
            let av_color = native_perf_av_value_color(display_value);
            let av_label = if av_offset_finite { "A/V" } else { "vid" };
            // value は padding なし。slot 右端で右寄せ → 文字数変化で左端だけ動く (= label と干渉せず)。
            let av_value_text = format!("{:+.1}ms", display_value);
            let av_value_right = panel_rect.min.x + panel_rect.width() - 10.0;
            let av_value_left = av_value_right - VALUE_SLOT_W;
            let row_y = panel_rect.min.y + 25.0;
            painter.text(
                egui::pos2(av_value_right, row_y),
                egui::Align2::RIGHT_TOP,
                &av_value_text,
                egui::FontId::monospace(10.0),
                av_color,
            );
            // label の右端 = value_slot 左端 - gap (= label の位置は完全に固定)
            painter.text(
                egui::pos2(av_value_left - LABEL_VALUE_GAP, row_y),
                egui::Align2::RIGHT_TOP,
                av_label,
                egui::FontId::monospace(10.0),
                av_color,
            );

            // 右端 2 (A/V の左): audio が master clock より何 ms 先行しているか。
            // 通常 ≈ 0、Norm clear バグ時は +5000ms 級。
            // panel width が狭いと左の「clock late ...」と重なるので、380px 未満は省略。
            //
            // ⚠ レイアウト: label "lead" + value_slot (= A/V と同じ 70px slot)。
            // av_label の左端から GROUP_GAP 左に lead value slot の右端を置く。
            // UNDERRUN は critical warning なので狭幅でも表示し、左の clock 表示を短縮して
            // 重なりを避ける。lead は通常診断値なので狭幅では省略する。
            let show_underrun = underrun_active;
            let show_lead = audio_active && !show_underrun && wide_enough_for_status;
            // av_label の x 位置を保守的に概算 (3 char ≈ 21px)。
            let av_label_left_approx = av_value_left - LABEL_VALUE_GAP - 24.0;
            let status_right = av_label_left_approx - GROUP_GAP;
            if show_lead {
                let lead_value_right = av_label_left_approx - GROUP_GAP;
                let lead_value_left = lead_value_right - VALUE_SLOT_W;
                let lead_value_text = format!("{:+.1}ms", latest.audio_lead_ms);
                let lead_color = if latest.audio_lead_ms.abs() < 50.0 {
                    egui::Color32::from_rgb(168, 176, 188) // グレー (= 通常)
                } else {
                    egui::Color32::from_rgb(255, 152, 60) // 橙 (= clock 乖離)
                };
                painter.text(
                    egui::pos2(lead_value_right, row_y),
                    egui::Align2::RIGHT_TOP,
                    &lead_value_text,
                    egui::FontId::monospace(10.0),
                    lead_color,
                );
                painter.text(
                    egui::pos2(lead_value_left - LABEL_VALUE_GAP, row_y),
                    egui::Align2::RIGHT_TOP,
                    "lead",
                    egui::FontId::monospace(10.0),
                    lead_color,
                );
            }

            // 右側ステータス: UNDERRUN は lead と同じ枠で排他的に描く。
            // 絵文字は使わない (CLAUDE.md 遵守)。
            if show_underrun {
                painter.text(
                    egui::pos2(status_right, row_y),
                    egui::Align2::RIGHT_TOP,
                    "UNDERRUN",
                    egui::FontId::monospace(10.0),
                    egui::Color32::from_rgb(255, 95, 95),
                );
            }

            painter.rect_filled(
                graph,
                2.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 16),
            );
            let expected_ms = native_perf_expected_frame_ms(history);
            let y_max_ms = (expected_ms * 2.0).clamp(8.0, 160.0);
            let y_for_ms = |ms: f32| {
                graph.max.y - (ms.clamp(0.0, y_max_ms) / y_max_ms) * graph.height()
            };
            let grid_lines = [
                (expected_ms * 0.5, format!("{:.1}", expected_ms * 0.5)),
                (expected_ms, format!("{:.1}", expected_ms)),
                (expected_ms * 2.0, format!("{:.0}", expected_ms * 2.0)),
            ];
            for (ms, label) in grid_lines {
                let y = y_for_ms(ms);
                painter.line_segment(
                    [egui::pos2(graph.min.x, y), egui::pos2(graph.max.x, y)],
                    egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 34),
                    ),
                );
                painter.text(
                    egui::pos2(graph.max.x - 2.0, y - 1.0),
                    egui::Align2::RIGHT_BOTTOM,
                    label,
                    egui::FontId::monospace(9.0),
                    egui::Color32::from_rgb(160, 166, 176),
                );
            }

            // ── A/V offset サブトラック用の Y スケール (±200ms 中心) ──
            // graph 中央が offset=0、上端が +200ms、下端が −200ms。
            // Norm clear バグ時は ±5000ms 級になるが、グラフは ±200 で saturate するので
            // 「飽和して上端 / 下端に張り付く」=「異常」のサインとして読める。
            let drift_y_max_ms: f32 = 200.0;
            let drift_center_y = (graph.min.y + graph.max.y) * 0.5;
            let drift_half_h = graph.height() * 0.5;
            let drift_y_for_ms = |ms: f32| {
                let clamped = ms.clamp(-drift_y_max_ms, drift_y_max_ms);
                drift_center_y - (clamped / drift_y_max_ms) * drift_half_h
            };
            // 0ms ライン (= drift センター) を点線で示す。
            {
                let y0 = drift_center_y;
                let dash_w: f32 = 6.0;
                let gap_w: f32 = 4.0;
                let mut x = graph.min.x;
                while x < graph.max.x {
                    let x_end = (x + dash_w).min(graph.max.x);
                    painter.line_segment(
                        [egui::pos2(x, y0), egui::pos2(x_end, y0)],
                        egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgba_unmultiplied(110, 220, 240, 80),
                        ),
                    );
                    x = x_end + gap_w;
                }
            }

            if let Some(last) = history.last() {
                let now = last.arrival;
                let px_per_sec = graph.width() / NATIVE_PERF_GRAPH_SECS;
                let mut prev_interval = None;
                let mut prev_total = None;
                let mut prev_drift = None;
                let mut last_draw_x = f32::INFINITY;
                let clipped = painter.with_clip_rect(graph);
                for (idx, sample) in history.iter().enumerate() {
                    let age = now.saturating_duration_since(sample.arrival).as_secs_f32();
                    if age > NATIVE_PERF_GRAPH_SECS {
                        continue;
                    }
                    let x = graph.max.x - age * px_per_sec;
                    if native_perf_should_thin_sample(sample, last_draw_x, x, idx, history.len()) {
                        continue;
                    }
                    last_draw_x = x;

                    // underrun 区間は橙色背景帯。赤縦線は frame drop 専用なので、
                    // audio 側の警告は別色で帯として見せる。
                    if native_perf_sample_has_audio_underrun_band(sample) {
                        clipped.line_segment(
                            [egui::pos2(x, graph.min.y), egui::pos2(x, graph.max.y)],
                            egui::Stroke::new(
                                2.0,
                                egui::Color32::from_rgba_unmultiplied(255, 165, 60, 70),
                            ),
                        );
                    }

                    let interval_y = y_for_ms(sample.interval_ms);
                    let total_y = y_for_ms(sample.total_ms);
                    let copy_y = y_for_ms(sample.copy_ms);
                    // サブトラック描画は av_offset (= 体感ズレ) を主とする。audio inactive
                    // または offset 未確定の sample は NaN なので skip (= prev_drift を更新しない)。
                    let drift_value = if sample.av_offset_ms.is_finite() {
                        Some(sample.av_offset_ms)
                    } else {
                        None
                    };
                    let interval_point = egui::pos2(x, interval_y);
                    let total_point = egui::pos2(x, total_y);
                    let drift_point = drift_value.map(|v| egui::pos2(x, drift_y_for_ms(v)));
                    if let Some(prev) = prev_interval {
                        clipped.line_segment(
                            [prev, interval_point],
                            egui::Stroke::new(1.8, egui::Color32::from_rgb(111, 211, 255)),
                        );
                    }
                    if let Some(prev) = prev_total {
                        clipped.line_segment(
                            [prev, total_point],
                            egui::Stroke::new(1.2, egui::Color32::from_rgb(255, 194, 87)),
                        );
                    }
                    if let (Some(prev), Some(curr)) = (prev_drift, drift_point) {
                        // av_offset サブトラック: 薄シアン (interval の濃い水色と区別)。
                        clipped.line_segment(
                            [prev, curr],
                            egui::Stroke::new(
                                1.4,
                                egui::Color32::from_rgba_unmultiplied(180, 240, 250, 200),
                            ),
                        );
                    }
                    clipped.circle_filled(
                        egui::pos2(x, copy_y),
                        1.4,
                        egui::Color32::from_rgb(178, 236, 135),
                    );
                    if native_perf_sample_has_late_drop(sample) {
                        clipped.line_segment(
                            [egui::pos2(x, graph.min.y), egui::pos2(x, graph.max.y)],
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 95, 95)),
                        );
                    }
                    prev_interval = Some(interval_point);
                    prev_total = Some(total_point);
                    if let Some(curr) = drift_point {
                        prev_drift = Some(curr);
                    } else {
                        prev_drift = None;
                    }
                }
            }

            let latest_sample = history.last().copied();
            let interval = latest_sample.map(|s| s.interval_ms).unwrap_or(0.0);
            let total = latest_sample.map(|s| s.total_ms).unwrap_or(0.0);
            let copy = latest_sample.map(|s| s.copy_ms).unwrap_or(0.0);
            let waitable = latest_sample.map(|s| s.present_waitable_ms).unwrap_or(0.0);
            let present = latest_sample.map(|s| s.present_call_ms).unwrap_or(0.0);
            let source = latest_sample.map(|s| s.source_delta_ms).unwrap_or(0.0);
            let footer = format!(
                "dt {:>4.1}  total {:>4.1}  copy {:>4.1}  wait {:>4.1}  present {:>4.1}  src {:>4.1}",
                interval, total, copy, waitable, present, source
            );
            painter.text(
                panel_rect.min + egui::vec2(10.0, 137.0),
                egui::Align2::LEFT_TOP,
                footer,
                egui::FontId::monospace(11.0),
                egui::Color32::from_rgb(212, 216, 224),
            );
        });
}

pub(super) fn draw_native_jump_panel(
    ctx: &egui::Context,
    overlay_height_points: f32,
    position_secs: f64,
    entries: &[NativeOverlayJumpEntry],
    jump_texture_ids: &HashMap<usize, egui::TextureId>,
    bookmark_title_edit: &mut Option<NativeBookmarkTitleEdit>,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    let panel_rect = native_jump_panel_rect(overlay_height_points);

    egui::Area::new(egui::Id::new("native_video_jump_panel"))
        .order(egui::Order::Foreground)
        .fixed_pos(panel_rect.min)
        .show(ctx, |ui| {
            ui.set_min_size(panel_rect.size());
            let rect = ui.min_rect();
            let painter = ui.painter().clone();
            painter.rect_filled(
                rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(14, 14, 18, 232),
            );
            painter.line_segment(
                [rect.right_top(), rect.right_bottom()],
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 55),
                ),
            );
            let _ = ui.interact(
                rect,
                egui::Id::new("native_video_jump_panel_bg"),
                egui::Sense::click(),
            );
            painter.text(
                rect.min + egui::vec2(10.0, 10.0),
                egui::Align2::LEFT_TOP,
                "ジャンプ",
                egui::FontId::proportional(13.0),
                egui::Color32::from_rgb(238, 238, 238),
            );

            let pin_rect = egui::Rect::from_min_size(
                rect.min + egui::vec2(rect.width() - 68.0, 6.0),
                egui::vec2(26.0, 24.0),
            );
            let pin_resp = ui.interact(
                pin_rect,
                egui::Id::new("native_jump_pin_here"),
                egui::Sense::click(),
            );
            draw_overlay_button_bg(&painter, pin_rect, pin_resp.hovered(), false);
            draw_overlay_pin_icon(
                &painter,
                pin_rect.center(),
                7.0,
                egui::Color32::from_rgb(140, 245, 170),
            );
            let pin_resp = pin_resp.on_hover_text("現在位置をピン留め");
            if pin_resp.clicked() {
                commands.push(NativeOverlayCommand::SetPinAt {
                    target_secs: position_secs,
                });
            }

            let bm_rect = egui::Rect::from_min_size(
                rect.min + egui::vec2(rect.width() - 36.0, 6.0),
                egui::vec2(26.0, 24.0),
            );
            let bm_resp = ui.interact(
                bm_rect,
                egui::Id::new("native_jump_bookmark_here"),
                egui::Sense::click(),
            );
            draw_overlay_button_bg(&painter, bm_rect, bm_resp.hovered(), false);
            draw_overlay_bookmark_icon(
                &painter,
                bm_rect.center(),
                7.0,
                egui::Color32::from_rgb(255, 220, 82),
            );
            let bm_resp = bm_resp.on_hover_text("現在位置をブックマーク [B]");
            if bm_resp.clicked() {
                commands.push(NativeOverlayCommand::AddBookmarkAt {
                    target_secs: position_secs,
                });
            }

            let content_rect = egui::Rect::from_min_max(rect.min + egui::vec2(0.0, 34.0), rect.max);
            let mut content_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(content_rect)
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
            );
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .max_height(content_rect.height())
                .show(&mut content_ui, |ui| {
                    ui.add_space(6.0);
                    if entries.is_empty() {
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            ui.colored_label(
                                egui::Color32::from_gray(170),
                                "ピン・ブックマーク・チャプターはまだありません",
                            );
                        });
                        return;
                    }

                    for kind in [
                        NativeOverlayTimelineMarkerKind::Pin,
                        NativeOverlayTimelineMarkerKind::Bookmark,
                        NativeOverlayTimelineMarkerKind::Chapter,
                    ] {
                        let section_entries: Vec<_> = entries
                            .iter()
                            .enumerate()
                            .filter(|(_, entry)| entry.kind == kind)
                            .collect();
                        if section_entries.is_empty() {
                            continue;
                        }
                        let (label, color) = match kind {
                            NativeOverlayTimelineMarkerKind::Pin => {
                                ("ピン留め", egui::Color32::from_rgb(140, 245, 170))
                            }
                            NativeOverlayTimelineMarkerKind::Bookmark => {
                                ("ブックマーク", egui::Color32::from_rgb(255, 220, 82))
                            }
                            NativeOverlayTimelineMarkerKind::Chapter => {
                                ("チャプター", egui::Color32::from_rgb(115, 210, 255))
                            }
                        };
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            ui.colored_label(color, egui::RichText::new(label).size(12.0));
                        });
                        ui.add_space(3.0);
                        for (idx, entry) in section_entries {
                            let time_text = format_native_jump_entry_time(entry, entries);
                            draw_native_jump_row(
                                ui,
                                idx,
                                entry,
                                &time_text,
                                jump_texture_ids,
                                bookmark_title_edit,
                                commands,
                            );
                        }
                        ui.add_space(8.0);
                    }
                });
        });
}

pub(super) fn draw_native_jump_row(
    ui: &mut egui::Ui,
    idx: usize,
    entry: &NativeOverlayJumpEntry,
    time_text: &str,
    jump_texture_ids: &HashMap<usize, egui::TextureId>,
    bookmark_title_edit: &mut Option<NativeBookmarkTitleEdit>,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    let row_h = 76.0;
    let row_w = (ui.available_width() - 12.0).max(260.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let (row_rect, resp) =
            ui.allocate_exact_size(egui::vec2(row_w, row_h), egui::Sense::click());
        let painter = ui.painter().clone();
        if resp.hovered() {
            painter.rect_filled(
                row_rect,
                4.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 22),
            );
        }
        let thumb_rect =
            egui::Rect::from_min_size(row_rect.min + egui::vec2(6.0, 4.0), egui::vec2(120.0, 68.0));
        painter.rect_filled(thumb_rect, 3.0, egui::Color32::from_rgb(30, 30, 35));
        if let Some(texture_id) = jump_texture_ids.get(&idx) {
            painter.image(
                *texture_id,
                thumb_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            painter.text(
                thumb_rect.center(),
                egui::Align2::CENTER_CENTER,
                "...",
                egui::FontId::proportional(14.0),
                egui::Color32::from_gray(140),
            );
        }
        painter.rect_stroke(
            thumb_rect,
            3.0,
            egui::Stroke::new(1.0, egui::Color32::from_gray(72)),
            egui::StrokeKind::Inside,
        );

        let text_x = thumb_rect.max.x + 10.0;
        let (kind_label, kind_color) = match entry.kind {
            NativeOverlayTimelineMarkerKind::Pin => ("PIN", egui::Color32::from_rgb(140, 245, 170)),
            NativeOverlayTimelineMarkerKind::Bookmark => {
                ("BM", egui::Color32::from_rgb(255, 220, 82))
            }
            NativeOverlayTimelineMarkerKind::Chapter => {
                ("CH", egui::Color32::from_rgb(115, 210, 255))
            }
        };
        painter.text(
            egui::pos2(text_x, row_rect.min.y + 14.0),
            egui::Align2::LEFT_CENTER,
            kind_label,
            egui::FontId::monospace(11.0),
            kind_color,
        );
        painter.text(
            egui::pos2(text_x + 36.0, row_rect.min.y + 14.0),
            egui::Align2::LEFT_CENTER,
            time_text,
            egui::FontId::proportional(12.0),
            egui::Color32::from_rgb(232, 232, 232),
        );
        if let Some(title) = entry.title.as_deref().filter(|s| !s.trim().is_empty()) {
            let title_max_chars = if entry.bookmark_id.is_some() { 16 } else { 22 };
            painter.text(
                egui::pos2(text_x, row_rect.min.y + 38.0),
                egui::Align2::LEFT_TOP,
                truncate_overlay_text(title, title_max_chars),
                egui::FontId::proportional(12.0),
                egui::Color32::from_rgb(205, 205, 205),
            );
        }

        let mut delete_clicked = false;
        let mut edit_clicked = false;
        if let Some(id) = entry.bookmark_id {
            let edit_rect = egui::Rect::from_min_size(
                egui::pos2(row_rect.max.x - 56.0, row_rect.min.y + 8.0),
                egui::vec2(22.0, 22.0),
            );
            let edit_resp = ui.interact(
                edit_rect,
                egui::Id::new(("native_jump_edit", id)),
                egui::Sense::click(),
            );
            draw_overlay_button_bg(&painter, edit_rect, edit_resp.hovered(), false);
            draw_overlay_pencil_icon(
                &painter,
                edit_rect,
                if edit_resp.hovered() {
                    egui::Color32::from_rgb(255, 235, 160)
                } else {
                    egui::Color32::from_rgb(225, 210, 150)
                },
            );
            let edit_resp = edit_resp.on_hover_text("ブックマーク名を編集");
            if edit_resp.clicked() {
                edit_clicked = true;
                *bookmark_title_edit = Some(NativeBookmarkTitleEdit {
                    id,
                    title: entry.title.clone().unwrap_or_default(),
                    request_focus: true,
                });
            }

            let delete_rect = egui::Rect::from_min_size(
                egui::pos2(row_rect.max.x - 28.0, row_rect.min.y + 8.0),
                egui::vec2(22.0, 22.0),
            );
            let delete_resp = ui.interact(
                delete_rect,
                egui::Id::new(("native_jump_delete", id)),
                egui::Sense::click(),
            );
            draw_overlay_button_bg(&painter, delete_rect, delete_resp.hovered(), false);
            painter.text(
                delete_rect.center(),
                egui::Align2::CENTER_CENTER,
                "X",
                egui::FontId::monospace(12.0),
                egui::Color32::from_rgb(240, 190, 190),
            );
            let delete_resp = delete_resp.on_hover_text("ブックマークを削除");
            if delete_resp.clicked() {
                delete_clicked = true;
                commands.push(NativeOverlayCommand::DeleteBookmark { id });
            }
        } else if entry.kind == NativeOverlayTimelineMarkerKind::Pin {
            let delete_rect = egui::Rect::from_min_size(
                egui::pos2(row_rect.max.x - 28.0, row_rect.min.y + 8.0),
                egui::vec2(22.0, 22.0),
            );
            let delete_resp = ui.interact(
                delete_rect,
                egui::Id::new("native_jump_delete_pin"),
                egui::Sense::click(),
            );
            draw_overlay_button_bg(&painter, delete_rect, delete_resp.hovered(), false);
            painter.text(
                delete_rect.center(),
                egui::Align2::CENTER_CENTER,
                "X",
                egui::FontId::monospace(12.0),
                egui::Color32::from_rgb(240, 190, 190),
            );
            let delete_resp = delete_resp.on_hover_text("ピン留めを解除");
            if delete_resp.clicked() {
                delete_clicked = true;
                commands.push(NativeOverlayCommand::DeletePin);
            }
        }

        if resp.clicked() && !delete_clicked && !edit_clicked {
            commands.push(NativeOverlayCommand::Seek {
                target_secs: entry.pts_secs,
            });
        }
    });
}

/// 戻り値は実際に描画したダイアログ rect。呼び出し側は `SetWindowRgn` の region に
/// この rect を使う (中央固定の概算 region だとダイアログ上端がクリップされるため)。
pub(super) fn draw_native_bookmark_title_editor(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    edit: &mut Option<NativeBookmarkTitleEdit>,
    commands: &mut Vec<NativeOverlayCommand>,
) -> Option<egui::Rect> {
    let Some(state) = edit.as_mut() else {
        return None;
    };
    let dialog_w = 360.0_f32.min((overlay_width_points - 32.0).max(260.0));
    let dialog_h = 142.0;
    let pos = egui::pos2(
        (overlay_width_points - dialog_w) * 0.5,
        (overlay_height_points - dialog_h) * 0.5,
    );
    let mut save = false;
    let mut clear = false;
    let mut cancel = false;

    let area_response = egui::Area::new(egui::Id::new("native_video_bookmark_title_editor"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(18, 18, 24, 244))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(112)))
                .corner_radius(egui::CornerRadius::same(5))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_min_width(dialog_w - 24.0);
                    ui.label(
                        egui::RichText::new("ブックマーク名")
                            .size(14.0)
                            .color(egui::Color32::from_rgb(238, 238, 238)),
                    );
                    ui.add_space(6.0);
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut state.title)
                            .desired_width(dialog_w - 24.0)
                            .hint_text("未設定"),
                    );
                    if state.request_focus {
                        response.request_focus();
                        state.request_focus = false;
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("保存").clicked() {
                            save = true;
                        }
                        if ui.button("名称なし").clicked() {
                            clear = true;
                        }
                        if ui.button("キャンセル").clicked() {
                            cancel = true;
                        }
                    });
                });
        });

    if save || clear {
        let state = edit.take().expect("bookmark title edit exists");
        commands.push(NativeOverlayCommand::SetBookmarkTitle {
            id: state.id,
            title: if clear { String::new() } else { state.title },
        });
    } else if cancel {
        *edit = None;
    }
    Some(area_response.response.rect)
}

#[derive(Copy, Clone)]
pub(super) enum NativeTopButtonGlyph {
    TileGrid,
    TileColumnsLess,
    TileColumnsMore,
    PerfGraph,
    Vst3,
    Close,
}

pub(super) fn draw_native_top_button(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    x: &mut f32,
    y: f32,
    width: f32,
    height: f32,
    gap: f32,
    id: &'static str,
    glyph: NativeTopButtonGlyph,
    active: bool,
    tooltip: &str,
    command: NativeOverlayCommand,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    let rect = egui::Rect::from_min_size(egui::pos2(*x, y), egui::vec2(width, height));
    let resp = ui.interact(rect, egui::Id::new(id), egui::Sense::click());
    draw_overlay_button_bg(painter, rect, resp.hovered(), active);
    match glyph {
        NativeTopButtonGlyph::TileGrid => draw_overlay_tile_grid_icon(painter, rect),
        NativeTopButtonGlyph::TileColumnsLess => draw_overlay_grid_density_icon(painter, rect, 3),
        NativeTopButtonGlyph::TileColumnsMore => draw_overlay_grid_density_icon(painter, rect, 5),
        NativeTopButtonGlyph::PerfGraph => draw_overlay_perf_graph_icon(painter, rect),
        NativeTopButtonGlyph::Vst3 => draw_overlay_vst3_top_icon(painter, rect),
        NativeTopButtonGlyph::Close => draw_overlay_close_icon(painter, rect),
    }
    let resp = resp.on_hover_text(tooltip);
    if resp.clicked() {
        commands.push(command);
    }
    *x -= width + gap;
}

pub(super) fn draw_native_frame_step_button(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    id: &'static str,
    direction: i32,
    tooltip: &str,
    hold: &mut Option<NativeFrameStepHold>,
    commands: &mut Vec<NativeOverlayCommand>,
) -> bool {
    let resp = ui.interact(rect, egui::Id::new(id), egui::Sense::click());
    draw_overlay_button_bg(painter, rect, resp.hovered(), false);
    draw_overlay_frame_step_icon(painter, rect, direction);
    let resp = resp.on_hover_text(tooltip);
    let primary_down = ui.ctx().input(|i| i.pointer.primary_down());
    let held_from_this_button = hold
        .as_ref()
        .is_some_and(|state| state.direction == direction);
    let down = resp.is_pointer_button_down_on() || (primary_down && held_from_this_button);
    let now = Instant::now();
    if down {
        match hold {
            Some(state) if state.direction == direction => {
                if now.saturating_duration_since(state.last_step_at) >= Duration::from_millis(100) {
                    state.last_step_at = now;
                    commands.push(NativeOverlayCommand::FrameStep { direction });
                }
            }
            _ => {
                *hold = Some(NativeFrameStepHold {
                    direction,
                    last_step_at: now,
                });
                commands.push(NativeOverlayCommand::FrameStep { direction });
            }
        }
    }
    down
}

pub(super) fn draw_overlay_frame_step_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    direction: i32,
) {
    let color = egui::Color32::from_rgb(238, 238, 238);
    let stroke = egui::Stroke::new(1.8, color);
    let c = rect.center();
    let sign = if direction < 0 { -1.0 } else { 1.0 };
    let bar_outer_x = c.x - sign * 7.0;
    let bar_inner_x = c.x - sign * 3.0;
    painter.line_segment(
        [
            egui::pos2(bar_outer_x, c.y - 8.0),
            egui::pos2(bar_outer_x, c.y + 8.0),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(bar_inner_x, c.y - 8.0),
            egui::pos2(bar_inner_x, c.y + 8.0),
        ],
        stroke,
    );
    let tip = egui::pos2(c.x + sign * 7.0, c.y);
    let back_x = c.x;
    painter.add(egui::Shape::convex_polygon(
        vec![
            tip,
            egui::pos2(back_x, c.y - 8.0),
            egui::pos2(back_x, c.y + 8.0),
        ],
        color,
        egui::Stroke::NONE,
    ));
}

pub(super) fn draw_overlay_camera_icon(painter: &egui::Painter, rect: egui::Rect) {
    let color = egui::Color32::from_rgb(238, 238, 238);
    let stroke = egui::Stroke::new(1.6, color);
    let body =
        egui::Rect::from_center_size(rect.center() + egui::vec2(0.0, 2.0), egui::vec2(18.0, 13.0));
    painter.rect_stroke(body, 2.0, stroke, egui::StrokeKind::Inside);
    let hump = egui::Rect::from_min_size(
        egui::pos2(body.min.x + 3.0, body.min.y - 4.0),
        egui::vec2(7.0, 4.0),
    );
    painter.rect_filled(hump, 1.2, color);
    painter.circle_stroke(body.center(), 4.1, stroke);
    painter.circle_filled(body.center(), 1.7, color);
}

pub(super) fn draw_overlay_tile_grid_icon(painter: &egui::Painter, rect: egui::Rect) {
    let cell = 7.0;
    let gap = 3.0;
    let total = cell * 2.0 + gap;
    let start = rect.center() - egui::vec2(total * 0.5, total * 0.5);
    for row in 0..2 {
        for col in 0..2 {
            let min = start + egui::vec2((cell + gap) * col as f32, (cell + gap) * row as f32);
            painter.rect_filled(
                egui::Rect::from_min_size(min, egui::vec2(cell, cell)),
                1.5,
                egui::Color32::from_rgb(238, 238, 238),
            );
        }
    }
}

pub(super) fn draw_overlay_grid_density_icon(painter: &egui::Painter, rect: egui::Rect, n: usize) {
    let n = n.max(2) as f32;
    let inner = 17.0_f32;
    let gap = if n >= 5.0 { 1.2 } else { 2.0 };
    let cell = ((inner - gap * (n - 1.0)) / n).max(1.0);
    let total = cell * n + gap * (n - 1.0);
    let start = rect.center() - egui::vec2(total * 0.5, total * 0.5);
    let rounding = if cell >= 4.0 { 1.0 } else { 0.5 };
    for row in 0..(n as usize) {
        for col in 0..(n as usize) {
            let min = start + egui::vec2((cell + gap) * col as f32, (cell + gap) * row as f32);
            painter.rect_filled(
                egui::Rect::from_min_size(min, egui::vec2(cell, cell)),
                rounding,
                egui::Color32::from_rgb(238, 238, 238),
            );
        }
    }
}

pub(super) fn draw_overlay_perf_graph_icon(painter: &egui::Painter, rect: egui::Rect) {
    let left = rect.min.x + 6.0;
    let right = rect.max.x - 5.0;
    let top = rect.min.y + 7.0;
    let bottom = rect.max.y - 6.0;
    painter.line_segment(
        [egui::pos2(left, bottom), egui::pos2(right, bottom)],
        egui::Stroke::new(1.0, egui::Color32::from_gray(140)),
    );
    let points = [
        egui::pos2(left, bottom - 3.0),
        egui::pos2(left + 5.0, bottom - 9.0),
        egui::pos2(left + 10.0, bottom - 5.0),
        egui::pos2(left + 15.0, top + 2.0),
        egui::pos2(right, bottom - 12.0),
    ];
    painter.add(egui::Shape::line(
        points.to_vec(),
        egui::Stroke::new(1.7, egui::Color32::from_rgb(170, 230, 255)),
    ));
}

pub(super) fn draw_overlay_vst3_top_icon(painter: &egui::Painter, rect: egui::Rect) {
    // 3 文字を等幅・gap 固定で並べることで proportional font 風の重なりを回避する。
    // 旧実装は V/S/T それぞれ独立座標だったため stroke 込みで S と T が重なっていた。
    let color = egui::Color32::from_rgb(238, 238, 238);
    let stroke = egui::Stroke::new(1.5, color);
    let c = rect.center();
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
    for [a, b] in [
        [egui::pos2(s_x1, top), egui::pos2(s_x0, top)],
        [egui::pos2(s_x0, top), egui::pos2(s_x0, mid)],
        [egui::pos2(s_x0, mid), egui::pos2(s_x1, mid)],
        [egui::pos2(s_x1, mid), egui::pos2(s_x1, bot)],
        [egui::pos2(s_x1, bot), egui::pos2(s_x0, bot)],
    ] {
        painter.line_segment([a, b], stroke);
    }

    // T
    let t_x0 = s_x1 + gap;
    let t_x1 = t_x0 + char_w;
    let t_xc = (t_x0 + t_x1) * 0.5;
    painter.line_segment([egui::pos2(t_x0, top), egui::pos2(t_x1, top)], stroke);
    painter.line_segment([egui::pos2(t_xc, top), egui::pos2(t_xc, bot)], stroke);
}

pub(super) fn draw_overlay_close_icon(painter: &egui::Painter, rect: egui::Rect) {
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.26;
    let stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(242, 242, 242));
    painter.line_segment([c + egui::vec2(-r, -r), c + egui::vec2(r, r)], stroke);
    painter.line_segment([c + egui::vec2(r, -r), c + egui::vec2(-r, r)], stroke);
}

pub(super) fn draw_overlay_vst3_gui_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    hovered: bool,
    visible: bool,
    enabled: bool,
) {
    draw_overlay_button_bg(painter, rect, hovered, visible);
    let color = if !enabled {
        egui::Color32::from_gray(84)
    } else if visible {
        egui::Color32::from_rgb(245, 132, 28)
    } else {
        egui::Color32::from_gray(132)
    };
    let fill = if visible {
        egui::Color32::from_rgba_unmultiplied(245, 132, 28, 34)
    } else {
        egui::Color32::from_rgba_unmultiplied(132, 132, 132, 20)
    };
    let icon_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(15.0, 12.0));
    painter.rect_filled(icon_rect, 2.0, fill);
    painter.rect_stroke(
        icon_rect,
        2.0,
        egui::Stroke::new(1.8, color),
        egui::StrokeKind::Inside,
    );
}

pub(super) fn native_checkmark_rect(overlay_width_points: f32, top: f32) -> egui::Rect {
    let radius = 18.0;
    let center = egui::pos2((overlay_width_points - 30.0).max(radius), top + radius);
    egui::Rect::from_center_size(center, egui::vec2(radius * 2.2, radius * 2.2))
}

pub(super) fn draw_native_checkmark(ctx: &egui::Context, overlay_width_points: f32, top: f32) {
    egui::Area::new(egui::Id::new("native_video_checkmark"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let radius = 18.0;
            let rect = native_checkmark_rect(overlay_width_points, top);
            let center = rect.center();
            ui.allocate_rect(rect, egui::Sense::hover());
            let painter = ui.painter();
            painter.circle_filled(
                center,
                radius,
                egui::Color32::from_rgba_unmultiplied(22, 154, 84, 226),
            );
            painter.circle_stroke(
                center,
                radius,
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 90),
                ),
            );
            painter.line_segment(
                [
                    center + egui::vec2(-7.0, 0.5),
                    center + egui::vec2(-2.0, 6.0),
                ],
                egui::Stroke::new(3.0, egui::Color32::WHITE),
            );
            painter.line_segment(
                [
                    center + egui::vec2(-2.0, 6.0),
                    center + egui::vec2(8.0, -7.0),
                ],
                egui::Stroke::new(3.0, egui::Color32::WHITE),
            );
        });
}

/// `draw_native_center_status` で描画する box の論理 rect を返す共通 helper (T28)。
///
/// region 計算 (HUD HWND `SetWindowRgn` 用) と描画関数 (`draw_native_center_status`) の
/// 両方が同じ式で box サイズを決める必要があり、これまでは 2 箇所重複していて
/// 片方だけ変えると HUD region < 描画 box でテキスト端が clip されていた
/// (Codex P2 / T28、2026-05-16)。
pub(super) fn native_center_status_rect(
    overlay_width_points: f32,
    overlay_height_points: f32,
    title: &str,
    has_body: bool,
) -> egui::Rect {
    let available_w = (overlay_width_points - 48.0).max(120.0);
    let box_w = if has_body {
        overlay_width_points.clamp(360.0, 720.0).min(available_w)
    } else {
        let text_w = title.chars().count() as f32 * 18.0 + 72.0;
        text_w.clamp(180.0, 420.0).min(available_w)
    };
    let box_h = if has_body { 132.0 } else { 62.0 };
    egui::Rect::from_center_size(
        egui::pos2(overlay_width_points * 0.5, overlay_height_points * 0.5),
        egui::vec2(box_w, box_h),
    )
}

pub(super) fn draw_native_center_status(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    title: &str,
    body: Option<&str>,
    is_error: bool,
) {
    egui::Area::new(egui::Id::new("native_video_center_status"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let full_rect = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(overlay_width_points, overlay_height_points),
            );
            ui.set_min_size(full_rect.size());
            let painter = ui.painter();
            let rect = native_center_status_rect(
                overlay_width_points,
                overlay_height_points,
                title,
                body.is_some(),
            );
            painter.rect_filled(
                rect,
                8.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 214),
            );
            let title_color = if is_error {
                egui::Color32::from_rgb(255, 120, 120)
            } else {
                egui::Color32::from_rgb(238, 238, 238)
            };
            painter.text(
                if body.is_some() {
                    egui::pos2(rect.center().x, rect.min.y + 26.0)
                } else {
                    rect.center()
                },
                egui::Align2::CENTER_CENTER,
                title,
                egui::FontId::proportional(if body.is_some() { 22.0 } else { 20.0 }),
                title_color,
            );
            if let Some(body) = body {
                let body_rect = egui::Rect::from_min_max(
                    rect.min + egui::vec2(22.0, 52.0),
                    rect.max - egui::vec2(22.0, 14.0),
                );
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(body_rect)
                        .layout(egui::Layout::top_down(egui::Align::Center)),
                );
                child.add(
                    egui::Label::new(
                        egui::RichText::new(body)
                            .size(14.0)
                            .color(egui::Color32::from_gray(230)),
                    )
                    .wrap(),
                );
            }
        });
}

pub(super) fn draw_native_center_pause_controls(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    excluded_panel_rects: &[egui::Rect],
    commands: &mut Vec<NativeOverlayCommand>,
) -> Option<[egui::Rect; 3]> {
    let mut drawn: Option<[egui::Rect; 3]> = None;
    egui::Area::new(egui::Id::new("native_video_center_pause_controls"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        // ⚠️ fade_in(false): egui::Area は新規可視時に animation_time をかけて
        // opacity を 0→1 に animate するが、本 overlay は paused 中の自動 tick
        // を持たない (wants_periodic_tick に paused_center_visible は含まれない)
        // ため、最初のレンダー後に追加フレームが流れず alpha が 1.0 に到達しない。
        // ユーザーには「半透明のまま」に見えてマウス移動で初めて正しい濃さに
        // なる挙動になる。要件 = 瞬時に最終濃度で出す、なので fade を無効化する。
        .fade_in(false)
        .show(ctx, |ui| {
            let full_rect = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(overlay_width_points, overlay_height_points),
            );
            ui.set_min_size(full_rect.size());
            let painter = ui.painter().clone();
            let radius = 56.0;
            let gap = 34.0;
            let center_y = full_rect.center().y;
            let replay_center = egui::pos2(full_rect.center().x - radius - gap * 0.5, center_y);
            let play_center = egui::pos2(full_rect.center().x + radius + gap * 0.5, center_y);

            let replay_rect =
                egui::Rect::from_center_size(replay_center, egui::vec2(radius * 2.0, radius * 2.0));
            let play_rect =
                egui::Rect::from_center_size(play_center, egui::vec2(radius * 2.0, radius * 2.0));

            let replay_resp = ui
                .interact(
                    replay_rect,
                    egui::Id::new("native_center_replay"),
                    egui::Sense::click(),
                )
                .on_hover_text("最初から再生 [W]");
            let play_resp = ui
                .interact(
                    play_rect,
                    egui::Id::new("native_center_play"),
                    egui::Sense::click(),
                )
                .on_hover_text("続きから再生 [Enter]");

            for (rect, hovered) in [
                (replay_rect, replay_resp.hovered()),
                (play_rect, play_resp.hovered()),
            ] {
                painter.circle_filled(
                    rect.center(),
                    radius,
                    if hovered {
                        egui::Color32::from_rgba_unmultiplied(40, 40, 46, 238)
                    } else {
                        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 214)
                    },
                );
                painter.circle_stroke(
                    rect.center(),
                    radius,
                    egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 70),
                    ),
                );
            }
            draw_overlay_replay_icon(&painter, replay_center, 22.0);
            draw_overlay_play_icon(&painter, play_center, 24.0);

            let label_font = egui::FontId::proportional(16.0);
            let hint_font = egui::FontId::proportional(13.0);
            let label_color = egui::Color32::WHITE;
            let hint_color = egui::Color32::from_gray(220);
            let replay_label =
                painter.layout_no_wrap("最初から".to_owned(), label_font.clone(), label_color);
            let play_label = painter.layout_no_wrap("続きから".to_owned(), label_font, label_color);
            let hint_label = painter.layout_no_wrap(
                "Enter: 再生 / W: 頭出し / ←→: シーク / J,K: マーカー移動".to_owned(),
                hint_font,
                hint_color,
            );
            let label_y = center_y + radius + 22.0;
            let hint_y = center_y + radius + 52.0;
            let replay_label_pos = egui::pos2(
                replay_center.x - replay_label.size().x * 0.5,
                label_y - replay_label.size().y * 0.5,
            );
            let play_label_pos = egui::pos2(
                play_center.x - play_label.size().x * 0.5,
                label_y - play_label.size().y * 0.5,
            );
            let hint_label_pos = egui::pos2(
                full_rect.center().x - hint_label.size().x * 0.5,
                hint_y - hint_label.size().y * 0.5,
            );
            let text_min = egui::pos2(
                replay_label_pos
                    .x
                    .min(play_label_pos.x)
                    .min(hint_label_pos.x),
                replay_label_pos
                    .y
                    .min(play_label_pos.y)
                    .min(hint_label_pos.y),
            );
            let text_max = egui::pos2(
                (replay_label_pos.x + replay_label.size().x)
                    .max(play_label_pos.x + play_label.size().x)
                    .max(hint_label_pos.x + hint_label.size().x),
                (replay_label_pos.y + replay_label.size().y)
                    .max(play_label_pos.y + play_label.size().y)
                    .max(hint_label_pos.y + hint_label.size().y),
            );
            let backdrop_rect =
                egui::Rect::from_min_max(text_min, text_max).expand2(egui::vec2(16.0, 9.0));
            painter.rect_filled(
                backdrop_rect,
                6.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 178),
            );
            painter.rect_stroke(
                backdrop_rect,
                6.0,
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 42),
                ),
                egui::StrokeKind::Outside,
            );
            painter.galley(replay_label_pos, replay_label, label_color);
            painter.galley(play_label_pos, play_label, label_color);
            painter.galley(hint_label_pos, hint_label, hint_color);

            // compute_hud_regions が SetWindowRgn に渡す「実描画 rect」を記録する。
            // 200×200pt 固定 region では両ボタンの外側 ~29pt とヒント帯が HUD HWND の
            // region 外に出てしまいクリップされていた。replay/play ボタン円形 rect と
            // backdrop rect をそれぞれ 4pt 膨らませて返し、`compute_hud_regions` が
            // 個別 RECT として SetWindowRgn 集合に push する (union しないことで、
            // ヒント横幅に引っ張られた不要な HUD 入力帯を作らない)。
            drawn = Some([
                replay_rect.expand(4.0),
                play_rect.expand(4.0),
                backdrop_rect.expand(4.0),
            ]);

            if replay_resp.clicked() {
                commands.push(NativeOverlayCommand::SeekToStartAndPlay);
            }
            if play_resp.clicked() {
                commands.push(NativeOverlayCommand::TogglePlay);
            }

            // 一時停止中の中央パネル外クリック処理。
            //
            // 設計上の問題: paused_center_visible の Area は set_min_size で
            // 全画面を占有する。egui::Context::wants_pointer_input は
            // `is_pointer_over_area() && !any_down` でも true になるため、
            // mouse UP イベント (= any_down=false) は常に true になり
            // should_forward_to_ui が false を返す → UI 側の
            // handle_native_video_mouse_button まで届かない。結果、playing 中の
            // クリック (= UI 側で処理されるパス) では成立する toggle/close が
            // paused 中だと完全に死ぬ。
            //
            // 対策: overlay 側で raw `*_clicked()` を見て、UI 側で発行されるはず
            // だった command を直接 emit する。primary は TogglePlay (= 再開)、
            // secondary は CloseFullscreen (= 右クリックで閉じる) に対応させて
            // playing 中と同じ操作感を保つ。
            //
            // 除外領域 (= 「中央パネル外でない」と判定する位置):
            // - 左右ボタン rect: 上の replay_resp/play_resp.clicked() で処理済
            // - backdrop_rect: ラベル「最初から/続きから」の黒背景 (Codex P3)
            // - excluded_panel_rects: 呼び出し元で「実際に描画中の」パネル rect
            //   (top bar / seek HUD / 左 jump / 右 metadata / VST3) を集めたもの。
            //   ホバー判定領域より狭く、不可視パネルを誤って除外しない。
            // 右クリック (close) はボタン上で押されても close 扱いで OK なので、
            // 中央ボタン rect は除外しない。
            let pos_opt = ctx.input(|i| i.pointer.interact_pos());
            if let Some(pos) = pos_opt {
                let in_visible_panel = excluded_panel_rects.iter().any(|r| r.contains(pos));
                if !in_visible_panel {
                    let on_center_button = replay_rect.contains(pos) || play_rect.contains(pos);
                    let on_label_backdrop = backdrop_rect.contains(pos);
                    if ctx.input(|i| i.pointer.primary_clicked())
                        && !on_center_button
                        && !on_label_backdrop
                    {
                        commands.push(NativeOverlayCommand::TogglePlay);
                    }
                    if ctx.input(|i| i.pointer.secondary_clicked()) {
                        commands.push(NativeOverlayCommand::CloseFullscreen);
                    }
                }
            }
        });
    drawn
}

pub(super) fn draw_native_toast(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    toast: &NativeOverlayToast,
) -> Option<egui::Rect> {
    let elapsed = toast.started_at.elapsed().as_secs_f32();
    let duration = if toast.centered { 2.5 } else { 1.8 };
    let alpha = if elapsed > duration - 0.35 {
        ((duration - elapsed) / 0.35).clamp(0.0, 1.0)
    } else {
        1.0
    };
    if alpha <= 0.0 {
        return None;
    }
    let mut drawn_rect = None;
    // interactable(false) でこの Area がクリックイベントを奪わないようにする。
    // 旧: set_min_size で画面全体を Area として確保していたため、トースト表示中は
    //     overlay の他のボタン (ループ / 再生 / etc.) クリックがこの Area に消費されていた。
    egui::Area::new(egui::Id::new("native_video_toast"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .interactable(false)
        .show(ctx, |ui| {
            let full_rect = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(overlay_width_points, overlay_height_points),
            );
            ui.set_min_size(full_rect.size());
            let painter = ui.painter();
            let font = egui::FontId::proportional(if toast.centered { 24.0 } else { 16.0 });
            let galley =
                painter.layout_no_wrap(toast.text.clone(), font.clone(), egui::Color32::WHITE);
            let padding = if toast.centered {
                egui::vec2(28.0, 18.0)
            } else {
                egui::vec2(16.0, 10.0)
            };
            let max_w = (overlay_width_points - 40.0).max(160.0);
            let size = egui::vec2(
                (galley.size().x + padding.x * 2.0).min(max_w),
                galley.size().y + padding.y * 2.0,
            );
            let rect = if toast.centered {
                // 中央 (full_rect.center().y) はレジューム picker の二重円
                // (半径 56) と完全に被る。HUD top バー (上端 ~60px) と picker top
                // (center_y - 56) の間に収めるため、上から 30% の位置に置く。
                // 最小 80px は極端に低い窓 (= 30% が HUD バーに食い込む) 用の床。
                let centered_y =
                    (full_rect.min.y + full_rect.height() * 0.30).max(full_rect.min.y + 80.0);
                egui::Rect::from_center_size(egui::pos2(full_rect.center().x, centered_y), size)
            } else {
                egui::Rect::from_min_size(
                    egui::pos2(full_rect.max.x - size.x - 20.0, full_rect.min.y + 62.0),
                    size,
                )
            };
            drawn_rect = Some(rect.expand(4.0));
            painter.rect_filled(
                rect,
                8.0,
                egui::Color32::from_rgba_unmultiplied(24, 24, 28, (alpha * 224.0) as u8),
            );
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &toast.text,
                font,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, (alpha * 255.0) as u8),
            );
        });
    drawn_rect
}

/// 音量ノーマライズ スキャン中の進捗パネル (中央表示)。
/// プログレスバー + キャンセルボタン (× / ESC)。
pub(super) fn draw_native_normalize_progress(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    progress: &crate::video::normalize_types::NormalizeProgressSnapshot,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    egui::Area::new(egui::Id::new("native_video_normalize_progress"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let full_rect = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(overlay_width_points, overlay_height_points),
            );
            ui.set_min_size(full_rect.size());
            let painter = ui.painter().clone();
            // Codex 2周目 P2: 全画面 blocker — 進捗パネル外のクリック / ホバーが背面の
            // HUD / seek bar / volume slider 等に届かないようキャプチャする (= モーダル化)。
            // 半透明の暗幕も兼ねる (動画は見えるが UI 操作は止まる)。
            let _block = ui.interact(
                full_rect,
                egui::Id::new("native_video_normalize_blocker"),
                egui::Sense::CLICK | egui::Sense::HOVER,
            );
            painter.rect_filled(
                full_rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 96),
            );
            // 中央パネル
            let panel_w = 420.0_f32;
            let panel_h = 110.0_f32;
            let panel_rect =
                egui::Rect::from_center_size(full_rect.center(), egui::vec2(panel_w, panel_h));
            painter.rect_filled(
                panel_rect,
                10.0,
                egui::Color32::from_rgba_unmultiplied(20, 20, 24, 232),
            );
            // タイトル
            painter.text(
                egui::pos2(panel_rect.center().x, panel_rect.min.y + 22.0),
                egui::Align2::CENTER_CENTER,
                "音量ノーマライズ中…",
                egui::FontId::proportional(16.0),
                egui::Color32::from_rgb(238, 238, 238),
            );
            // プログレスバー or スピナー
            let bar_pad_x = 24.0;
            let bar_y = panel_rect.center().y + 6.0;
            let bar_rect = egui::Rect::from_min_max(
                egui::pos2(panel_rect.min.x + bar_pad_x, bar_y - 4.0),
                egui::pos2(panel_rect.max.x - bar_pad_x, bar_y + 4.0),
            );
            painter.rect_filled(bar_rect, 2.0, egui::Color32::from_gray(60));
            if progress.indeterminate || progress.duration_ms == 0 {
                // スピナー的に動くインジケータ (時間ベース)
                let t = ui.ctx().input(|i| i.time as f32);
                let frac = ((t * 0.7).fract() + 0.0).clamp(0.0, 1.0);
                let lo = (frac - 0.18).clamp(0.0, 1.0);
                let hi = (frac + 0.18).clamp(0.0, 1.0);
                let lo_x = bar_rect.min.x + bar_rect.width() * lo;
                let hi_x = bar_rect.min.x + bar_rect.width() * hi;
                let chunk = egui::Rect::from_min_max(
                    egui::pos2(lo_x, bar_rect.min.y),
                    egui::pos2(hi_x, bar_rect.max.y),
                );
                painter.rect_filled(chunk, 2.0, egui::Color32::from_rgb(255, 198, 62));
                ui.ctx().request_repaint();
            } else {
                let frac = (progress.pts_processed_ms as f32 / progress.duration_ms as f32)
                    .clamp(0.0, 1.0);
                let filled = egui::Rect::from_min_max(
                    bar_rect.min,
                    egui::pos2(bar_rect.min.x + bar_rect.width() * frac, bar_rect.max.y),
                );
                painter.rect_filled(filled, 2.0, egui::Color32::from_rgb(255, 198, 62));
                let pct_text = format!("{:.0}%", frac * 100.0);
                painter.text(
                    egui::pos2(panel_rect.center().x, bar_y + 18.0),
                    egui::Align2::CENTER_CENTER,
                    pct_text,
                    egui::FontId::proportional(12.0),
                    egui::Color32::from_gray(200),
                );
            }
            // キャンセルボタン (右下)
            let cancel_size = 24.0;
            let cancel_rect = egui::Rect::from_min_size(
                egui::pos2(panel_rect.max.x - cancel_size - 8.0, panel_rect.min.y + 8.0),
                egui::vec2(cancel_size, cancel_size),
            );
            let cancel_resp = ui.interact(
                cancel_rect,
                egui::Id::new("native_video_normalize_cancel"),
                egui::Sense::click(),
            );
            let cancel_color = if cancel_resp.hovered() {
                egui::Color32::from_rgb(255, 120, 120)
            } else {
                egui::Color32::from_gray(180)
            };
            painter.text(
                cancel_rect.center(),
                egui::Align2::CENTER_CENTER,
                "\u{00D7}", // U+00D7 multiplication sign (ANSI 安全、glyph lint 通過)
                egui::FontId::proportional(20.0),
                cancel_color,
            );
            let cancel_resp = cancel_resp.on_hover_text("キャンセル [ESC]");
            if cancel_resp.clicked() || ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
                commands.push(NativeOverlayCommand::CancelNormalizeScan);
            }
        });
}

pub(super) fn draw_top_bar_background(painter: &egui::Painter, overlay_width_points: f32) {
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(overlay_width_points, 54.0));
    painter.rect_filled(
        rect,
        0.0,
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 186),
    );
}

pub(super) fn draw_top_bar_text_lines(
    painter: &egui::Painter,
    title_text: &str,
    sub_text: &str,
    name_truncate: usize,
    sub_truncate: usize,
) {
    painter.text(
        egui::pos2(14.0, 20.0),
        egui::Align2::LEFT_CENTER,
        truncate_overlay_text(title_text, name_truncate),
        egui::FontId::proportional(15.0),
        egui::Color32::from_rgb(240, 240, 240),
    );
    painter.text(
        egui::pos2(14.0, 39.0),
        egui::Align2::LEFT_CENTER,
        truncate_overlay_text(sub_text, sub_truncate),
        egui::FontId::proportional(12.0),
        egui::Color32::from_rgb(190, 190, 190),
    );
}

pub(super) fn draw_native_top_bar(
    ctx: &egui::Context,
    overlay_width_points: f32,
    position_secs: f64,
    duration_secs: f64,
    metadata: Option<&NativeOverlayMetadata>,
    perf_visible: bool,
    vst3_available: bool,
    vst3_panel_visible: bool,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    egui::Area::new(egui::Id::new("native_video_top_bar"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let rect =
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(overlay_width_points, 54.0));
            ui.set_min_size(rect.size());
            let painter = ui.painter().clone();
            draw_top_bar_background(&painter, overlay_width_points);
            let name = metadata
                .and_then(|m| {
                    m.title
                        .as_ref()
                        .filter(|title| !title.trim().is_empty())
                        .or(Some(&m.file_name))
                })
                .map(String::as_str)
                .unwrap_or("video");
            let sub = if let Some(m) = metadata {
                format!(
                    "{}x{}  {}  {}  {}",
                    m.width,
                    m.height,
                    format_fps(m.avg_fps),
                    m.video_codec,
                    format_overlay_time(position_secs)
                )
            } else {
                format!(
                    "{} / {}",
                    format_overlay_time(position_secs),
                    format_overlay_time(duration_secs)
                )
            };
            draw_top_bar_text_lines(&painter, name, &sub, 88, 120);

            let btn_size = 28.0;
            let gap = 8.0;
            let mut x = overlay_width_points - 12.0 - btn_size;
            let y = 13.0;

            draw_native_top_button(
                ui,
                &painter,
                &mut x,
                y,
                btn_size,
                btn_size,
                gap,
                "native_top_close",
                NativeTopButtonGlyph::Close,
                false,
                "動画を終了",
                NativeOverlayCommand::CloseFullscreen,
                commands,
            );
            draw_native_top_button(
                ui,
                &painter,
                &mut x,
                y,
                btn_size,
                btn_size,
                gap,
                "native_top_tile",
                NativeTopButtonGlyph::TileGrid,
                false,
                "サムネイル一覧 [S]",
                NativeOverlayCommand::ToggleTileMode,
                commands,
            );
            draw_native_top_button(
                ui,
                &painter,
                &mut x,
                y,
                btn_size,
                btn_size,
                gap,
                "native_top_perf",
                NativeTopButtonGlyph::PerfGraph,
                perf_visible,
                "Perfグラフ [P]",
                NativeOverlayCommand::TogglePerfOverlay,
                commands,
            );
            if vst3_available {
                draw_native_top_button(
                    ui,
                    &painter,
                    &mut x,
                    y,
                    btn_size,
                    btn_size,
                    gap,
                    "native_top_vst3",
                    NativeTopButtonGlyph::Vst3,
                    vst3_panel_visible,
                    "VST3 パネル表示/非表示",
                    NativeOverlayCommand::ToggleVst3Gui,
                    commands,
                );
            }
        });
}

pub(super) fn draw_native_top_bar_tile(
    ctx: &egui::Context,
    overlay_width_points: f32,
    metadata: Option<&NativeOverlayMetadata>,
    tile_state: &NativeOverlayTileOverlay,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    egui::Area::new(egui::Id::new("native_video_tile_top_bar"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let rect =
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(overlay_width_points, 54.0));
            ui.set_min_size(rect.size());
            let painter = ui.painter().clone();
            draw_top_bar_background(&painter, overlay_width_points);

            // タイトル: title (空白除去) → file_name → fallback_file_name → "video"
            let fallback = tile_state.fallback_file_name.trim();
            let title_text: &str = metadata
                .and_then(|m| {
                    m.title
                        .as_ref()
                        .map(String::as_str)
                        .filter(|s| !s.trim().is_empty())
                        .or_else(|| {
                            let fname = m.file_name.as_str();
                            if fname.trim().is_empty() {
                                None
                            } else {
                                Some(fname)
                            }
                        })
                })
                .unwrap_or_else(|| {
                    if fallback.is_empty() {
                        "video"
                    } else {
                        fallback
                    }
                });

            // サブ行: 真空状態を最優先で「タイルを準備中...」、それ以外は metadata 有無で分岐
            let progress_total = tile_state.progress_total;
            let progress_done = tile_state.progress_done;
            let finished = tile_state.finished;
            let interval_secs = tile_state.interval_secs;
            let show_counter = progress_total > 0 && !finished;
            let counter_suffix = if show_counter {
                format!("  {progress_done}/{progress_total}")
            } else {
                String::new()
            };

            let sub_text = if let Some(open_status) = tile_state.video_open_status {
                crate::video::avio_progress::build_preparing_message(open_status)
            } else if interval_secs <= 0.0 && progress_total == 0 {
                String::from("タイルを準備中...")
            } else if let Some(m) = metadata {
                format!(
                    "{}x{}  {}  {}  {}  間隔 {}{}",
                    m.width,
                    m.height,
                    format_fps(m.avg_fps),
                    m.video_codec,
                    format_overlay_time(m.duration_secs),
                    format_tile_interval(interval_secs),
                    counter_suffix
                )
            } else {
                format!(
                    "間隔 {}{}",
                    format_tile_interval(interval_secs),
                    counter_suffix
                )
            };

            draw_top_bar_text_lines(&painter, title_text, &sub_text, 70, 95);

            let btn_size = 28.0;
            let gap = 8.0;
            let mut x = overlay_width_points - 12.0 - btn_size;
            let y = 13.0;

            draw_native_top_button(
                ui,
                &painter,
                &mut x,
                y,
                btn_size,
                btn_size,
                gap,
                "native_tile_top_close",
                NativeTopButtonGlyph::Close,
                false,
                "動画に戻る [S / Esc]",
                NativeOverlayCommand::ToggleTileMode,
                commands,
            );
            draw_native_top_button(
                ui,
                &painter,
                &mut x,
                y,
                btn_size,
                btn_size,
                gap,
                "native_tile_columns_more",
                NativeTopButtonGlyph::TileColumnsMore,
                false,
                "サムネ列数を増やす [Ctrl+ホイール下]",
                NativeOverlayCommand::TileColumnsDelta { delta: 1 },
                commands,
            );
            draw_native_top_button(
                ui,
                &painter,
                &mut x,
                y,
                btn_size,
                btn_size,
                gap,
                "native_tile_columns_less",
                NativeTopButtonGlyph::TileColumnsLess,
                false,
                "サムネ列数を減らす [Ctrl+ホイール上]",
                NativeOverlayCommand::TileColumnsDelta { delta: -1 },
                commands,
            );
        });
}

pub(super) fn draw_native_navigation_preview(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    preview: &NativeOverlayNavigationPreview,
    preview_texture_id: Option<egui::TextureId>,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    egui::Area::new(egui::Id::new("native_video_navigation_preview"))
        .order(egui::Order::Background)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let full_rect = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(overlay_width_points, overlay_height_points),
            );
            ui.set_min_size(full_rect.size());
            let painter = ui.painter();
            painter.rect_filled(full_rect, 0.0, egui::Color32::BLACK);
            let _ = ui.interact(
                full_rect,
                egui::Id::new("native_video_navigation_preview_bg"),
                egui::Sense::click(),
            );

            if let (Some(texture_id), Some(thumbnail)) =
                (preview_texture_id, preview.thumbnail.as_ref())
            {
                let img_w = thumbnail.width.max(1) as f32;
                let img_h = thumbnail.height.max(1) as f32;
                let scale = (overlay_width_points / img_w)
                    .min(overlay_height_points / img_h)
                    .max(0.0);
                let dst_size = egui::vec2(img_w * scale, img_h * scale);
                let dst_rect = egui::Rect::from_center_size(full_rect.center(), dst_size);
                painter.image(
                    texture_id,
                    dst_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        });

    egui::Area::new(egui::Id::new("native_video_navigation_preview_top_bar"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let rect =
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(overlay_width_points, 54.0));
            ui.set_min_size(rect.size());
            let painter = ui.painter().clone();
            draw_top_bar_background(&painter, overlay_width_points);
            let title = if preview.file_name.trim().is_empty() {
                "video"
            } else {
                preview.file_name.as_str()
            };
            draw_top_bar_text_lines(&painter, title, &preview.subtitle, 88, 120);

            let btn_size = 28.0;
            let gap = 8.0;
            let mut x = overlay_width_points - 12.0 - btn_size;
            let y = 13.0;
            draw_native_top_button(
                ui,
                &painter,
                &mut x,
                y,
                btn_size,
                btn_size,
                gap,
                "native_preview_top_close",
                NativeTopButtonGlyph::Close,
                false,
                "動画を終了",
                NativeOverlayCommand::CloseFullscreen,
                commands,
            );
        });
}

/// Returns the actual drawn rect (after user drag). `compute_hud_regions` uses this to
/// keep the `SetWindowRgn` region in sync with the panel position.
pub(super) fn draw_native_vst3_panel(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    panel: &NativeOverlayVst3Panel,
    commands: &mut Vec<NativeOverlayCommand>,
    last_emitted_panel_pos: &mut Option<[f32; 2]>,
) -> Option<egui::Rect> {
    let rect = native_vst3_panel_rect(overlay_width_points, overlay_height_points, panel);
    // 実機修正 (2026-05-12 A): `fixed_pos` → `default_pos` + `movable(true)` でドラッグ可能化。
    // egui が internal memory に position を保存するので、`Id` が同じ限りフレーム間で位置維持。
    // 戻り値の actual rect を `compute_hud_regions` に渡して region を実位置に追従させる。
    //
    // 実機修正 (Codex 続編 P2 反映): `constrain_to(overlay_rect)` で画面外に出ないよう clamp。
    // 旧版は default の `constrain: true` だが、`ctx.content_rect()` は完全に画面いっぱいに
    // なるとは限らず (= viewport 設定次第)、解像度/DPI 変更や誤ドラッグでパネルが見えなく
    // なる懸念があった。明示的に overlay 全体を境界に指定する。
    let overlay_bounds = egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(overlay_width_points, overlay_height_points),
    );
    let saved_pos = panel.panel_pos.and_then(finite_panel_pos);
    let requested_pos = saved_pos.unwrap_or(rect.min);
    let default_pos = clamp_panel_pos_to_bounds(requested_pos, rect.size(), overlay_bounds);
    let saved_pos_was_clamped = saved_pos
        .map(|pos| (pos - default_pos).length_sq() > 0.25)
        .unwrap_or(false);
    let inner = egui::Area::new(egui::Id::new("native_video_vst3_panel"))
        .order(egui::Order::Foreground)
        .default_pos(default_pos)
        .movable(true)
        .constrain_to(overlay_bounds)
        .show(ctx, |ui| {
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());
            let frame = egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(14, 14, 18, 238))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 58),
                ))
                .inner_margin(egui::Margin::same(10));
            frame.show(ui, |ui| {
                ui.set_max_size(rect.size() - egui::vec2(20.0, 20.0));
                // 実機修正 (2026-05-12 UX): X (閉じる) ボタンを削除。
                // 旧版は X で panel だけ非表示 → VST ボタン再押下で panel 再表示、もう一度で
                // VST ウィンドウ消去、という 3 状態 toggle で複雑だった。
                // 新版は VST ボタン 1 押下で panel + GUI を一緒に on/off するシンプルな 2 状態 toggle。
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("VST3")
                            .strong()
                            .color(egui::Color32::from_rgb(242, 242, 242)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(&panel.state_text)
                            .small()
                            .color(egui::Color32::from_rgb(178, 188, 202)),
                    );
                });

                if let Some(reason) = panel.disabled_reason.as_ref() {
                    ui.add_space(6.0);
                    ui.colored_label(
                        egui::Color32::from_rgb(238, 184, 88),
                        "このセッションでは VST3 が一時停止しています",
                    );
                    ui.label(
                        egui::RichText::new(reason)
                            .small()
                            .color(egui::Color32::from_rgb(208, 208, 208)),
                    );
                    return;
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("動画").small());
                    let full_resp = ui.selectable_label(!panel.video_compact, "フル");
                    if full_resp
                        .on_hover_text("動画をフルスクリーン全体に表示します")
                        .clicked()
                    {
                        commands.push(NativeOverlayCommand::SetVst3VideoCompact { compact: false });
                    }
                    let compact_resp = ui.selectable_label(panel.video_compact, "右上 1/4");
                    if compact_resp
                        .on_hover_text("動画を右上 1/4 に縮小し、プラグイン GUI の領域を空けます")
                        .clicked()
                    {
                        commands.push(NativeOverlayCommand::SetVst3VideoCompact { compact: true });
                    }
                });

                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("native_vst3_panel_scroll")
                    .max_height(native_vst3_slot_list_height(panel))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if panel.slots.is_empty() {
                            ui.label(
                                egui::RichText::new("プラグイン未設定")
                                    .color(egui::Color32::from_rgb(190, 190, 190)),
                            );
                            ui.label(
                                egui::RichText::new(
                                    "環境設定の VST3 ページでチェーンに追加してください。",
                                )
                                .small()
                                .color(egui::Color32::from_rgb(160, 160, 160)),
                            );
                        }
                        for slot in &panel.slots {
                            draw_native_vst3_slot_row(ui, slot, commands);
                        }
                    });

                ui.separator();
                ui.label(
                    egui::RichText::new("チェーンスロット")
                        .small()
                        .strong()
                        .color(egui::Color32::from_rgb(210, 210, 210)),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("読込").small());
                    for chain in &panel.chain_slots {
                        let response = ui
                            .add_enabled(
                                chain.name.is_some(),
                                egui::Button::new(chain.key_label.clone()).small(),
                            )
                            .on_hover_text(native_vst3_chain_slot_tooltip(chain));
                        if response.clicked() {
                            commands.push(NativeOverlayCommand::Vst3LoadChainSlot {
                                slot_idx: chain.idx,
                            });
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("保存").small());
                    for chain in &panel.chain_slots {
                        let response = ui
                            .add(egui::Button::new(chain.key_label.clone()).small())
                            .on_hover_text(native_vst3_chain_slot_tooltip(chain));
                        if response.clicked() {
                            commands.push(NativeOverlayCommand::Vst3SaveChainSlot {
                                slot_idx: chain.idx,
                            });
                        }
                    }
                });
            });
        });
    let actual_pos = inner.response.rect.min;
    if (inner.response.drag_stopped() || saved_pos_was_clamped)
        && panel_pos_changed(panel.panel_pos, actual_pos)
        && panel_pos_changed(*last_emitted_panel_pos, actual_pos)
    {
        let pos = [actual_pos.x, actual_pos.y];
        *last_emitted_panel_pos = Some(pos);
        commands.push(NativeOverlayCommand::SetVst3PanelPos { pos });
    }
    Some(inner.response.rect)
}

fn finite_panel_pos(pos: [f32; 2]) -> Option<egui::Pos2> {
    if pos[0].is_finite() && pos[1].is_finite() {
        Some(egui::pos2(pos[0], pos[1]))
    } else {
        None
    }
}

fn clamp_panel_pos_to_bounds(pos: egui::Pos2, size: egui::Vec2, bounds: egui::Rect) -> egui::Pos2 {
    let max_x = (bounds.max.x - size.x).max(bounds.min.x);
    let max_y = (bounds.max.y - size.y).max(bounds.min.y);
    egui::pos2(
        pos.x.clamp(bounds.min.x, max_x),
        pos.y.clamp(bounds.min.y, max_y),
    )
}

fn panel_pos_changed(saved: Option<[f32; 2]>, actual: egui::Pos2) -> bool {
    saved
        .and_then(finite_panel_pos)
        .map(|pos| (pos - actual).length_sq() > 0.25)
        .unwrap_or(true)
}

pub(super) fn draw_native_vst3_slot_row(
    ui: &mut egui::Ui,
    slot: &NativeOverlayVst3Slot,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    ui.horizontal(|ui| {
        let mut enabled = !slot.bypass;
        let label = format!("{}. {}", slot.idx + 1, slot.name);
        let checkbox = ui.add_enabled(
            !slot.placeholder,
            egui::Checkbox::new(&mut enabled, truncate_overlay_text(&label, 42)),
        );
        if checkbox.on_hover_text("ON/OFF を切り替えます").changed() {
            commands.push(NativeOverlayCommand::Vst3SetBypass {
                idx: slot.idx,
                path: slot.path.clone(),
                bypass: !enabled,
            });
        }
        if slot.placeholder {
            ui.label(
                egui::RichText::new("読込中")
                    .small()
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            );
        } else if slot.state == NativeOverlayVst3SlotState::Loading {
            ui.label(
                egui::RichText::new("loading")
                    .small()
                    .color(egui::Color32::from_rgb(170, 200, 255)),
            );
        } else if slot.state == NativeOverlayVst3SlotState::Error {
            ui.label(
                egui::RichText::new("error")
                    .small()
                    .color(egui::Color32::from_rgb(245, 120, 120)),
            );
        }
        if let Some(ms) = slot.latency_ms
            && ms > 0.0
            && !slot.bypass
        {
            ui.label(
                egui::RichText::new(format!("{ms:.1}ms"))
                    .small()
                    .color(egui::Color32::from_rgb(255, 206, 116)),
            );
        }
        if slot.auto_bypassed_for_latency && slot.bypass {
            ui.label(
                egui::RichText::new("auto-OFF")
                    .small()
                    .strong()
                    .color(egui::Color32::from_rgb(255, 150, 150)),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(28.0, 22.0), egui::Sense::click());
            draw_overlay_vst3_gui_icon(
                ui.painter(),
                rect,
                response.hovered(),
                slot.gui_visible,
                !slot.placeholder,
            );
            if response
                .on_hover_text(if slot.gui_visible {
                    "プラグイン GUI を閉じる"
                } else {
                    "プラグイン GUI を表示"
                })
                .clicked()
                && !slot.placeholder
            {
                if slot.gui_visible {
                    commands.push(NativeOverlayCommand::Vst3HideSlotGui {
                        idx: slot.idx,
                        path: slot.path.clone(),
                    });
                } else {
                    commands.push(NativeOverlayCommand::Vst3ShowSlotGui {
                        idx: slot.idx,
                        path: slot.path.clone(),
                    });
                }
            }
        });
    });
}

pub(super) fn native_vst3_chain_slot_tooltip(slot: &NativeOverlayVst3ChainSlot) -> String {
    match slot.name.as_ref() {
        Some(name) => format!("{}\n{} 件", name, slot.plugin_count),
        None => format!("VST3 Slot {} は空です", slot.key_label),
    }
}

pub(super) fn draw_native_metadata_panel(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    metadata: &NativeOverlayMetadata,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    let rect = native_metadata_panel_rect(overlay_width_points, overlay_height_points);
    egui::Area::new(egui::Id::new("native_video_metadata_panel"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_min_size(rect.size());
            let rect = ui.min_rect();
            let painter = ui.painter();
            painter.rect_filled(
                rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(14, 14, 18, 232),
            );
            painter.line_segment(
                [rect.left_top(), rect.left_bottom()],
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 55),
                ),
            );
            let _ = ui.interact(
                rect,
                egui::Id::new("native_video_metadata_panel_bg"),
                egui::Sense::click(),
            );
            painter.text(
                rect.min + egui::vec2(14.0, 14.0),
                egui::Align2::LEFT_TOP,
                "動画メタ情報",
                egui::FontId::proportional(13.0),
                egui::Color32::from_rgb(238, 238, 238),
            );

            let title = metadata
                .title
                .as_deref()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or(&metadata.file_name);
            // 「GPU経路」は ファイル open 時の能力フラグ (= GPU video device が
            // 利用可能か)。per-frame の実プレゼン経路は別行「フレーム表示」で動的に
            // 表示する。
            let gpu_path_kind = if metadata.gpu_path_active {
                "利用可能"
            } else {
                "未利用"
            };
            let decode_kind = if metadata.hw_decode_active {
                "HW"
            } else {
                "SW"
            };
            let d3d11va = if metadata.d3d11va_supported {
                "対応"
            } else {
                "非対応"
            };
            let frame_path_kind = match metadata.last_present_path {
                crate::video::decoder::PresentPathSnapshot::Gpu => "GPU (D3D11)",
                crate::video::decoder::PresentPathSnapshot::Cpu => "CPU (アップロード)",
                crate::video::decoder::PresentPathSnapshot::Pending => "確認中",
            };
            let deinterlace_text = format_deinterlace_status(
                metadata.deinterlace_mode,
                metadata.deinterlace_status,
                metadata.interlace_detected,
            );
            let audio_label = match metadata.audio_codec.as_deref() {
                Some(codec) if metadata.audio_bit_rate_bps > 0 => {
                    format!(
                        "{} ({})",
                        codec,
                        format_bitrate(metadata.audio_bit_rate_bps)
                    )
                }
                Some(codec) => codec.to_string(),
                None => "なし".to_string(),
            };
            let mut rows = vec![
                ("ファイル", metadata.file_name.clone()),
                ("タイトル", title.to_string()),
                ("アーティスト", metadata.artist.clone().unwrap_or_default()),
                (
                    "元動画URL",
                    metadata.original_url.clone().unwrap_or_default(),
                ),
                ("説明", metadata.description.clone().unwrap_or_default()),
                ("解像度", format!("{}x{}", metadata.width, metadata.height)),
                ("フレームレート", format_fps(metadata.avg_fps)),
                ("コーデック", metadata.video_codec.clone()),
                ("デコーダ", metadata.video_decoder.clone()),
                ("音声", audio_label),
                ("総ビットレート", format_bitrate(metadata.bit_rate_bps)),
                ("長さ", format_overlay_time(metadata.duration_secs)),
                ("チャプター", metadata.chapter_count.to_string()),
                ("GPU経路", gpu_path_kind.to_string()),
                ("デコード", decode_kind.to_string()),
                ("フレーム表示", frame_path_kind.to_string()),
                ("デインターレース", deinterlace_text),
                ("D3D11VA", d3d11va.to_string()),
            ];
            rows.retain(|(_, value)| !metadata_clean_text(value).is_empty());

            let content_rect = egui::Rect::from_min_max(rect.min + egui::vec2(0.0, 38.0), rect.max);
            let mut content_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(content_rect)
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
            );
            egui::ScrollArea::vertical()
                .id_salt("native_video_metadata_scroll")
                .auto_shrink([false; 2])
                .max_height(content_rect.height())
                .show(&mut content_ui, |ui| {
                    ui.add_space(6.0);
                    for (label, value) in rows {
                        let value = metadata_clean_text(&value);
                        ui.horizontal_top(|ui| {
                            ui.add_space(14.0);
                            ui.add_sized(
                                egui::vec2(88.0, 18.0),
                                egui::Label::new(
                                    egui::RichText::new(label)
                                        .monospace()
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(150)),
                                ),
                            );
                            ui.vertical(|ui| {
                                ui.set_width((rect.width() - 118.0).max(160.0));
                                if let Some(url) = crate::ui_text_links::draw_text_with_links(
                                    ui,
                                    &value,
                                    crate::ui_fonts::user_text_font(12.0),
                                    egui::Color32::from_rgb(230, 230, 230),
                                    egui::Color32::from_rgb(115, 180, 255),
                                ) {
                                    commands.push(NativeOverlayCommand::OpenExternalUrl { url });
                                }
                            });
                        });
                        ui.add_space(7.0);
                    }
                });
        });
}

pub(super) fn draw_native_tile_overlay(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    state: &NativeOverlayTileOverlay,
    tile_texture_ids: &HashMap<usize, egui::TextureId>,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    // タイルグリッドは `Order::Background` で描く。chrome (上バー / toast = Foreground、
    // perf overlay = Middle) より必ず下に置くため。
    // grid を Foreground にすると、egui の `Area` は click されたレイヤを `move_to_top`
    // するので、グリッド背景を 1 回クリックしただけで grid が上バーの上に昇格し、
    // grid 先頭の全画面不透明黒塗りが上バーを丸ごと隠してしまう (= 上バーのボタンも
    // 押せなくなる)。Order を分ければ描画順は固定で `move_to_top` の影響を受けない。
    egui::Area::new(egui::Id::new("native_video_tile_overlay"))
        .order(egui::Order::Background)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let full_rect = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(overlay_width_points, overlay_height_points),
            );
            ui.set_min_size(full_rect.size());
            let painter = ui.painter();
            painter.rect_filled(full_rect, 0.0, egui::Color32::BLACK);
            let _ = ui.interact(
                full_rect,
                egui::Id::new("native_video_tile_overlay_bg"),
                egui::Sense::click(),
            );

            // ヘッダー (タイトル / メタデータ / ボタン列) は draw_native_top_bar_tile が
            // 描画する。タイルオーバーレイは中央の preparing 文言とグリッドだけを担う。

            if state.progress_done == 0 && !state.finished {
                let message = state
                    .video_open_status
                    .map(crate::video::avio_progress::build_preparing_message)
                    .unwrap_or_else(|| "タイルを準備中...".to_string());
                painter.text(
                    full_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    message,
                    egui::FontId::proportional(20.0),
                    egui::Color32::from_gray(180),
                );
            }

            let columns = state.columns.max(1);
            let label_h = 16.0;
            let gap_x = 6.0;
            let gap_y = 6.0;
            let grid_left = 16.0;
            let total_grid_w = (overlay_width_points - grid_left * 2.0).max(240.0);
            let tile_w = ((total_grid_w - gap_x * columns.saturating_sub(1) as f32)
                / columns as f32)
                .floor()
                .max(40.0);
            let aspect_h = if state.tile_w > 0 && state.tile_h > 0 {
                state.tile_h as f32 / state.tile_w as f32
            } else {
                9.0 / 16.0
            };
            let tile_h = (tile_w * aspect_h).round().max(30.0);
            let grid_top = 56.0;

            for idx in 0..state.timestamps.len() {
                let col = idx % columns;
                let row = idx / columns;
                let x0 = grid_left + (tile_w + gap_x) * col as f32;
                let y0 = grid_top + (tile_h + label_h + gap_y) * row as f32;
                let tile_rect =
                    egui::Rect::from_min_size(egui::pos2(x0, y0), egui::vec2(tile_w, tile_h));
                if tile_rect.max.y > overlay_height_points - 20.0 {
                    continue;
                }

                painter.rect_filled(tile_rect, 4.0, egui::Color32::from_rgb(28, 28, 32));
                if let Some(texture_id) = tile_texture_ids.get(&idx) {
                    painter.image(
                        *texture_id,
                        tile_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                } else {
                    painter.text(
                        tile_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "...",
                        egui::FontId::proportional(20.0),
                        egui::Color32::from_gray(120),
                    );
                }
                painter.rect_stroke(
                    tile_rect,
                    4.0,
                    egui::Stroke::new(1.0, egui::Color32::from_gray(82)),
                    egui::StrokeKind::Inside,
                );

                let pts = state.timestamps.get(idx).copied().unwrap_or(0.0);
                painter.text(
                    egui::pos2(tile_rect.center().x, tile_rect.max.y + label_h * 0.5),
                    egui::Align2::CENTER_CENTER,
                    format_overlay_time(pts),
                    egui::FontId::proportional(12.0),
                    egui::Color32::from_rgb(220, 220, 220),
                );

                let resp = ui.interact(
                    tile_rect,
                    egui::Id::new(("native_video_tile", idx)),
                    egui::Sense::click(),
                );
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    painter.rect_stroke(
                        tile_rect.expand(1.0),
                        4.0,
                        egui::Stroke::new(2.0, egui::Color32::from_rgb(235, 235, 235)),
                        egui::StrokeKind::Inside,
                    );
                }
                if resp.clicked() {
                    commands.push(NativeOverlayCommand::TileSeek { target_secs: pts });
                }
            }
        });
}

/// `source_delta_ms` を再生速度で正規化した「実 frame interval (ms)」を返す。
/// 0.5x なら 2 倍、2x なら半分。speed が NaN や 0 近傍なら 1.0 倍として扱う。
pub(super) fn native_perf_effective_interval_ms(sample: &NativeOverlayPerfSample) -> f32 {
    let speed = if sample.playback_speed.is_finite() && sample.playback_speed > 0.05 {
        sample.playback_speed
    } else {
        1.0
    };
    sample.source_delta_ms / speed
}

pub(super) fn native_perf_expected_frame_ms(history: &[NativeOverlayPerfSample]) -> f32 {
    native_perf_expected_frame_ms_from_values(
        history
            .iter()
            .rev()
            .take(180)
            .map(native_perf_effective_interval_ms),
    )
    .unwrap_or(16.67)
}

pub(super) fn native_perf_expected_frame_ms_from_samples<I>(samples: I) -> Option<f32>
where
    I: IntoIterator<Item = NativeOverlayPerfSample>,
{
    native_perf_expected_frame_ms_from_values(
        samples
            .into_iter()
            .map(|sample| native_perf_effective_interval_ms(&sample)),
    )
}

pub(super) fn native_perf_expected_frame_ms_from_values<I>(values: I) -> Option<f32>
where
    I: IntoIterator<Item = f32>,
{
    // Upper bound 250ms は MIN_PLAYBACK_SPEED=0.25 × 24fps (= 166.7ms) に余裕を持たせた値。
    // filter (= 入力 reject) と clamp (= 出力丸め) で同じ値にしておくと、24fps を 0.25x で
    // 再生した時に median が input を通過しても出力で潰れる、という不整合を防げる。
    const EXPECTED_MS_MAX: f32 = 250.0;
    let mut values: Vec<f32> = values
        .into_iter()
        .filter(|value| value.is_finite() && *value > 0.5 && *value < EXPECTED_MS_MAX)
        .collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(values[values.len() / 2].clamp(1.0, EXPECTED_MS_MAX))
}

pub(super) fn native_perf_visible_fps(history: &[NativeOverlayPerfSample]) -> Option<f32> {
    let last = history.last()?;
    let now = last.arrival;
    let mut prev_visible = false;
    let mut interval_sum_ms = 0.0_f32;
    let mut interval_count = 0_u32;
    for sample in history {
        let age = now.saturating_duration_since(sample.arrival).as_secs_f32();
        if age > NATIVE_PERF_GRAPH_SECS {
            continue;
        }
        if prev_visible && sample.interval_ms.is_finite() && sample.interval_ms > 0.0 {
            interval_sum_ms += sample.interval_ms;
            interval_count = interval_count.saturating_add(1);
        }
        prev_visible = true;
    }
    if interval_count == 0 || interval_sum_ms <= 0.0 {
        return None;
    }
    Some((interval_count as f32 * 1000.0) / interval_sum_ms)
}

pub(super) fn native_perf_av_value_color(value_ms: f32) -> egui::Color32 {
    let abs = value_ms.abs();
    if !abs.is_finite() || abs < NATIVE_PERF_AV_OFFSET_NORMAL_MS {
        egui::Color32::from_rgb(168, 176, 188)
    } else if abs < NATIVE_PERF_AV_OFFSET_SEVERE_MS {
        egui::Color32::from_rgb(255, 152, 60)
    } else {
        egui::Color32::from_rgb(255, 112, 112)
    }
}

pub(super) fn native_perf_sample_has_late_drop(sample: &NativeOverlayPerfSample) -> bool {
    sample.late_drop_delta > 0
}

pub(super) fn native_perf_sample_has_audio_underrun_band(sample: &NativeOverlayPerfSample) -> bool {
    sample.audio_active && sample.audio_underrun_active
}

pub(super) fn native_perf_should_thin_sample(
    sample: &NativeOverlayPerfSample,
    last_draw_x: f32,
    x: f32,
    idx: usize,
    history_len: usize,
) -> bool {
    !native_perf_sample_has_late_drop(sample)
        && (last_draw_x - x).abs() < 0.75
        && idx + 1 < history_len
}

pub(super) fn thumbnail_rgba_key(thumbnail: &NativeOverlayThumbnail) -> u64 {
    let ptr = Arc::as_ptr(&thumbnail.rgba) as usize as u64;
    ptr ^ thumbnail.target_secs.to_bits()
}

pub(super) fn fit_rect_in_rect(content_size: egui::Vec2, outer: egui::Rect) -> egui::Rect {
    if content_size.x <= 0.0 || content_size.y <= 0.0 {
        return outer;
    }
    let scale = (outer.width() / content_size.x).min(outer.height() / content_size.y);
    let size = content_size * scale;
    egui::Rect::from_center_size(outer.center(), size)
}

pub(super) fn native_jump_panel_width() -> f32 {
    320.0
}

pub(super) fn native_metadata_panel_width() -> f32 {
    430.0
}

pub(super) fn native_panel_top() -> f32 {
    56.0
}

pub(super) fn native_panel_hover_bottom(overlay_height_points: f32) -> f32 {
    (overlay_height_points - 48.0).max(native_panel_top())
}

pub(super) fn native_panel_hover_rect(
    min: egui::Pos2,
    size: egui::Vec2,
    overlay_height_points: f32,
) -> egui::Rect {
    let bottom = native_panel_hover_bottom(overlay_height_points);
    egui::Rect::from_min_max(egui::pos2(min.x, 0.0), egui::pos2(min.x + size.x, bottom))
}

pub(super) fn native_jump_panel_rect(overlay_height_points: f32) -> egui::Rect {
    let top = native_panel_top();
    let panel_h = (overlay_height_points - top - 48.0).max(240.0);
    egui::Rect::from_min_size(
        egui::pos2(0.0, top),
        egui::vec2(native_jump_panel_width(), panel_h),
    )
}

pub(super) fn native_metadata_panel_rect(
    overlay_width_points: f32,
    overlay_height_points: f32,
) -> egui::Rect {
    let panel_w = native_metadata_panel_width().min(overlay_width_points * 0.5);
    let top = native_panel_top();
    let panel_h = (overlay_height_points - top - 48.0).max(260.0);
    egui::Rect::from_min_size(
        egui::pos2(overlay_width_points - panel_w, top),
        egui::vec2(panel_w, panel_h),
    )
}

pub(super) fn native_vst3_panel_rect(
    overlay_width_points: f32,
    overlay_height_points: f32,
    panel: &NativeOverlayVst3Panel,
) -> egui::Rect {
    let width = 380.0_f32.min((overlay_width_points - 32.0).max(260.0));
    let row_count = panel.slots.len().max(1).min(10) as f32;
    let desired_height = 154.0 + row_count * 28.0;
    let max_height = (overlay_height_points - native_panel_top() - 56.0).max(240.0);
    let height = desired_height.clamp(236.0, max_height.min(620.0));
    egui::Rect::from_min_size(
        egui::pos2(18.0, native_panel_top() + 10.0),
        egui::vec2(width, height),
    )
}

pub(super) fn native_vst3_slot_list_height(panel: &NativeOverlayVst3Panel) -> f32 {
    let row_count = panel.slots.len().max(1).min(10) as f32;
    (row_count * 28.0 + 8.0).min(288.0)
}

pub(super) fn metadata_clean_text(value: &str) -> String {
    let normalized = value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace("\\r\\n", "\n")
        .replace("\\n", "\n");
    let mut lines = Vec::new();
    let mut last_was_blank = true;
    for line in normalized.lines() {
        let cleaned = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if cleaned.is_empty() {
            if !last_was_blank {
                lines.push(String::new());
                last_was_blank = true;
            }
        } else {
            lines.push(cleaned);
            last_was_blank = false;
        }
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

pub(super) fn timeline_markers_match(
    a: &[NativeOverlayTimelineMarker],
    b: &[NativeOverlayTimelineMarker],
) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(a, b)| a.kind == b.kind && (a.pts_secs - b.pts_secs).abs() <= f64::EPSILON)
}

pub(super) fn jump_entries_match(
    a: &[NativeOverlayJumpEntry],
    b: &[NativeOverlayJumpEntry],
) -> bool {
    a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(a, b)| {
            a.kind == b.kind
                && a.bookmark_id == b.bookmark_id
                && a.title == b.title
                && (a.pts_secs - b.pts_secs).abs() <= f64::EPSILON
                && a.thumbnail.as_ref().map(thumbnail_rgba_key)
                    == b.thumbnail.as_ref().map(thumbnail_rgba_key)
        })
}

pub(super) fn target_has_marker(
    markers: &[NativeOverlayTimelineMarker],
    target_secs: f64,
    duration_secs: f64,
    kind_matches: impl Fn(NativeOverlayTimelineMarkerKind) -> bool,
) -> bool {
    let bucket_window = crate::video::thumbnail::SECONDS_PER_BUCKET * 1.5;
    let visual_window = (duration_secs / 300.0).clamp(0.15, 1.5);
    let tolerance = bucket_window.max(visual_window);
    markers.iter().any(|marker| {
        kind_matches(marker.kind) && (marker.pts_secs - target_secs).abs() <= tolerance
    })
}

pub(super) fn draw_timeline_marker(
    painter: &egui::Painter,
    bar_rect: egui::Rect,
    duration_secs: f64,
    marker: NativeOverlayTimelineMarker,
) {
    if duration_secs <= 0.0 || !marker.pts_secs.is_finite() {
        return;
    }
    let frac = (marker.pts_secs / duration_secs).clamp(0.0, 1.0) as f32;
    let x = bar_rect.min.x + bar_rect.width() * frac;
    let (height, color) = match marker.kind {
        NativeOverlayTimelineMarkerKind::Pin => (30.0, egui::Color32::from_rgb(140, 245, 170)),
        NativeOverlayTimelineMarkerKind::Bookmark => (28.0, egui::Color32::from_rgb(255, 220, 82)),
        NativeOverlayTimelineMarkerKind::Chapter => (24.0, egui::Color32::from_rgb(115, 210, 255)),
    };
    let top = bar_rect.center().y - height * 0.5;
    let bottom = bar_rect.center().y + height * 0.5;
    painter.line_segment(
        [egui::pos2(x, top), egui::pos2(x, bottom)],
        egui::Stroke::new(2.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 150)),
    );
    painter.line_segment(
        [egui::pos2(x, top), egui::pos2(x, bottom)],
        egui::Stroke::new(1.0, color),
    );
}

pub(super) fn draw_overlay_button_bg(
    painter: &egui::Painter,
    rect: egui::Rect,
    hovered: bool,
    active: bool,
) {
    let bg = if active {
        egui::Color32::from_rgba_unmultiplied(80, 140, 220, 190)
    } else if hovered {
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 34)
    } else {
        egui::Color32::TRANSPARENT
    };
    painter.rect_filled(rect, 4.0, bg);
}

pub(super) fn draw_overlay_play_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(c.x - r * 0.45, c.y - r * 0.70),
            egui::pos2(c.x - r * 0.45, c.y + r * 0.70),
            egui::pos2(c.x + r * 0.65, c.y),
        ],
        egui::Color32::WHITE,
        egui::Stroke::NONE,
    ));
}

pub(super) fn draw_overlay_pause_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let stroke = egui::Stroke::new((r * 0.34).max(2.0), egui::Color32::WHITE);
    painter.line_segment(
        [
            egui::pos2(c.x - r * 0.35, c.y - r),
            egui::pos2(c.x - r * 0.35, c.y + r),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(c.x + r * 0.35, c.y - r),
            egui::pos2(c.x + r * 0.35, c.y + r),
        ],
        stroke,
    );
}

pub(super) fn draw_overlay_replay_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    use std::f32::consts::PI;

    let white = egui::Color32::WHITE;
    let stroke_w = (r * 0.18).max(1.8);
    let stroke = egui::Stroke::new(stroke_w, white);
    let radius = r * 0.78;

    // Draw a counter-clockwise replay arc, leaving room for the arrow head near 12 o'clock.
    let gap_half = 0.45;
    let start_angle = -PI / 2.0 + gap_half;
    let end_angle = start_angle - (2.0 * PI - 2.0 * gap_half);
    let segments = 40;
    let mut arc_points = Vec::with_capacity(segments + 1);
    for i in 0..=segments {
        let t = i as f32 / segments as f32;
        let angle = start_angle + (end_angle - start_angle) * t;
        arc_points.push(egui::pos2(
            c.x + radius * angle.cos(),
            c.y + radius * angle.sin(),
        ));
    }
    painter.add(egui::Shape::line(arc_points, stroke));

    let arrow_size = r * 0.32;
    let end_pos = egui::pos2(
        c.x + radius * end_angle.cos(),
        c.y + radius * end_angle.sin(),
    );
    let tangent = end_angle - PI / 2.0;
    let tip = egui::pos2(
        end_pos.x + arrow_size * tangent.cos(),
        end_pos.y + arrow_size * tangent.sin(),
    );
    let base_offset = arrow_size * 0.55;
    let base_a = egui::pos2(
        end_pos.x + base_offset * end_angle.cos(),
        end_pos.y + base_offset * end_angle.sin(),
    );
    let base_b = egui::pos2(
        end_pos.x - base_offset * end_angle.cos(),
        end_pos.y - base_offset * end_angle.sin(),
    );
    painter.add(egui::Shape::convex_polygon(
        vec![tip, base_a, base_b],
        white,
        egui::Stroke::NONE,
    ));

    let tri_x = r * 0.38;
    let tri_y = r * 0.42;
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(c.x + tri_x, c.y),
            egui::pos2(c.x - tri_x * 0.65, c.y - tri_y),
            egui::pos2(c.x - tri_x * 0.65, c.y + tri_y),
        ],
        white,
        egui::Stroke::NONE,
    ));
}

pub(super) fn draw_overlay_loop_icon(
    painter: &egui::Painter,
    c: egui::Pos2,
    r: f32,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new((r * 0.16).max(1.5), color);
    let left = c.x - r * 0.78;
    let right = c.x + r * 0.78;
    let top = c.y - r * 0.42;
    let bottom = c.y + r * 0.42;
    painter.line_segment([egui::pos2(left, top), egui::pos2(right, top)], stroke);
    painter.line_segment(
        [egui::pos2(right, bottom), egui::pos2(left, bottom)],
        stroke,
    );
    painter.line_segment(
        [egui::pos2(left, top), egui::pos2(left, c.y - r * 0.12)],
        stroke,
    );
    painter.line_segment(
        [egui::pos2(right, bottom), egui::pos2(right, c.y + r * 0.12)],
        stroke,
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(right + r * 0.03, top),
            egui::pos2(right - r * 0.34, top - r * 0.25),
            egui::pos2(right - r * 0.34, top + r * 0.25),
        ],
        color,
        egui::Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(left - r * 0.03, bottom),
            egui::pos2(left + r * 0.34, bottom - r * 0.25),
            egui::pos2(left + r * 0.34, bottom + r * 0.25),
        ],
        color,
        egui::Stroke::NONE,
    ));
}

pub(super) fn draw_overlay_bookmark_icon(
    painter: &egui::Painter,
    c: egui::Pos2,
    r: f32,
    fill: egui::Color32,
) {
    let rect = egui::Rect::from_center_size(c, egui::vec2(r * 1.10, r * 1.55));
    let notch = egui::pos2(rect.center().x, rect.max.y - r * 0.35);
    painter.add(egui::Shape::convex_polygon(
        vec![
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            notch,
            rect.left_bottom(),
        ],
        fill,
        egui::Stroke::new(1.2, egui::Color32::from_rgb(255, 245, 190)),
    ));
}

pub(super) fn draw_overlay_pencil_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new(1.6, color);
    let a = egui::pos2(
        rect.min.x + rect.width() * 0.30,
        rect.max.y - rect.height() * 0.28,
    );
    let b = egui::pos2(
        rect.max.x - rect.width() * 0.24,
        rect.min.y + rect.height() * 0.34,
    );
    painter.line_segment([a, b], stroke);
    painter.line_segment(
        [
            egui::pos2(a.x - 2.2, a.y + 2.2),
            egui::pos2(a.x + 3.2, a.y + 3.2),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(b.x - 2.8, b.y - 1.6),
            egui::pos2(b.x + 1.8, b.y + 2.8),
        ],
        stroke,
    );
}

pub(super) fn draw_overlay_pin_icon(
    painter: &egui::Painter,
    c: egui::Pos2,
    r: f32,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new((r * 0.18).max(1.5), color);
    let head = egui::Rect::from_center_size(
        egui::pos2(c.x - r * 0.05, c.y - r * 0.32),
        egui::vec2(r * 0.95, r * 0.48),
    );
    painter.rect_filled(head, 1.5, color);
    painter.line_segment(
        [
            egui::pos2(c.x - r * 0.06, c.y - r * 0.05),
            egui::pos2(c.x + r * 0.32, c.y + r * 0.44),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(c.x + r * 0.32, c.y + r * 0.44),
            egui::pos2(c.x + r * 0.08, c.y + r * 0.72),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(c.x + r * 0.10, c.y + r * 0.26),
            egui::pos2(c.x - r * 0.48, c.y + r * 0.84),
        ],
        egui::Stroke::new((r * 0.12).max(1.2), color),
    );
}

pub(super) fn draw_overlay_speaker_icon(
    painter: &egui::Painter,
    c: egui::Pos2,
    r: f32,
    muted: bool,
) {
    let white = egui::Color32::WHITE;
    let body = egui::Rect::from_min_max(
        egui::pos2(c.x - r * 0.75, c.y - r * 0.38),
        egui::pos2(c.x - r * 0.40, c.y + r * 0.38),
    );
    painter.rect_filled(body, 1.0, white);
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(body.max.x, body.min.y),
            egui::pos2(c.x + r * 0.10, c.y - r * 0.68),
            egui::pos2(c.x + r * 0.10, c.y + r * 0.68),
            egui::pos2(body.max.x, body.max.y),
        ],
        white,
        egui::Stroke::NONE,
    ));
    if muted {
        let stroke = egui::Stroke::new((r * 0.16).max(2.0), egui::Color32::from_rgb(240, 100, 100));
        painter.line_segment(
            [
                egui::pos2(c.x + r * 0.30, c.y - r * 0.50),
                egui::pos2(c.x + r * 0.85, c.y + r * 0.50),
            ],
            stroke,
        );
        painter.line_segment(
            [
                egui::pos2(c.x + r * 0.85, c.y - r * 0.50),
                egui::pos2(c.x + r * 0.30, c.y + r * 0.50),
            ],
            stroke,
        );
    } else {
        let stroke = egui::Stroke::new((r * 0.13).max(1.4), white);
        painter.line_segment(
            [
                egui::pos2(c.x + r * 0.35, c.y - r * 0.35),
                egui::pos2(c.x + r * 0.35, c.y + r * 0.35),
            ],
            stroke,
        );
        painter.line_segment(
            [
                egui::pos2(c.x + r * 0.62, c.y - r * 0.55),
                egui::pos2(c.x + r * 0.62, c.y + r * 0.55),
            ],
            stroke,
        );
    }
}

pub(super) fn finite_nonnegative(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

pub(super) fn finite_video_volume(value: f64) -> f64 {
    if value.is_finite() {
        crate::settings::clamp_video_volume(value)
    } else {
        0.0
    }
}

pub(super) fn format_overlay_time(secs: f64) -> String {
    let total = finite_nonnegative(secs).round() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub(super) fn format_overlay_time_millis(secs: f64) -> String {
    let total_ms = (finite_nonnegative(secs) * 1000.0).round() as u64;
    let total_secs = total_ms / 1000;
    let ms = total_ms % 1000;
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{ms:03}")
    } else {
        format!("{m}:{s:02}.{ms:03}")
    }
}

fn overlay_time_rounded_second_key(secs: f64) -> u64 {
    finite_nonnegative(secs).round() as u64
}

fn overlay_time_has_millis(secs: f64) -> bool {
    ((finite_nonnegative(secs) * 1000.0).round() as u64) % 1000 != 0
}

pub(super) fn format_native_jump_entry_time(
    entry: &NativeOverlayJumpEntry,
    entries: &[NativeOverlayJumpEntry],
) -> String {
    let second_key = overlay_time_rounded_second_key(entry.pts_secs);
    let same_second_count = entries
        .iter()
        .filter(|other| overlay_time_rounded_second_key(other.pts_secs) == second_key)
        .take(2)
        .count();
    if overlay_time_has_millis(entry.pts_secs) || same_second_count > 1 {
        format_overlay_time_millis(entry.pts_secs)
    } else {
        format_overlay_time(entry.pts_secs)
    }
}

pub(super) fn format_tile_interval(secs: f64) -> String {
    let secs = finite_nonnegative(secs);
    if secs >= 60.0 {
        format!("{}分", (secs / 60.0).round() as u64)
    } else {
        format!("{}秒", secs.round() as u64)
    }
}

pub(super) fn format_fps(fps: f64) -> String {
    if fps.is_finite() && fps > 0.0 {
        format!("{fps:.2}fps")
    } else {
        "fps ?".to_string()
    }
}

pub(super) fn format_bitrate(bit_rate_bps: i64) -> String {
    if bit_rate_bps <= 0 {
        return "unknown".to_string();
    }
    let mbps = bit_rate_bps as f64 / 1_000_000.0;
    if mbps >= 1.0 {
        format!("{mbps:.1}Mbps")
    } else {
        format!("{}kbps", (bit_rate_bps as f64 / 1000.0).round() as i64)
    }
}

/// 右パネル「デインターレース」行の表示文字列を組み立てる。
///
/// 入力は open 時の Settings モード (`mode`) と decoder thread から動的に
/// 更新される `status` / `interlace_detected` の組み合わせ。
pub(super) fn format_deinterlace_status(
    mode: crate::settings::VideoDeinterlaceMode,
    status: crate::video::decoder::DeinterlaceStatusSnapshot,
    interlace_detected: bool,
) -> String {
    use crate::settings::VideoDeinterlaceMode;
    use crate::video::decoder::DeinterlaceStatusSnapshot as S;
    match mode {
        VideoDeinterlaceMode::Off => "オフ".to_string(),
        VideoDeinterlaceMode::On => match status {
            S::Active => "常時 (適用中)".to_string(),
            S::Failed => "常時 (初期化失敗)".to_string(),
            S::Pending | S::Inactive => "常時 (準備中)".to_string(),
        },
        VideoDeinterlaceMode::Auto => match status {
            S::Active => "自動 - 適用中".to_string(),
            S::Failed => "自動 - 初期化失敗".to_string(),
            S::Pending => "自動 - 確認中".to_string(),
            S::Inactive => {
                if interlace_detected {
                    "自動 - 待機中".to_string()
                } else {
                    "自動 - プログレッシブ".to_string()
                }
            }
        },
    }
}

pub(super) fn truncate_overlay_text(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let mut out = String::new();
    for _ in 0..max_chars {
        let Some(ch) = chars.next() else {
            return out;
        };
        out.push(ch);
    }
    if chars.next().is_some() {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// jump / metadata 両パネルの底辺がホバー判定の底辺と一致し、シーク HUD
    /// (top y = overlay_h - 46) の上に 2pt の隙間が空くことを保証する回帰テスト。
    /// 上ホバーバー bottom y = 54 と panel top y = 56 の 2pt 隙間と対称になる前提。
    #[test]
    fn side_panel_bottoms_match_hover_bottom() {
        let overlay_h = 1080.0_f32;
        let overlay_w = 1920.0_f32;

        let hover_bottom = native_panel_hover_bottom(overlay_h);
        let jump = native_jump_panel_rect(overlay_h);
        let meta = native_metadata_panel_rect(overlay_w, overlay_h);

        assert_eq!(hover_bottom, overlay_h - 48.0);
        assert_eq!(jump.max.y, hover_bottom);
        assert_eq!(meta.max.y, hover_bottom);
    }

    fn jump_entry(pts_secs: f64) -> NativeOverlayJumpEntry {
        NativeOverlayJumpEntry {
            pts_secs,
            kind: NativeOverlayTimelineMarkerKind::Chapter,
            title: None,
            bookmark_id: None,
            thumbnail: None,
        }
    }

    #[test]
    fn native_jump_time_uses_millis_for_fractional_or_duplicate_seconds() {
        let entries = vec![
            jump_entry(80.0),
            jump_entry(80.04),
            jump_entry(597.88),
            jump_entry(600.0),
        ];

        assert_eq!(
            format_native_jump_entry_time(&entries[0], &entries),
            "1:20.000"
        );
        assert_eq!(
            format_native_jump_entry_time(&entries[1], &entries),
            "1:20.040"
        );
        assert_eq!(
            format_native_jump_entry_time(&entries[2], &entries),
            "9:57.880"
        );
        assert_eq!(
            format_native_jump_entry_time(&entries[3], &entries),
            "10:00"
        );
    }

    fn perf_sample(late_drop_delta: u32, interval_ms: f32) -> NativeOverlayPerfSample {
        NativeOverlayPerfSample {
            arrival: Instant::now(),
            interval_ms,
            total_ms: 0.0,
            copy_ms: 0.0,
            present_waitable_ms: 0.0,
            present_call_ms: 0.0,
            late_ms: 0.0,
            late_drop_delta,
            source_delta_ms: 16.67,
            playback_speed: 1.0,
            av_drift_ms: 0.0,
            av_offset_ms: 0.0,
            audio_active: true,
            audio_lead_ms: 0.0,
            audio_underrun_active: false,
        }
    }

    #[test]
    fn perf_red_marker_follows_drop_delta_not_interval_gap() {
        assert!(!native_perf_sample_has_late_drop(&perf_sample(0, 73.3)));
        assert!(native_perf_sample_has_late_drop(&perf_sample(1, 16.7)));
    }

    #[test]
    fn perf_thinning_preserves_drop_marker() {
        let normal = perf_sample(0, 16.7);
        let dropped = perf_sample(1, 16.7);
        assert!(native_perf_should_thin_sample(&normal, 100.0, 99.5, 3, 10));
        assert!(!native_perf_should_thin_sample(
            &dropped, 100.0, 99.5, 3, 10
        ));
    }

    #[test]
    fn perf_visible_fps_uses_visible_intervals_only() {
        let base = Instant::now();
        let mut old = perf_sample(0, 0.0);
        old.arrival = base;
        let mut after_gap = perf_sample(0, 7000.0);
        after_gap.arrival = base + Duration::from_millis(7000);
        let mut current = perf_sample(0, 16.0);
        current.arrival = base + Duration::from_millis(7016);

        let fps = native_perf_visible_fps(&[old, after_gap, current]).unwrap();
        assert!((fps - 62.5).abs() < 0.01, "fps={fps}");
    }

    #[test]
    fn perf_av_value_color_keeps_normal_jitter_gray() {
        let gray = egui::Color32::from_rgb(168, 176, 188);
        assert_eq!(native_perf_av_value_color(0.0), gray);
        assert_eq!(native_perf_av_value_color(19.9), gray);
        assert_eq!(native_perf_av_value_color(-99.9), gray);
        assert_eq!(native_perf_av_value_color(f32::NAN), gray);
    }

    #[test]
    fn perf_av_value_color_warns_only_outside_normal_range() {
        let warning = egui::Color32::from_rgb(255, 152, 60);
        let severe = egui::Color32::from_rgb(255, 112, 112);
        assert_eq!(native_perf_av_value_color(100.0), warning);
        assert_eq!(native_perf_av_value_color(-499.9), warning);
        assert_eq!(native_perf_av_value_color(500.0), severe);
        assert_eq!(native_perf_av_value_color(-5000.0), severe);
    }

    #[test]
    fn perf_underrun_band_requires_active_audio() {
        let mut audio_active = perf_sample(0, 16.7);
        audio_active.audio_underrun_active = true;
        audio_active.audio_active = true;
        audio_active.av_offset_ms = f32::NAN;
        let mut audio_inactive = audio_active;
        audio_inactive.audio_active = false;

        assert!(native_perf_sample_has_audio_underrun_band(&audio_active));
        assert!(!native_perf_sample_has_audio_underrun_band(&audio_inactive));
    }
}
