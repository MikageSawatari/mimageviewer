use super::*;
use musicvideo_core::{Animatable, NormRect, default_project};

fn box_bounds(rect: NormRect, angle: f32) -> LayerBounds {
    let mut project = default_project(10.0);
    project.layers[0].rect = Animatable::Const(rect);
    project.layers[0].rotation = Animatable::Const(angle);
    evaluate_layer_bounds(&project, project.layers[0].id, 0.0, &[]).unwrap()
}

fn image() -> Rect {
    Rect::from_min_size(Pos2::new(100.0, 80.0), Vec2::new(400.0, 225.0))
}

#[test]
fn selection_preview_maps_the_actual_image_rect() {
    let b = box_bounds(NormRect::new(0.25, 0.2, 0.5, 0.4), 0.0);
    let OverlayGeometry::Box(corners) = overlay_geometry(b, image()) else {
        panic!()
    };
    assert_eq!(corners[0], Pos2::new(200.0, 125.0));
    assert_eq!(corners[2], Pos2::new(400.0, 215.0));
    let b = box_bounds(NormRect::new(0.25, 0.2, 0.5, 0.4), 90.0);
    let OverlayGeometry::Box(corners) = overlay_geometry(b, image()) else {
        panic!()
    };
    assert!((corners[0].x - 345.0).abs() < 1e-4);
    assert!((corners[0].y - 70.0).abs() < 1e-4);
}

#[test]
fn selection_partial_crossing_and_surrounding_boxes_are_not_offscreen() {
    for rect in [
        NormRect::new(-0.1, 0.2, 0.3, 0.2),
        NormRect::new(-1.0, -1.0, 3.0, 3.0),
        NormRect::new(-1.0, 0.4, 3.0, 0.1),
    ] {
        assert!(matches!(
            overlay_geometry(box_bounds(rect, 0.0), image()),
            OverlayGeometry::Box(_)
        ));
    }
    // Every corner outside, but a rotated thin box crosses the image.
    assert!(matches!(
        overlay_geometry(
            box_bounds(NormRect::new(-1.0, 0.45, 3.0, 0.1), 30.0),
            image()
        ),
        OverlayGeometry::Box(_)
    ));
}

#[test]
fn selection_fully_offscreen_markers_point_to_all_sides_and_corners() {
    for (x, y) in [
        (-2.0, 0.4),
        (2.0, 0.4),
        (0.4, -2.0),
        (0.4, 2.0),
        (2.0, -2.0),
        (-2.0, 2.0),
    ] {
        let b = box_bounds(NormRect::new(x, y, 0.2, 0.2), 20.0);
        let OverlayGeometry::Offscreen { anchor, direction } = overlay_geometry(b, image()) else {
            panic!()
        };
        assert!(image().expand(0.001).contains(anchor));
        assert!(
            (anchor.x - image().left()).abs() < 0.001
                || (anchor.x - image().right()).abs() < 0.001
                || (anchor.y - image().top()).abs() < 0.001
                || (anchor.y - image().bottom()).abs() < 0.001
        );
        let center = Pos2::new(
            image().left() + (x + 0.1) * image().width(),
            image().top() + (y + 0.1) * image().height(),
        );
        assert!(direction.dot(center - image().center()) > 0.0);
    }
    // Its AABB overlaps the top-left corner, but the actual diamond does not.
    assert!(!intersects_image(&[
        (-0.3, 0.1),
        (0.1, -0.3),
        (-0.3, -0.7),
        (-0.7, -0.3)
    ]));
}

fn render(state: &mut EditorState, project: &Project, ready: bool) -> egui::FullOutput {
    state.target_lifetimes.reconcile(project);
    let layer_lifetimes = project
        .layers
        .iter()
        .filter_map(|layer| {
            state
                .layer_target_lifetime(layer.id)
                .map(|lifetime| (layer.id, lifetime))
        })
        .collect();
    let context = crate::test_context();
    let texture = context.load_texture(
        "test",
        egui::ColorImage::filled([160, 90], Color32::BLACK),
        Default::default(),
    );
    let mut output = egui::FullOutput::default();
    for _ in 0..2 {
        output = context.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 480.0))),
                ..Default::default()
            },
            |ctx| {
                egui::TopBottomPanel::top("preview-controls")
                    .show(ctx, |ui| state.preview_controls(ui));
                egui::CentralPanel::default().show(ctx, |ui| {
                    draw_preview(
                        ui,
                        ready.then_some(PreviewFrame {
                            texture: &texture,
                            project,
                            layer_lifetimes: &layer_lifetimes,
                            time_secs: 3.0,
                            modulator_values: &[],
                        }),
                        state,
                    )
                });
            },
        );
    }
    output
}

