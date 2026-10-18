//! The Painter's device limit is a constraint on the native window, not just its surface.
#![expect(unsafe_code)]

use std::io;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GetClientRect, GetWindowRect, GetWindowThreadProcessId, MINMAXINFO, SWP_NOACTIVATE,
            SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos, WINDOWPOS, WM_GETMINMAXINFO,
            WM_NCDESTROY, WM_WINDOWPOSCHANGING,
        },
    },
};
use winit::dpi::PhysicalSize;

// The callback/ID pair is private to this module. Refdata is a value, never a Painter pointer.
const SUBCLASS_ID: usize = 0x6567_736c;
pub(crate) const LOG_TARGET: &str = "egui_wgpu::surface_window_limit";

fn limited_size(width: i32, height: i32, flags: u32, limit: i32) -> (i32, i32) {
    if flags & SWP_NOSIZE != 0 {
        (width, height)
    } else {
        (width.min(limit), height.min(limit))
    }
}

unsafe extern "system" fn size_limit_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    limit: usize,
) -> LRESULT {
    let incoming_size = if message == WM_WINDOWPOSCHANGING {
        // SAFETY: This message owns a live WINDOWPOS. Only observe it before the chain;
        // DefWindowProc can already cap it using our WM_GETMINMAXINFO constraint.
        let pos = unsafe { &*(lparam as *const WINDOWPOS) };
        Some((pos.cx, pos.cy))
    } else {
        None
    };
    // winit must first apply its fullscreen and min/max-inner-size adjustments.
    // SAFETY: The subclass chain owns these message arguments for this invocation.
    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    match message {
        WM_WINDOWPOSCHANGING => {
            // SAFETY: WM_WINDOWPOSCHANGING supplies a live, mutable WINDOWPOS.
            let pos = unsafe { &mut *(lparam as *mut WINDOWPOS) };
            let adjusted = (pos.cx, pos.cy);
            let limited = limited_size(pos.cx, pos.cy, pos.flags, limit as i32);
            let incoming = incoming_size.unwrap();
            let requested =
                if limited_size(incoming.0, incoming.1, pos.flags, limit as i32) != incoming {
                    incoming
                } else {
                    adjusted // winit may itself enlarge a request before our final constraint.
                };
            if pos.flags & SWP_NOSIZE == 0
                && (incoming.0 > limit as i32 || incoming.1 > limit as i32 || limited != adjusted)
            {
                log::warn!(target: LOG_TARGET,
                    "Limiting HWND {hwnd:?} outer size {requested:?} to {limited:?} (device limit {limit})"
                );
            }
            (pos.cx, pos.cy) = limited;
        }
        WM_GETMINMAXINFO => {
            // SAFETY: WM_GETMINMAXINFO supplies a live, mutable MINMAXINFO.
            let info = unsafe { &mut *(lparam as *mut MINMAXINFO) };
            info.ptMaxTrackSize.x = info.ptMaxTrackSize.x.min(limit as i32);
            info.ptMaxTrackSize.y = info.ptMaxTrackSize.y.min(limit as i32);
            info.ptMaxSize.x = info.ptMaxSize.x.min(limit as i32);
            info.ptMaxSize.y = info.ptMaxSize.y.min(limit as i32);
        }
        WM_NCDESTROY => {
            // SAFETY: Remove only this callback/ID, on the window's destruction thread.
            unsafe { RemoveWindowSubclass(hwnd, Some(size_limit_proc), SUBCLASS_ID) };
        }
        _ => {}
    }
    result
}

fn owner_thread(hwnd: HWND) -> io::Result<()> {
    // SAFETY: These queries do not dereference application memory.
    let owner = unsafe { GetWindowThreadProcessId(hwnd, std::ptr::null_mut()) };
    if owner == 0 {
        return Err(io::Error::last_os_error());
    }
    if owner != unsafe { GetCurrentThreadId() } {
        return Err(io::Error::other(
            "surface window must be constrained on its owning thread",
        ));
    }
    Ok(())
}

pub(crate) fn register(hwnd: isize, limit: u32) -> io::Result<()> {
    let hwnd = hwnd as HWND;
    owner_thread(hwnd)?;
    if limit == 0 || limit > i32::MAX as u32 {
        return Err(io::Error::other("invalid surface window size limit"));
    }
    // SAFETY: Checked the owning thread; refdata is an integer with no borrowed lifetime.
    // Re-registering the same callback/ID updates refdata without stacking callbacks.
    if unsafe { SetWindowSubclass(hwnd, Some(size_limit_proc), SUBCLASS_ID, limit as usize) } == 0 {
        return Err(io::Error::other("SetWindowSubclass failed"));
    }
    Ok(())
}

fn outer_rect(hwnd: HWND) -> io::Result<RECT> {
    let mut rect = RECT::default();
    // SAFETY: rect is writable for the duration of the call.
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(rect)
}

pub(crate) fn enforce(hwnd: isize, limit: u32) -> io::Result<PhysicalSize<u32>> {
    let hwnd = hwnd as HWND;
    owner_thread(hwnd)?;
    let rect = outer_rect(hwnd)?;
    let requested = (rect.right - rect.left, rect.bottom - rect.top);
    let limited = limited_size(requested.0, requested.1, 0, limit as i32);
    if limited != requested {
        log::warn!(target: LOG_TARGET,
            "Correcting HWND {hwnd:?} outer size {requested:?} to {limited:?} (device limit {limit})"
        );
        // SAFETY: HWND is on this thread. Send WINDOWPOSCHANGING so the same constraint also
        // runs after winit's adjustments. Preserve position, z-order, visibility and focus.
        if unsafe {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                limited.0,
                limited.1,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    let rect = outer_rect(hwnd)?;
    let mut client = RECT::default();
    // SAFETY: client is writable for the duration of the call.
    if unsafe { GetClientRect(hwnd, &mut client) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let size = PhysicalSize::new(
        (client.right - client.left) as u32,
        (client.bottom - client.top) as u32,
    );
    if rect.right - rect.left > limit as i32
        || rect.bottom - rect.top > limit as i32
        || size.width > limit
        || size.height > limit
    {
        return Err(io::Error::other(
            "native window did not accept its surface size limit",
        ));
    }
    Ok(size)
}

#[cfg(test)]
#[path = "window_size_limit_tests.rs"]
mod tests;
