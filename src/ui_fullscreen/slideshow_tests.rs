//! Slideshow admission and cleanup through the production input owners.
use super::*;
use crate::ring_shortcut::{RingActionId, RingShortcutContext};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
enum Entry {
    Hud,
    Key,
    Ring,
}

fn render(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
    app.frame_counter += 1;
    crate::test_ui::headless_paint_output(ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 700.0),
            )),
            focused: true,
            time: Some(app.frame_counter as f64 / 60.0),
            events,
            ..Default::default()
        },
        |ui| app.render_fullscreen_viewport(ui),
    ))
    .drop_without_applying_deltas();
}

fn click(pos: egui::Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

fn activate(app: &mut App, ctx: &egui::Context, entry: Entry) {
    match entry {
        Entry::Hud => {
            // Read the actual button after a settled frame; do not duplicate its layout.
            render(app, ctx, vec![]);
            let pos = ctx
                .read_response(egui::Id::unique("fs_play_btn"))
                .expect("the locked image HUD must expose its play button")
                .rect
                .center();
            render(app, ctx, vec![egui::Event::PointerMoved(pos)]);
            render(app, ctx, click(pos));
        }
        Entry::Key => {
            let idx = app.fullscreen_idx.unwrap();
            ctx.run_pass(
                egui::RawInput {
                    focused: true,
                    events: vec![egui::Event::Key {
                        key: egui::Key::S,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    app.handle_fs_key_input(ctx, idx, false);
                },
            )
            .drop_without_applying_deltas();
            ctx.run_pass(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: egui::Key::S,
                        physical_key: None,
                        pressed: false,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |_| {},
            )
            .drop_without_applying_deltas();
        }
        Entry::Ring => {
            app.apply_ring_action(
                ctx,
                RingShortcutContext::ImageFullscreen,
                RingActionId::ImageSlideshow,
                "slideshow_test",
            );
        }
    }
}

fn seed_scroll_state(app: &mut App) {
    app.continuous_reading_scroll_transition = Some(
        crate::app::ContinuousReadingScrollTransition::AwaitingReanchor {
            history_trigger: crate::app::HistoryTrigger::UserChosen,
        },
    );
    app.slideshow_scroll_range_cache = Some((0, 0.0, 400.0));
}

fn assert_stopped(app: &App) {
    assert!(!app.slideshow_playing);
    assert_eq!(app.slideshow_anchor_idx, None);
    assert!(app.continuous_reading_scroll_transition.is_none());
    assert_eq!(app.slideshow_scroll_range_cache, None);
}

#[test]
fn slideshow_hud_key_and_ring_share_start_and_stop_cleanup() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for entry in [Entry::Hud, Entry::Key, Entry::Ring] {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        page_turn::install(&ctx);
        install_fs_navigator_input_tracking(&ctx);
        let mut app = super::page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_top_bar_locked = true;
        app.settings.slideshow_interval_secs = 30.0;
        app.keymap = Keymap::default();
        #[cfg(windows)]
        {
            app.native_video_in_window_active = true;
        }
        let before = Instant::now();
        activate(&mut app, &ctx, entry);
        let after = Instant::now();
        assert!(app.slideshow_playing, "entry={entry:?}");
        assert_eq!(app.slideshow_anchor_idx, Some(0), "entry={entry:?}");
        assert!(app.slideshow_next_at >= before + Duration::from_secs(30));
        assert!(app.slideshow_next_at <= after + Duration::from_secs(30));
        assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Closed);
        seed_scroll_state(&mut app);
        activate(&mut app, &ctx, entry);
        assert_stopped(&app);
        assert_eq!(app.fullscreen_idx, Some(0), "entry={entry:?}");
        assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Closed);
    }
}

