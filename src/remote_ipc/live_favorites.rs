use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::settings::FavoriteEntry;

enum FavoritesSource {
    Live(crate::settings_db::SettingsFavoritesReader),
    Paused {
        generation: u64,
        cached: Arc<Vec<FavoriteEntry>>,
    },
    ResumeFailed {
        generation: u64,
        cached: Arc<Vec<FavoriteEntry>>,
        error: String,
    },
    #[cfg(test)]
    Snapshot(Arc<Vec<FavoriteEntry>>),
}

#[derive(Debug)]
pub(super) enum LiveFavoritesError {
    Busy(String),
    Other(String),
}

impl LiveFavoritesError {
    pub(super) fn is_busy(&self) -> bool {
        matches!(self, Self::Busy(_))
    }
}

impl std::fmt::Display for LiveFavoritesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy(message) | Self::Other(message) => f.write_str(message),
        }
    }
}

/// Remote のお気に入り一覧・お気に入り検索が共有するライブ snapshot。
///
/// お気に入りは表示と検索範囲のために使い、ファイルアクセス境界には使わない。
pub(super) struct LiveFavorites {
    settings_path: Option<PathBuf>,
    source: Mutex<FavoritesSource>,
}

/// Opaque control for the one production persistent settings reader. It points to the same
/// `LiveFavorites` instance injected into every collection worker.
#[derive(Clone)]
pub(crate) struct RemoteSettingsReaderControl {
    favorites: Arc<LiveFavorites>,
}

impl LiveFavorites {
    pub(super) fn live(initial: Vec<FavoriteEntry>) -> Result<Arc<Self>, String> {
        let settings_path = crate::data_dir::get().join("settings.db");
        let reader = crate::settings_db::SettingsFavoritesReader::open_existing_read_only_at(
            &settings_path,
            initial,
        )
        .map_err(|error| format!("remote favorites read-only open failed: {error}"))?;
        Ok(Arc::new(Self {
            settings_path: Some(settings_path),
            source: Mutex::new(FavoritesSource::Live(reader)),
        }))
    }

    #[cfg(test)]
    pub(super) fn snapshot(initial: Vec<FavoriteEntry>) -> Arc<Self> {
        Arc::new(Self {
            settings_path: None,
            source: Mutex::new(FavoritesSource::Snapshot(Arc::new(initial))),
        })
    }

    pub(super) fn control(self: &Arc<Self>) -> RemoteSettingsReaderControl {
        RemoteSettingsReaderControl {
            favorites: Arc::clone(self),
        }
    }

    pub(super) fn current(&self) -> Result<Arc<Vec<FavoriteEntry>>, LiveFavoritesError> {
        let _family_lease = if self.settings_path.is_some() {
            Some(
                crate::settings_db::acquire_settings_family_read_lease().map_err(|error| {
                    if matches!(
                        error,
                        crate::settings_db::SettingsDbError::SettingsFamilyQuiescing
                    ) {
                        LiveFavoritesError::Busy(error.to_string())
                    } else {
                        LiveFavoritesError::Other(error.to_string())
                    }
                })?,
            )
        } else {
            None
        };
        let mut source = self
            .source
            .lock()
            .map_err(|_| LiveFavoritesError::Other("remote favorites lock poisoned".to_owned()))?;
        match &mut *source {
            FavoritesSource::Live(reader) => reader.current().map_err(|error| {
                LiveFavoritesError::Other(format!("remote favorites refresh failed: {error}"))
            }),
            FavoritesSource::Paused { .. } => Err(LiveFavoritesError::Busy(
                "settings recovery is pausing remote favorites".to_owned(),
            )),
            FavoritesSource::ResumeFailed { error, .. } => Err(LiveFavoritesError::Busy(format!(
                "remote favorites reader is unavailable and can be retried: {error}"
            ))),
            #[cfg(test)]
            FavoritesSource::Snapshot(favorites) => Ok(Arc::clone(favorites)),
        }
    }
}

impl RemoteSettingsReaderControl {
    #[cfg(test)]
    pub(crate) fn open_for_test(initial: Vec<FavoriteEntry>) -> Result<Self, String> {
        let favorites = LiveFavorites::live(initial)?;
        Ok(favorites.control())
    }

