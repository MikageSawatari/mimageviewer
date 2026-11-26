//! r11 event identity, main-only ownership, validation and synchronous execution.
use super::super::events::{EventAdmission, EventIdentity, capture_event_executions};
use super::*;
#[cfg(windows)]
use crate::app::{
    MediaNavigationResolveResult, MediaNavigationResolver, MediaNavigationResolverRequest,
    MediaNavigationResolverResponse,
};
use crate::operation_script::{ALLOWED_OPERATIONS, ScriptEvent};
use std::time::{Duration, Instant};

fn event_script(body: &str) -> OperationScriptState {
    OperationScriptState::Ready(Box::new(
        CompiledOperationScript::compile(&format!(
            "fn slots() {{ [] }} fn on_event(s) {{ {body} }}"
        ))
        .unwrap(),
    ))
}
fn identity(app: &App, surface: ScriptInputSurface) -> EventIdentity {
    app.collect_operation_script_event_identity(surface)
}
fn frame(ctx: &egui::Context, mut f: impl FnMut(&egui::Context)) {
    ctx.run_pass(egui::RawInput::default(), |ctx| f(ctx))
        .drop_without_applying_deltas();
}

#[test]
fn event_execute_counts_dispatches_after_snapshot_on_success_validation_and_evaluation_failure() {
    let mut app = grid_app("[]");
    let ctx = egui::Context::default();
    for (body, expected_actions, expected_cols) in [
        (r#"["GridColumnCount3","GridColumnCount4"]"#, 2, 4),
        (r#"["GridColumnCount3","GridMoveLast"]"#, 0, 2),
        ("loop {}", 0, 2),
        ("[]", 0, 2),
    ] {
        app.operation_script_events = Default::default();
        app.operation_script.state = event_script(body);
        app.settings.grid_cols = 2;
        let (_, samples) = capture_event_executions(|| {
            frame(&ctx, |ctx| {
                app.detect_operation_script_events(ctx, ScriptInputSurface::Grid);
            })
        });
        assert_eq!(app.settings.grid_cols, expected_cols, "{body}");
        assert_eq!(
            samples,
            [(
                expected_actions,
                Some(identity(&app, ScriptInputSurface::Grid))
            )],
            "{body}"
        );
        // An unchanged frame has no invocation and no execution interval.
        let (_, samples) = capture_event_executions(|| {
            frame(&ctx, |ctx| {
                app.detect_operation_script_events(ctx, ScriptInputSurface::Grid);
            })
        });
        assert!(samples.is_empty());
    }
}

#[cfg(windows)]
#[test]
fn event_execute_records_rejected_dispatch_and_skipped_suffix_after_same_frame_snapshot() {
    let _serial = crate::key_input::lock_test_input();
    let mut app = video_app("[]", true);
    let ctx = egui::Context::default();
    video_player(&app, 0).set_volume(0.5);
    video_player(&app, 0).set_muted(false);
    app.operation_script.state =
        event_script(r#"["VideoMute","VideoSeekStripToggle","VideoVolumeUp"]"#);
    let (_, samples) = capture_event_executions(|| {
        frame(&ctx, |ctx| {
            app.detect_operation_script_events(ctx, ScriptInputSurface::Viewer);
        })
    });
    assert!(video_player(&app, 0).is_muted());
    assert_eq!(video_player(&app, 0).volume(), 0.5);
    assert!(!video_player(&app, 0).is_playing());
    assert!(!video_player(&app, 0).intent_playing());
    assert!(toast(&app).contains("VideoSeekStripToggle"));
    assert_eq!(
        samples,
        [(2, Some(identity(&app, ScriptInputSurface::Viewer)))]
    );
    app.fs_feedback_toast = None;
    let (_, samples) = capture_event_executions(|| {
        frame(&ctx, |ctx| {
            app.detect_operation_script_events(ctx, ScriptInputSurface::Viewer);
        })
    });
    assert!(samples.is_empty());
    assert!(app.fs_feedback_toast.is_none());
}

#[test]
fn event_open_only_cannot_forge_open_or_execute_a_mixed_prefix() {
    let error = CompiledOperationScript::compile(
        r#"fn slots() { [] } fn on_event(s) { s.events = ["open"]; [] }"#,
    )
    .err()
    .expect("property assignment must be rejected before evaluation");
    assert!(error.contains("添字・プロパティへの代入"), "{error}");
    for action in [
        KeyAction::FsReadingDirectionToggle,
        KeyAction::FsFitModeCycle,
    ] {
        for view_only in [false, true] {
            let ctx = egui::Context::default();
            let mut app = grid_app("[]");
            app.fullscreen_idx = Some(0);
            let mut previous = identity(&app, ScriptInputSurface::Viewer);
            if view_only {
                // Isolate the view-only event set at the detector boundary.
                previous.context = ScriptContext::FsVideo;
            }
            app.operation_script_events.finish_event(previous);
            if !view_only {
                app.fullscreen_page_slice = crate::page_split::PageSlice::Right;
            }
            app.operation_script.state = event_script(&format!(
                r#"s = #{{events:["open"]}};
                if "open" in s.events {{
                    #{{run:["FsZoomIn","{}","FsNavigatorToggle"],events:s.events,notice:"forged open accepted"}}
                }} else {{ ["unexpected local event set"] }}"#,
                action.ini_name()
            ));
            let before = (
                app.fs_zoom,
                app.settings.fullscreen_navigator_visible,
                app.reading_direction,
                app.settings.fullscreen_fit_mode,
            );
            frame(&ctx, |ctx| {
                app.detect_operation_script_events(ctx, ScriptInputSurface::Viewer)
            });
            assert!(toast(&app).contains("EventNotAllowed"), "{}", toast(&app));
            assert!(toast(&app).contains(action.ini_name()), "{}", toast(&app));
            assert_eq!(
                (
                    app.fs_zoom,
                    app.settings.fullscreen_navigator_visible,
                    app.reading_direction,
                    app.settings.fullscreen_fit_mode
                ),
                before
            );
            app.fs_feedback_toast = None;
            frame(&ctx, |ctx| {
                app.detect_operation_script_events(ctx, ScriptInputSurface::Viewer)
            });
            assert!(
                app.fs_feedback_toast.is_none(),
                "rejected invocation must still resnapshot"
            );
        }
    }
}

#[test]
fn event_r11_faces_reentry_and_select_baseline_survive_viewer_in_observe_and_finish() {
    let mut app = grid_app("[]");
    let mut events = OperationScriptEvents::default();
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::View, ScriptEvent::Select]
    );
    assert!(
        events
            .observe(identity(&app, ScriptInputSurface::Grid))
            .is_empty()
    );
    app.fullscreen_idx = Some(0);
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Open, ScriptEvent::View, ScriptEvent::Page]
    );
    app.selected = Some(1);
    events.finish_event(identity(&app, ScriptInputSurface::Viewer));
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::View, ScriptEvent::Select]
    );
    // Even finish_event on a grid clears open/page and updates its cursor baseline.
    events.finish_event(identity(&app, ScriptInputSurface::Grid));
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Open, ScriptEvent::View, ScriptEvent::Page]
    );
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::View]
    );
    app.selected = None;
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::Select]
    );
    let folder = app.current_folder.clone();
    app.current_folder = Some(app.tmp.path().join("empty-elsewhere"));
    assert!(
        events
            .observe(identity(&app, ScriptInputSurface::Grid))
            .is_empty()
    );
    app.current_folder = folder;
    assert!(
        events
            .observe(identity(&app, ScriptInputSurface::Grid))
            .is_empty()
    );
}

