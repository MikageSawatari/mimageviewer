//! Anime-style visible-region GPU upscaling generated from Anime4K x2 variants.

use std::borrow::Cow;

use wgpu::util::DeviceExt as _;

const RECEPTIVE_MARGIN: i64 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Anime4kVariant {
    Small,
    Medium,
    Large,
    VeryLarge,
    UltraLarge,
}

impl Anime4kVariant {
    #[cfg(test)]
    pub(crate) const ALL: [Self; 5] = [
        Self::Small,
        Self::Medium,
        Self::Large,
        Self::VeryLarge,
        Self::UltraLarge,
    ];

    fn data(self) -> &'static Anime4kVariantData {
        GENERATED_ANIME4K_VARIANTS
            .iter()
            .find(|data| data.variant == self)
            .expect("generated Anime4K data for every variant")
    }

    #[cfg(test)]
    pub(crate) fn shader(self) -> &'static str {
        self.data().shader
    }
}

pub(crate) const STILL_IMAGE_ANIME4K_VARIANT: Anime4kVariant = Anime4kVariant::VeryLarge;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Anime4kPassInput {
    Source,
    Intermediate(usize),
}

#[derive(Debug)]
struct Anime4kVariantData {
    variant: Anime4kVariant,
    label: &'static str,
    shader: &'static str,
    input_binding_count: usize,
    /// Convolution pass inputs followed by the final resolve pass inputs.
    pass_inputs: &'static [&'static [Anime4kPassInput]],
}

impl Anime4kVariantData {
    fn intermediate_count(&self) -> usize {
        self.pass_inputs
            .len()
            .checked_sub(1)
            .expect("generated Anime4K topology includes a resolve pass")
    }
}

include!("gpu_anime4k_generated.rs");

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Anime4kPlan {
    pub(crate) source_size: [u32; 2],
    pub(crate) target_size: [u32; 2],
    pub(crate) source_region_px: [f32; 4],
    pub(crate) process_origin: [i32; 2],
    pub(crate) process_size: [u32; 2],
    pub(crate) texture_fetches: u64,
}

impl Anime4kPlan {
    pub(crate) fn new(
        source_size: [u32; 2],
        target_size: [u32; 2],
        source_region_px: [f32; 4],
    ) -> Result<Self, ()> {
        if source_size.contains(&0)
            || target_size.contains(&0)
            || source_region_px.iter().any(|value| !value.is_finite())
            || source_region_px[2] <= 0.0
            || source_region_px[3] <= 0.0
        {
            return Err(());
        }
        let mut origin = [0_i32; 2];
        let mut size = [0_u32; 2];
        for axis in 0..2 {
            let start = source_region_px[axis].floor() as i64 - RECEPTIVE_MARGIN;
            let end = (source_region_px[axis] + source_region_px[axis + 2]).ceil() as i64
                + RECEPTIVE_MARGIN;
            let start = start.clamp(0, i64::from(source_size[axis]) - 1);
            let end = end.clamp(start + 1, i64::from(source_size[axis]));
            origin[axis] = start as i32;
            size[axis] = (end - start) as u32;
        }
        let process_pixels = u64::from(size[0]).saturating_mul(u64::from(size[1]));
        let resolve_pixels = u64::from(target_size[0]).saturating_mul(u64::from(target_size[1]));
        Ok(Self {
            source_size,
            target_size,
            source_region_px,
            process_origin: origin,
            process_size: size,
            texture_fetches: process_pixels
                .saturating_mul(534)
                .saturating_add(resolve_pixels.saturating_mul(20)),
        })
    }
}

pub(crate) struct Anime4kResampler {
    variant_data: &'static Anime4kVariantData,
    bind_group_layout: wgpu::BindGroupLayout,
    convolution_pipelines: Vec<wgpu::RenderPipeline>,
    resolve_pipeline: wgpu::RenderPipeline,
}

