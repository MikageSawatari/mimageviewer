//! Texture output consumption is independent of whether pixels are presented.

pub(crate) fn apply_texture_sets(
    delta: &mut egui::TexturesDelta,
    mut apply: impl FnMut(egui::TextureId, &egui::epaint::ImageDelta),
) {
    for (id, updates) in delta.set.drain() {
        for update in updates {
            apply(id, &update);
        }
    }
}

/// Complete an output at the consumer's post-submission/free boundary.
pub(crate) fn finish_texture_output(
    mut delta: egui::TexturesDelta,
    mut free: impl FnMut(&egui::TextureId),
) {
    assert!(
        delta.set.is_empty(),
        "texture sets must be consumed before freeing"
    );
    for id in delta.free.drain() {
        free(&id);
    }
}

/// The renderer is ending after a fault; this Context will never be run again.
/// This is a lifetime terminal, never a way to skip delivery in a surviving Context.
pub(crate) fn terminate_renderer_output(delta: &mut egui::TexturesDelta) {
    delta.clear();
}

/// A headless assertion fixture has inspected this output and has no renderer.
#[cfg(test)]
pub(crate) fn discard_headless_output(output: egui::FullOutput) {
    output.drop_without_applying_deltas();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_output_keeps_whole_partial_order_and_frees_after_submission() {
        use std::cell::RefCell;
        let id = egui::TextureId::Managed(7);
        let image = || egui::ColorImage::filled([1, 1], egui::Color32::WHITE);
        let mut delta = egui::TexturesDelta::default();
        delta.push(
            id,
            egui::epaint::ImageDelta::full(image(), egui::TextureOptions::LINEAR),
        );
        delta.push(
            id,
            egui::epaint::ImageDelta::partial([2, 3], image(), egui::TextureOptions::NEAREST),
        );
        delta.free(id);
        delta.free(egui::TextureId::Managed(8));
        let operations = RefCell::new(Vec::new());
        apply_texture_sets(&mut delta, |actual_id, update| {
            assert_eq!(actual_id, id);
            operations.borrow_mut().push(("apply", update.pos));
        });
        assert!(delta.set.is_empty());
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
                ("free", None)
            ]
        );
        freed.sort();
        assert_eq!(freed, [id, egui::TextureId::Managed(8)]);
    }

    #[test]
    fn texture_output_free_only_does_not_create_an_upload() {
        let id = egui::TextureId::Managed(12);
        let mut delta = egui::TexturesDelta::default();
        delta.free(id);
        apply_texture_sets(&mut delta, |_, _| {
            panic!("free-only output uploaded a texture")
        });
        let mut freed = Vec::new();
        finish_texture_output(delta, |id| freed.push(*id));
        assert_eq!(freed, [id]);
    }
}
