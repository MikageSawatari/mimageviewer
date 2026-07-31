use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex, RwLock, mpsc};

use mimageviewer_ipc::{
    ContainerEntry, ContainerEntryKind, ContainerKind, ContainerPayload, ContainerRequest,
    ContainerResponse, MediaError, MediaErrorCode, PagePayload, PageRequest, PageResponse,
    RemoteAddress, RemoteSubresource, ThumbnailError, ThumbnailErrorCode, ThumbnailResponse,
};

use super::path_guard::{ResolveError, ResolvedFavoritePath, resolve_existing};
use super::thumbnail::WorkerContext;

const CONTAINER_ENTRY_LIMIT: usize = 1000;
const MAX_PAGE_RENDER_PX: u32 = crate::pdf_loader::PDF_RENDER_MAX_LONG_PX;
const PAGE_WEBP_QUALITY: f32 = 90.0;

pub(super) struct ContainerEngine {
    settings: Arc<crate::settings::Settings>,
    stats: Arc<Mutex<crate::stats::ThumbStats>>,
    pdf_passwords: crate::pdf_passwords::PdfPasswordStore,
    pdf_page_counts: Mutex<HashMap<PdfIdentity, u32>>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct PdfIdentity {
    path: std::path::PathBuf,
    mtime: i64,
    file_size: u64,
}

struct LoadedImage {
    image: image::DynamicImage,
}

impl ContainerEngine {
    pub(super) fn new(settings: crate::settings::Settings) -> Self {
        Self {
            settings: Arc::new(settings),
            stats: Arc::new(Mutex::new(crate::stats::ThumbStats::new())),
            pdf_passwords: crate::pdf_passwords::PdfPasswordStore::load(),
            pdf_page_counts: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn container(&self, request: ContainerRequest) -> ContainerResponse {
        let resolved = match self.resolve(&request.address) {
            Ok(resolved) => resolved,
            Err(error) => return ContainerResponse::Error(error),
        };
        match self.enumerate(&request.address, &resolved) {
            Ok(payload) => ContainerResponse::Success(payload),
            Err(error) => ContainerResponse::Error(error),
        }
    }

    pub(super) fn thumbnail(
        &self,
        request: &mimageviewer_ipc::ThumbnailRequest,
        context: &WorkerContext,
    ) -> ThumbnailResponse {
        let resolved = match self.resolve(&request.address) {
            Ok(resolved) => resolved,
            Err(error) => return thumbnail_error_from_media(error),
        };
        match self.load_image(
            &request.address,
            &resolved,
            request.target_px,
            false,
            context,
        ) {
            Ok(loaded) => crate::catalog::encode_thumb_webp(
                &loaded.image,
                request.target_px,
                self.settings.thumb_quality as f32,
            )
            .map(|(webp_bytes, _, _)| ThumbnailResponse::Success { webp_bytes })
            .unwrap_or_else(|| {
                thumbnail_error(
                    ThumbnailErrorCode::GenerationFailed,
                    "WebP エンコードに失敗しました",
                )
            }),
            Err(error) => thumbnail_error_from_media(error),
        }
    }

    pub(super) fn page(&self, request: PageRequest, context: &WorkerContext) -> PageResponse {
        if request.target_px == 0 || request.target_px > MAX_PAGE_RENDER_PX {
            return PageResponse::Error(media_error(
                MediaErrorCode::BadRequest,
                "画像サイズが範囲外です",
            ));
        }
        let resolved = match self.resolve(&request.address) {
            Ok(resolved) => resolved,
            Err(error) => return PageResponse::Error(error),
        };
        match self.load_image(
            &request.address,
            &resolved,
            request.target_px,
            true,
            context,
        ) {
            Ok(loaded) => match crate::catalog::encode_thumb_webp(
                &loaded.image,
                request.target_px,
                PAGE_WEBP_QUALITY,
            ) {
                Some((bytes, width, height)) => PageResponse::Success(PagePayload {
                    bytes,
                    content_type: "image/webp".to_owned(),
                    width,
                    height,
                }),
                None => PageResponse::Error(media_error(
                    MediaErrorCode::RenderFailed,
                    "WebP エンコードに失敗しました",
                )),
            },
            Err(error) => PageResponse::Error(error),
        }
    }

    fn resolve(&self, address: &RemoteAddress) -> Result<ResolvedFavoritePath, MediaError> {
        address
            .validate_syntax()
            .map_err(|_| media_error(MediaErrorCode::BadRequest, "コンテンツアドレスが不正です"))?;
        resolve_existing(
            &self.settings.favorites,
            &address.favorite_id,
            &address.relative_path,
        )
        .map_err(resolve_media_error)
    }

    fn enumerate(
        &self,
        address: &RemoteAddress,
        resolved: &ResolvedFavoritePath,
    ) -> Result<ContainerPayload, MediaError> {
        let metadata = std::fs::metadata(&resolved.canonical)
            .map_err(|_| media_error(MediaErrorCode::NotFound, "コンテナが見つかりません"))?;
        if !metadata.is_file() {
            return Err(media_error(
                MediaErrorCode::Unsupported,
                "対象は ZIP/PDF ファイルではありません",
            ));
        }
        if is_zip_path(&resolved.logical) {
            self.enumerate_zip(address, resolved)
        } else if is_pdf_path(&resolved.logical) {
            self.enumerate_pdf(address, resolved, &metadata)
        } else {
            Err(media_error(
                MediaErrorCode::Unsupported,
                "このコンテナ形式には対応していません",
            ))
        }
    }

    fn enumerate_zip(
        &self,
        address: &RemoteAddress,
        resolved: &ResolvedFavoritePath,
    ) -> Result<ContainerPayload, MediaError> {
        let requested_prefix = match &address.subresource {
            RemoteSubresource::File => String::new(),
            RemoteSubresource::ZipDirectory { prefix } => prefix.clone(),
            _ => {
                return Err(media_error(
                    MediaErrorCode::BadRequest,
                    "ZIP の一覧アドレスが不正です",
                ));
            }
        };
        let enumeration = crate::zip_loader::enumerate_image_entries_detailed(&resolved.logical)
            .map_err(|error| {
                crate::logger::log(format!("remote_ipc: zip_enumerate_failed error={error}"));
                media_error(MediaErrorCode::RenderFailed, "ZIP を列挙できませんでした")
            })?;
        let safe_entries = enumeration
            .entries
            .into_iter()
            .filter(|entry| {
                let safe = zip_entry_address(address, &entry.entry_name)
                    .validate_syntax()
                    .is_ok();
                if !safe {
                    crate::logger::log(
                        "remote_ipc: rejected unsafe ZIP entry during enumeration".to_owned(),
                    );
                }
                safe
            })
            .collect();
        let tree = crate::zip_tree::ZipTree::build(resolved.logical.clone(), safe_entries);
        let requested_segments = zip_prefix_segments(&requested_prefix);
        if tree.node_at(&requested_segments).is_none() {
            return Err(media_error(
                MediaErrorCode::NotFound,
                "ZIP 内の場所が見つかりません",
            ));
        }
        let effective_segments = tree.collapse_redundant(&requested_segments);
        let effective_prefix = zip_prefix(&effective_segments);
        let (items, _) =
            tree.materialize_level(&effective_segments, crate::app::BOOK_READING_PAGE_ORDER);
        let total = items.len();
        let entries = items
            .into_iter()
            .take(CONTAINER_ENTRY_LIMIT)
            .filter_map(|item| {
                let name = item.name().into_owned();
                match item {
                    crate::grid_item::GridItem::ZipDir { dir_prefix, .. } => Some(ContainerEntry {
                        name,
                        page_count: tree.page_count_for_prefix_str(&dir_prefix),
                        address: RemoteAddress {
                            favorite_id: address.favorite_id.clone(),
                            relative_path: address.relative_path.clone(),
                            subresource: RemoteSubresource::ZipDirectory { prefix: dir_prefix },
                        },
                        kind: ContainerEntryKind::Directory,
                    }),
                    crate::grid_item::GridItem::ZipImage { entry_name, .. } => {
                        Some(ContainerEntry {
                            name,
                            page_count: None,
                            address: zip_entry_address(address, &entry_name),
                            kind: ContainerEntryKind::Image,
                        })
                    }
                    _ => None,
                }
            })
            .collect();
        Ok(ContainerPayload {
            title: container_title(&resolved.logical),
            kind: ContainerKind::Zip,
            effective_address: RemoteAddress {
                favorite_id: address.favorite_id.clone(),
                relative_path: address.relative_path.clone(),
                subresource: if effective_prefix.is_empty() {
                    RemoteSubresource::File
                } else {
                    RemoteSubresource::ZipDirectory {
                        prefix: effective_prefix,
                    }
                },
            },
            entries,
            entry_limit: CONTAINER_ENTRY_LIMIT,
            truncated: total > CONTAINER_ENTRY_LIMIT,
        })
    }

    fn enumerate_pdf(
        &self,
        address: &RemoteAddress,
        resolved: &ResolvedFavoritePath,
        metadata: &std::fs::Metadata,
    ) -> Result<ContainerPayload, MediaError> {
        if !matches!(address.subresource, RemoteSubresource::File) {
            return Err(media_error(
                MediaErrorCode::BadRequest,
                "PDF の一覧アドレスが不正です",
            ));
        }
        let page_count = self.pdf_page_count(resolved, metadata)?;
        let entries = (0..page_count)
            .take(CONTAINER_ENTRY_LIMIT)
            .map(|page_number| ContainerEntry {
                address: RemoteAddress {
                    favorite_id: address.favorite_id.clone(),
                    relative_path: address.relative_path.clone(),
                    subresource: RemoteSubresource::PdfPage { page_number },
                },
                name: format!("Page {}", page_number + 1),
                kind: ContainerEntryKind::Image,
                page_count: None,
            })
            .collect();
        Ok(ContainerPayload {
            title: container_title(&resolved.logical),
            kind: ContainerKind::Pdf,
            effective_address: address.clone(),
            entries,
            entry_limit: CONTAINER_ENTRY_LIMIT,
            truncated: page_count as usize > CONTAINER_ENTRY_LIMIT,
        })
    }

    fn load_image(
        &self,
        address: &RemoteAddress,
        resolved: &ResolvedFavoritePath,
        target_px: u32,
        full_page: bool,
        context: &WorkerContext,
    ) -> Result<LoadedImage, MediaError> {
        if target_px == 0 || target_px > MAX_PAGE_RENDER_PX {
            return Err(media_error(
                MediaErrorCode::BadRequest,
                "画像サイズが範囲外です",
            ));
        }
        let metadata = std::fs::metadata(&resolved.canonical)
            .map_err(|_| media_error(MediaErrorCode::NotFound, "コンテナが見つかりません"))?;
        if !metadata.is_file() {
            return Err(media_error(
                MediaErrorCode::Unsupported,
                "対象はコンテナファイルではありません",
            ));
        }
        let mtime = crate::ui_helpers::mtime_secs(&metadata);
        let file_size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
        let mut request = crate::thumb_loader::LoadRequest {
            path: resolved.logical.clone(),
            mtime,
            file_size,
            skip_cache: full_page,
            // remote は PDF pool の Normal lane を使い、Critical 予約枠を消費しない。
            priority: false,
            context_epoch: 0,
            ..Default::default()
        };
        let catalog_folder = match &address.subresource {
            RemoteSubresource::File if is_zip_path(&resolved.logical) => {
                request.cache_key_override = Some(container_thumb_key(
                    crate::thumb_loader::CACHE_KEY_ZIP,
                    &resolved.logical,
                )?);
                resolved.logical.parent().ok_or_else(|| {
                    media_error(MediaErrorCode::PathRejected, "親フォルダを解決できません")
                })?
            }
            RemoteSubresource::File if is_pdf_path(&resolved.logical) => {
                self.ensure_pdf_page_in_range(resolved, &metadata, 0)?;
                request.pdf_page = Some(0);
                request.pdf_password = self.pdf_passwords.get(&resolved.logical);
                request.cache_key_override = Some(container_thumb_key(
                    crate::thumb_loader::CACHE_KEY_PDF,
                    &resolved.logical,
                )?);
                resolved.logical.parent().ok_or_else(|| {
                    media_error(MediaErrorCode::PathRejected, "親フォルダを解決できません")
                })?
            }
            RemoteSubresource::ZipEntry { entry_name } if is_zip_path(&resolved.logical) => {
                request.zip_entry = Some(entry_name.clone());
                &resolved.logical
            }
            RemoteSubresource::ZipDirectory { prefix } if is_zip_path(&resolved.logical) => {
                request.zip_dir_prefix = Some(prefix.clone());
                request.cache_key_override = Some(crate::grid_item::zipdir_cache_key(prefix));
                request.folder_thumb_sort = Some(crate::app::BOOK_READING_PAGE_ORDER);
                &resolved.logical
            }
            RemoteSubresource::PdfPage { page_number } if is_pdf_path(&resolved.logical) => {
                self.ensure_pdf_page_in_range(resolved, &metadata, *page_number)?;
                request.pdf_page = Some(*page_number);
                request.pdf_password = self.pdf_passwords.get(&resolved.logical);
                &resolved.logical
            }
            RemoteSubresource::File => {
                return Err(media_error(
                    MediaErrorCode::Unsupported,
                    "対象は ZIP/PDF ではありません",
                ));
            }
            _ => {
                return Err(media_error(
                    MediaErrorCode::BadRequest,
                    "コンテナ種別と内部アドレスが一致しません",
                ));
            }
        };

        let catalog = Arc::new(
            crate::catalog::CatalogDb::open(&crate::catalog::default_cache_dir(), catalog_folder)
                .map_err(|error| {
                crate::logger::log(format!(
                    "remote_ipc: container catalog open failed: {error}"
                ));
                media_error(
                    MediaErrorCode::Internal,
                    "サムネイルカタログを開けませんでした",
                )
            })?,
        );
        let cache_map = Arc::new(RwLock::new(HashMap::new()));
        if !full_page
            && let Some(key) = crate::thumb_loader::cache_key_for_request(&request)
            && let Ok(Some(entry)) = catalog.load_one(key.as_ref())
            && let Ok(mut map) = cache_map.write()
        {
            map.insert(key.into_owned(), entry);
        }

        let (tx, rx) = mpsc::channel();
        let done = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let keep_start = Arc::new(AtomicUsize::new(0));
        let keep_end = Arc::new(AtomicUsize::new(usize::MAX));
        crate::thumb_loader::process_load_request(
            &request,
            &cache_map,
            &tx,
            Some(&catalog),
            self.settings.thumb_px,
            self.settings.thumb_quality,
            target_px,
            crate::thumb_loader::CacheDecision::from_settings(&self.settings),
            &done,
            &self.stats,
            Some(&cancel),
            &keep_start,
            &keep_end,
            context.folder_pin_db.as_ref(),
            None,
            context.adjustment_db.as_ref(),
        );
        drop(tx);
        let color_image = rx
            .into_iter()
            .find_map(|message| (!message.finalized && !message.canceled).then_some(message.image))
            .flatten()
            .ok_or_else(|| {
                if matches!(address.subresource, RemoteSubresource::ZipDirectory { .. }) {
                    media_error(
                        MediaErrorCode::NotFound,
                        "ZIP 内に代表サムネイルが見つかりません",
                    )
                } else {
                    media_error(
                        MediaErrorCode::RenderFailed,
                        "mIV 本体でページをレンダリングできませんでした",
                    )
                }
            })?;
        let width = u32::try_from(color_image.size[0])
            .map_err(|_| media_error(MediaErrorCode::RenderFailed, "画像寸法が範囲外です"))?;
        let height = u32::try_from(color_image.size[1])
            .map_err(|_| media_error(MediaErrorCode::RenderFailed, "画像寸法が範囲外です"))?;
        let rgba = crate::capture::color_image_to_rgba(&color_image);
        let image = image::RgbaImage::from_raw(width, height, rgba)
            .map(image::DynamicImage::ImageRgba8)
            .ok_or_else(|| {
                media_error(
                    MediaErrorCode::RenderFailed,
                    "レンダリング結果を画像へ変換できませんでした",
                )
            })?;
        Ok(LoadedImage { image })
    }

    fn ensure_pdf_page_in_range(
        &self,
        resolved: &ResolvedFavoritePath,
        metadata: &std::fs::Metadata,
        page_number: u32,
    ) -> Result<(), MediaError> {
        validate_page_number(page_number, self.pdf_page_count(resolved, metadata)?)
    }

    /// 本体の PDF 一覧と同じ `pdf_meta` を先に引き、miss 時だけ PDFium で列挙する。
    /// `container_page_meta` は ZIP / folder / converted archive 用であり、PDF は
    /// password_required も保持する専用テーブルが正本になる。
    fn pdf_page_count(
        &self,
        resolved: &ResolvedFavoritePath,
        metadata: &std::fs::Metadata,
    ) -> Result<u32, MediaError> {
        let identity = PdfIdentity {
            path: resolved.canonical.clone(),
            mtime: crate::ui_helpers::mtime_secs(metadata),
            file_size: metadata.len(),
        };
        let cached = self
            .pdf_page_counts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&identity)
            .copied();
        let count = match cached {
            Some(count) => count,
            None => {
                let password = self.pdf_passwords.get(&resolved.logical);
                let catalog = open_parent_catalog(&resolved.logical);
                let filename = resolved.logical.file_name().and_then(|name| name.to_str());
                let persistent = catalog
                    .as_ref()
                    .zip(filename)
                    .and_then(|(catalog, filename)| {
                        catalog
                            .get_pdf_meta(
                                filename,
                                identity.mtime,
                                i64::try_from(identity.file_size).unwrap_or(i64::MAX),
                            )
                            .map_err(|error| {
                                crate::logger::log(format!(
                                    "remote_ipc: pdf meta lookup failed: {error}"
                                ));
                            })
                            .ok()
                            .flatten()
                    });
                let count = match persistent {
                    Some((_, true)) if password.is_none() => {
                        return Err(media_error(
                            MediaErrorCode::PasswordRequired,
                            "この PDF はパスワード保護されているため Web から開けません",
                        ));
                    }
                    Some((count, _)) if count > 0 => count,
                    _ => {
                        let pages = crate::pdf_loader::enumerate_pages(
                            &resolved.logical,
                            password.as_deref(),
                        )
                        .map_err(pdf_error)?;
                        let count = u32::try_from(pages.len()).unwrap_or(u32::MAX);
                        if let (Some(catalog), Some(filename)) = (catalog.as_ref(), filename) {
                            let write_result = if password.is_none() {
                                catalog.set_pdf_meta_safe(
                                    filename,
                                    identity.mtime,
                                    i64::try_from(identity.file_size).unwrap_or(i64::MAX),
                                    count,
                                )
                            } else {
                                catalog.set_pdf_meta_thumb(
                                    filename,
                                    identity.mtime,
                                    i64::try_from(identity.file_size).unwrap_or(i64::MAX),
                                    count,
                                )
                            };
                            if let Err(error) = write_result {
                                crate::logger::log(format!(
                                    "remote_ipc: pdf meta update failed: {error}"
                                ));
                            }
                        }
                        count
                    }
                };
                self.pdf_page_counts
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(identity, count);
                count
            }
        };
        Ok(count)
    }
}

fn open_parent_catalog(path: &Path) -> Option<crate::catalog::CatalogDb> {
    let parent = path.parent()?;
    crate::catalog::CatalogDb::open(&crate::catalog::default_cache_dir(), parent)
        .map_err(|error| {
            crate::logger::log(format!(
                "remote_ipc: PDF parent catalog open failed: {error}"
            ));
        })
        .ok()
}

fn validate_page_number(page_number: u32, page_count: u32) -> Result<(), MediaError> {
    if page_number < page_count {
        Ok(())
    } else {
        Err(media_error(
            MediaErrorCode::PageOutOfRange,
            "PDF ページ番号が範囲外です",
        ))
    }
}

fn zip_entry_address(container: &RemoteAddress, entry_name: &str) -> RemoteAddress {
    RemoteAddress {
        favorite_id: container.favorite_id.clone(),
        relative_path: container.relative_path.clone(),
        subresource: RemoteSubresource::ZipEntry {
            entry_name: entry_name.to_owned(),
        },
    }
}

fn zip_prefix_segments(prefix: &str) -> Vec<String> {
    prefix
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect()
}

fn zip_prefix(segments: &[String]) -> String {
    if segments.is_empty() {
        String::new()
    } else {
        format!("{}/", segments.join("/"))
    }
}

fn container_title(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "コンテナ".to_owned())
}

fn container_thumb_key(prefix: &str, path: &Path) -> Result<String, MediaError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!("{prefix}{name}"))
        .ok_or_else(|| media_error(MediaErrorCode::Unsupported, "ファイル名を解釈できません"))
}

