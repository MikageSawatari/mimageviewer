//! ファイル名 prefix スタック (v2.0.0、`docs/filename-stack-plan.md`) の App 側グルー。
//!
//! 純グループ化ロジックは [`crate::filename_stack`]。ここはそれを `App` の状態
//! (`stack_view` / `stack_mode_requested` / `stack_showing_flat`) とビュー (`self.items`) に橋渡しする。
//!
//! ビューは 2 段 (メンバーグリッドは設けない。1 枚スタックの割合が高く中間グリッドが煩雑なため):
//! - **集約グリッド** (`stack_showing_flat=false`): 1 グループ = 1 セル。複数枚画像はスタックセル +
//!   バッジ、単独は通常 Image/Video セル。コンテナ (フォルダ/ZIP/PDF) は先頭に素通し表示。
//! - **フラット読書フルスクリーン** (`stack_showing_flat=true`): セルを開くと `self.items` を全画像
//!   展開 (materialize_flat) に差し替えてフルスクリーンへ。`↓↑` は境界を越えて順送り、`Shift+↓↑` で
//!   次/前のスタック先頭へジャンプ、`Ctrl+↓↑` はフォルダ移動 (据え置き)。閉じると
//!   `stack_reconcile_after_fullscreen_close` が集約グリッドへ戻す。
//!
//! 集約グリッドの構築は `load_folder_with_scan` の hook 経由。入力は UI frame ごとに
//! 分割して収集し、grouping と集約/flat の materialize・ページ編集投影は worker で行う。
//! 受理時に bundle identity、items generation、request sequence、write stamps を照合する。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use crate::filename_stack::{StackMember, StackView};
use crate::grid_item::GridItem;
use crate::settings::{ListingSortMetadata, SortOrder};

/// ユーザー定義スクリプトによるグループ分けを **ワーカーで** 実行する際の保留状態
/// (`docs/filename-stack-scripting-plan.md`)。スクリプトは任意に重くなり得る (10 万件で
/// ~1 秒) ので UI スレッドで走らせず、通常フォルダを先に表示しつつ裏で計算し、完了後に
/// `poll_stack_script` が集約ビューへ差し替える。
pub(crate) enum StackPreparePending {
    Extracting(StackExtractPending),
    Grouping(StackScriptPending),
    Switching(StackSwitchPending),
    Retry(StackRetryPending),
}

/// The accepted aggregate stays with the same bundle while fullscreen uses the flat order.
/// Refreshing owns the aggregate grid until the replacement projection is accepted.
pub(crate) enum StackReturnState {
    Ready {
        view: Arc<StackView>,
        prepared: StackPreparedItems,
    },
    Refreshing {
        view: Arc<StackView>,
    },
    /// The retained aggregate is interactive after sustained read failure. Its worker retry
    /// remains owned by this bundle until a fresh projection lands or the view changes.
    Recovering {
        view: Arc<StackView>,
    },
}

impl StackPreparePending {
    fn cancel(&self) {
        let token = match self {
            Self::Extracting(pending) => &pending.cancel,
            Self::Grouping(pending) => &pending.cancel,
            Self::Switching(pending) => &pending.cancel,
            Self::Retry(pending) => &pending.cancel,
        };
        token.store(true, Ordering::Relaxed);
    }
}

struct StackGroupingSource {
    folder: PathBuf,
    sort_path: PathBuf,
    items: Vec<GridItem>,
    image_metas: Vec<Option<(i64, i64)>>,
    listing: StackListingSource,
    separator: char,
    sort: SortOrder,
    script_enabled: bool,
    group_per_parent: bool,
    display_order: crate::settings::GridDisplayOrder,
    available_edits: crate::app::page_edit_snapshot::PageEditAvailability,
    rating_available: bool,
    tags_available: bool,
    #[cfg(test)]
    group_runs: Option<Arc<std::sync::atomic::AtomicUsize>>,
    #[cfg(test)]
    read_failures: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

#[derive(Clone)]
pub(crate) enum StackListingSource {
    Normal(Arc<Vec<ListingSortMetadata>>),
    Subfolder(Arc<Vec<crate::app::SubfolderExpansionEntry>>),
}

pub(crate) struct StackExtractPending {
    cancel: Arc<AtomicBool>,
    source: StackGroupingSource,
    existing_keys: std::collections::HashSet<String>,
    folder_signature: Option<u64>,
    context_id: crate::app::ViewerContextId,
    items_generation: u64,
    item_count: usize,
    sequence: u64,
}

pub(crate) struct StackScriptPending {
    cancel: Arc<AtomicBool>,
    rx: Receiver<Result<Option<StackGroupReady>, String>>,
    source: Arc<StackGroupingSource>,
    context_id: crate::app::ViewerContextId,
    items_generation: u64,
    item_count: usize,
    sequence: u64,
    existing_keys: std::collections::HashSet<String>,
    folder_signature: Option<u64>,
    retry_attempt: u8,
}

struct StackGroupReady {
    view: Arc<StackView>,
    read: StackReadOutcome,
    rule: Option<String>,
    error: Option<String>,
}

struct StackCandidateOrder {
    items: Vec<GridItem>,
    metas: Vec<Option<(i64, i64)>>,
    videos: Vec<(usize, PathBuf, u64)>,
    #[cfg(test)]
    read_failures: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

impl StackCandidateOrder {
    fn new(items: Vec<GridItem>, metas: Vec<Option<(i64, i64)>>) -> Self {
        let videos = stack_video_items(&items, &metas);
        Self {
            items,
            metas,
            videos,
            #[cfg(test)]
            read_failures: None,
        }
    }
}

struct StackReadData {
    edits: crate::app::page_edit_snapshot::StablePageEditProjection,
    rating_cache: std::collections::HashMap<usize, u8>,
    tags_cache: std::collections::HashMap<String, Vec<String>>,
    rating_stamp: crate::page_edit_write_epoch::WriteStamp,
    tag_stamp: crate::page_edit_write_epoch::WriteStamp,
}

struct StackReadOutcome {
    order: StackCandidateOrder,
    result: Result<Option<StackReadData>, String>,
}

struct StackSwitchReady {
    target: StackReadOutcome,
    retained_aggregate: Option<StackReadOutcome>,
}

pub(crate) struct StackPreparedItems {
    pub items: Vec<GridItem>,
    pub metas: Vec<Option<(i64, i64)>>,
    pub videos: Vec<(usize, PathBuf, u64)>,
    pub edits: crate::app::page_edit_snapshot::StablePageEditProjection,
    pub rating_cache: std::collections::HashMap<usize, u8>,
    pub tags_cache: std::collections::HashMap<String, Vec<String>>,
    pub rating_stamp: crate::page_edit_write_epoch::WriteStamp,
    pub tag_stamp: crate::page_edit_write_epoch::WriteStamp,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StackSwitchTarget {
    Flat { index: usize },
    Aggregated { select: Option<usize> },
}

pub(crate) struct StackSwitchPending {
    cancel: Arc<AtomicBool>,
    rx: Receiver<StackSwitchReady>,
    view: Arc<StackView>,
    target: StackSwitchTarget,
    context_id: crate::app::ViewerContextId,
    items_generation: u64,
    item_count: usize,
    sequence: u64,
    retry_attempt: u8,
}

enum StackRetrySource {
    GroupFresh(
        Arc<StackGroupingSource>,
        std::collections::HashSet<String>,
        Option<u64>,
    ),
    GroupReady(
        Arc<StackGroupingSource>,
        Arc<StackView>,
        StackCandidateOrder,
        Option<String>,
        Option<String>,
        std::collections::HashSet<String>,
        Option<u64>,
    ),
    SwitchReady(
        Arc<StackView>,
        StackSwitchTarget,
        StackCandidateOrder,
        Option<StackCandidateOrder>,
    ),
    SwitchFresh(Arc<StackView>, StackSwitchTarget),
}

pub(crate) struct StackRetryPending {
    cancel: Arc<AtomicBool>,
    source: StackRetrySource,
    context_id: crate::app::ViewerContextId,
    items_generation: u64,
    item_count: usize,
    sequence: u64,
    due: std::time::Instant,
    retry_attempt: u8,
}

/// 通常フォルダの items から、集約に必要な材料 (passthrough / passthrough_metas / media) を
/// **非破壊で** 取り出す (`build_stack_aggregated` の前半と同じだが items を消費しない。
/// ワーカー経路では items をそのまま通常表示に使うため)。
pub(crate) fn extract_stack_parts(
    items: &[GridItem],
    image_metas: &[Option<(i64, i64)>],
    listing_sort_metas: &[ListingSortMetadata],
) -> (
    Vec<GridItem>,
    Vec<Option<(i64, i64)>>,
    Vec<ListingSortMetadata>,
    Vec<StackMember>,
) {
    assert_eq!(items.len(), image_metas.len());
    assert_eq!(items.len(), listing_sort_metas.len());
    let mut passthrough: Vec<GridItem> = Vec::new();
    let mut passthrough_metas: Vec<Option<(i64, i64)>> = Vec::new();
    let mut passthrough_sort_metas: Vec<ListingSortMetadata> = Vec::new();
    let mut media: Vec<StackMember> = Vec::new();
    for index in 0..items.len() {
        let it = &items[index];
        let meta = &image_metas[index];
        let (mtime, _) = meta.unwrap_or((0, 0));
        let sort_meta = listing_sort_metas[index];
        let sort_meta = crate::grid_item::listing_sort_metadata_for_item(it, sort_meta);
        match it {
            GridItem::Image(path) => media.push(StackMember {
                path: path.clone(),
                mtime,
                size: sort_meta.file_size,
                is_video: false,
            }),
            GridItem::Video(path) => media.push(StackMember {
                path: path.clone(),
                mtime,
                size: sort_meta.file_size,
                is_video: true,
            }),
            // 想定外種別は素通し (build_stack_aggregated と同じ防御)。
            other => {
                passthrough.push(other.clone());
                passthrough_metas.push(*meta);
                passthrough_sort_metas.push(sort_meta);
            }
        }
    }
    (
        passthrough,
        passthrough_metas,
        passthrough_sort_metas,
        media,
    )
}

/// 集約 / メンバービューの items から動画セルの `(idx, path, size)` を集める
/// (`start_loading_items` の video サムネスレッド用、元の媒体ループと同形式)。
pub(crate) fn stack_video_items(
    items: &[GridItem],
    metas: &[Option<(i64, i64)>],
) -> Vec<(usize, PathBuf, u64)> {
    items
        .iter()
        .enumerate()
        .filter_map(|(idx, it)| {
            if let GridItem::Video(p) = it {
                let size = metas
                    .get(idx)
                    .and_then(|m| *m)
                    .map(|(_, s)| s.max(0) as u64)
                    .unwrap_or(0);
                Some((idx, p.clone(), size))
            } else {
                None
            }
        })
        .collect()
}

fn stack_rating_key(item: &GridItem) -> Option<String> {
    if !item.accepts_rating() {
        return None;
    }
    match item {
        GridItem::Image(_) | GridItem::ZipImage { .. } | GridItem::PdfPage { .. } => {
            crate::edit_source::page_key_for_grid_item(item)
        }
        GridItem::Folder(path)
        | GridItem::ZipFile(path)
        | GridItem::PdfFile(path)
        | GridItem::Video(path)
        | GridItem::Audio(path) => Some(crate::adjustment_db::normalize_path(path)),
        GridItem::ConvertibleArchive { path, .. } => {
            Some(crate::adjustment_db::normalize_path(path))
        }
        _ => None,
    }
}

fn read_stack_candidate(
    order: StackCandidateOrder,
    available_edits: crate::app::page_edit_snapshot::PageEditAvailability,
    rating_available: bool,
    tags_available: bool,
    cancel: &AtomicBool,
) -> StackReadOutcome {
    let result = (|| -> Result<Option<StackReadData>, String> {
        #[cfg(test)]
        if order.read_failures.as_ref().is_some_and(|remaining| {
            remaining
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                    count.checked_sub(1)
                })
                .is_ok()
        }) {
            return Err("injected stack read failure".into());
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let Some(edits) =
            crate::app::page_edit_snapshot::PageEditSnapshot::load_and_project_stable(
                &order.items,
                available_edits,
                cancel,
            )?
        else {
            return Ok(None);
        };
        let rating_before = crate::rating_db::RATING_WRITES.sample();
        if rating_before.active_writers != 0 {
            return Ok(None);
        }
        let idx_keys = order
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| stack_rating_key(item).map(|key| (index, key)))
            .collect::<Vec<_>>();
        let keys = idx_keys
            .iter()
            .map(|(_, key)| key.clone())
            .collect::<Vec<_>>();
        let ratings = if rating_available && !keys.is_empty() {
            crate::rating_db::RatingDb::open_readonly(crate::rating_db::RatingDb::db_path())
                .map_err(|error| format!("rating DB read failed: {error}"))?
                .get_many(&keys)
        } else {
            Default::default()
        };
        let rating_cache = idx_keys
            .into_iter()
            .map(|(index, key)| (index, ratings.get(&key).copied().unwrap_or(0)))
            .collect();
        let rating_stamp = crate::rating_db::RATING_WRITES.sample();
        if !crate::page_edit_write_epoch::EditWriteEpoch::read_is_stable(
            rating_before,
            rating_stamp,
        ) {
            return Ok(None);
        }
        let tag_before = crate::tags_db::TAG_WRITES.sample();
        if tag_before.active_writers != 0 {
            return Ok(None);
        }
        let tag_keys = order
            .items
            .iter()
            .filter_map(crate::app::tag_item_path)
            .map(crate::tags_db::item_key_for_path)
            .collect::<std::collections::HashSet<_>>();
        let mut loaded_tags = if tags_available && !tag_keys.is_empty() {
            crate::tags_db::TagsDb::open_readonly(&crate::tags_db::TagsDb::db_path())
                .map_err(|error| format!("tags DB read failed: {error}"))?
                .get_many_display_tags(&tag_keys.iter().cloned().collect::<Vec<_>>())
        } else {
            Default::default()
        };
        let tags_cache = tag_keys
            .into_iter()
            .map(|key| {
                let tags = loaded_tags.remove(&key).unwrap_or_default();
                (key, tags)
            })
            .collect();
        let tag_stamp = crate::tags_db::TAG_WRITES.sample();
        if !crate::page_edit_write_epoch::EditWriteEpoch::read_is_stable(tag_before, tag_stamp)
            || cancel.load(Ordering::Relaxed)
        {
            return Ok(None);
        }
        Ok(Some(StackReadData {
            edits,
            rating_cache,
            tags_cache,
            rating_stamp,
            tag_stamp,
        }))
    })();
    StackReadOutcome { order, result }
}

impl StackReadOutcome {
    fn into_prepared(self) -> Result<Option<StackPreparedItems>, String> {
        let data = match self.result? {
            Some(data) => data,
            None => return Ok(None),
        };
        let StackCandidateOrder {
            items,
            metas,
            videos,
            ..
        } = self.order;
        Ok(Some(StackPreparedItems {
            items,
            metas,
            videos,
            edits: data.edits,
            rating_cache: data.rating_cache,
            tags_cache: data.tags_cache,
            rating_stamp: data.rating_stamp,
            tag_stamp: data.tag_stamp,
        }))
    }
}

impl StackReadData {
    fn stamps_current(&self) -> bool {
        crate::page_edit_write_epoch::PAGE_EDIT_WRITES.accepts(self.edits.stamp)
            && crate::rating_db::RATING_WRITES.accepts(self.rating_stamp)
            && crate::tags_db::TAG_WRITES.accepts(self.tag_stamp)
    }
}

fn stack_script_keys_for_images(
    images: &[StackMember],
    source: &str,
    cancel: Arc<AtomicBool>,
    group_per_parent: bool,
) -> Result<(Vec<String>, Option<String>), String> {
    if !group_per_parent {
        return crate::filename_stack_script::group_keys_cancellable(images, source, cancel)
            .map(|r| (r.keys, r.rule));
    }

    let script =
        crate::filename_stack_script::CompiledStackScript::compile(source, Arc::clone(&cancel))?;
    let mut order: Vec<String> = Vec::new();
    let mut buckets: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, image) in images.iter().enumerate() {
        let parent_key = image
            .path
            .parent()
            .map(crate::adjustment_db::normalize_path)
            .unwrap_or_default();
        if !buckets.contains_key(&parent_key) {
            order.push(parent_key.clone());
        }
        buckets.entry(parent_key).or_default().push(idx);
    }

