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

#[test]
fn browser870_final_cover_remote_explicit_ltr_cover_respects_directory_default_and_overrides() {
    use crate::settings::FinalCoverSpreadPreference as Preference;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("browser");
    for inside in [false, true] {
        let folder = if inside {
            root.join("month")
        } else {
            temp.path().join("outside")
        };
        std::fs::create_dir_all(&folder).unwrap();
        let pages = (0..4)
            .map(|index| {
                let page = folder.join(format!("{index:03}.png"));
                image::RgbImage::new(8, 12).save(&page).unwrap();
                crate::grid_item::GridItem::Image(page)
            })
            .collect::<Vec<_>>();
        let resolved = resolve_existing(folder.to_string_lossy().as_ref()).unwrap();
        for global in [false, true] {
            for preference in [Preference::FollowGlobal, Preference::On, Preference::Off] {
                let engine = ContainerEngine::new(crate::settings::Settings {
                    browser_view_output_dir: Some(root.clone()),
                    final_cover_spread_enabled: global,
                    ..Default::default()
                });
                let mut db = crate::spread_db::SpreadDb::open_at(Path::new(":memory:")).unwrap();
                db.set_final_cover_spread_preference(&folder, None, preference)
                    .unwrap();
                *engine.spread_db.state.lock().unwrap() = Ok(Some(db));
                let spread = engine
                    .spread_payload_with_read(
                        &ContainerRequest {
                            address: RemoteAddress::file(folder.to_string_lossy().into_owned()),
                            spread_mode: Some(RemoteSpreadMode::LtrCover),
                            reading_direction: Some(RemoteReadingDirection::Ltr),
                            force_single_page: false,
                        },
                        &resolved,
                        &pages,
                        None,
                        None,
                        SpreadDefaultSource::Directory {
                            book_locked: false,
                            canonical_image_book: false,
                        },
                        true,
                        None,
                        None,
                        None,
                        None,
                    )
                    .unwrap();
                let expected = preference.effective(!inside && global);
                assert_eq!(spread.configured, RemoteSpreadMode::LtrCover);
                assert_eq!(spread.effective, RemoteSpreadMode::LtrCover);
                assert_eq!(
                    spread.final_cover_preference,
                    remote_final_cover_preference(preference)
                );
                assert_eq!(spread.final_cover_enabled, expected);
                assert_eq!(spread.groups.len(), 3);
                let last = spread.groups.last().unwrap();
                assert_eq!(last.pages.len(), 1);
                let supplement = last.presentation.as_ref().is_some_and(|slots| {
                    slots.iter().any(|slot| {
                        slot.role == RemotePagePresentationRole::FrontCoverSupplement
                            && slot.address == spread.groups[0].pages[0]
                    })
                });
                assert_eq!(
                    supplement, expected,
                    "inside={inside} global={global} {preference:?}"
                );
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
                        .get_final_cover_spread_preference_with_fallback(&folder, None),
                    preference
                );
            }
        }
    }
}
