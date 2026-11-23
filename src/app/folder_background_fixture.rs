//! In-memory samples using the production cell painter, without an App or workers.

use super::{draw_cell, layout_cell_overlays};
use crate::folder_cell_background::FolderCellBackground;
use crate::grid_item::{GridItem, SearchContainerKind, ThumbnailState};

pub(crate) fn draw_samples(
    ui: &mut egui::Ui,
    colors: &FolderCellBackground,
    cell_visuals: &egui::Visuals,
    extended: bool,
) {
    let key = egui::Id::unique("folder-background-preview-texture");
    let texture = ui
        .ctx()
        .data(|data| data.get_temp::<egui::TextureHandle>(key))
        .unwrap_or_else(|| {
            let texture = ui.ctx().load_texture(
                "folder-background-representative",
                egui::ColorImage::filled([48, 32], egui::Color32::from_rgb(85, 140, 160)),
                Default::default(),
            );
            ui.ctx()
                .data_mut(|data| data.insert_temp(key, texture.clone()));
            texture
        });
    let loaded = ThumbnailState::loaded_raster(
        texture,
        crate::thumb_loader::ThumbLoadOrigin::FinalCache,
        false,
        128,
        crate::source_coverage::SourceCoverage::FullSource,
        Some((960, 640)),
        None,
    );
    let mut cases = vec![
        (
            "代表画像あり",
            GridItem::Folder("写真".into()),
            true,
            false,
            false,
            false,
        ),
        (
            "アイコンのみ",
            GridItem::Folder("資料".into()),
            false,
            false,
            false,
            false,
        ),
        (
            "検索結果",
            GridItem::SearchContainer {
                path: "G:/写真/春".into(),
                kind: SearchContainerKind::Folder,
                hit_count: 24,
                representative: None,
            },
            false,
            false,
            false,
            false,
        ),
        (
            "選択中",
            GridItem::Folder("選択".into()),
            false,
            true,
            false,
            false,
        ),
        (
            "画像",
            GridItem::Image("写真.jpg".into()),
            true,
            false,
            false,
            false,
        ),
    ];
    if extended {
        cases.extend([
            (
                "書庫内フォルダ",
                GridItem::ZipDir {
                    zip_path: "本.zip".into(),
                    dir_prefix: "資料/".into(),
                    is_archive: false,
                    representative: None,
                },
                false,
                false,
                false,
                false,
            ),
            (
                "チェック中",
                GridItem::Folder("チェック".into()),
                false,
                false,
                true,
                false,
            ),
            (
                "本",
                GridItem::ZipFile("本.zip".into()),
                false,
                false,
                false,
                false,
            ),
            (
                "ドライブ",
                GridItem::Folder("C:/".into()),
                false,
                false,
                false,
                true,
            ),
        ]);
    }
    let columns = if extended { 3 } else { 5 };
    let width = ((ui.available_width() - ui.spacing().item_spacing.x * (columns - 1) as f32)
        / columns as f32)
        .max(32.0);
    for row in cases.chunks(columns) {
        ui.horizontal_top(|ui| {
            for (label, item, has_image, selected, checked, drive) in row {
                ui.vertical(|ui| {
                    ui.set_width(width);
                    ui.small(*label);
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(width, 132.0), egui::Sense::hover());
                    let thumb = if *has_image {
                        &loaded
                    } else {
                        &ThumbnailState::Failed
                    };
                    // Headings stay on the surrounding surface. Only the cell
                    // painter uses the sample theme; child creation adds no layout.
                    let mut cell_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                    *cell_ui.visuals_mut() = cell_visuals.clone();
                    let ui = &cell_ui;
                    let layout = layout_cell_overlays(
                        ui.painter(),
                        rect,
                        Default::default(),
                        0,
                        item,
                        thumb,
                        &[],
                        None,
                        None,
                        *drive,
                        Default::default(),
                        Default::default(),
                        *checked,
                        None,
                        None,
                        false,
                        Default::default(),
                        true,
                        false,
                    );
                    draw_cell(
                        ui,
                        rect,
                        *selected,
                        *checked,
                        false,
                        &layout,
                        item,
                        thumb,
                        crate::rotation_db::Rotation::None,
                        None,
                        *drive,
                        Default::default(),
                        Default::default(),
                        false,
                        None,
                        colors,
                    );
                });
            }
        });
    }
}

#[doc(hidden)]
pub fn draw_folder_cell_background_snapshot_fixture(ui: &mut egui::Ui) {
    let cell_visuals = ui.visuals().clone();
    draw_samples(
        ui,
        &FolderCellBackground {
            enabled: true,
            ..Default::default()
        },
        &cell_visuals,
        true,
    );
}
