use super::*;

fn combo<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut T,
    choices: &[(T, &str)],
) {
    let selected = choices
        .iter()
        .find(|(candidate, _)| candidate == value)
        .map_or("指定", |(_, text)| *text);
    let name = ui.label(label);
    egui::ComboBox::from_id_salt(label)
        .selected_text(selected)
        .width(ui.available_width())
        .truncate()
        .popup_style(crate::os_theme::dark_popup_style(ui.ctx()))
        .show_ui(ui, |ui| {
            for (candidate, text) in choices {
                ui.selectable_value(value, *candidate, *text);
            }
        })
        .response
        .labelled_by(name.id);
}

fn number(ui: &mut egui::Ui, label: &str, value: &mut u32, range: std::ops::RangeInclusive<u32>) {
    ui.horizontal(|ui| {
        let name = ui.label(label);
        ui.add(egui::DragValue::new(value).range(range))
            .labelled_by(name.id);
    });
}

fn color(ui: &mut egui::Ui, label: &str, rgb: &mut [u8; 3]) {
    ui.horizontal(|ui| {
        let name = ui.label(label);
        ui.color_edit_button_srgb(rgb).labelled_by(name.id);
    });
}

fn anchor(ui: &mut egui::Ui, label: &str, value: &mut Anchor9) {
    combo(
        ui,
        label,
        value,
        &[
            (Anchor9::TopLeft, "左上"),
            (Anchor9::TopCenter, "上"),
            (Anchor9::TopRight, "右上"),
            (Anchor9::CenterLeft, "左"),
            (Anchor9::Center, "中央"),
            (Anchor9::CenterRight, "右"),
            (Anchor9::BottomLeft, "左下"),
            (Anchor9::BottomCenter, "下"),
            (Anchor9::BottomRight, "右下"),
        ],
    );
}

const RATIOS: &[(Option<(u32, u32)>, &str)] = &[
    (None, "固定しない"),
    (Some((1, 1)), "1:1"),
    (Some((4, 5)), "4:5"),
    (Some((3, 4)), "3:4"),
    (Some((4, 3)), "4:3"),
    (Some((2, 3)), "2:3"),
    (Some((3, 2)), "3:2"),
    (Some((16, 9)), "16:9"),
    (Some((9, 16)), "9:16"),
];

pub(super) fn custom_ratio_text(o: &CombineOptions) -> Option<String> {
    o.canvas_ratio
        .filter(|ratio| {
            !RATIOS
                .iter()
                .any(|(candidate, _)| *candidate == Some(*ratio))
        })
        .map(|(w, h)| format!("{w}:{h}"))
}

pub(super) fn draw(ui: &mut egui::Ui, state: &mut CombineDialogState, action: &mut Action) {
    draw_layout(ui, state);
    ui.separator();
    draw_canvas(ui, &mut state.options);
    ui.separator();
    draw_labels(ui, &mut state.options.labels);
    ui.separator();
    draw_output(ui, state, action);
}

