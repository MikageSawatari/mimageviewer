//! Shared ring geometry for the corner overlay and the 3D-photo header.
use eframe::egui;

pub(crate) const RING_DIAMETER: f32 = 18.0;

pub(crate) fn ring_shape(rect: egui::Rect, progress: Option<f32>) -> egui::Shape {
    let Some(progress) = progress else {
        return egui::Shape::Noop;
    };
    let center = rect.center();
    let radius = (rect.width().min(rect.height()) * 0.5 - 1.0).clamp(0.0, 8.0);
    let mut shapes = vec![egui::Shape::circle_stroke(
        center,
        radius,
        egui::Stroke::new(1.2, egui::Color32::from_white_alpha(42)),
    )];
    if progress > 0.0 {
        let start = -std::f32::consts::FRAC_PI_2;
        let sweep = std::f32::consts::TAU * progress;
        let segments = ((36.0 * progress).ceil() as usize).clamp(2, 36);
        let points = (0..=segments)
            .map(|i| {
                let angle = start + sweep * i as f32 / segments as f32;
                center + radius * egui::vec2(angle.cos(), angle.sin())
            })
            .collect();
        shapes.push(egui::Shape::line(
            points,
            egui::Stroke::new(2.0, egui::Color32::from_white_alpha(120)),
        ));
    }
    egui::Shape::Vec(shapes)
}
