use super::*;
use epaint::{Color32, ColorImage, ImageDelta, TextureId, textures::TextureOptions};

// No window, surface, application profile or egui FullOutput is created by these GPU tests.
fn headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        // Avoid the OpenGL/WGL adapter initialization path on Windows test machines.
        backends: if cfg!(target_os = "windows") {
            wgpu::Backends::DX12
        } else {
            wgpu::Backends::PRIMARY
        },
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = match pollster::block_on(instance.request_adapter(&Default::default())) {
        Ok(adapter) => adapter,
        Err(error) => {
            eprintln!("skipping headless renderer GPU test: {error}");
            return None;
        }
    };
    Some(
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("create headless renderer test device"),
    )
}

/// Render through the actual egui bind group and sampler, so managed textures need no test-only
/// COPY_SRC usage and the assertion covers the view's mip range as well as its generated pixels.
pub(crate) fn read_rendered_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut Renderer,
    id: TextureId,
    size: [u32; 2],
) -> Vec<Color32> {
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("renderer regression output"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let rect = epaint::Rect::from_min_size(
        epaint::Pos2::ZERO,
        epaint::Vec2::new(size[0] as f32, size[1] as f32),
    );
    let mut mesh = epaint::Mesh::with_texture(id);
    mesh.add_rect_with_uv(
        rect,
        epaint::Rect::from_min_max(epaint::Pos2::ZERO, epaint::Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );
    let jobs = [epaint::ClippedPrimitive {
        clip_rect: rect,
        primitive: Primitive::Mesh(mesh),
    }];
    let screen = ScreenDescriptor {
        size_in_pixels: size,
        pixels_per_point: 1.0,
    };
    let mut encoder = device.create_command_encoder(&Default::default());
    let callbacks = renderer.update_buffers(device, queue, &mut encoder, &jobs, &screen);
    assert!(callbacks.is_empty());
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.create_view(&Default::default()),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            })
            .forget_lifetime();
        renderer.render(&mut pass, &jobs, &screen);
    }
    queue.submit([encoder.finish()]);
    read_texture_level(device, queue, &target, 0, size)
}

fn read_texture_level(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    mip_level: u32,
    size: [u32; 2],
) -> Vec<Color32> {
    let stride = (size[0] * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("renderer regression readback"),
        size: u64::from(stride * size[1]),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(size[1]),
            },
        },
        wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).expect("readback receiver");
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .expect("wait for offscreen readback");
    receiver
        .recv()
        .expect("readback callback")
        .expect("map readback");
    let mapped = buffer
        .slice(..)
        .get_mapped_range()
        .expect("mapped readback range");
    let pixels = mapped
        .chunks_exact(stride as usize)
        .flat_map(|row| row[..size[0] as usize * 4].chunks_exact(4))
        .map(|pixel| Color32::from_rgba_premultiplied(pixel[0], pixel[1], pixel[2], pixel[3]))
        .collect();
    drop(mapped);
    buffer.unmap();
    pixels
}

#[test]
fn managed_mipmaps_regenerate_after_partial_upload_and_respect_view_options() {
    let Some((device, queue)) = headless_device() else {
        return;
    };
    let mut renderer = Renderer::new(
        &device,
        wgpu::TextureFormat::Rgba8Unorm,
        RendererOptions {
            dithering: false,
            ..Default::default()
        },
    );
    let id = TextureId::Managed(1);
    let options =
        TextureOptions::LINEAR.with_mipmap_mode(Some(epaint::textures::TextureFilter::Linear));
    renderer.update_texture(
        &device,
        &queue,
        id,
        &ImageDelta::full(ColorImage::filled([4, 4], Color32::BLACK), options),
    );
    assert_eq!(
        renderer
            .texture(&id)
            .unwrap()
            .texture
            .as_ref()
            .unwrap()
            .mip_level_count(),
        3
    );
    assert_eq!(
        read_rendered_texture(&device, &queue, &mut renderer, id, [1, 1]),
        [Color32::BLACK]
    );

    renderer.update_texture(
        &device,
        &queue,
        id,
        &ImageDelta::partial([0, 0], ColorImage::filled([1, 1], Color32::WHITE), options),
    );
    let sampled = read_rendered_texture(&device, &queue, &mut renderer, id, [1, 1])[0];
    assert!(
        (15..=17).contains(&sampled.r()),
        "regenerated 1x1 mip must average the white corner: {sampled:?}"
    );
    assert_eq!(sampled.r(), sampled.g());
    assert_eq!(sampled.r(), sampled.b());
    assert_eq!(sampled.a(), 255);

    renderer.update_texture(
        &device,
        &queue,
        id,
        &ImageDelta::partial(
            [0, 0],
            ColorImage::filled([1, 1], Color32::WHITE),
            TextureOptions::LINEAR,
        ),
    );
    assert_eq!(
        read_rendered_texture(&device, &queue, &mut renderer, id, [1, 1]),
        [Color32::BLACK],
        "disabling mipmaps must restrict the view to level zero"
    );
    renderer.update_texture(
        &device,
        &queue,
        id,
        &ImageDelta::full(
            ColorImage::filled([2, 2], Color32::WHITE),
            TextureOptions::LINEAR,
        ),
    );
    assert_eq!(renderer.texture_size(&id), Some([2, 2]));
    assert_eq!(
        renderer
            .texture(&id)
            .unwrap()
            .texture
            .as_ref()
            .unwrap()
            .mip_level_count(),
        1
    );
    renderer.free_texture(&id);
    assert!(renderer.texture_size(&id).is_none());
}

#[test]
fn public_mipmap_generator_preserves_odd_edge_texels() {
    let Some((device, queue)) = headless_device() else {
        return;
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("odd mipmap regression"),
        size: wgpu::Extent3d {
            width: 3,
            height: 3,
            depth_or_array_layers: 1,
        },
        mip_level_count: crate::mip_level_count(3, 3),
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let mut image = [Color32::BLACK; 9];
    image[8] = Color32::WHITE;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&image),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(12),
            rows_per_image: Some(3),
        },
        texture.size(),
    );
    crate::MipmapGenerator::new(&device).generate(&device, &queue, &texture);
    let pixel = read_texture_level(&device, &queue, &texture, 1, [1, 1])[0];
    assert!(
        (27..=29).contains(&pixel.r()),
        "last row and column must contribute one ninth: {pixel:?}"
    );
    assert_eq!(pixel.r(), pixel.g());
    assert_eq!(pixel.r(), pixel.b());
    assert_eq!(pixel.a(), 255);
}

#[test]
fn egui_and_mipmap_shaders_parse_and_validate() {
    for source in [include_str!("egui.wgsl"), include_str!("mipmap.wgsl")] {
        let module = wgpu::naga::front::wgsl::parse_str(source).expect("parse renderer WGSL");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("validate renderer WGSL");
    }
}

#[test]
fn image_sampling_uniform_uses_base_renderer_layout() {
    assert_eq!(std::mem::size_of::<UniformBuffer>(), 16);
    let shader = include_str!("egui.wgsl");
    assert!(shader.contains("textureSample(r_tex_color"));
    assert!(!shader.contains("textureSampleLevel(r_tex_color"));
    assert!(!shader.contains("textureSampleBias("));
}
