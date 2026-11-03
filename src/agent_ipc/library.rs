//! Read projections only: borrow the shared readers/actor and preserve source ordering.
use super::*;

impl Handlers {
    pub(super) fn library(
        &self,
        kind: LibraryKind,
        stars: Option<u8>,
        limit: Option<u32>,
        cursor: Option<String>,
        ctx: &ReadContext,
    ) -> Result<LibraryPage, AgentError> {
        ctx.checkpoint()?;
        if stars.is_some_and(|value| value > 5) {
            return Err(error(ErrorCode::BadRequest, "★は0～5で指定してください"));
        }
        let threshold = stars.unwrap_or(1).clamp(1, 5);
        let args = serde_json::json!([
            kind,
            if kind == LibraryKind::Rated {
                Some(threshold)
            } else {
                None
            },
            "source_order"
        ]);
        let offset = decode_cursor(cursor.as_deref(), "list_library", &args, ctx.granted.1)?;
        let limit = limit_value(limit)?;
        let mut rows = Vec::new();
        match kind {
            LibraryKind::Favorites => {
                let favorites = self.readers.favorites()?;
                ctx.checkpoint()?;
                return favorite_page(&favorites, offset, limit, args, ctx, |favorite| {
                    let mut row = library_row(favorite.path.to_string_lossy().into_owned());
                    if favorite.path.is_dir() {
                        row.kind = ItemKind::Folder;
                    }
                    row.name = favorite.name.clone();
                    row.id = Some(favorite.id.to_string());
                    Ok(row)
                });
            }
            LibraryKind::ReadingHistory => {
                use crate::reading_history_db::{ReadingHistoryDb, ReadingHistoryKind};
                if present(&ReadingHistoryDb::db_path())? {
                    let db = ReadingHistoryDb::open_readonly().map_err(|_| internal())?;
                    let history = db
                        .list_recent(crate::reading_history_db::READING_HISTORY_LIMIT_MAX)
                        .map_err(|_| internal())?;
                    ctx.checkpoint()?;
                    for entry in history {
                        ctx.checkpoint()?;
                        let mut row = library_row(entry.path.to_string_lossy().into_owned());
                        row.name = entry.title;
                        row.kind = match entry.kind {
                            ReadingHistoryKind::Folder => ItemKind::Folder,
                            ReadingHistoryKind::Zip | ReadingHistoryKind::Archive => {
                                ItemKind::Archive
                            }
                            ReadingHistoryKind::Pdf => ItemKind::Pdf,
                            ReadingHistoryKind::Video => ItemKind::Video,
                            ReadingHistoryKind::Audio => ItemKind::Audio,
                        };
                        row.last_read_at = Some(entry.last_read_at_ms);
                        row.page = entry.last_page.and_then(|page| u32::try_from(page).ok());
                        rows.push(row);
                    }
                }
            }
            LibraryKind::Bookshelf => {
                let settings = crate::settings_db::with_db_result(|db| db.load_into_settings())
                    .map_err(|_| internal())?;
                ctx.checkpoint()?;
                let books =
                    crate::books::list_book_paths_while(&settings.books_root_path(), || {
                        ctx.checkpoint().is_ok()
                    });
                ctx.checkpoint()?;
                for (name, path) in books.map_err(|_| internal())? {
                    ctx.checkpoint()?;
                    let mut row = library_row(path.to_string_lossy().into_owned());
                    row.name = name;
                    row.kind = ItemKind::Folder;
                    rows.push(row);
                }
            }
            LibraryKind::Bookmarks => {
                if present(&crate::book_bookmarks::db_path())? {
                    let bookmarks = crate::book_bookmarks::load_all_from_disk_readonly()
                        .map_err(|_| internal())?;
                    ctx.checkpoint()?;
                    for bookmark in bookmarks {
                        ctx.checkpoint()?;
                        rows.push(bookmark_row(bookmark));
                    }
                }
            }
            LibraryKind::Rated => {
                let path = crate::rating_db::RatingDb::db_path();
                if present(&path)? {
                    let db =
                        crate::rating_db::RatingDb::open_readonly(path).map_err(|_| internal())?;
                    let rated = db
                        .list_at_least_stars(threshold, &ctx.cancel)
                        .map_err(|_| internal())?;
                    ctx.checkpoint()?;
                    for rating in rated {
                        ctx.checkpoint()?;
                        rows.push(rated_row(rating));
                    }
                }
            }
        }
        // The remaining sources complete their ordering before pagination.
        ctx.checkpoint()?;
        let more = rows.len() > offset + limit;
        let (items, cursor, truncated) = bounded_rows(
            rows.into_iter().skip(offset).take(limit).collect(),
            offset,
            more,
            "list_library",
            args,
            "items",
            ctx.granted.1,
        )?;
        ctx.checkpoint()?;
        Ok(LibraryPage {
            items,
            cursor,
            truncated,
        })
    }

