//! ONNX Runtime 初期化・セッション管理。
//!
//! デフォルトは DirectML EP で GPU 推論する。NVIDIA ユーザーが設定で
//! TensorRT バックエンドを有効化した場合は TensorRT + CUDA EP を使う。
//! `ort::init_from()` がプロセス内 1 回限りなので、バックエンド切り替えは
//! アプリ再起動が必要。
//!
//! セッションは ModelKind ごとに遅延作成・キャッシュする。
//! バックエンドはプロセス内で 1 つに固定されるため、
//! セッションキャッシュは ModelKind 単位 (EP 単位ではない) で持つ。
//!
//! `onnxruntime.dll` と `onnxruntime_providers_shared.dll` は exe に
//! `include_bytes!` で埋め込まれており、初回 AiRuntime 作成時に
//! `%APPDATA%/mimageviewer/` へ展開される (PDFium と同じパターン)。
//! これにより VC++ 再頒布可能パッケージを利用者に要求しない。
//!
//! TensorRT バックエンド時は別途 ~1.5 GB の TRT pack を
//! `%APPDATA%/mimageviewer/tensorrt/` にダウンロードする必要がある
//! (`tensorrt_pack` モジュール参照)。pack 不在/破損時は DirectML に自動フォールバック。

use std::collections::HashMap;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ort::session::Session;

use super::{AiBackend, AiError, ModelKind};

static ORT_DLL_BYTES: &[u8] = include_bytes!("../../vendor/ort/onnxruntime.dll");
static ORT_PROVIDERS_SHARED_BYTES: &[u8] =
    include_bytes!("../../vendor/ort/onnxruntime_providers_shared.dll");

/// ort::init_from() の結果。プロセス内 1 回限りなので OnceLock で保持。
/// バックエンド切り替え時は再起動が必要。
static ORT_INIT: OnceLock<Result<ActiveBackend, String>> = OnceLock::new();

/// 指定ディレクトリを Windows の DLL 検索パスの **先頭** に固定する。
///
/// `onnxruntime.dll` は CUDA/cuDNN/TensorRT EP DLL を内部 LoadLibraryW で
/// 芋づる式にロードする。それらの依存先 (`cudart64_*.dll`, `nvinfer.dll` 等) は
/// exe のあるディレクトリにないため、Windows のデフォルト DLL 検索パスでは
/// 見つからない。
///
/// 対策の組み合わせ:
///   1. `SetDllDirectoryW(dir)` — 全 LoadLibrary 呼び出しの最初に検索する
///      ディレクトリを置く (AddDllDirectory より強力で、フラグなし LoadLibrary も
///      対象になる)
///   2. PATH 環境変数の先頭追加 — 子プロセスや一部 API 用の保険
///   3. providers DLL の事前 LoadLibrary — フルパスで明示ロードしておけば
///      ORT 内部の LoadLibrary は「既にロード済み」として名前解決成功する
///
/// OnceLock 内から呼ばれるので副作用 (env::set_var) は 1 プロセス 1 回のみ。
fn prepend_dll_search_path(dir: &std::path::Path) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::LibraryLoader::{LoadLibraryW, SetDllDirectoryW};

        // (1) SetDllDirectoryW: 全 LoadLibrary 検索の先頭にこのディレクトリを置く
        let wide: Vec<u16> = dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        if let Err(e) = SetDllDirectoryW(windows::core::PCWSTR(wide.as_ptr())) {
            crate::logger::log(format!(
                "[AI] SetDllDirectoryW({}) failed: {e:?}",
                dir.display()
            ));
        }

        // (3) 主要な providers DLL を事前にフルパスでロードしておく。
        //     ORT 内部の LoadLibrary は "もう同名 DLL がロード済み" を見て
        //     これらを参照できる。
        let preload_targets = [
            "onnxruntime_providers_shared.dll",
            "onnxruntime_providers_cuda.dll",
            "onnxruntime_providers_tensorrt.dll",
        ];
        for name in &preload_targets {
            let full = dir.join(name);
            if !full.exists() {
                continue;
            }
            let wide_full: Vec<u16> = full
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            match LoadLibraryW(windows::core::PCWSTR(wide_full.as_ptr())) {
                Ok(_) => {
                    crate::logger::log(format!("[AI] preloaded {}", full.display()));
                }
                Err(e) => {
                    crate::logger::log(format!("[AI] preload {} failed: {e:?}", full.display()));
                }
            }
        }
    }

    // (2) PATH の先頭に追加
    let dir_str = dir.to_string_lossy();
    let new_path = match std::env::var_os("PATH") {
        Some(existing) => {
            let mut s = std::ffi::OsString::from(dir_str.as_ref());
            s.push(";");
            s.push(&existing);
            s
        }
        None => std::ffi::OsString::from(dir_str.as_ref()),
    };
    // Safety: 単一スレッドから OnceLock 内 1 回のみ呼ばれる。
    unsafe {
        std::env::set_var("PATH", new_path);
    }
    crate::logger::log(format!(
        "[AI] DLL 検索パスに追加 (SetDllDirectory + PATH + preload): {}",
        dir.display()
    ));
}

