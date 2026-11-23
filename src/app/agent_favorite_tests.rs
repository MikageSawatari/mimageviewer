//! Headless favorite writes through App admission, persistence and real index owners.
#![cfg(windows)]
use super::*;
use crate::agent_ipc::changes::{ChangeRowOutcome, ChangeRowResult};
use crate::agent_ipc::favorite::PreparedFavorite;
use crate::agent_ipc::open::{OpenController, UiOpenOutcome};
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};
use crate::settings::{FavoriteEntry, MAX_FAVORITES, Settings};
use mimageviewer_agent_ipc::{
    AgentLevel, ErrorCode, FavoriteIndexState, MAX_PAGE_CHARS, Payload, mcp_content_chars,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

fn fixture(ctx: &egui::Context) -> AppTestEnv {
    let mut app = setup_app();
    app.initialized = true;
    app.indexer_init = IndexerInit::Unavailable;
    app.settings.agent_level = AgentLevel::ReadOnly;
    app.settings.agent_allow_settings = true;
    app.settings.book_root = Some(app.tmp.path().join("books"));
    app.settings.favorites.clear();
    app.install_agent_changes(crate::agent_ipc::changes::ChangeController::new(), ctx);
    app
}

fn request(app: &AppTestEnv, folder: &str) -> PreparedFavorite {
    let path = app.tmp.path().join(folder);
    std::fs::create_dir_all(&path).unwrap();
    PreparedFavorite {
        path,
        folder_check: Ok(()),
        name: None,
        item_index: None,
        container_index: None,
        default_books_root: app.tmp.path().join("books"),
        client_name: "favorite-client".into(),
    }
}

fn assert_no_publication(app: &App) {
    let ui = app.agent_changes.as_ref().unwrap();
    assert!(ui.controller.records().lock().unwrap().entries().is_empty());
    assert!(matches!(
        ui.notices.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

fn assert_final_budget(value: &impl serde::Serialize) {
    assert!(mcp_content_chars(value).unwrap() <= MAX_PAGE_CHARS);
    let structured = serde_json::to_value(value).unwrap();
    let text = serde_json::to_string(&structured).unwrap();
    let envelope = serde_json::json!({"jsonrpc":"2.0", "id":"\0".repeat(256), "result":{
        "content":[{"type":"text","text":text}], "structuredContent":structured, "isError":false,
        "resultType":"complete", "_meta":{"io.modelcontextprotocol/serverInfo":{
            "name":"mimageviewer", "version":env!("CARGO_PKG_VERSION")
        }}
    }});
    assert!(
        serde_json::to_string(&envelope).unwrap().chars().count()
            <= mimageviewer_agent_ipc::MAX_MCP_RESPONSE_CHARS
    );
}

fn assert_favorite_eq(actual: &FavoriteEntry, expected: &FavoriteEntry) {
    assert_eq!(actual.id, expected.id);
    assert_eq!(actual.name, expected.name);
    assert_eq!(actual.path, expected.path);
    assert_eq!(actual.auto_index_metadata, expected.auto_index_metadata);
    assert_eq!(actual.auto_index_structure, expected.auto_index_structure);
    assert_eq!(actual.auto_index_thumbs, expected.auto_index_thumbs);
    assert_eq!(actual.auto_index_similar, expected.auto_index_similar);
}

fn acquire_remote(session: &crate::remote_ipc::session::SessionHandle) {
    session.acquire(mimageviewer_ipc::SessionAcquireRequest {
        client_id: "favorite-test".into(),
        peer: mimageviewer_ipc::SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: None,
        },
    });
}

#[test]
fn agent_favorite_add_defaults_and_trimmed_names_persist_and_explain_manual_removal() {
    let ctx = egui::Context::default();
    for explicit in [false, true] {
        let mut app = fixture(&ctx);
        let mut prepared = request(&app, "猫の写真");
        if explicit {
            prepared.name = Some("  写真ライブラリ\t ".into());
        }
        let expected_name = if explicit {
            "写真ライブラリ"
        } else {
            "猫の写真"
        };
        let result = app.agent_set_favorite(&prepared).unwrap();
        assert!(result.added);
        assert_eq!(result.before, None);
        assert_eq!(
            result.after,
            FavoriteIndexState {
                item_index: false,
                container_index: false
            }
        );
        assert_eq!(result.name, expected_name);
        assert_eq!(Path::new(&result.path), prepared.path);
        let message = result.message.as_deref().unwrap();
        assert!(message.contains("AI") && message.contains("削除できません"));
        assert!(message.contains("お気に入り → 編集"));
        assert_final_budget(&result);
        assert_eq!(app.settings.favorites.len(), 1);
        let favorite = &app.settings.favorites[0];
        assert_ne!(favorite.id, uuid::Uuid::nil());
        assert_eq!(favorite.name, expected_name);
        assert_eq!(favorite.path, prepared.path);
        assert!(!favorite.auto_index_metadata && !favorite.auto_index_structure);
        assert!(!favorite.auto_index_thumbs && !favorite.auto_index_similar);
        assert_favorite_eq(&Settings::load().favorites[0], favorite);
        assert_eq!(app.cc.checked.len(), 1);
        let ui = app.agent_changes.as_ref().unwrap();
        let records = ui.controller.records();
        let records = records.lock().unwrap();
        assert_eq!(records.entries().len(), 1);
        let record = &records.entries()[0];
        assert_eq!(record.tool, "set_favorite");
        assert_eq!(record.client_name, prepared.client_name);
        assert!(record.counts.changed > 0);
        assert!(!record.rows.is_empty());
        assert!(
            record
                .rows
                .iter()
                .all(|row| row.result == ChangeRowResult::Done(ChangeRowOutcome::Changed))
        );
        let rendered = record
            .rows
            .iter()
            .map(|row| format!("{} {} {}", row.item, row.before, row.after))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains(expected_name));
        assert!(rendered.contains(&result.path));
        assert!(rendered.contains(message));
        let notice = ui.notices.try_recv().unwrap();
        assert_eq!(notice, record.notice);
        assert!(notice.contains(expected_name));
        assert!(notice.contains(&result.path));
        assert!(notice.contains(message));
        assert!(app.pref_state.is_none());
        assert!(!app.show_fav_add_dialog);
    }
}

#[test]
fn agent_favorite_existing_updates_only_supplied_flags_and_keeps_identity_and_other_options() {
    let ctx = egui::Context::default();
    for item_flag in [true, false] {
        let mut app = fixture(&ctx);
        let mut prepared = request(&app, "existing");
        let mut original = FavoriteEntry::new("利用者の名前".into(), prepared.path.clone());
        original.auto_index_metadata = !item_flag;
        original.auto_index_structure = item_flag;
        original.auto_index_thumbs = true;
        original.auto_index_similar = true;
        app.settings.favorites.push(original.clone());
        assert!(app.settings.save_checked());
        prepared.name = Some("ignored rename".into());
        if item_flag {
            prepared.item_index = Some(true);
        } else {
            prepared.container_index = Some(true);
        }
        let result = app.agent_set_favorite(&prepared).unwrap();
        assert!(!result.added);
        assert_eq!(result.name, original.name);
        assert_eq!(
            result.before,
            Some(FavoriteIndexState {
                item_index: !item_flag,
                container_index: item_flag
            })
        );
        assert_eq!(
            result.after,
            FavoriteIndexState {
                item_index: true,
                container_index: true
            }
        );
        assert!(result.message.is_none());
        assert_final_budget(&result);
        let mut expected = original;
        expected.auto_index_metadata = true;
        expected.auto_index_structure = true;
        assert_eq!(app.settings.favorites.len(), 1);
        assert_favorite_eq(&app.settings.favorites[0], &expected);
        assert_favorite_eq(&Settings::load().favorites[0], &expected);
        let ui = app.agent_changes.as_ref().unwrap();
        let records = ui.controller.records();
        let records = records.lock().unwrap();
        let record = &records.entries()[0];
        assert_eq!(record.tool, "set_favorite");
        assert_eq!(record.client_name, prepared.client_name);
        assert_eq!(record.counts.changed, 1);
        let expected_before = if item_flag {
            "アイテム索引: オフ、コンテナ索引: オン"
        } else {
            "アイテム索引: オン、コンテナ索引: オフ"
        };
        assert_eq!(record.rows[0].before, expected_before);
        assert_eq!(
            record.rows[0].after,
            "アイテム索引: オン、コンテナ索引: オン"
        );
        assert!(record.rows[0].item.contains(&result.name));
        assert!(record.rows[0].item.contains(&result.path));
        assert!(record.rows.iter().any(|row| row.before != row.after));
        let notice = ui.notices.try_recv().unwrap();
        assert_eq!(notice, record.notice);
        assert!(notice.contains(expected_before));
        assert!(notice.contains(&record.rows[0].after));
    }
}

#[test]
fn agent_favorite_noop_skips_save_records_and_notice_even_when_name_is_supplied() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let mut prepared = request(&app, "unchanged");
    app.settings.favorites.push(FavoriteEntry::new(
        "saved-name".into(),
        prepared.path.clone(),
    ));
    assert!(app.settings.save_checked());
    // A persisted sentinel catches an accidental save of otherwise equal flags.
    app.settings.favorites[0].name = "live-name".into();
    let generation = crate::settings::save_generation();
    let toast = app.fs_feedback_toast.as_ref().map(|toast| toast.0.clone());
    for explicit in [false, true] {
        prepared.name = Some("must not rename".into());
        prepared.item_index = explicit.then_some(false);
        prepared.container_index = explicit.then_some(false);
        let result = app.agent_set_favorite(&prepared).unwrap();
        assert!(!result.added);
        assert_eq!(result.name, "live-name");
        assert_eq!(result.before, Some(result.after));
        assert_eq!(app.settings.favorites[0].name, "live-name");
        assert_eq!(Settings::load().favorites[0].name, "saved-name");
        assert_eq!(crate::settings::save_generation(), generation);
        assert_eq!(
            app.fs_feedback_toast.as_ref().map(|toast| toast.0.clone()),
            toast
        );
        assert_no_publication(&app);
    }
}

#[test]
fn agent_favorite_missing_folder_uses_live_registration_to_allow_off_or_refuse_new() {
    let ctx = egui::Context::default();
    for registered_at_commit in [true, false] {
        let mut app = fixture(&ctx);
        let mut prepared = request(&app, "disconnected");
        std::fs::remove_dir(&prepared.path).unwrap();
        prepared.folder_check = Err(AgentError::new(
            ErrorCode::BadRequest,
            "指定されたフォルダを確認できません",
        ));
        prepared.item_index = Some(false);
        prepared.container_index = Some(false);
        let mut original = FavoriteEntry::new("利用者の名前".into(), prepared.path.clone());
        original.auto_index_metadata = true;
        original.auto_index_structure = true;
        original.auto_index_thumbs = true;
        original.auto_index_similar = true;
        // Deliberately change registration after queuing, in both directions.
        if !registered_at_commit {
            app.settings.favorites.push(original.clone());
        }
        assert!(app.settings.save_checked());
        let controller = OpenController::new(None);
        app.install_agent_open(controller.clone(), &ctx);
        let (ready, queued) = mpsc::channel();
        controller.set_enqueue_observer(ready);
        let worker = std::thread::spawn(move || controller.execute_favorite_for_test(prepared));
        queued.recv_timeout(Duration::from_secs(10)).unwrap();
        if registered_at_commit {
            app.settings.favorites.push(original.clone());
        } else {
            app.settings.favorites.clear();
        }
        assert!(app.settings.save_checked());
        let generation = crate::settings::save_generation();
        app.poll_agent_open_requests(&ctx);
        let outcome = worker.join().unwrap().unwrap();
        if registered_at_commit {
            let UiOpenOutcome::Written(Ok(Payload::FavoriteWrite(result))) = outcome else {
                panic!("expected a favorite response");
            };
            assert!(!result.added);
            assert_eq!(result.before, Some(state(&original)));
            assert!(!result.after.item_index && !result.after.container_index);
            original.auto_index_metadata = false;
            original.auto_index_structure = false;
            assert_favorite_eq(&app.settings.favorites[0], &original);
            assert_favorite_eq(&Settings::load().favorites[0], &original);
            assert_final_budget(&result);
            let ui = app.agent_changes.as_ref().unwrap();
            assert_eq!(ui.controller.records().lock().unwrap().entries().len(), 1);
            assert!(ui.notices.try_recv().unwrap().contains("オフ"));
        } else {
            let UiOpenOutcome::Refused(error) = outcome else {
                panic!("expected a new registration refusal");
            };
            assert_eq!(error.code, ErrorCode::BadRequest);
            assert!(app.settings.favorites.is_empty());
            assert!(Settings::load().favorites.is_empty());
            assert_eq!(crate::settings::save_generation(), generation);
            assert_no_publication(&app);
        }
    }
}

#[test]
fn agent_favorite_limit_refuses_add_but_allows_existing_flag_update() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let mut existing = request(&app, "at-cap");
    app.settings
        .favorites
        .push(FavoriteEntry::new("at-cap".into(), existing.path.clone()));
    for index in 1..MAX_FAVORITES {
        let path = app.tmp.path().join(format!("favorite-{index}"));
        app.settings
            .favorites
            .push(FavoriteEntry::new(format!("Favorite {index}"), path));
    }
    assert!(app.settings.save_checked());
    let overflow = request(&app, "overflow");
    let generation = crate::settings::save_generation();
    let error = app.agent_set_favorite(&overflow).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadRequest);
    assert!(error.message.contains("100"));
    assert_eq!(app.settings.favorites.len(), MAX_FAVORITES);
    assert_eq!(Settings::load().favorites.len(), MAX_FAVORITES);
    assert_eq!(crate::settings::save_generation(), generation);
    assert_no_publication(&app);
    existing.item_index = Some(true);
    assert!(!app.agent_set_favorite(&existing).unwrap().added);
    assert!(app.settings.favorites[0].auto_index_metadata);
    assert!(Settings::load().favorites[0].auto_index_metadata);
    assert_eq!(app.settings.favorites.len(), MAX_FAVORITES);
}

