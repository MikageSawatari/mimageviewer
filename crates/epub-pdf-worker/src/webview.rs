//! Narrow raw COM binding for the WebView2 APIs used by this spike.
//!
//! `webview2-com` is unavailable in the offline Cargo registry used by this
//! worktree. The WebView2 SDK COM IIDs and vtable slots below are fixed ABI
//! contracts. Loader is resolved dynamically; see README for distribution.
#![allow(unsafe_op_in_unsafe_fn)]
use base64::Engine;
use std::{
    cell::UnsafeCell,
    ffi::c_void,
    mem,
    path::Path,
    ptr,
    sync::atomic::{AtomicU32, Ordering},
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{FreeLibrary, HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
            LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW},
        },
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE,
            PeekMessageW, RegisterClassW, TranslateMessage, UnregisterClassW, WINDOW_EX_STYLE,
            WNDCLASSW, WS_OVERLAPPEDWINDOW,
        },
    },
    core::{GUID, PCSTR, PCWSTR},
};

type Raw = *mut c_void;
const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
const IID_ENV_CB: GUID = GUID::from_u128(0x4e8a3389_c9d8_4bd2_b6b5_124fee6cc14d);
const IID_CTRL_CB: GUID = GUID::from_u128(0x6c4819f3_c9b7_4260_8127_c9f5bde7f68c);
const IID_SCRIPT_CB: GUID = GUID::from_u128(0x49511172_cc67_4bca_9923_137112f4c4cc);
const IID_CDP_CB: GUID = GUID::from_u128(0x5c4889f0_5ef6_4c5a_952c_d8f1b92d0574);
const IID_PRINT_CB: GUID = GUID::from_u128(0xccf1ef04_fd8e_4d5f_b2de_0983e41b8c36);
const IID_WEBVIEW3: GUID = GUID::from_u128(0xa0d6df20_3b92_416d_aa0c_437a9c727857);
const IID_WEBVIEW7: GUID = GUID::from_u128(0x79c24d83_09a3_45ae_9418_487f32a58740);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe fn read_wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}
fn hr(code: i32, context: &str) -> Result<(), String> {
    if code < 0 {
        Err(format!("{context}: HRESULT 0x{:08X}", code as u32))
    } else {
        Ok(())
    }
}
unsafe fn slot(p: Raw, n: usize) -> Raw {
    *(*(p as *const *const Raw)).add(n)
}
unsafe fn add_ref(p: Raw) {
    let f: unsafe extern "system" fn(Raw) -> u32 = mem::transmute(slot(p, 1));
    f(p);
}
unsafe fn release(p: Raw) {
    let f: unsafe extern "system" fn(Raw) -> u32 = mem::transmute(slot(p, 2));
    f(p);
}
struct Com(Raw);
impl Drop for Com {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { release(self.0) }
        }
    }
}
impl Com {
    fn query(&self, iid: &GUID) -> Result<Self, String> {
        unsafe {
            let f: unsafe extern "system" fn(Raw, *const GUID, *mut Raw) -> i32 =
                mem::transmute(slot(self.0, 0));
            let mut out = ptr::null_mut();
            hr(f(self.0, iid, &mut out), "QueryInterface")?;
            Ok(Self(out))
        }
    }
}

