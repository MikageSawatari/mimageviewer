use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Condvar, Mutex, RwLock, mpsc};

use mimageviewer_ipc::{
    RemoteAddress, RemoteSubresource, ThumbnailError, ThumbnailErrorCode, ThumbnailRequest,
    ThumbnailResponse,
};

use super::container::ContainerEngine;
use super::path_guard::{ResolveError, ResolvedPath, resolve_existing};

pub(super) struct ThumbnailEngine {
    settings: Arc<crate::settings::Settings>,
    stats: Arc<Mutex<crate::stats::ThumbStats>>,
    inflight: Mutex<HashMap<RequestKey, Arc<Flight>>>,
}

pub(super) struct WorkerContext {
    pub(super) folder_pin_db: Option<crate::folder_thumb_pins::FolderThumbPinDb>,
    video_pin_db: Option<crate::video_pins::VideoPinDb>,
    rotation_db: Option<crate::rotation_db::RotationDb>,
    pub(super) adjustment_db: Option<crate::adjustment_db::AdjustmentDb>,
    pub(super) mask_db: Option<crate::mask_db::MaskDb>,
    pub(super) local_adjust_db: Option<crate::local_adjust_db::LocalAdjustDb>,
    pub(super) conceal_db: Option<crate::conceal_db::ConcealDb>,
    pub(super) comic_db: Option<crate::comic_db::ComicDb>,
    pub(super) crop_db: Option<crate::export_crop::CropDb>,
}

impl WorkerContext {
    pub(super) fn open() -> Self {
        Self {
            folder_pin_db: crate::folder_thumb_pins::FolderThumbPinDb::open().ok(),
            video_pin_db: crate::video_pins::VideoPinDb::open().ok(),
            rotation_db: crate::rotation_db::RotationDb::open().ok(),
            adjustment_db: crate::adjustment_db::AdjustmentDb::open().ok(),
            mask_db: crate::mask_db::MaskDb::open_readonly().ok(),
            local_adjust_db: crate::local_adjust_db::LocalAdjustDb::open_readonly(
                &crate::local_adjust_db::LocalAdjustDb::db_path(),
            )
            .ok(),
            conceal_db: crate::conceal_db::ConcealDb::open_readonly(
                &crate::conceal_db::ConcealDb::db_path(),
            )
            .ok(),
            comic_db: crate::comic_db::ComicDb::open_readonly().ok(),
            crop_db: crate::export_crop::CropDb::open_readonly(
                &crate::export_crop::CropDb::db_path(),
            )
            .ok(),
        }
    }
}

#[derive(Clone, Eq)]
struct RequestKey {
    address: RemoteAddress,
    source_address: Option<RemoteAddress>,
    target_px: u32,
}

impl PartialEq for RequestKey {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address
            && self.source_address == other.source_address
            && self.target_px == other.target_px
    }
}

impl Hash for RequestKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.address.hash(state);
        self.source_address.hash(state);
        self.target_px.hash(state);
    }
}

struct Flight {
    result: Mutex<Option<ThumbnailResponse>>,
    ready: Condvar,
}

impl ThumbnailEngine {
    pub(super) fn new(settings: crate::settings::Settings) -> Self {
        Self {
            settings: Arc::new(settings),
            stats: Arc::new(Mutex::new(crate::stats::ThumbStats::new())),
            inflight: Mutex::new(HashMap::new()),
        }
    }

