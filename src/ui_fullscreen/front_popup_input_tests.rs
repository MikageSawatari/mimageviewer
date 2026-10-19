//! Backlog 1.402: non-overflow foreground menus through the actual fullscreen renderer.
//! These tests express the intended input boundary; the released raw handlers may violate it.
use super::*;

fn viewport(width: f32) -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 614.0))
}

fn sources(ctx: &egui::Context, dims: [usize; 2]) -> crate::app::AppTestEnvForTest {
    crate::ui_fonts::configure_fonts(ctx);
    page_turn::install(ctx);
    install_fs_navigator_input_tracking(ctx);
    let mut app = crate::app::setup_app_for_test();
    app.items = (0..4)
        .map(|idx| GridItem::Image(PathBuf::from(format!("c:/front-popup/{idx}.png"))))
        .collect();
    app.visible_indices = vec![0, 1, 2, 3];
    app.fullscreen_idx = Some(1);
    app.items_generation = 41;
    app.spread_mode = SpreadMode::Single;
    app.reading_flow = ReadingFlow::Paged;
    app.settings.touch_still_chrome_learned = true;
    app.settings.fullscreen_top_bar_locked = true;
    app.settings.fullscreen_navigator_visible = false;
    app.settings.set_still_seek_bar_locked(true);
    app.settings.still_seek_strip_visible = false;
    app.fs_prev_focused = true;
    app.fs_overflow_panel_state = FsOverflowPanelState::Closed;
    #[cfg(windows)]
    {
        app.native_video_in_window_active = true;
    }
    // Initialize the real source before the first preparation pass, avoiding stale
    // accepted pipeline material from replacing an already-rendered portrait fixture.
    let pixels = Arc::new(egui::ColorImage::filled(dims, egui::Color32::LIGHT_BLUE));
    for idx in 0..4 {
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex: ctx.load_texture(
                    format!("front-popup-source-{idx}"),
                    (*pixels).clone(),
                    egui::TextureOptions::LINEAR,
                ),
                pixels: Arc::clone(&pixels),
                source_dims: Some(dims),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.record_page_dims_for_spread(idx, (dims[0] as u32, dims[1] as u32));
    }
    app
}

fn output(
    app: &mut App,
    ctx: &egui::Context,
    rect: egui::Rect,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    app.frame_counter += 1;
    let output = crate::test_ui::headless_paint_output(
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(rect),
                time: Some(app.frame_counter as f64 / 60.0),
                focused: true,
                events: std::iter::once(egui::Event::ModifiersChanged(egui::Modifiers::NONE))
                    .chain(events)
                    .collect(),
                ..Default::default()
            },
            |ui| app.render_fullscreen_viewport(ui),
        ),
    );
    assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Closed);
    output
}

fn frame(app: &mut App, ctx: &egui::Context, rect: egui::Rect, events: Vec<egui::Event>) {
    output(app, ctx, rect, events).drop_without_applying_deltas();
}

fn settle(app: &mut App, ctx: &egui::Context, rect: egui::Rect) {
    for _ in 0..4 {
        frame(app, ctx, rect, vec![]);
    }
}

