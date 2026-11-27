//! Immutable per-material color, per-callback uniforms, and the shared screen/export shader.
use crate::displayed_image_transform::DisplayedImageTransform;
use crate::plane_view::{PlaneLensKind, PlaneTilt, PlaneViewAnchor};
use egui::{Rect, Vec2};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

mod export;
#[cfg(test)]
mod gpu_tests;
pub(crate) use export::{PlaneFrameRenderer, PlaneOutputChoice, resolve_plane_output_size};

pub(crate) const PLANE_SHADER: &str = r#"
struct Vertex { @builtin(position) position:vec4<f32> };
@vertex fn vs_main(@builtin(vertex_index) i:u32)->Vertex {
    var p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
    var o:Vertex; o.position=vec4(p[i],0.,1.); return o;
}
struct Params {
    canvas:vec4<f32>, lens:vec4<f32>, camera:vec4<f32>, paint:vec4<f32>, frame:vec4<f32>,
    crop:vec4<f32>, coverage:vec4<f32>, material:vec4<f32>, mapping:vec4<f32>,
    underlay:vec4<f32>, checker:vec4<f32>, background:vec4<f32>, view:vec4<f32>,
};
@group(0) @binding(0) var color:texture_2d<f32>;
@group(0) @binding(1) var samp:sampler;
@group(1) @binding(0) var<uniform> params:Params;
fn inside(p:vec2<f32>, lo:vec2<f32>, hi:vec2<f32>)->bool { return all(p>=lo) && all(p<=hi); }
fn rotate_uv(p:vec2<f32>)->vec2<f32> {
    let q=u32(params.view.x+0.5)%4u;
    if q==1u {return vec2(p.y,1.-p.x);}
    if q==2u {return vec2(1.-p.x,1.-p.y);}
    if q==3u {return vec2(1.-p.y,p.x);}
    return p;
}
@fragment fn fs_main(in:Vertex)->@location(0) vec4<f32> {
    let p=params.mapping.xy+in.position.xy*params.mapping.zw;
    let c=params.canvas.xy+params.lens.xy*params.canvas.zw;
    let radius=0.5*min(params.canvas.z,params.canvas.w);
    let u=(p-c)/radius; let r=length(u); let k=params.camera.x;
    let kr=k*r;
    var theta=kr; var valid=true;
    if params.lens.w>0.5 && params.lens.w<1.5 { theta=2.*atan(0.5*kr); }
    if params.lens.w>1.5 {
        valid=kr<=2.; theta=2.*asin(clamp(0.5*kr,0.,1.));
    } else { valid=theta<3.141592653589793; }
    theta=select(0.,theta,valid);
    let sigma=select(sin(theta)/max(r,1.e-6),k,r<1.e-6);
    let d=vec3(sigma*u,cos(theta));
    let sy=sin(params.camera.y); let cy=cos(params.camera.y);
    let sp=sin(params.camera.z); let cp=cos(params.camera.z);
    let nx=vec3(cy,0.,-sy); let ny=vec3(sy*sp,cp,cy*sp); let n=vec3(sy*cp,-sp,cy*cp);
    let den=dot(n,d); valid=valid && den>1.e-6;
    let t=n.z/max(den,1.e-6); valid=valid && t>0.;
    let v=max(t,0.)*d-vec3(0.,0.,1.);
    let local_board=vec2(dot(nx,v),dot(ny,v));
    var q=c+(radius/max(k,1.e-7))*local_board;
    if k<=1.e-7 {q=p;}
    let delta=q-params.frame.xy;
    let local=vec2(params.frame.z*delta.x+params.frame.w*delta.y,-params.frame.w*delta.x+params.frame.z*delta.y);
    let in_uv=(params.frame.xy+local-params.paint.xy)/params.paint.zw;
    let source_uv=mix(params.crop.xy,params.crop.zw,rotate_uv(in_uv));
    let tex_uv=(source_uv-params.material.xy)/params.material.zw;
    // Derivatives and the only sample are evaluated uniformly, even on rejected rays.
    let dx=dpdx(tex_uv); let dy=dpdy(tex_uv);
    let sample_color=textureSampleGrad(color,samp,tex_uv,dx,dy);
    valid=valid && inside(in_uv,vec2(0.),vec2(1.))
        && inside(source_uv,params.coverage.xy,params.coverage.zw)
        && inside(tex_uv,vec2(0.),vec2(1.));
    let check=floor((p-params.checker.xy)/params.checker.zw);
    let odd=((check.x+check.y)%2.+2.)%2.;
    let gray=select(224./255.,176./255.,odd>=1.);
    let under=select(params.underlay.xyz,vec3(gray),params.underlay.w>0.5);
    let pixel=vec4(sample_color.rgb+under*(1.-sample_color.a),1.);
    let outside=select(vec4(0.),vec4(params.background.xyz,1.),params.camera.w>0.5);
    return select(outside,pixel,valid);
}
"#;

