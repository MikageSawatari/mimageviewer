#![expect(clippy::missing_errors_doc)]
#![expect(clippy::undocumented_unsafe_blocks)]
#![expect(clippy::unwrap_used)] // TODO(emilk): avoid unwraps
#![expect(unsafe_code)]

use crate::{
    RenderState, RootStartupFrameTag, StartupGpuStage, SurfaceConfig, SurfaceErrorAction,
    WgpuConfiguration, renderer,
};
use crate::{RendererOptions, capture::CaptureState};
use core::num::NonZeroU32;
use egui::{Context, ViewportId, ViewportIdMap, ViewportIdSet};
use std::sync::Arc;

struct SurfaceState {
    surface: wgpu::Surface<'static>,
    alpha_mode: wgpu::CompositeAlphaMode,
    width: u32,
    height: u32,
    resizing: bool,
    needs_reconfigure: bool,
    needs_recreate: bool,
}

/// Everything you need to paint egui with [`wgpu`] on [`winit`].
///
/// Alternatively you can use [`crate::Renderer`] directly.
///
/// NOTE: all egui viewports share the same painter.
pub struct Painter {
    startup_present: crate::startup_diagnostics::RootPresentObserver,
    context: Context,
    config: WgpuConfiguration,
    options: RendererOptions,
    support_transparent_backbuffer: bool,
    screen_capture_state: Option<CaptureState>,

    instance: wgpu::Instance,
    render_state: Option<RenderState>,

    // Per viewport/window:
    depth_texture_view: ViewportIdMap<wgpu::TextureView>,
    msaa_texture_view: ViewportIdMap<wgpu::TextureView>,
    surfaces: ViewportIdMap<SurfaceState>,
}

impl Painter {
    /// Manages [`wgpu`] state, including surface state, required to render egui.
    ///
    /// Only the [`wgpu::Instance`] is initialized here. Device selection and the initialization
    /// of render + surface state is deferred until the painter is given its first window target
    /// via [`set_window()`](Self::set_window). (Ensuring that a device that's compatible with the
    /// native window is chosen)
    ///
    /// Before calling [`paint_and_update_textures()`](Self::paint_and_update_textures) a
    /// [`wgpu::Surface`] must be initialized (and corresponding render state) by calling
    /// [`set_window()`](Self::set_window) once you have
    /// a [`winit::window::Window`] with a valid `.raw_window_handle()`
    /// associated.
    pub async fn new(
        context: Context,
        config: WgpuConfiguration,
        support_transparent_backbuffer: bool,
        options: RendererOptions,
    ) -> Self {
        config.startup_begin(StartupGpuStage::Instance);
        let instance = config.wgpu_setup.new_instance().await;
        config.startup_end(StartupGpuStage::Instance, true);

        Self {
            startup_present: Default::default(),
            context,
            config,
            options,
            support_transparent_backbuffer,
            screen_capture_state: None,

            instance,
            render_state: None,

            depth_texture_view: Default::default(),
            surfaces: Default::default(),
            msaa_texture_view: Default::default(),
        }
    }

    /// Get the [`RenderState`].
    ///
    /// Will return [`None`] if the render state has not been initialized yet.
    pub fn render_state(&self) -> Option<RenderState> {
        self.render_state.clone()
    }

    /// The surface configuration (present mode, frame latency) the surfaces are configured with.
    pub fn surface_config(&self) -> SurfaceConfig {
        self.config.surface
    }

    /// Change the surface configuration (present mode, frame latency) at runtime.
    ///
    /// All surfaces are reconfigured before the next paint.
    ///
    /// This is cheap to call every frame.
    pub fn set_surface_config(&mut self, surface_config: SurfaceConfig) {
        if self.config.surface == surface_config {
            return;
        }

        self.config.surface = surface_config;

        #[expect(clippy::iter_over_hash_type)]
        for surface in self.surfaces.values_mut() {
            surface.needs_reconfigure = true;
        }
    }

    fn configure_surface(
        surface_state: &SurfaceState,
        render_state: &RenderState,
        config: &WgpuConfiguration,
        viewport_id: ViewportId,
        observe_startup: bool,
    ) {
        profiling::function_scope!();

        let SurfaceConfig {
            present_mode,
            desired_maximum_frame_latency,
        } = config.surface;

        // Transaction presentation can hold a drawable during AppKit live resize. Keep the
        // configured low-latency path normally, but use three Metal drawables while resizing.
        #[cfg(all(target_os = "macos", feature = "macos-window-resize-jitter-fix"))]
        let desired_maximum_frame_latency = if surface_state.resizing {
            Some(desired_maximum_frame_latency.unwrap_or(2).max(2))
        } else {
            desired_maximum_frame_latency
        };

        let width = surface_state.width;
        let height = surface_state.height;

        let observe = viewport_id == ViewportId::ROOT && observe_startup;
        if observe {
            config.startup_begin(StartupGpuStage::SurfaceCapabilities);
        }
        let default_config =
            surface_state
                .surface
                .get_default_config(&render_state.adapter, width, height);
        if observe {
            config.startup_end(
                StartupGpuStage::SurfaceCapabilities,
                default_config.is_some(),
            );
        }
        let mut surf_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: render_state.target_format,
            present_mode,
            alpha_mode: surface_state.alpha_mode,
            view_formats: vec![render_state.target_format],
            ..default_config.expect("The surface isn't supported by this adapter")
        };

