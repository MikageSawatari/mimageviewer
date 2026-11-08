use super::super::tests::Fixture;
use super::*;

fn request(
    f: &Fixture,
    kind: LibraryKind,
    stars: Option<u8>,
    max_stars: Option<u8>,
    cursor: Option<String>,
) -> Result<LibraryPage, AgentError> {
    let connection = f.connection();
    let Payload::Library(page) = f
        .call(
            &connection,
            1,
            Command::ListLibrary {
                kind,
                stars,
                max_stars,
                limit: Some(1),
                cursor,
            },
        )
        .result?
    else {
        panic!("library page")
    };
    Ok(page)
}

fn value(row: &PageRow<LibraryItem>) -> &LibraryItem {
    let PageRow::Value(item) = row else {
        panic!("short row")
    };
    item
}

#[test]
fn rated_ceiling_preserves_order_paging_and_argument_binding() {
    let f = Fixture::new();
    let db = crate::rating_db::RatingDb::open().unwrap();
    for (name, stars) in [
        ("a.png", 2),
        ("b.png", 2),
        ("c.png", 3),
        ("d.png", 4),
        ("e.png", 1),
    ] {
        db.set(&format!("C:/{name}"), stars).unwrap();
    }
    let times = rusqlite::Connection::open(crate::rating_db::RatingDb::db_path()).unwrap();
    times
        .execute(
            "UPDATE ratings SET rated_at_ms = CASE WHEN path = 'C:/b.png' THEN 300 ELSE 100 END",
            [],
        )
        .unwrap();
    let mut cursor = None;
    let mut names = Vec::new();
    loop {
        let page = request(&f, LibraryKind::Rated, Some(2), Some(3), cursor).unwrap();
        assert_eq!(page.items.len(), 1);
        names.push(value(&page.items[0]).name.clone());
        if !page.truncated {
            assert!(page.cursor.is_none());
            break;
        }
        cursor = page.cursor;
    }
    assert_eq!(names, ["c.png", "b.png", "a.png"]);
    let first = request(&f, LibraryKind::Rated, Some(2), Some(2), None).unwrap();
    assert_eq!(value(&first.items[0]).rating, Some(2));
    assert_eq!(
        request(
            &f,
            LibraryKind::Rated,
            Some(2),
            Some(3),
            first.cursor.clone()
        )
        .unwrap_err()
        .code,
        ErrorCode::BadRequest
    );
    assert_eq!(
        request(&f, LibraryKind::Rated, Some(2), None, first.cursor.clone())
            .unwrap_err()
            .code,
        ErrorCode::BadRequest
    );
    let last = request(&f, LibraryKind::Rated, Some(2), Some(2), first.cursor).unwrap();
    assert_eq!(value(&last.items[0]).name, "a.png");
    assert!(!last.truncated);
    for min in [None, Some(0), Some(1)] {
        let page = request(&f, LibraryKind::Rated, min, Some(1), None).unwrap();
        assert_eq!(value(&page.items[0]).rating, Some(1));
    }
    let empty = request(&f, LibraryKind::Rated, Some(5), Some(5), None).unwrap();
    assert!(empty.items.is_empty());
    assert!(!empty.truncated);
    assert!(empty.cursor.is_none());
}

#[test]
fn rated_ceiling_rejects_nonrated_kinds_invalid_ranges_and_values() {
    let f = Fixture::new();
    for kind in [
        LibraryKind::Favorites,
        LibraryKind::ReadingHistory,
        LibraryKind::Bookshelf,
        LibraryKind::Bookmarks,
    ] {
        assert_eq!(
            request(&f, kind, None, Some(5), None).unwrap_err().code,
            ErrorCode::BadRequest
        );
    }
    for (min, max) in [
        (None, 0),
        (Some(0), 0),
        (Some(3), 2),
        (Some(5), 4),
        (Some(1), 6),
    ] {
        assert_eq!(
            request(&f, LibraryKind::Rated, min, Some(max), None)
                .unwrap_err()
                .code,
            ErrorCode::BadRequest
        );
    }
}
