//! EXIF clipboard behavior through the shared renderer and the fullscreen input handler.
use super::metadata_info_target_tests::{select, spread_app};
use super::*;
use crate::app::{AppTestEnvForTest, MetadataLoadPending, MetadataLoadResult};
use crate::grid_item::GridItem;
use crate::keymap::{Chord, KeyAction, KeyName, Keymap, KeymapSettings};
use crate::settings::SpreadMode;
use egui_kittest::kittest::Queryable;
use std::sync::atomic::Ordering;

fn bind(app: &mut App, key: KeyName) {
    let mut settings = KeymapSettings::default();
    settings.set_override_chords(KeyAction::FsCopyExif, vec![Chord::ctrl(key)]);
    app.keymap = Keymap::from_settings(&settings);
}

fn press(key: egui::Key, repeat: bool) -> Vec<egui::Event> {
    let event = |pressed, repeat| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat,
        modifiers: egui::Modifiers::CTRL,
    };
    if repeat {
        vec![event(true, true)]
    } else {
        vec![event(false, false), event(true, false)]
    }
}

fn frame(
    ctx: &egui::Context,
    app: &mut App,
    events: Vec<egui::Event>,
    mut body: impl FnMut(&mut egui::Ui, &mut App),
) -> Vec<String> {
    let mut raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(500.0, 400.0),
        )),
        focused: true,
        time: Some((ctx.cumulative_pass_nr() + 1) as f64 / 20.0),
        events,
        ..Default::default()
    };
    raw.viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .focused = Some(true);
    let output = ctx.run_pass(raw, |ctx| {
        ctx.root_ui(|root| {
            egui::CentralPanel::default().show(root, |ui| body(ui, app));
        });
    });
    let copied = output
        .platform_output
        .commands
        .iter()
        .filter_map(|command| match command {
            egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    output.drop_without_applying_deltas();
    copied
}

fn handle(ui: &mut egui::Ui, app: &mut App) {
    let idx = app.fullscreen_idx.unwrap();
    let _ = app.handle_fs_key_input(ui.ctx(), idx, false);
}

fn copy_key(ctx: &egui::Context, app: &mut App) -> Vec<String> {
    frame(ctx, app, press(egui::Key::C, false), handle)
}

fn camera(idx: usize) -> String {
    format!("[カメラ]\nカメラ メーカー: camera-{idx}")
}

fn raw_gps() -> ExifInfo {
    exif_reader::read_exif_from_bytes(&exif_reader::tests::gps_fixture_bytes(35, 139), &[]).unwrap()
}

#[test]
fn exif_copy_button_and_key_match_even_with_sections_folded() {
    let _input = crate::key_input::lock_test_input();
    let mut app = spread_app(SpreadMode::Single);
    bind(&mut app, KeyName::C);
    let ctx = egui::Context::default();
    let expected = copy_key(&ctx, &mut app);
    assert_eq!(expected, vec![camera(0)]);
    let mut harness = crate::test_ui::builder()
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .with_size(egui::vec2(360.0, 300.0))
        .build_ui_state(
            |ui, (app, copied): &mut (AppTestEnvForTest, Vec<String>)| {
                let idx = app.metadata_info_target_idx().0;
                let exif = app.get_exif_for_idx(idx).unwrap();
                if draw_exif_panel(ui, &exif, &mut app.exif_sections_open).1 {
                    app.copy_exif_info_to_clipboard(ui.ctx(), &exif);
                }
                ui.ctx().output_mut(|out| {
                    out.commands.retain(|command| {
                        if let egui::OutputCommand::CopyText(text) = command {
                            copied.push(text.clone());
                            false
                        } else {
                            true
                        }
                    });
                });
            },
            (app, Vec::new()),
        );
    harness.run();
    harness.get_by_label("Copy").click();
    harness.run();
    assert_eq!(harness.state().1, expected);
    harness
        .state_mut()
        .0
        .exif_sections_open
        .insert("Camera".into(), false);
    harness.get_by_label("Copy").click();
    harness.run();
    assert_eq!(harness.state().1, vec![camera(0), camera(0)]);
    let app = &mut harness.state_mut().0;
    assert_eq!(copy_key(&ctx, app), expected);
    assert_eq!(
        app.fs_feedback_toast.as_ref().unwrap().0,
        "EXIF をコピーしました"
    );
    assert_eq!(
        app.fs_feedback_toast_surface,
        Some(crate::app::ActionSurface::Viewer)
    );
}

#[test]
fn exif_copy_uses_spread_choice_and_ignores_grid_selection_and_checks() {
    let _input = crate::key_input::lock_test_input();
    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
        let mut app = spread_app(mode);
        bind(&mut app, KeyName::C);
        app.selected = Some(3);
        app.checked.extend([2, 3]);
        let ctx = egui::Context::default();
        assert_eq!(copy_key(&ctx, &mut app), vec![camera(0)]);
        if mode != SpreadMode::Single {
            select(
                &mut app,
                if mode.is_rtl() {
                    AdjustSpreadTarget::Left
                } else {
                    AdjustSpreadTarget::Right
                },
            );
            assert_eq!(copy_key(&ctx, &mut app), vec![camera(1)]);
            select(
                &mut app,
                if mode.is_rtl() {
                    AdjustSpreadTarget::Right
                } else {
                    AdjustSpreadTarget::Left
                },
            );
            assert_eq!(copy_key(&ctx, &mut app), vec![camera(0)]);
        }
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(app.selected, Some(3));
        assert_eq!(app.checked.len(), 2);
    }
}

