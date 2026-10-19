//! One drag owns its Shell snapshot independently of App/viewer lifetime.
//! Keep this object apartment-bound: the inner Shell object is used only on the UI STA.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Foundation::E_UNEXPECTED;
use windows::Win32::System::Com::{
    FORMATETC, IAdviseSink, IBindCtx, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA,
    STGMEDIUM,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{IDataObjectAsyncCapability, IDataObjectAsyncCapability_Impl};
use windows::core::{BOOL, HRESULT, IUnknownImpl, Interface, Ref, Result};

static NEXT_DRAG_ID: AtomicU64 = AtomicU64::new(1);

/// Local references live until SHDoDragDrop and its post-modal cleanup have returned.
/// Running operations then own themselves until the target calls EndOperation.
pub(super) struct DragDataObject {
    pub(super) data: IDataObject,
    capability: IDataObjectAsyncCapability,
    pub(super) id: u64,
}

impl DragDataObject {
    pub(super) fn new(inner: IDataObject) -> Result<Self> {
        let id = NEXT_DRAG_ID.fetch_add(1, Ordering::Relaxed);
        let data: IDataObject = AsyncDataObject {
            inner,
            state: RefCell::new(AsyncState::Disabled),
            id,
        }
        .into();
        let capability: IDataObjectAsyncCapability = data.cast()?;
        unsafe { capability.SetAsyncMode(true)? };
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
}

impl Drop for AsyncDataObject {
    fn drop(&mut self) {
        crate::logger::log(format!("file_drag: object released drag_id={}", self.id));
    }
}

impl AsyncDataObject_Impl {
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
        let keep_alive = {
            let mut state = self.state.borrow_mut();
            if !matches!(*state, AsyncState::Running(_)) {
                drop(state);
                return Err(self.invalid_transition("EndOperation"));
            }
            let AsyncState::Running(keep_alive) =
                std::mem::replace(&mut *state, AsyncState::Finished)
            else {
                unreachable!()
            };
            keep_alive
        };
        crate::logger::log(format!(
            "file_drag: async EndOperation drag_id={} thread={} result=0x{:08X} effect={effects}",
            self.id,
            unsafe { GetCurrentThreadId() },
            result.0 as u32,
        ));
        drop(keep_alive);
        Ok(())
    }
}

// Do not borrow async state across delegation. No App reference, viewer invalidation,
// extra data rendering, shutdown join, or clipboard completion behavior belongs here.
// Result<T> methods normalize successful HRESULTs to S_OK in windows-rs's generated
// vtable; the two HRESULT-returning methods preserve the inner success code exactly.
impl IDataObject_Impl for AsyncDataObject_Impl {
    fn GetData(&self, format: *const FORMATETC) -> Result<STGMEDIUM> {
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
        unsafe { self.inner.SetData(format, medium, release.as_bool()) }
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
