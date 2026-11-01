use super::*;
use egui_kittest::kittest::{NodeT, Queryable};

#[test]
fn exposure_descriptions_are_left_aligned_without_justification_and_still_wrap() {
    fn check_text(shape: &egui::Shape, found: &mut bool) {
        match shape {
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    check_text(shape, found);
                }
            }
            egui::Shape::Text(text) if text.galley.job.text.contains("UserComment") => {
                *found = true;
                assert!(!text.galley.job.justify);
                assert_eq!(text.galley.job.halign, egui::Align::Min);
                assert!(text.galley.rows.len() > 1, "description must still wrap");
            }
            _ => {}
        }
    }
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 1000.0),
            )),
            ..Default::default()
        },
        |ui| {
            draw_exposure(
                ui,
                &PermissionSelection {
                    level: AgentLevel::ReadOnly,
                    thumbnails: false,
                    gps: false,
                },
            );
        },
    );
    let mut found = false;
    for shape in &output.shapes {
        check_text(&shape.shape, &mut found);
    }
    assert!(
        found,
        "the actual generation-info description must be drawn"
    );
    output.drop_without_applying_deltas();
}

fn injected(_settings: &Settings) -> AgentIntegrationState {
    let env = backend::RegistrationEnvironment {
        local_app_data: None,
        app_data: None,
        user_profile: None,
        codex_home: None,
        launcher_exe: None,
        current_exe: PathBuf::from(r"C:\fixture\mimageviewer-core.exe"),
        data_dir: PathBuf::from(r"C:\fixture\data"),
        working_dir: PathBuf::from(r"C:\fixture"),
    };
    let command = backend::RegistrationCommand {
        command: r"C:\fixture\mimageviewer-mcp.exe".into(),
        args: vec!["--data-dir".into(), r"C:\fixture\data".into()],
    };
    let detection = backend::Detection {
        command: Ok(command.clone()),
        apps: [AppTarget::ClaudeDesktop, AppTarget::ChatGpt]
            .into_iter()
            .map(|target| backend::AppRegistration {
                target,
                state: RegistrationState::Registered,
                config_paths: vec![PathBuf::from(format!(
                    r"C:\fixture\{}\config",
                    target.name()
                ))],
                message: None,
                previews: {
                    let mut text = command.snippet(target);
                    if target == AppTarget::ClaudeDesktop {
                        // Existing registered JSON may need formatting on rewrite.
                        text = serde_json::from_str::<serde_json::Value>(&text)
                            .unwrap()
                            .to_string();
                    }
                    backend::RegistrationPreviews::from_config(target, &text, Some(&command))
                        .unwrap()
                },
            })
            .collect(),
    };
    let mut state = AgentIntegrationState::new();
    state.detection = DetectionState::Ready(env.clone(), detection.clone());
    state.mock_backend = Some(MockBackend {
        env,
        detection,
        writes: 0,
        write_result: Ok("登録しました".into()),
    });
    state
}

#[test]
fn agent_exposure_all_sixteen_permission_combinations() {
    let mut count = 0;
    for level in [
        AgentLevel::Disabled,
        AgentLevel::ReadOnly,
        AgentLevel::ReadAndOpen,
        AgentLevel::Organize,
    ] {
        for thumbnails in [false, true] {
            for gps in [false, true] {
                let result = exposure(PermissionSelection {
                    level,
                    thumbnails,
                    gps,
                });
                let enabled = level != AgentLevel::Disabled;
                assert_eq!(
                    result.shown.iter().any(|line| line.contains("絶対パス")),
                    enabled
                );
                assert_eq!(
                    result
                        .shown
                        .iter()
                        .any(|line| line.contains("縮小画像 (補正")),
                    enabled && thumbnails
                );
                assert_eq!(result.shown.contains(&"撮影位置 (GPS)"), enabled && gps);
                assert_eq!(
                    result
                        .actions
                        .contains(&"mIV の画面で開く (表示が切り替わる)"),
                    level >= AgentLevel::ReadAndOpen
                );
                assert_eq!(
                    result
                        .actions
                        .contains(&"タグ・★・コレクションの変更 (確認なし。1 回 500 件まで)"),
                    level == AgentLevel::Organize
                );
                assert!(result.hidden.contains(&"画像・動画・音声の元データ"));
                assert!(result.hidden.contains(&"PDF / ZIP の中身の本文"));
                assert!(result.hidden.iter().any(|line| line.contains("パスワード")));
                if enabled && !gps {
                    assert!(result.hidden.contains(&"撮影位置 (GPS)"));
                }
                count += 1;
            }
        }
    }
    assert_eq!(count, 16);
}

