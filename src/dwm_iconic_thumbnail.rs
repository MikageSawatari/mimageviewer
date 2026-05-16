//! DWM taskbar thumbnail support for native fullscreen video.
//!
//! Native video is rendered by an owned popup HWND with DirectComposition, while
//! the taskbar entry belongs to the eframe main HWND. During video fullscreen we
//! therefore provide an explicit iconic bitmap for the main HWND.

#![cfg(windows)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_FORCE_ICONIC_REPRESENTATION, DWMWA_HAS_ICONIC_BITMAP, DwmInvalidateIconicBitmaps,
    DwmSetIconicLivePreviewBitmap, DwmSetIconicThumbnail, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, HBITMAP,
    HGDIOBJ,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, WM_DWMSENDICONICLIVEPREVIEWBITMAP, WM_DWMSENDICONICTHUMBNAIL, WM_NCDESTROY,
};
use windows::core::BOOL;

const SUBCLASS_ID: usize = 0x6d49_5654_444d_574d; // "mIVTDMWM"
const CACHE_MAX_EDGE: u32 = 960;
const LIVE_PREVIEW_MAX_WIDTH: u32 = 1920;
const LIVE_PREVIEW_MAX_HEIGHT: u32 = 1080;
const SOURCE_BUCKET_SECS: f64 = 1.0;
const MIN_WORKER_INTERVAL: Duration = Duration::from_millis(900);
const PREVIEW_OBSERVED_GRACE: Duration = Duration::from_secs(4);

#[derive(Clone, Debug)]
pub struct VideoIconicSource {
    pub path: PathBuf,
    pub target_secs: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceKey {
    path: PathBuf,
    bucket: i64,
}

impl SourceKey {
    fn from_source(source: &VideoIconicSource) -> Self {
        let secs = source.target_secs.max(0.0);
        Self {
            path: source.path.clone(),
            bucket: (secs / SOURCE_BUCKET_SECS).floor() as i64,
        }
    }
}

#[derive(Clone)]
struct CachedFrame {
    key: SourceKey,
    width: u32,
    height: u32,
    rgba: Arc<Vec<u8>>,
}

struct IconicState {
    installed_hwnd: u64,
    attrs_hwnd: u64,
    attrs_active: bool,
    active: bool,
    source: Option<VideoIconicSource>,
    source_key: Option<SourceKey>,
    generation: u64,
    worker_generation: Option<u64>,
    last_spawn_at: Option<Instant>,
    preview_observed_until: Option<Instant>,
    refresh_requested: bool,
    cache: Option<CachedFrame>,
}

impl Default for IconicState {
    fn default() -> Self {
        Self {
            installed_hwnd: 0,
            attrs_hwnd: 0,
            attrs_active: false,
            active: false,
            source: None,
            source_key: None,
            generation: 0,
            worker_generation: None,
            last_spawn_at: None,
            preview_observed_until: None,
            refresh_requested: false,
            cache: None,
        }
    }
}

struct WorkerRequest {
    hwnd: u64,
    generation: u64,
    source: VideoIconicSource,
    key: SourceKey,
}

fn state() -> &'static Mutex<IconicState> {
    static STATE: OnceLock<Mutex<IconicState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(IconicState::default()))
}

pub fn sync_video_source(hwnd_raw: u64, source: Option<VideoIconicSource>) {
    if hwnd_raw == 0 {
        return;
    }
    ensure_subclass(hwnd_raw);

    let mut request = None;
    if let Ok(mut guard) = state().lock() {
        match source {
            Some(source) => {
                let key = SourceKey::from_source(&source);
                if !guard.active {
                    guard.active = true;
                    guard.generation = guard.generation.wrapping_add(1);
                }
                let path_changed = guard
                    .source_key
                    .as_ref()
                    .map(|old| old.path != key.path)
                    .unwrap_or(true);
                if guard.source_key.as_ref() != Some(&key) {
                    guard.source_key = Some(key.clone());
                    if path_changed {
                        guard.generation = guard.generation.wrapping_add(1);
                    }
                }
                guard.source = Some(source);
                set_iconic_attrs_locked(&mut guard, hwnd_raw, true);
                request = maybe_worker_request_locked(&mut guard, hwnd_raw);
            }
            None => {
                if guard.active || guard.attrs_active || guard.cache.is_some() {
                    guard.active = false;
                    guard.source = None;
                    guard.source_key = None;
                    guard.cache = None;
                    guard.preview_observed_until = None;
                    guard.refresh_requested = false;
                    guard.generation = guard.generation.wrapping_add(1);
                    set_iconic_attrs_locked(&mut guard, hwnd_raw, false);
                }
            }
        }
    }

    if let Some(request) = request {
        spawn_worker(request);
    }
}

