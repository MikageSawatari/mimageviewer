//! Production Window geometry, with synthetic input only (no native window/profile).
use super::*;
use egui_kittest::kittest::Queryable;

fn window_rect(ctx: &egui::Context, title: &str) -> egui::Rect {
    ctx.memory(|m| m.area_rect(egui::Id::unique(Some(title))))
        .unwrap()
}

fn scroll_rect(ctx: &egui::Context) -> egui::Rect {
    ctx.data_mut(|d| d.get_temp::<egui::Rect>(egui::Id::unique(Some("effect_picker_scroll_1727"))))
        .unwrap()
}

fn assert_columns<T>(h: &egui_kittest::Harness<'_, (T, String)>) {
    assert_columns_with_tolerance(h, 0.1);
}

fn assert_columns_with_tolerance<T>(h: &egui_kittest::Harness<'_, (T, String)>, tolerance: f32) {
    let inner = scroll_rect(&h.ctx);
    let min_width = EFFECT_GROUPS
        .iter()
        .flat_map(|g| g.kinds)
        .map(|kind| {
            h.ctx.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(
                        kind.picker_label().into(),
                        egui::FontId::proportional(12.0),
                        egui::Color32::WHITE,
                    )
                    .size()
                    .x
            }) + h.ctx.global_style().spacing.button_padding.x * 2.0
        })
        .fold(0.0, f32::max);
    let min_width = (min_width / egui::emath::GUI_ROUNDING).ceil() * egui::emath::GUI_ROUNDING;
    let columns = ((inner.width() + 6.0) / (min_width + 6.0)).floor() as usize;
    assert!(columns >= 1);
    for group in EFFECT_GROUPS {
        let rects: Vec<_> = group
            .kinds
            .iter()
            .filter(|kind| effect_picker_matches_query(**kind, &h.state().1))
            .map(|kind| {
                h.get_by(|n| {
                    n.role() == egui::accesskit::Role::Button
                        && n.label().as_deref() == Some(kind.picker_label())
                })
                .rect()
            })
            .collect();
        for row in rects.chunks(columns) {
            assert!(
                (row[0].left() - inner.left()).abs() <= tolerance,
                "inner={inner:?}, row={row:?}"
            );
            for (index, rect) in row.iter().enumerate() {
                assert!(
                    (rect.top() - row[0].top()).abs() <= tolerance,
                    "{}: {rects:?}",
                    group.title
                );
                assert!(rect.right() <= inner.right() + tolerance);
                if index > 0 {
                    assert!((rect.left() - row[index - 1].right() - 6.0).abs() <= tolerance);
                }
            }
        }
        if rects.len() > columns {
            assert!(rects[columns].top() >= rects[0].bottom() + 6.0 - tolerance);
        }
    }
}

fn assert_painted_label(
    shape: &egui::Shape,
    clip: egui::Rect,
    label: &str,
    button: egui::Rect,
) -> usize {
    match shape {
        egui::Shape::Vec(shapes) => shapes
            .iter()
            .map(|s| assert_painted_label(s, clip, label, button))
            .sum(),
        egui::Shape::Text(text)
            if text.galley.text() == label && button.intersects(text.visual_bounding_rect()) =>
        {
            assert_eq!(text.galley.rows.len(), 1);
            let ink = text.visual_bounding_rect();
            assert!(
                button.contains_rect(ink),
                "{label}: button={button:?}, ink={ink:?}"
            );
            assert!(
                clip.contains_rect(ink),
                "{label}: clip={clip:?}, ink={ink:?}"
            );
            1
        }
        _ => 0,
    }
}

// Keep each pointer transition in its own pass, like an actual resize gesture.
macro_rules! drag {
    ($h:expr, $from:expr, $to:expr) => {{
        let from = $from;
        let to = $to;
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        for event in [
            egui::Event::PointerMoved(from),
            button(from, true),
            egui::Event::PointerMoved(from.lerp(to, 0.5)),
            egui::Event::PointerMoved(to),
            button(to, false),
        ] {
            $h.event(event);
            $h.run_steps(1);
        }
        $h.run_steps(3);
    }};
}

#[test]
fn effect_picker_height_follows_resize_and_stays_stable() {
    check_height(0, "加工内容を選択");
}

#[test]
fn mask_pickers_height_follows_resize_and_stays_stable() {
    check_height(1, "マスク種類変更");
}

#[test]
fn add_layer_picker_height_follows_resize_and_stays_stable() {
    check_height(2, "補正レイヤーを追加");
}

