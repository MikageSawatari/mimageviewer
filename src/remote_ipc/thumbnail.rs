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
    io_sem: Arc<crate::io_semaphore::GlobalIoSemaphore>,
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

    /// `RemoteImageLoadKind::AutoTrimReference` 専用。**`open()` を呼ばないこと**が要点で、
    /// あれは SQLite を 9 個開くため、見開きページごとに走らせる値段ではない。
    ///
    /// 安全な理由: `load_image_timed` が `WorkerContext` を読むのは
    /// `prepare_remote_composite_timed` へ渡す 1 箇所だけで、そこは `compose_full_page`
    /// (= `RemoteImageLoadKind::composes_page()`) の内側にある。`AutoTrimReference` は
    /// それが false で、生 raster と bbox しか作らない。`decode_remote_source` も
    /// pin / adjustment DB を明示的に `None` で呼ぶ。
    ///
    /// 合成する load kind へこれを渡さないこと (補正・編集が黙って外れる)。
    pub(super) fn without_databases() -> Self {
        Self {
            folder_pin_db: None,
            video_pin_db: None,
            rotation_db: None,
            adjustment_db: None,
            mask_db: None,
            local_adjust_db: None,
            conceal_db: None,
            comic_db: None,
            crop_db: None,
        }
    }

    pub(super) fn rotation_for_remote_page(
        &self,
        logical_path: &Path,
        subresource: &RemoteSubresource,
    ) -> crate::rotation_db::Rotation {
        crate::edit_source::page_key_for_remote(logical_path, subresource)
            .and_then(|key| {
                self.rotation_db
                    .as_ref()
                    .and_then(|database| database.get_key(&key))
            })
            .unwrap_or(crate::rotation_db::Rotation::None)
    }
}

#[derive(Clone, Eq)]
struct RequestKey {
    address: RemoteAddress,
    source_address: Option<RemoteAddress>,
    target_px: u32,
    session: (String, String),
    admission: crate::catalog::CatalogAdmission,
}

impl PartialEq for RequestKey {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address
            && self.source_address == other.source_address
            && self.target_px == other.target_px
            && self.session == other.session
            && self.admission == other.admission
    }
}

impl Hash for RequestKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.address.hash(state);
        self.source_address.hash(state);
        self.target_px.hash(state);
        self.session.hash(state);
        self.admission.hash(state);
    }
}

struct Flight {
    result: Mutex<Option<ThumbnailResponse>>,
    ready: Condvar,
}

impl ThumbnailEngine {
    #[cfg(test)]
    pub(super) fn new(settings: crate::settings::Settings) -> Self {
        Self::new_with_io_sem(
            settings,
            Arc::new(crate::io_semaphore::GlobalIoSemaphore::new(1)),
        )
    }

