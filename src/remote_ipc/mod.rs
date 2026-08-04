mod ai_job;
mod collections;
mod container;
mod path_guard;
mod service;
mod thumbnail;
mod video_stream;

#[cfg(windows)]
mod pipe;
pub(crate) mod session;
pub(crate) use service::{RemoteServiceControl, RemoteServiceManager, RemoteServiceStatus};

#[cfg(test)]
pub(crate) use container::resolve_remote_effective_params_for_test;
pub(crate) mod ui;

pub(crate) fn remote_adjustment_values(
    params: &crate::adjustment::AdjustParams,
) -> mimageviewer_ipc::RemoteAdjustmentValues {
    mimageviewer_ipc::RemoteAdjustmentValues {
        brightness: params.brightness,
        contrast: params.contrast,
        gamma: params.gamma,
        saturation: params.saturation,
        temperature: params.temperature,
        black_point: params.black_point,
        white_point: params.white_point,
        midtone: params.midtone,
        auto_mode: params.auto_mode.map(|mode| match mode {
            crate::adjustment::AutoMode::Auto => mimageviewer_ipc::RemoteAutoMode::Auto,
            crate::adjustment::AutoMode::MangaCleanup => {
                mimageviewer_ipc::RemoteAutoMode::MangaCleanup
            }
        }),
        colorize: remote_colorize_params(&params.colorize),
        ai: Some(mimageviewer_ipc::RemoteAiAdjustmentValues {
            upscale_model: params.upscale_model.clone(),
            denoise_model: params.denoise_model.clone(),
        }),
    }
}

pub(crate) fn remote_colorize_params(
    params: &crate::colorize::ColorizeParams,
) -> mimageviewer_ipc::RemoteColorizeParams {
    mimageviewer_ipc::RemoteColorizeParams {
        mode: match params.mode {
            crate::colorize::ColorizeMode::Disabled => {
                mimageviewer_ipc::RemoteColorizeMode::Disabled
            }
            crate::colorize::ColorizeMode::MonochromeOnly => {
                mimageviewer_ipc::RemoteColorizeMode::MonochromeOnly
            }
            crate::colorize::ColorizeMode::AllImages => {
                mimageviewer_ipc::RemoteColorizeMode::AllImages
            }
        },
        mono_tolerance: params.mono_tolerance,
        palette: match params.palette {
            crate::colorize::ColorizePalette::Legacy4Color => {
                mimageviewer_ipc::RemoteColorizePalette::Legacy4Color
            }
            crate::colorize::ColorizePalette::LegacySkin => {
                mimageviewer_ipc::RemoteColorizePalette::LegacySkin
            }
            crate::colorize::ColorizePalette::Custom => {
                mimageviewer_ipc::RemoteColorizePalette::Custom
            }
        },
        control_points: params
            .control_points
            .iter()
            .map(|point| mimageviewer_ipc::RemoteColorizeControlPoint {
                color: point.color,
                strength: point.strength,
            })
            .collect(),
        luminance_weight: params.luminance_weight,
        density_normalization_strength: params.density_normalization_strength,
        tone_method: match params.tone_method {
            crate::colorize::ToneDensityMethod::Off => {
                mimageviewer_ipc::RemoteToneDensityMethod::Off
            }
            crate::colorize::ToneDensityMethod::Fast => {
                mimageviewer_ipc::RemoteToneDensityMethod::Fast
            }
            crate::colorize::ToneDensityMethod::LocalMean => {
                mimageviewer_ipc::RemoteToneDensityMethod::LocalMean
            }
            crate::colorize::ToneDensityMethod::Gaussian => {
                mimageviewer_ipc::RemoteToneDensityMethod::Gaussian
            }
        },
        tone_radius: params.tone_radius,
        tone_strength: params.tone_strength,
    }
}

