//! Reusable E-side C1–C12 fixtures for G3. G is not integrated in this worktree.
//! Composition payloads below are deliberately opaque: only E's boundary is exercised.
#![cfg(test)]

use super::*;
use crate::{
    app::{
        ViewerContextId,
        spread_shift_owner::{ViewerShift, ViewerSpreadShift},
    },
    book_resume_db::{BookResumeWriter, PageIdentity},
    book_scope::BookScopeKey,
    settings::{
        PageAloneSettings, ReadingDirection, ReadingFlow, SingletonSpreadEndpointSettings,
        SpreadMode,
    },
};
use std::{path::Path, sync::Arc, time::Duration};

/// Stable page refs and accepted row order. G can feed the same identities to its
/// manifest adapter, then compare its Automatic result with E's resolved anchor.
#[derive(Clone, Debug)]
pub(crate) struct JointBookFixture {
    pub(crate) scope: BookScopeKey,
    pub(crate) pages: Vec<SpreadShiftPageRef>,
    pub(crate) rules: ResolvedAutomaticRules,
}

pub(crate) fn four_page_book(root: impl AsRef<Path>) -> JointBookFixture {
    JointBookFixture {
        scope: BookScopeKey::folder(root).unwrap(),
        pages: ["a.jpg", "b.jpg", "c.jpg", "d.jpg"]
            .map(|name| SpreadShiftPageRef::RelativePath(name.into()))
            .to_vec(),
        rules: ResolvedAutomaticRules {
            mode: SpreadMode::Ltr,
            flow: ReadingFlow::Horizontal,
            direction: ReadingDirection::Ltr,
            final_cover: false,
            page_alone: PageAloneSettings::default(),
            singleton_endpoints: SingletonSpreadEndpointSettings::default(),
            landscape_epoch: 9,
        },
    }
}

impl JointBookFixture {
    pub(crate) fn receipt(&self, context: u64) -> RestoreReceipt {
        RestoreReceipt {
            open_id: 10,
            context_id: ViewerContextId::for_test(context),
            scope: self.scope.clone(),
            items_generation: 4,
            settings_generation: 5,
            shift_cutoff: 0,
            resume_cutoff: 0,
        }
    }

    pub(crate) fn request(&self, receipt: RestoreReceipt) -> ResolveShiftRequest {
        ResolveShiftRequest {
            scope: self.scope.clone(),
            manifest: ShiftManifest {
                generation: receipt.items_generation,
            },
            nav_order: ShiftNavOrder {
                generation: 12,
                indices: (0..self.pages.len()).collect::<Vec<_>>().into(),
            },
            identity_index: ShiftIdentityIndex::prepare(
                &self.scope,
                self.pages
                    .iter()
                    .enumerate()
                    .map(|(slot, page)| (page.to_identity(&self.scope).unwrap(), slot)),
            )
            .unwrap(),
            receipt,
            cause: ShiftRestoreCause::FreshOpen,
            rules: self.rules.clone(),
            memory_policy: ShiftMemoryPolicy::Remember,
            layout_eligibility: PairingEditAvailability::Automatic,
        }
    }

    pub(crate) fn live_owner(
        &self,
        anchor: usize,
        persistence: ShiftPersistence,
    ) -> ViewerSpreadShift {
        let mut owner = ViewerSpreadShift::default();
        owner.scope = Some(self.scope.clone());
        owner.state = ViewerShift::Book(SpreadShiftState::Adjusted {
            anchor_ref: self.pages[anchor].clone(),
            resolution: ShiftResolution::Resolved(anchor),
            persistence,
        });
        owner
    }
}

/// Model-only completion witness. Real writer scenarios below obtain this from
/// read_after; consumers must not fabricate such a proof in application code.
pub(crate) fn completed_read(
    receipt: &RestoreReceipt,
    row: Option<SpreadShiftPageRef>,
) -> ShiftReadReply {
    ShiftReadReply {
        cutoff: receipt.shift_cutoff,
        completed_through: receipt.shift_cutoff,
        maintenance_epoch: 0,
        row,
        status: Ok(()),
    }
}

