//! Shared root/deferred UI eligibility and output actions, independent of painting.

use super::winit_integration::PassMode;
use egui::{ViewportCommand, ViewportInfo};
use egui_winit::ActionRequested;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PassDecision {
    pub run_ui: bool,
    pub paint: bool,
}

impl PassDecision {
    /// The Windows minimized-Full policy does not change viewport metadata or
    /// turn an explicit LogicOnly tick into a UI pass.
    pub fn for_viewport(
        mode: PassMode,
        info: &ViewportInfo,
        has_visible_descendant: bool,
        actions_want_paint: bool,
        windows: bool,
    ) -> Self {
        let full = mode == PassMode::Full;
        let visible = info.visible().unwrap_or(true);
        Self {
            run_ui: full
                && (visible
                    || has_visible_descendant
                    || actions_want_paint
                    || (windows && info.minimized == Some(true))),
            paint: full && (visible || actions_want_paint),
        }
    }
}

/// Native preparation and renderer effects for the real immediate entry.
/// No viewport/context borrow may survive `prepare` or be held while `run_ui`
/// calls user code: an immediate callback may enter another immediate viewport.
pub(super) trait ImmediateViewportBackend {
    fn prepare(
        &mut self,
        ids: egui::ViewportIdPair,
        builder: egui::ViewportBuilder,
    ) -> Option<(egui::Context, egui::RawInput)>;

    fn run_ui(
        &mut self,
        _ids: egui::ViewportIdPair,
        ctx: &egui::Context,
        input: egui::RawInput,
        callback: &mut dyn FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        ctx.run_ui(input, callback)
    }

    /// Accept the shared renderer delta even if this viewport disappeared.
    fn accept_textures(
        &mut self,
        ids: egui::ViewportIdPair,
        ctx: &egui::Context,
        delta: egui::TexturesDelta,
    );

    /// Deliver platform/clipboard output and return this child's actual
    /// metadata and remaining explicit-paint actions, never the parent's.
    fn finish_output(
        &mut self,
        ids: egui::ViewportIdPair,
        ctx: &egui::Context,
        platform: egui::PlatformOutput,
        viewports: &mut egui::OrderedViewportIdMap<egui::ViewportOutput>,
    ) -> Option<(egui::ViewportInfo, bool)>;

    fn tessellate(
        &mut self,
        ctx: &egui::Context,
        shapes: Vec<egui::epaint::ClippedShape>,
        pixels_per_point: f32,
    ) -> Vec<egui::ClippedPrimitive> {
        ctx.tessellate(shapes, pixels_per_point)
    }

    fn paint(
        &mut self,
        ids: egui::ViewportIdPair,
        ctx: &egui::Context,
        primitives: Vec<egui::ClippedPrimitive>,
        pixels_per_point: f32,
    );

    fn apply_resources(&mut self, ids: egui::ViewportIdPair, ctx: &egui::Context);

    fn finish_viewports(
        &mut self,
        ids: egui::ViewportIdPair,
        ctx: &egui::Context,
        viewports: egui::OrderedViewportIdMap<egui::ViewportOutput>,
    );

    fn finished(&mut self, _ids: egui::ViewportIdPair, _ctx: &egui::Context) {}
}

/// Both native immediate registrations enter this driver. Renderer adapters
/// supply effects; input/UI/output ordering and child paint policy live here.
pub(super) fn run_immediate_viewport(
    backend: &mut impl ImmediateViewportBackend,
    immediate: egui::ImmediateViewport<'_>,
) {
    let egui::ImmediateViewport {
        ids,
        builder,
        mut viewport_ui_cb,
    } = immediate;
    let Some((ctx, input)) = backend.prepare(ids, builder) else {
        return;
    };
    let egui::FullOutput {
        platform_output,
        textures_delta,
        shapes,
        pixels_per_point,
        mut viewport_output,
    } = backend.run_ui(ids, &ctx, input, &mut viewport_ui_cb);

    backend.accept_textures(ids, &ctx, textures_delta);
    let paint = backend
        .finish_output(ids, &ctx, platform_output, &mut viewport_output)
        .is_some_and(|(info, wants_paint)| {
            PassDecision::for_viewport(
                PassMode::Full,
                &info,
                false,
                wants_paint,
                cfg!(target_os = "windows"),
            )
            .paint
        });
    if paint {
        let primitives = backend.tessellate(&ctx, shapes, pixels_per_point);
        backend.paint(ids, &ctx, primitives, pixels_per_point);
    } else {
        backend.apply_resources(ids, &ctx);
    }
    backend.finish_viewports(ids, &ctx, viewport_output);
    backend.finished(ids, &ctx);
}

/// Extract actions from this pass before deciding whether its surface needs
/// painting. Remaining native commands keep their order and normal owner.
pub(super) fn take_main_viewport_actions(
    commands: &mut Vec<ViewportCommand>,
    actions: &mut Vec<ActionRequested>,
) {
    for command in core::mem::take(commands) {
        let action = match command {
            ViewportCommand::RequestCut => ActionRequested::Cut,
            ViewportCommand::RequestCopy => ActionRequested::Copy,
            ViewportCommand::RequestPaste => ActionRequested::Paste,
            ViewportCommand::Screenshot(callback) => ActionRequested::Screenshot(callback),
            ViewportCommand::RequestPaintWhileHidden => ActionRequested::PaintWhileHidden,
            other => {
                commands.push(other);
                continue;
            }
        };
        actions.push(action);
    }
}

