use super::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Mutex,
};

fn retry(ctx: &Context) -> FullOutput {
    let mut pass = 0;
    ctx.run_ui(Default::default(), |ui| {
        if pass == 0 {
            ui.ctx().request_discard("delivery regression");
        }
        pass += 1;
    })
}

fn sink(ctx: &Context, count: &Rc<Cell<usize>>) -> impl Drop + use<> {
    let count = count.clone();
    ctx.enter_discard_texture_delivery(move |mut delta| {
        assert!(!delta.is_empty());
        count.set(count.get() + 1);
        delta.clear(); // Headless scope test has no renderer.
    })
}

#[test]
fn discard_texture_delivery_context_scope_restore_early_return_and_unwind() {
    let a = Context::default();
    let b = Context::default();
    let ca = Rc::new(Cell::new(0));
    let cb = Rc::new(Cell::new(0));
    {
        let _a = sink(&a, &ca);
        assert!(a.has_discard_texture_delivery());
        assert!(!b.has_discard_texture_delivery());
        let output = retry(&b);
        assert!(!output.textures_delta.is_empty(), "B keeps ownership");
        output.drop_without_applying_deltas();
        retry(&a).drop_without_applying_deltas();
        assert_eq!(ca.get(), 1);
        let nested = || {
            let _b = sink(&b, &cb);
            assert!(!a.has_discard_texture_delivery());
            assert!(b.has_discard_texture_delivery());
            let texture = b.load_texture(
                "B",
                crate::ColorImage::filled([1, 1], Color32::WHITE),
                Default::default(),
            );
            retry(&b).drop_without_applying_deltas();
            drop(texture);
            // Early return releases B and restores A.
            if b.has_discard_texture_delivery() {
                return;
            }
            panic!("nested scope must be active");
        };
        nested();
        assert_eq!(cb.get(), 1);
        assert!(a.has_discard_texture_delivery());
        assert!(!b.has_discard_texture_delivery());
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _b = sink(&b, &cb);
            panic!("scope unwind probe");
        }));
        assert!(unwind.is_err());
        assert!(a.has_discard_texture_delivery());
        assert!(!b.has_discard_texture_delivery());
    }
    assert!(!a.has_discard_texture_delivery());
    assert!(!b.has_discard_texture_delivery());
    let t = a.load_texture(
        "no-sink",
        crate::ColorImage::filled([1, 1], Color32::WHITE),
        Default::default(),
    );
    let output = retry(&a);
    assert!(output.textures_delta.set.contains_key(&t.id()));
    output.drop_without_applying_deltas();
    assert_eq!(ca.get(), 1);
}

#[test]
fn discard_texture_delivery_final_limit_direct_and_empty_do_not_call_sink() {
    for direct in [false, true] {
        let ctx = Context::default();
        let calls = Rc::new(Cell::new(0));
        let _scope = sink(&ctx, &calls);
        let mut t = ctx.load_texture(
            "normal",
            crate::ColorImage::filled([2, 2], Color32::BLACK),
            Default::default(),
        );
        let output = if direct {
            ctx.begin_pass(Default::default());
            ctx.request_discard("direct must remain direct");
            ctx.end_pass()
        } else {
            ctx.options_mut(|o| o.max_passes = 1.try_into().unwrap());
            ctx.run_ui(Default::default(), |ui| {
                ui.ctx().request_discard("limit");
            })
        };
        assert!(output.textures_delta.set.contains_key(&t.id()));
        output.drop_without_applying_deltas();
        assert_eq!(calls.get(), 0);
        t.set(
            crate::ColorImage::filled([2, 2], Color32::WHITE),
            Default::default(),
        );
        let output = ctx.run_ui(Default::default(), |_| {});
        assert!(output.textures_delta.set.contains_key(&t.id()));
        output.drop_without_applying_deltas();
        assert_eq!(calls.get(), 0);
        ctx.options_mut(|o| o.max_passes = 2.try_into().unwrap());
        retry(&ctx).drop_without_applying_deltas();
        assert_eq!(calls.get(), 0, "empty retry makes no delivery");
    }
}

