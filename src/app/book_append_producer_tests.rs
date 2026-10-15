//! Admission checks run without launching the application or invoking Shell deletion.
#![cfg(test)]

use super::{ActionSurface, App, AppTestEnvForTest, setup_app_for_test};
use crate::book_append_undo::{AppendReceiptBuilder, BookAppendUndoReceipt};
use crate::books::{BookOpIntent, BookOpPending, BookOpResult};
use crate::grid_item::GridItem;

fn receipt(app: &AppTestEnvForTest) -> BookAppendUndoReceipt {
    let root = app.tmp.path().join("producer-books");
    let folder = root.join("book");
    std::fs::create_dir_all(&folder).unwrap();
    let mut builder = AppendReceiptBuilder::begin(&root, &folder, "book".into()).unwrap();
    let page = folder.join("0001_page.jpg");
    std::fs::write(&page, b"generated page").unwrap();
    builder.record(&page).unwrap();
    BookAppendUndoReceipt {
        files: builder.finish().unwrap(),
        operation_id: 1,
        source_context: app.virtual_list_context_id(),
        accepted_revision: 0,
    }
}

fn hold_book_operation(
    app: &mut App,
    intent: BookOpIntent,
) -> std::sync::mpsc::Sender<Result<BookOpResult, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.book_op_pending = Some(BookOpPending { rx, intent });
    tx
}

pub(crate) const FOLLOWUP_PHASES: [&str; 5] = ["ready", "append", "undo", "source", "delete"];

pub(crate) fn followup_fixture(
    phase: &str,
) -> (
    AppTestEnvForTest,
    BookAppendUndoReceipt,
    Option<std::sync::mpsc::Sender<Result<BookOpResult, String>>>,
) {
    let mut app = setup_app_for_test();
    let mut append = receipt(&app);
    append.source_context = app.projected_viewer_context_id();
    append.accepted_revision = app.book_append_undo_revision;
    app.book_append_undo_receipt = Some(append.clone());
    let intent = match phase {
        "append" => Some(BookOpIntent::Append {
            operation_id: append.operation_id,
            source_context: append.source_context,
            accepted_revision: append.accepted_revision,
            target_folder: append.files.folder.clone(),
            observations: Default::default(),
        }),
        "undo" => Some(BookOpIntent::UndoAppend {
            receipt: append.clone(),
        }),
        "source" => Some(BookOpIntent::SourceMutation),
        "delete" => Some(BookOpIntent::Delete {
            path: append.files.folder.clone(),
        }),
        "ready" => None,
        _ => panic!("unknown followup phase"),
    };
    let worker = intent.map(|intent| hold_book_operation(&mut app, intent));
    (app, append, worker)
}

pub(crate) fn assert_followup_terminal(
    app: &mut App,
    append: &BookAppendUndoReceipt,
    worker: Option<std::sync::mpsc::Sender<Result<BookOpResult, String>>>,
    phase: &str,
) {
    if phase == "undo" {
        assert_eq!(app.book_append_undo_revision, append.accepted_revision);
        assert!(app.book_append_operation_matches_receipt(append.operation()));
    } else {
        assert!(
            app.book_append_undo_revision > append.accepted_revision,
            "{phase}"
        );
        assert!(app.book_append_undo_receipt.is_none(), "{phase}");
        if phase == "append" {
            // Deliver the real asynchronous append terminal after the user write.
            // A late successful completion must not resurrect the old receipt.
            let page = append.files.folder.join("0001_page.jpg");
            worker
                .unwrap()
                .send(Ok(BookOpResult::Append(crate::books::BookAppendSummary {
                    book_name: "book".into(),
                    folder: append.files.folder.clone(),
                    added: 1,
                    first_path: Some(page),
                    edit_copies: vec![],
                    semantic_copies: vec![],
                    erase_fallback_pages: 0,
                    skipped_hidden: 0,
                    undo_files: Some(append.files.clone()),
                    metadata_copy: Default::default(),
                })))
                .unwrap();
            app.poll_book_op_pending(&egui::Context::default());
            assert!(app.book_op_pending.is_none());
            assert!(app.book_append_undo_receipt.is_none());
        }
    }
}

