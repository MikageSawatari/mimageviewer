#[test]
fn browser870_remote_directory_ignores_saved_composition_and_preserves_other_metadata() {
    use crate::settings::{ReadingDirection as Direction, SpreadMode as Mode};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("browser");
    let month = root.join("2026-11");
    std::fs::create_dir_all(&month).unwrap();
    let page = month.join("001.png");
    image::RgbImage::new(8, 12).save(&page).unwrap();
    let items = vec![crate::grid_item::GridItem::Image(page)];
    let resolved = resolve_existing(month.to_string_lossy().as_ref()).unwrap();
    for plain_default in [false, true] {
        for book_locked in [false, true] {
            for canonical_image_book in [false, true] {
                for saved in [
                    None,
                    Some(Mode::Single),
                    Some(Mode::RtlCover),
                    Some(Mode::SplitRtl),
                ] {
                    let engine = ContainerEngine::new(crate::settings::Settings {
                        browser_view_output_dir: Some(root.clone()),
                        apply_default_spread_to_plain_folders: plain_default,
                        default_spread_mode: Mode::RtlCover,
                        default_reading_direction: Direction::Rtl,
                        ..Default::default()
                    });
                    let mut db = crate::spread_db::SpreadDb::open_at(Path::new(":memory:")).unwrap();
                    if let Some(mode) = saved {
                        db.set_mode_and_direction(
                            &month,
                            None,
                            mode,
                            Direction::Rtl,
                            (Mode::Ltr, crate::settings::ReadingFlow::Paged, Direction::Ltr),
                        )
                        .unwrap();
                        if mode == Mode::SplitRtl {
                            db.set_mode_only(&month, mode).unwrap();
                        }
                    }
                    db.set_final_cover_spread_preference(
                        &month,
                        None,
                        crate::settings::FinalCoverSpreadPreference::Off,
                    )
                    .unwrap();
                    let before = db.get_state_with_fallback(&month, None);
                    *engine.spread_db.state.lock().unwrap() = Ok(Some(db));
                    let spread = engine
                        .spread_payload_with_read(
                            &ContainerRequest {
                                address: RemoteAddress::file(month.to_string_lossy().into_owned()),
                                spread_mode: None,
                                reading_direction: None,
                                force_single_page: false,
                            },
                            &resolved,
                            &items,
                            None,
                            None,
                            SpreadDefaultSource::Directory {
                                book_locked,
                                canonical_image_book,
                            },
                            true,
                            None,
                            None,
                            None,
                            None,
                        )
                        .unwrap();
                    assert_eq!(spread.configured, RemoteSpreadMode::Single);
                    assert_eq!(spread.effective, RemoteSpreadMode::Single);
                    assert_eq!(spread.reading_direction, RemoteReadingDirection::Ltr);
                    assert_eq!(
                        spread.final_cover_preference,
                        RemoteFinalCoverSpreadPreference::Off
                    );
                    assert!(!spread.final_cover_enabled);
                    assert_eq!(
                        engine
                            .spread_db
                            .state
                            .lock()
                            .unwrap()
                            .as_ref()
                            .unwrap()
                            .as_ref()
                            .unwrap()
                            .get_state_with_fallback(&month, None),
                        before
                    );
                }
            }
        }
    }
}

#[test]
fn browser870_remote_directory_reads_live_root_and_retains_explicit_request_override() {
    use crate::settings::{ReadingDirection, SpreadMode};
    let guard = crate::settings_db::DataDirOverrideGuard::new();
    assert!(
        crate::settings_db::boot_settings_db(guard.path())
            .db
            .is_some()
    );
    let month = guard.path().join("browser/2026-11");
    std::fs::create_dir_all(&month).unwrap();
    image::RgbImage::new(8, 12).save(month.join("001.png")).unwrap();
    let mut settings = crate::settings::Settings {
        browser_view_output_dir: Some(guard.path().join("other")),
        apply_default_spread_to_plain_folders: true,
        default_spread_mode: SpreadMode::RtlCover,
        default_reading_direction: ReadingDirection::Rtl,
        ..Default::default()
    };
    crate::settings_db::with_db_result(|db| db.save_full(&settings)).unwrap();
    let mut engine = ContainerEngine::new(settings.clone());
    engine.reading_settings = RemoteReadingSettingsSource::Live;
    let mut db = crate::spread_db::SpreadDb::open_at(Path::new(":memory:")).unwrap();
    db.set_mode_and_direction(
        &month,
        None,
        SpreadMode::Rtl,
        ReadingDirection::Rtl,
        (
            SpreadMode::Single,
            crate::settings::ReadingFlow::Paged,
            ReadingDirection::Ltr,
        ),
    )
    .unwrap();
    let before = db.get_state_with_fallback(&month, None);
    *engine.spread_db.state.lock().unwrap() = Ok(Some(db));
    let open = |spread_mode| {
        let ContainerResponse::Success(payload) = engine.container(ContainerRequest {
            address: RemoteAddress::file(month.to_string_lossy().into_owned()),
            spread_mode,
            reading_direction: None,
            force_single_page: false,
        }) else {
            panic!("folder enumeration failed");
        };
        payload
    };
    assert_eq!(open(None).configured_spread_mode, RemoteSpreadMode::Rtl);
    settings.browser_view_output_dir = Some(guard.path().join("browser"));
    crate::settings_db::with_db_result(|db| db.save_full(&settings)).unwrap();
    let browser = open(None);
    assert_eq!(browser.configured_spread_mode, RemoteSpreadMode::Single);
    assert_eq!(browser.reading_direction, RemoteReadingDirection::Ltr);
    // Existing IPC callers may explicitly request a non-persistent projection.
    assert_eq!(
        open(Some(RemoteSpreadMode::Ltr)).configured_spread_mode,
        RemoteSpreadMode::Ltr
    );
    settings.browser_view_output_dir = Some(guard.path().join("other"));
    crate::settings_db::with_db_result(|db| db.save_full(&settings)).unwrap();
    assert_eq!(open(None).configured_spread_mode, RemoteSpreadMode::Rtl);
    assert_eq!(
        engine
            .spread_db
            .state
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .get_state_with_fallback(&month, None),
        before
    );
}
