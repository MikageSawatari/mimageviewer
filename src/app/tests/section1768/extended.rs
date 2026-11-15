use super::*;

enum Surface {
    OpenFolder,
    FolderAddress,
    Field,
    NestedPopup,
}

struct MenuFixture {
    env: crate::app::AppTestEnvForTest,
    ctx: egui::Context,
    surface: Surface,
    id: egui::Id,
    text: String,
    time: f64,
    retry: bool,
    passes: usize,
    escape_after: bool,
    raw_permit_before: bool,
    parent_open: bool,
    escape_allowed_after: bool,
}

impl MenuFixture {
    fn new(surface: Surface) -> Self {
        let mut env = crate::app::setup_app_for_test();
        env.show_open_folder_dialog = matches!(surface, Surface::OpenFolder);
        env.open_folder_input = "keep this path".into();
        env.settings.toolbar_section_order = vec![crate::settings::ToolbarSectionId::Folder];
        env.settings.folder_tree_pane_visible = matches!(surface, Surface::FolderAddress);
        let ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);
        crate::ui_fonts::configure_fonts(&ctx);
        let id = if matches!(surface, Surface::FolderAddress) {
            egui::Id::unique("folder_address_input")
        } else {
            egui::Id::unique("section1768_edit")
        };
        let mut fixture = Self {
            env,
            ctx,
            surface,
            id,
            text: "keep this draft".into(),
            time: 0.0,
            retry: false,
            passes: 0,
            escape_after: false,
            raw_permit_before: false,
            parent_open: true,
            escape_allowed_after: true,
        };
        for _ in 0..3 {
            fixture.frame(vec![]);
        }
        if matches!(fixture.surface, Surface::OpenFolder) {
            fixture.id = fixture.ctx.memory(|memory| memory.focused()).unwrap();
        }
        fixture
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.frame_in_viewport(egui::ViewportId::ROOT, events);
    }

    fn frame_in_viewport(&mut self, viewport: egui::ViewportId, events: Vec<Event>) {
        self.time += 0.05;
        let mut passes = 0;
        let mut input = egui::RawInput {
            viewport_id: viewport,
            time: Some(self.time),
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(1100.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        if viewport != egui::ViewportId::ROOT {
            input.viewports.insert(
                viewport,
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    focused: Some(true),
                    ..Default::default()
                },
            );
        }
        self.ctx
            .run_ui(input, |root| {
                passes += 1;
                let ctx = root.ctx().clone();
                let app = &mut self.env.app;
                app.update_ime_state(&ctx);
                let owner = app.keyboard_owner_for_pass(&ctx);
                self.raw_permit_before = owner.fullscreen_raw_key_permit().is_some();
                match self.surface {
                    Surface::OpenFolder => {
                        assert!(app.show_open_folder_dialog_window(&ctx).is_none());
                    }
                    Surface::FolderAddress => {
                        // Production ordering: the folder-pane fixed Esc caller is
                        // downstream of handle_keyboard's existing owner gates.
                        assert!(app.handle_keyboard(&ctx).is_none());
                        app.render_toolbar(root);
                    }
                    Surface::Field => {
                        let mut output =
                            crate::ime_focus::show_singleline(root, &mut self.text, None, |edit| {
                                edit.id(self.id).desired_width(300.0)
                            });
                        crate::ui_helpers::singleline_text_edit_context_menu(
                            root,
                            &mut output,
                            &mut self.text,
                        );
                    }
                    Surface::NestedPopup => {
                        egui::Popup::new(
                            egui::Id::unique("section1768_parent_popup"),
                            ctx.clone(),
                            Pos2::new(80.0, 60.0),
                            root.layer_id(),
                        )
                        .kind(egui::PopupKind::Menu)
                        .open_bool(&mut self.parent_open)
                        .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
                        .show(|ui| {
                            let mut output = crate::ime_focus::show_singleline(
                                ui,
                                &mut self.text,
                                None,
                                |edit| edit.id(self.id).desired_width(300.0),
                            );
                            crate::ui_helpers::singleline_text_edit_context_menu(
                                ui,
                                &mut output,
                                &mut self.text,
                            );
                        });
                    }
                }
                self.escape_after = ctx.input(|input| input.key_pressed(Key::Escape));
                self.escape_allowed_after = crate::keyboard_input::app_escape_allowed(&ctx);
                if self.retry && passes == 1 {
                    ctx.request_discard("section1768 menu Escape retry");
                }
            })
            .drop_without_applying_deltas();
        self.passes = passes;
    }

    fn menu_open(&self) -> bool {
        self.ctx
            .read_response(self.id)
            .is_some_and(|response| response.context_menu_opened())
    }

    fn press_menu(&mut self) -> Pos2 {
        let pos = self.ctx.read_response(self.id).unwrap().rect.center();
        self.frame(vec![Event::PointerMoved(pos)]);
        self.frame(vec![pointer(pos, true)]);
        pos
    }

    fn open_menu(&mut self) {
        let pos = self.press_menu();
        self.frame(vec![pointer(pos, false)]);
        self.frame(vec![]);
        assert!(
            self.menu_open(),
            "right-click must open the actual edit menu"
        );
    }
}