    /// Drop the persistent connection after the process-wide read domain reached active=0.
    pub(crate) fn pause(
        &self,
        permit: &crate::settings_db::SettingsFamilyMutationPermit,
    ) -> Result<(), String> {
        permit
            .validate()
            .map_err(|error| format!("settings-family permit rejected: {error}"))?;
        let mut source = self
            .favorites
            .source
            .lock()
            .map_err(|_| "remote favorites lock poisoned".to_owned())?;
        let cached = match &*source {
            FavoritesSource::Live(reader) => reader.cached(),
            FavoritesSource::Paused { generation, .. } if *generation == permit.generation() => {
                return Ok(());
            }
            FavoritesSource::ResumeFailed { generation, .. }
                if *generation == permit.generation() =>
            {
                return Ok(());
            }
            FavoritesSource::Paused { generation, .. }
            | FavoritesSource::ResumeFailed { generation, .. } => {
                return Err(format!(
                    "remote favorites is owned by generation {generation}, requested {}",
                    permit.generation()
                ));
            }
            #[cfg(test)]
            FavoritesSource::Snapshot(_) => return Ok(()),
        };
        let old = std::mem::replace(
            &mut *source,
            FavoritesSource::Paused {
                generation: permit.generation(),
                cached,
            },
        );
        drop(source);
        drop(old);
        Ok(())
    }

    /// Reopen exactly the reader paused by this operation. On failure the cached identity remains
    /// unavailable until an explicit retry; no request silently constructs another reader.
    pub(crate) fn resume_after_mutation(
        &self,
        permit: &crate::settings_db::SettingsFamilyMutationPermit,
    ) -> Result<(), String> {
        permit
            .validate()
            .map_err(|error| format!("settings-family permit rejected: {error}"))?;
        self.resume_inner(permit.generation())
    }

    /// Retry a prior recoverable resume after ordinary settings access has reopened. The read
    /// lease prevents a new family mutation from beginning between SQLite open and source install.
    pub(crate) fn retry_resume(&self, generation: u64) -> Result<(), String> {
        let _family_lease = crate::settings_db::acquire_settings_family_read_lease()
            .map_err(|error| error.to_string())?;
        self.resume_inner(generation)
    }

    fn resume_inner(&self, generation: u64) -> Result<(), String> {
        let settings_path = self
            .favorites
            .settings_path
            .as_ref()
            .ok_or_else(|| "snapshot favorites has no persistent reader".to_owned())?
            .clone();
        let cached = {
            let source = self
                .favorites
                .source
                .lock()
                .map_err(|_| "remote favorites lock poisoned".to_owned())?;
            match &*source {
                FavoritesSource::Paused {
                    generation: owner,
                    cached,
                }
                | FavoritesSource::ResumeFailed {
                    generation: owner,
                    cached,
                    ..
                } if *owner == generation => Arc::clone(cached),
                FavoritesSource::Live(_) => {
                    return Err("remote favorites reader is not paused".to_owned());
                }
                FavoritesSource::Paused {
                    generation: owner, ..
                }
                | FavoritesSource::ResumeFailed {
                    generation: owner, ..
                } => {
                    return Err(format!(
                        "remote favorites is owned by generation {owner}, requested {generation}"
                    ));
                }
                #[cfg(test)]
                FavoritesSource::Snapshot(_) => return Ok(()),
            }
        };
        match crate::settings_db::SettingsFavoritesReader::open_existing_read_only_at(
            &settings_path,
            cached.as_ref().clone(),
        ) {
            Ok(reader) => {
                let mut source = self
                    .favorites
                    .source
                    .lock()
                    .map_err(|_| "remote favorites lock poisoned".to_owned())?;
                match &*source {
                    FavoritesSource::Paused {
                        generation: owner, ..
                    }
                    | FavoritesSource::ResumeFailed {
                        generation: owner, ..
                    } if *owner == generation => {
                        *source = FavoritesSource::Live(reader);
                        Ok(())
                    }
                    _ => Err("remote favorites resume became stale".to_owned()),
                }
            }
            Err(error) => {
                let message = format!("remote favorites read-only reopen failed: {error}");
                let mut source = self
                    .favorites
                    .source
                    .lock()
                    .map_err(|_| "remote favorites lock poisoned".to_owned())?;
                match &*source {
                    FavoritesSource::Paused {
                        generation: owner, ..
                    }
                    | FavoritesSource::ResumeFailed {
                        generation: owner, ..
                    } if *owner == generation => {
                        *source = FavoritesSource::ResumeFailed {
                            generation,
                            cached,
                            error: message.clone(),
                        };
                    }
                    _ => return Err("remote favorites resume failure became stale".to_owned()),
                }
                Err(message)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_favorites_reflect_a_committed_deletion() {
        let data_dir = crate::settings_db::DataDirOverrideGuard::new();
        let root = data_dir.path().join("favorite");
        std::fs::create_dir(&root).unwrap();
        let favorite = FavoriteEntry::new("favorite".to_owned(), root);
        let mut settings = crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        };
        let db = crate::settings_db::SettingsDb::create_new(data_dir.path()).unwrap();
        db.save_full(&settings).unwrap();

        let favorites = LiveFavorites::live(settings.favorites.clone()).unwrap();
        let current = favorites.current().unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].id, favorite.id);
        assert_eq!(current[0].name, favorite.name);
        assert_eq!(current[0].path, favorite.path);

        settings.favorites.clear();
        db.save_full(&settings).unwrap();
        assert!(favorites.current().unwrap().is_empty());
    }