#[test]
fn agent_registration_single_owner_without_preferences_draft_gating() {
    for action in [Action::Register, Action::Remove] {
        let mut state = injected(&Settings::default());
        state.confirm(AppTarget::ClaudeDesktop, action, 0);
        assert!(matches!(state.operation,
            Some(RegistrationOperation::Confirming { ref request, .. })
                if request.target == AppTarget::ClaudeDesktop));
        state.confirm(AppTarget::ChatGpt, action, 0);
        assert!(matches!(state.operation,
            Some(RegistrationOperation::Confirming { ref request, .. })
                if request.target == AppTarget::ClaudeDesktop));
    }
}

#[test]
fn agent_registration_mock_completion_and_disconnection_release_single_owner() {
    let settings = Settings::default();
    let ctx = egui::Context::default();
    let mut state = injected(&settings);
    state.confirm(AppTarget::ClaudeDesktop, Action::Register, 0);
    state.submit(&ctx);
    assert!(matches!(
        state.operation,
        Some(RegistrationOperation::Writing { .. })
    ));
    state.poll(&ctx);
    assert!(state.operation.is_none());
    assert!(state.feedback.as_ref().unwrap().1.is_ok());
    state.poll(&ctx); // Mock detection only, never personal settings.
    let (tx, rx) = mpsc::channel();
    drop(tx);
    state.operation = Some(RegistrationOperation::Writing {
        target: AppTarget::ClaudeDesktop,
        rx,
    });
    state.poll(&ctx);
    assert!(state.operation.is_none());
    assert!(
        state
            .feedback
            .as_ref()
            .unwrap()
            .1
            .as_ref()
            .unwrap_err()
            .contains("失敗")
    );
    let (tx, rx) = mpsc::channel();
    drop(tx);
    state.detection = DetectionState::Checking(rx);
    state.poll(&ctx);
    assert!(
        matches!(state.detection, DetectionState::Failed(ref error) if error == "確認に失敗しました")
    );
}

#[test]
fn agent_registration_remove_survives_missing_registration_executable() {
    let settings = Settings::default();
    let mut state = injected(&settings);
    if let DetectionState::Ready(_, detection) = &mut state.detection {
        detection.command = Err("exe missing".into());
    }
    state.confirm(AppTarget::ClaudeDesktop, Action::Register, 0);
    assert!(state.operation.is_none());
    state.confirm(AppTarget::ClaudeDesktop, Action::Remove, 0);
    assert!(state.operation.is_some());
}

fn click_at(
    harness: &mut egui_kittest::Harness<'_, crate::app::AppTestEnvForTest>,
    position: egui::Pos2,
) {
    harness.event(egui::Event::PointerMoved(position));
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run_steps(1);
    }
}

fn app_harness() -> egui_kittest::Harness<'static, crate::app::AppTestEnvForTest> {
    app_harness_at_size(egui::vec2(1093.0, 614.0))
}

fn app_harness_at_size(
    size: egui::Vec2,
) -> egui_kittest::Harness<'static, crate::app::AppTestEnvForTest> {
    let mut app = crate::app::setup_app_for_test();
    app.open_agent_integration_dialog();
    let integration = injected(&app.settings);
    app.agent_integration_dialog = Some(integration);
    crate::test_ui::builder()
        .with_context_setup(|ctx| {
            crate::ui_fonts::configure_fonts(ctx);
            crate::ime_focus::install_ime_input_policy(ctx);
        })
        .with_size(size)
        .build_ui_state(
            |root, app| {
                app.update_ime_state(root.ctx());
                app.show_agent_integration_dialog(&root.ctx().clone());
            },
            app,
        )
}

fn mock_writes(harness: &egui_kittest::Harness<'_, crate::app::AppTestEnvForTest>) -> usize {
    harness
        .state()
        .agent_integration_dialog
        .as_ref()
        .unwrap()
        .mock_backend
        .as_ref()
        .unwrap()
        .writes
}

fn assert_confirming(harness: &egui_kittest::Harness<'_, crate::app::AppTestEnvForTest>) {
    assert!(matches!(
        harness
            .state()
            .agent_integration_dialog
            .as_ref()
            .unwrap()
            .operation,
        Some(RegistrationOperation::Confirming { .. })
    ));
    assert_eq!(mock_writes(harness), 0);
}

/// Activate the real app row, rather than assigning a confirmation directly.
fn open_confirmation_with_enter(
    label: &str,
) -> egui_kittest::Harness<'static, crate::app::AppTestEnvForTest> {
    let mut harness = app_harness();
    activate_confirmation_with_enter(&mut harness, label);
    harness
}

