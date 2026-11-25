//! §1.885: production key migration followed by the reader's identity resolver.
use super::*;
use crate::{
    app::{
        book_materialization::{materialize_archive_level, materialize_cached_document_rows},
        file_type_resume::{PositionResolution, PreparedPageIdentityIndex},
    },
    book_resume_db::{
        BookResumeDb, BookResumeWriter, PageIdentity, ReadingMeterValue, ReadingPosition,
    },
    book_resume_key::BookResumeKey,
    grid_item::GridItem,
};

pub(crate) const SAVED_PAGE: usize = 2;
pub(crate) const PAGE_COUNT: usize = 4;

// Use the same pure worker materializers as book open; no image decoding or GUI.
pub(crate) fn archive_pages(backing: &Path, prefix: &[String]) -> Vec<GridItem> {
    let base = if prefix.is_empty() {
        String::new()
    } else {
        format!("{}/", prefix.join("/"))
    };
    let tree = crate::zip_tree::ZipTree::build(
        backing.to_path_buf(),
        (0..PAGE_COUNT)
            .map(|page| crate::zip_loader::ZipImageEntry {
                entry_name: format!("{base}{page:02}.jpg"),
                mtime: 1,
                uncompressed_size: 10,
                container_ancestry: Vec::new(),
            })
            .collect(),
    );
    materialize_archive_level(&tree, prefix, crate::app::BOOK_READING_PAGE_ORDER, None).0
}

fn document_pages(path: &Path) -> Vec<GridItem> {
    materialize_cached_document_rows(path, PAGE_COUNT as u32, 1, 10).0
}

fn image_pages(folder: &Path) -> Vec<GridItem> {
    (0..PAGE_COUNT)
        .map(|page| GridItem::Image(folder.join(format!("{page:02}.jpg"))))
        .collect()
}

pub(crate) fn record_position(
    data: &Path,
    key: &BookResumeKey,
    pages: &[GridItem],
    logical: Option<&Path>,
    page: usize,
) -> ReadingPosition {
    let position = ReadingPosition {
        page,
        identity: Some(PageIdentity::for_item(&pages[page], logical).unwrap()),
    };
    let writer = BookResumeWriter::spawn_at(data.join("book_resume.db")).unwrap();
    assert!(writer.record_key_identity(
        key,
        page,
        ReadingMeterValue::new(page + 1, pages.len()),
        position.identity.clone(),
    ));
    // Drop drains the real FIFO and joins its writer before external maintenance.
    drop(writer);
    assert_eq!(read_position(data, key), Some(position.clone()));
    position
}

pub(crate) fn read_position(data: &Path, key: &BookResumeKey) -> Option<ReadingPosition> {
    BookResumeDb::open_at(&data.join("book_resume.db"))
        .unwrap()
        .get_position_key(key)
}

pub(crate) fn assert_restored(
    data: &Path,
    key: &BookResumeKey,
    pages: &[GridItem],
    logical: Option<&Path>,
    page: usize,
) {
    let position = read_position(data, key).unwrap();
    assert_eq!(
        position.page, page,
        "migration must never write the page column"
    );
    assert_eq!(
        position.identity,
        PageIdentity::for_item(&pages[page], logical)
    );
    let prepared = PreparedPageIdentityIndex::prepare(pages, logical, Default::default());
    assert_eq!(
        prepared.resolve(&position),
        PositionResolution::Restored(page)
    );
    // Also prove restoration follows the page identity, not a raw-index fallback.
    let mut reordered = pages.to_vec();
    reordered.reverse();
    let prepared = PreparedPageIdentityIndex::prepare(&reordered, logical, Default::default());
    assert_eq!(
        prepared.resolve(&position),
        PositionResolution::Restored(pages.len() - 1 - page)
    );
    let conn = rusqlite::Connection::open(data.join("book_resume.db")).unwrap();
    let meter: (usize, usize) = conn
        .query_row(
            "SELECT page_ordinal,page_total FROM book_resume WHERE path=?1",
            [key.storage_key()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(meter, (page + 1, pages.len()));
}

fn copy_book(data: &Path, old: &Path, new: &Path) {
    let report = copy_stores_at(
        data,
        &[
            StoreCopyPathMapping::exact(old, new),
            StoreCopyPathMapping::virtual_prefix(old, new),
        ],
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
}

fn rename_book(data: &Path, old: &Path, new: &Path) {
    let report = run_at(data, old, new);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
}

#[test]
fn section1886_file_rename_restores_logical_and_cache_identities() {
    for extension in ["rar", "cbr", "7z", "cb7", "lzh", "lha", "zip", "cbz"] {
        for logical_identity in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let old = PathBuf::from(format!("C:/section1886/old.{extension}"));
            let new = PathBuf::from(format!("C:/section1886/new.{extension}"));
            let old_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), &old);
            let new_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), &new);
            let old_key = BookResumeKey::legacy(&old_cache);
            record_position(
                temp.path(),
                &old_key,
                &archive_pages(&old_cache, &[]),
                logical_identity.then_some(old.as_path()),
                SAVED_PAGE,
            );
            let report = run_at_with_shape(temp.path(), &old, &new, false);
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            assert_eq!(
                report.converted_cache_pairs,
                vec![(old_cache, new_cache.clone())]
            );
            assert!(read_position(temp.path(), &old_key).is_none());
            assert_restored(
                temp.path(),
                &BookResumeKey::legacy(&new_cache),
                &archive_pages(&new_cache, &[]),
                logical_identity.then_some(new.as_path()),
                SAVED_PAGE,
            );
        }
    }
}

