//! Shared lazy GPU setup for lib and integration UI tests.
//!
//! Keep kittest's CPU-adapter preference and independent per-Harness renderer,
//! but on Windows request only the backends that the product requests.

#[path = "scroll_layout.rs"]
#[allow(dead_code)] // Each integration test uses only the helpers it needs.
pub(crate) mod scroll_layout;

#[cfg(windows)]
#[path = "../../src/wgpu_backend_policy.rs"]
#[allow(dead_code)] // Product argument parsing and candidate ordering are not used by the harness.
mod backend_policy;

pub(crate) fn wgpu_setup() -> egui_wgpu::WgpuSetup {
    #[cfg(windows)]
    {
        let request = std::env::var("MIV_TEST_GPU_BACKEND")
            .ok()
            .map(|value| {
                backend_policy::GpuBackendRequest::parse_value(&value)
                    .expect("MIV_TEST_GPU_BACKEND must be auto, dx12 or vulkan")
            })
            .unwrap_or_default();
        wgpu_setup_for_request(request)
    }
    #[cfg(not(windows))]
    egui_kittest::wgpu::default_wgpu_setup()
}

#[cfg(windows)]
fn wgpu_setup_for_request(request: backend_policy::GpuBackendRequest) -> egui_wgpu::WgpuSetup {
    #[allow(unused_mut)]
    let mut setup = egui_kittest::wgpu::default_wgpu_setup();
    if let egui_wgpu::WgpuSetup::CreateNew(create_new) = &mut setup {
        // Match the product's explicit override, including when WGPU_BACKEND
        // asks for GL. Enumeration initializes even unselected backends.
        create_new.instance_descriptor.backends = request.backends();
        create_new
            .instance_descriptor
            .backend_options
            .dx12
            .shader_compiler = wgpu::Dx12Compiler::Fxc;
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
    // 0.33 captured UI paint only. 0.37 adds a synthetic cursor by default;
    // keep it out of baseline comparisons without changing pointer delivery.
    egui_kittest::HarnessBuilder::default()
        .with_render_cursor(false)
        .renderer(egui_kittest::LazyRenderer::new(|| {
            egui_kittest::wgpu::WgpuTestRenderer::from_setup(wgpu_setup())
        }))
}

/// Fixed-size Context-root fixtures own their panel margins. Draw in the full
/// viewport without allocating into kittest's additional 8pt wrapper: doing so
/// would also expand its painted background. Not for fit-to-contents fixtures.
pub(crate) fn full_viewport_ui<R>(
    ui: &mut egui::Ui,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let viewport = ui.ctx().content_rect();
    let mut root = ui.new_child(egui::UiBuilder::new().max_rect(viewport));
    root.set_clip_rect(viewport);
    add_contents(&mut root)
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn context_root_fixture_panel_matches_raw_root_without_wrapper_margin() {
        use egui_kittest::TestRenderer as _;
        use std::cell::Cell;
        use std::rc::Rc;

        // Both legacy helper forms: a panel spanning the viewport, and a
        // transparent root containing its own explicit 8pt frame margin.
        for inset_frame in [false, true] {
            let size = egui::vec2(160.0, 96.0);
            let pointer = egui::pos2(if inset_frame { 9.0 } else { 1.0 }, 10.0);
            let draw = |ui: &mut egui::Ui| {
                let response = ui.allocate_response(egui::vec2(60.0, 24.0), egui::Sense::click());
                ui.painter()
                    .rect_filled(response.rect, 0.0, egui::Color32::RED);
                (
                    response.rect,
                    response.interact_rect,
                    response.contains_pointer(),
                )
            };
            let fixture = |ui: &mut egui::Ui| {
                if inset_frame {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| {
                            egui::Frame::central_panel(ui.style())
                                .outer_margin(8.0)
                                .inner_margin(0.0)
                                .show(ui, draw)
                                .inner
                        })
                        .inner
                } else {
                    egui::Panel::top("viewport_fixture")
                        .frame(egui::Frame::NONE)
                        .show(ui, draw)
                        .inner
                }
            };
            let actual = Rc::new(Cell::new((egui::Rect::NOTHING, egui::Rect::NOTHING, false)));
            let captured = actual.clone();
            let mut harness = super::builder().with_size(size).build_ui(|ui| {
                captured.set(super::full_viewport_ui(ui, fixture));
            });
            harness.run();
            harness.hover_at(pointer);
            harness.run_steps(3);
            let image = harness.render().expect("fixture GPU render");

            let ctx = egui::Context::default();
            let mut renderer =
                egui_kittest::wgpu::WgpuTestRenderer::from_setup(super::wgpu_setup());
            let mut expected = (egui::Rect::NOTHING, egui::Rect::NOTHING, false);
            let mut expected_image = None;
            for frame in 0..5 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        time: Some(frame as f64 / 60.0),
                        events: vec![egui::Event::PointerMoved(pointer)],
                        ..Default::default()
                    },
                    |ui| expected = fixture(ui),
                );
                renderer.handle_delta(&mut output.textures_delta);
                expected_image = Some(renderer.render(&ctx, &output).expect("raw root GPU render"));
            }
            assert_eq!(actual.get(), expected, "rect, clip and pointer ownership");
            assert!(expected.2, "pointer reaches the actual fixture");
            assert!(
                image == expected_image.unwrap(),
                "all RGBA pixels, including transparent edges and cursor-free capture"
            );
            assert_eq!(image.get_pixel(0, 95).0, [0, 0, 0, 0]);
        }
    }

    #[test]
    fn requests_product_backends_without_initializing_gpu() {
        use super::backend_policy::GpuBackendRequest;
        for request in [
            GpuBackendRequest::Auto,
            GpuBackendRequest::Dx12,
            GpuBackendRequest::Vulkan,
        ] {
            let egui_wgpu::WgpuSetup::CreateNew(setup) = super::wgpu_setup_for_request(request)
            else {
                panic!("UI tests must create independent renderers");
            };
            assert_eq!(setup.instance_descriptor.backends, request.backends());
            assert!(
                !setup
                    .instance_descriptor
                    .backends
                    .contains(wgpu::Backends::GL)
            );
            assert!(matches!(
                setup
                    .instance_descriptor
                    .backend_options
                    .dx12
                    .shader_compiler,
                wgpu::Dx12Compiler::Fxc
            ));
            assert!(
                setup.native_adapter_selector.is_some(),
                "retain kittest CPU preference"
            );
            assert!(
                setup.native_adapter_candidates.is_none(),
                "do not override kittest selector"
            );
        }
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
