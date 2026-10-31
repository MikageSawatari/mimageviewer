//! Stage-B font prefetch regressions. No system font or product process is used.
use super::tests::annotation;
#[cfg(windows)]
use super::tests::seed_context;
use super::*;
use crate::app::comic_fonts::{FontInvalidation, FontLoadResult};
use crate::app::final_prepare::tests::{app, begin_registration_frame, wait_ready};
use crate::ui_fullscreen::page_turn_submission_tests::{
    draw_current_without_sources, frame, install_presentation, offer_texture_for_test,
};

fn fake_fonts(keys: &HashSet<String>) -> FontLoadResult {
    // Filled bubbles run the real compositor without a font dependency.
    FontLoadResult::for_test(keys, Some(comic_core::FontSet::new()), HashSet::new())
}

fn cold_fonts(app: &mut App) {
    app.invalidate_comic_fonts(FontInvalidation::SourcesChanged);
    app.comic_fonts.loader_for_test(Arc::new(fake_fonts));
    app.comic_font_registry_loaded = true; // never enumerate real fonts in a test
}

fn present(app: &mut App, ctx: &egui::Context) {
    frame(ctx, |ui, ctx| draw_current_without_sources(app, ui, ctx));
    assert!(app.page_turn_current_unit_presented());
}

#[test]
fn comic_font_prefetch_plan_adoption_annotation_only_and_page_turn_material() {
    let ctx = egui::Context::default();
    install_presentation(&ctx);
    let mut app = app(&ctx);
    app.ensure_final_composite_texture(&ctx, 1).unwrap();
    let base = app
        .exact_complete_final_composite_entry(1)
        .unwrap()
        .1
        .pixels
        .clone();
    annotation(&mut app, 1);
    cold_fonts(&mut app);
    present(&mut app, &ctx);
    assert!(app.prepared_edit_input(1).is_none());
    app.prefetch_next_final_unit(&ctx, 0);
    assert!(
        app.comic_fonts.pending_for_test(),
        "planning requests fonts before B can bake"
    );
    assert!(app.final_effect_pending.is_empty());
    app.comic_fonts.wait_for_test();
    // Exercise the actual root update phase, rather than adopting via a test helper.
    app.render_media_and_edit_results(&ctx);
    app.comic_font_registry_loaded = false;
    let fonts = app.ensure_comic_fonts().unwrap();
    let doc = app.comic_docs[&app.page_path_key(1).unwrap()].clone();
    assert!(Arc::ptr_eq(
        &fonts,
        &app.ensure_comic_fonts_for(&doc).unwrap()
    ));
    assert!(
        !app.comic_font_registry_loaded,
        "satisfied display must not enumerate the picker registry"
    );
    let input = app.prepared_edit_input(1).unwrap();
    assert!(matches!(input.output, PreparedOutput::AnnotationOnly));
    assert!(input.comic.is_some());
    app.prefetch_next_final_unit(&ctx, 0);
    let key = wait_ready(&mut app, 1);
    let FinalEffectState::CpuReady(FinalEffectReady::Edited(ready)) =
        &app.final_effect_pending[&key].state
    else {
        panic!()
    };
    assert!(ready.output.as_ref().unwrap().final_pixels.is_none());
    assert!(ready.output.as_ref().unwrap().comic.is_some());
    begin_registration_frame(&mut app, 1802);
    assert!(app.adopt_prepared_edit(
        &ctx,
        key,
        FinalPrepareAdoption::RootFinalized(None, crate::ui_fullscreen::FinalUnitPosition::Next),
        false
    ));
    let comic = &app.comic_cache[&1];
    assert!(Arc::ptr_eq(&base, &comic.base));
    assert_ne!(
        comic.pixels.pixels, base.pixels,
        "real annotation pixels are baked"
    );
    let texture = comic.texture.id();
    assert_eq!(
        offer_texture_for_test(&mut app, &ctx, 1),
        texture,
        "turn uses the annotated texture"
    );
}

