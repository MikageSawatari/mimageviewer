//! One drag owns its Shell snapshot independently of App/viewer lifetime.
//! Keep this object apartment-bound: the inner Shell object is used only on the UI STA.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Foundation::{E_UNEXPECTED, S_OK};
use windows::Win32::System::Com::{
    DVASPECT_CONTENT, FORMATETC, IAdviseSink, IBindCtx, IDataObject, IDataObject_Impl,
    IEnumFORMATETC, IEnumSTATDATA, STGMEDIUM, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{
    CFSTR_PERFORMEDDROPEFFECT, IDataObjectAsyncCapability, IDataObjectAsyncCapability_Impl,
};
use windows::core::{BOOL, HRESULT, IUnknownImpl, Interface, Ref, Result};

static NEXT_DRAG_ID: AtomicU64 = AtomicU64::new(1);

/// Local references live until SHDoDragDrop and its post-modal cleanup have returned.
/// Running operations own themselves until EndOperation or Performed DropEffect.
pub(super) struct DragDataObject {
    pub(super) data: IDataObject,
    capability: IDataObjectAsyncCapability,
    pub(super) id: u64,
}

impl DragDataObject {
    pub(super) fn new(inner: IDataObject) -> Result<Self> {
        let id = NEXT_DRAG_ID.fetch_add(1, Ordering::Relaxed);
        let performed_format =
            unsafe { RegisterClipboardFormatW(CFSTR_PERFORMEDDROPEFFECT) } as u16;
        if performed_format == 0 {
            return Err(windows::core::Error::from_win32());
        }
        let data: IDataObject = AsyncDataObject {
            inner,
            state: RefCell::new(AsyncState::Disabled),
            id,
            performed_format,
        }
        .into();
        let capability: IDataObjectAsyncCapability = data.cast()?;
        unsafe { capability.SetAsyncMode(true)? };
        #[cfg(test)]
        tests::observe_lifecycle_event("enabled", id);
        crate::logger::log(format!(
            "file_drag: async enabled drag_id={id} thread={}",
            unsafe { GetCurrentThreadId() }
        ));
        Ok(Self {
            data,
            capability,
            id,
        })
    }

    /// The modal HRESULT/effect is deliberately not an input: only Start/End own lifetime.
    pub(super) fn finish_modal(&self) -> Result<bool> {
        let in_operation = unsafe { self.capability.InOperation()? }.as_bool();
        if !in_operation {
            // Synchronous targets and Esc never call EndOperation. Break the Armed cycle.
            self.data
                .cast_object_ref::<AsyncDataObject>()?
                .disarm_unstarted();
        }
        Ok(in_operation)
    }
}

enum AsyncState {
    Disabled,
    Armed(IDataObjectAsyncCapability),
    Running(IDataObjectAsyncCapability),
    Finished,
}

// windows-implement defaults to agile/FTM. Opt out, otherwise delegation could access
// the STA-bound Shell object directly from the target's background apartment.
#[windows::core::implement(IDataObject, IDataObjectAsyncCapability, Agile = false)]
struct AsyncDataObject {
    inner: IDataObject,
    state: RefCell<AsyncState>,
    id: u64,
    performed_format: u16,
}

impl Drop for AsyncDataObject {
    fn drop(&mut self) {
        #[cfg(test)]
        tests::observe_lifecycle_event("released", self.id);
        crate::logger::log(format!("file_drag: object released drag_id={}", self.id));
    }
}

impl AsyncDataObject_Impl {
    /// Both COM entry points hold a call reference throughout delegation/completion.
    /// Recheck state after delegation: the inner object may have reentered EndOperation.
    fn complete_running(&self, result: HRESULT, effects: u32, via: &str) -> bool {
        let keep_alive = {
            let mut state = self.state.borrow_mut();
            if !matches!(*state, AsyncState::Running(_)) {
                return false;
            }
            let AsyncState::Running(keep_alive) =
                std::mem::replace(&mut *state, AsyncState::Finished)
            else {
                unreachable!()
            };
            keep_alive
        };
        crate::logger::log(format!(
            "file_drag: async completed drag_id={} thread={} via={via} result=0x{:08X} effect={effects}",
            self.id,
            unsafe { GetCurrentThreadId() },
            result.0 as u32,
        ));
        drop(keep_alive);
        #[cfg(test)]
        tests::observe_lifecycle_event("end", self.id);
        true
    }

    fn performed_effect(&self, format: *const FORMATETC, medium: *const STGMEDIUM) -> Option<u32> {
        let format = unsafe { format.as_ref() }?;
        if format.cfFormat != self.performed_format
            || format.dwAspect != DVASPECT_CONTENT.0
            || format.lindex != -1
            || format.tymed & TYMED_HGLOBAL.0 as u32 == 0
        {
            return None;
        }
        let medium = unsafe { medium.as_ref() }?;
        if medium.tymed != TYMED_HGLOBAL.0 as u32 {
            return None;
        }
        let global = unsafe { medium.u.hGlobal };
        if unsafe { GlobalSize(global) } < std::mem::size_of::<u32>() {
            return None;
        }
        let ptr = unsafe { GlobalLock(global) };
        if ptr.is_null() {
            return None;
        }
        let effect = unsafe { ptr.cast::<u32>().read_unaligned() };
        let _ = unsafe { GlobalUnlock(global) };
        Some(effect)
    }

    fn disarm_unstarted(&self) {
        let old = {
            let mut state = self.state.borrow_mut();
            if matches!(*state, AsyncState::Armed(_)) {
                Some(std::mem::replace(&mut *state, AsyncState::Disabled))
            } else {
                None
            }
        };
        drop(old);
    }

    fn invalid_transition(&self, method: &str) -> windows::core::Error {
        crate::logger::log(format!(
            "file_drag: invalid async transition drag_id={} method={method}",
            self.id
        ));
        windows::core::Error::from_hresult(E_UNEXPECTED)
    }
}

impl IDataObjectAsyncCapability_Impl for AsyncDataObject_Impl {
    fn SetAsyncMode(&self, enabled: BOOL) -> Result<()> {
        // Keep the receiver alive while releasing a possible final self-hold, and perform
        // AddRef/Release outside any state borrow (COM delegation can reenter this object).
        let call_ref = self.to_interface::<IDataObjectAsyncCapability>();
        let old = {
            let mut state = self.state.borrow_mut();
            match (&*state, enabled.as_bool()) {
                (AsyncState::Disabled, true) => {
                    *state = AsyncState::Armed(call_ref);
                    None
                }
                (AsyncState::Armed(_) | AsyncState::Running(_), true)
                | (AsyncState::Disabled, false) => None,
                (AsyncState::Armed(_) | AsyncState::Finished, false) => {
                    Some(std::mem::replace(&mut *state, AsyncState::Disabled))
                }
                _ => {
                    drop(state);
                    return Err(self.invalid_transition("SetAsyncMode"));
                }
            }
        };
        drop(old);
        Ok(())
    }

    fn GetAsyncMode(&self) -> Result<BOOL> {
        Ok((!matches!(*self.state.borrow(), AsyncState::Disabled)).into())
    }

    fn StartOperation(&self, _reserved: Ref<'_, IBindCtx>) -> Result<()> {
        {
            let mut state = self.state.borrow_mut();
            if !matches!(*state, AsyncState::Armed(_)) {
                drop(state);
                return Err(self.invalid_transition("StartOperation"));
            }
            let AsyncState::Armed(keep_alive) =
                std::mem::replace(&mut *state, AsyncState::Finished)
            else {
                unreachable!()
            };
            *state = AsyncState::Running(keep_alive);
        }
        crate::logger::log(format!(
            "file_drag: async StartOperation drag_id={} thread={}",
            self.id,
            unsafe { GetCurrentThreadId() }
        ));
        #[cfg(test)]
        tests::observe_lifecycle_event("start", self.id);
        Ok(())
    }

    fn InOperation(&self) -> Result<BOOL> {
        Ok(matches!(*self.state.borrow(), AsyncState::Running(_)).into())
    }

    fn EndOperation(
        &self,
        result: HRESULT,
        _reserved: Ref<'_, IBindCtx>,
        effects: u32,
    ) -> Result<()> {
        let _call_ref = self.to_interface::<IDataObjectAsyncCapability>();
        #[cfg(test)]
        tests::observe_lifecycle_event("end_entered", self.id);
        crate::logger::log(format!(
            "file_drag: async EndOperation received drag_id={} thread={} result=0x{:08X} effect={effects}",
            self.id,
            unsafe { GetCurrentThreadId() },
            result.0 as u32,
        ));
        if self.complete_running(result, effects, "end_operation")
            || matches!(*self.state.borrow(), AsyncState::Finished)
        {
            Ok(())
        } else {
            Err(self.invalid_transition("EndOperation"))
        }
    }
}

// Do not borrow async state across delegation. No App reference, viewer invalidation,
// extra data rendering, shutdown join, or clipboard completion behavior belongs here.
// Result<T> methods normalize successful HRESULTs to S_OK in windows-rs's generated
// vtable; the two HRESULT-returning methods preserve the inner success code exactly.
impl IDataObject_Impl for AsyncDataObject_Impl {
    fn GetData(&self, format: *const FORMATETC) -> Result<STGMEDIUM> {
        #[cfg(test)]
        tests::observe_lifecycle_event("get_data", self.id);
        unsafe { self.inner.GetData(format) }
    }

    fn GetDataHere(&self, format: *const FORMATETC, medium: *mut STGMEDIUM) -> Result<()> {
        unsafe { self.inner.GetDataHere(format, medium) }
    }

    fn QueryGetData(&self, format: *const FORMATETC) -> HRESULT {
        unsafe { self.inner.QueryGetData(format) }
    }

    fn GetCanonicalFormatEtc(&self, input: *const FORMATETC, output: *mut FORMATETC) -> HRESULT {
        unsafe { self.inner.GetCanonicalFormatEtc(input, output) }
    }

    fn SetData(
        &self,
        format: *const FORMATETC,
        medium: *const STGMEDIUM,
        release: BOOL,
    ) -> Result<()> {
        let _call_ref = self.to_interface::<IDataObject>();
        // fRelease can destroy the HGLOBAL inside SetData: read only before delegation.
        // Logical Performed DropEffect can arrive before copying; it is not terminal.
        let effect = self.performed_effect(format, medium);
        #[cfg(test)]
        tests::observe_set_data(self.id, format, medium, release);
        let result = unsafe { self.inner.SetData(format, medium, release.as_bool()) };
        if let Some(effect) = effect {
            // The SDK's notification completes the outer owner even if the inner
            // Shell object does not store it (e.g. E_NOTIMPL). Preserve that HRESULT.
            self.complete_running(S_OK, effect, "performed_drop_effect");
        }
        result
    }

    fn EnumFormatEtc(&self, direction: u32) -> Result<IEnumFORMATETC> {
        unsafe { self.inner.EnumFormatEtc(direction) }
    }

    fn DAdvise(
        &self,
        format: *const FORMATETC,
        flags: u32,
        sink: Ref<'_, IAdviseSink>,
    ) -> Result<u32> {
        unsafe { self.inner.DAdvise(format, flags, sink.as_ref()) }
    }

    fn DUnadvise(&self, connection: u32) -> Result<()> {
        unsafe { self.inner.DUnadvise(connection) }
    }

    fn EnumDAdvise(&self) -> Result<IEnumSTATDATA> {
        unsafe { self.inner.EnumDAdvise() }
    }
}

#[cfg(test)]
mod tests;