#[test]
fn exif_copy_closed_panel_keeps_choice_but_navigation_and_mode_roundtrip_reset_it() {
    let _input = crate::key_input::lock_test_input();
    let mut app = spread_app(SpreadMode::Ltr);
    bind(&mut app, KeyName::C);
    let ctx = egui::Context::default();
    app.metadata_info_target_idx();
    select(&mut app, AdjustSpreadTarget::Right);
    app.fs_info_panel = Default::default();
    for tab in [MetadataPanelTab::Info, MetadataPanelTab::Similar] {
        app.similar_panel.tab = tab;
        assert_eq!(copy_key(&ctx, &mut app), vec![camera(1)]);
    }
    app.open_fullscreen(2, crate::app::HistoryTrigger::UserChosen);
    app.open_fullscreen(0, crate::app::HistoryTrigger::UserChosen);
    assert_eq!(copy_key(&ctx, &mut app), vec![camera(0)]);
    select(&mut app, AdjustSpreadTarget::Right);
    assert!(app.apply_fullscreen_spread_mode(&ctx, 0, SpreadMode::Single));
    assert!(app.apply_fullscreen_spread_mode(&ctx, 0, SpreadMode::Ltr));
    assert_eq!(copy_key(&ctx, &mut app), vec![camera(0)]);
}

#[test]
fn exif_copy_loading_deduplicates_pending_and_never_reserves_clipboard_output() {
    let _input = crate::key_input::lock_test_input();
    let mut app = spread_app(SpreadMode::Single);
    bind(&mut app, KeyName::C);
    let ctx = egui::Context::default();
    let key = app.metadata_cache_key(0).unwrap();
    app.exif_cache.remove(&key);
    let (pending, cancel, sender) = MetadataLoadPending::controlled_for_test(key.clone());
    app.metadata_pending = Some(pending);
    assert!(copy_key(&ctx, &mut app).is_empty());
    assert!(app.metadata_pending.is_some());
    assert!(!cancel.load(Ordering::Relaxed));
    assert_eq!(
        app.fs_feedback_toast.as_ref().unwrap().0,
        "EXIF を読み込んでいます。少し待ってからもう一度実行してください"
    );
    let count = app.feedback_toasts_for_test.len();
    assert!(frame(&ctx, &mut app, press(egui::Key::C, true), handle).is_empty());
    assert_eq!(app.feedback_toasts_for_test.len(), count);
    assert!(!cancel.load(Ordering::Relaxed));
    app.fullscreen_idx = Some(1);
    assert!(
        sender
            .send(MetadataLoadResult::exif_for_test(key, Some(raw_gps())))
            .is_ok()
    );
    assert!(frame(&ctx, &mut app, vec![], |_, app| app.poll_metadata_load()).is_empty());
    assert!(app.metadata_pending.is_none());
    assert_eq!(copy_key(&ctx, &mut app), vec![camera(1)]);
}

