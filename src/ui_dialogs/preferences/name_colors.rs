//! Draft-only controls for the details list name column.
use crate::details_name_colors::{
    DetailsNameCategory, DetailsNameColor, DetailsNameColors, contrast_ratio, row_backgrounds,
};
use crate::settings::{Settings, TextContrast};
use eframe::egui;

pub(super) fn draw_settings(ui: &mut egui::Ui, settings: &mut Settings) {
    let contrast = settings.text_contrast.normalized();
    let strong = contrast == TextContrast::Strong;
    ui.strong("詳細一覧の名前色");
    ui.checkbox(&mut settings.details_name_colors.enabled, "名前の色分け");
    ui.small("選択行は共通の選択文字色、チェック行は共通文字色で表示します。切り取り中も名前は不透明です。OFF にしてもカスタム色は保持します。");
    if strong {
        ui.small("強い文字コントラストでは既定色を使います。標準のカスタム色は保持され、標準へ戻すと再適用します。");
    }
    if ui.button("名前色をすべて既定に戻す").clicked() {
        clear_hex_editor(ui.ctx());
        settings.details_name_colors = DetailsNameColors::default();
    }
    let mut preview = settings.details_name_colors.clone();
    preview.enabled = true;
    let warned = DetailsNameCategory::ALL.iter().any(|&category| {
        [false, true].into_iter().any(|dark| {
            let visuals = theme_visuals(dark, contrast);
            let color = preview.color(category, &visuals, contrast);
            row_backgrounds(&visuals)
                .into_iter()
                .any(|bg| contrast_ratio(color, bg) < 4.5)
        })
    });
    if warned {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            if strong {
                "強い配色のフォルダ色はhoverで4.5:1未満です"
            } else {
                "標準配色の名前色は4.5:1未満です。交互行やhoverでは読みにくくなる場合があります"
            },
        );
        ui.small("各サンプルに実際の表示比を示します。3:1未満の状態には ! を付けます。");
    }
    if !preview.valid_standard_custom() {
        ui.colored_label(
            ui.visuals().error_fg_color,
            "通常行は3:1以上の色を指定してください",
        );
    }
    ui.add_space(4.0);
    let cell_width = ((ui.available_width() - 105.0) / 2.0).max(110.0);
    egui::Grid::new("details_name_colors_table")
        .num_columns(3)
        .spacing([8.0, 8.0])
        .striped(true)
        .show(ui, |ui| {
            ui.strong("カテゴリ / 色");
            ui.strong("Light");
            ui.strong("Dark");
            ui.end_row();
            for category in DetailsNameCategory::ALL {
                ui.push_id(category.index(), |ui| {
                    ui.vertical(|ui| {
                        let category_label = ui.strong(category.label());
                        let choice = &mut settings.details_name_colors.colors[category.index()];
                        let mut custom = matches!(choice, DetailsNameColor::Custom { .. });
                        let before = custom;
                        ui.add_enabled_ui(!strong, |ui| {
                            egui::ComboBox::from_id_salt("mode")
                                .width(80.0)
                                .selected_text(if custom { "カスタム" } else { "既定" })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut custom, false, "既定");
                                    ui.selectable_value(&mut custom, true, "カスタム");
                                })
                                .response
                                .labelled_by(category_label.id);
                        });
                        if custom != before {
                            clear_hex_editor(ui.ctx());
                            *choice = if custom {
                                custom_from_default(category)
                            } else {
                                DetailsNameColor::Default
                            };
                        }
                        if ui.small_button("既定へ戻す").clicked() {
                            clear_hex_editor(ui.ctx());
                            *choice = DetailsNameColor::Default;
                        }
                    });
                });
                for dark in [false, true] {
                    ui.push_id((category.index(), dark), |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(cell_width);
                            let visuals = theme_visuals(dark, contrast);
                            let choice = &mut settings.details_name_colors.colors[category.index()];
                            if !strong
                                && let DetailsNameColor::Custom {
                                    light,
                                    dark: dark_rgb,
                                } = choice
                            {
                                let rgb = if dark { dark_rgb } else { light };
                                draw_rgb_editor(ui, rgb);
                            } else {
                                let color = preview_color(
                                    &settings.details_name_colors,
                                    category,
                                    &visuals,
                                    contrast,
                                );
                                ui.monospace(format_hex(color.to_array()[..3].try_into().unwrap()));
                            }
                            let color = preview_color(
                                &settings.details_name_colors,
                                category,
                                &visuals,
                                contrast,
                            );
                            draw_samples(ui, color, &visuals);
                        });
                    });
                }
                ui.end_row();
            }
        });
}

