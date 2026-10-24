//! Backlog 1.403: non-overflow foreground menus through the actual fullscreen renderer.
//! These tests keep the physical foreground stream separate from the painted canvas.
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
                pdf_rerender_failures: Vec::new(),
                pdf_metadata: None,
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
    output_in_viewport(app, ctx, rect, egui::ViewportId::ROOT, events)
}

fn output_in_viewport(
    app: &mut App,
    ctx: &egui::Context,
    rect: egui::Rect,
    viewport_id: egui::ViewportId,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    app.frame_counter += 1;
    let mut input = egui::RawInput {
        viewport_id,
        screen_rect: Some(rect),
        time: Some(app.frame_counter as f64 / 60.0),
        focused: true,
        events: std::iter::once(egui::Event::ModifiersChanged(egui::Modifiers::NONE))
            .chain(events)
            .collect(),
        ..Default::default()
    };
    input.viewports.entry(viewport_id).or_default().focused = Some(true);
    let output = crate::test_ui::headless_paint_output(
        ctx.run_ui(input, |ui| app.render_fullscreen_viewport(ui)),
    );
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

#[derive(Clone, Copy, Debug)]
enum Menu {
    Fit,
    Spread,
    Projection,
    Seek,
    Rotation,
    Context,
}

impl Menu {
    fn button_id(self) -> Option<&'static str> {
        match self {
            Self::Fit => Some("fs_margin_fit_btn"),
            Self::Spread => Some("fs_spread_btn"),
            Self::Projection => Some("fs_panorama_projection_btn"),
            Self::Seek => Some(FULLSCREEN_STILL_SEEK_STRIP_BUTTON_ID),
            Self::Rotation => Some("fs_rotation_btn"),
            Self::Context => None,
        }
    }

    fn is_open(self, app: &App, ctx: &egui::Context) -> bool {
        match self {
            Self::Fit => app.fit_popup_open,
            Self::Spread => app.spread_popup_open,
            Self::Projection => app.panorama_projection_popup_open,
            Self::Seek => fs_still_seek_strip_popup_open(ctx),
            Self::Rotation => fs_rotation_popup_open(ctx),
            Self::Context => app.fs_context_menu_idx.is_some(),
        }
    }

    fn assert_outside(self, ctx: &egui::Context, pos: egui::Pos2) {
        let popup = match self {
            Self::Fit => ctx
                .data(|data| {
                    data.get_temp::<(egui::Rect, egui::Rect, egui::Vec2)>(egui::Id::unique(
                        "fs_fit_popup_scroll_layout",
                    ))
                })
                .expect("the real fit popup must publish its laid-out body")
                .0
                .expand(16.0),
            Self::Spread => ctx
                .read_response(egui::Id::unique(format!(
                    "spread_popup_{}",
                    SpreadMode::Single.to_int(),
                )))
                .expect("the real spread popup must lay out its first row")
                .rect
                .expand(16.0),
            Self::Projection => ctx
                .read_response(egui::Id::unique((
                    "fs_panorama_projection_item",
                    crate::panorama::PanoProjection::Perspective.shader_code(),
                )))
                .expect("the real projection popup must lay out its first row")
                .rect
                .expand(16.0),
            Self::Seek | Self::Rotation => egui::AreaState::load(
                ctx,
                egui::Id::unique(self.button_id().unwrap()).with("popup"),
            )
            .unwrap()
            .rect(),
            Self::Context => egui::AreaState::load(ctx, egui::Id::unique("fs_ctx_menu"))
                .unwrap()
                .rect(),
        };
        assert!(popup.is_positive());
        // S/P have many rows below the first one; requiring the point to lie
        // strictly to the left proves it is outside every row and the header.
        assert!(
            pos.x < popup.left(),
            "{self:?}: outside point {pos:?}, real popup {popup:?}"
        );
    }

    fn open(self, app: &mut App, ctx: &egui::Context, rect: egui::Rect) {
        if let Some(id) = self.button_id() {
            let button = ctx
                .read_response(egui::Id::unique(id))
                .unwrap_or_else(|| panic!("production toolbar must draw {id}"))
                .rect;
            assert!(button.is_positive());
            click(app, ctx, rect, button.center());
        } else {
            // No HWND is supplied by setup_app_for_test, so this draws the real
            // egui fallback and never invokes the native Shell menu.
            assert!(app.main_hwnd.is_none());
            app.fs_context_menu_idx = app.fullscreen_idx;
            app.fs_context_menu_pos = egui::pos2(650.0, 100.0);
        }
        settle(app, ctx, rect);
        assert!(self.is_open(app, ctx), "{self:?} must really open");
        let paint = output(app, ctx, rect, vec![]);
        let label = match self {
            Self::Fit => FullscreenFitMode::Page.label(),
            Self::Spread => SpreadMode::Single.label(),
            Self::Projection => crate::panorama::PanoProjection::Perspective.label(),
            Self::Seek => {
                paint.drop_without_applying_deltas();
                seek_row(app, ctx, rect);
                return;
            }
            Self::Rotation => "180°",
            Self::Context => {
                paint.drop_without_applying_deltas();
                let area = egui::AreaState::load(ctx, egui::Id::unique("fs_ctx_menu"))
                    .expect("the actual context menu Window must be laid out");
                assert!(area.rect().is_positive() && area.interactable);
                return;
            }
        };
        let painted_row = text_center(&paint, label);
        assert!(rect.contains(painted_row));
        paint.drop_without_applying_deltas();
    }
}

