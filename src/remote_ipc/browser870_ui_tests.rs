fn browser870_apply_payload(
    app: &mut crate::app::App,
    handle: &SessionHandle,
    owner: &RemoteSessionIdentity,
    payload: super::super::session::RemoteWritePayload,
    admission: crate::book_composition::CompositionWriteAdmission,
) -> RemoteWriteResponse {
    let operation = handle
        .begin_operation(owner, "browser870 reading settings".into())
        .unwrap();
    let producer = handle.clone();
    let worker = std::thread::spawn(move || {
        producer.submit_validated_write(payload, operation, admission)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !worker.is_finished() {
        for request in handle.take_pending_ui_requests() {
            let ClaimedRemoteUiRequest::Write(pending) = request else {
                panic!("unexpected UI request");
            };
            app.apply_pending_remote_write(pending, &egui::Context::default());
        }
        app.poll_remote_reading_writes();
        assert!(std::time::Instant::now() < deadline, "write did not settle");
        std::thread::yield_now();
    }
    let UiWriteOutcome::Write(response) = worker.join().unwrap() else {
        panic!("write lost session ownership");
    };
    response
}

#[test]
fn browser870_remote_all_three_write_routes_reject_without_changing_database() {
    use crate::book_composition::*;
    use crate::settings::{ReadingDirection as Direction, ReadingFlow, SpreadMode};
    let mut app = crate::app::setup_app_for_test();
    let root = app.tmp.path().join("browser");
    let month = root.join("2026-11");
    std::fs::create_dir_all(&month).unwrap();
    let data = tempfile::tempdir().unwrap();
    app.spread_db = Ok(crate::spread_db::SpreadDb::open_at(&data.path().join("spread.db")).unwrap());
    app.spread_db
        .as_ref()
        .unwrap()
        .set_presentation_state(
            &month,
            SpreadMode::RtlCover,
            ReadingFlow::Horizontal,
            Direction::Rtl,
            (SpreadMode::Single, ReadingFlow::Paged, Direction::Ltr),
        )
        .unwrap();
    let before = app
        .spread_db
        .as_ref()
        .unwrap()
        .get_state_with_fallback(&month, None);
    let (service, process) = BookCompositionService::spawn_at(data.path().into(), None).unwrap();
    // A synthetic legacy receipt exercises the mode-only route even if a future
    // composition admission admits directories. The production admission is unchanged.
    let scope = crate::book_scope::BookScopeKey::folder(&month).unwrap();
    service
        .save(
            scope.clone(),
            PageDomain::Document,
            fixtures::four_page_book(ReadingDirection::Ltr),
        )
        .recv()
        .unwrap()
        .status
        .unwrap();
    app.install_book_composition_runtime(service.clone());
    let (handle, owner, _) = active_session();
    app.set_remote_session_handle(handle.clone());
    app.settings.browser_view_output_dir = Some(root.clone());
    let engine = super::super::container::ContainerEngine::new(app.settings.clone());
    for route in 0..3 {
        let address = mimageviewer_ipc::RemoteAddress::file(month.to_string_lossy().into_owned());
        let request = if route == 2 {
            RemoteWriteRequest::SetViewMode {
                address,
                mode: mimageviewer_ipc::RemoteViewMode::Spread,
            }
        } else {
            RemoteWriteRequest::SetSpread {
                address,
                spread_mode: RemoteSpreadMode::Ltr,
                reading_direction: RemoteReadingDirection::Ltr,
            }
        };
        let admission = if route == 1 {
            let value = CompositionWriteAdmission {
                binding_scope: Some(scope.clone()),
                binding_kind: CompositionBindingWriteKind::LegacySpread(SpreadMode::Ltr),
                ..Default::default()
            };
            assert_eq!(
                value.legacy_spread_view_mode(&service.snapshot()).unwrap(),
                Some(SpreadMode::Ltr)
            );
            value
        } else {
            Default::default()
        };
        let payload = engine.qualified_write_payload(request).unwrap();
        assert!(payload.reading_target_is_directory());
        let result = browser870_apply_payload(&mut app, &handle, &owner, payload, admission);
        let RemoteWriteResponse::Error(error) = result else {
            panic!("route {route} accepted");
        };
        assert_eq!(error.code, RemoteWriteErrorCode::Unsupported);
        assert_eq!(
            error.message,
            "ブラウザ閲覧の保存先のフォルダは、単ページで表示します"
        );
        assert_eq!(
            app.spread_db
                .as_ref()
                .unwrap()
                .get_state_with_fallback(&month, None),
            before
        );
        assert!(app.remote_session_ui.pending_reading_writes.is_empty());
    }
    process.shutdown_and_join();
}

#[test]
fn browser870_remote_write_uses_current_root_and_preserves_outside_zip_pdf_writes() {
    use crate::settings::{ReadingDirection, SpreadMode};
    let mut app = crate::app::setup_app_for_test();
    app.settings.remember_spread_shift_per_book = false;
    let root = app.tmp.path().join("browser");
    let month = root.join("2026-11");
    let outside = app.tmp.path().join("outside");
    std::fs::create_dir_all(&month).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let (handle, owner, _) = active_session();
    app.set_remote_session_handle(handle.clone());
    let request = |path: &std::path::Path| RemoteWriteRequest::SetSpread {
        address: mimageviewer_ipc::RemoteAddress::file(path.to_string_lossy().into_owned()),
        spread_mode: RemoteSpreadMode::Ltr,
        reading_direction: RemoteReadingDirection::Ltr,
    };
    // Qualification under the old root must not decide the later UI policy.
    app.settings.browser_view_output_dir = Some(outside.clone());
    let engine = super::super::container::ContainerEngine::new(app.settings.clone());
    let payload = engine.qualified_write_payload(request(&month)).unwrap();
    app.settings.browser_view_output_dir = Some(root.clone());
    assert!(matches!(
        browser870_apply_payload(&mut app, &handle, &owner, payload, Default::default()),
        RemoteWriteResponse::Error(RemoteWriteError {
            code: RemoteWriteErrorCode::Unsupported,
            ..
        })
    ));
    let payload = engine.qualified_write_payload(request(&month)).unwrap();
    app.settings.browser_view_output_dir = Some(outside);
    assert!(matches!(
        browser870_apply_payload(&mut app, &handle, &owner, payload, Default::default()),
        RemoteWriteResponse::Success(_)
    ));
    assert_eq!(
        app.spread_db
            .as_ref()
            .unwrap()
            .get_state_with_fallback(&month, None)
            .mode,
        Some(SpreadMode::Ltr)
    );
    app.settings.browser_view_output_dir = Some(root);
    for path in [
        app.tmp.path().join("outside"),
        app.tmp.path().join("browser/book.zip"),
        app.tmp.path().join("browser/book.pdf"),
    ] {
        if path.extension().is_some() {
            std::fs::write(&path, b"container").unwrap();
        }
        let payload = engine.qualified_write_payload(request(&path)).unwrap();
        assert_eq!(
            payload.reading_target_is_directory(),
            path.extension().is_none()
        );
        assert!(matches!(
            browser870_apply_payload(&mut app, &handle, &owner, payload, Default::default()),
            RemoteWriteResponse::Success(_)
        ));
        let state = app
            .spread_db
            .as_ref()
            .unwrap()
            .get_state_with_fallback(&path, None);
        assert_eq!(state.mode, Some(SpreadMode::Ltr));
        assert_eq!(state.direction, Some(ReadingDirection::Ltr));
    }
}