#[test]
fn slideshow_start_accepts_only_displayed_still_pages_and_stop_always_cleans_up() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    app.items = vec![
        GridItem::Image(PathBuf::from("c:/slideshow/image.png")),
        GridItem::ZipImage {
            zip_path: PathBuf::from("c:/slideshow/book.zip"),
            entry_name: "page.png".to_string(),
        },
        GridItem::PdfPage {
            pdf_path: PathBuf::from("c:/slideshow/book.pdf"),
            page_num: 0,
            content_type: None,
        },
        GridItem::Video(PathBuf::from("c:/slideshow/video.mp4")),
        GridItem::Audio(PathBuf::from("c:/slideshow/music.flac")),
        GridItem::Folder(PathBuf::from("c:/slideshow/folder")),
        GridItem::ZipFile(PathBuf::from("c:/slideshow/book.zip")),
        GridItem::PdfFile(PathBuf::from("c:/slideshow/book.pdf")),
    ];
    app.settings.slideshow_interval_secs = 30.0;
    for idx in 0..=app.items.len() {
        app.fullscreen_idx = Some(idx);
        for ring in [false, true] {
            let clock = app.slideshow_next_at;
            if ring {
                activate(&mut app, &ctx, Entry::Ring);
            } else {
                app.toggle_fullscreen_slideshow(idx);
            }
            assert_eq!(app.slideshow_playing, idx < 3, "idx={idx}, ring={ring}");
            if idx < 3 {
                assert_eq!(app.slideshow_anchor_idx, Some(idx));
                app.toggle_fullscreen_slideshow(idx);
            } else {
                assert_eq!(app.slideshow_next_at, clock);
                assert_eq!(app.slideshow_anchor_idx, None);
            }
            // A non-still item (or a no-longer-valid index) must still allow stopping.
            app.slideshow_playing = true;
            app.slideshow_anchor_idx = Some(0);
            seed_scroll_state(&mut app);
            app.toggle_fullscreen_slideshow(idx);
            assert_stopped(&app);
        }
    }
}

#[test]
fn slideshow_overflow_file_change_policy_covers_all_states_and_fullscreen_exit() {
    let mut app = crate::app::setup_app_for_test();
    for (state, closes) in [
        (FsOverflowPanelState::Closed, false),
        (FsOverflowPanelState::Root, false),
        (FsOverflowPanelState::NavigatorPosition, false),
        (FsOverflowPanelState::SpreadAdjustment, true),
        (FsOverflowPanelState::MusicNoteColor, false),
        (FsOverflowPanelState::Slideshow, false),
    ] {
        assert_eq!(state.closes_on_file_change(), closes, "{state:?}");
        app.fs_overflow_panel_state = state;
        app.slideshow_playing = true;
        app.slideshow_anchor_idx = Some(1);
        let clock = app.slideshow_next_at;
        app.reset_fs_side_panel_runtime_for_file_change();
        assert_eq!(
            app.fs_overflow_panel_state,
            if closes {
                FsOverflowPanelState::Closed
            } else {
                state
            },
            "{state:?}"
        );
        assert!(app.slideshow_playing);
        assert_eq!(app.slideshow_anchor_idx, Some(1));
        assert_eq!(app.slideshow_next_at, clock);
        app.close_fullscreen();
        assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Closed);
        assert_stopped(&app);
    }
}

#[cfg(windows)]
#[test]
fn slideshow_toggle_mutates_only_the_mounted_viewer_context() {
    let ctx = egui::Context::default();
    let mut app = super::page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
    app.slideshow_playing = true;
    app.slideshow_anchor_idx = Some(0);
    seed_scroll_state(&mut app);
    let clock = app.slideshow_next_at;
    let generation = app.items_generation;
    let cache_count = app.fs_cache.len();
    app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
    let sibling = app.build_window_context_for_test(13960, |owner| {
        owner.items = vec![GridItem::Image(PathBuf::from("c:/slideshow/sibling.png"))];
        owner.fullscreen_idx = Some(0);
        owner.slideshow_playing = false;
        owner.slideshow_anchor_idx = None;
        owner.continuous_reading_scroll_transition = None;
        owner.slideshow_scroll_range_cache = None;
    });
    app.with_window_context_for_test(sibling, |owner| {
        activate(owner, &ctx, Entry::Ring);
        assert!(owner.slideshow_playing);
        assert_eq!(owner.slideshow_anchor_idx, Some(0));
        seed_scroll_state(owner);
        owner.toggle_fullscreen_slideshow(0);
        assert_stopped(owner);
        assert_eq!(
            owner.fs_overflow_panel_state,
            FsOverflowPanelState::Slideshow
        );
    });
    assert!(app.slideshow_playing);
    assert_eq!(app.slideshow_anchor_idx, Some(0));
    assert_eq!(app.slideshow_next_at, clock);
    assert!(app.continuous_reading_scroll_transition.is_some());
    assert_eq!(app.slideshow_scroll_range_cache, Some((0, 0.0, 400.0)));
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.fs_cache.len(), cache_count);
    assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Slideshow);
    app.with_window_context_for_test(sibling, |owner| assert_stopped(owner));
}