    #[test]
    fn persistent_reader_pause_is_generation_exact_and_reopens_after_recoverable_mutation() {
        let data_dir = crate::settings_db::DataDirOverrideGuard::new();
        let settings = crate::settings::Settings::default();
        let db = crate::settings_db::SettingsDb::create_new(data_dir.path()).unwrap();
        db.save_full(&settings).unwrap();
        drop(db);

        let favorites = LiveFavorites::live(settings.favorites.clone()).unwrap();
        let control = favorites.control();
        let first = crate::settings_db::quiesce_settings_family().unwrap();
        let first_generation = first.generation();
        control.pause(&first).unwrap();
        assert!(favorites.current().unwrap_err().is_busy());
        control.resume_after_mutation(&first).unwrap();
        first.resume_local().unwrap();
        assert!(favorites.current().unwrap().is_empty());
        assert!(control.retry_resume(first_generation).is_err());

        let second = crate::settings_db::quiesce_settings_family().unwrap();
        let second_generation = second.generation();
        assert_ne!(first_generation, second_generation);
        control.pause(&second).unwrap();
        second.resume_local().unwrap();
        let stale = control.retry_resume(first_generation).unwrap_err();
        assert!(stale.contains("owned by generation"));
        control.retry_resume(second_generation).unwrap();
        assert!(favorites.current().unwrap().is_empty());
    }

    #[test]
    fn resume_failure_reopens_local_domain_and_only_favorites_stays_busy_until_retry() {
        let data_dir = crate::settings_db::DataDirOverrideGuard::new();
        let settings = crate::settings::Settings::default();
        let db = crate::settings_db::SettingsDb::create_new(data_dir.path()).unwrap();
        db.save_full(&settings).unwrap();
        drop(db);

        let favorites = LiveFavorites::live(settings.favorites.clone()).unwrap();
        let control = favorites.control();
        let permit = crate::settings_db::quiesce_settings_family().unwrap();
        let generation = permit.generation();
        control.pause(&permit).unwrap();
        for suffix in ["settings.db-shm", "settings.db-wal", "settings.db"] {
            let _ = std::fs::remove_file(data_dir.path().join(suffix));
        }
        assert!(control.resume_after_mutation(&permit).is_err());
        permit.resume_local().unwrap();
        assert!(favorites.current().unwrap_err().is_busy());

        // Ordinary short settings access is independent of the failed persistent reader.
        let replacement = crate::settings_db::SettingsDb::create_new(data_dir.path()).unwrap();
        replacement.save_full(&settings).unwrap();
        drop(replacement);
        crate::settings_db::with_db_result(|db| db.load_sort_order()).unwrap();
        control.retry_resume(generation).unwrap();
        assert!(favorites.current().unwrap().is_empty());
    }
}