/// 解決済みバックエンド情報。
/// `requested` はユーザーが選んだもの、`effective` は実際に init できたもの。
/// pack 不在等で TRT が選択不可だった場合 `effective != requested` になる。
#[derive(Debug, Clone)]
pub struct ActiveBackend {
    pub requested: AiBackend,
    pub effective: AiBackend,
    pub dll_path: PathBuf,
    /// requested != effective のときフォールバック理由を記録 (UI 通知用)。
    pub fallback_reason: Option<String>,
}

/// onnxruntime.dll を初期化する。プロセス内 1 回のみ実行 (OnceLock)。
///
/// `requested` がユーザー選択バックエンド。TensorRt が選択されたが pack 不在/破損なら
/// DirectMl にフォールバック。CPU は DirectML 版 DLL のままセッション側で EP 登録なし
/// にする (CPU EP は両方の ORT DLL に内蔵)。
fn ensure_ort_initialized(requested: AiBackend) -> Result<ActiveBackend, AiError> {
    let result = ORT_INIT.get_or_init(|| -> Result<ActiveBackend, String> {
        let dir = crate::data_dir::get();
        std::fs::create_dir_all(&dir).map_err(|e| format!("data_dir create failed: {e}"))?;

        // TensorRt 要求時は pack 検証を試みる
        if requested == AiBackend::TensorRt {
            if super::tensorrt_pack::is_pack_installed() {
                let pack_dir = super::tensorrt_pack::pack_dir();
                let pack_dll = super::tensorrt_pack::pack_ort_dll_path();

                // CUDA / cuDNN / TensorRT の依存 DLL は onnxruntime.dll が
                // 内部から芋づる式にロードする。Windows のデフォルト DLL 検索パスは
                // exe のあるディレクトリなので、TRT pack ディレクトリを明示的に
                // 検索パスに追加する必要がある (PATH 先頭への prepend が一番確実)。
                prepend_dll_search_path(&pack_dir);

                match ort::init_from(&pack_dll) {
                    Ok(env_builder) => {
                        env_builder.commit();
                        crate::logger::log(format!(
                            "[AI] ORT initialized with TensorRT pack: {}",
                            pack_dll.display()
                        ));
                        return Ok(ActiveBackend {
                            requested: AiBackend::TensorRt,
                            effective: AiBackend::TensorRt,
                            dll_path: pack_dll,
                            fallback_reason: None,
                        });
                    }
                    Err(e) => {
                        let reason = format!("TensorRT pack の ort::init_from に失敗: {e}");
                        crate::logger::log(format!("[AI] {reason} — DirectML にフォールバック"));
                        // 下に落ちて DirectML 経路で初期化
                    }
                }
            } else {
                crate::logger::log(
                    "[AI] TensorRT バックエンドが要求されたが pack 未インストール — DirectML にフォールバック"
                        .to_string(),
                );
            }
        }

        // DirectML 経路 (デフォルト or TensorRt フォールバック or Cpu)
        let dll_path = dir.join("onnxruntime.dll");
        let providers_path = dir.join("onnxruntime_providers_shared.dll");

        crate::data_dir::extract_embedded_file(&dll_path, ORT_DLL_BYTES, "onnxruntime.dll")
            .map_err(|e| format!("onnxruntime.dll extract: {e}"))?;
        crate::data_dir::extract_embedded_file(
            &providers_path,
            ORT_PROVIDERS_SHARED_BYTES,
            "onnxruntime_providers_shared.dll",
        )
        .map_err(|e| format!("onnxruntime_providers_shared.dll extract: {e}"))?;

        ort::init_from(&dll_path)
            .map_err(|e| format!("ort::init_from: {e}"))?
            .commit();

        let fallback_reason = if requested == AiBackend::TensorRt {
            Some("TensorRT pack が利用できないため DirectML を使用しています".to_string())
        } else {
            None
        };
        let effective = match requested {
            AiBackend::TensorRt => AiBackend::DirectMl, // フォールバック
            other => other,
        };
        Ok(ActiveBackend {
            requested,
            effective,
            dll_path,
            fallback_reason,
        })
    });
    match result {
        Ok(active) => Ok(active.clone()),
        Err(e) => Err(AiError::Ort(e.clone())),
    }
}