    let mut keys = vec![String::new(); images.len()];
    let mut rule: Option<String> = None;
    let mut mixed_rule = false;
    for parent_key in order {
        if cancel.load(Ordering::Relaxed) {
            return Err("キャンセルされました".to_string());
        }
        let Some(indices) = buckets.get(&parent_key) else {
            continue;
        };
        let scoped_images: Vec<StackMember> =
            indices.iter().map(|&idx| images[idx].clone()).collect();
        let result = script.group_keys(&scoped_images)?;
        match result.rule {
            Some(next) => match rule.as_ref() {
                None => rule = Some(next),
                Some(current) if current == &next => {}
                Some(_) => mixed_rule = true,
            },
            None => {}
        }
        for (&image_idx, key) in indices.iter().zip(result.keys.into_iter()) {
            if let Some(slot) = keys.get_mut(image_idx) {
                *slot = crate::filename_stack::parent_scoped_key_for_path(
                    &images[image_idx].path,
                    &key,
                );
            }
        }
    }

    let rule = if mixed_rule {
        Some("親フォルダ別".to_string())
    } else {
        rule
    };
    Ok((keys, rule))
}

impl crate::app::App {
    /// 進行中のスタックスクリプトワーカーをキャンセルして破棄する。
    pub(crate) fn cancel_stack_script_pending(&mut self) {
        if let Some(p) = self.stack_script_pending.take() {
            p.cancel();
        }
        self.stack_request_sequence = self.stack_request_sequence.wrapping_add(1);
    }

    /// ユーザー定義スクリプトによるグループ分けをワーカーで開始する。通常フォルダは既に
    /// 表示済みで、完了後 `poll_stack_script` が集約ビューへ差し替える。
    ///
    /// `script_enabled` = 「分類ルールをスクリプトで行う」設定。true ならユーザーの
    /// `stack_rules.rhai` (無ければ内蔵既定)、false なら内蔵 `DEFAULT_SCRIPT` を使う。
    /// **ソースの読み込み (ファイル I/O) はワーカースレッド内で行う** (= UI スレッドに
    /// `read_to_string` を乗せない)。
    pub(crate) fn spawn_stack_script_worker(
        &mut self,
        folder: PathBuf,
        sort_path: PathBuf,
        listing: StackListingSource,
        separator: char,
        sort: SortOrder,
        existing_keys: std::collections::HashSet<String>,
        folder_signature: Option<u64>,
        script_enabled: bool,
        group_per_parent: bool,
    ) {
        self.cancel_stack_script_pending();
        let source = StackGroupingSource {
            folder,
            sort_path,
            items: Vec::new(),
            image_metas: Vec::new(),
            listing,
            separator,
            sort,
            script_enabled,
            group_per_parent,
            display_order: self.settings.grid_display_order.clone(),
            available_edits: crate::app::page_edit_snapshot::PageEditAvailability::for_app(self),
            rating_available: self.rating_db.is_some(),
            tags_available: self.tags_db.is_some(),
            #[cfg(test)]
            group_runs: None,
            #[cfg(test)]
            read_failures: None,
        };
        self.stack_script_pending = Some(StackPreparePending::Extracting(StackExtractPending {
            cancel: Arc::new(AtomicBool::new(false)),
            source,
            existing_keys,
            folder_signature,
            context_id: self.virtual_list_context_id(),
            items_generation: self.items_generation,
            item_count: self.items.len(),
            sequence: self.stack_request_sequence,
        }));
    }

