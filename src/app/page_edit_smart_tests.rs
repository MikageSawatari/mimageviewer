//! §1.920: Smart adoption uses the ordinary exit before validation and root capture.
use super::*;

fn page_edit_920_smart_fixture(
    app: &mut crate::app::AppTestEnvForTest,
) -> (SmartFolderSnapshot, PathBuf, PathBuf, PathBuf) {
    app.indexer_init.mark_unavailable_if_not_ready();
    app.active_quick_folder_slot = None;
    let root = app.tmp.path().join("page-edit-920-smart");
    std::fs::create_dir(&root).unwrap();
    let old_page = root.join("b.png");
    let new_page = root.join("a.png");
    let book = root.join("book.pdf");
    let child = root.join("child");
    std::fs::create_dir(&child).unwrap();
    for page in [&old_page, &new_page] {
        image::RgbImage::new(2, 2).save(page).unwrap();
    }
    std::fs::write(&book, b"%PDF-1.4\n").unwrap();
    let mut definition = crate::settings::SmartFolderDefinition::new("page edit 920");
    definition.rules.push(crate::settings::SmartFolderRule::new(
        root.clone(),
        true,
        Default::default(),
    ));
    let source_id = definition.rules[0].id;
    let entries = [
        (&old_page, SmartFolderEntryKind::Image),
        (&new_page, SmartFolderEntryKind::Image),
        (&book, SmartFolderEntryKind::Pdf),
        (&child, SmartFolderEntryKind::Folder),
    ]
    .into_iter()
    .map(|(path, kind)| SmartFolderEntry {
        source_id,
        source_root: root.clone(),
        source_order: 0,
        relative_parent: PathBuf::new(),
        path: path.clone(),
        kind,
        mtime: 1,
        file_size: Some(1),
        matching_rule_indices: vec![0],
    })
    .collect();
    app.settings.smart_folders = vec![definition.clone()];
    let snapshot = SmartFolderSnapshot {
        file_type_visibility: app.settings.file_type_visibility.clone(),
        definition,
        entries: Arc::new(entries),
        video_thumb_overrides: HashMap::new(),
        diag: SmartFolderDiag::default(),
    };
    app.current_folder = Some(root);
    page_edit_920_smart_set_rows(
        app,
        vec![
            GridItem::Image(old_page.clone()),
            GridItem::Image(new_page.clone()),
        ],
    );
    (snapshot, old_page, new_page, book)
}

fn page_edit_920_smart_set_rows(app: &mut App, items: Vec<GridItem>) {
    let count = items.len();
    app.items = items;
    app.image_metas = vec![None; count];
    app.thumbnails = vec![ThumbnailState::Pending; count];
    app.visible_indices = (0..count).collect();
    app.details_order = app.visible_indices.clone();
    app.page_identity_index = Arc::new(
        super::super::file_type_resume::PreparedPageIdentityIndex::prepare(
            &app.items,
            None,
            app.settings.file_type_visibility.clone(),
        ),
    );
    app.viewer_navigation_caches.invalidate();
}

fn page_edit_920_smart_prepare(
    app: &App,
    snapshot: SmartFolderSnapshot,
    reused: Option<Arc<ReusedSmartFolderMetadata>>,
) -> PreparedSmartFolder {
    let (tx, _rx) = mpsc::channel();
    let mut prepared = prepare_smart_folder(
        snapshot,
        without_lazy_metric_request(crate::rating_sort::ListingOrderRequest::from_settings(
            &app.settings,
        )),
        app.rating_session_write_generation,
        app.settings.grid_display_order.clone(),
        false,
        false,
        HashSet::new(),
        HashSet::new(),
        false,
        false,
        false,
        None,
        reused,
        SmartFolderPrepareResources {
            load_conceals: true,
            ..Default::default()
        },
        &AtomicBool::new(false),
        &tx,
    )
    .unwrap()
    .unwrap();
    // This is the same publication performed by spawn_smart_folder_prepare after the producer.
    prepared.metadata_revision = app.smart_folder_metadata_revision;
    prepared
}

fn page_edit_920_smart_draft(app: &mut App, page: &Path) -> (usize, String) {
    let index = app
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Image(path) if path == page))
        .unwrap();
    app.fullscreen_idx = Some(index);
    app.selected = Some(index);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.conceal_mode = true;
    app.conceal_mask_size = [2, 2];
    app.conceal_mask = Some(vec![true, false, true, false]);
    let key = app.page_path_key(index).unwrap();
    assert_eq!(app.conceal_db.as_ref().unwrap().dimensions(&key), None);
    (index, key)
}

