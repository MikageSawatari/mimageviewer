//! Clipboard classification, bounded decoding and file publication.
//! These functions perform no clipboard I/O; parsing and saving run on workers.

use sha2::{Digest, Sha256};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

pub(crate) const ORIGIN_FORMAT_NAME: &str = "mImageViewer Clipboard Origin v1";
pub(crate) const MAX_IMAGE_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const MAX_HTML_BYTES: usize = 32 * 1024 * 1024;
const MAX_RGBA_BYTES: u64 = 1024 * 1024 * 1024;
const PART_PREFIX: &str = ".miv-clipboard-";

pub(crate) fn process_nonce() -> &'static [u8] {
    static NONCE: std::sync::OnceLock<[u8; 16]> = std::sync::OnceLock::new();
    NONCE.get_or_init(|| *uuid::Uuid::new_v4().as_bytes())
}

pub(crate) fn marker_is_ours(bytes: &[u8]) -> bool {
    // HGLOBAL may be rounded up, but its first 16 bytes are the private nonce.
    bytes.get(..16) == Some(process_nonce())
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ClipboardFormats {
    pub names: Vec<String>,
    pub history_allowed: Option<bool>,
    pub own_marker: bool,
}

impl ClipboardFormats {
    pub fn has(&self, name: &str) -> bool {
        self.names.iter().any(|value| value == name)
    }

    fn has_files(&self) -> bool {
        ["CF_HDROP", "FileGroupDescriptorW", "Shell IDList Array"]
            .iter()
            .any(|name| self.has(name))
    }

    fn has_image(&self) -> bool {
        ["PNG", "CF_DIBV5", "CF_DIB"]
            .iter()
            .any(|name| self.has(name))
    }

    fn has_office(&self) -> bool {
        [
            "Embed Source",
            "Object Descriptor",
            "XML Spreadsheet",
            "Art::GVML ClipFormat",
        ]
        .iter()
        .any(|name| self.has(name))
            || self
                .names
                .iter()
                .any(|name| name.starts_with("PowerPoint "))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipboardKind {
    Ignored,
    Files,
    Image,
    Html,
    Other,
}

pub(crate) fn classify_automatic(
    formats: &ClipboardFormats,
    source_url: Option<&str>,
) -> ClipboardKind {
    if formats.has("ExcludeClipboardContentFromMonitorProcessing")
        || formats.has("Clipboard Viewer Ignore")
        || formats.history_allowed == Some(false)
        || formats.own_marker
    {
        return ClipboardKind::Ignored;
    }
    if formats.has_files() {
        return ClipboardKind::Files;
    }
    if formats.has_image() {
        return if formats.has_office() {
            ClipboardKind::Ignored
        } else {
            ClipboardKind::Image
        };
    }
    if formats.has("HTML Format") && source_url.and_then(sanitize_url).is_some() {
        return ClipboardKind::Html;
    }
    ClipboardKind::Other
}

pub(crate) fn classify_manual(formats: &ClipboardFormats) -> ClipboardKind {
    if formats.has_files() {
        ClipboardKind::Files
    } else if formats.has_image() {
        ClipboardKind::Image
    } else if formats.has("HTML Format") {
        ClipboardKind::Html
    } else {
        ClipboardKind::Other
    }
}

#[derive(Debug, Default)]
pub(crate) struct RawClipboardData {
    pub png: Option<Vec<u8>>,
    pub dib_v5: Option<Vec<u8>>,
    pub dib: Option<Vec<u8>>,
    pub html: Option<Vec<u8>>,
    pub uri_w: Option<Vec<u8>>,
    pub chromium_source: Option<Vec<u8>>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct CaptureOrigin {
    pub page_url: Option<String>,
    pub image_url: Option<String>,
}

impl CaptureOrigin {
    pub fn domain(&self) -> Option<String> {
        self.page_url
            .as_deref()
            .or(self.image_url.as_deref())
            .and_then(|value| url::Url::parse(value).ok())
            .and_then(|value| value.host_str().map(str::to_owned))
    }
}

/// Only Chromium's image URL format establishes image-copy provenance. We do
/// not infer Firefox provenance from its HTML fragment (plan §5.8).
pub(crate) fn image_origin(raw: &RawClipboardData) -> CaptureOrigin {
    if raw.uri_w.is_none() && raw.chromium_source.is_none() {
        return CaptureOrigin::default();
    }
    let image_url = raw.uri_w.as_deref().and_then(|bytes| {
        if bytes.len() % 2 != 0 {
            return None;
        }
        let units: Vec<_> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .take_while(|unit| *unit != 0)
            .collect();
        String::from_utf16(&units)
            .ok()
            .and_then(|value| sanitize_url(&value))
    });
    // Chromium writes CreateGlobalData(URL.spec()), a narrow, NUL-terminated
    // string; see ui/base/clipboard/clipboard_win.cc GetSource / WritePortable….
    let page_url = raw.chromium_source.as_deref().and_then(|bytes| {
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        std::str::from_utf8(&bytes[..end])
            .ok()
            .and_then(sanitize_url)
    });
    CaptureOrigin {
        page_url,
        image_url,
    }
}

pub(crate) fn sanitize_url(value: &str) -> Option<String> {
    // Reject literal control characters instead of allowing URL parsing to
    // erase line breaks supplied for a Zone.Identifier injection.
    if value.chars().any(char::is_control) {
        return None;
    }
    let mut parsed = url::Url::parse(value).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return None;
    }
    parsed.set_username("").ok()?;
    parsed.set_password(None).ok()?;
    parsed.set_fragment(None);
    Some(parsed.to_string())
}

pub(crate) fn html_source_url(bytes: &[u8]) -> Option<String> {
    if bytes.len() > MAX_HTML_BYTES {
        return None;
    }
    // SourceURL is a CF_HTML header field, never a string found in markup.
    for line in bytes.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.first() == Some(&b'<') {
            break;
        }
        if let Some(value) = line.strip_prefix(b"SourceURL:") {
            return std::str::from_utf8(value).ok().and_then(sanitize_url);
        }
    }
    None
}

pub(crate) fn validate_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("クリップボード画像のサイズが不正です".into());
    }
    if width > 32768 || height > 32768 || u64::from(width) * u64::from(height) * 4 > MAX_RGBA_BYTES
    {
        return Err("クリップボード画像が大きすぎます".into());
    }
    Ok(())
}

