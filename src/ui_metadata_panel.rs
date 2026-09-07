//! フルスクリーンのメタデータサイドパネル。
//!
//! AI 画像生成メタデータ (A1111/ComfyUI) と EXIF 撮影情報を右サイドパネルに表示する。

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::app::App;
use crate::exif_reader::{self, ExifInfo};
use crate::png_metadata::{A1111Metadata, AiMetadata, ComfyUIMetadata};
use crate::tag_ops::TagTarget;
use crate::xmp_reader::{self, XmpTweetInfo};

/// パネルタイトルバーの高さ
const TITLE_BAR_H: f32 = 32.0;
const LINK_COLOR: egui::Color32 = egui::Color32::from_rgb(115, 180, 255);
/// スクロール領域の内容幅を、**バーの有無にかかわらず**同じにする。
///
/// egui は非 floating のバーを出すときだけ溝を取る。内容量が変わるたびにパネルの幅が動き、
/// タブや本の帯が伸び縮みして読みづらい。常時表示にすれば幅は揃うが、今度は使わない灰色の
/// 溝がずっと見える。**溝の分だけ内容を狭めておく**と、バーが出る側では egui の確保と一致し、
/// 出ない側ではただの余白になる。
fn metadata_scroll_content_width(ui: &egui::Ui, outer_width: f32) -> f32 {
    let scroll = &ui.spacing().scroll;
    (outer_width - (scroll.bar_width + scroll.bar_inner_margin)).max(1.0)
}

const SIMILAR_THUMB_SIZE: f32 = 72.0;

/// パネルが抱えるサムネイルの上限。1 冊分の帯とその候補が丸ごと収まる程度にする。
/// 越えた分は古い順に落とす。
const SIMILAR_THUMB_CACHE_LIMIT: usize = 512;
/// Similar-panel reads can include ZIP enumeration and PDF rendering. Keep both newly-started
/// work and cancelled work that is still draining inside one fixed budget per viewer context.
const SIMILAR_THUMB_WORKER_LIMIT: usize = 4;
type SimilarThumbDemand = std::collections::HashMap<String, u64>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MetadataPanelTab {
    #[default]
    Info,
    Similar,
}

struct SimilarThumbResult {
    request_id: u64,
    item_key: String,
    image: Option<egui::ColorImage>,
}

#[derive(Clone)]
enum SimilarThumbState {
    Loading,
    Ready(egui::TextureHandle),
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SimilarThumbStamp {
    target: Option<crate::similar_index::SimilarItemTarget>,
    mtime: i64,
    file_size: i64,
    thumb_px: u32,
    thumb_quality: u8,
    /// Password changes must retry an otherwise unchanged PDF. This is a revision counter only;
    /// password material is never part of the cache key.
    pdf_credential_revision: u64,
}

struct SimilarThumbEntry {
    request_id: u64,
    stamp: SimilarThumbStamp,
    state: SimilarThumbEntryState,
}

enum SimilarThumbEntryState {
    Pending {
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    },
    Ready(egui::TextureHandle),
    Failed,
}

struct SimilarThumbJob {
    request_id: u64,
    item_key: String,
    target: crate::similar_index::SimilarItemTarget,
    mtime: i64,
    file_size: i64,
    thumb_px: u32,
    thumb_quality: u8,
    cache_decision: crate::thumb_loader::CacheDecision,
    pdf_passwords: Option<crate::pdf_passwords::PdfPasswordStore>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

pub(crate) struct SimilarPanelState {
    tab: MetadataPanelTab,
    origin_key: Option<String>,
    thumbnails: std::collections::HashMap<String, SimilarThumbEntry>,
    /// 受け付けた順。上限を超えた分をここから古い順に落とす。
    thumb_order: std::collections::VecDeque<String>,
    thumb_jobs: std::collections::VecDeque<SimilarThumbJob>,
    thumb_completions: std::collections::VecDeque<SimilarThumbResult>,
    thumb_running: usize,
    thumb_next_request_id: u64,
    thumb_last_upload_frame: Option<u64>,
    /// 直前に出せた結果。次の照会が返るまでこれを出し続ける。**表示している面ごとに 1 つ**
    /// (見開きなら左右で 2 つ)。
    ///
    /// 本を読み進めると起点はページごとに変わり、そのたびに照会が走る。返るまでの数フレーム
    /// を spinner に差し替えると、ページを送るたびに内容が消えて戻る。**古い内容を出したまま
    /// 差し替える方が読める。** 出している間は「更新中」と明記する。
    last_ready: Vec<Option<std::sync::Arc<crate::similar_index::ItemQuery>>>,
    thumb_tx: std::sync::mpsc::Sender<SimilarThumbResult>,
    thumb_rx: std::sync::mpsc::Receiver<SimilarThumbResult>,
}

impl Default for SimilarPanelState {
    fn default() -> Self {
        let (thumb_tx, thumb_rx) = std::sync::mpsc::channel();
        Self {
            tab: MetadataPanelTab::Info,
            origin_key: None,
            thumbnails: std::collections::HashMap::new(),
            thumb_order: std::collections::VecDeque::new(),
            thumb_jobs: std::collections::VecDeque::new(),
            thumb_completions: std::collections::VecDeque::new(),
            thumb_running: 0,
            thumb_next_request_id: 1,
            thumb_last_upload_frame: None,
            last_ready: Vec::new(),
            thumb_tx,
            thumb_rx,
        }
    }
}

impl SimilarPanelState {
    /// 起点が変わっても**サムネイルは捨てない**。
    ///
    /// 本を読み進めると起点は 1 ページごとに変わるが、候補も帯のページも同じ画像を指し続ける
    /// ことが多い。毎回捨てると、パネル全体が読み直しになって明滅する。上限を超えた分だけ
    /// 古い順に落とす。
    fn begin_origin(&mut self, origin_key: Option<String>) {
        if self.origin_key == origin_key {
            return;
        }
        self.origin_key = origin_key;
    }

    fn begin_thumbnail_frame(&mut self) {
        self.collect_thumbnail_results();
    }

    fn finish_thumbnail_frame(&mut self, ctx: &egui::Context, demand: &SimilarThumbDemand) {
        let changed = self.apply_thumbnail_completions(ctx, demand).1;
        self.dispatch_thumbnail_jobs(ctx, demand);
        if changed
            || self
                .thumb_completions
                .iter()
                .any(|result| demand.get(&result.item_key).copied() == Some(result.request_id))
        {
            ctx.request_repaint();
        }
    }

    fn collect_thumbnail_results(&mut self) {
        while let Ok(result) = self.thumb_rx.try_recv() {
            self.thumb_running = self.thumb_running.saturating_sub(1);
            self.thumb_completions.push_back(result);
        }
    }

    /// Apply terminal worker results for the currently visible requests. Stale identities drain;
    /// valid offscreen results stay on the CPU side until that card becomes visible again.
    fn apply_thumbnail_completions(
        &mut self,
        ctx: &egui::Context,
        demand: &SimilarThumbDemand,
    ) -> (usize, bool) {
        let frame_nr = ctx.cumulative_frame_nr();
        let may_upload = self.thumb_last_upload_frame != Some(frame_nr);
        let mut uploaded = 0;
        let mut changed = false;
        let mut deferred = std::collections::VecDeque::new();
        while let Some(result) = self.thumb_completions.pop_front() {
            let Some(entry) = self.thumbnails.get_mut(&result.item_key) else {
                continue;
            };
            if entry.request_id != result.request_id {
                continue;
            }
            if demand.get(&result.item_key).copied() != Some(result.request_id) {
                deferred.push_back(result);
                continue;
            }
            match result.image {
                Some(image) if may_upload && uploaded == 0 => {
                    entry.state = SimilarThumbEntryState::Ready(ctx.load_texture(
                        format!("similar-thumb:{}:{}", result.item_key, result.request_id),
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                    uploaded = 1;
                    changed = true;
                    self.thumb_last_upload_frame = Some(frame_nr);
                }
                Some(image) => deferred.push_back(SimilarThumbResult {
                    request_id: result.request_id,
                    item_key: result.item_key,
                    image: Some(image),
                }),
                None => {
                    entry.state = SimilarThumbEntryState::Failed;
                    changed = true;
                }
            }
        }
        self.thumb_completions = deferred;
        (uploaded, changed)
    }

    fn dispatch_thumbnail_jobs(&mut self, ctx: &egui::Context, demand: &SimilarThumbDemand) {
        for job in self.take_thumbnail_jobs_for_dispatch(demand) {
            self.spawn_thumbnail_job(ctx, job);
        }
    }

    fn take_thumbnail_jobs_for_dispatch(
        &mut self,
        demand: &SimilarThumbDemand,
    ) -> Vec<SimilarThumbJob> {
        use std::sync::atomic::Ordering;

        let mut jobs = Vec::new();
        let queued = self.thumb_jobs.len();
        for _ in 0..queued {
            let Some(job) = self.thumb_jobs.pop_front() else {
                break;
            };
            let current = self
                .thumbnails
                .get(&job.item_key)
                .is_some_and(|entry| entry.request_id == job.request_id);
            if !current || job.cancel.load(Ordering::Relaxed) {
                continue;
            }
            let is_demanded = demand.get(&job.item_key).copied() == Some(job.request_id);
            if is_demanded && self.thumb_running + jobs.len() < SIMILAR_THUMB_WORKER_LIMIT {
                jobs.push(job);
            } else {
                self.thumb_jobs.push_back(job);
            }
        }
        self.thumb_running += jobs.len();
        jobs
    }

    fn spawn_thumbnail_job(&mut self, ctx: &egui::Context, job: SimilarThumbJob) {
        let item_key = job.item_key.clone();
        let request_id = job.request_id;
        let failed_key = item_key.clone();
        let result_tx = self.thumb_tx.clone();
        let repaint = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("similar-panel-thumb".to_string())
            .spawn(move || {
                let image = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    load_similar_thumbnail(job)
                }))
                .ok()
                .flatten();
                let _ = result_tx.send(SimilarThumbResult {
                    request_id,
                    item_key,
                    image,
                });
                repaint.request_repaint();
            });
        if spawned.is_err() {
            self.thumb_running = self.thumb_running.saturating_sub(1);
            if let Some(entry) = self.thumbnails.get_mut(&failed_key)
                && entry.request_id == request_id
            {
                entry.state = SimilarThumbEntryState::Failed;
            }
        }
    }

    fn ensure_thumbnail(
        &mut self,
        hit: &crate::similar_index::QueryHit,
        thumb_px: u32,
        thumb_quality: u8,
        cache_decision: crate::thumb_loader::CacheDecision,
        pdf_passwords: Option<&crate::pdf_passwords::PdfPasswordStore>,
        ctx: &egui::Context,
    ) {
        self.ensure_thumbnail_for(
            &hit.item_key,
            crate::similar_index::target_for_hit(hit).cloned(),
            hit.mtime,
            hit.file_size,
            thumb_px,
            thumb_quality,
            cache_decision,
            pdf_passwords,
            ctx,
        );
    }

    /// 場所が分かっている 1 件のサムネイルを用意する。
    ///
    /// 結果一覧の行と、ページ帯にホバーしたページが同じ経路を通る。別々に読むと、同じ画像を
    /// 2 回読み、キャッシュの当たり方も食い違う。
    #[allow(clippy::too_many_arguments)]
    fn ensure_thumbnail_for(
        &mut self,
        key: &str,
        target: Option<crate::similar_index::SimilarItemTarget>,
        mtime: i64,
        file_size: i64,
        thumb_px: u32,
        thumb_quality: u8,
        cache_decision: crate::thumb_loader::CacheDecision,
        pdf_passwords: Option<&crate::pdf_passwords::PdfPasswordStore>,
        ctx: &egui::Context,
    ) {
        use std::sync::atomic::AtomicBool;

        let pdf_credential_revision = match target.as_ref() {
            Some(crate::similar_index::SimilarItemTarget::PdfPage { pdf_path, .. }) => {
                pdf_passwords.map_or(0, |passwords| passwords.credential_revision(pdf_path))
            }
            _ => 0,
        };
        let stamp = SimilarThumbStamp {
            target: target.clone(),
            mtime,
            file_size,
            thumb_px,
            thumb_quality,
            pdf_credential_revision,
        };
        if self
            .thumbnails
            .get(key)
            .is_some_and(|entry| entry.stamp == stamp)
        {
            return;
        }

        self.remove_thumbnail_request(key);
        let request_id = self.thumb_next_request_id;
        self.thumb_next_request_id = self.thumb_next_request_id.wrapping_add(1).max(1);
        let cancel = target
            .as_ref()
            .map(|_| std::sync::Arc::new(AtomicBool::new(false)));
        self.thumbnails.insert(
            key.to_owned(),
            SimilarThumbEntry {
                request_id,
                stamp,
                state: if target.is_some() {
                    SimilarThumbEntryState::Pending {
                        cancel: cancel
                            .as_ref()
                            .expect("target requests always allocate cancellation ownership")
                            .clone(),
                    }
                } else {
                    SimilarThumbEntryState::Failed
                },
            },
        );
        self.thumb_order.push_back(key.to_owned());

        if let (Some(target), Some(cancel)) = (target, cancel) {
            self.thumb_jobs.push_back(SimilarThumbJob {
                request_id,
                item_key: key.to_owned(),
                target,
                mtime,
                file_size,
                thumb_px,
                thumb_quality,
                cache_decision,
                pdf_passwords: pdf_passwords.cloned(),
                cancel,
            });
        }

        while self.thumbnails.len() > SIMILAR_THUMB_CACHE_LIMIT {
            let Some(oldest) = self.thumb_order.front().cloned() else {
                break;
            };
            self.remove_thumbnail_request(&oldest);
        }
        debug_assert!(self.thumb_order.len() <= SIMILAR_THUMB_CACHE_LIMIT);
        debug_assert!(self.thumb_jobs.len() <= SIMILAR_THUMB_CACHE_LIMIT);
        // `finish_thumbnail_frame` is the single per-frame dispatch/upload owner. Starting work
        // here would let every row bypass the visible-demand and upload budgets.
        ctx.request_repaint();
    }

    fn thumbnail_state(&self, key: &str) -> Option<SimilarThumbState> {
        self.thumbnails.get(key).map(|entry| match &entry.state {
            SimilarThumbEntryState::Pending { .. } => SimilarThumbState::Loading,
            SimilarThumbEntryState::Ready(texture) => SimilarThumbState::Ready(texture.clone()),
            SimilarThumbEntryState::Failed => SimilarThumbState::Failed,
        })
    }

    fn mark_thumbnail_demand(&self, key: &str, demand: &mut SimilarThumbDemand) {
        if let Some(entry) = self.thumbnails.get(key) {
            demand.insert(key.to_owned(), entry.request_id);
        }
    }

    /// Snapshot fixtures need stable placeholders without starting filesystem workers.
    fn insert_failed_snapshot_thumbnail(
        &mut self,
        key: String,
        target: Option<crate::similar_index::SimilarItemTarget>,
        mtime: i64,
        file_size: i64,
        thumb_px: u32,
        thumb_quality: u8,
    ) {
        self.thumbnails.insert(
            key,
            SimilarThumbEntry {
                request_id: 0,
                stamp: SimilarThumbStamp {
                    target,
                    mtime,
                    file_size,
                    thumb_px,
                    thumb_quality,
                    pdf_credential_revision: 0,
                },
                state: SimilarThumbEntryState::Failed,
            },
        );
    }

    fn remove_thumbnail_request(&mut self, key: &str) {
        use std::sync::atomic::Ordering;

        if let Some(entry) = self.thumbnails.remove(key)
            && let SimilarThumbEntryState::Pending { cancel } = entry.state
        {
            cancel.store(true, Ordering::Relaxed);
        }
        self.thumb_order.retain(|queued| queued != key);
        self.thumb_jobs.retain(|job| job.item_key != key);
    }
}

impl Drop for SimilarPanelState {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;

