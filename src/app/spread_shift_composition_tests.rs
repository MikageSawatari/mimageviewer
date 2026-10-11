// G3 routes exercise the real accepted viewer and disposable FIFO services.
use super::*;
use crate::app::book_composition_runtime::{
    BookCompositionSource, ProjectionAdoptionCause, ProjectionOrigin, ViewerBookComposition,
};
use crate::app::tests::phase_c_support::setup_app;
use crate::book_composition::{CompositionToken, PageDomain, StoredComposition, fixtures};
use std::time::Duration;

fn install_document(app: &mut App, root: &Path, pages: &[u32]) {
    app.current_folder = Some(root.to_path_buf());
    app.zip_nav = None;
    app.items = pages
        .iter()
        .map(|page_num| GridItem::PdfPage {
            pdf_path: root.to_path_buf(),
            page_num: *page_num,
            content_type: None,
        })
        .collect();
    app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
    app.image_metas = vec![None; app.items.len()];
    app.visible_indices = (0..app.items.len()).collect();
    app.page_identity_index = Arc::new(file_type_resume::PreparedPageIdentityIndex::prepare(
        &app.items,
        Some(root),
        app.settings.file_type_visibility.clone(),
    ));
    app.projection_recipe = crate::app::book_materialization::ProjectionRecipe::Book(
        crate::app::book_materialization::BookScope::Document(root.to_path_buf()),
    );
    app.book_composition = ViewerBookComposition::Automatic;
    app.spread_mode = crate::settings::SpreadMode::Ltr;
    app.viewer_navigation_caches.invalidate();
    app.spread_shift = ViewerSpreadShift::default();
    app.spread_shift.scope = app.current_spread_shift_scope();
    assert_eq!(
        app.spread_shift.scope,
        Some(BookScopeKey::document(root).unwrap())
    );
}

fn composed(app: &mut App, root: &Path) {
    let scope = BookScopeKey::document(root).unwrap();
    let token = CompositionToken {
        service_epoch: 1,
        publication_sequence: 1,
    };
    let composition = fixtures::four_page_book(crate::book_composition::ReadingDirection::Ltr);
    let prepared = crate::app::book_materialization::project_document_composition(
        root,
        4,
        (app.items.clone(), app.image_metas.clone()),
        app.settings.file_type_visibility.clone(),
        token,
        Some(&composition),
    )
    .unwrap();
    app.book_composition = ViewerBookComposition::Accepted {
        scope,
        token,
        record: Some(Arc::new(StoredComposition {
            domain: PageDomain::Document,
            composition,
        })),
        layout: prepared.composition,
        source: BookCompositionSource::Document {
            count: 4,
            metadata: None,
            physical_source: root.to_path_buf(),
        },
        generation: app.items_generation,
        cause: ProjectionAdoptionCause::Normal,
    };
    app.spread_shift.availability = PairingEditAvailability::Composed;
}

fn dormant(app: &mut App) {
    app.spread_shift.state = ViewerShift::Book(SpreadShiftState::Adjusted {
        anchor_ref: SpreadShiftPageRef::DocumentPageIdentity(1),
        resolution: ShiftResolution::Resolved(1),
        persistence: ShiftPersistence::Saved(7),
    });
}

fn seed(service: &SpreadShiftService, scope: &BookScopeKey, page: u32) {
    service
        .set_shift(
            scope.clone(),
            SpreadShiftPageRef::DocumentPageIdentity(page),
            ShiftOperation {
                context_id: ViewerContextId::for_test(998),
                items_generation: 0,
                operation_sequence: 1,
            },
        )
        .unwrap();
    let read = service
        .read_after(scope.clone(), service.watermarks().admitted)
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(read.status.is_ok());
}

