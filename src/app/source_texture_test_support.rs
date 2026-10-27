use super::*;

/// A canonical, immutable display source; no file I/O or renderer is needed.
pub(crate) fn seed(app: &mut App, ctx: &egui::Context, idx: usize) -> egui::TextureId {
    let pixels = Arc::new(egui::ColorImage::filled(
        [4, 6],
        egui::Color32::from_rgb(60, 120, 180),
    ));
    let tex = ctx.load_texture(
        format!("shared-source-{idx}"),
        (*pixels).clone(),
        DISPLAY_IMAGE_TEXTURE_OPTIONS,
    );
    let id = tex.id();
    app.bind_display_texture_identity(&tex, app.item_id(idx));
    app.fs_cache.insert(
        idx,
        FsCacheEntry::Static {
            pdf_rerender_failures: Vec::new(),
            pdf_metadata: None,
            tex,
            pixels,
            source_dims: Some([400, 600]),
            load_seq: 1,
            animation: StaticAnimationState::Still,
        },
    );
    id
}