        for entry in self.thumbnails.values() {
            if let SimilarThumbEntryState::Pending { cancel } = &entry.state {
                cancel.store(true, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
impl SimilarPanelState {
    pub(crate) fn set_context_marker_for_test(&mut self, marker: &str) {
        self.tab = MetadataPanelTab::Similar;
        self.origin_key = Some(marker.to_owned());
        self.last_ready = vec![Some(std::sync::Arc::new(
            crate::similar_index::ItemQuery::Failed(format!("ready:{marker}")),
        ))];
    }

    pub(crate) fn context_marker_for_test(&self) -> (bool, Option<String>, Option<String>) {
        let last_ready = self
            .last_ready
            .first()
            .and_then(Option::as_ref)
            .and_then(|query| match query.as_ref() {
                crate::similar_index::ItemQuery::Failed(marker) => Some(marker.clone()),
                _ => None,
            });
        (
            self.tab == MetadataPanelTab::Similar,
            self.origin_key.clone(),
            last_ready,
        )
    }

    /// Put one result on this state's own channel without starting a filesystem worker.
    pub(crate) fn queue_failed_completion_for_test(&mut self, key: &str) {
        let request_id = self.thumb_next_request_id;
        self.thumb_next_request_id = self.thumb_next_request_id.wrapping_add(1).max(1);
        self.thumbnails.insert(
            key.to_owned(),
            SimilarThumbEntry {
                request_id,
                stamp: SimilarThumbStamp {
                    target: None,
                    mtime: 0,
                    file_size: 0,
                    thumb_px: 1,
                    thumb_quality: 1,
                    pdf_credential_revision: 0,
                },
                state: SimilarThumbEntryState::Pending {
                    cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                },
            },
        );
        self.thumb_running += 1;
        self.thumb_tx
            .send(SimilarThumbResult {
                request_id,
                item_key: key.to_owned(),
                image: None,
            })
            .expect("test completion receiver remains owned by this state");
    }

    pub(crate) fn consume_completions_for_test(&mut self, ctx: &egui::Context) {
        self.collect_thumbnail_results();
        let demand = self
            .thumbnails
            .iter()
            .map(|(key, entry)| (key.clone(), entry.request_id))
            .collect();
        self.apply_thumbnail_completions(ctx, &demand);
    }

    pub(crate) fn thumbnail_failed_for_test(&self, key: &str) -> bool {
        matches!(self.thumbnail_state(key), Some(SimilarThumbState::Failed))
    }
}

fn load_similar_thumbnail(job: SimilarThumbJob) -> Option<egui::ColorImage> {
    use std::sync::atomic::Ordering;

    if job.cancel.load(Ordering::Relaxed) {
        return None;
    }
    let item_key = job.item_key.clone();
    let (path, zip_entry, pdf_page) = match job.target {
        crate::similar_index::SimilarItemTarget::File(path) => (path, None, None),
        crate::similar_index::SimilarItemTarget::PdfPage { pdf_path, page_num } => {
            (pdf_path, None, Some(page_num))
        }
        crate::similar_index::SimilarItemTarget::ZipPage {
            zip_path,
            entry_name: _,
        } => {
            // DB identity is normalized, while ZIP entry lookup is case-sensitive. Resolve the
            // actual entry spelling once inside the bounded worker.
            let resolved = crate::zip_loader::enumerate_image_entries_detailed(&zip_path)
                .ok()
                .and_then(|entries| {
                    entries.entries.into_iter().find(|entry| {
                        crate::similar_index::item_key_for_zip_page(&zip_path, &entry.entry_name)
                            == item_key
                    })
                })
                .map(|entry| entry.entry_name);
            (zip_path, Some(resolved?), None)
        }
    };
    if job.cancel.load(Ordering::Relaxed) {
        return None;
    }
    let pdf_password = pdf_page.and_then(|_| {
        job.pdf_passwords
            .as_ref()
            .and_then(|passwords| passwords.get(&path))
    });
    let (tx, rx) = std::sync::mpsc::channel();
    let request = crate::thumb_loader::LoadRequest {
        path,
        zip_entry,
        pdf_page,
        pdf_password,
        mtime: job.mtime,
        file_size: job.file_size,
        source_policy: crate::thumb_loader::LoadSourcePolicy::SourceOnly,
        priority: true,
        // Similar-panel requests own their cancellation token. They are deliberately outside
        // the App-global grid/PDF epoch, just like fullscreen per-viewer render work.
        context_epoch: 0,
        ..Default::default()
    };
    let cache_map = std::sync::RwLock::new(std::collections::HashMap::new());
    let generated = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stats = std::sync::Arc::new(std::sync::Mutex::new(crate::stats::ThumbStats::default()));
    let keep_start = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let keep_end = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(usize::MAX));
    crate::thumb_loader::process_load_request(
        &request,
        &cache_map,
        &tx,
        None,
        job.thumb_px,
        job.thumb_quality,
        job.thumb_px,
        job.cache_decision,
        &generated,
        &stats,
        Some(&job.cancel),
        &keep_start,
        &keep_end,
        None,
        None,
        None,
        None,
    );
    if job.cancel.load(Ordering::Relaxed) {
        return None;
    }
    rx.try_iter().find_map(|message| message.image)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SimilarPanelModel<'a> {
    NoIndex,
    Preparing,
    NotIndexed,
    Featureless,
    Empty,
    Results(&'a crate::similar_index::ItemMatches),
    Failed(&'a str),
}

/// パネルが扱う面。見開きなら**両方のページ**、単ページならそれだけ。
///
/// 見開きで片方しか調べないと、もう片方に別バージョンがあっても気付けない。補正パネルが
/// 左右を切り替えるのは編集が一度に 1 ページだからで、閲覧側にその制約はない。
fn similar_shown_indices(
    fullscreen_idx: Option<usize>,
    spread: crate::ui_fullscreen::SpreadPair,
) -> Vec<usize> {
    match (fullscreen_idx, spread) {
        (Some(_), crate::ui_fullscreen::SpreadPair::Double { left, right }) => vec![left, right],
        (Some(idx), crate::ui_fullscreen::SpreadPair::Single) => vec![idx],
        (None, _) => Vec::new(),
    }
}

/// パネルに並べる 1 面。見開きなら 2 つ、単ページなら 1 つ。
struct SimilarPageView<'a> {
    /// 見開きのときだけ付ける見出し。単ページでは面が 1 つしかないので付けない。
    heading: Option<&'static str>,
    item_key: Option<&'a str>,
    model: SimilarPanelModel<'a>,
    showing_previous: bool,
}

#[derive(Default)]
struct SimilarPanelActions {
    open_favorites: bool,
    open_hit: Option<crate::similar_index::QueryHit>,
    pin_hit: Option<crate::similar_index::QueryHit>,
    /// このフレームで「長押し表示」ボタンが押されたままの候補。押していないフレームは
    /// `None` になり、消費側が覗き見の終了を判定する。
    peek_held: Option<crate::similar_index::QueryHit>,
    /// ページ帯から選ばれた相手の本のページ。
    open_page: Option<(String, crate::similar_index::SimilarItemTarget)>,
    /// このフレームで実際に clip 内へ描いたサムネイル request。worker dispatch と GPU
    /// upload はこの需要を優先し、スクロール外の旧要求で新しい表示を待たせない。
    thumbnail_demand: SimilarThumbDemand,
}

/// 比較スロットから見た 1 件の状態。ボタンの見え方を決める。
#[derive(Clone, Copy, PartialEq, Eq)]
enum SimilarCompareState {
    /// この候補が比較画像として設定されている。
    Pinned,
    /// 比較画像を準備中 (この候補とは限らない)。
    Preparing,
    Idle,
}

fn similar_panel_model(query: &crate::similar_index::ItemQuery) -> SimilarPanelModel<'_> {
    match query {
        crate::similar_index::ItemQuery::NoIndex => SimilarPanelModel::NoIndex,
        crate::similar_index::ItemQuery::Preparing => SimilarPanelModel::Preparing,
        crate::similar_index::ItemQuery::NotIndexed => SimilarPanelModel::NotIndexed,
        crate::similar_index::ItemQuery::Featureless => SimilarPanelModel::Featureless,
        crate::similar_index::ItemQuery::Ready(matches) if matches.hits.is_empty() => {
            SimilarPanelModel::Empty
        }
        crate::similar_index::ItemQuery::Ready(matches) => SimilarPanelModel::Results(matches),
        crate::similar_index::ItemQuery::Failed(error) => SimilarPanelModel::Failed(error),
    }
}

/// 表示中のページの要約。候補の行と同じ並び (寸法 / 形式 / 大きさ) にする。**ここだけ別の
/// 書式にすると見比べられない。**
fn similar_origin_line(origin: &crate::similar_index::OriginItem) -> String {
    let size = crate::ui_helpers::format_bytes_small(origin.file_size.max(0) as u64);
    let size = match origin.kind {
        crate::similar_db::ItemKind::Image => size,
        crate::similar_db::ItemKind::ZipPage => format!("書庫 {size}"),
        crate::similar_db::ItemKind::PdfPage => format!("PDF {size}"),
    };
    format!(
        "{}×{} / {} / {}",
        origin.width,
        origin.height,
        origin.format.display_name(),
        size
    )
}

fn similar_difference_line(
    hit: &crate::similar_index::QueryHit,
    origin: &crate::similar_index::OriginItem,
) -> String {
    let size = crate::ui_helpers::format_bytes_small(hit.file_size.max(0) as u64);
    let size = match hit.kind {
        crate::similar_db::ItemKind::Image => size,
        crate::similar_db::ItemKind::ZipPage => format!("書庫 {size}"),
        crate::similar_db::ItemKind::PdfPage => format!("PDF {size}"),
    };
    format!(
        "{}×{} (この画像は {}×{}) / {} / {}",
        hit.width,
        hit.height,
        origin.width,
        origin.height,
        hit.format.display_name(),
        size
    )
}

fn similar_location_line(hit: &crate::similar_index::QueryHit) -> String {
    let Some(target) = crate::similar_index::target_for_hit(hit) else {
        return hit.item_key.clone();
    };
    match target {
        crate::similar_index::SimilarItemTarget::File(path) => {
            let file_name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            path.parent().map_or(file_name.clone(), |parent| {
                format!(
                    "{} / {}",
                    file_name,
                    crate::ui_dialogs::context_menu::native_path_text(parent)
                )
            })
        }
        crate::similar_index::SimilarItemTarget::ZipPage {
            zip_path,
            entry_name,
        } => format!("{} / {}", zip_path.display(), entry_name),
        crate::similar_index::SimilarItemTarget::PdfPage { pdf_path, page_num } => {
            format!("{} / Page {}", pdf_path.display(), page_num + 1)
        }
    }
}

/// 場所を「名前」と「その上のたどり」に分ける。
///
/// 差分の強調はこの単位で行う。パスを 1 本の文字列として比べると、違うのが何段目なのかを
/// 読み取れない。
struct SimilarPathParts {
    name: String,
    segments: Vec<String>,
}

fn similar_path_parts(
    target: Option<&crate::similar_index::SimilarItemTarget>,
    fallback_key: &str,
) -> SimilarPathParts {
    let split = |text: &str| {
        text.split(['/', '\\'])
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    match target {
        Some(crate::similar_index::SimilarItemTarget::File(path)) => SimilarPathParts {
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            segments: path
                .parent()
                .map(|parent| split(&crate::ui_dialogs::context_menu::native_path_text(parent)))
                .unwrap_or_default(),
        },
        Some(crate::similar_index::SimilarItemTarget::ZipPage {
            zip_path,
            entry_name,
        }) => SimilarPathParts {
            name: entry_name.clone(),
            segments: split(&crate::ui_dialogs::context_menu::native_path_text(zip_path)),
        },
        Some(crate::similar_index::SimilarItemTarget::PdfPage { pdf_path, page_num }) => {
            SimilarPathParts {
                name: format!("{} ページ目", page_num + 1),
                segments: split(&crate::ui_dialogs::context_menu::native_path_text(pdf_path)),
            }
        }
        None => {
            let mut segments = split(fallback_key);
            let name = segments.pop().unwrap_or_default();
            SimilarPathParts { name, segments }
        }
    }
}

/// 起点と比べて、`other` のどの区切りが違うかを返す。
///
/// 共通の頭と尻を残し、中間だけを違いとする。階層の深さが違うときに、位置がずれた共通部分を
/// 全部「違う」と塗ってしまわないため。
fn path_segment_diff(origin: &[String], other: &[String]) -> Vec<bool> {
    let mut differs = vec![true; other.len()];
    let mut head = 0;
    while head < origin.len()
        && head < other.len()
        && origin[head].eq_ignore_ascii_case(&other[head])
    {
        differs[head] = false;
        head += 1;
    }
    let mut tail = 0;
    while tail < origin.len().saturating_sub(head)
        && tail < other.len().saturating_sub(head)
        && origin[origin.len() - 1 - tail].eq_ignore_ascii_case(&other[other.len() - 1 - tail])
    {
        differs[other.len() - 1 - tail] = false;
        tail += 1;
    }
    differs
}

/// 違う部分の色。**警告ではない** — どちらが正しいという話ではなく、見比べる手がかり。
const PATH_DIFF_COLOR: egui::Color32 = egui::Color32::from_rgb(226, 183, 106);

fn path_layout_job(
    parts: &SimilarPathParts,
    origin: Option<&SimilarPathParts>,
    wrap_width: f32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap_width;
    let font = egui::FontId::proportional(10.0);
    let name_differs = origin.is_some_and(|origin| !origin.name.eq_ignore_ascii_case(&parts.name));
    job.append(
        &parts.name,
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color: if name_differs {
                PATH_DIFF_COLOR
            } else {
                DIM_COLOR
            },
            ..Default::default()
        },
    );
    if parts.segments.is_empty() {
        return job;
    }
    let differs = origin
        .map(|origin| path_segment_diff(&origin.segments, &parts.segments))
        .unwrap_or_else(|| vec![false; parts.segments.len()]);
    job.append(
        "\n",
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color: DIM_COLOR,
            ..Default::default()
        },
    );
    for (index, segment) in parts.segments.iter().enumerate() {
        if index > 0 {
            job.append(
                "\\",
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color: DIM_COLOR,
                    ..Default::default()
                },
            );
        }
        job.append(
            segment,
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color: if differs[index] {
                    PATH_DIFF_COLOR
                } else {
                    DIM_COLOR
                },
                ..Default::default()
            },
        );
    }
    job
}

/// 1 件ぶんのカード。表示中のページも候補も同じ形で出す。
///
/// `origin` を渡すと、場所のうち起点と違う部分だけ色を変える。同じ形で並べたうえで違いだけを
/// 目立たせるのが目的なので、片方だけ別のレイアウトにしない。
fn draw_similar_card(
    ui: &mut egui::Ui,
    thumb: Option<SimilarThumbState>,
    info: &str,
    path: &SimilarPathParts,
    origin: Option<&SimilarPathParts>,
) -> egui::Response {
    egui::Frame::new()
        .fill(egui::Color32::from_rgba_unmultiplied(38, 40, 48, 210))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 28),
        ))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(SIMILAR_THUMB_SIZE, SIMILAR_THUMB_SIZE),
                    egui::Sense::hover(),
                );
                ui.painter()
                    .rect_filled(rect, 2.0, egui::Color32::from_rgb(25, 26, 31));
                match thumb {
                    Some(SimilarThumbState::Ready(texture)) => {
                        let image_size = texture.size_vec2();
                        let scale = (rect.width() / image_size.x).min(rect.height() / image_size.y);
                        let image_rect =
                            egui::Rect::from_center_size(rect.center(), image_size * scale);
                        ui.painter().image(
                            texture.id(),
                            image_rect,
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                    Some(SimilarThumbState::Loading) => {
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "読込中",
                            egui::FontId::proportional(10.0),
                            DIM_COLOR,
                        );
                    }
                    Some(SimilarThumbState::Failed) | None => {
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "画像なし",
                            egui::FontId::proportional(10.0),
                            DIM_COLOR,
                        );
                    }
                }
                ui.vertical(|ui| {
                    let width = (ui.available_width() - 2.0).max(20.0);
                    ui.set_max_width(width);
                    ui.label(egui::RichText::new(info).color(TEXT_COLOR).size(11.0));
                    ui.add_space(3.0);
                    ui.label(path_layout_job(path, origin, width));
                });
            });
        })
        .response
}

fn similar_copy_path_text(hit: &crate::similar_index::QueryHit) -> String {
    let Some(target) = crate::similar_index::target_for_hit(hit) else {
        return hit.item_key.clone();
    };
    match target {
        crate::similar_index::SimilarItemTarget::File(path) => {
            crate::ui_dialogs::context_menu::native_path_text(&path)
        }
        crate::similar_index::SimilarItemTarget::ZipPage {
            zip_path,
            entry_name,
        } => format!(
            "{}:{}",
            crate::ui_dialogs::context_menu::native_path_text(&zip_path),
            entry_name
        ),
        crate::similar_index::SimilarItemTarget::PdfPage { pdf_path, page_num } => format!(
            "{}:Page {}",
            crate::ui_dialogs::context_menu::native_path_text(&pdf_path),
            page_num + 1
        ),
    }
}

fn similar_open_target(
    hit: &crate::similar_index::QueryHit,
) -> Option<(PathBuf, crate::snapshot::SnapshotTarget)> {
    similar_open_location(crate::similar_index::target_for_hit(hit).cloned()?)
}

/// 移動先を「開く場所」と「その中のどれか」に分ける。
///
/// 単体画像の結果とページ帯は別の型から来るが、開き方は同じでなければならない。
fn similar_open_location(
    target: crate::similar_index::SimilarItemTarget,
) -> Option<(PathBuf, crate::snapshot::SnapshotTarget)> {
    match target {
        crate::similar_index::SimilarItemTarget::File(path) => Some((
            path.parent()?.to_path_buf(),
            crate::snapshot::SnapshotTarget::Fs(path),
        )),
        crate::similar_index::SimilarItemTarget::ZipPage {
            zip_path,
            entry_name,
        } => Some((
            zip_path.clone(),
            crate::snapshot::SnapshotTarget::ZipImage {
                zip_path,
                entry_name,
            },
        )),
        crate::similar_index::SimilarItemTarget::PdfPage { pdf_path, page_num } => Some((
            pdf_path.clone(),
            crate::snapshot::SnapshotTarget::PdfPage { pdf_path, page_num },
        )),
    }
}

fn prepare_similar_compare_result(
    hit: crate::similar_index::QueryHit,
    pdf_passwords: crate::pdf_passwords::PdfPasswordStore,
    pdf_viewport: crate::pdf_loader::PdfDisplayTarget,
) -> Result<crate::app::ComparePinResult, String> {
    let target = crate::similar_index::target_for_hit(&hit)
        .cloned()
        .ok_or_else(|| "比較画像の場所を解決できません".to_string())?;
    let (display_name, pixels) = match target {
        crate::similar_index::SimilarItemTarget::File(path) => {
            let display_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("?")
                .to_string();
            let decoded = crate::canonical_image_loader::decode_canonical_image(
                crate::canonical_image_loader::CanonicalImageSource::File {
                    path: &path,
                    verified_bytes: None,
                },
                crate::canonical_image_loader::CanonicalDecodeOptions::fullscreen(
                    crate::canonical_image_loader::AnimationPolicy::FirstFrameOnly,
                ),
            )
            .map_err(|error| error.to_string())?;
            let crate::canonical_image_loader::CanonicalImageDecode::Static(image) = decoded else {
                return Err("動画は比較画像に設定できません".to_string());
            };
            (display_name, image.into_gpu_raster().pixels)
        }
        crate::similar_index::SimilarItemTarget::ZipPage {
            zip_path,
            entry_name: _,
        } => {
            let entry_name = crate::zip_loader::enumerate_image_entries_detailed(&zip_path)
                .map_err(|error| error.to_string())?
                .entries
                .into_iter()
                .find(|entry| {
                    crate::similar_index::item_key_for_zip_page(&zip_path, &entry.entry_name)
                        == hit.item_key
                })
                .map(|entry| entry.entry_name)
                .ok_or_else(|| "ZIP内の比較画像が見つかりません".to_string())?;
            let display_name = crate::zip_loader::entry_basename(&entry_name).to_string();
            let decoded = crate::canonical_image_loader::decode_canonical_image(
                crate::canonical_image_loader::CanonicalImageSource::ArchiveEntry {
                    archive_path: &zip_path,
                    entry_name: &entry_name,
                },
                crate::canonical_image_loader::CanonicalDecodeOptions::fullscreen(
                    crate::canonical_image_loader::AnimationPolicy::FirstFrameOnly,
                ),
            )
            .map_err(|error| error.to_string())?;
            let crate::canonical_image_loader::CanonicalImageDecode::Static(image) = decoded else {
                return Err("動画は比較画像に設定できません".to_string());
            };
            (display_name, image.into_gpu_raster().pixels)
        }
        crate::similar_index::SimilarItemTarget::PdfPage { pdf_path, page_num } => {
            let pdf_password = pdf_passwords.get(&pdf_path);
            let display_name = format!(
                "{} - Page {}",
                pdf_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("PDF"),
                page_num + 1
            );
            let rendered = crate::pdf_loader::render_page_for_display(
                &pdf_path,
                page_num,
                pdf_viewport,
                false,
                pdf_password.as_deref(),
                None,
                crate::pdf_loader::JobPriority::Critical,
                crate::pdf_loader::current_render_context_epoch(),
                crate::pdf_loader::CancelWaitPolicy::AbortOnCancel,
            )
            .map_err(|error| error.to_string())?;
            let image = crate::canonical_image_loader::clamp_dynamic_for_gpu(rendered.image);
            (
                display_name,
                crate::canonical_image_loader::dynamic_image_to_color_image(&image),
            )
        }
    };
    crate::ui_fullscreen::prepare_compare_pin_result(
        crate::capture::CapturePixelJob::already_adjusted(
            display_name,
            std::sync::Arc::new(pixels),
        ),
    )
}

#[derive(Clone)]
struct TagPanelRow {
    label: Option<String>,
    note: Option<String>,
    targets: Vec<TagTarget>,
    tags_by_target: Vec<Vec<String>>,
}

#[derive(Clone)]
struct TagPanelChoice {
    name: String,
    tag_key: String,
    count: usize,
    pinned: bool,
    last_applied_at: i64,
}

impl TagPanelRow {
    fn disabled() -> Self {
        Self {
            label: None,
            note: None,
            targets: Vec::new(),
            tags_by_target: Vec::new(),
        }
    }
}

impl App {
    fn open_similar_hit(&mut self, hit: &crate::similar_index::QueryHit) {
        if let Some(index) = self.items.iter().position(|item| {
            crate::app::similar_index_item_key(item).as_deref() == Some(hit.item_key.as_str())
        }) {
            self.open_fullscreen(index, crate::app::HistoryTrigger::UserChosen);
            return;
        }
        let Some((location, target)) = similar_open_target(hit) else {
            self.show_feedback_toast("画像の場所を開けません".to_string());
            return;
        };
        if self.is_snapshot_active() {
            let _ = self.dismiss_snapshot_without_restore();
        }
        self.snapshot_load_and_open(
            location,
            false,
            Some(target),
            crate::app::HistoryTrigger::UserChosen,
        );
    }

    /// ページ帯から相手の本のページを開く。
    ///
    /// 単体画像の移動と同じ経路を通す。いま開いている一覧に無い場所でも開けるよう、
    /// 解決済みの移動先を使う。
    fn open_similar_book_page(
        &mut self,
        item_key: &str,
        target: crate::similar_index::SimilarItemTarget,
    ) {
        if let Some(index) = self
            .items
            .iter()
            .position(|item| crate::app::similar_index_item_key(item).as_deref() == Some(item_key))
        {
            self.open_fullscreen(index, crate::app::HistoryTrigger::UserChosen);
            return;
        }
        let Some((location, target)) = similar_open_location(target) else {
            self.show_feedback_toast("ページの場所を開けません".to_string());
            return;
        };
        if self.is_snapshot_active() {
            let _ = self.dismiss_snapshot_without_restore();
        }
        self.snapshot_load_and_open(
            location,
            false,
            Some(target),
            crate::app::HistoryTrigger::UserChosen,
        );
    }

    fn pin_similar_hit(
        &mut self,
        ctx: &egui::Context,
        hit: &crate::similar_index::QueryHit,
        full_rect: egui::Rect,
    ) {
        let pdf_passwords = self.pdf_passwords.clone();
        let viewport = self.fs_pdf_display_target.unwrap_or_else(|| {
            crate::pdf_loader::PdfDisplayTarget::from_logical_size(
                full_rect.width(),
                full_rect.height(),
                ctx.pixels_per_point(),
                crate::pdf_loader::PdfDisplayFitMode::Page,
            )
        });
        let job_hit = hit.clone();
        self.start_external_compare_pin_job(ctx, hit.item_key.clone(), move || {
            prepare_similar_compare_result(job_hit, pdf_passwords, viewport)
        });
    }

    /// 「長押し表示」の 1 フレームぶんの状態遷移。
    ///
    /// `held` はこのフレームでボタンが押されたままの候補。押している間は比較画像として
    /// 表示し、離したら元の表示に戻す。比較画像がまだその候補でない場合はここで設定を始め、
    /// 読み込めた次のフレームから表示へ切り替わる。押しっぱなしのまま設定が終わらなければ
    /// 何も映らないが、そのときも設定は残るので 2 回目は即座に映る。
    fn update_similar_peek(
        &mut self,
        ctx: &egui::Context,
        held: Option<crate::similar_index::QueryHit>,
        full_rect: egui::Rect,
    ) {
        let transition = decide_similar_peek(
            self.similar_peek
                .as_ref()
                .map(|peek| peek.item_key.as_str()),
            held.as_ref().map(|hit| hit.item_key.as_str()),
        );
        if matches!(transition, SimilarPeekTransition::Idle) {
            return;
        }
        if matches!(
            transition,
            SimilarPeekTransition::Stop | SimilarPeekTransition::Switch
        ) && let Some(peek) = self.similar_peek.take()
        {
            self.end_similar_peek(ctx, peek);
        }
        let Some(hit) = held else {
            return;
        };
        if matches!(
            transition,
            SimilarPeekTransition::Start | SimilarPeekTransition::Switch
        ) {
            self.similar_peek = Some(crate::app::SimilarPeek {
                item_key: hit.item_key.clone(),
                restore_mode: self.compare_view_mode,
            });
        }
        self.advance_similar_peek(ctx, &hit, full_rect);
    }

    /// 押されている間、毎フレーム呼ばれる。比較画像が揃っていなければ設定を始めるだけで、
    /// 揃っていれば表示へ入る。どちらの場合も同じ経路を通るので、待ち時間の有無で分岐しない。
    fn advance_similar_peek(
        &mut self,
        ctx: &egui::Context,
        hit: &crate::similar_index::QueryHit,
        full_rect: egui::Rect,
    ) {
        let pinned = self
            .pinned_compare_slot
            .as_ref()
            .and_then(|slot| slot.external_item_key.as_deref())
            == Some(hit.item_key.as_str());
        if !pinned {
            if self.compare_pin_pending.is_none() {
                self.pin_similar_hit(ctx, hit, full_rect);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            return;
        }
        if !matches!(
            self.compare_view_mode,
            crate::app::CompareViewMode::PinnedNormal
        ) {
            self.compare_view_mode = crate::app::CompareViewMode::PinnedNormal;
            self.compare_wipe_dragging = false;
            if let Some(fs_idx) = self.fullscreen_idx {
                self.clear_compare_gpu_pair();
                self.ensure_compare_prepared_pair(ctx, fs_idx);
            }
        }
    }

    /// 覗き見の終了。押す前の表示状態へ戻す。
    fn end_similar_peek(&mut self, ctx: &egui::Context, peek: crate::app::SimilarPeek) {
        if !matches!(
            self.compare_view_mode,
            crate::app::CompareViewMode::PinnedNormal
        ) {
            return;
        }
        match peek.restore_mode {
            crate::app::CompareViewMode::PinnedNormal => {}
            crate::app::CompareViewMode::Off => self.hide_compare_pinned_view(),
            other => {
                self.compare_view_mode = other;
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.ensure_compare_prepared_pair(ctx, fs_idx);
                }
            }
        }
    }

    /// Update the transient right-panel hover latch for the current fullscreen frame.
    /// Rendering reads this value but does not own its lifetime, so navigator-consumed image
    /// input cannot freeze the panel at the previous frame's state.
    pub(crate) fn update_metadata_panel_hover_latch(
        &mut self,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        chrome_enabled: bool,
        navigator_exclusion: Option<egui::Rect>,
        seek_height: f32,
    ) {
        let pointer_pos = ctx.input(|i| {
            if self.cursor_hidden() {
                None
            } else {
                i.pointer.hover_pos()
            }
        });
        let hover_mode = self.settings.fullscreen_side_panel_mode.normalized()
            == crate::settings::FsSidePanelMode::Hover;
        let explicit = self.metadata_panel_click_shown();
        let hover_visible = chrome_enabled
            && hover_mode
            && crate::ui_fullscreen::metadata_panel_hover_active_at_with_seek_height(
                full_rect,
                pointer_pos,
                self.fs_info_panel.hover_active,
                navigator_exclusion,
                seek_height,
            );
        self.fs_info_panel.hover_active = !explicit && hover_visible;
    }

    /// フルスクリーンで右情報パネルを描画する。
    ///
    /// ロック OFF では画像へ重ねる。ロック ON では画像側が右を空けているので重ならない
    /// (領域を空けるかどうかは `still_info_panel_lock_effective_for_idx` が決め、ここは
    /// 同じ答えを `lock_effective` として受け取る。両者が別々に条件を綴ると、パネルを
    /// 描かないモードで右に空白の帯が残る。backlog §1.158)。
    ///
    /// 表示条件:
    /// - 通常ホバー: マウスカーソルが画面右端にある間
    /// - 明示表示: mode gate 済みの pointer owner、または touch handle owner
    ///
    /// 右パネル表示中は上部バーも常に同時表示する。
    /// 右パネルは常に上部バーの下から開始する。
    ///
    /// 戻り値: 右パネルが表示中なら true（上部バーの強制表示に使う）
    pub(crate) fn draw_metadata_panel(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        lock_effective: bool,
        seek_height: f32,
    ) -> bool {
        self.draw_metadata_panel_inner(ui, ctx, full_rect, lock_effective, seek_height)
    }

