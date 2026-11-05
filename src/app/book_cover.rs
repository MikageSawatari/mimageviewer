//! Worker-only book-cover resolution. Source selection is shared with thumbnails;
//! the resulting source identity always belongs to the original logical book.

use super::*;
use crate::ai_caption_db::FileSignature;
use crate::archive_converter::ArchiveFormat;
use crate::folder_thumb_pins::{FileKind, FolderPinSource, ResolvedKind, ResolvedPinTarget};
use crate::thumb_loader::{ReadLocation, ResolveStrategy};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BookKind {
    Zip,
    Pdf,
    Archive(ArchiveFormat),
    Folder,
}

impl BookKind {
    /// Classification only: safe to use while preparing a UI request.
    pub(crate) fn for_path(path: &Path) -> Self {
        let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        if crate::folder_tree::is_pdf_extension(&ext.to_ascii_lowercase())
            || ext.eq_ignore_ascii_case("epub")
        {
            Self::Pdf
        } else if let Some(format) = ArchiveFormat::from_extension(ext) {
            Self::Archive(format)
        } else if crate::folder_tree::is_zip_extension(&ext.to_ascii_lowercase()) {
            Self::Zip
        } else {
            Self::Folder
        }
    }

    fn grid_item(self, path: &Path) -> GridItem {
        match self {
            Self::Zip => GridItem::ZipFile(path.to_owned()),
            Self::Pdf => GridItem::PdfFile(path.to_owned()),
            Self::Archive(format) => GridItem::ConvertibleArchive {
                path: path.to_owned(),
                format,
            },
            Self::Folder => GridItem::Folder(path.to_owned()),
        }
    }
}

#[derive(Clone)]
pub(crate) struct BookCoverEnv {
    pub(crate) books_root: Option<PathBuf>,
    pub(crate) folder_thumb_sort: crate::settings::SortOrder,
    pub(crate) folder_thumb_depth: u32,
    pub(crate) visibility: crate::file_type_visibility::FileTypeVisibility,
    pub(crate) pin_db: Option<Arc<crate::folder_thumb_pins::FolderThumbPinDb>>,
    pub(crate) video_pins: Option<crate::video_pins::VideoPinReadSource>,
    pub(crate) archive_cache: Option<Arc<crate::archive_cache::ArchiveCacheDb>>,
    pub(crate) pdf_passwords: crate::pdf_passwords::PdfPasswordStore,
    pub(crate) current_pdf: Option<PathBuf>,
    pub(crate) current_password: Option<String>,
}

impl Default for BookCoverEnv {
    fn default() -> Self {
        Self {
            books_root: None,
            folder_thumb_sort: crate::settings::SortOrder::Numeric,
            folder_thumb_depth: 3,
            visibility: Default::default(),
            pin_db: None,
            video_pins: None,
            archive_cache: None,
            pdf_passwords: Default::default(),
            current_pdf: None,
            current_password: None,
        }
    }
}

