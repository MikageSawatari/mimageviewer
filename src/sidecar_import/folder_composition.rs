//! The folder adapter participates in the existing import owner and source fence.
//! Its receipt and missing-row mutation use the process composition actor.
use super::*;

pub(super) fn probe(
    data_dir: &Path,
    sidecar: &SidecarFile,
    source: &SidecarImportSource,
    requested: bool,
) -> SidecarProbeFamilyOutcome {
    if !requested {
        return SidecarProbeFamilyOutcome::NotRequested;
    }
    let result = (|| {
        let scope = crate::book_scope::BookScopeKey::folder(sidecar.folder())?;
        let service = crate::book_composition::service_at(data_dir)?;
        let receipt = service
            .receipt(scope)
            .recv()
            .map_err(|_| "folder composition receipt worker disconnected".to_owned())??;
        Ok::<_, String>(match (receipt, source) {
            (Some(receipt), SidecarImportSource::Disk(token))
                if receipt.disk_sync_marker == Some(token.sync_marker())
                    && receipt.consumed_root_digest == sidecar.folder_composition_digest() =>
            {
                SidecarProbeFamilyOutcome::AlreadySynchronized
            }
            _ => SidecarProbeFamilyOutcome::ImportRequired,
        })
    })();
    result.unwrap_or_else(SidecarProbeFamilyOutcome::Failed)
}

pub(super) fn commit(
    data_dir: &Path,
    sidecar: &SidecarFile,
    source: &SidecarImportSource,
    requested: bool,
    source_validation_error: Option<&str>,
    cancel: &AtomicBool,
) -> ImportFamilyOutcome<bool> {
    if !requested {
        return ImportFamilyOutcome::NotRequested;
    }
    if cancel.load(Ordering::Relaxed) {
        return ImportFamilyOutcome::Cancelled;
    }
    if let Some(error) = source_validation_error {
        return ImportFamilyOutcome::Failed(error.to_owned());
    }
    let result = (|| {
        crate::sidecar::revalidate_import_source(sidecar, source)?;
        let service = crate::book_composition::service_at(data_dir)?;
        let scope = crate::book_scope::BookScopeKey::folder(sidecar.folder())?;
        let reply = service
            .restore_folder(scope, sidecar.clone_import_snapshot(), source.clone())
            .recv()
            .map_err(|_| "folder composition restore worker disconnected".to_owned())?;
        reply.status?;
        Ok::<_, String>(reply.changed)
    })();
    match result {
        Ok(changed) => ImportFamilyOutcome::Applied(changed),
        Err(error) => ImportFamilyOutcome::Failed(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book_composition::*;

    const FOLDER: ImportFamilies = ImportFamilies {
        edits: false,
        tags: false,
        composition: true,
    };

    fn record() -> BookComposition {
        BookComposition::new(
            ReadingDirection::Rtl,
            vec![CompositionUnit::Single {
                page: CompositionPageRef::RelativePath {
                    path: "page.jpg".into(),
                },
                placement: SinglePlacement::Leading,
                companion: SingleCompanion::Blank,
            }],
        )
    }

    fn loaded(folder: &Path) -> LoadedSidecarImport {
        match SidecarFile::load_for_import(folder) {
            crate::sidecar::SidecarImportLoad::Loaded(loaded) => loaded,
            _ => panic!("strict folder source required"),
        }
    }

    #[test]
    fn folder_family_restores_after_edit_sync_and_retains_reuse_only_for_exact_family() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("book");
        let data = temp.path().join("data");
        std::fs::create_dir(&folder).unwrap();
        std::fs::create_dir(&data).unwrap();
        let mut source = SidecarFile::load(&folder);
        source.set_folder_composition(Some(record())).unwrap();
        assert!(source.flush_blocking());
        let (service, owner) = BookCompositionService::spawn_at(data.clone(), None).unwrap();
        let scope = crate::book_scope::BookScopeKey::folder(&folder).unwrap();
        service
            .prepare(scope.clone(), PageDomain::ImageFolder)
            .recv()
            .unwrap()
            .status
            .unwrap();
        let cancel = AtomicBool::new(false);
        // Independently synchronized edit/tag family does not establish a root receipt.
        let unrelated = super::super::probe(
            &folder,
            &data,
            ImportFamilies {
                composition: false,
                edits: false,
                tags: false,
            },
            &cancel,
        );
        assert!(matches!(unrelated, SidecarImportProbe::Current { .. }));
        let first = super::super::probe(&folder, &data, FOLDER, &cancel);
        assert_eq!(
            first.result().composition,
            SidecarProbeFamilyOutcome::ImportRequired
        );
        let prepared = super::super::prepare(loaded(&folder), FOLDER, &cancel).unwrap();
        let SidecarImportCompletion::Current { result, .. } =
            super::super::commit(&data, prepared, &cancel)
        else {
            panic!()
        };
        assert_eq!(result.composition, ImportFamilyOutcome::Applied(true));
        let checked = super::super::probe_for_restore(&folder, &data, FOLDER, &cancel, None, true);
        assert_eq!(
            checked.probe.result().composition,
            SidecarProbeFamilyOutcome::AlreadySynchronized
        );
        let proof = checked.proof_candidate.unwrap();
        let reused =
            super::super::probe_for_restore(&folder, &data, FOLDER, &cancel, Some(&proof), true);
        assert!(matches!(
            reused.reuse,
            super::super::SidecarProbeReuseOutcome::Hit
        ));
        let different = super::super::probe_for_restore(
            &folder,
            &data,
            ImportFamilies {
                composition: false,
                edits: false,
                tags: false,
            },
            &cancel,
            Some(&proof),
            true,
        );
        assert!(!matches!(
            different.reuse,
            super::super::SidecarProbeReuseOutcome::Hit
        ));
        let restored = service
            .prepare(scope, PageDomain::ImageFolder)
            .recv()
            .unwrap()
            .status
            .unwrap()
            .unwrap()
            .composition;
        let mut expected = record();
        expected.revision = restored.revision;
        assert_eq!(restored.revision, 1);
        assert_eq!(restored, expected);
        owner.shutdown_and_join();
    }

    #[test]
    fn folder_import_commit_after_remove_does_not_revive_prepared_root() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("book");
        let data = temp.path().join("data");
        std::fs::create_dir(&folder).unwrap();
        let mut source = SidecarFile::load(&folder);
        source.set_folder_composition(Some(record())).unwrap();
        assert!(source.flush_blocking());
        let (service, owner) = BookCompositionService::spawn_at(data.clone(), None).unwrap();
        let scope = crate::book_scope::BookScopeKey::folder(&folder).unwrap();
        service
            .prepare(scope.clone(), PageDomain::ImageFolder)
            .recv()
            .unwrap()
            .status
            .unwrap();
        let cancel = AtomicBool::new(false);
        let prepared = super::super::prepare(loaded(&folder), FOLDER, &cancel).unwrap();
        service
            .remove_folder(scope.clone(), false)
            .recv()
            .unwrap()
            .status
            .unwrap();
        let SidecarImportCompletion::Current { result, .. } =
            super::super::commit(&data, prepared, &cancel)
        else {
            panic!()
        };
        assert_eq!(result.composition, ImportFamilyOutcome::Applied(false));
        assert!(
            service
                .prepare(scope, PageDomain::ImageFolder)
                .recv()
                .unwrap()
                .status
                .unwrap()
                .is_none()
        );
        owner.shutdown_and_join();
    }
}