fn assert_page(app: &App) -> egui::Rect {
    let page = app
        .fullscreen_page_layout
        .single_page()
        .expect("the production renderer must publish the actual image page");
    assert_eq!(page.page_idx(), 1);
    let painted = page.transform.paint_rect;
    assert!(painted.is_positive(), "actual page is empty: {painted:?}");
    painted
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn click(app: &mut App, ctx: &egui::Context, rect: egui::Rect, pos: egui::Pos2) {
    frame(app, ctx, rect, vec![egui::Event::PointerMoved(pos)]);
    frame(app, ctx, rect, vec![button(pos, true)]);
    frame(app, ctx, rect, vec![button(pos, false)]);
}

fn open_menu(app: &mut App, ctx: &egui::Context, rect: egui::Rect, button_id: &str) {
    let pos = ctx
        .read_response(egui::Id::unique(button_id))
        .unwrap_or_else(|| panic!("the actual toolbar must draw {button_id}"))
        .rect
        .center();
    click(app, ctx, rect, pos);
    settle(app, ctx, rect);
    assert!(egui::Popup::is_id_open(
        ctx,
        egui::Id::unique(button_id).with("popup")
    ));
}

fn text_center(output: &egui::FullOutput, expected: &str) -> egui::Pos2 {
    fn find(shape: &egui::Shape, clip: egui::Rect, expected: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == expected => {
                let pos = text.galley.rect.translate(text.pos.to_vec2()).center();
                clip.contains(pos).then_some(pos)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, clip, expected)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|shape| find(&shape.shape, shape.clip_rect, expected))
        .unwrap_or_else(|| panic!("the actual foreground menu must visibly paint {expected:?}"))
}

fn menu_row(
    app: &mut App,
    ctx: &egui::Context,
    rect: egui::Rect,
    button_id: &str,
    label: &str,
) -> (egui::Pos2, egui::Rect) {
    let output = output(app, ctx, rect, vec![]);
    let pos = text_center(&output, label);
    output.drop_without_applying_deltas();
    let id = egui::Id::unique(button_id).with("popup");
    let area =
        egui::AreaState::load(ctx, id).expect("the production popup must register a real Area");
    let popup = area.rect();
    assert!(area.interactable && popup.is_positive());
    assert!(popup.contains(pos), "row={pos:?}, popup={popup:?}");
    assert_eq!(
        ctx.layer_id_at(pos),
        Some(egui::LayerId::new(egui::Order::Foreground, id)),
        "the selected menu row must actually be the frontmost input layer"
    );
    (pos, popup)
}

fn seek_row(app: &mut App, ctx: &egui::Context, rect: egui::Rect) -> (egui::Pos2, egui::Rect) {
    let preset = crate::settings::StillSeekStripHeight::Small;
    let label = crate::seek_strip_menu::SeekStripMenuLabels::for_viewport(rect).preset(
        preset.label(),
        app.settings.still_seek_strip_height_values.points(preset),
    );
    menu_row(
        app,
        ctx,
        rect,
        FULLSCREEN_STILL_SEEK_STRIP_BUTTON_ID,
        &label,
    )
}

fn select_seek_row_control(app: &mut App, ctx: &egui::Context, rect: egui::Rect) {
    open_menu(app, ctx, rect, FULLSCREEN_STILL_SEEK_STRIP_BUTTON_ID);
    let (pos, _) = seek_row(app, ctx, rect);
    assert_ne!(
        app.settings.still_seek_strip_height,
        crate::settings::StillSeekStripHeight::Small
    );
    click(app, ctx, rect, pos);
    assert_eq!(
        app.settings.still_seek_strip_height,
        crate::settings::StillSeekStripHeight::Small,
        "the actual foreground seek row must remain selectable"
    );
    assert!(!fs_still_seek_strip_popup_open(ctx));
    settle(app, ctx, rect);
}

fn drag_in_popup(
    app: &mut App,
    ctx: &egui::Context,
    rect: egui::Rect,
    popup: egui::Rect,
    start: egui::Pos2,
    delta: egui::Vec2,
) {
    let middle = start + delta * 0.5;
    let end = start + delta;
    assert!(popup.contains(start) && popup.contains(middle) && popup.contains(end));
    frame(
        app,
        ctx,
        rect,
        vec![egui::Event::PointerMoved(start), button(start, true)],
    );
    frame(app, ctx, rect, vec![egui::Event::PointerMoved(middle)]);
    frame(app, ctx, rect, vec![egui::Event::PointerMoved(end)]);
    frame(app, ctx, rect, vec![button(end, false)]);
}

#[test]
#[ignore = "backlog 1.402: 前面 popup の押下が背面へ届く (再現テスト)"]
fn front_popup_seek_row_does_not_pan_the_actual_top_right_navigator() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let rect = viewport(1093.0);
    let mut app = sources(&ctx, [80, 120]);
    app.settings.fullscreen_navigator_visible = true;
    app.settings.fullscreen_navigator_corner = FullscreenNavigatorCorner::TopRight;
    app.settings.fullscreen_navigator_size = 520.0;
    app.fs_zoom = 2.0;
    settle(&mut app, &ctx, rect);
    assert_page(&app);
    select_seek_row_control(&mut app, &ctx, rect);
    open_menu(&mut app, &ctx, rect, FULLSCREEN_STILL_SEEK_STRIP_BUTTON_ID);
    let (start, popup) = seek_row(&mut app, &ctx, rect);
    let media_rect = app.fullscreen_media_rect(rect, 1, false);
    let layout = app
        .fs_navigator_layout(media_rect)
        .expect("the real right-top navigator must exist");
    assert!(layout.content_rect.is_positive());
    assert!(
        layout.content_rect.contains(start),
        "row must overlap actual navigator image: row={start:?}, canvas={:?}, popup={popup:?}",
        layout.content_rect
    );
    let delta = egui::vec2(0.0, -36.0);
    assert!(layout.content_rect.contains(start + delta));
    let before = app.fs_pan;
    let zoom = app.fs_zoom;
    drag_in_popup(&mut app, &ctx, rect, popup, start, delta);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(app.fs_zoom, zoom);
    assert_eq!(
        app.fs_pan, before,
        "dragging the front seek menu must not pan its overlapping raw-input navigator"
    );
}