#[test]
fn event_initial_empty_grid_has_only_view_and_location_changes_do_not_select() {
    let mut app = grid_app("[]");
    app.selected = None;
    let mut events = OperationScriptEvents::default();
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::View]
    );
    app.current_folder = Some(app.tmp.path().join("elsewhere"));
    assert!(
        events
            .observe(identity(&app, ScriptInputSurface::Grid))
            .is_empty()
    );
}

#[test]
fn event_view_changes_grid_image_video_and_audio_shares_video_context() {
    let mut app = grid_app("[]");
    let mut events = OperationScriptEvents::default();
    events.observe(identity(&app, ScriptInputSurface::Grid));
    app.fullscreen_idx = Some(0);
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Open, ScriptEvent::View, ScriptEvent::Page]
    );
    let folder = app.current_folder.clone().unwrap();
    app.items[0] = GridItem::Video(folder.join("v.mp4"));
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::View, ScriptEvent::Page]
    );
    app.items[0] = GridItem::Audio(folder.join("a.mp3"));
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Page]
    );
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::View, ScriptEvent::Select]
    );
}

#[test]
fn event_r11_container_is_item_only_for_folders_pdf_epub_nested_and_converted_zip() {
    let mut app = grid_app("[]");
    app.fullscreen_idx = Some(0);
    let root = app.tmp.path().to_path_buf();
    let zip = root.join("cache.zip");
    let pdf = root.join("book.pdf");
    let epub = root.join("original.epub");
    let cases = vec![
        (
            GridItem::Image(root.join("folder/a.jpg")),
            root.join("folder"),
            "folder",
        ),
        (
            GridItem::Video(root.join("other/v.mp4")),
            root.join("other"),
            "folder",
        ),
        (
            GridItem::Audio(root.join("other/a.mp3")),
            root.join("other"),
            "folder",
        ),
        (
            GridItem::PdfPage {
                pdf_path: pdf.clone(),
                page_num: 4,
                content_type: None,
            },
            pdf,
            "pdf",
        ),
        (
            GridItem::PdfPage {
                pdf_path: epub.clone(),
                page_num: 5,
                content_type: None,
            },
            epub,
            "epub",
        ),
        (
            GridItem::ZipImage {
                zip_path: zip.clone(),
                entry_name: "chapter/a.jpg".into(),
            },
            zip.clone(),
            "zip",
        ),
        (
            GridItem::ZipImage {
                zip_path: zip.clone(),
                entry_name: "dir/inner.zip/chapter/a.jpg".into(),
            },
            zip.join("dir/inner.zip"),
            "zip",
        ),
        (
            GridItem::ZipImage {
                zip_path: zip.clone(),
                entry_name: "dir/inner.zip/sub/next.cbz/chapter/a.jpg".into(),
            },
            zip.join("dir/inner.zip/sub/next.cbz"),
            "zip",
        ),
    ];
    for (item, expected, kind) in cases {
        if let GridItem::ZipImage {
            zip_path,
            entry_name,
        } = &item
        {
            let parts = crate::zip_loader::split_nested_zip_path(entry_name);
            if parts.len() > 1 {
                // Existing container PathBuf construction keeps a raw nested prefix
                // as one segment: native outer boundary, literal internal slashes.
                let raw_prefix = parts[..parts.len() - 1].join("/");
                let existing = crate::spread_db::container_key(zip_path, &[raw_prefix]);
                assert_eq!(expected.to_str(), existing.to_str());
            }
        }
        app.items[0] = item;
        // Unrelated/mid-load source state must never change the item container.
        app.current_folder = Some(root.join("pending.zip"));
        app.archive_source_override = Some(root.join("source.rar"));
        let snapshot = app.collect_operation_script_snapshot(ScriptInputSurface::Viewer);
        assert_eq!(snapshot.book.path.as_deref(), expected.to_str());
        assert_eq!(snapshot.book.kind.as_deref(), Some(kind));
        assert_eq!(
            identity(&app, ScriptInputSurface::Viewer)
                .container
                .unwrap()
                .path,
            expected
        );
        app.current_folder = None;
        assert_eq!(
            app.collect_operation_script_snapshot(ScriptInputSurface::Viewer)
                .book
                .path,
            snapshot.book.path
        );
    }
}

#[test]
fn event_nonviewer_row_variants_have_no_fabricated_book_container() {
    let mut app = grid_app("[]");
    app.fullscreen_idx = Some(0);
    let path = app.tmp.path().join("container");
    let cases = vec![
        GridItem::Folder(path.clone()),
        GridItem::ZipFile(path.clone()),
        GridItem::PdfFile(path.clone()),
        GridItem::ConvertibleArchive {
            path: path.clone(),
            format: crate::archive_converter::ArchiveFormat::Rar,
        },
        GridItem::ZipDir {
            zip_path: path.clone(),
            dir_prefix: "chapter/".into(),
            is_archive: false,
            representative: None,
        },
        GridItem::Stack {
            key: "stack".into(),
            representative: path.clone(),
            count: 2,
        },
        GridItem::SearchContainer {
            path: path.clone(),
            kind: crate::grid_item::SearchContainerKind::Folder,
            hit_count: 1,
            representative: None,
        },
        GridItem::CollectionPlaceholder {
            path,
            last_known_kind: crate::collection_store::CollectionResolvedKind::Image,
            reason: crate::grid_item::CollectionPlaceholderReason::Missing,
        },
    ];
    for item in cases {
        app.items[0] = item;
        assert!(
            identity(&app, ScriptInputSurface::Viewer)
                .container
                .is_none()
        );
        let snapshot = app.collect_operation_script_snapshot(ScriptInputSurface::Viewer);
        assert!(snapshot.book.path.is_none());
        assert!(snapshot.book.kind.is_none());
    }
}

#[test]
fn event_r11_open_ignores_zip_subfolders_and_pending_source_but_follows_item_containers() {
    let mut app = grid_app("[]");
    app.fullscreen_idx = Some(0);
    let root = app.tmp.path().to_path_buf();
    let mut events = OperationScriptEvents::default();
    for (index, entry) in [
        "a/one.jpg",
        "b/two.jpg",
        "inside.zip/a.jpg",
        "inside.zip/deep/b.jpg",
    ]
    .iter()
    .enumerate()
    {
        app.items[0] = GridItem::ZipImage {
            zip_path: root.join("outer.zip"),
            entry_name: (*entry).into(),
        };
        let changed = events.observe(identity(&app, ScriptInputSurface::Viewer));
        assert_eq!(
            changed.contains(&ScriptEvent::Open),
            index == 0 || index == 2
        );
        app.current_folder = Some(root.join("enumerating-other.zip"));
        assert!(
            events
                .observe(identity(&app, ScriptInputSurface::Viewer))
                .is_empty()
        );
    }
    app.items[0] = GridItem::ZipImage {
        zip_path: root.join("replacement-cache.zip"),
        entry_name: "page.jpg".into(),
    };
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Open, ScriptEvent::Page]
    );
    // Search/collection/direct-media provenance does not substitute the list's location.
    for folder in ["search-parent", "collection-parent", "direct-parent"] {
        app.items[0] = GridItem::Image(root.join(folder).join("a.jpg"));
        assert_eq!(
            events.observe(identity(&app, ScriptInputSurface::Viewer)),
            [ScriptEvent::Open, ScriptEvent::Page]
        );
    }
}

