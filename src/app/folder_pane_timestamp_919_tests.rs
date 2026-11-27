use super::*;

fn frame(app: &mut App, ctx: &egui::Context, pass: f64, events: Vec<egui::Event>) {
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 600.0),
            )),
            time: Some(pass),
            input_clock: Some(pass),
            events,
            ..Default::default()
        },
        |ui| {
            app.input_update_begin(ctx);
            app.handle_keyboard(ctx);
            app.render_toolbar(ui);
            // Exercise the production raw/egui edge join, gesture recognition and
            // branch expansion. Navigation workers are outside this bounded test.
            app.render_folder_pane(ui);
            app.finish_folder_pane_presentation(ctx, true);
        },
    )
    .drop_without_applying_deltas();
}

fn click(pos: egui::Pos2, press: f64, release: f64, known: bool) -> Vec<egui::Event> {
    let mut events = vec![egui::Event::PointerMoved(pos)];
    for (pressed, time) in [(true, press), (false, release)] {
        if known {
            events.push(egui::Event::PointerButtonTime {
                button: egui::PointerButton::Primary,
                pressed,
                time,
            });
        }
        events.push(primary_event(pos, pressed));
    }
    events
}

#[test]
fn folder_pane_919_timestamped_stall_toggles_label_but_unstamped_does_not() {
    for known in [false, true] {
        let (mut app, ctx, _, b, _, pos) = ordered_label_fixture();
        ctx.options_mut(|o| {
            o.input_options.max_double_click_delay = 0.5;
            o.max_passes = std::num::NonZeroUsize::new(1).unwrap();
        });
        let base = ctx.time() + 1.0;
        frame(
            &mut app,
            &ctx,
            base + 0.08,
            click(pos, base, base + 0.08, known),
        );
        assert_label_expanded(&app, &b, false);
        frame(
            &mut app,
            &ctx,
            base + 0.8,
            click(pos, base + 0.25, base + 0.33, known),
        );
        assert_label_expanded(&app, &b, known);
    }
}

#[test]
fn folder_pane_919_mixed_timestamps_retain_pass_interval_in_both_orders() {
    for first_known in [false, true] {
        let (mut app, ctx, _, b, _, pos) = ordered_label_fixture();
        ctx.options_mut(|o| {
            o.input_options.max_double_click_delay = 0.5;
            o.max_passes = std::num::NonZeroUsize::new(1).unwrap();
        });
        let base = ctx.time() + 1.0;
        frame(
            &mut app,
            &ctx,
            base + 0.08,
            click(pos, base, base + 0.08, first_known),
        );
        frame(
            &mut app,
            &ctx,
            base + 0.8,
            click(pos, base + 0.25, base + 0.33, !first_known),
        );
        assert_label_expanded(&app, &b, false);
    }
}