        if let Some(desired_maximum_frame_latency) = desired_maximum_frame_latency {
            surf_config.desired_maximum_frame_latency = desired_maximum_frame_latency;
        }

        if observe {
            config.startup_begin(StartupGpuStage::SurfaceConfigure);
        }
        surface_state
            .surface
            .configure(&render_state.device, &surf_config);
        if observe {
            config.startup_end(StartupGpuStage::SurfaceConfigure, true);
        }
    }

    /// Drop the existing [`wgpu::Surface`] for `viewport_id` and create a fresh one for the
    /// given window via [`wgpu::Instance::create_surface`], then configure it.
    ///
    /// Used to recover from [`wgpu::CurrentSurfaceTexture::Lost`], where reconfiguring the
    /// existing surface object cannot recover.
    fn recreate_surface(
        &mut self,
        viewport_id: ViewportId,
        window: &Arc<winit::window::Window>,
    ) -> Result<(), crate::WgpuError> {
        profiling::function_scope!();

        let Some(old_state) = self.surfaces.remove(&viewport_id) else {
            return Ok(());
        };

        let surface = self.instance.create_surface(Arc::clone(window))?;
        self.install_surface(
            surface,
            viewport_id,
            old_state.width,
            old_state.height,
            old_state.resizing,
        );
        Ok(())
    }

    /// Updates (or clears) the [`winit::window::Window`] associated with the [`Painter`]
    ///
    /// This creates a [`wgpu::Surface`] for the given Window (as well as initializing render
    /// state if needed) that is used for egui rendering.
    ///
    /// This must be called before trying to render via
    /// [`paint_and_update_textures`](Self::paint_and_update_textures)
    ///
    /// # Portability
    ///
    /// _In particular it's important to note that on Android a it's only possible to create
    /// a window surface between `Resumed` and `Paused` lifecycle events, and Winit will panic on
    /// attempts to query the raw window handle while paused._
    ///
    /// On Android [`set_window`](Self::set_window) should be called with `Some(window)` for each
    /// `Resumed` event and `None` for each `Paused` event. Currently, on all other platforms
    /// [`set_window`](Self::set_window) may be called with `Some(window)` as soon as you have a
    /// valid [`winit::window::Window`].
    ///
    /// # Errors
    /// If the provided wgpu configuration does not match an available device.
    pub async fn set_window(
        &mut self,
        viewport_id: ViewportId,
        window: Option<Arc<winit::window::Window>>,
    ) -> Result<(), crate::WgpuError> {
        profiling::scope!("Painter::set_window"); // profile_function gives bad names for async functions

        if let Some(window) = window {
            let size = window.inner_size();
            if !self.surfaces.contains_key(&viewport_id) {
                let observe = viewport_id == ViewportId::ROOT && self.startup_present.pending();
                if observe {
                    self.config.startup_begin(StartupGpuStage::SurfaceCreate);
                }
                let surface = self.instance.create_surface(window);
                if observe {
                    self.config
                        .startup_end(StartupGpuStage::SurfaceCreate, surface.is_ok());
                }
                let surface = surface?;
                self.add_surface(surface, viewport_id, size).await?;
            }
        } else {
            self.remove_viewport(viewport_id);
        }
        Ok(())
    }

    /// Updates (or clears) the [`winit::window::Window`] associated with the [`Painter`] without taking ownership of the window.
    ///
    /// Like [`set_window`](Self::set_window) except:
    ///
    /// # Safety
    /// The user is responsible for ensuring that the window is alive for as long as it is set.
    pub async unsafe fn set_window_unsafe(
        &mut self,
        viewport_id: ViewportId,
        window: Option<&winit::window::Window>,
    ) -> Result<(), crate::WgpuError> {
        profiling::scope!("Painter::set_window_unsafe"); // profile_function gives bad names for async functions

        if let Some(window) = window {
            let size = window.inner_size();
            if !self.surfaces.contains_key(&viewport_id) {
                let observe = viewport_id == ViewportId::ROOT && self.startup_present.pending();
                if observe {
                    self.config.startup_begin(StartupGpuStage::SurfaceCreate);
                }
                let surface =
                    (|| unsafe {
                        Ok::<_, crate::WgpuError>(self.instance.create_surface_unsafe(
                            wgpu::SurfaceTargetUnsafe::from_window(&window)?,
                        )?)
                    })();
                if observe {
                    self.config
                        .startup_end(StartupGpuStage::SurfaceCreate, surface.is_ok());
                }
                let surface = surface?;
                self.add_surface(surface, viewport_id, size).await?;
            }
        } else {
            self.remove_viewport(viewport_id);
        }
        Ok(())
    }

    fn remove_viewport(&mut self, viewport_id: ViewportId) {
        remove_viewport_resources(
            viewport_id,
            &mut self.surfaces,
            &mut self.depth_texture_view,
            &mut self.msaa_texture_view,
        );
    }

    async fn add_surface(
        &mut self,
        surface: wgpu::Surface<'static>,
        viewport_id: ViewportId,
        size: winit::dpi::PhysicalSize<u32>,
    ) -> Result<(), crate::WgpuError> {
        if self.render_state.is_none() {
            let render_state =
                RenderState::create(&self.config, &self.instance, Some(&surface), self.options)
                    .await?;
            self.render_state = Some(render_state);
        }
        self.install_surface(surface, viewport_id, size.width, size.height, false);
        Ok(())
    }

    /// Inserts a freshly created surface into [`Self::surfaces`] and configures it.
    ///
    /// Render state must already be initialised before calling this.
    // NOTE: The same assumption is already required by `resize_and_generate_depth_texture_view_and_msaa_view`.
    fn install_surface(
        &mut self,
        surface: wgpu::Surface<'static>,
        viewport_id: ViewportId,
        width: u32,
        height: u32,
        resizing: bool,
    ) {
        let alpha_mode = {
            // Panic: We use the same failure mode as `resize_and_generate_depth_texture_view_and_msaa_view`
            let render_state = self
                .render_state
                .as_ref()
                .expect("install_surface called before render_state initialization");
            if self.support_transparent_backbuffer {
                let observe = viewport_id == ViewportId::ROOT && self.startup_present.pending();
                if observe {
                    self.config
                        .startup_begin(StartupGpuStage::SurfaceCapabilities);
                }
                let supported_alpha_modes =
                    surface.get_capabilities(&render_state.adapter).alpha_modes;
                if observe {
                    self.config
                        .startup_end(StartupGpuStage::SurfaceCapabilities, true);
                }
                // Prefer pre multiplied over post multiplied!
                if supported_alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
                    wgpu::CompositeAlphaMode::PreMultiplied
                } else if supported_alpha_modes.contains(&wgpu::CompositeAlphaMode::PostMultiplied)
                {
                    wgpu::CompositeAlphaMode::PostMultiplied
                } else {
                    log::warn!(
                        "Transparent window was requested, but the active wgpu surface does not support a `CompositeAlphaMode` with transparency."
                    );
                    wgpu::CompositeAlphaMode::Auto
                }
            } else {
                wgpu::CompositeAlphaMode::Auto
            }
        };
        self.surfaces.insert(
            viewport_id,
            SurfaceState {
                surface,
                width,
                height,
                alpha_mode,
                resizing,
                needs_reconfigure: false,
                needs_recreate: false,
            },
        );
        let Some(width) = NonZeroU32::new(width) else {
            log::debug!("The window width was zero; skipping generate textures");
            return;
        };
        let Some(height) = NonZeroU32::new(height) else {
            log::debug!("The window height was zero; skipping generate textures");
            return;
        };
        self.resize_and_generate_depth_texture_view_and_msaa_view(viewport_id, width, height);
    }

    /// Returns the maximum texture dimension supported if known
    ///
    /// This API will only return a known dimension after `set_window()` has been called
    /// at least once, since the underlying device and render state are initialized lazily
    /// once we have a window (that may determine the choice of adapter/device).
    pub fn max_texture_side(&self) -> Option<usize> {
        self.render_state
            .as_ref()
            .map(|rs| rs.device.limits().max_texture_dimension_2d as usize)
    }

    fn resize_and_generate_depth_texture_view_and_msaa_view(
        &mut self,
        viewport_id: ViewportId,
        width_in_pixels: NonZeroU32,
        height_in_pixels: NonZeroU32,
    ) {
        profiling::function_scope!();

        let width = width_in_pixels.get();
        let height = height_in_pixels.get();

        let render_state = self.render_state.as_ref().unwrap();
        let surface_state = self.surfaces.get_mut(&viewport_id).unwrap();

        surface_state.width = width;
        surface_state.height = height;

        Self::configure_surface(
            surface_state,
            render_state,
            &self.config,
            viewport_id,
            self.startup_present.pending(),
        );

        if let Some(depth_format) = self.options.depth_stencil_format {
            self.depth_texture_view.insert(
                viewport_id,
                render_state
                    .device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("egui_depth_texture"),
                        size: wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: self.options.msaa_samples.max(1),
                        dimension: wgpu::TextureDimension::D2,
                        format: depth_format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[depth_format],
                    })
                    .create_view(&wgpu::TextureViewDescriptor::default()),
            );
        }

        if let Some(render_state) = (self.options.msaa_samples > 1)
            .then_some(self.render_state.as_ref())
            .flatten()
        {
            let texture_format = render_state.target_format;
            self.msaa_texture_view.insert(
                viewport_id,
                render_state
                    .device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("egui_msaa_texture"),
                        size: wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: self.options.msaa_samples.max(1),
                        dimension: wgpu::TextureDimension::D2,
                        format: texture_format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[texture_format],
                    })
                    .create_view(&wgpu::TextureViewDescriptor::default()),
            );
        }
    }

    /// Handles changes of the resizing state.
    ///
    /// Should be called prior to the first [`Painter::on_window_resized`] call and after the last in
    /// the chain. Used to apply platform-specific logic, e.g. OSX Metal window resize jitter fix.
    pub fn on_window_resize_state_change(&mut self, viewport_id: ViewportId, resizing: bool) {
        profiling::function_scope!();

        let Some(state) = self.surfaces.get_mut(&viewport_id) else {
            return;
        };
        if state.resizing == resizing {
            if resizing {
                log::debug!(
                    "Painter::on_window_resize_state_change() redundant call while resizing"
                );
            } else {
                log::debug!(
                    "Painter::on_window_resize_state_change() redundant call after resizing"
                );
            }
            return;
        }

        // Set before reconfiguring so macOS live resize uses the temporary latency bump above.
        state.resizing = resizing;

        // Resizing is a bit tricky on macOS.
        // It requires enabling ["present_with_transaction"](https://developer.apple.com/documentation/quartzcore/cametallayer/presentswithtransaction)
        // flag to avoid jittering during the resize. Even though resize jittering on macOS
        // is common across rendering backends, the solution for wgpu/metal is known.
        //
        // See https://github.com/emilk/egui/issues/903
        #[cfg(all(target_os = "macos", feature = "macos-window-resize-jitter-fix"))]
        {
            // SAFETY: `as_hal::<Metal>()` returns `None` unless this surface is backed by wgpu's
            // Metal backend.
            unsafe {
                if let (Some(render_state), Some(hal_surface)) = (
                    self.render_state.as_ref(),
                    state.surface.as_hal::<wgpu::hal::api::Metal>(),
                ) {
                    hal_surface
                        .render_layer()
                        .lock()
                        .setPresentsWithTransaction(resizing);

                    Self::configure_surface(
                        state,
                        render_state,
                        &self.config,
                        viewport_id,
                        self.startup_present.pending(),
                    );
                }
            }
        }
    }

    pub fn on_window_resized(
        &mut self,
        viewport_id: ViewportId,
        width_in_pixels: NonZeroU32,
        height_in_pixels: NonZeroU32,
    ) {
        profiling::function_scope!();

        if self.surfaces.contains_key(&viewport_id) {
            self.resize_and_generate_depth_texture_view_and_msaa_view(
                viewport_id,
                width_in_pixels,
                height_in_pixels,
            );
        } else {
            log::warn!(
                "Ignoring window resize notification with no surface created via Painter::set_window()"
            );
        }
    }

    /// Deliver texture updates even when there is no frame to paint.
    ///
    /// If the renderer is not initialized the delta is unchanged: the caller's existing
    /// pending owner must retain it. Otherwise sets are drained, uploads submitted, then frees
    /// drained. This shares the same completion owner as every paint exit.
    pub fn apply_textures_delta(&mut self, textures_delta: &mut epaint::textures::TexturesDelta) {
        let _probe = DeliveryProbe::new(
            "no_paint",
            ViewportId::ROOT,
            false,
            textures_delta,
            if self.render_state.is_some() {
                PaintOutcome::AppliedWithoutPaint
            } else {
                PaintOutcome::RenderStateAbsent
            },
        );
        let _delivery = TextureDelivery::begin(
            self.render_state.as_ref(),
            textures_delta,
            crate::atlas_diag::Site::AppliedNoPaint,
            None,
        );
    }

    /// Returns two things:
    ///
    /// The approximate number of seconds spent on vsync-waiting (if any),
    /// and the captures captured screenshot if it was requested.
    ///
    /// If `capture_data` isn't empty, a screenshot will be captured.
    #[expect(clippy::too_many_arguments)]
    pub fn paint_and_update_textures(
        &mut self,
        viewport_id: ViewportId,
        pixels_per_point: f32,
        clear_color: [f32; 4],
        clipped_primitives: &[epaint::ClippedPrimitive],
        textures_delta: &mut epaint::textures::TexturesDelta,
        capture_data: Vec<egui::ScreenshotCallback>,
        window: &Arc<winit::window::Window>,
    ) -> f32 {
        self.paint_and_update_textures_with_startup_tag(
            viewport_id,
            pixels_per_point,
            clear_color,
            clipped_primitives,
            textures_delta,
            capture_data,
            window,
            None,
        )
    }

    /// Like [`Self::paint_and_update_textures`], with the accepted root pass tag.
    #[expect(clippy::too_many_arguments)]
    pub fn paint_and_update_textures_with_startup_tag(
        &mut self,
        viewport_id: ViewportId,
        pixels_per_point: f32,
        clear_color: [f32; 4],
        clipped_primitives: &[epaint::ClippedPrimitive],
        textures_delta: &mut epaint::textures::TexturesDelta,
        capture_data: Vec<egui::ScreenshotCallback>,
        window: &Arc<winit::window::Window>,
        startup_tag: Option<RootStartupFrameTag>,
    ) -> f32 {
        self.paint_and_update_textures_impl(
            viewport_id,
            pixels_per_point,
            clear_color,
            clipped_primitives,
            textures_delta,
            capture_data,
            Some(window),
            startup_tag,
        )
    }

    #[expect(clippy::too_many_arguments)]
    fn paint_and_update_textures_impl(
        &mut self,
        viewport_id: ViewportId,
        pixels_per_point: f32,
        clear_color: [f32; 4],
        clipped_primitives: &[epaint::ClippedPrimitive],
        textures_delta: &mut epaint::textures::TexturesDelta,
        capture_data: Vec<egui::ScreenshotCallback>,
        window: Option<&Arc<winit::window::Window>>,
        startup_tag: Option<RootStartupFrameTag>,
    ) -> f32 {
        profiling::function_scope!();
        let observe = self.config.startup_diagnostics.is_some()
            && self.startup_present.observes(viewport_id, startup_tag);
        let capture = !capture_data.is_empty();
        if capture && crate::atlas_diag::capture_probe_active() {
            crate::atlas_diag::log_line(format!(
                "[capture-probe] wgpu_capture_begin viewport={viewport_id:?} requests={}",
                capture_data.len()
            ));
        }
        let mut probe = DeliveryProbe::new(
            "paint",
            viewport_id,
            capture,
            textures_delta,
            PaintOutcome::RenderStateAbsent,
        );
        let mut vsync_sec = 0.0;

        // If the previous frame produced `CurrentSurfaceTexture::Lost`, the action match
        // below set `needs_recreate`. Recreate the surface now, before re-borrowing
        // `self.render_state` / `self.surfaces` for the rest of the paint.
        if self
            .surfaces
            .get(&viewport_id)
            .is_some_and(|s| s.needs_recreate)
            && let Some(window) = window
            && let Err(err) = self.recreate_surface(viewport_id, window)
        {
            log::error!("Failed to recreate surface for {viewport_id:?}: {err}");
            probe.outcome = PaintOutcome::SurfaceRecreationFailed;
            // Keep this paint batch's diagnostic site while sharing the non-paint completion owner.
            let _delivery = TextureDelivery::begin(
                self.render_state.as_ref(),
                textures_delta,
                crate::atlas_diag::Site::AppliedPaint,
                observe.then_some(&self.config),
            );
            return vsync_sec;
        }

        let delivery = TextureDelivery::begin(
            self.render_state.as_ref(),
            textures_delta,
            crate::atlas_diag::Site::AppliedPaint,
            observe.then_some(&self.config),
        );
        let Some(mut delivery) = delivery else {
            return vsync_sec;
        };
        let render_state = delivery.render_state;
        probe.outcome = PaintOutcome::SurfaceAbsent;

        let Some(surface_state) = self.surfaces.get_mut(&viewport_id) else {
            return vsync_sec;
        };

        probe.outcome = PaintOutcome::Skipped;
        if observe {
            self.config.startup_begin(StartupGpuStage::Buffers);
        }
        let mut encoder =
            render_state
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("encoder"),
                });

        // Upload all resources for the GPU.
        let screen_descriptor = renderer::ScreenDescriptor {
            size_in_pixels: [surface_state.width, surface_state.height],
            pixels_per_point,
        };

        let user_cmd_bufs = {
            let mut renderer = render_state.renderer.write();
            renderer.update_buffers(
                &render_state.device,
                &render_state.queue,
                &mut encoder,
                clipped_primitives,
                &screen_descriptor,
            )
        };

        // update_buffers may have queued new writes after the last mipmap submission.
        delivery.queue_needs_submit = true;
        if observe {
            self.config.startup_end(StartupGpuStage::Buffers, true);
        }
        if surface_state.needs_reconfigure {
            Self::configure_surface(
                surface_state,
                render_state,
                &self.config,
                viewport_id,
                self.startup_present.pending(),
            );
            surface_state.needs_reconfigure = false;
        }

        let output_frame = {
            profiling::scope!("get_current_texture");
            // This is what vsync-waiting happens on my Mac.
            let start = web_time::Instant::now();
            if observe {
                self.config.startup_begin(StartupGpuStage::SurfaceAcquire);
            }
            let output_frame = surface_state.surface.get_current_texture();
            if observe {
                self.config.startup_end(
                    StartupGpuStage::SurfaceAcquire,
                    matches!(
                        output_frame,
                        wgpu::CurrentSurfaceTexture::Success(_)
                            | wgpu::CurrentSurfaceTexture::Suboptimal(_)
                    ),
                );
            }
            vsync_sec += start.elapsed().as_secs_f32();
            output_frame
        };

        // `None` when there is no frame to present to. A capture is rendered to its own
        // texture and copied back to the cpu, so it is served even then — that is how a hidden
        // window, which the compositor gives no surface texture, is screenshotted at all.
        let output_frame = match output_frame {
            wgpu::CurrentSurfaceTexture::Success(frame) => Some(frame),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                surface_state.needs_reconfigure = true;
                Some(frame)
            }
            other => {
                match (*self.config.on_surface_status)(&other) {
                    SurfaceErrorAction::Reconfigure => {
                        probe.outcome = PaintOutcome::SurfaceReconfigured;
                        Self::configure_surface(
                            surface_state,
                            render_state,
                            &self.config,
                            viewport_id,
                            self.startup_present.pending(),
                        );
                        self.context.request_repaint_of(viewport_id);
                    }
                    SurfaceErrorAction::RecreateSurface => {
                        probe.outcome = PaintOutcome::SurfaceRecreated;
                        // Because of ownership, I could not find an easy way to do a full recovery here,
                        // as that would involve dropping the old surface and creating a new one.
                        // For now, we defer the recreation to the beginning of the next frame (which
                        // we ensure to arrive via `request_repaint_of`). A cleaner solution would be
                        // to untangle the ownership of `RenderState`.
                        surface_state.needs_recreate = true;
                        self.context.request_repaint_of(viewport_id);
                    }
                    SurfaceErrorAction::SkipFrame => {}
                }
                if !capture {
                    return vsync_sec;
                }
                None
            }
        };

        if observe {
            self.config.startup_begin(StartupGpuStage::Encode);
        }
        let mut capture_buffer = None;
        {
            let renderer = render_state.renderer.read();

            let target_texture = if capture {
                // The frame we would have presented decides the capture, when there is one.
                // Without it we have only the surface we configured, which is the same thing
                // as long as no resize is in flight.
                let (size, format) = match &output_frame {
                    Some(output_frame) => {
                        (output_frame.texture.size(), output_frame.texture.format())
                    }
                    None => (
                        wgpu::Extent3d {
                            width: surface_state.width,
                            height: surface_state.height,
                            depth_or_array_layers: 1,
                        },
                        render_state.target_format,
                    ),
                };
                if size.width == 0 || size.height == 0 {
                    log::warn!("Cannot capture a screenshot of a zero-sized surface");
                    probe.outcome = PaintOutcome::CaptureZeroSized;
                    return vsync_sec;
                }

                let capture_state = self
                    .screen_capture_state
                    .get_or_insert_with(|| CaptureState::new(&render_state.device, size, format));
                capture_state.update(&render_state.device, size);

                &capture_state.texture
            } else if let Some(output_frame) = &output_frame {
                &output_frame.texture
            } else {
                // Unreachable: without a frame to present to, and with nothing to capture,
                // we returned above.
                return vsync_sec;
            };
            let target_view = target_texture.create_view(&wgpu::TextureViewDescriptor::default());

            let (view, resolve_target) = (self.options.msaa_samples > 1)
                .then_some(self.msaa_texture_view.get(&viewport_id))
                .flatten()
                .map_or((&target_view, None), |texture_view| {
                    (texture_view, Some(&target_view))
                });

            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui_render"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear_color[0] as f64,
                            g: clear_color[1] as f64,
                            b: clear_color[2] as f64,
                            a: clear_color[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: self.depth_texture_view.get(&viewport_id).map(|view| {
                    wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: self
                            .options
                            .depth_stencil_format
                            .is_some_and(|depth_stencil_format| {
                                depth_stencil_format.has_depth_aspect()
                            })
                            .then_some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(1.0),
                                // It is very unlikely that the depth buffer is needed after egui finished rendering
                                // so no need to store it. (this can improve performance on tiling GPUs like mobile chips or Apple Silicon)
                                store: wgpu::StoreOp::Discard,
                            }),
                        stencil_ops: self
                            .options
                            .depth_stencil_format
                            .is_some_and(|depth_stencil_format| {
                                depth_stencil_format.has_stencil_aspect()
                            })
                            .then_some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(0),
                                store: wgpu::StoreOp::Discard,
                            }),
                    }
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            // Forgetting the pass' lifetime means that we are no longer compile-time protected from
            // runtime errors caused by accessing the parent encoder before the render pass is dropped.
            // Since we don't pass it on to the renderer, we should be perfectly safe against this mistake here!
            renderer.render(
                &mut render_pass.forget_lifetime(),
                clipped_primitives,
                &screen_descriptor,
            );

            if capture && let Some(capture_state) = &mut self.screen_capture_state {
                capture_buffer = Some(capture_state.copy_textures(
                    &render_state.device,
                    output_frame.as_ref(),
                    &mut encoder,
                ));
            }
        }

        let encoded = {
            profiling::scope!("CommandEncoder::finish");
            encoder.finish()
        };

        if observe {
            self.config.startup_end(StartupGpuStage::Encode, true);
        }
        // Submit the commands: both the main buffer and user-defined ones.
        {
            profiling::scope!("Queue::submit");
            // wgpu doesn't document where vsync can happen. Maybe here?
            let start = web_time::Instant::now();
            if observe {
                self.config.startup_begin(StartupGpuStage::QueueSubmit);
            }
            delivery.submit(core::iter::chain(user_cmd_bufs, [encoded]));
            if observe {
                self.config.startup_end(StartupGpuStage::QueueSubmit, true);
            }
            vsync_sec += start.elapsed().as_secs_f32();
        };

        probe.outcome = if output_frame.is_some() {
            PaintOutcome::Submitted
        } else {
            PaintOutcome::CaptureOnly
        };
        // Complete frees after the real submission; early exits use the same owner.
        drop(delivery);

        if let Some(capture_buffer) = capture_buffer
            && let Some(screen_capture_state) = &mut self.screen_capture_state
        {
            if crate::atlas_diag::capture_probe_active() {
                crate::atlas_diag::log_line(format!(
                    "[capture-probe] wgpu_capture_readback_queued viewport={viewport_id:?}"
                ));
            }
            screen_capture_state.read_screen_rgba_in_viewport(
                capture_buffer,
                capture_data,
                viewport_id,
            );
        }

        if let Some(output_frame) = output_frame {
            if let Some(window) = window {
                window.pre_present_notify();
            }

            profiling::scope!("present");
            // wgpu doesn't document where vsync can happen. Maybe here?
            let start = web_time::Instant::now();
            if observe {
                self.config.startup_begin(StartupGpuStage::Present);
            }
            render_state.queue.present(output_frame);
            if observe {
                self.config.startup_end(StartupGpuStage::Present, true);
                if let Some(event) = self.startup_present.returned(viewport_id, startup_tag) {
                    self.config.startup_event(event);
                }
            }
            vsync_sec += start.elapsed().as_secs_f32();
        }

        vsync_sec
    }

    pub fn gc_viewports(&mut self, active_viewports: &ViewportIdSet) {
        self.surfaces.retain(|id, _| active_viewports.contains(id));
        self.depth_texture_view
            .retain(|id, _| active_viewports.contains(id));
        self.msaa_texture_view
            .retain(|id, _| active_viewports.contains(id));
    }

    #[expect(clippy::needless_pass_by_ref_mut, clippy::unused_self)]
    pub fn destroy(&mut self) {
        // TODO(emilk): something here?
    }
}

