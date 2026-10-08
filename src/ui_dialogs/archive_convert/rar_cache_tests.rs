//! Real scan workers exercise retry, native reader sharing, and explicit-output policy.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::*;
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};

const WORKER_TIMEOUT: Duration = Duration::from_secs(30);

fn solid_split_fixture(app: &AppTestEnv) -> (PathBuf, PathBuf) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/archives/rar-multipart-filename-regression/real-split-control");
    let source_dir = app.tmp.path().join("rar-retry-source");
    std::fs::create_dir_all(&source_dir).unwrap();
    let mut paths = Vec::new();
    for part in [1, 2] {
        let name = format!("real-split-control.part{part}.rar");
        let mut bytes = std::fs::read(fixture.join(&name)).unwrap();
        // Existing book-resume fixture technique: preserve real split compressed entries,
        // set the solid archive flag, and recompute the RAR5 main-header CRC.
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
        let path = source_dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        paths.push(path);
    }
    let first = paths.remove(0);
    let later = paths.remove(0);
    let inspection = crate::rar_loader::inspect_for_direct_read(&later).unwrap();
    assert_eq!(inspection.resolved_path, first);
    assert_eq!(
        inspection.decision,
        crate::rar_loader::RarDirectReadDecision::Solid
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
    let metadata = std::fs::metadata(&first).unwrap();
    let pages = crate::zip_loader::enumerate_image_entries(cached).unwrap();
    assert_eq!(pages.len(), 1);
    app.archive_cache_db
        .as_ref()
        .unwrap()
        .record(
            first,
            crate::ui_helpers::mtime_secs(&metadata),
            metadata.len() as i64,
            ArchiveFormat::Rar,
            cached,
            std::fs::metadata(cached).unwrap().len() as i64,
            pages.len() as u32,
            false,
        )
        .unwrap();
}

fn request_open(app: &mut App, later: &Path) {
    app.load_folder(later.parent().unwrap().to_path_buf());
    let outcome =
        app.load_folder_or_convert_archive_with_auto_fullscreen(later.to_path_buf(), false);
    assert!(
        matches!(
            outcome,
            crate::app::FolderOpenOutcome::Classifying
                | crate::app::FolderOpenOutcome::ConversionDialogOpened
                | crate::app::FolderOpenOutcome::Loaded
        ),
        "normal RAR handler refused its source: {outcome:?}"
    );
    let ctx = egui::Context::default();
    let deadline = Instant::now() + WORKER_TIMEOUT;
    // The normal handler captures the parent source proof and typed history owner. Do not
    // consume scan messages until the test has inspected the real native decision below.
    while app.archive_convert.is_none() {
        app.poll_rar_archive_navigation_for_test(&ctx);
        assert!(
            Instant::now() < deadline,
            "normal RAR handler did not create scan owner"
        );
        std::thread::yield_now();
    }
}

fn receive_cached_scan(app: &mut App, first: &Path, cached: &Path) {
    let state = app.archive_convert.as_mut().unwrap();
    let message = state.rx.recv_timeout(WORKER_TIMEOUT).unwrap();
    assert!(
        matches!(&message,
        ArchiveConvertMsg::ScanDone(Ok(ArchiveScanOutcome::CachedZip { source, path }))
            if source == first && path == cached),
        "native scan did not resolve the first-source cache"
    );
    // Feed the unchanged native completion through the real UI consumer after inspecting its
    // decision. No synthetic summary, path, or worker completion is produced by this test.
    let (tx, rx) = mpsc::channel();
    tx.send(message).unwrap();
    state.rx = rx;
    app.poll_archive_convert_messages();
    let state = app.archive_convert.as_ref().unwrap();
    assert_eq!(state.src_path, first);
    assert_eq!(state.pending_nav.as_deref(), Some(cached));
    assert!(state.pending_direct_nav.is_none());
    assert!(matches!(state.phase, ArchiveConvertPhase::Scanning));
}