#[test]
fn exif_copy_cache_miss_starts_existing_metadata_load_without_copying() {
    let _input = crate::key_input::lock_test_input();
    let mut app = spread_app(SpreadMode::Single);
    bind(&mut app, KeyName::C);
    let ctx = egui::Context::default();
    let key = app.metadata_cache_key(0).unwrap();
    app.exif_cache.remove(&key);
    assert!(app.metadata_pending.is_none());
    assert!(copy_key(&ctx, &mut app).is_empty());
    assert!(app.metadata_pending.is_some());
    assert_eq!(
        app.fs_feedback_toast.as_ref().unwrap().0,
        "EXIF を読み込んでいます。少し待ってからもう一度実行してください"
    );
    app.metadata_pending.as_ref().unwrap().cancel();
}

#[test]
fn exif_copy_hiding_gps_reference_rows_keeps_coordinates_and_map_button() {
    let raw = raw_gps();
    let visible = raw
        .visible(&["GPSLatitudeRef".into(), "GPSLongitudeRef".into()])
        .unwrap();
    assert_eq!(visible.gps_coordinates(), raw.gps_coordinates());
    let mut harness = crate::test_ui::builder()
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .build_ui(move |ui| {
            let _ = draw_exif_panel(ui, &visible, &mut Default::default());
        });
    harness.run();
    harness.get_by_label("Google マップで開く");
}

#[test]
fn exif_copy_empty_hidden_and_unavailable_targets_do_not_write_clipboard() {
    let _input = crate::key_input::lock_test_input();
    let mut app = spread_app(SpreadMode::Single);
    bind(&mut app, KeyName::C);
    let ctx = egui::Context::default();
    let key = app.metadata_cache_key(0).unwrap();
    app.settings.exif_hidden_tags = vec!["Make".into()];
    assert!(copy_key(&ctx, &mut app).is_empty());
    assert_eq!(
        app.fs_feedback_toast.as_ref().unwrap().0,
        "この画像にはコピーできる EXIF がありません"
    );
    app.exif_cache.insert(key, None);
    assert!(copy_key(&ctx, &mut app).is_empty());
    assert_eq!(
        app.fs_feedback_toast.as_ref().unwrap().0,
        "この画像にはコピーできる EXIF がありません"
    );
    let count = app.feedback_toasts_for_test.len();
    app.fullscreen_idx = None;
    assert!(
        frame(&ctx, &mut app, vec![], |ui, app| app
            .copy_current_exif_to_clipboard(ui.ctx()))
        .is_empty()
    );
    app.fullscreen_idx = Some(usize::MAX);
    assert!(
        frame(&ctx, &mut app, vec![], |ui, app| app
            .copy_current_exif_to_clipboard(ui.ctx()))
        .is_empty()
    );
    app.fullscreen_idx = Some(0);
    app.items[0] = GridItem::Folder(app.tmp.path().to_path_buf());
    assert!(
        frame(&ctx, &mut app, vec![], |ui, app| app
            .copy_current_exif_to_clipboard(ui.ctx()))
        .is_empty()
    );
    assert_eq!(app.feedback_toasts_for_test.len(), count);
}

#[test]
fn exif_copy_late_unfiltered_result_uses_current_hidden_settings_and_restores_without_reload() {
    let _input = crate::key_input::lock_test_input();
    let mut app = spread_app(SpreadMode::Single);
    bind(&mut app, KeyName::C);
    let ctx = egui::Context::default();
    let key = app.metadata_cache_key(0).unwrap();
    app.exif_cache.remove(&key);
    let (pending, cancel, sender) = MetadataLoadPending::controlled_for_test(key.clone());
    app.metadata_pending = Some(pending);
    let raw = raw_gps();
    let hidden: Vec<_> = raw
        .sections
        .iter()
        .flat_map(|(_, fields)| fields.iter().map(|(tag, _)| tag.clone()))
        .collect();
    app.settings.exif_hidden_tags = hidden;
    assert!(
        sender
            .send(MetadataLoadResult::exif_for_test(key, Some(raw)))
            .is_ok()
    );
    app.poll_metadata_load();
    assert!(app.get_exif_for_idx(Some(0)).is_none());
    assert!(copy_key(&ctx, &mut app).is_empty());
    app.settings.exif_hidden_tags.clear();
    assert!(
        app.get_exif_for_idx(Some(0))
            .unwrap()
            .gps_coordinates()
            .is_some()
    );
    assert!(copy_key(&ctx, &mut app)[0].starts_with("[GPS]\n"));
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(app.metadata_pending.is_none());
    assert_eq!(app.exif_cache.len(), 4);
}