fn activate_confirmation_with_enter(
    harness: &mut egui_kittest::Harness<'_, crate::app::AppTestEnvForTest>,
    label: &str,
) {
    harness.run_steps(5);
    let scroll = harness
        .state()
        .agent_integration_dialog
        .as_ref()
        .unwrap()
        .page_scroll
        .unwrap();
    harness.event(egui::Event::PointerMoved(scroll.inner.center()));
    harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -100000.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
    });
    harness.run_steps(40);
    let button = harness.get_all_by_label(label).next().unwrap();
    assert!(
        scroll.inner.contains_rect(button.rect()),
        "real app-row button must be visible"
    );
    button.focus();
    harness.run_steps(2);
    assert!(harness.ctx.memory(|memory| memory.focused().is_some()));
    harness.key_press(egui::Key::Enter);
    harness.run_steps(3);
    assert_confirming(harness);
}

fn begin_composition(harness: &mut egui_kittest::Harness<'_, crate::app::AppTestEnvForTest>) {
    harness.event(egui::Event::Ime(egui::ImeEvent::Enabled));
    harness.event(egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "変換中".into(),
        active_range_chars: None,
    }));
    harness.run_steps(1);
    assert!(harness.state().ime_input_active(&harness.ctx));
}

#[test]
fn agent_registration_real_row_enter_opens_confirmation_without_writing() {
    for label in ["登録する", "登録を外す"] {
        let harness = open_confirmation_with_enter(label);
        let operation = &harness
            .state()
            .agent_integration_dialog
            .as_ref()
            .unwrap()
            .operation;
        assert!(
            matches!(operation, Some(RegistrationOperation::Confirming { request, .. })
            if request.target == AppTarget::ClaudeDesktop
                && request.action == if label == "登録する" { Action::Register } else { Action::Remove })
        );
    }
}

#[test]
fn agent_registration_confirmation_shows_the_worker_prepared_replacement() {
    let mut harness = app_harness();
    let preview = backend::RegistrationPreview {
        removed: "# actual previous registration\n".into(),
        added: "# actual edited registration\n".into(),
    };
    let integration = harness
        .state_mut()
        .agent_integration_dialog
        .as_mut()
        .unwrap();
    let DetectionState::Ready(_, detection) = &mut integration.detection else {
        panic!("injected detection is ready");
    };
    detection.apps[0].previews.register = Some(preview.clone());
    activate_confirmation_with_enter(&mut harness, "登録する");
    assert!(harness.query_by_label(&preview.removed).is_some());
    assert!(harness.query_by_label(&preview.added).is_some());
    assert_confirming(&harness);
}

#[test]
fn agent_registration_focused_copy_enter_does_not_write() {
    let mut harness = open_confirmation_with_enter("登録する");
    for index in 0..2 {
        harness
            .get_all_by_label("コピー")
            .nth(index)
            .unwrap()
            .focus();
        harness.run_steps(2);
        harness.key_press(egui::Key::Enter);
        harness.run_steps(3);
        assert_confirming(&harness);
    }
    // egui uses Escape to clear widget focus before drawing the dialog. It can
    // then cancel the confirmation, but must never invoke a write.
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    assert_eq!(mock_writes(&harness), 0);
    assert!(!harness.state().agent_registration_dialog_open());
    assert!(harness.state().agent_integration_dialog.is_some());
}

#[test]
fn agent_registration_focused_action_buttons_ignore_ime_keyboard_clicks() {
    for label in ["書き換える", "キャンセル"] {
        for key in [egui::Key::Enter, egui::Key::Space] {
            let mut harness = open_confirmation_with_enter("登録する");
            // The parent has its own Cancel; choose the enabled foreground button.
            harness
                .get_all_by_label(label)
                .find(|node| !node.accesskit_node().is_disabled())
                .unwrap()
                .focus();
            harness.run_steps(2);
            begin_composition(&mut harness);
            harness.key_press(key);
            harness.run_steps(3);
            assert_confirming(&harness);
        }
    }
}

#[test]
fn agent_registration_pointer_and_unfocused_dialog_enter_write() {
    for (pointer, composing) in [(true, false), (true, true), (false, false)] {
        let mut harness = open_confirmation_with_enter("登録する");
        if composing {
            begin_composition(&mut harness);
        }
        if pointer {
            let position = harness.get_by_label("書き換える").rect().center();
            click_at(&mut harness, position);
        } else {
            harness.ctx.memory_mut(|memory| {
                if let Some(id) = memory.focused() {
                    memory.surrender_focus(id);
                }
            });
            assert!(harness.ctx.memory(|memory| memory.focused().is_none()));
            harness.key_press(egui::Key::Enter);
        }
        harness.run_steps(3);
        assert_eq!(mock_writes(&harness), 1);
        assert!(
            harness
                .state()
                .agent_integration_dialog
                .as_ref()
                .unwrap()
                .feedback
                .as_ref()
                .unwrap()
                .1
                .is_ok()
        );
    }
}

