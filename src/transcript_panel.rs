//! 動画・音楽ビューで共有する文字起こし一覧。描画と一時状態だけを所有する。
use std::sync::Arc;

use crate::transcript_db::TranscriptSegment;
use crate::transcript_display::TranscriptSnapshot;
use eframe::egui;

#[derive(Clone, Copy, Debug, Default)]
struct Anchor {
    row: usize,
    inset: f32,
}

#[derive(Clone, Copy, Debug)]
enum ScrollMode {
    Following(Anchor),
    Manual(Anchor),
}
impl Default for ScrollMode {
    fn default() -> Self {
        Self::Following(Anchor::default())
    }
}

/// 行高の変更と可視行の探索を O(log N) で行う。本文の layout は可視行に限る。
#[derive(Clone, Debug, Default)]
struct RowHeights {
    values: Vec<f32>,
    tree: Vec<f32>,
}
impl RowHeights {
    fn new(count: usize, estimate: f32) -> Self {
        let mut result = Self {
            values: vec![estimate; count],
            tree: vec![0.0; count + 1],
        };
        for i in 1..=count {
            result.tree[i] += estimate;
            let parent = i + (i & i.wrapping_neg());
            if parent <= count {
                result.tree[parent] += result.tree[i];
            }
        }
        result
    }
    fn prefix(&self, mut end: usize) -> f32 {
        let mut sum = 0.0;
        while end > 0 {
            sum += self.tree[end];
            end &= end - 1;
        }
        sum
    }
    fn update(&mut self, row: usize, height: f32) -> bool {
        let delta = height - self.values[row];
        if delta.abs() < 0.25 {
            return false;
        }
        self.values[row] = height;
        let mut i = row + 1;
        while i < self.tree.len() {
            self.tree[i] += delta;
            i += i & i.wrapping_neg();
        }
        true
    }
    fn row_at(&self, offset: f32) -> usize {
        let mut index = 0;
        let mut sum = 0.0;
        let mut bit = self.tree.len().next_power_of_two() >> 1;
        while bit != 0 {
            let next = index + bit;
            if next < self.tree.len() && sum + self.tree[next] <= offset {
                index = next;
                sum += self.tree[next];
            }
            bit >>= 1;
        }
        index.min(self.values.len().saturating_sub(1))
    }
    fn anchor(&self, offset: f32) -> Anchor {
        let row = self.row_at(offset);
        Anchor {
            row,
            inset: (offset - self.prefix(row)).max(0.0),
        }
    }
    fn offset(&self, anchor: Anchor) -> f32 {
        if self.values.is_empty() {
            return 0.0;
        }
        let row = anchor.row.min(self.values.len() - 1);
        self.prefix(row) + anchor.inset.min(self.values[row].max(0.0))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Metrics {
    width: f32,
    font: egui::FontId,
    pixels_per_point: f32,
}

/// パネルを閉じる・別タブへ移ると破棄する。native と music は別々に所有する。
#[derive(Clone, Debug, Default)]
pub(crate) struct TranscriptPanelState {
    key: String,
    segments: Arc<Vec<TranscriptSegment>>,
    metrics: Option<Metrics>,
    font_definitions: Option<egui::FontDefinitions>,
    heights: RowHeights,
    mode: ScrollMode,
    geometry_scroll: Option<Anchor>,
    #[cfg(test)]
    measured_rows: usize,
    #[cfg(test)]
    last_scroll: Option<crate::test_ui::scroll_layout::ScrollLayout>,
}

pub(crate) fn summary_value(snapshot: &TranscriptSnapshot) -> String {
    let summary = snapshot.summary();
    summary
        .strip_prefix("文字起こし: ")
        .unwrap_or(&summary)
        .to_owned()
}
impl TranscriptPanelState {
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

fn active_row(segments: &[TranscriptSegment], position: Option<f64>) -> Option<usize> {
    let millis = position.filter(|value| value.is_finite() && *value >= 0.0)? * 1000.0;
    let index = segments
        .partition_point(|row| row.0 as f64 <= millis)
        .checked_sub(1)?;
    (millis < segments[index].1 as f64).then_some(index)
}

/// offset の変化から推測せず、この一覧に対する実際の入力だけで自動送りを止める。
fn manual_scroll_input(ui: &egui::Ui, rect: egui::Rect, gutter_width: f32) -> bool {
    ui.input(|input| {
        let Some(pos) = input.pointer.interact_pos() else {
            return false;
        };
        if !rect.contains(pos) {
            return false;
        }
        let wheel = input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::MouseWheel { .. }));
        let drag = input.pointer.primary_down()
            && input.pointer.is_decidedly_dragging()
            && input.pointer.delta() != egui::Vec2::ZERO;
        let scrollbar = input.pointer.primary_pressed() && pos.x >= rect.right() - gutter_width;
        wheel || drag || scrollbar
    })
}

/// 戻り値だけが操作。caller は既存の seek 経路へ再生側の秒を渡す。
pub(crate) fn draw_transcript_panel_body(
    ui: &mut egui::Ui,
    snapshot: &TranscriptSnapshot,
    raw_position_secs: f64,
    state: &mut TranscriptPanelState,
) -> Option<f64> {
    let started = crate::perf::is_enabled().then(std::time::Instant::now);
    if state.key != snapshot.key {
        state.reset();
        state.key.clone_from(&snapshot.key);
    }
    if let Some(notice) = snapshot.notice() {
        ui.label(notice);
    }
    if snapshot.segments.is_empty() {
        return None;
    }
    let active = active_row(
        &snapshot.segments,
        snapshot.time_mapping.to_transcript(raw_position_secs),
    );
    let mut resumed = false;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                matches!(state.mode, ScrollMode::Manual(_)),
                egui::Button::new("再生位置へ戻る"),
            )
            .clicked()
        {
            state.mode = ScrollMode::Following(match state.mode {
                ScrollMode::Following(anchor) | ScrollMode::Manual(anchor) => anchor,
            });
            resumed = true;
        }
    });
    let outer = ui.available_rect_before_wrap();
    let gutter = ui.spacing().scroll.bar_width + ui.spacing().scroll.bar_inner_margin;
    let font = egui::TextStyle::Body.resolve(ui.style());
    let time_width = 66.0;
    let metrics = Metrics {
        width: (outer.width() - gutter - time_width - 16.0).max(20.0),
        font: font.clone(),
        pixels_per_point: ui.ctx().pixels_per_point(),
    };
    let fresh = state.metrics.is_none();
    // 同じ FontId のままフォントが差し替わる設定にも追従する。巨大な font bytes は比較しない。
    let fonts_changed = ui.fonts(|fonts| {
        state.font_definitions.as_ref().is_none_or(|old| {
            let current = fonts.definitions();
            old.families != current.families
                || old.font_data.len() != current.font_data.len()
                || old.font_data.iter().any(|(name, data)| {
                    current
                        .font_data
                        .get(name)
                        .is_none_or(|value| !Arc::ptr_eq(data, value))
                })
        })
    });
    if fonts_changed {
        state.font_definitions = Some(ui.fonts(|fonts| fonts.definitions().clone()));
    }
    if fonts_changed
        || state.metrics.as_ref() != Some(&metrics)
        || !Arc::ptr_eq(&state.segments, &snapshot.segments)
    {
        state.heights = RowHeights::new(snapshot.segments.len(), font.size * 1.5 + 10.0);
        state.segments = Arc::clone(&snapshot.segments);
        state.metrics = Some(metrics.clone());
        state.geometry_scroll = match state.mode {
            ScrollMode::Manual(anchor) => Some(anchor),
            ScrollMode::Following(_) => None,
        };
    }
    if manual_scroll_input(ui, outer, gutter) && matches!(state.mode, ScrollMode::Following(_)) {
        state.mode = ScrollMode::Manual(match state.mode {
            ScrollMode::Following(anchor) | ScrollMode::Manual(anchor) => anchor,
        });
    }
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("transcript_rows", &snapshot.key))
        .auto_shrink([false, false])
        .min_scrolled_height(0.0)
        .max_height(outer.height().max(0.0))
        .scroll_source(egui::scroll_area::ScrollSource {
            drag: egui::scroll_area::DragScroll::Always,
            ..Default::default()
        });
    if matches!(state.mode, ScrollMode::Following(_)) {
        if let Some(row) = active {
            scroll = scroll.vertical_scroll_offset(state.heights.prefix(row));
        } else if fresh {
            scroll = scroll.vertical_scroll_offset(0.0);
        }
    } else if let Some(anchor) = state.geometry_scroll.take() {
        scroll = scroll.vertical_scroll_offset(state.heights.offset(anchor));
    }
    let mut seek = None;
    let mut measured = Vec::new();
    let output = ui
        .scope(|ui| {
            ui.spacing_mut().scroll.floating = false;
            scroll.show_viewport(ui, |ui, viewport| {
                ui.set_min_height(state.heights.prefix(snapshot.segments.len()));
                let top = ui.max_rect().top();
                let mut row = state.heights.row_at(viewport.top());
                while row < snapshot.segments.len()
                    && state.heights.prefix(row) <= viewport.bottom()
                {
                    let (start_ms, _, text) = &snapshot.segments[row];
                    let galley = ui.painter().layout(
                        text.clone(),
                        font.clone(),
                        ui.visuals().text_color(),
                        metrics.width,
                    );
                    let height = galley.size().y.max(font.size * 1.5) + 10.0;
                    measured.push((row, height));
                    let row_rect = egui::Rect::from_min_size(
                        egui::pos2(ui.max_rect().left(), top + state.heights.prefix(row)),
                        egui::vec2(ui.available_width(), state.heights.values[row]),
                    );
                    let enabled = snapshot.time_mapping.to_playback(*start_ms as f64 / 1000.0);
                    let response = ui.interact(
                        row_rect,
                        ui.id().with(("transcript_row", row)),
                        egui::Sense::click(),
                    );
                    let painter = ui
                        .painter()
                        .with_clip_rect(row_rect.intersect(ui.clip_rect()));
                    if active == Some(row) {
                        painter.rect_filled(row_rect, 3.0, ui.visuals().selection.bg_fill);
                    } else if response.hovered() {
                        painter.rect_filled(row_rect, 3.0, ui.visuals().widgets.hovered.bg_fill);
                    }
                    painter.text(
                        row_rect.min + egui::vec2(6.0, 5.0),
                        egui::Align2::LEFT_TOP,
                        crate::ui_helpers::format_hms(*start_ms as f64 / 1000.0),
                        egui::FontId::monospace(12.0),
                        ui.visuals().weak_text_color(),
                    );
                    painter.galley(
                        row_rect.min + egui::vec2(time_width, 5.0),
                        galley,
                        ui.visuals().text_color(),
                    );
                    if response.clicked() {
                        seek = enabled;
                    }
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            enabled.is_some(),
                            format!(
                                "{} {}",
                                crate::ui_helpers::format_hms(*start_ms as f64 / 1000.0),
                                text
                            ),
                        )
                    });
                    row += 1;
                }
            })
        })
        .inner;
    if fresh || resumed {
        let mut scroll_state = output.state;
        scroll_state.stop_vertical_scroll_motion();
        scroll_state.store(ui.ctx(), output.id);
    }
    let anchor = state.heights.anchor(output.state.clamped_offset().y);
    state.mode = match state.mode {
        ScrollMode::Following(_) => ScrollMode::Following(anchor),
        ScrollMode::Manual(_) => ScrollMode::Manual(anchor),
    };
    let mut changed = false;
    #[cfg(test)]
    {
        state.measured_rows = measured.len();
        state.last_scroll = Some(crate::test_ui::scroll_layout::ScrollLayout::from_output(
            &output,
        ));
    }
    // 可視範囲の計算中には prefix を変えず、pass 末尾にまとめて採用する。
    let measured_rows = measured.len();
    for (row, height) in measured {
        changed |= state.heights.update(row, height);
    }
    if changed {
        if matches!(state.mode, ScrollMode::Manual(_)) {
            state.geometry_scroll = Some(anchor);
        }
        ui.ctx().request_repaint();
    }
    if let Some(started) = started {
        crate::perf::event(
            "transcript",
            "display_render",
            Some(&snapshot.key),
            0,
            &[
                (
                    "ms",
                    serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                ),
                ("rows", serde_json::json!(snapshot.segments.len())),
                ("layout_rows", serde_json::json!(measured_rows)),
            ],
        );
    }
    seek
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::{NodeT, Queryable};
    #[test]
    fn transcript_row_heights_update_and_prefix_lookup() {
        let mut rows = RowHeights::new(5000, 20.0);
        assert_eq!(rows.prefix(5000), 100000.0);
        assert_eq!(rows.row_at(1000.0), 50);
        rows.update(4, 120.0);
        assert_eq!(rows.prefix(5), 200.0);
        assert_eq!(rows.row_at(190.0), 4);
        assert_eq!(rows.offset(Anchor { row: 5, inset: 2.0 }), 202.0);
    }
    #[test]
    fn transcript_active_row_uses_ranges_and_leaves_gaps_unhighlighted() {
        let rows = vec![(1000, 2000, "a".into()), (4000, 6000, "b".into())];
        assert_eq!(active_row(&rows, Some(1.5)), Some(0));
        assert_eq!(active_row(&rows, Some(3.0)), None);
        assert_eq!(active_row(&rows, None), None);
    }

    fn fixture(count: usize) -> TranscriptSnapshot {
        TranscriptSnapshot {key:"sample.mp4".into(),state:crate::transcript_display::TranscriptDataState::Available,
            segments:Arc::new((0..count).map(|index| ((index as u64+1)*1000,(index as u64+2)*1000,format!("行{index}：幅に合わせて折り返す日本語の文字起こしです。長い文章も全文を表示します。"))).collect()),
            time_mapping:crate::video::transcript_time::TranscriptTimeMapping::raw_pts(10.0),..Default::default()}
    }
    struct View {
        snapshot: TranscriptSnapshot,
        state: TranscriptPanelState,
        seek: Option<f64>,
        position: f64,
    }
    fn harness(count: usize) -> egui_kittest::Harness<'static, View> {
        crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(320.0, 460.0))
            .build_ui_state(
                |ui, view: &mut View| {
                    if let Some(seek) = draw_transcript_panel_body(
                        ui,
                        &view.snapshot,
                        view.position,
                        &mut view.state,
                    ) {
                        view.seek = Some(seek);
                    }
                },
                View {
                    snapshot: fixture(count),
                    state: Default::default(),
                    seek: None,
                    position: 11.5,
                },
            )
    }
    #[test]
    fn transcript_panel_5000_rows_only_measures_visible_and_wraps_text() {
        let mut harness = harness(5000);
        harness.run_steps(6);
        let state = &harness.state().state;
        assert_eq!(state.heights.values.len(), 5000);
        assert!(
            state.measured_rows < 50,
            "全区間をlayoutしない: {}",
            state.measured_rows
        );
        assert!(state.heights.values[0] > state.metrics.as_ref().unwrap().font.size * 2.0);
        state.last_scroll.unwrap().assert_height(
            crate::test_ui::scroll_layout::HeightExpectation::Scrolls,
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 460.0)),
            1.0,
        );
    }

    #[test]
    #[ignore = "明示実行する共有一覧のperf測定。製品exeやAppは起動しない"]
    fn transcript_panel_long_list_perf() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("transcript-render.jsonl");
        crate::perf::init_with_path(true, None, Some(log.clone()));
        let mut harness = harness(5000);
        harness.run_steps(8);
        // 初回のfont/layout準備を含む記録と、準備後の連続表示を分けて報告する。
        crate::perf::event("transcript", "display_warmup_end", None, 0, &[]);
        for frame in 0..120 {
            harness.state_mut().position = 11.5 + ((frame * 41) % 4999) as f64;
            harness.run_steps(1);
        }
        // test threadだけでwriterの完了を待つ。通常のUI用flushは非同期のため使わない。
        assert!(crate::log_writer::shutdown_until(
            std::time::Instant::now() + std::time::Duration::from_secs(2)
        ));
        let records: Vec<serde_json::Value> = std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let warmup_samples = records
            .iter()
            .take_while(|entry| entry["kind"] != "display_warmup_end")
            .filter(|entry| entry["kind"] == "display_render")
            .count();
        let entries: Vec<_> = records
            .iter()
            .filter(|entry| entry["kind"] == "display_render")
            .collect();
        assert!(entries.len() >= 120);
        let max_layout = entries
            .iter()
            .map(|entry| entry["layout_rows"].as_u64().unwrap())
            .max()
            .unwrap();
        assert!(max_layout < 50, "可視範囲だけlayoutする: {max_layout}");
        let all_max_ms = entries
            .iter()
            .map(|entry| entry["ms"].as_f64().unwrap())
            .max_by(f64::total_cmp)
            .unwrap();
        let mut ms: Vec<_> = entries
            .iter()
            .skip(warmup_samples)
            .map(|entry| entry["ms"].as_f64().unwrap())
            .collect();
        assert!(ms.len() >= 120);
        ms.sort_by(f64::total_cmp);
        println!(
            "transcript_render_perf {}",
            serde_json::json!({"rows":5000,"warmup_samples":warmup_samples,"samples":ms.len(),"max_layout_rows":max_layout,"median_ms":ms[ms.len()/2],"p95_ms":ms[(ms.len()*95/100).min(ms.len()-1)],"max_ms":ms.last(),"including_warmup_max_ms":all_max_ms})
        );
    }
    #[test]
    fn transcript_panel_row_click_returns_existing_playback_time_and_unknown_mapping_disables_seek()
    {
        for known in [true, false] {
            let mut harness = harness(3);
            if !known {
                harness.state_mut().snapshot.time_mapping = Default::default();
            }
            harness.run_steps(6);
            harness.get_by_label("00:01 行0：幅に合わせて折り返す日本語の文字起こしです。長い文章も全文を表示します。").click();
            harness.run_steps(2);
            assert_eq!(harness.state().seek, known.then_some(11.0));
        }
    }
    #[test]
    fn transcript_panel_wheel_pauses_follow_until_resume_and_source_change_resets() {
        let mut harness = harness(5000);
        harness.run_steps(6);
        harness.event(egui::Event::PointerMoved(egui::pos2(150.0, 200.0)));
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -160.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
        });
        harness.run_steps(3);
        assert!(matches!(harness.state().state.mode, ScrollMode::Manual(_)));
        harness.state_mut().position = 4011.5;
        harness.run_steps(2);
        assert!(matches!(harness.state().state.mode, ScrollMode::Manual(_)));
        harness.get_by_label("再生位置へ戻る").click();
        harness.run_steps(4);
        assert!(matches!(
            harness.state().state.mode,
            ScrollMode::Following(_)
        ));
        harness.state_mut().state.mode = ScrollMode::Manual(Anchor {
            row: 4000,
            inset: 2.0,
        });
        harness.state_mut().snapshot.key = "other.mp4".into();
        harness.run_steps(3);
        assert!(matches!(
            harness.state().state.mode,
            ScrollMode::Following(_)
        ));
    }
    #[test]
    fn transcript_panel_width_change_keeps_manual_row_anchor() {
        let mut harness = harness(5000);
        harness.run_steps(6);
        harness.state_mut().state.mode = ScrollMode::Manual(Anchor {
            row: 1000,
            inset: 0.0,
        });
        harness.state_mut().state.geometry_scroll = Some(Anchor {
            row: 1000,
            inset: 0.0,
        });
        harness.set_size(egui::vec2(260.0, 460.0));
        harness.run_steps(5);
        let ScrollMode::Manual(anchor) = harness.state().state.mode else {
            panic!("手動送りを保つ")
        };
        assert_eq!(anchor.row, 1000);
    }
    #[test]
    fn transcript_panel_snapshots() {
        let mut snapshots = egui_kittest::SnapshotResults::default();
        for dark in [true, false] {
            let mut snapshot = fixture(12);
            snapshot.state = crate::transcript_display::TranscriptDataState::Stale;
            let mut harness = crate::test_ui::builder()
                .with_context_setup(crate::ui_fonts::configure_fonts)
                .with_size(egui::vec2(320.0, 460.0))
                .build_ui_state(
                    move |ui, state| {
                        crate::os_theme::apply_resolved(
                            ui.ctx(),
                            if dark {
                                crate::os_theme::ResolvedTheme::Dark
                            } else {
                                crate::os_theme::ResolvedTheme::Light
                            },
                        );
                        draw_transcript_panel_body(ui, &snapshot, 11.5, state);
                    },
                    TranscriptPanelState::default(),
                );
            harness.run_steps(6);
            harness.snapshot(if dark {
                "transcript_panel_dark"
            } else {
                "transcript_panel_light"
            });
            snapshots.extend(harness.take_snapshot_results());
        }
        snapshots.unwrap();
    }
}
