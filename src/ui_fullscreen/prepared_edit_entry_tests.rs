//! Ctrl+E's actual pre-export gate must cancel B or accept its CPU-only stages.
use super::*;

#[test]
fn export_gate_cancels_unpublished_b_before_starting_the_ordinary_local_owner() {
    let ctx = egui::Context::default();
    let mut app = crate::app::final_prepare::tests::app(&ctx);
    let (key, cancel) = app.seed_prepared_edit_entry_test(&ctx, 1, false);
    app.fullscreen_idx = Some(1);
    assert!(app.ensure_export_local_adjust_ready(&ctx, &[1]).is_err());
    assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
    assert!(!app.final_effect_pending.contains_key(&key));
    assert!(app.local_adjust_pending.contains_key(&1));
    assert!(
        app.current_local_adjust_pixels(1).is_none(),
        "B did not fire an editing continuation"
    );
    app.local_adjust_pending.clear();
}

#[test]
fn export_gate_accepts_published_cpu_only_stages_without_another_operation_or_upload() {
    let ctx = egui::Context::default();
    let mut app = crate::app::final_prepare::tests::app(&ctx);
    let (key, _) = app.seed_prepared_edit_entry_test(&ctx, 1, true);
    app.fullscreen_idx = Some(1);
    let texture = app.final_composite_cache[&key].texture.id();
    let pixels = app.current_local_adjust_pixels(1).unwrap();
    assert!(app.ensure_export_local_adjust_ready(&ctx, &[1]).is_ok());
    assert!(app.local_adjust_pending.is_empty());
    assert!(std::sync::Arc::ptr_eq(
        &pixels,
        &app.current_local_adjust_pixels(1).unwrap()
    ));
    assert!(app.current_local_adjust_texture(1).is_none());
    assert_eq!(app.final_composite_cache[&key].texture.id(), texture);
}
