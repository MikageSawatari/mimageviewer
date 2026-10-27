//! Integration-owned retirement through a real Context, without native windows.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use crate::{
    Color32, ColorImage, Context, Event, FontId, FullOutput, Id, Key, Modifiers, RawInput,
    TextEdit, TextureOptions, ViewportBuilder, ViewportCommand, ViewportId, ViewportIdSet,
    ViewportInfo, pos2,
};

fn target() -> ViewportId {
    ViewportId::from_hash_of("retirement-target")
}
fn sibling() -> ViewportId {
    ViewportId::from_hash_of("retirement-sibling")
}
fn descendant() -> ViewportId {
    ViewportId::from_hash_of("retirement-descendant")
}
fn field_id(id: ViewportId) -> Id {
    Id::unique(("retirement-field", id))
}

fn input(id: ViewportId, events: Vec<Event>) -> RawInput {
    let mut input = RawInput {
        viewport_id: id,
        events,
        ..Default::default()
    };
    // Deliberately retain stale window metadata after retirement: metadata is
    // not a new registration and must not resurrect the retired viewport.
    for (viewport, parent) in [
        (target(), ViewportId::ROOT),
        (sibling(), ViewportId::ROOT),
        (descendant(), target()),
    ] {
        input.viewports.insert(
            viewport,
            ViewportInfo {
                parent: Some(parent),
                ..Default::default()
            },
        );
    }
    input
}

fn output_ids(output: FullOutput) -> ViewportIdSet {
    let ids = output.viewport_output.keys().copied().collect();
    output.drop_without_applying_deltas();
    ids
}

fn declare(
    ctx: &Context,
    id: ViewportId,
    text: Arc<Mutex<String>>,
    calls: Arc<AtomicUsize>,
    has_child: bool,
) {
    ctx.show_viewport_deferred(id, ViewportBuilder::default(), move |ui, _| {
        calls.fetch_add(1, Ordering::SeqCst);
        let mut text = text.lock().unwrap();
        let response = ui.add(TextEdit::singleline(&mut *text).id(field_id(id)));
        if ui.memory(|memory| memory.focused()).is_none() {
            response.request_focus();
        }
        ui.fonts_mut(|fonts| {
            fonts.layout_no_wrap(
                "viewport-cache-marker".into(),
                FontId::default(),
                Color32::WHITE,
            )
        });
        if has_child {
            declare(
                ui.ctx(),
                descendant(),
                Arc::new(Mutex::new("descendant".into())),
                Arc::new(AtomicUsize::new(0)),
                false,
            );
        }
    });
}

fn draw(ctx: &Context, id: ViewportId, events: Vec<Event>) -> (ViewportIdSet, usize) {
    let callback = ctx.read(|inner| inner.viewports[&id].viewport_ui_cb.clone().unwrap());
    let mut cached = 0;
    let output = ctx.run_ui(input(id, events), |ui| {
        callback(ui);
        cached = ui.fonts_mut(|fonts| fonts.num_galleys_in_cache());
    });
    (output_ids(output), cached)
}

fn setup() -> (Context, Arc<Mutex<String>>, Arc<AtomicUsize>) {
    let ctx = Context::default();
    ctx.set_embed_viewports(false);
    let text = Arc::new(Mutex::new("sibling".into()));
    let calls = Arc::new(AtomicUsize::new(0));
    output_ids(ctx.run_ui(input(ViewportId::ROOT, vec![]), |ui| {
        ui.label("root cache");
        declare(
            ui.ctx(),
            target(),
            Arc::new(Mutex::new("target".into())),
            Arc::clone(&calls),
            true,
        );
        declare(
            ui.ctx(),
            sibling(),
            Arc::clone(&text),
            Arc::clone(&calls),
            false,
        );
    }));
    // Prime each actual TextEdit before delivering composition input.
    draw(&ctx, target(), vec![]);
    draw(&ctx, descendant(), vec![]);
    draw(&ctx, sibling(), vec![]);
    (ctx, text, calls)
}

