//! 複数ページに対する編集 bundle の貼り付け / リセット。
//!
//! 対象解決と確認は UI thread 上の在メモリ状態だけで行う。画像寸法の probe、未選択の
//! 編集種類を保持するための DB 読み込み、6 DB の atomic apply は1本の worker が順番に
//! 処理する。回転だけは `PageEditBundle` の外なので、同じ pending state を使いながら
//! UI thread で1件ずつ `App::set_image_rotation` へ渡す。

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};

use eframe::egui;

use crate::app::App;
use crate::edit_bundle::{
    EditBundleDbPaths, EditMaskSnapshot, PageEditBundle, PreparedPageEditBundle,
};
use crate::grid_item::GridItem;
use crate::rotation_db::Rotation;

#[derive(Clone, Default)]
enum KeptRuntimeValue<T> {
    #[default]
    ReadDatabase,
    Absent,
    Present(T),
}

impl<T: Clone> KeptRuntimeValue<T> {
    fn from_memory_state(memory: Option<T>, present: bool) -> Self {
        match memory {
            Some(value) => Self::Present(value),
            None if !present => Self::Absent,
            None => Self::ReadDatabase,
        }
    }

    fn resolve(
        &self,
        read_database: impl FnOnce() -> Result<Option<T>, String>,
    ) -> Result<Option<T>, String> {
        match self {
            Self::ReadDatabase => read_database(),
            Self::Absent => Ok(None),
            Self::Present(value) => Ok(Some(value.clone())),
        }
    }
}

#[derive(Clone, Default)]
struct KeptRuntimeOverrides {
    adjustment: KeptRuntimeValue<crate::adjustment::AdjustParams>,
    local_adjust: KeptRuntimeValue<local_adjust_core::LocalAdjustmentLayers>,
    export_crop: KeptRuntimeValue<crate::export_crop::CropSettings>,
    comic: KeptRuntimeValue<Vec<comic_core::AnnotationObject>>,
}

#[derive(Clone)]
struct BulkPageEditTarget {
    idx: usize,
    page_key: String,
    display_label: String,
    item: GridItem,
    sidecar_coords: Option<(PathBuf, String)>,
    known_size: Option<[usize; 2]>,
    /// 部分リセットで残す種類について、worker開始前のUI memoryを固定したsnapshot。
    kept_runtime_overrides: KeptRuntimeOverrides,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ResetKinds {
    adjustment: bool,
    mask: bool,
    conceal: bool,
    local_adjust: bool,
    comic: bool,
    export_crop: bool,
    rotation: bool,
}

impl ResetKinds {
    const fn all() -> Self {
        Self {
            adjustment: true,
            mask: true,
            conceal: true,
            local_adjust: true,
            comic: true,
            export_crop: true,
            rotation: true,
        }
    }

    const fn any(self) -> bool {
        self.needs_bundle_worker() || self.rotation
    }

    const fn needs_bundle_worker(self) -> bool {
        self.adjustment
            || self.mask
            || self.conceal
            || self.local_adjust
            || self.comic
            || self.export_crop
    }

    const fn rotation_only(self) -> bool {
        self.rotation && !self.needs_bundle_worker()
    }

