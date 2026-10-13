#![cfg(test)]

use super::*;

#[test]
fn deferred_projection_wait_is_retired_by_close_source_replacement_and_fork() {
    for close in ["replace", "manual", "rating"] {
        let mut env = crate::app::setup_app_for_test();
        let scope = crate::book_scope::BookScopeKey::document("C:/waiting.pdf").unwrap();
        let (_deliver, receiver) = mpsc::channel();
        env.book_composition =
            super::super::book_composition_runtime::ViewerBookComposition::Preparing {
                scope: scope.clone(),
                receiver,
                origin: Default::default(),
            };
        let deferred = DeferredFsReopen {
            history_trigger: HistoryTrigger::UserChosen,
            resume_slideshow: false,
            target: super::super::DeferredFsTarget::None,
            resume_to_last_page: false,
            from_explicit_open: true,
            preserve_after_password_prompt: true,
        };
        env.defer_book_projection_open(deferred.clone());
        assert!(env.book_deferred_open_wait_active());
        let fork = env.book_composition_permission.clone();
        assert!(!fork.pending(), "a fork cannot inherit an unfinished open");

        // The separately owned sibling may be waiting for the same book, but
        // closing or replacing this source must not consume its continuation.
        let mut sibling = ViewerPresentationPermission::default();
        sibling.open = OpenPermission::AwaitingProjection {
            context_id: 999,
            items_generation: 42,
            scope,
            deferred,
        };
        match close {
            "manual" => env.close_fullscreen(),
            "rating" => env.close_fullscreen_for_rating_membership(),
            _ => env.cancel_pending_folder_nav(),
        }
        assert!(!env.book_deferred_open_wait_active());
        assert!(matches!(
            &env.book_composition_permission.open,
            OpenPermission::Idle
        ));
        assert!(matches!(
            &sibling.open,
            OpenPermission::AwaitingProjection {
                context_id: 999,
                items_generation: 42,
                ..
            }
        ));
        assert!(sibling.pending());
    }
}

#[test]
fn deferred_projection_carry_requires_original_context_generation_and_scope() {
    for changed in ["none", "context", "generation", "scope"] {
        let mut env = crate::app::setup_app_for_test();
        let scope = crate::book_scope::BookScopeKey::document("C:/waiting.pdf").unwrap();
        let context_id = env.spread_shift_context_id().serial();
        let old_generation = env.items_generation;
        let deferred = DeferredFsReopen {
            history_trigger: HistoryTrigger::UserChosen,
            resume_slideshow: false,
            target: super::super::DeferredFsTarget::None,
            resume_to_last_page: false,
            from_explicit_open: true,
            preserve_after_password_prompt: true,
        };
        env.book_composition_permission.open = OpenPermission::AwaitingProjection {
            context_id: if changed == "context" {
                context_id + 1
            } else {
                context_id
            },
            items_generation: if changed == "generation" {
                old_generation + 1
            } else {
                old_generation
            },
            scope: if changed == "scope" {
                crate::book_scope::BookScopeKey::document("C:/sibling.pdf").unwrap()
            } else {
                scope.clone()
            },
            deferred,
        };
        env.items_generation += 1;
        let new_generation = env.items_generation;
        env.retire_book_permission_after_projection(&scope, old_generation, new_generation);
        assert_eq!(env.book_deferred_open_wait_active(), changed == "none");
        if changed == "none" {
            assert!(matches!(&env.book_composition_permission.open,
                OpenPermission::AwaitingProjection { items_generation, .. }
                    if *items_generation == new_generation
            ));
        } else {
            assert!(matches!(
                &env.book_composition_permission.open,
                OpenPermission::Idle
            ));
        }
    }
}

fn occurrence(idx: usize) -> PresentationOccurrence {
    PresentationOccurrence {
        item_index: idx,
        navigation_anchor: 0,
        role: PresentationRole::Navigation,
        slice: PresentationSlice::Full,
        slot: if idx == 0 {
            PresentationSlot::Left
        } else {
            PresentationSlot::Right
        },
    }
}

#[test]
fn fullscreen_open_captures_rating_metadata_boundary_in_composition_continuation() {
    for boundary in [
        FsMetadataBoundary::Manual,
        FsMetadataBoundary::RatingMembership,
    ] {
        let mut env = crate::app::tests::phase_c_support::setup_app();
        let root = env.tmp.path().join("metadata-boundary.pdf");
        env.items = vec![crate::grid_item::GridItem::PdfPage {
            pdf_path: root,
            page_num: 0,
            content_type: None,
        }];
        env.visible_indices = vec![0];
        env.thumbnails = vec![crate::grid_item::ThumbnailState::Pending];
        env.image_metas = vec![None];
        let (service, process) =
            BookCompositionService::spawn_at(env.tmp.path().join("permission"), None).unwrap();
        env.install_book_composition_runtime(service);
        assert!(
            env.defer_book_composition_fullscreen_open_with_metadata_boundary(
                0,
                HistoryTrigger::UserChosen,
                FsOpenMaterialization::Eager,
                FsPageLoadContract::Sequential,
                BookTargetPurpose::Explicit,
                boundary
            )
        );
        let OpenPermission::Checking { intent, .. } = &env.book_composition_permission.open else {
            panic!("open must carry its own qualified continuation");
        };
        assert_eq!(intent.route, BookTargetRoute::FullscreenOpen(boundary));
        assert_eq!(
            intent.clone().route,
            BookTargetRoute::FullscreenOpen(boundary)
        );
        assert_eq!(env.fullscreen_idx, None);
        process.shutdown_and_join();
    }
}
fn hidden() -> CandidatePermission {
    CandidatePermission::Hidden(ScopePermissionToken {
        scope: crate::book_scope::BookScopeKey::document("C:/book.pdf").unwrap(),
        token: CompositionToken {
            service_epoch: 1,
            publication_sequence: 1,
        },
    })
}

fn assert_image_presentation_does_not_qualify(app: &mut crate::app::App) {
    let sequence = app.book_composition_permission.sequence;
    assert!(app.presentation_candidate(0).is_none());
    let demand = [occurrence(0)];
    app.retain_book_presentation_demand(&demand);
    assert!(matches!(
        app.book_composition_permission.occurrences.get(&demand[0]),
        Some(OccurrencePermission::Complete(
            CandidatePermission::Allowed(None)
        ))
    ));
    assert_eq!(
        app.book_presentation_current(&demand),
        PresentationDecision::Allowed(vec![true])
    );
    assert!(!app.defer_book_composition_fullscreen_open(
        0,
        HistoryTrigger::UserChosen,
        FsOpenMaterialization::Eager,
        FsPageLoadContract::Sequential,
        BookTargetPurpose::Explicit,
    ));
    assert_eq!(
        app.book_composition_permission.sequence, sequence,
        "ordinary images do not send candidate requests to the composition worker"
    );
    assert!(matches!(
        app.book_composition_permission.open,
        OpenPermission::Idle
    ));
}

