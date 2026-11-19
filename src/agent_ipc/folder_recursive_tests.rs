use super::*;

fn options() -> FolderOptions {
    FolderOptions {
        recursive: true,
        ..Default::default()
    }
}

fn recursive_list(
    f: &Fixture,
    path: &Path,
    options: FolderOptions,
    limit: Option<u32>,
    cursor: Option<String>,
) -> Result<FolderPage, AgentError> {
    let Payload::Folder(page) = f.handlers.list_folder_with_options(
        path.to_string_lossy().into_owned(),
        None,
        None,
        None,
        None,
        options,
        limit,
        cursor,
        &context(f),
    )?
    else {
        panic!("folder page")
    };
    Ok(page)
}

fn summary(f: &Fixture, path: &Path, options: FolderOptions) -> Result<FolderSummary, AgentError> {
    let Payload::FolderSummary(summary) = f.handlers.summarize_folder(
        path.to_string_lossy().into_owned(),
        None,
        None,
        None,
        None,
        options,
        &context(f),
    )?
    else {
        panic!("summary")
    };
    Ok(summary)
}

fn names(page: &FolderPage) -> Vec<String> {
    rows(page).iter().map(|row| row.name.clone()).collect()
}

fn cursor_state(cursor: &str) -> FolderCursor {
    let bytes = (0..cursor.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&cursor[index..index + 2], 16).unwrap())
        .collect::<Vec<_>>();
    serde_json::from_slice(&bytes).unwrap()
}

fn cursor_filter_tree(f: &Fixture) -> PathBuf {
    let root = f._dir.path().join("cursor-filters");
    std::fs::create_dir_all(root.join("child/deep")).unwrap();
    for name in ["child/deep/photo-a.PNG", "child/photo-b.jpg", "photo-c.png"] {
        std::fs::write(root.join(name), b"x").unwrap();
    }
    root
}

fn cursor_filter_options() -> FolderOptions {
    FolderOptions {
        name_contains: Some("photo".into()),
        extensions: Some(vec!["PNG".into(), "jpg".into()]),
        ..options()
    }
}

#[test]
fn recursive_cursor_rejects_changed_name_and_extension_filters() {
    let f = Fixture::new();
    let root = cursor_filter_tree(&f);
    let first = recursive_list(&f, &root, cursor_filter_options(), Some(1), None).unwrap();
    assert_eq!(names(&first), ["photo-a.PNG"]);
    let cursor = first.cursor.expect("remaining matching rows");
    for changed in [
        FolderOptions {
            name_contains: Some("photo-b".into()),
            ..cursor_filter_options()
        },
        FolderOptions {
            extensions: Some(vec!["jpg".into()]),
            ..cursor_filter_options()
        },
    ] {
        assert_eq!(
            recursive_list(&f, &root, changed, Some(1), Some(cursor.clone()))
                .unwrap_err()
                .code,
            ErrorCode::BadRequest
        );
    }
}

#[test]
fn recursive_cursor_normalizes_extension_case_order_and_default_max_depth() {
    let f = Fixture::new();
    let root = cursor_filter_tree(&f);
    let first = recursive_list(&f, &root, cursor_filter_options(), Some(1), None).unwrap();
    let cursor = first.cursor.expect("remaining matching rows");
    let expected = recursive_list(
        &f,
        &root,
        cursor_filter_options(),
        Some(1),
        Some(cursor.clone()),
    )
    .unwrap();
    assert_eq!(names(&expected), ["photo-b.jpg"]);
    for equivalent in [
        FolderOptions {
            extensions: Some(vec!["JPG".into(), "png".into()]),
            ..cursor_filter_options()
        },
        FolderOptions {
            extensions: Some(vec!["JPG".into(), "png".into()]),
            max_depth: Some(32),
            ..cursor_filter_options()
        },
    ] {
        assert_eq!(
            recursive_list(&f, &root, equivalent, Some(1), Some(cursor.clone())).unwrap(),
            expected
        );
    }
    // The explicit depth also emits a cursor accepted with the null/default depth.
    let explicit_depth = recursive_list(
        &f,
        &root,
        FolderOptions {
            max_depth: Some(32),
            ..cursor_filter_options()
        },
        Some(1),
        Some(cursor),
    )
    .unwrap();
    assert_eq!(explicit_depth, expected);
    let last = recursive_list(
        &f,
        &root,
        cursor_filter_options(),
        Some(1),
        explicit_depth.cursor,
    )
    .unwrap();
    assert_eq!(names(&last), ["photo-c.png"]);
    assert!(last.cursor.is_none());
}

