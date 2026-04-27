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
                let pack_dll = super::tensorrt_pack::pack_ort_dll_path();
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
pub struct AiRuntime {
    /// ModelKind → Session のキャッシュ。
    /// Session::run() は &mut self なので Mutex が必要。
    sessions: Mutex<HashMap<ModelKind, Session>>,
    /// 現プロセスで実際にロードされたバックエンド情報。
    backend: ActiveBackend,
    /// TensorRT FP16 推論を有効化するか (Settings から渡される)。
    tensorrt_fp16: bool,
}

impl AiRuntime {
    /// 新しい AiRuntime を作成する (DirectML バックエンド、互換 API)。
    ///
    /// テスト・ベンチ・既存呼び出し用のショートハンド。
    /// アプリ本体は `new_with_backend(backend, fp16)` を使ってユーザー設定を反映する。
    pub fn new() -> Result<Self, AiError> {
        Self::new_with_backend(AiBackend::DirectMl, true)
    }

    /// 指定バックエンドで新しい AiRuntime を作成する。
    ///
    /// 内部で `ort::init_from` を呼んで onnxruntime.dll を
    /// `%APPDATA%/mimageviewer/` (DirectML) または
    /// `%APPDATA%/mimageviewer/tensorrt/` (TensorRT pack) からロードする。
    /// OnceLock により最初の 1 回のみ実行され、以降は cache を返す。
    /// 異なる backend で 2 回呼ばれても初回の選択が固定される (ort::init_from の制約)。
    pub fn new_with_backend(backend: AiBackend, tensorrt_fp16: bool) -> Result<Self, AiError> {
        let active = ensure_ort_initialized(backend)?;
        Ok(AiRuntime {
            sessions: Mutex::new(HashMap::new()),
            backend: active,
            tensorrt_fp16,
        })
    }

    /// 実際にロードされたバックエンドを返す。UI の表示や log 用。
    /// pack 不在で TRT が DirectML にフォールバックした場合は `effective == DirectMl`。
    #[allow(dead_code)]
    pub fn active_backend(&self) -> &ActiveBackend {
        &self.backend
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

    /// TensorRT + CUDA EP を登録する。Phase 1 ではエンジンキャッシュ設定など
    /// TRT 固有オプションは未配線で、ort クレートのデフォルト挙動に任せる。
    /// Phase 2 で `with_engine_cache_*` / `with_fp16_*` 等を追加する。
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
        // Phase 2 で TensorRT EP の builder option (engine cache path, FP16 等) を
        // 配線する。Phase 1 はデフォルトオプションだけで通す。
        let _ = self.tensorrt_fp16; // Phase 2 で使用
        let trt = ort::ep::TensorRT::default().build();
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