    pub(super) fn get_collection(
        &self,
        id: Option<String>,
        name: Option<String>,
        limit: Option<u32>,
        cursor: Option<String>,
        ctx: &ReadContext,
    ) -> Result<ItemPage, AgentError> {
        ctx.checkpoint()?;
        let bad_lookup = || {
            error(
                ErrorCode::BadRequest,
                "コレクションの id または名前を 1 つ指定してください",
            )
        };
        let id = id.map(|value| value.trim().to_owned());
        let name = name.map(|value| value.trim().to_owned());
        if id.is_some() == name.is_some() {
            return Err(bad_lookup());
        }
        let requested_id = match id.as_deref() {
            Some(id) => Some(crate::collection_store::CollectionId::from_uuid(
                uuid::Uuid::parse_str(id).map_err(|_| bad_lookup())?,
            )),
            None => {
                if name.as_deref().is_none_or(str::is_empty) {
                    return Err(bad_lookup());
                }
                None
            }
        };
        let id = requested_id.map(|id| id.to_string());
        let args = serde_json::json!([id, name, "collection_order"]);
        let offset = decode_cursor(cursor.as_deref(), "get_collection", &args, ctx.granted.1)?;
        let limit = limit_value(limit)?;
        let lease = self
            .producer
            .as_ref()
            .and_then(|producer| producer.begin_request())
            .ok_or_else(|| error(ErrorCode::NotReady, "コレクションを準備しています…"))?;
        let catalog = lease
            .client()
            .list_catalog()
            .map_err(collection_error)?
            .recv()
            .map_err(|_| internal())?
            .map_err(collection_error)?;
        ctx.checkpoint()?;
        let mut matches = catalog.definitions.iter().filter(|definition| {
            requested_id.map_or_else(
                || Some(definition.name.as_str()) == name.as_deref(),
                |id| definition.id == id,
            )
        });
        let definition = matches
            .next()
            .ok_or_else(|| error(ErrorCode::BadRequest, "コレクションが見つかりません"))?;
        if matches.next().is_some() {
            return Err(error(
                ErrorCode::BadRequest,
                "同じ名前のコレクションが複数あります。id で指定してください",
            ));
        }
        let snapshot = lease
            .client()
            .load_collection(definition.id)
            .map_err(collection_error)?
            .recv()
            .map_err(|_| internal())?
            .map_err(collection_error)?;
        ctx.checkpoint()?;
        let settings = crate::settings_db::with_db_result(|db| db.load_into_settings())
            .map_err(|_| internal())?;
        ctx.checkpoint()?;
        let prepared = crate::collection_store::prepare_collection_snapshot_while(
            &snapshot,
            &settings.grid_display_order,
            || ctx.checkpoint().is_ok(),
            |_, _| {},
        );
        ctx.checkpoint()?;
        let prepared = prepared.map_err(|_| internal())?;
        let mut items = Vec::with_capacity(prepared.entries.len());
        for entry in prepared.entries.iter() {
            ctx.checkpoint()?;
            items.push(Item {
                path: entry.source_path.to_string_lossy().into_owned(),
                subresource: None,
                name: entry.item.name().into_owned(),
                kind: collection_item_kind(&entry.item),
                tags: Some(Vec::new()),
                container_tags: None,
                rating: 0,
            });
        }
        let page = page(
            items,
            offset,
            limit,
            false,
            "get_collection",
            args,
            ctx.granted.1,
        )?;
        ctx.checkpoint()?;
        Ok(page)
    }
}

