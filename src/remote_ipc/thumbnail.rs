use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Condvar, Mutex, RwLock, mpsc};

use mimageviewer_ipc::{ThumbnailError, ThumbnailErrorCode, ThumbnailRequest, ThumbnailResponse};

use super::path_guard::{
    ResolveError, ResolvedFavoritePath, canonicalize_within, resolve_existing,
};

pub(super) struct ThumbnailEngine {
    settings: Arc<crate::settings::Settings>,
    stats: Arc<Mutex<crate::stats::ThumbStats>>,
    inflight: Mutex<HashMap<RequestKey, Arc<Flight>>>,
}

pub(super) struct WorkerContext {
    folder_pin_db: Option<crate::folder_thumb_pins::FolderThumbPinDb>,
    rotation_db: Option<crate::rotation_db::RotationDb>,
    adjustment_db: Option<crate::adjustment_db::AdjustmentDb>,
}

impl WorkerContext {
    pub(super) fn open() -> Self {
        Self {
            folder_pin_db: crate::folder_thumb_pins::FolderThumbPinDb::open().ok(),
            rotation_db: crate::rotation_db::RotationDb::open().ok(),
            adjustment_db: crate::adjustment_db::AdjustmentDb::open().ok(),
        }
    }
}

#[derive(Clone, Eq)]
struct RequestKey {
    favorite_id: String,
    relative_path: String,
    target_px: u32,
}

impl PartialEq for RequestKey {
    fn eq(&self, other: &Self) -> bool {
        self.favorite_id == other.favorite_id
            && self.relative_path == other.relative_path
            && self.target_px == other.target_px
    }
}

impl Hash for RequestKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.favorite_id.hash(state);
        self.relative_path.hash(state);
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
    ) -> ThumbnailResponse {
        let key = RequestKey {
            favorite_id: request.favorite_id.clone(),
            relative_path: request.relative_path.clone(),
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
            self.generate(&request, context)
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

    fn generate(&self, request: &ThumbnailRequest, context: &WorkerContext) -> ThumbnailResponse {
        if request.target_px == 0 || request.target_px > 4096 {
            return error_response(
                ThumbnailErrorCode::BadRequest,
                "サムネイルサイズが範囲外です",
            );
        }
        let resolved = match resolve_existing(
            &self.settings.favorites,
            &request.favorite_id,
            &request.relative_path,
        ) {
            Ok(path) => path,
            Err(error) => return resolve_error_response(error),
        };
        match self.generate_resolved(&resolved, request.target_px, context) {
            Ok(webp_bytes) => ThumbnailResponse::Success { webp_bytes },
            Err(error) => error,
        }
    }

    fn generate_resolved(
        &self,
        resolved: &ResolvedFavoritePath,
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
        if is_folder {
            validate_folder_sources(
                &resolved.logical,
                &resolved.canonical_root,
                self.settings.folder_thumb_depth,
                context.folder_pin_db.as_ref(),
            )?;
        }
        let parent = resolved.logical.parent().ok_or_else(|| {
            error_response(
                ThumbnailErrorCode::PathRejected,
                "お気に入り root 自体のサムネイルは要求できません",
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
                error_response(
                    ThumbnailErrorCode::GenerationFailed,
                    "mIV 本体でサムネイルを生成できませんでした",
                )
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
}

/// 本体の folder representative resolver は junction と pin も通常 UI 向けに辿る。
/// IPC 境界では favorite 外を一切読めないよう、同じ深さの候補を生成前に検査する。
fn validate_folder_sources(
    folder: &Path,
    canonical_root: &Path,
    remaining_depth: u32,
    pin_db: Option<&crate::folder_thumb_pins::FolderThumbPinDb>,
) -> Result<(), ThumbnailResponse> {
    if let Some(pin_db) = pin_db
        && let Some(source) = pin_db.lookup(folder)
    {
        let lookup = |path: &Path| pin_db.lookup(path);
        if let Some(resolved) = crate::folder_thumb_pins::resolve_pin_target_cascaded_via(
            folder,
            &source,
            lookup,
            remaining_depth as usize,
        ) {
            canonicalize_within(canonical_root, &resolved.abs_path)
                .map_err(resolve_error_response)?;
        }
    }

    let entries = std::fs::read_dir(folder)
        .map_err(|_| error_response(ThumbnailErrorCode::NotFound, "フォルダを列挙できません"))?;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let kind = crate::fs_entry::classify_dir_entry(&entry, &file_type);
        if kind.is_directory() {
            if remaining_depth == 0 {
                continue;
            }
            canonicalize_within(canonical_root, &entry.path()).map_err(resolve_error_response)?;
            validate_folder_sources(&entry.path(), canonical_root, remaining_depth - 1, pin_db)?;
        } else if kind.is_file() && is_supported_image(&entry.path()) {
            canonicalize_within(canonical_root, &entry.path()).map_err(resolve_error_response)?;
        }
    }
    Ok(())
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

fn resolve_error_response(error: ResolveError) -> ThumbnailResponse {
    match error {
        ResolveError::InvalidFavoriteId | ResolveError::InvalidRelativePath => error_response(
            ThumbnailErrorCode::BadRequest,
            "favorite_id または相対パスが不正です",
        ),
        ResolveError::FavoriteNotFound => error_response(
            ThumbnailErrorCode::FavoriteNotFound,
            "お気に入りが登録されていません",
        ),
        ResolveError::EscapesFavorite => error_response(
            ThumbnailErrorCode::PathRejected,
            "お気に入りの外へ出るパスは拒否されました",
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

    #[test]
    fn identical_requests_share_one_flight() {
        let key = RequestKey {
            favorite_id: "a".to_owned(),
            relative_path: "b.jpg".to_owned(),
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
    fn folder_representative_scan_rejects_a_link_outside_the_favorite() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let folder = root.join("album");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.jpg"), b"secret").unwrap();
        let link = folder.join("escape");
        if make_dir_link(&outside, &link).is_err() {
            eprintln!("directory links are unavailable; escape assertion skipped");
            return;
        }

        let result = validate_folder_sources(
            &std::fs::canonicalize(&folder).unwrap(),
            &std::fs::canonicalize(&root).unwrap(),
            1,
            None,
        );
        assert!(matches!(
            result,
            Err(ThumbnailResponse::Error(ThumbnailError {
                code: ThumbnailErrorCode::PathRejected,
                ..
            }))
        ));
    }

    #[cfg(windows)]
    fn make_dir_link(target: &Path, link: &Path) -> std::io::Result<()> {
        match std::os::windows::fs::symlink_dir(target, link) {
            Ok(()) => Ok(()),
            Err(error) if error.raw_os_error() == Some(1314) => {
                let status = std::process::Command::new("cmd")
                    .args(["/d", "/c", "mklink", "/J"])
                    .arg(link)
                    .arg(target)
                    .status()?;
                if status.success() { Ok(()) } else { Err(error) }
            }
            Err(error) => Err(error),
        }
    }

    #[cfg(unix)]
    fn make_dir_link(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }
}