#[test]
fn g3_composed_unreadable_and_extracted_routes_do_not_touch_dormant_shift() {
    for kind in 0..3 {
        let mut env = setup_app();
        let root = env.tmp.path().join("route.pdf");
        install_document(&mut env, &root, &[0, 1, 2, 3]);
        let scope = env.current_spread_shift_scope().unwrap();
        let (service, owner) =
            SpreadShiftService::spawn_at(env.tmp.path().join("shift"), None).unwrap();
        env.spread_shift_service = Some(service.clone());
        seed(&service, &scope, 1);
        match kind {
            0 => composed(&mut env, &root),
            1 => {
                env.book_composition = ViewerBookComposition::Blocked {
                    scope: scope.clone(),
                    token: CompositionToken {
                        service_epoch: 1,
                        publication_sequence: 1,
                    },
                    error: "unreadable".into(),
                };
                env.spread_shift.availability = PairingEditAvailability::Unreadable;
            }
            _ => {
                env.projection_recipe =
                    crate::app::book_materialization::ProjectionRecipe::ExtractedResult(
                        crate::app::book_materialization::ExtractedSurface::Rating,
                    );
                env.spread_shift.availability = PairingEditAvailability::ExtractedResult;
            }
        }
        dormant(&mut env);
        let before = env.spread_shift.clone();
        let watermark = service.watermarks();
        let generation = env.spread_shift_settings_generation;
        let memory = env.settings.remember_spread_shift_per_book;
        let identities = Arc::clone(&env.page_identity_index);
        assert!(!env.set_live_spread_shift(2));
        assert!(!env.reset_live_spread_shift(false));
        assert!(!env.reset_live_spread_shift(true));
        assert!(!env.set_spread_shift_memory_from_pairing(memory));
        assert!(!env.set_spread_shift_memory_from_pairing(!memory));
        for mode in [
            crate::settings::SpreadMode::Single,
            crate::settings::SpreadMode::SplitLtr,
        ] {
            env.spread_mode = mode;
            assert!(!env.set_spread_shift_memory_from_pairing(memory));
            assert!(!env.set_spread_shift_memory_from_pairing(!memory));
            assert!(!env.reset_live_spread_shift(true));
        }
        assert!(env.admit_imported_shift_read(true).is_none());
        assert!(env.capture_book_shift_restore(4, true).is_none());
        assert!(env.begin_composition_removed_shift_restore(5).is_none());
        assert_eq!(env.spread_shift_anchor_idx(), None);
        assert!(
            !env.spread_shift
                .resolve(&identities, &Arc::new(vec![0, 2, 3]))
        );
        env.spread_shift.remap(&[0]);
        env.spread_shift.session_only();
        assert_eq!(env.spread_shift, before);
        assert_eq!(service.watermarks(), watermark);
        assert_eq!(env.spread_shift_settings_generation, generation);
        assert_eq!(env.settings.remember_spread_shift_per_book, memory);
        assert!(Arc::ptr_eq(&env.page_identity_index, &identities));
        let read = service
            .read_after(scope.clone(), watermark.admitted)
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert_eq!(read.row, Some(SpreadShiftPageRef::DocumentPageIdentity(1)));
        owner.shutdown_and_join();
    }
}

#[test]
fn g3_composition_removed_uses_saved_identity_on_and_ignores_dormant_row_off() {
    for memory in [true, false] {
        let mut env = setup_app();
        let root = env.tmp.path().join("removed.pdf");
        install_document(&mut env, &root, &[2, 0, 3, 1]);
        let scope = env.current_spread_shift_scope().unwrap();
        let (service, owner) =
            SpreadShiftService::spawn_at(env.tmp.path().join("shift"), None).unwrap();
        env.spread_shift_service = Some(service.clone());
        seed(&service, &scope, 1);
        dormant(&mut env);
        env.settings.remember_spread_shift_per_book = memory;
        let watermark = service.watermarks();
        let mut restore = env.begin_composition_removed_shift_restore(10).unwrap();
        assert!(env.poll_book_shift_restore(&mut restore).unwrap());
        assert_eq!(env.spread_shift_anchor_idx(), memory.then_some(3));
        assert_eq!(env.fenced_book_resume(&root), None);
        assert_eq!(service.watermarks(), watermark);
        let read = service
            .read_after(scope, watermark.admitted)
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert_eq!(read.row, Some(SpreadShiftPageRef::DocumentPageIdentity(1)));
        owner.shutdown_and_join();
    }
}

