//! Column-specific, read-only metadata projection. No UI hydration or sidecars.
use std::io::{Cursor, Read, Seek};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use mimageviewer_agent_ipc::*;

use crate::grid_item::GridItem;

const MAX_ZIP_BYTES: u64 = 64 * 1024 * 1024;

fn check_cancel(cancel: &AtomicBool) -> Result<(), AgentError> {
    if cancel.load(Ordering::Acquire) {
        Err(super::cancelled())
    } else {
        Ok(())
    }
}

fn bad_item() -> AgentError {
    super::error(ErrorCode::BadRequest, "項目の指定が正しくありません")
}

fn milliseconds(time: SystemTime) -> Option<i64> {
    match time.duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_millis()).ok(),
        Err(before) => i64::try_from(before.duration().as_millis())
            .ok()?
            .checked_neg(),
    }
}

fn available<T>(value: Option<T>) -> InfoField<T> {
    value.map_or(InfoField::Unavailable, InfoField::Present)
}

fn pixel_dimensions(dims: Option<(u32, u32)>) -> InfoField<Dimensions> {
    available(dims.map(|(width, height)| Dimensions {
        width,
        height,
        unit: DimensionUnit::Pixels,
    }))
}

pub(super) fn read(
    input: &ItemRef,
    permission: PermissionSnapshot,
    cancel: &AtomicBool,
) -> Result<ItemInfo, AgentError> {
    check_cancel(cancel)?;
    let path = super::input_path::validate(&input.path)?;
    let metadata = std::fs::metadata(path).ok();
    check_cancel(cancel)?;
    let kind = if metadata.as_ref().is_some_and(std::fs::Metadata::is_dir) {
        ItemKind::Folder
    } else {
        super::item_from_hit(&input.path).kind
    };
    let mut output = ItemInfo {
        item: input.clone(),
        name: path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned(),
        kind,
        size: available(metadata.as_ref().map(std::fs::Metadata::len)),
        modified: available(
            metadata
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(milliseconds),
        ),
        dimensions: InfoField::NotApplicable,
        rating: 0,
        tags: InfoField::Present(Vec::new()),
        container_size: None,
        container_modified: None,
        container_tags: Vec::new(),
        exif: InfoField::NotApplicable,
        generation: InfoField::NotApplicable,
    };

    let mut legacy_rating_key = None;
    let grid_item = if let Some(subresource) = &input.subresource {
        output.kind = ItemKind::Image;
        output.container_size = metadata.as_ref().map(std::fs::Metadata::len);
        output.container_modified = metadata
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(milliseconds);
        output.tags = InfoField::NotApplicable;
        match kind {
            ItemKind::Pdf
                if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("pdf")) =>
            {
                let page = subresource.strip_prefix("page_").ok_or_else(bad_item)?;
                let page_num = page.parse::<u32>().map_err(|_| bad_item())?;
                if page_num.to_string() != page {
                    return Err(bad_item());
                }
                output.name = format!("Page {}", u64::from(page_num) + 1);
                output.size = InfoField::NotApplicable;
                output.modified = InfoField::NotApplicable;
                // This existing call waits until the actual PDF IPC completes. Do not
                // add AbortOnCancel(Some(cancel)): the outer deadline may reply first,
                // but its Permit must remain held while this worker still owns IPC.
                check_cancel(cancel)?;
                let sizes = crate::pdf_loader::get_page_sizes_anonymous(path, None);
                check_cancel(cancel)?;
                output.dimensions = match sizes {
                    Ok(sizes) => {
                        let &(width, height) = sizes.get(page_num as usize).ok_or_else(bad_item)?;
                        if width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 {
                            InfoField::Present(Dimensions {
                                width: width.round() as u32,
                                height: height.round() as u32,
                                unit: DimensionUnit::Points,
                            })
                        } else {
                            InfoField::Unavailable
                        }
                    }
                    Err(_) => InfoField::Unavailable,
                };
                GridItem::PdfPage {
                    pdf_path: path.to_owned(),
                    page_num,
                    content_type: None,
                }
            }
            ItemKind::Archive
                if path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    crate::folder_tree::is_zip_extension(&e.to_ascii_lowercase())
                }) =>
            {
                crate::book_scope::validate_relative_reference(subresource)
                    .map_err(|_| bad_item())?;
                let member = subresource.replace('\\', "/");
                let extension = Path::new(&member)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if !crate::folder_tree::is_recognized_image_ext(&extension) {
                    return Err(bad_item());
                }
                let zip = read_zip(path, &member, cancel);
                check_cancel(cancel)?;
                let member = match zip {
                    Ok(zip) => {
                        legacy_rating_key =
                            Some(crate::adjustment_db::zip_entry_key(path, &zip.legacy_name));
                        output.size = available(zip.size);
                        output.modified = available(zip.modified);
                        if let Some(bytes) = zip.bytes {
                            output.dimensions = pixel_dimensions(bytes_dimensions(&bytes, &member));
                            check_cancel(cancel)?;
                            output.exif = project_exif(
                                crate::exif_reader::read_exif_from_bytes(&bytes, &[]),
                                permission.gps,
                            );
                            check_cancel(cancel)?;
                            output.generation = project_generation(
                                crate::png_metadata::extract_metadata_from_bytes_for_agent(&bytes),
                            );
                        } else {
                            output.dimensions = InfoField::TooLarge;
                            output.exif = InfoField::TooLarge;
                            output.generation = InfoField::TooLarge;
                        }
                        zip.name
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return Err(bad_item());
                    }
                    Err(_) => {
                        output.size = InfoField::Unavailable;
                        output.modified = InfoField::Unavailable;
                        output.dimensions = InfoField::Unavailable;
                        output.exif = InfoField::Unavailable;
                        output.generation = InfoField::Unavailable;
                        member
                    }
                };
                output.name = crate::zip_loader::entry_basename(&member).to_owned();
                output.item.subresource = Some(member.clone());
                GridItem::ZipImage {
                    zip_path: path.to_owned(),
                    entry_name: member,
                }
            }
            _ => return Err(bad_item()),
        }
    } else {
        if kind == ItemKind::Image {
            output.dimensions = pixel_dimensions(crate::remote_ipc::file_image_dims(path));
            check_cancel(cancel)?;
            output.exif = project_exif(crate::exif_reader::read_exif(path, &[]), permission.gps);
            check_cancel(cancel)?;
            output.generation =
                project_generation(crate::png_metadata::extract_metadata_for_agent(path));
        }
        GridItem::Image(path.to_owned())
    };

    check_cancel(cancel)?;
    enrich(&mut output, &grid_item, path, legacy_rating_key.as_deref())?;
    trim_descriptions(&mut output)?;
    check_cancel(cancel)?;
    Ok(output)
}