fn png_dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    if bytes.len() < 33
        || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
        || &bytes[12..16] != b"IHDR"
        || bytes[8..12] != 13u32.to_be_bytes()
    {
        return Err("クリップボードの PNG ヘッダーが不正です".into());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    validate_dimensions(width, height)?;
    Ok((width, height))
}

pub(crate) fn decode_capture_dib(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("クリップボード画像が大きすぎます".into());
    }
    let header = bytes
        .get(..12)
        .ok_or("クリップボード画像のヘッダーが不正です")?;
    let size = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
    let width = i32::from_le_bytes(header[4..8].try_into().unwrap());
    let height = i32::from_le_bytes(header[8..12].try_into().unwrap());
    if size < 40 || bytes.len() < size || width <= 0 || height == 0 || height == i32::MIN {
        return Err("クリップボード画像のヘッダーが不正です".into());
    }
    validate_dimensions(width as u32, height.unsigned_abs())?;
    crate::books::decode_cf_dib_rgba(bytes)
}

#[derive(Debug)]
pub(crate) struct CapturedImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub content_hash: [u8; 32],
    pub origin: CaptureOrigin,
}

pub(crate) fn decode_image(raw: &RawClipboardData) -> Result<CapturedImage, String> {
    // Selection is deterministic. A malformed or over-budget preferred format
    // is rejected; another decoder cannot bypass the entrance budget.
    let (png, width, height, rgba) = if let Some(bytes) = &raw.png {
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err("クリップボード画像が大きすぎます".into());
        }
        let (width, height) = png_dimensions(bytes)?;
        let mut reader =
            image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(32768);
        limits.max_image_height = Some(32768);
        limits.max_alloc = Some(MAX_RGBA_BYTES);
        reader.limits(limits);
        let rgba = reader
            .decode()
            .map_err(|error| format!("PNG を読み取れませんでした: {error}"))?
            .into_rgba8()
            .into_raw();
        (bytes.clone(), width, height, rgba)
    } else {
        let bytes = raw
            .dib_v5
            .as_deref()
            .or(raw.dib.as_deref())
            .ok_or("クリップボードに画像がありません")?;
        let (width, height, rgba) = decode_capture_dib(bytes)?;
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .and_then(|mut writer| writer.write_image_data(&rgba))
                .map_err(|error| format!("PNG を作成できませんでした: {error}"))?;
        }
        (png, width, height, rgba)
    };
    let mut hash = Sha256::new();
    hash.update(width.to_le_bytes());
    hash.update(height.to_le_bytes());
    hash.update(&rgba);
    Ok(CapturedImage {
        png,
        width,
        height,
        content_hash: hash.finalize().into(),
        origin: image_origin(raw),
    })
}

