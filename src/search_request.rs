//! Search-field prefill requests and the item names passed to them.

use std::path::Path;

use crate::grid_item::{GridItem, SearchContainerKind};
use crate::ui_pinned_search::PinnedSearchKind;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchRequest {
    pub target: PinnedSearchKind,
    pub query: String,
}

/// `None` is unsupported; `Some("")` is a supported item with an empty name.
/// Callers retain the latter entry and disable it rather than hiding it.
pub(crate) fn item_search_name(item: &GridItem) -> Option<String> {
    let name = match item {
        GridItem::Folder(path) => file_name(path),
        GridItem::Image(path)
        | GridItem::Video(path)
        | GridItem::Audio(path)
        | GridItem::ZipFile(path)
        | GridItem::PdfFile(path)
        | GridItem::ConvertibleArchive { path, .. } => file_stem(path),
        GridItem::ZipImage { entry_name, .. } => file_stem(Path::new(zip_basename(entry_name))),
        GridItem::ZipDir { dir_prefix, .. } => zip_basename(dir_prefix).to_owned(),
        GridItem::SearchContainer { path, kind, .. } => match kind {
            SearchContainerKind::Folder => file_name(path),
            SearchContainerKind::Zip => file_stem(path),
        },
        GridItem::PdfPage { .. }
        | GridItem::Stack { .. }
        | GridItem::CollectionPlaceholder { .. } => return None,
    };
    Some(name)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn zip_basename(entry: &str) -> &str {
    entry
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
}

fn select_all_id(input_id: egui::Id) -> egui::Id {
    input_id.with("search_request_select_all")
}

/// Select after the field actually receives focus: egui collapses unfocused selections.
pub(crate) fn request_select_all(ctx: &egui::Context, input_id: egui::Id, query: &str) {
    ctx.data_mut(|data| data.insert_temp(select_all_id(input_id), query.to_owned()));
}

/// Apply once, preserving IME composition and discarding requests after intervening edits.
pub(crate) fn apply_select_all(
    ctx: &egui::Context,
    query: &str,
    output: &mut egui::widgets::text_edit::TextEditOutput,
) {
    let id = select_all_id(output.response.id);
    let Some(requested) = ctx.data(|data| data.get_temp::<String>(id)) else {
        return;
    };
    if requested != query {
        ctx.data_mut(|data| data.remove::<String>(id));
        return;
    }
    if !output.response.has_focus() || crate::ime_focus::ime_input_active(ctx) {
        return;
    }
    let range = egui::text::CCursorRange::two(
        egui::text::CCursor::new(0),
        egui::text::CCursor::new(query.chars().count()),
    );
    output.state.cursor.set_char_range(Some(range));
    output.state.clone().store(ctx, output.response.id);
    output.cursor_range = Some(range);
    ctx.data_mut(|data| data.remove::<String>(id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn item_names_cover_each_supported_kind_without_display_name_rules() {
        let cases = [
            (GridItem::Folder("root/series.v2".into()), "series.v2"),
            (GridItem::Image("root/image.png".into()), "image"),
            (GridItem::Video("root/movie.mp4".into()), "movie"),
            (GridItem::Audio("root/music.flac".into()), "music"),
            (GridItem::ZipFile("root/book.cbz".into()), "book"),
            (GridItem::PdfFile("root/book.pdf".into()), "book"),
            (GridItem::PdfFile("root/book.epub".into()), "book"),
            (
                GridItem::ConvertibleArchive {
                    path: "root/book.7z".into(),
                    format: crate::archive_converter::ArchiveFormat::SevenZ,
                },
                "book",
            ),
            (
                GridItem::ZipImage {
                    zip_path: "outer.zip".into(),
                    entry_name: "chapter/inner.cbz/page.01.jpg".into(),
                },
                "page.01",
            ),
            (
                GridItem::ZipDir {
                    zip_path: "outer.zip".into(),
                    dir_prefix: "chapter/series.v2/".into(),
                    is_archive: false,
                    representative: None,
                },
                "series.v2",
            ),
            (
                GridItem::ZipDir {
                    zip_path: "outer.zip".into(),
                    dir_prefix: "chapter/inner.cbz/".into(),
                    is_archive: true,
                    representative: None,
                },
                "inner.cbz",
            ),
            (
                GridItem::SearchContainer {
                    path: "root/series.v2".into(),
                    kind: SearchContainerKind::Folder,
                    hit_count: 2,
                    representative: None,
                },
                "series.v2",
            ),
            (
                GridItem::SearchContainer {
                    path: "root/book.cbz".into(),
                    kind: SearchContainerKind::Zip,
                    hit_count: 2,
                    representative: None,
                },
                "book",
            ),
        ];
        for (item, expected) in cases {
            assert_eq!(
                item_search_name(&item).as_deref(),
                Some(expected),
                "{item:?}"
            );
        }
    }

    #[test]
    fn file_stems_preserve_leading_dots_spaces_and_all_but_the_last_extension() {
        for (name, expected) in [
            ("a.tar.gz", "a.tar"),
            (".hidden", ".hidden"),
            ("no_extension", "no_extension"),
            ("[作者] title 01.jpg", "[作者] title 01"),
            ("trailing.", "trailing"),
        ] {
            assert_eq!(
                item_search_name(&GridItem::Image(name.into())).as_deref(),
                Some(expected)
            );
            assert_eq!(
                item_search_name(&GridItem::ZipImage {
                    zip_path: "outer.zip".into(),
                    entry_name: format!("dir\\sub/{name}"),
                })
                .as_deref(),
                Some(expected)
            );
        }
    }

    #[test]
    fn empty_supported_names_are_distinct_from_unsupported_items() {
        for item in [
            GridItem::Image(PathBuf::new()),
            GridItem::Folder(PathBuf::new()),
            GridItem::ZipImage {
                zip_path: "outer.zip".into(),
                entry_name: String::new(),
            },
            GridItem::ZipDir {
                zip_path: "outer.zip".into(),
                dir_prefix: String::new(),
                is_archive: false,
                representative: None,
            },
        ] {
            assert_eq!(item_search_name(&item), Some(String::new()));
        }
        for item in [
            GridItem::PdfPage {
                pdf_path: "book.pdf".into(),
                page_num: 0,
                content_type: None,
            },
            GridItem::Stack {
                key: "series".into(),
                representative: "series1.png".into(),
                count: 2,
            },
            GridItem::CollectionPlaceholder {
                path: "missing.jpg".into(),
                last_known_kind: crate::collection_store::CollectionResolvedKind::Image,
                reason: crate::grid_item::CollectionPlaceholderReason::Missing,
            },
        ] {
            assert_eq!(item_search_name(&item), None);
        }
    }

    fn selection_pending(ctx: &egui::Context, id: egui::Id) -> bool {
        ctx.data(|data| data.get_temp::<String>(select_all_id(id)).is_some())
    }

    fn draw_field(
        ctx: &egui::Context,
        id: egui::Id,
        query: &mut String,
        focus: bool,
        events: Vec<egui::Event>,
    ) -> Option<egui::text::CCursorRange> {
        let mut range = None;
        crate::egui_output::discard_headless_output(ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                let mut request = focus;
                let mut output =
                    crate::ime_focus::show_singleline(ui, query, Some(&mut request), |edit| {
                        edit.id(id)
                    });
                apply_select_all(ui.ctx(), query, &mut output);
                range = output.state.cursor.char_range();
            },
        ));
        range
    }

    #[test]
    fn selection_waits_for_focus_then_selects_unicode_once_and_stores_state() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique("prefill");
        let mut query = "作者 title".to_owned();
        request_select_all(&ctx, id, &query);
        draw_field(&ctx, id, &mut query, false, vec![]);
        assert!(selection_pending(&ctx, id));
        let range = draw_field(&ctx, id, &mut query, true, vec![]).unwrap();
        assert_eq!(range.sorted_cursors()[0].index, 0);
        assert_eq!(range.sorted_cursors()[1].index, query.chars().count());
        assert!(!selection_pending(&ctx, id));
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap();
        assert_eq!(state.cursor.char_range(), Some(range));
        let caret = egui::text::CCursorRange::one(egui::text::CCursor::new(2));
        state.cursor.set_char_range(Some(caret));
        state.store(&ctx, id);
        assert_eq!(draw_field(&ctx, id, &mut query, false, vec![]), Some(caret));
    }

    #[test]
    fn selection_request_is_discarded_after_query_changes_even_without_focus() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique("prefill");
        request_select_all(&ctx, id, "old");
        draw_field(&ctx, id, &mut "new".to_owned(), false, vec![]);
        assert!(!selection_pending(&ctx, id));
    }

    #[test]
    #[expect(
        deprecated,
        reason = "Exercise compatibility IME notifications without changing the draft"
    )]
    fn selection_waits_for_ime_and_does_not_affect_sibling_fields() {
        let ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);
        let id = egui::Id::unique("prefill");
        let sibling = egui::Id::unique("sibling");
        let mut query = "作者".to_owned();
        draw_field(&ctx, id, &mut query, true, vec![]);
        request_select_all(&ctx, id, &query);
        // Enabled starts composition without inserting text into the draft.
        draw_field(
            &ctx,
            id,
            &mut query,
            false,
            vec![egui::Event::Ime(egui::ImeEvent::Enabled)],
        );
        assert!(selection_pending(&ctx, id));
        assert!(crate::ime_focus::ime_input_active(&ctx));
        assert!(!selection_pending(&ctx, sibling));
        let range = draw_field(
            &ctx,
            id,
            &mut query,
            false,
            vec![egui::Event::Ime(egui::ImeEvent::Disabled)],
        )
        .unwrap();
        assert_eq!(range.sorted_cursors()[0].index, 0);
        assert_eq!(range.sorted_cursors()[1].index, query.chars().count());
        assert!(!selection_pending(&ctx, id));
    }
}