fn pointer_button(pos: egui::Pos2, which: egui::PointerButton, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: which,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn key(key: egui::Key, pressed: bool) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn outside_image_point(app: &App) -> egui::Pos2 {
    let page = app.fullscreen_page_layout.single_page().unwrap();
    let pos = egui::pos2(260.0, 330.0);
    assert!(
        page.transform.paint_rect.contains(pos),
        "the outside press must hit the actual painted page"
    );
    pos
}

fn view_state(app: &App) -> (Option<usize>, f32, egui::Vec2, f32, Option<(f32, f32, f32)>) {
    (
        app.fullscreen_idx,
        app.fs_zoom,
        app.fs_pan,
        app.fs_free_rotation,
        app.panorama_state
            .as_ref()
            .map(|p| (p.yaw, p.pitch, p.fov_y)),
    )
}

#[test]
fn front_popup_all_six_menus_close_on_outside_press_and_own_the_entire_drag() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for menu in [
        Menu::Fit,
        Menu::Spread,
        Menu::Projection,
        Menu::Seek,
        Menu::Rotation,
        Menu::Context,
    ] {
        for which in [
            egui::PointerButton::Primary,
            egui::PointerButton::Secondary,
            egui::PointerButton::Middle,
        ] {
            let ctx = egui::Context::default();
            let rect = viewport(1093.0);
            let mut app = sources(&ctx, [320, 160]);
            app.fs_zoom = 2.0;
            if matches!(menu, Menu::Projection) {
                app.panorama_state = Some(crate::panorama::PanoramaState::new(
                    0.3,
                    0.1,
                    crate::panorama::PanoProjection::Perspective,
                ));
            }
            settle(&mut app, &ctx, rect);
            if matches!(menu, Menu::Projection) {
                assert!(app.is_panorama_mode_active(1));
            } else {
                assert_page(&app);
            }
            menu.open(&mut app, &ctx, rect);
            let start = if matches!(menu, Menu::Projection) {
                egui::pos2(260.0, 330.0)
            } else {
                outside_image_point(&app)
            };
            // All toolbar popups are anchored on the right at this viewport;
            // verify the chosen outside point cannot be the launch button.
            if let Some(id) = menu.button_id() {
                assert!(
                    !ctx.read_response(egui::Id::unique(id))
                        .unwrap()
                        .rect
                        .contains(start)
                );
            }
            menu.assert_outside(&ctx, start);
            let before = view_state(&app);
            frame(
                &mut app,
                &ctx,
                rect,
                vec![
                    egui::Event::PointerMoved(start),
                    pointer_button(start, which, true),
                ],
            );
            assert!(
                !menu.is_open(&app, &ctx),
                "{menu:?} must close on {which:?} press, before release"
            );
            for delta in [egui::vec2(35.0, 20.0), egui::vec2(70.0, 45.0)] {
                frame(
                    &mut app,
                    &ctx,
                    rect,
                    vec![egui::Event::PointerMoved(start + delta)],
                );
                assert_eq!(
                    view_state(&app),
                    before,
                    "{menu:?} {which:?}: held foreground input reached the real canvas"
                );
            }
            frame(
                &mut app,
                &ctx,
                rect,
                vec![pointer_button(start + egui::vec2(70.0, 45.0), which, false)],
            );
            assert_eq!(
                view_state(&app),
                before,
                "{menu:?} {which:?}: foreground release reached the canvas"
            );
            assert!(app.mouse_ring_flick.is_none() && app.mouse_gesture.is_none());
        }
    }
}

#[test]
fn front_popup_launch_button_closes_once_without_reopening_on_release() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for menu in [
        Menu::Fit,
        Menu::Spread,
        Menu::Projection,
        Menu::Seek,
        Menu::Rotation,
    ] {
        let ctx = egui::Context::default();
        let rect = viewport(1093.0);
        let mut app = sources(&ctx, [320, 160]);
        if matches!(menu, Menu::Projection) {
            app.panorama_state = Some(crate::panorama::PanoramaState::new(
                0.0,
                0.0,
                crate::panorama::PanoProjection::Perspective,
            ));
        }
        settle(&mut app, &ctx, rect);
        menu.open(&mut app, &ctx, rect);
        let pos = ctx
            .read_response(egui::Id::unique(menu.button_id().unwrap()))
            .unwrap()
            .rect
            .center();
        frame(
            &mut app,
            &ctx,
            rect,
            vec![egui::Event::PointerMoved(pos), button(pos, true)],
        );
        frame(&mut app, &ctx, rect, vec![button(pos, false)]);
        settle(&mut app, &ctx, rect);
        assert!(
            !menu.is_open(&app, &ctx),
            "{menu:?}: launch button must toggle once"
        );
        assert_eq!(app.fullscreen_idx, Some(1));
    }
}

#[test]
fn front_popup_nonpresented_toolbar_bools_do_not_block_the_real_erase_brush() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let rect = viewport(1093.0);
    let mut app = sources(&ctx, [320, 160]);
    settle(&mut app, &ctx, rect);
    Menu::Fit.open(&mut app, &ctx, rect);
    // Use the production E shortcut; it leaves the toolbar bool behind while
    // removing the toolbar from the rendered edit canvas.
    frame(&mut app, &ctx, rect, vec![key(egui::Key::E, true)]);
    frame(&mut app, &ctx, rect, vec![key(egui::Key::E, false)]);
    settle(&mut app, &ctx, rect);
    assert!(app.erase_mode, "the real E entry must reach erase mode");
    assert!(
        app.fit_popup_open,
        "fixture must retain the nonpresented open bool"
    );
    let page = assert_page(&app);
    let start = page.center();
    assert!(!app.erase_panel_rect(rect).contains(start));
    let before = app.erase_mask.clone();
    frame(
        &mut app,
        &ctx,
        rect,
        vec![egui::Event::PointerMoved(start), button(start, true)],
    );
    frame(
        &mut app,
        &ctx,
        rect,
        vec![egui::Event::PointerMoved(start + egui::vec2(20.0, 0.0))],
    );
    frame(
        &mut app,
        &ctx,
        rect,
        vec![button(start + egui::vec2(20.0, 0.0), false)],
    );
    assert_ne!(
        app.erase_mask, before,
        "a hidden toolbar bool must not own a new edit stroke"
    );
}