pub(crate) fn resolve_automatic(
    request: ResolveShiftRequest,
    read: Option<ShiftReadReply>,
    retained_state: SpreadShiftState,
) -> ResolveShiftReply {
    let epoch = read.as_ref().map_or(0, |read| read.maintenance_epoch);
    let BookLayoutSource::Automatic { shift, .. } = resolve_book_layout_source::<()>(
        PreparedLayoutRecipe::Automatic {
            shift_read: read,
            retained_state,
            maintenance_epoch: epoch,
        },
        request,
    )
    .unwrap() else {
        panic!("expected E Automatic projection")
    };
    shift
}

/// Test-only Preparing input from the real canonical builder. G supplies its
/// accepted nav generation and existing layout token; E invents neither here.
/// G must resolve and install the initial spread rules before using this helper.
pub(crate) fn preparing_snapshot<T>(
    app: &mut crate::app::App,
    nav_generation: u64,
    layout_token: T,
) -> EffectiveLayoutSnapshot<T> {
    assert!(
        app.spread_mode.is_spread(),
        "Preparing fixture requires resolved spread mode (Single/Split are unsupported)"
    );
    let nav = app.get_nav_indices();
    let anchors = app
        .spread_display_unit_pages_for_test(&nav)
        .into_iter()
        .map(|pages| {
            if app.spread_mode.is_rtl() {
                *pages.last().unwrap()
            } else {
                pages[0]
            }
        })
        .collect::<Vec<_>>();
    let units = anchors
        .into_iter()
        .map(|anchor| app.spread_display_composition_for_anchor(anchor))
        .collect::<Vec<_>>();
    let ViewerShift::Book(live_shift) = &app.spread_shift.state else {
        panic!("fixture requires Book owner")
    };
    EffectiveLayoutSnapshot {
        scope: app.spread_shift.scope.clone().unwrap(),
        context_id: app.spread_shift_context_id(),
        items_generation: app.items_generation,
        nav_generation,
        settings_generation: app.spread_shift_settings_generation,
        rules: app.accepted_spread_shift_rules(),
        live_shift: live_shift.clone(),
        units: units.into(),
        layout_token,
    }
}

fn operation(context: u64, sequence: u64) -> ShiftOperation {
    ShiftOperation {
        context_id: ViewerContextId::for_test(context),
        items_generation: 4,
        operation_sequence: sequence,
    }
}

fn read_row(service: &SpreadShiftService, scope: &BookScopeKey) -> ShiftReadReply {
    let read = service
        .read_after(scope.clone(), service.watermarks().admitted)
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(read.status.is_ok(), "{:?}", read.status);
    read
}

