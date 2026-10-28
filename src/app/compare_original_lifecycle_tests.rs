use super::*;

#[test]
fn compare_original_detached_park_drains_the_foreground_worker() {
    let mut app = setup_app_for_test();
    app.items = vec![GridItem::Image(app.tmp.path().join("compare-original.png"))];
    app.fullscreen_idx = Some(0);
    app.compare_view_mode = CompareViewMode::PinnedNormal;
    let pixels = Arc::new(egui::ColorImage::filled([2, 2], egui::Color32::WHITE));
    let geometry = CompareSourceGeometry {
        processed_source_size: [2, 2],
        crop: None,
        rotation: crate::rotation_db::Rotation::None,
    };
    let request = ComparePrepareRequest {
        current_idx: 0,
        pinned_source_idx: 1,
        output: ComparePrepareOutput::PinnedTexture,
    };
    let (tx, rx) = mpsc::channel();
    app.compare_preparation = ComparePreparationState::PreparingOriginal {
        pair: ComparePreparedPair {
            key: 1,
            current_idx: 0,
            pinned_source_idx: 1,
            current_input_size: [2, 2],
            pinned_input_size: [2, 2],
            target_size: [2, 2],
            current_geometry: geometry,
            original: CompareOriginalVariant::Absent,
            pixels: ComparePreparedPixels::PinnedTexture(pixels),
            pinned_texture: None,
            current_texture: None,
            diff_texture: None,
        },
        pending: ComparePreparePending {
            key: 1,
            current_geometry: geometry,
            request,
            scope: ComparePrepareScope::SinglePage,
            source_indices: vec![0],
            current_input_size: [2, 2],
            pinned_input_size: [2, 2],
            expected_target_size: [2, 2],
            started_at: std::time::Instant::now(),
            rx,
        },
    };
    app.reset_detached_pause_foreground_modes(0);
    assert!(matches!(app.compare_view_mode, CompareViewMode::Off));
    assert!(matches!(
        app.compare_preparation,
        ComparePreparationState::Draining(_)
    ));
    tx.send(Err("discarded completion".into())).unwrap();
    app.poll_compare_prepare_pending(&egui::Context::default());
    assert!(matches!(
        app.compare_preparation,
        ComparePreparationState::Unprepared
    ));
    assert!(app.feedback_toasts_for_test.is_empty());
}