#[test]
fn section1373_zip_representative_pin_set_and_remove_admit_before_dirty_is_consumed() {
    for remove in [false, true] {
        for phase in FOLLOWUP_PHASES {
            let (mut app, append, worker) = followup_fixture(phase);
            let zip = app.tmp.path().join("source.zip");
            let source = crate::folder_thumb_pins::FolderPinSource::ZipEntry {
                zip_rel: String::new(),
                entry: "page.jpg".into(),
            };
            let entries = vec![crate::zip_loader::ZipImageEntry {
                entry_name: "page.jpg".into(),
                uncompressed_size: 0,
                mtime: 0,
                container_ancestry: vec![],
            }];
            app.current_folder = Some(zip.clone());
            app.zip_nav = Some(crate::zip_tree::ZipNavState::new(std::sync::Arc::new(
                crate::zip_tree::ZipTree::build(zip.clone(), entries),
            )));
            app.items = vec![GridItem::ZipImage {
                zip_path: zip.clone(),
                entry_name: "page.jpg".into(),
            }];
            if remove {
                app.folder_thumb_pin_db
                    .as_ref()
                    .unwrap()
                    .set(&zip, &source)
                    .unwrap();
                let key = crate::path_key::normalize_keep_drive(&zip);
                app.folder_pin_map.insert(key, source.clone());
            }
            let generation = app.items_generation;
            app.toggle_folder_pin_for_idx(0);
            let pin = app.folder_thumb_pin_db.as_ref().unwrap().lookup(&zip);
            let expected = if phase == "undo" { remove } else { !remove };
            assert_eq!(pin.is_some(), expected, "{phase} remove={remove}");
            assert!(app.folder_thumb_pin_dirty.is_empty());
            assert_eq!(app.items_generation, generation);
            assert_followup_terminal(&mut app, &append, worker, phase);
        }
    }
}

#[test]
fn section1373_shared_media_write_entrances_gate_every_bookmark_and_pin_operation() {
    for action in [
        "add", "dedup", "bulk", "title", "remove", "clear", "pin", "unpin",
    ] {
        for phase in FOLLOWUP_PHASES {
            let (mut app, append, worker) = followup_fixture(phase);
            let path = app.tmp.path().join("video.mp4");
            let id = app
                .video_bookmark_db
                .as_ref()
                .unwrap()
                .add(&path, 1.0, Some("before"), &[])
                .unwrap();
            app.video_pin_db
                .as_ref()
                .unwrap()
                .set_pin(&path, 1.0, b"old")
                .unwrap();
            let bookmark_before = app.video_bookmark_db.as_ref().unwrap().list(&path);
            let pin_before = app.video_pin_db.as_ref().unwrap().lookup(&path).unwrap();
            let admitted = if action == "pin" || action == "unpin" {
                app.user_video_pin_db()
                    .map(|db| {
                        if action == "pin" {
                            db.set_pin(&path, 8.0, b"new").unwrap();
                        } else {
                            db.remove(&path).unwrap();
                        }
                    })
                    .is_some()
            } else {
                app.user_video_bookmark_db()
                    .map(|db| match action {
                        "add" => {
                            db.add(&path, 8.0, None, &[]).unwrap();
                        }
                        "dedup" => {
                            db.add_if_no_duplicate(&path, 8.0, None, 1.0).unwrap();
                        }
                        "bulk" => {
                            db.bulk_add_if_no_duplicate(&path, &[(8.0, None)], 1.0)
                                .unwrap();
                        }
                        "title" => {
                            db.update_title(id, Some("after")).unwrap();
                        }
                        "remove" => {
                            db.remove(id).unwrap();
                        }
                        "clear" => {
                            db.clear_for(&path).unwrap();
                        }
                        _ => unreachable!(),
                    })
                    .is_some()
            };
            assert_eq!(admitted, phase != "undo", "{action} {phase}");
            if phase == "undo" {
                let bookmarks = app.video_bookmark_db.as_ref().unwrap().list(&path);
                assert_eq!(bookmarks.len(), bookmark_before.len());
                assert_eq!(bookmarks[0].title, bookmark_before[0].title);
                let pin = app.video_pin_db.as_ref().unwrap().lookup(&path).unwrap();
                assert_eq!(pin.pin_pts_secs, pin_before.pin_pts_secs);
                assert_eq!(pin.thumb_webp, pin_before.thumb_webp);
            }
            assert_followup_terminal(&mut app, &append, worker, phase);
        }
    }
}

