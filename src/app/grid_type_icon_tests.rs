use super::*;
use crate::thumb_overlay_layout::{BadgeKind, THUMB_TYPE_ICON_SIZE, ThumbnailNameStyle};

fn items() -> Vec<(&'static str, GridItem)> {
    use crate::archive_converter::ArchiveFormat;
    use crate::grid_item::{
        CollectionPlaceholderReason, ContainerRepresentative, SearchContainerKind,
    };
    vec![
        (
            "フォルダ",
            GridItem::Folder("旅先で撮影した青空と街並み".into()),
        ),
        (
            "ZIP",
            GridItem::ZipFile("旅先で撮影した青空と街並み.zip".into()),
        ),
        (
            "変換 RAR",
            GridItem::ConvertibleArchive {
                path: "旅先で撮影した青空と街並み.rar".into(),
                format: ArchiveFormat::Rar,
            },
        ),
        (
            "PDF",
            GridItem::PdfFile("旅先で撮影した青空と街並み.pdf".into()),
        ),
        ("単体画像", GridItem::Image("写真.jpg".into())),
        (
            "動画",
            GridItem::Video("旅先で撮影した青空と街並み.mp4".into()),
        ),
        (
            "書庫内フォルダ",
            GridItem::ZipDir {
                zip_path: "本.zip".into(),
                dir_prefix: "旅先で撮影した青空と街並み/".into(),
                is_archive: false,
                representative: Some("a.jpg".into()),
            },
        ),
        ("音声", GridItem::Audio("旅先で録音した音楽.flac".into())),
        (
            "書庫内画像",
            GridItem::ZipImage {
                zip_path: "本.zip".into(),
                entry_name: "a.jpg".into(),
            },
        ),
        (
            "PDFページ",
            GridItem::PdfPage {
                pdf_path: "本.pdf".into(),
                page_num: 0,
                content_type: None,
            },
        ),
        (
            "入れ子書庫",
            GridItem::ZipDir {
                zip_path: "本.zip".into(),
                dir_prefix: "旅先の写真.rar/".into(),
                is_archive: true,
                representative: Some("a.jpg".into()),
            },
        ),
        (
            "画像スタック",
            GridItem::Stack {
                key: "写真".into(),
                representative: "写真.jpg".into(),
                count: 12,
            },
        ),
        (
            "集約フォルダ",
            GridItem::SearchContainer {
                path: "G:/写真/旅行/春".into(),
                kind: SearchContainerKind::Folder,
                hit_count: 24,
                representative: Some(ContainerRepresentative {
                    path: "写真.jpg".into(),
                    zip_entry: None,
                    pdf_page: None,
                }),
            },
        ),
        (
            "集約書庫",
            GridItem::SearchContainer {
                path: "G:/写真/旅行/春.zip".into(),
                kind: SearchContainerKind::Zip,
                hit_count: 12,
                representative: None,
            },
        ),
        (
            "参照元なし",
            GridItem::CollectionPlaceholder {
                path: "写真.jpg".into(),
                last_known_kind: crate::collection_store::CollectionResolvedKind::Image,
                reason: CollectionPlaceholderReason::Missing,
            },
        ),
        (
            "CBZ",
            GridItem::ZipFile("旅先で撮影した青空と街並み.cbz".into()),
        ),
    ]
}

fn thumbnail(ctx: &egui::Context, bright: bool) -> ThumbnailState {
    let size = [96, 72];
    let pixels = (0..size[1])
        .flat_map(|y| {
            (0..size[0]).map(move |x| {
                // A generated landscape with bright sky and dark foreground; no disk image/decode.
                if y < 35 {
                    if bright {
                        egui::Color32::from_rgb(200 + x as u8 / 3, 220, 245)
                    } else {
                        egui::Color32::from_rgb(22, 30, 52 + x as u8 / 4)
                    }
                } else if y < 50 + x % 19 {
                    if bright {
                        egui::Color32::from_rgb(110, 155, 105)
                    } else {
                        egui::Color32::from_rgb(34, 53, 49)
                    }
                } else if bright {
                    egui::Color32::from_rgb(230, 210, 159)
                } else {
                    egui::Color32::from_rgb(12, 18, 22)
                }
            })
        })
        .collect();
    ThumbnailState::loaded_raster(
        crate::ui_helpers::load_snapshot_texture(
            ctx,
            if bright { "1677-bright" } else { "1677-dark" },
            egui::ColorImage::new(size, pixels),
            Default::default(),
        ),
        crate::thumb_loader::ThumbLoadOrigin::FinalCache,
        false,
        128,
        crate::source_coverage::SourceCoverage::FullSource,
        Some((960, 720)),
        None,
    )
}