#[test]
fn agent_registration_cancel_button_enter_and_unfocused_escape_only_dismiss() {
    for focused_cancel in [true, false] {
        let mut harness = open_confirmation_with_enter("登録する");
        if focused_cancel {
            harness
                .get_all_by_label("キャンセル")
                .find(|node| !node.accesskit_node().is_disabled())
                .unwrap()
                .focus();
            harness.run_steps(2);
            harness.key_press(egui::Key::Enter);
        } else {
            harness.ctx.memory_mut(|memory| {
                if let Some(id) = memory.focused() {
                    memory.surrender_focus(id);
                }
            });
            harness.key_press(egui::Key::Escape);
        }
        harness.run_steps(3);
        assert_eq!(mock_writes(&harness), 0);
        assert!(!harness.state().agent_registration_dialog_open());
        assert!(harness.state().agent_integration_dialog.is_some());
    }
}

#[test]
fn agent_registration_confirmation_and_writing_block_backdrop_settings_and_x() {
    let mut harness = app_harness_at_size(egui::vec2(1600.0, 1000.0));
    harness.run_steps(4);
    let thumbnail = harness
        .get_by_label("縮小画像を AI に見せる")
        .rect()
        .center();
    let close = harness.get_by_label("Close window").rect().center();
    harness
        .state_mut()
        .agent_integration_dialog
        .as_mut()
        .unwrap()
        .confirm(AppTarget::ClaudeDesktop, Action::Register, 0);
    harness.run_steps(3);
    let modal_id = egui::Id::new("agent_registration_confirmation");
    let modal = harness
        .ctx
        .memory(|memory| memory.area_rect(modal_id))
        .unwrap();
    let start = modal.min + egui::vec2(80.0, 12.0);
    let end = start + (egui::pos2(1000.0, 80.0) - modal.min);
    harness.hover_at(start);
    harness.run_steps(1);
    harness.drag_at(start);
    harness.run_steps(1);
    harness.hover_at(end);
    harness.run_steps(2);
    harness.drop_at(end);
    harness.run_steps(2);
    let (tx, rx) = mpsc::channel();
    let mut receiver = Some(rx);
    for writing in [false, true] {
        if writing {
            harness
                .state_mut()
                .agent_integration_dialog
                .as_mut()
                .unwrap()
                .operation = Some(RegistrationOperation::Writing {
                target: AppTarget::ClaudeDesktop,
                rx: receiver.take().unwrap(),
            });
        }
        harness.run_steps(2);
        assert!(
            harness
                .get_by_label("縮小画像を AI に見せる")
                .accesskit_node()
                .is_disabled()
        );
        assert!(
            harness
                .get_all_by_label("Close window")
                .any(|node| node.accesskit_node().is_disabled() && node.rect().center() == close)
        );
        let foreground = harness
            .ctx
            .memory(|memory| memory.area_rect(modal_id))
            .unwrap();
        for position in [thumbnail, close] {
            assert!(!foreground.contains(position), "backdrop must be uncovered");
            click_at(&mut harness, position);
            assert!(harness.state().agent_registration_dialog_open());
            assert!(!harness.state().settings.agent_show_thumbnails);
        }
    }
    tx.send(Err("injected failure".into())).unwrap();
    harness.run_steps(4);
    assert!(!harness.state().agent_registration_dialog_open());
    assert!(harness.state().agent_integration_dialog.is_some());
}

#[test]
fn agent_registration_confirmation_ime_enter_escape_and_cancel_return() {
    for key in [egui::Key::Enter, egui::Key::Escape] {
        let mut harness = app_harness();
        harness.run_steps(3);
        harness
            .state_mut()
            .agent_integration_dialog
            .as_mut()
            .unwrap()
            .confirm(AppTarget::ClaudeDesktop, Action::Register, 0);
        harness.run_steps(3);
        harness.input_mut().events.extend([
            egui::Event::Ime(egui::ImeEvent::Enabled),
            egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "編集中".into(),
                active_range_chars: None,
            }),
        ]);
        harness.run_steps(1);
        assert!(harness.state().ime_input_active(&harness.ctx));
        harness.key_press(key);
        harness.run_steps(2);
        assert!(matches!(
            harness
                .state()
                .agent_integration_dialog
                .as_ref()
                .unwrap()
                .operation,
            Some(RegistrationOperation::Confirming { .. })
        ));
        harness.event(egui::Event::Ime(egui::ImeEvent::Disabled));
        harness.run_steps(2);
        harness.key_press(egui::Key::Escape);
        harness.run_steps(2);
        assert!(!harness.state().agent_registration_dialog_open());
        assert!(harness.state().agent_integration_dialog.is_some());
    }
}