#[test]
fn saved_folder_hidden_row_is_ignored_by_book_ordinary_and_extracted_image_consumers() {
    use crate::app::book_materialization::{BookScope, ExtractedSurface, ProjectionRecipe};
    let mut env = crate::app::tests::phase_c_support::setup_app();
    let root = env.tmp.path().join("plain-folder");
    std::fs::create_dir(&root).unwrap();
    let path = root.join("hidden.png");
    image::RgbaImage::new(8, 12).save(&path).unwrap();
    env.items = vec![crate::grid_item::GridItem::Image(path.clone())];
    env.image_metas = vec![None];
    env.thumbnails = vec![crate::grid_item::ThumbnailState::Pending];
    env.visible_indices = vec![0];
    env.current_folder = Some(root.clone());
    env.fullscreen_idx = Some(0);
    env.selected = Some(0);
    let index = std::sync::Arc::new(
        crate::app::file_type_resume::PreparedPageIdentityIndex::prepare(
            &env.items,
            Some(&root),
            env.settings.file_type_visibility.clone(),
        ),
    );
    env.page_identity_index = index;
    let (service, owner) =
        BookCompositionService::spawn_at(env.tmp.path().join("folder-permission-data"), None)
            .unwrap();
    env.install_book_composition_runtime(service.clone());
    let scope = crate::book_scope::BookScopeKey::folder(&root).unwrap();
    let page = CompositionPageRef::RelativePath {
        path: "hidden.png".into(),
    };
    let mut record = BookComposition::new(ReadingDirection::Rtl, vec![page.clone()]);
    record.hidden_pages.push(page);
    service
        .save(scope.clone(), PageDomain::ImageFolder, record)
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    let snapshot = service.snapshot();
    let row = snapshot.rows[&scope.storage_key()].clone();
    let token = snapshot.token_for(&scope);
    assert!(
        matches!(
            candidate_permission(
                &snapshot,
                &CandidatePage {
                    identity: crate::book_resume_db::PageIdentity::Image {
                        path: crate::path_key::normalize_keep_drive(&path),
                    },
                    physical_archive: None,
                    raw_tree: None,
                }
            )
            .unwrap(),
            CandidatePermission::Hidden(_)
        ),
        "preserve the saved row and the pure folder permission model"
    );
    for recipe in [
        ProjectionRecipe::Book(BookScope::ImageFolder(root.clone())),
        ProjectionRecipe::Book(BookScope::CompiledFolder(root.clone())),
        ProjectionRecipe::OrdinaryList,
        ProjectionRecipe::ExtractedResult(ExtractedSurface::Snapshot),
        ProjectionRecipe::ExtractedResult(ExtractedSurface::Rating),
    ] {
        env.projection_recipe = recipe;
        env.book_composition_permission = ViewerPresentationPermission::default();
        assert_image_presentation_does_not_qualify(&mut env);
    }
    #[cfg(windows)]
    {
        env.projection_recipe = ProjectionRecipe::Book(BookScope::ImageFolder(root.clone()));
        let detached = env.fork_materialized_still_context(&root, 0);
        env.with_window_context_for_test(detached, |app| {
            app.book_composition_permission = ViewerPresentationPermission::default();
            assert_image_presentation_does_not_qualify(app);
        });
        env.retire_context(
            detached,
            "folder_composition_excluded_permission_test",
            |_| (),
        )
        .unwrap();
        assert_image_presentation_does_not_qualify(&mut env);
    }
    let current = service.snapshot();
    assert_eq!(current.token_for(&scope), token);
    assert!(std::sync::Arc::ptr_eq(
        &current.rows[&scope.storage_key()],
        &row
    ));
    owner.shutdown_and_join();
}

#[test]
fn pair_projection_waits_atomically_and_retains_original_slot_mask() {
    let mut owner = ViewerPresentationPermission::default();
    let left = occurrence(0);
    let right = occurrence(1);
    owner.occurrences.insert(
        left,
        OccurrencePermission::Complete(CandidatePermission::Allowed(None)),
    );
    assert_eq!(
        owner.decision(&[left, right]),
        PresentationDecision::Checking
    );
    owner
        .occurrences
        .insert(right, OccurrencePermission::Complete(hidden()));
    assert_eq!(
        owner.decision(&[left, right]),
        PresentationDecision::Allowed(vec![true, false])
    );
    assert_eq!(
        owner.decision(&[left]),
        PresentationDecision::Allowed(vec![true])
    );
    owner
        .occurrences
        .insert(left, OccurrencePermission::Complete(hidden()));
    assert_eq!(owner.decision(&[left, right]), PresentationDecision::Hidden);
}
#[test]
fn read_only_fork_copies_only_allowed_episodes_without_pending_or_hidden() {
    let mut owner = ViewerPresentationPermission::default();
    owner.bind(2, 3);
    owner.occurrences.insert(
        occurrence(0),
        OccurrencePermission::Complete(CandidatePermission::Allowed(None)),
    );
    owner
        .occurrences
        .insert(occurrence(1), OccurrencePermission::Complete(hidden()));
    let mut sibling = owner.clone();
    sibling.bind(8, 3);
    assert_eq!(
        sibling.decision(&[occurrence(0)]),
        PresentationDecision::Allowed(vec![true])
    );
    assert_eq!(
        sibling.decision(&[occurrence(1)]),
        PresentationDecision::Checking
    );
    sibling.bind(8, 4);
    assert_eq!(
        sibling.decision(&[occurrence(0)]),
        PresentationDecision::Checking
    );
    assert_eq!(
        owner.decision(&[occurrence(0)]),
        PresentationDecision::Allowed(vec![true])
    );
}
#[test]
fn role_slot_slice_and_anchor_are_independent_new_presentation_demands() {
    let mut owner = ViewerPresentationPermission::default();
    let source = occurrence(0);
    owner.occurrences.insert(
        source,
        OccurrencePermission::Complete(CandidatePermission::Allowed(None)),
    );
    for changed in [
        PresentationOccurrence {
            role: PresentationRole::Supplement,
            ..source
        },
        PresentationOccurrence {
            navigation_anchor: 10,
            ..source
        },
        PresentationOccurrence {
            slice: PresentationSlice::Right,
            ..source
        },
        PresentationOccurrence {
            slot: PresentationSlot::Center,
            ..source
        },
    ] {
        assert_eq!(owner.decision(&[changed]), PresentationDecision::Checking);
    }
}
#[test]
fn retired_receiver_reply_cannot_grant_a_reentered_episode_or_sibling() {
    let mut owner = ViewerPresentationPermission::default();
    owner.bind(2, 3);
    let candidate = CandidatePage {
        identity: crate::book_resume_db::PageIdentity::Document {
            container: "C:/book.pdf".into(),
            page_num: 0,
        },
        physical_archive: None,
        raw_tree: None,
    };
    let old = owner.correlation(occurrence(0), &candidate);
    let (tx, rx) = mpsc::channel();
    owner.occurrences.insert(
        occurrence(0),
        OccurrencePermission::Checking {
            correlation: old.clone(),
            candidate: candidate.clone(),
            receiver: rx,
        },
    );
    let sibling = owner.clone();
    assert_eq!(
        sibling.decision(&[occurrence(0)]),
        PresentationDecision::Checking
    );
    owner.occurrences.clear();
    assert!(
        tx.send(CandidatePermissionReply {
            correlation: old.clone(),
            status: Ok(CandidatePermission::Allowed(None))
        })
        .is_err()
    );
    let next = owner.correlation(occurrence(0), &candidate);
    assert_ne!(old.request_id, next.request_id);
    assert_eq!(
        owner.decision(&[occurrence(0)]),
        PresentationDecision::Checking
    );
}

