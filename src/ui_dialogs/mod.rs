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

/// Startup dialogs must leave room for their fixed heading and action row even on a
/// short viewport. The 128-point allowance includes window chrome, initial offset,
/// frame margins and the bottom screen margin (also conservative for centered Modals).
/// Allocate the parent explicitly: a ScrollArea in an auto-sized Area otherwise
/// inherits the previous frame's content height and may collapse instead of scrolling.
fn startup_dialog_scroll_body<R>(
    ui: &mut eframe::egui::Ui,
    id: &str,
    preferred_height: f32,
    body: impl FnOnce(&mut eframe::egui::Ui) -> R,
) -> R {
    startup_dialog_scroll_body_with_axes(ui, id, preferred_height, [false, true], body)
}

fn startup_dialog_scroll_body_with_axes<R>(
    ui: &mut eframe::egui::Ui,
    id: &str,
    preferred_height: f32,
    axes: [bool; 2],
    body: impl FnOnce(&mut eframe::egui::Ui) -> R,
) -> R {
    use eframe::egui;
    let header_height = ui.cursor().top() - ui.min_rect().top();
    // Allow two wrapped action rows plus separators and spacing below the body.
    let footer_height = 2.0 * (ui.spacing().interact_size.y + ui.spacing().item_spacing.y) + 24.0;
    let mut height = preferred_height
        .min(ui.ctx().content_rect().height() - 128.0 - header_height - footer_height)
        .max(1.0);
    if ui.stack().contained_in(egui::UiKind::Window) {
        // A Window's Resize owner is authoritative; preserve user height changes.
        // Only auto-sized Modal/Area needs the explicit viewport budget alone.
        height = height.min((ui.available_height() - footer_height).max(1.0));
    }
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), height),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.spacing_mut().scroll = non_overlapping_dialog_scroll_style(ui.spacing().scroll);
            let scroll = egui::ScrollArea::new(axes)
                .id_salt(id)
                .max_height(height)
                .min_scrolled_height(height.min(64.0))
                .auto_shrink([false, false])
                .show(ui, body);
            // ScrollArea consumes smooth deltas, but raw deltas/events must also
            // stay here instead of reaching a background list handler.
            if ui.rect_contains_pointer(scroll.inner_rect) {
                ui.ctx().input_mut(|input| {
                    input.raw_scroll_delta = egui::Vec2::ZERO;
                    input.smooth_scroll_delta = egui::Vec2::ZERO;
                    input
                        .events
                        .retain(|event| !matches!(event, egui::Event::MouseWheel { .. }));
                });
            }
            scroll.inner
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
        .default_height(380.0)
        .resizable(resizable)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            startup_dialog_scroll_body(ui, id, 280.0, |ui| {
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
        "update_notice" => {
            let info = crate::update_check::UpdateInfo {
                latest_tag: "v99.0.0".to_owned(),
                latest_version: semver::Version::new(99, 0, 0),
                release_url: String::new(),
                body: "## 更新内容\n- 表示の改善\n".repeat(40),
                is_newer: true,
            };
            update_notice::draw_update_notice_dialog(
                ctx,
                &mut open,
                Some(&info),
                Some(&"接続エラーの詳細。".repeat(100)),
            );
        }
        "network_data_dir" => {
            let path =
                std::path::PathBuf::from(format!(r"\\server\share\{}", "長い保存先\\".repeat(40)));
            network_data_dir_notice::draw_network_data_dir_notice_dialog(ctx, &mut open, &path);
        }
        "restore_result" => settings_restore::draw_restore_result_snapshot_fixture(ctx),
        "restore_list" => settings_restore::draw_restore_list_snapshot_fixture(ctx),
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
                                super::startup_dialog_scroll_body(ui, "resize_body", 360.0, |ui| {
                                    ui.label("Long message\n".repeat(80));
                                });
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
                        super::startup_dialog_scroll_body(ui, "wheel_body", 280.0, |ui| {
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