#[test]
fn exif_copy_fullscreen_gates_leave_key_and_copy_cut_events_unconsumed() {
    let _input = crate::key_input::lock_test_input();
    for gate in ["video", "audio", "video-audio", "menu", "modal"] {
        let mut app = spread_app(SpreadMode::Single);
        bind(&mut app, KeyName::C);
        match gate {
            "video" | "video-audio" => {
                app.items[0] = GridItem::Video(app.tmp.path().join("clip.mp4"));
                if gate == "video-audio" {
                    app.video_audio_mode = Some(0);
                }
            }
            "audio" => app.items[0] = GridItem::Audio(app.tmp.path().join("song.mp3")),
            "menu" => app.fs_context_menu_idx = Some(0),
            "modal" => app.show_about_dialog = true,
            _ => unreachable!(),
        }
        let ctx = egui::Context::default();
        let mut events = press(egui::Key::C, false);
        events.extend([egui::Event::Copy, egui::Event::Cut]);
        assert!(
            frame(&ctx, &mut app, events, |ui, app| {
                handle(ui, app);
                ui.ctx().input(|i| {
                    assert!(
                        i.events.iter().any(|e| matches!(
                            e,
                            egui::Event::Key {
                                key: egui::Key::C,
                                pressed: true,
                                ..
                            }
                        )),
                        "{gate}"
                    );
                    assert!(
                        i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                        "{gate}"
                    );
                    assert!(
                        i.events.iter().any(|e| matches!(e, egui::Event::Cut)),
                        "{gate}"
                    );
                });
            })
            .is_empty()
        );
        assert!(app.feedback_toasts_for_test.is_empty(), "{gate}");
    }
}

#[test]
fn exif_copy_textedit_focus_keeps_normal_copy_and_does_not_consume_action() {
    let _input = crate::key_input::lock_test_input();
    let mut app = spread_app(SpreadMode::Single);
    bind(&mut app, KeyName::C);
    let ctx = egui::Context::default();
    let mut text = "selected text".to_string();
    let id = egui::Id::unique("exif copy focused text fixture");
    let mut request_focus = true;
    for _ in 0..2 {
        frame(&ctx, &mut app, vec![], |ui, _| {
            let mut output = crate::ime_focus::show_singleline(
                ui,
                &mut text,
                Some(&mut request_focus),
                |edit| edit.id(id),
            );
            output
                .state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(text.len()),
                )));
            output.state.store(ui.ctx(), id);
        });
    }
    let mut events = press(egui::Key::C, false);
    events.push(egui::Event::Copy);
    let copied = frame(&ctx, &mut app, events, |ui, app| {
        handle(ui, app);
        assert!(ui.ctx().input(|i| i.events.iter().any(|e| matches!(
            e,
            egui::Event::Key {
                key: egui::Key::C,
                pressed: true,
                ..
            }
        ))));
        let _ = crate::ime_focus::show_singleline(ui, &mut text, None, |edit| edit.id(id));
    });
    assert_eq!(copied, vec!["selected text"]);
    assert!(app.feedback_toasts_for_test.is_empty());
}

#[test]
fn exif_copy_owns_copy_and_cut_with_label_selection_on_initial_and_repeat_passes() {
    let _input = crate::key_input::lock_test_input();
    for (key_name, key, semantic) in [
        (KeyName::C, egui::Key::C, egui::Event::Copy),
        (KeyName::X, egui::Key::X, egui::Event::Cut),
    ] {
        let mut app = spread_app(SpreadMode::Single);
        bind(&mut app, key_name);
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let label = |ui: &mut egui::Ui| {
            ui.scope(|ui| draw_key_value_wrapped(ui, "Make", "selectable label text"))
                .response
                .rect
        };
        let mut label_rect = egui::Rect::NOTHING;
        let mut draw = |ui: &mut egui::Ui, app: &mut App| {
            handle(ui, app);
            label_rect = label(ui);
        };
        frame(&ctx, &mut app, vec![], &mut draw);
        drop(draw);
        let from = label_rect.left_center() + egui::vec2(1.0, 0.0);
        let to = label_rect.right_center() - egui::vec2(1.0, 0.0);
        let edge = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let draw = |ui: &mut egui::Ui, app: &mut App| {
            handle(ui, app);
            label(ui);
        };
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(from), edge(from, true)],
            draw,
        );
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(to)], draw);
        frame(&ctx, &mut app, vec![edge(to, false)], draw);
        // Prove a real, surviving label selection would otherwise write at end-pass.
        let selected = frame(&ctx, &mut app, vec![semantic.clone()], draw);
        assert_eq!(selected.len(), 1);
        assert!("Make:  selectable label text".contains(&selected[0]));
        app.feedback_toasts_for_test.clear();
        let mut events = press(key, false);
        events.push(semantic.clone());
        assert_eq!(frame(&ctx, &mut app, events, draw), vec![camera(0)]);
        let count = app.feedback_toasts_for_test.len();
        let mut events = press(key, true);
        events.push(semantic.clone());
        assert!(frame(&ctx, &mut app, events, draw).is_empty());
        assert_eq!(app.feedback_toasts_for_test.len(), count);
        assert_eq!(frame(&ctx, &mut app, vec![semantic], draw), selected);
    }
}

