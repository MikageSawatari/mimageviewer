#[cfg(test)]
mod spread_shift_d3 {
    use super::*;
    use crate::book_scope::BookScopeKey;
    use crate::spread_shift::{ShiftOperation, SpreadShiftPageRef, SpreadShiftService};
    use std::time::{Duration, Instant};

    fn operation() -> ShiftOperation {
        ShiftOperation {
            context_id: crate::app::ViewerContextId::for_test(901),
            items_generation: 1,
            operation_sequence: 1,
        }
    }

    fn row(service: &SpreadShiftService, scope: &BookScopeKey) -> Option<SpreadShiftPageRef> {
        let reply = service
            .read_after(scope.clone(), service.watermarks().admitted)
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(reply.status.is_ok(), "{:?}", reply.status);
        reply.row
    }

    fn set_spread(address: mimageviewer_ipc::RemoteAddress) -> RemoteWriteRequest {
        RemoteWriteRequest::SetSpread {
            address,
            spread_mode: RemoteSpreadMode::RtlCover,
            reading_direction: RemoteReadingDirection::Rtl,
        }
    }

    #[test]
    fn composition_remote_core_refuses_binding_but_completes_mode_only_worker_write() {
        use crate::book_composition::{
            BookCompositionService, PageDomain, ReadingDirection, fixtures::four_page_book,
        };
        let mut app = crate::app::setup_app_for_test();
        let data = tempfile::tempdir().unwrap();
        let book = data.path().join("book.pdf");
        std::fs::write(&book, b"no page decoding in mode-only fixture").unwrap();
        let scope = BookScopeKey::document(&book).unwrap();
        let db = crate::spread_db::SpreadDb::open_at(&data.path().join("spread.db")).unwrap();
        db.set(
            &book,
            crate::settings::SpreadMode::RtlCover,
            crate::settings::SpreadMode::Single,
            crate::settings::ReadingFlow::Paged,
            crate::settings::ReadingDirection::Ltr,
        )
        .unwrap();
        app.spread_db = Ok(db);
        let (service, process) =
            BookCompositionService::spawn_at(data.path().into(), None).unwrap();
        service
            .save(
                scope.clone(),
                PageDomain::Document,
                four_page_book(ReadingDirection::Ltr),
            )
            .recv_timeout(Duration::from_secs(30))
            .unwrap()
            .status
            .unwrap();
        let snapshot = service.snapshot();
        app.install_book_composition_runtime(service.clone());
        let address = mimageviewer_ipc::RemoteAddress::file(book.to_string_lossy().into_owned());
        assert!(matches!(
            app.apply_remote_write(&set_spread(address.clone())),
            RemoteWriteResponse::Error(RemoteWriteError {
                code: RemoteWriteErrorCode::Unsupported,
                ..
            })
        ));
        assert_eq!(
            app.spread_db.as_ref().unwrap().get(&book),
            Some(crate::settings::SpreadMode::RtlCover)
        );
        let (handle, owner, _) = active_session();
        app.set_remote_session_handle(handle.clone());
        let (pending, reply) = ClaimedRemoteWrite::for_test(
            handle.begin_operation(&owner, "mode-only".into()).unwrap(),
            RemoteWriteRequest::SetViewMode {
                address,
                mode: mimageviewer_ipc::RemoteViewMode::LandscapeSplit,
            },
        );
        app.apply_pending_remote_write(pending, &egui::Context::default());
        let PendingRemoteReadingWrite::ViewSetting { pending, receiver } = app
            .remote_session_ui
            .pending_reading_writes
            .pop()
            .expect("worker owns completion")
        else {
            panic!("mode-only typed completion");
        };
        let completion = receiver.recv_timeout(Duration::from_secs(30)).unwrap();
        let (deliver, receiver) = std::sync::mpsc::channel();
        deliver.send(completion).unwrap();
        app.remote_session_ui
            .pending_reading_writes
            .push(PendingRemoteReadingWrite::ViewSetting { pending, receiver });
        assert!(matches!(
            reply.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        app.poll_remote_reading_writes();
        assert!(matches!(
            reply.recv_timeout(Duration::from_secs(1)).unwrap(),
            UiWriteOutcome::Write(RemoteWriteResponse::Success(_))
        ));
        assert_eq!(
            app.spread_db.as_ref().unwrap().get(&book),
            Some(crate::settings::SpreadMode::SplitRtl)
        );
        assert_eq!(service.snapshot().rows, snapshot.rows);
        assert_eq!(
            service.snapshot().token_for(&scope),
            snapshot.token_for(&scope)
        );
        assert!(app.remote_session_ui.pending_reading_writes.is_empty());
        process.shutdown_and_join();
    }

    #[test]
    fn remote_trim_fifo_reads_queued_toggle_state_and_retires_claim_before_late_reply() {
        use crate::book_composition::{
            BookCompositionService, PageDomain, ReadingDirection, fixtures::four_page_book,
        };
        use crate::view_trim::{
            ViewTrimApplyMode, ViewTrimBookSettings, ViewTrimBookState, ViewTrimLinkedMargins,
            ViewTrimMargins,
        };
        use crate::view_trim_db::{ViewTrimDb, ViewTrimWriteBatch};
        for cancel_before_receipt in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            let data = tempfile::tempdir().unwrap();
            let book = data.path().join("remote-trim.pdf");
            std::fs::write(&book, b"remote trim admission requires no page decode").unwrap();
            let scope = BookScopeKey::document(&book).unwrap();
            let (service, process) =
                BookCompositionService::spawn_at(data.path().into(), None).unwrap();
            service
                .save(
                    scope.clone(),
                    PageDomain::Document,
                    four_page_book(ReadingDirection::Ltr),
                )
                .recv_timeout(Duration::from_secs(30))
                .unwrap()
                .status
                .unwrap();
            let before = service.snapshot();
            app.view_trim_db =
                Some(ViewTrimDb::open_at(&data.path().join("view_trim.db")).unwrap());
            app.install_book_composition_runtime(service.clone());
            app.settings.fullscreen_fit_mode = crate::settings::FullscreenFitMode::MarginFit;
            app.settings.margin_fit_enabled = true;
            let address =
                mimageviewer_ipc::RemoteAddress::file(book.to_string_lossy().into_owned());
            let key = remote_spread_key(&address).unwrap();
            let older = ViewTrimBookState {
                apply_mode: ViewTrimApplyMode::Book,
                book_settings: ViewTrimBookSettings {
                    enabled: true,
                    spread_separate: true,
                    ..Default::default()
                },
            };
            let requested = ViewTrimBookState {
                apply_mode: ViewTrimApplyMode::Page,
                book_settings: ViewTrimBookSettings {
                    enabled: true,
                    spread_separate: false,
                    spread_linked: ViewTrimLinkedMargins {
                        top: 0.19,
                        ..Default::default()
                    },
                    spread_left: ViewTrimMargins {
                        top: 0.06,
                        right: 0.03,
                        ..Default::default()
                    },
                    spread_right: ViewTrimMargins {
                        top: 0.10,
                        left: 0.07,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            };
            let mut expected = requested;
            expected.apply_mode = ViewTrimApplyMode::None;
            expected.book_settings.spread_separate = true;
            expected.book_settings = expected.book_settings.with_spread_separate(false);
            let (entered, release) = service.hold_worker_for_test();
            entered.recv_timeout(Duration::from_secs(30)).unwrap();
            drop(service.save_view_trim(ViewTrimWriteBatch {
                book: Some((key.exact.clone(), older)),
                pages: vec![],
            }));
            let (handle, owner, _) = active_session();
            app.set_remote_session_handle(handle.clone());
            let (pending, reply) = ClaimedRemoteWrite::for_test(
                handle
                    .begin_operation(&owner, "remote trim FIFO".into())
                    .unwrap(),
                RemoteWriteRequest::SetViewTrim {
                    address: address.clone(),
                    context_address: address,
                    state: serde_json::to_value(requested).unwrap(),
                },
            );
            let ctx = egui::Context::default();
            app.apply_pending_remote_write(pending, &ctx);
            assert!(
                app.view_trim_db
                    .as_ref()
                    .unwrap()
                    .get_book_state(&key.exact)
                    .is_none(),
                "Remote must enqueue rather than write ahead of the held projection batch"
            );
            assert!(matches!(
                reply.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ));
            let PendingRemoteReadingWrite::ViewTrim {
                pending,
                receiver: actual_worker,
            } = app
                .remote_session_ui
                .pending_reading_writes
                .pop()
                .expect("typed Remote trim owner")
            else {
                panic!("Remote trim continuation");
            };
            let (deliver, receiver) = std::sync::mpsc::channel();
            app.remote_session_ui
                .pending_reading_writes
                .push(PendingRemoteReadingWrite::ViewTrim { pending, receiver });
            let mut output = ctx.run_pass(egui::RawInput::default(), |ctx| {
                app.poll_remote_session(ctx)
            });
            output.textures_delta.clear();
            assert!(
                output.viewport_output[&egui::ViewportId::ROOT].repaint_delay
                    <= Duration::from_millis(16)
            );
            assert_eq!(app.remote_session_ui.pending_reading_writes.len(), 1);
            assert!(
                app.settings.margin_fit_enabled,
                "legacy migration waits for a successful current claim's receipt"
            );
            if cancel_before_receipt {
                handle.local_disconnect();
                app.poll_remote_reading_writes();
                assert!(
                    matches!(reply.recv_timeout(Duration::from_secs(1)).unwrap(), UiWriteOutcome::Session(response) if response.status != SessionStatus::Active)
                );
                assert!(app.remote_session_ui.pending_reading_writes.is_empty());
            }
            release.send(()).unwrap();
            service
                .prepare(scope.clone(), PageDomain::Document)
                .recv_timeout(Duration::from_secs(30))
                .unwrap()
                .status
                .unwrap();
            let completion = actual_worker.recv_timeout(Duration::from_secs(30)).unwrap();
            assert_eq!(
                completion.as_ref().unwrap(),
                &expected,
                "spread toggle conversion reads the queued older state inside the worker"
            );
            assert_eq!(
                app.view_trim_db
                    .as_ref()
                    .unwrap()
                    .get_book_state(&key.exact),
                Some(expected)
            );
            if cancel_before_receipt {
                assert!(
                    deliver.send(completion).is_err(),
                    "a late receipt cannot revive its retired Remote claim"
                );
                app.poll_remote_reading_writes();
                assert!(app.settings.margin_fit_enabled);
                assert_eq!(
                    app.settings.fullscreen_fit_mode,
                    crate::settings::FullscreenFitMode::MarginFit
                );
                assert!(matches!(
                    reply.try_recv(),
                    Err(std::sync::mpsc::TryRecvError::Disconnected)
                ));
            } else {
                deliver.send(completion).unwrap();
                app.poll_remote_reading_writes();
                let UiWriteOutcome::Write(RemoteWriteResponse::Success(result)) =
                    reply.recv_timeout(Duration::from_secs(1)).unwrap()
                else {
                    panic!("successful Remote trim terminal response");
                };
                assert_eq!(
                    result.view_trim_state,
                    Some(serde_json::to_value(expected).unwrap())
                );
                assert!(!app.settings.margin_fit_enabled);
                assert_eq!(
                    app.settings.fullscreen_fit_mode,
                    crate::settings::FullscreenFitMode::Page
                );
                assert!(app.remote_session_ui.pending_reading_writes.is_empty());
            }
            let after = service.snapshot();
            assert_eq!(before.rows, after.rows);
            assert_eq!(before.token_for(&scope), after.token_for(&scope));
            process.shutdown_and_join();
        }
    }

    fn zip_effective_address(
        app: &crate::app::App,
        path: &std::path::Path,
        names: &[&str],
    ) -> mimageviewer_ipc::RemoteAddress {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for name in names {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"fixture: no decoding in force-single projection")
                .unwrap();
        }
        zip.finish().unwrap();
        let engine = super::super::super::container::ContainerEngine::new(app.settings.clone());
        let response = engine.container(mimageviewer_ipc::ContainerRequest {
            address: mimageviewer_ipc::RemoteAddress {
                path: path.to_string_lossy().into_owned(),
                subresource: RemoteSubresource::ZipDirectory {
                    prefix: String::new(),
                },
            },
            spread_mode: Some(RemoteSpreadMode::Single),
            reading_direction: Some(RemoteReadingDirection::Ltr),
            force_single_page: true,
        });
        let mimageviewer_ipc::ContainerResponse::Success(payload) = response else {
            panic!("ZIP container fixture failed: {response:?}")
        };
        payload.effective_address
    }

    fn assert_zip_reset(
        app: &mut crate::app::App,
        service: &SpreadShiftService,
        scope: &BookScopeKey,
        address: mimageviewer_ipc::RemoteAddress,
    ) {
        service.drain_results();
        let (handle, owner, _) = active_session();
        app.set_remote_session_handle(handle.clone());
        app.spread_shift_service = Some(service.clone());
        app.settings.remember_spread_shift_per_book = true;
        let before = service.watermarks().admitted;
        let key = remote_spread_key(&address).unwrap();
        let (pending, reply) = ClaimedRemoteWrite::for_test(
            handle
                .begin_operation(&owner, "ZIP set spread".into())
                .unwrap(),
            set_spread(address),
        );
        app.apply_pending_remote_write(pending, &egui::Context::default());
        assert_eq!(service.watermarks().admitted, before + 1);
        assert_eq!(row(service, scope), None);
        assert!(matches!(
            reply.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        let results = service.drain_results();
        assert_eq!(results.len(), 1);
        assert!(results[0].status.is_ok());
        assert!(app.complete_remote_shift_reset(&results[0]));
        assert!(!app.complete_remote_shift_reset(&results[0]));
        assert!(matches!(
            reply.recv_timeout(Duration::from_secs(1)).unwrap(),
            UiWriteOutcome::Write(RemoteWriteResponse::Success(_))
        ));
        assert_eq!(
            app.spread_db.as_ref().unwrap().get(&key.exact),
            Some(crate::settings::SpreadMode::RtlCover)
        );
        assert!(app.remote_session_ui.pending_reading_writes.is_empty());
    }

    #[test]
    fn spread_shift_remote_zip_effective_address_reset_clears_only_accepted_chapter() {
        let mut app = crate::app::setup_app_for_test();
        let data = tempfile::tempdir().unwrap();
        let zip = data.path().join("book.zip");
        let address = zip_effective_address(&app, &zip, &["chapter/a.jpg", "chapter/b.jpg"]);
        assert_eq!(
            address.subresource,
            RemoteSubresource::ZipDirectory {
                prefix: "chapter/".into()
            }
        );
        let scope = BookScopeKey::archive(&zip, &["chapter".into()]).unwrap();
        let root = BookScopeKey::archive(&zip, &[]).unwrap();
        let sibling = BookScopeKey::archive(&zip, &["other".into()]).unwrap();
        let (service, process) =
            SpreadShiftService::spawn_at(data.path().join("store"), None).unwrap();
        let saved = SpreadShiftPageRef::ArchiveEntry("chapter/b.jpg".into());
        for target in [&scope, &root, &sibling] {
            service
                .set_shift(target.clone(), saved.clone(), operation())
                .unwrap();
            assert_eq!(row(&service, target), Some(saved.clone()));
        }
        assert_zip_reset(&mut app, &service, &scope, address);
        assert_eq!(row(&service, &root), Some(saved.clone()));
        assert_eq!(row(&service, &sibling), Some(saved));
        drop(process);
    }

    #[test]
    fn spread_shift_remote_zip_root_reset_accepts_effective_file_and_empty_prefix() {
        let mut app = crate::app::setup_app_for_test();
        let data = tempfile::tempdir().unwrap();
        let zip = data.path().join("book.zip");
        let effective = zip_effective_address(&app, &zip, &["a.jpg", "b.jpg"]);
        assert_eq!(effective.subresource, RemoteSubresource::File);
        let scope = BookScopeKey::archive(&zip, &[]).unwrap();
        let sibling = BookScopeKey::archive(&zip, &["chapter".into()]).unwrap();
        let (service, process) =
            SpreadShiftService::spawn_at(data.path().join("store"), None).unwrap();
        let saved = SpreadShiftPageRef::ArchiveEntry("b.jpg".into());
        service
            .set_shift(sibling.clone(), saved.clone(), operation())
            .unwrap();
        assert_eq!(row(&service, &sibling), Some(saved.clone()));
        // The production root response is File. Also cover its accepted wire
        // alias ZipDirectory { prefix: "" }, which previously failed Clear.
        let empty_prefix = mimageviewer_ipc::RemoteAddress {
            subresource: RemoteSubresource::ZipDirectory {
                prefix: String::new(),
            },
            ..effective.clone()
        };
        assert_eq!(
            remote_spread_key(&effective).unwrap(),
            remote_spread_key(&empty_prefix).unwrap()
        );
        for address in [effective, empty_prefix] {
            service
                .set_shift(scope.clone(), saved.clone(), operation())
                .unwrap();
            assert_eq!(row(&service, &scope), Some(saved.clone()));
            assert_zip_reset(&mut app, &service, &scope, address);
            assert_eq!(row(&service, &sibling), Some(saved.clone()));
        }
        drop(process);
    }

    #[test]
    fn spread_shift_remote_reset_waits_for_clear_commit_and_success_completion() {
        let mut app = crate::app::setup_app_for_test();
        let data = tempfile::tempdir().unwrap();
        let book = data.path().join("book");
        std::fs::create_dir_all(&book).unwrap();
        let scope = BookScopeKey::folder(&book).unwrap();
        let (service, process) = SpreadShiftService::spawn_at(data.path().into(), None).unwrap();
        service
            .set_shift(
                scope.clone(),
                SpreadShiftPageRef::RelativePath("b.jpg".into()),
                operation(),
            )
            .unwrap();
        assert!(row(&service, &scope).is_some());
        service.drain_results();
        let (handle, owner, _) = active_session();
        app.set_remote_session_handle(handle.clone());
        app.spread_shift_service = Some(service.clone());
        app.settings.remember_spread_shift_per_book = true;
        let before = handle.snapshot().active.unwrap().completed_count;
        let base = service.watermarks().admitted;
        let release = service.pause_for_test();
        let (pending, reply) = ClaimedRemoteWrite::for_test(
            handle.begin_operation(&owner, "set spread".into()).unwrap(),
            set_spread(mimageviewer_ipc::RemoteAddress::file(
                book.to_string_lossy().into_owned(),
            )),
        );
        app.apply_pending_remote_write(pending, &egui::Context::default());
        assert!(matches!(
            reply.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        assert_eq!(handle.snapshot().active.unwrap().completed_count, before);
        assert_eq!(service.watermarks().admitted, base + 1);
        assert_eq!(service.watermarks().completed_through, base);
        assert_eq!(app.remote_session_ui.pending_reading_writes.len(), 1);
        assert!(app.fs_feedback_toast.is_none());
        release.send(()).unwrap();
        assert_eq!(row(&service, &scope), None);
        assert!(matches!(
            reply.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let replies = service.drain_results();
            if !replies.is_empty() {
                assert_eq!(replies.len(), 1);
                assert!(app.complete_remote_shift_reset(&replies[0]));
                assert!(!app.complete_remote_shift_reset(&replies[0]));
                assert_eq!(
                    app.fs_feedback_toast.as_ref().unwrap().0,
                    "見開き調整を解除しました"
                );
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(matches!(
            reply.recv_timeout(Duration::from_secs(1)).unwrap(),
            UiWriteOutcome::Write(RemoteWriteResponse::Success(_))
        ));
        assert_eq!(
            handle.snapshot().active.unwrap().completed_count,
            before + 1
        );
        assert!(app.remote_session_ui.pending_reading_writes.is_empty());
        drop(process);
    }

    #[test]
    fn spread_shift_remote_memory_off_keeps_dormant_row_when_mode_changes() {
        let mut app = crate::app::setup_app_for_test();
        let data = tempfile::tempdir().unwrap();
        let book = data.path().join("book");
        std::fs::create_dir_all(&book).unwrap();
        let scope = BookScopeKey::folder(&book).unwrap();
        let saved = SpreadShiftPageRef::RelativePath("c.jpg".into());
        let (service, process) = SpreadShiftService::spawn_at(data.path().into(), None).unwrap();
        service
            .set_shift(scope.clone(), saved.clone(), operation())
            .unwrap();
        assert_eq!(row(&service, &scope), Some(saved.clone()));
        service.drain_results();
        let before = service.watermarks();
        let (handle, owner, _) = active_session();
        app.set_remote_session_handle(handle.clone());
        app.spread_shift_service = Some(service.clone());
        app.settings.remember_spread_shift_per_book = false;
        let (pending, reply) = ClaimedRemoteWrite::for_test(
            handle
                .begin_operation(&owner, "set spread while memory off".into())
                .unwrap(),
            set_spread(mimageviewer_ipc::RemoteAddress::file(
                book.to_string_lossy().into_owned(),
            )),
        );
        app.apply_pending_remote_write(pending, &egui::Context::default());
        assert!(matches!(
            reply.recv_timeout(Duration::from_secs(1)).unwrap(),
            UiWriteOutcome::Write(RemoteWriteResponse::Success(_))
        ));
        assert_eq!(row(&service, &scope), Some(saved));
        assert_eq!(service.watermarks(), before);
        assert!(app.remote_session_ui.pending_reading_writes.is_empty());
        drop(process);
    }

    #[test]
    fn spread_shift_remote_failed_base_setter_does_not_admit_clear() {
        let mut app = crate::app::setup_app_for_test();
        let data = tempfile::tempdir().unwrap();
        let book = data.path().join("book");
        std::fs::create_dir_all(&book).unwrap();
        let scope = BookScopeKey::folder(&book).unwrap();
        let saved = SpreadShiftPageRef::RelativePath("b.jpg".into());
        let (service, process) = SpreadShiftService::spawn_at(data.path().into(), None).unwrap();
        service
            .set_shift(scope.clone(), saved.clone(), operation())
            .unwrap();
        assert_eq!(row(&service, &scope), Some(saved.clone()));
        service.drain_results();
        let before = service.watermarks();
        let (handle, owner, _) = active_session();
        app.set_remote_session_handle(handle.clone());
        app.spread_shift_service = Some(service.clone());
        app.settings.remember_spread_shift_per_book = true;
        app.spread_db = Err("isolated base setter failure".into());
        let (pending, reply) = ClaimedRemoteWrite::for_test(
            handle
                .begin_operation(&owner, "failed set spread".into())
                .unwrap(),
            set_spread(mimageviewer_ipc::RemoteAddress::file(
                book.to_string_lossy().into_owned(),
            )),
        );
        app.apply_pending_remote_write(pending, &egui::Context::default());
        assert!(matches!(
            reply.recv_timeout(Duration::from_secs(1)).unwrap(),
            UiWriteOutcome::Write(RemoteWriteResponse::Error(RemoteWriteError {
                code: RemoteWriteErrorCode::PersistenceFailed,
                ..
            }))
        ));
        assert_eq!(service.watermarks(), before);
        assert_eq!(row(&service, &scope), Some(saved));
        assert!(app.remote_session_ui.pending_reading_writes.is_empty());
        drop(process);
    }

    #[test]
    fn spread_shift_remote_acquire_waits_for_shift_and_resume_then_snapshot_publication() {
        let mut app = crate::app::setup_app_for_test();
        let data = tempfile::tempdir().unwrap();
        let scope = BookScopeKey::folder(data.path().join("book")).unwrap();
        let (service, process) = SpreadShiftService::spawn_at(data.path().into(), None).unwrap();
        let resume =
            crate::book_resume_db::BookResumeWriter::spawn_at(data.path().join("resume.db"))
                .unwrap();
        let release_shift = service.pause_for_test();
        let release_resume = resume.pause_for_test();
        service
            .set_shift(
                scope.clone(),
                SpreadShiftPageRef::RelativePath("b.jpg".into()),
                operation(),
            )
            .unwrap();
        assert!(resume.record(&scope.logical_root, 2, None));
        let handle = SessionHandle::new();
        app.spread_shift_service = Some(service.clone());
        app.book_resume_writer = Some(resume);
        app.set_remote_session_handle(handle.clone());
        handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
            client_id: "spread-shift-acquire".into(),
            peer: mimageviewer_ipc::SessionPeerInfo {
                connection_kind: SessionConnectionKind::Direct,
                device_name: None,
            },
        });
        let ctx = egui::Context::default();
        app.poll_remote_session(&ctx);
        assert_eq!(
            handle.snapshot().phase,
            super::super::super::session::RemoteControlPhase::AcquiringRemote
        );
        release_shift.send(()).unwrap();
        assert!(row(&service, &scope).is_some());
        service.drain_results();
        service.drain_errors();
        app.poll_remote_session(&ctx);
        assert_eq!(
            handle.snapshot().phase,
            super::super::super::session::RemoteControlPhase::AcquiringRemote
        );
        release_resume.send(()).unwrap();
        let writer = app.book_resume_writer.as_ref().unwrap();
        writer
            .read_position_after(
                &crate::path_key::normalize(&scope.logical_root),
                writer.watermarks().admitted,
            )
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        app.poll_remote_session(&ctx);
        assert_eq!(
            handle.snapshot().phase,
            super::super::super::session::RemoteControlPhase::RemoteActive
        );
        assert!(handle.spread_shift_service().is_some());
        drop(process);
    }

    #[test]
    fn spread_shift_remote_resume_request_uses_accepted_fifo_read_before_later_sibling_record() {
        let mut app = crate::app::setup_app_for_test();
        let data = tempfile::tempdir().unwrap();
        let book = data.path().join("book");
        let writer =
            crate::book_resume_db::BookResumeWriter::spawn_at(data.path().join("resume.db"))
                .unwrap();
        let release = writer.pause_for_test();
        assert!(writer.record(&book, 2, None));
        app.book_resume_writer = Some(writer);
        let (handle, _, _) = active_session();
        app.set_remote_session_handle(handle.clone());
        let worker_handle = handle.clone();
        let worker_book = book.clone();
        let (response_tx, response_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            response_tx
                .send(worker_handle.read_book_resume(&worker_book))
                .unwrap();
        });
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.remote_session_ui.pending_resume_reads.is_empty() {
            app.apply_pending_remote_ui_requests(&handle, &ctx);
            assert!(
                Instant::now() < deadline,
                "Remote resume request was not claimed"
            );
            std::thread::yield_now();
        }
        assert_eq!(app.remote_session_ui.pending_resume_reads.len(), 1);
        assert_eq!(app.remote_session_ui.pending_resume_reads[0].cutoff, 1);
        app.poll_remote_resume_reads(&ctx);
        assert!(matches!(
            response_rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        let writer = app.book_resume_writer.as_ref().unwrap();
        // This later sibling Record must belong to the next entry, even when
        // its commit reaches the DB before the UI adopts the original read.
        assert!(writer.record(&book, 9, None));
        release.send(()).unwrap();
        let latest = writer
            .read_position_after(
                &crate::path_key::normalize(&book),
                writer.watermarks().admitted,
            )
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(latest.status.is_ok());
        assert_eq!(latest.position.unwrap().page, 9);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !app.remote_session_ui.pending_resume_reads.is_empty() {
            app.poll_remote_resume_reads(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(
            response_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Ok(Some(2))
        );
        worker.join().unwrap();
    }

    #[test]
    fn spread_shift_remote_pending_clear_repaints_each_tick_and_exit_terminals_claim_once() {
        let mut app = crate::app::setup_app_for_test();
        let data = tempfile::tempdir().unwrap();
        let book = data.path().join("book");
        std::fs::create_dir_all(&book).unwrap();
        let scope = BookScopeKey::folder(&book).unwrap();
        let (service, process) = SpreadShiftService::spawn_at(data.path().into(), None).unwrap();
        service
            .set_shift(
                scope.clone(),
                SpreadShiftPageRef::RelativePath("b.jpg".into()),
                operation(),
            )
            .unwrap();
        assert!(row(&service, &scope).is_some());
        service.drain_results();
        let (handle, owner, _) = active_session();
        app.spread_shift_service = Some(service.clone());
        app.set_remote_session_handle(handle.clone());
        app.settings.remember_spread_shift_per_book = true;
        let release = service.pause_for_test();
        let (pending, reply) = ClaimedRemoteWrite::for_test(
            handle
                .begin_operation(&owner, "clear pending until final exit".into())
                .unwrap(),
            set_spread(mimageviewer_ipc::RemoteAddress::file(
                book.to_string_lossy().into_owned(),
            )),
        );
        let ctx = egui::Context::default();
        app.apply_pending_remote_write(pending, &ctx);
        // Warmup frames consume egui's initial immediate repaints. Subsequent
        // idle ticks must still schedule the pending Clear continuation promptly.
        for frame in 0..6 {
            let mut raw = egui::RawInput::default();
            raw.time = Some(frame as f64 / 60.0);
            let mut output = ctx.run_pass(raw, |ctx| app.poll_remote_session(ctx));
            output.textures_delta.clear();
            assert!(
                output.viewport_output[&egui::ViewportId::ROOT].repaint_delay
                    <= Duration::from_millis(16)
            );
            assert!(matches!(
                reply.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ));
            assert_eq!(app.remote_session_ui.pending_reading_writes.len(), 1);
        }
        app.retire_remote_resources_for_final_exit();
        assert!(matches!(
            reply.recv_timeout(Duration::from_secs(1)).unwrap(),
            UiWriteOutcome::Write(RemoteWriteResponse::Error(RemoteWriteError {
                code: RemoteWriteErrorCode::UiTimeout,
                ..
            }))
        ));
        assert!(matches!(
            reply.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
        assert!(app.remote_session_ui.pending_reading_writes.is_empty());
        assert_eq!(
            handle.snapshot().phase,
            super::super::super::session::RemoteControlPhase::Local
        );
        // Retiring the reply owner does not roll back or cancel an admitted DB
        // operation. Its eventual completion has no live Remote claim to route.
        release.send(()).unwrap();
        assert_eq!(row(&service, &scope), None);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let replies = service.drain_results();
            if !replies.is_empty() {
                assert_eq!(replies.len(), 1);
                assert!(!app.complete_remote_shift_reset(&replies[0]));
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        drop(process);
    }
}
