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
    /// All fields describe changed keys, including destinations cleared to empty.
    pub adjusted: Vec<String>,
    pub local_adjusted: Vec<String>,
    pub cropped: Vec<String>,
    pub erased: Vec<String>,
    pub concealed: Vec<String>,
    pub annotated: Vec<String>,
    /// Changed destination keys only. Values and presence must be read from
    /// the current stores, since users can edit a page before this report polls.
    pub ratings: Vec<String>,
    pub tags: Vec<String>,
    pub errors: Vec<String>,
    pub database_elapsed: std::time::Duration,
}

impl AppendMetadataStores {
    /// Worker-only discovery for entry points without an App store snapshot.
    pub(crate) fn existing_at(data_dir: &Path) -> Self {
        Self {
            adjustment: data_dir.join("adjustment.db").is_file(),
            local_adjust: data_dir.join("local_adjust.db").is_file(),
            crop: data_dir.join("export_crop.db").is_file(),
            mask: data_dir.join("mask.db").is_file(),
            conceal: data_dir.join("conceal.db").is_file(),
            comic: data_dir.join("comic.db").is_file(),
            rating: data_dir.join("rating.db").is_file(),
            tags: data_dir.join("tags.db").is_file(),
        }
    }
}

/// Connections belong to the existing Append worker and are reused for every
/// destination. Metadata commits precede creation of the discoverable image.
/// There is no later metadata initializer that can overwrite a user's edit.
pub(crate) struct AppendMetadataWriter {
    adjustment: Option<crate::adjustment_db::AdjustmentDb>,
    local_adjust: Option<crate::local_adjust_db::LocalAdjustDb>,
    crop: Option<crate::export_crop::CropDb>,
    mask: Option<crate::mask_db::MaskDb>,
    conceal: Option<crate::conceal_db::ConcealDb>,
    comic: Option<crate::comic_db::ComicDb>,
    rating: Option<crate::rating_db::RatingDb>,
    tags: Option<crate::tags_db::TagsDb>,
    report: BookAppendMetadataCopySummary,
}

impl AppendMetadataWriter {
    pub(crate) fn open_at(data_dir: &Path, stores: AppendMetadataStores) -> Result<Self, String> {
        let started = std::time::Instant::now();
        macro_rules! open_store {
            ($enabled:expr, $name:literal, $db:ty) => {
                if $enabled {
                    let path = data_dir.join(concat!($name, ".db"));
                    // A captured store must still exist: never recreate lost data.
                    fs::metadata(&path)
                        .map_err(|error| metadata_initialization_error($name, error))?;
                    Some(
                        <$db>::open_at(&path)
                            .map_err(|error| metadata_initialization_error($name, error))?,
                    )
                } else {
                    None
                }
            };
        }
        let mut writer = Self {
            adjustment: open_store!(
                stores.adjustment,
                "adjustment",
                crate::adjustment_db::AdjustmentDb
            ),
            local_adjust: open_store!(
                stores.local_adjust,
                "local_adjust",
                crate::local_adjust_db::LocalAdjustDb
            ),
            crop: open_store!(stores.crop, "export_crop", crate::export_crop::CropDb),
            mask: open_store!(stores.mask, "mask", crate::mask_db::MaskDb),
            conceal: open_store!(stores.conceal, "conceal", crate::conceal_db::ConcealDb),
            comic: open_store!(stores.comic, "comic", crate::comic_db::ComicDb),
            rating: open_store!(stores.rating, "rating", crate::rating_db::RatingDb),
            tags: open_store!(stores.tags, "tags", crate::tags_db::TagsDb),
            report: Default::default(),
        };
        writer.report.database_elapsed = started.elapsed();
        Ok(writer)
    }