#[derive(Debug, Clone)]
pub(crate) struct CaptureTimestamp {
    pub stamp: String,
    pub month: String,
}

impl CaptureTimestamp {
    pub fn now() -> Self {
        #[cfg(windows)]
        {
            let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
            Self {
                stamp: format!(
                    "{:04}{:02}{:02}-{:02}{:02}{:02}-{:03}",
                    time.wYear,
                    time.wMonth,
                    time.wDay,
                    time.wHour,
                    time.wMinute,
                    time.wSecond,
                    time.wMilliseconds
                ),
                month: format!("{:04}-{:02}", time.wYear, time.wMonth),
            }
        }
        #[cfg(not(windows))]
        {
            // Production is Windows-only; deterministic UTC fallback for other
            // targets uses the UNIX date without a platform time dependency.
            let elapsed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            let days = elapsed.as_secs() / 86400;
            let z = days as i64 + 719468;
            let era = z / 146097;
            let doe = z - era * 146097;
            let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
            let mp = (5 * doy + 2) / 153;
            let day = doy - (153 * mp + 2) / 5 + 1;
            let month = mp + if mp < 10 { 3 } else { -9 };
            let year = yoe + era * 400 + i64::from(month <= 2);
            let seconds = elapsed.as_secs() % 86400;
            Self {
                stamp: format!(
                    "{year:04}{month:02}{day:02}-{:02}{:02}{:02}-{:03}",
                    seconds / 3600,
                    seconds / 60 % 60,
                    seconds % 60,
                    elapsed.subsec_millis()
                ),
                month: format!("{year:04}-{month:02}"),
            }
        }
    }
}

pub(crate) fn capture_filename(
    timestamp: &CaptureTimestamp,
    domain: Option<&str>,
    index: usize,
    total: usize,
    collision: usize,
    extension: &str,
) -> String {
    let suffix = if collision > 1 {
        format!("-{collision}")
    } else {
        String::new()
    };
    let domain = domain
        .map(|value| {
            value
                .chars()
                .map(|c| {
                    if c == '_' || c.is_control() || "<>:\"/\\|?*".contains(c) {
                        '-'
                    } else {
                        c
                    }
                })
                .collect::<String>()
        })
        .filter(|value| !value.is_empty())
        .map(|value| format!("-{value}"))
        .unwrap_or_default();
    let digits = total.max(1).to_string().len().max(2);
    format!(
        "{}{suffix}{domain}_{index:0digits$}.{extension}",
        timestamp.stamp
    )
}

pub(crate) fn motw_contents(origin: &CaptureOrigin) -> Option<String> {
    let page = origin.page_url.as_deref().and_then(sanitize_url);
    let image = origin.image_url.as_deref().and_then(sanitize_url);
    if page.is_none() && image.is_none() {
        return None;
    }
    let mut result = "[ZoneTransfer]\r\nZoneId=3\r\n".to_owned();
    if let Some(value) = page {
        result.push_str(&format!("ReferrerUrl={value}\r\n"));
    }
    if let Some(value) = image {
        result.push_str(&format!("HostUrl={value}\r\n"));
    }
    Some(result)
}

#[derive(Debug)]
pub(crate) struct SavedImage {
    pub path: PathBuf,
    pub metadata_error: Option<String>,
}

pub(crate) fn save_image(
    image: &CapturedImage,
    destination: &Path,
    timestamp: &CaptureTimestamp,
) -> Result<SavedImage, String> {
    let directory = destination.join(&timestamp.month);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("保存先を作成できませんでした: {error}"))?;
    let domain = image.origin.domain();
    let mut part = tempfile::Builder::new()
        .prefix(PART_PREFIX)
        .suffix(".part")
        .tempfile_in(&directory)
        .map_err(|error| format!("一時ファイルを作成できませんでした: {error}"))?;
    part.write_all(&image.png)
        .and_then(|()| part.flush())
        .map_err(|error| format!("画像を保存できませんでした: {error}"))?;
    // persist_noclobber publishes by rename on Windows without replacing an
    // existing file. The same bytes remain owned by the temporary file on a
    // collision; only the timestamp prefix changes.
    let mut collision = 1;
    let path = loop {
        let path = directory.join(capture_filename(
            timestamp,
            domain.as_deref(),
            1,
            1,
            collision,
            "png",
        ));
        match part.persist_noclobber(&path) {
            Ok(_) => break path,
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                part = error.file;
                collision += 1;
            }
            Err(error) => return Err(format!("画像を確定できませんでした: {}", error.error)),
        }
    };
    let metadata_error = write_motw(&path, &image.origin).err();
    Ok(SavedImage {
        path,
        metadata_error,
    })
}