fn is_zip_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            crate::folder_tree::is_zip_extension(&extension.to_ascii_lowercase())
        })
}

fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

fn pdf_error(error: std::io::Error) -> MediaError {
    let message = error.to_string();
    if message.contains("Password") || message.contains("password") {
        media_error(
            MediaErrorCode::PasswordRequired,
            "この PDF はパスワード保護されているため Web から開けません",
        )
    } else {
        crate::logger::log(format!("remote_ipc: pdf operation failed: {message}"));
        media_error(MediaErrorCode::RenderFailed, "PDF を開けませんでした")
    }
}

fn resolve_media_error(error: ResolveError) -> MediaError {
    match error {
        ResolveError::InvalidFavoriteId | ResolveError::InvalidRelativePath => media_error(
            MediaErrorCode::BadRequest,
            "favorite_id または相対パスが不正です",
        ),
        ResolveError::FavoriteNotFound => media_error(
            MediaErrorCode::FavoriteNotFound,
            "お気に入りが登録されていません",
        ),
        ResolveError::EscapesFavorite => media_error(
            MediaErrorCode::PathRejected,
            "お気に入りの外へ出るパスは拒否されました",
        ),
        ResolveError::Unavailable => media_error(MediaErrorCode::NotFound, "対象が見つかりません"),
    }
}

