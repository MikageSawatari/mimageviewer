use super::super::watermark::LogoCheckResult;
use super::*;

fn enable_image(h: &mut Harness<'_, WindowState>, path: &str) {
    h.state_mut().dialog.options.watermark = BatchWatermark {
        enabled: true,
        kind: WatermarkKind::Image,
        image_path: Some(path.into()),
        ..Default::default()
    };
    h.run_steps(4);
}

#[test]
fn batch_export_watermark_logo_errors_disable_start_and_enter_only_checks() {
    let mut h = window_harness(egui::vec2(1093.0, 1800.0), 1.0, false);
    enable_image(&mut h, "logo.svg");
    assert!(h.get_by_label("書き出す").accesskit_node().is_disabled());
    let _ = h.get_by_label("この形式は透かしに使えません");
    h.state_mut().dialog.options.watermark.image_path = Some("missing.png".into());
    h.state_mut().dialog.logo_check = Some(LogoCheck {
        path: Some("missing.png".into()),
        result: LogoCheckResult::Missing,
    });
    h.run_steps(2);
    assert!(h.get_by_label("書き出す").accesskit_node().is_disabled());
    let _ = h.get_by_label("透かしの画像が見つかりません: missing.png");
    // Hold a dedicated pending channel open, without waiting for disk or a thread.
    let (tx, rx) = mpsc::channel();
    h.state_mut().dialog.logo_check = Some(LogoCheck {
        path: Some("missing.png".into()),
        result: LogoCheckResult::Pending(rx),
    });
    h.run_steps(2);
    assert!(!h.get_by_label("書き出す").accesskit_node().is_disabled());
    h.get_by_label("ロゴのパス").focus();
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().starts, 0);
    assert!(
        tx.send(true).is_err(),
        "Enter must replace the check, including for the same path"
    );
    h.get_by_label("透かしを入れる").click();
    h.run_steps(2);
    assert!(!h.get_by_label("書き出す").accesskit_node().is_disabled());
}

#[test]
fn batch_export_watermark_small_viewports_keep_actions_and_reach_opacity() {
    for size in [egui::vec2(1093.0, 614.0), egui::vec2(1366.0, 728.0)] {
        for scale in [1.0, 2.0] {
            for kind in [WatermarkKind::Text, WatermarkKind::Image] {
                let mut h = window_harness(size, scale, true);
                h.state_mut().dialog.options.watermark.enabled = true;
                h.state_mut().dialog.options.watermark.kind = kind;
                h.state_mut().dialog.options.watermark.text = "作者の透かし".into();
                if kind == WatermarkKind::Image {
                    h.state_mut().dialog.options.watermark.image_path = Some("logo.png".into());
                    h.state_mut().dialog.logo_check = Some(LogoCheck {
                        path: Some("logo.png".into()),
                        result: LogoCheckResult::Found,
                    });
                }
                h.run_steps(12);
                let bounds = h.ctx.content_rect();
                for label in ["Close window", "書き出す", "キャンセル"] {
                    assert!(
                        bounds.contains_rect(h.get_by_label(label).rect()),
                        "{size:?}/{scale}/{kind:?}: {label}"
                    );
                }
                let before = h
                    .ctx
                    .data(|d| d.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_layout")))
                    .unwrap();
                before.assert_height(HeightExpectation::Scrolls, bounds, h.ctx.pixels_per_point());
                let start = h.get_by_label("書き出す").rect();
                wheel(&mut h, before);
                let after = h
                    .ctx
                    .data(|d| d.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_layout")))
                    .unwrap();
                after.assert_scrolled_to_end(before, h.ctx.pixels_per_point());
                assert_eq!(start, h.get_by_label("書き出す").rect());
                assert!(after.inner.contains_rect(h.get_by_label("不透明度").rect()));
                h.get_by_label("書き出す").click();
                h.run_steps(2);
                assert_eq!(h.state().starts, 1);
            }
        }
    }
}

#[test]
fn batch_export_watermark_color_popup_consumes_wheel_and_enter() {
    let mut h = window_harness(egui::vec2(1093.0, 614.0), 1.0, true);
    h.state_mut().dialog.options.watermark.enabled = true;
    h.state_mut().dialog.options.watermark.text = "作者".into();
    h.run_steps(12);
    h.get_by_label("文字色 #FFFFFF").scroll_to_me();
    h.run_steps(8);
    h.get_by_label("文字色 #FFFFFF").click();
    h.run_steps(2);
    assert!(egui::Popup::is_any_open(&h.ctx));
    let before = h
        .ctx
        .data(|d| d.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_layout")))
        .unwrap();
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -200.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
    });
    h.run_steps(16);
    let after = h
        .ctx
        .data(|d| d.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_layout")))
        .unwrap();
    assert_eq!(before.offset, after.offset);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().starts, 0);
}

#[test]
fn batch_export_watermark_text_enter_and_ime_have_one_owner() {
    let mut h = window_harness(egui::vec2(1093.0, 1800.0), 1.0, false);
    h.state_mut().dialog.options.watermark.enabled = true;
    h.run_steps(12);
    h.get_by_label("透かしの文字").focus();
    h.step();
    h.event(egui::Event::Ime(egui::ImeEvent::Enabled));
    h.event(egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "作者".into(),
        active_range_chars: None,
    }));
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().starts, 0);
    h.event(egui::Event::Ime(egui::ImeEvent::Commit("作者".into())));
    h.event(egui::Event::Ime(egui::ImeEvent::Disabled));
    h.run_steps(2);
    h.get_by_label("透かしの文字").focus();
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().starts, 1);
}

