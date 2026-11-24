//! Location-based policy shared by requests, cache retention and worker preparation.
use super::AutoCropIdentity;
use crate::settings::{FavoriteCoverAutoCropPreference, Settings};
use std::path::Path;
use std::sync::Arc;

#[cfg(test)]
thread_local! {
    static SCOPE_BUILDS_FOR_TEST: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CoverAutoCropScope {
    global: bool,
    identity: Option<AutoCropIdentity>,
    // Normalized roots, deepest first. FollowGlobal favorites are transparent.
    roots: Arc<[(String, bool)]>,
}

impl CoverAutoCropScope {
    pub(crate) fn from_settings(settings: &Settings) -> Self {
        #[cfg(test)]
        SCOPE_BUILDS_FOR_TEST.with(|count| count.set(count.get() + 1));
        let mut roots: Vec<_> = settings
            .favorites
            .iter()
            .filter_map(|favorite| {
                let enabled = match favorite.cover_auto_crop {
                    FavoriteCoverAutoCropPreference::FollowGlobal => return None,
                    FavoriteCoverAutoCropPreference::On => true,
                    FavoriteCoverAutoCropPreference::Off => false,
                };
                Some((
                    crate::search_index_db::normalize_path(&favorite.path),
                    enabled,
                ))
            })
            .collect();
        roots.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        roots.dedup_by(|a, b| a.0 == b.0);
        let global = settings.thumb_cover_auto_crop;
        let identity =
            (global || roots.iter().any(|(_, enabled)| *enabled)).then_some(AutoCropIdentity {
                binding: settings.thumb_cover_auto_crop_binding,
                folders: settings.thumb_cover_auto_crop_folders,
            });
        Self {
            global,
            identity,
            roots: roots.into(),
        }
    }

    #[cfg(test)]
    pub(crate) fn build_count_for_test() -> usize {
        SCOPE_BUILDS_FOR_TEST.with(std::cell::Cell::get)
    }

    /// A uniform policy for callers which already own a resolved identity.
    pub(crate) fn for_identity(identity: Option<AutoCropIdentity>) -> Self {
        Self {
            global: identity.is_some(),
            identity,
            roots: Default::default(),
        }
    }

    pub(crate) fn identity_for(&self, path: &Path) -> Option<AutoCropIdentity> {
        if self.roots.is_empty() {
            return self.global.then_some(self.identity).flatten();
        }
        let path = crate::search_index_db::normalize_path(path);
        let enabled = self
            .roots
            .iter()
            .find_map(|(root, enabled)| {
                let under = path == *root
                    || path
                        .strip_prefix(root.as_str())
                        .is_some_and(|tail| root.ends_with('/') || tail.starts_with('/'));
                under.then_some(*enabled)
            })
            .unwrap_or(self.global);
        enabled.then_some(self.identity).flatten()
    }

    /// The same effective policy representation is used for equality and collection digests.
    pub(crate) fn digest_bytes(&self) -> Vec<u8> {
        let mut out = vec![u8::from(self.global)];
        if let Some(identity) = self.identity {
            out.extend_from_slice(
                identity
                    .suffix_for_key(crate::thumb_loader::CACHE_KEY_FOLDER)
                    .as_bytes(),
            );
        }
        out.push(0);
        for (root, enabled) in self.roots.iter() {
            out.extend_from_slice(&(root.len() as u64).to_le_bytes());
            out.extend_from_slice(root.as_bytes());
            out.push(u8::from(*enabled));
        }
        out
    }
}

impl PartialEq for CoverAutoCropScope {
    fn eq(&self, other: &Self) -> bool {
        self.global == other.global && self.identity == other.identity && self.roots == other.roots
    }
}
impl Eq for CoverAutoCropScope {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FavoriteEntry, ReadingDirection};

    fn favorite(path: &str, preference: FavoriteCoverAutoCropPreference) -> FavoriteEntry {
        let mut favorite = FavoriteEntry::new(path.into(), path.into());
        favorite.cover_auto_crop = preference;
        favorite
    }

    #[test]
    fn favorite_cover_auto_crop_deepest_value_and_transparent_follow_global() {
        use FavoriteCoverAutoCropPreference::*;
        let settings = Settings {
            thumb_cover_auto_crop: true,
            favorites: vec![
                favorite("C:/Books", Off),
                favorite("C:/Books/Novels", On),
                favorite("C:/Books/Novels/Series", FollowGlobal),
            ],
            ..Default::default()
        };
        let scope = CoverAutoCropScope::from_settings(&settings);
        assert!(scope.identity_for(Path::new("C:/Books/book.zip")).is_none());
        assert!(
            scope
                .identity_for(Path::new("C:/Books/Novels/Series/book.zip"))
                .is_some()
        );
        assert!(
            scope
                .identity_for(Path::new("C:/Outside/book.zip"))
                .is_some()
        );
        let mut reordered = settings.clone();
        reordered.favorites.reverse();
        assert_eq!(scope, CoverAutoCropScope::from_settings(&reordered));
        fn assert_send<T: Send + Clone>() {}
        assert_send::<CoverAutoCropScope>();
    }

    #[test]
    fn favorite_cover_auto_crop_path_matching_uses_normalized_boundaries() {
        let settings = Settings {
            favorites: vec![favorite(r"C:\BOOKS", FavoriteCoverAutoCropPreference::On)],
            ..Default::default()
        };
        let scope = CoverAutoCropScope::from_settings(&settings);
        for path in [
            r"c:\books",
            "c:/books/Book.zip",
            "C:/BOOKS/sub/book.pdf",
            "C:/Bookshelf/book.zip",
            "D:/Books/book.zip",
        ] {
            assert_eq!(
                scope.identity_for(Path::new(path)).is_some(),
                crate::search_index_db::is_under(Path::new(path), Path::new(r"C:\BOOKS")),
                "{path}"
            );
        }
    }

    #[test]
    fn favorite_cover_auto_crop_global_off_keeps_identity_for_favorite_on() {
        let mut settings = Settings::default();
        let off = CoverAutoCropScope::from_settings(&settings);
        settings.thumb_cover_auto_crop_binding = ReadingDirection::Ltr;
        settings.thumb_cover_auto_crop_folders = true;
        assert_eq!(off, CoverAutoCropScope::from_settings(&settings));
        settings
            .favorites
            .push(favorite("C:/Books", FavoriteCoverAutoCropPreference::On));
        let enabled = CoverAutoCropScope::from_settings(&settings);
        assert_eq!(
            enabled.identity_for(Path::new("C:/Books/a.zip")),
            Some(AutoCropIdentity {
                binding: ReadingDirection::Ltr,
                folders: true
            })
        );
        assert!(
            enabled
                .identity_for(Path::new("C:/Outside/a.zip"))
                .is_none()
        );
        settings.thumb_cover_auto_crop_binding = ReadingDirection::Rtl;
        assert_ne!(enabled, CoverAutoCropScope::from_settings(&settings));
        settings.thumb_cover_auto_crop_binding = ReadingDirection::Ltr;
        settings.thumb_cover_auto_crop_folders = false;
        assert_ne!(enabled, CoverAutoCropScope::from_settings(&settings));
    }

    #[test]
    fn favorite_cover_auto_crop_no_override_is_uniform_without_path() {
        let mut settings = Settings::default();
        settings.favorites.push(favorite(
            "C:/Books",
            FavoriteCoverAutoCropPreference::FollowGlobal,
        ));
        for enabled in [false, true] {
            settings.thumb_cover_auto_crop = enabled;
            let scope = CoverAutoCropScope::from_settings(&settings);
            assert!(scope.roots.is_empty());
            assert_eq!(scope.identity_for(Path::new("")).is_some(), enabled);
            assert_eq!(
                scope.identity_for(Path::new("unrelated")),
                scope.identity_for(Path::new(""))
            );
        }
    }
}
