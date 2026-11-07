//! Pure 3D-photo export dialogs. The App owns admission, resources and I/O.
use crate::depth_export::{DepthExportFormat, DepthExportOptions};
use crate::depth_parallax::MotionKind;
use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub enum Action {
    Save,
    Cancel,
    Close,
}

pub(crate) enum Phase<'a> {
    Settings {
        options: &'a mut DepthExportOptions,
        source_long_edge: u32,
        period_secs: f32,
        save_enabled: bool,
    },
    Progress {
        completed: u32,
        total: u32,
        stopping: bool,
    },
    Finished {
        message: &'a str,
        failed: bool,
    },
}

pub struct DialogResponse {
    pub rect: egui::Rect,
    pub action_rects: Vec<egui::Rect>,
    pub scroll: Option<egui::scroll_area::ScrollAreaOutput<()>>,
    pub action: Option<Action>,
}

/// Include a source-size choice when every standard preset would enlarge it.
pub(crate) fn long_edge_choices(source: u32) -> Vec<u32> {
    let choices: Vec<_> = [720, 1080, 1280, 1600, 2160]
        .into_iter()
        .filter(|&edge| edge <= source)
        .collect();
    if choices.is_empty() {
        vec![source.max(1)]
    } else {
        choices
    }
}

pub(crate) fn default_long_edge(format: DepthExportFormat, source: u32) -> u32 {
    let choices = long_edge_choices(source);
    choices
        .iter()
        .copied()
        .filter(|&edge| edge <= format.default_long_edge())
        .next_back()
        .unwrap_or(choices[0])
}

pub(crate) fn draw(ctx: &egui::Context, mut phase: Phase<'_>, escape: bool) -> DialogResponse {
    let _ = egui::Modal::new(egui::Id::unique("depth_export_barrier"))
        .frame(egui::Frame::NONE)
        .backdrop_color(egui::Color32::TRANSPARENT)
        .show(ctx, |_| {});
    let screen = ctx.content_rect();
    let width = 420.0_f32.min((screen.width() - 40.0).max(160.0));
    let settings = matches!(phase, Phase::Settings { .. });
    let save_enabled = matches!(
        phase,
        Phase::Settings {
            save_enabled: true,
            ..
        }
    );
    let finished = matches!(phase, Phase::Finished { .. });
    let height = (if settings {
        340.0_f32
    } else if finished {
        220.0
    } else {
        170.0
    })
    .min((screen.height() - 70.0).max(100.0));
    let stopping = matches!(phase, Phase::Progress { stopping: true, .. });
    let mut open = true;
    let mut response = DialogResponse {
        rect: egui::Rect::NOTHING,
        action_rects: vec![],
        scroll: None,
        action: None,
    };
    crate::os_theme::with_dark_context_style(ctx, || {
        let window = egui::Window::new(if settings {
            "3D 写真の書き出し"
        } else {
            "3D 写真の書き出し状況"
        })
        .id(egui::Id::unique("depth_export_dialog"))
        .order(egui::Order::Foreground)
        .drag_area(egui::WindowDrag::Anywhere)
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_pos(screen.center() - egui::vec2(width, height) * 0.5)
        .default_size(egui::vec2(width, height))
        .max_size(egui::vec2(width, height))
        .min_width(160.0)
        .min_height(0.0)
        .show(ctx, |ui| {
            ui.style_mut().spacing.scroll.floating_allocated_width =
                ui.spacing().scroll.bar_inner_margin + ui.spacing().scroll.bar_width;
            let action_height = ui.spacing().interact_size.y
                + ui.spacing().item_spacing.y * 2.0
                + ui.spacing().interact_size.y * 0.25;
            let body_height = (ui.available_height() - action_height).max(0.0);
            let mut draw_body = |ui: &mut egui::Ui| match &mut phase {
                Phase::Settings {
                    options,
                    source_long_edge,
                    period_secs,
                    ..
                } => {
                    ui.label("表示中の3D写真を、繰り返す動画として保存します。");
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label("形式");
                        for (format, label) in [
                            (DepthExportFormat::Mp4, "MP4"),
                            (DepthExportFormat::Gif, "GIF"),
                            (DepthExportFormat::Apng, "APNG"),
                        ] {
                            if ui
                                .selectable_value(&mut options.format, format, label)
                                .changed()
                            {
                                options.long_edge = default_long_edge(format, *source_long_edge);
                            }
                        }
                    });
                    if options.format == DepthExportFormat::Apng {
                        ui.weak("APNG はファイルが大きくなります。");
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label("長辺");
                        for edge in long_edge_choices(*source_long_edge) {
                            ui.selectable_value(&mut options.long_edge, edge, format!("{edge} px"));
                        }
                    });
                    ui.label("動き");
                    ui.horizontal_wrapped(|ui| {
                        for motion in [
                            MotionKind::Horizontal,
                            MotionKind::Circle,
                            MotionKind::FigureEight,
                            MotionKind::Vertical,
                        ] {
                            ui.selectable_value(&mut options.motion, motion, motion.label());
                        }
                    });
                    if options.format == DepthExportFormat::Mp4 {
                        ui.horizontal_wrapped(|ui| {
                            let label = ui.label("ループ回数");
                            ui.add(egui::DragValue::new(&mut options.loops).range(1..=10))
                                .labelled_by(label.id);
                            ui.label(format!(
                                "（約 {:.1} 秒）",
                                *period_secs * options.loops as f32
                            ));
                        });
                    }
                    ui.add_space(6.0);
                    ui.weak("強さ・周期・止まる深さ・奥行きの整形は、現在の設定を使います。");
                }
                Phase::Progress {
                    completed,
                    total,
                    stopping,
                } => {
                    ui.label(if *stopping {
                        "キャンセルしています…"
                    } else {
                        "3D 写真を保存しています…"
                    });
                    ui.add(
                        egui::ProgressBar::new(*completed as f32 / (*total).max(1) as f32)
                            .text(format!("{completed} / {total} フレーム")),
                    );
                }
                Phase::Finished { message, failed } => {
                    if *failed {
                        ui.colored_label(ui.visuals().error_fg_color, *message);
                    } else {
                        ui.label(*message);
                    }
                }
            };
            if settings || finished {
                // Reserve the same viewport that ScrollArea reports; natural Window
                // sizing must not shrink underneath a larger input/clip rectangle.
                response.scroll = Some(
                    egui::ScrollArea::vertical()
                        .id_salt("depth_export_body")
                        .auto_shrink([false, false])
                        .max_height(body_height)
                        .show(ui, draw_body),
                );
            } else {
                // Progress is short and bounded. Keep its natural height.
                draw_body(ui);
            }
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if settings {
                    let save = ui
                        .add_enabled(save_enabled, egui::Button::new("保存先を選ぶ…"))
                        .on_disabled_hover_text(
                            "表示の準備中です。奥行きの計算が終わると保存できます。",
                        );
                    response.action_rects.push(save.rect);
                    if save.clicked() {
                        response.action = Some(Action::Save);
                    }
                }
                let close = ui.add_enabled(
                    !stopping,
                    egui::Button::new(if finished {
                        "閉じる"
                    } else {
                        "キャンセル"
                    }),
                );
                response.action_rects.push(close.rect);
                if close.clicked() {
                    response.action = Some(if finished {
                        Action::Close
                    } else {
                        Action::Cancel
                    });
                }
            });
        });
        if let Some(window) = window {
            response.rect = window.response.rect;
        }
    });
    if (!open || escape) && !stopping {
        response.action = Some(if finished {
            Action::Close
        } else {
            Action::Cancel
        });
    }
    response
}