/// ONNX Runtime ラッパー。
/// アプリ全体で 1 つだけ作成し、`Arc<AiRuntime>` で共有する。
///
/// ## Phase 3 アーキテクチャ
///
/// メインプロセスは **常に DirectML** で初期化される (`backend.effective` は
/// 常に `DirectMl` または `Cpu`)。TensorRT を使いたいときは `worker_pool` に
/// `TrtWorkerPool` を attach し、TRT 対応モデルの推論を子プロセスにルーティング
/// する。これにより:
///
/// - バックエンド切り替えで再起動が不要 (worker を attach/detach するだけ)
/// - TRT で動かないモデル (MI-GAN, Classifier) は DirectML で動かせる
/// - TRT クラッシュで GUI 全体が落ちない
pub struct AiRuntime {
    /// ModelKind → Session のキャッシュ (DirectML ローカルセッション)。
    /// Session::run() は &mut self なので Mutex が必要。
    sessions: Mutex<HashMap<ModelKind, Session>>,
    /// 現プロセスで実際にロードされたバックエンド情報。
    backend: ActiveBackend,
    /// TRT 推論ワーカープール。`None` なら全推論ローカル DirectML。
    /// 設定で TRT 有効化時に attach され、無効化で detach される (ホットリロード)。
    worker_pool: Mutex<Option<std::sync::Arc<super::trt_worker_pool::TrtWorkerPool>>>,
    /// UI に表示する worker 関連の最新通知 (Phase 3 Step 5、クラッシュ通知など)。
    /// `take_worker_notice()` で 1 回だけ取り出される。
    /// 設定: 起動失敗 / 推論中の死亡 / 手動 detach はここに乗らず、log のみ。
    worker_notice: Mutex<Option<WorkerNotice>>,
}

/// Phase 3 Step 5: TRT ワーカー関連の UI 通知。
///
/// `kind` で表示色 (赤/青) と動作 (再起動可能か) を分岐する。
#[derive(Debug, Clone)]
pub struct WorkerNotice {
    /// 通知種別 (UI で文言/動作分岐用)。
    pub kind: WorkerNoticeKind,
    /// 詳細メッセージ (英語/日本語混在 OK、log にも出る)。
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerNoticeKind {
    /// 起動を試みたが TRT pack 不在 / engine 不整合 / その他で起動失敗。
    /// バックエンド設定は TensorRt のままだが、実体は DirectML で動作している。
    SpawnFailed,
    /// 起動後の推論中に子プロセスが死亡 (DLL クラッシュ等)。
    /// 自動 detach 済みで、以降の推論は DirectML にフォールバックされる。
    DiedDuringInfer,
}

impl AiRuntime {
    /// 新しい AiRuntime を作成する (DirectML バックエンド、互換 API)。
    ///
    /// テスト・ベンチ・既存呼び出し用のショートハンド。
    /// アプリ本体は `new_with_backend(backend)` を使ってユーザー設定を反映する。
    pub fn new() -> Result<Self, AiError> {
        Self::new_with_backend(AiBackend::DirectMl)
    }

