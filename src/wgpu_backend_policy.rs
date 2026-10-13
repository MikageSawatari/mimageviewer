//! Backend set requested by the native main GUI (before adapter selection).

pub(crate) fn native_backends() -> wgpu::Backends {
    wgpu::Backends::DX12 | wgpu::Backends::VULKAN
}