#[test]
fn agent_favorite_permission_startup_modal_and_hidden_draft_refuse_before_mutation() {
    let ctx = egui::Context::default();
    for mode in [
        "level",
        "permission",
        "startup",
        "overlay",
        "modal",
        "preferences",
        "draft",
        "fav-add",
        "fav-edit",
        "remote",
    ] {
        let mut app = fixture(&ctx);
        let prepared = request(&app, "refused");
        assert!(app.settings.save_checked());
        let session = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(session.clone());
        let mut draft_pointer = None;
        let expected = match mode {
            "level" => {
                app.settings.agent_level = AgentLevel::Disabled;
                ErrorCode::Disabled
            }
            "permission" => {
                app.settings.agent_allow_settings = false;
                ErrorCode::Disabled
            }
            "startup" => {
                app.initialized = false;
                ErrorCode::NotReady
            }
            "overlay" => {
                app.indexer_init = IndexerInit::NotStarted;
                ErrorCode::NotReady
            }
            "modal" => {
                app.show_about_dialog = true;
                ErrorCode::Busy
            }
            "preferences" => {
                app.show_preferences = true;
                ErrorCode::Busy
            }
            "fav-add" => {
                app.show_fav_add_dialog = true;
                ErrorCode::Busy
            }
            "fav-edit" => {
                app.show_favorites_editor = true;
                ErrorCode::Busy
            }
            "draft" => {
                let candidate = app.settings.preferences_snapshot();
                app.install_headless_preferences_state(candidate);
                draft_pointer = Some(std::ptr::from_ref(
                    app.pref_state.as_ref().unwrap().as_ref(),
                ));
                ErrorCode::Busy
            }
            "remote" => {
                acquire_remote(&session);
                ErrorCode::Busy
            }
            _ => unreachable!(),
        };
        let generation = crate::settings::save_generation();
        let error = app.agent_set_favorite(&prepared).unwrap_err();
        assert_eq!(error.code, expected, "{mode}");
        assert!(app.settings.favorites.is_empty(), "{mode}");
        assert!(Settings::load().favorites.is_empty(), "{mode}");
        assert_eq!(crate::settings::save_generation(), generation, "{mode}");
        if let Some(pointer) = draft_pointer {
            assert_eq!(
                std::ptr::from_ref(app.pref_state.as_ref().unwrap().as_ref()),
                pointer
            );
        }
        assert_no_publication(&app);
    }
}