#[test]
fn joint_01_fenced_reentry_consumes_both_reads_once_and_rejects_other_generations() {
    for shift_first in [false, true] {
        let data = tempfile::tempdir().unwrap();
        let fixture = four_page_book(data.path().join("book"));
        let (service, process) = SpreadShiftService::spawn_at(data.path().into(), None).unwrap();
        let resume = BookResumeWriter::spawn_at(data.path().join("book_resume.db")).unwrap();
        let release_shift = service.pause_for_test();
        let release_resume = resume.pause_for_test();
        service
            .set_shift(
                fixture.scope.clone(),
                fixture.pages[1].clone(),
                operation(1, 1),
            )
            .unwrap();
        let identity = fixture.pages[2].to_identity(&fixture.scope).unwrap();
        assert!(resume.record_identity(
            &fixture.scope.logical_root,
            2,
            None,
            Some(identity.clone())
        ));
        let receipt = RestoreReceipt::capture(
            10,
            ViewerContextId::for_test(2),
            fixture.scope.clone(),
            4,
            5,
            &service,
            Some(&resume),
        );
        assert_eq!((receipt.shift_cutoff, receipt.resume_cutoff), (1, 1));
        let key = crate::path_key::normalize(&fixture.scope.logical_root);
        let mut pending = RestoreReadDependencies::admit(
            receipt.clone(),
            Some(&service),
            Some(&resume),
            Some(&key),
        );
        assert!(pending.try_complete(&receipt, 0).unwrap().is_none());
        if shift_first {
            release_shift.send(()).unwrap();
            read_row(&service, &fixture.scope);
        } else {
            release_resume.send(()).unwrap();
            resume
                .read_position_after(&key, 1)
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
        }
        assert!(pending.try_complete(&receipt, 0).unwrap().is_none());
        if shift_first {
            release_resume.send(()).unwrap();
            resume
                .read_position_after(&key, 1)
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
        } else {
            release_shift.send(()).unwrap();
            read_row(&service, &fixture.scope);
        }
        // A later sibling save belongs to the next entry, not this fixed read point.
        service
            .set_shift(
                fixture.scope.clone(),
                fixture.pages[3].clone(),
                operation(3, 1),
            )
            .unwrap();
        assert_eq!(
            read_row(&service, &fixture.scope).row,
            Some(fixture.pages[3].clone())
        );
        let completed = pending.try_complete(&receipt, 0).unwrap().unwrap();
        assert_eq!(completed.resume.unwrap().identity, Some(identity));
        let CompletedShiftRead::Captured(read) = completed.shift else {
            panic!("fresh proof expected")
        };
        let reply = resolve_automatic(
            fixture.request(receipt.clone()),
            Some(read),
            SpreadShiftState::None,
        );
        assert_eq!(reply.optional_shift.as_ref().unwrap().anchor_idx, 1);
        assert!(reply.matches_adoption(&receipt, 12, 0));
        for field in 0..5 {
            let mut stale = receipt.clone();
            match field {
                0 => stale.open_id += 1,
                1 => stale.context_id = ViewerContextId::for_test(9),
                2 => stale.scope = BookScopeKey::folder("C:/other").unwrap(),
                3 => stale.items_generation += 1,
                _ => stale.settings_generation += 1,
            }
            assert!(!reply.matches_adoption(&stale, 12, 0));
        }
        assert!(!reply.matches_adoption(&receipt, 13, 0));
        assert!(!reply.matches_adoption(&receipt, 12, 1));
        assert!(pending.try_complete(&receipt, 0).is_err());
        assert!(
            process
                .shutdown_and_join()
                .iter()
                .all(|reply| reply.status.is_ok())
        );
    }
}

#[test]
fn joint_02_scope_identity_separates_logical_source_cache_and_nested_books() {
    let source = BookScopeKey::archive(
        "C:/books/original.rar",
        &["wrapper/inner.zip".into(), "chapter".into()],
    )
    .unwrap();
    let cache = BookScopeKey::archive(
        "C:/cache/regenerated.zip",
        &["wrapper/inner.zip/chapter".into()],
    )
    .unwrap();
    let reference = SpreadShiftPageRef::ArchiveEntry("Wrapper/Inner.ZIP/Chapter/Page.JPG".into());
    assert_eq!(
        SpreadShiftPageRef::from_identity(&source, &reference.to_identity(&source).unwrap())
            .unwrap(),
        reference
    );
    assert!(
        SpreadShiftPageRef::from_identity(&source, &reference.to_identity(&cache).unwrap())
            .is_err()
    );
    assert_eq!(
        source.storage_key(),
        "c:/books/original.rar/wrapper/inner.zip/chapter"
    );
    assert_ne!(source.storage_key(), cache.storage_key());
    assert_ne!(
        source,
        BookScopeKey::document("C:/books/original.rar").unwrap()
    );
    assert_ne!(
        source,
        BookScopeKey::archive("D:/books/original.rar", &[source.level_prefix.clone()]).unwrap()
    );
    let data = tempfile::tempdir().unwrap();
    let (service, process) = SpreadShiftService::spawn_at(data.path().into(), None).unwrap();
    service
        .set_shift(source.clone(), reference.clone(), operation(1, 1))
        .unwrap();
    assert_eq!(read_row(&service, &source).row, Some(reference));
    assert!(
        read_row(
            &service,
            &BookScopeKey::document("C:/books/original.rar").unwrap()
        )
        .row
        .is_none()
    );
    assert!(read_row(&service, &cache).row.is_none());
    assert!(
        process
            .shutdown_and_join()
            .iter()
            .all(|reply| reply.status.is_ok())
    );
}

