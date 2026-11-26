use super::*;
use crate::export_dialog::{ExportEnd, ExportEvent, ExportFailure, ExportPending, ExportSuccess};
use crate::test_ui::scroll_layout::{HeightExpectation, ScrollLayout};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use std::sync::{Arc, atomic::AtomicBool, mpsc};

mod preset_tests;
mod watermark_tests;

fn item() -> BatchExportItem {
    BatchExportItem::new(
        "画像の名前".into(),
        "写真".into(),
        crate::books::CompositeSource::File {
            path: "C:/写真/画像.png".into(),
        },
        crate::books::BakedEditSnapshot {
            params: Default::default(),
            rotation: crate::rotation_db::Rotation::None,
            conceal: None,
            erase: None,
            local_adjust: None,
            comic: None,
            comic_source_dims: None,
            export_crop: None,
            crop_legacy_writeback: None,
            format: crate::capture::CaptureFormat::Png,
            jpeg_matte: crate::capture::JpegMatte::Black,
            stage: Default::default(),
            creative_lut: None,
            ai: None,
        },
    )
}

pub(super) fn dialog(expanded: bool) -> ExportBatchDialogState {
    let mut options = BatchExportOptions::from_legacy(
        "C:/書き出し".into(),
        "<filename>_<num:3>".into(),
        crate::capture::CaptureFormat::Png,
        crate::export_dialog::ExportScale::Full,
    );
    if expanded {
        options.destination = BatchDestination::SameAsSource {
            subfolder: "送信用".into(),
        };
        options.format.kind = BatchFormatKind::SameAsSource;
        options.keep_metadata = true;
        options.resize.mode = BatchResizeMode::Fit {
            width: 2048,
            height: 1536,
        };
        options.overwrite = OverwritePolicy::Replace;
    }
    ExportBatchDialogState {
        items: vec![item()],
        pending: Default::default(),
        total: 1,
        skipped: 2,
        options,
        initial_focus_done: true,
        error: None,
        logo_check: None,
        preset: PresetUi::default(),
    }
}

fn pending() -> (ExportPending, mpsc::Sender<ExportEvent>) {
    let (tx, rx) = mpsc::channel();
    (
        ExportPending {
            origin: crate::export_dialog::ExportOrigin::Batch,
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
            total: 100,
            done: 0,
            last_message: "準備中".into(),
            successes: vec![],
            skipped: vec![],
            errors: vec![],
            thumbnail_notices: vec![],
            end: None,
            cancel_requested: false,
        },
        tx,
    )
}

fn warnings() -> ExportPending {
    let (mut pending, _) = pending();
    pending.end = Some(ExportEnd::Cancelled);
    pending.last_message = "キャンセルしました: 成功 40 / スキップ 10 / 失敗 30".into();
    for i in 0..40 {
        pending
            .thumbnail_notices
            .push(crate::export_dialog::ExportThumbnailNotice {
            label: format!("画像{i}"),
            path: format!(
                "C:/出力/非常に長いファイル名が折り返されても閉じるボタンを押せる画像_{i:04}.png"
            )
            .into(),
            kinds: crate::save_with_metadata::ThumbnailNoticeKinds {
                exif_uninspectable: true,
                xmp: true,
                xmp_uninspectable: i % 2 == 0,
                maker_note: true,
            },
        });
        pending.errors.push(ExportFailure {
            label: format!("長い元のファイル名_{i:04}.png"),
            message: "保存先のファイルに書き込めませんでした。".repeat(3),
        });
    }
    pending
}

struct WindowState {
    dialog: ExportBatchDialogState,
    starts: usize,
    picks: usize,
    cancels: usize,
    presets: Vec<BatchExportPreset>,
    saves: usize,
}
fn window_harness(size: egui::Vec2, scale: f32, expanded: bool) -> Harness<'static, WindowState> {
    let mut ready = false;
    crate::test_ui::builder()
        .with_size(size)
        .with_context_setup(|ctx| {
            crate::ui_fonts::configure_fonts(ctx);
            crate::ime_focus::install_ime_input_policy(ctx);
        })
        .build_ui_state(
            move |ui, state: &mut WindowState| {
                if !ready {
                    crate::settings::apply_ui_scale_factor(ui.ctx(), scale);
                    ready = true;
                    return;
                }
                let ctx = ui.ctx();
                let enter = !crate::ime_focus::ime_input_active(ctx)
                    && ctx.input(|i| i.key_pressed(egui::Key::Enter));
                let action = draw_batch_window(ctx, &mut state.dialog, &state.presets, enter);
                if let Some(preset) = action.preset {
                    match state.dialog.edit_presets(&mut state.presets, preset) {
                        Ok(true) => state.saves += 1,
                        Ok(false) => {}
                        Err(error) => state.dialog.error = Some(error),
                    }
                }
                state.starts += usize::from(action.start);
                state.picks += usize::from(action.pick_folder);
                state.cancels += usize::from(action.cancel);
            },
            WindowState {
                dialog: dialog(expanded),
                starts: 0,
                picks: 0,
                cancels: 0,
                presets: Vec::new(),
                saves: 0,
            },
        )
}

