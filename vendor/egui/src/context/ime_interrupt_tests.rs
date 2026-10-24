//! Real nested viewport and discard passes; no window or backend is created.

use std::{cell::RefCell, rc::Rc};

use crate::output::IMEOutput;
use crate::{
    FullOutput, IMEPurpose, Id, PlatformOutput, RawInput, Rect, TextEdit, Ui, ViewportBuilder,
    ViewportClass, ViewportId, ViewportInfo, pos2, vec2,
};

use super::{Context, IMMEDIATE_VIEWPORT_RENDERER, ImmediateViewportRendererCallback};

#[derive(Clone, Copy, Debug)]
struct OutputSnapshot {
    ime: Option<IMEOutput>,
    passes: usize,
}

fn snapshot(output: FullOutput) -> OutputSnapshot {
    let result = OutputSnapshot {
        ime: output.platform_output.ime,
        passes: output.platform_output.num_completed_passes,
    };
    // Headless consumer: inspect each viewport's output separately, then
    // explicitly discard its texture deltas before any assertion can unwind.
    output.drop_without_applying_deltas();
    result
}

fn input(viewport: ViewportId, child: ViewportId) -> RawInput {
    let mut input = RawInput {
        viewport_id: viewport,
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 480.0))),
        ..Default::default()
    };
    input.viewports.insert(
        child,
        ViewportInfo {
            parent: Some(ViewportId::ROOT),
            ..Default::default()
        },
    );
    input
}

fn field(ui: &mut Ui, id: Id, position: crate::Pos2, password: bool, prime_focus: bool) {
    let mut text = "sample text".to_owned();
    let response = ui.put(
        Rect::from_min_size(position, vec2(180.0, 30.0)),
        TextEdit::singleline(&mut text).id(id).password(password),
    );
    if prime_focus {
        response.request_focus();
    }
}

#[derive(Clone, Copy, Debug)]
enum Request {
    None,
    Interrupt,
    Focus,
    NextFrameFocus,
}

impl Request {
    fn apply(self, ctx: &Context, id: Id) {
        ctx.memory_mut(|memory| match self {
            Self::None => {}
            Self::Interrupt => memory.interrupt_ime(),
            Self::Focus => memory.request_focus(id),
            Self::NextFrameFocus => memory.request_focus_next_frame(id),
        });
    }
}

// The renderer is thread-local, just as in an actual immediate backend. Restore
// any previous callback even when a regression assertion fails.
struct RestoreRenderer(Option<Box<ImmediateViewportRendererCallback>>);

impl Drop for RestoreRenderer {
    fn drop(&mut self) {
        IMMEDIATE_VIEWPORT_RENDERER.with(|renderer| renderer.replace(self.0.take()));
    }
}

struct NestedViewports {
    ctx: Context,
    child: ViewportId,
    child_outputs: Rc<RefCell<Vec<OutputSnapshot>>>,
    _restore_renderer: RestoreRenderer,
}

impl NestedViewports {
    fn new() -> Self {
        let ctx = Context::default();
        ctx.set_embed_viewports(false);
        let child = ViewportId::from_hash_of("ime-interrupt-child");
        let child_outputs = Rc::new(RefCell::new(Vec::new()));
        let outputs = Rc::clone(&child_outputs);
        let restore_renderer =
            RestoreRenderer(IMMEDIATE_VIEWPORT_RENDERER.with(|renderer| renderer.take()));
        Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
            let output = ctx.run_ui(input(viewport.ids.this, child), |ui| {
                (viewport.viewport_ui_cb)(ui);
            });
            outputs.borrow_mut().push(snapshot(output));
        });
        let harness = Self {
            ctx,
            child,
            child_outputs,
            _restore_renderer: restore_renderer,
        };
        // Establish focus before the measured passes. Initial request_focus
        // calls must not masquerade as a test's explicit interruption request.
        harness.run(Request::None, Request::None, true);
        let (parent, child) = ime_outputs(harness.run(Request::None, Request::None, false));
        assert!(!parent.should_interrupt_composition);
        assert!(!child.should_interrupt_composition);
        harness
    }

    fn run(
        &self,
        parent_request: Request,
        child_request: Request,
        prime: bool,
    ) -> (OutputSnapshot, OutputSnapshot) {
        let parent_id = Id::unique("ime-interrupt-parent-field");
        let child_id = Id::unique("ime-interrupt-child-field");
        let output = self.ctx.run_ui(input(ViewportId::ROOT, self.child), |ui| {
            field(ui, parent_id, pos2(10.0, 10.0), false, prime);
            // The parent's request must survive the child's begin_pass.
            parent_request.apply(ui.ctx(), parent_id);
            self.ctx.show_viewport_immediate(
                self.child,
                ViewportBuilder::default(),
                |ui, class| {
                    assert!(class == ViewportClass::Immediate);
                    assert_eq!(ui.ctx().viewport_id(), self.child);
                    field(ui, child_id, pos2(25.0, 35.0), true, prime);
                    child_request.apply(ui.ctx(), child_id);
                },
            );
            assert_eq!(self.ctx.viewport_id(), ViewportId::ROOT);
        });
        let parent = snapshot(output);
        let mut outputs = self.child_outputs.borrow_mut();
        assert_eq!(
            outputs.len(),
            1,
            "one real nested child run_ui per parent pass"
        );
        let child = outputs.pop().unwrap();
        assert_eq!(parent.passes, 1);
        assert_eq!(child.passes, 1);
        (parent, child)
    }
}