#[test]
fn event_nested_container_appends_literal_entry_without_host_path_replacement() {
    let mut app = grid_app("[]");
    app.fullscreen_idx = Some(0);
    let outer = app.tmp.path().join("outer.zip");
    for prefix in ["/inner.zip", "C:/inner.zip", "//server/share/inner.zip"] {
        let entry_name = format!("{prefix}/page.jpg");
        app.items[0] = GridItem::ZipImage {
            zip_path: outer.clone(),
            entry_name: entry_name.clone(),
        };
        let expected = format!(
            "{}{separator}{prefix}",
            outer.to_string_lossy(),
            separator = std::path::MAIN_SEPARATOR_STR,
        );
        // Stage 2b still compares the physical root and raw entry separately.
        assert_eq!(
            app.operation_script_page_identity().scalars.item,
            Some(ScriptItemIdentity::ZipImage(outer.clone(), entry_name))
        );
        let snapshot = app.collect_operation_script_snapshot(ScriptInputSurface::Viewer);
        assert_eq!(snapshot.book.path.as_deref(), Some(expected.as_str()));
        assert!(
            snapshot
                .book
                .path
                .unwrap()
                .starts_with(outer.to_string_lossy().as_ref())
        );
    }
}

#[test]
fn event_and_slot_invocations_publish_the_same_item_container_book_path() {
    let mut app = grid_app("[]");
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    let outer = app.tmp.path().join("converted-cache.zip");
    app.items[0] = GridItem::ZipImage {
        zip_path: outer.clone(),
        entry_name: "nested.zip/sub/page.png".into(),
    };
    let expected = outer.join("nested.zip").to_string_lossy().into_owned();
    app.operation_script.state = OperationScriptState::Ready(Box::new(
        CompiledOperationScript::compile(
            r#"fn slots() { [#{slot:1,label:"container",run:"entry"}] }
        fn entry(s) { #{run:[],notice:s.book.path} }
        fn on_event(s) { #{run:[],notice:s.book.path} }"#,
        )
        .unwrap(),
    ));
    let ctx = egui::Context::default();
    assert!(
        !app.run_script_slot(&ctx, 1, ScriptInputSurface::Viewer, false)
            .rejected()
    );
    assert_eq!(toast(&app), expected);
    app.fs_feedback_toast = None;
    app.detect_operation_script_events(&ctx, ScriptInputSurface::Viewer);
    assert_eq!(toast(&app), expected);
}

#[test]
fn event_r11_pdf_pages_halves_and_unit_scalars_fire_but_reader_order_does_not() {
    let mut app = grid_app("[]");
    let path = app.tmp.path().join("book.pdf");
    app.items = (0..2)
        .map(|page_num| GridItem::PdfPage {
            pdf_path: path.clone(),
            page_num,
            content_type: None,
        })
        .collect();
    app.fullscreen_idx = Some(0);
    let mut events = OperationScriptEvents::default();
    events.observe(identity(&app, ScriptInputSurface::Viewer));
    app.fullscreen_idx = Some(1);
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Page]
    );
    for slice in [
        crate::page_split::PageSlice::Left,
        crate::page_split::PageSlice::Right,
    ] {
        app.fullscreen_page_slice = slice;
        assert_eq!(
            events.observe(identity(&app, ScriptInputSurface::Viewer)),
            [ScriptEvent::Page]
        );
    }
    app.visible_indices.reverse();
    app.items_generation += 1;
    assert!(
        events
            .observe(identity(&app, ScriptInputSurface::Viewer))
            .is_empty()
    );
    app.items.push(GridItem::PdfPage {
        pdf_path: path,
        page_num: 2,
        content_type: None,
    });
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Page],
        "automatic item count is still an O(1) unit input"
    );
    app.spread_mode = crate::settings::SpreadMode::Rtl;
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Page]
    );
}

#[test]
fn event_composition_strong_arc_memo_semantics_and_equal_republication() {
    let mut app = grid_app("[]");
    super::page_identity_perf::prepare_rows(&mut app, 4, true);
    super::page_identity_perf::install_zip_composition(&mut app);
    let mut events = OperationScriptEvents::default();
    events.observe(identity(&app, ScriptInputSurface::Viewer));
    let old = Arc::clone(app.retained_composed_projection_input().unwrap());
    let mut replacement = Arc::new((*old).clone());
    Arc::make_mut(&mut replacement).token.publication_sequence += 1;
    Arc::make_mut(&mut replacement).prepared_rotations =
        Arc::new(std::collections::HashMap::from([(
            "ignored-geometry".into(),
            crate::rotation_db::Rotation::Cw90,
        )]));
    app.install_composition_for_test(Arc::clone(&replacement));
    assert!(
        events
            .observe(identity(&app, ScriptInputSurface::Viewer))
            .is_empty()
    );
    assert_eq!(
        Arc::strong_count(&old),
        1,
        "equal comparison adopted the new strong Arc"
    );
    assert_eq!(
        Arc::strong_count(&replacement),
        3,
        "caller + App + event baseline"
    );
    assert!(
        events
            .observe(identity(&app, ScriptInputSurface::Viewer))
            .is_empty()
    );
    assert_eq!(Arc::strong_count(&replacement), 3);
    let before = Arc::clone(&replacement);
    Arc::make_mut(&mut replacement).resolved.complete = !replacement.resolved.complete;
    assert!(
        !Arc::ptr_eq(&before, &replacement),
        "baseline strong reference forces copy on write"
    );
    app.install_composition_for_test(Arc::clone(&replacement));
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Page]
    );
    Arc::make_mut(&mut replacement)
        .resolved
        .visible_pages
        .swap(0, 1);
    app.install_composition_for_test(Arc::clone(&replacement));
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Page]
    );
    let idx = replacement.resolved.visible_pages[0].item_index;
    Arc::make_mut(&mut replacement)
        .resolved
        .page_overrides
        .insert(idx, crate::book_composition::PageOverride::Alone);
    app.install_composition_for_test(replacement);
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Page]
    );
    let mut replacement = Arc::clone(app.retained_composed_projection_input().unwrap());
    Arc::make_mut(&mut replacement).resolved.cover_enabled = !replacement.resolved.cover_enabled;
    app.install_composition_for_test(Arc::clone(&replacement));
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Page]
    );
    Arc::make_mut(&mut replacement).resolved.direction =
        crate::book_composition::ReadingDirection::Rtl;
    app.install_composition_for_test(replacement);
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Page]
    );
}

