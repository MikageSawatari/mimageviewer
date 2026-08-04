use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::Instant;

use mimageviewer_ipc::{
    ContainerEntry, ContainerEntryKind, ContainerKind, ContainerOpenMode, ContainerPayload,
    ContainerRequest, ContainerResponse, FolderListEntry, FolderListPayload, FolderListRequest,
    FolderListResponse, MediaError, MediaErrorCode, PageGroup, PagePayload, PagePriority,
    PageRequest, PageResponse, RemoteAddress, RemoteAiProgressPhase, RemoteAiStartRequest,
    RemoteAiTerminalCode, RemoteEntryKind, RemoteReadingDirection, RemoteSpreadMode,
    RemoteSubresource, RemoteWriteError, RemoteWriteErrorCode, RemoteWriteRequest, ThumbnailError,
    ThumbnailErrorCode, ThumbnailResponse,
};

use super::path_guard::{ResolveError, ResolvedFavoritePath, resolve_existing};
use super::thumbnail::WorkerContext;

const CONTAINER_ENTRY_LIMIT: usize = 1000;
const REMOTE_COMPOSITE_CACHE_ENTRIES: usize = 8;
const REMOTE_COMPOSITE_CACHE_BYTES: usize = 128 * 1024 * 1024;
const REMOTE_LUT_CACHE_ENTRIES: usize = 16;
const MAX_PAGE_RENDER_PX: u32 = crate::pdf_loader::PDF_RENDER_MAX_LONG_PX;
const PAGE_WEBP_QUALITY: f32 = 90.0;
/// Bump only when the native remote AI pipeline changes pixel semantics.
const REMOTE_AI_PIPELINE_SCHEMA: u32 = 1;

pub(super) struct ContainerEngine {
    settings: Arc<crate::settings::Settings>,
    stats: Arc<Mutex<crate::stats::ThumbStats>>,
    pdf_passwords: crate::pdf_passwords::PdfPasswordStore,
    pdf_page_counts: Mutex<HashMap<PdfIdentity, u32>>,
    spread_db: Mutex<Option<crate::spread_db::SpreadDb>>,
    resume_reader: Option<ResumeReader>,
    adjustment_settings: AdjustmentSettingsSource,
    creative_lut_cache: Mutex<RemoteCreativeLutCache>,
    page_composite_cache: Mutex<RemoteCompositeCache>,
    remote_ai_native_cache: Mutex<RemoteAiNativeCache>,
    comic_stamp_cache: Mutex<HashMap<String, Option<Arc<comic_core::RgbaOverlay>>>>,
    session: Option<super::session::SessionHandle>,
    page_prefetch_cancel: Mutex<Option<Arc<AtomicBool>>>,
}

enum AdjustmentSettingsSource {
    Live,
    #[cfg(test)]
    Snapshot(crate::settings_db::AdjustmentRenderSettings),
}

#[derive(Clone)]
struct RemoteAdjustmentIdentity {
    page_key: String,
    location_path: PathBuf,
    compiled_book: bool,
}

#[derive(Clone)]
struct RemotePreparedComposite {
    key: RemoteCompositeCacheKey,
    params: crate::adjustment::AdjustParams,
    lut_entry: Option<crate::creative_lut::CreativeLutEntry>,
    edits: RemoteEditSnapshot,
    settings: crate::settings_db::AdjustmentRenderSettings,
}

#[derive(Clone)]
struct RemoteEditSnapshot {
    erase: Option<crate::edit_source::MaskSnapshot>,
    local_adjust: Option<Vec<local_adjust_core::LocalAdjustmentLayer>>,
    conceal: Option<crate::edit_source::MaskSnapshot>,
    conceal_preset: crate::conceal::ConcealPreset,
    comic: Vec<comic_core::AnnotationObject>,
    export_crop: Option<crate::export_crop::CropSettings>,
    fingerprint: [u8; 32],
    pre_ai_fingerprint: [u8; 32],
}

struct RemoteMaterializedEdits {
    pixels: Arc<egui::ColorImage>,
    comic: Vec<comic_core::AnnotationObject>,
    export_crop: Option<crate::export_crop::CropSettings>,
    timing: crate::edit_source::EditSourceTiming,
    used_diffusion_fallback: bool,
}

#[derive(Clone, PartialEq)]
struct RemoteCompositeCacheKey {
    page_key: String,
    mtime: i64,
    file_size: i64,
    target_px: u32,
    params: crate::adjustment::AdjustParams,
    lut_entry: Option<crate::creative_lut::CreativeLutEntry>,
    edit_fingerprint: [u8; 32],
}

struct RemoteCompositeCacheEntry {
    key: RemoteCompositeCacheKey,
    pixels: Arc<egui::ColorImage>,
    bytes: usize,
}

#[derive(Clone, PartialEq)]
struct RemoteAiNativeCacheKey {
    page_key: String,
    mtime: i64,
    file_size: i64,
    source_size: [usize; 2],
    pre_ai_params: crate::adjustment::AdjustParams,
    pre_ai_edit_fingerprint: [u8; 32],
    ai_feature_mode: crate::settings::AiFeatureMode,
    ai_upscale_limit: crate::ai::upscale::AiProcessSizeLimit,
    ai_denoise_limit: crate::ai::upscale::AiProcessSizeLimit,
    ai_backend: Option<String>,
    background_mode: u8,
    pipeline_schema: u32,
    model_epoch: [u8; 32],
}

#[derive(Clone, PartialEq)]
struct RemoteAiResultIdentity {
    composite: RemoteCompositeCacheKey,
    ai_feature_mode: crate::settings::AiFeatureMode,
    ai_upscale_limit: crate::ai::upscale::AiProcessSizeLimit,
    ai_denoise_limit: crate::ai::upscale::AiProcessSizeLimit,
    ai_backend: Option<String>,
    retained_max_entries: usize,
    retained_max_mib: u64,
    background_mode: u8,
}

impl RemoteAiResultIdentity {
    fn from_prepared(prepared: &RemotePreparedComposite, background_mode: u8) -> Self {
        Self {
            composite: prepared.key.clone(),
            ai_feature_mode: prepared.settings.ai_feature_mode,
            ai_upscale_limit: prepared.settings.ai_upscale_limit,
            ai_denoise_limit: prepared.settings.ai_denoise_limit,
            ai_backend: prepared.settings.ai_backend.clone(),
            retained_max_entries: prepared.settings.retained_final_ai_cache_max_entries,
            retained_max_mib: prepared.settings.retained_final_ai_cache_max_mib,
            background_mode,
        }
    }
}

struct RemoteAiNativeCacheEntry {
    key: RemoteAiNativeCacheKey,
    pixels: Arc<egui::ColorImage>,
    used_upscale: bool,
    bytes: u64,
}

#[derive(Default)]
struct RemoteAiNativeCache {
    entries: VecDeque<RemoteAiNativeCacheEntry>,
    bytes: u64,
}

impl RemoteAiNativeCache {
    fn enforce_budget(&mut self, max_entries: usize, max_bytes: u64) {
        if max_entries == 0 || max_bytes == 0 {
            self.entries.clear();
            self.bytes = 0;
            return;
        }
        while self.entries.len() > max_entries || self.bytes > max_bytes {
            let Some(removed) = self.entries.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(removed.bytes);
        }
    }

    fn get(&mut self, key: &RemoteAiNativeCacheKey) -> Option<(Arc<egui::ColorImage>, bool)> {
        let position = self.entries.iter().position(|entry| entry.key == *key)?;
        let entry = self.entries.remove(position)?;
        let result = (Arc::clone(&entry.pixels), entry.used_upscale);
        self.entries.push_back(entry);
        Some(result)
    }

    fn insert(
        &mut self,
        key: RemoteAiNativeCacheKey,
        pixels: Arc<egui::ColorImage>,
        used_upscale: bool,
        max_entries: usize,
        max_bytes: u64,
    ) {
        if max_entries == 0 || max_bytes == 0 {
            self.enforce_budget(max_entries, max_bytes);
            return;
        }
        let bytes = pixels.as_raw().len() as u64;
        if bytes > max_bytes {
            return;
        }
        if let Some(position) = self.entries.iter().position(|entry| entry.key == key)
            && let Some(previous) = self.entries.remove(position)
        {
            self.bytes = self.bytes.saturating_sub(previous.bytes);
        }
        self.entries.push_back(RemoteAiNativeCacheEntry {
            key,
            pixels,
            used_upscale,
            bytes,
        });
        self.bytes = self.bytes.saturating_add(bytes);
        self.enforce_budget(max_entries, max_bytes);
    }
}

#[derive(Default)]
struct RemoteCompositeCache {
    entries: VecDeque<RemoteCompositeCacheEntry>,
    bytes: usize,
}

impl RemoteCompositeCache {
    fn get(&mut self, key: &RemoteCompositeCacheKey) -> Option<Arc<egui::ColorImage>> {
        let position = self.entries.iter().position(|entry| entry.key == *key)?;
        let entry = self.entries.remove(position)?;
        let pixels = Arc::clone(&entry.pixels);
        self.entries.push_back(entry);
        Some(pixels)
    }

    fn insert(&mut self, key: RemoteCompositeCacheKey, pixels: Arc<egui::ColorImage>) {
        let bytes = pixels
            .pixels
            .len()
            .saturating_mul(std::mem::size_of::<egui::Color32>());
        if bytes > REMOTE_COMPOSITE_CACHE_BYTES {
            return;
        }
        if let Some(position) = self.entries.iter().position(|entry| entry.key == key)
            && let Some(entry) = self.entries.remove(position)
        {
            self.bytes = self.bytes.saturating_sub(entry.bytes);
        }
        self.bytes = self.bytes.saturating_add(bytes);
        self.entries
            .push_back(RemoteCompositeCacheEntry { key, pixels, bytes });
        while self.entries.len() > REMOTE_COMPOSITE_CACHE_ENTRIES
            || self.bytes > REMOTE_COMPOSITE_CACHE_BYTES
        {
            let Some(entry) = self.entries.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(entry.bytes);
        }
    }
}

#[derive(Default)]
struct RemoteCreativeLutCache {
    entries: VecDeque<(
        crate::creative_lut::CreativeLutEntry,
        crate::creative_lut::SharedCreativeLut,
    )>,
}

impl RemoteCreativeLutCache {
    fn resolve(
        &mut self,
        entry: &crate::creative_lut::CreativeLutEntry,
    ) -> Result<crate::creative_lut::SharedCreativeLut, String> {
        if let Some(position) = self.entries.iter().position(|(cached, _)| cached == entry) {
            let cached = self.entries.remove(position).expect("position exists");
            let lut = Arc::clone(&cached.1);
            self.entries.push_back(cached);
            return Ok(lut);
        }
        self.entries.retain(|(cached, _)| cached.id != entry.id);
        let lut = crate::creative_lut::load_creative_lut_entry(entry)?;
        self.entries.push_back((entry.clone(), Arc::clone(&lut)));
        while self.entries.len() > REMOTE_LUT_CACHE_ENTRIES {
            self.entries.pop_front();
        }
        Ok(lut)
    }
}

enum ResumeReader {
    Session(super::session::SessionHandle),
    #[cfg(test)]
    Error(super::session::UiReadError),
}

impl ResumeReader {
    fn read_book_resume(&self, path: &Path) -> Result<Option<usize>, super::session::UiReadError> {
        match self {
            Self::Session(session) => session.read_book_resume(path),
            #[cfg(test)]
            Self::Error(error) => Err(*error),
        }
    }
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

struct SpreadPayload {
    configured: RemoteSpreadMode,
    effective: RemoteSpreadMode,
    reading_direction: RemoteReadingDirection,
    groups: Vec<PageGroup>,
}

struct ValidatedPageContext {
    page_index: u32,
    page_number: u32,
    page_count: u32,
    record_history: bool,
    record_resume: bool,
    bookmark_supported: bool,
}

struct RecomputedFolderListing {
    items: Vec<crate::grid_item::GridItem>,
    metas: Vec<Option<(i64, i64)>>,
    video_thumb_overrides: Vec<(std::path::PathBuf, std::path::PathBuf)>,
    scan_ms: f64,
    materialize_ms: f64,
    image_only: bool,
    compiled: bool,
}
fn folder_thumb_aspect_height_ratio(settings: &crate::settings::Settings, folder: &Path) -> f64 {
    let aspect = if settings.thumb_aspect_auto {
        crate::auto_aspect_cache::AutoAspectCacheDb::get_read_only(folder)
            .map(|entry| entry.aspect)
            .unwrap_or(crate::settings::ThumbAspect::Square)
    } else {
        settings.thumb_aspect
    };
    f64::from(aspect.height_ratio())
}

impl ContainerEngine {
    #[cfg(test)]
    pub(super) fn new(settings: crate::settings::Settings) -> Self {
        let adjustment_settings = AdjustmentSettingsSource::Snapshot(
            crate::settings_db::AdjustmentRenderSettings::from_settings(&settings),
        );
        Self::new_inner(settings, None, None, adjustment_settings)
    }

    pub(super) fn new_with_session(
        settings: crate::settings::Settings,
        session: super::session::SessionHandle,
    ) -> Self {
        Self::new_inner(
            settings,
            Some(ResumeReader::Session(session.clone())),
            Some(session),
            AdjustmentSettingsSource::Live,
        )
    }

    #[cfg(test)]
    fn new_with_resume_error(
        settings: crate::settings::Settings,
        error: super::session::UiReadError,
    ) -> Self {
        let adjustment_settings = AdjustmentSettingsSource::Snapshot(
            crate::settings_db::AdjustmentRenderSettings::from_settings(&settings),
        );
        Self::new_inner(
            settings,
            Some(ResumeReader::Error(error)),
            None,
            adjustment_settings,
        )
    }

    fn new_inner(
        settings: crate::settings::Settings,
        resume_reader: Option<ResumeReader>,
        session: Option<super::session::SessionHandle>,
        adjustment_settings: AdjustmentSettingsSource,
    ) -> Self {
        let spread_db_path = crate::data_dir::get().join("spread.db");
        let spread_db =
            match crate::spread_db::SpreadDb::open_existing_read_only_at(&spread_db_path) {
                Ok(db) => db,
                Err(error) => {
                    crate::logger::log(format!(
                        "remote_ipc: spread DB read-only open failed: {error}"
                    ));
                    None
                }
            };
        Self {
            settings: Arc::new(settings),
            stats: Arc::new(Mutex::new(crate::stats::ThumbStats::new())),
            pdf_passwords: crate::pdf_passwords::PdfPasswordStore::load(),
            pdf_page_counts: Mutex::new(HashMap::new()),
            spread_db: Mutex::new(spread_db),
            resume_reader,
            adjustment_settings,
            creative_lut_cache: Mutex::new(RemoteCreativeLutCache::default()),
            page_composite_cache: Mutex::new(RemoteCompositeCache::default()),
            remote_ai_native_cache: Mutex::new(RemoteAiNativeCache::default()),
            comic_stamp_cache: Mutex::new(HashMap::new()),
            session,
            page_prefetch_cancel: Mutex::new(None),
        }
    }
    fn adjustment_render_settings(
        &self,
    ) -> Result<crate::settings_db::AdjustmentRenderSettings, MediaError> {
        match &self.adjustment_settings {
            AdjustmentSettingsSource::Live => {
                crate::settings_db::with_db_result(|db| db.load_adjustment_render_settings())
                    .map_err(|error| {
                        crate::logger::log(format!(
                            "remote_ipc: live adjustment settings read failed: {error}"
                        ));
                        media_error(
                            MediaErrorCode::Internal,
                            "最新の補正設定を読み込めませんでした",
                        )
                    })
            }
            #[cfg(test)]
            AdjustmentSettingsSource::Snapshot(settings) => Ok(settings.clone()),
        }
    }