#[derive(Clone, Copy)]
struct Preview {
    size: Option<f32>,
    width: f32,
    height: f32,
    prioritize: bool,
    crowded: bool,
    font: f32,
    lines: usize,
    bookmark: bool,
    no_art: bool,
}
impl Default for Preview {
    fn default() -> Self {
        Self {
            size: Some(THUMB_TYPE_ICON_SIZE),
            width: 174.0,
            height: 132.0,
            prioritize: false,
            crowded: false,
            font: 1.0,
            lines: 1,
            bookmark: false,
            no_art: false,
        }
    }
}

fn draw(ui: &mut egui::Ui, p: Preview) {
    let bright = thumbnail(ui.ctx(), true);
    let dark = thumbnail(ui.ctx(), false);
    let cases = items();
    ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
    for (row_idx, row) in cases.chunks(4).enumerate() {
        ui.horizontal(|ui| {
            for (col, (label, item)) in row.iter().enumerate() {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(*label).small());
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(p.width, p.height), egui::Sense::hover());
                    let index = row_idx * 4 + col;
                    let thumb = if p.no_art {
                        &ThumbnailState::Failed
                    } else if index == 13 || index == 14 {
                        &ThumbnailState::NoArt
                    } else if index % 2 == 0 {
                        &bright
                    } else {
                        &dark
                    };
                    let flags = if p.crowded {
                        EditBadgeFlags {
                            page_override: true,
                            local_adjust: true,
                            mask: true,
                            conceal: true,
                            comic: true,
                            crop: true,
                            pin: true,
                        }
                    } else {
                        EditBadgeFlags::default()
                    };
                    let tags = if p.crowded {
                        vec!["旅行".to_owned(), "写真".to_owned()]
                    } else {
                        vec![]
                    };
                    let is_cut = p.crowded && index == 4;
                    let layout = layout_cell_overlays(
                        ui.painter(),
                        rect,
                        flags,
                        if item.accepts_rating() { 4 } else { 0 },
                        item,
                        thumb,
                        &tags,
                        p.bookmark.then_some("12:34"),
                        p.bookmark.then_some("お気に入りの場面"),
                        false,
                        VideoThumbnailIndicator::BottomLeftBadge,
                        AudioThumbnailIndicator::BottomLeftBadge,
                        p.crowded && index % 2 == 0,
                        Some(12),
                        matches!(item, GridItem::Video(_) | GridItem::Audio(_)).then_some("12:34"),
                        true,
                        ThumbnailNameStyle {
                            font_scale: p.font,
                            max_lines: p.lines,
                            prioritize_width: p.prioritize,
                        },
                        p.size,
                        is_cut,
                    );
                    let hits = draw_cell_with_hit_areas(
                        ui,
                        rect,
                        index == 1,
                        p.crowded && index % 2 == 0,
                        false,
                        &layout,
                        item,
                        thumb,
                        crate::rotation_db::Rotation::None,
                        None,
                        false,
                        VideoThumbnailIndicator::BottomLeftBadge,
                        AudioThumbnailIndicator::BottomLeftBadge,
                        is_cut,
                        Some(0.42),
                    );
                    if let Some(time) = layout.top_left.bookmark_time.as_ref() {
                        crate::ui_helpers::draw_overlay_bookmark_time_badge(
                            ui.painter(),
                            time,
                            false,
                        );
                    }
                    if let Some(title) = layout.bookmark_title.as_ref() {
                        crate::ui_helpers::draw_overlay_bookmark_title_badge(ui.painter(), title);
                    }
                    if let Some(icon) = layout.bottom_left.type_icon.as_ref() {
                        assert!(hits.contains(icon.rect.center()));
                    }
                    let badges: Vec<_> = layout.badge_placements().collect();
                    for (i, a) in badges.iter().enumerate() {
                        for b in badges.iter().skip(i + 1) {
                            assert!(!a.rect.intersects(b.rect), "{label}: {:?} / {:?}", a, b);
                        }
                    }
                });
            }
        });
    }
}

