use std::sync::{Arc, Mutex};

use crate::settings::FavoriteEntry;

enum FavoriteRootsSource {
    Live(crate::settings_db::SettingsFavoritesReader),
    Snapshot(Arc<Vec<FavoriteEntry>>),
}

/// Remote IPC 内の全 allowlist consumer が共有する最新 favorite roots。
///
/// production は専用 read-only SQLite connection の `data_version` を request ごとに確認し、
/// 変更時だけ全件を読み直す。test constructor は明示 snapshot のままにして GLOBAL_DB と分離する。
pub(super) struct RemoteFavoriteRoots {
    source: Mutex<FavoriteRootsSource>,
}

impl RemoteFavoriteRoots {
    pub(super) fn live(initial: Vec<FavoriteEntry>) -> Result<Arc<Self>, String> {
        let path = crate::data_dir::get().join("settings.db");
        let reader =
            crate::settings_db::SettingsFavoritesReader::open_existing_read_only_at(&path, initial)
                .map_err(|error| format!("remote favorites read-only open failed: {error}"))?;
        Ok(Arc::new(Self {
            source: Mutex::new(FavoriteRootsSource::Live(reader)),
        }))
    }

    pub(super) fn snapshot(initial: Vec<FavoriteEntry>) -> Arc<Self> {
        Arc::new(Self {
            source: Mutex::new(FavoriteRootsSource::Snapshot(Arc::new(initial))),
        })
    }

    pub(super) fn current(&self) -> Result<Arc<Vec<FavoriteEntry>>, String> {
        let mut source = self
            .source
            .lock()
            .map_err(|_| "remote favorites lock poisoned".to_owned())?;
        match &mut *source {
            FavoriteRootsSource::Live(reader) => reader
                .current()
                .map_err(|error| format!("remote favorites refresh failed: {error}")),
            FavoriteRootsSource::Snapshot(favorites) => Ok(Arc::clone(favorites)),
        }
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

        let roots = RemoteFavoriteRoots::live(settings.favorites.clone()).unwrap();
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
            Err(ResolveError::FavoriteNotFound)
        ));
    }
}
