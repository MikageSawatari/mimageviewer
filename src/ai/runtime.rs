//! ONNX Runtime 初期化・セッション管理。
//!
//! DirectML EP を使い、GPU アクセラレーションで推論する。
//! セッションは ModelKind ごとに遅延作成・キャッシュする。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use ort::session::Session;

use super::{AiError, ModelKind};

/// ONNX Runtime ラッパー。
/// アプリ全体で 1 つだけ作成し、`Arc<AiRuntime>` で共有する。
pub struct AiRuntime {
    /// ModelKind → Session のキャッシュ。
    /// Session::run() は &mut self なので Mutex が必要。
    sessions: Mutex<HashMap<ModelKind, Session>>,
}

impl AiRuntime {
    /// 新しい AiRuntime を作成する。
    pub fn new() -> Result<Self, AiError> {
        Ok(AiRuntime {
            sessions: Mutex::new(HashMap::new()),
        })
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

    fn load_model_inner(&self, kind: ModelKind, model_path: &Path, force_cpu: bool) -> Result<(), AiError> {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.contains_key(&kind) {
            return Ok(());
        }

        crate::logger::log(format!(
            "[AI] Loading model {:?} from {} ({})",
            kind,
            model_path.display(),
            if force_cpu { "CPU" } else { "DirectML" }
        ));

        let mut builder = Session::builder()
            .map_err(|e| AiError::Ort(format!("Session::builder: {e}")))?;

        builder = builder
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
            .map_err(|e| AiError::Ort(format!("optimization_level: {e}")))?;

        builder = builder
            .with_intra_threads(4)
            .map_err(|e| AiError::Ort(format!("intra_threads: {e}")))?;

        if !force_cpu {
            // DirectML EP を登録（失敗時は CPU フォールバック）
            builder = match builder
                .with_execution_providers([ort::ep::DirectML::default().build()])
            {
                Ok(b) => b,
                Err(e) => {
                    crate::logger::log(format!(
                        "[AI] DirectML EP registration failed, falling back to CPU: {}",
                        e
                    ));
                    e.recover()
                }
            };
        }

        let session = builder
            .commit_from_file(model_path)
            .map_err(|e| AiError::Ort(format!("Failed to load {}: {e}", model_path.display())))?;

        crate::logger::log(format!("[AI] Model {:?} loaded successfully", kind));
        sessions.insert(kind, session);
        Ok(())
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
