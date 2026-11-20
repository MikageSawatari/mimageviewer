//! STA-bound adapter; no filesystem access or App references in OLE callbacks.

use super::*;
use std::mem::ManuallyDrop;
use std::os::windows::ffi::OsStringExt;
use windows::Win32::Foundation::{E_INVALIDARG, HWND, LPARAM, POINT, POINTL, RECT, S_OK, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM,
    STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl,
    RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{
    CFSTR_DROPDESCRIPTION, CLSID_DragDropHelper, DROPDESCRIPTION, DROPIMAGE_COPY,
    DROPIMAGE_INVALID, DROPIMAGE_MOVE, DROPIMAGE_NONE, DragQueryFileW, HDROP, IDropTargetHelper,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, PostMessageW, WM_USER};
use windows::core::{Ref, Result as WinResult};

pub(crate) fn capture_window_metrics(hwnd: isize) -> Option<WindowMetrics> {
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    let mut rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut rect) }.ok()?;
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    (dpi != 0).then_some(WindowMetrics {
        client_size_px: [rect.right - rect.left, rect.bottom - rect.top],
        dpi,
    })
}

pub(crate) fn register_root_drop_target(
    hwnd: isize,
    ctx: egui::Context,
    shared: SharedMainDrop,
) -> std::result::Result<(), String> {
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    let helper: IDropTargetHelper =
        unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) }
            .map_err(|error| format!("ドロップ表示を準備できません: {error}"))?;
    let locate_ctx = ctx.clone();
    let locate_shared = shared.clone();
    let locate = Box::new(move |point: POINTL| {
        let metrics = capture_window_metrics(hwnd.0 as isize)?;
        let mut point = POINT {
            x: point.x,
            y: point.y,
        };
        if !unsafe { ScreenToClient(hwnd, &mut point) }.as_bool() {
            return None;
        }
        // Exactly one Context data read; never nest a Context call inside it.
        let presented = egui_wgpu::last_presented_pass(&locate_ctx, egui::ViewportId::ROOT);
        hit_test(
            &locate_shared.borrow().geometries,
            presented,
            metrics,
            [point.x, point.y],
        )
    });
    let target: IDropTarget = MainDropTarget {
        hwnd,
        ctx,
        shared,
        locate,
        helper: Rc::new(ShellVisuals(helper)),
        session: RefCell::new(None),
    }
    .into();
    unsafe { RevokeDragDrop(hwnd) }
        .map_err(|error| format!("既存のドロップ登録を解除できません: {error}"))?;
    unsafe { RegisterDragDrop(hwnd, &target) }
        .map_err(|error| format!("ドロップ先を登録できません: {error}"))
}

trait Visuals {
    fn enter(&self, hwnd: HWND, data: &IDataObject, point: &POINT, effect: DROPEFFECT);
    fn over(&self, point: &POINT, effect: DROPEFFECT);
    fn leave(&self);
    fn dropped(&self, data: &IDataObject, point: &POINT, effect: DROPEFFECT);
}

struct ShellVisuals(IDropTargetHelper);

fn log_visual(result: WinResult<()>) {
    if let Err(error) = result {
        crate::logger::log(format!(
            "main_drop_target: drag image helper failed: {error}"
        ));
    }
}

impl Visuals for ShellVisuals {
    fn enter(&self, hwnd: HWND, data: &IDataObject, point: &POINT, effect: DROPEFFECT) {
        log_visual(unsafe { self.0.DragEnter(hwnd, data, point, effect) });
    }
    fn over(&self, point: &POINT, effect: DROPEFFECT) {
        log_visual(unsafe { self.0.DragOver(point, effect) });
    }
    fn leave(&self) {
        log_visual(unsafe { self.0.DragLeave() });
    }
    fn dropped(&self, data: &IDataObject, point: &POINT, effect: DROPEFFECT) {
        log_visual(unsafe { self.0.Drop(data, point, effect) });
    }
}

#[windows::core::implement(IDropTarget, Agile = false)]
struct MainDropTarget {
    hwnd: HWND,
    ctx: egui::Context,
    shared: SharedMainDrop,
    locate: Box<dyn Fn(POINTL) -> Option<Hit>>,
    helper: Rc<dyn Visuals>,
    session: RefCell<Option<Rc<DragSession>>>,
}

