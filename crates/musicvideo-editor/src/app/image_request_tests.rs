use super::*;

fn settle(app: &mut MusicVideoLabApp, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.poll_loads(ctx);
        app.sync_runtime_resources(ctx);
        app.poll_resource_restore();
        if app.image_pending.is_none() && app.runtime_resources_ready() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "image request did not settle: {}",
            app.status
        );
        std::thread::yield_now();
    }
}

fn history(app: &mut MusicVideoLabApp, ctx: &egui::Context, redo: bool) {
    let changed = if redo {
        app.editor_state.redo(&mut app.project)
    } else {
        app.editor_state.undo(&mut app.project)
    };
    assert!(changed);
    app.handle_editor_response(EditorResponse {
        project_changed: true,
        history_changed: true,
        ..EditorResponse::default()
    });
    settle(app, ctx);
}

fn assert_image(app: &MusicVideoLabApp, id: &ImageId, path: &std::path::Path, red: u8) {
    let references = app
        .project
        .images
        .iter()
        .filter(|item| &item.id == id)
        .collect::<Vec<_>>();
    assert_eq!(references.len(), 1);
    assert_eq!(references[0].path, path);
    assert_eq!(app.assets.image(id).unwrap().rgba, [red, 34, 56, 255]);
    assert!(app.editor_state.resource_manifest().image(id).is_some());
}

#[test]
fn musicvideo_image_default_background_request_creates_reference_and_restores_history() {
    let ctx = crate::project::test_context();
    let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
    let initial = app.project.clone();
    assert!(initial.images.is_empty());
    let id = ImageId::new(BACKGROUND_ID);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("background.png");
    image::RgbaImage::from_pixel(1, 1, image::Rgba([12, 34, 56, 255]))
        .save(&path)
        .unwrap();

    // The toolbar's OpenBackground and missing-material ReplaceImage both call
    // this handler after the file picker. Exercise its real decode/blob worker.
    app.start_image_load(id.clone(), path.clone(), &ctx);
    assert!(app.image_pending.is_some(), "{}", app.status);
    assert_eq!(
        app.project, initial,
        "request admission must not edit the project"
    );
    settle(&mut app, &ctx);
    assert_image(&app, &id, &path, 12);
    assert!(matches!(
        initial.layers[0].kind,
        LayerKind::Image {
            fit: Fit::Cover,
            ..
        }
    ));
    assert_eq!(app.project.layers, initial.layers);
    assert_eq!(app.editor_state.history_depths(), (1, 0));
    let revision = app
        .editor_state
        .resource_manifest()
        .image(&id)
        .unwrap()
        .revision();
    std::fs::remove_file(&path).unwrap();

    history(&mut app, &ctx, false);
    assert_eq!(app.project, initial);
    assert!(!app.assets.has_image(&id));
    assert!(app.editor_state.resource_manifest().image(&id).is_none());
    history(&mut app, &ctx, true);
    assert_image(&app, &id, &path, 12);
    assert_eq!(
        app.editor_state
            .resource_manifest()
            .image(&id)
            .unwrap()
            .revision(),
        revision
    );

    // Replacement is still available after Redo, and after Undo has returned
    // to the initial layer-only slot. No ImageRef is manually supplied.
    for undo_to_initial in [false, true] {
        history(&mut app, &ctx, false);
        if !undo_to_initial {
            history(&mut app, &ctx, true);
        }
        image::RgbaImage::from_pixel(1, 1, image::Rgba([77, 34, 56, 255]))
            .save(&path)
            .unwrap();
        app.start_image_load(id.clone(), path.clone(), &ctx);
        assert!(app.image_pending.is_some(), "{}", app.status);
        settle(&mut app, &ctx);
        assert_image(&app, &id, &path, 77);
        // Return to the original insertion before the next loop iteration.
        history(&mut app, &ctx, false);
    }
}

