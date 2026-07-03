use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ui_helpers::HoverTipExt;

use super::{
    NativeBookmarkTitleEdit, NativeBulkBookmarkDialog, NativeFrameStepHold, NativeOverlayCommand,
    NativeOverlayJumpEntry, NativeOverlayMetadata, NativeOverlayNavigationPreview,
    NativeOverlayPerfSample, NativeOverlayPerfSnapshot, NativeOverlayRingGuide,
    NativeOverlayRingPicker, NativeOverlayShortcutHelp, NativeOverlayShortcutLabels,
    NativeOverlayTagDef, NativeOverlayThumbnail, NativeOverlayTileOverlay,
    NativeOverlayTimelineMarker, NativeOverlayTimelineMarkerKind, NativeOverlayToast,
    NativeOverlayVst3ChainSlot, NativeOverlayVst3Panel, NativeOverlayVst3Slot,
    NativeOverlayVst3SlotState,
};

const NATIVE_PERF_GRAPH_SECS: f32 = 6.0;
const NATIVE_PERF_AV_OFFSET_NORMAL_MS: f32 = 100.0;
const NATIVE_PERF_AV_OFFSET_SEVERE_MS: f32 = 500.0;

pub(super) fn native_label_with_shortcut(label: &str, shortcut: Option<&str>) -> String {
    match shortcut {
        Some(shortcut) if !shortcut.trim().is_empty() => format!("{label} [{shortcut}]"),
        _ => label.to_owned(),
    }
}

pub(super) fn native_joined_shortcuts(shortcuts: &[Option<&str>]) -> Option<String> {
    let labels = shortcuts
        .iter()
        .flatten()
        .map(|label| label.trim())
        .filter(|label| !label.is_empty())
        .collect::<Vec<_>>();
    (!labels.is_empty()).then(|| labels.join(" / "))
}

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

/// ジャンプ / ブックマークパネル本体 (`draw_native_jump_panel_body`) の描画オプション。
///
/// 動画は全機能 (ピン / チャプター / サムネイル) を出す。音楽ビュー (Inc 5c-A) は
/// **ブックマークのみ** を出すため `show_pins` / `show_chapters` / `show_thumbnails` /
/// `show_pin_button` を false にして同じ本体を共有する。これにより動画↔音声で
/// パネルの見た目・行レイアウト・一括登録ダイアログが同一コードで揃う (Inc 7 の
/// 動画→音声モードで切替前後の視覚ジャンプを防ぐ、docs §5.8)。
pub(crate) struct NativeJumpPanelOptions<'a> {
    /// パネル左上のタイトル ("ジャンプ" / "ブックマーク")。
    pub title: &'a str,
    /// エントリが空のときの案内文。
    pub empty_text: &'a str,
    /// ヘッダのピン留めボタンを出すか (音声は false)。
    pub show_pin_button: bool,
    /// ヘッダの一括登録ボタンを出すか。
    pub show_bulk_button: bool,
    /// ピン留めセクションを出すか (音声は false)。
    pub show_pins: bool,
    /// チャプターセクションを出すか (音声は false)。
    pub show_chapters: bool,
    /// 種別セクション見出し ("ブックマーク" 等) を出すか。音声はブックマークのみで
    /// 見出しが冗長なので false にしてフラットな一覧にする。
    pub show_section_headers: bool,
    /// 行の代表サムネイル列を出すか (音声は false)。
    pub show_thumbnails: bool,
}

/// 動画 native overlay 用のパネルオプション (従来挙動と完全一致)。
const VIDEO_JUMP_PANEL_OPTIONS: NativeJumpPanelOptions<'static> = NativeJumpPanelOptions {
    title: "ジャンプ",
    empty_text: "ピン・ブックマーク・チャプターはまだありません",
    show_pin_button: true,
    show_bulk_button: true,
    show_pins: true,
    show_chapters: true,
    show_section_headers: true,
    show_thumbnails: true,
};

pub(super) fn draw_native_jump_panel(
    ctx: &egui::Context,
    overlay_height_points: f32,
    position_secs: f64,
    entries: &[NativeOverlayJumpEntry],
    jump_texture_ids: &HashMap<usize, egui::TextureId>,
    shortcut_labels: Option<&NativeOverlayShortcutLabels>,
    bookmark_title_edit: &mut Option<NativeBookmarkTitleEdit>,
    bulk_bookmark_dialog: &mut Option<NativeBulkBookmarkDialog>,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    let panel_rect = native_jump_panel_rect(overlay_height_points);

    egui::Area::new(egui::Id::new("native_video_jump_panel"))
        .order(egui::Order::Foreground)
        .fixed_pos(panel_rect.min)
        .show(ctx, |ui| {
            ui.set_min_size(panel_rect.size());
            draw_native_jump_panel_body(
                ui,
                panel_rect,
                &VIDEO_JUMP_PANEL_OPTIONS,
                position_secs,
                entries,
                jump_texture_ids,
                shortcut_labels,
                bookmark_title_edit,
                bulk_bookmark_dialog,
                commands,
            );
        });
}

/// ジャンプ / ブックマークパネルの本体描画 (背景 + ヘッダボタン + スクロール一覧)。
///
/// `panel_rect` は呼び出し側が確定した実 rect (動画は `native_jump_panel_rect`、音楽ビューは
/// 端ホバーの overlay rect)。動画は自前の `egui::Area` の中でこれを呼び、音楽ビューも
/// 同様に Area を作ってから呼ぶ。発行される `NativeOverlayCommand` を、音楽側は music の
/// 実操作へ翻訳するアダプタで受ける (docs §5.8)。
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_native_jump_panel_body(
    ui: &mut egui::Ui,
    panel_rect: egui::Rect,
    opts: &NativeJumpPanelOptions,
    position_secs: f64,
    entries: &[NativeOverlayJumpEntry],
    jump_texture_ids: &HashMap<usize, egui::TextureId>,
    shortcut_labels: Option<&NativeOverlayShortcutLabels>,
    bookmark_title_edit: &mut Option<NativeBookmarkTitleEdit>,
    bulk_bookmark_dialog: &mut Option<NativeBulkBookmarkDialog>,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    let rect = panel_rect;
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
        opts.title,
        egui::FontId::proportional(13.0),
        egui::Color32::from_rgb(238, 238, 238),
    );

    // 一括ブックマーク登録ダイアログを開くボタン。
    // 配置 (実機フィードバック反映 2026-05-26): 左から右に「Pin (- 100pt) →
    // Bookmark (- 68pt) → 一括 Bookmark (- 36pt)」。ユーザー要求: ピン留めを左端、
    // ブックマーク系を右側にまとめる。一括は低頻度なので右端維持。
    if opts.show_bulk_button {
        let bulk_rect = egui::Rect::from_min_size(
            rect.min + egui::vec2(rect.width() - 36.0, 6.0),
            egui::vec2(26.0, 24.0),
        );
        let bulk_resp = ui.interact(
            bulk_rect,
            egui::Id::new("native_jump_bulk_bookmark"),
            egui::Sense::click(),
        );
        draw_overlay_button_bg(&painter, bulk_rect, bulk_resp.hovered(), false);
        draw_overlay_bulk_bookmark_icon(
            &painter,
            bulk_rect.center(),
            7.5,
            egui::Color32::from_rgb(255, 220, 82),
        );
        let bulk_resp = bulk_resp
            .hover_tip_dark("ブックマークを一括登録 (動画コメント等のチャプター形式の貼り付け)");
        if bulk_resp.clicked() && bulk_bookmark_dialog.is_none() {
            *bulk_bookmark_dialog = Some(NativeBulkBookmarkDialog {
                request_focus: true,
                ..Default::default()
            });
        }
    }

    if opts.show_pin_button {
        let pin_rect = egui::Rect::from_min_size(
            rect.min + egui::vec2(rect.width() - 100.0, 6.0),
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
        let pin_resp = pin_resp.hover_tip_dark(native_label_with_shortcut(
            "現在位置をピン留め",
            shortcut_labels.and_then(|s| s.pin.as_deref()),
        ));
        if pin_resp.clicked() {
            commands.push(NativeOverlayCommand::SetPinAt {
                target_secs: position_secs,
            });
        }
    }

    let bm_rect = egui::Rect::from_min_size(
        rect.min + egui::vec2(rect.width() - 68.0, 6.0),
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
    let bm_resp = bm_resp.hover_tip_dark(native_label_with_shortcut(
        "現在位置をブックマーク",
        shortcut_labels.and_then(|s| s.bookmark.as_deref()),
    ));
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
                    ui.colored_label(egui::Color32::from_gray(170), opts.empty_text);
                });
                return;
            }

            for kind in [
                NativeOverlayTimelineMarkerKind::Pin,
                NativeOverlayTimelineMarkerKind::Bookmark,
                NativeOverlayTimelineMarkerKind::Chapter,
            ] {
                let show_kind = match kind {
                    NativeOverlayTimelineMarkerKind::Pin => opts.show_pins,
                    NativeOverlayTimelineMarkerKind::Bookmark => true,
                    NativeOverlayTimelineMarkerKind::Chapter => opts.show_chapters,
                };
                if !show_kind {
                    continue;
                }
                let section_entries: Vec<_> = entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| entry.kind == kind)
                    .collect();
                if section_entries.is_empty() {
                    continue;
                }
                if opts.show_section_headers {
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
                }
                for (idx, entry) in section_entries {
                    let time_text = format_native_jump_entry_time(entry, entries);
                    draw_native_jump_row(
                        ui,
                        idx,
                        entry,
                        &time_text,
                        opts.show_thumbnails,
                        jump_texture_ids,
                        bookmark_title_edit,
                        commands,
                    );
                }
                ui.add_space(8.0);
            }
        });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_native_jump_row(
    ui: &mut egui::Ui,
    idx: usize,
    entry: &NativeOverlayJumpEntry,
    time_text: &str,
    show_thumbnail: bool,
    jump_texture_ids: &HashMap<usize, egui::TextureId>,
    bookmark_title_edit: &mut Option<NativeBookmarkTitleEdit>,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    // 行レイアウト: タイトルが長いと自動で複数行に折り返し、最大 5 行 (= 約 80pt)
    // まで row を縦に伸ばす。ホバー時はツールチップで全文を表示する。
    // 旧版は固定 76pt + 1 行 truncate (`…`) だったが、PHASE 表記付きチャプター等の
    // 長いタイトルが見切れていた (実機 fb 2026-05-26)。
    //
    // サムネ表示時は左に 120pt のサムネ列があり、テキストはその右 (+136pt) から始まる。
    // 音声ビュー (show_thumbnail=false, Inc 5c-A) はサムネ列が無いので、テキストを左端
    // (+12pt) から始めて行高も詰める。
    let row_h_min: f32 = if show_thumbnail { 76.0 } else { 52.0 };
    let row_w = (ui.available_width() - 12.0).max(260.0);
    let title_color = egui::Color32::from_rgb(205, 205, 205);
    let title_font = egui::FontId::proportional(12.0);
    let title_y_offset = 38.0; // BM ラベル (y +14) の下、サムネ下端 (y +72) より少し上から
    let title_bottom_pad = 6.0;
    let title_max_lines = 5;
    // text_x = (サムネ有) thumb_rect.max.x + 10 = row.min.x + 6 + 120 + 10 = +136
    //          (サムネ無) row.min.x + 12
    // title_max_w = row_rect.max.x - text_x - 6
    let text_x_offset = if show_thumbnail { 136.0 } else { 12.0 };
    let title_max_w = (row_w - text_x_offset - 6.0).max(40.0);
    // タイトルをここで一度 layout してその高さで行の縦サイズを決める。
    // painter 取得のためだけに ui.painter() を借りる (allocate 前の参照は OK)。
    let title_layout = entry
        .title
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|t| {
            layout_wrapped_with_max_lines(
                ui.painter(),
                t,
                title_font.clone(),
                title_color,
                title_max_w,
                title_max_lines,
            )
        });
    let title_h = title_layout
        .as_ref()
        .map(|(g, _)| g.size().y)
        .unwrap_or(0.0);
    let row_h = row_h_min.max(title_y_offset + title_h + title_bottom_pad);
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
        if show_thumbnail {
            let thumb_rect = egui::Rect::from_min_size(
                row_rect.min + egui::vec2(6.0, 4.0),
                egui::vec2(120.0, 68.0),
            );
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
        }

        let text_x = row_rect.min.x + text_x_offset;
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
        // 上で pre-compute した multi-line galley をここで描画する (allocate 前に layout
        // 済みなので row_h が title 高さに追従済み)。長すぎて 5 行で切れたタイトルは
        // ホバー時のツールチップで全文を出す (下の `needs_tooltip` 経路で実装)。
        if let Some((galley, _)) = title_layout.as_ref() {
            painter.galley(
                egui::pos2(text_x, row_rect.min.y + title_y_offset),
                galley.clone(),
                title_color,
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
            let edit_resp = edit_resp.hover_tip_dark("ブックマーク名を編集");
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
            let delete_resp = delete_resp.hover_tip_dark("ブックマークを削除");
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
            let delete_resp = delete_resp.hover_tip_dark("ピン留めを解除");
            if delete_resp.clicked() {
                delete_clicked = true;
                commands.push(NativeOverlayCommand::DeletePin);
            }
        }

        // タイトルが truncate されたか、もしくは複数行に wrap されたケースで、ホバー時に
        // 全文をツールチップで出す。短い 1 行タイトルは行内で全文見えているので tooltip
        // を抑止 (= ノイズ回避)。`was_truncated` は helper 戻り値の bool で判定。
        let needs_tooltip = title_layout
            .as_ref()
            .is_some_and(|(g, was_truncated)| *was_truncated || g.rows.len() > 1);
        let resp = if needs_tooltip {
            if let Some(full_title) = entry.title.as_deref() {
                resp.hover_tip_dark(full_title.to_owned())
            } else {
                resp
            }
        } else {
            resp
        };
        if resp.clicked() && !delete_clicked && !edit_clicked {
            commands.push(NativeOverlayCommand::Seek {
                target_secs: entry.pts_secs,
            });
        }
    });
}