    fn draw_metadata_panel_inner(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        lock_effective: bool,
        seek_height: f32,
    ) -> bool {
        let panel_rect =
            crate::ui_fullscreen::metadata_panel_rect_with_seek_height(full_rect, seek_height);

        // Most frames update this before image-input consumption in ui_fullscreen. Keep the draw
        // boundary idempotently current as well because capture-selection and other modal paths
        // can bypass the general image-input handler while this panel is still rendered.
        let navigator_exclusion = self.fullscreen_navigator_edge_exclusion(ctx, full_rect);
        self.update_metadata_panel_hover_latch(
            ctx,
            full_rect,
            !self.still_touch_chrome_is_latched(ctx),
            navigator_exclusion,
            seek_height,
        );
        // 表示するかの答えは `FullscreenInfoPanelState` だけが出す。ロックは実効値を使う。
        let mut panel_state = self.fs_info_panel;
        panel_state.locked = lock_effective;
        let explicit = panel_state.explicit_shown(self.settings.fullscreen_side_panel_mode);
        if !panel_state.visible(
            self.settings.fullscreen_side_panel_mode,
            self.fullscreen_tag_picker_open,
        ) {
            return false;
        }

        // パネル背景
        ui.painter().rect_filled(
            panel_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(18, 18, 22, 230),
        );
        // 左端に区切り線
        ui.painter().line_segment(
            [panel_rect.left_top(), panel_rect.left_bottom()],
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40),
            ),
        );

        // パネルのクリックイベントを消費
        let _ = ui.interact(
            panel_rect,
            egui::Id::new("metadata_panel_bg"),
            egui::Sense::click(),
        );

        // ── タイトルバー (ピン留めボタン付き) ──
        let title_rect =
            egui::Rect::from_min_size(panel_rect.min, egui::vec2(panel_rect.width(), TITLE_BAR_H));
        // タイトルバー背景 (やや明るめ)
        ui.painter().rect_filled(
            title_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(30, 30, 38, 240),
        );
        // 下端の区切り線
        ui.painter().line_segment(
            [
                egui::pos2(title_rect.min.x, title_rect.max.y),
                egui::pos2(title_rect.max.x, title_rect.max.y),
            ],
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 30),
            ),
        );

        // タイトルテキスト
        ui.painter().text(
            egui::pos2(title_rect.min.x + 10.0, title_rect.center().y),
            egui::Align2::LEFT_CENTER,
            "Image Info",
            egui::FontId::proportional(13.0),
            egui::Color32::from_gray(200),
        );

        // 鍵ボタン。ON では画像へ重ねず、右にパネル幅の領域を確保する (backlog §1.158)。
        // 閉じるボタンの左隣に置き、ロック中は閉じるボタンを出さない (閉じるのは解錠が先)。
        let button_size = 22.0;
        let button_margin = 5.0;
        let lock_rect = egui::Rect::from_min_size(
            egui::pos2(
                title_rect.max.x - button_size - button_margin,
                title_rect.min.y + (TITLE_BAR_H - button_size) * 0.5,
            ),
            egui::vec2(button_size, button_size),
        );
        let lock_resp = ui.interact(
            lock_rect,
            egui::Id::new("metadata_lock_btn"),
            egui::Sense::click(),
        );
        let locked_now = self.fs_info_panel.locked;
        let lock_bg = if locked_now {
            egui::Color32::from_rgba_unmultiplied(90, 110, 150, 220)
        } else if lock_resp.hovered() {
            egui::Color32::from_rgba_unmultiplied(100, 100, 100, 200)
        } else {
            egui::Color32::TRANSPARENT
        };
        ui.painter().rect_filled(lock_rect, 3.0, lock_bg);
        // 鍵の形は静止画・動画の固定バーと**同じベクター**を使う。ここで描き直さない
        // (docs/video-architecture.md「各バーとシークストリップには固定状態を示す鍵ボタン」)。
        crate::ui_fullscreen::draw_icons::draw_seek_lock_icon(
            ui.painter(),
            lock_rect.center(),
            lock_rect.width() * 0.34,
            locked_now,
        );
        let lock_hint = if locked_now {
            "固定を解除 (画像へ重ねる一時表示に戻す)"
        } else {
            "パネルを固定 (画像に重ねず、前後へ移動しても表示したまま)"
        };
        if lock_resp.on_hover_text(lock_hint).clicked() {
            self.fs_info_panel.locked = !locked_now;
            if !self.fs_info_panel.locked {
                // 解錠したら、その場の明示 open として残す (パネルが消えると操作を見失う)。
                self.fs_info_panel.open = crate::ui_helpers::MetadataPanelOpenState::ByPointer;
            }
        }

        // pointer / touch handle で明示的に開いた右パネルを閉じるボタン。
        if explicit && !locked_now {
            let close_size = 22.0;
            let close_margin = 5.0 + button_size + 4.0;
            let close_rect = egui::Rect::from_min_size(
                egui::pos2(
                    title_rect.max.x - close_size - close_margin,
                    title_rect.min.y + (TITLE_BAR_H - close_size) * 0.5,
                ),
                egui::vec2(close_size, close_size),
            );
            let close_resp = ui.interact(
                close_rect,
                egui::Id::new("metadata_close_btn"),
                egui::Sense::click(),
            );
            let close_bg = if close_resp.hovered() {
                egui::Color32::from_rgba_unmultiplied(100, 100, 100, 200)
            } else {
                egui::Color32::TRANSPARENT
            };
            ui.painter().rect_filled(close_rect, 3.0, close_bg);
            let c = close_rect.center();
            let d = close_rect.width().min(close_rect.height()) * 0.22;
            let stroke = egui::Stroke::new(1.7, egui::Color32::from_gray(225));
            ui.painter().line_segment(
                [egui::pos2(c.x - d, c.y - d), egui::pos2(c.x + d, c.y + d)],
                stroke,
            );
            ui.painter().line_segment(
                [egui::pos2(c.x + d, c.y - d), egui::pos2(c.x - d, c.y + d)],
                stroke,
            );
            if close_resp.on_hover_text("情報パネルを閉じる").clicked() {
                self.close_fullscreen_info_panel();
            }
        }

        // ── コンテンツ領域 (タイトルバーの下) ──
        let content_top = title_rect.max.y;
        let content_rect =
            egui::Rect::from_min_max(egui::pos2(panel_rect.min.x, content_top), panel_rect.max);

        let ai_metadata = self.get_current_ai_metadata();
        let exif_info = self.get_current_exif();
        let tweet_info = self.get_current_tweet_info();
        let sidecar_info = self.get_current_sidecar();
        let current_palette = self.current_fullscreen_color_palette();
        // Collect CPU completions before drawing. Which of them may upload, and which queued jobs
        // may start, is decided after the frame has produced its visible-card demand set.
        self.similar_panel.begin_thumbnail_frame();

        // タグパネル用の情報を先に集める (child_ui の &mut ui closure 前に借用を解消するため)
        let tag_rows = self.collect_fullscreen_tag_panel_rows();
        self.sync_fullscreen_tag_panel_state(&tag_rows);
        let tag_catalog: Vec<_> = self
            .cached_tag_choice_catalog()
            .into_iter()
            .map(|tag| TagPanelChoice {
                name: tag.name,
                tag_key: tag.tag_key,
                count: tag.count,
                pinned: tag.pinned,
                last_applied_at: tag.last_applied_at,
            })
            .collect();
        let pinned_tags: Vec<_> = self
            .settings
            .tags
            .iter()
            .filter(|tag| tag.show_shortcut)
            .map(|tag| TagPanelChoice {
                name: tag.name.clone(),
                tag_key: tag.tag_key.clone(),
                count: tag_catalog
                    .iter()
                    .find(|choice| choice.tag_key == tag.tag_key)
                    .map(|choice| choice.count)
                    .unwrap_or(0),
                pinned: true,
                last_applied_at: tag_catalog
                    .iter()
                    .find(|choice| choice.tag_key == tag.tag_key)
                    .map(|choice| choice.last_applied_at)
                    .unwrap_or(0),
            })
            .collect();
        let visible_tag_choices = tag_panel_visible_choices(
            &pinned_tags,
            &tag_rows,
            &self.fullscreen_tag_panel_sticky_tags,
            &tag_catalog,
        );
        let show_tag_panel =
            !visible_tag_choices.is_empty() || tag_rows.iter().any(|row| !row.targets.is_empty());

        // タグボタンクリックを closure 内で検出し、後段で self の操作に流す。
        let mut clicked_tag: Option<(String, Vec<TagTarget>)> = None;
        let mut set_tag: Option<(String, bool, Vec<TagTarget>)> = None;
        let mut searched_tag: Option<String> = None;
        let mut clicked_palette_rgb: Option<[u8; 3]> = None;
        let mut similar_actions = SimilarPanelActions::default();
        let pinned_compare_item_key = self
            .pinned_compare_slot
            .as_ref()
            .and_then(|slot| slot.external_item_key.clone());
        let compare_pin_preparing = self.compare_pin_pending.is_some();
        // ★ レーティング (画像/動画/音声で統一。★ → タグ → 内容 の先頭)。レーティング可能な
        // 単一アイテム (画像 / ZIP 内画像 / PDF ページ) でページ★を出す。
        let rating_idx = self
            .fullscreen_idx
            .filter(|&i| self.items.get(i).is_some_and(|it| it.accepts_rating()));
        let rating_stars = rating_idx.map(|i| self.get_rating(i)).unwrap_or(0);
        let mut set_rating: Option<u8> = None;
        // ページ★とは独立したコンテナ★を、タグ行の表示ラベルではなく現在の
        // container-rating ownership から解決する。これにより PDF / ZIP / 変換書庫でも
        // ページ行とコンテナ行を並べて表示できる。
        let container_rating = self.current_metadata_panel_container_rating();
        let mut set_container_rating: Option<u8> = None;
        let tag_picker_enter_pressed = self.dialog_enter_pressed(ctx);
        let tag_picker_escape_pressed = self.dialog_escape_pressed(ctx);
        if self.fullscreen_tag_picker_open && tag_picker_escape_pressed {
            self.fullscreen_tag_picker_open = false;
            self.fullscreen_tag_picker_input.clear();
            self.fullscreen_tag_picker_row_key = None;
            self.fullscreen_tag_picker_focus_request = false;
            self.fullscreen_tag_picker_recent_tab = false;
        }

        let inner_rect = content_rect.shrink2(egui::vec2(12.0, 8.0));
        let mut child_ui = ui.new_child(egui::UiBuilder::new().max_rect(inner_rect));
        child_ui.set_clip_rect(content_rect);
        apply_metadata_panel_dark_widget_style(&mut child_ui);
        // Metadata values often contain long CJK text, URLs, and hashes. Use a
        // solid scrollbar here so egui reserves a real gutter instead of
        // drawing the default floating bar on top of the text.
        child_ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();

        egui::ScrollArea::vertical()
            .id_salt("metadata_scroll")
            .auto_shrink([false, false])
            .show(&mut child_ui, |ui| {
                ui.set_width(metadata_scroll_content_width(ui, inner_rect.width()));

                draw_metadata_panel_tabs(ui, &mut self.similar_panel.tab);
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(8.0);

                if self.similar_panel.tab == MetadataPanelTab::Similar {
                    // 見開き中は**両方のページ**を見ている。片方だけ調べると、もう片方に
                    // 別バージョンがあっても気付けない。補正パネルが左右を切り替えるのは
                    // 編集が一度に 1 ページだからで、閲覧側にその制約はない。
                    let spread = self
                        .fullscreen_idx
                        .map(|idx| self.resolve_spread_pair(idx))
                        .unwrap_or(crate::ui_fullscreen::SpreadPair::Single);
                    let shown_indices = similar_shown_indices(self.fullscreen_idx, spread);
                    let shown_items: Vec<crate::grid_item::GridItem> = shown_indices
                        .iter()
                        .filter_map(|index| self.items.get(*index).cloned())
                        .collect();
                    let current_item = shown_items.first().cloned();
                    let origin_key = current_item
                        .as_ref()
                        .and_then(crate::app::similar_index_item_key);
                    self.similar_panel.begin_origin(origin_key.clone());
                    let page_keys: Vec<Option<String>> = shown_items
                        .iter()
                        .map(crate::app::similar_index_item_key)
                        .collect();
                    let queries: Vec<std::sync::Arc<crate::similar_index::ItemQuery>> = shown_items
                        .iter()
                        .map(|item| self.query_similar_item(item))
                        .collect();
                    let book = current_item
                        .as_ref()
                        .map(|item| self.query_similar_book(item));
                    let results_are_stale = self.similar_query_results_are_stale();
                    // 照会が返るまでの数フレームだけ、直前の結果を出し続ける。面ごとに覚える
                    // ので、見開きでも左右それぞれが空白にならない。
                    self.similar_panel.last_ready.resize(queries.len(), None);
                    let shown_queries: Vec<(
                        std::sync::Arc<crate::similar_index::ItemQuery>,
                        bool,
                    )> = queries
                        .iter()
                        .enumerate()
                        .map(|(slot, query)| {
                            if matches!(query.as_ref(), crate::similar_index::ItemQuery::Preparing)
                                && let Some(previous) = self.similar_panel.last_ready[slot].clone()
                            {
                                return (previous, true);
                            }
                            self.similar_panel.last_ready[slot] =
                                matches!(query.as_ref(), crate::similar_index::ItemQuery::Ready(_))
                                    .then(|| std::sync::Arc::clone(query));
                            (std::sync::Arc::clone(query), false)
                        })
                        .collect();
                    let spread_headings = shown_queries.len() > 1;
                    let views: Vec<SimilarPageView<'_>> = shown_queries
                        .iter()
                        .enumerate()
                        .map(|(slot, (shown, showing_previous))| SimilarPageView {
                            heading: spread_headings.then(|| {
                                if slot == 0 {
                                    "左ページ"
                                } else {
                                    "右ページ"
                                }
                            }),
                            item_key: page_keys.get(slot).and_then(Option::as_deref),
                            model: similar_panel_model(shown.as_ref()),
                            showing_previous: *showing_previous,
                        })
                        .collect();
                    draw_similar_panel(
                        ui,
                        &views,
                        book.as_deref(),
                        results_are_stale,
                        &mut self.similar_panel,
                        self.settings.thumb_px.max(SIMILAR_THUMB_SIZE as u32),
                        self.settings.thumb_quality,
                        crate::thumb_loader::CacheDecision::from_settings(&self.settings),
                        Some(&self.pdf_passwords),
                        pinned_compare_item_key.as_deref(),
                        compare_pin_preparing,
                        ctx,
                        &mut similar_actions,
                    );
                    return;
                }

                if self.fullscreen_tag_picker_open {
                    draw_fullscreen_tag_picker_panel(
                        ui,
                        &tag_catalog,
                        &tag_rows,
                        &mut self.fullscreen_tag_picker_open,
                        &mut self.fullscreen_tag_picker_input,
                        &mut self.fullscreen_tag_picker_row_key,
                        &mut self.fullscreen_tag_picker_focus_request,
                        &mut self.fullscreen_tag_picker_recent_tab,
                        tag_picker_enter_pressed,
                        &mut set_tag,
                    );
                    return;
                }

                // ── ★ レーティング (最上段。★ → タグ → 内容 の統一順序) ──
                // ページ★とコンテナ★を両方出す (コンテナ行があるときはページを明記)。
                let show_rating = rating_idx.is_some() || container_rating.is_some();
                if show_rating {
                    if let Some((label, stars)) = container_rating {
                        ui.label(egui::RichText::new(label).color(LABEL_COLOR).size(11.0));
                        set_container_rating = crate::ui_helpers::draw_rating_stars(ui, stars);
                        if rating_idx.is_some() {
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("ページ").color(LABEL_COLOR).size(11.0));
                        }
                    }
                    if rating_idx.is_some() {
                        set_rating = crate::ui_helpers::draw_rating_stars(ui, rating_stars);
                    }
                    if show_tag_panel
                        || tweet_info.is_some()
                        || ai_metadata.is_some()
                        || exif_info.is_some()
                        || sidecar_info.is_some()
                        || current_palette.is_some()
                    {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);
                    }
                }

                // ── タグパネル ──
                // ピン留めタグと、表示対象に付いている未ピン留めタグを ON/OFF ボタンで並べる。
                if show_tag_panel {
                    draw_tag_panel(
                        ui,
                        &visible_tag_choices,
                        &tag_rows,
                        &mut self.fullscreen_tag_picker_open,
                        &mut self.fullscreen_tag_picker_input,
                        &mut self.fullscreen_tag_picker_row_key,
                        &mut self.fullscreen_tag_picker_focus_request,
                        &mut self.fullscreen_tag_picker_recent_tab,
                        &mut clicked_tag,
                        &mut searched_tag,
                    );
                    if tweet_info.is_some()
                        || ai_metadata.is_some()
                        || exif_info.is_some()
                        || sidecar_info.is_some()
                        || current_palette.is_some()
                    {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);
                    }
                }

                // 画像色パレット
                if let Some(ref palette) = current_palette {
                    draw_image_color_palette_section(ui, palette, &mut clicked_palette_rgb);
                    if tweet_info.is_some()
                        || ai_metadata.is_some()
                        || exif_info.is_some()
                        || sidecar_info.is_some()
                    {
                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(8.0);
                    }
                }

                // X ツイート情報 (mXD 由来)
                if let Some(ref t) = tweet_info {
                    draw_tweet_panel(ui, ctx, t);
                    if ai_metadata.is_some() || exif_info.is_some() {
                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(8.0);
                    }
                }

                // AI メタデータセクション
                match ai_metadata {
                    Some(AiMetadata::A1111(ref meta)) => {
                        draw_a1111_panel(ui, ctx, meta);
                    }
                    Some(AiMetadata::ComfyUI(ref meta)) => {
                        let show_raw_prompt = self.metadata_show_raw_prompt;
                        let show_raw_workflow = self.metadata_show_raw_workflow;
                        let (new_rp, new_rw) =
                            draw_comfyui_panel(ui, ctx, meta, show_raw_prompt, show_raw_workflow);
                        self.metadata_show_raw_prompt = new_rp;
                        self.metadata_show_raw_workflow = new_rw;
                    }
                    Some(AiMetadata::Unknown(ref chunks)) => {
                        draw_unknown_panel(ui, chunks);
                    }
                    None => {}
                }

                // EXIF セクション
                if let Some(ref exif) = exif_info {
                    if ai_metadata.is_some() {
                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(8.0);
                    }
                    draw_exif_panel(ui, exif, &mut self.exif_sections_open);
                }

                // 外部メタデータ (サイドカー) セクション (FS 画像のみ。docs §11)
                if let Some(ref sc) = sidecar_info {
                    if ai_metadata.is_some()
                        || exif_info.is_some()
                        || tweet_info.is_some()
                        || show_tag_panel
                    {
                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(8.0);
                    }
                    draw_sidecar_section(ui, sc);
                }

                // 何もない場合 (★ 行も出ていないとき)
                if ai_metadata.is_none()
                    && exif_info.is_none()
                    && tweet_info.is_none()
                    && !show_tag_panel
                    && sidecar_info.is_none()
                    && current_palette.is_none()
                    && rating_idx.is_none()
                    && container_rating.is_none()
                {
                    draw_no_metadata(ui);
                }
            });

        self.similar_panel
            .finish_thumbnail_frame(ctx, &similar_actions.thumbnail_demand);

        if let Some(hit) = similar_actions.pin_hit.take() {
            self.pin_similar_hit(ctx, &hit, full_rect);
        }
        self.update_similar_peek(ctx, similar_actions.peek_held.take(), full_rect);
        if similar_actions.open_favorites {
            self.show_favorites_editor = true;
        }
        if let Some(hit) = similar_actions.open_hit {
            self.open_similar_hit(&hit);
        }
        if let Some((item_key, target)) = similar_actions.open_page {
            self.open_similar_book_page(&item_key, target);
        }

        // ★ レーティングの後処理 (draw_rating_stars が「同★再クリック=0」を解決済み)。
        if let (Some(idx), Some(new_stars)) = (rating_idx, set_rating) {
            self.set_rating(idx, new_stars);
        }
        if let Some(new_stars) = set_container_rating {
            match self.set_current_folder_rating(new_stars) {
                Ok(_) => {}
                Err(error) => self.report_rating_write_error(&error),
            }
        }
        // タグボタンクリックの後処理 (closure 外で self を可変借用する)
        if let Some((tag_name, targets)) = clicked_tag {
            self.request_tag_toggle_for_targets(
                &tag_name,
                targets,
                crate::app::ActionSurface::Viewer,
            );
        }
        if let Some((tag_name, add, targets)) = set_tag {
            if add {
                self.request_tag_add_for_targets(
                    &tag_name,
                    targets,
                    crate::app::ActionSurface::Viewer,
                );
            } else {
                self.request_tag_remove_for_targets(
                    &tag_name,
                    targets,
                    crate::app::ActionSurface::Viewer,
                );
            }
        }
        if let Some(tag_name) = searched_tag {
            self.open_tag_view_for_tag(&tag_name);
        }
        if let Some(rgb) = clicked_palette_rgb {
            self.apply_image_color_filter_from_swatch(rgb, ctx);
        }

        true
    }

    /// 音楽ビュー右パネル (Inc 5) 用のタグセクションを描く。
    ///
    /// 画像メタデータパネル (`draw_metadata_panel_inner`) と**同じ**タグ ON/OFF ボタン +
    /// ピッカー UI を再利用する (`draw_tag_panel` / `draw_fullscreen_tag_picker_panel` は
    /// 本モジュール private なのでこのメソッドを本モジュールに置く)。対象は現在のフルスク
    /// リーンアイテム (= 音声ファイル) のタグ。呼び出し側 (`ui_music_panels`) が右パネル内の
    /// 適切な `ui` (ダークスタイル + スクロール可) を渡す。IME (改名/ピッカー入力の Enter/
    /// Escape) は `dialog_*_pressed` 経由で画像パネルと同一の扱い。
    pub(crate) fn draw_music_tag_section(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // タグ行 (単一 audio target) + カタログ + ピン留めタグ + 可視選択肢を集める
        // (draw_metadata_panel_inner と同じ構築)。
        let tag_rows = self.collect_fullscreen_tag_panel_rows();
        self.sync_fullscreen_tag_panel_state(&tag_rows);
        let tag_catalog: Vec<_> = self
            .cached_tag_choice_catalog()
            .into_iter()
            .map(|tag| TagPanelChoice {
                name: tag.name,
                tag_key: tag.tag_key,
                count: tag.count,
                pinned: tag.pinned,
                last_applied_at: tag.last_applied_at,
            })
            .collect();
        let pinned_tags: Vec<_> = self
            .settings
            .tags
            .iter()
            .filter(|tag| tag.show_shortcut)
            .map(|tag| TagPanelChoice {
                name: tag.name.clone(),
                tag_key: tag.tag_key.clone(),
                count: tag_catalog
                    .iter()
                    .find(|choice| choice.tag_key == tag.tag_key)
                    .map(|choice| choice.count)
                    .unwrap_or(0),
                pinned: true,
                last_applied_at: tag_catalog
                    .iter()
                    .find(|choice| choice.tag_key == tag.tag_key)
                    .map(|choice| choice.last_applied_at)
                    .unwrap_or(0),
            })
            .collect();
        let visible_tag_choices = tag_panel_visible_choices(
            &pinned_tags,
            &tag_rows,
            &self.fullscreen_tag_panel_sticky_tags,
            &tag_catalog,
        );
        let show_tag_panel =
            !visible_tag_choices.is_empty() || tag_rows.iter().any(|row| !row.targets.is_empty());
        if !show_tag_panel {
            return;
        }

        let mut clicked_tag: Option<(String, Vec<TagTarget>)> = None;
        let mut set_tag: Option<(String, bool, Vec<TagTarget>)> = None;
        let mut searched_tag: Option<String> = None;
        let tag_picker_enter_pressed = self.dialog_enter_pressed(ctx);
        let tag_picker_escape_pressed = self.dialog_escape_pressed(ctx);
        if self.fullscreen_tag_picker_open && tag_picker_escape_pressed {
            self.fullscreen_tag_picker_open = false;
            self.fullscreen_tag_picker_input.clear();
            self.fullscreen_tag_picker_row_key = None;
            self.fullscreen_tag_picker_focus_request = false;
            self.fullscreen_tag_picker_recent_tab = false;
        }

        apply_metadata_panel_dark_widget_style(ui);
        if self.fullscreen_tag_picker_open {
            draw_fullscreen_tag_picker_panel(
                ui,
                &tag_catalog,
                &tag_rows,
                &mut self.fullscreen_tag_picker_open,
                &mut self.fullscreen_tag_picker_input,
                &mut self.fullscreen_tag_picker_row_key,
                &mut self.fullscreen_tag_picker_focus_request,
                &mut self.fullscreen_tag_picker_recent_tab,
                tag_picker_enter_pressed,
                &mut set_tag,
            );
        } else {
            draw_tag_panel(
                ui,
                &visible_tag_choices,
                &tag_rows,
                &mut self.fullscreen_tag_picker_open,
                &mut self.fullscreen_tag_picker_input,
                &mut self.fullscreen_tag_picker_row_key,
                &mut self.fullscreen_tag_picker_focus_request,
                &mut self.fullscreen_tag_picker_recent_tab,
                &mut clicked_tag,
                &mut searched_tag,
            );
        }

        // タグボタンクリックの後処理 (closure 外で self を可変借用、画像パネルと同一経路)。
        if let Some((tag_name, targets)) = clicked_tag {
            self.request_tag_toggle_for_targets(
                &tag_name,
                targets,
                crate::app::ActionSurface::Viewer,
            );
        }
        if let Some((tag_name, add, targets)) = set_tag {
            if add {
                self.request_tag_add_for_targets(
                    &tag_name,
                    targets,
                    crate::app::ActionSurface::Viewer,
                );
            } else {
                self.request_tag_remove_for_targets(
                    &tag_name,
                    targets,
                    crate::app::ActionSurface::Viewer,
                );
            }
        }
        if let Some(tag_name) = searched_tag {
            self.open_tag_view_for_tag(&tag_name);
        }
    }

    /// 現在のフルスクリーン画像の AI メタデータを取得する。
    fn get_current_ai_metadata(&self) -> Option<AiMetadata> {
        let idx = self.fullscreen_idx?;
        let key = self.metadata_cache_key(idx)?;
        self.metadata_cache.get(&key).cloned().flatten()
    }

    /// 現在のフルスクリーン画像の EXIF 情報を取得する。
    fn get_current_exif(&self) -> Option<ExifInfo> {
        let idx = self.fullscreen_idx?;
        let key = self.metadata_cache_key(idx)?;
        self.exif_cache.get(&key).cloned().flatten()
    }

    /// 現在のフルスクリーン画像の XMP (X/Twitter) 情報を取得する。
    fn get_current_tweet_info(&self) -> Option<XmpTweetInfo> {
        let idx = self.fullscreen_idx?;
        let key = self.metadata_cache_key(idx)?;
        self.xmp_cache.get(&key).cloned().flatten()
    }

    /// 現在のフルスクリーン画像の外部メタデータサイドカー (表示用) を取得する。
    /// worker (`run_metadata_load`) が FS 画像のみ埋めるので、動画 / ZIP / PDF は常に None。
    fn get_current_sidecar(&self) -> Option<crate::external_metadata::SidecarDisplay> {
        let idx = self.fullscreen_idx?;
        let key = self.metadata_cache_key(idx)?;
        self.sidecar_display_cache.get(&key).cloned().flatten()
    }

    pub(crate) fn current_metadata_panel_container_rating(&mut self) -> Option<(&'static str, u8)> {
        use crate::rating_db::RatingItemKind;

        let (_, _, meta) = self.current_container_rating_target()?;
        let label = match meta.kind {
            RatingItemKind::Folder | RatingItemKind::ZipDir => "フォルダ",
            RatingItemKind::PdfFile => "PDF",
            RatingItemKind::ZipFile | RatingItemKind::ConvertibleArchive => "書庫",
            _ => return None,
        };
        Some((label, self.current_folder_rating()))
    }

    fn collect_fullscreen_tag_panel_rows(&mut self) -> Vec<TagPanelRow> {
        let Some(idx) = self.fullscreen_idx else {
            return vec![TagPanelRow::disabled()];
        };

        if let crate::ui_fullscreen::SpreadPair::Double { left, right } =
            self.resolve_visible_spread_pair(idx)
        {
            if let Some(row) = self.container_spread_tag_panel_row(left, right) {
                return vec![row];
            }
            if let Some(rows) = self.normal_image_tag_panel_rows(&[left, right]) {
                return rows;
            }
        }

        if let Some(rows) = self.normal_image_tag_panel_rows(&[idx]) {
            return rows;
        }

        self.single_tag_panel_row(idx)
            .map(|row| vec![row])
            .unwrap_or_else(|| vec![TagPanelRow::disabled()])
    }

    fn single_tag_panel_row(&mut self, idx: usize) -> Option<TagPanelRow> {
        let target = self.tag_target_for_index(idx, true)?;
        let note = self.tag_target_note_for_item(idx, &target.path);
        Some(self.build_tag_panel_row(None, note, vec![target]))
    }

    fn container_spread_tag_panel_row(&mut self, left: usize, right: usize) -> Option<TagPanelRow> {
        let left_target = self.tag_target_for_index(left, true)?;
        let right_target = self.tag_target_for_index(right, true)?;
        if !crate::folder_tree::path_eq(&left_target.path, &right_target.path) {
            return None;
        }
        let note = self.tag_target_note_for_item(left, &left_target.path);
        Some(self.build_tag_panel_row(None, note, vec![left_target]))
    }

    fn normal_image_tag_panel_rows(&mut self, indices: &[usize]) -> Option<Vec<TagPanelRow>> {
        if indices.is_empty() {
            return None;
        }
        let mut image_paths = Vec::with_capacity(indices.len());
        for &idx in indices {
            match self.items.get(idx) {
                Some(crate::grid_item::GridItem::Image(path)) => image_paths.push(path.clone()),
                _ => return None,
            }
        }

        let shared_parent = shared_parent_folder(&image_paths)?;
        let folder_path = self
            .current_folder
            .clone()
            .filter(|path| crate::folder_tree::path_eq(path, &shared_parent))?;
        let folder_name = tag_path_display_name(&folder_path);
        let folder_target = self.tag_target_for_path(folder_path, false);
        let mut rows = vec![self.build_tag_panel_row(
            Some("フォルダ".to_string()),
            Some(format!("タグ対象: {folder_name}")),
            vec![folder_target],
        )];

        let mut page_targets = Vec::new();
        for &idx in indices {
            if let Some(target) = self.tag_target_for_index(idx, true) {
                page_targets.push(target);
            }
        }
        if !page_targets.is_empty() {
            let page_note = if page_targets.len() >= 2 {
                "タグ対象: 表示中の2ページ"
            } else {
                "タグ対象: 現在のページ"
            };
            rows.push(self.build_tag_panel_row(
                Some("ページ".to_string()),
                Some(page_note.to_string()),
                page_targets,
            ));
        }
        Some(rows)
    }

    fn build_tag_panel_row(
        &mut self,
        label: Option<String>,
        note: Option<String>,
        mut targets: Vec<TagTarget>,
    ) -> TagPanelRow {
        dedup_tag_targets(&mut targets);
        let paths: Vec<PathBuf> = targets.iter().map(|target| target.path.clone()).collect();
        self.hydrate_tags_cache_for_paths(&paths);
        let tags_by_target = paths
            .iter()
            .map(|path| {
                let key = crate::tags_db::item_key_for_path(path);
                self.tags_cache.get(&key).cloned().unwrap_or_default()
            })
            .collect();
        TagPanelRow {
            label,
            note,
            targets,
            tags_by_target,
        }
    }

    fn tag_target_note_for_item(&self, idx: usize, target_path: &Path) -> Option<String> {
        use crate::grid_item::GridItem;

        let target_kind = match self.items.get(idx)? {
            GridItem::ZipImage { .. }
            | GridItem::ZipFile(_)
            | GridItem::ConvertibleArchive { .. } => "この本",
            GridItem::PdfPage { .. } | GridItem::PdfFile(_) => "このPDF",
            _ => return None,
        };
        Some(format!(
            "タグ対象: {target_kind} ({})",
            tag_path_display_name(target_path)
        ))
    }

    fn sync_fullscreen_tag_panel_state(&mut self, rows: &[TagPanelRow]) {
        let target_key = tag_panel_target_key(rows);
        if self.fullscreen_tag_panel_target_key.as_deref() != Some(target_key.as_str()) {
            self.fullscreen_tag_panel_target_key = Some(target_key);
            self.fullscreen_tag_panel_sticky_tags.clear();
            self.fullscreen_tag_picker_open = false;
            self.fullscreen_tag_picker_input.clear();
            self.fullscreen_tag_picker_row_key = None;
            self.fullscreen_tag_picker_focus_request = false;
        }

        for row in rows {
            for tags in &row.tags_by_target {
                for tag in tags {
                    let tag_key = crate::tags_db::normalize_tag_key(tag);
                    if tag_key.is_empty()
                        || self
                            .settings
                            .tags
                            .iter()
                            .any(|def| def.show_shortcut && def.tag_key == tag_key)
                        || self
                            .fullscreen_tag_panel_sticky_tags
                            .iter()
                            .any(|(_, key)| key == &tag_key)
                    {
                        continue;
                    }
                    let name = self
                        .settings
                        .tags
                        .iter()
                        .find(|def| def.tag_key == tag_key)
                        .map(|def| def.name.clone())
                        .unwrap_or_else(|| crate::tags_db::strip_display_hash(tag).to_string());
                    self.fullscreen_tag_panel_sticky_tags.push((name, tag_key));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 描画ヘルパー
// ---------------------------------------------------------------------------

const LABEL_COLOR: egui::Color32 = egui::Color32::from_rgb(140, 160, 200);
const TEXT_COLOR: egui::Color32 = egui::Color32::from_rgb(230, 230, 230);
const DIM_COLOR: egui::Color32 = egui::Color32::from_rgb(150, 150, 150);
const JSON_COLOR: egui::Color32 = egui::Color32::from_rgb(190, 200, 210);
const SECTION_FONT: f32 = 14.0;
const BODY_FONT: f32 = 13.0;

fn apply_metadata_panel_dark_widget_style(ui: &mut egui::Ui) {
    let visuals = &mut ui.style_mut().visuals;
    visuals.extreme_bg_color = egui::Color32::from_rgb(28, 31, 38);
    visuals.selection.bg_fill = egui::Color32::from_rgb(58, 84, 116);
    visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(190, 215, 245));

    visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(42, 46, 56);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(48, 53, 64);
    visuals.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(88, 98, 118));
    visuals.widgets.inactive.fg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(232, 236, 244));

    visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(54, 60, 74);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(62, 70, 86);
    visuals.widgets.hovered.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(124, 144, 176));
    visuals.widgets.hovered.fg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(248, 250, 255));

    visuals.widgets.active.weak_bg_fill = egui::Color32::from_rgb(62, 72, 92);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(72, 86, 110);
    visuals.widgets.active.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(154, 178, 214));
    visuals.widgets.active.fg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 255, 255));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TagButtonState {
    Off,
    Mixed,
    On,
}