#[test]
fn agent_favorite_queued_write_rechecks_live_admission_without_opening_or_activation() {
    let ctx = egui::Context::default();
    for mode in [
        "allowed",
        "permission",
        "level",
        "startup",
        "modal",
        "remote",
    ] {
        let mut app = fixture(&ctx);
        let prepared = request(&app, "queued");
        assert!(app.settings.save_checked());
        let session = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(session.clone());
        let controller = OpenController::new(Some(session.clone()));
        app.install_agent_open(controller.clone(), &ctx);
        let (ready, queued) = mpsc::channel();
        controller.set_enqueue_observer(ready);
        let worker = std::thread::spawn(move || controller.execute_favorite_for_test(prepared));
        queued.recv_timeout(Duration::from_secs(10)).unwrap();
        match mode {
            "permission" => app.settings.agent_allow_settings = false,
            "level" => app.settings.agent_level = AgentLevel::Disabled,
            "startup" => app.initialized = false,
            "modal" => app.show_fav_add_dialog = true,
            "remote" => acquire_remote(&session),
            "allowed" => {}
            _ => unreachable!(),
        }
        app.poll_agent_open_requests(&ctx);
        match (mode, worker.join().unwrap().unwrap()) {
            ("allowed", UiOpenOutcome::Written(Ok(Payload::FavoriteWrite(result)))) => {
                assert!(result.added);
                assert_eq!(app.settings.favorites.len(), 1);
                assert_eq!(Settings::load().favorites.len(), 1);
            }
            (mode, UiOpenOutcome::Refused(error)) if mode != "allowed" => {
                assert_eq!(
                    error.code,
                    match mode {
                        "startup" => ErrorCode::NotReady,
                        "modal" | "remote" => ErrorCode::Busy,
                        _ => ErrorCode::Disabled,
                    }
                );
                assert!(app.settings.favorites.is_empty());
                assert!(Settings::load().favorites.is_empty());
                assert_no_publication(&app);
            }
            (mode, outcome) => panic!("unexpected queued favorite outcome: {mode}: {outcome:?}"),
        }
        assert!(app.pref_state.is_none());
        assert!(app.startup_open_path_resolve_pending.is_none());
        assert!(app.activation_open_path_rx.try_recv().is_err());
    }
}

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while !ready() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn agent_favorite_both_indexes_on_then_off_reconfigures_real_owners_and_cleans_only_index_data() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let mut prepared = request(&app, "indexed-pictures");
    let image_path = prepared.path.join("one.png");
    image::RgbImage::new(2, 2).save(&image_path).unwrap();
    let tag_key = crate::tags_db::item_key_for_path(&image_path);
    app.tags_db
        .as_mut()
        .unwrap()
        .set_item_tags(&tag_key, ["保持するタグ"], "favorite-test")
        .unwrap();
    let tags_before = app.tags_db.as_ref().unwrap().get_item_tags(&tag_key);
    assert!(!tags_before.is_empty());
    std::fs::create_dir(prepared.path.join("子フォルダ")).unwrap();
    let index_data = app.tmp.path().join("favorite-index-stores");
    let manager = crate::indexer_manager::IndexerManager::new_at(
        &index_data,
        &[],
        crate::settings::IndexerSpeedProfile::default(),
        Arc::new(crate::activity_gate::ActivityGate::new(0)),
        vec![app.tmp.path().join("books")],
    )
    .unwrap();
    let meta = manager.clone_fts_meta();
    app.indexer_init = IndexerInit::Ready(manager);
    let names = app.search_index_db.as_ref().unwrap().clone();
    app.name_index_manager =
        Some(crate::name_index_manager::NameIndexManager::new(names.clone(), None).unwrap());
    prepared.item_index = Some(true);
    prepared.container_index = Some(true);
    let added = app.agent_set_favorite(&prepared).unwrap();
    assert!(added.added && added.after.item_index && added.after.container_index);
    let original = app.settings.favorites[0].clone();
    let id = original.id;
    let image_key = crate::search_index_db::normalize_path(&image_path);
    wait_until(
        "AI favorite enables item and container supervisors and stores",
        || {
            let manager = app.indexer_init.as_ref().unwrap();
            !manager.is_reconciling()
                && manager.all_stats().iter().any(|entry| {
                    entry.favorite_id == id
                        && entry.favorite_path == prepared.path
                        && entry.stats.initial_scan_done
                })
                && meta.get(&image_key).unwrap().is_some_and(|entry| {
                    entry.favorite_id == id
                        && crate::path_key::eq_keep_drive(&entry.favorite_root, &prepared.path)
                })
                && app
                    .name_index_manager
                    .as_ref()
                    .unwrap()
                    .all_initial_scans_done()
                && app.name_index_stats_by_id().contains_key(&id)
                && names.count_for_favorite(&prepared.path).unwrap() > 0
        },
    );
    assert!(Settings::load().favorites[0].auto_index_metadata);
    assert!(Settings::load().favorites[0].auto_index_structure);
    // Keep an ON snapshot: stats_by_id filters its argument's flags, so querying
    // with the OFF settings would fail to prove that the actual owner stopped.
    let on_snapshot = [original.clone()];
    prepared.item_index = Some(false);
    prepared.container_index = Some(false);
    let reverted = app.agent_set_favorite(&prepared).unwrap();
    assert_eq!(reverted.before, Some(added.after));
    assert_eq!(
        reverted.after,
        FavoriteIndexState {
            item_index: false,
            container_index: false
        }
    );
    wait_until(
        "AI favorite OFF stops real owners and clears rebuildable stores",
        || {
            let manager = app.indexer_init.as_ref().unwrap();
            let name_manager = app.name_index_manager.as_ref().unwrap();
            !manager.is_reconciling()
                && manager
                    .all_stats()
                    .iter()
                    .all(|entry| entry.favorite_id != id)
                && meta.get(&image_key).unwrap().is_none()
                && name_manager.all_initial_scans_done()
                && name_manager.stats_by_id(&on_snapshot).is_empty()
                && names.count_for_favorite(&prepared.path).unwrap() == 0
        },
    );
    let mut expected = original;
    expected.auto_index_metadata = false;
    expected.auto_index_structure = false;
    assert_favorite_eq(&app.settings.favorites[0], &expected);
    assert_favorite_eq(&Settings::load().favorites[0], &expected);
    assert!(image_path.is_file());
    assert!(prepared.path.join("子フォルダ").is_dir());
    assert_eq!(
        app.tags_db.as_ref().unwrap().get_item_tags(&tag_key),
        tags_before
    );
}