#[test]
fn event_runtime_grid_executes_before_keys_once_and_inputs_still_change_next_frame() {
    let mut app = grid_app(r#"["GridMoveLast"]"#);
    app.operation_script.state = event_script(r#"#{run:["GridColumnCount3"],notice:s.events[0]}"#);
    let ctx = egui::Context::default();
    frame(&ctx, |ctx| {
        app.detect_operation_script_events(ctx, ScriptInputSurface::Grid);
        assert_eq!(app.settings.grid_cols, 3);
        assert_eq!(toast(&app), "view");
        app.settings.grid_cols = 2;
        app.detect_operation_script_events(ctx, ScriptInputSurface::Grid);
        assert_eq!(app.settings.grid_cols, 2);
    });
    frame(&ctx, |ctx| {
        app.detect_operation_script_events(ctx, ScriptInputSurface::Grid);
        assert_eq!(
            app.settings.grid_cols, 2,
            "unchanged first pass still owns the frame"
        );
        app.selected = Some(1);
        app.detect_operation_script_events(ctx, ScriptInputSurface::Grid);
        assert_eq!(app.settings.grid_cols, 2);
    });
    frame(&ctx, |ctx| {
        app.detect_operation_script_events(ctx, ScriptInputSurface::Grid);
        assert_eq!(app.settings.grid_cols, 3);
        assert_eq!(toast(&app), "select");
    });
}

#[test]
fn event_grid_actual_key_slot_navigation_is_observed_only_next_frame() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    let mut app = grid_app("[]");
    app.keymap = crate::keymap::Keymap::from_ini_str("[Global]\nScriptSlot1 = F13\n");
    app.operation_script.state = OperationScriptState::Ready(Box::new(CompiledOperationScript::compile(
        r#"fn slots() { [#{slot:1,label:"move",run:"entry"}] }
        fn entry(s) { ["GridMoveLast"] }
        fn on_event(s) { if "view" in s.events { #{run:["GridColumnCount4"],notice:"view"} } else { #{run:["GridColumnCount7"],notice:"select"} } }"#
    ).unwrap()));
    let ctx = egui::Context::default();
    ctx.run_pass(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::F13,
                physical_key: Some(egui::Key::F13),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            app.input_update_begin(ctx);
            assert!(app.input_main_grid_shortcuts(ctx, false).is_none());
            assert_eq!(app.selected, Some(1));
            assert_eq!(app.settings.grid_cols, 4);
            app.detect_operation_script_events(ctx, ScriptInputSurface::Grid);
            assert_eq!(
                app.settings.grid_cols, 4,
                "the key did not update the event baseline or run twice"
            );
        },
    )
    .drop_without_applying_deltas();
    frame(&ctx, |ctx| {
        app.input_update_begin(ctx);
        assert!(app.input_main_grid_shortcuts(ctx, false).is_none());
        assert_eq!(app.settings.grid_cols, 7);
        assert_eq!(toast(&app), "select");
    });
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
}

#[test]
fn event_runtime_rejects_entire_mixed_batch_and_never_publishes_success_notice() {
    let mut app = grid_app("[]");
    let ctx = egui::Context::default();
    for forbidden in [
        "GridMoveLast",
        "GridOpenSelected",
        "RatingItem3",
        "GridCopyPath",
    ] {
        app.operation_script_events = Default::default();
        app.settings.grid_cols = 2;
        app.operation_script.state = event_script(&format!(
            r#"#{{run:["GridColumnCount3","{forbidden}"],notice:"must not complete"}}"#
        ));
        app.detect_operation_script_events(&ctx, ScriptInputSurface::Grid);
        assert_eq!(app.settings.grid_cols, 2);
        assert!(toast(&app).contains("EventNotAllowed"), "{forbidden}");
    }
}

#[test]
fn event_runtime_rate_limit_drops_batches_notifies_once_and_does_not_replay() {
    let mut app = grid_app("[]");
    app.operation_script.state = event_script(r#"["GridColumnCount3"]"#);
    let ctx = egui::Context::default();
    let now = Instant::now();
    for index in 0..10 {
        app.selected = Some(index % 2);
        frame(&ctx, |ctx| {
            app.detect_operation_script_events_at(ctx, ScriptInputSurface::Grid, now)
        });
        assert_eq!(app.settings.grid_cols, 3);
    }
    app.settings.grid_cols = 2;
    app.selected = Some(0);
    frame(&ctx, |ctx| {
        app.detect_operation_script_events_at(ctx, ScriptInputSurface::Grid, now)
    });
    assert_eq!(app.settings.grid_cols, 2);
    assert!(toast(&app).contains("実行回数"));
    app.fs_feedback_toast = None;
    app.selected = Some(1);
    frame(&ctx, |ctx| {
        app.detect_operation_script_events_at(ctx, ScriptInputSurface::Grid, now)
    });
    assert!(app.fs_feedback_toast.is_none());
    assert_eq!(app.settings.grid_cols, 2);
    frame(&ctx, |ctx| {
        app.detect_operation_script_events_at(
            ctx,
            ScriptInputSurface::Grid,
            now + Duration::from_secs(1),
        )
    });
    assert_eq!(
        app.settings.grid_cols, 2,
        "expired rate window does not replay dropped events"
    );
    app.selected = Some(0);
    frame(&ctx, |ctx| {
        app.detect_operation_script_events_at(
            ctx,
            ScriptInputSurface::Grid,
            now + Duration::from_secs(1),
        )
    });
    assert_eq!(app.settings.grid_cols, 3);
}

#[test]
fn event_detection_cache_misses_do_not_read_rating_or_tag_databases() {
    let mut app = grid_app("[]");
    app.fullscreen_idx = Some(0);
    app.operation_script.state = event_script("[]");
    app.rating_cache.clear();
    app.tags_cache.clear();
    let ratings = crate::rating_db::read_operations_for_test();
    let tags = crate::tags_db::read_operations_for_test();
    app.detect_operation_script_events(&egui::Context::default(), ScriptInputSurface::Viewer);
    assert_eq!(crate::rating_db::read_operations_for_test(), ratings);
    assert_eq!(crate::tags_db::read_operations_for_test(), tags);
}
#[test]
fn event_limit_is_rolling_ten_attempts_and_notification_is_lifetime_once() {
    let mut events = OperationScriptEvents::default();
    let now = Instant::now();
    for index in 0..10 {
        assert_eq!(
            events.admit_execution(now + Duration::from_millis(index * 50)),
            EventAdmission::Execute
        );
    }
    assert_eq!(
        events.admit_execution(now + Duration::from_millis(999)),
        EventAdmission::Drop { notify: true }
    );
    assert_eq!(
        events.admit_execution(now + Duration::from_millis(999)),
        EventAdmission::Drop { notify: false }
    );
    assert_eq!(
        events.admit_execution(now + Duration::from_secs(1)),
        EventAdmission::Execute
    );
    assert_eq!(
        events.admit_execution(now + Duration::from_millis(1001)),
        EventAdmission::Drop { notify: false }
    );
    for index in 0..10 {
        assert_eq!(
            events.admit_execution(now + Duration::from_secs(3) + Duration::from_millis(index)),
            EventAdmission::Execute
        );
    }
    assert_eq!(
        events.admit_execution(now + Duration::from_millis(3011)),
        EventAdmission::Drop { notify: false }
    );
}

#[test]
fn event_rate_drop_advances_baseline_and_never_queues_a_late_replay() {
    let mut app = grid_app("[]");
    let mut events = OperationScriptEvents::default();
    events.observe(identity(&app, ScriptInputSurface::Grid));
    let now = Instant::now();
    for _ in 0..10 {
        assert_eq!(events.admit_execution(now), EventAdmission::Execute);
    }
    app.selected = Some(1);
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::Select]
    );
    assert_eq!(
        events.admit_execution(now),
        EventAdmission::Drop { notify: true }
    );
    assert!(
        events
            .observe(identity(&app, ScriptInputSurface::Grid))
            .is_empty()
    );
    assert_eq!(
        events.admit_execution(now + Duration::from_secs(1)),
        EventAdmission::Execute
    );
    assert!(
        events
            .observe(identity(&app, ScriptInputSurface::Grid))
            .is_empty()
    );
}

#[test]
fn event_detection_off_absent_loading_and_failed_do_not_claim_the_frame() {
    let mut app = grid_app("[]");
    let ctx = egui::Context::default();
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    app.operation_script.state = event_script("[]");
    app.settings.operation_script_enabled = false;
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    app.settings.operation_script_enabled = true;
    // This is an event-admission fixture, not a file-worker lifecycle test.
    app.operation_script.state =
        OperationScriptState::Loading(crate::operation_script::OperationScriptLoad);
    assert!(matches!(
        app.operation_script.state,
        OperationScriptState::Loading(_)
    ));
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    app.operation_script.state = OperationScriptState::Failed("test".into());
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    app.operation_script.state = event_script("[]");
    assert!(app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Viewer));
    assert!(app.fs_feedback_toast.is_none());
}