#[test]
fn joint_03_identity_reorders_filters_returns_and_document_exception_is_physical() {
    let fixture = four_page_book("C:/books/identity");
    let receipt = fixture.receipt(1);
    let reference = fixture.pages[1].clone();
    for (pages, nav, expected) in [
        (
            vec![
                fixture.pages[0].clone(),
                reference.clone(),
                fixture.pages[2].clone(),
            ],
            vec![0, 1, 2],
            Some(1),
        ),
        (
            vec![
                fixture.pages[2].clone(),
                fixture.pages[0].clone(),
                reference.clone(),
            ],
            vec![0, 1, 2],
            Some(2),
        ),
        (
            vec![
                fixture.pages[2].clone(),
                fixture.pages[0].clone(),
                reference.clone(),
            ],
            vec![0, 1],
            None,
        ),
        (
            vec![fixture.pages[0].clone(), fixture.pages[2].clone()],
            vec![0, 1],
            None,
        ),
        (fixture.pages.clone(), vec![0, 1, 2, 3], Some(1)),
    ] {
        let mut current = fixture.clone();
        current.pages = pages;
        let mut request = current.request(receipt.clone());
        request.nav_order.indices = nav.into();
        let reply = resolve_automatic(
            request,
            Some(completed_read(&receipt, Some(reference.clone()))),
            SpreadShiftState::None,
        );
        assert_eq!(reply.optional_shift.map(|shift| shift.anchor_idx), expected);
        assert!(
            matches!(reply.state, SpreadShiftState::Adjusted { anchor_ref, .. } if anchor_ref == reference)
        );
    }
    let mut off = fixture.request(receipt.clone());
    off.memory_policy = ShiftMemoryPolicy::SessionOnly;
    assert_eq!(
        resolve_automatic(off, None, SpreadShiftState::None).state,
        SpreadShiftState::None
    );
    for extension in ["pdf", "epub"] {
        let scope = BookScopeKey::document(format!("C:/books/document.{extension}")).unwrap();
        let physical = SpreadShiftPageRef::DocumentPageIdentity(42);
        let identity = physical.to_identity(&scope).unwrap();
        assert!(matches!(
            identity,
            PageIdentity::Document { page_num: 42, .. }
        ));
        let index = ShiftIdentityIndex::prepare(&scope, [(identity, 1)]).unwrap();
        assert_eq!(index.resolve(&physical), Some(1));
        assert_eq!(
            index.resolve(&SpreadShiftPageRef::DocumentPageIdentity(1)),
            None
        );
        // No hash/reflow mapping is promised: physical page 42 remains identity 42.
    }
}

#[test]
fn joint_preparing_rejects_single_and_split_before_enumerating_pair_anchors() {
    let mut app = crate::app::tests::phase_c_support::setup_app();
    for mode in [
        SpreadMode::Single,
        SpreadMode::SplitLtr,
        SpreadMode::SplitRtl,
    ] {
        app.spread_mode = mode;
        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            preparing_snapshot(&mut app, 0, ())
        }));
        let Err(reason) = rejected else {
            panic!("non-spread fixture mode must be rejected: {mode:?}")
        };
        let reason = reason
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| reason.downcast_ref::<String>().map(String::as_str));
        assert_eq!(
            reason,
            Some("Preparing fixture requires resolved spread mode (Single/Split are unsupported)")
        );
    }
}