    fn prepare_remote_edits(
        &self,
        page_key: &str,
        settings: &crate::settings_db::AdjustmentRenderSettings,
        context: &WorkerContext,
    ) -> Result<RemoteEditSnapshot, MediaError> {
        let erase = match context.mask_db.as_ref() {
            Some(db) => load_mask_snapshot(db, page_key)?,
            None => {
                let db = crate::mask_db::MaskDb::open_readonly()
                    .map_err(|error| remote_edit_db_open_error("erase", error))?;
                load_mask_snapshot(&db, page_key)?
            }
        };
        let local_adjust = match context.local_adjust_db.as_ref() {
            Some(db) => db
                .get_layers_checked(page_key)
                .map_err(|error| remote_edit_db_read_error("local-adjust", error))?,
            None => {
                let db = crate::local_adjust_db::LocalAdjustDb::open_readonly(
                    &crate::local_adjust_db::LocalAdjustDb::db_path(),
                )
                .map_err(|error| remote_edit_db_open_error("local-adjust", error))?;
                db.get_layers_checked(page_key)
                    .map_err(|error| remote_edit_db_read_error("local-adjust", error))?
            }
        };
        let conceal = match context.conceal_db.as_ref() {
            Some(db) => load_conceal_snapshot(db, page_key)?,
            None => {
                let db = crate::conceal_db::ConcealDb::open_readonly(
                    &crate::conceal_db::ConcealDb::db_path(),
                )
                .map_err(|error| remote_edit_db_open_error("conceal", error))?;
                load_conceal_snapshot(&db, page_key)?
            }
        };
        let comic = match context.comic_db.as_ref() {
            Some(db) => db
                .get_checked(page_key)
                .map_err(|error| remote_edit_db_read_error("comic", error))?
                .unwrap_or_default(),
            None => {
                let db = crate::comic_db::ComicDb::open_readonly()
                    .map_err(|error| remote_edit_db_open_error("comic", error))?;
                db.get_checked(page_key)
                    .map_err(|error| remote_edit_db_read_error("comic", error))?
                    .unwrap_or_default()
            }
        };
        let export_crop = match context.crop_db.as_ref() {
            Some(db) => db
                .get_checked(page_key)
                .map_err(|error| remote_edit_db_read_error("export-crop", error))?,
            None => {
                let db = crate::export_crop::CropDb::open_readonly(
                    &crate::export_crop::CropDb::db_path(),
                )
                .map_err(|error| remote_edit_db_open_error("export-crop", error))?;
                db.get_checked(page_key)
                    .map_err(|error| remote_edit_db_read_error("export-crop", error))?
            }
        };
        let conceal_preset = settings.conceal_preset.clone();
        let fingerprint = remote_edit_fingerprint(
            erase.as_ref(),
            local_adjust.as_ref(),
            conceal.as_ref(),
            &conceal_preset,
            &comic,
            export_crop.as_ref(),
        )?;
        let pre_ai_fingerprint = remote_pre_ai_edit_fingerprint(
            erase.as_ref(),
            local_adjust.as_ref(),
            conceal.as_ref(),
            &conceal_preset,
        )?;
        Ok(RemoteEditSnapshot {
            erase,
            local_adjust,
            conceal,
            conceal_preset,
            comic,
            export_crop,
            fingerprint,
            pre_ai_fingerprint,
        })
    }

    fn execute_remote_edits(
        &self,
        source: Arc<egui::ColorImage>,
        edits: RemoteEditSnapshot,
        cancel: &Arc<AtomicBool>,
    ) -> Result<RemoteMaterializedEdits, MediaError> {
        let ai_resources = edits.erase.as_ref().and_then(|_| {
            self.session
                .as_ref()
                .and_then(super::session::SessionHandle::remote_ai_resources)
        });
        let inpaint_runtime = ai_resources
            .as_ref()
            .map(|resources| Arc::clone(&resources.runtime));
        let inpaint_manager = ai_resources
            .map(|resources| resources.manager)
            .unwrap_or_else(|| Arc::new(crate::ai::model_manager::ModelManager::new()));
        let erase = match edits.erase {
            Some(mask) => {
                crate::edit_source::EditLayer::Materialize(crate::edit_source::EraseMaterialize {
                    mask,
                    runtime: inpaint_runtime,
                    manager: inpaint_manager,
                    log_prefix: "remote page".to_string(),
                })
            }
            None => crate::edit_source::EditLayer::Absent,
        };
        let local_adjust =
            edits
                .local_adjust
                .map_or(crate::edit_source::EditLayer::Absent, |layers| {
                    crate::edit_source::EditLayer::Materialize(
                        crate::edit_source::LocalAdjustMaterialize { layers },
                    )
                });
        let conceal = edits
            .conceal
            .map_or(crate::edit_source::EditLayer::Absent, |mask| {
                crate::edit_source::EditLayer::Materialize(crate::edit_source::ConcealMaterialize {
                    mask,
                    preset: edits.conceal_preset,
                })
            });
        let result = crate::edit_source::execute_edit_source(
            crate::edit_source::EditSourceRequest {
                raw: source,
                erase,
                local_adjust,
                conceal,
            },
            cancel,
        )
        .map_err(|error| {
            crate::logger::log(format!("remote_ipc: edit materialization failed: {error}"));
            media_error(
                MediaErrorCode::RenderFailed,
                "編集結果をページへ合成できませんでした",
            )
        })?;
        let crate::edit_source::EditSourceResult::Ready(output) = result else {
            return Err(media_error(
                MediaErrorCode::Busy,
                "ページの編集結果合成は取り消されました",
            ));
        };
        Ok(RemoteMaterializedEdits {
            pixels: output.pixels,
            comic: edits.comic,
            export_crop: edits.export_crop,
            timing: output.timing,
            used_diffusion_fallback: output.used_diffusion_fallback,
        })
    }

    fn prepare_remote_composite(
        &self,
        address: &RemoteAddress,
        logical_path: &Path,
        mtime: i64,
        file_size: i64,
        target_px: u32,
        preview: Option<&mimageviewer_ipc::RemoteAdjustmentPreview>,
        context: &WorkerContext,
    ) -> Result<Option<RemotePreparedComposite>, MediaError> {
        let Some(mut identity) = remote_adjustment_identity(address, logical_path) else {
            return Ok(None);
        };
        identity.compiled_book = matches!(address.subresource, RemoteSubresource::File)
            && logical_path.parent().is_some_and(|parent| {
                crate::books::is_direct_book_folder(&self.settings.books_root_path(), parent)
            });
        let settings = self.adjustment_render_settings()?;
        let edits = self.prepare_remote_edits(&identity.page_key, &settings, context)?;
        // `WorkerContext` は worker 起動時に 1 度だけ DB を開く。その 1 回が失敗した状態を
        // そのまま合成失敗にすると、一過性の失敗でも **その worker を通る全ページ**が
        // 以後ずっと失敗する。開けない事実は隠さず、握り直しだけ試みる。
        let reopened;
        let adjustment_db = match context.adjustment_db.as_ref() {
            Some(db) => db,
            None => {
                reopened = crate::adjustment_db::AdjustmentDb::open().map_err(|error| {
                    crate::logger::log(format!(
                        "remote_ipc: adjustment db reopen failed for page composition: {error}"
                    ));
                    media_error(
                        MediaErrorCode::Internal,
                        "補正データベースを開けないためページを合成できません",
                    )
                })?;
                &reopened
            }
        };
        let page = adjustment_db
            .get_page_params_checked(&identity.page_key)
            .map_err(|error| remote_adjustment_read_error("page", error))?;
        let favorite_params = if page.is_none()
            || preview.is_some_and(|preview| {
                preview.scope == mimageviewer_ipc::RemoteAdjustmentScope::Standard
            }) {
            adjustment_db
                .load_all_favorite_params_checked()
                .map_err(|error| remote_adjustment_read_error("location", error))?
        } else {
            HashMap::new()
        };
        let selected_page = if preview.is_some_and(|preview| {
            preview.scope == mimageviewer_ipc::RemoteAdjustmentScope::Standard
        }) {
            None
        } else {
            page.as_ref()
        };
        let mut params = resolve_remote_effective_params(
            &identity,
            selected_page,
            &settings.favorites,
            &favorite_params,
            &settings.global_preset,
        );
        if let Some(preview) = preview {
            params = super::apply_remote_adjustment_values(params, &preview.values)
                .map_err(|message| media_error(MediaErrorCode::BadRequest, message))?;
        }
        let lut_entry = params.creative_lut.id.and_then(|id| {
            settings
                .creative_luts
                .iter()
                .find(|entry| entry.id == id)
                .cloned()
        });
        let key = RemoteCompositeCacheKey {
            page_key: identity.page_key,
            mtime,
            file_size,
            target_px,
            params: params.clone(),
            lut_entry: lut_entry.clone(),
            edit_fingerprint: edits.fingerprint,
        };
        Ok(Some(RemotePreparedComposite {
            key,
            params,
            lut_entry,
            edits,
            settings,
        }))
    }

    fn resolve_remote_lut(
        &self,
        entry: Option<&crate::creative_lut::CreativeLutEntry>,
    ) -> Result<Option<crate::creative_lut::SharedCreativeLut>, MediaError> {
        let Some(entry) = entry else {
            return Ok(None);
        };
        self.creative_lut_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .resolve(entry)
            .map(Some)
            .map_err(|error| {
                crate::logger::log(format!(
                    "remote_ipc: Creative LUT load failed id={}: {error}",
                    entry.id
                ));
                media_error(
                    MediaErrorCode::RenderFailed,
                    "Creative LUT を読み込めないためページを合成できません",
                )
            })
    }

    fn begin_page_render(
        &self,
        priority: PagePriority,
        session_cancel: Option<Arc<AtomicBool>>,
    ) -> Arc<AtomicBool> {
        let cancel = session_cancel.unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let mut prefetch = self
            .page_prefetch_cancel
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(previous) = prefetch.take() {
            previous.store(true, Ordering::Relaxed);
        }
        if priority == PagePriority::Prefetch {
            *prefetch = Some(Arc::clone(&cancel));
        }
        cancel
    }

