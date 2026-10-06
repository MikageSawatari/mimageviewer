//! `App` のダイアログ・オーバーレイ表示メソッドを集めたサブモジュール。
//!
//! 各ファイルは `impl crate::app::App { fn show_xxx_dialog(...) {...} }` の
//! 形でメソッドを 1 つだけ提供する。これらのメソッドは `App::update()` から
//! 呼び出される。
//!
//! ダイアログを増やしたい場合は、ここに新しい .rs を追加し、`mod` 宣言を
//! 加えるだけで `update()` から `self.show_new_dialog(ctx)` として呼べる。

mod about;
#[doc(hidden)]
pub use about::draw_raw_license_snapshot_fixture;
pub(crate) mod archive_cache_manager;
pub(crate) mod archive_convert;
pub(crate) mod batch_convert;
mod cache_creator;
mod cache_manager;
pub(crate) mod collections;
pub mod content_restore;
pub(crate) mod context_menu;
mod context_shortcuts;
pub(crate) mod editing_addon;
pub(crate) mod epub_batch_convert;
pub(crate) mod epub_convert;
pub(crate) mod export_batch;
mod fav_add;
pub(crate) mod favorites_editor;
pub mod file_organize;
mod first_setup;
#[doc(hidden)]
pub use first_setup::draw_first_setup_dialog;
mod metadata_cleanup;
pub(crate) mod metadata_transfer;
pub mod network_data_dir_notice;
pub(crate) mod new_folder;
mod open_folder;
mod pdf_password;
mod pdf_worker_notice;
pub(crate) mod preferences;
pub(crate) mod rename_item;
mod rename_migration_recovery;
mod rotation_reset;
mod settings_incompatible;
pub(crate) mod settings_restore;
pub(crate) mod sidecar_restore;
pub(crate) mod smart_folder_editor;
mod stats_dialog;
mod subfolder_expansion;
mod susie_worker_notice;
mod tag_apply;
mod tag_editor;
mod thumb_quality;
mod thumb_quality_fullscreen;
pub(crate) mod trt_install;
pub(crate) mod trt_worker_notice;
mod update_notice;
pub(crate) mod video_upscale;
#[cfg(windows)]
mod vst3_actions;
#[cfg(windows)]
mod vst3_manager;
mod whats_new;

/// 長文ダイアログの縦スクロールバーが本文へ重ならないよう、floating bar の最大幅を
/// viewport の右側へ予約する。アプリ共通 style は一覧の表示面積を優先して floating の
/// 予約幅を 0 にしているため、折り返し本文を持つダイアログだけで局所適用する。
fn non_overlapping_dialog_scroll_style(
    mut scroll: eframe::egui::style::ScrollStyle,
) -> eframe::egui::style::ScrollStyle {
    if scroll.floating {
        scroll.floating_allocated_width = scroll
            .floating_allocated_width
            .max(scroll.bar_inner_margin + scroll.bar_width);
    }
    scroll
}

/// Measure the fixed action row using the same font/padding as ordinary buttons.
/// `spacing` includes the caller's explicit gaps and separator height.
fn startup_dialog_footer_height(ui: &eframe::egui::Ui, labels: &[&str], spacing: f32) -> f32 {
    startup_dialog_button_rows_height(ui, labels)
        + spacing
        + 3.0 * ui.spacing().item_spacing.y
        + 2.0
}

fn startup_dialog_button_rows_height(ui: &eframe::egui::Ui, labels: &[&str]) -> f32 {
    use eframe::egui;
    let mut height = 0.0_f32;
    let mut row_height = 0.0_f32;
    let mut row_width = 0.0_f32;
    for &label in labels {
        let size = egui::WidgetText::from(label)
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Wrap),
                (ui.available_width() - 2.0 * ui.spacing().button_padding.x).max(1.0),
                egui::TextStyle::Button,
            )
            .size()
            + 2.0 * ui.spacing().button_padding;
        if row_width > 0.0
            && row_width + ui.spacing().item_spacing.x + size.x > ui.available_width()
        {
            height += row_height + ui.spacing().item_spacing.y;
            row_width = 0.0;
            row_height = 0.0;
        }
        if row_width > 0.0 {
            row_width += ui.spacing().item_spacing.x;
        }
        row_width += size.x;
        row_height = row_height.max(size.y.max(ui.spacing().interact_size.y));
    }
    height + row_height
}