/// 戻り値は実際に描画したダイアログ rect。呼び出し側は `SetWindowRgn` の region に
/// この rect を使う (中央固定の概算 region だとダイアログ上端がクリップされるため)。
pub(crate) fn draw_native_bookmark_title_editor(
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
            // ボタン / TextEdit など ambient テーマ依存ウィジェットをダーク配色で描く。
            // 動画 native overlay は egui 既定 (= ダーク) の ctx なので no-op = バイト等価。
            // 音楽ビューはメイン ctx でアプリテーマが Light だと既定 Light になり、暗い
            // ダイアログ枠に白ボタン/白 TextEdit が乗る不具合になるため明示する (Inc 5c-A FB)。
            *ui.visuals_mut() = egui::Visuals::dark();
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

/// 一括ブックマーク登録ダイアログの (width, height) を overlay サイズから算出する。
///
/// 計算ルール (Codex C6/C10):
/// - **width**: 560pt 目安 / 上限 = overlay_w - 32 / 下限 = 360 (overlay が狭くてもこの幅)
///   ただし overlay_w 自体が下限 360 未満なら overlay_w 全体を使う。
/// - **height**: overlay_h - 64pt 目安 / 上限 720 / 下限 360。
///   ただし overlay_h が 360 未満の場合 overlay_h - 16 で off-screen 防止。
///
/// 実描画と HUD region 計算 (compute_hud_regions) の両方からこの関数を呼び、両者が
/// 食い違って下部ボタンが SetWindowRgn 外に落ちる事故を防ぐ。
pub(crate) fn native_bulk_bookmark_dialog_size(
    overlay_width_points: f32,
    overlay_height_points: f32,
) -> (f32, f32) {
    let max_w = (overlay_width_points - 32.0).max(0.0);
    let dialog_w = if overlay_width_points <= 360.0 {
        overlay_width_points
    } else {
        560.0_f32.min(max_w).max(360.0)
    };
    let dialog_h = if overlay_height_points < 360.0 {
        // 小画面: off-screen 防止。overlay_h - 16 で上下 8pt のマージンを残す。
        (overlay_height_points - 16.0).max(120.0)
    } else {
        ((overlay_height_points - 64.0).min(720.0)).max(360.0)
    };
    (dialog_w, dialog_h)
}

pub(super) fn native_shortcut_help_dialog_size(
    overlay_width_points: f32,
    overlay_height_points: f32,
) -> (f32, f32) {
    let dialog_w = overlay_width_points.min(680.0).max(360.0);
    let dialog_h = if overlay_height_points < 420.0 {
        (overlay_height_points - 16.0).max(160.0)
    } else {
        ((overlay_height_points - 72.0).min(640.0)).max(360.0)
    };
    (dialog_w, dialog_h)
}

pub(super) fn draw_native_shortcut_help_dialog(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    help: &NativeOverlayShortcutHelp,
    open: &mut bool,
) -> Option<egui::Rect> {
    if !*open {
        return None;
    }
    let (dialog_w, dialog_h) =
        native_shortcut_help_dialog_size(overlay_width_points, overlay_height_points);
    let pos = egui::pos2(
        (overlay_width_points - dialog_w) * 0.5,
        (overlay_height_points - dialog_h) * 0.5,
    );
    let mut close_requested = false;
    let scroll_max_h = (dialog_h - 104.0).max(120.0);

    let area_response = egui::Area::new(egui::Id::new("native_video_shortcut_help_dialog"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(18, 18, 24, 244))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(112)))
                .corner_radius(egui::CornerRadius::same(5))
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    ui.set_min_width(dialog_w - 28.0);
                    ui.set_max_width(dialog_w - 28.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("ショートカット")
                                .size(15.0)
                                .color(egui::Color32::from_rgb(238, 238, 238)),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let (close_rect, close_resp) = ui
                                .allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
                            draw_overlay_button_bg(
                                ui.painter(),
                                close_rect,
                                close_resp.hovered(),
                                false,
                            );
                            draw_overlay_close_icon(ui.painter(), close_rect);
                            if close_resp.hover_tip_dark("閉じる").clicked() {
                                close_requested = true;
                            }
                        });
                    });
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("現在のコンテキスト: 動画フルスクリーン")
                            .size(12.0)
                            .color(egui::Color32::from_gray(205)),
                    );
                    ui.add_space(6.0);

                    egui::ScrollArea::vertical()
                        .id_salt("native_video_shortcut_help_scroll")
                        .max_height(scroll_max_h)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for section in &help.sections {
                                if section.rows.is_empty() {
                                    continue;
                                }
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(&section.title)
                                        .strong()
                                        .color(egui::Color32::from_rgb(232, 232, 236)),
                                );
                                ui.add_space(2.0);
                                egui::Grid::new((
                                    "native_video_shortcut_help_section",
                                    section.title.as_str(),
                                ))
                                .num_columns(2)
                                .spacing([18.0, 4.0])
                                .striped(true)
                                .show(ui, |ui| {
                                    for row in &section.rows {
                                        ui.monospace(&row.keys);
                                        ui.label(&row.description);
                                        ui.end_row();
                                    }
                                });
                            }

                            if !help.fixed_rows.is_empty() {
                                ui.add_space(10.0);
                                ui.label(
                                    egui::RichText::new("固定キー")
                                        .strong()
                                        .color(egui::Color32::from_rgb(232, 232, 236)),
                                );
                                ui.add_space(2.0);
                                egui::Grid::new("native_video_shortcut_help_fixed")
                                    .num_columns(2)
                                    .spacing([18.0, 4.0])
                                    .striped(true)
                                    .show(ui, |ui| {
                                        for row in &help.fixed_rows {
                                            ui.monospace(&row.keys);
                                            ui.label(&row.description);
                                            ui.end_row();
                                        }
                                    });
                            }
                        });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);
                    if ui.button("閉じる").clicked() {
                        close_requested = true;
                    }
                });
        });

    if close_requested {
        *open = false;
    }
    Some(area_response.response.rect)
}

/// 一括ブックマーク登録ダイアログ。中央モーダル。
/// 戻り値は実描画 rect (region 計算用)。`None` ならダイアログ非表示。
///
/// レイアウト方針 (2026-05-24 ユーザー報告):
/// - **ダイアログ全体の高さは画面高に対して固定**。長文ペーストで textarea が膨らんでも
///   下部の「登録」「キャンセル」ボタンが画面外に逃げないようにする。
/// - textarea は ScrollArea で囲い、`max_height` 制約をかけて中身がはみ出たら
///   textarea 内側でスクロールさせる。
pub(crate) fn draw_native_bulk_bookmark_dialog(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    // 「現在の{subject_noun}に登録されている…」「この{subject_noun}のブックマークをすべて削除」で
    // 使う対象メディアの名詞。動画は "動画" (従来どおり = バイト等価)、音楽ビューは "音声"。
    subject_noun: &str,
    dialog: &mut Option<NativeBulkBookmarkDialog>,
    commands: &mut Vec<NativeOverlayCommand>,
) -> Option<egui::Rect> {
    let Some(state) = dialog.as_mut() else {
        return None;
    };
    // Ctrl+V で押されたテキストを focus 状態に依存せず textarea へ取り込む (Codex C8)。
    // 末尾追記方式: 連続貼り付け時は改行で区切る。
    if let Some(text) = state.pending_paste.take() {
        if !state.textarea.is_empty() && !state.textarea.ends_with('\n') {
            state.textarea.push('\n');
        }
        state.textarea.push_str(&text);
    }
    let (dialog_w, dialog_h) =
        native_bulk_bookmark_dialog_size(overlay_width_points, overlay_height_points);
    let pos = egui::pos2(
        (overlay_width_points - dialog_w) * 0.5,
        (overlay_height_points - dialog_h) * 0.5,
    );

    let (entries, errors) = crate::video_bookmarks_parser::parse_chapter_text(&state.textarea);
    let entry_count = entries.len();
    let error_count = errors.len();

    let mut register = false;
    let mut cancel = false;
    let mut request_clear_all = false;
    let mut confirm_clear_now = false;
    let mut request_export = false;

    // textarea ScrollArea の最大高さ: ダイアログ高から固定要素 (header / footer / frame
    // margin) を引いた残り。固定要素は実測値ベースの保守的見積もり。
    // 内訳: タイトル 22 + 説明文 3行 約 50 + textarea 上下スペース 16 + プレビュー 22 +
    //       登録/キャンセル行 32 + separator+「現在のブックマーク」+ コピー+チェックボックス行 約 70 +
    //       separator+「誤登録対策」+ ボタン 約 70 + frame margin 28 ≒ 310。
    // 余裕を見て 340。これで残りが textarea に割り当てられる。
    let textarea_max_h = (dialog_h - 340.0).max(80.0);

    let area_response = egui::Area::new(egui::Id::new("native_video_bulk_bookmark_dialog"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            // ボタン / TextEdit / checkbox など ambient テーマ依存ウィジェットをダーク配色で
            // 描く。動画は egui 既定 (ダーク) ctx なので no-op = バイト等価。音楽ビューは
            // メイン ctx で Light テーマだと白ウィジェットになるため明示する (Inc 5c-A FB)。
            *ui.visuals_mut() = egui::Visuals::dark();
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(18, 18, 24, 244))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(112)))
                .corner_radius(egui::CornerRadius::same(5))
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    ui.set_min_width(dialog_w - 28.0);
                    ui.set_max_width(dialog_w - 28.0);
                    // タイトル行: 左にタイトル、右端に × クローズボタン。
                    // 「キャンセル」ボタンが「登録」のすぐ隣にあって全体終了との結び付きが
                    // 弱いという報告 (2026-05-24) に対し、ダイアログ全体を閉じる統一動作を
                    // 右上に置く。エクスポートや一括削除を済ませた後の「閉じ方が分からない」
                    // 状態を解消する目的。
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("ブックマーク一括登録")
                                .size(15.0)
                                .color(egui::Color32::from_rgb(238, 238, 238)),
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                let (close_rect, close_resp) = ui.allocate_exact_size(
                                    egui::vec2(22.0, 22.0),
                                    egui::Sense::click(),
                                );
                                draw_overlay_button_bg(
                                    ui.painter(),
                                    close_rect,
                                    close_resp.hovered(),
                                    false,
                                );
                                draw_overlay_close_icon(ui.painter(), close_rect);
                                if close_resp.hover_tip_dark("閉じる").clicked() {
                                    cancel = true;
                                }
                            },
                        );
                    });
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(
                            "1 行 1 件、「hh:mm:ss タイトル」または「mm:ss タイトル」形式で\n\
                             貼り付けると一括登録できます (動画コメント等のチャプター記法に対応)。\n\
                             既存のブックマークと時刻が ±1 秒以内の行は重複として skip します。",
                        )
                        .size(11.5)
                        .color(egui::Color32::from_gray(190)),
                    );
                    ui.add_space(8.0);

                    // textarea を ScrollArea で囲い、長文ペースト時の高さ膨張を抑える。
                    // `desired_rows(6)` は ScrollArea 内の初期高さ目安 (= 中身が増えれば
                    // TextEdit 自体は高くなるが、ScrollArea の `max_height` でクリップ +
                    // 内部スクロールさせる)。
                    //
                    // `desired_width` は有限値 (dialog 内幅) を渡す。`f32::INFINITY` を渡すと
                    // `ScrollArea::vertical()` (= horizontal は scroll しない設計) と組み合
                    // わせたとき layout が壊れて TextEdit のクリック判定が崩れ、結果として
                    // フォーカスが取れず Ctrl+V (Event::Paste) が無視される (egui の TextEdit
                    // は `has_focus(id)` のときだけ Paste を処理する仕様、2026-05-24 ユーザー報告)。
                    egui::ScrollArea::vertical()
                        .id_salt("bulk_bookmark_textarea_scroll")
                        .max_height(textarea_max_h)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            // スクロールバー (~16pt) の上に textarea 右端が乗らないよう
                            // 18pt を予約してから desired_width を決める (Codex C13)。
                            let inner_w = (dialog_w - 28.0 - 18.0).max(120.0);
                            let response = ui.add(
                                egui::TextEdit::multiline(&mut state.textarea)
                                    .desired_width(inner_w)
                                    .desired_rows(6)
                                    .hint_text(
                                        "例:\n0:13 メインテーマ\n2:13 希望に満ちるアナザーデイ\n1:00:08 魏々たる丹砂",
                                    ),
                            );
                            if state.request_focus {
                                response.request_focus();
                                state.request_focus = false;
                            }
                            // ダイアログ表示中の focus 自動救済。
                            // Codex C7 反映: 確認削除モード (confirm_clear_all=true) のときは
                            // 救済を **抑止** する: 「削除を実行」/「やめる」ボタンに focus を
                            // 渡したいので、Tab / Enter / マウスクリックで一時的に focus が
                            // 離れても textarea へ戻さない。
                            // Codex C8 反映: Ctrl+V の first-paste race は `pending_paste` の
                            // direct-write 経路 (push_native_event 側) で吸収済みなので、
                            // ここでの focus 救済は「Enter キー / Tab 後の textarea 入力継続」
                            // 程度の役割。
                            if !state.confirm_clear_all {
                                let any_focus =
                                    ui.ctx().memory(|m| m.focused().is_some());
                                if !any_focus {
                                    response.request_focus();
                                }
                            }
                        });

                    ui.add_space(6.0);
                    // プレビュー (パース結果の件数 + エラー行の通知)。
                    let preview_text = if state.textarea.trim().is_empty() {
                        "貼り付け待ち".to_string()
                    } else if error_count == 0 {
                        format!("解釈成功: {entry_count} 件")
                    } else {
                        format!(
                            "解釈成功: {entry_count} 件 / 解釈できなかった行: {error_count} 件 (行番号: {})",
                            errors
                                .iter()
                                .take(8)
                                .map(|n| n.to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    };
                    let preview_color = if error_count == 0 {
                        egui::Color32::from_rgb(170, 220, 170)
                    } else {
                        egui::Color32::from_rgb(240, 200, 130)
                    };
                    ui.colored_label(preview_color, egui::RichText::new(preview_text).size(11.5));
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        let register_enabled = entry_count > 0;
                        let register_btn = egui::Button::new(
                            egui::RichText::new(format!("登録 ({entry_count} 件)")).size(13.0),
                        );
                        let register_resp = ui.add_enabled(register_enabled, register_btn);
                        if register_resp.clicked() {
                            register = true;
                        }
                        if ui.button("キャンセル").clicked() {
                            cancel = true;
                        }
                    });

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("現在のブックマーク一覧をエクスポート")
                            .size(12.0)
                            .color(egui::Color32::from_gray(200)),
                    );
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button("一覧をクリップボードにコピー")
                            .on_hover_text(format!(
                                "現在の{subject_noun}に登録されているブックマークを「mm:ss タイトル」\n\
                                 形式の行ごとにクリップボードへコピーします。"
                            ))
                            .clicked()
                        {
                            request_export = true;
                        }
                        ui.checkbox(&mut state.export_seconds_only, "秒単位にする")
                            .on_hover_text(
                                "ON: 整数秒に切り捨てます (動画コメント欄でリンク化される形式)。\n\
                                 OFF: 小数 3 桁 (ミリ秒精度) で書き出します。",
                            );
                    });

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("誤登録対策")
                            .size(12.0)
                            .color(egui::Color32::from_gray(200)),
                    );
                    ui.add_space(4.0);
                    if !state.confirm_clear_all {
                        let resp =
                            ui.button(format!("この{subject_noun}のブックマークをすべて削除"));
                        if resp.clicked() {
                            request_clear_all = true;
                        }
                    } else {
                        ui.colored_label(
                            egui::Color32::from_rgb(250, 170, 130),
                            egui::RichText::new("本当にすべて削除しますか? (取り消せません)")
                                .size(12.0),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.button("削除を実行").clicked() {
                                confirm_clear_now = true;
                            }
                            if ui.button("やめる").clicked() {
                                state.confirm_clear_all = false;
                            }
                        });
                    }
                });
        });

    if request_clear_all {
        state.confirm_clear_all = true;
    }
    if request_export {
        // エクスポートはダイアログを閉じずに発火 (チェックボックスを切り替えて再実行できる)。
        commands.push(NativeOverlayCommand::ExportBookmarksToClipboard {
            seconds_only: state.export_seconds_only,
        });
    }
    if confirm_clear_now {
        commands.push(NativeOverlayCommand::ClearAllBookmarksForCurrent);
        // ダイアログを閉じる (Codex C9): textarea にまだ貼り付けた行が残っていると、
        // 直後の誤クリック「登録」で削除した分を再登録してしまうため、削除確定時は
        // 状態ごと破棄して dialog を閉じる。再度開けば textarea も初期化される。
        *dialog = None;
    } else if register {
        let entry_tuples: Vec<(f64, String)> =
            entries.into_iter().map(|e| (e.pts_secs, e.title)).collect();
        commands.push(NativeOverlayCommand::BulkAddBookmarks {
            entries: entry_tuples,
        });
        *dialog = None;
    } else if cancel {
        *dialog = None;
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
    WindowToggle,
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
        NativeTopButtonGlyph::WindowToggle => draw_overlay_window_toggle_icon(painter, rect),
    }
    let resp = resp.hover_tip_dark(tooltip);
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
    let resp = resp.hover_tip_dark(tooltip);
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