/// Shared completion owner extending upstream's queue guard: early returns submit queued
/// writes, then release resources. A normal submit suppresses the fallback empty submit.
struct TextureDelivery<'a> {
    render_state: &'a RenderState,
    textures_delta: &'a mut epaint::textures::TexturesDelta,
    queue_needs_submit: bool,
    startup_observer: Option<&'a WgpuConfiguration>,
    observation: Option<crate::texture_delivery::DeliveryObservation>,
}

impl<'a> TextureDelivery<'a> {
    fn begin(
        render_state: Option<&'a RenderState>,
        textures_delta: &'a mut epaint::textures::TexturesDelta,
        site: crate::atlas_diag::Site,
        startup_observer: Option<&'a WgpuConfiguration>,
    ) -> Option<Self> {
        if let Some(observer) = startup_observer {
            observer.startup_begin(StartupGpuStage::TextureDelivery);
        }
        let Some(render_state) = render_state else {
            if let Some(observer) = startup_observer {
                observer.startup_end(StartupGpuStage::TextureDelivery, false);
                // Preserve the finalization observation even when the pending owner retains
                // the entire batch because no renderer exists.
                observer.startup_begin(StartupGpuStage::TextureDelivery);
                observer.startup_end(StartupGpuStage::TextureDelivery, false);
            }
            return None;
        };
        // Preserve upstream's fallback submission unless an upload actually submits the queue.
        let mut queue_needs_submit = true;
        let mut observation = crate::texture_delivery::DeliveryObservation::begin();
        {
            let mut renderer = render_state.renderer.write();
            let diag_id = renderer.diag_id();
            let batch = crate::atlas_diag::begin_applied_batch(site, diag_id, textures_delta);
            #[expect(clippy::iter_over_hash_type)] // Each ID's delta order is preserved.
            for (id, image_deltas) in textures_delta.set.drain() {
                for image_delta in image_deltas {
                    let before = renderer.texture_size(&id);
                    let submission = renderer.update_texture_with_submission(
                        &render_state.device,
                        &render_state.queue,
                        id,
                        &image_delta,
                    );
                    match submission {
                        renderer::TextureUpdateSubmission::Unchanged => {}
                        renderer::TextureUpdateSubmission::Pending => queue_needs_submit = true,
                        renderer::TextureUpdateSubmission::Submitted => queue_needs_submit = false,
                    }
                    if !matches!(submission, renderer::TextureUpdateSubmission::Unchanged)
                        && let Some(observation) = observation.as_mut()
                    {
                        observation.record(
                            diag_id,
                            id,
                            &image_delta,
                            matches!(submission, renderer::TextureUpdateSubmission::Submitted),
                            site,
                        );
                    }
                    let after = renderer.texture_size(&id);
                    crate::atlas_diag::record_applied(id, diag_id, &image_delta, before, after);
                }
            }
            if batch.tracked() {
                crate::atlas_diag::flush("applying a texture delta batch");
            }
        }
        if let Some(observer) = startup_observer {
            observer.startup_end(StartupGpuStage::TextureDelivery, true);
        }
        Some(Self {
            render_state,
            textures_delta,
            queue_needs_submit,
            startup_observer,
            observation,
        })
    }

