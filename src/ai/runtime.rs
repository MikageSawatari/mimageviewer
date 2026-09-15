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
//! `include_bytes!` で埋め込まれており、共通 runtime owner の worker が
//! `%APPDATA%/mimageviewer/` へ展開する (PDFium と同じパターン)。Microsoft 公式
//! app-local VC runtime は配布 exe の隣へ置くため、利用者の追加導入は不要。
//!
//! TensorRT バックエンド時は別途 ~1.5 GB の TRT pack を
//! `%APPDATA%/mimageviewer/tensorrt/` にダウンロードする必要がある
//! (`tensorrt_pack` モジュール参照)。pack 不在/破損時は DirectML に自動フォールバック。

use std::collections::HashMap;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use ort::session::Session;

use super::{AiBackend, AiError, ModelKind};

/// GUI process 全体の DirectML runtime 初期化を一度だけ所有する。
///
/// App、Remote、materializer は同じ `Arc<AiRuntimeInitOwner>` を共有し、ここ以外から
/// `AiRuntime` を構築しない。`Ready` / `Failed` は process lifetime 中の terminal state。
pub struct AiRuntimeInitOwner {
    state: Mutex<AiRuntimeInitState>,
    changed: Condvar,
    trt_worker_lifecycle: Arc<super::trt_worker_lifecycle::TrtWorkerLifecycleOwner>,
}

enum AiRuntimeInitState {
    Dormant,
    Initializing,
    Ready(Arc<AiRuntime>),
    Failed(Arc<AiError>),
}

#[derive(Clone)]
pub enum AiRuntimeInitSnapshot {
    Dormant,
    Initializing,
    Ready(Arc<AiRuntime>),
    Failed(Arc<AiError>),
}

pub enum AiRuntimeInitWait {
    Ready(Arc<AiRuntime>),
    Failed(Arc<AiError>),
    Cancelled,
}

