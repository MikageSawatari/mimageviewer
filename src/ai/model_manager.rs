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
    // portable ビルドでは埋め込まず、exe 隣の `models/` から loose 読みするため bytes を持たない。
    #[cfg(not(feature = "portable"))]
    bytes: &'static [u8],
}

/// exe に埋め込まれた全モデル。
static EMBEDDED_MODELS: &[EmbeddedModel] = &[
    EmbeddedModel {
        kind: ModelKind::UpscaleRealEsrganX4Plus,
        filename: "realesrgan_x4plus.onnx",
        #[cfg(not(feature = "portable"))]
        bytes: include_bytes!("../../vendor/models/realesrgan_x4plus.onnx"),
    },
    EmbeddedModel {
        kind: ModelKind::UpscaleRealEsrganAnime6B,
        filename: "realesrgan_x4plus_anime_6b.onnx",
        #[cfg(not(feature = "portable"))]
        bytes: include_bytes!("../../vendor/models/realesrgan_x4plus_anime_6b.onnx"),
    },
    EmbeddedModel {
        kind: ModelKind::UpscaleRealEsrGeneralV3,
        filename: "realesr_general_x4v3.onnx",
        #[cfg(not(feature = "portable"))]
        bytes: include_bytes!("../../vendor/models/realesr_general_x4v3.onnx"),
    },
    EmbeddedModel {
        kind: ModelKind::UpscaleRealCugan4x,
        filename: "realcugan_4x_conservative.onnx",
        #[cfg(not(feature = "portable"))]
        bytes: include_bytes!("../../vendor/models/realcugan_4x_conservative.onnx"),
    },
    EmbeddedModel {
        kind: ModelKind::UpscaleNmkdSiax4x,
        filename: "4x_NMKD-Siax_200k.onnx",
        #[cfg(not(feature = "portable"))]
        bytes: include_bytes!("../../vendor/models/4x_NMKD-Siax_200k.onnx"),
    },
    EmbeddedModel {
        kind: ModelKind::DenoiseRealplksr,
        filename: "dejpg_realplksr_otf.onnx",
        #[cfg(not(feature = "portable"))]
        bytes: include_bytes!("../../vendor/models/dejpg_realplksr_otf.onnx"),
    },
    EmbeddedModel {
        kind: ModelKind::InpaintMiGan,
        filename: "migan.onnx",
        #[cfg(not(feature = "portable"))]
        bytes: include_bytes!("../../vendor/models/migan.onnx"),
    },
    // 被写体マット (ModelKind::SubjectMatte = BiRefNet) は本体に埋め込まず、
    // 編集用追加パック (editing_addon) からダウンロードして供給する。
    // u2netp.onnx の埋め込みは v1.1.0 開発中に廃止 (spec §9: pack 導入時のみ有効化)。
];

/// 展開済みモデルディレクトリのキャッシュ。
static MODELS_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 全モデルを `%APPDATA%\mimageviewer\models\` に展開する。
/// サイズが一致すれば展開をスキップする（PDFium DLL と同じパターン）。
///
/// portable ビルドではモデルを埋め込まず exe 隣の `models/` に loose 同梱するため、
/// body 全体が `#[cfg(not(feature = "portable"))]` で消え no-op になる (呼び出し側 main.rs は
/// 無条件に呼ぶので関数自体は両ビルドで存在させる)。
pub fn ensure_models_extracted() {
    #[cfg(not(feature = "portable"))]
    {
        let dir = models_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            crate::logger::log(format!("[AI] Failed to create models dir: {e}"));
            return;
        }

        for model in EMBEDDED_MODELS {
            let path = dir.join(model.filename);
            // 防御: 埋め込みバイト列が空 (vendor/models/ が未セットアップの worktree でビルドされた場合)
            // は既存の実体ファイルを 0 バイトで上書きしないようにスキップする
            if model.bytes.is_empty() {
                continue;
            }
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
}

/// モデルディレクトリを返す。
/// 通常: `<data_dir>/models` (= 展開先)。portable: `<exe_dir>/models` (= loose 同梱先)。
fn models_dir() -> PathBuf {
    MODELS_DIR
        .get_or_init(|| {
            #[cfg(feature = "portable")]
            {
                crate::native_assets::bundled_root().join("models")
            }
            #[cfg(not(feature = "portable"))]
            {
                crate::data_dir::get().join("models")
            }
        })
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
        if path.exists() { Some(path) } else { None }
    }

    /// ModelKind に対応するファイル名を返す。
    fn model_filename(kind: ModelKind) -> Option<&'static str> {
        EMBEDDED_MODELS
            .iter()
            .find(|m| m.kind == kind)
            .map(|m| m.filename)
    }
}
