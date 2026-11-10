//! Independent archive-book regressions. Real workers and disposable profiles;
//! egui input is headless and no native viewport or product binary is launched.
#![cfg(windows)]

use super::tests::phase_c_support::{AppTestEnv, setup_app};
use super::*;
use crate::book_resume_db::{PageIdentity, ReadingMeterValue};
use crate::book_resume_key::BookResumeKey;
use crate::book_scope::BookScopeKey;
use std::io::Write;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

fn png() -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(8, 12)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn zip_bytes(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn write_pages(path: &Path, names: &[&str]) {
    let entries = names
        .iter()
        .map(|name| {
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::RgbImage::new(8, 12)
                .write_to(
                    &mut bytes,
                    if name.ends_with(".jpg") {
                        image::ImageFormat::Jpeg
                    } else {
                        image::ImageFormat::Png
                    },
                )
                .unwrap();
            (*name, bytes.into_inner())
        })
        .collect::<Vec<_>>();
    std::fs::write(path, zip_bytes(&entries)).unwrap();
}

fn tick(app: &mut App, ctx: &egui::Context) {
    app.poll_open_path_classification(ctx);
    app.poll_collection_history_transition(ctx);
    app.poll_zip_enumerate();
    app.poll_sidecar_restore(ctx);
    app.poll_book_composition_service_for_test(ctx);
    app.poll_fs_nav_lock(ctx);
}

fn poll_conversion(app: &mut App, ctx: &egui::Context) {
    if app.archive_convert.is_some() {
        ctx.run_pass(Default::default(), |ctx| {
            app.show_archive_convert_dialog(ctx)
        })
        .drop_without_applying_deltas();
    }
}

fn settled(app: &App) -> bool {
    app.zip_enumerate_pending.is_none()
        && app.top_level_grid_view.open_path_classification().is_none()
        && app
            .top_level_grid_view
            .history_navigation_transition()
            .is_none()
        && !app.sidecar_restore_active()
        && !app.book_composition.is_pending()
        && !app.book_composition_permission.pending()
        && !app.fs_nav_deferred_reopen_wait_active()
        && app.archive_convert.is_none()
}

fn settle(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + WAIT;
    loop {
        if app.projected_viewer_context_id() == app.viewer_context_main() {
            poll_conversion(app, ctx);
        }
        tick(app, ctx);
        if settled(app) {
            return;
        }
        assert!(Instant::now() < deadline, "archive book did not settle");
        std::thread::yield_now();
    }
}

fn settle_detached(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + WAIT;
    loop {
        // Conversion is App-global, while its initial shell and subsequent
        // enumeration/hydration belong to the detached bundle. Dispatch the
        // modal completion from main before mounting the reader, as production does.
        poll_conversion(app, ctx);
        let ready = app
            .with_active_viewer_context(|viewer| {
                tick(viewer, ctx);
                settled(viewer)
            })
            .expect("independent archive context");
        if ready && app.archive_convert.is_none() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "independent archive producer did not settle"
        );
        std::thread::yield_now();
    }
}

fn open_list(app: &mut App, ctx: &egui::Context, path: &Path) {
    app.indexer_init = IndexerInit::Unavailable;
    app.settings.detached_viewer_open_images_in_window = false;
    assert!(matches!(
        app.load_folder_or_convert_archive_with_auto_fullscreen(path.to_owned(), false),
        FolderOpenOutcome::Loaded | FolderOpenOutcome::Classifying
    ));
    settle(app, ctx);
    assert!(app.fullscreen_idx.is_none());
    assert!(app.zip_nav.as_ref().unwrap().at_root());
    app.settings.detached_viewer_open_images_in_window = true;
    app.settings.auto_fullscreen_zip_pdf = true;
    ctx.set_embed_viewports(false);
}

fn child(app: &App, prefix: &str) -> usize {
    app.items
        .iter()
        .position(
            |item| matches!(item, GridItem::ZipDir { dir_prefix, .. } if dir_prefix == prefix),
        )
        .unwrap_or_else(|| panic!("missing {prefix}: {:?}", app.items))
}