fn assert_absent(ctx: &Context, id: ViewportId) {
    ctx.read(|inner| {
        assert!(!inner.viewports.contains_key(&id));
        assert!(!inner.viewport_parents.contains_key(&id));
        assert!(!inner.viewport_parents.values().any(|parent| *parent == id));
        assert!(!inner.memory.focus.contains_key(&id));
        assert!(!inner.memory.interactions.contains_key(&id));
    });
}

fn sibling_state(ctx: &Context) -> (String, String, String, String, u64) {
    ctx.read(|inner| {
        let viewport = &inner.viewports[&sibling()];
        (
            format!("{:?}", viewport.input),
            format!("{:?}", inner.memory.focus.get(&sibling())),
            format!("{:?}", inner.memory.interactions.get(&sibling())),
            format!("{:?}", viewport.commands),
            viewport.repaint.cumulative_pass_nr,
        )
    })
}

#[test]
fn viewport_retirement_removes_approved_subtree_and_preserves_sibling_state() {
    let (ctx, text, calls) = setup();
    let (_, target_cache) = draw(&ctx, target(), vec![]);
    let (_, descendant_cache) = draw(&ctx, descendant(), vec![]);
    draw(
        &ctx,
        sibling(),
        vec![
            Event::ModifiersChanged(Modifiers::CTRL),
            Event::Key {
                key: Key::F5,
                physical_key: Some(Key::F5),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::CTRL,
            },
            Event::PointerMoved(pos2(35.0, 20.0)),
            Event::Ime(crate::ImeEvent::Preedit {
                text: "仮".into(),
                active_range_chars: Some(0..1),
            }),
        ],
    );
    assert_eq!(*text.lock().unwrap(), "sibling仮");
    let before = sibling_state(&ctx);
    let text_state = TextEdit::load_state(&ctx, field_id(sibling())).unwrap();
    assert!(text_state.cursor_purpose.is_ime_composition());
    let total_cache = ctx.read(|inner| inner.fonts.as_ref().unwrap().num_galleys_in_cache());
    let calls_before = calls.load(Ordering::SeqCst);
    let repaints = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&repaints);
    ctx.set_request_repaint_callback(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
    });
    let removed: ViewportIdSet = ctx.retire_viewport(target()).into_iter().collect();
    assert_eq!(removed, [target(), descendant()].into_iter().collect());
    assert_absent(&ctx, target());
    assert_absent(&ctx, descendant());
    assert_eq!(sibling_state(&ctx), before);
    let retained_text_state = TextEdit::load_state(&ctx, field_id(sibling())).unwrap();
    assert_eq!(
        retained_text_state.cursor.char_range(),
        text_state.cursor.char_range()
    );
    assert!(retained_text_state.cursor_purpose.is_ime_composition());
    assert!(Arc::ptr_eq(&retained_text_state.undoer, &text_state.undoer));
    assert_eq!(*text.lock().unwrap(), "sibling仮");
    assert_eq!(calls.load(Ordering::SeqCst), calls_before);
    assert_eq!(repaints.load(Ordering::SeqCst), 0);
    assert_eq!(
        ctx.read(|inner| inner.fonts.as_ref().unwrap().num_galleys_in_cache()),
        total_cache - target_cache - descendant_cache
    );
    // Unknown and repeated requests neither mutate nor reset the sibling.
    assert!(ctx.retire_viewport(target()).is_empty());
    assert!(
        ctx.retire_viewport(ViewportId::from_hash_of("unknown"))
            .is_empty()
    );
    assert_eq!(sibling_state(&ctx), before);
}

