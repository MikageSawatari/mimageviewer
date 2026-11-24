use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
};

use super::{BatchDestination, BatchExportItem, OverwritePolicy};

fn component_eq(a: Component<'_>, b: Component<'_>) -> bool {
    a.as_os_str().to_string_lossy().to_lowercase() == b.as_os_str().to_string_lossy().to_lowercase()
}

pub(super) fn common_ancestor(items: &[BatchExportItem]) -> Option<PathBuf> {
    let mut common: Vec<_> = items.first()?.source_dir.components().collect();
    for item in &items[1..] {
        let len = common
            .iter()
            .copied()
            .zip(item.source_dir.components())
            .take_while(|(a, b)| component_eq(*a, *b))
            .count();
        common.truncate(len);
    }
    if common.is_empty() {
        None
    } else {
        Some(common.iter().collect())
    }
}

pub(super) fn output_directory(
    destination: &BatchDestination,
    item: &BatchExportItem,
    base: Option<&Path>,
) -> PathBuf {
    match destination {
        BatchDestination::SameAsSource { subfolder } => {
            if subfolder.is_empty() {
                item.source_dir.clone()
            } else {
                item.source_dir
                    .join(crate::capture::basename_from_text(subfolder))
            }
        }
        BatchDestination::Folder {
            path,
            keep_structure: false,
        } => path.clone(),
        BatchDestination::Folder {
            path,
            keep_structure: true,
        } => {
            let mut output = path.clone();
            if let Some(base) = base {
                let mut relative = PathBuf::new();
                for component in item.source_dir.components().skip(base.components().count()) {
                    match component {
                        Component::Normal(name) => relative.push(name),
                        Component::ParentDir => {
                            relative.pop();
                        }
                        _ => {}
                    }
                }
                output.push(relative);
            } else {
                let mut relative = PathBuf::new();
                for component in item.source_dir.components() {
                    match component {
                        Component::Prefix(prefix) => {
                            use std::path::Prefix;
                            let name = match prefix.kind() {
                                Prefix::Disk(d) | Prefix::VerbatimDisk(d) => {
                                    (d as char).to_string()
                                }
                                Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                                    format!(
                                        "{}_{}",
                                        server.to_string_lossy(),
                                        share.to_string_lossy()
                                    )
                                }
                                _ => prefix.as_os_str().to_string_lossy().into_owned(),
                            };
                            output.push(crate::capture::basename_from_text(&name));
                        }
                        Component::Normal(name) => relative.push(name),
                        Component::ParentDir => {
                            relative.pop();
                        }
                        _ => {}
                    }
                }
                output.push(relative);
            }
            output
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct OutputKey(String);

impl OutputKey {
    fn from_path(path: &Path) -> Self {
        Self(path.to_string_lossy().to_lowercase())
    }
}

pub(super) enum Target {
    Write { path: PathBuf, replace: bool },
    Skip(PathBuf),
}

#[derive(Default)]
pub(super) struct OutputReservation {
    directories: HashMap<PathBuf, PathBuf>,
    claimed: HashSet<OutputKey>,
    protected: HashSet<OutputKey>,
}

impl OutputReservation {
    pub fn protect_sources(&mut self, items: &[BatchExportItem]) -> Result<(), String> {
        for item in items {
            let path = super::source_path(&item.source);
            let canonical = path.canonicalize().map_err(|error| {
                format!("元ファイルを確認できません: {}: {error}", path.display())
            })?;
            self.protected.insert(OutputKey::from_path(&canonical));
        }
        Ok(())
    }

    pub fn reserve(
        &mut self,
        directory: &Path,
        stem: &str,
        extension: &str,
        policy: OverwritePolicy,
    ) -> Result<Target, String> {
        let canonical = if let Some(path) = self.directories.get(directory) {
            path.clone()
        } else {
            std::fs::create_dir_all(directory).map_err(|e| {
                format!(
                    "保存先フォルダを作成できません: {}: {e}",
                    directory.display()
                )
            })?;
            let path = directory.canonicalize().map_err(|e| {
                format!(
                    "保存先フォルダを確認できません: {}: {e}",
                    directory.display()
                )
            })?;
            self.directories.insert(directory.to_owned(), path.clone());
            path
        };
        for seq in 0..=9999 {
            let filename = if seq == 0 {
                format!("{stem}.{extension}")
            } else {
                format!("{stem}_{seq}.{extension}")
            };
            let path = directory.join(&filename);
            let key = OutputKey::from_path(&canonical.join(filename));
            if self.claimed.contains(&key) {
                continue;
            }
            if !path.exists() {
                self.claimed.insert(key);
                return Ok(Target::Write {
                    path,
                    replace: false,
                });
            }
            let existing = path
                .canonicalize()
                .map_err(|e| format!("保存先を確認できません: {}: {e}", path.display()))?;
            let existing_key = OutputKey::from_path(&existing);
            if self.claimed.contains(&existing_key) {
                continue;
            }
            match policy {
                OverwritePolicy::Rename => continue,
                OverwritePolicy::Skip => {
                    self.claimed.insert(key);
                    return Ok(Target::Skip(path));
                }
                OverwritePolicy::Replace => {
                    if self.protected.contains(&existing_key) {
                        return Err("元ファイルは上書きしません".into());
                    }
                    self.claimed.insert(key);
                    return Ok(Target::Write {
                        path,
                        replace: true,
                    });
                }
            }
        }
        Err(format!("同名ファイルが多すぎます: {stem}.{extension}"))
    }

    pub fn record_written(&mut self, path: &Path) -> Result<(), String> {
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("出力したファイルを確認できません: {}: {e}", path.display()))?;
        self.claimed.insert(OutputKey::from_path(&canonical));
        Ok(())
    }
}
