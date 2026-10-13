//! Conversion metadata uses the existing store descriptors and FIFO owners.
//! One worker reads one page at a time; original rows are never updated/deleted.
use rusqlite::{Connection, OptionalExtension, params, types::Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, mpsc};

pub(crate) type CopyReply = Result<usize, String>;
pub(crate) type CopyPortal = Arc<
    dyn Fn(&crate::image_folder_zip::ImageFolderManifest, CopyPhase) -> mpsc::Receiver<CopyReply>
        + Send
        + Sync,
>;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyPhase {
    Check,
    Apply,
}

pub(crate) fn target_order(
    manifest: &crate::image_folder_zip::ImageFolderManifest,
    sort: crate::settings::SortOrder,
) -> Vec<String> {
    let mut names: Vec<_> = manifest
        .members
        .iter()
        .map(|member| (member.entry.clone(), sort.name_key(&member.entry)))
        .collect();
    // The existing archive reader assigns the outer ZIP's mtime to every entry.
    names.sort_by(|(_, a), (_, b)| sort.compare_name_keys(a, 0, b, 0));
    names.into_iter().map(|(name, _)| name).collect()
}

pub(crate) const CONFLICT: &str = "作成先に以前の登録や編集が残っているため、ZIP を作れませんでした。元のフォルダは残っています。";

pub(crate) struct ZipMetadataPlan {
    pub body_tags: Option<Vec<crate::tags_db::ItemTag>>,
    pub supplemental_ratings: HashMap<String, u8>,
    pub rows: usize,
}

