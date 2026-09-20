use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use super::{CollectionSourcePath, CollectionSourcePathKey};

pub const MAX_COLLECTION_IMPORT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_COLLECTION_IMPORT_NONEMPTY_LINES: usize = 50_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectionImportLimitError {
    Bytes,
    NonemptyLines,
}

impl fmt::Display for CollectionImportLimitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bytes => write!(f, "インポートテキストは32 MiB以下にしてください。"),
            Self::NonemptyLines => write!(
                f,
                "インポートテキストの空行以外は50,000行以下にしてください。"
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollectionImportLineStatus {
    Accepted,
    Duplicate { first_line: usize },
    Invalid { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectionImportLine {
    pub line_number: usize,
    pub original: String,
    pub resolved_path: Option<PathBuf>,
    pub source_key: Option<CollectionSourcePathKey>,
    pub status: CollectionImportLineStatus,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CollectionImportPreview {
    pub lines: Vec<CollectionImportLine>,
}

impl CollectionImportPreview {
    pub fn accepted_paths(&self) -> impl Iterator<Item = (&Path, &CollectionSourcePathKey)> + '_ {
        self.lines.iter().filter_map(|line| {
            if line.status != CollectionImportLineStatus::Accepted {
                return None;
            }
            Some((line.resolved_path.as_deref()?, line.source_key.as_ref()?))
        })
    }
}

/// 外部 text の純解析。ファイルの存在確認、link 解決、DB、network access は行わない。
pub fn parse_collection_text(
    text: &str,
    source_text_path: &Path,
) -> Result<CollectionImportPreview, CollectionImportLimitError> {
    if text.len() > MAX_COLLECTION_IMPORT_BYTES {
        return Err(CollectionImportLimitError::Bytes);
    }
    let mut lines = Vec::new();
    let mut first_by_key: HashMap<CollectionSourcePathKey, usize> = HashMap::new();
    let base_dir = source_text_path.parent().unwrap_or(source_text_path);
    let mut nonempty_lines = 0;

    for (index, raw_line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let line_number = index + 1;
        let original = raw_line.trim_end_matches('\r').to_owned();
        let trimmed = original.trim();
        if trimmed.is_empty() {
            continue;
        }
        nonempty_lines += 1;
        if nonempty_lines > MAX_COLLECTION_IMPORT_NONEMPTY_LINES {
            return Err(CollectionImportLimitError::NonemptyLines);
        }
        let value = match unquote_whole_line(trimmed) {
            Ok(value) => value,
            Err(reason) => {
                lines.push(CollectionImportLine {
                    line_number,
                    original,
                    resolved_path: None,
                    source_key: None,
                    status: CollectionImportLineStatus::Invalid { reason },
                });
                continue;
            }
        };
        match CollectionSourcePath::from_external_text(value, base_dir) {
            Ok(source) => {
                let key = source.key().clone();
                let status = if let Some(first_line) = first_by_key.get(&key) {
                    CollectionImportLineStatus::Duplicate {
                        first_line: *first_line,
                    }
                } else {
                    first_by_key.insert(key.clone(), line_number);
                    CollectionImportLineStatus::Accepted
                };
                lines.push(CollectionImportLine {
                    line_number,
                    original,
                    resolved_path: Some(source.path().to_path_buf()),
                    source_key: Some(key),
                    status,
                });
            }
            Err(error) => lines.push(CollectionImportLine {
                line_number,
                original,
                resolved_path: None,
                source_key: None,
                status: CollectionImportLineStatus::Invalid {
                    reason: error.to_string(),
                },
            }),
        }
    }
    Ok(CollectionImportPreview { lines })
}

/// 現在の有効順を UTF-8 text 本文へ直列化する純関数。
///
/// Windows の path に `"` は使えないため、空白を含む行だけ whole-line quote を付ける。
pub fn serialize_collection_paths<'a>(paths: impl IntoIterator<Item = &'a Path>) -> String {
    let mut output = String::new();
    for path in paths {
        let value = path.to_string_lossy();
        if value.chars().any(char::is_whitespace) {
            output.push('"');
            output.push_str(&value);
            output.push('"');
        } else {
            output.push_str(&value);
        }
        output.push_str("\r\n");
    }
    output
}

fn unquote_whole_line(value: &str) -> Result<&str, String> {
    if let Some(inner) = value.strip_prefix('"') {
        let Some(inner) = inner.strip_suffix('"') else {
            return Err("opening quote has no matching closing quote".into());
        };
        if inner.contains('"') {
            return Err("quotes are only accepted around the whole line".into());
        }
        if inner.is_empty() {
            return Err("quoted path is empty".into());
        }
        return Ok(inner);
    }
    if value.contains('"') {
        return Err("quotes are only accepted around the whole line".into());
    }
    Ok(value)
}