#[test]
fn agent_registration_long_confirmation_controls_and_scroll_small_200_percent() {
    use crate::test_ui::scroll_layout::HeightExpectation;
    for scale in [1.0, 2.0] {
        let settings = Settings::default();
        let mut state = injected(&settings);
        if let DetectionState::Ready(_, detection) = &mut state.detection {
            detection.apps[0].config_paths = vec![PathBuf::from(format!(
                r"C:\{}\config.json",
                "長いフォルダ名\\".repeat(100)
            ))];
        }
        state.confirm(AppTarget::ClaudeDesktop, Action::Register, 0);
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(1093.0, 614.0))
            .with_context_setup(move |ctx| {
                crate::ui_fonts::configure_fonts(ctx);
                crate::settings::apply_ui_scale_factor(ctx, scale);
            })
            .build_ui_state(
                |root, state| draw_registration_dialog(root.ctx(), state, false, false, false),
                state,
            );
        harness.run_steps(8);
        for label in ["書き換える", "キャンセル", "Close window"] {
            assert!(
                harness
                    .ctx
                    .content_rect()
                    .contains_rect(harness.get_by_label(label).rect()),
                "{label} scale={scale}"
            );
        }
        let before = harness.state().dialog_scroll.unwrap();
        before.assert_height(
            HeightExpectation::Scrolls,
            harness.ctx.content_rect(),
            harness.ctx.pixels_per_point(),
        );
        harness.event(egui::Event::PointerMoved(before.inner.center()));
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -100000.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
        });
        harness.run_steps(40);
        harness
            .state()
            .dialog_scroll
            .unwrap()
            .assert_scrolled_to_end(before, harness.ctx.pixels_per_point());
        harness.get_by_label("キャンセル").click();
        harness.run_steps(2);
        assert!(harness.state().operation.is_none());
    }
}

#[test]
fn agent_integration_window_small_200_percent_controls_and_end_reachable() {
    use crate::test_ui::scroll_layout::HeightExpectation;
    for scale in [1.0, 2.0] {
        let mut app = crate::app::setup_app_for_test();
        app.open_agent_integration_dialog();
        let mut integration = injected(&app.settings);
        if let DetectionState::Ready(_, detection) = &mut integration.detection {
            for row in &mut detection.apps {
                row.message = Some(format!(
                    "確認に失敗しました: {}",
                    "非常に長いエラー文\\".repeat(60)
                ));
                row.config_paths = vec![PathBuf::from(format!(
                    r"C:\{}\config",
                    "長いフォルダ名\\".repeat(100)
                ))];
            }
        }
        integration.instructions = Some(AppTarget::ChatGpt);
        app.agent_integration_dialog = Some(integration);
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(1093.0, 614.0))
            .with_context_setup(move |ctx| {
                crate::ui_fonts::configure_fonts(ctx);
                crate::settings::apply_ui_scale_factor(ctx, scale);
            })
            .build_ui_state(
                |root, app| app.show_agent_integration_dialog(&root.ctx().clone()),
                app,
            );
        harness.run_steps(8);
        let viewport = harness.ctx.content_rect();
        assert!(viewport.contains_rect(harness.get_by_label("Close window").rect()));
        let before = harness
            .state()
            .agent_integration_dialog
            .as_ref()
            .unwrap()
            .page_scroll
            .unwrap();
        before.assert_height(
            HeightExpectation::Scrolls,
            viewport,
            harness.ctx.pixels_per_point(),
        );
        assert!(before.content.x <= before.inner.width() + 1.0);
        harness.event(egui::Event::PointerMoved(before.inner.center()));
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -100000.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
        });
        harness.run_steps(40);
        harness
            .state()
            .agent_integration_dialog
            .as_ref()
            .unwrap()
            .page_scroll
            .unwrap()
            .assert_scrolled_to_end(before, harness.ctx.pixels_per_point());
        let last_copy = harness.get_all_by_label("コピー").last().unwrap().rect();
        assert!(viewport.contains_rect(last_copy));
        // The app controls precede the expanded instructions. With this long
        // path, those instructions alone exceed the viewport, so reaching their
        // last copy button puts the earlier controls above the scroll clip.
        // Navigate back with real wheel input and verify the control itself.
        let scroll = harness
            .state()
            .agent_integration_dialog
            .as_ref()
            .unwrap()
            .page_scroll
            .unwrap();
        let settings_button = harness.get_by_label("設定を開く").rect();
        harness.event(egui::Event::PointerMoved(scroll.inner.center()));
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, scroll.inner.center().y - settings_button.center().y),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
        });
        harness.run_steps(40);
        let settings_button = harness.get_by_label("設定を開く").rect();
        let scroll = harness
            .state()
            .agent_integration_dialog
            .as_ref()
            .unwrap()
            .page_scroll
            .unwrap();
        assert!(
            viewport.contains_rect(settings_button) && scroll.inner.contains_rect(settings_button),
            "settings control must be reachable: scale={scale}, viewport={viewport:?}, scroll={scroll:?}, button={settings_button:?}"
        );
    }
}