#[test]
fn section1886_folder_snapshot_moves_two_books_and_preserves_outside_books() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let old = temp.path().join("old");
    let new = temp.path().join("new");
    std::fs::create_dir_all(new.join("nested")).unwrap();
    for relative in ["one.rar", "nested/two.7z", "plain.zip", "photo.jpg"] {
        std::fs::write(new.join(relative), b"fixture").unwrap();
    }
    let enumeration = collect_converted_archive_rels(&new);
    assert!(!enumeration.incomplete);
    assert_eq!(enumeration.converted_archive_rels.len(), 3);
    for relative in ["one.rar", "nested/two.7z"] {
        let source = old.join(relative);
        let cache = crate::archive_cache::cache_zip_path_for_data_dir(&data, &source);
        record_position(
            &data,
            &BookResumeKey::legacy(&cache),
            &archive_pages(&cache, &[]),
            Some(&source),
            SAVED_PAGE,
        );
    }
    let plain_old = old.join("plain.zip");
    record_position(
        &data,
        &BookResumeKey::legacy(&plain_old),
        &archive_pages(&plain_old, &[]),
        None,
        1,
    );
    let outside = temp.path().join("outside.rar");
    let outside_cache = crate::archive_cache::cache_zip_path_for_data_dir(&data, &outside);
    let outside_before = record_position(
        &data,
        &BookResumeKey::legacy(&outside_cache),
        &archive_pages(&outside_cache, &[]),
        Some(&outside),
        1,
    );
    let report = run_at_with_converted_archives(
        &data,
        &old,
        &new,
        true,
        &enumeration.converted_archive_rels,
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        report.converted_cache_pairs.len(),
        2,
        "the unopened ordinary ZIP has no cache rows"
    );
    for relative in ["one.rar", "nested/two.7z"] {
        let source = new.join(relative);
        let cache = crate::archive_cache::cache_zip_path_for_data_dir(&data, &source);
        assert_restored(
            &data,
            &BookResumeKey::legacy(&cache),
            &archive_pages(&cache, &[]),
            Some(&source),
            SAVED_PAGE,
        );
        let old_cache =
            crate::archive_cache::cache_zip_path_for_data_dir(&data, &old.join(relative));
        assert!(read_position(&data, &BookResumeKey::legacy(&old_cache)).is_none());
    }
    let plain_new = new.join("plain.zip");
    assert_restored(
        &data,
        &BookResumeKey::legacy(&plain_new),
        &archive_pages(&plain_new, &[]),
        None,
        1,
    );
    assert_eq!(
        read_position(&data, &BookResumeKey::legacy(&outside_cache)),
        Some(outside_before)
    );
}

#[test]
fn section1886_shared_drive_stripped_cache_row_follows_renamed_side() {
    let temp = tempfile::tempdir().unwrap();
    let old = Path::new("C:/section1886/parent/book.rar");
    let other_drive = Path::new("D:/section1886/parent/book.rar");
    let new = Path::new("C:/section1886/renamed/book.rar");
    let cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), old);
    assert_eq!(
        cache,
        crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), other_drive)
    );
    record_position(
        temp.path(),
        &BookResumeKey::legacy(&cache),
        &archive_pages(&cache, &[]),
        None,
        SAVED_PAGE,
    );
    let report = run_at_with_converted_archives(
        temp.path(),
        old.parent().unwrap(),
        new.parent().unwrap(),
        true,
        &[PathBuf::from("book.rar")],
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let destination = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), new);
    assert_restored(
        temp.path(),
        &BookResumeKey::legacy(&destination),
        &archive_pages(&destination, &[]),
        None,
        SAVED_PAGE,
    );
    assert!(read_position(temp.path(), &BookResumeKey::legacy(&cache)).is_none());
}