    fn uncheck_empty(&mut self, counts: ResetKindCounts) {
        self.adjustment &= counts.adjustment > 0;
        self.mask &= counts.mask > 0;
        self.conceal &= counts.conceal > 0;
        self.local_adjust &= counts.local_adjust > 0;
        self.comic &= counts.comic > 0;
        self.export_crop &= counts.export_crop > 0;
        self.rotation &= counts.rotation > 0;
    }
}

#[derive(Clone)]
enum BulkPageEditOp {
    Paste {
        bundle: PageEditBundle,
        source_label: String,
    },
    Reset {
        kinds: ResetKinds,
    },
}

impl BulkPageEditOp {
    fn needs_bundle_worker(&self) -> bool {
        match self {
            Self::Paste { .. } => true,
            Self::Reset { kinds } => kinds.needs_bundle_worker(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct BulkTargetSelection {
    indices: Vec<usize>,
    dropped_non_targets: usize,
}

/// checked が空でない限り cursor へ戻らない。HashSet の反復順には依存せず、一覧順にする。
fn select_bulk_target_indices(
    items: &[GridItem],
    checked: &HashSet<usize>,
    cursor: Option<usize>,
) -> BulkTargetSelection {
    let mut candidates = if checked.is_empty() {
        cursor.into_iter().collect::<Vec<_>>()
    } else {
        checked.iter().copied().collect::<Vec<_>>()
    };
    candidates.sort_unstable();
    candidates.dedup();

    let mut indices = Vec::with_capacity(candidates.len());
    let mut dropped_non_targets = 0;
    for idx in candidates {
        match items.get(idx) {
            Some(item) if item.has_page_data() => indices.push(idx),
            Some(_) => dropped_non_targets += 1,
            // checked/cursor は現在の items に属するという App の不変条件。外れた idx は
            // 非対象種別とは数えず、呼び出し側で空結果として明示通知する。
            None => {}
        }
    }
    BulkTargetSelection {
        indices,
        dropped_non_targets,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TargetEditPresence {
    adjustment: bool,
    mask: bool,
    conceal: bool,
    local_adjust: bool,
    comic: bool,
    export_crop: bool,
    rotation: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ResetKindCounts {
    adjustment: usize,
    mask: usize,
    conceal: usize,
    local_adjust: usize,
    comic: usize,
    export_crop: usize,
    rotation: usize,
}

fn count_reset_kinds(presences: &[TargetEditPresence]) -> ResetKindCounts {
    presences
        .iter()
        .fold(ResetKindCounts::default(), |mut counts, presence| {
            counts.adjustment += usize::from(presence.adjustment);
            counts.mask += usize::from(presence.mask);
            counts.conceal += usize::from(presence.conceal);
            counts.local_adjust += usize::from(presence.local_adjust);
            counts.comic += usize::from(presence.comic);
            counts.export_crop += usize::from(presence.export_crop);
            counts.rotation += usize::from(presence.rotation);
            counts
        })
}

/// 選択された6種類だけを空にする。回転は bundle 外なので、この純関数では変更しない。
fn apply_reset_kinds(mut current: PageEditBundle, kinds: ResetKinds) -> PageEditBundle {
    if kinds.adjustment {
        current.adjust = None;
    }
    if kinds.mask {
        current.mask = None;
    }
    if kinds.conceal {
        current.conceal = None;
    }
    if kinds.local_adjust {
        current.local_adjust_layers = None;
    }
    if kinds.comic {
        current.comic = None;
    }
    if kinds.export_crop {
        current.export_crop = None;
    }
    current
}

struct BulkPasteConfirm {
    /// 操作を出した viewer context。確認ダイアログはどちらの viewport にも出るので、
    /// **押した瞬間ではなく要求した瞬間**の所有者を持ち回す。
    owner_context_id: crate::app::ViewerContextId,
    targets: Vec<BulkPageEditTarget>,
    dropped_non_targets: usize,
    overwrite_count: usize,
    op: BulkPageEditOp,
}

struct BulkResetConfirm {
    owner_context_id: crate::app::ViewerContextId,
    targets: Vec<BulkPageEditTarget>,
    dropped_non_targets: usize,
    counts: ResetKindCounts,
    kinds: ResetKinds,
}

#[derive(Clone, Copy)]
enum BulkRunEffect {
    Paste,
    Reset { kinds: ResetKinds },
}

enum BulkRunDriver {
    Worker {
        rx: Receiver<BulkWorkerEvent>,
        thread: Option<std::thread::JoinHandle<()>>,
    },
    RotationOnly {
        remaining: VecDeque<BulkPageEditTarget>,
    },
}

struct BulkPageEditRun {
    /// 操作を出した viewer context。**結果はここへ戻す。**
    ///
    /// pending は App-global で、この dialog は main と fullscreen の両方から poll される。
    /// 先に drain した側の bundle へ結果を書いていたので、別ウィンドウを開いたまま一覧側で
    /// 一括貼付 / 解除をすると、一覧側のキャッシュ・保持設定・undo が更新されないまま DB
    /// だけが変わっていた (v3.5.0 レビュー F08)。
    owner_context_id: crate::app::ViewerContextId,
    driver: BulkRunDriver,
    effect: BulkRunEffect,
    source_label: Option<String>,
    cancel: Arc<AtomicBool>,
    total: usize,
    completed: usize,
    succeeded: usize,
    failed: usize,
    dropped_non_targets: usize,
    first_failure: Option<String>,
}

enum AfterLocalAdjustFence {
    Bulk {
        owner_context_id: crate::app::ViewerContextId,
        targets: Vec<BulkPageEditTarget>,
        dropped_non_targets: usize,
        op: BulkPageEditOp,
    },
    Single {
        request: crate::edit_bundle::EditBundlePasteRequest,
    },
}

struct LocalAdjustFenceWait {
    stage: LocalAdjustDrainStage,
    next: AfterLocalAdjustFence,
}

enum LocalAdjustDrainStage {
    Producers,
    Writes { rx: crossbeam_channel::Receiver<()> },
}

enum BulkPageEditPhase {
    PasteConfirm(BulkPasteConfirm),
    ResetConfirm(BulkResetConfirm),
    WaitingForLocalAdjust(LocalAdjustFenceWait),
    Running(BulkPageEditRun),
}

/// App が持つ pending はこれ1個だけ。field の有無で phase を表さず enum に閉じ込める。
pub(crate) struct BulkPageEditPending {
    phase: BulkPageEditPhase,
}

enum BulkWorkerEvent {
    Item {
        target: BulkPageEditTarget,
        result: Result<PreparedPageEditBundle, String>,
    },
    Finished {
        cancelled: bool,
    },
}

fn drain_bulk_worker_events(
    rx: &Receiver<BulkWorkerEvent>,
    mut on_item: impl FnMut(BulkPageEditTarget, Result<PreparedPageEditBundle, String>),
) -> Result<bool, ()> {
    loop {
        match rx.recv() {
            Ok(BulkWorkerEvent::Item { target, result }) => on_item(target, result),
            Ok(BulkWorkerEvent::Finished { cancelled }) => return Ok(cancelled),
            Err(_) => return Err(()),
        }
    }
}

struct ResetBundleReaders {
    adjustment: Option<crate::adjustment_db::AdjustmentDb>,
    mask: Option<crate::mask_db::MaskDb>,
    conceal: Option<crate::conceal_db::ConcealDb>,
    local_adjust: Option<crate::local_adjust_db::LocalAdjustDb>,
    export_crop: Option<crate::export_crop::CropDb>,
    comic: Option<crate::comic_db::ComicDb>,
}

impl ResetBundleReaders {
    fn open(paths: &EditBundleDbPaths, kinds: ResetKinds) -> Result<Self, String> {
        let adjustment = (!kinds.adjustment)
            .then(|| {
                crate::adjustment_db::AdjustmentDb::open_at(&paths.adjustment)
                    .map_err(|error| format!("個別補正DBを開けませんでした: {error}"))
            })
            .transpose()?;
        let mask = (!kinds.mask)
            .then(|| {
                crate::mask_db::MaskDb::open_at(&paths.mask)
                    .map_err(|error| format!("消しゴムDBを開けませんでした: {error}"))
            })
            .transpose()?;
        let conceal = (!kinds.conceal)
            .then(|| {
                crate::conceal_db::ConcealDb::open_at(&paths.conceal)
                    .map_err(|error| format!("モザイクDBを開けませんでした: {error}"))
            })
            .transpose()?;
        let local_adjust = (!kinds.local_adjust)
            .then(|| {
                crate::local_adjust_db::LocalAdjustDb::open_at(&paths.local_adjust)
                    .map_err(|error| format!("補正レイヤーDBを開けませんでした: {error}"))
            })
            .transpose()?;
        let export_crop = (!kinds.export_crop)
            .then(|| {
                crate::export_crop::CropDb::open_at(&paths.export_crop)
                    .map_err(|error| format!("切り取りDBを開けませんでした: {error}"))
            })
            .transpose()?;
        let comic = (!kinds.comic)
            .then(|| {
                crate::comic_db::ComicDb::open_at(&paths.comic)
                    .map_err(|error| format!("注釈DBを開けませんでした: {error}"))
            })
            .transpose()?;
        Ok(Self {
            adjustment,
            mask,
            conceal,
            local_adjust,
            export_crop,
            comic,
        })
    }

    /// 未選択の種類だけを strict read する。選択済み種類は `None` のまま atomic apply へ
    /// 渡すため、RESET専用の別DELETE経路は作らない。
    fn load_kept_bundle(
        &self,
        key: &str,
        overrides: &KeptRuntimeOverrides,
    ) -> Result<PageEditBundle, String> {
        let adjust = overrides.adjustment.resolve(|| {
            self.adjustment
                .as_ref()
                .map(|db| db.get_page_params_checked(key))
                .transpose()
                .map(Option::flatten)
        })?;
        let mask = self
            .mask
            .as_ref()
            .map(|db| db.get_full_strict(key))
            .transpose()?
            .flatten()
            .map(|(pixels, shapes, size)| EditMaskSnapshot {
                pixels,
                shapes,
                size,
            });
        let conceal = self
            .conceal
            .as_ref()
            .map(|db| db.get_full_strict(key))
            .transpose()?
            .flatten()
            .map(|(pixels, shapes, size)| EditMaskSnapshot {
                pixels,
                shapes,
                size,
            });
        let local_adjust_layers = overrides
            .local_adjust
            .resolve(|| {
                self.local_adjust
                    .as_ref()
                    .map(|db| db.get_layers_checked(key))
                    .transpose()
                    .map(Option::flatten)
                    .map(|layers| layers.map(local_adjust_core::LocalAdjustmentLayers::new))
            })?
            .filter(|layers| !layers.is_empty());
        let export_crop = overrides.export_crop.resolve(|| {
            self.export_crop
                .as_ref()
                .map(|db| db.get_checked_strict(key))
                .transpose()
                .map(Option::flatten)
        })?;
        let comic = overrides
            .comic
            .resolve(|| {
                self.comic
                    .as_ref()
                    .map(|db| db.get_checked_strict(key))
                    .transpose()
                    .map(Option::flatten)
            })?
            .filter(|objects| !objects.is_empty());
        Ok(PageEditBundle {
            // reset は変換しない。mask / conceal は各 snapshot の保存寸法を持つ。
            source_size: [0, 0],
            adjust,
            mask,
            conceal,
            local_adjust_layers,
            export_crop,
            comic,
        })
    }
}

fn resolve_target_size(target: &BulkPageEditTarget) -> Result<[usize; 2], String> {
    if let Some([width, height]) = target.known_size
        && width > 0
        && height > 0
    {
        return Ok([width, height]);
    }

    let size = match &target.item {
        GridItem::Image(path) => crate::fast_resize::probe_dims(path),
        GridItem::ZipImage {
            zip_path,
            entry_name,
        } => {
            let bytes = crate::zip_loader::read_entry_bytes(zip_path, entry_name)
                .map_err(|error| format!("ZIP内画像を読み込めませんでした: {error}"))?;
            crate::app::probe_image_dims_from_bytes(&bytes)
                .map(|(width, height)| [width as usize, height as usize])
        }
        GridItem::PdfPage {
            content_type: Some(crate::pdf_loader::PdfPageContentType::Raster { w, h }),
            ..
        } if *w > 0 && *h > 0 => Some([*w as usize, *h as usize]),
        _ => None,
    };
    size.filter(|[width, height]| *width > 0 && *height > 0)
        .ok_or_else(|| "画像サイズを取得できませんでした".to_string())
}

fn prepare_and_apply_target(
    target: &BulkPageEditTarget,
    op: &BulkPageEditOp,
    paths: &EditBundleDbPaths,
    reset_readers: Result<&ResetBundleReaders, &str>,
) -> Result<PreparedPageEditBundle, String> {
    let prepared = match op {
        BulkPageEditOp::Paste { bundle, .. } => {
            let target_size = resolve_target_size(target)?;
            bundle.transformed_to(target_size)?.prepare()?
        }
        BulkPageEditOp::Reset { kinds } => {
            let readers = reset_readers.map_err(str::to_string)?;
            apply_reset_kinds(
                readers.load_kept_bundle(&target.page_key, &target.kept_runtime_overrides)?,
                *kinds,
            )
            .prepare()?
        }
    };
    prepared.apply_atomic(paths, &target.page_key)?;
    Ok(prepared)
}

fn spawn_bulk_worker(
    targets: Vec<BulkPageEditTarget>,
    op: BulkPageEditOp,
) -> Result<
    (
        Arc<AtomicBool>,
        Receiver<BulkWorkerEvent>,
        std::thread::JoinHandle<()>,
    ),
    String,
> {
    let paths = EditBundleDbPaths::default_data_dir();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    // Prepared mask は大きい。worker が UI commit より先走って複数件を保持しない。
    let (tx, rx) = mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("edit-bundle-bulk".to_string())
        .spawn(move || {
            let mut reset_readers = None;
            let mut reset_reader_error = None;
            if let BulkPageEditOp::Reset { kinds } = &op {
                match ResetBundleReaders::open(&paths, *kinds) {
                    Ok(readers) => reset_readers = Some(readers),
                    Err(error) => reset_reader_error = Some(error),
                }
            }

            let mut cancelled = false;
            for target in targets {
                if worker_cancel.load(Ordering::Relaxed) {
                    cancelled = true;
                    break;
                }
                let readers = match (&reset_readers, &reset_reader_error) {
                    (Some(readers), _) => Ok(readers),
                    (_, Some(error)) => Err(error.as_str()),
                    // Paste は reader を使わない。
                    _ => Err("リセット用DBを開けませんでした"),
                };
                let result = prepare_and_apply_target(&target, &op, &paths, readers);
                // apply_atomic 成功後の outcome は cancel が立っても必ず届ける。ここで捨てると
                // DB と runtime/cache/sidecar が乖離する。
                if tx.send(BulkWorkerEvent::Item { target, result }).is_err() {
                    return;
                }
            }
            let _ = tx.send(BulkWorkerEvent::Finished { cancelled });
        })
        .map_err(|error| format!("一括編集 worker を開始できませんでした: {error}"))?;
    Ok((cancel, rx, thread))
}

impl App {
    /// 右クリックメニューの「編集内容をまとめて貼り付け」入口。
    ///
    /// `cursor_idx` はチェックが空のときの対象。チェックがあればそちらが対象になる
    /// ([`select_bulk_target_indices`])。
    pub(crate) fn request_bulk_paste_page_edit_bundle(&mut self, cursor_idx: usize) {
        if self.edit_bundle_bulk_pending.is_some()
            || self.edit_bundle_copy_pending.is_some()
            || self.edit_bundle_apply_pending.is_some()
            || self.edit_bundle_paste_pending.is_some()
        {
            self.show_feedback_toast("別の編集内容を処理しています".to_string());
            return;
        }
        if !self.edit_bundle_databases_available() {
            self.show_feedback_toast("編集データベースを利用できません".to_string());
            return;
        }
        let Some(clipboard) = self.edit_bundle_clipboard.clone() else {
            self.show_feedback_toast("コピーされた編集内容がありません".to_string());
            return;
        };
        // **要求を出した viewer context をここで焼き付ける** (F08)。以降の確認・fence・
        // worker 完了は、どの viewport が poll してもこの context へ戻す。
        let owner_context_id = self.edit_request_owner_context();
        let (targets, dropped_non_targets) = self.bulk_page_edit_targets(cursor_idx);
        if targets.is_empty() {
            self.show_feedback_toast(if dropped_non_targets > 0 {
                format!("成功 0 件 / 対象外 {dropped_non_targets} 件")
            } else {
                "編集内容を貼り付ける対象がありません".to_string()
            });
            return;
        }

        let overwrite_count = targets
            .iter()
            .filter(|target| self.page_has_any_bundle_edit(target.idx, &target.page_key))
            .count();
        let op = BulkPageEditOp::Paste {
            bundle: clipboard.bundle,
            source_label: clipboard.source_label,
        };
        if overwrite_count > 0 {
            self.edit_bundle_bulk_pending = Some(BulkPageEditPending {
                phase: BulkPageEditPhase::PasteConfirm(BulkPasteConfirm {
                    owner_context_id,
                    targets,
                    dropped_non_targets,
                    overwrite_count,
                    op,
                }),
            });
        } else {
            self.start_bulk_page_edit(owner_context_id, targets, dropped_non_targets, op);
        }
    }

    /// 右クリックメニューの「編集内容をリセット…」入口。単一もチェック複数もここを通る。
    ///
    /// RESET確認は必ず `BulkPageEditPhase::ResetConfirm` を経由し、直接 worker を始めない。
    pub(crate) fn request_bulk_reset_page_edits(&mut self, cursor_idx: usize) {
        if self.edit_bundle_bulk_pending.is_some()
            || self.edit_bundle_copy_pending.is_some()
            || self.edit_bundle_apply_pending.is_some()
            || self.edit_bundle_paste_pending.is_some()
        {
            self.show_feedback_toast("別の編集内容を処理しています".to_string());
            return;
        }
        let owner_context_id = self.edit_request_owner_context();
        let (targets, dropped_non_targets) = self.bulk_page_edit_targets(cursor_idx);
        let presences = targets
            .iter()
            .map(|target| TargetEditPresence {
                adjustment: self.adjusted_page_keys.contains(&target.page_key),
                mask: self.mask_page_keys.contains(&target.page_key),
                conceal: self.conceal_page_keys.contains(&target.page_key),
                local_adjust: self.local_adjust_page_keys.contains(&target.page_key),
                comic: self.comic_page_keys.contains(&target.page_key),
                export_crop: self.export_crop_pages.contains(&target.idx),
                // get_rotation は cache miss 時に同期SELECTする。起動時に全件ロードされ、
                // set_image_rotation でも同期される key set だけを見る。
                rotation: self.rotation_page_keys.contains(&target.page_key),
            })
            .collect::<Vec<_>>();
        let counts = count_reset_kinds(&presences);
        let mut kinds = ResetKinds::all();
        kinds.uncheck_empty(counts);
        self.edit_bundle_bulk_pending = Some(BulkPageEditPending {
            phase: BulkPageEditPhase::ResetConfirm(BulkResetConfirm {
                owner_context_id,
                targets,
                dropped_non_targets,
                counts,
                kinds,
            }),
        });
    }

    fn bulk_page_edit_targets(&self, cursor_idx: usize) -> (Vec<BulkPageEditTarget>, usize) {
        let selection = select_bulk_target_indices(&self.items, &self.checked, Some(cursor_idx));
        let mut targets = Vec::with_capacity(selection.indices.len());
        for idx in selection.indices {
            let Some(item) = self.items.get(idx).cloned() else {
                continue;
            };
            let Some(page_key) = self.page_path_key(idx) else {
                crate::logger::log(format!(
                    "edit_bundle_bulk: page target has no page key idx={idx}"
                ));
                continue;
            };
            let display_label = item.name().into_owned();
            let sidecar_coords = (!self.idx_is_compiled_book_page(idx))
                .then(|| self.sidecar_coords(idx))
                .flatten();
            targets.push(BulkPageEditTarget {
                idx,
                page_key,
                display_label,
                item,
                sidecar_coords,
                known_size: self.known_page_source_size(idx),
                kept_runtime_overrides: KeptRuntimeOverrides::default(),
            });
        }
        (targets, selection.dropped_non_targets)
    }

    fn next_local_adjust_drain_stage(&self) -> Result<Option<LocalAdjustDrainStage>, String> {
        if self.local_adjust_lut_pending.is_some()
            || self.local_adjust_segmentation_pending.is_some()
        {
            return Ok(Some(LocalAdjustDrainStage::Producers));
        }
        if self.local_adjust_write_pending.is_empty() {
            return Ok(None);
        }
        let handle = self.local_adjust_write_handle.as_ref().ok_or_else(|| {
            "補正レイヤーの保存待ちがありますが、保存ワーカーがありません".to_string()
        })?;
        handle
            .enqueue_fence()
            .map(|rx| Some(LocalAdjustDrainStage::Writes { rx }))
            .map_err(|error| format!("補正レイヤーの保存完了を待てませんでした: {error}"))
    }

    /// 単一貼り付けも同じ local-adjust writer と競合するため、bulk と同じ非同期フェンスへ
    /// 合流させる。`true` は待機または開始失敗をこの owner が引き受けたことを表す。
    pub(crate) fn defer_single_page_edit_apply_for_local_adjust(
        &mut self,
        request: crate::edit_bundle::EditBundlePasteRequest,
    ) -> bool {
        self.commit_local_adjust_pending_edits();
        match self.next_local_adjust_drain_stage() {
            Ok(None) => false,
            Ok(Some(stage)) => {
                self.edit_bundle_bulk_pending = Some(BulkPageEditPending {
                    phase: BulkPageEditPhase::WaitingForLocalAdjust(LocalAdjustFenceWait {
                        stage,
                        next: AfterLocalAdjustFence::Single { request },
                    }),
                });
                true
            }
            Err(error) => {
                self.show_feedback_toast(error);
                true
            }
        }
    }

    fn start_bulk_page_edit(
        &mut self,
        owner_context_id: crate::app::ViewerContextId,
        mut targets: Vec<BulkPageEditTarget>,
        dropped_non_targets: usize,
        op: BulkPageEditOp,
    ) {
        if targets.is_empty() {
            self.show_feedback_toast(if dropped_non_targets > 0 {
                format!("成功 0 件 / 対象外 {dropped_non_targets} 件")
            } else {
                "一括編集の対象がありません".to_string()
            });
            return;
        }
        if self.edit_bundle_apply_pending.is_some() || self.edit_bundle_paste_pending.is_some() {
            self.show_feedback_toast("別の編集内容を処理しています".to_string());
            return;
        }
        if op.needs_bundle_worker() && !self.edit_bundle_databases_available() {
            self.show_feedback_toast("編集データベースを利用できません".to_string());
            return;
        }

        if op.needs_bundle_worker() {
            // 直前の補正レイヤー編集と、LUT / mask producer の結果を先に durable にする。
            // これを待たないと bulk transaction の後から古い保存が着地して上書きする。
            self.commit_local_adjust_pending_edits();
            match self.next_local_adjust_drain_stage() {
                Ok(None) => {}
                Ok(Some(stage)) => {
                    self.edit_bundle_bulk_pending = Some(BulkPageEditPending {
                        phase: BulkPageEditPhase::WaitingForLocalAdjust(LocalAdjustFenceWait {
                            stage,
                            next: AfterLocalAdjustFence::Bulk {
                                owner_context_id,
                                targets,
                                dropped_non_targets,
                                op,
                            },
                        }),
                    });
                    return;
                }
                Err(error) => {
                    self.show_feedback_toast(error);
                    return;
                }
            }
            // 保持 override の snapshot は **UI memory を読む**。確認ボタンと fence の
            // 完了はどちらの viewport でも起き得るので、読む先を所有者へ固定する
            // (v3.5.0 レビュー F08 の追補)。
            let snapshotted = self.with_owner_viewer_context(owner_context_id, |app| {
                app.snapshot_bulk_kept_overrides(&mut targets, &op);
            });
            if snapshotted.is_none() {
                self.show_feedback_toast(
                    "編集を開始したウィンドウが閉じられたため、一括編集を開始できませんでした"
                        .to_string(),
                );
                return;
            }
        }

        let total = targets.len();
        let (effect, source_label) = match &op {
            BulkPageEditOp::Paste { source_label, .. } => {
                (BulkRunEffect::Paste, Some(source_label.clone()))
            }
            BulkPageEditOp::Reset { kinds } => (BulkRunEffect::Reset { kinds: *kinds }, None),
        };
        let (cancel, driver) = match &op {
            BulkPageEditOp::Reset { kinds } if kinds.rotation_only() => {
                let cancel = Arc::new(AtomicBool::new(false));
                (
                    cancel,
                    BulkRunDriver::RotationOnly {
                        remaining: targets.into(),
                    },
                )
            }
            _ => match spawn_bulk_worker(targets, op) {
                Ok((cancel, rx, thread)) => (
                    cancel,
                    BulkRunDriver::Worker {
                        rx,
                        thread: Some(thread),
                    },
                ),
                Err(error) => {
                    self.show_feedback_toast(error);
                    return;
                }
            },
        };
        self.edit_bundle_bulk_pending = Some(BulkPageEditPending {
            phase: BulkPageEditPhase::Running(BulkPageEditRun {
                owner_context_id,
                driver,
                effect,
                source_label,
                cancel,
                total,
                completed: 0,
                succeeded: 0,
                failed: 0,
                dropped_non_targets,
                first_failure: None,
            }),
        });
    }

    fn snapshot_bulk_kept_overrides(
        &self,
        targets: &mut [BulkPageEditTarget],
        op: &BulkPageEditOp,
    ) {
        let BulkPageEditOp::Reset { kinds } = op else {
            return;
        };
        // 単一コピーがmemory overrideを渡す4種類と揃える。adjustmentも通常確定時は
        // DBと同時だが、drag中と保存失敗時はmemoryが先行する。mask / concealの編集中
        // bufferはページ別runtime cacheではなく、既存コピーも意図的にDBを正本とする。
        for target in targets {
            let current_idx = self.current_idx_for_bulk_target(target);
            if !kinds.adjustment {
                target.kept_runtime_overrides.adjustment = KeptRuntimeValue::from_memory_state(
                    current_idx.and_then(|idx| self.adjustment_page_params.get(&idx).cloned()),
                    self.adjusted_page_keys.contains(&target.page_key),
                );
            }
            if !kinds.local_adjust {
                target.kept_runtime_overrides.local_adjust = KeptRuntimeValue::from_memory_state(
                    current_idx.and_then(|idx| self.local_adjust_page_layers.get(&idx).cloned()),
                    self.local_adjust_page_keys.contains(&target.page_key),
                );
            }
            if !kinds.export_crop
                && let Some(idx) = current_idx
            {
                target.kept_runtime_overrides.export_crop = KeptRuntimeValue::from_memory_state(
                    self.export_crop_page_settings.get(&idx).copied(),
                    self.export_crop_pages.contains(&idx),
                );
            }
            if !kinds.comic {
                target.kept_runtime_overrides.comic = KeptRuntimeValue::from_memory_state(
                    self.comic_docs.get(&target.page_key).cloned(),
                    self.comic_page_keys.contains(&target.page_key),
                );
            }
        }
    }

    /// 終了時は現在itemのtransaction完了まで待ち、成功 outcome を runtime / sidecar
    /// へ反映してから既存の終了時 sidecar flush に渡す。先に join すると bounded
    /// channel の send で相互待ちになるため、Finished まで受信してから join する。
    pub(crate) fn finish_bulk_page_edit_for_exit(&mut self) {
        let Some(BulkPageEditPending {
            phase: BulkPageEditPhase::Running(mut run),
        }) = self.edit_bundle_bulk_pending.take()
        else {
            // 確認中 / fence待ち / 単一貼り付け待ちはまだDB操作を開始していない。
            return;
        };
        run.cancel.store(true, Ordering::Relaxed);
        let effect = run.effect;
        let owner = run.owner_context_id;
        let BulkRunDriver::Worker { rx, thread } = &mut run.driver else {
            // 回転のみはUI threadでしか進まないので、終了時に未処理分を開始しない。
            return;
        };

        if drain_bulk_worker_events(rx, |target, result| {
            // 終了時の残り分も要求元へ戻す。回転リセットは所有者の items から idx を
            // 引き直すので、mount 中の別ウィンドウで解決すると別のページを触り得る。
            let result = result.and_then(|prepared| {
                self.in_bulk_edit_owner(owner, |app| {
                    app.apply_bulk_worker_success(&target, prepared, effect)
                })
            });
            if let Err(error) = result {
                crate::logger::log(format!(
                    "edit_bundle_bulk: exit drain failed key={} label={} error={error}",
                    target.page_key, target.display_label
                ));
            }
        })
        .is_err()
        {
            crate::logger::log(
                "edit_bundle_bulk: worker disconnected during exit drain".to_string(),
            );
        }
        if thread.take().is_some_and(|thread| thread.join().is_err()) {
            crate::logger::log("edit_bundle_bulk: worker panicked during exit".to_string());
        }
    }
}

/// 終了トーストの文面。件数の意味が変わる唯一の場所。
///
/// 少数選択の一括処理は数フレームで終わり、実機ではキャンセルを押せないことがある。
/// その経路を言葉にするのはここだけなので、キャンセル時の「未処理 N 件」と
/// 「適用済みは残る」はこの関数のテストで固定する。
fn bulk_summary_text(run: &BulkPageEditRun, cancelled: bool) -> String {
    let unprocessed = run.total.saturating_sub(run.completed);
    let mut summary = if run.failed == 0 {
        format!("成功 {} 件", run.succeeded)
    } else {
        format!("成功 {} 件 / 失敗 {} 件", run.succeeded, run.failed)
    };
    if run.dropped_non_targets > 0 {
        summary.push_str(&format!(" / 対象外 {} 件", run.dropped_non_targets));
    }
    if cancelled {
        if unprocessed > 0 {
            summary.push_str(&format!(" / 未処理 {unprocessed} 件"));
        }
        summary.push_str("（キャンセルしました。適用済みの内容は保持されます）");
    }
    if let Some(first_failure) = &run.first_failure {
        summary.push_str(&format!("\n最初の失敗: {first_failure}"));
    }
    summary
}

enum BulkPollAction {
    Worker(BulkWorkerEvent),
    RotationOnly(BulkPageEditTarget),
    LocalAdjustFenceReady,
    LocalAdjustFenceFailed,
    DriverDisconnected,
}

#[derive(Clone, Copy)]
enum BulkDialogAction {
    None,
    CloseConfirm,
    StartPaste,
    StartReset,
    RequestCancel,
}

impl App {
    fn current_idx_for_bulk_target(&self, target: &BulkPageEditTarget) -> Option<usize> {
        if self.page_path_key(target.idx).as_deref() == Some(target.page_key.as_str()) {
            Some(target.idx)
        } else {
            (0..self.items.len())
                .find(|&idx| self.page_path_key(idx).as_deref() == Some(target.page_key.as_str()))
        }
    }

    fn reset_bulk_target_rotation(
        &mut self,
        target: &BulkPageEditTarget,
        current_idx: Option<usize>,
    ) -> Result<(), String> {
        if !self.rotation_page_keys.contains(&target.page_key) {
            return Ok(());
        }
        let idx = current_idx.ok_or_else(|| {
            "一覧が更新され、回転をリセットする対象を特定できませんでした".to_string()
        })?;
        // cache / rotation.db / page-key presence の所有者を迂回しない。
        self.set_image_rotation(idx, Rotation::None)
    }

    fn apply_bulk_worker_success(
        &mut self,
        target: &BulkPageEditTarget,
        prepared: PreparedPageEditBundle,
        effect: BulkRunEffect,
    ) -> Result<(), String> {
        let current_idx = self.current_idx_for_bulk_target(target);
        self.commit_page_edit_bundle_to_runtime(
            current_idx,
            &target.page_key,
            target.sidecar_coords.as_ref(),
            prepared,
        );
        let undo_kinds = match effect {
            BulkRunEffect::Paste => crate::edit_bundle_app::PageEditUndoInvalidation::all(),
            BulkRunEffect::Reset { kinds } => crate::edit_bundle_app::PageEditUndoInvalidation {
                adjustment: kinds.adjustment,
                mask: kinds.mask,
                conceal: kinds.conceal,
                local_adjust: kinds.local_adjust,
                comic: kinds.comic,
            },
        };
        self.invalidate_page_edit_undo_after_bundle_apply(
            target.idx,
            current_idx,
            &target.page_key,
            undo_kinds,
        );
        if let BulkRunEffect::Reset { kinds } = effect
            && kinds.rotation
        {
            // bundle が失敗した item ではここへ来ない。6 DB が成功したあとだけ回転を外す。
            self.reset_bulk_target_rotation(target, current_idx)?;
        }
        Ok(())
    }

    /// 一括編集の完了処理を、**要求を出した viewer context を mount した状態で**実行する。
    ///
    /// その context が既に無ければ (別ウィンドウを閉じた等)、DB は worker が既に書いている
    /// ので成功だが、runtime へ反映する先が無い。件ごとの失敗として利用者へ出す。
    fn in_bulk_edit_owner(
        &mut self,
        owner: crate::app::ViewerContextId,
        f: impl FnOnce(&mut Self) -> Result<(), String>,
    ) -> Result<(), String> {
        self.with_owner_viewer_context(owner, f).unwrap_or_else(|| {
            Err("編集を開始したウィンドウが閉じられたため、表示へ反映できませんでした".to_string())
        })
    }

    fn record_bulk_item_result(&mut self, target: &BulkPageEditTarget, result: Result<(), String>) {
        let Some(BulkPageEditPending {
            phase: BulkPageEditPhase::Running(run),
        }) = self.edit_bundle_bulk_pending.as_mut()
        else {
            return;
        };
        run.completed += 1;
        match result {
            Ok(()) => run.succeeded += 1,
            Err(error) => {
                run.failed += 1;
                let detail = format!("{}: {error}", target.display_label);
                if run.first_failure.is_none() {
                    run.first_failure = Some(detail.clone());
                }
                crate::logger::log(format!(
                    "edit_bundle_bulk: item failed key={} label={} error={error}",
                    target.page_key, target.display_label
                ));
            }
        }
    }

    fn finish_bulk_page_edit(&mut self, cancelled: bool) {
        let Some(BulkPageEditPending {
            phase: BulkPageEditPhase::Running(run),
        }) = self.edit_bundle_bulk_pending.take()
        else {
            return;
        };
        self.show_feedback_toast(bulk_summary_text(&run, cancelled));
    }

    fn poll_bulk_page_edit(&mut self, ctx: &egui::Context) {
        let waiting_for_local_adjust = matches!(
            self.edit_bundle_bulk_pending,
            Some(BulkPageEditPending {
                phase: BulkPageEditPhase::WaitingForLocalAdjust(_)
            })
        );
        if waiting_for_local_adjust {
            // producer 完了が新しい保存を enqueue し得るため、producer → write result の順で
            // 進めてからフェンス状態を判定する。
            self.poll_local_adjust_lut_load(ctx);
            self.poll_local_adjust_segmentation(ctx);
            self.poll_local_adjust_write_results();
        }
        let action = match self.edit_bundle_bulk_pending.as_mut() {
            Some(BulkPageEditPending {
                phase: BulkPageEditPhase::WaitingForLocalAdjust(wait),
            }) => match &wait.stage {
                LocalAdjustDrainStage::Producers => {
                    if self.local_adjust_lut_pending.is_none()
                        && self.local_adjust_segmentation_pending.is_none()
                    {
                        Some(BulkPollAction::LocalAdjustFenceReady)
                    } else {
                        None
                    }
                }
                LocalAdjustDrainStage::Writes { rx } => match rx.try_recv() {
                    Ok(()) => Some(BulkPollAction::LocalAdjustFenceReady),
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        Some(BulkPollAction::LocalAdjustFenceFailed)
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => None,
                },
            },
            Some(BulkPageEditPending {
                phase: BulkPageEditPhase::Running(run),
            }) => match &mut run.driver {
                BulkRunDriver::Worker { rx, .. } => match rx.try_recv() {
                    Ok(event) => Some(BulkPollAction::Worker(event)),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Some(BulkPollAction::DriverDisconnected)
                    }
                    Err(mpsc::TryRecvError::Empty) => None,
                },
                BulkRunDriver::RotationOnly { remaining } => {
                    if run.cancel.load(Ordering::Relaxed) && !remaining.is_empty() {
                        Some(BulkPollAction::Worker(BulkWorkerEvent::Finished {
                            cancelled: true,
                        }))
                    } else if let Some(target) = remaining.pop_front() {
                        Some(BulkPollAction::RotationOnly(target))
                    } else {
                        Some(BulkPollAction::Worker(BulkWorkerEvent::Finished {
                            cancelled: false,
                        }))
                    }
                }
            },
            _ => None,
        };

        match action {
            Some(BulkPollAction::Worker(BulkWorkerEvent::Item { target, result })) => {
                let (effect, owner) = match self.edit_bundle_bulk_pending.as_ref() {
                    Some(BulkPageEditPending {
                        phase: BulkPageEditPhase::Running(run),
                    }) => (run.effect, run.owner_context_id),
                    _ => return,
                };
                // **要求を出した context へ戻す。** どの viewport が先に drain しても、
                // items / キャッシュ / undo を更新するのは所有者側 (F08)。
                let applied = result.and_then(|prepared| {
                    self.in_bulk_edit_owner(owner, |app| {
                        app.apply_bulk_worker_success(&target, prepared, effect)
                    })
                });
                self.record_bulk_item_result(&target, applied);
            }
            Some(BulkPollAction::RotationOnly(target)) => {
                let owner = match self.edit_bundle_bulk_pending.as_ref() {
                    Some(BulkPageEditPending {
                        phase: BulkPageEditPhase::Running(run),
                    }) => run.owner_context_id,
                    _ => return,
                };
                let result = self.in_bulk_edit_owner(owner, |app| {
                    let current_idx = app.current_idx_for_bulk_target(&target);
                    app.reset_bulk_target_rotation(&target, current_idx)
                });
                self.record_bulk_item_result(&target, result);
            }
            Some(BulkPollAction::LocalAdjustFenceReady) => {
                // Fence ACK より前の completion は送信済み。sidecar mirror と pending map を
                // 先に反映してから、producer後発分を含む次stageを決める。
                self.poll_local_adjust_write_results();
                match self.next_local_adjust_drain_stage() {
                    Ok(Some(stage)) => {
                        if let Some(BulkPageEditPending {
                            phase: BulkPageEditPhase::WaitingForLocalAdjust(wait),
                        }) = self.edit_bundle_bulk_pending.as_mut()
                        {
                            wait.stage = stage;
                        }
                    }
                    Ok(None) => {
                        let Some(BulkPageEditPending {
                            phase: BulkPageEditPhase::WaitingForLocalAdjust(wait),
                        }) = self.edit_bundle_bulk_pending.take()
                        else {
                            return;
                        };
                        match wait.next {
                            AfterLocalAdjustFence::Bulk {
                                owner_context_id,
                                targets,
                                dropped_non_targets,
                                op,
                            } => self.start_bulk_page_edit(
                                owner_context_id,
                                targets,
                                dropped_non_targets,
                                op,
                            ),
                            AfterLocalAdjustFence::Single { request } => {
                                self.start_apply_page_edit_bundle(request);
                                // 単一貼り付け側の poll はこのフレームでは既に終わっている。
                                ctx.request_repaint_after(std::time::Duration::from_millis(40));
                            }
                        }
                    }
                    Err(error) => {
                        self.edit_bundle_bulk_pending = None;
                        self.show_feedback_toast(error);
                    }
                }
            }
            Some(BulkPollAction::LocalAdjustFenceFailed) => {
                self.edit_bundle_bulk_pending = None;
                self.show_feedback_toast(
                    "補正レイヤー保存ワーカーが停止したため、一括編集を開始できませんでした"
                        .to_string(),
                );
            }
            Some(BulkPollAction::Worker(BulkWorkerEvent::Finished { cancelled })) => {
                self.finish_bulk_page_edit(cancelled);
            }
            Some(BulkPollAction::DriverDisconnected) => {
                if let Some(BulkPageEditPending {
                    phase: BulkPageEditPhase::Running(run),
                }) = self.edit_bundle_bulk_pending.as_mut()
                {
                    let remaining = run.total.saturating_sub(run.completed);
                    run.failed += remaining;
                    run.completed = run.total;
                    run.first_failure.get_or_insert_with(|| {
                        "一括編集 worker が完了通知を返さず終了しました".to_string()
                    });
                }
                self.finish_bulk_page_edit(false);
            }
            None => {}
        }

        if matches!(
            self.edit_bundle_bulk_pending,
            Some(BulkPageEditPending {
                phase: BulkPageEditPhase::WaitingForLocalAdjust(_) | BulkPageEditPhase::Running(_)
            })
        ) {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        }
    }

    pub(crate) fn show_bulk_page_edit_dialog(&mut self, ctx: &egui::Context) {
        self.poll_bulk_page_edit(ctx);
        if self.edit_bundle_bulk_pending.is_none() {
            return;
        }
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let mut action = BulkDialogAction::None;

        match self.edit_bundle_bulk_pending.as_mut() {
            Some(BulkPageEditPending {
                phase: BulkPageEditPhase::PasteConfirm(confirm),
            }) => {
                if escape_pressed {
                    action = BulkDialogAction::CloseConfirm;
                }
                let source_label = match &confirm.op {
                    BulkPageEditOp::Paste { source_label, .. } => source_label.as_str(),
                    BulkPageEditOp::Reset { .. } => "コピー元",
                };
                egui::Window::new("編集内容をまとめて貼り付け")
                    .id(egui::Id::new("edit_bundle_bulk_paste_confirm"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.set_min_width(430.0);
                        ui.label(format!("対象: {} 件", confirm.targets.len()));
                        ui.label(format!(
                            "既存の編集内容を上書きする項目: {} 件",
                            confirm.overwrite_count
                        ));
                        ui.label(format!(
                            "対象外として除外: {} 件",
                            confirm.dropped_non_targets
                        ));
                        ui.add_space(6.0);
                        ui.label(format!(
                            "「{source_label}」の6種類の編集内容で、対象をまとめて上書きします。"
                        ));
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("上書きして貼り付け").clicked() {
                                action = BulkDialogAction::StartPaste;
                            }
                            if ui.button("キャンセル").clicked() {
                                action = BulkDialogAction::CloseConfirm;
                            }
                        });
                    });
            }
            Some(BulkPageEditPending {
                phase: BulkPageEditPhase::ResetConfirm(confirm),
            }) => {
                if escape_pressed {
                    action = BulkDialogAction::CloseConfirm;
                }
                egui::Window::new("編集内容をリセット")
                    .id(egui::Id::new("edit_bundle_bulk_reset_confirm"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.set_min_width(390.0);
                        ui.label(format!("対象: {} 件", confirm.targets.len()));
                        ui.label(format!(
                            "対象外として除外: {} 件",
                            confirm.dropped_non_targets
                        ));
                        ui.add_space(6.0);
                        reset_kind_checkbox(
                            ui,
                            &mut confirm.kinds.adjustment,
                            "補正",
                            confirm.counts.adjustment,
                        );
                        reset_kind_checkbox(
                            ui,
                            &mut confirm.kinds.mask,
                            "消しゴム",
                            confirm.counts.mask,
                        );
                        reset_kind_checkbox(
                            ui,
                            &mut confirm.kinds.conceal,
                            "モザイク",
                            confirm.counts.conceal,
                        );
                        reset_kind_checkbox(
                            ui,
                            &mut confirm.kinds.local_adjust,
                            "補正レイヤー",
                            confirm.counts.local_adjust,
                        );
                        reset_kind_checkbox(
                            ui,
                            &mut confirm.kinds.comic,
                            "注釈",
                            confirm.counts.comic,
                        );
                        reset_kind_checkbox(
                            ui,
                            &mut confirm.kinds.export_crop,
                            "切り取り",
                            confirm.counts.export_crop,
                        );
                        reset_kind_checkbox(
                            ui,
                            &mut confirm.kinds.rotation,
                            "回転",
                            confirm.counts.rotation,
                        );
                        ui.add_space(6.0);
                        ui.label("この操作は元に戻せません。★とタグは変更しません。");
                        ui.add_space(8.0);
                        let can_reset = !confirm.targets.is_empty() && confirm.kinds.any();
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(can_reset, egui::Button::new("リセット"))
                                .clicked()
                            {
                                action = BulkDialogAction::StartReset;
                            }
                            if ui.button("キャンセル").clicked() {
                                action = BulkDialogAction::CloseConfirm;
                            }
                        });
                    });
            }
            Some(BulkPageEditPending {
                phase: BulkPageEditPhase::WaitingForLocalAdjust(wait),
            }) => {
                if escape_pressed {
                    action = BulkDialogAction::RequestCancel;
                }
                let (target_count, dropped_non_targets) = match &wait.next {
                    AfterLocalAdjustFence::Bulk {
                        targets,
                        dropped_non_targets,
                        ..
                    } => (Some(targets.len()), *dropped_non_targets),
                    AfterLocalAdjustFence::Single { .. } => (None, 0),
                };
                egui::Window::new("編集内容を処理する準備")
                    .id(egui::Id::new("edit_bundle_local_adjust_fence"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.set_min_width(360.0);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("補正レイヤーの保存完了を待っています…");
                        });
                        if let Some(target_count) = target_count {
                            ui.label(format!("対象: {target_count} 件"));
                        }
                        if dropped_non_targets > 0 {
                            ui.label(format!("対象外として除外: {dropped_non_targets} 件"));
                        }
                        if ui.button("キャンセル").clicked() {
                            action = BulkDialogAction::RequestCancel;
                        }
                    });
            }
            Some(BulkPageEditPending {
                phase: BulkPageEditPhase::Running(run),
            }) => {
                if escape_pressed {
                    action = BulkDialogAction::RequestCancel;
                }
                let cancel_requested = run.cancel.load(Ordering::Relaxed);
                egui::Window::new("編集内容を一括処理")
                    .id(egui::Id::new("edit_bundle_bulk_progress"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.set_min_width(340.0);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            match (run.effect, run.source_label.as_deref()) {
                                (BulkRunEffect::Paste, Some(source_label)) => {
                                    ui.label(format!(
                                        "「{source_label}」の編集内容を貼り付けています…"
                                    ));
                                }
                                _ => {
                                    ui.label("編集内容をリセットしています…");
                                }
                            }
                        });
                        ui.label(format!("{} / {} 件", run.completed, run.total));
                        if run.dropped_non_targets > 0 {
                            ui.label(format!("対象外として除外: {} 件", run.dropped_non_targets));
                        }
                        if cancel_requested {
                            ui.label("キャンセル中…");
                        } else if ui.button("キャンセル").clicked() {
                            action = BulkDialogAction::RequestCancel;
                        }
                    });
            }
            None => {}
        }

