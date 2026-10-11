use super::*;
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};
use egui_kittest::kittest::Queryable;
use std::{cell::RefCell, path::Path, rc::Rc, sync::Arc, time::Duration};

const WAIT: Duration = Duration::from_secs(15);

struct FolderFixture {
    app: AppTestEnv,
    service: BookCompositionService,
    process: Option<BookCompositionProcessOwner>,
    scope: BookScopeKey,
}
impl Drop for FolderFixture {
    fn drop(&mut self) {
        if let Some(process) = self.process.take() {
            process.shutdown_and_join();
        }
    }
}

fn install_folder(app: &mut App, root: &Path, service: &BookCompositionService) {
    let source =
        super::super::book_composition_folder::prepare_folder_source(root, &app.settings).unwrap();
    assert!(matches!(
        source.recipe,
        super::super::book_materialization::ProjectionRecipe::Book(
            super::super::book_materialization::BookScope::ImageFolder(_)
        )
    ));
    app.current_folder = Some(root.into());
    app.zip_nav = None;
    app.projection_recipe = source.recipe;
    app.items = source.rows.0;
    app.image_metas = source.rows.1;
    app.thumbnails = vec![crate::grid_item::ThumbnailState::Pending; app.items.len()];
    app.visible_indices = (0..app.items.len()).collect();
    app.page_identity_index = Arc::new(
        super::super::file_type_resume::PreparedPageIdentityIndex::prepare(
            &app.items,
            Some(root),
            app.settings.file_type_visibility.clone(),
        ),
    );
    app.spread_mode = crate::settings::SpreadMode::Ltr;
    app.fullscreen_idx = None;
    let scope = BookScopeKey::folder(root).unwrap();
    app.book_composition = ViewerBookComposition::Accepted {
        token: service.snapshot().token_for(&scope),
        scope,
        record: None,
        layout: CompositionProjectionOutcome::Automatic,
        source: app.composition_source(),
        generation: app.items_generation,
        cause: super::super::book_composition_runtime::ProjectionAdoptionCause::Normal,
    };
}

fn fixture() -> FolderFixture {
    let mut app = setup_app();
    app.book_resume_writer = None;
    // The shared App fixture disables backups; this consumer fixture exercises
    // the released default (ON), including the actual mirror and cache adoption.
    app.settings.sidecar_backup_enabled = true;
    app.settings.auto_fullscreen_zip_pdf = true;
    app.settings.auto_fullscreen_image_folders = true;
    app.settings.skip_duplicate_images = false;
    let root = app.tmp.path().join("image-editor-folder");
    std::fs::create_dir(&root).unwrap();
    for name in ["a.png", "b.png", "c.png", "d.png"] {
        image::RgbaImage::from_pixel(8, 16, image::Rgba([20, 40, 60, 255]))
            .save(root.join(name))
            .unwrap();
    }
    let (service, process) = BookCompositionService::spawn_at(app.tmp.path().into(), None).unwrap();
    app.install_book_composition_runtime(service.clone());
    install_folder(&mut app, &root, &service);
    let mut sidecar = crate::sidecar::SidecarFile::new(root.clone());
    sidecar.set_tags("a.png", ["preserved"]);
    // Keep this page backup dirty in the UI cache: the editor's production fence
    // must preserve and write it before its folder mirror uses the same writer.
    app.sidecars.insert(root.clone(), sidecar);
    FolderFixture {
        app,
        service,
        process: Some(process),
        scope: BookScopeKey::folder(&root).unwrap(),
    }
}

fn finish_prepare(app: &mut App, ctx: &egui::Context) {
    let CompositionEditorState::Preparing { input, receiver } =
        std::mem::take(&mut app.composition_editor)
    else {
        panic!("folder editor must enter the shared Preparing state");
    };
    let completion = receiver.recv_timeout(WAIT).unwrap();
    let (tx, rx) = mpsc::channel();
    tx.send(completion).unwrap();
    app.composition_editor = CompositionEditorState::Preparing {
        input,
        receiver: rx,
    };
    app.poll_composition_editor_operations(ctx);
    let CompositionEditorState::Editing(session) = &app.composition_editor else {
        panic!("Editing expected: {:?}", app.composition_editor);
    };
    assert!(session.prepared.is_ok(), "{:?}", session.prepared);
    assert_eq!(session.input.domain, PageDomain::ImageFolder);
    assert!(
        session
            .pages
            .iter()
            .all(|page| matches!(page, CompositionPageRef::RelativePath { .. }))
    );
    // Wait on the actual admitted thumbnail batch before physical clicks. Image
    // arrival changes button geometry; frame counts are not a worker barrier.
    let CompositionEditorState::Editing(session) = &mut app.composition_editor else {
        unreachable!()
    };
    let thumbnails = session.thumbnails.as_mut().unwrap();
    thumbnails.request(session.pages.clone());
    thumbnails.wait_for_batch_for_test(WAIT);
}

