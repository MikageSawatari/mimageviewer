//! HUD overlay HWND — bars / interactive UI 用の独立 top-level window.
//!
//! VST GUI が presenter HWND の owned + TOPMOST になっているため、presenter HWND
//! 内の DComp tree に描画した overlay (= 上 bar / 下 seek bar / hover thumbnail 等)
//! は Windows の owner rule (= owned は owner より常に手前) で VST の裏に潜る。
//! HudOverlayWindow は presenter と同じ owner (= main HWND or presenter HWND) の
//! sibling として作られ、VST と並ぶ z-order group に属する。`WS_EX_TOPMOST` を
//! 維持しつつ、VST z-order 操作後に `HWND_TOPMOST` で後勝ち再アサートすることで
//! HUD を VST より前に出す。
//!
//! ## 入力モデル (3 層)
//!
//! - **Mouse** は HUD wndproc が region 内で受けて bounded event route に流す。region 外は
//!   `SetWindowRgn` で物理的に「存在しない」領域として穴を開けているので、OS が
//!   下層 (VST or presenter) に直接 mouse を配送する (= クロスプロセスでも安定)。
//! - **Touch** は HUD に届いた `PT_TOUCH` stream 全体を HWND ごとの bounded set で
//!   所有し、同じ overlay adapter へ送る。OS hit-test 済みなので常に widget passthrough。
//! - **Keyboard / IME** は HUD では受けない (`WS_EX_NOACTIVATE` で focus を取らない)。
//!   presenter HWND の既存 wndproc で受けて `NativeEguiOverlay` に流す。
//!
//! ## Region (= 物理形状)
//!
//! `apply_regions` が呼ばれるたびに `SetWindowRgn` で HUD HWND の物理形状を更新。
//! 含めるのは **実際にクリック可能な UI rect だけ** (= activation zone は含めない、
//! 含めると上端 / 下端に VST のノブやメニューが重なったとき入力を奪うため)。
//! 活性化 (= hover で bar を表示する判定) は pump observation を render が評価し、
//! synthetic pointer を流すことで実現する。
//!
//! ## Wndproc 概要
//!
//! - `WM_MOUSEMOVE` / `WM_*BUTTONDOWN/UP` / `WM_MOUSEWHEEL` は source-stamped event として
//!   enqueue する。cursor ownership 用 `WM_MOUSELEAVE` edge は pump にだけ送り、egui 用の
//!   generic `MouseLeave` とは分離する。pump は同一 drain 内の presenter/HUD handoff を
//!   集約してから auto-hide reducer を 1 回だけ更新する。
//! - `WM_LBUTTONDOWN` / `WM_RBUTTONDOWN` / `WM_MBUTTONDOWN`:
//!   1. down event 送出
//!   2. `held_buttons |= bit` (capture 成否に関係なく必ず tracking、Codex 11 P1 #1)
//!   3. `RequestFocusClaim` を pump へ enqueue し、dispatch 後に presenter HWND へ
//!      foreground/focus を戻す
//!   4. `SetCapture(hud_hwnd)` で region 外の up も拾えるようにする
//!   5. `GetCapture() != hud_hwnd` (= capture 失敗) なら synthetic up + held_buttons clear
//!      で egui の `pointer.any_down()` が stuck しないようにする
//! - `WM_*BUTTONUP`: ReleaseCapture + held_buttons clear。
//! - `WM_CAPTURECHANGED` / `WM_CANCELMODE` / `WM_DESTROY`: held_buttons に残っている
//!   ボタンの synthetic up と、HUD が所有する touch stream の Cancel を補完してから
//!   DefWindowProc (= `pointer.any_down()` の stuck を防ぐ)。
//! - `WM_MOUSEACTIVATE`: `MA_NOACTIVATE` を返す。
//! - `WM_NCHITTEST`: regions に含まれれば `HTCLIENT`、それ以外は `HTTRANSPARENT`
//!   (region 外は `SetWindowRgn` で穴になっているため通常メッセージは来ないが念のため)。
//! - `WM_WINDOWPOSCHANGING`: `WINDOWPOS::hwndInsertAfter` が自分より前を指す変更を
//!   検知したら bounded route へ `RequestRaiseHud` を流す (best-effort
//!   safety net、主経路は z-order 操作後 hook と pump observation)。
//! - `WM_DPICHANGED`: `DpiChanged { dpi, suggested_rect }` を発火。

use std::sync::Arc;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, DeleteObject, HRGN, RGN_OR, ScreenToClient, SetWindowRgn,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW,
    GWLP_USERDATA, GetWindowLongPtrW, HTCLIENT, HTTRANSPARENT, HWND_TOPMOST, IsWindow,
    MA_NOACTIVATE, RegisterClassExW, SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, WINDOWPOS, WM_APPCOMMAND,
    WM_CANCELMODE, WM_CAPTURECHANGED, WM_DESTROY, WM_DPICHANGED, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDBLCLK, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_NCCREATE, WM_NCDESTROY, WM_NCHITTEST, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_SETCURSOR, WM_WINDOWPOSCHANGING, WM_XBUTTONDBLCLK, WM_XBUTTONDOWN,
    WM_XBUTTONUP, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::Win32::UI::WindowsAndMessaging::{IDC_ARROW, LoadCursorW};
use windows::core::PCWSTR;
use windows::core::w;

use crate::touch_debug::{TouchDebugWindow, log_win32_message};
use crate::video::native_touch::NativeTouchOwnership;
use crate::video::native_window::{
    NativeCursorOwnershipEdge, NativeVideoKeyEvent, NativeVideoMouseButton,
    NativeVideoMouseButtonEvent, NativeVideoMouseEvent, NativeVideoMouseWheelEvent,
    NativeVideoWindowEvent, NativeVideoWindowEventSink, NativeVideoWindowSource,
    cancel_hud_touch_streams, handle_hud_pointer_message, should_discard_promoted_touch_mouse,
};