fn enrich(
    output: &mut ItemInfo,
    item: &GridItem,
    path: &Path,
    legacy_rating_key: Option<&str>,
) -> Result<(), AgentError> {
    // These are precisely App::rating_path_key / page_path_key's physical and
    // page keys; a page must never inherit the container's stars.
    let rating_key = crate::edit_source::page_key_for_grid_item(item)
        .unwrap_or_else(|| crate::adjustment_db::normalize_path(path));
    let ratings_path = crate::rating_db::RatingDb::db_path();
    if ratings_path.try_exists().map_err(|_| super::internal())? {
        let db = crate::rating_db::RatingDb::open_readonly(ratings_path)
            .map_err(|_| super::internal())?;
        // Presence matters: a current zero-star row must override old stars.
        let keys: Vec<_> = std::iter::once(rating_key.clone())
            .chain(legacy_rating_key.map(str::to_owned))
            .collect();
        let ratings = db.get_many(&keys);
        output.rating = ratings
            .get(&rating_key)
            .copied()
            .or_else(|| legacy_rating_key.and_then(|key| ratings.get(key).copied()))
            .unwrap_or(0);
    }
    let tags = super::optional_tags(&crate::tags_db::TagsDb::db_path())?;
    let tags = tags.map_or_else(Vec::new, |db| {
        db.display_tags_for_item(&crate::tags_db::item_key_for_path(path))
    });
    if output.item.subresource.is_some() {
        output.container_tags = tags;
    } else {
        output.tags = InfoField::Present(tags);
    }
    Ok(())
}