#[test]
fn front_popup_and_overflow_hold_survive_real_arrow_page_navigation() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for overflow in [false, true] {
        for which in [
            egui::PointerButton::Primary,
            egui::PointerButton::Secondary,
            egui::PointerButton::Middle,
        ] {
            if overflow && which == egui::PointerButton::Middle {
                // Overflow preserves the released middle-button canvas behavior;
                // only menus own middle input until its physical release.
                continue;
            }
            let ctx = egui::Context::default();
            let rect = viewport(1093.0);
            let mut app = sources(&ctx, [320, 160]);
            app.fs_zoom = 2.0;
            settle(&mut app, &ctx, rect);
            let start = if overflow {
                app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
                settle(&mut app, &ctx, rect);
                let panel = fs_overflow_panel_rect(rect, app.fs_overflow_panel_state).unwrap();
                // The frame margin owns input without focusing a Slider whose
                // ArrowRight handling would replace the page-navigation control.
                panel.min + egui::vec2(3.0, 3.0)
            } else {
                Menu::Fit.open(&mut app, &ctx, rect);
                outside_image_point(&app)
            };
            frame(
                &mut app,
                &ctx,
                rect,
                vec![
                    egui::Event::PointerMoved(start),
                    pointer_button(start, which, true),
                ],
            );
            assert!(app.fs_front_pointer.is_active());
            frame(&mut app, &ctx, rect, vec![key(egui::Key::ArrowRight, true)]);
            frame(
                &mut app,
                &ctx,
                rect,
                vec![key(egui::Key::ArrowRight, false)],
            );
            assert_ne!(
                app.fullscreen_idx,
                Some(1),
                "fixture must really navigate while held (overflow={overflow}, {which:?})"
            );
            let page = app
                .fullscreen_page_layout
                .single_page()
                .expect("the next page must actually paint");
            assert_eq!(Some(page.page_idx()), app.fullscreen_idx);
            assert!(
                app.fs_front_pointer.is_active(),
                "page navigation must preserve the {which:?} physical owner"
            );
            // Navigation normally resets the transform. Give the newly painted
            // page a usable pan range to expose a leaked held primary button.
            app.fs_zoom = 2.0;
            frame(&mut app, &ctx, rect, vec![]);
            let before = view_state(&app);
            let end = egui::pos2(360.0, 370.0);
            frame(&mut app, &ctx, rect, vec![egui::Event::PointerMoved(end)]);
            assert_eq!(
                view_state(&app),
                before,
                "page replacement must not release physical ownership (overflow={overflow}, {which:?})"
            );
            assert!(app.mouse_ring_flick.is_none() && app.mouse_gesture.is_none());
            frame(
                &mut app,
                &ctx,
                rect,
                vec![pointer_button(end, which, false)],
            );
            assert_eq!(view_state(&app), before);
            assert!(
                !app.fs_front_pointer.is_active(),
                "the real {which:?} release must finish the physical stream"
            );
        }
    }
}

#[test]
fn front_popup_dismissal_click_cannot_be_the_first_click_of_a_canvas_reset() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for expire_launcher_click in [false, true] {
        let ctx = egui::Context::default();
        let rect = viewport(1093.0);
        let mut app = sources(&ctx, [320, 160]);
        app.fs_zoom = 2.0;
        app.fs_pan = egui::vec2(30.0, 15.0);
        settle(&mut app, &ctx, rect);
        Menu::Fit.open(&mut app, &ctx, rect);
        if expire_launcher_click {
            for _ in 0..40 {
                frame(&mut app, &ctx, rect, vec![]);
            }
        }
        let pos = outside_image_point(&app);
        let before = (app.fs_zoom, app.fs_pan);
        click(&mut app, &ctx, rect, pos);
        assert!(!app.fit_popup_open);
        click(&mut app, &ctx, rect, pos);
        assert_eq!(
            (app.fs_zoom, app.fs_pan),
            before,
            "egui's second click straddles the foreground/canvas boundary"
        );
        assert!(
            ctx.input(|input| input
                .pointer
                .released_buttons_with_click_counts()
                .any(|(button, click)| button == egui::PointerButton::Primary
                    && click.is_some_and(|(_, count)| count >= 2))),
            "the boundary test must exercise egui's actual multi-click count"
        );
        if expire_launcher_click {
            assert!(ctx.input(|input| {
                input
                    .pointer
                    .button_double_clicked(egui::PointerButton::Primary)
            }));
        }
        // The next click completes the first pair accepted by the canvas, even
        // when egui counts the earlier menu click and reports a triple click.
        click(&mut app, &ctx, rect, pos);
        assert_eq!(
            app.fs_zoom, 1.0,
            "two canvas clicks immediately after dismissal must reset"
        );
        assert_eq!(app.fs_pan, egui::Vec2::ZERO);
        // Keep the normal, unbroken canvas double-click behavior as a second control.
        app.fs_zoom = 2.0;
        app.fs_pan = egui::vec2(30.0, 15.0);
        for _ in 0..40 {
            frame(&mut app, &ctx, rect, vec![]);
        }
        click(&mut app, &ctx, rect, pos);
        click(&mut app, &ctx, rect, pos);
        assert_eq!(app.fs_zoom, 1.0);
        assert_eq!(app.fs_pan, egui::Vec2::ZERO);
    }
}

fn combo_frame(
    app: &mut App,
    ctx: &egui::Context,
    rect: egui::Rect,
    viewport_id: egui::ViewportId,
    events: Vec<egui::Event>,
) -> egui::Id {
    app.frame_counter += 1;
    let mut input = egui::RawInput {
        viewport_id,
        screen_rect: Some(rect),
        time: Some(app.frame_counter as f64 / 60.0),
        focused: true,
        events,
        ..Default::default()
    };
    input.viewports.entry(viewport_id).or_default().focused = Some(true);
    let mut widget_id = None;
    crate::test_ui::headless_paint_output(ctx.run_ui(input, |ui| {
        app.render_fullscreen_viewport(ui);
        // Exercise egui's real panel-style ComboBox owner, including its
        // Response and Popup Area, alongside the actual fullscreen renderer.
        egui::Area::new(egui::Id::unique("front_popup_combo_panel"))
            .fixed_pos(egui::pos2(780.0, 180.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                let combo = egui::ComboBox::from_id_salt("front_popup_combo")
                    .selected_text("test panel choice")
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(false, "panel memory popup row")
                            .clicked()
                        {
                            ui.close();
                        }
                    });
                widget_id = Some(combo.response.id);
            });
    }))
    .drop_without_applying_deltas();
    widget_id.unwrap()
}

fn open_combo(
    app: &mut App,
    ctx: &egui::Context,
    rect: egui::Rect,
    viewport_id: egui::ViewportId,
) -> egui::Id {
    let mut widget = combo_frame(app, ctx, rect, viewport_id, vec![]);
    for _ in 0..3 {
        widget = combo_frame(app, ctx, rect, viewport_id, vec![]);
    }
    let pos = ctx.read_response(widget).unwrap().rect.center();
    combo_frame(
        app,
        ctx,
        rect,
        viewport_id,
        vec![egui::Event::PointerMoved(pos)],
    );
    combo_frame(app, ctx, rect, viewport_id, vec![button(pos, true)]);
    combo_frame(app, ctx, rect, viewport_id, vec![button(pos, false)]);
    for _ in 0..3 {
        combo_frame(app, ctx, rect, viewport_id, vec![]);
    }
    let popup = widget.with("popup");
    assert!(egui::Popup::is_id_open(ctx, popup));
    let area =
        egui::AreaState::load(ctx, popup).expect("ComboBox must lay out its actual memory popup");
    assert!(area.rect().is_positive() && area.interactable);
    assert_eq!(
        ctx.layer_id_at(area.rect().center()),
        Some(egui::LayerId::new(egui::Order::Foreground, popup))
    );
    popup
}

