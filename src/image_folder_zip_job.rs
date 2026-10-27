//! The modal operation's single worker. Leases/resources stay here through
//! confirmation; each terminal reply returns resources before the UI releases it.
use crate::image_folder_zip::{ConversionControl, ImageFolderManifest};
use crate::image_folder_zip_metadata::{self as metadata, CopyPhase, CopyPortal};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};

pub(crate) struct Resources {
    pub sidecars: HashMap<PathBuf, crate::sidecar::SidecarFile>,
    pub tags: Option<crate::tags_db::TagsDb>,
    pub saved_trims: usize,
    pub source_keys: std::collections::HashSet<String>,
    pub source_edits: std::collections::HashSet<String>,
    proofs: HashMap<PathBuf, Option<crate::sidecar::SidecarImportSource>>,
}
pub(crate) struct Inputs {
    pub data_dir: PathBuf,
    pub manifest: ImageFolderManifest,
    pub settings: Box<crate::settings::Settings>,
    pub session_ratings: HashMap<String, u8>,
    pub portals: Vec<CopyPortal>,
    pub favorite_pairs: Vec<(
        crate::settings::FavoriteEntry,
        crate::settings::FavoriteEntry,
    )>,
    pub trims: Vec<crate::view_trim_db::ViewTrimWriteBatch>,
    pub observer: Option<crate::content_identity::ContentIdentityProducer>,
}
#[derive(Clone)]
pub(crate) struct Summary {
    pub images: usize,
    pub bytes: u64,
    pub auxiliary: usize,
    pub rows: usize,
}
#[derive(Debug)]
pub(crate) struct Success {
    pub favorites: Vec<crate::settings::FavoriteEntry>,
    pub favorite_params: Vec<(uuid::Uuid, crate::adjustment::AdjustParams)>,
}
pub(crate) enum Command {
    Convert,
    Cancel,
}
pub(crate) enum Event {
    Confirm(Summary),
    Progress {
        images: usize,
        bytes: u64,
    },
    Committing,
    Finished {
        result: Result<Success, String>,
        source: PathBuf,
        destination: PathBuf,
    },
}
pub(crate) struct Worker {
    pub commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
    pub resources: Arc<Mutex<Resources>>,
}

pub(crate) fn spawn(
    input: Inputs,
    resources: Arc<Mutex<Resources>>,
    control: Arc<ConversionControl>,
    wake: egui::Context,
) -> Result<Worker, String> {
    let (commands, rx) = mpsc::channel();
    let (tx, events) = mpsc::sync_channel(1);
    let retained = Arc::clone(&resources);
    let data_dir = input.data_dir.clone();
    let source = input.manifest.source.clone();
    let destination = input.manifest.destination.clone();
    #[cfg(test)]
    let test_epoch = crate::page_edit_write_epoch::TestEpochScope::capture();
    std::thread::Builder::new()
        .name("image-folder-zip".into())
        .spawn(move || {
            #[cfg(test)]
            let _test_epoch = test_epoch.map(crate::page_edit_write_epoch::TestEpochScope::enter);
            let mut resources = retained.lock().unwrap_or_else(|e| e.into_inner());
            let notify = |event| {
                if matches!(&event, Event::Progress { .. }) {
                    let _ = tx.try_send(event);
                } else {
                    let _ = tx.send(event);
                }
                wake.request_repaint_of(egui::ViewportId::ROOT);
            };
            let result = run(input, &mut resources, &control, &rx, &notify);
            if let Err(error) = &result {
                crate::logger::log(format!("image folder ZIP {}: {error}", source.display()));
            }
            // Both success and ordinary errors reopen the handed-off tag connection.
            if resources.tags.is_none() {
                match crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")) {
                    Ok(tags) => resources.tags = Some(tags),
                    Err(e) => crate::logger::log(format!("image ZIP tag DB reopen: {e}")),
                }
            }
            drop(resources);
            notify(Event::Finished {
                result,
                source,
                destination,
            });
        })
        .map_err(|e| e.to_string())?;
    Ok(Worker {
        commands,
        events,
        resources,
    })
}

impl Resources {
    pub(crate) fn new(
        sidecars: HashMap<PathBuf, crate::sidecar::SidecarFile>,
        tags: Option<crate::tags_db::TagsDb>,
    ) -> Self {
        Self {
            sidecars,
            tags,
            saved_trims: 0,
            source_keys: Default::default(),
            source_edits: Default::default(),
            proofs: Default::default(),
        }
    }
}
fn validate_proofs(resources: &Resources) -> Result<(), String> {
    for (path, proof) in &resources.proofs {
        if let Some(proof) = proof {
            crate::sidecar::revalidate_import_source(&resources.sidecars[path], proof)?;
        } else {
            crate::sidecar::revalidate_missing_import_source(path)?;
        }
    }
    Ok(())
}
fn validate_source_directory(
    resources: &mut Resources,
    manifest: &ImageFolderManifest,
) -> Result<(), String> {
    let result = manifest.revalidate_source_directory();
    if result.is_err()
        && let Some(sidecar) = resources.sidecars.get_mut(&manifest.source)
    {
        // Preserve the captured edits without later writing them into a different directory.
        sidecar.disable_writes_for_session();
    }
    result
}
fn load_parent(
    resources: &mut Resources,
    source: &std::path::Path,
    data_dir: &std::path::Path,
) -> Result<(), String> {
    let parent = source.parent().ok_or("parent missing")?;
    let (sidecar, proof) = match crate::sidecar::SidecarFile::load_for_import(parent) {
        crate::sidecar::SidecarImportLoad::Loaded(loaded) => {
            let (sidecar, proof) = loaded.into_parts();
            (sidecar, Some(proof))
        }
        crate::sidecar::SidecarImportLoad::Missing { sidecar } => (sidecar, None),
        _ => return Err("parent saved data unreadable".into()),
    };
    if let Some(entry) = source
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| sidecar.items().get(&n.to_lowercase()))
    {
        if let Some(tags) = &entry.tags {
            let key = crate::path_key::normalize_keep_drive(source);
            if let Some(item) = crate::tags_db::prepare_sidecar_tag_item(key.clone(), tags)? {
                if let Some(proof) = &proof {
                    crate::sidecar::revalidate_import_source(&sidecar, proof)?;
                }
                let mut db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db"))
                    .map_err(|e| e.to_string())?;
                let folder_key = crate::path_key::normalize_keep_drive(parent);
                // This is only F's body entry, not a full parent import. Respect
                // the receipt of a consumed disk, but never mark the partial import
                // as consuming unrelated parent entries.
                let consumed =
                    if let Some(crate::sidecar::SidecarImportSource::Disk(token)) = &proof {
                        use rusqlite::OptionalExtension;
                        let conn = rusqlite::Connection::open_with_flags(
                            data_dir.join("tags.db"),
                            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                        )
                        .map_err(|e| e.to_string())?;
                        conn.query_row(
                            "SELECT sidecar_mtime FROM tag_sidecar_sync WHERE folder_key=?1",
                            [&folder_key],
                            |r| r.get::<_, i64>(0),
                        )
                        .optional()
                        .map_err(|e| e.to_string())?
                            == Some(token.sync_marker())
                    } else {
                        false
                    };
                if !consumed {
                    let result = db
                        .import_sidecar_tags_atomic(
                            &folder_key,
                            None,
                            &[item],
                            &AtomicBool::new(false),
                        )
                        .map_err(|e| e.to_string())?;
                    if let crate::tags_db::SidecarTagBatchOutcome::Applied(report) = result {
                        resources
                            .source_keys
                            .extend(report.imported.into_iter().map(|(key, _)| key));
                    }
                }
            }
        }
    }
    resources.sidecars.insert(parent.to_owned(), sidecar);
    resources.proofs.insert(parent.to_owned(), proof);
    Ok(())
}

