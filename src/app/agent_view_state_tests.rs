//! Pure/headless snapshots and fake UI requests; no product windows or input.
#![cfg(windows)]
use super::*;
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};

fn fixture() -> AppTestEnv {
    let mut app = setup_app();
    app.initialized = true;
    // Match the existing agent-write fixtures: startup is terminal without
    // starting background indexers, so the startup overlay is inactive.
    app.indexer_init = IndexerInit::Unavailable;
    app.settings.agent_level = AgentLevel::ReadOnly;
    app.settings.agent_show_current_view = true;
    app.settings.book_root = Some(app.tmp.path().join("books"));
    app.current_folder = Some(app.tmp.path().join("folder-not-on-disk"));
    app
}

fn state(app: &App) -> ViewState {
    app.agent_view_state(Path::new("C:/unused-default-books"))
        .unwrap()
}

#[test]
fn agent_view_state_classifies_every_list_kind_without_probing_paths() {
    let mut app = fixture();
    for (index, expected) in [
        ViewKind::Folder,
        ViewKind::SearchResults,
        ViewKind::Tag,
        ViewKind::ReadingHistory,
        ViewKind::Bookmark,
        ViewKind::Rating,
        ViewKind::SmartFolder,
        ViewKind::SubfolderExpansion,
        ViewKind::DriveList,
        ViewKind::Other,
    ]
    .into_iter()
    .enumerate()
    {
        app.items_are_global_search_view = index == 1;
        app.items_are_tag_view = index == 2;
        app.items_are_reading_history_view = index == 3;
        app.items_are_bookmark_view = index == 4;
        app.items_are_rating_view = index == 5;
        app.items_are_smart_folder_view = index == 6;
        app.items_are_subfolder_expansion_view = index == 7;
        app.items_are_drive_list = index == 8;
        if index == 9 {
            app.current_folder = None;
        }
        assert_eq!(state(&app).view, expected);
    }
}

#[test]
fn agent_view_state_reads_logical_target_and_linked_f12_main_without_mutation() {
    let mut app = fixture();
    let first = app.tmp.path().join("selected.png");
    let next = app.tmp.path().join("switching-to.mp4");
    app.items = vec![
        GridItem::Image(first.clone()),
        GridItem::Video(next.clone()),
    ];
    app.selected = Some(0);
    assert_eq!(state(&app).item.unwrap().path, first.to_string_lossy());
    app.fullscreen_idx = Some(1);
    // Headless representation of main's linked F12 presentation. No window is
    // launched and no detached predicate or context mount is needed to read it.
    app.settings.detached_viewer_enabled = true;
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    app.fs_viewport_presentation = Some(ViewerPresentation::DetachedWindow);
    app.fs_viewport_shown = true;
    assert!(!app.detached_viewer_independent_active);
    let before_items = app.items.clone();
    let before_generation = app.items_generation;
    let before_folder = app.current_folder.clone();
    let result = state(&app);
    assert!(result.fullscreen);
    assert_eq!(result.item.unwrap().path, next.to_string_lossy());
    assert_eq!(app.items, before_items);
    assert_eq!(app.items_generation, before_generation);
    assert_eq!(app.current_folder, before_folder);
    assert_eq!(app.selected, Some(0));
    assert_eq!(app.fullscreen_idx, Some(1));
    app.fullscreen_idx = Some(99);
    assert!(
        state(&app).item.is_none(),
        "invalid fullscreen target must not fall back to the selection"
    );
}

#[test]
fn agent_view_state_projects_zip_pdf_and_index_ownership_in_memory() {
    let mut app = fixture();
    let root = app.tmp.path().join("indexed");
    let mut favorite = crate::settings::FavoriteEntry::new("Indexed".into(), root.clone());
    favorite.auto_index_metadata = true;
    app.settings.favorites = vec![favorite];
    app.settings.book_root = Some(root.join("books"));
    app.current_folder = Some(root.clone());
    assert!(state(&app).search.favorite_root);
    app.current_folder = Some(root.join("book.zip"));
    let indexed = state(&app);
    assert!(indexed.search.item_index);
    assert!(!indexed.search.favorite_root);
    assert_eq!(indexed.search.favorite.unwrap().name, "Indexed");
    app.current_folder = app.settings.book_root.clone();
    assert!(!state(&app).search.item_index);
    let archive = root.join("book.zip");
    let zip = view_item(&GridItem::ZipImage {
        zip_path: archive.clone(),
        entry_name: "inner.cbz/a.png".into(),
    });
    assert_eq!(zip.path, archive.to_string_lossy());
    assert_eq!(zip.subresource.as_deref(), Some("inner.cbz/a.png"));
    let dir = view_item(&GridItem::ZipDir {
        zip_path: archive,
        dir_prefix: "inner.cbz/".into(),
        is_archive: true,
        representative: None,
    });
    assert_eq!(dir.subresource.as_deref(), Some("inner.cbz/"));
    let pdf = view_item(&GridItem::PdfPage {
        pdf_path: root.join("book.pdf"),
        page_num: 2,
        content_type: None,
    });
    assert_eq!(pdf.subresource.as_deref(), Some("page_2"));
    assert_eq!(pdf.name, "Page 3");
    app.settings.favorites[0].auto_index_metadata = false;
    app.current_folder = Some(root);
    let not_indexed = state(&app);
    assert!(not_indexed.search.favorite_root);
    assert!(!not_indexed.search.item_index);
}

