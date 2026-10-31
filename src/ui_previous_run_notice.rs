//! Modeless presentation of an immutable, already formatted previous-run report.
//! The diagnostics owner decides when to present it and retains the closed state.

const COPY_LABEL: &str = "報告用の文をコピー";
const CLOSE_LABEL: &str = "閉じる";
const DETAILS_LABEL: &str = "詳細（報告用）";
use crate::previous_run_diagnostics::PreviousRunReport;

/// Returns true only when the user closes this notice. No I/O or live diagnostics
/// are performed here; copying requires an explicit click and uses the displayed text.
pub(crate) fn draw(ctx: &egui::Context, report: &PreviousRunReport) -> bool {
    draw_notice(ctx, report).closed
}

#[derive(Default)]
struct DrawResult {
    closed: bool,
    #[cfg(test)]
    body: Option<crate::test_ui::scroll_layout::ScrollLayout>,
    #[cfg(test)]
    controls: Vec<egui::Response>,
    #[cfg(test)]
    heading: Option<egui::Response>,
    #[cfg(test)]
    summary: Option<egui::Rect>,
}

fn draw_notice(ctx: &egui::Context, report: &PreviousRunReport) -> DrawResult {
    let viewport = ctx.content_rect();
    let size = egui::vec2(
        600.0_f32.min((viewport.width() - 40.0).max(1.0)),
        // Window sizes include its title and frame. Let a long report use the
        // available screen; ScrollArea still shrinks a short report naturally.
        (viewport.height() - 40.0).max(1.0),
    );
    let mut open = true;
    let mut result = DrawResult::default();
    egui::Window::new("前回の終了について")
        .id(egui::Id::unique("previous_run_notice"))
        .order(egui::Order::Foreground)
        .open(&mut open)
        .collapsible(false)
        .default_pos(viewport.min + egui::vec2(20.0, 20.0))
        .default_size(size)
        .max_size(size)
        .min_size(egui::vec2(1.0, 1.0))
        .show(ctx, |ui| {
            ui.spacing_mut().scroll.floating_allocated_width =
                ui.spacing().scroll.bar_inner_margin + ui.spacing().scroll.bar_width;
            // The required small-screen sizes fit both buttons in one row.
            // Measure the actual button font rather than assuming a line height.
            let action_height = [COPY_LABEL, CLOSE_LABEL]
                .into_iter()
                .map(|label| {
                    egui::WidgetText::from(label)
                        .into_galley(ui, None, f32::INFINITY, egui::TextStyle::Button)
                        .size()
                        .y
                        + 2.0 * ui.spacing().button_padding.y
                })
                .fold(ui.spacing().interact_size.y, f32::max);
            let separator_height = 8.0;
            let footer_height =
                action_height + separator_height + 2.0 * ui.spacing().item_spacing.y;
            let summary = ui.add(egui::Label::new(report.summary()).wrap());
            #[cfg(test)]
            {
                result.summary = Some(summary.rect);
            }
            #[cfg(not(test))]
            let _ = summary;
            let header = egui::CollapsingHeader::new(DETAILS_LABEL)
                .id_salt("previous_run_details")
                .default_open(false)
                .show(ui, |ui| {
                    let body_height = (ui.available_height() - footer_height)
                        .min(viewport.bottom() - 16.0 - ui.cursor().top() - footer_height)
                        .max(1.0);
                    let body = egui::ScrollArea::vertical()
                        .id_salt("previous_run_report_body")
                        .max_height(body_height)
                        .min_scrolled_height(0.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(report.details()).wrap());
                        });
                    #[cfg(test)]
                    {
                        result.body = Some(
                            crate::test_ui::scroll_layout::ScrollLayout::from_output(&body),
                        );
                    }
                    #[cfg(not(test))]
                    let _ = body;
                });
            #[cfg(test)]
            {
                result.heading = Some(header.header_response);
            }
            #[cfg(not(test))]
            let _ = header;
            ui.add(egui::Separator::default().spacing(separator_height));
            ui.horizontal(|ui| {
                let copy = ui.button(COPY_LABEL);
                if copy.clicked() {
                    ctx.copy_text(report.body().to_owned());
                }
                let close = ui.button(CLOSE_LABEL);
                result.closed = close.clicked();
                #[cfg(test)]
                {
                    result.controls = vec![copy, close];
                }
            });
        });
    result.closed |= !open;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os_theme::ResolvedTheme;
    use crate::test_ui::scroll_layout::{HeightExpectation, ScrollLayout};
    use egui_kittest::{Harness, kittest::Queryable};

    const SUMMARY: &str = "前回の正常終了を確認できませんでした。\n原因は記録されていません。強制終了・電源断などでも、この表示になります。";
    const DETAILS: &str = "最終記録時刻: 2026-10-30 19:23:45 (UTC+09:00) / 終了との間隔不明\nmIV 4.10.0 / 記録ID core-1234-5678\n最終記録以降に状態が変わった可能性があります。\n";
    fn fixture_report(long: bool) -> PreviousRunReport {
        let summary = if long {
            crate::previous_run_diagnostics::test_support::checkpoint_report()
                .summary()
                .to_owned()
        } else {
            SUMMARY.to_owned()
        };
        PreviousRunReport::from_sections(summary, DETAILS.repeat(if long { 20 } else { 1 }))
    }

    #[derive(Default)]
    struct State {
        result: DrawResult,
        closed: bool,
        copied: Vec<String>,
        watched: Option<egui::Id>,
        title_close: Option<egui::Response>,
    }

    fn harness(
        size: egui::Vec2,
        scale: f32,
        theme: ResolvedTheme,
        long: bool,
    ) -> Harness<'static, State> {
        let report = fixture_report(long);
        harness_report(size, scale, theme, report)
    }

    fn harness_report(
        size: egui::Vec2,
        scale: f32,
        theme: ResolvedTheme,
        report: PreviousRunReport,
    ) -> Harness<'static, State> {
        let mut fonts_ready = false;
        crate::test_ui::builder().with_size(size).build_ui_state(
            move |ui, state: &mut State| {
                let ctx = &ui.ctx().clone();
                crate::os_theme::apply_resolved(ctx, theme);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    crate::settings::apply_ui_scale_factor(ctx, scale);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                crate::test_ui::full_viewport_ui(ui, |ui| {
                    egui::CentralPanel::default().show(ui, |_| {});
                });
                state.result = draw_notice(ctx, &report);
                state.closed |= state.result.closed;
                state.copied.extend(ctx.output_mut(|out| {
                    std::mem::take(&mut out.commands)
                        .into_iter()
                        .filter_map(|command| match command {
                            egui::OutputCommand::CopyText(text) => Some(text),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                }));
                state.title_close = state.watched.and_then(|id| ctx.read_response(id));
            },
            State::default(),
        )
    }

    fn watch_title_close(h: &mut Harness<'_, State>) {
        use egui_kittest::kittest::NodeT;
        let node = h.get_by_label("Close window");
        let id = (u128::from(node.accesskit_node().id()) >> 64) as u64;
        h.state_mut().watched = Some(unsafe { egui::Id::from_high_entropy_bits(id) });
        h.step();
    }

    fn click(h: &mut Harness<'_, State>, pos: egui::Pos2) {
        h.hover_at(pos);
        h.run();
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            h.step();
        }
    }
    fn expand(h: &mut Harness<'_, State>) {
        let pos = h.state().result.heading.as_ref().unwrap().rect.center();
        click(h, pos);
        h.run_steps(24);
        assert!(h.state().result.body.is_some());
    }

    #[test]
    fn previous_run_notice_small_viewport_controls_and_explicit_copy() {
        for size in [egui::vec2(1093.0, 614.0), egui::vec2(1366.0, 728.0)] {
            for scale in [1.0, 2.0] {
                for theme in [ResolvedTheme::Light, ResolvedTheme::Dark] {
                    let mut h = harness(size, scale, theme, true);
                    h.run_steps(12);
                    watch_title_close(&mut h);
                    assert!(h.state().copied.is_empty(), "presentation must not copy");
                    assert!(!h.state().closed);
                    let viewport = h.ctx.content_rect();
                    assert!((viewport.size() * scale - size).length() < 1.0);
                    let controls = &h.state().result.controls;
                    for response in controls.iter().chain(h.state().title_close.iter()) {
                        assert!(
                            viewport.contains_rect(response.rect),
                            "{size:?}/{scale}/{theme:?}: {:?}",
                            response.rect
                        );
                        assert!(response.interact_rect.contains_rect(response.rect));
                    }
                    assert!(h.state().result.body.is_none(), "details default collapsed");
                    assert!(viewport.contains_rect(h.state().result.summary.unwrap()));
                    assert!(
                        viewport.contains_rect(h.state().result.heading.as_ref().unwrap().rect)
                    );
                    let copy_pos = controls[0].rect.center();
                    click(&mut h, copy_pos);
                    assert_eq!(h.state().copied, [fixture_report(true).body()]);
                    assert!(!h.state().closed);
                    expand(&mut h);
                    let body = h.state().result.body.unwrap();
                    body.assert_height(HeightExpectation::Scrolls, viewport, scale);
                    // After the always-visible summary and header, details use
                    // the remaining height; the fixed actions must still fit.
                    assert!(body.inner.height() > 20.0);
                    assert!(viewport.contains_rect(body.inner));
                    for response in &h.state().result.controls {
                        assert!(viewport.contains_rect(response.rect));
                    }
                    let close_pos = h.state().result.controls[1].rect.center();
                    click(&mut h, close_pos);
                    assert!(h.state().closed, "fixed close receives pointer");
                    assert_eq!(h.state().copied.len(), 1);

                    // A fresh presentation also accepts the native-looking title ×.
                    let mut h = harness(size, scale, theme, false);
                    h.run_steps(12);
                    watch_title_close(&mut h);
                    let close_pos = h.state().title_close.as_ref().unwrap().rect.center();
                    click(&mut h, close_pos);
                    assert!(h.state().closed, "title close receives pointer");
                    assert!(h.state().copied.is_empty());
                }
            }
        }
    }

    #[test]
    fn previous_run_notice_body_scroll_reaches_end_without_moving_actions() {
        let mut h = harness(egui::vec2(1093.0, 614.0), 2.0, ResolvedTheme::Dark, true);
        h.run_steps(12);
        expand(&mut h);
        let before: ScrollLayout = h.state().result.body.unwrap();
        let summary = h.state().result.summary;
        let heading = h.state().result.heading.as_ref().unwrap().rect;
        before.assert_height(HeightExpectation::Scrolls, h.ctx.content_rect(), 2.0);
        let controls: Vec<_> = h.state().result.controls.iter().map(|r| r.rect).collect();
        h.hover_at(before.inner.center());
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -100_000.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
        });
        h.run_steps(24);
        h.state()
            .result
            .body
            .unwrap()
            .assert_scrolled_to_end(before, 2.0);
        assert_eq!(
            h.state()
                .result
                .controls
                .iter()
                .map(|r| r.rect)
                .collect::<Vec<_>>(),
            controls
        );
        assert!(h.state().copied.is_empty());
        assert_eq!(h.state().result.summary, summary);
        assert_eq!(h.state().result.heading.as_ref().unwrap().rect, heading);
    }

    #[test]
    fn previous_run_notice_short_body_fits() {
        let mut h = harness(egui::vec2(1366.0, 728.0), 1.0, ResolvedTheme::Light, false);
        h.run_steps(12);
        expand(&mut h);
        h.state().result.body.unwrap().assert_height(
            HeightExpectation::Fits,
            h.ctx.content_rect(),
            1.0,
        );
    }

    #[test]
    fn previous_run_notice_snapshot_small_light() {
        let mut h = harness(egui::vec2(1093.0, 614.0), 1.0, ResolvedTheme::Light, false);
        h.run_steps(12);
        h.snapshot("previous_run_notice_small_light");
    }

    #[test]
    fn previous_run_notice_snapshot_small_200_dark() {
        let mut h = harness(egui::vec2(1093.0, 614.0), 2.0, ResolvedTheme::Dark, true);
        h.run_steps(12);
        h.snapshot("previous_run_notice_small_200_dark");
    }
    #[test]
    fn previous_run_notice_snapshot_checkpoint_small_light() {
        let report = crate::previous_run_diagnostics::test_support::checkpoint_report();
        let mut h = harness_report(
            egui::vec2(1093.0, 614.0),
            1.0,
            ResolvedTheme::Light,
            report.clone(),
        );
        h.run_steps(12);
        expand(&mut h);
        let viewport = h.ctx.content_rect();
        for control in &h.state().result.controls {
            assert!(viewport.contains_rect(control.rect));
        }
        h.snapshot("previous_run_notice_checkpoint_small_light");
        let copy_pos = h.state().result.controls[0].rect.center();
        click(&mut h, copy_pos);
        assert_eq!(h.state().copied, [report.body()]);
    }

    #[test]
    fn previous_run_notice_snapshot_observations_small_light() {
        let report = crate::previous_run_diagnostics::test_support::stage_c_report();
        let mut h = harness_report(
            egui::vec2(1093.0, 614.0),
            1.0,
            ResolvedTheme::Light,
            report.clone(),
        );
        h.run_steps(12);
        for control in &h.state().result.controls {
            assert!(h.ctx.content_rect().contains_rect(control.rect));
        }
        h.snapshot("previous_run_notice_observations_small_light");
        let copy = h.state().result.controls[0].rect.center();
        click(&mut h, copy);
        assert_eq!(h.state().copied, [report.body()]);
    }
}