fn check_height(which: u8, title: &str) {
    let mut app = crate::app::setup_app_for_test();
    app.local_adjust_effect_picker_dialog_open = true;
    app.local_adjust_change_mask_dialog_open = true;
    app.local_adjust_add_layer_dialog_open = true;
    let layers = vec![local_adjust_core::LocalAdjustmentLayer::new(
        "layout test",
        local_adjust_core::LocalMask::Full,
        local_adjust_core::LocalEffect::None,
    )];
    let mut h = crate::test_ui::builder()
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .with_size(egui::vec2(2400.0, 1400.0))
        .build_ui_state(
            |root, (app, query)| match which {
                0 => app.draw_local_adjust_effect_picker_dialog(root.ctx(), 0, query, &mut None),
                1 => app.draw_local_adjust_change_mask_dialog(root.ctx(), &layers, 0, &mut None),
                _ => app.draw_local_adjust_add_layer_dialog(root.ctx(), (100, 100), &mut None),
            },
            (app, String::new()),
        );
    h.run_steps(4);
    let before = window_rect(&h.ctx, title);
    let scroll_before = (which == 0).then(|| scroll_rect(&h.ctx));
    let corner = before.max - egui::vec2(2.0, 2.0);
    drag!(h, corner, corner + egui::vec2(0.0, 150.0));
    let after = window_rect(&h.ctx, title);
    assert!(
        (after.height() - before.height() - 150.0).abs() <= 2.0,
        "{title}: before={before:?}, after={after:?}"
    );
    if let Some(scroll_before) = scroll_before {
        assert!((scroll_rect(&h.ctx).height() - scroll_before.height() - 150.0).abs() <= 2.0);
    } else {
        for kind in MASK_GROUPS.iter().flat_map(|g| g.kinds) {
            let rect = h.get_by_label(kind.label()).rect();
            let text_width = h.ctx.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(
                        kind.label().into(),
                        egui::FontId::proportional(12.0),
                        egui::Color32::WHITE,
                    )
                    .size()
                    .x
            });
            assert_eq!(rect.size(), LOCAL_ADJUST_MASK_PICKER_BUTTON_SIZE);
            assert!(
                text_width + h.ctx.global_style().spacing.button_padding.x * 2.0 <= rect.width()
            );
        }
    }
    for frame in 0..24 {
        if which == 0 {
            h.state_mut().1 = if frame % 2 == 0 {
                "no-such-effect".into()
            } else {
                String::new()
            };
        }
        h.event(egui::Event::PointerMoved(egui::pos2(
            20.0 + frame as f32,
            20.0,
        )));
        h.run_steps(1);
        assert_eq!(window_rect(&h.ctx, title).size(), after.size());
    }
    let corner = after.max - egui::vec2(2.0, 2.0);
    drag!(h, corner, corner - egui::vec2(0.0, 150.0));
    assert!((window_rect(&h.ctx, title).height() - before.height()).abs() <= 2.0);
}

