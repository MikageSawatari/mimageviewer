//! Multipart RAR opens must resolve the cache identity before deciding to convert.
//! These are headless handler/worker tests; no application or native window is launched.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use super::tests::phase_c_support::{AppTestEnv, setup_app};
use super::{App, FolderHistoryDirection, FolderNavHistoryTarget, StartupListIntent};
use crate::archive_converter::ArchiveFormat;
use crate::grid_item::{GridItem, ThumbnailState};
use crate::ui_dialogs::archive_convert::ArchiveConvertPhase;

const WORKER_TIMEOUT: Duration = Duration::from_secs(30);

impl App {
    /// Shared headless adapter for archive-dialog tests outside App's private module boundary.
    pub(crate) fn poll_rar_archive_navigation_for_test(&mut self, ctx: &egui::Context) {
        self.settle_open_path_classification_for_test();
        self.poll_collection_history_transition(ctx);
    }
}

fn fixture(app: &AppTestEnv, solid: bool) -> (PathBuf, PathBuf) {
    let root = app.tmp.path().join("multipart-source");
    std::fs::create_dir_all(&root).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/archives/rar-multipart-filename-regression/real-split-control");
    let mut paths = Vec::new();
    for part in [1, 2] {
        let name = format!("real-split-control.part{part}.rar");
        let mut bytes = std::fs::read(fixture.join(&name)).unwrap();
        if solid {
            // Same valid split fixture/solid-header adaptation as book_resume_meter_tests.
            // Preserve actual compressed data and volume headers, fixing the main-header CRC.
            assert_eq!(&bytes[..8], b"Rar!\x1a\x07\x01\x00");
            assert_eq!(bytes[13], 1);
            bytes[16] |= 4;
            let end = 13 + bytes[12] as usize;
            let mut crc = u32::MAX;
            for byte in &bytes[12..end] {
                crc ^= *byte as u32;
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
                }
            }
            bytes[8..12].copy_from_slice(&(!crc).to_le_bytes());
        }
        let path = root.join(name);
        std::fs::write(&path, bytes).unwrap();
        paths.push(path);
    }
    let first = paths.remove(0);
    let later = paths.remove(0);
    let inspection = crate::rar_loader::inspect_for_direct_read(&later).unwrap();
    assert_eq!(inspection.resolved_path, first);
    assert_eq!(
        inspection.decision,
        if solid {
            crate::rar_loader::RarDirectReadDecision::Solid
        } else {
            crate::rar_loader::RarDirectReadDecision::Direct
        }
    );
    (first, later)
}

fn configure(app: &mut App) {
    app.active_quick_folder_slot = None;
    app.settings.auto_fullscreen_zip_pdf = false;
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings.sidecar_backup_enabled = false;
    app.settings.tag_sidecar_backup_enabled = false;
    app.settings
        .set_archive_file_handling(crate::settings::ArchiveFileHandling::Convert);
}

fn record_cache(app: &App, first: &Path, cached: &Path) {
    crate::archive_converter::convert_to_zip(
        first,
        cached,
        ArchiveFormat::Rar,
        &AtomicBool::new(false),
        None,
    )
    .unwrap();
    let meta = std::fs::metadata(first).unwrap();
    let page_count = crate::zip_loader::enumerate_image_entries(cached)
        .unwrap()
        .len();
    assert_eq!(page_count, 1);
    app.archive_cache_db
        .as_ref()
        .unwrap()
        .record(
            first,
            crate::ui_helpers::mtime_secs(&meta),
            meta.len() as i64,
            ArchiveFormat::Rar,
            cached,
            std::fs::metadata(cached).unwrap().len() as i64,
            page_count as u32,
            false,
        )
        .unwrap();
}

fn tick(app: &mut App, ctx: &egui::Context) {
    app.settle_open_path_classification_for_test();
    app.poll_collection_history_transition(ctx);
    app.poll_smart_folder(ctx);
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        app.show_archive_convert_dialog(ctx);
    });
    if let Some(state) = &app.archive_convert {
        if let ArchiveConvertPhase::Error { message } = &state.phase {
            panic!("RAR open failed: {message}");
        }
    }
    app.poll_zip_enumerate();
    app.poll_sidecar_restore(ctx);
}