/// The clipboard and next-input queue belong to the action's viewport State.
pub(super) trait ClipboardInput {
    fn clipboard_text(&mut self) -> Option<String>;
    fn clipboard_image(&mut self) -> Option<egui::ColorImage>;
    fn input(&mut self) -> &mut egui::RawInput;
}

impl ClipboardInput for egui_winit::State {
    fn clipboard_text(&mut self) -> Option<String> {
        Self::clipboard_text(self)
    }

    fn clipboard_image(&mut self) -> Option<egui::ColorImage> {
        Self::clipboard_image(self)
    }

    fn input(&mut self) -> &mut egui::RawInput {
        self.egui_input_mut()
    }
}

pub(super) trait MainOutputTarget: ClipboardInput {
    fn handle_platform_output(&mut self, platform: egui::PlatformOutput);
}

/// Native adapter used only after user UI has returned, without holding a
/// viewport lock across the callback.
pub(super) struct WinitOutputTarget<'a> {
    pub state: &'a mut egui_winit::State,
    pub window: &'a winit::window::Window,
    pub event_loop: &'a winit::event_loop::ActiveEventLoop,
}

impl ClipboardInput for WinitOutputTarget<'_> {
    fn clipboard_text(&mut self) -> Option<String> {
        self.state.clipboard_text()
    }

    fn clipboard_image(&mut self) -> Option<egui::ColorImage> {
        self.state.clipboard_image()
    }

    fn input(&mut self) -> &mut egui::RawInput {
        self.state.egui_input_mut()
    }
}

impl MainOutputTarget for WinitOutputTarget<'_> {
    fn handle_platform_output(&mut self, platform: egui::PlatformOutput) {
        self.state
            .handle_platform_output_with_event_loop(self.window, self.event_loop, platform);
    }
}

/// Immediate output keeps its existing no-event-loop platform API. The
/// enclosing event-loop scope remains responsible for native viewport commands.
pub(super) struct WinitImmediateOutputTarget<'a> {
    pub state: &'a mut egui_winit::State,
    pub window: &'a winit::window::Window,
}

impl ClipboardInput for WinitImmediateOutputTarget<'_> {
    fn clipboard_text(&mut self) -> Option<String> {
        self.state.clipboard_text()
    }

    fn clipboard_image(&mut self) -> Option<egui::ColorImage> {
        self.state.clipboard_image()
    }

    fn input(&mut self) -> &mut egui::RawInput {
        self.state.egui_input_mut()
    }
}

impl MainOutputTarget for WinitImmediateOutputTarget<'_> {
    fn handle_platform_output(&mut self, platform: egui::PlatformOutput) {
        self.state.handle_platform_output(self.window, platform);
    }
}

/// Finish the current root/deferred pass's non-rendering output before choosing
/// its paint path. Native commands remain with the normal viewport handler.
pub(super) fn finish_main_viewport_output(
    platform: egui::PlatformOutput,
    commands: &mut Vec<ViewportCommand>,
    actions: &mut Vec<ActionRequested>,
    target: &mut impl MainOutputTarget,
) {
    take_main_viewport_actions(commands, actions);
    target.handle_platform_output(platform);
    process_non_paint_actions(actions, target);
}

/// Complete non-paint actions once, including on a Full pass that will not
/// paint. Keep capture/explicit-paint actions for the corresponding paint path.
pub(super) fn process_non_paint_actions(
    actions: &mut Vec<ActionRequested>,
    target: &mut impl ClipboardInput,
) {
    for action in core::mem::take(actions) {
        let event = match action {
            ActionRequested::Cut => Some(egui::Event::Cut),
            ActionRequested::Copy => Some(egui::Event::Copy),
            ActionRequested::Paste => {
                if let Some(contents) = target.clipboard_text() {
                    let contents = contents.replace("\r\n", "\n");
                    (!contents.is_empty()).then_some(egui::Event::Paste(contents))
                } else {
                    target
                        .clipboard_image()
                        .map(|image| egui::Event::PasteImage(std::sync::Arc::new(image)))
                }
            }
            paint_action @ (ActionRequested::Screenshot(_) | ActionRequested::PaintWhileHidden) => {
                actions.push(paint_action);
                continue;
            }
        };
        if let Some(event) = event {
            target.input().events.push(event);
        }
    }
}