fn favorite_page(
    favorites: &[crate::settings::FavoriteEntry],
    offset: usize,
    limit: usize,
    args: serde_json::Value,
    ctx: &ReadContext,
    mut read_row: impl FnMut(&crate::settings::FavoriteEntry) -> Result<LibraryItem, AgentError>,
) -> Result<LibraryPage, AgentError> {
    let mut rows = Vec::new();
    for favorite in favorites.iter().skip(offset).take(limit) {
        ctx.checkpoint()?;
        let row = read_row(favorite)?;
        ctx.checkpoint()?;
        rows.push(row);
        let (items, cursor, truncated) = bounded_rows(
            rows.clone(),
            offset,
            true,
            "list_library",
            args.clone(),
            "items",
            ctx.granted.1,
        )?;
        let count = items.len();
        ctx.publish(Payload::Library(LibraryPage {
            items,
            cursor,
            truncated,
        }));
        if count < rows.len() {
            rows.truncate(count);
            break;
        }
    }
    ctx.checkpoint()?;
    let more = offset + rows.len() < favorites.len();
    let (items, cursor, truncated) = bounded_rows(
        rows,
        offset,
        more,
        "list_library",
        args,
        "items",
        ctx.granted.1,
    )?;
    Ok(LibraryPage {
        items,
        cursor,
        truncated,
    })
}

fn present(path: &Path) -> Result<bool, AgentError> {
    path.try_exists().map_err(|_| internal())
}

fn library_row(path: String) -> LibraryItem {
    // A store projection must not probe every bookmark/rating target or open a
    // container merely to infer its kind. Known source kinds override this hint.
    let name = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_owned();
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    let kind = super::item_kind_from_extension(&extension);
    LibraryItem {
        path,
        subresource: None,
        name,
        kind,
        id: None,
        page: None,
        added_at: None,
        last_read_at: None,
        rated_at: None,
        rating: None,
    }
}

fn collection_item_kind(item: &crate::grid_item::GridItem) -> ItemKind {
    use crate::collection_store::CollectionResolvedKind as K;
    use crate::grid_item::GridItem as G;
    match item {
        G::Image(_) => ItemKind::Image,
        G::Video(_) => ItemKind::Video,
        G::Audio(_) => ItemKind::Audio,
        G::Folder(_) => ItemKind::Folder,
        G::ZipFile(_) | G::ConvertibleArchive { .. } => ItemKind::Archive,
        G::PdfFile(_) => ItemKind::Pdf,
        G::CollectionPlaceholder {
            last_known_kind, ..
        } => match last_known_kind {
            K::Image => ItemKind::Image,
            K::Video => ItemKind::Video,
            K::Audio => ItemKind::Audio,
            K::Folder => ItemKind::Folder,
            K::Zip | K::ConvertibleArchive => ItemKind::Archive,
            K::Pdf => ItemKind::Pdf,
            K::Unresolved => ItemKind::Other,
        },
        _ => ItemKind::Other,
    }
}

fn bookmark_row(bookmark: crate::book_bookmarks::BookBookmark) -> LibraryItem {
    use crate::book_bookmarks::PageIdentity;
    let display_name = bookmark.page_identity.display_name();
    let mut row = match bookmark.page_identity {
        PageIdentity::RelativePath(relative) => {
            let mut row = library_row(
                bookmark
                    .container_path
                    .join(relative)
                    .to_string_lossy()
                    .into_owned(),
            );
            row.kind = ItemKind::Image;
            row
        }
        PageIdentity::ArchiveEntry(member) => {
            let mut row = library_row(bookmark.container_path.to_string_lossy().into_owned());
            row.subresource = Some(member);
            row.kind = ItemKind::Image;
            row
        }
        PageIdentity::PdfPage(page) => {
            let mut row = library_row(bookmark.container_path.to_string_lossy().into_owned());
            row.subresource = Some(format!("page_{page}"));
            row.kind = ItemKind::Pdf;
            row
        }
    };
    row.name = bookmark.title.unwrap_or(display_name);
    row.id = Some(bookmark.id.to_string());
    row.page = u32::try_from(bookmark.page_index_hint).ok();
    row.added_at = Some(bookmark.created_at_ms);
    row
}

