//! Non-drawing child Close acceptance, separate from input/UI and root shutdown.

use egui::{Context, RawInput, ViewportId};

use crate::{App, ViewportCloseResponse};

/// Native adapters borrow their resource maps only within each method. The
/// application hook must be free to inspect Context and enter its own owner.
pub(super) trait ChildCloseBackend {
    fn close_requests(&self) -> Vec<ViewportId>;
    fn consume_close(&mut self, id: ViewportId);
    fn registration(&self, ctx: &Context, id: ViewportId) -> Option<egui::ViewportRegistration> {
        ctx.viewport_registration(id)
    }
    fn retire(&mut self, ctx: &Context, approval: &egui::ApprovedViewportRetirement);
    /// Append cleanup-generated deltas to the existing shared renderer owner.
    /// A Full still uses its ordinary paint/no-paint submission exactly once.
    fn collect_retirement_delta(&mut self, _ctx: &Context) {}
    /// Complete delivery when no ordinary Full renderer consumer follows.
    fn finish_retirement(&mut self, ctx: &Context) {
        self.collect_retirement_delta(ctx);
    }
    /// Normal GC uses exactly the same resource cleanup, without a Close hook.
    fn gc(&mut self, _ctx: &Context) {}
}

/// Consume terminal commands before ordinary commands or resource-map lookups.
pub(super) fn consume_approved_retirements(
    ctx: &Context,
    backend: &mut impl ChildCloseBackend,
    commands: &mut Vec<egui::ViewportCommand>,
) {
    commands.retain(|command| {
        if let egui::ViewportCommand::RetireApproved(approval) = command {
            backend.retire(ctx, approval);
            false
        } else {
            true
        }
    });
}

/// LogicOnly uses the same consumer before native per-window command lookup.
pub(super) fn finish_logic_output(
    ctx: &Context,
    backend: &mut impl ChildCloseBackend,
    output: &mut egui::LogicOutput,
) {
    for commands in output.viewport_commands.values_mut() {
        consume_approved_retirements(ctx, backend, commands);
    }
    backend.finish_retirement(ctx);
}

/// Full/immediate deltas must already belong to the shared renderer. Called only
/// after the outer run returned, before ordinary output can recreate resources.
pub(super) fn finish_full_output(
    ctx: &Context,
    backend: &mut impl ChildCloseBackend,
    output: &mut egui::FullOutput,
) {
    for viewport in output.viewport_output.values_mut() {
        consume_approved_retirements(ctx, backend, &mut viewport.commands);
    }
    output
        .viewport_output
        .retain(|id, viewport| ctx.viewport_registration(*id) == Some(viewport.registration));
    backend.gc(ctx);
    // Output callback drops above may free textures too. The ordinary Full
    // renderer consumer applies the complete batch without an extra submission.
    backend.collect_retirement_delta(ctx);
}

/// Accept Full, immediate, discard and cleanup deltas into the one shared owner.
/// TexturesDelta::append intentionally discards earlier sets for newer frees;
/// that optimization must not erase this renderer-owned submission batch.
pub(super) fn append_texture_delta(
    pending: &mut egui::TexturesDelta,
    mut newer: egui::TexturesDelta,
) {
    for (id, updates) in newer.set.drain() {
        for update in updates {
            pending.push(id, update);
        }
    }
    pending.free.extend(core::mem::take(&mut newer.free));
}

/// Final non-drawing delivery before a shared renderer/Context ends.
/// Does not create new App close requests during root shutdown.
pub(super) fn finish_pending_retirements(ctx: &Context, backend: &mut impl ChildCloseBackend) {
    consume_approved_retirements(ctx, backend, &mut ctx.take_approved_viewport_retirements());
    backend.finish_retirement(ctx);
}

/// Called at native event completion or before/after a complete outer UI/logic
/// operation, never from immediate callbacks, plugin hooks or inside a UI pass.
pub(super) fn dispatch_child_close_requests(
    ctx: &Context,
    app: &mut dyn App,
    backend: &mut impl ChildCloseBackend,
) {
    // App-originated approval wins over the same old lifetime's OS request.
    consume_approved_retirements(ctx, backend, &mut ctx.take_approved_viewport_retirements());
    for id in backend.close_requests() {
        if id == ViewportId::ROOT || !backend.close_requests().contains(&id) {
            continue;
        }
        // Capture before entering user code, which may close/reopen the same ID.
        let registration = backend.registration(ctx, id);
        match app.on_viewport_close_requested(ctx, id) {
            ViewportCloseResponse::DelegateToUi => {}
            response => {
                backend.consume_close(id);
                if let Some(registration) = registration {
                    ctx.consume_registered_viewport_close(registration);
                }
                if matches!(response, ViewportCloseResponse::Close)
                    && let Some(registration) = registration
                {
                    ctx.approve_viewport_retirement(registration);
                }
            }
        }
        consume_approved_retirements(ctx, backend, &mut ctx.take_approved_viewport_retirements());
    }
    backend.finish_retirement(ctx);
}

