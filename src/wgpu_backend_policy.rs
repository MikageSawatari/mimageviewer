//! Root GUI backend request and its bounded, surface-compatible adapter order.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GpuBackendRequest {
    #[default]
    Auto,
    Dx12,
    Vulkan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GpuBackendParseError {
    InvalidBackend,
    #[cfg(feature = "test-script")]
    InvalidFailureInjection,
}

impl std::fmt::Display for GpuBackendParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidBackend => "--gpu-backend には auto / dx12 / vulkan を指定してください",
            #[cfg(feature = "test-script")]
            Self::InvalidFailureInjection => "--gpu-test-fail-device には dx12 を指定してください",
        })
    }
}

impl std::error::Error for GpuBackendParseError {}

impl GpuBackendRequest {
    pub(crate) fn parse_value(value: &str) -> Result<Self, GpuBackendParseError> {
        match value.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "dx12" => Ok(Self::Dx12),
            "vulkan" => Ok(Self::Vulkan),
            _ => Err(GpuBackendParseError::InvalidBackend),
        }
    }

    /// Parse startup options only; arguments after `--` are paths.
    pub(crate) fn parse_args(args: &[String]) -> Result<Self, GpuBackendParseError> {
        let mut request = Self::Auto;
        visit_option_values(args, |flag, value| {
            if flag == "--gpu-backend" {
                request = Self::parse_value(value.ok_or(GpuBackendParseError::InvalidBackend)?)?;
            }
            Ok(())
        })?;
        Ok(request)
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Dx12 => "dx12",
            Self::Vulkan => "vulkan",
        }
    }

    pub(crate) fn backends(self) -> wgpu::Backends {
        match self {
            Self::Auto => wgpu::Backends::DX12 | wgpu::Backends::VULKAN,
            Self::Dx12 => wgpu::Backends::DX12,
            Self::Vulkan => wgpu::Backends::VULKAN,
        }
    }

    /// Keep only the first supported adapter per API, independent of enumeration order.
    pub(crate) fn candidate_indices(self, adapters: &[(wgpu::Backend, bool)]) -> Vec<usize> {
        let order: &[wgpu::Backend] = match self {
            Self::Auto => &[wgpu::Backend::Dx12, wgpu::Backend::Vulkan],
            Self::Dx12 => &[wgpu::Backend::Dx12],
            Self::Vulkan => &[wgpu::Backend::Vulkan],
        };
        order
            .iter()
            .filter_map(|backend| {
                adapters
                    .iter()
                    .position(|(api, supported)| api == backend && *supported)
            })
            .collect()
    }

    pub(crate) fn native_adapter_candidates(
        self,
        adapters: &[wgpu::Adapter],
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Vec<wgpu::Adapter> {
        let supported: Vec<_> = adapters
            .iter()
            .map(|adapter| {
                (
                    adapter.get_info().backend,
                    surface.is_none_or(|surface| adapter.is_surface_supported(surface)),
                )
            })
            .collect();
        self.candidate_indices(&supported)
            .into_iter()
            .map(|index| adapters[index].clone())
            .collect()
    }
}

/// Visit actual options, consuming values so a value resembling a flag is not parsed twice.
fn visit_option_values(
    args: &[String],
    mut visit: impl FnMut(&str, Option<&str>) -> Result<(), GpuBackendParseError>,
) -> Result<(), GpuBackendParseError> {
    let mut args = args.iter().peekable();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        let takes_value = matches!(
            arg.as_str(),
            "--gpu-backend"
                | "--gpu-test-fail-device"
                | "--data-dir"
                | "--test-script"
                | "--test-evidence-dir"
                | "--settings-override"
                | "--window-size"
                | "--perf-log-path"
                | "--play-test"
                | "--play-duration"
                | "--play-test-start"
                | "--dcomp-presenter-test"
                | "--dcomp-duration"
                | "--dcomp-window-size"
                | "--dcomp-sync-interval"
                | "--dcomp-start"
                | "--whatsnew-from"
        ) || (arg == "--perf-log"
            && args.peek().is_some_and(|next| !next.starts_with("--")));
        let value = if takes_value {
            args.next().map(String::as_str)
        } else {
            None
        };
        visit(arg, value)?;
    }
    Ok(())
}

#[cfg(feature = "test-script")]
pub(crate) fn parse_fail_device_args(args: &[String]) -> Result<bool, GpuBackendParseError> {
    let mut fail_dx12 = false;
    visit_option_values(args, |flag, value| {
        if flag == "--gpu-test-fail-device" {
            if !value.is_some_and(|value| value.eq_ignore_ascii_case("dx12")) {
                return Err(GpuBackendParseError::InvalidFailureInjection);
            }
            fail_dx12 = true;
        }
        Ok(())
    })?;
    Ok(fail_dx12)
}