#[test]
fn section1768_open_folder_retained_menu_escape_then_parent_escape() {
    let _input = crate::key_input::lock_test_input();
    let mut fixture = MenuFixture::new(Surface::OpenFolder);
    fixture.open_menu();
    fixture.frame(vec![key(Key::Escape, true, Modifiers::NONE)]);
    assert!(!fixture.menu_open());
    assert!(fixture.env.show_open_folder_dialog);
    assert_eq!(fixture.env.open_folder_input, "keep this path");
    assert!(!fixture.escape_after);
    fixture.frame(vec![key(Key::Escape, false, Modifiers::NONE)]);
    fixture.frame(vec![key(Key::Escape, true, Modifiers::NONE)]);
    assert!(!fixture.env.show_open_folder_dialog);
    assert!(fixture.env.open_folder_input.is_empty());
}

#[test]
fn section1768_open_folder_fresh_menu_and_escape_close_parent_without_orphan() {
    let _input = crate::key_input::lock_test_input();
    let mut fixture = MenuFixture::new(Surface::OpenFolder);
    let pos = fixture.press_menu();
    fixture.frame(vec![
        pointer(pos, false),
        key(Key::Escape, true, Modifiers::NONE),
    ]);
    assert!(!fixture.env.show_open_folder_dialog);
    assert!(!fixture.menu_open());
    assert!(!egui::Popup::is_any_open(&fixture.ctx));
}

#[test]
fn section1768_search_fresh_menu_and_escape_close_parent_without_orphan() {
    let _input = crate::key_input::lock_test_input();
    for pinned in [false, true] {
        let mut fixture = SearchEscapeFixture::new(pinned);
        let pos = fixture
            .ctx
            .read_response(fixture.id())
            .unwrap()
            .rect
            .center();
        fixture.frame(vec![Event::PointerMoved(pos)]);
        fixture.frame(vec![pointer(pos, true)]);
        fixture.frame(vec![
            pointer(pos, false),
            key(Key::Escape, true, Modifiers::NONE),
        ]);
        assert!(!fixture.env.global_search.active, "pinned={pinned}");
        assert!(!fixture.menu_open(), "pinned={pinned}");
        assert!(!egui::Popup::is_any_open(&fixture.ctx));
    }
}

#[test]
fn section1768_search_second_new_escape_reaches_parent() {
    let _input = crate::key_input::lock_test_input();
    for pinned in [false, true] {
        let mut fixture = SearchEscapeFixture::new(pinned);
        fixture.open_menu();
        fixture.escape();
        assert!(fixture.env.global_search.active);
        fixture.frame(vec![key(Key::Escape, false, Modifiers::NONE)]);
        fixture.escape();
        assert!(!fixture.env.global_search.active, "pinned={pinned}");
    }
}