fn start(app: &mut App, ctx: &egui::Context) {
    assert!(app.start_local_composition_editor(ctx));
    finish_prepare(app, ctx);
}

fn draft(app: &App) -> BookComposition {
    let CompositionEditorState::Editing(session) = &app.composition_editor else {
        panic!("Editing expected");
    };
    session.prepared.as_ref().unwrap().draft.record().clone()
}

fn finish_projection(app: &mut App, ctx: &egui::Context) {
    // Deliver real worker results through the production adoption path, with no
    // sleeps or repeated timing guesses. Read and projection are exclusive states.
    for _ in 0..4 {
        app.poll_book_composition_read(ctx);
        let state = std::mem::take(&mut app.book_composition);
        app.book_composition = match state {
            ViewerBookComposition::Preparing {
                scope,
                receiver,
                origin,
            } => {
                let reply = receiver.recv_timeout(WAIT).unwrap();
                let (tx, rx) = mpsc::channel();
                tx.send(reply).unwrap();
                ViewerBookComposition::Preparing {
                    scope,
                    receiver: rx,
                    origin,
                }
            }
            ViewerBookComposition::Projecting {
                scope,
                token,
                record,
                source,
                generation,
                origin,
                receiver,
            } => {
                let reply = receiver.recv_timeout(WAIT).unwrap();
                assert!(reply.is_ok(), "{:?}", reply.as_ref().err());
                let (tx, rx) = mpsc::channel();
                tx.send(reply).unwrap();
                ViewerBookComposition::Projecting {
                    scope,
                    token,
                    record,
                    source,
                    generation,
                    origin,
                    receiver: rx,
                }
            }
            state => {
                app.book_composition = state;
                return;
            }
        };
    }
    panic!("folder projection did not finish");
}

fn harvest_save(
    f: &mut FolderFixture,
    ctx: &egui::Context,
    sibling: ViewerContextId,
    sibling_first: bool,
) {
    assert_eq!(f.app.composition_editor_operations.tickets.len(), 1);
    let completion = f.app.composition_editor_operations.tickets[0]
        .receiver
        .recv_timeout(WAIT)
        .unwrap();
    assert!(
        completion.status.is_ok(),
        "{:?}",
        completion.status.as_ref().err()
    );
    // The actual worker has committed and mirrored, while its receipt is still
    // pending. A late tag completion changes the UI's old full snapshot. Both
    // ordinary flush producers must honor that same save owner, not republish
    // the old root over the worker's new backup.
    // Persistence keys are normalized, while the existing page writer owns the
    // physical folder spelling used by App's cache. Do not invent a second cache
    // owner for the normalized alias in this fixture.
    let root = f.app.current_folder.clone().unwrap();
    let mirrored = crate::sidecar::SidecarFile::load(&root);
    let before = std::fs::read(root.join(crate::sidecar::SIDECAR_FILENAME)).unwrap();
    let tag = if mirrored.folder_composition().is_some() {
        "late-composed"
    } else {
        "late-automatic"
    };
    let cached =
        f.app.sidecars.get_mut(&root).unwrap_or_else(|| {
            panic!("cached full snapshot must remain until {tag} receipt adoption")
        });
    cached.set_tags("c.png", [tag]);
    cached.set_dirty_since_for_test(
        std::time::Instant::now() - crate::sidecar::PERIODIC_FLUSH_INTERVAL,
    );
    let neighbor = f.app.tmp.path().join("flush-neighbor");
    std::fs::create_dir_all(&neighbor).unwrap();
    let mut independent = crate::sidecar::SidecarFile::new(neighbor.clone());
    independent.set_tags("neighbor.png", [tag]);
    f.app.sidecars.insert(neighbor.clone(), independent);
    f.app.flush_all_sidecars();
    f.app.flush_periodic_sidecars();
    crate::sidecar::wait_for_pending_writes();
    assert_eq!(
        std::fs::read(root.join(crate::sidecar::SIDECAR_FILENAME)).unwrap(),
        before
    );
    assert_eq!(
        crate::sidecar::SidecarFile::load(&neighbor).items()["neighbor.png"]
            .tags
            .as_ref()
            .unwrap(),
        &[format!("#{tag}")],
        "another folder must keep flushing while this receipt is pending"
    );
    let (tx, rx) = mpsc::channel();
    tx.send(completion).unwrap();
    f.app.composition_editor_operations.tickets[0].receiver = rx;
    f.app.poll_composition_editor_operations(ctx);
    f.app.poll_book_composition_service_for_test(ctx);
    assert!(
        f.app.sidecars[&root].is_dirty(),
        "late items keep their exact owner"
    );
    f.app.flush_all_sidecars();
    crate::sidecar::wait_for_pending_writes();
    let after = crate::sidecar::SidecarFile::load(&root);
    assert_eq!(after.folder_composition(), mirrored.folder_composition());
    assert_eq!(
        after.items()["c.png"].tags.as_ref().unwrap(),
        &[format!("#{tag}")]
    );
    if sibling_first {
        f.app
            .with_viewer_context(sibling, |viewer| finish_projection(viewer, ctx))
            .unwrap();
    }
    finish_projection(&mut f.app, ctx);
    if !sibling_first {
        f.app
            .with_viewer_context(sibling, |viewer| finish_projection(viewer, ctx))
            .unwrap();
    }
    f.app.poll_composition_editor_operations(ctx);
    assert!(!f.app.composition_editor_modal_active());
    assert!(!f.app.composition_editor_writes_pending());
}

