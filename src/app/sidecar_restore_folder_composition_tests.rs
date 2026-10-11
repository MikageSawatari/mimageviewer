//! Headless worker/continuation fixtures. Every wait is tied to an admitted
//! channel/FIFO request or thread completion; no timing polls drive the state machine.
use super::super::{
    book_composition_runtime::ViewerBookComposition,
    book_materialization::{BookScope, ProjectionRecipe},
};
use super::*;
use crate::book_composition::{
    BookComposition, BookCompositionProcessOwner, BookCompositionService, CompositionPageRef,
    CompositionUnit, PageDomain, ReadingDirection, SingleCompanion, SinglePlacement,
};
use std::os::windows::io::AsRawHandle;
use windows::Win32::{
    Foundation::{HANDLE, WAIT_OBJECT_0},
    System::Threading::WaitForSingleObject,
};

const WAIT: std::time::Duration = std::time::Duration::from_secs(30);

fn record() -> BookComposition {
    let mut record = BookComposition::new(
        ReadingDirection::Rtl,
        ["03.png", "01.png", "02.png"]
            .into_iter()
            .map(|name| CompositionUnit::Single {
                page: CompositionPageRef::RelativePath { path: name.into() },
                placement: SinglePlacement::Center,
                companion: SingleCompanion::Vacant,
            })
            .collect(),
    );
    record.hidden_pages = vec![CompositionPageRef::RelativePath {
        path: "02.png".into(),
    }];
    record
}

fn files(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    for name in ["01.png", "02.png", "03.png"] {
        image::RgbImage::new(8, 12).save(root.join(name)).unwrap();
    }
    let mut sidecar = crate::sidecar::SidecarFile::new(root.to_owned());
    sidecar.set_folder_composition(Some(record())).unwrap();
    assert!(sidecar.flush_blocking());
}

fn runtime(app: &mut App, data: &Path) -> (BookCompositionService, BookCompositionProcessOwner) {
    let (service, owner) = BookCompositionService::spawn_at(data.to_owned(), None).unwrap();
    // A completed FIFO read proves startup initialization, without polling status.
    service
        .prepare(
            crate::book_scope::BookScopeKey::folder(data).unwrap(),
            PageDomain::ImageFolder,
        )
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    app.install_book_composition_runtime(service.clone());
    (service, owner)
}

fn install(app: &mut App, root: &Path) {
    let source =
        super::super::book_composition_folder::prepare_folder_source(root, &app.settings).unwrap();
    app.current_folder = Some(root.to_owned());
    app.zip_nav = None;
    app.pdf_placeholder_count = None;
    app.install_prepared_aggregate_items(
        source.recipe,
        source.rows.0,
        source.rows.1,
        Default::default(),
    );
    app.rebuild_visible_indices();
    app.book_composition = ViewerBookComposition::Automatic;
}

fn begin(app: &mut App, root: &Path, action: FirstDisplayAction) -> Receiver<ThumbMsg> {
    let (tx, rx) = mpsc::channel();
    let now = std::time::Instant::now();
    let continuation = SidecarLoadContinuation {
        navigation_after_hydration: None,
        audio_art_prune: None,
        first_display_action: action,
        restore_intent: StartupListIntent::PreservePresentation,
        auto_aspect_seed: None,
        source_path: root.to_owned(),
        source_is_directory: true,
        prepared_subfolder: None,
        prepared_page_edits: None,
        prepared_aggregate: None,
        catalog_existing_keys: HashSet::new(),
        video_items: Vec::new(),
        sli_seq: 0,
        sli_t0: now,
        items_len: app.items.len(),
        detached_physical: false,
        tx,
        cancel: Arc::new(AtomicBool::new(false)),
        restore_started_at: now,
    };
    assert!(app.begin_sidecar_restore(continuation, false).is_ok());
    rx
}

fn ready<T>(receiver: &mut Receiver<T>) {
    let value = receiver
        .recv_timeout(WAIT)
        .expect("admitted worker completion");
    let (send, receive) = mpsc::channel();
    assert!(send.send(value).is_ok());
    *receiver = receive;
}