#[test]
fn front_popup_memory_combo_preserves_outside_drag_then_closes_on_outside_click() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let rect = viewport(1093.0);
    let mut app = sources(&ctx, [320, 160]);
    app.fs_zoom = 2.0;
    settle(&mut app, &ctx, rect);
    assert_page(&app);
    let popup = open_combo(&mut app, &ctx, rect, egui::ViewportId::ROOT);
    let start = outside_image_point(&app);
    assert!(
        !egui::AreaState::load(&ctx, popup)
            .unwrap()
            .rect()
            .contains(start)
    );
    let end = start + egui::vec2(70.0, 40.0);
    let before = view_state(&app);
    for events in [
        vec![egui::Event::PointerMoved(start), button(start, true)],
        vec![egui::Event::PointerMoved(end)],
        vec![button(end, false)],
    ] {
        combo_frame(&mut app, &ctx, rect, egui::ViewportId::ROOT, events);
        assert!(
            egui::Popup::is_id_open(&ctx, popup),
            "egui's memory popup must preserve its click-based outside-close contract"
        );
        assert_eq!(
            view_state(&app),
            before,
            "outside drag belongs to the memory popup through release"
        );
    }
    for events in [
        vec![egui::Event::PointerMoved(start)],
        vec![button(start, true)],
        vec![button(start, false)],
    ] {
        combo_frame(&mut app, &ctx, rect, egui::ViewportId::ROOT, events);
        assert_eq!(view_state(&app), before);
    }
    assert!(!egui::Popup::is_id_open(&ctx, popup));
    // Closing the popup must not leave a permanent canvas suppression.
    frame(
        &mut app,
        &ctx,
        rect,
        vec![egui::Event::PointerMoved(start), button(start, true)],
    );
    frame(&mut app, &ctx, rect, vec![egui::Event::PointerMoved(end)]);
    assert_ne!(
        app.fs_pan, before.2,
        "a fresh canvas drag must still work after popup release"
    );
    frame(&mut app, &ctx, rect, vec![button(end, false)]);
}

#[test]
fn front_popup_memory_close_is_local_to_the_rendered_viewport() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let rect = viewport(1093.0);
    let mut root = sources(&ctx, [320, 160]);
    let root_popup = open_combo(&mut root, &ctx, rect, egui::ViewportId::ROOT);
    let child_id = egui::ViewportId::from_hash_of("front-popup-child");
    // Model the existing foreground handoff with one guarded App environment.
    // A second setup_app_for_test would recursively acquire its process-global DB lock.
    root.fs_front_pointer = Default::default();
    let child_popup = open_combo(&mut root, &ctx, rect, child_id);
    let outside = egui::pos2(260.0, 330.0);
    for events in [
        vec![egui::Event::PointerMoved(outside)],
        vec![button(outside, true)],
        vec![button(outside, false)],
    ] {
        combo_frame(&mut root, &ctx, rect, child_id, events);
    }
    assert!(!egui::Popup::is_id_open(&ctx, child_popup));
    root.fs_front_pointer = Default::default();
    combo_frame(&mut root, &ctx, rect, egui::ViewportId::ROOT, vec![]);
    assert!(
        egui::Popup::is_id_open(&ctx, root_popup),
        "closing a child viewport's menu must not call global Popup::close_all"
    );
    assert_eq!(root.fullscreen_idx, Some(1));
}

#[test]
fn front_popup_actual_seek_press_close_keeps_the_other_viewports_menu_open() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let rect = viewport(1093.0);
    let mut app = sources(&ctx, [320, 160]);
    settle(&mut app, &ctx, rect);
    Menu::Seek.open(&mut app, &ctx, rect);
    let child = egui::ViewportId::from_hash_of("front-seek-menu-child");
    app.fs_front_pointer = Default::default();
    for _ in 0..4 {
        combo_frame(&mut app, &ctx, rect, child, vec![]);
    }
    let launcher = ctx
        .read_response(egui::Id::unique(FULLSCREEN_STILL_SEEK_STRIP_BUTTON_ID))
        .expect("the child viewer must draw its real seek launcher")
        .rect
        .center();
    for events in [
        vec![egui::Event::PointerMoved(launcher)],
        vec![button(launcher, true)],
        vec![button(launcher, false)],
    ] {
        combo_frame(&mut app, &ctx, rect, child, events);
    }
    for _ in 0..3 {
        combo_frame(&mut app, &ctx, rect, child, vec![]);
    }
    assert!(fs_still_seek_strip_popup_open(&ctx));
    let outside = egui::pos2(260.0, 330.0);
    combo_frame(
        &mut app,
        &ctx,
        rect,
        child,
        vec![egui::Event::PointerMoved(outside), button(outside, true)],
    );
    assert!(
        !fs_still_seek_strip_popup_open(&ctx),
        "the child menu must close on press"
    );
    assert!(app.fs_front_pointer.is_active());
    combo_frame(&mut app, &ctx, rect, child, vec![button(outside, false)]);
    app.fs_front_pointer = Default::default();
    combo_frame(&mut app, &ctx, rect, egui::ViewportId::ROOT, vec![]);
    assert!(
        fs_still_seek_strip_popup_open(&ctx),
        "the real child seek close must preserve the root menu"
    );
    assert_eq!(app.fullscreen_idx, Some(1));
}

fn contact(id: u64, phase: egui::TouchPhase, pos: egui::Pos2) -> egui::Event {
    egui::Event::Touch {
        device_id: egui::TouchDeviceId(84),
        id: egui::TouchId(id),
        phase,
        pos,
        force: None,
    }
}