/// 動画 HUD 2 段化リデザイン (Phase 4): 前/次マーカーへスキップするアイコン。
/// `direction < 0` で `|◀` (前マーカー、prev)、`direction > 0` で `▶|` (次マーカー、next)。
/// 縦バー + 三角の組み合わせ。CD プレイヤーや YouTube のチャプター移動慣習に合わせる。
/// `enabled=false` (= マーカー無し) のときは半透明グレーで描く。
pub(crate) fn draw_overlay_skip_to_marker_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    direction: i32,
    enabled: bool,
) {
    let color = if enabled {
        egui::Color32::from_rgb(238, 238, 238)
    } else {
        egui::Color32::from_rgba_unmultiplied(238, 238, 238, 90)
    };
    let stroke = egui::Stroke::new(1.8, color);
    let c = rect.center();
    let sign = if direction < 0 { -1.0 } else { 1.0 };
    // 縦バー: 進行方向側 (prev なら左、next なら右)
    let bar_x = c.x + sign * 7.0;
    painter.line_segment(
        [egui::pos2(bar_x, c.y - 8.0), egui::pos2(bar_x, c.y + 8.0)],
        stroke,
    );
    // 三角: バーへ向かう向き (prev なら左向き ◀、next なら右向き ▶)
    // バーから 2pt 内側に三角の先端を寄せて視覚的にくっつける
    let tip_x = c.x + sign * 5.0;
    let base_x = c.x - sign * 5.0;
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(tip_x, c.y),
            egui::pos2(base_x, c.y - 6.0),
            egui::pos2(base_x, c.y + 6.0),
        ],
        color,
        egui::Stroke::NONE,
    ));
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

/// 動画 HUD 2 段化リデザイン (Phase 6): 前/次ファイル (前/次項目) ボタン用の単純矢印アイコン。
/// `direction < 0` で上向き ↑ (= 前項目)、`direction > 0` で下向き ↓ (= 次項目)。
/// ファイル切替は上下キーに対応する操作なので、左右矢印 (= シーク操作の慣習) ではなく
/// 上下三角を使う。同じ HUD 行に並ぶ `|◀ ▶|` (skip marker) や `◀ ▶` (frame step) との
/// 衝突を避けつつ、「上=前、下=次」のキーボード規約と一致させる。
pub(super) fn draw_overlay_arrow_icon(painter: &egui::Painter, rect: egui::Rect, direction: i32) {
    let color = egui::Color32::from_rgb(238, 238, 238);
    let c = rect.center();
    let sign = if direction < 0 { -1.0 } else { 1.0 };
    // 三角: 進行方向側 (prev=上向き、next=下向き)
    let tip_y = c.y + sign * 7.0;
    let base_y = c.y - sign * 4.0;
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(c.x, tip_y),
            egui::pos2(c.x - 6.0, base_y),
            egui::pos2(c.x + 6.0, base_y),
        ],
        color,
        egui::Stroke::NONE,
    ));
    // 軸線 (アイテム列を示唆): 三角の反対側に短い縦線
    let stroke = egui::Stroke::new(1.8, color);
    let line_far = c.y - sign * 8.0;
    painter.line_segment([egui::pos2(c.x, base_y), egui::pos2(c.x, line_far)], stroke);
}

/// 動画 HUD 2 段化リデザイン (Phase 5): キャプチャパレットの「ファイル保存」ボタン用アイコン。
/// 単純な下向き矢印 + ベースライン (ファイル保存の universal アイコン)。
/// フロッピーディスクはレトロすぎる + camera との視覚的衝突を避けるため使わない。
pub(super) fn draw_overlay_save_icon(painter: &egui::Painter, rect: egui::Rect) {
    let color = egui::Color32::from_rgb(238, 238, 238);
    let stroke = egui::Stroke::new(1.8, color);
    let c = rect.center();
    // 縦線 (矢印の軸): center 上下に伸びる
    painter.line_segment(
        [egui::pos2(c.x, c.y - 7.0), egui::pos2(c.x, c.y + 3.0)],
        stroke,
    );
    // 矢印頭 (下向き三角): 線の下端に
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(c.x, c.y + 6.0),
            egui::pos2(c.x - 4.5, c.y + 0.0),
            egui::pos2(c.x + 4.5, c.y + 0.0),
        ],
        color,
        egui::Stroke::NONE,
    ));
    // ベースライン (ファイルを示す横線): 矢印の下に
    painter.line_segment(
        [
            egui::pos2(c.x - 7.0, c.y + 9.0),
            egui::pos2(c.x + 7.0, c.y + 9.0),
        ],
        stroke,
    );
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

/// ウィンドウ / 全画面 切り替えボタンのアイコン。タイトルバー付きの矩形 (= 一般的な
/// 「ウィンドウ」表現) を線で描く。トグルなので状態非依存の固定アイコン。
pub(super) fn draw_overlay_window_toggle_icon(painter: &egui::Painter, rect: egui::Rect) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) * 0.30;
    let stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(242, 242, 242));
    let win = egui::Rect::from_center_size(c, egui::vec2(s * 2.0, s * 1.7));
    painter.line_segment([win.left_top(), win.right_top()], stroke);
    painter.line_segment([win.right_top(), win.right_bottom()], stroke);
    painter.line_segment([win.right_bottom(), win.left_bottom()], stroke);
    painter.line_segment([win.left_bottom(), win.left_top()], stroke);
    let title_y = win.top() + s * 0.55;
    painter.line_segment(
        [
            egui::pos2(win.left(), title_y),
            egui::pos2(win.right(), title_y),
        ],
        stroke,
    );
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
        // Passive status only. Keeping the full-screen Area interactable makes
        // broken-video error/preparing overlays consume right-click release
        // events before the fullscreen close handler can see them.
        .interactable(false)
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

pub(super) fn native_ring_guide_overlay_rect(
    overlay_width_points: f32,
    overlay_height_points: f32,
    pixels_per_point: f32,
    guide: &NativeOverlayRingGuide,
) -> egui::Rect {
    let margin = 16.0;
    let usable_w = (overlay_width_points - margin * 2.0).max(120.0);
    let usable_h = (overlay_height_points - margin * 2.0).max(120.0);
    let radius = ring_guide_radius(overlay_width_points, overlay_height_points);
    let size = ((radius + 116.0) * 2.0).min(usable_w).min(usable_h);
    let center = guide.center_client_px.map_or_else(
        || egui::pos2(overlay_width_points * 0.5, overlay_height_points * 0.5),
        |pos| {
            let ppp = pixels_per_point.max(1.0);
            egui::pos2(pos.x / ppp, pos.y / ppp)
        },
    );
    egui::Rect::from_center_size(center, egui::vec2(size, size))
}

pub(super) fn draw_native_ring_guide_overlay(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    pixels_per_point: f32,
    guide: &NativeOverlayRingGuide,
) -> Option<egui::Rect> {
    let outer_rect = native_ring_guide_overlay_rect(
        overlay_width_points,
        overlay_height_points,
        pixels_per_point,
        guide,
    );
    let radius = ((outer_rect.width().min(outer_rect.height()) - 224.0) * 0.5).clamp(120.0, 164.0);
    let area_response = egui::Area::new(egui::Id::new("native_video_ring_guide_overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(outer_rect.min)
        .interactable(false)
        .show(ctx, |ui| {
            ui.set_min_size(outer_rect.size());
            let panel_rect = egui::Rect::from_min_size(ui.min_rect().min, outer_rect.size());
            let center = panel_rect.center();
            let painter = ui.painter();
            let inner_radius = 62.0_f32;
            let outer_radius = radius + 46.0;
            let label_radius = (inner_radius + outer_radius) * 0.5;
            for (idx, unit) in native_ring_direction_units().iter().enumerate() {
                let is_selected = guide.selected_slot == Some(idx);
                let fill = if is_selected {
                    egui::Color32::from_rgba_unmultiplied(72, 126, 190, 218)
                } else {
                    egui::Color32::from_black_alpha(118)
                };
                let stroke = if is_selected {
                    egui::Stroke::new(2.0, egui::Color32::WHITE)
                } else {
                    egui::Stroke::new(1.0, egui::Color32::from_white_alpha(120))
                };
                draw_native_annular_segment(
                    painter,
                    center,
                    inner_radius,
                    outer_radius,
                    idx,
                    fill,
                    stroke,
                );
                let action_label = guide
                    .slots
                    .get(idx)
                    .map(|slot| slot.action_label.as_str())
                    .unwrap_or("");
                draw_native_ring_segment_label(
                    painter,
                    center + *unit * label_radius,
                    &truncate_overlay_text(action_label, 11),
                    is_selected,
                );
            }
        });

    Some(area_response.response.rect)
}

fn ring_guide_radius(overlay_width_points: f32, overlay_height_points: f32) -> f32 {
    (overlay_width_points.min(overlay_height_points) * 0.20).clamp(144.0, 164.0)
}

fn draw_native_annular_segment(
    painter: &egui::Painter,
    center: egui::Pos2,
    inner_radius: f32,
    outer_radius: f32,
    idx: usize,
    fill: egui::Color32,
    stroke: egui::Stroke,
) {
    let mid = native_ring_direction_angle_rad(idx);
    let half = std::f32::consts::PI / 8.0;
    let start = mid - half;
    let end = mid + half;
    let steps = 8;
    let mut outer_points = Vec::with_capacity(steps + 1);
    let mut inner_points = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = start + (end - start) * i as f32 / steps as f32;
        outer_points.push(native_ring_point(center, outer_radius, t));
        inner_points.push(native_ring_point(center, inner_radius, t));
    }
    for i in 0..steps {
        painter.add(egui::Shape::convex_polygon(
            vec![
                outer_points[i],
                outer_points[i + 1],
                inner_points[i + 1],
                inner_points[i],
            ],
            fill,
            egui::Stroke::NONE,
        ));
    }
    painter.add(egui::Shape::line(outer_points.clone(), stroke));
    painter.add(egui::Shape::line(inner_points.clone(), stroke));
    painter.line_segment([outer_points[0], inner_points[0]], stroke);
    painter.line_segment(
        [*outer_points.last().unwrap(), *inner_points.last().unwrap()],
        stroke,
    );
}

fn draw_native_ring_segment_label(
    painter: &egui::Painter,
    center: egui::Pos2,
    text: &str,
    selected: bool,
) {
    let font = egui::FontId::proportional(if selected { 14.5 } else { 13.5 });
    let color = egui::Color32::WHITE;
    let galley = painter.layout_no_wrap(text.to_string(), font, color);
    let padding = egui::vec2(8.0, 4.0);
    let rect = egui::Rect::from_center_size(center, galley.size() + padding * 2.0);
    painter.rect_filled(
        rect,
        4.0,
        if selected {
            egui::Color32::from_rgba_unmultiplied(22, 44, 72, 232)
        } else {
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 214)
        },
    );
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(
            if selected { 1.4 } else { 1.0 },
            egui::Color32::from_white_alpha(if selected { 210 } else { 150 }),
        ),
        egui::StrokeKind::Outside,
    );
    painter.galley(rect.min + padding, galley, color);
}