#[test]
fn section1886_namespace_identity_is_rewritten_once_for_files_and_folders() {
    for tree in [false, true] {
        for logical_identity in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let old_root = Path::new("C:/section1886/old");
            // Destination inside the old spelling exposes accidental second rewrites.
            let new_root = Path::new("C:/section1886/old/new");
            let old = old_root.join("book.rar");
            let new = new_root.join("book.rar");
            let old_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), &old);
            let new_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), &new);
            let prefix = vec!["Chapter".into(), "nested.zip".into()];
            let old_key = BookResumeKey::archive(&old, &prefix).unwrap();
            record_position(
                temp.path(),
                &old_key,
                &archive_pages(&old_cache, &prefix),
                logical_identity.then_some(old.as_path()),
                SAVED_PAGE,
            );
            let report = if tree {
                run_at_with_converted_archives(
                    temp.path(),
                    old_root,
                    new_root,
                    true,
                    &[PathBuf::from("book.rar")],
                )
            } else {
                run_at_with_shape(temp.path(), &old, &new, false)
            };
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            assert!(read_position(temp.path(), &old_key).is_none());
            assert_restored(
                temp.path(),
                &BookResumeKey::archive(&new, &prefix).unwrap(),
                &archive_pages(&new_cache, &prefix),
                logical_identity.then_some(new.as_path()),
                SAVED_PAGE,
            );
            assert_eq!(
                report.converted_cache_pairs.len(),
                usize::from(!logical_identity)
            );
        }
    }
}

#[test]
fn section1886_case_only_cache_keys_are_noop_and_position_stays_restored() {
    for logical_identity in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let old = Path::new("C:/section1886/Book.RAR");
        let new = Path::new("C:/section1886/book.rar");
        let cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), old);
        record_position(
            temp.path(),
            &BookResumeKey::legacy(&cache),
            &archive_pages(&cache, &[]),
            logical_identity.then_some(old),
            SAVED_PAGE,
        );
        let report = run_at_with_shape(temp.path(), old, new, false);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report.converted_cache_pairs.is_empty());
        assert_eq!(report.rows, 0);
        let new_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), new);
        assert_restored(
            temp.path(),
            &BookResumeKey::legacy(&new_cache),
            &archive_pages(&new_cache, &[]),
            logical_identity.then_some(new),
            SAVED_PAGE,
        );
    }
}

#[test]
fn section1886_resume_destination_wins_preserves_page_meter_and_identity() {
    for logical_identity in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let old = Path::new("C:/section1886/old.rar");
        let new = Path::new("C:/section1886/new.rar");
        let old_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), old);
        let new_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), new);
        record_position(
            temp.path(),
            &BookResumeKey::legacy(&old_cache),
            &archive_pages(&old_cache, &[]),
            logical_identity.then_some(old),
            SAVED_PAGE,
        );
        let expected = record_position(
            temp.path(),
            &BookResumeKey::legacy(&new_cache),
            &archive_pages(&new_cache, &[]),
            logical_identity.then_some(new),
            1,
        );
        let report = run_at_with_shape(temp.path(), old, new, false);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(read_position(temp.path(), &BookResumeKey::legacy(&old_cache)).is_none());
        assert_eq!(
            read_position(temp.path(), &BookResumeKey::legacy(&new_cache)),
            Some(expected)
        );
        assert_restored(
            temp.path(),
            &BookResumeKey::legacy(&new_cache),
            &archive_pages(&new_cache, &[]),
            logical_identity.then_some(new),
            1,
        );
    }
}

#[test]
fn section1886_resume_identity_updates_when_drive_stripped_cache_key_does_not_move() {
    let temp = tempfile::tempdir().unwrap();
    let old = Path::new("C:/section1886/book.rar");
    let new = Path::new("D:/section1886/book.rar");
    let cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), old);
    assert_eq!(
        cache,
        crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), new)
    );
    record_position(
        temp.path(),
        &BookResumeKey::legacy(&cache),
        &archive_pages(&cache, &[]),
        Some(old),
        SAVED_PAGE,
    );
    let report = run_at(temp.path(), old, new);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_restored(
        temp.path(),
        &BookResumeKey::legacy(&cache),
        &archive_pages(&cache, &[]),
        Some(new),
        SAVED_PAGE,
    );
    assert_eq!(report.converted_cache_pairs, vec![(cache.clone(), cache)]);
}