#[test]
fn joint_04_preparing_captures_nonempty_live_units_roles_placement_and_owner_token() {
    let mut app = crate::app::tests::phase_c_support::setup_app();
    let fixture = four_page_book(app.tmp.path().join("book"));
    app.current_folder = Some(fixture.scope.logical_root.clone());
    app.items = fixture
        .pages
        .iter()
        .map(|page| match page.to_identity(&fixture.scope).unwrap() {
            PageIdentity::Image { path } => crate::grid_item::GridItem::Image(path.into()),
            _ => unreachable!(),
        })
        .collect();
    app.visible_indices = (0..4).collect();
    app.thumbnails = vec![crate::grid_item::ThumbnailState::Pending; 4];
    app.image_metas = vec![None; 4];
    app.viewer_navigation_caches.invalidate();
    app.spread_mode = SpreadMode::RtlCover;
    app.reading_direction = ReadingDirection::Rtl;
    app.settings.final_cover_spread_enabled = true;
    app.settings.singleton_spread_first_enabled = true;
    app.settings.singleton_spread_last_enabled = true;
    // An opaque caller-owned token stands in for G's existing accepted token,
    // never a hash/generation generated by the shift resolver.
    let token: Arc<str> = Arc::from("accepted layout owner token");
    for memory in [false, true] {
        app.settings.remember_spread_shift_per_book = memory;
        for persistence in [
            ShiftPersistence::SessionOnly,
            ShiftPersistence::Saving(10),
            ShiftPersistence::Saved(8),
            ShiftPersistence::SaveFailed,
        ] {
            app.spread_shift = fixture.live_owner(1, persistence);
            let live = preparing_snapshot(&mut app, 12, Arc::clone(&token));
            assert!(!live.units.is_empty());
            assert!(
                live.units
                    .iter()
                    .any(|unit| !unit.pages_in_reading_order().is_empty())
            );
            let expected = live
                .units
                .iter()
                .map(|unit| {
                    (
                        unit.navigation_anchor_idx(),
                        unit.pages_in_reading_order().to_vec(),
                        unit.singleton_placement(),
                    )
                })
                .collect::<Vec<_>>();
            let captured =
                capture_effective_automatic_layout(AutomaticContextToken { effective: &live });
            assert_eq!(captured.live_shift, live.live_shift);
            assert_eq!(captured.rules, live.rules);
            assert_eq!(
                (
                    captured.scope,
                    captured.context_id,
                    captured.items_generation,
                    captured.nav_generation,
                    captured.settings_generation
                ),
                (
                    live.scope.clone(),
                    live.context_id,
                    live.items_generation,
                    12,
                    live.settings_generation
                )
            );
            assert!(Arc::ptr_eq(&captured.units, &live.units));
            assert!(Arc::ptr_eq(&captured.layout_token, &token));
            assert_eq!(
                captured
                    .units
                    .iter()
                    .map(|unit| (
                        unit.navigation_anchor_idx(),
                        unit.pages_in_reading_order().to_vec(),
                        unit.singleton_placement()
                    ))
                    .collect::<Vec<_>>(),
                expected
            );
            // Change the accepted live owner after capture. The Preparing input is fixed.
            app.spread_shift = fixture.live_owner(3, ShiftPersistence::SessionOnly);
            assert_eq!(captured.live_shift, live.live_shift);
        }
    }
}