    pub(crate) fn prepare_destination(
        &mut self,
        destination: &Path,
        full_source: Option<&Path>,
        semantic_source: Option<&Path>,
    ) -> Result<(), String> {
        let started = std::time::Instant::now();
        let to = crate::adjustment_db::normalize_path(destination);
        let full = full_source.map(crate::adjustment_db::normalize_path);
        let semantic = semantic_source
            .or(full_source)
            .map(crate::adjustment_db::normalize_path);
        // Preserve each store's global write epoch and publish all eight
        // committed initial values before the physical file becomes visible.
        let has_edit_store = self.adjustment.is_some()
            || self.local_adjust.is_some()
            || self.crop.is_some()
            || self.mask.is_some()
            || self.conceal.is_some()
            || self.comic.is_some();
        let _edit_epoch =
            has_edit_store.then(|| crate::page_edit_write_epoch::PAGE_EDIT_WRITES.begin());
        let _rating_epoch = self
            .rating
            .as_ref()
            .map(|_| crate::rating_db::RATING_WRITES.begin());
        let _tag_epoch = self
            .tags
            .as_ref()
            .map(|_| crate::tags_db::TAG_WRITES.begin());
        macro_rules! initialize_store {
            ($field:ident, $name:literal, $clear:ident, $copy:ident, $source:expr, $changed:ident) => {
                if let Some(db) = self.$field.as_mut() {
                    db.$clear(&to)
                        .map_err(|error| metadata_initialization_error($name, error))?;
                    if let Some(from) = $source.as_deref() {
                        db.$copy(from, &to)
                            .map_err(|error| metadata_initialization_error($name, error))?;
                    }
                    self.report.$changed.push(to.clone());
                }
            };
        }
        initialize_store!(
            adjustment,
            "adjustment",
            remove_page_params,
            copy_page_params_key,
            full,
            adjusted
        );
        initialize_store!(
            local_adjust,
            "local_adjust",
            remove_layers,
            copy_entry_key,
            full,
            local_adjusted
        );
        initialize_store!(crop, "export_crop", remove, copy_entry_key, full, cropped);
        initialize_store!(mask, "mask", delete, copy_entry_key, full, erased);
        initialize_store!(conceal, "conceal", delete, copy_entry_key, full, concealed);
        initialize_store!(comic, "comic", delete, copy_entry_key, full, annotated);
        if let Some(db) = self.rating.as_mut() {
            db.set(&to, 0)
                .map_err(|error| metadata_initialization_error("rating", error))?;
            if let Some(from) = semantic.as_deref() {
                db.copy_entry_key(from, &to)
                    .map_err(|error| metadata_initialization_error("rating", error))?;
            }
            self.report.ratings.push(to.clone());
        }
        if let Some(db) = self.tags.as_mut() {
            if let Some(from) = semantic.as_deref() {
                db.copy_item_key(from, &to)
                    .map_err(|error| metadata_initialization_error("tags", error))?;
            } else {
                // Match a newly numbered page's original unset-tag semantics;
                // do not create an EDIT decision for a source without tags.
                db.prune_items(std::slice::from_ref(&to))
                    .map_err(|error| metadata_initialization_error("tags", error))?;
            }
            self.report.tags.push(to);
        }
        self.report.database_elapsed += started.elapsed();
        Ok(())
    }

    pub(crate) fn finish(self) -> BookAppendMetadataCopySummary {
        self.report
    }
}