#[test]
fn batch_export_replace_disables_file_time_without_changing_saved_choice() {
    for saved in [false, true] {
        let mut h = window_harness(egui::vec2(1093.0, 1800.0), 1.0, false);
        h.run_steps(12);
        assert!(
            !h.get_by_label("元の更新日時にする")
                .accesskit_node()
                .is_disabled()
        );
        if saved {
            h.get_by_label("元の更新日時にする").click();
            h.run_steps(2);
        }
        assert_eq!(h.state().dialog.options.keep_file_time, saved);
        h.get_by_label("上書き").click();
        h.run_steps(2);
        assert_eq!(h.state().dialog.options.overwrite, OverwritePolicy::Replace);
        {
            let checkbox = h.get_by_label("元の更新日時にする");
            assert_eq!(
                checkbox.accesskit_node().role(),
                egui::accesskit::Role::CheckBox
            );
            assert!(checkbox.accesskit_node().is_disabled());
            assert_eq!(
                checkbox.accesskit_node().toggled(),
                Some(if saved {
                    egui::accesskit::Toggled::True
                } else {
                    egui::accesskit::Toggled::False
                })
            );
        }
        // Disabled pointer input must not overwrite the persisted choice.
        h.get_by_label("元の更新日時にする").click();
        h.run_steps(2);
        assert_eq!(h.state().dialog.options.keep_file_time, saved);
        let serialized = serde_json::to_string(&h.state().dialog.options).unwrap();
        let restored: BatchExportOptions = serde_json::from_str(&serialized).unwrap();
        assert_eq!(restored.keep_file_time, saved);
        assert_eq!(restored.overwrite, OverwritePolicy::Replace);
        h.state_mut().dialog.options = restored;
        h.run_steps(2);
        assert!(
            h.get_by_label("元の更新日時にする")
                .accesskit_node()
                .is_disabled()
        );
        for (label, policy) in [
            ("スキップ", OverwritePolicy::Skip),
            ("連番", OverwritePolicy::Rename),
        ] {
            h.get_by_label(label).click();
            h.run_steps(2);
            assert_eq!(h.state().dialog.options.overwrite, policy);
            assert!(
                !h.get_by_label("元の更新日時にする")
                    .accesskit_node()
                    .is_disabled()
            );
            assert_eq!(h.state().dialog.options.keep_file_time, saved);
        }
        h.get_by_label("元の更新日時にする").click();
        h.run_steps(2);
        assert_eq!(h.state().dialog.options.keep_file_time, !saved);
    }
}

#[test]
fn batch_export_conditional_fields_have_accessible_names() {
    // Keep the ordinary accessibility check enabled for every conditional field,
    // including resize modes and lossless WebP states outside the snapshot fixtures.
    let mut h = window_harness(egui::vec2(1093.0, 1800.0), 1.0, false);
    h.run_steps(4);
    for same_folder in [false, true] {
        for (kind, lossless) in [
            (BatchFormatKind::Png, false),
            (BatchFormatKind::Jpeg, false),
            (BatchFormatKind::Webp, false),
            (BatchFormatKind::Webp, true),
            (BatchFormatKind::SameAsSource, false),
            (BatchFormatKind::SameAsSource, true),
        ] {
            for mode in [
                BatchResizeMode::None,
                BatchResizeMode::Percent(50),
                BatchResizeMode::LongEdge(2048),
                BatchResizeMode::ShortEdge(2048),
                BatchResizeMode::Width(2048),
                BatchResizeMode::Height(2048),
                BatchResizeMode::Fit {
                    width: 2048,
                    height: 1536,
                },
            ] {
                {
                    let options = &mut h.state_mut().dialog.options;
                    options.destination = if same_folder {
                        BatchDestination::SameAsSource {
                            subfolder: "送信用".into(),
                        }
                    } else {
                        BatchDestination::Folder {
                            path: "C:/書き出し".into(),
                            keep_structure: true,
                        }
                    };
                    options.format.kind = kind;
                    options.format.webp_lossless = lossless;
                    options.resize.mode = mode;
                }
                h.run_steps(3);
                for label in ["ファイル形式", "リサイズ", "方式", "ファイル名"] {
                    let _ = h.get_by_label(label);
                }
                let _ = h.get_by_label(if same_folder {
                    "サブフォルダ (空なら元のフォルダの直下)"
                } else {
                    "保存先"
                });
                if matches!(kind, BatchFormatKind::Jpeg | BatchFormatKind::SameAsSource) {
                    let _ = h.get_by_label("JPEG 品質");
                }
                if !lossless
                    && matches!(kind, BatchFormatKind::Webp | BatchFormatKind::SameAsSource)
                {
                    let _ = h.get_by_label("WebP 品質");
                }
                match mode {
                    BatchResizeMode::None => {}
                    BatchResizeMode::Percent(_) => {
                        let _ = h.get_by_label("割合");
                    }
                    BatchResizeMode::LongEdge(_) => {
                        let _ = h.get_by_label("長辺");
                    }
                    BatchResizeMode::ShortEdge(_) => {
                        let _ = h.get_by_label("短辺");
                    }
                    BatchResizeMode::Width(_) => {
                        let _ = h.get_by_label("幅");
                    }
                    BatchResizeMode::Height(_) => {
                        let _ = h.get_by_label("高さ");
                    }
                    BatchResizeMode::Fit { .. } => {
                        let _ = h.get_by_label("幅");
                        let _ = h.get_by_label("高さ");
                    }
                }
                assert_eq!(h.state().starts, 0);
            }
        }
    }
}