/// Exact native ownership closure; an ID-only parent cannot authorize a new life.
pub(super) fn native_retirement_ids(
    ctx: &Context,
    approval: &egui::ApprovedViewportRetirement,
    owners: impl Iterator<
        Item = (
            ViewportId,
            Option<egui::ViewportRegistration>,
            Option<egui::ViewportRegistration>,
        ),
    >,
) -> (egui::ApprovedViewportRetirement, egui::ViewportIdSet) {
    let owners: Vec<_> = owners
        .filter_map(|(id, registration, parent)| {
            registration
                .or_else(|| ctx.viewport_registration(id))
                .map(|registration| (registration, parent))
        })
        .collect();
    let mut approval = approval.clone();
    approval.include_descendants(&owners);
    let ids = owners
        .iter()
        .filter_map(|(registration, _)| {
            approval
                .registrations()
                .contains(registration)
                .then_some(registration.viewport_id())
        })
        .collect();
    (approval, ids)
}

/// Explicit same-ID registration replacement before an immediate callback.
/// Only obsolete native owners end here: the Context already holds the new
/// registration, and its IME/input policy must remain untouched. This is not
/// execution of an App approval while a parent UI pass is active.
pub(super) fn stale_native_subtree(
    ctx: &Context,
    root: ViewportId,
    owners: impl Iterator<
        Item = (
            ViewportId,
            Option<egui::ViewportRegistration>,
            Option<egui::ViewportRegistration>,
        ),
    >,
) -> egui::ViewportIdSet {
    let owners: Vec<_> = owners.collect();
    let Some((_, old, _)) = owners.iter().find(|(id, _, _)| *id == root) else {
        return Default::default();
    };
    if *old == ctx.viewport_registration(root) {
        return Default::default();
    }
    let mut registrations: Vec<_> = old.iter().copied().collect();
    let mut ids = egui::ViewportIdSet::from_iter([root]);
    loop {
        let count = ids.len();
        for &(id, registration, parent) in &owners {
            if id != ViewportId::ROOT
                && parent.is_some_and(|parent| registrations.contains(&parent))
                && ids.insert(id)
            {
                if let Some(registration) = registration {
                    registrations.push(registration);
                }
            }
        }
        if ids.len() == count {
            return ids;
        }
    }
}

/// IME is ID-keyed; never erase a reopened registration's policy.
pub(super) fn finish_context_retirement(
    ctx: &Context,
    approval: &egui::ApprovedViewportRetirement,
) {
    let mut ids = ctx.retire_approved_viewports(approval);
    // Include Context descendants created under the same live parent after
    // approval, as well as native-only lifetimes whose Context GC already ran.
    for registration in approval.registrations() {
        let id = registration.viewport_id();
        if ctx.viewport_registration(id).is_none() && !ids.contains(&id) {
            ids.push(id);
        }
    }
    egui_ime::retire_viewport_ime_policy(ctx, &ids);
}

/// Remove only this lifecycle request from a pending metadata snapshot. Key,
/// pointer, focus, modifiers and IME events retain their order and owner.
pub(super) fn consume_close_in_input(input: &mut RawInput, id: ViewportId) {
    if let Some(info) = input.viewports.get_mut(&id) {
        info.events
            .retain(|event| *event != egui::ViewportEvent::Close);
    }
}

/// Drop native children before their ancestors, without changing live siblings.
pub(super) fn child_first_ids(
    retired: &egui::ViewportIdSet,
    parents: impl Iterator<Item = (ViewportId, ViewportId)>,
) -> Vec<ViewportId> {
    let parents: Vec<_> = parents.collect();
    let mut remaining = retired.clone();
    let mut ordered = Vec::new();
    while !remaining.is_empty() {
        let leaves: Vec<_> = remaining
            .iter()
            .copied()
            .filter(|id| {
                !parents.iter().any(|(child, parent)| {
                    child != parent && parent == id && remaining.contains(child)
                })
            })
            .collect();
        assert!(!leaves.is_empty(), "native viewport ownership cycle");
        for id in leaves {
            remaining.remove(&id);
            ordered.push(id);
        }
    }
    ordered
}

#[cfg(test)]
pub(super) mod viewport_retirement_tests {
    use super::super::epi_integration::{EpiIntegration, tests::headless_integration};
    use super::*;
    use egui::{FullOutput, OrderedViewportIdMap, ViewportOutput, ViewportRegistration};

    pub trait Adapter: ChildCloseBackend + Sized {
        fn new(ctx: &Context) -> Self;
        fn sync(&mut self, ctx: &Context, outputs: &OrderedViewportIdMap<ViewportOutput>);
        fn registration_for(&self, id: ViewportId) -> Option<ViewportRegistration>;
        fn set_close(&mut self, id: ViewportId);
        fn pending(&self, id: ViewportId) -> Vec<egui::Event>;
        fn accept_delta(&mut self, delta: egui::TexturesDelta);
        fn take_delta(&mut self) -> egui::TexturesDelta;
        fn replace_stale_native(&mut self, ctx: &Context, id: ViewportId);
        fn seed_stale_native(&mut self, ctx: &Context, id: ViewportId) -> egui::TextureId;
        fn native_input(&self, id: ViewportId) -> Vec<egui::Event>;
    }