#[cfg(windows)]
#[test]
fn event_detection_mounted_context_and_linked_detached_skip_without_touching_main_owner() {
    let mut app = grid_app("[]");
    app.operation_script.state = event_script("[]");
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    let baseline = identity(&app, ScriptInputSurface::Viewer);
    app.operation_script_events.finish_event(baseline.clone());
    let other = app.build_window_context_for_test(363_401, |mounted| {
        crate::app::seed_snapshot_context_for_test(
            mounted,
            r"C:\event-other",
            &[r"C:\event-other\other.jpg"],
        );
        mounted.fullscreen_idx = Some(0);
        mounted.viewer_presentation = ViewerPresentation::DetachedWindow;
    });
    let ctx = egui::Context::default();
    app.with_viewer_context(other, |mounted| {
        for surface in [ScriptInputSurface::Grid, ScriptInputSurface::Viewer] {
            assert!(!mounted.begin_operation_script_event_detection(&ctx, surface));
        }
    })
    .unwrap();
    assert!(
        app.operation_script_events.observe(baseline).is_empty(),
        "the common owner was not swapped or rewritten"
    );
    // A parked sibling does not block the main viewer, even in this same frame.
    assert!(app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Viewer));
    assert!(app.fs_feedback_toast.is_none());
}

#[cfg(windows)]
#[test]
fn event_detection_linked_f12_and_transition_skip_viewer_but_main_list_continues() {
    let mut app = grid_app("[]");
    app.operation_script.state = event_script("[]");
    app.fullscreen_idx = Some(0);
    let ctx = egui::Context::default();
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Viewer));
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.video_presentation_transition
        .reset_stable(ViewerPresentation::Fullscreen);
    app.video_presentation_transition.request_transition(
        ViewerPresentation::DetachedWindow,
        false,
        false,
        0,
        None,
        None,
    );
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Viewer));
    assert!(app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    assert_eq!(
        identity(&app, ScriptInputSurface::Grid).context,
        ScriptContext::Grid
    );
    assert!(app.fs_feedback_toast.is_none());
}

#[test]
fn event_detection_edit_and_page_overview_coalesce_after_exit() {
    let mut app = grid_app("[]");
    app.operation_script.state = event_script("[]");
    let baseline = identity(&app, ScriptInputSurface::Grid);
    app.operation_script_events.finish_event(baseline);
    let ctx = egui::Context::default();
    app.erase_mode = true;
    app.selected = Some(1);
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    app.erase_mode = false;
    let metadata = Arc::new(
        crate::page_overview::PageOverviewMetadata::new([
            crate::page_overview::PageOverviewEntry {
                items_idx: 0,
                page: Arc::new(crate::page_overview::PageOverviewPage {
                    identity: crate::book_resume_db::PageIdentity::for_item(&app.items[0], None)
                        .unwrap(),
                    label: "first".into(),
                    white_companion: None,
                }),
            },
        ])
        .unwrap(),
    );
    let projection = Arc::new(
        crate::page_overview::PageOverviewProjection::new(metadata, Arc::new(vec![0])).unwrap(),
    );
    app.page_overview = crate::page_overview::PageOverviewState::open(
        projection,
        crate::page_overview::PageOverviewStamp {
            context_serial: 1,
            items_generation: app.items_generation,
            reader_order_revision: 1,
            incarnation: 1,
        },
        0,
        4,
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0)),
        Default::default(),
    )
    .unwrap();
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    app.page_overview = Default::default();
    assert!(app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    let current = identity(&app, ScriptInputSurface::Grid);
    assert_eq!(
        app.operation_script_events.observe(current),
        [ScriptEvent::Select]
    );
    assert!(app.fs_feedback_toast.is_none());
}

#[cfg(windows)]
#[test]
fn event_detection_native_overlay_uses_existing_snapshot_and_does_not_claim_blocked_frame() {
    let mut app = video_app("[]", false);
    app.operation_script.state = event_script("[]");
    let ctx = egui::Context::default();
    for modal in [false, true] {
        video_player(&app, 0).set_native_overlay_input_routing_for_test(
            crate::video::native_presenter::NativeOverlayInputRouting {
                text_input_active: !modal,
                modal_dialog_active: modal,
                ..Default::default()
            },
        );
        assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Viewer));
    }
    video_player(&app, 0).set_native_overlay_input_routing_for_test(Default::default());
    assert!(app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Viewer));
    assert!(app.fs_feedback_toast.is_none());
}