fn native_ring_direction_angle_rad(idx: usize) -> f32 {
    match idx {
        0 => -std::f32::consts::FRAC_PI_2,
        1 => -std::f32::consts::FRAC_PI_4,
        2 => 0.0,
        3 => std::f32::consts::FRAC_PI_4,
        4 => std::f32::consts::FRAC_PI_2,
        5 => std::f32::consts::FRAC_PI_4 * 3.0,
        6 => std::f32::consts::PI,
        _ => -std::f32::consts::FRAC_PI_4 * 3.0,
    }
}

fn native_ring_point(center: egui::Pos2, radius: f32, angle: f32) -> egui::Pos2 {
    center + egui::vec2(angle.cos() * radius, angle.sin() * radius)
}

fn native_ring_direction_units() -> &'static [egui::Vec2; 8] {
    const D: f32 = std::f32::consts::FRAC_1_SQRT_2;
    const UNITS: [egui::Vec2; 8] = [
        egui::Vec2::new(0.0, -1.0),
        egui::Vec2::new(D, -D),
        egui::Vec2::new(1.0, 0.0),
        egui::Vec2::new(D, D),
        egui::Vec2::new(0.0, 1.0),
        egui::Vec2::new(-D, D),
        egui::Vec2::new(-1.0, 0.0),
        egui::Vec2::new(-D, -D),
    ];
    &UNITS
}

#[cfg(test)]
mod ring_guide_tests {
    use super::*;

    #[test]
    fn native_ring_guide_center_converts_client_pixels_to_points() {
        let guide = NativeOverlayRingGuide {
            heading: String::new(),
            detail: String::new(),
            selected_slot: None,
            center_client_px: Some(egui::pos2(300.0, 150.0)),
            slots: Vec::new(),
        };

        let rect = native_ring_guide_overlay_rect(800.0, 600.0, 1.5, &guide);

        assert!((rect.center().x - 200.0).abs() < 0.01, "{rect:?}");
        assert!((rect.center().y - 100.0).abs() < 0.01, "{rect:?}");
    }
}

pub(super) fn native_ring_picker_overlay_rect(
    overlay_width_points: f32,
    overlay_height_points: f32,
    picker: &NativeOverlayRingPicker,
) -> egui::Rect {
    let margin = 16.0;
    let usable_w = (overlay_width_points - margin * 2.0).max(120.0);
    let usable_h = (overlay_height_points - margin * 2.0).max(120.0);
    let row_h = 32.0;
    let desired_w = (overlay_width_points * 0.60).clamp(340.0, 560.0);
    let panel_w = desired_w.min(usable_w);
    let desired_h = if let Some(drill) = picker.drill.as_ref() {
        104.0 + row_h * drill.items.len().max(1) as f32
    } else {
        96.0 + row_h * picker.rows.len() as f32
    };
    let panel_h = desired_h.min(usable_h);
    egui::Rect::from_center_size(
        egui::pos2(overlay_width_points * 0.5, overlay_height_points * 0.5),
        egui::vec2(panel_w, panel_h),
    )
}

pub(super) fn draw_native_ring_picker_overlay(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    picker: &NativeOverlayRingPicker,
) -> Option<egui::Rect> {
    let panel_rect =
        native_ring_picker_overlay_rect(overlay_width_points, overlay_height_points, picker);
    let selected_row = picker
        .selected_row
        .map(|row| row.min(picker.rows.len().saturating_sub(1)));

    let area_response = egui::Area::new(egui::Id::new("native_video_ring_picker_overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(panel_rect.min)
        .interactable(false)
        .show(ctx, |ui| {
            ui.set_min_size(panel_rect.size());
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 224))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 90),
                ))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    let inner_w = (panel_rect.width() - 28.0).max(1.0);
                    ui.set_width(inner_w);
                    ui.label(
                        egui::RichText::new(truncate_overlay_text(&picker.title, 36))
                            .size(17.0)
                            .color(egui::Color32::WHITE),
                    );
                    ui.add_space(6.0);
                    if let Some(drill) = picker.drill.as_ref() {
                        ui.label(
                            egui::RichText::new(truncate_overlay_text(&drill.title, 36))
                                .size(14.0)
                                .color(egui::Color32::from_white_alpha(200)),
                        );
                        ui.add_space(8.0);
                        let row_h = 32.0;
                        let selected = drill.selected.min(drill.items.len().saturating_sub(1));
                        for (idx, item) in drill.items.iter().enumerate() {
                            let is_selected = idx == selected;
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), row_h),
                                egui::Sense::hover(),
                            );
                            ui.painter().rect_filled(
                                rect,
                                5.0,
                                if is_selected {
                                    egui::Color32::from_rgb(56, 94, 138)
                                } else {
                                    egui::Color32::TRANSPARENT
                                },
                            );
                            ui.painter().text(
                                egui::pos2(rect.min.x + 10.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                truncate_overlay_text(item, 44),
                                egui::FontId::proportional(if is_selected { 15.5 } else { 14.5 }),
                                egui::Color32::from_white_alpha(if is_selected {
                                    255
                                } else {
                                    215
                                }),
                            );
                        }
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(truncate_overlay_text(&drill.footer, 54))
                                .size(12.0)
                                .color(egui::Color32::from_white_alpha(190)),
                        );
                    } else {
                        let row_h = 32.0;
                        let available_h = (panel_rect.height() - 96.0).max(row_h);
                        let visible_rows = ((available_h / row_h).floor() as usize)
                            .max(1)
                            .min(picker.rows.len().max(1));
                        let focus_row = selected_row.unwrap_or(0);
                        let start = focus_row
                            .saturating_sub(visible_rows / 2)
                            .min(picker.rows.len().saturating_sub(visible_rows));
                        let end = (start + visible_rows).min(picker.rows.len());
                        let mut first_row_rect = None;
                        let mut last_row_rect = None;
                        let has_scrollbar = picker.rows.len() > visible_rows;
                        let scrollbar_gutter = if has_scrollbar { 18.0 } else { 0.0 };
                        for (idx, row) in picker.rows.iter().enumerate().take(end).skip(start) {
                            let selected = selected_row == Some(idx);
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), row_h),
                                egui::Sense::hover(),
                            );
                            first_row_rect.get_or_insert(rect);
                            last_row_rect = Some(rect);
                            let row_rect = egui::Rect::from_min_max(
                                rect.min,
                                egui::pos2(
                                    (rect.max.x - scrollbar_gutter).max(rect.min.x),
                                    rect.max.y,
                                ),
                            );
                            let fill = if selected {
                                egui::Color32::from_rgb(56, 94, 138)
                            } else {
                                egui::Color32::TRANSPARENT
                            };
                            ui.painter().rect_filled(row_rect, 5.0, fill);
                            let label_pos = egui::pos2(row_rect.min.x + 10.0, row_rect.center().y);
                            let value_pos = egui::pos2(row_rect.max.x - 10.0, row_rect.center().y);
                            ui.painter().text(
                                label_pos,
                                egui::Align2::LEFT_CENTER,
                                truncate_overlay_text(&row.label, 16),
                                egui::FontId::proportional(14.5),
                                egui::Color32::from_white_alpha(if selected { 245 } else { 205 }),
                            );
                            ui.painter().text(
                                value_pos,
                                egui::Align2::RIGHT_CENTER,
                                truncate_overlay_text(&row.value, 32),
                                egui::FontId::proportional(14.5),
                                egui::Color32::WHITE,
                            );
                        }
                        if let (Some(first), Some(last)) = (first_row_rect, last_row_rect) {
                            draw_native_overlay_scrollbar(
                                ui.painter(),
                                egui::Rect::from_min_max(first.min, last.max),
                                picker.rows.len(),
                                visible_rows,
                                start,
                            );
                        }
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(truncate_overlay_text(&picker.footer, 54))
                                .size(12.0)
                                .color(egui::Color32::from_white_alpha(190)),
                        );
                    }
                });
        });

    Some(area_response.response.rect)
}

fn draw_native_overlay_scrollbar(
    painter: &egui::Painter,
    rows_rect: egui::Rect,
    total_rows: usize,
    visible_rows: usize,
    scroll_top: usize,
) {
    if total_rows <= visible_rows || visible_rows == 0 {
        return;
    }
    let track = egui::Rect::from_min_max(
        egui::pos2(rows_rect.max.x - 8.0, rows_rect.min.y + 4.0),
        egui::pos2(rows_rect.max.x - 3.0, rows_rect.max.y - 4.0),
    );
    if track.height() <= 4.0 {
        return;
    }
    painter.rect_filled(track, 2.5, egui::Color32::from_white_alpha(48));
    let ratio = visible_rows as f32 / total_rows as f32;
    let thumb_h = (track.height() * ratio).clamp(18.0, track.height());
    let max_scroll = total_rows.saturating_sub(visible_rows).max(1) as f32;
    let t = (scroll_top as f32 / max_scroll).clamp(0.0, 1.0);
    let thumb_top = track.min.y + (track.height() - thumb_h) * t;
    let thumb = egui::Rect::from_min_max(
        egui::pos2(track.min.x, thumb_top),
        egui::pos2(track.max.x, thumb_top + thumb_h),
    );
    painter.rect_filled(thumb, 2.5, egui::Color32::from_white_alpha(180));
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
            let cancel_resp = cancel_resp.hover_tip_dark("キャンセル [ESC]");
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
    fallback_file_name: &str,
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
            let fallback = fallback_file_name.trim();
            let name = metadata
                .and_then(|m| {
                    m.title
                        .as_ref()
                        .map(String::as_str)
                        .filter(|title| !title.trim().is_empty())
                        .or_else(|| {
                            let file_name = m.file_name.as_str();
                            (!file_name.trim().is_empty()).then_some(file_name)
                        })
                })
                .unwrap_or_else(|| {
                    if fallback.is_empty() {
                        "video"
                    } else {
                        fallback
                    }
                });
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
            let shortcuts = metadata.map(|m| &m.shortcuts);

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
                "native_top_window_toggle",
                NativeTopButtonGlyph::WindowToggle,
                false,
                &native_label_with_shortcut(
                    "ウィンドウ / 全画面 切り替え",
                    shortcuts.and_then(|s| s.window_mode.as_deref()),
                ),
                NativeOverlayCommand::ToggleWindowMode,
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
                &native_label_with_shortcut(
                    "サムネイル一覧",
                    shortcuts.and_then(|s| s.tile_mode.as_deref()),
                ),
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
                &native_label_with_shortcut(
                    "Perfグラフ",
                    shortcuts.and_then(|s| s.perf_overlay.as_deref()),
                ),
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
            let shortcuts = metadata.map(|m| &m.shortcuts);
            let return_shortcut = native_joined_shortcuts(&[
                shortcuts.and_then(|s| s.tile_mode.as_deref()),
                Some("Esc"),
            ]);

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
                &native_label_with_shortcut("動画に戻る", return_shortcut.as_deref()),
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
            // サムネイルがあるときだけ黒背景で埋めてスワップ中の中間フレームを隠す。
            // サムネイル無し (resume サムネ未キャッシュの新規動画など) で黒く塗ると、
            // 動画→動画ナビの間じゅう全画面が真っ黒に点滅して見えるだけなので塗らない。
            // その場合は presenter の video visual (スワップ中は直前フレーム) を透かす。
            let has_thumbnail = preview_texture_id.is_some() && preview.thumbnail.is_some();
            if has_thumbnail {
                painter.rect_filled(full_rect, 0.0, egui::Color32::BLACK);
            }
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
                        .hover_tip_dark("動画をフルスクリーン全体に表示します")
                        .clicked()
                    {
                        commands.push(NativeOverlayCommand::SetVst3VideoCompact { compact: false });
                    }
                    let compact_resp = ui.selectable_label(panel.video_compact, "右上 1/4");
                    if compact_resp
                        .hover_tip_dark("動画を右上 1/4 に縮小し、プラグイン GUI の領域を空けます")
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
                            .hover_tip_dark(native_vst3_chain_slot_tooltip(chain));
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
                            .hover_tip_dark(native_vst3_chain_slot_tooltip(chain));
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
        if checkbox.hover_tip_dark("ON/OFF を切り替えます").changed() {
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
                .hover_tip_dark(if slot.gui_visible {
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

fn draw_native_tag_panel(
    ui: &mut egui::Ui,
    metadata: &NativeOverlayMetadata,
    tag_picker_open: &mut bool,
    tag_picker_input: &mut String,
    tag_picker_focus_request: &mut bool,
    tag_picker_recent_tab: &mut bool,
    _sticky_item_key: &mut Option<String>,
    sticky_tags: &mut Vec<NativeOverlayTagDef>,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    let visible_tags = native_visible_tag_choices(metadata, sticky_tags);

    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(
            egui::RichText::new("タグ")
                .color(egui::Color32::WHITE)
                .size(13.0)
                .strong(),
        );
        let plus_label = if *tag_picker_open { "×" } else { "＋" };
        let plus_resp = ui
            .small_button(plus_label)
            .on_hover_text(if *tag_picker_open {
                "タグ入力を閉じる"
            } else {
                "タグを検索/入力して付ける"
            });
        if plus_resp.clicked() {
            *tag_picker_open = !*tag_picker_open;
            if *tag_picker_open {
                tag_picker_input.clear();
                *tag_picker_focus_request = true;
                *tag_picker_recent_tab = false;
            } else {
                tag_picker_input.clear();
                *tag_picker_focus_request = false;
                *tag_picker_recent_tab = false;
            }
        }
    });
    ui.add_space(4.0);

    if !visible_tags.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.add_space(14.0);
            for def in &visible_tags {
                let with_hash = format!("#{}", def.name);
                let is_on = native_tag_is_on(&metadata.current_tags, &def.tag_key);
                let (text_color, fill_color, stroke_color) = native_tag_button_visuals(is_on);
                let label = egui::RichText::new(&with_hash).color(text_color).strong();
                let button = egui::Button::new(label)
                    .fill(fill_color)
                    .stroke(egui::Stroke::new(1.15, stroke_color));
                let resp = ui.add(button);
                let resp = resp.on_hover_text(if is_on {
                    format!("クリックで `{with_hash}` を削除")
                } else {
                    format!("クリックで `{with_hash}` を付与")
                });
                let clicked = resp.clicked();
                resp.context_menu(|ui| {
                    if ui.button("このタグで探す").clicked() {
                        commands.push(NativeOverlayCommand::OpenTagViewForTag {
                            name: def.name.clone(),
                        });
                        ui.close();
                    }
                });
                if clicked {
                    commands.push(NativeOverlayCommand::ToggleTag {
                        name: def.name.clone(),
                    });
                }
            }
        });
    } else {
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new("（タグなし）")
                    .size(11.0)
                    .color(egui::Color32::from_gray(150)),
            );
        });
    }
}

