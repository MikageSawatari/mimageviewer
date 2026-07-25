//! 明示メタ情報 import 完了後の viewer-context cache 再構築。
//!
//! import worker は DB 更新だけを担当し、途中の値を UI へ配信しない。各 context の
//! 現在項目キーを compact な snapshot として受け取り、DB の一括読込結果を所有権ごと
//! UI へ返す。UI は `items_generation` が一致する結果だけを swap する。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::metadata_transfer::{ImportChangedSections, ImportPageStateSnapshot};

/// SQLiteのbind上限回避と終了時cancel応答を兼ねるworker work unit。
const DB_KEY_CHUNK: usize = 500;

#[derive(Debug)]
pub(crate) struct ItemKey {
    pub(crate) index: usize,
    /// rating / tag / page-state が同じidentityならこの1本を共用する。
    pub(crate) key: String,
    pub(crate) rating: bool,
    pub(crate) tags: bool,
    pub(crate) page: bool,
    /// import前にこのpage identityへ編集状態が存在したか。DB再取得後の状態と
    /// ORして、削除・追加のどちらでもmaterialized thumbnailを失効させる。
    pub(crate) had_page_state: bool,
    /// page identityだけが共有キーと異なる稀な項目で追加保持する。
    pub(crate) alternate_page_key: Option<String>,
    pub(crate) container_path: Option<PathBuf>,
    pub(crate) video_path: Option<PathBuf>,
    pub(crate) video_size: u64,
}