#[test]
fn front_popup_touch_ownership_lasts_until_the_last_contact_end_or_cancel() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for last_phase in [egui::TouchPhase::End, egui::TouchPhase::Cancel] {
        let ctx = egui::Context::default();
        let rect = viewport(1093.0);
        let mut app = sources(&ctx, [320, 160]);
        app.fs_zoom = 2.0;
        settle(&mut app, &ctx, rect);
        Menu::Fit.open(&mut app, &ctx, rect);
        let start = outside_image_point(&app);
        let second = start + egui::vec2(25.0, 0.0);
        let before = view_state(&app);
        frame(
            &mut app,
            &ctx,
            rect,
            vec![
                contact(1, egui::TouchPhase::Start, start),
                egui::Event::PointerMoved(start),
                button(start, true),
            ],
        );
        assert!(!app.fit_popup_open);
        assert!(app.fs_front_pointer.is_active());
        frame(
            &mut app,
            &ctx,
            rect,
            vec![contact(2, egui::TouchPhase::Start, second)],
        );
        frame(
            &mut app,
            &ctx,
            rect,
            vec![
                contact(1, egui::TouchPhase::End, start),
                button(start, false),
            ],
        );
        assert!(
            app.fs_front_pointer.is_active(),
            "ending the first/promoted finger cannot release the remaining contact"
        );
        let end = second + egui::vec2(65.0, 35.0);
        frame(
            &mut app,
            &ctx,
            rect,
            vec![
                contact(2, egui::TouchPhase::Move, end),
                egui::Event::PointerMoved(end),
            ],
        );
        assert_eq!(view_state(&app), before);
        frame(
            &mut app,
            &ctx,
            rect,
            vec![contact(2, last_phase, end), egui::Event::PointerGone],
        );
        frame(&mut app, &ctx, rect, vec![]);
        assert!(
            !app.fs_front_pointer.is_active(),
            "the final {last_phase:?} must end the physical stream"
        );
        assert_eq!(view_state(&app), before);
        frame(
            &mut app,
            &ctx,
            rect,
            vec![egui::Event::PointerMoved(start), button(start, true)],
        );
        frame(
            &mut app,
            &ctx,
            rect,
            vec![egui::Event::PointerMoved(start + egui::vec2(40.0, 30.0))],
        );
        assert_ne!(
            app.fs_pan, before.2,
            "a new pointer stream must reach the real canvas after the last finger ends"
        );
        frame(
            &mut app,
            &ctx,
            rect,
            vec![button(start + egui::vec2(40.0, 30.0), false)],
        );
    }
}

#[test]
fn front_popup_touch_cancel_then_new_contact_in_one_pass_starts_a_new_canvas_stream() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let rect = viewport(1093.0);
    let mut app = sources(&ctx, [320, 160]);
    app.fs_zoom = 2.0;
    settle(&mut app, &ctx, rect);
    Menu::Fit.open(&mut app, &ctx, rect);
    let start = outside_image_point(&app);
    frame(
        &mut app,
        &ctx,
        rect,
        vec![
            contact(1, egui::TouchPhase::Start, start),
            egui::Event::PointerMoved(start),
            button(start, true),
        ],
    );
    assert!(!app.fit_popup_open);
    assert!(app.fs_front_pointer.is_active());
    let before = view_state(&app);
    let next = start + egui::vec2(10.0, 10.0);
    // Match winit's cancellation contract: there is no promoted primary
    // release. The following Start belongs to a new physical contact group.
    frame(
        &mut app,
        &ctx,
        rect,
        vec![
            contact(1, egui::TouchPhase::Cancel, start),
            egui::Event::PointerGone,
            contact(2, egui::TouchPhase::Start, next),
            egui::Event::PointerMoved(next),
            button(next, true),
        ],
    );
    assert!(
        !app.fs_front_pointer.is_active(),
        "the canceled foreground group must not own the following canvas contact"
    );
    assert_eq!(view_state(&app), before);
    let end = next + egui::vec2(60.0, 35.0);
    frame(
        &mut app,
        &ctx,
        rect,
        vec![
            contact(2, egui::TouchPhase::Move, end),
            egui::Event::PointerMoved(end),
        ],
    );
    assert_ne!(
        app.fs_pan, before.2,
        "the new contact must pan the actual painted page"
    );
    frame(
        &mut app,
        &ctx,
        rect,
        vec![
            contact(2, egui::TouchPhase::End, end),
            button(end, false),
            egui::Event::PointerGone,
        ],
    );
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(!app.fs_front_pointer.is_active());
}

#[test]
fn front_popup_actual_context_submenu_child_press_keeps_its_parent_open() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    // The fallback is a Window with inline CollapsingHeader children. A tall
    // viewport keeps the genuine OpenWith header and its child fully visible.
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1093.0, 1600.0));
    let mut app = sources(&ctx, [320, 160]);
    app.fs_zoom = 2.0;
    settle(&mut app, &ctx, rect);
    assert_page(&app);
    Menu::Context.open(&mut app, &ctx, rect);
    let paint = output(&mut app, &ctx, rect, vec![]);
    let header = text_center(&paint, "アプリケーションで開く…");
    paint.drop_without_applying_deltas();
    click(&mut app, &ctx, rect, header);
    for _ in 0..20 {
        frame(&mut app, &ctx, rect, vec![]);
    }
    assert!(Menu::Context.is_open(&app, &ctx));
    let paint = output(&mut app, &ctx, rect, vec![]);
    let child = text_center(&paint, "外部ツールの設定…");
    paint.drop_without_applying_deltas();
    let parent = egui::AreaState::load(&ctx, egui::Id::unique("fs_ctx_menu")).unwrap();
    assert!(parent.rect().is_positive() && parent.rect().contains(child));
    assert_eq!(
        ctx.layer_id_at(child),
        Some(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::unique("fs_ctx_menu")
        ))
    );
    let before = view_state(&app);
    frame(
        &mut app,
        &ctx,
        rect,
        vec![egui::Event::PointerMoved(child), button(child, true)],
    );
    assert!(
        Menu::Context.is_open(&app, &ctx),
        "an actual child row press is inside the parent menu"
    );
    assert_eq!(view_state(&app), before);
    // Move before release so the child command is not dispatched. This also
    // verifies the parent's inside-start stream stays owned outside its rect.
    let outside = egui::pos2(260.0, 330.0);
    assert!(!parent.rect().contains(outside));
    frame(
        &mut app,
        &ctx,
        rect,
        vec![egui::Event::PointerMoved(outside)],
    );
    frame(&mut app, &ctx, rect, vec![button(outside, false)]);
    assert!(Menu::Context.is_open(&app, &ctx));
    assert_eq!(view_state(&app), before);
    assert!(!app.fs_front_pointer.is_active());
}