#[test]
fn navigation_open_waits_for_atomic_unit_then_requalifies_same_unit_survivor() {
    let mut env = crate::app::tests::phase_c_support::setup_app();
    let root = env.tmp.path().join("atomic-open.pdf");
    env.items = (0..2)
        .map(|page_num| crate::grid_item::GridItem::PdfPage {
            pdf_path: root.clone(),
            page_num,
            content_type: None,
        })
        .collect();
    env.current_folder = Some(root.clone());
    env.fullscreen_idx = Some(0);
    env.selected = Some(0);
    env.spread_mode = crate::settings::SpreadMode::Ltr;
    let (service, process) =
        BookCompositionService::spawn_at(env.tmp.path().join("permission"), None).unwrap();
    env.install_book_composition_runtime(service.clone());
    let context_id = env.spread_shift_context_id().serial();
    let generation = env.items_generation;
    env.book_composition_permission.bind(context_id, generation);
    let scope = crate::book_scope::BookScopeKey::document(&root).unwrap();
    service
        .prepare(scope.clone(), PageDomain::Document)
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    let proof = ScopePermissionToken {
        token: service.snapshot().token_for(&scope),
        scope,
    };
    let mut candidates = Vec::new();
    let mut deliveries = Vec::new();
    for idx in 0..2 {
        let candidate = env.presentation_candidate(idx).unwrap();
        let correlation = env
            .book_composition_permission
            .correlation(occurrence(idx), &candidate);
        let (send, receiver) = mpsc::channel();
        deliveries.push((send, correlation.clone()));
        candidates.push(OpenCandidate {
            correlation,
            candidate,
            state: OpenCandidateState::Checking(receiver),
        });
    }
    env.book_composition_permission.open = OpenPermission::Checking {
        intent: OpenIntent {
            context_id,
            items_generation: generation,
            idx: 0,
            history_trigger: HistoryTrigger::UserChosen,
            materialization: FsOpenMaterialization::NavigationTargetMaterializationOnly,
            contract: FsPageLoadContract::Sequential,
            purpose: BookTargetPurpose::Next,
            route: BookTargetRoute::FullscreenNavigation,
        },
        candidates,
    };
    deliveries[0]
        .0
        .send(CandidatePermissionReply {
            correlation: deliveries[0].1.clone(),
            status: Ok(CandidatePermission::Hidden(proof.clone())),
        })
        .unwrap();
    env.poll_book_presentation_permission(&egui::Context::default());
    assert!(
        matches!(&env.book_composition_permission.open, OpenPermission::Checking { intent, .. } if intent.idx == 0)
    );
    assert_eq!(env.selected, Some(0));
    assert_eq!(env.fullscreen_idx, Some(0));
    deliveries[1]
        .0
        .send(CandidatePermissionReply {
            correlation: deliveries[1].1.clone(),
            status: Ok(CandidatePermission::Allowed(Some(proof))),
        })
        .unwrap();
    env.poll_book_presentation_permission(&egui::Context::default());
    assert!(
        matches!(&env.book_composition_permission.open, OpenPermission::Checking { intent, .. } if intent.idx == 1),
        "permitted trailing real stays in the same unit and obtains a fresh anchor episode"
    );
    assert_eq!(env.selected, Some(0));
    assert_eq!(env.fullscreen_idx, Some(0));
    assert_eq!(env.items_generation, generation);
    process.shutdown_and_join();
}

