//! AI モデルのパス管理。
//!
//! モデルは exe に `include_bytes!` で埋め込まれ、
//! 初回起動時に `%APPDATA%/mimageviewer/models/` に展開される。

use std::path::PathBuf;
use std::sync::OnceLock;

use super::ModelKind;

/// 埋め込みモデルの定義。
struct EmbeddedModel {
    kind: ModelKind,
    filename: &'static str,
    bytes: &'static [u8],
}

/// exe に埋め込まれた全モデル。
static EMBEDDED_MODELS: &[EmbeddedModel] = &[
    EmbeddedModel {
        kind: ModelKind::ClassifierMobileNet,
        filename: "anime_classifier_mobilenetv3.onnx",
        bytes: include_bytes!("../../vendor/models/anime_classifier_mobilenetv3.onnx"),
    },
    EmbeddedModel {
        kind: ModelKind::UpscaleRealEsrganX4Plus,
        filename: "realesrgan_x4plus.onnx",
        bytes: include_bytes!("../../vendor/models/realesrgan_x4plus.onnx"),
    },
    EmbeddedModel {
        kind: ModelKind::UpscaleRealEsrganAnime6B,
        filename: "realesrgan_x4plus_anime_6b.onnx",
        bytes: include_bytes!("../../vendor/models/realesrgan_x4plus_anime_6b.onnx"),
    },
    EmbeddedModel {
        kind: ModelKind::UpscaleRealEsrGeneralV3,
        filename: "realesr_general_x4v3.onnx",
        bytes: include_bytes!("../../vendor/models/realesr_general_x4v3.onnx"),
    },
    EmbeddedModel {
        kind: ModelKind::UpscaleRealCugan4x,
        filename: "realcugan_4x_conservative.onnx",
        bytes: include_bytes!("../../vendor/models/realcugan_4x_conservative.onnx"),
    },
    EmbeddedModel {
        kind: ModelKind::DenoiseRealplksr,
        filename: "dejpg_realplksr_otf.onnx",
        bytes: include_bytes!("../../vendor/models/dejpg_realplksr_otf.onnx"),
    },
    EmbeddedModel {
        kind: ModelKind::InpaintMiGan,
        filename: "migan.onnx",
        bytes: include_bytes!("../../vendor/models/migan.onnx"),
    },
];

/// 展開済みモデルディレクトリのキャッシュ。
static MODELS_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 全モデルを `%APPDATA%\mimageviewer\models\` に展開する。
/// サイズが一致すれば展開をスキップする（PDFium DLL と同じパターン）。
pub fn ensure_models_extracted() {
    let dir = models_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        crate::logger::log(format!("[AI] Failed to create models dir: {e}"));
        return;
    }

    for model in EMBEDDED_MODELS {
        let path = dir.join(model.filename);
        let needs_extract = match std::fs::metadata(&path) {
            Ok(meta) => meta.len() != model.bytes.len() as u64,
            Err(_) => true,
        };
        if needs_extract {
            match std::fs::write(&path, model.bytes) {
                Ok(()) => {
                    crate::logger::log(format!(
                        "[AI] Model extracted: {} ({} bytes)",
                        model.filename,
                        model.bytes.len(),
                    ));
                }
                Err(e) => {
                    crate::logger::log(format!(
                        "[AI] Failed to extract {}: {e}",
                        model.filename,
                    ));
                }
            }
        }
    }
}

/// モデル展開先ディレクトリを返す。
fn models_dir() -> PathBuf {
    MODELS_DIR
        .get_or_init(|| crate::data_dir::get().join("models"))
        .clone()
}

/// モデル管理マネージャ。
pub struct ModelManager {
    models_dir: PathBuf,
}

impl ModelManager {
    /// 新しい ModelManager を作成する。
    pub fn new() -> Self {
        ModelManager {
            models_dir: models_dir(),
        }
    }

    /// モデルファイルのパスを返す。
    /// モデルは exe に同梱されているため常に存在する。
    pub fn model_path(&self, kind: ModelKind) -> Option<PathBuf> {
        let filename = Self::model_filename(kind)?;
        let path = self.models_dir.join(filename);
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }

    /// ModelKind に対応するファイル名を返す。
    fn model_filename(kind: ModelKind) -> Option<&'static str> {
        EMBEDDED_MODELS.iter()
            .find(|m| m.kind == kind)
            .map(|m| m.filename)
    }
}
