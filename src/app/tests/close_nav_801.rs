use super::*;
use crate::perf::close_nav::{Outcome, Stage, TestCapture, TestEvent, WaitSite};

fn prepare_pdf_return(app: &mut App, root: &Path) {
    let book = root.join("book.pdf");
    std::fs::write(&book, b"fake PDF: no render or enumeration is requested").unwrap();
    app.current_folder = Some(book.clone());
    app.items = vec![GridItem::PdfPage {
        pdf_path: book,
        page_num: 0,
        content_type: None,
    }];
    app.fullscreen_idx = Some(0);
    app.pending_return_to_parent.request_live();
    app.settings.book_root = Some(root.join("books"));
    app.final_ai_pending.insert(
        FinalAiKey {
            edit_key: EditResultKey {
                item_id: crate::item_identity::ItemId::NONE,
                items_generation: app.items_generation,
                idx: 0,
                source_gen: 0,
                erase_mask_gen: 0,
                local_gen: 0,
                conceal_mask_gen: 0,
                conceal_gen: 0,
            },
            color_ai_hash: 0,
            bg: 0,
        },
        FinalAiPending {
            feedback_request: FinalAiFeedbackRequest::default(),
            job_id: 801,
            cancel: Arc::new(AtomicBool::new(false)),
            retained_key: None,
            retained_epoch: 0,
            allow_retained_orphan: false,
            cancel_on_drop: true,
        },
    );
}

fn close_to_parent(app: &mut App, root: &Path) {
    let nav = app
        .take_pending_return_to_parent_nav()
        .expect("accepted PDF return");
    assert!(matches!(&nav, crate::ui_main::AddressBarNav::Direct(path, _) if path == root));
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    assert_eq!(app.current_folder.as_deref(), Some(root));
    assert!(app.fullscreen_idx.is_none());
}

#[test]
fn close_nav_801_inference_sessions_held_does_not_block_pdf_return_handler() {
    let mut env = setup_app_for_test();
    let root = env.tmp.path().join("shelf");
    std::fs::create_dir(&root).unwrap();
    prepare_pdf_return(&mut env, &root);
    let runtime = Arc::new(crate::ai::runtime::AiRuntime::empty_for_test());
    env.ai_runtime_init = crate::ai::runtime::AiRuntimeInitOwner::ready_for_test(runtime.clone());
    let (held_tx, held_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let holder = std::thread::spawn(move || {
        let mut completed_while_held = false;
        runtime.hold_sessions_for_test(|| {
            held_tx.send(()).unwrap();
            completed_while_held = done_rx.recv_timeout(Duration::from_secs(10)).is_ok();
        });
        completed_while_held
    });
    held_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let capture = TestCapture::begin(None);
    close_to_parent(&mut env, &root);
    done_tx.send(()).ok();
    assert!(
        holder.join().unwrap(),
        "handler waited for the inference mutex"
    );
    let events = capture.events();
    assert!(!events.contains(&TestEvent::Attempt(WaitSite::AiSessions)));
    assert!(events.contains(&TestEvent::Stage(Stage::PathIsFile, Outcome::Complete)));
    assert!(events.contains(&TestEvent::Stage(Stage::ScanDirectory, Outcome::Complete)));
    assert!(events.contains(&TestEvent::Stage(Stage::DirectOpen, Outcome::Loaded)));
}

#[test]
fn close_nav_801_ai_queue_held_does_not_block_pdf_return_handler() {
    let mut env = setup_app_for_test();
    let root = env.tmp.path().join("shelf");
    std::fs::create_dir(&root).unwrap();
    prepare_pdf_return(&mut env, &root);
    let inner = Arc::new((
        Mutex::new(AiJobQueueState {
            display: Default::default(),
            prefetch: Default::default(),
            shutdown: false,
        }),
        Condvar::new(),
    ));
    env.ai_job_queue = Some(AiJobQueue {
        inner: inner.clone(),
    });
    let (held_tx, held_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _queue = inner.0.lock().unwrap();
        held_tx.send(()).unwrap();
        done_rx.recv_timeout(Duration::from_secs(10)).is_ok()
    });
    held_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    close_to_parent(&mut env, &root);
    done_tx.send(()).ok();
    assert!(
        holder.join().unwrap(),
        "handler waited for the AI queue mutex"
    );
}

#[test]
fn close_nav_801_remote_state_held_blocks_pdf_return_handler_at_named_lock() {
    let mut env = setup_app_for_test();
    let root = env.tmp.path().join("shelf");
    std::fs::create_dir(&root).unwrap();
    prepare_pdf_return(&mut env, &root);
    let handle = crate::remote_ipc::session::SessionHandle::new();
    env.set_remote_session_handle(handle.clone());
    let (held_tx, held_rx) = mpsc::channel();
    let (events_tx, events_rx) = mpsc::channel();
    // The worker releases even on a failed expectation: the handler can finish and
    // the assertion reports a failure instead of hanging the whole test runner.
    let holder = std::thread::spawn(move || {
        let mut reached = false;
        let mut acquired_before_release = false;
        handle.hold_snapshot_state_for_test(|| {
            held_tx.send(()).unwrap();
            while let Ok(event) = events_rx.recv_timeout(Duration::from_secs(10)) {
                if event == TestEvent::Attempt(WaitSite::RemoteSessionState) {
                    reached = true;
                    acquired_before_release =
                        events_rx.recv_timeout(Duration::from_millis(100)).is_ok();
                    break;
                }
            }
        });
        (reached, acquired_before_release)
    });
    held_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let capture = TestCapture::begin(Some(events_tx));
    close_to_parent(&mut env, &root);
    let (reached, acquired_before_release) = holder.join().unwrap();
    assert!(reached, "handler no longer reaches the remote state lock");
    assert!(
        !acquired_before_release,
        "handler acquired a mutex still held by another thread"
    );
    assert!(
        capture
            .events()
            .contains(&TestEvent::Acquired(WaitSite::RemoteSessionState))
    );
}

#[test]
fn close_nav_801_modal_refusal_and_unsupported_return_have_typed_reasons() {
    let mut env = setup_app_for_test();
    let capture = TestCapture::begin(None);
    assert!(!env.apply_fullscreen_close_nav_immediate(crate::ui_main::AddressBarNav::HistoryBack));
    // This existing modal has no worker/device prerequisite.
    env.epub_batch_convert = Some(
        crate::ui_dialogs::epub_batch_convert::EpubBatchPending::completed_for_test(
            PathBuf::new(),
            Err(crate::epub_convert::EpubConvertError::Cancelled),
        ),
    );
    let root = env.tmp.path().to_path_buf();
    assert!(
        env.apply_fullscreen_close_nav_immediate(crate::ui_main::AddressBarNav::Direct(
            root,
            StartupListIntent::ExplicitList
        ))
    );
    let events = capture.events();
    assert!(events.contains(&TestEvent::Stage(
        Stage::ApplyReturn,
        Outcome::UnsupportedNavigation
    )));
    assert!(events.contains(&TestEvent::Stage(
        Stage::OpenAdmission,
        Outcome::ModalBlocked
    )));
}