#[test]
fn musicvideo_image_unknown_replacement_is_rejected_without_creating_resources() {
    let ctx = crate::project::test_context();
    let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
    let initial = app.project.clone();
    let id = ImageId::new("absent-from-references-and-layers");
    app.start_image_load(id.clone(), PathBuf::from("unused.png"), &ctx);
    assert!(app.image_pending.is_none());
    assert_eq!(app.status, "画像の差替対象が見つかりません");
    assert_eq!(app.project, initial);
    assert_eq!(app.editor_state.history_depths(), (0, 0));
    assert!(app.editor_state.resource_manifest().image(&id).is_none());
}

#[test]
fn musicvideo_image_unresolved_layer_without_reference_can_be_replaced() {
    let ctx = crate::project::test_context();
    let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
    let id = ImageId::new("unresolved-layer-only");
    app.project
        .layers
        .push(new_image_layer(&app.project, id.clone()));
    let scene = evaluate(&app.project, 0.0, &SpectrumFrame::new(vec![]));
    let mut frame = Frame::new(app.project.output.width, app.project.output.height).unwrap();
    let report = render_frame(&scene, &app.assets, &mut frame).unwrap();
    let notice = unresolved_image_notices(&app.project, &report, &app.image_errors)
        .into_iter()
        .find(|notice| notice.id == id)
        .expect("unresolved material is offered for replacement");
    assert!(notice.path.is_none());
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("resolved.png");
    image::RgbaImage::from_pixel(1, 1, image::Rgba([12, 34, 56, 255]))
        .save(&path)
        .unwrap();
    app.start_image_load(notice.id, path.clone(), &ctx);
    assert!(app.image_pending.is_some(), "{}", app.status);
    settle(&mut app, &ctx);
    assert_image(&app, &id, &path, 12);
    let scene = evaluate(&app.project, 0.0, &SpectrumFrame::new(vec![]));
    let report = render_frame(&scene, &app.assets, &mut frame).unwrap();
    assert!(
        !report
            .unresolved_assets
            .contains(&UnresolvedAsset::Image(id))
    );
}

#[test]
fn musicvideo_image_add_then_replace_keeps_one_reference_and_undoable_pixels() {
    let ctx = crate::project::test_context();
    let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
    let initial = app.project.clone();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("added.png");
    image::RgbaImage::from_pixel(1, 1, image::Rgba([12, 34, 56, 255]))
        .save(&path)
        .unwrap();
    app.start_image_request(ImageLoadTarget::AddLayer, path.clone(), &ctx);
    assert_eq!(app.project, initial);
    settle(&mut app, &ctx);
    assert_eq!(app.project.layers.len(), initial.layers.len() + 1);
    let id = app.project.images[0].id.clone();
    assert_ne!(id, ImageId::new(BACKGROUND_ID));
    assert_image(&app, &id, &path, 12);
    image::RgbaImage::from_pixel(1, 1, image::Rgba([77, 34, 56, 255]))
        .save(&path)
        .unwrap();
    app.start_image_load(id.clone(), path.clone(), &ctx);
    settle(&mut app, &ctx);
    assert_image(&app, &id, &path, 77);
    history(&mut app, &ctx, false);
    assert_image(&app, &id, &path, 12);
    history(&mut app, &ctx, false);
    assert_eq!(app.project, initial);
    assert!(!app.assets.has_image(&id));
    history(&mut app, &ctx, true);
    history(&mut app, &ctx, true);
    assert_image(&app, &id, &path, 77);
}