#[test]
fn joint_05_composed_is_opaque_dormant_row_survives_and_removal_uses_current_policy() {
    let data = tempfile::tempdir().unwrap();
    let fixture = four_page_book(data.path().join("book"));
    let (service, process) = SpreadShiftService::spawn_at(data.path().into(), None).unwrap();
    service
        .set_shift(
            fixture.scope.clone(),
            fixture.pages[1].clone(),
            operation(1, 1),
        )
        .unwrap();
    let read = read_row(&service, &fixture.scope);
    let before = service.watermarks();
    let mut receipt = fixture.receipt(2);
    receipt.shift_cutoff = before.admitted;
    let mut request = fixture.request(receipt.clone());
    request.layout_eligibility = PairingEditAvailability::Composed;
    // Invalid Automatic inputs prove the Composed branch never resolves dormant rows.
    request.manifest.generation += 1;
    let opaque = Arc::new(vec!["G-owned explicit units"]);
    let BookLayoutSource::Composed(returned) =
        resolve_book_layout_source(PreparedLayoutRecipe::Composed(Arc::clone(&opaque)), request)
            .unwrap()
    else {
        panic!()
    };
    assert!(Arc::ptr_eq(&returned, &opaque));
    assert_eq!(service.watermarks(), before);
    assert_eq!(read_row(&service, &fixture.scope).row, read.row);
    for (memory, missing, expected) in [
        (ShiftMemoryPolicy::Remember, false, Some(1)),
        (ShiftMemoryPolicy::SessionOnly, false, None),
        (ShiftMemoryPolicy::Remember, true, None),
    ] {
        let mut request = fixture.request(receipt.clone());
        request.cause = ShiftRestoreCause::CompositionRemoved;
        request.memory_policy = memory;
        if missing {
            request.identity_index = ShiftIdentityIndex::default();
        }
        let reply = resolve_automatic(request, Some(read.clone()), SpreadShiftState::None);
        assert_eq!(reply.optional_shift.map(|shift| shift.anchor_idx), expected);
    }
    assert_eq!(service.watermarks(), before);
    assert_eq!(
        read_row(&service, &fixture.scope).row,
        Some(fixture.pages[1].clone())
    );
    assert!(
        process
            .shutdown_and_join()
            .iter()
            .all(|reply| reply.status.is_ok())
    );
}

#[test]
fn joint_06_pairing_gate_rejects_before_shift_writes_and_preserves_single_split_navigation() {
    let data = tempfile::tempdir().unwrap();
    let fixture = four_page_book(data.path().join("book"));
    let (service, process) = SpreadShiftService::spawn_at(data.path().into(), None).unwrap();
    service
        .set_shift(
            fixture.scope.clone(),
            fixture.pages[1].clone(),
            operation(1, 1),
        )
        .unwrap();
    let original = read_row(&service, &fixture.scope);
    let before = service.watermarks();
    let owner = fixture.live_owner(1, ShiftPersistence::SessionOnly);
    for availability in [
        PairingEditAvailability::Composed,
        PairingEditAvailability::Unreadable,
        PairingEditAvailability::ExtractedResult,
    ] {
        for action in [
            PairingAction::Nudge,
            PairingAction::ClearShift,
            PairingAction::SetMemoryPolicy,
        ] {
            let PairingAdmission::Rejected(reason) = admit_pairing_action(
                PairingContextToken {
                    rules: &fixture.rules,
                    availability,
                },
                action,
            ) else {
                panic!("write intent admitted")
            };
            assert!(!reason.is_empty());
            if availability == PairingEditAvailability::Composed {
                assert_eq!(
                    reason,
                    "本の構成で設定されています。組を変えるには本の構成を編集してください"
                );
            }
        }
        for mode in [
            SpreadMode::Single,
            SpreadMode::SplitLtr,
            SpreadMode::SplitRtl,
        ] {
            let mut rules = fixture.rules.clone();
            rules.mode = mode;
            assert_eq!(
                admit_pairing_action(
                    PairingContextToken {
                        rules: &rules,
                        availability
                    },
                    PairingAction::Nudge
                ),
                PairingAdmission::Accepted(PairingOperation::NavigationFallback)
            );
        }
    }
    assert_eq!(owner, fixture.live_owner(1, ShiftPersistence::SessionOnly));
    assert_eq!(service.watermarks(), before);
    assert_eq!(read_row(&service, &fixture.scope).row, original.row);
    assert!(
        service
            .drain_results()
            .iter()
            .all(|reply| reply.sequence <= before.admitted)
    );
    assert!(
        process
            .shutdown_and_join()
            .iter()
            .all(|reply| reply.status.is_ok())
    );
}