#[test]
fn recursive_preorder_depth_and_summary_share_filtered_logical_rows() {
    let f = Fixture::new();
    let root = f._dir.path().join("tree");
    std::fs::create_dir_all(root.join("A/deep")).unwrap();
    std::fs::create_dir(root.join("B")).unwrap();
    for name in [
        "a.png",
        "A/a.PNG",
        "A/deep/deep.png",
        "B/b.png",
        "A/doc.pdf",
        "book.zip",
    ] {
        std::fs::write(root.join(name), b"fixture").unwrap();
    }
    let db = crate::rating_db::RatingDb::open().unwrap();
    db.set(
        &crate::adjustment_db::normalize_path(&root.join("A/a.PNG")),
        4,
    )
    .unwrap();
    let page = recursive_list(&f, &root, options(), None, None).unwrap();
    assert_eq!(
        names(&page),
        [
            "A", "deep", "deep.png", "doc.pdf", "a.PNG", "B", "b.png", "book.zip", "a.png"
        ]
    );
    assert!(!page.book_composition_applied);
    let shallow = recursive_list(
        &f,
        &root,
        FolderOptions {
            max_depth: Some(1),
            ..options()
        },
        None,
        None,
    )
    .unwrap();
    assert!(!names(&shallow).contains(&"deep.png".into()));
    assert!(names(&shallow).contains(&"deep".into()));
    let all = summary(&f, &root, options()).unwrap();
    assert_eq!(all.total, page.items.len());
    assert_eq!(all.stars, [8, 0, 0, 0, 1, 0]);
    assert_eq!(all.kinds.get(&FolderKind::Folder), Some(&3));
    let folders = all.folders.unwrap();
    assert_eq!(
        folders
            .iter()
            .map(|folder| (folder.name.as_str(), folder.total))
            .collect::<Vec<_>>(),
        [("A", 4), ("B", 1)]
    );
    let filtered = FolderOptions {
        extensions: Some(vec!["PNG".into()]),
        name_contains: Some("a.".into()),
        ..options()
    };
    assert_eq!(summary(&f, &root, filtered).unwrap().total, 2);
    assert!(
        summary(&f, &root, FolderOptions::default())
            .unwrap()
            .folders
            .is_none()
    );
}

#[test]
fn recursive_folder_overlay_resets_between_siblings_and_returns_to_ancestor() {
    let f = Fixture::new();
    let root = f._dir.path().join("overlays");
    for child in ["A", "B", "B/A-again"] {
        std::fs::create_dir_all(root.join(child)).unwrap();
        for name in ["1.png", "2.png"] {
            std::fs::write(root.join(child).join(name), b"x").unwrap();
        }
    }
    let a = crate::settings::FavoriteEntry::new("A".into(), root.join("A"));
    let again = crate::settings::FavoriteEntry::new("A again".into(), root.join("B/A-again"));
    let mut state = crate::settings::FavoriteViewState::from_settings(&Settings::default());
    state.sort_order = crate::settings::SortOrder::FileNameDesc;
    let db = crate::adjustment_db::AdjustmentDb::open().unwrap();
    db.set_favorite_view_state(a.id, &state).unwrap();
    db.set_favorite_view_state(again.id, &state).unwrap();
    settings(|s| {
        s.remember_favorite_view_state = true;
        s.favorites = vec![a, again];
        s.sort_order = crate::settings::SortOrder::FileName;
    });
    assert_eq!(
        names(&recursive_list(&f, &root, options(), None, None).unwrap()),
        [
            "A", "2.png", "1.png", "B", "A-again", "2.png", "1.png", "1.png", "2.png"
        ]
    );
}