fn page_edit_920_smart_assert_saved(app: &App, key: &str) {
    let (mask, shapes, dimensions) = app
        .conceal_db
        .as_ref()
        .unwrap()
        .get_full_checked(key)
        .unwrap()
        .unwrap();
    assert_eq!(mask, vec![true, false, true, false]);
    assert!(shapes.is_empty());
    assert_eq!(dimensions, [2, 2]);
    assert!(!app.conceal_mode);
    assert!(app.conceal_mask.is_none());
}

fn page_edit_920_smart_settle(app: &mut App, definition_id: uuid::Uuid) {
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !app.items_are_smart_folder_view
        || app.smart_folder_busy()
        || app.sidecar_restore_active()
    {
        assert!(Instant::now() < deadline, "Smart adoption did not settle");
        app.poll_smart_folder(&ctx);
        app.poll_sidecar_restore(&ctx);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(app.current_smart_folder_id, Some(definition_id));
}

fn page_edit_920_smart_root_ready(app: &mut App, prepared: PreparedSmartFolder) {
    let id = prepared.snapshot.definition.id;
    let intent = SmartTransitionIntent::Direct { origin: None };
    let navigation = app.capture_smart_list_navigation(&intent).unwrap();
    let request_id = app.smart_folder_transition_sequence.wrapping_add(1);
    app.smart_folder_transition_sequence = request_id;
    app.smart_folder_transition = Some(SmartFolderTransition {
        request_id,
        restore_intent: StartupListIntent::ExplicitList,
        navigation,
        intent,
        target: SmartFolderTransitionTarget::Root(
            super::super::top_level_grid_view::SmartFolderViewState::root(id, Vec::new()),
        ),
        progress: SmartFolderProgress::default(),
        phase: SmartFolderTransitionPhase::RootReady(Box::new(prepared)),
    });
}

#[test]
fn page_edit_920_smart_without_session_saves_before_install_and_root_ready_revision_checks() {
    for staged in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let (snapshot, old_page, new_page, _) = page_edit_920_smart_fixture(&mut app);
        let definition_id = snapshot.definition.id;
        let prepared = page_edit_920_smart_prepare(&app, snapshot, None);
        assert!(
            prepared
                .metadata
                .aggregate
                .as_ref()
                .unwrap()
                .conceal_pages
                .is_empty()
        );
        let revision = prepared.metadata_revision;
        let generation = app.items_generation;
        let old_items = app.items.clone();
        let (_, key) = page_edit_920_smart_draft(&mut app, &old_page);

        if staged {
            page_edit_920_smart_root_ready(&mut app, prepared);
            app.poll_smart_folder_transition_root(&egui::Context::default());
            assert!(matches!(
                app.smart_folder_transition.as_ref().unwrap().phase,
                SmartFolderTransitionPhase::RootPrepare(_)
            ));
        } else {
            assert!(!app.install_prepared_smart_folder(prepared));
            assert!(app.smart_folder_prepare_pending.is_some());
        }
        page_edit_920_smart_assert_saved(&app, &key);
        assert!(app.smart_folder_metadata_revision > revision);
        assert_eq!(
            app.items_generation, generation,
            "the pre-save candidate must not install"
        );
        assert_eq!(app.items, old_items);

        page_edit_920_smart_settle(&mut app, definition_id);
        let index = app
            .items
            .iter()
            .position(|item| item == &GridItem::Image(old_page.clone()))
            .unwrap();
        assert!(
            app.conceal_pages.contains(&index),
            "the existing worker rereads the saved key"
        );
        page_edit_920_smart_assert_saved(&app, &key);
        assert_eq!(
            app.conceal_db
                .as_ref()
                .unwrap()
                .dimensions(&crate::adjustment_db::normalize_path(&new_page)),
            None
        );
    }
}