    fn submit(&mut self, commands: impl IntoIterator<Item = wgpu::CommandBuffer>) {
        self.render_state.queue.submit(commands);
        #[cfg(test)]
        SUBMISSION_COUNTS.with(|counts| {
            let mut next = counts.get();
            next.normal += 1;
            counts.set(next);
        });
        self.queue_needs_submit = false;
        if let Some(observation) = self.observation.as_mut() {
            observation.submitted();
        }
    }
}

impl Drop for TextureDelivery<'_> {
    fn drop(&mut self) {
        if let Some(observer) = self.startup_observer {
            observer.startup_begin(StartupGpuStage::TextureDelivery);
        }
        if self.queue_needs_submit {
            self.render_state.queue.submit([]);
            #[cfg(test)]
            SUBMISSION_COUNTS.with(|counts| {
                let mut next = counts.get();
                next.empty += 1;
                counts.set(next);
            });
        }
        // Mip generation may already have submitted the entire batch. Otherwise
        // the normal or fallback submit above has now completed the queue handoff.
        if let Some(observation) = self.observation.as_mut() {
            observation.submitted();
        }
        {
            let mut renderer = self.render_state.renderer.write();
            #[expect(clippy::iter_over_hash_type)]
            for id in self.textures_delta.free.drain() {
                renderer.free_texture(&id);
            }
        }
        if let Some(observer) = self.startup_observer {
            observer.startup_end(StartupGpuStage::TextureDelivery, true);
        }
    }
}

