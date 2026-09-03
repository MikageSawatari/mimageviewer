//! 仮想ページを、外部プロセスや将来の D&D / clipboard が受け取れる実ファイルへ変換する。
//!
//! ファイル I/O、SQLite read、decode、合成、encode はすべて呼び出し側の worker thread で
//! 実行する。UI thread が持つのは軽量な [`MaterializeRequest`] snapshot だけである。

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use sha2::Digest as _;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MaterializeSource {
    File {
        path: PathBuf,
        image_page: bool,
    },
    ZipEntry {
        /// 実際に entry bytes を読む ZIP。変換アーカイブでは cache ZIP を指す。
        zip_path: PathBuf,
        entry_name: String,
    },
    PdfPage {
        pdf_path: PathBuf,
        page_num: u32,
        password: Option<String>,
    },
    /// 動画の 1 フレーム。**ワーカーで decode する** — `video::screenshot::capture_frame`
    /// がクリップボードコピー / <kbd>Ctrl+S</kbd> と同じ経路で原寸 RGBA を返す。
    ///
    /// 時刻をミリ秒の整数で持つのは cache key に入れるため (`f64` は `Hash` ではない)。
    /// UI 側は `VideoPlayer::screenshot_target_secs()` — 最後に提示したフレームの PTS —
    /// を渡す。クロックの現在値ではないので、一時停止中でも見えている絵と一致する。
    VideoFrame {
        path: PathBuf,
        target_millis: u64,
    },
    /// UI スレッドで既に組み上がった画素。見開きの合成がここへ来る。
    ///
    /// **ディスク上のどのファイルでもない。** 画素は `MaterializeRequest::rendered_pixels`
    /// が運ぶ (`Arc<ColorImage>` は Hash にできないので cache key には入れられない)。
    /// ここには一時ファイル名にする `label` だけを置く。
    ///
    /// **画素の指紋は持たない。** 元ファイルが無い = stamp が採れないので、この source は
    /// `lookup_reusable` にも cache への insert にも通らない (どちらも `modified_ns` を
    /// 要求する)。同一性を作っても読み手がいないので、v3.5.0 まで UI 入力ハンドラ内で
    /// 回していた全画素 SHA-256 (実測 12MP で 24ms) は捨てた (レビュー F05)。
    Rendered {
        label: String,
    },
}

impl MaterializeSource {
    /// 失敗を利用者へ見せるときの名前。ZIP / PDF は「どのページか」まで出す。
    /// パスだけだと、複数ページを渡したときにどれが失敗したのか分からない。
    pub fn display_label(&self) -> String {
        match self {
            Self::File { path, .. } => path.display().to_string(),
            Self::ZipEntry {
                zip_path,
                entry_name,
            } => format!("{} / {entry_name}", zip_path.display()),
            Self::PdfPage {
                pdf_path, page_num, ..
            } => format!("{} / {} ページ", pdf_path.display(), page_num + 1),
            Self::VideoFrame {
                path,
                target_millis,
            } => format!(
                "{} / {:.3} 秒",
                path.display(),
                *target_millis as f64 / 1000.0
            ),
            Self::Rendered { label, .. } => label.clone(),
        }
    }

    /// decode / source validation に使う物理ファイル。変換アーカイブでは、利用者から
    /// 見えている書庫ではなく cache ZIP を指す。
    ///
    /// `Rendered` は物理ファイルを持たないので `None`。**空パスで代用しない** —
    /// 呼び出し側に「無いときどうするか」を書かせる。
    fn source_path(&self) -> Option<&Path> {
        match self {
            Self::File { path, .. } => Some(path),
            Self::ZipEntry { zip_path, .. } => Some(zip_path),
            Self::PdfPage { pdf_path, .. } => Some(pdf_path),
            // 動画ファイルは実在するので stamp が採れる。同じ位置での再起動は cache に当たる。
            Self::VideoFrame { path, .. } => Some(path),
            Self::Rendered { .. } => None,
        }
    }

    fn source_kind(&self) -> MaterializeSourceKind {
        match self {
            Self::File {
                image_page: true, ..
            } => MaterializeSourceKind::ImageFile,
            Self::File {
                image_page: false, ..
            } => MaterializeSourceKind::OtherFile,
            Self::ZipEntry { .. } => MaterializeSourceKind::ZipEntry,
            Self::PdfPage { .. } => MaterializeSourceKind::PdfPage,
            Self::VideoFrame { .. } => MaterializeSourceKind::VideoFrame,
            Self::Rendered { .. } => MaterializeSourceKind::Rendered,
        }
    }

    fn original_name(&self) -> String {
        match self {
            Self::File { path, .. } => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "page".to_string()),
            Self::ZipEntry { entry_name, .. } => entry_name
                .replace('\\', "/")
                .rsplit('/')
                .find(|part| !part.is_empty())
                .unwrap_or("page")
                .to_string(),
            Self::PdfPage {
                pdf_path, page_num, ..
            } => {
                let stem = pdf_path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .filter(|stem| !stem.is_empty())
                    .unwrap_or_else(|| "document".to_string());
                format!("{stem}-page-{}.png", page_num + 1)
            }
            Self::VideoFrame {
                path,
                target_millis,
            } => {
                let stem = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .filter(|stem| !stem.is_empty())
                    .unwrap_or_else(|| "video".to_string());
                format!("{stem}-{:.3}s.png", *target_millis as f64 / 1000.0)
            }
            Self::Rendered { label, .. } => format!("{label}.png"),
        }
    }
}

