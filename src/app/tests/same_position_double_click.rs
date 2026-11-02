//! Real grid handlers and navigation, without a window or OS pointer input.
use super::*;
use std::io::Write;

fn settle(app: &mut App, ctx: &egui::Context) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        app.settle_open_path_classification_for_test();
        app.poll_history_navigation_transition_for_test(ctx);
        app.poll_zip_enumerate();
        app.poll_reading_history_thumbnail_sources(ctx);
        app.poll_sidecar_restore(ctx);
        app.poll_book_composition_service_for_test(ctx);
        app.poll_book_presentation_permission(ctx);
        if app.top_level_grid_view.open_path_classification().is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && app.zip_enumerate_pending.is_none()
            && app.fs_nav_after_pdf_enumerate.is_none()
            && app.reading_history_sources_pending.is_empty()
            && !app.sidecar_restore_active()
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "listing did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn dispatch(app: &mut App, ctx: &egui::Context, nav: crate::ui_main::AddressBarNav) {
    app.post_render_main_navigation(
        ctx,
        update_phases::MainFrameNavigation {
            fav_nav: None,
            toolbar_fav_nav: None,
            fullscreen_close_nav: None,
            keyboard_nav: None,
            keyboard_nav_from_fullscreen_close: false,
            gamepad_nav: None,
            address_nav: None,
            open_folder_nav: None,
            context_nav: None,
            folder_pane_nav: None,
            grid_nav: Some(nav),
        },
    );
}

