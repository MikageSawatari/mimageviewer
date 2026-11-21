use super::*;

#[cfg(windows)]
#[test]
fn realmachine50_equal_rect_replacement_keeps_keys_on_owning_hwnd() {
    std::thread::spawn(|| {
        use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, RECT, WPARAM};
        use windows::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject, SetWindowRgn};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, GetWindowRect,
            IsWindowVisible, SW_SHOWNOACTIVATE, SendMessageW, ShowWindow,
            WM_KEYDOWN, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
        };
        use windows::core::w;
        struct EmptyWindow(HWND);
        impl Drop for EmptyWindow {
            fn drop(&mut self) { let _ = unsafe { DestroyWindow(self.0) }; }
        }
        let _input = crate::key_input::lock_test_input();
        let create = |visible| {
            let module = unsafe { GetModuleHandleW(None) }.unwrap();
            let hwnd = unsafe { CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW, w!("STATIC"),
                w!("mIV empty-region input ownership fixture"), WS_POPUP,
                60, 80, 500, 400, None, None, Some(HINSTANCE(module.0)), None,
            ) }.unwrap();
            let window = EmptyWindow(hwnd);
            // No pixels, activation, user input or application window. This is
            // the same empty-region fake-input surface used by native tests.
            let region = unsafe { CreateRectRgn(0, 0, 0, 0) };
            assert!(!region.is_invalid());
            let assigned = unsafe { SetWindowRgn(hwnd, Some(region), false) };
            if assigned == 0 { let _ = unsafe { DeleteObject(region.into()) }; }
            assert_ne!(assigned, 0);
            if visible { let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) }; }
            window
        };
        let _main = create(false);
        let first = create(true);
        let second = create(true);
        let mut rect = RECT::default();
        unsafe { GetWindowRect(first.0, &mut rect) }.unwrap();
        let mut same = RECT::default();
        unsafe { GetWindowRect(second.0, &mut same) }.unwrap();
        assert_eq!((rect.left, rect.top, rect.right, rect.bottom),
                   (same.left, same.top, same.right, same.bottom));
        assert!(unsafe { IsWindowVisible(first.0) }.as_bool());
        assert!(unsafe { IsWindowVisible(second.0) }.as_bool());
        // Native allocation ownership is explicit even while both windows have equal rects.
        let chosen = first.0;
        let replacement = second.0;
        let old_viewport = egui::ViewportId::from_hash_of("realmachine50-old-media");
        let new_viewport = egui::ViewportId::from_hash_of("realmachine50-new-still");
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        for viewport in [old_viewport, new_viewport] {
            ctx.show_viewport_deferred(viewport, Default::default(), |_, _| {});
        }
        use eframe::native_window_input::*;
        set_native_window_input_listener(&ctx, std::sync::Arc::new(crate::key_input::NativeKeyInputListener)).unwrap();
        let register = |hwnd: HWND, viewport_id| register_native_window_allocation(&ctx, NativeWindowAllocation {
            viewport_id, registration: ctx.viewport_registration(viewport_id).unwrap(),
            window_id: NativeWindowId::from(hwnd.0 as u64), windows_hwnd: Some(hwnd.0 as isize),
        });
        let mut old = register(chosen, old_viewport);
        let _new = register(replacement, new_viewport);
        let old_owner = native_window_allocation_witness(&ctx, old_viewport).unwrap();
        let new_owner = native_window_allocation_witness(&ctx, new_viewport).unwrap();
        unsafe { SendMessageW(replacement, WM_KEYDOWN, Some(WPARAM(0x44)), Some(LPARAM(1 | (0x20 << 16)))) };
        unsafe { SendMessageW(chosen, WM_KEYDOWN, Some(WPARAM(0x26)), Some(LPARAM(1 | (0x48 << 16)))) };
        crate::key_input::begin_frame();
        let edges = crate::key_input::native_operation_key_edges(new_viewport);
        let old_edges = crate::key_input::native_operation_key_edges(old_viewport);
        assert!(old_edges.iter().any(|edge| edge.source_hwnd == chosen.0 as u64
            && edge.source_viewport == old_viewport && edge.virtual_key == 0x26 && edge.pressed),
            "the older live HWND keeps its original viewport registration; edges={old_edges:?}");
        assert!(edges.iter().any(|edge| edge.source_hwnd == replacement.0 as u64
            && edge.source_viewport == new_viewport && edge.virtual_key == 0x44 && edge.pressed),
            "replacement viewport must receive D from its own HWND while an older HWND has the same rectangle; edges={edges:?}");
        assert!(edges.iter().all(|edge| edge.source_hwnd == replacement.0 as u64));
        assert!(old_edges.iter().all(|edge| edge.source_hwnd == chosen.0 as u64));
        old.retire();
        assert!(!old_owner.is_alive());
        assert!(new_owner.is_alive());
        // Retiring the allocation withdraws input while the old HWND still
        // exists, and its later lease Drop cannot erase the successor's edge.
        unsafe { SendMessageW(chosen, WM_KEYDOWN, Some(WPARAM(0x45)), Some(LPARAM(1 | (0x12 << 16)))) };
        unsafe { SendMessageW(replacement, WM_KEYDOWN, Some(WPARAM(0x26)), Some(LPARAM(1 | (0x48 << 16)))) };
        drop(old);
        crate::key_input::begin_frame();
        assert!(crate::key_input::native_operation_key_edges(old_viewport).is_empty());
        let successor_edges = crate::key_input::native_operation_key_edges(new_viewport);
        assert!(successor_edges.iter().any(|edge| edge.virtual_key == 0x26 && edge.pressed
            && edge.source_hwnd == replacement.0 as u64));
    }).join().unwrap();
}

#[test]
fn headless_fullscreen_focus_records_request_only_for_foreign_foreground() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for foreign in [false, true] {
        let foreground = 11;
        let target = if foreign { 22 } else { foreground };
        let _focus = crate::test_native_focus::scoped(foreground, target, !foreign);
        let ctx = egui::Context::default();
        install_fs_navigator_input_tracking(&ctx);
        let mut app = specified_scale_consumer_tests::pdf_app(&ctx, 1);
        app.viewer_presentation = ViewerPresentation::Fullscreen;
        app.settings.vst3_gui_visible = false;
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0));
        let draw = |app: &mut App, events| {
            crate::test_ui::headless_paint_output(ctx.run_pass(
                egui::RawInput {
                    screen_rect: Some(rect),
                    events,
                    ..Default::default()
                },
                |ctx| ctx.root_ui(|root_ui| app.render_fullscreen_viewport(root_ui)),
            ))
        };
        for _ in 0..5 {
            let _ = draw(&mut app, vec![]);
        }
        assert_eq!(app.fs_prev_foreground_hwnd, foreground as usize);
        // Opening has a separate focus grace period. Test a settled viewport click.
        app.fs_focus_regained_at = None;
        assert!(!app.fs_primary_suppression.is_active());
        let output = draw(
            &mut app,
            vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(
            crate::test_native_focus::requests(),
            if foreign { vec![target] } else { vec![] },
            "the real fullscreen input handler must preserve its focus decision",
        );
        let focus_command = output.viewport_output.values().any(|viewport| {
            viewport
                .commands
                .iter()
                .any(|cmd| matches!(cmd, egui::ViewportCommand::Focus))
        });
        assert_eq!(focus_command, foreign);
        assert_eq!(app.fs_primary_suppression.is_active(), foreign);
    }
}