#[derive(Clone)]
pub struct PageEditContext {
    pub page_key: String,
    pub params: crate::adjustment::AdjustParams,
    /// どこまで焼くか。外部ツールの設定から UI スレッドで確定させて渡す。
    pub stage: crate::bake_stage::BakeStage,
    /// 読み込み済み Creative LUT の一覧。**解決済みの 1 本ではなく一覧を渡す。**
    ///
    /// どの LUT を使うかは params が決めるが、スタック内ページのように一覧 index を
    /// 持たない対象では worker が params を DB から読み直す。UI で仮置き params から
    /// 解決してしまうと、ページ個別の LUT が無視され、親か共通の色で書き出される
    /// (v3.5.0 レビュー F10)。params と LUT は同じ所有者が同じ 1 つの答えから解決する。
    pub creative_luts: crate::creative_lut::CreativeLutSnapshot,
    pub conceal_preset: crate::conceal::ConcealPreset,
    pub erase_mono_tolerance: u8,
    pub comic_source_dims: Option<[usize; 2]>,
    pub ai_runtime: Option<Arc<crate::ai::runtime::AiRuntime>>,
    pub ai_model_manager: Arc<crate::ai::model_manager::ModelManager>,
    /// AI 段を焼くための材料。`None` は「この経路では AI 段を焼かない」。
    pub ai_materials: Option<crate::books::BookAiMaterials>,
    /// Stack member は App の idx cache に載らないため、worker でページ個別値を解決する。
    pub load_page_params_from_db: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MaterializePolicy {
    /// 一時ファイルへ、ページに加えた編集を反映して書き出す。**常に再エンコードする。**
    TempEdited,
    /// 一時ファイルへ、編集前のデータをそのまま書き出す。**常に無劣化。**
    TempOriginal,
    /// ディスク上の実ファイルをそのまま渡す。仮想ページには渡せない。
    OriginalFile,
}

#[derive(Clone)]
pub struct MaterializeRequest {
    pub source: MaterializeSource,
    pub policy: MaterializePolicy,
    pub page_edits: Option<PageEditContext>,
    pub pdf_render_long_edge: u32,
    /// `MaterializeSource::Rendered` のときだけ `Some`。UI スレッドで組み上げた画素を
    /// そのまま運ぶ。cache key に入れられない (`Arc<ColorImage>` は Hash ではない) ので
    /// source ではなく request が持つ。
    pub rendered_pixels: Option<Arc<egui::ColorImage>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterializeSourceKind {
    ImageFile,
    OtherFile,
    ZipEntry,
    /// 動画の 1 フレーム。ワーカーで decode する。
    VideoFrame,
    /// 既に組み上がった画素 (見開き合成)。
    Rendered,
    PdfPage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterializeDecision {
    /// 実ファイルをそのまま渡す。`OriginalFile` だけがここへ来る。
    DirectOriginal,
    /// 実ファイルの中身を一時ファイルへコピーする (再エンコードしない)。
    CopyOriginalFile,
    /// ZIP エントリの元バイト列を一時ファイルへ書き出す (再エンコードしない)。
    ExtractOriginal,
    RenderPng,
    /// 仮想ページに `OriginalFile` を要求された。元ファイルが無いので起動しない。
    RefuseVirtualPage,
}

/// 出力の形。**編集の有無では変えない。**
///
/// 以前は「編集が無ければ焼かずに元バイト列を出す」最適化を入れていたが、やめた
/// (利用者判断 2026-09-02)。受け取る側から見ると、**同じ設定・同じツールなのに、その
/// ページに編集が付いているかどうかで拡張子も EXIF の有無も変わる**。編集の有無は
/// ツール側から見えないので区別しようがない。値ごとに 1 つの振る舞いへ固定する:
/// `TempEdited` は常に PNG、`TempOriginal` は常に元バイト列。
pub fn decide_materialization(
    source: MaterializeSourceKind,
    policy: MaterializePolicy,
) -> MaterializeDecision {
    match policy {
        MaterializePolicy::OriginalFile => match source {
            MaterializeSourceKind::ImageFile | MaterializeSourceKind::OtherFile => {
                MaterializeDecision::DirectOriginal
            }
            // 合成した見開きにも動画のフレームにも、対応する元ファイルは存在しない。
            MaterializeSourceKind::ZipEntry
            | MaterializeSourceKind::PdfPage
            | MaterializeSourceKind::VideoFrame
            | MaterializeSourceKind::Rendered => MaterializeDecision::RefuseVirtualPage,
        },
        // 動画・音声 (`OtherFile`) はどちらの一時ポリシーでも実ファイルを渡す。
        // 数 GB を一時フォルダーへ複製する代償が、上書き保存に対する安全側の利益に
        // 見合わない (§4.3 の 2026-09-02 決定)。UI もこれを「動画ファイル」と表示する。
        MaterializePolicy::TempOriginal => match source {
            MaterializeSourceKind::OtherFile => MaterializeDecision::DirectOriginal,
            MaterializeSourceKind::ImageFile => MaterializeDecision::CopyOriginalFile,
            MaterializeSourceKind::ZipEntry => MaterializeDecision::ExtractOriginal,
            // 合成・動画フレーム・PDF ページには「編集前のバイト列」が存在しない。
            MaterializeSourceKind::PdfPage
            | MaterializeSourceKind::VideoFrame
            | MaterializeSourceKind::Rendered => MaterializeDecision::RenderPng,
        },
        MaterializePolicy::TempEdited => match source {
            MaterializeSourceKind::OtherFile => MaterializeDecision::DirectOriginal,
            MaterializeSourceKind::ImageFile
            | MaterializeSourceKind::ZipEntry
            | MaterializeSourceKind::PdfPage
            | MaterializeSourceKind::VideoFrame
            | MaterializeSourceKind::Rendered => MaterializeDecision::RenderPng,
        },
    }
}

fn normalized_pdf_render_long_edge(edge: u32) -> u32 {
    if edge == 0 { 4096 } else { edge }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FileStamp {
    modified_ns: Option<u128>,
    size: u64,
}

fn file_stamp(path: &Path) -> Result<FileStamp, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("対象を確認できません: {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("対象はファイルではありません: {}", path.display()));
    }
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos());
    Ok(FileStamp {
        modified_ns,
        size: metadata.len(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    source: MaterializeSource,
    policy: MaterializePolicy,
    pdf_render_long_edge: u32,
    edit_fingerprint: [u8; 32],
}

#[derive(Clone)]
struct CacheRecord {
    path: PathBuf,
    source_stamp: FileStamp,
    output_stamp: FileStamp,
}

#[derive(Default)]
struct TempState {
    initialized: bool,
    cache: HashMap<CacheKey, CacheRecord>,
    reserved: HashSet<PathBuf>,
    keep_paths: HashSet<PathBuf>,
}

struct MaterializerInner {
    temp_root: PathBuf,
    process_dir: PathBuf,
    generation: AtomicU64,
    state: Mutex<TempState>,
    startup_cleanup_ready: (Mutex<bool>, Condvar),
}

pub struct Materializer {
    inner: Arc<MaterializerInner>,
    startup_cleanup: Option<JoinHandle<()>>,
}

impl Materializer {
    pub fn new() -> Self {
        #[cfg(test)]
        {
            static NEXT_TEST_ROOT: AtomicU64 = AtomicU64::new(0);
            let sequence = NEXT_TEST_ROOT.fetch_add(1, Ordering::Relaxed);
            let temp_root = std::env::temp_dir().join(format!(
                "mimageviewer-materializer-test-{}-{sequence}",
                std::process::id()
            ));
            return Self::new_at(temp_root, std::process::id(), false);
        }

        #[cfg(not(test))]
        let temp_root = if cfg!(feature = "portable") {
            crate::data_dir::get().join("temp")
        } else {
            std::env::temp_dir().join("mimageviewer")
        };
        #[cfg(not(test))]
        {
            Self::new_at(temp_root, std::process::id(), true)
        }
    }

    fn new_at(temp_root: PathBuf, pid: u32, start_cleanup: bool) -> Self {
        let process_dir = temp_root.join(format!("ext-{pid}"));
        let inner = Arc::new(MaterializerInner {
            temp_root: temp_root.clone(),
            process_dir,
            generation: AtomicU64::new(0),
            state: Mutex::new(TempState::default()),
            startup_cleanup_ready: (Mutex::new(!start_cleanup), Condvar::new()),
        });
        let startup_cleanup = start_cleanup.then(|| {
            let cleanup_inner = Arc::clone(&inner);
            std::thread::Builder::new()
                .name("materializer-orphan-cleanup".to_string())
                .spawn(move || {
                    cleanup_startup_directories(&temp_root, pid, pid_is_alive);
                    let (ready, ready_changed) = &cleanup_inner.startup_cleanup_ready;
                    *ready.lock().unwrap_or_else(|error| error.into_inner()) = true;
                    ready_changed.notify_all();
                })
                .unwrap_or_else(|error| {
                    crate::logger::log(format!(
                        "materializer: orphan cleanup worker start failed: {error}"
                    ));
                    let (ready, ready_changed) = &inner.startup_cleanup_ready;
                    *ready.lock().unwrap_or_else(|error| error.into_inner()) = true;
                    ready_changed.notify_all();
                    std::thread::spawn(|| {})
                })
        });
        Self {
            inner,
            startup_cleanup,
        }
    }

    pub fn begin_generation(&self) -> u64 {
        self.inner
            .generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
    }

    pub fn cancel_all(&self) {
        self.inner.generation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn generation_is_current(&self, generation: u64) -> bool {
        self.inner.generation.load(Ordering::Acquire) == generation
    }

    pub fn session(&self) -> MaterializeSession {
        MaterializeSession {
            inner: Arc::clone(&self.inner),
            databases: None,
            worker_ai_runtime: None,
        }
    }

    /// materialize worker が停止した後に呼ぶ。終了掃除自体も worker で実行し join する。
    pub fn shutdown(&mut self) {
        self.cancel_all();
        if let Some(handle) = self.startup_cleanup.take() {
            let _ = handle.join();
        }
        let inner = Arc::clone(&self.inner);
        match std::thread::Builder::new()
            .name("materializer-exit-cleanup".to_string())
            .spawn(move || cleanup_own_process_directory(&inner))
        {
            Ok(handle) => {
                let _ = handle.join();
            }
            Err(error) => crate::logger::log(format!(
                "materializer: exit cleanup worker start failed: {error}"
            )),
        }
    }
}

impl Default for Materializer {
    fn default() -> Self {
        Self::new()
    }
}

struct EditDatabases {
    adjustment: crate::adjustment_db::AdjustmentDb,
    rotation: crate::rotation_db::RotationDb,
    mask: crate::mask_db::MaskDb,
    local_adjust: crate::local_adjust_db::LocalAdjustDb,
    conceal: crate::conceal_db::ConcealDb,
    comic: crate::comic_db::ComicDb,
    crop: crate::export_crop::CropDb,
}

impl EditDatabases {
    fn open() -> Result<Self, String> {
        Ok(Self {
            adjustment: crate::adjustment_db::AdjustmentDb::open_readonly()
                .map_err(|error| format!("補正データベースを開けません: {error}"))?,
            rotation: crate::rotation_db::RotationDb::open_readonly()
                .map_err(|error| format!("回転データベースを開けません: {error}"))?,
            mask: crate::mask_db::MaskDb::open_readonly()
                .map_err(|error| format!("消しゴムデータベースを開けません: {error}"))?,
            local_adjust: crate::local_adjust_db::LocalAdjustDb::open_readonly(
                &crate::local_adjust_db::LocalAdjustDb::db_path(),
            )
            .map_err(|error| format!("補正レイヤーデータベースを開けません: {error}"))?,
            conceal: crate::conceal_db::ConcealDb::open_readonly(
                &crate::conceal_db::ConcealDb::db_path(),
            )
            .map_err(|error| format!("隠蔽加工データベースを開けません: {error}"))?,
            comic: crate::comic_db::ComicDb::open_readonly()
                .map_err(|error| format!("注釈データベースを開けません: {error}"))?,
            crop: crate::export_crop::CropDb::open_readonly(&crate::export_crop::CropDb::db_path())
                .map_err(|error| format!("切り取りデータベースを開けません: {error}"))?,
        })
    }
}

pub struct MaterializeSession {
    inner: Arc<MaterializerInner>,
    databases: Option<EditDatabases>,
    worker_ai_runtime: Option<Arc<crate::ai::runtime::AiRuntime>>,
}

struct LoadedPageEdits {
    snapshot: crate::books::BakedEditSnapshot,
    requires_composite: bool,
    fingerprint: [u8; 32],
}

impl MaterializeSession {
    pub fn ensure_current(&self, cancel: &AtomicBool, generation: u64) -> Result<(), String> {
        check_current(&self.inner, cancel, generation)
    }

    pub fn materialize(
        &mut self,
        request: &MaterializeRequest,
        cancel: &Arc<AtomicBool>,
        generation: u64,
    ) -> Result<PreparedMaterializedFile, String> {
        check_current(&self.inner, cancel, generation)?;

        let loaded_edits =
            if request.policy == MaterializePolicy::TempEdited && request.page_edits.is_some() {
                Some(self.load_page_edits(
                    request.page_edits.as_ref().expect("checked above"),
                    cancel,
                    generation,
                )?)
            } else {
                None
            };
        let decision = decide_materialization(request.source.source_kind(), request.policy);
        if decision == MaterializeDecision::RefuseVirtualPage {
            return Err(virtual_page_refusal_message(&request.source));
        }
        if decision == MaterializeDecision::DirectOriginal {
            let direct_path = request
                .source
                .source_path()
                .ok_or_else(|| "渡せる実ファイルがありません".to_string())?;
            std::fs::metadata(direct_path).map_err(|error| {
                format!("対象を確認できません: {}: {error}", direct_path.display())
            })?;
            return Ok(PreparedMaterializedFile::direct(direct_path.to_path_buf()));
        }
        // `Rendered` には元ファイルが無いので stamp を採れない。**空 stamp は cache を
        // 常に miss させる** (`lookup_reusable` が `modified_ns.is_some()` を要求する)。
        // 画素は既に手元にあり、やり直しは encode だけなので、これでよい。
        let source_stamp = match request.source.source_path() {
            Some(path) => file_stamp(path)?,
            None => FileStamp::default(),
        };

        let edit_fingerprint = loaded_edits
            .as_ref()
            .map(|loaded| loaded.fingerprint)
            .unwrap_or([0; 32]);
        let pdf_render_long_edge = normalized_pdf_render_long_edge(request.pdf_render_long_edge);
        let key = CacheKey {
            source: request.source.clone(),
            policy: request.policy,
            pdf_render_long_edge,
            edit_fingerprint,
        };
        ensure_process_directory(&self.inner)?;
        if let Some(record) = lookup_reusable(&self.inner, &key, source_stamp) {
            return Ok(PreparedMaterializedFile::reused(
                record.path,
                Arc::clone(&self.inner),
            ));
        }

        check_current(&self.inner, cancel, generation)?;
        let desired_name = materialized_name(&request.source, decision);
        let mut lease = reserve_collision_path(&self.inner, &desired_name)?;
        let output_path = lease.path().to_path_buf();
        let write_result = (|| -> Result<(), String> {
            match decision {
                MaterializeDecision::CopyOriginalFile => copy_original_file(
                    &request.source,
                    lease.file_mut()?,
                    &output_path,
                    &self.inner,
                    cancel,
                    generation,
                ),
                MaterializeDecision::ExtractOriginal => write_zip_entry(
                    &request.source,
                    lease.file_mut()?,
                    &output_path,
                    &self.inner,
                    cancel,
                    generation,
                ),
                MaterializeDecision::RenderPng => {
                    // 既に組み上がった画素は decode も焼き込みもせず、そのまま encode する。
                    // 補正・回転・注釈は UI 側で反映済みの表示画素だから。
                    let image = if let MaterializeSource::Rendered { .. } = &request.source {
                        let pixels = request
                            .rendered_pixels
                            .as_ref()
                            .ok_or_else(|| "組み上げた画素が渡されていません".to_string())?;
                        egui::ColorImage::clone(pixels)
                    } else if let MaterializeSource::VideoFrame {
                        path,
                        target_millis,
                    } = &request.source
                    {
                        decode_video_frame(path, *target_millis)?
                    } else {
                        let source = composite_source(&request.source)?;
                        let image = crate::books::decode_composite_source_for_materialization(
                            &source,
                            pdf_render_long_edge,
                            Arc::clone(cancel),
                        )?;
                        image
                    };
                    check_current(&self.inner, cancel, generation)?;
                    let image = if let Some(loaded) = loaded_edits {
                        if loaded.requires_composite {
                            crate::books::compose_book_page_for_materialization(
                                image,
                                &loaded.snapshot,
                                Arc::clone(cancel),
                            )?
                        } else {
                            image
                        }
                    } else {
                        image
                    };
                    check_current(&self.inner, cancel, generation)?;
                    let rgba = crate::capture::color_image_to_rgba(&image);
                    crate::capture::write_rgba_with_matte(
                        lease.file_mut()?,
                        crate::capture::CaptureFormat::Png,
                        crate::capture::JpegMatte::Black,
                        image.size[0] as u32,
                        image.size[1] as u32,
                        &rgba,
                    )
                }
                MaterializeDecision::DirectOriginal | MaterializeDecision::RefuseVirtualPage => {
                    unreachable!("handled above")
                }
            }
        })();
        write_result?;
        lease.flush()?;
        check_current(&self.inner, cancel, generation)?;
        lease.finish(key, source_stamp)
    }

    /// worker で使う AI runtime。UI が既に持っていればそれを、無ければ worker が 1 度だけ作る。
    ///
    /// 消しゴム (MI-GAN) と AI 段が同じ 1 つを共有する。別々に作ると同じ GPU 上に
    /// 2 つの session が並ぶ。
    fn resolve_worker_ai_runtime(
        &mut self,
        context: &PageEditContext,
    ) -> Option<Arc<crate::ai::runtime::AiRuntime>> {
        if let Some(runtime) = context.ai_runtime.clone() {
            return Some(runtime);
        }
        if self.worker_ai_runtime.is_none() {
            self.worker_ai_runtime = crate::ai::runtime::AiRuntime::new_with_backend(
                crate::ai::AiBackend::DirectMl,
            )
            .map(Arc::new)
            .map_err(|error| {
                crate::logger::log(format!(
                    "materializer: AI runtime init failed; using diffusion fallback: {error}"
                ));
                error
            })
            .ok();
        }
        self.worker_ai_runtime.clone()
    }

    fn load_page_edits(
        &mut self,
        context: &PageEditContext,
        cancel: &Arc<AtomicBool>,
        generation: u64,
    ) -> Result<LoadedPageEdits, String> {
        check_current(&self.inner, cancel, generation)?;
        if self.databases.is_none() {
            self.databases = Some(EditDatabases::open()?);
        }
        check_current(&self.inner, cancel, generation)?;
        let databases = self.databases.as_ref().expect("opened above");
        let key = &context.page_key;
        let stored_params = if context.load_page_params_from_db {
            databases
                .adjustment
                .get_page_params_checked(key)
                .map_err(|error| format!("補正データを読み取れません: {error}"))?
        } else {
            None
        };
        let (params, creative_lut) = baked_params_and_lut(context, stored_params);
        check_current(&self.inner, cancel, generation)?;
        let rotation = databases
            .rotation
            .get_key_checked(key)
            .map_err(|error| format!("回転データを読み取れません: {error}"))?
            .unwrap_or(crate::rotation_db::Rotation::None);
        check_current(&self.inner, cancel, generation)?;
        let erase_raw = databases
            .mask
            .get_full_checked(key)
            .map_err(|error| format!("消しゴムデータを読み取れません: {error}"))?;
        check_current(&self.inner, cancel, generation)?;
        let local_adjust = databases
            .local_adjust
            .get_layers_checked(key)
            .map_err(|error| format!("補正レイヤーを読み取れません: {error}"))?
            .filter(|layers| !layers.is_empty())
            // v3.4.0 の master で BakedEditSnapshot::local_adjust が Arc 化された
            // (合成側で clone を避けるため)。ここも合わせる。
            .map(std::sync::Arc::new);
        check_current(&self.inner, cancel, generation)?;
        let conceal_raw = databases
            .conceal
            .get_full_checked(key)
            .map_err(|error| format!("隠蔽加工データを読み取れません: {error}"))?;
        check_current(&self.inner, cancel, generation)?;
        let comic_objects = databases
            .comic
            .get_checked(key)
            .map_err(|error| format!("注釈データを読み取れません: {error}"))?
            .filter(|objects| !objects.is_empty());
        check_current(&self.inner, cancel, generation)?;
        let export_crop = databases
            .crop
            .get_checked(key)
            .map_err(|error| format!("切り取りデータを読み取れません: {error}"))?;
        check_current(&self.inner, cancel, generation)?;

        let requires_composite = crate::books::page_requires_full_composite(
            &params,
            rotation,
            conceal_raw.is_some(),
            erase_raw.is_some(),
            local_adjust.is_some(),
            comic_objects.is_some(),
            export_crop.is_some(),
            context.stage,
        );
        // AI を通す気があるかは 1 か所で決め、**出力の同一性と実行の両方**がその答えを使う。
        // 別々に綴ると、設定を変えたのに前の出力が再利用される (v3.5.0 レビュー R05)。
        let ai_materials = context.ai_materials.clone().filter(|materials| {
            crate::books::stage_requests_ai(context.stage, &params, materials.policy.feature_mode)
        });
        let fingerprint = edit_fingerprint(
            &params,
            rotation,
            erase_raw.as_ref(),
            local_adjust.as_deref(),
            conceal_raw.as_ref(),
            &context.conceal_preset,
            comic_objects.as_ref(),
            export_crop.as_ref(),
            context.erase_mono_tolerance,
            context.comic_source_dims,
            context.stage,
            ai_materials.as_ref().map(|materials| materials.policy),
        )?;

        let conceal = conceal_raw.map(|(bitmap, shapes, size)| crate::books::BookConcealSnapshot {
            mask: crate::books::BookMaskSnapshot {
                bitmap,
                shapes,
                size,
            },
            preset: context.conceal_preset.clone(),
        });
        // AI 段。**通す気があるのに runtime を用意できなければ失敗にする** — AI 抜きの絵は
        // 寸法から別物なので、黙って落として成功と言ってはならない (レビュー R14)。
        let ai = match ai_materials {
            Some(materials) => {
                let runtime = self
                    .resolve_worker_ai_runtime(context)
                    .ok_or_else(crate::books::ai_runtime_unavailable_error)?;
                Some(crate::books::book_ai_snapshot(
                    materials,
                    runtime,
                    params.clone(),
                ))
            }
            None => None,
        };
        let erase = erase_raw.map(|(bitmap, shapes, size)| {
            let runtime = self.resolve_worker_ai_runtime(context);
            let manager = Arc::clone(&context.ai_model_manager);
            let mono_tolerance = context.erase_mono_tolerance;
            let run: crate::books::BookEraseRunner =
                Box::new(move |base, bitmap, shapes, cancel| {
                    let result = crate::ui_erase::erase_from_saved_mask(
                        runtime.as_ref(),
                        &manager,
                        base,
                        bitmap,
                        shapes,
                        mono_tolerance,
                        cancel,
                        "materializer-composite",
                    )?;
                    Ok(crate::books::BookEraseResult {
                        image: result.image,
                        used_diffusion_fallback: result.used_diffusion_fallback,
                    })
                });
            crate::books::BookEraseSnapshot {
                mask: crate::books::BookMaskSnapshot {
                    bitmap,
                    shapes,
                    size,
                },
                run,
            }
        });
        check_current(&self.inner, cancel, generation)?;
        let comic = if let Some(objects) = comic_objects {
            check_current(&self.inner, cancel, generation)?;
            let fonts = crate::comic_overlay::load_comic_fonts_for(&objects)
                .ok_or_else(|| "テキスト注釈用フォントを読み取れません".to_string())?;
            check_current(&self.inner, cancel, generation)?;
            Some(crate::books::BookComicSnapshot {
                objects,
                fonts: Arc::new(fonts),
                stamp_cache: HashMap::new(),
            })
        } else {
            None
        };
        Ok(LoadedPageEdits {
            snapshot: crate::books::BakedEditSnapshot {
                params,
                rotation,
                conceal,
                erase,
                local_adjust,
                comic,
                comic_source_dims: context.comic_source_dims,
                export_crop,
                crop_legacy_writeback: None,
                format: crate::capture::CaptureFormat::Png,
                jpeg_matte: crate::capture::JpegMatte::Black,
                stage: context.stage,
                creative_lut,
                ai,
            },
            requires_composite,
            fingerprint,
        })
    }
}

fn virtual_page_refusal_message(source: &MaterializeSource) -> String {
    match source {
        MaterializeSource::ZipEntry { .. } => "圧縮ファイル内のページには元のファイルがありません。書き出してから編集してください (フルスクリーンで Ctrl+E)".to_string(),
        MaterializeSource::PdfPage { .. } => "PDF 内のページには元のファイルがありません。書き出してから編集してください (フルスクリーンで Ctrl+E)".to_string(),
        MaterializeSource::File { .. } => "元のファイルを渡せません".to_string(),
        MaterializeSource::VideoFrame { .. } | MaterializeSource::Rendered { .. } => {
            "見開きの合成と動画のフレームには元のファイルがありません。「一時ファイル」を渡す設定にしてください".to_string()
        }
    }
}

/// 動画の 1 フレームを原寸で decode する。
///
/// クリップボードコピーと <kbd>Ctrl+S</kbd> が使うのと**同じ helper**
/// ([`crate::video::screenshot::capture_frame`])。再生用デコーダとは別の FFmpeg input を
/// 開き、向き情報も適用済みの RGBA を返す。ここは実体化ワーカーなので UI を止めない。
fn decode_video_frame(path: &Path, target_millis: u64) -> Result<egui::ColorImage, String> {
    let frame = crate::video::screenshot::capture_frame(path, target_millis as f64 / 1000.0)?;
    let width = frame.width as usize;
    let height = frame.height as usize;
    if width == 0 || height == 0 || frame.rgba.len() != width * height * 4 {
        return Err("動画フレームのサイズが不正です".to_string());
    }
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [width, height],
        &frame.rgba,
    ))
}

/// 元ファイルの中身を一時ファイルへ流す。**全部読んでから書かない**。
/// 数百 MB の RAW / TIFF でもメモリを積まず、途中でキャンセルも効く。
fn copy_original_file(
    source: &MaterializeSource,
    file: &mut std::fs::File,
    path: &Path,
    inner: &MaterializerInner,
    cancel: &Arc<AtomicBool>,
    generation: u64,
) -> Result<(), String> {
    let MaterializeSource::File {
        path: source_path, ..
    } = source
    else {
        return Err("実ファイルではありません".to_string());
    };
    let mut reader = std::fs::File::open(source_path).map_err(|error| {
        format!(
            "元ファイルを読み取れません: {}: {error}",
            source_path.display()
        )
    })?;
    use std::io::{Read as _, Write as _};
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        check_current(inner, cancel, generation)?;
        let read = reader.read(&mut buffer).map_err(|error| {
            format!(
                "元ファイルを読み取れません: {}: {error}",
                source_path.display()
            )
        })?;
        if read == 0 {
            return Ok(());
        }
        file.write_all(&buffer[..read]).map_err(|error| {
            format!("一時ファイルを書き込めません: {}: {error}", path.display())
        })?;
    }
}

fn composite_source(source: &MaterializeSource) -> Result<crate::books::CompositeSource, String> {
    Ok(match source {
        MaterializeSource::File {
            path,
            image_page: true,
        } => crate::books::CompositeSource::File { path: path.clone() },
        MaterializeSource::ZipEntry {
            zip_path,
            entry_name,
            ..
        } => crate::books::CompositeSource::ZipEntry {
            zip_path: zip_path.clone(),
            entry_name: entry_name.clone(),
        },
        MaterializeSource::PdfPage {
            pdf_path,
            page_num,
            password,
        } => crate::books::CompositeSource::PdfPage {
            pdf_path: pdf_path.clone(),
            page_num: *page_num,
            password: password.clone(),
        },
        MaterializeSource::File {
            image_page: false, ..
        } => return Err("この実ファイルは画像として実体化できません".to_string()),
        MaterializeSource::VideoFrame { .. } | MaterializeSource::Rendered { .. } => {
            return Err("この対象は画像 decode 経路へ渡せません".to_string());
        }
    })
}

fn write_zip_entry(
    source: &MaterializeSource,
    file: &mut std::fs::File,
    path: &Path,
    inner: &MaterializerInner,
    cancel: &Arc<AtomicBool>,
    generation: u64,
) -> Result<(), String> {
    let MaterializeSource::ZipEntry {
        zip_path,
        entry_name,
        ..
    } = source
    else {
        return Err("ZIP エントリではありません".to_string());
    };
    let bytes = crate::zip_loader::read_entry_bytes(zip_path, entry_name)
        .map_err(|error| format!("ZIP 内画像を読み取れません: {entry_name}: {error}"))?;
    check_current(inner, cancel, generation)?;
    use std::io::Write as _;
    for chunk in bytes.chunks(1024 * 1024) {
        check_current(inner, cancel, generation)?;
        file.write_all(chunk).map_err(|error| {
            format!("一時ファイルを書き込めません: {}: {error}", path.display())
        })?;
    }
    Ok(())
}

fn materialized_name(source: &MaterializeSource, decision: MaterializeDecision) -> String {
    let original = source.original_name();
    // 再エンコードしない出力は元の名前と拡張子のまま渡す。ツールのタイトルバーにも
    // 「名前を付けて保存」にも、利用者が知っている名前が出る。
    if matches!(
        decision,
        MaterializeDecision::CopyOriginalFile | MaterializeDecision::ExtractOriginal
    ) {
        return crate::books::sanitize_materialized_basename(&original, "page");
    }
    let stem = Path::new(&original)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "page".to_string());
    format!(
        "{}.png",
        crate::books::sanitize_materialized_basename(&stem, "page")
    )
}

fn ensure_process_directory(inner: &MaterializerInner) -> Result<(), String> {
    let (startup_ready, startup_changed) = &inner.startup_cleanup_ready;
    let mut ready = startup_ready
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    while !*ready {
        ready = startup_changed
            .wait(ready)
            .unwrap_or_else(|error| error.into_inner());
    }
    drop(ready);
    let mut state = inner
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if state.initialized {
        return validate_real_directory(&inner.process_dir, "一時フォルダー");
    }
    std::fs::create_dir_all(&inner.temp_root).map_err(|error| {
        format!(
            "一時ファイルの親フォルダーを作成できません: {}: {error}",
            inner.temp_root.display()
        )
    })?;
    validate_real_directory(&inner.temp_root, "一時ファイルの親フォルダー")?;
    std::fs::create_dir(&inner.process_dir).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            format!(
                "既存の一時フォルダーは安全のため再利用しません: {}",
                inner.process_dir.display()
            )
        } else {
            format!(
                "一時フォルダーを作成できません: {}: {error}",
                inner.process_dir.display()
            )
        }
    })?;
    validate_real_directory(&inner.process_dir, "一時フォルダー")?;
    state.initialized = true;
    Ok(())
}

