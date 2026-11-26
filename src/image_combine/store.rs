//! Worker-only recipe persistence and the UI's key-only, generation-checked index.
pub(crate) use super::CombineRecipe;
use super::{CombineSourceRef, Destination};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS combine_recipes (
    output_key TEXT PRIMARY KEY,
    recipe TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);";

pub(crate) fn source_path(source: &CombineSourceRef) -> &Path {
    match source {
        CombineSourceRef::File { path } => path,
        CombineSourceRef::ZipEntry { zip_path, .. } => zip_path,
        CombineSourceRef::PdfPage { pdf_path, .. } => pdf_path,
    }
}

fn source_path_mut(source: &mut CombineSourceRef) -> &mut PathBuf {
    match source {
        CombineSourceRef::File { path } => path,
        CombineSourceRef::ZipEntry { zip_path, .. } => zip_path,
        CombineSourceRef::PdfPage { pdf_path, .. } => pdf_path,
    }
}

fn connection(path: &Path, create: bool) -> Result<Connection, String> {
    let flags = if create {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let connection = Connection::open_with_flags(path, flags).map_err(|e| e.to_string())?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    Ok(connection)
}

/// No migrations: this store has not shipped before recipe v1.
pub(crate) fn initialize_database_at(path: &Path) -> Result<(), String> {
    connection(path, true)?
        .execute_batch(SCHEMA)
        .map_err(|e| e.to_string())
}

fn existing_connection(data_dir: &Path) -> Result<Option<Connection>, String> {
    let path = data_dir.join("combine.db");
    match std::fs::metadata(&path) {
        Ok(_) => connection(&path, false).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn validate_recipe(recipe: &CombineRecipe) -> Result<(), String> {
    if recipe.v != 1 {
        return Err(format!("結合の構成の版に対応していません: {}", recipe.v));
    }
    if recipe.items.is_empty() || recipe.items.len() > 100 {
        return Err("結合の構成の画像数が無効です".into());
    }
    if recipe
        .items
        .iter()
        .any(|item| !source_path(&item.source).is_absolute())
    {
        return Err("結合の構成に絶対パスでない画像があります".into());
    }
    if recipe.items.iter().any(|item| {
        item.annotation_space
            .is_some_and(|[width, height]| width == 0 || height == 0)
    }) {
        return Err("結合の構成に無効な注釈の基準寸法があります".into());
    }
    Ok(())
}

/// Called after the output file is committed, on the session's worker.
pub(crate) fn save_recipe(
    data_dir: &Path,
    output: &Path,
    recipe: &CombineRecipe,
) -> Result<(), String> {
    let output = std::path::absolute(output).map_err(|e| e.to_string())?;
    let mut recipe = recipe.clone();
    for item in &mut recipe.items {
        let path = source_path_mut(&mut item.source);
        *path = std::path::absolute(&*path).map_err(|e| e.to_string())?;
    }
    if let Destination::Folder(path) = &mut recipe.options.destination {
        *path = std::path::absolute(&*path).map_err(|e| e.to_string())?;
    }
    validate_recipe(&recipe)?;
    let json = serde_json::to_string(&recipe).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let connection = connection(&data_dir.join("combine.db"), true)?;
    connection
        .execute_batch(SCHEMA)
        .map_err(|e| e.to_string())?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis()
        .min(i64::MAX as u128) as i64;
    connection.execute(
        "INSERT INTO combine_recipes(output_key, recipe, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(output_key) DO UPDATE SET recipe=excluded.recipe, updated_at=excluded.updated_at",
        rusqlite::params![crate::adjustment_db::normalize_path(&output), json, now],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn load_recipe(data_dir: &Path, output: &Path) -> Result<Option<CombineRecipe>, String> {
    let Some(connection) = existing_connection(data_dir)? else {
        return Ok(None);
    };
    let json: Option<String> = connection
        .query_row(
            "SELECT recipe FROM combine_recipes WHERE output_key=?1",
            [crate::adjustment_db::normalize_path(output)],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    json.map(|json| {
        let recipe =
            serde_json::from_str(&json).map_err(|e| format!("結合の構成を読めません: {e}"))?;
        validate_recipe(&recipe)?;
        Ok(recipe)
    })
    .transpose()
}

fn load_keys(data_dir: &Path) -> Result<HashSet<String>, String> {
    let Some(connection) = existing_connection(data_dir)? else {
        return Ok(HashSet::new());
    };
    let mut statement = connection
        .prepare("SELECT output_key FROM combine_recipes")
        .map_err(|e| e.to_string())?;
    let keys = statement
        .query_map([], |row| row.get(0))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<_>>()
        .map_err(|e| e.to_string());
    keys
}

type IndexResult = (u64, Result<HashSet<String>, String>);

pub(crate) struct CombineIndex {
    keys: HashSet<String>,
    loaded: bool,
    generation: u64,
    stale: bool,
    tx: mpsc::Sender<IndexResult>,
    rx: mpsc::Receiver<IndexResult>,
}

impl Default for CombineIndex {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            keys: HashSet::new(),
            loaded: false,
            generation: 0,
            stale: true,
            tx,
            rx,
        }
    }
}

impl CombineIndex {
    #[cfg(test)]
    pub(crate) fn set_keys_for_test(&mut self, keys: HashSet<String>) {
        self.generation = self.generation.wrapping_add(1);
        self.keys = keys;
        self.loaded = true;
        self.stale = false;
    }

    pub(crate) fn contains_or_unloaded(&self, normalized_key: &str) -> bool {
        !self.loaded || self.keys.contains(normalized_key)
    }

    pub(crate) fn request_reload(&mut self, data_dir: PathBuf, ctx: egui::Context) {
        self.stale = false;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let tx = self.tx.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("combine-index".into())
            .spawn(move || {
                let _ = tx.send((generation, load_keys(&data_dir)));
                ctx.request_repaint();
            })
        {
            crate::logger::log(format!("combine index worker could not start: {error}"));
        }
    }

    /// UI-only: adoption performs no SQLite or filesystem calls. Errors preserve the last index.
    pub(crate) fn mark_stale(&mut self) {
        self.stale = true;
    }

    pub(crate) fn poll(&mut self, data_dir: &Path, ctx: &egui::Context) {
        if self.stale {
            self.request_reload(data_dir.to_owned(), ctx.clone());
        }
        while let Ok((generation, result)) = self.rx.try_recv() {
            self.apply(generation, result);
        }
    }

    fn apply(&mut self, generation: u64, result: Result<HashSet<String>, String>) {
        if generation != self.generation {
            return;
        }
        match result {
            Ok(keys) => {
                self.keys = keys;
                self.loaded = true;
            }
            Err(error) => crate::logger::log(format!("combine index load failed: {error}")),
        }
    }
}

#[cfg(test)]
mod image_combine_tests;
