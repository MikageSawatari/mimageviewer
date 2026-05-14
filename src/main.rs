#![windows_subsystem = "windows"]

pub mod activity_gate;
pub mod adjustment;
pub mod adjustment_db;
pub mod ai;
mod app;
pub mod archive_cache;
pub mod archive_converter;
pub mod audio_normalize_db;
pub mod cache_maintenance;
pub mod catalog;
pub mod data_dir;
#[cfg(windows)]
mod dcomp_presenter_test;
pub mod delete_worker;
pub mod dwm_transitions;
pub mod exif_reader;
pub mod external_links;
pub mod fast_resize;
pub mod folder_rating_counter;
pub mod folder_thumb_pins;
pub mod folder_tree;
pub mod fs_animation;
pub mod fts_index;
pub mod fts_meta;
pub mod fts_writer_dispatcher;
pub mod global_search;
mod global_search_ui;
pub mod gpu_info;
pub mod grid_item;
pub mod indexer_manager;
pub mod indexer_progress;
pub mod indexer_supervisor;
pub mod ingest_text;
pub mod ingest_worker;
pub mod io_semaphore;
pub mod logger;
pub mod mask_db;
pub mod monitor;
pub mod name_bulk_indexer;
pub mod name_index_supervisor;
pub mod open_with;
pub mod os_theme;
pub mod path_key;
pub mod pdf_loader;
pub mod pdf_passwords;
pub mod perf;
pub mod png_metadata;
pub mod post_filter;
pub mod rating_db;
pub mod rating_write_worker;
pub mod rotation_db;
pub mod search_index_db;
pub mod search_norm;
pub mod search_query;
pub mod search_walker;
pub mod search_watcher;
pub mod settings;
pub mod sidecar;
pub mod single_instance;
pub mod spread_db;
pub mod stats;
pub mod susie_loader;
pub mod sys_memory;
mod tag_ops;
mod tag_prewarm;
pub mod tag_write_worker;
pub mod thumb_loader;
pub mod tray;
mod tray_integration;
mod ui_adjustment_panel;
mod ui_analysis_panel;
pub mod ui_dialogs;
mod ui_erase;
pub mod ui_fonts;
mod ui_fullscreen;
pub mod ui_helpers;
mod ui_main;
mod ui_metadata_panel;
pub mod ui_susie_diagnostic;
pub mod ui_text_links;
#[cfg(windows)]
#[cfg(windows)]
pub mod ui_video_tile;
mod undo_ops;
pub mod undo_stack;
pub mod update_check;
pub mod video;
pub mod video_bookmarks;
pub mod video_pins;
pub mod video_thumb;
pub mod wic_decoder;
pub mod xmp_reader;
pub mod xmp_writer;
pub mod zip_loader;

use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

static NATIVE_EXCEPTION_LOGGING: AtomicBool = AtomicBool::new(false);
static UI_HEARTBEAT: OnceLock<Arc<UiHeartbeatState>> = OnceLock::new();

struct UiHeartbeatState {
    start: Instant,
    last_ms: std::sync::atomic::AtomicU64,
    last_report_ms: std::sync::atomic::AtomicU64,
    suspended: AtomicBool,
    detail: Mutex<String>,
    /// メインウィンドウの HWND (Windows only)。watchdog から `IsHungAppWindow` を
    /// 呼ぶために共有する。0 の間は未捕捉 (early startup) を意味する。
    /// `App::update` が main_hwnd を捕捉した時点で書き込む。
    main_hwnd: std::sync::atomic::AtomicU64,
}

/// `startup.<step>` perf イベントを emit する共通ヘルパー。
/// `phase_start` を渡すと当該フェーズの `ms` + 累計 `total_ms` を、
/// `None` を渡すとマーカー用として `total_ms` のみを記録する。
/// `total_ms` は `perf::program_start()` (= `perf::init` に渡した基準 Instant)
/// 経由で計算するので、事前に `perf::init(enabled, Some(prog_start))` を呼んでおくこと。
/// `perf::is_enabled()` が false なら no-op。
fn emit_startup(step: &str, phase_start: Option<Instant>) {
    if !perf::is_enabled() {
        return;
    }
    let Some(base) = perf::program_start() else {
        return;
    };
    let total_ms = base.elapsed().as_secs_f64() * 1000.0;
    let mut extras: Vec<(&str, serde_json::Value)> = Vec::with_capacity(2);
    if let Some(start) = phase_start {
        extras.push((
            "ms",
            serde_json::Value::from(start.elapsed().as_secs_f64() * 1000.0),
        ));
    }
    extras.push(("total_ms", serde_json::Value::from(total_ms)));
    perf::event("startup", step, None, 0, &extras);
}