// Observe only actual returned submit calls, with no production state or queue added.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SubmissionCounts {
    normal: usize,
    empty: usize,
}

#[cfg(test)]
std::thread_local! {
    static SUBMISSION_COUNTS: std::cell::Cell<SubmissionCounts> = const { std::cell::Cell::new(SubmissionCounts { normal: 0, empty: 0 }) };
}

#[cfg(test)]
fn submission_counts() -> SubmissionCounts {
    SUBMISSION_COUNTS.with(std::cell::Cell::get)
}

fn remove_viewport_resources<S, D, M>(
    viewport_id: ViewportId,
    surfaces: &mut ViewportIdMap<S>,
    depth_views: &mut ViewportIdMap<D>,
    msaa_views: &mut ViewportIdMap<M>,
) {
    surfaces.remove(&viewport_id);
    depth_views.remove(&viewport_id);
    msaa_views.remove(&viewport_id);
}

#[cfg(test)]
#[path = "winit_tests.rs"]
mod tests;

/// Diagnostic classification only; resource completion belongs exclusively to TextureDelivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaintOutcome {
    RenderStateAbsent,
    AppliedWithoutPaint,
    SurfaceAbsent,
    SurfaceRecreated,
    SurfaceRecreationFailed,
    SurfaceReconfigured,
    Skipped,
    CaptureZeroSized,
    CaptureOnly,
    Submitted,
}

