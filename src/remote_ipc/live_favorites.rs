use std::sync::{Arc, Mutex};

use mimageviewer_registered_roots::{RegisteredRootCatalog, RegisteredRootsSnapshot};

use crate::settings::FavoriteEntry;

pub(super) struct AllowedRootsSnapshot {
    pub favorites: Arc<Vec<FavoriteEntry>>,
    pub registered: Arc<RegisteredRootsSnapshot>,
}

enum RootsSource {
    Live {
        favorites: crate::settings_db::SettingsFavoritesReader,
        registered: RegisteredRootCatalog,
    },
    Snapshot {
        favorites: Arc<Vec<FavoriteEntry>>,
        registered: Arc<RegisteredRootsSnapshot>,
    },
}

/// Every Remote IPC allowlist consumer shares this one current root snapshot.
///
/// Production checks persistent read-only SQLite connections' `data_version` on every request.
/// Test constructors remain explicit snapshots so they stay isolated from GLOBAL_DB.
pub(super) struct RemoteRoots {
    source: Mutex<RootsSource>,
}

impl RemoteRoots {
    pub(super) fn live(initial: Vec<FavoriteEntry>) -> Result<Arc<Self>, String> {
        let data_dir = crate::data_dir::get();
        let settings_path = data_dir.join("settings.db");
        let favorites = crate::settings_db::SettingsFavoritesReader::open_existing_read_only_at(
            &settings_path,
            initial,
        )
        .map_err(|error| format!("remote favorites read-only open failed: {error}"))?;
        let registered = RegisteredRootCatalog::open(&data_dir)
            .map_err(|error| format!("remote registered roots read-only open failed: {error}"))?;
        log_registered_limit(registered.snapshot().as_ref());
        Ok(Arc::new(Self {
            source: Mutex::new(RootsSource::Live {
                favorites,
                registered,
            }),
        }))
    }

    pub(super) fn snapshot(initial: Vec<FavoriteEntry>) -> Arc<Self> {
        Self::snapshot_with_registered(initial, RegisteredRootsSnapshot::empty())
    }

    pub(super) fn snapshot_with_registered(
        initial: Vec<FavoriteEntry>,
        registered: Arc<RegisteredRootsSnapshot>,
    ) -> Arc<Self> {
        Arc::new(Self {
            source: Mutex::new(RootsSource::Snapshot {
                favorites: Arc::new(initial),
                registered,
            }),
        })
    }

    pub(super) fn current(&self) -> Result<AllowedRootsSnapshot, String> {
        let mut source = self
            .source
            .lock()
            .map_err(|_| "remote roots lock poisoned".to_owned())?;
        match &mut *source {
            RootsSource::Live {
                favorites,
                registered,
            } => {
                let favorites = favorites
                    .current()
                    .map_err(|error| format!("remote favorites refresh failed: {error}"))?;
                let changed = registered
                    .refresh()
                    .map_err(|error| format!("remote registered roots refresh failed: {error}"))?;
                let registered = registered.snapshot();
                if changed {
                    log_registered_limit(registered.as_ref());
                }
                Ok(AllowedRootsSnapshot {
                    favorites,
                    registered,
                })
            }
            RootsSource::Snapshot {
                favorites,
                registered,
            } => Ok(AllowedRootsSnapshot {
                favorites: Arc::clone(favorites),
                registered: Arc::clone(registered),
            }),
        }
    }
}

fn log_registered_limit(snapshot: &RegisteredRootsSnapshot) {
    if snapshot.limit_reached() {
        crate::logger::log(format!(
            "remote_ipc: registered root limit reached discovered={} retained={}",
            snapshot.discovered_count(),
            snapshot.roots().len()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_ipc::path_guard::{ResolveError, resolve_existing};

    #[test]
    fn live_roots_reject_a_favorite_after_its_committed_deletion() {
        let data_dir = crate::settings_db::DataDirOverrideGuard::new();
        let root = data_dir.path().join("favorite");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("page.jpg"), b"page").unwrap();
        let favorite = FavoriteEntry::new("favorite".to_owned(), root);
        let mut settings = crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        };
        let db = crate::settings_db::SettingsDb::create_new(data_dir.path()).unwrap();
        db.save_full(&settings).unwrap();

        let roots = RemoteRoots::live(settings.favorites.clone()).unwrap();
        assert!(
            resolve_existing(
                &roots.current().unwrap(),
                &favorite.id.to_string(),
                "page.jpg"
            )
            .is_ok()
        );

        settings.favorites.clear();
        db.save_full(&settings).unwrap();
        assert!(matches!(
            resolve_existing(
                &roots.current().unwrap(),
                &favorite.id.to_string(),
                "page.jpg"
            ),
            Err(ResolveError::RootNotFound)
        ));
    }
}