    /// 同一要求は 1 本だけ生成し、同時到着した要求はその結果を共有する。
    pub(super) fn handle(
        &self,
        request: ThumbnailRequest,
        context: &WorkerContext,
        container_engine: &ContainerEngine,
    ) -> ThumbnailResponse {
        let key = RequestKey {
            address: request.address.clone(),
            source_address: request.source_address.clone(),
            target_px: request.target_px,
        };
        let (flight, owner) = {
            let mut inflight = self
                .inflight
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(existing) = inflight.get(&key) {
                (Arc::clone(existing), false)
            } else {
                let flight = Arc::new(Flight {
                    result: Mutex::new(None),
                    ready: Condvar::new(),
                });
                inflight.insert(key.clone(), Arc::clone(&flight));
                (flight, true)
            }
        };

        if !owner {
            let mut result = flight
                .result
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            while result.is_none() {
                result = flight
                    .ready
                    .wait(result)
                    .unwrap_or_else(|error| error.into_inner());
            }
            return result.clone().expect("flight result checked above");
        }

        let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.generate(&request, context, container_engine)
        }))
        .unwrap_or_else(|_| {
            error_response(
                ThumbnailErrorCode::Internal,
                "サムネイル生成中に内部エラーが発生しました",
            )
        });
        {
            let mut result = flight
                .result
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            *result = Some(response.clone());
            flight.ready.notify_all();
        }
        self.inflight
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&key);
        response
    }

    fn generate(
        &self,
        request: &ThumbnailRequest,
        context: &WorkerContext,
        container_engine: &ContainerEngine,
    ) -> ThumbnailResponse {
        if request.target_px == 0 || request.target_px > 4096 {
            return error_response(
                ThumbnailErrorCode::BadRequest,
                "サムネイルサイズが範囲外です",
            );
        }
        if !matches!(request.address.subresource, RemoteSubresource::File)
            || is_container_path(Path::new(&request.address.path))
        {
            return container_engine.thumbnail(request, context);
        }
        let resolved = match resolve_existing(&request.address.path) {
            Ok(path) => path,
            Err(error) => return resolve_error_response(error),
        };
        match self.generate_resolved(
            &resolved,
            request.source_address.as_ref(),
            request.target_px,
            context,
        ) {
            Ok(webp_bytes) => ThumbnailResponse::Success { webp_bytes },
            Err(error) => error,
        }
    }

    fn generate_resolved(
        &self,
        resolved: &ResolvedPath,
        source_address: Option<&RemoteAddress>,
        target_px: u32,
        context: &WorkerContext,
    ) -> Result<Vec<u8>, ThumbnailResponse> {
        if is_supported_video(&resolved.canonical) {
            return self.generate_video_resolved(resolved, source_address, target_px, context);
        }
        if source_address.is_some() {
            return Err(error_response(
                ThumbnailErrorCode::BadRequest,
                "サムネイル出所は動画にだけ指定できます",
            ));
        }
        self.generate_catalog_resolved(resolved, target_px, context)
    }

    fn generate_catalog_resolved(
        &self,
        resolved: &ResolvedPath,
        target_px: u32,
        context: &WorkerContext,
    ) -> Result<Vec<u8>, ThumbnailResponse> {
        let metadata = std::fs::metadata(&resolved.canonical)
            .map_err(|_| error_response(ThumbnailErrorCode::NotFound, "対象が見つかりません"))?;
        let is_folder = metadata.is_dir();
        if !is_folder && !is_supported_image(&resolved.canonical) {
            return Err(error_response(
                ThumbnailErrorCode::Unsupported,
                "この種類のサムネイルは今回の増分では扱いません",
            ));
        }
        let parent = resolved.logical.parent().ok_or_else(|| {
            error_response(
                ThumbnailErrorCode::PathRejected,
                "閲覧起点自体のサムネイルは要求できません",
            )
        })?;
        let use_full_path_key = crate::path_key::is_drive_or_share_root(parent);
        let mtime = crate::ui_helpers::mtime_secs(&metadata);
        let file_size = if is_folder { 0 } else { metadata.len() as i64 };
        let cache_key = if is_folder {
            crate::thumb_loader::folder_thumb_auto_cache_key_for_path(
                &resolved.logical,
                use_full_path_key,
                self.settings.folder_thumb_sort,
                self.settings.folder_thumb_depth,
            )
        } else if use_full_path_key {
            Some(format!("imgthumb:{}", resolved.logical.to_string_lossy()))
        } else {
            resolved
                .logical
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        }
        .ok_or_else(|| {
            error_response(
                ThumbnailErrorCode::Unsupported,
                "ファイル名をサムネイルキーへ変換できません",
            )
        })?;

        let mut load_request = crate::thumb_loader::LoadRequest {
            // decode は検証済みの logical path を使う。catalog key、pin DB、回転 DB が
            // 本体 UI と同じ spelling になり、canonical path は境界判定専用に保つ。
            path: resolved.logical.clone(),
            mtime,
            file_size,
            priority: true,
            cache_key_override: Some(cache_key),
            folder_thumb_sort: is_folder.then_some(self.settings.folder_thumb_sort),
            folder_thumb_depth: self.settings.folder_thumb_depth,
            ..Default::default()
        };
        if is_folder {
            apply_supported_folder_pin(
                &mut load_request,
                &resolved.logical,
                context.folder_pin_db.as_ref(),
            );
        }

        let catalog = Arc::new(
            crate::catalog::CatalogDb::open(&crate::catalog::default_cache_dir(), parent).map_err(
                |error| {
                    crate::logger::log(format!("remote_ipc: catalog open failed: {error}"));
                    error_response(
                        ThumbnailErrorCode::Internal,
                        "サムネイルカタログを開けませんでした",
                    )
                },
            )?,
        );
        let cache_map = Arc::new(RwLock::new(HashMap::new()));
        if let Some(key) = crate::thumb_loader::cache_key_for_request(&load_request)
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
        let effective_target = target_px.min(self.settings.thumb_px.max(1));
        crate::thumb_loader::process_load_request(
            &load_request,
            &cache_map,
            &tx,
            Some(&catalog),
            self.settings.thumb_px,
            self.settings.thumb_quality,
            effective_target,
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
                if is_folder {
                    // process_load_request 内の本体共通 resolve_folder_thumb_image が
                    // None を返した結果。Web 独自探索ではなく、本体 UI と同じ条件で
                    // 代表画像が無いことを 404 として区別する。
                    error_response(
                        ThumbnailErrorCode::NotFound,
                        "フォルダ内に代表サムネイルが見つかりません",
                    )
                } else {
                    error_response(
                        ThumbnailErrorCode::GenerationFailed,
                        "mIV 本体でサムネイルを生成できませんでした",
                    )
                }
            })?;

        let mut image = color_image_to_dynamic(&color_image).ok_or_else(|| {
            error_response(
                ThumbnailErrorCode::GenerationFailed,
                "サムネイル画像を WebP へ変換できませんでした",
            )
        })?;
        // 本体 UI と同様、通常画像だけに手動回転 DB を適用する。フォルダ代表は
        // GridItem::Folder なので回転対象外。
        if !is_folder
            && let Some(rotation) = context
                .rotation_db
                .as_ref()
                .and_then(|database| database.get(&resolved.logical))
        {
            image = match rotation {
                crate::rotation_db::Rotation::None => image,
                crate::rotation_db::Rotation::Cw90 => image.rotate90(),
                crate::rotation_db::Rotation::Cw180 => image.rotate180(),
                crate::rotation_db::Rotation::Cw270 => image.rotate270(),
            };
        }
        crate::catalog::encode_thumb_webp(
            &image,
            effective_target,
            self.settings.thumb_quality as f32,
        )
        .map(|(bytes, _, _)| bytes)
        .ok_or_else(|| {
            error_response(
                ThumbnailErrorCode::GenerationFailed,
                "WebP エンコードに失敗しました",
            )
        })
    }

    fn generate_video_resolved(
        &self,
        resolved: &ResolvedPath,
        source_address: Option<&RemoteAddress>,
        target_px: u32,
        context: &WorkerContext,
    ) -> Result<Vec<u8>, ThumbnailResponse> {
        let metadata = std::fs::metadata(&resolved.canonical)
            .map_err(|_| error_response(ThumbnailErrorCode::NotFound, "対象が見つかりません"))?;
        if !metadata.is_file() {
            return Err(error_response(
                ThumbnailErrorCode::Unsupported,
                "動画ファイルではありません",
            ));
        }

        // Keep the desktop priority chain: user pin, selected sidecar, then Shell.
        // Pin/Shell results deliberately bypass CatalogDb; video cache ownership
        // remains with video_pins.db, Windows thumbcache, and HTTP's 60-second cache.
        if let Some(pin) = context
            .video_pin_db
            .as_ref()
            .and_then(|database| database.lookup(&resolved.logical))
            && !pin.thumb_webp.is_empty()
        {
            if crate::catalog::decode_thumb_to_color_image(&pin.thumb_webp).is_some() {
                return Ok(pin.thumb_webp);
            }
            crate::logger::log(format!(
                "remote_ipc: invalid video pin WebP path={}",
                resolved.logical.display()
            ));
        }

        if let Some(source_address) = source_address
            && let Some(sidecar) = self.resolve_video_sidecar(resolved, source_address)?
        {
            return self.generate_catalog_resolved(&sidecar, target_px, context);
        }

        let effective_target = target_px.min(self.settings.thumb_px.max(1));
        let (image, diag) =
            crate::video_thumb::get_video_thumbnail(&resolved.logical, effective_target as i32);
        let Some(image) = image else {
            crate::logger::log(format!(
                "remote_ipc: video shell FAIL path={} stage={} hr={} get_ms={}",
                resolved.logical.display(),
                diag.stage_label(),
                diag.hresult_hex(),
                diag.get_image_ms,
            ));
            return Err(if diag.extraction_may_be_pending() {
                error_response(
                    ThumbnailErrorCode::NotReady,
                    "Windows が動画サムネイルを抽出中です",
                )
            } else {
                error_response(
                    ThumbnailErrorCode::GenerationFailed,
                    "Windows Shell が動画サムネイルを生成できませんでした",
                )
            });
        };
        let image = color_image_to_dynamic(&image).ok_or_else(|| {
            error_response(
                ThumbnailErrorCode::GenerationFailed,
                "動画サムネイルを WebP へ変換できませんでした",
            )
        })?;
        crate::catalog::encode_thumb_webp(
            &image,
            effective_target,
            self.settings.thumb_quality as f32,
        )
        .map(|(bytes, _, _)| bytes)
        .ok_or_else(|| {
            error_response(
                ThumbnailErrorCode::GenerationFailed,
                "WebP エンコードに失敗しました",
            )
        })
    }

    fn resolve_video_sidecar(
        &self,
        video: &ResolvedPath,
        source_address: &RemoteAddress,
    ) -> Result<Option<ResolvedPath>, ThumbnailResponse> {
        if !self.settings.skip_image_if_video_exists || !self.settings.video_thumb_use_sidecar_image
        {
            return Ok(None);
        }
        source_address.validate_syntax().map_err(|error| {
            error_response(
                ThumbnailErrorCode::BadRequest,
                if error == mimageviewer_ipc::AddressError::NetworkPath {
                    mimageviewer_ipc::REMOTE_NETWORK_PATH_MESSAGE
                } else {
                    "サムネイル出所のアドレスが不正です"
                },
            )
        })?;
        if !matches!(source_address.subresource, RemoteSubresource::File) {
            return Err(error_response(
                ThumbnailErrorCode::BadRequest,
                "動画 sidecar は実ファイルでなければなりません",
            ));
        }
        if crate::path_key::normalize_keep_drive(Path::new(&source_address.path))
            == crate::path_key::normalize_keep_drive(&video.logical)
        {
            return Ok(None);
        }

        let sidecar = resolve_existing(&source_address.path).map_err(resolve_error_response)?;
        let same_parent = sidecar
            .canonical
            .parent()
            .zip(video.canonical.parent())
            .is_some_and(|(left, right)| {
                crate::path_key::normalize_keep_drive(left)
                    == crate::path_key::normalize_keep_drive(right)
            });
        let same_stem = file_stem_lower(&sidecar.canonical)
            .zip(file_stem_lower(&video.canonical))
            .is_some_and(|(left, right)| left == right);
        if !same_parent || !same_stem || !is_supported_image(&sidecar.canonical) {
            return Err(error_response(
                ThumbnailErrorCode::PathRejected,
                "動画と同じフォルダ・stem の sidecar だけを使用できます",
            ));
        }
        Ok(Some(sidecar))
    }
}

