//! Input admission happens only after the existing main-frame arbiter chooses a winner.

use super::drive_jump::{DriveJumpRequest, DriveJumpResolution, finish_drive_jump_for_test};
use super::*;
use crate::folder_pane::FolderPaneActivation;
use crate::keymap::KeyAction;
use crate::ui_main::AddressBarNav;

fn missing() -> DriveJumpResolution {
    DriveJumpResolution {
        target: None,
        warnings: Vec::new(),
        outcome: "missing",
    }
}

fn dispatch(app: &mut App, ctx: &egui::Context, nav: AddressBarNav) {
    let mut frame = update_phases::MainFrameNavigation::default();
    frame.keyboard_nav = Some(nav);
    app.post_render_main_navigation(ctx, frame);
}

#[test]
fn drive_jump_key_candidate_wins_over_same_frame_folder_pane_open() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let drive_target = app.tmp.path().join("drive-target");
    let pane_target = app.tmp.path().join("pane-target");
    std::fs::create_dir(&drive_target).unwrap();
    std::fs::create_dir(&pane_target).unwrap();
    let resolved = drive_target.clone();
    app.drive_jump_resolver = Some(Arc::new(move |_| DriveJumpResolution {
        target: Some(crate::folder_tree::OpenablePathResolution {
            path: resolved.clone(),
            kind: crate::folder_tree::OpenablePathKind::Directory,
            requested_is_file: false,
        }),
        warnings: Vec::new(),
        outcome: "target",
    }));
    let candidate = app.apply_location_navigation_key_action(
        &ctx,
        KeyAction::GridSwitchDriveD,
        "drive-priority-key",
    );
    assert!(matches!(candidate, Some(AddressBarNav::DriveJump(_))));
    assert!(
        app.drive_jump_pending.is_none(),
        "a candidate must not admit or start a worker"
    );
    let mut frame = update_phases::MainFrameNavigation::default();
    frame.keyboard_nav = candidate;
    frame.folder_pane_nav = Some(FolderPaneActivation::Open(pane_target));
    app.post_render_main_navigation(&ctx, frame);
    assert!(
        app.folder_pane_open_pending.is_none(),
        "the lower-priority pane cannot start a scan"
    );
    assert!(matches!(&app.drive_jump_pending.as_ref().unwrap().request,
        DriveJumpRequest::LastLocation { drive_key, .. } if drive_key == "D:"));
    finish_drive_jump_for_test(&mut app, &ctx);
    app.finish_direct_navigation_scan_for_test();
    assert_eq!(app.current_folder.as_ref(), Some(&drive_target));
}

#[test]
fn drive_jump_toolbar_candidate_wins_over_same_frame_folder_pane_open() {
    use egui_kittest::kittest::{NodeT, Queryable};
    use std::{cell::RefCell, rc::Rc};

    let app = Rc::new(RefCell::new(setup_app_for_test()));
    let target = app.borrow().tmp.path().join("toolbar-drive");
    let pane = app.borrow().tmp.path().join("pane");
    std::fs::create_dir(&target).unwrap();
    std::fs::create_dir(&pane).unwrap();
    let key = drive_current_key_for_path(&target).unwrap();
    app.borrow_mut().settings.show_toolbar_drives = true;
    app.borrow_mut()
        .set_quick_folder_slot_target(QuickFolderSlotId::A, target.clone());
    let render = app.clone();
    let candidates = Rc::new(RefCell::new(Vec::new()));
    let output = candidates.clone();
    let mut harness = crate::test_ui::builder()
        .with_size(egui::vec2(1600.0, 500.0))
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .build_ui(move |ui| {
            crate::test_ui::full_viewport_ui(ui, |root| {
                if let Some(candidate) = render.borrow_mut().render_toolbar(root).1 {
                    output.borrow_mut().push(candidate);
                }
            })
        });
    harness.run();
    assert!(candidates.borrow().is_empty());
    harness.get_by_label(&key).click();
    harness.run();
    assert!(!app.borrow().drive_jump_pending());
    let candidate = candidates.borrow_mut().pop().expect("toolbar drive input");
    assert!(candidates.borrow().is_empty());
    assert!(matches!(candidate, AddressBarNav::DriveJump(_)));
    let mut frame = update_phases::MainFrameNavigation::default();
    frame.address_nav = Some(candidate); // Production render_toolbar's second return value.
    frame.folder_pane_nav = Some(FolderPaneActivation::Open(pane));
    app.borrow_mut()
        .post_render_main_navigation(&harness.ctx, frame);
    assert!(app.borrow().drive_jump_pending());
    assert!(app.borrow().folder_pane_open_pending.is_none());
    finish_drive_jump_for_test(&mut app.borrow_mut(), &harness.ctx);
    app.borrow_mut().finish_direct_navigation_scan_for_test();
    assert_eq!(app.borrow().current_folder.as_ref(), Some(&target));
}

#[test]
fn drive_jump_losing_input_does_not_admit_or_retire_the_existing_owner() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let (started_tx, started) = mpsc::channel();
    app.drive_jump_resolver = Some(Arc::new(move |request| {
        started_tx.send(request.clone()).unwrap();
        missing()
    }));
    let first = app
        .apply_switch_drive_letter(&ctx, 'D', "drive-first")
        .unwrap();
    dispatch(&mut app, &ctx, first);
    let first_request = started
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    let cancel = app.drive_jump_pending.as_ref().unwrap().cancel.clone();
    let second = app
        .apply_switch_drive_letter(&ctx, 'E', "drive-losing")
        .unwrap();
    assert_eq!(
        app.drive_jump_pending.as_ref().unwrap().request,
        first_request
    );
    assert!(
        !cancel.load(Ordering::Relaxed),
        "preparing an input cannot replace an owner"
    );
    let favorite = app.tmp.path().join("winning-favorite");
    std::fs::create_dir(&favorite).unwrap();
    let mut frame = update_phases::MainFrameNavigation::default();
    frame.toolbar_fav_nav = Some(favorite.clone());
    frame.keyboard_nav = Some(second);
    app.post_render_main_navigation(&ctx, frame);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.drive_jump_pending.is_none());
    assert!(
        started.try_recv().is_err(),
        "the losing drive input must not start a worker"
    );
    app.finish_direct_navigation_scan_for_test();
    assert_eq!(app.current_folder.as_ref(), Some(&favorite));
}