/// Exercise real wgpu limits validation, without opening a deliberately broken HAL device.
#[cfg(feature = "test-script")]
pub(crate) fn inject_device_failure(
    enabled: bool,
    adapter: &wgpu::AdapterInfo,
    descriptor: &mut wgpu::DeviceDescriptor<'_>,
) {
    if enabled && adapter.backend == wgpu::Backend::Dx12 {
        descriptor.required_limits.max_texture_dimension_2d = u32::MAX;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn parses_backend_values_and_default() {
        assert_eq!(
            GpuBackendRequest::parse_args(&[]),
            Ok(GpuBackendRequest::Auto)
        );
        for (value, expected) in [
            ("auto", GpuBackendRequest::Auto),
            ("DX12", GpuBackendRequest::Dx12),
            ("VuLkAn", GpuBackendRequest::Vulkan),
        ] {
            assert_eq!(
                GpuBackendRequest::parse_args(&args(&["--gpu-backend", value, r"C:\images"])),
                Ok(expected)
            );
            assert_eq!(
                GpuBackendRequest::parse_value(expected.as_str()),
                Ok(expected)
            );
        }
    }

    #[test]
    fn rejects_invalid_and_missing_values_without_silent_auto() {
        for values in [
            vec!["--gpu-backend"],
            vec!["--gpu-backend", "opengl"],
            vec!["--gpu-backend", ""],
            vec!["--gpu-backend", "--"],
            vec!["--gpu-backend", "--window-size", "800x600"],
        ] {
            assert_eq!(
                GpuBackendRequest::parse_args(&args(&values)),
                Err(GpuBackendParseError::InvalidBackend)
            );
        }
        assert_eq!(
            GpuBackendRequest::parse_args(&args(&["--", "--gpu-backend", "opengl"])),
            Ok(GpuBackendRequest::Auto)
        );
    }

    #[test]
    fn flag_shaped_values_and_delimited_paths_are_not_backend_options() {
        for values in [
            vec!["--data-dir", "--gpu-backend", "vulkan"],
            vec!["--test-script", "--gpu-backend", "vulkan"],
            vec!["--gpu-test-fail-device", "--gpu-backend", "vulkan"],
            vec!["--", "--gpu-backend", "vulkan"],
        ] {
            assert_eq!(
                GpuBackendRequest::parse_args(&args(&values)),
                Ok(GpuBackendRequest::Auto)
            );
        }
    }

    #[cfg(feature = "test-script")]
    #[test]
    fn failure_injection_is_explicit_dx12_only_and_respects_other_values() {
        assert_eq!(parse_fail_device_args(&[]), Ok(false));
        assert_eq!(
            parse_fail_device_args(&args(&["--gpu-test-fail-device", "DX12"])),
            Ok(true)
        );
        for values in [
            vec!["--gpu-test-fail-device"],
            vec!["--gpu-test-fail-device", "vulkan"],
            vec!["--gpu-test-fail-device", "--"],
        ] {
            assert_eq!(
                parse_fail_device_args(&args(&values)),
                Err(GpuBackendParseError::InvalidFailureInjection)
            );
        }
        for values in [
            vec!["--", "--gpu-test-fail-device", "dx12"],
            vec!["--data-dir", "--gpu-test-fail-device", "dx12"],
        ] {
            assert_eq!(parse_fail_device_args(&args(&values)), Ok(false));
        }
        assert!(
            GpuBackendParseError::InvalidFailureInjection
                .to_string()
                .contains("--gpu-test-fail-device")
        );
    }

    #[cfg(feature = "test-script")]
    #[test]
    fn injection_changes_only_the_dx12_requested_limits() {
        for (enabled, backend, injected) in [
            (true, wgpu::Backend::Dx12, true),
            (false, wgpu::Backend::Dx12, false),
            (true, wgpu::Backend::Vulkan, false),
        ] {
            let adapter = wgpu::AdapterInfo::new(wgpu::DeviceType::Other, backend);
            let mut descriptor = wgpu::DeviceDescriptor::default();
            let original = descriptor.required_limits.max_texture_dimension_2d;
            inject_device_failure(enabled, &adapter, &mut descriptor);
            assert_eq!(
                descriptor.required_limits.max_texture_dimension_2d,
                if injected { u32::MAX } else { original }
            );
        }
    }

    #[test]
    fn request_constrains_instance_backends() {
        assert_eq!(
            GpuBackendRequest::Auto.backends(),
            wgpu::Backends::DX12 | wgpu::Backends::VULKAN
        );
        assert_eq!(GpuBackendRequest::Dx12.backends(), wgpu::Backends::DX12);
        assert_eq!(GpuBackendRequest::Vulkan.backends(), wgpu::Backends::VULKAN);
    }

    #[test]
    fn candidates_keep_first_supported_per_api_in_requested_order() {
        use wgpu::Backend::{Dx12, Gl, Vulkan};
        let adapters = [
            (Vulkan, true),
            (Dx12, false),
            (Gl, true),
            (Dx12, true),
            (Dx12, true),
            (Vulkan, true),
        ];
        assert_eq!(GpuBackendRequest::Auto.candidate_indices(&adapters), [3, 0]);
        assert_eq!(GpuBackendRequest::Dx12.candidate_indices(&adapters), [3]);
        assert_eq!(GpuBackendRequest::Vulkan.candidate_indices(&adapters), [0]);
    }

    #[test]
    fn absent_and_surface_incompatible_apis_are_not_candidates() {
        use wgpu::Backend::{Dx12, Vulkan};
        assert_eq!(
            GpuBackendRequest::Auto.candidate_indices(&[(Vulkan, true)]),
            [0]
        );
        assert_eq!(
            GpuBackendRequest::Auto.candidate_indices(&[(Dx12, true)]),
            [0]
        );
        assert!(
            GpuBackendRequest::Dx12
                .candidate_indices(&[(Vulkan, true)])
                .is_empty()
        );
        assert!(
            GpuBackendRequest::Vulkan
                .candidate_indices(&[(Dx12, true)])
                .is_empty()
        );
        assert!(
            GpuBackendRequest::Auto
                .candidate_indices(&[(Dx12, false), (Vulkan, false)])
                .is_empty()
        );
        assert!(GpuBackendRequest::Auto.candidate_indices(&[]).is_empty());
    }
}
