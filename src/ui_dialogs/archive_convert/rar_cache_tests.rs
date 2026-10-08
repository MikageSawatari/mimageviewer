//! Real scan workers exercise retry, native reader sharing, and explicit-output policy.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::*;
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};

const WORKER_TIMEOUT: Duration = Duration::from_secs(30);

impl App {
    /// Run the actual dialog-message consumer without painting or adopting any navigation.
    pub(crate) fn poll_rar_archive_messages_for_test(&mut self) {
        self.poll_archive_convert_messages();
    }
}

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
fn rar_archive_cache_first_volume_password_scan_retry_reuses_saved_actual_path() {
    let mut app = setup_app();
    configure(&mut app);
    let (first, _later) = solid_split_fixture(&app);
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
    request_open(&mut app, &first);
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
        Some(first.as_path())
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
    let (first, _later) = solid_split_fixture(&app);
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
    request_open(&mut app, &first);
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
fn rar_archive_cache_first_sibling_conversion_ignores_cache_and_keeps_first_filename() {
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
    assert!(app.request_explicit_zip_convert(first.clone(), ArchiveFormat::Rar));
    let state = app.archive_convert.as_mut().unwrap();
    let message = state.rx.recv_timeout(WORKER_TIMEOUT).unwrap();
    assert!(
        matches!(&message, ArchiveConvertMsg::ScanDone(Ok(ArchiveScanOutcome::NeedsConversion { source, summary }))
        if source == &first && summary.image_count == 1),
        "explicit output must not adopt a conversion cache or replace its clicked basename"
    );
    let (tx, rx) = mpsc::channel();
    tx.send(message).unwrap();
    state.rx = rx;
    app.poll_archive_convert_messages();
    let state = app.archive_convert.as_ref().unwrap();
    assert_eq!(state.src_path, first);
    assert!(matches!(state.phase, ArchiveConvertPhase::Confirm { .. }));
    assert!(state.pending_nav.is_none());
    app.start_archive_convert();
    let output = sibling_zip_path(&first);
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
    assert_ne!(output, sibling_zip_path(&later));
    assert!(!sibling_zip_path(&later).exists());
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

const ENCRYPTED_FIXTURE_PASSWORD: &str = "public-mimageviewer-rar-test";

fn header_encrypted_split_fixture(app: &AppTestEnv) -> (PathBuf, PathBuf) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/archives/rar-header-encrypted-multipart-legacy-cache");
    let source_dir = app.tmp.path().join("header-encrypted-source");
    std::fs::create_dir_all(&source_dir).unwrap();
    let first = source_dir.join("header-encrypted.part1.rar");
    let later = source_dir.join("header-encrypted.part2.rar");
    for path in [&first, &later] {
        std::fs::copy(fixture.join(path.file_name().unwrap()), path).unwrap();
    }
    // This is real header encryption: native enumeration without a password must fail.
    assert_eq!(
        crate::rar_loader::inspect_for_direct_read(&later)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    (first, later)
}

fn receive_real_password_required(app: &mut App) -> Arc<AtomicBool> {
    let state = app.archive_convert.as_mut().unwrap();
    let message = state.rx.recv_timeout(WORKER_TIMEOUT).unwrap();
    assert!(matches!(
        &message,
        ArchiveConvertMsg::ScanDone(Err(ConvertError::PasswordRequired))
    ));
    let (tx, rx) = mpsc::channel();
    tx.send(message).unwrap();
    state.rx = rx;
    app.poll_archive_convert_messages();
    let state = app.archive_convert.as_ref().unwrap();
    assert!(matches!(
        state.phase,
        ArchiveConvertPhase::PasswordRequired {
            resume: ArchivePasswordResume::Scan,
            ..
        }
    ));
    Arc::clone(&state.cancel)
}

fn receive_scan_for_ui(app: &mut App) {
    let state = app.archive_convert.as_mut().unwrap();
    let message = state.rx.recv_timeout(WORKER_TIMEOUT).unwrap();
    let (tx, rx) = mpsc::channel();
    tx.send(message).unwrap();
    state.rx = rx;
    app.poll_archive_convert_messages();
}

#[test]
fn rar_archive_cache_header_encrypted_first_volume_real_password_retry_converts() {
    let mut app = setup_app();
    configure(&mut app);
    let (first, later) = header_encrypted_split_fixture(&app);
    request_open(&mut app, &first);
    let cancel = receive_real_password_required(&mut app);
    app.archive_convert.as_mut().unwrap().password_input =
        format!(" {ENCRYPTED_FIXTURE_PASSWORD} ");
    app.apply_archive_password();
    let state = app.archive_convert.as_ref().unwrap();
    assert_eq!(state.password.as_deref(), Some(ENCRYPTED_FIXTURE_PASSWORD));
    assert!(Arc::ptr_eq(&cancel, &state.cancel));
    assert_eq!(state.src_path, first);
    receive_scan_for_ui(&mut app);
    let cached = crate::archive_cache::cache_zip_path_for_data_dir(app.tmp.path(), &first);
    adopt_cached(&mut app, &cached);
    assert_eq!(
        app.archive_source_override.as_deref(),
        Some(first.as_path())
    );
    assert_eq!(app.try_archive_cache_lookup(&first), Some(cached));
    assert!(app.try_archive_cache_lookup(&later).is_none());
    app.record_book_resume(0);
    assert_eq!(
        app.last_book_resume.as_ref().map(|(key, _, _)| key),
        Some(&crate::archive_cache::cache_zip_path_for_data_dir(
            app.tmp.path(),
            &first
        ))
    );
}

#[test]
fn rar_archive_cache_header_encrypted_later_volume_real_password_retry_shows_first_hint() {
    let mut app = setup_app();
    configure(&mut app);
    let (first, later) = header_encrypted_split_fixture(&app);
    request_open(&mut app, &later);
    let cancel = receive_real_password_required(&mut app);
    assert_eq!(app.archive_convert.as_ref().unwrap().src_path, later);
    app.archive_convert.as_mut().unwrap().password_input =
        format!(" {ENCRYPTED_FIXTURE_PASSWORD} ");
    app.apply_archive_password();
    assert!(Arc::ptr_eq(
        &cancel,
        &app.archive_convert.as_ref().unwrap().cancel
    ));
    // A real header-encrypted later-volume scan at the clicked input yields no pages.
    // Neither a synthetic password phase nor a synthetic summary is used here.
    receive_scan_for_ui(&mut app);
    let state = app.archive_convert.as_ref().unwrap();
    let ArchiveConvertPhase::Error { message } = &state.phase else {
        panic!("encrypted later-volume retry must show first-volume guidance, not convert");
    };
    assert_eq!(
        message,
        "画像が見つかりません。分割RARの場合は最初のファイルを開いてください。"
    );
    assert!(
        !message.contains(first.file_name().unwrap().to_str().unwrap()),
        "unknown encrypted source must not infer first filename: {message}"
    );
    assert_eq!(state.src_path, later);
    assert!(state.pending_nav.is_none());
    assert!(state.pending_direct_nav.is_none());
    assert!(state.pending_sibling_output.is_none());
    assert!(app.try_archive_cache_lookup(&first).is_none());
    assert!(app.try_archive_cache_lookup(&later).is_none());
    assert!(!crate::archive_cache::cache_zip_path_for_data_dir(app.tmp.path(), &first).exists());
    assert!(!crate::archive_cache::cache_zip_path_for_data_dir(app.tmp.path(), &later).exists());
}

#[test]
fn rar_archive_cache_later_volume_explicit_conversion_refuses_first_cache() {
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
    receive_scan_for_ui(&mut app);
    let state = app.archive_convert.as_ref().unwrap();
    let ArchiveConvertPhase::Error { message } = &state.phase else {
        panic!("explicit later-volume conversion must refuse instead of confirming");
    };
    assert!(message.contains(first.file_name().unwrap().to_str().unwrap()));
    assert!(message.contains("最初"));
    assert_eq!(state.src_path, later);
    assert!(state.pending_nav.is_none());
    assert!(state.pending_direct_nav.is_none());
    assert!(state.pending_sibling_output.is_none());
    assert!(!sibling_zip_path(&first).exists());
    assert!(!sibling_zip_path(&later).exists());
    assert_eq!(std::fs::read(&cached).unwrap(), before);
    assert!(!cached.with_extension("zip.part").exists());
}

#[test]
fn rar_archive_cache_refusal_keeps_legacy_cache_resume_and_page_edits() {
    let mut app = setup_app();
    configure(&mut app);
    let (first, later) = solid_split_fixture(&app);
    let legacy = app
        .archive_cache_db
        .as_ref()
        .unwrap()
        .reserve_cache_zip_path(&later)
        .unwrap();
    record_cache(&app, &first, &legacy);
    let metadata = std::fs::metadata(&later).unwrap();
    app.archive_cache_db
        .as_ref()
        .unwrap()
        .record(
            &later,
            crate::ui_helpers::mtime_secs(&metadata),
            metadata.len() as i64,
            ArchiveFormat::Rar,
            &legacy,
            std::fs::metadata(&legacy).unwrap().len() as i64,
            1,
            true,
        )
        .unwrap();
    let value = crate::book_resume_db::ReadingMeterValue::new(1, 1);
    app.persist_book_resume(legacy.clone(), 0, value);
    let page = crate::zip_loader::enumerate_image_entries(&legacy)
        .unwrap()
        .remove(0);
    let edit_key = crate::adjustment_db::zip_entry_key(&legacy, &page.entry_name);
    app.rotation_db
        .as_ref()
        .unwrap()
        .set_key(&edit_key, crate::rotation_db::Rotation::Cw90)
        .unwrap();
    let before = std::fs::read(&legacy).unwrap();
    let _reader = crate::zip_loader::open_archive(&legacy).unwrap();
    request_open(&mut app, &later);
    receive_scan_for_ui(&mut app);
    assert!(
        matches!(&app.archive_convert.as_ref().unwrap().phase, ArchiveConvertPhase::Error { message }
        if message.contains("最初のファイル"))
    );
    assert_eq!(std::fs::read(&legacy).unwrap(), before);
    assert_eq!(
        app.archive_cache_db.as_ref().unwrap().peek(
            &later,
            crate::ui_helpers::mtime_secs(&metadata),
            metadata.len() as i64,
        ),
        Some(legacy.clone())
    );
    let snapshot = app
        .book_resume_writer
        .as_ref()
        .unwrap()
        .read_all()
        .recv_timeout(WORKER_TIMEOUT)
        .unwrap()
        .unwrap();
    assert_eq!(
        snapshot.values.get(&crate::path_key::normalize(&legacy)),
        Some(&value)
    );
    assert_eq!(
        app.rotation_db.as_ref().unwrap().get_key(&edit_key),
        Some(crate::rotation_db::Rotation::Cw90)
    );
}