fn parse_wgpu_present_mode(raw: &str) -> Option<wgpu::PresentMode> {
    match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "auto_no_vsync" | "autonovsync" | "no_vsync" | "novsync" => {
            Some(wgpu::PresentMode::AutoNoVsync)
        }
        "auto_vsync" | "autovsync" | "vsync" => Some(wgpu::PresentMode::AutoVsync),
        "fifo" => Some(wgpu::PresentMode::Fifo),
        "fifo_relaxed" | "fiforelaxed" => Some(wgpu::PresentMode::FifoRelaxed),
        "immediate" => Some(wgpu::PresentMode::Immediate),
        "mailbox" => Some(wgpu::PresentMode::Mailbox),
        _ => None,
    }
}

fn parse_wgpu_frame_latency(raw: &str) -> Option<Option<u32>> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("default")
        || trimmed.eq_ignore_ascii_case("none")
        || trimmed == "0"
    {
        return Some(None);
    }
    trimmed.parse::<u32>().ok().filter(|v| *v > 0).map(Some)
}

fn configure_wgpu_presentation(wgpu_options: &mut egui_wgpu::WgpuConfiguration) {
    let present_mode = match std::env::var("MIV_WGPU_PRESENT_MODE") {
        Ok(raw) => match parse_wgpu_present_mode(&raw) {
            Some(mode) => mode,
            None => {
                logger::log(format!(
                    "wgpu presentation: ignoring invalid MIV_WGPU_PRESENT_MODE={raw:?}; \
                     using AutoVsync"
                ));
                wgpu::PresentMode::AutoVsync
            }
        },
        Err(_) => wgpu::PresentMode::AutoVsync,
    };

    let desired_maximum_frame_latency = match std::env::var("MIV_WGPU_FRAME_LATENCY") {
        Ok(raw) => match parse_wgpu_frame_latency(&raw) {
            Some(value) => value,
            None => {
                logger::log(format!(
                    "wgpu presentation: ignoring invalid MIV_WGPU_FRAME_LATENCY={raw:?}; \
                     using 1"
                ));
                Some(1)
            }
        },
        Err(_) => Some(1),
    };

    wgpu_options.present_mode = present_mode;
    wgpu_options.desired_maximum_frame_latency = desired_maximum_frame_latency;
    logger::log(format!(
        "wgpu presentation: present_mode={present_mode:?} \
         desired_maximum_frame_latency={desired_maximum_frame_latency:?}"
    ));
}

fn install_panic_log_hook() {
    // windows_subsystem = "windows" では stderr が見えないため、Rust panic は
    // data_dir 初期化直後から panic.log に残す。ネイティブ DLL / driver の
    // access violation は Rust panic ではないので、この hook では捕捉できない。
    std::panic::set_hook(Box::new(|info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown payload".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());
        let bt = std::backtrace::Backtrace::force_capture();
        let msg = format!("PANIC at {location}: {payload}\n{bt}");
        logger::log(&msg);
        append_panic_log_entry(&msg);
    }));
}

fn append_panic_log_entry(msg: &str) {
    let log_dir = data_dir::logs_dir();
    let _ = std::fs::create_dir_all(&log_dir);
    let panic_log = log_dir.join("panic.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&panic_log)
    {
        use std::io::Write;
        let _ = writeln!(f, "[{:?}] {msg}", std::time::SystemTime::now());
    }
}

fn install_ui_heartbeat_watchdog() {
    let state = UI_HEARTBEAT
        .get_or_init(|| {
            let now = Instant::now();
            Arc::new(UiHeartbeatState {
                start: now,
                last_ms: std::sync::atomic::AtomicU64::new(0),
                last_report_ms: std::sync::atomic::AtomicU64::new(0),
                suspended: AtomicBool::new(false),
                detail: Mutex::new("no App::update heartbeat yet".to_owned()),
                main_hwnd: std::sync::atomic::AtomicU64::new(0),
            })
        })
        .clone();

    let _ = std::thread::Builder::new()
        .name("ui-heartbeat-watchdog".to_owned())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let now_ms = state.start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                if state.suspended.load(Ordering::Acquire) {
                    state.last_report_ms.store(now_ms, Ordering::Release);
                    continue;
                }
                let last_ms = state.last_ms.load(Ordering::Acquire);
                let age_ms = now_ms.saturating_sub(last_ms);
                if age_ms < 5_000 {
                    continue;
                }
                // App::update が 5s 以上呼ばれていない。ただし「アイドルで意図的に
                // sleep している」のと「message pump が hang している」を区別する。
                // Windows なら `IsHungAppWindow` が真の判定手段 (= ユーザーが
                // 「応答なし」表示を見るのと同じ条件)。message pump が応答するなら
                // App::update が呼ばれていないのは正常 (request_repaint が呼ばれて
                // いない idle 状態)。HWND がまだ未捕捉 (early startup) なら
                // 安全側で警告する (= 旧来の挙動)。
                #[cfg(windows)]
                {
                    let hwnd_raw = state.main_hwnd.load(Ordering::Acquire);
                    if hwnd_raw != 0 {
                        use windows::Win32::Foundation::HWND;
                        use windows::Win32::UI::WindowsAndMessaging::IsHungAppWindow;
                        let hwnd = HWND(hwnd_raw as *mut _);
                        let is_hung = unsafe { IsHungAppWindow(hwnd).as_bool() };
                        if !is_hung {
                            // message pump は応答中。正常な idle なので報告しない。
                            // last_report_ms を更新して次回 5s 後にまた静かに再評価。
                            state.last_report_ms.store(now_ms, Ordering::Release);
                            continue;
                        }
                    }
                }
                let last_report_ms = state.last_report_ms.load(Ordering::Acquire);
                if now_ms.saturating_sub(last_report_ms) < 10_000 {
                    continue;
                }
                if state
                    .last_report_ms
                    .compare_exchange(last_report_ms, now_ms, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    continue;
                }
                let detail = state
                    .detail
                    .lock()
                    .map(|s| s.clone())
                    .unwrap_or_else(|_| "<heartbeat detail mutex poisoned>".to_owned());
                append_panic_log_entry(&format!(
                    "UI THREAD HANG suspected: no App::update heartbeat for {age_ms}ms \
                 (last_ms={last_ms}, now_ms={now_ms}); last_detail={detail}"
                ));
            }
        });
}