fn scope(source: &Path, raw_prefix: &str) -> BookScopeKey {
    BookScopeKey::archive(
        source,
        &raw_prefix
            .trim_end_matches('/')
            .split('/')
            .map(str::to_owned)
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

fn target(source: &Path, literal: &str, raw: &str) -> ArchiveBookTarget {
    ArchiveBookTarget {
        requested_prefix: literal.into(),
        scope: scope(source, raw),
        entry: ArchiveBookEntry::Activation,
    }
}

fn descriptor(
    backing: &Path,
    logical: Option<&Path>,
    literal: &str,
    raw: &str,
) -> ViewerContextDescriptor {
    ViewerContextDescriptor::Zip {
        path: backing.to_owned(),
        archive_source_override: logical.map(Path::to_owned),
        target: DetachedZipTarget::BookScope(target(logical.unwrap_or(backing), literal, raw)),
    }
}

fn entry(app: &App) -> &str {
    match &app.items[app.fullscreen_idx.expect("book reader is open")] {
        GridItem::ZipImage { entry_name, .. } => entry_name,
        other => panic!("expected archive page, got {other:?}"),
    }
}

#[derive(Debug, PartialEq)]
struct MainState {
    items: Vec<GridItem>,
    generation: u64,
    prefix: Vec<String>,
    raw: Vec<String>,
    selected: Option<usize>,
    scroll: f32,
    folder: Option<PathBuf>,
    address: String,
    facet: String,
    facet_route: String,
    ratings: [bool; 6],
    suppression: String,
    file_types: String,
    visible: Vec<usize>,
    has_chosen_return: bool,
}

fn main_state(app: &App) -> MainState {
    MainState {
        items: app.items.clone(),
        generation: app.items_generation,
        prefix: app.zip_nav.as_ref().unwrap().current().to_vec(),
        raw: app.zip_nav.as_ref().unwrap().current_raw().to_vec(),
        selected: app.selected,
        scroll: app.grid_scroll.offset(),
        folder: app.current_folder.clone(),
        address: app.address.clone(),
        facet: format!("{:?}", app.settings.facet_filter),
        facet_route: format!("{:?}", app.facet_navigation),
        ratings: app.settings.rating_filter,
        suppression: format!("{:?}", app.rating_filter_suppressed_at),
        file_types: format!("{:?}", app.settings.file_type_visibility),
        visible: app.visible_indices.clone(),
        has_chosen_return: app
            .zip_nav
            .as_ref()
            .unwrap()
            .chosen_list_return_plan()
            .is_some(),
    }
}

fn assert_reader(app: &mut App, ctx: &egui::Context, source: &Path, raw: &str, expected: &str) {
    settle_detached(app, ctx);
    app.with_active_viewer_context(|viewer| {
        assert_ne!(
            viewer.projected_viewer_context_id(),
            viewer.viewer_context_main()
        );
        assert_eq!(
            viewer.viewer_presentation,
            ViewerPresentation::DetachedWindow
        );
        assert!(viewer.detached_viewer_independent_active);
        assert_eq!(entry(viewer), expected);
        assert_eq!(
            viewer.zip_nav.as_ref().unwrap().current_raw().join("/"),
            raw
        );
        assert_eq!(
            viewer.current_book_resume_key(),
            Some(
                BookResumeKey::archive(
                    source,
                    &raw.split('/').map(str::to_owned).collect::<Vec<_>>()
                )
                .unwrap()
            )
        );
        assert!(
            viewer
                .zip_nav
                .as_ref()
                .unwrap()
                .chosen_list_return_plan()
                .is_none()
        );
        assert!(viewer.take_pending_return_to_parent_nav().is_none());
    })
    .unwrap();
}

fn activate(app: &mut App, ctx: &egui::Context, prefix: &str) {
    let index = child(app, prefix);
    assert_eq!(
        app.activate_main_zipdir(index, ctx),
        ZipDirActivationOutcome::BookRequested
    );
}

fn send_enter(app: &mut App, ctx: &egui::Context) {
    ctx.begin_pass(egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    });
    assert!(app.handle_keyboard(ctx).is_none());
    ctx.end_pass().drop_without_applying_deltas();
}

fn double_click(mut app: AppTestEnv, check: impl FnOnce(&mut App, &egui::Context, MainState)) {
    struct State {
        app: AppTestEnv,
        origin: egui::Pos2,
    }
    // Layout before capturing the invariant: ordinary grid layout owns its scroll clamp.
    app.grid_scroll.set_position(0.0);
    let mut configured = false;
    let mut harness = crate::test_ui::builder()
        .with_size(egui::vec2(1000.0, 600.0))
        .with_step_dt(0.01)
        .build_ui_state(
            move |ui, state: &mut State| {
                let ctx = ui.ctx().clone();
                if !configured {
                    crate::ui_fonts::configure_fonts(&ctx);
                    ctx.set_embed_viewports(false);
                    configured = true;
                    ctx.request_repaint();
                    return;
                }
                state.origin = ui.available_rect_before_wrap().min;
                state.app.begin_grid_click_input_frame(&ctx);
                assert!(state.app.render_grid(ui).is_none());
            },
            State {
                app,
                origin: egui::Pos2::ZERO,
            },
        );
    harness.run_steps(6);
    let before = main_state(&harness.state().app);
    let pos = harness.state().origin + egui::vec2(25.0, 25.0);
    harness.hover_at(pos);
    for _ in 0..2 {
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        harness.step();
    }
    // Workers use the context supplied at dispatch; no native viewport is rendered.
    let ctx = egui::Context::default();
    check(&mut harness.state_mut().app, &ctx, before);
}

#[test]
fn section1675_multiwindow_zipdir_book_all_inputs() {
    for nested in [false, true] {
        for converted in [false, true] {
            for input in ["enter", "pad", "double_click"] {
                let mut app = setup_app();
                let ctx = egui::Context::default();
                let backing = app.tmp.path().join("outer.zip");
                let source = if converted {
                    app.tmp.path().join("logical.7z")
                } else {
                    backing.clone()
                };
                let prefix = if nested { "A.zip/" } else { "A/" };
                let raw = prefix.trim_end_matches('/');
                let expected = format!("{prefix}001.png");
                if nested {
                    let inner = zip_bytes(&[("001.png", png()), ("002.png", png())]);
                    std::fs::write(
                        &backing,
                        zip_bytes(&[("A.zip", inner.clone()), ("B.zip", inner)]),
                    )
                    .unwrap();
                } else {
                    write_pages(&backing, &["A/001.png", "A/002.png", "B/001.png"]);
                }
                if converted {
                    // A real validated cache hit installs the logical source and
                    // proven backing before the internal cell is dispatched.
                    std::fs::write(&source, b"disposable converted source").unwrap();
                    let stamp = std::fs::metadata(&source).unwrap();
                    app.archive_cache_db
                        .as_ref()
                        .unwrap()
                        .record(
                            &source,
                            crate::ui_helpers::mtime_secs(&stamp),
                            stamp.len() as i64,
                            crate::archive_converter::ArchiveFormat::SevenZ,
                            &backing,
                            std::fs::metadata(&backing).unwrap().len() as i64,
                            3,
                            false,
                        )
                        .unwrap();
                }
                open_list(&mut app, &ctx, &source);
                assert_eq!(
                    app.archive_source_override.as_deref(),
                    converted.then_some(source.as_path())
                );
                app.selected = Some(child(&app, prefix));
                let check = |app: &mut App, ctx: &egui::Context, before: MainState| {
                    assert!(
                        app.active_viewer_context_id().is_some(),
                        "{input}/{nested}/{converted}"
                    );
                    if converted {
                        let conversion = app.archive_convert.as_ref().expect(
                            "converted BookScope dispatch must create the real archive owner",
                        );
                        let owner = conversion
                            .completion
                            .detached_grid_archive_owner()
                            .expect("typed detached conversion completion");
                        assert_eq!(owner.source_path, source);
                        assert_eq!(
                            owner.target,
                            DetachedZipTarget::BookScope(target(&source, prefix, raw))
                        );
                        assert_eq!(app.active_detached_window_id(), Some(owner.lease.window_id));
                        assert_eq!(
                            app.active_detached_session.unwrap().content_phase,
                            DetachedSessionContentPhase::Preparing
                        );
                    }
                    assert_reader(app, ctx, &source, raw, &expected);
                    assert_eq!(main_state(app), before, "{input}/{nested}/{converted}");
                    assert!(app.fullscreen_idx.is_none());
                    let window = app.active_detached_window_id().unwrap();
                    assert!(app.locate_window_context(window).is_some());
                };
                if input == "double_click" {
                    double_click(app, check);
                } else {
                    app.grid_scroll.set_position(137.0);
                    let before = main_state(&app);
                    if input == "enter" {
                        send_enter(&mut app, &ctx);
                    } else {
                        assert!(app.handle_gamepad_grid_accept(&ctx).is_none());
                    }
                    check(&mut app, &ctx, before);
                }
            }
        }
    }
}

#[test]
fn section1675_multiwindow_zipdir_branch_stays_main() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("branch.zip");
    write_pages(&book, &["A/first/001.png", "A/second/001.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    let idx = child(&app, "A/");
    assert_eq!(
        app.activate_main_zipdir(idx, &ctx),
        ZipDirActivationOutcome::List
    );
    settle(&mut app, &ctx);
    assert!(app.active_viewer_context_id().is_none());
    assert!(app.active_detached_session.is_none());
    assert!(app.detached_image_windows.is_empty());
    assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["A"]);
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .chosen_list_return_plan()
            .is_some()
    );
    assert!(
        app.items
            .iter()
            .all(|item| matches!(item, GridItem::ZipDir { .. }))
    );
}

