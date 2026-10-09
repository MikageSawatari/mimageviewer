use std::collections::HashMap;

use egui::{Color32, Pos2, Rect, Stroke, TextureHandle, Vec2};
use musicvideo_core::{LayerBounds, LayerId, ModulatorValue, Project, evaluate_layer_bounds};

use crate::{EditTargetLifetime, EditorState};

/// All inputs belong to the displayed texture, never the transport's current time.
#[derive(Clone, Copy)]
pub struct PreviewFrame<'a> {
    pub texture: &'a TextureHandle,
    pub project: &'a Project,
    /// Captured with the project at request time, never at result application.
    pub layer_lifetimes: &'a HashMap<LayerId, EditTargetLifetime>,
    pub time_secs: f64,
    pub modulator_values: &'a [ModulatorValue],
}

impl PreviewFrame<'_> {
    pub fn selected_bounds(&self, state: &EditorState) -> Option<LayerBounds> {
        if !state.show_selection_bounds {
            return None;
        }
        let layer = state.selected_layer?;
        let lifetime = state.layer_target_lifetime(layer)?;
        if self.layer_lifetimes.get(&layer).copied() != Some(lifetime) {
            return None;
        }
        evaluate_layer_bounds(self.project, layer, self.time_secs, self.modulator_values)
    }
}

/// Draws only editor decorations over an already-composited texture.
pub fn draw_preview(ui: &mut egui::Ui, frame: Option<PreviewFrame<'_>>, state: &mut EditorState) {
    if let Some(frame) = frame {
        let source = frame.texture.size_vec2();
        let available = ui.available_size().max(Vec2::ZERO);
        let (area, _) = ui.allocate_exact_size(available, egui::Sense::hover());
        let scale = (area.width() / source.x).min(area.height() / source.y);
        let image_rect = Rect::from_center_size(area.center(), source * scale);
        let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(area));
        painter.image(
            frame.texture.id(),
            image_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        if image_rect.is_positive()
            && let Some(bounds) = frame.selected_bounds(state)
        {
            let layer = frame
                .project
                .layers
                .iter()
                .find(|layer| Some(layer.id) == state.selected_layer)
                .unwrap();
            paint_selection(&painter, image_rect, bounds, &layer.name);
        }
    } else {
        ui.centered_and_justified(|ui| {
            ui.spinner();
            ui.label("プレビューを準備しています");
        });
    }
}

#[derive(Debug, PartialEq)]
enum OverlayGeometry {
    Box([Pos2; 4]),
    Offscreen { anchor: Pos2, direction: Vec2 },
}

fn overlay_geometry(bounds: LayerBounds, image: Rect) -> OverlayGeometry {
    // This is the only normalized-output -> preview mapping.
    let corners = bounds
        .corners
        .map(|(x, y)| image.min + Vec2::new(x * image.width(), y * image.height()));
    if intersects_image(&bounds.corners) {
        return OverlayGeometry::Box(corners);
    }
    let center = (corners[0] + corners[2].to_vec2()) * 0.5;
    let delta = center - image.center();
    let fraction = (image.width() * 0.5 / delta.x.abs()).min(image.height() * 0.5 / delta.y.abs());
    OverlayGeometry::Offscreen {
        anchor: image.center() + delta * fraction,
        direction: delta.normalized(),
    }
}

/// Separating-axis test: a rotated box's AABB or corners alone are insufficient
/// (a box can surround the image or cross it with every corner outside).
fn intersects_image(corners: &[(f32, f32); 4]) -> bool {
    let image = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
    let axes = [(1.0, 0.0), (0.0, 1.0)].into_iter().chain((0..4).map(|i| {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        (a.1 - b.1, b.0 - a.0)
    }));
    for (x, y) in axes {
        let range = |points: &[(f32, f32); 4]| {
            points
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), p| {
                    let v = p.0 * x + p.1 * y;
                    (min.min(v), max.max(v))
                })
        };
        let a = range(corners);
        let b = range(&image);
        if a.1 < b.0 || b.1 < a.0 {
            return false;
        }
    }
    true
}

fn paint_selection(painter: &egui::Painter, image: Rect, bounds: LayerBounds, name: &str) {
    let strokes = [
        Stroke::new(3.0, Color32::BLACK),
        Stroke::new(1.0, Color32::WHITE),
    ];
    let (label_at, offscreen) = match overlay_geometry(bounds, image) {
        OverlayGeometry::Box(corners) => {
            for stroke in strokes {
                if bounds.visible {
                    painter.add(egui::Shape::closed_line(corners.to_vec(), stroke));
                } else {
                    for i in 0..4 {
                        // Clip BEFORE generating dashes, so a far-off box cannot
                        // allocate unbounded shapes on the UI thread.
                        if let Some(line) =
                            clip_segment([corners[i], corners[(i + 1) % 4]], painter.clip_rect())
                        {
                            painter.extend(egui::Shape::dashed_line(&line, stroke, 6.0, 4.0));
                        }
                    }
                }
            }
            (corners[0] + Vec2::new(4.0, 4.0), false)
        }
        OverlayGeometry::Offscreen { anchor, direction } => {
            let tip = anchor - direction * 3.0;
            let normal = Vec2::new(-direction.y, direction.x);
            for stroke in strokes {
                painter.line_segment([tip - direction * 18.0, tip], stroke);
                painter.line_segment([tip - direction * 7.0 + normal * 5.0, tip], stroke);
                painter.line_segment([tip - direction * 7.0 - normal * 5.0, tip], stroke);
            }
            (anchor - direction * 26.0, true)
        }
    };
    let suffix = match (offscreen, bounds.visible) {
        (true, false) => "（画面外・非表示）",
        (true, true) => "（画面外）",
        (false, false) => "（非表示）",
        (false, true) => "",
    };
    let galley = painter.layout_no_wrap(
        format!("{name}{suffix}"),
        egui::FontId::proportional(13.0),
        Color32::WHITE,
    );
    let clip = painter.clip_rect().shrink(2.0);
    if !clip.is_positive() {
        return;
    }
    let pos = Pos2::new(
        label_at.x.clamp(
            clip.left(),
            (clip.right() - galley.size().x).max(clip.left()),
        ),
        label_at.y.clamp(
            clip.top(),
            (clip.bottom() - galley.size().y).max(clip.top()),
        ),
    );
    painter.rect_filled(
        Rect::from_min_size(pos, galley.size()).expand(2.0),
        2.0,
        Color32::from_black_alpha(220),
    );
    painter.galley(pos, galley, Color32::WHITE);
}

fn clip_segment(line: [Pos2; 2], clip: Rect) -> Option<[Pos2; 2]> {
    // Use f64 here so very distant coordinates do not lose panel-sized endpoints.
    let x = f64::from(line[0].x);
    let y = f64::from(line[0].y);
    let dx = f64::from(line[1].x) - x;
    let dy = f64::from(line[1].y) - y;
    let mut start = 0.0_f64;
    let mut end = 1.0_f64;
    for (p, q) in [
        (-dx, x - f64::from(clip.left())),
        (dx, f64::from(clip.right()) - x),
        (-dy, y - f64::from(clip.top())),
        (dy, f64::from(clip.bottom()) - y),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else if p < 0.0 {
            start = start.max(q / p);
        } else {
            end = end.min(q / p);
        }
    }
    (start <= end).then_some([
        Pos2::new((x + dx * start) as f32, (y + dy * start) as f32),
        Pos2::new((x + dx * end) as f32, (y + dy * end) as f32),
    ])
}

#[cfg(test)]
mod tests;