fn draw_layout(ui: &mut egui::Ui, state: &mut CombineDialogState) {
    ui.strong("配置");
    let o = &mut state.options;
    combo(
        ui,
        "並べ方",
        &mut o.layout,
        &[
            (CombineLayout::Row, "横一列"),
            (CombineLayout::Column, "縦一列"),
            (CombineLayout::Grid, "グリッド"),
            (CombineLayout::JustifiedRows, "行ごとに詰める"),
            (CombineLayout::JustifiedColumns, "列ごとに詰める"),
        ],
    );
    ui.add_enabled_ui(o.layout != CombineLayout::Column, |ui| {
        combo(
            ui,
            "向き",
            &mut o.direction,
            &[
                (CombineDirection::LeftToRight, "左から右"),
                (CombineDirection::RightToLeft, "右から左"),
            ],
        )
    });
    match o.layout {
        CombineLayout::Row | CombineLayout::Column => {
            combo(
                ui,
                "辺のそろえ方",
                &mut o.line_size,
                &[
                    (LineSize::Largest, "いちばん大きい画像にそろえる"),
                    (LineSize::Smallest, "いちばん小さい画像にそろえる"),
                    (LineSize::None, "そろえない"),
                ],
            );
            if o.line_size == LineSize::None {
                let choices = if o.layout == CombineLayout::Row {
                    [
                        (LineAlign::Start, "上"),
                        (LineAlign::Center, "中央"),
                        (LineAlign::End, "下"),
                    ]
                } else {
                    [
                        (LineAlign::Start, "左"),
                        (LineAlign::Center, "中央"),
                        (LineAlign::End, "右"),
                    ]
                };
                combo(ui, "そろえ位置", &mut o.line_align, &choices);
            }
        }
        CombineLayout::Grid => {
            let mut automatic = o.grid_columns.is_none();
            if ui.checkbox(&mut automatic, "列数を自動で決める").changed() {
                o.grid_columns = if automatic { None } else { Some(2) };
            }
            if let Some(columns) = &mut o.grid_columns {
                number(ui, "列数", columns, 1..=20);
            }
            if o.canvas_ratio.is_some() {
                ui.add_enabled(false, egui::Label::new("セルの比率: 全体の比率に合わせる"));
            } else {
                let mut ratio = match o.cell_ratio {
                    CellRatio::FirstImage => None,
                    CellRatio::Ratio(w, h) => Some((w, h)),
                };
                let choices = RATIOS
                    .iter()
                    .map(|(ratio, label)| {
                        (
                            *ratio,
                            if ratio.is_none() {
                                "先頭の画像に合わせる"
                            } else {
                                *label
                            },
                        )
                    })
                    .collect::<Vec<_>>();
                combo(ui, "セルの比率", &mut ratio, &choices);
                o.cell_ratio = ratio.map_or(CellRatio::FirstImage, |(w, h)| CellRatio::Ratio(w, h));
            }
            combo(
                ui,
                "セルへの入れ方",
                &mut o.cell_fit,
                &[(CellFit::Fit, "収める"), (CellFit::Fill, "埋める")],
            );
        }
        CombineLayout::JustifiedRows | CombineLayout::JustifiedColumns => {
            ui.label(if o.layout == CombineLayout::JustifiedRows {
                "行の分け方"
            } else {
                "列の分け方"
            });
            let automatic = matches!(o.split, Split::Auto);
            ui.horizontal(|ui| {
                if ui.radio(automatic, "自動").clicked() {
                    o.split = Split::Auto;
                }
                if ui.radio(!automatic, "指定").clicked() && automatic {
                    let counts = state
                        .preview
                        .as_ref()
                        .map(|p| p.layout.split.clone())
                        .unwrap_or_else(|| vec![state.items.len() as u32]);
                    state.split_text = counts
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join("-");
                    o.split = Split::Counts(counts);
                }
            });
            if matches!(o.split, Split::Counts(_)) {
                text(
                    ui,
                    &mut state.split_text,
                    "image_combine_split",
                    ui.available_width(),
                );
                match parse_counts(&state.split_text, state.items.len()) {
                    Ok(counts) => o.split = Split::Counts(counts),
                    Err(error) => {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    }
                }
            } else if let Some(preview) = &state.preview {
                ui.small(format!(
                    "分け方: {}",
                    preview
                        .layout
                        .split
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join("-")
                ));
            }
        }
    }
    anchor(ui, "位置の基準点", &mut o.anchor);
    let ratio_allowed = !(matches!(o.layout, CombineLayout::Row | CombineLayout::Column)
        && o.line_size == LineSize::None);
    if !ratio_allowed {
        o.canvas_ratio = None;
        state.custom_ratio_text = None;
    }
    ui.add_enabled_ui(ratio_allowed, |ui| {
        let mut choice = if state.custom_ratio_text.is_some() {
            Some((0, 0))
        } else {
            o.canvas_ratio
        };
        let mut choices = RATIOS.to_vec();
        choices.push((Some((0, 0)), "指定"));
        let before = choice;
        combo(ui, "全体の比率", &mut choice, &choices);
        if choice != before {
            if choice == Some((0, 0)) {
                let (w, h) = o.canvas_ratio.unwrap_or((1, 1));
                state.custom_ratio_text = Some(format!("{w}:{h}"));
                o.canvas_ratio = Some((w, h));
            } else {
                state.custom_ratio_text = None;
                o.canvas_ratio = choice;
            }
        }
        if let Some(ratio_text) = &mut state.custom_ratio_text {
            text(ui, ratio_text, "image_combine_ratio", ui.available_width());
            match parse_ratio(ratio_text) {
                Ok(ratio) => o.canvas_ratio = Some(ratio),
                Err(error) => {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            }
        }
    });
}

fn draw_canvas(ui: &mut egui::Ui, o: &mut CombineOptions) {
    ui.strong("隙間・余白・背景");
    let old = o.gap_x;
    number(ui, "横の隙間 (px)", &mut o.gap_x, 0..=500);
    if o.gaps_linked && old != o.gap_x {
        o.gap_y = o.gap_x;
    }
    if ui
        .checkbox(&mut o.gaps_linked, "縦横を同じにする")
        .changed()
        && o.gaps_linked
    {
        o.gap_y = o.gap_x;
    }
    if !o.gaps_linked {
        number(ui, "縦の隙間 (px)", &mut o.gap_y, 0..=500);
    }
    number(ui, "外周の余白 (px)", &mut o.margin, 0..=500);
    let jpeg = matches!(o.format, CombineFormat::Jpeg { .. });
    let mut transparent = o.background == Background::Transparent;
    if jpeg && transparent {
        transparent = false;
        o.background = Background::Color([255; 3]);
    }
    if ui
        .add_enabled(!jpeg, egui::Checkbox::new(&mut transparent, "透明な背景"))
        .changed()
    {
        o.background = if transparent {
            Background::Transparent
        } else {
            Background::Color([255; 3])
        };
    }
    if jpeg {
        ui.small("JPEG は透明にできないため、背景色で保存します");
    }
    if let Background::Color(rgb) = &mut o.background {
        color(ui, "背景色", rgb);
    }
    ui.strong("角丸・枠線");
    number(ui, "角丸の半径 (px)", &mut o.corner_radius, 0..=500);
    number(ui, "枠線の太さ (px)", &mut o.border.width_px, 0..=100);
    color(ui, "枠線の色", &mut o.border.color);
}

fn draw_labels(ui: &mut egui::Ui, o: &mut LabelOptions) {
    ui.strong("ラベル");
    combo(
        ui,
        "置き方",
        &mut o.placement,
        &[
            (LabelPlacement::None, "なし"),
            (LabelPlacement::Overlay, "重ねる"),
            (LabelPlacement::Band, "下の帯"),
        ],
    );
    if o.placement == LabelPlacement::None {
        return;
    }
    combo(
        ui,
        "番号",
        &mut o.number_format,
        &[
            (NumberFormat::Arabic, "1, 2, 3"),
            (NumberFormat::ZeroPadded, "01, 02, 03"),
            (NumberFormat::UpperAlpha, "A, B, C"),
            (NumberFormat::LowerAlpha, "a, b, c"),
        ],
    );
    number(ui, "開始番号", &mut o.number_start, 0..=999);
    combo(
        ui,
        "デザイン",
        &mut o.style,
        &[
            (BadgeStyle::Plain, "文字"),
            (BadgeStyle::Box, "角丸の箱"),
            (BadgeStyle::Pill, "丸いバッジ"),
        ],
    );
    number(ui, "文字の大きさ (px)", &mut o.text_size_px, 8..=500);
    color(ui, "文字色", &mut o.text_color);
    if o.style == BadgeStyle::Plain {
        ui.checkbox(&mut o.outline, "黒の縁取り");
    } else {
        color(ui, "下地の色", &mut o.fill_color);
        let mut opacity = o.fill_opacity as u32;
        number(ui, "下地の不透明度 (%)", &mut opacity, 0..=100);
        o.fill_opacity = opacity as u8;
    }
    if o.placement == LabelPlacement::Overlay {
        anchor(ui, "ラベルの位置", &mut o.overlay_anchor);
        number(ui, "内側の余白 (px)", &mut o.overlay_margin_px, 0..=500);
    } else {
        number(ui, "左右の余白 (px)", &mut o.overlay_margin_px, 0..=500);
        let mut same = matches!(o.band_color, BandColor::Background);
        if ui.checkbox(&mut same, "帯を背景と同じ色にする").changed() {
            o.band_color = if same {
                BandColor::Background
            } else {
                BandColor::Color([255; 3])
            };
        }
        if let BandColor::Color(rgb) = &mut o.band_color {
            color(ui, "帯の色", rgb);
        }
        combo(
            ui,
            "帯のそろえ位置",
            &mut o.band_align,
            &[
                (HAlign::Left, "左"),
                (HAlign::Center, "中央"),
                (HAlign::Right, "右"),
            ],
        );
    }
}

fn draw_output(ui: &mut egui::Ui, state: &mut CombineDialogState, action: &mut Action) {
    ui.strong("出力");
    let blocked = state.replacement_is_source();
    if let Some(output) = &mut state.reopened {
        ui.add_enabled_ui(!blocked, |ui| {
            ui.checkbox(&mut output.replace, "元の結合画像を置き換える");
        });
        if blocked {
            ui.small("元の結合画像が素材に含まれるため、置き換えできません");
        }
        if output.replace {
            ui.small("元の画像を書き換えます。この操作は元に戻せません");
            if output.has_edits {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "この画像の注釈などの編集は残ります。配置を変えると位置がずれます",
                );
            }
        }
    }
    state.enforce_replacement_target();
    let replacing = state.replacement_path().is_some();
    let o = &mut state.options;
    let (mut mode, mut size) = match o.output_size {
        OutputSize::Original => (0, 2048),
        OutputSize::Width(n) => (1, n),
        OutputSize::Height(n) => (2, n),
        OutputSize::LongEdge(n) => (3, n),
    };
    combo(
        ui,
        "出力の大きさ",
        &mut mode,
        &[(0, "そのまま"), (1, "幅"), (2, "高さ"), (3, "長辺")],
    );
    if mode != 0 {
        number(ui, "指定サイズ (px)", &mut size, 16..=65500);
    }
    o.output_size = match mode {
        1 => OutputSize::Width(size),
        2 => OutputSize::Height(size),
        3 => OutputSize::LongEdge(size),
        _ => OutputSize::Original,
    };
    let mut format = match o.format {
        CombineFormat::Png => 0,
        CombineFormat::Jpeg { .. } => 1,
        CombineFormat::Webp { .. } => 2,
    };
    let previous = format;
    ui.add_enabled_ui(!replacing, |ui| {
        combo(
            ui,
            "形式",
            &mut format,
            &[(0, "PNG"), (1, "JPEG"), (2, "WebP")],
        )
    });
    if previous != format {
        o.format = match format {
            1 => CombineFormat::Jpeg { quality: 95 },
            2 => CombineFormat::Webp {
                quality: 90,
                lossless: false,
            },
            _ => CombineFormat::Png,
        };
    }
    match &mut o.format {
        CombineFormat::Jpeg { quality } | CombineFormat::Webp { quality, .. } => {
            let mut n = *quality as u32;
            number(ui, "品質", &mut n, 1..=100);
            *quality = n as u8;
        }
        _ => {}
    }
    if let CombineFormat::Webp { lossless, .. } = &mut o.format {
        ui.add_enabled_ui(!replacing, |ui| ui.checkbox(lossless, "可逆圧縮"));
    }
    if format == 1 && o.background == Background::Transparent {
        o.background = Background::Color([255; 3]);
    }
    ui.small("元の情報 (EXIF・AI プロンプトなど) は書きません");
    ui.add_enabled_ui(!replacing, |ui| {
        let first = matches!(o.destination, Destination::FirstImageFolder);
        if ui.radio(first, "先頭の画像のフォルダ").clicked() {
            o.destination = Destination::FirstImageFolder;
        }
        if ui.radio(!first, "指定のフォルダ").clicked() {
            o.destination = Destination::Folder(PathBuf::from(state.folder_text.trim()));
        }
        if let Destination::Folder(path) = &mut o.destination {
            ui.label("保存先フォルダ");
            if text(
                ui,
                &mut state.folder_text,
                "image_combine_folder",
                ui.available_width(),
            )
            .changed()
            {
                *path = PathBuf::from(state.folder_text.trim());
            }
            action.pick_folder |= ui.button("変更...").clicked();
        }
    });
}

