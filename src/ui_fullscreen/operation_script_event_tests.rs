//! Headless owning-pass event tests: no native window/application is launched.
use super::*;
use crate::app::operation_script_runtime::ScriptInputSurface;
use crate::operation_script::{
    ALLOWED_OPERATIONS, CompiledOperationScript, OperationScriptState, ScriptContext,
};

fn raw() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(600.0, 400.0),
        )),
        focused: true,
        ..Default::default()
    }
}

fn ready(ctx: &egui::Context) -> crate::app::AppTestEnvForTest {
    install_fs_navigator_input_tracking(ctx);
    crate::key_input::install_operation_input_plugin(ctx);
    let mut app = page_turn_submission_tests::ready_app(ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = false;
    app.settings.video_in_window_mode = true;
    app.viewer_presentation = crate::app::ViewerPresentation::MainWindow;
    #[cfg(windows)]
    {
        app.native_video_in_window_active = true;
    }
    app.ai_upscale_enabled = false;
    for idx in 0..4 {
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
        if let Some(FsCacheEntry::Static { pixels, .. }) = app.fs_cache.get_mut(&idx) {
            *pixels = Arc::new(egui::ColorImage::filled(
                [80, 120],
                egui::Color32::TRANSPARENT,
            ));
        }
    }
    app
}

fn event(app: &mut App, action: KeyAction) {
    app.settings.operation_script_enabled = true;
    app.operation_script_events = Default::default();
    app.operation_script.state = OperationScriptState::Ready(Box::new(
        CompiledOperationScript::compile(&format!(
            "fn slots() {{ [] }} fn on_event(s) {{ #{{run:[\"{}\"],notice:\"event complete\"}} }}",
            action.ini_name()
        ))
        .unwrap(),
    ));
}

#[cfg(windows)]
#[test]
fn operation_script_event_safe_images_resnapshot_then_real_draw_and_later_frames_stay_stable() {
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = ready(&ctx);
    // Settle pre-existing material/adoption before attributing a change to the A.
    ctx.run_ui(raw(), |ui| app.render_fullscreen_viewport(ui))
        .drop_without_applying_deltas();
    for operation in ALLOWED_OPERATIONS
        .iter()
        .filter(|op| op.event_allowed && op.contexts.contains(ScriptContext::FsImage))
    {
        event(&mut app, operation.action);
        let mut immediate = None;
        ctx.run_ui(raw(), |ui| {
            app.detect_operation_script_events(ui.ctx(), ScriptInputSurface::Viewer);
            assert_eq!(
                app.fs_feedback_toast.as_ref().unwrap().0,
                "event complete",
                "{:?}",
                operation.action
            );
            immediate =
                Some(app.collect_operation_script_event_identity(ScriptInputSurface::Viewer));
            app.render_fullscreen_viewport(ui);
        })
        .drop_without_applying_deltas();
        let immediate = immediate.unwrap();
        assert_eq!(
            app.collect_operation_script_event_identity(ScriptInputSurface::Viewer),
            immediate,
            "remaining draw: {:?}",
            operation.action
        );
        let count = match &app.operation_script.state {
            OperationScriptState::Ready(script) => script.last_operation_count(),
            _ => unreachable!(),
        };
        app.fs_feedback_toast = None;
        for _ in 0..2 {
            ctx.run_ui(raw(), |ui| app.render_fullscreen_viewport(ui))
                .drop_without_applying_deltas();
            assert_eq!(
                app.collect_operation_script_event_identity(ScriptInputSurface::Viewer),
                immediate,
                "later frame: {:?}",
                operation.action
            );
            assert!(
                app.fs_feedback_toast
                    .as_ref()
                    .is_none_or(|toast| toast.0 != "event complete"),
                "event re-ran: {:?}",
                operation.action
            );
            assert!(
                matches!(&app.operation_script.state, OperationScriptState::Ready(script) if script.last_operation_count() == count)
            );
        }
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn operation_script_event_background_and_analysis_pixel_absorb_unresolved_anchor_synchronously() {
    use crate::app::spread_shift_owner::ViewerShift;
    use crate::spread_shift::{
        ShiftPersistence, ShiftResolution, SpreadShiftPageRef, SpreadShiftState,
    };
    let ctx = egui::Context::default();
    let mut app = ready(&ctx);
    app.spread_mode = SpreadMode::Ltr;
    app.page_identity_index = Arc::new(
        crate::app::file_type_resume::PreparedPageIdentityIndex::prepare(
            &app.items,
            None,
            app.settings.file_type_visibility.clone(),
        ),
    );
    for action in [KeyAction::FsBgCycle, KeyAction::FsPixelGrid] {
        app.spread_shift = Default::default();
        app.spread_shift.scope =
            Some(crate::book_scope::BookScopeKey::folder("c:/page-turn").unwrap());
        app.spread_shift.state = ViewerShift::Book(SpreadShiftState::Adjusted {
            anchor_ref: SpreadShiftPageRef::RelativePath("1.png".into()),
            resolution: ShiftResolution::NotInView,
            persistence: ShiftPersistence::SessionOnly,
        });
        app.viewer_navigation_caches.invalidate();
        app.analysis_mode = action == KeyAction::FsPixelGrid;
        event(&mut app, action);
        ctx.run_ui(raw(), |ui| {
            assert_eq!(app.spread_shift.anchor_idx(), None);
            app.detect_operation_script_events(ui.ctx(), ScriptInputSurface::Viewer);
            assert_eq!(app.spread_shift.anchor_idx(), Some(1));
            let immediate = app.collect_operation_script_event_identity(ScriptInputSurface::Viewer);
            // The remaining preparation reads these same production resolvers.
            app.get_nav_indices();
            app.reconcile_fullscreen_page_slice(0);
            app.spread_display_composition_for_anchor(0);
            assert_eq!(
                app.collect_operation_script_event_identity(ScriptInputSurface::Viewer),
                immediate
            );
        })
        .drop_without_applying_deltas();
        app.fs_feedback_toast = None;
        ctx.run_ui(raw(), |ui| {
            app.detect_operation_script_events(ui.ctx(), ScriptInputSurface::Viewer)
        })
        .drop_without_applying_deltas();
        assert!(app.fs_feedback_toast.is_none(), "{action:?}");
    }
}

#[cfg(windows)]
#[test]
fn operation_script_event_loading_cache_arrival_does_not_retrigger_safe_a() {
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = ready(&ctx);
    let loaded = app.fs_cache.remove(&0).unwrap();
    event(&mut app, KeyAction::FsBgCycle);
    let mut immediate = None;
    let mut loaded = Some(loaded);
    ctx.run_ui(raw(), |ui| {
        app.detect_operation_script_events(ui.ctx(), ScriptInputSurface::Viewer);
        assert_eq!(app.fs_transparent_bg_mode, 1);
        immediate = Some(app.collect_operation_script_event_identity(ScriptInputSurface::Viewer));
        app.fs_cache.insert(0, loaded.take().unwrap());
        app.render_fullscreen_viewport(ui);
    })
    .drop_without_applying_deltas();
    assert_eq!(
        app.collect_operation_script_event_identity(ScriptInputSurface::Viewer),
        immediate.unwrap()
    );
    app.fs_feedback_toast = None;
    ctx.run_ui(raw(), |ui| app.render_fullscreen_viewport(ui))
        .drop_without_applying_deltas();
    assert_eq!(app.fs_transparent_bg_mode, 1);
}

#[test]
fn operation_script_event_direction_fit_and_single_have_late_slice_counterexamples() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx);
    app.record_page_dims_for_spread(0, (240, 120));
    if let Some(FsCacheEntry::Static { source_dims, .. }) = app.fs_cache.get_mut(&0) {
        *source_dims = Some([240, 120]);
    }
    for action in [
        KeyAction::FsReadingDirectionToggle,
        KeyAction::FsFitModeCycle,
        KeyAction::FsSpreadSingle,
    ] {
        app.spread_mode = SpreadMode::SplitLtr;
        app.fs_free_rotation = if action == KeyAction::FsSpreadSingle {
            0.0
        } else {
            15.0
        };
        app.fullscreen_page_slice = if action == KeyAction::FsSpreadSingle {
            crate::page_split::PageSlice::Left
        } else {
            crate::page_split::PageSlice::Full
        };
        let (_, rejected, _) = app.execute_fullscreen_script_action(&ctx, action);
        assert!(!rejected, "{action:?}");
        let immediate = app.collect_operation_script_event_identity(ScriptInputSurface::Viewer);
        app.reconcile_fullscreen_page_slice(0);
        assert_ne!(
            app.collect_operation_script_event_identity(ScriptInputSurface::Viewer),
            immediate,
            "{action:?}"
        );
        assert!(
            !ALLOWED_OPERATIONS
                .iter()
                .find(|op| op.action == action)
                .unwrap()
                .event_allowed
        );
    }
}

#[cfg(windows)]
#[test]
fn operation_script_event_embedded_detection_precedes_normal_key_and_slot_change_fires_next_frame()
{
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = ready(&ctx);
    app.settings.operation_script_enabled = true;
    app.keymap = crate::keymap::Keymap::from_ini_str("[Global]\nScriptSlot1 = F16\n");
    app.operation_script.state = OperationScriptState::Ready(Box::new(CompiledOperationScript::compile(
        r#"fn slots() { [#{slot:1,label:"next",run:"next"}] }
        fn next(s) { ["FsPageNext"] }
        fn on_event(s) { #{run:[],notice:if "page" in s.events { s.item.name } else { "other" }} }"#
    ).unwrap()));
    let mut input = raw();
    input.events.push(egui::Event::Key {
        key: egui::Key::F16,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    ctx.run_ui(input, |ui| app.render_fullscreen_viewport(ui))
        .drop_without_applying_deltas();
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(
        app.fs_feedback_toast.as_ref().unwrap().0,
        "0.png",
        "event saw the pre-key page"
    );
    ctx.run_ui(raw(), |ui| app.render_fullscreen_viewport(ui))
        .drop_without_applying_deltas();
    assert_eq!(app.fs_feedback_toast.as_ref().unwrap().0, "1.png");
    crate::key_input::clear_test_frame();
}

#[cfg(windows)]
#[test]
fn operation_script_event_child_pass_uses_child_text_focus_and_root_frame_number() {
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    ctx.set_embed_viewports(false);
    let focused = std::rc::Rc::new(std::cell::Cell::new(true));
    let child = std::rc::Rc::new(std::cell::Cell::new(egui::ViewportId::ROOT));
    let observed = child.clone();
    let text_focus = focused.clone();
    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
        observed.set(viewport.ids.this);
        let mut input = raw();
        input.viewport_id = viewport.ids.this;
        input.viewports.insert(
            viewport.ids.this,
            egui::ViewportInfo {
                parent: Some(egui::ViewportId::ROOT),
                focused: Some(true),
                ..Default::default()
            },
        );
        ctx.run_ui(input, |ui| {
            if text_focus.get() {
                let mut text = String::new();
                let mut focus = true;
                crate::ime_focus::show_singleline(ui, &mut text, Some(&mut focus), |edit| edit);
            } else {
                ctx.memory_mut(|memory| {
                    if let Some(id) = memory.focused() {
                        memory.surrender_focus(id);
                    }
                });
            }
            (viewport.viewport_ui_cb)(ui);
        })
        .drop_without_applying_deltas();
    });
    let mut app = ready(&ctx);
    app.settings.video_in_window_mode = false;
    app.native_video_in_window_active = false;
    app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
    event(&mut app, KeyAction::FsZoomIn);
    let zoom = app.fs_zoom;
    ctx.run_ui(raw(), |ui| app.render_fullscreen_viewport(ui))
        .drop_without_applying_deltas();
    assert_ne!(child.get(), egui::ViewportId::ROOT);
    assert_eq!(
        app.fs_zoom, zoom,
        "ROOT focus cannot admit a child's text-owned event"
    );
    focused.set(false);
    ctx.run_ui(raw(), |ui| app.render_fullscreen_viewport(ui))
        .drop_without_applying_deltas();
    assert!(app.fs_zoom > zoom);
    let zoom = app.fs_zoom;
    ctx.run_ui(raw(), |ui| app.render_fullscreen_viewport(ui))
        .drop_without_applying_deltas();
    assert_eq!(
        app.fs_zoom, zoom,
        "one event invocation across child/ROOT frame counts"
    );
    crate::key_input::clear_test_frame();
}

#[test]
fn operation_script_event_hook_is_before_forwarded_overview_and_normal_key_paths() {
    // Does not execute the known §1.897 broken ROOT forwarding path.
    let source = include_str!("../ui_fullscreen.rs");
    let body = source
        .split("let mut render_fs_body =")
        .nth(1)
        .unwrap()
        .split("render_fs_body(root_ui, true)")
        .next()
        .unwrap();
    let event = body.find("self.detect_operation_script_events(").unwrap();
    assert!(
        event
            < body
                .find("if let Some(root) = root_input.as_ref()")
                .unwrap()
    );
    assert!(event < body.find("page_overview.session()").unwrap());
    assert!(event < body.find("handle_fs_key_input_with_operation(").unwrap());
    assert_eq!(
        body.matches("self.detect_operation_script_events(").count(),
        1
    );
}