#[test]
fn section1675_multiwindow_unsupported_scope_preserves_chosen_list() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("unsupported-scope.zip");
    write_pages(
        &book,
        &["part:one/001.png", "part:one/002.png", "B/001.png"],
    );
    open_list(&mut app, &ctx, &book);
    assert!(app.settings.detached_viewer_open_images_in_window);
    assert!(app.settings.effective_auto_fullscreen_zip_pdf());
    assert!(app.zip_nav.as_ref().unwrap().at_root());
    let feedback_start = app.feedback_toasts_for_test.len();
    let request_seq = app.detached_grid_archive_open_request_seq;
    let index = child(&app, "part:one/");
    app.selected = Some(index);
    assert_eq!(
        app.activate_main_zipdir(index, &ctx),
        ZipDirActivationOutcome::List,
    );
    settle(&mut app, &ctx);
    assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["part:one"]);
    assert_eq!(app.zip_nav.as_ref().unwrap().current_raw(), &["part:one"]);
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .chosen_list_return_plan()
            .is_some()
    );
    assert_eq!(app.items.len(), 2);
    assert!(app.items.iter().all(|item| matches!(item, GridItem::ZipImage { entry_name, .. } if entry_name.starts_with("part:one/"))));
    assert!(app.fullscreen_idx.is_none());
    assert!(app.active_viewer_context_id().is_none());
    assert!(app.active_detached_session.is_none());
    assert!(app.detached_image_windows.is_empty());
    assert!(app.archive_convert.is_none());
    assert_eq!(app.detached_grid_archive_open_request_seq, request_seq);
    // Later composition polls must reuse the recorded Unsupported outcome.
    settle(&mut app, &ctx);
    let feedback = &app.feedback_toasts_for_test[feedback_start..];
    assert_eq!(feedback.len(), 1);
    assert_eq!(
        feedback[0].0,
        "この内部フォルダは本として開けないため、一覧を表示します"
    );
    assert!(matches!(feedback[0].1, crate::feedback::FeedbackKind::Info));
}

fn seed_resume(app: &mut App, source: &Path, raw: &str, name: &str) {
    let key = BookResumeKey::archive(
        source,
        &raw.split('/').map(str::to_owned).collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(app.persist_book_resume_key_with_identity(
        key.clone(),
        1,
        ReadingMeterValue::new(2, 3),
        PageIdentity::for_item(
            &GridItem::ZipImage {
                zip_path: source.to_owned(),
                entry_name: name.into()
            },
            Some(source)
        ),
    ));
    let writer = app.book_resume_writer.as_ref().unwrap();
    let cutoff = writer.watermarks().admitted;
    let reply = writer
        .read_position_after(&key.storage_key(), cutoff)
        .recv_timeout(WAIT)
        .unwrap();
    reply.status.unwrap();
    assert!(reply.completed_through >= cutoff);
    assert!(reply.position.is_some());
}

#[test]
fn section1675_multiwindow_scope_before_first_install() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("root-and-inner.zip");
    let composition_data = app.tmp.path().join("composition");
    let composition_owner = install_composition(&mut app, composition_data);
    write_pages(
        &book,
        &["000-root.png", "A/001.png", "A/002.png", "B/001.png"],
    );
    open_list(&mut app, &ctx, &book);
    seed_resume(&mut app, &book, "A", "A/002.png");
    assert!(app.persist_book_resume_key_with_identity(
        BookResumeKey::legacy(&book),
        0,
        ReadingMeterValue::new(1, 1),
        PageIdentity::for_item(
            &GridItem::ZipImage {
                zip_path: book.clone(),
                entry_name: "000-root.png".into()
            },
            Some(&book)
        ),
    ));
    app.settings.book_open_resume = crate::settings::ResumeMode::Resume;
    let before = main_state(&app);
    activate(&mut app, &ctx, "A/");
    app.with_active_viewer_context(|viewer| {
        assert!(
            viewer.items.is_empty(),
            "async enumeration has not installed root pages"
        );
        assert!(viewer.zip_nav.is_none());
        assert!(viewer.fullscreen_idx.is_none());
        assert!(
            !viewer.pending_auto_fs_open,
            "the typed request is the sole open owner"
        );
        assert!(viewer.fs_nav_after_pdf_enumerate.is_none());
        assert!(matches!(
            viewer
                .zip_enumerate_pending
                .as_ref()
                .map(|p| &p.presentation),
            Some(ZipEnumerationPresentation::ArchiveBook(_))
        ));
    })
    .unwrap();
    assert_reader(&mut app, &ctx, &book, "A", "A/002.png");
    app.with_active_viewer_context(|viewer| {
        assert!(viewer.items.iter().all(|item| matches!(item, GridItem::ZipImage { entry_name, .. } if entry_name.starts_with("A/"))));
        assert_eq!(viewer.book_composition.scope(), Some(&scope(&book, "A")));
    }).unwrap();
    assert_eq!(main_state(&app), before);
    composition_owner.shutdown_and_join();
}

#[test]
fn section1675_multiwindow_scope_literal_prefix() {
    for prefix in ["UpperCase", "upperCase"] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("case.zip");
        write_pages(
            &book,
            &[
                "UpperCase/001.png",
                "UpperCase/002.png",
                "upperCase/001.png",
                "upperCase/002.png",
            ],
        );
        open_list(&mut app, &ctx, &book);
        seed_resume(&mut app, &book, prefix, &format!("{prefix}/002.png"));
        app.settings.book_open_resume = crate::settings::ResumeMode::Resume;
        assert!(app.start_active_detached_book_context(
            descriptor(&book, None, &format!("{prefix}/"), prefix),
            &ctx,
            None,
            None
        ));
        assert_reader(&mut app, &ctx, &book, prefix, &format!("{prefix}/002.png"));
    }
}