/// タグパネル描画 (docs/archive/search-metadata/tag-feature.md §4.4)。
///
/// 登録タグを ON/OFF ボタンで横並び表示。各ボタンの外観:
/// - ON (現在のファイルに付与済み): 緑背景 + 強調
/// - Mixed (見開きの一部ページだけ付与済み): アンバー背景
/// - OFF: 通常
/// - 対応形式外: グレーアウト (クリック不可)
///
/// クリック時は `clicked_tag` に `TagDef.name` を書き込む (closure 外でトグル実行)。
fn draw_tag_panel(
    ui: &mut egui::Ui,
    visible_tags: &[TagPanelChoice],
    rows: &[TagPanelRow],
    picker_open: &mut bool,
    picker_input: &mut String,
    picker_row_key: &mut Option<String>,
    picker_focus_request: &mut bool,
    picker_recent_tab: &mut bool,
    clicked_tag: &mut Option<(String, Vec<TagTarget>)>,
    searched_tag: &mut Option<String>,
) {
    let any_taggable = rows.iter().any(|row| !row.targets.is_empty());
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("タグ")
                .color(egui::Color32::WHITE)
                .size(14.0)
                .strong(),
        );
        if !any_taggable {
            ui.label(egui::RichText::new("(対象外)").size(10.0).color(DIM_COLOR))
                .on_hover_text("この項目にはタグを付けられません。");
        }
    });
    ui.add_space(4.0);

    for (row_idx, row) in rows.iter().enumerate() {
        if row_idx > 0 {
            ui.add_space(7.0);
        }
        let row_key = tag_panel_row_key(row);
        ui.horizontal_wrapped(|ui| {
            if let Some(label) = row.label.as_deref() {
                ui.label(
                    egui::RichText::new(label)
                        .color(LABEL_COLOR)
                        .size(12.0)
                        .strong(),
                );
            }
            if let Some(note) = row.note.as_deref() {
                ui.label(egui::RichText::new(note).size(11.0).color(DIM_COLOR));
            }
            if !row.targets.is_empty() {
                let row_picker_open = *picker_open
                    && picker_row_key
                        .as_deref()
                        .is_some_and(|key| key == row_key.as_str());
                let plus_label = if row_picker_open { "×" } else { "＋" };
                let resp = ui
                    .small_button(plus_label)
                    .on_hover_text(if row_picker_open {
                        "タグ入力を閉じる"
                    } else {
                        "タグを検索/入力して付ける"
                    });
                if resp.clicked() {
                    if row_picker_open {
                        *picker_open = false;
                        picker_input.clear();
                        *picker_row_key = None;
                        *picker_focus_request = false;
                        *picker_recent_tab = false;
                    } else {
                        *picker_open = true;
                        picker_input.clear();
                        *picker_row_key = Some(row_key.clone());
                        *picker_focus_request = true;
                        *picker_recent_tab = false;
                    }
                }
            }
        });
        ui.add_space(3.0);

        // ボタンを折り返し配置。付与中は `#タグ名` を緑色で、未付与は通常色で表示する
        // (丸ドット等の装飾は付けない: ラベルの色とボタン背景で状態を伝える)。
        if visible_tags.is_empty() {
            ui.label(
                egui::RichText::new("（タグなし）")
                    .size(11.0)
                    .color(DIM_COLOR),
            );
        } else {
            ui.horizontal_wrapped(|ui| {
                for def in visible_tags {
                    let with_hash = format!("#{}", def.name);
                    let state = tag_button_state(row, &def.tag_key);
                    let (text_color, fill_color, stroke_color) = tag_button_visuals(state);
                    let label = egui::RichText::new(&with_hash).color(text_color).strong();
                    let btn = egui::Button::new(label)
                        .fill(fill_color)
                        .stroke(egui::Stroke::new(1.15, stroke_color));
                    let is_taggable = !row.targets.is_empty();
                    let resp = ui.add_enabled(is_taggable, btn);
                    let resp = resp.on_hover_text(match state {
                        TagButtonState::On => format!("クリックで `{with_hash}` を削除"),
                        TagButtonState::Mixed => {
                            format!("一部の対象に付与済み。クリックで `{with_hash}` を全対象に付与")
                        }
                        TagButtonState::Off => format!("クリックで `{with_hash}` を付与"),
                    });
                    let clicked = resp.clicked();
                    resp.context_menu(|ui| {
                        if ui.button("このタグで探す").clicked() {
                            *searched_tag = Some(def.name.clone());
                            ui.close();
                        }
                    });
                    if clicked {
                        *clicked_tag = Some((def.name.clone(), row.targets.clone()));
                    }
                }
            });
        }
    }
}

fn draw_image_color_palette_section(
    ui: &mut egui::Ui,
    palette: &crate::color_search::Palette,
    clicked_rgb: &mut Option<[u8; 3]>,
) {
    if palette.colors.is_empty() {
        return;
    }

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("画像色")
                .color(egui::Color32::WHITE)
                .size(14.0)
                .strong(),
        )
        .on_hover_text("画像として扱える項目だけを、この色で絞り込みます。");
        ui.label(
            egui::RichText::new(format!("{} 色", palette.colors.len()))
                .color(DIM_COLOR)
                .size(11.0),
        );
    });
    ui.add_space(5.0);

    ui.horizontal_wrapped(|ui| {
        for color in &palette.colors {
            let rgb = color.rgb;
            let fill = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
            let swatch_size = egui::vec2(24.0, 24.0);
            let (rect, response) = ui.allocate_exact_size(swatch_size, egui::Sense::click());
            let stroke_color = if response.hovered() {
                egui::Color32::WHITE
            } else {
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 80)
            };
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(5), fill);
            ui.painter().rect_stroke(
                rect,
                egui::CornerRadius::same(5),
                egui::Stroke::new(1.0, stroke_color),
                egui::epaint::StrokeKind::Outside,
            );
            let tooltip = format!(
                "{} ({:.1}%)\nクリックで画像色フィルタに使用",
                crate::color_search::hex_rgb(rgb),
                color.ratio * 100.0
            );
            if response.on_hover_text(tooltip).clicked() {
                *clicked_rgb = Some(rgb);
            }
        }
    });
}

fn draw_fullscreen_tag_picker_panel(
    ui: &mut egui::Ui,
    tag_catalog: &[TagPanelChoice],
    rows: &[TagPanelRow],
    picker_open: &mut bool,
    picker_input: &mut String,
    picker_row_key: &mut Option<String>,
    picker_focus_request: &mut bool,
    picker_recent_tab: &mut bool,
    enter_pressed: bool,
    set_tag: &mut Option<(String, bool, Vec<TagTarget>)>,
) {
    let selected_idx = picker_row_key
        .as_deref()
        .and_then(|key| rows.iter().position(|row| tag_panel_row_key(row) == key))
        .or_else(|| rows.iter().position(|row| !row.targets.is_empty()));

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("タグを選択")
                .color(egui::Color32::WHITE)
                .size(15.0)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("戻る").clicked() {
                close_fullscreen_tag_picker(
                    picker_open,
                    picker_input,
                    picker_row_key,
                    picker_focus_request,
                    picker_recent_tab,
                );
            }
        });
    });
    ui.add_space(6.0);

    let Some(row_idx) = selected_idx else {
        ui.label(
            egui::RichText::new("タグを付けられる対象がありません。")
                .size(12.0)
                .color(DIM_COLOR),
        );
        return;
    };
    let row = &rows[row_idx];
    if let Some(label) = row.label.as_deref() {
        ui.label(
            egui::RichText::new(label)
                .color(LABEL_COLOR)
                .size(12.0)
                .strong(),
        );
    }
    if let Some(note) = row.note.as_deref() {
        ui.label(egui::RichText::new(note).size(11.0).color(DIM_COLOR));
    }
    ui.add_space(8.0);

    let mut close_after_apply = false;
    draw_fullscreen_tag_picker(
        ui,
        tag_catalog,
        row,
        picker_input,
        picker_focus_request,
        picker_recent_tab,
        enter_pressed,
        set_tag,
        &mut close_after_apply,
    );
    if close_after_apply {
        close_fullscreen_tag_picker(
            picker_open,
            picker_input,
            picker_row_key,
            picker_focus_request,
            picker_recent_tab,
        );
    }
}

fn close_fullscreen_tag_picker(
    picker_open: &mut bool,
    picker_input: &mut String,
    picker_row_key: &mut Option<String>,
    picker_focus_request: &mut bool,
    picker_recent_tab: &mut bool,
) {
    *picker_open = false;
    picker_input.clear();
    *picker_row_key = None;
    *picker_focus_request = false;
    *picker_recent_tab = false;
}

fn draw_fullscreen_tag_picker(
    ui: &mut egui::Ui,
    tag_catalog: &[TagPanelChoice],
    row: &TagPanelRow,
    input: &mut String,
    focus_request: &mut bool,
    recent_tab: &mut bool,
    enter_pressed: bool,
    set_tag: &mut Option<(String, bool, Vec<TagTarget>)>,
    close_after_apply: &mut bool,
) {
    ui.add_space(5.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("タグ:").size(12.0).color(DIM_COLOR));
        let input_resp = crate::ime_focus::add_sized_singleline(
            ui,
            egui::vec2(180.0, 22.0),
            input,
            Some(focus_request),
            |edit| {
                edit.hint_text("タグを検索/入力")
                    .return_key(None::<egui::KeyboardShortcut>)
            },
        );
        let normalized = crate::tags_db::normalize_tag_display_name(input.trim());
        let valid = tag_panel_input_valid(&normalized);
        let add_clicked = ui.add_enabled(valid, egui::Button::new("付ける")).clicked();
        let enter_pressed = input_resp.has_focus() && enter_pressed;
        if valid && (add_clicked || enter_pressed) {
            *set_tag = Some((normalized.clone(), true, row.targets.clone()));
            input.clear();
            *focus_request = true;
            *close_after_apply = true;
        }
    });
    let normalized = crate::tags_db::normalize_tag_display_name(input.trim());
    let input_too_long = normalized.chars().count() > 64;
    let input_has_whitespace = crate::tags_db::tag_display_name_has_whitespace(&normalized);
    if input_too_long || input_has_whitespace {
        ui.label(
            egui::RichText::new(if input_too_long {
                "タグ名は64文字以内です。"
            } else {
                "タグ名に空白は使えません。"
            })
            .size(11.0)
            .color(egui::Color32::from_rgb(220, 120, 90)),
        );
    }

    let query_key = crate::tags_db::normalize_tag_key(&normalized);
    draw_tag_picker_tabs(ui, recent_tab);
    let mut choices = tag_panel_picker_choices(tag_catalog, &query_key, *recent_tab);
    choices.truncate(12);
    if choices.is_empty() {
        ui.label(egui::RichText::new("候補なし").size(11.0).color(DIM_COLOR));
        return;
    }

    for choice in choices {
        ui.horizontal(|ui| {
            let tag = format!("#{}", choice.name);
            let label_w = (ui.available_width() - 124.0).max(96.0);
            // タグ名は固定幅 label_w を確保しつつ左揃えにする。`add_sized` は中央寄せ +
            // 引き伸ばしになり、行ごとに `#` の x がずれて読みづらい。`set_min_width` で
            // label_w を必ず消費し、右側の件数 / ボタン列の右揃えを保つ。
            let resp = ui
                .allocate_ui_with_layout(
                    egui::vec2(label_w, 20.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_width(label_w);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&tag)
                                    .monospace()
                                    .color(egui::Color32::from_rgb(246, 248, 252)),
                            )
                            .truncate(),
                        )
                    },
                )
                .inner;
            resp.on_hover_text(tag);
            let meta = if choice.count > 0 {
                format!("{}件", choice.count)
            } else if choice.pinned {
                "ピン".to_string()
            } else {
                String::new()
            };
            ui.add_sized(
                [34.0, 20.0],
                egui::Label::new(
                    egui::RichText::new(meta)
                        .size(11.0)
                        .color(egui::Color32::from_rgb(188, 198, 214)),
                ),
            );
            let state = tag_button_state(row, &choice.tag_key);
            let add = state != TagButtonState::On;
            let label = if add { "付ける" } else { "外す" };
            if ui.button(label).clicked() {
                *set_tag = Some((choice.name.clone(), add, row.targets.clone()));
                *close_after_apply = true;
            }
        });
    }
}

fn tag_panel_input_valid(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 64
        && !crate::tags_db::tag_display_name_has_whitespace(name)
}

fn tag_button_visuals(state: TagButtonState) -> (egui::Color32, egui::Color32, egui::Color32) {
    match state {
        TagButtonState::On => (
            egui::Color32::from_rgb(236, 255, 238),
            egui::Color32::from_rgba_unmultiplied(26, 108, 62, 244),
            egui::Color32::from_rgb(132, 236, 156),
        ),
        TagButtonState::Mixed => (
            egui::Color32::from_rgb(255, 244, 214),
            egui::Color32::from_rgba_unmultiplied(118, 78, 20, 244),
            egui::Color32::from_rgb(255, 190, 92),
        ),
        TagButtonState::Off => (
            egui::Color32::from_rgb(248, 250, 255),
            egui::Color32::from_rgba_unmultiplied(35, 38, 50, 244),
            egui::Color32::from_rgb(132, 146, 174),
        ),
    }
}

fn draw_tag_picker_tabs(ui: &mut egui::Ui, recent_tab: &mut bool) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if tag_picker_tab_button(ui, "ピン留め", !*recent_tab).clicked() {
            *recent_tab = false;
        }
        if tag_picker_tab_button(ui, "最近", *recent_tab).clicked() {
            *recent_tab = true;
        }
    });
    ui.add_space(2.0);
}

fn tag_picker_tab_button(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let text = egui::RichText::new(label).color(if selected {
        egui::Color32::from_rgb(248, 252, 255)
    } else {
        egui::Color32::from_rgb(202, 210, 224)
    });
    ui.add(
        egui::Button::new(text)
            .fill(if selected {
                egui::Color32::from_rgb(58, 84, 116)
            } else {
                egui::Color32::from_rgb(42, 46, 56)
            })
            .stroke(egui::Stroke::new(
                1.0,
                if selected {
                    egui::Color32::from_rgb(142, 176, 218)
                } else {
                    egui::Color32::from_rgb(82, 92, 112)
                },
            )),
    )
}