fn media_error(code: MediaErrorCode, message: impl Into<String>) -> MediaError {
    MediaError::new(code, message)
}

fn thumbnail_error_from_media(error: MediaError) -> ThumbnailResponse {
    let code = match error.code {
        MediaErrorCode::BadRequest => ThumbnailErrorCode::BadRequest,
        MediaErrorCode::FavoriteNotFound => ThumbnailErrorCode::FavoriteNotFound,
        MediaErrorCode::PathRejected => ThumbnailErrorCode::PathRejected,
        MediaErrorCode::NotFound => ThumbnailErrorCode::NotFound,
        MediaErrorCode::Unsupported => ThumbnailErrorCode::Unsupported,
        MediaErrorCode::PasswordRequired => ThumbnailErrorCode::PasswordRequired,
        MediaErrorCode::PageOutOfRange => ThumbnailErrorCode::PageOutOfRange,
        MediaErrorCode::Busy => ThumbnailErrorCode::Busy,
        MediaErrorCode::RenderFailed => ThumbnailErrorCode::GenerationFailed,
        MediaErrorCode::Internal => ThumbnailErrorCode::Internal,
    };
    thumbnail_error(code, error.message)
}

fn thumbnail_error(code: ThumbnailErrorCode, message: impl Into<String>) -> ThumbnailResponse {
    ThumbnailResponse::Error(ThumbnailError::new(code, message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::FavoriteEntry;

    #[test]
    fn pdf_page_range_rejects_the_upper_bound() {
        assert!(validate_page_number(0, 1).is_ok());
        assert!(matches!(
            validate_page_number(1, 1),
            Err(MediaError {
                code: MediaErrorCode::PageOutOfRange,
                ..
            })
        ));
        assert!(validate_page_number(0, 0).is_err());
    }

    #[test]
    fn container_resolution_rejects_a_path_outside_favorites() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let outside = temp.path().join("outside.zip");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&outside, b"not a zip").unwrap();
        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let settings = crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        };
        let engine = ContainerEngine::new(settings);
        let address = RemoteAddress::file(favorite.id.to_string(), "../outside.zip");
        assert!(matches!(
            engine.resolve(&address),
            Err(MediaError {
                code: MediaErrorCode::BadRequest,
                ..
            })
        ));

        let safe_root = favorite.path.clone();
        std::fs::write(safe_root.join("book.zip"), b"not a zip").unwrap();
        let unsafe_entry = RemoteAddress {
            favorite_id: favorite.id.to_string(),
            relative_path: "book.zip".to_owned(),
            subresource: RemoteSubresource::ZipEntry {
                entry_name: "../secret.jpg".to_owned(),
            },
        };
        assert!(matches!(
            engine.resolve(&unsafe_entry),
            Err(MediaError {
                code: MediaErrorCode::BadRequest,
                ..
            })
        ));
    }

    #[test]
    fn zip_materialization_uses_book_filename_order() {
        let tree = crate::zip_tree::ZipTree::build(
            "book.zip".into(),
            vec![
                crate::zip_loader::ZipImageEntry {
                    entry_name: "10.jpg".to_owned(),
                    uncompressed_size: 1,
                    mtime: 0,
                },
                crate::zip_loader::ZipImageEntry {
                    entry_name: "2.jpg".to_owned(),
                    uncompressed_size: 1,
                    mtime: 0,
                },
            ],
        );
        let (items, _) = tree.materialize_level(&[], crate::app::BOOK_READING_PAGE_ORDER);
        let names = items
            .iter()
            .map(|item| item.name().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, ["2.jpg", "10.jpg"]);
    }
}