    pub struct HeadlessDisplay;
    impl raw_window_handle::HasDisplayHandle for HeadlessDisplay {
        fn display_handle(
            &self,
        ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
            Err(raw_window_handle::HandleError::Unavailable)
        }
    }

    pub fn stale_texture(ctx: &Context) -> egui::TextureHandle {
        let mut texture = ctx.load_texture(
            "stale-native-texture",
            egui::ColorImage::filled([2, 2], egui::Color32::BLACK),
            egui::TextureOptions::NEAREST,
        );
        texture.set_partial(
            [1, 0],
            egui::ColorImage::filled([1, 1], egui::Color32::RED),
            egui::TextureOptions::NEAREST,
        );
        texture
    }

    pub fn stale_event() -> egui::Event {
        egui::Event::Text("old-native-must-not-transfer".into())
    }

    fn ids() -> (ViewportId, ViewportId, ViewportId) {
        (
            ViewportId::from_hash_of("retirement-child"),
            ViewportId::from_hash_of("retirement-descendant"),
            ViewportId::from_hash_of("retirement-sibling"),
        )
    }

    fn declare(ctx: &Context, id: ViewportId) {
        ctx.show_viewport_deferred(id, egui::ViewportBuilder::default(), |_, _| {});
    }

    pub fn input(id: ViewportId, ime: bool) -> egui::RawInput {
        let (child, descendant, sibling) = ids();
        let mut input = egui::RawInput {
            viewport_id: id,
            ..Default::default()
        };
        input
            .viewports
            .get_mut(&ViewportId::ROOT)
            .unwrap()
            .minimized = Some(true);
        for target in [child, descendant, sibling] {
            input.viewports.insert(
                target,
                egui::ViewportInfo {
                    parent: Some(if target == descendant {
                        child
                    } else {
                        ViewportId::ROOT
                    }),
                    minimized: Some(true),
                    ..Default::default()
                },
            );
        }
        if id != ViewportId::ROOT {
            input.viewports.entry(id).or_insert(egui::ViewportInfo {
                parent: Some(ViewportId::ROOT),
                ..Default::default()
            });
        }
        if ime {
            input.events.push(egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "composing".into(),
                active_range_chars: None,
            }));
        }
        input
    }

    fn fixture<A: Adapter>() -> (EpiIntegration, A) {
        let integration = headless_integration();
        let ctx = &integration.egui_ctx;
        let mut adapter = A::new(ctx);
        let (child, descendant, sibling) = ids();
        let output = ctx.run_ui(input(ViewportId::ROOT, false), |ui| {
            declare(ui.ctx(), child);
            declare(ui.ctx(), sibling);
        });
        deliver(ctx, &mut adapter, output);
        let output = ctx.run_ui(input(child, true), |ui| declare(ui.ctx(), descendant));
        deliver(ctx, &mut adapter, output);
        let output = ctx.run_ui(input(descendant, true), |_| {});
        deliver(ctx, &mut adapter, output);
        let output = ctx.run_ui(input(sibling, true), |_| {});
        deliver(ctx, &mut adapter, output);
        assert!(egui_ime::ime_input_active_in_viewport(ctx, child));
        assert!(egui_ime::ime_input_active_in_viewport(ctx, descendant));
        assert!(egui_ime::ime_input_active_in_viewport(ctx, sibling));
        (integration, adapter)
    }

    pub fn deliver<A: Adapter>(ctx: &Context, adapter: &mut A, mut output: FullOutput) {
        adapter.accept_delta(core::mem::take(&mut output.textures_delta));
        finish_full_output(ctx, adapter, &mut output);
        adapter.sync(ctx, &output.viewport_output);
        output.drop_without_applying_deltas();
    }

    struct ClosingApp {
        binding: Option<ViewportRegistration>,
        logic: usize,
        ui: usize,
        discard: bool,
        keep_child: bool,
        cancel_root: bool,
    }
    impl App for ClosingApp {
        fn logic(&mut self, ctx: &Context, _: &mut crate::Frame) {
            self.logic += 1;
            if let Some(binding) = self.binding.take() {
                ctx.approve_viewport_retirement(binding);
                assert!(
                    ctx.viewport_registration(binding.viewport_id()).is_some(),
                    "not retired inside App::logic"
                );
            } else if self.discard && self.logic == 2 {
                assert!(
                    ctx.has_pending_viewport_retirements(),
                    "Remote readiness must remain in progress during discard"
                );
            }
            if self.cancel_root {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
        fn ui(&mut self, ui: &mut egui::Ui, _: &mut crate::Frame) {
            self.ui += 1;
            let (child, _, sibling) = ids();
            declare(ui.ctx(), sibling);
            if self.keep_child {
                declare(ui.ctx(), child);
            }
            if self.discard && self.ui == 1 {
                ui.ctx().request_discard("retirement discard");
            }
        }
    }

    fn closing(ctx: &Context, discard: bool, keep_child: bool) -> ClosingApp {
        ClosingApp {
            binding: ctx.viewport_registration(ids().0),
            logic: 0,
            ui: 0,
            discard,
            keep_child,
            cancel_root: false,
        }
    }

    fn assert_retired<A: Adapter>(
        ctx: &Context,
        adapter: &A,
        sibling: ViewportRegistration,
        pending: &[egui::Event],
    ) {
        let (child, descendant, sibling_id) = ids();
        for id in [child, descendant] {
            assert!(ctx.viewport_registration(id).is_none());
            assert!(adapter.registration_for(id).is_none());
            assert!(!egui_ime::ime_input_active_in_viewport(ctx, id));
        }
        assert_eq!(adapter.registration_for(sibling_id), Some(sibling));
        assert_eq!(ctx.viewport_registration(sibling_id), Some(sibling));
        assert_eq!(adapter.pending(sibling_id), pending);
        assert!(egui_ime::ime_input_active_in_viewport(ctx, sibling_id));
    }

    pub fn full_logic_gc_discard<A: Adapter>() {
        for (discard, keep_child) in [(false, true), (false, false), (true, false)] {
            let (mut integration, mut adapter) = fixture::<A>();
            let ctx = integration.egui_ctx.clone();
            let sibling = ctx.viewport_registration(ids().2).unwrap();
            let pending = adapter.pending(ids().2);
            let mut app = closing(&ctx, discard, keep_child);
            let output = integration.update(&mut app, None, input(ViewportId::ROOT, false));
            assert_eq!(app.logic, if discard { 2 } else { 1 });
            assert_eq!(app.ui, app.logic);
            assert!(
                adapter.registration_for(ids().0).is_some(),
                "native cleanup waits for outer return"
            );
            assert!(
                output.viewport_output[&ViewportId::ROOT]
                    .commands
                    .iter()
                    .any(|cmd| matches!(cmd, egui::ViewportCommand::RetireApproved(_)))
            );
            deliver(&ctx, &mut adapter, output);
            assert_retired(&ctx, &adapter, sibling, &pending);
        }
    }

    pub fn logic_only_and_pass_outside_hook<A: Adapter>() {
        let (mut integration, mut adapter) = fixture::<A>();
        let ctx = integration.egui_ctx.clone();
        let sibling = ctx.viewport_registration(ids().2).unwrap();
        let pending = adapter.pending(ids().2);
        let mut app = closing(&ctx, false, false);
        let (mut output, _) =
            integration.update_logic_only(&mut app, input(ViewportId::ROOT, false));
        assert_eq!(app.ui, 0);
        assert!(
            ctx.viewport_registration(ids().0).is_some(),
            "LogicOnly creates no GC/UI"
        );
        let mut duplicate = output.clone();
        finish_logic_output(&ctx, &mut adapter, &mut output);
        assert_retired(&ctx, &adapter, sibling, &pending);
        // Reapplying a diagnostic clone is idempotent and does not invoke App.
        finish_logic_output(&ctx, &mut adapter, &mut duplicate);
        assert_eq!(app.logic, 1);
    }

    struct HookApp {
        binding: Option<ViewportRegistration>,
        response: crate::ViewportCloseResponse,
        calls: usize,
    }
    impl App for HookApp {
        fn ui(&mut self, _: &mut egui::Ui, _: &mut crate::Frame) {
            panic!("close hook ran UI")
        }
        fn on_viewport_close_requested(
            &mut self,
            ctx: &Context,
            _: ViewportId,
        ) -> crate::ViewportCloseResponse {
            self.calls += 1;
            if self.response == crate::ViewportCloseResponse::RetirementTransferred {
                ctx.approve_viewport_retirement(self.binding.take().unwrap());
            }
            self.response
        }
    }

    pub fn close_request_separation<A: Adapter>() {
        for response in [
            crate::ViewportCloseResponse::Close,
            crate::ViewportCloseResponse::RetirementTransferred,
        ] {
            let (integration, mut adapter) = fixture::<A>();
            let ctx = &integration.egui_ctx;
            let mut app = HookApp {
                binding: ctx.viewport_registration(ids().0),
                response: crate::ViewportCloseResponse::Cancel,
                calls: 0,
            };
            adapter.set_close(ids().0);
            dispatch_child_close_requests(ctx, &mut app, &mut adapter);
            dispatch_child_close_requests(ctx, &mut app, &mut adapter);
            assert_eq!(app.calls, 1, "modal rejection consumes only that request");
            assert!(adapter.registration_for(ids().0).is_some());
            app.response = response;
            adapter.set_close(ids().0);
            dispatch_child_close_requests(ctx, &mut app, &mut adapter);
            dispatch_child_close_requests(ctx, &mut app, &mut adapter);
            assert_eq!(app.calls, 2);
            assert!(adapter.registration_for(ids().0).is_none());
            assert!(ctx.viewport_registration(ids().0).is_none());
        }
        let (integration, mut adapter) = fixture::<A>();
        let ctx = &integration.egui_ctx;
        let mut app = HookApp {
            binding: None,
            response: crate::ViewportCloseResponse::DelegateToUi,
            calls: 0,
        };
        adapter.set_close(ids().0);
        dispatch_child_close_requests(ctx, &mut app, &mut adapter);
        assert!(
            adapter.close_requests().contains(&ids().0),
            "unmanaged UI delegation is preserved"
        );
        ctx.approve_viewport_retirement(ctx.viewport_registration(ids().0).unwrap());
        dispatch_child_close_requests(ctx, &mut app, &mut adapter);
        assert_eq!(
            app.calls, 1,
            "App-originated approval wins over old OS Close"
        );
    }

    pub fn context_gc_native_descendants_and_ime<A: Adapter>() {
        let (integration, mut adapter) = fixture::<A>();
        let ctx = &integration.egui_ctx;
        let (child, descendant, sibling_id) = ids();
        let registration = ctx.viewport_registration(child).unwrap();
        let sibling = ctx.viewport_registration(sibling_id).unwrap();
        let pending = adapter.pending(sibling_id);
        // Context GC can precede native cleanup and must not erase authority held
        // in an existing App binding. This entry deliberately bypasses plugin output.
        let _ = ctx.retire_viewport(child);
        assert!(egui_ime::ime_input_active_in_viewport(ctx, child));
        assert!(adapter.registration_for(descendant).is_some());
        ctx.approve_viewport_retirement(registration);
        let mut no_hook = HookApp {
            binding: None,
            response: crate::ViewportCloseResponse::DelegateToUi,
            calls: 0,
        };
        dispatch_child_close_requests(ctx, &mut no_hook, &mut adapter);
        assert_eq!(no_hook.calls, 0);
        assert_retired(ctx, &adapter, sibling, &pending);

        // A newly declared Context-only descendant still belongs to an approved
        // parent. Its IME state must end even when it had no native resource yet.
        let (integration, mut adapter) = fixture::<A>();
        let ctx = &integration.egui_ctx;
        let late = ViewportId::from_hash_of("retirement-late-descendant");
        let output = ctx.run_ui(input(child, false), |ui| {
            ui.ctx()
                .approve_viewport_retirement(ui.ctx().viewport_registration(child).unwrap());
            declare(ui.ctx(), late);
            let mut raw = input(late, true);
            raw.viewports.insert(
                late,
                egui::ViewportInfo {
                    parent: Some(child),
                    ..Default::default()
                },
            );
            let nested = ui.ctx().run_ui(raw, |_| {});
            assert!(egui_ime::ime_input_active_in_viewport(ui.ctx(), late));
            nested.drop_without_applying_deltas();
        });
        deliver(ctx, &mut adapter, output);
        assert!(ctx.viewport_registration(late).is_none());
        assert!(!egui_ime::ime_input_active_in_viewport(ctx, late));
    }

    pub fn old_output_reopen_and_duplicate<A: Adapter>() {
        let (integration, mut adapter) = fixture::<A>();
        let ctx = &integration.egui_ctx;
        let (child, _, sibling) = ids();
        let old = ctx.run_ui(input(ViewportId::ROOT, false), |ui| {
            declare(ui.ctx(), child);
            declare(ui.ctx(), sibling);
            ui.ctx()
                .send_viewport_cmd_to(child, egui::ViewportCommand::Title("stale-title".into()));
        });
        let old_registration = ctx.viewport_registration(child).unwrap();
        ctx.approve_viewport_retirement(old_registration);
        let commands = ctx.take_approved_viewport_retirements();
        let mut once = commands.clone();
        consume_approved_retirements(ctx, &mut adapter, &mut once);
        let output = ctx.run_ui(input(ViewportId::ROOT, false), |ui| {
            declare(ui.ctx(), child);
            declare(ui.ctx(), sibling);
        });
        let new_registration = ctx.viewport_registration(child).unwrap();
        assert_ne!(old_registration, new_registration);
        let mut combined = old;
        combined.append(output);
        deliver(ctx, &mut adapter, combined);
        assert_eq!(adapter.registration_for(child), Some(new_registration));
        consume_approved_retirements(ctx, &mut adapter, &mut commands.clone());
        adapter.finish_retirement(ctx);
        assert_eq!(ctx.viewport_registration(child), Some(new_registration));
        assert_eq!(adapter.registration_for(child), Some(new_registration));
    }

    pub fn overlapping_gc_and_root_close<A: Adapter>() {
        for (cancel_root, gc_first) in [(true, false), (false, false), (true, true), (false, true)]
        {
            let (mut integration, mut adapter) = fixture::<A>();
            let ctx = integration.egui_ctx.clone();
            let (child, descendant, _) = ids();
            ctx.approve_viewport_retirement(ctx.viewport_registration(descendant).unwrap());
            let mut app = closing(&ctx, false, false);
            app.cancel_root = cancel_root;
            let mut raw = input(ViewportId::ROOT, false);
            raw.viewports
                .get_mut(&ViewportId::ROOT)
                .unwrap()
                .events
                .push(egui::ViewportEvent::Close);
            let output = integration.update(&mut app, None, raw);
            assert_eq!(integration.should_close(), !cancel_root);
            if gc_first {
                adapter.gc(&ctx);
            }
            deliver(&ctx, &mut adapter, output);
            assert!(adapter.registration_for(child).is_none());
            assert!(adapter.registration_for(descendant).is_none());
            adapter.gc(&ctx);
            adapter.finish_retirement(&ctx);
            assert!(adapter.registration_for(ViewportId::ROOT).is_some());
        }
    }

    pub fn immediate_nested<A: Adapter + 'static>() {
        use std::{cell::RefCell, rc::Rc};
        let (integration, adapter) = fixture::<A>();
        let ctx = &integration.egui_ctx;
        let adapter = Rc::new(RefCell::new(adapter));
        let (child, descendant, sibling) = ids();
        let retained = adapter.clone();
        Context::set_immediate_viewport_renderer(move |ctx, mut immediate| {
            let output = ctx.run_ui(input(immediate.ids.this, false), |ui| {
                (immediate.viewport_ui_cb)(ui)
            });
            assert!(!output.viewport_output.values().any(|vp| {
                vp.commands
                    .iter()
                    .any(|cmd| matches!(cmd, egui::ViewportCommand::RetireApproved(_)))
            }));
            assert!(ctx.has_pending_viewport_retirements());
            // The real immediate adapters accept output while their parent is
            // active, but perform no GC or approved retirement at this boundary.
            let mut output = output;
            let mut adapter = retained.borrow_mut();
            adapter.accept_delta(core::mem::take(&mut output.textures_delta));
            adapter.sync(ctx, &output.viewport_output);
            output.drop_without_applying_deltas();
        });
        let output = ctx.run_ui(input(ViewportId::ROOT, false), |ui| {
            declare(ui.ctx(), sibling);
            ui.ctx()
                .show_viewport_immediate(child, egui::ViewportBuilder::default(), |ui, _| {
                    declare(ui.ctx(), descendant);
                    ui.ctx().approve_viewport_retirement(
                        ui.ctx().viewport_registration(child).unwrap(),
                    );
                });
            assert!(ui.ctx().viewport_registration(child).is_some());
            assert!(adapter.borrow().registration_for(child).is_some());
        });
        deliver(ctx, &mut *adapter.borrow_mut(), output);
        assert!(adapter.borrow().registration_for(child).is_none());
        assert!(adapter.borrow().registration_for(descendant).is_none());
        assert!(adapter.borrow().registration_for(sibling).is_some());
    }

    pub fn discard_immediate_reopen_replaces_native<A: Adapter + 'static>() {
        use std::{cell::RefCell, rc::Rc};
        let (integration, mut adapter) = fixture::<A>();
        let ctx = &integration.egui_ctx;
        let (child, descendant, sibling) = ids();
        let old = ctx.viewport_registration(child).unwrap();
        let texture_id = adapter.seed_stale_native(ctx, child);
        let adapter = Rc::new(RefCell::new(adapter));
        let retained = adapter.clone();
        Context::set_immediate_viewport_renderer(move |ctx, mut immediate| {
            assert_ne!(ctx.viewport_registration(child), Some(old));
            {
                let mut adapter = retained.borrow_mut();
                adapter.replace_stale_native(ctx, child);
                assert!(
                    adapter.registration_for(child).is_none(),
                    "old resource owner removed, never retagged"
                );
                assert!(
                    adapter.registration_for(descendant).is_none(),
                    "old native descendants share its lifetime"
                );
                let registration = ctx.viewport_registration(child).unwrap();
                let initial = egui::ViewportOutput {
                    registration,
                    parent_registration: ctx.viewport_registration(immediate.ids.parent),
                    parent: immediate.ids.parent,
                    class: egui::ViewportClass::Immediate,
                    builder: immediate.builder.clone(),
                    viewport_ui_cb: None,
                    commands: Vec::new(),
                    repaint_delay: Default::default(),
                };
                adapter.sync(ctx, &OrderedViewportIdMap::from_iter([(child, initial)]));
                assert_eq!(adapter.registration_for(child), Some(registration));
                assert!(!adapter.native_input(child).contains(&stale_event()));
                assert!(!adapter.close_requests().contains(&child));
            }
            let mut output = ctx.run_ui(input(child, false), |ui| (immediate.viewport_ui_cb)(ui));
            let mut adapter = retained.borrow_mut();
            adapter.accept_delta(core::mem::take(&mut output.textures_delta));
            adapter.sync(ctx, &output.viewport_output);
            output.drop_without_applying_deltas();
        });
        let mut passes = 0;
        let mut callbacks = 0;
        let output = ctx.run_ui(input(ViewportId::ROOT, false), |ui| {
            declare(ui.ctx(), sibling);
            if passes == 0 {
                // Actual Context GC ends the old child before discard pass 2.
                ui.ctx().request_discard("same-id immediate reopen");
            } else {
                ui.ctx().show_viewport_immediate(
                    child,
                    egui::ViewportBuilder::default(),
                    |ui, _| {
                        assert!(!ui.input(|input| input.events.contains(&stale_event())));
                        callbacks += 1;
                    },
                );
            }
            passes += 1;
        });
        assert_eq!(passes, 2);
        assert_eq!(callbacks, 1);
        deliver(ctx, &mut *adapter.borrow_mut(), output);
        assert_eq!(
            adapter.borrow().registration_for(child),
            ctx.viewport_registration(child)
        );
        assert_ne!(ctx.viewport_registration(child), Some(old));
        assert!(ctx.viewport_registration(sibling).is_some());
        let mut delta = adapter.borrow_mut().take_delta();
        assert!(
            delta.free.contains(&texture_id),
            "old native callback drop delta stayed with shared renderer"
        );
        delta.clear();
    }

    /// Headless native ownership evidence, distinct from the real wgpu readback
    /// in egui-wgpu. Uses real State, callback captures, registration GC and
    /// backend cleanup; it does not claim an OS surface or GL context.
    pub fn discard_texture_delivery_native_reopen<A: Adapter + 'static>() {
        use std::{
            cell::{Cell, RefCell},
            rc::Rc,
        };
        let (integration, mut adapter) = fixture::<A>();
        let ctx = &integration.egui_ctx;
        let (child, descendant, sibling) = ids();
        let old = ctx.viewport_registration(child).unwrap();
        let texture_id = adapter.seed_stale_native(ctx, child);
        let adapter = Rc::new(RefCell::new(adapter));
        let deliveries = Rc::new(Cell::new(0));
        let got = deliveries.clone();
        let receiver = Rc::downgrade(&adapter);
        let _scope = ctx.enter_discard_texture_delivery(move |delta| {
            assert_eq!(
                delta.set[&texture_id].len(),
                2,
                "whole and partial move before retry"
            );
            assert!(!delta.free.contains(&texture_id));
            let receiver = receiver.upgrade().unwrap();
            receiver.borrow_mut().accept_delta(delta);
            got.set(got.get() + 1);
        });
        let observed = deliveries.clone();
        let receiver = Rc::downgrade(&adapter);
        Context::set_immediate_viewport_renderer(move |ctx, mut immediate| {
            assert_eq!(
                observed.get(),
                1,
                "pass-1 delta delivered before native callback drop"
            );
            let receiver = receiver.upgrade().unwrap();
            {
                let mut adapter = receiver.borrow_mut();
                adapter.replace_stale_native(ctx, child);
                assert!(adapter.registration_for(child).is_none());
                assert!(adapter.registration_for(descendant).is_none());
                let mut delta = adapter.take_delta();
                assert!(
                    delta.free.contains(&texture_id),
                    "last native callback handle really dropped"
                );
                assert_eq!(
                    delta.set[&texture_id].len(),
                    2,
                    "pending preserves older sets despite later free"
                );
                assert_eq!(delta.set[&texture_id][0].pos, None);
                assert_eq!(delta.set[&texture_id][1].pos, Some([1, 0]));
                delta.clear(); // resource-consumer GPU evidence is a separate offscreen test
                let registration = ctx.viewport_registration(child).unwrap();
                let initial = egui::ViewportOutput {
                    registration,
                    parent_registration: ctx.viewport_registration(immediate.ids.parent),
                    parent: immediate.ids.parent,
                    class: egui::ViewportClass::Immediate,
                    builder: immediate.builder.clone(),
                    viewport_ui_cb: None,
                    commands: Vec::new(),
                    repaint_delay: Default::default(),
                };
                adapter.sync(ctx, &OrderedViewportIdMap::from_iter([(child, initial)]));
                assert!(!adapter.native_input(child).contains(&stale_event()));
            }
            let mut output = ctx.run_ui(input(child, false), |ui| (immediate.viewport_ui_cb)(ui));
            let mut adapter = receiver.borrow_mut();
            adapter.accept_delta(core::mem::take(&mut output.textures_delta));
            adapter.sync(ctx, &output.viewport_output);
            output.drop_without_applying_deltas();
        });
        let mut passes = 0;
        let output = ctx.run_ui(input(ViewportId::ROOT, false), |ui| {
            declare(ui.ctx(), sibling);
            if passes == 0 {
                ui.ctx().request_discard("delivery and native reopen");
            } else {
                ui.ctx()
                    .show_viewport_immediate(child, Default::default(), |_, _| {});
            }
            passes += 1;
        });
        assert_eq!(passes, 2);
        assert_eq!(deliveries.get(), 1);
        assert!(!output.textures_delta.set.contains_key(&texture_id));
        deliver(ctx, &mut *adapter.borrow_mut(), output);
        assert_ne!(ctx.viewport_registration(child), Some(old));
        assert_eq!(
            adapter.borrow().registration_for(child),
            ctx.viewport_registration(child)
        );
        assert!(ctx.viewport_registration(sibling).is_some());
        assert!(
            egui_ime::ime_input_active_in_viewport(ctx, sibling),
            "real IME plugin sibling ownership survives"
        );
        let mut pending = adapter.borrow_mut().take_delta();
        pending.clear();
    }

    pub fn legacy_hook_captures_lifetime_before_reopen<A: Adapter>() {
        let (integration, mut adapter) = fixture::<A>();
        let ctx = &integration.egui_ctx;
        let (child, _, sibling) = ids();
        struct ReopenApp {
            child: ViewportId,
            sibling: ViewportId,
            calls: usize,
        }
        impl App for ReopenApp {
            fn ui(&mut self, _: &mut egui::Ui, _: &mut crate::Frame) {
                panic!("no UI hook")
            }
            fn on_viewport_close_requested(
                &mut self,
                ctx: &Context,
                _: ViewportId,
            ) -> crate::ViewportCloseResponse {
                self.calls += 1;
                let _ = ctx.retire_viewport(self.child);
                declare(ctx, self.child);
                declare(ctx, self.sibling);
                // New-life Close must not be consumed as part of the old request.
                let mut raw = input(ViewportId::ROOT, false);
                raw.viewports
                    .get_mut(&self.child)
                    .unwrap()
                    .events
                    .push(egui::ViewportEvent::Close);
                let _ = ctx.run_logic(&raw, |_| {});
                crate::ViewportCloseResponse::Close
            }
        }
        let old = ctx.viewport_registration(child).unwrap();
        adapter.set_close(child);
        let mut app = ReopenApp {
            child,
            sibling,
            calls: 0,
        };
        dispatch_child_close_requests(ctx, &mut app, &mut adapter);
        let new = ctx.viewport_registration(child).unwrap();
        assert_ne!(old, new);
        assert_eq!(app.calls, 1);
        assert!(
            adapter.registration_for(child).is_none(),
            "only old native owner retired"
        );
        assert!(ctx.input(|input| input.raw.viewports[&child].close_requested()));
        assert!(ctx.viewport_registration(sibling).is_some());
    }

    pub fn delta_and_callback_drop<A: Adapter>() {
        for root_ending in [false, true] {
            let (mut integration, mut adapter) = fixture::<A>();
            let context = integration.egui_ctx.clone();
            let ctx = &context;
            let (child, _, sibling) = ids();
            let texture = ctx.load_texture(
                "retirement-drop-texture",
                egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
                Default::default(),
            );
            let texture_id = texture.id();
            let mut initial = ctx.take_viewport_retirement_texture_delta();
            adapter.accept_delta(core::mem::take(&mut initial));
            let mut partial = egui::TexturesDelta::default();
            partial.push(
                texture_id,
                egui::epaint::ImageDelta::partial(
                    [0, 0],
                    egui::ColorImage::filled([1, 1], egui::Color32::BLACK),
                    Default::default(),
                ),
            );
            adapter.accept_delta(partial);
            let output = ctx.run_ui(input(ViewportId::ROOT, false), |ui| {
                let texture = texture.clone();
                ui.ctx().show_viewport_deferred(
                    child,
                    egui::ViewportBuilder::default(),
                    move |_, _| {
                        let _ = &texture;
                    },
                );
                declare(ui.ctx(), sibling);
            });
            deliver(ctx, &mut adapter, output);
            drop(texture);
            if root_ending {
                let mut app = closing(ctx, false, false);
                let mut raw = input(ViewportId::ROOT, false);
                raw.viewports
                    .get_mut(&ViewportId::ROOT)
                    .unwrap()
                    .events
                    .push(egui::ViewportEvent::Close);
                let output = integration.update(&mut app, None, raw);
                assert!(integration.should_close());
                deliver(ctx, &mut adapter, output);
                finish_pending_retirements(ctx, &mut adapter);
            } else {
                ctx.approve_viewport_retirement(ctx.viewport_registration(child).unwrap());
                let mut hook = HookApp {
                    binding: None,
                    response: crate::ViewportCloseResponse::DelegateToUi,
                    calls: 0,
                };
                dispatch_child_close_requests(ctx, &mut hook, &mut adapter);
            }
            let mut delta = adapter.take_delta();
            assert_eq!(
                delta.set[&texture_id].len(),
                2,
                "whole/partial ordering retained in shared owner"
            );
            assert!(
                delta.free.contains(&texture_id),
                "Context/native callback drop free delivered without another pass"
            );
            delta.clear(); // headless fake renderer explicitly inspected delivery
        }
    }
}