fn apply_supported_folder_pin(
    request: &mut crate::thumb_loader::LoadRequest,
    container: &Path,
    pin_db: Option<&crate::folder_thumb_pins::FolderThumbPinDb>,
) {
    let Some(pin_db) = pin_db else {
        return;
    };
    let Some(source) = pin_db.lookup(container) else {
        return;
    };
    let lookup = |path: &Path| pin_db.lookup(path);
    let Some(resolved) = crate::folder_thumb_pins::resolve_pin_target_cascaded_via(
        container,
        &source,
        lookup,
        request.folder_thumb_depth as usize,
    ) else {
        return;
    };
    use crate::folder_thumb_pins::ResolvedKind;
    use crate::thumb_loader::ResolveStrategy;
    let strategy = match resolved.kind {
        ResolvedKind::Image => ResolveStrategy::DirectImage,
        ResolvedKind::Folder => ResolveStrategy::FolderRepresentative,
        // ZIP / PDF / 動画は今回の縦串増分の明示的な非スコープ。
        _ => return,
    };
    let Some(base_key) = request.cache_key_override.as_deref() else {
        return;
    };
    request.cache_key_override = Some(format!(
        "{base_key}{}{}",
        crate::thumb_loader::CACHE_KEY_PIN_SUFFIX,
        resolved.source_id
    ));
    request.path = resolved.abs_path;
    request.mtime = resolved.mtime;
    request.file_size = resolved.file_size;
    request.resolve_override = Some(strategy);
}