#[test]
fn section1675_multiwindow_scope_resume_setting() {
    for mode in [
        crate::settings::ResumeMode::FromStart,
        crate::settings::ResumeMode::Resume,
    ] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("resume-setting.zip");
        write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
        open_list(&mut app, &ctx, &book);
        seed_resume(&mut app, &book, "A", "A/002.png");
        app.settings.book_open_resume = mode;
        // book_nav_resume belongs to DFS and must not override a direct choice.
        app.settings.book_nav_resume = if mode.resumes() {
            crate::settings::ResumeMode::FromStart
        } else {
            crate::settings::ResumeMode::Resume
        };
        activate(&mut app, &ctx, "A/");
        assert_reader(
            &mut app,
            &ctx,
            &book,
            "A",
            if mode.resumes() {
                "A/002.png"
            } else {
                "A/001.png"
            },
        );
    }
}

#[test]
fn section1675_multiwindow_scope_collapse_and_raw() {
    for excluded_cover in [false, true] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("wrapper.zip");
        write_pages(
            &book,
            &[
                "A/cover.png",
                "A/one/two/001.jpg",
                "A/one/two/002.jpg",
                "B/001.jpg",
            ],
        );
        if excluded_cover {
            app.settings
                .file_type_visibility
                .excluded_extensions
                .insert("png".into());
        }
        open_list(&mut app, &ctx, &book);
        // Raw A still has a cover; policy can collapse only the display level to one/two.
        seed_resume(
            &mut app,
            &book,
            "A",
            if excluded_cover {
                "A/one/two/002.jpg"
            } else {
                "A/cover.png"
            },
        );
        app.settings.book_open_resume = crate::settings::ResumeMode::Resume;
        activate(&mut app, &ctx, "A/");
        let expected = if excluded_cover {
            "A/one/two/002.jpg"
        } else {
            "A/cover.png"
        };
        assert_reader(&mut app, &ctx, &book, "A", expected);
        app.with_active_viewer_context(|viewer| {
            assert_eq!(
                viewer.zip_nav.as_ref().unwrap().current().join("/"),
                if excluded_cover { "A/one/two" } else { "A" }
            );
        })
        .unwrap();
    }
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("deep-wrapper.zip");
    write_pages(
        &book,
        &["A/one/two/001.png", "A/one/two/002.png", "B/001.png"],
    );
    open_list(&mut app, &ctx, &book);
    seed_resume(&mut app, &book, "A/one/two", "A/one/two/002.png");
    app.settings.book_open_resume = crate::settings::ResumeMode::Resume;
    activate(&mut app, &ctx, "A/");
    assert_reader(&mut app, &ctx, &book, "A/one/two", "A/one/two/002.png");
}

#[test]
fn section1675_multiwindow_scope_no_main_return() {
    for close in ["escape", "window_close"] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("close.zip");
        write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
        open_list(&mut app, &ctx, &book);
        app.selected = Some(child(&app, "A/"));
        app.grid_scroll.set_position(153.0);
        let before = main_state(&app);
        activate(&mut app, &ctx, "A/");
        assert_reader(&mut app, &ctx, &book, "A", "A/001.png");
        app.with_active_viewer_context(|viewer| {
            viewer.mark_active_detached_viewport_rendered(&ctx);
            assert_eq!(
                viewer.active_detached_session.unwrap().content_phase,
                DetachedSessionContentPhase::Ready
            );
            assert_eq!(
                viewer.active_detached_window_state(),
                Some(DetachedWindowState::Active)
            );
            // Returning through any ancestor would expose a leaked Chosen level.
            let mut nav = viewer.zip_nav.as_ref().unwrap().clone();
            loop {
                assert!(nav.chosen_list_return_plan().is_none());
                if !nav.back() {
                    break;
                }
            }
            if close == "escape" {
                viewer.handle_fullscreen_close_request_immediate();
            }
        })
        .unwrap();
        if close == "window_close" {
            assert!(app.close_current_active_viewer_context(&ctx));
        }
        assert_eq!(main_state(&app), before, "{close}");
        assert!(app.fullscreen_idx.is_none());
        assert!(app.take_pending_return_to_parent_nav().is_none());
    }
}

fn request(target: ArchiveBookTarget, resume: bool) -> DeferredFsReopen {
    DeferredFsReopen {
        history_trigger: HistoryTrigger::UserChosen,
        resume_slideshow: false,
        target: DeferredFsTarget::CurrentArchiveBook(target),
        resume_to_last_page: resume.into(),
        from_explicit_open: true,
        preserve_after_password_prompt: false,
    }
}

fn prepared(app: &App, path: &Path) -> crate::zip_loader::PreparedZipEnumeration {
    crate::zip_loader::PreparedZipEnumeration::prepare(
        path.to_owned(),
        crate::zip_loader::enumerate_image_entries_detailed(path).unwrap(),
        app.settings.file_type_visibility.clone(),
    )
}

fn install_composition(
    app: &mut App,
    data: PathBuf,
) -> crate::book_composition::BookCompositionProcessOwner {
    let (service, owner) =
        crate::book_composition::BookCompositionService::spawn_at(data, None).unwrap();
    app.install_book_composition_runtime(service);
    owner
}

#[test]
fn section1675_multiwindow_scope_literal_prefix_prepared() {
    for converted in [false, true] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let backing = app.tmp.path().join("prepared.zip");
        let logical = app.tmp.path().join("logical.rar");
        let source = if converted { &logical } else { &backing };
        write_pages(
            &backing,
            &[
                "000-root.png",
                "Upper/One/001.png",
                "Upper/One/002.png",
                "lower/001.png",
            ],
        );
        open_list(&mut app, &ctx, &backing);
        seed_resume(&mut app, source, "Upper/One", "Upper/One/002.png");
        let enumeration = prepared(&app, &backing);
        let open = request(target(source, "Upper/", "Upper/One"), true);
        let before = main_state(&app);
        assert!(app.start_active_detached_loading_context(
            &ctx, None, DetachedSource::Book, source.clone(), |viewer, _| {
                viewer.load_zip_as_folder_with_presentation(
                    backing.clone(), viewer.input_seq, Some(enumeration),
                    ZipCacheRouting::ProvenBacking, converted.then_some(source.as_path()),
                    ZipEnumerationPresentation::ArchiveBook(open),
                );
                assert!(viewer.zip_enumerate_pending.is_none(), "prepared enumeration installs synchronously");
                assert_eq!(viewer.zip_nav.as_ref().unwrap().current_raw(), &["Upper", "One"]);
                assert!(viewer.items.iter().all(|item| matches!(item, GridItem::ZipImage { entry_name, .. } if entry_name.starts_with("Upper/One/"))));
                assert_eq!(viewer.archive_source_override.as_deref(), converted.then_some(source.as_path()));
                assert!(!viewer.pending_auto_fs_open);
                DetachedLoadingContextInit::Continue
            },
        ).is_some());
        assert_reader(&mut app, &ctx, source, "Upper/One", "Upper/One/002.png");
        assert_eq!(main_state(&app), before);
    }
}