fn harness(
    theme: crate::os_theme::ResolvedTheme,
    configs: Vec<(&'static str, Preview)>,
) -> egui_kittest::Harness<'static> {
    let width = configs
        .iter()
        .map(|(_, p)| p.width * 4.0 + 30.0)
        .sum::<f32>()
        + 24.0;
    let height = configs
        .iter()
        .map(|(_, p)| (p.height + 24.0) * 4.0 + 50.0)
        .fold(0.0, f32::max);
    let mut ready = false;
    crate::test_ui::builder()
        .with_size(egui::vec2(width, height))
        .build_ui(move |root_ui| {
            crate::test_ui::full_viewport_ui(root_ui, |root_ui| {
                let ctx = &root_ui.ctx().clone();
                crate::os_theme::apply_resolved(ctx, theme);
                if !ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(root_ui, |ui| {
                    ui.horizontal_top(|ui| {
                        for (title, p) in &configs {
                            ui.vertical(|ui| {
                                ui.heading(*title);
                                draw(ui, *p);
                            });
                        }
                    });
                });
            });
        })
}

#[test]
fn thumbnail_type_icon_snapshot_dark() {
    let mut h = harness(
        crate::os_theme::ResolvedTheme::Dark,
        vec![("種類アイコン / 20pt", Preview::default())],
    );
    h.run();
    h.snapshot("thumbnail_type_icon_dark");
}
#[test]
fn thumbnail_type_icon_snapshot_light() {
    let mut h = harness(
        crate::os_theme::ResolvedTheme::Light,
        vec![("種類アイコン / 20pt", Preview::default())],
    );
    h.run();
    h.snapshot("thumbnail_type_icon_light");
}