/// HUD overlay HWND の生成設定。
pub(super) struct HudOverlayConfig {
    /// HUD の owner として設定する HWND (= 通常は presenter HWND)。
    /// 同じ owner の sibling として、VST GUI HWND と並ぶ z-order group に入る。
    pub owner_hwnd: HWND,
    /// HUD HWND の初期 screen 座標 (Codex CP7 P1 #2 反映)。
    /// presenter HWND の `GetWindowRect` で取得した位置を渡す。`(0, 0)` ハードコードだと
    /// secondary monitor / 負座標 monitor で fullscreen presenter が動いているときに
    /// HUD が別モニターに出てしまう。
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// HUD wndproc が拾った mouse / WM_DPICHANGED / raise 要求を流す bounded route。
    /// pump/render がそれぞれの endpoint から drain する。
    pub event_sink: NativeVideoWindowEventSink,
    /// Region 共有用。`apply_regions` から書き込み、wndproc の `WM_NCHITTEST` が
    /// 読み出す (region 自体は `SetWindowRgn` 経由で OS に渡しているので
    /// `WM_NCHITTEST` まで届く mouse はほぼないが、フェイルセーフ)。
    pub regions: Arc<std::sync::Mutex<HudInteractiveRegions>>,
}

/// 現在 HUD が interactive として扱う矩形群。activation zone は **含めない**
/// (= 含めると VST のノブやメニューが上下端に重なったとき入力を奪うため)。
/// drag 中 (= egui `pointer.any_down() && wants_pointer_input`) のフレームは
/// `regions` を `[画面全体]` に置換して drag を維持する。
#[derive(Default, Clone)]
pub(super) struct HudInteractiveRegions {
    pub regions: Vec<RECT>,
}

impl HudInteractiveRegions {
    /// Cursor が region のいずれかに含まれるか。`WM_NCHITTEST` のフェイルセーフ用。
    pub fn contains(&self, client_x: i32, client_y: i32) -> bool {
        self.regions.iter().any(|r| {
            client_x >= r.left && client_x < r.right && client_y >= r.top && client_y < r.bottom
        })
    }
}

/// HUD HWND の RAII handle。Drop で `DestroyWindow`。
///
/// **作成スレッド所有**: `HudOverlayWindow` は `Send`/`Sync` を実装しない。
/// `DestroyWindow` は HWND 作成スレッドで呼ぶ必要があり、wndproc も同じスレッドで
/// dispatch されるため、別スレッドへの move は禁止する (Codex CP2 P2 反映)。
/// Stage 4 以降は専用 pump thread で生成・所有・破棄する。
pub(super) struct HudOverlayWindow {
    hwnd: HWND,
    /// `WindowState` の raw pointer。実際のライフサイクル管理は wndproc
    /// (`WM_NCCREATE` → `WM_NCDESTROY` で `Box::into_raw` / `Box::from_raw`)。
    /// この field は debug 用のメモのみ。
    _state_ptr: *mut WindowState,
    /// 直近に `apply_regions` で適用した rect 集合の hash。次回呼び出し時に
    /// 同じ hash なら `SetWindowRgn` を skip する (= 毎フレーム呼ばれても no-op
    /// で済むよう responsibility をこの API に集約、Codex CP2 P3 反映)。
    /// `None` の初期値は `create` 直後の空 region 状態に対応する hash で上書きされる。
    last_regions_hash: Option<u64>,
    owner_thread: std::thread::ThreadId,
    _not_send_or_sync: std::marker::PhantomData<std::rc::Rc<()>>,
}

// HWND と `*mut WindowState` を持つため `Send`/`Sync` は自動 derive されない。
// 上記コメントの通り、作成スレッド所有のまま使う設計なので明示的な
// `unsafe impl Send` は **付けない**。

impl HudOverlayWindow {
    #[track_caller]
    fn assert_owner_thread(&self) {
        assert_eq!(
            std::thread::current().id(),
            self.owner_thread,
            "HudOverlayWindow operation ran on a non-owner thread"
        );
    }