#[test]
fn agent_favorite_oversized_response_refuses_before_save_or_publication() {
    let ctx = egui::Context::default();
    for large_path in [false, true] {
        let mut app = fixture(&ctx);
        let mut prepared = request(&app, "budget");
        if large_path {
            // A prepared boundary fixture: avoid constructing an impossible OS
            // path on disk while exercising the complete response preflight.
            prepared.path = PathBuf::from(format!("C:/{}", "x".repeat(MAX_PAGE_CHARS)));
        } else {
            prepared.name = Some("\\\"\n".repeat(MAX_PAGE_CHARS));
        }
        assert!(app.settings.save_checked());
        let generation = crate::settings::save_generation();
        let error = app.agent_set_favorite(&prepared).unwrap_err();
        assert_eq!(error.code, ErrorCode::BadRequest);
        assert!(app.settings.favorites.is_empty());
        assert!(Settings::load().favorites.is_empty());
        assert_eq!(crate::settings::save_generation(), generation);
        assert_no_publication(&app);
        assert!(app.pref_state.is_none());
    }
}

#[test]
fn agent_favorite_existing_path_spellings_update_without_duplicate_registration() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let mut prepared = request(&app, "Mixed-Case");
    app.agent_set_favorite(&prepared).unwrap();
    let id = app.settings.favorites[0].id;
    let path = app.settings.favorites[0].path.clone();
    for spelling in [
        format!("{}\\", path.display()),
        path.to_string_lossy().to_ascii_lowercase(),
    ] {
        prepared.path = PathBuf::from(spelling);
        prepared.item_index = Some(!app.settings.favorites[0].auto_index_metadata);
        let result = app.agent_set_favorite(&prepared).unwrap();
        assert!(!result.added);
        assert_eq!(app.settings.favorites.len(), 1);
        assert_eq!(app.settings.favorites[0].id, id);
        assert_eq!(app.settings.favorites[0].path, path);
    }
}

#[test]
fn agent_favorite_new_book_library_targets_match_the_dialog_exclusion() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let mut prepared = request(&app, "book-target");
    prepared.path = app.settings.book_root.as_ref().unwrap().join("book");
    std::fs::create_dir_all(&prepared.path).unwrap();
    let generation = crate::settings::save_generation();
    assert_eq!(
        app.agent_set_favorite(&prepared).unwrap_err().code,
        ErrorCode::BadRequest
    );
    assert_eq!(crate::settings::save_generation(), generation);
    assert!(app.settings.favorites.is_empty());
    assert_no_publication(&app);
}