#[cfg(windows)]
#[test]
fn exif_copy_context_switch_uses_own_cache_choice_pending_and_latest_hidden_settings() {
    let _input = crate::key_input::lock_test_input();
    let mut app = spread_app(SpreadMode::Ltr);
    bind(&mut app, KeyName::C);
    let ctx = egui::Context::default();
    app.metadata_info_target_idx();
    select(&mut app, AdjustSpreadTarget::Right);
    let main_target = app.similar_panel.info_target.clone();
    let main_cache = format!("{:?}", app.exif_cache);
    let (pending, main_cancel, main_sender) =
        MetadataLoadPending::controlled_for_test("main-pending".into());
    app.metadata_pending = Some(pending);
    let mut sibling_cancel = None;
    let mut sibling_sender = None;
    let sibling = app.build_window_context_for_test(1889, |app| {
        app.items = vec![
            GridItem::Image(PathBuf::from("C:/exif-fixture/a.jpg")),
            GridItem::Image(PathBuf::from("C:/exif-fixture/b.jpg")),
        ];
        app.image_metas = vec![Some((1, 1)); 2];
        app.fullscreen_idx = Some(0);
        app.spread_mode = SpreadMode::Ltr;
        let key = app.metadata_cache_key(0).unwrap();
        app.exif_cache.insert(key, Some(raw_gps()));
        app.metadata_info_target_idx();
        let (pending, cancel, sender) =
            MetadataLoadPending::controlled_for_test("sibling-pending".into());
        app.metadata_pending = Some(pending);
        sibling_cancel = Some(cancel);
        sibling_sender = Some(sender);
    });
    assert_eq!(copy_key(&ctx, &mut app), vec![camera(1)]);
    let hidden: Vec<_> = raw_gps()
        .sections
        .iter()
        .flat_map(|(_, fields)| fields.iter().map(|(tag, _)| tag.clone()))
        .collect();
    app.settings.exif_hidden_tags = hidden;
    app.with_window_context_for_test(sibling, |app| {
        let target = app.similar_panel.info_target.clone();
        let cache = format!("{:?}", app.exif_cache);
        assert!(app.get_exif_for_idx(Some(0)).is_none());
        assert!(copy_key(&ctx, app).is_empty());
        app.settings.exif_hidden_tags.clear();
        assert!(copy_key(&ctx, app)[0].starts_with("[GPS]\n"));
        assert_eq!(app.similar_panel.info_target, target);
        assert_eq!(format!("{:?}", app.exif_cache), cache);
        assert!(app.metadata_pending.is_some());
    });
    assert_eq!(app.similar_panel.info_target, main_target);
    assert_eq!(format!("{:?}", app.exif_cache), main_cache);
    assert!(app.metadata_pending.is_some());
    assert!(!main_cancel.load(Ordering::Relaxed));
    assert!(!sibling_cancel.unwrap().load(Ordering::Relaxed));
    assert!(
        main_sender
            .send(MetadataLoadResult::exif_for_test(
                "main-pending".into(),
                None
            ))
            .is_ok()
    );
    assert!(
        sibling_sender
            .unwrap()
            .send(MetadataLoadResult::exif_for_test(
                "sibling-pending".into(),
                None
            ))
            .is_ok()
    );
}