    pub(super) fn create(cfg: HudOverlayConfig) -> Result<Self, String> {
        register_window_class()?;
        unsafe {
            let hmodule =
                GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW for HUD: {e:?}"))?;
            let state = Box::new(WindowState {
                event_sink: cfg.event_sink,
                regions: cfg.regions,
                held_buttons: 0,
                touch_ownership: NativeTouchOwnership::default(),
                mouse_tracking: false,
                last_mouse_move_log_at: None,
            });
            let state_ptr = Box::into_raw(state);

            let owner = if cfg.owner_hwnd.0.is_null() {
                None
            } else {
                Some(cfg.owner_hwnd)
            };
            let hwnd = match CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW,
                HUD_CLASS_NAME,
                w!("mIV HUD Overlay"),
                WS_POPUP,
                cfg.x,
                cfg.y,
                cfg.width.max(1) as i32,
                cfg.height.max(1) as i32,
                owner,
                None,
                Some(hmodule.into()),
                Some(state_ptr.cast()),
            ) {
                Ok(h) => h,
                Err(err) => {
                    // Codex CP2 P2 反映: `CreateWindowExW` 失敗時に `Box::from_raw` を呼ぶと
                    // double free のリスクがある:
                    //   - `WM_NCCREATE` 後に他の理由で `CreateWindowExW` が失敗した場合、
                    //     Windows は `WM_NCDESTROY` を呼んで wndproc 側で Box drop が走る。
                    //     その後で外側でも `Box::from_raw` すると double free。
                    //   - `WM_NCCREATE` 前に失敗した場合は wndproc 側で drop されないが、
                    //     その区別を `state_ptr` から確実に判定する手段がない
                    //     (= HWND が無効なので `GetWindowLongPtrW` も使えない)。
                    // 安全側に倒して **state_ptr を leak** する。`CreateWindowExW` の
                    // 失敗は通常レアなのでメモリリークは許容範囲。
                    //
                    // T32 (Codex P2 / 2026-05-16): リトライ累積を検知できるように
                    // プロセス全体のリーク回数を atomic で記録する。ログ初回 + 10/100/1000
                    // 件単位で警告を吐く。「リーク許容」の前提が破綻していたら検出する。
                    static LEAK_COUNT: std::sync::atomic::AtomicU64 =
                        std::sync::atomic::AtomicU64::new(0);
                    let prev = LEAK_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let total = prev + 1;
                    if total == 1
                        || total == 10
                        || total == 100
                        || total == 1000
                        || total % 1000 == 0
                    {
                        crate::logger::log(format!(
                            "[HUD WindowState leak] CreateWindowExW failed; leaked {total} WindowState box(es) total ({} bytes/box) — error: {err:?}",
                            std::mem::size_of::<WindowState>(),
                        ));
                    }
                    let _ = state_ptr;
                    return Err(format!("CreateWindowExW HUD: {err:?}"));
                }
            };

            crate::presentation_observer::register(
                crate::presentation_observer::WindowRole::Hud,
                hwnd.0 as usize as u64,
            );

            crate::dwm_transitions::disable_transitions_for_window(hwnd);

            // 初期 region は「空」(= 全画面 click-through、bar 非表示時)。
            // 後で `apply_regions` で実 UI rect を入れる。`NULL` を渡すと region
            // 解除 = 全画面復活 → VST が押せなくなる致命バグになるので、必ず
            // CreateRectRgn(0,0,0,0) で空 HRGN を作って渡す (Codex 6 P1 #2)。
            //
            // Codex CP2 (再) P2 反映: 失敗時は hash を保存しない (= 次回 apply_regions で
            // 必ず再試行)。失敗パターンとして CreateRectRgn の GDI handle 枯渇や
            // SetWindowRgn 自体の失敗が考えられる。HUD が region 設定できない状態だと
            // 全画面入力を持ったままになるので、復旧のため次回も試行する。
            let initial_hash = if apply_window_region(hwnd, &[]) {
                Some(hash_regions(&[]))
            } else {
                None
            };

            // Codex CP4 P2 #2 反映: `WS_POPUP` で `WS_VISIBLE` を付けずに作っているので、
            // ここで明示的に `SW_SHOWNA` で表示する。`SW_SHOWNA` (No-Activate) は
            // foreground を奪わずに表示するので、`WS_EX_NOACTIVATE` の意図 (= focus を
            // 取らない) と整合する。
            // 初期 region は空 (= 全画面穴) なので、ShowWindow を呼んでも物理的に
            // 表示される pixel はなく flicker は起きない。CP5 で region に実 UI rect が
            // 入って初めて HUD のピクセルが見えるようになる。
            let _ = crate::presentation_observer::show_window(
                hwnd,
                SW_SHOWNA,
                crate::presentation_observer::WindowRole::Hud,
                "HudOverlayWindow::create",
            );

            Ok(Self {
                hwnd,
                _state_ptr: state_ptr,
                last_regions_hash: initial_hash,
                owner_thread: std::thread::current().id(),
                _not_send_or_sync: std::marker::PhantomData,
            })
        }
    }

    pub(super) fn hwnd(&self) -> HWND {
        self.assert_owner_thread();
        self.hwnd
    }

    /// 同じサイズ / 位置への resize は no-op (= `SetWindowPos` を毎フレーム呼ばない)。
    /// `MirrorHudGeometry` 経路から呼ばれる。
    pub(super) fn set_geometry(&self, x: i32, y: i32, w: u32, h: u32) {
        self.assert_owner_thread();
        if self.hwnd.0.is_null() {
            return;
        }
        unsafe {
            let _ = crate::presentation_observer::set_window_pos(
                self.hwnd,
                None,
                x,
                y,
                w.max(1) as i32,
                h.max(1) as i32,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
                crate::presentation_observer::WindowRole::Hud,
                "HudOverlayWindow::set_geometry",
            );
        }
    }

    /// HUD overlay ウィンドウの表示 / 非表示を切り替える (Inc 7 hidden presenter:
    /// 動画→音声モード中は presenter ウィンドウと一緒に HUD overlay も hide して、
    /// bar / VST click-through region が egui 音楽ビュー上に残らないようにする)。
    /// `SW_SHOWNA` は foreground を奪わずに表示する (= `WS_EX_NOACTIVATE` の意図と整合)。
    /// region は `SetWindowRgn` 状態が保持されるので、show 後の次フレームの
    /// `apply_regions` (hash gate) が現行 UI rect を再適用する。
    pub(super) fn set_visible(&self, visible: bool) {
        self.assert_owner_thread();
        if self.hwnd.0.is_null() {
            return;
        }
        unsafe {
            let _ = crate::presentation_observer::show_window(
                self.hwnd,
                if visible { SW_SHOWNA } else { SW_HIDE },
                crate::presentation_observer::WindowRole::Hud,
                "HudOverlayWindow::set_visible",
            );
        }
    }

    /// HUD HWND を VST GUI より前面に上げ直す。retry burst の各回で呼ばれる。
    pub(super) fn raise_to_top(&self) {
        self.assert_owner_thread();
        if self.hwnd.0.is_null() {
            return;
        }
        unsafe {
            let _ = crate::presentation_observer::set_window_pos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
                crate::presentation_observer::WindowRole::Hud,
                "HudOverlayWindow::raise_to_top",
            );
        }
    }

    /// HUD HWND の物理形状を `regions` に合わせて `SetWindowRgn` で更新。
    /// regions が空なら空 HRGN を渡す (= HUD は「どこにも存在しない」、Codex 6 P1 #2)。
    /// **前回と同じ rect 集合のときは `SetWindowRgn` を呼ばずに skip** する
    /// (= hash 比較、Codex CP2 P3 反映)。CP5 で毎フレーム呼ばれても idempotent。
    /// HRGN 所有権: `SetWindowRgn` 成功時は OS が HRGN を所有 (= `DeleteObject` 不要)、
    /// 失敗時のみ自分で破棄 (Codex 7 P3 #6)。
    ///
    /// 共有 `regions: Arc<Mutex<HudInteractiveRegions>>` (= `WM_NCHITTEST`
    /// フェイルセーフ用) の更新は呼び出し側の責務 (`NativeEguiOverlay::run` 末尾)。
    pub(super) fn apply_regions(&mut self, regions: &[RECT]) {
        self.assert_owner_thread();
        if self.hwnd.0.is_null() {
            return;
        }
        let new_hash = hash_regions(regions);
        if self.last_regions_hash == Some(new_hash) {
            return; // 前回と同じ rect 集合 → SetWindowRgn skip
        }
        // Codex CP2 (再) P2 反映: 成功時のみ hash を更新する。失敗時は
        // `last_regions_hash` を `None` にリセットし、次回 (= 同じ regions が来ても)
        // skip せずに必ず再試行できるようにする。
        if apply_window_region(self.hwnd, regions) {
            self.last_regions_hash = Some(new_hash);
        } else {
            self.last_regions_hash = None;
        }
    }
}