#[test]
fn front_popup_actual_fit_unit_combo_child_press_keeps_its_parent_open() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1093.0, 1200.0));
    let mut app = sources(&ctx, [320, 160]);
    app.settings.fullscreen_specified_scale = Default::default();
    settle(&mut app, &ctx, rect);
    assert_page(&app);
    Menu::Fit.open(&mut app, &ctx, rect);
    assert!(
        app.settings
            .common_fullscreen_specified_scale()
            .width
            .enabled
    );
    let unit = ctx
        .data(|data| {
            data.get_temp::<egui::Rect>(egui::Id::unique(("specified_scale_unit_response", "幅")))
        })
        .expect("the actual fit editor must expose its unit ComboBox response");
    assert!(unit.is_positive() && rect.contains_rect(unit));
    click(&mut app, &ctx, rect, unit.center());
    settle(&mut app, &ctx, rect);
    assert!(app.fit_popup_open && egui::Popup::is_any_open(&ctx));
    let paint = output(&mut app, &ctx, rect, vec![]);
    let choice = text_center(&paint, "px");
    paint.drop_without_applying_deltas();
    let layer = ctx
        .layer_id_at(choice)
        .expect("the actual unit popup must own the selected row");
    assert_eq!(layer.order, egui::Order::Foreground);
    let area = egui::AreaState::load(&ctx, layer.id)
        .expect("the unit ComboBox must draw a real Popup Area");
    assert!(area.interactable && area.rect().is_positive() && area.rect().contains(choice));
    let before = view_state(&app);
    frame(
        &mut app,
        &ctx,
        rect,
        vec![egui::Event::PointerMoved(choice), button(choice, true)],
    );
    assert!(
        app.fit_popup_open,
        "a real child popup row press must not dismiss the fit parent"
    );
    assert_eq!(view_state(&app), before);
    frame(&mut app, &ctx, rect, vec![button(choice, false)]);
    assert!(
        app.fit_popup_open,
        "selecting a unit keeps the fit editor open"
    );
    assert!(!egui::Popup::is_id_open(&ctx, layer.id));
    assert!(
        matches!(
            app.settings
                .common_fullscreen_specified_scale()
                .width
                .active(),
            Some(crate::settings::SpecifiedScaleAxis::LogicalPixels(_)),
        ),
        "the actual child row must still change the width unit"
    );
    assert_eq!(view_state(&app), before);
}

#[test]
fn front_popup_owned_release_and_new_canvas_press_in_one_pass_keep_their_own_edges() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for drag_new_stream in [false, true] {
        let ctx = egui::Context::default();
        let rect = viewport(1093.0);
        let mut app = sources(&ctx, [320, 160]);
        app.fs_zoom = 2.0;
        app.fs_pan = egui::vec2(30.0, 15.0);
        settle(&mut app, &ctx, rect);
        assert_page(&app);
        Menu::Fit.open(&mut app, &ctx, rect);
        let pos = outside_image_point(&app);
        Menu::Fit.assert_outside(&ctx, pos);
        let before = view_state(&app);
        frame(
            &mut app,
            &ctx,
            rect,
            vec![egui::Event::PointerMoved(pos), button(pos, true)],
        );
        assert!(!app.fit_popup_open);
        assert!(app.fs_front_pointer.is_active());
        assert_eq!(view_state(&app), before);
        // egui exposes the old release/click and the new held press together.
        // Ownership must switch for the new stream without making the old
        // foreground terminal edge into a click accepted by the canvas.
        frame(
            &mut app,
            &ctx,
            rect,
            vec![button(pos, false), button(pos, true)],
        );
        assert!(!app.fs_front_pointer.is_active());
        assert_eq!(
            view_state(&app),
            before,
            "the old owned release cannot reset the canvas"
        );
        let release = if drag_new_stream {
            let end = pos + egui::vec2(60.0, 35.0);
            frame(&mut app, &ctx, rect, vec![egui::Event::PointerMoved(end)]);
            assert_ne!(
                app.fs_pan, before.2,
                "the new press in the mixed pass must start a real canvas drag"
            );
            assert_eq!(app.fs_zoom, before.1);
            end
        } else {
            pos
        };
        frame(&mut app, &ctx, rect, vec![button(release, false)]);
        assert_eq!(app.fullscreen_idx, Some(1));
        assert_eq!(
            app.fs_zoom, before.1,
            "the new click cannot double-click with the old owned release"
        );
        if !drag_new_stream {
            assert_eq!(view_state(&app), before);
            let end = pos + egui::vec2(60.0, 35.0);
            frame(&mut app, &ctx, rect, vec![button(pos, true)]);
            frame(&mut app, &ctx, rect, vec![egui::Event::PointerMoved(end)]);
            assert_ne!(
                app.fs_pan, before.2,
                "a following fresh canvas drag must also remain usable"
            );
            frame(&mut app, &ctx, rect, vec![button(end, false)]);
        }
        assert!(!app.fs_front_pointer.is_active());
    }
}