#[test]
fn agent_recent_table_uses_page_width_and_natural_top_aligned_rows() {
    for width in [900.0, 360.0, 180.0] {
        let mut recent = vec![(
            "Claude Desktop".into(),
            "1.0".into(),
            "2026-11-01 12:34:56".into(),
        )];
        recent.extend((1..8).map(|_| {
            (
                "長".repeat(128),
                "v".repeat(64),
                "2026-11-01 12:34:56".into(),
            )
        }));
        let mut harness =
            crate::test_ui::builder()
                .with_size(egui::vec2(width, 3000.0))
                .with_context_setup(crate::ui_fonts::configure_fonts)
                .build_ui_state(
                    |root,
                     (recent, measured): &mut (
                        Vec<(String, String, String)>,
                        Vec<[egui::Rect; 3]>,
                    )| {
                        *measured = draw_recent_table(root, recent);
                    },
                    (recent, Vec::new()),
                );
        harness.run_steps(5);
        let cells = &harness.state().1;
        assert_eq!(cells.len(), 8);
        let viewport = harness.ctx.content_rect();
        for (index, row) in cells.iter().enumerate() {
            let top = row[0].top();
            for cell in row {
                assert!(
                    (cell.top() - top).abs() <= 1.0,
                    "cells must be top aligned width={width}: {row:?}"
                );
                assert!(
                    cell.left() >= viewport.left() && cell.right() <= viewport.right(),
                    "column escaped page width={width}: {cell:?}"
                );
            }
            if index > 0 {
                let previous_bottom = cells[index - 1]
                    .iter()
                    .map(egui::Rect::bottom)
                    .fold(f32::NEG_INFINITY, f32::max);
                assert!(
                    (top - previous_bottom - 3.0).abs() <= 3.0,
                    "row adds blank height width={width}: previous={previous_bottom} next={top}"
                );
            }
        }
        if width == 900.0 {
            for cell in &cells[0] {
                assert!(
                    cell.height() < 24.0,
                    "normal name/date must fit a single natural line: {cell:?}"
                );
            }
            assert!(
                cells[0][2].width() > 100.0,
                "timestamp must remain readable"
            );
        }
    }
}

fn change_fixture(
    rows: usize,
) -> (
    AgentIntegrationState,
    crate::agent_ipc::changes::ChangeRecordId,
) {
    use crate::agent_ipc::changes::{
        ChangeController, ChangeCounts, ChangeRecord, ChangeRow, ChangeRowOutcome, ChangeRowResult,
    };
    let settings = Settings::default();
    let mut state = injected(&settings);
    let controller = ChangeController::new();
    let id = controller.publish(ChangeRecord {
        id: Default::default(),
        at: std::time::UNIX_EPOCH,
        client_name: "名乗った AI アプリの長い名前".repeat(10),
        tool: "add_to_collection".into(),
        collection: Some(crate::agent_ipc::changes::CollectionTarget {
            id: "collection-long-name".into(),
            name: "長いコレクション名".repeat(20),
        }),
        counts: ChangeCounts {
            changed: rows,
            ..Default::default()
        },
        summary: "非常に長いコレクション名への追加".repeat(12),
        rows: (0..rows)
            .map(|index| ChangeRow {
                item: format!(r"C:\写真\{}\項目{index}.jpg", "長いフォルダ名\\".repeat(12)),
                before: "未登録".repeat(15),
                after: "長いコレクション名".repeat(20),
                result: ChangeRowResult::Done(ChangeRowOutcome::Text(format!(
                    "変更しました {index}"
                ))),
            })
            .collect(),
        notice: String::new(),
    });
    state.changes = controller.records().lock().unwrap().entries().to_vec();
    (state, id)
}

