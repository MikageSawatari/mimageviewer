use super::*;
use crate::agent_registration as backend;
use crate::ui_dialogs::agent_integration::MockBackend;
use egui_kittest::kittest::Queryable;
use std::{cell::RefCell, rc::Rc, sync::mpsc};

fn mock(states: [RegistrationState; 2]) -> MockBackend {
    MockBackend {
        env: backend::RegistrationEnvironment {
            local_app_data: None,
            app_data: None,
            user_profile: None,
            codex_home: None,
            launcher_exe: None,
            current_exe: r"C:\fixture\core.exe".into(),
            data_dir: r"C:\fixture\data".into(),
            working_dir: r"C:\fixture".into(),
        },
        detection: backend::Detection {
            command: Err("fixture".into()),
            apps: [AppTarget::ClaudeDesktop, AppTarget::ChatGpt]
                .into_iter()
                .zip(states)
                .map(|(target, state)| backend::AppRegistration {
                    target,
                    state,
                    config_paths: vec![],
                    message: None,
                    changes: Default::default(),
                })
                .collect(),
        },
        write_result: Ok("fixture".into()),
        writes: 0,
    }
}

#[test]
fn ask_ai_only_registered_apps_are_offered() {
    for state in [
        RegistrationState::NotFound,
        RegistrationState::Unregistered,
        RegistrationState::DifferentTarget,
        RegistrationState::Unreadable,
        RegistrationState::Ambiguous,
        RegistrationState::Registered,
    ] {
        let backend = mock([state, state]);
        assert_eq!(
            registered_apps(&backend.detection),
            [state == RegistrationState::Registered; 2]
        );
    }
}

#[test]
fn ask_ai_detection_interval_pending_and_failed_results() {
    let ctx = egui::Context::default();
    let now = Instant::now();
    let mut state = AskAiRegistration {
        mock_backend: Some(mock([
            RegistrationState::Registered,
            RegistrationState::DifferentTarget,
        ])),
        ..Default::default()
    };
    state.refresh(&ctx, now);
    assert!(state.pending.is_some());
    assert_eq!(state.latest, None);
    state.refresh(&ctx, now + Duration::from_secs(4));
    assert_eq!(state.latest, Some([true, false]));
    assert!(state.pending.is_none());
    assert_eq!(state.last_started, Some(now));
    state.refresh(&ctx, now + Duration::from_secs(5));
    assert!(state.pending.is_some());
    assert_eq!(state.last_started, Some(now + Duration::from_secs(5)));

    // A delayed worker stays the sole owner even after the interval expires.
    let (tx, rx) = mpsc::channel();
    state.pending = Some(rx);
    state.refresh(&ctx, now + Duration::from_secs(20));
    assert_eq!(state.last_started, Some(now + Duration::from_secs(5)));
    tx.send(Err("fixture failure".into())).unwrap();
    state.refresh(&ctx, now + Duration::from_secs(21));
    assert_eq!(state.latest, Some([false; 2]));
    assert!(
        state.pending.is_some(),
        "expired interval starts a new detection after adopting failure"
    );
    let (tx, rx) = mpsc::channel();
    state.pending = Some(rx);
    drop(tx);
    state.refresh(&ctx, now + Duration::from_secs(22));
    assert_eq!(state.latest, Some([false; 2]));
    assert!(state.pending.is_none());
}

#[derive(Default)]
struct MenuFixture {
    registered: [bool; 2],
    checking: bool,
    responses: Vec<(Destination, egui::Response)>,
    selected: Vec<Destination>,
    registration: Option<AskAiRegistration>,
}

fn harness(fixture: MenuFixture) -> egui_kittest::Harness<'static, MenuFixture> {
    crate::test_ui::builder()
        .with_size(egui::vec2(1000.0, 600.0))
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .build_ui_state(
            |ui, state: &mut MenuFixture| {
                if let Some(registration) = &mut state.registration {
                    registration.refresh(ui.ctx(), Instant::now());
                    state.registered = registration.latest.unwrap_or([false; 2]);
                    state.checking =
                        registration.latest.is_none() && registration.pending.is_some();
                }
                state.responses.clear();
                if let Some(destination) = draw_contents(
                    ui,
                    state.registered,
                    state.checking,
                    |destination, response| state.responses.push((destination, response.clone())),
                ) {
                    state.selected.push(destination);
                }
            },
            fixture,
        )
}

#[test]
fn ask_ai_submenu_order_for_unregistered_claude_both_and_checking() {
    for (registered, checking, apps) in [
        ([false; 2], false, vec![]),
        ([true, false], false, vec![Destination::ClaudeApp]),
        (
            [true; 2],
            false,
            vec![Destination::ClaudeApp, Destination::ChatGptApp],
        ),
        ([false; 2], true, vec![]),
    ] {
        let mut harness = harness(MenuFixture {
            registered,
            checking,
            ..Default::default()
        });
        harness.run();
        // U1: registered apps replace the Web and getting-started sections.
        let web_and_guide = if apps.is_empty() {
            vec![
                Destination::Google,
                Destination::ChatGptWeb,
                Destination::ClaudeWeb,
                Destination::Guide,
            ]
        } else {
            Vec::new()
        };
        let expected: Vec<_> = apps.into_iter().chain(web_and_guide).collect();
        let actual: Vec<_> = harness
            .state()
            .responses
            .iter()
            .map(|(destination, _)| *destination)
            .collect();
        assert_eq!(actual, expected);
        let mut bottom = f32::NEG_INFINITY;
        for destination in expected {
            let rect = harness.get_by_label(destination.label()).rect();
            assert!(rect.top() >= bottom);
            bottom = rect.bottom();
        }
        assert_eq!(
            harness.query_by_label("AI アプリの登録を確認中…").is_some(),
            checking
        );
    }
}

