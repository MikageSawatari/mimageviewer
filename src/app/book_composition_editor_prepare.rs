//! Worker-only full-source preparation and save-time source validation.
//! No application state, settings writes, shift writes or UI calls live here.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::SystemTime,
};

use super::{book_composition_runtime::BookCompositionSource, book_materialization::*};
use crate::{
    book_composition::*,
    book_scope::BookScopeKey,
    file_type_visibility::FileTypeVisibility,
    grid_item::GridItem,
    rotation_db::Rotation,
    settings::{GridDisplayOrder, SortOrder},
    spread_shift::{ResolvedAutomaticRules, SpreadShiftPageRef},
};

#[derive(Clone)]
pub(crate) struct EditorSourceInput {
    pub(crate) scope: BookScopeKey,
    pub(crate) domain: PageDomain,
    pub(crate) source: BookCompositionSource,
    pub(crate) policy: FileTypeVisibility,
    pub(crate) placement: GridDisplayOrder,
    pub(crate) sort: SortOrder,
    /// Captured resolved rules and live shift, never a second saved-shift read.
    pub(crate) rules: ResolvedAutomaticRules,
    pub(crate) shift: Option<SpreadShiftPageRef>,
    /// Current accepted cached page rotations override durable worker reads.
    pub(crate) rotations: HashMap<CompositionPageRef, Rotation>,
    pub(crate) data_dir: PathBuf,
    pub(crate) password: Option<String>,
    /// Single/Split viewing mode does not own the book's pairing binding.
    /// Absent for an accepted Spread capture and for manual composition edits.
    pub(crate) initial_binding: Option<InitialEditorBinding>,
    /// Reuse the existing process RAW scheduler for editor thumbnail work.
    pub(crate) raw_executor: std::sync::Arc<crate::raw::RawDevelopExecutor>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct InitialEditorBinding {
    pub(crate) default_mode: crate::settings::SpreadMode,
    pub(crate) default_direction: crate::settings::ReadingDirection,
    pub(crate) follow_document_direction: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedEditorSource {
    pub(crate) draft: CompositionDraft,
    pub(crate) raw_pages: Vec<CanonicalRawPage>,
    /// The full policy-admitted source rows, independent of facets and hidden.
    pub(crate) rows: MaterializedBookRows,
    pub(crate) fingerprint: SourceFingerprint,
    pub(crate) result_source: std::sync::Arc<EditorResultSource>,
    pub(crate) result: EditorResult,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileWitness {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceFingerprint {
    files: Vec<FileWitness>,
    raw_pages: Vec<CompositionPageRef>,
    document_stamp: Option<crate::pdf_loader::DocumentStamp>,
}

const SOURCE_CHANGED: &str = "ページが変更されています。読み直してください。";

impl std::fmt::Debug for EditorSourceInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorSourceInput")
            .field("scope", &self.scope)
            .field("domain", &self.domain)
            .field("source", &self.source)
            .field("policy", &self.policy)
            .field("placement", &self.placement)
            .field("sort", &self.sort)
            .field("rules", &self.rules)
            .field("shift", &self.shift)
            .field("rotations", &self.rotations)
            .field("data_dir", &self.data_dir)
            .field("has_password", &self.password.is_some())
            .field("initial_binding", &self.initial_binding)
            .finish()
    }
}

fn source_files(input: &EditorSourceInput) -> Result<Vec<FileWitness>, String> {
    let physical = match &input.source {
        BookCompositionSource::Archive { tree, .. } => tree.zip_path.as_path(),
        BookCompositionSource::Document {
            physical_source,
            read_target,
            ..
        } => read_target
            .as_ref()
            .map_or(physical_source.as_path(), |read| read.read_path.as_path()),
        BookCompositionSource::Unavailable => return Err("本の元ページを取得できません".into()),
    };
    let mut paths = vec![input.scope.logical_root.clone()];
    if crate::path_key::normalize_keep_drive(physical)
        != crate::path_key::normalize_keep_drive(&paths[0])
    {
        paths.push(physical.to_owned());
    }
    paths
        .into_iter()
        .map(|path| {
            let metadata = std::fs::metadata(&path)
                .map_err(|error| format!("元ページを確認できません: {error}"))?;
            if !metadata.is_file() {
                return Err("本の元ファイルがありません".into());
            }
            Ok(FileWitness {
                path,
                size: metadata.len(),
                modified: metadata.modified().map_err(|error| error.to_string())?,
            })
        })
        .collect()
}

fn page_for_item(item: &GridItem) -> Option<CompositionPageRef> {
    match item {
        GridItem::ZipImage { entry_name, .. } => Some(CompositionPageRef::ArchiveEntry {
            entry_name: entry_name.clone(),
        }),
        GridItem::PdfPage { page_num, .. } => Some(CompositionPageRef::PdfPage {
            page_num: *page_num,
        }),
        _ => None,
    }
}

fn source_rows(
    input: &EditorSourceInput,
) -> Result<(Vec<CanonicalRawPage>, Vec<ManifestPage>, PageRows), String> {
    crate::book_composition::store::validate_scope_domain(&input.scope, &input.domain)?;
    let (raw_items, rows) = match &input.source {
        BookCompositionSource::Archive { tree, prefix, .. } => {
            if BookScopeKey::archive(&input.scope.logical_root, prefix)? != input.scope {
                return Err("本の元階層が一致しません".into());
            }
            let raw = tree.raw_source();
            let projected = raw.project(&input.policy);
            (
                materialize_archive_level(raw, prefix, input.sort, Some(&input.placement)).0,
                materialize_archive_level(&projected, prefix, input.sort, Some(&input.placement)),
            )
        }
        BookCompositionSource::Document {
            count,
            metadata,
            physical_source,
            ..
        } => {
            if *count as usize > MAX_PAGES {
                return Err("本のページ数が上限を超えています".into());
            }
            let (mtime, size) = metadata.unwrap_or_default();
            let (items, metas, _) = materialize_cached_document_rows(
                physical_source,
                *count,
                mtime,
                size.max(0) as u64,
            );
            let rows = if input.policy.allows_physical_file(&input.scope.logical_root) {
                (items.clone(), metas)
            } else {
                (vec![], vec![])
            };
            (items, rows)
        }
        BookCompositionSource::Unavailable => return Err("本の元ページを取得できません".into()),
    };
    let present: HashMap<_, _> = rows
        .0
        .iter()
        .enumerate()
        .filter_map(|(index, item)| page_for_item(item).map(|page| (page, index)))
        .collect();
    let raw_pages: Vec<_> = raw_items
        .iter()
        .enumerate()
        .filter_map(|(raw_index, item)| {
            page_for_item(item).map(|page| CanonicalRawPage { page, raw_index })
        })
        .collect();
    let manifest = raw_pages
        .iter()
        .map(|raw| ManifestPage {
            page: raw.page.clone(),
            availability: present.get(&raw.page).map_or(
                ManifestAvailability::ExcludedByPolicy,
                |item_index| ManifestAvailability::Present {
                    item_index: *item_index,
                },
            ),
        })
        .collect();
    Ok((raw_pages, manifest, rows))
}

/// G3's common projection adapters provide save landing/resume projections.
/// Its synthetic token is for a private worker calculation, never a live adoption.
pub(crate) fn project_record(
    input: &EditorSourceInput,
    record: Option<&BookComposition>,
) -> Result<MaterializedBookRows, String> {
    let (_, _, rows) = source_rows(input)?;
    project_source_rows(input, record, rows)
}

fn project_source_rows(
    input: &EditorSourceInput,
    record: Option<&BookComposition>,
    rows: PageRows,
) -> Result<MaterializedBookRows, String> {
    let token = CompositionToken {
        service_epoch: 0,
        publication_sequence: 0,
    };
    let result = match &input.source {
        BookCompositionSource::Archive { tree, prefix, .. } => project_archive_composition(
            tree,
            &input.scope.logical_root,
            prefix,
            rows,
            input.sort,
            Some(&input.placement),
            input.policy.clone(),
            token,
            record,
        ),
        BookCompositionSource::Document { count, .. } => project_document_composition(
            &input.scope.logical_root,
            *count,
            rows,
            input.policy.clone(),
            token,
            record,
        ),
        BookCompositionSource::Unavailable => return Err("本の元ページを取得できません".into()),
    }
    .map_err(|error| error.to_string())?;
    match &result.composition {
        CompositionProjectionOutcome::Unsupported { reason }
        | CompositionProjectionOutcome::Unreadable { reason } => Err(reason.clone()),
        _ => Ok(result),
    }
}

pub(crate) fn prepare(
    input: &EditorSourceInput,
    saved: Option<&BookComposition>,
) -> Result<PreparedEditorSource, String> {
    let files = source_files(input)?;
    let (raw_pages, manifest, source_rows) = source_rows(input)?;
    // The adapter refuses unsafe/ambiguous references without modifying legacy rows.
    let rows = project_source_rows(input, None, source_rows)?;
    let mut resolved_input = input.clone();
    if saved.is_none()
        && let Some(initial) = input.initial_binding
    {
        let binding = read_initial_binding(input, initial)?;
        resolved_input.rules.mode = binding.mode;
        resolved_input.rules.direction = binding.direction;
    }
    let record = saved.cloned().unwrap_or_else(|| {
        let direction = match resolved_input.rules.direction {
            crate::settings::ReadingDirection::Ltr => ReadingDirection::Ltr,
            crate::settings::ReadingDirection::Rtl => ReadingDirection::Rtl,
        };
        let mut record = BookComposition::new(
            direction,
            raw_pages.iter().map(|r| r.page.clone()).collect(),
        );
        record.cover_enabled = resolved_input.rules.mode.has_cover();
        record
    });
    let draft = CompositionDraft::new(record, input.domain.clone(), manifest)
        .map_err(|error| error.to_string())?;
    let result_source = std::sync::Arc::new(EditorResultSource {
        landscape: read_landscape(input, &rows.items)?,
        items: rows.items.clone(),
        manifest: draft.manifest().to_vec(),
        domain: input.domain.clone(),
        rules: resolved_input.rules,
    });
    let result = derive_result(&result_source, draft.record(), draft.generation())?;
    let document_stamp = match &input.source {
        BookCompositionSource::Document { read_target, .. } => {
            read_target.as_ref().map(|read| read.stamp.clone())
        }
        _ => None,
    };
    let fingerprint = SourceFingerprint {
        files,
        raw_pages: raw_pages.iter().map(|raw| raw.page.clone()).collect(),
        document_stamp,
    };
    // Reject a source changed before/during preparation instead of presenting a
    // stale draft with a new fingerprint that could later approve its save.
    verify_source(input, &fingerprint)?;
    Ok(PreparedEditorSource {
        draft,
        raw_pages,
        rows,
        fingerprint,
        result_source,
        result,
    })
}

fn read_initial_binding(
    input: &EditorSourceInput,
    initial: InitialEditorBinding,
) -> Result<crate::spread_db::ResolvedSpreadReading, String> {
    // C1: the existing PC base-spread key remains physical for converted
    // archives. Composition persistence continues to use the logical scope.
    let key = match &input.source {
        BookCompositionSource::Archive {
            tree,
            display_prefix,
            ..
        } => crate::spread_db::container_key_with_fallback(&tree.zip_path, display_prefix),
        _ => crate::spread_db::container_key_with_fallback(&input.scope.logical_root, &[]),
    };
    let stored =
        crate::spread_db::SpreadDb::open_existing_read_only_at(&input.data_dir.join("spread.db"))
            .map_err(|error| error.to_string())?
            .map(|db| db.get_state_with_fallback(&key.exact, key.fallback.as_deref()))
            .unwrap_or_default();
    let document_direction = if stored.needs_document_direction(initial.follow_document_direction) {
        match &input.source {
            BookCompositionSource::Document {
                read_target,
                physical_source,
                ..
            } => {
                let read = match read_target {
                    Some(read) => read.clone(),
                    None => crate::pdf_loader::resolve_read_target(physical_source)
                        .map_err(|error| error.to_string())?,
                };
                if let Some(direction) = read.epub_direction {
                    Some(direction)
                } else {
                    crate::pdf_loader::enumerate_pages_with_read_target(
                        &input.scope.logical_root,
                        &read,
                        input.password.as_deref(),
                        None,
                        crate::pdf_loader::EnumerateOptions {
                            want_direction: true,
                        },
                    )
                    .map_err(|error| error.to_string())?
                    .direction
                }
            }
            _ => None,
        }
    } else {
        None
    };
    Ok(resolve_initial_binding(stored, initial, document_direction))
}

fn resolve_initial_binding(
    mut stored: crate::spread_db::StoredSpreadState,
    initial: InitialEditorBinding,
    document_direction: Option<crate::pdf_loader::PdfReadingDirection>,
) -> crate::spread_db::ResolvedSpreadReading {
    // A mode-only Single/Split row must not erase its underlying cover binding.
    stored.view_mode = None;
    let mut resolved = stored.resolve_reading(
        initial.default_mode,
        initial.default_direction,
        initial.follow_document_direction,
        document_direction,
    );
    if !resolved.mode.is_spread() {
        resolved.mode = crate::settings::SpreadMode::Ltr.with_reading_direction(resolved.direction);
    }
    resolved
}

pub(crate) fn verify_source(
    input: &EditorSourceInput,
    fingerprint: &SourceFingerprint,
) -> Result<(), String> {
    if source_files(input)? != fingerprint.files {
        return Err(SOURCE_CHANGED.into());
    }
    let actual = match &input.source {
        BookCompositionSource::Archive { tree, prefix, .. } => {
            // Re-enumerate through the existing read-only loader, not the retained
            // display tree. Converted cache origins retain their existing adapter.
            let enumeration = if crate::path_key::normalize_keep_drive(&tree.zip_path)
                != crate::path_key::normalize_keep_drive(&input.scope.logical_root)
            {
                crate::zip_loader::enumerate_converted_image_entries_detailed_readonly_with_cancel(
                    &tree.zip_path,
                    None,
                )
            } else {
                crate::zip_loader::enumerate_image_entries_detailed_readonly_with_cancel(
                    &tree.zip_path,
                    None,
                )
            }
            .map_err(|error| error.to_string())?;
            let fresh = crate::zip_tree::ZipTree::build(tree.zip_path.clone(), enumeration.entries);
            materialize_archive_level(&fresh, prefix, input.sort, Some(&input.placement))
                .0
                .iter()
                .filter_map(page_for_item)
                .collect::<Vec<_>>()
        }
        BookCompositionSource::Document {
            count,
            physical_source,
            read_target,
            ..
        } => {
            let current_stamp = read_target.as_ref().map(|read| read.stamp.clone());
            if current_stamp != fingerprint.document_stamp {
                return Err(SOURCE_CHANGED.into());
            }
            let read = match read_target {
                Some(read) => read.clone(),
                None => crate::pdf_loader::resolve_read_target(physical_source)
                    .map_err(|error| error.to_string())?,
            };
            let enumerated = crate::pdf_loader::enumerate_pages_with_read_target(
                &input.scope.logical_root,
                &read,
                input.password.as_deref(),
                None,
                crate::pdf_loader::EnumerateOptions::default(),
            )
            .map_err(|error| error.to_string())?;
            if enumerated.pages.len() != *count as usize {
                return Err(SOURCE_CHANGED.into());
            }
            (0..*count)
                .map(|page_num| CompositionPageRef::PdfPage { page_num })
                .collect()
        }
        BookCompositionSource::Unavailable => return Err(SOURCE_CHANGED.into()),
    };
    if actual != fingerprint.raw_pages || source_files(input)? != fingerprint.files {
        return Err(SOURCE_CHANGED.into());
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub(crate) struct EditorResultSource {
    items: Vec<GridItem>,
    landscape: Vec<bool>,
    manifest: Vec<ManifestPage>,
    domain: PageDomain,
    rules: ResolvedAutomaticRules,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EditorResult {
    pub(crate) generation: u64,
    pub(crate) cover: Option<CompositionPageRef>,
    pub(crate) groups: Vec<DerivedEditorGroup>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DerivedEditorGroup {
    Pair(CompositionPageRef, CompositionPageRef),
    Single {
        page: CompositionPageRef,
        placement: SinglePlacement,
        companion: DerivedCompanion,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DerivedCompanion {
    Vacant,
    Blank,
    Cover(CompositionPageRef),
}

pub(crate) fn derive_result(
    source: &EditorResultSource,
    record: &BookComposition,
    generation: u64,
) -> Result<EditorResult, String> {
    use crate::{
        displayed_image_transform::SingletonSpreadPlacement as Side, ui_fullscreen::SpreadPageRole,
    };
    let resolved =
        resolve_composition(record, &source.domain, &source.manifest).map_err(|e| e.to_string())?;
    let indices: HashMap<_, _> = source
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| page_for_item(item).map(|page| (page.matching_key(), index)))
        .collect();
    let order: Vec<_> = resolved
        .visible_pages
        .iter()
        .filter_map(|page| indices.get(&page.page.matching_key()).copied())
        .collect();
    let items: Vec<_> = order
        .iter()
        .map(|&index| source.items[index].clone())
        .collect();
    let landscape: Vec<_> = order.iter().map(|&index| source.landscape[index]).collect();
    let mode = match (record.direction, record.cover_enabled) {
        (ReadingDirection::Ltr, false) => crate::settings::SpreadMode::Ltr,
        (ReadingDirection::Ltr, true) => crate::settings::SpreadMode::LtrCover,
        (ReadingDirection::Rtl, false) => crate::settings::SpreadMode::Rtl,
        (ReadingDirection::Rtl, true) => crate::settings::SpreadMode::RtlCover,
    };
    let overrides = order
        .iter()
        .enumerate()
        .filter_map(|(index, original)| {
            resolved
                .page_overrides
                .get(original)
                .copied()
                .map(|directive| (index, directive))
        })
        .collect();
    let groups = crate::ui_fullscreen::build_remote_spread_page_groups_with_overrides(
        &items,
        mode,
        &landscape,
        source.rules.final_cover,
        source.rules.singleton_endpoints,
        source.rules.page_alone,
        true,
        None,
        &overrides,
    );
    let page_at = |index| {
        items
            .get(index)
            .and_then(page_for_item)
            .ok_or_else(|| "結果の実ページがありません".to_string())
    };
    let mut output = Vec::with_capacity(groups.len());
    for group in groups {
        let mut navigation = group.indices;
        if mode.is_rtl() && navigation.len() == 2 {
            navigation.reverse();
        }
        match navigation.as_slice() {
            [first, second] => output.push(DerivedEditorGroup::Pair(
                page_at(*first)?,
                page_at(*second)?,
            )),
            [index] => {
                let supplement = group.presentation.as_ref().and_then(|pages| {
                    pages
                        .iter()
                        .find(|page| page.role == SpreadPageRole::FrontCoverSupplement)
                });
                let side = if supplement.is_some() {
                    match group.presentation.as_ref().and_then(|pages| {
                        pages
                            .iter()
                            .position(|page| page.role == SpreadPageRole::Navigation)
                    }) {
                        Some(0) => Side::Left,
                        Some(1) => Side::Right,
                        _ => return Err("結果の表紙側が不正です".into()),
                    }
                } else {
                    group.singleton_placement.side()
                };
                let placement = match (side, record.direction) {
                    (Side::Center, _) => SinglePlacement::Center,
                    (Side::Left, ReadingDirection::Ltr) | (Side::Right, ReadingDirection::Rtl) => {
                        SinglePlacement::Leading
                    }
                    _ => SinglePlacement::Trailing,
                };
                let companion = if let Some(supplement) = supplement {
                    DerivedCompanion::Cover(page_at(supplement.idx)?)
                } else if group.singleton_placement.has_white_companion() {
                    DerivedCompanion::Blank
                } else {
                    DerivedCompanion::Vacant
                };
                output.push(DerivedEditorGroup::Single {
                    page: page_at(*index)?,
                    placement,
                    companion,
                });
            }
            _ => return Err("結果のページ数が不正です".into()),
        }
    }
    Ok(EditorResult {
        generation,
        cover: record
            .cover_enabled
            .then(|| items.first().and_then(page_for_item))
            .flatten(),
        groups: output,
    })
}

/// One session owns one mailbox. Superseded unsent drafts are replaced, and the
/// receiver accepts only a result whose generation still matches its draft.
pub(crate) struct EditorResultWorker {
    latest: std::sync::Arc<std::sync::Mutex<Option<(u64, BookComposition)>>>,
    wake: std::sync::mpsc::SyncSender<()>,
    pub(crate) receiver: std::sync::mpsc::Receiver<(u64, Result<EditorResult, String>)>,
}
impl EditorResultWorker {
    pub(crate) fn spawn(
        source: std::sync::Arc<EditorResultSource>,
        context: egui::Context,
    ) -> Result<Self, String> {
        let latest = std::sync::Arc::new(std::sync::Mutex::new(None::<(u64, BookComposition)>));
        let mailbox = latest.clone();
        let (wake, requests) = std::sync::mpsc::sync_channel(1);
        let (results, receiver) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("composition-editor-results".into())
            .spawn(move || {
                while requests.recv().is_ok() {
                    let request = mailbox.lock().unwrap().take();
                    if let Some((generation, record)) = request {
                        let result = derive_result(&source, &record, generation);
                        if results.send((generation, result)).is_err() {
                            break;
                        }
                        context.request_repaint_of(egui::ViewportId::ROOT);
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            latest,
            wake,
            receiver,
        })
    }
    pub(crate) fn request(&self, generation: u64, record: BookComposition) {
        *self.latest.lock().unwrap() = Some((generation, record));
        let _ = self.wake.try_send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, sync::Arc};

    #[cfg(windows)]
    #[test]
    fn avif_archive_landscape_probe_reads_complete_container_beyond_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let input = archive_input(temp.path(), &[("page.avif", 64, 32)]);
        let mut bytes = crate::avif_decoder::tests::with_clap_and_mirror(true);
        let iloc = bytes.windows(4).position(|b| b == b"iloc").unwrap() - 4;
        let mdat = bytes.windows(4).position(|b| b == b"mdat").unwrap() - 4;
        let padding = 70 * 1024u32;
        let offset = u32::from_be_bytes(bytes[iloc + 22..iloc + 26].try_into().unwrap());
        bytes[iloc + 22..iloc + 26].copy_from_slice(&(offset + padding).to_be_bytes());
        let mut free = vec![0; padding as usize];
        free[..4].copy_from_slice(&padding.to_be_bytes());
        free[4..8].copy_from_slice(b"free");
        bytes.splice(mdat..mdat, free);
        let zip_path = temp.path().join("book.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        writer
            .start_file("page.avif", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&bytes).unwrap();
        writer.finish().unwrap();
        let items = [GridItem::ZipImage {
            zip_path,
            entry_name: "page.avif".into(),
        }];
        assert_eq!(read_landscape(&input, &items).unwrap(), [true]);
    }

    fn archive_input(root: &Path, entries: &[(&str, u32, u32)]) -> EditorSourceInput {
        let path = root.join("book.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, width, height) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            let image = image::RgbaImage::new(*width, *height);
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image)
                .write_to(&mut bytes, image::ImageFormat::Png)
                .unwrap();
            zip.write_all(bytes.get_ref()).unwrap();
        }
        zip.finish().unwrap();
        let enumeration =
            crate::zip_loader::enumerate_image_entries_detailed_readonly_with_cancel(&path, None)
                .unwrap();
        let tree = Arc::new(crate::zip_tree::ZipTree::build(
            path.clone(),
            enumeration.entries,
        ));
        source_input(
            root,
            BookScopeKey::archive(&path, &[]).unwrap(),
            PageDomain::ArchiveLevel {
                prefix: String::new(),
            },
            BookCompositionSource::Archive {
                tree,
                prefix: vec![],
                display_prefix: vec![],
            },
        )
    }

    fn source_input(
        root: &Path,
        scope: BookScopeKey,
        domain: PageDomain,
        source: BookCompositionSource,
    ) -> EditorSourceInput {
        EditorSourceInput {
            scope,
            domain,
            source,
            policy: FileTypeVisibility::default(),
            placement: GridDisplayOrder::default(),
            sort: SortOrder::FileName,
            rules: ResolvedAutomaticRules {
                mode: crate::settings::SpreadMode::Ltr,
                direction: crate::settings::ReadingDirection::Ltr,
                flow: crate::settings::ReadingFlow::Paged,
                final_cover: false,
                page_alone: Default::default(),
                singleton_endpoints: Default::default(),
                landscape_epoch: 0,
            },
            shift: None,
            rotations: HashMap::new(),
            data_dir: root.to_owned(),
            password: None,
            initial_binding: None,
            raw_executor: std::sync::Arc::new(crate::raw::RawDevelopExecutor::new(1).unwrap()),
        }
    }

    fn page(name: &str) -> CompositionPageRef {
        CompositionPageRef::ArchiveEntry {
            entry_name: name.into(),
        }
    }

    fn pdf_page(page_num: u32) -> CompositionPageRef {
        CompositionPageRef::PdfPage { page_num }
    }

    /// Actual page objects/xref consumed by the existing in-process PDF worker
    /// test backend. This does not fake dimensions or enumeration responses.
    fn write_document_pdf(path: &Path, sizes: &[(u32, u32)], rtl: bool) {
        let preferences = if rtl {
            " /ViewerPreferences << /Direction /R2L >>"
        } else {
            ""
        };
        let kids = (0..sizes.len())
            .map(|index| format!("{} 0 R", index + 3))
            .collect::<Vec<_>>()
            .join(" ");
        let mut objects = vec![
            format!("<< /Type /Catalog /Pages 2 0 R{preferences} >>"),
            format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", sizes.len()),
        ];
        objects.extend(sizes.iter().map(|(width, height)| {
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] >>")
        }));
        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Root 1 0 R /Size {} >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        std::fs::write(path, bytes).unwrap();
    }

    fn document_input(root: &Path, logical: &Path, count: u32) -> EditorSourceInput {
        let read = crate::pdf_loader::resolve_read_target(logical).unwrap();
        source_input(
            root,
            BookScopeKey::document(logical).unwrap(),
            PageDomain::Document,
            BookCompositionSource::Document {
                count,
                metadata: None,
                physical_source: logical.to_owned(),
                read_target: Some(read),
            },
        )
    }

    fn write_document_epub(path: &Path) {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let entries = [
            ("mimetype", "application/epub+zip"),
            (
                "META-INF/container.xml",
                r#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="book.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            ),
            (
                "book.opf",
                r#"<?xml version="1.0"?><package version="3.0" unique-identifier="id" xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">editor-fixture</dc:identifier><dc:title>Editor fixture</dc:title><dc:language>ja</dc:language></metadata><manifest><item id="page" href="page.xhtml" media-type="application/xhtml+xml"/></manifest><spine page-progression-direction="rtl"><itemref idref="page"/></spine></package>"#,
            ),
            (
                "page.xhtml",
                r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>Editor fixture</title></head><body><p>Fixture</p></body></html>"#,
            ),
        ];
        for (name, body) in entries {
            zip.start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn pdf_worker_reads_real_page_boxes_and_document_direction_for_initial_binding() {
        let root = tempfile::tempdir().unwrap();
        let logical = root.path().join("book.pdf");
        write_document_pdf(
            &logical,
            &[(100, 200), (100, 200), (300, 100), (100, 200)],
            true,
        );
        let backend = crate::pdf_loader::RemotePdfTestBackend::for_path(&logical);
        let mut input = document_input(root.path(), &logical, 4);
        input.initial_binding = Some(initial_defaults());
        let resolve_counter = crate::pdf_loader::RemoteResolveTestCounter::for_path(&logical);
        let prepared = prepare(&input, None).unwrap();
        assert_eq!(prepared.draft.record().direction, ReadingDirection::Rtl);
        assert_eq!(prepared.draft.record().cover_enabled, false);
        assert_eq!(
            prepared.result.groups[0],
            DerivedEditorGroup::Pair(pdf_page(0), pdf_page(1))
        );
        assert!(prepared.result.groups.iter().any(
            |unit| matches!(unit, DerivedEditorGroup::Single { page, .. } if *page == pdf_page(2))
        ));
        assert_eq!(prepared.raw_pages.len(), 4);
        assert_eq!(prepared.rows.items.len(), 4);
        assert!(backend.direction_requests().contains(&true));
        verify_source(&input, &prepared.fingerprint).unwrap();
        assert_eq!(
            resolve_counter.take(),
            0,
            "retained PDF read target is reused"
        );
    }

    #[test]
    fn epub_worker_keeps_pinned_generation_logical_identity_and_direction() {
        let root = tempfile::tempdir().unwrap();
        let logical = root.path().join("book.epub");
        write_document_epub(&logical);
        let generated = logical.with_extension("generated.pdf");
        write_document_pdf(&generated, &[(100, 200), (100, 200), (100, 200)], false);
        let size = std::fs::metadata(&generated).unwrap().len();
        let _pin = crate::pdf_loader::pin_epub_with_direction_for_test(
            &logical,
            731,
            size,
            crate::pdf_loader::PdfReadingDirection::R2L,
        );
        let backend = crate::pdf_loader::RemotePdfTestBackend::for_path(&generated);
        let mut input = document_input(root.path(), &logical, 3);
        let mut initial = initial_defaults();
        initial.default_mode = crate::settings::SpreadMode::LtrCover;
        input.initial_binding = Some(initial);
        let resolve_counter = crate::pdf_loader::RemoteResolveTestCounter::for_path(&logical);
        let prepared = prepare(&input, None).unwrap();
        assert_eq!(prepared.draft.record().direction, ReadingDirection::Rtl);
        assert_eq!(prepared.result.cover, Some(pdf_page(0)));
        assert_eq!(
            prepared.result.groups[1],
            DerivedEditorGroup::Pair(pdf_page(1), pdf_page(2))
        );
        assert_eq!(prepared.raw_pages.len(), 3);
        assert!(prepared.rows.items.iter().all(
            |item| matches!(item, GridItem::PdfPage { pdf_path, .. } if *pdf_path == logical)
        ));
        assert_eq!(
            prepared.fingerprint.document_stamp,
            Some(crate::pdf_loader::DocumentStamp::Generation {
                id: 731,
                pdf_size: size
            })
        );
        assert_eq!(prepared.fingerprint.files.len(), 2);
        assert!(
            !backend.direction_requests().contains(&true),
            "pinned EPUB direction is authoritative"
        );
        verify_source(&input, &prepared.fingerprint).unwrap();
        assert_eq!(
            resolve_counter.take(),
            0,
            "no conversion or generation re-resolution"
        );
        std::fs::write(&logical, b"changed EPUB source").unwrap();
        assert_eq!(
            verify_source(&input, &prepared.fingerprint).unwrap_err(),
            SOURCE_CHANGED
        );
    }

    #[test]
    fn document_worker_rejects_retained_page_count_that_disagrees_with_real_pdf() {
        let root = tempfile::tempdir().unwrap();
        let logical = root.path().join("book.pdf");
        write_document_pdf(&logical, &[(100, 200), (100, 200)], false);
        let _backend = crate::pdf_loader::RemotePdfTestBackend::for_path(&logical);
        let input = document_input(root.path(), &logical, 3);
        assert_eq!(prepare(&input, None).unwrap_err(), SOURCE_CHANGED);
    }

    #[test]
    fn independent_policies_keep_full_raw_hidden_pages_and_saved_alias_topology() {
        let root = tempfile::tempdir().unwrap();
        let first = archive_input(
            root.path(),
            &[("a.png", 8, 16), ("b.jpg", 8, 16), ("c.png", 8, 16)],
        );
        let mut second = first.clone();
        second.policy.excluded_extensions.insert("jpg".into());
        let mut saved = BookComposition::new(
            ReadingDirection::Ltr,
            vec![page("A.PNG"), page("B.JPG"), page("C.PNG")],
        );
        saved.page_overrides.push(PageOverrideEntry {
            page: page("A.PNG"),
            directive: PageOverride::BlankOpposite {
                page_side: PageSide::Leading,
            },
        });
        saved.hidden_pages.push(page("B.JPG"));
        let before = saved.clone();
        let all = prepare(&first, Some(&saved)).unwrap();
        let filtered = prepare(&second, Some(&saved)).unwrap();
        assert_eq!(all.raw_pages, filtered.raw_pages);
        assert_eq!(all.raw_pages.len(), 3, "blank is not a raw page");
        assert_eq!(
            all.draft.record(),
            &saved,
            "saved aliases are preserved without new-page duplicates"
        );
        assert_eq!(filtered.draft.record(), &saved);
        assert_eq!(
            all.rows.items.len(),
            3,
            "hidden source page remains explicitly previewable"
        );
        assert_eq!(filtered.rows.items.len(), 2);
        assert_eq!(saved, before);
        let projected_all = project_record(&first, Some(&saved)).unwrap();
        let projected_filtered = project_record(&second, Some(&saved)).unwrap();
        assert_eq!(
            projected_all.items.len(),
            2,
            "viewer projection applies hidden separately"
        );
        assert_eq!(projected_filtered.items.len(), 2);
        assert_eq!(filtered.draft.record().pages().count(), 3);
        verify_source(&first, &all.fingerprint).unwrap();
        verify_source(&second, &filtered.fingerprint).unwrap();
    }

    #[test]
    fn worker_derives_full_book_with_saved_rotation_and_live_override() {
        let root = tempfile::tempdir().unwrap();
        let mut input = archive_input(
            root.path(),
            &[
                ("a.png", 8, 16),
                ("b.png", 8, 16),
                ("c.png", 8, 16),
                ("d.png", 8, 16),
            ],
        );
        let (_, _, rows) = source_rows(&input).unwrap();
        let key = crate::edit_source::page_key_for_grid_item(&rows.0[1]).unwrap();
        let db = crate::rotation_db::RotationDb::open_at(&root.path().join("rotation.db")).unwrap();
        db.set_key(&key, Rotation::from_degrees(90)).unwrap();
        let prepared = prepare(&input, None).unwrap();
        assert_eq!(prepared.raw_pages.len(), 4);
        assert!(
            matches!(&prepared.result.groups[0], DerivedEditorGroup::Single { page: first, .. } if *first == page("a.png"))
        );
        assert!(
            matches!(&prepared.result.groups[1], DerivedEditorGroup::Single { page: second, .. } if *second == page("b.png"))
        );
        input.rotations.insert(page("b.png"), Rotation::None);
        let prepared = prepare(&input, None).unwrap();
        assert_eq!(
            prepared.result.groups[0],
            DerivedEditorGroup::Pair(page("a.png"), page("b.png"))
        );
        assert_eq!(
            prepared.result.groups[1],
            DerivedEditorGroup::Pair(page("c.png"), page("d.png"))
        );
    }

    #[test]
    fn worker_preserves_order_overrides_missing_hidden_and_excluded_members() {
        let root = tempfile::tempdir().unwrap();
        let mut input = archive_input(root.path(), &[("a.png", 8, 16), ("b.png", 8, 16)]);
        input.policy.excluded_extensions.insert("png".into());
        let mut saved = BookComposition::new(
            ReadingDirection::Rtl,
            vec![page("missing.png"), page("a.png")],
        );
        saved.hidden_pages.push(page("missing.png"));
        saved.page_overrides.push(PageOverrideEntry {
            page: page("missing.png"),
            directive: PageOverride::Alone,
        });
        let prepared = prepare(&input, Some(&saved)).unwrap();
        assert!(prepared.rows.items.is_empty());
        assert_eq!(
            prepared.draft.record().page_order,
            vec![page("missing.png"), page("a.png"), page("b.png")]
        );
        assert_eq!(prepared.draft.record().hidden_pages, saved.hidden_pages);
        assert_eq!(prepared.draft.record().page_overrides, saved.page_overrides);
        assert!(prepared.result.groups.is_empty());
    }

    #[test]
    fn worker_source_change_rejects_save_and_keeps_draft() {
        let root = tempfile::tempdir().unwrap();
        let input = archive_input(root.path(), &[("a.png", 8, 16), ("b.png", 8, 16)]);
        let prepared = prepare(&input, None).unwrap();
        let before = prepared.draft.record().clone();
        std::fs::write(root.path().join("book.zip"), b"changed archive").unwrap();
        assert_eq!(
            verify_source(&input, &prepared.fingerprint).unwrap_err(),
            SOURCE_CHANGED
        );
        assert_eq!(prepared.draft.record(), &before);
    }

    #[test]
    fn result_generation_rebuilds_order_hidden_and_directives_without_mutating_record() {
        let root = tempfile::tempdir().unwrap();
        let input = archive_input(
            root.path(),
            &[
                ("a.png", 8, 16),
                ("b.png", 8, 16),
                ("c.png", 8, 16),
                ("d.png", 8, 16),
                ("e.png", 8, 16),
            ],
        );
        let mut prepared = prepare(&input, None).unwrap();
        prepared
            .draft
            .apply(CompositionEdit::SetHidden {
                page: page("b.png"),
                hidden: true,
            })
            .unwrap();
        let record = prepared.draft.record().clone();
        let result = derive_result(
            &prepared.result_source,
            &record,
            prepared.draft.generation(),
        )
        .unwrap();
        assert_eq!(
            result.groups,
            vec![
                DerivedEditorGroup::Pair(page("a.png"), page("c.png")),
                DerivedEditorGroup::Pair(page("d.png"), page("e.png"))
            ]
        );
        assert_eq!(prepared.draft.record(), &record);
        prepared
            .draft
            .apply(CompositionEdit::SetOverride {
                page: page("c.png"),
                directive: Some(PageOverride::BlankOpposite {
                    page_side: PageSide::Trailing,
                }),
            })
            .unwrap();
        let blank = derive_result(
            &prepared.result_source,
            prepared.draft.record(),
            prepared.draft.generation(),
        )
        .unwrap();
        assert!(
            matches!(&blank.groups[1], DerivedEditorGroup::Single { page: p, placement: SinglePlacement::Trailing, companion: DerivedCompanion::Blank } if *p == page("c.png"))
        );
        prepared.draft.undo();
        let undo = derive_result(
            &prepared.result_source,
            prepared.draft.record(),
            prepared.draft.generation(),
        )
        .unwrap();
        assert_eq!(undo.groups, result.groups);
        prepared.draft.redo();
        let redo = derive_result(
            &prepared.result_source,
            prepared.draft.record(),
            prepared.draft.generation(),
        )
        .unwrap();
        assert_eq!(redo.groups, blank.groups);
        assert!(redo.generation > blank.generation);
    }

    #[test]
    fn result_worker_coalesces_to_latest_generation_and_drops_with_session() {
        let root = tempfile::tempdir().unwrap();
        let input = archive_input(
            root.path(),
            &[("a.png", 8, 16), ("b.png", 8, 16), ("c.png", 8, 16)],
        );
        let mut prepared = prepare(&input, None).unwrap();
        let worker =
            EditorResultWorker::spawn(prepared.result_source.clone(), egui::Context::default())
                .unwrap();
        for hidden in [true, false, true] {
            prepared
                .draft
                .apply(CompositionEdit::SetHidden {
                    page: page("b.png"),
                    hidden,
                })
                .unwrap();
            worker.request(prepared.draft.generation(), prepared.draft.record().clone());
        }
        let latest = prepared.draft.generation();
        loop {
            let (generation, result) = worker
                .receiver
                .recv_timeout(std::time::Duration::from_secs(15))
                .unwrap();
            if generation == latest {
                let result = result.unwrap();
                assert_eq!(
                    result.groups,
                    vec![DerivedEditorGroup::Pair(page("a.png"), page("c.png"))]
                );
                break;
            }
            assert!(generation < latest);
        }
    }

    #[test]
    fn worker_rtl_shift_is_not_frozen_and_does_not_re_read_saved_shift_store() {
        let root = tempfile::tempdir().unwrap();
        let mut input = archive_input(
            root.path(),
            &[
                ("a.png", 8, 16),
                ("b.png", 8, 16),
                ("c.png", 8, 16),
                ("d.png", 8, 16),
            ],
        );
        input.rules.mode = crate::settings::SpreadMode::Rtl;
        input.rules.direction = crate::settings::ReadingDirection::Rtl;
        input.shift = Some(SpreadShiftPageRef::ArchiveEntry("b.png".into()));
        let prepared = prepare(&input, None).unwrap();
        assert_eq!(prepared.draft.record().direction, ReadingDirection::Rtl);
        assert_eq!(
            prepared.result.groups[0],
            DerivedEditorGroup::Pair(page("a.png"), page("b.png"))
        );
        assert!(!root.path().join("spread_shift.db").exists());
    }

    #[test]
    fn worker_captures_forced_blank_and_cover_supplement_from_canonical_presentation() {
        for mode in [
            crate::settings::SpreadMode::LtrCover,
            crate::settings::SpreadMode::RtlCover,
        ] {
            let root = tempfile::tempdir().unwrap();
            let mut input = archive_input(
                root.path(),
                &[
                    ("a.png", 8, 16),
                    ("b.png", 8, 16),
                    ("c.png", 8, 16),
                    ("d.png", 8, 16),
                ],
            );
            input.rules.mode = mode;
            input.rules.direction = mode.canonical_reading_direction().unwrap();
            input.rules.page_alone.after_cover = true;
            input.rules.page_alone.last = true;
            let blank = prepare(&input, None).unwrap();
            assert_eq!(blank.result.cover, Some(page("a.png")));
            assert!(blank.result.groups.iter().any(|unit| matches!(
                unit,
                DerivedEditorGroup::Single {
                    companion: DerivedCompanion::Blank,
                    ..
                }
            )));
            input.rules.final_cover = true;
            let supplement = prepare(&input, None).unwrap();
            assert!(supplement.result.groups.iter().any(|unit| matches!(unit,
                DerivedEditorGroup::Single { companion: DerivedCompanion::Cover(cover), .. } if *cover == page("a.png"))));
            let resolved = resolve_composition(
                supplement.draft.record(),
                supplement.draft.domain(),
                supplement.draft.manifest(),
            )
            .unwrap();
            assert_eq!(resolved.visible_pages.len(), 4);
            assert_eq!(supplement.raw_pages.len(), 4);
            assert_eq!(
                supplement.rows.items.len(),
                4,
                "cover helper is not a source row"
            );
            assert!(
                supplement.draft.record().page_overrides.is_empty(),
                "automatic presentation is never persisted"
            );
        }
    }

    fn initial_defaults() -> InitialEditorBinding {
        InitialEditorBinding {
            default_mode: crate::settings::SpreadMode::Ltr,
            default_direction: crate::settings::ReadingDirection::Ltr,
            follow_document_direction: true,
        }
    }

    fn store_rtl_cover(db: &mut crate::spread_db::SpreadDb, path: &Path) {
        db.set_mode_and_direction(
            path,
            None,
            crate::settings::SpreadMode::RtlCover,
            crate::settings::ReadingDirection::Rtl,
            (
                crate::settings::SpreadMode::Ltr,
                crate::settings::ReadingFlow::Paged,
                crate::settings::ReadingDirection::Ltr,
            ),
        )
        .unwrap();
    }

    #[test]
    fn single_split_initialization_restores_saved_exact_rtl_cover_binding_read_only() {
        for viewing in [
            crate::settings::SpreadMode::Single,
            crate::settings::SpreadMode::SplitLtr,
        ] {
            let root = tempfile::tempdir().unwrap();
            let mut input = archive_input(
                root.path(),
                &[("a.png", 8, 16), ("b.png", 8, 16), ("c.png", 8, 16)],
            );
            let path = match &input.source {
                BookCompositionSource::Archive { tree, .. } => tree.zip_path.clone(),
                _ => unreachable!(),
            };
            let mut db =
                crate::spread_db::SpreadDb::open_at(&root.path().join("spread.db")).unwrap();
            store_rtl_cover(&mut db, &path);
            db.set_mode_only(&path, viewing).unwrap();
            let before = db.get_state_with_fallback(&path, None);
            input.initial_binding = Some(initial_defaults());
            // Root's provisional Spread fallback is replaced by the book binding.
            input.rules.mode = crate::settings::SpreadMode::Ltr;
            let prepared = prepare(&input, None).unwrap();
            assert_eq!(prepared.draft.record().direction, ReadingDirection::Rtl);
            assert_eq!(prepared.result.cover, Some(page("a.png")));
            assert_eq!(
                prepared.result.groups[1],
                DerivedEditorGroup::Pair(page("b.png"), page("c.png"))
            );
            assert_eq!(db.get_state_with_fallback(&path, None), before);
        }
    }

    #[test]
    fn single_split_initialization_preserves_parent_cover_and_exact_view_only_row() {
        let root = tempfile::tempdir().unwrap();
        let mut input = archive_input(
            root.path(),
            &[
                ("chapter/a.png", 8, 16),
                ("chapter/b.png", 8, 16),
                ("chapter/c.png", 8, 16),
            ],
        );
        let physical = match &mut input.source {
            BookCompositionSource::Archive {
                tree,
                prefix,
                display_prefix,
            } => {
                *prefix = vec!["chapter".into()];
                *display_prefix = prefix.clone();
                tree.zip_path.clone()
            }
            _ => unreachable!(),
        };
        input.scope = BookScopeKey::archive(&physical, &["chapter".into()]).unwrap();
        input.domain = PageDomain::ArchiveLevel {
            prefix: "chapter".into(),
        };
        input.initial_binding = Some(initial_defaults());
        let mut db = crate::spread_db::SpreadDb::open_at(&root.path().join("spread.db")).unwrap();
        store_rtl_cover(&mut db, &physical);
        let exact = physical.join("chapter");
        db.set_mode_only(&exact, crate::settings::SpreadMode::SplitLtr)
            .unwrap();
        let before = db.get_state_with_fallback(&exact, Some(&physical));
        let prepared = prepare(&input, None).unwrap();
        assert_eq!(prepared.draft.record().direction, ReadingDirection::Rtl);
        assert_eq!(prepared.result.cover, Some(page("chapter/a.png")));
        assert_eq!(db.get_state_with_fallback(&exact, Some(&physical)), before);
    }

    #[test]
    fn converted_initial_binding_uses_physical_pc_key_separate_from_logical_scope() {
        let root = tempfile::tempdir().unwrap();
        let mut input = archive_input(root.path(), &[("a.png", 8, 16)]);
        let physical = match &input.source {
            BookCompositionSource::Archive { tree, .. } => tree.zip_path.clone(),
            _ => unreachable!(),
        };
        let logical = root.path().join("original.rar");
        input.scope = BookScopeKey::archive(&logical, &[]).unwrap();
        let mut db = crate::spread_db::SpreadDb::open_at(&root.path().join("spread.db")).unwrap();
        store_rtl_cover(&mut db, &physical);
        db.set_mode_and_direction(
            &logical,
            None,
            crate::settings::SpreadMode::LtrCover,
            crate::settings::ReadingDirection::Ltr,
            (
                crate::settings::SpreadMode::Ltr,
                crate::settings::ReadingFlow::Paged,
                crate::settings::ReadingDirection::Ltr,
            ),
        )
        .unwrap();
        let resolved = read_initial_binding(&input, initial_defaults()).unwrap();
        assert_eq!(resolved.mode, crate::settings::SpreadMode::RtlCover);
        assert_eq!(resolved.direction, crate::settings::ReadingDirection::Rtl);
        assert_eq!(
            input.scope.logical_root,
            BookScopeKey::archive(&logical, &[]).unwrap().logical_root
        );
    }

    #[test]
    fn document_direction_initialization_uses_existing_resolver_without_inventing_cover() {
        use crate::{
            settings::{ReadingDirection as Direction, SpreadMode},
            spread_db::{StoredSpreadState, StoredViewMode},
        };
        for view_mode in [StoredViewMode::Single, StoredViewMode::LandscapeSplit] {
            let inherited = StoredSpreadState {
                view_mode: Some(view_mode),
                ..Default::default()
            };
            let mut defaults = initial_defaults();
            defaults.default_mode = SpreadMode::LtrCover;
            let resolved = resolve_initial_binding(
                inherited,
                defaults,
                Some(crate::pdf_loader::PdfReadingDirection::R2L),
            );
            assert_eq!(resolved.mode, SpreadMode::RtlCover);
            assert_eq!(resolved.direction, Direction::Rtl);
            let explicit_single = StoredSpreadState {
                mode: Some(SpreadMode::Single),
                direction: Some(Direction::Rtl),
                view_mode: Some(view_mode),
                ..Default::default()
            };
            let resolved = resolve_initial_binding(
                explicit_single,
                defaults,
                Some(crate::pdf_loader::PdfReadingDirection::L2R),
            );
            assert_eq!(resolved.mode, SpreadMode::Rtl);
            assert_eq!(resolved.direction, Direction::Rtl);
            assert!(!resolved.mode.has_cover());
        }
    }
}

fn read_landscape(input: &EditorSourceInput, items: &[GridItem]) -> Result<Vec<bool>, String> {
    let keys: Vec<_> = items
        .iter()
        .map(crate::edit_source::page_key_for_grid_item)
        .collect();
    let db_path = input.data_dir.join("rotation.db");
    let stored = if db_path.exists() {
        crate::rotation_db::RotationDb::open_readonly_at(&db_path)
            .map_err(|error| error.to_string())?
            .get_many(keys.iter().filter_map(|key| key.as_deref()))
    } else {
        HashMap::new()
    };
    let live: HashMap<_, _> = input
        .rotations
        .iter()
        .map(|(page, rotation)| (page.matching_key(), *rotation))
        .collect();
    let mut dims = HashMap::new();
    match &input.source {
        BookCompositionSource::Archive { tree, .. } => {
            let mut archive = if items
                .iter()
                .any(|item| matches!(item, GridItem::ZipImage { .. }))
            {
                Some(
                    crate::zip_loader::open_archive(&tree.zip_path)
                        .map_err(|error| error.to_string())?,
                )
            } else {
                None
            };
            for item in items {
                let GridItem::ZipImage { entry_name, .. } = item else {
                    continue;
                };
                let head = if crate::raw_format::is_raw_path(Path::new(entry_name))
                    || Path::new(entry_name)
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("avif"))
                {
                    crate::zip_loader::read_entry_bytes(&tree.zip_path, entry_name)
                } else {
                    // Nested members require the existing nested byte reader.
                    // Ordinary members only decompress a bounded header prefix.
                    crate::zip_loader::read_entry_prefix_from_archive(
                        archive.as_mut().unwrap(),
                        entry_name,
                        64 * 1024,
                    )
                    .or_else(|_| crate::zip_loader::read_entry_bytes(&tree.zip_path, entry_name))
                }
                .map_err(|error| format!("ページ寸法を読み取れません ({entry_name}): {error}"))?;
                let size = super::metadata_ops::probe_image_dims_from_bytes(&head, entry_name)
                    .ok_or_else(|| format!("ページ寸法を読み取れません ({entry_name})"))?;
                if size.0 == 0 || size.1 == 0 {
                    return Err(format!("ページ寸法が不正です ({entry_name})"));
                }
                dims.insert(
                    CompositionPageRef::ArchiveEntry {
                        entry_name: entry_name.clone(),
                    },
                    size,
                );
            }
        }
        BookCompositionSource::Document {
            read_target,
            physical_source,
            count,
            ..
        } if !items.is_empty() => {
            let sizes = match read_target {
                Some(read) => crate::pdf_loader::get_page_sizes_with_read_target(
                    &input.scope.logical_root,
                    read,
                    input.password.as_deref(),
                ),
                None => {
                    crate::pdf_loader::get_page_sizes(physical_source, input.password.as_deref())
                }
            }
            .map_err(|error| format!("ページ寸法を読み取れません: {error}"))?;
            if sizes.len() != *count as usize {
                return Err(SOURCE_CHANGED.into());
            }
            for (index, (width, height)) in sizes.into_iter().enumerate() {
                if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
                    return Err("ページ寸法が不正です".into());
                }
                // Only ratio is used; fixed-point preserves sub-point page boxes.
                dims.insert(
                    CompositionPageRef::PdfPage {
                        page_num: index as u32,
                    },
                    (
                        (width * 1000.0).round().max(1.0) as u32,
                        (height * 1000.0).round().max(1.0) as u32,
                    ),
                );
            }
        }
        _ => {}
    }
    items
        .iter()
        .zip(keys)
        .map(|(item, key)| {
            let Some(page) = page_for_item(item) else {
                return Ok(false);
            };
            let (width, height) = dims
                .get(&page)
                .copied()
                .ok_or("ページ寸法を確定できません")?;
            let rotation = live
                .get(&page.matching_key())
                .copied()
                .or_else(|| key.as_ref().and_then(|key| stored.get(key)).copied())
                .unwrap_or(Rotation::None);
            Ok(crate::rotation_db::landscape_after_rotation(
                width, height, rotation,
            ))
        })
        .collect()
}