    /// 指定バックエンドで新しい AiRuntime を作成する。
    ///
    /// 内部で `ort::init_from` を呼んで onnxruntime.dll を
    /// `%APPDATA%/mimageviewer/` (DirectML) または
    /// `%APPDATA%/mimageviewer/tensorrt/` (TensorRT pack) からロードする。
    /// OnceLock により最初の 1 回のみ実行され、以降は cache を返す。
    /// 異なる backend で 2 回呼ばれても初回の選択が固定される (ort::init_from の制約)。
    ///
    /// FP16 推論は TensorRT 利用時に常時 ON (画質劣化は知覚不能、1.5-2x 高速化)。
    /// FP32 比較が必要なデバッグ用途のため設定として外出ししない方針。
    pub fn new_with_backend(backend: AiBackend) -> Result<Self, AiError> {
        let active = ensure_ort_initialized(backend)?;
        Ok(AiRuntime {
            sessions: Mutex::new(HashMap::new()),
            backend: active,
            worker_pool: Mutex::new(None),
            worker_notice: Mutex::new(None),
        })
    }

    /// 実際にロードされたバックエンドを返す。UI の表示や log 用。
    /// pack 不在で TRT が DirectML にフォールバックした場合は `effective == DirectMl`。
    #[allow(dead_code)]
    pub fn active_backend(&self) -> &ActiveBackend {
        &self.backend
    }

    /// TRT ワーカープールを attach する (Phase 3、ホットリロード対応)。
    /// 既に attach されている場合は古いものを drop してから差し替える
    /// (drop で worker プロセスが shutdown される)。
    pub fn attach_worker_pool(&self, pool: std::sync::Arc<super::trt_worker_pool::TrtWorkerPool>) {
        *self.worker_pool.lock().unwrap() = Some(pool);
        crate::logger::log("[AI] TRT worker pool attached".to_string());
    }

    /// TRT ワーカープールを detach する (停止)。
    #[allow(dead_code)]
    pub fn detach_worker_pool(&self) {
        let prev = self.worker_pool.lock().unwrap().take();
        if let Some(pool) = prev {
            // Arc 内 TrtWorkerPool は Drop で shutdown_and_wait される。
            // ここでは Arc::strong_count をチェックして他で使われていなければ
            // 即時 drop されるが、共有されていたら最後の Arc が drop されたとき
            // にクリーンアップされる。
            crate::logger::log(format!(
                "[AI] TRT worker pool detached (other Arc refs: {})",
                std::sync::Arc::strong_count(&pool) - 1
            ));
            drop(pool);
        }
    }

    /// TRT ワーカープールが attach されているか。
    #[allow(dead_code)]
    pub fn has_worker_pool(&self) -> bool {
        self.worker_pool.lock().unwrap().is_some()
    }

    /// UI 通知を 1 回だけ取り出す (consume)。
    ///
    /// アプリの update ループで毎フレーム呼び、`Some(notice)` が返ったら
    /// バナー表示に転写する。読み出し済みの通知は次の `report_*` まで `None`。
    pub fn take_worker_notice(&self) -> Option<WorkerNotice> {
        self.worker_notice.lock().unwrap().take()
    }

    /// 起動失敗を UI 通知に登録する (Phase 3 Step 5)。
    ///
    /// `spawn_trt_worker_pool` 内で `TrtWorkerPool::start()` が Err を返したときに
    /// 呼ぶ。pool は attach されないので detach は不要、log + UI 通知のみ。
    pub fn report_worker_spawn_failed(&self, detail: String) {
        crate::logger::log(format!("[AI] TRT worker spawn failed: {detail}"));
        *self.worker_notice.lock().unwrap() = Some(WorkerNotice {
            kind: WorkerNoticeKind::SpawnFailed,
            detail,
        });
    }