#[test]
fn thumbnail_type_icon_preserves_small_primary_markers() {
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    let output = ctx.run_ui(Default::default(), |ui| {
        let loaded = thumbnail(ui.ctx(), true);
        for (_, item) in items() {
            for thumb in [&ThumbnailState::Pending, &ThumbnailState::Failed, &loaded] {
                for side in [32.0, 48.0, 60.0, 74.0, 94.0, 174.0] {
                    for meter in [false, true] {
                        for is_cut in [false, true] {
                            let cell = egui::Rect::from_min_size(
                                egui::pos2(10.0, 20.0),
                                egui::Vec2::splat(side),
                            );
                            let layout = layout_cell_overlays(
                                ui.painter(),
                                cell,
                                Default::default(),
                                0,
                                &item,
                                thumb,
                                &[],
                                None,
                                None,
                                false,
                                VideoThumbnailIndicator::PlayIcon,
                                AudioThumbnailIndicator::MusicNoteIcon,
                                false,
                                None,
                                None,
                                meter,
                                ThumbnailNameStyle::default(),
                                Some(THUMB_TYPE_ICON_SIZE),
                                is_cut,
                            );
                            if let Some(icon) = layout.bottom_left.type_icon {
                                if is_cut
                                    && let Some((center, radius)) =
                                        cut_badge_geometry(cell.shrink(4.0))
                                {
                                    assert!(!icon.rect.intersects(egui::Rect::from_center_size(
                                        center,
                                        egui::Vec2::splat(radius * 2.0)
                                    )));
                                }
                                for obstacle in primary_marker_obstacles(
                                    ui.painter(),
                                    cell.shrink(4.0),
                                    &item,
                                    thumb,
                                    false,
                                    if is_cut {
                                        VideoThumbnailIndicator::Hidden
                                    } else {
                                        VideoThumbnailIndicator::PlayIcon
                                    },
                                    AudioThumbnailIndicator::MusicNoteIcon,
                                ) {
                                    assert!(
                                        !icon.rect.intersects(obstacle),
                                        "{item:?}, side={side}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    });
    let _ = crate::test_ui::headless_paint_output(output);
}

#[test]
#[ignore = "Writes user comparison images into target/1677-preview; run explicitly"]
fn thumbnail_type_icon_write_previews() {
    let output = std::path::Path::new("target/1677-preview");
    std::fs::create_dir_all(output).unwrap();
    let mut readme = String::from(
        "§1.677 試作比較。製品起動なし・共通の本体セル描画へ生成画像を渡した headless PNG。\n寸法は logical pt、1pt=1px。背景の枠は各辺2pt、glyph16/20/24ptのplate20/24/28pt。\n各一覧の項目・明暗画像・選択・メーターを固定。左からフォルダ/ZIP/RAR/PDF、画像/動画/書庫内フォルダ/音声、書庫内画像/PDFページ/入れ子書庫/スタック、集約フォルダ/集約書庫/参照元なし/CBZ。\n集約書庫と参照元なしは画像なし。FinalCacheの生成textureを利用。他の全状態は回帰テストで検査。\n\n",
    );
    for (name, theme) in [
        ("dark", crate::os_theme::ResolvedTheme::Dark),
        ("light", crate::os_theme::ResolvedTheme::Light),
    ] {
        let p = Preview::default();
        let suites = vec![
            (
                "off-on",
                vec![("OFF", Preview { size: None, ..p }), ("ON / 20pt", p)],
                "通常セル174×132、名前幅優先OFF、OFF/ONを左右比較",
            ),
            (
                "sizes",
                vec![
                    (
                        "小 / 16pt",
                        Preview {
                            size: Some(16.0),
                            ..p
                        },
                    ),
                    ("中 / 20pt", p),
                    (
                        "大 / 24pt",
                        Preview {
                            size: Some(24.0),
                            ..p
                        },
                    ),
                ],
                "同じ一覧でglyph16/20/24ptを左右比較",
            ),
            (
                "narrow",
                vec![
                    (
                        "名前幅優先 OFF",
                        Preview {
                            width: 74.0,
                            height: 94.0,
                            ..p
                        },
                    ),
                    (
                        "名前幅優先 ON",
                        Preview {
                            width: 74.0,
                            height: 94.0,
                            prioritize: true,
                            ..p
                        },
                    ),
                ],
                "狭いセル74×94、名前幅優先OFF/ON",
            ),
            (
                "crowded",
                vec![
                    ("名前幅優先 OFF", Preview { crowded: true, ..p }),
                    (
                        "名前幅優先 ON",
                        Preview {
                            crowded: true,
                            prioritize: true,
                            ..p
                        },
                    ),
                ],
                "174×132、★/タグ/全編集状態/チェック/スタック枚数/長さ/メーター、幅優先OFF/ON",
            ),
            (
                "no-art",
                vec![
                    (
                        "代表画像なし / OFF",
                        Preview {
                            size: None,
                            no_art: true,
                            ..p
                        },
                    ),
                    ("代表画像なし / ON", Preview { no_art: true, ..p }),
                ],
                "174×132、全種類で代表画像なし、OFF/ON比較",
            ),
            (
                "large-names-bookmarks",
                vec![
                    (
                        "名前200% / 4行",
                        Preview {
                            font: 2.0,
                            lines: 4,
                            prioritize: true,
                            ..p
                        },
                    ),
                    (
                        "ブックマーク",
                        Preview {
                            bookmark: true,
                            prioritize: true,
                            ..p
                        },
                    ),
                ],
                "174×132、名前200%最大4行、中央ブックマークタイトル",
            ),
        ];
        for (suite, configs, description) in suites {
            let file = format!("{suite}-{name}.png");
            let mut h = harness(theme, configs);
            h.run();
            h.render().unwrap().save(output.join(&file)).unwrap();
            readme.push_str(&format!("{file}: {name} / {description}\n"));
        }
    }
    std::fs::write(output.join("README.txt"), readme).unwrap();
}

#[test]
fn thumbnail_type_icon_all_states_contexts_and_hit_regions() {
    for theme in [
        crate::os_theme::ResolvedTheme::Dark,
        crate::os_theme::ResolvedTheme::Light,
    ] {
        for dpi in [1.0, 2.0] {
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(dpi);
            crate::os_theme::apply_resolved(&ctx, theme);
            crate::ui_fonts::configure_fonts(&ctx);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 800.0),
                )),
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                let loaded = thumbnail(ui.ctx(), true);
                let mut cached = loaded.clone();
                if let ThumbnailState::Loaded { origin, .. } = &mut cached {
                    *origin = crate::thumb_loader::ThumbLoadOrigin::UpgradeableCache;
                }
                for thumb in [
                    &ThumbnailState::Pending,
                    &ThumbnailState::Evicted,
                    &ThumbnailState::NoArt,
                    &ThumbnailState::Failed,
                    &loaded,
                    &cached,
                ] {
                    for (label, item) in items() {
                        for width in [60.0, 74.0, 174.0, 300.0] {
                            for prioritize in [false, true] {
                                for bookmark in [false, true] {
                                    let rect = egui::Rect::from_min_size(
                                        egui::pos2(20.0, 30.0),
                                        egui::vec2(width, 180.0),
                                    );
                                    let style = ThumbnailNameStyle {
                                        prioritize_width: prioritize,
                                        ..Default::default()
                                    };
                                    let make = |size| {
                                        layout_cell_overlays(
                                            ui.painter(),
                                            rect,
                                            EditBadgeFlags::default(),
                                            0,
                                            &item,
                                            thumb,
                                            &[],
                                            bookmark.then_some("12:34"),
                                            bookmark.then_some("場面"),
                                            false,
                                            VideoThumbnailIndicator::BottomLeftBadge,
                                            AudioThumbnailIndicator::BottomLeftBadge,
                                            false,
                                            None,
                                            None,
                                            true,
                                            style,
                                            size,
                                            false,
                                        )
                                    };
                                    let off = make(None);
                                    let on = make(Some(THUMB_TYPE_ICON_SIZE));
                                    assert!(off.bottom_left.type_icon.is_none());
                                    if width >= 174.0 {
                                        assert!(
                                            on.bottom_left.type_icon.is_some(),
                                            "missing icon: {label}, bookmark={bookmark}"
                                        );
                                    }
                                    if let Some(icon) = on.bottom_left.type_icon.as_ref() {
                                        assert!(rect.contains_rect(icon.rect));
                                        for obstacle in custom_caption_obstacles(
                                            ui.painter(),
                                            rect,
                                            &item,
                                            thumb,
                                            true,
                                        ) {
                                            assert!(
                                                !icon.rect.intersects(obstacle),
                                                "custom content: {label}"
                                            );
                                        }
                                        let hits = draw_cell_with_hit_areas(
                                            ui,
                                            rect,
                                            false,
                                            false,
                                            false,
                                            &on,
                                            &item,
                                            thumb,
                                            crate::rotation_db::Rotation::None,
                                            None,
                                            false,
                                            VideoThumbnailIndicator::BottomLeftBadge,
                                            AudioThumbnailIndicator::BottomLeftBadge,
                                            false,
                                            None,
                                        );
                                        for pos in [
                                            icon.rect.center(),
                                            icon.rect.min + egui::vec2(0.1, 0.1),
                                            icon.rect.max - egui::vec2(0.1, 0.1),
                                        ] {
                                            assert!(
                                                hits.contains(pos),
                                                "icon classified as background: {label}"
                                            );
                                        }
                                    }
                                    let badges: Vec<_> = on.badge_placements().collect();
                                    for (i, a) in badges.iter().enumerate() {
                                        for b in badges.iter().skip(i + 1) {
                                            assert!(
                                                !a.rect.intersects(b.rect),
                                                "{label}: {a:?} / {b:?}"
                                            );
                                        }
                                    }
                                    assert_eq!(on.book_resume_meter, off.book_resume_meter);
                                }
                            }
                        }
                    }
                }
            });
            let _ = crate::test_ui::headless_paint_output(output);
        }
    }
}