fn ensure_subclass(hwnd_raw: u64) {
    let mut should_install = false;
    if let Ok(guard) = state().lock() {
        should_install = guard.installed_hwnd != hwnd_raw;
    }
    if !should_install {
        return;
    }

    let hwnd = HWND(hwnd_raw as *mut _);
    let ok = unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, 0).as_bool() };
    if ok {
        if let Ok(mut guard) = state().lock() {
            guard.installed_hwnd = hwnd_raw;
        }
        crate::logger::log(format!(
            "[dwm-iconic] installed main HWND subclass hwnd=0x{hwnd_raw:x}"
        ));
    } else {
        crate::logger::log(format!(
            "[dwm-iconic] SetWindowSubclass failed hwnd=0x{hwnd_raw:x}"
        ));
    }
}

fn maybe_worker_request_locked(guard: &mut IconicState, hwnd: u64) -> Option<WorkerRequest> {
    if guard.worker_generation.is_some() {
        return None;
    }
    let source = guard.source.clone()?;
    let key = guard.source_key.clone()?;
    let now = Instant::now();
    let preview_observed = guard
        .preview_observed_until
        .is_some_and(|until| now <= until);
    if !preview_observed {
        guard.preview_observed_until = None;
    }
    let first_or_swapped_file = guard
        .cache
        .as_ref()
        .map(|cache| cache.key.path != key.path)
        .unwrap_or(true);
    let stale = guard
        .cache
        .as_ref()
        .map(|cache| cache.key != key)
        .unwrap_or(true);
    let requested_stale =
        (guard.refresh_requested || preview_observed) && stale && guard.cache.is_some();
    if !first_or_swapped_file && !requested_stale {
        return None;
    }
    if guard
        .last_spawn_at
        .is_some_and(|last| now.duration_since(last) < MIN_WORKER_INTERVAL)
    {
        return None;
    }
    let generation = guard.generation;
    guard.worker_generation = Some(generation);
    guard.last_spawn_at = Some(now);
    guard.refresh_requested = false;
    Some(WorkerRequest {
        hwnd,
        generation,
        source,
        key,
    })
}

fn spawn_worker(request: WorkerRequest) {
    let generation = request.generation;
    let spawn_result = std::thread::Builder::new()
        .name("dwm-iconic-video-thumb".into())
        .spawn(move || {
            let result = crate::video::screenshot::capture_frame(
                &request.source.path,
                request.source.target_secs,
            )
            .map(|frame| {
                let (width, height, rgba) =
                    resize_rgba_max_edge(&frame.rgba, frame.width, frame.height, CACHE_MAX_EDGE);
                CachedFrame {
                    key: request.key.clone(),
                    width,
                    height,
                    rgba: Arc::new(rgba),
                }
            });
            finish_worker(request, result);
        });
    if let Err(err) = spawn_result {
        if let Ok(mut guard) = state().lock() {
            if guard.worker_generation == Some(generation) {
                guard.worker_generation = None;
            }
        }
        crate::logger::log(format!("[dwm-iconic] worker spawn failed: {err}"));
    }
}

fn finish_worker(request: WorkerRequest, result: Result<CachedFrame, String>) {
    let mut accepted = false;
    if let Ok(mut guard) = state().lock() {
        if guard.worker_generation == Some(request.generation) {
            guard.worker_generation = None;
        }
        match result {
            Ok(frame)
                if guard.active
                    && guard.generation == request.generation
                    && guard
                        .source_key
                        .as_ref()
                        .is_some_and(|key| key.path == request.key.path) =>
            {
                guard.cache = Some(frame);
                accepted = true;
            }
            Ok(_) => {}
            Err(err) => {
                crate::logger::log(format!(
                    "[dwm-iconic] thumbnail capture failed gen={} target={:.3}s {}: {err}",
                    request.generation,
                    request.source.target_secs,
                    request.source.path.display()
                ));
            }
        }
    }

    if accepted {
        let hwnd = HWND(request.hwnd as *mut _);
        let _ = unsafe { DwmInvalidateIconicBitmaps(hwnd) };
        crate::logger::log(format!(
            "[dwm-iconic] thumbnail cache updated target={:.3}s {}",
            request.source.target_secs,
            request
                .source
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
        ));
    }
}