fn sync_native_tag_sticky(
    metadata: &NativeOverlayMetadata,
    sticky_item_key: &mut Option<String>,
    sticky_tags: &mut Vec<NativeOverlayTagDef>,
) -> bool {
    let changed = sticky_item_key.as_deref() != Some(metadata.item_key.as_str());
    if changed {
        *sticky_item_key = Some(metadata.item_key.clone());
        sticky_tags.clear();
    }
    for tag in &metadata.current_tags {
        let tag_key = crate::tags_db::normalize_tag_key(tag);
        if tag_key.is_empty()
            || metadata
                .shortcut_tags
                .iter()
                .any(|def| def.tag_key == tag_key)
            || sticky_tags.iter().any(|def| def.tag_key == tag_key)
        {
            continue;
        }
        sticky_tags.push(NativeOverlayTagDef {
            name: native_tag_display_name(metadata, &tag_key, tag),
            tag_key,
            count: 0,
            pinned: false,
            last_applied_at: 0,
        });
    }
    changed
}

fn native_visible_tag_choices(
    metadata: &NativeOverlayMetadata,
    sticky_tags: &[NativeOverlayTagDef],
) -> Vec<NativeOverlayTagDef> {
    let mut out = Vec::new();
    let mut seen = Vec::<String>::new();
    for def in metadata.shortcut_tags.iter() {
        if !def.tag_key.is_empty() && !seen.iter().any(|key| key == &def.tag_key) {
            seen.push(def.tag_key.clone());
            out.push(def.clone());
        }
    }
    for def in sticky_tags {
        if !def.tag_key.is_empty() && !seen.iter().any(|key| key == &def.tag_key) {
            seen.push(def.tag_key.clone());
            out.push(def.clone());
        }
    }
    for tag in &metadata.current_tags {
        let tag_key = crate::tags_db::normalize_tag_key(tag);
        if !tag_key.is_empty() && !seen.iter().any(|key| key == &tag_key) {
            seen.push(tag_key.clone());
            out.push(NativeOverlayTagDef {
                name: native_tag_display_name(metadata, &tag_key, tag),
                tag_key,
                count: 0,
                pinned: false,
                last_applied_at: 0,
            });
        }
    }
    out
}

fn native_tag_display_name(
    metadata: &NativeOverlayMetadata,
    tag_key: &str,
    fallback: &str,
) -> String {
    metadata
        .tag_choices
        .iter()
        .chain(metadata.shortcut_tags.iter())
        .find(|def| def.tag_key == tag_key)
        .map(|def| def.name.clone())
        .unwrap_or_else(|| crate::tags_db::strip_display_hash(fallback).to_string())
}

fn native_tag_is_on(current_tags: &[String], tag_key: &str) -> bool {
    current_tags
        .iter()
        .any(|tag| crate::tags_db::normalize_tag_key(tag) == tag_key)
}

fn native_tag_button_visuals(is_on: bool) -> (egui::Color32, egui::Color32, egui::Color32) {
    if is_on {
        (
            egui::Color32::from_rgb(236, 255, 238),
            egui::Color32::from_rgba_unmultiplied(26, 108, 62, 246),
            egui::Color32::from_rgb(132, 236, 156),
        )
    } else {
        (
            egui::Color32::from_rgb(248, 250, 255),
            egui::Color32::from_rgba_unmultiplied(35, 38, 50, 246),
            egui::Color32::from_rgb(132, 146, 174),
        )
    }
}

fn draw_native_tag_picker_panel(
    ui: &mut egui::Ui,
    metadata: &NativeOverlayMetadata,
    tag_picker_open: &mut bool,
    tag_picker_input: &mut String,
    tag_picker_focus_request: &mut bool,
    tag_picker_recent_tab: &mut bool,
    tag_picker_enter_pressed: bool,
    tag_picker_ime_active: bool,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(
            egui::RichText::new("タグを選択")
                .color(egui::Color32::WHITE)
                .size(13.0)
                .strong(),
        );
        if ui.button("戻る").clicked() {
            close_native_tag_picker(
                tag_picker_open,
                tag_picker_input,
                tag_picker_focus_request,
                tag_picker_recent_tab,
            );
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        // 長いタイトル / ファイル名は単一行 (Extend) だとパネル右端を越えて描かれて
        // しまうため、パネル幅で折り返して最大 2 行に収める。超過分は末尾 `…` + ホバーで
        // 全文を出す (jump panel のブックマーク タイトルと同じ方針)。
        let max_w = (ui.available_width() - 4.0).max(40.0);
        let (galley, truncated) = layout_wrapped_with_max_lines(
            ui.painter(),
            &metadata.file_name,
            egui::FontId::proportional(11.0),
            egui::Color32::from_rgb(168, 176, 188),
            max_w,
            2,
        );
        let resp = ui.add(egui::Label::new(galley));
        if truncated {
            resp.on_hover_text(&metadata.file_name);
        }
    });

    let mut close_after_apply = false;
    draw_native_tag_picker(
        ui,
        metadata,
        tag_picker_input,
        tag_picker_focus_request,
        tag_picker_recent_tab,
        tag_picker_enter_pressed,
        tag_picker_ime_active,
        commands,
        &mut close_after_apply,
    );
    if close_after_apply {
        close_native_tag_picker(
            tag_picker_open,
            tag_picker_input,
            tag_picker_focus_request,
            tag_picker_recent_tab,
        );
    }
}

fn close_native_tag_picker(
    tag_picker_open: &mut bool,
    tag_picker_input: &mut String,
    tag_picker_focus_request: &mut bool,
    tag_picker_recent_tab: &mut bool,
) {
    *tag_picker_open = false;
    tag_picker_input.clear();
    *tag_picker_focus_request = false;
    *tag_picker_recent_tab = false;
}

fn draw_native_tag_picker(
    ui: &mut egui::Ui,
    metadata: &NativeOverlayMetadata,
    input: &mut String,
    focus_request: &mut bool,
    recent_tab: &mut bool,
    enter_pressed: bool,
    ime_active: bool,
    commands: &mut Vec<NativeOverlayCommand>,
    close_after_apply: &mut bool,
) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        let input_resp = ui.add_sized(
            [174.0, 22.0],
            egui::TextEdit::singleline(input)
                .hint_text("タグを検索/入力")
                .return_key(None::<egui::KeyboardShortcut>),
        );
        // IME 変換確定/キャンセルの Enter/Escape で TextEdit のフォーカスが外れることが
        // ある (Windows IME)。そのフレームに合わせてフォーカスを取り戻す
        // (静止画右パネルの `restore_focus_for_ime_key` と同方針)。
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
        let valid = native_tag_input_valid(&normalized);
        let add_clicked = ui.add_enabled(valid, egui::Button::new("付ける")).clicked();
        let enter_pressed = input_resp.has_focus() && enter_pressed;
        if valid && (add_clicked || enter_pressed) {
            commands.push(NativeOverlayCommand::AddTag {
                name: normalized.clone(),
            });
            input.clear();
            *focus_request = true;
            *close_after_apply = true;
        }
    });
    let normalized = crate::tags_db::normalize_tag_display_name(input.trim());
    let input_too_long = normalized.chars().count() > 64;
    let input_has_whitespace = crate::tags_db::tag_display_name_has_whitespace(&normalized);
    if input_too_long || input_has_whitespace {
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new(if input_too_long {
                    "タグ名は64文字以内です。"
                } else {
                    "タグ名に空白は使えません。"
                })
                .size(11.0)
                .color(egui::Color32::from_rgb(220, 120, 90)),
            );
        });
    }

    let query_key = crate::tags_db::normalize_tag_key(&normalized);
    draw_native_tag_picker_tabs(ui, recent_tab);
    let mut choices = native_picker_choices(metadata, &query_key, *recent_tab);
    choices.truncate(12);
    ui.add_space(4.0);
    if choices.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new("候補なし")
                    .size(11.0)
                    .color(egui::Color32::from_gray(150)),
            );
        });
        return;
    }

    for choice in choices {
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            let tag = format!("#{}", choice.name);
            let label_w = (ui.available_width() - 118.0).max(96.0);
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
            let is_on = native_tag_is_on(&metadata.current_tags, &choice.tag_key);
            let button_label = if is_on { "外す" } else { "付ける" };
            if ui.button(button_label).clicked() {
                if is_on {
                    commands.push(NativeOverlayCommand::RemoveTag {
                        name: choice.name.clone(),
                    });
                    *close_after_apply = true;
                } else {
                    commands.push(NativeOverlayCommand::AddTag {
                        name: choice.name.clone(),
                    });
                    *close_after_apply = true;
                }
            }
        });
    }
}

fn native_tag_input_valid(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 64
        && !crate::tags_db::tag_display_name_has_whitespace(name)
}

fn draw_native_tag_picker_tabs(ui: &mut egui::Ui, recent_tab: &mut bool) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        if ui.selectable_label(!*recent_tab, "ピン留め").clicked() {
            *recent_tab = false;
        }
        if ui.selectable_label(*recent_tab, "最近").clicked() {
            *recent_tab = true;
        }
    });
}