#[test]
fn agent_changes_max_rows_long_text_small_200_percent_reaches_last_copy() {
    use crate::test_ui::scroll_layout::{HeightExpectation, ScrollLayout};
    for scale in [1.0, 2.0] {
        let (state, id) = change_fixture(500);
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(1093.0, 614.0))
            .with_context_setup(move |ctx| {
                crate::ui_fonts::configure_fonts(ctx);
                crate::settings::apply_ui_scale_factor(ctx, scale);
            })
            .build_ui_state(
                |root, state| {
                    root.spacing_mut().scroll = panel_scroll_style();
                    let output = egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(root, |ui| {
                            egui::collapsing_header::CollapsingState::load_with_default_open(
                                ui.ctx(),
                                ui.make_persistent_id(("agent_change", id)),
                                true,
                            )
                            .store(ui.ctx());
                            draw_changes(ui, state);
                        });
                    root.ctx().data_mut(|data| {
                        data.insert_temp(
                            egui::Id::new("changes_scroll"),
                            ScrollLayout::from_output(&output),
                        )
                    });
                },
                state,
            );
        harness.run_steps(8);
        let read = |harness: &egui_kittest::Harness<'_, AgentIntegrationState>| {
            harness.ctx.data(|data| {
                data.get_temp::<ScrollLayout>(egui::Id::new("changes_scroll"))
                    .unwrap()
            })
        };
        let before = read(&harness);
        before.assert_height(
            HeightExpectation::Scrolls,
            harness.ctx.content_rect(),
            harness.ctx.pixels_per_point(),
        );
        assert!(
            before.content.x <= before.inner.width() + 1.0,
            "long text widened the page scale={scale}: {before:?}"
        );
        harness.event(egui::Event::PointerMoved(before.inner.center()));
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -1_000_000.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
        });
        harness.run_steps(40);
        read(&harness).assert_scrolled_to_end(before, harness.ctx.pixels_per_point());
        let viewport = harness.ctx.content_rect();
        let copies: Vec<_> = harness.get_all_by_label("コピー").collect();
        let last = copies.last().expect("500th row copy button");
        assert!(
            viewport.contains_rect(last.rect()),
            "last row copy not reachable scale={scale}"
        );
        last.click();
        harness.run_steps(1);
        assert!(harness.output().platform_output.commands.iter().any(|command| {
            matches!(command, egui::OutputCommand::CopyText(text) if text.contains("項目499.jpg") && text.contains("前の値:") && text.contains("後の値:") && text.contains("結果: 変更しました 499"))
        }), "last row copy must contain its item, before, after and result");
    }
}

#[test]
fn agent_changes_copy_text_preserves_all_rows_and_results() {
    let (state, _) = change_fixture(2);
    let record = &state.changes[0];
    let copied = change_record_text(record);
    assert!(copied.contains("add_to_collection / 2 件"));
    for row in &record.rows {
        assert!(copied.contains(&change_row_text(row)));
    }
}

#[test]
fn agent_change_row_presentation_is_derived_from_typed_completion_state() {
    use crate::agent_ipc::changes::{ChangeRow, ChangeRowOutcome, ChangeRowResult};
    let mut row = ChangeRow {
        item: "page.png".into(),
        before: String::new(),
        after: String::new(),
        result: ChangeRowResult::Pending,
    };
    assert_eq!(
        change_row_text(&row),
        "項目: page.png\n前の値: 処理中\n後の値: 処理中\n結果: 処理中"
    );
    row.before = "#old".into();
    row.after = "#new".into();
    row.result = ChangeRowResult::Done(ChangeRowOutcome::Changed);
    assert_eq!(
        change_row_text(&row),
        "項目: page.png\n前の値: #old\n後の値: #new\n結果: 変更"
    );
    row.result = ChangeRowResult::Done(ChangeRowOutcome::Unchanged);
    assert!(change_row_text(&row).ends_with("結果: 変更なし"));
    row.result = ChangeRowResult::Failed("保存エラー".into());
    assert!(change_row_text(&row).ends_with("結果: 失敗: 保存エラー"));
    row.result = ChangeRowResult::Done(ChangeRowOutcome::Text("変更しました".into()));
    assert!(change_row_text(&row).ends_with("結果: 変更しました"));
}

#[test]
fn agent_changes_same_name_collections_show_and_copy_distinct_targets() {
    use crate::agent_ipc::changes::{ChangeController, CollectionTarget};
    let (mut state, _) = change_fixture(1);
    let controller = ChangeController::new();
    for id in ["collection-a", "collection-b"] {
        let mut record = (*state.changes[0]).clone();
        record.client_name = "fixture".into();
        record.summary = "追加".into();
        record.collection = Some(CollectionTarget {
            id: id.into(),
            name: "同じ名前".into(),
        });
        record.rows[0] = crate::agent_ipc::changes::ChangeRow {
            item: r"C:\写真\海.jpg".into(),
            before: "未登録".into(),
            after: "登録済み".into(),
            result: crate::agent_ipc::changes::ChangeRowResult::Done(
                crate::agent_ipc::changes::ChangeRowOutcome::Text("変更しました".into()),
            ),
        };
        controller.publish(record);
    }
    state.changes = controller.records().lock().unwrap().entries().to_vec();
    let mut harness = crate::test_ui::builder()
        .with_size(egui::vec2(900.0, 1000.0))
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .build_ui_state(
            |ui, state| {
                for record in &state.changes {
                    egui::collapsing_header::CollapsingState::load_with_default_open(
                        ui.ctx(),
                        ui.make_persistent_id(("agent_change", record.id)),
                        true,
                    )
                    .store(ui.ctx());
                }
                draw_changes(ui, state);
            },
            state,
        );
    harness.run_steps(5);
    for (index, id) in ["collection-b", "collection-a"].into_iter().enumerate() {
        let target = format!("コレクション: 同じ名前\nコレクション ID: {id}");
        assert_eq!(harness.get_all_by_label(&target).count(), 1);
        let record = &harness.state().changes[index];
        assert!(change_record_text(record).contains(&target));
        assert!(change_row_copy_text(record, &record.rows[0]).contains(&target));
        for button in ["この変更をコピー", "コピー"] {
            harness.get_all_by_label(button).nth(index).unwrap().click();
            harness.run_steps(1);
            assert!(
                harness
                    .output()
                    .platform_output
                    .commands
                    .iter()
                    .any(|command| {
                        matches!(command, egui::OutputCommand::CopyText(text)
                    if text.contains(&target) && text.contains(r"C:\写真\海.jpg"))
                    }),
                "{button} must retain the target id and name: {id}"
            );
        }
    }
}