fn rated_row(rating: crate::rating_db::RatingRow) -> LibraryItem {
    use crate::rating_db::RatingItemKind as K;
    let (key_path, key_member) = rating
        .key
        .split_once("::")
        .map_or((rating.key.as_str(), None), |(path, member)| {
            (path, Some(member))
        });
    let mut row = library_row(
        rating
            .source_path
            .clone()
            .unwrap_or_else(|| key_path.to_owned()),
    );
    row.kind = match rating.kind {
        Some(K::Image | K::ZipImage) => ItemKind::Image,
        Some(K::Video) => ItemKind::Video,
        Some(K::Audio) => ItemKind::Audio,
        Some(K::Folder) => ItemKind::Folder,
        Some(K::ZipFile | K::ConvertibleArchive) => ItemKind::Archive,
        Some(K::PdfFile | K::PdfPage) => ItemKind::Pdf,
        Some(K::ZipDir) => ItemKind::Folder,
        None => row.kind,
    };
    let page = rating
        .page_num
        .or_else(|| key_member.and_then(|member| member.strip_prefix("page_")?.parse().ok()));
    row.subresource =
        if rating.kind == Some(K::PdfPage) || (rating.kind.is_none() && page.is_some()) {
            page.map(|page| format!("page_{page}"))
        } else if rating.kind == Some(K::ZipDir) {
            rating.dir_prefix.clone()
        } else {
            rating
                .entry_name
                .clone()
                .or_else(|| key_member.map(str::to_owned))
        };
    if rating.kind.is_none() && row.subresource.is_some() {
        row.kind = if page.is_some() {
            ItemKind::Pdf
        } else {
            ItemKind::Image
        };
    }
    if let Some(subresource) = &row.subresource {
        row.name = if let Some(page) = page {
            format!("Page {}", u64::from(page) + 1)
        } else {
            subresource
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(subresource)
                .to_owned()
        };
    }
    row.page = page;
    row.rating = Some(rating.stars);
    row.rated_at = rating.rated_at_ms;
    row
}

#[cfg(test)]
mod tests {
    use super::super::tests::Fixture;
    use super::*;

    fn value<T>(row: &PageRow<T>) -> &T {
        match row {
            PageRow::Value(value) => value,
            PageRow::TooLong(_) => panic!("short fixture row"),
        }
    }

    fn list(
        f: &Fixture,
        kind: LibraryKind,
        stars: Option<u8>,
        limit: Option<u32>,
        cursor: Option<String>,
    ) -> LibraryPage {
        let connection = f.connection();
        let Payload::Library(page) = f
            .call(
                &connection,
                1,
                Command::ListLibrary {
                    kind,
                    stars,
                    limit,
                    cursor,
                },
            )
            .result
            .unwrap()
        else {
            panic!("library page")
        };
        page
    }

