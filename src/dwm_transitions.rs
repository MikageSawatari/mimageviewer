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

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::EnumThreadWindows;
use windows::core::BOOL;

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

unsafe extern "system" fn enum_proc(hwnd: HWND, _lparam: LPARAM) -> BOOL {
    let disable: BOOL = BOOL(1);
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            &disable as *const BOOL as *const _,
            std::mem::size_of::<BOOL>() as u32,
        )
    };
    BOOL(1) // TRUE = 列挙続行
}
