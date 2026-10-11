//! Pass-owned main-grid routing for Everything search. No transport is started here.
use crate::app::App;
use crate::keyboard_input::ShortcutSurface;
use crate::keymap::KeyAction;

impl App {
    pub(crate) fn dispatch_everything_search_shortcut(&mut self, ctx: &egui::Context) -> bool {
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
            modifiers,
            events: vec![egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers,
            }],
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
        assert!(ctx.input(|input| input.events.is_empty()));
        let _ = ctx.end_pass();

        app.keymap = crate::keymap::Keymap::from_ini_str("[Grid]\nGlobalEverythingSearch = F24\n");
        key_pass(
            &ctx,
            egui::Key::R,
            egui::Modifiers::CTRL,
            Some(main_owner()),
        );
        assert!(!app.dispatch_everything_search_shortcut(&ctx));
        let _ = ctx.end_pass();
        key_pass(
            &ctx,
            egui::Key::F24,
            egui::Modifiers::NONE,
            Some(main_owner()),
        );
        assert!(app.dispatch_everything_search_shortcut(&ctx));
        let _ = ctx.end_pass();
        app.keymap = crate::keymap::Keymap::from_ini_str("[Grid]\nGlobalEverythingSearch = none\n");
        key_pass(
            &ctx,
            egui::Key::R,
            egui::Modifiers::CTRL,
            Some(main_owner()),
        );
        assert!(!app.dispatch_everything_search_shortcut(&ctx));
        let _ = ctx.end_pass();
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
        assert_eq!(ctx.input(|input| input.events.len()), 1);
        let _ = ctx.end_pass();
        app.address_has_focus = false;
        // IME preedit/commit grace and focus recovery are pass-owned claims; the
        // real dispatcher must retain their edge for the field, not just skip open.
        for phase in [
            TextInputPhase::PendingFocus,
            TextInputPhase::Focused,
            TextInputPhase::FocusRecovery,
            TextInputPhase::ImeGrace,
        ] {
            key_pass(
                &ctx,
                egui::Key::R,
                egui::Modifiers::CTRL,
                Some(KeyboardOwner::TextInput {
                    viewport: egui::ViewportId::ROOT,
                    widget_id: egui::Id::new("everything-ime-field"),
                    phase,
                }),
            );
            assert!(!app.dispatch_everything_search_shortcut(&ctx), "{phase:?}");
            assert_eq!(ctx.input(|input| input.events.len()), 1, "{phase:?}");
            assert!(!app.global_search.active);
            let _ = ctx.end_pass();
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
        for owner in [
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
        ] {
            key_pass(&ctx, egui::Key::R, egui::Modifiers::CTRL, Some(owner));
            assert!(!app.dispatch_everything_search_shortcut(&ctx));
            assert_eq!(ctx.input(|input| input.events.len()), 1);
            assert!(!app.global_search.active);
            let _ = ctx.end_pass();
        }
        app.fullscreen_idx = Some(0);
        key_pass(
            &ctx,
            egui::Key::R,
            egui::Modifiers::CTRL,
            Some(main_owner()),
        );
        assert!(!app.dispatch_everything_search_shortcut(&ctx));
        assert_eq!(ctx.input(|input| input.events.len()), 1);
        let _ = ctx.end_pass();
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
            assert_eq!(ctx.input(|input| input.events.len()), 1);
            let _ = ctx.end_pass();
        });
        app.items = vec![crate::grid_item::GridItem::Video(
            app.tmp.path().join("fake.mp4"),
        )];
        app.fullscreen_idx = Some(0);
        app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
        key_pass(
            &ctx,
            egui::Key::R,
            egui::Modifiers::CTRL,
            Some(main_owner()),
        );
        assert!(!app.dispatch_everything_search_shortcut(&ctx));
        assert!(!app.global_search.active);
        assert_eq!(ctx.input(|input| input.events.len()), 1);
        let _ = ctx.end_pass();
    }
}