impl Drop for HudOverlayWindow {
    fn drop(&mut self) {
        self.assert_owner_thread();
        if !self.hwnd.0.is_null() {
            unsafe {
                if IsWindow(Some(self.hwnd)).as_bool() {
                    let _ = crate::presentation_observer::destroy_window(
                        self.hwnd,
                        crate::presentation_observer::WindowRole::Hud,
                        "HudOverlayWindow::drop",
                    );
                }
            }
        }
        crate::presentation_observer::unregister(
            crate::presentation_observer::WindowRole::Hud,
            self.hwnd.0 as usize as u64,
        );
        // state_ptr は WM_NCDESTROY で Box::from_raw して drop される。
    }
}

// -----------------------------------------------------------------------------
// 内部
// -----------------------------------------------------------------------------

/// Win32 window class 名 (NUL 終端 UTF-16)。`w!()` マクロが `\0` を末尾に
/// 自動付与する。`PCWSTR` 互換のリテラルとして `RegisterClassExW` /
/// `CreateWindowExW` 双方に同じ値を渡す (Codex CP2 P1 反映)。
const HUD_CLASS_NAME: PCWSTR = w!("mIVHudOverlay");

/// Mouse button tracking bitset.
const BTN_LEFT: u8 = 1 << 0;
const BTN_RIGHT: u8 = 1 << 1;
const BTN_MIDDLE: u8 = 1 << 2;
const BTN_X1: u8 = 1 << 3;
const BTN_X2: u8 = 1 << 4;

struct WindowState {
    event_sink: NativeVideoWindowEventSink,
    regions: Arc<std::sync::Mutex<HudInteractiveRegions>>,
    /// 現在押下中のマウスボタン (`BTN_*` の OR)。`WM_CAPTURECHANGED` 等で残っていたら
    /// synthetic up を補完する。
    held_buttons: u8,
    /// Whole-stream `PT_TOUCH` ownership scoped to this HUD HWND.
    touch_ownership: NativeTouchOwnership,
    /// `TrackMouseEvent(TME_LEAVE)` 登録済みフラグ。
    mouse_tracking: bool,
    /// CP9 実機 debug: HUD wndproc が WM_MOUSEMOVE を log した直近時刻。100ms 周期で
    /// 1 回だけ log する rate limit 用。
    last_mouse_move_log_at: Option<std::time::Instant>,
}

fn register_window_class() -> Result<(), String> {
    // T31 (Codex P2 / 2026-05-16): 旧 `Once + static mut RESULT` を `OnceLock` 化。
    // `static mut` の `static_mut_refs` lint 抑制を解除し、結果クローンも `get_or_init`
    // 経由で safe Rust になる。挙動 (1 回登録 + ERROR_CLASS_ALREADY_EXISTS 許容) は不変。
    use std::sync::OnceLock;
    static REGISTER_RESULT: OnceLock<Result<(), String>> = OnceLock::new();
    REGISTER_RESULT
        .get_or_init(|| unsafe {
            let hmodule = match GetModuleHandleW(None) {
                Ok(h) => h,
                Err(err) => {
                    return Err(format!("GetModuleHandleW for HUD class: {err:?}"));
                }
            };
            // 実機修正 (2026-05-12): カーソル消失バグ対策として `hCursor = IDC_ARROW`
            // を明示設定 (Codex 助言 #3 反映)。`hCursor` が未設定だと、HUD region 内に
            // cursor が入ったとき OS が「cursor 未指定」と判断して非表示にすることがある。
            // WM_SETCURSOR のフォールバックと併用 (double-safety)。
            let class_cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
                lpfnWndProc: Some(hud_wnd_proc),
                hInstance: hmodule.into(),
                lpszClassName: HUD_CLASS_NAME,
                hCursor: class_cursor,
                ..Default::default()
            };
            if RegisterClassExW(&class) == 0 {
                let err = std::io::Error::last_os_error();
                // ERROR_CLASS_ALREADY_EXISTS = 1410 は無視 (= 別 thread 由来等)。
                if err.raw_os_error() != Some(1410) {
                    return Err(format!("RegisterClassExW HUD: {err:?}"));
                }
            }
            Ok(())
        })
        .clone()
}

/// `SetWindowRgn` の wrap。空 `regions` は `CreateRectRgn(0, 0, 0, 0)` 1 個の region を
/// 渡す (= HUD は「どこにも存在しない」、Codex 6 P1 #2)。HRGN 所有権:
/// `SetWindowRgn` 成功時は OS 所有なので `DeleteObject` しない (Codex 7 P3 #6)。
///
/// 戻り値: `SetWindowRgn` が成功して OS 側 region が新しい状態に切り替わったかどうか
/// (Codex CP2 (再) P2 反映)。失敗時は OS 側 region が旧状態のままなので、呼び出し側で
/// hash cache を更新せず次回必ず再試行すること。
#[must_use]
fn apply_window_region(hwnd: HWND, regions: &[RECT]) -> bool {
    if hwnd.0.is_null() {
        return false;
    }
    unsafe {
        let hrgn: HRGN = if regions.is_empty() {
            CreateRectRgn(0, 0, 0, 0)
        } else {
            let combined = CreateRectRgn(
                regions[0].left,
                regions[0].top,
                regions[0].right,
                regions[0].bottom,
            );
            for rect in regions.iter().skip(1) {
                let next = CreateRectRgn(rect.left, rect.top, rect.right, rect.bottom);
                let _ = CombineRgn(Some(combined), Some(combined), Some(next), RGN_OR);
                // next は CombineRgn 後に不要なので破棄。OS には combined だけ渡す。
                let _ = DeleteObject(next.into());
            }
            combined
        };
        if hrgn.is_invalid() {
            // CreateRectRgn が NULL を返した (= GDI handle 枯渇等)。
            // 呼び出し側に失敗を伝える。
            return false;
        }
        let result = SetWindowRgn(hwnd, Some(hrgn), false);
        if result == 0 {
            // SetWindowRgn 失敗時は OS が region を所有しないので自分で破棄。
            let _ = DeleteObject(hrgn.into());
            false
        } else {
            // 成功時は OS が所有するので何もしない。
            true
        }
    }
}

