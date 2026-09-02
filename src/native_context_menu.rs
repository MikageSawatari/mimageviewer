//! Native Windows Shell context menu bridge.
//!
//! The module owns only Win32/Shell menu plumbing. mIV-specific side effects are
//! returned as command IDs and executed by the caller on the normal App path.

use std::path::PathBuf;

use crate::context_menu_model::{MenuCommand, MenuNode};

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

const DROP_EFFECT_COPY_VALUE: u32 = 1;
const DROP_EFFECT_MOVE_VALUE: u32 = 2;

/// ファイル選択を OLE clipboard へ載せるときの `Preferred DropEffect`。
/// Paste はフォルダ背景の canonical verb 経路なので、この経路の対象外。
fn preferred_drop_effect_for_file_verb(verb: ShellClipboardVerb) -> Option<u32> {
    match verb {
        ShellClipboardVerb::Copy => Some(DROP_EFFECT_COPY_VALUE),
        ShellClipboardVerb::Cut => Some(DROP_EFFECT_MOVE_VALUE),
        ShellClipboardVerb::Paste => None,
    }
}

#[derive(Debug, Clone)]
pub struct NativeContextMenuRequest {
    pub hwnd: isize,
    pub screen_pos: (i32, i32),
    pub background_folder: Option<PathBuf>,
    pub paths: Vec<PathBuf>,
    pub miv_items: Vec<MenuNode>,
    pub shell_menu_placement: ShellMenuPlacement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellMenuPlacement {
    Submenu,
    Inline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeContextMenuResult {
    Canceled,
    MivCommand(MenuCommand),
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

fn leaf_commands(nodes: &[MenuNode]) -> Vec<&MenuCommand> {
    fn visit<'a>(nodes: &'a [MenuNode], out: &mut Vec<&'a MenuCommand>) {
        for node in nodes {
            match node {
                MenuNode::Item { command, .. } => out.push(command),
                MenuNode::Submenu { children, .. } => visit(children, out),
                MenuNode::Separator => {}
            }
        }
    }
    let mut commands = Vec::new();
    visit(nodes, &mut commands);
    commands
}

fn shell_menu_insert_position(placement: ShellMenuPlacement, nodes: &[MenuNode]) -> Option<u32> {
    match placement {
        ShellMenuPlacement::Submenu => Some(0),
        ShellMenuPlacement::Inline => {
            let top_level_positions = u32::try_from(nodes.len()).ok()?;
            top_level_positions.checked_add(u32::from(!nodes.is_empty()))
        }
    }
}

fn shell_verb_offset(command_id: u32) -> Option<u32> {
    command_id.checked_sub(SHELL_ID_FIRST)
}

fn can_keep_miv_menu_after_shell_failure(miv_count: usize) -> bool {
    miv_count > 0
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
    use std::cell::{Cell, RefCell};
    use std::ffi::CString;
    use std::mem::ManuallyDrop;
    use std::os::windows::ffi::OsStrExt;
    use std::time::Instant;

    use serde_json::Value;
    use windows::Win32::Foundation::{
        DV_E_DVASPECT, DV_E_LINDEX, DV_E_TYMED, E_INVALIDARG, E_OUTOFMEMORY, HANDLE, HWND, LPARAM,
        LRESULT, POINT, RPC_E_CHANGED_MODE, S_OK, WPARAM,
    };
    use windows::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize, DATADIR_GET,
        DVASPECT_CONTENT, FORMATETC, IAdviseSink, IBindCtx, IDataObject, IDataObject_Impl,
        IEnumFORMATETC, IEnumSTATDATA, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
    };
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    use windows::Win32::System::Ole::{OleSetClipboard, ReleaseStgMedium};
    use windows::Win32::UI::Controls::{
        ICC_BAR_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, TOOLTIPS_CLASSW, TTF_ABSOLUTE,
        TTF_TRACK, TTM_ADDTOOLW, TTM_DELTOOLW, TTM_SETMAXTIPWIDTH, TTM_TRACKACTIVATE,
        TTM_TRACKPOSITION, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
    };
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_SFUIObject, CFSTR_PREFERREDDROPEFFECT, CMF_EXPLORE, CMF_NORMAL, CMINVOKECOMMANDINFO,
        DefSubclassProc, IContextMenu, IContextMenu2, IContextMenu3, IShellFolder,
        RemoveWindowSubclass, SHCreateShellItemArrayFromIDLists, SHCreateStdEnumFmtEtc,
        SHGetDesktopFolder, SHParseDisplayName, SetWindowSubclass,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CW_USEDEFAULT, CreatePopupMenu, CreateWindowExW, DeleteMenu, DestroyMenu,
        DestroyWindow, GetCursorPos, GetForegroundWindow, HMENU, HWND_TOPMOST, MF_BYPOSITION,
        MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SendMessageW, SetWindowPos, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
        WINDOW_STYLE, WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR, WM_MENUSELECT,
        WS_EX_TOPMOST, WS_POPUP,
    };
    use windows::core::{HRESULT, Interface, PCSTR, PCWSTR, PWSTR, Ref};

    const SUBCLASS_ID: usize = 0x6d69_7643; // "mivC"
    const SLOW_NATIVE_MENU_STAGE_LOG_MS: f64 = 120.0;

    fn elapsed_ms(t0: Instant) -> f64 {
        t0.elapsed().as_secs_f64() * 1000.0
    }