impl MainDropTarget_Impl {
    fn evaluate(
        &self,
        session: &DragSession,
        point: POINTL,
        modifiers: u32,
        dropping: bool,
    ) -> (Decision, Option<InternalDrag>) {
        let hit = (self.locate)(point);
        let shared = self.shared.borrow();
        let decision = decide(
            hit,
            shared.admission.is_ok(),
            shared.internal.as_ref(),
            session.has_paths,
            session.allows_copy,
            modifiers,
        );
        // Hover needs no path snapshot; clone the complete payload only for Drop.
        let internal = if dropping {
            shared.internal.clone()
        } else {
            None
        };
        (decision, internal)
    }

    fn session(&self) -> Option<Rc<DragSession>> {
        self.session.borrow().clone()
    }

    fn finish_session(&self) {
        let old = self.session.borrow_mut().take();
        // Describe/Release can reenter COM; neither RefCell is borrowed here.
        if let Some(session) = old {
            session.describe(Description::Invalid);
        }
    }
}

fn effect(decision: &Decision) -> DROPEFFECT {
    if decision.accepted() {
        DROPEFFECT_COPY
    } else {
        DROPEFFECT_NONE
    }
}

impl IDropTarget_Impl for MainDropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<'_, IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        output: *mut DROPEFFECT,
    ) -> WinResult<()> {
        let output = unsafe { output.as_mut() }.ok_or(E_INVALIDARG)?;
        let allows_copy = output.0 & DROPEFFECT_COPY.0 != 0;
        *output = DROPEFFECT_NONE;
        self.finish_session();
        let data = data.as_ref().ok_or(E_INVALIDARG)?;
        let internal = self.shared.borrow().internal.is_some();
        let session = Rc::new(DragSession::new(
            Rc::new(OleData(data.clone())),
            allows_copy,
            internal,
        ));
        let old = self.session.borrow_mut().replace(session.clone());
        drop(old);
        let (decision, _) = self.evaluate(&session, *point, keys.0, false);
        *output = effect(&decision);
        let point = POINT {
            x: point.x,
            y: point.y,
        };
        self.helper.enter(self.hwnd, data, &point, *output);
        session.describe(decision.description());
        Ok(())
    }

    fn DragOver(
        &self,
        keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        output: *mut DROPEFFECT,
    ) -> WinResult<()> {
        let output = unsafe { output.as_mut() }.ok_or(E_INVALIDARG)?;
        *output = DROPEFFECT_NONE;
        if let Some(session) = self.session() {
            let (decision, _) = self.evaluate(&session, *point, keys.0, false);
            *output = effect(&decision);
            session.describe(decision.description());
        }
        self.helper.over(
            &POINT {
                x: point.x,
                y: point.y,
            },
            *output,
        );
        Ok(())
    }

    fn DragLeave(&self) -> WinResult<()> {
        self.finish_session();
        self.helper.leave();
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<'_, IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        output: *mut DROPEFFECT,
    ) -> WinResult<()> {
        let output = unsafe { output.as_mut() }.ok_or(E_INVALIDARG)?;
        *output = DROPEFFECT_NONE;
        let data = data.as_ref().ok_or(E_INVALIDARG)?;
        if let Some(session) = self.session() {
            let (decision, internal) = self.evaluate(&session, *point, keys.0, true);
            session.describe(decision.description());
            match session.drop_request(&decision, internal.as_ref()) {
                Ok(Some(request)) => {
                    self.shared.borrow_mut().drops.push(request);
                    *output = effect(&decision);
                    if internal.is_none() {
                        self.ctx.request_repaint();
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    self.shared
                        .borrow_mut()
                        .drops
                        .push(DropRequest::Failed(format!(
                            "ドロップした項目を読み取れません: {error}"
                        )));
                    self.ctx.request_repaint();
                }
            }
        }
        self.helper.dropped(
            data,
            &POINT {
                x: point.x,
                y: point.y,
            },
            *output,
        );
        self.finish_session();
        Ok(())
    }
}

struct OleData(IDataObject);

