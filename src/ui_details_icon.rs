//! Small, theme-colored vector icons for the Details preview column.
//! Classification uses the typed grid item; painting never inspects a filename.

use eframe::egui;

use crate::grid_item::{GridItem, SearchContainerKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DetailsIconKind {
    Folder,
    Image,
    Video,
    Audio,
    Archive,
    Pdf,
    ZipImage,
    ZipDirFolder,
    ZipDirArchive,
    PdfPage,
    Stack,
    SearchFolder,
    SearchZip,
    Unavailable,
}

pub(crate) fn details_icon_kind(item: &GridItem) -> DetailsIconKind {
    match item {
        GridItem::Folder(_) => DetailsIconKind::Folder,
        GridItem::Image(_) => DetailsIconKind::Image,
        GridItem::Video(_) => DetailsIconKind::Video,
        GridItem::Audio(_) => DetailsIconKind::Audio,
        GridItem::ZipFile(_) => DetailsIconKind::Archive,
        GridItem::PdfFile(_) => DetailsIconKind::Pdf,
        GridItem::ConvertibleArchive { .. } => DetailsIconKind::Archive,
        GridItem::ZipImage { .. } => DetailsIconKind::ZipImage,
        GridItem::ZipDir {
            is_archive: false, ..
        } => DetailsIconKind::ZipDirFolder,
        GridItem::ZipDir {
            is_archive: true, ..
        } => DetailsIconKind::ZipDirArchive,
        GridItem::PdfPage { .. } => DetailsIconKind::PdfPage,
        GridItem::Stack { .. } => DetailsIconKind::Stack,
        GridItem::SearchContainer {
            kind: SearchContainerKind::Folder,
            ..
        } => DetailsIconKind::SearchFolder,
        GridItem::SearchContainer {
            kind: SearchContainerKind::Zip,
            ..
        } => DetailsIconKind::SearchZip,
        GridItem::CollectionPlaceholder { .. } => DetailsIconKind::Unavailable,
    }
}

struct IconCanvas<'a> {
    painter: &'a egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    color: egui::Color32,
}

impl IconCanvas<'_> {
    fn p(&self, x: f32, y: f32) -> egui::Pos2 {
        self.origin + egui::vec2(x * self.scale, y * self.scale)
    }

    fn line(&self, a: (f32, f32), b: (f32, f32)) {
        self.painter.line_segment(
            [self.p(a.0, a.1), self.p(b.0, b.1)],
            egui::Stroke::new(1.25, self.color),
        );
    }

    fn box_outline(&self, left: f32, top: f32, right: f32, bottom: f32) {
        self.painter.rect_stroke(
            egui::Rect::from_min_max(self.p(left, top), self.p(right, bottom)),
            1.0,
            egui::Stroke::new(1.25, self.color),
            egui::StrokeKind::Inside,
        );
    }

    fn image(&self, left: f32, top: f32, right: f32, bottom: f32) {
        self.box_outline(left, top, right, bottom);
        self.painter
            .circle_filled(self.p(left + 4.0, top + 4.0), 1.35 * self.scale, self.color);
        self.painter.add(egui::Shape::line(
            vec![
                self.p(left + 2.0, bottom - 2.0),
                self.p(left + 6.5, top + 7.0),
                self.p(left + 9.0, bottom - 4.0),
                self.p(right - 5.0, top + 6.5),
                self.p(right - 2.0, bottom - 2.0),
            ],
            egui::Stroke::new(1.2, self.color),
        ));
    }

    fn folder(&self) {
        self.line((1.5, 5.0), (1.5, 3.0));
        self.line((1.5, 3.0), (7.5, 3.0));
        self.line((7.5, 3.0), (9.5, 5.0));
        self.box_outline(1.5, 5.0, 18.5, 16.0);
    }

    fn archive(&self) {
        self.box_outline(2.5, 1.5, 17.5, 16.5);
        self.line((2.5, 5.0), (17.5, 5.0));
        self.line((10.0, 5.0), (10.0, 14.5));
        for y in [7.0, 9.5, 12.0, 14.5] {
            self.line((8.0, y), (12.0, y));
        }
    }

    fn pdf(&self) {
        self.line((4.0, 1.0), (12.0, 1.0));
        self.line((12.0, 1.0), (17.0, 6.0));
        self.line((17.0, 6.0), (17.0, 17.0));
        self.line((17.0, 17.0), (4.0, 17.0));
        self.line((4.0, 17.0), (4.0, 1.0));
        self.line((12.0, 1.0), (12.0, 6.0));
        self.line((12.0, 6.0), (17.0, 6.0));
        self.line((6.5, 9.0), (14.5, 9.0));
        self.line((6.5, 12.0), (13.5, 12.0));
        self.line((6.5, 15.0), (11.5, 15.0));
    }

    fn search_mark(&self) {
        self.painter.circle_stroke(
            self.p(15.0, 12.5),
            2.5 * self.scale,
            egui::Stroke::new(1.25, self.color),
        );
        self.line((16.8, 14.3), (19.5, 17.0));
    }
}