    /// 推論中の死亡を UI 通知に登録し、pool を detach する (Phase 3 Step 5)。
    ///
    /// 内部で `detach_worker_pool()` を呼ぶので、以降の `should_route_to_worker` は
    /// 即 `false` を返し、推論は DirectML にフォールバックする。
    fn report_worker_died(&self, detail: String) {
        crate::logger::log(format!("[AI] TRT worker died during infer: {detail}"));
        // 同 lock 内で detach すると detach 内部の logger も無問題 (Mutex は別)。
        self.detach_worker_pool();
        *self.worker_notice.lock().unwrap() = Some(WorkerNotice {
            kind: WorkerNoticeKind::DiedDuringInfer,
            detail,
        });
    }

    /// 指定モデルが TRT ワーカーへルーティングされるかを判定する。
    ///
    /// 条件:
    /// - worker pool が attach 済み
    /// - そのモデルが TRT で動かしてうれしいタイプ
    ///   - Upscale 系 (5 モデル): 1.4-3.4x 高速化
    ///   - Denoise: 4.5x 高速化
    ///   - MI-GAN: TRT エンジンビルドが 5+ 分かかるため、安定するまで DirectML
    ///   - Classifier: TRT で engine 生成されない (CUDA EP fallback、効果なし)
    pub fn should_route_to_worker(&self, kind: ModelKind) -> bool {
        if !self.has_worker_pool() {
            return false;
        }
        match kind {
            ModelKind::InpaintMiGan => false,
            ModelKind::ClassifierMobileNet => false,
            ModelKind::UpscaleRealEsrganX4Plus
            | ModelKind::UpscaleRealEsrganAnime6B
            | ModelKind::UpscaleRealEsrGeneralV3
            | ModelKind::UpscaleRealCugan4x
            | ModelKind::UpscaleNmkdSiax4x
            | ModelKind::DenoiseRealplksr => true,
        }
    }

    /// TRT ワーカー経由で推論を実行する。
    ///
    /// 呼び出し前に `should_route_to_worker(kind) == true` でなければならない
    /// (worker_pool が None だと panic ではなく Err を返す)。
    /// 戻り値: `(出力 shape NCHW, 平坦化された Vec<f32>)`。
    ///
    /// ワーカー側は initial LoadModel を要求する仕様なので、ここで lazy load する
    /// (1 度ロードしたモデルは worker 側で kept、再 LoadModel は idempotent)。
    pub fn infer_via_worker(
        &self,
        kind: ModelKind,
        input: &ndarray::Array4<f32>,
    ) -> Result<(Vec<i64>, Vec<f32>), AiError> {
        let pool_opt = self.worker_pool.lock().unwrap().clone();
        let pool = pool_opt.ok_or_else(|| {
            AiError::Ort("infer_via_worker: pool が attach されていない".to_string())
        })?;
        // ワーカー側で lazy load (idempotent)。failure は Err にフォワード。
        if let Err(e) = pool.load_model(kind) {
            // load_model 内で I/O 失敗を検出していたら pool.is_dead() == true。
            // その場合はここで報告 + detach して、以降の呼び出しは DirectML へ。
            if pool.is_dead() {
                self.report_worker_died(format!("load_model({kind:?}): {e}"));
            }
            return Err(AiError::Ort(format!("worker.load_model({kind:?}): {e}")));
        }
        match pool.infer(kind, input) {
            Ok(out) => Ok(out),
            Err(e) => {
                if pool.is_dead() {
                    self.report_worker_died(format!("infer({kind:?}): {e}"));
                }
                Err(AiError::Ort(format!("worker.infer({kind:?}): {e}")))
            }
        }
    }

    /// 指定モデルのセッションがロード済みか確認する。
    pub fn is_loaded(&self, kind: ModelKind) -> bool {
        self.sessions.lock().unwrap().contains_key(&kind)
    }

    /// モデルファイルからセッションをロードしてキャッシュする。
    /// すでにロード済みの場合は何もしない。
    pub fn load_model(&self, kind: ModelKind, model_path: &Path) -> Result<(), AiError> {
        self.load_model_inner(kind, model_path, false)
    }

    /// CPU 専用でモデルをロードする（DirectML 非互換モデル用）。
    pub fn load_model_cpu(&self, kind: ModelKind, model_path: &Path) -> Result<(), AiError> {
        self.load_model_inner(kind, model_path, true)
    }

