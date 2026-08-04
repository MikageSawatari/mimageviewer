//! Development-only GPU Lanczos3 spike for v2.11.0 stage 3.
//!
//! This module deliberately stops below product integration. It proves that a
//! display-sized Rgba8Unorm texture can be generated and registered with egui
//! while the original TextureHandle remains the owner of logical image
//! dimensions. The stage-4 cache/lifecycle/UI routing is intentionally absent.

use std::borrow::Cow;

use wgpu::util::DeviceExt as _;

pub const LANCZOS3_SHADER: &str = include_str!("gpu_lanczos_spike.wgsl");

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LanczosPlan {
    pub source_size: [u32; 2],
    pub target_size: [u32; 2],
    pub mip_level: u32,
    pub mip_source_size: [u32; 2],
    pub vertical_scale: f32,
    pub horizontal_scale: f32,
    pub vertical_max_taps: u32,
    pub horizontal_max_taps: u32,
    pub texture_fetches: u64,
}

impl LanczosPlan {
    pub fn new(
        source_size: [u32; 2],
        target_size: [u32; 2],
        use_mip_pre_shrink: bool,
    ) -> Result<Self, String> {
        if source_size.contains(&0) || target_size.contains(&0) {
            return Err("source and target dimensions must be non-zero".to_string());
        }
        if target_size[0] > source_size[0] || target_size[1] > source_size[1] {
            return Err("the stage-3 spike only supports downscaling".to_string());
        }

        let mip_level = if use_mip_pre_shrink {
            mip_pre_shrink_level(source_size, target_size)
        } else {
            0
        };
        let mip_source_size = mip_extent(source_size, mip_level);
        let horizontal_scale = target_size[0] as f32 / mip_source_size[0] as f32;
        let vertical_scale = target_size[1] as f32 / mip_source_size[1] as f32;
        if horizontal_scale > 1.001 || vertical_scale > 1.001 {
            return Err(format!(
                "mip level {mip_level} undershoots target: mip={mip_source_size:?} target={target_size:?}"
            ));
        }

        let horizontal_max_taps = max_lanczos_taps(mip_source_size[0], target_size[0]);
        let vertical_max_taps = max_lanczos_taps(mip_source_size[1], target_size[1]);
        let vertical_fetches = axis_fetch_count(mip_source_size[1], target_size[1])
            .saturating_mul(u64::from(mip_source_size[0]));
        let horizontal_fetches = axis_fetch_count(mip_source_size[0], target_size[0])
            .saturating_mul(u64::from(target_size[1]));

        Ok(Self {
            source_size,
            target_size,
            mip_level,
            mip_source_size,
            vertical_scale,
            horizontal_scale,
            vertical_max_taps,
            horizontal_max_taps,
            texture_fetches: vertical_fetches.saturating_add(horizontal_fetches),
        })
    }
}

/// L = floor(log2(1/s)), where s is the smaller per-axis scale.
pub fn mip_pre_shrink_level(source_size: [u32; 2], target_size: [u32; 2]) -> u32 {
    let scale_x = target_size[0].max(1) as f64 / source_size[0].max(1) as f64;
    let scale_y = target_size[1].max(1) as f64 / source_size[1].max(1) as f64;
    let scale = scale_x.min(scale_y).clamp(f64::MIN_POSITIVE, 1.0);
    let requested = (1.0 / scale).log2().floor().max(0.0) as u32;
    let max_level = egui_wgpu::mip_level_count(source_size[0], source_size[1]) - 1;
    requested.min(max_level)
}

pub fn mip_extent(mut size: [u32; 2], level: u32) -> [u32; 2] {
    for _ in 0..level {
        size[0] = (size[0] / 2).max(1);
        size[1] = (size[1] / 2).max(1);
    }
    size
}

fn sample_range(source_len: u32, target_len: u32, target_index: u32) -> (u32, u32) {
    let scale = target_len as f64 / source_len as f64;
    let filter_stretch = (1.0 / scale).max(1.0);
    let support = 3.0 * filter_stretch;
    let center = (target_index as f64 + 0.5) / scale;
    let start =
        ((center - 0.5 - support).floor() as i64 + 1).clamp(0, i64::from(source_len)) as u32;
    let end = ((center - 0.5 + support).ceil() as i64).clamp(0, i64::from(source_len)) as u32;
    (start, end.max(start))
}