    fn spawn_stack_grouping_source(
        &mut self,
        source: Arc<StackGroupingSource>,
        existing_keys: std::collections::HashSet<String>,
        folder_signature: Option<u64>,
        retry_attempt: u8,
    ) {
        self.cancel_stack_script_pending();
        let sequence = self.stack_request_sequence;
        let context_id = self.virtual_list_context_id();
        let items_generation = self.items_generation;
        let item_count = self.items.len();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_w = Arc::clone(&cancel);
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_source = Arc::clone(&source);
        let repaint = crate::page_edit_write_epoch::PAGE_EDIT_WRITES.repaint_context();
        #[cfg(test)]
        let test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::capture();
        let spawned = std::thread::Builder::new()
            .name("stack-script".into())
            .spawn(move || {
                #[cfg(test)]
                let _test_epoch_scope =
                    test_epoch_scope.map(crate::page_edit_write_epoch::TestEpochScope::enter);
                #[cfg(test)]
                if let Some(runs) = &worker_source.group_runs {
                    runs.fetch_add(1, Ordering::Relaxed);
                }
                let listing_sort_metas = match &worker_source.listing {
                    StackListingSource::Normal(metas) => metas.as_ref().clone(),
                    StackListingSource::Subfolder(entries) => {
                        crate::app::listing_sort_metas_for_entries(entries, &worker_source.items)
                    }
                };
                let (passthrough, passthrough_metas, passthrough_sort_metas, media) =
                    extract_stack_parts(
                        &worker_source.items,
                        &worker_source.image_metas,
                        &listing_sort_metas,
                    );
                let source_text = if worker_source.script_enabled {
                    crate::filename_stack_script::active_script_source()
                } else {
                    crate::filename_stack_script::DEFAULT_SCRIPT.to_string()
                };
                let image_indices = media
                    .iter()
                    .enumerate()
                    .filter(|(_, member)| !member.is_video)
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                let images = image_indices
                    .iter()
                    .map(|&index| media[index].clone())
                    .collect::<Vec<_>>();
                let grouping = stack_script_keys_for_images(
                    &images,
                    &source_text,
                    Arc::clone(&cancel_w),
                    worker_source.group_per_parent,
                );
                if cancel_w.load(Ordering::Relaxed) {
                    return;
                }
                let (groups, rule, error) = match grouping {
                    Ok((keys, rule)) => {
                        let mut full = vec![String::new(); media.len()];
                        for (key, &index) in keys.into_iter().zip(&image_indices) {
                            full[index] = key;
                        }
                        (
                            crate::filename_stack::group_by_keys(
                                media.clone(),
                                &full,
                                worker_source.sort,
                            ),
                            rule,
                            None,
                        )
                    }
                    Err(error) => {
                        crate::logger::log(format!(
                            "stack script failed (async), fallback to builtin: {error}"
                        ));
                        let groups = if worker_source.group_per_parent {
                            crate::filename_stack::group_media_by_parent(
                                media.clone(),
                                worker_source.separator,
                                worker_source.sort,
                            )
                        } else {
                            crate::filename_stack::group_media(
                                media.clone(),
                                worker_source.separator,
                                worker_source.sort,
                            )
                        };
                        (groups, None, Some(error))
                    }
                };
                let view = Arc::new(StackView::from_groups_with_display_order(
                    worker_source.folder.clone(),
                    passthrough,
                    passthrough_metas,
                    passthrough_sort_metas,
                    worker_source.separator,
                    worker_source.sort,
                    groups,
                    worker_source.display_order.clone(),
                ));
                let (items, metas) = view.materialize_aggregated();
                let mut order = StackCandidateOrder::new(items, metas);
                #[cfg(test)]
                {
                    order.read_failures = worker_source.read_failures.clone();
                }
                let read = read_stack_candidate(
                    order,
                    worker_source.available_edits,
                    worker_source.rating_available,
                    worker_source.tags_available,
                    &cancel_w,
                );
                let _ = tx.send(Ok(Some(StackGroupReady {
                    view,
                    read,
                    rule,
                    error,
                })));
                if let Some(ctx) = repaint {
                    ctx.request_repaint();
                }
            });
        if spawned.is_err() {
            self.stack_script_error =
                Some("スタックスクリプトのワーカーを起動できませんでした".into());
            return;
        }
        self.stack_script_pending = Some(StackPreparePending::Grouping(StackScriptPending {
            cancel,
            rx,
            source,
            context_id,
            items_generation,
            item_count,
            sequence,
            existing_keys,
            folder_signature,
            retry_attempt,
        }));
    }

    fn spawn_stack_group_read(
        &mut self,
        ctx: &egui::Context,
        source: Arc<StackGroupingSource>,
        view: Arc<StackView>,
        order: StackCandidateOrder,
        rule: Option<String>,
        error: Option<String>,
        existing_keys: std::collections::HashSet<String>,
        folder_signature: Option<u64>,
        retry_attempt: u8,
    ) {
        self.cancel_stack_script_pending();
        let sequence = self.stack_request_sequence;
        let context_id = self.virtual_list_context_id();
        let items_generation = self.items_generation;
        let item_count = self.items.len();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let repaint = ctx.clone();
        let availability = source.available_edits;
        let rating_available = source.rating_available;
        let tags_available = source.tags_available;
        let (tx, rx) = std::sync::mpsc::channel();
        #[cfg(test)]
        let test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::capture();
        let spawned = std::thread::Builder::new()
            .name("stack-group-read".into())
            .spawn(move || {
                #[cfg(test)]
                let _test_epoch_scope =
                    test_epoch_scope.map(crate::page_edit_write_epoch::TestEpochScope::enter);
                let read = read_stack_candidate(
                    order,
                    availability,
                    rating_available,
                    tags_available,
                    &worker_cancel,
                );
                let _ = tx.send(Ok(Some(StackGroupReady {
                    view,
                    read,
                    rule,
                    error,
                })));
                repaint.request_repaint();
            });
        if spawned.is_err() {
            self.schedule_stack_retry(
                ctx,
                StackRetrySource::GroupFresh(source, existing_keys, folder_signature),
                true,
                retry_attempt,
            );
            return;
        }
        self.stack_script_pending = Some(StackPreparePending::Grouping(StackScriptPending {
            cancel,
            rx,
            source,
            context_id,
            items_generation,
            item_count,
            sequence,
            existing_keys,
            folder_signature,
            retry_attempt,
        }));
    }

    /// Accept only a candidate prepared for the mounted bundle and its exact source order.
    pub(crate) fn poll_stack_script(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.stack_script_pending.take() else {
            return;
        };
        match pending {
            StackPreparePending::Extracting(pending) => self.poll_stack_extracting(ctx, pending),
            StackPreparePending::Grouping(pending) => self.poll_stack_grouping(ctx, pending),
            StackPreparePending::Switching(pending) => self.poll_stack_switch(ctx, pending),
            StackPreparePending::Retry(pending) => self.poll_stack_retry(ctx, pending),
        }
    }

    fn poll_stack_extracting(&mut self, ctx: &egui::Context, mut pending: StackExtractPending) {
        let current = !pending.cancel.load(Ordering::Relaxed)
            && self.stack_mode_requested
            && !self.stack_showing_flat
            && self.stack_view.is_none()
            && self.virtual_list_context_id() == pending.context_id
            && self.items_generation == pending.items_generation
            && self.items.len() == pending.item_count
            && self.stack_request_sequence == pending.sequence
            && self
                .current_folder
                .as_ref()
                .is_some_and(|folder| crate::folder_tree::path_eq(folder, &pending.source.folder))
            && match &pending.source.listing {
                StackListingSource::Normal(_) => true,
                StackListingSource::Subfolder(entries) => self
                    .subfolder_expansion_snapshot
                    .as_ref()
                    .is_some_and(|snapshot| Arc::ptr_eq(&snapshot.entries, entries)),
            };
        if !current {
            return;
        }
        if !self.stack_group_config_current(&pending.source) {
            self.rebase_stack_group_source(
                &pending.source,
                pending.existing_keys,
                pending.folder_signature,
            );
            ctx.request_repaint();
            return;
        }
        let start = pending.source.items.len();
        let end = (start + 2048).min(pending.item_count);
        pending
            .source
            .items
            .extend(self.items[start..end].iter().cloned());
        pending
            .source
            .image_metas
            .extend_from_slice(&self.image_metas[start..end]);
        if end < pending.item_count {
            self.stack_script_pending = Some(StackPreparePending::Extracting(pending));
            ctx.request_repaint();
        } else {
            self.spawn_stack_grouping_source(
                Arc::new(pending.source),
                pending.existing_keys,
                pending.folder_signature,
                0,
            );
        }
    }

    fn stack_prepared_stamps_current(prepared: &StackPreparedItems) -> bool {
        crate::page_edit_write_epoch::PAGE_EDIT_WRITES.accepts(prepared.edits.stamp)
            && crate::rating_db::RATING_WRITES.accepts(prepared.rating_stamp)
            && crate::tags_db::TAG_WRITES.accepts(prepared.tag_stamp)
    }

    fn stack_group_config_current(&self, source: &StackGroupingSource) -> bool {
        source.separator == self.settings.stack_separator
            && source.script_enabled == self.settings.stack_script_enabled
            && source.sort == self.book_sort_order_for_path(&source.sort_path)
            && source.display_order == self.settings.grid_display_order
    }

    fn rebase_stack_group_source(
        &mut self,
        source: &StackGroupingSource,
        existing_keys: std::collections::HashSet<String>,
        folder_signature: Option<u64>,
    ) {
        let sort = self.book_sort_order_for_path(&source.sort_path);
        self.spawn_stack_script_worker(
            source.folder.clone(),
            source.sort_path.clone(),
            source.listing.clone(),
            self.settings.stack_separator,
            sort,
            existing_keys,
            folder_signature,
            self.settings.stack_script_enabled,
            source.group_per_parent,
        );
    }

