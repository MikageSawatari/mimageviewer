use std::sync::{Arc, Mutex};

use crate::settings::FavoriteEntry;

enum FavoritesSource {
    Live(crate::settings_db::SettingsFavoritesReader),
    #[cfg(test)]
    Snapshot(Arc<Vec<FavoriteEntry>>),
}

/// Remote のお気に入り一覧・お気に入り検索が共有するライブ snapshot。
///
/// お気に入りは表示と検索範囲のために使い、ファイルアクセス境界には使わない。
pub(super) struct LiveFavorites {
    source: Mutex<FavoritesSource>,
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
            source: Mutex::new(FavoritesSource::Live(reader)),
        }))
    }

    #[cfg(test)]
    pub(super) fn snapshot(initial: Vec<FavoriteEntry>) -> Arc<Self> {
        Arc::new(Self {
            source: Mutex::new(FavoritesSource::Snapshot(Arc::new(initial))),
        })
    }

    pub(super) fn current(&self) -> Result<Arc<Vec<FavoriteEntry>>, String> {
        let mut source = self
            .source
            .lock()
            .map_err(|_| "remote favorites lock poisoned".to_owned())?;
        match &mut *source {
            FavoritesSource::Live(reader) => reader
                .current()
                .map_err(|error| format!("remote favorites refresh failed: {error}")),
            #[cfg(test)]
            FavoritesSource::Snapshot(favorites) => Ok(Arc::clone(favorites)),
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
}