fn native_picker_choices(
    metadata: &NativeOverlayMetadata,
    query_key: &str,
    recent_tab: bool,
) -> Vec<NativeOverlayTagDef> {
    let mut out = Vec::new();
    let mut seen = Vec::<String>::new();
    for choice in metadata.tag_choices.iter() {
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

pub(super) fn draw_native_metadata_panel(
    ctx: &egui::Context,
    overlay_width_points: f32,
    overlay_height_points: f32,
    metadata: &NativeOverlayMetadata,
    tag_picker_open: &mut bool,
    tag_picker_input: &mut String,
    tag_picker_focus_request: &mut bool,
    sticky_item_key: &mut Option<String>,
    sticky_tags: &mut Vec<NativeOverlayTagDef>,
    tag_picker_recent_tab: &mut bool,
    tag_picker_enter_pressed: bool,
    tag_picker_escape_pressed: bool,
    tag_picker_ime_active: bool,
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
                .unwrap_or(if metadata.probe_info_available {
                    &metadata.file_name
                } else {
                    ""
                });
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
                (
                    "解像度",
                    if metadata.probe_info_available {
                        format!("{}x{}", metadata.width, metadata.height)
                    } else {
                        String::new()
                    },
                ),
                (
                    "フレームレート",
                    if metadata.probe_info_available {
                        format_fps(metadata.avg_fps)
                    } else {
                        String::new()
                    },
                ),
                ("コーデック", metadata.video_codec.clone()),
                ("デコーダ", metadata.video_decoder.clone()),
                (
                    "音声",
                    if metadata.probe_info_available {
                        audio_label
                    } else {
                        String::new()
                    },
                ),
                (
                    "総ビットレート",
                    if metadata.probe_info_available {
                        format_bitrate(metadata.bit_rate_bps)
                    } else {
                        String::new()
                    },
                ),
                (
                    "長さ",
                    if metadata.probe_info_available {
                        format_overlay_time(metadata.duration_secs)
                    } else {
                        String::new()
                    },
                ),
                (
                    "チャプター",
                    if metadata.probe_info_available {
                        metadata.chapter_count.to_string()
                    } else {
                        String::new()
                    },
                ),
                (
                    "GPU経路",
                    if metadata.probe_info_available {
                        gpu_path_kind.to_string()
                    } else {
                        String::new()
                    },
                ),
                (
                    "デコード",
                    if metadata.probe_info_available {
                        decode_kind.to_string()
                    } else {
                        String::new()
                    },
                ),
                (
                    "フレーム表示",
                    if metadata.probe_info_available {
                        frame_path_kind.to_string()
                    } else {
                        String::new()
                    },
                ),
                (
                    "デインターレース",
                    if metadata.probe_info_available {
                        deinterlace_text
                    } else {
                        String::new()
                    },
                ),
                (
                    "D3D11VA",
                    if metadata.probe_info_available {
                        d3d11va.to_string()
                    } else {
                        String::new()
                    },
                ),
            ];
            rows.retain(|(_, value)| !metadata_clean_text(value).is_empty());

            let content_rect = egui::Rect::from_min_max(rect.min + egui::vec2(0.0, 38.0), rect.max);
            let mut content_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(content_rect)
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
            );
            content_ui.spacing_mut().scroll = native_metadata_panel_scroll_style();
            egui::ScrollArea::vertical()
                .id_salt("native_video_metadata_scroll")
                .auto_shrink([false; 2])
                .max_height(content_rect.height())
                .show(&mut content_ui, |ui| {
                    ui.add_space(6.0);
                    if sync_native_tag_sticky(metadata, sticky_item_key, sticky_tags) {
                        close_native_tag_picker(
                            tag_picker_open,
                            tag_picker_input,
                            tag_picker_focus_request,
                            tag_picker_recent_tab,
                        );
                    }
                    if *tag_picker_open {
                        // Escape でピッカーを閉じる (静止画右パネルと挙動を揃える)。
                        // IME 変換キャンセルの Escape は呼び出し側で除外済み。
                        if tag_picker_escape_pressed {
                            close_native_tag_picker(
                                tag_picker_open,
                                tag_picker_input,
                                tag_picker_focus_request,
                                tag_picker_recent_tab,
                            );
                            return;
                        }
                        draw_native_tag_picker_panel(
                            ui,
                            metadata,
                            tag_picker_open,
                            tag_picker_input,
                            tag_picker_focus_request,
                            tag_picker_recent_tab,
                            tag_picker_enter_pressed,
                            tag_picker_ime_active,
                            commands,
                        );
                        return;
                    }
                    // ── ★ レーティング (最上段。★ → タグ → 内容 の統一順序、Inc 5 FB) ──
                    // タイトル/タグ/メタ行と同じ 14px の左インデントを付ける (Inc 5 FB: 動画は
                    // ★行だけ左余白が無く音楽ビューと食い違っていた)。
                    let star_cmd = ui
                        .horizontal(|ui| {
                            ui.add_space(14.0);
                            crate::ui_helpers::draw_rating_stars(ui, metadata.rating)
                        })
                        .inner;
                    if let Some(new_stars) = star_cmd {
                        commands.push(NativeOverlayCommand::SetRating { stars: new_stars });
                    }
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);
                    draw_native_tag_panel(
                        ui,
                        metadata,
                        tag_picker_open,
                        tag_picker_input,
                        tag_picker_focus_request,
                        tag_picker_recent_tab,
                        sticky_item_key,
                        sticky_tags,
                        commands,
                    );
                    if (!metadata.shortcut_tags.is_empty()
                        || !metadata.current_tags.is_empty()
                        || *tag_picker_open)
                        && !rows.is_empty()
                    {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);
                    }
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

pub(super) fn native_metadata_panel_scroll_style() -> egui::style::ScrollStyle {
    let mut scroll = egui::style::ScrollStyle::solid();
    scroll.bar_width = 8.0;
    scroll.bar_inner_margin = 4.0;
    scroll.bar_outer_margin = 2.0;
    scroll.foreground_color = true;
    scroll
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
            let label_text_y_offset = 3.0;
            let cell_pad = 3.0;
            let cell_bottom_pad = 5.0;
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
            let selected_idx = state
                .selected_idx
                .filter(|idx| *idx < state.timestamps.len());

            for idx in 0..state.timestamps.len() {
                let col = idx % columns;
                let row = idx / columns;
                let x0 = grid_left + (tile_w + gap_x) * col as f32;
                let y0 = grid_top + (tile_h + label_h + gap_y) * row as f32;
                let tile_rect =
                    egui::Rect::from_min_size(egui::pos2(x0, y0), egui::vec2(tile_w, tile_h));
                if tile_rect.max.y > overlay_height_points - (label_h + cell_bottom_pad) {
                    continue;
                }
                let label_rect = egui::Rect::from_min_max(
                    egui::pos2(tile_rect.min.x, tile_rect.max.y),
                    egui::pos2(tile_rect.max.x, tile_rect.max.y + label_h),
                );
                let cell_rect = egui::Rect::from_min_max(
                    tile_rect.min - egui::vec2(cell_pad, cell_pad),
                    label_rect.max + egui::vec2(cell_pad, cell_bottom_pad),
                );
                let selected = selected_idx == Some(idx);

                if selected {
                    painter.rect_filled(cell_rect, 6.0, egui::Color32::from_rgb(255, 194, 62));
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
                    label_rect.center() + egui::vec2(0.0, label_text_y_offset),
                    egui::Align2::CENTER_CENTER,
                    format_overlay_time(pts),
                    egui::FontId::proportional(12.0),
                    if selected {
                        egui::Color32::from_rgb(28, 22, 8)
                    } else {
                        egui::Color32::from_rgb(220, 220, 220)
                    },
                );

                let resp = ui.interact(
                    cell_rect,
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
                if selected {
                    painter.rect_stroke(
                        tile_rect.expand(1.0),
                        5.0,
                        egui::Stroke::new(2.0, egui::Color32::from_rgb(28, 22, 8)),
                        egui::StrokeKind::Inside,
                    );
                    painter.rect_stroke(
                        cell_rect,
                        6.0,
                        egui::Stroke::new(3.0, egui::Color32::from_rgb(255, 223, 96)),
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
    // 動画 HUD 2 段化リデザイン (Phase 3): シーク HUD の上に 2pt の隙間を保つ。
    // HUD_BOTTOM_HEIGHT + 2.0 で HUD top - 2pt = 隙間下端。
    (overlay_height_points - (crate::video::native_presenter::HUD_BOTTOM_HEIGHT + 2.0))
        .max(native_panel_top())
}

pub(super) fn native_jump_panel_rect(overlay_height_points: f32) -> egui::Rect {
    let top = native_panel_top();
    // 動画 HUD 2 段化リデザイン (Phase 3): パネル底辺 = HUD top - 2pt の隙間
    // (= overlay_h - HUD_BOTTOM_HEIGHT - 2)。`native_panel_hover_bottom` と一致させる。
    let panel_h =
        (overlay_height_points - top - (crate::video::native_presenter::HUD_BOTTOM_HEIGHT + 2.0))
            .max(240.0);
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
    let panel_h =
        (overlay_height_points - top - (crate::video::native_presenter::HUD_BOTTOM_HEIGHT + 2.0))
            .max(260.0);
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
    // 動画 HUD 2 段化リデザイン (Phase 3): パネル下端 = HUD top - 10pt の隙間
    // (= overlay_h - HUD_BOTTOM_HEIGHT - 10)。パネル top は native_panel_top()+10 なので
    // max_height = overlay_h - (native_panel_top()+10) - (HUD_BOTTOM_HEIGHT+10) と等価。
    // 簡単のため (HUD_BOTTOM_HEIGHT + 10.0) で底辺余白を取る。
    let max_height = (overlay_height_points
        - native_panel_top()
        - (crate::video::native_presenter::HUD_BOTTOM_HEIGHT + 10.0))
        .max(240.0);
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

/// 現在再生位置の **直前にある最後のチャプター/ブックマーク** を返す。
///
/// 「現在再生中」の判定基準: `entry.pts_secs <= position_secs` を満たすマーカー群から
/// `pts_secs` が最大のものを選ぶ (= ユーザーが「ここから先まで進んだ」最後のマーカー)。
/// Pin は含めない (= ユーザーが付けたフレームピンであり「再生中の区間」を表さないため)。
///
/// 種別優先順位: 「(s) 直近の方を表示」を採用。Chapter / Bookmark の区別なく **pts_secs
/// が最大** のものを返す (= 種別関係なく時間的に直前のマーカー)。
///
/// すべてのマーカーが現在位置より後 / 一覧が空 / 該当 kind が無い場合は None。
pub(super) fn find_now_playing_marker(
    entries: &[NativeOverlayJumpEntry],
    position_secs: f64,
) -> Option<&NativeOverlayJumpEntry> {
    entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.kind,
                NativeOverlayTimelineMarkerKind::Chapter
                    | NativeOverlayTimelineMarkerKind::Bookmark
            )
        })
        .filter(|entry| entry.pts_secs <= position_secs)
        .max_by(|a, b| {
            a.pts_secs
                .partial_cmp(&b.pts_secs)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
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

pub(crate) fn draw_overlay_button_bg(
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

pub(crate) fn draw_overlay_play_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
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

pub(crate) fn draw_overlay_pause_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
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

pub(crate) fn draw_overlay_replay_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
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

pub(crate) fn draw_overlay_loop_icon(
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

pub(crate) fn draw_overlay_continuous_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    mode: crate::video::VideoContinuousMode,
) {
    fn draw_triangle(painter: &egui::Painter, c: egui::Pos2, w: f32, h: f32, color: egui::Color32) {
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(c.x + w * 0.45, c.y),
                egui::pos2(c.x - w * 0.45, c.y - h * 0.5),
                egui::pos2(c.x - w * 0.45, c.y + h * 0.5),
            ],
            color,
            egui::Stroke::NONE,
        ));
    }
    fn draw_arrow_head(
        painter: &egui::Painter,
        tip: egui::Pos2,
        dir: egui::Vec2,
        size: f32,
        color: egui::Color32,
    ) {
        let dir = dir.normalized();
        let normal = egui::vec2(-dir.y, dir.x);
        let base = tip - dir * size;
        painter.add(egui::Shape::convex_polygon(
            vec![
                tip,
                base + normal * size * 0.55,
                base - normal * size * 0.55,
            ],
            color,
            egui::Stroke::NONE,
        ));
    }

    let c = rect.center();
    let side = rect.width().min(rect.height());
    let base = egui::Color32::from_rgb(238, 238, 238);
    let accent = egui::Color32::from_rgb(35, 175, 225);
    let stroke = egui::Stroke::new((side * 0.055).max(1.25), base);
    let row_gap = side * 0.24;
    let y0 = c.y - row_gap;
    let y1 = c.y;
    let y2 = c.y + row_gap;
    let line_x0 = c.x - side * 0.12;
    let line_x1 = c.x + side * 0.08;
    let dot_x = c.x - side * 0.34;
    let dot_r = side * 0.035;

    draw_triangle(
        painter,
        egui::pos2(dot_x, y0),
        side * 0.22,
        side * 0.25,
        base,
    );
    painter.line_segment([egui::pos2(line_x0, y0), egui::pos2(line_x1, y0)], stroke);
    for y in [y1, y2] {
        painter.circle_filled(egui::pos2(dot_x, y), dot_r, base);
        painter.line_segment([egui::pos2(line_x0, y), egui::pos2(line_x1, y)], stroke);
    }

    let arrow_stroke = egui::Stroke::new((side * 0.075).max(1.65), accent);
    let arrow_size = side * 0.15;
    match mode {
        crate::video::VideoContinuousMode::Off => {}
        crate::video::VideoContinuousMode::Continuous => {
            let elbow_x = c.x + side * 0.40;
            let start = egui::pos2(line_x1 + side * 0.10, y0);
            let elbow = egui::pos2(elbow_x, y0);
            let tip = egui::pos2(elbow_x, y1 + side * 0.13);
            painter.line_segment([start, elbow], arrow_stroke);
            painter.line_segment([elbow, tip], arrow_stroke);
            draw_arrow_head(painter, tip, egui::vec2(0.0, 1.0), arrow_size, accent);
        }
        crate::video::VideoContinuousMode::ContinuousLoop => {
            let right_x = c.x + side * 0.43;
            let tail_x = line_x1 + side * 0.10;
            let tip = egui::pos2(line_x1 + side * 0.06, y0);
            let top_right = egui::pos2(right_x, y0);
            let bottom_right = egui::pos2(right_x, y2);
            let bottom_start = egui::pos2(tail_x, y2);
            painter.line_segment([bottom_start, bottom_right], arrow_stroke);
            painter.line_segment([bottom_right, top_right], arrow_stroke);
            painter.line_segment([top_right, tip], arrow_stroke);
            draw_arrow_head(painter, tip, egui::vec2(-1.0, 0.0), arrow_size, accent);
        }
    }
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

/// 一括ブックマーク登録用のアイコン。
/// 左側に小さなブックマーク、右側に「リスト」を示す 3 本の横線。
pub(super) fn draw_overlay_bulk_bookmark_icon(
    painter: &egui::Painter,
    c: egui::Pos2,
    r: f32,
    fill: egui::Color32,
) {
    // 左側の小さなブックマーク (個別 bookmark の縮小版)
    let mark_w = r * 0.85;
    let mark_h = r * 1.30;
    let mark_center = egui::pos2(c.x - r * 0.55, c.y);
    let rect = egui::Rect::from_center_size(mark_center, egui::vec2(mark_w, mark_h));
    let notch = egui::pos2(rect.center().x, rect.max.y - r * 0.28);
    painter.add(egui::Shape::convex_polygon(
        vec![
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            notch,
            rect.left_bottom(),
        ],
        fill,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 245, 190)),
    ));
    // 右側のリスト線 (3 本)
    let line_color = fill;
    let stroke = egui::Stroke::new(1.4, line_color);
    let x0 = c.x + r * 0.15;
    let x1 = c.x + r * 0.85;
    for i in 0..3 {
        let y = c.y + (i as f32 - 1.0) * r * 0.45;
        painter.line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], stroke);
    }
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

pub(crate) fn draw_overlay_speaker_icon(
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

/// 音量 dB フェーダースライダー (動画 HUD / 音楽 HUD 共有、Inc 5c-B1)。
///
/// `vol_rect` にトラック背景 + fill (0dB 未満 = グレー / 0dB 超 = ブースト黄) +
/// `VIDEO_VOLUME_FADER_DB_MARKS` の目盛りを painter だけで描き、クリック / ドラッグ /
/// ダブルクリック (= 0dB リセット) を解釈する。右クリックは扱わず、フルスクリーンの通常
/// 右クリック挙動に譲る (実機 FB 2026-07-02)。フェーダーマッピングは
/// `crate::settings::video_volume_*_fader_pos` (= -80..+18dB) を使う。
///
/// 返り値 `Some((volume, persist))` = 呼び出し側が発行すべき音量変更 (`persist` は
/// `settings` へ保存すべきかどうか = ドラッグ確定 / クリック / ダブルクリックで true、
/// ドラッグ中は false)。動画は `NativeOverlayCommand::SetVolume` へ、音楽は
/// `settings.video_volume` + `player.set_volume` へ翻訳する。
///
/// `last_volume_target` はドラッグ確定 (`drag_stopped`) 時に最後のドラッグ値を
/// 永続化するための frame 跨ぎ state。呼び出し側 (App / overlay) が所有する。
/// `tooltip` が `Some` ならダーク配色のホバーチップを付ける (`None` = 付けない)。
pub(crate) fn draw_overlay_volume_slider(
    ui: &egui::Ui,
    painter: &egui::Painter,
    vol_rect: egui::Rect,
    volume: f64,
    id: egui::Id,
    tooltip: Option<String>,
    last_volume_target: &mut Option<f64>,
) -> Option<(f64, bool)> {
    painter.rect_filled(vol_rect, 2.0, egui::Color32::from_gray(74));
    let volume = finite_video_volume(volume);
    let volume_pos = crate::settings::video_volume_linear_to_fader_pos(volume) as f32;
    let zero_frac = crate::settings::video_volume_db_to_fader_pos(0.0) as f32;
    let normal_fill_frac = volume_pos.min(zero_frac);
    if normal_fill_frac > 0.0 {
        let normal_fill = egui::Rect::from_min_max(
            vol_rect.min,
            egui::pos2(
                vol_rect.min.x + vol_rect.width() * normal_fill_frac,
                vol_rect.max.y,
            ),
        );
        painter.rect_filled(normal_fill, 2.0, egui::Color32::from_rgb(220, 220, 220));
    }
    if volume_pos > zero_frac {
        let boost_fill = egui::Rect::from_min_max(
            egui::pos2(
                vol_rect.min.x + vol_rect.width() * zero_frac,
                vol_rect.min.y,
            ),
            egui::pos2(
                vol_rect.min.x + vol_rect.width() * volume_pos,
                vol_rect.max.y,
            ),
        );
        painter.rect_filled(boost_fill, 2.0, egui::Color32::from_rgb(255, 198, 62));
    }
    for &db in &crate::settings::VIDEO_VOLUME_FADER_DB_MARKS {
        let frac = crate::settings::video_volume_db_to_fader_pos(db) as f32;
        let x = vol_rect.min.x + vol_rect.width() * frac;
        let tick_h = if db == 0.0 {
            8.0
        } else if db > 0.0 {
            6.0
        } else {
            4.0
        };
        let color = if db == 0.0 {
            egui::Color32::from_gray(170)
        } else if db > 0.0 {
            egui::Color32::from_rgb(220, 170, 70)
        } else {
            egui::Color32::from_gray(118)
        };
        painter.line_segment(
            [
                egui::pos2(x, vol_rect.center().y - tick_h * 0.5),
                egui::pos2(x, vol_rect.center().y + tick_h * 0.5),
            ],
            egui::Stroke::new(1.0, color),
        );
    }
    let vol_resp = ui.interact(
        vol_rect.expand2(egui::vec2(0.0, 10.0)),
        id,
        egui::Sense::click_and_drag(),
    );
    if vol_resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    let vol_resp = match tooltip {
        Some(text) => vol_resp.hover_tip_dark(text),
        None => vol_resp,
    };
    let mut out = None;
    // リセットはダブルクリックのみ。右クリック (secondary) はフルスクリーンの通常
    // 右クリック挙動 (リング/ジェスチャ/閉じる) に譲り、ここでは扱わない。以前は
    // secondary_clicked でも 0dB リセットしていたが、同じ右クリックが背後のフルスクリーン
    // ハンドラにも届いて「リセット後に再生終了」する二重動作になっていた (実機 FB 2026-07-02)。
    if vol_resp.double_clicked() {
        *last_volume_target = Some(1.0);
        out = Some((1.0, true));
    } else if (vol_resp.clicked() || vol_resp.dragged())
        && let Some(pos) = vol_resp.interact_pointer_pos()
    {
        let value = crate::settings::video_volume_fader_pos_to_linear(
            ((pos.x - vol_rect.min.x) / vol_rect.width()).clamp(0.0, 1.0) as f64,
        );
        *last_volume_target = Some(value);
        out = Some((value, vol_resp.clicked() && !vol_resp.dragged()));
    }
    if vol_resp.drag_stopped() {
        let value = last_volume_target.take().unwrap_or(volume);
        out = Some((value, true));
    }
    out
}

/// 再生速度ボタン + プリセット popup (動画 HUD / 音楽 HUD 共有、Inc 5c-B2)。
///
/// `speed_rect` にボタン背景 (`draw_overlay_button_bg`) + 現在速度ラベル
/// (`format_playback_speed`) を描き、左クリックで popup をトグル、ダブルクリックで x1
/// リセット。右クリックは扱わず、フルスクリーンの通常右クリック挙動に譲る (実機 FB
/// 2026-07-02)。`popup_open` が true の間は `speed_rect` の上に `PLAYBACK_SPEED_CHOICES`
/// の選択 popup を Area で出す。popup 外クリックで閉じる。
///
/// 返り値: 速度変更を発行すべきなら `Some(speed)` (呼び出し側が `SetPlaybackSpeed` /
/// `player.set_playback_speed` へ翻訳)。x1 リセットは `Some(1.0)`、選択は clamp 済み速度。
/// `popup_open` は helper が直接書き換える (frame 跨ぎ state、呼び出し側が所有)。popup を
/// 描いた frame はその rect を `popup_rect_out` へ書く (native HWND の SetWindowRgn 用。
/// 使わない呼び出し側 = 音楽は `&mut None` を渡して無視してよい)。
///
/// popup の位置は `container_left`/`container_width` (= 描画コンテナの左端 + 幅) と
/// `hud_top` (= HUD 上端 Y) から算出する。動画は overlay 座標 (left=0, width=overlay 幅)、
/// 音楽は `hud_rect` の座標をそのまま渡す。
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_overlay_speed_control(
    ctx: &egui::Context,
    ui: &egui::Ui,
    painter: &egui::Painter,
    speed_rect: egui::Rect,
    text_center_y: f32,
    playback_speed: f64,
    button_id: egui::Id,
    popup_area_id: egui::Id,
    container_left: f32,
    container_width: f32,
    hud_top: f32,
    popup_open: &mut bool,
    popup_rect_out: &mut Option<egui::Rect>,
) -> Option<f64> {
    use crate::video::clock::{PLAYBACK_SPEED_CHOICES, format_playback_speed};

    let mut result = None;
    let mut speed_resp = ui.interact(speed_rect, button_id, egui::Sense::click());
    draw_overlay_button_bg(painter, speed_rect, speed_resp.hovered(), false);
    painter.text(
        egui::pos2(speed_rect.center().x, text_center_y),
        egui::Align2::CENTER_CENTER,
        format_playback_speed(playback_speed),
        egui::FontId::proportional(12.0),
        egui::Color32::from_rgb(238, 238, 238),
    );
    // リセットはダブルクリックのみ (音量スライダーと同じ理由。右クリックは背後の
    // フルスクリーン右クリック挙動に譲る。実機 FB 2026-07-02)。
    if speed_resp.double_clicked() {
        *popup_open = false;
        result = Some(1.0);
    } else if speed_resp.clicked() {
        *popup_open = !*popup_open;
    }
    if !*popup_open {
        speed_resp = speed_resp.hover_tip_dark("再生速度 (ダブルクリックで x1)");
    }
    if *popup_open {
        let popup_w = 356.0_f32.min((container_width - 16.0).max(180.0));
        let speed_choice_size = egui::vec2(46.0, 24.0);
        let speed_choice_text_y = 4.0;
        let popup_h = speed_choice_size.y + 12.0;
        // clamp 範囲を正規化する: 極端に狭いコンテナ (`container_width < popup_w + 16`) では
        // `max_x < min_x` になり `f32::clamp` が panic するため、`max_x` を `min_x` 以上に
        // 押し上げる。正常幅では `max_x` は元の式と一致し挙動不変 (Codex 5c-B2 P2)。
        let popup_min_x = container_left + 8.0;
        let popup_max_x = (container_left + container_width - popup_w - 8.0).max(popup_min_x);
        let popup_x = (speed_rect.center().x - popup_w * 0.5).clamp(popup_min_x, popup_max_x);
        let popup_y = (hud_top - popup_h - 6.0).max(8.0);
        let mut selected_speed = None;
        let popup_inner = egui::Area::new(popup_area_id)
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(popup_x, popup_y))
            .show(ctx, |ui| {
                // popup は `ctx` 直下の独立 Area なので、コンテキストの既定 visuals を継承する。
                // 音楽ビューのフルスクリーンコンテキストは os_theme で light になり得るため、
                // 選択ボタンの `interact_selectable` 色が崩れる (実機 FB 2026-07-02)。dark 固定に
                // することで動画 (既定 dark) と音楽で見た目を揃える。
                *ui.visuals_mut() = egui::Visuals::dark();
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 225))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(110)))
                    .corner_radius(egui::CornerRadius::same(4))
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.set_min_width(popup_w - 12.0);
                        ui.horizontal_wrapped(|ui| {
                            for speed in PLAYBACK_SPEED_CHOICES {
                                let selected = (playback_speed - speed).abs() < 1.0e-6;
                                let label = format_playback_speed(speed);
                                let (button_rect, button_resp) =
                                    ui.allocate_exact_size(speed_choice_size, egui::Sense::click());
                                if button_resp.hovered() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                let visuals =
                                    ui.style().interact_selectable(&button_resp, selected);
                                let painter = ui.painter();
                                painter.rect_filled(button_rect, 3.0, visuals.weak_bg_fill);
                                painter.rect_stroke(
                                    button_rect,
                                    3.0,
                                    visuals.bg_stroke,
                                    egui::StrokeKind::Inside,
                                );
                                painter.text(
                                    button_rect.center() + egui::vec2(0.0, speed_choice_text_y),
                                    egui::Align2::CENTER_CENTER,
                                    label,
                                    egui::TextStyle::Button.resolve(ui.style()),
                                    visuals.fg_stroke.color,
                                );
                                if button_resp.clicked() {
                                    selected_speed = Some(speed);
                                }
                            }
                        });
                    });
            });
        let popup_rect = popup_inner.response.rect;
        *popup_rect_out = Some(popup_rect);
        if ctx.input(|i| i.pointer.any_click())
            && !speed_resp.hovered()
            && let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
            && !popup_rect.contains(pos)
        {
            *popup_open = false;
        }
        if let Some(speed) = selected_speed {
            let speed = crate::video::clock::clamp_playback_speed(speed);
            *popup_open = false;
            result = Some(speed);
        }
    }
    result
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