#[cfg(windows)]
#[test]
fn section1373_native_bookmark_delete_and_title_handlers_follow_common_admission() {
    for title in [false, true] {
        for phase in FOLLOWUP_PHASES {
            let (mut app, append, worker) = followup_fixture(phase);
            let path = app.tmp.path().join("video.mp4");
            let id = app
                .video_bookmark_db
                .as_ref()
                .unwrap()
                .add(&path, 1.0, Some("before"), &[])
                .unwrap();
            app.fullscreen_idx = Some(0);
            let ctx = egui::Context::default();
            if title {
                app.handle_native_video_set_bookmark_title_command(&ctx, 0, id, "after".into());
            } else {
                app.handle_native_video_delete_bookmark_command(&ctx, 0, id);
            }
            let bookmarks = app.video_bookmark_db.as_ref().unwrap().list(&path);
            if phase == "undo" {
                assert_eq!(bookmarks.len(), 1);
                assert_eq!(bookmarks[0].title.as_deref(), Some("before"));
            } else if title {
                assert_eq!(bookmarks[0].title.as_deref(), Some("after"));
            } else {
                assert!(bookmarks.is_empty());
            }
            assert_followup_terminal(&mut app, &append, worker, phase);
        }
    }
}

#[test]
fn section1373_rejected_targets_do_not_consume_receipt_or_advance_revision() {
    let (mut app, append, _worker) = followup_fixture("ready");
    app.folder_thumb_pin_db = None;
    app.video_bookmark_db = None;
    app.video_pin_db = None;
    assert!(app.user_folder_thumb_pin_db().is_none());
    assert!(app.user_video_bookmark_db().is_none());
    assert!(app.user_video_pin_db().is_none());
    app.delete_bookmark_browser_rows(&[]);
    app.toggle_folder_pin_for_idx(usize::MAX);
    #[cfg(windows)]
    app.handle_native_video_delete_bookmark_command(&egui::Context::default(), 99, 1);
    assert_eq!(app.book_append_undo_revision, append.accepted_revision);
    assert!(app.book_append_operation_matches_receipt(append.operation()));
}

#[test]
fn section1373_slot_save_and_rotation_reset_respect_followup_boundary() {
    for action in ["erase_slot", "conceal_slot", "rotation_reset"] {
        for phase in FOLLOWUP_PHASES {
            let (mut app, append, worker) = followup_fixture(phase);
            match action {
                "erase_slot" => {
                    app.erase_mask_size = [2, 1];
                    app.erase_mask = Some(vec![true, false]);
                    app.save_mask_to_slot(1);
                    assert_eq!(
                        app.mask_db
                            .as_ref()
                            .unwrap()
                            .get_slot_full(1, 2, 1)
                            .is_some(),
                        phase != "undo"
                    );
                }
                "conceal_slot" => {
                    app.conceal_mask_size = [2, 1];
                    app.conceal_mask = Some(vec![true, false]);
                    app.save_conceal_mask_to_slot(1);
                    assert_eq!(
                        app.conceal_db
                            .as_ref()
                            .unwrap()
                            .get_slot_full(1, 2, 1)
                            .is_some(),
                        phase != "undo"
                    );
                }
                _ => {
                    let path = app.tmp.path().join("image.jpg");
                    app.rotation_db
                        .as_ref()
                        .unwrap()
                        .set(&path, crate::rotation_db::Rotation::Cw90)
                        .unwrap();
                    assert_eq!(app.reset_saved_rotations(), phase != "undo");
                    assert_eq!(
                        app.rotation_db.as_ref().unwrap().count(),
                        if phase == "undo" { 1 } else { 0 }
                    );
                }
            }
            assert_followup_terminal(&mut app, &append, worker, phase);
        }
    }
}

#[test]
fn section1373_bookmark_browser_delete_admits_before_submitting_the_shared_worker() {
    for phase in FOLLOWUP_PHASES {
        let (mut app, append, worker) = followup_fixture(phase);
        let path = app.tmp.path().join("audio.wav");
        let id = app
            .video_bookmark_db
            .as_ref()
            .unwrap()
            .add(&path, 1.0, None, &[])
            .unwrap();
        let row = crate::bookmark_browser::BookmarkBrowserRow {
            source: crate::bookmark_browser::BookmarkRowSource::Media {
                id,
                path: path.clone(),
                pts_secs: 1.0,
                title: None,
                is_audio: true,
            },
            item: GridItem::Audio(path.clone()),
            relative_page_provenance: None,
            image_meta: None,
            marker_thumbnail: None,
            created_at_ms: 0,
            missing: false,
        };
        app.delete_bookmark_browser_rows(&[row]);
        assert_eq!(app.bookmark_delete_pending.is_some(), phase != "undo");
        if phase == "undo" {
            assert_eq!(app.video_bookmark_db.as_ref().unwrap().list(&path).len(), 1);
        }
        assert_followup_terminal(&mut app, &append, worker, phase);
    }
}