#[test]
fn g3_composed_open_keeps_resume_fence_independent_of_paused_shift_service() {
    let mut env = setup_app();
    let root = env.tmp.path().join("resume.pdf");
    install_document(&mut env, &root, &[0, 1, 2, 3]);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) =
        SpreadShiftService::spawn_at(env.tmp.path().join("shift"), None).unwrap();
    env.spread_shift_service = Some(service.clone());
    let shift_pause = service.pause_for_test();
    service
        .set_shift(
            scope,
            SpreadShiftPageRef::DocumentPageIdentity(1),
            ShiftOperation {
                context_id: ViewerContextId::for_test(998),
                items_generation: 0,
                operation_sequence: 1,
            },
        )
        .unwrap();
    composed(&mut env, &root);
    dormant(&mut env);
    env.book_resume_writer =
        crate::book_resume_db::BookResumeWriter::spawn_at(env.tmp.path().join("resume.db"));
    let writer = env.book_resume_writer.as_ref().unwrap();
    let resume_pause = writer.pause_for_test();
    assert!(writer.record(&root, 2, None));
    let cutoff = writer.watermarks().admitted;
    let before = env.spread_shift.clone();
    let mut restore = env.begin_book_shift_restore(20).unwrap();
    assert!(!env.poll_book_shift_restore(&mut restore).unwrap());
    resume_pause.send(()).unwrap();
    env.book_resume_writer
        .as_ref()
        .unwrap()
        .read_position_after(&crate::path_key::normalize(&root), cutoff)
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(
        env.poll_book_shift_restore(&mut restore).unwrap(),
        "dormant shift worker must not block resume"
    );
    assert_eq!(env.spread_shift.state, before.state);
    assert_eq!(env.fenced_book_resume(&root).unwrap().unwrap().page, 2);
    shift_pause.send(()).unwrap();
    owner.shutdown_and_join();
}

#[test]
fn g3_preparing_waits_for_classification_then_reads_original_shift_cutoff() {
    let mut env = setup_app();
    let root = env.tmp.path().join("prepare.pdf");
    install_document(&mut env, &root, &[0, 1, 2, 3]);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) =
        SpreadShiftService::spawn_at(env.tmp.path().join("shift"), None).unwrap();
    env.spread_shift_service = Some(service.clone());
    let pause = service.pause_for_test();
    service
        .set_shift(
            scope.clone(),
            SpreadShiftPageRef::DocumentPageIdentity(1),
            ShiftOperation {
                context_id: ViewerContextId::for_test(998),
                items_generation: 0,
                operation_sequence: 1,
            },
        )
        .unwrap();
    let (_sender, receiver) = std::sync::mpsc::channel();
    env.book_composition = ViewerBookComposition::Preparing {
        scope: scope.clone(),
        receiver,
        origin: ProjectionOrigin::Fresh,
    };
    let mut restore = env.begin_book_shift_restore(30).unwrap();
    assert!(!env.poll_book_shift_restore(&mut restore).unwrap());
    env.book_composition = ViewerBookComposition::Automatic;
    env.spread_shift.availability = PairingEditAvailability::Automatic;
    assert!(!env.poll_book_shift_restore(&mut restore).unwrap());
    pause.send(()).unwrap();
    service
        .read_after(scope, service.watermarks().admitted)
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(env.poll_book_shift_restore(&mut restore).unwrap());
    assert_eq!(env.spread_shift_anchor_idx(), Some(1));
    owner.shutdown_and_join();
}

#[test]
fn g3_global_memory_preference_changes_policy_preserves_composed_dormant_owner() {
    let mut env = setup_app();
    let root = env.tmp.path().join("policy.pdf");
    install_document(&mut env, &root, &[0, 1, 2, 3]);
    composed(&mut env, &root);
    dormant(&mut env);
    let before = env.spread_shift.clone();
    let generation = env.spread_shift_settings_generation;
    let memory = env.settings.remember_spread_shift_per_book;
    env.spread_shift_memory_setting_changed(memory);
    assert_eq!(env.spread_shift_settings_generation, generation);
    env.spread_shift_memory_setting_changed(!memory);
    assert_eq!(env.spread_shift, before);
    assert_eq!(env.settings.remember_spread_shift_per_book, !memory);
    assert_eq!(env.spread_shift_settings_generation, generation + 1);
}