    pub(super) fn new_with_io_sem(
        settings: crate::settings::Settings,
        io_sem: Arc<crate::io_semaphore::GlobalIoSemaphore>,
    ) -> Self {
        Self {
            io_sem,
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
        owner: &mimageviewer_ipc::RemoteSessionIdentity,
        cancellation: &super::session::RemoteOperationCancellation,
    ) -> ThumbnailResponse {
        let admission =
            crate::catalog::CatalogAccess::for_cache_dir(&crate::catalog::default_cache_dir())
                .admit();
        let key = RequestKey {
            address: request.address.clone(),
            source_address: request.source_address.clone(),
            target_px: request.target_px,
            session: (owner.client_id.clone(), owner.session_id.clone()),
            admission,
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
            self.generate(&request, context, container_engine, admission, cancellation)
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
        admission: crate::catalog::CatalogAdmission,
        cancellation: &super::session::RemoteOperationCancellation,
    ) -> ThumbnailResponse {
        if cancellation.is_cancelled() {
            return error_response(
                ThumbnailErrorCode::NotReady,
                "音声サムネイル要求は取り消されました",
            );
        }
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
        let settings = match container_engine.settings_for_listing() {
            Ok(settings) => settings,
            Err(_) => {
                return error_response(
                    ThumbnailErrorCode::Internal,
                    "現在の表示設定を取得できませんでした",
                );
            }
        };
        match self.generate_resolved(
            &resolved,
            request.source_address.as_ref(),
            request.target_px,
            context,
            &settings,
            admission,
            container_engine.raw_develop_executor(),
            cancellation,
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
        settings: &crate::settings::Settings,
        admission: crate::catalog::CatalogAdmission,
        raw_executor: &crate::raw::RawDevelopExecutor,
        cancellation: &super::session::RemoteOperationCancellation,
    ) -> Result<Vec<u8>, ThumbnailResponse> {
        if is_supported_audio(&resolved.canonical) {
            return self.generate_audio_resolved(
                resolved,
                source_address,
                target_px,
                context,
                settings,
                admission,
                raw_executor,
                cancellation,
            );
        }
        if is_supported_video(&resolved.canonical) {
            return self.generate_video_resolved_with_settings(
                resolved,
                source_address,
                target_px,
                context,
                settings,
                admission,
                &cancellation.flag(),
            );
        }
        if source_address.is_some() {
            return Err(error_response(
                ThumbnailErrorCode::BadRequest,
                "サムネイル出所は動画にだけ指定できます",
            ));
        }
        self.generate_catalog_resolved_admitted(
            resolved,
            target_px,
            context,
            admission,
            &cancellation.flag(),
        )
    }

    #[cfg(test)]
    fn generate_catalog_resolved(
        &self,
        resolved: &ResolvedPath,
        target_px: u32,
        context: &WorkerContext,
    ) -> Result<Vec<u8>, ThumbnailResponse> {
        let admission =
            crate::catalog::CatalogAccess::for_cache_dir(&crate::catalog::default_cache_dir())
                .admit();
        self.generate_catalog_resolved_admitted(
            resolved,
            target_px,
            context,
            admission,
            &Arc::new(AtomicBool::new(false)),
        )
    }

    fn generate_catalog_resolved_admitted(
        &self,
        resolved: &ResolvedPath,
        target_px: u32,
        context: &WorkerContext,
        admission: crate::catalog::CatalogAdmission,
        cancellation: &Arc<AtomicBool>,
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
            folder_thumb_provenance: is_folder
                .then_some(crate::catalog::FolderThumbProvenance::AutoSelected),
            ..Default::default()
        };
        if is_folder {
            apply_supported_folder_pin(
                &mut load_request,
                &resolved.logical,
                context.folder_pin_db.as_ref(),
            );
        }

        // Resolve a folder representative before the catalog lookup. Remote
        // accepts any usable RAW preview, and otherwise only reads the catalog.
        let raw_requirement = crate::thumb_loader::remote_raw_thumbnail_requirement(
            &load_request,
            context.folder_pin_db.as_ref(),
        );
        match raw_requirement {
            crate::thumb_loader::RemoteRawThumbRequirement::NotRaw => {}
            crate::thumb_loader::RemoteRawThumbRequirement::PreviewAvailable => {
                load_request.source_policy = crate::thumb_loader::LoadSourcePolicy::SourceOnly;
            }
            crate::thumb_loader::RemoteRawThumbRequirement::CatalogOnly => {
                load_request.source_policy = crate::thumb_loader::LoadSourcePolicy::CacheOnly;
            }
        }

        let cache_dir = crate::catalog::default_cache_dir();
        let access = crate::catalog::CatalogAccess::for_cache_dir(&cache_dir);
        let mut catalog = crate::catalog::CatalogDb::open_admitted(&cache_dir, parent, admission)
            .ok()
            .map(Arc::new);
        if matches!(
            load_request.source_policy,
            crate::thumb_loader::LoadSourcePolicy::CacheOnly
        ) && catalog.is_none()
        {
            let read_admission = access.wait_read_admission(cancellation).ok_or_else(|| {
                error_response(
                    ThumbnailErrorCode::NotReady,
                    "サムネイル要求は取り消されました",
                )
            })?;
            catalog = crate::catalog::CatalogDb::open_existing_read_only_admitted(
                &cache_dir,
                parent,
                read_admission,
            )
            .ok()
            .flatten()
            .map(Arc::new);
        }
        let cache_map = Arc::new(RwLock::new(HashMap::new()));
        if let Some(key) = crate::thumb_loader::cache_key_for_request(&load_request)
            && let Some(catalog) = &catalog
            && let Ok(Some(entry)) = catalog.load_one(key.as_ref())
            && let Ok(mut map) = cache_map.write()
        {
            map.insert(key.into_owned(), entry);
        }

        let (tx, rx) = mpsc::channel();
        let done = Arc::new(AtomicUsize::new(0));
        let keep_start = Arc::new(AtomicUsize::new(0));
        let keep_end = Arc::new(AtomicUsize::new(usize::MAX));
        let effective_target = target_px.min(self.settings.thumb_px.max(1));
        let (raw_unavailable_tx, raw_unavailable_rx) = mpsc::channel();
        let raw_handoff =
            crate::thumb_loader::RawThumbHandoff::RemotePreviewOnly(raw_unavailable_tx);
        let cache_decision = if matches!(
            raw_requirement,
            crate::thumb_loader::RemoteRawThumbRequirement::NotRaw
        ) {
            crate::thumb_loader::CacheDecision::from_settings(&self.settings)
        } else {
            // A small Remote preview must not replace a PC half-developed row.
            crate::thumb_loader::CacheDecision::without_thumbnail()
        };
        crate::thumb_loader::process_load_request(
            &mut load_request,
            &cache_map,
            &tx,
            catalog.as_ref(),
            self.settings.thumb_px,
            self.settings.thumb_quality,
            effective_target,
            cache_decision,
            &done,
            &self.stats,
            Some(cancellation),
            &keep_start,
            &keep_end,
            None,
            context.folder_pin_db.as_ref(),
            None,
            context.adjustment_db.as_ref(),
            Some(&raw_handoff),
        );
        if matches!(
            raw_unavailable_rx.try_recv(),
            Ok(crate::thumb_loader::RawThumbUnavailable::NeedsHalfDevelopment)
        ) {
            return Err(error_response(
                ThumbnailErrorCode::NoThumbnail,
                "RAW に使えるサムネイルがありません",
            ));
        }
        drop(tx);
        let color_image = rx
            .into_iter()
            .find_map(|message| match message.payload {
                crate::thumb_loader::ThumbMsgPayload::Pixels(pixels) => Some(pixels.image),
                _ => None,
            })
            .ok_or_else(|| {
                if matches!(
                    raw_requirement,
                    crate::thumb_loader::RemoteRawThumbRequirement::CatalogOnly
                        | crate::thumb_loader::RemoteRawThumbRequirement::PreviewAvailable
                ) {
                    error_response(
                        ThumbnailErrorCode::NoThumbnail,
                        "RAW に使えるサムネイルがありません",
                    )
                } else if is_folder {
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
        if !is_folder {
            let rotation =
                context.rotation_for_remote_page(&resolved.logical, &RemoteSubresource::File);
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

    fn generate_audio_resolved(
        &self,
        resolved: &ResolvedPath,
        source_address: Option<&RemoteAddress>,
        target_px: u32,
        context: &WorkerContext,
        settings: &crate::settings::Settings,
        admission: crate::catalog::CatalogAdmission,
        raw_executor: &crate::raw::RawDevelopExecutor,
        cancellation: &super::session::RemoteOperationCancellation,
    ) -> Result<Vec<u8>, ThumbnailResponse> {
        let cancel_flag = cancellation.flag();
        let _io_permit = self
            .io_sem
            .acquire_cancellable(crate::io_semaphore::IoPriority::Normal, &cancel_flag)
            .ok_or_else(|| {
                error_response(
                    ThumbnailErrorCode::NotReady,
                    "音声サムネイル要求は取り消されました",
                )
            })?;
        let sidecar = match source_address {
            Some(source) => self.resolve_media_sidecar(resolved, source, settings)?,
            None => None,
        };
        let original_stamp = thumbnail_source_stamp(&resolved.logical)?;
        // Keep successful sidecar pixels on the existing image catalog/RAW-preview
        // path. A failed decode needs source classification: its channel terminal
        // deliberately carries no error detail, so it alone cannot authorize fallback.
        if let Some(sidecar) = &sidecar {
            let sidecar_stamp = thumbnail_source_stamp(&sidecar.logical)?;
            let result = self.generate_catalog_resolved_admitted(
                sidecar,
                target_px,
                context,
                admission,
                &cancel_flag,
            );
            if cancellation.is_cancelled() {
                return Err(error_response(
                    ThumbnailErrorCode::NotReady,
                    "音声サムネイル要求は取り消されました",
                ));
            }
            let invalid = match &result {
                Err(ThumbnailResponse::Error(ThumbnailError {
                    code: ThumbnailErrorCode::NoThumbnail,
                    ..
                })) => true,
                Err(ThumbnailResponse::Error(ThumbnailError {
                    code: ThumbnailErrorCode::GenerationFailed,
                    ..
                })) => {
                    let usable = if crate::raw_format::is_raw_path(&sidecar.logical) {
                        // Diagnostic classification must not promote a Remote preview
                        // request into a new half-development job.
                        crate::raw::raw_decoder::preview(crate::raw::RawSource::Path(
                            &sidecar.logical,
                        ))
                        .map(|_| true)
                    } else {
                        crate::thumb_loader::decode_image_for_thumb_with_dims(
                            &sidecar.logical,
                            target_px,
                            raw_executor,
                        )
                        .map(|image| image.is_some())
                    };
                    match usable {
                        Ok(usable) => !usable,
                        Err(
                            crate::raw::RawError::Corrupt(_)
                            | crate::raw::RawError::Unsupported(_)
                            | crate::raw::RawError::NoUsablePreview(_)
                            | crate::raw::RawError::TooLarge,
                        ) => true,
                        Err(crate::raw::RawError::Cancelled | crate::raw::RawError::Stale) => {
                            return Err(error_response(
                                ThumbnailErrorCode::NotReady,
                                "音声サムネイル要求は取り消されました",
                            ));
                        }
                        Err(error) => {
                            return Err(error_response(
                                ThumbnailErrorCode::GenerationFailed,
                                format!("audio sidecar: {error}"),
                            ));
                        }
                    }
                }
                _ => false,
            };
            if cancellation.is_cancelled() {
                return Err(error_response(
                    ThumbnailErrorCode::NotReady,
                    "音声サムネイル要求は取り消されました",
                ));
            }
            if thumbnail_source_stamp(&resolved.logical)? != original_stamp
                || thumbnail_source_stamp(&sidecar.logical)? != sidecar_stamp
            {
                return Err(error_response(
                    ThumbnailErrorCode::GenerationFailed,
                    "サムネイル生成中に出所が変更されました",
                ));
            }
            if !invalid {
                return result;
            }
            crate::logger::log(format!(
                "remote_ipc: invalid audio sidecar fallback path={}",
                sidecar.logical.display()
            ));
        }
        let pixels = crate::audio_thumbnail::generate(
            &resolved.logical,
            None,
            settings,
            crate::thumb_loader::LoadSourcePolicy::CacheOrSource,
            admission,
            target_px,
            &|| cancellation.is_cancelled(),
        )
        .map_err(|error| match error {
            crate::audio_thumbnail::AudioThumbnailError::Canceled => error_response(
                ThumbnailErrorCode::NotReady,
                "音声サムネイル要求は取り消されました",
            ),
            crate::audio_thumbnail::AudioThumbnailError::Failed(message) => {
                error_response(ThumbnailErrorCode::GenerationFailed, message)
            }
        })?;
        if thumbnail_source_stamp(&resolved.logical)? != original_stamp {
            return Err(error_response(
                ThumbnailErrorCode::GenerationFailed,
                "サムネイル生成中に出所が変更されました",
            ));
        }
        let pixels = pixels.ok_or_else(|| {
            error_response(
                ThumbnailErrorCode::NoThumbnail,
                "音声ファイルに画像がありません",
            )
        })?;
        let image = color_image_to_dynamic(&pixels.image).ok_or_else(|| {
            error_response(
                ThumbnailErrorCode::GenerationFailed,
                "画像変換に失敗しました",
            )
        })?;
        crate::catalog::encode_thumb_webp(
            &image,
            target_px.min(settings.thumb_px.max(1)),
            settings.thumb_quality as f32,
        )
        .map(|(bytes, _, _)| bytes)
        .ok_or_else(|| {
            error_response(
                ThumbnailErrorCode::GenerationFailed,
                "WebP エンコードに失敗しました",
            )
        })
    }

    #[cfg(test)]
    fn generate_video_resolved(
        &self,
        resolved: &ResolvedPath,
        source_address: Option<&RemoteAddress>,
        target_px: u32,
        context: &WorkerContext,
    ) -> Result<Vec<u8>, ThumbnailResponse> {
        self.generate_video_resolved_with_settings(
            resolved,
            source_address,
            target_px,
            context,
            &self.settings,
            crate::catalog::CatalogAccess::for_cache_dir(&crate::catalog::default_cache_dir())
                .admit(),
            &Arc::new(AtomicBool::new(false)),
        )
    }

    fn generate_video_resolved_with_settings(
        &self,
        resolved: &ResolvedPath,
        source_address: Option<&RemoteAddress>,
        target_px: u32,
        context: &WorkerContext,
        settings: &crate::settings::Settings,
        admission: crate::catalog::CatalogAdmission,
        cancellation: &Arc<AtomicBool>,
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
            && let Some(sidecar) = self.resolve_media_sidecar(resolved, source_address, settings)?
        {
            return self.generate_catalog_resolved_admitted(
                &sidecar,
                target_px,
                context,
                admission,
                cancellation,
            );
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

    #[cfg(test)]
    fn resolve_video_sidecar(
        &self,
        video: &ResolvedPath,
        source_address: &RemoteAddress,
    ) -> Result<Option<ResolvedPath>, ThumbnailResponse> {
        self.resolve_media_sidecar(video, source_address, &self.settings)
    }

    fn resolve_media_sidecar(
        &self,
        video: &ResolvedPath,
        source_address: &RemoteAddress,
        settings: &crate::settings::Settings,
    ) -> Result<Option<ResolvedPath>, ThumbnailResponse> {
        if !settings.skip_image_if_video_exists || !settings.video_thumb_use_sidecar_image {
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

        let hinted = Path::new(&source_address.path);
        let logical_parent_matches =
            hinted
                .parent()
                .zip(video.logical.parent())
                .is_some_and(|(a, b)| {
                    crate::path_key::normalize_keep_drive(a)
                        == crate::path_key::normalize_keep_drive(b)
                });
        let logical_stem_matches = file_stem_lower(hinted)
            .zip(file_stem_lower(&video.logical))
            .is_some_and(|(a, b)| a == b);
        if !logical_parent_matches || !logical_stem_matches || !is_supported_image(hinted) {
            return Err(error_response(
                ThumbnailErrorCode::PathRejected,
                "同じフォルダ・stem の画像だけを使用できます",
            ));
        }
        let sidecar = match resolve_existing(&source_address.path) {
            Ok(source) => source,
            Err(ResolveError::Unavailable)
                if std::fs::metadata(hinted)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(resolve_error_response(error)),
        };
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
    let provenance = if matches!(resolved.kind, ResolvedKind::Folder) {
        crate::catalog::FolderThumbProvenance::AutoSelected
    } else {
        crate::catalog::FolderThumbProvenance::Seeded
    };
    let Some(base_key) = crate::thumb_loader::folder_thumb_cache_key_for_path(
        container,
        container
            .parent()
            .is_some_and(crate::path_key::is_drive_or_share_root),
        request.folder_thumb_sort.unwrap_or_default(),
        request.folder_thumb_depth,
        provenance,
    ) else {
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
    request.folder_thumb_provenance = Some(provenance);
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
    crate::folder_tree::is_paged_document_path(path)
        || path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                crate::folder_tree::is_zip_extension(&extension.to_ascii_lowercase())
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

    #[cfg(windows)]
    #[test]
    fn remote_raw_thumbnail_uses_small_preview_without_half_development() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let folder = data_dir.path().join("remote-raw-thumbnail");
        std::fs::create_dir_all(&folder).unwrap();
        let source = Path::new("vendor/raw-samples/1018.cr2");
        assert!(source.is_file(), "Run .\\scripts\\setup-raw-samples.ps1");
        let path = folder.join("page.cr2");
        std::fs::copy(source, &path).unwrap();
        let mut settings = crate::settings::Settings::default();
        settings.thumb_px = 2048;
        let engine = ThumbnailEngine::new(settings);
        let context = WorkerContext::open();
        let resolved = resolve_existing(path.to_string_lossy().as_ref()).unwrap();
        let preview = crate::raw::raw_decoder::preview(crate::raw::RawSource::Path(&path)).unwrap();
        assert!(preview.image.width().max(preview.image.height()) < 2048);
        let catalog =
            crate::catalog::CatalogDb::open(&crate::catalog::default_cache_dir(), &folder).unwrap();
        let cached = image::DynamicImage::new_rgb8(8, 8);
        let webp = crate::catalog::encode_thumb_webp(&cached, 8, 80.0)
            .unwrap()
            .0;
        let metadata = std::fs::metadata(&path).unwrap();
        catalog
            .save(
                "page.cr2",
                crate::ui_helpers::mtime_secs(&metadata),
                metadata.len() as i64,
                8,
                8,
                Some((8, 8)),
                &webp,
            )
            .unwrap();
        let bytes = engine
            .generate_catalog_resolved(&resolved, 2048, &context)
            .unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert!(decoded.width().max(decoded.height()) > 8);
        let saved = catalog.load_one("page.cr2").unwrap().unwrap();
        let saved = image::load_from_memory(&saved.jpeg_data).unwrap();
        assert_eq!((saved.width(), saved.height()), (8, 8));
    }

    #[cfg(windows)]
    #[test]
    fn remote_raw_without_preview_uses_catalog_only_or_no_thumbnail() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let folder = data_dir.path().join("remote-raw-catalog-only");
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("page.cr2");
        std::fs::write(&path, b"invalid raw with no preview").unwrap();
        let engine = ThumbnailEngine::new(crate::settings::Settings::default());
        let context = WorkerContext::open();
        let resolved = resolve_existing(path.to_string_lossy().as_ref()).unwrap();
        let missing = engine.generate_catalog_resolved(&resolved, 256, &context);
        assert!(matches!(
            missing,
            Err(ThumbnailResponse::Error(ThumbnailError {
                code: ThumbnailErrorCode::NoThumbnail,
                ..
            }))
        ));

        let catalog =
            crate::catalog::CatalogDb::open(&crate::catalog::default_cache_dir(), &folder).unwrap();
        let cached = image::DynamicImage::new_rgb8(8, 8);
        let webp = crate::catalog::encode_thumb_webp(&cached, 8, 80.0)
            .unwrap()
            .0;
        let metadata = std::fs::metadata(&path).unwrap();
        catalog
            .save(
                "page.cr2",
                crate::ui_helpers::mtime_secs(&metadata),
                metadata.len() as i64,
                8,
                8,
                Some((8, 8)),
                &webp,
            )
            .unwrap();
        let bytes = engine
            .generate_catalog_resolved(&resolved, 256, &context)
            .unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (8, 8));
    }

    #[cfg(windows)]
    #[test]
    fn cached_raw_folder_representative_with_no_preview_uses_catalog() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let root = data_dir.path().join("remote-cached-raw-folder");
        let folder = root.join("pages");
        std::fs::create_dir_all(&folder).unwrap();
        let raw_path = folder.join("page.cr2");
        std::fs::write(&raw_path, b"invalid raw with no preview").unwrap();
        let mut settings = crate::settings::Settings::default();
        settings.thumb_px = 2048;
        let engine = ThumbnailEngine::new(settings);
        let resolved = resolve_existing(folder.to_string_lossy().as_ref()).unwrap();
        let context = WorkerContext::open();
        assert!(matches!(
            engine.generate_catalog_resolved(&resolved, 2048, &context),
            Err(ThumbnailResponse::Error(ThumbnailError {
                code: ThumbnailErrorCode::NoThumbnail,
                ..
            }))
        ));
        let key = crate::thumb_loader::folder_thumb_auto_cache_key_for_path(
            &resolved.logical,
            false,
            engine.settings.folder_thumb_sort,
            engine.settings.folder_thumb_depth,
        )
        .unwrap();
        let catalog =
            crate::catalog::CatalogDb::open(&crate::catalog::default_cache_dir(), &root).unwrap();
        let cached = image::DynamicImage::new_rgb8(8, 8);
        let webp = crate::catalog::encode_thumb_webp(&cached, 8, 80.0)
            .unwrap()
            .0;
        let meta = std::fs::metadata(&folder).unwrap();
        let raw_meta = std::fs::metadata(&raw_path).unwrap();
        let proof = crate::catalog::FolderSelectionProof {
            directories: Vec::new(),
            pin_store: crate::catalog::PinStoreProof::NotConsulted,
            winner: crate::catalog::FolderSelectionWinner {
                path: raw_path,
                mtime: crate::ui_helpers::mtime_secs(&raw_meta),
                file_size: raw_meta.len() as i64,
                archive_row_key: None,
            },
        };
        catalog
            .save_auto_folder_bytes(
                &key,
                crate::ui_helpers::mtime_secs(&meta),
                0,
                Some((8, 8)),
                None,
                &webp,
                Some(&proof),
            )
            .unwrap();
        assert!(catalog.load_one(&key).unwrap().is_some());
        let bytes = engine
            .generate_catalog_resolved(&resolved, 2048, &context)
            .unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (8, 8));
    }

    #[test]
    fn remote_folder_pins_use_typed_seeded_and_auto_keys() {
        use crate::folder_thumb_pins::{FileKind, FolderPinSource};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("folder");
        let child = root.join("child");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(root.join("cover.jpg"), b"not decoded").unwrap();
        let db = crate::folder_thumb_pins::FolderThumbPinDb::open_at(&temp.path().join("pins.db"))
            .unwrap();
        let make_request = || crate::thumb_loader::LoadRequest {
            path: root.clone(),
            cache_key_override: crate::thumb_loader::folder_thumb_auto_cache_key_for_path(
                &root,
                false,
                crate::settings::SortOrder::Numeric,
                3,
            ),
            folder_thumb_sort: Some(crate::settings::SortOrder::Numeric),
            folder_thumb_depth: 3,
            folder_thumb_provenance: Some(crate::catalog::FolderThumbProvenance::AutoSelected),
            ..Default::default()
        };
        db.set(
            &root,
            &FolderPinSource::File {
                rel: "cover.jpg".to_owned(),
                kind: FileKind::Image,
            },
        )
        .unwrap();
        let mut image_pin = make_request();
        apply_supported_folder_pin(&mut image_pin, &root, Some(&db));
        assert_eq!(
            image_pin.folder_thumb_provenance,
            Some(crate::catalog::FolderThumbProvenance::Seeded)
        );
        assert!(
            image_pin
                .cache_key_override
                .as_deref()
                .unwrap()
                .contains("auto-v2")
        );

        db.set(
            &root,
            &FolderPinSource::File {
                rel: "child".to_owned(),
                kind: FileKind::Folder,
            },
        )
        .unwrap();
        let mut folder_pin = make_request();
        apply_supported_folder_pin(&mut folder_pin, &root, Some(&db));
        assert_eq!(
            folder_pin.folder_thumb_provenance,
            Some(crate::catalog::FolderThumbProvenance::AutoSelected)
        );
        assert!(
            folder_pin
                .cache_key_override
                .as_deref()
                .unwrap()
                .contains("auto-v3")
        );
    }

    #[test]
    fn remote_folder_round_trip_reselects_after_archive_tile_write() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let root = data_dir.path().join("library");
        let child = root.join("20-child");
        std::fs::create_dir_all(&child).unwrap();
        let red = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([230, 20, 20, 255]),
        ));
        red.save(child.join("cover.png")).unwrap();
        let archive = root.join("10-book.cbz");
        std::fs::write(&archive, b"invalid archive bytes").unwrap();
        let mut settings = crate::settings::Settings::default();
        settings.cache_policy = crate::settings::CachePolicy::Always;
        settings.folder_thumb_sort = crate::settings::SortOrder::Numeric;
        let engine = ThumbnailEngine::new(settings);
        let context = WorkerContext::open();
        let resolved = resolve_existing(root.to_string_lossy().as_ref()).unwrap();
        let first = engine
            .generate_catalog_resolved(&resolved, 64, &context)
            .unwrap();
        let pixel = image::load_from_memory(&first)
            .unwrap()
            .to_rgba8()
            .get_pixel(0, 0)
            .0;
        assert!(pixel[0] > pixel[2], "initial representative should be red");

        let blue = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([20, 20, 230, 255]),
        ));
        let webp = crate::catalog::encode_thumb_webp(&blue, 8, 80.0).unwrap().0;
        let meta = std::fs::metadata(&archive).unwrap();
        let catalog =
            crate::catalog::CatalogDb::open(&crate::catalog::default_cache_dir(), &root).unwrap();
        catalog
            .save_thumb_bytes(
                "zipthumb:10-book.cbz",
                crate::ui_helpers::mtime_secs(&meta),
                meta.len() as i64,
                Some((8, 8)),
                &webp,
            )
            .unwrap();
        let second = engine
            .generate_catalog_resolved(&resolved, 64, &context)
            .unwrap();
        let pixel = image::load_from_memory(&second)
            .unwrap()
            .to_rgba8()
            .get_pixel(0, 0)
            .0;
        assert!(pixel[2] > pixel[0], "new archive tile should win on reload");
    }

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
    fn audio_sidecar_flac_preserves_audio_address_and_falls_back_after_removal_or_disable() {
        let _data = crate::data_dir::TestDataDirGuard::new();
        let temp = tempfile::tempdir().unwrap();
        let audio = temp.path().join("song.flac");
        let sidecar = temp.path().join("song.png");
        std::fs::write(&audio, b"audio").unwrap();
        image::DynamicImage::new_rgb8(12, 8).save(&sidecar).unwrap();
        let settings = crate::settings::Settings::default();
        let engine = ThumbnailEngine::new(settings.clone());
        let context = WorkerContext::without_databases();
        let (cancel, _wake) = super::super::session::RemoteOperationCancellation::for_test();
        let raw_executor = crate::raw::RawDevelopExecutor::new(1).unwrap();
        let resolved = resolve_existing(audio.to_string_lossy().as_ref()).unwrap();
        let hint = RemoteAddress::file(sidecar.to_string_lossy().into_owned());
        let admission =
            crate::catalog::CatalogAccess::for_cache_dir(&crate::catalog::default_cache_dir())
                .admit();
        let bytes = engine
            .generate_audio_resolved(
                &resolved,
                Some(&hint),
                64,
                &context,
                &settings,
                admission,
                &raw_executor,
                &cancel,
            )
            .unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (12, 8));
        let mut disabled = settings.clone();
        disabled.video_thumb_use_sidecar_image = false;
        let result = engine.generate_audio_resolved(
            &resolved,
            Some(&hint),
            64,
            &context,
            &disabled,
            admission,
            &raw_executor,
            &cancel,
        );
        assert!(matches!(
            result,
            Err(ThumbnailResponse::Error(ThumbnailError {
                code: ThumbnailErrorCode::NoThumbnail,
                ..
            }))
        ));
        std::fs::remove_file(&sidecar).unwrap();
        let result = engine.generate_audio_resolved(
            &resolved,
            Some(&hint),
            64,
            &context,
            &settings,
            admission,
            &raw_executor,
            &cancel,
        );
        assert!(matches!(
            result,
            Err(ThumbnailResponse::Error(ThumbnailError {
                code: ThumbnailErrorCode::NoThumbnail,
                ..
            }))
        ));
        assert_eq!(resolved.logical.file_name().unwrap(), "song.flac");
    }

    #[test]
    fn invalid_audio_sidecar_falls_back_to_embedded_mp3_and_flac_no_art() {
        let _data = crate::data_dir::TestDataDirGuard::new();
        let temp = tempfile::tempdir().unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(12, 8)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let mut picture = b"\0image/png\0\x03\0".to_vec();
        picture.extend(png.into_inner());
        let mut tag = b"APIC".to_vec();
        tag.extend((picture.len() as u32).to_be_bytes());
        tag.extend([0, 0]);
        tag.extend(picture);
        let size = tag.len() as u32;
        let mut bytes = b"ID3\x03\0\0".to_vec();
        bytes.extend([
            (size >> 21) as u8 & 127,
            (size >> 14) as u8 & 127,
            (size >> 7) as u8 & 127,
            size as u8 & 127,
        ]);
        bytes.extend(tag);
        let settings = crate::settings::Settings::default();
        let engine = ThumbnailEngine::new(settings.clone());
        let context = WorkerContext::without_databases();
        let (cancel, _wake) = super::super::session::RemoteOperationCancellation::for_test();
        let raw_executor = crate::raw::RawDevelopExecutor::new(1).unwrap();
        let admission =
            crate::catalog::CatalogAccess::for_cache_dir(&crate::catalog::default_cache_dir())
                .admit();
        for extension in ["mp3", "flac"] {
            let audio = temp.path().join(format!("song.{extension}"));
            std::fs::write(
                &audio,
                if extension == "mp3" {
                    bytes.as_slice()
                } else {
                    b"audio"
                },
            )
            .unwrap();
            let sidecar = temp.path().join("song.png");
            std::fs::write(&sidecar, b"corrupt PNG").unwrap();
            let resolved = resolve_existing(audio.to_string_lossy().as_ref()).unwrap();
            let hint = RemoteAddress::file(sidecar.to_string_lossy().into_owned());
            let result = engine.generate_audio_resolved(
                &resolved,
                Some(&hint),
                64,
                &context,
                &settings,
                admission,
                &raw_executor,
                &cancel,
            );
            if extension == "mp3" {
                let image = image::load_from_memory(&result.unwrap()).unwrap();
                assert_eq!((image.width(), image.height()), (12, 8));
            } else {
                assert!(matches!(
                    result,
                    Err(ThumbnailResponse::Error(ThumbnailError {
                        code: ThumbnailErrorCode::NoThumbnail,
                        ..
                    }))
                ));
            }
        }
    }

    #[test]
    fn audio_sidecar_rejects_other_stem_without_hiding_path_violation() {
        let temp = tempfile::tempdir().unwrap();
        let audio = temp.path().join("song.mp3");
        std::fs::write(&audio, b"audio").unwrap();
        let engine = ThumbnailEngine::new(crate::settings::Settings::default());
        let resolved = resolve_existing(audio.to_string_lossy().as_ref()).unwrap();
        let result = engine.resolve_media_sidecar(
            &resolved,
            &RemoteAddress::file(temp.path().join("other.jpg").to_string_lossy().into_owned()),
            &engine.settings,
        );
        assert!(matches!(
            result,
            Err(ThumbnailResponse::Error(ThumbnailError {
                code: ThumbnailErrorCode::PathRejected,
                ..
            }))
        ));
    }

    #[test]
    fn flights_do_not_merge_session_or_catalog_admission_epochs() {
        let first = RequestKey {
            address: RemoteAddress::file("C:/Music/song.mp3"),
            source_address: None,
            target_px: 128,
            session: ("client".into(), "session-a".into()),
            admission: crate::catalog::CatalogAdmission::Admitted(1),
        };
        let mut second = first.clone();
        second.session.1 = "session-b".into();
        assert!(first != second);
        second = first.clone();
        second.admission = crate::catalog::CatalogAdmission::Admitted(2);
        assert!(first != second);
        second.admission = crate::catalog::CatalogAdmission::DisplayOnly(1);
        assert!(first != second);
    }

    #[test]
    fn identical_requests_share_one_flight() {
        let key = RequestKey {
            address: RemoteAddress::file("C:/Pictures/b.jpg"),
            source_address: None,
            target_px: 128,
            session: ("client".into(), "session".into()),
            admission: crate::catalog::CatalogAdmission::Admitted(0),
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

fn is_supported_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "mp3" | "m4a" | "flac" | "aac" | "ogg" | "opus" | "wav" | "wma"
            )
        })
}

fn thumbnail_source_stamp(path: &Path) -> Result<(i64, u64), ThumbnailResponse> {
    let metadata = std::fs::metadata(path)
        .map_err(|_| error_response(ThumbnailErrorCode::NotFound, "対象が見つかりません"))?;
    Ok((crate::ui_helpers::mtime_secs(&metadata), metadata.len()))
}