#[test]
fn retired_folder_editor_save_excludes_old_snapshot_from_other_folder_checking_flush() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    let root = f.app.current_folder.clone().unwrap();
    let service = f.service.clone();
    let owner = f
        .app
        .build_window_context_for_test(720, |viewer| install_folder(viewer, &root, &service));
    f.app
        .with_viewer_context(owner, |viewer| start(viewer, &ctx))
        .unwrap();
    let (entered, release) = service.hold_worker_for_test();
    entered.recv_timeout(WAIT).unwrap();
    let edited = f
        .app
        .with_viewer_context(owner, |viewer| {
            viewer.apply_editor_ui_action(
                &ctx,
                ui::EditorUiAction::SetHidden {
                    page: 1,
                    hidden: true,
                },
            );
            let edited = draft(viewer);
            viewer.save_local_composition_editor(&ctx, false);
            edited
        })
        .unwrap();
    assert!(f.app.composition_editor_writes_pending());
    f.app
        .retire_context(owner, "retire_folder_save_before_other_restore", |_| ())
        .unwrap();
    assert!(!f.app.composition_editor_modal_active());
    assert!(f.app.composition_editor_writes_pending());
    release.send(()).unwrap();
    let completion = f.app.composition_editor_operations.tickets[0]
        .receiver
        .recv_timeout(WAIT)
        .unwrap();
    assert!(
        completion.status.is_ok(),
        "{:?}",
        completion.status.as_ref().err()
    );
    let saved = f
        .service
        .prepare(f.scope.clone(), PageDomain::ImageFolder)
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap()
        .unwrap()
        .composition;
    assert_eq!(saved.units, edited.units);
    assert_eq!(saved.hidden_pages, edited.hidden_pages);
    let edited = saved;
    // The worker has committed and mirrored, but the surviving ticket has not
    // delivered its receipt. A later tag completion still owns the old root.
    let mirrored = crate::sidecar::SidecarFile::load(&root);
    assert_eq!(mirrored.folder_composition(), Some(&edited));
    let before = std::fs::read(root.join(crate::sidecar::SIDECAR_FILENAME)).unwrap();
    let cached = f.app.sidecars.get_mut(&root).unwrap();
    assert!(cached.folder_composition().is_none());
    cached.set_tags("c.png", ["late-after-owner-close"]);

    let neighbor = f.app.tmp.path().join("checking-neighbor");
    std::fs::create_dir(&neighbor).unwrap();
    image::RgbaImage::new(8, 16)
        .save(neighbor.join("neighbor.png"))
        .unwrap();
    let mut independent = crate::sidecar::SidecarFile::new(neighbor.clone());
    independent.set_tags("neighbor.png", ["before-checking"]);
    assert!(independent.flush_blocking());
    independent.set_tags("neighbor.png", ["independent-checking"]);
    f.app.sidecars.insert(neighbor.clone(), independent);
    install_folder(&mut f.app, &neighbor, &service);
    let (thumb_tx, _thumb_rx) = mpsc::channel();
    let now = std::time::Instant::now();
    let continuation = super::super::sidecar_restore::SidecarLoadContinuation {
        navigation_after_hydration: None,
        audio_art_prune: None,
        first_display_action: super::super::sidecar_restore::FirstDisplayAction::None,
        restore_intent: super::super::StartupListIntent::PreservePresentation,
        auto_aspect_seed: None,
        source_path: neighbor.clone(),
        source_is_directory: true,
        prepared_subfolder: None,
        prepared_page_edits: None,
        prepared_aggregate: None,
        catalog_existing_keys: Default::default(),
        video_items: Vec::new(),
        sli_seq: 0,
        sli_t0: now,
        items_len: f.app.items.len(),
        detached_physical: false,
        tx: thumb_tx,
        cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        restore_started_at: now,
    };
    assert!(f.app.begin_sidecar_restore(continuation, false).is_ok());
    // Each step advances a distinct admitted owner: page index, quiescence,
    // then the real Checking worker. No empty polling or elapsed-time guesses.
    f.app.advance_sidecar_restore_worker_for_test();
    f.app.advance_sidecar_restore_worker_for_test();
    assert!(f.app.sidecar_restore_checking_for_test());
    f.app.advance_sidecar_restore_worker_for_test();
    assert!(f.app.composition_editor_writes_pending());
    assert_eq!(
        std::fs::read(root.join(crate::sidecar::SIDECAR_FILENAME)).unwrap(),
        before,
        "Checking must not republish the saved folder's old cached root"
    );
    assert_eq!(
        crate::sidecar::SidecarFile::load(&neighbor).items()["neighbor.png"]
            .tags
            .as_ref()
            .unwrap(),
        &["#independent-checking"],
        "Checking must continue flushing unrelated folders"
    );
    let cached = &f.app.sidecars[&root];
    assert!(cached.is_dirty());
    assert!(cached.folder_composition().is_none());
    assert_eq!(
        cached.items()["c.png"].tags.as_ref().unwrap(),
        &["#late-after-owner-close"]
    );
    f.app.resolve_sidecar_restore_for_exit();
    let (tx, rx) = mpsc::channel();
    tx.send(completion).unwrap();
    f.app.composition_editor_operations.tickets[0].receiver = rx;
    f.app.poll_composition_editor_operations(&ctx);
    f.app.poll_book_composition_service_for_test(&ctx);
    assert!(!f.app.composition_editor_writes_pending());
    assert!(!f.app.composition_editor_modal_active());
    assert!(f.app.sidecars[&root].is_dirty());
    f.app.flush_all_sidecars();
    crate::sidecar::wait_for_pending_writes();
    let after = crate::sidecar::SidecarFile::load(&root);
    assert_eq!(after.folder_composition(), Some(&edited));
    assert_eq!(
        after.items()["a.png"].tags.as_ref().unwrap(),
        &["#preserved"]
    );
    assert_eq!(
        after.items()["c.png"].tags.as_ref().unwrap(),
        &["#late-after-owner-close"]
    );
}