#[test]
fn external_identity_permission_refuses_hidden_before_source_or_selection_changes() {
    use crate::book_composition::{
        PageDomain, ReadingDirection, fixtures::document_page, fixtures::four_page_book,
    };
    let mut env = crate::app::tests::phase_c_support::setup_app();
    let root = env.tmp.path().join("external-hidden.pdf");
    let scope = crate::book_scope::BookScopeKey::document(&root).unwrap();
    let (service, process) =
        BookCompositionService::spawn_at(env.tmp.path().join("identity-permission"), None).unwrap();
    let mut record = four_page_book(ReadingDirection::Ltr);
    record.hidden_pages.push(document_page(1));
    service
        .save(scope.clone(), PageDomain::Document, record)
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    env.install_book_composition_runtime(service.clone());
    let before = (
        env.items.clone(),
        env.current_folder.clone(),
        env.selected,
        env.fullscreen_idx,
        env.items_generation,
    );
    let candidate = CandidatePage {
        identity: document_page(1).resume_identity(&root),
        physical_archive: None,
        raw_tree: None,
    };
    let context_id = env.spread_shift_context_id().serial();
    let generation = env.items_generation;
    env.book_composition_permission.bind(context_id, generation);
    let correlation = env
        .book_composition_permission
        .correlation_for(CandidateOccurrence::IdentityOnly, &candidate);
    let (deliver, receiver) = mpsc::channel();
    env.book_composition_permission.open = OpenPermission::IdentityChecking {
        operation_id: 91,
        correlation: correlation.clone(),
        candidate: candidate.clone(),
        receiver,
    };
    let ctx = egui::Context::default();
    assert_eq!(
        env.qualify_book_identity_permission(&ctx, 91, candidate.clone()),
        BookIdentityPermission::Checking
    );
    assert_eq!(
        (
            env.items.clone(),
            env.current_folder.clone(),
            env.selected,
            env.fullscreen_idx,
            env.items_generation
        ),
        before
    );
    let reply = service
        .check_candidate(correlation, candidate.clone())
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap();
    deliver.send(reply).unwrap();
    assert_eq!(
        env.qualify_book_identity_permission(&ctx, 91, candidate.clone()),
        BookIdentityPermission::Hidden
    );
    assert!(env.fs_feedback_toast.is_some());
    env.fs_feedback_toast = None;
    assert_eq!(
        env.qualify_book_identity_permission(&ctx, 91, candidate.clone()),
        BookIdentityPermission::Hidden
    );
    assert!(
        env.fs_feedback_toast.is_none(),
        "one refused external request publishes one notice"
    );
    assert_eq!(
        (
            env.items.clone(),
            env.current_folder.clone(),
            env.selected,
            env.fullscreen_idx,
            env.items_generation
        ),
        before
    );
    service
        .remove(scope, PageDomain::Document)
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    let mut result = env.qualify_book_identity_permission(&ctx, 91, candidate.clone());
    if result == BookIdentityPermission::Checking {
        let state = std::mem::take(&mut env.book_composition_permission.open);
        let OpenPermission::IdentityChecking {
            operation_id,
            correlation,
            candidate: captured,
            receiver,
        } = state
        else {
            panic!("identity requalification request");
        };
        let reply = receiver
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap();
        let (deliver, receiver) = mpsc::channel();
        deliver.send(reply).unwrap();
        env.book_composition_permission.open = OpenPermission::IdentityChecking {
            operation_id,
            correlation,
            candidate: captured,
            receiver,
        };
        result = env.qualify_book_identity_permission(&ctx, 91, candidate);
    }
    assert_eq!(
        result,
        BookIdentityPermission::Allowed,
        "a new publication requalifies the same logical external identity"
    );
    env.book_composition_permission.occurrences.insert(
        occurrence(0),
        OccurrencePermission::Complete(CandidatePermission::Allowed(None)),
    );
    env.retire_book_identity_permission(90);
    assert!(matches!(
        &env.book_composition_permission.open,
        OpenPermission::IdentityComplete {
            operation_id: 91,
            ..
        }
    ));
    env.retire_book_identity_permission(91);
    assert!(matches!(
        &env.book_composition_permission.open,
        OpenPermission::Idle
    ));
    assert_eq!(
        env.book_composition_permission.decision(&[occurrence(0)]),
        PresentationDecision::Allowed(vec![true])
    );
    process.shutdown_and_join();
}

#[test]
fn hidden_navigation_keeps_captured_identity_until_projection_order_is_ready() {
    let mut env = crate::app::tests::phase_c_support::setup_app();
    let root = env.tmp.path().join("hidden-order.pdf");
    env.current_folder = Some(root.clone());
    env.items = (0..2)
        .map(|page_num| crate::grid_item::GridItem::PdfPage {
            pdf_path: root.clone(),
            page_num,
            content_type: None,
        })
        .collect();
    env.fullscreen_idx = Some(0);
    env.selected = Some(0);
    let (service, process) =
        BookCompositionService::spawn_at(env.tmp.path().join("hidden-order-data"), None).unwrap();
    env.install_book_composition_runtime(service.clone());
    let scope = crate::book_scope::BookScopeKey::document(&root).unwrap();
    service
        .prepare(scope.clone(), PageDomain::Document)
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    let proof = ScopePermissionToken {
        token: service.snapshot().token_for(&scope),
        scope: scope.clone(),
    };
    let context_id = env.spread_shift_context_id().serial();
    let generation = env.items_generation;
    env.book_composition_permission.bind(context_id, generation);
    let mut candidates = Vec::new();
    for idx in 0..2 {
        let candidate = env.presentation_candidate(idx).unwrap();
        let correlation = env
            .book_composition_permission
            .correlation(occurrence(idx), &candidate);
        let permission = if idx == 0 {
            CandidatePermission::Hidden(proof.clone())
        } else {
            CandidatePermission::Allowed(Some(proof.clone()))
        };
        candidates.push(OpenCandidate {
            correlation,
            candidate,
            state: OpenCandidateState::Complete(permission),
        });
    }
    env.book_composition_permission.open = OpenPermission::Checking {
        intent: OpenIntent {
            context_id,
            items_generation: env.items_generation,
            idx: 0,
            history_trigger: HistoryTrigger::UserChosen,
            materialization: FsOpenMaterialization::NavigationTargetMaterializationOnly,
            contract: FsPageLoadContract::Sequential,
            purpose: BookTargetPurpose::Next,
            route: BookTargetRoute::FullscreenNavigation,
        },
        candidates,
    };
    let (_hold, receiver) = mpsc::channel();
    env.book_composition = crate::app::book_composition_runtime::ViewerBookComposition::Preparing {
        scope,
        receiver,
        origin: Default::default(),
    };
    env.poll_book_presentation_permission(&egui::Context::default());
    assert!(
        matches!(&env.book_composition_permission.open, OpenPermission::Ready { intent, candidates } if intent.idx == 0 && candidates[0].correlation.identity == crate::book_composition::fixtures::document_page(0).resume_identity(&root))
    );
    assert_eq!(env.fullscreen_idx, Some(0));
    assert_eq!(env.selected, Some(0));
    process.shutdown_and_join();
}

#[test]
fn unreadable_projection_cannot_turn_cached_candidate_allowance_into_legacy_paint() {
    let mut env = crate::app::tests::phase_c_support::setup_app();
    let root = env.tmp.path().join("blocked-projection.pdf");
    env.current_folder = Some(root.clone());
    env.items = vec![crate::grid_item::GridItem::PdfPage {
        pdf_path: root.clone(),
        page_num: 0,
        content_type: None,
    }];
    env.projection_recipe = crate::app::book_materialization::ProjectionRecipe::Book(
        crate::app::book_materialization::BookScope::Document(root.clone()),
    );
    let scope = crate::book_scope::BookScopeKey::document(&root).unwrap();
    let (service, process) =
        BookCompositionService::spawn_at(env.tmp.path().join("blocked-projection-data"), None)
            .unwrap();
    env.install_book_composition_runtime(service.clone());
    let token = service.snapshot().token_for(&scope);
    env.book_composition = crate::app::book_composition_runtime::ViewerBookComposition::Blocked {
        scope,
        token,
        error: "projection could not be resolved".into(),
    };
    let demand = [occurrence(0)];
    env.install_allowed_presentation_for_test(&demand);
    assert_eq!(
        env.book_presentation_current(&demand),
        PresentationDecision::Checking
    );
    let before = (env.selected, env.fullscreen_idx, env.items_generation);
    assert!(env.defer_book_composition_fullscreen_open(
        0,
        HistoryTrigger::UserChosen,
        FsOpenMaterialization::Eager,
        FsPageLoadContract::Sequential,
        BookTargetPurpose::Explicit
    ));
    assert_eq!(
        (env.selected, env.fullscreen_idx, env.items_generation),
        before
    );
    assert!(matches!(
        env.book_composition_permission.open,
        OpenPermission::Idle
    ));
    process.shutdown_and_join();
}