fn prepare_wipe(app: &mut App) {
    let pixels = Arc::new(egui::ColorImage::filled(
        [320, 160],
        egui::Color32::LIGHT_RED,
    ));
    app.pinned_compare_slot = Some(crate::app::PinnedCompareSlot {
        pixels: Arc::clone(&pixels),
        indicator_pixels: Arc::clone(&pixels),
        indicator_texture: None,
        display_name: "front popup comparison".into(),
        source_idx: 42,
        external_item_key: None,
        source_size: [320, 160],
    });
    app.compare_preparation =
        crate::app::ComparePreparationState::Ready(crate::app::ComparePreparedPair {
            key: 1,
            current_idx: 1,
            pinned_source_idx: 42,
            current_input_size: [320, 160],
            pinned_input_size: [320, 160],
            target_size: [320, 160],
            pixels: crate::app::ComparePreparedPixels::WipeTextures {
                pinned: Arc::clone(&pixels),
                current: pixels,
            },
            pinned_texture: None,
            current_texture: None,
            diff_texture: None,
        });
}

#[test]
#[ignore = "backlog 1.402: 前面 popup の押下が背面へ届く (再現テスト)"]
fn front_popup_seek_row_does_not_move_the_actual_comparison_wipe() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let rect = viewport(1093.0);
    let mut app = sources(&ctx, [320, 160]);
    settle(&mut app, &ctx, rect);
    let painted = assert_page(&app);
    assert!(painted.width() > painted.height() * 1.5);
    prepare_wipe(&mut app);
    app.compare_view_mode = crate::app::CompareViewMode::wipe(0.5);
    settle(&mut app, &ctx, rect);
    assert!(app.compare_prepared_pair_matches(1));
    // The positive control must run with the comparison actually visible too.
    select_seek_row_control(&mut app, &ctx, rect);
    open_menu(&mut app, &ctx, rect, FULLSCREEN_STILL_SEEK_STRIP_BUTTON_ID);
    let (start, popup) = seek_row(&mut app, &ctx, rect);
    let draw = App::compare_image_draw_rect(
        app.fullscreen_media_rect(rect, 1, false),
        [320, 160],
        app.fs_zoom_pan(),
        app.fullscreen_fit_scale_limits(ctx.pixels_per_point()),
    )
    .expect("the actual comparison must have a nonempty painted rectangle");
    assert!(
        draw.is_positive() && draw.contains(start),
        "draw={draw:?}, row={start:?}, popup={popup:?}"
    );
    let fraction = (start.x - draw.left()) / draw.width();
    app.compare_view_mode = crate::app::CompareViewMode::wipe(fraction);
    settle(&mut app, &ctx, rect);
    assert!(app.compare_prepared_pair_matches(1));
    let (start_after_prepare, _) = seek_row(&mut app, &ctx, rect);
    assert_eq!(start_after_prepare, start);
    let delta = egui::vec2(42.0, 0.0);
    assert!(draw.contains(start + delta));
    drag_in_popup(&mut app, &ctx, rect, popup, start, delta);
    let crate::app::CompareViewMode::Wipe {
        fraction: after, ..
    } = app.compare_view_mode
    else {
        panic!("the comparison must remain a wipe");
    };
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(
        after, fraction,
        "dragging the front seek menu must not move the overlapping raw-input wipe boundary"
    );
}

#[test]
#[ignore = "backlog 1.402: 前面 popup の押下が背面へ届く (再現テスト)"]
fn front_popup_rotation_row_does_not_pan_the_actual_analysis_canvas() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    // A narrow viewport puts the genuine rotation Area over the image side of
    // analysis, rather than over the analysis controls on the right.
    let rect = viewport(480.0);
    let mut app = sources(&ctx, [160, 320]);
    app.toggle_analysis_mode();
    settle(&mut app, &ctx, rect);
    assert!(app.analysis_mode);
    assert_page(&app);
    open_menu(&mut app, &ctx, rect, "fs_rotation_btn");
    let (choice, _) = menu_row(&mut app, &ctx, rect, "fs_rotation_btn", "180°");
    assert!(analysis_image_rect(rect).contains(choice));
    assert_ne!(app.get_rotation(1), crate::rotation_db::Rotation::Cw180);
    click(&mut app, &ctx, rect, choice);
    assert_eq!(
        app.get_rotation(1),
        crate::rotation_db::Rotation::Cw180,
        "the actual front rotation row must update the page rotation"
    );
    assert!(!fs_rotation_popup_open(&ctx));
    settle(&mut app, &ctx, rect);
    let painted = assert_page(&app);
    open_menu(&mut app, &ctx, rect, "fs_rotation_btn");
    let (start, popup) = menu_row(&mut app, &ctx, rect, "fs_rotation_btn", "180°");
    let canvas = analysis_image_rect(rect);
    assert!(
        canvas.contains(start),
        "rotation row must overlap actual analysis input canvas: row={start:?}, canvas={canvas:?}, popup={popup:?}, paint={painted:?}"
    );
    let delta = egui::vec2(0.0, 25.0);
    assert!(canvas.contains(start + delta));
    let before = app.analysis_pan;
    drag_in_popup(&mut app, &ctx, rect, popup, start, delta);
    assert!(app.analysis_mode);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(
        app.analysis_pan, before,
        "dragging the front rotation menu must not pan its overlapping raw-input analysis canvas"
    );
}