fn draw_metadata_panel_tabs(ui: &mut egui::Ui, tab: &mut MetadataPanelTab) {
    const TAB_HEIGHT: f32 = 24.0;
    const TAB_GAP: f32 = 4.0;
    let width = ui.available_width().max(1.0);
    let (row_rect, _) = ui.allocate_exact_size(egui::vec2(width, TAB_HEIGHT), egui::Sense::hover());
    let tab_width = ((row_rect.width() - TAB_GAP) / 2.0).max(1.0);
    let info_rect = egui::Rect::from_min_size(row_rect.min, egui::vec2(tab_width, TAB_HEIGHT));
    let similar_rect = egui::Rect::from_min_size(
        egui::pos2(info_rect.right() + TAB_GAP, row_rect.top()),
        egui::vec2(tab_width, TAB_HEIGHT),
    );
    if crate::ui_helpers::draw_panel_tab_button(
        ui,
        info_rect,
        "metadata_panel_info_tab",
        "情報",
        *tab == MetadataPanelTab::Info,
    )
    .clicked()
    {
        *tab = MetadataPanelTab::Info;
    }
    if crate::ui_helpers::draw_panel_tab_button(
        ui,
        similar_rect,
        "metadata_panel_similar_tab",
        "類似",
        *tab == MetadataPanelTab::Similar,
    )
    .clicked()
    {
        *tab = MetadataPanelTab::Similar;
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn draw_similar_panel(
    ui: &mut egui::Ui,
    views: &[SimilarPageView<'_>],
    book: Option<&crate::similar_index::BookQuery>,
    results_are_stale: bool,
    state: &mut SimilarPanelState,
    thumb_px: u32,
    thumb_quality: u8,
    cache_decision: crate::thumb_loader::CacheDecision,
    pdf_passwords: Option<&crate::pdf_passwords::PdfPasswordStore>,
    pinned_item_key: Option<&str>,
    compare_pin_preparing: bool,
    ctx: &egui::Context,
    actions: &mut SimilarPanelActions,
) {
    if results_are_stale
        && views.iter().any(|view| {
            !matches!(
                view.model,
                SimilarPanelModel::NoIndex
                    | SimilarPanelModel::Preparing
                    | SimilarPanelModel::Failed(_)
            )
        })
    {
        ui.label(
            egui::RichText::new("索引を更新中です。変更は更新完了後に結果へ反映されます")
                .color(DIM_COLOR)
                .size(11.0),
        );
        ui.add_space(8.0);
    }
    // 本の関係を先に出す。これは本ごとに決まるので読み進めても動かない。ページの結果は
    // 件数が変わるので、後ろに置かないと本の節が上下に動いてしまう。
    if let Some(book) = book {
        let shown_keys = views
            .iter()
            .filter_map(|view| view.item_key)
            .collect::<Vec<_>>();
        draw_book_relations(
            ui,
            book,
            &shown_keys,
            state,
            thumb_px,
            thumb_quality,
            cache_decision,
            pdf_passwords,
            ctx,
            actions,
        );
    }
    for (index, view) in views.iter().enumerate() {
        if let Some(heading) = view.heading {
            if index > 0 {
                ui.add_space(10.0);
            }
            ui.label(
                egui::RichText::new(heading)
                    .color(egui::Color32::WHITE)
                    .size(14.0)
                    .strong(),
            );
            ui.add_space(4.0);
        }
        draw_similar_page_results(
            ui,
            view,
            state,
            thumb_px,
            thumb_quality,
            cache_decision,
            pdf_passwords,
            pinned_item_key,
            compare_pin_preparing,
            ctx,
            actions,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_similar_page_results(
    ui: &mut egui::Ui,
    view: &SimilarPageView<'_>,
    state: &mut SimilarPanelState,
    thumb_px: u32,
    thumb_quality: u8,
    cache_decision: crate::thumb_loader::CacheDecision,
    pdf_passwords: Option<&crate::pdf_passwords::PdfPasswordStore>,
    pinned_item_key: Option<&str>,
    compare_pin_preparing: bool,
    ctx: &egui::Context,
    actions: &mut SimilarPanelActions,
) {
    let compare_state = |hit: &crate::similar_index::QueryHit| {
        if pinned_item_key == Some(hit.item_key.as_str()) {
            SimilarCompareState::Pinned
        } else if compare_pin_preparing {
            SimilarCompareState::Preparing
        } else {
            SimilarCompareState::Idle
        }
    };
    let showing_previous = view.showing_previous;
    match view.model {
        SimilarPanelModel::NoIndex => {
            draw_similar_state_message(
                ui,
                "索引がありません",
                Some(SIMILAR_INDEX_NO_STORE_GUIDANCE),
            );
            if ui.button("お気に入りの設定を開く").clicked() {
                actions.open_favorites = true;
            }
        }
        SimilarPanelModel::Preparing => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("準備中");
            });
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        SimilarPanelModel::NotIndexed => {
            draw_similar_state_message(
                ui,
                "この画像は索引に含まれていません",
                Some(SIMILAR_INDEX_MISSING_ITEM_GUIDANCE),
            );
            if ui.button("お気に入りの設定を開く").clicked() {
                actions.open_favorites = true;
            }
        }
        SimilarPanelModel::Featureless => {
            draw_similar_state_message(
                ui,
                "この画像は特徴が少ないため判定できません",
                Some(
                    "色や模様の情報が乏しい画像 (単色・ほぼ白紙など) は、別の画像と区別できないため対象外です。",
                ),
            );
        }
        SimilarPanelModel::Empty => {
            draw_similar_state_message(ui, "別バージョンは見つかりませんでした", None);
        }
        SimilarPanelModel::Failed(error) => {
            draw_similar_state_message(ui, "索引を読み込めませんでした", Some(error));
        }
        SimilarPanelModel::Results(matches) => {
            let origin = &matches.origin;
            let origin_path = similar_path_parts(origin.target.as_ref(), &origin.item_key);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("表示中")
                        .color(egui::Color32::WHITE)
                        .size(14.0)
                        .strong(),
                );
                if showing_previous {
                    ui.label(egui::RichText::new("(更新中)").color(DIM_COLOR).size(10.0));
                }
            });
            ui.add_space(4.0);
            let origin_response = draw_similar_card(
                ui,
                state.thumbnail_state(&origin.item_key),
                &similar_origin_line(origin),
                &origin_path,
                None,
            );
            if ui.is_rect_visible(origin_response.rect) {
                state.ensure_thumbnail_for(
                    &origin.item_key,
                    origin.target.clone(),
                    origin.mtime,
                    origin.file_size,
                    thumb_px,
                    thumb_quality,
                    cache_decision,
                    pdf_passwords,
                    ctx,
                );
                state.mark_thumbnail_demand(&origin.item_key, &mut actions.thumbnail_demand);
            }
            ui.add_space(10.0);

            let mut previous_band = None;
            for hit in &matches.hits {
                if previous_band != Some(hit.band) {
                    if previous_band.is_some() {
                        ui.add_space(8.0);
                    }
                    let heading = match hit.band {
                        crate::similar_index::MatchBand::NearlyIdentical => "ほぼ同一",
                        crate::similar_index::MatchBand::OtherVersion => "別バージョン",
                    };
                    ui.label(
                        egui::RichText::new(heading)
                            .color(egui::Color32::WHITE)
                            .size(14.0)
                            .strong(),
                    );
                    ui.add_space(4.0);
                    previous_band = Some(hit.band);
                }

                let response = draw_similar_card(
                    ui,
                    state.thumbnail_state(&hit.item_key),
                    &similar_difference_line(hit, origin),
                    &similar_path_parts(hit.target.as_ref(), &hit.item_key),
                    Some(&origin_path),
                )
                .interact(egui::Sense::click());
                // Query results can exceed the 512-entry cache. Admit only rows that can be
                // painted; admitting every clipped row each frame would evict the whole previous
                // frame before any worker completion could still match its request id.
                if ui.is_rect_visible(response.rect) {
                    state.ensure_thumbnail(
                        hit,
                        thumb_px,
                        thumb_quality,
                        cache_decision,
                        pdf_passwords,
                        ctx,
                    );
                    state.mark_thumbnail_demand(&hit.item_key, &mut actions.thumbnail_demand);
                }
                let response = response.on_hover_text("クリックでこの画像へ移動");
                response.context_menu(|ui| {
                    if ui.button("パスをコピー").clicked() {
                        ctx.copy_text(similar_copy_path_text(hit));
                        ui.close();
                    }
                });
                if response.clicked() {
                    actions.open_hit = Some(hit.clone());
                }
                draw_similar_hit_buttons(ui, hit, compare_state(hit), actions);
                ui.add_space(6.0);
            }
        }
    }
}

/// 「この本と重なる本」。
///
/// 本を開いていないときは何も出さない。`NotBook` は失敗ではなく「この画像は本のページでは
/// ない」という状態なので、文言を出して場所を取ることはしない。
#[allow(clippy::too_many_arguments)]
fn draw_book_relations(
    ui: &mut egui::Ui,
    book: &crate::similar_index::BookQuery,
    // shown_item_keys: いま画面に出ているページ。見開きなら 2 つ。帯にはそれぞれ ▼ を出す。
    shown_item_keys: &[&str],
    state: &mut SimilarPanelState,
    thumb_px: u32,
    thumb_quality: u8,
    cache_decision: crate::thumb_loader::CacheDecision,
    pdf_passwords: Option<&crate::pdf_passwords::PdfPasswordStore>,
    ctx: &egui::Context,
    actions: &mut SimilarPanelActions,
) {
    use crate::similar_index::BookQuery;

    if matches!(book, BookQuery::NotBook | BookQuery::NotIndexed) {
        return;
    }
    ui.label(
        egui::RichText::new("この本と重なる本")
            .color(egui::Color32::WHITE)
            .size(14.0)
            .strong(),
    );
    ui.add_space(4.0);
    match book {
        BookQuery::NotBook | BookQuery::NotIndexed => {}
        BookQuery::Preparing => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("調べています");
            });
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        BookQuery::Featureless => {
            ui.label(
                egui::RichText::new("この本は特徴の少ないページばかりで判定できません")
                    .color(DIM_COLOR)
                    .size(11.0),
            );
        }
        BookQuery::Failed(error) => {
            ui.label(
                egui::RichText::new(format!("調べられませんでした: {error}"))
                    .color(DIM_COLOR)
                    .size(11.0),
            );
        }
        BookQuery::Ready(relations) if relations.hits.is_empty() => {
            ui.label(
                egui::RichText::new("重なる本は見つかりませんでした")
                    .color(DIM_COLOR)
                    .size(11.0),
            );
        }
        BookQuery::Ready(relations) => {
            draw_page_strip_legend(ui);

            ui.add_space(6.0);
            let current_pages = shown_item_keys
                .iter()
                .filter_map(|key| {
                    relations
                        .origin_page_keys
                        .iter()
                        .position(|page| page == key)
                })
                .collect::<Vec<_>>();
            for hit in &relations.hits {
                ui.label(
                    egui::RichText::new(book_relation_name(&hit.other_container_key))
                        .color(TEXT_COLOR)
                        .size(11.0),
                );
                ui.label(
                    egui::RichText::new(book_relation_line(hit))
                        .color(DIM_COLOR)
                        .size(10.0),
                );
                ui.add_space(3.0);
                // 単体画像の結果と同じ語彙にする。あちらに [移動] があってこちらに無いと、
                // 本へ移る手段が帯のクリックだけになり、操作が別物に見える。
                if ui
                    .small_button("移動")
                    .on_hover_text("重なりが始まるページを、相手の本で開く")
                    .clicked()
                    && let Some(opened) = book_open_target(hit)
                {
                    actions.open_page = Some(opened);
                }
                ui.add_space(3.0);
                draw_page_strip(
                    ui,
                    hit,
                    &current_pages,
                    state,
                    thumb_px,
                    thumb_quality,
                    cache_decision,
                    pdf_passwords,
                    ctx,
                    actions,
                );
                ui.add_space(8.0);
            }
        }
    }
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(8.0);
}

fn book_relation_name(container_key: &str) -> String {
    container_key
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(container_key)
        .to_owned()
}

/// 関係の要約。**「消してよい」とは書かない** (§0)。何ページ重なっていて、それぞれの本の
/// どれだけを占めるかという事実だけを出す。
fn book_relation_line(hit: &crate::similar_index::BookRelationHit) -> String {
    let relation = match &hit.pair.relation {
        crate::dupe::book::Relation::Same => "ほぼ同じ内容",
        crate::dupe::book::Relation::Contains { .. } => "片方がもう片方を含む",
        crate::dupe::book::Relation::Unrelated => "部分的に重なる",
        crate::dupe::book::Relation::Undecidable => "判定できる材料が足りない",
    };
    format!(
        "{relation} / {} ページ一致 / この本の {:.0}% ・相手の {:.0}%\nこの本 {} ページ ・相手 {} ページ",
        hit.pair.matched,
        hit.pair.coverage_a * 100.0,
        hit.pair.coverage_b * 100.0,
        hit.pages.len(),
        hit.other_page_count
    )
}

/// 帯の色の意味。**濃さの違いは説明が無いと読めない。**
fn draw_page_strip_legend(ui: &mut egui::Ui) {
    use crate::similar_index::BookPageState;

    let swatch = |ui: &mut egui::Ui, state: BookPageState, label: &str, hover: &str| {
        let (rect, response) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 1.0, page_state_color(state));
        response.on_hover_text(hover);
        ui.label(egui::RichText::new(label).color(DIM_COLOR).size(10.0));
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 2.0);
        swatch(
            ui,
            BookPageState::Strong,
            "ほぼ同一",
            "対応するページがあり、見た目もほぼ同じ",
        );
        swatch(
            ui,
            BookPageState::Weak,
            "別バージョン",
            "対応するページはあるが、解像度や修正で見た目が違う",
        );
        swatch(
            ui,
            BookPageState::Unmatched,
            "対応なし",
            "相手の本に対応するページが無い",
        );
        swatch(
            ui,
            BookPageState::Excluded,
            "対象外",
            "特徴が少ないか、どの本にもあるページ。割合の分母からも外れている",
        );
    });
}

/// 本の [移動] が開くページ。
///
/// **重なりが始まるページ**を選ぶ。相手の本の 1 ページ目ではない — 単話が総集編の 34 ページ
/// 目から入っているとき、開きたいのは表紙ではなくそこである。
fn book_open_target(
    hit: &crate::similar_index::BookRelationHit,
) -> Option<(String, crate::similar_index::SimilarItemTarget)> {
    let page = hit.pages.iter().find(|page| page.other_target.is_some())?;
    Some((page.other_item_key.clone()?, page.other_target.clone()?))
}

const STRIP_HEIGHT: f32 = 16.0;

fn page_state_color(state: crate::similar_index::BookPageState) -> egui::Color32 {
    use crate::similar_index::BookPageState;
    match state {
        BookPageState::Strong => egui::Color32::from_rgb(86, 156, 214),
        BookPageState::Weak => egui::Color32::from_rgb(78, 105, 130),
        // 帯の地色 (28,28,28) から見分けられる明るさにする。暗くしすぎると「何も無い」と
        // 区別が付かず、除外したページ数を隠したのと同じになる。
        BookPageState::Unmatched => egui::Color32::from_rgb(72, 72, 72),
        // 採点対象外は不一致の「濃い版」ではなく別種なので、明るさではなく色味で分ける。
        BookPageState::Excluded => egui::Color32::from_rgb(84, 74, 56),
    }
}

/// 帯 1 本ぶんの集約結果。
struct StripColumns {
    /// コマごとに出す状態。ページが 1 つも入らないコマは `None`。
    strongest: Vec<Option<crate::similar_index::BookPageState>>,
    /// そのコマで最初に移動先を持つページ。押したときの行き先になる。
    first_target: Vec<Option<usize>>,
}

/// ページを表示幅のコマへ畳む。
///
/// 1 コマに複数ページが入るときは**一番強い状態を出す**。対応が連続していれば塗りが続き、
/// たまたま数ページ一致しただけなら細い線として残る — この見分けが判定そのものになる
/// (§14.2)。弱い側に寄せて平均を取ると、その区別が消える。
fn summarize_page_strip(
    pages: &[crate::similar_index::BookPageMatch],
    columns: usize,
) -> StripColumns {
    use crate::similar_index::BookPageState;

    let mut strongest = vec![None::<BookPageState>; columns];
    let mut first_target = vec![None::<usize>; columns];
    if pages.is_empty() || columns == 0 {
        return StripColumns {
            strongest,
            first_target,
        };
    }
    let rank = |state: BookPageState| match state {
        BookPageState::Strong => 3,
        BookPageState::Weak => 2,
        BookPageState::Unmatched => 1,
        BookPageState::Excluded => 0,
    };
    // コマ側から回す。ページ数が幅より多いときは 1 コマが範囲を受け持ち、少ないときは
    // 1 ページが複数のコマにまたがる。ページ側から回すと後者で空のコマが残り、帯が縞に
    // なって「対応が途切れている」ように見えてしまう。
    for column in 0..columns {
        let start = column * pages.len() / columns;
        let end = (((column + 1) * pages.len()) / columns)
            .max(start + 1)
            .min(pages.len());
        for (page, entry) in pages.iter().enumerate().take(end).skip(start) {
            if strongest[column].is_none_or(|current| rank(entry.state) > rank(current)) {
                strongest[column] = Some(entry.state);
            }
            if entry.other_target.is_some() && first_target[column].is_none() {
                first_target[column] = Some(page);
            }
        }
    }
    StripColumns {
        strongest,
        first_target,
    }
}

/// 1 冊分のページを 1 行の帯にする。
///
/// 200〜600 ページを 300 px 弱に収めるので 1 ページが 1 px を切る。1 コマに複数ページが
/// 入るときは**一番強い状態を出す**。対応が連続していれば塗りが続き、たまたま数ページ
/// 一致しただけなら細い線として残る — この見分けが判定そのものになる (§14.2)。
#[allow(clippy::too_many_arguments)]
fn draw_page_strip(
    ui: &mut egui::Ui,
    hit: &crate::similar_index::BookRelationHit,
    current_pages: &[usize],
    state: &mut SimilarPanelState,
    thumb_px: u32,
    thumb_quality: u8,
    cache_decision: crate::thumb_loader::CacheDecision,
    pdf_passwords: Option<&crate::pdf_passwords::PdfPasswordStore>,
    ctx: &egui::Context,
    actions: &mut SimilarPanelActions,
) {
    if hit.pages.is_empty() {
        return;
    }
    let width = ui.available_width().max(1.0);
    let (outer, response) = ui.allocate_exact_size(
        egui::vec2(width, STRIP_HEIGHT + STRIP_MARKER_HEIGHT),
        egui::Sense::click(),
    );
    let rect = egui::Rect::from_min_size(
        egui::pos2(outer.left(), outer.top() + STRIP_MARKER_HEIGHT),
        egui::vec2(width, STRIP_HEIGHT),
    );
    let painter = ui.painter_at(outer);
    painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(28, 28, 28));

    let columns = width.floor().max(1.0) as usize;
    let StripColumns {
        strongest,
        first_target,
    } = summarize_page_strip(&hit.pages, columns);
    // 1 ページが 3 px 以上取れるなら、ページごとの区画として描く。ページ数を数えられ、
    // ▼ がどの区画を指しているのかが分かる。取れないときだけ 1 px のコマへ畳む。
    let per_page = width / hit.pages.len() as f32;
    if per_page >= STRIP_MIN_CELL {
        for (page, entry) in hit.pages.iter().enumerate() {
            let left = rect.left() + strip_page_left(width, hit.pages.len(), page);
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(left, rect.top()),
                    egui::vec2((per_page - 1.0).max(1.0), STRIP_HEIGHT),
                ),
                0.0,
                page_state_color(entry.state),
            );
        }
    } else {
        for (column, state) in strongest.iter().enumerate() {
            let Some(state) = state else {
                continue;
            };
            let x = rect.left() + column as f32;
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(1.0, STRIP_HEIGHT)),
                0.0,
                page_state_color(*state),
            );
        }
    }

    // いま見ているページの位置。これが無いと、帯のどこに自分がいるのか分からない。
    // 見開きなら 2 ページとも指す。片方だけだと、もう片方を見落とす。
    for page in current_pages {
        let x = rect.left() + strip_page_center(width, hit.pages.len(), *page);
        let tip = egui::pos2(x, rect.top() - 1.0);
        painter.add(egui::Shape::convex_polygon(
            vec![
                tip,
                egui::pos2(x - 4.0, tip.y - STRIP_MARKER_HEIGHT + 2.0),
                egui::pos2(x + 4.0, tip.y - STRIP_MARKER_HEIGHT + 2.0),
            ],
            egui::Color32::WHITE,
            egui::Stroke::NONE,
        ));
    }

    let Some(pos) = response.hover_pos() else {
        return;
    };
    let column = ((pos.x - rect.left()).floor().max(0.0) as usize).min(columns - 1);
    // 押したときに対象になるページ範囲を、そのまま囲う。カーソル中心の固定幅にすると、
    // 1 ページが十数 px ある帯で枠だけが細く残り、どのページを指しているのか分からない。
    let pages = hit.pages.len();
    let first = column * pages / columns;
    let last = (((column + 1) * pages) / columns).max(first + 1).min(pages);
    let left = strip_page_left(width, pages, first);
    let right = strip_page_left(width, pages, last).max(left + STRIP_MIN_CELL);
    painter.rect_stroke(
        egui::Rect::from_min_size(
            egui::pos2(rect.left() + left, rect.top()),
            egui::vec2(right - left, STRIP_HEIGHT),
        ),
        0.0,
        egui::Stroke::new(1.0, egui::Color32::WHITE),
        egui::StrokeKind::Inside,
    );
    let Some(page) = first_target[column] else {
        response.on_hover_text("ここに対応するページはありません");
        return;
    };
    let entry = &hit.pages[page];
    let Some(item_key) = entry.other_item_key.clone() else {
        return;
    };
    // 行のサムネイルと同じ経路で読む。飛ぶ前にどのページなのかを見せる。
    state.ensure_thumbnail_for(
        &item_key,
        entry.other_target.clone(),
        entry.other_mtime,
        entry.other_file_size,
        thumb_px,
        thumb_quality,
        cache_decision,
        pdf_passwords,
        ctx,
    );
    state.mark_thumbnail_demand(&item_key, &mut actions.thumbnail_demand);
    let thumb = state.thumbnail_state(&item_key);
    let caption = format!(
        "この本の {} ページ目 → 相手の {} ページ目",
        page + 1,
        entry.other_page_index.map_or(0, |index| index + 1)
    );
    response.clone().on_hover_ui(|ui| {
        ui.set_max_width(STRIP_PREVIEW_SIZE);
        ui.label(egui::RichText::new(caption).size(11.0));
        match thumb {
            Some(SimilarThumbState::Ready(texture)) => {
                let size = texture.size_vec2();
                let scale = (STRIP_PREVIEW_SIZE / size.x.max(size.y)).min(1.0);
                ui.add(egui::Image::new(&texture).fit_to_exact_size(size * scale));
            }
            Some(SimilarThumbState::Loading) | None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new("読込中").size(10.0));
                });
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Some(SimilarThumbState::Failed) => {
                ui.label(egui::RichText::new("画像なし").size(10.0));
            }
        }
    });
    if response.clicked()
        && let Some(target) = entry.other_target.clone()
    {
        actions.open_page = Some((item_key, target));
    }
}

/// いま見ているページを指す ▼ の高さ。帯の上に確保する。
const STRIP_MARKER_HEIGHT: f32 = 7.0;

/// ページごとの区画として描くのに要る幅。これを割ると隙間が取れず、区画が数えられない。
const STRIP_MIN_CELL: f32 = 3.0;

/// 帯の中でそのページの区画が始まる位置。
fn strip_page_left(width: f32, pages: usize, page: usize) -> f32 {
    width / pages.max(1) as f32 * page as f32
}