/// Await the exact current worker, then let the production coordinator consume it once.
/// The native thread handle wait supplies the Checking owner's join-finished proof.
pub(super) fn advance_source_step(app: &mut App) {
    let mut state = app.sidecar_restore.take().expect("source continuation");
    match &mut state.phase {
        Phase::PreparingPageIndex(preparing) => ready(&mut preparing.rx),
        Phase::Checking(checking) => {
            let handle = checking.handle.as_ref().expect("checking worker handle");
            assert_eq!(
                unsafe { WaitForSingleObject(HANDLE(handle.as_raw_handle()), 30_000) },
                WAIT_OBJECT_0
            );
            assert!(handle.is_finished());
        }
        Phase::Running(running) => ready(&mut running.rx),
        Phase::CacheRefreshing(refreshing) => ready(&mut refreshing.rx),
        Phase::InvalidatingPreview(invalidating) => ready(&mut invalidating.rx),
        Phase::Quiescing(quiescing) => {
            if let LocalAdjustFence::Waiting(receiver) = &quiescing.local_adjust_fence {
                receiver
                    .recv_timeout(WAIT)
                    .expect("admitted local-adjust fence");
                quiescing.local_adjust_fence = LocalAdjustFence::Reached;
            }
        }
        Phase::Resuming => panic!("source already terminal"),
    }
    app.sidecar_restore = Some(state);
    app.poll_sidecar_restore(&egui::Context::default());
}

fn finish_source(app: &mut App) -> mpsc::Sender<()> {
    let mut release = None;
    // Normal restore crosses at most six worker phases plus one quiescence fence.
    // Each iteration consumes a different completed phase; it never retries an empty poll.
    for _ in 0..9 {
        if matches!(
            app.sidecar_restore.as_ref().map(|state| &state.phase),
            None | Some(Phase::Resuming)
        ) {
            return release.expect("resume fence was paused after writer quiescence");
        }
        if matches!(
            app.sidecar_restore.as_ref().map(|state| &state.phase),
            Some(Phase::Checking(_))
        ) && release.is_none()
        {
            // Quiescing includes this writer. Pause only after that ownership
            // boundary, so the fixture holds Common's later resume read rather
            // than preventing the source import itself from ever starting.
            release = Some(app.book_resume_writer.as_ref().unwrap().pause_for_test());
        }
        assert!(
            app.fullscreen_idx.is_none(),
            "source import must precede first display"
        );
        advance_source_step(app);
    }
    panic!("source restore did not reach its terminal phase");
}

fn composition_fence(service: &BookCompositionService, root: &Path) {
    service
        .prepare(
            crate::book_scope::BookScopeKey::folder(root).unwrap(),
            PageDomain::ImageFolder,
        )
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
}

fn settle_projection(app: &mut App) {
    if let ViewerBookComposition::Projecting { receiver, .. } = &mut app.book_composition {
        ready(receiver);
    }
    app.poll_book_composition_read(&egui::Context::default());
}

fn release_resume(app: &mut App, release: mpsc::Sender<()>) {
    release.send(()).unwrap();
    app.book_resume_writer
        .as_ref()
        .unwrap()
        .read_all()
        .recv_timeout(WAIT)
        .unwrap()
        .unwrap();
    if let Some(service) = &app.spread_shift_service {
        service
            .count_after(service.watermarks().admitted)
            .recv_timeout(WAIT)
            .unwrap();
    }
}