#[test]
fn effect_picker_labels_and_columns_fit_at_narrow_widths() {
    let mut failures = Vec::new();
    for width in [560.0, 580.0, 640.0, 720.0, 860.0] {
        let mut app = crate::app::setup_app_for_test();
        app.local_adjust_effect_picker_dialog_open = true;
        let mut h = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(2400.0, 1400.0))
            .build_ui_state(
                |root, (app, query)| {
                    app.draw_local_adjust_effect_picker_dialog(root.ctx(), 0, query, &mut None);
                },
                (app, String::new()),
            );
        h.run_steps(4);
        let before = window_rect(&h.ctx, "加工内容を選択");
        let corner = before.max - egui::vec2(2.0, 2.0);
        drag!(
            h,
            corner,
            corner + egui::vec2(width - before.width(), 360.0 - before.height())
        );
        let resized = window_rect(&h.ctx, "加工内容を選択");
        assert!(
            (resized.width() - width).abs() <= 2.0 && (resized.height() - 360.0).abs() <= 2.0,
            "{resized:?}"
        );
        let padding = h.ctx.global_style().spacing.button_padding.x * 2.0;
        for kind in EFFECT_GROUPS.iter().flat_map(|g| g.kinds) {
            let label = kind.picker_label();
            let rect = h
                .get_by(|n| {
                    n.role() == egui::accesskit::Role::Button && n.label().as_deref() == Some(label)
                })
                .rect();
            let text_width = h.ctx.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(
                        label.into(),
                        egui::FontId::proportional(12.0),
                        egui::Color32::WHITE,
                    )
                    .size()
                    .x
            });
            if rect.width() + 0.1 < text_width + padding {
                failures.push(format!(
                    "width={width}, {label}: button={}, text+padding={}",
                    rect.width(),
                    text_width + padding
                ));
            }
            if (rect.height() - 30.0).abs() > 0.1 {
                failures.push(format!("width={width}, {label}: height={}", rect.height()));
            }
        }
        assert_columns(&h);
        for query in [
            "グラデーションマップ",
            "ポスタリゼーション",
            "no-such-effect-1727",
            "",
        ] {
            h.state_mut().1 = query.into();
            h.run_steps(3);
            assert_columns(&h);
            assert_eq!(window_rect(&h.ctx, "加工内容を選択").size(), resized.size());
            if !query.is_empty() && !query.starts_with("no-such") {
                let button = h.get_by_label(query).rect();
                let painted: usize = h
                    .output()
                    .shapes
                    .iter()
                    .map(|s| assert_painted_label(&s.shape, s.clip_rect, query, button))
                    .sum();
                assert_eq!(painted, 1, "label must actually be painted: {query}");
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn effect_picker_uses_the_calculated_columns() {
    let mut app = crate::app::setup_app_for_test();
    app.local_adjust_effect_picker_dialog_open = true;
    let mut h = crate::test_ui::builder()
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .with_size(egui::vec2(2400.0, 1400.0))
        .build_ui_state(
            |root, (app, query)| {
                app.draw_local_adjust_effect_picker_dialog(root.ctx(), 0, query, &mut None);
            },
            (app, String::new()),
        );
    h.run_steps(4);
    let before = window_rect(&h.ctx, "加工内容を選択");
    let corner = before.max - egui::vec2(2.0, 2.0);
    drag!(h, corner, corner - egui::vec2(300.0, 0.0));
    let inner = h
        .ctx
        .data_mut(|d| d.get_temp::<egui::Rect>(egui::Id::unique(Some("effect_picker_scroll_1727"))))
        .unwrap();
    for group in EFFECT_GROUPS {
        let rects: Vec<_> = group
            .kinds
            .iter()
            .map(|kind| {
                h.get_by(|n| {
                    n.role() == egui::accesskit::Role::Button
                        && n.label().as_deref() == Some(kind.picker_label())
                })
                .rect()
            })
            .collect();
        let min_width = EFFECT_GROUPS
            .iter()
            .flat_map(|g| g.kinds)
            .map(|kind| {
                h.ctx.fonts_mut(|fonts| {
                    fonts
                        .layout_no_wrap(
                            kind.picker_label().into(),
                            egui::FontId::proportional(12.0),
                            egui::Color32::WHITE,
                        )
                        .size()
                        .x
                }) + h.ctx.global_style().spacing.button_padding.x * 2.0
            })
            .fold(0.0, f32::max);
        let columns = ((inner.width() + 6.0) / (min_width + 6.0)).floor() as usize;
        assert!(columns >= 1);
        let actual = rects
            .iter()
            .take_while(|r| r.top() < rects[0].bottom())
            .count();
        assert_eq!(
            actual,
            columns.min(rects.len()),
            "{}: inner={inner:?}, buttons={rects:?}",
            group.title
        );
    }
}

#[test]
fn effect_picker_fractional_dpi_keeps_labels_columns_and_window_stable() {
    check_render_configuration(1.25, 1.0, false);
}

#[test]
fn effect_picker_ui_scale_200_keeps_labels_columns_and_window_stable() {
    check_render_configuration(1.0, 2.0, false);
}

#[test]
fn effect_picker_meiryo_keeps_labels_columns_and_window_stable() {
    check_render_configuration(1.0, 1.0, true);
}

fn label_width(ctx: &egui::Context, label: &str) -> f32 {
    ctx.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(
                label.into(),
                egui::FontId::proportional(12.0),
                egui::Color32::WHITE,
            )
            .size()
            .x
    })
}