pub(super) fn bytes_dimensions(bytes: &[u8], name: &str) -> Option<(u32, u32)> {
    if crate::raw_format::is_raw_path(Path::new(name)) {
        let dims = crate::raw::raw_decoder::info(crate::raw::RawSource::Bytes(bytes))
            .ok()?
            .developed_dims;
        return Some((dims[0], dims[1]));
    }
    if let Some(result) = crate::image_decode::avif_dimensions(
        crate::image_decode::Source::Bytes { bytes, hint: name },
        crate::avif_decoder::Workload::Background,
    ) {
        return result.ok();
    }
    image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

pub(super) struct ZipRead {
    pub(super) name: String,
    pub(super) legacy_name: String,
    pub(super) size: Option<u64>,
    pub(super) modified: Option<i64>,
    // With payload reads enabled, None means the leaf or an intermediate
    // archive exceeded the bound. Identity-only reads always leave this None;
    // size=None then denotes an intermediate archive that could not be read.
    pub(super) bytes: Option<Vec<u8>>,
}

pub(super) fn read_zip(path: &Path, member: &str, cancel: &AtomicBool) -> std::io::Result<ZipRead> {
    read_zip_with_leaf_bytes(path, member, cancel, true)
}

/// Canonical and legacy identities require no decompression of the image.
/// Only intermediate nested ZIPs are read, with the normal bounded reader.
pub(super) fn read_zip_identity(
    path: &Path,
    member: &str,
    cancel: &AtomicBool,
) -> std::io::Result<ZipRead> {
    read_zip_with_leaf_bytes(path, member, cancel, false)
}

pub(super) struct ZipDirectoryIdentity {
    pub(super) name: String,
    pub(super) legacy_name: String,
}

/// Inspect directory identities without enumerating/decoding images or changing
/// any ZIP migration state. None denotes an oversized intermediate archive.
pub(super) fn read_zip_directory_identity(
    path: &Path,
    prefix: &str,
    cancel: &AtomicBool,
) -> std::io::Result<Option<ZipDirectoryIdentity>> {
    match crate::zip_loader::open_archive(path)? {
        crate::zip_loader::ZipArchiveHandle::Zip(archive) => {
            read_zip_directory_impl(archive, prefix, cancel)
        }
        crate::zip_loader::ZipArchiveHandle::Rar(_) => Err(std::io::ErrorKind::InvalidInput.into()),
    }
}

fn read_zip_directory_impl<R: Read + Seek>(
    mut archive: zip::ZipArchive<R>,
    prefix: &str,
    cancel: &AtomicBool,
) -> std::io::Result<Option<ZipDirectoryIdentity>> {
    // Literal directory paths take precedence over nested ZIP boundaries, just
    // as image lookups do. ZIPs need not contain explicit directory entries.
    for index in 0..archive.len() {
        if cancel.load(Ordering::Acquire) {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        let entry = archive.by_index_raw(index).map_err(std::io::Error::other)?;
        if let Some(identity) = zip_directory_entry_identity(&entry, prefix) {
            return Ok(Some(identity));
        }
    }
    let parts = crate::zip_loader::split_nested_zip_path(prefix);
    let Some((first, remainder)) = parts.split_first().filter(|(_, rest)| !rest.is_empty()) else {
        return Err(std::io::ErrorKind::NotFound.into());
    };
    let index = crate::zip_loader::resolve_entry_index(&mut archive, first, None, Some(cancel))?;
    // A terminal nested book needs its identity only. Do not read its payload.
    let terminal = remainder.len() == 1 && remainder[0].is_empty();
    let outer = read_zip_index(&mut archive, index, cancel, !terminal)?;
    if terminal {
        return Ok(Some(ZipDirectoryIdentity {
            name: format!("{}/", outer.name),
            legacy_name: format!("{}/", outer.legacy_name),
        }));
    }
    let Some(bytes) = outer.bytes else {
        return Ok(None);
    };
    drop(archive);
    let inner = zip::ZipArchive::new(Cursor::new(bytes)).map_err(std::io::Error::other)?;
    Ok(
        read_zip_directory_impl(inner, &remainder.join("/"), cancel)?.map(|identity| {
            ZipDirectoryIdentity {
                name: format!("{}/{}", outer.name, identity.name),
                legacy_name: format!("{}/{}", outer.legacy_name, identity.legacy_name),
            }
        }),
    )
}

fn zip_directory_entry_identity(
    entry: &zip::read::ZipFile<'_>,
    prefix: &str,
) -> Option<ZipDirectoryIdentity> {
    let raw = entry.name_raw();
    let current = crate::zip_loader::zip_entry_name(entry);
    let current_name = current.replace('\\', "/");
    let legacy_name = entry.name().replace('\\', "/");
    if !current_name.starts_with(prefix) && !legacy_name.starts_with(prefix) {
        return None;
    }
    let utf8 = std::str::from_utf8(raw).ok();
    // zip 2.x exposes the legacy spelling decoded as UTF-8 (including Unicode
    // path extras), or CP437, which maps exactly one character per raw byte.
    let legacy_utf8 = String::from_utf8_lossy(raw) == entry.name();
    let current_cp932 = utf8.is_none() && current != entry.name();
    // Match one raw boundary in both spellings. CP932's trail byte 0x5c is a
    // legacy backslash, so normalized legacy and current component counts can
    // differ (e.g. 表/inner/). Never truncate both by the same component count.
    for (offset, byte) in raw.iter().enumerate() {
        if !matches!(byte, b'/' | b'\\') {
            continue;
        }
        let end = offset + 1;
        let legacy_prefix = if legacy_utf8 {
            String::from_utf8_lossy(&raw[..end]).into_owned()
        } else {
            entry.name().chars().take(end).collect()
        };
        let current_prefix = if let Some(utf8) = utf8 {
            utf8[..end].to_owned()
        } else if current_cp932 {
            let (decoded, _, had_errors) = encoding_rs::SHIFT_JIS.decode(&raw[..end]);
            if had_errors {
                continue;
            }
            decoded.into_owned()
        } else {
            legacy_prefix.clone()
        };
        let current_prefix = current_prefix.replace('\\', "/");
        let legacy_prefix = legacy_prefix.replace('\\', "/");
        // A 0x5c inside a CP932 character is not a decoded separator.
        if current_prefix.ends_with('/')
            && current_name.starts_with(&current_prefix)
            && (current_prefix == prefix || legacy_prefix == prefix)
        {
            return Some(ZipDirectoryIdentity {
                name: current_prefix,
                legacy_name: legacy_prefix,
            });
        }
    }
    None
}

fn read_zip_with_leaf_bytes(
    path: &Path,
    member: &str,
    cancel: &AtomicBool,
    read_leaf_bytes: bool,
) -> std::io::Result<ZipRead> {
    match crate::zip_loader::open_archive(path)? {
        crate::zip_loader::ZipArchiveHandle::Zip(archive) => {
            read_zip_member_impl(archive, member, cancel, read_leaf_bytes)
        }
        crate::zip_loader::ZipArchiveHandle::Rar(_) => Err(std::io::ErrorKind::InvalidInput.into()),
    }
}

#[cfg(test)]
fn read_zip_member<R: Read + Seek>(
    archive: zip::ZipArchive<R>,
    member: &str,
    cancel: &AtomicBool,
) -> std::io::Result<ZipRead> {
    read_zip_member_impl(archive, member, cancel, true)
}

fn read_zip_member_impl<R: Read + Seek>(
    mut archive: zip::ZipArchive<R>,
    member: &str,
    cancel: &AtomicBool,
    read_leaf_bytes: bool,
) -> std::io::Result<ZipRead> {
    // Literal full names take precedence over nested boundaries, as in the
    // existing read_entry_bytes path (including flattened conversion caches).
    match crate::zip_loader::resolve_entry_index(&mut archive, member, None, Some(cancel)) {
        Ok(index) => return read_zip_index(&mut archive, index, cancel, read_leaf_bytes),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
        Err(_) => {}
    }
    let parts = crate::zip_loader::split_nested_zip_path(member);
    let Some((first, remainder)) = parts.split_first().filter(|(_, rest)| !rest.is_empty()) else {
        return Err(std::io::ErrorKind::NotFound.into());
    };
    let index = crate::zip_loader::resolve_entry_index(&mut archive, first, None, Some(cancel))?;
    let outer = read_zip_index(&mut archive, index, cancel, true)?;
    let Some(bytes) = outer.bytes else {
        return Ok(ZipRead {
            name: member.to_owned(),
            legacy_name: member.to_owned(),
            size: None,
            modified: None,
            bytes: None,
        });
    };
    // Release the parent's memory before visiting another nested archive.
    // The 64 MiB bound must not multiply by the number of nesting levels.
    drop(archive);
    let inner = zip::ZipArchive::new(Cursor::new(bytes)).map_err(std::io::Error::other)?;
    let mut result = read_zip_member_impl(inner, &remainder.join("/"), cancel, read_leaf_bytes)?;
    result.name = format!("{}/{}", outer.name, result.name);
    result.legacy_name = format!("{}/{}", outer.legacy_name, result.legacy_name);
    Ok(result)
}

fn read_zip_index<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
    cancel: &AtomicBool,
    read_bytes: bool,
) -> std::io::Result<ZipRead> {
    // Raw metadata is inspected before constructing a decompressor or reserving
    // the member's declared size. Actual reads get an independent hard cap.
    let entry = archive.by_index_raw(index).map_err(std::io::Error::other)?;
    if entry.is_dir() {
        return Err(std::io::ErrorKind::NotFound.into());
    }
    let mut result = ZipRead {
        name: crate::zip_loader::zip_entry_name(&entry).replace('\\', "/"),
        legacy_name: entry.name().replace('\\', "/"),
        size: Some(entry.size()),
        modified: entry.last_modified().and_then(zip_milliseconds),
        bytes: None,
    };
    if !read_bytes || entry.size() > MAX_ZIP_BYTES {
        return Ok(result);
    }
    drop(entry);
    let mut entry = archive.by_index(index).map_err(std::io::Error::other)?;
    result.bytes = read_capped(&mut entry, MAX_ZIP_BYTES, cancel)?;
    Ok(result)
}

pub(super) fn read_capped(
    reader: &mut impl Read,
    maximum: u64,
    cancel: &AtomicBool,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 32 * 1024];
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        let remaining = maximum.saturating_sub(bytes.len() as u64);
        let read_limit = (remaining.saturating_add(1)).min(buffer.len() as u64) as usize;
        let count = reader.read(&mut buffer[..read_limit])?;
        if count == 0 {
            return Ok(Some(bytes));
        }
        if count as u64 > remaining {
            return Ok(None);
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

fn zip_milliseconds(date: zip::DateTime) -> Option<i64> {
    if !date.is_valid() {
        return None;
    }
    // DOS timestamps carry no timezone. Preserve their calendar value, using
    // UTC for the wire conversion rather than substituting the outer mtime.
    let mut year = i64::from(date.year());
    let month = i64::from(date.month());
    year -= i64::from(month <= 2);
    let era = year / 400;
    let year_in_era = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let day_in_year = (153 * month + 2) / 5 + i64::from(date.day()) - 1;
    let day_in_era = year_in_era * 365 + year_in_era / 4 - year_in_era / 100 + day_in_year;
    let days = era * 146_097 + day_in_era - 719_468;
    Some(
        (days * 86400
            + i64::from(date.hour()) * 3600
            + i64::from(date.minute()) * 60
            + i64::from(date.second()))
            * 1000,
    )
}

fn project_exif(source: Option<crate::exif_reader::ExifInfo>, gps: bool) -> InfoField<ExifInfo> {
    let Some(source) = source else {
        return InfoField::Unavailable;
    };
    let fields = source.sections.iter().flat_map(|(_, fields)| fields);
    let lookup = |wanted: &str| {
        fields
            .clone()
            .find(|(key, _)| key == wanted)
            .map(|(_, value)| value.clone())
    };
    let mut info = ExifInfo {
        make: lookup("Make"),
        model: lookup("Model"),
        lens: lookup("LensModel"),
        captured_at: lookup("DateTimeOriginal"),
        exposure_time: lookup("ExposureTime"),
        f_number: lookup("FNumber"),
        iso: lookup("PhotographicSensitivity"),
        focal_length: lookup("FocalLength"),
        orientation: lookup("Orientation"),
        gps: None,
    };
    if gps {
        info.gps = (|| {
            let latitude = coordinate(
                &lookup("GPSLatitude")?,
                lookup("GPSLatitudeRef").as_deref(),
                90.0,
            )?;
            let longitude = coordinate(
                &lookup("GPSLongitude")?,
                lookup("GPSLongitudeRef").as_deref(),
                180.0,
            )?;
            let altitude = lookup("GPSAltitude").and_then(|value| {
                let altitude = value.split_whitespace().next()?.parse::<f64>().ok()?;
                if !altitude.is_finite() {
                    return None;
                }
                Some(
                    if value.contains("below sea level")
                        || lookup("GPSAltitudeRef").is_some_and(|v| v == "Below sea level")
                    {
                        -altitude.abs()
                    } else {
                        altitude
                    },
                )
            });
            Some(GpsInfo {
                latitude,
                longitude,
                altitude,
            })
        })();
    }
    InfoField::Present(info)
}

fn coordinate(value: &str, reference: Option<&str>, maximum: f64) -> Option<f64> {
    let components = value
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .filter(|p| !p.is_empty())
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let degrees = match components.as_slice() {
        [d] => *d,
        [d, m] if *m < 60.0 => d + m / 60.0,
        [d, m, s] if *m < 60.0 && *s < 60.0 => d + m / 60.0 + s / 3600.0,
        _ => return None,
    };
    if !degrees.is_finite() || degrees > maximum {
        return None;
    }
    let direction = reference
        .and_then(|r| r.chars().next())
        .or_else(|| value.chars().last())?;
    match direction {
        'N' | 'E' => Some(degrees),
        'S' | 'W' => Some(-degrees),
        _ => None,
    }
}

fn text(value: impl Into<String>) -> TextValue {
    TextValue {
        text: value.into(),
        truncated: false,
    }
}

fn project_generation(
    source: Option<crate::png_metadata::AiMetadata>,
) -> InfoField<GenerationInfo> {
    use crate::png_metadata::{AiMetadata, ComfyPromptRole};
    let Some(source) = source else {
        return InfoField::Unavailable;
    };
    if matches!(source, AiMetadata::Unknown(_)) {
        return InfoField::Unavailable;
    }
    let mut output = GenerationInfo {
        tool: crate::png_metadata::ai_tool_name(&source).map(str::to_owned),
        model: crate::png_metadata::model_name(&source),
        ..Default::default()
    };
    let params = match &source {
        AiMetadata::A1111(meta) => {
            output.prompt = (!meta.prompt.is_empty()).then(|| text(meta.prompt.clone()));
            output.negative_prompt =
                (!meta.negative_prompt.is_empty()).then(|| text(meta.negative_prompt.clone()));
            &meta.params
        }
        AiMetadata::ComfyUI(meta) => {
            for (role, destination) in [
                (ComfyPromptRole::Positive, &mut output.prompt),
                (ComfyPromptRole::Negative, &mut output.negative_prompt),
            ] {
                let values = meta
                    .resolved_prompts
                    .iter()
                    .filter(|prompt| prompt.role == role)
                    .map(|prompt| prompt.text.as_str())
                    .collect::<Vec<_>>();
                if !values.is_empty() {
                    *destination = Some(text(values.join("\n")));
                }
            }
            &meta.sampler_params
        }
        AiMetadata::Unknown(_) => unreachable!(),
    };
    let param = |names: &[&str]| {
        params
            .iter()
            .find(|(key, _)| names.iter().any(|name| key.eq_ignore_ascii_case(name)))
            .map(|(_, value)| value.clone())
    };
    output.seed = param(&["seed"]).map(text);
    output.steps = param(&["steps", "num_inference_steps"]).map(text);
    output.sampler = param(&["sampler", "sampler_name"]).map(text);
    output.cfg = param(&[
        "cfg",
        "cfg scale",
        "cfg_scale",
        "guidance_scale",
        "guidance scale",
        "scale",
    ])
    .map(text);
    output.dimensions = param(&["size", "dimensions"])
        .or_else(|| Some(format!("{}x{}", param(&["width"])?, param(&["height"])?)))
        .map(text);
    InfoField::Present(output)
}

fn trim_descriptions(output: &mut ItemInfo) -> Result<(), AgentError> {
    // Restrict only explanatory TextValue fields. Other fields retain exact
    // identity; the page owner emits a digest marker if they alone are too big.
    while mcp_content_chars(output).map_err(|_| super::internal())? > MAX_ITEM_CHARS {
        let InfoField::Present(generation) = &mut output.generation else {
            break;
        };
        let longest = [
            &mut generation.prompt,
            &mut generation.negative_prompt,
            &mut generation.seed,
            &mut generation.steps,
            &mut generation.sampler,
            &mut generation.cfg,
            &mut generation.dimensions,
        ]
        .into_iter()
        .filter_map(Option::as_mut)
        .filter(|value| !value.text.is_empty())
        .max_by_key(|value| value.text.len());
        let Some(value) = longest else {
            break;
        };
        let target = value.text.chars().count() / 2;
        let boundary = value
            .text
            .char_indices()
            .nth(target)
            .map_or(value.text.len(), |(index, _)| index);
        value.text.truncate(boundary);
        value.truncated = true;
    }
    Ok(())
}

#[cfg(test)]
#[path = "item_info_tests.rs"]
mod tests;
