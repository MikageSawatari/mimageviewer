use super::*;

#[cfg(test)]
thread_local! {
    pub(super) static CACHE_ROOT_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Debug, Default)]
pub(crate) struct ConvertedArchiveEnumeration {
    pub(crate) converted_archive_rels: Vec<PathBuf>,
    pub(crate) incomplete: bool,
}

fn candidate(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .and_then(crate::archive_converter::ArchiveFormat::nested_from_extension)
        .is_some()
}

/// Worker-only FS snapshot. Never consult the stores: prior queued renames or
/// writes may not have committed their rows yet.
pub(crate) fn collect_converted_archive_rels(new_root: &Path) -> ConvertedArchiveEnumeration {
    let started = std::time::Instant::now();
    let mut result = ConvertedArchiveEnumeration::default();
    // Do not follow a renamed link even when the caller classified its target
    // as a directory before the rename.
    let mut pending = match std::fs::symlink_metadata(new_root) {
        Ok(metadata) if metadata.file_type().is_symlink() => Vec::new(),
        Ok(_) => vec![new_root.to_path_buf()],
        Err(error) => {
            enumeration_error(&mut result, new_root, &error);
            Vec::new()
        }
    };
    while let Some(directory) = pending.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                enumeration_error(&mut result, &directory, &error);
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    enumeration_error(&mut result, &directory, &error);
                    continue;
                }
            };
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(error) => {
                    enumeration_error(&mut result, &entry.path(), &error);
                    continue;
                }
            };
            // Windows junctions (mount-point reparse tags) are also symlinks
            // in std::fs::FileType; neither they nor symbolic links are followed.
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() && candidate(&path) {
                result.converted_archive_rels.push(
                    path.strip_prefix(new_root)
                        .expect("read_dir descendants stay below the root")
                        .to_path_buf(),
                );
            }
        }
    }
    result.converted_archive_rels.sort();
    crate::perf::event(
        "rename",
        "converted_archive_enumeration",
        None,
        0,
        &[
            (
                "count",
                serde_json::json!(result.converted_archive_rels.len()),
            ),
            (
                "elapsed_ms",
                serde_json::json!(started.elapsed().as_millis()),
            ),
            ("incomplete", serde_json::json!(result.incomplete)),
        ],
    );
    result
}

fn enumeration_error(
    result: &mut ConvertedArchiveEnumeration,
    path: &Path,
    error: &std::io::Error,
) {
    result.incomplete = true;
    crate::logger::log(format!(
        "rename converted archive enumeration: {}: {error}",
        path.display()
    ));
}

/// Pure prediction, safe on the UI thread. Journal input is lexical relative
/// paths only; do not allow malformed persisted input to escape the renamed tree.
pub(crate) fn converted_archive_cache_pairs(
    data_dir: &Path,
    old: &Path,
    new: &Path,
    tree: bool,
    rels: &[PathBuf],
) -> Vec<(PathBuf, PathBuf)> {
    let sources = if tree {
        rels.iter()
            .filter(|rel| {
                !rel.as_os_str().is_empty()
                    && rel
                        .components()
                        .all(|part| matches!(part, std::path::Component::Normal(_)))
                    && candidate(rel)
            })
            .map(|rel| (old.join(rel), new.join(rel)))
            .collect::<Vec<_>>()
    } else if candidate(old) {
        vec![(old.to_path_buf(), new.to_path_buf())]
    } else {
        Vec::new()
    };
    sources
        .into_iter()
        .map(|(old, new)| {
            (
                crate::archive_cache::cache_zip_path_for_data_dir(data_dir, &old),
                crate::archive_cache::cache_zip_path_for_data_dir(data_dir, &new),
            )
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Read the cache root once per descriptor transaction. Extract the fixed
/// h2/hash/filename container before either slash descendants or virtual pages.
fn cache_container(key: &str, root_prefix: &str) -> Option<String> {
    let suffix = key.strip_prefix(root_prefix)?.split("::").next()?;
    let parts = suffix.split('/').take(3).collect::<Vec<_>>();
    (parts.len() == 3).then(|| format!("{root_prefix}{}", parts.join("/")))
}

pub(super) fn cache_roots_in_store(
    tx: &rusqlite::Transaction<'_>,
    data_dir: &Path,
    descriptor: &StoreDescriptor,
    resume_identity: bool,
) -> rusqlite::Result<BTreeSet<String>> {
    let prefix = format!(
        "{}/",
        descriptor.normalize_path(&data_dir.join("archive_cache"))
    );
    let upper = prefix_upper_bound(&prefix).expect("nonempty cache root");
    let columns = if resume_identity {
        format!("{}, page_identity", descriptor.column)
    } else {
        descriptor.column.to_owned()
    };
    let mut statement = tx.prepare(&format!(
        "SELECT {columns} FROM {} WHERE {} >= ?1 AND {} < ?2",
        descriptor.table, descriptor.column, descriptor.column,
    ))?;
    #[cfg(test)]
    CACHE_ROOT_READS.with(|reads| reads.set(reads.get() + 1));
    let mut rows = statement.query(rusqlite::params![prefix, upper])?;
    let mut roots = BTreeSet::new();
    while let Some(row) = rows.next()? {
        let key: String = row.get(0)?;
        if let Some(root) = cache_container(&key, &prefix) {
            roots.insert(root);
        }
        if resume_identity {
            let json: Option<String> = row.get(1)?;
            if let Some(identity) = json.and_then(|json| {
                serde_json::from_str::<crate::book_resume_db::PageIdentity>(&json).ok()
            }) {
                use crate::book_resume_db::PageIdentity;
                let path = match identity {
                    PageIdentity::Image { path } => path,
                    PageIdentity::Archive { container, .. }
                    | PageIdentity::Document { container, .. } => container,
                };
                let key = descriptor.normalize_path(Path::new(&path));
                if let Some(root) = cache_container(&key, &prefix) {
                    roots.insert(root);
                }
            }
        }
    }
    Ok(roots)
}

pub(super) fn record_changed_pair(report: &mut RenameMigrationReport, pair: &(PathBuf, PathBuf)) {
    // The operation owner deduplicates once after all stores commit.
    report.converted_cache_pairs.push(pair.clone());
}