fn has_text(output: &egui::FullOutput, text: &str) -> bool {
    output
        .shapes
        .iter()
        .any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == text))
}

#[test]
fn selection_painter_has_two_strokes_name_hidden_dashes_and_panel_clip() {
    let mut project = default_project(10.0);
    project.layers[0].name = "test-box".into();
    project.layers[0].rect = Animatable::Const(NormRect::new(-0.1, 0.2, 0.3, 0.2));
    let mut state = EditorState::default();
    state.select_layer(&project, 0);
    let output = render(&mut state, &project, true);
    let paths: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Path(path) if s.clip_rect.min.y > 20.0 => Some((path, s.clip_rect)),
            _ => None,
        })
        .collect();
    assert_eq!(paths.len(), 2);
    assert!(paths.iter().all(|(path, clip)| path.closed
        && clip.min.x >= 0.0
        && clip.min.y > 20.0
        && clip.max.x <= 640.0
        && clip.max.y <= 480.0));
    assert_eq!(paths[0].0.stroke.width, 3.0);
    assert_eq!(paths[1].0.stroke.width, 1.0);
    assert!(has_text(&output, "test-box"));
    project.layers[0].visible = Animatable::Const(false);
    let hidden = render(&mut state, &project, true);
    assert!(has_text(&hidden, "test-box（非表示）"));
    assert!(
        !hidden
            .shapes
            .iter()
            .any(|s| s.clip_rect.min.y > 20.0
                && matches!(&s.shape, egui::Shape::Path(p) if p.closed))
    );
    assert!(
        hidden
            .shapes
            .iter()
            .filter(
                |s| s.clip_rect.min.y > 20.0 && matches!(&s.shape, egui::Shape::LineSegment { .. })
            )
            .count()
            > 8
    );
    project.layers[0].rect = Animatable::Const(NormRect::new(2.0, 0.2, 0.3, 0.2));
    let outside = render(&mut state, &project, true);
    assert!(has_text(&outside, "test-box（画面外・非表示）"));
    assert_eq!(
        outside
            .shapes
            .iter()
            .filter(
                |s| s.clip_rect.min.y > 20.0 && matches!(&s.shape, egui::Shape::LineSegment { .. })
            )
            .count(),
        6
    );
}

#[test]
fn selection_no_frame_no_selection_or_disabled_draws_no_outline() {
    let mut project = default_project(10.0);
    project.layers[0].name = "test-box".into();
    let mut state = EditorState::default();
    assert!(!has_text(&render(&mut state, &project, true), "test-box"));
    state.select_layer(&project, 0);
    assert!(!has_text(&render(&mut state, &project, false), "test-box"));
    state.show_selection_bounds = false;
    assert!(!has_text(&render(&mut state, &project, true), "test-box"));
}

#[test]
fn selection_toggle_is_ui_only_across_undo_and_project_reset() {
    let mut project = default_project(10.0);
    let mut state = EditorState::default();
    state.reset_for_project(&project);
    assert!(state.show_selection_bounds);
    let before = state.begin_external_edit(&project);
    state.show_selection_bounds = false;
    assert!(!state.finish_external_edit(&project, before));
    assert!(!state.can_undo());
    let before = state.begin_external_edit(&project);
    project.display_gain_db = 4.0;
    assert!(state.finish_external_edit(&project, before));
    assert!(state.undo(&mut project));
    assert!(!state.show_selection_bounds);
    assert!(state.redo(&mut project));
    assert!(!state.show_selection_bounds);
    state.reset_for_project(&project);
    assert!(!state.show_selection_bounds);
}

#[test]
fn selection_dashes_clip_far_edges_before_allocating_segments() {
    let clip = Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 100.0));
    assert_eq!(
        clip_segment([Pos2::new(-1e8, 50.0), Pos2::new(1e8, 50.0)], clip),
        Some([Pos2::new(0.0, 50.0), Pos2::new(100.0, 50.0)])
    );
    assert_eq!(
        clip_segment([Pos2::new(-100.0, -100.0), Pos2::new(-50.0, -50.0)], clip),
        None
    );
}