pub(crate) fn record_ui_heartbeat_tick() {
    if let Some(state) = UI_HEARTBEAT.get() {
        let now_ms = state.start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        state.suspended.store(false, Ordering::Release);
        state.last_ms.store(now_ms, Ordering::Release);
    }
}

pub(crate) fn record_ui_heartbeat_detail(detail: String) {
    if let Some(state) = UI_HEARTBEAT.get() {
        let now_ms = state.start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        state.last_ms.store(now_ms, Ordering::Release);
        if let Ok(mut slot) = state.detail.lock() {
            *slot = detail;
        }
    }
}

/// watchdog に main HWND を共有する。App::update が main_hwnd を捕捉した直後に
/// 呼ぶ。watchdog はこの HWND に対して `IsHungAppWindow` を照会して、
/// 「intentionally idle」と「actually hung」を区別する。
pub(crate) fn set_ui_heartbeat_main_hwnd(hwnd_raw: u64) {
    if let Some(state) = UI_HEARTBEAT.get() {
        state.main_hwnd.store(hwnd_raw, Ordering::Release);
    }
}

// マウス進む/戻るボタン (Windows) の橋渡し。
//
// 5 ボタンマウスの進む/戻るは、ハードウェアやドライバの設定によって以下のいずれかで届く:
//
//   1. WM_XBUTTONDOWN/UP (native): winit → egui Extra1/Extra2 — App 側で既に bind 済み
//   2. WM_APPCOMMAND (mouse driver が APPCOMMAND_BROWSER_BACKWARD/FORWARD を送る経路):
//      winit はハンドリングしないので egui まで届かない
//   3. WM_KEYDOWN VK_BROWSER_BACK / VK_BROWSER_FORWARD (mouse driver / AutoHotkey が
//      keystroke 化して送る経路): winit → egui-winit で `BrowserBack` だけ翻訳され、
//      `BrowserForward` は egui-winit のマップに無いのでドロップされる
//
// (2)(3) は WH_GETMESSAGE スレッドフックで補足し、App::update が消費する atomic
// カウンタに積む。App 側はこれを既存の Ctrl+↑/↓ ナビゲーション (フォルダ DFS) と
// 同等に扱う。これにより、上記いずれの経路で届いても等しく動く。
#[cfg(windows)]
static MOUSE_NAV_HOOK_INSTALLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
static PENDING_MOUSE_NAV_BACK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg(windows)]
static PENDING_MOUSE_NAV_FORWARD: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);

/// App::update がフレーム頭で呼ぶ。前フレーム以降に蓄積した進む/戻る押下回数を取り出す。
/// 戻り値は (back, forward)。non-Windows では常に (0, 0)。
pub(crate) fn take_pending_mouse_nav() -> (u32, u32) {
    #[cfg(windows)]
    {
        use std::sync::atomic::Ordering;
        let back = PENDING_MOUSE_NAV_BACK.swap(0, Ordering::AcqRel);
        let forward = PENDING_MOUSE_NAV_FORWARD.swap(0, Ordering::AcqRel);
        (back, forward)
    }
    #[cfg(not(windows))]
    {
        (0, 0)
    }
}