impl Anime4kResampler {
    pub(crate) fn new(device: &wgpu::Device, variant: Anime4kVariant) -> Self {
        let variant_data = variant.data();
        let mut entries = (0..variant_data.input_binding_count)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding: binding as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            })
            .collect::<Vec<_>>();
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: variant_data.input_binding_count as u32,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("anime4k bind group layout"),
            entries: &entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("anime4k pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(variant_data.label),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(variant_data.shader)),
        });
        let convolution_pipelines = (0..variant_data.intermediate_count())
            .map(|index| {
                create_pipeline(
                    device,
                    &layout,
                    &shader,
                    &format!("fs_anime4k_{index}"),
                    wgpu::TextureFormat::Rgba16Float,
                )
            })
            .collect();
        let resolve_pipeline = create_pipeline(
            device,
            &layout,
            &shader,
            "fs_anime4k_resolve",
            wgpu::TextureFormat::Rgba8Unorm,
        );
        Self {
            variant_data,
            bind_group_layout,
            convolution_pipelines,
            resolve_pipeline,
        }
    }

    pub(crate) fn prepare_job(
        &self,
        device: &wgpu::Device,
        source: &wgpu::Texture,
        plan: Anime4kPlan,
    ) -> Result<Anime4kJob, ()> {
        if source.format() != wgpu::TextureFormat::Rgba8Unorm
            || [source.width(), source.height()] != plan.source_size
        {
            return Err(());
        }
        let source_view = source.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let intermediate_count = self.variant_data.intermediate_count();
        let intermediate_textures = (0..intermediate_count)
            .map(|_| {
                create_target_texture(device, plan.process_size, wgpu::TextureFormat::Rgba16Float)
            })
            .collect::<Vec<_>>();
        let intermediate_views = intermediate_textures
            .iter()
            .map(|texture| texture.create_view(&Default::default()))
            .collect::<Vec<_>>();
        let output_texture =
            create_target_texture(device, plan.target_size, wgpu::TextureFormat::Rgba8Unorm);
        let output_view = output_texture.create_view(&Default::default());
        let source_uniform = params_uniform(
            device,
            plan,
            plan.process_size,
            plan.source_size,
            plan.process_origin,
        );
        let intermediate_uniform =
            params_uniform(device, plan, plan.process_size, plan.process_size, [0, 0]);
        let resolve_uniform =
            params_uniform(device, plan, plan.target_size, plan.source_size, [0, 0]);

        let mut bind_groups = Vec::with_capacity(self.variant_data.pass_inputs.len());
        for inputs in &self.variant_data.pass_inputs[..intermediate_count] {
            let views = resolve_pass_views(inputs, &source_view, &intermediate_views);
            let uniform = if inputs
                .iter()
                .all(|input| *input == Anime4kPassInput::Source)
            {
                &source_uniform
            } else {
                &intermediate_uniform
            };
            bind_groups.push(create_bind_group(
                device,
                &self.bind_group_layout,
                self.variant_data.input_binding_count,
                &views,
                uniform,
            ));
        }
        let resolve_views = resolve_pass_views(
            self.variant_data
                .pass_inputs
                .last()
                .expect("generated Anime4K resolve inputs"),
            &source_view,
            &intermediate_views,
        );
        bind_groups.push(create_bind_group(
            device,
            &self.bind_group_layout,
            self.variant_data.input_binding_count,
            &resolve_views,
            &resolve_uniform,
        ));
        Ok(Anime4kJob {
            _intermediate_textures: intermediate_textures,
            intermediate_views,
            output_texture,
            output_view,
            bind_groups,
            _uniforms: [source_uniform, intermediate_uniform, resolve_uniform],
        })
    }

    pub(crate) fn encode(&self, encoder: &mut wgpu::CommandEncoder, job: &Anime4kJob) {
        let intermediate_count = self.variant_data.intermediate_count();
        for pass in 0..intermediate_count {
            encode_pass(
                encoder,
                &job.intermediate_views[pass],
                &self.convolution_pipelines[pass],
                &job.bind_groups[pass],
            );
        }
        encode_pass(
            encoder,
            &job.output_view,
            &self.resolve_pipeline,
            &job.bind_groups[intermediate_count],
        );
    }
}

pub(crate) struct Anime4kJob {
    _intermediate_textures: Vec<wgpu::Texture>,
    intermediate_views: Vec<wgpu::TextureView>,
    output_texture: wgpu::Texture,
    output_view: wgpu::TextureView,
    bind_groups: Vec<wgpu::BindGroup>,
    _uniforms: [wgpu::Buffer; 3],
}

impl Anime4kJob {
    pub(crate) fn output_view(&self) -> &wgpu::TextureView {
        &self.output_view
    }

    pub(crate) fn into_output_texture(self) -> wgpu::Texture {
        self.output_texture
    }
}

