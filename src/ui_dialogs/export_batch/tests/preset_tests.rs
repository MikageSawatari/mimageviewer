use super::super::watermark::LogoCheckResult;
use super::*;

fn preset(name: &str) -> BatchExportPreset {
    BatchExportPreset {
        name: name.into(),
        options: dialog(false).options,
    }
}

#[test]
fn batch_export_presets_save_replace_recall_delete_and_keep_save_order() {
    let mut state = dialog(false);
    let mut presets = Vec::new();
    state.preset.editing = Some("  印刷用　".into());
    assert!(
        state
            .edit_presets(&mut presets, PresetAction::Save)
            .unwrap()
    );
    assert_eq!(presets, vec![preset("印刷用")]);
    state.preset.editing = Some("送信用".into());
    state
        .edit_presets(&mut presets, PresetAction::Save)
        .unwrap();
    state.options.format.jpeg_quality = 67;
    state.options.resize.mode = BatchResizeMode::Percent(25);
    state.preset.editing = Some(" 印刷用 ".into());
    state
        .edit_presets(&mut presets, PresetAction::Save)
        .unwrap();
    assert_eq!(
        presets.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["印刷用", "送信用"]
    );
    let saved = presets[0].clone();
    state.options = BatchExportOptions::default();
    state.recall_preset(&saved, &egui::Context::default());
    assert_eq!(state.options, saved.options);
    state.options.template = "手で変更".into();
    assert_eq!(presets[0], saved, "recall must copy values without binding");
    let options = state.options.clone();
    state
        .edit_presets(&mut presets, PresetAction::Delete)
        .unwrap();
    assert_eq!(presets, vec![preset("送信用")]);
    assert!(state.preset.selected.is_none());
    assert_eq!(
        state.options, options,
        "delete must not change dialog values"
    );
}

#[test]
fn batch_export_presets_validate_trimmed_unicode_names_and_limit() {
    let mut state = dialog(false);
    let mut presets = Vec::new();
    for name in ["", " \t　 ", &"名".repeat(41)] {
        state.preset.editing = Some(name.into());
        assert!(
            state
                .edit_presets(&mut presets, PresetAction::Save)
                .is_err()
        );
        assert!(presets.is_empty());
    }
    for name in ["名".to_owned(), "名".repeat(40)] {
        state.preset.editing = Some(format!("　{name} "));
        state
            .edit_presets(&mut presets, PresetAction::Save)
            .unwrap();
        assert_eq!(presets.last().unwrap().name, name);
    }
    for n in 2..50 {
        state.preset.editing = Some(format!("プリセット{n}"));
        state
            .edit_presets(&mut presets, PresetAction::Save)
            .unwrap();
    }
    assert_eq!(presets.len(), 50);
    let before = presets.clone();
    state.preset.editing = Some("51件目".into());
    assert!(
        state
            .edit_presets(&mut presets, PresetAction::Save)
            .unwrap_err()
            .contains("50 件")
    );
    assert_eq!(presets, before);
    state.preset.editing = Some(" 名 ".into());
    state.options.keep_metadata = true;
    state
        .edit_presets(&mut presets, PresetAction::Save)
        .unwrap();
    assert_eq!(presets.len(), 50);
    assert!(presets[0].options.keep_metadata);
    state
        .edit_presets(&mut presets, PresetAction::Delete)
        .unwrap();
    state.preset.editing = Some("空いた枠".into());
    state
        .edit_presets(&mut presets, PresetAction::Save)
        .unwrap();
    assert_eq!(presets.len(), 50);
    assert_eq!(presets.last().unwrap().name, "空いた枠");
}

#[test]
fn batch_export_presets_default_roundtrip_and_preferences_preserve_live_values() {
    let missing: crate::settings::Settings = serde_json::from_str("{}").unwrap();
    assert!(missing.export_batch_presets.is_empty());
    let mut live = missing.clone();
    live.export_batch_presets = vec![preset("保存した値")];
    let encoded = serde_json::to_string(&live).unwrap();
    let loaded: crate::settings::Settings = serde_json::from_str(&encoded).unwrap();
    assert_eq!(loaded.export_batch_presets, live.export_batch_presets);
    let mut draft = missing;
    draft.overwrite_non_preferences_from(&mut live);
    assert_eq!(draft.export_batch_presets, loaded.export_batch_presets);
    let transfer = crate::settings_transfer::export_preferences(&draft).unwrap();
    assert!(!transfer.json.contains("export_batch_presets"));
    assert!(!transfer.json.contains("保存した値"));
}