fn set_iconic_attrs_locked(guard: &mut IconicState, hwnd_raw: u64, active: bool) {
    if guard.attrs_hwnd == hwnd_raw && guard.attrs_active == active {
        return;
    }
    let hwnd = HWND(hwnd_raw as *mut _);
    let value = BOOL(active as i32);
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_HAS_ICONIC_BITMAP,
            &value as *const BOOL as *const _,
            std::mem::size_of::<BOOL>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_FORCE_ICONIC_REPRESENTATION,
            &value as *const BOOL as *const _,
            std::mem::size_of::<BOOL>() as u32,
        );
        let _ = DwmInvalidateIconicBitmaps(hwnd);
    }
    guard.attrs_hwnd = hwnd_raw;
    guard.attrs_active = active;
    crate::logger::log(format!(
        "[dwm-iconic] iconic attrs active={active} hwnd=0x{hwnd_raw:x}"
    ));
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    umsg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _uid_subclass: usize,
    _ref_data: usize,
) -> LRESULT {
    match umsg {
        WM_DWMSENDICONICTHUMBNAIL => {
            let max_w = loword(lparam.0 as usize).max(1);
            let max_h = hiword(lparam.0 as usize).max(1);
            if handle_thumbnail_request(hwnd, max_w, max_h) {
                return LRESULT(0);
            }
        }
        WM_DWMSENDICONICLIVEPREVIEWBITMAP => {
            if handle_live_preview_request(hwnd) {
                return LRESULT(0);
            }
        }
        WM_NCDESTROY => {
            if let Ok(mut guard) = state().lock() {
                if guard.installed_hwnd == hwnd.0 as u64 {
                    guard.installed_hwnd = 0;
                }
                if guard.attrs_hwnd == hwnd.0 as u64 {
                    guard.attrs_hwnd = 0;
                    guard.attrs_active = false;
                }
            }
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID) };
        }
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, umsg, wparam, lparam) }
}

fn handle_thumbnail_request(hwnd: HWND, max_w: u32, max_h: u32) -> bool {
    let (frame, refresh) = cached_frame_and_refresh_request(hwnd);
    if let Some(request) = refresh {
        spawn_worker(request);
    }
    let Some(frame) = frame else {
        return false;
    };
    let (w, h) = fit_dimensions(frame.width, frame.height, max_w, max_h);
    let bgra = render_bgra_canvas(&frame, w, h);
    let Some(bitmap) = create_hbitmap_bgra(w, h, &bgra) else {
        return false;
    };
    let ok = unsafe { DwmSetIconicThumbnail(hwnd, bitmap, 0).is_ok() };
    unsafe {
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
    }
    ok
}

fn handle_live_preview_request(hwnd: HWND) -> bool {
    let (frame, refresh) = cached_frame_and_refresh_request(hwnd);
    if let Some(request) = refresh {
        spawn_worker(request);
    }
    let Some(frame) = frame else {
        return false;
    };
    let mut rect = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rect) }.is_err() {
        return false;
    }
    let client_w = (rect.right - rect.left).max(1) as u32;
    let client_h = (rect.bottom - rect.top).max(1) as u32;
    let (w, h) = if client_w > LIVE_PREVIEW_MAX_WIDTH || client_h > LIVE_PREVIEW_MAX_HEIGHT {
        fit_dimensions(
            client_w,
            client_h,
            LIVE_PREVIEW_MAX_WIDTH,
            LIVE_PREVIEW_MAX_HEIGHT,
        )
    } else {
        (client_w, client_h)
    };
    let bgra = render_bgra_canvas(&frame, w, h);
    let Some(bitmap) = create_hbitmap_bgra(w, h, &bgra) else {
        return false;
    };
    let ok = unsafe { DwmSetIconicLivePreviewBitmap(hwnd, bitmap, None, 0).is_ok() };
    unsafe {
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
    }
    ok
}

fn cached_frame_and_refresh_request(hwnd: HWND) -> (Option<CachedFrame>, Option<WorkerRequest>) {
    let hwnd_raw = hwnd.0 as usize as u64;
    state()
        .lock()
        .ok()
        .map(|mut guard| {
            if !guard.active {
                return (None, None);
            }
            guard.preview_observed_until = Some(Instant::now() + PREVIEW_OBSERVED_GRACE);
            let stale = match (guard.cache.as_ref(), guard.source_key.as_ref()) {
                (Some(cache), Some(key)) => cache.key != *key,
                (None, Some(_)) => true,
                _ => false,
            };
            let refresh = if stale {
                guard.refresh_requested = true;
                maybe_worker_request_locked(&mut guard, hwnd_raw)
            } else {
                None
            };
            (guard.cache.clone(), refresh)
        })
        .unwrap_or((None, None))
}

fn fit_dimensions(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let src_w = src_w.max(1);
    let src_h = src_h.max(1);
    let max_w = max_w.max(1);
    let max_h = max_h.max(1);
    let scale = (max_w as f64 / src_w as f64).min(max_h as f64 / src_h as f64);
    let w = ((src_w as f64 * scale).round() as u32).clamp(1, max_w);
    let h = ((src_h as f64 * scale).round() as u32).clamp(1, max_h);
    (w, h)
}