fn format(format: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: format,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

struct Medium(STGMEDIUM);
impl Drop for Medium {
    fn drop(&mut self) {
        unsafe { ReleaseStgMedium(&mut self.0) };
    }
}

impl DragData for OleData {
    fn has_paths(&self) -> bool {
        (unsafe { self.0.QueryGetData(&format(CF_HDROP.0)) }) == S_OK
    }

    fn paths(&self) -> std::result::Result<Vec<PathBuf>, String> {
        let medium =
            Medium(unsafe { self.0.GetData(&format(CF_HDROP.0)) }.map_err(|e| e.to_string())?);
        if medium.0.tymed != TYMED_HGLOBAL.0 as u32 {
            return Err("CF_HDROP is not HGLOBAL".into());
        }
        let hdrop = HDROP(unsafe { medium.0.u.hGlobal }.0);
        let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
        let mut paths = Vec::new();
        for index in 0..count {
            let length = unsafe { DragQueryFileW(hdrop, index, None) };
            if length == 0 {
                return Err("CF_HDROP contains an empty path".into());
            }
            let mut wide = vec![0; length as usize + 1];
            if unsafe { DragQueryFileW(hdrop, index, Some(&mut wide)) } != length {
                return Err("CF_HDROP path could not be read".into());
            }
            paths.push(PathBuf::from(std::ffi::OsString::from_wide(
                &wide[..length as usize],
            )));
        }
        Ok(paths)
    }

    fn set_description(&self, description: &Description) -> std::result::Result<(), String> {
        let mut value = DROPDESCRIPTION::default();
        let mut message = [0u16; 260];
        let mut insert = [0u16; 260];
        value.r#type = match description {
            Description::Invalid => DROPIMAGE_INVALID,
            Description::None => DROPIMAGE_NONE,
            Description::Transfer(operation, path) => {
                let text = match operation {
                    ShellTransferOperation::Copy => "%1 へコピー",
                    ShellTransferOperation::Move => "%1 へ移動",
                };
                write_wide(&mut message, text);
                let name = path
                    .file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy();
                write_wide(&mut insert, &name);
                match operation {
                    ShellTransferOperation::Copy => DROPIMAGE_COPY,
                    ShellTransferOperation::Move => DROPIMAGE_MOVE,
                }
            }
        };
        value.szMessage = message;
        value.szInsert = insert;
        let cf = unsafe { RegisterClipboardFormatW(CFSTR_DROPDESCRIPTION) };
        if cf == 0 {
            return Err("RegisterClipboardFormat(DropDescription) failed".into());
        }
        let hglobal = unsafe { GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of::<DROPDESCRIPTION>()) }
            .map_err(|e| e.to_string())?;
        let medium = Medium(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: hglobal },
            pUnkForRelease: ManuallyDrop::new(None),
        });
        let ptr = unsafe { GlobalLock(hglobal) };
        if ptr.is_null() {
            return Err("GlobalLock(DropDescription) failed".into());
        }
        unsafe { ptr.cast::<DROPDESCRIPTION>().write_unaligned(value) };
        let _ = unsafe { GlobalUnlock(hglobal) };
        // FALSE leaves ownership local even when SetData fails; no completion-effect writes.
        unsafe { self.0.SetData(&format(cf as u16), &medium.0, false) }
            .map_err(|e| e.to_string())?;
        update_drag_window(&self.0);
        Ok(())
    }
}

fn write_wide(output: &mut [u16; 260], text: &str) {
    for (slot, value) in output[..259].iter_mut().zip(text.encode_utf16()) {
        *slot = value;
    }
}

fn update_drag_window(data: &IDataObject) {
    let cf = unsafe { RegisterClipboardFormatW(windows::core::w!("DragWindow")) };
    if cf == 0 {
        return;
    }
    let Ok(medium) = (unsafe { data.GetData(&format(cf as u16)) }) else {
        return;
    };
    let medium = Medium(medium);
    if medium.0.tymed != TYMED_HGLOBAL.0 as u32 {
        return;
    }
    let global = unsafe { medium.0.u.hGlobal };
    if unsafe { GlobalSize(global) } < std::mem::size_of::<u32>() {
        return;
    }
    let ptr = unsafe { GlobalLock(global) };
    if ptr.is_null() {
        return;
    }
    // Shell's DragWindow format is a DWORD representation even on 64-bit Windows.
    let handle = unsafe { ptr.cast::<u32>().read_unaligned() };
    let _ = unsafe { GlobalUnlock(global) };
    if handle != 0 {
        let _ = unsafe {
            PostMessageW(
                Some(HWND(handle as usize as *mut core::ffi::c_void)),
                WM_USER + 3,
                WPARAM(0),
                LPARAM(0),
            )
        };
    }
}

#[cfg(test)]
mod tests;