/// `text` を `painter` で `font` レイアウトしたとき、幅が `max_width` を超えない
/// ように末尾を `…` で省略した `Galley` を返す。文字幅は `layout_no_wrap` で実測
/// するので CJK / ASCII / 絵文字混在でも overshoot しない。
///
/// `text` がそのまま収まれば省略なしの Galley を返す。`…` も入らない極小幅は
/// `None`。`ui_helpers::draw_cell_filename` と同じ手法 (平均幅近似が CJK 混在で
/// 使えないので 1 文字ずつ削って再 layout する)。
///
/// `soft_char_cap` は **iterate 開始前の上限** で、ここで先に頭から `soft_char_cap`
/// 文字に切ってから layout / iterate を始める。再 layout は最大でも `soft_char_cap`
/// 回に制限される (Codex P2 反映、`draw_cell_filename` の 18 文字 soft cap と同じ
/// 考え方)。bulk import で極端に長いタイトルが入っていてもフレーム hitch しない。
/// 呼び出し側は panel の物理上限に対して余裕を持った値を渡す。
///
/// 文字数固定で truncate していた既存呼び出し (`truncate_overlay_text(s, N)`) で
/// CJK が多いタイトルが実幅で右にはみ出すのを避けるために導入 (左ジャンプ
/// パネルのブックマークタイトルが panel 右端を越える事象、2026-05)。
///
/// TODO (Codex P3): 切り詰めは Unicode scalar 単位なので emoji ZWJ / variation
/// selector / 結合文字を split する可能性がある。emoji 混じりタイトルが実害になる
/// なら `unicode-segmentation` で grapheme cluster 単位に切る。
/// 指定 max_width で wrap し、max_lines 行までに収まる multi-line Galley を返す。
/// 行数オーバー時は本文を binary-search でカットし末尾 `…` を付けて再 layout する。
///
/// 戻り値: `(galley, was_truncated)`。`was_truncated=true` のとき、ホバー時に全文を
/// ツールチップで表示するなどのフォローを呼び出し側で行う。
///
/// jump panel のブックマーク タイトルなど、横幅制限がある一方で長いタイトル全体を
/// 複数行で見せたい用途で使う。1 行だけ truncate したい場合は
/// `layout_truncated_to_width` を使うこと。
pub(super) fn layout_wrapped_with_max_lines(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    max_width: f32,
    max_lines: usize,
) -> (std::sync::Arc<egui::Galley>, bool) {
    // `max_lines = 0` は「表示しない」契約 (Codex P3 指摘): 空 galley を返す。
    if max_lines == 0 {
        let empty = painter.layout(String::new(), font, color, max_width.max(1.0));
        return (empty, false);
    }
    // 病的に長い title (bulk import で貼り付けた数千〜数万文字) でも UI スレッドの
    // draw path で O(N) を走らせないよう、入力を `MAX_INPUT_CHARS` で頭打ちにする
    // (Codex P2 指摘)。max_lines × 想定 1 行当たり char 数 (≈ 100) より十分多い 1024
    // を上限とする。これ以上は元々 5 行に絶対収まらないので、cap した時点で truncate
    // 確定 (was_truncated=true)。
    const MAX_INPUT_CHARS: usize = 1024;
    let mut input_truncated = false;
    let mut chars: Vec<char> = text.chars().take(MAX_INPUT_CHARS + 1).collect();
    if chars.len() > MAX_INPUT_CHARS {
        chars.truncate(MAX_INPUT_CHARS);
        input_truncated = true;
    }
    let capped_text: String = if input_truncated {
        chars.iter().collect::<String>() + "…"
    } else {
        chars.iter().collect()
    };

    let full = painter.layout(capped_text.clone(), font.clone(), color, max_width.max(1.0));
    if full.rows.len() <= max_lines {
        return (full, input_truncated);
    }
    if chars.is_empty() {
        return (full, input_truncated);
    }
    // 行数超過 → 二分探索で本文を縮めて末尾 `…` を付けて再 layout。bounded ~log N 回。
    let mut lo: usize = 1;
    let mut hi: usize = chars.len();
    let mut best: Option<std::sync::Arc<egui::Galley>> = None;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let candidate: String = chars[..mid].iter().collect::<String>() + "…";
        let g = painter.layout(candidate, font.clone(), color, max_width.max(1.0));
        if g.rows.len() <= max_lines {
            best = Some(g);
            lo = mid + 1;
        } else if mid == 0 {
            break;
        } else {
            hi = mid - 1;
        }
    }
    match best {
        Some(g) => (g, true),
        None => (full, true), // 1 文字 + `…` でも溢れる極小幅: 諦めて full を返す
    }
}