fn harness(f: FolderFixture) -> (Rc<RefCell<FolderFixture>>, egui_kittest::Harness<'static>) {
    let f = Rc::new(RefCell::new(f));
    let render = f.clone();
    let mut ready = false;
    let h = crate::test_ui::builder()
        .with_size(egui::vec2(1093.0, 614.0))
        .build(move |ctx| {
            ctx.style_mut(|style| style.scroll_animation = egui::style::ScrollAnimation::none());
            if !ready {
                crate::ui_fonts::configure_fonts(ctx);
                crate::ime_focus::install_ime_input_policy(ctx);
                ready = true;
                ctx.request_repaint();
                return;
            }
            let mut f = render.borrow_mut();
            f.app.update_ime_state(ctx);
            f.app.show_local_composition_editor(ctx);
        });
    (f, h)
}

fn click_visible(h: &mut egui_kittest::Harness<'_>, label: &str) {
    // Source-list move controls are below its nested list on a small screen.
    // Scroll the real editor body before sending a physical pointer click;
    // clicking an offscreen accessibility rectangle would hit the footer.
    // Thumbnail workers may still request frames: process the input/layout
    // frames without requiring this asynchronous screen to become idle.
    h.get_by_label(label).scroll_to_me();
    h.run_steps(4);
    assert!(
        h.ctx
            .content_rect()
            .contains_rect(h.get_by_label(label).rect()),
        "{label} must be visible before clicking"
    );
    h.get_by_label(label).click();
    h.run_steps(4);
}

