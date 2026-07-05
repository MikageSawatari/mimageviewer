//! Windows DWM ウィンドウトランジション(フェードイン/アウト)の無効化。
//!
//! egui 0.33 + eframe は子ビューポート (show_viewport_immediate) の HWND を
//! 公開していないため、自プロセス/自スレッドの全トップレベルウィンドウを列挙して
//! `DwmSetWindowAttribute(DWMWA_TRANSITIONS_FORCEDISABLED, TRUE)` を適用する。
//!
//! - DWM 属性は一度設定すればウィンドウのライフタイム中維持される (再設定不要)。
//! - メインウィンドウにも適用されるが、開閉時のフェードを消すだけで
//!   副作用はない (既存 UI のアニメーションは egui 側で描画している)。
//! - Raymond Chen (Microsoft) のブログでも、この属性がウィンドウ単体のフェード抑止の
//!   正式な手段として示されている:
//!   <https://devblogs.microsoft.com/oldnewthing/20121003-00/?p=6423>
//!
//! Windows 11 で効きが不安定という報告があるため、失敗は無視 (ベストエフォート)。

use windows::Win32::Foundation::{HWND, LPARAM, RECT, S_OK};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_CLOAK, DWMWA_COLOR_DEFAULT,
    DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, GetWindowRect, HWND_TOP, IsIconic, IsWindowVisible, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
};
use windows::core::{BOOL, Result};

struct ComInitScope {
    needs_uninit: bool,
}

impl ComInitScope {
    fn init_sta() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Self {
            needs_uninit: hr == S_OK,
        }
    }
}

impl Drop for ComInitScope {
    fn drop(&mut self) {
        if self.needs_uninit {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

/// 自スレッドの全トップレベルウィンドウに対して DWM トランジションを無効化する。
///
/// egui の全ビューポート (メイン + フルスクリーン子ビューポート) は同じスレッド
/// (UI スレッド) 上で winit によって作成されるため、GetCurrentThreadId() +
/// EnumThreadWindows で漏れなく列挙できる。
///
/// 属性はウィンドウが作られて初めて適用できるので、新しいビューポートが
/// 作成されるたびに呼び出す必要がある。再適用は冪等 (既に設定済みなら no-op)。
pub fn disable_transitions_for_thread_windows() {
    unsafe {
        let tid = GetCurrentThreadId();
        let _ = EnumThreadWindows(tid, Some(enum_proc), LPARAM(0));
    }
}

pub fn disable_transitions_for_window(hwnd: HWND) {
    let disable: BOOL = BOOL(1);
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            &disable as *const BOOL as *const _,
            std::mem::size_of::<BOOL>() as u32,
        )
    };
}

pub fn set_window_cloaked(hwnd: HWND, cloaked: bool) -> windows::core::Result<()> {
    let cloak: i32 = if cloaked { 1 } else { 0 };
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            &cloak as *const i32 as *const _,
            std::mem::size_of::<i32>() as u32,
        )
    }
}

pub fn raise_visible_thread_window_matching_rect(main_hwnd: HWND, expected: RECT) -> Option<HWND> {
    let hwnd = find_visible_thread_window_matching_rect(main_hwnd, expected)?;
    unsafe {
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        if SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, flags).is_ok() {
            Some(hwnd)
        } else {
            None
        }
    }
}

pub fn move_window_to_desktop_of(owner_hwnd: HWND, target_hwnd: HWND) -> Result<()> {
    if owner_hwnd.0.is_null() || target_hwnd.0.is_null() || owner_hwnd == target_hwnd {
        return Ok(());
    }
    let _com = ComInitScope::init_sta();
    unsafe {
        let manager: IVirtualDesktopManager =
            CoCreateInstance(&VirtualDesktopManager, None, CLSCTX_ALL)?;
        let desktop_id = manager.GetWindowDesktopId(owner_hwnd)?;
        manager.MoveWindowToDesktop(target_hwnd, &desktop_id)
    }
}

