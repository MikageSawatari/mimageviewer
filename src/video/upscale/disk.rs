use std::io;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiskSpaceEstimate {
    pub estimated_video_bytes: u64,
    pub required_bytes: u64,
    pub multiplier: f64,
}

pub fn estimate_required_bytes(
    output_width: u32,
    output_height: u32,
    estimated_frames: u64,
    bits_per_pixel_per_frame: f64,
    uses_two_step_fallback: bool,
) -> DiskSpaceEstimate {
    let pixels = output_width as f64 * output_height as f64;
    let estimated_video_bytes =
        (pixels * estimated_frames as f64 * bits_per_pixel_per_frame / 8.0).ceil() as u64;
    let multiplier = if uses_two_step_fallback { 1.5 } else { 1.25 };
    let required_bytes = (estimated_video_bytes as f64 * multiplier).ceil() as u64;
    DiskSpaceEstimate {
        estimated_video_bytes,
        required_bytes,
        multiplier,
    }
}

/// Returns available bytes on the drive containing `path`.
///
/// Callers should prefer passing the source file's parent directory or an existing output
/// directory. If `path` is a file or a not-yet-created work directory, this function queries its
/// parent directory.
#[cfg(windows)]
pub fn free_bytes_available(path: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::PCWSTR;

    let query_path = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().unwrap_or(path).to_path_buf()
    };
    let wide: Vec<u16> = query_path.as_os_str().encode_wide().chain([0]).collect();
    let mut free: u64 = 0;
    unsafe {
        GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None)
            .map_err(|e| io::Error::other(e.to_string()))?;
    }
    Ok(free)
}

#[cfg(not(windows))]
pub fn free_bytes_available(_path: &Path) -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "disk free-space query is implemented for Windows",
    ))
}

pub fn has_enough_space(path: &Path, required_bytes: u64) -> io::Result<Option<bool>> {
    match free_bytes_available(path) {
        Ok(free) => Ok(Some(free >= required_bytes)),
        Err(err) if err.kind() == io::ErrorKind::Unsupported => Ok(None),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_uses_one_pass_multiplier_by_default() {
        let estimate = estimate_required_bytes(100, 50, 10, 0.8, false);

        assert_eq!(estimate.estimated_video_bytes, 5000);
        assert_eq!(estimate.required_bytes, 6250);
        assert_eq!(estimate.multiplier, 1.25);
    }

    #[test]
    fn estimate_uses_larger_multiplier_for_two_step_fallback() {
        let estimate = estimate_required_bytes(100, 50, 10, 0.8, true);

        assert_eq!(estimate.estimated_video_bytes, 5000);
        assert_eq!(estimate.required_bytes, 7500);
        assert_eq!(estimate.multiplier, 1.5);
    }
}