#[test]
fn joint_07_nested_only_copy_probe_and_import_coverage_publish_logical_scope_deltas() {
    use crate::rename_key_migration::{
        StoreCopyPathMapping, copy_stores_at, ledger_keys_with_restorable_rows_at,
    };
    let data = tempfile::tempdir().unwrap();
    let source = Path::new("C:/books/100%_book.zip");
    let target = Path::new("D:/books/100%_book.zip");
    let scope = BookScopeKey::archive(source, &["inner.zip/chapter".into()]).unwrap();
    let reference = SpreadShiftPageRef::ArchiveEntry("inner.zip/chapter/anchor.jpg".into());
    let db = data.path().join("spread_shift.db");
    initialize_database_at(&db).unwrap();
    let mut connection = store::open_existing(&db).unwrap().unwrap();
    store::set(&mut connection, &scope.storage_key(), &reference).unwrap();
    drop(connection);
    let source_key = crate::path_key::normalize_keep_drive(source);
    assert_eq!(
        ledger_keys_with_restorable_rows_at(
            data.path(),
            &[source_key.clone(), format!("{source_key}2")]
        ),
        std::collections::BTreeSet::from([source_key])
    );
    let report = copy_stores_at(
        data.path(),
        &[
            StoreCopyPathMapping::exact(source, target),
            StoreCopyPathMapping::virtual_prefix(source, target),
        ],
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.rows, 1);
    let copied = BookScopeKey::archive(target, &[scope.level_prefix.clone()]).unwrap();
    let connection = store::open_existing(&db).unwrap().unwrap();
    assert_eq!(
        store::load_rows(&connection)
            .unwrap()
            .get(&copied.storage_key()),
        Some(&reference)
    );
    drop(connection);
    let (service, process) = SpreadShiftService::spawn_at(data.path().into(), None).unwrap();
    let read = read_row(&service, &copied);
    let fixture = JointBookFixture {
        scope: copied,
        pages: vec![reference],
        rules: four_page_book("C:/unused").rules,
    };
    let reply = resolve_automatic(
        fixture.request(fixture.receipt(1)),
        Some(read),
        SpreadShiftState::None,
    );
    assert_eq!(reply.optional_shift.unwrap().anchor_idx, 0);
    assert!(
        process
            .shutdown_and_join()
            .iter()
            .all(|reply| reply.status.is_ok())
    );

    // Public transfer API: nonempty Family with omitted child clears only that root's child.
    let isolated = tempfile::tempdir().unwrap();
    let root = isolated.path().join("export-root");
    std::fs::create_dir_all(&root).unwrap();
    let book = root.join("book.zip");
    std::fs::write(&book, b"metadata-only fixture").unwrap();
    let data = isolated.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let db = data.join("spread_shift.db");
    initialize_database_at(&db).unwrap();
    let root_scope = BookScopeKey::document(&book).unwrap();
    let child = BookScopeKey::archive(&book, &["chapter".into()]).unwrap();
    let unrelated = BookScopeKey::archive(root.join("book.zip2"), &["chapter".into()]).unwrap();
    let exported = SpreadShiftPageRef::ArchiveEntry("a.jpg".into());
    let mut connection = store::open_existing(&db).unwrap().unwrap();
    store::set(&mut connection, &root_scope.storage_key(), &exported).unwrap();
    drop(connection);
    let cancel = std::sync::atomic::AtomicBool::new(false);
    crate::metadata_transfer::export_at(&data, &root, false, &cancel, |_| {}).unwrap();
    let mut connection = store::open_existing(&db).unwrap().unwrap();
    store::clear(&mut connection, &root_scope.storage_key()).unwrap();
    let old = SpreadShiftPageRef::ArchiveEntry("chapter/old.jpg".into());
    store::set(&mut connection, &child.storage_key(), &old).unwrap();
    store::set(&mut connection, &unrelated.storage_key(), &old).unwrap();
    drop(connection);
    let summary = crate::metadata_transfer::import_at(&data, &root, &cancel, |_| {}).unwrap();
    assert_eq!(summary.failed_entries, 0);
    assert_eq!(summary.committed.spread_shift_additions, 1);
    assert_eq!(summary.committed.spread_shift_removals, 1);
    assert_eq!(
        summary.committed.spread_shift_observation_roots,
        std::collections::BTreeSet::from([book.clone()])
    );
    assert_eq!(
        summary.committed.spread_shift_scopes,
        std::collections::BTreeSet::from([root_scope.storage_key(), child.storage_key()])
    );
    let connection = store::open_existing(&db).unwrap().unwrap();
    let rows = store::load_rows(&connection).unwrap();
    assert_eq!(rows.get(&root_scope.storage_key()), Some(&exported));
    assert!(!rows.contains_key(&child.storage_key()));
    assert_eq!(rows.get(&unrelated.storage_key()), Some(&old));
}

