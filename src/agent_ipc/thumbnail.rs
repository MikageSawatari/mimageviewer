//! Direct, bounded agent thumbnails. No catalog, representative resolution, or
//! metadata migration is involved in this lane.
use super::*;
use std::io::Cursor;

const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_DECODED_BYTES: u64 = 256 * 1024 * 1024;

impl Handlers {
    pub(super) fn get_thumbnail(
        &self,
        items: Vec<ItemRef>,
        long_side: Option<u32>,
        cursor: Option<String>,
        ctx: &ReadContext,
    ) -> Result<Payload, AgentError> {
        if !ctx.permission.thumbnails {
            return Err(error(ErrorCode::Disabled, "縮小画像の共有は無効です"));
        }
        if items.is_empty() || items.len() > MAX_THUMBNAIL_ITEMS {
            return Err(error(
                ErrorCode::BadRequest,
                "項目は1～20件で指定してください",
            ));
        }
        let side = long_side.unwrap_or(DEFAULT_THUMBNAIL_SIDE);
        if side < DEFAULT_THUMBNAIL_SIDE {
            return Err(error(
                ErrorCode::BadRequest,
                "一辺は256～512で指定してください",
            ));
        }
        let side = side.min(MAX_THUMBNAIL_SIDE);
        let normalized = items
            .iter()
            .map(|item| {
                Ok(serde_json::json!([
                    super::input_path::cursor_key(&item.path)?,
                    item.subresource.as_ref().map(|s| s.replace('\\', "/"))
                ]))
            })
            .collect::<Result<Vec<_>, AgentError>>()?;
        let args = serde_json::json!([normalized, side, "input_order"]);
        let offset = decode_cursor(cursor.as_deref(), "get_thumbnail", &args)?;
        if offset > items.len() {
            return Err(error(ErrorCode::BadRequest, "最初からやり直してください"));
        }
        ctx.checkpoint()?;
        let settings = crate::settings_db::with_db_result(|db| db.load_into_settings())
            .map_err(|_| internal())?;
        let edits = EditReaders::open()?;
        let io = crate::io_semaphore::GlobalIoSemaphore::process_shared(
            settings.indexer_speed_profile.io_permits(),
        );
        let mut page = ThumbnailBuilder::new(offset, args);
        for item in items.iter().skip(offset) {
            ctx.checkpoint()?;
            let (row, image) =
                read_thumbnail(item, side, settings.thumb_quality, ctx, &edits, &io)?;
            ctx.checkpoint()?;
            if !page.push(row, image)? {
                break;
            }
            // A deadline may return only a complete, validated prefix. Cursor
            // advances by its input offset, so continuation never repeats I/O.
            let more = offset + page.rows.len() < items.len();
            ctx.publish(Payload::Thumbnails(page.response(more)?));
        }
        ctx.checkpoint()?;
        let more = offset + page.rows.len() < items.len();
        Ok(Payload::Thumbnails(page.response(more)?))
    }
}

struct ThumbnailBuilder {
    rows: Vec<ThumbnailItem>,
    images: Vec<ThumbnailImage>,
    bytes: usize,
    offset: usize,
    args: serde_json::Value,
}

impl ThumbnailBuilder {
    fn new(offset: usize, args: serde_json::Value) -> Self {
        Self {
            rows: Vec::new(),
            images: Vec::new(),
            bytes: 0,
            offset,
            args,
        }
    }

    fn push(
        &mut self,
        mut row: ThumbnailItem,
        mut image: Option<Vec<u8>>,
    ) -> Result<bool, AgentError> {
        // A TooLong identity has no attachment even if decoding was successful.
        if mcp_content_chars(&row).map_err(|_| internal())? > MAX_ITEM_CHARS {
            image = None;
        }
        if image
            .as_ref()
            .is_some_and(|bytes| bytes.len() > MAX_THUMBNAIL_IMAGE_BYTES)
        {
            row.status = ThumbnailStatus::TooLarge;
            row.dimensions = None;
            image = None;
        }
        if image
            .as_ref()
            .is_some_and(|bytes| self.bytes + bytes.len() > MAX_THUMBNAIL_TOTAL_BYTES)
        {
            return Ok(false);
        }
        self.rows.push(row);
        let response = self.response(true)?;
        if response.page.items.len() < self.rows.len() {
            self.rows.pop();
            return Ok(false);
        }
        if let Some(webp) = image {
            self.bytes += webp.len();
            self.images.push(ThumbnailImage {
                item_index: self.rows.len() - 1,
                webp,
            });
        }
        Ok(true)
    }