/// A caption uses Body text without button padding. Reserve its wrapped height
/// separately unless the entire caption and action row fit on one line.
fn startup_dialog_captioned_footer_height(
    ui: &eframe::egui::Ui,
    caption: &str,
    labels: &[&str],
    spacing: f32,
) -> f32 {
    use eframe::egui;
    let caption_size = egui::WidgetText::from(caption)
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Wrap),
            ui.available_width().max(1.0),
            egui::TextStyle::Body,
        )
        .size();
    let buttons_width: f32 = labels
        .iter()
        .map(|label| {
            egui::WidgetText::from(*label)
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Extend),
                    f32::INFINITY,
                    egui::TextStyle::Button,
                )
                .size()
                .x
                + 2.0 * ui.spacing().button_padding.x
                + ui.spacing().item_spacing.x
        })
        .sum();
    let buttons_height = startup_dialog_button_rows_height(ui, labels);
    let rows_height = if caption_size.x + buttons_width <= ui.available_width() {
        caption_size.y.max(buttons_height)
    } else {
        caption_size.y + ui.spacing().item_spacing.y + buttons_height
    };
    rows_height + spacing + 3.0 * ui.spacing().item_spacing.y + 2.0
}

/// Give overflowing content the available screen, and let short content shrink.
/// The explicit child max_rect prevents auto-sized Areas inheriting a tiny body.
fn startup_dialog_scroll_body<R>(
    ui: &mut eframe::egui::Ui,
    id: &str,
    footer_height: f32,
    body: impl FnOnce(&mut eframe::egui::Ui) -> R,
) -> eframe::egui::scroll_area::ScrollAreaOutput<R> {
    startup_dialog_scroll_body_with_axes(ui, id, footer_height, [false, true], body)
}

fn startup_dialog_scroll_body_with_axes<R>(
    ui: &mut eframe::egui::Ui,
    id: &str,
    footer_height: f32,
    axes: [bool; 2],
    body: impl FnOnce(&mut eframe::egui::Ui) -> R,
) -> eframe::egui::scroll_area::ScrollAreaOutput<R> {
    use eframe::egui;
    let viewport = ui.ctx().content_rect();
    let frame_margin = ui
        .stack()
        .iter()
        .find(|stack| stack.kind() == Some(egui::UiKind::Frame))
        .map_or(egui::epaint::MarginF32::ZERO, |stack| {
            stack.frame().total_margin()
        });
    let header_height = ui.cursor().top() - ui.min_rect().top();
    let mut height =
        viewport.height() - 32.0 - frame_margin.sum().y - header_height - footer_height;
    if ui.stack().contained_in(egui::UiKind::Window) {
        // A positioned/resized Window has an authoritative content rectangle.
        // Absolute cursor coordinates are inappropriate for centered Modals.
        height = height
            .min(viewport.bottom() - 16.0 - frame_margin.bottom - ui.cursor().top() - footer_height)
            .min(ui.available_height() - footer_height);
    }
    let height = height.max(1.0);
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), height),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.spacing_mut().scroll = non_overlapping_dialog_scroll_style(ui.spacing().scroll);
            let scroll = egui::ScrollArea::new(axes)
                .id_salt(id)
                .max_height(height)
                .min_scrolled_height(0.0)
                .auto_shrink([false, true])
                .show(ui, body);
            if ui.rect_contains_pointer(scroll.inner_rect) {
                ui.ctx().input_mut(|input| {
                    input.raw_scroll_delta = egui::Vec2::ZERO;
                    input.smooth_scroll_delta = egui::Vec2::ZERO;
                    input
                        .events
                        .retain(|event| !matches!(event, egui::Event::MouseWheel { .. }));
                });
            }
            scroll
        },
    )
    .inner
}