/// CP9 実機 debug: `RECT` slice hash を外部から呼べる版 (`presenter::publish_hud_regions`
/// の重複抑制用)。内部 `hash_regions` と同じ実装。
pub(super) fn hash_regions_for_debug(regions: &[RECT]) -> u64 {
    hash_regions(regions)
}

/// `RECT` のスライスを 64bit hash に潰す。順序を含めて等価性判定する
/// (= 並び順が違うだけでも別 region と扱う、再順序で `SetWindowRgn` が変わるため)。
/// `RECT` は `Hash` を実装していないので各 field を順に hash する。
fn hash_regions(regions: &[RECT]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    regions.len().hash(&mut h);
    for r in regions {
        r.left.hash(&mut h);
        r.top.hash(&mut h);
        r.right.hash(&mut h);
        r.bottom.hash(&mut h);
    }
    h.finish()
}

fn signed_low_word(value: isize) -> i32 {
    (value as i16) as i32
}

fn signed_high_word(value: isize) -> i32 {
    ((value >> 16) as i16) as i32
}

fn mouse_shift(wparam: WPARAM) -> bool {
    (wparam.0 & 0x0004) != 0
}

fn mouse_ctrl(wparam: WPARAM) -> bool {
    (wparam.0 & 0x0008) != 0
}

fn button_bit_for_msg(msg: u32, wparam: WPARAM) -> (NativeVideoMouseButton, u8) {
    match msg {
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
            (NativeVideoMouseButton::Left, BTN_LEFT)
        }
        WM_RBUTTONDOWN | WM_RBUTTONUP | WM_RBUTTONDBLCLK => {
            (NativeVideoMouseButton::Right, BTN_RIGHT)
        }
        WM_MBUTTONDOWN | WM_MBUTTONUP | WM_MBUTTONDBLCLK => {
            (NativeVideoMouseButton::Middle, BTN_MIDDLE)
        }
        WM_XBUTTONDOWN | WM_XBUTTONUP | WM_XBUTTONDBLCLK => {
            match ((wparam.0 >> 16) & 0xFFFF) as u16 {
                2 => (NativeVideoMouseButton::Extra2, BTN_X2),
                _ => (NativeVideoMouseButton::Extra1, BTN_X1),
            }
        }
        _ => (NativeVideoMouseButton::Left, BTN_LEFT),
    }
}

fn mouse_message_is_down(msg: u32) -> bool {
    matches!(
        msg,
        WM_LBUTTONDOWN
            | WM_RBUTTONDOWN
            | WM_MBUTTONDOWN
            | WM_XBUTTONDOWN
            | WM_LBUTTONDBLCLK
            | WM_RBUTTONDBLCLK
            | WM_MBUTTONDBLCLK
            | WM_XBUTTONDBLCLK
    )
}

fn track_mouse_leave(hwnd: HWND, state: &mut WindowState) -> bool {
    if state.mouse_tracking {
        return true;
    }
    let mut tme = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    let registered = unsafe { TrackMouseEvent(&mut tme).is_ok() };
    state.mouse_tracking = registered;
    registered
}

/// 現在 held_buttons に残っているボタンの synthetic up を補完し、`MouseLeave` も流す。
/// `WM_CAPTURECHANGED` / `WM_CANCELMODE` / `WM_DESTROY` 共通 cleanup。
fn emit_synthetic_button_cleanup(state: &mut WindowState) {
    let held = state.held_buttons;
    state.held_buttons = 0;

    if (held & BTN_LEFT) != 0 {
        state.event_sink.send(NativeVideoWindowEvent::MouseButton(
            NativeVideoMouseButtonEvent {
                button: NativeVideoMouseButton::Left,
                down: false,
                double_click: false,
                x: 0,
                y: 0,
                shift: false,
                ctrl: false,
            },
        ));
    }
    if (held & BTN_RIGHT) != 0 {
        state.event_sink.send(NativeVideoWindowEvent::MouseButton(
            NativeVideoMouseButtonEvent {
                button: NativeVideoMouseButton::Right,
                down: false,
                double_click: false,
                x: 0,
                y: 0,
                shift: false,
                ctrl: false,
            },
        ));
    }
    if (held & BTN_MIDDLE) != 0 {
        state.event_sink.send(NativeVideoWindowEvent::MouseButton(
            NativeVideoMouseButtonEvent {
                button: NativeVideoMouseButton::Middle,
                down: false,
                double_click: false,
                x: 0,
                y: 0,
                shift: false,
                ctrl: false,
            },
        ));
    }
    if (held & BTN_X1) != 0 {
        state.event_sink.send(NativeVideoWindowEvent::MouseButton(
            NativeVideoMouseButtonEvent {
                button: NativeVideoMouseButton::Extra1,
                down: false,
                double_click: false,
                x: 0,
                y: 0,
                shift: false,
                ctrl: false,
            },
        ));
    }
    if (held & BTN_X2) != 0 {
        state.event_sink.send(NativeVideoWindowEvent::MouseButton(
            NativeVideoMouseButtonEvent {
                button: NativeVideoMouseButton::Extra2,
                down: false,
                double_click: false,
                x: 0,
                y: 0,
                shift: false,
                ctrl: false,
            },
        ));
    }

    // CP9 実機修正: capture 喪失 cleanup でも `MouseLeave` は流さない。
    // 同じ振動ループ理由 (上の `WM_MOUSELEAVE` ハンドラ コメント参照)。
    // synthetic up を流せば egui の `pointer.any_down()` は false に戻るので drag stuck は解消。
    state.mouse_tracking = false;
}