impl AiRuntimeInitOwner {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(AiRuntimeInitState::Dormant),
            changed: Condvar::new(),
            trt_worker_lifecycle: crate::ai::trt_worker_lifecycle::TrtWorkerLifecycleOwner::new(),
        })
    }

    pub fn trt_worker_lifecycle(
        &self,
    ) -> Arc<super::trt_worker_lifecycle::TrtWorkerLifecycleOwner> {
        Arc::clone(&self.trt_worker_lifecycle)
    }

    pub fn snapshot(&self) -> AiRuntimeInitSnapshot {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        match &*state {
            AiRuntimeInitState::Dormant => AiRuntimeInitSnapshot::Dormant,
            AiRuntimeInitState::Initializing => AiRuntimeInitSnapshot::Initializing,
            AiRuntimeInitState::Ready(runtime) => AiRuntimeInitSnapshot::Ready(Arc::clone(runtime)),
            AiRuntimeInitState::Failed(error) => AiRuntimeInitSnapshot::Failed(Arc::clone(error)),
        }
    }

    pub fn ready_runtime(&self) -> Option<Arc<AiRuntime>> {
        match self.snapshot() {
            AiRuntimeInitSnapshot::Ready(runtime) => Some(runtime),
            AiRuntimeInitSnapshot::Dormant
            | AiRuntimeInitSnapshot::Initializing
            | AiRuntimeInitSnapshot::Failed(_) => None,
        }
    }

    /// DirectML runtime worker を一度だけ開始する。呼び出し元は待たない。
    pub fn start(self: &Arc<Self>, repaint: impl Fn() + Send + Sync + 'static) -> bool {
        let trt_worker_lifecycle = Arc::clone(&self.trt_worker_lifecycle);
        self.start_with(
            "ai-runtime-init",
            move || {
                let created = AiRuntime::new_with_backend_and_trt_lifecycle(
                    AiBackend::DirectMl,
                    trt_worker_lifecycle,
                );
                match &created {
                    Ok(runtime) => {
                        let active = runtime.active_backend();
                        crate::logger::log(format!(
                            "[AI] Runtime initialized (DirectML always in main, requested={:?}, effective={:?})",
                            active.requested, active.effective
                        ));
                    }
                    Err(error) => {
                        crate::logger::log(format!("[AI] Runtime init failed: {error}"));
                    }
                }
                created
            },
            repaint,
        )
    }

    fn start_with<F, R>(self: &Arc<Self>, thread_name: &str, initialize: F, repaint: R) -> bool
    where
        F: FnOnce() -> Result<AiRuntime, AiError> + Send + 'static,
        R: Fn() + Send + Sync + 'static,
    {
        self.start_with_spawner(thread_name, initialize, repaint, |builder, task| {
            builder.spawn(task)
        })
    }

    fn start_with_spawner<F, R, S>(
        self: &Arc<Self>,
        thread_name: &str,
        initialize: F,
        repaint: R,
        spawn: S,
    ) -> bool
    where
        F: FnOnce() -> Result<AiRuntime, AiError> + Send + 'static,
        R: Fn() + Send + Sync + 'static,
        S: FnOnce(
            std::thread::Builder,
            Box<dyn FnOnce() + Send>,
        ) -> std::io::Result<std::thread::JoinHandle<()>>,
    {
        {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if !matches!(*state, AiRuntimeInitState::Dormant) {
                return false;
            }
            *state = AiRuntimeInitState::Initializing;
        }
        self.changed.notify_all();

        let owner = Arc::clone(self);
        let repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(repaint);
        let repaint_worker = Arc::clone(&repaint);
        let task: Box<dyn FnOnce() + Send> = Box::new(move || {
            let created = std::panic::catch_unwind(std::panic::AssertUnwindSafe(initialize))
                .unwrap_or_else(|_| {
                    let error =
                        AiError::Ort("AI runtime initialization worker panicked".to_owned());
                    crate::logger::log(format!("[AI] Runtime init failed: {error}"));
                    Err(error)
                });
            owner.publish_terminal(created);
            repaint_worker();
        });
        let spawned = spawn(
            std::thread::Builder::new().name(thread_name.to_owned()),
            task,
        );
        if let Err(error) = spawned {
            let error = AiError::Io(error);
            crate::logger::log(format!("[AI] Runtime init worker spawn failed: {error}"));
            self.publish_terminal(Err(error));
            repaint();
        }
        true
    }

    fn publish_terminal(&self, created: Result<AiRuntime, AiError>) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !matches!(*state, AiRuntimeInitState::Initializing) {
            return;
        }
        *state = match created {
            Ok(runtime) => AiRuntimeInitState::Ready(Arc::new(runtime)),
            Err(error) => AiRuntimeInitState::Failed(Arc::new(error)),
        };
        drop(state);
        self.changed.notify_all();
    }

    /// background request だけが使う cancel-aware terminal wait。
    ///
    /// Condvar は短い timeout で待ち、request 固有の cancel / generation を再確認する。
    /// process-global 初期化自体は request cancel では止めない。
    pub fn wait_terminal_while(&self, mut keep_waiting: impl FnMut() -> bool) -> AiRuntimeInitWait {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        loop {
            // A request cancellation is the consumer's terminal state even when it
            // races with process-global initialization publication.  In particular,
            // do not hand a newly ready runtime to a request that was already dropped.
            if !keep_waiting() {
                return AiRuntimeInitWait::Cancelled;
            }
            match &*state {
                AiRuntimeInitState::Ready(runtime) => {
                    return AiRuntimeInitWait::Ready(Arc::clone(runtime));
                }
                AiRuntimeInitState::Failed(error) => {
                    return AiRuntimeInitWait::Failed(Arc::clone(error));
                }
                AiRuntimeInitState::Dormant | AiRuntimeInitState::Initializing => {}
            }
            let waited = self
                .changed
                .wait_timeout(state, std::time::Duration::from_millis(50))
                .unwrap_or_else(|error| error.into_inner());
            state = waited.0;
        }
    }

    #[cfg(test)]
    pub(crate) fn failed_for_test(message: &str) -> Arc<Self> {
        let owner = Self::new();
        *owner.state.lock().unwrap() =
            AiRuntimeInitState::Failed(Arc::new(AiError::Ort(message.to_owned())));
        owner
    }
}