fn check_same_position(opening_batch_size: usize, second_batch_size: usize, reclick: bool) {
    for idle_frames in [0, 200] {
        for source in ["folder", "zip", "zipdir"] {
            for target in [0usize, 1] {
                let mut app = setup_app_for_test();
                app.settings.grid_view_mode = crate::settings::GridViewMode::Thumbnail;
                app.settings.grid_click_selection_mode =
                    crate::settings::GridClickSelectionMode::Explorer;
                app.settings.grid_open_selected_item_on_click = reclick;
                app.settings.auto_fullscreen_zip_pdf = false;
                let root = app.tmp.path().join("same-position");
                std::fs::create_dir_all(&root).unwrap();
                let mut png = std::io::Cursor::new(Vec::new());
                image::DynamicImage::new_rgb8(8, 8)
                    .write_to(&mut png, image::ImageFormat::Png)
                    .unwrap();
                if source != "folder" {
                    let mut zip =
                        zip::ZipWriter::new(std::fs::File::create(root.join("book.zip")).unwrap());
                    let prefixes: &[&str] = if source == "zipdir" {
                        &["inner_A/", "inner_B/", "inner_C/"]
                    } else {
                        &[""]
                    };
                    for name in prefixes.iter().flat_map(|prefix| {
                        ["001.png", "002.png", "003.png"].map(|name| format!("{prefix}{name}"))
                    }) {
                        zip.start_file(name, zip::write::SimpleFileOptions::default())
                            .unwrap();
                        zip.write_all(png.get_ref()).unwrap();
                    }
                    zip.finish().unwrap();
                } else {
                    std::fs::create_dir_all(root.join("book")).unwrap();
                    for name in ["001.png", "002.png", "003.png"] {
                        std::fs::write(root.join("book").join(name), png.get_ref()).unwrap();
                    }
                }
                if target == 1 && source != "zipdir" {
                    std::fs::create_dir_all(root.join("000")).unwrap();
                }
                app.load_folder(if source == "zipdir" {
                    root.join("book.zip")
                } else {
                    root
                });
                let mut harness = crate::test_ui::builder()
                    .with_context_setup(crate::ui_fonts::configure_fonts)
                    .with_size(egui::vec2(960.0, 600.0))
                    .with_step_dt(0.01)
                    .build_ui_state(
                        |ui, state: &mut (AppTestEnvForTest, usize)| {
                            let ctx = ui.ctx().clone();
                            settle(&mut state.0, &ctx);
                            state.0.begin_grid_click_input_frame(&ctx);
                            if state.0.fullscreen_idx.is_some() {
                                return;
                            }
                            let generation = state.0.items_generation;
                            let nav = state.0.render_grid(ui);
                            if nav.is_some() || generation != state.0.items_generation {
                                state.1 += 1;
                            }
                            if let Some(nav) = nav {
                                dispatch(&mut state.0, &ctx, nav);
                            }
                        },
                        (app, 0),
                    );
                harness.run();
                let pos = harness
                    .ctx
                    .data(|data| {
                        data.get_temp::<egui::Response>(egui::Id::unique((
                            "grid_cell_input_response",
                            harness.state().0.grid_scroll_context_id(),
                            target,
                        )))
                    })
                    .unwrap()
                    .rect
                    .center();
                harness.hover_at(pos);
                harness.run();
                // Historical comparisons open in four passes, since the old
                // implementation already loses same-batch click multiplicity.
                // Only the initial positioning emits PointerMoved.
                for opening in [true, false] {
                    if opening {
                        // Protect the early-reclick premise independently of the
                        // selection installed after the container has opened.
                        assert_eq!(harness.state().0.selected, Some(0));
                    }
                    let batch_size = if opening {
                        opening_batch_size
                    } else {
                        second_batch_size
                    };
                    for (batch, edges) in [true, false, true, false].chunks(batch_size).enumerate()
                    {
                        let dt = if !opening && batch == 0 && idle_frames == 0 {
                            2.0
                        } else {
                            0.01
                        };
                        harness.input_mut().time = Some(harness.ctx.input(|input| input.time) + dt);
                        harness.input_mut().events = edges
                            .iter()
                            .map(|&pressed| egui::Event::PointerButton {
                                pos,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            })
                            .collect();
                        harness.step();
                    }
                    if opening {
                        harness.input_mut().time = None;
                        harness.run_steps(idle_frames);
                        let ctx = harness.ctx.clone();
                        settle(&mut harness.state_mut().0, &ctx);
                    }
                    eprintln!(
                        "samepos source={source} target={target} batch={batch_size} idle={idle_frames} reclick={reclick} opening={opening} opens={} selected={:?} fullscreen={:?} rows={:?}",
                        harness.state().1,
                        harness.state().0.selected,
                        harness.state().0.fullscreen_idx,
                        harness
                            .state()
                            .0
                            .items
                            .iter()
                            .map(|i| i.name().into_owned())
                            .collect::<Vec<_>>()
                    );
                    if opening {
                        assert_eq!(harness.state().1, 1, "{source} target={target}");
                        assert_eq!(harness.state().0.fullscreen_idx, None);
                        assert_eq!(harness.state().0.items.len(), 3);
                        assert_eq!(
                            harness.state().0.items[target].name(),
                            if target == 0 { "001.png" } else { "002.png" }
                        );
                        assert_eq!(harness.state().0.selected, Some(0));
                    } else {
                        assert_eq!(
                            harness.state().0.fullscreen_idx,
                            Some(target),
                            "{source} target={target} batch={batch_size}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn section1680_same_position_four_edge_passes_after_listing_replacement() {
    check_same_position(1, 1, false);
}

#[test]
fn section1680_same_position_two_click_passes_after_listing_replacement() {
    check_same_position(1, 2, false);
}

#[test]
fn section1680_same_position_one_batch_after_listing_replacement() {
    check_same_position(1, 4, false);
}

#[test]
fn section1680_same_position_batched_open_and_batched_reopen() {
    check_same_position(4, 4, false);
}

#[test]
fn section1680_same_position_batched_early_reclick_discards_remaining_edges() {
    // Target 0 is initially selected: its first release opens, leaving the
    // second press/release in the same batch to the activation consumption gate.
    check_same_position(4, 4, true);
}