#[test]
fn section1675_multiwindow_scope_missing_changed_hidden() {
    use crate::book_composition::{
        BookComposition, BookCompositionService, CompositionPageRef, PageDomain, ReadingDirection,
    };
    for failure in ["missing", "changed", "hidden"] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("changed.zip");
        write_pages(&book, &["000-root.png", "A/Old/001.png", "B/001.png"]);
        open_list(&mut app, &ctx, &book);
        let before = main_state(&app);
        // Capture the request exactly as the main list saw it, then change the file
        // before enumeration. The main list deliberately remains the old snapshot.
        let open = descriptor(&book, None, "A/", "A/Old");
        let mut composition_owner = None;
        match failure {
            "missing" => write_pages(&book, &["000-root.png", "B/001.png"]),
            "changed" => write_pages(&book, &["000-root.png", "A/New/001.png", "B/001.png"]),
            "hidden" => {
                let (service, owner) =
                    BookCompositionService::spawn_at(app.tmp.path().join("composition"), None)
                        .unwrap();
                let page = CompositionPageRef::ArchiveEntry {
                    entry_name: "A/Old/001.png".into(),
                };
                let mut record = BookComposition::new(ReadingDirection::Ltr, vec![page.clone()]);
                record.hidden_pages.push(page);
                service
                    .save(
                        scope(&book, "A/Old"),
                        PageDomain::ArchiveLevel {
                            prefix: "A/Old".into(),
                        },
                        record,
                    )
                    .recv_timeout(WAIT)
                    .unwrap()
                    .status
                    .unwrap();
                app.install_book_composition_runtime(service);
                composition_owner = Some(owner);
            }
            _ => unreachable!(),
        }
        assert!(app.start_active_detached_book_context(open, &ctx, None, None));
        let window = app.active_detached_window_id().unwrap();
        let deadline = Instant::now() + WAIT;
        loop {
            app.with_active_viewer_context(|viewer| tick(viewer, &ctx))
                .unwrap();
            if app.detached_window_state(window) == Some(DetachedWindowState::Closing) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{failure} must end the initial open"
            );
            std::thread::yield_now();
        }
        app.with_active_viewer_context(|viewer| {
            assert!(
                viewer.fullscreen_idx.is_none(),
                "{failure}: no root or child fallback"
            );
            assert!(!viewer.pending_auto_fs_open);
            assert!(viewer.fs_nav_after_pdf_enumerate.is_none());
            assert!(!viewer.fs_nav_deferred_reopen_wait_active());
            assert!(
                viewer.fs_feedback_toast.is_some(),
                "{failure}: explain terminal refusal"
            );
            assert!(viewer.take_pending_return_to_parent_nav().is_none());
        })
        .unwrap();
        assert_eq!(main_state(&app), before, "{failure}");
        if let Some(owner) = composition_owner {
            owner.shutdown_and_join();
        }
    }
}

#[test]
fn section1675_multiwindow_scope_resolver_typed_failures() {
    let app = setup_app();
    let book = app.tmp.path().join("resolver.zip");
    write_pages(&book, &["root.png", "A/New/001.png", "B/001.png"]);
    let tree = prepared(&app, &book).tree;
    assert!(matches!(
        App::resolve_archive_book_nav(
            Arc::clone(&tree),
            &book,
            &target(&book, "Missing/", "Missing")
        ),
        Err(ArchiveBookOpenOutcome::ScopeMissing)
    ));
    assert!(matches!(
        App::resolve_archive_book_nav(Arc::clone(&tree), &book, &target(&book, "A/", "A/Old")),
        Err(ArchiveBookOpenOutcome::ScopeChanged)
    ));
    let other_source = app.tmp.path().join("other.zip");
    assert!(matches!(
        App::resolve_archive_book_nav(
            Arc::clone(&tree),
            &book,
            &target(&other_source, "A/", "A/New")
        ),
        Err(ArchiveBookOpenOutcome::ScopeChanged)
    ));
    let resolved =
        App::resolve_archive_book_nav(tree, &book, &target(&book, "A/", "A/New")).unwrap();
    assert_eq!(resolved.current_raw(), &["A", "New"]);
    assert!(resolved.chosen_list_return_plan().is_none());
}