/// Keep only a printable summary across draining. It carries no texture application rights.
/// Constructed before TextureDelivery so its drop observes that owner's completed submission/free.
struct DeliveryProbe {
    site: &'static str,
    viewport_id: ViewportId,
    capture: bool,
    atlas: String,
    outcome: PaintOutcome,
}

impl DeliveryProbe {
    fn new(
        site: &'static str,
        viewport_id: ViewportId,
        capture: bool,
        textures_delta: &epaint::textures::TexturesDelta,
        outcome: PaintOutcome,
    ) -> Self {
        let atlas = textures_delta
            .set
            .get(&epaint::TextureId::default())
            .into_iter()
            .flatten()
            .map(|delta| {
                format!(
                    "{}x{}{}",
                    delta.image.width(),
                    delta.image.height(),
                    if delta.pos.is_none() {
                        "full"
                    } else {
                        "partial"
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        Self {
            site,
            viewport_id,
            capture,
            atlas,
            outcome,
        }
    }
}

impl Drop for DeliveryProbe {
    fn drop(&mut self) {
        if !self.atlas.is_empty() {
            crate::atlas_diag::log_line(format!(
                "[atlas-probe] site={} outcome={:?} deltas=[{}]",
                self.site, self.outcome, self.atlas
            ));
        }
        if self.capture && crate::atlas_diag::capture_probe_active() {
            crate::atlas_diag::log_line(format!(
                "[capture-probe] wgpu_capture_outcome viewport={:?} outcome={:?}",
                self.viewport_id, self.outcome
            ));
        }
    }
}