#[derive(Default)]
struct ObservationPlugin {
    passes: Arc<Mutex<Vec<(usize, bool, usize)>>>,
    accepted: Arc<Mutex<usize>>,
}
impl crate::Plugin for ObservationPlugin {
    fn debug_name(&self) -> &'static str {
        "discard delivery observation"
    }
    fn output_hook(&mut self, _: &Context, output: &mut FullOutput) {
        let pass = output.platform_output.num_completed_passes;
        let discarded = output.platform_output.requested_discard();
        self.passes.lock().unwrap().push((
            pass,
            discarded,
            output
                .textures_delta
                .set
                .values()
                .map(|updates| updates.len())
                .sum(),
        ));
        // Like page-turn submission receipts, discarded passes are never accepted.
        if !discarded {
            *self.accepted.lock().unwrap() += 1;
        }
        output.textures_delta.push(
            TextureId::User(100 + pass as u64),
            epaint::ImageDelta::full(
                crate::ColorImage::filled([1, 1], Color32::WHITE),
                TextureOptions::NEAREST,
            ),
        );
    }
}

#[test]
fn discard_texture_delivery_plugins_ime_receipt_and_multiple_retries_once() {
    let ctx = Context::default();
    ctx.options_mut(|o| o.max_passes = 3.try_into().unwrap());
    let plugin = ObservationPlugin::default();
    let passes = plugin.passes.clone();
    let accepted = plugin.accepted.clone();
    ctx.add_plugin(plugin);
    let delivered = Rc::new(RefCell::new(Vec::new()));
    let got = delivered.clone();
    let ctx_in_sink = ctx.clone();
    let _scope = ctx.enter_discard_texture_delivery(move |mut delta| {
        assert!(
            ctx_in_sink.has_discard_texture_delivery(),
            "TLS registration borrow released"
        );
        ctx_in_sink.options(|_| ()); // Context lock also released.
        got.borrow_mut()
            .push(delta.set.keys().copied().collect::<Vec<_>>());
        delta.clear();
    });
    let mut pass = 0;
    let output = ctx.run_ui(Default::default(), |ui| {
        ui.output_mut(|o| {
            o.ime = Some(crate::output::IMEOutput {
                purpose: crate::IMEPurpose::Normal,
                rect: Rect::from_min_size(Pos2::ZERO, vec2(pass as f32 + 1.0, 1.0)),
                cursor_rect: Rect::NOTHING,
                should_interrupt_composition: pass == 0,
            })
        });
        if pass < 2 {
            ui.ctx().request_discard("plugin retry");
        }
        pass += 1;
    });
    assert_eq!(
        passes.lock().unwrap().len(),
        3,
        "one output hook per actual pass"
    );
    assert_eq!(*accepted.lock().unwrap(), 1, "only final receipt accepted");
    assert_eq!(delivered.borrow().len(), 2);
    assert!(delivered.borrow()[0].contains(&TextureId::User(101)));
    assert!(delivered.borrow()[1].contains(&TextureId::User(102)));
    assert!(
        !delivered.borrow()[1].contains(&TextureId::User(101)),
        "plugins never reobserve delivered delta"
    );
    assert!(
        output
            .textures_delta
            .set
            .contains_key(&TextureId::User(103))
    );
    assert!(
        !output
            .textures_delta
            .set
            .contains_key(&TextureId::User(101))
    );
    let ime = output.platform_output.ime.unwrap();
    assert!(ime.should_interrupt_composition);
    assert_eq!(ime.rect.width(), 3.0);
    output.drop_without_applying_deltas();
}

#[test]
fn discard_texture_delivery_nested_immediate_discard_uses_same_scope() {
    let ctx = Context::default();
    ctx.set_embed_viewports(false);
    let calls = Rc::new(Cell::new(0));
    let _scope = sink(&ctx, &calls);
    Context::set_immediate_viewport_renderer(|ctx, mut immediate| {
        assert!(ctx.has_discard_texture_delivery());
        let mut input = crate::RawInput {
            viewport_id: immediate.ids.this,
            ..Default::default()
        };
        input
            .viewports
            .entry(immediate.ids.this)
            .or_default()
            .parent = Some(immediate.ids.parent);
        ctx.run_ui(input, |ui| (immediate.viewport_ui_cb)(ui))
            .drop_without_applying_deltas();
    });
    let mut textures = Vec::new();
    let mut child_pass = 0;
    let output = ctx.run_ui(Default::default(), |ui| {
        ui.ctx().show_viewport_immediate(
            crate::ViewportId::from_hash_of("nested-discard"),
            Default::default(),
            |ui, _| {
                if child_pass == 0 {
                    textures.push(ui.ctx().load_texture(
                        "nested",
                        crate::ColorImage::filled([1, 1], Color32::WHITE),
                        Default::default(),
                    ));
                    ui.ctx().request_discard("child retry");
                }
                child_pass += 1;
            },
        );
    });
    assert_eq!(child_pass, 2);
    assert_eq!(calls.get(), 1);
    assert!(output.textures_delta.is_empty());
    output.drop_without_applying_deltas();
}