/// 非公開の AI / filter 値を保った完全パラメータへ、remote 編集値を重ねる。
pub(crate) fn apply_remote_adjustment_values(
    mut params: crate::adjustment::AdjustParams,
    values: &mimageviewer_ipc::RemoteAdjustmentValues,
) -> Result<crate::adjustment::AdjustParams, &'static str> {
    let finite = [
        values.brightness,
        values.contrast,
        values.gamma,
        values.saturation,
        values.temperature,
        values.midtone,
    ]
    .into_iter()
    .all(f32::is_finite);
    if !finite
        || !(-100.0..=100.0).contains(&values.brightness)
        || !(-100.0..=100.0).contains(&values.contrast)
        || !(0.2..=5.0).contains(&values.gamma)
        || !(-100.0..=100.0).contains(&values.saturation)
        || !(-100.0..=100.0).contains(&values.temperature)
        || !(0.1..=10.0).contains(&values.midtone)
        || values.black_point > 254
        || values.white_point < 1
    {
        return Err("画像補正値が範囲外です");
    }
    let colorize = &values.colorize;
    if !(1..=64).contains(&colorize.mono_tolerance)
        || colorize.luminance_weight > 100
        || colorize.density_normalization_strength > 100
        || !colorize.tone_radius.is_finite()
        || !(0.1..=4.0).contains(&colorize.tone_radius)
        || colorize.tone_strength > 100
        || !(2..=10).contains(&colorize.control_points.len())
        || colorize
            .control_points
            .iter()
            .any(|point| !point.strength.is_finite() || !(0.0..=10.0).contains(&point.strength))
    {
        return Err("カラー化設定が範囲外です");
    }
    params.brightness = values.brightness;
    params.contrast = values.contrast;
    params.gamma = values.gamma;
    params.saturation = values.saturation;
    params.temperature = values.temperature;
    params.black_point = values.black_point;
    params.white_point = values.white_point;
    params.midtone = values.midtone;
    params.auto_mode = values.auto_mode.map(|mode| match mode {
        mimageviewer_ipc::RemoteAutoMode::Auto => crate::adjustment::AutoMode::Auto,
        mimageviewer_ipc::RemoteAutoMode::MangaCleanup => crate::adjustment::AutoMode::MangaCleanup,
    });
    params.colorize = crate::colorize::ColorizeParams {
        mode: match colorize.mode {
            mimageviewer_ipc::RemoteColorizeMode::Disabled => {
                crate::colorize::ColorizeMode::Disabled
            }
            mimageviewer_ipc::RemoteColorizeMode::MonochromeOnly => {
                crate::colorize::ColorizeMode::MonochromeOnly
            }
            mimageviewer_ipc::RemoteColorizeMode::AllImages => {
                crate::colorize::ColorizeMode::AllImages
            }
        },
        mono_tolerance: colorize.mono_tolerance,
        palette: match colorize.palette {
            mimageviewer_ipc::RemoteColorizePalette::Legacy4Color => {
                crate::colorize::ColorizePalette::Legacy4Color
            }
            mimageviewer_ipc::RemoteColorizePalette::LegacySkin => {
                crate::colorize::ColorizePalette::LegacySkin
            }
            mimageviewer_ipc::RemoteColorizePalette::Custom => {
                crate::colorize::ColorizePalette::Custom
            }
        },
        control_points: colorize
            .control_points
            .iter()
            .map(|point| crate::colorize::ColorizeControlPoint {
                color: point.color,
                strength: point.strength,
            })
            .collect(),
        luminance_weight: colorize.luminance_weight,
        density_normalization_strength: colorize.density_normalization_strength,
        tone_method: match colorize.tone_method {
            mimageviewer_ipc::RemoteToneDensityMethod::Off => {
                crate::colorize::ToneDensityMethod::Off
            }
            mimageviewer_ipc::RemoteToneDensityMethod::Fast => {
                crate::colorize::ToneDensityMethod::Fast
            }
            mimageviewer_ipc::RemoteToneDensityMethod::LocalMean => {
                crate::colorize::ToneDensityMethod::LocalMean
            }
            mimageviewer_ipc::RemoteToneDensityMethod::Gaussian => {
                crate::colorize::ToneDensityMethod::Gaussian
            }
        },
        tone_radius: colorize.tone_radius,
        tone_strength: colorize.tone_strength,
    };
    if let Some(ai) = values.ai.as_ref() {
        if let Some(key) = ai.upscale_model.as_deref()
            && key != "auto"
            && !crate::ai::ModelKind::from_str(key)
                .is_some_and(|model| crate::ai::ModelKind::upscale_models().contains(&model))
        {
            return Err("AI アップスケールの選択値が不正です");
        }
        if let Some(key) = ai.denoise_model.as_deref()
            && !crate::ai::ModelKind::from_str(key)
                .is_some_and(|model| crate::ai::ModelKind::denoise_models().contains(&model))
        {
            return Err("AI デノイズの選択値が不正です");
        }
        params.upscale_model = ai.upscale_model.clone();
        params.denoise_model = ai.denoise_model.clone();
    }
    Ok(params)
}

#[cfg(test)]
mod adjustment_value_tests {
    use super::*;

    #[test]
    fn remote_colorize_default_matches_core_default() {
        assert_eq!(
            remote_colorize_params(&crate::colorize::ColorizeParams::default()),
            mimageviewer_ipc::RemoteColorizeParams::default()
        );
    }