#[test]
fn batch_export_presets_name_textedit_enter_saves_once_without_export() {
    let mut h = window_harness(egui::vec2(1093.0, 900.0), 1.0, false);
    h.run_steps(12);
    h.get_by_label("保存...").click();
    h.run_steps(2);
    h.get_by_label("プリセット名").focus();
    h.step();
    h.get_by_label("プリセット名").type_text(" 作者用 ");
    h.run_steps(2);
    assert_eq!(
        h.get_by_label("プリセット名").accesskit_node().role(),
        egui::accesskit::Role::TextInput
    );
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!((h.state().saves, h.state().starts), (1, 0));
    assert_eq!(h.state().presets, vec![preset("作者用")]);
    assert!(h.state().dialog.preset.editing.is_none());
    h.get_by_label("保存...").click();
    h.run_steps(2);
    h.get_by_label("上書き保存").click();
    h.run_steps(2);
    assert_eq!((h.state().saves, h.state().starts), (2, 0));
    assert_eq!(h.state().presets.len(), 1);
}

#[test]
fn batch_export_presets_same_frame_name_click_and_enter_never_exports() {
    let mut h = window_harness(egui::vec2(1093.0, 900.0), 1.0, false);
    h.run_steps(12);
    h.get_by_label("保存...").click();
    h.run_steps(2);
    h.state_mut().dialog.preset.editing = Some("同時入力".into());
    h.get_by_label("ファイル名").focus();
    h.step();
    let pos = h.get_by_label("プリセット名").rect().center();
    h.input_mut().events.extend([
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    h.step();
    h.run_steps(2);
    assert_eq!((h.state().saves, h.state().starts), (0, 0));
}

#[test]
fn batch_export_presets_name_ime_and_invalid_enter_never_export() {
    let mut h = window_harness(egui::vec2(1093.0, 900.0), 1.0, false);
    h.run_steps(12);
    h.get_by_label("保存...").click();
    h.run_steps(2);
    assert!(h.get_by_label("保存").accesskit_node().is_disabled());
    h.get_by_label("プリセット名").focus();
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!((h.state().saves, h.state().starts), (0, 0));
    assert!(h.state().dialog.error.as_ref().unwrap().contains("1〜40"));
    h.get_by_label("プリセット名").focus();
    h.step();
    h.event(egui::Event::Ime(egui::ImeEvent::Enabled));
    h.event(egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "作者".into(),
        active_range_chars: None,
    }));
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!((h.state().saves, h.state().starts), (0, 0));
    h.event(egui::Event::Ime(egui::ImeEvent::Commit("作者".into())));
    h.event(egui::Event::Ime(egui::ImeEvent::Disabled));
    h.run_steps(2);
    h.get_by_label("プリセット名").focus();
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!((h.state().saves, h.state().starts), (1, 0));
    assert_eq!(h.state().presets[0].name, "作者");
}

#[test]
fn batch_export_presets_back_after_empty_name_enter_clears_only_name_error() {
    for (edit_name, other_error) in [
        (false, None),
        (true, None),
        (
            false,
            Some("保存先・ファイル名と書き出す画像を確認してください"),
        ),
        (false, Some("プリセットは 50 件まで保存できます")),
    ] {
        let mut h = window_harness(egui::vec2(1093.0, 900.0), 1.0, false);
        h.run_steps(12);
        h.get_by_label("保存...").click();
        h.run_steps(2);
        h.get_by_label("プリセット名").focus();
        h.step();
        h.key_press(egui::Key::Enter);
        h.run_steps(2);
        assert!(h.state().dialog.error.as_ref().unwrap().contains("1〜40"));
        if edit_name {
            // Cancelling must clear the earlier error even after the name changes.
            h.get_by_label("プリセット名").focus();
            h.step();
            h.get_by_label("プリセット名").type_text("保存しない名前");
            h.run_steps(2);
        }
        if let Some(error) = other_error {
            h.state_mut().dialog.error = Some(error.into());
            h.run_steps(2);
        }
        h.get_by_label("戻る").click();
        h.run_steps(2);
        assert!(h.state().dialog.preset.editing.is_none());
        assert_eq!(h.state().dialog.error.as_deref(), other_error);
        assert!(h.state().presets.is_empty());
        assert_eq!((h.state().saves, h.state().starts), (0, 0));
        let _ = h.get_by_label("保存...");
    }
}