fn wait_for_book(app: &mut App, cached: &Path) {
    let ctx = egui::Context::default();
    let deadline = Instant::now() + WORKER_TIMEOUT;
    loop {
        tick(app, &ctx);
        if app.current_folder.as_deref() == Some(cached)
            && app.archive_convert.is_none()
            && app.zip_enumerate_pending.is_none()
            && app.smart_folder_transition.is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && !app.sidecar_restore_active()
        {
            assert!(
                matches!(app.items.as_slice(), [GridItem::ZipImage { zip_path, .. }] if zip_path == cached)
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "RAR open did not adopt {cached:?}; current={:?} logical={:?} surface={:?} archive={:?} zip={} history={} classification={} sidecar={}",
            app.current_folder,
            app.archive_source_override,
            app.top_level_grid_view.surface(),
            app.archive_convert.as_ref().map(|state| (
                &state.completion,
                &state.src_path,
                &state.pending_nav,
                &state.pending_direct_nav,
                match &state.phase {
                    ArchiveConvertPhase::Scanning => "Scanning",
                    ArchiveConvertPhase::PasswordRequired { .. } => "PasswordRequired",
                    ArchiveConvertPhase::Confirm { .. } => "Confirm",
                    ArchiveConvertPhase::Converting { .. } => "Converting",
                    ArchiveConvertPhase::Error { .. } => "Error",
                },
            )),
            app.zip_enumerate_pending.is_some(),
            app.top_level_grid_view
                .history_navigation_transition()
                .is_some(),
            app.top_level_grid_view.open_path_classification().is_some(),
            app.sidecar_restore_active(),
        );
        std::thread::yield_now();
    }
}

fn open_normal(app: &mut App, source: &Path) {
    // The public handler first classifies archive-looking paths asynchronously. Accepted
    // classification is a live request; tick then drives its preflight/probe and adoption.
    let outcome =
        app.load_folder_or_convert_archive_with_auto_fullscreen(source.to_path_buf(), false);
    assert!(
        matches!(
            outcome,
            super::FolderOpenOutcome::Classifying
                | super::FolderOpenOutcome::ConversionDialogOpened
                | super::FolderOpenOutcome::Loaded
        ),
        "RAR handler refused {source:?}: {outcome:?}"
    );
}

#[derive(Clone, Copy, Debug)]
enum Entry {
    Normal,
    Smart,
    History,
    Rating,
    Collection,
}

fn open_entry(app: &mut App, source: &Path, entry: Entry) {
    match entry {
        Entry::Normal => open_normal(app, source),
        Entry::History => {
            app.folder_nav_back_stack =
                vec![FolderNavHistoryTarget::Path(source.to_path_buf()).into()];
            app.folder_nav_forward_stack.clear();
            app.dispatch_folder_history_input_for_test(FolderHistoryDirection::Back);
        }
        Entry::Smart => {
            let mut definition =
                crate::settings::SmartFolderDefinition::new("RAR cache regression");
            definition.rules.push(crate::settings::SmartFolderRule::new(
                source.parent().unwrap().to_path_buf(),
                true,
                Default::default(),
            ));
            let id = definition.id;
            app.settings.smart_folders = vec![definition];
            app.open_smart_folder_staged(id, false);
            let ctx = egui::Context::default();
            let deadline = Instant::now() + WORKER_TIMEOUT;
            while !app.items_are_smart_folder_view
                || app.smart_folder_transition.is_some()
                || app.smart_folder_pending.is_some()
                || app.smart_folder_prepare_pending.is_some()
            {
                app.poll_smart_folder(&ctx);
                assert!(Instant::now() < deadline, "Smart root did not settle");
                std::thread::yield_now();
            }
            let idx = app
                .items
                .iter()
                .position(|item| item.drag_source_path() == Some(source))
                .unwrap();
            assert!(app.begin_smart_grid_container_navigation(
                idx,
                source.to_path_buf(),
                false,
                StartupListIntent::ExplicitList,
            ));
        }
        Entry::Rating => {
            let mut meta = crate::rating_db::RatingMeta::new(
                crate::rating_db::RatingItemKind::ConvertibleArchive,
            )
            .with_source_path(source);
            meta.archive_format = Some("rar".into());
            app.rating_db
                .as_ref()
                .unwrap()
                .set_user_rating(
                    &crate::adjustment_db::normalize_path(source),
                    4,
                    Some(&meta),
                )
                .unwrap();
            app.enter_rating_view_from_menu(4);
            let deadline = Instant::now() + WORKER_TIMEOUT;
            while app.rating_view_pending.is_some() {
                app.poll_rating_view();
                assert!(Instant::now() < deadline, "Rating root did not settle");
                std::thread::yield_now();
            }
            assert!(app.items_are_rating_view);
            let owner = app.rating_view_physical_load_owner(source).unwrap();
            assert!(app.start_rating_physical_open(owner, StartupListIntent::ExplicitList));
        }
        Entry::Collection => {
            install_collection_row(app, source);
            // Same native row-owner capture and classified open used by the main grid.
            let owner = app.main_grid_archive_open_owner(0, source);
            assert!(
                matches!(&owner, super::OpenRequestOwner::MainGridArchive(intent) if intent.collection_grid_owner.is_some())
            );
            assert!(matches!(
                app.load_folder_or_convert_archive_with_auto_fullscreen_owned(
                    source.to_path_buf(),
                    false,
                    owner,
                    StartupListIntent::ExplicitList,
                ),
                super::FolderOpenOutcome::Classifying
                    | super::FolderOpenOutcome::ConversionDialogOpened
                    | super::FolderOpenOutcome::Loaded
            ));
        }
    }
}

fn install_collection_row(app: &mut App, path: &Path) {
    use super::top_level_grid_view::{
        CollectionGridIdentity, CollectionGridLoadState, TopLevelGridSurface,
    };
    use crate::collection_store::{
        CollectionDefinition, CollectionEntry, CollectionEntryId, CollectionOrderMode,
        CollectionSnapshot, CollectionSourcePath, prepare_collection_snapshot,
    };
    let collection_id = crate::collection_store::CollectionId::new();
    let source = CollectionSourcePath::from_trusted(path).unwrap();
    let snapshot = CollectionSnapshot {
        catalog_revision: 1,
        definition: CollectionDefinition {
            id: collection_id,
            name: "RAR cache regression".into(),
            order_mode: CollectionOrderMode::Manual,
            standard_sort: crate::settings::SortOrder::FileName,
            shuffle_seed: 0,
            revision: 7,
        },
        entries: std::sync::Arc::from(vec![CollectionEntry {
            id: CollectionEntryId::new(),
            collection_id,
            source_path: source.path().to_path_buf(),
            source_key: source.key().clone(),
            resolved_kind: crate::collection_store::CollectionResolvedKind::ConvertibleArchive,
            manual_position: 0,
        }]),
    };
    let prepared = std::sync::Arc::new(
        prepare_collection_snapshot(
            &snapshot,
            &app.settings.grid_display_order,
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap(),
    );
    app.top_level_grid_view.begin(
        TopLevelGridSurface::Collection(CollectionGridIdentity { collection_id }),
        None,
    );
    app.items = vec![prepared.entries[0].item.clone()];
    app.image_metas = vec![None];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.visible_indices = vec![0];
    app.selected = Some(0);
    app.items_generation = app.items_generation.wrapping_add(1);
    let generation = app.items_generation;
    let session = app.top_level_grid_view.collection_session_mut().unwrap();
    session.accepted_revision = 7;
    session.wanted_revision = 7;
    session.load = CollectionGridLoadState::Ready(
        super::top_level_grid_view::CollectionGridInstalledPresentation::without_thumbnail_sources(
            prepared,
        ),
    );
    session.installed_items_generation = Some(generation);
}

fn retained_reader_cache_hit(entry: Entry) {
    let mut app = setup_app();
    configure(&mut app);
    let (first, later) = fixture(&app, true);
    let cache_dir = app.tmp.path().join("retained-profile-cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    // Exercise DB's actual saved path, including a cache retained after copying a data-dir.
    let cached = cache_dir.join("existing-book.zip");
    assert_ne!(
        cached,
        crate::archive_cache::cache_zip_path_for_data_dir(app.tmp.path(), &first)
    );
    record_cache(&app, &first, &cached);
    assert!(app.try_archive_cache_lookup(&later).is_none());
    assert_eq!(app.try_archive_cache_lookup(&first), Some(cached.clone()));
    let before = std::fs::read(&cached).unwrap();
    // Both real thumbnail/template and explicit open readers remain live during the open.
    assert!(crate::zip_loader::read_first_image_bytes(&cached).is_some());
    let mut reader = crate::zip_loader::open_archive(&cached).unwrap();
    let page = crate::zip_loader::enumerate_image_entries(&cached)
        .unwrap()
        .remove(0);
    assert!(
        !crate::zip_loader::read_entry_from_archive(&mut reader, &page.entry_name)
            .unwrap()
            .is_empty()
    );
    // Deterministically fail any attempted conversion even if sharing differs on this host.
    // Cache reuse must not create a .part writer or replace the existing ZIP.
    std::fs::create_dir(cached.with_extension("zip.part")).unwrap();
    app.load_folder(first.parent().unwrap().to_path_buf());
    open_entry(&mut app, &later, entry);
    wait_for_book(&mut app, &cached);
    assert_eq!(
        std::fs::read(&cached).unwrap(),
        before,
        "{entry:?} rewrote a valid cache"
    );
    assert!(cached.with_extension("zip.part").is_dir());
    assert_eq!(app.try_archive_cache_lookup(&first), Some(cached.clone()));
    assert!(app.try_archive_cache_lookup(&later).is_none());
    assert!(
        !crate::zip_loader::read_entry_from_archive(&mut reader, &page.entry_name)
            .unwrap()
            .is_empty()
    );
    match entry {
        Entry::Smart => {
            let state = app.top_level_grid_view.smart_folder().unwrap();
            assert!(
                matches!(&state.position,
                super::top_level_grid_view::SmartFolderPosition::Container { root_entry, current }
                    if root_entry == &later && current == &later),
                "Smart root archive must preserve its exact logical row: {state:?}"
            );
            assert!(state.navigation_entries.iter().any(|entry| {
                entry.logical_path == later
                    && entry.kind == super::smart_folder::SmartChildKind::ConvertibleArchive
            }));
            assert_eq!(
                app.archive_source_override.as_deref(),
                Some(later.as_path())
            );
        }
        Entry::Rating => assert!(
            matches!(app.folder_nav_current_target(), Some(FolderNavHistoryTarget::RatingPhysical(restore)) if restore.visible_path == later)
        ),
        Entry::Collection => assert!(matches!(
            app.folder_nav_current_target(),
            Some(FolderNavHistoryTarget::CollectionPhysical(_))
        )),
        Entry::History => assert!(app.folder_nav_back_stack.is_empty()),
        Entry::Normal => {}
    }
}

#[test]
fn rar_archive_cache_later_volume_normal_open_reuses_live_cached_zip() {
    retained_reader_cache_hit(Entry::Normal);
}

#[test]
fn rar_archive_cache_later_volume_smart_open_reuses_live_cached_zip() {
    retained_reader_cache_hit(Entry::Smart);
}

#[test]
fn rar_archive_cache_later_volume_history_open_reuses_live_cached_zip() {
    retained_reader_cache_hit(Entry::History);
}

#[test]
fn rar_archive_cache_later_volume_rating_open_reuses_live_cached_zip() {
    retained_reader_cache_hit(Entry::Rating);
}

#[test]
fn rar_archive_cache_later_volume_collection_open_reuses_live_cached_zip() {
    retained_reader_cache_hit(Entry::Collection);
}

fn settle_resume(app: &mut App) {
    let writer = app.book_resume_writer.as_ref().unwrap();
    writer
        .read_all()
        .recv_timeout(WORKER_TIMEOUT)
        .unwrap()
        .unwrap();
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while writer.is_busy() {
        assert!(Instant::now() < deadline, "resume writer did not settle");
        std::thread::yield_now();
    }
    app.poll_book_resume_meters(&egui::Context::default());
}

fn refresh_meter(app: &mut App, later: &Path) {
    let meta = std::fs::metadata(later).unwrap();
    app.install_new_items(
        vec![GridItem::ConvertibleArchive {
            path: later.to_path_buf(),
            format: ArchiveFormat::Rar,
        }],
        vec![Some((
            crate::ui_helpers::mtime_secs(&meta),
            meta.len() as i64,
        ))],
    );
    let scope = std::collections::HashSet::from([0]);
    app.start_converted_archive_cache_paths_refresh(&scope, (0, 1), (0, 1));
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while app.converted_archive_cache_paths_pending.is_some() {
        app.poll_converted_archive_cache_paths(&egui::Context::default());
        assert!(
            Instant::now() < deadline,
            "meter source worker did not settle"
        );
        std::thread::yield_now();
    }
}

#[test]
fn rar_archive_cache_first_conversion_delete_reconvert_resume_meter_keep_first_key() {
    let mut app = setup_app();
    configure(&mut app);
    settle_resume(&mut app);
    let (first, later) = fixture(&app, true);
    let cached = crate::archive_cache::cache_zip_path_for_data_dir(app.tmp.path(), &first);
    assert!(!cached.exists());
    app.load_folder(first.parent().unwrap().to_path_buf());
    open_normal(&mut app, &later);
    wait_for_book(&mut app, &cached);
    assert_eq!(app.try_archive_cache_lookup(&first), Some(cached.clone()));
    assert!(app.try_archive_cache_lookup(&later).is_none());
    app.record_book_resume(0);
    // Deliberately conflicting later-part key must never win over the resolved first source.
    let wrong_key = crate::archive_cache::cache_zip_path_for_data_dir(app.tmp.path(), &later);
    app.persist_book_resume(
        wrong_key,
        0,
        crate::book_resume_db::ReadingMeterValue::new(1, 5),
    );
    settle_resume(&mut app);
    let expected = crate::book_resume_db::ReadingMeterValue::new(1, 1);
    app.load_folder(first.parent().unwrap().to_path_buf());
    refresh_meter(&mut app, &later);
    assert_eq!(app.thumbnail_book_resume_meter(0), expected);

    app.archive_cache_manager_result = None;
    app.archive_cache_maint_pending = Some(crate::cache_maintenance::spawn_archive(
        crate::cache_maintenance::ArchiveMaintTask::DeleteSelected {
            src_paths: vec![first.clone()],
        },
        app.archive_cache_db.as_ref().unwrap().clone(),
    ));
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while app.archive_cache_manager_result.is_none() {
        app.poll_archive_cache_maint_pending();
        assert!(Instant::now() < deadline, "cache deletion did not settle");
        std::thread::yield_now();
    }
    assert!(!cached.exists());
    refresh_meter(&mut app, &later);
    assert_eq!(app.thumbnail_book_resume_meter(0), expected);
    assert_eq!(
        app.converted_archive_cache_paths
            .get(&crate::path_key::normalize_keep_drive(&later)),
        Some(&super::ConvertedArchiveSourceState::Unavailable {
            logical_source: Some(first.clone())
        })
    );
    open_normal(&mut app, &later);
    wait_for_book(&mut app, &cached);
    assert_eq!(app.resume_page_for_container(), Some(0));
    assert_eq!(app.try_archive_cache_lookup(&first), Some(cached.clone()));
    assert!(app.try_archive_cache_lookup(&later).is_none());
    app.load_folder(first.parent().unwrap().to_path_buf());
    refresh_meter(&mut app, &later);
    assert_eq!(app.thumbnail_book_resume_meter(0), expected);
}

#[test]
fn rar_archive_cache_direct_later_volume_has_priority_over_old_conversion_cache() {
    let mut app = setup_app();
    configure(&mut app);
    let (first, later) = fixture(&app, false);
    let cached = app
        .archive_cache_db
        .as_ref()
        .unwrap()
        .reserve_cache_zip_path(&first)
        .unwrap();
    record_cache(&app, &first, &cached);
    app.load_folder(first.parent().unwrap().to_path_buf());
    open_normal(&mut app, &later);
    wait_for_book(&mut app, &first);
    // Typed history preserves the clicked logical row while Direct reads/saves at first RAR.
    assert_eq!(
        app.archive_source_override.as_deref(),
        Some(later.as_path())
    );
    app.record_book_resume(0);
    assert_eq!(
        app.last_book_resume.as_ref().map(|(key, _, _)| key),
        Some(&first)
    );
    assert_eq!(app.try_archive_cache_lookup(&first), Some(cached));
}

#[test]
fn rar_archive_cache_completed_probe_cancelled_before_adoption_keeps_new_folder() {
    let mut app = setup_app();
    configure(&mut app);
    let (first, later) = fixture(&app, true);
    let cached = app
        .archive_cache_db
        .as_ref()
        .unwrap()
        .reserve_cache_zip_path(&first)
        .unwrap();
    record_cache(&app, &first, &cached);
    app.load_folder(first.parent().unwrap().to_path_buf());
    open_normal(&mut app, &later);
    // Hold the actual worker result between completion and UI adoption. Requeue that exact
    // message so the test does not construct a synthetic summary/cache outcome.
    let ctx = egui::Context::default();
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while app.archive_convert.is_none() {
        app.settle_open_path_classification_for_test();
        app.poll_collection_history_transition(&ctx);
        assert!(Instant::now() < deadline, "RAR probe owner was not created");
        std::thread::yield_now();
    }
    let state = app.archive_convert.as_mut().unwrap();
    let cancel = std::sync::Arc::clone(&state.cancel);
    let completion = state.rx.recv_timeout(WORKER_TIMEOUT).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(completion).unwrap();
    state.rx = rx;
    let replacement = app.tmp.path().join("replacement-folder");
    std::fs::create_dir_all(&replacement).unwrap();
    assert!(app.cancel_archive_convert_for_navigation("test_rar_cancel_after_scan"));
    app.load_folder(replacement.clone());
    tick(&mut app, &egui::Context::default());
    assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
    assert!(app.archive_convert.is_none());
    assert_eq!(app.current_folder.as_deref(), Some(replacement.as_path()));
    assert!(app.archive_source_override.is_none());
    assert!(app.zip_enumerate_pending.is_none());
    assert_eq!(app.try_archive_cache_lookup(&first), Some(cached));
}