#[test]
fn new_filters_validate_normalize_and_combine_without_matching_virtual_tags() {
    let mut row = FolderItem {
        path: "C:/a".into(),
        subresource: None,
        name: "日本語Ä.PnG".into(),
        kind: FolderKind::Image,
        size: None,
        modified: None,
        rating: Some(3),
        tags: Some(vec!["#tag".into()]),
    };
    let extra = FolderOptions {
        name_contains: Some("語ä".into()),
        extensions: Some(vec!["PNG".into(), "jpg".into()]),
        tagged: Some(true),
        ..Default::default()
    };
    let filters = FolderFilters::new(Some(2), Some(4), Some(vec![FolderKind::Image]))
        .unwrap()
        .with_extra(&extra)
        .unwrap();
    assert!(filters.matches(&row));
    assert_eq!(filters.extensions, Some(vec!["jpg".into(), "png".into()]));
    row.tags = Some(vec![]);
    assert!(!filters.matches(&row));
    row.tags = None;
    assert!(
        !FolderFilters::default()
            .with_extra(&FolderOptions {
                tagged: Some(false),
                ..Default::default()
            })
            .unwrap()
            .matches(&row)
    );
    for values in [
        vec![],
        vec![""],
        vec![".png"],
        vec!["a.b"],
        vec!["PNG", "png"],
    ] {
        assert!(
            FolderFilters::default()
                .with_extra(&FolderOptions {
                    extensions: Some(values.into_iter().map(str::to_owned).collect()),
                    ..Default::default()
                })
                .is_err()
        );
    }
    for name in [String::new(), "a".repeat(201)] {
        assert!(
            FolderFilters::default()
                .with_extra(&FolderOptions {
                    name_contains: Some(name),
                    ..Default::default()
                })
                .is_err()
        );
    }
}

#[test]
fn recursive_tagged_pages_open_each_database_once_and_preserve_base_cursor() {
    let f = Fixture::new();
    let root = f._dir.path().join("tagged");
    std::fs::create_dir_all(root.join("child")).unwrap();
    let db = crate::rating_db::RatingDb::open().unwrap();
    let mut tags = crate::tags_db::TagsDb::open().unwrap();
    for name in ["child/a.png", "child/b.png", "last.png"] {
        std::fs::write(root.join(name), b"x").unwrap();
        let key = crate::tags_db::item_key_for_path(&root.join(name));
        tags.set_item_tags(&key, ["tag"], "test").unwrap();
    }
    settings(|s| s.sort_order = crate::settings::SortOrder::RatingDesc);
    DB_OPENS.with(|count| count.set((0, 0)));
    let first = recursive_list(
        &f,
        &root,
        FolderOptions {
            tagged: Some(true),
            ..options()
        },
        Some(1),
        None,
    )
    .unwrap();
    assert_eq!(DB_OPENS.with(|count| count.get()), (1, 1));
    assert_eq!(names(&first), ["a.png"]);
    settings(|s| s.sort_order = crate::settings::SortOrder::FileName);
    let first = recursive_list(
        &f,
        &root,
        FolderOptions {
            tagged: Some(true),
            ..options()
        },
        Some(1),
        None,
    )
    .unwrap();
    db.set(
        &crate::adjustment_db::normalize_path(&root.join("child/a.png")),
        4,
    )
    .unwrap();
    let second = recursive_list(
        &f,
        &root,
        FolderOptions {
            tagged: Some(true),
            ..options()
        },
        Some(1),
        first.cursor.clone(),
    )
    .unwrap();
    assert_eq!(names(&second), ["b.png"]);
    assert_eq!(
        recursive_list(
            &f,
            &root,
            FolderOptions {
                tagged: Some(false),
                ..options()
            },
            Some(1),
            first.cursor.clone()
        )
        .unwrap_err()
        .code,
        ErrorCode::BadRequest
    );
    assert_eq!(
        recursive_list(
            &f,
            &root,
            FolderOptions {
                max_depth: Some(1),
                tagged: Some(true),
                ..options()
            },
            Some(1),
            first.cursor
        )
        .unwrap_err()
        .code,
        ErrorCode::BadRequest
    );
    assert_eq!(
        summary(
            &f,
            &root,
            FolderOptions {
                tagged: Some(false),
                ..options()
            }
        )
        .unwrap()
        .total,
        1
    );
}