#[test]
fn viewport_retirement_does_not_reappear_in_later_outputs_and_allows_new_open() {
    let (ctx, text, calls) = setup();
    let _ = ctx.retire_viewport(target());
    for _ in 0..2 {
        let root = output_ids(ctx.run_ui(input(ViewportId::ROOT, vec![]), |ui| {
            declare(
                ui.ctx(),
                sibling(),
                Arc::clone(&text),
                Arc::clone(&calls),
                false,
            );
        }));
        let (child, _) = draw(&ctx, sibling(), vec![]);
        for ids in [root, child] {
            assert!(ids.contains(&ViewportId::ROOT) && ids.contains(&sibling()));
            assert!(!ids.contains(&target()) && !ids.contains(&descendant()));
        }
        assert_absent(&ctx, target());
    }
    // A later explicit open can reuse the ID. There is no tombstone or latch.
    let reopened = output_ids(ctx.run_ui(input(ViewportId::ROOT, vec![]), |ui| {
        declare(
            ui.ctx(),
            target(),
            Arc::new(Mutex::new("new target".into())),
            Arc::clone(&calls),
            false,
        );
        declare(
            ui.ctx(),
            sibling(),
            Arc::clone(&text),
            Arc::clone(&calls),
            false,
        );
        ui.ctx().read(|inner| {
            assert!(inner.viewports[&target()].input.keys_down.is_empty());
            assert!(!inner.memory.focus.contains_key(&target()));
        });
    }));
    assert!(reopened.contains(&target()));
    assert!(!reopened.contains(&descendant()));
}

#[test]
fn viewport_retirement_hide_and_minimize_are_not_close() {
    let (ctx, text, calls) = setup();
    for minimize in [false, true] {
        let mut raw = input(ViewportId::ROOT, vec![]);
        raw.viewports.get_mut(&target()).unwrap().minimized = Some(minimize);
        raw.viewports.get_mut(&target()).unwrap().occluded = Some(true);
        let _ = ctx.run_logic(&raw, |_| {});
        let ids = output_ids(ctx.run_ui(raw, |ui| {
            declare(
                ui.ctx(),
                target(),
                Arc::new(Mutex::new("target".into())),
                Arc::clone(&calls),
                true,
            );
            declare(
                ui.ctx(),
                sibling(),
                Arc::clone(&text),
                Arc::clone(&calls),
                false,
            );
            ui.ctx()
                .send_viewport_cmd_to(target(), ViewportCommand::Visible(false));
            ui.ctx()
                .send_viewport_cmd_to(target(), ViewportCommand::Minimized(minimize));
        }));
        assert!(ids.contains(&target()) && ids.contains(&descendant()));
        assert!(ctx.read(|inner| inner.memory.focus.contains_key(&target())));
    }
}

#[test]
fn viewport_retirement_normal_gc_reuses_cleanup_for_subtree() {
    let (ctx, text, calls) = setup();
    let before = sibling_state(&ctx);
    // The normal parent pass stops declaring only the approved subtree.
    let ids = output_ids(ctx.run_ui(input(ViewportId::ROOT, vec![]), |ui| {
        declare(
            ui.ctx(),
            sibling(),
            Arc::clone(&text),
            Arc::clone(&calls),
            false,
        );
    }));
    assert!(!ids.contains(&target()) && !ids.contains(&descendant()));
    assert_absent(&ctx, target());
    assert_absent(&ctx, descendant());
    assert_eq!(sibling_state(&ctx), before);
}

#[test]
fn viewport_retirement_resets_only_a_retired_memory_owner() {
    for last in [target(), sibling()] {
        let (ctx, _, _) = setup();
        draw(&ctx, last, vec![]);
        ctx.read(|inner| {
            assert_eq!(inner.memory.viewport_id, last);
            assert_eq!(inner.last_viewport, last);
        });
        let _ = ctx.retire_viewport(target());
        let expected = if last == target() {
            ViewportId::ROOT
        } else {
            sibling()
        };
        ctx.read(|inner| {
            assert_eq!(inner.memory.viewport_id, expected);
            assert_eq!(inner.last_viewport, expected);
            assert!(inner.viewports.contains_key(&ViewportId::ROOT));
        });
    }
}