#[test]
fn section1886_resume_rolls_back_logical_and_cache_rows_if_identity_rewrite_fails() {
    let temp = tempfile::tempdir().unwrap();
    let old = Path::new("C:/section1886/old.rar");
    let new = Path::new("C:/section1886/new.rar");
    let old_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), old);
    let new_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), new);
    let logical_key = BookResumeKey::legacy(old);
    let old_key = BookResumeKey::legacy(&old_cache);
    let logical_before =
        record_position(temp.path(), &logical_key, &archive_pages(old, &[]), None, 1);
    let cache_before = record_position(
        temp.path(),
        &old_key,
        &archive_pages(&old_cache, &[]),
        Some(old),
        SAVED_PAGE,
    );
    let conn = rusqlite::Connection::open(temp.path().join("book_resume.db")).unwrap();
    conn.execute_batch(&format!(
        "CREATE TRIGGER fail_cache_identity BEFORE UPDATE OF page_identity ON book_resume
        WHEN NEW.path='{}' BEGIN SELECT RAISE(ABORT,'identity failure'); END;",
        BookResumeKey::legacy(&new_cache)
            .storage_key()
            .replace('\'', "''")
    ))
    .unwrap();
    let report = run_at(temp.path(), old, new);
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert!(report.converted_cache_pairs.is_empty());
    assert_eq!(
        read_position(temp.path(), &logical_key),
        Some(logical_before)
    );
    assert_eq!(read_position(temp.path(), &old_key), Some(cache_before));
    assert!(read_position(temp.path(), &BookResumeKey::legacy(new)).is_none());
    assert!(read_position(temp.path(), &BookResumeKey::legacy(&new_cache)).is_none());
}

#[test]
fn section1885_restore_zip_after_external_move_and_copy_keeps_source_identity() {
    for source_remains in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("before/book.zip");
        let new = temp.path().join("after/book.zip");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        // File presence models the external move/copy. Enumeration is supplied
        // to the production materializer independently of archive byte decoding.
        std::fs::write(&old, b"same archive contents").unwrap();
        let old_key = BookResumeKey::legacy(&old);
        let before = record_position(
            temp.path(),
            &old_key,
            &archive_pages(&old, &[]),
            None,
            SAVED_PAGE,
        );
        if source_remains {
            std::fs::copy(&old, &new).unwrap();
        } else {
            std::fs::rename(&old, &new).unwrap();
        }
        assert_eq!(old.exists(), source_remains);
        copy_book(temp.path(), &old, &new);
        assert_restored(
            temp.path(),
            &BookResumeKey::legacy(&new),
            &archive_pages(&new, &[]),
            None,
            SAVED_PAGE,
        );
        assert_eq!(read_position(temp.path(), &old_key), Some(before));
        if source_remains {
            assert_restored(
                temp.path(),
                &old_key,
                &archive_pages(&old, &[]),
                None,
                SAVED_PAGE,
            );
        }
    }
}

#[test]
fn section1885_rename_zip_restores_archive_identity() {
    let temp = tempfile::tempdir().unwrap();
    let old = Path::new("C:/section1885/before.zip");
    let new = Path::new("D:/section1885/after.zip");
    record_position(
        temp.path(),
        &BookResumeKey::legacy(old),
        &archive_pages(old, &[]),
        None,
        SAVED_PAGE,
    );
    rename_book(temp.path(), old, new);
    assert!(read_position(temp.path(), &BookResumeKey::legacy(old)).is_none());
    assert_restored(
        temp.path(),
        &BookResumeKey::legacy(new),
        &archive_pages(new, &[]),
        None,
        SAVED_PAGE,
    );
}

#[test]
fn section1885_rename_zip_parent_preserves_adjacent_prefix_sibling() {
    let temp = tempfile::tempdir().unwrap();
    let old = Path::new("C:/section1885/before_%/book.zip");
    let new = Path::new("E:/section1885/after/book.zip");
    let sibling = Path::new("C:/section1885/before_%other/book.zip");
    record_position(
        temp.path(),
        &BookResumeKey::legacy(old),
        &archive_pages(old, &[]),
        None,
        SAVED_PAGE,
    );
    let sibling_before = record_position(
        temp.path(),
        &BookResumeKey::legacy(sibling),
        &archive_pages(sibling, &[]),
        None,
        1,
    );
    rename_book(temp.path(), old.parent().unwrap(), new.parent().unwrap());
    assert_restored(
        temp.path(),
        &BookResumeKey::legacy(new),
        &archive_pages(new, &[]),
        None,
        SAVED_PAGE,
    );
    assert_eq!(
        read_position(temp.path(), &BookResumeKey::legacy(sibling)),
        Some(sibling_before)
    );
    assert_restored(
        temp.path(),
        &BookResumeKey::legacy(sibling),
        &archive_pages(sibling, &[]),
        None,
        1,
    );
}