fn color_image_to_dynamic(image: &egui::ColorImage) -> Option<image::DynamicImage> {
    let width = u32::try_from(image.size[0]).ok()?;
    let height = u32::try_from(image.size[1]).ok()?;
    let rgba = crate::capture::color_image_to_rgba(image);
    image::RgbaImage::from_raw(width, height, rgba).map(image::DynamicImage::ImageRgba8)
}

fn is_supported_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            crate::folder_tree::is_recognized_image_ext(&extension.to_ascii_lowercase())
        })
}

fn is_supported_video(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            let extension = extension.to_ascii_lowercase();
            crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&extension.as_str())
        })
}

fn file_stem_lower(path: &Path) -> Option<String> {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_lowercase())
}

fn is_container_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            let extension = extension.to_ascii_lowercase();
            extension == "pdf" || crate::folder_tree::is_zip_extension(&extension)
        })
}

fn resolve_error_response(error: ResolveError) -> ThumbnailResponse {
    match error {
        ResolveError::InvalidPath => {
            error_response(ThumbnailErrorCode::BadRequest, "絶対パスが不正です")
        }
        ResolveError::NetworkPath => error_response(
            ThumbnailErrorCode::BadRequest,
            mimageviewer_ipc::REMOTE_NETWORK_PATH_MESSAGE,
        ),
        ResolveError::Unavailable => {
            error_response(ThumbnailErrorCode::NotFound, "対象が見つかりません")
        }
    }
}