#[derive(Clone, Copy)]
enum CallbackKind {
    Interface,
    Text,
    Bool,
}
#[derive(Default)]
struct Completion {
    done: bool,
    code: i32,
    object: Raw,
    text: Option<String>,
    boolean: bool,
}
#[repr(C)]
struct CallbackVTable {
    query: unsafe extern "system" fn(*mut Callback, *const GUID, *mut Raw) -> i32,
    addref: unsafe extern "system" fn(*mut Callback) -> u32,
    release: unsafe extern "system" fn(*mut Callback) -> u32,
    invoke: unsafe extern "system" fn(*mut Callback, i32, Raw) -> i32,
}
#[repr(C)]
struct Callback {
    vtbl: *const CallbackVTable,
    refs: AtomicU32,
    iid: GUID,
    kind: CallbackKind,
    value: UnsafeCell<Completion>,
}
unsafe extern "system" fn cb_query(this: *mut Callback, iid: *const GUID, out: *mut Raw) -> i32 {
    if iid.is_null() || out.is_null() {
        return 0x80004003u32 as i32;
    }
    *out = ptr::null_mut();
    if *iid == IID_IUNKNOWN || *iid == (*this).iid {
        *out = this.cast();
        cb_addref(this);
        0
    } else {
        0x80004002u32 as i32
    }
}
unsafe extern "system" fn cb_addref(this: *mut Callback) -> u32 {
    (*this).refs.fetch_add(1, Ordering::Relaxed) + 1
}
unsafe extern "system" fn cb_release(this: *mut Callback) -> u32 {
    let n = (*this).refs.fetch_sub(1, Ordering::Release) - 1;
    if n == 0 {
        std::sync::atomic::fence(Ordering::Acquire);
        drop(Box::from_raw(this));
    }
    n
}
unsafe extern "system" fn cb_invoke(this: *mut Callback, code: i32, result: Raw) -> i32 {
    let cb = &*this;
    let out = &mut *cb.value.get();
    out.code = code;
    match cb.kind {
        CallbackKind::Interface => {
            out.object = result;
            if !result.is_null() {
                add_ref(result)
            }
        }
        CallbackKind::Text => out.text = Some(read_wide(result.cast())),
        CallbackKind::Bool => out.boolean = result as isize != 0,
    }
    out.done = true;
    0
}
static CALLBACK_VTBL: CallbackVTable = CallbackVTable {
    query: cb_query,
    addref: cb_addref,
    release: cb_release,
    invoke: cb_invoke,
};
struct Pending(*mut Callback);
impl Pending {
    fn new(iid: GUID, kind: CallbackKind) -> Self {
        Self(Box::into_raw(Box::new(Callback {
            vtbl: &CALLBACK_VTBL,
            refs: AtomicU32::new(1),
            iid,
            kind,
            value: UnsafeCell::new(Completion::default()),
        })))
    }
    fn raw(&self) -> Raw {
        self.0.cast()
    }
    fn wait(&self, deadline: Instant) -> Result<Completion, String> {
        loop {
            unsafe {
                let value = &mut *(*self.0).value.get();
                if value.done {
                    let value = mem::take(value);
                    hr(value.code, "WebView2 callback")?;
                    return Ok(value);
                }
            }
            if Instant::now() >= deadline {
                return Err("WebView2 timeout".into());
            }
            pump();
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        unsafe {
            cb_release(self.0);
        }
    }
}
fn pump() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
unsafe extern "system" fn host_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

type VersionFn = unsafe extern "system" fn(*const u16, *mut *mut u16) -> i32;
type CreateFn = unsafe extern "system" fn(*const u16, *const u16, Raw, Raw) -> i32;
pub struct Host {
    module: HMODULE,
    hwnd: HWND,
    instance: HINSTANCE,
    class_name: Vec<u16>,
    class_registered: bool,
    com_initialized: bool,
    environment: Option<Com>,
    controller: Option<Com>,
    webview: Option<Com>,
    webview3: Option<Com>,
    webview7: Option<Com>,
    pub version: String,
}
impl Host {
    pub fn new(user_data: &Path, deadline: Instant) -> Result<Self, String> {
        let loader = std::env::var_os("MIV_WEBVIEW2_LOADER")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_exe()
                    .unwrap_or_default()
                    .with_file_name("WebView2Loader.dll")
            });
        let dll = unsafe { LoadLibraryW(PCWSTR(wide(&loader.to_string_lossy()).as_ptr())) }
            .map_err(|e| format!("WebView2Loader.dll missing ({}): {e}", loader.display()))?;
        let mut h = Self {
            module: dll,
            hwnd: HWND::default(),
            instance: HINSTANCE::default(),
            class_name: wide("mIV-epub-pdf-worker-hidden-host"),
            class_registered: false,
            com_initialized: false,
            environment: None,
            controller: None,
            webview: None,
            webview3: None,
            webview7: None,
            version: String::new(),
        };
        let version_fn: VersionFn = unsafe {
            mem::transmute(
                GetProcAddress(
                    dll,
                    PCSTR(
                        c"GetAvailableCoreWebView2BrowserVersionString"
                            .as_ptr()
                            .cast(),
                    ),
                )
                .ok_or("loader lacks runtime probe")?,
            )
        };
        let mut version_ptr = ptr::null_mut();
        let code = unsafe { version_fn(ptr::null(), &mut version_ptr) };
        if code < 0 || version_ptr.is_null() {
            return Err(format!(
                "WebView2 runtime missing: HRESULT 0x{:08X}",
                code as u32
            ));
        }
        h.version = unsafe { read_wide(version_ptr) };
        unsafe {
            windows::Win32::System::Com::CoTaskMemFree(Some(version_ptr.cast()));
        }
        hr(
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.0,
            "CoInitializeEx",
        )?;
        h.com_initialized = true;
        let module =
            unsafe { GetModuleHandleW(None) }.map_err(|e| format!("GetModuleHandleW: {e}"))?;
        h.instance = HINSTANCE(module.0);
        let class = WNDCLASSW {
            lpfnWndProc: Some(host_wnd_proc),
            hInstance: h.instance,
            lpszClassName: PCWSTR(h.class_name.as_ptr()),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err(format!(
                "RegisterClassW: {}",
                windows::core::Error::from_win32()
            ));
        }
        h.class_registered = true;
        h.hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(h.class_name.as_ptr()),
                PCWSTR(wide("mIV EPUB PDF worker").as_ptr()),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                1200,
                1700,
                None,
                None,
                Some(h.instance),
                None,
            )
        }
        .map_err(|e| format!("hidden host window: {e}"))?;
        let create_fn: CreateFn = unsafe {
            mem::transmute(
                GetProcAddress(
                    dll,
                    PCSTR(c"CreateCoreWebView2EnvironmentWithOptions".as_ptr().cast()),
                )
                .ok_or("loader lacks environment factory")?,
            )
        };
        let callback = Pending::new(IID_ENV_CB, CallbackKind::Interface);
        let ud = wide(&user_data.to_string_lossy());
        hr(
            unsafe { create_fn(ptr::null(), ud.as_ptr(), ptr::null_mut(), callback.raw()) },
            "CreateCoreWebView2EnvironmentWithOptions",
        )?;
        h.environment = Some(Com(callback
            .wait(deadline)
            .map_err(|e| {
                format!(
                    "WebView2 runtime {}; environment creation failed: {e}",
                    h.version
                )
            })?
            .object));
        let env = h.environment.as_ref().unwrap();
        let callback = Pending::new(IID_CTRL_CB, CallbackKind::Interface);
        unsafe {
            let f: unsafe extern "system" fn(Raw, HWND, Raw) -> i32 =
                mem::transmute(slot(env.0, 3));
            hr(
                f(env.0, h.hwnd, callback.raw()),
                "CreateCoreWebView2Controller",
            )?;
        }
        h.controller = Some(Com(callback
            .wait(deadline)
            .map_err(|e| {
                format!(
                    "WebView2 runtime {}; controller creation failed: {e}",
                    h.version
                )
            })?
            .object));
        let controller = h.controller.as_ref().unwrap();
        unsafe {
            let set_bounds: unsafe extern "system" fn(Raw, RECT) -> i32 =
                mem::transmute(slot(controller.0, 6));
            hr(
                set_bounds(
                    controller.0,
                    RECT {
                        left: 0,
                        top: 0,
                        right: 1200,
                        bottom: 1700,
                    },
                ),
                "put_Bounds",
            )?;
        }
        let mut view = ptr::null_mut();
        unsafe {
            let f: unsafe extern "system" fn(Raw, *mut Raw) -> i32 =
                mem::transmute(slot(controller.0, 25));
            hr(f(controller.0, &mut view), "get_CoreWebView2")?;
        }
        h.webview = Some(Com(view));
        h.webview3 = Some(h.webview.as_ref().unwrap().query(&IID_WEBVIEW3)?);
        h.webview7 = h.webview.as_ref().unwrap().query(&IID_WEBVIEW7).ok();
        Ok(h)
    }
    fn view(&self) -> Raw {
        self.webview.as_ref().unwrap().0
    }
    pub fn map_folder(&self, dir: &Path) -> Result<(), String> {
        let host = wide("epub.invalid");
        let folder = wide(&dir.to_string_lossy());
        let v = self.webview3.as_ref().unwrap().0;
        unsafe {
            let f: unsafe extern "system" fn(Raw, *const u16, *const u16, i32) -> i32 =
                mem::transmute(slot(v, 71));
            hr(
                f(v, host.as_ptr(), folder.as_ptr(), 0),
                "SetVirtualHostNameToFolderMapping",
            )
        }
    }
    pub fn navigate(&self, url: &str) -> Result<(), String> {
        let v = self.view();
        let url = wide(url);
        unsafe {
            let f: unsafe extern "system" fn(Raw, *const u16) -> i32 = mem::transmute(slot(v, 5));
            hr(f(v, url.as_ptr()), "Navigate")
        }
    }
    pub fn script(&self, script: &str, deadline: Instant) -> Result<String, String> {
        let v = self.view();
        let script = wide(script);
        let callback = Pending::new(IID_SCRIPT_CB, CallbackKind::Text);
        unsafe {
            let f: unsafe extern "system" fn(Raw, *const u16, Raw) -> i32 =
                mem::transmute(slot(v, 29));
            hr(f(v, script.as_ptr(), callback.raw()), "ExecuteScript")?;
        }
        Ok(callback.wait(deadline)?.text.unwrap_or_default())
    }
    pub fn wait_ready(&self, url: &str, deadline: Instant) -> Result<(), String> {
        let desired = serde_json::to_string(url).map_err(|e| e.to_string())?;
        let test = format!(
            r#"(()=>location.href===new URL({desired}).href&&document.readyState==='complete'&&document.fonts.status==='loaded'&&[...document.images].every(i=>i.complete&&i.naturalWidth>0)&&[...document.querySelectorAll('iframe')].every(f=>f.contentDocument&&f.contentDocument.readyState==='complete'&&f.contentDocument.fonts.status==='loaded'&&[...f.contentDocument.images].every(i=>i.complete&&i.naturalWidth>0)))()"#
        );
        loop {
            if Instant::now() >= deadline {
                return Err("WebView2 timeout waiting for images/iframes/fonts".into());
            }
            if self.script(&test, deadline)?.trim() == "true" {
                return Ok(());
            }
            pump();
            thread::sleep(Duration::from_millis(100));
        }
    }
    pub fn devtools_pdf(&self, deadline: Instant) -> Result<Vec<u8>, String> {
        let v = self.view();
        let callback = Pending::new(IID_CDP_CB, CallbackKind::Text);
        let method = wide("Page.printToPDF");
        let args = wide(
            r#"{"preferCSSPageSize":true,"printBackground":true,"marginTop":0,"marginBottom":0,"marginLeft":0,"marginRight":0,"displayHeaderFooter":false}"#,
        );
        unsafe {
            let f: unsafe extern "system" fn(Raw, *const u16, *const u16, Raw) -> i32 =
                mem::transmute(slot(v, 36));
            hr(
                f(v, method.as_ptr(), args.as_ptr(), callback.raw()),
                "CallDevToolsProtocolMethod",
            )?;
        }
        let response = callback.wait(deadline)?.text.unwrap_or_default();
        let parsed: serde_json::Value =
            serde_json::from_str(&response).map_err(|e| format!("invalid CDP response: {e}"))?;
        if let Some(error) = parsed.get("error") {
            return Err(format!("Page.printToPDF: {error}"));
        }
        let data = parsed.get("data").and_then(|x| x.as_str()).ok_or_else(|| {
            format!(
                "Page.printToPDF omitted data: {}",
                response.chars().take(300).collect::<String>()
            )
        })?;
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| e.to_string())
    }
    pub fn print_to_pdf(&self, path: &Path, deadline: Instant) -> Result<bool, String> {
        let Some(v) = &self.webview7 else {
            return Err("ICoreWebView2_7 unavailable".into());
        };
        let callback = Pending::new(IID_PRINT_CB, CallbackKind::Bool);
        let path = wide(&path.to_string_lossy());
        unsafe {
            let f: unsafe extern "system" fn(Raw, *const u16, Raw, Raw) -> i32 =
                mem::transmute(slot(v.0, 80));
            hr(
                f(v.0, path.as_ptr(), ptr::null_mut(), callback.raw()),
                "ICoreWebView2_7::PrintToPdf",
            )?;
        }
        Ok(callback.wait(deadline)?.boolean)
    }
    pub fn close(&mut self) {
        if let Some(c) = &self.controller {
            unsafe {
                let f: unsafe extern "system" fn(Raw) -> i32 = mem::transmute(slot(c.0, 24));
                let _ = f(c.0);
            }
        }
        self.webview7 = None;
        self.webview3 = None;
        self.webview = None;
        self.controller = None;
        self.environment = None;
        pump();
        if !self.hwnd.is_invalid() {
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
            self.hwnd = HWND::default();
        }
        if self.class_registered {
            unsafe {
                let _ = UnregisterClassW(PCWSTR(self.class_name.as_ptr()), Some(self.instance));
            }
            self.class_registered = false;
        }
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.close();
        unsafe {
            if self.com_initialized {
                CoUninitialize();
            }
            if !self.module.is_invalid() {
                let _ = FreeLibrary(self.module);
            }
        }
    }
}