#[test]
fn batch_export_presets_popup_consumes_wheel_and_enter() {
    let mut h = window_harness(egui::vec2(1093.0, 614.0), 1.0, true);
    h.state_mut().presets = (0..50).map(|n| preset(&format!("設定{n:02}"))).collect();
    h.run_steps(12);
    h.get_by_label("プリセット").focus();
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(egui::Popup::is_any_open(&h.ctx));
    assert_eq!(h.state().starts, 0);
    let before = h
        .ctx
        .data(|d| d.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_layout")))
        .unwrap();
    let pos = h.get_by_label("設定00").rect().center();
    h.event(egui::Event::PointerMoved(pos));
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -100000.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
    });
    h.run_steps(32);
    let after = h
        .ctx
        .data(|d| d.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_layout")))
        .unwrap();
    assert_eq!(before.offset, after.offset);
    // The ComboBox's own scroll area must reach the last saved entry.
    let last = h.get_by_label("設定49").rect();
    assert!(h.ctx.content_rect().contains_rect(last));
    h.get_by_label("設定49").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog.preset.selected.as_deref(), Some("設定49"));
    assert_eq!(h.state().starts, 0);
}

#[test]
fn batch_export_presets_recall_rechecks_same_logo_and_discards_old_receiver() {
    let mut h = window_harness(egui::vec2(1093.0, 1800.0), 1.0, false);
    let mut saved = preset("ロゴ付き");
    saved.options.watermark = BatchWatermark {
        enabled: true,
        kind: WatermarkKind::Image,
        image_path: Some("logo.svg".into()),
        ..Default::default()
    };
    h.state_mut().presets.push(saved.clone());
    h.state_mut().dialog.options = saved.options.clone();
    let (tx, rx) = mpsc::channel();
    h.state_mut().dialog.logo_check = Some(LogoCheck {
        path: Some("logo.svg".into()),
        result: LogoCheckResult::Pending(rx),
    });
    h.run_steps(12);
    h.get_by_label("プリセット").click();
    h.run_steps(2);
    h.get_by_label("ロゴ付き").click();
    h.run_steps(2);
    assert!(tx.send(true).is_err());
    assert!(matches!(
        h.state().dialog.logo_check.as_ref().unwrap().result,
        LogoCheckResult::Unsupported
    ));
    assert!(h.get_by_label("書き出す").accesskit_node().is_disabled());
    assert_eq!(h.state().starts, 0);
}