fn draw_startup_worker_notice(
    ctx: &eframe::egui::Context,
    title: &str,
    id: &str,
    width: f32,
    body: &str,
    retry_label: Option<&str>,
    resizable: bool,
) -> (bool, bool) {
    use eframe::egui;
    let content = ctx.content_rect();
    let mut retry = false;
    let mut close = false;
    egui::Window::new(title)
        .id(egui::Id::new(id))
        .default_pos(egui::pos2(
            content.max.x - width - 36.0,
            content.min.y + 56.0,
        ))
        .default_width(width)
        .default_height((ctx.content_rect().height() - 80.0).max(1.0))
        .resizable(resizable)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let labels = retry_label.map_or_else(|| vec!["閉じる"], |retry| vec![retry, "閉じる"]);
            let footer = startup_dialog_footer_height(ui, &labels, 6.0);
            startup_dialog_scroll_body(ui, id, footer, |ui| {
                ui.label(body);
            });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if let Some(label) = retry_label {
                    retry = ui.button(label).clicked();
                }
                close = ui.button("閉じる").clicked();
            });
        });
    (retry, close)
}

/// Pure startup dialog fixtures: no App, settings I/O, network, or worker startup.
#[doc(hidden)]
pub fn draw_startup_dialog_snapshot_fixture(ui: &mut eframe::egui::Ui, kind: &str) {
    let ctx = ui.ctx();
    let mut open = true;
    match kind {
        "first_setup" => {
            draw_first_setup_dialog(ctx, &mut crate::settings::Settings::default());
        }
        "boot_incompatible" | "boot_unreadable" => {
            settings_incompatible::draw_boot_problem_snapshot_fixture(
                ctx,
                kind == "boot_incompatible",
            );
        }
        "whats_new" => {
            let mut entries: Vec<_> = crate::version_highlights::table().iter().collect();
            entries.reverse();
            whats_new::draw_whats_new_dialog(ctx, &mut open, &entries);
        }
        "update_notice" | "update_current" | "update_error" => {
            let info = crate::update_check::UpdateInfo {
                latest_tag: "v99.0.0".to_owned(),
                latest_version: semver::Version::new(99, 0, 0),
                release_url: String::new(),
                body: "## 更新内容\n- 表示の改善\n".repeat(40),
                is_newer: kind != "update_current",
            };
            update_notice::draw_update_notice_dialog(
                ctx,
                &mut open,
                (kind != "update_error").then_some(&info),
                (kind != "update_current").then_some(&"接続エラーの詳細。".repeat(100)),
            );
        }
        "network_data_dir" => {
            let path =
                std::path::PathBuf::from(format!(r"\\server\share\{}", "長い保存先\\".repeat(40)));
            network_data_dir_notice::draw_network_data_dir_notice_dialog(ctx, &mut open, &path);
        }
        "restore_result" | "restore_success" | "restore_recoverable" | "restore_remote" => {
            settings_restore::draw_restore_result_snapshot_fixture(ctx, kind)
        }
        "restore_list" => settings_restore::draw_restore_list_snapshot_fixture(ctx),
        "restore_confirm" | "restore_reset" => {
            settings_restore::draw_restore_confirm_snapshot_fixture(ctx, kind == "restore_reset")
        }
        "mouse_migration" => {
            draw_mouse_nav_migration_dialog(ctx, &mut open);
        }
        "rename_recovery" | "rename_quarantining" => {
            rename_migration_recovery::draw_rename_recovery_dialog(
                ctx,
                kind == "rename_quarantining",
            );
        }
        "archive_confirm" | "archive_empty" | "archive_sibling" | "archive_scanning"
        | "archive_converting" | "archive_error" => {
            archive_convert::draw_archive_startup_snapshot_fixture(ctx, kind)
        }
        "pdf_notice" | "susie_notice" | "trt_notice" => {
            let (title, width, retry) = match kind {
                "pdf_notice" => ("PDF の準備を開始できませんでした", 420.0, None),
                "susie_notice" => ("Susie プラグインでの読み込みを打ち切りました", 420.0, None),
                _ => ("TensorRT 起動失敗", 360.0, Some("ワーカーを再起動")),
            };
            draw_startup_worker_notice(
                ctx,
                title,
                kind,
                width,
                &"準備に失敗しました。詳細: 読み込みエラー。\n".repeat(80),
                retry,
                kind != "trt_notice",
            );
        }
        _ => panic!("unknown startup snapshot fixture: {kind}"),
    }
}