#[test]
fn page_edit_920_smart_session_keeps_frozen_membership_order_and_reusable_metadata() {
    for staged in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let (snapshot, old_page, _, _) = page_edit_920_smart_fixture(&mut app);
        let definition_id = snapshot.definition.id;
        let initial = page_edit_920_smart_prepare(&app, snapshot.clone(), None);
        assert!(app.install_prepared_smart_folder(initial));
        page_edit_920_smart_settle(&mut app, definition_id);
        let frozen = app
            .top_level_grid_view
            .smart_folder_session()
            .unwrap()
            .root_resort_metadata()
            .unwrap();
        let prepared = page_edit_920_smart_prepare(&app, snapshot, Some(frozen.clone()));
        let order = prepared.items.clone();
        let revision = app.smart_folder_metadata_revision;
        let page_revision = app.page_edit_revision;
        let (_, key) = page_edit_920_smart_draft(&mut app, &old_page);

        if staged {
            page_edit_920_smart_root_ready(&mut app, prepared);
            app.poll_smart_folder_transition_root(&egui::Context::default());
            assert!(app.smart_folder_transition.is_none());
        } else {
            assert!(app.install_prepared_smart_folder(prepared));
        }

        page_edit_920_smart_assert_saved(&app, &key);
        assert_eq!(app.smart_folder_metadata_revision, revision);
        assert!(app.page_edit_revision > page_revision);
        assert_eq!(app.items, order);
        let retained = app
            .top_level_grid_view
            .smart_folder_session()
            .unwrap()
            .root_resort_metadata()
            .unwrap();
        assert!(Arc::ptr_eq(&frozen, &retained));
        assert!(!retained.conceal_paths.contains(&key));
        assert!(
            app.conceal_pages.is_empty(),
            "Esc has the same frozen reusable badge result"
        );
        assert!(app.smart_folder_prepare_pending.is_none());
    }
}

#[test]
fn page_edit_920_smart_child_ready_saves_before_capturing_the_old_root() {
    let mut app = crate::app::setup_app_for_test();
    let (snapshot, old_page, new_page, book) = page_edit_920_smart_fixture(&mut app);
    let definition_id = snapshot.definition.id;
    let prepared = page_edit_920_smart_prepare(&app, snapshot, None);
    assert!(app.install_prepared_smart_folder(prepared));
    page_edit_920_smart_settle(&mut app, definition_id);
    let root_items = app.items.clone();
    let frozen = app
        .top_level_grid_view
        .smart_folder_session()
        .unwrap()
        .root_resort_metadata()
        .unwrap();
    let (old_index, key) = page_edit_920_smart_draft(&mut app, &old_page);
    let mut target = app.top_level_grid_view.smart_folder().unwrap().clone();
    assert!(target.enter_containing_path(&book));
    let source = SmartChildSource::try_new(book.clone(), book.clone()).unwrap();
    let navigation = app
        .capture_smart_list_navigation(&SmartTransitionIntent::Direct { origin: None })
        .unwrap();

    assert!(app.adopt_smart_child_ready(
        19208,
        SmartFolderTransitionRoot::Resident,
        target,
        source,
        SmartPhysicalReady::PdfPages {
            pages: vec![crate::pdf_loader::PdfPageEntry {
                page_num: 0,
                mtime: 1,
                file_size: 1
            }],
            direction: None,
            password: None,
            save_password: false,
        },
        false,
        SmartPhysicalOpenEffects::default(),
        StartupListIntent::ExplicitList,
        navigation,
        None,
    ));

    page_edit_920_smart_assert_saved(&app, &key);
    let session = app.top_level_grid_view.smart_folder_session().unwrap();
    let SmartFolderOpenPhase::Child {
        parked_root: SmartFolderRootPayload::Visible(grid),
        ..
    } = &session.phase
    else {
        panic!("the old root must be retained")
    };
    assert_eq!(grid.items, root_items);
    assert!(
        grid.conceal_pages.contains(&old_index),
        "capture must observe the completed draft save"
    );
    assert!(Arc::ptr_eq(
        &frozen,
        &session.root_resort_metadata().unwrap()
    ));
    assert!(
        !frozen.conceal_paths.contains(&key),
        "the reusable result remains frozen"
    );
    assert!(matches!(
        app.items.as_slice(),
        [GridItem::PdfPage { page_num: 0, .. }]
    ));
    assert_eq!(
        app.conceal_db
            .as_ref()
            .unwrap()
            .dimensions(&crate::adjustment_db::normalize_path(&new_page)),
        None
    );
    assert_eq!(
        app.conceal_db
            .as_ref()
            .unwrap()
            .dimensions(&app.page_path_key(0).unwrap()),
        None
    );
}

