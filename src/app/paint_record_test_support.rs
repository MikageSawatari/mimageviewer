//! Test-only paint observations shared by platform-independent draw sites.

use crate::gpu_lanczos::{FullscreenPaintPayload, FullscreenPaintResource, TestPaintProvenance};
use std::cell::Cell;
use std::sync::Arc;

thread_local! {
    static CAPTURE_DEPTH: Cell<usize> = const { Cell::new(0) };
}

struct CaptureGuard;

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        CAPTURE_DEPTH.with(|depth| depth.set(depth.get() - 1));
    }
}

pub(crate) fn with_capture<R>(run: impl FnOnce() -> R) -> R {
    CAPTURE_DEPTH.with(|depth| depth.set(depth.get() + 1));
    let _guard = CaptureGuard;
    run()
}

/// This marker is adjacent to one submitted image mesh in the painter's shape
/// stream. Texture IDs can be shared by distinct submissions, so they are only
/// checked for consistency and never used as provenance lookup keys.
#[derive(Clone)]
struct PaintSubmission {
    texture: egui::TextureId,
    provenance: TestPaintProvenance,
}

#[derive(Clone)]
struct WhiteCompanionSubmission;

/// Marker emitted next to the actual draw shape, never inferred from a unit DTO.
pub(crate) fn record_white_companion_paint(painter: &egui::Painter, rect: egui::Rect) {
    if !CAPTURE_DEPTH.with(|depth| depth.get() > 0) {
        return;
    }
    painter.add(egui::PaintCallback {
        rect,
        callback: Arc::new(WhiteCompanionSubmission),
    });
}

#[derive(Clone, Debug)]
pub(crate) struct WhiteCompanionPaintRecord {
    pub(crate) viewport: egui::ViewportId,
    pub(crate) rect: egui::Rect,
    pub(crate) clip: egui::Rect,
}

pub(crate) fn white_companion_paint_records(
    viewport: egui::ViewportId,
    output: &egui::FullOutput,
) -> Vec<WhiteCompanionPaintRecord> {
    fn visit(
        viewport: egui::ViewportId,
        clip: egui::Rect,
        shape: &egui::Shape,
        pending: &mut bool,
        records: &mut Vec<WhiteCompanionPaintRecord>,
    ) {
        match shape {
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    visit(viewport, clip, shape, pending, records);
                }
            }
            egui::Shape::Callback(callback)
                if callback
                    .callback
                    .downcast_ref::<WhiteCompanionSubmission>()
                    .is_some() =>
            {
                *pending = true;
            }
            egui::Shape::Rect(shape) if std::mem::take(pending) => {
                assert_eq!(shape.fill, egui::Color32::WHITE);
                records.push(WhiteCompanionPaintRecord {
                    viewport,
                    rect: shape.rect,
                    clip,
                });
            }
            _ => {}
        }
    }
    let mut records = Vec::new();
    let mut pending = false;
    for clipped in &output.shapes {
        visit(
            viewport,
            clipped.clip_rect,
            &clipped.shape,
            &mut pending,
            &mut records,
        );
    }
    assert!(!pending, "white companion marker has no draw shape");
    records
}

/// Read-only submission evidence shared by normal resources and captured presentation materials.
pub(crate) trait PaintRecordResource {
    fn paint_texture_id(&self) -> egui::TextureId;
    fn test_paint_provenance(&self) -> Option<&TestPaintProvenance>;
}

impl PaintRecordResource for FullscreenPaintResource {
    fn paint_texture_id(&self) -> egui::TextureId {
        self.paint_texture_id()
    }

    fn test_paint_provenance(&self) -> Option<&TestPaintProvenance> {
        self.test_paint_provenance()
    }
}

impl PaintRecordResource for FullscreenPaintPayload {
    fn paint_texture_id(&self) -> egui::TextureId {
        self.paint_texture_id()
    }

    fn test_paint_provenance(&self) -> Option<&TestPaintProvenance> {
        self.test_paint_provenance()
    }
}

pub(crate) fn record_selected_paint_resource<R: PaintRecordResource + ?Sized>(
    painter: &egui::Painter,
    resource: &R,
) {
    if !CAPTURE_DEPTH.with(|depth| depth.get() > 0) {
        return;
    }
    let Some(provenance) = resource.test_paint_provenance() else {
        return;
    };
    painter.add(egui::PaintCallback {
        rect: egui::Rect::NOTHING,
        callback: Arc::new(PaintSubmission {
            texture: resource.paint_texture_id(),
            provenance: provenance.clone(),
        }),
    });
}

#[derive(Clone, Debug)]
pub(crate) struct PaintVertex {
    pub(crate) pos: egui::Pos2,
    pub(crate) uv: egui::Pos2,
}