fn check_render_configuration(native_ppp: f32, zoom: f32, meiryo: bool) {
    let mut app = crate::app::setup_app_for_test();
    app.local_adjust_effect_picker_dialog_open = true;
    let mut h = crate::test_ui::builder()
        .with_size(egui::vec2(2400.0, 1600.0))
        .with_pixels_per_point(native_ppp)
        .with_context_setup(move |ctx| {
            crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
            if meiryo {
                let path = std::path::PathBuf::from(r"C:\Windows\Fonts\meiryo.ttc");
                assert!(
                    path.is_file(),
                    "Windows test prerequisite: Meiryo must be installed"
                );
                crate::ui_fonts::configure_fonts_with_settings(
                    ctx,
                    &crate::settings::UiFontSettings {
                        selection: crate::settings::UiFontSelection::Face {
                            display_name: "Meiryo".into(),
                            path,
                            face_index: 0,
                            post_script_name: "Meiryo".into(),
                        },
                        ..Default::default()
                    },
                );
            } else {
                crate::ui_fonts::configure_fonts(ctx);
            }
            crate::settings::apply_ui_scale_factor(ctx, zoom);
        })
        .build_ui_state(
            |root, (app, query)| {
                app.draw_local_adjust_effect_picker_dialog(root.ctx(), 0, query, &mut None);
            },
            (app, String::new()),
        );
    h.run_steps(4);
    assert!((h.ctx.zoom_factor() - zoom).abs() < 0.001);
    assert!((h.ctx.pixels_per_point() - native_ppp * zoom).abs() < 0.001);
    assert_eq!(
        h.ctx.input(|i| i.viewport().native_pixels_per_point),
        Some(native_ppp)
    );
    if meiryo {
        // Reject silent fallback: both the selected family and different actual
        // catalog text metrics must be present, not merely a font setting.
        h.ctx.fonts_mut(|fonts| {
            let defs = fonts.definitions();
            assert!(defs.font_data.contains_key("ui_primary"));
            assert_eq!(
                defs.families[&egui::FontFamily::Proportional][0],
                "ui_primary"
            );
        });
        let default_ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&default_ctx);
        let mut different = false;
        default_ctx
            .run_pass(Default::default(), |ctx| {
                different = EFFECT_GROUPS.iter().flat_map(|g| g.kinds).any(|kind| {
                    (label_width(ctx, kind.picker_label())
                        - label_width(&h.ctx, kind.picker_label()))
                    .abs()
                        > 0.1
                });
            })
            .drop_without_applying_deltas();
        assert!(different, "Meiryo must actually change picker text widths");
    }
    let before = window_rect(&h.ctx, "加工内容を選択");
    let corner = before.max - egui::vec2(2.0, 2.0);
    drag!(
        h,
        corner,
        corner + egui::vec2(560.0 - before.width(), 360.0 - before.height())
    );
    let expected = window_rect(&h.ctx, "加工内容を選択");
    assert!((expected.width() - 560.0).abs() < 2.0 && (expected.height() - 360.0).abs() < 2.0);
    let padding: egui::Margin = h.ctx.global_style().spacing.button_padding.into();
    let first = h
        .get_by_label(EFFECT_GROUPS[0].kinds[0].picker_label())
        .rect();
    eprintln!(
        "native_ppp={native_ppp}, zoom={zoom}, meiryo={meiryo}: scroll={:?}, first={first:?}",
        scroll_rect(&h.ctx)
    );
    for frame in 0..24 {
        let query = [
            "",
            "グラデーションマップ",
            "ポスタリゼーション",
            "no-such-effect-1727",
        ][frame % 4];
        h.state_mut().1 = query.into();
        h.event(egui::Event::PointerMoved(egui::pos2(
            20.0 + frame as f32,
            20.0,
        )));
        h.run_steps(1);
        assert_eq!(
            window_rect(&h.ctx, "加工内容を選択").size(),
            expected.size()
        );
        // ScrollArea pixel-rounds its content bounds before placing widgets,
        // while inner_rect retains the unsnapped bounds.
        // Keep the original cases' 0.1pt tolerance; these conditions use the
        // snapshot policy's one-physical-pixel geometry tolerance. Text ink,
        // button height and Window stability below retain their strict checks.
        assert_columns_with_tolerance(&h, 1.0 / h.ctx.pixels_per_point());
        for kind in EFFECT_GROUPS
            .iter()
            .flat_map(|g| g.kinds)
            .filter(|kind| effect_picker_matches_query(**kind, query))
        {
            let label = kind.picker_label();
            let button = h
                .get_by(|n| {
                    n.role() == egui::accesskit::Role::Button && n.label().as_deref() == Some(label)
                })
                .rect();
            assert!(
                label_width(&h.ctx, label) + padding.sum().x <= button.width() + 0.1,
                "{label}: {button:?}"
            );
            assert!((button.height() - 30.0).abs() < 0.1);
        }
        if frame % 4 == 1 || frame % 4 == 2 {
            let button = h.get_by_label(query).rect();
            let painted: usize = h
                .output()
                .shapes
                .iter()
                .map(|s| assert_painted_label(&s.shape, s.clip_rect, query, button))
                .sum();
            assert_eq!(painted, 1, "{query}");
        }
    }
}

#[test]
fn effect_picker_default_dark_snapshot() {
    // The production Window renderer needs only transient UI state. No App,
    // settings store, worker, native window, or real image is constructed.
    let mut open = true;
    let mut query = String::new();
    let mut h = crate::test_ui::builder()
        // Leave room for the production default_pos and viewport margins so
        // egui doesn't shrink the 860x560 Window to fit the screenshot canvas.
        .with_size(egui::vec2(1000.0, 660.0))
        .with_context_setup(|ctx| {
            crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
            crate::ui_fonts::configure_fonts(ctx);
        })
        .build_ui(move |ui| {
            let _ = show_local_adjust_effect_picker_window(ui.ctx(), &mut open, &mut query);
        });
    h.run_steps(4);
    assert_eq!(
        window_rect(&h.ctx, "加工内容を選択").size(),
        egui::vec2(860.0, 560.0)
    );
    h.snapshot("local_adjust_effect_picker_default_dark");
}