    fn load_model_inner(
        &self,
        kind: ModelKind,
        model_path: &Path,
        force_cpu: bool,
    ) -> Result<(), AiError> {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.contains_key(&kind) {
            return Ok(());
        }

        let backend_label = if force_cpu {
            "CPU (forced)"
        } else {
            match self.backend.effective {
                AiBackend::DirectMl => "DirectML",
                AiBackend::TensorRt => "TensorRT",
                AiBackend::Cpu => "CPU",
            }
        };
        crate::logger::log(format!(
            "[AI] Loading model {:?} from {} ({})",
            kind,
            model_path.display(),
            backend_label
        ));

        let mut builder =
            Session::builder().map_err(|e| AiError::Ort(format!("Session::builder: {e}")))?;

        builder = builder
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
            .map_err(|e| AiError::Ort(format!("optimization_level: {e}")))?;

        builder = builder
            .with_intra_threads(4)
            .map_err(|e| AiError::Ort(format!("intra_threads: {e}")))?;

        if !force_cpu {
            builder = self.register_eps(builder, kind);
        }

        let session = builder
            .commit_from_file(model_path)
            .map_err(|e| AiError::Ort(format!("Failed to load {}: {e}", model_path.display())))?;

        crate::logger::log(format!("[AI] Model {:?} loaded successfully", kind));
        sessions.insert(kind, session);
        Ok(())
    }

    /// 現在のバックエンドに応じて EP を登録する。失敗時は CPU フォールバック。
    fn register_eps(
        &self,
        builder: ort::session::builder::SessionBuilder,
        kind: ModelKind,
    ) -> ort::session::builder::SessionBuilder {
        match self.backend.effective {
            AiBackend::DirectMl => self.register_directml_ep(builder),
            AiBackend::TensorRt => self.register_tensorrt_eps(builder, kind),
            AiBackend::Cpu => builder, // EP 未登録 = CPU
        }
    }

    fn register_directml_ep(
        &self,
        builder: ort::session::builder::SessionBuilder,
    ) -> ort::session::builder::SessionBuilder {
        match builder.with_execution_providers([ort::ep::DirectML::default().build()]) {
            Ok(b) => b,
            Err(e) => {
                crate::logger::log(format!(
                    "[AI] DirectML EP registration failed, falling back to CPU: {}",
                    e
                ));
                e.recover()
            }
        }
    }

