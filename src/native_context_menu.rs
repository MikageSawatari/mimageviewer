//! Native Windows Shell context menu bridge.
//!
//! The module owns only Win32/Shell menu plumbing. mIV-specific side effects are
//! returned as command IDs and executed by the caller on the normal App path.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellClipboardVerb {
    Copy,
    Cut,
    Paste,
}

impl ShellClipboardVerb {
    fn canonical_name(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Cut => "cut",
            Self::Paste => "paste",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeMivCommand {
    CopyPath,
    CopyFileName,
    CopyImageToClipboard,
    JumpToFolder,
    RotateLeft,
    RotateRight,
    ToggleRepresentativeThumb,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeMivMenuItem {
    pub command: NativeMivCommand,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct NativeContextMenuRequest {
    pub hwnd: isize,
    pub screen_pos: (i32, i32),
    pub background_folder: Option<PathBuf>,
    pub paths: Vec<PathBuf>,
    pub miv_items: Vec<NativeMivMenuItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeContextMenuResult {
    Canceled,
    MivCommand(NativeMivCommand),
    ShellCommandInvoked,
    Fallback { reason: String },
}

const SHELL_ID_FIRST: u32 = 1;
const SHELL_ID_LAST: u32 = 0x6fff;
const MIV_ID_FIRST: u32 = 0x7000;

fn miv_command_id(index: usize) -> Option<u32> {
    MIV_ID_FIRST.checked_add(u32::try_from(index).ok()?)
}

fn miv_command_index(command_id: u32, len: usize) -> Option<usize> {
    let index = command_id.checked_sub(MIV_ID_FIRST)? as usize;
    (index < len).then_some(index)
}

fn shell_verb_offset(command_id: u32) -> Option<u32> {
    command_id.checked_sub(SHELL_ID_FIRST)
}

#[cfg(windows)]
pub fn show_native_context_menu(request: NativeContextMenuRequest) -> NativeContextMenuResult {
    windows_impl::show_native_context_menu(request)
}

#[cfg(windows)]
pub fn invoke_shell_file_verb(
    hwnd: isize,
    paths: &[PathBuf],
    verb: ShellClipboardVerb,
) -> Result<(), String> {
    windows_impl::invoke_shell_file_verb(hwnd, paths, verb)
}

#[cfg(windows)]
pub fn invoke_shell_folder_background_verb(
    hwnd: isize,
    folder: &std::path::Path,
    verb: ShellClipboardVerb,
) -> Result<(), String> {
    windows_impl::invoke_shell_folder_background_verb(hwnd, folder, verb)
}

#[cfg(not(windows))]
pub fn show_native_context_menu(request: NativeContextMenuRequest) -> NativeContextMenuResult {
    let _ = request;
    NativeContextMenuResult::Fallback {
        reason: "native Shell context menu is available only on Windows".to_string(),
    }
}

#[cfg(not(windows))]
pub fn invoke_shell_file_verb(
    hwnd: isize,
    paths: &[PathBuf],
    verb: ShellClipboardVerb,
) -> Result<(), String> {
    let _ = (hwnd, paths, verb);
    Err("Shell clipboard verbs are available only on Windows".to_string())
}

#[cfg(not(windows))]
pub fn invoke_shell_folder_background_verb(
    hwnd: isize,
    folder: &std::path::Path,
    verb: ShellClipboardVerb,
) -> Result<(), String> {
    let _ = (hwnd, folder, verb);
    Err("Shell clipboard verbs are available only on Windows".to_string())
}

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use std::ffi::CString;
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::Foundation::{
        HANDLE, HWND, LPARAM, LRESULT, POINT, RPC_E_CHANGED_MODE, WPARAM,
    };
    use windows::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx,
    };
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_SFUIObject, CMF_CANRENAME, CMF_EXPLORE, CMF_NORMAL, CMINVOKECOMMANDINFO,
        DefSubclassProc, IContextMenu, IContextMenu2, IContextMenu3, IShellFolder,
        RemoveWindowSubclass, SHCreateShellItemArrayFromIDLists, SHGetDesktopFolder,
        SHParseDisplayName, SetWindowSubclass,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, HMENU, MF_SEPARATOR, MF_STRING,
        SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_DRAWITEM,
        WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR,
    };
    use windows::core::{Interface, PCSTR, PCWSTR};

    const SUBCLASS_ID: usize = 0x6d69_7643; // "mivC"

    pub(super) fn show_native_context_menu(
        request: NativeContextMenuRequest,
    ) -> NativeContextMenuResult {
        if request.hwnd == 0 {
            return NativeContextMenuResult::Fallback {
                reason: "main HWND is not available".to_string(),
            };
        }
        if request.background_folder.is_none()
            && request.paths.is_empty()
            && request.miv_items.is_empty()
        {
            return NativeContextMenuResult::Canceled;
        }

        let _com = match ComStaGuard::new() {
            Ok(guard) => guard,
            Err(reason) => return NativeContextMenuResult::Fallback { reason },
        };

        let menu = match unsafe { CreatePopupMenu() } {
            Ok(menu) => MenuGuard::new(menu),
            Err(e) => {
                return NativeContextMenuResult::Fallback {
                    reason: format!("CreatePopupMenu failed: {e}"),
                };
            }
        };

        for (index, item) in request.miv_items.iter().enumerate() {
            let Some(id) = miv_command_id(index) else {
                return NativeContextMenuResult::Fallback {
                    reason: "too many mIV context menu commands".to_string(),
                };
            };
            if let Err(e) = append_menu_string(menu.handle(), id, &item.label) {
                return NativeContextMenuResult::Fallback {
                    reason: format!("AppendMenuW(mIV) failed: {e}"),
                };
            }
        }

        let hwnd = HWND(request.hwnd as *mut core::ffi::c_void);
        let shell_menu = if let Some(folder) = request.background_folder.as_ref() {
            if !request.miv_items.is_empty()
                && unsafe { AppendMenuW(menu.handle(), MF_SEPARATOR, 0, PCWSTR::null()) }.is_err()
            {
                return NativeContextMenuResult::Fallback {
                    reason: "AppendMenuW(separator) failed".to_string(),
                };
            }

            let shell_menu = match shell_context_menu_for_folder_background(hwnd, folder) {
                Ok(menu) => Some(menu),
                Err(reason) => return NativeContextMenuResult::Fallback { reason },
            };
            if let Some(shell_menu) = shell_menu.as_ref() {
                let insert_at =
                    request.miv_items.len() as u32 + u32::from(!request.miv_items.is_empty());
                if let Err(reason) = query_shell_context_menu(shell_menu, menu.handle(), insert_at)
                {
                    return NativeContextMenuResult::Fallback { reason };
                }
            }
            shell_menu
        } else if request.paths.is_empty() {
            None
        } else {
            if !request.miv_items.is_empty()
                && unsafe { AppendMenuW(menu.handle(), MF_SEPARATOR, 0, PCWSTR::null()) }.is_err()
            {
                return NativeContextMenuResult::Fallback {
                    reason: "AppendMenuW(separator) failed".to_string(),
                };
            }

            let shell_menu = match shell_context_menu_for_paths(&request.paths) {
                Ok(menu) => menu,
                Err(reason) => return NativeContextMenuResult::Fallback { reason },
            };
            let insert_at =
                request.miv_items.len() as u32 + u32::from(!request.miv_items.is_empty());
            if let Err(reason) = query_shell_context_menu(&shell_menu, menu.handle(), insert_at) {
                return NativeContextMenuResult::Fallback { reason };
            }
            Some(shell_menu)
        };

        let forwarder = shell_menu
            .as_ref()
            .map(ContextMenuMessageForwarder::from_context_menu);
        let _subclass_guard = forwarder
            .as_ref()
            .and_then(|forwarder| MenuSubclassGuard::install(hwnd, forwarder));

        let (screen_x, screen_y) = cursor_screen_pos().unwrap_or(request.screen_pos);
        let selected = unsafe {
            TrackPopupMenuEx(
                menu.handle(),
                (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
                screen_x,
                screen_y,
                hwnd,
                None,
            )
            .0 as u32
        };
        if selected == 0 {
            return NativeContextMenuResult::Canceled;
        }

        if let Some(index) = miv_command_index(selected, request.miv_items.len()) {
            return NativeContextMenuResult::MivCommand(request.miv_items[index].command);
        }

        let Some(shell_menu) = shell_menu else {
            return NativeContextMenuResult::Fallback {
                reason: format!("selected unknown command id {selected}"),
            };
        };
        let Some(offset) = shell_verb_offset(selected) else {
            return NativeContextMenuResult::Fallback {
                reason: format!("selected command id {selected} is outside Shell range"),
            };
        };

        let invoke = CMINVOKECOMMANDINFO {
            cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
            hwnd,
            lpVerb: PCSTR(offset as usize as *const u8),
            nShow: SW_SHOWNORMAL.0,
            hIcon: HANDLE::default(),
            ..Default::default()
        };
        match unsafe { shell_menu.InvokeCommand(&invoke) } {
            Ok(()) => NativeContextMenuResult::ShellCommandInvoked,
            Err(e) => NativeContextMenuResult::Fallback {
                reason: format!("IContextMenu::InvokeCommand failed: {e}"),
            },
        }
    }

    pub(super) fn invoke_shell_file_verb(
        hwnd: isize,
        paths: &[PathBuf],
        verb: ShellClipboardVerb,
    ) -> Result<(), String> {
        if hwnd == 0 {
            return Err("main HWND is not available".to_string());
        }
        if paths.is_empty() {
            return Ok(());
        }
        let _com = ComStaGuard::new()?;
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        let menu = shell_context_menu_for_paths(paths)?;
        let popup = MenuGuard::new(
            unsafe { CreatePopupMenu() }.map_err(|e| format!("CreatePopupMenu failed: {e}"))?,
        );
        query_shell_context_menu(&menu, popup.handle(), 0)?;
        invoke_canonical_verb(&menu, hwnd, verb)
    }

    pub(super) fn invoke_shell_folder_background_verb(
        hwnd: isize,
        folder: &std::path::Path,
        verb: ShellClipboardVerb,
    ) -> Result<(), String> {
        if hwnd == 0 {
            return Err("main HWND is not available".to_string());
        }
        let _com = ComStaGuard::new()?;
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        let menu = shell_context_menu_for_folder_background(hwnd, folder)?;
        let popup = MenuGuard::new(
            unsafe { CreatePopupMenu() }.map_err(|e| format!("CreatePopupMenu failed: {e}"))?,
        );
        query_shell_context_menu(&menu, popup.handle(), 0)?;
        invoke_canonical_verb(&menu, hwnd, verb)
    }

    fn append_menu_string(menu: HMENU, id: u32, label: &str) -> windows::core::Result<()> {
        let wide = wide_null(label);
        unsafe { AppendMenuW(menu, MF_STRING, id as usize, PCWSTR(wide.as_ptr())) }
    }

    fn invoke_canonical_verb(
        shell_menu: &IContextMenu,
        hwnd: HWND,
        verb: ShellClipboardVerb,
    ) -> Result<(), String> {
        let verb_name = verb.canonical_name();
        let verb_cstr = CString::new(verb_name).expect("static Shell verb has no NUL");
        let invoke = CMINVOKECOMMANDINFO {
            cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
            hwnd,
            lpVerb: PCSTR(verb_cstr.as_ptr() as *const u8),
            nShow: SW_SHOWNORMAL.0,
            hIcon: HANDLE::default(),
            ..Default::default()
        };
        unsafe { shell_menu.InvokeCommand(&invoke) }
            .map_err(|e| format!("IContextMenu::InvokeCommand({verb_name}) failed: {e}"))
    }

    fn query_shell_context_menu(
        shell_menu: &IContextMenu,
        menu: HMENU,
        insert_at: u32,
    ) -> Result<(), String> {
        let hr = unsafe {
            shell_menu.QueryContextMenu(
                menu,
                insert_at,
                SHELL_ID_FIRST,
                SHELL_ID_LAST,
                CMF_NORMAL | CMF_EXPLORE | CMF_CANRENAME,
            )
        };
        if hr.is_err() {
            Err(format!(
                "IContextMenu::QueryContextMenu failed: 0x{:08x}",
                hr.0
            ))
        } else {
            Ok(())
        }
    }

    fn shell_context_menu_for_paths(paths: &[PathBuf]) -> Result<IContextMenu, String> {
        let mut pidls: Vec<*const ITEMIDLIST> = Vec::with_capacity(paths.len());
        let mut failed_paths = 0usize;
        for path in paths {
            let wide = shell_parse_wide_path(path);
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            let parsed = unsafe {
                SHParseDisplayName(PCWSTR(wide.as_ptr()), None::<&IBindCtx>, &mut pidl, 0, None)
            };
            if parsed.is_ok() && !pidl.is_null() {
                pidls.push(pidl as *const ITEMIDLIST);
            } else {
                failed_paths += 1;
                if !pidl.is_null() {
                    unsafe { CoTaskMemFree(Some(pidl as *const core::ffi::c_void)) };
                }
            }
        }

        let free_pidls = |pidls: &[*const ITEMIDLIST]| {
            for &pidl in pidls {
                unsafe { CoTaskMemFree(Some(pidl as *const core::ffi::c_void)) };
            }
        };

        if pidls.is_empty() {
            return Err("SHParseDisplayName failed for all context menu paths".to_string());
        }

        let array = match unsafe { SHCreateShellItemArrayFromIDLists(&pidls) } {
            Ok(array) => array,
            Err(e) => {
                free_pidls(&pidls);
                return Err(format!("SHCreateShellItemArrayFromIDLists failed: {e}"));
            }
        };
        free_pidls(&pidls);

        let menu: IContextMenu = match unsafe {
            array.BindToHandler(None::<&IBindCtx>, &BHID_SFUIObject)
        } {
            Ok(menu) => menu,
            Err(e) => {
                return Err(format!(
                    "IShellItemArray::BindToHandler(BHID_SFUIObject) failed: {e}; failed_paths={failed_paths}"
                ));
            }
        };
        Ok(menu)
    }

    fn shell_context_menu_for_folder_background(
        hwnd: HWND,
        folder: &std::path::Path,
    ) -> Result<IContextMenu, String> {
        let wide = shell_parse_wide_path(folder);
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        unsafe { SHParseDisplayName(PCWSTR(wide.as_ptr()), None::<&IBindCtx>, &mut pidl, 0, None) }
            .map_err(|e| format!("SHParseDisplayName(background folder) failed: {e}"))?;
        if pidl.is_null() {
            return Err("SHParseDisplayName(background folder) returned null PIDL".to_string());
        }

        let desktop = unsafe { SHGetDesktopFolder() }
            .map_err(|e| format!("SHGetDesktopFolder failed: {e}"))?;
        let folder_shell: IShellFolder =
            match unsafe { desktop.BindToObject(pidl as *const ITEMIDLIST, None::<&IBindCtx>) } {
                Ok(folder_shell) => folder_shell,
                Err(e) => {
                    unsafe { CoTaskMemFree(Some(pidl as *const core::ffi::c_void)) };
                    return Err(format!("IShellFolder::BindToObject failed: {e}"));
                }
            };
        unsafe { CoTaskMemFree(Some(pidl as *const core::ffi::c_void)) };

        unsafe { folder_shell.CreateViewObject(hwnd) }
            .map_err(|e| format!("IShellFolder::CreateViewObject(IContextMenu) failed: {e}"))
    }

    fn shell_parse_wide_path(path: &std::path::Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .map(|ch| if ch == b'/' as u16 { b'\\' as u16 } else { ch })
            .chain(std::iter::once(0))
            .collect()
    }

    fn wide_null(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn cursor_screen_pos() -> Option<(i32, i32)> {
        let mut point = POINT::default();
        unsafe { GetCursorPos(&mut point) }
            .is_ok()
            .then_some((point.x, point.y))
    }

    struct MenuGuard(HMENU);

    impl MenuGuard {
        fn new(menu: HMENU) -> Self {
            Self(menu)
        }

        fn handle(&self) -> HMENU {
            self.0
        }
    }

    impl Drop for MenuGuard {
        fn drop(&mut self) {
            let _ = unsafe { DestroyMenu(self.0) };
        }
    }

    struct ComStaGuard {
        uninitialize: bool,
    }

    impl ComStaGuard {
        fn new() -> Result<Self, String> {
            let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            if hr.is_ok() {
                Ok(Self { uninitialize: true })
            } else if hr == RPC_E_CHANGED_MODE {
                Ok(Self {
                    uninitialize: false,
                })
            } else {
                Err(format!("CoInitializeEx(STA) failed: 0x{:08x}", hr.0))
            }
        }
    }

    impl Drop for ComStaGuard {
        fn drop(&mut self) {
            if self.uninitialize {
                unsafe { CoUninitialize() };
            }
        }
    }

    struct ContextMenuMessageForwarder {
        menu2: Option<IContextMenu2>,
        menu3: Option<IContextMenu3>,
    }

    impl ContextMenuMessageForwarder {
        fn from_context_menu(menu: &IContextMenu) -> Self {
            Self {
                menu2: menu.cast::<IContextMenu2>().ok(),
                menu3: menu.cast::<IContextMenu3>().ok(),
            }
        }

        unsafe fn forward(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
            if !matches!(
                msg,
                WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR
            ) {
                return None;
            }

            if let Some(menu3) = &self.menu3 {
                let mut result = LRESULT(0);
                if unsafe {
                    menu3
                        .HandleMenuMsg2(msg, wparam, lparam, Some(&mut result))
                        .is_ok()
                } {
                    return Some(result);
                }
            }
            if let Some(menu2) = &self.menu2 {
                let _ = unsafe { menu2.HandleMenuMsg(msg, wparam, lparam) };
                return Some(LRESULT(0));
            }
            None
        }
    }

    struct MenuSubclassGuard {
        hwnd: HWND,
        installed: bool,
    }

    impl MenuSubclassGuard {
        fn install(hwnd: HWND, forwarder: &ContextMenuMessageForwarder) -> Option<Self> {
            if forwarder.menu2.is_none() && forwarder.menu3.is_none() {
                return None;
            }
            let installed = unsafe {
                SetWindowSubclass(
                    hwnd,
                    Some(context_menu_subclass_proc),
                    SUBCLASS_ID,
                    forwarder as *const ContextMenuMessageForwarder as usize,
                )
                .as_bool()
            };
            if !installed {
                crate::logger::log("native_context_menu: SetWindowSubclass failed");
            }
            Some(Self { hwnd, installed })
        }
    }

    impl Drop for MenuSubclassGuard {
        fn drop(&mut self) {
            if self.installed {
                let _ = unsafe {
                    RemoveWindowSubclass(self.hwnd, Some(context_menu_subclass_proc), SUBCLASS_ID)
                };
            }
        }
    }

    unsafe extern "system" fn context_menu_subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        ref_data: usize,
    ) -> LRESULT {
        if ref_data != 0 {
            let forwarder = unsafe { &*(ref_data as *const ContextMenuMessageForwarder) };
            if let Some(result) = unsafe { forwarder.forward(msg, wparam, lparam) } {
                return result;
            }
        }
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn miv_command_ids_roundtrip_inside_custom_range() {
        assert_eq!(miv_command_id(0), Some(MIV_ID_FIRST));
        assert_eq!(miv_command_index(MIV_ID_FIRST, 1), Some(0));
        assert_eq!(miv_command_index(MIV_ID_FIRST + 2, 2), None);
    }

    #[test]
    fn shell_verb_offset_is_relative_to_shell_first_id() {
        assert_eq!(shell_verb_offset(SHELL_ID_FIRST), Some(0));
        assert_eq!(shell_verb_offset(SHELL_ID_FIRST + 12), Some(12));
        assert_eq!(shell_verb_offset(0), None);
    }

    #[test]
    fn shell_clipboard_verbs_use_canonical_shell_names() {
        assert_eq!(ShellClipboardVerb::Copy.canonical_name(), "copy");
        assert_eq!(ShellClipboardVerb::Cut.canonical_name(), "cut");
        assert_eq!(ShellClipboardVerb::Paste.canonical_name(), "paste");
    }
}