fn metadata_initialization_error(store: &str, error: impl std::fmt::Display) -> String {
    let error = format!("ページのメタデータを初期化できません: {store}: {error}");
    crate::logger::log(format!("[book-append-undo] {error}"));
    error
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
            skipped_hidden: 0,
            undo_files: Some(receipt.files),
            metadata_copy: Default::default(),
        };
        fs::write(fixture.folder.join("physical-sidecar"), b"external").unwrap();
        finalize_append_receipt(&mut summary);
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
        let mut dbs = append_test_support::TestMetadata::open(&data);
        let source = crate::adjustment_db::normalize_path(&fixture.source);
        dbs.set_all(&source, 4);
        let full_path = fixture.folder.join("0001_full.jpg");
        let semantic_path = fixture.folder.join("0002_semantic.jpg");
        let full = crate::adjustment_db::normalize_path(&full_path);
        let semantic = crate::adjustment_db::normalize_path(&semantic_path);
        // Old rows for nonexistent destinations may be overwritten. Semantic
        // copies must clear baked image edits rather than applying them twice.
        dbs.set_all(&full, 2);
        dbs.set_all(&semantic, 2);
        let mut writer = AppendMetadataWriter::open_at(&data, dbs.stores()).unwrap();
        writer
            .prepare_destination(&full_path, Some(&fixture.source), None)
            .unwrap();
        writer
            .prepare_destination(&semantic_path, None, Some(&fixture.source))
            .unwrap();
        assert!(!full_path.exists());
        assert!(!semantic_path.exists());
        dbs.assert_all(&full, Some(4));
        dbs.assert_edits(&semantic, None);
        assert_eq!(dbs.rating.get(&semantic), 4);
        assert_eq!(dbs.tags.display_tags_for_item(&semantic), ["#v4"]);
        let report = writer.finish();
        assert_eq!(report.adjusted, [full.clone(), semantic.clone()]);
        assert_eq!(report.ratings, [full.clone(), semantic.clone()]);
        assert_eq!(report.tags, [full, semantic]);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn section1373_metadata_database_cold_and_warm_per_file_benchmark() {
        use std::time::Instant;
        let fixture = Fixture::new();
        let data = fixture.tmp.path().join("benchmark-data");
        let mut dbs = append_test_support::TestMetadata::open(&data);
        let source = crate::adjustment_db::normalize_path(&fixture.source);
        dbs.set_all(&source, 4);
        for count in [1usize, 100, 1000] {
            let cold_started = Instant::now();
            let mut writer = AppendMetadataWriter::open_at(&data, dbs.stores()).unwrap();
            let cold = cold_started.elapsed();
            writer
                .prepare_destination(
                    &fixture.folder.join(format!("warm-{count}.jpg")),
                    Some(&fixture.source),
                    None,
                )
                .unwrap();
            let started = Instant::now();
            for number in 0..count {
                writer
                    .prepare_destination(
                        &fixture.folder.join(format!("r7-{count}-{number}.jpg")),
                        Some(&fixture.source),
                        None,
                    )
                    .unwrap();
            }
            let current = started.elapsed();
            // Old equivalent: one connection per store per append, no per-file
            // destination reset, store-at-a-time copies and bulk report reads.
            let baseline_open = Instant::now();
            let mut baseline = append_test_support::TestMetadata::open(&data);
            let baseline_cold = baseline_open.elapsed();
            baseline
                .tags
                .copy_item_key(&source, &format!("baseline-warm-{count}"))
                .unwrap();
            let keys = (0..count)
                .map(|number| {
                    crate::adjustment_db::normalize_path(
                        &fixture.folder.join(format!("old-{count}-{number}.jpg")),
                    )
                })
                .collect::<Vec<_>>();
            let key_refs = keys.iter().map(String::as_str).collect::<Vec<_>>();
            let baseline_started = Instant::now();
            for key in &keys {
                baseline
                    .adjustment
                    .copy_page_params_key(&source, key)
                    .unwrap();
            }
            for key in &keys {
                baseline.local_adjust.copy_entry_key(&source, key).unwrap();
            }
            for key in &keys {
                baseline.crop.copy_entry_key(&source, key).unwrap();
            }
            for key in &keys {
                baseline.mask.copy_entry_key(&source, key).unwrap();
            }
            for key in &keys {
                baseline.conceal.copy_entry_key(&source, key).unwrap();
            }
            for key in &keys {
                baseline.comic.copy_entry_key(&source, key).unwrap();
            }
            for key in &keys {
                baseline.rating.copy_entry_key(&source, key).unwrap();
            }
            for key in &keys {
                baseline.tags.copy_item_key(&source, key).unwrap();
            }
            assert_eq!(
                baseline.adjustment.load_page_params_many(&key_refs).len(),
                count
            );
            assert_eq!(
                baseline.local_adjust.load_existing_layer_keys(&keys).len(),
                count
            );
            assert_eq!(baseline.crop.load_many(&key_refs).len(), count);
            assert_eq!(
                baseline.mask.load_existing_mask_keys(&key_refs).len(),
                count
            );
            assert_eq!(
                baseline.conceal.load_existing_conceal_keys(&key_refs).len(),
                count
            );
            assert_eq!(
                baseline.comic.load_existing_comic_keys(&key_refs).len(),
                count
            );
            assert_eq!(baseline.rating.get_many(&keys).len(), count);
            assert_eq!(baseline.tags.get_many_display_tags(&keys).len(), count);
            let old = baseline_started.elapsed();
            eprintln!(
                "section1373 DB files={count} r7_cold_open_ms={:.3} old_cold_open_ms={:.3} r7_warm_total_ms={:.3} r7_warm_us_per_file={:.3} old_warm_total_ms={:.3} old_warm_us_per_file={:.3} delta_us_per_file={:.3}",
                cold.as_secs_f64() * 1e3,
                baseline_cold.as_secs_f64() * 1e3,
                current.as_secs_f64() * 1e3,
                current.as_secs_f64() * 1e6 / count as f64,
                old.as_secs_f64() * 1e3,
                old.as_secs_f64() * 1e6 / count as f64,
                (current.as_secs_f64() - old.as_secs_f64()) * 1e6 / count as f64,
            );
        }
    }

    #[test]
    fn section1373_missing_metadata_store_fails_before_output_without_recreating_data() {
        let fixture = Fixture::new();
        let data = fixture.tmp.path().join("missing-data");
        let result = AppendMetadataWriter::open_at(
            &data,
            AppendMetadataStores {
                adjustment: true,
                ..Default::default()
            },
        );
        assert!(result.is_err());
        assert!(!data.exists());
        assert!(fixture.source.exists());
    }

    #[test]
    fn section1373_plain_page_initialization_preserves_unset_tags_and_measures_database_cost() {
        use std::time::Instant;
        let fixture = Fixture::new();
        let data = fixture.tmp.path().join("plain-perf-data");
        let mut dbs = append_test_support::TestMetadata::open(&data);
        let old_key = crate::adjustment_db::normalize_path(&fixture.folder.join("orphan.jpg"));
        dbs.set_all(&old_key, 3);
        let mut writer = AppendMetadataWriter::open_at(&data, dbs.stores()).unwrap();
        writer
            .prepare_destination(&fixture.folder.join("orphan.jpg"), None, None)
            .unwrap();
        dbs.assert_all(&old_key, None);
        assert!(!dbs.tags.has_item_state(&old_key));
        for count in [1usize, 100, 1000] {
            let started = Instant::now();
            for index in 0..count {
                writer
                    .prepare_destination(
                        &fixture.folder.join(format!("plain-{count}-{index}.jpg")),
                        None,
                        None,
                    )
                    .unwrap();
            }
            let elapsed = started.elapsed();
            eprintln!(
                "section1373 DB plain files={count} total_ms={:.3} us_per_file={:.3} old_copy_ms=0 (no mapping)",
                elapsed.as_secs_f64() * 1e3,
                elapsed.as_secs_f64() * 1e6 / count as f64
            );
        }
    }
}

