//! Worker-only validation and target preparation. No metadata stores are written
//! here; the UI owns permission/Remote admission and the accepted mutation.
use std::path::{Path, PathBuf};

use mimageviewer_agent_ipc::{AgentError, Command, ErrorCode, ItemRef, MetadataSkipReason};

use crate::app::agent_rating_write::PreparedRatingTarget;
use crate::rating_db::{RatingItemKind, RatingMeta};
use crate::tag_ops::TagTarget;

#[derive(Clone, Debug)]
pub(crate) struct PreparedItem<T> {
    pub(crate) input: ItemRef,
    pub(crate) target: Result<T, MetadataSkipReason>,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedTags {
    pub(crate) items: Vec<PreparedItem<TagTarget>>,
    pub(crate) tags: Vec<String>,
    pub(crate) add: bool,
    pub(crate) client_name: String,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedRatings {
    pub(crate) items: Vec<PreparedItem<PreparedRatingTarget>>,
    pub(crate) stars: u8,
    pub(crate) client_name: String,
}

#[derive(Clone, Debug)]
pub(crate) enum PreparedWrite {
    Tags(PreparedTags),
    Ratings(PreparedRatings),
}

pub(crate) fn is_write(command: &Command) -> bool {
    matches!(
        command,
        Command::AddTags { .. } | Command::RemoveTags { .. } | Command::SetRating { .. }
    )
}

fn checkpoint(ctx: &super::ReadContext) -> Result<(), AgentError> {
    ctx.checkpoint()?;
    if ctx.expired() {
        Err(super::timeout())
    } else {
        Ok(())
    }
}

fn bad_request(message: &str) -> AgentError {
    super::error(ErrorCode::BadRequest, message)
}

pub(crate) fn prepare(
    command: Command,
    ctx: &super::ReadContext,
    client_name: String,
) -> Result<PreparedWrite, AgentError> {
    checkpoint(ctx)?;
    let (inputs, tags, add, stars) = match command {
        Command::AddTags { items, tags } => (items, Some(tags), true, 0),
        Command::RemoveTags { items, tags } => (items, Some(tags), false, 0),
        Command::SetRating { items, stars } if stars <= 5 => (items, None, false, stars),
        Command::SetRating { .. } => return Err(bad_request("★は 0～5 で指定してください")),
        _ => return Err(bad_request("書き込み操作の指定が正しくありません")),
    };
    if inputs.is_empty() || inputs.len() > 500 {
        return Err(bad_request("項目は 1～500 件で指定してください"));
    }
    // Validate the complete tag list before any path is inspected or queued.
    let tags = tags
        .map(|tags| {
            if tags.is_empty() || tags.len() > 10 {
                return Err(bad_request("タグは 1～10 個で指定してください"));
            }
            tags.into_iter()
                .map(|tag| {
                    let tag = crate::tags_db::normalize_tag_display_name(&tag);
                    if crate::tags_db::is_valid_tag_display_name(&tag) {
                        Ok(tag)
                    } else {
                        Err(bad_request(
                            "タグ名は空白を含まない 1～64 文字で指定してください",
                        ))
                    }
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    checkpoint(ctx)?;
    // Use the already-open settings owner, rather than the startup recovery
    // loader. Its OS default Pictures lookup belongs on this worker as well.
    let settings = crate::settings_db::with_db_result(|db| db.load_into_settings())
        .map_err(|_| super::error(ErrorCode::Busy, "設定を読み込めませんでした"))?;
    let books_root = settings.books_root_path();
    checkpoint(ctx)?;
    if let Some(tags) = tags {
        let mut items = Vec::with_capacity(inputs.len());
        for input in inputs {
            checkpoint(ctx)?;
            let target = prepare_tag(&input, &books_root);
            checkpoint(ctx)?;
            items.push(PreparedItem { input, target });
        }
        Ok(PreparedWrite::Tags(PreparedTags {
            items,
            tags,
            add,
            client_name,
        }))
    } else {
        let mut items = Vec::with_capacity(inputs.len());
        for input in inputs {
            checkpoint(ctx)?;
            let target = prepare_rating(&input, &books_root, ctx)?;
            checkpoint(ctx)?;
            items.push(PreparedItem { input, target });
        }
        Ok(PreparedWrite::Ratings(PreparedRatings {
            items,
            stars,
            client_name,
        }))
    }
}

fn physical(input: &ItemRef) -> Result<(PathBuf, RatingMeta), MetadataSkipReason> {
    let path =
        super::input_path::validate(&input.path).map_err(|_| MetadataSkipReason::InvalidPath)?;
    let stat = std::fs::metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            MetadataSkipReason::NotFound
        } else {
            MetadataSkipReason::Unavailable
        }
    })?;
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut archive_format = None;
    let kind = if stat.is_dir() {
        RatingItemKind::Folder
    } else if !stat.is_file() {
        return Err(MetadataSkipReason::UnsupportedType);
    } else if crate::folder_tree::is_recognized_image_ext(&extension) {
        RatingItemKind::Image
    } else if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&extension.as_str()) {
        RatingItemKind::Video
    } else if crate::folder_tree::is_audio_ext(&extension) {
        RatingItemKind::Audio
    } else if crate::folder_tree::is_zip_extension(&extension) {
        RatingItemKind::ZipFile
    } else if crate::folder_tree::is_paged_document_path(path) {
        RatingItemKind::PdfFile
    } else if let Some(format) = crate::archive_converter::ArchiveFormat::from_extension(&extension)
    {
        archive_format = Some(
            match format {
                crate::archive_converter::ArchiveFormat::Rar => "rar",
                crate::archive_converter::ArchiveFormat::SevenZ => "7z",
                crate::archive_converter::ArchiveFormat::Lzh => "lzh",
                crate::archive_converter::ArchiveFormat::Zip => "zip",
            }
            .to_owned(),
        );
        RatingItemKind::ConvertibleArchive
    } else {
        return Err(MetadataSkipReason::UnsupportedType);
    };
    let mut meta = RatingMeta::new(kind).with_source_path(path);
    meta.archive_format = archive_format;
    Ok((path.to_owned(), meta))
}

fn compiled_page(path: &Path, meta: &RatingMeta, books_root: &Path) -> bool {
    meta.kind == RatingItemKind::Image
        && path
            .parent()
            .is_some_and(|parent| crate::books::is_direct_book_folder(books_root, parent))
}

fn prepare_tag(input: &ItemRef, books_root: &Path) -> Result<TagTarget, MetadataSkipReason> {
    super::input_path::validate(&input.path).map_err(|_| MetadataSkipReason::InvalidPath)?;
    if input.subresource.is_some() {
        return Err(MetadataSkipReason::UnsupportedSubresource);
    }
    let (path, meta) = physical(input)?;
    let tag_sidecar = (meta.kind != RatingItemKind::Folder
        && !compiled_page(&path, &meta, books_root))
    .then(|| crate::tag_write_worker::sidecar_target_for_real_file(&path))
    .flatten();
    Ok(TagTarget { path, tag_sidecar })
}

fn prepare_rating(
    input: &ItemRef,
    books_root: &Path,
    ctx: &super::ReadContext,
) -> Result<Result<PreparedRatingTarget, MetadataSkipReason>, AgentError> {
    prepare_rating_with_pdf_sizes(input, books_root, ctx, |path| {
        crate::pdf_loader::get_page_sizes_anonymous(path, None)
    })
}

fn prepare_rating_with_pdf_sizes(
    input: &ItemRef,
    books_root: &Path,
    ctx: &super::ReadContext,
    pdf_sizes: impl FnOnce(&Path) -> std::io::Result<Vec<(f32, f32)>>,
) -> Result<Result<PreparedRatingTarget, MetadataSkipReason>, AgentError> {
    let (path, mut meta) = match physical(input) {
        Ok(physical) => physical,
        Err(reason) => return Ok(Err(reason)),
    };
    checkpoint(ctx)?;
    let mut legacy_key = None;
    let key = if let Some(member) = input.subresource.as_deref() {
        match meta.kind {
            RatingItemKind::ZipFile => {
                let member = member.replace('\\', "/");
                let is_directory = member.ends_with('/');
                let reference = member.strip_suffix('/').unwrap_or(&member);
                if crate::book_scope::validate_relative_reference(reference).is_err()
                    || (!is_directory
                        && !crate::folder_tree::is_recognized_image_ext(
                            &Path::new(&member)
                                .extension()
                                .and_then(|extension| extension.to_str())
                                .unwrap_or("")
                                .to_ascii_lowercase(),
                        ))
                {
                    return Ok(Err(MetadataSkipReason::UnsupportedSubresource));
                }
                checkpoint(ctx)?;
                if is_directory {
                    let identity =
                        super::item_info::read_zip_directory_identity(&path, &member, &ctx.cancel);
                    checkpoint(ctx)?;
                    let identity = match identity {
                        Ok(Some(identity)) => identity,
                        Ok(None) => return Ok(Err(MetadataSkipReason::Unavailable)),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            return Ok(Err(MetadataSkipReason::NotFound));
                        }
                        Err(_) => return Ok(Err(MetadataSkipReason::Unavailable)),
                    };
                    let key = zip_directory_rating_key(&path, &identity.name);
                    let old_key = zip_directory_rating_key(&path, &identity.legacy_name);
                    legacy_key = (old_key != key).then_some(old_key);
                    meta.kind = RatingItemKind::ZipDir;
                    meta.zipdir_is_archive = Some(crate::zip_tree::segment_is_archive(
                        crate::grid_item::zipdir_display_name(&identity.name),
                    ));
                    meta.dir_prefix = Some(identity.name);
                    key
                } else {
                    let zip = super::item_info::read_zip_identity(&path, &member, &ctx.cancel);
                    checkpoint(ctx)?;
                    let zip = match zip {
                        Ok(zip) if zip.size.is_some() => zip,
                        Ok(_) => return Ok(Err(MetadataSkipReason::Unavailable)),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            return Ok(Err(MetadataSkipReason::NotFound));
                        }
                        Err(_) => return Ok(Err(MetadataSkipReason::Unavailable)),
                    };
                    let key = crate::adjustment_db::zip_entry_key(&path, &zip.name);
                    let old_key = crate::adjustment_db::zip_entry_key(&path, &zip.legacy_name);
                    legacy_key = (old_key != key).then_some(old_key);
                    meta.kind = RatingItemKind::ZipImage;
                    meta.entry_name = Some(zip.name);
                    key
                }
            }
            RatingItemKind::PdfFile
                if path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf")) =>
            {
                let page_num = match pdf_page_number(member) {
                    Some(page) => page,
                    None => return Ok(Err(MetadataSkipReason::UnsupportedSubresource)),
                };
                checkpoint(ctx)?;
                // No diagnostic identity or cancellation token: wait for actual
                // PDF completion, so the request permit owns the IPC lifetime.
                let sizes = pdf_sizes(&path);
                checkpoint(ctx)?;
                let sizes = match sizes {
                    Ok(sizes) => sizes,
                    Err(_) => return Ok(Err(MetadataSkipReason::Unavailable)),
                };
                if !pdf_page_in_range(page_num, sizes.len()) {
                    return Ok(Err(MetadataSkipReason::UnsupportedSubresource));
                }
                meta.kind = RatingItemKind::PdfPage;
                meta.page_num = Some(page_num);
                crate::edit_source::page_key_for_pdf(&path, page_num)
            }
            _ => return Ok(Err(MetadataSkipReason::UnsupportedSubresource)),
        }
    } else {
        crate::adjustment_db::normalize_path(&path)
    };
    let compiled_book_page = compiled_page(&path, &meta, books_root);
    let xmp_target = (meta.kind == RatingItemKind::Image
        && !compiled_book_page
        && crate::xmp_writer::is_writable_format(&path))
    .then(|| path.clone());
    Ok(Ok(PreparedRatingTarget {
        key,
        legacy_key,
        source: path,
        meta,
        xmp_target,
        compiled_book_page,
    }))
}

pub(super) fn zip_directory_rating_key(path: &Path, prefix: &str) -> String {
    crate::adjustment_db::normalize_path(&crate::app::book_container_key(path, prefix))
}

fn pdf_page_number(member: &str) -> Option<u32> {
    let number = member.strip_prefix("page_")?;
    let page = number.parse::<u32>().ok()?;
    (page.to_string() == number).then_some(page)
}

fn pdf_page_in_range(page: u32, count: usize) -> bool {
    usize::try_from(page).is_ok_and(|page| page < count)
}

#[cfg(test)]
#[path = "metadata_write_tests.rs"]
mod tests;