    #[test]
    fn remote_overlay_applies_colorize_and_preserves_hidden_adjustment_fields() {
        let base = crate::adjustment::AdjustParams {
            upscale_model: Some("auto".to_owned()),
            denoise_model: Some("denoise_realplksr".to_owned()),
            smart_sharpen: 42,
            ..Default::default()
        };
        let mut values = remote_adjustment_values(&base);
        values.brightness = 10.0;
        values.contrast = -5.0;
        values.gamma = 1.2;
        values.saturation = 7.0;
        values.temperature = 3.0;
        values.black_point = 4;
        values.white_point = 247;
        values.midtone = 0.8;
        values.auto_mode = Some(mimageviewer_ipc::RemoteAutoMode::MangaCleanup);
        values.colorize.mode = mimageviewer_ipc::RemoteColorizeMode::AllImages;
        values.colorize.palette = mimageviewer_ipc::RemoteColorizePalette::Custom;
        values.colorize.control_points = vec![
            mimageviewer_ipc::RemoteColorizeControlPoint {
                color: [1, 2, 3],
                strength: 2.5,
            },
            mimageviewer_ipc::RemoteColorizeControlPoint {
                color: [240, 230, 220],
                strength: 0.75,
            },
        ];

        let applied = apply_remote_adjustment_values(base.clone(), &values).unwrap();

        assert_eq!(
            applied.upscale_model,
            values.ai.as_ref().unwrap().upscale_model
        );
        assert_eq!(
            applied.denoise_model,
            values.ai.as_ref().unwrap().denoise_model
        );
        assert_eq!(remote_colorize_params(&applied.colorize), values.colorize);
        assert_eq!(applied.smart_sharpen, base.smart_sharpen);
        assert_eq!(remote_adjustment_values(&applied), values);
    }

    #[test]
    fn remote_immediate_overlay_rejects_non_finite_values() {
        let mut values = remote_adjustment_values(&crate::adjustment::AdjustParams::default());
        values.gamma = f32::NAN;
        assert!(apply_remote_adjustment_values(Default::default(), &values).is_err());
    }

    #[test]
    fn missing_remote_ai_values_preserve_saved_models() {
        let base = crate::adjustment::AdjustParams {
            upscale_model: Some("auto".to_owned()),
            denoise_model: Some("denoise_realplksr".to_owned()),
            ..Default::default()
        };
        let mut values = remote_adjustment_values(&base);
        values.ai = None;
        let applied = apply_remote_adjustment_values(base.clone(), &values).unwrap();
        assert_eq!(applied.upscale_model, base.upscale_model);
        assert_eq!(applied.denoise_model, base.denoise_model);
    }

    #[test]
    fn remote_ai_values_reject_unknown_or_wrong_kind_models() {
        let mut values = remote_adjustment_values(&crate::adjustment::AdjustParams::default());
        values.ai.as_mut().unwrap().upscale_model = Some("denoise_realplksr".to_owned());
        assert!(apply_remote_adjustment_values(Default::default(), &values).is_err());

        values.ai.as_mut().unwrap().upscale_model = Some("realcugan_4x".to_owned());
        values.ai.as_mut().unwrap().denoise_model = Some("realcugan_4x".to_owned());
        assert!(apply_remote_adjustment_values(Default::default(), &values).is_err());
    }

    #[test]
    fn remote_colorize_overlay_rejects_invalid_custom_control_points() {
        let mut values = remote_adjustment_values(&crate::adjustment::AdjustParams::default());
        values.colorize.control_points.truncate(1);
        assert!(apply_remote_adjustment_values(Default::default(), &values).is_err());

        let mut values = remote_adjustment_values(&crate::adjustment::AdjustParams::default());
        values.colorize.control_points[0].strength = f32::INFINITY;
        assert!(apply_remote_adjustment_values(Default::default(), &values).is_err());
    }
}

pub(crate) struct RemoteIpcServer {
    #[cfg(windows)]
    _guard: pipe::ServerGuard,
}

impl RemoteIpcServer {
    pub(crate) fn start(settings: crate::settings::Settings) -> Result<Self, String> {
        #[cfg(windows)]
        {
            return pipe::ServerGuard::start(settings).map(|guard| Self { _guard: guard });
        }
        #[cfg(not(windows))]
        {
            let _ = settings;
            Err("リモート接続は Windows の名前付きパイプ専用です".to_owned())
        }
    }

    pub(crate) fn session_handle(&self) -> session::SessionHandle {
        #[cfg(windows)]
        {
            self._guard.session_handle()
        }
        #[cfg(not(windows))]
        {
            unreachable!("remote IPC server is Windows-only")
        }
    }
}