impl App {
    /// Capture handles and settings only; opening stores and source I/O happen on the worker.
    pub(crate) fn book_cover_env(&self) -> BookCoverEnv {
        BookCoverEnv {
            books_root: Some(self.book_root_path()),
            folder_thumb_sort: self.settings.folder_thumb_sort,
            folder_thumb_depth: self.settings.folder_thumb_depth,
            visibility: self.listing_file_type_visibility().clone(),
            pin_db: self.folder_thumb_pin_db.clone(),
            video_pins: self.video_pin_db.as_ref().and_then(|db| db.read_source()),
            archive_cache: self.archive_cache_db.clone(),
            pdf_passwords: self.pdf_passwords.clone(),
            current_pdf: self.current_folder.clone(),
            current_password: self.pdf_current_password.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BookCoverFailure {
    Unconverted,
    Missing,
    ReadFailed,
    PasswordRequired,
}

#[derive(Debug)]
pub(crate) struct ResolvedBookCover {
    pub(crate) location: ReadLocation,
    pub(crate) signature_path: PathBuf,
    pub(crate) signature: FileSignature,
    pub(crate) cover_ref: String,
    pub(crate) epub_read: Option<crate::pdf_loader::ReadTarget>,
    pub(crate) pdf_password: Option<String>,
    pub(crate) preloaded_bytes: Option<Vec<u8>>,
}

#[derive(Debug)]
pub(crate) enum BookCover {
    Readable(ResolvedBookCover),
    VideoFrame {
        cover_ref: String,
        signature_path: PathBuf,
        signature: FileSignature,
    },
    Unreadable(BookCoverFailure),
}

struct CoverResolver<'a> {
    env: &'a BookCoverEnv,
    video_db: Option<crate::video_pins::VideoPinDb>,
    visited: HashSet<String>,
}

pub(crate) fn resolve_book_cover(
    book_path: &Path,
    kind: BookKind,
    env: &BookCoverEnv,
) -> BookCover {
    // The UI supplies an I/O-free kind hint. An accepted recipe can classify
    // a real directory named album.pdf/album.zip as a document/archive.
    // Establish the actual owner here, before choosing its signature policy.
    let metadata = match std::fs::metadata(book_path) {
        Ok(metadata) => metadata,
        Err(_) => return BookCover::Unreadable(BookCoverFailure::Missing),
    };
    let kind = if metadata.is_dir() {
        BookKind::Folder
    } else {
        kind
    };
    let original_signature = if kind != BookKind::Folder {
        match FileSignature::from_metadata(&metadata) {
            Some(signature) => Some(signature),
            None => return BookCover::Unreadable(BookCoverFailure::ReadFailed),
        }
    } else {
        None
    };
    let video_db = env
        .video_pins
        .as_ref()
        .and_then(|source| crate::video_pins::VideoPinDb::open_readonly(&source.path).ok());
    let mut resolver = CoverResolver {
        env,
        video_db,
        visited: HashSet::new(),
    };
    match resolver.resolve(book_path, kind, None) {
        Ok(mut cover) => {
            // File books are signed by the original file, never the converted ZIP.
            if kind != BookKind::Folder {
                cover.signature_path = book_path.to_owned();
                cover.signature = match cover
                    .epub_read
                    .as_ref()
                    .and_then(|read| read.display_source_state)
                {
                    Some(source) if kind == BookKind::Pdf => match source_signature(source) {
                        Some(signature) => signature,
                        None => return BookCover::Unreadable(BookCoverFailure::ReadFailed),
                    },
                    _ => original_signature.unwrap(),
                };
            }
            cover.cover_ref = location_ref(book_path, &cover.signature_path, &cover.location);
            BookCover::Readable(cover)
        }
        Err(CoverError::Video {
            path,
            pts,
            signature,
        }) => BookCover::VideoFrame {
            cover_ref: format!("{}#frame={pts}", relative_ref(book_path, &path)),
            signature_path: path,
            signature,
        },
        Err(CoverError::Failure(error)) => BookCover::Unreadable(error),
    }
}

enum CoverError {
    Failure(BookCoverFailure),
    Video {
        path: PathBuf,
        pts: f64,
        signature: FileSignature,
    },
}

impl From<BookCoverFailure> for CoverError {
    fn from(value: BookCoverFailure) -> Self {
        Self::Failure(value)
    }
}

impl CoverResolver<'_> {
    fn resolve(
        &mut self,
        path: &Path,
        kind: BookKind,
        selected: Option<&crate::thumb_loader::SelectedChildPin>,
    ) -> Result<ResolvedBookCover, CoverError> {
        // Reconstructing a cached child is not another folder-search level.
        // Each child repeats its own released selection at folder_thumb_depth;
        // only a repeated owner terminates the cached reconstruction chain.
        if !self
            .visited
            .insert(crate::path_key::normalize_keep_drive(path))
        {
            return Err(BookCoverFailure::ReadFailed.into());
        }
        let metadata = std::fs::metadata(path).map_err(|_| BookCoverFailure::Missing)?;
        // Classify every owner, including the top level, on the worker.
        let kind = if metadata.is_dir() {
            BookKind::Folder
        } else {
            kind
        };
        // An automatic child fallback can name a direct image, not just a child book.
        if kind == BookKind::Folder && metadata.is_file() && selected.is_none() {
            return Ok(ResolvedBookCover {
                location: ReadLocation::File(path.to_owned()),
                signature_path: path.to_owned(),
                signature: FileSignature::from_metadata(&metadata)
                    .ok_or(BookCoverFailure::ReadFailed)?,
                cover_ref: String::new(),
                epub_read: None,
                pdf_password: None,
                preloaded_bytes: None,
            });
        }
        let item = kind.grid_item(path);
        let source = if let Some(selected) = selected {
            Some(
                source_for_selected_pin(&selected.container, &selected.target)
                    .ok_or(BookCoverFailure::ReadFailed)?,
            )
        } else {
            self.env.pin_db.as_ref().and_then(|db| db.lookup(path))
        };
        let mut pins = HashMap::new();
        if let Some(source) = source.as_ref() {
            pins.insert(crate::path_key::normalize_keep_drive(path), source.clone());
        }
        let target = source.as_ref().and_then(|source| {
            resolve_pin_target_cascaded(
                path,
                source,
                self.env.pin_db.as_deref(),
                self.env.folder_thumb_depth as usize,
            )
        });
        let mut converted = HashMap::new();
        let mut logical_paths = Vec::new();
        for candidate_path in
            std::iter::once(path).chain(target.as_ref().map(|target| target.abs_path.as_path()))
        {
            if !crate::folder_tree::is_convertible_archive_path(candidate_path)
                && !(candidate_path == path && matches!(kind, BookKind::Archive(_)))
            {
                continue;
            }
            let archive_key = crate::path_key::normalize_keep_drive(candidate_path);
            if converted.contains_key(&archive_key) {
                continue;
            }
            let candidate_metadata =
                std::fs::metadata(candidate_path).map_err(|_| BookCoverFailure::Missing)?;
            let candidate = ConvertedArchiveCandidate {
                archive_key: archive_key.clone(),
                path: candidate_path.to_owned(),
                mtime: crate::ui_helpers::mtime_secs(&candidate_metadata),
                file_size: candidate_metadata.len() as i64,
                idx: 0,
            };
            let state = resolve_converted_archive_candidate(
                &candidate,
                self.env.archive_cache.as_deref(),
                &AtomicBool::new(false),
                0,
            )
            .ok_or(BookCoverFailure::ReadFailed)?;
            if let Some(load_path) = state.load_path() {
                logical_paths.push((load_path.to_owned(), candidate_path.to_owned()));
            } else if candidate_path == path && matches!(kind, BookKind::Archive(_)) {
                return Err(BookCoverFailure::Unconverted.into());
            }
            converted.insert(archive_key, state);
        }
        let password = crate::pdf_passwords::pdf_open_password(
            &self.env.pdf_passwords,
            self.env.current_pdf.as_deref(),
            self.env.current_password.as_deref(),
            path,
        );
        let mut request = make_load_request_with_visibility(
            &item,
            0,
            crate::ui_helpers::mtime_secs(&metadata),
            if metadata.is_dir() {
                0
            } else {
                metadata.len() as i64
            },
            true,
            password.as_deref(),
            Some(self.env.folder_thumb_sort),
            self.env.folder_thumb_depth,
            &pins,
            &converted,
            None,
            None,
            self.env.pin_db.as_deref(),
            self.video_db.as_ref(),
            true,
            &self.env.visibility,
        )
        .ok_or(BookCoverFailure::ReadFailed)?;
        // The selected cached row owns its exact fixed page/entry. The synthetic
        // source above feeds that position back through the released App planner.
        if request.folder_thumb_provenance == Some(crate::catalog::FolderThumbProvenance::Seeded)
            && request.resolve_override.is_none()
            && request.pdf_page.is_none()
            && request.zip_entry.is_none()
            && let Some(target) = target
                .as_ref()
                .filter(|target| target.kind == ResolvedKind::Video)
            && let Some(pin) = self
                .video_db
                .as_ref()
                .and_then(|db| db.lookup(&target.abs_path))
        {
            return Err(CoverError::Video {
                path: target.abs_path.clone(),
                pts: pin.pin_pts_secs,
                signature: file_signature(&target.abs_path)?,
            });
        }
        loop {
            if !request.file_type_visibility.is_all_visible()
                && request.epub_pin_fallback.is_some()
                && (request.zip_entry.is_some()
                    || matches!(
                        request.resolve_override,
                        Some(
                            ResolveStrategy::ZipFirstImage | ResolveStrategy::ZipDirRepresentative
                        )
                    ))
                && !crate::thumb_loader::policy_archive_pin_visible(&request, None)
            {
                request = *request.epub_pin_fallback.take().unwrap();
                continue;
            }
            let prepared = match crate::thumb_loader::prepare_load_request_stamp(&request) {
                Ok(prepared) => prepared,
                Err(()) => {
                    if let Some(fallback) = request.epub_pin_fallback.take() {
                        request = *fallback;
                        continue;
                    }
                    return Err(BookCoverFailure::Unconverted.into());
                }
            };
            let epub_read = prepared.as_ref().map(|(_, read)| read.clone());
            let request = prepared.as_ref().map_or(&request, |(request, _)| request);
            // ZIP first-image selection returns bytes. Capture their logical
            // source signature before that read, so a concurrent replacement
            // cannot attach a newer source signature to older bytes.
            let request_logical_path = logical_paths
                .iter()
                .find(|(load, _)| crate::folder_tree::path_eq(load, &request.path))
                .map_or(request.path.as_path(), |(_, logical)| logical.as_path());
            let request_signature = std::fs::metadata(request_logical_path)
                .ok()
                .filter(|metadata| metadata.is_file())
                .and_then(|metadata| FileSignature::from_metadata(&metadata));
            let resolved = crate::thumb_loader::resolve_read_after_cache_miss(
                request,
                self.env.pin_db.as_deref(),
                None,
            );
            if matches!(resolved.location, ReadLocation::ChildCached) {
                let selected = resolved
                    .child_selection
                    .ok_or(BookCoverFailure::ReadFailed)?;
                let child = selected
                    .pin
                    .as_ref()
                    .map_or(selected.container.as_path(), |pin| pin.container.as_path());
                return self.resolve(child, BookKind::for_path(child), selected.pin.as_ref());
            }
            let physical_path = match &resolved.location {
                ReadLocation::File(path) => path,
                ReadLocation::ArchiveEntry { archive, .. } => archive,
                ReadLocation::PdfPage { pdf, .. } => pdf,
                _ => return Err(BookCoverFailure::ReadFailed.into()),
            };
            let logical_path = logical_paths
                .iter()
                .find(|(load, _)| crate::folder_tree::path_eq(load, physical_path))
                .map_or_else(|| physical_path.to_owned(), |(_, logical)| logical.clone());
            let signature = match epub_read
                .as_ref()
                .and_then(|read| read.display_source_state)
            {
                Some(source) => source_signature(source).ok_or(BookCoverFailure::ReadFailed)?,
                None if crate::folder_tree::path_eq(&logical_path, request_logical_path) => {
                    request_signature.ok_or(BookCoverFailure::ReadFailed)?
                }
                None => file_signature(&logical_path)?,
            };
            let pdf_password = crate::pdf_passwords::pdf_open_password(
                &self.env.pdf_passwords,
                self.env.current_pdf.as_deref(),
                self.env.current_password.as_deref(),
                &logical_path,
            );
            let mut preloaded_bytes = resolved.preloaded_zip_bytes;
            if let ReadLocation::ArchiveEntry { archive, entry } = &resolved.location
                && preloaded_bytes.is_none()
            {
                preloaded_bytes = Some(
                    crate::zip_loader::read_entry_bytes(archive, entry)
                        .map_err(|_| BookCoverFailure::ReadFailed)?,
                );
            }
            let mut read = epub_read;
            if let ReadLocation::PdfPage { pdf, page } = &resolved.location {
                let target = match read.as_ref() {
                    Some(read) => read.clone(),
                    None => crate::pdf_loader::resolve_read_target(pdf)
                        .map_err(|_| BookCoverFailure::ReadFailed)?,
                };
                validate_pdf_page(pdf, &target, pdf_password.as_deref(), *page)?;
                read = Some(target);
            }
            return Ok(ResolvedBookCover {
                location: resolved.location,
                signature_path: logical_path,
                signature,
                cover_ref: String::new(),
                epub_read: read,
                pdf_password,
                preloaded_bytes,
            });
        }
    }
}

fn validate_pdf_page(
    pdf: &Path,
    read: &crate::pdf_loader::ReadTarget,
    password: Option<&str>,
    page: u32,
) -> Result<(), BookCoverFailure> {
    #[cfg(test)]
    if let Some(result) = tests::validate_pdf_fixture(pdf, read, password, page) {
        return result;
    }
    let pages = crate::pdf_loader::enumerate_pages_with_read_target(
        pdf,
        read,
        password,
        None,
        Default::default(),
    )
    .map_err(|error| {
        if crate::pdf_loader::is_password_required_error(&error) {
            BookCoverFailure::PasswordRequired
        } else {
            BookCoverFailure::ReadFailed
        }
    })?;
    if pages.pages.iter().any(|entry| entry.page_num == page) {
        Ok(())
    } else {
        Err(BookCoverFailure::ReadFailed)
    }
}

fn source_for_selected_pin(
    container: &Path,
    target: &ResolvedPinTarget,
) -> Option<FolderPinSource> {
    let rel = if crate::folder_tree::path_eq(container, &target.abs_path) {
        String::new()
    } else {
        target
            .abs_path
            .strip_prefix(container)
            .ok()?
            .to_string_lossy()
            .replace('\\', "/")
    };
    Some(match target.kind {
        ResolvedKind::ZipEntry => FolderPinSource::ZipEntry {
            zip_rel: rel,
            entry: target.zip_entry.clone()?,
        },
        ResolvedKind::ZipDirRepresentative => FolderPinSource::ZipDir {
            zip_rel: rel,
            dir_prefix: target.zip_dir_prefix.clone()?,
        },
        ResolvedKind::PdfPage | ResolvedKind::PdfFirstPage => FolderPinSource::PdfPage {
            pdf_rel: rel,
            page: target.pdf_page.unwrap_or(0),
        },
        kind => FolderPinSource::File {
            rel,
            kind: match kind {
                ResolvedKind::Image => FileKind::Image,
                ResolvedKind::Video => FileKind::Video,
                ResolvedKind::Folder => FileKind::Folder,
                ResolvedKind::ZipFirstImage => FileKind::ZipFile,
                ResolvedKind::ArchiveFirstImage => FileKind::ConvertibleArchive,
                _ => return None,
            },
        },
    })
}

fn file_signature(path: &Path) -> Result<FileSignature, BookCoverFailure> {
    let metadata = std::fs::metadata(path).map_err(|_| BookCoverFailure::Missing)?;
    FileSignature::from_metadata(&metadata).ok_or(BookCoverFailure::ReadFailed)
}

fn source_signature(source: crate::epub_cache::SourceState) -> Option<FileSignature> {
    const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;
    Some(FileSignature {
        size: source.size,
        mtime_ms: i64::try_from(source.mtime_ticks.checked_sub(FILETIME_UNIX_EPOCH)? / 10_000)
            .ok()?,
    })
}

fn relative_ref(book: &Path, source: &Path) -> String {
    if crate::folder_tree::path_eq(book, source) {
        String::new()
    } else {
        source
            .strip_prefix(book)
            .unwrap_or(source)
            .to_string_lossy()
            .replace('\\', "/")
    }
}

fn location_ref(book: &Path, logical_source: &Path, location: &ReadLocation) -> String {
    let relative = relative_ref(book, logical_source);
    match location {
        ReadLocation::ArchiveEntry { entry, .. } => format!("{relative}#entry={entry}"),
        ReadLocation::PdfPage { page, .. } => format!("{relative}#page={page}"),
        _ => relative,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::Write;
    use tempfile::TempDir;

    struct PdfFixture {
        path: PathBuf,
        password: Option<String>,
        pages: u32,
        source: Option<crate::epub_cache::SourceState>,
    }
    thread_local! { static PDF_FIXTURE: RefCell<Option<PdfFixture>> = const { RefCell::new(None) }; }

    // Substitute only the native PDF boundary, retaining the real planner,
    // EPUB resolver/lease, selection, password policy, and failure routing.
    pub(super) fn validate_pdf_fixture(
        pdf: &Path,
        read: &crate::pdf_loader::ReadTarget,
        password: Option<&str>,
        page: u32,
    ) -> Option<Result<(), BookCoverFailure>> {
        PDF_FIXTURE.with(|slot| {
            slot.borrow().as_ref().map(|fixture| {
                assert_eq!(pdf, fixture.path);
                assert_eq!(read.display_source_state, fixture.source);
                if password != fixture.password.as_deref() {
                    Err(BookCoverFailure::PasswordRequired)
                } else if page >= fixture.pages {
                    Err(BookCoverFailure::ReadFailed)
                } else {
                    Ok(())
                }
            })
        })
    }

    fn with_pdf<T>(fixture: PdfFixture, run: impl FnOnce() -> T) -> T {
        struct Reset(Option<PdfFixture>);
        impl Drop for Reset {
            fn drop(&mut self) {
                PDF_FIXTURE.with(|slot| {
                    slot.replace(self.0.take());
                });
            }
        }
        let _reset = Reset(PDF_FIXTURE.with(|slot| slot.replace(Some(fixture))));
        run()
    }

    fn readable(cover: BookCover) -> ResolvedBookCover {
        match cover {
            BookCover::Readable(cover) => cover,
            other => panic!("expected readable cover, got {other:?}"),
        }
    }

    fn write_zip(path: &Path, names: &[&str]) {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for name in names {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(name.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    fn pin_env(tmp: &TempDir) -> BookCoverEnv {
        BookCoverEnv {
            pin_db: Some(Arc::new(
                crate::folder_thumb_pins::FolderThumbPinDb::open_at(&tmp.path().join("pins.db"))
                    .unwrap(),
            )),
            ..Default::default()
        }
    }

    fn webp() -> Vec<u8> {
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::new(8, 8));
        crate::catalog::encode_thumb_webp(&image, 8, 75.0)
            .unwrap()
            .0
    }

    #[test]
    fn book_kind_classification_is_pure_and_keeps_archive_formats() {
        for (path, expected) in [
            ("missing/BOOK.CBZ", BookKind::Zip),
            ("missing/BOOK.PDF", BookKind::Pdf),
            ("missing/BOOK.EPUB", BookKind::Pdf),
            ("missing/BOOK.CBR", BookKind::Archive(ArchiveFormat::Rar)),
            ("missing/BOOK.CB7", BookKind::Archive(ArchiveFormat::SevenZ)),
            ("missing/folder", BookKind::Folder),
        ] {
            assert_eq!(BookKind::for_path(Path::new(path)), expected);
        }
    }

    #[test]
    fn book_cover_epub_filetime_matches_metadata_and_truncates_submillisecond_ticks() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("source.epub");
        std::fs::write(&path, b"book").unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!(
            source_signature(crate::epub_cache::source_state(&metadata)),
            FileSignature::from_metadata(&metadata)
        );
        let epoch = 116_444_736_000_000_000;
        assert_eq!(
            source_signature(crate::epub_cache::SourceState {
                size: 7,
                mtime_ticks: epoch + 12_349_999
            }),
            Some(FileSignature {
                size: 7,
                mtime_ms: 1234
            })
        );
        assert_eq!(
            source_signature(crate::epub_cache::SourceState {
                size: 7,
                mtime_ticks: epoch - 1
            }),
            None
        );
    }

    #[test]
    fn book_cover_zip_uses_archive_order_and_retains_preloaded_bytes() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("book.zip");
        write_zip(&path, &["z.jpg", "001.jpg"]);
        let env = BookCoverEnv {
            folder_thumb_sort: crate::settings::SortOrder::FileName,
            ..Default::default()
        };
        let cover = readable(resolve_book_cover(&path, BookKind::Zip, &env));
        assert_eq!(cover.cover_ref, "#entry=z.jpg");
        assert_eq!(cover.signature_path, path);
        assert_eq!(cover.signature, file_signature(&path).unwrap());
        assert_eq!(cover.preloaded_bytes.as_deref(), Some(b"z.jpg".as_slice()));
    }

    #[test]
    fn book_cover_zip_fixed_entry_change_is_observed_and_missing_entry_never_falls_back() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("book.zip");
        write_zip(&path, &["001.jpg", "002.jpg"]);
        let env = pin_env(&tmp);
        let db = env.pin_db.as_ref().unwrap();
        for entry in ["002.jpg", "001.jpg"] {
            db.set(
                &path,
                &FolderPinSource::ZipEntry {
                    zip_rel: String::new(),
                    entry: entry.into(),
                },
            )
            .unwrap();
            let cover = readable(resolve_book_cover(&path, BookKind::Zip, &env));
            assert_eq!(cover.cover_ref, format!("#entry={entry}"));
            assert_eq!(cover.signature, file_signature(&path).unwrap());
        }
        db.set(
            &path,
            &FolderPinSource::ZipEntry {
                zip_rel: String::new(),
                entry: "missing.jpg".into(),
            },
        )
        .unwrap();
        assert!(matches!(
            resolve_book_cover(&path, BookKind::Zip, &env),
            BookCover::Unreadable(BookCoverFailure::ReadFailed)
        ));
    }

    #[test]
    fn book_cover_folder_uses_folder_sort_depth_and_actual_file_signature() {
        let tmp = TempDir::new().unwrap();
        let folder = tmp.path().join("book");
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        std::fs::write(folder.join("10.jpg"), b"ten").unwrap();
        std::fs::write(folder.join("2.jpg"), b"two").unwrap();
        let mut env = BookCoverEnv::default();
        let cover = readable(resolve_book_cover(&folder, BookKind::Folder, &env));
        assert_eq!(cover.cover_ref, "2.jpg");
        assert_eq!(cover.signature_path, folder.join("2.jpg"));
        // FileName follows Windows logical name order too: 2 precedes 10.
        env.folder_thumb_sort = crate::settings::SortOrder::FileName;
        assert_eq!(
            readable(resolve_book_cover(&folder, BookKind::Folder, &env)).cover_ref,
            "2.jpg"
        );
        for (name, seconds) in [("2.jpg", 1_600_000_000), ("10.jpg", 1_600_000_010)] {
            std::fs::File::options()
                .write(true)
                .open(folder.join(name))
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new().set_modified(
                        std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds),
                    ),
                )
                .unwrap();
        }
        env.folder_thumb_sort = crate::settings::SortOrder::DateDesc;
        assert_eq!(
            readable(resolve_book_cover(&folder, BookKind::Folder, &env)).cover_ref,
            "10.jpg"
        );
        std::fs::remove_file(folder.join("10.jpg")).unwrap();
        std::fs::remove_file(folder.join("2.jpg")).unwrap();
        std::fs::write(folder.join("sub/cover.jpg"), b"sub").unwrap();
        env.folder_thumb_depth = 0;
        assert!(matches!(
            resolve_book_cover(&folder, BookKind::Folder, &env),
            BookCover::Unreadable(BookCoverFailure::ReadFailed)
        ));
        env.folder_thumb_depth = 1;
        assert_eq!(
            readable(resolve_book_cover(&folder, BookKind::Folder, &env)).cover_ref,
            "sub/cover.jpg"
        );
    }

    #[test]
    fn book_cover_folder_pin_cascade_keeps_fixed_image() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("book");
        let child = root.join("child");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(root.join("001.jpg"), b"auto").unwrap();
        std::fs::write(child.join("fixed.jpg"), b"fixed").unwrap();
        let env = pin_env(&tmp);
        env.pin_db
            .as_ref()
            .unwrap()
            .set(
                &root,
                &FolderPinSource::File {
                    rel: "child".into(),
                    kind: FileKind::Folder,
                },
            )
            .unwrap();
        env.pin_db
            .as_ref()
            .unwrap()
            .set(
                &child,
                &FolderPinSource::File {
                    rel: "fixed.jpg".into(),
                    kind: FileKind::Image,
                },
            )
            .unwrap();
        let cover = readable(resolve_book_cover(&root, BookKind::Folder, &env));
        assert_eq!(cover.cover_ref, "child/fixed.jpg");
        assert_eq!(cover.signature_path, child.join("fixed.jpg"));
    }

    #[test]
    fn book_cover_unconverted_and_missing_are_distinct() {
        let _data = crate::data_dir::TestDataDirGuard::new();
        let tmp = TempDir::new().unwrap();
        for (name, kind) in [
            ("book.7z", BookKind::Archive(ArchiveFormat::SevenZ)),
            ("book.epub", BookKind::Pdf),
        ] {
            let path = tmp.path().join(name);
            assert!(matches!(
                resolve_book_cover(&path, kind, &BookCoverEnv::default()),
                BookCover::Unreadable(BookCoverFailure::Missing)
            ));
            std::fs::write(&path, b"not converted").unwrap();
            assert!(matches!(
                resolve_book_cover(&path, kind, &BookCoverEnv::default()),
                BookCover::Unreadable(BookCoverFailure::Unconverted)
            ));
        }
    }

    #[test]
    fn book_cover_converted_archive_uses_original_signature_and_actual_entry_spelling() {
        let _data = crate::data_dir::TestDataDirGuard::new();
        let tmp = TempDir::new().unwrap();
        let original = tmp.path().join("book.7z");
        let converted = tmp.path().join("converted.zip");
        std::fs::write(&original, b"original sevenz").unwrap();
        write_zip(&converted, &["normalized/cover_2.jpg"]);
        let metadata = std::fs::metadata(&original).unwrap();
        let cache = Arc::new(crate::archive_cache::ArchiveCacheDb::open().unwrap());
        cache
            .record(
                &original,
                crate::ui_helpers::mtime_secs(&metadata),
                metadata.len() as i64,
                ArchiveFormat::SevenZ,
                &converted,
                std::fs::metadata(&converted).unwrap().len() as i64,
                1,
                false,
            )
            .unwrap();
        let env = BookCoverEnv {
            archive_cache: Some(cache),
            ..Default::default()
        };
        let cover = readable(resolve_book_cover(
            &original,
            BookKind::Archive(ArchiveFormat::SevenZ),
            &env,
        ));
        assert_eq!(cover.cover_ref, "#entry=normalized/cover_2.jpg");
        assert_eq!(cover.signature_path, original);
        assert_eq!(
            cover.signature,
            FileSignature::from_metadata(&metadata).unwrap()
        );
        assert!(
            matches!(cover.location, ReadLocation::ArchiveEntry { archive, .. } if archive == converted)
        );
    }

    #[test]
    fn book_cover_direct_rar_uses_original_book_identity() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/archives/rar-multipart-filename-regression/○×△□ Vol.2.rar");
        let cover = readable(resolve_book_cover(
            &path,
            BookKind::Archive(ArchiveFormat::Rar),
            &BookCoverEnv::default(),
        ));
        assert_eq!(cover.cover_ref, "#entry=test_no_metadata.png");
        assert_eq!(cover.signature_path, path);
        assert_eq!(cover.signature, file_signature(&path).unwrap());
    }

    #[test]
    fn book_cover_pdf_fixed_page_and_password_validation_share_the_open_policy() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("book.pdf");
        std::fs::write(&path, b"native boundary fixture").unwrap();
        let mut env = pin_env(&tmp);
        env.current_pdf = Some(path.clone());
        env.current_password = Some("validated".into());
        env.pin_db
            .as_ref()
            .unwrap()
            .set(
                &path,
                &FolderPinSource::PdfPage {
                    pdf_rel: String::new(),
                    page: 2,
                },
            )
            .unwrap();
        with_pdf(
            PdfFixture {
                path: path.clone(),
                password: Some("validated".into()),
                pages: 3,
                source: None,
            },
            || {
                let cover = readable(resolve_book_cover(&path, BookKind::Pdf, &env));
                assert_eq!(cover.cover_ref, "#page=2");
                assert_eq!(cover.pdf_password.as_deref(), Some("validated"));
                assert!(cover.epub_read.is_some());
                env.pin_db
                    .as_ref()
                    .unwrap()
                    .set(
                        &path,
                        &FolderPinSource::PdfPage {
                            pdf_rel: String::new(),
                            page: 3,
                        },
                    )
                    .unwrap();
                assert!(matches!(
                    resolve_book_cover(&path, BookKind::Pdf, &env),
                    BookCover::Unreadable(BookCoverFailure::ReadFailed)
                ));
                env.current_password = None;
                assert!(matches!(
                    resolve_book_cover(&path, BookKind::Pdf, &env),
                    BookCover::Unreadable(BookCoverFailure::PasswordRequired)
                ));
            },
        );
    }

    #[test]
    fn book_cover_epub_uses_the_leased_generation_source_signature() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("book.epub");
        std::fs::write(&path, b"current source differs from retained generation").unwrap();
        let source = crate::epub_cache::SourceState {
            size: 17,
            mtime_ticks: 116_444_736_000_123_456,
        };
        let _lease = crate::pdf_loader::pin_epub_with_source_for_test(&path, 7, 100, source);
        with_pdf(
            PdfFixture {
                path: path.clone(),
                password: None,
                pages: 1,
                source: Some(source),
            },
            || {
                let cover = readable(resolve_book_cover(
                    &path,
                    BookKind::Pdf,
                    &BookCoverEnv::default(),
                ));
                assert_eq!(cover.cover_ref, "#page=0");
                assert_eq!(cover.signature, source_signature(source).unwrap());
                assert_ne!(cover.signature, file_signature(&path).unwrap());
                assert_eq!(cover.epub_read.unwrap().display_source_state, Some(source));
            },
        );
    }

    #[test]
    fn book_cover_cached_child_folder_preserves_the_pin_owner_and_third_pdf_page() {
        let _data = crate::data_dir::TestDataDirGuard::new();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("A");
        let child = root.join("B");
        std::fs::create_dir_all(&child).unwrap();
        let pdf = child.join("book.pdf");
        std::fs::write(&pdf, b"native boundary fixture").unwrap();
        let env = pin_env(&tmp);
        let source = FolderPinSource::PdfPage {
            pdf_rel: "book.pdf".into(),
            page: 2,
        };
        env.pin_db.as_ref().unwrap().set(&child, &source).unwrap();
        let target =
            resolve_pin_target_cascaded(&child, &source, env.pin_db.as_deref(), 3).unwrap();
        let base = crate::thumb_loader::folder_thumb_cache_key_for_path(
            &child,
            false,
            env.folder_thumb_sort,
            3,
            crate::catalog::FolderThumbProvenance::Seeded,
        )
        .unwrap();
        let key = crate::thumb_loader::pinned_folder_row_key(&base, &target.source_id, None);
        let catalog =
            crate::catalog::CatalogDb::open(&crate::catalog::default_cache_dir(), &root).unwrap();
        catalog
            .save_thumb_bytes(&key, target.mtime, target.file_size, Some((8, 8)), &webp())
            .unwrap();
        with_pdf(
            PdfFixture {
                path: pdf.clone(),
                password: None,
                pages: 3,
                source: None,
            },
            || {
                let cover = readable(resolve_book_cover(&root, BookKind::Folder, &env));
                assert_eq!(cover.cover_ref, "B/book.pdf#page=2");
                assert_eq!(cover.signature_path, pdf);
            },
        );
    }

    #[test]
    fn book_cover_cached_child_zip_rechecks_the_childs_fixed_entry() {
        let _data = crate::data_dir::TestDataDirGuard::new();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("book");
        std::fs::create_dir_all(&root).unwrap();
        let zip = root.join("child.zip");
        write_zip(&zip, &["001.jpg", "fixed.jpg"]);
        let env = pin_env(&tmp);
        let source = FolderPinSource::ZipEntry {
            zip_rel: String::new(),
            entry: "fixed.jpg".into(),
        };
        env.pin_db.as_ref().unwrap().set(&zip, &source).unwrap();
        let target = resolve_pin_target_cascaded(&zip, &source, env.pin_db.as_deref(), 3).unwrap();
        let key = format!("zipthumb:child.zip#pin:{}", target.source_id);
        let catalog =
            crate::catalog::CatalogDb::open(&crate::catalog::default_cache_dir(), &root).unwrap();
        catalog
            .save_thumb_bytes(&key, target.mtime, target.file_size, Some((8, 8)), &webp())
            .unwrap();
        let cover = readable(resolve_book_cover(&root, BookKind::Folder, &env));
        assert_eq!(cover.cover_ref, "child.zip#entry=fixed.jpg");
        assert_eq!(cover.signature_path, zip);
    }

    #[test]
    fn book_cover_depth_one_cached_child_keeps_cascaded_third_pdf_page() {
        let _data = crate::data_dir::TestDataDirGuard::new();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("A");
        let child = root.join("B");
        let owner = child.join("C");
        std::fs::create_dir_all(&owner).unwrap();
        let pdf = owner.join("book.pdf");
        std::fs::write(&pdf, b"native boundary fixture").unwrap();
        let mut env = pin_env(&tmp);
        env.folder_thumb_depth = 1;
        let db = env.pin_db.as_ref().unwrap();
        db.set(
            &owner,
            &FolderPinSource::PdfPage {
                pdf_rel: "book.pdf".into(),
                page: 2,
            },
        )
        .unwrap();
        let source = FolderPinSource::File {
            rel: "C".into(),
            kind: FileKind::Folder,
        };
        db.set(&child, &source).unwrap();
        let target = resolve_pin_target_cascaded(&child, &source, Some(db), 1).unwrap();
        assert_eq!(target.pdf_page, Some(2));
        let base = crate::thumb_loader::folder_thumb_cache_key_for_path(
            &child,
            false,
            env.folder_thumb_sort,
            1,
            crate::catalog::FolderThumbProvenance::Seeded,
        )
        .unwrap();
        let key = crate::thumb_loader::pinned_folder_row_key(&base, &target.source_id, None);
        let catalog =
            crate::catalog::CatalogDb::open(&crate::catalog::default_cache_dir(), &root).unwrap();
        catalog
            .save_thumb_bytes(&key, target.mtime, target.file_size, Some((8, 8)), &webp())
            .unwrap();
        with_pdf(
            PdfFixture {
                path: pdf.clone(),
                password: None,
                pages: 3,
                source: None,
            },
            || {
                let cover = readable(resolve_book_cover(&root, BookKind::Folder, &env));
                assert_eq!(cover.cover_ref, "B/C/book.pdf#page=2");
                assert_eq!(cover.signature_path, pdf);
            },
        );
    }

    #[test]
    fn book_cover_automatic_fallback_cached_row_restarts_its_child_book_and_keeps_nested_pdf_pin() {
        use crate::catalog::FolderThumbProvenance as P;
        let _data = crate::data_dir::TestDataDirGuard::new();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("A");
        // Folder names need not agree with the extension-only BookKind hint.
        let child = root.join("B.pdf");
        let grandchild = child.join("C");
        std::fs::create_dir_all(&grandchild).unwrap();
        let pdf = grandchild.join("book.pdf");
        std::fs::write(&pdf, b"native boundary fixture").unwrap();
        let archive = child.join("book.cbz");
        let mut inner = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        inner
            .start_file("page.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        inner.write_all(b"nested source").unwrap();
        let inner = inner.finish().unwrap().into_inner();
        let mut outer = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        outer
            .start_file("inner.zip", zip::write::SimpleFileOptions::default())
            .unwrap();
        outer.write_all(&inner).unwrap();
        outer.finish().unwrap();
        let mut env = pin_env(&tmp);
        env.folder_thumb_depth = 1;
        env.visibility.excluded_extensions.insert("zip".into());
        let pdf_source = FolderPinSource::PdfPage {
            pdf_rel: "book.pdf".into(),
            page: 2,
        };
        let hidden_source = FolderPinSource::ZipEntry {
            zip_rel: "book.cbz".into(),
            entry: "inner.zip/page.png".into(),
        };
        let db = env.pin_db.as_ref().unwrap();
        db.set(&grandchild, &pdf_source).unwrap();
        db.set(&child, &hidden_source).unwrap();
        let pdf_target = resolve_pin_target_cascaded(
            &grandchild,
            &pdf_source,
            Some(db),
            env.folder_thumb_depth as usize,
        )
        .unwrap();
        let hidden_target = resolve_pin_target_cascaded(
            &child,
            &hidden_source,
            Some(db),
            env.folder_thumb_depth as usize,
        )
        .unwrap();
        let cache_dir = crate::catalog::default_cache_dir();
        let grandchild_base = crate::thumb_loader::folder_thumb_cache_key_for_path(
            &grandchild,
            false,
            env.folder_thumb_sort,
            env.folder_thumb_depth,
            P::Seeded,
        )
        .unwrap();
        let grandchild_key = crate::thumb_loader::representative_cache_key_for_policy(
            &crate::thumb_loader::pinned_folder_row_key(
                &grandchild_base,
                &pdf_target.source_id,
                None,
            ),
            &env.visibility,
        );
        let child_catalog = crate::catalog::CatalogDb::open(&cache_dir, &child).unwrap();
        let webp = webp();
        child_catalog
            .save_thumb_bytes(
                &grandchild_key,
                pdf_target.mtime,
                pdf_target.file_size,
                Some((8, 8)),
                &webp,
            )
            .unwrap();
        let automatic = LoadRequest {
            path: child.clone(),
            folder_thumb_sort: Some(env.folder_thumb_sort),
            folder_thumb_depth: env.folder_thumb_depth,
            folder_thumb_provenance: Some(P::AutoSelected),
            file_type_visibility: env.visibility.clone(),
            cache_key_override: Some(
                crate::thumb_loader::folder_thumb_cache_key_for_path(
                    &child,
                    false,
                    env.folder_thumb_sort,
                    env.folder_thumb_depth,
                    P::AutoSelected,
                )
                .unwrap(),
            ),
            ..Default::default()
        };
        let fallback =
            crate::thumb_loader::resolve_read_after_cache_miss(&automatic, Some(db), None);
        assert!(matches!(fallback.location, ReadLocation::ChildCached));
        let fallback_proof = fallback.folder_selection_proof.unwrap();
        assert_eq!(fallback_proof.winner.path, pdf);
        let child_base = crate::thumb_loader::folder_thumb_cache_key_for_path(
            &child,
            false,
            env.folder_thumb_sort,
            env.folder_thumb_depth,
            P::Seeded,
        )
        .unwrap();
        let child_key = crate::thumb_loader::representative_cache_key_for_policy(
            &crate::thumb_loader::pinned_folder_row_key(
                &child_base,
                &hidden_target.source_id,
                None,
            ),
            &env.visibility,
        );
        let parent_catalog = crate::catalog::CatalogDb::open(&cache_dir, &root).unwrap();
        parent_catalog
            .save_with_folder_proof(
                &child_key,
                hidden_target.mtime,
                hidden_target.file_size,
                8,
                8,
                Some((8, 8)),
                Some((8, 8)),
                &webp,
                Some(P::AutoSelected),
                Some(&fallback_proof),
            )
            .unwrap();
        let before = parent_catalog.load_one(&child_key).unwrap().unwrap();
        let parent_request = LoadRequest {
            path: root.clone(),
            ..automatic.clone()
        };
        let parent_selection =
            crate::thumb_loader::resolve_read_after_cache_miss(&parent_request, Some(db), None);
        assert!(matches!(
            parent_selection.location,
            ReadLocation::ChildCached
        ));
        let selection = parent_selection.child_selection.unwrap();
        assert_eq!(selection.container, child);
        assert_eq!(selection.source_path, pdf);
        assert!(selection.pin.is_none());
        with_pdf(
            PdfFixture {
                path: pdf.clone(),
                password: None,
                pages: 3,
                source: None,
            },
            || {
                let cover = readable(resolve_book_cover(&root, BookKind::Folder, &env));
                assert_eq!(cover.cover_ref, "B.pdf/C/book.pdf#page=2");
                assert_eq!(cover.signature_path, pdf);
                // Reconstruct two cached owners at depth 1, then exercise a
                // cached back-edge in the same traversal (A -> B -> C -> A).
                let mut resolver = CoverResolver {
                    env: &env,
                    video_db: None,
                    visited: HashSet::new(),
                };
                assert!(resolver.resolve(&root, BookKind::Folder, None).is_ok());
                for owner in [&root, &child, &grandchild] {
                    assert!(
                        resolver
                            .visited
                            .contains(&crate::path_key::normalize_keep_drive(owner))
                    );
                }
                assert!(matches!(
                    resolver.resolve(&root, BookKind::Folder, None),
                    Err(CoverError::Failure(BookCoverFailure::ReadFailed))
                ));
            },
        );
        let after = parent_catalog.load_one(&child_key).unwrap().unwrap();
        // Book resolution leaves the thumbnail bytes, selection proof, stamp,
        // and released AutoSelected persistence format untouched.
        assert_eq!(after.jpeg_data, before.jpeg_data);
        assert_eq!(after.selection_proof, before.selection_proof);
        assert_eq!(after.folder_provenance, before.folder_provenance);
        assert_eq!(
            (after.mtime, after.file_size),
            (before.mtime, before.file_size)
        );
    }

    #[test]
    fn book_cover_pin_cycles_and_cached_chain_revisits_stop() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("A");
        std::fs::create_dir_all(root.join("B")).unwrap();
        let env = pin_env(&tmp);
        // An empty pinned descendant terminates, and revisiting a previously
        // selected owner is rejected before another selection can begin.
        env.pin_db
            .as_ref()
            .unwrap()
            .set(
                &root,
                &FolderPinSource::File {
                    rel: "B".into(),
                    kind: FileKind::Folder,
                },
            )
            .unwrap();
        assert!(matches!(
            resolve_book_cover(&root, BookKind::Folder, &env),
            BookCover::Unreadable(BookCoverFailure::ReadFailed)
        ));
        let mut resolver = CoverResolver {
            env: &env,
            video_db: None,
            visited: HashSet::from([crate::path_key::normalize_keep_drive(&root)]),
        };
        assert!(matches!(
            resolver.resolve(&root, BookKind::Folder, None),
            Err(CoverError::Failure(BookCoverFailure::ReadFailed))
        ));
        // A -> B -> A is rejected before stat or selection, even when a
        // corrupt cached back-edge names an owner that no longer exists.
        let mut resolver = CoverResolver {
            env: &env,
            video_db: None,
            visited: HashSet::from([
                crate::path_key::normalize_keep_drive(&root),
                crate::path_key::normalize_keep_drive(&root.join("B")),
            ]),
        };
        std::fs::remove_dir(root.join("B")).unwrap();
        std::fs::remove_dir(&root).unwrap();
        assert!(matches!(
            resolver.resolve(&root, BookKind::Folder, None),
            Err(CoverError::Failure(BookCoverFailure::ReadFailed))
        ));
    }

    #[test]
    fn book_cover_video_frame_retains_pts_and_video_signature_and_dead_pin_falls_back() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("book");
        std::fs::create_dir_all(&root).unwrap();
        let video = root.join("clip.mp4");
        std::fs::write(&video, b"video source").unwrap();
        std::fs::write(root.join("auto.jpg"), b"auto").unwrap();
        let db = crate::video_pins::VideoPinDb::open_at(&tmp.path().join("video.db")).unwrap();
        db.set_pin(&video, 12.5, &webp()).unwrap();
        let mut env = pin_env(&tmp);
        env.video_pins = db.read_source();
        env.pin_db
            .as_ref()
            .unwrap()
            .set(
                &root,
                &FolderPinSource::File {
                    rel: "clip.mp4".into(),
                    kind: FileKind::Video,
                },
            )
            .unwrap();
        match resolve_book_cover(&root, BookKind::Folder, &env) {
            BookCover::VideoFrame {
                cover_ref,
                signature_path,
                signature,
            } => {
                assert_eq!(cover_ref, "clip.mp4#frame=12.5");
                assert_eq!(signature_path, video);
                assert_eq!(signature, file_signature(&video).unwrap());
            }
            other => panic!("expected frame, got {other:?}"),
        }
        env.video_pins = None;
        assert_eq!(
            readable(resolve_book_cover(&root, BookKind::Folder, &env)).cover_ref,
            "auto.jpg"
        );
    }

    #[test]
    fn book_cover_hidden_fixed_archive_entry_uses_the_same_automatic_projection() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("book.zip");
        write_zip(&path, &["hidden.png", "visible.jpg"]);
        let mut env = pin_env(&tmp);
        env.visibility.excluded_extensions.insert("png".into());
        env.pin_db
            .as_ref()
            .unwrap()
            .set(
                &path,
                &FolderPinSource::ZipEntry {
                    zip_rel: String::new(),
                    entry: "hidden.png".into(),
                },
            )
            .unwrap();
        assert_eq!(
            readable(resolve_book_cover(&path, BookKind::Zip, &env)).cover_ref,
            "#entry=visible.jpg"
        );
    }

    #[test]
    fn book_cover_folder_pin_resolves_converted_archive_dependencies_on_the_worker() {
        let _data = crate::data_dir::TestDataDirGuard::new();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("book");
        std::fs::create_dir_all(&root).unwrap();
        let original = root.join("child.7z");
        let converted = tmp.path().join("converted.zip");
        std::fs::write(&original, b"original sevenz").unwrap();
        write_zip(&converted, &["normalized/actual.jpg"]);
        let metadata = std::fs::metadata(&original).unwrap();
        let cache = Arc::new(crate::archive_cache::ArchiveCacheDb::open().unwrap());
        cache
            .record(
                &original,
                crate::ui_helpers::mtime_secs(&metadata),
                metadata.len() as i64,
                ArchiveFormat::SevenZ,
                &converted,
                std::fs::metadata(&converted).unwrap().len() as i64,
                1,
                false,
            )
            .unwrap();
        let mut env = pin_env(&tmp);
        env.archive_cache = Some(cache);
        env.pin_db
            .as_ref()
            .unwrap()
            .set(
                &root,
                &FolderPinSource::File {
                    rel: "child.7z".into(),
                    kind: FileKind::ConvertibleArchive,
                },
            )
            .unwrap();
        let cover = readable(resolve_book_cover(&root, BookKind::Folder, &env));
        assert_eq!(cover.cover_ref, "child.7z#entry=normalized/actual.jpg");
        assert_eq!(cover.signature_path, original);
        assert_eq!(
            cover.signature,
            FileSignature::from_metadata(&metadata).unwrap()
        );
    }
}