#[test]
fn section1885_restore_pdf_restores_document_identity() {
    let temp = tempfile::tempdir().unwrap();
    let old = Path::new("C:/section1885/before/book.pdf");
    let new = Path::new("D:/section1885/after/book.pdf");
    let key = BookResumeKey::legacy(old);
    let before = record_position(temp.path(), &key, &document_pages(old), None, SAVED_PAGE);
    copy_book(temp.path(), old, new);
    assert_restored(
        temp.path(),
        &BookResumeKey::legacy(new),
        &document_pages(new),
        None,
        SAVED_PAGE,
    );
    assert_eq!(read_position(temp.path(), &key), Some(before));
}

#[test]
fn section1885_rename_pdf_restores_document_identity() {
    let temp = tempfile::tempdir().unwrap();
    let old = Path::new("C:/section1885/before.pdf");
    let new = Path::new("D:/section1885/after.pdf");
    record_position(
        temp.path(),
        &BookResumeKey::legacy(old),
        &document_pages(old),
        None,
        SAVED_PAGE,
    );
    rename_book(temp.path(), old, new);
    assert_restored(
        temp.path(),
        &BookResumeKey::legacy(new),
        &document_pages(new),
        None,
        SAVED_PAGE,
    );
}

#[test]
fn section1885_rename_image_folder_restores_image_identity() {
    let temp = tempfile::tempdir().unwrap();
    let old = Path::new("C:/section1885/before");
    let new = Path::new("D:/section1885/after");
    record_position(
        temp.path(),
        &BookResumeKey::legacy(old),
        &image_pages(old),
        None,
        SAVED_PAGE,
    );
    rename_book(temp.path(), old, new);
    assert_restored(
        temp.path(),
        &BookResumeKey::legacy(new),
        &image_pages(new),
        None,
        SAVED_PAGE,
    );
}

#[test]
fn section1885_rename_image_folder_parent_restores_image_identity() {
    let temp = tempfile::tempdir().unwrap();
    let old = Path::new("C:/section1885/before/album");
    let new = Path::new("D:/section1885/after/album");
    record_position(
        temp.path(),
        &BookResumeKey::legacy(old),
        &image_pages(old),
        None,
        SAVED_PAGE,
    );
    rename_book(temp.path(), old.parent().unwrap(), new.parent().unwrap());
    assert_restored(
        temp.path(),
        &BookResumeKey::legacy(new),
        &image_pages(new),
        None,
        SAVED_PAGE,
    );
}

#[test]
fn section1885_destination_wins_keeps_page_and_identity_for_exact_and_prefix() {
    for copy in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let old = Path::new("C:/section1885/before.zip");
        let new = Path::new("D:/section1885/after.zip");
        // Cover exact and ordinary virtual-prefix rows. A preexisting mismatched
        // identity is deliberately retained too: this operation is no repair pass.
        for suffix in ["", "::inner.zip", "::stale.zip"] {
            let source = PathBuf::from(format!("{}{suffix}", old.display()));
            let destination = PathBuf::from(format!("{}{suffix}", new.display()));
            record_position(
                temp.path(),
                &BookResumeKey::legacy(&source),
                &archive_pages(&source, &[]),
                None,
                SAVED_PAGE,
            );
            let pages = archive_pages(
                if suffix == "::stale.zip" {
                    &source
                } else {
                    &destination
                },
                &[],
            );
            record_position(
                temp.path(),
                &BookResumeKey::legacy(&destination),
                &pages,
                None,
                1,
            );
        }
        if copy {
            copy_book(temp.path(), old, new);
        } else {
            rename_book(temp.path(), old, new);
        }
        for suffix in ["", "::inner.zip", "::stale.zip"] {
            let source = PathBuf::from(format!("{}{suffix}", old.display()));
            let destination = PathBuf::from(format!("{}{suffix}", new.display()));
            let pages = archive_pages(
                if suffix == "::stale.zip" {
                    &source
                } else {
                    &destination
                },
                &[],
            );
            assert_restored(
                temp.path(),
                &BookResumeKey::legacy(&destination),
                &pages,
                None,
                1,
            );
            assert_eq!(
                read_position(temp.path(), &BookResumeKey::legacy(&source)).is_some(),
                copy
            );
        }
    }
}