#[cfg(test)]
pub(crate) mod append_test_support {
    use super::*;

    pub(crate) struct TestMetadata {
        pub adjustment: crate::adjustment_db::AdjustmentDb,
        pub local_adjust: crate::local_adjust_db::LocalAdjustDb,
        pub crop: crate::export_crop::CropDb,
        pub mask: crate::mask_db::MaskDb,
        pub conceal: crate::conceal_db::ConcealDb,
        pub comic: crate::comic_db::ComicDb,
        pub rating: crate::rating_db::RatingDb,
        pub tags: crate::tags_db::TagsDb,
    }

    impl TestMetadata {
        pub(crate) fn open(data: &Path) -> Self {
            fs::create_dir_all(data).unwrap();
            Self {
                adjustment: crate::adjustment_db::AdjustmentDb::open_at(
                    &data.join("adjustment.db"),
                )
                .unwrap(),
                local_adjust: crate::local_adjust_db::LocalAdjustDb::open_at(
                    &data.join("local_adjust.db"),
                )
                .unwrap(),
                crop: crate::export_crop::CropDb::open_at(&data.join("export_crop.db")).unwrap(),
                mask: crate::mask_db::MaskDb::open_at(&data.join("mask.db")).unwrap(),
                conceal: crate::conceal_db::ConcealDb::open_at(&data.join("conceal.db")).unwrap(),
                comic: crate::comic_db::ComicDb::open_at(&data.join("comic.db")).unwrap(),
                rating: crate::rating_db::RatingDb::open_at(data.join("rating.db")).unwrap(),
                tags: crate::tags_db::TagsDb::open_at(&data.join("tags.db")).unwrap(),
            }
        }

