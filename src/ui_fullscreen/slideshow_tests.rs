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
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 700.0),
            )),
            focused: true,
            events,
            ..Default::default()
        },
        |ui| app.render_fullscreen_viewport(ui),
    )
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
fn slideshow_subpanel_survives_file_change_but_closes_at_fullscreen_exit() {
    let mut app = crate::app::setup_app_for_test();
    for state in [
        FsOverflowPanelState::Root,
        FsOverflowPanelState::NavigatorPosition,
        FsOverflowPanelState::SpreadAdjustment,
        FsOverflowPanelState::MusicNoteColor,
        FsOverflowPanelState::Slideshow,
    ] {
        app.fs_overflow_panel_state = state;
        app.slideshow_playing = true;
        app.slideshow_anchor_idx = Some(1);
        let clock = app.slideshow_next_at;
        app.reset_fs_side_panel_runtime_for_file_change();
        assert_eq!(
            app.fs_overflow_panel_state,
            if state == FsOverflowPanelState::Slideshow {
                state
            } else {
                FsOverflowPanelState::Closed
            }
        );
        assert!(app.slideshow_playing);
        assert_eq!(app.slideshow_anchor_idx, Some(1));
        assert_eq!(app.slideshow_next_at, clock);
    }
    app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
    app.close_fullscreen();
    assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Closed);
    assert_stopped(&app);
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