#[test]
fn agent_view_state_ui_rechecks_permission_and_refuses_remote_but_allows_dialogs() {
    let mut app = fixture();
    app.show_preferences = true;
    assert!(app.agent_view_state(Path::new("C:/books")).is_ok());
    for (level, allowed) in [(AgentLevel::Disabled, true), (AgentLevel::ReadOnly, false)] {
        app.settings.agent_level = level;
        app.settings.agent_show_current_view = allowed;
        assert_eq!(
            app.agent_view_state(Path::new("C:/books"))
                .unwrap_err()
                .code,
            ErrorCode::Disabled
        );
    }
    app.settings.agent_level = AgentLevel::ReadOnly;
    app.settings.agent_show_current_view = true;
    let session = crate::remote_ipc::session::SessionHandle::new();
    app.set_remote_session_handle(session.clone());
    session.acquire(mimageviewer_ipc::SessionAcquireRequest {
        client_id: "view-test".into(),
        peer: mimageviewer_ipc::SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: None,
        },
    });
    assert_eq!(
        app.agent_view_state(Path::new("C:/books"))
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
}

#[test]
fn agent_view_state_queued_read_rechecks_live_permission_without_activation() {
    for mode in ["allowed", "disabled", "remote"] {
        let mut app = fixture();
        let session = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(session.clone());
        let ctx = egui::Context::default();
        let controller = crate::agent_ipc::open::OpenController::new(None);
        app.install_agent_open(controller.clone(), &ctx);
        let (ready, queued) = mpsc::channel();
        controller.set_enqueue_observer(ready);
        let worker = std::thread::spawn(move || controller.execute_view_state_for_test());
        queued.recv_timeout(Duration::from_secs(10)).unwrap();
        if mode == "remote" {
            session.acquire(mimageviewer_ipc::SessionAcquireRequest {
                client_id: "queued-view".into(),
                peer: mimageviewer_ipc::SessionPeerInfo {
                    connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                    device_name: None,
                },
            });
        } else if mode == "disabled" {
            app.settings.agent_show_current_view = false;
        }
        app.poll_agent_open_requests(&ctx);
        let result = worker.join().unwrap();
        if mode == "allowed" {
            let state = result.unwrap();
            assert_eq!(state.view, ViewKind::Folder);
            assert_eq!(
                state.folder,
                app.current_folder
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
            );
        } else {
            assert_eq!(
                result.unwrap_err().code,
                if mode == "remote" {
                    ErrorCode::Busy
                } else {
                    ErrorCode::Disabled
                }
            );
        }
        assert!(app.startup_open_path_resolve_pending.is_none());
        assert!(app.activation_open_path_rx.try_recv().is_err());
    }
}

#[test]
fn agent_view_state_refuses_startup_and_keeps_absent_targets_explicit() {
    let mut app = fixture();
    for (initialized, indexer_init) in [
        (false, IndexerInit::Unavailable),
        (true, IndexerInit::NotStarted),
    ] {
        app.initialized = initialized;
        app.indexer_init = indexer_init;
        assert_eq!(
            app.agent_view_state(Path::new("C:/books"))
                .unwrap_err()
                .code,
            ErrorCode::NotReady
        );
    }
    app.initialized = true;
    app.indexer_init = IndexerInit::Unavailable;
    app.current_folder = None;
    app.items.clear();
    app.selected = None;
    app.fullscreen_idx = None;
    let result = state(&app);
    assert_eq!(result.view, ViewKind::Other);
    assert!(result.folder.is_none() && result.item.is_none());
    assert!(!result.fullscreen && !result.search.item_index);
    assert!(result.search.favorite.is_none());
}