#[cfg(windows)]
#[test]
fn event_native_poll_video_hook_executes_and_overlay_snapshot_blocks_before_evaluation() {
    for blocked in [true, false] {
        let mut app = video_app("[]", false);
        app.operation_script.state =
            event_script(r#"#{run:["VideoMute"],notice:"native event complete"}"#);
        let loaded_operations = match &app.operation_script.state {
            OperationScriptState::Ready(script) => script.last_operation_count(),
            _ => unreachable!(),
        };
        video_player(&app, 0).set_native_overlay_input_routing_for_test(
            crate::video::native_presenter::NativeOverlayInputRouting {
                text_input_active: blocked,
                ..Default::default()
            },
        );
        let ctx = egui::Context::default();
        frame(&ctx, |ctx| app.poll_video(ctx));
        if blocked {
            assert!(app.fs_feedback_toast.is_none());
            assert!(!video_player(&app, 0).is_muted());
            assert!(
                matches!(&app.operation_script.state, OperationScriptState::Ready(script) if script.last_operation_count() == loaded_operations)
            );
        } else {
            assert_eq!(toast(&app), "native event complete");
            assert!(video_player(&app, 0).is_muted());
        }
    }
}

#[test]
fn event_safe_grid_actions_preserve_all_four_identity_components_even_from_details() {
    let mut app = grid_app("[]");
    let ctx = egui::Context::default();
    for operation in ALLOWED_OPERATIONS.iter().filter(|op| {
        op.event_permission == crate::operation_script::EventPermission::Synchronous
            && op.contexts.contains(ScriptContext::Grid)
    }) {
        app.set_grid_view_mode(crate::settings::GridViewMode::Details);
        app.operation_script_events = Default::default();
        app.operation_script.state = event_script(&format!(
            r#"#{{run:["{}"],notice:"grid event complete"}}"#,
            operation.action.ini_name()
        ));
        frame(&ctx, |ctx| {
            app.detect_operation_script_events(ctx, ScriptInputSurface::Grid);
            assert_eq!(toast(&app), "grid event complete", "{:?}", operation.action);
            let immediate = identity(&app, ScriptInputSurface::Grid);
            app.rebuild_visible_indices();
            app.rebuild_details_order();
            assert_eq!(
                identity(&app, ScriptInputSurface::Grid),
                immediate,
                "{:?}",
                operation.action
            );
        });
        app.fs_feedback_toast = None;
        frame(&ctx, |ctx| {
            app.detect_operation_script_events(ctx, ScriptInputSurface::Grid)
        });
        assert!(app.fs_feedback_toast.is_none(), "{:?}", operation.action);
    }
}

#[cfg(windows)]
#[test]
fn event_safe_video_actions_preserve_all_four_identity_components_for_native_and_music() {
    let _serial = crate::key_input::lock_test_input();
    use crate::app::VideoKeyActionSurface;
    for surface in [
        VideoKeyActionSurface::Native,
        VideoKeyActionSurface::EguiVideo,
        VideoKeyActionSurface::EguiMusic,
    ] {
        let music = surface == VideoKeyActionSurface::EguiMusic;
        // One live fixture per iteration, never acquire setup_app's mutex twice.
        let mut app = video_app("[]", music);
        let ctx = egui::Context::default();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
        crate::key_input::install_operation_input_plugin(&ctx);
        app.settings.video_in_window_mode = true;
        app.native_video_in_window_active = true;
        app.viewer_presentation = ViewerPresentation::MainWindow;
        app.video_continuous_mode = crate::video::VideoContinuousMode::Continuous;
        if surface == VideoKeyActionSurface::EguiVideo {
            video_player(&app, 0).set_native_presenter_hidden_for_test(true);
        }
        for operation in ALLOWED_OPERATIONS.iter().filter(|op| {
            op.event_permission == crate::operation_script::EventPermission::Synchronous
                && op.contexts.contains(ScriptContext::FsVideo)
        }) {
            if music && operation.action == KeyAction::VideoSeekStripToggle {
                continue; // Released music keys do not expose the video strip.
            }
            video_player(&app, 0).set_playing(false);
            video_player(&app, 0).set_position_for_test(10.0);
            video_player(&app, 0).set_last_displayed_pts_for_test(10.0);
            video_player(&app, 0).set_volume(0.5);
            video_player(&app, 0).set_muted(false);
            assert!(!video_player(&app, 0).intent_playing(), "normal user pause");
            let seek_base = video_player(&app, 0).user_seek_base_secs_for_test();
            let serial = video_player(&app, 0).current_seek_serial();
            app.settings.video_loop_mode = crate::settings::VideoLoopMode::Off;
            app.settings.video_loop = false;
            app.operation_script_events = Default::default();
            app.operation_script.state = event_script(&format!(
                r#"#{{run:["{}"],notice:"media event complete"}}"#,
                operation.action.ini_name()
            ));
            let mut immediate = None;
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 400.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    app.detect_operation_script_events(ui.ctx(), ScriptInputSurface::Viewer);
                    assert_eq!(
                        toast(&app),
                        "media event complete",
                        "{:?}, {surface:?}",
                        operation.action
                    );
                    assert!(
                        !video_player(&app, 0).intent_playing(),
                        "{:?}, {surface:?}",
                        operation.action
                    );
                    assert!(!video_player(&app, 0).is_playing());
                    assert!(video_player(&app, 0).user_seek_base_secs_for_test() <= seek_base);
                    assert_eq!(video_player(&app, 0).current_seek_serial(), serial);
                    assert!(!video_player(&app, 0).clock_is_seeking_for_test());
                    immediate = Some(identity(&app, ScriptInputSurface::Viewer));
                    app.poll_video(ui.ctx());
                    if surface != VideoKeyActionSurface::Native {
                        app.render_fullscreen_viewport(ui);
                    }
                },
            )
            .drop_without_applying_deltas();
            let immediate = immediate.unwrap();
            assert!(!video_player(&app, 0).intent_playing());
            assert!(!video_player(&app, 0).is_playing());
            assert!(!video_player(&app, 0).is_at_eof());
            assert!(app.video_continuous_last_eof.is_none());
            assert!(app.media_navigation_pending.is_none());
            assert_eq!(app.fullscreen_idx, Some(0));
            assert_eq!(
                identity(&app, ScriptInputSurface::Viewer),
                immediate,
                "{:?}, {surface:?}",
                operation.action
            );
            app.fs_feedback_toast = None;
            ctx.run_ui(egui::RawInput::default(), |ui| {
                app.poll_video(ui.ctx());
                if surface != VideoKeyActionSurface::Native {
                    app.render_fullscreen_viewport(ui);
                }
            })
            .drop_without_applying_deltas();
            assert_eq!(
                identity(&app, ScriptInputSurface::Viewer),
                immediate,
                "{:?}, {surface:?}",
                operation.action
            );
            assert!(
                app.fs_feedback_toast
                    .as_ref()
                    .is_none_or(|toast| toast.0 != "media event complete"),
                "{:?}, {surface:?}",
                operation.action
            );
            assert!(!video_player(&app, 0).intent_playing());
            assert!(!video_player(&app, 0).is_playing());
            assert!(!video_player(&app, 0).is_at_eof());
            assert!(app.video_continuous_last_eof.is_none());
            assert!(app.media_navigation_pending.is_none());
            assert_eq!(app.fullscreen_idx, Some(0));
        }
    }
}

#[cfg(windows)]
fn short_playing_media_app(ctx: &egui::Context, music: bool) -> AppTestEnv {
    let mut app = video_app("[]", music);
    app.video_continuous_mode = crate::video::VideoContinuousMode::Continuous;
    app.settings.video_loop_mode = crate::settings::VideoLoopMode::Off;
    app.settings.video_loop = false;
    for idx in 0..2 {
        let path = app.items[idx].file_operation_path().unwrap().to_path_buf();
        let mut player = crate::video::VideoPlayer::video_only_disconnected_for_test(path);
        player.set_duration_for_test(4.0);
        if idx == 0 {
            player.confirm_playback_start_for_test(crate::video::PlaybackStartOrigin::UserPlay);
            player.set_position_for_test(3.5);
        }
        app.fs_cache.insert(
            idx,
            crate::fs_animation::FsCacheEntry::Video {
                player: Box::new(player),
                load_seq: 0,
            },
        );
    }
    app.settings.operation_script_enabled = true;
    app.operation_script.state = event_script("[]");
    app.detect_operation_script_events(ctx, ScriptInputSurface::Viewer);
    // Seed the event baseline; each assertion phase uses fresh ROOT frames.
    assert!(video_player(&app, 0).is_playing());
    assert!(video_player(&app, 0).intent_playing());
    assert!(!video_player(&app, 0).clock_is_seeking_for_test());
    app
}