    #[test]
    fn library_favorites_history_and_bookshelf_use_live_order_and_page_cursors() {
        use crate::reading_history_db::{
            ReadingHistoryDb, ReadingHistoryEntry, ReadingHistoryKind,
        };
        let f = Fixture::new();
        let root = crate::data_dir::get();
        let shelf = root.join("shelf");
        std::fs::create_dir_all(shelf.join("Zulu")).unwrap();
        std::fs::create_dir_all(shelf.join("Alpha").join("nested")).unwrap();
        std::fs::write(shelf.join("Alpha").join("invalid.png"), b"not an image").unwrap();
        let favorite = crate::settings::FavoriteEntry::new("live favorite".into(), shelf.clone());
        crate::settings_db::with_db_result(|db| {
            let mut settings = db.load_into_settings()?;
            settings.favorites = vec![favorite.clone()];
            settings.book_root = Some(shelf.clone());
            db.save_full(&settings)
        })
        .unwrap();
        let favorites = list(&f, LibraryKind::Favorites, None, None, None);
        assert_eq!(favorites.items.len(), 1);
        assert_eq!(value(&favorites.items[0]).name, "live favorite");
        assert_eq!(value(&favorites.items[0]).id, Some(favorite.id.to_string()));
        assert_eq!(value(&favorites.items[0]).kind, ItemKind::Folder);
        let shelf_page = list(&f, LibraryKind::Bookshelf, None, Some(1), None);
        assert_eq!(value(&shelf_page.items[0]).name, "Alpha");
        let shelf_next = list(&f, LibraryKind::Bookshelf, None, Some(1), shelf_page.cursor);
        assert_eq!(value(&shelf_next.items[0]).name, "Zulu");
        assert!(shelf_next.cursor.is_none());
        assert!(
            value(&shelf_next.items[0]).page.is_none(),
            "bookshelf never counts pages"
        );

        let history = ReadingHistoryDb::open().unwrap();
        for (name, timestamp) in [("older", 100), ("newer", 200)] {
            let mut entry = ReadingHistoryEntry::new(
                root.join(name),
                ReadingHistoryKind::Folder,
                None,
                name.into(),
                Some(3),
                Some(8),
            );
            entry.last_read_at_ms = timestamp;
            entry.file_size = Some(0);
            entry.mtime_ms = Some(0);
            history.upsert(entry, 1000).unwrap();
        }
        let first = list(&f, LibraryKind::ReadingHistory, None, Some(1), None);
        assert_eq!(value(&first.items[0]).name, "newer");
        assert_eq!(value(&first.items[0]).last_read_at, Some(200));
        assert_eq!(value(&first.items[0]).page, Some(3));
        let second = list(&f, LibraryKind::ReadingHistory, None, Some(1), first.cursor);
        assert_eq!(value(&second.items[0]).name, "older");
        assert!(second.cursor.is_none());
    }

