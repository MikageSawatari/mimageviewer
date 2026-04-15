//! AI 機能モジュール。
//!
//! ONNX Runtime (ort crate) + DirectML を使い、
//! 画像タイプ分類・アップスケール・Inpainting を提供する。

pub mod classify;
pub mod denoise;
pub mod inpaint;
pub mod model_manager;
pub mod runtime;
pub mod upscale;

use std::fmt;

/// AI 処理で発生しうるエラー。
#[derive(Debug)]
pub enum AiError {
    /// ONNX Runtime エラー
    Ort(String),
    /// モデルファイルが見つからない
    ModelNotFound(ModelKind),
    /// 画像処理エラー
    ImageProcessing(String),
    /// IO エラー
    Io(std::io::Error),
    /// キャンセルされた
    Cancelled,
}

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AiError::Ort(e) => write!(f, "ONNX Runtime error: {e}"),
            AiError::ModelNotFound(k) => write!(f, "Model not found: {k:?}"),
            AiError::ImageProcessing(e) => write!(f, "Image processing error: {e}"),
            AiError::Io(e) => write!(f, "IO error: {e}"),
            AiError::Cancelled => write!(f, "Cancelled"),
        }
    }
}

impl From<std::io::Error> for AiError {
    fn from(e: std::io::Error) -> Self {
        AiError::Io(e)
    }
}

/// 使用可能な AI モデルの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelKind {
    /// 画像タイプ分類（MobileNetV3）
    ClassifierMobileNet,
    /// Real-ESRGAN x4plus（写真/CG 向け）
    UpscaleRealEsrganX4Plus,
    /// Real-ESRGAN Anime 6B（カラーイラスト向け）
    UpscaleRealEsrganAnime6B,
    /// realesr-general-x4v3（汎用）
    UpscaleRealEsrGeneralV3,
    /// Real-CUGAN 4x conservative（漫画・スクリーントーン保持向け）
    UpscaleRealCugan4x,
    /// JPEG ノイズ除去 (RealPLKSR)
    DenoiseRealplksr,
    /// MI-GAN Inpainting
    InpaintMiGan,
}

impl ModelKind {
    /// 設定保存用の文字列表現。
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelKind::ClassifierMobileNet => "classifier_mobilenet",
            ModelKind::UpscaleRealEsrganX4Plus => "realesrgan_x4plus",
            ModelKind::UpscaleRealEsrganAnime6B => "realesrgan_anime6b",
            ModelKind::UpscaleRealEsrGeneralV3 => "realesr_general_v3",
            ModelKind::DenoiseRealplksr => "denoise_realplksr",
            ModelKind::InpaintMiGan => "inpaint_migan",
            ModelKind::UpscaleRealCugan4x => "realcugan_4x",
        }
    }

    /// UI 表示用のラベル。
    pub fn display_label(&self) -> &'static str {
        match self {
            ModelKind::ClassifierMobileNet => "分類器 (MobileNetV3)",
            ModelKind::UpscaleRealEsrganX4Plus => "写真/CG (Real-ESRGAN x4plus)",
            ModelKind::UpscaleRealEsrganAnime6B => "イラスト (Real-ESRGAN Anime)",
            ModelKind::UpscaleRealEsrGeneralV3 => "汎用 (Real-ESRGAN General)",
            ModelKind::DenoiseRealplksr => "JPEG ノイズ除去",
            ModelKind::InpaintMiGan => "補完 (MI-GAN)",
            ModelKind::UpscaleRealCugan4x => "漫画 (Real-CUGAN 4x)",
        }
    }

    /// 設定文字列からモデル種別を復元する。
    pub fn from_str(s: &str) -> Option<ModelKind> {
        match s {
            "classifier_mobilenet" => Some(ModelKind::ClassifierMobileNet),
            "realesrgan_x4plus" => Some(ModelKind::UpscaleRealEsrganX4Plus),
            "realesrgan_anime6b" => Some(ModelKind::UpscaleRealEsrganAnime6B),
            // "waifu2x_cunet" は廃止。旧設定ファイルに存在する場合は無視
            "realesr_general_v3" => Some(ModelKind::UpscaleRealEsrGeneralV3),
            // "inpaint_lama" は旧設定ファイルとの互換用
            "inpaint_migan" | "inpaint_lama" => Some(ModelKind::InpaintMiGan),
            "realcugan_4x" => Some(ModelKind::UpscaleRealCugan4x),
            "denoise_realplksr" => Some(ModelKind::DenoiseRealplksr),
            _ => None,
        }
    }

    /// デノイズ用モデル一覧（UI プルダウンに表示するもの）。
    pub fn denoise_models() -> &'static [ModelKind] {
        &[
            ModelKind::DenoiseRealplksr,
        ]
    }

    /// アップスケール用モデル一覧（UI プルダウンに表示するもの）。
    pub fn upscale_models() -> &'static [ModelKind] {
        &[
            ModelKind::UpscaleRealEsrganX4Plus,
            ModelKind::UpscaleRealEsrganAnime6B,
            ModelKind::UpscaleRealEsrGeneralV3,
            ModelKind::UpscaleRealCugan4x,
        ]
    }
}

/// 画像カテゴリ（分類器の出力）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageCategory {
    /// カラーイラスト
    Illustration,
    /// 漫画（モノクロ）
    Comic,
    /// 3D CG
    ThreeD,
    /// 実写写真
    RealLife,
}

impl ImageCategory {
    /// カテゴリに最適なアップスケールモデルを返す。
    pub fn preferred_upscale_model(&self) -> ModelKind {
        match self {
            ImageCategory::Illustration => ModelKind::UpscaleRealEsrganAnime6B,
            ImageCategory::Comic => ModelKind::UpscaleRealCugan4x,
            ImageCategory::ThreeD => ModelKind::UpscaleRealEsrganX4Plus,
            ImageCategory::RealLife => ModelKind::UpscaleRealEsrganX4Plus,
        }
    }

    /// UI 表示用ラベル。
    pub fn display_label(&self) -> &'static str {
        match self {
            ImageCategory::Illustration => "イラスト",
            ImageCategory::Comic => "漫画",
            ImageCategory::ThreeD => "3D CG",
            ImageCategory::RealLife => "写真",
        }
    }
}