#[test]
fn batch_export_presets_row_stays_within_body_width_at_all_scales() {
    for scale in [1.0, 2.0] {
        // Default, selected, new name, and overwrite name cover both sets of
        // trailing buttons, including the widest allowed Japanese preset name.
        for mode in 0..4 {
            let mut h = window_harness(egui::vec2(1093.0, 614.0), scale, mode != 0);
            if mode != 0 {
                let saved = preset(&"長い名前".repeat(10));
                h.state_mut().dialog.preset.selected = Some(saved.name.clone());
                if mode >= 2 {
                    h.state_mut().dialog.preset.editing = Some(if mode == 2 {
                        "新しい名前".into()
                    } else {
                        saved.name.clone()
                    });
                }
                h.state_mut().presets.push(saved);
            }
            h.run_steps(12);
            let body = h
                .ctx
                .data(|data| data.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_layout")))
                .unwrap();
            let labels = match mode {
                0 | 1 => ["プリセット", "保存...", "削除"],
                2 => ["プリセット名", "保存", "戻る"],
                _ => ["プリセット名", "上書き保存", "戻る"],
            };
            let tolerance = 1.0 / h.ctx.pixels_per_point();
            for label in labels {
                let rect = h.get_by_label(label).rect();
                assert!(
                    rect.left() >= body.inner.left() - tolerance
                        && rect.right() <= body.inner.right() + tolerance,
                    "preset row exceeds body width: scale={scale}, mode={mode}, label={label}, rect={rect:?}, body={body:?}",
                );
            }
        }
    }
}

#[test]
fn batch_export_presets_small_viewports_keep_fixed_rows_and_reach_last_setting() {
    for scale in [1.0, 2.0] {
        for editing in [false, true] {
            let mut h = window_harness(egui::vec2(1093.0, 614.0), scale, true);
            let saved = preset(&"長い名前".repeat(10));
            h.state_mut().dialog.preset.selected = Some(saved.name.clone());
            if editing {
                h.state_mut().dialog.preset.editing = Some(saved.name.clone());
            }
            h.state_mut().presets.push(saved);
            h.state_mut().dialog.options.watermark.enabled = true;
            h.run_steps(12);
            let bounds = h.ctx.content_rect();
            let labels = if editing {
                ["プリセット名", "上書き保存", "戻る"]
            } else {
                ["プリセット", "保存...", "削除"]
            };
            for label in labels
                .into_iter()
                .chain(["書き出す", "キャンセル", "Close window"])
            {
                assert!(
                    bounds.contains_rect(h.get_by_label(label).rect()),
                    "{scale}/{editing}: {label}"
                );
            }
            let fixed = h.get_by_label(labels[0]).rect();
            let before = h
                .ctx
                .data(|d| d.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_layout")))
                .unwrap();
            before.assert_height(HeightExpectation::Scrolls, bounds, h.ctx.pixels_per_point());
            assert_long_dialog_uses_remaining_height(&h.ctx, "一括エクスポート");
            wheel(&mut h, before);
            let after = h
                .ctx
                .data(|d| d.get_temp::<ScrollLayout>(egui::Id::unique("batch_export_layout")))
                .unwrap();
            after.assert_scrolled_to_end(before, h.ctx.pixels_per_point());
            assert_eq!(fixed, h.get_by_label(labels[0]).rect());
            assert!(after.inner.contains_rect(h.get_by_label("不透明度").rect()));
            h.get_by_label(if editing { "戻る" } else { "削除" })
                .click();
            h.run_steps(2);
            if editing {
                assert!(h.state().dialog.preset.editing.is_none());
            } else {
                assert!(h.state().presets.is_empty());
            }
        }
    }
}

fn scroll_to(h: &mut Harness<'_, crate::app::AppTestEnvForTest>, label: &str) {
    h.get_by_label(label).scroll_to_me();
    h.run_steps(4);
}

fn replace_text(h: &mut Harness<'_, crate::app::AppTestEnvForTest>, label: &str, text: &str) {
    scroll_to(h, label);
    h.get_by_label(label).focus();
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
        });
        h.step();
    }
    h.get_by_label(label).type_text(text);
    h.run_steps(2);
}

fn set_output_options(
    h: &mut Harness<'_, crate::app::AppTestEnvForTest>,
    directory: &std::path::Path,
) {
    replace_text(h, "保存先", &directory.display().to_string());
    h.get_by_label("元のフォルダ構造を保つ").click();
    h.run_steps(2);
    replace_text(h, "ファイル名", "preset_<num:2>");
    scroll_to(h, "ファイル形式");
    h.get_by_label("ファイル形式").click();
    h.run_steps(2);
    h.get_by_label("WebP").click();
    h.run_steps(2);
    scroll_to(h, "WebP 品質");
    h.get_by_label("WebP 品質").set_value_accesskit(83.0);
    h.run_steps(2);
    h.get_by_label("WebP 可逆圧縮").click();
    h.run_steps(2);
    for label in [
        "元の情報 (EXIF・XMP・AI プロンプト・色) を残す",
        "元の更新日時にする",
    ] {
        scroll_to(h, label);
        h.get_by_label(label).click();
        h.run_steps(2);
    }
    scroll_to(h, "リサイズ");
    h.get_by_label("リサイズ").click();
    h.run_steps(2);
    h.get_by_label("割合").click();
    h.run_steps(2);
}

fn set_logo_options(h: &mut Harness<'_, crate::app::AppTestEnvForTest>, logo: &std::path::Path) {
    scroll_to(h, "透かしを入れる");
    h.get_by_label("透かしを入れる").click();
    h.run_steps(2);
    scroll_to(h, "画像");
    h.get_by_label("画像").click();
    h.run_steps(2);
    replace_text(h, "ロゴのパス", &logo.display().to_string());
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(h.state().export_pending.is_none());
    scroll_to(h, "ロゴの大きさ");
    h.get_by_label("ロゴの大きさ").click();
    h.run_steps(2);
    h.get_by_label("実寸").click();
    h.run_steps(2);
    for (label, value) in [("余白 (短辺に対する割合)", 0.0), ("不透明度", 100.0)] {
        scroll_to(h, label);
        h.get_by_label(label).set_value_accesskit(value);
        h.run_steps(2);
    }
}

