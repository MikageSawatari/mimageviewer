use super::*;
use egui::{Color32, ColorImage, TextureHandle, TextureOptions};
use std::sync::Mutex as StdMutex;

#[derive(Default)]
struct RecordingApp {
    calls: Vec<(u64, usize, u64)>,
    old: Option<TextureHandle>,
    added: Option<TextureHandle>,
}

impl App for RecordingApp {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut crate::Frame) {
        if let Some(old) = self.old.as_mut() {
            old.set(
                ColorImage::filled([2, 2], Color32::RED),
                TextureOptions::NEAREST,
            );
            ui.painter().image(
                old.id(),
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(2.0, 2.0)),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }

    fn on_native_wgpu_root_output_finalized(
        &mut self,
        ctx: &egui::Context,
        frame: u64,
        passes: usize,
    ) {
        self.calls
            .push((frame, passes, ctx.cumulative_frame_nr_for(ViewportId::ROOT)));
        drop(self.old.take());
        self.added = Some(ctx.load_texture(
            "finalized-only",
            ColorImage::filled([2, 2], Color32::GREEN),
            TextureOptions::NEAREST,
        ));
    }
}

#[test]
fn finalized_root_runs_once_after_end_pass_retry_and_preserves_update_then_free() {
    for max_passes in [1, 2] {
        let mut integration = super::super::epi_integration::tests::headless_integration();
        let ctx = integration.egui_ctx.clone();
        ctx.options_mut(|o| o.max_passes = max_passes.try_into().unwrap());
        let plugin_passes = Arc::new(StdMutex::new(Vec::new()));
        let observed = plugin_passes.clone();
        ctx.on_end_pass(
            "late discard",
            Arc::new(move |ui| {
                observed.lock().unwrap().push(ui.ctx().current_pass_index());
                ui.ctx().request_discard("end-pass plugin");
            }),
        );
        let old = ctx.load_texture(
            "old drawn entry",
            ColorImage::filled([2, 2], Color32::WHITE),
            TextureOptions::NEAREST,
        );
        let id = old.id();
        ctx.tex_manager().write().take_delta().clear();
        let mut app = RecordingApp {
            old: Some(old),
            ..Default::default()
        };
        let producing = ctx.cumulative_frame_nr_for(ViewportId::ROOT);
        let delivered = Rc::new(RefCell::new(TexturesDelta::default()));
        let discarded = delivered.clone();
        let discarded_updates = Rc::new(core::cell::Cell::new(0));
        let discarded_count = discarded_updates.clone();
        let _scope = ctx.enter_discard_texture_delivery(move |delta| {
            if delta.set.contains_key(&id) {
                discarded_count.set(discarded_count.get() + 1);
            }
            super::super::viewport_lifecycle::append_texture_delta(
                &mut discarded.borrow_mut(),
                delta,
            );
        });
        let mut output = integration.update(&mut app, None, Default::default());
        assert!(
            app.calls.is_empty(),
            "App UI/end-pass cannot finalize the frame"
        );
        assert!(
            output.platform_output.requested_discard(),
            "max-pass still finalizes"
        );
        finalize_native_wgpu_root_output(&mut app, &ctx, ViewportId::ROOT, producing, &mut output);
        assert_eq!(app.calls, [(producing, max_passes, producing + 1)]);
        assert_eq!(
            *plugin_passes.lock().unwrap(),
            (0..max_passes).collect::<Vec<_>>()
        );
        assert_eq!(
            output.textures_delta.set[&id].len(),
            1,
            "hook free cannot cancel final-pass update"
        );
        assert!(output.textures_delta.free.contains(&id));
        assert!(
            output
                .textures_delta
                .set
                .contains_key(&app.added.as_ref().unwrap().id())
        );
        assert!(
            !output.shapes.is_empty(),
            "ordinary drawing survives finalization"
        );
        assert!(
            ctx.tex_manager().write().take_delta().is_empty(),
            "hook delta moved exactly once"
        );
        super::super::viewport_lifecycle::append_texture_delta(
            &mut delivered.borrow_mut(),
            core::mem::take(&mut output.textures_delta),
        );
        assert_eq!(discarded_updates.get(), max_passes - 1);
        // A newer full update supersedes the prior full pixel content; free must not.
        assert_eq!(delivered.borrow().set[&id].len(), 1);
        assert!(delivered.borrow().free.contains(&id));
        delivered.borrow_mut().clear();
        output.drop_without_applying_deltas();
    }
}

#[test]
fn finalized_root_excludes_deferred_immediate_retries_and_logic_only() {
    let ctx = egui::Context::default();
    let mut app = RecordingApp::default();
    let producing = ctx.cumulative_frame_nr_for(ViewportId::ROOT);
    let child = ViewportId::from_hash_of("finalize-child");
    ctx.set_embed_viewports(false);
    let immediate_passes = Arc::new(StdMutex::new(0));
    let observed = immediate_passes.clone();
    egui::Context::set_immediate_viewport_renderer(move |ctx, mut immediate| {
        let mut input = egui::RawInput {
            viewport_id: immediate.ids.this,
            ..Default::default()
        };
        input
            .viewports
            .entry(immediate.ids.this)
            .or_default()
            .parent = Some(immediate.ids.parent);
        let mut output = ctx.run_ui(input, |ui| {
            *observed.lock().unwrap() += 1;
            (immediate.viewport_ui_cb)(ui);
            if ui.ctx().current_pass_index() == 0 {
                ui.ctx().request_discard("child retry");
            }
        });
        let mut child_app = RecordingApp::default();
        finalize_native_wgpu_root_output(
            &mut child_app,
            ctx,
            immediate.ids.this,
            producing,
            &mut output,
        );
        assert!(child_app.calls.is_empty());
        output.drop_without_applying_deltas();
    });
    let mut output = ctx.run_ui(Default::default(), |ui| {
        ui.ctx()
            .show_viewport_immediate(child, Default::default(), |_, _| {});
    });
    assert_eq!(*immediate_passes.lock().unwrap(), 2);
    finalize_native_wgpu_root_output(&mut app, &ctx, ViewportId::ROOT, producing, &mut output);
    assert_eq!(app.calls.len(), 1);
    output.drop_without_applying_deltas();

    let mut input = egui::RawInput {
        viewport_id: child,
        ..Default::default()
    };
    input.viewports.entry(child).or_default().parent = Some(ViewportId::ROOT);
    let mut output = ctx.run_ui(input, |_| {});
    finalize_native_wgpu_root_output(&mut app, &ctx, child, producing, &mut output);
    assert_eq!(app.calls.len(), 1);
    output.drop_without_applying_deltas();

    let mut integration = super::super::epi_integration::tests::headless_integration();
    integration.update_logic_only(&mut app, Default::default());
    assert_eq!(app.calls.len(), 1, "LogicOnly has no Full finalizer");
}