    /// TensorRT + CUDA EP を登録する。
    /// - エンジンキャッシュは `%APPDATA%/mimageviewer/tensorrt-engines/<model_kind>/` に
    ///   モデルごとに分ける (キャッシュ削除時にモデル単位で消せる)
    /// - FP16 は Settings から制御 (デフォルト ON)
    /// - max_workspace_size は VRAM の 30% (上限 4 GiB) を割り当て
    /// - TRT EP が失敗した場合 CUDA EP がフォールバック、両方失敗で CPU
    fn register_tensorrt_eps(
        &self,
        builder: ort::session::builder::SessionBuilder,
        kind: ModelKind,
    ) -> ort::session::builder::SessionBuilder {
        let cache_dir = super::tensorrt_pack::engine_cache_dir().join(kind.as_str());
        if let Err(e) = std::fs::create_dir_all(&cache_dir) {
            crate::logger::log(format!(
                "[AI] TRT engine cache dir create failed ({}): {} — そのまま続行",
                cache_dir.display(),
                e
            ));
        }

        // Workspace は VRAM 容量の 30%、上限 4 GiB。
        // サムネイル・通常デコード・補正と GPU メモリを共有するため取り過ぎない。
        const WORKSPACE_CAP_BYTES: usize = 4 * 1024 * 1024 * 1024; // 4 GiB
        let workspace_bytes = (crate::gpu_info::vram_cap_from_percent(30) as usize)
            .min(WORKSPACE_CAP_BYTES)
            .max(512 * 1024 * 1024); // 最低 512 MiB

        // builder optimization level 3 (デフォルト) で engine をビルドする。
        //
        // 当初 5 (最高) を採用していたが、Apr 28 のユーザー実測で:
        // - level 5: x4plus 1 個のコンパイルだけで 5 分超、全 6 モデルだと 30 分超
        // - level 3: 同モデル 60〜90 秒、全 6 モデルで 6〜10 分 (見積もり通り)
        // - 推論時間の差は anime6b 994ms → 951ms = 5% で、5+ 分のビルド待ちに
        //   見合わない (level 5 の理論値は +10-20% だが実測はずっと小さい)。
        //
        // 一般のビューワー利用では「初回セットアップを早く終わらせる」ほうが
        // 「runtime 推論が 5% 速い」より価値が高いと判断して 3 に固定。
        const TRT_BUILDER_OPT_LEVEL: u8 = 3;

        // FP16 推論は常時 ON。画質劣化は ESRGAN クラスのモデルでは知覚不能
        // (1 ピクセル平均 0.05-0.2/255、PSNR -0.05 dB 程度)、1.5-2x 高速化が
        // 効くため利点が大きい。FP32 比較が必要な場面のため設定 UI には出さない。
        const TRT_FP16: bool = true;

        // ハードウェア互換 (kAMPERE_PLUS) モードを常時 ON。
        // ORT の `trt_engine_hw_compatible=1` を立てると、生成 engine が sm80+
        // (Ampere/Ada/Hopper/Blackwell) 全 GPU で動作するようになる。これにより
        // mikage 機で 1 度 build した engine を全 RTX 30/40/50 ユーザーへ配布できる
        // (`nvinfer_builder_resource_*` を再配布せず済むため法的にクリーン)。
        //
        // 性能影響 (RTX 4090 実測 Apr 28):
        //   - wall time 増分: 平均 +5.4%、最大 +8.8%
        //   - session_run/tile: 平均 +8.8%、最大 +15.8% (anime6b)
        //   - DirectML 比では依然 1.8-4x 高速で、ユーザー体感差はほぼなし
        const TRT_HW_COMPAT: bool = true;

        crate::logger::log(format!(
            "[AI] TRT EP options: cache_path={}, fp16={}, hw_compat={}, workspace={} MiB, builder_opt_level={}",
            cache_dir.display(),
            TRT_FP16,
            TRT_HW_COMPAT,
            workspace_bytes / (1024 * 1024),
            TRT_BUILDER_OPT_LEVEL
        ));

        let trt = ort::ep::TensorRT::default()
            .with_engine_cache(true)
            .with_engine_cache_path(cache_dir.to_string_lossy().to_string())
            .with_fp16(TRT_FP16)
            .with_max_workspace_size(workspace_bytes)
            .with_builder_optimization_level(TRT_BUILDER_OPT_LEVEL)
            .with_engine_hw_compatible(TRT_HW_COMPAT)
            .build();
        let cuda = ort::ep::CUDA::default().build();
        match builder.with_execution_providers([trt, cuda]) {
            Ok(b) => b,
            Err(e) => {
                crate::logger::log(format!(
                    "[AI] TensorRT/CUDA EP registration failed for {:?}: {} — CPU フォールバック",
                    kind, e
                ));
                e.recover()
            }
        }
    }

    /// セッションをロック取得して推論を実行するクロージャを呼ぶ。
    ///
    /// `Session::run()` が `&mut self` を要求するため、
    /// この関数でロック範囲を限定する。
    pub fn with_session<F, R>(&self, kind: ModelKind, f: F) -> Result<R, AiError>
    where
        F: FnOnce(&mut Session) -> Result<R, AiError>,
    {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(&kind)
            .ok_or(AiError::ModelNotFound(kind))?;
        f(session)
    }

    /// 指定モデルのセッションをアンロードする。
    #[allow(dead_code)]
    pub fn unload_model(&self, kind: ModelKind) {
        self.sessions.lock().unwrap().remove(&kind);
    }
}

// AiRuntime の Mutex 内部の Session は Send+Sync。
// AiRuntime 自体を Arc で共有して複数スレッドからアクセスする。
unsafe impl Send for AiRuntime {}
unsafe impl Sync for AiRuntime {}
