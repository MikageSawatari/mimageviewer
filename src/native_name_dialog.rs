//! Windows standard single-line name input dialog.
//!
//! The dialog is deliberately synchronous: `DialogBoxIndirectParamW` owns the
//! nested modal message loop, including IME, clipboard, undo, Tab, Enter, and
//! Escape behavior. Callers must not hold application locks across
//! [`prompt_name`].

/// Parameters for a filesystem name prompt.
pub(crate) struct NameInputRequest<'a> {
    /// Owner window (`App::main_hwnd`).
    pub owner: Option<isize>,
    pub title: &'a str,
    pub caption: &'a str,
    pub initial: &'a str,
    /// Initial selection in UTF-16 code units. `None` selects all text.
    pub select_utf16: Option<(usize, usize)>,
}

/// Outcome of one prompt. `Failed` is kept apart from `Cancelled` on purpose:
/// a dialog that cannot be created would otherwise make the menu item do
/// nothing at all, with no way to tell that from the user declining.
pub(crate) enum NamePromptOutcome {
    Accepted(String),
    Cancelled,
    Failed,
}

#[cfg(windows)]
pub(crate) fn prompt_name(req: &NameInputRequest<'_>) -> NamePromptOutcome {
    windows_impl::prompt_name(req)
}

/// Non-Windows CI stub. Filesystem name editing is a Windows-only UI feature.
#[cfg(not(windows))]
pub(crate) fn prompt_name(req: &NameInputRequest<'_>) -> NamePromptOutcome {
    let _ = (
        req.owner,
        req.title,
        req.caption,
        req.initial,
        req.select_utf16,
    );
    NamePromptOutcome::Failed
}

#[cfg(windows)]
pub(crate) fn show_warning(owner: Option<isize>, title: &str, message: &str) {
    windows_impl::show_warning(owner, title, message);
}

#[cfg(not(windows))]
pub(crate) fn show_warning(_owner: Option<isize>, _title: &str, _message: &str) {}

#[cfg(windows)]
pub(crate) fn confirm_warning(owner: Option<isize>, title: &str, message: &str) -> bool {
    windows_impl::confirm_warning(owner, title, message)
}

#[cfg(not(windows))]
pub(crate) fn confirm_warning(_owner: Option<isize>, _title: &str, _message: &str) -> bool {
    false
}

#[cfg(windows)]
mod windows_impl {
    use std::ffi::c_void;
    use std::mem::{align_of, size_of};