/// Ends both input transports at the HUD HWND lifecycle boundary.
///
/// Mouse capture cleanup synthesizes missing button-up events; touch cleanup
/// emits Cancel for every owned stream before releasing the per-HWND set.
fn emit_input_cleanup(state: &mut WindowState) {
    let WindowState {
        event_sink,
        touch_ownership,
        ..
    } = state;
    cancel_hud_touch_streams(touch_ownership, event_sink);
    emit_synthetic_button_cleanup(state);
}

fn window_state(hwnd: HWND) -> Option<&'static mut WindowState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &mut *ptr })
    }
}

unsafe extern "system" fn hud_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    log_win32_message(TouchDebugWindow::Hud, hwnd, msg, wparam, lparam);
    if let Some(state) = window_state(hwnd) {
        let WindowState {
            event_sink,
            touch_ownership,
            ..
        } = state;
        if let Some(result) =
            handle_hud_pointer_message(hwnd, msg, wparam, touch_ownership, event_sink)
        {
            return result;
        }
    }
    match msg {
        WM_NCCREATE => {
            let createstruct = lparam.0 as *const CREATESTRUCTW;
            if !createstruct.is_null() {
                let state = unsafe { (*createstruct).lpCreateParams } as *mut WindowState;
                if !state.is_null() {
                    unsafe {
                        let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
                    }
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),

        // 実機修正 (2026-05-12 Codex P2 #6 反映): cursor 非表示状態の扱い。
        //
        // 旧版 (P2 #6 修正前): TRUE を返すだけ + SetCursor 不要 → 直近設定の cursor 維持。
        // これは egui の ResizeHorizontal 等が WM_SETCURSOR で上書きされない利点はあるが、
        // **直前が `SetCursor(None)` (= idle で auto-hide) だと cursor 非表示が残る** バグ。
        //
        // 現在の方針 (2026-06-06): WM_SETCURSOR では復帰も SetCursor もせず `LRESULT(1)` を返す
        // だけ (= DefWindowProc がクラスカーソルを出すのを防ぎ、直近 SetCursor を維持)。
        // presenter 側の対になる方針は `native_window.rs` の WM_SETCURSOR branch に置く。
        // navigation preview は source swap 中に HUD region を全画面化するため、キー操作だけでも
        // 静止 cursor の下へ HUD HWND が広がり WM_SETCURSOR / zero-delta WM_MOUSEMOVE が発生する。
        // ここ (や WM_MOUSEMOVE) で復帰すると「↓キーで次の動画に行くだけで cursor が復活」する。
        // 実カーソルアイコンは pump-owned reducer が local input ownership を確認して駆動する。
        // ownership を失った後は外部 window の WM_SETCURSOR に任せ、ここでは書き込まない。
        WM_SETCURSOR => {
            let hit_test = signed_low_word(lparam.0);
            let trigger_message = ((lparam.0 as u32 >> 16) & 0xffff) as u16;
            let result = LRESULT(1);
            crate::video::cursor_debug::log(format_args!(
                "layer=win32 event=WM_SETCURSOR window=hud hwnd=0x{:016X} cursor_hwnd=0x{:016X} hit_test={hit_test} trigger_message=0x{trigger_message:04X} handler=explicit returned={}",
                hwnd.0 as usize as u64, wparam.0 as u64, result.0,
            ));
            result
        }

        WM_NCHITTEST => {
            // SetWindowRgn で region 外は OS が下層に転送するため通常ここまで来ない。
            // フェイルセーフ: regions に含まれるか確認して HTCLIENT / HTTRANSPARENT。
            let x = signed_low_word(lparam.0);
            let y = signed_high_word(lparam.0);
            let mut pt = POINT { x, y };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut pt);
            }
            let hit = window_state(hwnd)
                .and_then(|s| s.regions.lock().ok().map(|g| g.contains(pt.x, pt.y)))
                .unwrap_or(false);
            if hit {
                LRESULT(HTCLIENT as isize)
            } else {
                LRESULT(HTTRANSPARENT as isize)
            }
        }

        WM_MOUSEMOVE => {
            crate::video::cursor_debug::log(format_args!(
                "layer=win32 event=WM_MOUSEMOVE window=hud hwnd=0x{:016X} client_px=({}, {})",
                hwnd.0 as usize as u64,
                signed_low_word(lparam.0),
                signed_high_word(lparam.0),
            ));
            if should_discard_promoted_touch_mouse(msg, NativeVideoWindowSource::Hud) {
                return LRESULT(0);
            }
            if let Some(state) = window_state(hwnd) {
                // `WM_MOUSEMOVE` ではここで cursor を復帰しない。navigation preview の HUD
                // 全画面化で「カーソル下の window」が presenter HWND ⇄ HUD HWND に切り替わると、
                // OS は **位置不変 (zero-delta) の `WM_MOUSEMOVE`** を新しい window に届ける。これで
                // この move 単体で cursor を復帰させると、キー操作だけの動画ナビで auto-hide
                // 済みカーソルが復活してしまう (2026-06-06)。pump-owned reducer は前回座標と
                // 比較し、zero-delta handoff では activity clock と hidden 状態を維持する。
                let event = NativeVideoMouseEvent {
                    x: signed_low_word(lparam.0),
                    y: signed_high_word(lparam.0),
                    shift: mouse_shift(wparam),
                    ctrl: mouse_ctrl(wparam),
                };
                // CP9 実機 debug: 100ms 周期で 1 回 log。
                if super::hud_debug_enabled() {
                    let now = std::time::Instant::now();
                    let should_log = state
                        .last_mouse_move_log_at
                        .map(|t| now.duration_since(t) >= std::time::Duration::from_millis(100))
                        .unwrap_or(true);
                    if should_log {
                        state.last_mouse_move_log_at = Some(now);
                        crate::logger::log(format!(
                            "[HUD-DEBUG] WM_MOUSEMOVE x={} y={}",
                            event.x, event.y
                        ));
                    }
                }
                state
                    .event_sink
                    .send(NativeVideoWindowEvent::MouseMove(event));
                if !track_mouse_leave(hwnd, state) {
                    state
                        .event_sink
                        .send(NativeVideoWindowEvent::CursorOwnership(
                            NativeCursorOwnershipEdge::TrackingFailed,
                        ));
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_MOUSEWHEEL => {
            if should_discard_promoted_touch_mouse(msg, NativeVideoWindowSource::Hud) {
                return LRESULT(0);
            }
            if let Some(state) = window_state(hwnd) {
                // WM_MOUSEWHEEL は screen coordinates。client に変換。
                let mut pt = POINT {
                    x: signed_low_word(lparam.0),
                    y: signed_high_word(lparam.0),
                };
                unsafe {
                    use windows::Win32::Graphics::Gdi::ScreenToClient;
                    let _ = ScreenToClient(hwnd, &mut pt);
                }
                let event = NativeVideoMouseWheelEvent {
                    delta: signed_high_word(wparam.0 as isize) as i16,
                    x: pt.x,
                    y: pt.y,
                    shift: mouse_shift(wparam),
                    ctrl: mouse_ctrl(wparam),
                };
                state
                    .event_sink
                    .send(NativeVideoWindowEvent::MouseWheel(event));
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_MOUSELEAVE => {
            // Sending this message already consumed the HWND's `TME_LEAVE`
            // registration, so the mirror must be cleared before any policy
            // that can return early. `track_mouse_leave` skips re-registration
            // while the flag is set, so a discarded leave would otherwise stop
            // every later leave for the life of this HWND.
            if let Some(state) = window_state(hwnd) {
                state.mouse_tracking = false;
            }
            if should_discard_promoted_touch_mouse(msg, NativeVideoWindowSource::Hud) {
                return LRESULT(0);
            }
            if super::hud_debug_enabled() {
                crate::logger::log(
                    "[HUD-DEBUG] WM_MOUSELEAVE (ignored, not forwarded)".to_string(),
                );
            }
            if let Some(state) = window_state(hwnd) {
                state
                    .event_sink
                    .send(NativeVideoWindowEvent::CursorOwnership(
                        NativeCursorOwnershipEdge::Leave,
                    ));
                // CP9 実機修正: HUD wndproc の `WM_MOUSELEAVE` を overlay に流さない。
                //
                // 問題: HUD HWND の region は `compute_hud_regions` 結果で頻繁に変化する。
                // region が縮むと cursor が「window から離れた」扱いになり OS が `WM_MOUSELEAVE`
                // を送るが、cursor 自体は presenter HWND client rect 内にいる。これを overlay に
                // 流すと `pointer_pos = None` → `top_bar_visible = false` → region 再縮小 →
                // また外扱い → ... という振動ループを起こす (実機で右上ホバーで点滅、VST ボタンや
                // seek bar が反応しない原因)。
                //
                // egui 側の真の leave は presenter HWND wndproc が流す。HUD 経路では cursor
                // ownership edge だけを pump へ送り、generic MouseLeave は静かにしておく。
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN | WM_RBUTTONUP
        | WM_RBUTTONDBLCLK | WM_MBUTTONDOWN | WM_MBUTTONUP | WM_MBUTTONDBLCLK | WM_XBUTTONDOWN
        | WM_XBUTTONUP | WM_XBUTTONDBLCLK => {
            if should_discard_promoted_touch_mouse(msg, NativeVideoWindowSource::Hud) {
                return LRESULT(0);
            }
            if let Some(state) = window_state(hwnd) {
                let (button, bit) = button_bit_for_msg(msg, wparam);
                let down = mouse_message_is_down(msg);
                let dbl = matches!(
                    msg,
                    WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK | WM_MBUTTONDBLCLK | WM_XBUTTONDBLCLK
                );
                let x = signed_low_word(lparam.0);
                let y = signed_high_word(lparam.0);
                let shift = mouse_shift(wparam);
                let ctrl = mouse_ctrl(wparam);

                // CP9 実機 debug: button event は rare なので毎回 log。
                if super::hud_debug_enabled() {
                    crate::logger::log(format!(
                        "[HUD-DEBUG] WM_*BUTTON{} button={:?} x={} y={} dbl={}",
                        if down { "DOWN" } else { "UP" },
                        button,
                        x,
                        y,
                        dbl,
                    ));
                }
                // 1. down/up event を bounded route に流す。
                state.event_sink.send(NativeVideoWindowEvent::MouseButton(
                    NativeVideoMouseButtonEvent {
                        button,
                        down,
                        double_click: dbl,
                        x,
                        y,
                        shift,
                        ctrl,
                    },
                ));

                if down {
                    // 2. capture 成否に関係なく必ず tracking (Codex 11 P1 #1)。
                    state.held_buttons |= bit;
                    // 3. focus handoff は wndproc 内で実行せず pump task に enqueue する。
                    state
                        .event_sink
                        .send(NativeVideoWindowEvent::RequestFocusClaim);
                    // 4. SetCapture(hud_hwnd) で region 外の up も拾えるようにする。
                    let prev_capture = unsafe { SetCapture(hwnd) };
                    let _ = prev_capture;
                    // 5. capture 失敗チェック。**即時 synthetic up は流さない** (Codex CP9 実機 P1 #2 反映):
                    //    旧実装は capture 失敗時に即 synthetic up を流していたが、down と up が
                    //    同フレームに egui に届いて seek drag が完成しない問題があった (= 実機で seek
                    //    操作が反応しない原因)。capture を取れていなくても down event は流しているので
                    //    egui は click 判定する。cursor が region 外で up したケースは
                    //    `WM_CAPTURECHANGED` / `WM_CANCELMODE` 経路の cleanup で synthetic up が流れる
                    //    (= held_buttons は立てたままなので確実に補完される)。
                    let cur = unsafe { GetCapture() };
                    if cur.0 != hwnd.0 {
                        crate::logger::log(format!(
                            "[HUD] SetCapture failed: got={:p} expected={:p} button={:?}",
                            cur.0, hwnd.0, button
                        ));
                    }
                } else {
                    // up: held_buttons から該当 bit をクリアして ReleaseCapture。
                    state.held_buttons &= !bit;
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                }
            }
            // WM_XBUTTONUP は MouseButton(Extra1/Extra2) で既に進む/戻るを処理済み。
            // DefWindowProc に流すと Windows が APPCOMMAND_BROWSER_BACKWARD/FORWARD を
            // 合成し、本ファイル下の WM_APPCOMMAND handler が再度 KeyDown(0xA6/0xA7) を
            // 生成して 1 押下 = 2 ナビになるため、TRUE を返して抑止 (Codex 2 周目 P2)。
            if msg == WM_XBUTTONUP {
                return LRESULT(1);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_APPCOMMAND => {
            // 通常は HUD の WM_XBUTTONUP 後に `DefWindowProcW` が APPCOMMAND を生成し、
            // 親 (presenter) HWND へ昇格して presenter wndproc 側で拾われる。ただし
            // pathological case として、mouse driver が HUD HWND へ `SendMessage(WM_APPCOMMAND, ...)`
            // を直接送ってくる経路がありうる (= HUD は sibling top-level なので親への
            // 自動 forward は起きない)。その際にも UI 側にナビゲーションを届けるため、
            // ここで合成 KeyDown(0xA6/0xA7) に変換して bounded route に流す
            // (= presenter 側のハンドラと同一の経路、Codex P2)。
            let cmd_word = ((lparam.0 >> 16) & 0xFFFF) as u32;
            let app_command = cmd_word & 0xFFF;
            let synth_vk = match app_command {
                1 => Some(0xA6_u32), // APPCOMMAND_BROWSER_BACKWARD → VK_BROWSER_BACK
                2 => Some(0xA7_u32), // APPCOMMAND_BROWSER_FORWARD  → VK_BROWSER_FORWARD
                _ => None,
            };
            if let Some(vk) = synth_vk
                && let Some(state) = window_state(hwnd)
            {
                state
                    .event_sink
                    .send(NativeVideoWindowEvent::KeyDown(NativeVideoKeyEvent {
                        virtual_key: vk,
                        scan_code: 0,
                        extended: false,
                        shift: false,
                        ctrl: false,
                        alt: false,
                        repeat: false,
                    }));
                // WM_APPCOMMAND の規約: 処理した場合 TRUE を返す。
                return LRESULT(1);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_CAPTURECHANGED | WM_CANCELMODE => {
            if super::hud_debug_enabled() {
                let held = window_state(hwnd).map(|s| s.held_buttons).unwrap_or(0);
                crate::logger::log(format!(
                    "[HUD-DEBUG] {} held_buttons=0x{:02x}",
                    if msg == WM_CAPTURECHANGED {
                        "WM_CAPTURECHANGED"
                    } else {
                        "WM_CANCELMODE"
                    },
                    held
                ));
            }
            if let Some(state) = window_state(hwnd) {
                state
                    .event_sink
                    .send(NativeVideoWindowEvent::CursorOwnership(
                        NativeCursorOwnershipEdge::CaptureLost,
                    ));
                emit_input_cleanup(state);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_WINDOWPOSCHANGING => {
            // `hwndInsertAfter` が自分より前を指す変更 (= 別 window が割り込む) を
            // 検知したら raise 要求を流す。完全な判定ではなく best-effort safety net。
            // (主経路は z-order 操作後 hook と presenter polling で吸収するので緩めに。)
            //
            // Codex CP2 P2 反映: 旧コード `if !lparam.0 == 0` は bitwise NOT 比較で
            // ほぼ常に false。`if lparam.0 != 0` (= lparam が valid pointer) が正しい。
            if lparam.0 != 0 {
                let wp = lparam.0 as *const WINDOWPOS;
                if !wp.is_null() {
                    let insert_after = unsafe { (*wp).hwndInsertAfter };
                    // HWND_TOP (= 0) は「先頭に挿入」、HWND_TOPMOST (= -1) は「TOPMOST の先頭」。
                    // どちらも HUD より前なので raise を要求。
                    // HWND_BOTTOM (= 1) や HWND_NOTOPMOST (= -2) は HUD を後ろに送るので除外。
                    let raw = insert_after.0 as isize;
                    if raw == 0 || raw == -1 {
                        if let Some(state) = window_state(hwnd) {
                            state
                                .event_sink
                                .send(NativeVideoWindowEvent::RequestRaiseHud);
                        }
                    }
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_DPICHANGED => {
            // wparam: LOWORD = new X DPI、HIWORD = new Y DPI。lparam: 推奨新 RECT へのポインタ。
            let dpi = (wparam.0 & 0xFFFF) as u32;
            let suggested_rect = if lparam.0 != 0 {
                let r = lparam.0 as *const RECT;
                if !r.is_null() {
                    unsafe { *r }
                } else {
                    RECT::default()
                }
            } else {
                RECT::default()
            };
            if let Some(state) = window_state(hwnd) {
                state.event_sink.send(NativeVideoWindowEvent::DpiChanged {
                    dpi,
                    suggested_rect,
                });
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_DESTROY => {
            // capture 中だったボタンの synthetic up と touch Cancel を補完してから
            // state を残す (`WM_NCDESTROY` で Box drop)。
            if let Some(state) = window_state(hwnd) {
                state
                    .event_sink
                    .send(NativeVideoWindowEvent::CursorOwnership(
                        NativeCursorOwnershipEdge::Leave,
                    ));
                emit_input_cleanup(state);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_NCDESTROY => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                unsafe {
                    emit_input_cleanup(&mut *ptr);
                    let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    let _ = Box::from_raw(ptr);
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