#[test]
fn g4f_restore_precedes_auto_fullscreen_and_projection_without_resume_deadlock() {
    let mut env = crate::app::setup_app_for_test();
    env.settings.sidecar_backup_enabled = true;
    env.settings.auto_fullscreen_zip_pdf = true;
    env.settings.auto_fullscreen_image_folders = true;
    env.settings.book_open_resume = crate::settings::ResumeMode::FromStart;
    env.settings.detached_viewer_open_images_in_window = false;
    let root = env.tmp.path().join("restored-first-display");
    files(&root);
    let data = env.tmp.path().to_owned();
    let (service, owner) = runtime(&mut env, &data);
    install(&mut env, &root);
    let _thumbnails = begin(&mut env, &root, FirstDisplayAction::AutoFullscreenFolder);
    assert!(env.folder_composition_restore_source_pending());
    env.poll_book_composition_read(&egui::Context::default());
    assert!(matches!(
        env.book_composition,
        ViewerBookComposition::Automatic
    ));
    assert!(service.snapshot().rows.is_empty());
    let release = finish_source(&mut env);
    assert!(!env.folder_composition_restore_source_pending());
    assert!(
        service.snapshot().rows.contains_key(
            &crate::book_scope::BookScopeKey::folder(&root)
                .unwrap()
                .storage_key()
        )
    );
    env.poll_book_composition_read(&egui::Context::default());
    composition_fence(&service, &root);
    env.poll_book_composition_read(&egui::Context::default());
    assert!(matches!(
        env.book_composition,
        ViewerBookComposition::Projecting { .. }
    ));
    assert!(env.fullscreen_idx.is_none());
    release_resume(&mut env, release);
    env.poll_sidecar_restore(&egui::Context::default());
    assert!(
        env.sidecar_restore.is_none(),
        "Common's resume fence must finish while projection waits"
    );
    assert!(
        env.fullscreen_idx.is_none(),
        "first-display target must wait for composition rows"
    );
    settle_projection(&mut env);
    composition_fence(&service, &root);
    env.poll_book_composition_read(&egui::Context::default());
    assert_eq!(
        env.items,
        vec![
            GridItem::Image(root.join("03.png")),
            GridItem::Image(root.join("01.png"))
        ]
    );
    assert_eq!(env.fullscreen_idx, Some(0));
    owner.shutdown_and_join();
}

#[test]
fn g4f_explicit_folder_restore_respects_off_mixed_policy_and_backup_admission() {
    for (book_enabled, backup_enabled, mixed, video_excluded, expect_restore) in [
        (false, true, false, false, true),
        (true, false, false, false, false),
        (true, true, true, false, false),
        (true, true, true, true, true),
    ] {
        let mut env = crate::app::setup_app_for_test();
        env.settings.sidecar_backup_enabled = backup_enabled;
        env.settings.tag_sidecar_backup_enabled = !backup_enabled; // tags-only never admits composition
        env.settings.auto_fullscreen_zip_pdf = true;
        env.settings.auto_fullscreen_image_folders = book_enabled;
        let root = env.tmp.path().join("restore-admission");
        files(&root);
        if mixed {
            std::fs::write(root.join("clip.mp4"), b"video").unwrap();
        }
        if video_excluded {
            env.settings
                .file_type_visibility
                .excluded_extensions
                .insert("mp4".into());
        }
        let data = env.tmp.path().to_owned();
        let (service, owner) = runtime(&mut env, &data);
        install(&mut env, &root);
        let recipe = env.projection_recipe.clone();
        let rows = env.items.clone();
        let _thumbnails = begin(&mut env, &root, FirstDisplayAction::None);
        assert_eq!(
            env.sidecar_restore
                .as_ref()
                .unwrap()
                .common
                .families
                .composition,
            expect_restore
        );
        let release = finish_source(&mut env);
        composition_fence(&service, &root);
        let scope = crate::book_scope::BookScopeKey::folder(&root).unwrap();
        assert_eq!(
            service.snapshot().rows.contains_key(&scope.storage_key()),
            expect_restore
        );
        release_resume(&mut env, release);
        env.poll_sidecar_restore(&egui::Context::default());
        assert!(env.sidecar_restore.is_none());
        env.poll_book_composition_read(&egui::Context::default());
        composition_fence(&service, &root);
        env.poll_book_composition_read(&egui::Context::default());
        settle_projection(&mut env);
        assert_eq!(env.projection_recipe, recipe);
        if !book_enabled || (mixed && !video_excluded) {
            assert_eq!(recipe, ProjectionRecipe::OrdinaryList);
            assert_eq!(env.items, rows);
        } else if expect_restore {
            assert!(matches!(
                recipe,
                ProjectionRecipe::Book(BookScope::ImageFolder(_))
            ));
            assert_eq!(env.items.len(), 2);
        } else {
            assert_eq!(env.items, rows);
        }
        owner.shutdown_and_join();
    }
}