#[cfg(windows)]
#[test]
fn front_popup_actual_f11_round_trip_retires_the_old_viewport_pointer_owner() {
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let rect = viewport(1093.0);
    let mut app = sources(&ctx, [320, 160]);
    app.settings.video_in_window_mode = false;
    app.native_video_in_window_active = false;
    app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
    app.fs_zoom = 2.0;
    let full = app.fullscreen_viewport_id();
    assert_ne!(full, egui::ViewportId::ROOT);
    // Supply actual nested immediate viewport passes, rather than egui's
    // embedded Window fallback, with input local to the destination viewport.
    let child_input = std::rc::Rc::new(std::cell::RefCell::new((0.0, Vec::<egui::Event>::new())));
    let child_launcher = std::rc::Rc::new(std::cell::Cell::new(None));
    let input = child_input.clone();
    let launcher = child_launcher.clone();
    ctx.set_embed_viewports(false);
    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
        let (time, events) = input.borrow().clone();
        let mut raw = egui::RawInput {
            viewport_id: viewport.ids.this,
            screen_rect: Some(rect),
            time: Some(time),
            focused: true,
            events,
            ..Default::default()
        };
        raw.viewports.insert(
            viewport.ids.this,
            egui::ViewportInfo {
                parent: Some(viewport.ids.parent),
                focused: Some(true),
                ..Default::default()
            },
        );
        crate::test_ui::headless_paint_output(ctx.run_ui(raw, |ui| {
            (viewport.viewport_ui_cb)(ui);
            launcher.set(
                ctx.read_response(egui::Id::unique("fs_margin_fit_btn"))
                    .map(|response| response.rect.center()),
            );
        }))
        .drop_without_applying_deltas();
    });
    let paint = |app: &mut App, viewport_id, events: Vec<egui::Event>| {
        assert_eq!(
            viewport_id == egui::ViewportId::ROOT,
            app.native_video_in_window_active
        );
        let root_events = if viewport_id == egui::ViewportId::ROOT {
            *child_input.borrow_mut() = ((app.frame_counter + 1) as f64 / 60.0, Vec::new());
            events
        } else {
            assert_eq!(viewport_id, app.fullscreen_viewport_id());
            *child_input.borrow_mut() = ((app.frame_counter + 1) as f64 / 60.0, events);
            Vec::new()
        };
        frame(app, &ctx, rect, root_events);
    };
    for _ in 0..4 {
        paint(&mut app, full, vec![]);
    }
    assert_page(&app);
    let launcher = child_launcher
        .get()
        .expect("actual fullscreen child must draw its fit launcher");
    for events in [
        vec![egui::Event::PointerMoved(launcher)],
        vec![button(launcher, true)],
        vec![button(launcher, false)],
    ] {
        paint(&mut app, full, events);
    }
    for _ in 0..4 {
        paint(&mut app, full, vec![]);
    }
    assert!(app.fit_popup_open);
    let old = outside_image_point(&app);
    paint(
        &mut app,
        full,
        vec![egui::Event::PointerMoved(old), button(old, true)],
    );
    assert!(app.fs_front_pointer.is_active());
    // Dispatch the actual F11 binding, and intentionally never send the old
    // fullscreen viewport a release while the renderer transfers to ROOT.
    paint(&mut app, full, vec![key(egui::Key::F11, true)]);
    assert!(app.settings.video_in_window_mode);
    assert_eq!(
        app.viewer_presentation,
        crate::app::ViewerPresentation::MainWindow
    );
    assert!(
        !app.fs_front_pointer.is_active(),
        "F11 must retire the old viewport owner"
    );
    settle(&mut app, &ctx, rect);
    let start = egui::pos2(280.0, 340.0);
    let end = start + egui::vec2(45.0, 30.0);
    let before = app.fs_pan;
    for events in [
        vec![egui::Event::PointerMoved(start), button(start, true)],
        vec![egui::Event::PointerMoved(end)],
        vec![button(end, false)],
    ] {
        frame(&mut app, &ctx, rect, events);
    }
    assert_ne!(
        app.fs_pan, before,
        "the new ROOT viewport must accept a fresh drag"
    );
    Menu::Fit.open(&mut app, &ctx, rect);
    frame(
        &mut app,
        &ctx,
        rect,
        vec![egui::Event::PointerMoved(old), button(old, true)],
    );
    assert!(app.fs_front_pointer.is_active());
    frame(&mut app, &ctx, rect, vec![key(egui::Key::F11, true)]);
    assert!(!app.settings.video_in_window_mode);
    assert_eq!(
        app.viewer_presentation,
        crate::app::ViewerPresentation::Fullscreen
    );
    assert!(
        !app.fs_front_pointer.is_active(),
        "return transfer must retire ROOT ownership too"
    );
    paint(&mut app, full, vec![key(egui::Key::F11, false)]);
    let before = app.fs_pan;
    paint(
        &mut app,
        full,
        vec![egui::Event::PointerMoved(start), button(start, true)],
    );
    paint(&mut app, full, vec![egui::Event::PointerMoved(end)]);
    assert_ne!(
        app.fs_pan, before,
        "fullscreen to ROOT to fullscreen must leave the canvas usable"
    );
    paint(&mut app, full, vec![button(end, false)]);
    let before = (app.fs_zoom, app.fs_pan);
    for click_number in 1..=2 {
        for events in [
            vec![egui::Event::PointerMoved(start)],
            vec![button(start, true)],
            vec![button(start, false)],
        ] {
            paint(&mut app, full, events);
        }
        if click_number == 1 {
            assert_eq!(
                (app.fs_zoom, app.fs_pan),
                before,
                "the first new fullscreen canvas click must not reset"
            );
        }
    }
    assert_eq!(
        app.fs_zoom, 1.0,
        "the second fullscreen canvas click must reset after the actual F11 round trip"
    );
    assert_eq!(app.fs_pan, egui::Vec2::ZERO);
}

fn navigator_sources(
    ctx: &egui::Context,
    rect: egui::Rect,
    panorama: bool,
) -> crate::app::AppTestEnvForTest {
    let mut app = sources(ctx, [320, 160]);
    app.settings.fullscreen_navigator_visible = true;
    app.settings.fullscreen_navigator_corner = FullscreenNavigatorCorner::TopRight;
    app.settings.fullscreen_navigator_size = 520.0;
    app.fs_zoom = 2.0;
    if panorama {
        app.panorama_state = Some(crate::panorama::PanoramaState::new(
            0.3,
            0.1,
            crate::panorama::PanoProjection::Perspective,
        ));
    }
    settle(&mut app, ctx, rect);
    if panorama {
        assert!(app.is_panorama_mode_active(1));
    } else {
        assert_page(&app);
    }
    app
}

fn navigator_content(app: &mut App, rect: egui::Rect, panorama: bool) -> egui::Rect {
    let media = app.fullscreen_media_rect(rect, 1, false);
    let content = if panorama {
        app.panorama_navigator_layout(media, 1)
            .unwrap()
            .content_rect
    } else {
        app.fs_navigator_layout(media).unwrap().content_rect
    };
    assert!(content.is_positive());
    content
}

#[test]
fn front_popup_navigator_double_click_counts_only_backing_clicks_for_flat_and_panorama() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for (panorama, expire_launcher_click) in
        [(false, true), (false, false), (true, true), (true, false)]
    {
        let ctx = egui::Context::default();
        let rect = viewport(1093.0);
        let mut app = navigator_sources(&ctx, rect, panorama);
        app.settings.fullscreen_navigator_corner = FullscreenNavigatorCorner::BottomLeft;
        settle(&mut app, &ctx, rect);
        let menu = if panorama {
            Menu::Projection
        } else {
            Menu::Fit
        };
        menu.open(&mut app, &ctx, rect);
        if expire_launcher_click {
            // Let egui's launcher click expire without a wall-clock wait so
            // closing click + backing click 1 is definitely a raw double-click.
            for _ in 0..40 {
                frame(&mut app, &ctx, rect, vec![]);
            }
        }
        let content = navigator_content(&mut app, rect, panorama);
        let popup = if panorama {
            ctx.read_response(egui::Id::unique((
                "fs_panorama_projection_item",
                crate::panorama::PanoProjection::Perspective.shader_code(),
            )))
            .unwrap()
            .rect
            .expand(16.0)
        } else {
            ctx.data(|data| {
                data.get_temp::<(egui::Rect, egui::Rect, egui::Vec2)>(egui::Id::unique(
                    "fs_fit_popup_scroll_layout",
                ))
            })
            .unwrap()
            .0
        };
        let exposed = egui::Rect::from_min_max(
            content.min,
            egui::pos2(content.right().min(popup.left() - 20.0), content.bottom()),
        )
        .shrink(4.0);
        assert!(
            exposed.width() > 8.0 && exposed.height() > 30.0,
            "panorama={panorama}, content={content:?}, popup={popup:?}, exposed={exposed:?}"
        );
        let pos = exposed.lerp_inside(egui::vec2(0.35, 0.7));
        menu.assert_outside(&ctx, pos);
        let before = view_state(&app);
        click(&mut app, &ctx, rect, pos);
        assert!(!menu.is_open(&app, &ctx));
        assert_eq!(
            view_state(&app),
            before,
            "the closing click belongs to the menu"
        );
        click(&mut app, &ctx, rect, pos);
        if expire_launcher_click {
            let raw_count = ctx.input(|input| {
                input
                    .pointer
                    .released_buttons_with_click_counts()
                    .find(|(button, _)| *button == egui::PointerButton::Primary)
                    .and_then(|(_, click)| click)
                    .map(|(_, count)| count)
            });
            assert_eq!(
                raw_count,
                Some(2),
                "first backing click must be egui's raw second click (panorama={panorama})"
            );
        }
        assert_eq!(
            view_state(&app),
            before,
            "backing click 1 must not center with the closing click (panorama={panorama}, expired={expire_launcher_click})"
        );
        click(&mut app, &ctx, rect, pos);
        if panorama {
            assert_ne!(
                view_state(&app).4,
                before.4,
                "backing click 2 must center the real panorama navigator"
            );
        } else {
            assert_ne!(
                app.fs_pan, before.2,
                "backing click 2 must center the real flat navigator"
            );
        }
        assert_eq!(app.fullscreen_idx, Some(1));
    }
}