    fn finish_page_render(&self, priority: PagePriority, cancel: &Arc<AtomicBool>) {
        if priority != PagePriority::Prefetch {
            return;
        }
        let mut prefetch = self
            .page_prefetch_cancel
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if prefetch
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, cancel))
        {
            *prefetch = None;
        }
    }

    pub(super) fn container(&self, request: ContainerRequest) -> ContainerResponse {
        let started = Instant::now();
        let source_kind = media_source_kind(&request.address);
        let resolved = match self.resolve(&request.address) {
            Ok(resolved) => resolved,
            Err(error) => return ContainerResponse::Error(error),
        };
        let response = match self.enumerate(&request, &resolved) {
            Ok(payload) => ContainerResponse::Success(payload),
            Err(error) => ContainerResponse::Error(error),
        };
        let (outcome, entry_count, group_count, configured, effective, direction) = match &response
        {
            ContainerResponse::Success(payload) => (
                "ok",
                payload.entries.len(),
                payload.page_groups.len(),
                remote_spread_mode_name(payload.configured_spread_mode),
                remote_spread_mode_name(payload.effective_spread_mode),
                remote_reading_direction_name(payload.reading_direction),
            ),
            ContainerResponse::Error(_) => ("error", 0, 0, "none", "none", "none"),
        };
        crate::logger::log(format!(
            "remote_ipc: media_operation operation=container source_kind={source_kind} outcome={outcome} duration_ms={:.1} entry_count={entry_count} group_count={group_count} configured_spread={configured} effective_spread={effective} reading_direction={direction}",
            started.elapsed().as_secs_f64() * 1000.0
        ));
        response
    }

    pub(super) fn folder_list(&self, request: FolderListRequest) -> FolderListResponse {
        let started = Instant::now();
        let resolved = match self.resolve(&request.address) {
            Ok(resolved) => resolved,
            Err(error) => return FolderListResponse::Error(error),
        };
        if !matches!(request.address.subresource, RemoteSubresource::File)
            || !std::fs::metadata(&resolved.canonical).is_ok_and(|metadata| metadata.is_dir())
        {
            return FolderListResponse::Error(media_error(
                MediaErrorCode::BadRequest,
                "フォルダ一覧のアドレスが不正です",
            ));
        }
        let thumb_aspect_height_ratio =
            folder_thumb_aspect_height_ratio(&self.settings, &resolved.logical);
        let listing = match self.recompute_folder_listing(&resolved.logical) {
            Ok(listing) => listing,
            Err(error) => {
                return FolderListResponse::Error(media_error_from_remote_write(error));
            }
        };
        let entries = listing
            .items
            .iter()
            .zip(&listing.metas)
            .filter_map(|(item, meta)| {
                self.folder_list_entry(
                    &request.address,
                    item,
                    *meta,
                    &listing.video_thumb_overrides,
                )
            })
            .collect::<Vec<_>>();
        let response = FolderListResponse::Success(FolderListPayload {
            effective_address: request.address,
            thumb_aspect_height_ratio,
            entries,
            scan_ms: listing.scan_ms,
            materialize_ms: listing.materialize_ms,
        });
        let entry_count = match &response {
            FolderListResponse::Success(payload) => payload.entries.len(),
            FolderListResponse::Error(_) => 0,
        };
        crate::logger::log(format!(
            "remote_ipc: media_operation operation=folder_list outcome=ok duration_ms={:.1} entry_count={entry_count} scan_ms={:.1} materialize_ms={:.1}",
            started.elapsed().as_secs_f64() * 1000.0,
            listing.scan_ms,
            listing.materialize_ms,
        ));
        response
    }

    pub(super) fn validate_write_request(
        &self,
        request: &mut RemoteWriteRequest,
    ) -> Result<(), RemoteWriteError> {
        match request {
            RemoteWriteRequest::SetSpread { address, .. } => {
                let resolved = self
                    .resolve(address)
                    .map_err(remote_write_error_from_media)?;
                let metadata = std::fs::metadata(&resolved.canonical).ok();
                let is_file = metadata.as_ref().is_some_and(|value| value.is_file());
                let is_directory = metadata.as_ref().is_some_and(|value| value.is_dir());
                let supported = match address.subresource {
                    RemoteSubresource::File => {
                        is_directory
                            || is_file
                                && (is_zip_path(&resolved.logical)
                                    || is_pdf_path(&resolved.logical))
                    }
                    RemoteSubresource::ZipDirectory { .. } => {
                        is_file && is_zip_path(&resolved.logical)
                    }
                    RemoteSubresource::ZipEntry { .. } | RemoteSubresource::PdfPage { .. } => false,
                };
                supported.then_some(()).ok_or_else(|| {
                    RemoteWriteError::new(
                        RemoteWriteErrorCode::Unsupported,
                        "見開き設定を書き込めるコンテナではありません",
                    )
                })
            }
            RemoteWriteRequest::RecordReadingProgress {
                address,
                context_address,
                page_index,
                page_number,
                page_count,
                record_resume,
                record_history,
            } => {
                let validated = self.validate_page_context(address, context_address)?;
                if !validated.record_resume && !validated.record_history {
                    return Err(RemoteWriteError::new(
                        RemoteWriteErrorCode::Unsupported,
                        "この一覧は読書位置の記録対象ではありません",
                    ));
                }
                *page_index = validated.page_index;
                *page_number = validated.page_number;
                *page_count = validated.page_count;
                *record_resume = validated.record_resume;
                *record_history = validated.record_history;
                Ok(())
            }
            RemoteWriteRequest::SetRating { address, stars } => {
                if *stars > 5 {
                    return Err(RemoteWriteError::new(
                        RemoteWriteErrorCode::BadRequest,
                        "レーティングは 0〜5 で指定してください",
                    ));
                }
                self.validate_rating_page(address)
            }
            RemoteWriteRequest::SetBookmark {
                address,
                context_address,
                page_index,
                ..
            } => {
                let validated = self.validate_page_context(address, context_address)?;
                if !validated.bookmark_supported {
                    return Err(RemoteWriteError::new(
                        RemoteWriteErrorCode::Unsupported,
                        "このページは本のブックマーク対象ではありません",
                    ));
                }
                *page_index = validated.page_index;
                Ok(())
            }
            RemoteWriteRequest::GetItemState {
                address,
                context_address,
                page_index,
                bookmark_supported,
            } => {
                self.validate_rating_page(address)?;
                let validated = self.validate_page_context(address, context_address)?;
                *page_index = validated.page_index;
                *bookmark_supported = validated.bookmark_supported;
                Ok(())
            }
            RemoteWriteRequest::SetAdjustment {
                address, values, ..
            } => {
                self.validate_rating_page(address)?;
                super::apply_remote_adjustment_values(
                    crate::adjustment::AdjustParams::default(),
                    values,
                )
                .map(|_| ())
                .map_err(|message| RemoteWriteError::new(RemoteWriteErrorCode::BadRequest, message))
            }
            RemoteWriteRequest::GetAdjustmentState { address } => {
                self.validate_rating_page(address)
            }
        }
    }

    fn validate_rating_page(&self, address: &RemoteAddress) -> Result<(), RemoteWriteError> {
        let resolved = self
            .resolve(address)
            .map_err(remote_write_error_from_media)?;
        let metadata = std::fs::metadata(&resolved.canonical).map_err(|_| {
            RemoteWriteError::new(RemoteWriteErrorCode::NotFound, "ページが見つかりません")
        })?;
        match &address.subresource {
            RemoteSubresource::File => {
                let extension = resolved
                    .logical
                    .extension()
                    .and_then(|value| value.to_str())
                    .map(str::to_ascii_lowercase)
                    .unwrap_or_default();
                if metadata.is_file() && crate::folder_tree::is_recognized_image_ext(&extension) {
                    Ok(())
                } else {
                    Err(RemoteWriteError::new(
                        RemoteWriteErrorCode::Unsupported,
                        "このファイルにはレーティングを付けられません",
                    ))
                }
            }
            RemoteSubresource::ZipEntry { entry_name } if is_zip_path(&resolved.logical) => {
                let enumeration =
                    crate::zip_loader::enumerate_image_entries_detailed(&resolved.logical)
                        .map_err(|_| {
                            RemoteWriteError::new(
                                RemoteWriteErrorCode::PersistenceFailed,
                                "ZIP を列挙できませんでした",
                            )
                        })?;
                enumeration
                    .entries
                    .iter()
                    .any(|entry| entry.entry_name == *entry_name)
                    .then_some(())
                    .ok_or_else(|| {
                        RemoteWriteError::new(
                            RemoteWriteErrorCode::NotFound,
                            "ZIP 内のページが見つかりません",
                        )
                    })
            }
            RemoteSubresource::PdfPage { page_number } if is_pdf_path(&resolved.logical) => self
                .ensure_pdf_page_in_range(&resolved, &metadata, *page_number)
                .map_err(remote_write_error_from_media),
            _ => Err(RemoteWriteError::new(
                RemoteWriteErrorCode::Unsupported,
                "このページにはレーティングを付けられません",
            )),
        }
    }

    fn validate_page_context(
        &self,
        address: &RemoteAddress,
        context_address: &RemoteAddress,
    ) -> Result<ValidatedPageContext, RemoteWriteError> {
        if address.favorite_id != context_address.favorite_id {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::BadRequest,
                "ページと閲覧コンテキストが一致しません",
            ));
        }
        match &address.subresource {
            RemoteSubresource::File => self.validate_folder_page(address, context_address),
            RemoteSubresource::ZipEntry { entry_name } => {
                self.validate_zip_page(address, context_address, entry_name)
            }
            RemoteSubresource::PdfPage { page_number } => {
                self.validate_pdf_page(address, context_address, *page_number)
            }
            RemoteSubresource::ZipDirectory { .. } => Err(RemoteWriteError::new(
                RemoteWriteErrorCode::Unsupported,
                "コンテナ自体はページではありません",
            )),
        }
    }

    fn validate_folder_page(
        &self,
        address: &RemoteAddress,
        context_address: &RemoteAddress,
    ) -> Result<ValidatedPageContext, RemoteWriteError> {
        if !matches!(context_address.subresource, RemoteSubresource::File) {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::BadRequest,
                "画像フォルダのコンテキストが不正です",
            ));
        }
        let page = self
            .resolve(address)
            .map_err(remote_write_error_from_media)?;
        let context = self
            .resolve(context_address)
            .map_err(remote_write_error_from_media)?;
        if !std::fs::metadata(&page.canonical).is_ok_and(|metadata| metadata.is_file())
            || !std::fs::metadata(&context.canonical).is_ok_and(|metadata| metadata.is_dir())
            || page.canonical.parent() != Some(context.canonical.as_path())
        {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::PathRejected,
                "画像が閲覧フォルダの直下にありません",
            ));
        }
        let listing = self.recompute_folder_listing(&context.logical)?;
        let items = listing.items;
        let image_only = listing.image_only;
        let compiled = listing.compiled;
        let index = items
            .iter()
            .position(|item| {
                matches!(
                    item,
                    crate::grid_item::GridItem::Image(path)
                        if crate::path_key::eq_keep_drive(path, &page.logical)
                )
            })
            .ok_or_else(|| {
                RemoteWriteError::new(
                    RemoteWriteErrorCode::NotFound,
                    "画像フォルダ内のページが見つかりません",
                )
            })?;
        let page_number = items[..=index]
            .iter()
            .filter(|item| item.has_page_data())
            .count();
        let page_count = items.iter().filter(|item| item.has_page_data()).count();
        validated_context(
            index,
            page_count,
            image_only,
            true,
            compiled || (image_only && self.settings.auto_fullscreen_image_folders_enabled()),
        )
        .map(|mut context| {
            context.page_number = u32::try_from(page_number).unwrap_or(u32::MAX);
            context
        })
    }

    fn recompute_folder_listing(
        &self,
        folder: &Path,
    ) -> Result<RecomputedFolderListing, RemoteWriteError> {
        let scan_started = Instant::now();
        let scan = crate::app::folder_scan::scan_directory_with_settings(folder, &self.settings)
            .map_err(|_| {
                RemoteWriteError::new(
                    RemoteWriteErrorCode::PersistenceFailed,
                    "画像フォルダを走査できませんでした",
                )
            })?;
        let scan_ms = scan_started.elapsed().as_secs_f64() * 1000.0;
        let compiled =
            crate::books::is_direct_book_folder(&self.settings.books_root_path(), folder);
        let image_only = if compiled {
            scan.all_media
                .iter()
                .any(|(_, kind, _, _)| *kind == crate::app::folder_scan::ScanMediaKind::Image)
        } else {
            crate::app::folder_scan::is_image_only_book_contents(
                !scan.folders.is_empty(),
                &scan.all_media,
            )
        };
        let materialize_started = Instant::now();
        let materialized =
            crate::app::materialize_local_folder_listing(folder, scan, &self.settings);
        let materialize_ms = materialize_started.elapsed().as_secs_f64() * 1000.0;
        Ok(RecomputedFolderListing {
            items: materialized.items,
            metas: materialized.metas,
            video_thumb_overrides: materialized.video_thumb_overrides,
            scan_ms,
            materialize_ms,
            image_only,
            compiled,
        })
    }

    fn folder_list_entry(
        &self,
        container: &RemoteAddress,
        item: &crate::grid_item::GridItem,
        meta: Option<(i64, i64)>,
        video_thumb_overrides: &[(std::path::PathBuf, std::path::PathBuf)],
    ) -> Option<FolderListEntry> {
        let (path, kind) = match item {
            crate::grid_item::GridItem::Folder(path) => (path, RemoteEntryKind::Folder),
            crate::grid_item::GridItem::Image(path) => (path, RemoteEntryKind::Image),
            crate::grid_item::GridItem::Video(path) => (path, RemoteEntryKind::Video),
            crate::grid_item::GridItem::Audio(path) => (path, RemoteEntryKind::Audio),
            crate::grid_item::GridItem::ZipFile(path) => (path, RemoteEntryKind::Zip),
            crate::grid_item::GridItem::PdfFile(path) => (path, RemoteEntryKind::Pdf),
            crate::grid_item::GridItem::ConvertibleArchive { path, .. } => {
                (path, RemoteEntryKind::Archive)
            }
            _ => return None,
        };
        let address_for = |candidate: &Path| {
            let name = candidate.file_name()?.to_str()?;
            let parent = container.relative_path.trim_end_matches('/');
            let relative_path = if parent.is_empty() {
                name.to_owned()
            } else {
                format!("{parent}/{name}")
            };
            Some(RemoteAddress::file(
                container.favorite_id.clone(),
                relative_path,
            ))
        };
        let address = address_for(path)?;
        self.resolve(&address).ok()?;
        let thumbnail_address = if kind == RemoteEntryKind::Video {
            video_thumb_overrides
                .iter()
                .rev()
                .find(|(video, _)| crate::path_key::eq_keep_drive(video, path))
                .and_then(|(_, image)| address_for(image))
                .filter(|candidate| self.resolve(candidate).is_ok())
                .unwrap_or_else(|| address.clone())
        } else {
            address.clone()
        };
        let (mtime, size) = meta.unwrap_or((0, 0));
        Some(FolderListEntry {
            address,
            thumbnail_address,
            name: item.name().into_owned(),
            kind,
            size: u64::try_from(size).unwrap_or(0),
            mtime,
        })
    }

    fn validate_zip_page(
        &self,
        address: &RemoteAddress,
        context_address: &RemoteAddress,
        entry_name: &str,
    ) -> Result<ValidatedPageContext, RemoteWriteError> {
        if address.relative_path != context_address.relative_path {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::BadRequest,
                "ZIP ページとコンテキストが一致しません",
            ));
        }
        let resolved = self
            .resolve(address)
            .map_err(remote_write_error_from_media)?;
        if !is_zip_path(&resolved.logical) {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::Unsupported,
                "ZIP ページではありません",
            ));
        }
        let requested_prefix = match &context_address.subresource {
            RemoteSubresource::File => String::new(),
            RemoteSubresource::ZipDirectory { prefix } => prefix.clone(),
            _ => {
                return Err(RemoteWriteError::new(
                    RemoteWriteErrorCode::BadRequest,
                    "ZIP の閲覧コンテキストが不正です",
                ));
            }
        };
        let enumeration = crate::zip_loader::enumerate_image_entries_detailed(&resolved.logical)
            .map_err(|_| {
                RemoteWriteError::new(
                    RemoteWriteErrorCode::PersistenceFailed,
                    "ZIP を列挙できませんでした",
                )
            })?;
        let tree = crate::zip_tree::ZipTree::build(resolved.logical, enumeration.entries);
        let requested_segments = zip_prefix_segments(&requested_prefix);
        if tree.node_at(&requested_segments).is_none() {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::NotFound,
                "ZIP 内の場所が見つかりません",
            ));
        }
        let effective_segments = tree.collapse_redundant(&requested_segments);
        let root_segments = tree.collapse_redundant(&[]);
        let (mut items, mut metas) =
            tree.materialize_level(&effective_segments, crate::app::BOOK_READING_PAGE_ORDER);
        crate::grid_item::arrange_grid_items(
            &mut items,
            &mut metas,
            &self.settings.grid_display_order,
            Some(crate::app::BOOK_READING_PAGE_ORDER),
        );
        let index = items
            .iter()
            .position(|item| {
                matches!(
                    item,
                    crate::grid_item::GridItem::ZipImage {
                        entry_name: candidate,
                        ..
                    } if candidate == entry_name
                )
            })
            .ok_or_else(|| {
                RemoteWriteError::new(
                    RemoteWriteErrorCode::NotFound,
                    "ZIP 内のページが見つかりません",
                )
            })?;
        let image_position = items[..=index]
            .iter()
            .filter(|item| item.has_page_data())
            .count()
            .saturating_sub(1);
        let image_count = items.iter().filter(|item| item.has_page_data()).count();
        validated_context(
            index,
            image_count,
            items.iter().all(|item| item.has_page_data()),
            effective_segments == root_segments,
            true,
        )
        .map(|mut context| {
            context.page_number =
                u32::try_from(image_position.saturating_add(1)).unwrap_or(u32::MAX);
            context
        })
    }

    fn validate_pdf_page(
        &self,
        address: &RemoteAddress,
        context_address: &RemoteAddress,
        page_number: u32,
    ) -> Result<ValidatedPageContext, RemoteWriteError> {
        if address.relative_path != context_address.relative_path
            || !matches!(context_address.subresource, RemoteSubresource::File)
        {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::BadRequest,
                "PDF ページとコンテキストが一致しません",
            ));
        }
        let resolved = self
            .resolve(address)
            .map_err(remote_write_error_from_media)?;
        let metadata = std::fs::metadata(&resolved.canonical).map_err(|_| {
            RemoteWriteError::new(RemoteWriteErrorCode::NotFound, "PDF が見つかりません")
        })?;
        let page_count = self
            .pdf_page_count(&resolved, &metadata)
            .map_err(remote_write_error_from_media)?;
        validate_page_number(page_number, page_count).map_err(remote_write_error_from_media)?;
        validated_context(page_number as usize, page_count as usize, true, true, true)
    }

    pub(super) fn thumbnail(
        &self,
        request: &mimageviewer_ipc::ThumbnailRequest,
        context: &WorkerContext,
    ) -> ThumbnailResponse {
        let started = Instant::now();
        let source_kind = media_source_kind(&request.address);
        let resolved = match self.resolve(&request.address) {
            Ok(resolved) => resolved,
            Err(error) => return thumbnail_error_from_media(error),
        };
        let response = match self.load_image(
            &request.address,
            &resolved,
            request.target_px,
            false,
            false,
            context,
            None,
            None,
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
        };
        let (outcome, output_bytes) = match &response {
            ThumbnailResponse::Success { webp_bytes } => ("ok", webp_bytes.len()),
            ThumbnailResponse::Error(_) => ("error", 0),
        };
        crate::logger::log(format!(
            "remote_ipc: media_operation operation=thumbnail source_kind={source_kind} outcome={outcome} duration_ms={:.1} output_bytes={output_bytes}",
            started.elapsed().as_secs_f64() * 1000.0
        ));
        response
    }

    pub(super) fn page_with_session_cancel(
        &self,
        request: PageRequest,
        context: &WorkerContext,
        session_cancel: Arc<AtomicBool>,
    ) -> PageResponse {
        self.page_inner(request, context, Some(session_cancel))
    }

    fn page_inner(
        &self,
        request: PageRequest,
        context: &WorkerContext,
        session_cancel: Option<Arc<AtomicBool>>,
    ) -> PageResponse {
        let started = Instant::now();
        let source_kind = media_source_kind(&request.address);
        let priority = request.priority;
        if request.target_px == 0 || request.target_px > MAX_PAGE_RENDER_PX {
            return PageResponse::Error(media_error(
                MediaErrorCode::BadRequest,
                "画像サイズが範囲外です",
            ));
        }
        if let Some(preview) = request.adjustment_preview.as_ref()
            && let Err(message) = super::apply_remote_adjustment_values(
                crate::adjustment::AdjustParams::default(),
                &preview.values,
            )
        {
            return PageResponse::Error(media_error(MediaErrorCode::BadRequest, message));
        }
        let resolved = match self.resolve(&request.address) {
            Ok(resolved) => resolved,
            Err(error) => return PageResponse::Error(error),
        };
        let cancel = self.begin_page_render(priority, session_cancel);
        let response = match self.load_image(
            &request.address,
            &resolved,
            request.target_px,
            true,
            priority == PagePriority::Foreground,
            context,
            Some(&cancel),
            request.adjustment_preview.as_ref(),
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
        };
        let (outcome, output_bytes) = match &response {
            PageResponse::Success(payload) => ("ok", payload.bytes.len()),
            PageResponse::Error(_) => ("error", 0),
        };
        self.finish_page_render(priority, &cancel);
        crate::logger::log(format!(
            "remote_ipc: media_operation operation=page source_kind={source_kind} priority={} outcome={outcome} duration_ms={:.1} output_bytes={output_bytes}",
            if priority == PagePriority::Prefetch {
                "prefetch"
            } else {
                "foreground"
            },
            started.elapsed().as_secs_f64() * 1000.0
        ));
        response
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

    pub(super) fn execute_remote_ai(
        &self,
        request: &RemoteAiStartRequest,
        progress: &dyn super::ai_job::RemoteAiProgressSink,
        cancel: &Arc<AtomicBool>,
    ) -> super::ai_job::RemoteAiExecutionOutcome {
        match self.execute_remote_ai_inner(request, progress, cancel) {
            Ok(results) => super::ai_job::RemoteAiExecutionOutcome::Ready(results),
            Err(RemoteAiRunError::NotApplicable {
                code,
                message,
                page_index,
            }) => super::ai_job::RemoteAiExecutionOutcome::NotApplicable {
                code,
                message,
                page_index,
            },
            Err(RemoteAiRunError::Superseded(message)) => {
                super::ai_job::RemoteAiExecutionOutcome::Superseded(message)
            }
            Err(RemoteAiRunError::Failed(message)) => {
                super::ai_job::RemoteAiExecutionOutcome::Failed(message)
            }
        }
    }

    fn execute_remote_ai_inner(
        &self,
        request: &RemoteAiStartRequest,
        progress: &dyn super::ai_job::RemoteAiProgressSink,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Vec<PagePayload>, RemoteAiRunError> {
        self.execute_remote_ai_inner_with(
            request,
            progress,
            cancel,
            &|engine, address, logical_path, mtime, file_size, target_px, context| {
                engine.prepare_remote_composite(
                    address,
                    logical_path,
                    mtime,
                    file_size,
                    target_px,
                    None,
                    context,
                )
            },
            &|engine, address, resolved, metadata, page_index, cancel| {
                engine.decode_remote_ai_source(address, resolved, metadata, page_index, cancel)
            },
            &|engine| {
                engine
                    .session
                    .as_ref()
                    .and_then(super::session::SessionHandle::remote_ai_resources)
            },
        )
    }

    fn execute_remote_ai_inner_with(
        &self,
        request: &RemoteAiStartRequest,
        progress: &dyn super::ai_job::RemoteAiProgressSink,
        cancel: &Arc<AtomicBool>,
        prepare_composite: &dyn Fn(
            &Self,
            &RemoteAddress,
            &Path,
            i64,
            i64,
            u32,
            &WorkerContext,
        ) -> Result<Option<RemotePreparedComposite>, MediaError>,
        decode_source: &dyn Fn(
            &Self,
            &RemoteAddress,
            &ResolvedFavoritePath,
            &std::fs::Metadata,
            usize,
            &Arc<AtomicBool>,
        )
            -> Result<(Arc<egui::ColorImage>, [usize; 2]), RemoteAiRunError>,
        resources_for_remote: &dyn Fn(&Self) -> Option<super::session::RemoteAiResources>,
    ) -> Result<Vec<PagePayload>, RemoteAiRunError> {
        let context = WorkerContext::open();
        let page_count = request.pages.len();
        let mut results = Vec::with_capacity(page_count);
        let mut identities = Vec::with_capacity(page_count);

        for (page_index, page) in request.pages.iter().enumerate() {
            check_remote_ai_cancel(cancel)?;
            progress.update(
                mimageviewer_ipc::RemoteAiJobState::PreparingSource,
                Some(remote_ai_progress(
                    RemoteAiProgressPhase::PreparingSource,
                    page_index,
                    page_count,
                    0,
                    None,
                )),
            );
            let resolved = self.resolve(&page.address).map_err(remote_ai_media_error)?;
            let metadata = std::fs::metadata(&resolved.canonical)
                .map_err(|error| RemoteAiRunError::Failed(format!("source metadata: {error}")))?;
            if !metadata.is_file() {
                return Err(RemoteAiRunError::Failed(
                    "AI source is not a file".to_owned(),
                ));
            }
            let mtime = crate::ui_helpers::mtime_secs(&metadata);
            let file_size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
            let prepared = prepare_composite(
                self,
                &page.address,
                &resolved.logical,
                mtime,
                file_size,
                page.target_px,
                &context,
            )
            .map_err(remote_ai_media_error)?
            .ok_or_else(|| {
                RemoteAiRunError::Failed("address does not identify an image page".to_owned())
            })?;
            let (source, source_dims) = decode_source(
                self,
                &page.address,
                &resolved,
                &metadata,
                page_index,
                cancel,
            )?;
            check_remote_ai_cancel(cancel)?;
            let pre_ai_edit_fingerprint = prepared.edits.pre_ai_fingerprint;
            let materialized = self
                .execute_remote_edits(source, prepared.edits.clone(), cancel)
                .map_err(remote_ai_media_error)?;
            let selected = crate::ai::final_pipeline::select_final_ai_models(
                &materialized.pixels,
                &prepared.params,
                prepared.settings.ai_feature_mode,
                prepared.settings.ai_upscale_limit,
                prepared.settings.ai_denoise_limit,
            )
            .ok_or_else(|| RemoteAiRunError::NotApplicable {
                code: RemoteAiTerminalCode::SizeGate,
                message: "AI is disabled or the source is outside the configured size gate"
                    .to_owned(),
                page_index,
            })?;
            // Animated/vector/size-gated sources terminate above without initializing or loading
            // the shared runtime. Only an applicable final-AI request claims the bridge.
            let resources = resources_for_remote(self)
                .ok_or_else(|| RemoteAiRunError::Failed("AI runtime is unavailable".to_owned()))?;

            let native_key = RemoteAiNativeCacheKey {
                page_key: prepared.key.page_key.clone(),
                mtime,
                file_size,
                source_size: materialized.pixels.size,
                pre_ai_params: remote_ai_pre_params(&prepared.params),
                pre_ai_edit_fingerprint,
                ai_feature_mode: prepared.settings.ai_feature_mode,
                ai_upscale_limit: prepared.settings.ai_upscale_limit,
                ai_denoise_limit: prepared.settings.ai_denoise_limit,
                ai_backend: prepared.settings.ai_backend.clone(),
                background_mode: resources.background_mode,
                pipeline_schema: REMOTE_AI_PIPELINE_SCHEMA,
                model_epoch: remote_ai_model_epoch(
                    &resources.runtime,
                    &resources.manager,
                    selected,
                ),
            };
            let (max_entries, max_bytes) =
                remote_ai_native_budget(&prepared.settings).unwrap_or((0, 0));
            let cached = {
                let mut cache = self
                    .remote_ai_native_cache
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                // Re-apply the live retained-cache settings even on a hit. Otherwise lowering
                // the configured budget would not affect remote entries until the next miss.
                cache.enforce_budget(max_entries, max_bytes);
                cache.get(&native_key)
            };
            let (native_pixels, used_upscale) = match cached {
                Some(hit) => hit,
                None => {
                    let progress_adapter = ContainerFinalAiProgress {
                        sink: progress,
                        page_index,
                        page_count,
                    };
                    let output = crate::ai::final_pipeline::execute_selected_final_ai(
                        &resources.runtime,
                        &resources.manager,
                        crate::ai::final_pipeline::FinalAiExecutionRequest {
                            source: Arc::clone(&materialized.pixels),
                            adjust_before_ai: (!prepared.params.is_color_identity())
                                .then(|| prepared.params.clone()),
                            denoise_kind: selected.denoise,
                            upscale_kind: selected.upscale,
                            background_mode: resources.background_mode,
                        },
                        cancel,
                        &progress_adapter,
                    )
                    .map_err(|error| match error {
                        crate::ai::final_pipeline::FinalAiExecutionError::Cancelled => {
                            RemoteAiRunError::Failed("AI job was cancelled".to_owned())
                        }
                        crate::ai::final_pipeline::FinalAiExecutionError::Failed(error) => {
                            RemoteAiRunError::Failed(error)
                        }
                    })?;
                    let pixels = Arc::new(output.image);
                    self.remote_ai_native_cache
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .insert(
                            native_key,
                            Arc::clone(&pixels),
                            output.used_upscale,
                            max_entries,
                            max_bytes,
                        );
                    (pixels, output.used_upscale)
                }
            };

            progress.update(
                mimageviewer_ipc::RemoteAiJobState::Finalizing,
                Some(remote_ai_progress(
                    RemoteAiProgressPhase::Finalizing,
                    page_index,
                    page_count,
                    4,
                    None,
                )),
            );
            check_remote_ai_cancel(cancel)?;
            let lut = self
                .resolve_remote_lut(prepared.lut_entry.as_ref())
                .map_err(remote_ai_media_error)?;
            let plan = crate::final_composite::build_final_composite_plan_after_ai(
                &prepared.params,
                lut.map(|lut| (lut, prepared.params.creative_lut.strength)),
                used_upscale,
            );
            let mut pixels = match crate::final_composite::execute_final_composite(
                native_pixels,
                plan,
                cancel,
            ) {
                crate::final_composite::FinalCompositeResult::Ready { pixels, .. } => pixels,
                crate::final_composite::FinalCompositeResult::Cancelled => {
                    return Err(RemoteAiRunError::Failed("AI job was cancelled".to_owned()));
                }
            };
            if !materialized.comic.is_empty()
                && let Some(fonts) = crate::comic_overlay::load_comic_fonts_for(&materialized.comic)
            {
                pixels = crate::edit_source::comic_composite(
                    &pixels,
                    &materialized.comic,
                    source_dims,
                    &fonts,
                    &mut self
                        .comic_stamp_cache
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()),
                    cancel,
                );
            }
            if let Some(crop) = materialized.export_crop {
                let rect =
                    crate::edit_source::export_crop_rect_for_pixels(crop, source_dims, pixels.size);
                pixels = Arc::new(
                    crate::export_crop::crop_color_image(&pixels, rect)
                        .map_err(|error| RemoteAiRunError::Failed(error.to_string()))?,
                );
            }
            let image = loaded_image_from_color_image(&pixels).map_err(remote_ai_media_error)?;
            let (bytes, width, height) =
                crate::catalog::encode_thumb_webp(&image.image, page.target_px, PAGE_WEBP_QUALITY)
                    .ok_or_else(|| RemoteAiRunError::Failed("WebP encoding failed".to_owned()))?;
            results.push(PagePayload {
                bytes,
                content_type: "image/webp".to_owned(),
                width,
                height,
            });
            identities.push((
                page.address.clone(),
                page.target_px,
                RemoteAiResultIdentity::from_prepared(&prepared, resources.background_mode),
            ));
        }

        // Result bytes are publishable only if source/edit/settings still match the snapshots
        // that produced them. Re-open worker DB handles so this is a true completion-time read.
        let validation_context = WorkerContext::open();
        let current_background = self
            .session
            .as_ref()
            .and_then(super::session::SessionHandle::remote_ai_resources)
            .map(|resources| resources.background_mode)
            .ok_or_else(|| RemoteAiRunError::Superseded("AI runtime was detached".to_owned()))?;
        for (address, target_px, expected) in identities {
            check_remote_ai_cancel(cancel)?;
            let resolved = self.resolve(&address).map_err(|_| {
                RemoteAiRunError::Superseded("source is no longer available".to_owned())
            })?;
            let metadata = std::fs::metadata(&resolved.canonical)
                .map_err(|_| RemoteAiRunError::Superseded("source metadata changed".to_owned()))?;
            let current = self
                .prepare_remote_composite(
                    &address,
                    &resolved.logical,
                    crate::ui_helpers::mtime_secs(&metadata),
                    i64::try_from(metadata.len()).unwrap_or(i64::MAX),
                    target_px,
                    None,
                    &validation_context,
                )
                .map_err(|_| {
                    RemoteAiRunError::Superseded("source snapshot cannot be revalidated".to_owned())
                })?
                .map(|prepared| {
                    RemoteAiResultIdentity::from_prepared(&prepared, current_background)
                });
            if current.as_ref() != Some(&expected) {
                return Err(RemoteAiRunError::Superseded(
                    "source, edits, or AI settings changed while the job was running".to_owned(),
                ));
            }
        }
        Ok(results)
    }

    fn decode_remote_ai_source(
        &self,
        address: &RemoteAddress,
        resolved: &ResolvedFavoritePath,
        metadata: &std::fs::Metadata,
        page_index: usize,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(Arc<egui::ColorImage>, [usize; 2]), RemoteAiRunError> {
        match &address.subresource {
            RemoteSubresource::File if is_image_path(&resolved.logical) => {
                decode_remote_ai_canonical(
                    crate::canonical_image_loader::CanonicalImageSource::File {
                        path: &resolved.canonical,
                        verified_bytes: None,
                    },
                    page_index,
                    cancel,
                )
            }
            RemoteSubresource::ZipEntry { entry_name } if is_zip_path(&resolved.logical) => {
                decode_remote_ai_canonical(
                    crate::canonical_image_loader::CanonicalImageSource::ArchiveEntry {
                        archive_path: &resolved.canonical,
                        entry_name,
                    },
                    page_index,
                    cancel,
                )
            }
            RemoteSubresource::PdfPage { page_number } if is_pdf_path(&resolved.logical) => {
                self.ensure_pdf_page_in_range(resolved, metadata, *page_number)
                    .map_err(remote_ai_media_error)?;
                let password = self.pdf_passwords.get(&resolved.logical);
                let analysis = crate::pdf_loader::analyze_page_content_type(
                    &resolved.canonical,
                    *page_number,
                    password.as_deref(),
                    Some(Arc::clone(cancel)),
                )
                .map_err(|error| RemoteAiRunError::Failed(error.to_string()))?;
                if matches!(
                    analysis.content_type,
                    crate::pdf_loader::PdfPageContentType::Vector
                ) {
                    return Err(RemoteAiRunError::NotApplicable {
                        code: RemoteAiTerminalCode::VectorPdf,
                        message: "vector PDF pages are not AI raster sources".to_owned(),
                        page_index,
                    });
                }
                match crate::pdf_loader::render_page_canonical_raster(
                    &resolved.canonical,
                    *page_number,
                    analysis.content_type,
                    password.as_deref(),
                    Some(Arc::clone(cancel)),
                    crate::pdf_loader::JobPriority::Normal,
                    0,
                    crate::pdf_loader::CancelWaitPolicy::AbortOnCancel,
                )
                .map_err(|error| RemoteAiRunError::Failed(error.to_string()))?
                {
                    crate::pdf_loader::CanonicalPdfPage::Vector => {
                        Err(RemoteAiRunError::NotApplicable {
                            code: RemoteAiTerminalCode::VectorPdf,
                            message: "PDF page became vector during canonical render".to_owned(),
                            page_index,
                        })
                    }
                    crate::pdf_loader::CanonicalPdfPage::Raster {
                        image, native_dims, ..
                    } => Ok((
                        Arc::new(crate::canonical_image_loader::dynamic_image_to_color_image(
                            &image,
                        )),
                        [native_dims[0] as usize, native_dims[1] as usize],
                    )),
                }
            }
            _ => Err(RemoteAiRunError::Failed(
                "address is not a supported still-image page".to_owned(),
            )),
        }
    }

    fn enumerate(
        &self,
        request: &ContainerRequest,
        resolved: &ResolvedFavoritePath,
    ) -> Result<ContainerPayload, MediaError> {
        let metadata = std::fs::metadata(&resolved.canonical)
            .map_err(|_| media_error(MediaErrorCode::NotFound, "コンテナが見つかりません"))?;
        if metadata.is_dir() {
            return self.enumerate_folder(request, resolved);
        }
        if !metadata.is_file() {
            return Err(media_error(
                MediaErrorCode::Unsupported,
                "対象はフォルダまたは ZIP/PDF ファイルではありません",
            ));
        }
        if is_zip_path(&resolved.logical) {
            self.enumerate_zip(request, resolved)
        } else if is_pdf_path(&resolved.logical) {
            self.enumerate_pdf(request, resolved, &metadata)
        } else {
            Err(media_error(
                MediaErrorCode::Unsupported,
                "このコンテナ形式には対応していません",
            ))
        }
    }

    fn resume_page_for_items(
        &self,
        container: &RemoteAddress,
        container_path: &Path,
        items: &[crate::grid_item::GridItem],
        resume_supported: bool,
    ) -> Option<RemoteAddress> {
        if !resume_supported {
            return None;
        }
        let Some(reader) = self.resume_reader.as_ref() else {
            return None;
        };
        let saved = match reader.read_book_resume(container_path) {
            Ok(saved) => saved,
            Err(error) => {
                crate::logger::log(format!(
                    "remote_ipc: book resume read failed; falling back to first page error={error:?}"
                ));
                return None;
            }
        };
        let Some(saved) = saved else {
            return None;
        };
        let resolved = resolve_resume_page(container, items, saved);
        if resolved.is_none() {
            crate::logger::log(format!(
                "remote_ipc: saved book resume is outside the current readable pages saved_index={saved} item_count={}",
                items.len()
            ));
        }
        resolved
    }

    fn container_open_mode(&self, auto_open: bool) -> ContainerOpenMode {
        if !auto_open {
            ContainerOpenMode::Grid
        } else if self.settings.book_open_resume.resumes() {
            ContainerOpenMode::ResumePage
        } else {
            ContainerOpenMode::FirstPage
        }
    }

    fn enumerate_folder(
        &self,
        request: &ContainerRequest,
        resolved: &ResolvedFavoritePath,
    ) -> Result<ContainerPayload, MediaError> {
        if !matches!(request.address.subresource, RemoteSubresource::File) {
            return Err(media_error(
                MediaErrorCode::BadRequest,
                "画像フォルダの一覧アドレスが不正です",
            ));
        }
        let listing = self
            .recompute_folder_listing(&resolved.logical)
            .map_err(media_error_from_remote_write)?;
        let resume_page =
            self.resume_page_for_items(&request.address, &resolved.logical, &listing.items, true);
        let total = listing
            .items
            .iter()
            .filter(|item| matches!(item, crate::grid_item::GridItem::Image(_)))
            .count();
        let auto_open = listing.image_only && self.settings.auto_fullscreen_image_folders_enabled();
        let items = listing
            .items
            .into_iter()
            .filter(|item| matches!(item, crate::grid_item::GridItem::Image(_)))
            .take(CONTAINER_ENTRY_LIMIT)
            .collect::<Vec<_>>();
        let entries = items
            .iter()
            .filter_map(|item| {
                Some(ContainerEntry {
                    address: grid_item_address(&request.address, item)?,
                    name: item.name().into_owned(),
                    kind: ContainerEntryKind::Image,
                    page_count: None,
                })
            })
            .collect::<Vec<_>>();
        let spread = self.spread_payload(request, resolved, &items, None);
        Ok(ContainerPayload {
            title: container_title(&resolved.logical),
            kind: ContainerKind::Folder,
            effective_address: request.address.clone(),
            entries,
            thumb_aspect_height_ratio: super::collections::aggregate_thumb_aspect_height_ratio(
                &self.settings,
            ),
            resume_page,
            open_mode: self.container_open_mode(auto_open),
            configured_spread_mode: spread.configured,
            effective_spread_mode: spread.effective,
            reading_direction: spread.reading_direction,
            spread_page_gap_px: self.settings.spread_page_gap_px,
            page_groups: spread.groups,
            entry_limit: CONTAINER_ENTRY_LIMIT,
            truncated: total > CONTAINER_ENTRY_LIMIT,
        })
    }

    fn enumerate_zip(
        &self,
        request: &ContainerRequest,
        resolved: &ResolvedFavoritePath,
    ) -> Result<ContainerPayload, MediaError> {
        let address = &request.address;
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
        let enumeration_started = Instant::now();
        let enumeration = crate::zip_loader::enumerate_image_entries_detailed(&resolved.logical)
            .map_err(|error| {
                crate::logger::log(format!("remote_ipc: zip_enumerate_failed error={error}"));
                media_error(MediaErrorCode::RenderFailed, "ZIP を列挙できませんでした")
            })?;
        crate::logger::log(format!(
            "remote_ipc: zip_enumerate_complete duration_ms={:.1} raw_entry_count={}",
            enumeration_started.elapsed().as_secs_f64() * 1000.0,
            enumeration.entries.len()
        ));
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
        let root_segments = tree.collapse_redundant(&[]);
        let effective_prefix = zip_prefix(&effective_segments);
        let (items, _) =
            tree.materialize_level(&effective_segments, crate::app::BOOK_READING_PAGE_ORDER);
        let total = items.len();
        let at_resume_root = effective_segments == root_segments;
        let resume_page =
            self.resume_page_for_items(address, &resolved.logical, &items, at_resume_root);
        let items = items
            .into_iter()
            .take(CONTAINER_ENTRY_LIMIT)
            .collect::<Vec<_>>();
        let entries = items
            .iter()
            .filter_map(|item| {
                let name = item.name().into_owned();
                match item {
                    crate::grid_item::GridItem::ZipDir { dir_prefix, .. } => Some(ContainerEntry {
                        name,
                        page_count: tree.page_count_for_prefix_str(&dir_prefix),
                        address: RemoteAddress {
                            favorite_id: address.favorite_id.clone(),
                            relative_path: address.relative_path.clone(),
                            subresource: RemoteSubresource::ZipDirectory {
                                prefix: dir_prefix.clone(),
                            },
                        },
                        kind: ContainerEntryKind::Directory,
                    }),
                    crate::grid_item::GridItem::ZipImage { entry_name, .. } => {
                        Some(ContainerEntry {
                            name,
                            page_count: None,
                            address: zip_entry_address(address, entry_name),
                            kind: ContainerEntryKind::Image,
                        })
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>();
        let spread = self.spread_payload(
            request,
            resolved,
            &items,
            Some((&effective_segments, &resolved.logical)),
        );
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
            thumb_aspect_height_ratio: super::collections::aggregate_thumb_aspect_height_ratio(
                &self.settings,
            ),
            resume_page,
            open_mode: self.container_open_mode(
                at_resume_root && self.settings.effective_auto_fullscreen_zip_pdf(),
            ),
            configured_spread_mode: spread.configured,
            effective_spread_mode: spread.effective,
            reading_direction: spread.reading_direction,
            spread_page_gap_px: self.settings.spread_page_gap_px,
            page_groups: spread.groups,
            entry_limit: CONTAINER_ENTRY_LIMIT,
            truncated: total > CONTAINER_ENTRY_LIMIT,
        })
    }

    fn enumerate_pdf(
        &self,
        request: &ContainerRequest,
        resolved: &ResolvedFavoritePath,
        metadata: &std::fs::Metadata,
    ) -> Result<ContainerPayload, MediaError> {
        let address = &request.address;
        if !matches!(address.subresource, RemoteSubresource::File) {
            return Err(media_error(
                MediaErrorCode::BadRequest,
                "PDF の一覧アドレスが不正です",
            ));
        }
        let page_count = self.pdf_page_count(resolved, metadata)?;
        let page_numbers = (0..page_count)
            .take(CONTAINER_ENTRY_LIMIT)
            .collect::<Vec<_>>();
        let items = page_numbers
            .iter()
            .map(|page_number| crate::grid_item::GridItem::PdfPage {
                pdf_path: resolved.logical.clone(),
                page_num: *page_number,
                content_type: None,
            })
            .collect::<Vec<_>>();
        let resume_page = self.resume_page_for_items(address, &resolved.logical, &items, true);
        let entries = page_numbers
            .into_iter()
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
            .collect::<Vec<_>>();
        let spread = self.spread_payload(request, resolved, &items, None);
        Ok(ContainerPayload {
            title: container_title(&resolved.logical),
            kind: ContainerKind::Pdf,
            effective_address: address.clone(),
            entries,
            thumb_aspect_height_ratio: super::collections::aggregate_thumb_aspect_height_ratio(
                &self.settings,
            ),
            resume_page,
            open_mode: self.container_open_mode(self.settings.effective_auto_fullscreen_zip_pdf()),
            configured_spread_mode: spread.configured,
            effective_spread_mode: spread.effective,
            reading_direction: spread.reading_direction,
            spread_page_gap_px: self.settings.spread_page_gap_px,
            page_groups: spread.groups,
            entry_limit: CONTAINER_ENTRY_LIMIT,
            truncated: page_count as usize > CONTAINER_ENTRY_LIMIT,
        })
    }

    fn spread_payload(
        &self,
        request: &ContainerRequest,
        resolved: &ResolvedFavoritePath,
        items: &[crate::grid_item::GridItem],
        zip_context: Option<(&[String], &Path)>,
    ) -> SpreadPayload {
        let key = if let Some((segments, root)) = zip_context {
            crate::spread_db::container_key_with_fallback(root, segments)
        } else {
            crate::spread_db::container_key_with_fallback(&resolved.logical, &[])
        };
        let (stored_mode, stored_direction) =
            self.stored_spread_state(&key.exact, key.fallback.as_deref());
        let (configured, effective, reading_direction) = resolve_spread_state(
            request.spread_mode,
            request.reading_direction,
            stored_mode,
            stored_direction,
            self.settings.default_spread_mode,
            self.settings.default_reading_direction,
            request.force_single_page,
        );
        let landscape = self.cached_landscape_flags(&resolved.logical, items);
        let index_groups = crate::ui_fullscreen::build_remote_spread_page_groups(
            items,
            core_spread_mode(effective),
            &landscape,
        );
        let groups = index_groups
            .into_iter()
            .filter_map(|indices| {
                let pages = indices
                    .into_iter()
                    .filter_map(|index| grid_item_address(&request.address, items.get(index)?))
                    .collect::<Vec<_>>();
                let anchor = if effective.is_rtl() && pages.len() == 2 {
                    pages.get(1).cloned()
                } else {
                    pages.first().cloned()
                }?;
                Some(PageGroup { anchor, pages })
            })
            .collect::<Vec<_>>();
        SpreadPayload {
            configured,
            effective,
            reading_direction,
            groups,
        }
    }

    fn stored_spread_state(
        &self,
        key: &Path,
        fallback: Option<&Path>,
    ) -> (
        Option<crate::settings::SpreadMode>,
        Option<crate::settings::ReadingDirection>,
    ) {
        let db = self
            .spread_db
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let stored = db
            .as_ref()
            .map(|db| db.get_state_with_fallback(key, fallback))
            .unwrap_or_default();
        (stored.mode, stored.direction)
    }

    fn cached_landscape_flags(
        &self,
        container_path: &Path,
        items: &[crate::grid_item::GridItem],
    ) -> Vec<bool> {
        let cached = crate::catalog::CatalogDb::open_existing_read_only(
            &crate::catalog::default_cache_dir(),
            container_path,
        )
        .ok()
        .flatten()
        .and_then(|catalog| catalog.load_all().ok())
        .unwrap_or_default();
        items
            .iter()
            .map(|item| {
                let key = match item {
                    crate::grid_item::GridItem::Image(path) => path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned),
                    crate::grid_item::GridItem::ZipImage { entry_name, .. } => {
                        Some(entry_name.clone())
                    }
                    crate::grid_item::GridItem::PdfPage { page_num, .. } => {
                        Some(crate::grid_item::pdf_page_cache_key(*page_num))
                    }
                    _ => None,
                };
                key.and_then(|key| cached.get(&key))
                    .and_then(|entry| {
                        entry
                            .source_dims
                            .or_else(|| crate::catalog::decode_thumb_dims(&entry.jpeg_data))
                    })
                    .is_some_and(|(width, height)| width > height)
            })
            .collect()
    }

    fn load_image(
        &self,
        address: &RemoteAddress,
        resolved: &ResolvedFavoritePath,
        target_px: u32,
        full_page: bool,
        foreground: bool,
        context: &WorkerContext,
        external_cancel: Option<&Arc<AtomicBool>>,
        adjustment_preview: Option<&mimageviewer_ipc::RemoteAdjustmentPreview>,
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
            // foreground でも HighNormal までとし、ローカル UI 用 Critical 予約枠は
            // 消費しない。prefetch は Normal lane へ分離する。
            priority: foreground,
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
            RemoteSubresource::File if is_image_path(&resolved.logical) => {
                resolved.logical.parent().ok_or_else(|| {
                    media_error(MediaErrorCode::PathRejected, "親フォルダを解決できません")
                })?
            }
            RemoteSubresource::File => {
                return Err(media_error(
                    MediaErrorCode::Unsupported,
                    "対象は画像または ZIP/PDF ではありません",
                ));
            }
            _ => {
                return Err(media_error(
                    MediaErrorCode::BadRequest,
                    "コンテナ種別と内部アドレスが一致しません",
                ));
            }
        };

        let prepared_composite = if full_page {
            self.prepare_remote_composite(
                address,
                &resolved.logical,
                mtime,
                file_size,
                target_px,
                adjustment_preview,
                context,
            )?
        } else {
            None
        };
        if let Some(prepared) = prepared_composite.as_ref()
            && let Some(pixels) = self
                .page_composite_cache
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&prepared.key)
        {
            if external_cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
                return Err(media_error(
                    MediaErrorCode::Busy,
                    "先読みは新しいページ要求に置き換えられました",
                ));
            }
            crate::logger::log(format!(
                "remote_ipc: final_composite cache=hit key={}",
                prepared.key.page_key
            ));
            return loaded_image_from_color_image(&pixels);
        }

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
        let cancel = external_cancel
            .cloned()
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
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
        let mut saw_canceled = false;
        let (color_image, decoded_source_dims) = rx
            .into_iter()
            .find_map(|message| {
                saw_canceled |= message.canceled;
                if !message.finalized && !message.canceled {
                    message.image.map(|image| (image, message.source_dims))
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                if saw_canceled || cancel.load(Ordering::Relaxed) {
                    media_error(
                        MediaErrorCode::Busy,
                        "先読みは新しいページ要求に置き換えられました",
                    )
                } else if matches!(address.subresource, RemoteSubresource::ZipDirectory { .. }) {
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
        let source_dims = decoded_source_dims
            .map(|(width, height)| [width as usize, height as usize])
            .unwrap_or(color_image.size);
        let mut pixels = Arc::new(color_image);
        if let Some(prepared) = prepared_composite {
            let edit_started = Instant::now();
            let materialized = self.execute_remote_edits(pixels, prepared.edits, &cancel)?;
            pixels = materialized.pixels;
            crate::logger::log(format!(
                "remote_ipc: edit_materialize elapsed_ms={:.1} erase_ms={:.1} local_adjust_ms={:.1} conceal_ms={:.1} diffusion_fallback={}",
                edit_started.elapsed().as_secs_f64() * 1000.0,
                materialized.timing.erase_ms,
                materialized.timing.local_adjust_ms,
                materialized.timing.conceal_ms,
                materialized.used_diffusion_fallback,
            ));
            let lut = self.resolve_remote_lut(prepared.lut_entry.as_ref())?;
            pixels = execute_remote_composite(pixels, &prepared.params, lut, &cancel)?;
            if !materialized.comic.is_empty()
                && let Some(fonts) = crate::comic_overlay::load_comic_fonts_for(&materialized.comic)
            {
                pixels = crate::edit_source::comic_composite(
                    &pixels,
                    &materialized.comic,
                    source_dims,
                    &fonts,
                    &mut self
                        .comic_stamp_cache
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()),
                    &cancel,
                );
            }
            if let Some(crop) = materialized.export_crop {
                let rect =
                    crate::edit_source::export_crop_rect_for_pixels(crop, source_dims, pixels.size);
                pixels = Arc::new(crate::export_crop::crop_color_image(&pixels, rect).map_err(
                    |error| {
                        crate::logger::log(format!("remote_ipc: export crop failed: {error}"));
                        media_error(
                            MediaErrorCode::RenderFailed,
                            "ページの切り取り結果を作成できませんでした",
                        )
                    },
                )?);
            }
            self.page_composite_cache
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(prepared.key.clone(), Arc::clone(&pixels));
            crate::logger::log(format!(
                "remote_ipc: final_composite cache=miss key={}",
                prepared.key.page_key
            ));
        }
        loaded_image_from_color_image(&pixels)
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

fn load_mask_snapshot(
    db: &crate::mask_db::MaskDb,
    page_key: &str,
) -> Result<Option<crate::edit_source::MaskSnapshot>, MediaError> {
    let loaded = db
        .get_full_checked(page_key)
        .map_err(|error| remote_edit_db_read_error("erase", error))?;
    Ok(
        loaded.map(|(bitmap, shapes, size)| crate::edit_source::MaskSnapshot {
            bitmap,
            shapes,
            size,
        }),
    )
}

fn load_conceal_snapshot(
    db: &crate::conceal_db::ConcealDb,
    page_key: &str,
) -> Result<Option<crate::edit_source::MaskSnapshot>, MediaError> {
    let loaded = db
        .get_full_checked(page_key)
        .map_err(|error| remote_edit_db_read_error("conceal", error))?;
    Ok(
        loaded.map(|(bitmap, shapes, size)| crate::edit_source::MaskSnapshot {
            bitmap,
            shapes,
            size,
        }),
    )
}

fn remote_edit_fingerprint(
    erase: Option<&crate::edit_source::MaskSnapshot>,
    local_adjust: Option<&Vec<local_adjust_core::LocalAdjustmentLayer>>,
    conceal: Option<&crate::edit_source::MaskSnapshot>,
    conceal_preset: &crate::conceal::ConcealPreset,
    comic: &[comic_core::AnnotationObject],
    export_crop: Option<&crate::export_crop::CropSettings>,
) -> Result<[u8; 32], MediaError> {
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    hash_remote_edit_value(
        &mut digest,
        b"erase",
        &erase.map(|mask| (&mask.bitmap, &mask.shapes, mask.size)),
    )?;
    hash_remote_edit_value(&mut digest, b"local", &local_adjust)?;
    hash_remote_edit_value(
        &mut digest,
        b"conceal",
        &conceal.map(|mask| (&mask.bitmap, &mask.shapes, mask.size)),
    )?;
    if conceal.is_some() {
        hash_remote_edit_value(&mut digest, b"conceal-preset", conceal_preset)?;
    }
    hash_remote_edit_value(&mut digest, b"comic", &comic)?;
    hash_remote_edit_value(&mut digest, b"crop", &export_crop)?;
    Ok(digest.finalize().into())
}

fn remote_pre_ai_edit_fingerprint(
    erase: Option<&crate::edit_source::MaskSnapshot>,
    local_adjust: Option<&Vec<local_adjust_core::LocalAdjustmentLayer>>,
    conceal: Option<&crate::edit_source::MaskSnapshot>,
    conceal_preset: &crate::conceal::ConcealPreset,
) -> Result<[u8; 32], MediaError> {
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    hash_remote_edit_value(
        &mut digest,
        b"erase",
        &erase.map(|mask| (&mask.bitmap, &mask.shapes, mask.size)),
    )?;
    hash_remote_edit_value(&mut digest, b"local", &local_adjust)?;
    hash_remote_edit_value(
        &mut digest,
        b"conceal",
        &conceal.map(|mask| (&mask.bitmap, &mask.shapes, mask.size)),
    )?;
    if conceal.is_some() {
        hash_remote_edit_value(&mut digest, b"conceal-preset", conceal_preset)?;
    }
    Ok(digest.finalize().into())
}

fn hash_remote_edit_value<T: serde::Serialize>(
    digest: &mut sha2::Sha256,
    label: &[u8],
    value: &T,
) -> Result<(), MediaError> {
    use sha2::Digest;
    digest.update(label);
    let bytes = serde_json::to_vec(value).map_err(|error| {
        crate::logger::log(format!(
            "remote_ipc: edit snapshot fingerprint failed: {error}"
        ));
        media_error(
            MediaErrorCode::Internal,
            "編集結果のキャッシュキーを作成できませんでした",
        )
    })?;
    digest.update(bytes);
    Ok(())
}

fn remote_edit_db_open_error(kind: &str, error: rusqlite::Error) -> MediaError {
    crate::logger::log(format!(
        "remote_ipc: {kind} DB reopen failed for edit materialization: {error}"
    ));
    media_error(
        MediaErrorCode::Internal,
        "編集データベースを開けないためページを合成できません",
    )
}

fn remote_edit_db_read_error(kind: &str, error: String) -> MediaError {
    crate::logger::log(format!(
        "remote_ipc: {kind} DB read failed for edit materialization: {error}"
    ));
    media_error(
        MediaErrorCode::Internal,
        "編集データベースを読めないためページを合成できません",
    )
}

fn remote_adjustment_identity(
    address: &RemoteAddress,
    logical_path: &Path,
) -> Option<RemoteAdjustmentIdentity> {
    let page_key = crate::edit_source::page_key_for_remote(logical_path, &address.subresource)?;
    match &address.subresource {
        RemoteSubresource::File => {
            let location_path = if is_zip_path(logical_path) || is_pdf_path(logical_path) {
                logical_path.to_path_buf()
            } else {
                logical_path.parent()?.to_path_buf()
            };
            Some(RemoteAdjustmentIdentity {
                page_key,
                location_path,
                compiled_book: false,
            })
        }
        RemoteSubresource::ZipEntry { .. } => Some(RemoteAdjustmentIdentity {
            page_key,
            location_path: logical_path.to_path_buf(),
            compiled_book: false,
        }),
        RemoteSubresource::PdfPage { .. } => Some(RemoteAdjustmentIdentity {
            page_key,
            location_path: logical_path.to_path_buf(),
            compiled_book: false,
        }),
        RemoteSubresource::ZipDirectory { .. } => None,
    }
}

fn resolve_remote_effective_params(
    identity: &RemoteAdjustmentIdentity,
    page: Option<&crate::adjustment::AdjustParams>,
    favorites: &[crate::settings::FavoriteEntry],
    favorite_params: &HashMap<uuid::Uuid, crate::adjustment::AdjustParams>,
    global: &crate::adjustment::AdjustParams,
) -> crate::adjustment::AdjustParams {
    if identity.compiled_book {
        return page.cloned().unwrap_or_default();
    }
    crate::final_composite::resolve_effective_params(
        page,
        || {
            crate::final_composite::active_favorite_default_id_for_path(
                &identity.location_path,
                favorites,
                None,
                |id| favorite_params.contains_key(&id),
            )
            .and_then(|id| favorite_params.get(&id))
        },
        global,
    )
    .clone()
}

#[cfg(test)]
pub(crate) fn resolve_remote_effective_params_for_test(
    logical_path: &Path,
    subresource: &RemoteSubresource,
    page: Option<&crate::adjustment::AdjustParams>,
    favorites: &[crate::settings::FavoriteEntry],
    favorite_params: &HashMap<uuid::Uuid, crate::adjustment::AdjustParams>,
    global: &crate::adjustment::AdjustParams,
) -> crate::adjustment::AdjustParams {
    let address = RemoteAddress {
        favorite_id: String::new(),
        relative_path: String::new(),
        subresource: subresource.clone(),
    };
    let identity = remote_adjustment_identity(&address, logical_path)
        .expect("test subresource must identify a page");
    resolve_remote_effective_params(&identity, page, favorites, favorite_params, global)
}

fn execute_remote_composite(
    source: Arc<egui::ColorImage>,
    params: &crate::adjustment::AdjustParams,
    lut: Option<crate::creative_lut::SharedCreativeLut>,
    cancel: &AtomicBool,
) -> Result<Arc<egui::ColorImage>, MediaError> {
    let creative_lut = lut.map(|lut| (lut, params.creative_lut.strength));
    let plan = crate::final_composite::build_final_composite_plan_without_ai(params, creative_lut);
    match crate::final_composite::execute_final_composite(source, plan, cancel) {
        crate::final_composite::FinalCompositeResult::Ready {
            pixels,
            elapsed_ms,
            timing,
        } => {
            crate::logger::log(format!(
                "remote_ipc: final_composite elapsed_ms={elapsed_ms:.1} adjust_ms={:.1} sharpen_ms={:.1} colorize_check_ms={:.1} colorize_apply_ms={:.1} colorize_applied={} creative_lut_ms={:.1} post_filter_ms={:.1}",
                timing.adjust_ms,
                timing.sharpen_ms,
                timing.colorize_check_ms,
                timing.colorize_apply_ms,
                timing.colorize_applied,
                timing.creative_lut_ms,
                timing.post_filter_ms,
            ));
            Ok(pixels)
        }
        crate::final_composite::FinalCompositeResult::Cancelled => Err(media_error(
            MediaErrorCode::Busy,
            "先読みは新しいページ要求に置き換えられました",
        )),
    }
}

enum RemoteAiRunError {
    NotApplicable {
        code: RemoteAiTerminalCode,
        message: String,
        page_index: usize,
    },
    Superseded(String),
    Failed(String),
}

fn remote_ai_media_error(error: MediaError) -> RemoteAiRunError {
    RemoteAiRunError::Failed(error.message)
}

fn check_remote_ai_cancel(cancel: &AtomicBool) -> Result<(), RemoteAiRunError> {
    if cancel.load(Ordering::Relaxed) {
        Err(RemoteAiRunError::Failed("AI job was cancelled".to_owned()))
    } else {
        Ok(())
    }
}

fn remote_ai_pre_params(
    params: &crate::adjustment::AdjustParams,
) -> crate::adjustment::AdjustParams {
    let mut result = params.clone();
    result.post_filter = crate::adjustment::PostFilter::None;
    result.creative_lut = crate::creative_lut::CreativeLutSelection::default();
    result.colorize = crate::colorize::ColorizeParams::default();
    result.smart_sharpen = 0;
    result
}

fn remote_ai_native_budget(
    settings: &crate::settings_db::AdjustmentRenderSettings,
) -> Option<(usize, u64)> {
    let max_entries = settings.retained_final_ai_cache_max_entries;
    let max_mib = settings.retained_final_ai_cache_max_mib;
    if max_entries == 0 || max_mib == 0 {
        return None;
    }
    Some((max_entries, max_mib.saturating_mul(1024 * 1024)))
}

fn remote_ai_model_epoch(
    runtime: &crate::ai::runtime::AiRuntime,
    manager: &crate::ai::model_manager::ModelManager,
    selected: crate::ai::final_pipeline::SelectedFinalAiModels,
) -> [u8; 32] {
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    digest.update(REMOTE_AI_PIPELINE_SCHEMA.to_le_bytes());
    digest.update(runtime.active_backend().requested.as_str().as_bytes());
    digest.update(runtime.active_backend().effective.as_str().as_bytes());
    digest.update(crate::ai::tensorrt_pack::EXPECTED_TRT_PACK_VERSION.to_le_bytes());
    for kind in [selected.denoise, selected.upscale].into_iter().flatten() {
        digest.update(format!("{kind:?}").as_bytes());
        digest.update([u8::from(runtime.should_route_to_worker(kind))]);
        if let Some(path) = manager.model_path(kind) {
            digest.update(path.as_os_str().to_string_lossy().as_bytes());
            if let Ok(metadata) = std::fs::metadata(path) {
                digest.update(metadata.len().to_le_bytes());
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos())
                    .unwrap_or_default();
                digest.update(modified.to_le_bytes());
            }
        }
    }
    digest.finalize().into()
}

fn remote_ai_progress(
    phase: RemoteAiProgressPhase,
    page_index: usize,
    page_count: usize,
    stage_index: u32,
    tiles: Option<(usize, usize)>,
) -> mimageviewer_ipc::RemoteAiProgress {
    mimageviewer_ipc::RemoteAiProgress {
        phase,
        page_index: page_index as u32,
        page_count: page_count as u32,
        stage_index,
        stage_count: 5,
        completed_tiles: tiles.map(|(completed, _)| completed as u32),
        total_tiles: tiles.map(|(_, total)| total as u32),
    }
}

struct ContainerFinalAiProgress<'a> {
    sink: &'a dyn super::ai_job::RemoteAiProgressSink,
    page_index: usize,
    page_count: usize,
}

impl crate::ai::final_pipeline::FinalAiProgressSink for ContainerFinalAiProgress<'_> {
    fn loading_model(&self, _kind: crate::ai::ModelKind) {
        self.sink.update(
            mimageviewer_ipc::RemoteAiJobState::LoadingModel,
            Some(remote_ai_progress(
                RemoteAiProgressPhase::LoadingModel,
                self.page_index,
                self.page_count,
                1,
                None,
            )),
        );
    }

    fn denoising(&self, completed_tiles: usize, total_tiles: usize) {
        self.sink.update(
            mimageviewer_ipc::RemoteAiJobState::Denoising,
            Some(remote_ai_progress(
                RemoteAiProgressPhase::Denoising,
                self.page_index,
                self.page_count,
                2,
                Some((completed_tiles, total_tiles)),
            )),
        );
    }

    fn upscaling(&self, completed_tiles: usize, total_tiles: usize) {
        self.sink.update(
            mimageviewer_ipc::RemoteAiJobState::Upscaling,
            Some(remote_ai_progress(
                RemoteAiProgressPhase::Upscaling,
                self.page_index,
                self.page_count,
                3,
                Some((completed_tiles, total_tiles)),
            )),
        );
    }
}

fn decode_remote_ai_canonical(
    source: crate::canonical_image_loader::CanonicalImageSource<'_>,
    page_index: usize,
    cancel: &Arc<AtomicBool>,
) -> Result<(Arc<egui::ColorImage>, [usize; 2]), RemoteAiRunError> {
    let decoded = crate::canonical_image_loader::decode_canonical_image(
        source,
        crate::canonical_image_loader::CanonicalDecodeOptions {
            susie_priority: true,
            susie_cancel: Some(cancel),
        },
    )
    .map_err(|error| RemoteAiRunError::Failed(error.to_string()))?;
    match decoded {
        crate::canonical_image_loader::CanonicalImageDecode::Static(image) => {
            let raster = image.into_gpu_raster();
            Ok((Arc::new(raster.pixels), raster.source_dims))
        }
        crate::canonical_image_loader::CanonicalImageDecode::Animated { format, .. } => {
            let (code, label) = match format {
                crate::canonical_image_loader::CanonicalAnimatedFormat::Gif => {
                    (RemoteAiTerminalCode::AnimatedGif, "animated GIF")
                }
                crate::canonical_image_loader::CanonicalAnimatedFormat::Apng => {
                    (RemoteAiTerminalCode::AnimatedApng, "animated PNG")
                }
                crate::canonical_image_loader::CanonicalAnimatedFormat::WebP => {
                    (RemoteAiTerminalCode::AnimatedWebp, "animated WebP")
                }
            };
            Err(RemoteAiRunError::NotApplicable {
                code,
                message: format!("{label} is not an AI still-image source"),
                page_index,
            })
        }
    }
}

fn loaded_image_from_color_image(pixels: &egui::ColorImage) -> Result<LoadedImage, MediaError> {
    let width = u32::try_from(pixels.size[0])
        .map_err(|_| media_error(MediaErrorCode::RenderFailed, "画像の幅が範囲外です"))?;
    let height = u32::try_from(pixels.size[1])
        .map_err(|_| media_error(MediaErrorCode::RenderFailed, "画像の高さが範囲外です"))?;
    let rgba = crate::capture::color_image_to_rgba(pixels);
    let image = image::RgbaImage::from_raw(width, height, rgba)
        .map(image::DynamicImage::ImageRgba8)
        .ok_or_else(|| {
            media_error(
                MediaErrorCode::RenderFailed,
                "ページ画像を WebP エンコード用の形式へ変換できませんでした",
            )
        })?;
    Ok(LoadedImage { image })
}

fn remote_adjustment_read_error(scope: &str, error: String) -> MediaError {
    crate::logger::log(format!(
        "remote_ipc: live adjustment DB read failed scope={scope}: {error}"
    ));
    media_error(
        MediaErrorCode::Internal,
        format!("最新の補正データを読み込めませんでした ({scope})"),
    )
}

fn validated_context(
    page_index: usize,
    page_count: usize,
    record_history: bool,
    record_resume: bool,
    bookmark_supported: bool,
) -> Result<ValidatedPageContext, RemoteWriteError> {
    let page_index = u32::try_from(page_index).map_err(|_| {
        RemoteWriteError::new(
            RemoteWriteErrorCode::Unsupported,
            "ページ index が上限を超えています",
        )
    })?;
    let page_count = u32::try_from(page_count).map_err(|_| {
        RemoteWriteError::new(
            RemoteWriteErrorCode::Unsupported,
            "ページ数が上限を超えています",
        )
    })?;
    Ok(ValidatedPageContext {
        page_index,
        page_number: page_index.saturating_add(1),
        page_count,
        record_history,
        record_resume,
        bookmark_supported,
    })
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

fn grid_item_address(
    container: &RemoteAddress,
    item: &crate::grid_item::GridItem,
) -> Option<RemoteAddress> {
    match item {
        crate::grid_item::GridItem::Image(path) => {
            let name = path.file_name()?.to_str()?;
            let parent = container.relative_path.trim_end_matches('/');
            let relative_path = if parent.is_empty() {
                name.to_owned()
            } else {
                format!("{parent}/{name}")
            };
            Some(RemoteAddress::file(
                container.favorite_id.clone(),
                relative_path,
            ))
        }
        crate::grid_item::GridItem::ZipImage { entry_name, .. } => {
            Some(zip_entry_address(container, entry_name))
        }
        crate::grid_item::GridItem::PdfPage { page_num, .. } => Some(RemoteAddress {
            favorite_id: container.favorite_id.clone(),
            relative_path: container.relative_path.clone(),
            subresource: RemoteSubresource::PdfPage {
                page_number: *page_num,
            },
        }),
        _ => None,
    }
}

fn resolve_resume_page(
    container: &RemoteAddress,
    items: &[crate::grid_item::GridItem],
    saved_index: usize,
) -> Option<RemoteAddress> {
    items
        .get(saved_index)
        .and_then(|item| grid_item_address(container, item))
}

fn core_spread_mode(mode: RemoteSpreadMode) -> crate::settings::SpreadMode {
    match mode {
        RemoteSpreadMode::Single => crate::settings::SpreadMode::Single,
        RemoteSpreadMode::Ltr => crate::settings::SpreadMode::Ltr,
        RemoteSpreadMode::LtrCover => crate::settings::SpreadMode::LtrCover,
        RemoteSpreadMode::Rtl => crate::settings::SpreadMode::Rtl,
        RemoteSpreadMode::RtlCover => crate::settings::SpreadMode::RtlCover,
    }
}

fn remote_spread_mode(mode: crate::settings::SpreadMode) -> RemoteSpreadMode {
    match mode {
        crate::settings::SpreadMode::Ltr => RemoteSpreadMode::Ltr,
        crate::settings::SpreadMode::LtrCover => RemoteSpreadMode::LtrCover,
        crate::settings::SpreadMode::Rtl => RemoteSpreadMode::Rtl,
        crate::settings::SpreadMode::RtlCover => RemoteSpreadMode::RtlCover,
        crate::settings::SpreadMode::Single | crate::settings::SpreadMode::Vertical => {
            RemoteSpreadMode::Single
        }
    }
}

fn remote_spread_mode_name(mode: RemoteSpreadMode) -> &'static str {
    match mode {
        RemoteSpreadMode::Single => "single",
        RemoteSpreadMode::Ltr => "ltr",
        RemoteSpreadMode::LtrCover => "ltr_cover",
        RemoteSpreadMode::Rtl => "rtl",
        RemoteSpreadMode::RtlCover => "rtl_cover",
    }
}

fn remote_reading_direction(
    direction: crate::settings::ReadingDirection,
) -> RemoteReadingDirection {
    match direction {
        crate::settings::ReadingDirection::Ltr => RemoteReadingDirection::Ltr,
        crate::settings::ReadingDirection::Rtl => RemoteReadingDirection::Rtl,
    }
}

fn remote_reading_direction_name(direction: RemoteReadingDirection) -> &'static str {
    match direction {
        RemoteReadingDirection::Ltr => "ltr",
        RemoteReadingDirection::Rtl => "rtl",
    }
}