#[test]
fn joint_08_sibling_sessions_fork_and_ordinary_list_do_not_publish_writes() {
    let data = tempfile::tempdir().unwrap();
    let fixture = four_page_book(data.path().join("book"));
    let (service, process) = SpreadShiftService::spawn_at(data.path().into(), None).unwrap();
    let a = fixture.live_owner(1, ShiftPersistence::Saving(1));
    let b = fixture.live_owner(3, ShiftPersistence::SessionOnly);
    service
        .set_shift(
            fixture.scope.clone(),
            fixture.pages[1].clone(),
            operation(1, 1),
        )
        .unwrap();
    read_row(&service, &fixture.scope);
    assert_eq!(b.anchor_idx(), Some(3));
    service
        .set_shift(
            fixture.scope.clone(),
            fixture.pages[3].clone(),
            operation(2, 1),
        )
        .unwrap();
    let read = read_row(&service, &fixture.scope);
    assert_eq!(a.anchor_idx(), Some(1));
    let before = service.watermarks();
    let fork = a.fork_accepted();
    assert_eq!(fork.anchor_idx(), Some(1));
    assert!(matches!(
        fork.state,
        ViewerShift::Book(SpreadShiftState::Adjusted {
            persistence: ShiftPersistence::SessionOnly,
            ..
        })
    ));
    drop(fork);
    let mut ordinary = b.clone();
    ordinary.detach_book_scope_for_ordinary_list();
    assert_eq!(ordinary.scope, None);
    assert_eq!(ordinary.state, ViewerShift::Ordinary(3));
    assert_eq!(
        admit_pairing_action(
            PairingContextToken {
                rules: &fixture.rules,
                availability: PairingEditAvailability::Automatic
            },
            PairingAction::Nudge
        ),
        PairingAdmission::Accepted(PairingOperation::EditShift)
    );
    let mut request = fixture.request(RestoreReceipt::capture(
        11,
        ViewerContextId::for_test(3),
        fixture.scope.clone(),
        4,
        5,
        &service,
        None,
    ));
    let reopened = resolve_automatic(request.clone(), Some(read), SpreadShiftState::None);
    assert_eq!(reopened.optional_shift.unwrap().anchor_idx, 3);
    request.layout_eligibility = PairingEditAvailability::ExtractedResult;
    assert!(matches!(
        resolve_book_layout_source::<()>(PreparedLayoutRecipe::ExtractedResult, request).unwrap(),
        BookLayoutSource::ExtractedResult
    ));
    assert_eq!(service.watermarks(), before);
    assert_eq!(a.anchor_idx(), Some(1));
    assert_eq!(b.anchor_idx(), Some(3));
    assert!(
        process
            .shutdown_and_join()
            .iter()
            .all(|reply| reply.status.is_ok())
    );
}