#[test]
fn section1675_multiwindow_same_archive_sibling_unchanged() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("siblings.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "B/001.png", "B/002.png"]);
    open_list(&mut app, &ctx, &book);
    // The sibling is a real window-bound independent reader. An unbound builder
    // fixture is a transient owner that the next build may legitimately retire.
    assert!(app.start_active_detached_book_context(
        descriptor(&book, None, "B/", "B"),
        &ctx,
        None,
        None
    ));
    assert_reader(&mut app, &ctx, &book, "B", "B/001.png");
    let sibling = app.active_viewer_context_id().unwrap();
    let sibling_window = app.active_detached_window_id().unwrap();
    let sibling_placement = app
        .with_active_viewer_context(|viewer| {
            let page = viewer.fullscreen_idx.unwrap();
            let pixels = Arc::new(egui::ColorImage::filled([8, 12], egui::Color32::WHITE));
            let tex = ctx.load_texture(
                "s3-sibling-page",
                (*pixels).clone(),
                egui::TextureOptions::LINEAR,
            );
            viewer.fs_cache.insert(
                page,
                FsCacheEntry::Static {
                    pdf_metadata: None,
                    pdf_rerender_failures: Vec::new(),
                    tex,
                    pixels,
                    source_dims: Some([8, 12]),
                    load_seq: 0,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
            viewer.mark_active_detached_viewport_rendered(&ctx);
            assert_eq!(
                viewer.active_detached_session.unwrap().content_phase,
                DetachedSessionContentPhase::Ready
            );
            assert_eq!(
                viewer.active_detached_window_state(),
                Some(DetachedWindowState::Active)
            );
            viewer.detached_viewer_window_placement()
        })
        .unwrap();
    assert!(app.park_and_close_current_active_detached_viewer(&ctx));
    assert_eq!(
        app.detached_window_state(sibling_window),
        Some(DetachedWindowState::Parked)
    );
    assert!(
        app.detached_image_windows
            .iter()
            .any(|snapshot| snapshot.id == sibling_window)
    );
    let sibling_before = app
        .with_viewer_context(sibling, |viewer| {
            assert_eq!(entry(viewer), "B/001.png");
            // Hold inert queues so asynchronous drain cannot obscure ownership.
            viewer.reload_queue = Some(Arc::new((
                std::sync::Mutex::new(Vec::new()),
                std::sync::Condvar::new(),
            )));
            viewer.heavy_io_queue = Some(Arc::new((
                std::sync::Mutex::new(Vec::new()),
                std::sync::Condvar::new(),
            )));
            (
                main_state(viewer),
                Arc::clone(&viewer.cancel_token),
                viewer.reload_queue.clone().unwrap(),
                viewer.heavy_io_queue.clone().unwrap(),
                viewer.cancel_token.load(Ordering::Relaxed),
            )
        })
        .unwrap();
    let main_before = main_state(&app);
    activate(&mut app, &ctx, "A/");
    assert_reader(&mut app, &ctx, &book, "A", "A/001.png");
    app.with_active_viewer_context(|viewer| {
        viewer.mark_active_detached_viewport_rendered(&ctx);
        assert_eq!(
            viewer.active_detached_session.unwrap().content_phase,
            DetachedSessionContentPhase::Ready
        );
        assert_eq!(
            viewer.active_detached_window_state(),
            Some(DetachedWindowState::Active)
        );
        viewer.handle_fullscreen_close_request_immediate();
    })
    .unwrap();
    app.with_viewer_context(sibling, |viewer| {
        assert_eq!(main_state(viewer), sibling_before.0);
        assert_eq!(entry(viewer), "B/001.png");
        assert!(Arc::ptr_eq(&viewer.cancel_token, &sibling_before.1));
        assert_eq!(sibling_before.1.load(Ordering::Relaxed), sibling_before.4);
        assert!(Arc::ptr_eq(
            viewer.reload_queue.as_ref().unwrap(),
            &sibling_before.2
        ));
        assert!(Arc::ptr_eq(
            viewer.heavy_io_queue.as_ref().unwrap(),
            &sibling_before.3
        ));
    })
    .unwrap();
    assert_eq!(
        app.locate_window_context(sibling_window),
        Some((sibling, ContextResidence::AtRest))
    );
    assert_eq!(
        app.detached_window_state(sibling_window),
        Some(DetachedWindowState::Parked)
    );
    assert!(
        app.detached_image_windows
            .iter()
            .any(|snapshot| snapshot.id == sibling_window)
    );
    app.with_viewer_context(sibling, |viewer| {
        assert_eq!(viewer.detached_viewer_window_placement(), sibling_placement);
    })
    .unwrap();
    assert_eq!(main_state(&app), main_before);
}

/// Replace only the worker transport. The production request, lease, path, and
/// presentation stay in their original owner, allowing deterministic late delivery.
fn hold_enumeration(
    app: &mut App,
) -> mpsc::Sender<Result<crate::zip_loader::PreparedZipEnumeration, String>> {
    let (tx, rx) = mpsc::channel();
    app.with_active_viewer_context(|viewer| {
        let pending = viewer
            .zip_enumerate_pending
            .as_mut()
            .expect("initial enumeration owner");
        assert!(matches!(
            pending.presentation,
            ZipEnumerationPresentation::ArchiveBook(_)
        ));
        pending.cancel.store(true, Ordering::Relaxed);
        pending.cancel = Arc::new(AtomicBool::new(false));
        pending.rx = rx;
    })
    .unwrap();
    tx
}

#[test]
fn section1675_multiwindow_scope_stale_lease() {
    for terminal in ["cancel", "window_replaced", "remote", "exit"] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("late.zip");
        write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
        open_list(&mut app, &ctx, &book);
        let before = main_state(&app);
        activate(&mut app, &ctx, "A/");
        let lease = DetachedSessionLease {
            window_id: app.active_detached_window_id().unwrap(),
        };
        let tx = hold_enumeration(&mut app);
        let late = prepared(&app, &book);
        match terminal {
            "cancel" => {
                app.with_active_viewer_context(|viewer| {
                    assert!(viewer.cancel_detached_initial_open_for_lease(lease, "s3_test_cancel"));
                    assert!(viewer.zip_enumerate_pending.is_none());
                })
                .unwrap();
                assert!(app.close_current_active_viewer_context(&ctx));
            }
            "window_replaced" => {
                assert!(app.close_current_active_viewer_context(&ctx));
            }
            "remote" => {
                let handle = crate::remote_ipc::session::SessionHandle::new();
                app.set_remote_session_handle(handle.clone());
                handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
                    client_id: "s3-test".into(),
                    peer: mimageviewer_ipc::SessionPeerInfo {
                        connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                        device_name: None,
                    },
                });
                app.poll_remote_session(&ctx);
                assert_eq!(
                    handle.snapshot().phase,
                    crate::remote_ipc::session::RemoteControlPhase::RemoteActive
                );
                assert!(
                    tx.send(Ok(late)).is_err(),
                    "Remote must consume the original owner"
                );
                handle.local_disconnect();
                app.poll_remote_session(&ctx);
                // Remote's own local-return contract may rematerialize main. Its
                // lifetime invariant here is that the retired request cannot reopen.
                assert!(app.active_viewer_context_id().is_none());
                continue;
            }
            "exit" => {
                app.close_all_viewer_sessions_with_cause(&ctx, ViewerCloseCause::AppExit)
                    .unwrap();
                assert!(tx.send(Ok(late)).is_err());
                assert!(app.active_viewer_context_id().is_none());
                assert_eq!(main_state(&app), before);
                continue;
            }
            _ => unreachable!(),
        }
        // A replacement reader is real and uses its own enumeration/restore owners.
        assert!(app.start_active_detached_book_context(
            descriptor(&book, None, "B/", "B"),
            &ctx,
            None,
            None
        ));
        let replacement = app.active_viewer_context_id().unwrap();
        let replacement_window = app.active_detached_window_id().unwrap();
        assert_ne!(replacement_window, lease.window_id);
        assert!(
            tx.send(Ok(late)).is_err(),
            "{terminal}: cancelled transport must be gone"
        );
        assert_reader(&mut app, &ctx, &book, "B", "B/001.png");
        assert_eq!(app.active_viewer_context_id(), Some(replacement));
        assert_eq!(app.active_detached_window_id(), Some(replacement_window));
        app.with_active_viewer_context(|viewer| {
            assert!(!viewer.cancel_detached_initial_open_for_lease(lease, "s3_test_stale_lease"));
            viewer.finalize_archive_book_open_outcome(ArchiveBookOpenOutcome::Stale);
            assert_eq!(entry(viewer), "B/001.png");
            assert!(!viewer.active_detached_window_is_closing());
        })
        .unwrap();
        assert_eq!(main_state(&app), before);
    }
}