impl ItemKey {
    fn page_key(&self) -> Option<&str> {
        if let Some(key) = self.alternate_page_key.as_deref() {
            Some(key)
        } else if self.page {
            Some(&self.key)
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub(crate) struct ContextRequest {
    pub(crate) slot: ContextSlot,
    pub(crate) items_generation: u64,
    pub(crate) items: Vec<ItemKey>,
    /// UIの段階snapshot中に収集済みのlegacy XMP tag seed対象。
    pub(crate) legacy_seed_paths: Vec<PathBuf>,
    pub(crate) current_rating_key: Option<String>,
    pub(crate) spread_container_path: Option<PathBuf>,
    pub(crate) old_folder_pin_keys: HashSet<String>,
    pub(crate) folder_pin_paths: Vec<PathBuf>,
    pub(crate) folder_pin_aliases: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextSlot {
    Main,
    ActiveDetached(Option<u64>),
    PausedDetached { index: usize, window_id: u64 },
}

pub(crate) struct ContextResult {
    pub(crate) slot: ContextSlot,
    pub(crate) items_generation: u64,
    pub(crate) rating_cache: Option<HashMap<usize, u8>>,
    pub(crate) tags_cache: Option<HashMap<String, Vec<String>>>,
    pub(crate) current_rating: Option<u8>,
    pub(crate) page_state: Option<PageStateResult>,
    pub(crate) folder_pin_map: Option<HashMap<String, crate::folder_thumb_pins::FolderPinSource>>,
    pub(crate) folder_pin_reset_indices: Option<Vec<usize>>,
    pub(crate) video_pin_blobs: Option<HashMap<PathBuf, Vec<u8>>>,
    pub(crate) video_items: Option<Vec<(usize, PathBuf, u64)>>,
    pub(crate) container_state: Option<ContainerStateResult>,
    /// `ContextRequest`からcloneせず所有権を返し、UIで再走査せずworkerを再生成する。
    pub(crate) legacy_seed_paths: Vec<PathBuf>,
}

pub(crate) struct ContainerStateResult {
    pub(crate) spread_mode: Option<crate::settings::SpreadMode>,
    pub(crate) reading_flow: Option<crate::settings::ReadingFlow>,
    pub(crate) reading_direction: Option<crate::settings::ReadingDirection>,
    pub(crate) view_trim: Option<crate::view_trim::ViewTrimBookState>,
}

pub(crate) struct PageStateResult {
    pub(crate) adjustment_page_params: HashMap<usize, crate::adjustment::AdjustParams>,
    pub(crate) local_adjust_pages: HashSet<usize>,
    pub(crate) export_crop_page_settings: HashMap<usize, crate::export_crop::CropSettings>,
    pub(crate) view_trim_page_overrides: HashMap<usize, crate::view_trim::ViewTrimPageOverride>,
    pub(crate) mask_pages: HashSet<usize>,
    pub(crate) conceal_pages: HashSet<usize>,
    pub(crate) comic_pages: HashSet<usize>,
    pub(crate) rotation_cache: HashMap<usize, crate::rotation_db::Rotation>,
    pub(crate) thumbnail_reset_indices: Vec<usize>,
}

pub(crate) struct RefreshResult {
    pub(crate) contexts: Vec<ContextResult>,
    pub(crate) page_snapshot: Option<ImportPageStateSnapshot>,
    pub(crate) errors: Vec<String>,
}

pub(crate) fn run(
    data_dir: PathBuf,
    requests: Vec<ContextRequest>,
    changed: ImportChangedSections,
    cancel: &AtomicBool,
) -> Option<RefreshResult> {
    let started = std::time::Instant::now();
    let context_count = requests.len();
    let item_count = requests
        .iter()
        .map(|request| request.items.len())
        .sum::<usize>();
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let mut errors = Vec::new();
    let page_snapshot = if changed.page_state {
        match crate::metadata_transfer::load_import_page_state_snapshot_cancellable(
            &data_dir, cancel,
        ) {
            Ok(snapshot) => Some(snapshot),
            Err(crate::metadata_transfer::TransferError::Cancelled) => return None,
            Err(error) => {
                errors.push(format!("編集状態索引を再構築できませんでした: {error}"));
                None
            }
        }
    } else {
        None
    };

    let rating_db = changed.ratings.then(|| {
        crate::rating_db::RatingDb::open_readonly(data_dir.join("rating.db"))
            .map_err(|error| format!("評価DBを再読込できませんでした: {error}"))
    });
    let tags_db = changed.tags.then(|| {
        crate::tags_db::TagsDb::open_readonly(&data_dir.join("tags.db"))
            .map_err(|error| format!("タグDBを再読込できませんでした: {error}"))
    });
    let adjustment_db = changed.page_state.then(|| {
        crate::adjustment_db::AdjustmentDb::open_at(&data_dir.join("adjustment.db"))
            .map_err(|error| format!("画像補正DBを再読込できませんでした: {error}"))
    });
    let local_adjust_db = changed.page_state.then(|| {
        crate::local_adjust_db::LocalAdjustDb::open_readonly(&data_dir.join("local_adjust.db"))
            .map_err(|error| format!("ローカル補正DBを再読込できませんでした: {error}"))
    });
    let crop_db = changed.page_state.then(|| {
        crate::export_crop::CropDb::open_at(&data_dir.join("export_crop.db"))
            .map_err(|error| format!("クロップDBを再読込できませんでした: {error}"))
    });
    let view_trim_db = changed.page_state.then(|| {
        crate::view_trim_db::ViewTrimDb::open_at(&data_dir.join("view_trim.db"))
            .map_err(|error| format!("表示トリムDBを再読込できませんでした: {error}"))
    });
    let mask_db = changed.page_state.then(|| {
        crate::mask_db::MaskDb::open_at(&data_dir.join("mask.db"))
            .map_err(|error| format!("マスクDBを再読込できませんでした: {error}"))
    });
    let conceal_db = changed.page_state.then(|| {
        crate::conceal_db::ConcealDb::open_at(&data_dir.join("conceal.db"))
            .map_err(|error| format!("隠蔽DBを再読込できませんでした: {error}"))
    });
    let comic_db = changed.page_state.then(|| {
        crate::comic_db::ComicDb::open_at(&data_dir.join("comic.db"))
            .map_err(|error| format!("注釈DBを再読込できませんでした: {error}"))
    });
    let rotation_db = changed.page_state.then(|| {
        crate::rotation_db::RotationDb::open_at(&data_dir.join("rotation.db"))
            .map_err(|error| format!("回転DBを再読込できませんでした: {error}"))
    });
    let folder_pin_db = changed.thumbnail_pins.then(|| {
        crate::folder_thumb_pins::FolderThumbPinDb::open_at(&data_dir.join("folder_thumb_pins.db"))
            .map_err(|error| format!("フォルダピンDBを再読込できませんでした: {error}"))
    });
    let video_pin_db = changed.thumbnail_pins.then(|| {
        crate::video_pins::VideoPinDb::open_readonly(&data_dir.join("video_pins.db"))
            .map_err(|error| format!("動画ピンDBを再読込できませんでした: {error}"))
    });
    let spread_db = changed.container_state.then(|| {
        crate::spread_db::SpreadDb::open_at(&data_dir.join("spread.db"))
            .map_err(|error| format!("見開きDBを再読込できませんでした: {error}"))
    });
    let container_trim_db = changed.container_state.then(|| {
        crate::view_trim_db::ViewTrimDb::open_at(&data_dir.join("view_trim.db"))
            .map_err(|error| format!("本の表示トリムDBを再読込できませんでした: {error}"))
    });

    for result in [
        rating_db.as_ref().and_then(|result| result.as_ref().err()),
        tags_db.as_ref().and_then(|result| result.as_ref().err()),
        adjustment_db
            .as_ref()
            .and_then(|result| result.as_ref().err()),
        local_adjust_db
            .as_ref()
            .and_then(|result| result.as_ref().err()),
        crop_db.as_ref().and_then(|result| result.as_ref().err()),
        view_trim_db
            .as_ref()
            .and_then(|result| result.as_ref().err()),
        mask_db.as_ref().and_then(|result| result.as_ref().err()),
        conceal_db.as_ref().and_then(|result| result.as_ref().err()),
        comic_db.as_ref().and_then(|result| result.as_ref().err()),
        rotation_db
            .as_ref()
            .and_then(|result| result.as_ref().err()),
        folder_pin_db
            .as_ref()
            .and_then(|result| result.as_ref().err()),
        video_pin_db
            .as_ref()
            .and_then(|result| result.as_ref().err()),
        spread_db.as_ref().and_then(|result| result.as_ref().err()),
        container_trim_db
            .as_ref()
            .and_then(|result| result.as_ref().err()),
    ]
    .into_iter()
    .flatten()
    {
        errors.push(result.clone());
    }

    let mut contexts = Vec::with_capacity(requests.len());
    for request in requests {
        contexts.push(build_context_result(
            request,
            changed,
            rating_db.as_ref().and_then(|result| result.as_ref().ok()),
            tags_db.as_ref().and_then(|result| result.as_ref().ok()),
            adjustment_db
                .as_ref()
                .and_then(|result| result.as_ref().ok()),
            local_adjust_db
                .as_ref()
                .and_then(|result| result.as_ref().ok()),
            crop_db.as_ref().and_then(|result| result.as_ref().ok()),
            view_trim_db
                .as_ref()
                .and_then(|result| result.as_ref().ok()),
            mask_db.as_ref().and_then(|result| result.as_ref().ok()),
            conceal_db.as_ref().and_then(|result| result.as_ref().ok()),
            comic_db.as_ref().and_then(|result| result.as_ref().ok()),
            rotation_db.as_ref().and_then(|result| result.as_ref().ok()),
            folder_pin_db
                .as_ref()
                .and_then(|result| result.as_ref().ok()),
            video_pin_db
                .as_ref()
                .and_then(|result| result.as_ref().ok()),
            spread_db.as_ref().and_then(|result| result.as_ref().ok()),
            container_trim_db
                .as_ref()
                .and_then(|result| result.as_ref().ok()),
            cancel,
        )?);
    }
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let result = RefreshResult {
        contexts,
        page_snapshot,
        errors,
    };
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    crate::logger::log(format!(
        "metadata import refresh: contexts={context_count} items={item_count} \
         errors={} elapsed_ms={elapsed_ms:.1}",
        result.errors.len()
    ));
    if crate::perf::is_enabled() {
        crate::perf::event(
            "metadata_import",
            "terminal_refresh",
            None,
            0,
            &[
                ("contexts", serde_json::Value::from(context_count as u64)),
                ("items", serde_json::Value::from(item_count as u64)),
                (
                    "errors",
                    serde_json::Value::from(result.errors.len() as u64),
                ),
                ("ms", serde_json::Value::from(elapsed_ms)),
            ],
        );
    }
    Some(result)
}

#[allow(clippy::too_many_arguments)]
fn build_context_result(
    mut request: ContextRequest,
    changed: ImportChangedSections,
    rating_db: Option<&crate::rating_db::RatingDb>,
    tags_db: Option<&crate::tags_db::TagsDb>,
    adjustment_db: Option<&crate::adjustment_db::AdjustmentDb>,
    local_adjust_db: Option<&crate::local_adjust_db::LocalAdjustDb>,
    crop_db: Option<&crate::export_crop::CropDb>,
    view_trim_db: Option<&crate::view_trim_db::ViewTrimDb>,
    mask_db: Option<&crate::mask_db::MaskDb>,
    conceal_db: Option<&crate::conceal_db::ConcealDb>,
    comic_db: Option<&crate::comic_db::ComicDb>,
    rotation_db: Option<&crate::rotation_db::RotationDb>,
    folder_pin_db: Option<&crate::folder_thumb_pins::FolderThumbPinDb>,
    video_pin_db: Option<&crate::video_pins::VideoPinDb>,
    spread_db: Option<&crate::spread_db::SpreadDb>,
    container_trim_db: Option<&crate::view_trim_db::ViewTrimDb>,
    cancel: &AtomicBool,
) -> Option<ContextResult> {
    let mut rating_cache = (changed.ratings && rating_db.is_some()).then(HashMap::new);
    let mut tags_cache = (changed.tags && tags_db.is_some()).then(HashMap::new);
    let all_page_databases_available = [
        adjustment_db.is_some(),
        local_adjust_db.is_some(),
        crop_db.is_some(),
        view_trim_db.is_some(),
        mask_db.is_some(),
        conceal_db.is_some(),
        comic_db.is_some(),
        rotation_db.is_some(),
    ]
    .into_iter()
    .all(|available| available);
    let mut page_state =
        (changed.page_state && all_page_databases_available).then(PageStateResult::default);

    for chunk in request.items.chunks(DB_KEY_CHUNK) {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let rating_keys = chunk
            .iter()
            .filter(|item| item.rating)
            .map(|item| item.key.clone())
            .collect::<Vec<_>>();
        let tag_keys = chunk
            .iter()
            .filter(|item| item.tags)
            .map(|item| item.key.clone())
            .collect::<Vec<_>>();
        if let (Some(cache), Some(db)) = (rating_cache.as_mut(), rating_db) {
            let loaded = db.get_many(&rating_keys);
            for item in chunk.iter().filter(|item| item.rating) {
                cache.insert(item.index, loaded.get(&item.key).copied().unwrap_or(0));
            }
        }
        if let (Some(cache), Some(db)) = (tags_cache.as_mut(), tags_db) {
            // `tags_cache` の空 Vec は「DB読込済みだがタグなし」を表す sentinel。
            // 行があるキーだけを載せると facet の Tagged 判定が未ロード扱いで
            // permissive になるため、対象キーを先に空で初期化してから実値で上書きする。
            for key in &tag_keys {
                cache.entry(key.clone()).or_default();
            }
            for (key, tags) in db.get_many_display_tags(&tag_keys) {
                cache.insert(key, tags);
            }
        }

        let page_items = chunk
            .iter()
            .filter_map(|item| item.page_key().map(|key| (item.index, key)))
            .collect::<Vec<_>>();
        let page_keys = page_items.iter().map(|(_, key)| *key).collect::<Vec<_>>();
        if let Some(state) = page_state.as_mut() {
            let reset_start = state.thumbnail_reset_indices.len();
            if let Some(db) = adjustment_db {
                let loaded = db.load_page_params_many(&page_keys);
                for (index, key) in &page_items {
                    if let Some(value) = loaded.get(*key) {
                        state.adjustment_page_params.insert(*index, value.clone());
                    }
                }
            }
            if let Some(db) = local_adjust_db {
                let owned = page_keys
                    .iter()
                    .map(|key| (*key).to_string())
                    .collect::<Vec<_>>();
                let loaded = db.load_existing_layer_keys(&owned);
                for (index, key) in &page_items {
                    if loaded.contains(*key) {
                        state.local_adjust_pages.insert(*index);
                    }
                }
            }
            if let Some(db) = crop_db {
                let loaded = db.load_many(&page_keys);
                for (index, key) in &page_items {
                    if let Some(value) = loaded.get(*key) {
                        state.export_crop_page_settings.insert(*index, *value);
                    }
                }
            }
            if let Some(db) = view_trim_db {
                let loaded = db.load_page_overrides_many(&page_keys);
                for (index, key) in &page_items {
                    if let Some(value) = loaded.get(*key) {
                        state.view_trim_page_overrides.insert(*index, *value);
                    }
                }
            }
            if let Some(db) = mask_db {
                let loaded = db.load_existing_mask_keys(&page_keys);
                collect_indices(&mut state.mask_pages, &page_items, &loaded);
            }
            if let Some(db) = conceal_db {
                let loaded = db.load_existing_conceal_keys(&page_keys);
                collect_indices(&mut state.conceal_pages, &page_items, &loaded);
            }
            if let Some(db) = comic_db {
                let loaded = db.load_existing_comic_keys(&page_keys);
                collect_indices(&mut state.comic_pages, &page_items, &loaded);
            }
            if let Some(db) = rotation_db {
                let loaded = db.get_many(page_keys.iter().copied());
                for (index, key) in &page_items {
                    if let Some(rotation) = loaded.get(*key) {
                        state.rotation_cache.insert(*index, *rotation);
                    }
                }
            }
            for item in chunk.iter().filter(|item| item.page_key().is_some()) {
                let has_new_state = state.adjustment_page_params.contains_key(&item.index)
                    || state.local_adjust_pages.contains(&item.index)
                    || state.export_crop_page_settings.contains_key(&item.index)
                    || state.view_trim_page_overrides.contains_key(&item.index)
                    || state.mask_pages.contains(&item.index)
                    || state.conceal_pages.contains(&item.index)
                    || state.comic_pages.contains(&item.index)
                    || state.rotation_cache.contains_key(&item.index);
                if item.had_page_state || has_new_state {
                    state.thumbnail_reset_indices.push(item.index);
                }
            }
            debug_assert!(
                state.thumbnail_reset_indices[reset_start..]
                    .windows(2)
                    .all(|pair| pair[0] < pair[1])
            );
        }
    }

    let current_rating = request
        .current_rating_key
        .as_deref()
        .and_then(|key| rating_db.map(|db| db.get(key)));
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let folder_pin_map = if changed.thumbnail_pins {
        folder_pin_db.map(|db| {
            let mut pins = HashMap::new();
            for chunk in request.items.chunks(DB_KEY_CHUNK) {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                pins.extend(
                    db.lookup_many(
                        chunk
                            .iter()
                            .filter_map(|item| item.container_path.as_deref()),
                    ),
                );
            }
            for chunk in request.folder_pin_paths.chunks(DB_KEY_CHUNK) {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                pins.extend(db.lookup_many(chunk));
            }
            if !cancel.load(Ordering::Relaxed) {
                crate::app::App::apply_folder_pin_aliases(&mut pins, &request.folder_pin_aliases);
            }
            pins
        })
    } else {
        None
    };
    let folder_pin_reset_indices = folder_pin_map.as_ref().map(|pins| {
        let affected = request
            .old_folder_pin_keys
            .iter()
            .chain(pins.keys())
            .collect::<HashSet<_>>();
        request
            .items
            .iter()
            .filter_map(|item| {
                let path = item.container_path.as_deref()?;
                affected
                    .contains(&crate::path_key::normalize_keep_drive(path))
                    .then_some(item.index)
            })
            .collect()
    });
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let video_pin_blobs =
        if changed.thumbnail_pins {
            video_pin_db.map(|db| {
                let mut blobs = HashMap::new();
                // VideoPinDb側も500件ずつqueryするため、同じ境界でcancelを確認する。
                for chunk in request.items.chunks(DB_KEY_CHUNK) {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    blobs.extend(db.lookup_webps_many(
                        chunk.iter().filter_map(|item| item.video_path.as_deref()),
                    ));
                }
                blobs
            })
        } else {
            None
        };
    let video_items = video_pin_blobs.as_ref().map(|_| {
        request
            .items
            .iter_mut()
            .filter_map(|item| {
                let path = item.video_path.take()?;
                Some((item.index, path, item.video_size))
            })
            .collect()
    });
    let container_state =
        if changed.container_state && spread_db.is_some() && container_trim_db.is_some() {
            request.spread_container_path.as_deref().map(|path| {
                let spread_mode = spread_db.and_then(|db| db.get(path));
                let reading_flow = spread_db.and_then(|db| db.get_flow(path));
                let reading_direction = spread_db.and_then(|db| db.get_direction(path));
                let view_trim = container_trim_db.and_then(|db| db.get_book_state(path));
                ContainerStateResult {
                    spread_mode,
                    reading_flow,
                    reading_direction,
                    view_trim,
                }
            })
        } else {
            None
        };

    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    Some(ContextResult {
        slot: request.slot,
        items_generation: request.items_generation,
        rating_cache,
        tags_cache,
        current_rating,
        page_state,
        folder_pin_map,
        folder_pin_reset_indices,
        video_pin_blobs,
        video_items,
        container_state,
        legacy_seed_paths: request.legacy_seed_paths,
    })
}

fn collect_indices(
    destination: &mut HashSet<usize>,
    page_items: &[(usize, &str)],
    loaded: &HashSet<String>,
) {
    for (index, key) in page_items {
        if loaded.contains(*key) {
            destination.insert(*index);
        }
    }
}

impl Default for PageStateResult {
    fn default() -> Self {
        Self {
            adjustment_page_params: HashMap::new(),
            local_adjust_pages: HashSet::new(),
            export_crop_page_settings: HashMap::new(),
            view_trim_page_overrides: HashMap::new(),
            mask_pages: HashSet::new(),
            conceal_pages: HashSet::new(),
            comic_pages: HashSet::new(),
            rotation_cache: HashMap::new(),
            thumbnail_reset_indices: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_reloads_rating_and_tags_for_compact_context_keys() {
        let temp = tempfile::TempDir::new().unwrap();
        let data_dir = temp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let key = "c:/pictures/a.jpg".to_string();
        let untagged_key = "c:/pictures/b.jpg".to_string();
        let tag_only_key = "c:/pictures/search-container".to_string();
        let legacy_seed_path = PathBuf::from("c:/pictures/a.jpg");
        crate::rating_db::RatingDb::open_at(data_dir.join("rating.db"))
            .unwrap()
            .set(&key, 4)
            .unwrap();
        crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db"))
            .unwrap()
            .set_item_tags(&key, ["portable"], crate::tags_db::source::METADATA_IMPORT)
            .unwrap();

        let result = run(
            data_dir,
            vec![ContextRequest {
                slot: ContextSlot::Main,
                items_generation: 7,
                items: vec![
                    ItemKey {
                        index: 3,
                        key: key.clone(),
                        rating: true,
                        tags: true,
                        page: true,
                        had_page_state: false,
                        alternate_page_key: None,
                        container_path: None,
                        video_path: None,
                        video_size: 0,
                    },
                    ItemKey {
                        index: 4,
                        key: untagged_key.clone(),
                        rating: true,
                        tags: true,
                        page: true,
                        had_page_state: false,
                        alternate_page_key: None,
                        container_path: None,
                        video_path: None,
                        video_size: 0,
                    },
                    ItemKey {
                        index: 5,
                        key: tag_only_key.clone(),
                        rating: false,
                        tags: true,
                        page: false,
                        had_page_state: false,
                        alternate_page_key: None,
                        container_path: None,
                        video_path: None,
                        video_size: 0,
                    },
                ],
                legacy_seed_paths: vec![legacy_seed_path.clone()],
                current_rating_key: Some(key.clone()),
                spread_container_path: None,
                old_folder_pin_keys: HashSet::new(),
                folder_pin_paths: Vec::new(),
                folder_pin_aliases: Vec::new(),
            }],
            ImportChangedSections {
                ratings: true,
                tags: true,
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .expect("refresh should complete");

        assert!(result.errors.is_empty());
        let context = &result.contexts[0];
        assert_eq!(context.rating_cache.as_ref().unwrap().get(&3), Some(&4));
        assert_eq!(
            context.tags_cache.as_ref().unwrap().get(&key),
            Some(&vec!["#portable".to_string()])
        );
        assert_eq!(
            context.tags_cache.as_ref().unwrap().get(&untagged_key),
            Some(&Vec::new()),
            "DB行がない対象も、読込済みの空タグsentinelとして返す"
        );
        assert_eq!(
            context.tags_cache.as_ref().unwrap().get(&tag_only_key),
            Some(&Vec::new()),
            "rating対象外でもtag対象なら空sentinelを返す"
        );
        assert!(!context.rating_cache.as_ref().unwrap().contains_key(&5));
        assert_eq!(context.current_rating, Some(4));
        assert_eq!(
            context.legacy_seed_paths,
            vec![legacy_seed_path],
            "legacy seed path ownership must round-trip through the refresh worker"
        );
    }

    #[test]
    fn worker_error_does_not_publish_empty_replacement_cache() {
        let temp = tempfile::TempDir::new().unwrap();
        let result = run(
            temp.path().join("missing"),
            vec![ContextRequest {
                slot: ContextSlot::Main,
                items_generation: 1,
                items: vec![ItemKey {
                    index: 0,
                    key: "c:/missing.jpg".to_string(),
                    rating: true,
                    tags: true,
                    page: false,
                    had_page_state: false,
                    alternate_page_key: None,
                    container_path: None,
                    video_path: None,
                    video_size: 0,
                }],
                legacy_seed_paths: Vec::new(),
                current_rating_key: None,
                spread_container_path: None,
                old_folder_pin_keys: HashSet::new(),
                folder_pin_paths: Vec::new(),
                folder_pin_aliases: Vec::new(),
            }],
            ImportChangedSections {
                ratings: true,
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .expect("refresh should report DB errors");
        assert!(!result.errors.is_empty());
        assert!(result.contexts[0].rating_cache.is_none());
    }

    #[test]
    fn worker_honors_terminal_refresh_cancel_before_database_work() {
        let cancel = AtomicBool::new(true);
        let result = run(
            PathBuf::from("unused"),
            Vec::new(),
            ImportChangedSections {
                ratings: true,
                page_state: true,
                ..Default::default()
            },
            &cancel,
        );
        assert!(result.is_none());
    }

    #[test]
    fn page_refresh_resets_thumbnails_for_added_and_removed_edit_state() {
        let temp = tempfile::TempDir::new().unwrap();
        let data_dir = temp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let added_key = "c:/pictures/added.jpg".to_string();
        let removed_key = "c:/pictures/removed.jpg".to_string();
        crate::adjustment_db::AdjustmentDb::open_at(&data_dir.join("adjustment.db")).unwrap();
        crate::local_adjust_db::LocalAdjustDb::open_at(&data_dir.join("local_adjust.db")).unwrap();
        crate::export_crop::CropDb::open_at(&data_dir.join("export_crop.db")).unwrap();
        crate::view_trim_db::ViewTrimDb::open_at(&data_dir.join("view_trim.db")).unwrap();
        crate::mask_db::MaskDb::open_at(&data_dir.join("mask.db")).unwrap();
        crate::conceal_db::ConcealDb::open_at(&data_dir.join("conceal.db")).unwrap();
        crate::comic_db::ComicDb::open_at(&data_dir.join("comic.db")).unwrap();
        crate::rotation_db::RotationDb::open_at(&data_dir.join("rotation.db"))
            .unwrap()
            .set_key(&added_key, crate::rotation_db::Rotation::Cw90)
            .unwrap();

        let result = run(
            data_dir,
            vec![ContextRequest {
                slot: ContextSlot::Main,
                items_generation: 4,
                items: vec![
                    ItemKey {
                        index: 10,
                        key: added_key,
                        rating: false,
                        tags: false,
                        page: true,
                        had_page_state: false,
                        alternate_page_key: None,
                        container_path: None,
                        video_path: None,
                        video_size: 0,
                    },
                    ItemKey {
                        index: 20,
                        key: removed_key,
                        rating: false,
                        tags: false,
                        page: true,
                        had_page_state: true,
                        alternate_page_key: None,
                        container_path: None,
                        video_path: None,
                        video_size: 0,
                    },
                ],
                legacy_seed_paths: Vec::new(),
                current_rating_key: None,
                spread_container_path: None,
                old_folder_pin_keys: HashSet::new(),
                folder_pin_paths: Vec::new(),
                folder_pin_aliases: Vec::new(),
            }],
            ImportChangedSections {
                page_state: true,
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .expect("refresh should complete");
        let page = result.contexts[0].page_state.as_ref().unwrap();
        assert_eq!(page.thumbnail_reset_indices, vec![10, 20]);
        assert_eq!(
            page.rotation_cache.get(&10),
            Some(&crate::rotation_db::Rotation::Cw90)
        );
    }

    #[test]
    fn worker_restores_folder_aliases_and_reports_video_without_a_remaining_pin() {
        let temp = tempfile::TempDir::new().unwrap();
        let data_dir = temp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let effective = PathBuf::from("c:/books/outer.zip/book/inner");
        let literal = PathBuf::from("c:/books/outer.zip/book");
        let source = crate::folder_thumb_pins::FolderPinSource::ZipEntry {
            zip_rel: String::new(),
            entry: "book/inner/cover.jpg".to_string(),
        };
        crate::folder_thumb_pins::FolderThumbPinDb::open_at(&data_dir.join("folder_thumb_pins.db"))
            .unwrap()
            .set(&effective, &source)
            .unwrap();
        // DBは存在するが対象動画の行はない。これはimportでpinが削除された状態と同じ。
        crate::video_pins::VideoPinDb::open_at(&data_dir.join("video_pins.db")).unwrap();
        let video = PathBuf::from("c:/books/movie.mp4");
        let effective_key = crate::path_key::normalize_keep_drive(&effective);
        let literal_key = crate::path_key::normalize_keep_drive(&literal);

        let result = run(
            data_dir,
            vec![ContextRequest {
                slot: ContextSlot::Main,
                items_generation: 9,
                items: vec![ItemKey {
                    index: 4,
                    key: crate::path_key::normalize_keep_drive(&video),
                    rating: true,
                    tags: true,
                    page: false,
                    had_page_state: false,
                    alternate_page_key: None,
                    container_path: None,
                    video_path: Some(video.clone()),
                    video_size: 123,
                }],
                legacy_seed_paths: Vec::new(),
                current_rating_key: None,
                spread_container_path: None,
                old_folder_pin_keys: HashSet::new(),
                folder_pin_paths: vec![effective],
                folder_pin_aliases: vec![(literal_key.clone(), effective_key)],
            }],
            ImportChangedSections {
                thumbnail_pins: true,
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .expect("refresh should complete");

        let context = &result.contexts[0];
        assert_eq!(
            context
                .folder_pin_map
                .as_ref()
                .and_then(|pins| pins.get(&literal_key)),
            Some(&source)
        );
        assert!(context.video_pin_blobs.as_ref().unwrap().is_empty());
        assert_eq!(
            context.video_items.as_ref().unwrap(),
            &vec![(4, video, 123)],
            "pin削除でも通常サムネ再生成対象に残す"
        );
    }
}
