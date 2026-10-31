use super::*;
use egui_kittest::kittest::{NodeT, Queryable};

fn injected(settings: &Settings) -> AgentIntegrationState {
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
    let detection = backend::Detection {
        command: Ok(backend::RegistrationCommand {
            command: r"C:\fixture\mimageviewer-mcp.exe".into(),
            args: vec!["--data-dir".into(), r"C:\fixture\data".into()],
        }),
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
            })
            .collect(),
    };
    let mut state = AgentIntegrationState::new(settings);
    state.detection = DetectionState::Ready(env.clone(), detection.clone());
    state.mock_backend = Some(MockBackend {
        env,
        detection,
        write_result: Ok("登録しました".into()),
    });
    state
}

#[test]
fn agent_exposure_all_twelve_permission_combinations() {
    let mut count = 0;
    for level in [
        AgentLevel::Disabled,
        AgentLevel::ReadOnly,
        AgentLevel::ReadAndOpen,
    ] {
        for thumbnails in [false, true] {
            for gps in [false, true] {
                let result = exposure(PermissionDraft {
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
                    level == AgentLevel::ReadAndOpen
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
    assert_eq!(count, 12);
}

#[test]
fn agent_registration_draft_guard_all_three_fields_and_both_actions() {
    let live = Settings::default();
    for field in 0..3 {
        for action in [Action::Register, Action::Remove] {
            let mut settings = live.clone();
            match field {
                0 => settings.agent_level = AgentLevel::ReadOnly,
                1 => settings.agent_show_thumbnails = true,
                _ => settings.agent_show_gps = true,
            }
            let mut state = injected(&live);
            state.confirm(&settings, AppTarget::ClaudeDesktop, action);
            assert!(state.operation.is_none());
            state.confirm(&live, AppTarget::ClaudeDesktop, action);
            assert!(matches!(
                state.operation,
                Some(RegistrationOperation::Confirming { .. })
            ));
            state.confirm(&live, AppTarget::ChatGpt, action);
            assert!(
                matches!(state.operation, Some(RegistrationOperation::Confirming { ref request, .. }) if request.target == AppTarget::ClaudeDesktop)
            );
        }
    }
}

#[test]
fn agent_registration_mock_completion_and_disconnection_release_single_owner() {
    let settings = Settings::default();
    let ctx = egui::Context::default();
    let mut state = injected(&settings);
    state.confirm(&settings, AppTarget::ClaudeDesktop, Action::Register);
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
    state.confirm(&settings, AppTarget::ClaudeDesktop, Action::Register);
    assert!(state.operation.is_none());
    state.confirm(&settings, AppTarget::ClaudeDesktop, Action::Remove);
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
    app.open_preferences_page(PreferencesPage::AgentIntegration);
    app.ensure_preferences_state();
    let integration = injected(&app.settings);
    app.pref_state.as_mut().unwrap().agent_integration = integration;
    crate::test_ui::builder()
        .with_context_setup(|ctx| {
            crate::ui_fonts::configure_fonts(ctx);
            crate::ime_focus::install_ime_input_policy(ctx);
        })
        .with_size(size)
        .build_ui_state(
            |root, app| {
                app.update_ime_state(root.ctx());
                app.show_preferences_dialog(&root.ctx().clone());
            },
            app,
        )
}

#[test]
fn agent_registration_confirmation_and_writing_block_backdrop_settings_ok_cancel_x() {
    // Extra horizontal space separates the two real windows for backdrop hits.
    // Small-screen layout is tested independently with the actual preferences.
    let mut harness = app_harness_at_size(egui::vec2(1400.0, 900.0));
    harness.run_steps(4);
    let thumbnail = harness
        .get_by_label("縮小画像を AI に見せる")
        .rect()
        .center();
    let ok = harness.get_by_label("  OK  ").rect().center();
    let cancel = harness.get_by_label("キャンセル").rect().center();
    let close = harness.get_by_label("Close window").rect().center();
    let settings = harness.state().settings.clone();
    harness
        .state_mut()
        .pref_state
        .as_mut()
        .unwrap()
        .agent_integration
        .confirm(&settings, AppTarget::ClaudeDesktop, Action::Register);
    harness.run_steps(3);
    // Move the foreground window through its real title drag. Its default
    // position covers some former backdrop button coordinates; clicking those
    // would correctly cancel the foreground operation, not test the backdrop.
    let modal_id = egui::Id::new("agent_registration_confirmation");
    let modal = harness
        .ctx
        .memory(|memory| memory.area_rect(modal_id))
        .unwrap();
    let start = modal.min + egui::vec2(80.0, 12.0);
    let end = start + (egui::pos2(800.0, 80.0) - modal.min);
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
                .pref_state
                .as_mut()
                .unwrap()
                .agent_integration
                .operation = Some(RegistrationOperation::Writing {
                target: AppTarget::ClaudeDesktop,
                rx: receiver.take().unwrap(),
            });
        }
        harness.run_steps(2);
        for label in ["縮小画像を AI に見せる", "  OK  "] {
            assert!(harness.get_by_label(label).accesskit_node().is_disabled());
        }
        for label in ["キャンセル", "Close window"] {
            assert!(
                harness
                    .get_all_by_label(label)
                    .any(|node| node.accesskit_node().is_disabled()
                        && [cancel, close].contains(&node.rect().center()))
            );
        }
        let foreground = harness
            .ctx
            .memory(|memory| memory.area_rect(modal_id))
            .unwrap();
        for position in [thumbnail, ok, cancel, close] {
            assert!(
                !foreground.contains(position),
                "backdrop point {position:?} is covered by foreground {foreground:?}"
            );
            click_at(&mut harness, position);
            assert!(harness.state().show_preferences);
            assert!(harness.state().agent_registration_dialog_open());
            assert!(
                !harness
                    .state()
                    .pref_state
                    .as_ref()
                    .unwrap()
                    .settings
                    .agent_show_thumbnails
            );
        }
        if writing {
            break;
        }
    }
    tx.send(Err("injected failure".into())).unwrap();
    harness.run_steps(4);
    assert!(!harness.state().agent_registration_dialog_open());
    assert!(harness.state().show_preferences);
}

#[test]
fn agent_registration_confirmation_ime_enter_escape_and_cancel_return() {
    for key in [egui::Key::Enter, egui::Key::Escape] {
        let mut harness = app_harness();
        harness.run_steps(3);
        let settings = harness.state().settings.clone();
        harness
            .state_mut()
            .pref_state
            .as_mut()
            .unwrap()
            .agent_integration
            .confirm(&settings, AppTarget::ClaudeDesktop, Action::Register);
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
                .pref_state
                .as_ref()
                .unwrap()
                .agent_integration
                .operation,
            Some(RegistrationOperation::Confirming { .. })
        ));
        harness.event(egui::Event::Ime(egui::ImeEvent::Disabled));
        harness.run_steps(2);
        harness.key_press(egui::Key::Escape);
        harness.run_steps(2);
        assert!(!harness.state().agent_registration_dialog_open());
        assert!(harness.state().show_preferences);
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
        state.confirm(&settings, AppTarget::ClaudeDesktop, Action::Register);
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(1093.0, 614.0))
            .with_context_setup(move |ctx| {
                crate::ui_fonts::configure_fonts(ctx);
                crate::settings::apply_ui_scale_factor(ctx, scale);
            })
            .build_ui_state(
                |root, state| draw_registration_dialog(root.ctx(), state, false, false),
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
fn agent_integration_page_end_reachable_long_error_at_200_percent() {
    use crate::test_ui::scroll_layout::{HeightExpectation, ScrollLayout};
    for scale in [1.0, 2.0] {
        let settings = Settings::default();
        let mut state = super::super::PreferencesState::from_settings(
            &settings,
            crate::external_tool::LaunchTarget::None,
            None,
            crate::ai::trt_worker_lifecycle::TrtWorkerLifecycleOwner::new().snapshot(),
            false,
            0,
            0,
            0,
        );
        state.selected = PreferencesPage::AgentIntegration;
        state.agent_integration = injected(&settings);
        state.agent_integration.detection = DetectionState::Failed(format!(
            "確認に失敗しました: {}",
            "非常に長いエラー文とパス\\".repeat(80)
        ));
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(1093.0, 614.0))
            .with_context_setup(move |ctx| {
                crate::ui_fonts::configure_fonts(ctx);
                crate::settings::apply_ui_scale_factor(ctx, scale);
            })
            .build_ui_state(
                |root, state| {
                    root.spacing_mut().scroll = super::super::pref_panel_scroll_style();
                    let output = egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(root, |ui| super::super::draw_page(ui, state, false));
                    root.ctx().data_mut(|data| {
                        data.insert_temp(
                            egui::Id::new("agent_page_scroll"),
                            ScrollLayout::from_output(&output),
                        )
                    });
                },
                state,
            );
        harness.run_steps(6);
        let read = |harness: &egui_kittest::Harness<'_, super::super::PreferencesState>| {
            harness.ctx.data(|data| {
                data.get_temp::<ScrollLayout>(egui::Id::new("agent_page_scroll"))
                    .unwrap()
            })
        };
        let before = read(&harness);
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
        read(&harness).assert_scrolled_to_end(before, harness.ctx.pixels_per_point());
        assert!(
            harness
                .ctx
                .content_rect()
                .contains_rect(harness.get_by_label("設定を開く").rect())
        );
    }
}

#[test]
fn agent_integration_real_preferences_small_200_percent_controls_and_page_end() {
    use crate::test_ui::scroll_layout::HeightExpectation;
    for scale in [1.0, 2.0] {
        let mut app = crate::app::setup_app_for_test();
        app.open_preferences_page(PreferencesPage::AgentIntegration);
        app.ensure_preferences_state();
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
        app.pref_state.as_mut().unwrap().agent_integration = integration;
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(1093.0, 614.0))
            .with_context_setup(move |ctx| {
                crate::ui_fonts::configure_fonts(ctx);
                crate::settings::apply_ui_scale_factor(ctx, scale);
            })
            .build_ui_state(
                |root, app| app.show_preferences_dialog(&root.ctx().clone()),
                app,
            );
        harness.run_steps(8);
        let viewport = harness.ctx.content_rect();
        for label in ["  OK  ", "キャンセル", "Close window"] {
            assert!(
                viewport.contains_rect(harness.get_by_label(label).rect()),
                "actual preferences: {label} scale={scale}"
            );
        }
        let before = harness
            .state()
            .pref_state
            .as_ref()
            .unwrap()
            .agent_integration
            .page_scroll
            .unwrap();
        before.assert_height(
            HeightExpectation::Scrolls,
            viewport,
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
            .pref_state
            .as_ref()
            .unwrap()
            .agent_integration
            .page_scroll
            .unwrap()
            .assert_scrolled_to_end(before, harness.ctx.pixels_per_point());
        let mut copies = harness.get_all_by_label("コピー");
        assert!(
            copies.any(|node| viewport.contains_rect(node.rect())),
            "manual instructions must be reachable scale={scale}"
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