#[derive(Clone, Debug)]
pub(crate) struct PaintRecord {
    pub(crate) viewport: egui::ViewportId,
    pub(crate) texture: egui::TextureId,
    pub(crate) provenance: TestPaintProvenance,
    pub(crate) vertices: Vec<PaintVertex>,
    pub(crate) clip: egui::Rect,
}

fn collect_shape(
    viewport: egui::ViewportId,
    clip: egui::Rect,
    shape: &egui::Shape,
    pending: &mut Option<PaintSubmission>,
    out: &mut Vec<PaintRecord>,
) {
    match shape {
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_shape(viewport, clip, shape, pending, out);
            }
        }
        egui::Shape::Callback(callback) => {
            if let Some(submission) = callback.callback.downcast_ref::<PaintSubmission>() {
                assert!(
                    pending.is_none(),
                    "paint proof must precede exactly one mesh"
                );
                *pending = Some(submission.clone());
            }
        }
        egui::Shape::Mesh(mesh) => {
            if let Some(submission) = pending.take() {
                assert_eq!(
                    mesh.texture_id, submission.texture,
                    "paint proof must match the next submitted mesh"
                );
                out.push(PaintRecord {
                    viewport,
                    texture: mesh.texture_id,
                    provenance: submission.provenance,
                    vertices: mesh
                        .vertices
                        .iter()
                        .map(|vertex| PaintVertex {
                            pos: vertex.pos,
                            uv: vertex.uv,
                        })
                        .collect(),
                    clip,
                });
            }
        }
        _ => {}
    }
}

pub(crate) fn paint_records(
    viewport: egui::ViewportId,
    output: &egui::FullOutput,
) -> Vec<PaintRecord> {
    let mut records = Vec::new();
    let mut pending = None;
    for clipped in &output.shapes {
        collect_shape(
            viewport,
            clipped.clip_rect,
            &clipped.shape,
            &mut pending,
            &mut records,
        );
    }
    assert!(pending.is_none(), "paint proof has no submitted mesh");
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_presentation_record_preserves_provenance_after_source_release() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "native-record-input",
            egui::ColorImage::filled([8, 8], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let source_texture_id = texture.id();
        let proof = TestPaintProvenance {
            context: crate::app::ViewerContextId::for_test(7),
            items_generation: 3,
            item: "captured-page".to_owned(),
            page: 2,
        };
        let source = crate::gpu_lanczos::test_native_resource(
            FullscreenPaintResource::resampleable(
                2,
                texture,
                crate::gpu_lanczos::FullscreenPaintSourceGeneration { items: 3, input: 5 },
            )
            .with_test_paint_provenance(proof.clone()),
            [4, 4],
        );
        let payload =
            FullscreenPaintPayload::PresentationMaterial(source.capture_presentation(&ctx));
        drop(source);
        assert!(ctx.tex_manager().read().meta(source_texture_id).is_none());
        let output = with_capture(|| {
            crate::test_ui::headless_paint_output(ctx.run_pass(egui::RawInput::default(), |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::background());
                record_selected_paint_resource(&painter, &payload);
                painter.image(
                    payload.paint_texture_id(),
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(8.0, 8.0)),
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }))
        });
        let records = paint_records(egui::ViewportId::ROOT, &output);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].texture, payload.paint_texture_id());
        assert_eq!(records[0].provenance, proof);
        assert!(payload.source_texture().is_none());
    }

    #[test]
    fn multiwindow_scenario_same_texture_keeps_each_submission_provenance() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "same-texture",
            egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
            egui::TextureOptions::NEAREST,
        );
        let proof = |item: &str| TestPaintProvenance {
            context: crate::app::ViewerContextId::for_test(7),
            items_generation: 1,
            item: item.to_owned(),
            page: 0,
        };
        let first = FullscreenPaintResource::direct(texture.clone())
            .with_test_paint_provenance(proof("first"));
        let second =
            FullscreenPaintResource::direct(texture).with_test_paint_provenance(proof("second"));
        let output = with_capture(|| {
            crate::test_ui::headless_paint_output(ctx.run_pass(egui::RawInput::default(), |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::background());
                for (index, resource) in [&first, &second].into_iter().enumerate() {
                    record_selected_paint_resource(&painter, resource);
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(index as f32 * 10.0, 0.0),
                        egui::vec2(8.0, 8.0),
                    );
                    painter.image(
                        resource.paint_texture_id(),
                        rect,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
            }))
        });
        let records = paint_records(egui::ViewportId::ROOT, &output);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].texture, records[1].texture);
        assert_eq!(records[0].provenance.item, "first");
        assert_eq!(records[1].provenance.item, "second");
        assert_ne!(records[0].vertices[0].pos, records[1].vertices[0].pos);
    }
}
