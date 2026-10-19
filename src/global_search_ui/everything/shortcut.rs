//! Pass-owned main-grid routing for Everything search. No transport is started here.
use crate::app::App;
use crate::keyboard_input::ShortcutSurface;
use crate::keymap::KeyAction;

impl App {
    /// Only root-owned application input may explain a refusal. Modal and text
    /// owners keep their keys, including a key-capture dialog's custom binding.
    pub(crate) fn notify_everything_search_shortcut_refusal(
        &mut self,
        ctx: &egui::Context,
    ) -> bool {
        if ctx.viewport_id() != egui::ViewportId::ROOT {
            return false;
        }
        let entry = self.everything_search_entry_availability();
        if entry == super::EverythingSearchEntryAvailability::ViewerContext {
            return false;
        }
        let Some(reason) = entry.reason_text() else {
            return false;
        };
        let Some(permit) = self.keyboard_owner_for_pass(ctx).shortcut_permit() else {
            return false;
        };
        if permit.scope().viewport != ctx.viewport_id()
            || permit.scope().surface != ShortcutSurface::Main
            || self.any_dialog_open()
            || !self
                .keymap
                .consume_action_no_repeat(ctx, KeyAction::GlobalEverythingSearch)
        {
            return false;
        }
        let message = format!("Everything ファイル名検索: {reason}");
        let duration = crate::ui_fullscreen::feedback_toast_duration(&message);
        self.show_feedback_toast_on_with_duration(
            message,
            crate::app::ActionSurface::MainWindow,
            duration,
        );
        true
    }

    pub(crate) fn dispatch_everything_search_shortcut(&mut self, ctx: &egui::Context) -> bool {
        if self.notify_everything_search_shortcut_refusal(ctx) {
            return true;
        }
        if ctx.viewport_id() != egui::ViewportId::ROOT || !self.everything_search_entry_allowed() {
            return false;
        }
        let Some(permit) = self.keyboard_owner_for_pass(ctx).shortcut_permit() else {
            return false;
        };
        if permit.scope().viewport != ctx.viewport_id()
            || permit.scope().surface != ShortcutSurface::Main
        {
            return false;
        }
        if !self
            .keymap
            .consume_action_no_repeat(ctx, KeyAction::GlobalEverythingSearch)
        {
            return false;
        }
        self.open_everything_search();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyboard_input::{KeyboardOwner, ShortcutScope, TextInputPhase};

    #[cfg(windows)]
    struct ClearKeyFrame;
    #[cfg(windows)]
    impl Drop for ClearKeyFrame {
        fn drop(&mut self) {
            crate::key_input::clear_test_frame();
        }
    }

    fn key_pass(
        ctx: &egui::Context,
        key: egui::Key,
        modifiers: egui::Modifiers,
        owner: Option<KeyboardOwner>,
    ) {
        ctx.begin_pass(egui::RawInput {
            events: std::iter::once(egui::Event::ModifiersChanged(modifiers))
                .chain(vec![egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers,
                }])
                .collect(),
            ..Default::default()
        });
        if let Some(owner) = owner {
            crate::keyboard_input::cache_keyboard_owner(ctx, owner);
        }
    }

    fn main_owner() -> KeyboardOwner {
        KeyboardOwner::ApplicationShortcut {
            scope: ShortcutScope::new(egui::ViewportId::ROOT, ShortcutSurface::Main),
        }
    }

    fn assert_ctrl_r_pending(ctx: &egui::Context, repeated: bool) {
        ctx.input(|input| {
            assert_eq!(
                input.events,
                [
                    egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
                    egui::Event::Key {
                        key: egui::Key::R,
                        physical_key: Some(egui::Key::R),
                        pressed: true,
                        repeat: repeated,
                        modifiers: egui::Modifiers::CTRL,
                    }
                ]
            );
        });
    }

