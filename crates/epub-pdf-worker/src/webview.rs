//! WebView2 host for EPUB printing. All COM objects belong to this STA thread.
use base64::Engine;
use std::{
    cell::RefCell,
    collections::HashSet,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, CoreWebView2EnvironmentOptions,
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
        CreateCoreWebView2EnvironmentWithOptions, GetAvailableCoreWebView2BrowserVersionString,
        ICoreWebView2, ICoreWebView2_3, ICoreWebView2_22, ICoreWebView2Controller,
        ICoreWebView2Environment, ICoreWebView2Environment7, ICoreWebView2EnvironmentOptions,
    },
    NavigationStartingEventHandler, WebResourceRequestedEventHandler, take_pwstr,
};
use windows::{
    Win32::{
        Foundation::{
            E_NOINTERFACE, E_POINTER, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
        },
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
            LibraryLoader::GetModuleHandleW,
        },
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG,
            MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
            RegisterClassW, TranslateMessage, UnregisterClassW, WINDOW_EX_STYLE, WNDCLASSW,
            WS_OVERLAPPEDWINDOW,
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

fn wait_timeout_ms(deadline: Instant) -> u32 {
    let remaining = deadline.saturating_duration_since(Instant::now());
    remaining.as_millis().min(60_000) as u32
}

fn wait_for_messages(deadline: Instant) {
    let millis = wait_timeout_ms(deadline);
    unsafe { MsgWaitForMultipleObjectsEx(None, millis, QS_ALLINPUT, MWMO_INPUTAVAILABLE) };
    pump();
}

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
        wait_for_messages(deadline);
    }
}

pub const MAX_RECORDED_BLOCKED_URLS: usize = 200;
pub const BROWSER_NETWORK_ARGUMENTS: &str = "--host-resolver-rules=\"MAP * ^NOTFOUND\" --disable-background-networking --dns-prefetch-disable --disable-preconnect --no-pings --disable-sync --disable-component-update --disable-extensions --no-first-run";

fn is_webview_environment_key(key: &std::ffi::OsStr) -> bool {
    key.to_string_lossy()
        .to_ascii_uppercase()
        .starts_with("WEBVIEW2_")
}

pub fn clear_webview_environment() {
    // The loader recognizes WEBVIEW2_* process variables, and may add new
    // overrides in later runtimes. Remove the entire namespace before probing.
    for (key, _) in std::env::vars_os() {
        if is_webview_environment_key(&key) {
            // SAFETY: called at worker start, before any WebView2 or worker threads.
            unsafe { std::env::remove_var(key) };
        }
    }
}

#[derive(Default)]
struct UserDataFolderObservation {
    redirected_to: Option<PathBuf>,
    error: Option<String>,
}

fn observe_user_data_folder(
    requested: &Path,
    actual: Result<PathBuf, String>,
    same_identity: impl FnOnce(&Path, &Path) -> Result<bool, String>,
) -> UserDataFolderObservation {
    match actual {
        Ok(actual) => match same_identity(requested, &actual) {
            Ok(true) => UserDataFolderObservation::default(),
            Ok(false) => UserDataFolderObservation {
                redirected_to: Some(actual),
                error: None,
            },
            Err(error) => UserDataFolderObservation {
                redirected_to: None,
                error: Some(format!(
                    "cannot compare requested {} with actual {}: {error}",
                    requested.display(),
                    actual.display()
                )),
            },
        },
        Err(error) => UserDataFolderObservation {
            redirected_to: None,
            error: Some(error),
        },
    }
}

fn network_options() -> Result<ICoreWebView2EnvironmentOptions, String> {
    let options: ICoreWebView2EnvironmentOptions = CoreWebView2EnvironmentOptions::default().into();
    let args = wide(BROWSER_NETWORK_ARGUMENTS);
    unsafe { options.SetAdditionalBrowserArguments(PCWSTR(args.as_ptr())) }
        .map_err(|e| format!("SetAdditionalBrowserArguments: {e}"))?;
    Ok(options)
}

#[derive(Default)]
struct BlockedRequests {
    urls: Vec<String>,
    seen: HashSet<String>,
    count: usize,
}