#[cfg(test)]
mod image_combine_tests {
    use super::*;
    use egui_kittest::kittest::{NodeT, Queryable};

    #[test]
    fn image_combine_replace_output_controls_lock_target_and_show_edit_warning() {
        let ctx = egui::Context::default();
        let mut state = super::super::fixture_state(&ctx, false);
        state.reopened = Some(ReopenedOutput {
            path: PathBuf::from("C:/Pictures/combined.jpg"),
            format: CombineFormat::Jpeg { quality: 95 },
            replace: true,
            has_edits: true,
        });
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(700.0, 900.0))
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .build_ui_state(
                |ui, state: &mut CombineDialogState| {
                    let mut action = Action::default();
                    draw_output(ui, state, &mut action);
                    super::super::draw_footer(ui, state, &mut action);
                },
                state,
            );
        harness.run();
        assert!(
            harness
                .get_by_role_and_label(egui::accesskit::Role::TextInput, "名前")
                .accesskit_node()
                .is_disabled()
        );
        assert!(
            harness
                .get_by_label("指定のフォルダ")
                .accesskit_node()
                .is_disabled()
        );
        assert!(
            harness
                .get_by_label("変更...")
                .accesskit_node()
                .is_disabled()
        );
        harness.get_by_label("この画像の注釈などの編集は残ります。配置を変えると位置がずれます");
        assert!(
            !harness
                .get_by_label("元の結合画像を置き換える")
                .accesskit_node()
                .is_disabled()
        );
        harness.get_by_label("元の結合画像を置き換える").click();
        harness.run();
        assert!(
            !harness
                .get_by_role_and_label(egui::accesskit::Role::TextInput, "名前")
                .accesskit_node()
                .is_disabled()
        );
        assert!(
            !harness
                .get_by_label("変更...")
                .accesskit_node()
                .is_disabled()
        );
    }
}