#[test]
fn recursive_validation_permissions_reparse_and_access_errors_are_explicit() {
    let f = Fixture::new();
    let root = f._dir.path().join("validation");
    std::fs::create_dir(&root).unwrap();
    for depth in [0, 33] {
        assert_eq!(
            recursive_list(
                &f,
                &root,
                FolderOptions {
                    max_depth: Some(depth),
                    ..options()
                },
                None,
                None
            )
            .unwrap_err()
            .code,
            ErrorCode::BadRequest
        );
    }
    assert_eq!(
        recursive_list(
            &f,
            &root,
            FolderOptions {
                max_depth: Some(1),
                ..Default::default()
            },
            None,
            None
        )
        .unwrap_err()
        .code,
        ErrorCode::BadRequest
    );
    for name in ["a.zip", "a.pdf"] {
        std::fs::write(root.join(name), b"x").unwrap();
        assert_eq!(
            recursive_list(&f, &root.join(name), options(), None, None)
                .unwrap_err()
                .code,
            ErrorCode::BadRequest
        );
    }
    assert_eq!(
        f.handlers
            .list_folder_with_options(
                root.to_string_lossy().into(),
                Some("a/".into()),
                None,
                None,
                None,
                options(),
                None,
                None,
                &context(&f)
            )
            .unwrap_err()
            .code,
        ErrorCode::BadRequest
    );
    for tag in [0xA000_0003, 0xA000_000C] {
        assert!(name_surrogate(tag));
    }
    assert!(!name_surrogate(0x9000_001A)); // cloud placeholder
    assert!(enter_reparse_tag(Ok(0x9000_001A)).unwrap());
    assert!(!enter_reparse_tag(Ok(0xA000_0003)).unwrap());
    assert!(enter_reparse_tag(Err(std::io::ErrorKind::PermissionDenied.into())).is_err());
    assert!(enter_reparse_tag(Err(std::io::ErrorKind::Other.into())).is_err());
    let denied = folder_read_error(&root, true, std::io::ErrorKind::PermissionDenied.into());
    assert_eq!(denied.code, ErrorCode::BadRequest);
    assert!(denied.message.contains(root.to_str().unwrap()));
    assert_eq!(
        folder_read_error(&root, false, std::io::ErrorKind::PermissionDenied.into()).code,
        ErrorCode::Internal
    );
    assert_eq!(
        folder_read_error(&root, true, std::io::ErrorKind::Other.into()).code,
        ErrorCode::Internal
    );
    f.set_level(AgentLevel::Disabled);
    let connection = f.connection();
    assert_eq!(
        f.call(
            &connection,
            10,
            Command::SummarizeFolder {
                path: root.to_string_lossy().into(),
                subresource: None,
                min_stars: None,
                max_stars: None,
                kinds: None,
                recursive: false,
                max_depth: None,
                name_contains: None,
                extensions: None,
                tagged: None
            }
        )
        .result
        .unwrap_err()
        .code,
        ErrorCode::Disabled
    );
}

#[test]
fn recursive_summary_limits_direct_folders_to_two_hundred() {
    let f = Fixture::new();
    let root = f._dir.path().join("many");
    std::fs::create_dir(&root).unwrap();
    for index in 0..201 {
        std::fs::create_dir(root.join(format!("{index:04}"))).unwrap();
    }
    let result = summary(&f, &root, options()).unwrap();
    assert_eq!(result.total, 201);
    assert!(result.folders_truncated);
    assert!(mcp_content_chars(&result).unwrap() <= MAX_PAGE_CHARS);
    assert!((1..=200).contains(&result.folders.unwrap().len()));
    // Short synthetic paths isolate the count cap from the independent text cap.
    let listing = FolderListing {
        items: vec![],
        applied: false,
        args: serde_json::Value::Null,
        folders: (0..201)
            .map(|index| {
                (
                    FolderSummaryFolder {
                        path: format!("C:/{index}"),
                        name: index.to_string(),
                        total: 0,
                    },
                    0..0,
                )
            })
            .collect(),
    };
    let capped = summarize_listing(listing, true, &FolderFilters::default()).unwrap();
    assert!(capped.folders_truncated);
    assert_eq!(capped.folders.unwrap().len(), 200);
}

#[test]
fn summary_zip_pdf_and_virtual_tag_filters_match_their_listings() {
    let f = Fixture::new();
    let (_, owner) = composition(&f);
    let zip = f._dir.path().join("summary.zip");
    std::fs::write(&zip, zip_bytes(&[("a.png", b"x"), ("b.png", b"x")])).unwrap();
    let pdf = f._dir.path().join("summary.pdf");
    let _catalog = seed_pdf(&pdf, 3);
    for (path, count, kind) in [
        (&zip, 2, FolderKind::ZipEntry),
        (&pdf, 3, FolderKind::PdfPage),
    ] {
        let result = summary(&f, path, FolderOptions::default()).unwrap();
        assert_eq!(result.total, count);
        assert_eq!(result.kinds.get(&kind), Some(&count));
        for tagged in [true, false] {
            assert_eq!(
                summary(
                    &f,
                    path,
                    FolderOptions {
                        tagged: Some(tagged),
                        ..Default::default()
                    }
                )
                .unwrap()
                .total,
                0
            );
        }
    }
    owner.shutdown_and_join();
}

