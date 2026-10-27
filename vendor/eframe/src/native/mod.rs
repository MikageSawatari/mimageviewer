mod app_icon;
mod epi_integration;
mod event_loop_context;
mod frame_output;
pub mod run;
mod viewport_lifecycle;

#[cfg(target_os = "macos")]
pub(crate) mod macos;

/// File storage which can be used by native backends.
#[cfg(feature = "persistence")]
pub mod file_storage;

pub(crate) mod winit_integration;

#[cfg(feature = "glow")]
mod glow_integration;

#[cfg(feature = "wgpu_no_default_features")]
mod wgpu_integration;

#[cfg(feature = "miv-test-script-window-witness")]
pub use epi_integration::HeadlessEpiIntegration;