#[cfg(windows)]
fn video_player_mut(app: &mut App, idx: usize) -> &mut crate::video::VideoPlayer {
    match app.fs_cache.get_mut(&idx).unwrap() {
        crate::fs_animation::FsCacheEntry::Video { player, .. } => player,
        _ => panic!("video fixture"),
    }
}

#[cfg(windows)]
fn event_media_resolver(
    app: &mut App,
) -> (
    std::sync::mpsc::Receiver<MediaNavigationResolverRequest>,
    std::sync::mpsc::Sender<MediaNavigationResolverResponse>,
) {
    let (request_tx, request_rx) = std::sync::mpsc::channel();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    app.media_navigation_resolver = Some(MediaNavigationResolver {
        request_tx,
        result_rx,
        next_request_id: 1,
    });
    (request_rx, result_tx)
}

#[cfg(windows)]
#[test]
fn event_r13_rejects_all_nine_playback_advancing_actions_atomically_on_short_playing_video() {
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = short_playing_media_app(&ctx, false);
    let (requests, _responses) = event_media_resolver(&mut app);
    for action in [
        KeyAction::VideoPlayPause,
        KeyAction::VideoLoop,
        KeyAction::VideoSeekStart,
        KeyAction::VideoSeekBackSmall,
        KeyAction::VideoSeekForwardSmall,
        KeyAction::VideoSeekBackMedium,
        KeyAction::VideoSeekForwardMedium,
        KeyAction::VideoSeekBackLarge,
        KeyAction::VideoSeekForwardLarge,
    ] {
        app.operation_script_events = Default::default();
        app.settings.video_loop_mode = crate::settings::VideoLoopMode::Full;
        app.settings.video_loop = true;
        video_player(&app, 0).set_muted(false);
        video_player(&app, 0).set_volume(0.5);
        let serial = video_player(&app, 0).current_seek_serial();
        app.operation_script.state = event_script(&format!(
            r#"["VideoMute","{}","VideoVolumeUp"]"#,
            action.ini_name()
        ));
        let (_, samples) = capture_event_executions(|| {
            frame(&ctx, |ctx| {
                app.poll_video(ctx);
            })
        });
        assert!(
            toast(&app).contains("EventNotAllowed"),
            "{action:?}: {}",
            toast(&app)
        );
        assert!(toast(&app).contains(action.ini_name()));
        assert_eq!(samples.len(), 1);
        assert_eq!(samples.first().map(|sample| sample.0), Some(0));
        assert!(video_player(&app, 0).is_playing());
        assert!(video_player(&app, 0).intent_playing());
        assert!(!video_player(&app, 0).is_muted());
        assert_eq!(video_player(&app, 0).volume(), 0.5);
        assert_eq!(video_player(&app, 0).current_seek_serial(), serial);
        assert_eq!(
            app.settings.video_loop_mode,
            crate::settings::VideoLoopMode::Full
        );
        assert!(app.settings.video_loop);
        assert!(requests.try_recv().is_err());
        assert!(app.media_navigation_pending.is_none());
        assert_eq!(app.fullscreen_idx, Some(0));
    }
}

#[cfg(windows)]
#[test]
fn event_r13_eof_back_seeks_are_rejected_atomically_without_dispatch_or_restart() {
    let _serial = crate::key_input::lock_test_input();
    use crate::app::VideoKeyActionSurface;
    for surface in [
        VideoKeyActionSurface::Native,
        VideoKeyActionSurface::EguiVideo,
        VideoKeyActionSurface::EguiMusic,
    ] {
        let ctx = egui::Context::default();
        let mut app = short_playing_media_app(&ctx, surface == VideoKeyActionSurface::EguiMusic);
        let (requests, _responses) = event_media_resolver(&mut app);
        // Isolate the event from the independent continuous-EOF next adoption.
        // The continuous-ON positive control below exercises that other producer.
        app.video_continuous_mode = crate::video::VideoContinuousMode::Off;
        if surface == VideoKeyActionSurface::EguiVideo {
            video_player(&app, 0).set_native_presenter_hidden_for_test(true);
        }
        video_player(&app, 0).mark_eof_for_test(4.0);
        assert!(video_player(&app, 0).is_at_eof());
        assert!(!video_player(&app, 0).is_playing());
        for action in [
            KeyAction::VideoSeekBackSmall,
            KeyAction::VideoSeekBackMedium,
            KeyAction::VideoSeekBackLarge,
        ] {
            video_player(&app, 0).set_muted(false);
            video_player(&app, 0).set_volume(0.5);
            let before = (
                video_player(&app, 0).position(),
                video_player(&app, 0).current_seek_serial(),
                video_player(&app, 0).intent_playing(),
            );
            app.operation_script_events = Default::default();
            app.operation_script.state = event_script(&format!(
                r#"["VideoMute","{}","VideoVolumeUp"]"#,
                action.ini_name()
            ));
            let (_, samples) = capture_event_executions(|| {
                frame(&ctx, |ctx| {
                    app.detect_operation_script_events(ctx, ScriptInputSurface::Viewer);
                    assert!(video_player(&app, 0).is_at_eof());
                    assert!(!video_player(&app, 0).is_playing());
                    assert_eq!(
                        (
                            video_player(&app, 0).position(),
                            video_player(&app, 0).current_seek_serial(),
                            video_player(&app, 0).intent_playing()
                        ),
                        before
                    );
                    app.poll_video(ctx);
                })
            });
            assert!(toast(&app).contains("EventNotAllowed"));
            assert!(toast(&app).contains(action.ini_name()));
            assert_eq!(samples.len(), 1);
            assert_eq!(samples[0].0, 0);
            assert!(!video_player(&app, 0).is_muted(), "no synchronous prefix");
            assert_eq!(video_player(&app, 0).volume(), 0.5, "no synchronous suffix");
            assert_eq!(
                (
                    video_player(&app, 0).position(),
                    video_player(&app, 0).current_seek_serial(),
                    video_player(&app, 0).intent_playing()
                ),
                before
            );
            assert!(video_player(&app, 0).is_at_eof());
            assert!(!video_player(&app, 0).is_playing());
            assert!(!video_player(&app, 0).clock_is_seeking_for_test());
            assert_eq!(app.fullscreen_idx, Some(0));
            assert!(requests.try_recv().is_err());
            assert!(app.media_navigation_pending.is_none());
        }
    }
}