#[doc(hidden)]
pub fn draw_snapshot_fixture(ctx: &egui::Context, progress: bool) -> DialogResponse {
    let mut options = DepthExportOptions::default();
    if progress {
        draw(
            ctx,
            Phase::Progress {
                completed: 120,
                total: 360,
                stopping: false,
            },
            false,
        )
    } else {
        draw(
            ctx,
            Phase::Settings {
                options: &mut options,
                source_long_edge: 2000,
                period_secs: 3.0,
                save_enabled: true,
            },
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn depth_export_long_edge_choices_cover_each_threshold_without_enlarging() {
        for source in [
            0, 1, 360, 719, 720, 721, 1079, 1080, 1081, 1279, 1280, 1281, 1599, 1600, 1601, 2159,
            2160, 2161, 4000,
        ] {
            let expected = if source < 720 {
                vec![source.max(1)]
            } else if source < 1080 {
                vec![720]
            } else if source < 1280 {
                vec![720, 1080]
            } else if source < 1600 {
                vec![720, 1080, 1280]
            } else if source < 2160 {
                vec![720, 1080, 1280, 1600]
            } else {
                vec![720, 1080, 1280, 1600, 2160]
            };
            assert_eq!(long_edge_choices(source), expected, "source={source}");
        }
    }

    #[test]
    fn depth_export_format_switch_resets_long_edge_to_bounded_default() {
        for (source, expected) in [
            (4000, [720, 1080, 1280]),
            (1280, [720, 1080, 1280]),
            (1279, [720, 1080, 1080]),
            (1000, [720, 720, 720]),
            (600, [600, 600, 600]),
        ] {
            let mut harness = crate::test_ui::builder()
                .with_context_setup(crate::ui_fonts::configure_fonts)
                .with_size(egui::vec2(1093.0, 614.0))
                .build_ui_state(
                    move |ui, options: &mut DepthExportOptions| {
                        draw(
                            ui.ctx(),
                            Phase::Settings {
                                options,
                                source_long_edge: source,
                                period_secs: 3.0,
                                save_enabled: true,
                            },
                            false,
                        );
                    },
                    DepthExportOptions {
                        long_edge: default_long_edge(DepthExportFormat::Mp4, source),
                        ..DepthExportOptions::default()
                    },
                );
            harness.run();
            assert_eq!(harness.state().long_edge, expected[2]);
            for (format, edge) in [
                DepthExportFormat::Gif,
                DepthExportFormat::Apng,
                DepthExportFormat::Mp4,
            ]
            .into_iter()
            .zip(expected)
            {
                // Exercise a manual size choice before every format change.
                let largest = *long_edge_choices(source).last().unwrap();
                harness.get_by_label(&format!("{largest} px")).click();
                harness.run();
                assert_eq!(harness.state().long_edge, largest);
                harness.get_by_label(format.label()).click();
                harness.run();
                assert_eq!(harness.state().format, format);
                assert_eq!(
                    harness.state().long_edge,
                    edge,
                    "{format:?}, source={source}"
                );
                assert_eq!(harness.state().loops, 4);
            }
            // Re-selecting the current format must preserve a manual choice.
            let largest = *long_edge_choices(source).last().unwrap();
            harness.get_by_label(&format!("{largest} px")).click();
            harness.run();
            harness.get_by_label("MP4").click();
            harness.run();
            assert_eq!(harness.state().long_edge, largest);
        }
    }
}