pub(crate) fn draw_details_preview_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    kind: DetailsIconKind,
    color: egui::Color32,
    muted: bool,
) {
    if rect.width() < 12.0 || rect.height() < 12.0 {
        return;
    }
    let alpha = if muted { 90 } else { color.a() };
    let color = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha);
    let scale = (rect.width() / 20.0).min(rect.height() / 18.0).min(1.0);
    let origin = rect.center() - egui::vec2(10.0 * scale, 9.0 * scale);
    let c = IconCanvas {
        painter,
        origin,
        scale,
        color,
    };
    match kind {
        DetailsIconKind::Folder => c.folder(),
        DetailsIconKind::Image => c.image(1.5, 1.5, 18.5, 16.5),
        DetailsIconKind::Video => {
            c.box_outline(1.5, 2.0, 18.5, 16.0);
            c.painter.add(egui::Shape::convex_polygon(
                vec![c.p(7.5, 5.0), c.p(7.5, 13.0), c.p(14.0, 9.0)],
                color,
                egui::Stroke::NONE,
            ));
        }
        DetailsIconKind::Audio => {
            c.line((8.0, 12.0), (8.0, 3.0));
            c.line((8.0, 3.0), (16.5, 1.5));
            c.line((16.5, 1.5), (16.5, 10.5));
            c.painter.circle_filled(c.p(5.0, 13.0), 2.6 * scale, color);
            c.painter.circle_filled(c.p(13.5, 11.5), 2.6 * scale, color);
        }
        DetailsIconKind::Archive => c.archive(),
        DetailsIconKind::Pdf => c.pdf(),
        DetailsIconKind::ZipImage => {
            c.box_outline(1.0, 1.0, 16.0, 14.0);
            c.image(4.0, 4.0, 19.0, 17.0);
        }
        DetailsIconKind::ZipDirFolder => {
            c.box_outline(1.0, 2.0, 15.0, 13.0);
            c.folder();
        }
        DetailsIconKind::ZipDirArchive => {
            c.archive();
            c.box_outline(11.5, 9.5, 19.0, 17.0);
            c.line((13.0, 12.0), (17.5, 12.0));
        }
        DetailsIconKind::PdfPage => {
            c.box_outline(1.0, 1.0, 14.0, 14.0);
            c.box_outline(5.0, 4.0, 18.0, 17.0);
            c.line((8.0, 9.0), (15.0, 9.0));
            c.line((8.0, 12.0), (15.0, 12.0));
        }
        DetailsIconKind::Stack => {
            c.box_outline(1.0, 1.0, 13.0, 12.0);
            c.box_outline(3.5, 3.5, 16.0, 14.5);
            c.image(6.0, 6.0, 19.0, 17.0);
        }
        DetailsIconKind::SearchFolder => {
            c.folder();
            c.search_mark();
        }
        DetailsIconKind::SearchZip => {
            c.archive();
            c.search_mark();
        }
        DetailsIconKind::Unavailable => {
            c.box_outline(3.5, 1.0, 16.5, 17.0);
            c.line((5.5, 14.5), (14.5, 5.5));
            c.line((5.5, 5.5), (14.5, 14.5));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_converter::ArchiveFormat;
    use crate::grid_item::CollectionPlaceholderReason;
    use std::path::PathBuf;

    #[test]
    fn maps_every_grid_kind_without_filename_guessing() {
        let path = PathBuf::from("misleading.zip");
        let cases = [
            (GridItem::Folder(path.clone()), DetailsIconKind::Folder),
            (GridItem::Image(path.clone()), DetailsIconKind::Image),
            (GridItem::Video(path.clone()), DetailsIconKind::Video),
            (GridItem::Audio(path.clone()), DetailsIconKind::Audio),
            (GridItem::ZipFile(path.clone()), DetailsIconKind::Archive),
            (GridItem::PdfFile(path.clone()), DetailsIconKind::Pdf),
            (
                GridItem::ConvertibleArchive {
                    path: path.clone(),
                    format: ArchiveFormat::Zip,
                },
                DetailsIconKind::Archive,
            ),
            (
                GridItem::ConvertibleArchive {
                    path: path.clone(),
                    format: ArchiveFormat::Rar,
                },
                DetailsIconKind::Archive,
            ),
            (
                GridItem::ConvertibleArchive {
                    path: path.clone(),
                    format: ArchiveFormat::SevenZ,
                },
                DetailsIconKind::Archive,
            ),
            (
                GridItem::ConvertibleArchive {
                    path: path.clone(),
                    format: ArchiveFormat::Lzh,
                },
                DetailsIconKind::Archive,
            ),
            (
                GridItem::ZipImage {
                    zip_path: path.clone(),
                    entry_name: "page.jpg".into(),
                },
                DetailsIconKind::ZipImage,
            ),
            (
                GridItem::ZipDir {
                    zip_path: path.clone(),
                    dir_prefix: "chapter/".into(),
                    is_archive: false,
                    representative: None,
                },
                DetailsIconKind::ZipDirFolder,
            ),
            (
                GridItem::ZipDir {
                    zip_path: path.clone(),
                    dir_prefix: "nested.zip/".into(),
                    is_archive: true,
                    representative: None,
                },
                DetailsIconKind::ZipDirArchive,
            ),
            (
                GridItem::PdfPage {
                    pdf_path: path.clone(),
                    page_num: 0,
                    content_type: None,
                },
                DetailsIconKind::PdfPage,
            ),
            (
                GridItem::Stack {
                    key: "p".into(),
                    representative: path.clone(),
                    count: 2,
                },
                DetailsIconKind::Stack,
            ),
            (
                GridItem::SearchContainer {
                    path: path.clone(),
                    kind: SearchContainerKind::Folder,
                    hit_count: 1,
                    representative: None,
                },
                DetailsIconKind::SearchFolder,
            ),
            (
                GridItem::SearchContainer {
                    path: path.clone(),
                    kind: SearchContainerKind::Zip,
                    hit_count: 1,
                    representative: None,
                },
                DetailsIconKind::SearchZip,
            ),
            (
                GridItem::CollectionPlaceholder {
                    path,
                    last_known_kind: crate::collection_store::CollectionResolvedKind::Image,
                    reason: CollectionPlaceholderReason::Missing,
                },
                DetailsIconKind::Unavailable,
            ),
        ];
        for (item, expected) in cases {
            assert_eq!(details_icon_kind(&item), expected, "{item:?}");
        }
    }
}