struct DropProbe {
    context: Weak<epaint::mutex::RwLock<super::ContextImpl>>,
    drops: Arc<AtomicUsize>,
    unlocked: Arc<AtomicBool>,
}

impl Drop for DropProbe {
    fn drop(&mut self) {
        // The ordinary Context read exercises the real lock. Catch egui's
        // existing deadlock diagnostic if a future change drops us locked.
        let unlocked = self.context.upgrade().is_none_or(|context| {
            let context = super::Context(context);
            catch_unwind(AssertUnwindSafe(|| context.viewport_id())).is_ok()
        });
        self.unlocked.store(unlocked, Ordering::SeqCst);
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn viewport_retirement_drops_callback_owners_outside_context_lock() {
    for normal_gc in [false, true] {
        let ctx = Context::default();
        ctx.set_embed_viewports(false);
        let drops = Arc::new(AtomicUsize::new(0));
        let unlocked = Arc::new(AtomicBool::new(false));
        let mut probe = Some(DropProbe {
            context: Arc::downgrade(&ctx.0),
            drops: Arc::clone(&drops),
            unlocked: Arc::clone(&unlocked),
        });
        output_ids(ctx.run_ui(input(ViewportId::ROOT, vec![]), |ui| {
            let owner = probe.take().unwrap();
            ui.ctx()
                .show_viewport_deferred(target(), ViewportBuilder::default(), move |_, _| {
                    let _ = &owner;
                });
        }));
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        if normal_gc {
            output_ids(ctx.run_ui(input(ViewportId::ROOT, vec![]), |_| {}));
        } else {
            assert_eq!(ctx.retire_viewport(target()), vec![target()]);
        }
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(unlocked.load(Ordering::SeqCst));
    }
}

#[test]
fn viewport_retirement_preserves_shared_pending_texture_deltas() {
    let (ctx, _, _) = setup();
    let closed = ctx.load_texture(
        "retired owner",
        ColorImage::new([1, 1], vec![Color32::RED]),
        TextureOptions::default(),
    );
    let sibling_texture = ctx.load_texture(
        "surviving owner",
        ColorImage::new([1, 1], vec![Color32::GREEN]),
        TextureOptions::default(),
    );
    let closed_id = closed.id();
    let surviving_id = sibling_texture.id();
    // A callback can own a TextureHandle. Its release must remain in the
    // shared manager, alongside all undelivered uploads.
    ctx.write(|inner| {
        inner.viewports.get_mut(&target()).unwrap().viewport_ui_cb = Some(Arc::new(move |_| {
            let _ = closed.id();
        }))
    });
    let _ = ctx.retire_viewport(target());
    let output = ctx.run_ui(input(sibling(), vec![]), |_| {});
    assert!(
        output
            .textures_delta
            .set
            .iter()
            .any(|(id, _)| *id == closed_id)
    );
    assert!(
        output
            .textures_delta
            .set
            .iter()
            .any(|(id, _)| *id == surviving_id)
    );
    assert!(output.textures_delta.free.contains(&closed_id));
    assert!(!output.textures_delta.free.contains(&surviving_id));
    output.drop_without_applying_deltas();
    drop(sibling_texture);
    output_ids(ctx.run_ui(input(ViewportId::ROOT, vec![]), |_| {}));
}

#[test]
fn viewport_retirement_rejects_root_and_active_pass() {
    let (ctx, _, _) = setup();
    let registered = ctx.read(|inner| inner.all_viewport_ids());
    assert!(catch_unwind(AssertUnwindSafe(|| ctx.retire_viewport(ViewportId::ROOT))).is_err());
    assert_eq!(ctx.read(|inner| inner.all_viewport_ids()), registered);
    ctx.begin_pass(input(ViewportId::ROOT, vec![]));
    assert!(catch_unwind(AssertUnwindSafe(|| ctx.retire_viewport(target()))).is_err());
    assert_eq!(ctx.read(|inner| inner.all_viewport_ids()), registered);
    output_ids(ctx.end_pass());
}
