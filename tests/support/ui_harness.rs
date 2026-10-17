//! Shared lazy GPU setup for lib and integration UI tests.
//!
//! Keep kittest's CPU-adapter preference and independent per-Harness renderer,
//! but on Windows request only the backends that the product requests.

#[path = "scroll_layout.rs"]
#[allow(dead_code)] // Each integration test uses only the helpers it needs.
pub(crate) mod scroll_layout;

#[cfg(windows)]
#[path = "../../src/wgpu_backend_policy.rs"]
mod backend_policy;

pub(crate) fn wgpu_setup() -> egui_wgpu::WgpuSetup {
    #[allow(unused_mut)]
    let mut setup = egui_kittest::wgpu::default_wgpu_setup();
    #[cfg(windows)]
    if let egui_wgpu::WgpuSetup::CreateNew(create_new) = &mut setup {
        // Match the product's explicit override, including when WGPU_BACKEND
        // asks for GL. Enumeration initializes even unselected backends.
        create_new.instance_descriptor.backends = backend_policy::native_backends();
    }
    setup
}

/// Retain paint/platform observations from a fixture with no renderer.
/// Texture upload/submission tests must consume their deltas instead of using this helper.
pub(crate) fn headless_paint_output(mut output: egui::FullOutput) -> egui::FullOutput {
    egui::FullOutput {
        textures_delta: std::mem::take(&mut output.textures_delta),
        ..Default::default()
    }
    .drop_without_applying_deltas();
    output
}

pub(crate) fn builder<State>() -> egui_kittest::HarnessBuilder<State> {
    egui_kittest::HarnessBuilder::default().renderer(egui_kittest::LazyRenderer::new(|| {
        egui_kittest::wgpu::WgpuTestRenderer::from_setup(wgpu_setup())
    }))
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn requests_product_backends_without_initializing_gpu() {
        let egui_wgpu::WgpuSetup::CreateNew(setup) = super::wgpu_setup() else {
            panic!("UI tests must create independent renderers");
        };
        assert_eq!(
            setup.instance_descriptor.backends,
            super::backend_policy::native_backends()
        );
        assert!(
            !setup
                .instance_descriptor
                .backends
                .contains(wgpu::Backends::GL)
        );
        assert!(setup.native_adapter_selector.is_some());
    }

    #[test]
    fn render_state_enumerates_only_product_backends() {
        let state = egui_kittest::wgpu::create_render_state(
            super::wgpu_setup(),
            egui_wgpu::RendererOptions::PREDICTABLE,
        );
        eprintln!("UI test adapter: {:?}", state.adapter.get_info());
        assert!(!state.available_adapters.is_empty());
        for adapter in &state.available_adapters {
            assert!(matches!(
                adapter.get_info().backend,
                wgpu::Backend::Dx12 | wgpu::Backend::Vulkan
            ));
        }
    }
}
