//! Worker-only folder adapter. The composition actor remains the only DB writer;
//! the existing sidecar writer remains the only backup writer.
use super::{BookComposition, PageDomain, store};
use crate::sidecar::{LoadedSidecarImport, SidecarFile, SidecarImportLoad, SidecarImportSource};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

pub(crate) enum FolderSource {
    Missing(PathBuf),
    Loaded(LoadedSidecarImport),
}

pub(crate) fn prepare_source(folder: &Path) -> Result<FolderSource, String> {
    match SidecarFile::load_for_import(folder) {
        SidecarImportLoad::Missing { .. } => Ok(FolderSource::Missing(folder.to_owned())),
        SidecarImportLoad::Loaded(loaded) => Ok(FolderSource::Loaded(loaded)),
        SidecarImportLoad::Unreadable { error, .. } | SidecarImportLoad::Corrupt { error, .. } => {
            Err(error)
        }
        SidecarImportLoad::UnsupportedVersion { version, .. } => {
            Err(format!("unsupported folder backup version {version}"))
        }
        SidecarImportLoad::WriterFailed { .. } => Err("folder backup writer failed".into()),
        SidecarImportLoad::ChangedDuringRead { .. } => {
            Err("folder backup changed during read".into())
        }
    }
}

impl FolderSource {
    pub(crate) fn revalidate(&self) -> Result<(), String> {
        match self {
            Self::Loaded(loaded) => {
                crate::sidecar::revalidate_import_source(loaded.sidecar(), loaded.source())
            }
            Self::Missing(folder) => crate::sidecar::revalidate_missing_import_source(folder),
        }
    }

    pub(crate) fn consume_in(
        &self,
        connection: &Connection,
        table: &str,
        key: &str,
    ) -> Result<(), String> {
        let folder = match self {
            Self::Missing(folder) => folder.as_path(),
            Self::Loaded(loaded) => loaded.sidecar().folder(),
        };
        if crate::book_scope::BookScopeKey::folder(folder)?.storage_key() != key {
            return Err("folder backup belongs to a different composition scope".into());
        }
        if let Self::Loaded(loaded) = self {
            let marker = match loaded.source() {
                SidecarImportSource::Disk(token) => Some(token.sync_marker()),
                SidecarImportSource::PendingWriter { .. } => None,
            };
            store::consume_folder_root_in(
                connection,
                table,
                key,
                loaded.sidecar().folder_composition_digest(),
                marker,
            )?;
        }
        Ok(())
    }
}

/// Recheck both owners inside the same transaction, after strict revalidation.
/// An existing row wins; an identical consumed root can only advance its marker.
pub(crate) fn restore_in(
    connection: &Connection,
    table: &str,
    key: &str,
    sidecar: &SidecarFile,
    source: &SidecarImportSource,
) -> Result<bool, String> {
    if crate::book_scope::BookScopeKey::folder(sidecar.folder())?.storage_key() != key {
        return Err("folder backup belongs to a different composition scope".into());
    }
    crate::sidecar::revalidate_import_source(sidecar, source)?;
    let root = sidecar.folder_composition();
    if let Some(root) = root {
        root.validate(&PageDomain::ImageFolder)
            .map_err(|error| error.to_string())?;
    }
    let digest = sidecar.folder_composition_digest();
    let consumed = store::folder_receipt_in(connection, table, key)?
        .is_some_and(|receipt| receipt.consumed_root_digest == digest);
    let existing = store::row_in(connection, table, key)?;
    if existing
        .as_ref()
        .is_some_and(|row| row.domain != PageDomain::ImageFolder)
    {
        return Err("saved composition kind does not match folder source".into());
    }
    let changed = if existing.is_none() && !consumed {
        root.map(|root| {
            store::replace_row_in(connection, table, key, &PageDomain::ImageFolder, root)
        })
        .transpose()?
        .unwrap_or(false)
    } else {
        false
    };
    let marker = match source {
        SidecarImportSource::Disk(token) => Some(token.sync_marker()),
        SidecarImportSource::PendingWriter { .. } => None,
    };
    store::consume_folder_root_in(connection, table, key, digest, marker)?;
    Ok(changed)
}

/// DB success is retained when the existing backup writer fails. No retry/rollback.
pub(crate) fn mirror(folder: &Path, root: Option<BookComposition>) -> Result<(), String> {
    let warning = "フォルダのバックアップを更新できませんでした。古いバックアップが残る可能性があります。移動前に再保存するか、メタ情報を書き出してください。";
    let mut sidecar = match prepare_source(folder) {
        Ok(FolderSource::Loaded(loaded)) => loaded.sidecar().clone_import_snapshot(),
        Ok(FolderSource::Missing(_)) => SidecarFile::new(folder.to_owned()),
        Err(error) => {
            crate::logger::log(format!("folder composition backup read failed: {error}"));
            return Err(warning.into());
        }
    };
    if let Err(error) = sidecar.set_folder_composition(root) {
        crate::logger::log(format!(
            "folder composition backup preparation failed: {error}"
        ));
        return Err(warning.into());
    }
    if sidecar.flush_blocking() {
        Ok(())
    } else {
        Err(warning.into())
    }
}
