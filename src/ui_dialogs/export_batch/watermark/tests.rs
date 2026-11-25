use super::*;

fn image_dialog(path: &str) -> ExportBatchDialogState {
    let mut state = super::super::tests::dialog(false);
    state.options.watermark.enabled = true;
    state.options.watermark.kind = WatermarkKind::Image;
    state.options.watermark.image_path = Some(path.into());
    state
}

fn pending(state: &mut ExportBatchDialogState) -> mpsc::Sender<bool> {
    let (tx, rx) = mpsc::channel();
    state.logo_check = Some(LogoCheck {
        path: state.options.watermark.image_path.clone(),
        result: LogoCheckResult::Pending(rx),
    });
    tx
}

#[test]
fn watermark_logo_check_pending_errors_and_disabled_watermark() {
    let mut state = image_dialog("missing.png");
    let ctx = egui::Context::default();
    let tx = pending(&mut state);
    assert!(state.can_start());
    tx.send(false).unwrap();
    state.poll_logo_check(&ctx);
    assert!(!state.can_start());
    assert!(
        state
            .logo_error()
            .unwrap()
            .contains("透かしの画像が見つかりません:")
    );
    state.options.watermark.enabled = false;
    assert!(state.can_start());
    state.options.watermark.enabled = true;
    state.options.watermark.image_path = Some("logo.svg".into());
    state.begin_logo_check(&ctx);
    assert!(matches!(
        state.logo_check.as_ref().unwrap().result,
        LogoCheckResult::Unsupported
    ));
    assert!(!state.can_start());
    assert_eq!(state.logo_error().unwrap(), "この形式は透かしに使えません");
    state.options.watermark.kind = WatermarkKind::Text;
    assert!(state.can_start());
}

#[test]
fn watermark_logo_check_reselection_discards_old_a_b_a_channels() {
    let ctx = egui::Context::default();
    let mut state = image_dialog("A.png");
    let old_a = pending(&mut state);
    state.options.watermark.image_path = Some("B.png".into());
    state.begin_logo_check(&ctx);
    assert!(old_a.send(false).is_err());
    let old_b = pending(&mut state);
    state.options.watermark.image_path = Some("A.png".into());
    state.begin_logo_check(&ctx);
    assert!(old_b.send(false).is_err());
    let current_a = pending(&mut state);
    current_a.send(true).unwrap();
    state.poll_logo_check(&ctx);
    assert!(matches!(
        state.logo_check.as_ref().unwrap().result,
        LogoCheckResult::Found
    ));
    assert!(state.can_start());
    // Reselecting the very same path creates a fresh receiver too.
    let previous = pending(&mut state);
    state.begin_logo_check(&ctx);
    assert!(previous.send(false).is_err());
    let latest = pending(&mut state);
    latest.send(true).unwrap();
    state.poll_logo_check(&ctx);
    assert!(state.can_start());
    let closed = pending(&mut state);
    drop(state);
    assert!(closed.send(false).is_err());
}

#[test]
fn watermark_logo_check_metadata_accepts_files_and_rejects_directories() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("logo.png");
    std::fs::write(&file, b"existence only, decoded by export worker").unwrap();
    let directory = tmp.path().join("directory.png");
    std::fs::create_dir(&directory).unwrap();
    for (path, expected) in [
        (&file, true),
        (&directory, false),
        (&tmp.path().join("missing.png"), false),
    ] {
        let mut state = image_dialog(&path.display().to_string());
        state.begin_logo_check(&egui::Context::default());
        let Some(LogoCheck {
            result: LogoCheckResult::Pending(rx),
            ..
        }) = state.logo_check
        else {
            panic!("metadata check must be asynchronous")
        };
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap(),
            expected
        );
    }
}