#[test]
fn external_bookmark_handler_qualifies_hidden_identity_before_installing_source() {
    use crate::book_bookmarks::{BookBookmark, BookContainerKind, PageIdentity};
    use crate::book_composition::{
        PageDomain, ReadingDirection,
        fixtures::{document_page, four_page_book},
    };
    use crate::bookmark_browser::{BookmarkBrowserRow, BookmarkRowSource};
    let mut env = crate::app::tests::phase_c_support::setup_app();
    let root = env.tmp.path().join("external-bookmark.pdf");
    std::fs::write(&root, b"hidden bookmark never starts document loading").unwrap();
    let scope = crate::book_scope::BookScopeKey::document(&root).unwrap();
    let (service, process) =
        BookCompositionService::spawn_at(env.tmp.path().join("external-bookmark-data"), None)
            .unwrap();
    let mut record = four_page_book(ReadingDirection::Ltr);
    record.hidden_pages.push(document_page(1));
    service
        .save(scope, PageDomain::Document, record)
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    env.install_book_composition_runtime(service.clone());
    let bookmark = BookBookmark {
        id: 71,
        container_key: crate::path_key::normalize(&root),
        container_path: root.clone(),
        container_kind: BookContainerKind::Pdf,
        page_identity: PageIdentity::PdfPage(1),
        page_index_hint: 99,
        created_at_ms: 10,
        title: Some("saved external target".into()),
    };
    let row = BookmarkBrowserRow {
        source: BookmarkRowSource::Book(bookmark.clone()),
        item: crate::grid_item::GridItem::PdfPage {
            pdf_path: root.clone(),
            page_num: 1,
            content_type: None,
        },
        relative_page_provenance: None,
        image_meta: None,
        marker_thumbnail: None,
        created_at_ms: 10,
        missing: false,
    };
    let before = (
        env.items.clone(),
        env.current_folder.clone(),
        env.selected,
        env.fullscreen_idx,
        env.items_generation,
        env.bookmark_view_state.clone(),
    );
    let candidate = CandidatePage {
        identity: document_page(1).resume_identity(&root),
        physical_archive: None,
        raw_tree: None,
    };
    let context_id = env.spread_shift_context_id().serial();
    let generation = env.items_generation;
    env.book_composition_permission.bind(context_id, generation);
    let correlation = env
        .book_composition_permission
        .correlation_for(CandidateOccurrence::IdentityOnly, &candidate);
    let operation_id = env.bookmark_open_request_seq + 1;
    let (deliver, receiver) = mpsc::channel();
    env.book_composition_permission.open = OpenPermission::IdentityChecking {
        operation_id,
        correlation: correlation.clone(),
        candidate: candidate.clone(),
        receiver,
    };
    let ctx = egui::Context::default();
    env.open_bookmark_browser_row_classified_with_navigation(&ctx, &row, None);
    assert!(env.bookmark_open_pending.is_some());
    assert_eq!(
        (
            env.items.clone(),
            env.current_folder.clone(),
            env.selected,
            env.fullscreen_idx,
            env.items_generation,
            env.bookmark_view_state.clone()
        ),
        before
    );
    deliver
        .send(
            service
                .check_candidate(correlation, candidate)
                .recv_timeout(std::time::Duration::from_secs(30))
                .unwrap(),
        )
        .unwrap();
    env.poll_bookmark_book_open(&ctx);
    assert!(env.bookmark_open_pending.is_none());
    assert!(matches!(
        env.book_composition_permission.open,
        OpenPermission::Idle
    ));
    assert_eq!(
        (
            env.items.clone(),
            env.current_folder.clone(),
            env.selected,
            env.fullscreen_idx,
            env.items_generation,
            env.bookmark_view_state.clone()
        ),
        before
    );
    assert_eq!(row.source, BookmarkRowSource::Book(bookmark));
    assert!(env.fs_feedback_toast.is_some());
    process.shutdown_and_join();
}