    fn response(&self, more: bool) -> Result<ThumbnailResponse, AgentError> {
        let (items, cursor, truncated) = bounded_rows(
            self.rows.clone(),
            self.offset,
            more,
            "get_thumbnail",
            self.args.clone(),
            "items",
        )?;
        let images = self
            .images
            .iter()
            .filter(|image| image.item_index < items.len())
            .cloned()
            .collect();
        Ok(ThumbnailResponse {
            page: ThumbnailPage {
                items,
                cursor,
                truncated,
            },
            images,
        })
    }
}

struct EditReaders {
    conceal: Option<crate::conceal_db::ConcealDb>,
    rotation: Option<crate::rotation_db::RotationDb>,
}

#[derive(Clone, Copy)]
enum EditItem<'a> {
    File,
    PdfPage(u32),
    ZipEntry { current: &'a str, legacy: &'a str },
}

fn edit_keys(paths: &[std::path::PathBuf], item: EditItem<'_>) -> Result<Vec<String>, AgentError> {
    let mut keys = Vec::new();
    // Rotation priority is spelling first (given, long, short), then current
    // before legacy. Conceal checks every key and any hit hides the pixels.
    for path in paths {
        match item {
            EditItem::ZipEntry { current, legacy } => {
                for name in [current, legacy] {
                    let key = crate::adjustment_db::zip_entry_key(path, name);
                    if !keys.contains(&key) {
                        keys.push(key);
                    }
                }
            }
            EditItem::File | EditItem::PdfPage(_) => {
                let grid_item = match item {
                    EditItem::PdfPage(page_num) => crate::grid_item::GridItem::PdfPage {
                        pdf_path: path.to_owned(),
                        page_num,
                        content_type: None,
                    },
                    _ => crate::grid_item::GridItem::Image(path.to_owned()),
                };
                let key =
                    crate::edit_source::page_key_for_grid_item(&grid_item).ok_or_else(internal)?;
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
    }
    Ok(keys)
}

fn edit_read_error() -> AgentError {
    // SQLite errors can contain the private path or item key. Only a bounded
    // category belongs in diagnostics for this agent lane.
    crate::logger::log("agent thumbnail edit metadata read failed");
    internal()
}

impl EditReaders {
    fn open() -> Result<Self, AgentError> {
        let conceal_path = crate::conceal_db::ConcealDb::db_path();
        let conceal = if conceal_path.try_exists().map_err(|_| edit_read_error())? {
            Some(
                crate::conceal_db::ConcealDb::open_readonly(&conceal_path)
                    .map_err(|_| edit_read_error())?,
            )
        } else {
            None
        };
        let rotation = if crate::rotation_db::RotationDb::db_path()
            .try_exists()
            .map_err(|_| edit_read_error())?
        {
            Some(crate::rotation_db::RotationDb::open_readonly().map_err(|_| edit_read_error())?)
        } else {
            None
        };
        Ok(Self { conceal, rotation })
    }

    fn hidden(&self, keys: &[String]) -> Result<bool, AgentError> {
        if let Some(db) = &self.conceal {
            for key in keys {
                if db
                    .get_full_checked(key)
                    .map_err(|_| edit_read_error())?
                    .is_some()
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn rotation(&self, keys: &[String]) -> Result<crate::rotation_db::Rotation, AgentError> {
        if let Some(db) = &self.rotation {
            for key in keys {
                if let Some(rotation) = db.get_key_checked(key).map_err(|_| edit_read_error())? {
                    return Ok(rotation);
                }
            }
        }
        Ok(crate::rotation_db::Rotation::None)
    }
}

fn supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            crate::folder_tree::SUPPORTED_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
        })
}

fn unavailable(item: &ItemRef, status: ThumbnailStatus) -> (ThumbnailItem, Option<Vec<u8>>) {
    (
        ThumbnailItem {
            item: item.clone(),
            status,
            dimensions: None,
        },
        None,
    )
}

fn read_thumbnail(
    item: &ItemRef,
    side: u32,
    quality: u8,
    ctx: &ReadContext,
    edits: &EditReaders,
    io: &crate::io_semaphore::GlobalIoSemaphore,
) -> Result<(ThumbnailItem, Option<Vec<u8>>), AgentError> {
    let path = super::input_path::validate(&item.path)?;
    let mut exact = item.clone();
    let keys;
    let pixels;
    if let Some(member) = &item.subresource {
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            let page = member
                .strip_prefix("page_")
                .and_then(|s| s.parse::<u32>().ok().filter(|n| n.to_string() == s))
                .ok_or_else(|| error(ErrorCode::BadRequest, "ページの指定が正しくありません"))?;
            keys = edit_keys(
                &super::input_path::edit_spellings(path)?,
                EditItem::PdfPage(page),
            )?;
            if edits.hidden(&keys)? {
                return Ok(unavailable(&exact, ThumbnailStatus::HiddenByEdit));
            }
            pixels = crate::pdf_loader::render_page_anonymous(path, page, side, ctx.cancel.clone())
                .map(|result| result.image)
                .map_err(|_| ThumbnailStatus::Unavailable);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| crate::folder_tree::is_zip_extension(&e.to_ascii_lowercase()))
        {
            crate::book_scope::validate_relative_reference(member)
                .map_err(|_| error(ErrorCode::BadRequest, "項目の指定が正しくありません"))?;
            if !supported(Path::new(member)) {
                return Ok(unavailable(item, ThumbnailStatus::Unsupported));
            }
            let paths = super::input_path::edit_spellings(path)?;
            let _permit = io
                .acquire_cancellable(crate::io_semaphore::IoPriority::Normal, &ctx.cancel)
                .ok_or_else(cancelled)?;
            let read = match super::item_info::read_zip(path, member, &ctx.cancel) {
                Ok(read) => read,
                Err(_) => {
                    ctx.checkpoint()?;
                    return Ok(unavailable(item, ThumbnailStatus::Unavailable));
                }
            };
            exact.subresource = Some(read.name.clone());
            keys = edit_keys(
                &paths,
                EditItem::ZipEntry {
                    current: &read.name,
                    legacy: &read.legacy_name,
                },
            )?;
            if edits.hidden(&keys)? {
                return Ok(unavailable(&exact, ThumbnailStatus::HiddenByEdit));
            }
            pixels = match read.bytes {
                Some(bytes) => decode_bytes(&bytes, &read.name, side, &ctx.cancel),
                None => Err(ThumbnailStatus::TooLarge),
            };
        } else {
            return Ok(unavailable(item, ThumbnailStatus::Unsupported));
        }
    } else {
        if !supported(path) {
            return Ok(unavailable(item, ThumbnailStatus::Unsupported));
        }
        keys = edit_keys(&super::input_path::edit_spellings(path)?, EditItem::File)?;
        if edits.hidden(&keys)? {
            return Ok(unavailable(item, ThumbnailStatus::HiddenByEdit));
        }
        let _permit = io
            .acquire_cancellable(crate::io_semaphore::IoPriority::Normal, &ctx.cancel)
            .ok_or_else(cancelled)?;
        let mut file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(_) => return Ok(unavailable(item, ThumbnailStatus::Unavailable)),
        };
        match file.metadata() {
            Ok(metadata) if !metadata.is_file() => {
                return Ok(unavailable(item, ThumbnailStatus::Unsupported));
            }
            Ok(metadata) if metadata.len() > MAX_FILE_BYTES => {
                return Ok(unavailable(item, ThumbnailStatus::TooLarge));
            }
            _ => {}
        }
        pixels = match super::item_info::read_capped(&mut file, MAX_FILE_BYTES, &ctx.cancel) {
            Ok(Some(bytes)) => decode_bytes(&bytes, &item.path, side, &ctx.cancel),
            Ok(None) => Err(ThumbnailStatus::TooLarge),
            Err(_) => Err(ThumbnailStatus::Unavailable),
        };
    }
    ctx.checkpoint()?;
    let pixels = match pixels {
        Ok(pixels) => pixels,
        Err(status) => return Ok(unavailable(&exact, status)),
    };
    let pixels = crate::capture::rotate_dynamic_image(pixels, edits.rotation(&keys)?);
    ctx.checkpoint()?;
    let Some((webp, width, height)) =
        crate::catalog::encode_thumb_webp(&pixels, side, f32::from(quality))
    else {
        return Ok(unavailable(&exact, ThumbnailStatus::Unavailable));
    };
    ctx.checkpoint()?;
    Ok((
        ThumbnailItem {
            item: exact,
            status: ThumbnailStatus::Ok,
            dimensions: Some(Dimensions {
                width,
                height,
                unit: DimensionUnit::Pixels,
            }),
        },
        Some(webp),
    ))
}