fn create_target_texture(
    device: &wgpu::Device,
    size: [u32; 2],
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("anime4k target"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn params_uniform(
    device: &wgpu::Device,
    plan: Anime4kPlan,
    output_size: [u32; 2],
    input_size: [u32; 2],
    input_origin: [i32; 2],
) -> wgpu::Buffer {
    let mut bytes = [0_u8; 64];
    for (index, value) in output_size.into_iter().enumerate() {
        bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_ne_bytes());
    }
    for (index, value) in input_size.into_iter().enumerate() {
        let offset = 8 + index * 4;
        bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }
    for (index, value) in input_origin.into_iter().enumerate() {
        let offset = 16 + index * 4;
        bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }
    for (index, value) in plan.process_origin.into_iter().enumerate() {
        let offset = 24 + index * 4;
        bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }
    for (base, values) in [(32, plan.source_size), (40, plan.process_size)] {
        for (index, value) in values.into_iter().enumerate() {
            let offset = base + index * 4;
            bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
        }
    }
    for (index, value) in plan.source_region_px.into_iter().enumerate() {
        let offset = 48 + index * 4;
        bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("anime4k params"),
        contents: &bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    })
}

fn resolve_pass_views<'a>(
    inputs: &[Anime4kPassInput],
    source_view: &'a wgpu::TextureView,
    intermediate_views: &'a [wgpu::TextureView],
) -> Vec<&'a wgpu::TextureView> {
    inputs
        .iter()
        .map(|input| match *input {
            Anime4kPassInput::Source => source_view,
            Anime4kPassInput::Intermediate(index) => intermediate_views
                .get(index)
                .expect("generated Anime4K input references an earlier pass"),
        })
        .collect()
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    input_binding_count: usize,
    views: &[&wgpu::TextureView],
    uniform: &wgpu::Buffer,
) -> wgpu::BindGroup {
    let fallback = views[0];
    let mut entries = (0..input_binding_count)
        .map(|binding| wgpu::BindGroupEntry {
            binding: binding as u32,
            resource: wgpu::BindingResource::TextureView(
                views.get(binding).copied().unwrap_or(fallback),
            ),
        })
        .collect::<Vec<_>>();
    entries.push(wgpu::BindGroupEntry {
        binding: input_binding_count as u32,
        resource: uniform.as_entire_binding(),
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("anime4k pass inputs"),
        layout,
        entries: &entries,
    })
}

fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    fragment_entry: &str,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(fragment_entry),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    })
}

fn encode_pass(
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("anime4k convolution pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::{Anime4kPassInput, Anime4kVariant, STILL_IMAGE_ANIME4K_VARIANT};

    #[test]
    fn generated_variant_topologies_are_complete_and_self_consistent() {
        let expected = [
            (Anime4kVariant::Small, 5, 2, 1),
            (Anime4kVariant::Medium, 9, 7, 1),
            (Anime4kVariant::Large, 10, 4, 3),
            (Anime4kVariant::VeryLarge, 18, 14, 3),
            (Anime4kVariant::UltraLarge, 25, 15, 3),
        ];
        assert_eq!(Anime4kVariant::ALL.len(), expected.len());

        for (variant, pass_count, binding_count, correction_count) in expected {
            let data = variant.data();
            assert_eq!(data.variant, variant);
            assert_eq!(data.pass_inputs.len(), pass_count, "{variant:?}");
            assert_eq!(data.input_binding_count, binding_count, "{variant:?}");
            assert_eq!(
                data.pass_inputs.iter().map(|inputs| inputs.len()).max(),
                Some(binding_count),
                "{variant:?}"
            );
            assert!(data.shader.starts_with("// MIT License"), "{variant:?}");
            assert!(
                data.shader.contains(&format!(
                    "Anime4K_Upscale_CNN_x2_{}.glsl",
                    &data.label[11..]
                )),
                "{variant:?}"
            );

            for (pass, inputs) in data.pass_inputs[..data.intermediate_count()]
                .iter()
                .enumerate()
            {
                assert!(!inputs.is_empty(), "{variant:?} pass {pass}");
                let source_only = inputs
                    .iter()
                    .all(|input| *input == Anime4kPassInput::Source);
                let intermediates_only = inputs.iter().all(|input| {
                    matches!(
                        input,
                        Anime4kPassInput::Intermediate(index) if *index < pass
                    )
                });
                assert!(source_only || intermediates_only, "{variant:?} pass {pass}");
            }

            let resolve = data.pass_inputs.last().unwrap();
            assert_eq!(resolve.first(), Some(&Anime4kPassInput::Source));
            assert_eq!(resolve.len() - 1, correction_count, "{variant:?}");
            assert!(resolve[1..].iter().all(|input| {
                matches!(
                    input,
                    Anime4kPassInput::Intermediate(index)
                        if *index < data.intermediate_count()
                )
            }));
        }
    }

    #[test]
    fn still_image_path_remains_on_very_large() {
        assert_eq!(STILL_IMAGE_ANIME4K_VARIANT, Anime4kVariant::VeryLarge);
    }
}
