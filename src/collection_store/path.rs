use std::path::{Path, PathBuf};

use super::{
    CollectionRegistration, CollectionResolvedKind, CollectionSourceMigration,
    CollectionSourceMigrationScope, CollectionSourcePathKey, CollectionStoreError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectionImportPathPolicy {
    /// 既存の列挙済み実体。合法な extended drive/UNC prefix は通常表記へ畳む。
    TrustedPhysicalSource,
    /// 外部 text。device/verbatim、URL、wildcard、展開式を admission で拒否する。
    ExternalText,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectionSourcePath {
    path: PathBuf,
    key: CollectionSourcePathKey,
}

impl CollectionSourcePath {
    pub fn from_trusted(path: impl AsRef<Path>) -> Result<Self, CollectionStoreError> {
        normalize_collection_path(
            path.as_ref(),
            CollectionImportPathPolicy::TrustedPhysicalSource,
        )
    }

    pub fn from_external_text(value: &str, base_dir: &Path) -> Result<Self, CollectionStoreError> {
        validate_external_text_path(value)?;
        let absolute = if is_windows_absolute(value) {
            PathBuf::from(value)
        } else {
            if is_windows_rooted_without_drive(value) || looks_drive_relative(value) {
                return Err(CollectionStoreError::InvalidPath(
                    "drive-relative or root-relative paths are ambiguous".into(),
                ));
            }
            let normalized_base = normalize_collection_path(
                base_dir,
                CollectionImportPathPolicy::TrustedPhysicalSource,
            )?;
            normalized_base.path.join(value)
        };
        normalize_collection_path(&absolute, CollectionImportPathPolicy::ExternalText)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn key(&self) -> &CollectionSourcePathKey {
        &self.key
    }
}

impl CollectionRegistration {
    pub fn from_trusted_path(
        path: impl AsRef<Path>,
        resolved_kind: CollectionResolvedKind,
    ) -> Result<Self, CollectionStoreError> {
        let source = CollectionSourcePath::from_trusted(path)?;
        Ok(Self {
            source_path: source.path,
            source_key: source.key,
            resolved_kind,
        })
    }

    pub fn from_source_path(
        source: CollectionSourcePath,
        resolved_kind: CollectionResolvedKind,
    ) -> Self {
        Self {
            source_path: source.path,
            source_key: source.key,
            resolved_kind,
        }
    }
}

impl CollectionSourceMigration {
    pub fn from_trusted_paths(
        old_path: impl AsRef<Path>,
        new_path: impl AsRef<Path>,
        scope: CollectionSourceMigrationScope,
    ) -> Result<Self, CollectionStoreError> {
        let old = CollectionSourcePath::from_trusted(old_path)?;
        let new = CollectionSourcePath::from_trusted(new_path)?;
        Ok(Self {
            old_path: old.path,
            old_key: old.key,
            new_path: new.path,
            new_key: new.key,
            scope,
        })
    }

    pub(crate) fn replacement_for(
        &self,
        source_path: &Path,
        source_key: &CollectionSourcePathKey,
    ) -> Result<Option<CollectionSourcePath>, CollectionStoreError> {
        if source_key.namespace() != self.old_key.namespace() {
            return Ok(None);
        }
        if source_key == &self.old_key {
            return Ok(Some(CollectionSourcePath {
                path: self.new_path.clone(),
                key: self.new_key.clone(),
            }));
        }
        if self.scope != CollectionSourceMigrationScope::Tree {
            return Ok(None);
        }
        let old_prefix = format!("{}/", self.old_key.normalized_path().trim_end_matches('/'));
        if !source_key.normalized_path().starts_with(&old_prefix) {
            return Ok(None);
        }

        let Some(suffix) = component_suffix(source_path, &self.old_path) else {
            return Err(CollectionStoreError::InvalidPath(
                "stored path and source key disagree during tree migration".into(),
            ));
        };
        let mut replacement = self.new_path.clone();
        for component in suffix {
            replacement.push(component);
        }
        CollectionSourcePath::from_trusted(replacement).map(Some)
    }
}

impl super::CollectionSourceMigrationBatch {
    pub fn from_trusted_paths<I, O, N>(mappings: I) -> Result<Self, CollectionStoreError>
    where
        I: IntoIterator<Item = (O, N, CollectionSourceMigrationScope)>,
        O: AsRef<Path>,
        N: AsRef<Path>,
    {
        let migrations = mappings
            .into_iter()
            .map(|(old, new, scope)| CollectionSourceMigration::from_trusted_paths(old, new, scope))
            .collect::<Result<Vec<_>, _>>()?;
        if migrations.is_empty() {
            return Err(CollectionStoreError::InvalidPath(
                "collection source migration batch is empty".into(),
            ));
        }
        Ok(Self { migrations })
    }

    pub fn len(&self) -> usize {
        self.migrations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.migrations.is_empty()
    }
}

fn normalize_collection_path(
    path: &Path,
    policy: CollectionImportPathPolicy,
) -> Result<CollectionSourcePath, CollectionStoreError> {
    let raw = path.to_string_lossy();
    if raw.is_empty() {
        return Err(CollectionStoreError::InvalidPath("empty path".into()));
    }
    let mut slash = raw.replace('\\', "/");
    if policy == CollectionImportPathPolicy::ExternalText && is_extended_or_device(&slash) {
        return Err(CollectionStoreError::InvalidPath(
            "device and verbatim paths are not accepted from text import".into(),
        ));
    }
    slash = trusted_extended_to_ordinary(&slash)?;

    let (root, root_key, rest) = if is_drive_absolute(&slash) {
        let drive = &slash[..2];
        (
            format!("{}\\", drive),
            drive.to_ascii_lowercase(),
            slash[3..].to_owned(),
        )
    } else if slash.starts_with("//") {
        let parts: Vec<_> = slash[2..]
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        let server = parts
            .first()
            .copied()
            .ok_or_else(|| CollectionStoreError::InvalidPath("UNC server is missing".into()))?;
        let share = parts
            .get(1)
            .copied()
            .ok_or_else(|| CollectionStoreError::InvalidPath("UNC share is missing".into()))?;
        if matches!(server, "." | "..") || matches!(share, "." | "..") {
            return Err(CollectionStoreError::InvalidPath(
                "UNC server/share is invalid".into(),
            ));
        }
        (
            format!(r"\\{}\{}", server, share),
            format!("//{}/{}", server.to_lowercase(), share.to_lowercase()),
            parts[2..].join("/"),
        )
    } else {
        return Err(CollectionStoreError::InvalidPath(
            "an absolute drive or UNC path is required".into(),
        ));
    };

    let mut components: Vec<&str> = Vec::new();
    for component in rest.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.is_empty() {
                    return Err(CollectionStoreError::InvalidPath(
                        "path traverses above its drive/share root".into(),
                    ));
                }
                components.pop();
            }
            value => components.push(value),
        }
    }

    let display = if components.is_empty() {
        root
    } else if root.ends_with('\\') {
        format!("{}{}", root, components.join("\\"))
    } else {
        format!("{}\\{}", root, components.join("\\"))
    };
    let normalized_path = if components.is_empty() {
        if root_key.starts_with("//") {
            root_key
        } else {
            format!("{root_key}/")
        }
    } else {
        format!(
            "{}/{}",
            root_key.trim_end_matches('/'),
            components.join("/")
        )
        .to_lowercase()
    };
    Ok(CollectionSourcePath {
        path: PathBuf::from(display),
        key: CollectionSourcePathKey::new_filesystem(normalized_path),
    })
}

