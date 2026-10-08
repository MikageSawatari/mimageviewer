//! Shared launcher/core pointer contract. The launcher owns publication; readers
//! resolve a single immutable generation and retain that path for their lifetime.
use std::io;
use std::path::{Path, PathBuf};

pub const POINTER_FILE: &str = "current";
pub const PREPARATION_ERROR_ENV: &str = "MIV_EFFETUNE_PREPARATION_ERROR";
pub const GENERATION_ENV: &str = "MIV_EFFETUNE_GENERATION";
pub const REJECTED_GENERATION_ENV: &str = "MIV_EFFETUNE_REJECTED_GENERATION";
pub const FINGERPRINT_LENGTH: usize = 12;
pub const PATH_TOO_LONG_MARKER: &str = "EffeTune path too long";

pub fn checked_directory(path: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let reparse = metadata.file_type().is_symlink();
    if reparse || !metadata.is_dir() {
        return Err(io::Error::other(
            "EffeTune directory is invalid or a reparse point",
        ));
    }
    Ok(())
}

pub fn valid_generation(name: &str) -> bool {
    let Some((fingerprint, nonce)) = name.split_once('-') else {
        return false;
    };
    fingerprint.len() == FINGERPRINT_LENGTH
        && fingerprint
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        && (6..=32).contains(&nonce.len())
        && nonce.bytes().all(|b| b.is_ascii_alphanumeric())
}

pub fn encode_pointer(generation: &str) -> io::Result<String> {
    if !valid_generation(generation) {
        return Err(io::Error::other("invalid EffeTune generation"));
    }
    Ok(format!("effetune-v2\n{generation}\n"))
}

pub fn parse_pointer(text: &str) -> io::Result<&str> {
    let generation = text
        .strip_prefix("effetune-v2\n")
        .and_then(|text| text.strip_suffix('\n'))
        .filter(|name| valid_generation(name))
        .ok_or_else(|| io::Error::other("invalid EffeTune current pointer"))?;
    Ok(generation)
}

pub fn read_generation(container: &Path) -> io::Result<PathBuf> {
    let generation = container.join(read_pointer(container)?);
    checked_directory(&generation)?;
    Ok(generation)
}

pub fn read_pointer(container: &Path) -> io::Result<String> {
    checked_directory(container)?;
    let pointer = container.join(POINTER_FILE);
    // A pointer is a regular file; do not follow symlinks/reparse points.
    let metadata = std::fs::symlink_metadata(&pointer)?;
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let reparse = metadata.file_type().is_symlink();
    if reparse || !metadata.is_file() || metadata.len() > 160 {
        return Err(io::Error::other("invalid EffeTune pointer file"));
    }
    let text = std::fs::read_to_string(pointer)?;
    Ok(parse_pointer(&text)?.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_accepts_only_a_fingerprint_and_single_generation_basename() {
        let name = format!("{}-Abc123", "a".repeat(FINGERPRINT_LENGTH));
        let text = encode_pointer(&name).unwrap();
        assert_eq!(parse_pointer(&text).unwrap(), name);
        for bad in [
            "../outside",
            "C:/outside",
            "a/../b",
            "a\\b",
            "a:stream",
            "-Abc123",
        ] {
            assert!(encode_pointer(bad).is_err());
        }
        assert!(parse_pointer(&format!("{text}extra\n")).is_err());
    }
}