fn preview_color(
    colors: &DetailsNameColors,
    category: DetailsNameCategory,
    visuals: &egui::Visuals,
    contrast: TextContrast,
) -> egui::Color32 {
    let mut preview = colors.clone();
    preview.enabled = true;
    preview.color(category, visuals, contrast)
}

fn theme_visuals(dark: bool, contrast: TextContrast) -> egui::Visuals {
    crate::os_theme::app_visuals(
        if dark {
            crate::os_theme::ResolvedTheme::Dark
        } else {
            crate::os_theme::ResolvedTheme::Light
        },
        contrast,
    )
}

fn custom_from_default(category: DetailsNameCategory) -> DetailsNameColor {
    let defaults = DetailsNameColors::default();
    let rgb = |dark| {
        let color = defaults.color(
            category,
            &theme_visuals(dark, TextContrast::Standard),
            TextContrast::Standard,
        );
        [color.r(), color.g(), color.b()]
    };
    DetailsNameColor::Custom {
        light: rgb(false),
        dark: rgb(true),
    }
}

fn format_hex(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    Some([(value >> 16) as u8, (value >> 8) as u8, value as u8])
}

// Only the focused widget needs unfinished text. RGB in the Preferences draft is
// the sole color value; this transient egui editor never becomes product state.
#[derive(Clone)]
struct ActiveHexEditor {
    widget_id: egui::Id,
    text: String,
}

fn editor_id() -> egui::Id {
    egui::Id::new("details_name_active_hex_editor")
}

pub(super) fn clear_hex_editor(ctx: &egui::Context) {
    ctx.data_mut(|data| data.remove::<ActiveHexEditor>(editor_id()));
}

pub(super) fn hex_input_ready(ctx: &egui::Context) -> bool {
    let editor = ctx.data(|data| data.get_temp::<ActiveHexEditor>(editor_id()));
    editor.is_none_or(|editor| {
        !ctx.memory(|memory| memory.has_focus(editor.widget_id))
            || parse_hex(&editor.text).is_some()
    })
}

fn draw_rgb_editor(ui: &mut egui::Ui, rgb: &mut [u8; 3]) -> egui::Response {
    ui.horizontal_wrapped(|ui| {
        let color_response = ui.color_edit_button_srgb(rgb);
        crate::ui_helpers::set_accessibility_label(&color_response, "色");
        if color_response.changed() {
            clear_hex_editor(ui.ctx());
        }
        let widget_id = ui.make_persistent_id("rgb_hex");
        let mut hex = ui
            .ctx()
            .data(|data| data.get_temp::<ActiveHexEditor>(editor_id()))
            .filter(|editor| editor.widget_id == widget_id)
            .map_or_else(|| format_hex(*rgb), |editor| editor.text);
        let response = crate::ime_focus::add_singleline(ui, &mut hex, None, |edit| {
            edit.id(widget_id)
                .desired_width(76.0)
                .font(egui::TextStyle::Monospace)
        })
        .labelled_by(color_response.id);
        if response.changed()
            && !crate::ime_focus::ime_composing_with_pending_events(
                ui.ctx(),
                ui.ctx().viewport_id(),
                &[],
            )
            && let Some(parsed) = parse_hex(&hex)
        {
            *rgb = parsed;
        }
        if response.has_focus() {
            ui.ctx().data_mut(|data| {
                data.insert_temp(
                    editor_id(),
                    ActiveHexEditor {
                        widget_id,
                        text: hex.clone(),
                    },
                )
            });
            if parse_hex(&hex).is_none() {
                ui.colored_label(ui.visuals().error_fg_color, "HEX は6桁で指定");
            }
        } else if response.lost_focus() {
            // A field rendered later can lose focus after an earlier field has
            // already installed its new editor. Only release this widget's data.
            ui.ctx().data_mut(|data| {
                if data
                    .get_temp::<ActiveHexEditor>(editor_id())
                    .is_some_and(|editor| editor.widget_id == widget_id)
                {
                    data.remove::<ActiveHexEditor>(editor_id());
                }
            });
        }
        response
    })
    .inner
}