#[test]
fn section1675_multiwindow_scope_wait_is_owned() {
    use crate::book_composition::{
        BookComposition, BookCompositionService, CompositionPageRef, PageDomain, ReadingDirection,
    };
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("wait.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    let before = main_state(&app);
    let (service, owner) =
        BookCompositionService::spawn_at(app.tmp.path().join("composition"), None).unwrap();
    app.install_book_composition_runtime(service.clone());
    let (entered, release) = service.hold_worker_for_test();
    entered.recv_timeout(WAIT).unwrap();
    let save = service.save(
        scope(&book, "A"),
        PageDomain::ArchiveLevel { prefix: "A".into() },
        BookComposition::new(
            ReadingDirection::Ltr,
            vec![
                CompositionPageRef::ArchiveEntry {
                    entry_name: "A/002.png".into(),
                },
                CompositionPageRef::ArchiveEntry {
                    entry_name: "A/001.png".into(),
                },
            ],
        ),
    );
    activate(&mut app, &ctx, "A/");
    let lease = DetachedSessionLease {
        window_id: app.active_detached_window_id().unwrap(),
    };
    let tx = hold_enumeration(&mut app);
    app.with_active_viewer_context(|viewer| {
        assert_eq!(
            viewer.active_detached_session.unwrap().content_phase,
            DetachedSessionContentPhase::Preparing
        );
        assert!(viewer.active_detached_transition_outstanding());
        assert!(viewer.fs_nav_after_pdf_enumerate.is_none());
        assert!(viewer.fullscreen_idx.is_none());
    })
    .unwrap();
    tx.send(Ok(prepared(&app, &book))).unwrap();
    // Stop at adoption before polling hydration. This is the production handoff,
    // not a synthetic second request in another deferred slot.
    let (deliver_prepare, original_prepare) = app
        .with_active_viewer_context(|viewer| {
            viewer.poll_zip_enumerate();
            assert!(viewer.zip_enumerate_pending.is_none());
            assert!(viewer.sidecar_restore_active());
            assert_eq!(
                viewer.active_detached_session.unwrap().content_phase,
                DetachedSessionContentPhase::Preparing
            );
            assert!(viewer.active_detached_transition_outstanding());
            assert!(viewer.fullscreen_idx.is_none());
            viewer.poll_book_composition_service_for_test(&ctx);
            let book_composition_runtime::ViewerBookComposition::Preparing { receiver, .. } =
                &mut viewer.book_composition
            else {
                panic!("the queued save must hold real composition classification");
            };
            let (deliver, waiting) = mpsc::channel();
            (deliver, std::mem::replace(receiver, waiting))
        })
        .unwrap();
    // Preparing is part of hydration's shift/read classification contract. Let
    // that real reply advance to Projecting, then hold only the second worker's
    // delivery so hydration can finish and transfer the open to permission.
    release.send(()).unwrap();
    save.recv_timeout(WAIT).unwrap().status.unwrap();
    deliver_prepare
        .send(original_prepare.recv_timeout(WAIT).unwrap())
        .unwrap();
    let (deliver_projection, original_projection) = app
        .with_active_viewer_context(|viewer| {
            viewer.poll_book_composition_service_for_test(&ctx);
            let book_composition_runtime::ViewerBookComposition::Projecting { receiver, .. } =
                &mut viewer.book_composition
            else {
                panic!("the accepted saved record must start the real projection worker");
            };
            let (deliver, waiting) = mpsc::channel();
            (deliver, std::mem::replace(receiver, waiting))
        })
        .unwrap();
    let projection_reply = original_projection.recv_timeout(WAIT).unwrap();
    let deadline = Instant::now() + WAIT;
    loop {
        let waiting = app
            .with_active_viewer_context(|viewer| {
                tick(viewer, &ctx);
                assert_eq!(
                    viewer.active_detached_session.unwrap().content_phase,
                    DetachedSessionContentPhase::Preparing
                );
                assert!(viewer.active_detached_transition_outstanding());
                assert!(viewer.fullscreen_idx.is_none());
                viewer.book_composition_permission.pending()
            })
            .unwrap();
        if waiting {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "composition wait must own the request"
        );
        std::thread::yield_now();
    }
    app.with_active_viewer_context(|viewer| {
        assert!(viewer.cancel_detached_initial_open_for_lease(lease, "s3_test_composition_cancel"));
        assert!(!viewer.book_composition_permission.pending());
        assert!(!viewer.fs_nav_deferred_reopen_wait_active());
    })
    .unwrap();
    // Completion can still update the list's composition; it cannot regain the
    // cancelled request's authority to display the first page.
    let _ = deliver_projection.send(projection_reply);
    app.with_active_viewer_context(|viewer| {
        for _ in 0..3 {
            tick(viewer, &ctx);
        }
        assert!(viewer.fullscreen_idx.is_none());
        assert!(viewer.active_detached_window_is_closing());
        assert!(viewer.take_pending_return_to_parent_nav().is_none());
    })
    .unwrap();
    assert_eq!(main_state(&app), before);
    owner.shutdown_and_join();
}

#[test]
fn section1675_multiwindow_scope_cancel_during_hydration() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("hydration-cancel.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    let before = main_state(&app);
    activate(&mut app, &ctx, "A/");
    let lease = DetachedSessionLease {
        window_id: app.active_detached_window_id().unwrap(),
    };
    let tx = hold_enumeration(&mut app);
    tx.send(Ok(prepared(&app, &book))).unwrap();
    app.with_active_viewer_context(|viewer| {
        viewer.poll_zip_enumerate();
        assert!(viewer.sidecar_restore_active());
        assert!(viewer.fullscreen_idx.is_none());
        assert!(viewer.sidecar_restore_navigation_fullscreen_wait_active());
        assert!(viewer.cancel_detached_initial_open_for_lease(lease, "s3_test_hydration_cancel"));
        assert!(viewer.active_detached_window_is_closing());
        settle(viewer, &ctx);
        assert!(
            viewer.fullscreen_idx.is_none(),
            "late identity/hydration completion cannot reopen a cancelled book"
        );
        assert!(!viewer.sidecar_restore_navigation_fullscreen_wait_active());
        assert!(!viewer.fs_nav_deferred_reopen_wait_active());
    })
    .unwrap();
    assert_eq!(main_state(&app), before);
}

#[test]
fn section1675_multiwindow_scope_conversion_continuation() {
    use crate::ui_dialogs::archive_convert::{
        ArchiveConvertCompletionPolicy, ArchiveConvertPhase, ArchiveConvertState,
        ArchivePasswordResume,
    };
    for action in ["converted", "password_cancel", "stale"] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let composition_data = app.tmp.path().join("composition");
        let composition_owner = install_composition(&mut app, composition_data);
        let main_book = app.tmp.path().join("main.zip");
        let source = app.tmp.path().join("logical.rar");
        let backing = app.tmp.path().join("converted.zip");
        std::fs::write(
            &source,
            b"disposable logical source; converter is not launched",
        )
        .unwrap();
        write_pages(&main_book, &["A/001.png", "B/001.png"]);
        write_pages(
            &backing,
            &[
                "root.png",
                "Upper/001.png",
                "Upper/002.png",
                "Other/001.png",
            ],
        );
        open_list(&mut app, &ctx, &main_book);
        seed_resume(&mut app, &source, "Upper", "Upper/002.png");
        app.settings.book_open_resume = crate::settings::ResumeMode::Resume;
        let before = main_state(&app);
        let window = app
            .start_active_detached_loading_context(
                &ctx,
                None,
                DetachedSource::Book,
                source.clone(),
                |_, _| DetachedLoadingContextInit::Continue,
            )
            .unwrap();
        app.detached_grid_archive_open_request_seq += 1;
        let owner = DetachedGridArchiveOpenRequestOwner {
            request_id: app.detached_grid_archive_open_request_seq,
            lease: DetachedSessionLease { window_id: window },
            source_path: source.clone(),
            target: DetachedZipTarget::BookScope(target(&source, "Upper/", "Upper")),
            collection_restore: None,
        };
        let (_tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        app.archive_convert = Some(Box::new(ArchiveConvertState {
            restore_intent: StartupListIntent::ExplicitList,
            src_path: source.clone(),
            input_seq: app.input_seq,
            format: crate::archive_converter::ArchiveFormat::Rar,
            password: None,
            password_input: String::new(),
            phase: if action == "password_cancel" {
                ArchiveConvertPhase::PasswordRequired {
                    message: None,
                    resume: ArchivePasswordResume::Scan,
                }
            } else {
                ArchiveConvertPhase::Scanning
            },
            cancel: Arc::clone(&cancel),
            rx,
            pending_nav: (action != "password_cancel").then(|| backing.clone()),
            pending_direct_nav: None,
            allow_direct_read: true,
            completion: ArchiveConvertCompletionPolicy::DetachedGridArchive(owner.clone()),
            pending_sibling_output: None,
            auto_fullscreen: true,
            deferred_fullscreen: None,
            suppress_confirm: false,
            suppress_confirm_next_time: false,
        }));
        if action == "password_cancel" {
            app.with_active_viewer_context(|viewer| {
                assert!(
                    viewer
                        .cancel_detached_initial_open_for_lease(owner.lease, "s3_password_cancel")
                );
            })
            .unwrap();
            assert!(app.archive_convert.is_none());
            assert!(cancel.load(Ordering::Relaxed));
            assert!(!app.detached_grid_archive_open_owner_is_current(&owner));
            assert_eq!(
                app.detached_window_state(window),
                Some(DetachedWindowState::Closing)
            );
        } else if action == "stale" {
            assert!(app.close_current_active_viewer_context(&ctx));
            // Delivering a stale converted backing must not manufacture a root reader.
            ctx.run_pass(Default::default(), |ctx| {
                app.show_archive_convert_dialog(ctx)
            })
            .drop_without_applying_deltas();
            assert!(app.active_viewer_context_id().is_none());
            assert!(app.archive_convert.is_none());
        } else {
            // A completed conversion arrives at the existing modal completion
            // seam with the same typed request owner.
            ctx.run_pass(Default::default(), |ctx| {
                app.show_archive_convert_dialog(ctx)
            })
            .drop_without_applying_deltas();
            assert!(app.archive_convert.is_none());
            assert_reader(&mut app, &ctx, &source, "Upper", "Upper/002.png");
            assert_eq!(
                app.active_detached_window_id(),
                Some(window),
                "continue the original lease"
            );
            app.with_active_viewer_context(|viewer| {
                assert_eq!(viewer.current_folder.as_deref(), Some(backing.as_path()));
                assert_eq!(
                    viewer.archive_source_override.as_deref(),
                    Some(source.as_path())
                );
                assert_eq!(
                    viewer.book_composition.scope(),
                    Some(&scope(&source, "Upper"))
                );
            })
            .unwrap();
        }
        assert_eq!(main_state(&app), before, "{action}");
        composition_owner.shutdown_and_join();
    }
}

#[test]
fn section1675_multiwindow_scope_park_and_reopen() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("reopen.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    let before = main_state(&app);
    activate(&mut app, &ctx, "A/");
    assert_reader(&mut app, &ctx, &book, "A", "A/001.png");
    let window = app.active_detached_window_id().unwrap();
    let parked_context = app.active_viewer_context_id().unwrap();
    let placement = app.with_active_viewer_context(|viewer| {
        let page = viewer.items.iter().position(|item| matches!(item, GridItem::ZipImage { entry_name, .. } if entry_name == "A/002.png")).unwrap();
        viewer.fullscreen_idx = Some(page);
        let pixels = Arc::new(egui::ColorImage::filled([8, 12], egui::Color32::WHITE));
        let tex = ctx.load_texture("s3-parked-page", (*pixels).clone(), egui::TextureOptions::LINEAR);
        viewer.fs_cache.insert(page, FsCacheEntry::Static {
            pdf_metadata: None, pdf_rerender_failures: Vec::new(), tex, pixels,
            source_dims: Some([8, 12]), load_seq: 0,
            animation: crate::fs_animation::StaticAnimationState::Still,
        });
        viewer.detached_viewer_window_placement()
    }).unwrap();
    assert!(app.park_and_close_current_active_detached_viewer(&ctx));
    assert_eq!(
        app.detached_window_state(window),
        Some(DetachedWindowState::Parked)
    );
    let snapshot = app
        .detached_image_windows
        .iter()
        .find(|snapshot| snapshot.id == window)
        .unwrap();
    assert!(
        matches!(&snapshot.reopen_descriptor, Some(ViewerContextDescriptor::Zip { target: DetachedZipTarget::RequiredPage { entry_name }, .. }) if entry_name == "A/002.png")
    );
    // Drop the retained bundle to reach the real descriptor-reopen consumer;
    // the existing snapshot still owns the parked window and placement.
    app.retire_context(parked_context, "s3_test_descriptor_reopen", |_| ())
        .unwrap();
    assert!(app.activate_detached_image_window_snapshot(&ctx, window));
    settle_detached(&mut app, &ctx);
    assert_eq!(app.active_detached_window_id(), Some(window));
    app.with_active_viewer_context(|viewer| {
        assert_eq!(entry(viewer), "A/002.png");
        assert_eq!(viewer.detached_viewer_window_placement(), placement);
        assert!(
            viewer
                .zip_nav
                .as_ref()
                .unwrap()
                .chosen_list_return_plan()
                .is_none()
        );
    })
    .unwrap();
    assert_eq!(main_state(&app), before);
}
