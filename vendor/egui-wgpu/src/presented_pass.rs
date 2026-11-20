//! Correlate native hit-test snapshots with a completed surface presentation.

fn key(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("egui_wgpu::last_presented_pass", viewport))
}

/// Last egui pass actually presented for this viewport, if any.
/// Skipped paints and capture-only rendering do not advance this value.
pub fn last_presented_pass(ctx: &egui::Context, viewport: egui::ViewportId) -> Option<u64> {
    ctx.data(|data| data.get_temp(key(viewport)))
}

pub(crate) fn present_if_available<T, R>(
    ctx: &egui::Context,
    viewport: egui::ViewportId,
    frame: Option<T>,
    present: impl FnOnce(T) -> R,
) -> Option<R> {
    let result = present(frame?);
    // The present call must have returned before publishing the pass number.
    let pass = ctx.cumulative_pass_nr_for(viewport);
    ctx.data_mut(|data| data.insert_temp(key(viewport), pass));
    Some(result)
}
