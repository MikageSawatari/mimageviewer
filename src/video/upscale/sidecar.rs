use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SCHEMA_VERSION: u32 = 1;
pub const PARTIAL_HASH_CHUNK_BYTES: u64 = 1024 * 1024;
/// Output dimensions are capped at 8K UHD. Landscape and portrait 8K are accepted.
/// Square outputs are bounded by the short edge, matching the 16:9 8K cap.
pub const MAX_OUTPUT_LONG_EDGE: u32 = 7680;
pub const MAX_OUTPUT_SHORT_EDGE: u32 = 4320;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoUpscaleSidecar {
    pub schema: u32,
    pub source: SourceInfo,
    pub miv: MivInfo,
    pub upscale: UpscaleInfo,
    pub encode: EncodeInfo,
    pub output: OutputInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInfo {
    pub file_name: String,
    pub size: u64,
    /// Stored for diagnostics and stale-file forensics. Validation intentionally
    /// does not compare mtime because sync tools often rewrite it.
    pub mtime_unix_ms: u64,
    pub head_tail_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MivInfo {
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpscaleInfo {
    pub scale: u32,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncodeInfo {
    pub container: String,
    pub video_codec: String,
    pub encoder: String,
    pub quality_level: u8,
    pub crf: u8,
    pub preset: u8,
    pub pixel_format: String,
    pub audio: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputInfo {
    pub path: String,
    pub width: u32,
    pub height: u32,
}

impl VideoUpscaleSidecar {
    pub fn new(
        source: SourceInfo,
        upscale: UpscaleInfo,
        encode: EncodeInfo,
        output: OutputInfo,
    ) -> Self {
        Self {
            schema: SCHEMA_VERSION,
            source,
            miv: MivInfo {
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            upscale,
            encode,
            output,
        }
    }

    /// Validates that this sidecar matches the given source file.
    ///
    /// Compares file name, size, and a partial SHA-256. `mtime_unix_ms` is not
    /// compared because cloud sync and copy tools can rewrite mtime without
    /// changing content. The partial hash provides stable content identity
    /// without false negatives from benign mtime drift.
    pub fn is_valid_for_source(&self, source_path: &Path) -> io::Result<bool> {
        if self.schema != SCHEMA_VERSION {
            return Ok(false);
        }

        let current = source_info_for(source_path)?;
        Ok(self.source.file_name == current.file_name
            && self.source.size == current.size
            && self.source.head_tail_sha256 == current.head_tail_sha256)
    }
}

pub fn source_info_for(path: &Path) -> io::Result<SourceInfo> {
    let metadata = path.metadata()?;
    let modified = metadata.modified()?;
    let mtime_unix_ms = modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    Ok(SourceInfo {
        file_name,
        size: metadata.len(),
        mtime_unix_ms,
        head_tail_sha256: partial_hash_head_tail(path)?,
    })
}

pub fn partial_hash_head_tail(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut hasher = Sha256::new();

    if size <= PARTIAL_HASH_CHUNK_BYTES * 2 {
        let mut buf = Vec::with_capacity(size as usize);
        file.read_to_end(&mut buf)?;
        hasher.update(&buf);
    } else {
        let mut head = vec![0; PARTIAL_HASH_CHUNK_BYTES as usize];
        file.read_exact(&mut head)?;
        hasher.update(&head);

        file.seek(SeekFrom::End(-(PARTIAL_HASH_CHUNK_BYTES as i64)))?;
        let mut tail = vec![0; PARTIAL_HASH_CHUNK_BYTES as usize];
        file.read_exact(&mut tail)?;
        hasher.update(&tail);
    }

    Ok(hex_encode(&hasher.finalize()))
}

pub fn derived_video_path_for(source_path: &Path) -> PathBuf {
    derived_path_with_suffix(source_path, "miv.mkv")
}

pub fn derived_sidecar_path_for(source_path: &Path) -> PathBuf {
    derived_path_with_suffix(source_path, "miv.json")
}

pub fn output_within_mvp_limit(width: u32, height: u32) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    let long_edge = width.max(height);
    let short_edge = width.min(height);
    long_edge <= MAX_OUTPUT_LONG_EDGE && short_edge <= MAX_OUTPUT_SHORT_EDGE
}

fn derived_path_with_suffix(source_path: &Path, suffix: &str) -> PathBuf {
    let stem = source_path
        .file_stem()
        .or_else(|| source_path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "video".to_owned());
    let file_name = format!("{stem}.{suffix}");
    source_path.with_file_name(file_name)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_bytes(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn partial_hash_hashes_small_file_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.mp4");
        write_bytes(&path, b"small-video");

        let actual = partial_hash_head_tail(&path).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(b"small-video");
        let expected = hex_encode(&hasher.finalize());

        assert_eq!(actual, expected);
    }

    #[test]
    fn partial_hash_changes_when_tail_changes() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.mp4");
        let b = dir.path().join("b.mp4");
        let mut bytes = vec![7_u8; (PARTIAL_HASH_CHUNK_BYTES * 2 + 32) as usize];
        write_bytes(&a, &bytes);
        let last = bytes.len() - 1;
        bytes[last] = 8;
        write_bytes(&b, &bytes);

        assert_ne!(
            partial_hash_head_tail(&a).unwrap(),
            partial_hash_head_tail(&b).unwrap()
        );
    }

    #[test]
    fn validation_ignores_mtime_when_size_and_partial_hash_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("movie.mp4");
        write_bytes(&path, b"same-content");

        let mut source = source_info_for(&path).unwrap();
        source.mtime_unix_ms = source.mtime_unix_ms.saturating_add(60_000);
        let sidecar = VideoUpscaleSidecar {
            schema: SCHEMA_VERSION,
            source,
            miv: MivInfo {
                version: "test".to_owned(),
            },
            upscale: UpscaleInfo {
                scale: 2,
                model: "realesr_general_v3".to_owned(),
            },
            encode: EncodeInfo {
                container: "mkv".to_owned(),
                video_codec: "av1".to_owned(),
                encoder: "libsvtav1".to_owned(),
                quality_level: 3,
                crf: 28,
                preset: 8,
                pixel_format: "yuv420p".to_owned(),
                audio: "none".to_owned(),
            },
            output: OutputInfo {
                path: "movie.miv.mkv".to_owned(),
                width: 3840,
                height: 2160,
            },
        };

        assert!(sidecar.is_valid_for_source(&path).unwrap());
    }

    #[test]
    fn derived_paths_use_miv_suffixes() {
        let source = Path::new(r"C:\videos\movie.mp4");
        assert_eq!(
            derived_video_path_for(source),
            PathBuf::from(r"C:\videos\movie.miv.mkv")
        );
        assert_eq!(
            derived_sidecar_path_for(source),
            PathBuf::from(r"C:\videos\movie.miv.json")
        );
    }

    #[test]
    fn output_limit_allows_8k_and_blocks_larger_outputs() {
        assert!(output_within_mvp_limit(3840, 2160));
        assert!(output_within_mvp_limit(7680, 4320));
        assert!(output_within_mvp_limit(4320, 7680));
        assert!(!output_within_mvp_limit(7681, 4320));
        assert!(!output_within_mvp_limit(15360, 8640));
        assert!(!output_within_mvp_limit(0, 2160));
    }
}