fn reserve_collision_path(
    inner: &Arc<MaterializerInner>,
    desired_name: &str,
) -> Result<PendingTempLease, String> {
    validate_real_directory(&inner.process_dir, "一時フォルダー")?;
    let desired = Path::new(desired_name);
    let stem = desired
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "page".to_string());
    let extension = desired
        .extension()
        .map(|extension| extension.to_string_lossy().into_owned());
    let mut state = inner
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for sequence in 0..10_000u32 {
        let name = if sequence == 0 {
            desired_name.to_string()
        } else if let Some(extension) = &extension {
            format!("{stem}-{sequence}.{extension}")
        } else {
            format!("{stem}-{sequence}")
        };
        let path = inner.process_dir.join(name);
        if state.reserved.contains(&path) {
            continue;
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                state.reserved.insert(path.clone());
                return Ok(PendingTempLease {
                    path,
                    inner: Arc::clone(inner),
                    file: Some(file),
                    active: true,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "一時ファイルを予約できません: {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Err("一時ファイル名の連番が上限に達しました".to_string())
}

fn delete_request_owned_file(inner: &MaterializerInner, path: &Path) {
    // reserved を持ったまま削除する。先に予約を返すと、別 request が同名を
    // create_new した直後に旧 request の Drop がその新ファイルを消し得る。
    let mut state = inner
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _ = std::fs::remove_file(path);
    state.reserved.remove(path);
}

/// `create_new` で確保した request-owned path。`finish` より前のどの `?` でも、
/// この要求が作った placeholder/output だけを削除して予約を返す。
struct PendingTempLease {
    path: PathBuf,
    inner: Arc<MaterializerInner>,
    file: Option<std::fs::File>,
    active: bool,
}

impl PendingTempLease {
    fn path(&self) -> &Path {
        &self.path
    }

    fn file_mut(&mut self) -> Result<&mut std::fs::File, String> {
        self.file
            .as_mut()
            .ok_or_else(|| "一時ファイルの所有権は既に移動済みです".to_string())
    }

    fn flush(&mut self) -> Result<(), String> {
        use std::io::Write as _;
        let path = self.path.clone();
        self.file_mut()?
            .flush()
            .map_err(|error| format!("一時ファイルを書き込めません: {}: {error}", path.display()))
    }

    fn finish(
        mut self,
        key: CacheKey,
        source_stamp: FileStamp,
    ) -> Result<PreparedMaterializedFile, String> {
        drop(self.file.take());
        // Windows では最終書込時刻が handle close で確定し得る。cache record は
        // close 後に取得した stamp だけを保持する。
        let output_stamp = file_stamp(&self.path)?;
        self.active = false;
        Ok(PreparedMaterializedFile::created(
            self.path.clone(),
            Arc::clone(&self.inner),
            key,
            source_stamp,
            output_stamp,
        ))
    }
}

impl Drop for PendingTempLease {
    fn drop(&mut self) {
        drop(self.file.take());
        if self.active {
            delete_request_owned_file(&self.inner, &self.path);
        }
    }
}

fn lookup_reusable(
    inner: &MaterializerInner,
    key: &CacheKey,
    source_stamp: FileStamp,
) -> Option<CacheRecord> {
    let mut state = inner
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let record = state.cache.get(key)?.clone();
    let valid = source_stamp.modified_ns.is_some()
        && record.source_stamp.modified_ns.is_some()
        && record.output_stamp.modified_ns.is_some()
        && record.source_stamp == source_stamp
        && file_stamp(&record.path)
            .ok()
            .is_some_and(|stamp| stamp.modified_ns.is_some() && stamp == record.output_stamp);
    if valid {
        Some(record)
    } else {
        state.cache.remove(key);
        None
    }
}

pub struct PreparedMaterializedFile {
    path: PathBuf,
    original: bool,
    ownership: PreparedOwnership,
}

enum PreparedOwnership {
    Direct,
    /// Cache hit は既に process directory ownership へ移ったファイルを借りる。
    ProcessOwnedReuse {
        inner: Arc<MaterializerInner>,
    },
    RequestOwned {
        inner: Arc<MaterializerInner>,
        cache_key: CacheKey,
        source_stamp: FileStamp,
        output_stamp: FileStamp,
    },
    Transferred,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TempOwnershipStage {
    Direct,
    ProcessOwned,
    RequestOwned,
    Transferred,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TempOwnershipEvent {
    LaunchSucceeded,
    RequestDropped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TempOwnershipTransition {
    Retain,
    TransferToProcess,
    DeleteRequestFile,
}

fn temp_ownership_transition(
    stage: TempOwnershipStage,
    event: TempOwnershipEvent,
) -> TempOwnershipTransition {
    match (stage, event) {
        (TempOwnershipStage::RequestOwned, TempOwnershipEvent::LaunchSucceeded) => {
            TempOwnershipTransition::TransferToProcess
        }
        (TempOwnershipStage::RequestOwned, TempOwnershipEvent::RequestDropped) => {
            TempOwnershipTransition::DeleteRequestFile
        }
        _ => TempOwnershipTransition::Retain,
    }
}

impl PreparedOwnership {
    fn stage(&self) -> TempOwnershipStage {
        match self {
            Self::Direct => TempOwnershipStage::Direct,
            Self::ProcessOwnedReuse { .. } => TempOwnershipStage::ProcessOwned,
            Self::RequestOwned { .. } => TempOwnershipStage::RequestOwned,
            Self::Transferred => TempOwnershipStage::Transferred,
        }
    }
}

impl PreparedMaterializedFile {
    fn direct(path: PathBuf) -> Self {
        Self {
            path,
            original: true,
            ownership: PreparedOwnership::Direct,
        }
    }

    fn reused(path: PathBuf, inner: Arc<MaterializerInner>) -> Self {
        Self {
            path,
            original: false,
            ownership: PreparedOwnership::ProcessOwnedReuse { inner },
        }
    }

    fn created(
        path: PathBuf,
        inner: Arc<MaterializerInner>,
        key: CacheKey,
        source_stamp: FileStamp,
        output_stamp: FileStamp,
    ) -> Self {
        Self {
            path,
            original: false,
            ownership: PreparedOwnership::RequestOwned {
                inner,
                cache_key: key,
                source_stamp,
                output_stamp,
            },
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_original(&self) -> bool {
        self.original
    }

    /// 外部アプリへの spawn / Invoke が成功した直後に呼ぶ。
    pub fn transfer_to_process_directory(&mut self, keep_temp: bool) {
        debug_assert_ne!(
            temp_ownership_transition(self.ownership.stage(), TempOwnershipEvent::LaunchSucceeded,),
            TempOwnershipTransition::DeleteRequestFile
        );
        let ownership = std::mem::replace(&mut self.ownership, PreparedOwnership::Transferred);
        match ownership {
            PreparedOwnership::Direct | PreparedOwnership::Transferred => return,
            PreparedOwnership::ProcessOwnedReuse { inner } => {
                if keep_temp {
                    inner
                        .state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .keep_paths
                        .insert(self.path.clone());
                }
            }
            PreparedOwnership::RequestOwned {
                inner,
                cache_key,
                source_stamp,
                output_stamp,
            } => {
                let mut state = inner
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if source_stamp.modified_ns.is_some() && output_stamp.modified_ns.is_some() {
                    state.cache.insert(
                        cache_key,
                        CacheRecord {
                            path: self.path.clone(),
                            source_stamp,
                            output_stamp,
                        },
                    );
                }
                state.reserved.remove(&self.path);
                if keep_temp {
                    state.keep_paths.insert(self.path.clone());
                }
            }
        }
    }
}

impl Drop for PreparedMaterializedFile {
    fn drop(&mut self) {
        if temp_ownership_transition(self.ownership.stage(), TempOwnershipEvent::RequestDropped)
            == TempOwnershipTransition::DeleteRequestFile
            && let PreparedOwnership::RequestOwned { inner, .. } = &self.ownership
        {
            delete_request_owned_file(inner, &self.path);
        }
    }
}

fn check_current(
    inner: &MaterializerInner,
    cancel: &AtomicBool,
    generation: u64,
) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) || inner.generation.load(Ordering::Acquire) != generation {
        Err("外部ツールの準備をキャンセルしました".to_string())
    } else {
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
/// 同じ出力になる要求を同じ値へ畳む指紋。
///
/// **段も含める。** 同じ編集内容でも、どこまで焼いたかで出力が違う。含めないと
/// 「AI まで」で作った一時ファイルが「編集まで」の要求へ再利用される。
fn edit_fingerprint(
    params: &crate::adjustment::AdjustParams,
    rotation: crate::rotation_db::Rotation,
    erase: Option<&(Vec<bool>, Vec<crate::mask_db::Shape>, [usize; 2])>,
    local_adjust: Option<&Vec<local_adjust_core::LocalAdjustmentLayer>>,
    conceal: Option<&(Vec<bool>, Vec<crate::mask_db::Shape>, [usize; 2])>,
    conceal_preset: &crate::conceal::ConcealPreset,
    comic: Option<&Vec<comic_core::AnnotationObject>>,
    crop: Option<&crate::export_crop::CropSettings>,
    erase_mono_tolerance: u8,
    comic_source_dims: Option<[usize; 2]>,
    stage: crate::bake_stage::BakeStage,
    // `ai_policy` は AI 段が実際に使う設定。段が AI へ届かないときだけ `None`。
    // 出力を変える値がここに入っていないと、設定を変えた後も同じ key に当たって前の
    // 出力を渡してしまう (v3.5.0 レビュー R05)。
    ai_policy: Option<crate::books::BookAiPolicy>,
) -> Result<[u8; 32], String> {
    let mut digest = sha2::Sha256::new();
    for (domain, value) in [
        ("params", serde_json::to_vec(params)),
        ("rotation", serde_json::to_vec(&rotation.degrees())),
        ("erase", serde_json::to_vec(&erase)),
        ("local_adjust", serde_json::to_vec(&local_adjust)),
        ("conceal", serde_json::to_vec(&conceal)),
        ("conceal_preset", serde_json::to_vec(conceal_preset)),
        ("comic", serde_json::to_vec(&comic)),
        ("crop", serde_json::to_vec(&crop)),
        (
            "erase_mono_tolerance",
            serde_json::to_vec(&erase_mono_tolerance),
        ),
        ("comic_source_dims", serde_json::to_vec(&comic_source_dims)),
        ("stage", serde_json::to_vec(&stage)),
        ("ai_policy", serde_json::to_vec(&ai_policy)),
    ] {
        let value = value.map_err(|error| format!("編集状態を検証できません: {error}"))?;
        digest.update((domain.len() as u32).to_le_bytes());
        digest.update(domain.as_bytes());
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value);
    }
    Ok(digest.finalize().into())
}

fn cleanup_own_process_directory(inner: &MaterializerInner) {
    let keep_paths = {
        let state = inner
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !state.initialized {
            return;
        }
        state.keep_paths.clone()
    };
    if let Err(error) = validate_real_directory(&inner.process_dir, "一時フォルダー") {
        crate::logger::log(format!(
            "materializer: exit cleanup refused process directory path={} error={error}",
            inner.process_dir.display()
        ));
        return;
    }
    let Ok(entries) = std::fs::read_dir(&inner.process_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if keep_paths.contains(&path) {
            continue;
        }
        let result = remove_tree_without_following_links(&path);
        if let Err(error) = result {
            crate::logger::log(format!(
                "materializer: exit cleanup failed path={} error={error}",
                path.display()
            ));
        }
    }
    let _ = std::fs::remove_dir(&inner.process_dir);
}

fn orphan_pid(name: &str) -> Option<u32> {
    name.strip_prefix("ext-")?.parse().ok()
}

fn startup_cleanup_candidates<I, F>(entries: I, current_pid: u32, mut alive: F) -> Vec<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
    F: FnMut(u32) -> bool,
{
    entries
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(orphan_pid)
                .is_some_and(|pid| pid == current_pid || !alive(pid))
        })
        .collect()
}

fn cleanup_startup_directories(root: &Path, current_pid: u32, alive: impl FnMut(u32) -> bool) {
    if let Err(error) = validate_real_directory(root, "一時ファイルの親フォルダー") {
        if root.exists() {
            crate::logger::log(format!(
                "materializer: orphan cleanup refused root path={} error={error}",
                root.display()
            ));
        }
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let paths = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path());
    for path in startup_cleanup_candidates(paths, current_pid, alive) {
        if let Err(error) = remove_tree_without_following_links(&path) {
            crate::logger::log(format!(
                "materializer: orphan cleanup failed path={} error={error}",
                path.display()
            ));
        }
    }
}

fn remove_tree_without_following_links(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata_is_link_or_reparse(&metadata) {
        return std::fs::remove_dir(path).or_else(|_| std::fs::remove_file(path));
    }
    if !metadata.is_dir() {
        return std::fs::remove_file(path);
    }
    for entry in std::fs::read_dir(path)? {
        remove_tree_without_following_links(&entry?.path())?;
    }
    std::fs::remove_dir(path)
}

fn metadata_is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn validate_real_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("{label}を確認できません: {}: {error}", path.display()))?;
    if metadata_is_link_or_reparse(&metadata) {
        return Err(format!(
            "{label}が reparse point のため安全に使用できません: {}",
            path.display()
        ));
    }
    if !metadata.is_dir() {
        return Err(format!(
            "{label}はフォルダーではありません: {}",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn pid_is_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, GetLastError};
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let handle = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
        Ok(handle) => handle,
        Err(_) => return unsafe { GetLastError() } != ERROR_INVALID_PARAMETER,
    };
    let mut exit_code = 0u32;
    // Query failure is not proof that the process is dead. Cleanup must only select a PID
    // directory when death is certain, otherwise another live mIV can lose files in use.
    let alive = match unsafe { GetExitCodeProcess(handle, &mut exit_code) } {
        Ok(()) => exit_code == 259, // STILL_ACTIVE
        Err(_) => true,
    };
    let _ = unsafe { CloseHandle(handle) };
    alive
}

#[cfg(not(windows))]
fn pid_is_alive(pid: u32) -> bool {
    PathBuf::from("/proc").join(pid.to_string()).exists()
}

/// 焼き込みに使う params と、その params が指す Creative LUT。**両方を 1 か所で決める。**
///
/// `stored` は DB のページ個別値 (`load_page_params_from_db` のときだけ読む)。UI 側で
/// 仮置き params から LUT を解決して渡していたので、スタック内ページのように worker が
/// params を読み直す対象では、ページ個別の LUT が捨てられ親か共通の色で書き出されていた
/// (v3.5.0 レビュー F10)。
fn baked_params_and_lut(
    context: &PageEditContext,
    stored: Option<crate::adjustment::AdjustParams>,
) -> (
    crate::adjustment::AdjustParams,
    Option<(crate::creative_lut::SharedCreativeLut, f32)>,
) {
    let params = stored.unwrap_or_else(|| context.params.clone());
    let creative_lut = context
        .stage
        .includes_display_adjust()
        .then(|| context.creative_luts.resolve(&params))
        .flatten();
    (params, creative_lut)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_test_zip(path: &Path, entry_name: &str, bytes: &[u8]) {
        use std::io::Write as _;
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file(entry_name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
        writer.finish().unwrap();
    }

    fn zip_request(path: &Path, entry_name: &str) -> MaterializeRequest {
        MaterializeRequest {
            source: MaterializeSource::ZipEntry {
                zip_path: path.to_path_buf(),
                entry_name: entry_name.to_string(),
            },
            policy: MaterializePolicy::TempOriginal,
            page_edits: None,
            pdf_render_long_edge: 4096,
            rendered_pixels: None,
        }
    }

    fn lut_named(name: &str) -> crate::creative_lut::SharedCreativeLut {
        std::sync::Arc::new(local_adjust_core::CubeLutParams {
            name: name.to_string(),
            ..Default::default()
        })
    }

    fn page_edit_context(
        placeholder: crate::adjustment::AdjustParams,
        luts: crate::creative_lut::CreativeLutSnapshot,
        load_page_params_from_db: bool,
    ) -> PageEditContext {
        PageEditContext {
            page_key: "key".to_string(),
            params: placeholder,
            stage: crate::bake_stage::BakeStage::DisplayAdjust,
            creative_luts: luts,
            conceal_preset: crate::conceal::ConcealPreset::default(),
            erase_mono_tolerance: 0,
            comic_source_dims: None,
            ai_runtime: None,
            ai_model_manager: std::sync::Arc::new(crate::ai::model_manager::ModelManager::new()),
            ai_materials: None,
            load_page_params_from_db,
        }
    }

    fn params_with_lut(id: uuid::Uuid) -> crate::adjustment::AdjustParams {
        let mut params = crate::adjustment::AdjustParams::default();
        params.creative_lut = crate::creative_lut::CreativeLutSelection {
            id: Some(id),
            strength: 1.0,
        };
        params
    }

    /// ページ個別の params を worker が読み直したら、**LUT もその params から引く**。
    ///
    /// UI で仮置き params から解決した 1 本を渡していたので、一覧 index を持たない
    /// スタック内ページでは、ページ個別に設定した LUT が捨てられて親か共通の色で
    /// 書き出されていた (v3.5.0 レビュー F10)。
    #[test]
    fn the_baked_lut_follows_the_params_the_worker_actually_uses() {
        let page_lut_id = uuid::Uuid::from_u128(1);
        let parent_lut_id = uuid::Uuid::from_u128(2);
        let luts = crate::creative_lut::CreativeLutSnapshot::from_loaded(
            [
                (page_lut_id, lut_named("page")),
                (parent_lut_id, lut_named("parent")),
            ]
            .into_iter()
            .collect(),
        );
        // UI が渡すのは親 / 共通の仮置き値。実際に焼くのは DB のページ個別値。
        let context = page_edit_context(params_with_lut(parent_lut_id), luts, true);

        let (params, lut) = baked_params_and_lut(&context, Some(params_with_lut(page_lut_id)));

        assert_eq!(params.creative_lut.id, Some(page_lut_id));
        assert_eq!(lut.expect("ページ個別の LUT が選ばれる").0.name, "page");
    }

    /// DB にページ個別値が無ければ、仮置き値とその LUT で焼く。
    #[test]
    fn the_baked_lut_falls_back_to_the_placeholder_when_the_page_has_no_row() {
        let parent_lut_id = uuid::Uuid::from_u128(2);
        let luts = crate::creative_lut::CreativeLutSnapshot::from_loaded(
            [(parent_lut_id, lut_named("parent"))].into_iter().collect(),
        );
        let context = page_edit_context(params_with_lut(parent_lut_id), luts, true);

        let (params, lut) = baked_params_and_lut(&context, None);

        assert_eq!(params.creative_lut.id, Some(parent_lut_id));
        assert_eq!(lut.expect("親の LUT で焼く").0.name, "parent");
    }

    /// 表示補正を焼かない段では LUT を焼かない (段の意味を LUT だけ素通りさせない)。
    #[test]
    fn a_stage_without_display_adjust_bakes_no_lut() {
        let lut_id = uuid::Uuid::from_u128(1);
        let luts = crate::creative_lut::CreativeLutSnapshot::from_loaded(
            [(lut_id, lut_named("page"))].into_iter().collect(),
        );
        let mut context = page_edit_context(params_with_lut(lut_id), luts, true);
        context.stage = crate::bake_stage::BakeStage::Edits;

        let (_, lut) = baked_params_and_lut(&context, Some(params_with_lut(lut_id)));

        assert!(lut.is_none());
    }

    /// 一時ポリシーは**実ファイルもコピーする**。渡した先で上書き保存されても元データが
    /// 変わらないことが、この 2 値を分けている理由 (§4.3 の 2026-09-02 決定)。
    #[test]
    fn only_the_real_file_policy_hands_over_the_file_itself() {
        assert_eq!(
            decide_materialization(
                MaterializeSourceKind::ImageFile,
                MaterializePolicy::TempOriginal,
            ),
            MaterializeDecision::CopyOriginalFile
        );
        assert_eq!(
            decide_materialization(
                MaterializeSourceKind::ImageFile,
                MaterializePolicy::TempEdited
            ),
            MaterializeDecision::RenderPng
        );
        assert_eq!(
            decide_materialization(
                MaterializeSourceKind::ImageFile,
                MaterializePolicy::OriginalFile,
            ),
            MaterializeDecision::DirectOriginal
        );
    }

    /// **出力の形は編集の有無で変えない。** 同じ設定・同じツールなのにページ次第で
    /// 拡張子や EXIF の有無が変わると、受け取る側から区別できない (2026-09-02 決定)。
    #[test]
    fn the_output_shape_does_not_depend_on_whether_the_page_carries_edits() {
        for source in [
            MaterializeSourceKind::ImageFile,
            MaterializeSourceKind::ZipEntry,
        ] {
            assert_eq!(
                decide_materialization(source, MaterializePolicy::TempEdited),
                MaterializeDecision::RenderPng,
                "{source:?}: 「編集を反映」は常に焼く"
            );
        }
        assert_eq!(
            decide_materialization(
                MaterializeSourceKind::ZipEntry,
                MaterializePolicy::TempOriginal,
            ),
            MaterializeDecision::ExtractOriginal,
            "「編集前」は常に元バイト列"
        );
        assert_eq!(
            decide_materialization(
                MaterializeSourceKind::ImageFile,
                MaterializePolicy::TempOriginal,
            ),
            MaterializeDecision::CopyOriginalFile,
            "「編集前」は常に元バイト列"
        );
    }

    /// 動画・音声はどの一時ポリシーでも実ファイルを渡す。数 GB のコピーが、上書き保存に
    /// 対する安全側の利益に見合わない (§4.3 の 2026-09-02 決定)。
    #[test]
    fn video_and_audio_are_never_copied_into_the_temp_directory() {
        for policy in [
            MaterializePolicy::TempEdited,
            MaterializePolicy::TempOriginal,
            MaterializePolicy::OriginalFile,
        ] {
            assert_eq!(
                decide_materialization(MaterializeSourceKind::OtherFile, policy),
                MaterializeDecision::DirectOriginal,
                "{policy:?}"
            );
        }
    }

    #[test]
    fn pdf_pages_always_render_and_refuse_only_the_real_file_policy() {
        assert_eq!(
            decide_materialization(
                MaterializeSourceKind::PdfPage,
                MaterializePolicy::TempOriginal
            ),
            MaterializeDecision::RenderPng
        );
        assert_eq!(
            decide_materialization(
                MaterializeSourceKind::PdfPage,
                MaterializePolicy::TempEdited
            ),
            MaterializeDecision::RenderPng
        );
    }

    #[test]
    fn the_real_file_policy_refuses_virtual_pages() {
        for source in [
            MaterializeSourceKind::ZipEntry,
            MaterializeSourceKind::PdfPage,
        ] {
            assert_eq!(
                decide_materialization(source, MaterializePolicy::OriginalFile),
                MaterializeDecision::RefuseVirtualPage
            );
        }
    }

    /// 合成した見開きにも動画のフレームにも、対応する元ファイルは無い。**それらしい
    /// 一時ファイルを「元のファイル」として渡さない。**
    #[test]
    fn rendered_pixels_encode_under_temp_policies_and_refuse_the_real_file_policy() {
        for policy in [
            MaterializePolicy::TempEdited,
            MaterializePolicy::TempOriginal,
        ] {
            assert_eq!(
                decide_materialization(MaterializeSourceKind::Rendered, policy),
                MaterializeDecision::RenderPng,
                "{policy:?}"
            );
        }
        assert_eq!(
            decide_materialization(
                MaterializeSourceKind::Rendered,
                MaterializePolicy::OriginalFile
            ),
            MaterializeDecision::RefuseVirtualPage
        );
    }

    /// 合成した見開きは **一時ファイル cache に載らない**。
    ///
    /// 元ファイルが無いので stamp が採れず、`lookup_reusable` も cache への insert も
    /// `modified_ns` を要求するため、どちらも通らない。つまり再利用は起きない。それでも
    /// v3.5.0 まで、cache key を作るためだけに UI 入力ハンドラ内で全画素 SHA-256 を回して
    /// いた (実測 12MP で 24ms)。読み手のいない同一性は作らない (レビュー F05)。
    #[test]
    fn a_rendered_source_is_never_cached_so_it_needs_no_pixel_identity() {
        let source = MaterializeSource::Rendered {
            label: "spread".to_string(),
        };

        assert!(
            source.source_path().is_none(),
            "元ファイルが無いので stamp を採れない"
        );
        assert!(
            FileStamp::default().modified_ns.is_none(),
            "空 stamp は lookup も insert も通らない"
        );
    }

    /// 画素は request が運ぶ。source だけ来て画素が無い組み合わせは**黙って空の PNG を
    /// 書かず**、失敗として返す。
    #[test]
    fn a_rendered_source_without_pixels_fails_instead_of_writing_an_empty_image() {
        let temp = tempfile::tempdir().unwrap();
        let manager = Materializer::new_at(temp.path().join("materialized"), 82, false);
        let generation = manager.begin_generation();
        let cancel = Arc::new(AtomicBool::new(false));
        let request = MaterializeRequest {
            source: MaterializeSource::Rendered {
                label: "spread".to_string(),
            },
            policy: MaterializePolicy::TempEdited,
            page_edits: None,
            pdf_render_long_edge: 4096,
            rendered_pixels: None,
        };

        let error = match manager.session().materialize(&request, &cancel, generation) {
            Ok(_) => panic!("a Rendered source without pixels must not produce a file"),
            Err(error) => error,
        };
        assert!(error.contains("画素"));
    }

    #[test]
    fn rendered_pixels_are_written_under_the_label_they_were_given() {
        let temp = tempfile::tempdir().unwrap();
        let manager = Materializer::new_at(temp.path().join("materialized"), 83, false);
        let generation = manager.begin_generation();
        let cancel = Arc::new(AtomicBool::new(false));
        let image = egui::ColorImage::new([2, 1], vec![egui::Color32::from_rgb(9, 9, 9); 2]);
        let request = MaterializeRequest {
            source: MaterializeSource::Rendered {
                label: "page04_page05".to_string(),
            },
            policy: MaterializePolicy::TempEdited,
            page_edits: None,
            pdf_render_long_edge: 4096,
            rendered_pixels: Some(Arc::new(image)),
        };

        let prepared = manager
            .session()
            .materialize(&request, &cancel, generation)
            .unwrap();

        assert!(!prepared.is_original());
        assert_eq!(
            prepared.path().file_name().unwrap().to_string_lossy(),
            "page04_page05.png"
        );
        assert!(prepared.path().metadata().unwrap().len() > 0);
    }

    #[test]
    fn zero_pdf_edge_is_normalized_at_the_generic_materializer_boundary() {
        assert_eq!(normalized_pdf_render_long_edge(0), 4096);
        assert_eq!(normalized_pdf_render_long_edge(2048), 2048);
    }

    #[test]
    fn request_owned_lifetime_transition_deletes_only_before_successful_handoff() {
        assert_eq!(
            temp_ownership_transition(
                TempOwnershipStage::RequestOwned,
                TempOwnershipEvent::RequestDropped,
            ),
            TempOwnershipTransition::DeleteRequestFile
        );
        assert_eq!(
            temp_ownership_transition(
                TempOwnershipStage::RequestOwned,
                TempOwnershipEvent::LaunchSucceeded,
            ),
            TempOwnershipTransition::TransferToProcess
        );
        for stage in [
            TempOwnershipStage::Direct,
            TempOwnershipStage::ProcessOwned,
            TempOwnershipStage::Transferred,
        ] {
            assert_eq!(
                temp_ownership_transition(stage, TempOwnershipEvent::RequestDropped),
                TempOwnershipTransition::Retain
            );
        }
    }

    #[test]
    fn orphan_cleanup_never_selects_live_pid_or_unrelated_directory() {
        let entries = vec![
            PathBuf::from("ext-10"),
            PathBuf::from("ext-20"),
            PathBuf::from("cache"),
            PathBuf::from("ext-invalid"),
        ];
        assert_eq!(
            startup_cleanup_candidates(entries, 99, |pid| pid == 10),
            vec![PathBuf::from("ext-20")]
        );
        assert_eq!(
            startup_cleanup_candidates(
                vec![PathBuf::from("ext-10"), PathBuf::from("ext-99")],
                99,
                |_| true,
            ),
            vec![PathBuf::from("ext-99")],
            "the current PID directory predates this process and must be reclaimed before use"
        );
    }

    #[test]
    fn startup_cleanup_refuses_a_linked_root_without_touching_its_target() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let linked_root = temp.path().join("linked-root");
        std::fs::create_dir(&target).unwrap();
        let sentinel = target.join("ext-7").join("sentinel.txt");
        std::fs::create_dir(target.join("ext-7")).unwrap();
        std::fs::write(&sentinel, b"keep").unwrap();

        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(&target, &linked_root).is_err() {
            // Developer mode / symlink privilege is not guaranteed on every Windows test host.
            return;
        }
        #[cfg(not(windows))]
        std::os::unix::fs::symlink(&target, &linked_root).unwrap();

        cleanup_startup_directories(&linked_root, 7, |_| false);

        assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep");
        assert!(validate_real_directory(&linked_root, "test root").is_err());
    }

    #[test]
    fn generation_rejects_stale_request_before_launch_boundary() {
        let manager = Materializer::new_at(PathBuf::from("unused"), 77, false);
        let current = manager.begin_generation();
        assert!(manager.generation_is_current(current));
        manager.begin_generation();
        assert!(!manager.generation_is_current(current));
        assert!(
            manager
                .session()
                .ensure_current(&AtomicBool::new(false), current)
                .is_err()
        );
    }

    #[test]
    fn zip_original_writes_exact_entry_bytes_and_reuses_valid_target() {
        let temp = tempfile::tempdir().unwrap();
        let zip_path = temp.path().join("book.zip");
        let original = b"exact compressed entry bytes\0\xff";
        write_test_zip(&zip_path, "nested/page.jpg", original);
        let manager = Materializer::new_at(temp.path().join("materialized"), 79, false);
        let generation = manager.begin_generation();
        let cancel = Arc::new(AtomicBool::new(false));
        let request = zip_request(&zip_path, "nested/page.jpg");
        let mut session = manager.session();
        let mut first = session.materialize(&request, &cancel, generation).unwrap();
        assert_eq!(std::fs::read(first.path()).unwrap(), original);
        let first_path = first.path().to_path_buf();
        first.transfer_to_process_directory(false);
        drop(first);

        let mut second = session.materialize(&request, &cancel, generation).unwrap();
        assert_eq!(second.path(), first_path);
        std::fs::write(&first_path, b"changed output").unwrap();
        second.transfer_to_process_directory(false);
        drop(second);

        let mut after_output_change = session.materialize(&request, &cancel, generation).unwrap();
        assert_ne!(after_output_change.path(), first_path);
        assert_eq!(std::fs::read(after_output_change.path()).unwrap(), original);
        let after_output_change_path = after_output_change.path().to_path_buf();
        after_output_change.transfer_to_process_directory(false);
        drop(after_output_change);

        let changed_source = b"source entry changed and has a different size";
        write_test_zip(&zip_path, "nested/page.jpg", changed_source);
        let after_source_change = session.materialize(&request, &cancel, generation).unwrap();
        assert_ne!(after_source_change.path(), after_output_change_path);
        assert_eq!(
            std::fs::read(after_source_change.path()).unwrap(),
            changed_source
        );
    }

    #[test]
    fn atomic_lease_skips_foreign_collision_and_deletes_only_its_own_path() {
        let temp = tempfile::tempdir().unwrap();
        let manager = Materializer::new_at(temp.path().join("materialized"), 81, false);
        ensure_process_directory(&manager.inner).unwrap();
        let foreign = manager.inner.process_dir.join("page.jpg");
        std::fs::write(&foreign, b"foreign").unwrap();

        let lease = reserve_collision_path(&manager.inner, "page.jpg").unwrap();
        let leased_path = lease.path().to_path_buf();
        assert_ne!(leased_path, foreign);
        assert!(leased_path.exists());
        drop(lease);

        assert_eq!(std::fs::read(&foreign).unwrap(), b"foreign");
        assert!(!leased_path.exists());
        assert!(manager.inner.state.lock().unwrap().reserved.is_empty());
    }

    #[test]
    fn atomic_lease_writes_png_through_the_reserved_handle() {
        let temp = tempfile::tempdir().unwrap();
        let manager = Materializer::new_at(temp.path().join("materialized"), 84, false);
        ensure_process_directory(&manager.inner).unwrap();
        let mut lease = reserve_collision_path(&manager.inner, "page.png").unwrap();
        crate::capture::write_rgba_with_matte(
            lease.file_mut().unwrap(),
            crate::capture::CaptureFormat::Png,
            crate::capture::JpegMatte::Black,
            1,
            1,
            &[255, 0, 0, 255],
        )
        .unwrap();
        lease.flush().unwrap();
        let bytes = std::fs::read(lease.path()).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1, 1));
    }

    /// コピーは中身も名前も変えない。**再エンコードしない**ので、JPEG は JPEG のまま
    /// 渡り、ツールの「名前を付けて保存」にも利用者が知っている名前が出る。
    #[test]
    fn copying_a_real_file_preserves_its_bytes_and_its_name() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("original.jpg");
        std::fs::write(&source, b"not really a jpeg, but exact bytes matter").unwrap();
        let manager = Materializer::new_at(temp.path().join("materialized"), 81, false);
        let generation = manager.begin_generation();
        let cancel = Arc::new(AtomicBool::new(false));
        let request = MaterializeRequest {
            source: MaterializeSource::File {
                path: source.clone(),
                image_page: true,
            },
            policy: MaterializePolicy::TempOriginal,
            page_edits: None,
            pdf_render_long_edge: 4096,
            rendered_pixels: None,
        };

        let prepared = manager
            .session()
            .materialize(&request, &cancel, generation)
            .unwrap();

        assert_ne!(
            prepared.path(),
            source,
            "元ファイルそのものを渡してはいけない"
        );
        assert!(!prepared.is_original());
        assert_eq!(
            prepared.path().file_name().unwrap().to_string_lossy(),
            "original.jpg"
        );
        assert_eq!(
            std::fs::read(prepared.path()).unwrap(),
            std::fs::read(&source).unwrap()
        );
    }
    #[test]
    /// 段が違えば出力が違うので、指紋も違わなければならない。同じにすると
    /// 「AI まで」で作った一時ファイルが「編集まで」の要求へ再利用される。
    #[test]
    fn the_fingerprint_separates_requests_that_bake_to_different_depths() {
        let for_stage = |stage| {
            edit_fingerprint(
                &crate::adjustment::AdjustParams::default(),
                crate::rotation_db::Rotation::None,
                None,
                None,
                None,
                &crate::conceal::ConcealPreset::default(),
                None,
                None,
                0,
                None,
                stage,
                None,
            )
            .unwrap()
        };
        let edits = for_stage(crate::bake_stage::BakeStage::Edits);
        let ai = for_stage(crate::bake_stage::BakeStage::Ai);
        let display = for_stage(crate::bake_stage::BakeStage::DisplayAdjust);
        assert_ne!(edits, ai);
        assert_ne!(ai, display);
        assert_ne!(edits, display);
    }

    /// AI 段の**設定が変われば出力も変わる**ので、指紋も変わらなければならない。
    ///
    /// 実体化 cache は source stamp と指紋で再利用を決める。AI の設定が指紋に入っていな
    /// かったので、透過画像の AI 下地を黒→白に変えて再実行しても、前の黒下地のファイルを
    /// そのまま渡していた (v3.5.0 レビュー R05)。
    #[test]
    fn the_fingerprint_separates_requests_that_use_different_ai_settings() {
        let policy = |transparent_bg_mode, long_edge_px| crate::books::BookAiPolicy {
            feature_mode: crate::settings::AiFeatureMode::HighQuality,
            upscale_limit: crate::ai::upscale::AiProcessSizeLimit::square(long_edge_px),
            denoise_limit: crate::ai::upscale::AiProcessSizeLimit::square(long_edge_px),
            transparent_bg_mode,
        };
        let fingerprint = |ai_policy| {
            edit_fingerprint(
                &crate::adjustment::AdjustParams::default(),
                crate::rotation_db::Rotation::None,
                None,
                None,
                None,
                &crate::conceal::ConcealPreset::default(),
                None,
                None,
                0,
                None,
                crate::bake_stage::BakeStage::Ai,
                ai_policy,
            )
            .unwrap()
        };

        let black_bg = fingerprint(Some(policy(0, 4096)));

        assert_ne!(
            black_bg,
            fingerprint(Some(policy(1, 4096))),
            "AI の下地を変えたら別の出力"
        );
        assert_ne!(
            black_bg,
            fingerprint(Some(policy(0, 2048))),
            "AI の有効範囲を変えたら別の出力"
        );
        assert_ne!(
            black_bg,
            fingerprint(None),
            "AI を通す / 通さないは別の出力"
        );
    }

    #[test]
    fn edit_fingerprint_covers_erase_tolerance_and_comic_source_dimensions() {
        let fingerprint = |tolerance, dimensions| {
            edit_fingerprint(
                &crate::adjustment::AdjustParams::default(),
                crate::rotation_db::Rotation::None,
                None,
                None,
                None,
                &crate::conceal::ConcealPreset::default(),
                None,
                None,
                tolerance,
                dimensions,
                crate::bake_stage::BakeStage::default(),
                None,
            )
            .unwrap()
        };
        let baseline = fingerprint(8, None);
        assert_ne!(baseline, fingerprint(9, None));
        assert_ne!(baseline, fingerprint(8, Some([1920, 1080])));
    }

    #[test]
    fn shutdown_removes_non_keep_files_but_retains_keep_temp_files() {
        let temp = tempfile::tempdir().unwrap();
        let zip_path = temp.path().join("book.zip");
        write_test_zip(&zip_path, "page.jpg", b"page");

        let mut normal = Materializer::new_at(temp.path().join("normal"), 83, false);
        let generation = normal.begin_generation();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut prepared = normal
            .session()
            .materialize(&zip_request(&zip_path, "page.jpg"), &cancel, generation)
            .unwrap();
        let normal_path = prepared.path().to_path_buf();
        prepared.transfer_to_process_directory(false);
        drop(prepared);
        normal.shutdown();
        assert!(!normal_path.exists());

        let mut keep = Materializer::new_at(temp.path().join("keep"), 84, false);
        let generation = keep.begin_generation();
        let mut prepared = keep
            .session()
            .materialize(&zip_request(&zip_path, "page.jpg"), &cancel, generation)
            .unwrap();
        let keep_path = prepared.path().to_path_buf();
        prepared.transfer_to_process_directory(true);
        drop(prepared);
        keep.shutdown();
        assert!(keep_path.exists());
    }

    /// 「元のファイル」を ZIP 内ページに要求されたら、**それらしい一時ファイルを黙って
    /// 渡さない**。渡すと「編集したのに反映されない」という一番分かりにくい失敗になる。
    #[test]
    fn the_real_file_policy_refuses_a_zip_page_instead_of_handing_over_a_lookalike() {
        let temp = tempfile::tempdir().unwrap();
        let zip_path = temp.path().join("book.zip");
        write_test_zip(&zip_path, "page.jpg", b"page");
        let manager = Materializer::new_at(temp.path().join("materialized"), 80, false);
        let generation = manager.begin_generation();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut request = zip_request(&zip_path, "page.jpg");
        request.policy = MaterializePolicy::OriginalFile;
        let error = match manager.session().materialize(&request, &cancel, generation) {
            Ok(_) => panic!("a ZIP page has no original file to hand over"),
            Err(error) => error,
        };
        assert!(error.contains("元のファイルがありません"));
        assert!(error.contains("Ctrl+E"));
    }

    #[test]
    fn transferred_file_is_not_deleted_when_request_drops() {
        let temp = tempfile::tempdir().unwrap();
        let manager = Materializer::new_at(temp.path().to_path_buf(), 77, false);
        std::fs::create_dir_all(&manager.inner.process_dir).unwrap();
        manager.inner.state.lock().unwrap().initialized = true;
        let path = manager.inner.process_dir.join("page.png");
        std::fs::write(&path, b"png").unwrap();
        let stamp = file_stamp(&path).unwrap();
        let key = CacheKey {
            source: MaterializeSource::File {
                path: temp.path().join("source.png"),
                image_page: true,
            },
            policy: MaterializePolicy::TempEdited,
            pdf_render_long_edge: 4096,
            edit_fingerprint: [0; 32],
        };
        let mut prepared = PreparedMaterializedFile::created(
            path.clone(),
            Arc::clone(&manager.inner),
            key,
            stamp,
            stamp,
        );
        prepared.transfer_to_process_directory(false);
        drop(prepared);
        assert!(path.exists());
    }

    #[test]
    fn uncommitted_file_is_deleted_when_request_drops() {
        let temp = tempfile::tempdir().unwrap();
        let manager = Materializer::new_at(temp.path().to_path_buf(), 78, false);
        std::fs::create_dir_all(&manager.inner.process_dir).unwrap();
        let path = manager.inner.process_dir.join("page.png");
        std::fs::write(&path, b"png").unwrap();
        let stamp = file_stamp(&path).unwrap();
        let key = CacheKey {
            source: MaterializeSource::File {
                path: temp.path().join("source.png"),
                image_page: true,
            },
            policy: MaterializePolicy::TempEdited,
            pdf_render_long_edge: 4096,
            edit_fingerprint: [0; 32],
        };
        let prepared = PreparedMaterializedFile::created(
            path.clone(),
            Arc::clone(&manager.inner),
            key,
            stamp,
            stamp,
        );
        drop(prepared);
        assert!(!path.exists());
    }
}