pub fn find_visible_thread_window_matching_rect(main_hwnd: HWND, expected: RECT) -> Option<HWND> {
    find_visible_thread_window_matching_rect_excluding(main_hwnd, expected, &[])
}

pub fn find_visible_thread_window_matching_rect_excluding(
    main_hwnd: HWND,
    expected: RECT,
    excluded_hwnds: &[u64],
) -> Option<HWND> {
    let mut state = RaiseWindowState {
        main_hwnd,
        expected,
        excluded_hwnds: excluded_hwnds.as_ptr(),
        excluded_hwnds_len: excluded_hwnds.len(),
        best_hwnd: HWND::default(),
        best_score: i64::MAX,
    };
    unsafe {
        let tid = GetCurrentThreadId();
        let state_ptr = &mut state as *mut RaiseWindowState;
        let _ = EnumThreadWindows(tid, Some(raise_enum_proc), LPARAM(state_ptr as isize));
        if state.best_hwnd.0.is_null() {
            return None;
        }
        Some(state.best_hwnd)
    }
}

pub fn debug_thread_windows_for_rect(main_hwnd: HWND, expected: RECT, limit: usize) -> String {
    let mut state = DebugWindowsState {
        main_hwnd,
        expected,
        entries: Vec::new(),
    };
    unsafe {
        let tid = GetCurrentThreadId();
        let state_ptr = &mut state as *mut DebugWindowsState;
        let _ = EnumThreadWindows(
            tid,
            Some(debug_windows_enum_proc),
            LPARAM(state_ptr as isize),
        );
    }
    state.entries.sort_by_key(|entry| entry.score);
    state
        .entries
        .into_iter()
        .take(limit)
        .map(|entry| entry.format())
        .collect::<Vec<_>>()
        .join("; ")
}

pub fn set_window_chrome_black(hwnd: HWND) {
    set_window_chrome_color(hwnd, 0x000000);
}

pub fn restore_window_chrome_for_theme(hwnd: HWND, dark: bool) {
    let dark_mode: i32 = if dark { 1 } else { 0 };
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark_mode as *const i32 as *const _,
            std::mem::size_of::<i32>() as u32,
        );
    }
    set_window_chrome_color(hwnd, DWMWA_COLOR_DEFAULT);
}

fn set_window_chrome_color(hwnd: HWND, color: u32) {
    let size = std::mem::size_of::<u32>() as u32;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            &color as *const u32 as *const _,
            size,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &color as *const u32 as *const _,
            size,
        );
    }
}

unsafe extern "system" fn enum_proc(hwnd: HWND, _lparam: LPARAM) -> BOOL {
    disable_transitions_for_window(hwnd);
    BOOL(1) // TRUE = 列挙続行
}

struct RaiseWindowState {
    main_hwnd: HWND,
    expected: RECT,
    excluded_hwnds: *const u64,
    excluded_hwnds_len: usize,
    best_hwnd: HWND,
    best_score: i64,
}

struct DebugWindowsState {
    main_hwnd: HWND,
    expected: RECT,
    entries: Vec<DebugWindowEntry>,
}

struct DebugWindowEntry {
    hwnd_raw: u64,
    is_main: bool,
    visible: bool,
    iconic: bool,
    rect_ok: bool,
    rect: RECT,
    score: i64,
    contains_center: bool,
    covers_most_expected: bool,
}

impl DebugWindowEntry {
    fn format(&self) -> String {
        if self.rect_ok {
            format!(
                "hwnd=0x{:x} main={} visible={} iconic={} rect=({},{} {}x{}) \
                 score={} center={} cover={}",
                self.hwnd_raw,
                self.is_main,
                self.visible,
                self.iconic,
                self.rect.left,
                self.rect.top,
                self.rect.right - self.rect.left,
                self.rect.bottom - self.rect.top,
                self.score,
                self.contains_center,
                self.covers_most_expected
            )
        } else {
            format!(
                "hwnd=0x{:x} main={} visible={} iconic={} rect=<err>",
                self.hwnd_raw, self.is_main, self.visible, self.iconic
            )
        }
    }
}