#[test]
fn page_edit_920_smart_resident_restore_saves_the_child_and_retains_the_frozen_root() {
    let mut app = crate::app::setup_app_for_test();
    let (snapshot, old_page, _, book) = page_edit_920_smart_fixture(&mut app);
    let definition_id = snapshot.definition.id;
    let definition = snapshot.definition.clone();
    let prepared = page_edit_920_smart_prepare(&app, snapshot, None);
    assert!(app.install_prepared_smart_folder(prepared));
    page_edit_920_smart_settle(&mut app, definition_id);
    let root_items = app.items.clone();
    let frozen = app
        .top_level_grid_view
        .smart_folder_session()
        .unwrap()
        .root_resort_metadata()
        .unwrap();
    let grid = app.take_visible_smart_folder_grid();
    let mut session = app.top_level_grid_view.take_smart_folder_session().unwrap();
    session.phase = SmartFolderOpenPhase::Child {
        logical_path: book.clone(),
        parked_root: SmartFolderRootPayload::Visible(grid),
    };
    app.top_level_grid_view
        .install_smart_folder_session(session);
    let mut state = app.top_level_grid_view.smart_folder().unwrap().clone();
    assert!(state.enter_containing_path(&book));
    app.top_level_grid_view.replace_surface(
        super::super::top_level_grid_view::TopLevelGridSurface::SmartFolder(state),
    );
    app.items_are_smart_folder_view = false;
    app.current_folder = Some(book);
    page_edit_920_smart_set_rows(&mut app, vec![GridItem::Image(old_page.clone())]);
    let (_, key) = page_edit_920_smart_draft(&mut app, &old_page);
    let revision = app.smart_folder_metadata_revision;
    assert!(app.resident_smart_root_restore_is_exact(&definition));

    assert!(
        app.restore_smart_folder_for_synthetic_path(&smart_folder_synthetic_path(definition_id))
    );

    page_edit_920_smart_assert_saved(&app, &key);
    assert_eq!(app.smart_folder_metadata_revision, revision);
    assert_eq!(app.items, root_items);
    let session = app.top_level_grid_view.smart_folder_session().unwrap();
    assert!(matches!(session.phase, SmartFolderOpenPhase::Root));
    assert!(Arc::ptr_eq(
        &frozen,
        &session.root_resort_metadata().unwrap()
    ));
    assert!(!frozen.conceal_paths.contains(&key));
    assert!(app.smart_folder_transition.is_none());
}

#[test]
fn page_edit_920_smart_rejected_requests_do_not_end_the_draft() {
    for stale in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let (snapshot, old_page, _, _) = page_edit_920_smart_fixture(&mut app);
        let prepared = page_edit_920_smart_prepare(&app, snapshot, None);
        let (_, key) = page_edit_920_smart_draft(&mut app, &old_page);
        page_edit_920_smart_root_ready(&mut app, prepared);
        if stale {
            app.current_folder = Some(app.tmp.path().join("a different source"));
        } else {
            app.settings.smart_folders.clear();
        }
        app.poll_smart_folder_transition_root(&egui::Context::default());
        assert!(app.smart_folder_transition.is_none());
        assert!(app.conceal_mode);
        assert_eq!(
            app.conceal_mask.as_deref(),
            Some([true, false, true, false].as_slice())
        );
        assert_eq!(app.conceal_db.as_ref().unwrap().dimensions(&key), None);
    }
}

#[test]
fn page_edit_920_smart_root_ready_for_child_preflight_retains_the_draft() {
    let mut app = crate::app::setup_app_for_test();
    let (snapshot, old_page, _, _) = page_edit_920_smart_fixture(&mut app);
    let definition_id = snapshot.definition.id;
    let prepared = page_edit_920_smart_prepare(&app, snapshot, None);
    let entries = smart_root_navigation_entries(&prepared.items);
    let child = old_page.parent().unwrap().join("child");
    let mut state =
        super::super::top_level_grid_view::SmartFolderViewState::root_with_navigation_entries(
            definition_id,
            entries,
        );
    assert!(state.enter_containing_path(&child));
    let (_, key) = page_edit_920_smart_draft(&mut app, &old_page);
    page_edit_920_smart_root_ready(&mut app, prepared);
    app.smart_folder_transition.as_mut().unwrap().target = SmartFolderTransitionTarget::Child {
        state,
        source: SmartChildSource::try_new(child.clone(), child).unwrap(),
        kind: SmartChildKind::Folder,
        auto_fullscreen: false,
        effects: SmartPhysicalOpenEffects::default(),
        archive_commit: None,
    };

    app.poll_smart_folder_transition_root(&egui::Context::default());

    assert!(matches!(
        app.smart_folder_transition.as_ref().unwrap().phase,
        SmartFolderTransitionPhase::ChildPreflight { .. }
    ));
    assert!(app.conceal_mode);
    assert_eq!(
        app.conceal_mask.as_deref(),
        Some([true, false, true, false].as_slice())
    );
    assert_eq!(app.conceal_db.as_ref().unwrap().dimensions(&key), None);
}