fn write_motw(path: &Path, origin: &CaptureOrigin) -> Result<(), String> {
    #[cfg(windows)]
    if let Some(contents) = motw_contents(origin) {
        let mut ads = path.as_os_str().to_os_string();
        ads.push(":Zone.Identifier");
        std::fs::write(PathBuf::from(ads), contents)
            .map_err(|error| format!("出どころを記録できませんでした: {error}"))?;
    }
    #[cfg(not(windows))]
    let _ = (path, origin);
    Ok(())
}

/// Startup worker cleanup is intentionally shallow and does not follow links
/// or Windows reparse points. It removes only this module's owned .part names.
pub(crate) fn cleanup_part_files(destination: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(destination) {
        Ok(metadata) if is_reparse(&metadata) => {
            return Err("保存先のリンクをたどって一時ファイルを削除することはできません".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    }
    let months = match std::fs::read_dir(destination) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    for month in months {
        let month = month.map_err(|error| error.to_string())?;
        let name = month.file_name();
        let name = name.to_string_lossy();
        if name.len() != 7
            || name.as_bytes()[4] != b'-'
            || !name
                .bytes()
                .enumerate()
                .all(|(index, byte)| index == 4 || byte.is_ascii_digit())
        {
            continue;
        }
        let metadata =
            std::fs::symlink_metadata(month.path()).map_err(|error| error.to_string())?;
        if !metadata.is_dir() || is_reparse(&metadata) {
            continue;
        }
        for entry in std::fs::read_dir(month.path()).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with(PART_PREFIX) || !name.ends_with(".part") {
                continue;
            }
            let metadata =
                std::fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
            if metadata.is_file() && !is_reparse(&metadata) {
                std::fs::remove_file(entry.path()).map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(())
}

fn is_reparse(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formats(names: &[&str]) -> ClipboardFormats {
        ClipboardFormats {
            names: names.iter().map(|name| (*name).to_owned()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn s0_format_fixtures_obey_automatic_and_manual_routes() {
        let fixtures: &[(&str, &[&str], Option<&str>, ClipboardKind, ClipboardKind)] = &[
            (
                "Chrome image 173723",
                &[
                    "PNG",
                    "CF_DIBV5",
                    "HTML Format",
                    "UniformResourceLocatorW",
                    "Chromium internal source URL",
                    "CF_DIB",
                ],
                None,
                ClipboardKind::Image,
                ClipboardKind::Image,
            ),
            (
                "Chrome page 173736",
                &["HTML Format", "CF_UNICODETEXT"],
                Some("https://example.com/artworks/1#1"),
                ClipboardKind::Html,
                ClipboardKind::Html,
            ),
            (
                "Excel 173803",
                &[
                    "CF_DIB",
                    "CF_DIBV5",
                    "Embed Source",
                    "Object Descriptor",
                    "XML Spreadsheet",
                    "Csv",
                    "Rich Text Format",
                    "HTML Format",
                    "CF_UNICODETEXT",
                ],
                Some("file:///sheet"),
                ClipboardKind::Ignored,
                ClipboardKind::Image,
            ),
            (
                "Word 173826",
                &[
                    "Rich Text Format",
                    "HTML Format",
                    "CF_UNICODETEXT",
                    "CF_ENHMETAFILE",
                    "Embed Source",
                    "Object Descriptor",
                ],
                None,
                ClipboardKind::Other,
                ClipboardKind::Html,
            ),
            (
                "PowerPoint 173844",
                &[
                    "PNG",
                    "JFIF",
                    "GIF",
                    "CF_DIB",
                    "CF_DIBV5",
                    "image/svg+xml",
                    "PowerPoint 12.0 Internal Shapes",
                ],
                None,
                ClipboardKind::Ignored,
                ClipboardKind::Image,
            ),
            (
                "Explorer 173857",
                &["CF_HDROP", "Shell IDList Array", "FileGroupDescriptorW"],
                None,
                ClipboardKind::Files,
                ClipboardKind::Files,
            ),
            (
                "mIV files 173911",
                &["CF_HDROP", "Shell IDList Array", "FileGroupDescriptorW"],
                None,
                ClipboardKind::Files,
                ClipboardKind::Files,
            ),
            (
                "mIV image 173922 before marker",
                &["CF_DIB", "CF_DIBV5", "CF_BITMAP"],
                None,
                ClipboardKind::Image,
                ClipboardKind::Image,
            ),
            (
                "Snipping Tool 180739",
                &[
                    "PNG",
                    "CF_DIB",
                    "CF_DIBV5",
                    "CanIncludeInClipboardHistory",
                    "CanUploadToCloudClipboard",
                ],
                None,
                ClipboardKind::Image,
                ClipboardKind::Image,
            ),
            (
                "Edge image 180801",
                &[
                    "PNG",
                    "CF_DIBV5",
                    "HTML Format",
                    "UniformResourceLocatorW",
                    "Chromium internal source URL",
                    "CF_DIB",
                ],
                None,
                ClipboardKind::Image,
                ClipboardKind::Image,
            ),
            (
                "Firefox image 180822",
                &[
                    "PNG",
                    "CF_DIBV5",
                    "CF_DIB",
                    "HTML Format",
                    "text/html",
                    "text/_moz_htmlcontext",
                ],
                None,
                ClipboardKind::Image,
                ClipboardKind::Image,
            ),
        ];
        for (label, names, source, automatic, manual) in fixtures {
            let formats = formats(names);
            assert_eq!(classify_automatic(&formats, *source), *automatic, "{label}");
            assert_eq!(classify_manual(&formats), *manual, "{label}");
        }
    }

    #[test]
    fn exclusions_precede_files_and_images_and_manual_ignores_exclusions() {
        for excluded in [
            "ExcludeClipboardContentFromMonitorProcessing",
            "Clipboard Viewer Ignore",
        ] {
            let formats = formats(&[excluded, "PNG", "CF_HDROP"]);
            assert_eq!(classify_automatic(&formats, None), ClipboardKind::Ignored);
            assert_eq!(classify_manual(&formats), ClipboardKind::Files);
        }
        let mut formats = formats(&["PNG", "HTML Format", "CF_UNICODETEXT"]);
        formats.history_allowed = Some(false);
        assert_eq!(
            classify_automatic(&formats, Some("https://example.com")),
            ClipboardKind::Ignored
        );
        assert_eq!(classify_manual(&formats), ClipboardKind::Image);
        formats.history_allowed = Some(true);
        assert_eq!(classify_automatic(&formats, None), ClipboardKind::Image);
        formats.own_marker = true;
        assert_eq!(classify_automatic(&formats, None), ClipboardKind::Ignored);
        assert_eq!(classify_manual(&formats), ClipboardKind::Image);
        assert!(marker_is_ours(process_nonce()));
        let mut padded = process_nonce().to_vec();
        padded.resize(24, 0);
        assert!(marker_is_ours(&padded));
        let mut foreign = process_nonce().to_vec();
        foreign[0] ^= 1;
        assert!(!marker_is_ours(&foreign));
        assert!(!marker_is_ours(&foreign[..15]));
    }

    #[test]
    fn all_office_formats_exclude_image_without_falling_through_to_html() {
        for name in [
            "Embed Source",
            "Object Descriptor",
            "XML Spreadsheet",
            "Art::GVML ClipFormat",
            "PowerPoint 12.0 Internal Shapes",
        ] {
            assert_eq!(
                classify_automatic(
                    &formats(&[name, "PNG", "HTML Format"]),
                    Some("https://example.com")
                ),
                ClipboardKind::Ignored
            );
        }
        for file in ["CF_HDROP", "FileGroupDescriptorW", "Shell IDList Array"] {
            assert_eq!(
                classify_automatic(&formats(&[file, "PNG"]), None),
                ClipboardKind::Files
            );
        }
    }

    fn dib(width: i32, alpha: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; 40 + alpha.len() * 4];
        bytes[0..4].copy_from_slice(&40u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&width.to_le_bytes());
        bytes[8..12].copy_from_slice(&(-1i32).to_le_bytes());
        bytes[12..14].copy_from_slice(&1u16.to_le_bytes());
        bytes[14..16].copy_from_slice(&32u16.to_le_bytes());
        for (index, alpha) in alpha.iter().enumerate() {
            bytes[40 + index * 4..44 + index * 4].copy_from_slice(&[3, 2, 1, *alpha]);
        }
        bytes
    }

    fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(rgba)
                .unwrap();
        }
        bytes
    }

    #[test]
    fn decoding_precedence_preserves_png_transparency_and_dib_rules() {
        let transparent = png(1, 1, &[1, 2, 3, 0]);
        let raw = RawClipboardData {
            png: Some(transparent.clone()),
            dib_v5: Some(dib(1, &[255])),
            ..Default::default()
        };
        let image = decode_image(&raw).unwrap();
        assert_eq!(image.png, transparent);
        assert_eq!(
            image::load_from_memory(&image.png)
                .unwrap()
                .into_rgba8()
                .as_raw(),
            &[1, 2, 3, 0]
        );
        let partial = RawClipboardData {
            dib_v5: Some(dib(2, &[0, 128])),
            dib: Some(dib(1, &[255])),
            ..Default::default()
        };
        let image = decode_image(&partial).unwrap();
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(
            image::load_from_memory(&image.png)
                .unwrap()
                .into_rgba8()
                .as_raw(),
            &[1, 2, 3, 0, 1, 2, 3, 128]
        );
        let undefined = decode_image(&RawClipboardData {
            dib: Some(dib(1, &[0])),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            image::load_from_memory(&undefined.png)
                .unwrap()
                .into_rgba8()
                .as_raw(),
            &[1, 2, 3, 255]
        );
    }

    #[test]
    fn png_and_dib_hashes_identify_the_same_decoded_content() {
        let a = decode_image(&RawClipboardData {
            png: Some(png(1, 1, &[1, 2, 3, 128])),
            ..Default::default()
        })
        .unwrap();
        let b = decode_image(&RawClipboardData {
            dib: Some(dib(1, &[128])),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(a.content_hash, b.content_hash);
        let different = decode_image(&RawClipboardData {
            png: Some(png(1, 1, &[1, 2, 4, 128])),
            ..Default::default()
        })
        .unwrap();
        assert_ne!(a.content_hash, different.content_hash);
    }

    #[test]
    fn oversized_headers_are_rejected_before_pixel_data_or_fallback() {
        let mut giant_png = png(1, 1, &[1, 2, 3, 255]);
        giant_png[16..20].copy_from_slice(&40000u32.to_be_bytes());
        let raw = RawClipboardData {
            png: Some(giant_png),
            dib: Some(dib(1, &[255])),
            ..Default::default()
        };
        assert!(decode_image(&raw).unwrap_err().contains("大きすぎ"));
        // No pixel bytes are present: the size budget must win before the
        // common decoder's missing-pixel-data validation or allocation.
        assert!(
            decode_capture_dib(&dib(40000, &[]))
                .unwrap_err()
                .contains("大きすぎ")
        );
        assert!(validate_dimensions(32768, 8192).is_ok());
        assert!(validate_dimensions(32768, 8193).is_err());
        assert!(validate_dimensions(1, 32769).is_err());
        assert!(validate_dimensions(0, 1).is_err());
    }

    fn timestamp() -> CaptureTimestamp {
        CaptureTimestamp {
            stamp: "20261002-153012-345".into(),
            month: "2026-10".into(),
        }
    }

    #[test]
    fn names_keep_one_stack_prefix_with_domain_underscores_and_three_digits() {
        for collision in [1, 2, 3] {
            let first = capture_filename(
                &timestamp(),
                Some("my_site.example.com"),
                1,
                125,
                collision,
                "png",
            );
            let first_stem = Path::new(&first).file_stem().unwrap().to_str().unwrap();
            let prefix = crate::filename_stack::prefix_of(first_stem, '_').to_owned();
            assert!(prefix.contains("my-site.example.com"));
            assert!(!prefix.contains('_'));
            for index in 1..=125 {
                let name = capture_filename(
                    &timestamp(),
                    Some("my_site.example.com"),
                    index,
                    125,
                    collision,
                    "png",
                );
                let stem = Path::new(&name).file_stem().unwrap().to_str().unwrap();
                assert_eq!(crate::filename_stack::prefix_of(stem, '_'), prefix);
            }
            if collision > 1 {
                assert!(prefix.starts_with(&format!("20261002-153012-345-{collision}-")));
            }
            assert!(first.ends_with("_001.png"));
        }
        assert_eq!(
            capture_filename(&timestamp(), None, 1, 1, 1, "png"),
            "20261002-153012-345_01.png"
        );
        assert!(
            !capture_filename(&timestamp(), Some("a/b:c?d*e\\f\n"), 1, 1, 1, "png").contains('/')
        );
    }

    #[test]
    fn monthly_part_publication_avoids_overwrite_and_cleanup_preserves_other_files() {
        let root = tempfile::tempdir().unwrap();
        let image = decode_image(&RawClipboardData {
            png: Some(png(1, 1, &[1, 2, 3, 255])),
            ..Default::default()
        })
        .unwrap();
        let first = save_image(&image, root.path(), &timestamp()).unwrap();
        let second = save_image(&image, root.path(), &timestamp()).unwrap();
        assert_eq!(first.path.parent().unwrap(), root.path().join("2026-10"));
        assert_eq!(
            second.path.file_name().unwrap(),
            "20261002-153012-345-2_01.png"
        );
        assert_eq!(std::fs::read(&first.path).unwrap(), image.png);
        let month = first.path.parent().unwrap();
        let orphan = month.join(".miv-clipboard-orphan.part");
        let unrelated = month.join("user.part");
        std::fs::write(&orphan, b"partial").unwrap();
        std::fs::write(&unrelated, b"keep").unwrap();
        std::fs::create_dir_all(root.path().join("other")).unwrap();
        let outside_month = root.path().join("other/.miv-clipboard-orphan.part");
        std::fs::write(&outside_month, b"keep").unwrap();
        cleanup_part_files(root.path()).unwrap();
        assert!(!orphan.exists());
        assert!(unrelated.exists());
        assert!(outside_month.exists());
        assert!(first.path.exists());
        assert!(second.path.exists());
    }

    #[test]
    fn origin_formats_have_distinct_encoding_and_firefox_html_is_not_provenance() {
        let uri_w = "https://user:secret@img.example.com/a.png#part\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let chromium_source = b"https://user:secret@page.example.com/post#part\0padding".to_vec();
        let raw = RawClipboardData {
            uri_w: Some(uri_w),
            chromium_source: Some(chromium_source),
            ..Default::default()
        };
        let origin = image_origin(&raw);
        assert_eq!(
            origin.page_url.as_deref(),
            Some("https://page.example.com/post")
        );
        assert_eq!(
            origin.image_url.as_deref(),
            Some("https://img.example.com/a.png")
        );
        assert_eq!(origin.domain().as_deref(), Some("page.example.com"));
        let firefox = RawClipboardData {
            html: Some(
                b"Version:1.0\r\n<html><img src='https://image.example.com/a'></html>".to_vec(),
            ),
            ..Default::default()
        };
        assert_eq!(image_origin(&firefox), CaptureOrigin::default());
        assert_eq!(
            image_origin(&RawClipboardData {
                chromium_source: Some(b"https://example.com/\xff".to_vec()),
                ..Default::default()
            }),
            CaptureOrigin::default()
        );
    }

    #[test]
    fn motw_removes_credentials_and_fragments_and_rejects_line_injection() {
        let origin = CaptureOrigin {
            page_url: Some("https://user:secret@page.example.com/path?q=1#fragment".into()),
            image_url: Some("https://img.example.com/a.png#fragment".into()),
        };
        assert_eq!(
            motw_contents(&origin).unwrap(),
            "[ZoneTransfer]\r\nZoneId=3\r\nReferrerUrl=https://page.example.com/path?q=1\r\nHostUrl=https://img.example.com/a.png\r\n"
        );
        assert!(
            motw_contents(&CaptureOrigin {
                page_url: Some("https://example.com/\r\nZoneId=0".into()),
                image_url: Some("file:///secret".into())
            })
            .is_none()
        );
        assert_eq!(html_source_url(b"Version:1.0\r\nSourceURL:https://user:pw@example.com/post#fragment\r\n<html></html>"), Some("https://example.com/post".into()));
        assert!(html_source_url(b"<html>\nSourceURL:https://example.com\n</html>").is_none());
        assert!(html_source_url(b"Version:1.0\nSourceURL:file:///local\n").is_none());
        assert!(html_source_url(b"Version:1.0\nSourceURL:https://example.com/\xff\n").is_none());
    }
}