#[test]
fn g3_composition_removed_pending_read_stays_in_existing_context_owner() {
    let mut env = setup_app();
    let root = env.tmp.path().join("pending-removal.pdf");
    install_document(&mut env, &root, &[0, 1, 2, 3]);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) =
        SpreadShiftService::spawn_at(env.tmp.path().join("shift"), None).unwrap();
    env.spread_shift_service = Some(service.clone());
    let pause = service.pause_for_test();
    service
        .set_shift(
            scope.clone(),
            SpreadShiftPageRef::DocumentPageIdentity(1),
            ShiftOperation {
                context_id: ViewerContextId::for_test(998),
                items_generation: 0,
                operation_sequence: 1,
            },
        )
        .unwrap();
    let watermark = service.watermarks();
    env.start_composition_removed_shift_restore(40);
    assert!(env.spread_shift.retained_readoption_pending());
    pause.send(()).unwrap();
    service
        .read_after(scope, watermark.admitted)
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    env.poll_retained_shift_readoption(&egui::Context::default());
    assert!(!env.spread_shift.retained_readoption_pending());
    assert_eq!(env.spread_shift_anchor_idx(), Some(1));
    assert_eq!(service.watermarks().admitted, watermark.admitted);
    owner.shutdown_and_join();
}

#[test]
fn g3_composition_removed_missing_saved_identity_does_not_substitute_raw_slot() {
    let mut env = setup_app();
    let root = env.tmp.path().join("filtered-removal.pdf");
    install_document(&mut env, &root, &[0, 2, 3]);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) =
        SpreadShiftService::spawn_at(env.tmp.path().join("shift"), None).unwrap();
    env.spread_shift_service = Some(service.clone());
    seed(&service, &scope, 1);
    let watermark = service.watermarks();
    env.start_composition_removed_shift_restore(41);
    assert_eq!(env.spread_shift_anchor_idx(), None);
    assert!(matches!(
        env.spread_shift.state,
        ViewerShift::Book(SpreadShiftState::Adjusted {
            anchor_ref: SpreadShiftPageRef::DocumentPageIdentity(1),
            resolution: ShiftResolution::NotInView,
            ..
        })
    ));
    assert_eq!(service.watermarks(), watermark);
    owner.shutdown_and_join();
}

#[cfg(windows)]
#[test]
fn g3_composed_inactive_context_keeps_dormant_owner_when_global_memory_changes() {
    let mut env = setup_app();
    let root = env.tmp.path().join("main.pdf");
    let sibling_root = env.tmp.path().join("sibling.pdf");
    install_document(&mut env, &root, &[0, 1, 2, 3]);
    let sibling = env.build_window_context_for_test(139_003, |app| {
        install_document(app, &sibling_root, &[0, 1, 2, 3]);
        composed(app, &sibling_root);
        dormant(app);
    });
    let mut before = None;
    env.with_window_context_for_test(sibling, |app| {
        before = Some((
            app.spread_shift.clone(),
            app.book_composition.clone(),
            app.items_generation,
            Arc::clone(&app.page_identity_index),
        ));
    });
    let memory = env.settings.remember_spread_shift_per_book;
    env.spread_shift_memory_setting_changed(!memory);
    env.with_window_context_for_test(sibling, |app| {
        let (shift, composition, generation, identities) = before.as_ref().unwrap();
        assert_eq!(&app.spread_shift, shift);
        assert_eq!(&app.book_composition, composition);
        assert_eq!(app.items_generation, *generation);
        assert!(Arc::ptr_eq(&app.page_identity_index, identities));
        assert!(!app.set_spread_shift_memory_from_pairing(memory));
        assert_eq!(&app.spread_shift, shift);
    });
    env.retire_context(sibling, "g3_composed_sibling_test", |_| ())
        .unwrap();
}