fn wheel(h: &mut Harness<'_, impl Sized>, layout: ScrollLayout) {
    h.event(egui::Event::PointerMoved(layout.inner.center()));
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -100000.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
    });
    h.run_steps(32);
}

fn assert_long_dialog_uses_remaining_height(ctx: &egui::Context, title: &str) {
    let bounds = ctx.content_rect();
    let window = ctx.memory(|memory| memory.area_rect(egui::Id::unique(Some(title))).unwrap());
    assert!(
        bounds.contains_rect(window),
        "window outside screen: {window:?}"
    );
    // The footer measurement reserves a few item spacings in addition to its
    // visible controls. Permit that small reserve, not the old 180pt empty gap.
    let bottom = bounds.bottom() - DIALOG_MARGIN;
    let tolerance = 16.0 + 1.0 / ctx.pixels_per_point();
    assert!(
        (bottom - window.bottom()).abs() <= tolerance,
        "long dialog left unused screen height: bounds={bounds:?}, window={window:?}"
    );
}

#[test]
fn batch_export_small_viewports_keep_actions_and_reach_last_setting() {
    for size in [egui::vec2(1093.0, 614.0), egui::vec2(1366.0, 728.0)] {
        for scale in [1.0, 2.0] {
            let mut h = window_harness(size, scale, true);
            h.state_mut().dialog.error = Some(
                "保存先の設定を確認してください。長いエラーも操作行の内側で折り返します。"
                    .repeat(2),
            );
            h.run_steps(12);
            let bounds = h.ctx.content_rect();
            for label in ["Close window", "書き出す", "キャンセル"] {
                assert!(
                    bounds.contains_rect(h.get_by_label(label).rect()),
                    "{size:?}/{scale}: {label}"
                );
            }
            let before = h
                .ctx
                .data(|d| d.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_layout")))
                .unwrap();
            before.assert_height(HeightExpectation::Scrolls, bounds, h.ctx.pixels_per_point());
            assert_long_dialog_uses_remaining_height(&h.ctx, "一括エクスポート");
            let start_rect = h.get_by_label("書き出す").rect();
            wheel(&mut h, before);
            let after = h
                .ctx
                .data(|d| d.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_layout")))
                .unwrap();
            after.assert_scrolled_to_end(before, h.ctx.pixels_per_point());
            assert_eq!(start_rect, h.get_by_label("書き出す").rect());
            assert!(after.inner.intersects(h.get_by_label("方式").rect()));
            h.get_by_label("書き出す").click();
            h.run_steps(2);
            assert_eq!(h.state().starts, 1);
            h.get_by_label("キャンセル").click();
            h.run_steps(2);
            assert_eq!(h.state().cancels, 1);
            h.get_by_label("Close window").click();
            h.run_steps(2);
            assert!(h.state().cancels >= 2);
        }
    }
}

#[test]
fn batch_export_warning_small_viewports_scroll_both_lists_and_keep_close() {
    for scale in [1.0, 2.0] {
        let mut ready = false;
        let mut h = crate::test_ui::builder()
            .with_size(egui::vec2(1093.0, 614.0))
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .build_ui_state(
                move |ui, state: &mut (ExportPending, bool)| {
                    if !ready {
                        crate::settings::apply_ui_scale_factor(ui.ctx(), scale);
                        ready = true;
                        return;
                    }
                    state.1 |= draw_progress_window(ui.ctx(), &state.0).0;
                },
                (warnings(), false),
            );
        h.run_steps(12);
        let bounds = h.ctx.content_rect();
        let close = h.get_by_label("閉じる").rect();
        assert!(bounds.contains_rect(close));
        assert!(bounds.contains_rect(h.get_by_label("Close window").rect()));
        let before = h
            .ctx
            .data(|d| d.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_warning_layout")))
            .unwrap();
        before.assert_height(HeightExpectation::Scrolls, bounds, h.ctx.pixels_per_point());
        assert_long_dialog_uses_remaining_height(&h.ctx, "エクスポート進行状況");
        wheel(&mut h, before);
        let after = h
            .ctx
            .data(|d| d.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_warning_layout")))
            .unwrap();
        after.assert_scrolled_to_end(before, h.ctx.pixels_per_point());
        assert_eq!(close, h.get_by_label("閉じる").rect());
        h.get_by_label("閉じる").click();
        h.run_steps(2);
        assert!(h.state().1);
        h.state_mut().1 = false;
        h.get_by_label("Close window").click();
        h.run_steps(2);
        assert!(h.state().1);
    }
}

#[test]
fn batch_export_default_and_expanded_long_bodies_use_remaining_height() {
    for size in [egui::vec2(1093.0, 614.0), egui::vec2(1366.0, 728.0)] {
        for scale in [1.0, 2.0] {
            for expanded in [false, true] {
                let mut h = window_harness(size, scale, expanded);
                h.run_steps(12);
                let body = h
                    .ctx
                    .data(|data| {
                        data.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_layout"))
                    })
                    .unwrap();
                // A tall viewport may fit the default form. Only overflowing
                // content must reach the bottom; fitting content stays natural.
                if body.content.y > body.inner.height() + 1.0 / h.ctx.pixels_per_point() {
                    assert_long_dialog_uses_remaining_height(&h.ctx, "一括エクスポート");
                } else {
                    body.assert_height(
                        HeightExpectation::Fits,
                        h.ctx.content_rect(),
                        h.ctx.pixels_per_point(),
                    );
                }
            }
        }
    }
}

#[test]
fn batch_export_short_warning_body_and_progress_keep_natural_height() {
    for warning in [false, true] {
        let mut state = warnings();
        state.thumbnail_notices.truncate(1);
        state.errors.clear();
        if !warning {
            state.end = None;
            state.thumbnail_notices.clear();
            state.last_message = "保存中".into();
        }
        let mut h = crate::test_ui::builder()
            .with_size(egui::vec2(1093.0, 614.0))
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .build_ui_state(
                |ui, state: &mut ExportPending| {
                    draw_progress_window(ui.ctx(), state);
                },
                state,
            );
        h.run_steps(12);
        if warning {
            let body = h
                .ctx
                .data(|data| {
                    data.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_warning_layout"))
                })
                .unwrap();
            body.assert_height(
                HeightExpectation::Fits,
                h.ctx.content_rect(),
                h.ctx.pixels_per_point(),
            );
        }
        let window = h.ctx.memory(|memory| {
            memory
                .area_rect(egui::Id::unique(Some("エクスポート進行状況")))
                .unwrap()
        });
        assert!(
            h.ctx.content_rect().bottom() - window.bottom() > 60.0,
            "short dialog must shrink: {window:?}"
        );
    }
}

#[test]
fn batch_export_enter_routes_text_buttons_popup_and_same_frame_focus_once() {
    let mut h = window_harness(egui::vec2(1093.0, 1100.0), 1.0, false);
    h.run_steps(12);
    h.get_by_label("ファイル名").focus();
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().starts, 1);
    h.get_by_label("変更...").focus();
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!((h.state().starts, h.state().picks), (1, 1));
    h.get_by_label("書き出す").focus();
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().starts, 2);
    // A focused value editor must never trigger generic submit.
    h.get_by_label("拡大する").focus();
    h.step();
    assert!(h.get_by_label("拡大する").is_focused());
    let allow_upscale_before = h.state().dialog.options.resize.allow_upscale;
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().starts, 2);
    assert_ne!(
        h.state().dialog.options.resize.allow_upscale,
        allow_upscale_before
    );
    assert!(h.get_by_label("拡大する").is_focused());
    let checkbox_focus = h.ctx.memory(|memory| memory.focused()).unwrap();
    let pos = h.get_by_label("ファイル名").rect().center();
    // Harness::event/key_press queue events for separate frames. RawInput is
    // required to test a click and Enter arriving in the very same frame.
    h.input_mut().events.extend([
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    h.step();
    assert_eq!(h.state().starts, 2);
    assert!(h.get_by_label("ファイル名").is_focused());
    let filename_focus = h.ctx.memory(|memory| memory.focused()).unwrap();
    assert_ne!(filename_focus, checkbox_focus);
    assert!(h.ctx.read_response(filename_focus).unwrap().gained_focus());
    h.run_steps(2);
    assert_eq!(h.state().starts, 2);
    h.ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().starts, 3);
    h.get_by_label("ファイル形式").focus();
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(egui::Popup::is_any_open(&h.ctx));
    assert_eq!(h.state().starts, 3);
    h.get_by_label("WebP").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog.options.format.kind, BatchFormatKind::Webp);
    assert_eq!(h.state().starts, 3);
}

#[test]
fn batch_export_popup_wheel_does_not_scroll_dialog_body() {
    let mut h = window_harness(egui::vec2(1093.0, 614.0), 1.0, true);
    h.run_steps(12);
    let before = h
        .ctx
        .data(|d| d.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_layout")))
        .unwrap();
    wheel(&mut h, before);
    h.get_by_label("方式").click();
    h.run_steps(2);
    assert!(egui::Popup::is_any_open(&h.ctx));
    let before = h
        .ctx
        .data(|d| d.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_layout")))
        .unwrap();
    let pos = h.get_by_label("バイリニア").rect().center();
    h.event(egui::Event::PointerMoved(pos));
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, 200.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
    });
    h.run_steps(16);
    let after = h
        .ctx
        .data(|d| d.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_layout")))
        .unwrap();
    assert_eq!(before.offset, after.offset);
    assert_eq!(h.state().starts, 0);
}

#[test]
fn batch_export_path_typing_preserves_spaces_between_frames() {
    let mut h = window_harness(egui::vec2(1093.0, 900.0), 1.0, false);
    h.run_steps(12);
    h.get_by_label("保存先").focus();
    h.step();
    // Select the whole existing field through the actual TextEdit.
    h.event(egui::Event::Key {
        key: egui::Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
    });
    h.step();
    h.event(egui::Event::Key {
        key: egui::Key::A,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    for ch in "C:/My Folder/output".chars() {
        h.event(egui::Event::Text(ch.to_string()));
        h.step();
    }
    assert!(
        matches!(&h.state().dialog.options.destination,BatchDestination::Folder { path,.. } if path == std::path::Path::new("C:/My Folder/output"))
    );
}

fn app_harness(
    app: crate::app::AppTestEnvForTest,
) -> Harness<'static, crate::app::AppTestEnvForTest> {
    crate::test_ui::builder()
        .with_size(egui::vec2(1093.0, 1100.0))
        .with_context_setup(|ctx| {
            crate::ui_fonts::configure_fonts(ctx);
            crate::ime_focus::install_ime_input_policy(ctx);
        })
        .build_ui_state(
            |ui, app: &mut crate::app::AppTestEnvForTest| {
                app.draw_export_batch_dialog(ui.ctx());
                app.draw_export_progress_dialog(ui.ctx());
            },
            app,
        )
}

#[test]
fn batch_export_dialog_edit_start_persist_reopen_and_worker_effect() {
    let mut app = crate::app::tests::phase_c_support::setup_app_with_fresh_data_dir();
    let source = app.tmp.path().join("input.png");
    image::RgbaImage::from_pixel(12, 8, image::Rgba([50, 60, 70, 255]))
        .save(&source)
        .unwrap();
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
    std::fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_modified(time)
        .unwrap();
    let directory = app.tmp.path().join("output");
    std::fs::create_dir_all(&directory).unwrap();
    app.settings.export_batch_directory = Some(directory.clone());
    app.settings.export_batch_template = "<filename>".into();
    app.settings.export_batch_scale = crate::export_dialog::ExportScale::Half;
    app.items.push(crate::grid_item::GridItem::Image(source));
    app.thumbnails
        .push(crate::grid_item::ThumbnailState::Pending);
    app.visible_indices = vec![0];
    app.checked.insert(0);
    let legacy = (
        app.settings.export_batch_directory.clone(),
        app.settings.export_batch_template.clone(),
        app.settings.export_batch_format,
        app.settings.export_batch_scale,
    );
    app.open_export_batch_dialog(&egui::Context::default());
    let mut h = app_harness(app);
    h.run_steps(12);
    assert!(
        !h.state()
            .export_batch_dialog
            .as_ref()
            .unwrap()
            .options
            .keep_file_time
    );
    // AccessKit keeps nodes for clipped body widgets too. Node::click sends
    // pointer events to the node's center; it does not reveal the node first.
    // Check the actual viewport rather than assuming that the harness's screen
    // size makes every body control visible.
    h.get_by_label("元の更新日時にする").scroll_to_me();
    h.run_steps(4);
    let checkbox = h.get_by_label("元の更新日時にする");
    assert_eq!(
        checkbox.accesskit_node().role(),
        egui::accesskit::Role::CheckBox
    );
    assert!(!checkbox.accesskit_node().is_disabled());
    let body = h
        .ctx
        .data(|data| data.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_layout")))
        .unwrap();
    assert!(
        body.inner.contains_rect(checkbox.rect()),
        "checkbox must be inside the actual scroll viewport before clicking: body={body:?}, checkbox={:?}",
        checkbox.rect()
    );
    h.get_by_label("元の更新日時にする").click();
    h.run_steps(2);
    assert!(
        h.state()
            .export_batch_dialog
            .as_ref()
            .unwrap()
            .options
            .keep_file_time
    );
    h.get_by_label("書き出す").click();
    h.run_steps(2);
    assert!(h.state().export_batch_dialog.is_none());
    let saved = h.state().settings.export_batch_options.clone().unwrap();
    assert!(saved.keep_file_time);
    let db = crate::settings_db::SettingsDb::open(h.state().tmp.path()).unwrap();
    let disk = db.load_into_settings().unwrap();
    assert_eq!(disk.export_batch_options, Some(saved.clone()));
    assert_eq!(
        (
            disk.export_batch_directory,
            disk.export_batch_template,
            disk.export_batch_format,
            disk.export_batch_scale
        ),
        legacy
    );
    let mut wrote = None;
    loop {
        match h
            .state()
            .export_pending
            .as_ref()
            .unwrap()
            .rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap()
        {
            ExportEvent::Completed(success) => wrote = Some(success.path),
            ExportEvent::Failed(error) => panic!("{}", error.message),
            ExportEvent::AllDone => break,
            ExportEvent::Cancelled => panic!("unexpected cancel"),
            _ => {}
        }
    }
    let wrote = wrote.unwrap();
    assert_eq!(wrote, directory.join("input.png"));
    assert_eq!(image::image_dimensions(&wrote).unwrap(), (6, 4));
    assert_eq!(std::fs::metadata(wrote).unwrap().modified().unwrap(), time);
    h.state_mut().export_pending = None;
    h.state_mut().settings.export_batch_options =
        db.load_into_settings().unwrap().export_batch_options;
    // A newer value is the sole owner even if legacy fields subsequently differ.
    h.state_mut().settings.export_batch_template = "ignored".into();
    let ctx = h.ctx.clone();
    h.state_mut().open_export_batch_dialog(&ctx);
    h.run_steps(2);
    assert_eq!(
        h.state().export_batch_dialog.as_ref().unwrap().options,
        saved
    );
    h.get_by_label("書き出す").click();
    h.run_steps(2);
    assert!(h.state().export_batch_dialog.is_none());
    let mut restored_output = None;
    loop {
        match h
            .state()
            .export_pending
            .as_ref()
            .unwrap()
            .rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap()
        {
            ExportEvent::Completed(success) => restored_output = Some(success.path),
            ExportEvent::Failed(error) => panic!("{}", error.message),
            ExportEvent::AllDone => break,
            ExportEvent::Cancelled => panic!("unexpected cancel"),
            _ => {}
        }
    }
    let restored_output = restored_output.unwrap();
    assert_eq!(restored_output, directory.join("input_1.png"));
    assert_eq!(image::image_dimensions(&restored_output).unwrap(), (6, 4));
    assert_eq!(
        image::guess_format(&std::fs::read(&restored_output).unwrap()).unwrap(),
        image::ImageFormat::Png
    );
    assert_eq!(
        std::fs::metadata(restored_output)
            .unwrap()
            .modified()
            .unwrap(),
        time
    );
}

#[test]
fn batch_export_endings_keep_notices_after_disconnect_and_refresh_matching_folder_once() {
    for end in [ExportEnd::Done, ExportEnd::Cancelled] {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let current = app.tmp.path().join("current");
        std::fs::create_dir_all(&current).unwrap();
        app.current_folder = Some(current.clone());
        app.current_folder_last_mtime = Some(std::time::SystemTime::now());
        let (p, tx) = pending();
        app.export_pending = Some(p);
        for path in [app.tmp.path().join("other/a.png"), current.join("b.png")] {
            tx.send(ExportEvent::Completed(ExportSuccess {
                label: "image".into(),
                path: path.clone(),
            }))
            .unwrap();
            tx.send(ExportEvent::ThumbnailNotice {
                label: "image".into(),
                path,
                kinds: crate::save_with_metadata::ThumbnailNoticeKinds {
                    xmp: true,
                    ..Default::default()
                },
            })
            .unwrap();
        }
        tx.send(ExportEvent::Skipped {
            label: "existing".into(),
            path: current.join("existing.png"),
        })
        .unwrap();
        let ctx = egui::Context::default();
        app.poll_export_pending(&ctx);
        assert!(app.export_pending.as_ref().unwrap().end.is_none());
        assert_eq!(app.export_pending.as_ref().unwrap().done, 3);
        assert!(app.folder_refresh_pending.is_none());
        tx.send(if end == ExportEnd::Done {
            ExportEvent::AllDone
        } else {
            ExportEvent::Cancelled
        })
        .unwrap();
        drop(tx);
        app.poll_export_pending(&ctx);
        assert_eq!(app.export_pending.as_ref().unwrap().end, Some(end));
        assert_eq!(app.folder_refresh_pending.as_ref().unwrap().folder, current);
        app.folder_refresh_pending = None;
        app.poll_export_pending(&ctx);
        assert!(app.export_pending.is_some());
        assert!(app.folder_refresh_pending.is_none());
        assert_eq!(
            app.export_pending.as_ref().unwrap().thumbnail_notices.len(),
            2
        );
        assert!(
            !app.export_pending
                .as_ref()
                .unwrap()
                .last_message
                .contains("中断")
        );
    }
}

#[test]
fn single_export_success_with_warning_keeps_window_without_saved_output_toast() {
    let mut app = crate::app::setup_app_for_test();
    let (mut p, tx) = pending();
    p.total = 1;
    app.export_pending = Some(p);
    app.fs_feedback_toast = None;
    app.fs_feedback_toast_action = None;
    let path = app.tmp.path().join("single.jpg");
    tx.send(ExportEvent::Completed(ExportSuccess {
        label: "single".into(),
        path: path.clone(),
    }))
    .unwrap();
    tx.send(ExportEvent::ThumbnailNotice {
        label: "single".into(),
        path,
        kinds: crate::save_with_metadata::ThumbnailNoticeKinds {
            exif_uninspectable: true,
            ..Default::default()
        },
    })
    .unwrap();
    tx.send(ExportEvent::AllDone).unwrap();
    drop(tx);
    let ctx = egui::Context::default();
    app.poll_export_pending(&ctx);
    app.poll_export_pending(&ctx); // normal worker disconnection keeps the warning
    let p = app.export_pending.as_ref().unwrap();
    assert_eq!(p.end, Some(ExportEnd::Done));
    assert_eq!(p.successes.len(), 1);
    assert_eq!(p.thumbnail_notices.len(), 1);
    assert!(app.fs_feedback_toast.is_none());
    assert!(app.fs_feedback_toast_action.is_none());
}

#[test]
fn single_export_warning_list_shows_uninspectable_exif_label() {
    let (mut p, _) = pending();
    p.end = Some(ExportEnd::Done);
    p.last_message = "完了: 成功 1 / スキップ 0 / 失敗 0".into();
    p.thumbnail_notices
        .push(crate::export_dialog::ExportThumbnailNotice {
            label: "single".into(),
            path: "single.jpg".into(),
            kinds: crate::save_with_metadata::ThumbnailNoticeKinds {
                exif_uninspectable: true,
                ..Default::default()
            },
        });
    let mut h = crate::test_ui::builder()
        .with_size(egui::vec2(1093.0, 614.0))
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .build_ui_state(
            |ui, p: &mut ExportPending| {
                draw_progress_window(ui.ctx(), p);
            },
            p,
        );
    h.run_steps(12);
    h.get_by_label("single.jpg: EXIF (外せませんでした)");
    h.get_by_label("閉じる");
}

#[test]
fn batch_export_poll_implements_every_completion_notification_and_plain_cancel() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    for (success, skip, fail, cancel, expected) in [
        (1, 0, 0, false, "保存しました: output-0.png"),
        (2, 0, 0, false, "2 件をエクスポートしました"),
        (
            1,
            2,
            0,
            false,
            "1 件をエクスポートしました (既にある 2 件はスキップ)",
        ),
        (
            0,
            2,
            0,
            false,
            "既にある 2 件をスキップしました (書き出した画像はありません)",
        ),
        (1, 2, 1, false, "完了: 成功 1 / スキップ 2 / 失敗 1"),
        (1, 2, 1, true, "エクスポートをキャンセルしました"),
    ] {
        let (p, tx) = pending();
        app.export_pending = Some(p);
        app.fs_feedback_toast = None;
        app.fs_feedback_toast_action = None;
        for i in 0..success {
            tx.send(ExportEvent::Completed(ExportSuccess {
                label: "image".into(),
                path: app.tmp.path().join(format!("output-{i}.png")),
            }))
            .unwrap();
        }
        for _ in 0..skip {
            tx.send(ExportEvent::Skipped {
                label: "existing".into(),
                path: app.tmp.path().join("existing.png"),
            })
            .unwrap();
        }
        for _ in 0..fail {
            tx.send(ExportEvent::Failed(ExportFailure {
                label: "bad".into(),
                message: "failure".into(),
            }))
            .unwrap();
        }
        tx.send(if cancel {
            ExportEvent::Cancelled
        } else {
            ExportEvent::AllDone
        })
        .unwrap();
        drop(tx);
        app.poll_export_pending(&ctx);
        if fail > 0 && !cancel {
            let p = app.export_pending.as_ref().unwrap();
            assert_eq!(p.last_message, expected);
            assert_eq!(p.done, success + skip + fail);
            assert!(app.fs_feedback_toast.is_none());
            app.poll_export_pending(&ctx);
            assert_eq!(
                app.export_pending.as_ref().unwrap().end,
                Some(ExportEnd::Done)
            );
        } else {
            assert!(app.export_pending.is_none());
            assert_eq!(app.fs_feedback_toast.as_ref().unwrap().0, expected);
            if success == 1 && skip == 0 && !cancel {
                assert_eq!(
                    app.fs_feedback_toast_action.as_ref().unwrap().reveal_path(),
                    Some(&app.tmp.path().join("output-0.png"))
                );
            } else {
                assert!(app.fs_feedback_toast_action.is_none());
            }
        }
    }
}

#[test]
fn batch_export_warning_close_and_title_close_discard_ended_pending() {
    for title in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        app.export_pending = Some(warnings());
        let mut h = app_harness(app);
        h.run_steps(4);
        h.get_by_label(if title { "Close window" } else { "閉じる" })
            .click();
        h.run_steps(2);
        assert!(h.state().export_pending.is_none());
    }
}

#[test]
fn batch_export_options_migrate_only_when_new_value_is_absent() {
    let mut app = crate::app::setup_app_for_test();
    app.settings.export_batch_directory = Some(app.tmp.path().to_owned());
    app.settings.export_batch_template = "legacy_<num>".into();
    app.settings.export_batch_format = crate::capture::CaptureFormat::Jpeg75;
    app.settings.export_batch_scale = crate::export_dialog::ExportScale::Quarter;
    let mut json = serde_json::to_value(&app.settings).unwrap();
    json.as_object_mut().unwrap().remove("export_batch_options");
    app.settings = serde_json::from_value(json).unwrap();
    let old = app.batch_export_initial_options();
    assert_eq!(old.format.jpeg_quality, 75);
    assert_eq!(old.resize.mode, BatchResizeMode::Percent(25));
    assert_eq!(old.template, "legacy_<num>");
    let mut new = old;
    new.template = "new".into();
    new.destination = BatchDestination::SameAsSource {
        subfolder: "copy".into(),
    };
    new.keep_metadata = true;
    app.settings.export_batch_options = Some(new.clone());
    app.settings.export_batch_template = "ignored".into();
    assert_eq!(app.batch_export_initial_options(), new);
}

#[test]
fn batch_export_warning_is_only_shown_after_end_and_still_allows_cancel() {
    let mut state = warnings();
    state.end = None;
    let mut h = crate::test_ui::builder()
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .build_ui_state(
            |ui, state: &mut (ExportPending, bool)| {
                state.1 |= draw_progress_window(ui.ctx(), &state.0).1;
            },
            (state, false),
        );
    h.run_steps(4);
    h.get_by_label("キャンセル").click();
    h.run_steps(2);
    assert!(h.state().1);
    assert!(
        h.ctx
            .data(|d| d.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_warning_layout")))
            .is_none()
    );
}

mod gpu_tests {
    use super::*;
    #[test]
    fn batch_export_snapshot_default() {
        let mut h = window_harness(egui::vec2(1093.0, 614.0), 1.0, false);
        h.run_steps(12);
        h.snapshot("batch_export_default");
    }
    #[test]
    fn batch_export_snapshot_expanded() {
        let mut h = window_harness(egui::vec2(1093.0, 614.0), 1.0, true);
        h.run_steps(12);
        h.snapshot("batch_export_expanded_stage_a");
    }
    #[test]
    fn batch_export_snapshot_thumbnail_warning() {
        let mut h = crate::test_ui::builder()
            .with_size(egui::vec2(1093.0, 614.0))
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .build_ui_state(
                |ui, p: &mut ExportPending| {
                    draw_progress_window(ui.ctx(), p);
                },
                warnings(),
            );
        h.run_steps(12);
        h.snapshot("batch_export_thumbnail_warning");
    }
}