fn ime_outputs((parent, child): (OutputSnapshot, OutputSnapshot)) -> (IMEOutput, IMEOutput) {
    let parent = parent.ime.expect("parent TextEdit must produce IMEOutput");
    let child = child.ime.expect("child TextEdit must produce IMEOutput");
    assert_eq!(parent.purpose, IMEPurpose::Normal);
    assert_eq!(child.purpose, IMEPurpose::Password);
    (parent, child)
}

fn assert_nested_requests(parent_requests: bool, child_requests: bool) {
    for request in [Request::Interrupt, Request::Focus, Request::NextFrameFocus] {
        let harness = NestedViewports::new();
        let (parent, child) = ime_outputs(harness.run(
            if parent_requests {
                request
            } else {
                Request::None
            },
            if child_requests {
                request
            } else {
                Request::None
            },
            false,
        ));
        assert_eq!(
            parent.should_interrupt_composition, parent_requests,
            "parent: {request:?}"
        );
        assert_eq!(
            child.should_interrupt_composition, child_requests,
            "child: {request:?}"
        );
        let (parent, child) = ime_outputs(harness.run(Request::None, Request::None, false));
        assert!(
            !parent.should_interrupt_composition,
            "parent request leaked into a new run"
        );
        assert!(
            !child.should_interrupt_composition,
            "child request leaked into a new run"
        );
    }
}

#[test]
fn ime_interrupt_nested_parent_only() {
    assert_nested_requests(true, false);
}

#[test]
fn ime_interrupt_nested_child_only() {
    assert_nested_requests(false, true);
}

#[test]
fn ime_interrupt_nested_both() {
    assert_nested_requests(true, true);
}

#[test]
fn ime_interrupt_nested_neither() {
    assert_nested_requests(false, false);
}

#[test]
fn ime_interrupt_discard_keeps_request_and_latest_text_edit_metadata() {
    let ctx = Context::default();
    let id = Id::unique("ime-interrupt-discard-field");
    let child = ViewportId::from_hash_of("unused-child");
    snapshot(ctx.run_ui(input(ViewportId::ROOT, child), |ui| {
        field(ui, id, pos2(10.0, 10.0), false, true);
    }));
    let mut per_pass = Vec::new();
    let output = ctx.run_ui(input(ViewportId::ROOT, child), |ui| {
        let pass = per_pass.len();
        field(
            ui,
            id,
            pos2(20.0 + 100.0 * pass as f32, 40.0),
            pass != 0,
            false,
        );
        per_pass.push(ui.output(|output| output.ime));
        if pass == 0 {
            ui.memory_mut(|memory| memory.interrupt_ime());
            ui.ctx()
                .request_discard("ime_interrupt first pass requests composition interruption");
        }
    });
    let output = snapshot(output);
    assert_eq!(output.passes, 2);
    assert_eq!(per_pass.len(), 2);
    let first = per_pass[0].expect("first-pass TextEdit IMEOutput");
    let mut latest = per_pass[1].expect("second-pass TextEdit IMEOutput");
    assert_ne!(first.rect, latest.rect);
    assert_ne!(first.cursor_rect, latest.cursor_rect);
    assert_eq!(first.purpose, IMEPurpose::Normal);
    assert_eq!(latest.purpose, IMEPurpose::Password);
    assert!(!latest.should_interrupt_composition);
    latest.should_interrupt_composition = true;
    assert_eq!(
        output.ime,
        Some(latest),
        "new metadata must retain the earlier undelivered request"
    );
    let next = snapshot(ctx.run_ui(input(ViewportId::ROOT, child), |ui| {
        field(ui, id, pos2(120.0, 40.0), true, false);
    }));
    assert_eq!(next.passes, 1);
    latest.should_interrupt_composition = false;
    assert_eq!(
        next.ime,
        Some(latest),
        "a transferred request must not survive into a new run"
    );
}

#[test]
fn ime_interrupt_platform_append_preserves_option_and_request_contract() {
    let old = IMEOutput {
        purpose: IMEPurpose::Normal,
        rect: Rect::from_min_size(pos2(1.0, 2.0), vec2(30.0, 40.0)),
        cursor_rect: Rect::from_min_size(pos2(3.0, 4.0), vec2(1.0, 5.0)),
        should_interrupt_composition: false,
    };
    let new = IMEOutput {
        purpose: IMEPurpose::Password,
        rect: old.rect.translate(vec2(100.0, 200.0)),
        cursor_rect: old.cursor_rect.translate(vec2(100.0, 200.0)),
        ..old
    };
    for old_request in [false, true] {
        for new_request in [false, true] {
            for old_present in [false, true] {
                for new_present in [false, true] {
                    let old = old_present.then_some(IMEOutput {
                        should_interrupt_composition: old_request,
                        ..old
                    });
                    let new = new_present.then_some(IMEOutput {
                        should_interrupt_composition: new_request,
                        ..new
                    });
                    let mut accumulated = PlatformOutput {
                        ime: old,
                        ..Default::default()
                    };
                    accumulated.append(PlatformOutput {
                        ime: new,
                        ..Default::default()
                    });
                    let mut expected = new.or(old);
                    if let Some(ime) = &mut expected {
                        ime.should_interrupt_composition = old
                            .is_some_and(|i| i.should_interrupt_composition)
                            || new.is_some_and(|i| i.should_interrupt_composition);
                    }
                    assert_eq!(accumulated.ime, expected);
                    let transferred = accumulated.take();
                    assert_eq!(transferred.ime, expected);
                    assert!(accumulated.ime.is_none());
                }
            }
        }
    }
}