#[test]
fn section1885_virtual_prefix_copy_and_move_restore_only_selected_rows() {
    for copy in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let old = Path::new("C:/section1885/before.zip");
        let new = Path::new("D:/section1885/after.zip");
        let old_inner = PathBuf::from(format!("{}::inner.zip", old.display()));
        let new_inner = PathBuf::from(format!("{}::inner.zip", new.display()));
        let sibling = Path::new("C:/section1885/before.zip-other::inner.zip");
        let before = record_position(
            temp.path(),
            &BookResumeKey::legacy(&old_inner),
            &archive_pages(&old_inner, &[]),
            None,
            SAVED_PAGE,
        );
        record_position(
            temp.path(),
            &BookResumeKey::legacy(sibling),
            &archive_pages(sibling, &[]),
            None,
            1,
        );
        if copy {
            copy_book(temp.path(), old, new);
        } else {
            rename_book(temp.path(), old, new);
        }
        assert_restored(
            temp.path(),
            &BookResumeKey::legacy(&new_inner),
            &archive_pages(&new_inner, &[]),
            None,
            SAVED_PAGE,
        );
        assert_restored(
            temp.path(),
            &BookResumeKey::legacy(sibling),
            &archive_pages(sibling, &[]),
            None,
            1,
        );
        assert_eq!(
            read_position(temp.path(), &BookResumeKey::legacy(&old_inner)),
            copy.then_some(before)
        );
    }
}

#[test]
fn section1885_restore_same_source_to_two_targets_keeps_each_identity() {
    let temp = tempfile::tempdir().unwrap();
    let old = Path::new("C:/section1885/before/a.zip");
    let targets = [
        Path::new("D:/section1885/dir/a.zip"),
        Path::new("D:/section1885/dir/a - copy.zip"),
    ];
    let prefix = vec!["Chapter".into(), "Inner.zip".into()];
    let old_inner = PathBuf::from(format!("{}::inner.zip", old.display()));
    let sources = [
        (BookResumeKey::legacy(old), archive_pages(old, &[])),
        (
            BookResumeKey::legacy(&old_inner),
            archive_pages(&old_inner, &[]),
        ),
        (
            BookResumeKey::archive(old, &prefix).unwrap(),
            archive_pages(old, &prefix),
        ),
    ];
    let before = sources
        .iter()
        .map(|(key, pages)| record_position(temp.path(), key, pages, None, SAVED_PAGE))
        .collect::<Vec<_>>();
    let mappings = targets
        .iter()
        .flat_map(|&target| {
            [
                StoreCopyPathMapping::exact(old, target),
                StoreCopyPathMapping::virtual_prefix(old, target),
            ]
        })
        .collect::<Vec<_>>();
    let report = copy_stores_at(temp.path(), &mappings);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    for target in targets {
        let inner = PathBuf::from(format!("{}::inner.zip", target.display()));
        for (key, pages) in [
            (BookResumeKey::legacy(target), archive_pages(target, &[])),
            (BookResumeKey::legacy(&inner), archive_pages(&inner, &[])),
            (
                BookResumeKey::archive(target, &prefix).unwrap(),
                archive_pages(target, &prefix),
            ),
        ] {
            assert_restored(temp.path(), &key, &pages, None, SAVED_PAGE);
        }
    }
    for ((key, _), position) in sources.iter().zip(before) {
        assert_eq!(read_position(temp.path(), key), Some(position));
    }
}