fn resolve_spread_state(
    requested: Option<RemoteSpreadMode>,
    requested_direction: Option<RemoteReadingDirection>,
    stored_mode: Option<crate::settings::SpreadMode>,
    stored_direction: Option<crate::settings::ReadingDirection>,
    default_mode: crate::settings::SpreadMode,
    default_direction: crate::settings::ReadingDirection,
    force_single_page: bool,
) -> (RemoteSpreadMode, RemoteSpreadMode, RemoteReadingDirection) {
    let configured =
        requested.unwrap_or_else(|| remote_spread_mode(stored_mode.unwrap_or(default_mode)));
    let mut reading_direction = requested_direction
        .unwrap_or_else(|| remote_reading_direction(stored_direction.unwrap_or(default_direction)));
    if configured.is_rtl() {
        reading_direction = RemoteReadingDirection::Rtl;
    } else if matches!(
        configured,
        RemoteSpreadMode::Ltr | RemoteSpreadMode::LtrCover
    ) {
        reading_direction = RemoteReadingDirection::Ltr;
    }
    let effective = if force_single_page {
        RemoteSpreadMode::Single
    } else {
        configured
    };
    (configured, effective, reading_direction)
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

fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            crate::folder_tree::is_recognized_image_ext(&extension.to_ascii_lowercase())
        })
}