/// Backend entry tests share effects only: the production immediate driver owns
/// the UI/output ordering and the decision whether to call these painter effects.
#[cfg(test)]
pub(super) mod immediate_tests {
    use super::*;
    use egui::{Context, Event, RawInput, ViewportId, ViewportIdPair};
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{Arc, Mutex},
    };

    type Entry = dyn for<'a> Fn(&mut FakeImmediateBackend, egui::ImmediateViewport<'a>);

    #[derive(Clone, Copy, Debug, Default)]
    struct Scenario {
        child_visible: bool,
        existing_capture: bool,
        current_capture: bool,
        existing_paint: bool,
        current_paint: bool,
        parent_capture: bool,
    }

    #[derive(Default)]
    struct Owner {
        input: RawInput,
        platform: Vec<egui::PlatformOutput>,
        clipboard: String,
    }

    impl ClipboardInput for Owner {
        fn clipboard_text(&mut self) -> Option<String> {
            Some(self.clipboard.clone())
        }
        fn clipboard_image(&mut self) -> Option<egui::ColorImage> {
            panic!("text clipboard is present")
        }
        fn input(&mut self) -> &mut RawInput {
            &mut self.input
        }
    }

    impl MainOutputTarget for Owner {
        fn handle_platform_output(&mut self, platform: egui::PlatformOutput) {
            for command in &platform.commands {
                if let egui::OutputCommand::CopyText(text) = command {
                    self.clipboard.clone_from(text);
                }
            }
            self.platform.push(platform);
        }
    }

    #[derive(Default)]
    struct Effects {
        info: egui::ViewportIdMap<ViewportInfo>,
        owners: egui::ViewportIdMap<Owner>,
        actions: egui::ViewportIdMap<Vec<ActionRequested>>,
        pending: egui::TexturesDelta,
        order: Vec<&'static str>,
        painted: Vec<ViewportId>,
        parent_painted: Vec<ViewportId>,
        accepted: usize,
        applied: usize,
        applied_textures: Vec<egui::TextureId>,
        applied_frees: Vec<egui::TextureId>,
        commands: Vec<(ViewportId, ViewportCommand)>,
        callback_inputs: Vec<Vec<Event>>,
        created_textures: Vec<egui::TextureId>,
        surface_missing: bool,
        removed_owner: Option<ViewportId>,
    }

    impl Drop for Effects {
        fn drop(&mut self) {
            // Test-only unwinding cleanup: preserve the first failed assertion
            // instead of masking it with unapplied-delta Drop's second panic.
            self.pending.clear();
        }
    }

    /// Public only so both native modules can instantiate their actual generic
    /// immediate entry with this resource adapter.
    pub(crate) struct FakeImmediateBackend {
        ctx: Context,
        effects: Rc<RefCell<Effects>>,
    }

    fn dispatch_commands(
        effects: &mut Effects,
        viewports: &mut egui::OrderedViewportIdMap<egui::ViewportOutput>,
    ) {
        for (&id, output) in viewports {
            if effects.removed_owner == Some(id) {
                output.commands.clear();
                continue;
            }
            take_main_viewport_actions(
                &mut output.commands,
                effects.actions.entry(id).or_default(),
            );
            effects.commands.extend(
                core::mem::take(&mut output.commands)
                    .into_iter()
                    .map(|command| (id, command)),
            );
        }
    }

    fn apply(effects: &mut Effects) {
        effects.applied += 1;
        let allocated: Vec<_> = effects.pending.set.iter().map(|(id, _)| *id).collect();
        effects.applied_textures.extend(allocated);
        let freed = effects.pending.free.clone();
        effects.applied_frees.extend(freed);
        effects.pending.clear();
    }

    impl ImmediateViewportBackend for FakeImmediateBackend {
        fn prepare(
            &mut self,
            ids: ViewportIdPair,
            _builder: egui::ViewportBuilder,
        ) -> Option<(Context, RawInput)> {
            let mut effects = self.effects.borrow_mut();
            effects.order.push("prepare");
            let mut input = effects.owners.entry(ids.this).or_default().input.take();
            input.viewport_id = ids.this;
            input.viewports = effects.info.clone();
            input.focused = true;
            input.screen_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 300.0),
            ));
            Some((self.ctx.clone(), input))
        }

        fn accept_textures(
            &mut self,
            _ids: ViewportIdPair,
            _ctx: &Context,
            delta: egui::TexturesDelta,
        ) {
            let mut effects = self.effects.borrow_mut();
            effects.order.push("textures");
            effects.accepted += 1;
            effects.pending.append(delta);
        }

        fn finish_output(
            &mut self,
            ids: ViewportIdPair,
            _ctx: &Context,
            platform: egui::PlatformOutput,
            viewports: &mut egui::OrderedViewportIdMap<egui::ViewportOutput>,
        ) -> Option<(ViewportInfo, bool)> {
            let mut effects = self.effects.borrow_mut();
            effects.order.push("output");
            if effects.removed_owner == Some(ids.this) {
                return None;
            }
            let surface_missing = effects.surface_missing;
            let Effects {
                owners,
                actions,
                info,
                ..
            } = &mut *effects;
            let actions = actions.entry(ids.this).or_default();
            finish_main_viewport_output(
                platform,
                &mut viewports.get_mut(&ids.this).unwrap().commands,
                actions,
                owners.entry(ids.this).or_default(),
            );
            let facts = (
                info[&ids.this].clone(),
                actions.iter().any(ActionRequested::wants_paint),
            );
            (!surface_missing).then_some(facts)
        }

        fn tessellate(
            &mut self,
            ctx: &Context,
            shapes: Vec<egui::epaint::ClippedShape>,
            pixels_per_point: f32,
        ) -> Vec<egui::ClippedPrimitive> {
            self.effects.borrow_mut().order.push("tessellate");
            ctx.tessellate(shapes, pixels_per_point)
        }

        fn paint(
            &mut self,
            ids: ViewportIdPair,
            _ctx: &Context,
            _primitives: Vec<egui::ClippedPrimitive>,
            _pixels_per_point: f32,
        ) {
            let mut effects = self.effects.borrow_mut();
            effects.order.push("paint");
            effects.painted.push(ids.this);
            apply(&mut effects);
            for action in core::mem::take(effects.actions.entry(ids.this).or_default()) {
                match action {
                    ActionRequested::Screenshot(callback) => callback.complete(Arc::new(
                        egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
                    )),
                    ActionRequested::PaintWhileHidden => {}
                    _ => panic!("non-paint action reached painter"),
                }
            }
        }

        fn apply_resources(&mut self, _ids: ViewportIdPair, _ctx: &Context) {
            let mut effects = self.effects.borrow_mut();
            effects.order.push("resources");
            apply(&mut effects);
        }

        fn finish_viewports(
            &mut self,
            _ids: ViewportIdPair,
            _ctx: &Context,
            mut viewports: egui::OrderedViewportIdMap<egui::ViewportOutput>,
        ) {
            let mut effects = self.effects.borrow_mut();
            effects.order.push("viewports");
            dispatch_commands(&mut effects, &mut viewports);
        }

        fn finished(&mut self, _ids: ViewportIdPair, _ctx: &Context) {
            self.effects.borrow_mut().order.push("finished");
        }
    }

    fn capture(
        label: &'static str,
        completed: &Arc<Mutex<Vec<&'static str>>>,
    ) -> egui::ScreenshotCallback {
        let completed = Arc::clone(completed);
        egui::ScreenshotCallback::new(move |image| {
            assert_eq!(image.size, if label == "parent" { [9, 7] } else { [2, 2] });
            completed.lock().unwrap().push(label);
        })
    }

    /// The parent surface effect is separate from the actual child entry under
    /// test. This uses the production main-path decision helper, but does not
    /// claim to invoke the native parent main renderer in this headless harness.
    fn finish_fake_parent_surface(effects: &mut Effects) {
        let info = effects.info[&ViewportId::ROOT].clone();
        let actions = effects.actions.entry(ViewportId::ROOT).or_default();
        let decision = PassDecision::for_viewport(
            PassMode::Full,
            &info,
            false,
            actions.iter().any(ActionRequested::wants_paint),
            cfg!(target_os = "windows"),
        );
        if decision.paint {
            effects.parent_painted.push(ViewportId::ROOT);
            for action in core::mem::take(actions) {
                match action {
                    ActionRequested::Screenshot(callback) => callback.complete(Arc::new(
                        egui::ColorImage::filled([9, 7], egui::Color32::WHITE),
                    )),
                    ActionRequested::PaintWhileHidden => {}
                    _ => panic!("parent non-paint action reached painter"),
                }
            }
        }
    }

    fn run_nested_scenario(entry: Rc<Entry>, scenario: Scenario) {
        let ctx = Context::default();
        ctx.set_embed_viewports(false);
        let child = ViewportId::from_hash_of("immediate-entry-child");
        let effects = Rc::new(RefCell::new(Effects::default()));
        let completed = Arc::new(Mutex::new(Vec::new()));
        let current = capture("current", &completed);
        {
            let mut effects = effects.borrow_mut();
            effects.info.insert(
                ViewportId::ROOT,
                ViewportInfo {
                    minimized: Some(true),
                    occluded: Some(false),
                    ..Default::default()
                },
            );
            effects.info.insert(
                child,
                ViewportInfo {
                    minimized: Some(!scenario.child_visible),
                    occluded: Some(false),
                    ..Default::default()
                },
            );
            if scenario.existing_capture {
                effects
                    .actions
                    .entry(child)
                    .or_default()
                    .push(ActionRequested::Screenshot(capture("existing", &completed)));
            }
            if scenario.existing_paint {
                effects
                    .actions
                    .entry(child)
                    .or_default()
                    .push(ActionRequested::PaintWhileHidden);
            }
            if scenario.parent_capture {
                effects
                    .actions
                    .entry(ViewportId::ROOT)
                    .or_default()
                    .push(ActionRequested::Screenshot(capture("parent", &completed)));
            }
        }
        let renderer_effects = Rc::clone(&effects);
        Context::set_immediate_viewport_renderer(move |ctx, immediate| {
            entry(
                &mut FakeImmediateBackend {
                    ctx: ctx.clone(),
                    effects: Rc::clone(&renderer_effects),
                },
                immediate,
            );
        });

        let mut child_text = String::new();
        for pass in 0..3 {
            effects.borrow_mut().order.clear();
            let mut raw = RawInput {
                events: vec![Event::Text("parent-only".to_owned())],
                ..Default::default()
            };
            raw.viewports = effects.borrow().info.clone();
            let mut output = ctx.run_ui(raw, |ui| {
                assert_eq!(ui.ctx().viewport_id(), ViewportId::ROOT);
                ui.ctx().show_viewport_immediate(
                    child,
                    egui::ViewportBuilder::default(),
                    |ui, class| {
                        assert!(class == egui::ViewportClass::Immediate);
                        assert_eq!(ui.ctx().viewport_id(), child);
                        assert_eq!(
                            ui.input(|i| i.viewport().minimized),
                            Some(!scenario.child_visible)
                        );
                        // This borrow would panic if preparation kept the native
                        // resource borrow across the actual nested user callback.
                        {
                            let mut effects = effects.borrow_mut();
                            effects.order.push("callback");
                            effects.callback_inputs.push(ui.input(|i| i.events.clone()));
                        }
                        let field = egui::Id::unique("immediate-entry-field");
                        ui.memory_mut(|memory| memory.request_focus(field));
                        ui.add(egui::TextEdit::singleline(&mut child_text).id(field));
                        ui.ctx().copy_text("child\r\nclip".to_owned());
                        let texture = ui.ctx().load_texture(
                            "immediate-entry-resource",
                            egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
                            egui::TextureOptions::LINEAR,
                        );
                        effects.borrow_mut().created_textures.push(texture.id());
                        drop(texture);
                        if pass == 0 {
                            for command in [
                                ViewportCommand::RequestCut,
                                ViewportCommand::RequestCopy,
                                ViewportCommand::RequestPaste,
                                ViewportCommand::Title("child-title".to_owned()),
                            ] {
                                ui.ctx().send_viewport_cmd(command);
                            }
                            if scenario.current_capture {
                                ui.ctx().send_viewport_cmd(ViewportCommand::Screenshot(
                                    current.clone(),
                                ));
                            }
                            if scenario.current_paint {
                                ui.ctx()
                                    .send_viewport_cmd(ViewportCommand::RequestPaintWhileHidden);
                            }
                        }
                    },
                );
                assert_eq!(
                    ui.ctx().viewport_id(),
                    ViewportId::ROOT,
                    "immediate child must restore its parent Context"
                );
            });
            let expected_paint = scenario.child_visible
                || (pass == 0 && (scenario.existing_capture || scenario.existing_paint))
                || (pass == 1 && (scenario.current_capture || scenario.current_paint));
            let expected_order = if expected_paint {
                vec![
                    "prepare",
                    "callback",
                    "textures",
                    "output",
                    "tessellate",
                    "paint",
                    "viewports",
                    "finished",
                ]
            } else {
                vec![
                    "prepare",
                    "callback",
                    "textures",
                    "output",
                    "resources",
                    "viewports",
                    "finished",
                ]
            };
            assert_eq!(
                effects.borrow().order,
                expected_order,
                "{scenario:?}, pass {pass}"
            );
            let mut expected_captures = Vec::new();
            if scenario.existing_capture {
                expected_captures.push("existing");
            }
            if scenario.parent_capture {
                expected_captures.push("parent");
            }
            if pass >= 1 && scenario.current_capture {
                expected_captures.push("current");
            }
            // egui intentionally emits nested viewport commands only with the
            // outermost output. Queue them through the ordinary action owner;
            // the next existing Full pass services them without a retry wake.
            dispatch_commands(&mut effects.borrow_mut(), &mut output.viewport_output);
            finish_fake_parent_surface(&mut effects.borrow_mut());
            assert_eq!(
                *completed.lock().unwrap(),
                expected_captures,
                "{scenario:?}, pass {pass}"
            );
            assert_eq!(
                effects.borrow().parent_painted.len(),
                usize::from(scenario.parent_capture),
                "parent paint is counted separately from child paint, {scenario:?}, pass {pass}"
            );
            output.drop_without_applying_deltas();
        }
        let effects = effects.borrow();
        assert_eq!(effects.accepted, 3);
        assert_eq!(effects.applied, 3);
        assert!(effects.pending.is_empty());
        assert!(
            effects
                .created_textures
                .iter()
                .all(|id| effects.applied_textures.contains(id))
        );
        assert!(
            effects.painted.iter().all(|id| *id == child),
            "child entry cannot paint the parent"
        );
        let expected_child_paints = if scenario.child_visible {
            3
        } else {
            usize::from(scenario.existing_capture || scenario.existing_paint)
                + usize::from(scenario.current_capture || scenario.current_paint)
        };
        assert_eq!(effects.painted.len(), expected_child_paints);
        assert_eq!(
            effects.parent_painted,
            vec![ViewportId::ROOT; usize::from(scenario.parent_capture)]
        );
        assert_eq!(effects.info[&ViewportId::ROOT].visible(), Some(false));
        assert_eq!(
            effects.callback_inputs,
            vec![
                vec![],
                vec![],
                vec![
                    Event::Cut,
                    Event::Copy,
                    Event::Paste("child\nclip".to_owned())
                ]
            ]
        );
        let owner = &effects.owners[&child];
        assert_eq!(owner.platform.len(), 3);
        assert!(owner.platform.iter().all(|platform| platform.ime.is_some()));
        assert!(
            owner.input.events.is_empty(),
            "third Full must consume clipboard events once"
        );
        assert_eq!(
            effects
                .commands
                .iter()
                .filter(|(id, command)| *id == child
                    && matches!(command, ViewportCommand::Title(title) if title == "child-title"))
                .count(),
            1
        );
        assert_eq!(
            effects.actions.get(&ViewportId::ROOT).map_or(0, Vec::len),
            0
        );
        assert!(
            !effects.owners.contains_key(&ViewportId::ROOT),
            "child output cannot use parent State"
        );
    }

    fn run_direct_current_output(entry: Rc<Entry>) {
        let ctx = Context::default();
        ctx.set_embed_viewports(false);
        let child = ViewportId::from_hash_of("immediate-entry-direct-child");
        let effects = Rc::new(RefCell::new(Effects::default()));
        for id in [ViewportId::ROOT, child] {
            effects.borrow_mut().info.insert(
                id,
                ViewportInfo {
                    minimized: Some(true),
                    occluded: Some(false),
                    ..Default::default()
                },
            );
        }
        let completed = Arc::new(Mutex::new(Vec::new()));
        let current = capture("direct-current", &completed);
        let mut backend = FakeImmediateBackend {
            ctx,
            effects: Rc::clone(&effects),
        };
        let mut text = String::new();
        entry(
            &mut backend,
            egui::ImmediateViewport {
                ids: ViewportIdPair::from_self_and_parent(child, ViewportId::ROOT),
                builder: egui::ViewportBuilder::default(),
                viewport_ui_cb: Box::new(|ui| {
                    effects.borrow_mut().order.push("callback");
                    let id = egui::Id::unique("immediate-entry-direct-field");
                    ui.memory_mut(|memory| memory.request_focus(id));
                    ui.add(egui::TextEdit::singleline(&mut text).id(id));
                    ui.ctx().copy_text("direct\r\nclipboard".to_owned());
                    for command in [
                        ViewportCommand::RequestCopy,
                        ViewportCommand::RequestPaste,
                        ViewportCommand::Title("direct-title".to_owned()),
                        ViewportCommand::Screenshot(current.clone()),
                    ] {
                        ui.ctx().send_viewport_cmd(command);
                    }
                }),
            },
        );
        let effects = effects.borrow();
        assert_eq!(
            effects.painted,
            vec![child],
            "this entry's current Screenshot forces its own paint"
        );
        assert_eq!(*completed.lock().unwrap(), vec!["direct-current"]);
        assert_eq!(
            effects.owners[&child].input.events,
            vec![Event::Copy, Event::Paste("direct\nclipboard".to_owned())]
        );
        assert!(effects.actions[&child].is_empty());
        assert_eq!(
            effects
                .commands
                .iter()
                .filter(|(id, command)| *id == child
                    && matches!(command, ViewportCommand::Title(title) if title == "direct-title"))
                .count(),
            1
        );
        assert_eq!(
            effects.order,
            vec![
                "prepare",
                "callback",
                "textures",
                "output",
                "tessellate",
                "paint",
                "viewports",
                "finished"
            ]
        );
    }

    fn run_missing_facts(entry: Rc<Entry>, remove_owner: bool) {
        let ctx = Context::default();
        ctx.set_embed_viewports(false);
        let child = ViewportId::from_hash_of("immediate-entry-missing-surface");
        let effects = Rc::new(RefCell::new(Effects::default()));
        for id in [ViewportId::ROOT, child] {
            effects.borrow_mut().info.insert(
                id,
                ViewportInfo {
                    minimized: Some(false),
                    occluded: Some(false),
                    ..Default::default()
                },
            );
        }
        let completed = Arc::new(Mutex::new(Vec::new()));
        effects
            .borrow_mut()
            .actions
            .entry(child)
            .or_default()
            .push(ActionRequested::Screenshot(capture("existing", &completed)));
        let current = capture("current", &completed);
        let mut backend = FakeImmediateBackend {
            ctx,
            effects: Rc::clone(&effects),
        };
        let mut text = String::new();
        entry(
            &mut backend,
            egui::ImmediateViewport {
                ids: ViewportIdPair::from_self_and_parent(child, ViewportId::ROOT),
                builder: egui::ViewportBuilder::default(),
                viewport_ui_cb: Box::new(|ui| {
                    let mut effects = effects.borrow_mut();
                    effects.order.push("callback");
                    if remove_owner {
                        effects.removed_owner = Some(child);
                        effects.owners.remove(&child);
                        effects.actions.remove(&child);
                    } else {
                        effects.surface_missing = true;
                    }
                    let texture = ui.ctx().load_texture(
                        "missing-surface-resource",
                        egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
                        egui::TextureOptions::LINEAR,
                    );
                    effects.created_textures.push(texture.id());
                    drop(texture);
                    drop(effects);
                    let id = egui::Id::unique("missing-surface-field");
                    ui.memory_mut(|memory| memory.request_focus(id));
                    ui.add(egui::TextEdit::singleline(&mut text).id(id));
                    ui.ctx().copy_text("live-owner-clipboard".to_owned());
                    ui.ctx().send_viewport_cmd(ViewportCommand::RequestCopy);
                    ui.ctx()
                        .send_viewport_cmd(ViewportCommand::Screenshot(current.clone()));
                    ui.ctx().send_viewport_cmd_to(
                        ViewportId::ROOT,
                        ViewportCommand::Title("other-owner-command".to_owned()),
                    );
                }),
            },
        );
        let effects = effects.borrow();
        assert_eq!(
            effects.order,
            vec![
                "prepare",
                "callback",
                "textures",
                "output",
                "resources",
                "viewports",
                "finished"
            ]
        );
        assert_eq!(effects.accepted, 1);
        assert_eq!(effects.applied, 1);
        assert!(effects.pending.is_empty());
        assert!(effects.painted.is_empty());
        let texture = effects.created_textures[0];
        assert!(effects.applied_textures.contains(&texture));
        assert!(effects.applied_frees.contains(&texture));
        assert_eq!(effects.commands.iter().filter(|(id, command)| *id == ViewportId::ROOT && matches!(command, ViewportCommand::Title(title) if title == "other-owner-command")).count(), 1);
        assert!(completed.lock().unwrap().is_empty());
        if remove_owner {
            assert!(!effects.owners.contains_key(&child));
            assert!(!effects.actions.contains_key(&child));
        } else {
            let owner = &effects.owners[&child];
            assert_eq!(owner.input.events, vec![Event::Copy]);
            assert_eq!(owner.platform.len(), 1);
            assert!(owner.platform[0].ime.is_some());
            assert!(matches!(
                effects.actions[&child].as_slice(),
                [
                    ActionRequested::Screenshot(_),
                    ActionRequested::Screenshot(_)
                ]
            ));
        }
    }

    /// Run on a fresh thread so the real thread-local immediate renderer does
    /// not escape into unrelated tests after these registered-entry scenarios.
    pub(crate) fn check_immediate_entry(
        entry: impl for<'a> Fn(&mut FakeImmediateBackend, egui::ImmediateViewport<'a>) + Send + 'static,
    ) {
        std::thread::spawn(move || {
            let entry: Rc<Entry> = Rc::new(entry);
            for scenario in [
                Scenario {
                    child_visible: true,
                    ..Default::default()
                },
                Scenario::default(),
                Scenario {
                    existing_capture: true,
                    ..Default::default()
                },
                Scenario {
                    current_capture: true,
                    ..Default::default()
                },
                Scenario {
                    existing_capture: true,
                    current_capture: true,
                    ..Default::default()
                },
                Scenario {
                    existing_paint: true,
                    ..Default::default()
                },
                Scenario {
                    current_paint: true,
                    ..Default::default()
                },
                Scenario {
                    parent_capture: true,
                    ..Default::default()
                },
            ] {
                run_nested_scenario(Rc::clone(&entry), scenario);
            }
            run_direct_current_output(Rc::clone(&entry));
            run_missing_facts(Rc::clone(&entry), false);
            run_missing_facts(entry, true);
        })
        .join()
        .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, ViewportId};

    #[derive(Default)]
    struct FakeState {
        input: egui::RawInput,
        text: Option<String>,
        image: Option<egui::ColorImage>,
        text_reads: usize,
        image_reads: usize,
        platform_outputs: Vec<egui::PlatformOutput>,
    }

    impl ClipboardInput for FakeState {
        fn clipboard_text(&mut self) -> Option<String> {
            self.text_reads += 1;
            self.text.clone()
        }

        fn clipboard_image(&mut self) -> Option<egui::ColorImage> {
            self.image_reads += 1;
            self.image.clone()
        }

        fn input(&mut self) -> &mut egui::RawInput {
            &mut self.input
        }
    }

    impl MainOutputTarget for FakeState {
        fn handle_platform_output(&mut self, platform: egui::PlatformOutput) {
            // As with the native adapter, platform clipboard writes precede
            // actions that read the clipboard into the next input batch.
            for command in &platform.commands {
                if let egui::OutputCommand::CopyText(text) = command {
                    self.text = Some(text.clone());
                }
            }
            self.platform_outputs.push(platform);
        }
    }

    #[test]
    fn minimized_full_ui_and_surface_decisions_preserve_platform_and_logic_only_contracts() {
        for windows in [false, true] {
            for (minimized, occluded) in
                [(false, false), (true, false), (false, true), (true, true)]
            {
                let info = ViewportInfo {
                    minimized: Some(minimized),
                    occluded: Some(occluded),
                    ..Default::default()
                };
                for has_visible_descendant in [false, true] {
                    for explicit_paint in [false, true] {
                        let decision = PassDecision::for_viewport(
                            PassMode::Full,
                            &info,
                            has_visible_descendant,
                            explicit_paint,
                            windows,
                        );
                        let visible = !minimized && !occluded;
                        assert_eq!(decision.paint, visible || explicit_paint);
                        assert_eq!(
                            decision.run_ui,
                            visible
                                || has_visible_descendant
                                || explicit_paint
                                || (windows && minimized)
                        );
                        assert_eq!(
                            PassDecision::for_viewport(
                                PassMode::LogicOnly,
                                &info,
                                has_visible_descendant,
                                explicit_paint,
                                windows,
                            ),
                            PassDecision {
                                run_ui: false,
                                paint: false
                            }
                        );
                        assert_eq!(info.minimized, Some(minimized));
                        assert_eq!(info.occluded, Some(occluded));
                    }
                }
            }
        }
    }

    #[test]
    fn minimized_full_actions_keep_capture_identity_native_commands_and_owner_input_order() {
        let capture = egui::ScreenshotCallback::new(|_| panic!("capture needs a paint"));
        let mut actions = vec![ActionRequested::Copy];
        let mut commands = vec![
            ViewportCommand::Title("title".to_owned()),
            ViewportCommand::RequestCut,
            ViewportCommand::Screenshot(capture.clone()),
            ViewportCommand::RequestPaste,
            ViewportCommand::Minimized(false),
            ViewportCommand::RequestPaintWhileHidden,
        ];
        take_main_viewport_actions(&mut commands, &mut actions);
        assert!(
            matches!(commands.as_slice(), [ViewportCommand::Title(title), ViewportCommand::Minimized(false)] if title == "title")
        );
        let count = actions.len();
        take_main_viewport_actions(&mut commands, &mut actions);
        assert_eq!(
            actions.len(),
            count,
            "extraction cannot schedule actions twice"
        );

        let child = ViewportId::from_hash_of("clipboard-owner");
        let mut target = FakeState {
            input: egui::RawInput {
                viewport_id: child,
                events: vec![Event::Text("old".to_owned())],
                ..Default::default()
            },
            text: Some("one\r\ntwo".to_owned()),
            ..Default::default()
        };
        let sibling = FakeState::default();
        process_non_paint_actions(&mut actions, &mut target);
        assert_eq!(target.input.viewport_id, child);
        assert_eq!(
            target.input.events,
            vec![
                Event::Text("old".to_owned()),
                Event::Copy,
                Event::Cut,
                Event::Paste("one\ntwo".to_owned())
            ]
        );
        assert!(sibling.input.events.is_empty());
        assert!(
            matches!(actions.as_slice(), [ActionRequested::Screenshot(callback), ActionRequested::PaintWhileHidden] if callback == &capture)
        );
        process_non_paint_actions(&mut actions, &mut target);
        assert_eq!(target.input.events.len(), 4);
        assert_eq!(target.text_reads, 1);
        assert_eq!(target.image_reads, 0);
        assert_eq!(
            actions.len(),
            2,
            "non-paint completion must retain paint actions"
        );
    }

    #[test]
    fn minimized_full_paste_distinguishes_empty_text_from_missing_text_and_image() {
        for text in [Some("".to_owned()), Some("text".to_owned()), None] {
            let image = egui::ColorImage::filled([2, 1], egui::Color32::WHITE);
            let mut target = FakeState {
                text: text.clone(),
                image: Some(image.clone()),
                ..Default::default()
            };
            let mut actions = vec![ActionRequested::Paste];
            process_non_paint_actions(&mut actions, &mut target);
            match text {
                Some(text) if text.is_empty() => assert!(target.input.events.is_empty()),
                Some(text) => assert_eq!(target.input.events, vec![Event::Paste(text)]),
                None => assert_eq!(
                    target.input.events,
                    vec![Event::PasteImage(std::sync::Arc::new(image))]
                ),
            }
            assert!(actions.is_empty());
            assert_eq!(target.text_reads, 1);
            assert_eq!(target.image_reads, usize::from(target.text.is_none()));
        }
        let mut target = FakeState::default();
        let mut actions = vec![ActionRequested::Paste];
        process_non_paint_actions(&mut actions, &mut target);
        assert!(actions.is_empty());
        assert!(target.input.events.is_empty());
    }

    #[test]
    fn minimized_full_output_boundary_delivers_platform_ime_actions_and_retains_paint_requests() {
        let child = ViewportId::from_hash_of("output-owner");
        let mut owner = FakeState {
            input: egui::RawInput {
                viewport_id: child,
                events: vec![Event::Text("old".to_owned())],
                ..Default::default()
            },
            text: Some("before-platform".to_owned()),
            ..Default::default()
        };
        let sibling = FakeState::default();
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(100.0, 30.0));
        let platform = egui::PlatformOutput {
            commands: vec![egui::OutputCommand::CopyText("platform\r\ntext".to_owned())],
            ime: Some(egui::output::IMEOutput {
                purpose: egui::IMEPurpose::Normal,
                rect,
                cursor_rect: rect,
                should_interrupt_composition: true,
            }),
            ..Default::default()
        };
        let capture = egui::ScreenshotCallback::new(|_| panic!("no surface painted yet"));
        let mut commands = vec![
            ViewportCommand::Title("child".to_owned()),
            ViewportCommand::RequestCopy,
            ViewportCommand::Screenshot(capture.clone()),
            ViewportCommand::RequestPaste,
            ViewportCommand::RequestPaintWhileHidden,
            ViewportCommand::Minimized(false),
        ];
        let mut actions = vec![ActionRequested::Cut];
        finish_main_viewport_output(platform.clone(), &mut commands, &mut actions, &mut owner);
        assert!(owner.platform_outputs == vec![platform]);
        assert_eq!(owner.input.viewport_id, child);
        assert_eq!(
            owner.input.events,
            vec![
                Event::Text("old".to_owned()),
                Event::Cut,
                Event::Copy,
                Event::Paste("platform\ntext".to_owned()),
            ]
        );
        assert!(sibling.input.events.is_empty() && sibling.platform_outputs.is_empty());
        assert!(
            matches!(commands.as_slice(), [ViewportCommand::Title(title), ViewportCommand::Minimized(false)] if title == "child")
        );
        assert!(
            matches!(actions.as_slice(), [ActionRequested::Screenshot(callback), ActionRequested::PaintWhileHidden] if callback == &capture)
        );
        assert_eq!(owner.text_reads, 1);

        finish_main_viewport_output(Default::default(), &mut commands, &mut actions, &mut owner);
        assert_eq!(
            owner.input.events.len(),
            4,
            "a later pass cannot replay clipboard actions"
        );
        assert_eq!(owner.text_reads, 1);
        assert_eq!(owner.platform_outputs.len(), 2);
        assert_eq!(
            actions.len(),
            2,
            "capture and explicit paint remain pending for paint"
        );
        assert_eq!(
            commands.len(),
            2,
            "native commands remain for their ordinary handler"
        );
    }
}