// Count allocation lifetime, including requests surviving the interactive state.
static LIVE_COLOR_BYTES: AtomicU64 = AtomicU64::new(0);
pub(crate) fn resident_bytes() -> u64 {
    LIVE_COLOR_BYTES.load(Ordering::Relaxed)
}
struct PlaneAllocation(u64);
impl PlaneAllocation {
    fn new(bytes: u64) -> Self {
        LIVE_COLOR_BYTES.fetch_add(bytes, Ordering::Relaxed);
        Self(bytes)
    }
}
impl Drop for PlaneAllocation {
    fn drop(&mut self) {
        LIVE_COLOR_BYTES.fetch_sub(self.0, Ordering::Relaxed);
    }
}

pub(crate) struct PlaneGpuResources {
    // Explicitly retain the copied texture for the material allocation lifetime.
    pub _color: crate::gpu_color_copy::MipColor,
    pub bind: wgpu::BindGroup,
}
pub(crate) struct PlaneGpuCopy {
    pub texture_id: egui::TextureId,
    pub gpu: PlaneGpuResources,
    _allocation: PlaneAllocation,
}
pub(crate) struct PlaneMaterial {
    pub copy: Arc<PlaneGpuCopy>,
    pub description: PlaneMaterialDescription,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlaneMaterialDescription {
    pub source_size: [usize; 2],
    pub texture_size: Vec2,
    pub coverage: Rect,
    pub sampler_roi: Rect,
    pub display_basis: crate::material_display_basis::MaterialDisplayBasis,
    pub material_kind: crate::gpu_lanczos::FullscreenPageMaterialKind,
}
impl PlaneMaterial {
    pub(crate) fn with_description(
        copy: Arc<PlaneGpuCopy>,
        description: PlaneMaterialDescription,
    ) -> Arc<Self> {
        Arc::new(Self { copy, description })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PlaneRenderParams {
    pub values: [[f32; 4]; 13],
}
impl PlaneRenderParams {
    pub(crate) fn screen(
        transform: &DisplayedImageTransform,
        canvas: Rect,
        center: Vec2,
        strength: f32,
        lens: PlaneLensKind,
        tilt: PlaneTilt,
        material: PlaneMaterialDescription,
        underlay: &crate::ui_fullscreen::FsBgStyle,
        background: egui::Color32,
    ) -> Self {
        let ppp = transform.pixels_per_point;
        let paint = transform.paint_rect * ppp;
        let frame = transform.full_image_rect.center() * ppp;
        let (sin, cos) = transform.free_rotation_rad.sin_cos();
        let coverage = material.coverage;
        let roi = material.sampler_roi;
        let source = Rect::from_min_max(
            coverage.min + coverage.size() * roi.min.to_vec2(),
            coverage.min + coverage.size() * roi.max.to_vec2(),
        );
        let (rgb, kind, cell) = match underlay {
            crate::ui_fullscreen::FsBgStyle::Solid(color) => {
                (color.to_array(), 0., Vec2::splat(1.))
            }
            crate::ui_fullscreen::FsBgStyle::Checker(texture) => (
                [0; 4],
                1.,
                Vec2::new(texture.size()[0] as f32, texture.size()[1] as f32) * ppp * 0.5,
            ),
        };
        let crop = transform.uv_rect;
        let canvas = canvas * ppp;
        let started = crate::perf::is_enabled().then(std::time::Instant::now);
        let k_fill = crate::plane_view::plane_effective_quad(transform, material)
            .map(|quad| {
                crate::plane_view::plane_fill_coefficient(
                    canvas,
                    canvas.min + center * canvas.size(),
                    quad.map(|p| p * ppp),
                    lens,
                )
            })
            .unwrap_or(0.);
        if let Some(started) = started {
            crate::perf::event(
                "plane_view",
                "fill_coefficient",
                None,
                0,
                &[("ms", (started.elapsed().as_secs_f64() * 1000.).into())],
            );
        }
        let bg = background.to_array();
        Self {
            values: [
                [canvas.min.x, canvas.min.y, canvas.width(), canvas.height()],
                [center.x, center.y, strength, lens.shader_id()],
                [strength * k_fill, tilt.yaw, tilt.pitch, 0.],
                [paint.min.x, paint.min.y, paint.width(), paint.height()],
                [frame.x, frame.y, cos, sin],
                [crop.min.x, crop.min.y, crop.max.x, crop.max.y],
                [
                    coverage.min.x,
                    coverage.min.y,
                    coverage.max.x,
                    coverage.max.y,
                ],
                [source.min.x, source.min.y, source.width(), source.height()],
                [0., 0., 1., 1.],
                [
                    rgb[0] as f32 / 255.,
                    rgb[1] as f32 / 255.,
                    rgb[2] as f32 / 255.,
                    kind,
                ],
                [paint.min.x, paint.min.y, cell.x, cell.y],
                [
                    bg[0] as f32 / 255.,
                    bg[1] as f32 / 255.,
                    bg[2] as f32 / 255.,
                    1.,
                ],
                [transform.rotation.degrees() as f32 / 90., 0., 0., 0.],
            ],
        }
    }
    fn bytes(&self) -> Vec<u8> {
        self.values
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect()
    }
    pub(crate) fn canvas(&self) -> Rect {
        let v = self.values[0];
        Rect::from_min_size(egui::pos2(v[0], v[1]), egui::vec2(v[2], v[3]))
    }
    pub(crate) fn for_output(mut self, size: [u32; 2]) -> Self {
        let c = self.canvas();
        self.values[8] = [
            c.min.x,
            c.min.y,
            c.width() / size[0] as f32,
            c.height() / size[1] as f32,
        ];
        self.values[2][3] = 1.;
        self
    }
}
pub(crate) struct PlaneFrameSnapshot {
    pub context: crate::app::ViewerContextId,
    pub anchor: PlaneViewAnchor,
    pub material: Arc<PlaneMaterial>,
    pub params: PlaneRenderParams,
}
#[derive(Clone)]
pub struct PlaneFrameRequest {
    pub(crate) snapshot: Arc<PlaneFrameSnapshot>,
    pub(crate) size: [u32; 2],
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pub(crate) pipeline: Arc<PlanePipeline>,
}
impl PlaneFrameRequest {
    pub(crate) fn with_choice(&self, choice: PlaneOutputChoice) -> Self {
        let mut r = self.clone();
        r.size = resolve_plane_output_size(
            self.snapshot.params.canvas().size(),
            choice,
            &self.device.limits(),
        );
        r
    }
    pub(crate) fn render(&self) -> Result<Vec<u8>, String> {
        PlaneFrameRenderer::new(self).render()
    }
    pub(crate) fn color_image(&self) -> Result<egui::ColorImage, String> {
        let rgba = self.render()?;
        Ok(egui::ColorImage::from_rgba_unmultiplied(
            [self.size[0] as usize, self.size[1] as usize],
            &rgba,
        ))
    }
}

fn texture_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}
pub(crate) struct PlanePipeline {
    #[cfg(test)]
    pub format: wgpu::TextureFormat,
    pub pipeline: wgpu::RenderPipeline,
    material_layout: wgpu::BindGroupLayout,
    uniform_layout: wgpu::BindGroupLayout,
}
impl PlanePipeline {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("plane material"),
            entries: &[
                texture_entry(),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("plane frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("plane camera"),
            source: wgpu::ShaderSource::Wgsl(PLANE_SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("plane camera"),
            bind_group_layouts: &[Some(&material_layout), Some(&uniform_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("plane camera"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            #[cfg(test)]
            format,
            pipeline,
            material_layout,
            uniform_layout,
        }
    }
}
#[derive(Default)]
pub(crate) struct PlaneRendererCache {
    pipelines: std::collections::HashMap<wgpu::TextureFormat, Arc<PlanePipeline>>,
    copier: Option<crate::gpu_color_copy::ColorMipCopier>,
}
impl PlaneRendererCache {
    pub(crate) fn pipeline(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> Arc<PlanePipeline> {
        self.pipelines
            .entry(format)
            .or_insert_with(|| Arc::new(PlanePipeline::new(device, format)))
            .clone()
    }
    pub(crate) fn copy(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture_id: egui::TextureId,
        source: &wgpu::Texture,
        anisotropic: bool,
    ) -> Result<Arc<PlaneGpuCopy>, String> {
        let started = crate::perf::is_enabled().then(std::time::Instant::now);
        let copier = self
            .copier
            .get_or_insert_with(|| crate::gpu_color_copy::ColorMipCopier::new(device));
        let color = copier.copy_color(device, queue, source)?;
        let pipeline = self.pipeline(device, wgpu::TextureFormat::Rgba8Unorm);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("plane trilinear"),
            anisotropy_clamp: if anisotropic { 16 } else { 1 },
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("plane immutable material"),
            layout: &pipeline.material_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let bytes = color.byte_size();
        if let Some(started) = started {
            crate::perf::event(
                "plane_view",
                "color_copy",
                None,
                0,
                &[
                    ("ms", (started.elapsed().as_secs_f64() * 1000.).into()),
                    ("bytes", bytes.into()),
                ],
            );
        }
        Ok(Arc::new(PlaneGpuCopy {
            texture_id,
            gpu: PlaneGpuResources {
                _color: color,
                bind,
            },
            _allocation: PlaneAllocation::new(bytes),
        }))
    }
}
struct PlaneUniform {
    buffer: wgpu::Buffer,
    bind: wgpu::BindGroup,
}
impl PlaneUniform {
    fn new(device: &wgpu::Device, pipeline: &PlanePipeline, params: PlaneRenderParams) -> Self {
        use wgpu::util::DeviceExt;
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("plane private frame"),
            contents: &params.bytes(),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("plane private frame"),
            layout: &pipeline.uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self { buffer, bind }
    }
}
pub(crate) struct PlaneShaderCallback {
    pub snapshot: Arc<PlaneFrameSnapshot>,
    pipeline: Arc<PlanePipeline>,
    uniform: PlaneUniform,
}
impl PlaneShaderCallback {
    pub(crate) fn new(
        device: &wgpu::Device,
        pipeline: Arc<PlanePipeline>,
        snapshot: Arc<PlaneFrameSnapshot>,
    ) -> Self {
        let uniform = PlaneUniform::new(device, &pipeline, snapshot.params);
        Self {
            snapshot,
            pipeline,
            uniform,
        }
    }
}
impl egui_wgpu::CallbackTrait for PlaneShaderCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        queue.write_buffer(&self.uniform.buffer, 0, &self.snapshot.params.bytes());
        Vec::new()
    }
    fn paint(
        &self,
        _info: egui::epaint::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        pass.set_pipeline(&self.pipeline.pipeline);
        pass.set_bind_group(0, &self.snapshot.material.copy.gpu.bind, &[]);
        pass.set_bind_group(1, &self.uniform.bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plane_shared_wgsl_parses_and_validates() {
        let module =
            wgpu::naga::front::wgsl::parse_str(PLANE_SHADER).expect("shared PlaneView WGSL");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
    #[test]
    fn plane_output_resolver_preserves_aspect_and_texture_and_padded_buffer_limits() {
        for canvas in [
            Vec2::new(1200., 900.),
            Vec2::new(317., 409.),
            Vec2::new(17000., 9000.),
        ] {
            for texture_limit in [256, 2048, 8192] {
                for buffer_limit in [256 * 256, 16 * 1024 * 1024, 256 * 1024 * 1024] {
                    let limits = wgpu::Limits {
                        max_texture_dimension_2d: texture_limit,
                        max_buffer_size: buffer_limit,
                        ..Default::default()
                    };
                    for choice in [
                        PlaneOutputChoice::Canvas,
                        PlaneOutputChoice::Export(crate::export_dialog::ExportScale::Full),
                        PlaneOutputChoice::Export(crate::export_dialog::ExportScale::Half),
                        PlaneOutputChoice::Export(crate::export_dialog::ExportScale::Quarter),
                        PlaneOutputChoice::Export(crate::export_dialog::ExportScale::LongEdge(
                            1000,
                        )),
                    ] {
                        let [w, h] = resolve_plane_output_size(canvas, choice, &limits);
                        assert!(w > 0 && h > 0 && w.max(h) <= texture_limit);
                        assert!(
                            u64::from((w * 4).div_ceil(256) * 256) * u64::from(h) <= buffer_limit
                        );
                        assert!((w as f32 / h as f32 - canvas.x / canvas.y).abs() < 0.03);
                    }
                }
            }
        }
    }
    #[test]
    fn plane_output_pixel_centers_and_checker_mapping_are_in_canvas_coordinates() {
        let mut params = PlaneRenderParams {
            values: [[0.; 4]; 13],
        };
        params.values[0] = [21., 37., 800., 600.];
        for size in [[800, 600], [1600, 1200], [400, 300]] {
            let mapped = params.for_output(size).values[8];
            let p = egui::pos2(mapped[0] + 0.5 * mapped[2], mapped[1] + 0.5 * mapped[3]);
            assert_eq!(
                p,
                egui::pos2(21. + 400. / size[0] as f32, 37. + 300. / size[1] as f32)
            );
            let center = egui::pos2(
                mapped[0] + (size[0] as f32 / 2.) * mapped[2],
                mapped[1] + (size[1] as f32 / 2.) * mapped[3],
            );
            assert_eq!(center, egui::pos2(421., 337.));
        }
    }
}