unsafe extern "system" fn raise_enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let state = unsafe { &mut *(lparam.0 as *mut RaiseWindowState) };
    let hwnd_raw = hwnd.0 as usize as u64;
    let excluded = if state.excluded_hwnds.is_null() || state.excluded_hwnds_len == 0 {
        false
    } else {
        let excluded_hwnds =
            unsafe { std::slice::from_raw_parts(state.excluded_hwnds, state.excluded_hwnds_len) };
        excluded_hwnds.contains(&hwnd_raw)
    };
    if hwnd.0 == state.main_hwnd.0 || excluded || !unsafe { IsWindowVisible(hwnd).as_bool() } {
        return BOOL(1);
    }

    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return BOOL(1);
    }

    let width = (rect.right - rect.left).max(0) as i64;
    let height = (rect.bottom - rect.top).max(0) as i64;
    let expected_width = (state.expected.right - state.expected.left).max(0) as i64;
    let expected_height = (state.expected.bottom - state.expected.top).max(0) as i64;
    if width <= 0 || height <= 0 || expected_width <= 0 || expected_height <= 0 {
        return BOOL(1);
    }

    let cx = state.expected.left + (state.expected.right - state.expected.left) / 2;
    let cy = state.expected.top + (state.expected.bottom - state.expected.top) / 2;
    let contains_center = rect.left <= cx && cx < rect.right && rect.top <= cy && cy < rect.bottom;
    let covers_most_expected = width * height >= (expected_width * expected_height * 2) / 3;
    if !contains_center || !covers_most_expected {
        return BOOL(1);
    }

    let score = (rect.left - state.expected.left).abs() as i64
        + (rect.top - state.expected.top).abs() as i64
        + (rect.right - state.expected.right).abs() as i64
        + (rect.bottom - state.expected.bottom).abs() as i64;
    if score < state.best_score {
        state.best_score = score;
        state.best_hwnd = hwnd;
    }
    BOOL(1)
}

unsafe extern "system" fn debug_windows_enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let state = unsafe { &mut *(lparam.0 as *mut DebugWindowsState) };
    let visible = unsafe { IsWindowVisible(hwnd).as_bool() };
    let iconic = unsafe { IsIconic(hwnd).as_bool() };
    let mut rect = RECT::default();
    let rect_ok = unsafe { GetWindowRect(hwnd, &mut rect).is_ok() };
    let expected_width = (state.expected.right - state.expected.left).max(0) as i64;
    let expected_height = (state.expected.bottom - state.expected.top).max(0) as i64;
    let (score, contains_center, covers_most_expected) = if rect_ok {
        let width = (rect.right - rect.left).max(0) as i64;
        let height = (rect.bottom - rect.top).max(0) as i64;
        let cx = state.expected.left + (state.expected.right - state.expected.left) / 2;
        let cy = state.expected.top + (state.expected.bottom - state.expected.top) / 2;
        (
            (rect.left - state.expected.left).abs() as i64
                + (rect.top - state.expected.top).abs() as i64
                + (rect.right - state.expected.right).abs() as i64
                + (rect.bottom - state.expected.bottom).abs() as i64,
            rect.left <= cx && cx < rect.right && rect.top <= cy && cy < rect.bottom,
            width > 0
                && height > 0
                && expected_width > 0
                && expected_height > 0
                && width * height >= (expected_width * expected_height * 2) / 3,
        )
    } else {
        (i64::MAX, false, false)
    };
    state.entries.push(DebugWindowEntry {
        hwnd_raw: hwnd.0 as usize as u64,
        is_main: hwnd.0 == state.main_hwnd.0,
        visible,
        iconic,
        rect_ok,
        rect,
        score,
        contains_center,
        covers_most_expected,
    });
    BOOL(1)
}