impl BlockedRequests {
    fn record(&mut self, url: String) {
        self.count += 1;
        if self.urls.len() < MAX_RECORDED_BLOCKED_URLS && self.seen.insert(url.clone()) {
            self.urls.push(url);
        }
    }
}

fn allowed_virtual_url(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    scheme.eq_ignore_ascii_case("https") && authority.eq_ignore_ascii_case("epub.invalid")
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
    blocked: Rc<RefCell<BlockedRequests>>,
    pub request_filter: String,
    pub version: String,
    pub user_data_folder_redirected: Option<PathBuf>,
    pub user_data_folder_check_error: Option<String>,
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
            blocked: Rc::new(RefCell::new(BlockedRequests::default())),
            request_filter: String::new(),
            version,
            user_data_folder_redirected: None,
            user_data_folder_check_error: None,
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
        let options = network_options()?;
        unsafe {
            CreateCoreWebView2EnvironmentWithOptions(
                PCWSTR::null(),
                PCWSTR(ud.as_ptr()),
                &options,
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
        let actual = (|| -> Result<PathBuf, String> {
            let environment7: ICoreWebView2Environment7 = h
                .environment
                .as_ref()
                .unwrap()
                .cast()
                .map_err(|e| format!("ICoreWebView2Environment7 unavailable: {e}"))?;
            let mut actual_ptr = PWSTR::null();
            unsafe { environment7.UserDataFolder(&mut actual_ptr) }
                .map_err(|e| format!("UserDataFolder query failed: {e}"))?;
            if actual_ptr.is_null() {
                return Err("UserDataFolder query returned no path".into());
            }
            Ok(PathBuf::from(take_pwstr(actual_ptr)))
        })();
        let observation =
            observe_user_data_folder(user_data, actual, crate::paths::same_directory_identity);
        if let Some(actual) = &observation.redirected_to {
            eprintln!(
                "WebView2 user data folder differs: requested {}, actual {}",
                user_data.display(),
                actual.display()
            );
        }
        if let Some(error) = &observation.error {
            eprintln!("WebView2 user data folder check: {error}");
        }
        h.user_data_folder_redirected = observation.redirected_to;
        h.user_data_folder_check_error = observation.error;

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
        let settings = unsafe { h.view().Settings() }.map_err(|e| format!("Settings: {e}"))?;
        unsafe { settings.SetIsScriptEnabled(false) }
            .map_err(|e| format!("SetIsScriptEnabled: {e}"))?;
        h.request_filter = h.install_request_guard()?;
        Ok(h)
    }

    fn view(&self) -> &ICoreWebView2 {
        self.webview.as_ref().unwrap()
    }

    fn install_request_guard(&self) -> Result<String, String> {
        let filter = wide("*");
        let view22 = self.view().cast::<ICoreWebView2_22>().map_err(|error| {
            if error.code() == E_NOINTERFACE {
                format!("WebView2 unsupported: ICoreWebView2_22 is required: {error}")
            } else {
                format!("ICoreWebView2_22: {error}")
            }
        })?;
        unsafe {
            view22.AddWebResourceRequestedFilterWithRequestSourceKinds(
                PCWSTR(filter.as_ptr()),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
            )
        }
        .map_err(|e| format!("AddWebResourceRequestedFilterWithRequestSourceKinds: {e}"))?;
        let blocked = self.blocked.clone();
        let environment = self.environment.as_ref().unwrap().clone();
        let handler = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut uri = PWSTR::null();
            let url = match unsafe { args.Request().and_then(|request| request.Uri(&mut uri)) } {
                Ok(()) => take_pwstr(uri),
                Err(_) => "<unavailable resource URL>".into(),
            };
            if !allowed_virtual_url(&url) {
                blocked.borrow_mut().record(url);
                let reason = wide("Blocked");
                let headers = wide("Cache-Control: no-store\r\nContent-Type: text/plain\r\n");
                let response = unsafe {
                    environment.CreateWebResourceResponse(
                        None,
                        403,
                        PCWSTR(reason.as_ptr()),
                        PCWSTR(headers.as_ptr()),
                    )?
                };
                unsafe { args.SetResponse(&response)? };
            }
            Ok(())
        }));
        let mut token = 0;
        unsafe { self.view().add_WebResourceRequested(&handler, &mut token) }
            .map_err(|e| format!("add_WebResourceRequested: {e}"))?;
        let blocked = self.blocked.clone();
        let navigation = NavigationStartingEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut uri = PWSTR::null();
            let url = match unsafe { args.Uri(&mut uri) } {
                Ok(()) => take_pwstr(uri),
                Err(_) => "<unavailable navigation URL>".into(),
            };
            if !allowed_virtual_url(&url) {
                blocked.borrow_mut().record(url);
                unsafe { args.SetCancel(true)? };
            }
            Ok(())
        }));
        unsafe { self.view().add_NavigationStarting(&navigation, &mut token) }
            .map_err(|e| format!("add_NavigationStarting: {e}"))?;
        unsafe {
            self.view()
                .add_FrameNavigationStarting(&navigation, &mut token)
        }
        .map_err(|e| format!("add_FrameNavigationStarting: {e}"))?;
        Ok("all_source_kinds".into())
    }

    pub fn reset_blocked(&self) {
        *self.blocked.borrow_mut() = BlockedRequests::default();
    }

    pub fn blocked_requests(&self) -> (Vec<String>, usize) {
        let blocked = self.blocked.borrow();
        (blocked.urls.clone(), blocked.count)
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

    pub fn script(&self, script: &str, deadline: Instant) -> Result<serde_json::Value, String> {
        // ExecuteScript's interaction with disabled page scripts is not stated
        // in the local bindings. Use CDP for host expressions; the reviewer
        // must verify this path on a running WebView2 outside the sandbox.
        let response = self.devtools_call(
            "Runtime.evaluate",
            &serde_json::json!({"expression":script,"returnByValue":true}),
            deadline,
        )?;
        if response.get("exceptionDetails").is_some() {
            return Err(format!("Runtime.evaluate: {response}"));
        }
        Ok(response["result"]["value"].clone())
    }

    fn devtools_call(
        &self,
        name: &str,
        params: &serde_json::Value,
        deadline: Instant,
    ) -> Result<serde_json::Value, String> {
        let (tx, rx) = mpsc::channel();
        let callback =
            CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, text| {
                let _ = tx.send(result.map(|()| text));
                Ok(())
            }));
        let method = wide(name);
        let args = wide(&params.to_string());
        unsafe {
            self.view().CallDevToolsProtocolMethod(
                PCWSTR(method.as_ptr()),
                PCWSTR(args.as_ptr()),
                &callback,
            )
        }
        .map_err(|e| format!("CallDevToolsProtocolMethod({name}): {e}"))?;
        let response = wait(rx, deadline)?;
        serde_json::from_str(&response).map_err(|e| format!("invalid CDP response: {e}"))
    }

    pub fn wait_ready(&self, url: &str, deadline: Instant) -> Result<(), String> {
        let desired = serde_json::to_string(url).map_err(|e| e.to_string())?;
        let test = format!(
            r#"(()=>{{const local=u=>{{try{{return new URL(u,location.href).origin===location.origin}}catch{{return false}}}};const images=d=>[...d.images].every(i=>i.complete&&(!local(i.src)||i.naturalWidth>0));return location.href===new URL({desired}).href&&document.readyState==='complete'&&document.fonts.status==='loaded'&&images(document)&&[...document.querySelectorAll('iframe')].every(f=>!local(f.src)||(f.contentDocument&&f.contentDocument.readyState==='complete'&&f.contentDocument.fonts.status==='loaded'&&images(f.contentDocument)))}})()"#
        );
        loop {
            if Instant::now() >= deadline {
                return Err("WebView2 timeout waiting for images/iframes/fonts".into());
            }
            if self.script(&test, deadline)? == true {
                return Ok(());
            }
            wait_for_messages((Instant::now() + Duration::from_millis(100)).min(deadline));
        }
    }

    pub fn book_script_ran(&self, deadline: Instant) -> Result<bool, String> {
        Ok(self.script("document.body?.dataset?.mivBookScriptRan === '1' || [...document.querySelectorAll('iframe')].some(f => f.contentDocument?.body?.dataset?.mivBookScriptRan === '1')", deadline)? == true)
    }

    pub fn devtools_pdf(&self, deadline: Instant) -> Result<Vec<u8>, String> {
        let parsed = self.devtools_call("Page.printToPDF", &serde_json::json!({"preferCSSPageSize":true,"printBackground":true,"marginTop":0,"marginBottom":0,"marginLeft":0,"marginRight":0,"displayHeaderFooter":false}), deadline)?;
        if let Some(error) = parsed.get("error") {
            return Err(format!("Page.printToPDF: {error}"));
        }
        let data = parsed.get("data").and_then(|x| x.as_str()).ok_or_else(|| {
            format!(
                "Page.printToPDF omitted data: {}",
                parsed.to_string().chars().take(300).collect::<String>()
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn virtual_host_only() {
        assert!(allowed_virtual_url("https://epub.invalid/a"));
        for url in [
            "http://epub.invalid/a",
            "https://epub.invalid.evil/a",
            "https://epub.invalid:443/a",
            "https://user@epub.invalid/a",
            "file:///a",
            "data:text/plain,a",
        ] {
            assert!(!allowed_virtual_url(url), "{url}");
        }
    }
    #[test]
    fn blocked_is_capped_and_counted() {
        let mut b = BlockedRequests::default();
        for i in 0..300 {
            b.record(format!("http://example.invalid/{i}"));
        }
        b.record("http://example.invalid/0".into());
        assert_eq!(b.count, 301);
        assert_eq!(b.urls.len(), MAX_RECORDED_BLOCKED_URLS);
    }

    #[test]
    fn message_wait_is_always_finite() {
        assert_eq!(
            wait_timeout_ms(Instant::now() + Duration::from_secs(172_800)),
            60_000
        );
        assert!(wait_timeout_ms(Instant::now()) < u32::MAX);
    }

    #[test]
    fn browser_options_carry_network_arguments() {
        let options = network_options().unwrap();
        let mut arguments = PWSTR::null();
        unsafe { options.AdditionalBrowserArguments(&mut arguments) }.unwrap();
        assert_eq!(take_pwstr(arguments), BROWSER_NETWORK_ARGUMENTS);
        assert_eq!(
            BROWSER_NETWORK_ARGUMENTS,
            "--host-resolver-rules=\"MAP * ^NOTFOUND\" --disable-background-networking --dns-prefetch-disable --disable-preconnect --no-pings --disable-sync --disable-component-update --disable-extensions --no-first-run"
        );
    }

    #[test]
    fn clears_entire_webview_override_namespace() {
        for key in [
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
            "WEBVIEW2_USER_DATA_FOLDER",
            "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
            "WEBVIEW2_RELEASE_CHANNEL_PREFERENCE",
            "WEBVIEW2_PIPE_FOR_SCRIPT_DEBUGGER",
            "webview2_future_override",
        ] {
            assert!(is_webview_environment_key(std::ffi::OsStr::new(key)));
        }
        assert!(!is_webview_environment_key(std::ffi::OsStr::new(
            "MIV_EPUB_PDF_TEST_PANIC"
        )));
    }

    #[test]
    fn user_data_folder_mismatch_is_diagnostic() {
        let requested = Path::new(r"C:\worker\requested");
        let actual = PathBuf::from(r"C:\policy\actual");
        let observation = observe_user_data_folder(requested, Ok(actual.clone()), |_, _| Ok(false));
        assert_eq!(observation.redirected_to, Some(actual));
        assert!(observation.error.is_none());
    }

    #[test]
    fn user_data_folder_identity_error_is_diagnostic() {
        let actual = PathBuf::from(r"C:\policy\actual");
        let observation = observe_user_data_folder(
            Path::new(r"C:\worker\requested"),
            Ok(actual.clone()),
            |_, _| Err("access denied".into()),
        );
        assert!(observation.redirected_to.is_none());
        let error = observation.error.unwrap();
        assert!(error.contains(&actual.display().to_string()));
        assert!(error.contains("access denied"));

        let observation = observe_user_data_folder(
            Path::new(r"C:\worker\requested"),
            Err("query unavailable".into()),
            |_, _| unreachable!(),
        );
        assert!(observation.redirected_to.is_none());
        assert_eq!(observation.error.as_deref(), Some("query unavailable"));
    }
}