        pub(crate) fn stores(&self) -> AppendMetadataStores {
            AppendMetadataStores {
                adjustment: true,
                local_adjust: true,
                crop: true,
                mask: true,
                conceal: true,
                comic: true,
                rating: true,
                tags: true,
            }
        }

        pub(crate) fn set_all(&mut self, key: &str, version: u8) {
            let params = crate::adjustment::AdjustParams {
                brightness: version as f32,
                ..Default::default()
            };
            self.adjustment.set_page_params(key, &params).unwrap();
            self.local_adjust
                .set_layers(
                    key,
                    &[local_adjust_core::LocalAdjustmentLayer::new(
                        format!("v{version}"),
                        local_adjust_core::LocalMask::Full,
                        local_adjust_core::LocalEffect::None,
                    )],
                )
                .unwrap();
            self.crop
                .set(
                    key,
                    crate::export_crop::CropSettings {
                        rect: crate::export_crop::CropRect {
                            min_x: version as f32 / 10.0,
                            min_y: 0.0,
                            max_x: 0.9,
                            max_y: 0.9,
                        },
                        aspect_mode: Default::default(),
                        source_size: Some([2, 1]),
                    },
                )
                .unwrap();
            let bitmap = if version % 2 == 0 {
                [true, false]
            } else {
                [false, true]
            };
            self.mask.set(key, &bitmap, &[], 2, 1).unwrap();
            self.conceal.set(key, &bitmap, &[], 2, 1).unwrap();
            self.comic
                .set_raw(
                    key,
                    crate::comic_db::DOC_VERSION,
                    &format!("[{{\"version\":{version}}}]"),
                )
                .unwrap();
            self.rating.set(key, version).unwrap();
            self.tags
                .set_item_tags(key, [format!("v{version}")], "test")
                .unwrap();
        }

        pub(crate) fn clear_all(&mut self, key: &str) {
            self.adjustment.remove_page_params(key).unwrap();
            self.local_adjust.remove_layers(key).unwrap();
            self.crop.remove(key).unwrap();
            self.mask.delete(key).unwrap();
            self.conceal.delete(key).unwrap();
            self.comic.delete(key).unwrap();
            self.rating.set(key, 0).unwrap();
            self.tags.clear_item_tags(key).unwrap();
        }

        pub(crate) fn assert_edits(&self, key: &str, version: Option<u8>) {
            if let Some(version) = version {
                assert_eq!(
                    self.adjustment.get_page_params(key).unwrap().brightness,
                    version as f32
                );
                assert_eq!(
                    self.local_adjust.get_layers(key).unwrap()[0].name,
                    format!("v{version}")
                );
                assert_eq!(
                    self.crop.get(key).unwrap().rect.min_x,
                    version as f32 / 10.0
                );
                let bitmap = if version % 2 == 0 {
                    vec![true, false]
                } else {
                    vec![false, true]
                };
                assert_eq!(self.mask.get_full(key, 2, 1).unwrap().0, bitmap);
                assert_eq!(self.conceal.get_full(key, 2, 1).unwrap().0, bitmap);
                assert_eq!(
                    self.comic.get_raw(key).unwrap().1,
                    format!("[{{\"version\":{version}}}]")
                );
            } else {
                assert!(self.adjustment.get_page_params(key).is_none());
                assert!(self.local_adjust.get_layers(key).is_none());
                assert!(self.crop.get(key).is_none());
                assert!(self.mask.get_full(key, 2, 1).is_none());
                assert!(self.conceal.get_full(key, 2, 1).is_none());
                assert!(self.comic.get_raw(key).is_none());
            }
        }

        pub(crate) fn assert_all(&self, key: &str, version: Option<u8>) {
            self.assert_edits(key, version);
            assert_eq!(self.rating.get(key), version.unwrap_or(0));
            let expected = version.map(|v| vec![format!("#v{v}")]).unwrap_or_default();
            assert_eq!(self.tags.display_tags_for_item(key), expected);
        }
    }
}