fn open_existing(path: &Path) -> Result<Option<Connection>, String> {
    if !path.try_exists().map_err(|e| e.to_string())? {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    Ok(Some(conn))
}

fn rating_remap(
    columns: &[String],
    row: &mut Vec<Value>,
    destination: &Path,
    entry: Option<&str>,
) -> Result<(), String> {
    replace_column(
        columns,
        row,
        "source_path",
        Value::Text(destination.to_string_lossy().into_owned()),
    )?;
    replace_column(
        columns,
        row,
        "kind",
        Value::Integer(if entry.is_some() { 6 } else { 3 }),
    )?;
    replace_column(
        columns,
        row,
        "entry_name",
        entry.map_or(Value::Null, |s| Value::Text(s.into())),
    )?;
    for name in [
        "page_num",
        "dir_prefix",
        "archive_format",
        "zipdir_is_archive",
        "zipdir_representative",
    ] {
        replace_column(columns, row, name, Value::Null)?;
    }
    Ok(())
}

fn copy_store_rows(
    data_dir: &Path,
    manifest: &crate::image_folder_zip::ImageFolderManifest,
    phase: CopyPhase,
    supplemental: &HashMap<String, u8>,
) -> CopyReply {
    let mut count = 0;
    for descriptor in crate::rename_key_migration::STORES {
        let pages = matches!(
            descriptor.table,
            "page_params"
                | "masks"
                | "conceal_entries"
                | "local_adjust_pages"
                | "export_crop_pages"
                | "comic_entries"
                | "rotations"
                | "view_trim_pages"
        );
        let container = matches!(
            descriptor.table,
            "spreads"
                | "final_cover_spreads"
                | "singleton_spread_placements"
                | "singleton_spread_endpoint_placements"
                | "page_alone_preferences"
                | "view_trim_books"
                | "spread_shifts"
                | "folder_thumb_pins"
        );
        if !pages && !container && descriptor.table != "ratings" {
            continue;
        }
        let Some(conn) = open_existing(&data_dir.join(descriptor.file))? else {
            continue;
        };
        let exists = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
                [descriptor.table],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .is_some();
        if !exists && descriptor.optional_legacy_table() {
            continue;
        }
        if descriptor.table == "spread_shifts" {
            crate::spread_shift::validate_database_schema(&conn)?;
        }
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        if pages || descriptor.table == "ratings" {
            for member in &manifest.members {
                if descriptor.table == "ratings"
                    && supplemental
                        .contains_key(&crate::path_key::normalize_keep_drive(&member.path))
                {
                    continue;
                }
                count += copy_rows_on(
                    &tx,
                    descriptor.table,
                    descriptor.column,
                    &descriptor.normalize_path(&member.path),
                    &crate::adjustment_db::zip_entry_key(&manifest.destination, &member.entry),
                    phase,
                    |columns, row| {
                        if descriptor.table == "ratings" {
                            rating_remap(columns, row, &manifest.destination, Some(&member.entry))?;
                        }
                        Ok(())
                    },
                )?;
            }
        }
        if container || descriptor.table == "ratings" {
            count += copy_rows_on(
                &tx,
                descriptor.table,
                descriptor.column,
                &descriptor.normalize_path(&manifest.source),
                &descriptor.normalize_path(&manifest.destination),
                phase,
                |columns, row| {
                    if descriptor.table == "ratings" {
                        rating_remap(columns, row, &manifest.destination, None)?;
                    }
                    if descriptor.table == "spread_shifts" {
                        let i = columns
                            .iter()
                            .position(|c| c == "anchor_ref")
                            .ok_or("shift anchor absent")?;
                        let Value::Text(encoded) = &row[i] else {
                            return Err("invalid shift anchor".into());
                        };
                        let value: crate::spread_shift::SpreadShiftPageRef =
                            serde_json::from_str(encoded).map_err(|e| e.to_string())?;
                        let crate::spread_shift::SpreadShiftPageRef::RelativePath(name) = value
                        else {
                            return Err("invalid folder shift reference".into());
                        };
                        let remapped = crate::spread_shift::SpreadShiftPageRef::ArchiveEntry(name);
                        remapped.validate()?;
                        row[i] = Value::Text(
                            serde_json::to_string(&remapped).map_err(|e| e.to_string())?,
                        );
                    }
                    if descriptor.table == "folder_thumb_pins" {
                        let get = |name: &str| {
                            columns
                                .iter()
                                .position(|c| c == name)
                                .and_then(|i| match &row[i] {
                                    Value::Text(s) => Some(s.clone()),
                                    _ => None,
                                })
                        };
                        let kind = get("source_kind").ok_or("invalid folder representative")?;
                        let entry =
                            get("source_rel").ok_or("invalid folder representative member")?;
                        if kind != "image" {
                            return Err("invalid image-folder representative kind".into());
                        }
                        crate::book_composition::PageDomain::ImageFolder
                            .validate_page(
                                &crate::book_composition::CompositionPageRef::RelativePath {
                                    path: entry.clone(),
                                },
                            )
                            .map_err(|e| e.to_string())?;
                        replace_column(
                            columns,
                            row,
                            "source_kind",
                            Value::Text("zipentry".into()),
                        )?;
                        replace_column(columns, row, "source_rel", Value::Text(String::new()))?;
                        replace_column(columns, row, "source_entry", Value::Text(entry))?;
                    }
                    Ok(())
                },
            )?;
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    // Dormant G4F rows must never be resurrected as a ZIP composition.
    if let Some(conn) = open_existing(&data_dir.join("book_composition.db"))? {
        crate::book_composition::validate_database_schema(&conn)?;
        let key = crate::book_scope::BookScopeKey::new(&manifest.destination, "")?;
        let present = conn
            .query_row(
                "SELECT 1 FROM book_compositions WHERE book_key=?1",
                [key.storage_key()],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .is_some();
        if present {
            return Err(CONFLICT.into());
        }
    }
    Ok(count)
}

pub(crate) fn prepare_metadata(
    data_dir: &Path,
    manifest: &crate::image_folder_zip::ImageFolderManifest,
    read_xmp: bool,
    session_ratings: &HashMap<String, u8>,
) -> Result<ZipMetadataPlan, String> {
    let mut union = BTreeMap::<String, crate::tags_db::ItemTag>::new();
    let mut decided = false;
    let tags = open_existing(&data_dir.join("tags.db"))?;
    let ratings = open_existing(&data_dir.join("rating.db"))?;
    let mut supplemental_ratings = HashMap::new();
    for path in std::iter::once(&manifest.source).chain(manifest.members.iter().map(|m| &m.path)) {
        let key = crate::path_key::normalize_keep_drive(path);
        if let Some(conn) = &tags {
            let state = conn
                .query_row(
                    "SELECT 1 FROM tag_item_state WHERE item_key=?1",
                    [&key],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let mut stmt=conn.prepare("SELECT tag,tag_key,applied_at FROM item_tags WHERE item_key=?1 ORDER BY tag_key").map_err(|e|e.to_string())?;
            let items = stmt
                .query_map([&key], |r| {
                    Ok(crate::tags_db::ItemTag {
                        tag: r.get(0)?,
                        tag_key: r.get(1)?,
                        applied_at: r.get(2)?,
                    })
                })
                .map_err(|e| e.to_string())?;
            decided |= state.is_some();
            for item in items {
                let item = item.map_err(|e| e.to_string())?;
                if item.tag_key != crate::tags_db::normalize_tag_key(&item.tag) {
                    return Err("invalid saved tag key".into());
                }
                decided = true;
                if union
                    .get(&item.tag_key)
                    .is_none_or(|old| item.applied_at < old.applied_at)
                {
                    union.insert(item.tag_key.clone(), item);
                }
            }
        }
        if path != &manifest.source {
            crate::metadata_transfer::folder_zip_page_backup(data_dir, path)?;
            let db_present = ratings
                .as_ref()
                .map(|conn| {
                    conn.query_row("SELECT 1 FROM ratings WHERE path=?1", [&key], |r| {
                        r.get::<_, i64>(0)
                    })
                    .optional()
                })
                .transpose()
                .map_err(|e| e.to_string())?
                .flatten()
                .is_some();
            if !db_present {
                if let Some(&stars) = session_ratings.get(&key) {
                    supplemental_ratings.insert(key, stars);
                } else if read_xmp && let Some(stars) = crate::xmp_reader::read_xmp_rating(path) {
                    supplemental_ratings.insert(key, stars);
                }
            }
        }
    }
    let mut rows = copy_store_rows(data_dir, manifest, CopyPhase::Check, &supplemental_ratings)?;
    for member in &manifest.members {
        if let Some(&stars) =
            supplemental_ratings.get(&crate::path_key::normalize_keep_drive(&member.path))
        {
            if !check_supplemental_rating(ratings.as_ref(), manifest, member, stars)? && stars != 0
            {
                rows += 1;
            }
        }
    }
    if let Some(conn) = &tags {
        let prefix = format!(
            "{}::%",
            crate::tags_db::escape_like_pattern(&crate::path_key::normalize_keep_drive(
                &manifest.destination
            ))
        );
        let present: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM item_tags WHERE item_key LIKE ?1 ESCAPE '\\' UNION SELECT 1 FROM tag_item_state WHERE item_key LIKE ?1 ESCAPE '\\')", [&prefix], |r| r.get(0)).map_err(|e|e.to_string())?;
        if present {
            return Err(CONFLICT.into());
        }
    }
    let body_tags = decided.then(|| union.into_values().collect::<Vec<_>>());
    let body_present = check_body_tags(
        tags.as_ref(),
        &crate::path_key::normalize_keep_drive(&manifest.destination),
        body_tags.as_deref(),
    )?;
    if !body_present && let Some(tags) = &body_tags {
        rows += tags.len() + 1; // The explicit decision is a durable row, including empty tags.
    }
    Ok(ZipMetadataPlan {
        body_tags,
        supplemental_ratings,
        rows,
    })
}

fn check_supplemental_rating(
    conn: Option<&Connection>,
    manifest: &crate::image_folder_zip::ImageFolderManifest,
    member: &crate::image_folder_zip::ImageMember,
    stars: u8,
) -> Result<bool, String> {
    let Some(conn) = conn else { return Ok(false) };
    let key = crate::adjustment_db::zip_entry_key(&manifest.destination, &member.entry);
    let current = conn.query_row("SELECT stars, rated_at_ms, source_path, kind, entry_name, page_num, dir_prefix, archive_format, zipdir_is_archive, zipdir_representative FROM ratings WHERE path=?1", [&key], |r| (0..10).map(|i| r.get::<_,Value>(i)).collect::<Result<Vec<_>,_>>()).optional().map_err(|e|e.to_string())?;
    if let Some(current) = current {
        let expected = vec![
            Value::Integer(stars.into()),
            Value::Null,
            Value::Text(manifest.destination.to_string_lossy().into_owned()),
            Value::Integer(6),
            Value::Text(member.entry.clone()),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
        ];
        if current != expected {
            return Err(CONFLICT.into());
        }
        return Ok(true);
    }
    Ok(false)
}

fn check_body_tags(
    conn: Option<&Connection>,
    key: &str,
    expected: Option<&[crate::tags_db::ItemTag]>,
) -> Result<bool, String> {
    let Some(conn) = conn else {
        return Ok(false);
    };
    let decided = conn
        .query_row(
            "SELECT 1 FROM tag_item_state WHERE item_key=?1",
            [key],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .is_some();
    let mut stmt = conn
        .prepare("SELECT tag,tag_key,applied_at FROM item_tags WHERE item_key=?1 ORDER BY tag_key")
        .map_err(|e| e.to_string())?;
    let current = stmt
        .query_map([key], |r| {
            Ok(crate::tags_db::ItemTag {
                tag: r.get(0)?,
                tag_key: r.get(1)?,
                applied_at: r.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if decided || !current.is_empty() {
        if expected != Some(current.as_slice()) {
            return Err(CONFLICT.into());
        }
        return Ok(true);
    }
    Ok(false)
}

pub(crate) fn apply_metadata(
    data_dir: &Path,
    manifest: &crate::image_folder_zip::ImageFolderManifest,
    plan: &ZipMetadataPlan,
) -> CopyReply {
    let mut count = copy_store_rows(
        data_dir,
        manifest,
        CopyPhase::Apply,
        &plan.supplemental_ratings,
    )?;
    if let Some(tags) = &plan.body_tags {
        let db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db"))
            .map_err(|e| e.to_string())?;
        drop(db);
        let conn = open_existing(&data_dir.join("tags.db"))?.ok_or("tag DB absent")?;
        let key = crate::path_key::normalize_keep_drive(&manifest.destination);
        if !check_body_tags(Some(&conn), &key, Some(tags))? {
            let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
            for tag in tags {
                tx.execute(
                    "INSERT INTO item_tags(item_key,tag,tag_key,applied_at) VALUES(?1,?2,?3,?4)",
                    params![key, tag.tag, tag.tag_key, tag.applied_at],
                )
                .map_err(|e| e.to_string())?;
            }
            tx.execute(
                "INSERT INTO tag_item_state(item_key,decided_at,source) VALUES(?1,unixepoch(),?2)",
                params![key, crate::tags_db::source::METADATA_IMPORT],
            )
            .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            count += tags.len() + 1;
        }
    }
    if !plan.supplemental_ratings.is_empty() {
        let db = crate::rating_db::RatingDb::open_at(data_dir.join("rating.db"))
            .map_err(|e| e.to_string())?;
        for member in &manifest.members {
            if let Some(&stars) = plan
                .supplemental_ratings
                .get(&crate::path_key::normalize_keep_drive(&member.path))
            {
                let conn = open_existing(&data_dir.join("rating.db"))?;
                if check_supplemental_rating(conn.as_ref(), manifest, member, stars)? {
                    continue;
                }
                let meta =
                    crate::rating_db::RatingMeta::new(crate::rating_db::RatingItemKind::ZipImage)
                        .with_source_path(&manifest.destination);
                let mut meta = meta;
                meta.entry_name = Some(member.entry.clone());
                db.set_imported_rating(
                    &crate::adjustment_db::zip_entry_key(&manifest.destination, &member.entry),
                    stars,
                    Some(&meta),
                )
                .map_err(|e| e.to_string())?;
                count += usize::from(stars != 0);
            }
        }
    }
    Ok(count)
}

/// Common exact-row copy primitive, invoked on the store's existing writer.
/// Equality is checked before insertion; an absent source cannot erase old target data.
pub(crate) fn copy_rows_on(
    conn: &Connection,
    table: &str,
    key_column: &str,
    source: &str,
    target: &str,
    phase: CopyPhase,
    mut remap: impl FnMut(&[String], &mut Vec<Value>) -> Result<(), String>,
) -> CopyReply {
    let columns = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| e.to_string())?
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if columns.iter().any(|name| {
        name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }) {
        return Err("unsupported metadata columns".into());
    }
    if columns.is_empty() {
        return Err(format!("missing metadata table {table}"));
    }
    let key_index = columns
        .iter()
        .position(|c| c == key_column)
        .ok_or("missing metadata key column")?;
    let read = |key: &str| -> Result<Vec<Vec<Value>>, String> {
        let mut stmt = conn
            .prepare(&format!("SELECT * FROM {table} WHERE {key_column}=?1"))
            .map_err(|e| e.to_string())?;
        stmt.query_map([key], |row| {
            (0..columns.len()).map(|i| row.get(i)).collect()
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
    };
    let mut expected = read(source)?;
    for row in &mut expected {
        row[key_index] = Value::Text(target.into());
        remap(&columns, row)?;
    }
    let mut current = read(target)?;
    // Bookmark IDs are local row identities; the clone receives fresh IDs.
    if table == "book_bookmarks" {
        let id = columns
            .iter()
            .position(|c| c == "id")
            .ok_or("bookmark ID column absent")?;
        for row in &mut current {
            row[id] = Value::Null;
        }
        expected.sort_by_key(|row| format!("{row:?}"));
        current.sort_by_key(|row| format!("{row:?}"));
    }
    if !current.is_empty() {
        if current != expected {
            return Err(CONFLICT.into());
        }
        return Ok(0);
    }
    if phase == CopyPhase::Apply {
        let names = columns.join(",");
        let placeholders = (0..columns.len())
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let mut stmt = conn
            .prepare(&format!(
                "INSERT INTO {table} ({names}) VALUES ({placeholders})"
            ))
            .map_err(|e| e.to_string())?;
        for row in &expected {
            stmt.execute(rusqlite::params_from_iter(row))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(expected.len())
}

pub(crate) fn replace_column(
    columns: &[String],
    row: &mut [Value],
    column: &str,
    value: Value,
) -> Result<(), String> {
    let index = columns
        .iter()
        .position(|c| c == column)
        .ok_or_else(|| format!("missing metadata column {column}"))?;
    row[index] = value;
    Ok(())
}

pub(crate) fn clone_resume_on(
    conn: &Connection,
    source: &Path,
    destination: &Path,
    members: &[String],
    legacy: Option<&[String]>,
    phase: CopyPhase,
) -> CopyReply {
    let source_key = crate::path_key::normalize(source);
    let target_key = crate::path_key::normalize(destination);
    copy_rows_on(
        conn,
        "book_resume",
        "path",
        &source_key,
        &target_key,
        phase,
        |columns, row| {
            let idx = columns
                .iter()
                .position(|c| c == "page_identity")
                .ok_or("resume identity column absent")?;
            if row[idx] == Value::Null {
                if let Some(legacy) = legacy {
                    let page = columns
                        .iter()
                        .position(|c| c == "page")
                        .ok_or("resume index absent")?;
                    if let Value::Integer(old) = row[page] {
                        if let Some(name) = usize::try_from(old).ok().and_then(|i| legacy.get(i)) {
                            let identity = crate::book_resume_db::PageIdentity::Image {
                                path: crate::path_key::normalize_keep_drive(&source.join(name)),
                            };
                            row[idx] = Value::Text(
                                serde_json::to_string(&identity).map_err(|e| e.to_string())?,
                            );
                        }
                    }
                }
            }
            if let Value::Text(json) = &row[idx] {
                let identity: crate::book_resume_db::PageIdentity =
                    serde_json::from_str(json).map_err(|e| e.to_string())?;
                let remapped = match identity {
                    crate::book_resume_db::PageIdentity::Image { path } => {
                        let relative = members
                            .iter()
                            .find(|n| {
                                crate::path_key::normalize_keep_drive(&source.join(n)) == path
                            })
                            .cloned()
                            .or_else(|| {
                                Path::new(&path)
                                    .strip_prefix(Path::new(
                                        &crate::path_key::normalize_keep_drive(source),
                                    ))
                                    .ok()
                                    .and_then(|p| p.to_str().map(str::to_owned))
                            })
                            .ok_or("resume image outside source folder")?;
                        crate::book_scope::validate_relative_reference(&relative)?;
                        crate::book_resume_db::PageIdentity::Archive {
                            container: crate::path_key::normalize_keep_drive(destination),
                            entry_name: relative,
                        }
                    }
                    _ => return Err("invalid image-folder resume identity".into()),
                };
                if let crate::book_resume_db::PageIdentity::Archive { entry_name, .. } = &remapped {
                    if let Some(index) = members.iter().position(|name| name == entry_name) {
                        replace_column(columns, row, "page", Value::Integer(index as i64))?;
                        if columns.iter().any(|c| c == "page_ordinal") {
                            let ordinal = columns.iter().position(|c| c == "page_ordinal").unwrap();
                            if row[ordinal] != Value::Null {
                                replace_column(
                                    columns,
                                    row,
                                    "page_ordinal",
                                    Value::Integer((index + 1) as i64),
                                )?;
                                replace_column(
                                    columns,
                                    row,
                                    "page_total",
                                    Value::Integer(members.len() as i64),
                                )?;
                            }
                        }
                    }
                }
                row[idx] =
                    Value::Text(serde_json::to_string(&remapped).map_err(|e| e.to_string())?);
            }
            // Unknown legacy order remains raw and identityless, rather than guessing/clamping.
            Ok(())
        },
    )
}

pub(crate) fn clone_history_on(
    conn: &Connection,
    source: &Path,
    destination: &Path,
    phase: CopyPhase,
) -> CopyReply {
    copy_rows_on(
        conn,
        "reading_history",
        "key",
        &crate::path_key::normalize_keep_drive(source),
        &crate::path_key::normalize_keep_drive(destination),
        phase,
        |columns, row| {
            replace_column(
                columns,
                row,
                "path",
                Value::Text(destination.to_string_lossy().into_owned()),
            )?;
            replace_column(columns, row, "kind", Value::Text("zip".into()))?;
            replace_column(columns, row, "archive_format", Value::Null)?;
            // Physical folder stat is not the newly created ZIP's stat.
            replace_column(columns, row, "file_size", Value::Null)?;
            replace_column(columns, row, "mtime_ms", Value::Null)
        },
    )
}

pub(crate) fn clone_bookmarks_on(
    conn: &Connection,
    source: &Path,
    destination: &Path,
    members: &[String],
    phase: CopyPhase,
) -> CopyReply {
    copy_rows_on(
        conn,
        "book_bookmarks",
        "container_key",
        &crate::path_key::normalize_keep_drive(source),
        &crate::path_key::normalize_keep_drive(destination),
        phase,
        |columns, row| {
            let read = |name: &str| -> Result<String, String> {
                let i = columns
                    .iter()
                    .position(|c| c == name)
                    .ok_or("bookmark column absent")?;
                match &row[i] {
                    Value::Text(s) => Ok(s.clone()),
                    _ => Err("invalid bookmark value".into()),
                }
            };
            if read("container_kind")? != "image_folder" || read("page_kind")? != "relative_path" {
                return Err("invalid image-folder bookmark".into());
            }
            let name = read("page_value")?;
            crate::book_scope::validate_relative_reference(&name)?;
            if let Some(index) = members
                .iter()
                .position(|member| member.eq_ignore_ascii_case(&name))
            {
                replace_column(
                    columns,
                    row,
                    "page_value",
                    Value::Text(members[index].clone()),
                )?;
                replace_column(
                    columns,
                    row,
                    "page_key",
                    Value::Text(crate::book_scope::normalize_relative_reference(
                        &members[index],
                    )?),
                )?;
                replace_column(
                    columns,
                    row,
                    "page_index_hint",
                    Value::Integer(index as i64),
                )?;
            }
            replace_column(columns, row, "id", Value::Null)?;
            replace_column(
                columns,
                row,
                "container_path",
                Value::Text(destination.to_string_lossy().into_owned()),
            )?;
            replace_column(columns, row, "container_kind", Value::Text("zip".into()))?;
            replace_column(
                columns,
                row,
                "page_kind",
                Value::Text("archive_entry".into()),
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (
        tempfile::TempDir,
        tempfile::TempDir,
        crate::image_folder_zip::ImageFolderManifest,
    ) {
        let data = tempfile::tempdir().unwrap();
        let files = tempfile::tempdir().unwrap();
        let source = files.path().join("book");
        std::fs::create_dir(&source).unwrap();
        for name in ["a.jpg", "b.jpg"] {
            std::fs::write(source.join(name), b"image bytes").unwrap();
        }
        let manifest = crate::image_folder_zip::prepare_manifest(
            &source,
            None,
            &crate::image_folder_zip::ConversionControl::default(),
        )
        .unwrap();
        (data, files, manifest)
    }

    #[test]
    fn folder_zip_tags_union_preserves_earliest_time_and_never_creates_page_tags() {
        let (data, _files, manifest) = fixture();
        let db = crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap();
        drop(db);
        let conn = Connection::open(data.path().join("tags.db")).unwrap();
        for (path, time) in [
            (&manifest.source, 30),
            (&manifest.members[0].path, 10),
            (&manifest.members[1].path, 20),
        ] {
            let key = crate::path_key::normalize_keep_drive(path);
            conn.execute(
                "INSERT INTO item_tags VALUES(?1,'same','same',?2)",
                params![key, time],
            )
            .unwrap();
            conn.execute("INSERT INTO tag_item_state VALUES(?1,1,'edit')", [key])
                .unwrap();
        }
        let plan = prepare_metadata(data.path(), &manifest, false, &HashMap::new()).unwrap();
        assert_eq!(plan.body_tags.as_ref().unwrap()[0].applied_at, 10);
        assert_eq!(plan.rows, 2);
        assert_eq!(apply_metadata(data.path(), &manifest, &plan).unwrap(), 2);
        let target = crate::path_key::normalize_keep_drive(&manifest.destination);
        assert_eq!(
            conn.query_row(
                "SELECT applied_at FROM item_tags WHERE item_key=?1",
                [target],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            10
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM item_tags", [], |r| r
                .get::<_, usize>(0))
                .unwrap(),
            4
        );
        for page in &manifest.members {
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM item_tags WHERE item_key=?1",
                    [crate::adjustment_db::zip_entry_key(
                        &manifest.destination,
                        &page.entry
                    )],
                    |r| r.get::<_, usize>(0)
                )
                .unwrap(),
                0
            );
        }
    }

    #[test]
    fn folder_zip_row_counts_include_supplemental_rating_and_empty_tag_decision_once() {
        let _scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let (data, _files, manifest) = fixture();
        let mut tags = crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap();
        tags.set_item_tags(
            &crate::path_key::normalize_keep_drive(&manifest.source),
            Vec::<String>::new(),
            crate::tags_db::source::EDIT,
        )
        .unwrap();
        let session = HashMap::from([
            (
                crate::path_key::normalize_keep_drive(&manifest.members[0].path),
                4,
            ),
            (
                crate::path_key::normalize_keep_drive(&manifest.members[1].path),
                0,
            ),
        ]);
        let plan = prepare_metadata(data.path(), &manifest, false, &session).unwrap();
        assert_eq!(plan.body_tags.as_deref(), Some([].as_slice()));
        assert_eq!(plan.rows, 2); // A nonzero rating and a durable empty tag decision.
        assert_eq!(apply_metadata(data.path(), &manifest, &plan).unwrap(), 2);
        let already = prepare_metadata(data.path(), &manifest, false, &session).unwrap();
        assert_eq!(already.rows, 0);
        assert_eq!(apply_metadata(data.path(), &manifest, &already).unwrap(), 0);
        let rating = crate::rating_db::RatingDb::open_at(data.path().join("rating.db")).unwrap();
        assert!(
            rating
                .row_for_key(&crate::path_key::normalize_keep_drive(
                    &manifest.members[0].path
                ))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            rating.get(&crate::adjustment_db::zip_entry_key(
                &manifest.destination,
                &manifest.members[0].entry
            )),
            4
        );
    }

    #[test]
    fn folder_zip_page_edit_copy_keeps_source_and_refuses_different_destination() {
        let (data, _files, manifest) = fixture();
        let db = crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
            .unwrap();
        let source = crate::path_key::normalize_keep_drive(&manifest.members[0].path);
        let target =
            crate::adjustment_db::zip_entry_key(&manifest.destination, &manifest.members[0].entry);
        let params = crate::adjustment::AdjustParams::default();
        db.set_page_params(&source, &params).unwrap();
        let plan = prepare_metadata(data.path(), &manifest, false, &HashMap::new()).unwrap();
        apply_metadata(data.path(), &manifest, &plan).unwrap();
        assert_eq!(db.get_page_params(&source), db.get_page_params(&target));
        let conn = Connection::open(data.path().join("adjustment.db")).unwrap();
        conn.execute(
            "UPDATE page_params SET params_json='{}' WHERE page_path=?1",
            [target],
        )
        .unwrap();
        assert!(prepare_metadata(data.path(), &manifest, false, &HashMap::new()).is_err());
        assert_eq!(db.get_page_params(&source), Some(params));
    }

    #[test]
    fn folder_zip_db_rating_metadata_and_timestamp_use_zip_page_consumer() {
        let (data, _files, manifest) = fixture();
        let db = crate::rating_db::RatingDb::open_at(data.path().join("rating.db")).unwrap();
        let source = crate::path_key::normalize_keep_drive(&manifest.members[0].path);
        db.set(&source, 4).unwrap();
        let old = db.row_for_key(&source).unwrap().unwrap();
        let plan = prepare_metadata(data.path(), &manifest, false, &HashMap::new()).unwrap();
        apply_metadata(data.path(), &manifest, &plan).unwrap();
        let target =
            crate::adjustment_db::zip_entry_key(&manifest.destination, &manifest.members[0].entry);
        let row = db.row_for_key(&target).unwrap().unwrap();
        assert_eq!(row.kind, Some(crate::rating_db::RatingItemKind::ZipImage));
        assert_eq!(
            row.source_path,
            Some(manifest.destination.to_string_lossy().into_owned())
        );
        assert_eq!(row.entry_name, Some(manifest.members[0].entry.clone()));
        assert_eq!(row.rated_at_ms, old.rated_at_ms);
        assert_eq!(db.get(&source), 4);
    }

    #[test]
    fn folder_zip_resume_missing_and_legacy_values_are_kept_as_typed_archive_or_raw() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE book_resume(path TEXT PRIMARY KEY,page INTEGER,page_identity TEXT,meter_ordinal INTEGER,meter_total INTEGER);").unwrap();
        let source = Path::new("C:/books/folder");
        let target = Path::new("C:/books/folder.zip");
        let identity = crate::book_resume_db::PageIdentity::Image {
            path: "c:/books/folder/missing.jpg".into(),
        };
        conn.execute(
            "INSERT INTO book_resume VALUES(?1,77,?2,78,100)",
            params![
                crate::path_key::normalize(source),
                serde_json::to_string(&identity).unwrap()
            ],
        )
        .unwrap();
        clone_resume_on(&conn, source, target, &[], None, CopyPhase::Apply).unwrap();
        let encoded = conn
            .query_row(
                "SELECT page_identity FROM book_resume WHERE path=?1",
                [crate::path_key::normalize(target)],
                |r| r.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<crate::book_resume_db::PageIdentity>(&encoded).unwrap(),
            crate::book_resume_db::PageIdentity::Archive {
                container: "c:/books/folder.zip".into(),
                entry_name: "missing.jpg".into()
            }
        );
        conn.execute(
            "DELETE FROM book_resume WHERE path=?1",
            [crate::path_key::normalize(target)],
        )
        .unwrap();
        conn.execute(
            "UPDATE book_resume SET page=999,page_identity=NULL WHERE path=?1",
            [crate::path_key::normalize(source)],
        )
        .unwrap();
        clone_resume_on(&conn, source, target, &[], None, CopyPhase::Apply).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT page FROM book_resume WHERE path=?1",
                [crate::path_key::normalize(target)],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            999
        );
    }
    #[test]
    fn copy_preflight_conflict_and_equal_noop_keep_source_and_unrelated_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE data(path TEXT PRIMARY KEY, value TEXT); INSERT INTO data VALUES('a','edit'),('sibling','other');").unwrap();
        assert_eq!(
            copy_rows_on(
                &conn,
                "data",
                "path",
                "a",
                "zip",
                CopyPhase::Check,
                |_, _| Ok(())
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM data", [], |r| r.get::<_, usize>(0))
                .unwrap(),
            2
        );
        copy_rows_on(
            &conn,
            "data",
            "path",
            "a",
            "zip",
            CopyPhase::Apply,
            |_, _| Ok(()),
        )
        .unwrap();
        assert_eq!(
            copy_rows_on(
                &conn,
                "data",
                "path",
                "a",
                "zip",
                CopyPhase::Apply,
                |_, _| Ok(())
            )
            .unwrap(),
            0
        );
        conn.execute("UPDATE data SET value='old' WHERE path='zip'", [])
            .unwrap();
        assert!(
            copy_rows_on(
                &conn,
                "data",
                "path",
                "a",
                "zip",
                CopyPhase::Apply,
                |_, _| Ok(())
            )
            .is_err()
        );
        assert!(
            copy_rows_on(
                &conn,
                "data",
                "path",
                "absent",
                "zip",
                CopyPhase::Apply,
                |_, _| Ok(())
            )
            .is_err()
        );
        assert_eq!(
            conn.query_row("SELECT value FROM data WHERE path='a'", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "edit"
        );
    }
}