/// 帯の中でそのページが占める区画の中心。▼ はここを指す。
fn strip_page_center(width: f32, pages: usize, page: usize) -> f32 {
    let per_page = width / pages.max(1) as f32;
    strip_page_left(width, pages, page) + per_page / 2.0
}

/// ホバー時に出すページの一辺。行のサムネイル (72px) より大きくして、飛ぶ前に中身が分かる
/// 程度にする。
const STRIP_PREVIEW_SIZE: f32 = 180.0;

/// 「長押し表示」が 1 フレームでどう動くか。押している対象は key で見分ける。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SimilarPeekTransition {
    /// 覗いておらず、押されてもいない。
    Idle,
    Start,
    Continue,
    Stop,
    /// 押したまま別の候補へ移った。前の表示を戻してから新しい方を覗く。
    Switch,
}

fn decide_similar_peek(current: Option<&str>, held: Option<&str>) -> SimilarPeekTransition {
    match (current, held) {
        (None, None) => SimilarPeekTransition::Idle,
        (None, Some(_)) => SimilarPeekTransition::Start,
        (Some(current), Some(held)) if current == held => SimilarPeekTransition::Continue,
        (Some(_), Some(_)) => SimilarPeekTransition::Switch,
        (Some(_), None) => SimilarPeekTransition::Stop,
    }
}

/// 索引が無い / 対象外のときに出す案内。
///
/// **どうすれば使えるようになるかを書く。** 「含まれていません」だけでは、それが設定で
/// 変えられることも、どこで変えるのかも分からない。
const SIMILAR_INDEX_NO_STORE_GUIDANCE: &str = "この機能はお気に入りに登録した場所で使えます。お気に入りの設定で「別バージョン索引」を \
     有効にすると、その場所の画像が対象になります。";

/// 索引はあるが、この画像が入っていないときの案内。
///
/// **原因を断定しない。** 場所が対象外なのか、対象だがまだ登録されていないのかは、この状態
/// からは区別できない。断定すると片方の場合に嘘になる。
const SIMILAR_INDEX_MISSING_ITEM_GUIDANCE: &str = "この場所が「別バージョン索引」の対象になっていないか、まだ登録されていない画像です。 \
     お気に入りの設定で、対象の場所と索引の状態を確認できます。";

/// 結果が無いときの状態表示。
///
/// 既定の `ui.label` は暗色パネル上で読みづらい色になる。結果一覧と同じ色使いに揃える。
fn draw_similar_state_message(ui: &mut egui::Ui, headline: &str, detail: Option<&str>) {
    ui.label(egui::RichText::new(headline).color(TEXT_COLOR).size(12.0));
    if let Some(detail) = detail {
        ui.add_space(3.0);
        ui.label(egui::RichText::new(detail).color(DIM_COLOR).size(11.0));
        ui.add_space(3.0);
    }
}

/// 1 件ぶんの操作ボタン。/// 1 件ぶんの操作ボタン。
///
/// 以前はホバー中に X を押す設計だったが、mIV に「ホバーしたまま打鍵する」操作は他に無く、
/// どの候補が対象なのか画面から読み取れなかった。押した対象が曖昧にならないボタンにする。
fn draw_similar_hit_buttons(
    ui: &mut egui::Ui,
    hit: &crate::similar_index::QueryHit,
    state: SimilarCompareState,
    actions: &mut SimilarPanelActions,
) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if ui
            .small_button("移動")
            .on_hover_text("この画像を開く")
            .clicked()
        {
            actions.open_hit = Some(hit.clone());
        }

        let (label, hover) = match state {
            SimilarCompareState::Pinned => (
                "比較解除",
                "比較画像の設定を解除する".to_owned(),
            ),
            SimilarCompareState::Preparing => ("準備中", "比較画像を準備しています".to_owned()),
            SimilarCompareState::Idle => (
                "比較に設定",
                "比較画像に設定する。設定後は C で表示、Shift+C でワイプ、Alt+C で差分".to_owned(),
            ),
        };
        let pin = ui.add_enabled(
            !matches!(state, SimilarCompareState::Preparing),
            egui::Button::new(label).small(),
        );
        if pin.on_hover_text(hover).clicked() {
            actions.pin_hit = Some(hit.clone());
        }

        let peek = ui
            .add(egui::Button::new("長押し表示").small())
            .on_hover_text("押している間だけこの画像を表示する。まだ比較画像でない場合は、読み込めた時点で表示に切り替わる");
        if peek.is_pointer_button_down_on() {
            actions.peek_held = Some(hit.clone());
        }
    });
}

/// Visual fixture for the production metadata-panel tabs and similar-result rows.
#[doc(hidden)]
/// スナップショット用の本の関係。**連続した収録**と**散発的な一致**の両方を入れて、
/// 帯がその区別を保っていることを目で見て確かめられるようにする。
fn book_snapshot_fixture() -> crate::similar_index::BookQuery {
    use crate::similar_index::{BookPageMatch, BookPageState};

    let page = |state: BookPageState, other: Option<u32>| BookPageMatch {
        state,
        other_mtime: 0,
        other_file_size: 0,
        other_page_index: other,
        other_target: other.map(|_| {
            crate::similar_index::SimilarItemTarget::File(PathBuf::from(r"D:\Archive\other.png"))
        }),
        other_item_key: other.map(|_| "d:/archive/other.png".to_string()),
    };
    let contiguous = (0..180u32)
        .map(|index| match index {
            0..=3 => page(BookPageState::Excluded, None),
            34..=110 => page(BookPageState::Strong, Some(index - 34)),
            111..=118 => page(BookPageState::Weak, Some(index - 34)),
            _ => page(BookPageState::Unmatched, None),
        })
        .collect::<Vec<_>>();
    let scattered = (0..180u32)
        .map(|index| {
            if index % 23 == 0 {
                page(BookPageState::Strong, Some(index))
            } else {
                page(BookPageState::Unmatched, None)
            }
        })
        .collect::<Vec<_>>();
    crate::similar_index::BookQuery::Ready(crate::similar_index::BookRelations {
        origin_page_keys: (0..180)
            .map(|index| format!("c:/books/this/{index:03}.png"))
            .collect(),
        hits: vec![
            crate::similar_index::BookRelationHit {
                other_container_key: r"E:\books\総集編.zip".to_string(),
                other_page_count: 402,
                pair: crate::dupe::book::BookPair {
                    a: 1,
                    b: 2,
                    matched: 85,
                    distinctive_a: 176,
                    distinctive_b: 402,
                    coverage_a: 0.483,
                    coverage_b: 0.211,
                    relation: crate::dupe::book::Relation::Contains { whole: 2 },
                    alignment: Vec::new(),
                },
                pages: contiguous,
            },
            crate::similar_index::BookRelationHit {
                other_container_key: r"E:\books\別作品.zip".to_string(),
                other_page_count: 190,
                pair: crate::dupe::book::BookPair {
                    a: 1,
                    b: 3,
                    matched: 8,
                    distinctive_a: 176,
                    distinctive_b: 190,
                    coverage_a: 0.045,
                    coverage_b: 0.042,
                    relation: crate::dupe::book::Relation::Unrelated,
                    alignment: Vec::new(),
                },
                pages: scattered,
            },
        ],
    })
}

/// 結果が無いときの各状態。**ここが撮られていないと、読めない色や案内の欠落に気付けない。**
/// 実際、索引に無い旨の一行は既定色のままで暗く沈んでいた。
pub fn draw_similar_states_snapshot_fixture(ui: &mut egui::Ui) {
    ui.set_width(360.0);
    apply_metadata_panel_dark_widget_style(ui);
    ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(28, 30, 36))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            let mut actions = SimilarPanelActions::default();
            let mut state = SimilarPanelState::default();
            let ctx = ui.ctx().clone();
            for model in [
                SimilarPanelModel::NoIndex,
                SimilarPanelModel::NotIndexed,
                SimilarPanelModel::Featureless,
                SimilarPanelModel::Empty,
                SimilarPanelModel::Failed("similar.db: database is locked"),
            ] {
                draw_similar_page_results(
                    ui,
                    &SimilarPageView {
                        heading: None,
                        item_key: None,
                        model,
                        showing_previous: false,
                    },
                    &mut state,
                    72,
                    85,
                    crate::thumb_loader::CacheDecision::without_thumbnail(),
                    None,
                    None,
                    false,
                    &ctx,
                    &mut actions,
                );
                ui.add_space(10.0);
            }
        });
}

pub fn draw_similar_panel_snapshot_fixture(ui: &mut egui::Ui, similar_selected: bool) {
    ui.set_width(360.0);
    apply_metadata_panel_dark_widget_style(ui);
    // 本番と同じ scroll 構成で描く。ここを省くと、**溝の有無で幅が動く**という当の症状が
    // スナップショットに出ない。
    ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(28, 30, 36))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            let outer_width = ui.available_width();
            egui::ScrollArea::vertical()
                .id_salt("metadata_snapshot_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(metadata_scroll_content_width(ui, outer_width));
                    let mut tab = if similar_selected {
                        MetadataPanelTab::Similar
                    } else {
                        MetadataPanelTab::Info
                    };
                    draw_metadata_panel_tabs(ui, &mut tab);
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(8.0);
                    if !similar_selected {
                        ui.label(
                            egui::RichText::new("画像情報")
                                .color(egui::Color32::WHITE)
                                .size(16.0)
                                .strong(),
                        );
                        draw_key_value_wrapped(ui, "ファイル", "sample.jpg");
                        draw_key_value_wrapped(ui, "サイズ", "1200×1600");
                        return;
                    }

                    let hits = vec![
                        crate::similar_index::QueryHit {
                            item_id: 2,
                            item_key: crate::similar_index::item_key_for_file(Path::new(
                                r"C:\Pictures\edits\sample.png",
                            )),
                            kind: crate::similar_db::ItemKind::Image,
                            container_key: None,
                            page_index: None,
                            distance: 5,
                            band: crate::similar_index::MatchBand::NearlyIdentical,
                            mtime: 1,
                            file_size: 4_404_019,
                            width: 2400,
                            height: 3200,
                            format: crate::similar_image::SimilarImageFormat::Png,
                            target: Some(crate::similar_index::SimilarItemTarget::File(
                                PathBuf::from(r"C:\Pictures\edits\sample.png"),
                            )),
                        },
                        crate::similar_index::QueryHit {
                            item_id: 3,
                            item_key: crate::similar_index::item_key_for_file(Path::new(
                                r"D:\Archive\sample.webp",
                            )),
                            kind: crate::similar_db::ItemKind::Image,
                            container_key: None,
                            page_index: None,
                            distance: 24,
                            band: crate::similar_index::MatchBand::OtherVersion,
                            mtime: 1,
                            file_size: 921_600,
                            width: 900,
                            height: 1200,
                            format: crate::similar_image::SimilarImageFormat::WebP,
                            target: Some(crate::similar_index::SimilarItemTarget::File(
                                PathBuf::from(r"D:\Archive\sample.webp"),
                            )),
                        },
                    ];
                    // 表示中のページと候補で、フォルダ名とファイル名が一部だけ違う組にする。
                    // 差分の色分けが効いているかを目で見て確かめられる。
                    let matches = crate::similar_index::ItemMatches {
                        origin: crate::similar_index::OriginItem {
                            item_key: "c:/pictures/edits/2024-05/sample.png".to_string(),
                            kind: crate::similar_db::ItemKind::Image,
                            mtime: 0,
                            file_size: 4_400_000,
                            width: 1200,
                            height: 1600,
                            format: crate::similar_image::SimilarImageFormat::Png,
                            target: Some(crate::similar_index::SimilarItemTarget::File(
                                PathBuf::from(r"C:\Pictures\edits\2024-05\sample.png"),
                            )),
                        },
                        hits,
                    };
                    let mut state = SimilarPanelState::default();
                    state.insert_failed_snapshot_thumbnail(
                        matches.origin.item_key.clone(),
                        matches.origin.target.clone(),
                        matches.origin.mtime,
                        matches.origin.file_size,
                        72,
                        85,
                    );
                    for hit in &matches.hits {
                        state.insert_failed_snapshot_thumbnail(
                            hit.item_key.clone(),
                            hit.target.clone(),
                            hit.mtime,
                            hit.file_size,
                            72,
                            85,
                        );
                    }
                    let mut actions = SimilarPanelActions::default();
                    let ctx = ui.ctx().clone();
                    let book = book_snapshot_fixture();
                    let current_page = "c:/books/this/060.png".to_string();
                    let views = [SimilarPageView {
                        heading: None,
                        item_key: Some(current_page.as_str()),
                        model: SimilarPanelModel::Results(&matches),
                        showing_previous: false,
                    }];
                    draw_similar_panel(
                        ui,
                        &views,
                        Some(&book),
                        true,
                        &mut state,
                        72,
                        85,
                        crate::thumb_loader::CacheDecision::without_thumbnail(),
                        None,
                        Some(matches.hits[0].item_key.as_str()),
                        false,
                        &ctx,
                        &mut actions,
                    );
                });
        });
}

fn tag_panel_picker_choices(
    tag_catalog: &[TagPanelChoice],
    query_key: &str,
    recent_tab: bool,
) -> Vec<TagPanelChoice> {
    let mut out = Vec::new();
    let mut seen = Vec::<String>::new();
    for choice in tag_catalog {
        if choice.tag_key.is_empty() || seen.iter().any(|key| key == &choice.tag_key) {
            continue;
        }
        if !query_key.is_empty() {
            if !choice.tag_key.starts_with(query_key) {
                continue;
            }
        } else if recent_tab {
            if choice.last_applied_at <= 0 {
                continue;
            }
        } else if !choice.pinned {
            continue;
        }
        seen.push(choice.tag_key.clone());
        out.push(choice.clone());
    }
    if !query_key.is_empty() {
        out.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| b.last_applied_at.cmp(&a.last_applied_at))
                .then_with(|| b.count.cmp(&a.count))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
    } else if recent_tab {
        out.sort_by(|a, b| {
            b.last_applied_at
                .cmp(&a.last_applied_at)
                .then_with(|| b.count.cmp(&a.count))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
    }
    out
}

fn tag_panel_visible_choices(
    pinned_tags: &[TagPanelChoice],
    rows: &[TagPanelRow],
    sticky_tags: &[(String, String)],
    tag_catalog: &[TagPanelChoice],
) -> Vec<TagPanelChoice> {
    let mut out = Vec::new();
    let mut seen = Vec::<String>::new();
    for choice in pinned_tags {
        push_tag_panel_choice(&mut out, &mut seen, choice.clone());
    }
    for (name, tag_key) in sticky_tags {
        push_tag_panel_choice(
            &mut out,
            &mut seen,
            tag_panel_choice_from_key(tag_catalog, tag_key, name, false),
        );
    }
    for row in rows {
        for tags in &row.tags_by_target {
            for tag in tags {
                let tag_key = crate::tags_db::normalize_tag_key(tag);
                if tag_key.is_empty() {
                    continue;
                }
                push_tag_panel_choice(
                    &mut out,
                    &mut seen,
                    tag_panel_choice_from_key(
                        tag_catalog,
                        &tag_key,
                        crate::tags_db::strip_display_hash(tag),
                        false,
                    ),
                );
            }
        }
    }
    out
}

fn push_tag_panel_choice(
    out: &mut Vec<TagPanelChoice>,
    seen: &mut Vec<String>,
    choice: TagPanelChoice,
) {
    if choice.tag_key.is_empty() || seen.iter().any(|key| key == &choice.tag_key) {
        return;
    }
    seen.push(choice.tag_key.clone());
    out.push(choice);
}

fn tag_panel_choice_from_key(
    tag_catalog: &[TagPanelChoice],
    tag_key: &str,
    fallback_name: &str,
    pinned: bool,
) -> TagPanelChoice {
    tag_catalog
        .iter()
        .find(|choice| choice.tag_key == tag_key)
        .cloned()
        .map(|mut choice| {
            choice.pinned |= pinned;
            choice
        })
        .unwrap_or_else(|| TagPanelChoice {
            name: fallback_name.to_string(),
            tag_key: tag_key.to_string(),
            count: 0,
            pinned,
            last_applied_at: 0,
        })
}

fn tag_panel_target_key(rows: &[TagPanelRow]) -> String {
    rows.iter()
        .map(tag_panel_row_key)
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

fn tag_panel_row_key(row: &TagPanelRow) -> String {
    row.targets
        .iter()
        .map(|target| crate::tags_db::item_key_for_path(&target.path))
        .collect::<Vec<_>>()
        .join("\u{1e}")
}

fn tag_button_state(row: &TagPanelRow, tag_key: &str) -> TagButtonState {
    let target_count = row.tags_by_target.len();
    if target_count == 0 {
        return TagButtonState::Off;
    }
    let tagged_count = row
        .tags_by_target
        .iter()
        .filter(|tags| {
            tags.iter()
                .any(|tag| crate::tags_db::normalize_tag_key(tag) == tag_key)
        })
        .count();
    if tagged_count == 0 {
        TagButtonState::Off
    } else if tagged_count == target_count {
        TagButtonState::On
    } else {
        TagButtonState::Mixed
    }
}

fn dedup_tag_targets(targets: &mut Vec<TagTarget>) {
    let mut seen: Vec<PathBuf> = Vec::new();
    targets.retain(|target| {
        if seen
            .iter()
            .any(|path| crate::folder_tree::path_eq(path, &target.path))
        {
            false
        } else {
            seen.push(target.path.clone());
            true
        }
    });
}

fn shared_parent_folder(paths: &[PathBuf]) -> Option<PathBuf> {
    let first_parent = paths.first()?.parent()?;
    if paths.iter().all(|path| {
        path.parent()
            .is_some_and(|p| crate::folder_tree::path_eq(p, first_parent))
    }) {
        Some(first_parent.to_path_buf())
    } else {
        None
    }
}

fn tag_path_display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

fn draw_a1111_panel(ui: &mut egui::Ui, ctx: &egui::Context, meta: &A1111Metadata) {
    // ヘッダー
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("AI Metadata")
                .color(egui::Color32::WHITE)
                .size(16.0)
                .strong(),
        );
        ui.label(
            egui::RichText::new("A1111")
                .color(egui::Color32::from_rgb(100, 180, 255))
                .size(12.0)
                .background_color(egui::Color32::from_rgba_unmultiplied(100, 180, 255, 30)),
        );
    });
    ui.add_space(8.0);

    // Prompt
    if !meta.prompt.is_empty() {
        draw_text_section(ui, ctx, "Prompt", &meta.prompt);
    }

    // Negative prompt
    if !meta.negative_prompt.is_empty() {
        ui.add_space(6.0);
        draw_text_section(ui, ctx, "Negative Prompt", &meta.negative_prompt);
    }

    // Parameters
    if !meta.params.is_empty() {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Parameters")
                .color(LABEL_COLOR)
                .size(SECTION_FONT),
        );
        ui.add_space(2.0);
        for (key, val) in &meta.params {
            draw_key_value_wrapped(ui, key, val);
        }
    }
}

fn draw_comfyui_panel(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    meta: &ComfyUIMetadata,
    show_raw_prompt: bool,
    show_raw_workflow: bool,
) -> (bool, bool) {
    let mut rp = show_raw_prompt;
    let mut rw = show_raw_workflow;

    // ヘッダー
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("AI Metadata")
                .color(egui::Color32::WHITE)
                .size(16.0)
                .strong(),
        );
        ui.label(
            egui::RichText::new("ComfyUI")
                .color(egui::Color32::from_rgb(120, 220, 120))
                .size(12.0)
                .background_color(egui::Color32::from_rgba_unmultiplied(120, 220, 120, 30)),
        );
    });
    ui.add_space(8.0);

    // Extracted prompts
    if !meta.extracted_prompts.is_empty() {
        let combined = meta.extracted_prompts.join("\n---\n");
        draw_text_section(ui, ctx, "Prompt", &combined);
    }

    // Extracted negatives
    if !meta.extracted_negatives.is_empty() {
        ui.add_space(6.0);
        let combined = meta.extracted_negatives.join("\n---\n");
        draw_text_section(ui, ctx, "Negative Prompt", &combined);
    }

    // Sampler parameters
    if !meta.sampler_params.is_empty() {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Parameters")
                .color(LABEL_COLOR)
                .size(SECTION_FONT),
        );
        ui.add_space(2.0);
        for (key, val) in &meta.sampler_params {
            draw_key_value_wrapped(ui, key, val);
        }
    }

    // Raw JSON sections (collapsible)
    ui.add_space(10.0);
    {
        let json_str = serde_json::to_string_pretty(&meta.prompt_json).unwrap_or_default();
        draw_collapsible_json_section(ui, ctx, "Raw Prompt JSON", &json_str, &mut rp);
    }

    if let Some(ref wf) = meta.workflow_json {
        ui.add_space(4.0);
        let json_str = serde_json::to_string_pretty(wf).unwrap_or_default();
        draw_collapsible_json_section(ui, ctx, "Raw Workflow JSON", &json_str, &mut rw);
    }

    (rp, rw)
}

fn draw_unknown_panel(ui: &mut egui::Ui, chunks: &[(String, String)]) {
    ui.label(
        egui::RichText::new("Metadata")
            .color(egui::Color32::WHITE)
            .size(16.0)
            .strong(),
    );
    ui.add_space(8.0);

    for (key, val) in chunks {
        ui.label(
            egui::RichText::new(key)
                .color(LABEL_COLOR)
                .size(SECTION_FONT),
        );
        ui.add_space(2.0);
        let display = if val.len() > 2000 {
            format!("{}...", &val[..2000])
        } else {
            val.clone()
        };
        ui.label(
            egui::RichText::new(display)
                .color(TEXT_COLOR)
                .font(crate::ui_fonts::user_text_font(BODY_FONT)),
        );
        ui.add_space(8.0);
    }
}

fn draw_no_metadata(ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new("Image Info")
            .color(egui::Color32::WHITE)
            .size(16.0)
            .strong(),
    );
    ui.add_space(20.0);
    ui.label(
        egui::RichText::new("No metadata found.")
            .color(DIM_COLOR)
            .size(BODY_FONT),
    );
}

fn draw_exif_panel(
    ui: &mut egui::Ui,
    exif: &ExifInfo,
    sections_open: &mut std::collections::HashMap<String, bool>,
) {
    ui.label(
        egui::RichText::new("EXIF")
            .color(egui::Color32::WHITE)
            .size(16.0)
            .strong(),
    );
    ui.add_space(6.0);

    for (group, fields) in &exif.sections {
        let key = format!("{:?}", group);
        let open = sections_open.entry(key).or_insert(true);
        let display_section = group.display_name();
        let header = if *open {
            format!("▼ {display_section}")
        } else {
            format!("▶ {display_section}")
        };
        if ui
            .selectable_label(
                *open,
                egui::RichText::new(&header)
                    .color(LABEL_COLOR)
                    .size(SECTION_FONT),
            )
            .clicked()
        {
            *open = !*open;
        }

        if *open {
            ui.add_space(2.0);
            for (tag_name, value) in fields {
                let display_tag = exif_reader::tag_display_name(tag_name);
                draw_key_value_wrapped(ui, display_tag, value);
            }
            ui.add_space(4.0);
        }
    }
}

/// キー: 値 を1つの LayoutJob で描画し、長い値も確実に折り返す。
fn draw_key_value_wrapped(ui: &mut egui::Ui, key: &str, val: &str) {
    if !crate::ui_text_links::find_http_urls(val).is_empty() {
        ui.horizontal_top(|ui| {
            ui.label(
                egui::RichText::new(format!("{key}:  "))
                    .font(egui::FontId::proportional(BODY_FONT))
                    .color(DIM_COLOR),
            );
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                draw_user_text_with_links(ui, val, BODY_FONT);
            });
        });
        return;
    }

    let mut job = egui::text::LayoutJob::default();
    job.wrap = egui::text::TextWrapping {
        max_width: ui.available_width(),
        ..Default::default()
    };
    job.append(
        &format!("{key}:  "),
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(BODY_FONT),
            color: DIM_COLOR,
            ..Default::default()
        },
    );
    job.append(
        val,
        0.0,
        egui::TextFormat {
            font_id: crate::ui_fonts::user_text_font(BODY_FONT),
            color: TEXT_COLOR,
            ..Default::default()
        },
    );
    ui.label(job);
}