        match action {
            BulkDialogAction::CloseConfirm => {
                self.edit_bundle_bulk_pending = None;
            }
            BulkDialogAction::StartPaste => {
                let Some(BulkPageEditPending {
                    phase: BulkPageEditPhase::PasteConfirm(confirm),
                }) = self.edit_bundle_bulk_pending.take()
                else {
                    return;
                };
                self.start_bulk_page_edit(
                    confirm.owner_context_id,
                    confirm.targets,
                    confirm.dropped_non_targets,
                    confirm.op,
                );
            }
            BulkDialogAction::StartReset => {
                let Some(BulkPageEditPending {
                    phase: BulkPageEditPhase::ResetConfirm(confirm),
                }) = self.edit_bundle_bulk_pending.take()
                else {
                    return;
                };
                self.start_bulk_page_edit(
                    confirm.owner_context_id,
                    confirm.targets,
                    confirm.dropped_non_targets,
                    BulkPageEditOp::Reset {
                        kinds: confirm.kinds,
                    },
                );
            }
            BulkDialogAction::RequestCancel => {
                match self.edit_bundle_bulk_pending.take() {
                    Some(BulkPageEditPending {
                        phase: BulkPageEditPhase::WaitingForLocalAdjust(wait),
                    }) => {
                        // bulk DB処理はまだ始まっていない。local-adjust保存自体は継続する。
                        match wait.next {
                            AfterLocalAdjustFence::Bulk {
                                targets,
                                dropped_non_targets,
                                ..
                            } => {
                                let mut summary = "成功 0 件".to_string();
                                if dropped_non_targets > 0 {
                                    summary
                                        .push_str(&format!(" / 対象外 {dropped_non_targets} 件"));
                                }
                                if !targets.is_empty() {
                                    summary.push_str(&format!(" / 未処理 {} 件", targets.len()));
                                }
                                summary.push_str(
                                    "（キャンセルしました。適用済みの内容は保持されます）",
                                );
                                self.show_feedback_toast(summary);
                            }
                            AfterLocalAdjustFence::Single { .. } => self.show_feedback_toast(
                                "編集内容の貼り付けをキャンセルしました".to_string(),
                            ),
                        }
                    }
                    Some(BulkPageEditPending {
                        phase: BulkPageEditPhase::Running(run),
                    }) => {
                        // receiver は捨てず、現在itemの outcome と Finished までpollする。
                        run.cancel.store(true, Ordering::Relaxed);
                        self.edit_bundle_bulk_pending = Some(BulkPageEditPending {
                            phase: BulkPageEditPhase::Running(run),
                        });
                        ctx.request_repaint();
                    }
                    pending => {
                        self.edit_bundle_bulk_pending = pending;
                    }
                }
            }
            BulkDialogAction::None => {}
        }
    }
}

