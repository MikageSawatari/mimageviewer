//! A single append receipt. Filesystem inspection and recycling run on workers.
//!
//! The direct physical composition includes hidden files, sidecars and directories.
//! No recursive scan, hash, retained file handles or recovery state is introduced.
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AppendOperation {
    pub operation_id: u64,
    pub source_context: crate::app::ViewerContextId,
    pub accepted_revision: u64,
}

/// Existing live source owners whose automatic observations terminate with this
/// operation. Folder-watch sequence coverage is unavailable for book folders;
/// the terminal full-composition check is therefore unconditional.
#[derive(Clone, Debug, Default)]
pub(crate) struct AppendObservations {
    contexts: Vec<crate::app::ViewerContextId>,
}

impl AppendObservations {
    pub(crate) fn record(&mut self, context: crate::app::ViewerContextId) {
        if !self.contexts.contains(&context) {
            self.contexts.push(context);
        }
    }
    pub(crate) fn contexts(&self) -> &[crate::app::ViewerContextId] {
        &self.contexts
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AppendMetadataStores {
    pub adjustment: bool,
    pub local_adjust: bool,
    pub crop: bool,
    pub mask: bool,
    pub conceal: bool,
    pub comic: bool,
    pub rating: bool,
    pub tags: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BookAppendMetadataCopySummary {
    pub adjusted: Vec<String>,
    pub local_adjusted: Vec<String>,
    pub cropped: Vec<String>,
    pub erased: Vec<String>,
    pub concealed: Vec<String>,
    pub annotated: Vec<String>,
    /// Worker-confirmed destination values, including empty/zero results.
    pub ratings: std::collections::HashMap<String, u8>,
    pub tags: std::collections::HashMap<String, Vec<String>>,
    pub errors: Vec<String>,
}

pub(crate) fn copy_append_metadata_at(
    data_dir: &Path,
    summary: &mut crate::books::BookAppendSummary,
    stores: AppendMetadataStores,
) {
    let mappings = |values: &[crate::books::BookPathMapping]| {
        values
            .iter()
            .filter_map(|mapping| {
                let from = crate::adjustment_db::normalize_path(&mapping.from);
                let to = crate::adjustment_db::normalize_path(&mapping.to);
                (from != to).then_some((from, to))
            })
            .collect::<Vec<_>>()
    };
    let edits = mappings(&summary.edit_copies);
    let mut semantic = mappings(&summary.semantic_copies);
    semantic.extend(edits.iter().cloned());
    let keys = edits.iter().map(|(_, to)| to.clone()).collect::<Vec<_>>();
    let key_refs = keys.iter().map(String::as_str).collect::<Vec<_>>();
    let semantic_keys = semantic
        .iter()
        .map(|(_, to)| to.clone())
        .collect::<Vec<_>>();
    let report = &mut summary.metadata_copy;
    // Existing copy helpers publish the same epochs as their UI predecessors.
    // One epoch around the whole batch keeps readers from observing a half copy.
    let _edit_epoch = crate::page_edit_write_epoch::PAGE_EDIT_WRITES.begin();
    let _rating_epoch = crate::rating_db::RATING_WRITES.begin();
    let _tag_epoch = crate::tags_db::TAG_WRITES.begin();
    macro_rules! copy_store {
        ($enabled:expr, $name:literal, $db:ty, $pairs:expr, $method:ident, $after:expr) => {
            if $enabled && !$pairs.is_empty() {
                let path = data_dir.join(concat!($name, ".db"));
                // Never silently recreate a store that disappeared after UI capture.
                match fs::metadata(&path)
                    .map_err(|error| error.to_string())
                    .and_then(|_| <$db>::open_at(&path).map_err(|error| error.to_string()))
                {
                    Ok(mut db) => {
                        #[allow(unused_mut)]
                        let db = &mut db;
                        for (from, to) in &$pairs {
                            if let Err(error) = db.$method(from, to) {
                                report.errors.push(format!("{}: {error}", $name));
                            }
                        }
                        ($after)(db, report);
                    }
                    Err(error) => report.errors.push(format!("{}: {error}", $name)),
                }
            }
        };
    }
    copy_store!(
        stores.adjustment,
        "adjustment",
        crate::adjustment_db::AdjustmentDb,
        edits,
        copy_page_params_key,
        |db: &mut crate::adjustment_db::AdjustmentDb,
         report: &mut BookAppendMetadataCopySummary| {
            report
                .adjusted
                .extend(db.load_page_params_many(&key_refs).into_keys());
        }
    );
    copy_store!(
        stores.local_adjust,
        "local_adjust",
        crate::local_adjust_db::LocalAdjustDb,
        edits,
        copy_entry_key,
        |db: &mut crate::local_adjust_db::LocalAdjustDb,
         report: &mut BookAppendMetadataCopySummary| {
            report
                .local_adjusted
                .extend(db.load_existing_layer_keys(&keys));
        }
    );
    copy_store!(
        stores.crop,
        "export_crop",
        crate::export_crop::CropDb,
        edits,
        copy_entry_key,
        |db: &mut crate::export_crop::CropDb, report: &mut BookAppendMetadataCopySummary| {
            report.cropped.extend(db.load_many(&key_refs).into_keys());
        }
    );
    copy_store!(
        stores.mask,
        "mask",
        crate::mask_db::MaskDb,
        edits,
        copy_entry_key,
        |db: &mut crate::mask_db::MaskDb, report: &mut BookAppendMetadataCopySummary| {
            report.erased.extend(db.load_existing_mask_keys(&key_refs));
        }
    );
    copy_store!(
        stores.conceal,
        "conceal",
        crate::conceal_db::ConcealDb,
        edits,
        copy_entry_key,
        |db: &mut crate::conceal_db::ConcealDb, report: &mut BookAppendMetadataCopySummary| {
            report
                .concealed
                .extend(db.load_existing_conceal_keys(&key_refs));
        }
    );
    copy_store!(
        stores.comic,
        "comic",
        crate::comic_db::ComicDb,
        edits,
        copy_entry_key,
        |db: &mut crate::comic_db::ComicDb, report: &mut BookAppendMetadataCopySummary| {
            report
                .annotated
                .extend(db.load_existing_comic_keys(&key_refs));
        }
    );
    copy_store!(
        stores.rating,
        "rating",
        crate::rating_db::RatingDb,
        semantic,
        copy_entry_key,
        |db: &mut crate::rating_db::RatingDb, report: &mut BookAppendMetadataCopySummary| {
            let mut ratings = db.get_many(&semantic_keys);
            for key in &semantic_keys {
                ratings.entry(key.clone()).or_insert(0);
            }
            report.ratings.extend(ratings);
        }
    );
    copy_store!(
        stores.tags,
        "tags",
        crate::tags_db::TagsDb,
        semantic,
        copy_item_key,
        |db: &mut crate::tags_db::TagsDb, report: &mut BookAppendMetadataCopySummary| {
            let mut tags = db.get_many_display_tags(&semantic_keys);
            for key in &semantic_keys {
                tags.entry(key.clone()).or_default();
            }
            report.tags.extend(tags);
        }
    );
    if !report.errors.is_empty() {
        summary.undo_files = None;
        for error in &report.errors {
            crate::logger::log(format!("[book-append-undo] metadata copy: {error}"));
        }
    }
    finalize_append_receipt(summary);
}

#[derive(Clone, Debug)]
pub(crate) struct BookAppendUndoReceipt {
    pub files: AppendFilesReceipt,
    pub operation_id: u64,
    pub source_context: crate::app::ViewerContextId,
    pub accepted_revision: u64,
}

impl BookAppendUndoReceipt {
    pub(crate) fn operation(&self) -> AppendOperation {
        AppendOperation {
            operation_id: self.operation_id,
            source_context: self.source_context,
            accepted_revision: self.accepted_revision,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum EntryKind {
    File,
    Directory,
    Reparse,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EntryStamp {
    kind: EntryKind,
    size: u64,
    modified: SystemTime,
}

/// Exact names and unrounded modification times, captured without following links.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BookCompositionStamp(BTreeMap<OsString, EntryStamp>);

#[derive(Clone, Debug)]
pub(crate) struct CreatedFileStamp {
    pub path: PathBuf,
    stamp: EntryStamp,
}

#[derive(Clone, Debug)]
pub(crate) struct AppendFilesReceipt {
    pub root: PathBuf,
    pub folder: PathBuf,
    pub book_name: String,
    pub created: Vec<CreatedFileStamp>,
    pub composition: BookCompositionStamp,
}

impl AppendFilesReceipt {
    /// Called after metadata copy, and again for the fresh refresh preparation.
    /// This is worker I/O; never use this on the UI thread.
    pub(crate) fn verify_current(&self) -> Result<(), String> {
        verify_output_scope(&self.root, &self.folder, &self.created)?;
        let current = capture_book_composition(&self.folder)?;
        self.verify_composition(&current)
    }

    pub(crate) fn verify_composition(&self, current: &BookCompositionStamp) -> Result<(), String> {
        if &self.composition != current || self.created.is_empty() {
            return Err("本の内容が変わったため追加を取り消せません".into());
        }
        for created in &self.created {
            let name = created
                .path
                .file_name()
                .ok_or_else(|| "追加ページの名前が不正です".to_string())?;
            if current.0.get(name) != Some(&created.stamp) {
                return Err("本の内容が変わったため追加を取り消せません".into());
            }
        }
        Ok(())
    }
}

/// Before plus only exact, successfully written outputs determines the expected composition.
pub(crate) struct AppendReceiptBuilder {
    root: PathBuf,
    folder: PathBuf,
    book_name: String,
    before: BookCompositionStamp,
    created: Vec<CreatedFileStamp>,
}

impl AppendReceiptBuilder {
    pub(crate) fn begin(root: &Path, folder: &Path, book_name: String) -> Result<Self, String> {
        verify_output_scope(root, folder, &[])?;
        Ok(Self {
            root: root.into(),
            folder: folder.into(),
            book_name,
            before: capture_book_composition(folder)?,
            created: Vec::new(),
        })
    }

    pub(crate) fn record(&mut self, path: &Path) -> Result<(), String> {
        let created = CreatedFileStamp {
            path: path.into(),
            stamp: stamp_at(path)?,
        };
        if path
            .parent()
            .is_none_or(|parent| !crate::folder_tree::path_eq(parent, &self.folder))
            || created.stamp.kind != EntryKind::File
        {
            return Err("取り消すページの範囲が不正です".into());
        }
        if self
            .before
            .0
            .contains_key(path.file_name().ok_or("追加ページの名前が不正です")?)
            || self
                .created
                .iter()
                .any(|previous| crate::folder_tree::path_eq(&previous.path, path))
        {
            return Err("追加ページが既存ページと重複しています".into());
        }
        self.created.push(created);
        Ok(())
    }

    pub(crate) fn finish(self) -> Result<AppendFilesReceipt, String> {
        let mut expected = self.before;
        for created in &self.created {
            expected.0.insert(
                created
                    .path
                    .file_name()
                    .ok_or("追加ページの名前が不正です")?
                    .into(),
                created.stamp.clone(),
            );
        }
        let receipt = AppendFilesReceipt {
            root: self.root,
            folder: self.folder,
            book_name: self.book_name,
            created: self.created,
            composition: expected,
        };
        receipt.verify_current()?;
        Ok(receipt)
    }
}

/// Metadata copy currently writes only central stores. Any future physical own effect
/// must be explicitly recorded rather than accepting an unexplained sidecar difference.
pub(crate) fn finalize_append_receipt(summary: &mut crate::books::BookAppendSummary) {
    if let Some(files) = &summary.undo_files
        && let Err(error) = files.verify_current()
    {
        crate::logger::log(format!(
            "[book-append-undo] metadata terminal receipt: {error}"
        ));
        summary.undo_files = None;
    }
}

pub(crate) fn capture_book_composition(folder: &Path) -> Result<BookCompositionStamp, String> {
    let mut entries = BTreeMap::new();
    for entry in
        fs::read_dir(folder).map_err(|error| format!("本の構成を確認できません: {error}"))?
    {
        let entry = entry.map_err(|error| format!("本の構成を確認できません: {error}"))?;
        entries.insert(entry.file_name(), stamp_at(&entry.path())?);
    }
    Ok(BookCompositionStamp(entries))
}

fn stamp_at(path: &Path) -> Result<EntryStamp, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("{} の状態を確認できません: {error}", path.display()))?;
    let kind = if is_reparse(&metadata) {
        EntryKind::Reparse
    } else if metadata.is_file() {
        EntryKind::File
    } else if metadata.is_dir() {
        EntryKind::Directory
    } else {
        EntryKind::Other
    };
    Ok(EntryStamp {
        kind,
        size: metadata.len(),
        modified: metadata
            .modified()
            .map_err(|error| format!("更新時刻を確認できません: {error}"))?,
    })
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn verify_output_scope(
    root: &Path,
    folder: &Path,
    created: &[CreatedFileStamp],
) -> Result<(), String> {
    let root = std::path::absolute(root).map_err(|error| error.to_string())?;
    let folder = std::path::absolute(folder).map_err(|error| error.to_string())?;
    if folder
        .parent()
        .is_none_or(|parent| !crate::folder_tree::path_eq(parent, &root))
    {
        return Err("本棚直下の本ではありません".into());
    }
    for ancestor in folder.ancestors() {
        let metadata = fs::symlink_metadata(ancestor)
            .map_err(|error| format!("本の保存先を確認できません: {error}"))?;
        if !metadata.is_dir() || is_reparse(&metadata) {
            return Err("リンクを含む本の保存先は取り消せません".into());
        }
    }
    let mut paths = HashSet::new();
    for output in created {
        let path = std::path::absolute(&output.path).map_err(|error| error.to_string())?;
        if path
            .parent()
            .is_none_or(|parent| !crate::folder_tree::path_eq(parent, &folder))
            || output.stamp.kind != EntryKind::File
            || !paths.insert(crate::adjustment_db::normalize_path(&path))
        {
            return Err("取り消すページの範囲が不正です".into());
        }
        if stamp_at(&path)? != output.stamp {
            return Err("本の内容が変わったため追加を取り消せません".into());
        }
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct BookAppendUndoResult {
    pub book_name: String,
    pub folder: PathBuf,
    pub succeeded: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
    pub purge_errors: Vec<String>,
    pub store_mutations: crate::rename_key_migration::StoreMutationEffects,
}

pub(crate) fn undo_append_at(
    data_dir: PathBuf,
    receipt: BookAppendUndoReceipt,
    hwnd: Option<isize>,
) -> Result<crate::books::BookOpResult, String> {
    undo_append_with(
        &data_dir,
        receipt,
        |paths| crate::delete_worker::recycle_required(hwnd, paths),
        |scopes, guard| {
            crate::rename_key_migration::purge_removed_scopes_guarded_at(
                &data_dir,
                scopes,
                &[],
                guard,
            )
        },
    )
    .map(crate::books::BookOpResult::UndoAppend)
}

fn undo_append_with(
    data_dir: &Path,
    receipt: BookAppendUndoReceipt,
    recycle: impl FnOnce(&[PathBuf]) -> crate::delete_worker::DeleteChunkOutcome,
    purge: impl FnOnce(
        &[crate::delete_worker::DeleteSourceScope],
        &crate::rename_key_migration::DeleteMaintenanceLease,
    ) -> crate::rename_key_migration::PurgeReport,
) -> Result<BookAppendUndoResult, String> {
    let paths = receipt
        .files
        .created
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let scopes = paths
        .iter()
        .cloned()
        .map(crate::delete_worker::DeleteSourceScope::Exact)
        .collect::<Vec<_>>();
    let guard = crate::rename_key_migration::acquire_delete_epub_guard(data_dir, &scopes)?;
    // Validate all outputs before the first Shell call. The lease lasts through purge.
    receipt.files.verify_current()?;
    let recycled = recycle(&paths);
    let succeeded_scopes = recycled
        .succeeded
        .iter()
        .cloned()
        .map(crate::delete_worker::DeleteSourceScope::Exact)
        .collect::<Vec<_>>();
    let report = if succeeded_scopes.is_empty() {
        Default::default()
    } else {
        purge(&succeeded_scopes, &guard)
    };
    Ok(BookAppendUndoResult {
        book_name: receipt.files.book_name,
        folder: receipt.files.folder,
        succeeded: recycled.succeeded,
        failed: recycled.failed,
        purge_errors: report.errors,
        store_mutations: report.store_mutations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        tmp: tempfile::TempDir,
        root: PathBuf,
        folder: PathBuf,
        source: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().join("books");
            let folder = root.join("book");
            fs::create_dir_all(&folder).unwrap();
            let source = tmp.path().join("source.jpg");
            fs::write(&source, b"source remains untouched").unwrap();
            Self {
                tmp,
                root,
                folder,
                source,
            }
        }

        fn receipt(&self, count: usize) -> BookAppendUndoReceipt {
            let mut builder =
                AppendReceiptBuilder::begin(&self.root, &self.folder, "book".into()).unwrap();
            for number in 1..=count {
                let path = self.folder.join(format!("{number:04}_page.jpg"));
                fs::write(&path, b"generated copy").unwrap();
                builder.record(&path).unwrap();
            }
            BookAppendUndoReceipt {
                files: builder.finish().unwrap(),
                operation_id: 7,
                source_context: crate::app::ViewerContextId::for_test(3),
                accepted_revision: 11,
            }
        }
    }

    #[test]
    fn section1373_receipt_contains_all_exact_created_files_without_source_or_folder() {
        let fixture = Fixture::new();
        fs::write(fixture.folder.join("existing.jpg"), b"old page").unwrap();
        fs::write(fixture.folder.join(".hidden"), b"hidden").unwrap();
        fs::create_dir(fixture.folder.join("subdirectory")).unwrap();
        let receipt = fixture.receipt(3);
        assert_eq!(receipt.files.created.len(), 3);
        assert_eq!(receipt.files.composition.0.len(), 6);
        assert!(
            receipt
                .files
                .created
                .iter()
                .all(|file| file.path.parent() == Some(fixture.folder.as_path()))
        );
        assert_eq!(receipt.operation().operation_id, 7);
        receipt.files.verify_current().unwrap();
        assert_eq!(
            fs::read(fixture.source).unwrap(),
            b"source remains untouched"
        );
    }

    #[test]
    fn section1373_before_plus_outputs_rejects_unexplained_direct_entries() {
        for change in ["extra", "removed", "renamed", "subdirectory"] {
            let fixture = Fixture::new();
            let existing = fixture.folder.join("existing.bin");
            fs::write(&existing, b"existing").unwrap();
            let mut builder =
                AppendReceiptBuilder::begin(&fixture.root, &fixture.folder, "book".into()).unwrap();
            let output = fixture.folder.join("0001_copy.jpg");
            fs::write(&output, b"copy").unwrap();
            builder.record(&output).unwrap();
            match change {
                "extra" => {
                    fs::write(fixture.folder.join(".external-sidecar"), b"external").unwrap()
                }
                "removed" => fs::remove_file(&existing).unwrap(),
                "renamed" => fs::rename(&existing, fixture.folder.join("renamed.bin")).unwrap(),
                _ => fs::create_dir(fixture.folder.join("new-subdir")).unwrap(),
            }
            assert!(builder.finish().is_err(), "{change}");
            assert_eq!(fs::read(output).unwrap(), b"copy");
        }
    }

    #[test]
    fn section1373_existing_and_generated_same_size_same_second_edit_is_detected() {
        for edit_generated in [false, true] {
            let fixture = Fixture::new();
            let existing = fixture.folder.join("existing.bin");
            fs::write(&existing, b"old!").unwrap();
            let set_time = |path: &Path, nanos| {
                fs::File::options()
                    .write(true)
                    .open(path)
                    .unwrap()
                    .set_times(fs::FileTimes::new().set_modified(
                        SystemTime::UNIX_EPOCH + std::time::Duration::new(1_700_000_000, nanos),
                    ))
                    .unwrap()
            };
            set_time(&existing, 100_000);
            let mut builder =
                AppendReceiptBuilder::begin(&fixture.root, &fixture.folder, "book".into()).unwrap();
            let output = fixture.folder.join("0001_copy.jpg");
            fs::write(&output, b"copy").unwrap();
            set_time(&output, 100_000);
            builder.record(&output).unwrap();
            let receipt = builder.finish().unwrap();
            let edited = if edit_generated { &output } else { &existing };
            fs::write(edited, b"edit").unwrap();
            set_time(edited, 300_000);
            assert!(receipt.verify_current().is_err());
        }
    }

    #[test]
    fn section1373_metadata_terminal_rechecks_physical_composition() {
        let fixture = Fixture::new();
        let receipt = fixture.receipt(1);
        let mut summary = crate::books::BookAppendSummary {
            book_name: "book".into(),
            folder: fixture.folder.clone(),
            added: 1,
            first_path: Some(receipt.files.created[0].path.clone()),
            edit_copies: Vec::new(),
            semantic_copies: Vec::new(),
            erase_fallback_pages: 0,
            undo_files: Some(receipt.files),
            metadata_copy: Default::default(),
        };
        fs::write(fixture.folder.join("physical-sidecar"), b"external").unwrap();
        copy_append_metadata_at(
            fixture.tmp.path(),
            &mut summary,
            AppendMetadataStores::default(),
        );
        assert!(summary.undo_files.is_none());
        assert_eq!(summary.added, 1);
        assert!(summary.first_path.unwrap().exists());
    }

    #[test]
    fn section1373_fresh_refresh_snapshot_cannot_adopt_later_external_difference() {
        let fixture = Fixture::new();
        let receipt = fixture.receipt(1);
        let exact = capture_book_composition(&fixture.folder).unwrap();
        receipt.files.verify_composition(&exact).unwrap();
        fs::write(fixture.folder.join("later.bin"), b"late").unwrap();
        let latest = capture_book_composition(&fixture.folder).unwrap();
        assert!(receipt.files.verify_composition(&latest).is_err());
    }

    #[test]
    fn section1373_empty_receipt_and_outputs_outside_direct_book_are_rejected() {
        let fixture = Fixture::new();
        assert!(
            AppendReceiptBuilder::begin(&fixture.root, &fixture.folder, "book".into())
                .unwrap()
                .finish()
                .is_err()
        );
        let mut builder =
            AppendReceiptBuilder::begin(&fixture.root, &fixture.folder, "book".into()).unwrap();
        assert!(builder.record(&fixture.source).is_err());
        let nested = fixture.folder.join("nested");
        fs::create_dir(&nested).unwrap();
        assert!(AppendReceiptBuilder::begin(&fixture.root, &nested, "nested".into()).is_err());
    }

    #[test]
    fn section1373_duplicate_or_retargeted_receipt_paths_are_rejected() {
        let fixture = Fixture::new();
        let mut receipt = fixture.receipt(1);
        receipt.files.created.push(receipt.files.created[0].clone());
        assert!(receipt.files.verify_current().is_err());
        receipt.files.created.pop();
        receipt.files.created[0].path = fixture.source.clone();
        assert!(receipt.files.verify_current().is_err());
    }

    #[test]
    fn section1373_all_targets_verify_before_any_shell_or_purge_call() {
        let fixture = Fixture::new();
        let receipt = fixture.receipt(2);
        fs::write(&receipt.files.created[1].path, b"changed second copy").unwrap();
        let result = undo_append_with(
            fixture.tmp.path(),
            receipt,
            |_| panic!("no Shell operation may run before all outputs match"),
            |_, _| panic!("no purge may run before verification"),
        );
        assert!(result.is_err());
        assert!(fixture.folder.join("0001_page.jpg").exists());
    }

    #[test]
    fn section1373_partial_recycle_purges_only_exact_confirmed_successes() {
        let fixture = Fixture::new();
        let receipt = fixture.receipt(2);
        let first = receipt.files.created[0].path.clone();
        let second = receipt.files.created[1].path.clone();
        let result = undo_append_with(
            fixture.tmp.path(),
            receipt,
            |paths| {
                assert_eq!(paths, [first.clone(), second.clone()]);
                fs::remove_file(&first).unwrap();
                crate::delete_worker::DeleteChunkOutcome {
                    succeeded: vec![first.clone()],
                    failed: vec![(second.clone(), "cannot recycle".into())],
                    shell_aborted: false,
                    retryable: false,
                }
            },
            |scopes, _| {
                assert_eq!(
                    scopes,
                    [crate::delete_worker::DeleteSourceScope::Exact(
                        first.clone()
                    )]
                );
                crate::rename_key_migration::PurgeReport::default()
            },
        )
        .unwrap();
        assert_eq!(result.succeeded, [first]);
        assert_eq!(result.failed.len(), 1);
        assert!(second.exists());
        assert!(fixture.source.exists());
        assert!(fixture.folder.exists());
    }

    #[test]
    fn section1373_no_confirmed_recycle_means_no_purge() {
        let fixture = Fixture::new();
        let receipt = fixture.receipt(1);
        let result = undo_append_with(
            fixture.tmp.path(),
            receipt,
            |paths| {
                crate::delete_worker::DeleteChunkOutcome::all_failed(paths, "unconfirmed".into())
            },
            |_, _| panic!("no success, no purge"),
        )
        .unwrap();
        assert!(result.succeeded.is_empty());
        assert_eq!(result.failed.len(), 1);
    }

    #[test]
    fn section1373_metadata_copy_preserves_full_edit_and_semantic_only_routes() {
        let fixture = Fixture::new();
        let data = fixture.tmp.path().join("data");
        fs::create_dir(&data).unwrap();
        let receipt = fixture.receipt(2);
        let source = crate::adjustment_db::normalize_path(&fixture.source);
        let full = crate::adjustment_db::normalize_path(&receipt.files.created[0].path);
        let semantic = crate::adjustment_db::normalize_path(&receipt.files.created[1].path);
        let adjustments =
            crate::adjustment_db::AdjustmentDb::open_at(&data.join("adjustment.db")).unwrap();
        adjustments
            .set_page_params(&source, &crate::adjustment::AdjustParams::default())
            .unwrap();
        let ratings = crate::rating_db::RatingDb::open_at(data.join("rating.db")).unwrap();
        ratings.set(&source, 4).unwrap();
        let mut tags = crate::tags_db::TagsDb::open_at(&data.join("tags.db")).unwrap();
        tags.set_item_tags(&source, ["copied"], "test").unwrap();
        let mut summary = crate::books::BookAppendSummary {
            book_name: "book".into(),
            folder: fixture.folder.clone(),
            added: 2,
            first_path: Some(receipt.files.created[0].path.clone()),
            edit_copies: vec![crate::books::BookPathMapping {
                from: fixture.source.clone(),
                to: receipt.files.created[0].path.clone(),
            }],
            semantic_copies: vec![crate::books::BookPathMapping {
                from: fixture.source.clone(),
                to: receipt.files.created[1].path.clone(),
            }],
            erase_fallback_pages: 0,
            undo_files: Some(receipt.files),
            metadata_copy: Default::default(),
        };
        copy_append_metadata_at(
            &data,
            &mut summary,
            AppendMetadataStores {
                adjustment: true,
                rating: true,
                tags: true,
                ..Default::default()
            },
        );
        assert!(
            summary.metadata_copy.errors.is_empty(),
            "{:?}",
            summary.metadata_copy.errors
        );
        assert_eq!(summary.metadata_copy.adjusted, [full.clone()]);
        assert!(adjustments.get_page_params(&full).is_some());
        assert!(adjustments.get_page_params(&semantic).is_none());
        assert_eq!(ratings.get(&full), 4);
        assert_eq!(ratings.get(&semantic), 4);
        assert_eq!(summary.metadata_copy.ratings.get(&full), Some(&4));
        assert_eq!(summary.metadata_copy.ratings.get(&semantic), Some(&4));
        let copied_tags = tags.get_many_display_tags(&[full, semantic]);
        assert_eq!(summary.metadata_copy.tags, copied_tags);
        assert_eq!(copied_tags.len(), 2);
        assert!(
            copied_tags
                .values()
                .all(|tags| tags.iter().map(String::as_str).eq(["#copied"]))
        );
        assert!(summary.undo_files.is_some());
    }

    #[test]
    fn section1373_missing_metadata_store_disables_receipt_without_recreating_data() {
        let fixture = Fixture::new();
        let data = fixture.tmp.path().join("missing-data");
        let receipt = fixture.receipt(1);
        let mut summary = crate::books::BookAppendSummary {
            book_name: "book".into(),
            folder: fixture.folder.clone(),
            added: 1,
            first_path: Some(receipt.files.created[0].path.clone()),
            edit_copies: vec![crate::books::BookPathMapping {
                from: fixture.source.clone(),
                to: receipt.files.created[0].path.clone(),
            }],
            semantic_copies: Vec::new(),
            erase_fallback_pages: 0,
            undo_files: Some(receipt.files),
            metadata_copy: Default::default(),
        };
        copy_append_metadata_at(
            &data,
            &mut summary,
            AppendMetadataStores {
                adjustment: true,
                ..Default::default()
            },
        );
        assert_eq!(summary.metadata_copy.errors.len(), 1);
        assert!(summary.undo_files.is_none());
        assert!(!data.exists());
        assert!(summary.first_path.unwrap().exists());
        assert!(fixture.source.exists());
    }
}