fn draw_user_text_with_links(ui: &mut egui::Ui, text: &str, font_size: f32) {
    if let Some(url) = crate::ui_text_links::draw_text_with_links(
        ui,
        text,
        crate::ui_fonts::user_text_font(font_size),
        TEXT_COLOR,
        LINK_COLOR,
    ) {
        crate::ui_helpers::open_url(&url);
    }
}

/// 折りたたみ可能な JSON セクションを描画する。
fn draw_collapsible_json_section(
    ui: &mut egui::Ui,
    _ctx: &egui::Context,
    label: &str,
    json: &str,
    open: &mut bool,
) {
    if ui
        .selectable_label(
            *open,
            egui::RichText::new(if *open {
                format!("▼ {label}")
            } else {
                format!("▶ {label}")
            })
            .color(DIM_COLOR)
            .size(BODY_FONT),
        )
        .clicked()
    {
        *open = !*open;
    }
    if *open {
        egui::ScrollArea::vertical()
            .id_salt(label)
            .max_height(300.0)
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(json)
                        .color(JSON_COLOR)
                        .size(11.0)
                        .monospace(),
                );
            });
    }
}

/// 外部メタデータ (サイドカー) セクション。JSON は汎用 key/value ツリー、
/// TXT はテキスト表示 (docs/sidecar-metadata-ingest.md §11)。
/// 特定スキーマの代表フィールドをハードコードしない (どんな JSON でも同一ロジック)。
fn draw_sidecar_section(ui: &mut egui::Ui, sc: &crate::external_metadata::SidecarDisplay) {
    ui.label(
        egui::RichText::new("外部メタデータ")
            .color(egui::Color32::WHITE)
            .size(16.0)
            .strong(),
    );
    ui.add_space(4.0);
    match sc {
        crate::external_metadata::SidecarDisplay::Json(v) => {
            draw_json_node(ui, None, v, 0);
        }
        crate::external_metadata::SidecarDisplay::Text(t) => {
            draw_user_text_with_links(ui, t.as_str(), 11.0);
        }
    }
}

/// JSON 値がスカラ (Null/Bool/Number/String) かを **非アロケート** で判定する。
/// 配列分類の毎フレーム走査で `json_scalar_str` (String 生成) を避けるために使う。
fn is_json_scalar(v: &serde_json::Value) -> bool {
    matches!(
        v,
        serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_)
    )
}

/// JSON 値がスカラ (Null/Bool/Number/String) ならその表示文字列を返す。
fn json_scalar_str(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Null => Some("null".to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn json_dim_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(DIM_COLOR).size(BODY_FONT));
}

/// JSON 値を 1 ノード描画する。スカラは `key: value`、スカラ配列は 1 行、
/// ネストした配列/オブジェクトはインデントして再帰。深さ上限で打ち切る。
///
/// egui は immediate-mode なのでパネル表示中は毎フレーム再描画される。サイドカーは最大 2MB
/// 許容するため、巨大配列/オブジェクトを毎フレーム全件走査 / join / widget 化すると重い (Codex P2)。
/// 分類も描画も **先頭 `MAX_ITEMS` 件で完結** させ、残りは件数表示にする
/// (分類は非アロケートの `is_json_scalar` で先頭 MAX_ITEMS 件のみ判定する)。
fn draw_json_node(ui: &mut egui::Ui, key: Option<&str>, v: &serde_json::Value, depth: usize) {
    const MAX_DEPTH: usize = 8;
    const MAX_ITEMS: usize = 100;
    if let Some(s) = json_scalar_str(v) {
        draw_key_value_wrapped(ui, key.unwrap_or("-"), &s);
        return;
    }
    match v {
        serde_json::Value::Array(a) => {
            // 分類は先頭 MAX_ITEMS 件のみ・非アロケート判定 (巨大配列の毎フレーム全件走査を回避)。
            // 先頭が全てスカラならスカラ配列としてプレビュー表示する (101 件目以降に非スカラが
            // 混ざっていても、表示はあくまで先頭 100 件 + 残り件数なので実用上問題ない)。
            if a.iter().take(MAX_ITEMS).all(is_json_scalar) {
                // スカラのみの配列は 1 行に連結 (先頭 MAX_ITEMS 件まで)
                let mut joined = a
                    .iter()
                    .take(MAX_ITEMS)
                    .filter_map(json_scalar_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                if a.len() > MAX_ITEMS {
                    joined.push_str(&format!(" … (他 {} 件)", a.len() - MAX_ITEMS));
                }
                draw_key_value_wrapped(ui, key.unwrap_or("-"), &joined);
            } else if depth >= MAX_DEPTH {
                draw_key_value_wrapped(ui, key.unwrap_or("-"), &format!("[{} 件]", a.len()));
            } else {
                if let Some(k) = key {
                    json_dim_label(ui, &format!("{k}:"));
                }
                ui.indent(("sc_arr", depth, key.unwrap_or("")), |ui| {
                    for (i, e) in a.iter().take(MAX_ITEMS).enumerate() {
                        draw_json_node(ui, Some(&format!("[{i}]")), e, depth + 1);
                    }
                    if a.len() > MAX_ITEMS {
                        json_dim_label(ui, &format!("… (他 {} 件)", a.len() - MAX_ITEMS));
                    }
                });
            }
        }
        serde_json::Value::Object(m) => {
            if depth >= MAX_DEPTH {
                draw_key_value_wrapped(ui, key.unwrap_or("-"), &format!("{{{} キー}}", m.len()));
            } else if depth == 0 {
                // トップレベルはインデントせず並べる (キー数も先頭 MAX_ITEMS 件で打ち切る)
                for (k, val) in m.iter().take(MAX_ITEMS) {
                    draw_json_node(ui, Some(k), val, depth + 1);
                }
                if m.len() > MAX_ITEMS {
                    json_dim_label(ui, &format!("… (他 {} 件)", m.len() - MAX_ITEMS));
                }
            } else {
                if let Some(k) = key {
                    json_dim_label(ui, &format!("{k}:"));
                }
                ui.indent(("sc_obj", depth, key.unwrap_or("")), |ui| {
                    for (k, val) in m.iter().take(MAX_ITEMS) {
                        draw_json_node(ui, Some(k), val, depth + 1);
                    }
                    if m.len() > MAX_ITEMS {
                        json_dim_label(ui, &format!("… (他 {} 件)", m.len() - MAX_ITEMS));
                    }
                });
            }
        }
        // Null/Bool/Number/String は上の scalar 経路で処理済み
        _ => {}
    }
}

/// 「X ツイート情報」セクションを描画する。
/// `xtw:TweetId` がある画像 (mXD が保存したもの) のときだけ呼ばれる。
fn draw_tweet_panel(ui: &mut egui::Ui, ctx: &egui::Context, t: &XmpTweetInfo) {
    // ヘッダー
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("X ツイート情報")
                .color(egui::Color32::WHITE)
                .size(16.0)
                .strong(),
        );
        if let Some(src) = &t.source {
            let (label, color) = match src.as_str() {
                "Likes" => ("いいね", egui::Color32::from_rgb(240, 100, 140)),
                "Bookmarks" => ("ブックマーク", egui::Color32::from_rgb(100, 160, 240)),
                other => (other, egui::Color32::from_rgb(180, 180, 180)),
            };
            ui.label(
                egui::RichText::new(label)
                    .color(color)
                    .size(12.0)
                    .background_color(egui::Color32::from_rgba_unmultiplied(
                        color.r(),
                        color.g(),
                        color.b(),
                        30,
                    )),
            );
        }
    });
    ui.add_space(8.0);

    // 投稿者
    if t.author_display_name.is_some() || t.author_screen_name.is_some() {
        let display = t
            .author_display_name
            .as_deref()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let screen = t
            .author_screen_name
            .as_deref()
            .map(|s| format!("@{s}"))
            .unwrap_or_default();
        let combined = match (display.is_empty(), screen.is_empty()) {
            (false, false) => format!("{display} ({screen})"),
            (true, false) => screen,
            (false, true) => display,
            _ => String::new(),
        };
        draw_key_value_wrapped(ui, "投稿者", &combined);
    }

    if let Some(ts) = t.posted_at.as_deref() {
        draw_key_value_wrapped(ui, "投稿日時", &format_xmp_datetime(ts));
    }
    if let Some(ts) = t.discovered_at.as_deref() {
        draw_key_value_wrapped(ui, "発見日時", &format_xmp_datetime(ts));
    }

    // スレッド / メディア位置 (単独投稿・単枚は省略)
    let thread_interesting = t.thread_part.map(|n| n > 1).unwrap_or(false);
    let media_interesting = t.media_count.map(|n| n > 1).unwrap_or(false);
    if thread_interesting || media_interesting {
        let mut parts = Vec::new();
        if let Some(tp) = t.thread_part {
            parts.push(format!("スレッド {tp} 番目"));
        }
        if let (Some(mi), Some(mc)) = (t.media_index, t.media_count) {
            parts.push(format!("メディア {mi}/{mc}"));
        }
        if !parts.is_empty() {
            draw_key_value_wrapped(ui, "位置", &parts.join(" / "));
        }
    }

    // 本文
    if let Some(body) = t.description.as_deref().filter(|s| !s.is_empty()) {
        ui.add_space(4.0);
        draw_text_section(ui, ctx, "本文", body);
    }

    // アクションボタン
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        if let Some(url) = t.tweet_url.as_deref() {
            if xmp_reader::is_safe_tweet_url(url)
                && ui.button("元ツイートを開く").on_hover_text(url).clicked()
            {
                let _ = opener::open(url);
            }
        }
        if let Some(url) = t.author_url.as_deref() {
            if xmp_reader::is_safe_tweet_url(url)
                && ui
                    .button("投稿者のタイムラインを開く")
                    .on_hover_text(url)
                    .clicked()
            {
                let _ = opener::open(url);
            }
        }
        if let Some(url) = t.tweet_url.as_deref() {
            if xmp_reader::is_safe_tweet_url(url) && ui.small_button("URL コピー").clicked() {
                ctx.copy_text(url.to_string());
            }
        }
    });

    // 引用元 (別ツイートが RT/引用したことで保存された場合のみ)
    if let Some(qurl) = t.quoted_by_url.as_deref() {
        ui.add_space(4.0);
        let by = t
            .quoted_by_author_display_name
            .as_deref()
            .unwrap_or("")
            .to_string();
        let handle = t
            .quoted_by_screen_name
            .as_deref()
            .map(|s| format!("@{s}"))
            .unwrap_or_default();
        let label = match (by.is_empty(), handle.is_empty()) {
            (false, false) => format!("{by} ({handle}) が引用"),
            (_, false) => format!("{handle} が引用"),
            (false, _) => format!("{by} が引用"),
            _ => "別ツイートから引用".to_string(),
        };
        ui.label(egui::RichText::new(label).color(DIM_COLOR).size(BODY_FONT));
        if xmp_reader::is_safe_tweet_url(qurl)
            && ui
                .button("引用した投稿を開く")
                .on_hover_text(qurl)
                .clicked()
        {
            let _ = opener::open(qurl);
        }
    }
}

/// mXD / ExifTool が書く日時を視認しやすい形に整える。
/// 入力例: `"2026:04:16 04:09:58.0000000+00:00"` (ExifTool の `:` 区切り)
///         `"2026-04-16T04:09:58.0000000+00:00"` (ISO-8601)
/// 出力例: `"2026-04-16 04:09:58 UTC"` / `"2026-04-16 04:09:58 +09:00"`
/// パターンに合わなければ原文を返す。
fn format_xmp_datetime(raw: &str) -> String {
    // 日付部の `:` を `-` に (ExifTool 形式対策)。先頭 10 文字内の最初の 2 個が対象。
    // 10 文字境界で切るのは日付と時刻を混同しないため ("YYYY:MM:DD" の 10 文字)。
    let date_part: String = raw
        .chars()
        .take(10)
        .scan(0u8, |replaced, c| {
            let out = if c == ':' && *replaced < 2 {
                *replaced += 1;
                '-'
            } else {
                c
            };
            Some(out)
        })
        .collect();
    let rest: String = raw.chars().skip(10).collect();
    // ISO-8601 の `T` を空白に揃える (UI の読みやすさ)
    let rest = rest.replacen('T', " ", 1);

    // タイムゾーンを先に切り出す: `Z` / `+HH:MM` / `-HH:MM` のいずれか。
    // 小数秒(.0000000) の位置より後ろの `+`/`-`/`Z` を採用する。
    let tz_search_start = rest.find('.').map(|d| d + 1).unwrap_or(0);
    let tz_pos = rest[tz_search_start..]
        .find(|c: char| c == '+' || c == '-' || c == 'Z')
        .map(|i| tz_search_start + i);
    let (body, tz_suffix) = match tz_pos {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest.as_str(), ""),
    };
    // 小数秒を捨てる
    let time = match body.find('.') {
        Some(dot) => &body[..dot],
        None => body,
    };
    let tz_label = match tz_suffix {
        "+00:00" | "Z" => " UTC".to_string(),
        "" => String::new(),
        other => format!(" {other}"),
    };
    format!("{date_part}{time}{tz_label}")
}

#[cfg(test)]
mod format_datetime_tests {
    use super::format_xmp_datetime;

    #[test]
    fn utc_iso8601_collapses_offset() {
        assert_eq!(
            format_xmp_datetime("2026-04-16T04:09:58.0000000+00:00"),
            "2026-04-16 04:09:58 UTC"
        );
    }

    #[test]
    fn utc_exiftool_colon_date_converted() {
        assert_eq!(
            format_xmp_datetime("2026:04:16 04:09:58.0000000+00:00"),
            "2026-04-16 04:09:58 UTC"
        );
    }

    #[test]
    fn non_utc_offset_preserved() {
        assert_eq!(
            format_xmp_datetime("2026-04-16T04:09:58.500+09:00"),
            "2026-04-16 04:09:58 +09:00"
        );
    }

    #[test]
    fn no_fractional_seconds() {
        assert_eq!(
            format_xmp_datetime("2026-04-16T04:09:58Z"),
            "2026-04-16 04:09:58 UTC"
        );
    }

    #[test]
    fn unparseable_returns_near_original() {
        // `:` 置換は走るが壊れた文字列はそのまま (日付部 10 文字を超える位置は保持)
        let out = format_xmp_datetime("not a date");
        assert_eq!(out, "not a date");
    }
}

#[cfg(test)]
mod similar_panel_tests {
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    use super::{
        SIMILAR_THUMB_CACHE_LIMIT, SIMILAR_THUMB_WORKER_LIMIT, SimilarPanelModel,
        SimilarPanelState, SimilarPeekTransition, SimilarThumbResult, SimilarThumbState,
        decide_similar_peek, similar_copy_path_text, similar_difference_line,
        similar_location_line, similar_panel_model, summarize_page_strip,
    };
    use crate::similar_db::ItemKind;
    use crate::similar_image::SimilarImageFormat;
    use crate::similar_index::BookPageMatch;
    use crate::similar_index::{ItemQuery, MatchBand, QueryHit};

    fn strip_page(state: crate::similar_index::BookPageState, other: Option<u32>) -> BookPageMatch {
        BookPageMatch {
            state,
            other_mtime: 0,
            other_file_size: 0,
            other_page_index: other,
            other_target: other.map(|_| {
                crate::similar_index::SimilarItemTarget::File(PathBuf::from("c:/other.png"))
            }),
            other_item_key: other.map(|_| "c:/other.png".to_string()),
        }
    }

    /// 連続した収録と、たまたま数ページ一致しただけのものが、畳んだ後も見分けられること。
    /// ここで弱い側に寄せると「この単話は合本の 34〜77 ページに入っている」が読めなくなる。
    #[test]
    fn a_contiguous_run_survives_folding_and_scattered_hits_stay_thin() {
        use crate::similar_index::BookPageState;

        let contiguous = (0..100)
            .map(|page| {
                if (30..60).contains(&page) {
                    strip_page(BookPageState::Strong, Some(page as u32))
                } else {
                    strip_page(BookPageState::Unmatched, None)
                }
            })
            .collect::<Vec<_>>();
        let folded = summarize_page_strip(&contiguous, 20);
        let strong = folded
            .strongest
            .iter()
            .filter(|state| **state == Some(BookPageState::Strong))
            .count();
        assert_eq!(strong, 6, "30 pages of 100 fold into 6 of 20 columns");

        let scattered = (0..100)
            .map(|page| {
                if page % 17 == 0 {
                    strip_page(BookPageState::Strong, Some(page as u32))
                } else {
                    strip_page(BookPageState::Unmatched, None)
                }
            })
            .collect::<Vec<_>>();
        let folded = summarize_page_strip(&scattered, 20);
        let strong = folded
            .strongest
            .iter()
            .filter(|state| **state == Some(BookPageState::Strong))
            .count();
        assert!(
            strong <= 6,
            "scattered hits must not fill the strip: {strong} columns"
        );
    }

    /// 採点対象外は「一致しなかった」より弱く畳む。分母から外れているページを不一致と
    /// 同じ色で出すと、被覆率の分母と帯の見た目が食い違う。
    #[test]
    fn an_excluded_page_never_outranks_a_real_state() {
        use crate::similar_index::BookPageState;

        let pages = vec![
            strip_page(BookPageState::Excluded, None),
            strip_page(BookPageState::Unmatched, None),
        ];
        let folded = summarize_page_strip(&pages, 1);
        assert_eq!(folded.strongest[0], Some(BookPageState::Unmatched));

        let pages = vec![
            strip_page(BookPageState::Excluded, None),
            strip_page(BookPageState::Weak, Some(3)),
        ];
        let folded = summarize_page_strip(&pages, 1);
        assert_eq!(folded.strongest[0], Some(BookPageState::Weak));
        assert_eq!(folded.first_target[0], Some(1));
    }

    /// ページ数が幅より少ないときは、1 ページが複数のコマにまたがる。**空のコマを残さない。**
    /// 残すと帯が縞になり、対応が途切れているように見える。
    #[test]
    fn a_short_book_fills_the_whole_strip() {
        use crate::similar_index::BookPageState;

        let pages = (0..3)
            .map(|page| strip_page(BookPageState::Strong, Some(page as u32)))
            .collect::<Vec<_>>();
        let folded = summarize_page_strip(&pages, 10);
        assert_eq!(folded.strongest.len(), 10);
        assert!(
            folded
                .strongest
                .iter()
                .all(|state| *state == Some(BookPageState::Strong)),
            "a 3-page book must paint all 10 columns: {:?}",
            folded.strongest
        );
        assert!(folded.first_target.iter().all(Option::is_some));
    }

    /// どのコマも、自分が受け持つページの範囲だけを見る。隣の範囲まで拾うと、収録区間の
    /// 端が実際より広く見える。
    #[test]
    fn each_column_reads_only_its_own_pages() {
        use crate::similar_index::BookPageState;

        let pages = (0..100)
            .map(|page| {
                if page < 50 {
                    strip_page(BookPageState::Strong, Some(page as u32))
                } else {
                    strip_page(BookPageState::Unmatched, None)
                }
            })
            .collect::<Vec<_>>();
        let folded = summarize_page_strip(&pages, 10);
        assert_eq!(
            folded.strongest,
            vec![
                Some(BookPageState::Strong),
                Some(BookPageState::Strong),
                Some(BookPageState::Strong),
                Some(BookPageState::Strong),
                Some(BookPageState::Strong),
                Some(BookPageState::Unmatched),
                Some(BookPageState::Unmatched),
                Some(BookPageState::Unmatched),
                Some(BookPageState::Unmatched),
                Some(BookPageState::Unmatched),
            ]
        );
    }

    /// 本の [移動] は重なりが始まるページを開く。相手の 1 ページ目を開くと、総集編の
    /// どこに入っているのかを自分で探し直すことになる。
    #[test]
    fn opening_a_book_lands_where_the_overlap_starts() {
        use crate::similar_index::{BookPageState, BookRelationHit};

        let mut pages = (0..10)
            .map(|_| strip_page(BookPageState::Unmatched, None))
            .collect::<Vec<_>>();
        pages[4] = strip_page(BookPageState::Strong, Some(33));
        pages[5] = strip_page(BookPageState::Strong, Some(34));
        let hit = BookRelationHit {
            other_container_key: "e:/books/anthology.zip".to_string(),
            other_page_count: 200,
            pair: crate::dupe::book::BookPair {
                a: 1,
                b: 2,
                matched: 2,
                distinctive_a: 10,
                distinctive_b: 200,
                coverage_a: 0.2,
                coverage_b: 0.01,
                relation: crate::dupe::book::Relation::Contains { whole: 2 },
                alignment: Vec::new(),
            },
            pages,
        };
        let (key, _) = super::book_open_target(&hit).expect("an overlapping page exists");
        assert_eq!(key, "c:/other.png");

        let empty = BookRelationHit {
            pages: (0..3)
                .map(|_| strip_page(BookPageState::Unmatched, None))
                .collect(),
            ..hit
        };
        assert!(super::book_open_target(&empty).is_none());
    }

    /// 区画・▼・ホバー枠が同じ換算を使うこと。ここが割れると、指している場所と開く場所が
    /// ずれる。
    #[test]
    fn the_strip_places_a_page_the_same_way_everywhere() {
        let width = 220.0;
        let pages = 22;
        for page in 0..pages {
            let left = super::strip_page_left(width, pages, page);
            let right = super::strip_page_left(width, pages, page + 1);
            let center = super::strip_page_center(width, pages, page);
            assert!(
                left < center && center < right,
                "page {page}: center {center} is outside [{left}, {right})"
            );
        }
        assert_eq!(super::strip_page_left(width, pages, 0), 0.0);
        assert!((super::strip_page_left(width, pages, pages) - width).abs() < 0.001);

        // ページが 1 枚しかなくても割り算が壊れないこと。
        assert_eq!(super::strip_page_left(width, 1, 0), 0.0);
        assert!((super::strip_page_center(width, 1, 0) - width / 2.0).abs() < 0.001);
    }

    /// 見開きでは両方のページを調べる。片方に落とすと、もう片方の別バージョンが出なくなる。
    #[test]
    fn a_spread_asks_about_both_pages() {
        use crate::ui_fullscreen::SpreadPair;

        assert_eq!(
            super::similar_shown_indices(Some(4), SpreadPair::Double { left: 4, right: 5 }),
            vec![4, 5]
        );
        assert_eq!(
            super::similar_shown_indices(Some(4), SpreadPair::Single),
            vec![4]
        );
        assert!(super::similar_shown_indices(None, SpreadPair::Single).is_empty());
    }

    fn request_test_thumbnail(
        state: &mut SimilarPanelState,
        key: &str,
        target: crate::similar_index::SimilarItemTarget,
        mtime: i64,
        passwords: Option<&crate::pdf_passwords::PdfPasswordStore>,
        ctx: &egui::Context,
    ) {
        state.ensure_thumbnail_for(
            key,
            Some(target),
            mtime,
            200,
            72,
            85,
            crate::thumb_loader::CacheDecision::without_thumbnail(),
            passwords,
            ctx,
        );
    }

    fn one_pixel_image(color: egui::Color32) -> egui::ColorImage {
        egui::ColorImage::new([1, 1], vec![color])
    }

    fn demand_for(state: &SimilarPanelState, keys: &[&str]) -> super::SimilarThumbDemand {
        keys.iter()
            .filter_map(|key| {
                state
                    .thumbnails
                    .get(*key)
                    .map(|entry| ((*key).to_owned(), entry.request_id))
            })
            .collect()
    }