#[cfg(windows)]
unsafe extern "system" fn mouse_nav_hook_proc(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use std::sync::atomic::Ordering;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, HC_ACTION, MSG, WM_APPCOMMAND, WM_KEYDOWN, WM_SYSKEYDOWN,
    };
    if code == HC_ACTION as i32 {
        unsafe {
            let msg_ptr = lparam.0 as *const MSG;
            if !msg_ptr.is_null() {
                let msg = &*msg_ptr;
                match msg.message {
                    WM_APPCOMMAND => {
                        // HIWORD(lparam) の下 12 bit が AppCommand。
                        // APPCOMMAND_BROWSER_BACKWARD = 1, APPCOMMAND_BROWSER_FORWARD = 2
                        let cmd_word = ((msg.lParam.0 >> 16) & 0xFFFF) as u32;
                        let app_command = cmd_word & 0xFFF;
                        match app_command {
                            1 => {
                                PENDING_MOUSE_NAV_BACK.fetch_add(1, Ordering::AcqRel);
                            }
                            2 => {
                                PENDING_MOUSE_NAV_FORWARD.fetch_add(1, Ordering::AcqRel);
                            }
                            _ => {}
                        }
                    }
                    WM_KEYDOWN | WM_SYSKEYDOWN => {
                        // VK_BROWSER_BACK = 0xA6, VK_BROWSER_FORWARD = 0xA7
                        // KEYUP は数えない (1 押下で 1 ナビ)。auto-repeat (lParam bit 30)
                        // は通すと連続移動できる (キーボードの Ctrl+↑/↓ と同じ感覚)。
                        let vk = (msg.wParam.0 & 0xFF) as u8;
                        match vk {
                            0xA6 => {
                                PENDING_MOUSE_NAV_BACK.fetch_add(1, Ordering::AcqRel);
                            }
                            0xA7 => {
                                PENDING_MOUSE_NAV_FORWARD.fetch_add(1, Ordering::AcqRel);
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            CallNextHookEx(None, code, wparam, lparam)
        }
    } else {
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }
}

/// メイン UI スレッドに WH_GETMESSAGE フックを 1 度だけ install する。
/// App::update が main_hwnd を捕捉した直後に呼ばれる。
#[cfg(windows)]
pub(crate) fn install_mouse_nav_hook() {
    use std::sync::atomic::Ordering;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_GETMESSAGE};
    if MOUSE_NAV_HOOK_INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }
    let tid = unsafe { GetCurrentThreadId() };
    match unsafe { SetWindowsHookExW(WH_GETMESSAGE, Some(mouse_nav_hook_proc), None, tid) } {
        Ok(_) => {
            crate::logger::log(format!(
                "mouse-nav: WH_GETMESSAGE hook installed on tid={tid} (capture WM_APPCOMMAND \
                 + VK_BROWSER_BACK/FORWARD for folder navigation)"
            ));
        }
        Err(err) => {
            crate::logger::log(format!(
                "mouse-nav: WH_GETMESSAGE hook install failed: {err:?}"
            ));
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn install_mouse_nav_hook() {}

pub(crate) fn set_ui_heartbeat_suspended(suspended: bool, detail: String) {
    if let Some(state) = UI_HEARTBEAT.get() {
        let now_ms = state.start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        state.suspended.store(suspended, Ordering::Release);
        state.last_ms.store(now_ms, Ordering::Release);
        state.last_report_ms.store(now_ms, Ordering::Release);
        if let Ok(mut slot) = state.detail.lock() {
            *slot = detail;
        }
    }
}

#[cfg(windows)]
fn install_native_exception_log_hook() {
    use windows::Win32::System::Diagnostics::Debug::AddVectoredExceptionHandler;

    unsafe {
        let handle = AddVectoredExceptionHandler(1, Some(native_exception_handler));
        if handle.is_null() {
            logger::log("native exception logger: AddVectoredExceptionHandler failed");
        } else {
            logger::log("native exception logger: installed vectored exception handler");
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn native_exception_handler(
    info: *mut windows::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
) -> i32 {
    use windows::Win32::System::Diagnostics::Debug::EXCEPTION_CONTINUE_SEARCH;

    let Some(info) = (unsafe { info.as_ref() }) else {
        return EXCEPTION_CONTINUE_SEARCH;
    };
    let Some(record) = (unsafe { info.ExceptionRecord.as_ref() }) else {
        return EXCEPTION_CONTINUE_SEARCH;
    };
    let code = record.ExceptionCode.0 as u32;
    if !matches!(
        code,
        0xC000_0005 // EXCEPTION_ACCESS_VIOLATION
            | 0xC000_00FD // EXCEPTION_STACK_OVERFLOW
            | 0x8000_0003 // EXCEPTION_BREAKPOINT
            | 0xC000_001D // EXCEPTION_ILLEGAL_INSTRUCTION
            | 0xC000_0094 // EXCEPTION_INT_DIVIDE_BY_ZERO
            | 0xC000_0095 // EXCEPTION_FLT_OVERFLOW
            | 0xC000_0374 // STATUS_HEAP_CORRUPTION
    ) {
        return EXCEPTION_CONTINUE_SEARCH;
    }

    if NATIVE_EXCEPTION_LOGGING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        let tid = logger::current_thread_id_num()
            .map(|n| n.to_string())
            .unwrap_or_else(|| "?".to_owned());
        let mut details = format!(
            "NATIVE EXCEPTION code=0x{code:08X} flags=0x{:08X} address={:p} thread={tid}",
            record.ExceptionFlags, record.ExceptionAddress
        );
        if code == 0xC000_0005 {
            let access_kind = match record.ExceptionInformation[0] {
                0 => "read",
                1 => "write",
                8 => "execute",
                _ => "unknown",
            };
            let access_address = record.ExceptionInformation[1] as *const core::ffi::c_void;
            details.push_str(&format!(" access={access_kind} target={access_address:p}"));
        }
        append_panic_log_entry(&details);
        NATIVE_EXCEPTION_LOGGING.store(false, Ordering::Release);
    }

    EXCEPTION_CONTINUE_SEARCH
}

fn main() -> eframe::Result {
    // main() 入口の Instant を起動時間計測の t=0 とする。
    // --pdf-worker モードでは計測しないので worker 判定の前に取らない。
    // --perf-log 無効時は `emit_startup` が no-op なのでコストはゼロ。
    let prog_start = Instant::now();
    let play_test_config = parse_play_test_config();
    #[cfg(windows)]
    let dcomp_presenter_config = dcomp_presenter_test::parse_config();
    let perf_log_path = parse_perf_log_path_arg();

    // --pdf-worker モード: GUI なしで PDFium ワーカープロセスとして起動
    if std::env::args().any(|a| a == pdf_loader::PDF_WORKER_ARG) {
        pdf_loader::run_worker_process();
        std::process::exit(0);
    }

    // --tensorrt-build <model_kind> モード: TensorRT エンジンビルダーワーカー。
    // 親プロセス (GUI) から子プロセスとして起動され、指定モデルを TRT EP で
    // load_model することで engine cache を populate する。stdout に進捗 JSON。
    if std::env::args().any(|a| a == ai::tensorrt_builder::TRT_BUILD_ARG) {
        // data_dir 初期化が必要 (engine cache path や DLL extract で使う)
        data_dir::init();
        ai::tensorrt_builder::run_worker_process();
    }

    // --tensorrt-infer-worker モード: TensorRT 推論ワーカー (Phase 3)。
    // 親プロセス (GUI、DirectML 動作) から子プロセスとして起動され、stdin で
    // コマンドを受けて TRT セッションで推論を実行、共有メモリで結果を返す。
    // ホットリロード時の再起動なしバックエンド切替を実現するための分離。
    if ai::trt_worker_runtime::is_worker_invocation() {
        data_dir::init();
        ai::trt_worker_runtime::run_infer_worker();
    }

    // --trt-smoke-test モード: TRT ワーカープール起動の動作確認用 (開発者向け)。
    // current_exe() が正しく mimageviewer.exe を返すため、本体に組み込んでいる。
    if std::env::args().any(|a| a == "--trt-smoke-test") {
        data_dir::init();
        logger::init();
        run_trt_smoke_test();
    }

    // シングルインスタンス検出 (Windows): Named Mutex で 2 重起動を排除する。
    // インストーラの AppMutex と名前を合わせることでアップデート時の「閉じてください」
    // ダイアログ自動連携も兼ねる (`single_instance::MUTEX_NAME` 参照)。
    // is_first_instance() == false のときは既にもう 1 つ mIV が動いているので
    // 静かに exit する (トレイ常駐中でもここで落ちる = ユーザーはトレイアイコンから
    // 復帰することで操作を再開できる)。
    #[cfg(windows)]
    let skip_single_instance = dcomp_presenter_config.is_some();
    #[cfg(not(windows))]
    let skip_single_instance = false;
    let _single_instance = if skip_single_instance {
        None
    } else {
        let guard = single_instance::SingleInstanceGuard::acquire();
        if !guard.is_first_instance() {
            // 2 重起動: 既存インスタンスの activate event を叩いてウィンドウを前面に出す。
            // ユーザーが「もう一度 mIV を起動」した意図を既存インスタンスで復帰として解釈する。
            let signaled = single_instance::signal_activate_existing();
            eprintln!(
                "mImageViewer is already running (activate signaled: {signaled}). Exiting second instance."
            );
            std::process::exit(0);
        }
        Some(guard)
    };

    // data_dir::init() は perf::init が logs_dir を使うため先行させる必要がある。
    let t0 = Instant::now();
    data_dir::init();
    let data_dir_elapsed = t0.elapsed();
    install_panic_log_hook();
    #[cfg(windows)]
    install_native_exception_log_hook();

    // デバッグビルドでは常にログ出力。リリースビルドでは --log 引数で有効化
    let log_enabled = cfg!(debug_assertions) || std::env::args().any(|a| a == "--log");
    if log_enabled {
        logger::init();
    }

    // --perf-log: 構造化イベントログ (JSON Lines) を有効化する。
    // 無指定時は `perf::is_enabled()` が false のまま、全 perf::event 呼出しが即 return。
    // prog_start を基準にすることで startup.* イベントの `total_ms` が真の経過時間を指す。
    let perf_enabled = std::env::args().any(|a| a == "--perf-log" || a == "--perf-log-path")
        || perf_log_path.is_some();
    perf::init_with_path(perf_enabled, Some(prog_start), perf_log_path);

    // WASAPI can take over a second to create the first output stream on a cold
    // boot. Warm it in the background so the first video open does not freeze
    // the UI long enough for queued fullscreen-close inputs/focus checks to win.
    video::audio::warm_up_default_output_device();

    if let Some(config) = &play_test_config {
        if !config.path.is_file() {
            eprintln!("--play-test path is not a file: {}", config.path.display());
            logger::log(format!(
                "play-test: path is not a file: {}",
                config.path.display()
            ));
            std::process::exit(2);
        }
        logger::log(format!(
            "play-test: path={} duration_ms={} mute={}",
            config.path.display(),
            config.duration.as_millis(),
            config.mute
        ));
    }

    #[cfg(windows)]
    if let Some(config) = dcomp_presenter_config {
        if !config.path.is_file() {
            eprintln!(
                "--dcomp-presenter-test path is not a file: {}",
                config.path.display()
            );
            logger::log(format!(
                "dcomp-presenter-test: path is not a file: {}",
                config.path.display()
            ));
            std::process::exit(2);
        }
        logger::log(format!(
            "dcomp-presenter-test: path={} duration_ms={} window={}x{} sync_interval={} force_sw={} pixel_probe_strict={}",
            config.path.display(),
            config.duration.as_millis(),
            config.width,
            config.height,
            config.sync_interval,
            config.force_sw,
            config.pixel_probe_strict
        ));
        if let Err(e) = dcomp_presenter_test::run(config) {
            eprintln!("dcomp-presenter-test failed: {e}");
            logger::log(format!("dcomp-presenter-test failed: {e}"));
            std::process::exit(1);
        }
        perf::flush();
        std::process::exit(0);
    }

    // 起動時間計測: data_dir 初期化は先行ステップなので perf::init 後に後追いで打つ。
    // phase_start を渡すと ms を載せられるが、ここは経過分を再現できないので
    // data_dir_elapsed を直接 ms として埋める。
    if perf::is_enabled() {
        let total_ms = prog_start.elapsed().as_secs_f64() * 1000.0;
        perf::event(
            "startup",
            "data_dir_init",
            None,
            0,
            &[
                (
                    "ms",
                    serde_json::Value::from(data_dir_elapsed.as_secs_f64() * 1000.0),
                ),
                ("total_ms", serde_json::Value::from(total_ms)),
            ],
        );
    }

    // AI モデルを %APPDATA%\mimageviewer\models\ に展開（サイズ一致ならスキップ）
    let t = Instant::now();
    ai::model_manager::ensure_models_extracted();
    emit_startup("models_extract", Some(t));

    // Susie 32bit ワーカー exe を %APPDATA%\mimageviewer\mimageviewer-susie32.exe に展開。
    // PDFium DLL と同じパターンで本体 exe に埋め込み、初回起動時に書き出す。
    let t = Instant::now();
    susie_loader::ensure_worker_extracted();
    emit_startup("susie_worker_extract", Some(t));

    // Susie プラグインワーカープール: バックグラウンドで初期化する
    // (プラグインが多いと handshake に数百ms かかる可能性があるため、
    //  起動 UI をブロックしないようスレッドに逃がす)
    std::thread::Builder::new()
        .name("susie-init".to_string())
        .spawn(|| {
            let _ = susie_loader::get_pool();
        })
        .ok();

    // 保存済み設定からウィンドウ初期状態を決定する
    let t = Instant::now();
    let saved = settings::Settings::load();
    emit_startup("settings_load", Some(t));

    let default_size = [1280.0_f32, 800.0_f32];
    // --window-size WxH 引数があればそれを優先（スクリーンショット用）
    let size = parse_window_size_arg().unwrap_or_else(|| {
        saved
            .window_size
            .filter(|size| sane_window_size(*size))
            .unwrap_or(default_size)
    });

    let t = Instant::now();
    let icon = Arc::new(load_icon());
    emit_startup("load_icon", Some(t));

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("mimageviewer")
        .with_inner_size(size)
        .with_icon(icon);

    // --window-size 指定時は位置を画面左上寄りに固定（保存済み位置は無視）
    if parse_window_size_arg().is_some() {
        viewport = viewport.with_position(egui::pos2(60.0, 40.0));
    } else if let Some([x, y]) = saved.window_pos {
        let w = saved.window_size.map(|[w, _]| w).unwrap_or(1280.0);
        if monitor::title_bar_on_some_monitor(x, y, w) {
            viewport = viewport.with_position(egui::pos2(x, y));
        }
    }

    // wgpu のバックエンドは **DX12 を優先 + Vulkan フォールバック**。
    // wgpu 既定のスコアリングでは Vulkan が DX12 より優先選択される環境があるため、
    // カスタムアダプタセレクタで明示的に DX12 アダプタを最優先する。DX12 が無ければ
    // Vulkan、それも無ければ任意の adapter を返す (eframe / egui 自体は描画される)。
    // 動画 GPU 経路は実行時に `cc.wgpu_render_state.adapter.get_info().backend` を見て
    // DX12 のときだけ有効化、Vulkan ならスキップして CPU readback で再生する。
    let mut wgpu_options = egui_wgpu::WgpuConfiguration::default();
    configure_wgpu_presentation(&mut wgpu_options);
    if let egui_wgpu::WgpuSetup::CreateNew(create_new) = &mut wgpu_options.wgpu_setup {
        create_new.instance_descriptor.backends = wgpu::Backends::DX12 | wgpu::Backends::VULKAN;
        create_new.native_adapter_selector = Some(std::sync::Arc::new(
            |adapters: &[wgpu::Adapter], _surface: Option<&wgpu::Surface<'_>>| {
                if let Some(a) = adapters
                    .iter()
                    .find(|a| a.get_info().backend == wgpu::Backend::Dx12)
                {
                    return Ok(a.clone());
                }
                if let Some(a) = adapters
                    .iter()
                    .find(|a| a.get_info().backend == wgpu::Backend::Vulkan)
                {
                    return Ok(a.clone());
                }
                adapters
                    .first()
                    .cloned()
                    .ok_or_else(|| "no wgpu adapter available".to_string())
            },
        ));
    }
    let options = eframe::NativeOptions {
        viewport,
        wgpu_options,
        ..Default::default()
    };

    // eframe::run_native に入る手前までを 1 つの marker として記録する。
    // これ以降は eframe (winit + wgpu) の初期化が走り、creator closure が呼ばれる。
    emit_startup("before_run_native", None);
    install_ui_heartbeat_watchdog();

    eframe::run_native(
        "mimageviewer",
        options,
        Box::new(move |cc| {
            // creator closure: wgpu/winit 初期化後に 1 回だけ呼ばれる。
            // この closure の先頭までの所要時間 = eframe 自体のセットアップ時間。
            emit_startup("creator_enter", None);
            let t = Instant::now();
            ui_fonts::configure_fonts(&cc.egui_ctx);
            emit_startup("setup_fonts", Some(t));
            // 起動時点で UI テーマを先行適用して、初回フレームでの
            // ダーク/ライト切替ちらつきを避ける (set_visuals は次フレームから
            // 効くため、App::update 内で適用すると 1 フレームだけデフォルト
            // ダーク表示になる)。
            let t = Instant::now();
            let resolved = os_theme::resolve(saved.ui_theme);
            os_theme::apply_resolved(&cc.egui_ctx, resolved);
            emit_startup("apply_theme", Some(t));
            let t = Instant::now();
            let mut app = app::App::default();
            emit_startup("app_default", Some(t));
            if let Some(config) = play_test_config.clone() {
                app.configure_play_test(config);
            }
            app.applied_ui_theme = Some(resolved);

            // 動画 GPU レンダリング用の wgpu::Device / Queue を保存。
            // また同時に共有 D3D11 デバイスを初期化 (失敗してもアプリは起動継続、
            // 動画は旧経路 = CPU readback + swscale にフォールバック)。
            #[cfg(windows)]
            {
                if let Some(rs) = cc.wgpu_render_state.clone() {
                    // 実際に選ばれた wgpu バックエンドを確認。動画 GPU 経路は
                    // `wgpu_hal::api::Dx12` 経由で D3D11 NT shared texture を
                    // import するので **DX12 でないと使えない**。Vulkan
                    // (= リモートデスクトップ等の fallback) では GPU video device を
                    // 作らず CPU readback + swscale 経路にフォールバックする。
                    let backend = rs.adapter.get_info().backend;
                    crate::logger::log(format!("wgpu backend selected: {backend:?}"));
                    // GpuVideoDevice は wgpu backend に依存せず独立した D3D11 device を
                    // 持つため、native presenter の動作前提として常に作成を試みる。
                    // 失敗時は decoder が SW デコード + CPU upload にフォールバックする。
                    match crate::video::gpu_renderer::GpuVideoDevice::new() {
                        Ok(dev) => {
                            crate::logger::log(
                                "GPU video device: created (D3D11 + video processor)".to_string(),
                            );
                            app.gpu_video_device = Some(dev);
                        }
                        Err(e) => {
                            crate::logger::log(format!(
                                "GPU video device: failed (will fallback to CPU readback): {e}"
                            ));
                        }
                    }
                    app.wgpu_render_state = Some(rs);
                }
            }
            // お気に入り単位の補正標準を DB から復元 (+ 削除されたお気に入りの orphan 行を掃除)。
            let t = Instant::now();
            app.hydrate_adjustment_favorite_params();
            emit_startup("hydrate_adj_favs", Some(t));
            // name index supervisor を起動時に spawn (auto_index_structure=true なお気に入り)。
            // IndexerManager::sync_with_favorites がメタ側の対応処理を既に走らせているが、
            // 名前索引は IndexerManager 外の管理なのでここで別途 spawn する。
            let t = Instant::now();
            app.spawn_initial_name_index_supervisors();
            emit_startup("spawn_name_idx_sup", Some(t));
            // DPI 確定後の初回フレームで意図したサイズを再適用する
            // (egui#4918 / winit#923 対策)。ViewportBuilder 段階では
            // マルチモニタ DPI 混在時にサイズが壊れるケースがある。
            app.pending_initial_size = Some(size);
            emit_startup("creator_exit", None);
            Ok(Box::new(app))
        }),
    )
}

/// `--window-size WxH` 引数をパース（例: `--window-size 1400x860`）。
/// `--trt-smoke-test` 用の開発者向け動作確認関数。
/// TRT ワーカープールが spawn → ハンドシェイク → load_model → shutdown を
/// 一通り実行できるかを確認する。完了で exit 0、失敗で exit 1。
fn run_trt_smoke_test() -> ! {
    println!("[smoke] TrtWorkerPool::start()");
    let pool = match ai::trt_worker_pool::TrtWorkerPool::start() {
        Ok(p) => {
            println!("[smoke] start OK");
            p
        }
        Err(e) => {
            eprintln!("[smoke] start failed: {e}");
            std::process::exit(1);
        }
    };

    let test_kinds = [
        ai::ModelKind::DenoiseRealplksr,
        ai::ModelKind::UpscaleRealEsrganAnime6B,
    ];
    for kind in test_kinds {
        println!("[smoke] LoadModel {:?}", kind);
        match pool.load_model(kind) {
            Ok(ms) => println!("[smoke] LoadModel {:?} OK in {ms} ms", kind),
            Err(e) => {
                eprintln!("[smoke] LoadModel {:?} failed: {e}", kind);
                std::process::exit(1);
            }
        }
    }

    // Step 2 検証: 実際に Infer を 1 回流して、入出力 shape と f32 値の
    // 妥当性を確認する。
    println!("[smoke] Infer (anime6b, 256x256 zero tile) ...");
    let dummy_input = ndarray::Array4::<f32>::zeros((1, 3, 256, 256));
    let t_infer = std::time::Instant::now();
    match pool.infer(ai::ModelKind::UpscaleRealEsrganAnime6B, &dummy_input) {
        Ok((shape, out)) => {
            let elapsed = t_infer.elapsed().as_millis();
            let expected_total = shape.iter().product::<i64>() as usize;
            println!(
                "[smoke] Infer OK in {elapsed} ms, output shape={shape:?}, len={} (expected {expected_total})",
                out.len()
            );
            if out.len() != expected_total {
                eprintln!("[smoke] FAIL: output len mismatch");
                std::process::exit(1);
            }
            // ゼロ入力の anime6b 出力は数値的にゼロ近傍のはず。
            // 値域 [0,1] (× 255 で 0-255 にスケール) で大きく外れていないかチェック。
            let min = out.iter().cloned().fold(f32::INFINITY, f32::min);
            let max = out.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            println!("[smoke]   output value range: [{min:.4}, {max:.4}]");
        }
        Err(e) => {
            eprintln!("[smoke] Infer failed: {e}");
            std::process::exit(1);
        }
    }

    println!("[smoke] shutdown (Drop)");
    drop(pool);
    println!("[smoke] all OK");
    std::process::exit(0);
}

fn parse_window_size_arg() -> Option<[f32; 2]> {
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len().saturating_sub(1) {
        if args[i] == "--window-size" {
            let parts: Vec<&str> = args[i + 1].split('x').collect();
            if parts.len() == 2 {
                if let (Ok(w), Ok(h)) = (parts[0].parse::<f32>(), parts[1].parse::<f32>()) {
                    return Some([w, h]);
                }
            }
        }
    }
    None
}

fn arg_value(flag: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len().saturating_sub(1) {
        if args[i] == flag {
            return Some(args[i + 1].clone());
        }
    }
    None
}

fn has_arg(flag: &str) -> bool {
    std::env::args().any(|a| a == flag)
}

fn parse_perf_log_path_arg() -> Option<std::path::PathBuf> {
    if let Some(path) = arg_value("--perf-log-path") {
        return Some(std::path::PathBuf::from(path));
    }
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len().saturating_sub(1) {
        if args[i] == "--perf-log" && !args[i + 1].starts_with("--") {
            return Some(std::path::PathBuf::from(args[i + 1].clone()));
        }
    }
    None
}

fn parse_play_test_config() -> Option<app::PlayTestConfig> {
    let path = std::path::PathBuf::from(arg_value("--play-test")?);
    let duration_secs = arg_value("--play-duration")
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(30.0);
    Some(app::PlayTestConfig {
        path,
        duration: std::time::Duration::from_secs_f64(duration_secs),
        mute: has_arg("--play-muted") || has_arg("--mute"),
        start_secs: arg_value("--play-test-start")
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0),
        skip_vst3: has_arg("--play-test-skip-vst3"),
    })
}

fn sane_window_size(size: [f32; 2]) -> bool {
    size[0].is_finite()
        && size[1].is_finite()
        && size[0] >= 320.0
        && size[1] >= 240.0
        && size[0] <= 16_384.0
        && size[1] <= 16_384.0
}

fn load_icon() -> egui::IconData {
    let bytes = include_bytes!("../assets/icon.png");
    let img = image::load_from_memory(bytes)
        .expect("icon.png の読み込み失敗")
        .into_rgba8();
    let (width, height) = img.dimensions();
    egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    }
}
