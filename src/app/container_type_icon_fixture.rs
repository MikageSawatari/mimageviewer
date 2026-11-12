//! App-free snapshot of loaded representatives projected through the grid's icon-only policy.

use super::{draw_cell_with_hit_areas, layout_cell_overlays};
use crate::grid_item::{GridItem, SearchContainerKind, ThumbnailState, effective_thumbnail_state};
use crate::settings::{AudioThumbnailIndicator, VideoThumbnailIndicator};

/// All inputs are fixed and in memory; no App, file read, scan or thumbnail worker.
#[doc(hidden)]
pub fn draw_container_type_icon_snapshot_fixture(ui: &mut egui::Ui) {
    let pixels = (0..48 * 32)
        .map(|i| {
            let (x, y) = (i % 48, i / 48);
            if y < 17 {
                egui::Color32::from_rgb(96 + x as u8, 156, 218)
            } else if y < 23 + x % 7 {
                egui::Color32::from_rgb(61, 117, 83)
            } else {
                egui::Color32::from_rgb(192, 161, 102)
            }
        })
        .collect();
    let loaded = ThumbnailState::loaded_raster(
        crate::ui_helpers::load_snapshot_texture(
            ui.ctx(),
            "container-icon-only-loaded-representative",
            egui::ColorImage::new([48, 32], pixels),
            Default::default(),
        ),
        crate::thumb_loader::ThumbLoadOrigin::FinalCache,
        false,
        128,
        crate::source_coverage::SourceCoverage::FullSource,
        Some((960, 640)),
        None,
    );
    let cases = [
        ("フォルダ", GridItem::Folder("春の写真".into()), false),
        ("ZIP", GridItem::ZipFile("春の作品集.zip".into()), false),
        ("PDF", GridItem::PdfFile("春の作品集.pdf".into()), false),
        ("ドライブ", GridItem::Folder("C:/".into()), true),
        (
            "RAR",
            GridItem::ConvertibleArchive {
                path: "春の作品集.rar".into(),
                format: crate::archive_converter::ArchiveFormat::Rar,
            },
            false,
        ),
        (
            "書庫内フォルダ",
            GridItem::ZipDir {
                zip_path: "本.zip".into(),
                dir_prefix: "春の写真/".into(),
                is_archive: false,
                representative: Some("001.jpg".into()),
            },
            false,
        ),
        (
            "検索のまとまり",
            GridItem::SearchContainer {
                path: "G:/写真/旅行/春".into(),
                kind: SearchContainerKind::Folder,
                hit_count: 24,
                representative: None,
            },
            false,
        ),
        (
            "検索の書庫",
            GridItem::SearchContainer {
                path: "G:/写真/旅行/春.zip".into(),
                kind: SearchContainerKind::Zip,
                hit_count: 12,
                representative: None,
            },
            false,
        ),
        (
            "入れ子書庫",
            GridItem::ZipDir {
                zip_path: "本.zip".into(),
                dir_prefix: "春の写真.zip/".into(),
                is_archive: true,
                representative: Some("001.jpg".into()),
            },
            false,
        ),
        ("画像", GridItem::Image("春の写真.jpg".into()), false),
        (
            "本の中の画像",
            GridItem::ZipImage {
                zip_path: "本.zip".into(),
                entry_name: "001.jpg".into(),
            },
            false,
        ),
        (
            "PDF のページ",
            GridItem::PdfPage {
                pdf_path: "本.pdf".into(),
                page_num: 0,
                content_type: None,
            },
            false,
        ),
    ];
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
    ui.label("コンテナの種類アイコン: ON / 左上の種類マーク: ON");
    for row in cases.chunks(4) {
        ui.horizontal_top(|ui| {
            for (label, item, drive) in row {
                ui.vertical(|ui| {
                    ui.set_width(174.0);
                    ui.label(egui::RichText::new(*label).small());
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(174.0, 122.0), egui::Sense::hover());
                    let thumb = effective_thumbnail_state(
                        item,
                        &loaded,
                        crate::grid_item::ContainerIconKinds::ALL,
                    );
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
                        VideoThumbnailIndicator::BottomLeftBadge,
                        AudioThumbnailIndicator::BottomLeftBadge,
                        false,
                        None,
                        None,
                        false,
                        Default::default(),
                        true,
                        false,
                    );
                    draw_cell_with_hit_areas(
                        ui,
                        rect,
                        false,
                        false,
                        false,
                        &layout,
                        item,
                        thumb,
                        crate::rotation_db::Rotation::None,
                        None,
                        *drive,
                        VideoThumbnailIndicator::BottomLeftBadge,
                        AudioThumbnailIndicator::BottomLeftBadge,
                        false,
                        None,
                    );
                });
            }
        });
    }
}