#[test]
fn summary_counts_original_oversized_rows_and_truncates_folders_by_char_budget() {
    let item = FolderItem {
        path: format!("C:/{}", "long".repeat(5000)),
        subresource: None,
        name: "original.png".into(),
        kind: FolderKind::Image,
        size: None,
        modified: None,
        rating: Some(4),
        tags: Some(vec![]),
    };
    assert!(mcp_content_chars(&item).unwrap() > MAX_ITEM_CHARS);
    let listing = FolderListing {
        items: vec![item],
        applied: false,
        args: serde_json::Value::Null,
        folders: (0..100)
            .map(|index| {
                (
                    FolderSummaryFolder {
                        path: format!("C:/{index}/{}", "long".repeat(100)),
                        name: format!("folder{index}"),
                        total: 0,
                    },
                    0..1,
                )
            })
            .collect(),
    };
    let result = summarize_listing(listing, true, &FolderFilters::default()).unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.kinds.get(&FolderKind::Image), Some(&1));
    assert_eq!(result.stars[4], 1);
    assert!(result.folders_truncated);
    assert!(result.folders.as_ref().unwrap().len() < 100);
    assert!(
        result
            .folders
            .as_ref()
            .unwrap()
            .iter()
            .all(|folder| folder.total == 1)
    );
    assert!(mcp_content_chars(&result).unwrap() <= MAX_PAGE_CHARS);
}

#[cfg(windows)]
#[test]
fn recursive_junction_is_listed_without_entering_the_cycle() {
    let f = Fixture::new();
    let root = f._dir.path().join("junction-tree");
    std::fs::create_dir_all(root.join("child")).unwrap();
    std::fs::write(root.join("child/a.png"), b"x").unwrap();
    let link = root.join("child/back");
    let output = std::process::Command::new("cmd.exe")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!enter_folder(&link).unwrap());
    let result = recursive_list(&f, &root, options(), None, None).unwrap();
    assert_eq!(names(&result), ["child", "back", "a.png"]);
    let result = summary(&f, &root, options()).unwrap();
    assert_eq!(result.total, 3);
    assert_eq!(result.folders.unwrap()[0].total, 2);
    // Remove only the junction itself before the temporary owner is dropped.
    std::fs::remove_dir(link).unwrap();
}