fn decode_bytes(
    bytes: &[u8],
    hint: &str,
    side: u32,
    cancel: &AtomicBool,
) -> Result<image::DynamicImage, ThumbnailStatus> {
    if cancel.load(Ordering::Acquire) {
        return Err(ThumbnailStatus::Unavailable);
    }
    if crate::raw_format::is_raw_path(Path::new(hint)) {
        // preview_bounded probes the actual embedded preview header and checks
        // its output layout before allocation. Developed RAW dimensions are
        // irrelevant here: this lane never develops the full sensor image.
        return crate::raw::raw_decoder::preview_bounded(
            crate::raw::RawSource::Bytes(bytes),
            MAX_DECODED_BYTES,
        )
        .map(|preview| preview.image)
        .map_err(|_| ThumbnailStatus::Unavailable);
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        match crate::thumb_loader::decode_jpeg_turbo_scaled_from_bytes(bytes, side) {
            Ok((image, _)) => {
                return Ok(crate::thumb_loader::apply_exif_orientation_from_bytes(
                    image, bytes,
                ));
            }
            Err(crate::thumb_loader::DctDecodeError::TerminalRejection(_)) => {
                return Err(ThumbnailStatus::Unavailable);
            }
            Err(crate::thumb_loader::DctDecodeError::Fallback(_)) => {}
        }
    }
    if cancel.load(Ordering::Acquire) {
        return Err(ThumbnailStatus::Unavailable);
    }
    // Only full-size decoding needs this estimate: TurboJPEG already bounds
    // its DCT-scaled allocation, so large JPEG sources can still fit safely.
    // Bounded WIC cannot distinguish allocation rejection from codec failure;
    // use the same read-only header probe as get_item_info for TooLarge.
    if super::item_info::bytes_dimensions(bytes, hint).is_some_and(|(w, h)| {
        u64::from(w)
            .checked_mul(u64::from(h))
            .and_then(|n| n.checked_mul(4))
            .is_none_or(|bytes| bytes > MAX_DECODED_BYTES)
    }) {
        return Err(ThumbnailStatus::TooLarge);
    }
    if cancel.load(Ordering::Acquire) {
        return Err(ThumbnailStatus::Unavailable);
    }
    let source = crate::image_decode::Source::Bytes { bytes, hint };
    crate::image_decode::primary(
        source,
        crate::avif_decoder::Workload::Background,
        Some(MAX_DECODED_BYTES),
        Some(cancel),
        || {
            let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
            let mut limits = image::Limits::default();
            limits.max_alloc = Some(MAX_DECODED_BYTES);
            reader.limits(limits);
            reader.decode()
        },
    )
    .map(|decoded| decoded.oriented(source))
    .map_err(|_| ThumbnailStatus::Unavailable)
}

#[cfg(test)]
#[path = "thumbnail_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "thumbnail_alias_tests.rs"]
mod alias_tests;
