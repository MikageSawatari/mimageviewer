//! Pixel limits shared by scene evaluation and the editor's input ranges.
//!
//! Apply these after whole-layer scaling. Keeping glyph size and dilation
//! bounded also bounds comic-core's per-glyph coverage preparation; clipping
//! the final text surface alone cannot bound those intermediate bitmaps.

pub const MAX_GLOW_RADIUS_PX: f32 = 128.0;
pub const MAX_GLOW_SPREAD_PX: f32 = 128.0;
pub const MAX_TEXT_SIZE_PX: f32 = 1000.0;
pub const MAX_TEXT_OUTLINE_PX: f32 = 128.0;
pub const MAX_TEXT_SHADOW_SPREAD_PX: f32 = 128.0;

/// Avoid copying text and spectrum data on the usual already-evaluated path.
pub(crate) fn bounded_item(item: &crate::SceneItem) -> std::borrow::Cow<'_, crate::SceneItem> {
    let glow_bounded = item.glow.is_none_or(|glow| {
        glow.radius_px <= MAX_GLOW_RADIUS_PX && glow.spread_px <= MAX_GLOW_SPREAD_PX
    });
    let text_bounded = match &item.kind {
        crate::SceneItemKind::Text { size_px, style, .. }
        | crate::SceneItemKind::TimeText { size_px, style, .. } => {
            *size_px <= MAX_TEXT_SIZE_PX
                && style
                    .outline
                    .is_none_or(|s| s.width_px <= MAX_TEXT_OUTLINE_PX)
                && style
                    .extra_outlines
                    .iter()
                    .all(|s| s.width_px <= MAX_TEXT_OUTLINE_PX)
                && style
                    .shadow
                    .is_none_or(|s| s.spread_px <= MAX_TEXT_SHADOW_SPREAD_PX)
        }
        _ => true,
    };
    if glow_bounded && text_bounded {
        std::borrow::Cow::Borrowed(item)
    } else {
        let mut bounded = item.clone();
        clamp_item_dimensions(&mut bounded);
        std::borrow::Cow::Owned(bounded)
    }
}

/// Shared by evaluated projects and callers supplying a scene directly.
pub(crate) fn clamp_item_dimensions(item: &mut crate::SceneItem) {
    if let Some(glow) = &mut item.glow {
        glow.radius_px = glow.radius_px.min(MAX_GLOW_RADIUS_PX);
        glow.spread_px = glow.spread_px.min(MAX_GLOW_SPREAD_PX);
    }
    if let crate::SceneItemKind::Text { size_px, style, .. }
    | crate::SceneItemKind::TimeText { size_px, style, .. } = &mut item.kind
    {
        *size_px = size_px.min(MAX_TEXT_SIZE_PX);
        if let Some(outline) = &mut style.outline {
            outline.width_px = outline.width_px.min(MAX_TEXT_OUTLINE_PX);
        }
        for outline in &mut style.extra_outlines {
            outline.width_px = outline.width_px.min(MAX_TEXT_OUTLINE_PX);
        }
        if let Some(shadow) = &mut style.shadow {
            shadow.spread_px = shadow.spread_px.min(MAX_TEXT_SHADOW_SPREAD_PX);
        }
    }
}