fn error_response(code: ThumbnailErrorCode, message: impl Into<String>) -> ThumbnailResponse {
    ThumbnailResponse::Error(ThumbnailError::new(code, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker_context_with_video_pin(
        video_pin_db: Option<crate::video_pins::VideoPinDb>,
    ) -> WorkerContext {
        WorkerContext {
            folder_pin_db: None,
            video_pin_db,
            rotation_db: None,
            adjustment_db: None,
            mask_db: None,
            local_adjust_db: None,
            conceal_db: None,
            comic_db: None,
            crop_db: None,
        }
    }

    #[test]
    fn identical_requests_share_one_flight() {
        let key = RequestKey {
            address: RemoteAddress::file("C:/Pictures/b.jpg"),
            source_address: None,
            target_px: 128,
        };
        let mut map = HashMap::new();
        let flight = Arc::new(Flight {
            result: Mutex::new(None),
            ready: Condvar::new(),
        });
        map.insert(key.clone(), Arc::clone(&flight));
        assert!(Arc::ptr_eq(map.get(&key).unwrap(), &flight));
    }

    #[test]
    fn video_pin_precedes_sidecar_and_shell() {
        let temp = tempfile::tempdir().unwrap();
        let video = temp.path().join("clip.mp4");
        let sidecar = temp.path().join("clip.jpg");
        std::fs::write(&video, b"not a real video").unwrap();
        std::fs::write(&sidecar, b"not a real image").unwrap();
        let pin_image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([240, 20, 30, 255]),
        ));
        let pin_webp = crate::catalog::encode_thumb_webp(&pin_image, 8, 80.0)
            .unwrap()
            .0;
        let pin_db =
            crate::video_pins::VideoPinDb::open_at(&temp.path().join("video_pins.db")).unwrap();
        pin_db.set_pin(&video, 2.0, &pin_webp).unwrap();
        let context = worker_context_with_video_pin(Some(pin_db));
        let engine = ThumbnailEngine::new(crate::settings::Settings::default());
        let resolved = resolve_existing(video.to_string_lossy().as_ref()).unwrap();

        let result = engine
            .generate_video_resolved(
                &resolved,
                Some(&RemoteAddress::file(sidecar.to_string_lossy().into_owned())),
                128,
                &context,
            )
            .unwrap();

        assert_eq!(result, pin_webp);
    }

    #[test]
    fn video_sidecar_must_share_parent_and_stem() {
        let temp = tempfile::tempdir().unwrap();
        let video_dir = temp.path().join("videos");
        let image_dir = temp.path().join("images");
        std::fs::create_dir_all(&video_dir).unwrap();
        std::fs::create_dir_all(&image_dir).unwrap();
        let video = video_dir.join("clip.mp4");
        let sidecar = image_dir.join("clip.jpg");
        std::fs::write(&video, b"video").unwrap();
        std::fs::write(&sidecar, b"image").unwrap();
        let engine = ThumbnailEngine::new(crate::settings::Settings::default());
        let resolved = resolve_existing(video.to_string_lossy().as_ref()).unwrap();

        let error = engine
            .resolve_video_sidecar(
                &resolved,
                &RemoteAddress::file(sidecar.to_string_lossy().into_owned()),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            ThumbnailResponse::Error(ThumbnailError {
                code: ThumbnailErrorCode::PathRejected,
                ..
            })
        ));
    }
}