#[test]
fn section1885_restore_same_converted_source_to_two_targets_keeps_each_identity() {
    for extension in ["rar", "7z", "lzh", "zip"] {
        let temp = tempfile::tempdir().unwrap();
        let old = PathBuf::from(format!("C:/section1885/before/a.{extension}"));
        let targets = [
            PathBuf::from(format!("D:/section1885/dir/a.{extension}")),
            PathBuf::from(format!("D:/section1885/dir/a - copy.{extension}")),
        ];
        let old_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), &old);
        let old_cache_inner = PathBuf::from(format!("{}::inner.zip", old_cache.display()));
        let prefix = vec!["Chapter".into(), "Inner.zip".into()];
        // Ordinary root and virtual-prefix rows have cache keys with logical
        // source identities; namespaced inner-book rows use the logical key too.
        let sources = [
            (
                BookResumeKey::legacy(&old_cache),
                archive_pages(&old_cache, &[]),
            ),
            (
                BookResumeKey::legacy(&old_cache_inner),
                archive_pages(&old_cache_inner, &[]),
            ),
            (
                BookResumeKey::archive(&old, &prefix).unwrap(),
                archive_pages(&old_cache, &prefix),
            ),
        ];
        let before = sources
            .iter()
            .map(|(key, pages)| record_position(temp.path(), key, pages, Some(&old), SAVED_PAGE))
            .collect::<Vec<_>>();
        let mappings = targets
            .iter()
            .flat_map(|target| {
                let cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), target);
                [
                    StoreCopyPathMapping::exact(&old, target),
                    StoreCopyPathMapping::virtual_prefix(&old, target),
                    StoreCopyPathMapping::exact(&old_cache, &cache),
                    StoreCopyPathMapping::virtual_prefix(&old_cache, &cache),
                ]
            })
            .collect::<Vec<_>>();
        let report = copy_stores_at(temp.path(), &mappings);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        for target in targets {
            let cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), &target);
            let cache_inner = PathBuf::from(format!("{}::inner.zip", cache.display()));
            for (key, pages) in [
                (BookResumeKey::legacy(&cache), archive_pages(&cache, &[])),
                (
                    BookResumeKey::legacy(&cache_inner),
                    archive_pages(&cache_inner, &[]),
                ),
                (
                    BookResumeKey::archive(&target, &prefix).unwrap(),
                    archive_pages(&cache, &prefix),
                ),
            ] {
                assert_restored(temp.path(), &key, &pages, Some(&target), SAVED_PAGE);
            }
        }
        for ((key, _), position) in sources.iter().zip(before) {
            assert_eq!(read_position(temp.path(), key), Some(position));
        }
    }
}

#[test]
fn section1885_converted_archive_batch_rewrites_logical_and_cache_identity_paths() {
    for extension in ["rar", "7z", "lzh", "zip"] {
        for identity_is_logical in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let old = PathBuf::from(format!("C:/section1885/before/book.{extension}"));
            let new = PathBuf::from(format!("D:/section1885/after/book.{extension}"));
            let old_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), &old);
            let new_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), &new);
            let old_logical = identity_is_logical.then_some(old.as_path());
            let new_logical = identity_is_logical.then_some(new.as_path());
            // Root resume is keyed by the backing ZIP, but new app records use
            // the logical source identity. Also retain the no-override cache case.
            let before = record_position(
                temp.path(),
                &BookResumeKey::legacy(&old_cache),
                &archive_pages(&old_cache, &[]),
                old_logical,
                SAVED_PAGE,
            );
            let report = copy_stores_at(
                temp.path(),
                &[
                    StoreCopyPathMapping::exact(&old, &new),
                    StoreCopyPathMapping::virtual_prefix(&old, &new),
                    StoreCopyPathMapping::exact(&old_cache, &new_cache),
                    StoreCopyPathMapping::virtual_prefix(&old_cache, &new_cache),
                ],
            );
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            assert_restored(
                temp.path(),
                &BookResumeKey::legacy(&new_cache),
                &archive_pages(&new_cache, &[]),
                new_logical,
                SAVED_PAGE,
            );
            assert_eq!(
                read_position(temp.path(), &BookResumeKey::legacy(&old_cache)),
                Some(before)
            );
        }
    }
}

#[test]
fn section1885_inner_archive_rows_still_resolve_after_copy_and_rename() {
    for copy in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let old = Path::new("C:/section1885/before.zip");
        let new = Path::new("D:/section1885/after.zip");
        let prefix = vec!["Chapter".into(), "Inner.zip".into()];
        let old_key = BookResumeKey::archive(old, &prefix).unwrap();
        let new_key = BookResumeKey::archive(new, &prefix).unwrap();
        let before = record_position(
            temp.path(),
            &old_key,
            &archive_pages(old, &prefix),
            Some(old),
            SAVED_PAGE,
        );
        if copy {
            copy_book(temp.path(), old, new);
        } else {
            rename_book(temp.path(), old, new);
        }
        assert_restored(
            temp.path(),
            &new_key,
            &archive_pages(new, &prefix),
            Some(new),
            SAVED_PAGE,
        );
        assert_eq!(read_position(temp.path(), &old_key), copy.then_some(before));
    }
}

