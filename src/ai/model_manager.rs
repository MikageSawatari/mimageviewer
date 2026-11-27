//! AI モデルのパス管理。
//!
//! モデルは exe に `include_bytes!` で埋め込まれ、
//! 初回起動時に `%APPDATA%/mimageviewer/models/` に展開される。

use std::path::PathBuf;
use std::sync::OnceLock;

use super::ModelKind;

#[cfg(not(feature = "portable"))]
fn extraction_span(stage: miv_startup::Stage) -> miv_startup::Span {
    miv_startup::span(miv_startup::Lane::Core, stage).watched_optional(miv_startup::watch_handle(
        miv_startup::WatchSlot::CoreStartup,
    ))
}

/// 同梱物の識別子。検出器は TensorRT 等の固定 ModelKind 表に加えない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbeddedAsset {
    Model(ModelKind),
    FaceDetectorYuNet,
}

/// 埋め込みモデルの定義。
struct EmbeddedModel {
    asset: EmbeddedAsset,
    filename: &'static str,
    // portable ビルドでは埋め込まず、exe 隣の `models/` から loose 読みするため bytes を持たない。
    #[cfg(not(feature = "portable"))]
    bytes: &'static [u8],
}

/// exe に埋め込まれた全モデル。
static EMBEDDED_MODELS: &[EmbeddedModel] = &[
    EmbeddedModel {
        asset: EmbeddedAsset::Model(ModelKind::UpscaleRealEsrganX4Plus),
        filename: "realesrgan_x4plus.onnx",
        #[cfg(not(feature = "portable"))]
        bytes: include_bytes!("../../vendor/models/realesrgan_x4plus.onnx"),
    },
    EmbeddedModel {
        asset: EmbeddedAsset::Model(ModelKind::UpscaleRealEsrganAnime6B),
        filename: "realesrgan_x4plus_anime_6b.onnx",
        #[cfg(not(feature = "portable"))]
        bytes: include_bytes!("../../vendor/models/realesrgan_x4plus_anime_6b.onnx"),
    },
    EmbeddedModel {
        asset: EmbeddedAsset::Model(ModelKind::UpscaleRealEsrGeneralV3),
        filename: "realesr_general_x4v3.onnx",
        #[cfg(not(feature = "portable"))]
        bytes: include_bytes!("../../vendor/models/realesr_general_x4v3.onnx"),
    },
    EmbeddedModel {
        asset: EmbeddedAsset::Model(ModelKind::UpscaleRealCugan4x),
        filename: "realcugan_4x_conservative.onnx",
        #[cfg(not(feature = "portable"))]
        bytes: include_bytes!("../../vendor/models/realcugan_4x_conservative.onnx"),
    },
    EmbeddedModel {
        asset: EmbeddedAsset::Model(ModelKind::UpscaleNmkdSiax4x),
        filename: "4x_NMKD-Siax_200k.onnx",
        #[cfg(not(feature = "portable"))]
        bytes: include_bytes!("../../vendor/models/4x_NMKD-Siax_200k.onnx"),
    },
    EmbeddedModel {
        asset: EmbeddedAsset::Model(ModelKind::DenoiseRealplksr),
        filename: "dejpg_realplksr_otf.onnx",
        #[cfg(not(feature = "portable"))]
        bytes: include_bytes!("../../vendor/models/dejpg_realplksr_otf.onnx"),
    },
    EmbeddedModel {
        asset: EmbeddedAsset::Model(ModelKind::InpaintMiGan),
        filename: "migan.onnx",
        #[cfg(not(feature = "portable"))]
        bytes: include_bytes!("../../vendor/models/migan.onnx"),
    },
    EmbeddedModel {
        asset: EmbeddedAsset::FaceDetectorYuNet,
        filename: "face_detection_yunet_2026may.onnx",
        #[cfg(not(feature = "portable"))]
        bytes: include_bytes!("../../vendor/models/face_detection_yunet_2026may.onnx"),
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
        let mkdir = extraction_span(miv_startup::Stage::AssetWrite)
            .detail(&format!("models-directory path={}", dir.display()));
        let mkdir_result = std::fs::create_dir_all(&dir);
        mkdir.finish(if mkdir_result.is_ok() {
            miv_startup::Outcome::Ok
        } else {
            miv_startup::Outcome::Error
        });
        if let Err(e) = mkdir_result {
            crate::logger::log(format!("[AI] Failed to create models dir: {e}"));
            return;
        }

        for model in EMBEDDED_MODELS {
            let path = dir.join(model.filename);
            let model_span = extraction_span(miv_startup::Stage::AiModel).detail(&format!(
                "model={} bytes={} path={}",
                model.filename,
                model.bytes.len(),
                path.display()
            ));
            // 防御: 埋め込みバイト列が空 (vendor/models/ が未セットアップの worktree でビルドされた場合)
            // は既存の実体ファイルを 0 バイトで上書きしないようにスキップする
            if model.bytes.is_empty() {
                model_span.finish(miv_startup::Outcome::Skipped);
                continue;
            }
            let verify = extraction_span(miv_startup::Stage::AssetVerify).detail(&format!(
                "model={} size-compare bytes={} path={}",
                model.filename,
                model.bytes.len(),
                path.display()
            ));
            let metadata = std::fs::metadata(&path);
            let needs_extract = match &metadata {
                Ok(meta) => meta.len() != model.bytes.len() as u64,
                Err(_) => true,
            };
            verify.finish(match &metadata {
                Ok(_) => miv_startup::Outcome::Ok,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    miv_startup::Outcome::Skipped
                }
                Err(_) => miv_startup::Outcome::Error,
            });
            let mut outcome = miv_startup::Outcome::Skipped;
            if needs_extract {
                let write = extraction_span(miv_startup::Stage::AssetWrite).detail(&format!(
                    "model={} bytes={} path={}",
                    model.filename,
                    model.bytes.len(),
                    path.display()
                ));
                let result = std::fs::write(&path, model.bytes);
                outcome = if result.is_ok() {
                    miv_startup::Outcome::Ok
                } else {
                    miv_startup::Outcome::Error
                };
                write.finish(outcome);
                match result {
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
            model_span
                .detail(if needs_extract {
                    "extraction-attempted"
                } else {
                    "reused-size-match"
                })
                .finish(outcome);
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
    /// 深度は追加アドオンを worker 側で検証する。未導入・不正なら None。
    /// その他は既存の同梱モデルを参照する。UI から深度の解決を呼ばないこと。
    pub fn model_path(&self, kind: ModelKind) -> Option<PathBuf> {
        if kind == ModelKind::DepthDa3MonoLarge {
            return crate::depth_addon::model_path();
        }
        let filename = Self::model_filename(kind)?;
        let path = self.models_dir.join(filename);
        if path.exists() { Some(path) } else { None }
    }

    /// 同梱 YuNet のパス。通常版は展開先、portable 版は exe 隣の models。
    /// ファイルの存在を確認するため worker から呼ぶこと。
    pub fn face_detector_yunet_path(&self) -> Option<PathBuf> {
        let filename = Self::asset_filename(EmbeddedAsset::FaceDetectorYuNet)?;
        let path = self.models_dir.join(filename);
        if path.exists() { Some(path) } else { None }
    }

    /// ModelKind に対応するファイル名を返す。
    fn model_filename(kind: ModelKind) -> Option<&'static str> {
        Self::asset_filename(EmbeddedAsset::Model(kind))
    }

    fn asset_filename(asset: EmbeddedAsset) -> Option<&'static str> {
        EMBEDDED_MODELS
            .iter()
            .find(|m| m.asset == asset)
            .map(|m| m.filename)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_asset_yunet_path_and_existing_model_paths() {
        let temp = tempfile::tempdir().unwrap();
        // The manager receives the resolved normal/portable models directory.
        let manager = ModelManager {
            models_dir: temp.path().join("models"),
        };
        std::fs::create_dir_all(&manager.models_dir).unwrap();
        assert!(manager.face_detector_yunet_path().is_none());
        let face = manager.models_dir.join("face_detection_yunet_2026may.onnx");
        std::fs::write(&face, b"fixture").unwrap();
        assert_eq!(manager.face_detector_yunet_path(), Some(face));
        for (kind, name) in [
            (ModelKind::UpscaleRealEsrganX4Plus, "realesrgan_x4plus.onnx"),
            (
                ModelKind::UpscaleRealEsrganAnime6B,
                "realesrgan_x4plus_anime_6b.onnx",
            ),
            (
                ModelKind::UpscaleRealEsrGeneralV3,
                "realesr_general_x4v3.onnx",
            ),
            (
                ModelKind::UpscaleRealCugan4x,
                "realcugan_4x_conservative.onnx",
            ),
            (ModelKind::UpscaleNmkdSiax4x, "4x_NMKD-Siax_200k.onnx"),
            (ModelKind::DenoiseRealplksr, "dejpg_realplksr_otf.onnx"),
            (ModelKind::InpaintMiGan, "migan.onnx"),
        ] {
            let path = manager.models_dir.join(name);
            std::fs::write(&path, b"fixture").unwrap();
            assert_eq!(manager.model_path(kind), Some(path));
        }
        assert_eq!(ModelManager::model_filename(ModelKind::SubjectMatte), None);
    }
}