    #[test]
    #[expect(
        deprecated,
        reason = "Exercise retained IME compatibility notifications and cancellation ordering"
    )]
    fn everything_ctrl_r_reopen_selects_previous_query_and_typing_replaces_it() {
        #[cfg(windows)]
        let _serial = crate::key_input::lock_test_input();
        #[cfg(windows)]
        let _clear = ClearKeyFrame;
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);

        fn draw(ctx: &egui::Context, app: &mut App) -> egui::Id {
            ctx.root_ui(|root_ui| {
                egui::CentralPanel::default().show(root_ui, |ui| {
                    super::super::ui::draw_everything_controls(
                        ui,
                        &mut app.global_search.query,
                        &mut app.global_search.focus_request,
                    );
                })
            });
            ctx.memory(|memory| memory.focused()).unwrap()
        }

        app.open_everything_search();
        for (previous, replacement, ime) in [("mist", "mist", false), ("日本語🍀", "検索", true)]
        {
            app.global_search.query = previous.into();
            ctx.begin_pass(egui::RawInput::default());
            let id = draw(&ctx, &mut app);
            let mut state = egui::TextEdit::load_state(&ctx, id).unwrap();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(previous.chars().count()),
                )));
            state.store(&ctx, id);
            ctx.memory_mut(|memory| memory.surrender_focus(id));
            let _ = crate::test_ui::headless_paint_output(ctx.end_pass());

            key_pass(
                &ctx,
                egui::Key::R,
                egui::Modifiers::CTRL,
                Some(main_owner()),
            );
            assert!(
                app.dispatch_everything_search_shortcut(&ctx),
                "reopen {previous}"
            );
            let id = draw(&ctx, &mut app);
            let selection = egui::TextEdit::load_state(&ctx, id)
                .unwrap()
                .cursor
                .char_range()
                .unwrap()
                .as_sorted_char_range();
            assert_eq!(
                selection,
                egui::text::CharIndex(0)..egui::text::CharIndex(previous.chars().count())
            );
            assert!(!app.global_search.focus_request);
            assert!(
                app.global_search.pending.is_none(),
                "reopen must not send IPC"
            );
            let _ = crate::test_ui::headless_paint_output(ctx.end_pass());

            let mut events = if ime {
                vec![
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    egui::Event::Ime(egui::ImeEvent::Preedit {
                        text: replacement.into(),
                        active_range_chars: None,
                    }),
                    egui::Event::Ime(egui::ImeEvent::Commit(replacement.into())),
                    egui::Event::Ime(egui::ImeEvent::Disabled),
                ]
            } else {
                vec![egui::Event::Text(replacement.into())]
            };
            events.insert(
                0,
                egui::Event::Key {
                    key: egui::Key::R,
                    physical_key: Some(egui::Key::R),
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
            );
            ctx.begin_pass(egui::RawInput {
                events,
                ..Default::default()
            });
            draw(&ctx, &mut app);
            assert_eq!(app.global_search.query, replacement);
            let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
            ctx.begin_pass(egui::RawInput {
                events: vec![egui::Event::Text("x".into())],
                ..Default::default()
            });
            draw(&ctx, &mut app);
            assert_eq!(
                app.global_search.query,
                format!("{replacement}x"),
                "ordinary typing must not repeatedly select the query"
            );
            let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
            if ime {
                // Even an explicit caller focus request during composition must
                // preserve the preedit selection. Recovery retries are helper-owned.
                app.global_search.focus_request = true;
                ctx.begin_pass(egui::RawInput {
                    events: vec![
                        egui::Event::Ime(egui::ImeEvent::Enabled),
                        egui::Event::Ime(egui::ImeEvent::Preedit {
                            text: "未".into(),
                            active_range_chars: None,
                        }),
                    ],
                    ..Default::default()
                });
                let id = draw(&ctx, &mut app);
                let selection = egui::TextEdit::load_state(&ctx, id)
                    .unwrap()
                    .cursor
                    .char_range()
                    .unwrap()
                    .as_sorted_char_range();
                let start = replacement.chars().count() + 1;
                assert_eq!(
                    selection,
                    egui::text::CharIndex(start)..egui::text::CharIndex(start + 1)
                );
                let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
                ctx.begin_pass(egui::RawInput {
                    events: vec![
                        egui::Event::Ime(egui::ImeEvent::Commit("確".into())),
                        egui::Event::Ime(egui::ImeEvent::Disabled),
                    ],
                    ..Default::default()
                });
                draw(&ctx, &mut app);
                assert_eq!(app.global_search.query, format!("{replacement}x確"));
                let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
            }
        }
    }

    #[test]
    fn everything_shortcut_dispatch_opens_once_and_honors_rebind_and_none() {
        #[cfg(windows)]
        let _serial = crate::key_input::lock_test_input();
        #[cfg(windows)]
        let _clear = ClearKeyFrame;
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        key_pass(
            &ctx,
            egui::Key::R,
            egui::Modifiers::CTRL,
            Some(main_owner()),
        );
        assert!(app.dispatch_everything_search_shortcut(&ctx));
        assert!(!app.dispatch_everything_search_shortcut(&ctx));
        assert!(app.global_search.active);
        assert_eq!(
            app.global_search.source,
            super::super::SearchSource::Everything
        );
        assert!(
            app.global_search.pending.is_none(),
            "opening must not send IPC"
        );
        assert_eq!(
            ctx.input(|input| input.events.clone()),
            [egui::Event::ModifiersChanged(egui::Modifiers::CTRL)],
            "the shortcut consumes its key while preserving the ordered modifier notification"
        );
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());

        app.keymap = crate::keymap::Keymap::from_ini_str("[Grid]\nGlobalEverythingSearch = F24\n");
        key_pass(
            &ctx,
            egui::Key::R,
            egui::Modifiers::CTRL,
            Some(main_owner()),
        );
        assert!(!app.dispatch_everything_search_shortcut(&ctx));
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
        key_pass(
            &ctx,
            egui::Key::F24,
            egui::Modifiers::NONE,
            Some(main_owner()),
        );
        assert!(app.dispatch_everything_search_shortcut(&ctx));
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
        app.keymap = crate::keymap::Keymap::from_ini_str("[Grid]\nGlobalEverythingSearch = none\n");
        key_pass(
            &ctx,
            egui::Key::R,
            egui::Modifiers::CTRL,
            Some(main_owner()),
        );
        assert!(!app.dispatch_everything_search_shortcut(&ctx));
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
    }

    #[test]
    fn everything_shortcut_dispatch_preserves_text_and_ime_owned_input() {
        #[cfg(windows)]
        let _serial = crate::key_input::lock_test_input();
        #[cfg(windows)]
        let _clear = ClearKeyFrame;
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        // The real App ownership snapshot recognizes a tracked focused field.
        app.address_has_focus = true;
        key_pass(&ctx, egui::Key::R, egui::Modifiers::CTRL, None);
        assert!(!app.dispatch_everything_search_shortcut(&ctx));
        assert_ctrl_r_pending(&ctx, false);
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
        app.address_has_focus = false;
        // IME composition and focus recovery are pass-owned claims; the
        // real dispatcher must retain their edge for the field, not just skip open.
        for phase in [
            TextInputPhase::PendingFocus,
            TextInputPhase::Focused,
            TextInputPhase::FocusRecovery,
            TextInputPhase::ImeComposing,
        ] {
            key_pass(
                &ctx,
                egui::Key::R,
                egui::Modifiers::CTRL,
                Some(KeyboardOwner::TextInput {
                    viewport: egui::ViewportId::ROOT,
                    widget_id: egui::Id::unique("everything-ime-field"),
                    phase,
                }),
            );
            assert!(!app.dispatch_everything_search_shortcut(&ctx), "{phase:?}");
            assert_ctrl_r_pending(&ctx, true);
            assert!(!app.global_search.active);
            let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
        }
    }

    #[test]
    fn everything_shortcut_dispatch_rejects_fullscreen_and_foreign_viewport_permits() {
        #[cfg(windows)]
        let _serial = crate::key_input::lock_test_input();
        #[cfg(windows)]
        let _clear = ClearKeyFrame;
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        for (index, owner) in [
            KeyboardOwner::Modal,
            KeyboardOwner::ApplicationShortcut {
                scope: ShortcutScope::new(egui::ViewportId::ROOT, ShortcutSurface::Fullscreen),
            },
            KeyboardOwner::ApplicationShortcut {
                scope: ShortcutScope::new(
                    egui::ViewportId::from_hash_of("detached"),
                    ShortcutSurface::Main,
                ),
            },
        ]
        .into_iter()
        .enumerate()
        {
            key_pass(&ctx, egui::Key::R, egui::Modifiers::CTRL, Some(owner));
            assert!(!app.dispatch_everything_search_shortcut(&ctx));
            assert_ctrl_r_pending(&ctx, index != 0);
            assert!(!app.global_search.active);
            let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
        }
        app.fullscreen_idx = Some(0);
        let ctx = egui::Context::default();
        key_pass(
            &ctx,
            egui::Key::R,
            egui::Modifiers::CTRL,
            Some(main_owner()),
        );
        assert!(app.dispatch_everything_search_shortcut(&ctx));
        assert!(!app.dispatch_everything_search_shortcut(&ctx));
        assert!(
            app.fs_feedback_toast
                .as_ref()
                .unwrap()
                .0
                .contains("メインウィンドウで画像・動画")
        );
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
    }

    #[cfg(windows)]
    #[test]
    fn everything_shortcut_dispatch_rejects_detached_context_and_native_video() {
        let _serial = crate::key_input::lock_test_input();
        let _clear = ClearKeyFrame;
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let window = app.build_window_context_for_test(1376, |_| {});
        app.with_window_context_for_test(window, |mounted| {
            key_pass(
                &ctx,
                egui::Key::R,
                egui::Modifiers::CTRL,
                Some(main_owner()),
            );
            assert!(!mounted.dispatch_everything_search_shortcut(&ctx));
            assert!(!mounted.global_search.active);
            assert_ctrl_r_pending(&ctx, false);
            let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
        });
        app.items = vec![crate::grid_item::GridItem::Video(
            app.tmp.path().join("fake.mp4"),
        )];
        app.fullscreen_idx = Some(0);
        app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
        let ctx = egui::Context::default();
        key_pass(
            &ctx,
            egui::Key::R,
            egui::Modifiers::CTRL,
            Some(main_owner()),
        );
        assert!(app.dispatch_everything_search_shortcut(&ctx));
        assert!(!app.global_search.active);
        assert!(app.fs_feedback_toast.is_some());
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
    }

    #[test]
    fn everything_entry_reasons_preserve_existing_admission_table() {
        use super::super::EverythingSearchEntryAvailability as Entry;
        let mut app = crate::app::setup_app_for_test();
        let check = |app: &App, expected| {
            let entry = app.everything_search_entry_availability();
            assert_eq!(entry, expected);
            let legacy_grid = app.main_folder_history_available() && app.fullscreen_idx.is_none();
            // The native-backdrop predicate itself requires fullscreen_idx=Some,
            // already excluded here by the former unconditional idx rule.
            let legacy_allowed = legacy_grid
                && !app.common_modal_dialog_open()
                && !app.remote_session_blocks_local_control();
            assert_eq!(entry.is_allowed(), legacy_allowed);
            assert_eq!(app.everything_search_entry_allowed(), legacy_allowed);
            assert_eq!(entry.reason_text().is_none(), legacy_allowed);
        };
        check(&app, Entry::Available);
        app.show_preferences = true;
        check(&app, Entry::OtherOperation);
        app.show_preferences = false;
        // VST manager was not part of Everything's functional admission. Keep
        // that released rule even though the keyboard belongs to a dialog.
        app.show_vst3_manager = true;
        check(&app, Entry::Available);
        app.show_vst3_manager = false;
        app.fullscreen_idx = Some(0);
        app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
        check(&app, Entry::MainViewer);
        app.viewer_presentation = crate::app::ViewerPresentation::MainWindow;
        check(&app, Entry::MainViewer);
        app.viewer_presentation = crate::app::ViewerPresentation::DetachedWindow;
        check(&app, Entry::LinkedViewer);
        app.fullscreen_idx = None;
        #[cfg(windows)]
        {
            let sibling = app.build_window_context_for_test(1386, |_| {});
            app.with_window_context_for_test(sibling, |viewer| check(viewer, Entry::ViewerContext));
            check(&app, Entry::Available);
        }
        let handle = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(handle.clone());
        handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
            client_id: "search-reason-test".into(),
            peer: mimageviewer_ipc::SessionPeerInfo {
                connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                device_name: None,
            },
        });
        check(&app, Entry::RemoteControl);
        assert_eq!(
            Entry::RemoteControl.reason_text(),
            Some("mIV Remote で操作中のため検索できません")
        );
    }

    #[test]
    fn everything_refusal_consumes_one_press_and_preserves_search_and_viewer() {
        #[cfg(windows)]
        let _serial = crate::key_input::lock_test_input();
        #[cfg(windows)]
        let _clear = ClearKeyFrame;
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        app.fullscreen_idx = Some(0);
        app.viewer_presentation = crate::app::ViewerPresentation::DetachedWindow;
        let generation = app.items_generation;
        key_pass(
            &ctx,
            egui::Key::R,
            egui::Modifiers::CTRL,
            Some(main_owner()),
        );
        assert!(app.notify_everything_search_shortcut_refusal(&ctx));
        let first = app.fs_feedback_toast.clone().expect("refusal");
        assert!(
            first
                .0
                .contains("連動した別ウィンドウで閲覧している間は使えません")
        );
        assert_eq!(
            app.fs_feedback_toast_surface,
            Some(crate::app::ActionSurface::MainWindow)
        );
        assert!(!app.dispatch_everything_search_shortcut(&ctx));
        assert_eq!(app.fs_feedback_toast.as_ref().unwrap().1, first.1);
        assert!(!app.global_search.active);
        assert!(app.global_search.pending.is_none());
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.fullscreen_idx, Some(0));
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());

        key_pass(
            &ctx,
            egui::Key::R,
            egui::Modifiers::CTRL,
            Some(main_owner()),
        );
        // egui marks the second held press as repeat.
        assert!(!app.notify_everything_search_shortcut_refusal(&ctx));
        assert_eq!(app.fs_feedback_toast.as_ref().unwrap().1, first.1);
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
    }

    #[test]
    fn everything_refusal_respects_modal_text_foreign_scope_and_rebinding() {
        #[cfg(windows)]
        let _serial = crate::key_input::lock_test_input();
        #[cfg(windows)]
        let _clear = ClearKeyFrame;
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        app.fullscreen_idx = Some(0);
        for owner in [
            KeyboardOwner::Modal,
            KeyboardOwner::FocusedUi {
                viewport: egui::ViewportId::ROOT,
                widget_id: egui::Id::unique("key capture"),
            },
            KeyboardOwner::TextInput {
                viewport: egui::ViewportId::ROOT,
                widget_id: egui::Id::unique("field"),
                phase: TextInputPhase::PendingFocus,
            },
            KeyboardOwner::TextInput {
                viewport: egui::ViewportId::ROOT,
                widget_id: egui::Id::unique("field"),
                phase: TextInputPhase::Focused,
            },
            KeyboardOwner::TextInput {
                viewport: egui::ViewportId::ROOT,
                widget_id: egui::Id::unique("field"),
                phase: TextInputPhase::ImeComposing,
            },
            KeyboardOwner::TextInput {
                viewport: egui::ViewportId::ROOT,
                widget_id: egui::Id::unique("field"),
                phase: TextInputPhase::FocusRecovery,
            },
            KeyboardOwner::ApplicationShortcut {
                scope: ShortcutScope::new(egui::ViewportId::ROOT, ShortcutSurface::Fullscreen),
            },
            KeyboardOwner::ApplicationShortcut {
                scope: ShortcutScope::new(
                    egui::ViewportId::from_hash_of("foreign"),
                    ShortcutSurface::Main,
                ),
            },
        ] {
            key_pass(&ctx, egui::Key::R, egui::Modifiers::CTRL, Some(owner));
            let before = ctx.input(|input| input.events.clone());
            assert!(!app.dispatch_everything_search_shortcut(&ctx), "{owner:?}");
            assert_eq!(ctx.input(|input| input.events.clone()), before);
            assert!(app.fs_feedback_toast.is_none());
            let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
        }
        // Even an older cached Main permit must not bypass a newly shown modal.
        app.show_operation_customize = true;
        app.keymap = crate::keymap::Keymap::from_ini_str("[Grid]\nGlobalEverythingSearch = F24\n");
        key_pass(
            &ctx,
            egui::Key::F24,
            egui::Modifiers::NONE,
            Some(main_owner()),
        );
        assert!(!app.notify_everything_search_shortcut_refusal(&ctx));
        assert!(ctx.input(|input| input.events.iter().any(|e| matches!(
            e,
            egui::Event::Key {
                key: egui::Key::F24,
                ..
            }
        ))));
        assert!(app.fs_feedback_toast.is_none());
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
        app.show_operation_customize = false;
        let mismatch = egui::Context::default();
        key_pass(
            &mismatch,
            egui::Key::F24,
            egui::Modifiers::CTRL,
            Some(main_owner()),
        );
        let before = mismatch.input(|input| input.events.clone());
        assert!(!app.notify_everything_search_shortcut_refusal(&mismatch));
        assert_eq!(mismatch.input(|input| input.events.clone()), before);
        let _ = crate::test_ui::headless_paint_output(mismatch.end_pass());
        key_pass(
            &ctx,
            egui::Key::R,
            egui::Modifiers::CTRL,
            Some(main_owner()),
        );
        assert!(!app.notify_everything_search_shortcut_refusal(&ctx));
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
        // Release was not sent above: use a new context for a fresh edge.
        let fresh = egui::Context::default();
        key_pass(
            &fresh,
            egui::Key::F24,
            egui::Modifiers::NONE,
            Some(main_owner()),
        );
        assert!(app.notify_everything_search_shortcut_refusal(&fresh));
        let _ = crate::test_ui::headless_paint_output(fresh.end_pass());
        app.fs_feedback_toast = None;
        app.keymap = crate::keymap::Keymap::from_ini_str("[Grid]\nGlobalEverythingSearch = none\n");
        let fresh = egui::Context::default();
        key_pass(
            &fresh,
            egui::Key::R,
            egui::Modifiers::CTRL,
            Some(main_owner()),
        );
        assert!(!app.notify_everything_search_shortcut_refusal(&fresh));
        assert!(app.fs_feedback_toast.is_none());
        let _ = crate::test_ui::headless_paint_output(fresh.end_pass());
    }

    #[test]
    fn everything_refusal_preserves_fullscreen_owned_root_commands() {
        #[cfg(windows)]
        let _serial = crate::key_input::lock_test_input();
        #[cfg(windows)]
        let _clear = ClearKeyFrame;
        let mut app = crate::app::setup_app_for_test();
        app.fullscreen_idx = Some(0);
        app.keymap =
            crate::keymap::Keymap::from_ini_str("[Grid]\nGlobalEverythingSearch = Ctrl+G\n");
        let ctx = egui::Context::default();
        key_pass(
            &ctx,
            egui::Key::G,
            egui::Modifiers::CTRL,
            Some(KeyboardOwner::ApplicationShortcut {
                scope: ShortcutScope::new(egui::ViewportId::ROOT, ShortcutSurface::Fullscreen),
            }),
        );
        let before = ctx.input(|input| input.events.clone());
        assert!(!app.dispatch_everything_search_shortcut(&ctx));
        assert!(app.fs_feedback_toast.is_none());
        assert_eq!(ctx.input(|input| input.events.clone()), before);
        // Grid's rebind must leave the fullscreen Ctrl+G editing action usable.
        assert!(
            app.keymap
                .pressed_action(&ctx, KeyAction::FsLocalAdjustMode)
        );
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
    }
}