    use windows::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    };
    use windows::Win32::System::SystemServices::SS_PATHELLIPSIS;
    use windows::Win32::UI::Controls::{EM_LIMITTEXT, EM_SETSEL};
    use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows::Win32::UI::WindowsAndMessaging::{
        BS_DEFPUSHBUTTON, BS_PUSHBUTTON, DLGPROC, DLGTEMPLATE, DS_MODALFRAME, DS_SETFONT,
        DialogBoxIndirectParamW, ES_AUTOHSCROLL, EndDialog, GetDlgItem, GetWindowLongPtrW,
        GetWindowRect, GetWindowTextLengthW, GetWindowTextW, HWND_TOP, IDCANCEL, IDOK,
        MB_ICONWARNING, MB_OK, MB_OKCANCEL, MessageBoxW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        SWP_SHOWWINDOW, SendMessageW, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos,
        WINDOW_LONG_PTR_INDEX, WM_CLOSE, WM_COMMAND, WM_INITDIALOG, WS_BORDER, WS_CAPTION,
        WS_CHILD, WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
    };
    use windows::core::PCWSTR;

    use super::{NameInputRequest, NamePromptOutcome};

    const NAME_EDIT_ID: i32 = 1001;
    const MAX_NAME_UTF16: usize = 255;

    const STATIC_CLASS: u16 = 0x0082;
    const EDIT_CLASS: u16 = 0x0081;
    const BUTTON_CLASS: u16 = 0x0080;

    struct DialogState {
        owner: Option<isize>,
        initial_utf16_len: usize,
        select_utf16: Option<(usize, usize)>,
        result: Option<String>,
    }

    struct DialogTemplate {
        // `DialogBoxIndirectParamW` requires DWORD alignment. A `Vec<u8>` does
        // not provide that guarantee, so the completed byte stream is stored
        // in an aligned `Vec<u32>`.
        words: Vec<u32>,
        #[cfg(test)]
        item_offsets: Vec<usize>,
    }

    impl DialogTemplate {
        fn as_ptr(&self) -> *const DLGTEMPLATE {
            debug_assert_eq!(self.words.as_ptr() as usize % align_of::<DLGTEMPLATE>(), 0);
            self.words.as_ptr().cast()
        }

        #[cfg(test)]
        fn bytes(&self) -> Vec<u8> {
            self.words
                .iter()
                .flat_map(|word| word.to_le_bytes())
                .collect()
        }
    }

    pub(super) fn prompt_name(req: &NameInputRequest<'_>) -> NamePromptOutcome {
        let template = build_dialog_template(req.title, req.caption, req.initial);
        let mut state = DialogState {
            owner: req.owner,
            initial_utf16_len: req.initial.encode_utf16().count(),
            select_utf16: req.select_utf16,
            result: None,
        };
        let owner = hwnd_from_raw(req.owner);
        let dialog_result = unsafe {
            DialogBoxIndirectParamW(
                None,
                template.as_ptr(),
                owner,
                Some(dialog_proc),
                LPARAM((&mut state as *mut DialogState) as isize),
            )
        };
        if dialog_result == IDOK.0 as isize {
            match state.result.take() {
                Some(name) => NamePromptOutcome::Accepted(name),
                // OK was pressed but the EDIT text could not be read back.
                None => {
                    crate::logger::log("native_name_dialog: accepted but text read failed");
                    NamePromptOutcome::Failed
                }
            }
        } else if dialog_result == IDCANCEL.0 as isize {
            NamePromptOutcome::Cancelled
        } else {
            // -1 means the dialog could not be created; 0 means an invalid owner.
            // A malformed template would otherwise silently disable the caller.
            let error = unsafe { GetLastError() };
            crate::logger::log(format!(
                "native_name_dialog: DialogBoxIndirectParamW returned {dialog_result} \
                 (GetLastError={:#x})",
                error.0
            ));
            NamePromptOutcome::Failed
        }
    }

    pub(super) fn show_warning(owner: Option<isize>, title: &str, message: &str) {
        let title = wide_z(title);
        let message = wide_z(message);
        unsafe {
            let _ = MessageBoxW(
                hwnd_from_raw(owner),
                PCWSTR(message.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONWARNING,
            );
        }
    }

    pub(super) fn confirm_warning(owner: Option<isize>, title: &str, message: &str) -> bool {
        let title = wide_z(title);
        let message = wide_z(message);
        unsafe {
            MessageBoxW(
                hwnd_from_raw(owner),
                PCWSTR(message.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OKCANCEL | MB_ICONWARNING,
            ) == IDOK
        }
    }

    unsafe extern "system" fn dialog_proc(
        dialog: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> isize {
        match message {
            WM_INITDIALOG => {
                let state_ptr = lparam.0 as *mut DialogState;
                if state_ptr.is_null() {
                    return 0;
                }
                unsafe {
                    SetWindowLongPtrW(dialog, dialog_user_index(), state_ptr as isize);
                    let state = &mut *state_ptr;
                    place_over_owner(dialog, state.owner);
                    if let Ok(edit) = GetDlgItem(Some(dialog), NAME_EDIT_ID) {
                        let _ =
                            SendMessageW(edit, EM_LIMITTEXT, Some(WPARAM(MAX_NAME_UTF16)), None);
                        let (start, end) = match state.select_utf16 {
                            Some((start, end)) => (
                                start.min(state.initial_utf16_len),
                                end.min(state.initial_utf16_len) as isize,
                            ),
                            None => (0, -1),
                        };
                        let _ =
                            SendMessageW(edit, EM_SETSEL, Some(WPARAM(start)), Some(LPARAM(end)));
                        let _ = SetFocus(Some(edit));
                    }
                }
                // Focus was assigned explicitly to the EDIT control.
                0
            }
            WM_COMMAND => {
                let command_id = (wparam.0 & 0xffff) as i32;
                if command_id == IDOK.0 {
                    let result = unsafe { read_name(dialog) };
                    if let Some(state) = unsafe { dialog_state_ptr(dialog).as_mut() } {
                        state.result = result;
                    }
                    let _ = unsafe { EndDialog(dialog, IDOK.0 as isize) };
                    1
                } else if command_id == IDCANCEL.0 {
                    if let Some(state) = unsafe { dialog_state_ptr(dialog).as_mut() } {
                        state.result = None;
                    }
                    let _ = unsafe { EndDialog(dialog, IDCANCEL.0 as isize) };
                    1
                } else {
                    0
                }
            }
            WM_CLOSE => {
                if let Some(state) = unsafe { dialog_state_ptr(dialog).as_mut() } {
                    state.result = None;
                }
                let _ = unsafe { EndDialog(dialog, IDCANCEL.0 as isize) };
                1
            }
            _ => 0,
        }
    }

    unsafe fn dialog_state_ptr(dialog: HWND) -> *mut DialogState {
        (unsafe { GetWindowLongPtrW(dialog, dialog_user_index()) }) as *mut DialogState
    }

    unsafe fn read_name(dialog: HWND) -> Option<String> {
        let edit = unsafe { GetDlgItem(Some(dialog), NAME_EDIT_ID) }.ok()?;
        let len = unsafe { GetWindowTextLengthW(edit) };
        if len < 0 {
            return None;
        }
        let mut buffer = vec![0u16; len as usize + 1];
        let copied = unsafe { GetWindowTextW(edit, &mut buffer) };
        if copied < 0 {
            return None;
        }
        String::from_utf16(&buffer[..copied as usize]).ok()
    }

    /// Place the dialog over its owner and put it in front.
    ///
    /// Only the owner is disabled by the dialog manager, and this application keeps other
    /// top-level windows of its own - the fullscreen viewport and the native video window -
    /// alive beside it. If the dialog is not raised and focused, the application looks frozen
    /// while the modal waits somewhere behind them. The position is clamped to the owner's
    /// monitor so a centred dialog cannot land off-screen either.
    unsafe fn place_over_owner(dialog: HWND, owner: Option<isize>) {
        let mut dialog_rect = RECT::default();
        if unsafe { GetWindowRect(dialog, &mut dialog_rect) }.is_err() {
            return;
        }
        let width = dialog_rect.right - dialog_rect.left;
        let height = dialog_rect.bottom - dialog_rect.top;

        if let Some(owner) = hwnd_from_raw(owner) {
            let mut owner_rect = RECT::default();
            if unsafe { GetWindowRect(owner, &mut owner_rect) }.is_ok() {
                let mut x = owner_rect.left + ((owner_rect.right - owner_rect.left - width) / 2);
                let mut y = owner_rect.top + ((owner_rect.bottom - owner_rect.top - height) / 2);
                if let Some(work) = unsafe { owner_work_area(owner) } {
                    x = x.clamp(work.left, (work.right - width).max(work.left));
                    y = y.clamp(work.top, (work.bottom - height).max(work.top));
                }
                let _ = unsafe {
                    SetWindowPos(
                        dialog,
                        Some(HWND_TOP),
                        x,
                        y,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOACTIVATE,
                    )
                };
            }
        }

        // Raise and focus explicitly. A sibling window of ours that re-asserts foreground
        // would otherwise leave the modal unreachable except through Alt+Tab.
        let _ = unsafe {
            SetWindowPos(
                dialog,
                Some(HWND_TOP),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            )
        };
        let _ = unsafe { SetForegroundWindow(dialog) };
    }

    unsafe fn owner_work_area(owner: HWND) -> Option<RECT> {
        let monitor = unsafe { MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST) };
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            Some(info.rcWork)
        } else {
            None
        }
    }

    fn dialog_user_index() -> WINDOW_LONG_PTR_INDEX {
        // DWLP_USER = DWLP_MSGRESULT + sizeof(LRESULT) + sizeof(DLGPROC).
        WINDOW_LONG_PTR_INDEX((size_of::<LRESULT>() + size_of::<DLGPROC>()) as i32)
    }

    fn hwnd_from_raw(raw: Option<isize>) -> Option<HWND> {
        raw.filter(|value| *value != 0)
            .map(|value| HWND(value as *mut c_void))
    }

    fn wide_z(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn build_dialog_template(title: &str, caption: &str, initial: &str) -> DialogTemplate {
        let mut bytes = Vec::new();
        #[cfg(test)]
        let mut item_offsets = Vec::with_capacity(4);
        push_dialog_header(&mut bytes, title);
        push_dialog_items(
            &mut bytes,
            #[cfg(test)]
            &mut item_offsets,
            caption,
            initial,
        );
        finish_dialog_template(
            bytes,
            #[cfg(test)]
            item_offsets,
        )
    }

    fn push_dialog_header(bytes: &mut Vec<u8>, title: &str) {
        push_u32(bytes, dialog_style());
        push_u32(bytes, 0);
        push_u16(bytes, 4);
        push_i16(bytes, 0);
        push_i16(bytes, 0);
        push_i16(bytes, 290);
        push_i16(bytes, 82);
        push_u16(bytes, 0);
        push_u16(bytes, 0);
        push_utf16z(bytes, title);
        push_u16(bytes, 9);
        push_utf16z(bytes, "MS\u{20}Shell\u{20}Dlg");
    }

    fn dialog_style() -> u32 {
        DS_MODALFRAME as u32 | DS_SETFONT as u32 | WS_POPUP.0 | WS_CAPTION.0 | WS_SYSMENU.0
    }

    fn push_dialog_items(
        bytes: &mut Vec<u8>,
        #[cfg(test)] item_offsets: &mut Vec<usize>,
        caption: &str,
        initial: &str,
    ) {
        push_item(
            bytes,
            #[cfg(test)]
            item_offsets,
            WS_CHILD.0 | WS_VISIBLE.0 | SS_PATHELLIPSIS.0,
            (7, 7, 276, 10),
            -1,
            STATIC_CLASS,
            caption,
        );
        push_item(
            bytes,
            #[cfg(test)]
            item_offsets,
            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | WS_BORDER.0 | ES_AUTOHSCROLL as u32,
            (7, 21, 276, 14),
            NAME_EDIT_ID,
            EDIT_CLASS,
            initial,
        );
        push_item(
            bytes,
            #[cfg(test)]
            item_offsets,
            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
            (172, 57, 52, 14),
            IDOK.0,
            BUTTON_CLASS,
            "OK",
        );
        push_item(
            bytes,
            #[cfg(test)]
            item_offsets,
            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            (231, 57, 52, 14),
            IDCANCEL.0,
            BUTTON_CLASS,
            "キャンセル",
        );
    }

    fn finish_dialog_template(
        mut bytes: Vec<u8>,
        #[cfg(test)] item_offsets: Vec<usize>,
    ) -> DialogTemplate {
        align_dword(&mut bytes);
        let words = bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        DialogTemplate {
            words,
            #[cfg(test)]
            item_offsets,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_item(
        bytes: &mut Vec<u8>,
        #[cfg(test)] item_offsets: &mut Vec<usize>,
        style: u32,
        rect: (i16, i16, i16, i16),
        id: i32,
        class_ordinal: u16,
        title: &str,
    ) {
        align_dword(bytes);
        #[cfg(test)]
        item_offsets.push(bytes.len());
        push_u32(bytes, style);
        push_u32(bytes, 0);
        push_i16(bytes, rect.0);
        push_i16(bytes, rect.1);
        push_i16(bytes, rect.2);
        push_i16(bytes, rect.3);
        push_u16(bytes, id as u16);
        push_u16(bytes, 0xffff);
        push_u16(bytes, class_ordinal);
        push_utf16z(bytes, title);
        push_u16(bytes, 0);
    }

    fn align_dword(bytes: &mut Vec<u8>) {
        while !bytes.len().is_multiple_of(4) {
            bytes.push(0);
        }
    }

    fn push_u16(bytes: &mut Vec<u8>, value: u16) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_i16(bytes: &mut Vec<u8>, value: i16) {
        push_u16(bytes, value as u16);
    }

    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_utf16z(bytes: &mut Vec<u8>, value: &str) {
        for unit in value.encode_utf16().chain(std::iter::once(0)) {
            push_u16(bytes, unit);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{DLGTEMPLATE, build_dialog_template};
        use std::mem::{align_of, size_of};

        #[test]
        fn template_is_dword_aligned_and_contains_four_aligned_items() {
            let template =
                build_dialog_template("名前の変更", "対象:\u{20}C:\\very\\long", "🎬movie.mp4");
            let bytes = template.bytes();

            assert_eq!(
                template.words.as_ptr() as usize % align_of::<DLGTEMPLATE>(),
                0
            );
            assert_eq!(bytes.len() % 4, 0);
            assert!(bytes.len() >= size_of::<DLGTEMPLATE>());
            assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), 4);
            assert_eq!(template.item_offsets.len(), 4);
            assert!(template.item_offsets.iter().all(|offset| offset % 4 == 0));
        }

        #[test]
        fn template_embeds_utf16_title_caption_initial_and_font() {
            let values = [
                "名前の変更",
                "対象:\u{20}C:\\画像\\🎬movie.mp4",
                "🎬movie.mp4",
                "MS\u{20}Shell\u{20}Dlg",
            ];
            let template = build_dialog_template(values[0], values[1], values[2]);
            let bytes = template.bytes();

            for value in values {
                let needle: Vec<u8> = value
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .flat_map(u16::to_le_bytes)
                    .collect();
                assert!(bytes.windows(needle.len()).any(|window| window == needle));
            }
        }
    }
}