    #[test]
    fn favorite_deadline_returns_completed_prefix_before_blocked_next_row_finishes() {
        let mut f = Fixture::new();
        let root = crate::data_dir::get();
        let favorites = vec![
            crate::settings::FavoriteEntry::new("first".into(), root.join("first")),
            crate::settings::FavoriteEntry::new("second".into(), root.join("second")),
        ];
        let base = Instant::now();
        let elapsed = Arc::new(AtomicUsize::new(0));
        let clock = elapsed.clone();
        let (started_send, started) = crossbeam_channel::bounded(1);
        let (release, gate) = crossbeam_channel::bounded(1);
        let args = serde_json::json!([LibraryKind::Favorites, null, "source_order"]);
        let worker_args = args.clone();
        Arc::get_mut(&mut f.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
            now: Arc::new(move || base + Duration::from_secs(clock.load(Ordering::Acquire) as u64)),
            execute: Box::new(move |command, ctx| match command {
                Command::Hello { version, .. } => Ok(Payload::Hello { version }),
                Command::ListLibrary {
                    kind: LibraryKind::Favorites,
                    ..
                } => {
                    let mut position = 0;
                    favorite_page(&favorites, 0, 50, worker_args.clone(), ctx, |favorite| {
                        if position == 1 {
                            started_send.send(()).unwrap();
                            gate.recv_timeout(Duration::from_secs(10)).unwrap();
                        }
                        position += 1;
                        Ok(library_row(favorite.path.to_string_lossy().into_owned()))
                    })
                    .map(Payload::Library)
                }
                _ => panic!("unexpected command"),
            }),
        }));
        let connection = f.connection();
        let (send, replies) = crossbeam_channel::unbounded();
        let worker = connection
            .request(
                Request {
                    id: 1,
                    command: Command::ListLibrary {
                        kind: LibraryKind::Favorites,
                        stars: None,
                        limit: None,
                        cursor: None,
                    },
                },
                send,
            )
            .unwrap();
        started.recv_timeout(Duration::from_secs(10)).unwrap();
        elapsed.store(20, Ordering::Release);
        let response = replies
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .wire();
        let Payload::Library(page) = response.result.unwrap() else {
            panic!("expected prefix");
        };
        assert_eq!(page.items.len(), 1);
        assert!(page.truncated);
        assert_eq!(
            decode_cursor(
                page.cursor.as_deref(),
                "list_library",
                &args,
                permission().unwrap().1,
            )
            .unwrap(),
            1
        );
        assert_eq!(f.handlers.active.load(Ordering::Acquire), 1);
        assert!(!worker.is_finished());
        release.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(f.handlers.active.load(Ordering::Acquire), 0);
    }

    #[test]
    fn library_ratings_use_threshold_order_and_original_virtual_identities() {
        use crate::rating_db::{RatingDb, RatingItemKind, RatingMeta};
        let f = Fixture::new();
        let db = RatingDb::open().unwrap();
        let mut zip = RatingMeta::new(RatingItemKind::ZipImage);
        zip.source_path = Some("C:/Missing/Case.zip".into());
        zip.entry_name = Some("Pages/Cat.png".into());
        let mut pdf = RatingMeta::new(RatingItemKind::PdfPage);
        pdf.source_path = Some("C:/Missing/Book.pdf".into());
        pdf.page_num = Some(2);
        db.set_user_rating("zipped-key", 4, Some(&zip)).unwrap();
        db.set_user_rating("C:/older.png", 4, None).unwrap();
        db.set_imported_rating("pdf-key", 5, Some(&pdf)).unwrap();
        db.set_user_rating("C:/small.png", 1, None).unwrap();
        let timestamps = rusqlite::Connection::open(RatingDb::db_path()).unwrap();
        timestamps.execute("UPDATE ratings SET rated_at_ms = CASE WHEN path = 'zipped-key' THEN 300 ELSE 100 END WHERE stars = 4", []).unwrap();
        let page = list(&f, LibraryKind::Rated, Some(4), None, None);
        assert_eq!(
            page.items.len(),
            3,
            "threshold includes five and four stars"
        );
        let high = value(&page.items[0]);
        assert_eq!(high.rating, Some(5));
        assert_eq!(high.path, "C:/Missing/Book.pdf");
        assert_eq!(high.subresource.as_deref(), Some("page_2"));
        assert_eq!(high.page, Some(2));
        let zip = value(&page.items[1]);
        assert_eq!(zip.path, "C:/Missing/Case.zip");
        assert_eq!(zip.subresource.as_deref(), Some("Pages/Cat.png"));
        assert_eq!(zip.kind, ItemKind::Image);
        assert_eq!(zip.rated_at, Some(300));
        assert_eq!(value(&page.items[2]).path, "C:/older.png");
        let connection = f.connection();
        assert_eq!(
            f.call(
                &connection,
                2,
                Command::ListLibrary {
                    kind: LibraryKind::Rated,
                    stars: Some(255),
                    limit: None,
                    cursor: None,
                }
            )
            .result
            .unwrap_err()
            .code,
            ErrorCode::BadRequest
        );
    }

    #[test]
    fn library_bookmarks_project_missing_sources_without_decode_or_stat_filter() {
        let f = Fixture::new();
        let path = crate::book_bookmarks::db_path();
        crate::book_bookmarks::ensure_schema_at(&path).unwrap();
        let db = rusqlite::Connection::open(path).unwrap();
        db.execute(
            "INSERT INTO book_bookmarks(container_key,container_path,container_kind,page_kind,page_value,page_key,page_index_hint,created_at_ms,title)
             VALUES ('missing','C:/Missing/Book.zip','zip','archive_entry','Pages/Cat.png','pages/cat.png',3,400,'saved title')", []
        ).unwrap();
        let page = list(&f, LibraryKind::Bookmarks, None, None, None);
        assert_eq!(page.items.len(), 1);
        let row = value(&page.items[0]);
        assert_eq!(row.path, "C:/Missing/Book.zip");
        assert_eq!(row.subresource.as_deref(), Some("Pages/Cat.png"));
        assert_eq!(row.name, "saved title");
        assert_eq!(row.page, Some(3));
        assert_eq!(row.added_at, Some(400));
    }

    #[test]
    fn collection_lookup_uses_prepared_order_and_keeps_missing_references() {
        use crate::collection_store::{
            CollectionOrderMode, CollectionRegistration, CollectionResolvedKind,
        };
        let f = Fixture::new();
        let root = crate::data_dir::get();
        let alpha = root.join("Alpha.png");
        let beta = root.join("Beta.png");
        let zulu = root.join("Zulu.png");
        std::fs::write(&alpha, b"not decoded").unwrap();
        std::fs::write(&beta, b"not decoded").unwrap();
        std::fs::write(&zulu, b"not decoded").unwrap();
        let actor = f.runtime.as_ref().unwrap().client();
        let name = "reading list".repeat(20);
        assert!(
            name.len() > 200,
            "actor allows names beyond search-query limits"
        );
        let collection = actor
            .create_collection(name.clone())
            .unwrap()
            .recv()
            .unwrap()
            .unwrap();
        let added = actor
            .add_batch(
                collection.definition.id,
                collection.definition.revision,
                vec![
                    CollectionRegistration::from_trusted_path(
                        &alpha,
                        CollectionResolvedKind::Image,
                    )
                    .unwrap(),
                    CollectionRegistration::from_trusted_path(&beta, CollectionResolvedKind::Image)
                        .unwrap(),
                    CollectionRegistration::from_trusted_path(&zulu, CollectionResolvedKind::Image)
                        .unwrap(),
                ],
            )
            .unwrap()
            .recv()
            .unwrap()
            .unwrap();
        let sorted = actor
            .set_order(
                collection.definition.id,
                added.snapshot.definition.revision,
                CollectionOrderMode::Standard,
                crate::settings::SortOrder::FileNameDesc,
            )
            .unwrap()
            .recv()
            .unwrap()
            .unwrap();
        std::fs::remove_file(&alpha).unwrap();
        let connection = f.connection();
        let Payload::Items(first) = f
            .call(
                &connection,
                1,
                Command::GetCollection {
                    id: Some(collection.definition.id.to_string()),
                    name: None,
                    limit: Some(1),
                    cursor: None,
                },
            )
            .result
            .unwrap()
        else {
            panic!("collection page")
        };
        assert_eq!(value(&first.items[0]).name, "Zulu.png");
        let Payload::Items(second) = f
            .call(
                &connection,
                2,
                Command::GetCollection {
                    id: Some(sorted.definition.id.to_string()),
                    name: None,
                    limit: Some(1),
                    cursor: first.cursor,
                },
            )
            .result
            .unwrap()
        else {
            panic!("collection page")
        };
        assert_eq!(value(&second.items[0]).name, "Beta.png");
        let Payload::Items(third) = f
            .call(
                &connection,
                3,
                Command::GetCollection {
                    id: Some(sorted.definition.id.to_string()),
                    name: None,
                    limit: Some(1),
                    cursor: second.cursor,
                },
            )
            .result
            .unwrap()
        else {
            panic!("collection page")
        };
        assert_eq!(value(&third.items[0]).name, "Alpha.png");
        assert_eq!(value(&third.items[0]).kind, ItemKind::Image);
        assert!(third.cursor.is_none());
        let Payload::Items(named) = f
            .call(
                &connection,
                3,
                Command::GetCollection {
                    id: None,
                    name: Some(name.clone()),
                    limit: None,
                    cursor: None,
                },
            )
            .result
            .unwrap()
        else {
            panic!("collection page")
        };
        assert_eq!(named.items.len(), 3);
        assert_eq!(
            f.call(
                &connection,
                4,
                Command::GetCollection {
                    id: Some(sorted.definition.id.to_string()),
                    name: Some(name),
                    limit: None,
                    cursor: None,
                }
            )
            .result
            .unwrap_err()
            .code,
            ErrorCode::BadRequest
        );
    }
}
