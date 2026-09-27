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
        pub fn miv_raw_copy_rgb_adjusted(
            handle: *mut c_void,
            data: *mut u8,
            stride: usize,
            bright_mode: c_int,
            gain: f32,
        ) -> c_int;
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

        fn open_bytes(bytes: &[u8]) -> Result<Self, i32> {
            let handle = unsafe { miv_raw_new() };
            assert!(!handle.is_null());
            let code = unsafe { miv_raw_open_buffer(handle, bytes.as_ptr(), bytes.len()) };
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

    #[test]
    fn oversized_recorded_thumbnail_is_rejected_before_allocation() {
        let mut bytes = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/raw-samples/2756.dng"),
        )
        .expect("Run scripts/setup-raw-samples.ps1");
        // Ricoh GXR's big-endian SubIFD preview has StripByteCounts = 56,811.
        let tag = [0x01, 0x17, 0x00, 0x04, 0, 0, 0, 1, 0, 0, 0xdd, 0xeb];
        let offset = bytes
            .windows(tag.len())
            .position(|part| part == tag)
            .unwrap();
        let oversized = 512u32 * 1024 * 1024 + 1;
        bytes[offset + 8..offset + 12].copy_from_slice(&oversized.to_be_bytes());
        let handle = Handle::open_bytes(&bytes).expect("patched DNG should still open");
        let mut oversized_index = None;
        for index in 0..8 {
            let mut info = MivRawPreviewInfo {
                format: 0,
                width: 0,
                height: 0,
                tflip: 0,
                length: 0,
            };
            if unsafe { miv_raw_preview_info(handle.0, index, &mut info) } == 0
                && info.length == oversized
            {
                oversized_index = Some(index);
                break;
            }
        }
        let index = oversized_index.expect("LibRaw should expose the crafted length");
        let mut output = MivRawBuffer {
            data: std::ptr::null_mut(),
            length: 0,
            width: 0,
            height: 0,
            colors: 0,
            format: 0,
        };
        assert_eq!(
            unsafe { miv_raw_preview_extract(handle.0, index, &mut output) },
            -100012
        );
        assert!(output.data.is_null());
        assert_eq!(output.length, 0);
    }

    #[test]
    fn callback_interrupts_libraw_before_output_exists() {
        unsafe extern "C" fn cancel_at_interpolate(
            user: *mut c_void,
            stage: i32,
            _iteration: i32,
            _expected: i32,
        ) -> i32 {
            if stage == 1 << 11 {
                unsafe { *(user as *mut bool) = true };
                1
            } else {
                0
            }
        }
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/raw-samples/1018.cr2");
        let handle = Handle::open(&path).expect("Run scripts/setup-raw-samples.ps1");
        let mut reached_interpolate = false;
        let code = unsafe {
            miv_raw_develop(
                handle.0,
                0,
                0,
                Some(cancel_at_interpolate),
                (&mut reached_interpolate as *mut bool).cast(),
            )
        };
        assert!(reached_interpolate, "callback must reach INTERPOLATE");
        assert_eq!(code, -100010, "LibRaw must report cancellation itself");
        let mut width = 0;
        let mut height = 0;
        assert_ne!(
            unsafe { miv_raw_image_info(handle.0, &mut width, &mut height) },
            0
        );
        assert_eq!((width, height), (0, 0));
    }
}
