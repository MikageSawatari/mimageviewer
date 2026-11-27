//! Accepted opens block edit admission; stopped slideshow permissions cannot land.
#![cfg(test)]

use super::*;

fn intent(app: &App, route: BookTargetRoute) -> OpenIntent {
    OpenIntent {
        context_id: app.spread_shift_context_id().serial(),
        items_generation: app.items_generation,
        idx: 1,
        history_trigger: HistoryTrigger::UserChosen,
        materialization: FsOpenMaterialization::Eager,
        contract: FsPageLoadContract::Sequential,
        landing: None,
        purpose: BookTargetPurpose::Next,
        route,
        navigation_origin: Vec::new(),
    }
}

#[test]
fn page_edit_admission_accepted_book_open_and_dfs_wait_until_navigation_finishes() {
    for slideshow in [false, true] {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        app.fullscreen_idx = Some(0);
        let route = if slideshow {
            BookTargetRoute::Slideshow { slice: None }
        } else {
            BookTargetRoute::FullscreenNavigation
        };
        app.book_composition_permission.open = OpenPermission::Ready {
            intent: intent(&app, route),
            candidates: Vec::new(),
        };
        assert!(app.page_edit_navigation_pending());
        assert!(!app.fullscreen_edit_mode_entry_allowed(0));
        app.book_composition_permission.open =
            OpenPermission::ContinueDfs(BookDfsContinuation::Slideshow);
        assert!(!app.fullscreen_edit_mode_entry_allowed(0));
        app.book_composition_permission.open = OpenPermission::Idle;
        assert!(app.fullscreen_edit_mode_entry_allowed(0));
    }
}

#[test]
fn page_edit_admission_stopped_slideshow_permission_completion_never_changes_page() {
    for ready in [false, true] {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::phase_c_support::setup_app();
        app.fullscreen_idx = Some(0);
        app.slideshow_playing = true;
        let intent = intent(&app, BookTargetRoute::Slideshow { slice: None });
        app.book_composition_permission.open = if ready {
            OpenPermission::Ready {
                intent,
                candidates: Vec::new(),
            }
        } else {
            OpenPermission::Checking {
                intent,
                candidates: Vec::new(),
            }
        };
        app.stop_projected_slideshow_playback();
        app.poll_book_presentation_permission(&ctx);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(!app.slideshow_playing);
        assert!(!app.book_composition_permission.open_pending());
        assert!(app.fullscreen_edit_mode_entry_allowed(0));
    }
}

#[test]
fn page_edit_admission_passive_book_permission_read_does_not_block_editing() {
    let mut app = crate::app::tests::phase_c_support::setup_app();
    app.fullscreen_idx = Some(0);
    let occurrence = PresentationOccurrence {
        item_index: 0,
        navigation_anchor: 0,
        role: PresentationRole::Navigation,
        slice: PresentationSlice::Full,
        slot: PresentationSlot::Center,
    };
    let candidate = CandidatePage {
        identity: crate::book_resume_db::PageIdentity::Document {
            container: "C:/passive-book.pdf".into(),
            page_num: 0,
        },
        physical_archive: None,
        raw_tree: None,
    };
    let context = app.spread_shift_context_id().serial();
    let generation = app.items_generation;
    app.book_composition_permission.bind(context, generation);
    let correlation = app
        .book_composition_permission
        .correlation(occurrence, &candidate);
    let (_tx, receiver) = mpsc::channel();
    app.book_composition_permission.occurrences.insert(
        occurrence,
        OccurrencePermission::Checking {
            correlation,
            candidate,
            receiver,
        },
    );
    assert!(app.book_composition_permission.pending());
    assert!(!app.book_composition_permission.open_pending());
    assert!(!app.page_edit_navigation_pending());
    assert!(app.fullscreen_edit_mode_entry_allowed(0));
}