fn wait_portals(
    portals: &[CopyPortal],
    manifest: &ImageFolderManifest,
    phase: CopyPhase,
    control: &ConversionControl,
) -> Result<usize, String> {
    let mut count = 0;
    for portal in portals {
        control.check()?;
        // Once accepted, a copy/check receipt must be drained before releasing
        // the resources. Cancellation prevents the next request, not this wait.
        count += portal(manifest, phase)
            .recv()
            .map_err(|_| "保存処理が終了しました。元のフォルダは残っています。")??;
        control.check()?;
    }
    Ok(count)
}

fn restore_source(
    resources: &mut Resources,
    source: &std::path::Path,
    data_dir: &std::path::Path,
) -> Result<(), String> {
    match crate::sidecar::SidecarFile::load_for_import(source) {
        crate::sidecar::SidecarImportLoad::Loaded(loaded) => {
            let cancel = AtomicBool::new(false);
            let prepared = crate::sidecar_import::prepare(
                loaded,
                crate::sidecar_import::ImportFamilies::ALL,
                &cancel,
            )?;
            match crate::sidecar_import::commit(data_dir, prepared, &cancel) {
                crate::sidecar_import::SidecarImportCompletion::Current { sidecar, result } => {
                    use crate::sidecar_import::ImportFamilyOutcome;
                    if let crate::sidecar_import::ImportFamilyOutcome::Applied(report) =
                        &result.edits
                    {
                        for keys in [
                            &report.delta.adjusted,
                            &report.delta.masked,
                            &report.delta.concealed,
                            &report.delta.local_adjusted,
                            &report.delta.cropped,
                            &report.delta.comic,
                        ] {
                            resources.source_keys.extend(keys.iter().cloned());
                            resources.source_edits.extend(keys.iter().cloned());
                        }
                    }
                    if let crate::sidecar_import::ImportFamilyOutcome::Applied(report) =
                        &result.tags
                    {
                        resources
                            .source_keys
                            .extend(report.imported.iter().map(|(key, _)| key.clone()));
                    }
                    if let Some(error) = [
                        match &result.edits {
                            ImportFamilyOutcome::Failed(e)
                            | ImportFamilyOutcome::SourceChanged(e) => Some(e),
                            _ => None,
                        },
                        match &result.tags {
                            ImportFamilyOutcome::Failed(e)
                            | ImportFamilyOutcome::SourceChanged(e) => Some(e),
                            _ => None,
                        },
                    ]
                    .into_iter()
                    .flatten()
                    .next()
                    {
                        return Err(error.clone());
                    }
                    if let Some(error) = result.source_validation_error {
                        return Err(error);
                    }
                    resources.sidecars.insert(source.to_owned(), sidecar);
                }
                _ => return Err("元のフォルダの保存データが変更されました。".into()),
            }
        }
        crate::sidecar::SidecarImportLoad::Missing { sidecar } => {
            resources.sidecars.insert(source.to_owned(), sidecar);
        }
        _ => return Err("元のフォルダの保存データを読み込めませんでした。".into()),
    }
    Ok(())
}

fn run(
    input: Inputs,
    resources: &mut Resources,
    control: &ConversionControl,
    commands: &mpsc::Receiver<Command>,
    notify: &impl Fn(Event),
) -> Result<Success, String> {
    // A handed-off dirty sidecar belongs to the original directory too.
    validate_source_directory(resources, &input.manifest)?;
    for sidecar in resources.sidecars.values_mut() {
        sidecar.queue_flush();
    }
    crate::sidecar::wait_for_pending_writes();
    if resources.sidecars.values().any(|s| !s.written_to_disk()) {
        return Err("余白や編集の保存を完了できませんでした。".into());
    }
    validate_source_directory(resources, &input.manifest)?;
    for batch in &input.trims {
        crate::view_trim_db::ViewTrimDb::open_at(&input.data_dir.join("view_trim.db"))
            .map_err(|e| e.to_string())?
            .apply_write_batch(batch)
            .map_err(|e| e.to_string())?;
        resources.saved_trims += 1;
    }
    // Captured user saves above must finish even when cancellation arrived while
    // they were pending. The remaining work only prepares this conversion.
    control.check()?;
    // Existing completion owners were drained by the UI before these leases.
    let shift = crate::spread_shift::maintenance_at(&input.data_dir)?;
    control.check()?;
    let _composition = crate::book_composition::maintenance_at(&input.data_dir)?;
    control.check()?;
    restore_source(resources, &input.manifest.source, &input.data_dir)?;
    control.check()?;
    load_parent(resources, &input.manifest.source, &input.data_dir)?;
    control.check()?;
    match crate::sidecar::SidecarFile::load_for_import(&input.manifest.source) {
        crate::sidecar::SidecarImportLoad::Loaded(loaded) => {
            let (sidecar, proof) = loaded.into_parts();
            resources
                .sidecars
                .insert(input.manifest.source.clone(), sidecar);
            resources
                .proofs
                .insert(input.manifest.source.clone(), Some(proof));
        }
        crate::sidecar::SidecarImportLoad::Missing { .. } => {
            resources.proofs.insert(input.manifest.source.clone(), None);
        }
        _ => return Err("source saved data changed".into()),
    }
    // A preceding XMP write may have changed bytes since the initial FS estimate.
    validate_source_directory(resources, &input.manifest)?;
    let manifest = crate::image_folder_zip::prepare_manifest(
        &input.manifest.source,
        Some(&crate::books::settings_books_root(&input.settings)),
        control,
    )?;
    let plan = metadata::prepare_metadata(
        &input.data_dir,
        &manifest,
        input.settings.write_rating_to_xmp,
        &input.session_ratings,
        control,
    )?;
    let actor_rows = wait_portals(&input.portals, &manifest, CopyPhase::Check, control)?;
    let mut favorite_rows = input.favorite_pairs.len();
    if !input.favorite_pairs.is_empty() {
        control.check()?;
        crate::settings_db::with_db_result(|db| {
            db.copy_folder_zip_favorites(&input.favorite_pairs, CopyPhase::Check)
        })
        .map_err(|e| e.to_string())?;
        favorite_rows += favorite_params(
            &input.data_dir,
            &input.favorite_pairs,
            CopyPhase::Check,
            control,
        )?;
    }
    check_backups(
        &input.data_dir,
        &manifest,
        &plan,
        resources,
        &input.settings,
        false,
        control,
    )?;
    control.check()?;
    notify(Event::Confirm(Summary {
        images: manifest.members.len(),
        bytes: manifest.total_bytes,
        auxiliary: manifest.auxiliary_files,
        rows: plan.rows + actor_rows + favorite_rows,
    }));
    if !matches!(commands.recv(), Ok(Command::Convert)) || control.is_cancelled() {
        return Err("変換を取り消しました。元のフォルダは残っています。".into());
    }
    let zip = crate::image_folder_zip::write_zip(&manifest, control, |images, bytes| {
        notify(Event::Progress { images, bytes })
    })?;
    wait_portals(&input.portals, &manifest, CopyPhase::Check, control)?;
    metadata::prepare_metadata(
        &input.data_dir,
        &manifest,
        input.settings.write_rating_to_xmp,
        &input.session_ratings,
        control,
    )?;
    let keys = manifest
        .members
        .iter()
        .map(|m| crate::adjustment_db::zip_entry_key(&manifest.destination, &m.entry))
        .chain(std::iter::once(crate::path_key::normalize_keep_drive(
            &manifest.destination,
        )))
        .collect::<Vec<_>>();
    let _page = crate::page_edit_write_epoch::PAGE_EDIT_WRITES.begin_for_keys(&keys);
    let _rating = crate::rating_db::RATING_WRITES.begin_for_keys(&keys);
    let _tag = crate::tags_db::TAG_WRITES.begin_for_keys(&keys);
    validate_proofs(resources)?;
    let published = zip.publish(&manifest, control)?;
    notify(Event::Committing);
    let copied = (|| {
        metadata::apply_metadata(&input.data_dir, &manifest, &plan)?;
        shift.invalidate_root_family(&manifest.destination);
        wait_portals(&input.portals, &manifest, CopyPhase::Apply, control)?;
        if !input.favorite_pairs.is_empty() {
            favorite_params(
                &input.data_dir,
                &input.favorite_pairs,
                CopyPhase::Apply,
                control,
            )?;
            crate::settings_db::with_db_result(|db| {
                db.copy_folder_zip_favorites(&input.favorite_pairs, CopyPhase::Apply)
            })
            .map_err(|e| e.to_string())?;
        }
        validate_proofs(resources)?;
        check_backups(
            &input.data_dir,
            &manifest,
            &plan,
            resources,
            &input.settings,
            true,
            control,
        )?;
        if let Some(observer) = &input.observer {
            observer.record_for_path(
                &manifest.destination,
                crate::content_identity::ContentIdentityTrigger::Edit,
            );
        }
        Ok(Success {
            favorites: input
                .favorite_pairs
                .iter()
                .map(|(_, new)| new.clone())
                .collect(),
            favorite_params: copied_favorite_params(&input.data_dir, &input.favorite_pairs)?,
        })
    })();
    if copied.is_err()
        && let Err(error) = published.remove_owned()
    {
        crate::logger::log(format!("image folder ZIP cleanup: {error}"));
    }
    copied
}