    fn request_target_kind(request: &NativeContextMenuRequest) -> &'static str {
        if request.background_folder.is_some() {
            "background"
        } else if !request.paths.is_empty() {
            "paths"
        } else {
            "miv_only"
        }
    }

    fn emit_native_menu_timing(
        kind: &'static str,
        key: Option<&str>,
        ms: f64,
        path_count: usize,
        miv_count: usize,
        extras: &[(&'static str, Value)],
    ) {
        if !crate::perf::is_enabled() {
            return;
        }
        let mut fields = Vec::with_capacity(3 + extras.len());
        fields.push(("ms", Value::from(ms)));
        fields.push(("path_count", Value::from(path_count as u64)));
        fields.push(("miv_count", Value::from(miv_count as u64)));
        for (name, value) in extras {
            fields.push((*name, value.clone()));
        }
        crate::perf::event("native_menu", kind, key, 0, &fields);
    }

    fn log_slow_native_menu_stage(
        kind: &str,
        key: &str,
        ms: f64,
        path_count: usize,
        miv_count: usize,
    ) {
        if ms >= SLOW_NATIVE_MENU_STAGE_LOG_MS {
            crate::logger::log(format!(
                "native_context_menu: slow {kind} {ms:.1}ms target={key} paths={path_count} miv_items={miv_count}"
            ));
        }
    }

    fn shell_context_menu_for_paths_timed(
        paths: &[PathBuf],
        key: &'static str,
        path_count: usize,
        miv_count: usize,
    ) -> Result<IContextMenu, String> {
        let t0 = Instant::now();
        let result = shell_context_menu_for_paths(paths);
        let ms = elapsed_ms(t0);
        emit_native_menu_timing(
            "show_shell_bind",
            Some(key),
            ms,
            path_count,
            miv_count,
            &[("success", Value::from(result.is_ok()))],
        );
        log_slow_native_menu_stage("show_shell_bind", key, ms, path_count, miv_count);
        result
    }

    fn shell_context_menu_for_folder_background_timed(
        hwnd: HWND,
        folder: &std::path::Path,
        key: &'static str,
        path_count: usize,
        miv_count: usize,
    ) -> Result<IContextMenu, String> {
        let t0 = Instant::now();
        let result = shell_context_menu_for_folder_background(hwnd, folder);
        let ms = elapsed_ms(t0);
        emit_native_menu_timing(
            "show_shell_bind",
            Some(key),
            ms,
            path_count,
            miv_count,
            &[("success", Value::from(result.is_ok()))],
        );
        log_slow_native_menu_stage("show_shell_bind", key, ms, path_count, miv_count);
        result
    }

    fn query_shell_context_menu_timed(
        shell_menu: &IContextMenu,
        menu: HMENU,
        insert_at: u32,
        key: &'static str,
        path_count: usize,
        miv_count: usize,
    ) -> Result<(), String> {
        let t0 = Instant::now();
        let result = query_shell_context_menu(shell_menu, menu, insert_at);
        let ms = elapsed_ms(t0);
        emit_native_menu_timing(
            "show_query_shell",
            Some(key),
            ms,
            path_count,
            miv_count,
            &[("success", Value::from(result.is_ok()))],
        );
        log_slow_native_menu_stage("show_query_shell", key, ms, path_count, miv_count);
        result
    }

    fn emit_shell_verb_timing(
        kind: &'static str,
        target: &'static str,
        verb: ShellClipboardVerb,
        ms: f64,
        path_count: usize,
        success: bool,
    ) {
        emit_native_menu_timing(
            kind,
            Some(verb.canonical_name()),
            ms,
            path_count,
            0,
            &[
                ("target", Value::from(target)),
                ("success", Value::from(success)),
            ],
        );
        if ms >= SLOW_NATIVE_MENU_STAGE_LOG_MS {
            crate::logger::log(format!(
                "native_context_menu: slow {kind} {ms:.1}ms target={target} verb={} paths={path_count}",
                verb.canonical_name()
            ));
        }
    }

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

        let total_t0 = Instant::now();
        let target_kind = request_target_kind(&request);
        let path_count = if request.background_folder.is_some() {
            1
        } else {
            request.paths.len()
        };
        let miv_count = leaf_commands(&request.miv_items).len();

        let stage_t0 = Instant::now();
        let _com = match ComStaGuard::new() {
            Ok(guard) => guard,
            Err(reason) => return NativeContextMenuResult::Fallback { reason },
        };
        emit_native_menu_timing(
            "show_com_init",
            Some(target_kind),
            elapsed_ms(stage_t0),
            path_count,
            miv_count,
            &[],
        );

        let stage_t0 = Instant::now();
        let menu = match unsafe { CreatePopupMenu() } {
            Ok(menu) => MenuGuard::new(menu),
            Err(e) => {
                return NativeContextMenuResult::Fallback {
                    reason: format!("CreatePopupMenu failed: {e}"),
                };
            }
        };
        emit_native_menu_timing(
            "show_create_popup",
            Some(target_kind),
            elapsed_ms(stage_t0),
            path_count,
            miv_count,
            &[],
        );

        let stage_t0 = Instant::now();
        let mut leaf_index = 0;
        let mut disabled_reasons = std::collections::HashMap::new();
        if let Err(reason) = append_miv_menu_nodes(
            menu.handle(),
            &request.miv_items,
            &mut leaf_index,
            &mut disabled_reasons,
        ) {
            return NativeContextMenuResult::Fallback { reason };
        }
        emit_native_menu_timing(
            "show_append_miv",
            Some(target_kind),
            elapsed_ms(stage_t0),
            path_count,
            miv_count,
            &[],
        );

        let hwnd = HWND(request.hwnd as *mut core::ffi::c_void);
        let shell_menu = if let Some(folder) = request.background_folder.as_ref() {
            match shell_context_menu_for_folder_background_timed(
                hwnd,
                folder,
                target_kind,
                path_count,
                miv_count,
            ) {
                Ok(menu) => Some(menu),
                Err(reason) if can_keep_miv_menu_after_shell_failure(miv_count) => {
                    crate::logger::log(format!(
                        "native_context_menu: Windows menu unavailable; showing mIV-only menu: {reason}"
                    ));
                    None
                }
                Err(reason) => return NativeContextMenuResult::Fallback { reason },
            }
        } else if request.paths.is_empty() {
            None
        } else {
            match shell_context_menu_for_paths_timed(
                &request.paths,
                target_kind,
                path_count,
                miv_count,
            ) {
                Ok(menu) => Some(menu),
                Err(reason) if can_keep_miv_menu_after_shell_failure(miv_count) => {
                    crate::logger::log(format!(
                        "native_context_menu: Windows menu unavailable; showing mIV-only menu: {reason}"
                    ));
                    None
                }
                Err(reason) => return NativeContextMenuResult::Fallback { reason },
            }
        };

        let mut lazy_shell_submenu = None;
        if let Some(shell_menu) = shell_menu.as_ref() {
            if let Err(reason) = append_root_separator(menu.handle(), !request.miv_items.is_empty())
            {
                return NativeContextMenuResult::Fallback { reason };
            }
            match request.shell_menu_placement {
                ShellMenuPlacement::Inline => {
                    let Some(insert_at) =
                        shell_menu_insert_position(ShellMenuPlacement::Inline, &request.miv_items)
                    else {
                        return NativeContextMenuResult::Fallback {
                            reason: "too many mIV context menu positions".to_string(),
                        };
                    };
                    if let Err(reason) = query_shell_context_menu_timed(
                        shell_menu,
                        menu.handle(),
                        insert_at,
                        target_kind,
                        path_count,
                        miv_count,
                    ) {
                        return NativeContextMenuResult::Fallback { reason };
                    }
                }
                ShellMenuPlacement::Submenu => {
                    let submenu = match append_lazy_shell_submenu(menu.handle()) {
                        Ok(submenu) => submenu,
                        Err(reason) => return NativeContextMenuResult::Fallback { reason },
                    };
                    lazy_shell_submenu = Some(LazyShellSubmenu::new(
                        submenu,
                        target_kind,
                        path_count,
                        miv_count,
                    ));
                }
            }
        }

        let stage_t0 = Instant::now();
        let shell_forwarder = shell_menu
            .as_ref()
            .map(|menu| ContextMenuMessageForwarder::new(menu, lazy_shell_submenu));
        let tooltip = if disabled_reasons.is_empty() {
            None
        } else {
            match TrackedMenuTooltip::new(hwnd, disabled_reasons) {
                Ok(tooltip) => Some(RefCell::new(tooltip)),
                Err(error) => {
                    crate::logger::log(format!(
                        "native_context_menu: disabled reason tooltip unavailable: {error}"
                    ));
                    None
                }
            }
        };
        let message_state = MenuMessageState {
            shell: shell_forwarder,
            tooltip,
        };
        let _subclass_guard = match MenuSubclassGuard::install(hwnd, &message_state) {
            Ok(guard) => guard,
            Err(reason) => return NativeContextMenuResult::Fallback { reason },
        };
        emit_native_menu_timing(
            "show_subclass",
            Some(target_kind),
            elapsed_ms(stage_t0),
            path_count,
            miv_count,
            &[],
        );

        let (screen_x, screen_y) = cursor_screen_pos().unwrap_or(request.screen_pos);
        let pre_track_ms = elapsed_ms(total_t0);
        emit_native_menu_timing(
            "show_pre_track",
            Some(target_kind),
            pre_track_ms,
            path_count,
            miv_count,
            &[],
        );
        log_slow_native_menu_stage(
            "show_pre_track",
            target_kind,
            pre_track_ms,
            path_count,
            miv_count,
        );
        let track_t0 = Instant::now();
        // 診断: どの窓をオーナーにしてメニューを出し、その前後で前面窓が変わるか。
        // 「F12 の別窓で右クリックするとメインが前に出る」の機構を推測でなく実測で決めるため
        // (backlog §1.162)。原因が確定したら消す。
        let foreground_before = unsafe { GetForegroundWindow().0 as usize as u64 };
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
        {
            let foreground_after = unsafe { GetForegroundWindow().0 as usize as u64 };
            let owner = hwnd.0 as usize as u64;
            crate::logger::log(format!(
                "native_context_menu: owner=0x{owner:x} fg_before=0x{foreground_before:x} fg_after=0x{foreground_after:x} owner_was_fg={} fg_changed={} target={target_kind}",
                foreground_before == owner,
                foreground_before != foreground_after,
            ));
        }
        message_state.hide_tooltip();
        emit_native_menu_timing(
            "show_track_popup_block",
            Some(target_kind),
            elapsed_ms(track_t0),
            path_count,
            miv_count,
            &[("selected_id", Value::from(selected as u64))],
        );
        if selected == 0 {
            if let Some(reason) = message_state
                .shell
                .as_ref()
                .and_then(ContextMenuMessageForwarder::lazy_query_error)
            {
                return NativeContextMenuResult::Fallback { reason };
            }
            return NativeContextMenuResult::Canceled;
        }

        let commands = leaf_commands(&request.miv_items);
        if let Some(index) = miv_command_index(selected, commands.len()) {
            return NativeContextMenuResult::MivCommand(commands[index].clone());
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
        let stage_t0 = Instant::now();
        match unsafe { shell_menu.InvokeCommand(&invoke) } {
            Ok(()) => {
                let ms = elapsed_ms(stage_t0);
                emit_native_menu_timing(
                    "show_invoke_shell",
                    Some(target_kind),
                    ms,
                    path_count,
                    miv_count,
                    &[("success", Value::from(true))],
                );
                log_slow_native_menu_stage(
                    "show_invoke_shell",
                    target_kind,
                    ms,
                    path_count,
                    miv_count,
                );
                NativeContextMenuResult::ShellCommandInvoked
            }
            Err(e) => {
                let ms = elapsed_ms(stage_t0);
                emit_native_menu_timing(
                    "show_invoke_shell",
                    Some(target_kind),
                    ms,
                    path_count,
                    miv_count,
                    &[("success", Value::from(false))],
                );
                log_slow_native_menu_stage(
                    "show_invoke_shell",
                    target_kind,
                    ms,
                    path_count,
                    miv_count,
                );
                NativeContextMenuResult::Fallback {
                    reason: format!("IContextMenu::InvokeCommand failed: {e}"),
                }
            }
        }
    }

    pub(super) fn invoke_shell_file_verb(
        _hwnd: isize,
        paths: &[PathBuf],
        verb: ShellClipboardVerb,
    ) -> Result<(), String> {
        if paths.is_empty() {
            return Ok(());
        }
        let preferred_drop_effect = preferred_drop_effect_for_file_verb(verb)
            .ok_or_else(|| "file clipboard path accepts only Copy or Cut".to_string())?;
        let _com = ComStaGuard::new()?;
        let stage_t0 = Instant::now();
        let data_result = crate::file_drag::shell_data_object_for_paths(paths).map_err(
            |(failed_paths, error)| {
                format!("could not build Shell IDataObject: {error:?}; failed_paths={failed_paths}")
            },
        );
        let ms = elapsed_ms(stage_t0);
        emit_shell_verb_timing(
            "verb_data_object_bind",
            "paths",
            verb,
            ms,
            paths.len(),
            data_result.is_ok(),
        );
        let (data, failed_paths) = data_result?;
        if failed_paths != 0 {
            return Err(format!(
                "SHParseDisplayName failed for {failed_paths} of {} clipboard paths",
                paths.len()
            ));
        }

        let stage_t0 = Instant::now();
        let effect_result = data_object_with_preferred_drop_effect(data, preferred_drop_effect);
        let ms = elapsed_ms(stage_t0);
        emit_shell_verb_timing(
            "verb_set_preferred_drop_effect",
            "paths",
            verb,
            ms,
            paths.len(),
            effect_result.is_ok(),
        );
        let clipboard_data = effect_result?;

        let stage_t0 = Instant::now();
        let result = unsafe { OleSetClipboard(&clipboard_data) }
            .map_err(|error| format!("OleSetClipboard failed: {error}"));
        let ms = elapsed_ms(stage_t0);
        emit_shell_verb_timing(
            "verb_set_clipboard",
            "paths",
            verb,
            ms,
            paths.len(),
            result.is_ok(),
        );
        result
    }

    fn data_object_with_preferred_drop_effect(
        shell_data: IDataObject,
        effect: u32,
    ) -> Result<IDataObject, String> {
        let format_id = unsafe { RegisterClipboardFormatW(CFSTR_PREFERREDDROPEFFECT) };
        if format_id == 0 {
            return Err("RegisterClipboardFormatW(Preferred DropEffect) failed".to_string());
        }
        let cf_format = u16::try_from(format_id)
            .map_err(|_| format!("Preferred DropEffect format ID is out of range: {format_id}"))?;
        Ok(ClipboardDataObject {
            inner: shell_data,
            preferred_format: cf_format,
            preferred_effect: effect,
        }
        .into())
    }

    fn preferred_drop_effect_format(cf_format: u16) -> FORMATETC {
        FORMATETC {
            cfFormat: cf_format,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    fn validate_preferred_drop_effect_request(
        format: &FORMATETC,
    ) -> std::result::Result<(), HRESULT> {
        if format.dwAspect != DVASPECT_CONTENT.0 {
            return Err(DV_E_DVASPECT);
        }
        if format.lindex != -1 {
            return Err(DV_E_LINDEX);
        }
        if format.tymed & TYMED_HGLOBAL.0 as u32 == 0 {
            return Err(DV_E_TYMED);
        }
        Ok(())
    }

    fn preferred_drop_effect_medium(effect: u32) -> windows::core::Result<STGMEDIUM> {
        let hglobal = unsafe { GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of::<u32>()) }
            .map_err(|_| windows::core::Error::from_hresult(E_OUTOFMEMORY))?;
        let mut medium = STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: hglobal },
            pUnkForRelease: ManuallyDrop::new(None),
        };
        let locked = unsafe { GlobalLock(hglobal) };
        if locked.is_null() {
            unsafe { ReleaseStgMedium(&mut medium) };
            return Err(windows::core::Error::from_hresult(E_OUTOFMEMORY));
        }
        unsafe { locked.cast::<u32>().write(effect) };
        // GlobalUnlock はロック数が 0 になった成功時にも false を返す。
        let _ = unsafe { GlobalUnlock(hglobal) };
        Ok(medium)
    }

    /// BHID_DataObject のファイル形式をそのまま委譲し、Copy/Cut の意図だけを追加する。
    /// Shell の IDataObject は SetData が E_NOTIMPL のため、既知の追加形式を所有する外側の
    /// IDataObject が必要になる。
    #[windows::core::implement(IDataObject)]
    struct ClipboardDataObject {
        inner: IDataObject,
        preferred_format: u16,
        preferred_effect: u32,
    }

    impl IDataObject_Impl for ClipboardDataObject_Impl {
        fn GetData(&self, format: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
            let Some(format_ref) = (unsafe { format.as_ref() }) else {
                return Err(windows::core::Error::from_hresult(E_INVALIDARG));
            };
            if format_ref.cfFormat != self.preferred_format {
                return unsafe { self.inner.GetData(format) };
            }
            validate_preferred_drop_effect_request(format_ref)
                .map_err(windows::core::Error::from_hresult)?;
            preferred_drop_effect_medium(self.preferred_effect)
        }

        fn GetDataHere(
            &self,
            format: *const FORMATETC,
            medium: *mut STGMEDIUM,
        ) -> windows::core::Result<()> {
            unsafe { self.inner.GetDataHere(format, medium) }
        }

        fn QueryGetData(&self, format: *const FORMATETC) -> HRESULT {
            let Some(format_ref) = (unsafe { format.as_ref() }) else {
                return E_INVALIDARG;
            };
            if format_ref.cfFormat != self.preferred_format {
                return unsafe { self.inner.QueryGetData(format) };
            }
            validate_preferred_drop_effect_request(format_ref).map_or_else(|error| error, |_| S_OK)
        }

        fn GetCanonicalFormatEtc(
            &self,
            format_in: *const FORMATETC,
            format_out: *mut FORMATETC,
        ) -> HRESULT {
            unsafe { self.inner.GetCanonicalFormatEtc(format_in, format_out) }
        }

        fn SetData(
            &self,
            format: *const FORMATETC,
            medium: *const STGMEDIUM,
            release: windows::core::BOOL,
        ) -> windows::core::Result<()> {
            unsafe { self.inner.SetData(format, medium, release.as_bool()) }
        }

        fn EnumFormatEtc(&self, direction: u32) -> windows::core::Result<IEnumFORMATETC> {
            let inner_enum = unsafe { self.inner.EnumFormatEtc(direction) }?;
            if direction != DATADIR_GET.0 as u32 {
                return Ok(inner_enum);
            }

            let mut formats: Vec<FORMATETC> = Vec::new();
            loop {
                let mut format = FORMATETC::default();
                let mut fetched = 0;
                let status = unsafe {
                    inner_enum.Next(std::slice::from_mut(&mut format), Some(&mut fetched))
                };
                if status.is_err() {
                    for existing in &formats {
                        if !existing.ptd.is_null() {
                            unsafe { CoTaskMemFree(Some(existing.ptd.cast())) };
                        }
                    }
                    return Err(windows::core::Error::from_hresult(status));
                }
                if fetched == 0 {
                    break;
                }
                formats.push(format);
                if status != S_OK {
                    break;
                }
            }
            if !formats
                .iter()
                .any(|format| format.cfFormat == self.preferred_format)
            {
                formats.push(preferred_drop_effect_format(self.preferred_format));
            }
            let result = unsafe { SHCreateStdEnumFmtEtc(&formats) };
            for format in &formats {
                if !format.ptd.is_null() {
                    unsafe { CoTaskMemFree(Some(format.ptd.cast())) };
                }
            }
            result
        }

        fn DAdvise(
            &self,
            format: *const FORMATETC,
            flags: u32,
            sink: Ref<IAdviseSink>,
        ) -> windows::core::Result<u32> {
            unsafe { self.inner.DAdvise(format, flags, sink.as_ref()) }
        }

        fn DUnadvise(&self, connection: u32) -> windows::core::Result<()> {
            unsafe { self.inner.DUnadvise(connection) }
        }

        fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
            unsafe { self.inner.EnumDAdvise() }
        }
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
        let stage_t0 = Instant::now();
        let menu_result = shell_context_menu_for_folder_background(hwnd, folder);
        let ms = elapsed_ms(stage_t0);
        emit_shell_verb_timing(
            "verb_shell_bind",
            "background",
            verb,
            ms,
            1,
            menu_result.is_ok(),
        );
        let menu = menu_result?;
        let popup = MenuGuard::new(
            unsafe { CreatePopupMenu() }.map_err(|e| format!("CreatePopupMenu failed: {e}"))?,
        );
        let stage_t0 = Instant::now();
        let query_result = query_shell_context_menu(&menu, popup.handle(), 0);
        let ms = elapsed_ms(stage_t0);
        emit_shell_verb_timing(
            "verb_query_shell",
            "background",
            verb,
            ms,
            1,
            query_result.is_ok(),
        );
        query_result?;
        let stage_t0 = Instant::now();
        let result = invoke_canonical_verb(&menu, hwnd, verb);
        let ms = elapsed_ms(stage_t0);
        emit_shell_verb_timing("verb_invoke", "background", verb, ms, 1, result.is_ok());
        result
    }

    fn append_root_separator(menu: HMENU, needed: bool) -> Result<(), String> {
        if !needed {
            return Ok(());
        }
        unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) }
            .map_err(|error| format!("AppendMenuW(separator) failed: {error}"))
    }

    fn append_lazy_shell_submenu(menu: HMENU) -> Result<HMENU, String> {
        let mut submenu = MenuGuard::new(
            unsafe { CreatePopupMenu() }
                .map_err(|error| format!("CreatePopupMenu(Windows submenu) failed: {error}"))?,
        );
        append_menu_string(submenu.handle(), 0, "読み込み中…", false)
            .map_err(|error| format!("AppendMenuW(Windows placeholder) failed: {error}"))?;
        let label = wide_null("Windows のメニュー");
        unsafe {
            AppendMenuW(
                menu,
                MF_POPUP | MF_STRING,
                submenu.handle().0 as usize,
                PCWSTR(label.as_ptr()),
            )
        }
        .map_err(|error| format!("AppendMenuW(Windows submenu) failed: {error}"))?;
        let handle = submenu.handle();
        // root menu が子 HMENU を再帰所有する。
        submenu.release();
        Ok(handle)
    }

    fn append_miv_menu_nodes(
        menu: HMENU,
        nodes: &[MenuNode],
        leaf_index: &mut usize,
        disabled_reasons: &mut std::collections::HashMap<u32, String>,
    ) -> Result<(), String> {
        for node in nodes {
            match node {
                MenuNode::Separator => {
                    unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) }
                        .map_err(|error| format!("AppendMenuW(mIV separator) failed: {error}"))?;
                }
                MenuNode::Item {
                    label,
                    enabled,
                    disabled_reason,
                    ..
                } => {
                    let id = miv_command_id(*leaf_index)
                        .ok_or_else(|| "too many mIV context menu commands".to_string())?;
                    append_menu_string(menu, id, label, *enabled)
                        .map_err(|error| format!("AppendMenuW(mIV) failed: {error}"))?;
                    record_disabled_reason(
                        disabled_reasons,
                        id,
                        *enabled,
                        disabled_reason.as_deref(),
                    );
                    *leaf_index += 1;
                }
                MenuNode::Submenu { label, children } => {
                    let mut child =
                        MenuGuard::new(unsafe { CreatePopupMenu() }.map_err(|error| {
                            format!("CreatePopupMenu(submenu) failed: {error}")
                        })?);
                    append_miv_menu_nodes(child.handle(), children, leaf_index, disabled_reasons)?;
                    let escaped = label.replace('&', "&&");
                    let wide = wide_null(&escaped);
                    unsafe {
                        AppendMenuW(
                            menu,
                            MF_POPUP | MF_STRING,
                            child.handle().0 as usize,
                            PCWSTR(wide.as_ptr()),
                        )
                    }
                    .map_err(|error| format!("AppendMenuW(mIV submenu) failed: {error}"))?;
                    // Once attached, the root menu owns the child recursively.
                    child.release();
                }
            }
        }
        Ok(())
    }

    fn record_disabled_reason(
        reasons: &mut std::collections::HashMap<u32, String>,
        command_id: u32,
        enabled: bool,
        reason: Option<&str>,
    ) {
        if !enabled
            && let Some(reason) = reason
            && !reason.is_empty()
        {
            reasons.insert(command_id, reason.to_string());
        }
    }

    fn append_menu_string(
        menu: HMENU,
        id: u32,
        label: &str,
        enabled: bool,
    ) -> windows::core::Result<()> {
        let escaped = label.replace('&', "&&");
        let wide = wide_null(&escaped);
        let flags = if enabled {
            MF_STRING
        } else {
            MF_STRING | MF_GRAYED
        };
        unsafe { AppendMenuW(menu, flags, id as usize, PCWSTR(wide.as_ptr())) }
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
                CMF_NORMAL | CMF_EXPLORE,
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

        let desktop = match unsafe { SHGetDesktopFolder() } {
            Ok(desktop) => desktop,
            Err(e) => {
                unsafe { CoTaskMemFree(Some(pidl as *const core::ffi::c_void)) };
                return Err(format!("SHGetDesktopFolder failed: {e}"));
            }
        };
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

    struct MenuGuard(Option<HMENU>);

    impl MenuGuard {
        fn new(menu: HMENU) -> Self {
            Self(Some(menu))
        }

        fn handle(&self) -> HMENU {
            self.0.expect("menu guard must own a handle")
        }

        fn release(&mut self) {
            self.0 = None;
        }
    }

    impl Drop for MenuGuard {
        fn drop(&mut self) {
            if let Some(menu) = self.0.take() {
                let _ = unsafe { DestroyMenu(menu) };
            }
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

    struct LazyShellSubmenu {
        handle: HMENU,
        initialized: Cell<bool>,
        query_error: RefCell<Option<String>>,
        target_kind: &'static str,
        path_count: usize,
        miv_count: usize,
    }

    impl LazyShellSubmenu {
        fn new(
            handle: HMENU,
            target_kind: &'static str,
            path_count: usize,
            miv_count: usize,
        ) -> Self {
            Self {
                handle,
                initialized: Cell::new(false),
                query_error: RefCell::new(None),
                target_kind,
                path_count,
                miv_count,
            }
        }

        fn initialize(&self, shell_menu: &IContextMenu) {
            if self.initialized.replace(true) {
                return;
            }
            let result = (|| {
                unsafe { DeleteMenu(self.handle, 0, MF_BYPOSITION) }
                    .map_err(|error| format!("DeleteMenu(Windows placeholder) failed: {error}"))?;
                let insert_at = shell_menu_insert_position(ShellMenuPlacement::Submenu, &[])
                    .expect("submenu insertion position is always zero");
                query_shell_context_menu_timed(
                    shell_menu,
                    self.handle,
                    insert_at,
                    self.target_kind,
                    self.path_count,
                    self.miv_count,
                )
            })();
            if let Err(reason) = result {
                crate::logger::log(format!(
                    "native_context_menu: lazy Windows submenu failed: {reason}"
                ));
                let _ = append_menu_string(
                    self.handle,
                    0,
                    "Windows のメニューを読み込めませんでした",
                    false,
                );
                *self.query_error.borrow_mut() = Some(reason);
            }
        }
    }

    struct ContextMenuMessageForwarder {
        shell_menu: IContextMenu,
        menu2: Option<IContextMenu2>,
        menu3: Option<IContextMenu3>,
        lazy_shell_submenu: Option<LazyShellSubmenu>,
    }

    impl ContextMenuMessageForwarder {
        fn new(menu: &IContextMenu, lazy_shell_submenu: Option<LazyShellSubmenu>) -> Self {
            Self {
                shell_menu: menu.clone(),
                menu2: menu.cast::<IContextMenu2>().ok(),
                menu3: menu.cast::<IContextMenu3>().ok(),
                lazy_shell_submenu,
            }
        }

        fn needs_subclass(&self) -> bool {
            self.lazy_shell_submenu.is_some() || self.menu2.is_some() || self.menu3.is_some()
        }

        fn lazy_query_error(&self) -> Option<String> {
            self.lazy_shell_submenu
                .as_ref()
                .and_then(|submenu| submenu.query_error.borrow().clone())
        }

        fn shell_messages_are_ready(&self) -> bool {
            self.lazy_shell_submenu.as_ref().is_none_or(|submenu| {
                submenu.initialized.get() && submenu.query_error.borrow().is_none()
            })
        }

        unsafe fn forward(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
            if msg == WM_INITMENUPOPUP
                && let Some(submenu) = &self.lazy_shell_submenu
                && submenu.handle.0 as usize == wparam.0
            {
                submenu.initialize(&self.shell_menu);
            }
            // 遅延 submenu をまだ開いていない間は QueryContextMenu 前なので、Shell
            // extension へ root 側の menu message を渡さない。Query 後にだけ、Shell が
            // 作った owner-draw / nested submenu の message を従来どおり転送する。
            if !self.shell_messages_are_ready() {
                return None;
            }
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

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum MenuTooltipSelection {
        Hide,
        Show(u32),
    }

    fn menu_tooltip_selection(wparam: usize, lparam: isize) -> MenuTooltipSelection {
        let command_id = (wparam & 0xffff) as u32;
        let flags = ((wparam >> 16) & 0xffff) as u32;
        if (flags == 0xffff && lparam == 0)
            || flags & MF_POPUP.0 != 0
            || flags & MF_SEPARATOR.0 != 0
        {
            MenuTooltipSelection::Hide
        } else {
            MenuTooltipSelection::Show(command_id)
        }
    }

    fn pack_track_position(x: i32, y: i32) -> isize {
        let x = x.clamp(i16::MIN as i32, i16::MAX as i32) as i16 as u16 as u32;
        let y = y.clamp(i16::MIN as i32, i16::MAX as i32) as i16 as u16 as u32;
        (x | (y << 16)) as isize
    }

    struct TrackedMenuTooltip {
        owner: HWND,
        hwnd: HWND,
        reasons: std::collections::HashMap<u32, String>,
        text: Vec<u16>,
        active_id: Option<u32>,
    }

    impl TrackedMenuTooltip {
        fn new(
            owner: HWND,
            reasons: std::collections::HashMap<u32, String>,
        ) -> Result<Self, String> {
            let common_controls = INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_BAR_CLASSES,
            };
            if !unsafe { InitCommonControlsEx(&common_controls) }.as_bool() {
                return Err(format!(
                    "InitCommonControlsEx(tooltip) failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_TOPMOST,
                    TOOLTIPS_CLASSW,
                    PCWSTR::null(),
                    WINDOW_STYLE(WS_POPUP.0 | TTS_ALWAYSTIP | TTS_NOPREFIX),
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    Some(owner),
                    None,
                    None,
                    None,
                )
            }
            .map_err(|error| format!("CreateWindowExW(tooltip) failed: {error}"))?;
            let _ = unsafe {
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )
            };
            unsafe { SendMessageW(hwnd, TTM_SETMAXTIPWIDTH, Some(WPARAM(0)), Some(LPARAM(500))) };
            Ok(Self {
                owner,
                hwnd,
                reasons,
                text: Vec::new(),
                active_id: None,
            })
        }

        fn tool_info(&mut self) -> TTTOOLINFOW {
            TTTOOLINFOW {
                cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
                uFlags: TTF_TRACK | TTF_ABSOLUTE,
                hwnd: self.owner,
                uId: 1,
                lpszText: PWSTR(self.text.as_mut_ptr()),
                ..Default::default()
            }
        }

        fn handle_menu_select(&mut self, wparam: WPARAM, lparam: LPARAM) {
            match menu_tooltip_selection(wparam.0, lparam.0) {
                MenuTooltipSelection::Show(id) if self.reasons.contains_key(&id) => self.show(id),
                MenuTooltipSelection::Hide | MenuTooltipSelection::Show(_) => self.hide(),
            }
        }

        fn show(&mut self, id: u32) {
            if self.active_id == Some(id) {
                return;
            }
            self.hide();
            let Some(reason) = self.reasons.get(&id).cloned() else {
                return;
            };
            self.text = wide_null(&reason);
            let tool_info = self.tool_info();
            let added = unsafe {
                SendMessageW(
                    self.hwnd,
                    TTM_ADDTOOLW,
                    Some(WPARAM(0)),
                    Some(LPARAM((&tool_info as *const TTTOOLINFOW) as isize)),
                )
            };
            if added.0 == 0 {
                self.text.clear();
                return;
            }
            let mut cursor = POINT::default();
            let _ = unsafe { GetCursorPos(&mut cursor) };
            unsafe {
                SendMessageW(
                    self.hwnd,
                    TTM_TRACKPOSITION,
                    Some(WPARAM(0)),
                    Some(LPARAM(pack_track_position(cursor.x + 12, cursor.y + 20))),
                );
                SendMessageW(
                    self.hwnd,
                    TTM_TRACKACTIVATE,
                    Some(WPARAM(1)),
                    Some(LPARAM((&tool_info as *const TTTOOLINFOW) as isize)),
                );
            }
            self.active_id = Some(id);
        }

        fn hide(&mut self) {
            if self.active_id.is_none() {
                return;
            }
            let tool_info = self.tool_info();
            unsafe {
                SendMessageW(
                    self.hwnd,
                    TTM_TRACKACTIVATE,
                    Some(WPARAM(0)),
                    Some(LPARAM((&tool_info as *const TTTOOLINFOW) as isize)),
                );
                SendMessageW(
                    self.hwnd,
                    TTM_DELTOOLW,
                    Some(WPARAM(0)),
                    Some(LPARAM((&tool_info as *const TTTOOLINFOW) as isize)),
                );
            }
            self.active_id = None;
            self.text.clear();
        }
    }

    impl Drop for TrackedMenuTooltip {
        fn drop(&mut self) {
            self.hide();
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }

    struct MenuMessageState {
        shell: Option<ContextMenuMessageForwarder>,
        tooltip: Option<RefCell<TrackedMenuTooltip>>,
    }

    impl MenuMessageState {
        fn needs_subclass(&self) -> bool {
            self.tooltip.is_some()
                || self
                    .shell
                    .as_ref()
                    .is_some_and(ContextMenuMessageForwarder::needs_subclass)
        }

        fn hide_tooltip(&self) {
            if let Some(tooltip) = &self.tooltip {
                tooltip.borrow_mut().hide();
            }
        }

        unsafe fn forward(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
            if msg == WM_MENUSELECT
                && let Some(tooltip) = &self.tooltip
            {
                tooltip.borrow_mut().handle_menu_select(wparam, lparam);
            }
            self.shell
                .as_ref()
                .and_then(|shell| unsafe { shell.forward(msg, wparam, lparam) })
        }
    }

    struct MenuSubclassGuard {
        hwnd: HWND,
        installed: bool,
    }

    impl MenuSubclassGuard {
        fn install(hwnd: HWND, state: &MenuMessageState) -> Result<Option<Self>, String> {
            if !state.needs_subclass() {
                return Ok(None);
            }
            let installed = unsafe {
                SetWindowSubclass(
                    hwnd,
                    Some(context_menu_subclass_proc),
                    SUBCLASS_ID,
                    state as *const MenuMessageState as usize,
                )
                .as_bool()
            };
            if !installed {
                return Err("SetWindowSubclass failed".to_string());
            }
            Ok(Some(Self { hwnd, installed }))
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
            let state = unsafe { &*(ref_data as *const MenuMessageState) };
            if let Some(result) = unsafe { state.forward(msg, wparam, lparam) } {
                return result;
            }
        }
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    #[cfg(test)]
    mod data_object_smoke_test {
        use super::*;

        #[test]
        fn disabled_reason_map_records_only_disabled_nonempty_leaf_reasons() {
            let mut reasons = std::collections::HashMap::new();
            record_disabled_reason(&mut reasons, MIV_ID_FIRST, false, Some("reason"));
            record_disabled_reason(&mut reasons, MIV_ID_FIRST + 1, true, Some("enabled"));
            record_disabled_reason(&mut reasons, MIV_ID_FIRST + 2, false, None);
            record_disabled_reason(&mut reasons, MIV_ID_FIRST + 3, false, Some(""));
            assert_eq!(
                reasons,
                std::collections::HashMap::from([(MIV_ID_FIRST, "reason".to_string())])
            );
        }

        #[test]
        fn menu_selection_shows_only_plain_command_ids_and_hides_menu_metadata() {
            let pack = |id: u32, flags: u32| (id as usize) | ((flags as usize) << 16);
            assert_eq!(
                menu_tooltip_selection(pack(MIV_ID_FIRST, 0), 1),
                MenuTooltipSelection::Show(MIV_ID_FIRST)
            );
            assert_eq!(
                menu_tooltip_selection(pack(2, MF_POPUP.0), 1),
                MenuTooltipSelection::Hide
            );
            assert_eq!(
                menu_tooltip_selection(pack(0, MF_SEPARATOR.0), 1),
                MenuTooltipSelection::Hide
            );
            assert_eq!(
                menu_tooltip_selection(0xffff_0000, 0),
                MenuTooltipSelection::Hide
            );
        }

        #[test]
        fn tooltip_track_position_preserves_signed_screen_coordinates() {
            let packed = pack_track_position(-12, 345);
            assert_eq!((packed as u32 & 0xffff) as u16 as i16, -12);
            assert_eq!(((packed as u32 >> 16) & 0xffff) as u16 as i16, 345);
        }

        #[test]
        fn cross_folder_clipboard_data_object_exposes_preferred_move_effect() {
            let _com = ComStaGuard::new().expect("COM STA");
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let paths = [root.join("Cargo.toml"), root.join("src/lib.rs")];
            let (data, failed_paths) = crate::file_drag::shell_data_object_for_paths(&paths)
                .expect("cross-folder Shell IDataObject");
            assert_eq!(failed_paths, 0);
            let data = data_object_with_preferred_drop_effect(data, DROP_EFFECT_MOVE_VALUE)
                .expect("Preferred DropEffect");

            let file_drop_format = FORMATETC {
                cfFormat: windows::Win32::System::Ole::CF_HDROP.0,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as u32,
            };
            assert_eq!(unsafe { data.QueryGetData(&file_drop_format) }, S_OK);

            let format_id = unsafe { RegisterClipboardFormatW(CFSTR_PREFERREDDROPEFFECT) };
            let format = preferred_drop_effect_format(format_id as u16);
            assert_eq!(unsafe { data.QueryGetData(&format) }, S_OK);
            let mut medium = unsafe { data.GetData(&format) }.expect("Preferred DropEffect data");
            let hglobal = unsafe { medium.u.hGlobal };
            let locked = unsafe { GlobalLock(hglobal) };
            assert!(!locked.is_null());
            assert_eq!(
                unsafe { locked.cast::<u32>().read() },
                DROP_EFFECT_MOVE_VALUE
            );
            let _ = unsafe { GlobalUnlock(hglobal) };
            unsafe { ReleaseStgMedium(&mut medium) };
        }
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
    fn inline_shell_insert_position_counts_only_top_level_nodes() {
        let item = |command, label: &str| MenuNode::Item {
            command,
            label: label.to_string(),
            enabled: true,
            disabled_reason: None,
        };
        let nodes = vec![
            item(MenuCommand::CopyPath, "copy"),
            MenuNode::Separator,
            MenuNode::Submenu {
                label: "apps".to_string(),
                children: vec![
                    item(
                        MenuCommand::ExternalTool(crate::external_tool::ExternalToolId(7)),
                        "tool",
                    ),
                    MenuNode::Separator,
                    item(MenuCommand::OpenExternalToolSettings, "settings"),
                ],
            },
        ];

        assert_eq!(
            shell_menu_insert_position(ShellMenuPlacement::Inline, &nodes),
            Some(4)
        );
        assert_eq!(
            shell_menu_insert_position(ShellMenuPlacement::Inline, &[]),
            Some(0)
        );
    }

    #[test]
    fn shell_submenu_queries_from_its_own_first_position() {
        let nodes = vec![
            MenuNode::Separator,
            MenuNode::Submenu {
                label: "mIV submenu".to_string(),
                children: Vec::new(),
            },
        ];
        assert_eq!(
            shell_menu_insert_position(ShellMenuPlacement::Submenu, &nodes),
            Some(0)
        );
    }

    #[test]
    fn leaf_command_order_matches_preorder_id_assignment() {
        let leaf = |command, label: &str| MenuNode::Item {
            command,
            label: label.to_string(),
            enabled: true,
            disabled_reason: None,
        };
        let nodes = vec![
            leaf(MenuCommand::CopyPath, "copy"),
            MenuNode::Submenu {
                label: "apps".to_string(),
                children: vec![
                    leaf(
                        MenuCommand::OpenWithAssociation {
                            display_name: "Viewer".to_string(),
                            handler_id: "viewer.id".to_string(),
                        },
                        "Viewer",
                    ),
                    leaf(MenuCommand::OpenExternalToolSettings, "settings"),
                ],
            },
            leaf(MenuCommand::Deselect, "deselect"),
        ];
        let commands = leaf_commands(&nodes);
        assert_eq!(commands.len(), 4);
        for (index, expected) in commands.iter().enumerate() {
            let id = miv_command_id(index).unwrap();
            let reverse = miv_command_index(id, commands.len()).unwrap();
            assert_eq!(commands[reverse], *expected);
        }
        assert!(matches!(commands[0], MenuCommand::CopyPath));
        assert!(matches!(
            commands[1],
            MenuCommand::OpenWithAssociation { .. }
        ));
        assert!(matches!(commands[2], MenuCommand::OpenExternalToolSettings));
        assert!(matches!(commands[3], MenuCommand::Deselect));
    }

    #[test]
    fn shell_acquisition_failure_keeps_miv_items_available() {
        assert!(can_keep_miv_menu_after_shell_failure(1));
        assert!(can_keep_miv_menu_after_shell_failure(12));
        assert!(!can_keep_miv_menu_after_shell_failure(0));
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

    #[test]
    fn file_clipboard_verbs_select_copy_and_move_drop_effects() {
        assert_eq!(
            preferred_drop_effect_for_file_verb(ShellClipboardVerb::Copy),
            Some(DROP_EFFECT_COPY_VALUE)
        );
        assert_eq!(
            preferred_drop_effect_for_file_verb(ShellClipboardVerb::Cut),
            Some(DROP_EFFECT_MOVE_VALUE)
        );
        assert_eq!(
            preferred_drop_effect_for_file_verb(ShellClipboardVerb::Paste),
            None,
            "Paste remains a folder-background canonical Shell verb"
        );
    }
}
