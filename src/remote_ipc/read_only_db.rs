use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// A missing DB may be created by App after Remote starts. Only that state is
/// reopened on a worker read; successful connections and open errors are retained.
pub(super) struct LateCreatedReadOnlyDb<T> {
    path: PathBuf,
    pub(super) state: Mutex<Result<Option<T>, String>>,
    open: fn(&Path) -> rusqlite::Result<Option<T>>,
    label: &'static str,
}

impl<T> LateCreatedReadOnlyDb<T> {
    /// Keep intentionally absent test readers independent of the process data_dir.
    #[cfg(test)]
    pub(super) fn missing_fixture() -> Self {
        Self::new(PathBuf::new(), |_| Ok(None), "fixture")
    }

    pub(super) fn new(
        path: PathBuf,
        open: fn(&Path) -> rusqlite::Result<Option<T>>,
        label: &'static str,
    ) -> Self {
        let db = Self {
            path,
            state: Mutex::new(Ok(None)),
            open,
            label,
        };
        drop(db.read());
        db
    }

    pub(super) fn read(&self) -> MutexGuard<'_, Result<Option<T>, String>> {
        self.read_with_lock(|state| state.lock().unwrap_or_else(|error| error.into_inner()))
    }

    /// Allow the page pipeline to measure mutex wait without counting DB open time.
    pub(super) fn read_with_lock<'a>(
        &'a self,
        lock: impl FnOnce(
            &'a Mutex<Result<Option<T>, String>>,
        ) -> MutexGuard<'a, Result<Option<T>, String>>,
    ) -> MutexGuard<'a, Result<Option<T>, String>> {
        let mut state = lock(&self.state);
        if matches!(&*state, Ok(None)) {
            // The opener checks existence once and never creates the DB. Do not
            // add a second stat here or retry an actual open failure.
            *state = (self.open)(&self.path).map_err(|error| {
                crate::logger::log(format!(
                    "remote_ipc: {} DB read-only open failed: {error}",
                    self.label
                ));
                error.to_string()
            });
        }
        state
    }
}

/// Adapt read-only openers that report a missing file as an error. Do not call
/// this around an opener that already checks existence (spread / view-trim).
pub(super) fn open_existing_read_only<T>(
    path: &Path,
    open: fn(&Path) -> rusqlite::Result<T>,
) -> rusqlite::Result<Option<T>> {
    match path.try_exists() {
        Ok(false) => return Ok(None),
        Ok(true) => {}
        Err(error) => return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(error))),
    }
    open(path).map(Some)
}