fn max_lanczos_taps(source_len: u32, target_len: u32) -> u32 {
    (0..target_len)
        .map(|index| {
            let (start, end) = sample_range(source_len, target_len, index);
            end - start
        })
        .max()
        .unwrap_or(0)
}

fn axis_fetch_count(source_len: u32, target_len: u32) -> u64 {
    (0..target_len)
        .map(|index| {
            let (start, end) = sample_range(source_len, target_len, index);
            u64::from(end - start)
        })
        .sum()
}

pub struct Lanczos3Resampler {
    bind_group_layout: wgpu::BindGroupLayout,
    vertical_pipeline: wgpu::RenderPipeline,
    horizontal_pipeline: wgpu::RenderPipeline,
}

impl Lanczos3Resampler {
    pub fn new(device: &wgpu::Device) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mIV Lanczos spike bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mIV Lanczos spike pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mIV Lanczos spike shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(LANCZOS3_SHADER)),
        });
        let vertical_pipeline = create_pipeline(
            device,
            &pipeline_layout,
            &shader,
            "mIV Lanczos spike vertical pipeline",
            "fs_vertical",
            wgpu::TextureFormat::Rgba16Float,
        );
        let horizontal_pipeline = create_pipeline(
            device,
            &pipeline_layout,
            &shader,
            "mIV Lanczos spike horizontal pipeline",
            "fs_horizontal",
            wgpu::TextureFormat::Rgba8Unorm,
        );
        Self {
            bind_group_layout,
            vertical_pipeline,
            horizontal_pipeline,
        }
    }

    pub fn prepare_job(
        &self,
        device: &wgpu::Device,
        source_texture: &wgpu::Texture,
        plan: LanczosPlan,
    ) -> Result<LanczosJob, String> {
        if source_texture.format() != wgpu::TextureFormat::Rgba8Unorm {
            return Err(format!(
                "source texture must be Rgba8Unorm, got {:?}",
                source_texture.format()
            ));
        }
        if source_texture.width() != plan.source_size[0]
            || source_texture.height() != plan.source_size[1]
        {
            return Err(format!(
                "source texture {:?} does not match plan {:?}",
                [source_texture.width(), source_texture.height()],
                plan.source_size
            ));
        }
        if source_texture.mip_level_count() <= plan.mip_level {
            return Err(format!(
                "source has {} mip levels but plan requires level {}",
                source_texture.mip_level_count(),
                plan.mip_level
            ));
        }

        let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("mIV Lanczos spike selected mip"),
            base_mip_level: plan.mip_level,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let intermediate_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mIV Lanczos spike vertical intermediate"),
            size: wgpu::Extent3d {
                width: plan.mip_source_size[0],
                height: plan.target_size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let intermediate_view =
            intermediate_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mIV Lanczos spike output"),
            size: wgpu::Extent3d {
                width: plan.target_size[0],
                height: plan.target_size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let vertical_uniform = target_len_uniform(device, plan.target_size[1], "vertical");
        let horizontal_uniform = target_len_uniform(device, plan.target_size[0], "horizontal");
        let vertical_bind_group = create_bind_group(
            device,
            &self.bind_group_layout,
            &source_view,
            &vertical_uniform,
            "mIV Lanczos spike vertical bind group",
        );
        let horizontal_bind_group = create_bind_group(
            device,
            &self.bind_group_layout,
            &intermediate_view,
            &horizontal_uniform,
            "mIV Lanczos spike horizontal bind group",
        );

        Ok(LanczosJob {
            plan,
            intermediate_texture,
            intermediate_view,
            output_texture,
            output_view,
            vertical_bind_group,
            horizontal_bind_group,
            _vertical_uniform: vertical_uniform,
            _horizontal_uniform: horizontal_uniform,
        })
    }

    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        job: &LanczosJob,
        query_set: Option<&wgpu::QuerySet>,
        beginning_timestamp: Option<u32>,
        end_timestamp: Option<u32>,
    ) {
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mIV Lanczos spike vertical pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &job.intermediate_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: query_set.and_then(|query_set| {
                    beginning_timestamp.map(|index| wgpu::RenderPassTimestampWrites {
                        query_set,
                        beginning_of_pass_write_index: Some(index),
                        end_of_pass_write_index: None,
                    })
                }),
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.vertical_pipeline);
            pass.set_bind_group(0, &job.vertical_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mIV Lanczos spike horizontal pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &job.output_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: query_set.and_then(|query_set| {
                    end_timestamp.map(|index| wgpu::RenderPassTimestampWrites {
                        query_set,
                        beginning_of_pass_write_index: None,
                        end_of_pass_write_index: Some(index),
                    })
                }),
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.horizontal_pipeline);
            pass.set_bind_group(0, &job.horizontal_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

pub struct LanczosJob {
    pub plan: LanczosPlan,
    // Kept explicitly because the bind groups and repeated benchmark encodes use them.
    intermediate_texture: wgpu::Texture,
    intermediate_view: wgpu::TextureView,
    pub output_texture: wgpu::Texture,
    pub output_view: wgpu::TextureView,
    vertical_bind_group: wgpu::BindGroup,
    horizontal_bind_group: wgpu::BindGroup,
    _vertical_uniform: wgpu::Buffer,
    _horizontal_uniform: wgpu::Buffer,
}

impl LanczosJob {
    pub fn register_native_texture(
        &self,
        renderer: &mut egui_wgpu::Renderer,
        device: &wgpu::Device,
    ) -> egui::TextureId {
        renderer.register_native_texture(device, &self.output_view, wgpu::FilterMode::Linear)
    }

    pub fn intermediate_size(&self) -> [u32; 2] {
        [
            self.intermediate_texture.width(),
            self.intermediate_texture.height(),
        ]
    }
}

/// Public egui-wgpu API proof for C-1: managed level-0 data can be read without
/// replacing the logical TextureHandle that owns size_vec2().
pub fn managed_source_texture<'a>(
    renderer: &'a egui_wgpu::Renderer,
    id: &egui::TextureId,
) -> Option<&'a wgpu::Texture> {
    renderer.texture(id)?.texture.as_ref()
}