#[cfg(test)]
mod tests {
    use super::non_overlapping_dialog_scroll_style;

    #[test]
    fn startup_dialog_body_grows_for_overflow_and_shrinks_to_content() {
        use eframe::egui;
        use egui_kittest::Harness;
        for size in [egui::vec2(1093.0, 614.0), egui::vec2(1920.0, 1440.0)] {
            for scale in [1.0, 2.0] {
                for long in [false, true] {
                    let mut harness = Harness::builder().with_size(size).build_state(
                        move |ctx, rects: &mut Option<(f32, f32, egui::Rect)>| {
                            crate::settings::apply_ui_scale_factor(ctx, scale);
                            egui::Modal::new(egui::Id::new("body_sizing")).show(ctx, |ui| {
                                ui.set_width(480.0_f32.min(ctx.content_rect().width() - 48.0));
                                ui.heading("Heading");
                                ui.add_space(8.0);
                                ui.label("Choose the initial settings.");
                                let footer =
                                    super::startup_dialog_footer_height(ui, &["Start"], 26.0);
                                let body = super::startup_dialog_scroll_body(
                                    ui,
                                    "sizing_body",
                                    footer,
                                    |ui| {
                                        ui.label(if long {
                                            "Long message\n".repeat(120)
                                        } else {
                                            "Short message".to_owned()
                                        });
                                    },
                                );
                                // ScrollAreaOutput.inner_rect precedes auto_shrink.
                                // Measure the actual allocation before drawing the footer.
                                let body_height = ui.cursor().top()
                                    - body.inner_rect.top()
                                    - ui.spacing().item_spacing.y;
                                ui.add_space(14.0);
                                ui.separator();
                                ui.add_space(6.0);
                                let button = ui.button("Start");
                                *rects = Some((body_height, body.content_size.y, button.rect));
                            });
                        },
                        None,
                    );
                    harness.run_steps(12);
                    let (body_height, content_height, button) = harness.state().unwrap();
                    let viewport = harness.ctx.content_rect();
                    assert!(viewport.contains_rect(button));
                    if long {
                        assert!(content_height > body_height);
                        // A one-line viewport passed button-only geometry tests.
                        assert!(
                            body_height >= viewport.height() * 0.45,
                            "{size:?} scale {scale}: body {body_height}, viewport {viewport:?}"
                        );
                    } else {
                        assert!(
                            (body_height - content_height).abs() < 1.0,
                            "{size:?} scale {scale}: short body {body_height}, content {content_height}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn startup_dialog_short_window_keeps_natural_height() {
        use eframe::egui;
        use egui_kittest::Harness;
        for size in [egui::vec2(1093.0, 614.0), egui::vec2(1920.0, 1440.0)] {
            for scale in [1.0, 2.0] {
                let mut harness = Harness::builder().with_size(size).build_state(
                    move |ctx, height: &mut Option<f32>| {
                        crate::settings::apply_ui_scale_factor(ctx, scale);
                        let response = egui::Window::new("Short startup notice")
                            .default_width(460.0)
                            .default_height(ctx.content_rect().height() - 80.0)
                            .default_pos(egui::pos2(60.0, 40.0))
                            .show(ctx, |ui| {
                                let footer =
                                    super::startup_dialog_footer_height(ui, &["Close"], 0.0);
                                super::startup_dialog_scroll_body(
                                    ui,
                                    "short_window_body",
                                    footer,
                                    |ui| {
                                        ui.label("Short message");
                                    },
                                );
                                ui.button("Close");
                            })
                            .unwrap();
                        *height = Some(response.response.rect.height());
                    },
                    None,
                );
                harness.run_steps(12);
                assert!(
                    harness.state().unwrap() < 110.0,
                    "{size:?} scale {scale}: {:?}",
                    harness.state()
                );
            }
        }
    }

    #[test]
    fn startup_dialog_wrapped_button_remains_inside_narrow_window() {
        use eframe::egui;
        use egui_kittest::Harness;
        for scale in [1.0, 2.0] {
            let mut harness = Harness::builder()
                .with_size(egui::vec2(1093.0, 614.0))
                .build_state(
                    move |ctx, rects: &mut Option<(egui::Rect, egui::Rect)>| {
                        crate::settings::apply_ui_scale_factor(ctx, scale);
                        egui::Window::new("Narrow startup notice")
                            .fixed_size(egui::vec2(200.0, ctx.content_rect().height() - 80.0))
                            .default_pos(egui::pos2(60.0, 40.0))
                            .show(ctx, |ui| {
                                let label = "Confirm these initial settings and continue opening the selected archive";
                                let footer = super::startup_dialog_footer_height(ui, &[label], 0.0);
                                super::startup_dialog_scroll_body(ui, "wrapped_action_body", footer, |ui| {
                                    ui.label("Long message\n".repeat(80));
                                });
                                ui.horizontal_wrapped(|ui| {
                                    let button = ui.button(label);
                                    assert!(button.rect.height() > ui.spacing().interact_size.y);
                                    *rects = Some((button.rect, button.interact_rect));
                                });
                            });
                    },
                    None,
                );
            harness.run_steps(12);
            let (rect, interact_rect) = harness.state().unwrap();
            assert!(harness.ctx.content_rect().contains_rect(rect));
            assert!(
                interact_rect.contains_rect(rect),
                "scale {scale}: {rect:?} / {interact_rect:?}"
            );
        }
    }

    #[test]
    fn startup_dialog_body_respects_window_height() {
        use eframe::egui;
        use egui_kittest::Harness;
        let mut footer_tops = Vec::new();
        for height in [220.0, 400.0] {
            let mut harness = Harness::builder()
                .with_size(egui::vec2(1093.0, 614.0))
                .build_state(
                    move |ctx, rects: &mut Option<(egui::Rect, egui::Rect)>| {
                        egui::Window::new("Resizable startup notice")
                            .fixed_size(egui::vec2(460.0, height))
                            .default_pos(egui::pos2(60.0, 40.0))
                            .show(ctx, |ui| {
                                ui.heading("Heading");
                                let footer =
                                    super::startup_dialog_footer_height(ui, &["Close"], 0.0);
                                super::startup_dialog_scroll_body(
                                    ui,
                                    "resize_body",
                                    footer,
                                    |ui| {
                                        ui.label("Long message\n".repeat(80));
                                    },
                                );
                                let footer = ui.button("Close");
                                *rects = Some((footer.rect, footer.interact_rect));
                            });
                    },
                    None,
                );
            harness.run();
            let (rect, interact_rect) = harness.state().unwrap();
            assert!(interact_rect.contains_rect(rect));
            assert!(rect.bottom() <= 40.0 + height + 40.0, "{height}: {rect:?}");
            footer_tops.push(rect.top());
        }
        assert!(footer_tops[1] - footer_tops[0] > 100.0, "{footer_tops:?}");
    }

    #[test]
    fn startup_dialog_body_scrolls_and_consumes_wheel() {
        use eframe::egui;
        use egui_kittest::Harness;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1093.0, 614.0))
            .build_state(
                |ctx, state: &mut (f32, bool)| {
                    let had_wheel = ctx.input(|input| {
                        input
                            .events
                            .iter()
                            .any(|event| matches!(event, egui::Event::MouseWheel { .. }))
                    });
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.heading("Heading");
                        super::startup_dialog_scroll_body(ui, "wheel_body", 32.0, |ui| {
                            state.0 = ui.cursor().top();
                            ui.label("Long message\n".repeat(80));
                        });
                        if had_wheel {
                            state.1 = ctx.input(|input| {
                                input.raw_scroll_delta == egui::Vec2::ZERO
                                    && input.smooth_scroll_delta == egui::Vec2::ZERO
                                    && !input.events.iter().any(|event| {
                                        matches!(event, egui::Event::MouseWheel { .. })
                                    })
                            });
                        }
                    });
                },
                (0.0, false),
            );
        harness.run();
        let initial_top = harness.state().0;
        harness.hover_at(egui::pos2(50.0, 120.0));
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -150.0),
            modifiers: egui::Modifiers::NONE,
        });
        harness.run();
        assert!(harness.state().0 < initial_top - 20.0);
        assert!(harness.state().1);
    }

    #[test]
    fn dialog_scroll_style_reserves_the_full_floating_bar_width() {
        let mut scroll = eframe::egui::style::ScrollStyle::floating();
        scroll.bar_width = 10.0;
        scroll.bar_inner_margin = 4.0;
        scroll.floating_allocated_width = 0.0;

        let scroll = non_overlapping_dialog_scroll_style(scroll);

        assert_eq!(scroll.floating_allocated_width, 14.0);
        assert!(scroll.allocated_width() >= scroll.bar_width);
    }

    #[test]
    fn dialog_scroll_style_leaves_solid_scrollbars_unchanged() {
        let scroll = eframe::egui::style::ScrollStyle::solid();
        assert_eq!(non_overlapping_dialog_scroll_style(scroll), scroll);
    }
}