#[test]
fn front_popup_and_overflow_same_pass_complete_owned_stream_cannot_leak_before_a_fresh_press() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for overflow in [false, true] {
        let ctx = egui::Context::default();
        let rect = viewport(1093.0);
        let mut app = navigator_sources(&ctx, rect, false);
        if !overflow {
            app.settings.fullscreen_navigator_corner = FullscreenNavigatorCorner::BottomLeft;
            settle(&mut app, &ctx, rect);
        }
        let content = navigator_content(&mut app, rect, false);
        let boundary = if overflow {
            app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
            settle(&mut app, &ctx, rect);
            fs_overflow_panel_rect(rect, app.fs_overflow_panel_state)
                .unwrap()
                .left()
        } else {
            Menu::Fit.open(&mut app, &ctx, rect);
            ctx.data(|data| {
                data.get_temp::<(egui::Rect, egui::Rect, egui::Vec2)>(egui::Id::unique(
                    "fs_fit_popup_scroll_layout",
                ))
            })
            .unwrap()
            .0
            .left()
                - 20.0
        };
        let exposed = egui::Rect::from_min_max(
            content.min,
            egui::pos2(boundary.min(content.right()), content.bottom()),
        )
        .shrink(5.0);
        assert!(
            exposed.width() > 24.0 && exposed.height() > 100.0,
            "overflow={overflow}, content={content:?}, boundary={boundary}, exposed={exposed:?}"
        );
        let fresh = exposed.lerp_inside(egui::vec2(0.5, 0.65));
        let owned = if overflow {
            let overlap = content
                .intersect(fs_overflow_panel_rect(rect, app.fs_overflow_panel_state).unwrap())
                .shrink(5.0);
            assert!(overlap.width() > 25.0 && overlap.height() > 60.0);
            overlap.center()
        } else {
            exposed.lerp_inside(egui::vec2(0.5, 0.2))
        };
        let before = view_state(&app);
        // No previous pass owns this stream. Ownership must be projected at
        // each event, not inferred from the final button level of this pass.
        frame(
            &mut app,
            &ctx,
            rect,
            vec![
                egui::Event::PointerMoved(owned),
                button(owned, true),
                egui::Event::PointerMoved(fresh),
                button(fresh, false),
                button(fresh, true),
            ],
        );
        assert_eq!(
            view_state(&app),
            before,
            "earlier owned motion/release must stay hidden (overflow={overflow})"
        );
        assert_eq!(app.fs_front_pointer.is_active(), !overflow);
        if !overflow {
            // Menus use the pass-entry presentation snapshot: another press
            // in that same pass still belongs to the just-dismissed menu.
            frame(&mut app, &ctx, rect, vec![button(fresh, false)]);
            assert_eq!(view_state(&app), before);
            assert!(!app.fs_front_pointer.is_active());
            frame(&mut app, &ctx, rect, vec![button(fresh, true)]);
        }
        let end = fresh + egui::vec2(0.0, 30.0);
        assert!(content.contains(end));
        frame(&mut app, &ctx, rect, vec![egui::Event::PointerMoved(end)]);
        assert_ne!(
            app.fs_pan, before.2,
            "the new backing press must start navigator pan"
        );
        frame(&mut app, &ctx, rect, vec![button(end, false)]);
        assert_eq!(app.fullscreen_idx, Some(1));
    }
}

#[test]
fn front_popup_overflow_middle_short_click_and_drag_zoom_preserve_released_canvas_behavior() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for overflow in [false, true] {
        for drag in [false, true] {
            let ctx = egui::Context::default();
            let rect = viewport(1093.0);
            let mut app = sources(&ctx, [320, 160]);
            app.fs_zoom = 2.0;
            app.settings.ring_shortcuts.mouse_buttons_image.middle =
                crate::ring_shortcut::RingActionId::ImageRotateRight;
            settle(&mut app, &ctx, rect);
            let panel = fs_overflow_panel_rect(rect, FsOverflowPanelState::Slideshow).unwrap();
            let start = panel.min + egui::vec2(3.0, 3.0);
            assert!(assert_page(&app).contains(start));
            if overflow {
                app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
                settle(&mut app, &ctx, rect);
            }
            let before = view_state(&app);
            let rotation = app.get_rotation(1);
            frame(
                &mut app,
                &ctx,
                rect,
                vec![
                    egui::Event::PointerMoved(start),
                    pointer_button(start, egui::PointerButton::Middle, true),
                ],
            );
            assert!(
                !app.fs_front_pointer.is_active(),
                "overflow must not capture Middle"
            );
            let end = if drag {
                start + egui::vec2(-45.0, 55.0)
            } else {
                start
            };
            if drag {
                frame(&mut app, &ctx, rect, vec![egui::Event::PointerMoved(end)]);
                assert_ne!(
                    app.fs_zoom, before.1,
                    "middle drag must zoom through overflow"
                );
            }
            frame(
                &mut app,
                &ctx,
                rect,
                vec![pointer_button(end, egui::PointerButton::Middle, false)],
            );
            if drag {
                assert_eq!(
                    app.get_rotation(1),
                    rotation,
                    "middle drag is not a short click"
                );
            } else {
                assert_eq!(
                    app.get_rotation(1),
                    crate::rotation_db::Rotation::Cw90,
                    "the assigned middle short-click action must run through overflow"
                );
            }
            assert_eq!(app.fullscreen_idx, Some(1));
            assert!(!app.fs_front_pointer.is_active());
        }
    }
}