#[test]
fn all_hidden_zip_level_keeps_dfs_continuation_for_manual_and_slideshow() {
    use crate::app::{
        book_composition_runtime::{
            BookCompositionSource, BookProjectionReply, ProjectionAdoptionCause, ProjectionOrigin,
            ViewerBookComposition,
        },
        book_materialization::{
            BookScope, ProjectionRecipe, materialize_archive_level, project_archive_composition,
        },
        file_type_resume::PreparedPageIdentityIndex,
    };
    use crate::book_composition::{
        BookComposition, CompositionPageRef, PageDomain, ReadingDirection,
    };
    use crate::grid_item::ThumbnailState;
    use std::{
        io::Write,
        sync::Arc,
        time::{Duration, Instant},
    };
    for slideshow in [false, true] {
        let mut env = crate::app::tests::phase_c_support::setup_app();
        let root = env.tmp.path().join("dfs.zip");
        let names = ["a-old/a.jpg", "b-hidden/b.jpg", "c-show/c.jpg"];
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&root).unwrap());
        for name in names {
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer
                .write_all(b"fixture navigation; no paint or texture work")
                .unwrap();
        }
        writer.finish().unwrap();
        let tree = Arc::new(crate::zip_tree::ZipTree::build(
            root.clone(),
            names
                .into_iter()
                .map(|name| crate::zip_loader::ZipImageEntry {
                    entry_name: name.into(),
                    uncompressed_size: 0,
                    mtime: 0,
                    container_ancestry: vec![],
                })
                .collect(),
        ));
        let prefix = vec!["b-hidden".to_owned()];
        let nav = crate::zip_tree::ZipNavState::restore_prefix(tree.clone(), "b-hidden").unwrap();
        let policy = env.settings.file_type_visibility.clone();
        let (items, metas) = materialize_archive_level(
            &tree,
            &prefix,
            crate::app::BOOK_READING_PAGE_ORDER,
            Some(&env.settings.grid_display_order),
        );
        env.current_folder = Some(root.clone());
        env.zip_nav = Some(nav);
        env.items = items.clone();
        env.image_metas = metas.clone();
        env.thumbnails = vec![ThumbnailState::Pending; items.len()];
        env.visible_indices = (0..items.len()).collect();
        env.page_identity_index = Arc::new(PreparedPageIdentityIndex::prepare(
            &items,
            Some(&root),
            policy.clone(),
        ));
        env.projection_recipe = ProjectionRecipe::Book(BookScope::Archive {
            logical_root: root.clone(),
            prefix: prefix.clone(),
        });
        env.fullscreen_idx = Some(0); // Previous shown unit waits while this level adopts.
        env.selected = Some(0);
        env.slideshow_playing = slideshow;
        env.slideshow_anchor_idx = Some(0);
        let old_timer = Instant::now()
            .checked_sub(Duration::from_secs(3600))
            .unwrap();
        env.slideshow_next_at = old_timer;
        let scope = crate::book_scope::BookScopeKey::archive(&root, &prefix).unwrap();
        let page = CompositionPageRef::ArchiveEntry {
            entry_name: "b-hidden/b.jpg".into(),
        };
        let mut composition = BookComposition::new(ReadingDirection::Ltr, vec![page.clone()]);
        composition.hidden_pages.push(page);
        let (service, process) =
            BookCompositionService::spawn_at(env.tmp.path().join("dfs-data"), None).unwrap();
        service
            .save(
                scope.clone(),
                PageDomain::ArchiveLevel {
                    prefix: "b-hidden".into(),
                },
                composition,
            )
            .recv_timeout(Duration::from_secs(30))
            .unwrap()
            .status
            .unwrap();
        env.install_book_composition_runtime(service.clone());
        let snapshot = service.snapshot();
        let record = snapshot.rows[&scope.storage_key()].clone();
        let token = snapshot.token_for(&scope);
        let rows = project_archive_composition(
            &tree,
            &root,
            &prefix,
            (items, metas),
            crate::app::BOOK_READING_PAGE_ORDER,
            Some(&env.settings.grid_display_order),
            policy,
            token,
            Some(&record.composition),
        )
        .unwrap();
        assert!(rows.items.is_empty());
        let context_id = env.spread_shift_context_id().serial();
        let generation = env.items_generation;
        env.book_composition_permission.bind(context_id, generation);
        let demand = env.book_target_presentation_demand(0);
        let candidates = demand
            .into_iter()
            .map(|occurrence| {
                let candidate = env.presentation_candidate(occurrence.item_index).unwrap();
                let correlation = env
                    .book_composition_permission
                    .correlation(occurrence, &candidate);
                let permission =
                    crate::book_composition::candidate_permission(&snapshot, &candidate).unwrap();
                assert!(matches!(permission, CandidatePermission::Hidden(_)));
                OpenCandidate {
                    correlation,
                    candidate,
                    state: OpenCandidateState::Complete(permission),
                }
            })
            .collect();
        let continuation = if slideshow {
            BookDfsContinuation::Slideshow
        } else {
            BookDfsContinuation::Navigation { forward: true }
        };
        env.book_composition_permission.open = OpenPermission::Ready {
            intent: OpenIntent {
                context_id,
                items_generation: generation,
                idx: 0,
                history_trigger: HistoryTrigger::UserChosen,
                materialization: FsOpenMaterialization::Eager,
                contract: FsPageLoadContract::Sequential,
                purpose: BookTargetPurpose::FirstInBook,
                route: BookTargetRoute::ZipDfs(continuation),
            },
            candidates,
        };
        let (deliver, receiver) = mpsc::channel();
        assert!(
            deliver
                .send(Ok(BookProjectionReply {
                    rows,
                    landing: None,
                    edits: None,
                    automatic_preferences: None,
                    refresh_landings: HashMap::new(),
                    saved_trim: None
                }))
                .is_ok()
        );
        env.book_composition = ViewerBookComposition::Projecting {
            scope,
            token,
            record: Some(record),
            source: BookCompositionSource::Archive {
                tree: tree.clone(),
                prefix: prefix.clone(),
                display_prefix: prefix,
            },
            generation,
            origin: Box::new(ProjectionOrigin::Fresh),
            receiver,
        };
        let ctx = egui::Context::default();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            env.poll_book_composition_read(ctx)
        });
        assert_eq!(
            env.zip_nav.as_ref().unwrap().current(),
            &["c-show".to_owned()],
            "all-hidden adoption must not close and retire the DFS continuation"
        );
        assert_eq!(
            env.slideshow_next_at, old_timer,
            "an unqualified next level cannot restart the slideshow from the old numeric index"
        );
        assert_eq!(
            env.slideshow_anchor_idx,
            if slideshow { Some(0) } else { None },
            "manual Ctrl-navigation clears the stopped slideshow anchor; automatic DFS retains its unscheduled anchor while checking"
        );

        // Keep the next level's real permission worker, and deliver its exact
        // completion while bypassing unrelated sidecar hydration in this fixture.
        let scope = crate::book_scope::BookScopeKey::archive(&root, &["c-show".into()]).unwrap();
        env.sidecar_restore = None;
        env.book_composition = ViewerBookComposition::Accepted {
            scope: scope.clone(),
            token: service.snapshot().token_for(&scope),
            record: None,
            layout: CompositionProjectionOutcome::Automatic,
            source: BookCompositionSource::Archive {
                tree: tree.clone(),
                prefix: vec!["c-show".into()],
                display_prefix: vec!["c-show".into()],
            },
            generation: env.items_generation,
            cause: ProjectionAdoptionCause::Normal,
        };
        let state = std::mem::take(&mut env.book_composition_permission.open);
        let OpenPermission::Checking {
            intent,
            mut candidates,
        } = state
        else {
            panic!("next real unit is independently qualified");
        };
        for candidate in &mut candidates {
            if let OpenCandidateState::Checking(receiver) = &candidate.state {
                let reply = receiver.recv_timeout(Duration::from_secs(30)).unwrap();
                assert_eq!(reply.correlation, candidate.correlation);
                candidate.state = OpenCandidateState::Complete(reply.status.unwrap());
            }
        }
        env.book_composition_permission.open = OpenPermission::Ready { intent, candidates };
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            env.poll_book_composition_read(ctx)
        });
        assert!(
            matches!(env.items.get(env.fullscreen_idx.unwrap()), Some(crate::grid_item::GridItem::ZipImage { entry_name, .. }) if entry_name == "c-show/c.jpg")
        );
        assert_eq!(env.selected, env.fullscreen_idx);
        assert_eq!(env.slideshow_playing, slideshow);
        if slideshow {
            assert!(env.slideshow_next_at > old_timer);
            assert_eq!(env.slideshow_anchor_idx, env.fullscreen_idx);
        } else {
            assert_eq!(env.slideshow_next_at, old_timer);
        }
        process.shutdown_and_join();
    }
}