#[cfg(windows)]
fn native_edge(
    viewport: egui::ViewportId,
    virtual_key: u32,
    modifiers: Modifiers,
) -> crate::key_input::KeyEdge {
    crate::key_input::KeyEdge {
        operation_id: crate::key_input::KeyOperationId::unassigned(),
        source_hwnd: 0,
        source_viewport: viewport,
        virtual_key,
        scan_code: 0,
        extended: false,
        pressed: true,
        repeat: false,
        ctrl: modifiers.ctrl,
        shift: modifiers.shift,
        alt: modifiers.alt,
    }
}

#[test]
#[cfg(windows)]
fn section1768_menu_claims_native_twin_native_only_modifiers_and_retry() {
    let _input = crate::key_input::lock_test_input();
    let root = egui::ViewportId::ROOT;
    let sibling = egui::ViewportId::from_hash_of("section1768_sibling");
    for (egui_twin, modifiers, retry) in [
        (true, Modifiers::NONE, false),
        (false, Modifiers::NONE, false),
        (
            true,
            Modifiers::CTRL | Modifiers::SHIFT | Modifiers::ALT,
            false,
        ),
        (false, Modifiers::SHIFT, true),
    ] {
        crate::key_input::clear_test_frame();
        let mut fixture = MenuFixture::new(Surface::Field);
        fixture.open_menu();
        let escape = native_edge(root, 0x1B, modifiers);
        let mut repeat = escape;
        repeat.repeat = true;
        let mut release = escape;
        release.pressed = false;
        crate::key_input::set_test_routed_frame(vec![
            escape,
            repeat,
            release,
            native_edge(root, 0x78, Modifiers::NONE),
            native_edge(sibling, 0x1B, modifiers),
        ]);
        fixture.retry = retry;
        fixture.frame(if egui_twin {
            vec![key(Key::Escape, true, modifiers)]
        } else {
            vec![]
        });
        assert!(
            !fixture.menu_open(),
            "twin={egui_twin}, modifiers={modifiers:?}"
        );
        assert!(!fixture.escape_after);
        assert_eq!(fixture.text, "keep this draft");
        assert!(!crate::key_input::pressed_key_down(root, |edge| edge
            .virtual_key
            == 0x1B));
        assert_eq!(
            crate::key_input::consume_key_edges(root, |edge| edge.virtual_key == 0x1B),
            (false, false)
        );
        assert!(crate::key_input::pressed_key_down(root, |edge| edge
            .virtual_key
            == 0x78));
        assert!(crate::key_input::pressed_key_down(sibling, |edge| edge
            .virtual_key
            == 0x1B));
        if retry {
            assert!(
                fixture.passes >= 2,
                "the discard must actually rerun layout"
            );
        }
        // A later delivery is not suppressed by the previous menu claim.
        crate::key_input::set_test_frame(vec![native_edge(root, 0x1B, Modifiers::NONE)]);
        fixture.retry = false;
        fixture.frame(vec![]);
        assert!(crate::key_input::pressed_key_down(root, |edge| edge
            .virtual_key
            == 0x1B));
    }
}

#[test]
#[cfg(windows)]
#[expect(
    deprecated,
    reason = "Exercise platform IME compatibility notifications"
)]
fn section1768_composing_menu_preserves_native_escape_and_parent() {
    let _input = crate::key_input::lock_test_input();
    let mut fixture = MenuFixture::new(Surface::OpenFolder);
    fixture.frame(vec![
        Event::Ime(egui::ImeEvent::Enabled),
        Event::Ime(egui::ImeEvent::Preedit {
            text: "変換中".into(),
            active_range_chars: None,
        }),
    ]);
    assert!(crate::ime_focus::ime_input_active(&fixture.ctx));
    fixture.open_menu();
    crate::key_input::set_test_frame(vec![native_edge(
        egui::ViewportId::ROOT,
        0x1B,
        Modifiers::NONE,
    )]);
    fixture.frame(vec![]);
    assert!(fixture.menu_open(), "IME owns native-only Escape");
    assert!(fixture.env.show_open_folder_dialog);
    assert!(crate::key_input::pressed_key_down(
        egui::ViewportId::ROOT,
        |edge| edge.virtual_key == 0x1B
    ));
    fixture.frame(vec![key(Key::Escape, true, Modifiers::NONE)]);
    assert!(fixture.env.show_open_folder_dialog);
    assert!(
        fixture.escape_after,
        "menu helper must not claim IME's egui Escape"
    );
    assert!(crate::key_input::pressed_key_down(
        egui::ViewportId::ROOT,
        |edge| edge.virtual_key == 0x1B
    ));
}