// portable ビルドでは埋め込まず exe 隣の loose onnxruntime*.dll を使う (native_assets 参照)。
#[cfg(not(feature = "portable"))]
static ORT_DLL_BYTES: &[u8] = include_bytes!("../../vendor/ort/onnxruntime.dll");
#[cfg(not(feature = "portable"))]
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
///   3. `onnxruntime_providers_shared.dll` の事前 LoadLibrary — フルパスで明示
///      ロードしておけば ORT 内部の LoadLibrary は「既にロード済み」として名前解決成功する
///      (CUDA / TensorRT の provider DLL は対象外。理由は下のコメント参照)
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

        // (3) provider bridge の実体である providers_shared.dll だけ事前にフルパスで
        //     ロードしておく。ORT 内部の LoadLibrary は "もう同名 DLL がロード済み" を
        //     見てこれを参照できる。
        //
        //     `onnxruntime_providers_cuda.dll` / `onnxruntime_providers_tensorrt.dll` は
        //     **ここに足さないこと**。これらは onnxruntime.dll が provider bridge を
        //     初期化した後にその手順で読ませる前提の DLL で、単独 LoadLibrary すると
        //     DllMain 内でアクセス違反を起こす。ローダが握って 0x8007045A
        //     (ERROR_DLL_INIT_FAILED) に変換するのでプロセスは生き延びるが、その裏で
        //     WER が走るため 1 本あたり数秒を捨てたうえ、Windows のイベントログには
        //     mimageviewer-core.exe のクラッシュとして記録が残る。2026-04 の PoC で
        //     3 手段を束にして入れたときの名残で、実際は (1) SetDllDirectoryW と
        //     (2) PATH 追加だけで TensorRT EP は動く (2026-08 に実測して除去)。
        let preload_targets = ["onnxruntime_providers_shared.dll"];
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
        let mut trt_fallback_reason = None;

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
                        trt_fallback_reason = Some(reason);
                        // 下に落ちて DirectML 経路で初期化
                    }
                }
            } else {
                let reason = "TensorRT pack が未インストールです".to_owned();
                crate::logger::log(format!(
                    "[AI] TensorRT バックエンドが要求されたが {reason} — DirectML にフォールバック"
                ));
                trt_fallback_reason = Some(reason);
            }
        }

        // DirectML 経路 (デフォルト or TensorRt フォールバック or Cpu)
        // portable: exe 隣の loose onnxruntime.dll を使う (providers_shared.dll も同居必須なので
        // 存在確認だけ行う)。通常: 埋め込みバイト列を data_dir へ展開する。
        #[cfg(feature = "portable")]
        let dll_path = {
            let _ = crate::native_assets::bundled("onnxruntime_providers_shared.dll")?;
            crate::native_assets::bundled("onnxruntime.dll")?
        };
        #[cfg(not(feature = "portable"))]
        let dll_path = {
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
            dll_path
        };

        ort::init_from(&dll_path)
            .map_err(|e| format!("ort::init_from: {e}"))?
            .commit();

        let fallback_reason = if requested == AiBackend::TensorRt {
            Some(trt_fallback_reason.unwrap_or_else(|| {
                "TensorRT pack が利用できないため DirectML を使用しています".to_string()
            }))
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
/// 常に `DirectMl` または `Cpu`)。TensorRT 対象の推論は process 共通の typed
/// lifecycle owner から request 単位の worker route を取得する。これにより:
///
/// - バックエンド切り替えでアプリ再起動が不要
/// - TRT で動かないモデル (MI-GAN など) は DirectML で動かせる
/// - TRT クラッシュで GUI 全体が落ちない
pub struct AiRuntime {
    /// ModelKind → Session のキャッシュ (DirectML ローカルセッション)。
    /// Session::run() は &mut self なので Mutex が必要。
    sessions: Mutex<HashMap<ModelKind, Session>>,
    /// 現プロセスで実際にロードされたバックエンド情報。
    backend: ActiveBackend,
    /// GUI process で共有する TensorRT child-process lifecycle。pool、retry、notice、
    /// backend/pack revision はこの owner の排他 state だけが所有する。
    trt_worker_lifecycle: Arc<super::trt_worker_lifecycle::TrtWorkerLifecycleOwner>,
}

impl AiRuntime {
    /// 新しい AiRuntime を作成する (DirectML バックエンド、互換 API)。
    ///
    /// テスト・ベンチなど独立process用のショートハンド。GUI process は
    /// `AiRuntimeInitOwner` だけが `new_with_backend` を呼ぶ。
    pub fn new() -> Result<Self, AiError> {
        Self::new_with_backend(AiBackend::DirectMl)
    }

    /// 指定バックエンドで新しい AiRuntime を作成する。
    ///
    /// 内部で `ort::init_from` を呼んで onnxruntime.dll を
    /// `%APPDATA%/mimageviewer/` (DirectML) または
    /// `%APPDATA%/mimageviewer/tensorrt/` (TensorRT pack) からロードする。
    /// OnceLock により最初の成功初期化を固定し、以降は同じ environment を返す。
    /// 異なる backend で 2 回呼ばれても初回の選択が固定される (ort::init_from の制約)。
    ///
    /// FP16 推論は TensorRT 利用時に常時 ON (画質劣化は知覚不能、1.5-2x 高速化)。
    /// FP32 比較が必要なデバッグ用途のため設定として外出ししない方針。
    pub fn new_with_backend(backend: AiBackend) -> Result<Self, AiError> {
        Self::new_with_backend_and_trt_lifecycle(
            backend,
            super::trt_worker_lifecycle::TrtWorkerLifecycleOwner::new(),
        )
    }

    pub fn new_with_backend_and_worker_exe(
        backend: AiBackend,
        worker_exe: PathBuf,
    ) -> Result<Self, AiError> {
        Self::new_with_backend_and_trt_lifecycle(
            backend,
            super::trt_worker_lifecycle::TrtWorkerLifecycleOwner::new_for_diagnostic_worker(
                worker_exe,
            ),
        )
    }

    fn new_with_backend_and_trt_lifecycle(
        backend: AiBackend,
        trt_worker_lifecycle: Arc<super::trt_worker_lifecycle::TrtWorkerLifecycleOwner>,
    ) -> Result<Self, AiError> {
        let active = ensure_ort_initialized(backend)?;
        Ok(AiRuntime {
            sessions: Mutex::new(HashMap::new()),
            backend: active,
            trt_worker_lifecycle,
        })
    }

    /// 実際にロードされたバックエンドを返す。UI の表示や log 用。
    /// pack 不在で TRT が DirectML にフォールバックした場合は `effective == DirectMl`。
    #[allow(dead_code)]
    pub fn active_backend(&self) -> &ActiveBackend {
        &self.backend
    }

    pub fn trt_worker_lifecycle(
        &self,
    ) -> Arc<super::trt_worker_lifecycle::TrtWorkerLifecycleOwner> {
        Arc::clone(&self.trt_worker_lifecycle)
    }

    pub fn trt_worker_snapshot(&self) -> super::trt_worker_lifecycle::TrtWorkerSnapshot {
        self.trt_worker_lifecycle.snapshot()
    }

    pub fn trt_route_generation(
        &self,
        kind: ModelKind,
    ) -> super::trt_worker_lifecycle::TrtRouteGeneration {
        self.trt_worker_lifecycle.route_generation(kind)
    }

    pub fn trt_route_or_begin(
        &self,
        kind: ModelKind,
    ) -> super::trt_worker_lifecycle::TrtInferenceRoute {
        self.trt_worker_lifecycle.route_or_begin(kind)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn fake_runtime() -> AiRuntime {
        AiRuntime {
            sessions: Mutex::new(HashMap::new()),
            backend: ActiveBackend {
                requested: AiBackend::DirectMl,
                effective: AiBackend::DirectMl,
                dll_path: PathBuf::from("fake-onnxruntime.dll"),
                fallback_reason: None,
            },
            trt_worker_lifecycle: crate::ai::trt_worker_lifecycle::TrtWorkerLifecycleOwner::new(),
        }
    }

    #[test]
    fn init_owner_starts_once_and_publishes_ready() {
        let owner = AiRuntimeInitOwner::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_worker = Arc::clone(&calls);
        assert!(owner.start_with(
            "ai-init-owner-ready-test",
            move || {
                calls_worker.fetch_add(1, Ordering::SeqCst);
                Ok(fake_runtime())
            },
            || {},
        ));
        assert!(!owner.start_with("ai-init-owner-second-test", || Ok(fake_runtime()), || {},));
        assert!(matches!(
            owner.wait_terminal_while(|| true),
            AiRuntimeInitWait::Ready(_)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn init_owner_worker_error_and_panic_are_failed_terminal() {
        for panic_worker in [false, true] {
            let owner = AiRuntimeInitOwner::new();
            owner.start_with(
                "ai-init-owner-failed-test",
                move || {
                    if panic_worker {
                        panic!("injected init panic");
                    }
                    Err(AiError::Ort("injected init error".to_owned()))
                },
                || {},
            );
            let first_error = match owner.wait_terminal_while(|| true) {
                AiRuntimeInitWait::Failed(error) => error,
                _ => panic!("failed initializer must publish Failed"),
            };
            let late_error = match owner.wait_terminal_while(|| true) {
                AiRuntimeInitWait::Failed(error) => error,
                _ => panic!("late consumer must observe the same Failed terminal"),
            };
            assert!(Arc::ptr_eq(&first_error, &late_error));
            assert!(!owner.start_with("ai-init-owner-retry-test", || Ok(fake_runtime()), || {},));
        }
    }

    #[test]
    fn init_owner_spawn_failure_is_failed_terminal_and_wakes_consumers() {
        let owner = AiRuntimeInitOwner::new();
        let waiter_owner = Arc::clone(&owner);
        let (waiting_tx, waiting_rx) = std::sync::mpsc::channel();
        let (wait_done_tx, wait_done_rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let mut announced = false;
            let result = waiter_owner.wait_terminal_while(|| {
                if !announced {
                    waiting_tx.send(()).unwrap();
                    announced = true;
                }
                true
            });
            wait_done_tx.send(result).unwrap();
        });
        waiting_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("consumer must enter the terminal wait before spawn failure");

        let repaints = Arc::new(AtomicUsize::new(0));
        let repaints_callback = Arc::clone(&repaints);
        assert!(owner.start_with_spawner(
            "ai-init-owner-spawn-failure-test",
            || panic!("initializer must be dropped when spawn fails"),
            move || {
                repaints_callback.fetch_add(1, Ordering::SeqCst);
            },
            |_builder, _task| { Err(std::io::Error::other("injected thread spawn failure")) },
        ));
        assert!(matches!(
            wait_done_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("spawn failure must wake the blocked consumer"),
            AiRuntimeInitWait::Failed(_)
        ));
        waiter.join().unwrap();
        assert_eq!(repaints.load(Ordering::SeqCst), 1);
        assert!(!owner.start_with(
            "ai-init-owner-after-spawn-failure",
            || Ok(fake_runtime()),
            || {}
        ));
    }

    #[test]
    fn init_owner_wait_can_cancel_without_changing_owner() {
        let owner = AiRuntimeInitOwner::new();
        let keep_waiting = AtomicBool::new(false);
        assert!(matches!(
            owner.wait_terminal_while(|| keep_waiting.load(Ordering::Acquire)),
            AiRuntimeInitWait::Cancelled
        ));
        assert!(matches!(owner.snapshot(), AiRuntimeInitSnapshot::Dormant));
    }

    #[test]
    fn init_owner_wait_prefers_request_cancel_over_ready_or_failed_terminal() {
        for fail in [false, true] {
            let owner = AiRuntimeInitOwner::new();
            assert!(owner.start_with(
                "ai-init-owner-cancel-terminal-race-test",
                move || {
                    if fail {
                        Err(AiError::Ort("injected terminal failure".to_owned()))
                    } else {
                        Ok(fake_runtime())
                    }
                },
                || {},
            ));
            while matches!(owner.snapshot(), AiRuntimeInitSnapshot::Initializing) {
                std::thread::yield_now();
            }
            assert!(matches!(
                owner.wait_terminal_while(|| false),
                AiRuntimeInitWait::Cancelled
            ));
        }
    }

    #[test]
    fn init_owner_completion_is_safe_after_app_side_owner_is_dropped() {
        let owner = AiRuntimeInitOwner::new();
        let weak = Arc::downgrade(&owner);
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        assert!(owner.start_with(
            "ai-init-owner-drop-test",
            move || {
                release_rx.recv().expect("release init worker");
                Err(AiError::Ort("injected late failure".to_owned()))
            },
            move || {
                let _ = done_tx.send(());
            },
        ));
        drop(owner);
        release_tx.send(()).unwrap();
        done_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("dropped App-side owner must not prevent terminal publication");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while weak.upgrade().is_some() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn dynamic_load_failures_return_without_reentering_once_lock() {
        const CHILD_ENV: &str = "MIV_ORT_LOAD_FAILURE_CHILD";
        if let Some(case) = std::env::var_os(CHILD_ENV) {
            match case.to_string_lossy().as_ref() {
                "missing" => {
                    let missing = std::env::temp_dir().join(format!(
                        "miv-missing-onnxruntime-{}.dll",
                        std::process::id()
                    ));
                    let error = match ort::init_from(&missing) {
                        Ok(_) => panic!("missing DLL must fail"),
                        Err(error) => error,
                    };
                    assert!(matches!(error, ort::LoadDynamicError::Dlopen { .. }));
                }
                #[cfg(windows)]
                "missing-api" => {
                    let system = std::env::var_os("SystemRoot")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
                        .join("System32")
                        .join("kernel32.dll");
                    let error = match ort::init_from(system) {
                        Ok(_) => panic!("non-ORT DLL must fail"),
                        Err(error) => error,
                    };
                    assert!(matches!(error, ort::LoadDynamicError::MissingApi { .. }));
                }
                other => panic!("unknown isolated ORT load failure case: {other}"),
            }
            return;
        }

        let exe = std::env::current_exe().expect("test executable");
        let cases: &[&str] = if cfg!(windows) {
            &["missing", "missing-api"]
        } else {
            &["missing"]
        };
        for case in cases {
            let mut child = std::process::Command::new(&exe)
                .arg("--exact")
                .arg(
                    "ai::runtime::tests::dynamic_load_failures_return_without_reentering_once_lock",
                )
                .arg("--nocapture")
                .env(CHILD_ENV, case)
                .spawn()
                .expect("spawn isolated load failure test");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if let Some(status) = child.try_wait().expect("poll isolated test") {
                    assert!(status.success(), "isolated {case} test failed: {status}");
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("ort dynamic-load {case} test did not return within five seconds");
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }
}