fn target_len_uniform(device: &wgpu::Device, target_len: u32, axis: &str) -> wgpu::Buffer {
    let mut bytes = [0_u8; 16];
    bytes[..4].copy_from_slice(&target_len.to_ne_bytes());
    bytes[4..8].copy_from_slice(&1.0_f32.to_ne_bytes());
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(&format!("mIV Lanczos spike {axis} uniform")),
        contents: &bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    })
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source_view: &wgpu::TextureView,
    uniform: &wgpu::Buffer,
    label: &str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uniform.as_entire_binding(),
            },
        ],
    })
}

fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    label: &str,
    fragment_entry: &str,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn target(source: [u32; 2], ratio: f64) -> [u32; 2] {
        [
            (source[0] as f64 * ratio) as u32,
            (source[1] as f64 * ratio) as u32,
        ]
    }

    #[test]
    fn mip_pre_shrink_matches_stage_three_formula() {
        let source = [2480, 3508];
        assert_eq!(mip_pre_shrink_level(source, target(source, 0.63)), 0);
        assert_eq!(mip_pre_shrink_level(source, target(source, 0.41)), 1);
        assert_eq!(mip_pre_shrink_level(source, target(source, 0.25)), 2);
    }

    #[test]
    fn mip_pre_shrink_bounds_lanczos_taps_and_fetches() {
        let source = [2480, 3508];
        for ratio in [0.63, 0.41, 0.25] {
            let plan = LanczosPlan::new(source, target(source, ratio), true).unwrap();
            assert!((0.5..=1.001).contains(&plan.horizontal_scale));
            assert!((0.5..=1.001).contains(&plan.vertical_scale));
            assert!(plan.horizontal_max_taps <= 12, "{plan:?}");
            assert!(plan.vertical_max_taps <= 12, "{plan:?}");
            assert!(plan.texture_fetches <= 120_000_000, "{plan:?}");
        }
    }

    #[test]
    fn fixed_six_taps_would_not_match_downscale_support() {
        let source = [2480, 3508];
        let direct = LanczosPlan::new(source, target(source, 0.41), false).unwrap();
        assert!(direct.horizontal_max_taps >= 15, "{direct:?}");
        assert!(direct.vertical_max_taps >= 15, "{direct:?}");
    }

    #[test]
    fn shader_parses_and_validates() {
        let module = wgpu::naga::front::wgsl::parse_str(LANCZOS3_SHADER)
            .expect("parse GPU Lanczos spike WGSL");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("validate GPU Lanczos spike WGSL");
    }
}