#[test]
fn slideshow_handler_preserves_selection_and_timer_while_hidden_target_is_qualified() {
    use crate::book_composition::{
        PageDomain, ReadingDirection,
        fixtures::{document_page, four_page_book},
    };
    use crate::grid_item::ThumbnailState;
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };
    for stop_while_checking in [false, true] {
        let mut env = crate::app::tests::phase_c_support::setup_app();
        let root = env.tmp.path().join("slideshow-permission.pdf");
        std::fs::write(&root, b"fixture: permission precedes page decode").unwrap();
        env.items = (0..3)
            .map(|page_num| crate::grid_item::GridItem::PdfPage {
                pdf_path: root.clone(),
                page_num,
                content_type: None,
            })
            .collect();
        env.thumbnails = vec![ThumbnailState::Pending; 3];
        env.image_metas = vec![None; 3];
        env.visible_indices = vec![0, 1, 2];
        env.items_generation += 1;
        env.current_folder = None;
        env.projection_recipe = crate::app::book_materialization::ProjectionRecipe::OrdinaryList;
        env.page_identity_index = Arc::new(
            crate::app::file_type_resume::PreparedPageIdentityIndex::prepare(
                &env.items,
                Some(&root),
                env.settings.file_type_visibility.clone(),
            ),
        );
        env.spread_mode = crate::settings::SpreadMode::Single;
        env.fullscreen_idx = Some(0);
        env.selected = Some(0);
        env.scroll_to_selected = false;
        env.slideshow_playing = true;
        env.slideshow_anchor_idx = Some(0);
        let old_timer = Instant::now()
            .checked_sub(Duration::from_secs(3600))
            .unwrap();
        env.slideshow_next_at = old_timer;
        let scope = crate::book_scope::BookScopeKey::document(&root).unwrap();
        let (service, process) = BookCompositionService::spawn_at(
            env.tmp.path().join("slideshow-permission-data"),
            None,
        )
        .unwrap();
        let mut record = four_page_book(ReadingDirection::Ltr);
        record.hidden_pages.push(document_page(1));
        service
            .save(scope, PageDomain::Document, record)
            .recv_timeout(Duration::from_secs(30))
            .unwrap()
            .status
            .unwrap();
        env.install_book_composition_runtime(service.clone());
        let context_id = env.spread_shift_context_id().serial();
        let generation = env.items_generation;
        env.book_composition_permission.bind(context_id, generation);
        let ctx = egui::Context::default();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            env.advance_slideshow(ctx, 0)
        });
        assert_eq!(
            (
                env.fullscreen_idx,
                env.selected,
                env.scroll_to_selected,
                env.slideshow_anchor_idx,
                env.slideshow_next_at
            ),
            (Some(0), Some(0), false, Some(0), old_timer)
        );
        let state = std::mem::take(&mut env.book_composition_permission.open);
        let OpenPermission::Checking {
            intent,
            mut candidates,
        } = state
        else {
            panic!("actual slideshow handler admits a typed B request");
        };
        assert_eq!(intent.idx, 1);
        assert!(matches!(
            intent.route,
            BookTargetRoute::Slideshow { slice: None }
        ));
        assert_eq!(candidates.len(), 1);
        let OpenCandidateState::Checking(receiver) = &candidates[0].state else {
            panic!("candidate B receiver");
        };
        let hidden_reply = receiver.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_eq!(hidden_reply.correlation, candidates[0].correlation);
        assert!(matches!(
            &hidden_reply.status,
            Ok(CandidatePermission::Hidden(_))
        ));
        let (deliver, receiver) = mpsc::channel();
        candidates[0].state = OpenCandidateState::Checking(receiver);
        env.book_composition_permission.open = OpenPermission::Checking { intent, candidates };
        if stop_while_checking {
            assert!(env.stop_slideshow_playback());
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                env.poll_book_presentation_permission(ctx)
            });
            assert!(deliver.send(hidden_reply).is_err());
            assert!(matches!(
                env.book_composition_permission.open,
                OpenPermission::Idle
            ));
            assert_eq!(
                (env.fullscreen_idx, env.selected, env.slideshow_next_at),
                (Some(0), Some(0), old_timer)
            );
            assert!(!env.slideshow_playing);
        } else {
            deliver.send(hidden_reply).unwrap();
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                env.poll_book_presentation_permission(ctx)
            });
            assert_eq!(
                (
                    env.fullscreen_idx,
                    env.selected,
                    env.scroll_to_selected,
                    env.slideshow_anchor_idx,
                    env.slideshow_next_at
                ),
                (Some(0), Some(0), false, Some(0), old_timer)
            );
            let pending = match &env.book_composition_permission.open {
                OpenPermission::Checking { candidates, .. } => candidates
                    .iter()
                    .map(|candidate| candidate.correlation.clone())
                    .collect::<Vec<_>>(),
                _ => panic!("C checking request"),
            };
            let sequence = env.book_composition_permission.sequence;
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                env.advance_slideshow(ctx, 0)
            });
            assert!(
                matches!(&env.book_composition_permission.open, OpenPermission::Checking { intent, candidates } if intent.idx == 2 && candidates.iter().map(|candidate| candidate.correlation.clone()).collect::<Vec<_>>() == pending)
            );
            assert_eq!(
                env.book_composition_permission.sequence, sequence,
                "expired timer reentry cannot replace the in-flight qualified skip cursor"
            );
            assert_eq!(
                (env.fullscreen_idx, env.selected, env.slideshow_next_at),
                (Some(0), Some(0), old_timer)
            );
            let state = std::mem::take(&mut env.book_composition_permission.open);
            let OpenPermission::Checking {
                intent,
                mut candidates,
            } = state
            else {
                panic!("hidden B admits a fresh candidate episode for visible C");
            };
            assert_eq!(intent.idx, 2);
            for candidate in &mut candidates {
                let OpenCandidateState::Checking(receiver) = &candidate.state else {
                    panic!("new candidate receiver");
                };
                let reply = receiver.recv_timeout(Duration::from_secs(30)).unwrap();
                assert_eq!(reply.correlation, candidate.correlation);
                let (deliver, receiver) = mpsc::channel();
                deliver.send(reply).unwrap();
                candidate.state = OpenCandidateState::Checking(receiver);
            }
            env.book_composition_permission.open = OpenPermission::Checking { intent, candidates };
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                env.poll_book_presentation_permission(ctx)
            });
            assert_eq!(env.fullscreen_idx, Some(2));
            assert_eq!(env.selected, Some(2));
            assert!(env.slideshow_playing);
            assert_eq!(env.slideshow_anchor_idx, Some(2));
            assert!(env.slideshow_next_at > old_timer);
        }
        process.shutdown_and_join();
    }
}