#[test]
fn agent_integration_is_nonmodal_until_registration_and_preserves_preferences_permissions() {
    let mut app = crate::app::setup_app_for_test();
    let mut edited = app.settings.clone();
    app.open_agent_integration_dialog();
    app.agent_integration_dialog = Some(injected(&app.settings));
    assert!(!app.agent_registration_dialog_open());
    app.settings.agent_level = AgentLevel::Organize;
    app.settings.agent_show_thumbnails = true;
    app.settings.agent_show_gps = true;
    crate::ui_dialogs::preferences::prepare_preferences_settings_for_commit(
        &mut edited,
        &mut app.app.settings,
    );
    assert_eq!(edited.agent_level, AgentLevel::Organize);
    assert!(edited.agent_show_thumbnails && edited.agent_show_gps);
    app.agent_integration_dialog.as_mut().unwrap().confirm(
        AppTarget::ClaudeDesktop,
        Action::Register,
        0,
    );
    assert!(app.agent_registration_dialog_open());
    assert!(app.common_modal_dialog_open());
}

#[test]
fn agent_integration_immediate_permissions_commit_snapshot_and_epoch() {
    let mut app = crate::app::setup_app_for_test();
    assert!(app.settings.save_checked());
    let read = || crate::settings_db::with_db_result(|db| db.load_agent_permission()).unwrap();
    let mut previous = read();
    for selection in [
        PermissionSelection {
            level: AgentLevel::Organize,
            thumbnails: false,
            gps: false,
        },
        PermissionSelection {
            level: AgentLevel::Organize,
            thumbnails: true,
            gps: false,
        },
        PermissionSelection {
            level: AgentLevel::Organize,
            thumbnails: true,
            gps: true,
        },
        PermissionSelection {
            level: AgentLevel::ReadOnly,
            thumbnails: true,
            gps: true,
        },
    ] {
        app.commit_agent_permissions(selection);
        assert_eq!(PermissionSelection::from(&app.settings), selection);
        let current = read();
        assert_eq!(current.0.level, selection.level);
        assert_eq!(current.0.thumbnails, selection.thumbnails);
        assert_eq!(current.0.gps, selection.gps);
        assert_ne!(current.1, previous.1);
        previous = current;
    }
}

#[test]
fn agent_integration_checkbox_save_failure_restores_live_ui_and_permission_epoch() {
    let mut harness = app_harness();
    assert!(harness.state().settings.save_checked());
    let read = || crate::settings_db::with_db_result(|db| db.load_agent_permission()).unwrap();
    let before = read();
    harness.run_steps(5);
    // AppTestEnv owns the test-global data-dir lock and resets suppression on drop.
    crate::settings_db::set_save_suppressed(true);
    harness.get_by_label("縮小画像を AI に見せる").click();
    harness.run_steps(3);
    assert!(!harness.state().settings.agent_show_thumbnails);
    // The global accessor fails closed for reads too while suppression is set.
    // Re-enable only the test accessor after proving the failed UI save did not
    // clear suppression; no application save runs between this and the read.
    assert!(crate::settings_db::save_suppressed());
    crate::settings_db::set_save_suppressed(false);
    assert_eq!(read(), before);
    assert!(
        harness
            .state()
            .fs_feedback_toast
            .as_ref()
            .unwrap()
            .0
            .contains("保存済みの値に戻しました")
    );
    assert!(harness.state().agent_integration_dialog.is_some());
}

#[test]
fn agent_integration_checkbox_commits_without_preferences_or_ok() {
    let mut harness = app_harness();
    harness.run_steps(5);
    assert!(!harness.state().show_preferences);
    harness.get_by_label("縮小画像を AI に見せる").click();
    harness.run_steps(3);
    assert!(harness.state().settings.agent_show_thumbnails);
    let saved = crate::settings_db::with_db_result(|db| db.load_agent_permission()).unwrap();
    assert!(saved.0.thumbnails);
    harness
        .get_by_label("撮影位置 (GPS) を AI に見せる")
        .click();
    harness.run_steps(3);
    assert!(harness.state().settings.agent_show_gps);
    let saved = crate::settings_db::with_db_result(|db| db.load_agent_permission()).unwrap();
    assert!(saved.0.gps);
}