#[test]
fn ask_ai_destination_ids_survive_section_insertion_and_removal() {
    let mut harness = harness(MenuFixture {
        registered: [true; 2],
        ..Default::default()
    });
    harness.run();
    let mut baseline: Vec<_> = harness
        .state()
        .responses
        .iter()
        .map(|(dest, response)| (*dest, response.id))
        .collect();
    // Web and guide items appear only while nothing is registered (U1); take
    // their ids from the unregistered and checking layouts, which differ by
    // the "checking" row above them.
    for checking in [false, true] {
        harness.state_mut().registered = [false; 2];
        harness.state_mut().checking = checking;
        harness.run();
        for (destination, response) in &harness.state().responses {
            if let Some((_, id)) = baseline.iter().find(|(dest, _)| dest == destination) {
                assert_eq!(response.id, *id);
            } else {
                baseline.push((*destination, response.id));
            }
        }
    }
    assert_eq!(baseline.len(), 6);
    for (registered, checking) in [
        ([false, true], false),
        ([true, false], false),
        ([false; 2], false),
        ([false; 2], true),
        ([true; 2], false),
    ] {
        harness.state_mut().registered = registered;
        harness.state_mut().checking = checking;
        harness.run();
        for (destination, id) in &baseline {
            if let Some((_, response)) = harness
                .state()
                .responses
                .iter()
                .find(|(dest, _)| dest == destination)
            {
                assert_eq!(response.id, *id);
            }
        }
    }
}

#[test]
fn ask_ai_pressed_destination_never_becomes_another_destination() {
    for (before, after) in [([false; 2], [true; 2]), ([true; 2], [false; 2])] {
        for destination in [
            Destination::ClaudeApp,
            Destination::ChatGptApp,
            Destination::Google,
            Destination::ChatGptWeb,
            Destination::ClaudeWeb,
            Destination::Guide,
        ] {
            if before == [false; 2]
                && matches!(
                    destination,
                    Destination::ClaudeApp | Destination::ChatGptApp
                )
            {
                continue;
            }
            let (tx, rx) = mpsc::channel();
            let mut harness = harness(MenuFixture {
                registration: Some(AskAiRegistration {
                    latest: Some(before),
                    pending: Some(rx),
                    last_started: Some(Instant::now()),
                    ..Default::default()
                }),
                ..Default::default()
            });
            harness.run();
            let pos = harness.get_by_label(destination.label()).rect().center();
            harness.hover_at(pos);
            harness.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            });
            harness.step();
            let backend = mock(after.map(|registered| {
                if registered {
                    RegistrationState::Registered
                } else {
                    RegistrationState::Unregistered
                }
            }));
            tx.send(Ok((backend.env, backend.detection))).unwrap();
            harness.step();
            harness.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            });
            harness.step();
            harness.run();
            assert!(
                harness
                    .state()
                    .selected
                    .iter()
                    .all(|selected| *selected == destination)
            );
        }
    }
}

#[test]
fn ask_ai_menu_command_respects_hidden_setting_and_opens_submenu() {
    for hidden in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        app.ask_ai_registration.mock_backend = Some(mock([RegistrationState::Unregistered; 2]));
        if hidden {
            app.settings
                .menu_layout
                .hidden_commands
                .push("HelpAskAi".into());
        }
        let app = Rc::new(RefCell::new(app));
        let render = Rc::clone(&app);
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(1093.0, 614.0))
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .build_ui(move |ui| {
                render.borrow_mut().render_menubar(ui);
            });
        harness.run();
        assert!(app.borrow().ask_ai_registration.last_started.is_none());
        harness.get_by_label("ヘルプ").click();
        harness.run();
        assert_eq!(harness.query_by_label("AI で調べる").is_none(), hidden);
        if !hidden {
            assert!(
                harness.get_by_label("AI で調べる").rect().top()
                    >= harness.get_by_label("ヘルプサイトを開く").rect().bottom()
            );
            harness.get_by_label("AI で調べる").click();
            harness.run();
            assert!(
                harness
                    .query_by_label(Destination::Google.label())
                    .is_some()
            );
            assert!(app.borrow().ask_ai_registration.latest.is_some());
        }
    }
}

#[test]
fn ask_ai_app_click_submits_once_to_worker_without_waiting() {
    use crate::external_links::TestOpenerGuard;
    use std::sync::Mutex;
    let mut app = crate::app::setup_app_for_test();
    app.ask_ai_registration.latest = Some([true; 2]);
    app.ask_ai_registration.last_started = Some(Instant::now());
    let (called_tx, called_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let caller = std::thread::current().id();
    let _opener = TestOpenerGuard::install(move |target| {
        assert_ne!(std::thread::current().id(), caller);
        assert!(
            matches!(target, OpenTarget::AiApp(AiAppLink::ClaudeNew { prompt }) if prompt == APP_PROMPT)
        );
        called_tx.send(()).unwrap();
        release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        Ok(())
    });
    let mut harness = crate::test_ui::builder()
        .with_size(egui::vec2(1000.0, 600.0))
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .build_ui_state(|ui, app| app.draw_ask_ai_menu(ui), app);
    harness.run();
    harness.get_by_label(Destination::ClaudeApp.label()).click();
    harness.run();
    called_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(called_rx.try_recv().is_err());
    release_tx.send(()).unwrap();
}
