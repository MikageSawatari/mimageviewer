mod collections;
mod container;
mod path_guard;
mod thumbnail;
mod video_stream;

#[cfg(windows)]
mod pipe;
pub(crate) mod session;

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
    }
}

/// 非公開の AI / filter 値を保った完全パラメータへ、remote 即時値だけを重ねる。
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
    Ok(params)
}

#[cfg(test)]
mod adjustment_value_tests {
    use super::*;

    #[test]
    fn remote_immediate_overlay_preserves_hidden_adjustment_fields() {
        let mut base = crate::adjustment::AdjustParams {
            upscale_model: Some("auto".to_owned()),
            denoise_model: Some("denoise_realplksr".to_owned()),
            smart_sharpen: 42,
            ..Default::default()
        };
        base.colorize.mode = crate::colorize::ColorizeMode::AllImages;
        let values = mimageviewer_ipc::RemoteAdjustmentValues {
            brightness: 10.0,
            contrast: -5.0,
            gamma: 1.2,
            saturation: 7.0,
            temperature: 3.0,
            black_point: 4,
            white_point: 247,
            midtone: 0.8,
            auto_mode: Some(mimageviewer_ipc::RemoteAutoMode::MangaCleanup),
        };

        let applied = apply_remote_adjustment_values(base.clone(), &values).unwrap();

        assert_eq!(applied.upscale_model, base.upscale_model);
        assert_eq!(applied.denoise_model, base.denoise_model);
        assert_eq!(applied.colorize, base.colorize);
        assert_eq!(applied.smart_sharpen, base.smart_sharpen);
        assert_eq!(remote_adjustment_values(&applied), values);
    }

    #[test]
    fn remote_immediate_overlay_rejects_non_finite_values() {
        let mut values = remote_adjustment_values(&crate::adjustment::AdjustParams::default());
        values.gamma = f32::NAN;
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
            Err("--remote-ipc は Windows の名前付きパイプ専用です".to_owned())
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