#[cfg(windows)]
#[test]
fn event_r13_remaining_four_controls_do_not_restart_an_eof_source() {
    let _serial = crate::key_input::lock_test_input();
    use crate::app::VideoKeyActionSurface;
    for surface in [
        VideoKeyActionSurface::Native,
        VideoKeyActionSurface::EguiVideo,
        VideoKeyActionSurface::EguiMusic,
    ] {
        let ctx = egui::Context::default();
        let mut app = short_playing_media_app(&ctx, surface == VideoKeyActionSurface::EguiMusic);
        let (requests, _responses) = event_media_resolver(&mut app);
        app.video_continuous_mode = crate::video::VideoContinuousMode::Off;
        if surface == VideoKeyActionSurface::EguiVideo {
            video_player(&app, 0).set_native_presenter_hidden_for_test(true);
        }
        video_player(&app, 0).mark_eof_for_test(4.0);
        video_player(&app, 0).set_muted(false);
        video_player(&app, 0).set_volume(0.5);
        let before = (
            video_player(&app, 0).position(),
            video_player(&app, 0).current_seek_serial(),
            video_player(&app, 0).intent_playing(),
        );
        assert!(video_player(&app, 0).is_at_eof());
        assert!(!video_player(&app, 0).is_playing());
        app.operation_script_events = Default::default();
        app.operation_script.state = event_script(
            r#"#{run:["VideoMute","VideoVolumeUp","VideoVolumeDown","VideoSeekStripToggle"],notice:"EOF controls accepted"}"#,
        );
        let (_, samples) = capture_event_executions(|| {
            frame(&ctx, |ctx| {
                app.detect_operation_script_events(ctx, ScriptInputSurface::Viewer);
                if surface == VideoKeyActionSurface::EguiMusic {
                    // The first three controls execute; the released music path rejects
                    // the fourth (strip) at dispatch. No new music-strip support is added.
                    assert!(toast(&app).contains("VideoSeekStripToggle"));
                    assert!(!toast(&app).contains("EventNotAllowed"));
                } else {
                    assert_eq!(toast(&app), "EOF controls accepted");
                }
                // The strip's independent analysis worker may report its own toast.
                app.poll_video(ctx);
            })
        });
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].0, 4);
        assert!(
            video_player(&app, 0).is_muted(),
            "the controls really executed"
        );
        assert!(video_player(&app, 0).is_at_eof());
        assert!(!video_player(&app, 0).is_playing());
        assert!(!video_player(&app, 0).clock_is_seeking_for_test());
        assert_eq!(
            (
                video_player(&app, 0).position(),
                video_player(&app, 0).current_seek_serial(),
                video_player(&app, 0).intent_playing()
            ),
            before
        );
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(requests.try_recv().is_err());
        assert!(app.media_navigation_pending.is_none());
        let (_, samples) = capture_event_executions(|| {
            frame(&ctx, |ctx| {
                app.detect_operation_script_events(ctx, ScriptInputSurface::Viewer);
                app.poll_video(ctx);
            })
        });
        assert!(samples.is_empty());
        assert!(video_player(&app, 0).is_at_eof());
        assert!(!video_player(&app, 0).is_playing());
        assert_eq!(video_player(&app, 0).intent_playing(), before.2);
    }
}

#[cfg(windows)]
fn run_event_eof_adoption_control() {
    let ctx = egui::Context::default();
    let mut app = short_playing_media_app(&ctx, false);
    let (requests, responses) = event_media_resolver(&mut app);
    drop(app.fs_cache.remove(&1)); // Target's real open must use the disconnected factory.
    app.operation_script.state = event_script(
        r#"if "page" in s.events { #{run:[],notice:"next adopted page"} } else { [] }"#,
    );
    video_player(&app, 0).notify_demux_exhausted_for_test();
    video_player_mut(&mut app, 0).backdate_eof_quiet_for_test(Duration::from_millis(49));
    frame(&ctx, |ctx| app.poll_video(ctx));
    assert!(video_player(&app, 0).is_at_eof());
    assert!(!video_player(&app, 0).clock_is_seeking_for_test());
    assert!(app.video_continuous_last_eof.is_some());
    let request = requests
        .try_recv()
        .expect("real poll must request next after EOF");
    assert_eq!(request.source, "video_continuous_eof");
    assert!(
        request
            .candidates
            .iter()
            .any(|candidate| candidate.idx == 1)
    );
    responses
        .send(MediaNavigationResolverResponse {
            request_id: request.request_id,
            result: MediaNavigationResolveResult {
                target_idx: Some(1),
                missing: vec![],
            },
        })
        .unwrap();
    // No presenter thread exists in the fixture. Exercise the real native EOF
    // admission's no-presenter open path using the existing disconnected factory.
    drop(video_player_mut(&mut app, 0).take_native_output());
    let (_, opens) = crate::video::VideoPlayer::with_disconnected_opens_for_test(|| {
        frame(&ctx, |ctx| app.poll_video(ctx));
    });
    assert!(
        !opens.is_empty(),
        "next source goes through the real open factory"
    );
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(app.media_navigation_pending.is_none());
    app.fs_feedback_toast = None;
    frame(&ctx, |ctx| {
        app.detect_operation_script_events(ctx, ScriptInputSurface::Viewer);
        app.poll_video(ctx);
    });
    assert_eq!(toast(&app), "next adopted page");
}

#[cfg(windows)]
#[test]
fn event_r13_eof_control_runs_tick_resolver_next_adoption_and_then_page_detection() {
    let _serial = crate::key_input::lock_test_input();
    run_event_eof_adoption_control();
}

#[cfg(windows)]
#[test]
fn event_r13_paused_seek_start_is_rejected_atomically_without_dispatch_or_resume() {
    let _serial = crate::key_input::lock_test_input();
    use crate::app::VideoKeyActionSurface;
    for surface in [
        VideoKeyActionSurface::Native,
        VideoKeyActionSurface::EguiVideo,
        VideoKeyActionSurface::EguiMusic,
    ] {
        let ctx = egui::Context::default();
        let mut app = short_playing_media_app(&ctx, surface == VideoKeyActionSurface::EguiMusic);
        let (requests, _responses) = event_media_resolver(&mut app);
        if surface == VideoKeyActionSurface::EguiVideo {
            video_player(&app, 0).set_native_presenter_hidden_for_test(true);
        }
        video_player(&app, 0).set_playing(false);
        video_player(&app, 0).set_muted(false);
        video_player(&app, 0).set_volume(0.5);
        let before = (
            video_player(&app, 0).position(),
            video_player(&app, 0).current_seek_serial(),
        );
        assert!(!video_player(&app, 0).intent_playing());
        app.operation_script_events = Default::default();
        app.operation_script.state =
            event_script(r#"["VideoMute","VideoSeekStart","VideoVolumeUp"]"#);
        let (_, samples) = capture_event_executions(|| {
            frame(&ctx, |ctx| {
                app.detect_operation_script_events(ctx, ScriptInputSurface::Viewer);
                app.poll_video(ctx);
            })
        });
        assert!(toast(&app).contains("EventNotAllowed"));
        assert!(toast(&app).contains("VideoSeekStart"));
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].0, 0);
        assert!(!video_player(&app, 0).is_playing());
        assert!(!video_player(&app, 0).intent_playing());
        assert!(!video_player(&app, 0).clock_is_seeking_for_test());
        assert!(!video_player(&app, 0).is_muted());
        assert_eq!(video_player(&app, 0).volume(), 0.5);
        assert_eq!(
            (
                video_player(&app, 0).position(),
                video_player(&app, 0).current_seek_serial()
            ),
            before
        );
        assert!(requests.try_recv().is_err());
        assert!(app.media_navigation_pending.is_none());
        assert_eq!(app.fullscreen_idx, Some(0));
        app.fs_feedback_toast = None;
        frame(&ctx, |ctx| {
            app.detect_operation_script_events(ctx, ScriptInputSurface::Viewer);
            app.poll_video(ctx);
        });
        assert!(app.fs_feedback_toast.is_none());
        assert!(!video_player(&app, 0).intent_playing());
    }
}