#[test]
fn section1768_folder_address_menu_owner_blocks_folder_pane_fixed_escape() {
    let _input = crate::key_input::lock_test_input();
    for (native, egui_twin) in [
        (false, true),
        #[cfg(windows)]
        (true, true),
        #[cfg(windows)]
        (true, false),
    ] {
        crate::key_input::clear_test_frame();
        let mut fixture = MenuFixture::new(Surface::FolderAddress);
        fixture.open_menu();
        assert!(!fixture.env.folder_pane.has_focus);
        #[cfg(windows)]
        if native {
            crate::key_input::set_test_frame(vec![native_edge(
                egui::ViewportId::ROOT,
                0x1B,
                Modifiers::NONE,
            )]);
        }
        fixture.frame(if egui_twin {
            vec![key(Key::Escape, true, Modifiers::NONE)]
        } else {
            vec![]
        });
        assert!(
            !fixture.env.folder_pane.has_focus,
            "retained address menu Escape must not reach folder-pane fixed Esc caller: native={native}, egui={egui_twin}"
        );
        assert!(!fixture.menu_open(), "native={native}, egui={egui_twin}");
        assert!(!fixture.escape_after);
        #[cfg(windows)]
        assert!(!crate::key_input::pressed_key_down(
            egui::ViewportId::ROOT,
            |edge| edge.virtual_key == 0x1B
        ));
    }
}

#[test]
fn section1768_nested_edit_menu_escape_keeps_parent_egui_popup() {
    let _input = crate::key_input::lock_test_input();
    let mut fixture = MenuFixture::new(Surface::NestedPopup);
    fixture.open_menu();
    assert!(fixture.parent_open);
    fixture.frame(vec![key(Key::Escape, true, Modifiers::NONE)]);
    assert!(!fixture.menu_open());
    assert!(
        fixture.parent_open,
        "child menu must claim Escape before parent Popup::show reads it"
    );
    assert!(!fixture.escape_after);
    assert_eq!(fixture.text, "keep this draft");
    // The parent keeps its ordinary close behavior for the next new press.
    fixture.frame(vec![key(Key::Escape, false, Modifiers::NONE)]);
    fixture.frame(vec![key(Key::Escape, true, Modifiers::NONE)]);
    assert!(!fixture.parent_open);
}

#[test]
fn section1768_root_menu_claim_does_not_block_drawn_sibling_viewport_escape() {
    let _input = crate::key_input::lock_test_input();
    let sibling = egui::ViewportId::from_hash_of("section1768_drawn_sibling");
    let mut fixture = MenuFixture::new(Surface::Field);
    fixture.open_menu();
    // Draw the same helper/widget id in an actual sibling viewport while the
    // root popup is retained. Eligibility belongs to the current viewport.
    fixture.frame_in_viewport(sibling, vec![]);
    assert!(fixture.escape_allowed_after);
    #[cfg(windows)]
    crate::key_input::set_test_routed_frame(vec![
        native_edge(egui::ViewportId::ROOT, 0x1B, Modifiers::NONE),
        native_edge(sibling, 0x1B, Modifiers::NONE),
    ]);
    fixture.frame(vec![key(Key::Escape, true, Modifiers::NONE)]);
    assert!(!fixture.menu_open());
    assert!(!fixture.escape_after);
    assert!(!fixture.escape_allowed_after);
    fixture.frame_in_viewport(sibling, vec![key(Key::Escape, true, Modifiers::NONE)]);
    assert!(
        fixture.escape_allowed_after,
        "root popup ownership must not suppress sibling parent Escape"
    );
    assert!(
        fixture.escape_after,
        "root popup claim must leave sibling egui input intact"
    );
    #[cfg(windows)]
    assert!(crate::key_input::pressed_key_down(sibling, |edge| edge
        .virtual_key
        == 0x1B));
}