fn adopt_cached(app: &mut App, cached: &Path) {
    let ctx = egui::Context::default();
    let deadline = Instant::now() + WORKER_TIMEOUT;
    loop {
        app.poll_rar_archive_navigation_for_test(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            app.show_archive_convert_dialog(ctx)
        });
        app.poll_zip_enumerate();
        app.poll_sidecar_restore(&ctx);
        if app.archive_convert.is_none()
            && app.current_folder.as_deref() == Some(cached)
            && app.zip_enumerate_pending.is_none()
            && !app.sidecar_restore_active()
        {
            assert!(
                matches!(app.items.as_slice(), [crate::grid_item::GridItem::ZipImage { zip_path, .. }] if zip_path == cached)
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "cached ZIP adoption did not settle; current={:?} logical={:?} surface={:?} archive={:?} zip={} history={} classification={} sidecar={}",
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

#[test]
fn rar_archive_cache_password_scan_retry_resolves_first_source_and_saved_actual_path() {
    let mut app = setup_app();
    configure(&mut app);
    let (first, later) = solid_split_fixture(&app);
    let cache_dir = app.tmp.path().join("retained-retry-cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let cached = cache_dir.join("saved-book.zip");
    record_cache(&app, &first, &cached);
    assert_ne!(
        cached,
        crate::archive_cache::cache_zip_path_for_data_dir(app.tmp.path(), &first)
    );
    let mut reader = crate::zip_loader::open_archive(&cached).unwrap();
    let page = crate::zip_loader::enumerate_image_entries(&cached)
        .unwrap()
        .remove(0);
    let before = crate::zip_loader::read_entry_from_archive(&mut reader, &page.entry_name).unwrap();
    std::fs::create_dir(cached.with_extension("zip.part")).unwrap();
    request_open(&mut app, &later);
    let state = app.archive_convert.as_mut().unwrap();
    // Drain the original real scan without adopting it. The retry fixture starts at the
    // actual password phase; a dummy password on this unencrypted RAR exercises the production
    // password-bearing scan path without an external packer/encrypted user-file dependency.
    assert!(matches!(
        state.rx.recv_timeout(WORKER_TIMEOUT).unwrap(),
        ArchiveConvertMsg::ScanDone(Ok(_))
    ));
    let cancel = Arc::clone(&state.cancel);
    state.phase = ArchiveConvertPhase::PasswordRequired {
        message: None,
        resume: ArchivePasswordResume::Scan,
    };
    state.password_input = " dummy-password ".into();
    app.apply_archive_password();
    let state = app.archive_convert.as_ref().unwrap();
    assert_eq!(state.password.as_deref(), Some("dummy-password"));
    assert!(Arc::ptr_eq(&cancel, &state.cancel));
    assert!(matches!(state.phase, ArchiveConvertPhase::Scanning));
    assert!(!state.allow_direct_read);
    receive_cached_scan(&mut app, &first, &cached);
    adopt_cached(&mut app, &cached);
    assert_eq!(
        app.archive_source_override.as_deref(),
        Some(later.as_path())
    );
    assert_eq!(
        crate::zip_loader::read_entry_from_archive(&mut reader, &page.entry_name).unwrap(),
        before
    );
    assert!(cached.with_extension("zip.part").is_dir());
}

#[test]
fn rar_archive_cache_open_reuses_deterministic_destination_held_by_real_zip_readers() {
    let mut app = setup_app();
    configure(&mut app);
    let (first, later) = solid_split_fixture(&app);
    let cached = app
        .archive_cache_db
        .as_ref()
        .unwrap()
        .reserve_cache_zip_path(&first)
        .unwrap();
    assert_eq!(
        cached,
        crate::archive_cache::cache_zip_path_for_data_dir(app.tmp.path(), &first)
    );
    record_cache(&app, &first, &cached);
    let before = std::fs::read(&cached).unwrap();
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
    request_open(&mut app, &later);
    receive_cached_scan(&mut app, &first, &cached);
    adopt_cached(&mut app, &cached);
    assert_eq!(std::fs::read(&cached).unwrap(), before);
    assert!(!cached.with_extension("zip.part").exists());
    assert!(
        !crate::zip_loader::read_entry_from_archive(&mut reader, &page.entry_name)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn rar_archive_cache_sibling_conversion_ignores_cache_and_keeps_clicked_volume_filename() {
    let mut app = setup_app();
    configure(&mut app);
    let (first, later) = solid_split_fixture(&app);
    let cached = app
        .archive_cache_db
        .as_ref()
        .unwrap()
        .reserve_cache_zip_path(&first)
        .unwrap();
    record_cache(&app, &first, &cached);
    let before = std::fs::read(&cached).unwrap();
    let _reader = crate::zip_loader::open_archive(&cached).unwrap();
    assert!(app.request_explicit_zip_convert(later.clone(), ArchiveFormat::Rar));
    let state = app.archive_convert.as_mut().unwrap();
    let message = state.rx.recv_timeout(WORKER_TIMEOUT).unwrap();
    assert!(
        matches!(&message, ArchiveConvertMsg::ScanDone(Ok(ArchiveScanOutcome::NeedsConversion { source, summary }))
        if source == &later && summary.image_count == 1),
        "explicit output must not adopt a conversion cache or replace its clicked basename"
    );
    let (tx, rx) = mpsc::channel();
    tx.send(message).unwrap();
    state.rx = rx;
    app.poll_archive_convert_messages();
    let state = app.archive_convert.as_ref().unwrap();
    assert_eq!(state.src_path, later);
    assert!(matches!(state.phase, ArchiveConvertPhase::Confirm { .. }));
    assert!(state.pending_nav.is_none());
    app.start_archive_convert();
    let output = sibling_zip_path(&later);
    let deadline = Instant::now() + WORKER_TIMEOUT;
    loop {
        app.poll_archive_convert_messages();
        let state = app.archive_convert.as_ref().unwrap();
        if let ArchiveConvertPhase::Error { message } = &state.phase {
            panic!("sibling conversion failed: {message}");
        }
        if state.pending_sibling_output.as_deref() == Some(output.as_path()) {
            assert!(state.pending_nav.is_none());
            assert!(state.pending_direct_nav.is_none());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "sibling conversion did not finish"
        );
        std::thread::yield_now();
    }
    assert!(output.exists());
    assert_ne!(output, sibling_zip_path(&first));
    assert!(!sibling_zip_path(&first).exists());
    assert_eq!(
        crate::zip_loader::enumerate_image_entries(&output)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(std::fs::read(&cached).unwrap(), before);
    let metadata = std::fs::metadata(&first).unwrap();
    assert_eq!(
        app.archive_cache_db.as_ref().unwrap().peek(
            &first,
            crate::ui_helpers::mtime_secs(&metadata),
            metadata.len() as i64
        ),
        Some(cached)
    );
}
