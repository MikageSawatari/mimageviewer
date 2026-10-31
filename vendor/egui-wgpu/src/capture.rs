use egui::ScreenshotCallback;
use epaint::ColorImage;
use std::sync::Arc;
use wgpu::{BindGroupLayout, MultisampleState, StoreOp};

/// A texture and a buffer for reading the rendered frame back to the cpu.
///
/// The texture is required since [`wgpu::TextureUsages::COPY_SRC`] is not an allowed
/// flag for the surface texture on all platforms. This means that anytime we want to
/// capture the frame, we first render it to this texture, and then we can copy it to
/// both the surface texture (via a render pass) and the buffer (via a texture to buffer copy),
/// from where we can pull it back
/// to the cpu.
///
/// Since the frame is rendered to this texture and not to the surface, a capture needs no
/// surface texture at all: an occluded or minimized window, which has none to give, can still
/// be captured.
pub struct CaptureState {
    padding: BufferPadding,
    pub texture: wgpu::Texture,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
}

impl CaptureState {
    /// `size` and `format` are those of the surface we would otherwise render to.
    pub fn new(device: &wgpu::Device, size: wgpu::Extent3d, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("texture_copy.wgsl"));

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("texture_copy"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let bind_group_layout = pipeline.get_bind_group_layout(0);

        let (texture, padding, bind_group) =
            Self::create_texture(device, size, format, &bind_group_layout);

        Self {
            padding,
            texture,
            pipeline,
            bind_group,
        }
    }