fn validate_external_text_path(value: &str) -> Result<(), CollectionStoreError> {
    if value.is_empty() {
        return Err(CollectionStoreError::InvalidPath("empty path".into()));
    }
    let slash = value.replace('\\', "/");
    let lower = slash.to_ascii_lowercase();
    if is_extended_or_device(&slash) {
        return Err(CollectionStoreError::InvalidPath(
            "device and verbatim paths are not accepted from text import".into(),
        ));
    }
    if lower.contains("://") || lower.starts_with("file:") {
        return Err(CollectionStoreError::InvalidPath(
            "URL is not a file path".into(),
        ));
    }
    if value.contains('*') || value.contains('?') {
        return Err(CollectionStoreError::InvalidPath(
            "wildcards are not expanded".into(),
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(CollectionStoreError::InvalidPath(
            "control characters are not accepted".into(),
        ));
    }
    for (index, byte) in value.bytes().enumerate() {
        if byte == b':' && !(index == 1 && value.as_bytes()[0].is_ascii_alphabetic()) {
            return Err(CollectionStoreError::InvalidPath(
                "alternate data streams are not accepted".into(),
            ));
        }
    }
    if slash
        .split('/')
        .filter(|component| !component.is_empty())
        .any(is_dos_device_component)
    {
        return Err(CollectionStoreError::InvalidPath(
            "DOS device names are not accepted".into(),
        ));
    }
    let bytes = value.as_bytes();
    if value.contains("${")
        || value.contains("$(")
        || value.starts_with('$')
        || bytes.iter().filter(|byte| **byte == b'%').count() >= 2
    {
        return Err(CollectionStoreError::InvalidPath(
            "environment and command expressions are not expanded".into(),
        ));
    }
    Ok(())
}

fn is_dos_device_component(component: &str) -> bool {
    let trimmed = component.trim_end_matches([' ', '.']);
    let stem = trimmed.split('.').next().unwrap_or_default();
    let upper = stem.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) || upper
        .strip_prefix("COM")
        .is_some_and(|suffix| matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
        || upper.strip_prefix("LPT").is_some_and(|suffix| {
            matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
        })
}

fn trusted_extended_to_ordinary(value: &str) -> Result<String, CollectionStoreError> {
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("//./") {
        return Err(CollectionStoreError::InvalidPath(
            "device namespace is not a filesystem source".into(),
        ));
    }
    if lower.starts_with("//?/unc/") {
        return Ok(format!("//{}", &value[8..]));
    }
    if lower.starts_with("//?/") {
        let remainder = &value[4..];
        if is_drive_absolute(remainder) {
            return Ok(remainder.to_owned());
        }
        return Err(CollectionStoreError::InvalidPath(
            "unsupported verbatim source".into(),
        ));
    }
    if lower.starts_with("/??/unc/") {
        return Ok(format!("//{}", &value[8..]));
    }
    if lower.starts_with("/??/") {
        let remainder = &value[4..];
        if is_drive_absolute(remainder) {
            return Ok(remainder.to_owned());
        }
        return Err(CollectionStoreError::InvalidPath(
            "unsupported native source".into(),
        ));
    }
    Ok(value.to_owned())
}

fn is_extended_or_device(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.starts_with("//?/") || lower.starts_with("//./") || lower.starts_with("/??/")
}

fn is_drive_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}

fn is_windows_absolute(value: &str) -> bool {
    let slash = value.replace('\\', "/");
    is_drive_absolute(&slash) || slash.starts_with("//")
}

fn is_windows_rooted_without_drive(value: &str) -> bool {
    (value.starts_with('\\') || value.starts_with('/')) && !is_windows_absolute(value)
}

fn looks_drive_relative(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || !matches!(bytes[2], b'\\' | b'/'))
}

fn component_suffix(path: &Path, prefix: &Path) -> Option<Vec<String>> {
    let path = CollectionSourcePath::from_trusted(path).ok()?;
    let prefix = CollectionSourcePath::from_trusted(prefix).ok()?;
    let path_parts = ordinary_components(path.path());
    let prefix_parts = ordinary_components(prefix.path());
    if prefix_parts.len() > path_parts.len()
        || !path_parts
            .iter()
            .zip(&prefix_parts)
            .all(|(left, right)| left.to_lowercase() == right.to_lowercase())
    {
        return None;
    }
    Some(path_parts[prefix_parts.len()..].to_vec())
}

fn ordinary_components(path: &Path) -> Vec<String> {
    path.to_string_lossy()
        .replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}