    fn poll_stack_grouping(&mut self, ctx: &egui::Context, pending: StackScriptPending) {
        let current = self.stack_mode_requested
            && !self.stack_showing_flat
            && self.stack_view.is_none()
            && self.virtual_list_context_id() == pending.context_id
            && self.items_generation == pending.items_generation
            && self.items.len() == pending.item_count
            && self.stack_request_sequence == pending.sequence
            && match &pending.source.listing {
                StackListingSource::Normal(_) => true,
                StackListingSource::Subfolder(entries) => self
                    .subfolder_expansion_snapshot
                    .as_ref()
                    .is_some_and(|snapshot| Arc::ptr_eq(&snapshot.entries, entries)),
            }
            && self
                .current_folder
                .as_ref()
                .is_some_and(|folder| crate::folder_tree::path_eq(folder, &pending.source.folder));
        if !current {
            pending.cancel.store(true, Ordering::Relaxed);
            return;
        }
        if !self.stack_group_config_current(&pending.source) {
            pending.cancel.store(true, Ordering::Relaxed);
            self.rebase_stack_group_source(
                &pending.source,
                pending.existing_keys,
                pending.folder_signature,
            );
            ctx.request_repaint();
            return;
        }
        // Main fullscreen retains the old list. A detached window is parked only when the
        // complete candidate is accepted, preserving the existing park timing.
        if self.fullscreen_idx.is_some() && !self.viewer_session_is_detached() {
            self.stack_script_pending = Some(StackPreparePending::Grouping(pending));
            return;
        }
        let received = match pending.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.stack_script_pending = Some(StackPreparePending::Grouping(pending));
                return;
            }
            Err(TryRecvError::Disconnected) => Err("stack worker disconnected".into()),
        };
        let ready = match received {
            Ok(Some(ready)) => ready,
            outcome => {
                if let Err(error) = outcome {
                    crate::logger::log(format!("stack grouping failed: {error}"));
                }
                self.schedule_stack_retry(
                    ctx,
                    StackRetrySource::GroupFresh(
                        pending.source,
                        pending.existing_keys,
                        pending.folder_signature,
                    ),
                    true,
                    pending.retry_attempt,
                );
                return;
            }
        };
        let StackGroupReady {
            view,
            read,
            rule,
            error,
        } = ready;
        let valid = read
            .result
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .is_some_and(StackReadData::stamps_current);
        if !valid {
            let failed = read.result.is_err();
            if let Err(problem) = &read.result {
                crate::logger::log(format!("stack prepare failed: {problem}"));
            }
            self.schedule_stack_retry(
                ctx,
                StackRetrySource::GroupReady(
                    pending.source,
                    view,
                    read.order,
                    rule,
                    error,
                    pending.existing_keys,
                    pending.folder_signature,
                ),
                failed,
                pending.retry_attempt,
            );
            return;
        }
        let prepared = read.into_prepared().unwrap().unwrap();
        #[cfg(windows)]
        if self.fullscreen_idx.is_some() {
            self.park_detached_session_for_stack_aggregation(ctx);
        }
        if self.fullscreen_idx.is_some() {
            self.close_fullscreen();
        }
        self.apply_stack_group_ready(
            pending.source.folder.clone(),
            pending.existing_keys,
            pending.folder_signature,
            view,
            prepared,
            rule,
            error,
        );
    }

    fn apply_stack_group_ready(
        &mut self,
        folder: PathBuf,
        existing_keys: std::collections::HashSet<String>,
        folder_signature: Option<u64>,
        view: Arc<StackView>,
        prepared: StackPreparedItems,
        rule: Option<String>,
        error: Option<String>,
    ) {
        let collapsible = view.has_collapsible_stack();
        let is_subfolder_expansion_stack =
            crate::folder_tree::path_eq(&folder, &crate::app::subfolder_expansion_synthetic_path());
        let pin_map = std::mem::take(&mut self.folder_pin_map);
        self.start_loading_stack_items(folder, prepared, existing_keys, folder_signature, pin_map);
        if is_subfolder_expansion_stack {
            self.restore_subfolder_expansion_view_state_after_items_install();
        }
        self.stack_mode_requested = true;
        self.stack_view = Some(view);
        self.stack_active_rule = rule.clone();
        self.stack_script_error = error.clone();
        if let Some(target) = self.stack_toggle_select_path.take()
            && let Some(idx) = self
                .stack_view
                .as_ref()
                .and_then(|view| view.aggregated_index_for_member_path(&target))
        {
            self.selected = Some(idx);
            self.scroll_to_selected = true;
        }
        if error.is_some() {
            self.show_feedback_toast(
                "スタックのスクリプトでエラー。既定ルールで表示します (詳細はヘルプ参照)".into(),
            );
        } else if !collapsible {
            self.show_feedback_toast(
                "まとめられるスタックがありませんでした (分類ルールはヘルプ参照)".into(),
            );
        } else if let Some(rule) = rule {
            self.show_feedback_toast(format!("スタック: 「{rule}」でまとめました"));
        }
    }

    fn start_stack_switch(
        &mut self,
        ctx: &egui::Context,
        view: Arc<StackView>,
        target: StackSwitchTarget,
    ) {
        self.start_stack_switch_with_order(ctx, view, target, None, 0);
    }

    fn start_stack_switch_with_order(
        &mut self,
        ctx: &egui::Context,
        view: Arc<StackView>,
        target: StackSwitchTarget,
        orders: Option<(StackCandidateOrder, Option<StackCandidateOrder>)>,
        retry_attempt: u8,
    ) {
        self.cancel_stack_script_pending();
        let sequence = self.stack_request_sequence;
        let context_id = self.virtual_list_context_id();
        let items_generation = self.items_generation;
        let item_count = self.items.len();
        let available_edits = crate::app::page_edit_snapshot::PageEditAvailability::for_app(self);
        let rating_available = self.rating_db.is_some();
        let tags_available = self.tags_db.is_some();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_view = Arc::clone(&view);
        let repaint = ctx.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        #[cfg(test)]
        let test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::capture();
        let spawned = std::thread::Builder::new()
            .name("stack-view-prepare".into())
            .spawn(move || {
                #[cfg(test)]
                let _test_epoch_scope =
                    test_epoch_scope.map(crate::page_edit_write_epoch::TestEpochScope::enter);
                let (target_order, retained_order) = match orders {
                    Some(orders) => orders,
                    None => {
                        let (items, metas) = match target {
                            StackSwitchTarget::Flat { .. } => worker_view.materialize_flat(),
                            StackSwitchTarget::Aggregated { .. } => {
                                worker_view.materialize_aggregated()
                            }
                        };
                        let retained =
                            matches!(target, StackSwitchTarget::Flat { .. }).then(|| {
                                let (items, metas) = worker_view.materialize_aggregated();
                                StackCandidateOrder::new(items, metas)
                            });
                        (StackCandidateOrder::new(items, metas), retained)
                    }
                };
                let target_read = read_stack_candidate(
                    target_order,
                    available_edits,
                    rating_available,
                    tags_available,
                    &worker_cancel,
                );
                let retained_aggregate = retained_order.map(|order| {
                    read_stack_candidate(
                        order,
                        available_edits,
                        rating_available,
                        tags_available,
                        &worker_cancel,
                    )
                });
                let _ = tx.send(StackSwitchReady {
                    target: target_read,
                    retained_aggregate,
                });
                repaint.request_repaint();
            });
        if spawned.is_err() {
            self.schedule_stack_retry(
                ctx,
                StackRetrySource::SwitchFresh(view, target),
                true,
                retry_attempt,
            );
            return;
        }
        self.stack_script_pending = Some(StackPreparePending::Switching(StackSwitchPending {
            cancel,
            rx,
            view,
            target,
            context_id,
            items_generation,
            item_count,
            sequence,
            retry_attempt,
        }));
    }

    fn poll_stack_switch(&mut self, ctx: &egui::Context, pending: StackSwitchPending) {
        let current = self.stack_mode_requested
            && self.virtual_list_context_id() == pending.context_id
            && self.items_generation == pending.items_generation
            && self.items.len() == pending.item_count
            && self.stack_request_sequence == pending.sequence
            && self
                .stack_view
                .as_ref()
                .is_some_and(|view| Arc::ptr_eq(view, &pending.view))
            && self
                .current_folder
                .as_ref()
                .is_some_and(|folder| crate::folder_tree::path_eq(folder, &pending.view.folder))
            && match pending.target {
                StackSwitchTarget::Flat { .. } => {
                    !self.stack_showing_flat && self.fullscreen_idx.is_none()
                }
                StackSwitchTarget::Aggregated { .. } => {
                    !self.stack_showing_flat
                        && self.fullscreen_idx.is_none()
                        && matches!(self.stack_return_state.as_ref(),
                            Some(StackReturnState::Refreshing { view } | StackReturnState::Recovering { view })
                                if Arc::ptr_eq(view, &pending.view))
                }
            };
        if !current {
            pending.cancel.store(true, Ordering::Relaxed);
            return;
        }
        let received = match pending.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.stack_script_pending = Some(StackPreparePending::Switching(pending));
                return;
            }
            Err(TryRecvError::Disconnected) => {
                self.schedule_stack_retry(
                    ctx,
                    StackRetrySource::SwitchFresh(pending.view, pending.target),
                    true,
                    pending.retry_attempt,
                );
                return;
            }
        };
        let StackSwitchReady {
            target,
            retained_aggregate,
        } = received;
        let target_valid = target
            .result
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .is_some_and(StackReadData::stamps_current);
        let retained_valid = match retained_aggregate.as_ref() {
            Some(read) => read
                .result
                .as_ref()
                .ok()
                .and_then(Option::as_ref)
                .is_some_and(StackReadData::stamps_current),
            None => matches!(pending.target, StackSwitchTarget::Aggregated { .. }),
        };
        if target_valid && retained_valid {
            let prepared = target.into_prepared().unwrap().unwrap();
            let retained = retained_aggregate.map(|read| read.into_prepared().unwrap().unwrap());
            self.apply_stack_switch_ready(prepared, retained, pending.view, pending.target);
            return;
        }
        let failed = target.result.is_err()
            || retained_aggregate
                .as_ref()
                .is_some_and(|read| read.result.is_err());
        for error in [
            target.result.as_ref().err(),
            retained_aggregate
                .as_ref()
                .and_then(|read| read.result.as_ref().err()),
        ]
        .into_iter()
        .flatten()
        {
            crate::logger::log(format!("stack view prepare failed: {error}"));
        }
        self.schedule_stack_retry(
            ctx,
            StackRetrySource::SwitchReady(
                pending.view,
                pending.target,
                target.order,
                retained_aggregate.map(|read| read.order),
            ),
            failed,
            pending.retry_attempt,
        );
    }

    fn apply_stack_switch_ready(
        &mut self,
        prepared: StackPreparedItems,
        retained_aggregate: Option<StackPreparedItems>,
        view: Arc<StackView>,
        target: StackSwitchTarget,
    ) {
        let select = match target {
            StackSwitchTarget::Flat { index } => Some(index),
            StackSwitchTarget::Aggregated { select } => select,
        };
        self.swap_stack_view_items(prepared, &view.folder, select);
        match target {
            StackSwitchTarget::Flat { index } => {
                self.stack_return_state = Some(StackReturnState::Ready {
                    view,
                    prepared: retained_aggregate.expect("flat switch has accepted aggregate"),
                });
                self.stack_showing_flat = true;
                self.fs_open_intent_from_grid = true;
                self.open_fullscreen(index, crate::app::HistoryTrigger::UserChosen);
            }
            StackSwitchTarget::Aggregated { .. } => {
                self.stack_showing_flat = false;
                self.stack_return_state = None;
            }
        }
    }

    fn stack_writers_active() -> bool {
        crate::page_edit_write_epoch::PAGE_EDIT_WRITES
            .sample()
            .active_writers
            != 0
            || crate::rating_db::RATING_WRITES.sample().active_writers != 0
            || crate::tags_db::TAG_WRITES.sample().active_writers != 0
    }

    fn schedule_stack_retry(
        &mut self,
        ctx: &egui::Context,
        source: StackRetrySource,
        failed_read: bool,
        prior_attempt: u8,
    ) {
        let retry_attempt = if failed_read {
            prior_attempt.saturating_add(1)
        } else {
            0
        };
        let delay = if failed_read {
            crate::app::page_edit_snapshot::reconcile_retry_delay(retry_attempt)
        } else {
            std::time::Duration::ZERO
        };
        // Two failures at the 4 s cap (attempts 7 and 8) distinguish a transient lock from a
        // sustained outage. Release item input once, then keep the same capped retry alive.
        const STACK_RETURN_UNBLOCK_ATTEMPT: u8 = 8;
        if failed_read && retry_attempt >= STACK_RETURN_UNBLOCK_ATTEMPT {
            let returning_view = match &source {
                StackRetrySource::SwitchReady(view, StackSwitchTarget::Aggregated { .. }, ..)
                | StackRetrySource::SwitchFresh(view, StackSwitchTarget::Aggregated { .. }) => {
                    Some(Arc::clone(view))
                }
                _ => None,
            };
            if let Some(view) = returning_view
                && matches!(self.stack_return_state.as_ref(),
                    Some(StackReturnState::Refreshing { view: current }) if Arc::ptr_eq(current, &view))
            {
                self.stack_return_state = Some(StackReturnState::Recovering { view });
                self.show_feedback_toast(
                    "編集情報を更新できませんでした。自動で再試行します。".into(),
                );
            }
        }
        self.stack_script_pending = Some(StackPreparePending::Retry(StackRetryPending {
            cancel: Arc::new(AtomicBool::new(false)),
            source,
            context_id: self.virtual_list_context_id(),
            items_generation: self.items_generation,
            item_count: self.items.len(),
            sequence: self.stack_request_sequence,
            due: std::time::Instant::now() + delay,
            retry_attempt,
        }));
        if Self::stack_writers_active() {
            // Guard completion wakes the UI. One immediate repaint closes the race with a
            // writer that completes between the active sample and pending installation.
            ctx.request_repaint();
        } else if delay.is_zero() {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(delay);
        }
    }

    fn poll_stack_retry(&mut self, ctx: &egui::Context, pending: StackRetryPending) {
        if pending.cancel.load(Ordering::Relaxed)
            || self.virtual_list_context_id() != pending.context_id
            || self.items_generation != pending.items_generation
            || self.items.len() != pending.item_count
            || self.stack_request_sequence != pending.sequence
            || !self.stack_mode_requested
        {
            return;
        }
        if Self::stack_writers_active() {
            self.stack_script_pending = Some(StackPreparePending::Retry(pending));
            return;
        }
        if std::time::Instant::now() < pending.due {
            ctx.request_repaint_after(
                pending
                    .due
                    .saturating_duration_since(std::time::Instant::now()),
            );
            self.stack_script_pending = Some(StackPreparePending::Retry(pending));
            return;
        }
        match pending.source {
            StackRetrySource::GroupFresh(source, existing_keys, folder_signature)
                if self.stack_view.is_none() && !self.stack_showing_flat =>
            {
                if self.stack_group_config_current(&source) {
                    self.spawn_stack_grouping_source(
                        source,
                        existing_keys,
                        folder_signature,
                        pending.retry_attempt,
                    );
                } else {
                    self.rebase_stack_group_source(&source, existing_keys, folder_signature);
                }
            }
            StackRetrySource::GroupReady(
                source,
                view,
                order,
                rule,
                error,
                existing_keys,
                folder_signature,
            ) if self.stack_view.is_none() && !self.stack_showing_flat => {
                if self.stack_group_config_current(&source) {
                    self.spawn_stack_group_read(
                        ctx,
                        source,
                        view,
                        order,
                        rule,
                        error,
                        existing_keys,
                        folder_signature,
                        pending.retry_attempt,
                    );
                } else {
                    self.rebase_stack_group_source(&source, existing_keys, folder_signature);
                }
            }
            StackRetrySource::SwitchReady(view, target, order, retained)
                if self
                    .stack_view
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &view)) =>
            {
                self.start_stack_switch_with_order(
                    ctx,
                    view,
                    target,
                    Some((order, retained)),
                    pending.retry_attempt,
                );
            }
            StackRetrySource::SwitchFresh(view, target)
                if self
                    .stack_view
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &view)) =>
            {
                self.start_stack_switch_with_order(ctx, view, target, None, pending.retry_attempt);
            }
            _ => {}
        }
    }

    /// スタックモードのトグルが使える状況か。
    /// 通常フォルダ、またはサブ展開スナップショット表示で有効。ZIP ツリー / PDF ページ一覧 /
    /// 検索 / タグ / 閲覧履歴 / ドライブ一覧などの特殊・仮想ビューでは無効。
    pub(crate) fn stack_mode_available(&self) -> bool {
        let regular_folder = self.current_folder_last_mtime.is_some();
        let subfolder_expansion = self.items_are_subfolder_expansion_view
            && self.subfolder_expansion_snapshot.is_some()
            && self.subfolder_expansion_pending.is_none()
            && self.subfolder_expansion_install_pending.is_none();
        self.current_folder.is_some()
            && (regular_folder || subfolder_expansion)
            && self.zip_nav.is_none()
            && !self.items_are_global_search_view
            && !self.items_are_tag_view
            && !self.items_are_reading_history_view
            && !self.items_are_drive_list
            && !self.global_search.active
            && !self.favsearch.active
            && !self.tag_view.active
            // Ctrl+F (現在地フィルタ) 中はトグル不可: トグルは load_folder 経由なので
            // search_filter / search_query が消えてしまうため。
            && !self.show_search_bar
            && self.search_filter.is_none()
            && self.search_pending.is_none()
    }

    /// スタックモードが ON か。トグルボタンの選択状態表示に使う。
    pub(crate) fn stack_mode_on(&self) -> bool {
        self.stack_mode_requested
    }

    /// 集約グリッドを表示中か (= スタックモード ON かつフラット読書フルスクリーン中でない)。
    /// グリッドのセルクリック → フラットフルスクリーンへ入れる状態かの判定に使う。
    pub(crate) fn stack_mode_aggregated(&self) -> bool {
        self.stack_view.is_some() && !self.stack_showing_flat
    }

    /// 現在カーソル位置 (selected) のセルの代表画像/動画パス。スタックトグルで被写体を保つため。
    /// 通常セル (Image/Video) はそのパス、スタックセルは代表画像、コンテナは `None`。
    fn current_selected_representative_path(&self) -> Option<std::path::PathBuf> {
        let idx = self.selected?;
        match self.items.get(idx)? {
            GridItem::Image(p) | GridItem::Video(p) => Some(p.clone()),
            GridItem::Stack { representative, .. } => Some(representative.clone()),
            _ => None,
        }
    }

    /// スタックモードを切り替える。同一フォルダを再読込して集約 / 通常を作り直す
    /// (folder_changes=false なので `stack_mode_requested` は維持される)。
    pub(crate) fn toggle_stack_mode(&mut self) {
        if !self.stack_mode_available() {
            self.show_feedback_toast("スタック表示は通常フォルダまたはサブ展開で使えます".into());
            return;
        }
        if self.items_are_subfolder_expansion_view {
            self.toggle_subfolder_stack_mode();
            return;
        }
        let Some(folder) = self.current_folder.clone() else {
            return;
        };
        // トグル前のカーソル画像 (代表パス) を捕まえ、トグル後も同じ被写体に留まるようにする。
        let target = self.current_selected_representative_path();
        // 通常フォルダでの選択は名前ベースの select_after_load で復元する (ON 時の計算中の
        // 通常表示 / OFF 時の最終表示の両方で効く)。ON 時の最終的な集約ビューでは、その画像を
        // 含むスタックセルへ apply_stack_script_result が選択し直す。
        if let Some(name) = target
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
        {
            self.select_after_load = Some(name.to_string());
        }
        self.stack_mode_requested = !self.stack_mode_requested;
        self.stack_toggle_select_path = if self.stack_mode_requested {
            target
        } else {
            None
        };
        self.load_folder(folder);
        // スクリプトをワーカーで計算中 (async) のときは、ここではトーストしない。完了時に
        // poll_stack_script が採用ルール / 失敗 / 非該当のトーストを出す。
        if self.stack_mode_requested && self.stack_script_pending.is_none() {
            // スクリプトが失敗して既定ルールへフォールバックした場合は最優先で知らせる。
            if self.stack_script_error.is_some() {
                self.show_feedback_toast(
                    "スタックのスクリプトでエラー。既定ルールで表示します (詳細はヘルプ参照)"
                        .into(),
                );
            } else if self
                .stack_view
                .as_ref()
                .is_some_and(|sv| !sv.has_collapsible_stack())
            {
                self.show_feedback_toast(
                    "まとめられるスタックがありませんでした (分類ルールはヘルプ参照)".into(),
                );
            } else if let Some(rule) = self.stack_active_rule.clone() {
                // 採用された分類ルール名をトーストで知らせる (どのルールが当たったか可視化)。
                self.show_feedback_toast(format!("スタック: 「{rule}」でまとめました"));
            }
        }
    }

    /// サブ展開ビュー上でスタック表示を切り替える。
    ///
    /// ON: 現在のサブ展開一覧を親フォルダ単位で分類し、同じ合成ビューに集約セルを表示する。
    /// OFF: 保持済みスナップショットを再インストールして、サブ展開のフラット一覧へ戻す。
    fn toggle_subfolder_stack_mode(&mut self) {
        let target = self.current_selected_representative_path();
        self.cancel_stack_script_pending();
        self.stack_active_rule = None;
        self.stack_script_error = None;

        if self.stack_mode_requested {
            self.stack_mode_requested = false;
            self.stack_toggle_select_path = None;
            self.stack_view = None;
            self.stack_return_state = None;
            self.stack_showing_flat = false;
            if self.reinstall_subfolder_expansion_snapshot() {
                if let Some(target) = target
                    && let Some(idx) = self.items.iter().position(|item| match item {
                        GridItem::Image(path) | GridItem::Video(path) => {
                            crate::folder_tree::path_eq(path, &target)
                        }
                        _ => false,
                    })
                {
                    self.selected = Some(idx);
                    self.scroll_to_selected = true;
                }
            } else if let Some(root) = self.subfolder_expansion_root.clone() {
                let roots = if self.subfolder_expansion_roots.is_empty() {
                    vec![root.clone()]
                } else {
                    self.subfolder_expansion_roots.clone()
                };
                self.start_subfolder_expansion_scan_roots(root, roots);
            }
            return;
        }

        if self.subfolder_expansion_snapshot.is_none() {
            self.show_feedback_toast("サブ展開のスナップショットがありません".into());
            return;
        }

        let listing = StackListingSource::Subfolder(Arc::clone(
            &self.subfolder_expansion_snapshot.as_ref().unwrap().entries,
        ));
        self.stack_mode_requested = true;
        self.stack_toggle_select_path = target;
        self.stack_showing_flat = false;
        self.stack_view = None;
        self.stack_return_state = None;
        let sort_path = self
            .subfolder_expansion_root
            .as_deref()
            .or(self.current_folder.as_deref())
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(crate::app::subfolder_expansion_synthetic_path);
        let sort = self.book_sort_order_for_path(&sort_path);
        self.spawn_stack_script_worker(
            crate::app::subfolder_expansion_synthetic_path(),
            sort_path,
            listing,
            self.settings.stack_separator,
            sort,
            std::collections::HashSet::new(),
            None,
            self.settings.stack_script_enabled,
            true,
        );
    }

    /// 集約グリッドでメディアセル (スタック / 単独画像 / 動画) を開いたとき、フラット読書
    /// フルスクリーンへ入る。`agg_idx` は集約 `self.items` の index。コンテナ (passthrough) は
    /// `false` を返して通常ナビ (フォルダ/ZIP/PDF を開く) に委ねる。戻り値 true = ここで処理した。
    ///
    /// `from_double_click` = ダブルクリック経由か (動画の場合に 2 打目の play/pause トグルを
    /// 抑制するため)。通常の grid→fullscreen 経路と同じ開幕ガードをここで張る。
    pub(crate) fn stack_try_open_from_grid(
        &mut self,
        ctx: &egui::Context,
        agg_idx: usize,
        from_double_click: bool,
    ) -> bool {
        if !self.grid_item_input_allowed() {
            return true;
        }
        if !self.stack_mode_aggregated() {
            return false;
        }
        let flat_idx = self
            .stack_view
            .as_ref()
            .and_then(|sv| sv.flat_index_for_aggregated(agg_idx));
        let Some(flat_idx) = flat_idx else {
            // passthrough コンテナ → フルスクリーンでなく通常ナビへ。
            return false;
        };
        #[cfg(not(windows))]
        let _ = ctx;
        // items をフラット配列へ差し替える前に、集約セルの path で same-media 前面化を
        // 判定する。別 media/still のときだけ現 active context を先に park する。
        // (review-v2.3.0 追補4: stack flat grid open)
        #[cfg(windows)]
        if !self.prepare_detached_context_for_grid_open(ctx, agg_idx) {
            return true;
        }
        // 開幕ガード (通常の grid open 経路と同じ):
        // - Enter で開いた同フレームに fullscreen 側が同じ Enter を拾って即 close するのを防ぐ
        //   (Enter が押下されていなければ fullscreen 側初フレームで自動リセットされるので、
        //    click/gamepad 経由で立てても無害)。
        self.fs_suppress_enter_close_until_release = true;
        // - ダブルクリックで動画を開いたとき、2 打目クリックが fullscreen の動画 play/pause を
        //   トグルしないよう抑制する (静止画は open_fullscreen の focus-regain グレースで足りる)。
        if from_double_click && matches!(self.items.get(agg_idx), Some(GridItem::Video(_))) {
            self.fs_primary_suppression.arm_pointer_stream();
        }
        self.stack_enter_flat_fullscreen(ctx, flat_idx);
        true
    }

    /// フラット読書ビュー (全画像を展開した並び) へ `self.items` を差し替え、`flat_idx` を
    /// フルスクリーンで開く。
    ///
    /// Worker が prepared items と exact-key 投影を返した後、軽量ビュー切替の
    /// 後始末 (idx 状態 + キュー破棄 / visible_indices 再構築) を必ず行う。これを怠ると旧 (集約) ビューの stale な
    /// `visible_indices` が範囲外参照 panic を起こす (Codex P1)。
    fn stack_enter_flat_fullscreen(&mut self, ctx: &egui::Context, flat_idx: usize) {
        let Some(view) = self.stack_view.as_ref().map(Arc::clone) else {
            return;
        };
        self.start_stack_switch(ctx, view, StackSwitchTarget::Flat { index: flat_idx });
    }

    /// フルスクリーン中の `Shift+↓↑`: 次/前のスタックの先頭画像へジャンプする。
    /// フラット読書ビューでないときは `false` (= 呼び出し側が通常のページ送りに委ねる)。
    /// 端では stack ジャンプ可能位置が無いので `true` (消費) のまま no-op にする。
    pub(crate) fn stack_jump(&mut self, ctx: &egui::Context, forward: bool) -> bool {
        if !self.stack_showing_flat {
            return false;
        }
        let Some(cur) = self.fullscreen_idx else {
            return false;
        };
        let target = self
            .stack_view
            .as_ref()
            .and_then(|sv| sv.stack_jump_target(cur, forward));
        if let Some(t) = target {
            self.open_fullscreen_from_fs_navigation(ctx, t, crate::app::HistoryTrigger::UserChosen);
        }
        true
    }

    /// フラットフルスクリーンが閉じたら集約グリッドへ戻す (毎フレーム reconcile、
    /// `render_grid` の直前で呼ぶ)。スタックモードが解除済み (フォルダナビ等) なら何もしない。
    pub(crate) fn stack_reconcile_after_fullscreen_close(&mut self, ctx: &egui::Context) {
        if self.fullscreen_idx.is_some() {
            return;
        }
        let state = self.stack_return_state.take();
        if let Some(StackReturnState::Ready { view, prepared }) = state {
            if !self.stack_showing_flat
                || !self
                    .stack_view
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &view))
            {
                return;
            }
            let select = self
                .selected
                .and_then(|flat| view.group_of_flat_index(flat))
                .map(|group| view.aggregated_index_of_group(group));
            let current = Self::stack_prepared_stamps_current(&prepared);
            self.swap_stack_view_items(prepared, &view.folder, select);
            self.stack_showing_flat = false;
            if !current {
                self.stack_return_state = Some(StackReturnState::Refreshing {
                    view: Arc::clone(&view),
                });
                self.start_stack_switch(ctx, view, StackSwitchTarget::Aggregated { select });
            }
            return;
        }
        let (view, recovering) = match state {
            Some(StackReturnState::Refreshing { view }) => (view, false),
            Some(StackReturnState::Recovering { view }) => (view, true),
            _ => return,
        };
        if !self
            .stack_view
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &view))
        {
            return;
        }
        self.stack_return_state = Some(if recovering {
            StackReturnState::Recovering {
                view: Arc::clone(&view),
            }
        } else {
            StackReturnState::Refreshing {
                view: Arc::clone(&view),
            }
        });
        if self.stack_script_pending.is_none() {
            self.start_stack_switch(
                ctx,
                view,
                StackSwitchTarget::Aggregated {
                    select: self.selected,
                },
            );
        }
    }

    /// The mounted bundle owns whether its retained aggregate may receive item input.
    /// All grid input owners (key, pointer, touch, ring, gamepad, and menu) use this gate.
    pub(crate) fn grid_item_input_allowed(&self) -> bool {
        !matches!(self.stack_return_state.as_ref(),
            Some(StackReturnState::Refreshing { view })
                if self.stack_view.as_ref().is_some_and(|current| Arc::ptr_eq(current, view)))
    }

    /// 集約/フラット間の in-memory ビュー切替の共通後始末。`select` を選択し scroll する。
    fn swap_stack_view_items(
        &mut self,
        prepared: StackPreparedItems,
        folder: &std::path::Path,
        select: Option<usize>,
    ) {
        // 旧ビューの in-flight 検索 / 詳細メタ pending を停止 (idx が付け替わる)。
        if let Some(pending) = self.search_pending.take() {
            pending.cancel();
        }
        if let Some(pending) = self.metadata_pending.take() {
            pending.cancel();
        }
        let StackPreparedItems {
            items,
            metas,
            edits,
            rating_cache,
            tags_cache,
            ..
        } = prepared;
        self.install_prepared_aggregate_items(items, metas);
        if crate::folder_tree::path_eq(folder, &crate::app::subfolder_expansion_synthetic_path()) {
            self.restore_subfolder_expansion_view_state_after_items_install();
        }
        self.invalidate_idx_state_and_queues();
        self.current_folder_rating_cache = None;
        // The worker's exact page-key snapshot owns this new order, including real members of
        // a synthetic sub-folder expansion. Never hydrate by the synthetic folder prefix.
        self.install_prepared_page_edits(edits);
        self.local_adjust_generation.clear();
        self.local_adjust_cache.clear();
        self.metadata_cache.clear();
        self.exif_cache.clear();
        self.xmp_cache.clear();
        self.clear_tags_cache();
        self.search_filter = None;
        self.search_query.clear();
        self.selected = select;
        self.scroll_offset_y = 0.0;
        self.scroll_to_selected = select.is_some();
        self.scroll_hint
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.rating_cache = rating_cache;
        // The worker's rating stamp was checked at acceptance, so this cache already includes
        // every committed session write. Mark the context current without rescanning all items.
        self.rating_session_write_seen_generation = self.rating_session_write_generation;
        self.mark_color_filter_scope_dirty();
        // ★ visible_indices 再構築。stale index による範囲外参照 panic を防ぐ (Codex P1)。
        self.rebuild_visible_indices();
        self.replace_tags_cache(tags_cache);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack_page_edit_fixture(app: &mut crate::app::App, folder: PathBuf, pages: &[PathBuf]) {
        let media = pages
            .iter()
            .map(|path| StackMember {
                path: path.clone(),
                mtime: 0,
                size: Some(1),
                is_video: false,
            })
            .collect();
        let view = StackView::build(
            folder.clone(),
            Vec::new(),
            Vec::new(),
            media,
            '_',
            SortOrder::FileName,
        );
        let (items, metas) = view.materialize_aggregated();
        app.current_folder = Some(folder);
        app.items = items;
        app.image_metas = metas;
        app.thumbnails = vec![crate::grid_item::ThumbnailState::Pending; app.items.len()];
        app.stack_view = Some(Arc::new(view));
        app.stack_mode_requested = true;
        app.rebuild_visible_indices();
    }

    #[test]
    fn a2_stack_subfolder_flat_uses_exact_page_key_for_fullscreen() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let mut app = crate::app::setup_app_for_test();
        let dir = app.tmp.path().join("nested");
        std::fs::create_dir(&dir).unwrap();
        let first = dir.join("post_0.png");
        let second = dir.join("post_1.png");
        for path in [&first, &second] {
            image::RgbaImage::new(1, 1).save(path).unwrap();
        }
        let key = crate::adjustment_db::normalize_path(&second);
        app.mask_db
            .as_ref()
            .unwrap()
            .set(&key, &[true], &[], 1, 1)
            .unwrap();
        stack_page_edit_fixture(
            &mut app,
            crate::app::subfolder_expansion_synthetic_path(),
            &[first, second],
        );
        let ctx = egui::Context::default();
        app.stack_enter_flat_fullscreen(&ctx, 0);
        for _ in 0..200 {
            app.poll_stack_script(&ctx);
            if app.stack_showing_flat {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(app.stack_showing_flat);
        assert!(
            app.mask_pages.contains(&1),
            "flat subfolder member must select its saved mask"
        );
    }

    #[test]
    fn a2_stack_subfolder_aggregate_flat_round_trip_preserves_mask() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let mut app = crate::app::setup_app_for_test();
        let dir = app.tmp.path().join("round-trip");
        std::fs::create_dir(&dir).unwrap();
        let first = dir.join("post_0.png");
        let second = dir.join("post_1.png");
        for path in [&first, &second] {
            image::RgbaImage::new(1, 1).save(path).unwrap();
        }
        let key = crate::adjustment_db::normalize_path(&second);
        app.mask_db
            .as_ref()
            .unwrap()
            .set(&key, &[true], &[], 1, 1)
            .unwrap();
        stack_page_edit_fixture(
            &mut app,
            crate::app::subfolder_expansion_synthetic_path(),
            &[first, second],
        );
        let ctx = egui::Context::default();
        app.stack_enter_flat_fullscreen(&ctx, 0);
        for _ in 0..200 {
            app.poll_stack_script(&ctx);
            if app.stack_showing_flat {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.close_fullscreen();
        app.stack_reconcile_after_fullscreen_close(&ctx);
        assert!(
            !app.stack_showing_flat,
            "closing restores the aggregate immediately"
        );
        assert!(matches!(app.items.first(), Some(GridItem::Stack { .. })));
        assert!(
            app.stack_script_pending.is_none(),
            "unchanged stamps need no worker on close"
        );
        app.stack_enter_flat_fullscreen(&ctx, 0);
        for _ in 0..200 {
            app.poll_stack_script(&ctx);
            if app.stack_showing_flat {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            app.mask_pages.contains(&1),
            "reopened flat member must keep its saved mask"
        );
    }

    #[test]
    fn a2_stack_stale_return_shows_aggregate_and_blocks_grid_until_refresh() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let mut app = crate::app::setup_app_for_test();
        let dir = app.tmp.path().join("stale-return");
        std::fs::create_dir(&dir).unwrap();
        let pages = [
            dir.join("post_0.png"),
            dir.join("post_1.png"),
            dir.join("solo_0.png"),
        ];
        for page in &pages {
            image::RgbaImage::new(1, 1).save(page).unwrap();
        }
        stack_page_edit_fixture(&mut app, dir, &pages);
        let ctx = egui::Context::default();
        app.stack_enter_flat_fullscreen(&ctx, 0);
        for _ in 0..200 {
            app.poll_stack_script(&ctx);
            if app.stack_showing_flat {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(app.stack_showing_flat);
        let key = crate::adjustment_db::normalize_path(&pages[2]);
        app.mask_db
            .as_ref()
            .unwrap()
            .set(&key, &[true], &[], 1, 1)
            .unwrap();
        app.close_fullscreen();
        app.stack_reconcile_after_fullscreen_close(&ctx);
        assert!(!app.stack_showing_flat);
        assert!(!app.grid_item_input_allowed());
        assert!(matches!(app.items.first(), Some(GridItem::Stack { .. })));
        assert!(app.stack_try_open_from_grid(&ctx, 0, false));
        assert!(app.fullscreen_idx.is_none());
        for _ in 0..200 {
            app.poll_stack_script(&ctx);
            if app.grid_item_input_allowed() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(app.grid_item_input_allowed());
        let solo = app
            .items
            .iter()
            .position(|item| matches!(item, GridItem::Image(path) if path == &pages[2]))
            .unwrap();
        assert!(app.mask_pages.contains(&solo));
    }

    #[test]
    fn a2_stack_normal_folder_flat_mask_control() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let mut app = crate::app::setup_app_for_test();
        let dir = app.tmp.path().join("normal");
        std::fs::create_dir(&dir).unwrap();
        let first = dir.join("post_0.png");
        let second = dir.join("post_1.png");
        for path in [&first, &second] {
            image::RgbaImage::new(1, 1).save(path).unwrap();
        }
        let key = crate::adjustment_db::normalize_path(&second);
        app.mask_db
            .as_ref()
            .unwrap()
            .set(&key, &[true], &[], 1, 1)
            .unwrap();
        stack_page_edit_fixture(&mut app, dir, &[first, second]);
        let ctx = egui::Context::default();
        app.stack_enter_flat_fullscreen(&ctx, 0);
        for _ in 0..200 {
            app.poll_stack_script(&ctx);
            if app.stack_showing_flat {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            app.mask_pages.contains(&1),
            "normal folder control retains its mask"
        );
    }

    fn pending_group_for_test(
        app: &mut crate::app::App,
        view: Arc<StackView>,
        ctx: &egui::Context,
    ) -> StackScriptPending {
        let (items, metas) = view.materialize_aggregated();
        let read = read_stack_candidate(
            StackCandidateOrder::new(items, metas),
            crate::app::page_edit_snapshot::PageEditAvailability::for_app(app),
            app.rating_db.is_some(),
            app.tags_db.is_some(),
            &AtomicBool::new(false),
        );
        match &read.result {
            Ok(Some(_)) => {}
            Ok(None) => panic!("the isolated stack candidate unexpectedly became stale"),
            Err(error) => panic!("stack candidate read failed: {error}"),
        }
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Ok(Some(StackGroupReady {
            view: Arc::clone(&view),
            read,
            rule: None,
            error: None,
        })))
        .unwrap();
        let folder = view.folder.clone();
        app.items = vec![GridItem::Image(folder.join("post_0.png"))];
        app.image_metas = vec![None];
        app.stack_view = None;
        app.stack_mode_requested = true;
        let source = Arc::new(StackGroupingSource {
            sort_path: folder.clone(),
            folder,
            items: Vec::new(),
            image_metas: Vec::new(),
            listing: StackListingSource::Normal(Arc::new(Vec::new())),
            separator: '_',
            sort: SortOrder::FileName,
            script_enabled: false,
            group_per_parent: false,
            display_order: app.settings.grid_display_order.clone(),
            available_edits: crate::app::page_edit_snapshot::PageEditAvailability::for_app(app),
            rating_available: app.rating_db.is_some(),
            tags_available: app.tags_db.is_some(),
            group_runs: None,
            read_failures: None,
        });
        let _ = ctx;
        StackScriptPending {
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
            source,
            context_id: app.virtual_list_context_id(),
            items_generation: app.items_generation,
            item_count: app.items.len(),
            sequence: app.stack_request_sequence,
            existing_keys: Default::default(),
            folder_signature: None,
            retry_attempt: 0,
        }
    }

    #[test]
    fn a2_stack_stale_grouping_after_write_rebases() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let folder = app.tmp.path().join("stale-group-write");
        std::fs::create_dir(&folder).unwrap();
        let page = folder.join("post_0.png");
        image::RgbaImage::new(1, 1).save(&page).unwrap();
        stack_page_edit_fixture(&mut app, folder, &[page.clone()]);
        let view = Arc::clone(app.stack_view.as_ref().unwrap());
        let pending = pending_group_for_test(&mut app, view, &ctx);
        app.mask_db
            .as_ref()
            .unwrap()
            .set(
                &crate::adjustment_db::normalize_path(&page),
                &[true],
                &[],
                1,
                1,
            )
            .unwrap();
        app.stack_script_pending = Some(StackPreparePending::Grouping(pending));
        app.poll_stack_script(&ctx);
        assert!(matches!(
            app.stack_script_pending,
            Some(StackPreparePending::Retry(_))
        ));
        assert!(
            app.stack_view.is_none(),
            "stale worker order must not be installed"
        );
    }

    #[test]
    fn a2_stack_stale_grouping_after_new_request_is_rejected() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let folder = app.tmp.path().join("stale-group-sequence");
        std::fs::create_dir(&folder).unwrap();
        let page = folder.join("post_0.png");
        image::RgbaImage::new(1, 1).save(&page).unwrap();
        stack_page_edit_fixture(&mut app, folder, &[page]);
        let view = Arc::clone(app.stack_view.as_ref().unwrap());
        let pending = pending_group_for_test(&mut app, view, &ctx);
        app.stack_request_sequence = app.stack_request_sequence.wrapping_add(1);
        app.stack_script_pending = Some(StackPreparePending::Grouping(pending));
        app.poll_stack_script(&ctx);
        assert!(app.stack_script_pending.is_none());
        assert!(
            app.stack_view.is_none(),
            "older switch must not install its grouping"
        );
    }

    #[test]
    fn a2_stack_grouping_rebases_when_each_captured_setting_changes() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        for changed in 0..4 {
            let mut app = crate::app::setup_app_for_test();
            let ctx = egui::Context::default();
            let folder = app.tmp.path().join(format!("config-{changed}"));
            std::fs::create_dir(&folder).unwrap();
            let page = folder.join("post_0.png");
            image::RgbaImage::new(1, 1).save(&page).unwrap();
            stack_page_edit_fixture(&mut app, folder, &[page]);
            let view = Arc::clone(app.stack_view.as_ref().unwrap());
            let pending = pending_group_for_test(&mut app, view, &ctx);
            match changed {
                0 => app.settings.stack_separator = '-',
                1 => app.settings.stack_script_enabled = true,
                2 => app.settings.sort_order = SortOrder::FileNameDesc,
                3 => app
                    .settings
                    .grid_display_order
                    .assign(crate::settings::GridItemDisplayKind::Image, 2),
                _ => unreachable!(),
            }
            app.stack_script_pending = Some(StackPreparePending::Grouping(pending));
            app.poll_stack_script(&ctx);
            let Some(StackPreparePending::Extracting(next)) = app.stack_script_pending.as_ref()
            else {
                panic!("changed setting {changed} must rebase, not accept old grouping");
            };
            assert!(app.stack_view.is_none());
            assert!(app.stack_group_config_current(&next.source));
            app.cancel_stack_script_pending();
        }
    }

    #[test]
    fn a2_stack_large_extract_is_sliced_before_worker_grouping() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let folder = app.tmp.path().join("large-extract");
        app.current_folder = Some(folder.clone());
        app.stack_mode_requested = true;
        app.items = (0..100_000)
            .map(|index| GridItem::Image(folder.join(format!("post_{index:06}.png"))))
            .collect();
        app.image_metas = vec![None; app.items.len()];
        let metas = vec![ListingSortMetadata::new(0, None); app.items.len()];
        app.spawn_stack_script_worker(
            folder.clone(),
            folder,
            StackListingSource::Normal(Arc::new(metas)),
            '_',
            SortOrder::FileName,
            Default::default(),
            None,
            false,
            false,
        );
        let started = std::time::Instant::now();
        app.poll_stack_script(&ctx);
        let elapsed = started.elapsed();
        let copied = match app.stack_script_pending.as_ref() {
            Some(StackPreparePending::Extracting(pending)) => pending.source.items.len(),
            _ => panic!("large source must still be in sliced extraction"),
        };
        assert_eq!(copied, 2048);
        assert!(
            elapsed < std::time::Duration::from_millis(100),
            "single UI slice: {elapsed:?}"
        );
        app.cancel_stack_script_pending();
    }

    #[test]
    fn a2_stack_failed_reads_reuse_grouping_with_bounded_wakes() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let folder = app.tmp.path().join("retry-grouping");
        app.current_folder = Some(folder.clone());
        app.stack_mode_requested = true;
        app.items = vec![
            GridItem::Image(folder.join("post_0.png")),
            GridItem::Image(folder.join("post_1.png")),
        ];
        app.image_metas = vec![None; 2];
        let separator = app.settings.stack_separator;
        let sort = app.settings.sort_order;
        let script_enabled = app.settings.stack_script_enabled;
        app.spawn_stack_script_worker(
            folder.clone(),
            folder,
            StackListingSource::Normal(Arc::new(vec![ListingSortMetadata::new(0, None); 2])),
            separator,
            sort,
            Default::default(),
            None,
            script_enabled,
            false,
        );
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let failures = Arc::new(std::sync::atomic::AtomicUsize::new(3));
        let Some(StackPreparePending::Extracting(pending)) = app.stack_script_pending.as_mut()
        else {
            panic!("extracting")
        };
        pending.source.group_runs = Some(Arc::clone(&runs));
        pending.source.read_failures = Some(Arc::clone(&failures));
        let mut observed_delays = Vec::new();
        for expected_failure in 1..=3 {
            for _ in 0..200 {
                app.poll_stack_script(&ctx);
                if matches!(
                    app.stack_script_pending,
                    Some(StackPreparePending::Retry(_))
                ) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let Some(StackPreparePending::Retry(pending)) = app.stack_script_pending.as_mut()
            else {
                panic!("retry after failure")
            };
            assert_eq!(pending.retry_attempt, expected_failure);
            let remaining = pending
                .due
                .saturating_duration_since(std::time::Instant::now());
            observed_delays.push(remaining);
            assert!(remaining >= std::time::Duration::from_millis(50));
            assert!(remaining <= std::time::Duration::from_secs(4));
            assert_eq!(
                runs.load(Ordering::Relaxed),
                1,
                "read failure must not rerun grouping"
            );
            if expected_failure == 1 {
                let guard = crate::page_edit_write_epoch::PAGE_EDIT_WRITES.begin();
                pending.due = std::time::Instant::now();
                app.poll_stack_script(&ctx);
                assert!(matches!(
                    app.stack_script_pending,
                    Some(StackPreparePending::Retry(_))
                ));
                assert_eq!(runs.load(Ordering::Relaxed), 1);
                drop(guard);
            } else {
                pending.due = std::time::Instant::now();
            }
        }
        assert!(observed_delays[1] > observed_delays[0]);
        assert!(observed_delays[2] > observed_delays[1]);
        for _ in 0..200 {
            app.poll_stack_script(&ctx);
            if app.stack_view.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            app.stack_view.is_some(),
            "a later successful read converges without another write"
        );
        assert_eq!(runs.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a2_stack_stale_return_unblocks_after_sustained_failure_and_later_recovers() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let folder = app.tmp.path().join("stale-return-failure");
        std::fs::create_dir(&folder).unwrap();
        let page = folder.join("solo_0.png");
        image::RgbaImage::new(1, 1).save(&page).unwrap();
        app.mask_db
            .as_ref()
            .unwrap()
            .set(
                &crate::adjustment_db::normalize_path(&page),
                &[true],
                &[],
                1,
                1,
            )
            .unwrap();
        stack_page_edit_fixture(&mut app, folder.clone(), &[page]);
        let view = Arc::clone(app.stack_view.as_ref().unwrap());
        let (items, metas) = view.materialize_aggregated();
        let accepted = read_stack_candidate(
            StackCandidateOrder::new(items, metas),
            crate::app::page_edit_snapshot::PageEditAvailability::for_app(&app),
            app.rating_db.is_some(),
            app.tags_db.is_some(),
            &AtomicBool::new(false),
        )
        .into_prepared()
        .unwrap()
        .unwrap();
        app.swap_stack_view_items(accepted, &folder, Some(0));
        assert!(
            app.mask_pages.contains(&0),
            "accepted edit is retained during failure"
        );
        app.stack_return_state = Some(StackReturnState::Refreshing {
            view: Arc::clone(&view),
        });
        let (items, metas) = view.materialize_aggregated();
        let mut order = StackCandidateOrder::new(items, metas);
        let failures = Arc::new(std::sync::atomic::AtomicUsize::new(9));
        order.read_failures = Some(Arc::clone(&failures));
        app.start_stack_switch_with_order(
            &ctx,
            view,
            StackSwitchTarget::Aggregated { select: Some(0) },
            Some((order, None)),
            0,
        );
        for attempt in 1..=9 {
            for _ in 0..200 {
                app.poll_stack_script(&ctx);
                if matches!(
                    app.stack_script_pending,
                    Some(StackPreparePending::Retry(_))
                ) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let Some(StackPreparePending::Retry(retry)) = app.stack_script_pending.as_ref() else {
                panic!("failure {attempt} must retain the background retry");
            };
            assert_eq!(retry.retry_attempt, attempt);
            let delay = retry
                .due
                .saturating_duration_since(std::time::Instant::now());
            assert!(delay <= std::time::Duration::from_secs(4));
            if attempt >= 7 {
                assert!(delay >= std::time::Duration::from_millis(3_900));
            }
            assert_eq!(!app.grid_item_input_allowed(), attempt < 8);
            assert!(app.mask_pages.contains(&0));
            if attempt == 8 {
                assert!(matches!(
                    app.stack_return_state,
                    Some(StackReturnState::Recovering { .. })
                ));
                assert_eq!(
                    app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str()),
                    Some("編集情報を更新できませんでした。自動で再試行します。")
                );
            }
            if let Some(StackPreparePending::Retry(retry)) = app.stack_script_pending.as_mut() {
                retry.due = std::time::Instant::now();
            }
        }
        assert_eq!(failures.load(Ordering::Relaxed), 0);
        for _ in 0..200 {
            app.poll_stack_script(&ctx);
            if app.stack_return_state.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            app.stack_return_state.is_none(),
            "successful reread converges without a write"
        );
        assert!(app.stack_script_pending.is_none());
        assert!(app.mask_pages.contains(&0));
    }

    fn with_foreign_epoch_writers(body: impl FnOnce()) {
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let writer = std::thread::spawn(move || {
            let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
            let temp = tempfile::tempdir().unwrap();
            let _page = crate::page_edit_write_epoch::PAGE_EDIT_WRITES.begin();
            let _rating = crate::rating_db::RATING_WRITES.begin();
            let _tag = crate::tags_db::TAG_WRITES.begin();
            crate::mask_db::MaskDb::open_at(&temp.path().join("mask.db"))
                .unwrap()
                .set("foreign-page", &[true], &[], 1, 1)
                .unwrap();
            crate::rating_db::RatingDb::open_at(temp.path().join("rating.db"))
                .unwrap()
                .set("foreign-rating", 1)
                .unwrap();
            crate::tags_db::TagsDb::open_at(&temp.path().join("tags.db"))
                .unwrap()
                .set_item_tags("foreign-tag", ["writer"], "test")
                .unwrap();
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        entered_rx.recv().unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
        release_tx.send(()).unwrap();
        writer.join().unwrap();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn audit_a2_stack_subfolder_round_trip_with_foreign_edit_rating_tag_writes() {
        with_foreign_epoch_writers(a2_stack_subfolder_aggregate_flat_round_trip_preserves_mask);
    }

    #[test]
    fn audit_a2_stack_stale_return_with_foreign_edit_rating_tag_writes() {
        with_foreign_epoch_writers(
            a2_stack_stale_return_unblocks_after_sustained_failure_and_later_recovers,
        );
    }

    #[test]
    fn audit_a2_stack_grouping_rebase_with_foreign_edit_rating_tag_writes() {
        with_foreign_epoch_writers(a2_stack_grouping_rebases_when_each_captured_setting_changes);
    }

    fn image(path: &str) -> StackMember {
        StackMember {
            path: PathBuf::from(path),
            mtime: 0,
            size: Some(0),
            is_video: false,
        }
    }

    #[test]
    #[should_panic]
    fn extract_stack_parts_rejects_misaligned_sort_metadata() {
        let items = vec![GridItem::Image(PathBuf::from(r"C:\root\a.jpg"))];
        let image_metas = vec![Some((1, 1))];
        let listing_sort_metas = Vec::new();
        let _ = extract_stack_parts(&items, &image_metas, &listing_sort_metas);
    }

    #[test]
    fn script_grouping_scopes_keys_by_parent_for_subfolder_stack() {
        let images = vec![
            image(r"C:\root\a\scan_001.jpg"),
            image(r"C:\root\a\scan_002.jpg"),
            image(r"C:\root\a\other_001.jpg"),
            image(r"C:\root\a\other_002.jpg"),
            image(r"C:\root\b\scan_001.jpg"),
            image(r"C:\root\b\scan_002.jpg"),
            image(r"C:\root\b\other_001.jpg"),
            image(r"C:\root\b\other_002.jpg"),
        ];
        let (keys, _rule) = stack_script_keys_for_images(
            &images,
            crate::filename_stack_script::DEFAULT_SCRIPT,
            Arc::new(AtomicBool::new(false)),
            true,
        )
        .expect("default script groups per parent");
        assert_eq!(keys.len(), images.len());
        assert_eq!(keys[0], keys[1]);
        assert_eq!(keys[2], keys[3]);
        assert_eq!(keys[4], keys[5]);
        assert_eq!(keys[6], keys[7]);
        assert_ne!(keys[0], keys[2]);
        assert_ne!(keys[0], keys[4]);
        assert_ne!(keys[2], keys[6]);
    }
}