#[test]
fn folder_editor_real_input_reorders_hides_saves_reopens_removes_and_reloads_siblings() {
    for sibling_first in [false, true] {
        let mut f = fixture();
        let ctx = egui::Context::default();
        let root = f.app.current_folder.clone().unwrap();
        let service = f.service.clone();
        let sibling = f
            .app
            .build_window_context_for_test(719, |viewer| install_folder(viewer, &root, &service));
        start(&mut f.app, &ctx);
        let original = draft(&f.app);
        let (f, mut h) = harness(f);
        h.run_steps(2);
        click_visible(&mut h, "4: d.png");
        assert!(
            matches!(
                f.borrow().app.composition_editor,
                CompositionEditorState::Editing(_)
            ),
            "after page selection: {:?}",
            f.borrow().app.composition_editor
        );
        click_visible(&mut h, "ページを前へ");
        assert!(
            matches!(
                f.borrow().app.composition_editor,
                CompositionEditorState::Editing(_)
            ),
            "after page move: {:?}",
            f.borrow().app.composition_editor
        );
        if h.query_by_label("変更する").is_some() {
            click_visible(&mut h, "変更する");
        }
        assert_ne!(draft(&f.borrow().app).units, original.units);
        click_visible(&mut h, "2: b.png");
        click_visible(&mut h, "非表示");
        let edited = draft(&f.borrow().app);
        assert!(
            edited
                .hidden_pages
                .contains(&CompositionPageRef::RelativePath {
                    path: "b.png".into()
                })
        );
        f.borrow_mut()
            .app
            .with_viewer_context(sibling, |viewer| {
                assert_eq!(
                    viewer.items.len(),
                    4,
                    "draft edits must not mutate the sibling"
                );
                assert!(
                    !viewer.start_local_composition_editor(&ctx),
                    "one global modal owner"
                );
            })
            .unwrap();
        f.borrow_mut()
            .app
            .sidecars
            .entry(root.clone())
            .or_insert_with(|| crate::sidecar::SidecarFile::load(&root));
        click_visible(&mut h, "保存");
        harvest_save(&mut f.borrow_mut(), &h.ctx, sibling, sibling_first);
        let stored = f
            .borrow()
            .service
            .prepare(f.borrow().scope.clone(), PageDomain::ImageFolder)
            .recv_timeout(WAIT)
            .unwrap()
            .status
            .unwrap()
            .unwrap();
        assert_eq!(stored.composition.units, edited.units);
        assert_eq!(stored.composition.hidden_pages, edited.hidden_pages);
        assert_eq!(f.borrow().app.items.len(), 3);
        f.borrow_mut()
            .app
            .with_viewer_context(sibling, |viewer| assert_eq!(viewer.items.len(), 3))
            .unwrap();
        let backup = crate::sidecar::SidecarFile::load(&root);
        assert_eq!(backup.folder_composition(), Some(&stored.composition));
        assert_eq!(
            backup.items()["a.png"].tags.as_ref().unwrap(),
            &["#preserved"]
        );
        start(&mut f.borrow_mut().app, &h.ctx);
        assert_eq!(draft(&f.borrow().app), stored.composition);
        h.run_steps(2);
        click_visible(&mut h, "構成を解除");
        // A previous reload can retire a verified-clean cache. Model the normal
        // page/tag owner loaded before the next save, with the current root.
        f.borrow_mut()
            .app
            .sidecars
            .entry(root.clone())
            .or_insert_with(|| crate::sidecar::SidecarFile::load(&root));
        click_visible(&mut h, "解除する");
        harvest_save(&mut f.borrow_mut(), &h.ctx, sibling, !sibling_first);
        assert!(
            f.borrow()
                .service
                .prepare(f.borrow().scope.clone(), PageDomain::ImageFolder)
                .recv_timeout(WAIT)
                .unwrap()
                .status
                .unwrap()
                .is_none()
        );
        assert_eq!(f.borrow().app.items.len(), 4);
        f.borrow_mut()
            .app
            .with_viewer_context(sibling, |viewer| assert_eq!(viewer.items.len(), 4))
            .unwrap();
        let backup = crate::sidecar::SidecarFile::load(&root);
        assert_eq!(backup.folder_composition(), None);
        assert_eq!(
            backup.items()["a.png"].tags.as_ref().unwrap(),
            &["#preserved"]
        );
        start(&mut f.borrow_mut().app, &h.ctx);
        assert!(draft(&f.borrow().app).hidden_pages.is_empty());
        f.borrow_mut()
            .app
            .close_local_composition_editor(CompositionEditorCloseReason::Cancel);
    }
}
