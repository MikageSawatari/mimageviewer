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

#[derive(Clone)]
struct SelectAllRequest {
    issued_pass: u64,
    query: String,
}

/// The field's first draw consumes this request, even if selection is no longer appropriate.
pub(crate) fn request_select_all(ctx: &egui::Context, input_id: egui::Id, query: &str) {
    let request = SelectAllRequest {
        issued_pass: ctx.cumulative_pass_nr(),
        query: query.to_owned(),
    };
    ctx.data_mut(|data| data.insert_temp(select_all_id(input_id), request));
}

/// Ordinary search openings discard any prefill request left by a previous session.
pub(crate) fn clear_select_all(ctx: &egui::Context, input_id: egui::Id) {
    ctx.data_mut(|data| data.remove::<SelectAllRequest>(select_all_id(input_id)));
}

#[cfg(test)]
pub(crate) fn select_all_pending_for_test(ctx: &egui::Context, input_id: egui::Id) -> bool {
    ctx.data(|data| {
        data.get_temp::<SelectAllRequest>(select_all_id(input_id))
            .is_some()
    })
}

/// Apply only on the first draw in the issuing or following pass, preserving IME composition.
pub(crate) fn apply_select_all(
    ctx: &egui::Context,
    query: &str,
    focus_requested: bool,
    output: &mut egui::widgets::text_edit::TextEditOutput,
) {
    let id = select_all_id(output.response.id);
    let pass = ctx.cumulative_pass_nr();
    let Some(requested) = ctx.data_mut(|data| {
        let requested = data.get_temp::<SelectAllRequest>(id)?;
        if pass < requested.issued_pass {
            return None;
        }
        data.remove::<SelectAllRequest>(id);
        Some(requested)
    }) else {
        return;
    };
    if requested.query != query
        || pass - requested.issued_pass > 1
        || !(focus_requested || output.response.has_focus())
        || crate::ime_focus::ime_input_active(ctx)
    {
        return;
    }
    let range = egui::text::CCursorRange::two(
        egui::text::CCursor::new(0),
        egui::text::CCursor::new(query.chars().count()),
    );
    output.state.cursor.set_char_range(Some(range));
    output.state.clone().store(ctx, output.response.id);
    output.cursor_range = Some(range);
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
        select_all_pending_for_test(ctx, id)
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
                apply_select_all(ui.ctx(), query, focus, &mut output);
                range = output.state.cursor.char_range();
            },
        ));
        range
    }

    #[test]
    fn selection_selects_unicode_once_and_stores_state_on_focus_request() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique("prefill");
        let mut query = "作者 title".to_owned();
        request_select_all(&ctx, id, &query);
        let range = draw_field(&ctx, id, &mut query, true, vec![]).unwrap();
        assert_eq!(range.sorted_cursors()[0].index.0, 0);
        assert_eq!(range.sorted_cursors()[1].index.0, query.chars().count());
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
    fn selection_request_is_discarded_on_first_draw_without_focus() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique("prefill");
        let mut query = "title".to_owned();
        request_select_all(&ctx, id, &query);
        draw_field(&ctx, id, &mut query, false, vec![]);
        assert!(!selection_pending(&ctx, id));
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap();
        let caret = egui::text::CCursorRange::one(egui::text::CCursor::new(2));
        state.cursor.set_char_range(Some(caret));
        state.store(&ctx, id);
        assert_eq!(draw_field(&ctx, id, &mut query, true, vec![]), Some(caret));
    }

    #[test]
    fn cleared_selection_does_not_apply_on_a_later_focus_request() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique("prefill");
        let sibling = egui::Id::unique("sibling");
        let mut query = "title".to_owned();
        draw_field(&ctx, id, &mut query, false, vec![]);
        request_select_all(&ctx, id, &query);
        request_select_all(&ctx, sibling, &query);
        clear_select_all(&ctx, id);
        assert!(!selection_pending(&ctx, id));
        assert!(selection_pending(&ctx, sibling));
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap();
        let caret = egui::text::CCursorRange::one(egui::text::CCursor::new(2));
        state.cursor.set_char_range(Some(caret));
        state.store(&ctx, id);
        assert_eq!(draw_field(&ctx, id, &mut query, true, vec![]), Some(caret));
        assert!(selection_pending(&ctx, sibling));
    }

    #[test]
    fn selection_request_selects_on_issuing_or_next_pass_and_expires_after_two() {
        for age in [0, 1, 2] {
            let ctx = egui::Context::default();
            let id = egui::Id::unique("prefill");
            let mut query = "title".to_owned();
            request_select_all(&ctx, id, &query);
            let issued_pass = ctx.cumulative_pass_nr();
            while ctx.cumulative_pass_nr() < issued_pass + age {
                ctx.run_ui(Default::default(), |_| {})
                    .drop_without_applying_deltas();
            }
            assert!(selection_pending(&ctx, id));
            let range = draw_field(&ctx, id, &mut query, true, vec![]);
            if age <= 1 {
                let range = range.expect("current or next pass must select the entire query");
                assert_eq!(range.sorted_cursors()[0].index.0, 0, "age={age}");
                assert_eq!(
                    range.sorted_cursors()[1].index.0,
                    query.chars().count(),
                    "age={age}"
                );
            } else {
                assert!(range.is_none_or(|range| range.is_empty()), "age={age}");
            }
            assert!(!selection_pending(&ctx, id));
        }
    }

    #[test]
    fn selection_request_uses_existing_focus_without_a_new_focus_request() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique("prefill");
        let mut query = "title".to_owned();
        draw_field(&ctx, id, &mut query, true, vec![]);
        request_select_all(&ctx, id, &query);
        let range = draw_field(&ctx, id, &mut query, false, vec![]).unwrap();
        assert_eq!(range.sorted_cursors()[0].index.0, 0);
        assert_eq!(range.sorted_cursors()[1].index.0, query.chars().count());
        assert!(!selection_pending(&ctx, id));
    }

    #[test]
    fn selection_accepts_the_focus_request_pass_before_response_has_focus() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique("prefill");
        let mut query = "作者 title".to_owned();
        request_select_all(&ctx, id, &query);
        ctx.run_ui(Default::default(), |ui| {
            let mut output =
                crate::ime_focus::show_singleline(ui, &mut query, None, |edit| edit.id(id));
            assert!(!output.response.has_focus());
            apply_select_all(ui.ctx(), &query, true, &mut output);
            let range = output.state.cursor.char_range().unwrap();
            assert_eq!(range.sorted_cursors()[0].index.0, 0);
            assert_eq!(range.sorted_cursors()[1].index.0, query.chars().count());
            assert_eq!(output.cursor_range, Some(range));
            assert!(!selection_pending(ui.ctx(), id));
        })
        .drop_without_applying_deltas();
    }

    #[test]
    fn selection_requests_are_independent_between_fields() {
        let ctx = egui::Context::default();
        let first = egui::Id::unique("first");
        let second = egui::Id::unique("second");
        let mut query = "title".to_owned();
        request_select_all(&ctx, first, &query);
        request_select_all(&ctx, second, &query);
        draw_field(&ctx, first, &mut query, true, vec![]);
        assert!(!selection_pending(&ctx, first));
        assert!(selection_pending(&ctx, second));
        let range = draw_field(&ctx, second, &mut query, true, vec![]).unwrap();
        assert_eq!(range.sorted_cursors()[0].index.0, 0);
        assert_eq!(range.sorted_cursors()[1].index.0, query.chars().count());
        assert!(!selection_pending(&ctx, second));
    }

    #[test]
    #[expect(
        deprecated,
        reason = "Exercise compatibility IME notifications without changing the draft"
    )]
    fn selection_is_discarded_during_ime_and_does_not_select_after_composition() {
        let ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);
        let id = egui::Id::unique("prefill");
        let mut query = "作者".to_owned();
        draw_field(&ctx, id, &mut query, true, vec![]);
        request_select_all(&ctx, id, &query);
        // Enabled starts composition without inserting text into the draft.
        let range = draw_field(
            &ctx,
            id,
            &mut query,
            false,
            vec![egui::Event::Ime(egui::ImeEvent::Enabled)],
        );
        assert!(!selection_pending(&ctx, id));
        assert!(crate::ime_focus::ime_input_active(&ctx));
        assert!(range.unwrap().is_empty());
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap();
        let caret = egui::text::CCursorRange::one(egui::text::CCursor::new(1));
        state.cursor.set_char_range(Some(caret));
        state.store(&ctx, id);
        let range = draw_field(
            &ctx,
            id,
            &mut query,
            false,
            vec![egui::Event::Ime(egui::ImeEvent::Disabled)],
        );
        assert!(range.unwrap().is_empty());
        assert!(!selection_pending(&ctx, id));
    }
}