#[test]
fn comic_font_prefetch_two_missing_page_keys_stop_requesting() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    cold_fonts(&mut app);
    let starts = Arc::new(AtomicUsize::new(0));
    let count = starts.clone();
    app.comic_fonts.loader_for_test(Arc::new(move |keys| {
        count.fetch_add(1, Ordering::Relaxed);
        fake_fonts(keys)
    }));
    for (idx, font) in [(1, "no-path-a"), (2, "no-path-b")] {
        app.comic_pages.insert(idx);
        let page = app.page_path_key(idx).unwrap();
        app.comic_docs.insert(
            page,
            vec![comic_core::AnnotationObject::new_text(
                1,
                (0.0, 0.0),
                comic_core::TextBlock {
                    text: "missing".to_owned(),
                    font_key: font.to_owned(),
                    ..Default::default()
                },
            )],
        );
    }
    for _ in 0..12 {
        for idx in [1, 2] {
            app.request_comic_fonts_for_page(&ctx, idx);
            if app.comic_fonts.pending_for_test() {
                app.comic_fonts.wait_for_test();
                app.render_media_and_edit_results(&ctx);
            }
        }
    }
    assert_eq!(starts.load(Ordering::Relaxed), 2);
    assert!(app.prepared_edit_input(1).unwrap().comic.is_none());
    assert!(app.prepared_edit_input(2).unwrap().comic.is_none());
    app.invalidate_comic_fonts(FontInvalidation::FontAdded);
    app.request_comic_fonts_for_page(&ctx, 1);
    assert!(
        app.comic_fonts.pending_for_test(),
        "adding a font releases attempted keys"
    );
    app.comic_fonts.wait_for_test();
    app.comic_fonts.poll();
    assert_eq!(starts.load(Ordering::Relaxed), 3);
}

#[test]
fn comic_font_prefetch_late_worker_cannot_replace_display_sync_fonts() {
    let ctx = egui::Context::default();
    install_presentation(&ctx);
    let mut app = app(&ctx);
    app.ensure_final_composite_texture(&ctx, 1).unwrap();
    annotation(&mut app, 1);
    cold_fonts(&mut app);
    let (entered_tx, entered) = mpsc::channel();
    let (resume, resume_rx) = mpsc::channel();
    let resume_rx = Mutex::new(resume_rx);
    let starts = AtomicUsize::new(0);
    app.comic_fonts.loader_for_test(Arc::new(move |keys| {
        if starts.fetch_add(1, Ordering::Relaxed) == 0 {
            entered_tx.send(()).unwrap();
            resume_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
        }
        fake_fonts(keys)
    }));
    present(&mut app, &ctx);
    app.prefetch_next_final_unit(&ctx, 0);
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    let doc = app.comic_docs[&app.page_path_key(1).unwrap()].clone();
    let displayed = app.ensure_comic_fonts_for(&doc).unwrap();
    assert!(!app.comic_fonts.pending_for_test());
    resume.send(()).unwrap();
    app.render_media_and_edit_results(&ctx);
    assert!(Arc::ptr_eq(
        &displayed,
        app.comic_fonts.fonts().as_ref().unwrap()
    ));
    app.prefetch_next_final_unit(&ctx, 0);
    let key = wait_ready(&mut app, 1);
    let FinalEffectState::CpuReady(FinalEffectReady::Edited(ready)) =
        &app.final_effect_pending[&key].state
    else {
        panic!()
    };
    assert!(Arc::ptr_eq(
        &displayed,
        ready.input.comic.as_ref().unwrap().fonts.as_ref().unwrap()
    ));
}

#[cfg(windows)]
#[test]
fn comic_font_prefetch_f12_uses_shared_owner_and_survives_park() {
    let ctx = egui::Context::default();
    install_presentation(&ctx);
    let mut app = app(&ctx);
    let window = 96_602;
    let id = app.build_active_context_for_test(Some(window), DetachedSource::Image, |owner| {
        seed_context(owner, &ctx);
        owner.local_adjust_pages.clear();
        owner.local_adjust_page_layers.clear();
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
        owner.detached_viewer_independent_active = true;
        owner.set_detached_window_binding_for_test(Some(window));
    });
    cold_fonts(&mut app);
    app.with_active_viewer_context(|owner| {
        owner.activate_mounted_as_independent_detached(window);
        owner.ensure_final_composite_texture(&ctx, 1).unwrap();
        present(owner, &ctx);
        owner.prefetch_next_final_unit(&ctx, 0);
        assert!(owner.comic_fonts.pending_for_test());
    })
    .unwrap();
    assert!(
        app.comic_fonts.pending_for_test(),
        "unmount retains the App-wide request"
    );
    assert!(app.pause_current_active_viewer_context(&ctx));
    app.comic_fonts.wait_for_test();
    app.render_media_and_edit_results(&ctx);
    let shared = app.comic_fonts.fonts().clone().unwrap();
    app.with_viewer_context(id, |owner| {
        assert!(Arc::ptr_eq(
            &shared,
            owner.comic_fonts.fonts().as_ref().unwrap()
        ));
        let input = owner.prepared_edit_input(1).unwrap();
        assert!(matches!(input.output, PreparedOutput::AnnotationOnly));
        assert!(Arc::ptr_eq(
            &shared,
            input.comic.as_ref().unwrap().fonts.as_ref().unwrap()
        ));
    })
    .unwrap();
    app.retire_context(id, "font-shared-owner", |_| ()).unwrap();
    assert!(Arc::ptr_eq(
        &shared,
        app.comic_fonts.fonts().as_ref().unwrap()
    ));
}