/// 単一行で max_width を超えたら末尾 `…` で省略する従来 helper。
/// `layout_wrapped_with_max_lines` の導入後、jump panel のタイトルは multi-line
/// 経路に移行したため、現在は呼び出し元なしだが、将来 1 行 truncate が必要な場面
/// (HUD 上部のファイル名等) で再利用できるよう残す。
#[allow(dead_code)]
pub(super) fn layout_truncated_to_width(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    max_width: f32,
    soft_char_cap: usize,
) -> Option<std::sync::Arc<egui::Galley>> {
    if max_width < 1.0 || soft_char_cap == 0 {
        return None;
    }
    // 入力を **単一パス** で soft_char_cap 文字以内に頭打ちにする (Codex P3 反映)。
    // 旧版は `text.chars().count()` で全文走査していたため、1000 文字級タイトルでは
    // 描画パス上で毎フレーム O(N) が残っていた。`by_ref().take()` で N+1 文字目まで
    // 進めて overflowed を判定するので、N 文字未満なら early stop、超えても N+1 文字で
    // 止まる。
    let mut iter = text.chars();
    let chars: Vec<char> = iter.by_ref().take(soft_char_cap).collect();
    let capped_to_cap = iter.next().is_some();
    let initial: String = if capped_to_cap {
        // 頭から soft_char_cap 文字 + `…` で開始 (= 既にこの時点で truncate 表示)。
        let mut s: String = chars.iter().collect();
        s.push('…');
        s
    } else {
        chars.iter().collect()
    };
    let initial_galley = painter.layout_no_wrap(initial, font.clone(), color);
    if initial_galley.size().x <= max_width {
        return Some(initial_galley);
    }
    // 末尾を 1 文字ずつ削って `…` を足し、初めて max_width に収まったものを採用。
    // iterate 対象は capped 状態の文字列 (= 既に soft_char_cap 以下) なので、再 layout
    // 回数は最大 `min(soft_char_cap, total_chars) - 1` 回に bounded (Codex P2)。
    for take in (1..chars.len()).rev() {
        let candidate: String = chars[..take].iter().collect::<String>() + "…";
        let g = painter.layout_no_wrap(candidate, font.clone(), color);
        if g.size().x <= max_width {
            return Some(g);
        }
    }
    // 1 文字 + `…` でも入らない極小幅: `…` 単独で試して、それでも入らないなら諦め。
    let ellipsis = painter.layout_no_wrap("…".to_string(), font.clone(), color);
    if ellipsis.size().x <= max_width {
        Some(ellipsis)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Snapshot シナリオで使う `NativeOverlayJumpEntry` を最小フィールドだけで組み立てる。
    /// thumbnail は None (= 表示しないテスト)、bookmark_id も None (= 任意)。
    fn entry(
        pts: f64,
        kind: NativeOverlayTimelineMarkerKind,
        title: &str,
    ) -> NativeOverlayJumpEntry {
        NativeOverlayJumpEntry {
            pts_secs: pts,
            kind,
            title: Some(title.to_string()),
            bookmark_id: None,
            thumbnail: None,
        }
    }

    /// 通常 case: チャプター/ブックマーク混在で、現在位置 30 秒の直前にあるのは
    /// 25 秒の Chapter (= pts 最大)。
    #[test]
    fn find_now_playing_returns_latest_marker_before_position() {
        let entries = vec![
            entry(0.0, NativeOverlayTimelineMarkerKind::Chapter, "OP"),
            entry(15.0, NativeOverlayTimelineMarkerKind::Bookmark, "サビ"),
            entry(25.0, NativeOverlayTimelineMarkerKind::Chapter, "Aメロ"),
            entry(60.0, NativeOverlayTimelineMarkerKind::Chapter, "Bメロ"),
        ];
        let now = find_now_playing_marker(&entries, 30.0).unwrap();
        assert_eq!(now.pts_secs, 25.0);
        assert_eq!(now.title.as_deref(), Some("Aメロ"));
    }

    /// 「(s) 直近の方を表示」: 種別を問わず pts_secs が最大のものが勝つ。
    /// Bookmark (20.0) と Chapter (15.0) では Bookmark が選ばれる。
    #[test]
    fn find_now_playing_picks_closest_regardless_of_kind() {
        let entries = vec![
            entry(15.0, NativeOverlayTimelineMarkerKind::Chapter, "前章"),
            entry(
                20.0,
                NativeOverlayTimelineMarkerKind::Bookmark,
                "ハイライト",
            ),
        ];
        let now = find_now_playing_marker(&entries, 25.0).unwrap();
        assert_eq!(now.pts_secs, 20.0);
        assert_eq!(now.title.as_deref(), Some("ハイライト"));
    }

    /// Pin は除外される (= 「再生中の区間」を表さないため)。
    /// Pin (30.0) の方が新しくても、Chapter (10.0) が選ばれる。
    #[test]
    fn find_now_playing_excludes_pins() {
        let entries = vec![
            entry(10.0, NativeOverlayTimelineMarkerKind::Chapter, "OP"),
            entry(30.0, NativeOverlayTimelineMarkerKind::Pin, "Pinned frame"),
        ];
        let now = find_now_playing_marker(&entries, 60.0).unwrap();
        assert_eq!(now.pts_secs, 10.0);
        assert_eq!(now.title.as_deref(), Some("OP"));
    }

    /// 全マーカーが現在位置より後 → None (= 表示しない)。
    #[test]
    fn find_now_playing_returns_none_when_all_markers_in_future() {
        let entries = vec![
            entry(60.0, NativeOverlayTimelineMarkerKind::Chapter, "Bメロ"),
            entry(90.0, NativeOverlayTimelineMarkerKind::Chapter, "サビ"),
        ];
        assert!(find_now_playing_marker(&entries, 30.0).is_none());
    }

    /// 該当 kind 無し (= Pin のみ) → None。
    #[test]
    fn find_now_playing_returns_none_when_only_pins_exist() {
        let entries = vec![
            entry(5.0, NativeOverlayTimelineMarkerKind::Pin, "Pin 1"),
            entry(15.0, NativeOverlayTimelineMarkerKind::Pin, "Pin 2"),
        ];
        assert!(find_now_playing_marker(&entries, 60.0).is_none());
    }

    /// 空 entries → None。
    #[test]
    fn find_now_playing_returns_none_for_empty_entries() {
        let entries: Vec<NativeOverlayJumpEntry> = vec![];
        assert!(find_now_playing_marker(&entries, 30.0).is_none());
    }

    /// 境界条件: pts_secs == position_secs はちょうど「直前」とみなして含める。
    /// (= 「今、ここを通過したばかり」を反映)
    #[test]
    fn find_now_playing_includes_marker_at_exact_position() {
        let entries = vec![entry(
            30.0,
            NativeOverlayTimelineMarkerKind::Chapter,
            "ちょうど 30 秒",
        )];
        let now = find_now_playing_marker(&entries, 30.0).unwrap();
        assert_eq!(now.pts_secs, 30.0);
    }

    /// `layout_truncated_to_width` テスト用の painter を作る。egui Context を一度
    /// run して fonts を初期化し、background layer の painter を返す。実フォントは
    /// egui デフォルト (Yu Gothic ではない) だが、本ヘルパーの保証は「戻り値の
    /// galley size が max_width 以下になること」なので、フォントが何であっても
    /// 検証可能。
    fn test_painter() -> (egui::Context, egui::Painter) {
        let ctx = egui::Context::default();
        // Run one no-op pass so fonts are initialized.
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let painter = ctx.layer_painter(egui::LayerId::background());
        (ctx, painter)
    }

    #[test]
    fn layout_truncated_to_width_returns_full_when_fits() {
        let (_ctx, painter) = test_painter();
        let font = egui::FontId::proportional(12.0);
        let color = egui::Color32::WHITE;
        // 十分広い max_width なら原文 (省略なし) がそのまま返る。
        let galley =
            layout_truncated_to_width(&painter, "hello", font, color, 10_000.0, 48).unwrap();
        assert_eq!(galley.text(), "hello");
    }

    #[test]
    fn layout_truncated_to_width_bounds_galley_by_max_width() {
        let (_ctx, painter) = test_painter();
        let font = egui::FontId::proportional(12.0);
        let color = egui::Color32::WHITE;
        // 狭い max_width: 必ず max_width 以下に収まる (実フォント幅で省略される)。
        let max_w = 40.0;
        let galley =
            layout_truncated_to_width(&painter, "abcdefghijklmnopqrstuv", font, color, max_w, 48)
                .unwrap();
        assert!(
            galley.size().x <= max_w,
            "galley width {} exceeds max_width {}",
            galley.size().x,
            max_w
        );
        // 原文より短くなっているはず (省略マーカーが入っている前提)。
        assert!(galley.text().len() < 22);
    }

    #[test]
    fn layout_truncated_to_width_caps_long_input_without_full_walk() {
        let (_ctx, painter) = test_painter();
        let font = egui::FontId::proportional(12.0);
        let color = egui::Color32::WHITE;
        // 1000 文字の入力に soft_char_cap=16 を渡しても、内部 iterate は 16 回以下に
        // bounded で完了する (Codex P2/P3 反映)。戻り値の文字数は cap+省略マーカーを
        // 超えないこと、かつ max_width で実幅 bounded であることを確認。
        let long = "a".repeat(1000);
        let max_w = 200.0;
        let galley = layout_truncated_to_width(&painter, &long, font, color, max_w, 16).unwrap();
        assert!(galley.size().x <= max_w);
        // text() は `…` 1 文字 + 0..=16 文字、合計 17 文字以下。
        assert!(
            galley.text().chars().count() <= 17,
            "galley text {:?} exceeds soft cap + ellipsis",
            galley.text()
        );
    }

    #[test]
    fn layout_truncated_to_width_handles_cjk_mixed_width() {
        let (_ctx, painter) = test_painter();
        let font = egui::FontId::proportional(12.0);
        let color = egui::Color32::WHITE;
        // 全角混じりタイトル: 旧 truncate_overlay_text(_, 16) は文字数で切るので
        // overshoot していた。実幅省略なので max_w を必ず守る。
        let mixed = "モンドの夕暮れ Dusk in Mondstadt";
        let max_w = 80.0;
        let galley = layout_truncated_to_width(&painter, mixed, font, color, max_w, 48).unwrap();
        assert!(
            galley.size().x <= max_w,
            "CJK galley width {} exceeds max_width {}",
            galley.size().x,
            max_w
        );
    }

    #[test]
    fn layout_truncated_to_width_returns_none_for_invalid_args() {
        let (_ctx, painter) = test_painter();
        let font = egui::FontId::proportional(12.0);
        let color = egui::Color32::WHITE;
        // max_width が 0 (= 1.0 未満) なら None。
        assert!(layout_truncated_to_width(&painter, "abc", font.clone(), color, 0.0, 48).is_none());
        // soft_char_cap = 0 でも None (caller の設定ミスを silently 描画しない)。
        assert!(layout_truncated_to_width(&painter, "abc", font, color, 100.0, 0).is_none());
    }

    /// jump / metadata 両パネルの底辺がホバー判定の底辺と一致し、シーク HUD
    /// (top y = overlay_h - HUD_BOTTOM_HEIGHT) の上に 2pt の隙間が空くことを保証する
    /// 回帰テスト。上ホバーバー bottom y = 54 と panel top y = 56 の 2pt 隙間と対称になる前提。
    /// 動画 HUD 2 段化リデザイン (Phase 3) で HUD 高さが 46 → 64 に変わったので、
    /// パネル底辺も連動して `overlay_h - 66 (= HUD_BOTTOM_HEIGHT + 2)` になる。
    #[test]
    fn side_panel_bottoms_match_hover_bottom() {
        use crate::video::native_presenter::HUD_BOTTOM_HEIGHT;
        let overlay_h = 1080.0_f32;
        let overlay_w = 1920.0_f32;

        let hover_bottom = native_panel_hover_bottom(overlay_h);
        let jump = native_jump_panel_rect(overlay_h);
        let meta = native_metadata_panel_rect(overlay_w, overlay_h);

        assert_eq!(hover_bottom, overlay_h - (HUD_BOTTOM_HEIGHT + 2.0));
        assert_eq!(jump.max.y, hover_bottom);
        assert_eq!(meta.max.y, hover_bottom);
    }

    #[test]
    fn metadata_panel_scrollbar_stays_visible_without_hover() {
        let scroll = native_metadata_panel_scroll_style();
        assert!(!scroll.floating);
        assert!(scroll.foreground_color);
        assert!(scroll.bar_width >= 8.0);
        assert!(scroll.allocated_width() >= 10.0);
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