    fn create_texture(
        device: &wgpu::Device,
        size: wgpu::Extent3d,
        format: wgpu::TextureFormat,
        layout: &BindGroupLayout,
    ) -> (wgpu::Texture, BufferPadding, wgpu::BindGroup) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("egui_screen_capture_texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let padding = BufferPadding::new(size.width);

        let view = texture.create_view(&Default::default());

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
            label: None,
        });

        (texture, padding, bind_group)
    }

    /// Updates the [`CaptureState`] if the size of the surface has changed
    pub fn update(&mut self, device: &wgpu::Device, size: wgpu::Extent3d) {
        if self.texture.size() != size {
            let format = self.texture.format();
            let (new_texture, padding, bind_group) = Self::create_texture(
                device,
                size,
                format,
                &self.pipeline.get_bind_group_layout(0),
            );
            self.texture = new_texture;
            self.padding = padding;
            self.bind_group = bind_group;
        }
    }

    /// Handles copying from the [`CaptureState`] texture to the buffer, and to the surface
    /// texture when there is one to present to.
    /// Pass the returned buffer to [`CaptureState::read_screen_rgba`] to read the data back to the cpu.
    pub fn copy_textures(
        &mut self,
        device: &wgpu::Device,
        output_frame: Option<&wgpu::SurfaceTexture>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> wgpu::Buffer {
        debug_assert!(
            output_frame.is_none_or(|frame| frame.texture.size() == self.texture.size()),
            "Texture sizes must match, `CaptureState::update` was probably not called"
        );

        // It would be more efficient to reuse the Buffer, e.g. via some kind of ring buffer, but
        // for most screenshot use cases this should be fine. When taking many screenshots (e.g. for a video)
        // it might make sense to revisit this and implement a more efficient solution.
        #[allow(clippy::allow_attributes, clippy::arc_with_non_send_sync)] // For wasm
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("egui_screen_capture_buffer"),
            size: (self.padding.padded_bytes_per_row * self.texture.height()) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let padding = self.padding;
        let tex = &mut self.texture;

        let tex_extent = tex.size();

        encoder.copy_texture_to_buffer(
            tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padding.padded_bytes_per_row),
                    rows_per_image: None,
                },
            },
            tex_extent,
        );

        // Blit to the surface, for the frame that is actually shown. A hidden window has no
        // surface texture to blit to, and nothing to show: the capture above is all we want.
        if let Some(output_frame) = output_frame {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("texture_copy"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output_frame.texture.create_view(&Default::default()),
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        buffer
    }

    /// Handles copying from the [`CaptureState`] texture to the surface texture and the cpu
    /// This function is non-blocking and will send the data to the given sender when it's ready.
    /// Pass in the buffer returned from [`CaptureState::copy_textures`].
    /// Make sure to call this after the encoder has been submitted.
    pub fn read_screen_rgba(&self, buffer: wgpu::Buffer, callbacks: Vec<ScreenshotCallback>) {
        self.read_screen_rgba_with_viewport(buffer, callbacks, None);
    }

    pub(crate) fn read_screen_rgba_in_viewport(
        &self,
        buffer: wgpu::Buffer,
        callbacks: Vec<ScreenshotCallback>,
        viewport_id: egui::ViewportId,
    ) {
        self.read_screen_rgba_with_viewport(buffer, callbacks, Some(viewport_id));
    }

    fn read_screen_rgba_with_viewport(
        &self,
        buffer: wgpu::Buffer,
        callbacks: Vec<ScreenshotCallback>,
        viewport_id: Option<egui::ViewportId>,
    ) {
        #[allow(clippy::allow_attributes, clippy::arc_with_non_send_sync)] // For wasm
        let buffer = Arc::new(buffer);
        let buffer_clone = Arc::clone(&buffer);
        let buffer_slice = buffer_clone.slice(..);
        let format = self.texture.format();
        let tex_extent = self.texture.size();
        let padding = self.padding;
        let to_rgba = match format {
            wgpu::TextureFormat::Rgba8Unorm => [0, 1, 2, 3],
            wgpu::TextureFormat::Bgra8Unorm => [2, 1, 0, 3],
            _ => {
                if crate::atlas_diag::capture_probe_active() {
                    crate::atlas_diag::log_line(format!(
                        "[capture-probe] wgpu_capture_image viewport={viewport_id:?} produced=false unsupported_format={format:?} callbacks={}",
                        callbacks.len()
                    ));
                }
                log::error!(
                    "Screen can't be captured unless the surface format is Rgba8Unorm or Bgra8Unorm. Current surface format is {format:?}"
                );
                return;
            }
        };
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            if let Err(err) = result {
                if crate::atlas_diag::capture_probe_active() {
                    crate::atlas_diag::log_line(format!(
                        "[capture-probe] wgpu_capture_image viewport={viewport_id:?} produced=false map_error={err}"
                    ));
                }
                log::error!("Failed to map buffer for reading: {err}");
                return;
            }
            let buffer_slice = buffer.slice(..);

            let mut pixels = Vec::with_capacity((tex_extent.width * tex_extent.height) as usize);
            let mapped_range = match buffer_slice.get_mapped_range() {
                Ok(range) => range,
                Err(err) => {
                    if crate::atlas_diag::capture_probe_active() {
                        crate::atlas_diag::log_line(format!(
                            "[capture-probe] wgpu_capture_image viewport={viewport_id:?} produced=false mapped_range_error={err}"
                        ));
                    }
                    log::error!("Failed to get mapped range for reading: {err}");
                    return;
                }
            };
            for padded_row in mapped_range.chunks(padding.padded_bytes_per_row as usize) {
                let row = &padded_row[..padding.unpadded_bytes_per_row as usize];
                for color in row.chunks(4) {
                    pixels.push(epaint::Color32::from_rgba_premultiplied(
                        color[to_rgba[0]],
                        color[to_rgba[1]],
                        color[to_rgba[2]],
                        color[to_rgba[3]],
                    ));
                }
            }
            drop(mapped_range);
            buffer.unmap();

            let image = Arc::new(ColorImage::new(
                [tex_extent.width as usize, tex_extent.height as usize],
                pixels,
            ));
            let callback_count = callbacks.len();
            for callback in callbacks {
                callback.complete(Arc::clone(&image));
            }
            if crate::atlas_diag::capture_probe_active() {
                crate::atlas_diag::log_line(format!(
                    "[capture-probe] wgpu_capture_image viewport={viewport_id:?} produced=true size={}x{} callback_requests={callback_count}",
                    tex_extent.width, tex_extent.height
                ));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn capture_headless_callback_readback_preserves_rgba_and_unpads_bgra() {
        struct RestoreProbe(bool);
        impl Drop for RestoreProbe {
            fn drop(&mut self) {
                crate::atlas_diag::set_capture_probe(self.0);
            }
        }
        let _restore = RestoreProbe(crate::atlas_diag::capture_probe_active());
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .expect("headless DX12 adapter required for capture regression");
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))
            .expect("headless capture device");
        for format in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Bgra8Unorm,
        ] {
            let probe = format == wgpu::TextureFormat::Bgra8Unorm;
            crate::atlas_diag::set_capture_probe(probe);
            assert_eq!(crate::atlas_diag::capture_probe_active(), probe);
            let mut capture = CaptureState::new(
                &device,
                wgpu::Extent3d {
                    width: 3,
                    height: 2,
                    depth_or_array_layers: 1,
                },
                format,
            );
            // Exercise resize and the non-256-byte row padding path.
            capture.update(
                &device,
                wgpu::Extent3d {
                    width: 5,
                    height: 3,
                    depth_or_array_layers: 1,
                },
            );
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let view = capture.texture.create_view(&Default::default());
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 1.0,
                                g: 0.0,
                                b: 0.0,
                                a: 1.0,
                            }),
                            store: StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    ..Default::default()
                });
            }
            let buffer = capture.copy_textures(&device, None, &mut encoder);
            let submission = queue.submit([encoder.finish()]);
            let (tx, rx) = mpsc::channel();
            let first_tx = tx.clone();
            let callback = ScreenshotCallback::new(move |image| first_tx.send(image).unwrap());
            let second = ScreenshotCallback::new(move |image| tx.send(image).unwrap());
            capture.read_screen_rgba(buffer, vec![callback.clone(), callback, second]);
            // Waiting is confined to headless readback tests, never the painter.
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: Some(Duration::from_secs(10)),
                })
                .expect("readback callbacks complete");
            for _ in 0..2 {
                let image = rx.recv_timeout(Duration::from_secs(10)).unwrap();
                assert_eq!(image.size, [5, 3]);
                assert_eq!(image.pixels, vec![epaint::Color32::RED; 15]);
            }
            assert!(rx.try_recv().is_err(), "a cloned callback remains one-shot");
        }
    }
}

#[derive(Copy, Clone)]
struct BufferPadding {
    unpadded_bytes_per_row: u32,
    padded_bytes_per_row: u32,
}

impl BufferPadding {
    fn new(width: u32) -> Self {
        let bytes_per_pixel = core::mem::size_of::<u32>() as u32;
        let unpadded_bytes_per_row = width * bytes_per_pixel;
        let padded_bytes_per_row =
            wgpu::util::align_to(unpadded_bytes_per_row, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        Self {
            unpadded_bytes_per_row,
            padded_bytes_per_row,
        }
    }
}