#[test]
fn g4f_folder_restore_source_gate_is_bound_to_its_context_generation_and_phase() {
    let mut env = crate::app::setup_app_for_test();
    let root = env.tmp.path().join("source-owner");
    files(&root);
    env.settings.sidecar_backup_enabled = true;
    env.settings.auto_fullscreen_zip_pdf = true;
    env.settings.auto_fullscreen_image_folders = true;
    install(&mut env, &root);
    let _thumbnails = begin(&mut env, &root, FirstDisplayAction::None);
    assert!(env.folder_composition_restore_source_pending());
    let generation = env.items_generation;
    env.items_generation += 1;
    assert!(!env.folder_composition_restore_source_pending());
    env.items_generation = generation;
    let target = env.sidecar_restore.as_ref().unwrap().common.target_context;
    env.sidecar_restore.as_mut().unwrap().common.target_context = ViewerContextId::for_test(999);
    assert!(!env.folder_composition_restore_source_pending());
    env.sidecar_restore.as_mut().unwrap().common.target_context = target;
    assert!(env.folder_composition_restore_source_pending());
    // Phase::Resuming publishes the source dependency's terminal fact and lets
    // the independently owned Common resume fence complete classification.
    env.sidecar_restore.as_mut().unwrap().phase = Phase::Resuming;
    assert!(!env.folder_composition_restore_source_pending());
    env.sidecar_restore.take();
}

#[test]
fn g4f_folder_restore_preserves_existing_container_name_classification() {
    for name in ["photos.zip", "photos.pdf"] {
        let mut env = crate::app::setup_app_for_test();
        let root = env.tmp.path().join(name);
        files(&root);
        env.settings.sidecar_backup_enabled = true;
        env.settings.auto_fullscreen_zip_pdf = true;
        env.settings.auto_fullscreen_image_folders = true;
        install(&mut env, &root);
        assert!(!matches!(
            env.projection_recipe,
            ProjectionRecipe::Book(BookScope::ImageFolder(_))
        ));
        let _thumbnails = begin(&mut env, &root, FirstDisplayAction::None);
        assert!(
            !env.sidecar_restore
                .as_ref()
                .unwrap()
                .common
                .families
                .composition
        );
        env.sidecar_restore
            .as_mut()
            .unwrap()
            .common
            .discard_target();
        env.resolve_sidecar_restore_for_exit();
    }
}

#[test]
fn g4f_sibling_folder_projection_finishes_while_other_context_restores_source() {
    let mut env = crate::app::setup_app_for_test();
    env.settings.sidecar_backup_enabled = true;
    env.settings.auto_fullscreen_zip_pdf = true;
    env.settings.auto_fullscreen_image_folders = true;
    let root = env.tmp.path().join("source-and-sibling");
    files(&root);
    let data = env.tmp.path().to_owned();
    let (service, owner) = runtime(&mut env, &data);
    let scope = crate::book_scope::BookScopeKey::folder(&root).unwrap();
    service
        .save(scope.clone(), PageDomain::ImageFolder, record())
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    service.drain_changed_scope_keys();
    install(&mut env, &root);
    let main_rows = env.items.clone();
    let main_generation = env.items_generation;
    let sibling = env.fork_materialized_still_context(&root, 0);
    let _thumbnails = begin(&mut env, &root, FirstDisplayAction::None);
    assert!(env.folder_composition_restore_source_pending());
    env.with_window_context_for_test(sibling, |app| {
        assert!(!app.folder_composition_restore_source_pending());
        assert!(!app.sidecar_restore_blocks_projected_context());
        app.poll_book_composition_read(&egui::Context::default());
        composition_fence(&service, &root);
        app.poll_book_composition_read(&egui::Context::default());
        settle_projection(app);
        assert!(matches!(
            app.book_composition,
            ViewerBookComposition::Accepted { .. }
        ));
        assert_eq!(
            app.items,
            vec![
                GridItem::Image(root.join("03.png")),
                GridItem::Image(root.join("01.png"))
            ]
        );
    });
    assert!(env.folder_composition_restore_source_pending());
    assert_eq!(env.items, main_rows);
    assert_eq!(env.items_generation, main_generation);
    env.sidecar_restore
        .as_mut()
        .unwrap()
        .common
        .discard_target();
    env.resolve_sidecar_restore_for_exit();
    env.retire_context(sibling, "g4f_restore_sibling", |_| ())
        .unwrap();
    owner.shutdown_and_join();
}