#[test]
fn section1373_explicit_view_and_resume_resets_gate_before_clearing_live_state() {
    for action in ["favorite_remove", "favorite_clear", "resume_clear"] {
        for phase in FOLLOWUP_PHASES {
            let (mut app, append, worker) = followup_fixture(phase);
            let id = uuid::Uuid::new_v4();
            let state = crate::settings::FavoriteViewState::from_settings(&app.settings);
            app.favorite_view_states.insert(id, state.clone());
            let resume = (app.tmp.path().join("book.zip"), 2, None, None);
            app.last_book_resume = Some(resume.clone());
            match action {
                "favorite_remove" => {
                    app.reset_favorite_view_state(id);
                }
                "favorite_clear" => {
                    app.clear_all_favorite_view_states();
                }
                "resume_clear" => app.clear_book_resume_async(),
                _ => unreachable!(),
            }
            if action == "resume_clear" {
                assert_eq!(app.last_book_resume, (phase == "undo").then_some(resume));
                assert_eq!(app.favorite_view_states.get(&id), Some(&state));
            } else {
                assert_eq!(app.favorite_view_states.contains_key(&id), phase == "undo");
                assert_eq!(app.last_book_resume, Some(resume));
            }
            assert_followup_terminal(&mut app, &append, worker, phase);
        }
    }
}

#[test]
fn section1373_tag_admission_rejects_only_undo_append_before_cache_or_job_submission() {
    for kind in ["source", "delete", "undo"] {
        let mut app = setup_app_for_test();
        let undo_receipt = receipt(&app);
        let path = app.tmp.path().join("other.jpg");
        std::fs::write(&path, b"other source").unwrap();
        app.items = vec![GridItem::Image(path.clone())];
        app.selected = Some(0);
        let intent = match kind {
            "source" => BookOpIntent::SourceMutation,
            "delete" => BookOpIntent::Delete {
                path: undo_receipt.files.folder.clone(),
            },
            _ => BookOpIntent::UndoAppend {
                receipt: undo_receipt,
            },
        };
        let _worker = hold_book_operation(&mut app, intent);
        app.request_tag_add_for_selection("accepted", ActionSurface::MainWindow);
        let key = crate::tags_db::item_key_for_path(&path);
        if kind == "undo" {
            assert!(app.pending_tag_undos.is_empty());
            assert!(app.tags_cache.get(&key).is_none_or(|tags| tags.is_empty()));
            assert!(
                app.tag_write_handle
                    .as_ref()
                    .is_none_or(|handle| !handle.has_unconsumed_batch())
            );
        } else {
            // No poll has run: acceptance must preserve existing optimistic tags
            // and register the normal pending metadata Undo for other book intents.
            assert!(!app.pending_tag_undos.is_empty());
            assert_eq!(
                app.tags_cache.get(&key).unwrap(),
                &vec!["#accepted".to_owned()]
            );
        }
    }
}

#[test]
fn section1373_erase_undo_and_redo_reject_only_pending_undo_append() {
    for undo_append in [false, true] {
        let mut app = setup_app_for_test();
        app.erase_mask_size = [2, 1];
        app.erase_mask = Some(vec![false, false]);
        app.push_undo_snapshot();
        app.erase_mask = Some(vec![true, false]);
        let intent = if undo_append {
            BookOpIntent::UndoAppend {
                receipt: receipt(&app),
            }
        } else {
            BookOpIntent::SourceMutation
        };
        let _worker = hold_book_operation(&mut app, intent);
        assert_eq!(app.undo_erase(), !undo_append);
        if undo_append {
            assert_eq!(app.erase_mask.as_ref().unwrap(), &vec![true, false]);
        } else {
            assert_eq!(app.erase_mask.as_ref().unwrap(), &vec![false, false]);
            assert!(app.redo_erase());
            assert_eq!(app.erase_mask.as_ref().unwrap(), &vec![true, false]);
        }
    }
}

