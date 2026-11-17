//! Shader creation for the fixed-layout static-image NIS and Anime4K VL paths.
use std::borrow::Cow;

struct Bytecode {
    entry: &'static str,
    bytes: &'static [u8],
}

struct BytecodeSet {
    vertex: Bytecode,
    fragments: &'static [(&'static str, Bytecode)],
}

#[cfg(windows)]
include!(concat!(env!("OUT_DIR"), "/still_image_bytecode.rs"));

pub(crate) enum Family {
    Nis,
    Anime4kVl,
}

impl Family {
    fn bytecode(self) -> Option<&'static BytecodeSet> {
        #[cfg(windows)]
        {
            // Same flag policy as root egui-wgpu's instance descriptor. Both FXC
            // variants are built, independent of the Cargo optimization profile.
            let debug = wgpu::InstanceFlags::from_build_config()
                .with_env()
                .contains(wgpu::InstanceFlags::DEBUG);
            Some(match (self, debug) {
                (Self::Nis, false) => &NIS_RELEASE,
                (Self::Nis, true) => &NIS_DEBUG,
                (Self::Anime4kVl, false) => &ANIME4K_VL_RELEASE,
                (Self::Anime4kVl, true) => &ANIME4K_VL_DEBUG,
            })
        }
        #[cfg(not(windows))]
        {
            None
        }
    }
}

struct Stage {
    module: wgpu::ShaderModule,
    entry: &'static str,
}

pub(crate) struct ResamplerShaders {
    modules: ShaderModules,
}

enum ShaderModules {
    Wgsl(wgpu::ShaderModule),
    Dxbc {
        vertex: Stage,
        fragments: Vec<(&'static str, Stage)>,
    },
}

/// Requesting this opt-in only permits the explicit unsafe passthrough API;
/// ordinary create_shader_module still parses/validates/compiles WGSL as before.
pub(crate) fn request_passthrough(features: wgpu::Features) -> wgpu::Features {
    features & wgpu::Features::PASSTHROUGH_SHADERS
}

impl ResamplerShaders {
    #[cfg(test)]
    pub(crate) fn uses_dxbc(&self) -> bool {
        matches!(&self.modules, ShaderModules::Dxbc { .. })
    }

    pub(crate) fn new(device: &wgpu::Device, source: &'static str, family: Option<Family>) -> Self {
        let bytecode = family.and_then(Family::bytecode).filter(|_| {
            device.adapter_info().backend == wgpu::Backend::Dx12
                && device
                    .features()
                    .contains(wgpu::Features::PASSTHROUGH_SHADERS)
        });
        // The backend/feature choice lives here only; no retry or error fallback.
        if let Some(bytecode) = bytecode {
            Self {
                modules: ShaderModules::Dxbc {
                    vertex: passthrough(device, &bytecode.vertex),
                    fragments: bytecode
                        .fragments
                        .iter()
                        .map(|(name, blob)| (*name, passthrough(device, blob)))
                        .collect(),
                },
            }
        } else {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("static-image WGSL"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(source)),
            });
            Self {
                modules: ShaderModules::Wgsl(module),
            }
        }
    }

    pub(crate) fn create_pipeline(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        entry: &str,
        format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        let (vertex, vertex_entry, fragment, entry) = match &self.modules {
            ShaderModules::Wgsl(module) => (module, "vs_main", module, entry),
            ShaderModules::Dxbc { vertex, fragments } => {
                let (_, stage) = fragments
                    .iter()
                    .find(|(name, _)| *name == entry)
                    .expect("generated static-image fragment");
                (&vertex.module, vertex.entry, &stage.module, stage.entry)
            }
        };
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(entry),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: vertex,
                entry_point: Some(vertex_entry),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: fragment,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }
}

fn passthrough(device: &wgpu::Device, blob: &Bytecode) -> Stage {
    // SAFETY: build.rs compiles trusted WGSL with the exact fixed layout's HAL
    // register/special-constant mapping, SM5.1 and FXC flags. Explicit pipeline
    // layouts and the permanent WGSL/DXBC GPU parity tests enforce this contract.
    // wgpu-hal 30.0.1 passes dxil bytes unchanged to D3D12 (also accepts DXBC).
    // Its public API documents DXIL: recheck this implementation on wgpu updates.
    let module = unsafe {
        device.create_shader_module_passthrough(wgpu::ShaderModuleDescriptorPassthrough {
            label: Some(blob.entry),
            entry_points: Cow::Owned(vec![wgpu::PassthroughShaderEntryPoint {
                name: Cow::Borrowed(blob.entry),
                workgroup_size: (0, 0, 0),
            }]),
            dxil: Some(Cow::Borrowed(blob.bytes)),
            ..Default::default()
        })
    };
    Stage {
        module,
        entry: blob.entry,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn still_bytecode_contract_tracks_locked_hal_and_naga() {
        // An upstream update requires an explicit contract review even when a
        // particular GPU happens to render the old generator's output equally.
        let lock: toml_edit::DocumentMut = include_str!("../Cargo.lock").parse().unwrap();
        let packages = lock["package"].as_array_of_tables().unwrap();
        for name in ["wgpu", "wgpu-hal", "wgpu-core"] {
            let matches = packages
                .iter()
                .filter(|p| p["name"].as_str() == Some(name))
                .collect::<Vec<_>>();
            assert_eq!(
                matches.len(),
                1,
                "review all wgpu graphs before changing the DXBC contract"
            );
            assert_eq!(
                matches[0]["version"].as_str(),
                Some("30.0.1"),
                "wgpu update: recheck static-image DXBC binding, HLSL Options, FXC and passthrough"
            );
            assert!(
                matches[0]["dependencies"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|d| d.as_str() == Some("naga 30.0.1")),
                "{name}: runtime naga must match the generator"
            );
        }
        let manifest: toml_edit::DocumentMut = include_str!("../Cargo.toml").parse().unwrap();
        assert_eq!(
            manifest["build-dependencies"]["naga_wgpu"]["version"].as_str(),
            Some("=30.0.1")
        );
    }
}