fn draw_samples(ui: &mut egui::Ui, color: egui::Color32, visuals: &egui::Visuals) {
    let [normal, alternating, hover] = row_backgrounds(visuals);
    let common = visuals.text_color();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
        for (label, fg, bg) in [
            ("通常", color, normal),
            ("交互", color, alternating),
            ("hover", color, hover),
            (
                "選択",
                visuals.selection.stroke.color,
                visuals.selection.bg_fill,
            ),
            ("チェック", common, visuals.widgets.active.bg_fill),
            ("切取 ✂", color, normal),
        ] {
            let ratio = contrast_ratio(fg, bg);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(61.0, 33.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 2.0, bg);
            ui.painter().text(
                rect.center_top() + egui::vec2(0.0, 2.0),
                egui::Align2::CENTER_TOP,
                label,
                egui::FontId::proportional(11.0),
                fg,
            );
            ui.painter().text(
                rect.center_bottom() - egui::vec2(0.0, 2.0),
                egui::Align2::CENTER_BOTTOM,
                format!("{ratio:.2}:1{}", if ratio < 3.0 { " !" } else { "" }),
                egui::FontId::proportional(10.0),
                fg,
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_color_warning_matches_standard_and_strong_previews() {
        use egui_kittest::kittest::Queryable;
        let standard_warning =
            "標準配色の名前色は4.5:1未満です。交互行やhoverでは読みにくくなる場合があります";
        let strong_warning = "強い配色のフォルダ色はhoverで4.5:1未満です";
        for contrast in [TextContrast::Standard, TextContrast::Strong] {
            let mut draft = Settings::default();
            draft.text_contrast = contrast;
            let mut harness = crate::test_ui::builder()
                .with_context_setup(crate::ui_fonts::configure_fonts)
                .with_size(egui::vec2(760.0, 1200.0))
                .build_ui_state(
                    |root_ui, draft| {
                        let ctx = &root_ui.ctx().clone();
                        egui::CentralPanel::default().show(root_ui, |ui| {
                            draw_settings(ui, draft);
                        });
                    },
                    draft,
                );
            harness.run();
            let strong = contrast == TextContrast::Strong;
            assert_eq!(harness.query_by_label(strong_warning).is_some(), strong);
            assert_eq!(harness.query_by_label(standard_warning).is_some(), !strong);
        }
    }

    #[test]
    fn category_reset_and_all_reset_handlers_only_edit_the_draft() {
        use egui_kittest::kittest::Queryable;
        let mut live = Settings::default();
        live.details_name_colors.enabled = false;
        live.details_name_colors.colors[0] = DetailsNameColor::Custom {
            light: [80, 60, 0],
            dark: [214, 186, 102],
        };
        live.details_name_colors.colors[1] = DetailsNameColor::Custom {
            light: [55, 80, 15],
            dark: [180, 220, 140],
        };
        let before = live.details_name_colors.clone();
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(680.0, 1200.0))
            .build_ui_state(
                |root_ui, draft| {
                    let ctx = &root_ui.ctx().clone();
                    egui::CentralPanel::default().show(root_ui, |ui| {
                        draw_settings(ui, draft);
                    });
                },
                live.preferences_snapshot(),
            );
        harness.run();
        harness
            .get_all_by_label("既定へ戻す")
            .next()
            .unwrap()
            .click();
        harness.run();
        assert_eq!(
            harness.state().details_name_colors.colors[0],
            DetailsNameColor::Default
        );
        assert_eq!(
            harness.state().details_name_colors.colors[1],
            before.colors[1]
        );
        assert!(!harness.state().details_name_colors.enabled);
        harness.get_by_label("名前色をすべて既定に戻す").click();
        harness.run();
        assert_eq!(
            harness.state().details_name_colors,
            DetailsNameColors::default()
        );
        assert_eq!(live.details_name_colors, before);
    }

    #[test]
    fn name_color_hex_accepts_only_opaque_rgb() {
        assert_eq!(parse_hex("#A87e00"), Some([168, 126, 0]));
        assert_eq!(parse_hex("D6BA66"), Some([214, 186, 102]));
        for invalid in ["#FFF", "#12345678", "12345G", "#", "日本語"] {
            assert_eq!(parse_hex(invalid), None);
        }
        assert_eq!(format_hex([0, 15, 255]), "#000FFF");
    }

    #[test]
    fn category_custom_starts_with_both_standard_defaults() {
        for category in DetailsNameCategory::ALL {
            let mut colors = DetailsNameColors::default();
            colors.colors[category.index()] = custom_from_default(category);
            assert!(colors.valid_standard_custom());
            for dark in [false, true] {
                let visuals = theme_visuals(dark, TextContrast::Standard);
                assert_eq!(
                    colors.color(category, &visuals, TextContrast::Standard),
                    DetailsNameColors::default().color(category, &visuals, TextContrast::Standard)
                );
            }
        }
    }
    fn editor_pass(ctx: &egui::Context, rgb: &mut [u8; 3], events: Vec<egui::Event>) -> egui::Id {
        let mut id = egui::Id::NULL;
        let _ = ctx
            .run_pass(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 100.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    ctx.root_ui(|root_ui| {
                        egui::CentralPanel::default().show(root_ui, |ui| {
                            id = draw_rgb_editor(ui, rgb).id;
                        })
                    });
                },
            )
            .drop_without_applying_deltas();
        id
    }

    fn select_editor(ctx: &egui::Context, id: egui::Id, rgb: &mut [u8; 3]) {
        ctx.memory_mut(|memory| memory.request_focus(id));
        editor_pass(ctx, rgb, vec![]);
        let mut state = egui::TextEdit::load_state(ctx, id).unwrap();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(7),
            )));
        state.store(ctx, id);
    }

    #[test]
    fn hex_handler_keeps_partial_text_without_mutating_rgb_and_accepts_complete_value() {
        let ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);
        let mut rgb = [55, 55, 55];
        let id = editor_pass(&ctx, &mut rgb, vec![]);
        select_editor(&ctx, id, &mut rgb);
        editor_pass(&ctx, &mut rgb, vec![egui::Event::Text("#A87".into())]);
        assert_eq!(rgb, [55, 55, 55]);
        assert!(!hex_input_ready(&ctx));
        editor_pass(&ctx, &mut rgb, vec![egui::Event::Text("E00".into())]);
        assert_eq!(rgb, [168, 126, 0]);
        assert!(hex_input_ready(&ctx));
        select_editor(&ctx, id, &mut rgb);
        editor_pass(&ctx, &mut rgb, vec![egui::Event::Text("#FFFFGG".into())]);
        assert_eq!(rgb, [168, 126, 0]);
        assert!(!hex_input_ready(&ctx));
    }

    #[test]
    fn hex_handler_ime_preedit_does_not_publish_an_rgb_value() {
        let ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);
        let mut rgb = [55, 55, 55];
        let id = editor_pass(&ctx, &mut rgb, vec![]);
        select_editor(&ctx, id, &mut rgb);
        editor_pass(
            &ctx,
            &mut rgb,
            vec![
                egui::Event::Ime(egui::ImeEvent::Enabled),
                egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "#A87E00".into(),
                    active_range_chars: None,
                }),
            ],
        );
        assert_eq!(rgb, [55, 55, 55]);
        editor_pass(
            &ctx,
            &mut rgb,
            vec![
                egui::Event::Ime(egui::ImeEvent::Commit("#A87E00".into())),
                egui::Event::Ime(egui::ImeEvent::Disabled),
            ],
        );
        assert_eq!(rgb, [168, 126, 0]);
    }

    fn hex_editor_pair_pass(
        ctx: &egui::Context,
        colors: &mut [[u8; 3]; 2],
        events: Vec<egui::Event>,
    ) -> [egui::Response; 2] {
        let mut responses = Vec::new();
        ctx.run_pass(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 160.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                ctx.root_ui(|root_ui| {
                    egui::CentralPanel::default().show(root_ui, |ui| {
                        for (index, rgb) in colors.iter_mut().enumerate() {
                            ui.push_id(index, |ui| {
                                responses.push(draw_rgb_editor(ui, rgb));
                            });
                        }
                    })
                });
            },
        )
        .drop_without_applying_deltas();
        responses.try_into().expect("two real HEX editors")
    }

    fn hex_editor_click_events(position: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            },
        ]
    }

    fn hex_editor_replace_events(text: &str) -> Vec<egui::Event> {
        vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers {
                    ctrl: true,
                    command: true,
                    ..Default::default()
                },
            },
            egui::Event::Text(text.into()),
        ]
    }

    fn assert_active_hex_editor(ctx: &egui::Context, id: egui::Id, text: &str) {
        let editor = ctx
            .data(|data| data.get_temp::<ActiveHexEditor>(editor_id()))
            .expect("focused HEX editor owns its text");
        assert_eq!(editor.widget_id, id);
        assert_eq!(editor.text, text);
        assert!(ctx.memory(|memory| memory.has_focus(id)));
    }

    #[test]
    fn hex_focus_transfer_keeps_the_new_editors_partial_input() {
        for target in 0..2 {
            let ctx = egui::Context::default();
            crate::ui_fonts::configure_fonts(&ctx);
            crate::ime_focus::install_ime_input_policy(&ctx);
            let mut colors = [[55; 3], [66; 3]];
            let original = colors;
            let initial = hex_editor_pair_pass(&ctx, &mut colors, vec![]);
            let old_id = initial[1 - target].id;
            let new_id = initial[target].id;
            ctx.memory_mut(|memory| memory.request_focus(old_id));
            let focused = hex_editor_pair_pass(&ctx, &mut colors, vec![]);
            assert_eq!(focused[target].id, new_id);

            // 利用者採択: 通常のclickと文字入力は別passとして検査する。
            // 同batchの0.37差は下の専用回帰で固定し、製品側で入力を分割しない。
            let clicked = hex_editor_pair_pass(
                &ctx,
                &mut colors,
                hex_editor_click_events(focused[target].rect.center()),
            );
            assert!(clicked[target].clicked());
            assert_active_hex_editor(&ctx, new_id, &format_hex(original[target]));
            assert!(!ctx.memory(|memory| memory.has_focus(old_id)));

            hex_editor_pair_pass(&ctx, &mut colors, hex_editor_replace_events("#A87"));
            assert_eq!(colors, original, "partial input must not publish an RGB");
            assert_active_hex_editor(&ctx, new_id, "#A87");
            assert!(!hex_input_ready(&ctx));

            hex_editor_pair_pass(&ctx, &mut colors, vec![]);
            assert_active_hex_editor(&ctx, new_id, "#A87");
            assert!(
                !hex_input_ready(&ctx),
                "an idle pass must keep partial input"
            );

            hex_editor_pair_pass(&ctx, &mut colors, vec![egui::Event::Text("E00".into())]);
            let mut expected = original;
            expected[target] = [168, 126, 0];
            assert_eq!(colors, expected, "only the new field may publish its RGB");
            assert_active_hex_editor(&ctx, new_id, "#A87E00");
            assert!(hex_input_ready(&ctx));
        }
    }

    #[test]
    fn hex_focus_transfer_same_batch_adopts_egui037_without_cross_field_edits() {
        for target in 0..2 {
            let ctx = egui::Context::default();
            crate::ui_fonts::configure_fonts(&ctx);
            crate::ime_focus::install_ime_input_policy(&ctx);
            let mut colors = [[55; 3], [66; 3]];
            let original = colors;
            let initial = hex_editor_pair_pass(&ctx, &mut colors, vec![]);
            let old_id = initial[1 - target].id;
            let new_id = initial[target].id;
            ctx.memory_mut(|memory| memory.request_focus(old_id));
            let focused = hex_editor_pair_pass(&ctx, &mut colors, vec![]);
            let mut same_batch = hex_editor_click_events(focused[target].rect.center());
            same_batch.extend(hex_editor_replace_events("#A87"));
            let responses = hex_editor_pair_pass(&ctx, &mut colors, same_batch);

            // 2026-10-17採択: 0.37では新欄へのfocusより先にeventsが処理される。
            // このbatchのTextはどちらにも入力されず、focusだけが移る。
            assert!(responses[target].clicked());
            assert!(responses[target].gained_focus());
            assert!(responses.iter().all(|response| !response.changed()));
            assert_eq!(
                colors, original,
                "same-batch text must not alter either RGB"
            );
            assert_active_hex_editor(&ctx, new_id, &format_hex(original[target]));
            assert!(!ctx.memory(|memory| memory.has_focus(old_id)));
            assert!(
                hex_input_ready(&ctx),
                "dropped text must not create a partial draft"
            );

            // focus確定後の新しい入力は届き、旧欄には入らない。
            hex_editor_pair_pass(&ctx, &mut colors, hex_editor_replace_events("#A87E00"));
            let mut expected = original;
            expected[target] = [168, 126, 0];
            assert_eq!(colors, expected);
            assert_active_hex_editor(&ctx, new_id, "#A87E00");
        }
    }

    #[test]
    fn strong_and_disabled_previews_keep_saved_custom_values() {
        let mut colors = DetailsNameColors::default();
        colors.colors[1] = DetailsNameColor::Custom {
            light: [55, 80, 15],
            dark: [180, 220, 140],
        };
        let stored = colors.colors;
        colors.enabled = false;
        let visuals = theme_visuals(false, TextContrast::Standard);
        assert_eq!(
            preview_color(
                &colors,
                DetailsNameCategory::Book,
                &visuals,
                TextContrast::Standard
            ),
            egui::Color32::from_rgb(55, 80, 15)
        );
        let visuals = theme_visuals(false, TextContrast::Strong);
        assert_eq!(
            preview_color(
                &colors,
                DetailsNameCategory::Book,
                &visuals,
                TextContrast::Strong
            ),
            visuals.text_color()
        );
        assert_eq!(colors.colors, stored);
    }
}
