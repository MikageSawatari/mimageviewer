//! WebView2 host for EPUB printing. All COM objects belong to this STA thread.
use base64::Engine;
use std::{
    path::Path,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, ExecuteScriptCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY, CreateCoreWebView2EnvironmentWithOptions,
        GetAvailableCoreWebView2BrowserVersionString, ICoreWebView2, ICoreWebView2_3,
        ICoreWebView2Controller, ICoreWebView2Environment,
    },
    take_pwstr,
};
use windows::{
    Win32::{
        Foundation::{E_POINTER, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
            LibraryLoader::GetModuleHandleW,
        },
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE,
            PeekMessageW, RegisterClassW, TranslateMessage, UnregisterClassW, WINDOW_EX_STYLE,
            WNDCLASSW, WS_OVERLAPPEDWINDOW,
        },
    },
    core::{Interface, PCWSTR, PWSTR},
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
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

// wait_with_pump blocks in GetMessage. This bounded equivalent preserves the
// deadline required by the conversion command.
fn wait<T>(rx: Receiver<windows::core::Result<T>>, deadline: Instant) -> Result<T, String> {
    loop {
        match rx.try_recv() {
            Ok(result) => return result.map_err(|e| format!("WebView2 callback: {e}")),
            Err(TryRecvError::Disconnected) => return Err("WebView2 callback disconnected".into()),
            Err(TryRecvError::Empty) => {}
        }
        if Instant::now() >= deadline {
            return Err("WebView2 timeout".into());
        }
        pump();
        thread::sleep(Duration::from_millis(10));
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

pub struct Host {
    hwnd: HWND,
    instance: HINSTANCE,
    class_name: Vec<u16>,
    class_registered: bool,
    com_initialized: bool,
    environment: Option<ICoreWebView2Environment>,
    controller: Option<ICoreWebView2Controller>,
    webview: Option<ICoreWebView2>,
    webview3: Option<ICoreWebView2_3>,
    pub version: String,
}

impl Host {
    pub fn new(user_data: &Path, deadline: Instant) -> Result<Self, String> {
        let mut version_ptr = PWSTR::null();
        let probe = unsafe {
            GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version_ptr)
        };
        if let Err(error) = probe {
            return Err(format!("WebView2 runtime missing: {error}"));
        }
        if version_ptr.is_null() {
            return Err("WebView2 runtime missing: empty version".into());
        }
        let version = take_pwstr(version_ptr);
        let mut h = Self {
            hwnd: HWND::default(),
            instance: HINSTANCE::default(),
            class_name: wide("mIV-epub-pdf-worker-hidden-host"),
            class_registered: false,
            com_initialized: false,
            environment: None,
            controller: None,
            webview: None,
            webview3: None,
            version,
        };
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .map_err(|e| format!("CoInitializeEx: {e}"))?;
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
                unsafe { GetLastError() }.to_hresult()
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

        let (tx, rx) = mpsc::channel();
        let callback = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
            move |result, environment| {
                let _ = tx.send(result.and_then(|()| {
                    environment.ok_or_else(|| windows::core::Error::from(E_POINTER))
                }));
                Ok(())
            },
        ));
        let ud = wide(&user_data.to_string_lossy());
        unsafe {
            CreateCoreWebView2EnvironmentWithOptions(
                PCWSTR::null(),
                PCWSTR(ud.as_ptr()),
                None,
                &callback,
            )
        }
        .map_err(|e| format!("CreateCoreWebView2EnvironmentWithOptions: {e}"))?;
        h.environment = Some(wait(rx, deadline).map_err(|e| {
            format!(
                "WebView2 runtime {}; environment creation failed: {e}",
                h.version
            )
        })?);

        let (tx, rx) = mpsc::channel();
        let callback = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
            move |result, controller| {
                let _ = tx.send(result.and_then(|()| {
                    controller.ok_or_else(|| windows::core::Error::from(E_POINTER))
                }));
                Ok(())
            },
        ));
        unsafe {
            h.environment
                .as_ref()
                .unwrap()
                .CreateCoreWebView2Controller(h.hwnd, &callback)
        }
        .map_err(|e| format!("CreateCoreWebView2Controller: {e}"))?;
        h.controller = Some(wait(rx, deadline).map_err(|e| {
            format!(
                "WebView2 runtime {}; controller creation failed: {e}",
                h.version
            )
        })?);
        let controller = h.controller.as_ref().unwrap();
        unsafe {
            controller.SetBounds(RECT {
                left: 0,
                top: 0,
                right: 1200,
                bottom: 1700,
            })
        }
        .map_err(|e| format!("put_Bounds: {e}"))?;
        let view =
            unsafe { controller.CoreWebView2() }.map_err(|e| format!("get_CoreWebView2: {e}"))?;
        h.webview3 = Some(view.cast().map_err(|e| format!("ICoreWebView2_3: {e}"))?);
        h.webview = Some(view);
        Ok(h)
    }

    fn view(&self) -> &ICoreWebView2 {
        self.webview.as_ref().unwrap()
    }

    pub fn map_folder(&self, dir: &Path) -> Result<(), String> {
        let host = wide("epub.invalid");
        let folder = wide(&dir.to_string_lossy());
        unsafe {
            self.webview3
                .as_ref()
                .unwrap()
                .SetVirtualHostNameToFolderMapping(
                    PCWSTR(host.as_ptr()),
                    PCWSTR(folder.as_ptr()),
                    COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY,
                )
        }
        .map_err(|e| format!("SetVirtualHostNameToFolderMapping: {e}"))
    }

    pub fn navigate(&self, url: &str) -> Result<(), String> {
        let url = wide(url);
        unsafe { self.view().Navigate(PCWSTR(url.as_ptr())) }.map_err(|e| format!("Navigate: {e}"))
    }

    pub fn script(&self, script: &str, deadline: Instant) -> Result<String, String> {
        let (tx, rx) = mpsc::channel();
        let callback = ExecuteScriptCompletedHandler::create(Box::new(move |result, text| {
            let _ = tx.send(result.map(|()| text));
            Ok(())
        }));
        let script = wide(script);
        unsafe {
            self.view()
                .ExecuteScript(PCWSTR(script.as_ptr()), &callback)
        }
        .map_err(|e| format!("ExecuteScript: {e}"))?;
        wait(rx, deadline)
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
        let (tx, rx) = mpsc::channel();
        let callback =
            CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, text| {
                let _ = tx.send(result.map(|()| text));
                Ok(())
            }));
        let method = wide("Page.printToPDF");
        let args = wide(
            r#"{"preferCSSPageSize":true,"printBackground":true,"marginTop":0,"marginBottom":0,"marginLeft":0,"marginRight":0,"displayHeaderFooter":false}"#,
        );
        unsafe {
            self.view().CallDevToolsProtocolMethod(
                PCWSTR(method.as_ptr()),
                PCWSTR(args.as_ptr()),
                &callback,
            )
        }
        .map_err(|e| format!("CallDevToolsProtocolMethod: {e}"))?;
        let response = wait(rx, deadline)?;
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

    pub fn close(&mut self) {
        if let Some(controller) = &self.controller {
            let _ = unsafe { controller.Close() };
        }
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
        if self.com_initialized {
            unsafe { CoUninitialize() };
        }
    }
}
