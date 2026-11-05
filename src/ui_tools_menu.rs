//! Tools menu body shared by the menu bar and headless UI coverage.
use eframe::egui;

use crate::keymap::{Keymap, MenuCommandId};
use crate::ui_helpers::HoverTipExt;

#[derive(Clone, Copy)]
pub(crate) struct ToolsMenuState<'a> {
    pub convert_enabled: bool,
    pub caption_disabled_reason: Option<&'a str>,
    pub caption_progress_available: bool,
    pub video_selected: bool,
}

fn group(command: MenuCommandId) -> Option<u8> {
    use MenuCommandId::*;
    match command {
        ConvertToZip => Some(0),
        ToolsCreateAiCaptions | ToolsShowCaptionProgress => Some(1),
        VideoRegisterUpscale | VideoDeleteUpscale | VideoShowUpscaleTasks => Some(2),
        _ => None,
    }
}

pub(crate) fn draw(
    ui: &mut egui::Ui,
    commands: &[MenuCommandId],
    keymap: &Keymap,
    state: ToolsMenuState<'_>,
) -> Option<MenuCommandId> {
    let mut previous_group = None;
    let mut clicked = None;
    for &command in commands {
        let Some(current_group) = group(command) else {
            continue;
        };
        if previous_group.is_some_and(|previous| previous != current_group) {
            ui.separator();
        }
        previous_group = Some(current_group);
        let reason = match command {
            MenuCommandId::ConvertToZip => (!state.convert_enabled).then_some(
                "RAR/CBR/7z/CB7/LZH/LHA ファイルを選択してください (スペースキーで複数選択可)",
            ),
            MenuCommandId::ToolsCreateAiCaptions => state.caption_disabled_reason,
            MenuCommandId::ToolsShowCaptionProgress => {
                (!state.caption_progress_available).then_some("作成中の AI 説明文はありません")
            }
            MenuCommandId::VideoRegisterUpscale | MenuCommandId::VideoDeleteUpscale => {
                (!state.video_selected).then_some("一覧で動画ファイルを選択してください")
            }
            MenuCommandId::VideoShowUpscaleTasks => None,
            _ => unreachable!("Tools group checked above"),
        };
        let response = ui
            .add_enabled(
                reason.is_none(),
                egui::Button::new(keymap.menu_command_label(command)),
            )
            .hover_tip_disabled(reason.unwrap_or_default());
        if response.clicked() {
            clicked = Some(command);
            ui.close();
        }
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{KeymapSettings, TopMenuId, menu_commands_for_parent};
    use egui_kittest::kittest::{NodeT, Queryable};
    use std::cell::Cell;

    #[test]
    fn tools_menu_catalog_order_and_group_separators() {
        use MenuCommandId::*;
        let commands = menu_commands_for_parent(TopMenuId::Tools)
            .map(|spec| spec.id)
            .collect::<Vec<_>>();
        assert_eq!(
            commands,
            [
                ConvertToZip,
                ToolsCreateAiCaptions,
                ToolsShowCaptionProgress,
                VideoRegisterUpscale,
                VideoDeleteUpscale,
                VideoShowUpscaleTasks
            ]
        );
        assert_eq!(
            commands.iter().copied().map(group).collect::<Vec<_>>(),
            [Some(0), Some(1), Some(1), Some(2), Some(2), Some(2)]
        );
        assert_eq!(
            commands
                .windows(2)
                .filter(|pair| group(pair[0]) != group(pair[1]))
                .count(),
            2
        );
    }

    fn harness(state: ToolsMenuState<'static>, dark: bool) -> egui_kittest::Harness<'static> {
        let commands = menu_commands_for_parent(TopMenuId::Tools)
            .map(|spec| spec.id)
            .collect::<Vec<_>>();
        let keymap = Keymap::from_settings(&KeymapSettings::default());
        let fonts_ready = Cell::new(false);
        crate::test_ui::builder()
            .with_size(egui::vec2(370.0, 240.0))
            .build_ui(move |ui| {
                let ctx = &ui.ctx().clone();
                crate::os_theme::apply_resolved(
                    ctx,
                    if dark {
                        crate::os_theme::ResolvedTheme::Dark
                    } else {
                        crate::os_theme::ResolvedTheme::Light
                    },
                );
                if !fonts_ready.replace(true) {
                    crate::ui_fonts::configure_fonts(ctx);
                    ctx.request_repaint();
                }
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    draw(ui, &commands, &keymap, state);
                });
            })
    }

    #[test]
    fn tools_menu_enabled_conditions_and_disabled_reasons() {
        for (convert, caption, progress, video) in [
            (false, false, false, false),
            (true, false, false, false),
            (false, true, false, false),
            (false, false, true, false),
            (false, false, false, true),
            (true, true, true, true),
        ] {
            let mut harness = harness(
                ToolsMenuState {
                    convert_enabled: convert,
                    caption_disabled_reason: (!caption)
                        .then_some(crate::context_menu_model::CAPTION_NO_TARGET_REASON),
                    caption_progress_available: progress,
                    video_selected: video,
                },
                true,
            );
            harness.run();
            let keymap = Keymap::from_settings(&KeymapSettings::default());
            for command in menu_commands_for_parent(TopMenuId::Tools).map(|spec| spec.id) {
                let label = keymap.menu_command_label(command);
                let row = harness.get_by_label(&label);
                let enabled = match command {
                    MenuCommandId::ConvertToZip => convert,
                    MenuCommandId::ToolsCreateAiCaptions => caption,
                    MenuCommandId::ToolsShowCaptionProgress => progress,
                    MenuCommandId::VideoRegisterUpscale | MenuCommandId::VideoDeleteUpscale => {
                        video
                    }
                    MenuCommandId::VideoShowUpscaleTasks => true,
                    _ => unreachable!(),
                };
                assert_eq!(row.accesskit_node().is_disabled(), !enabled, "{command:?}");
            }
            if !progress {
                harness.get_by_label("AI 説明文の作成状況").hover();
                harness.run();
                assert!(
                    harness
                        .get_by_label("作成中の AI 説明文はありません")
                        .rect()
                        .is_positive()
                );
            }
        }
    }

    #[test]
    fn tools_menu_snapshot_dark_and_light() {
        let mut snapshots = egui_kittest::SnapshotResults::default();
        for dark in [true, false] {
            let mut harness = harness(
                ToolsMenuState {
                    convert_enabled: false,
                    caption_disabled_reason: None,
                    caption_progress_available: true,
                    video_selected: false,
                },
                dark,
            );
            harness.run();
            harness.snapshot(if dark {
                "tools_menu_dark"
            } else {
                "tools_menu_light"
            });
            snapshots.extend(harness.take_snapshot_results());
        }
        snapshots.unwrap();
    }
}