fn scroll_to(h: &mut Harness<'_, crate::app::AppTestEnvForTest>, label: &str) {
    h.get_by_label(label).scroll_to_me();
    h.run_steps(4);
}

fn receive_output(h: &Harness<'_, crate::app::AppTestEnvForTest>) -> std::path::PathBuf {
    let mut path = None;
    loop {
        match h
            .state()
            .export_pending
            .as_ref()
            .unwrap()
            .rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap()
        {
            ExportEvent::Completed(success) => path = Some(success.path),
            ExportEvent::Failed(error) => panic!("{}", error.message),
            ExportEvent::AllDone => return path.expect("one exported image"),
            ExportEvent::Cancelled => panic!("unexpected cancellation"),
            _ => {}
        }
    }
}

#[test]
fn batch_export_watermark_dialog_edit_start_persist_reopen_and_worker_effect() {
    for kind in [WatermarkKind::Text, WatermarkKind::Image] {
        let mut app = crate::app::tests::phase_c_support::setup_app_with_fresh_data_dir();
        let source = app.tmp.path().join("input.png");
        image::RgbaImage::from_pixel(160, 120, image::Rgba([20, 40, 60, 255]))
            .save(&source)
            .unwrap();
        let logo = app.tmp.path().join("logo.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 0, 0, 255]))
            .save(&logo)
            .unwrap();
        app.settings.export_batch_directory = Some(app.tmp.path().join("out"));
        // This existing legacy preference avoids a filesystem check of a missing folder.
        std::fs::create_dir_all(app.settings.export_batch_directory.as_ref().unwrap()).unwrap();
        app.settings.export_batch_template = "<filename>".into();
        app.settings.export_batch_format = crate::capture::CaptureFormat::Png;
        app.settings.export_batch_scale = crate::export_dialog::ExportScale::Full;
        app.items.push(crate::grid_item::GridItem::Image(source));
        app.thumbnails
            .push(crate::grid_item::ThumbnailState::Pending);
        app.visible_indices = vec![0];
        app.checked.insert(0);
        app.open_export_batch_dialog(&egui::Context::default());
        let mut h = app_harness(app);
        h.run_steps(12);
        scroll_to(&mut h, "透かしを入れる");
        h.get_by_label("透かしを入れる").click();
        h.run_steps(2);
        match kind {
            WatermarkKind::Text => {
                scroll_to(&mut h, "透かしの文字");
                h.get_by_label("透かしの文字").focus();
                h.step();
                h.get_by_label("透かしの文字").type_text("作者 Logo");
                h.run_steps(2);
                h.get_by_label("文字の高さ (短辺に対する割合)")
                    .set_value_accesskit(30.0);
                h.run_steps(2);
            }
            WatermarkKind::Image => {
                scroll_to(&mut h, "画像");
                h.get_by_label("画像").click();
                h.run_steps(2);
                scroll_to(&mut h, "ロゴのパス");
                h.get_by_label("ロゴのパス").focus();
                h.step();
                h.get_by_label("ロゴのパス")
                    .type_text(&logo.display().to_string());
                h.run_steps(2);
                h.key_press(egui::Key::Enter);
                h.run_steps(2);
                assert!(
                    h.state().export_batch_dialog.is_some(),
                    "logo Enter must only check"
                );
            }
        }
        scroll_to(&mut h, "不透明度");
        h.get_by_label("不透明度").set_value_accesskit(100.0);
        h.get_by_label("余白 (短辺に対する割合)")
            .set_value_accesskit(0.0);
        h.run_steps(2);
        h.get_by_label("書き出す").click();
        h.run_steps(2);
        assert!(h.state().export_batch_dialog.is_none());
        let saved = h.state().settings.export_batch_options.clone().unwrap();
        assert!(saved.watermark.enabled);
        assert_eq!(saved.watermark.kind, kind);
        assert_eq!(saved.watermark.opacity_percent, 100);
        assert_eq!(saved.watermark.margin_percent, 0);
        if kind == WatermarkKind::Text {
            assert_eq!(saved.watermark.text, "作者 Logo");
            assert_eq!(saved.watermark.text_size_percent, 30);
        } else {
            assert_eq!(saved.watermark.image_path, Some(logo));
        }
        let db = crate::settings_db::SettingsDb::open(h.state().tmp.path()).unwrap();
        assert_eq!(
            db.load_into_settings().unwrap().export_batch_options,
            Some(saved.clone())
        );
        let first = receive_output(&h);
        let pixels = image::open(&first).unwrap().into_rgba8();
        assert!(
            pixels.pixels().any(|pixel| pixel.0 != [20, 40, 60, 255]),
            "{kind:?} must affect exported pixels"
        );
        assert_eq!(pixels.get_pixel(0, 0).0, [20, 40, 60, 255]);
        if kind == WatermarkKind::Image {
            assert_eq!(pixels.get_pixel(159, 119).0, [200, 0, 0, 255]);
        }
        h.state_mut().export_pending = None;
        h.state_mut().settings.export_batch_options =
            db.load_into_settings().unwrap().export_batch_options;
        let ctx = h.ctx.clone();
        h.state_mut().open_export_batch_dialog(&ctx);
        h.run_steps(4);
        assert_eq!(
            h.state().export_batch_dialog.as_ref().unwrap().options,
            saved
        );
        h.get_by_label("書き出す").click();
        h.run_steps(2);
        let second = receive_output(&h);
        assert_ne!(first, second);
        assert_eq!(image::open(second).unwrap().into_rgba8(), pixels);
    }
}

mod gpu_tests {
    use super::*;
    #[test]
    fn batch_export_snapshot_watermark_text_and_image() {
        let mut snapshots = egui_kittest::SnapshotResults::default();
        for kind in [WatermarkKind::Text, WatermarkKind::Image] {
            let mut h = window_harness(egui::vec2(1093.0, 614.0), 1.0, true);
            h.state_mut().dialog.options.watermark.enabled = true;
            h.state_mut().dialog.options.watermark.kind = kind;
            h.state_mut().dialog.options.watermark.text = "作者の透かし".into();
            if kind == WatermarkKind::Image {
                h.state_mut().dialog.options.watermark.image_path = Some("logo.png".into());
                h.state_mut().dialog.logo_check = Some(LogoCheck {
                    path: Some("logo.png".into()),
                    result: LogoCheckResult::Found,
                });
            }
            h.run_steps(12);
            h.get_by_label("不透明度").scroll_to_me();
            h.run_steps(12);
            h.snapshot(match kind {
                WatermarkKind::Text => "batch_export_watermark_text",
                WatermarkKind::Image => "batch_export_watermark_image",
            });
            snapshots.extend(h.take_snapshot_results());
        }
    }
}