fn media_source_kind(address: &RemoteAddress) -> &'static str {
    match address.subresource {
        RemoteSubresource::ZipDirectory { .. } | RemoteSubresource::ZipEntry { .. } => "zip",
        RemoteSubresource::PdfPage { .. } => "pdf",
        RemoteSubresource::File => {
            let path = Path::new(&address.relative_path);
            if is_zip_path(path) {
                "zip"
            } else if is_pdf_path(path) {
                "pdf"
            } else {
                "file"
            }
        }
    }
}

fn pdf_error(error: std::io::Error) -> MediaError {
    let message = error.to_string();
    if crate::pdf_loader::is_password_required_error(&error) {
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

fn remote_write_error_from_media(error: MediaError) -> RemoteWriteError {
    let code = match error.code {
        MediaErrorCode::BadRequest => RemoteWriteErrorCode::BadRequest,
        MediaErrorCode::FavoriteNotFound => RemoteWriteErrorCode::FavoriteNotFound,
        MediaErrorCode::PathRejected => RemoteWriteErrorCode::PathRejected,
        MediaErrorCode::NotFound => RemoteWriteErrorCode::NotFound,
        MediaErrorCode::Unsupported => RemoteWriteErrorCode::Unsupported,
        MediaErrorCode::Busy => RemoteWriteErrorCode::Busy,
        MediaErrorCode::PasswordRequired
        | MediaErrorCode::PageOutOfRange
        | MediaErrorCode::RenderFailed
        | MediaErrorCode::Internal => RemoteWriteErrorCode::Internal,
    };
    RemoteWriteError::new(code, error.message)
}

fn media_error_from_remote_write(error: RemoteWriteError) -> MediaError {
    let code = match error.code {
        RemoteWriteErrorCode::BadRequest => MediaErrorCode::BadRequest,
        RemoteWriteErrorCode::FavoriteNotFound => MediaErrorCode::FavoriteNotFound,
        RemoteWriteErrorCode::PathRejected => MediaErrorCode::PathRejected,
        RemoteWriteErrorCode::NotFound => MediaErrorCode::NotFound,
        RemoteWriteErrorCode::Unsupported => MediaErrorCode::Unsupported,
        RemoteWriteErrorCode::Busy => MediaErrorCode::Busy,
        RemoteWriteErrorCode::UiTimeout
        | RemoteWriteErrorCode::PersistenceFailed
        | RemoteWriteErrorCode::Internal => MediaErrorCode::Internal,
    };
    MediaError::new(code, error.message)
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
    use std::io::{Cursor, Write};

    struct NoRemoteAiProgress;

    impl super::super::ai_job::RemoteAiProgressSink for NoRemoteAiProgress {
        fn update(
            &self,
            _state: mimageviewer_ipc::RemoteAiJobState,
            _progress: Option<mimageviewer_ipc::RemoteAiProgress>,
        ) {
        }
    }

    fn remote_ai_test_png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([
                (x * 53 + y * 7) as u8,
                (x * 11 + y * 61) as u8,
                (x * 29 + y * 17) as u8,
                255,
            ])
        });
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn remote_ai_test_zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut output);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, bytes) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
        }
        output.into_inner()
    }

    fn write_remote_ai_test_zip(path: &Path, entries: &[(&str, &[u8])]) {
        std::fs::write(path, remote_ai_test_zip_bytes(entries)).unwrap();
    }

    fn remote_ai_test_request(address: RemoteAddress) -> RemoteAiStartRequest {
        RemoteAiStartRequest {
            request_id: "remote-ai-container-test".to_owned(),
            pages: vec![mimageviewer_ipc::RemoteAiPageRequest {
                address,
                target_px: 1024,
            }],
        }
    }

    fn remote_ai_cache_key(page_key: &str) -> RemoteAiNativeCacheKey {
        RemoteAiNativeCacheKey {
            page_key: page_key.to_owned(),
            mtime: 1,
            file_size: 2,
            source_size: [2, 2],
            pre_ai_params: crate::adjustment::AdjustParams::default(),
            pre_ai_edit_fingerprint: [0; 32],
            ai_feature_mode: crate::settings::AiFeatureMode::Light,
            ai_upscale_limit: crate::ai::upscale::AiProcessSizeLimit::square(4096),
            ai_denoise_limit: crate::ai::upscale::AiProcessSizeLimit::square(4096),
            ai_backend: Some("directml".to_owned()),
            background_mode: 0,
            pipeline_schema: REMOTE_AI_PIPELINE_SCHEMA,
            model_epoch: [0; 32],
        }
    }

    #[test]
    fn remote_ai_native_budget_is_derived_exactly_from_retained_settings() {
        let mut settings = crate::settings::Settings::default();
        settings.retained_final_ai_cache_max_entries = 7;
        settings.retained_final_ai_cache_max_mib = 23;
        let snapshot = crate::settings_db::AdjustmentRenderSettings::from_settings(&settings);
        assert_eq!(
            remote_ai_native_budget(&snapshot),
            Some((7, 23 * 1024 * 1024))
        );

        settings.retained_final_ai_cache_max_entries = 0;
        let snapshot = crate::settings_db::AdjustmentRenderSettings::from_settings(&settings);
        assert_eq!(remote_ai_native_budget(&snapshot), None);
    }

    #[test]
    fn remote_ai_native_cache_obeys_both_independent_lru_bounds() {
        let pixels = Arc::new(egui::ColorImage::new([2, 2], vec![egui::Color32::BLACK; 4]));
        let bytes = pixels.as_raw().len() as u64;
        let mut cache = RemoteAiNativeCache::default();
        cache.insert(
            remote_ai_cache_key("one"),
            Arc::clone(&pixels),
            false,
            1,
            bytes * 2,
        );
        cache.insert(
            remote_ai_cache_key("two"),
            Arc::clone(&pixels),
            true,
            1,
            bytes * 2,
        );
        assert!(cache.get(&remote_ai_cache_key("one")).is_none());
        assert_eq!(
            cache.get(&remote_ai_cache_key("two")).map(|hit| hit.1),
            Some(true)
        );

        cache.insert(
            remote_ai_cache_key("three"),
            Arc::clone(&pixels),
            false,
            3,
            bytes,
        );
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.entries[0].key.page_key, "three");

        cache.insert(remote_ai_cache_key("disabled"), pixels, false, 0, bytes);
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn remote_ai_native_cache_applies_lowered_live_budget_before_lookup() {
        let pixels = Arc::new(egui::ColorImage::new([2, 2], vec![egui::Color32::BLACK; 4]));
        let bytes = pixels.as_raw().len() as u64;
        let mut cache = RemoteAiNativeCache::default();
        for page_key in ["one", "two"] {
            cache.insert(
                remote_ai_cache_key(page_key),
                Arc::clone(&pixels),
                false,
                2,
                bytes * 2,
            );
        }

        cache.enforce_budget(1, bytes * 2);
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.entries[0].key.page_key, "two");

        cache.enforce_budget(0, bytes * 2);
        assert!(cache.entries.is_empty());
        assert_eq!(cache.bytes, 0);
    }

    #[test]
    fn remote_executor_rejects_vector_pdf_and_size_gate_before_runtime_acquisition() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("vector.pdf"),
            b"classification is supplied by the test seam",
        )
        .unwrap();
        std::fs::write(
            root.join("large.png"),
            b"decode is supplied by the test seam",
        )
        .unwrap();
        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let mut settings = crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        };
        settings.ai_feature_mode = crate::settings::AiFeatureMode::Light;
        settings.global_preset.upscale_model = Some("realesr_general_v3".to_owned());
        settings.ai_upscale_size_limit = Some(crate::ai::upscale::AiProcessSizeLimit::square(4));
        let render_settings =
            crate::settings_db::AdjustmentRenderSettings::from_settings(&settings);
        let engine = ContainerEngine::new(settings);
        let cancel = Arc::new(AtomicBool::new(false));
        let runtime_acquisitions = AtomicUsize::new(0);
        let resources = |_engine: &ContainerEngine| {
            runtime_acquisitions.fetch_add(1, Ordering::Relaxed);
            None
        };
        let prepare = |_engine: &ContainerEngine,
                       address: &RemoteAddress,
                       logical_path: &Path,
                       mtime: i64,
                       file_size: i64,
                       target_px: u32,
                       _context: &WorkerContext| {
            let params = render_settings.global_preset.clone();
            let page_key =
                crate::edit_source::page_key_for_remote(logical_path, &address.subresource)
                    .expect("test address identifies a page");
            Ok(Some(RemotePreparedComposite {
                key: RemoteCompositeCacheKey {
                    page_key,
                    mtime,
                    file_size,
                    target_px,
                    params: params.clone(),
                    lut_entry: None,
                    edit_fingerprint: [0; 32],
                },
                params,
                lut_entry: None,
                edits: RemoteEditSnapshot {
                    erase: None,
                    local_adjust: None,
                    conceal: None,
                    conceal_preset: render_settings.conceal_preset.clone(),
                    comic: Vec::new(),
                    export_crop: None,
                    fingerprint: [0; 32],
                    pre_ai_fingerprint: [0; 32],
                },
                settings: render_settings.clone(),
            }))
        };
        let decode = |_engine: &ContainerEngine,
                      address: &RemoteAddress,
                      _resolved: &ResolvedFavoritePath,
                      _metadata: &std::fs::Metadata,
                      page_index: usize,
                      _cancel: &Arc<AtomicBool>| {
            if matches!(address.subresource, RemoteSubresource::PdfPage { .. }) {
                Err(RemoteAiRunError::NotApplicable {
                    code: RemoteAiTerminalCode::VectorPdf,
                    message: "vector fixture".to_owned(),
                    page_index,
                })
            } else {
                Ok((
                    Arc::new(egui::ColorImage::new(
                        [4, 3],
                        vec![egui::Color32::BLACK; 12],
                    )),
                    [4, 3],
                ))
            }
        };

        let vector = remote_ai_test_request(RemoteAddress {
            favorite_id: favorite.id.to_string(),
            relative_path: "vector.pdf".to_owned(),
            subresource: RemoteSubresource::PdfPage { page_number: 0 },
        });
        let vector_result = engine.execute_remote_ai_inner_with(
            &vector,
            &NoRemoteAiProgress,
            &cancel,
            &prepare,
            &decode,
            &resources,
        );
        match vector_result {
            Err(RemoteAiRunError::NotApplicable {
                code: RemoteAiTerminalCode::VectorPdf,
                page_index: 0,
                ..
            }) => {}
            Err(RemoteAiRunError::NotApplicable { code, message, .. }) => {
                panic!("unexpected vector terminal {code:?}: {message}");
            }
            Err(RemoteAiRunError::Superseded(message)) => {
                panic!("vector fixture was superseded: {message}");
            }
            Err(RemoteAiRunError::Failed(message)) => {
                panic!("vector fixture failed before classification: {message}");
            }
            Ok(_) => panic!("vector fixture unexpectedly reached AI output"),
        }

        let size_gated =
            remote_ai_test_request(RemoteAddress::file(favorite.id.to_string(), "large.png"));
        let size_result = engine.execute_remote_ai_inner_with(
            &size_gated,
            &NoRemoteAiProgress,
            &cancel,
            &prepare,
            &decode,
            &resources,
        );
        match size_result {
            Err(RemoteAiRunError::NotApplicable {
                code: RemoteAiTerminalCode::SizeGate,
                page_index: 0,
                ..
            }) => {}
            Err(RemoteAiRunError::NotApplicable { code, message, .. }) => {
                panic!("unexpected size terminal {code:?}: {message}");
            }
            Err(RemoteAiRunError::Superseded(message)) => {
                panic!("size fixture was superseded: {message}");
            }
            Err(RemoteAiRunError::Failed(message)) => {
                panic!("size fixture failed before gate: {message}");
            }
            Ok(_) => panic!("size fixture unexpectedly reached AI output"),
        }
        assert_eq!(runtime_acquisitions.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn decode_remote_ai_source_routes_nested_zip_through_the_canonical_decoder() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        std::fs::create_dir_all(&root).unwrap();
        let page = remote_ai_test_png(4, 3);
        let inner = remote_ai_test_zip_bytes(&[("page.png", &page)]);
        let outer = root.join("book.cbz");
        write_remote_ai_test_zip(&outer, &[("chapter.zip", &inner)]);
        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let engine = ContainerEngine::new(crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        });
        let address = RemoteAddress {
            favorite_id: favorite.id.to_string(),
            relative_path: "book.cbz".to_owned(),
            subresource: RemoteSubresource::ZipEntry {
                entry_name: "chapter.zip/page.png".to_owned(),
            },
        };
        let resolved = engine.resolve(&address).unwrap();
        let metadata = std::fs::metadata(&resolved.canonical).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));

        let Ok((actual, actual_dims)) =
            engine.decode_remote_ai_source(&address, &resolved, &metadata, 0, &cancel)
        else {
            panic!("nested ZIP remote source must decode");
        };
        let Ok((expected, expected_dims)) = decode_remote_ai_canonical(
            crate::canonical_image_loader::CanonicalImageSource::File {
                path: Path::new("page.png"),
                verified_bytes: Some(&page),
            },
            0,
            &cancel,
        ) else {
            panic!("verified page bytes must decode canonically");
        };

        assert_eq!(actual_dims, [4, 3]);
        assert_eq!(actual_dims, expected_dims);
        assert_eq!(actual.size, expected.size);
        assert_eq!(actual.pixels, expected.pixels);
    }

    #[test]
    fn remote_default_adjustment_preserves_pixels() {
        let source = Arc::new(egui::ColorImage::new(
            [3, 2],
            vec![
                egui::Color32::from_rgba_unmultiplied(1, 2, 3, 255),
                egui::Color32::from_rgba_unmultiplied(40, 50, 60, 200),
                egui::Color32::from_rgba_unmultiplied(70, 80, 90, 128),
                egui::Color32::from_rgba_unmultiplied(100, 110, 120, 255),
                egui::Color32::from_rgba_unmultiplied(130, 140, 150, 64),
                egui::Color32::from_rgba_unmultiplied(200, 210, 220, 255),
            ],
        ));
        let result = execute_remote_composite(
            Arc::clone(&source),
            &crate::adjustment::AdjustParams::default(),
            None,
            &AtomicBool::new(false),
        )
        .unwrap();

        assert!(Arc::ptr_eq(&result, &source));
        assert_eq!(result.pixels, source.pixels);
    }

    #[test]
    fn foreground_page_cancels_only_the_active_remote_prefetch() {
        let engine = ContainerEngine::new(crate::settings::Settings::default());
        let prefetch = engine.begin_page_render(PagePriority::Prefetch, None);
        let foreground = engine.begin_page_render(PagePriority::Foreground, None);

        assert!(prefetch.load(Ordering::Relaxed));
        assert!(!foreground.load(Ordering::Relaxed));
    }

    #[test]
    fn remote_composite_cache_key_includes_effective_params() {
        let mut cache = RemoteCompositeCache::default();
        let pixels = Arc::new(egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]));
        let mut key = RemoteCompositeCacheKey {
            page_key: "page".to_owned(),
            mtime: 1,
            file_size: 2,
            target_px: 1024,
            params: crate::adjustment::AdjustParams::default(),
            lut_entry: None,
            edit_fingerprint: [0; 32],
        };
        cache.insert(key.clone(), Arc::clone(&pixels));
        let base_key = key.clone();
        key.params.brightness = 10.0;
        assert!(cache.get(&key).is_none());
        let mut edited_key = base_key;
        edited_key.edit_fingerprint[0] = 1;
        assert!(cache.get(&edited_key).is_none());
    }

    #[test]
    fn compiled_book_remote_adjustment_uses_identity_until_a_page_override_exists() {
        let identity = RemoteAdjustmentIdentity {
            page_key: "compiled-page".to_owned(),
            location_path: PathBuf::from("C:/books/compiled"),
            compiled_book: true,
        };
        let mut global = crate::adjustment::AdjustParams::default();
        global.brightness = 45.0;
        let resolved =
            resolve_remote_effective_params(&identity, None, &[], &HashMap::new(), &global);
        assert_eq!(resolved, crate::adjustment::AdjustParams::default());

        let mut page = crate::adjustment::AdjustParams::default();
        page.brightness = 18.0;
        let resolved =
            resolve_remote_effective_params(&identity, Some(&page), &[], &HashMap::new(), &global);
        assert_eq!(resolved, page);
    }

    #[test]
    fn remote_edit_adapter_materializes_conceal_before_final_composite() {
        let engine = ContainerEngine::new(crate::settings::Settings::default());
        let source = Arc::new(egui::ColorImage::new(
            [2, 1],
            vec![egui::Color32::RED, egui::Color32::GREEN],
        ));
        let mut preset = crate::conceal::ConcealPreset::default();
        preset.conceal_type = crate::conceal::ConcealType::BlackFill;
        preset.fill_opacity_percent = 100;
        let result = engine
            .execute_remote_edits(
                source,
                RemoteEditSnapshot {
                    erase: None,
                    local_adjust: None,
                    conceal: Some(crate::edit_source::MaskSnapshot {
                        bitmap: vec![true, false],
                        shapes: Vec::new(),
                        size: [2, 1],
                    }),
                    conceal_preset: preset,
                    comic: Vec::new(),
                    export_crop: None,
                    fingerprint: [0; 32],
                    pre_ai_fingerprint: [0; 32],
                },
                &Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        assert_eq!(result.pixels.pixels[0], egui::Color32::BLACK);
        assert_eq!(result.pixels.pixels[1], egui::Color32::GREEN);
    }

    #[test]
    fn remote_virtual_page_identity_uses_the_app_adjustment_keys_and_container_location() {
        let container = PathBuf::from("C:/books/nested/book.zip");
        let zip_address = RemoteAddress {
            favorite_id: "favorite".to_owned(),
            relative_path: "book.zip".to_owned(),
            subresource: RemoteSubresource::ZipEntry {
                entry_name: "Chapter/001.JPG".to_owned(),
            },
        };
        let zip = remote_adjustment_identity(&zip_address, &container).unwrap();
        assert_eq!(
            zip.page_key,
            crate::adjustment_db::zip_entry_key(&container, "Chapter/001.JPG")
        );
        assert_eq!(zip.location_path, container);

        let pdf_path = PathBuf::from("C:/books/nested/book.pdf");
        let pdf_address = RemoteAddress {
            favorite_id: "favorite".to_owned(),
            relative_path: "book.pdf".to_owned(),
            subresource: RemoteSubresource::PdfPage { page_number: 7 },
        };
        let pdf = remote_adjustment_identity(&pdf_address, &pdf_path).unwrap();
        assert_eq!(
            pdf.page_key,
            crate::adjustment_db::zip_entry_key(&pdf_path, "page_7")
        );
        assert_eq!(pdf.location_path, pdf_path);
    }
    #[test]
    fn resume_page_resolution_rejects_positions_outside_the_current_pages() {
        let container = RemoteAddress::file("favorite", "book.pdf");
        let items = (0..3)
            .map(|page_num| crate::grid_item::GridItem::PdfPage {
                pdf_path: std::path::PathBuf::from("book.pdf"),
                page_num,
                content_type: None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            resolve_resume_page(&container, &items, 1),
            Some(RemoteAddress {
                favorite_id: "favorite".to_owned(),
                relative_path: "book.pdf".to_owned(),
                subresource: RemoteSubresource::PdfPage { page_number: 1 },
            })
        );
        assert_eq!(resolve_resume_page(&container, &items, items.len()), None);
        assert_eq!(
            resolve_resume_page(&container, &items, items.len() + 20),
            None
        );
    }

    #[test]
    fn container_open_mode_matches_the_local_auto_open_and_resume_settings() {
        let mut settings = crate::settings::Settings::default();
        settings.book_open_resume = crate::settings::ResumeMode::Resume;
        let engine = ContainerEngine::new(settings.clone());
        assert_eq!(engine.container_open_mode(false), ContainerOpenMode::Grid);
        assert_eq!(
            engine.container_open_mode(true),
            ContainerOpenMode::ResumePage
        );

        settings.book_open_resume = crate::settings::ResumeMode::FromStart;
        let engine = ContainerEngine::new(settings);
        assert_eq!(
            engine.container_open_mode(true),
            ContainerOpenMode::FirstPage
        );
    }

    #[test]
    fn resume_read_failures_fall_back_without_failing_container_enumeration() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let album = root.join("album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("001.jpg"), b"one").unwrap();
        std::fs::write(album.join("002.jpg"), b"two").unwrap();
        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let address = RemoteAddress::file(favorite.id.to_string(), "album");

        for error in [
            super::super::session::UiReadError::Busy,
            super::super::session::UiReadError::Timeout,
            super::super::session::UiReadError::Stopped,
        ] {
            let mut settings = crate::settings::Settings {
                favorites: vec![favorite.clone()],
                ..Default::default()
            };
            settings.auto_fullscreen_zip_pdf = true;
            settings.auto_fullscreen_image_folders = true;
            settings.book_open_resume = crate::settings::ResumeMode::Resume;
            let engine = ContainerEngine::new_with_resume_error(settings, error);
            let response = engine.container(ContainerRequest {
                address: address.clone(),
                spread_mode: None,
                reading_direction: None,
                force_single_page: false,
            });

            let ContainerResponse::Success(payload) = response else {
                panic!("resume read failure must not fail container enumeration: {error:?}");
            };
            assert_eq!(payload.entries.len(), 2);
            assert_eq!(payload.resume_page, None);
            assert_eq!(payload.open_mode, ContainerOpenMode::ResumePage);
        }
    }

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
    fn password_protected_pdf_is_reported_distinctly() {
        let error = pdf_error(std::io::Error::other(
            "worker error: MIV_PDF_PASSWORD_REQUIRED",
        ));
        assert_eq!(error.code, MediaErrorCode::PasswordRequired);
        assert!(error.message.contains("パスワード保護"));
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

    #[test]
    fn folder_progress_validation_recomputes_local_index_count_and_bookmark_support() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let album = root.join("album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("001.jpg"), b"one").unwrap();
        std::fs::write(album.join("002.jpg"), b"two").unwrap();
        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let mut settings = crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        };
        settings.auto_fullscreen_zip_pdf = true;
        settings.auto_fullscreen_image_folders = true;
        let engine = ContainerEngine::new(settings);
        let context = RemoteAddress::file(favorite.id.to_string(), "album");
        let page = RemoteAddress::file(favorite.id.to_string(), "album/002.jpg");
        let mut request = RemoteWriteRequest::RecordReadingProgress {
            address: page.clone(),
            context_address: context.clone(),
            page_index: 999,
            page_number: 999,
            page_count: 999,
            record_resume: false,
            record_history: false,
        };
        engine.validate_write_request(&mut request).unwrap();
        assert!(matches!(
            request,
            RemoteWriteRequest::RecordReadingProgress {
                page_index: 1,
                page_number: 2,
                page_count: 2,
                record_resume: true,
                record_history: true,
                ..
            }
        ));

        let mut query = RemoteWriteRequest::GetItemState {
            address: page,
            context_address: context,
            page_index: 999,
            bookmark_supported: false,
        };
        engine.validate_write_request(&mut query).unwrap();
        assert!(matches!(
            query,
            RemoteWriteRequest::GetItemState {
                page_index: 1,
                bookmark_supported: true,
                ..
            }
        ));
    }

    #[test]
    fn mixed_folder_publishes_resume_index_but_not_history_or_bookmark_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let album = root.join("album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("001.jpg"), b"one").unwrap();
        std::fs::write(album.join("clip.mp4"), b"video").unwrap();
        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let engine = ContainerEngine::new(crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        });
        let mut request = RemoteWriteRequest::RecordReadingProgress {
            address: RemoteAddress::file(favorite.id.to_string(), "album/001.jpg"),
            context_address: RemoteAddress::file(favorite.id.to_string(), "album"),
            page_index: 0,
            page_number: 1,
            page_count: 1,
            record_resume: false,
            record_history: true,
        };
        engine.validate_write_request(&mut request).unwrap();
        assert!(matches!(
            request,
            RemoteWriteRequest::RecordReadingProgress {
                page_index: 0,
                page_number: 1,
                page_count: 1,
                record_resume: true,
                record_history: false,
                ..
            }
        ));
    }

    fn assert_local_remote_folder_listing_match(
        engine: &ContainerEngine,
        favorite: &FavoriteEntry,
        relative_folder: &str,
    ) -> FolderListPayload {
        let folder = favorite.path.join(relative_folder);
        let scan = crate::app::folder_scan::scan_directory_with_settings(&folder, &engine.settings)
            .unwrap();
        let local = crate::app::materialize_local_folder_listing(&folder, scan, &engine.settings);
        let response = engine.folder_list(FolderListRequest {
            address: RemoteAddress::file(favorite.id.to_string(), relative_folder),
        });
        let FolderListResponse::Success(remote) = response else {
            panic!("remote folder listing failed for {relative_folder}");
        };

        assert_eq!(
            remote.entries.len(),
            local.items.len(),
            "local and remote folder entry counts drifted for {relative_folder}"
        );
        for ((entry, item), meta) in remote.entries.iter().zip(&local.items).zip(&local.metas) {
            let (expected_kind, path) = match item {
                crate::grid_item::GridItem::Folder(path) => (RemoteEntryKind::Folder, path),
                crate::grid_item::GridItem::Image(path) => (RemoteEntryKind::Image, path),
                crate::grid_item::GridItem::Video(path) => (RemoteEntryKind::Video, path),
                crate::grid_item::GridItem::Audio(path) => (RemoteEntryKind::Audio, path),
                crate::grid_item::GridItem::ZipFile(path) => (RemoteEntryKind::Zip, path),
                crate::grid_item::GridItem::PdfFile(path) => (RemoteEntryKind::Pdf, path),
                crate::grid_item::GridItem::ConvertibleArchive { path, .. } => {
                    (RemoteEntryKind::Archive, path)
                }
                _ => panic!("physical folder listing produced a virtual item"),
            };
            let name = path.file_name().unwrap().to_string_lossy();
            let expected_address =
                RemoteAddress::file(favorite.id.to_string(), format!("{relative_folder}/{name}"));
            let expected_thumbnail = if expected_kind == RemoteEntryKind::Video {
                local
                    .video_thumb_overrides
                    .iter()
                    .rev()
                    .find(|(video, _)| crate::path_key::eq_keep_drive(video, path))
                    .map(|(_, image)| {
                        RemoteAddress::file(
                            favorite.id.to_string(),
                            format!(
                                "{relative_folder}/{}",
                                image.file_name().unwrap().to_string_lossy()
                            ),
                        )
                    })
                    .unwrap_or_else(|| expected_address.clone())
            } else {
                expected_address.clone()
            };
            let (expected_mtime, expected_size) = meta.unwrap_or((0, 0));

            assert_eq!(entry.kind, expected_kind, "kind drifted for {name}");
            assert_eq!(entry.name, name, "name drifted for {name}");
            assert_eq!(
                entry.address, expected_address,
                "address drifted for {name}"
            );
            assert_eq!(
                entry.thumbnail_address, expected_thumbnail,
                "thumbnail source drifted for {name}"
            );
            assert_eq!(entry.mtime, expected_mtime, "mtime drifted for {name}");
            assert_eq!(
                entry.size,
                u64::try_from(expected_size).unwrap_or(0),
                "size drifted for {name}"
            );
        }

        let page_count = local
            .items
            .iter()
            .filter(|item| item.has_page_data())
            .count();
        let context = RemoteAddress::file(favorite.id.to_string(), relative_folder);
        for (expected_index, item) in local.items.iter().enumerate() {
            let crate::grid_item::GridItem::Image(path) = item else {
                continue;
            };
            let name = path.file_name().unwrap().to_string_lossy();
            let page =
                RemoteAddress::file(favorite.id.to_string(), format!("{relative_folder}/{name}"));
            let validated = engine.validate_folder_page(&page, &context).unwrap();
            assert_eq!(validated.page_index as usize, expected_index);
            assert_eq!(validated.page_count as usize, page_count);
            assert_eq!(
                validated.page_number as usize,
                local.items[..=expected_index]
                    .iter()
                    .filter(|candidate| candidate.has_page_data())
                    .count()
            );
        }

        remote
    }
    #[test]
    fn folder_recomputation_matches_local_listing_for_required_materials() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let image_only = root.join("image-only");
        let mixed = root.join("mixed");
        let duplicate_ext = root.join("duplicate-ext");
        let virtual_duplicate = root.join("virtual-duplicate");
        for folder in [&image_only, &mixed, &duplicate_ext, &virtual_duplicate] {
            std::fs::create_dir_all(folder).unwrap();
        }

        std::fs::write(image_only.join("10.jpg"), b"ten").unwrap();
        std::fs::write(image_only.join("2.jpg"), b"two").unwrap();

        std::fs::write(mixed.join("page.jpg"), b"page").unwrap();
        std::fs::write(mixed.join("clip.mp4"), b"video").unwrap();
        std::fs::write(mixed.join("clip.jpg"), b"sidecar").unwrap();

        std::fs::write(duplicate_ext.join("same.jpg"), b"jpeg").unwrap();
        std::fs::write(duplicate_ext.join("same.png"), b"png").unwrap();
        std::fs::write(duplicate_ext.join("other.jpg"), b"other").unwrap();

        std::fs::create_dir_all(virtual_duplicate.join("volume")).unwrap();
        std::fs::write(virtual_duplicate.join("volume.zip"), b"zip").unwrap();
        std::fs::write(virtual_duplicate.join("page.jpg"), b"page").unwrap();

        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let settings = crate::settings::Settings {
            favorites: vec![favorite.clone()],
            skip_duplicate_images: true,
            skip_zip_if_folder_exists: true,
            skip_image_if_video_exists: true,
            video_thumb_use_sidecar_image: true,
            thumb_aspect_auto: false,
            thumb_aspect: crate::settings::ThumbAspect::Landscape3x2,
            ..Default::default()
        };
        let engine = ContainerEngine::new(settings);

        for relative in ["image-only", "mixed", "duplicate-ext", "virtual-duplicate"] {
            let remote = assert_local_remote_folder_listing_match(&engine, &favorite, relative);
            if relative == "mixed" {
                assert!(
                    remote.entries.iter().all(|entry| entry.name != "clip.jpg"),
                    "the absorbed sidecar must not remain as an independent remote tile"
                );
                assert!((remote.thumb_aspect_height_ratio - (2.0 / 3.0)).abs() < 1e-6);
                let video = remote
                    .entries
                    .iter()
                    .find(|entry| entry.name == "clip.mp4")
                    .expect("video entry");
                assert_eq!(video.kind, RemoteEntryKind::Video);
                assert_eq!(video.address.relative_path, "mixed/clip.mp4");
                assert_eq!(video.thumbnail_address.relative_path, "mixed/clip.jpg");
            }
        }
    }

    #[test]
    fn folder_spread_groups_share_cover_landscape_rtl_and_portrait_rules() {
        let items = (0..5)
            .map(|index| {
                crate::grid_item::GridItem::Image(std::path::PathBuf::from(format!(
                    "page-{index}.jpg"
                )))
            })
            .collect::<Vec<_>>();
        let portrait = vec![false; items.len()];

        assert_eq!(
            crate::ui_fullscreen::build_remote_spread_page_groups(
                &items,
                crate::settings::SpreadMode::LtrCover,
                &portrait,
            ),
            vec![vec![0], vec![1, 2], vec![3, 4]]
        );

        let mut with_landscape = portrait.clone();
        with_landscape[2] = true;
        assert_eq!(
            crate::ui_fullscreen::build_remote_spread_page_groups(
                &items,
                crate::settings::SpreadMode::Ltr,
                &with_landscape,
            ),
            vec![vec![0, 1], vec![2], vec![3, 4]]
        );
        assert_eq!(
            crate::ui_fullscreen::build_remote_spread_page_groups(
                &items,
                crate::settings::SpreadMode::Rtl,
                &portrait,
            ),
            vec![vec![1, 0], vec![3, 2], vec![4]]
        );

        let (_, effective, _) = resolve_spread_state(
            Some(RemoteSpreadMode::RtlCover),
            Some(RemoteReadingDirection::Rtl),
            None,
            None,
            crate::settings::SpreadMode::Single,
            crate::settings::ReadingDirection::Ltr,
            true,
        );
        assert_eq!(
            crate::ui_fullscreen::build_remote_spread_page_groups(
                &items,
                core_spread_mode(effective),
                &portrait,
            ),
            vec![vec![0], vec![1], vec![2], vec![3], vec![4]]
        );
    }

    #[test]
    fn folder_container_uses_page_groups_and_accepts_spread_writes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let album = root.join("album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("001.jpg"), b"one").unwrap();
        std::fs::write(album.join("002.jpg"), b"two").unwrap();
        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let engine = ContainerEngine::new(crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        });
        let address = RemoteAddress::file(favorite.id.to_string(), "album");
        let response = engine.container(ContainerRequest {
            address: address.clone(),
            spread_mode: Some(RemoteSpreadMode::Ltr),
            reading_direction: Some(RemoteReadingDirection::Ltr),
            force_single_page: false,
        });
        let ContainerResponse::Success(payload) = response else {
            panic!("folder container enumeration failed");
        };
        assert_eq!(payload.kind, ContainerKind::Folder);
        assert_eq!(payload.entries.len(), 2);
        assert_eq!(payload.page_groups.len(), 1);
        assert_eq!(payload.page_groups[0].pages.len(), 2);

        let mut write = RemoteWriteRequest::SetSpread {
            address,
            spread_mode: RemoteSpreadMode::RtlCover,
            reading_direction: RemoteReadingDirection::Rtl,
        };
        engine.validate_write_request(&mut write).unwrap();
    }

    #[test]
    fn portrait_forces_single_without_changing_the_configured_mode() {
        assert_eq!(
            resolve_spread_state(
                None,
                None,
                Some(crate::settings::SpreadMode::RtlCover),
                Some(crate::settings::ReadingDirection::Ltr),
                crate::settings::SpreadMode::Ltr,
                crate::settings::ReadingDirection::Ltr,
                true,
            ),
            (
                RemoteSpreadMode::RtlCover,
                RemoteSpreadMode::Single,
                RemoteReadingDirection::Rtl,
            )
        );
        assert_eq!(
            resolve_spread_state(
                Some(RemoteSpreadMode::LtrCover),
                Some(RemoteReadingDirection::Rtl),
                Some(crate::settings::SpreadMode::Rtl),
                Some(crate::settings::ReadingDirection::Rtl),
                crate::settings::SpreadMode::Single,
                crate::settings::ReadingDirection::Rtl,
                false,
            ),
            (
                RemoteSpreadMode::LtrCover,
                RemoteSpreadMode::LtrCover,
                RemoteReadingDirection::Ltr,
            )
        );
        assert_eq!(
            resolve_spread_state(
                None,
                None,
                Some(crate::settings::SpreadMode::Single),
                Some(crate::settings::ReadingDirection::Rtl),
                crate::settings::SpreadMode::Ltr,
                crate::settings::ReadingDirection::Ltr,
                false,
            ),
            (
                RemoteSpreadMode::Single,
                RemoteSpreadMode::Single,
                RemoteReadingDirection::Rtl,
            )
        );
    }

    #[test]
    fn spread_mode_resolution_uses_stored_then_default_and_never_writes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("spread.db");
        let book = temp.path().join("book.zip");
        let writable = crate::spread_db::SpreadDb::open_at(&path).unwrap();
        writable
            .set(
                &book,
                crate::settings::SpreadMode::Rtl,
                crate::settings::SpreadMode::Single,
                crate::settings::ReadingFlow::Paged,
                crate::settings::ReadingDirection::Ltr,
            )
            .unwrap();
        drop(writable);
        let read_only = crate::spread_db::SpreadDb::open_existing_read_only_at(&path)
            .unwrap()
            .unwrap();
        let stored = read_only.get(&book);
        let stored_direction = read_only.get_direction(&book);
        assert_eq!(
            resolve_spread_state(
                Some(RemoteSpreadMode::Ltr),
                None,
                stored,
                stored_direction,
                crate::settings::SpreadMode::Single,
                crate::settings::ReadingDirection::Rtl,
                false,
            ),
            (
                RemoteSpreadMode::Ltr,
                RemoteSpreadMode::Ltr,
                RemoteReadingDirection::Ltr,
            )
        );
        assert_eq!(read_only.get(&book), Some(crate::settings::SpreadMode::Rtl));
        assert_eq!(
            read_only.get_direction(&book),
            Some(crate::settings::ReadingDirection::Rtl)
        );
        assert_eq!(
            resolve_spread_state(
                None,
                None,
                read_only.get(&book),
                read_only.get_direction(&book),
                crate::settings::SpreadMode::Single,
                crate::settings::ReadingDirection::Ltr,
                true,
            ),
            (
                RemoteSpreadMode::Rtl,
                RemoteSpreadMode::Single,
                RemoteReadingDirection::Rtl,
            )
        );
        assert_eq!(
            read_only.get(&book),
            Some(crate::settings::SpreadMode::Rtl),
            "portrait-only effective Single must not overwrite the configured value"
        );
        assert_eq!(
            resolve_spread_state(
                None,
                None,
                None,
                None,
                crate::settings::SpreadMode::LtrCover,
                crate::settings::ReadingDirection::Rtl,
                false,
            )
            .0,
            RemoteSpreadMode::LtrCover
        );
    }
}