    #[test]
    fn thumbnail_admission_is_bounded_and_cancels_the_oldest_request() {
        let ctx = egui::Context::default();
        let mut state = SimilarPanelState::default();
        request_test_thumbnail(
            &mut state,
            "item-0",
            crate::similar_index::SimilarItemTarget::File(PathBuf::from("c:/item-0.png")),
            1,
            None,
            &ctx,
        );
        let super::SimilarThumbEntryState::Pending {
            cancel: oldest_cancel,
        } = &state.thumbnails["item-0"].state
        else {
            panic!("file request has a cancellation owner")
        };
        let oldest_cancel = oldest_cancel.clone();
        for index in 1..=SIMILAR_THUMB_CACHE_LIMIT {
            request_test_thumbnail(
                &mut state,
                &format!("item-{index}"),
                crate::similar_index::SimilarItemTarget::File(PathBuf::from(format!(
                    "c:/item-{index}.png"
                ))),
                1,
                None,
                &ctx,
            );
        }

        assert_eq!(state.thumbnails.len(), SIMILAR_THUMB_CACHE_LIMIT);
        assert_eq!(state.thumb_order.len(), SIMILAR_THUMB_CACHE_LIMIT);
        assert_eq!(state.thumb_jobs.len(), SIMILAR_THUMB_CACHE_LIMIT);
        assert!(!state.thumbnails.contains_key("item-0"));
        assert!(oldest_cancel.load(Ordering::Relaxed));
    }

    #[test]
    fn same_source_is_reused_across_global_epoch_bumps_and_stale_completion_is_ignored() {
        let ctx = egui::Context::default();
        let mut state = SimilarPanelState::default();
        let first_target =
            crate::similar_index::SimilarItemTarget::File(PathBuf::from("c:/same.png"));
        request_test_thumbnail(&mut state, "same", first_target.clone(), 1, None, &ctx);
        let first_id = state.thumbnails["same"].request_id;
        let super::SimilarThumbEntryState::Pending {
            cancel: first_cancel,
        } = &state.thumbnails["same"].state
        else {
            panic!("request has a cancellation owner")
        };
        let first_cancel = first_cancel.clone();

        crate::pdf_loader::bump_render_context_epoch();
        request_test_thumbnail(&mut state, "same", first_target, 1, None, &ctx);
        assert_eq!(state.thumbnails["same"].request_id, first_id);
        assert_eq!(
            state.thumb_jobs.len(),
            1,
            "same source must reuse pending work"
        );

        request_test_thumbnail(
            &mut state,
            "same",
            crate::similar_index::SimilarItemTarget::File(PathBuf::from("c:/replacement.png")),
            2,
            None,
            &ctx,
        );
        let replacement_id = state.thumbnails["same"].request_id;
        assert_ne!(replacement_id, first_id);
        assert!(first_cancel.load(Ordering::Relaxed));
        state.thumb_completions.push_back(SimilarThumbResult {
            request_id: first_id,
            item_key: "same".to_owned(),
            image: Some(one_pixel_image(egui::Color32::RED)),
        });
        state.thumb_completions.push_back(SimilarThumbResult {
            request_id: replacement_id,
            item_key: "same".to_owned(),
            image: None,
        });

        let demand = demand_for(&state, &["same"]);
        assert_eq!(state.apply_thumbnail_completions(&ctx, &demand).0, 0);
        assert!(matches!(
            state.thumbnail_state("same"),
            Some(SimilarThumbState::Failed)
        ));
    }

    #[test]
    fn pdf_password_revision_retries_only_that_unchanged_pdf() {
        let ctx = egui::Context::default();
        let mut state = SimilarPanelState::default();
        let pdf = PathBuf::from("c:/locked.pdf");
        let target = crate::similar_index::SimilarItemTarget::PdfPage {
            pdf_path: pdf.clone(),
            page_num: 3,
        };
        let mut passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        request_test_thumbnail(
            &mut state,
            "locked#3",
            target.clone(),
            1,
            Some(&passwords),
            &ctx,
        );
        let first_id = state.thumbnails["locked#3"].request_id;
        passwords.bump_credential_revision_for_test(&pdf);
        request_test_thumbnail(&mut state, "locked#3", target, 1, Some(&passwords), &ctx);
        assert_ne!(state.thumbnails["locked#3"].request_id, first_id);
    }

    #[test]
    fn completion_upload_is_limited_across_multiple_polls_in_the_same_frame() {
        let ctx = egui::Context::default();
        let mut state = SimilarPanelState::default();
        for key in ["a", "b"] {
            request_test_thumbnail(
                &mut state,
                key,
                crate::similar_index::SimilarItemTarget::File(PathBuf::from(format!(
                    "c:/{key}.png"
                ))),
                1,
                None,
                &ctx,
            );
        }
        let demand = demand_for(&state, &["a", "b"]);
        let jobs = state.take_thumbnail_jobs_for_dispatch(&demand);
        assert_eq!(jobs.len(), 2);
        for (job, color) in jobs
            .into_iter()
            .zip([egui::Color32::RED, egui::Color32::BLUE])
        {
            state
                .thumb_tx
                .send(SimilarThumbResult {
                    request_id: job.request_id,
                    item_key: job.item_key,
                    image: Some(one_pixel_image(color)),
                })
                .unwrap();
        }
        state.collect_thumbnail_results();

        assert_eq!(state.apply_thumbnail_completions(&ctx, &demand).0, 1);
        assert_eq!(state.apply_thumbnail_completions(&ctx, &demand).0, 0);
        assert!(matches!(
            state.thumbnail_state("a"),
            Some(SimilarThumbState::Ready(_))
        ));
        assert!(matches!(
            state.thumbnail_state("b"),
            Some(SimilarThumbState::Loading)
        ));

        let _ = ctx.run(egui::RawInput::default(), |_| {});
        assert_eq!(state.apply_thumbnail_completions(&ctx, &demand).0, 1);
        assert!(matches!(
            state.thumbnail_state("b"),
            Some(SimilarThumbState::Ready(_))
        ));
    }

    #[test]
    fn dispatcher_counts_cancelled_workers_until_their_terminal_result_drains() {
        let ctx = egui::Context::default();
        let mut state = SimilarPanelState::default();
        for index in 0..10 {
            request_test_thumbnail(
                &mut state,
                &format!("queued-{index}"),
                crate::similar_index::SimilarItemTarget::File(PathBuf::from(format!(
                    "c:/queued-{index}.png"
                ))),
                1,
                None,
                &ctx,
            );
        }

        let all_keys = (0..10)
            .map(|index| format!("queued-{index}"))
            .collect::<Vec<_>>();
        let demand = all_keys.iter().map(String::as_str).collect::<Vec<_>>();
        let demand = demand_for(&state, &demand);
        let dispatched = state.take_thumbnail_jobs_for_dispatch(&demand);
        assert_eq!(dispatched.len(), SIMILAR_THUMB_WORKER_LIMIT);
        assert_eq!(state.thumb_running, SIMILAR_THUMB_WORKER_LIMIT);
        let cancelled = &dispatched[0];
        let cancelled_flag = cancelled.cancel.clone();
        state.remove_thumbnail_request(&cancelled.item_key);
        assert!(cancelled_flag.load(Ordering::Relaxed));
        assert_eq!(
            state.thumb_running, SIMILAR_THUMB_WORKER_LIMIT,
            "cancelled work remains part of the running budget until it reports terminal"
        );
        assert!(state.take_thumbnail_jobs_for_dispatch(&demand).is_empty());

        state
            .thumb_tx
            .send(SimilarThumbResult {
                request_id: cancelled.request_id,
                item_key: cancelled.item_key.clone(),
                image: None,
            })
            .unwrap();
        state.collect_thumbnail_results();
        state.apply_thumbnail_completions(&ctx, &demand);
        assert_eq!(state.thumb_running, SIMILAR_THUMB_WORKER_LIMIT - 1);
        assert_eq!(state.take_thumbnail_jobs_for_dispatch(&demand).len(), 1);
        assert_eq!(state.thumb_running, SIMILAR_THUMB_WORKER_LIMIT);
    }

    #[test]
    fn request_completion_eviction_and_late_results_share_one_identity_pipeline() {
        let ctx = egui::Context::default();
        let mut state = SimilarPanelState::default();
        request_test_thumbnail(
            &mut state,
            "first",
            crate::similar_index::SimilarItemTarget::File(PathBuf::from("c:/first.png")),
            1,
            None,
            &ctx,
        );
        let first_demand = demand_for(&state, &["first"]);
        let mut first_job = state.take_thumbnail_jobs_for_dispatch(&first_demand);
        assert_eq!(first_job.len(), 1);
        let first_job = first_job.pop().unwrap();
        state
            .thumb_tx
            .send(SimilarThumbResult {
                request_id: first_job.request_id,
                item_key: first_job.item_key.clone(),
                image: Some(one_pixel_image(egui::Color32::GREEN)),
            })
            .unwrap();
        state.collect_thumbnail_results();
        assert_eq!(state.apply_thumbnail_completions(&ctx, &first_demand).0, 1);
        assert!(matches!(
            state.thumbnail_state("first"),
            Some(SimilarThumbState::Ready(_))
        ));

        for index in 0..SIMILAR_THUMB_CACHE_LIMIT {
            request_test_thumbnail(
                &mut state,
                &format!("later-{index}"),
                crate::similar_index::SimilarItemTarget::File(PathBuf::from(format!(
                    "c:/later-{index}.png"
                ))),
                1,
                None,
                &ctx,
            );
        }
        assert_eq!(state.thumbnails.len(), SIMILAR_THUMB_CACHE_LIMIT);
        assert!(!state.thumbnails.contains_key("first"));

        for image in [Some(one_pixel_image(egui::Color32::YELLOW)), None] {
            state
                .thumb_tx
                .send(SimilarThumbResult {
                    request_id: first_job.request_id,
                    item_key: first_job.item_key.clone(),
                    image,
                })
                .unwrap();
        }
        state.collect_thumbnail_results();
        state.apply_thumbnail_completions(&ctx, &Default::default());
        assert!(
            !state.thumbnails.contains_key("first"),
            "late success and failure must not resurrect an evicted request"
        );
    }

    #[test]
    fn current_visible_demand_overtakes_old_queued_and_completed_work() {
        let ctx = egui::Context::default();
        let mut state = SimilarPanelState::default();
        for index in 0..100 {
            request_test_thumbnail(
                &mut state,
                &format!("old-{index}"),
                crate::similar_index::SimilarItemTarget::File(PathBuf::from(format!(
                    "c:/old-{index}.png"
                ))),
                1,
                None,
                &ctx,
            );
        }
        request_test_thumbnail(
            &mut state,
            "new-visible",
            crate::similar_index::SimilarItemTarget::File(PathBuf::from("c:/new-visible.png")),
            1,
            None,
            &ctx,
        );
        let demand = demand_for(&state, &["new-visible"]);
        let jobs = state.take_thumbnail_jobs_for_dispatch(&demand);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].item_key, "new-visible");

        for index in 0..100 {
            let key = format!("old-{index}");
            state
                .thumb_tx
                .send(SimilarThumbResult {
                    request_id: state.thumbnails[&key].request_id,
                    item_key: key,
                    image: Some(one_pixel_image(egui::Color32::GRAY)),
                })
                .unwrap();
        }
        state
            .thumb_tx
            .send(SimilarThumbResult {
                request_id: jobs[0].request_id,
                item_key: jobs[0].item_key.clone(),
                image: Some(one_pixel_image(egui::Color32::WHITE)),
            })
            .unwrap();
        state.collect_thumbnail_results();

        assert_eq!(state.apply_thumbnail_completions(&ctx, &demand).0, 1);
        assert!(matches!(
            state.thumbnail_state("new-visible"),
            Some(SimilarThumbState::Ready(_))
        ));
        assert_eq!(
            state.thumb_completions.len(),
            100,
            "offscreen CPU results stay cached without delaying the current visible result"
        );
    }

    #[test]
    fn clipped_large_results_admit_only_visible_cards_and_keep_request_ids_stable() {
        let ctx = egui::Context::default();
        let origin = crate::similar_index::OriginItem {
            item_key: "c:/origin.png".to_owned(),
            kind: ItemKind::Image,
            mtime: 1,
            file_size: 100,
            width: 100,
            height: 100,
            format: SimilarImageFormat::Png,
            target: Some(crate::similar_index::SimilarItemTarget::File(
                PathBuf::from("c:/origin.png"),
            )),
        };
        let hits = (0..600)
            .map(|index| QueryHit {
                item_id: index + 1,
                item_key: format!("c:/hit-{index}.png"),
                kind: ItemKind::Image,
                container_key: None,
                page_index: None,
                distance: 1,
                band: MatchBand::NearlyIdentical,
                mtime: 1,
                file_size: 100,
                width: 100,
                height: 100,
                format: SimilarImageFormat::Png,
                target: Some(crate::similar_index::SimilarItemTarget::File(
                    PathBuf::from(format!("c:/hit-{index}.png")),
                )),
            })
            .collect();
        let matches = crate::similar_index::ItemMatches { origin, hits };
        let mut state = SimilarPanelState::default();

        let draw_frame = |state: &mut SimilarPanelState| {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.set_clip_rect(egui::Rect::from_min_max(
                        egui::pos2(0.0, 0.0),
                        egui::pos2(420.0, 420.0),
                    ));
                    let view = super::SimilarPageView {
                        heading: None,
                        item_key: Some(matches.origin.item_key.as_str()),
                        model: SimilarPanelModel::Results(&matches),
                        showing_previous: false,
                    };
                    let mut actions = super::SimilarPanelActions::default();
                    super::draw_similar_page_results(
                        ui,
                        &view,
                        state,
                        72,
                        85,
                        crate::thumb_loader::CacheDecision::without_thumbnail(),
                        None,
                        None,
                        false,
                        ctx,
                        &mut actions,
                    );
                });
            });
        };
        draw_frame(&mut state);
        let first_frame = state
            .thumb_order
            .iter()
            .map(|key| (key.clone(), state.thumbnails[key].request_id))
            .collect::<Vec<_>>();
        assert!(
            first_frame.len() > 1,
            "the clip should include at least one hit"
        );
        assert!(
            first_frame.len() < SIMILAR_THUMB_CACHE_LIMIT,
            "clipped rows must not enter the bounded cache: {}",
            first_frame.len()
        );

        draw_frame(&mut state);
        let second_frame = state
            .thumb_order
            .iter()
            .map(|key| (key.clone(), state.thumbnails[key].request_id))
            .collect::<Vec<_>>();
        assert_eq!(
            second_frame, first_frame,
            "a second frame must reuse visible requests instead of cycling all 600 rows"
        );
    }

    /// 押している間だけ覗き、離したら戻す。押したまま別の候補へ滑らせた場合は、前の表示を
    /// 戻してから新しい方へ移る。ここで Switch を Continue と同じに扱うと、離しても
    /// 最初の候補の restore_mode が残り、元の表示へ戻れなくなる。
    #[test]
    fn holding_a_peek_button_starts_continues_and_stops() {
        assert_eq!(decide_similar_peek(None, None), SimilarPeekTransition::Idle);
        assert_eq!(
            decide_similar_peek(None, Some("a")),
            SimilarPeekTransition::Start
        );
        assert_eq!(
            decide_similar_peek(Some("a"), Some("a")),
            SimilarPeekTransition::Continue
        );
        assert_eq!(
            decide_similar_peek(Some("a"), Some("b")),
            SimilarPeekTransition::Switch
        );
        assert_eq!(
            decide_similar_peek(Some("a"), None),
            SimilarPeekTransition::Stop
        );
    }

    fn origin() -> crate::similar_index::OriginItem {
        crate::similar_index::OriginItem {
            item_key: "c:/pictures/original.png".to_string(),
            kind: ItemKind::Image,
            mtime: 0,
            file_size: 4_400_000,
            width: 1200,
            height: 1600,
            format: SimilarImageFormat::Png,
            target: Some(crate::similar_index::SimilarItemTarget::File(
                PathBuf::from(r"C:\Pictures\original.png"),
            )),
        }
    }

    /// 起点と違う区切りだけを違いとする。共通の頭と尻は残す。
    #[test]
    fn only_the_differing_path_segments_are_marked() {
        let origin = ["E:", "share", "18", "doujin", "__new24", "283625"]
            .map(str::to_owned)
            .to_vec();
        let other = ["E:", "share", "18", "doujin", "__new26", "286166"]
            .map(str::to_owned)
            .to_vec();
        assert_eq!(
            super::path_segment_diff(&origin, &other),
            vec![false, false, false, false, true, true]
        );

        // 階層が 1 段深い相手でも、共通の尻は共通のまま残る。
        let deeper = ["E:", "share", "18", "doujin", "old", "__new24", "283625"]
            .map(str::to_owned)
            .to_vec();
        assert_eq!(
            super::path_segment_diff(&origin, &deeper),
            vec![false, false, false, false, true, false, false]
        );

        // 完全に同じなら、どこも違わない。
        assert_eq!(
            super::path_segment_diff(&origin, &origin),
            vec![false; origin.len()]
        );
    }

    fn hit(kind: ItemKind, format: SimilarImageFormat) -> QueryHit {
        QueryHit {
            item_id: 2,
            item_key: "c:/pictures/copy.png".to_string(),
            kind,
            container_key: None,
            page_index: None,
            distance: 47,
            band: MatchBand::OtherVersion,
            mtime: 1,
            file_size: 4_404_019,
            width: 2400,
            height: 3200,
            format,
            target: Some(crate::similar_index::SimilarItemTarget::File(
                PathBuf::from(r"C:\Pictures\copy.png"),
            )),
        }
    }

    #[test]
    fn five_empty_states_remain_distinct_typed_models() {
        assert_eq!(
            similar_panel_model(&ItemQuery::NoIndex),
            SimilarPanelModel::NoIndex
        );
        assert_eq!(
            similar_panel_model(&ItemQuery::Preparing),
            SimilarPanelModel::Preparing
        );
        assert_eq!(
            similar_panel_model(&ItemQuery::NotIndexed),
            SimilarPanelModel::NotIndexed
        );
        assert_eq!(
            similar_panel_model(&ItemQuery::Featureless),
            SimilarPanelModel::Featureless
        );
        assert_eq!(
            similar_panel_model(&ItemQuery::Ready(crate::similar_index::ItemMatches {
                origin: origin(),
                hits: Vec::new(),
            })),
            SimilarPanelModel::Empty
        );
    }

    #[test]
    fn loose_image_location_names_the_file_and_copy_uses_the_full_path() {
        let hit = hit(ItemKind::Image, SimilarImageFormat::Png);
        let location = similar_location_line(&hit);
        assert!(location.starts_with("copy.png / "), "{location}");
        assert!(location.to_lowercase().contains("pictures"), "{location}");

        let copied = similar_copy_path_text(&hit);
        assert!(copied.ends_with("copy.png"), "{copied}");
        assert!(copied.to_lowercase().contains("pictures"), "{copied}");
        #[cfg(windows)]
        assert!(!copied.contains('/'), "{copied}");
    }

    #[test]
    fn difference_lines_contain_facts_only_and_never_the_raw_distance() {
        let cases = [
            (ItemKind::Image, SimilarImageFormat::Png, "PNG", "4.20 MB"),
            (
                ItemKind::ZipPage,
                SimilarImageFormat::Jpeg,
                "JPEG",
                "書庫 4.20 MB",
            ),
            (
                ItemKind::PdfPage,
                SimilarImageFormat::Pdf,
                "PDF",
                "PDF 4.20 MB",
            ),
        ];
        for (kind, format, format_label, size_label) in cases {
            let line = similar_difference_line(&hit(kind, format), &origin());
            assert!(line.contains("2400×3200 (この画像は 1200×1600)"));
            assert!(line.contains(format_label));
            assert!(line.contains(size_label));
            assert!(!line.contains("47"), "raw distance leaked: {line}");
            for forbidden in ["高画質", "推奨", "おすすめ", "残すべき", "上位"] {
                assert!(!line.contains(forbidden), "evaluative word leaked: {line}");
            }
        }
    }
}

/// テキストセクション (ラベル + コピーボタン + テキスト) を描画する。
fn draw_text_section(ui: &mut egui::Ui, ctx: &egui::Context, label: &str, text: &str) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .color(LABEL_COLOR)
                .size(SECTION_FONT),
        );
        if ui
            .small_button("Copy")
            .on_hover_text("Copy to clipboard")
            .clicked()
        {
            ctx.copy_text(text.to_string());
        }
    });
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(text)
            .color(TEXT_COLOR)
            .font(crate::ui_fonts::user_text_font(BODY_FONT)),
    );
}

#[cfg(test)]
mod tag_panel_tests {
    use std::path::PathBuf;

    use super::{
        TagButtonState, TagPanelChoice, TagPanelRow, tag_button_state, tag_panel_picker_choices,
        tag_panel_visible_choices,
    };
    use crate::tag_ops::TagTarget;

    fn row(path: &str, tags: &[&str]) -> TagPanelRow {
        TagPanelRow {
            label: None,
            note: None,
            targets: vec![TagTarget {
                path: PathBuf::from(path),
                tag_sidecar: None,
            }],
            tags_by_target: vec![tags.iter().map(|tag| (*tag).to_string()).collect()],
        }
    }

    fn choice(name: &str, pinned: bool) -> TagPanelChoice {
        TagPanelChoice {
            name: name.to_string(),
            tag_key: crate::tags_db::normalize_tag_key(name),
            count: 0,
            pinned,
            last_applied_at: 0,
        }
    }

    fn recent_choice(name: &str, pinned: bool, last_applied_at: i64) -> TagPanelChoice {
        TagPanelChoice {
            name: name.to_string(),
            tag_key: crate::tags_db::normalize_tag_key(name),
            count: 0,
            pinned,
            last_applied_at,
        }
    }

    #[test]
    fn visible_choices_include_current_unpinned_tags() {
        let rows = vec![row("C:/media/a.mp4", &["#旅行"])];
        let visible = tag_panel_visible_choices(&[choice("人物", true)], &rows, &[], &[]);
        let names: Vec<_> = visible.iter().map(|choice| choice.name.as_str()).collect();
        assert_eq!(names, vec!["人物", "旅行"]);
    }

    #[test]
    fn sticky_unpinned_tag_remains_as_off_button_after_removal() {
        let rows = vec![row("C:/media/a.mp4", &[])];
        let sticky = vec![(
            "旅行".to_string(),
            crate::tags_db::normalize_tag_key("旅行"),
        )];
        let visible = tag_panel_visible_choices(&[], &rows, &sticky, &[]);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].name, "旅行");
        assert_eq!(
            tag_button_state(&rows[0], &visible[0].tag_key),
            TagButtonState::Off
        );
    }

    #[test]
    fn picker_pinned_tab_shows_only_pinned_tags_when_input_empty() {
        let catalog = vec![
            recent_choice("人物", true, 10),
            recent_choice("旅行", false, 20),
        ];
        let choices = tag_panel_picker_choices(&catalog, "", false);
        let names: Vec<_> = choices.iter().map(|choice| choice.name.as_str()).collect();
        assert_eq!(names, vec!["人物"]);
    }

    #[test]
    fn picker_recent_tab_uses_last_applied_order_without_removed_only_tags() {
        let catalog = vec![
            recent_choice("人物", true, 10),
            recent_choice("旅行", false, 30),
            recent_choice("未使用", false, 0),
        ];
        let choices = tag_panel_picker_choices(&catalog, "", true);
        let names: Vec<_> = choices.iter().map(|choice| choice.name.as_str()).collect();
        assert_eq!(names, vec!["旅行", "人物"]);
    }

    #[test]
    fn picker_search_ignores_current_tab() {
        let catalog = vec![
            recent_choice("人物", true, 10),
            recent_choice("旅行", false, 30),
        ];
        let query_key = crate::tags_db::normalize_tag_key("旅");
        let choices = tag_panel_picker_choices(&catalog, &query_key, false);
        let names: Vec<_> = choices.iter().map(|choice| choice.name.as_str()).collect();
        assert_eq!(names, vec!["旅行"]);
    }
}
