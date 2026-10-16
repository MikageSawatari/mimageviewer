//! Existing output-consumer boundaries, kept separate from whether pixels are presented.
//!
//! These are the 0.33.3 iteration/termination rules. Submission and early-error ownership
//! remain with each renderer; the 0.37 delta drain and no-surface paths are a later stage.

pub(crate) fn apply_texture_sets(
    delta: &egui::TexturesDelta,
    mut apply: impl FnMut(egui::TextureId, &egui::epaint::ImageDelta),
) {
    for (id, image_delta) in &delta.set {
        apply(*id, image_delta);
    }
}

/// Complete an output at the consumer's existing post-submission/free boundary.
pub(crate) fn finish_texture_output(
    delta: egui::TexturesDelta,
    mut free: impl FnMut(&egui::TextureId),
) {
    for id in &delta.free {
        free(id);
    }
}

/// A headless assertion fixture has inspected this output and has no renderer to receive it.
/// Do not use this to skip resource delivery in a product Context.
#[cfg(test)]
pub(crate) fn discard_headless_output(output: egui::FullOutput) {
    drop(output);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_output_keeps_whole_partial_order_and_frees_after_submission() {
        use std::cell::RefCell;

        let id = egui::TextureId::Managed(7);
        let image = || egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]);
        let delta = egui::TexturesDelta {
            set: vec![
                (
                    id,
                    egui::epaint::ImageDelta::full(image(), egui::TextureOptions::LINEAR),
                ),
                (
                    id,
                    egui::epaint::ImageDelta::partial(
                        [2, 3],
                        image(),
                        egui::TextureOptions::NEAREST,
                    ),
                ),
            ],
            free: vec![id, egui::TextureId::Managed(8)],
        };
        let operations = RefCell::new(Vec::new());
        apply_texture_sets(&delta, |actual_id, update| {
            assert_eq!(actual_id, id);
            operations.borrow_mut().push(("apply", update.pos));
        });
        operations.borrow_mut().push(("submit", None));
        let mut freed = Vec::new();
        finish_texture_output(delta, |id| {
            freed.push(*id);
            operations.borrow_mut().push(("free", None));
        });
        assert_eq!(
            operations.into_inner(),
            [
                ("apply", None),
                ("apply", Some([2, 3])),
                ("submit", None),
                ("free", None),
                ("free", None),
            ]
        );
        assert_eq!(freed, [id, egui::TextureId::Managed(8)]);
    }

    #[test]
    fn texture_output_free_only_does_not_create_an_upload() {
        let id = egui::TextureId::Managed(12);
        let delta = egui::TexturesDelta {
            set: Vec::new(),
            free: vec![id],
        };
        apply_texture_sets(&delta, |_, _| panic!("free-only output uploaded a texture"));
        let mut freed = Vec::new();
        finish_texture_output(delta, |id| freed.push(*id));
        assert_eq!(freed, [id]);
    }
}