#[test]
fn p5a_image_add_centers_maximum_aspect_rect_and_contain_through_history() {
    let ctx = crate::project::test_context();
    let temp = tempfile::tempdir().unwrap();
    for (width, height, expected) in [
        (80, 160, NormRect::new(0.375, 0.0, 0.25, 1.0)),
        (160, 40, NormRect::new(0.0, 0.25, 1.0, 0.5)),
        (64, 64, NormRect::new(0.25, 0.0, 0.5, 1.0)),
        (200, 100, NormRect::FULL),
    ] {
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        assert!(
            app.resource_store.is_some(),
            "initialization: {}",
            app.status
        );
        assert!(
            app.restore_worker.is_some(),
            "initialization: {}",
            app.status
        );
        app.project.output.width = 200;
        app.project.output.height = 100;
        let initial = app.project.clone();
        let path = temp.path().join(format!("{width}x{height}.png"));
        image::RgbaImage::from_pixel(width, height, image::Rgba([12, 34, 56, 255]))
            .save(&path)
            .unwrap();
        app.start_image_request(ImageLoadTarget::AddLayer, path.clone(), &ctx);
        assert!(app.image_pending.is_some(), "admission: {}", app.status);
        assert_eq!(
            app.project, initial,
            "placement waits for decoded dimensions"
        );
        settle(&mut app, &ctx);

        let added = app.project.clone();
        assert_eq!(added.images.len(), 1, "{width}x{height}: {}", app.status);
        let id = &added.images[0].id;
        let layer = added
            .layers
            .iter()
            .find(|layer| matches!(&layer.kind, LayerKind::Image { image, .. } if image == id))
            .unwrap();
        assert_eq!(layer.rect, Animatable::Const(expected), "{width}x{height}");
        assert!(matches!(
            layer.kind,
            LayerKind::Image {
                fit: Fit::Contain,
                ..
            }
        ));
        assert_eq!(app.editor_state.history_depths(), (1, 0));
        let revision = app
            .editor_state
            .resource_manifest()
            .image(id)
            .unwrap()
            .revision();
        std::fs::remove_file(&path).unwrap();

        history(&mut app, &ctx, false);
        assert_eq!(app.project, initial);
        assert!(!app.assets.has_image(id));
        history(&mut app, &ctx, true);
        assert_eq!(app.project, added);
        assert_eq!(app.assets.image(id).unwrap().width, width);
        assert_eq!(app.assets.image(id).unwrap().height, height);
        assert_eq!(
            app.editor_state
                .resource_manifest()
                .image(id)
                .unwrap()
                .revision(),
            revision
        );
    }
}

#[test]
fn p5a_image_replace_preserves_existing_rect_and_fit_through_history() {
    let ctx = crate::project::test_context();
    let temp = tempfile::tempdir().unwrap();
    let original_path = temp.path().join("original.png");
    let replacement_path = temp.path().join("replacement.png");
    image::RgbaImage::from_pixel(1, 1, image::Rgba([12, 34, 56, 255]))
        .save(&original_path)
        .unwrap();
    image::RgbaImage::from_pixel(1, 1, image::Rgba([77, 34, 56, 255]))
        .save(&replacement_path)
        .unwrap();
    for expected_fit in [Fit::Cover, Fit::Contain, Fit::Stretch] {
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        let id = ImageId::new(BACKGROUND_ID);
        app.start_image_load(id.clone(), original_path.clone(), &ctx);
        settle(&mut app, &ctx);

        let checkpoint = app.editor_state.begin_external_edit(&app.project);
        let layer = &mut app.project.layers[0];
        layer.rect = Animatable::Const(NormRect::new(0.1, 0.2, 0.6, 0.3));
        if let LayerKind::Image { fit, .. } = &mut layer.kind {
            *fit = expected_fit;
        }
        assert!(
            app.editor_state
                .finish_external_edit(&app.project, checkpoint)
        );
        let before = app.project.clone();

        app.start_image_load(id.clone(), replacement_path.clone(), &ctx);
        settle(&mut app, &ctx);
        let replaced = app.project.clone();
        assert_eq!(replaced.layers, before.layers);
        assert_image(&app, &id, &replacement_path, 77);
        history(&mut app, &ctx, false);
        assert_eq!(app.project, before);
        assert_image(&app, &id, &original_path, 12);
        history(&mut app, &ctx, true);
        assert_eq!(app.project, replaced);
        assert_image(&app, &id, &replacement_path, 77);

        // The pre-replacement Fit/rect edit remains its own undo unit.
        history(&mut app, &ctx, false);
        history(&mut app, &ctx, false);
        assert_eq!(
            app.project.layers[0].rect,
            Animatable::Const(NormRect::FULL)
        );
        assert!(matches!(
            app.project.layers[0].kind,
            LayerKind::Image {
                fit: Fit::Cover,
                ..
            }
        ));
        history(&mut app, &ctx, true);
        assert_eq!(app.project, before);
    }
}