#[test]
fn same_page_slideshow_qualifies_requested_half_before_slice_and_timer_effects() {
    use crate::{book_composition::PageDomain, grid_item::ThumbnailState, page_split::PageSlice};
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };
    for stop_while_checking in [false, true] {
        let mut env = crate::app::tests::phase_c_support::setup_app();
        let root = env.tmp.path().join("slideshow-split-permission.pdf");
        std::fs::write(
            &root,
            b"fixture: candidate admission does not decode images",
        )
        .unwrap();
        env.items = vec![crate::grid_item::GridItem::PdfPage {
            pdf_path: root.clone(),
            page_num: 0,
            content_type: None,
        }];
        env.thumbnails = vec![ThumbnailState::Pending];
        env.image_metas = vec![None];
        env.visible_indices = vec![0];
        env.items_generation += 1;
        env.current_folder = None;
        env.projection_recipe = crate::app::book_materialization::ProjectionRecipe::OrdinaryList;
        env.page_identity_index = Arc::new(
            crate::app::file_type_resume::PreparedPageIdentityIndex::prepare(
                &env.items,
                Some(&root),
                env.settings.file_type_visibility.clone(),
            ),
        );
        env.spread_mode = crate::settings::SpreadMode::SplitLtr;
        env.record_page_dims_for_spread(0, (2000, 1000));
        env.rotation_cache
            .insert(0, crate::rotation_db::Rotation::None);
        env.fullscreen_idx = Some(0);
        env.fullscreen_page_slice = PageSlice::Left;
        env.selected = Some(0);
        env.scroll_to_selected = false;
        env.slideshow_playing = true;
        env.slideshow_anchor_idx = Some(0);
        let old_timer = Instant::now()
            .checked_sub(Duration::from_secs(3600))
            .unwrap();
        env.slideshow_next_at = old_timer;
        let scope = crate::book_scope::BookScopeKey::document(&root).unwrap();
        let (service, process) =
            BookCompositionService::spawn_at(env.tmp.path().join("slideshow-split-data"), None)
                .unwrap();
        service
            .prepare(scope, PageDomain::Document)
            .recv_timeout(Duration::from_secs(30))
            .unwrap()
            .status
            .unwrap();
        env.install_book_composition_runtime(service);
        let left = PresentationOccurrence {
            item_index: 0,
            navigation_anchor: 0,
            role: PresentationRole::Navigation,
            slice: PresentationSlice::Left,
            slot: PresentationSlot::Center,
        };
        let right = PresentationOccurrence {
            slice: PresentationSlice::Right,
            ..left
        };
        env.install_allowed_presentation_for_test(&[left]);
        let original_generation = env.items_generation;
        let original_items = env.items.clone();
        assert_eq!(
            env.book_presentation_current(&[left]),
            PresentationDecision::Allowed(vec![true])
        );
        assert_eq!(
            env.book_presentation_current(&[right]),
            PresentationDecision::Checking
        );
        let ctx = egui::Context::default();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            env.advance_slideshow(ctx, 0)
        });
        assert_eq!(
            (
                env.fullscreen_page_slice,
                env.selected,
                env.scroll_to_selected,
                env.slideshow_anchor_idx,
                env.slideshow_next_at
            ),
            (PageSlice::Left, Some(0), false, Some(0), old_timer)
        );
        let state = std::mem::take(&mut env.book_composition_permission.open);
        let OpenPermission::Checking {
            intent,
            mut candidates,
        } = state
        else {
            panic!("same-page flip owns a new requested-half qualification");
        };
        assert!(matches!(
            &intent.route,
            BookTargetRoute::Slideshow {
                slice: Some(PageSlice::Right)
            }
        ));
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].occurrence(),
            right,
            "the existing Left proof cannot qualify the requested Right half"
        );
        let correlation = candidates[0].correlation.clone();
        let OpenCandidateState::Checking(receiver) = &candidates[0].state else {
            panic!("requested-half worker receiver");
        };
        let reply = receiver.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_eq!(reply.correlation, correlation);
        assert!(matches!(
            &reply.status,
            Ok(CandidatePermission::Allowed(Some(_)))
        ));
        let (deliver, receiver) = mpsc::channel();
        candidates[0].state = OpenCandidateState::Checking(receiver);
        env.book_composition_permission.open = OpenPermission::Checking { intent, candidates };
        let sequence = env.book_composition_permission.sequence;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            env.advance_slideshow(ctx, 0)
        });
        assert_eq!(env.book_composition_permission.sequence, sequence);
        assert!(
            matches!(&env.book_composition_permission.open, OpenPermission::Checking { candidates, .. } if candidates[0].correlation == correlation)
        );
        assert_eq!(
            (env.fullscreen_page_slice, env.slideshow_next_at),
            (PageSlice::Left, old_timer)
        );
        if stop_while_checking {
            assert!(env.stop_slideshow_playback());
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                env.poll_book_presentation_permission(ctx)
            });
            assert!(deliver.send(reply).is_err());
            assert!(matches!(
                env.book_composition_permission.open,
                OpenPermission::Idle
            ));
            assert_eq!(
                (
                    env.fullscreen_page_slice,
                    env.selected,
                    env.slideshow_next_at
                ),
                (PageSlice::Left, Some(0), old_timer)
            );
            assert_eq!(
                env.book_presentation_current(&[right]),
                PresentationDecision::Checking
            );
        } else {
            deliver.send(reply).unwrap();
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                env.poll_book_presentation_permission(ctx)
            });
            assert_eq!(env.fullscreen_idx, Some(0));
            assert_eq!(env.fullscreen_page_slice, PageSlice::Right);
            assert_eq!(
                env.items_generation, original_generation,
                "a same-source half change never rematerializes the source"
            );
            assert_eq!(env.items, original_items);
            assert_eq!(env.current_folder, None);
            assert!(matches!(
                env.projection_recipe,
                crate::app::book_materialization::ProjectionRecipe::OrdinaryList
            ));
            assert_eq!(env.spread_mode, crate::settings::SpreadMode::SplitLtr);
            assert_eq!(env.selected, Some(0));
            assert!(env.scroll_to_selected);
            assert!(env.slideshow_playing);
            assert!(env.slideshow_next_at > old_timer);
            assert!(!env.has_book_slideshow_continuation());
            assert_eq!(
                env.book_presentation_current(&[right]),
                PresentationDecision::Allowed(vec![true]),
                "exact requested-half receipt is adopted before display/timer effects"
            );
        }
        process.shutdown_and_join();
    }
}