pub(crate) fn draw_mouse_nav_migration_dialog(
    ctx: &eframe::egui::Context,
    open: &mut bool,
) -> Option<crate::ring_shortcut::MouseBackForwardActionId> {
    use eframe::egui;
    let mut choice = None;
    egui::Window::new("マウス戻る/進むボタン")
            .collapsible(false)
            .resizable(false)
            .open(open)
            .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
            .default_height((ctx.content_rect().height() - 80.0).max(1.0))
            .show(ctx, |ui| {
                ui.set_width(420.0_f32.min((ctx.content_rect().width() - 48.0).max(1.0)));
                let footer = startup_dialog_footer_height(ui, &["標準にする", "従来どおり"], 12.0);
                startup_dialog_scroll_body(ui, "mouse_nav_migration_body", footer, |ui| {
                ui.label("マウスの戻る/進むボタンの標準動作を選んでください。");
                ui.add_space(6.0);
                ui.label("標準では、ブラウザやエクスプローラーに近いフォルダ履歴の戻る/進むとして使います。");
                ui.label("従来どおり、ツリー順の前/次フォルダ移動として使うこともできます。");
                ui.add_space(6.0);
                ui.small("後で 環境設定 > マウスボタン から変更できます。");
                });
                ui.add_space(12.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("標準にする").clicked() {
                        choice = Some(
                            crate::ring_shortcut::MouseBackForwardActionId::FolderHistoryPrevNext,
                        );
                    }
                    if ui.button("従来どおり").clicked() {
                        choice =
                            Some(crate::ring_shortcut::MouseBackForwardActionId::TreeFolderPrevNext);
                    }
                });
            });

    choice
}
