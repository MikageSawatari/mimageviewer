# egui-wgpu

[![Latest version](https://img.shields.io/crates/v/egui-wgpu.svg)](https://crates.io/crates/egui-wgpu)
[![Documentation](https://docs.rs/egui-wgpu/badge.svg)](https://docs.rs/egui-wgpu)
![MIT](https://img.shields.io/badge/license-MIT-blue.svg)
![Apache](https://img.shields.io/badge/license-Apache-blue.svg)

This crate provides bindings between [`egui`](https://github.com/emilk/egui) and [wgpu](https://crates.io/crates/wgpu).

This was originally hosted at https://github.com/hasenbanck/egui_wgpu_backend

## mImageViewer patch

This directory is based on `egui-wgpu` 0.33.3. The local patch adds opt-in mipmap allocation,
GPU mip-chain generation, and mipmap sampler filtering for managed textures whose
`TextureOptions::mipmap_mode` is set. See `docs/downscale-moire-lod-plan.md` in the application
repository for the behavior and maintenance notes.