fn resize_rgba_max_edge(src: &[u8], src_w: u32, src_h: u32, max_edge: u32) -> (u32, u32, Vec<u8>) {
    let max_edge = max_edge.max(1);
    let src_w = src_w.max(1);
    let src_h = src_h.max(1);
    let largest = src_w.max(src_h);
    let (dst_w, dst_h) = if largest <= max_edge {
        (src_w, src_h)
    } else if src_w >= src_h {
        (
            max_edge,
            ((src_h as u64 * max_edge as u64 + src_w as u64 / 2) / src_w as u64).max(1) as u32,
        )
    } else {
        (
            ((src_w as u64 * max_edge as u64 + src_h as u64 / 2) / src_h as u64).max(1) as u32,
            max_edge,
        )
    };
    if dst_w == src_w && dst_h == src_h {
        return (dst_w, dst_h, src.to_vec());
    }
    let mut out = vec![0; dst_w as usize * dst_h as usize * 4];
    blit_nearest_rgba(src, src_w, src_h, &mut out, dst_w, dst_h);
    (dst_w, dst_h, out)
}

fn render_bgra_canvas(frame: &CachedFrame, canvas_w: u32, canvas_h: u32) -> Vec<u8> {
    let canvas_w = canvas_w.max(1);
    let canvas_h = canvas_h.max(1);
    let (draw_w, draw_h) = fit_dimensions(frame.width, frame.height, canvas_w, canvas_h);
    let offset_x = (canvas_w - draw_w) / 2;
    let offset_y = (canvas_h - draw_h) / 2;
    let mut out = vec![0; canvas_w as usize * canvas_h as usize * 4];
    for px in out.chunks_exact_mut(4) {
        px[3] = 255;
    }
    for y in 0..draw_h {
        let sy = (y as u64 * frame.height as u64 / draw_h as u64) as u32;
        for x in 0..draw_w {
            let sx = (x as u64 * frame.width as u64 / draw_w as u64) as u32;
            let src_idx = ((sy * frame.width + sx) as usize) * 4;
            let dst_idx = (((y + offset_y) * canvas_w + (x + offset_x)) as usize) * 4;
            out[dst_idx] = frame.rgba[src_idx + 2];
            out[dst_idx + 1] = frame.rgba[src_idx + 1];
            out[dst_idx + 2] = frame.rgba[src_idx];
            out[dst_idx + 3] = 255;
        }
    }
    out
}

fn blit_nearest_rgba(src: &[u8], src_w: u32, src_h: u32, dst: &mut [u8], dst_w: u32, dst_h: u32) {
    for y in 0..dst_h {
        let sy = (y as u64 * src_h as u64 / dst_h as u64) as u32;
        for x in 0..dst_w {
            let sx = (x as u64 * src_w as u64 / dst_w as u64) as u32;
            let src_idx = ((sy * src_w + sx) as usize) * 4;
            let dst_idx = ((y * dst_w + x) as usize) * 4;
            if src_idx + 4 <= src.len() && dst_idx + 4 <= dst.len() {
                dst[dst_idx..dst_idx + 4].copy_from_slice(&src[src_idx..src_idx + 4]);
            }
        }
    }
}

fn create_hbitmap_bgra(width: u32, height: u32, bgra: &[u8]) -> Option<HBITMAP> {
    if width == 0 || height == 0 || bgra.len() < width as usize * height as usize * 4 {
        return None;
    }
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: width * height * 4,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let bitmap =
        unsafe { CreateDIBSection(None, &mut info, DIB_RGB_COLORS, &mut bits, None, 0) }.ok()?;
    if bits.is_null() {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
        }
        return None;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(
            bgra.as_ptr(),
            bits.cast::<u8>(),
            width as usize * height as usize * 4,
        );
    }
    Some(bitmap)
}

fn loword(value: usize) -> u32 {
    (value & 0xffff) as u32
}

fn hiword(value: usize) -> u32 {
    ((value >> 16) & 0xffff) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_dimensions_preserves_aspect() {
        assert_eq!(fit_dimensions(1920, 1080, 200, 200), (200, 113));
        assert_eq!(fit_dimensions(1080, 1920, 200, 200), (113, 200));
    }

    #[test]
    fn resize_rgba_max_edge_downscales() {
        let src = vec![255; 4 * 4 * 4];
        let (w, h, out) = resize_rgba_max_edge(&src, 4, 4, 2);
        assert_eq!((w, h), (2, 2));
        assert_eq!(out.len(), 2 * 2 * 4);
    }
}