fn save_from_name_field(h: &mut Harness<'_, crate::app::AppTestEnvForTest>, name: &str) {
    h.get_by_label("保存...").click();
    h.run_steps(2);
    replace_text(h, "プリセット名", name);
    let before = crate::settings::save_generation();
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(
        crate::settings::save_generation(),
        before + 1,
        "exactly one settings save"
    );
    assert!(
        h.state().export_pending.is_none(),
        "preset Enter must not export"
    );
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
fn batch_export_presets_dialog_save_settings_reopen_recall_and_worker_effect() {
    let mut app = crate::app::tests::phase_c_support::setup_app_with_fresh_data_dir();
    let source = app.tmp.path().join("input.png");
    image::RgbaImage::from_pixel(12, 8, image::Rgba([20, 40, 60, 255]))
        .save(&source)
        .unwrap();
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
    std::fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_modified(time)
        .unwrap();
    let logo = app.tmp.path().join("logo.png");
    image::RgbaImage::from_pixel(1, 1, image::Rgba([200, 0, 0, 255]))
        .save(&logo)
        .unwrap();
    let directory = app.tmp.path().join("プリセットの出力先");
    app.settings.export_batch_directory = Some(app.tmp.path().to_owned());
    app.settings.export_batch_scale = crate::export_dialog::ExportScale::Full;
    app.items.push(crate::grid_item::GridItem::Image(source));
    app.thumbnails
        .push(crate::grid_item::ThumbnailState::Pending);
    app.visible_indices = vec![0];
    app.checked.insert(0);
    app.open_export_batch_dialog(&egui::Context::default());
    let mut h = app_harness(app);
    h.run_steps(12);
    set_output_options(&mut h, &directory);
    set_logo_options(&mut h, &logo);
    let options = h
        .state()
        .export_batch_dialog
        .as_ref()
        .unwrap()
        .options
        .clone();
    assert_eq!(options.resize.mode, BatchResizeMode::Percent(50));
    assert_eq!(options.format.webp_quality, 83);
    save_from_name_field(&mut h, " 作者用 ");
    let saved = BatchExportPreset {
        name: "作者用".into(),
        options: options.clone(),
    };
    assert_eq!(h.state().settings.export_batch_presets, [saved.clone()]);
    assert!(
        h.state().settings.export_batch_options.is_none(),
        "saving a preset does not start export"
    );
    let db = crate::settings_db::SettingsDb::open(h.state().tmp.path()).unwrap();
    let disk = db.load_into_settings().unwrap();
    assert_eq!(disk.export_batch_presets, [saved.clone()]);
    h.get_by_label("キャンセル").click();
    h.run_steps(2);
    assert!(h.state().export_batch_dialog.is_none());
    // Reopen from serialized settings, then deliberately edit different values.
    h.state_mut().settings.export_batch_presets = disk.export_batch_presets;
    let ctx = h.ctx.clone();
    h.state_mut().open_export_batch_dialog(&ctx);
    h.run_steps(4);
    replace_text(&mut h, "ファイル名", "別の名前");
    assert_ne!(
        h.state().export_batch_dialog.as_ref().unwrap().options,
        options
    );
    h.get_by_label("プリセット").click();
    h.run_steps(2);
    h.get_by_label("作者用").click();
    h.run_steps(2);
    assert_eq!(
        h.state().export_batch_dialog.as_ref().unwrap().options,
        options
    );
    assert!(
        h.state()
            .export_batch_dialog
            .as_ref()
            .unwrap()
            .logo_check
            .is_some()
    );
    h.get_by_label("書き出す").click();
    h.run_steps(2);
    assert!(h.state().export_batch_dialog.is_none());
    let wrote = receive_output(&h);
    assert_eq!(wrote, directory.join("preset_01.webp"));
    assert_eq!(
        image::guess_format(&std::fs::read(&wrote).unwrap()).unwrap(),
        image::ImageFormat::WebP
    );
    let pixels = image::open(&wrote).unwrap().into_rgba8();
    assert_eq!(pixels.dimensions(), (6, 4));
    assert_eq!(pixels.get_pixel(0, 0).0, [20, 40, 60, 255]);
    assert_eq!(pixels.get_pixel(5, 3).0, [200, 0, 0, 255]);
    assert_eq!(std::fs::metadata(&wrote).unwrap().modified().unwrap(), time);
    let disk = db.load_into_settings().unwrap();
    assert_eq!(disk.export_batch_options, Some(options));
    assert_eq!(disk.export_batch_presets, [saved]);
}

#[test]
fn batch_export_presets_ui_replace_delete_persist_and_failure_notifies_without_rollback() {
    let mut app = crate::app::tests::phase_c_support::setup_app_with_fresh_data_dir();
    app.export_batch_dialog = Some(dialog(false));
    let mut h = app_harness(app);
    h.run_steps(12);
    save_from_name_field(&mut h, "保存済み");
    let db = crate::settings_db::SettingsDb::open(h.state().tmp.path()).unwrap();
    let saved = db.load_into_settings().unwrap().export_batch_presets;
    assert_eq!(saved[0].name, "保存済み");
    h.state_mut()
        .export_batch_dialog
        .as_mut()
        .unwrap()
        .options
        .keep_metadata = true;
    h.get_by_label("保存...").click();
    h.run_steps(2);
    h.get_by_label("上書き保存").click();
    h.run_steps(2);
    assert_eq!(h.state().settings.export_batch_presets.len(), 1);
    assert!(
        db.load_into_settings().unwrap().export_batch_presets[0]
            .options
            .keep_metadata
    );
    h.get_by_label("削除").click();
    h.run_steps(2);
    assert!(h.state().settings.export_batch_presets.is_empty());
    assert!(
        db.load_into_settings()
            .unwrap()
            .export_batch_presets
            .is_empty()
    );
    assert!(
        h.state().export_batch_dialog.is_some(),
        "no confirmation dialog"
    );
    // The fixture owns the test-global settings lock and resets suppression on drop.
    crate::settings_db::set_save_suppressed(true);
    h.get_by_label("保存...").click();
    h.run_steps(2);
    replace_text(&mut h, "プリセット名", "未保存");
    let before = crate::settings::save_generation();
    h.get_by_label("保存").click();
    h.run_steps(2);
    assert_eq!(crate::settings::save_generation(), before);
    assert_eq!(h.state().settings.export_batch_presets[0].name, "未保存");
    assert!(
        h.state()
            .fs_feedback_toast
            .as_ref()
            .unwrap()
            .0
            .contains("プリセットを保存できませんでした")
    );
    crate::settings_db::set_save_suppressed(false);
    assert!(
        db.load_into_settings()
            .unwrap()
            .export_batch_presets
            .is_empty()
    );
    // A later ordinary settings save includes the in-memory change.
    assert!(h.state().settings.save_checked());
    let pending_save = db.load_into_settings().unwrap().export_batch_presets;
    assert_eq!(pending_save, h.state().settings.export_batch_presets);
    crate::settings_db::set_save_suppressed(true);
    h.state_mut().fs_feedback_toast = None;
    h.get_by_label("削除").click();
    h.run_steps(2);
    assert!(h.state().settings.export_batch_presets.is_empty());
    assert!(
        h.state()
            .fs_feedback_toast
            .as_ref()
            .unwrap()
            .0
            .contains("プリセットを保存できませんでした")
    );
    crate::settings_db::set_save_suppressed(false);
    assert_eq!(
        db.load_into_settings().unwrap().export_batch_presets,
        pending_save
    );
    assert!(h.state().export_pending.is_none());
}

mod gpu_tests {
    use super::*;
    #[test]
    fn batch_export_snapshot_preset_selected_and_name_editor() {
        let mut snapshots = egui_kittest::SnapshotResults::default();
        for editing in [false, true] {
            let mut h = window_harness(egui::vec2(1093.0, 614.0), 1.0, true);
            h.state_mut().presets.push(preset("作者のロゴ付き"));
            h.state_mut().dialog.preset.selected = Some("作者のロゴ付き".into());
            if editing {
                h.state_mut().dialog.preset.editing = Some("作者のロゴ付き".into());
            }
            h.run_steps(12);
            h.snapshot(if editing {
                "batch_export_preset_name"
            } else {
                "batch_export_preset_selected"
            });
            snapshots.extend(h.take_snapshot_results());
        }
    }
}
