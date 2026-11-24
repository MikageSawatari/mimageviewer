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

const SAVED_PAGE: usize = 2;
const PAGE_COUNT: usize = 4;

// Use the same pure worker materializers as book open; no image decoding or GUI.
fn archive_pages(backing: &Path, prefix: &[String]) -> Vec<GridItem> {
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

fn record_position(
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

fn read_position(data: &Path, key: &BookResumeKey) -> Option<ReadingPosition> {
    BookResumeDb::open_at(&data.join("book_resume.db"))
        .unwrap()
        .get_position_key(key)
}

fn assert_restored(
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
