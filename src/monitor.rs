/// Windows モニター情報のヘルパー関数群。
/// DPI スケーリングを考慮した論理ピクセル座標で返す。

// -----------------------------------------------------------------------
// FFI 定義（user32.dll / shcore.dll）
// -----------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod ffi {
    #[repr(C)]
    pub struct Point { pub x: i32, pub y: i32 }

    #[repr(C)]
    pub struct Rect { pub left: i32, pub top: i32, pub right: i32, pub bottom: i32 }

    #[repr(C)]
    pub struct MonitorInfo {
        pub cb_size:    u32,
        pub rc_monitor: Rect,
        pub rc_work:    Rect,
        pub dw_flags:   u32,
    }

    pub const MONITOR_DEFAULTTONULL:    u32 = 0x0000_0000;
    pub const MONITOR_DEFAULTTONEAREST: u32 = 0x0000_0002;
    pub const MDT_EFFECTIVE_DPI:        u32 = 0;

    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn MonitorFromPoint(pt: Point, flags: u32) -> isize;
        pub fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfo) -> i32;
    }

    #[link(name = "shcore")]
    unsafe extern "system" {
        /// HRESULT: 0 = S_OK
        pub fn GetDpiForMonitor(
            monitor: isize,
            dpi_type: u32,
            dpi_x: *mut u32,
            dpi_y: *mut u32,
        ) -> i32;
    }
}

// -----------------------------------------------------------------------
// 公開 API
// -----------------------------------------------------------------------

/// タイトルバーの中央が接続済みモニター上にあるかを確認する。
/// モニターが切断されて座標が画面外になっている場合 false を返す。
/// 引数は egui 論理ピクセル座標。
pub fn title_bar_on_some_monitor(win_x: f32, win_y: f32, win_w: f32) -> bool {
    let check_x = (win_x + win_w / 2.0) as i32;
    let check_y = (win_y + 15.0) as i32;

    #[cfg(target_os = "windows")]
    {
        use ffi::*;
        let handle = unsafe {
            MonitorFromPoint(Point { x: check_x, y: check_y }, MONITOR_DEFAULTTONULL)
        };
        handle != 0
    }

    #[cfg(not(target_os = "windows"))]
    { let _ = (check_x, check_y); true }
}

/// 指定した座標を含むモニターの情報をログ付きで調査し、論理ピクセル矩形を返す。
///
/// # 座標系について
/// `MonitorFromPoint` は物理ピクセル座標を期待するが、egui が報告する
/// `outer_rect` は論理ピクセル座標（DPI スケール済み）。
/// DPI 100% 環境では一致するが、HiDPI では差が生じる。
/// そのため、入力座標をそのまま渡した場合と、モニター DPI で補正した場合の
/// 両方をログに出力して実態を把握する。
pub fn get_monitor_logical_rect_at(x: f32, y: f32) -> Option<egui::Rect> {
    #[cfg(target_os = "windows")]
    {
        use ffi::*;
        use std::mem;

        crate::logger::log(format!(
            "[monitor] get_monitor_logical_rect_at input: physical=({x:.1}, {y:.1})"
        ));

        // まず論理座標をそのまま物理座標として渡して試みる
        let monitor = unsafe {
            MonitorFromPoint(Point { x: x as i32, y: y as i32 }, MONITOR_DEFAULTTONEAREST)
        };
        if monitor == 0 {
            crate::logger::log("[monitor] MonitorFromPoint returned null".to_string());
            return None;
        }

        let mut info = MonitorInfo {
            cb_size:    mem::size_of::<MonitorInfo>() as u32,
            rc_monitor: Rect { left: 0, top: 0, right: 0, bottom: 0 },
            rc_work:    Rect { left: 0, top: 0, right: 0, bottom: 0 },
            dw_flags:   0,
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            crate::logger::log("[monitor] GetMonitorInfoW failed".to_string());
            return None;
        }

        let mut dpi_x: u32 = 96;
        let mut dpi_y: u32 = 96;
        let dpi_hr = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        if dpi_hr != 0 {
            crate::logger::log(format!("[monitor] GetDpiForMonitor failed: HRESULT={dpi_hr:#x}, using 96 DPI"));
            dpi_x = 96;
        }
        let scale = dpi_x.max(1) as f32 / 96.0;

        let r = &info.rc_monitor;
        let logical_rect = egui::Rect::from_min_max(
            egui::pos2(r.left  as f32 / scale, r.top    as f32 / scale),
            egui::pos2(r.right as f32 / scale, r.bottom as f32 / scale),
        );

        crate::logger::log(format!(
            "[monitor] handle={monitor:#x}  DPI={dpi_x}  scale={scale:.2}  \
             phys=[{},{},{},{}]  logical=[{:.1},{:.1},{:.1},{:.1}]",
            r.left, r.top, r.right, r.bottom,
            logical_rect.min.x, logical_rect.min.y,
            logical_rect.max.x, logical_rect.max.y,
        ));

        Some(logical_rect)
    }

    #[cfg(not(target_os = "windows"))]
    { let _ = (x, y); None }
}