#[test]
fn section1885_book_folder_rename_restores_nested_zip_and_pdf_identity() {
    let temp = tempfile::tempdir().unwrap();
    let old = Path::new("C:/section1885/books/one");
    let new = Path::new("D:/section1885/books/two");
    let sibling = Path::new("C:/section1885/books/one-other");
    let pages_for_folder = |folder: &Path| {
        let zip = folder.join("chapter.zip");
        let pdf = folder.join("chapter.pdf");
        let prefix = vec!["Chapter".into(), "Inner.zip".into()];
        [
            (BookResumeKey::legacy(folder), image_pages(folder)),
            (BookResumeKey::legacy(&zip), archive_pages(&zip, &[])),
            (BookResumeKey::legacy(&pdf), document_pages(&pdf)),
            (
                BookResumeKey::archive(&zip, &prefix).unwrap(),
                archive_pages(&zip, &prefix),
            ),
        ]
    };
    let originals = pages_for_folder(old);
    for (key, pages) in &originals {
        record_position(temp.path(), key, pages, None, SAVED_PAGE);
    }
    let sibling_rows = pages_for_folder(sibling);
    let sibling_before = sibling_rows
        .iter()
        .map(|(key, pages)| record_position(temp.path(), key, pages, None, 1))
        .collect::<Vec<_>>();
    let coverage = [old, new]
        .map(|root| crate::pdf_loader::EpubPinCoverage::PathAndDescendants(root.to_path_buf()));
    let guard = acquire_store_maintenance(temp.path(), &coverage).unwrap();
    let report = migrate_book_paths_at(
        temp.path(),
        &[(old.to_path_buf(), new.to_path_buf())],
        &[],
        &guard,
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    for (key, _) in originals {
        assert!(read_position(temp.path(), &key).is_none());
    }
    for (key, pages) in pages_for_folder(new) {
        assert_restored(temp.path(), &key, &pages, None, SAVED_PAGE);
    }
    for ((key, pages), position) in sibling_rows.iter().zip(sibling_before) {
        assert_eq!(read_position(temp.path(), key), Some(position));
        assert_restored(temp.path(), key, pages, None, 1);
    }
}

#[test]
fn section1885_book_folder_chain_rewrites_inner_identity_once() {
    let temp = tempfile::tempdir().unwrap();
    let roots =
        ["one", "two", "three"].map(|name| PathBuf::from(format!("C:/section1885/books/{name}")));
    let prefix = vec!["Chapter".into(), "Inner.zip".into()];
    for (index, root) in roots[..2].iter().enumerate() {
        let zip = root.join("chapter.zip");
        record_position(
            temp.path(),
            &BookResumeKey::archive(&zip, &prefix).unwrap(),
            &archive_pages(&zip, &prefix),
            None,
            index + 1,
        );
    }
    let coverage = roots
        .iter()
        .cloned()
        .map(crate::pdf_loader::EpubPinCoverage::PathAndDescendants)
        .collect::<Vec<_>>();
    let guard = acquire_store_maintenance(temp.path(), &coverage).unwrap();
    let report = migrate_book_paths_at(
        temp.path(),
        &[
            (roots[0].clone(), roots[1].clone()),
            (roots[1].clone(), roots[2].clone()),
        ],
        &[],
        &guard,
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(
        read_position(
            temp.path(),
            &BookResumeKey::archive(&roots[0].join("chapter.zip"), &prefix).unwrap()
        )
        .is_none()
    );
    for (index, root) in roots[1..].iter().enumerate() {
        let zip = root.join("chapter.zip");
        assert_restored(
            temp.path(),
            &BookResumeKey::archive(&zip, &prefix).unwrap(),
            &archive_pages(&zip, &prefix),
            None,
            index + 1,
        );
    }
}

#[test]
fn section1885_identity_write_failure_rolls_back_ordinary_key_copy_and_move() {
    for copy in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let old = Path::new("C:/section1885/before.zip");
        let new = Path::new("D:/section1885/after.zip");
        let key = BookResumeKey::legacy(old);
        let before = record_position(
            temp.path(),
            &key,
            &archive_pages(old, &[]),
            None,
            SAVED_PAGE,
        );
        let conn = rusqlite::Connection::open(temp.path().join("book_resume.db")).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_identity BEFORE UPDATE OF page_identity ON book_resume BEGIN SELECT RAISE(ABORT, 'injected identity failure'); END;").unwrap();
        let errors = if copy {
            copy_stores_at(temp.path(), &[StoreCopyPathMapping::exact(old, new)]).errors
        } else {
            run_at(temp.path(), old, new).errors
        };
        assert!(!errors.is_empty());
        assert!(read_position(temp.path(), &BookResumeKey::legacy(new)).is_none());
        assert_eq!(read_position(temp.path(), &key), Some(before));
        assert_restored(
            temp.path(),
            &key,
            &archive_pages(old, &[]),
            None,
            SAVED_PAGE,
        );
    }
}