#[test]
fn section1373_spread_shift_acceptance_rejects_only_pending_undo_append() {
    for undo_append in [false, true] {
        let mut app = setup_app_for_test();
        let path = app.tmp.path().join("ordinary.jpg");
        app.items = vec![GridItem::Image(path)];
        let intent = if undo_append {
            BookOpIntent::UndoAppend {
                receipt: receipt(&app),
            }
        } else {
            BookOpIntent::SourceMutation
        };
        let _worker = hold_book_operation(&mut app, intent);
        assert_eq!(app.set_live_spread_shift(0), !undo_append);
        assert_eq!(app.spread_shift_anchor_idx(), (!undo_append).then_some(0));
    }
}

#[cfg(windows)]
#[test]
fn section1373_keyaction_undo_busy_or_invalid_receipt_never_pops_older_metadata() {
    let _serial = crate::key_input::lock_test_input();
    crate::key_input::clear_test_frame();
    for case in ["busy", "invalid"] {
        let mut app = setup_app_for_test();
        let mut append = receipt(&app);
        append.source_context = app.projected_viewer_context_id();
        append.accepted_revision = app.book_append_undo_revision;
        let path = app.tmp.path().join("older.jpg");
        app.meta_undo.push(crate::undo_stack::UndoEntry::Rating {
            changes: vec![crate::undo_stack::RatingChange {
                path_key: path.to_string_lossy().into_owned(),
                source_path: path,
                meta: None,
                xmp_target: None,
                before: 0,
                after: 1,
            }],
            summary: "older".into(),
            origin: None,
        });
        let _worker = if case == "busy" {
            Some(hold_book_operation(&mut app, BookOpIntent::SourceMutation))
        } else {
            append.accepted_revision = append.accepted_revision.wrapping_add(1);
            None
        };
        app.book_append_undo_receipt = Some(append);
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput {
            modifiers: egui::Modifiers::CTRL,
            events: vec![egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL,
            }],
            ..Default::default()
        });
        let owner = crate::keyboard_input::decide_keyboard_owner(
            crate::keyboard_input::KeyboardOwnershipSnapshot {
                viewport: ctx.viewport_id(),
                viewport_focused: true,
                modal: false,
                text_input: None,
                focused_ui: None,
                shortcut_scope: Some(crate::keyboard_input::ShortcutScope::new(
                    ctx.viewport_id(),
                    crate::keyboard_input::ShortcutSurface::Main,
                )),
            },
        );
        crate::keyboard_input::cache_keyboard_owner(&ctx, owner);
        app.handle_meta_undo_keys(&ctx);
        assert_eq!(app.meta_undo.undo_len(), 1, "{case}");
        assert!(
            !app.keymap
                .consume_action(&ctx, crate::keymap::KeyAction::UndoLastAction)
        );
        if case == "invalid" {
            assert!(app.book_append_undo_receipt.is_none());
        }
        let _ = ctx.end_pass();
    }
}

#[test]
fn section1373_uncollected_writer_completion_and_pending_flush_keep_receipt_available() {
    for case in ["local_adjust_completion", "trim_flush"] {
        let mut app = setup_app_for_test();
        let mut append = receipt(&app);
        append.source_context = app.projected_viewer_context_id();
        append.accepted_revision = app.book_append_undo_revision;
        let operation = append.operation();
        app.book_append_undo_receipt = Some(append);
        if case == "local_adjust_completion" {
            let context = app.projected_viewer_context_id();
            app.local_adjust_write_pending.insert(
                "pending.jpg".into(),
                super::LocalAdjustWritePending {
                    generation: 1,
                    origin: super::LocalAdjustWriteOrigin {
                        context,
                        target: super::PageAdjustmentTarget {
                            page_key: "pending.jpg".into(),
                            location_path: None,
                            sidecar_coords: None,
                            compiled_book: false,
                            idx_hint: super::PageIndexHint::Absent,
                        },
                        revision: 0,
                    },
                    layers: Default::default(),
                },
            );
        } else {
            app.view_trim_save_pending = true;
        }
        assert!(app.try_undo_book_append(&egui::Context::default()));
        assert!(app.book_op_pending.is_none());
        assert!(app.book_append_operation_matches_receipt(operation));
    }
}