fn copied_favorite_params(
    data_dir: &std::path::Path,
    pairs: &[(
        crate::settings::FavoriteEntry,
        crate::settings::FavoriteEntry,
    )],
) -> Result<Vec<(uuid::Uuid, crate::adjustment::AdjustParams)>, String> {
    use rusqlite::OptionalExtension;
    if pairs.is_empty() {
        return Ok(vec![]);
    }
    let conn =
        rusqlite::Connection::open(data_dir.join("adjustment.db")).map_err(|e| e.to_string())?;
    let mut values = vec![];
    for (_, target) in pairs {
        let json = conn
            .query_row(
                "SELECT params_json FROM favorite_params WHERE favorite_id=?1",
                [target.id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(json) = json {
            values.push((
                target.id,
                serde_json::from_str(&json).map_err(|e| e.to_string())?,
            ));
        }
    }
    Ok(values)
}

fn favorite_params(
    data_dir: &std::path::Path,
    pairs: &[(
        crate::settings::FavoriteEntry,
        crate::settings::FavoriteEntry,
    )],
    phase: CopyPhase,
    control: &ConversionControl,
) -> Result<usize, String> {
    control.check()?;
    let path = data_dir.join("adjustment.db");
    if !path.try_exists().map_err(|e| e.to_string())? {
        return Ok(0);
    }
    let conn = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
    let mut count = 0;
    for (old, new) in pairs {
        control.check()?;
        count += metadata::copy_rows_on(
            &conn,
            "favorite_params",
            "favorite_id",
            &old.id.to_string(),
            &new.id.to_string(),
            phase,
            |columns, row| {
                let i = columns
                    .iter()
                    .position(|c| c == "params_json")
                    .ok_or("favorite params absent")?;
                let rusqlite::types::Value::Text(json) = &row[i] else {
                    return Err("invalid favorite adjustment".into());
                };
                let _: crate::adjustment::AdjustParams =
                    serde_json::from_str(json).map_err(|e| e.to_string())?;
                Ok(())
            },
        )?;
    }
    Ok(count)
}

fn check_backups(
    data_dir: &std::path::Path,
    manifest: &ImageFolderManifest,
    plan: &metadata::ZipMetadataPlan,
    resources: &mut Resources,
    settings: &crate::settings::Settings,
    apply: bool,
    control: &ConversionControl,
) -> Result<(), String> {
    control.check()?;
    let parent = manifest.destination.parent().unwrap();
    if !resources.sidecars.contains_key(parent) {
        let sidecar = match crate::sidecar::SidecarFile::load_for_import(parent) {
            crate::sidecar::SidecarImportLoad::Loaded(loaded) => loaded.into_parts().0,
            crate::sidecar::SidecarImportLoad::Missing { sidecar } => sidecar,
            _ => return Err("作成先の保存データを読み込めませんでした。".into()),
        };
        resources.sidecars.insert(parent.to_owned(), sidecar);
    }
    let sidecar = resources.sidecars.get_mut(parent).unwrap();
    let body = manifest
        .destination
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_lowercase();
    for member in &manifest.members {
        control.check()?;
        let entry = crate::metadata_transfer::folder_zip_page_backup(data_dir, &member.path)?;
        let key = format!("{body}::{}", member.entry.to_lowercase());
        if let Some(existing) = sidecar.items().get(&key)
            && serde_json::to_value(existing).map_err(|e| e.to_string())?
                != serde_json::to_value(&entry).map_err(|e| e.to_string())?
        {
            return Err(metadata::CONFLICT.into());
        }
        if apply && settings.sidecar_backup_enabled {
            sidecar.replace_edit_bundle(&key, entry);
        }
    }
    control.check()?;
    if let Some(existing) = sidecar.items().get(&body) {
        let mut without_tags = existing.clone();
        without_tags.tags = None;
        if serde_json::to_value(without_tags).map_err(|e| e.to_string())?
            != serde_json::to_value(crate::sidecar::SidecarEntry::default())
                .map_err(|e| e.to_string())?
        {
            return Err(metadata::CONFLICT.into());
        }
    }
    let tags = plan
        .body_tags
        .as_ref()
        .map(|tags| tags.iter().map(|t| t.tag.clone()).collect::<Vec<_>>());
    if let Some(existing) = sidecar.items().get(&body)
        && existing.tags != tags
    {
        return Err(metadata::CONFLICT.into());
    }
    if apply
        && settings.tag_sidecar_backup_enabled
        && let Some(tags) = tags
    {
        sidecar.set_tags(&body, tags);
    }
    if apply && !sidecar.flush_blocking() {
        return Err("ZIP の登録や編集の保存を完了できませんでした。".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, params};
    use std::path::Path;
    use std::time::Duration;

    struct Fixture {
        data: tempfile::TempDir,
        files: tempfile::TempDir,
        manifest: ImageFolderManifest,
    }
    impl Fixture {
        fn new() -> Self {
            let data = tempfile::tempdir().unwrap();
            // App startup eagerly initializes these six stores even in a fresh
            // profile. Sidecar import requires their real schemas for its atomic
            // attached-store transaction; an empty directory is not an App profile.
            drop(
                crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
                    .unwrap(),
            );
            drop(crate::mask_db::MaskDb::open_at(&data.path().join("mask.db")).unwrap());
            drop(crate::conceal_db::ConcealDb::open_at(&data.path().join("conceal.db")).unwrap());
            drop(
                crate::local_adjust_db::LocalAdjustDb::open_at(
                    &data.path().join("local_adjust.db"),
                )
                .unwrap(),
            );
            drop(crate::export_crop::CropDb::open_at(&data.path().join("export_crop.db")).unwrap());
            drop(crate::comic_db::ComicDb::open_at(&data.path().join("comic.db")).unwrap());
            let files = tempfile::tempdir().unwrap();
            let source = files.path().join("book");
            std::fs::create_dir(&source).unwrap();
            for name in ["a.jpg", "b.jpg"] {
                std::fs::write(source.join(name), name.as_bytes()).unwrap();
            }
            let manifest = crate::image_folder_zip::prepare_manifest(
                &source,
                None,
                &ConversionControl::default(),
            )
            .unwrap();
            Self {
                data,
                files,
                manifest,
            }
        }
        fn input(&self) -> Inputs {
            Inputs {
                data_dir: self.data.path().to_owned(),
                manifest: self.manifest.clone(),
                settings: Box::default(),
                session_ratings: HashMap::new(),
                portals: Vec::new(),
                favorite_pairs: Vec::new(),
                trims: Vec::new(),
                observer: None,
            }
        }
        fn start(
            &self,
            input: Inputs,
            sidecars: HashMap<PathBuf, crate::sidecar::SidecarFile>,
        ) -> (Worker, Arc<ConversionControl>) {
            let tags = crate::tags_db::TagsDb::open_at(&self.data.path().join("tags.db")).unwrap();
            let resources = Arc::new(Mutex::new(Resources::new(sidecars, Some(tags))));
            let control = Arc::new(ConversionControl::default());
            let worker = spawn(
                input,
                resources,
                Arc::clone(&control),
                egui::Context::default(),
            )
            .unwrap();
            (worker, control)
        }
        fn source_key(&self, name: &str) -> String {
            crate::path_key::normalize_keep_drive(&self.manifest.source.join(name))
        }
        fn target_key(&self, name: &str) -> String {
            crate::adjustment_db::zip_entry_key(&self.manifest.destination, name)
        }
        fn assert_no_destination(&self) {
            assert!(!self.manifest.destination.exists());
            for entry in std::fs::read_dir(self.files.path()).unwrap() {
                let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                assert!(
                    !name.starts_with(".miv-image-zip-"),
                    "temporary ZIP retained: {name}"
                );
            }
        }
    }
    fn receive(worker: &Worker) -> Event {
        worker
            .events
            .recv_timeout(Duration::from_secs(30))
            .expect("ZIP worker completion signal")
    }
    fn confirm(worker: &Worker) -> Summary {
        match receive(worker) {
            Event::Confirm(summary) => summary,
            Event::Finished { result, .. } => {
                panic!("worker failed before confirmation: {result:?}")
            }
            _ => panic!("unexpected event before confirmation"),
        }
    }
    fn finish(worker: &Worker) -> Result<Vec<crate::settings::FavoriteEntry>, String> {
        loop {
            match receive(worker) {
                Event::Finished { result, .. } => {
                    // Finished is sent after the resource lock and maintenance leases return.
                    assert!(worker.resources.try_lock().unwrap().tags.is_some());
                    return result.map(|success| success.favorites);
                }
                Event::Progress { .. } | Event::Committing => {}
                Event::Confirm(_) => panic!("unexpected second confirmation"),
            }
        }
    }
    fn tags(data: &Path, key: &str) -> Vec<String> {
        let conn = Connection::open(data.join("tags.db")).unwrap();
        conn.prepare("SELECT tag_key FROM item_tags WHERE item_key=?1 ORDER BY tag_key")
            .unwrap()
            .query_map([key], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }
    fn dirty_source(fixture: &Fixture) -> crate::sidecar::SidecarFile {
        let mut sidecar = crate::sidecar::SidecarFile::load(&fixture.manifest.source);
        sidecar.set_tags("a.jpg", ["one", "shared"]);
        sidecar.set_tags("b.jpg", ["discarded"]);
        sidecar.set_adjust("a.jpg", crate::adjustment::AdjustParams::default());
        sidecar
    }

    #[test]
    fn folder_zip_worker_flushes_dat_before_union_and_preserves_parent_with_backups_on_and_off() {
        let _scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        for backup in [true, false] {
            let fixture = Fixture::new();
            let mut db =
                crate::tags_db::TagsDb::open_at(&fixture.data.path().join("tags.db")).unwrap();
            // An explicit empty decision prevents an old DAT tag from resurfacing.
            db.set_item_tags(
                &fixture.source_key("b.jpg"),
                Vec::<String>::new(),
                crate::tags_db::source::EDIT,
            )
            .unwrap();
            drop(db);
            let mut parent = crate::sidecar::SidecarFile::load(fixture.files.path());
            parent.set_tags("book", ["folder", "shared"]);
            parent.set_tags("sibling.zip", ["keep"]);
            parent.set_adjust(
                "sibling.zip::p.jpg",
                crate::adjustment::AdjustParams::default(),
            );
            let sibling = serde_json::to_value(parent.items().get("sibling.zip")).unwrap();
            let sibling_page =
                serde_json::to_value(parent.items().get("sibling.zip::p.jpg")).unwrap();
            let source = dirty_source(&fixture);
            let old_items = serde_json::to_value(source.items()).unwrap();
            let mut input = fixture.input();
            input.settings.sidecar_backup_enabled = backup;
            input.settings.tag_sidecar_backup_enabled = backup;
            let (worker, _) = fixture.start(
                input,
                HashMap::from([
                    (fixture.manifest.source.clone(), source),
                    (fixture.files.path().to_owned(), parent),
                ]),
            );
            let summary = confirm(&worker);
            assert_eq!(summary.images, 2);
            assert_eq!(summary.rows, 5); // adjustment + three body tags + explicit decision
            let body = crate::path_key::normalize_keep_drive(&fixture.manifest.source);
            assert_eq!(tags(fixture.data.path(), &body), ["folder", "shared"]);
            assert!(tags(fixture.data.path(), &fixture.source_key("b.jpg")).is_empty());
            worker.commands.send(Command::Convert).unwrap();
            assert!(finish(&worker).unwrap().is_empty());
            assert_eq!(
                tags(
                    fixture.data.path(),
                    &crate::path_key::normalize_keep_drive(&fixture.manifest.destination)
                ),
                ["folder", "one", "shared"]
            );
            for name in ["a.jpg", "b.jpg"] {
                assert!(tags(fixture.data.path(), &fixture.target_key(name)).is_empty());
            }
            let source = crate::sidecar::SidecarFile::load(&fixture.manifest.source);
            assert_eq!(serde_json::to_value(source.items()).unwrap(), old_items);
            let parent = crate::sidecar::SidecarFile::load(fixture.files.path());
            assert_eq!(
                serde_json::to_value(parent.items().get("sibling.zip")).unwrap(),
                sibling
            );
            assert_eq!(
                serde_json::to_value(parent.items().get("sibling.zip::p.jpg")).unwrap(),
                sibling_page
            );
            assert_eq!(parent.items().contains_key("book.zip::a.jpg"), backup);
            assert_eq!(parent.items().contains_key("book.zip"), backup);
            if backup {
                assert!(parent.items()["book.zip::a.jpg"].tags.is_none());
                assert_eq!(parent.items()["book.zip"].tags.as_ref().unwrap().len(), 3);
            }
            let db = crate::adjustment_db::AdjustmentDb::open_at(
                &fixture.data.path().join("adjustment.db"),
            )
            .unwrap();
            assert_eq!(
                db.get_page_params(&fixture.source_key("a.jpg")),
                db.get_page_params(&fixture.target_key("a.jpg"))
            );
            let mut zip =
                zip::ZipArchive::new(std::fs::File::open(&fixture.manifest.destination).unwrap())
                    .unwrap();
            for name in ["a.jpg", "b.jpg"] {
                use std::io::Read;
                let mut entry = zip.by_name(name).unwrap();
                assert_eq!(entry.compression(), zip::CompressionMethod::Stored);
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                assert_eq!(
                    bytes,
                    std::fs::read(fixture.manifest.source.join(name)).unwrap()
                );
            }
        }
    }

    #[test]
    fn folder_zip_worker_confirm_cancel_retains_source_supplementation_and_returns_resources() {
        let _scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let fixture = Fixture::new();
        let (worker, _) = fixture.start(
            fixture.input(),
            HashMap::from([(fixture.manifest.source.clone(), dirty_source(&fixture))]),
        );
        confirm(&worker);
        worker.commands.send(Command::Cancel).unwrap();
        assert!(finish(&worker).unwrap_err().contains("取り消しました"));
        fixture.assert_no_destination();
        assert_eq!(
            tags(fixture.data.path(), &fixture.source_key("a.jpg")),
            ["one", "shared"]
        );
        assert!(tags(fixture.data.path(), &fixture.target_key("a.jpg")).is_empty());
        let db =
            crate::adjustment_db::AdjustmentDb::open_at(&fixture.data.path().join("adjustment.db"))
                .unwrap();
        assert!(db.get_page_params(&fixture.source_key("a.jpg")).is_some());
        assert!(db.get_page_params(&fixture.target_key("a.jpg")).is_none());
        let resources = worker.resources.lock().unwrap();
        assert!(resources.source_keys.contains(&fixture.source_key("a.jpg")));
        assert!(
            resources
                .source_edits
                .contains(&fixture.source_key("a.jpg"))
        );
        assert!(resources.sidecars[&fixture.manifest.source].written_to_disk());
    }

    #[test]
    fn folder_zip_worker_preparation_cancel_drains_accepted_check_and_stops_next_request() {
        let _scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let fixture = Fixture::new();
        let (called, calls) = mpsc::channel();
        let mut input = fixture.input();
        for index in 0..2 {
            let called = called.clone();
            input.portals.push(Arc::new(move |_, phase| {
                let (reply, receive) = mpsc::channel();
                called.send((index, phase, reply)).unwrap();
                receive
            }));
        }
        drop(called);
        let (worker, control) = fixture.start(
            input,
            HashMap::from([(fixture.manifest.source.clone(), dirty_source(&fixture))]),
        );
        let (index, phase, reply) = calls.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_eq!(index, 0);
        assert!(phase == CopyPhase::Check);
        assert!(control.cancel());
        // The in-flight Check still owns its receipt/resources until the reply.
        assert!(matches!(
            worker.events.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(worker.resources.try_lock().is_err());
        reply.send(Ok(0)).unwrap();
        assert!(finish(&worker).unwrap_err().contains("取り消しました"));
        assert!(matches!(
            calls.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        fixture.assert_no_destination();
        assert_eq!(
            tags(fixture.data.path(), &fixture.source_key("a.jpg")),
            ["one", "shared"]
        );
        assert!(tags(fixture.data.path(), &fixture.target_key("a.jpg")).is_empty());
        let resources = worker.resources.lock().unwrap();
        assert!(
            resources
                .source_edits
                .contains(&fixture.source_key("a.jpg"))
        );
        assert!(resources.sidecars[&fixture.manifest.source].written_to_disk());
    }

    #[test]
    fn folder_zip_worker_waits_for_explicit_copy_replies_and_cancels_before_publication() {
        let _scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let fixture = Fixture::new();
        let (called, calls) = mpsc::channel();
        let mut input = fixture.input();
        input.portals.push(Arc::new(move |_, phase| {
            let (reply, receive) = mpsc::channel();
            called.send((phase, reply)).unwrap();
            receive
        }));
        let (worker, control) = fixture.start(
            input,
            HashMap::from([(fixture.manifest.source.clone(), dirty_source(&fixture))]),
        );
        let (phase, reply) = calls.recv_timeout(Duration::from_secs(30)).unwrap();
        assert!(phase == CopyPhase::Check);
        assert!(matches!(
            worker.events.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        reply.send(Ok(0)).unwrap();
        confirm(&worker);
        worker.commands.send(Command::Convert).unwrap();
        let (phase, reply) = calls.recv_timeout(Duration::from_secs(30)).unwrap();
        assert!(phase == CopyPhase::Check);
        assert!(!fixture.manifest.destination.exists());
        assert!(control.cancel());
        reply.send(Ok(0)).unwrap();
        assert!(finish(&worker).is_err());
        fixture.assert_no_destination();
        assert_eq!(
            tags(fixture.data.path(), &fixture.source_key("a.jpg")),
            ["one", "shared"]
        );
        assert!(tags(fixture.data.path(), &fixture.target_key("a.jpg")).is_empty());
        assert!(matches!(
            calls.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn folder_zip_worker_consumed_parent_dat_does_not_resurrect_folder_tags() {
        let _scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let fixture = Fixture::new();
        let mut parent = crate::sidecar::SidecarFile::load(fixture.files.path());
        parent.set_tags("book", ["old"]);
        assert!(parent.flush_blocking());
        let crate::sidecar::SidecarImportLoad::Loaded(loaded) =
            crate::sidecar::SidecarFile::load_for_import(fixture.files.path())
        else {
            panic!("parent DAT missing")
        };
        let (_, crate::sidecar::SidecarImportSource::Disk(token)) = loaded.into_parts() else {
            panic!("parent DAT not durable")
        };
        let db = crate::tags_db::TagsDb::open_at(&fixture.data.path().join("tags.db")).unwrap();
        db.sidecar_sync_upsert(
            &crate::path_key::normalize_keep_drive(fixture.files.path()),
            token.sync_marker(),
        )
        .unwrap();
        drop(db);
        let (worker, _) = fixture.start(fixture.input(), HashMap::new());
        assert_eq!(confirm(&worker).rows, 0);
        worker.commands.send(Command::Convert).unwrap();
        finish(&worker).unwrap();
        assert!(
            tags(
                fixture.data.path(),
                &crate::path_key::normalize_keep_drive(&fixture.manifest.source)
            )
            .is_empty()
        );
        assert!(
            tags(
                fixture.data.path(),
                &crate::path_key::normalize_keep_drive(&fixture.manifest.destination)
            )
            .is_empty()
        );
        let db = crate::tags_db::TagsDb::open_at(&fixture.data.path().join("tags.db")).unwrap();
        assert_eq!(
            db.sidecar_sync_get(&crate::path_key::normalize_keep_drive(fixture.files.path())),
            Some(token.sync_marker())
        );
        assert_eq!(
            crate::sidecar::SidecarFile::load(fixture.files.path()).items()["book"].tags,
            Some(vec!["#old".into()])
        );
    }

    #[test]
    fn folder_zip_worker_rejects_replaced_original_directory_before_dat_import() {
        let _scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let fixture = Fixture::new();
        let original_dirty = dirty_source(&fixture);
        let original_items = serde_json::to_value(original_dirty.items()).unwrap();
        let mut parent = crate::sidecar::SidecarFile::load(fixture.files.path());
        parent.set_tags("sibling.zip", ["retained"]);
        std::fs::rename(
            &fixture.manifest.source,
            fixture.files.path().join("original"),
        )
        .unwrap();
        std::fs::create_dir(&fixture.manifest.source).unwrap();
        std::fs::write(fixture.manifest.source.join("a.jpg"), b"replacement").unwrap();
        let mut replacement = crate::sidecar::SidecarFile::load(&fixture.manifest.source);
        replacement.set_tags("a.jpg", ["replacement"]);
        assert!(replacement.flush_blocking());
        let replacement_bytes = std::fs::read(
            fixture
                .manifest
                .source
                .join(crate::sidecar::SIDECAR_FILENAME),
        )
        .unwrap();
        let (worker, _) = fixture.start(
            fixture.input(),
            HashMap::from([
                (fixture.manifest.source.clone(), original_dirty),
                (fixture.files.path().to_owned(), parent),
            ]),
        );
        assert!(finish(&worker).is_err());
        fixture.assert_no_destination();
        assert!(tags(fixture.data.path(), &fixture.source_key("a.jpg")).is_empty());
        {
            let mut resources = worker.resources.lock().unwrap();
            assert!(resources.source_keys.is_empty());
            assert_eq!(
                serde_json::to_value(resources.sidecars[&fixture.manifest.source].items()).unwrap(),
                original_items
            );
            // App may queue returned owners again; only the stale source is write-disabled.
            for sidecar in resources.sidecars.values_mut() {
                sidecar.queue_flush();
            }
            crate::sidecar::wait_for_pending_writes();
            assert!(resources.sidecars[fixture.files.path()].written_to_disk());
        }
        assert_eq!(
            crate::sidecar::SidecarFile::load(fixture.files.path()).items()["sibling.zip"].tags,
            Some(vec!["#retained".into()])
        );
        assert_eq!(
            std::fs::read(
                fixture
                    .manifest
                    .source
                    .join(crate::sidecar::SIDECAR_FILENAME)
            )
            .unwrap(),
            replacement_bytes
        );
    }

    #[test]
    fn folder_zip_worker_revalidates_existing_and_missing_parent_and_source_dat_proofs() {
        let _scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        for parent in [false, true] {
            for existing in [false, true] {
                let fixture = Fixture::new();
                let folder = if parent {
                    fixture.files.path()
                } else {
                    &fixture.manifest.source
                };
                if existing {
                    let mut sidecar = crate::sidecar::SidecarFile::load(folder);
                    sidecar.set_tags(if parent { "book" } else { "a.jpg" }, ["before"]);
                    assert!(sidecar.flush_blocking());
                }
                let (worker, _) = fixture.start(fixture.input(), HashMap::new());
                confirm(&worker);
                let mut changed = crate::sidecar::SidecarFile::load(folder);
                changed.set_tags(if parent { "book" } else { "a.jpg" }, ["changed"]);
                assert!(changed.flush_blocking());
                worker.commands.send(Command::Convert).unwrap();
                assert!(finish(&worker).is_err());
                fixture.assert_no_destination();
                assert_eq!(
                    crate::sidecar::SidecarFile::load(folder).items()
                        [if parent { "book" } else { "a.jpg" }]
                    .tags,
                    Some(vec!["#changed".into()])
                );
            }
        }
    }

    #[test]
    fn folder_zip_worker_destination_race_keeps_existing_bytes_and_source_rows() {
        let _scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let fixture = Fixture::new();
        let (worker, _) = fixture.start(
            fixture.input(),
            HashMap::from([(fixture.manifest.source.clone(), dirty_source(&fixture))]),
        );
        confirm(&worker);
        std::fs::write(&fixture.manifest.destination, b"existing user ZIP").unwrap();
        worker.commands.send(Command::Convert).unwrap();
        assert!(finish(&worker).is_err());
        assert_eq!(
            std::fs::read(&fixture.manifest.destination).unwrap(),
            b"existing user ZIP"
        );
        assert_eq!(
            tags(fixture.data.path(), &fixture.source_key("a.jpg")),
            ["one", "shared"]
        );
        assert!(tags(fixture.data.path(), &fixture.target_key("a.jpg")).is_empty());
    }

    fn xmp_jpeg(stars: u8) -> Vec<u8> {
        let xml = format!(
            "<x:xmpmeta xmlns:x='adobe:ns:meta/'><rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'><rdf:Description xmlns:xmp='http://ns.adobe.com/xap/1.0/' xmp:Rating='{stars}'/></rdf:RDF></x:xmpmeta>"
        );
        let mut payload = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        payload.extend_from_slice(xml.as_bytes());
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        jpeg.extend_from_slice(&payload);
        jpeg.extend_from_slice(&[0xff, 0xda, 0, 2, 0xff, 0xd9]);
        jpeg
    }
    #[test]
    fn folder_zip_worker_reads_unvisited_xmp_without_source_rating_commit_and_respects_clear() {
        let _scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        for (read_xmp, clear, expected_rows) in
            [(true, false, 1), (false, false, 0), (true, true, 0)]
        {
            let fixture = Fixture::new();
            let path = fixture.manifest.source.join("a.jpg");
            std::fs::write(&path, xmp_jpeg(4)).unwrap();
            // No item/index/cache is supplied: all manifest pages are evaluated by the worker.
            let mut input = fixture.input();
            input.settings.write_rating_to_xmp = read_xmp;
            if clear {
                input.session_ratings.insert(fixture.source_key("a.jpg"), 0);
            }
            let (worker, _) = fixture.start(input, HashMap::new());
            assert_eq!(confirm(&worker).rows, expected_rows);
            worker.commands.send(Command::Convert).unwrap();
            finish(&worker).unwrap();
            let db =
                crate::rating_db::RatingDb::open_at(fixture.data.path().join("rating.db")).unwrap();
            assert!(
                db.row_for_key(&fixture.source_key("a.jpg"))
                    .unwrap()
                    .is_none()
            );
            let row = db.row_for_key(&fixture.target_key("a.jpg")).unwrap();
            if expected_rows == 1 {
                let row = row.unwrap();
                assert_eq!(row.stars, 4);
                assert_eq!(row.kind, Some(crate::rating_db::RatingItemKind::ZipImage));
                assert_eq!(row.entry_name, Some("a.jpg".into()));
                assert_eq!(row.rated_at_ms, None);
            } else {
                assert!(row.is_none());
            }
        }
    }

    #[test]
    fn folder_zip_worker_keeps_collection_file_entry_ids_order_seed_and_revision_unchanged() {
        let _scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let fixture = Fixture::new();
        let collection = fixture.data.path().join("collection.db");
        // The existing collection schema is a retained, source-only registration fixture.
        let conn = Connection::open(&collection).unwrap();
        conn.execute_batch("CREATE TABLE collection_meta(singleton INTEGER PRIMARY KEY, catalog_revision INTEGER NOT NULL);
            INSERT INTO collection_meta VALUES(1,23);
            CREATE TABLE collections(id TEXT PRIMARY KEY,name TEXT,order_mode TEXT,sort_order TEXT,shuffle_seed TEXT,revision INTEGER,catalog_position INTEGER,created_at_ms INTEGER,updated_at_ms INTEGER);
            INSERT INTO collections VALUES('00000000-0000-0000-0000-000000000001','kept','manual','file_name','0011223344556677',7,0,10,20);
            CREATE TABLE collection_entries(id TEXT PRIMARY KEY,collection_id TEXT,source_namespace TEXT,source_path TEXT,normalized_path TEXT,resolved_kind TEXT,manual_position INTEGER,created_at_ms INTEGER);
            PRAGMA user_version=2;").unwrap();
        for (id, path, kind, position) in [
            (
                "00000000-0000-0000-0000-000000000002",
                &fixture.manifest.source,
                "folder",
                1,
            ),
            (
                "00000000-0000-0000-0000-000000000003",
                &fixture.manifest.members[0].path,
                "image",
                0,
            ),
        ] {
            conn.execute("INSERT INTO collection_entries VALUES(?1,'00000000-0000-0000-0000-000000000001','filesystem_path',?2,?3,?4,?5,30)",
                params![id, path.to_string_lossy(), crate::path_key::normalize_keep_drive(path), kind, position]).unwrap();
        }
        drop(conn);
        let before = std::fs::read(&collection).unwrap();
        let (worker, _) = fixture.start(fixture.input(), HashMap::new());
        confirm(&worker);
        worker.commands.send(Command::Convert).unwrap();
        finish(&worker).unwrap();
        assert_eq!(std::fs::read(&collection).unwrap(), before);
        assert!(fixture.manifest.source.is_dir());
        for member in &fixture.manifest.members {
            assert!(std::fs::File::open(&member.path).is_ok());
        }
    }

    #[test]
    fn folder_zip_worker_root_consumers_keep_mode_only_rtl_cover_trim_and_typed_shift() {
        use crate::book_scope::BookScopeKey;
        use crate::settings::{
            FinalCoverSpreadPreference, PageAlonePreference, PageAlonePreferences,
            ReadingDirection, ReadingFlow, SingletonSpreadEndpointPreferences,
            SingletonSpreadPlacementPreference, SpreadMode,
        };
        use crate::spread_shift::{
            ShiftIdentityIndex, ShiftOperation, SpreadShiftPageRef, SpreadShiftService,
        };
        let _scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        for (view, resolved_mode) in [
            (SpreadMode::Single, SpreadMode::Single),
            (SpreadMode::Ltr, SpreadMode::RtlCover),
            (SpreadMode::SplitLtr, SpreadMode::SplitRtl),
        ] {
            for anchor_name in ["a.jpg", "Missing.jpg"] {
                let fixture = Fixture::new();
                let source = &fixture.manifest.source;
                let destination = &fixture.manifest.destination;
                let sibling = fixture.files.path().join("sibling.zip");
                let source_scope = BookScopeKey::folder(source).unwrap();
                let target_scope = BookScopeKey::archive(destination, &[]).unwrap();
                let sibling_scope = BookScopeKey::archive(&sibling, &[]).unwrap();
                assert!(target_scope.level_prefix.is_empty());

                let spread =
                    crate::spread_db::SpreadDb::open_at(&fixture.data.path().join("spread.db"))
                        .unwrap();
                let defaults = (
                    SpreadMode::Single,
                    ReadingFlow::Paged,
                    ReadingDirection::Ltr,
                );
                // The old RTL/cover binding stays dormant beneath each viewing choice.
                spread
                    .set_presentation_state(
                        source,
                        SpreadMode::RtlCover,
                        ReadingFlow::Horizontal,
                        ReadingDirection::Rtl,
                        defaults,
                    )
                    .unwrap();
                spread.set_mode_only(source, view).unwrap();
                spread
                    .set_final_cover_spread_preference(
                        source,
                        None,
                        FinalCoverSpreadPreference::Off,
                    )
                    .unwrap();
                let endpoints = SingletonSpreadEndpointPreferences {
                    first: SingletonSpreadPlacementPreference::Place,
                    last: SingletonSpreadPlacementPreference::Center,
                };
                spread
                    .set_singleton_spread_endpoint_preferences(source, None, endpoints)
                    .unwrap();
                let alone = PageAlonePreferences {
                    after_cover: PageAlonePreference::On,
                    last: PageAlonePreference::Off,
                };
                spread
                    .set_page_alone_preferences(source, None, alone)
                    .unwrap();
                spread
                    .set_presentation_state(
                        &sibling,
                        SpreadMode::SplitLtr,
                        ReadingFlow::Vertical,
                        ReadingDirection::Ltr,
                        defaults,
                    )
                    .unwrap();
                spread
                    .set_final_cover_spread_preference(
                        &sibling,
                        None,
                        FinalCoverSpreadPreference::On,
                    )
                    .unwrap();
                let source_spread = spread.get_state_with_fallback(source, None);
                let sibling_spread = spread.get_state_with_fallback(&sibling, None);
                assert_eq!(source_spread.mode, Some(SpreadMode::RtlCover));
                assert!(source_spread.view_mode.is_some());

                let trim = crate::view_trim_db::ViewTrimDb::open_at(
                    &fixture.data.path().join("view_trim.db"),
                )
                .unwrap();
                let source_trim = crate::view_trim::ViewTrimBookState {
                    apply_mode: crate::view_trim::ViewTrimApplyMode::Book,
                    book_settings: crate::view_trim::ViewTrimBookSettings {
                        enabled: true,
                        single: crate::view_trim::ViewTrimMargins {
                            left: 0.04,
                            top: 0.03,
                            right: 0.02,
                            bottom: 0.01,
                        },
                        ..Default::default()
                    },
                };
                let sibling_trim = crate::view_trim::ViewTrimBookState {
                    apply_mode: crate::view_trim::ViewTrimApplyMode::Auto,
                    ..Default::default()
                };
                trim.set_book_state(source, source_trim).unwrap();
                trim.set_book_state(&sibling, sibling_trim).unwrap();

                let (shift, shift_owner) =
                    SpreadShiftService::spawn_at(fixture.data.path().to_owned(), None).unwrap();
                let operation = |sequence| ShiftOperation {
                    context_id: crate::app::ViewerContextId::for_test(1),
                    items_generation: 4,
                    operation_sequence: sequence,
                };
                shift
                    .set_shift(
                        source_scope.clone(),
                        SpreadShiftPageRef::RelativePath(anchor_name.into()),
                        operation(1),
                    )
                    .unwrap();
                let cutoff = shift
                    .set_shift(
                        sibling_scope.clone(),
                        SpreadShiftPageRef::ArchiveEntry("keep.jpg".into()),
                        operation(2),
                    )
                    .unwrap();
                let read = |scope| {
                    shift
                        .read_after(scope, cutoff)
                        .recv_timeout(Duration::from_secs(30))
                        .unwrap()
                };
                let source_shift = read(source_scope.clone());
                let sibling_shift = read(sibling_scope.clone());
                let target_before = read(target_scope.clone());
                source_shift.status.clone().unwrap();
                sibling_shift.status.clone().unwrap();
                assert_eq!(target_before.row, None);

                // Dormant image-folder composition rows survive, but never become ZIP rows.
                let composition_path = fixture.data.path().join("book_composition.db");
                crate::book_composition::initialize_database_at(&composition_path).unwrap();
                let composition_conn = Connection::open(&composition_path).unwrap();
                let dormant = crate::book_composition::BookComposition::new(
                    crate::book_composition::ReadingDirection::Rtl,
                    vec![
                        crate::book_composition::CompositionPageRef::RelativePath {
                            path: "a.jpg".into(),
                        },
                        crate::book_composition::CompositionPageRef::RelativePath {
                            path: "b.jpg".into(),
                        },
                    ],
                );
                crate::book_composition::store::replace_row_in(
                    &composition_conn,
                    "book_compositions",
                    &source_scope.storage_key(),
                    &crate::book_composition::PageDomain::ImageFolder,
                    &dormant,
                )
                .unwrap();
                let compositions_before =
                    crate::book_composition::store::load_rows(&composition_conn).unwrap();

                let (worker, _) = fixture.start(fixture.input(), HashMap::new());
                assert_eq!(confirm(&worker).rows, 6); // spread/cover/endpoints/page-alone/trim/shift
                worker.commands.send(Command::Convert).unwrap();
                finish(&worker).unwrap();

                let target_spread = spread.get_state_with_fallback(destination, None);
                assert_eq!(target_spread, source_spread);
                let resolved = target_spread.resolve_reading(
                    SpreadMode::Ltr,
                    ReadingDirection::Ltr,
                    false,
                    None,
                );
                assert_eq!(resolved.mode, resolved_mode);
                assert_eq!(resolved.direction, ReadingDirection::Rtl);
                assert_eq!(target_spread.flow, Some(ReadingFlow::Horizontal));
                assert_eq!(
                    spread.get_final_cover_spread_preference_with_fallback(destination, None),
                    FinalCoverSpreadPreference::Off
                );
                assert_eq!(
                    spread
                        .get_singleton_spread_endpoint_preferences_with_fallback(destination, None)
                        .unwrap(),
                    endpoints
                );
                assert_eq!(
                    spread
                        .get_page_alone_preferences_with_fallback(destination, None)
                        .unwrap(),
                    alone
                );
                assert_eq!(trim.get_book_state(destination), Some(source_trim));
                assert_eq!(trim.get_book_state(source), Some(source_trim));
                assert_eq!(trim.get_book_state(&sibling), Some(sibling_trim));
                assert_eq!(spread.get_state_with_fallback(source, None), source_spread);
                assert_eq!(
                    spread.get_final_cover_spread_preference_with_fallback(source, None),
                    FinalCoverSpreadPreference::Off
                );
                assert_eq!(
                    spread
                        .get_singleton_spread_endpoint_preferences_with_fallback(source, None)
                        .unwrap(),
                    endpoints
                );
                assert_eq!(
                    spread
                        .get_page_alone_preferences_with_fallback(source, None)
                        .unwrap(),
                    alone
                );
                assert_eq!(
                    spread.get_state_with_fallback(&sibling, None),
                    sibling_spread
                );
                assert_eq!(
                    spread.get_final_cover_spread_preference_with_fallback(&sibling, None),
                    FinalCoverSpreadPreference::On
                );

                let target_shift = read(target_scope.clone());
                target_shift.status.clone().unwrap();
                assert_eq!(
                    target_shift.row,
                    Some(SpreadShiftPageRef::ArchiveEntry(anchor_name.into()))
                );
                assert!(target_shift.maintenance_epoch > target_before.maintenance_epoch);
                let source_after = read(source_scope.clone());
                let sibling_after = read(sibling_scope.clone());
                assert_eq!(source_after, source_shift);
                assert_eq!(sibling_after, sibling_shift);
                // Resolve against the actual ZIP reader's entry identities, not a raw index hint.
                let mut zip =
                    zip::ZipArchive::new(std::fs::File::open(destination).unwrap()).unwrap();
                let identities = (0..zip.len())
                    .map(|index| {
                        let entry_name = zip.by_index(index).unwrap().name().to_owned();
                        (
                            crate::book_resume_db::PageIdentity::Archive {
                                container: target_scope.storage_key(),
                                entry_name,
                            },
                            index,
                        )
                    })
                    .collect::<Vec<_>>();
                let index = ShiftIdentityIndex::prepare(&target_scope, identities).unwrap();
                let anchor = target_shift.row.as_ref().unwrap();
                assert_eq!(index.resolve(anchor), (anchor_name == "a.jpg").then_some(0));
                assert_eq!(
                    anchor.to_identity(&target_scope).unwrap(),
                    crate::book_resume_db::PageIdentity::Archive {
                        container: target_scope.storage_key(),
                        entry_name: anchor_name.into(),
                    }
                );
                assert_eq!(
                    crate::book_composition::store::load_rows(&composition_conn).unwrap(),
                    compositions_before
                );
                assert!(!compositions_before.contains_key(&target_scope.storage_key()));

                // Existing spread/trim stores keep their drive-stripped keys; shifts use BookScopeKey.
                let spread_conn = Connection::open(fixture.data.path().join("spread.db")).unwrap();
                assert_eq!(
                    spread_conn
                        .query_row(
                            "SELECT count(*) FROM spreads WHERE path=?1",
                            [crate::path_key::normalize(destination)],
                            |r| r.get::<_, usize>(0)
                        )
                        .unwrap(),
                    1
                );
                let trim_conn = Connection::open(fixture.data.path().join("view_trim.db")).unwrap();
                assert_eq!(
                    trim_conn
                        .query_row(
                            "SELECT count(*) FROM view_trim_books WHERE book_key=?1",
                            [crate::path_key::normalize(destination)],
                            |r| r.get::<_, usize>(0)
                        )
                        .unwrap(),
                    1
                );
                let shift_conn =
                    Connection::open(fixture.data.path().join("spread_shift.db")).unwrap();
                assert_eq!(
                    shift_conn
                        .query_row(
                            "SELECT count(*) FROM spread_shifts WHERE path=?1",
                            [target_scope.storage_key()],
                            |r| r.get::<_, usize>(0)
                        )
                        .unwrap(),
                    1
                );
                shift_owner.shutdown_and_join();
            }
        }
    }
}