#[test]
fn recursive_near_limit_performance_and_over_limit_rejection() {
    let f = Fixture::new();
    let root = f._dir.path().join("near-limit");
    std::fs::create_dir(&root).unwrap();
    let db = crate::rating_db::RatingDb::open().unwrap();
    let mut tags = crate::tags_db::TagsDb::open().unwrap();
    for index in 0..1000 {
        let folder = if index == 0 {
            root.clone()
        } else {
            root.join(format!("{index:04}"))
        };
        if index > 0 {
            std::fs::create_dir(&folder).unwrap();
        }
        for file in 0..49 {
            std::fs::write(folder.join(format!("{file:02}.png")), b"x").unwrap();
        }
        db.set(
            &crate::adjustment_db::normalize_path(&folder.join("48.png")),
            3,
        )
        .unwrap();
        tags.set_item_tags(
            &crate::tags_db::item_key_for_path(&folder.join("48.png")),
            ["tag"],
            "test",
        )
        .unwrap();
    }
    let measured = |label: &str, options: FolderOptions, cursor: Option<String>| {
        let started = Instant::now();
        let page = recursive_list(&f, &root, options, None, cursor).unwrap();
        eprintln!(
            "MCP740_PERF {label}: {} ms, rows={}, cursor={}",
            started.elapsed().as_millis(),
            page.items.len(),
            page.cursor.is_some()
        );
        assert!(started.elapsed() <= Duration::from_secs(10));
        assert!(!page.items.is_empty() && page.items.len() <= 100);
        assert!(page.cursor.is_some());
        page
    };
    let expected_paths = |rated: bool| {
        let mut paths = Vec::with_capacity(49_999);
        // Root folder rows precede its media; each child is followed by its media.
        for index in (1..1000).chain(std::iter::once(0)) {
            let folder = if index == 0 {
                root.clone()
            } else {
                let folder = root.join(format!("{index:04}"));
                paths.push(folder.clone());
                folder
            };
            if rated {
                paths.push(folder.join("48.png"));
            }
            for file in 0..if rated { 48 } else { 49 } {
                paths.push(folder.join(format!("{file:02}.png")));
            }
        }
        assert_eq!(paths.len(), 49_999);
        paths
    };
    let page_paths = |page: &FolderPage| {
        let items = rows(page);
        assert_eq!(items.len(), page.items.len(), "no oversized-row markers");
        items
            .iter()
            .map(|row| PathBuf::from(&row.path))
            .collect::<Vec<_>>()
    };
    let name_order = expected_paths(false);
    let first = measured("first", options(), None);
    let first_paths = page_paths(&first);
    assert_eq!(first_paths, name_order[..first.items.len()]);
    let first_position = cursor_state(first.cursor.as_deref().unwrap());
    assert_eq!(first_position.version, 5);
    assert_eq!(first_position.returned, first.items.len());
    let second = measured("continuation", options(), first.cursor);
    let second_paths = page_paths(&second);
    assert_eq!(
        second_paths,
        name_order[first.items.len()..first.items.len() + second.items.len()]
    );
    let first_set = first_paths.iter().collect::<std::collections::HashSet<_>>();
    let second_set = second_paths
        .iter()
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(first_set.len(), first_paths.len());
    assert_eq!(second_set.len(), second_paths.len());
    assert!(first_set.is_disjoint(&second_set));
    let second_position = cursor_state(second.cursor.as_deref().unwrap());
    assert_eq!(
        second_position.returned,
        first.items.len() + second.items.len()
    );
    assert!(second_position.returned > first_position.returned);
    settings(|s| s.sort_order = crate::settings::SortOrder::RatingDesc);
    let rated = measured("saved-rating-order", options(), None);
    let rating_order = expected_paths(true);
    let rated_paths = page_paths(&rated);
    assert_eq!(rated_paths, rating_order[..rated.items.len()]);
    assert_eq!(first_paths[1], root.join("0001/00.png"));
    assert_eq!(rated_paths[1], root.join("0001/48.png"));
    assert_ne!(first_paths[1], rated_paths[1]);
    assert_eq!(rows(&rated)[1].rating, Some(3));
    let tagged = measured(
        "tagged",
        FolderOptions {
            tagged: Some(true),
            ..options()
        },
        None,
    );
    let tagged_order = rating_order
        .iter()
        .filter(|path| path.file_name().unwrap() == "48.png")
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(tagged_order.len(), 1000);
    assert_eq!(page_paths(&tagged), tagged_order[..tagged.items.len()]);
    for row in rows(&tagged) {
        assert_eq!(row.name, "48.png");
        assert_eq!(row.kind, FolderKind::Image);
        assert_eq!(row.rating, Some(3));
        assert_eq!(row.tags.as_deref(), Some(&["#tag".to_owned()][..]));
        assert!(row.subresource.is_none());
    }
    assert_eq!(
        cursor_state(tagged.cursor.as_deref().unwrap()).returned,
        (tagged.items.len() - 1) * 50 + 2
    );
    let started = Instant::now();
    let result = summary(&f, &root, options()).unwrap();
    eprintln!(
        "MCP740_PERF summary: {} ms, total={}",
        started.elapsed().as_millis(),
        result.total
    );
    assert_eq!(result.total, 49_999);
    assert_eq!(result.stars[3], 1000);
    assert!(started.elapsed() <= Duration::from_secs(10));
    std::fs::create_dir(root.join("overflow")).unwrap();
    assert_eq!(
        summary(&f, &root, options()).unwrap_err().code,
        ErrorCode::BadRequest
    );
    std::fs::remove_dir(root.join("overflow")).unwrap();
    for name in ["extra1.png", "extra2.png"] {
        std::fs::write(root.join(name), b"x").unwrap();
    }
    assert_eq!(
        summary(&f, &root, options()).unwrap_err().code,
        ErrorCode::BadRequest
    );
}