#[test]
fn slideshow_automatic_page_advance_keeps_the_settings_subpanel() {
    let ctx = egui::Context::default();
    let mut app = super::page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = false;
    app.toggle_fullscreen_slideshow(0);
    app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
    app.advance_slideshow(&ctx, 0);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(app.slideshow_playing);
    assert_eq!(app.slideshow_anchor_idx, Some(1));
    assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Slideshow);
}

#[test]
fn slideshow_real_timer_advance_keeps_the_overflow_root_open() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    page_turn::install(&ctx);
    install_fs_navigator_input_tracking(&ctx);
    let mut app = page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_top_bar_locked = true;
    app.settings.page_turn_animation_enabled = false;
    app.settings.slideshow_interval_secs = 30.0;
    #[cfg(windows)]
    {
        app.native_video_in_window_active = true;
    }
    for _ in 0..3 {
        render(&mut app, &ctx, vec![]);
    }
    assert!(app.fullscreen_page_layout.single_page().is_some());
    app.toggle_fullscreen_slideshow(0);
    let pos = ctx
        .read_response(egui::Id::unique("fs_overflow_btn"))
        .expect("the actual HUD must expose the overflow button")
        .rect
        .center();
    render(&mut app, &ctx, vec![egui::Event::PointerMoved(pos)]);
    render(&mut app, &ctx, click(pos));
    render(&mut app, &ctx, vec![]);
    assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Root);
    assert_eq!(app.fullscreen_idx, Some(0));

    // Expire the deadline without calling advance_slideshow directly: the next
    // production fullscreen pass must execute its timer and file-change reset.
    app.slideshow_next_at = Instant::now() - Duration::from_millis(1);
    render(&mut app, &ctx, vec![]);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Root);
    render(&mut app, &ctx, vec![]);
    assert!(app.fullscreen_page_layout.single_page().is_some());
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(app.slideshow_playing);
    assert_eq!(app.slideshow_anchor_idx, Some(1));
    assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Root);
    assert!(app.slideshow_next_at > Instant::now());
}

#[test]
fn slideshow_retained_overflow_root_rebuilds_file_dependent_view() {
    use crate::app::book_materialization::{BookScope, ProjectionRecipe};
    use crate::app::spread_shift_owner::ViewerShift;

    let mut app = crate::app::setup_app_for_test();
    app.fs_overflow_panel_state = FsOverflowPanelState::Root;
    app.projection_recipe = ProjectionRecipe::Book(BookScope::ImageFolder(PathBuf::from(
        "c:/slideshow/first-book",
    )));
    app.spread_shift.state = ViewerShift::Ordinary(1);
    let first = app.fullscreen_overflow_panel_view(false);
    let shift_status = |view: &FsOverflowPanelView| {
        view.root_rows
            .iter()
            .find(|row| row.item == FullscreenOverflowItemId::SpreadAdjustment)
            .unwrap()
            .status
            .clone()
    };
    assert_eq!(shift_status(&first), "調整あり  >");
    assert!(first.composition_available);

    // Model the accepted source/shift state at the shared file-change boundary.
    // Root survives, but neither its row status nor composition eligibility may
    // retain a view from the old file/book.
    app.projection_recipe = ProjectionRecipe::OrdinaryList;
    app.spread_shift.state = ViewerShift::None;
    app.reset_fs_side_panel_runtime_for_file_change();
    let next = app.fullscreen_overflow_panel_view(false);
    assert_eq!(next.state, FsOverflowPanelState::Root);
    assert_eq!(shift_status(&next), "確認  >");
    assert!(!next.composition_available);
}