fn reset_kind_checkbox(ui: &mut egui::Ui, checked: &mut bool, label: &str, count: usize) {
    ui.add_enabled_ui(count > 0, |ui| {
        ui.checkbox(checked, format!("{label} {count} 件"));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(name: &str) -> GridItem {
        GridItem::Image(PathBuf::from(name))
    }

    #[test]
    fn checked_targets_win_over_cursor_and_non_targets_are_reported() {
        let items = vec![
            image("cursor.png"),
            GridItem::Video(PathBuf::from("movie.mp4")),
            GridItem::ZipImage {
                zip_path: PathBuf::from("book.zip"),
                entry_name: "page.png".to_string(),
            },
        ];
        let checked = HashSet::from([1, 2]);
        let selected = select_bulk_target_indices(&items, &checked, Some(0));
        assert_eq!(selected.indices, vec![2]);
        assert_eq!(selected.dropped_non_targets, 1);

        let cursor_only = select_bulk_target_indices(&items, &HashSet::new(), Some(0));
        assert_eq!(cursor_only.indices, vec![0]);
        assert_eq!(cursor_only.dropped_non_targets, 0);
    }

    #[test]
    fn all_checked_non_targets_return_an_explicit_empty_result() {
        let items = vec![
            image("cursor.png"),
            GridItem::Audio(PathBuf::from("sound.flac")),
            GridItem::Folder(PathBuf::from("folder")),
        ];
        let selected = select_bulk_target_indices(&items, &HashSet::from([1, 2]), Some(0));
        assert!(selected.indices.is_empty());
        assert_eq!(selected.dropped_non_targets, 2);
    }

    #[test]
    fn reset_subset_clears_only_selected_bundle_fields() {
        let mask = EditMaskSnapshot {
            pixels: vec![true, false, false, true],
            shapes: Vec::new(),
            size: [2, 2],
        };
        let crop = crate::export_crop::CropSettings {
            rect: crate::export_crop::CropRect {
                min_x: 0.1,
                min_y: 0.2,
                max_x: 0.8,
                max_y: 0.9,
            },
            aspect_mode: crate::export_crop::CropAspectMode::Free,
            source_size: Some([20, 30]),
        };
        let bundle = PageEditBundle {
            source_size: [20, 30],
            adjust: Some(crate::adjustment::AdjustParams::default()),
            mask: Some(mask.clone()),
            conceal: Some(mask),
            local_adjust_layers: Some(local_adjust_core::LocalAdjustmentLayers::default()),
            export_crop: Some(crop),
            comic: Some(Vec::new()),
        };
        let kinds = ResetKinds {
            adjustment: true,
            conceal: true,
            comic: true,
            rotation: true,
            ..ResetKinds::default()
        };
        let reset = apply_reset_kinds(bundle.clone(), kinds);
        assert_eq!(reset.source_size, bundle.source_size);
        assert!(reset.adjust.is_none());
        assert_eq!(reset.mask, bundle.mask);
        assert!(reset.conceal.is_none());
        assert_eq!(reset.local_adjust_layers, bundle.local_adjust_layers);
        assert_eq!(reset.export_crop, bundle.export_crop);
        assert!(reset.comic.is_none());

        let rotation_only = ResetKinds {
            rotation: true,
            ..ResetKinds::default()
        };
        assert!(rotation_only.rotation_only());
        assert!(!rotation_only.needs_bundle_worker());
        assert!(
            !BulkPageEditOp::Reset {
                kinds: rotation_only
            }
            .needs_bundle_worker()
        );
        assert_eq!(apply_reset_kinds(bundle.clone(), rotation_only), bundle);
    }

    #[test]
    fn per_kind_counts_include_mixed_and_unedited_targets() {
        let presences = [
            TargetEditPresence {
                adjustment: true,
                local_adjust: true,
                rotation: true,
                ..TargetEditPresence::default()
            },
            TargetEditPresence {
                mask: true,
                conceal: true,
                comic: true,
                export_crop: true,
                ..TargetEditPresence::default()
            },
            TargetEditPresence::default(),
        ];
        assert_eq!(
            count_reset_kinds(&presences),
            ResetKindCounts {
                adjustment: 1,
                mask: 1,
                conceal: 1,
                local_adjust: 1,
                comic: 1,
                export_crop: 1,
                rotation: 1,
            }
        );
        assert_eq!(presences.len(), 3, "編集なしの対象も総対象数に残る");
    }

    #[test]
    fn partial_reset_keeps_newer_in_memory_comic_and_adjustment() {
        let mut app = crate::app::setup_app_for_test();
        let page_path = app.tmp.path().join("page.png");
        app.items = vec![GridItem::Image(page_path)];
        let key = app.page_path_key(0).unwrap();
        let comic = |id, text: &str| {
            vec![comic_core::AnnotationObject::new_text(
                id,
                (10.0, 20.0),
                comic_core::TextBlock {
                    text: text.to_string(),
                    ..comic_core::TextBlock::default()
                },
            )]
        };
        let stale_db_comic = comic(1, "DB");
        let newer_memory_comic = comic(2, "memory");
        let mut stale_db_adjustment = crate::adjustment::AdjustParams::default();
        stale_db_adjustment.brightness = 0.1;
        let mut newer_memory_adjustment = stale_db_adjustment.clone();
        newer_memory_adjustment.brightness = 0.25;
        let paths = EditBundleDbPaths::default_data_dir();
        PageEditBundle {
            source_size: [1, 1],
            adjust: Some(stale_db_adjustment),
            mask: Some(EditMaskSnapshot {
                pixels: vec![true],
                shapes: Vec::new(),
                size: [1, 1],
            }),
            comic: Some(stale_db_comic),
            ..PageEditBundle::default()
        }
        .prepare()
        .unwrap()
        .apply_atomic(&paths, &key)
        .unwrap();

        app.adjusted_page_keys.insert(key.clone());
        app.adjustment_page_params
            .insert(0, newer_memory_adjustment.clone());
        app.mask_page_keys.insert(key.clone());
        app.mask_pages.insert(0);
        app.comic_page_keys.insert(key.clone());
        app.comic_pages.insert(0);
        app.comic_docs
            .insert(key.clone(), newer_memory_comic.clone());

        let kinds = ResetKinds {
            mask: true,
            ..ResetKinds::default()
        };
        let op = BulkPageEditOp::Reset { kinds };
        let (mut targets, dropped_non_targets) = app.bulk_page_edit_targets(0);
        assert_eq!(dropped_non_targets, 0);
        assert_eq!(targets.len(), 1);
        app.snapshot_bulk_kept_overrides(&mut targets, &op);

        let readers = ResetBundleReaders::open(&paths, kinds).unwrap();
        let prepared = prepare_and_apply_target(&targets[0], &op, &paths, Ok(&readers)).unwrap();
        app.apply_bulk_worker_success(&targets[0], prepared, BulkRunEffect::Reset { kinds })
            .unwrap();

        assert_eq!(app.comic_docs.get(&key), Some(&newer_memory_comic));
        assert_eq!(
            app.adjustment_page_params.get(&0),
            Some(&newer_memory_adjustment)
        );
        assert_eq!(
            crate::comic_db::ComicDb::open_at(&paths.comic)
                .unwrap()
                .get(&key),
            Some(newer_memory_comic)
        );
        assert_eq!(
            crate::adjustment_db::AdjustmentDb::open_at(&paths.adjustment)
                .unwrap()
                .get_page_params(&key),
            Some(newer_memory_adjustment)
        );
    }

    /// 一括編集の結果は、**操作を出した viewer context** の runtime へ入る。
    ///
    /// pending は App-global で、この dialog は main と fullscreen の両方から poll される。
    /// 先に drain した側の bundle へ書いていたので、別ウィンドウを開いたまま一覧側で一括
    /// 貼付をすると、一覧側のキャッシュ・保持設定・undo が更新されないまま DB だけが
    /// 変わっていた (v3.5.0 レビュー F08)。**別ウィンドウを mount した状態で drain して**、
    /// 所有者側に入り、別ウィンドウ側が変わらないことを見る。
    #[test]
    #[cfg(windows)]
    fn a_bulk_result_lands_in_the_context_that_asked_for_it() {
        let egui_ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        app.items = vec![image("owner.png")];
        let owner = app.edit_request_owner_context();
        let key = app.page_path_key(0).expect("ページキー");

        let other = app.build_window_context_for_test(940, |ctx| {
            ctx.items = vec![image("other.png")];
        });

        let mut adjust = crate::adjustment::AdjustParams::default();
        adjust.brightness = 7.0;
        let (tx, rx) = mpsc::channel();
        tx.send(BulkWorkerEvent::Item {
            target: BulkPageEditTarget {
                idx: 0,
                page_key: key.clone(),
                display_label: "owner.png".to_string(),
                item: image("owner.png"),
                sidecar_coords: None,
                known_size: Some([1, 1]),
                kept_runtime_overrides: KeptRuntimeOverrides::default(),
            },
            result: Ok(crate::edit_bundle::PreparedPageEditBundle {
                source_size: [1, 1],
                adjust: Some(adjust),
                adjust_json: None,
                mask: None,
                conceal: None,
                local_adjust_layers: None,
                local_adjust_json: None,
                export_crop: None,
                comic: None,
                comic_json: None,
            }),
        })
        .expect("送れる");
        app.edit_bundle_bulk_pending = Some(BulkPageEditPending {
            phase: BulkPageEditPhase::Running(BulkPageEditRun {
                owner_context_id: owner,
                driver: BulkRunDriver::Worker { rx, thread: None },
                effect: BulkRunEffect::Paste,
                source_label: None,
                cancel: Arc::new(AtomicBool::new(false)),
                total: 1,
                completed: 0,
                succeeded: 0,
                failed: 0,
                dropped_non_targets: 0,
                first_failure: None,
            }),
        });

        // 別ウィンドウ側の pass が先に完了を drain する。
        app.with_owner_viewer_context(other, |mounted| {
            mounted.poll_bulk_page_edit(&egui_ctx);
        })
        .expect("別 context を mount できる");

        assert_eq!(
            app.adjustment_page_params.get(&0).map(|p| p.brightness),
            Some(7.0),
            "所有者の runtime に入る"
        );
        let other_params = app
            .with_owner_viewer_context(other, |ctx| {
                ctx.adjustment_page_params.get(&0).map(|p| p.brightness)
            })
            .expect("別 context を mount できる");
        assert_eq!(other_params, None, "別ウィンドウの bundle は変えない");
    }

    fn run_for_summary(
        total: usize,
        completed: usize,
        succeeded: usize,
        failed: usize,
        dropped_non_targets: usize,
    ) -> BulkPageEditRun {
        BulkPageEditRun {
            owner_context_id: crate::app::ViewerContextId::for_test(0),
            driver: BulkRunDriver::RotationOnly {
                remaining: VecDeque::new(),
            },
            effect: BulkRunEffect::Paste,
            source_label: None,
            cancel: Arc::new(AtomicBool::new(false)),
            total,
            completed,
            succeeded,
            failed,
            dropped_non_targets,
            first_failure: None,
        }
    }

    /// 少数を選んだ一括処理は数フレームで終わるので、実機ではキャンセルを押せない。
    /// キャンセル時の文面はここでしか作られないため、この経路はテストで押さえる。
    #[test]
    fn a_cancelled_run_reports_what_was_left_and_says_the_applied_part_stays() {
        let run = run_for_summary(10, 4, 4, 0, 0);
        assert_eq!(
            bulk_summary_text(&run, true),
            "成功 4 件 / 未処理 6 件（キャンセルしました。適用済みの内容は保持されます）"
        );

        // 最後の 1 件を処理し終えた直後のキャンセルは、残りが無いので件数を出さない。
        // それでも「適用済みは残る」は言う: 押した側は中止できたかを知りたい。
        let finished = run_for_summary(3, 3, 3, 0, 0);
        assert_eq!(
            bulk_summary_text(&finished, true),
            "成功 3 件（キャンセルしました。適用済みの内容は保持されます）"
        );

        // キャンセルしなければ、未処理もキャンセル文言も出ない。
        assert_eq!(
            bulk_summary_text(&run_for_summary(2, 2, 2, 0, 0), false),
            "成功 2 件"
        );
    }

    /// 失敗と対象外は、キャンセルしたかどうかと独立に出る。
    #[test]
    fn failures_and_dropped_non_targets_are_reported_alongside_a_cancel() {
        let mut run = run_for_summary(9, 5, 3, 2, 4);
        run.first_failure = Some("b.jpg: 画像サイズを取得できませんでした".to_string());
        assert_eq!(
            bulk_summary_text(&run, true),
            "成功 3 件 / 失敗 2 件 / 対象外 4 件 / 未処理 4 件（キャンセルしました。\
適用済みの内容は保持されます）\n最初の失敗: b.jpg: 画像サイズを取得できませんでした"
        );
    }

    /// キャンセルは「次の対象を始めない」であって「進行中を捨てる」ではない。捨てると
    /// DB へ commit 済みの結果が runtime / sidecar へ反映されないまま乖離する。
    ///
    /// 実際の worker を回す。bounded channel (容量 1) のおかげで、受信を始める前に
    /// cancel を立てれば 3 件目の判定は必ず cancel を見る: worker は 1 件目を buffer へ
    /// 置き、2 件目の send で待たされるので、こちらが 2 件受け取るまで 3 件目へ進めない。
    /// 各 item の適用自体は失敗してよい。ここで見るのは「届くか」と「止まるか」だけ。
    #[test]
    fn cancelling_stops_before_the_next_target_and_still_delivers_the_ones_in_flight() {
        let _app = crate::app::setup_app_for_test();
        let targets: Vec<BulkPageEditTarget> = ["a.png", "b.png", "c.png"]
            .into_iter()
            .enumerate()
            .map(|(idx, name)| BulkPageEditTarget {
                idx,
                page_key: name.to_string(),
                display_label: name.to_string(),
                item: image(name),
                sidecar_coords: None,
                known_size: Some([1, 1]),
                kept_runtime_overrides: KeptRuntimeOverrides::default(),
            })
            .collect();

        let (cancel, rx, thread) = spawn_bulk_worker(
            targets,
            BulkPageEditOp::Reset {
                kinds: ResetKinds::all(),
            },
        )
        .expect("worker starts");
        // 1 件も受け取らないうちに押す = 進捗ウィンドウのキャンセル相当。
        cancel.store(true, Ordering::Relaxed);

        let mut delivered = Vec::new();
        let cancelled = drain_bulk_worker_events(&rx, |target, _result| {
            delivered.push(target.page_key);
        })
        .expect("worker reports how it ended");
        thread.join().unwrap();

        assert!(cancelled, "キャンセルとして終わったことを UI へ伝える");
        assert!(
            !delivered.contains(&"c.png".to_string()),
            "cancel 後の対象は始めない: {delivered:?}"
        );
        assert!(
            delivered
                .iter()
                .all(|key| ["a.png", "b.png"].contains(&key.as_str())),
            "届くのは cancel より前に着手した分だけ: {delivered:?}"
        );
    }

    #[test]
    fn edit_bundle_bulk_exit_drain_receives_item_before_bounded_worker_finishes() {
        let (tx, rx) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let target = BulkPageEditTarget {
                idx: 0,
                page_key: "page".to_string(),
                display_label: "page.png".to_string(),
                item: image("page.png"),
                sidecar_coords: None,
                known_size: Some([1, 1]),
                kept_runtime_overrides: KeptRuntimeOverrides::default(),
            };
            let prepared = PageEditBundle {
                source_size: [1, 1],
                adjust: None,
                mask: None,
                conceal: None,
                local_adjust_layers: None,
                export_crop: None,
                comic: None,
            }
            .prepare()
            .unwrap();
            tx.send(BulkWorkerEvent::Item {
                target,
                result: Ok(prepared),
            })
            .unwrap();
            tx.send(BulkWorkerEvent::Finished { cancelled: true })
                .unwrap();
        });

        let mut seen_keys = Vec::new();
        let cancelled = drain_bulk_worker_events(&rx, |target, result| {
            assert!(result.is_ok());
            seen_keys.push(target.page_key);
        })
        .unwrap();
        worker.join().unwrap();

        assert!(cancelled);
        assert_eq!(seen_keys, ["page"]);
    }

    #[test]
    fn partial_reset_keeps_unselected_rows_without_a_source_size() {
        let dir = tempfile::tempdir().unwrap();
        let paths = EditBundleDbPaths::in_dir(dir.path());
        let _ = crate::adjustment_db::AdjustmentDb::open_at(&paths.adjustment).unwrap();
        let _ = crate::mask_db::MaskDb::open_at(&paths.mask).unwrap();
        let _ = crate::conceal_db::ConcealDb::open_at(&paths.conceal).unwrap();
        let _ = crate::local_adjust_db::LocalAdjustDb::open_at(&paths.local_adjust).unwrap();
        let _ = crate::export_crop::CropDb::open_at(&paths.export_crop).unwrap();
        let _ = crate::comic_db::ComicDb::open_at(&paths.comic).unwrap();

        let key = "page";
        let mut adjust = crate::adjustment::AdjustParams::default();
        adjust.brightness = 0.25;
        let source = PageEditBundle {
            source_size: [2, 2],
            adjust: Some(adjust.clone()),
            mask: Some(EditMaskSnapshot {
                pixels: vec![true, false, false, true],
                shapes: Vec::new(),
                size: [2, 2],
            }),
            ..PageEditBundle::default()
        };
        source.prepare().unwrap().apply_atomic(&paths, key).unwrap();

        let kinds = ResetKinds {
            mask: true,
            ..ResetKinds::default()
        };
        let readers = ResetBundleReaders::open(&paths, kinds).unwrap();
        let reset = apply_reset_kinds(
            readers
                .load_kept_bundle(key, &KeptRuntimeOverrides::default())
                .unwrap(),
            kinds,
        );
        assert_eq!(reset.source_size, [0, 0]);
        let prepared = reset.prepare().unwrap();
        prepared.apply_atomic(&paths, key).unwrap();

        assert_eq!(
            crate::adjustment_db::AdjustmentDb::open_at(&paths.adjustment)
                .unwrap()
                .get_page_params(key),
            Some(adjust)
        );
        assert!(
            crate::mask_db::MaskDb::open_at(&paths.mask)
                .unwrap()
                .get_full_checked(key)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn partial_reset_uses_the_in_memory_local_adjust_override() {
        let dir = tempfile::tempdir().unwrap();
        let paths = EditBundleDbPaths::in_dir(dir.path());
        let _ = crate::adjustment_db::AdjustmentDb::open_at(&paths.adjustment).unwrap();
        let _ = crate::mask_db::MaskDb::open_at(&paths.mask).unwrap();
        let _ = crate::conceal_db::ConcealDb::open_at(&paths.conceal).unwrap();
        let _ = crate::local_adjust_db::LocalAdjustDb::open_at(&paths.local_adjust).unwrap();
        let _ = crate::export_crop::CropDb::open_at(&paths.export_crop).unwrap();
        let _ = crate::comic_db::ComicDb::open_at(&paths.comic).unwrap();

        let layers = |name: &str| {
            local_adjust_core::LocalAdjustmentLayers::new(vec![
                local_adjust_core::LocalAdjustmentLayer::new(
                    name,
                    local_adjust_core::LocalMask::Full,
                    local_adjust_core::LocalEffect::None,
                ),
            ])
        };
        let key = "page";
        PageEditBundle {
            source_size: [1, 1],
            mask: Some(EditMaskSnapshot {
                pixels: vec![true],
                shapes: Vec::new(),
                size: [1, 1],
            }),
            local_adjust_layers: Some(layers("stale DB")),
            ..PageEditBundle::default()
        }
        .prepare()
        .unwrap()
        .apply_atomic(&paths, key)
        .unwrap();

        let memory_layers = layers("screen state");
        let kinds = ResetKinds {
            mask: true,
            ..ResetKinds::default()
        };
        let readers = ResetBundleReaders::open(&paths, kinds).unwrap();
        let overrides = KeptRuntimeOverrides {
            local_adjust: KeptRuntimeValue::Present(memory_layers.clone()),
            ..KeptRuntimeOverrides::default()
        };
        apply_reset_kinds(readers.load_kept_bundle(key, &overrides).unwrap(), kinds)
            .prepare()
            .unwrap()
            .apply_atomic(&paths, key)
            .unwrap();

        assert_eq!(
            crate::local_adjust_db::LocalAdjustDb::open_at(&paths.local_adjust)
                .unwrap()
                .get_layers(key)
                .unwrap()
                .as_slice(),
            memory_layers.as_slice()
        );
    }
}
