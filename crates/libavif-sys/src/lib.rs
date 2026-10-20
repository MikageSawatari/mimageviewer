//! Opaque libavif decoder and the small, project-owned shim ABI.
pub const BUILD_ID: &str = env!("MIV_LIBAVIF_BUILD_ID");
#[cfg(windows)]
mod windows {
    use std::ffi::{c_char, c_int, c_void};

    #[repr(C)]
    #[derive(Default)]
    pub struct Info {
        pub width: u32,
        pub height: u32,
        pub depth: u32,
        pub alpha: u32,
        pub crop_x: u32,
        pub crop_y: u32,
        pub crop_width: u32,
        pub crop_height: u32,
        pub rotation: u32,
        pub mirror: u32,
    }

    unsafe extern "C" {
        pub fn miv_avif_new(dimension: u32, pixels: u32, threads: c_int) -> *mut c_void;
        pub fn miv_avif_parse(
            handle: *mut c_void,
            data: *const u8,
            length: usize,
            info: *mut Info,
        ) -> c_int;
        pub fn miv_avif_decode(
            handle: *mut c_void,
            pixels: *mut u8,
            length: usize,
            stride: u32,
            stage: *mut c_int,
        ) -> c_int;
        pub fn miv_avif_error(handle: *mut c_void, code: c_int, out: *mut c_char, length: usize);
        pub fn miv_avif_close(handle: *mut c_void);
    }
}
#[cfg(windows)]
pub use windows::*;
