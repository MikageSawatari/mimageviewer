#![allow(non_camel_case_types)]

#[cfg(windows)]
mod windows {
    use std::ffi::{c_char, c_int, c_void};

    #[repr(C)]
    pub struct MivRawInfo {
        pub width: u32,
        pub height: u32,
        pub flip: u32,
        pub unsupported: c_int,
        pub make: [c_char; 128],
        pub model: [c_char; 128],
        pub preview_count: u32,
    }

    #[repr(C)]
    pub struct MivRawPreviewInfo {
        pub format: u32,
        pub width: u32,
        pub height: u32,
        pub tflip: u32,
        pub length: u32,
    }

    #[repr(C)]
    pub struct MivRawBuffer {
        pub data: *mut u8,
        pub length: usize,
        pub width: u32,
        pub height: u32,
        pub colors: u32,
        pub format: u32,
    }

    pub type ProgressCallback = unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int) -> c_int;

    unsafe extern "C" {
        pub fn miv_raw_new() -> *mut c_void;
        pub fn miv_raw_open_path(handle: *mut c_void, path: *const u16) -> c_int;
        pub fn miv_raw_open_buffer(handle: *mut c_void, data: *const u8, length: usize) -> c_int;
        pub fn miv_raw_info(handle: *mut c_void, info: *mut MivRawInfo) -> c_int;
        pub fn miv_raw_preview_info(
            handle: *mut c_void,
            index: u32,
            info: *mut MivRawPreviewInfo,
        ) -> c_int;
        pub fn miv_raw_preview_extract(
            handle: *mut c_void,
            index: u32,
            result: *mut MivRawBuffer,
        ) -> c_int;
        pub fn miv_raw_develop(
            handle: *mut c_void,
            half: c_int,
            bright_mode: c_int,
            callback: Option<ProgressCallback>,
            user: *mut c_void,
        ) -> c_int;
        pub fn miv_raw_image_info(handle: *mut c_void, width: *mut u32, height: *mut u32) -> c_int;
        pub fn miv_raw_copy_rgb(handle: *mut c_void, data: *mut u8, stride: usize) -> c_int;
        pub fn miv_raw_set_cancel_flag(handle: *mut c_void) -> c_int;
        pub fn miv_raw_free(data: *mut u8) -> c_int;
        pub fn miv_raw_close(handle: *mut c_void) -> c_int;
    }
}

#[cfg(windows)]
pub use windows::*;

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    struct Handle(*mut c_void);
    impl Handle {
        fn open(path: &Path) -> Result<Self, i32> {
            let handle = unsafe { miv_raw_new() };
            assert!(!handle.is_null());
            let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
            wide.push(0);
            let code = unsafe { miv_raw_open_path(handle, wide.as_ptr()) };
            if code != 0 {
                unsafe { miv_raw_close(handle) };
                return Err(code);
            }
            Ok(Self(handle))
        }
    }
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { miv_raw_close(self.0) };
        }
    }

    #[test]
    fn all_cc0_sample_dimensions_match() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/raw-samples");
        assert!(
            manifest.join("1023.dng").is_file(),
            "Run scripts/setup-raw-samples.ps1"
        );
        for entry in std::fs::read_dir(manifest).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("part") {
                continue;
            }
            let info_handle =
                Handle::open(&path).unwrap_or_else(|code| panic!("{} open {code}", path.display()));
            let mut info = MivRawInfo {
                width: 0,
                height: 0,
                flip: 0,
                unsupported: 0,
                make: [0; 128],
                model: [0; 128],
                preview_count: 0,
            };
            let code = unsafe { miv_raw_info(info_handle.0, &mut info) };
            assert_eq!(code, 0, "{} info", path.display());
            if info.unsupported != 0 {
                continue;
            }
            let develop_handle = Handle::open(&path).unwrap();
            let code =
                unsafe { miv_raw_develop(develop_handle.0, 0, 0, None, std::ptr::null_mut()) };
            assert_eq!(code, 0, "{} develop", path.display());
            let mut width = 0;
            let mut height = 0;
            let code = unsafe { miv_raw_image_info(develop_handle.0, &mut width, &mut height) };
            assert_eq!(code, 0, "{} output", path.display());
            assert_eq!(
                (width, height),
                (info.width, info.height),
                "{} dimensions",
                path.display()
            );
        }
    }
}
