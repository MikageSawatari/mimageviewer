//! Headless COM tests: no clipboard, drag loop, application window, or user profile.
use super::*;
use std::cell::Cell;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::ptr::null_mut;
use std::rc::Rc;
use std::sync::{Arc, atomic::AtomicUsize};
use windows::Win32::Foundation::{DATA_S_SAMEFORMATETC, E_NOINTERFACE, E_NOTIMPL, S_FALSE, S_OK};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize, CreateDataAdviseHolder, DATADIR_GET,
    DVASPECT_CONTENT, IAdviseSink_Impl, IDataObject_Vtbl, IMoniker, IStream, STGMEDIUM_0,
    TYMED_HGLOBAL,
};
use windows::Win32::System::Ole::{CF_HDROP, ReleaseStgMedium};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP, SHCreateStdEnumFmtEtc};
use windows::core::{GUID, IUnknown, IUnknown_Vtbl};

struct Sta;
impl Sta {
    fn new() -> Self {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap() };
        Self
    }
}
impl Drop for Sta {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
        ..Default::default()
    }
}

#[derive(Debug, PartialEq)]
struct Call(&'static str, Vec<usize>);
struct Probe {
    calls: RefCell<Vec<Call>>,
    status: Cell<HRESULT>,
    callback: RefCell<Option<Box<dyn FnOnce()>>>,
    medium: RefCell<Option<STGMEDIUM>>,
    formats: IEnumFORMATETC,
    advises: IEnumSTATDATA,
    drops: Arc<AtomicUsize>,
    get_thread: Cell<u32>,
}
impl Probe {
    fn record(&self, method: &'static str, args: Vec<usize>) -> HRESULT {
        self.calls.borrow_mut().push(Call(method, args));
        self.status.get()
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        if let Some(mut medium) = self.medium.get_mut().take() {
            unsafe {
                ReleaseStgMedium(&mut medium);
            }
        }
    }
}

// An ABI-level mock is intentional: windows-rs Result methods cannot produce S_FALSE.
// This lets the outer object's allowed S_OK normalization be tested separately from
// its two raw-HRESULT methods, without changing production vtables or dependencies.
#[repr(C)]
struct RawInner {
    vtable: &'static IDataObject_Vtbl,
    refs: Cell<u32>,
    probe: Rc<Probe>,
}
impl Drop for RawInner {
    fn drop(&mut self) {
        self.probe.drops.fetch_add(1, Ordering::SeqCst);
    }
}
unsafe fn inner<'a>(this: *mut c_void) -> &'a RawInner {
    unsafe { &*this.cast() }
}
unsafe extern "system" fn qi(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    unsafe {
        *out = null_mut();
    }
    if unsafe { *iid == IDataObject::IID || *iid == IUnknown::IID } {
        unsafe {
            *out = this;
            add_ref(this);
        }
        S_OK
    } else {
        E_NOINTERFACE
    }
}
unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    let refs = &unsafe { inner(this) }.refs;
    refs.set(refs.get() + 1);
    refs.get()
}
unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    let remaining = unsafe { inner(this) }.refs.get() - 1;
    unsafe { inner(this) }.refs.set(remaining);
    if remaining == 0 {
        drop(unsafe { Box::from_raw(this.cast::<RawInner>()) });
    }
    remaining
}
unsafe extern "system" fn get(
    this: *mut c_void,
    f: *const FORMATETC,
    m: *mut STGMEDIUM,
) -> HRESULT {
    let p = &unsafe { inner(this) }.probe;
    p.get_thread.set(unsafe { GetCurrentThreadId() });
    let hr = p.record("get", vec![f as usize]);
    let callback = p.callback.borrow_mut().take();
    if let Some(callback) = callback {
        callback();
    }
    if hr.is_ok() {
        unsafe {
            *m = p.medium.borrow_mut().take().unwrap_or_default();
        }
    }
    hr
}
unsafe extern "system" fn here(
    this: *mut c_void,
    f: *const FORMATETC,
    m: *mut STGMEDIUM,
) -> HRESULT {
    unsafe { inner(this) }
        .probe
        .record("here", vec![f as usize, m as usize])
}
unsafe extern "system" fn query(this: *mut c_void, f: *const FORMATETC) -> HRESULT {
    unsafe { inner(this) }
        .probe
        .record("query", vec![f as usize])
}
unsafe extern "system" fn canonical(
    this: *mut c_void,
    f: *const FORMATETC,
    out: *mut FORMATETC,
) -> HRESULT {
    let hr = unsafe { inner(this) }
        .probe
        .record("canonical", vec![f as usize, out as usize]);
    if hr.is_ok() {
        unsafe {
            *out = *f;
            (*out).lindex = 71;
        }
    }
    hr
}
unsafe extern "system" fn set(
    this: *mut c_void,
    f: *const FORMATETC,
    m: *const STGMEDIUM,
    take: BOOL,
) -> HRESULT {
    let hr = unsafe { inner(this) }
        .probe
        .record("set", vec![f as usize, m as usize, take.0 as usize]);
    if hr.is_ok() && take.as_bool() {
        // fRelease transfers the existing ownership, not the input structure's storage.
        // Bit-copy without AddRef, then let ReleaseStgMedium mutate only our local copy.
        let mut owned = unsafe { m.read() };
        unsafe {
            ReleaseStgMedium(&mut owned);
        }
    }
    hr
}
unsafe extern "system" fn enum_formats(
    this: *mut c_void,
    direction: u32,
    out: *mut *mut c_void,
) -> HRESULT {
    let p = &unsafe { inner(this) }.probe;
    let hr = p.record("formats", vec![direction as usize]);
    if hr.is_ok() {
        unsafe {
            *out = p.formats.clone().into_raw();
        }
    }
    hr
}
unsafe extern "system" fn advise(
    this: *mut c_void,
    f: *const FORMATETC,
    flags: u32,
    sink: *mut c_void,
    out: *mut u32,
) -> HRESULT {
    let hr = unsafe { inner(this) }
        .probe
        .record("advise", vec![f as usize, flags as usize, sink as usize]);
    if hr.is_ok() {
        unsafe {
            *out = 73;
        }
    }
    hr
}
unsafe extern "system" fn unadvise(this: *mut c_void, connection: u32) -> HRESULT {
    unsafe { inner(this) }
        .probe
        .record("unadvise", vec![connection as usize])
}
unsafe extern "system" fn enum_advises(this: *mut c_void, out: *mut *mut c_void) -> HRESULT {
    let p = &unsafe { inner(this) }.probe;
    let hr = p.record("advises", vec![]);
    if hr.is_ok() {
        unsafe {
            *out = p.advises.clone().into_raw();
        }
    }
    hr
}
static INNER_VTABLE: IDataObject_Vtbl = IDataObject_Vtbl {
    base__: IUnknown_Vtbl {
        QueryInterface: qi,
        AddRef: add_ref,
        Release: release,
    },
    GetData: get,
    GetDataHere: here,
    QueryGetData: query,
    GetCanonicalFormatEtc: canonical,
    SetData: set,
    EnumFormatEtc: enum_formats,
    DAdvise: advise,
    DUnadvise: unadvise,
    EnumDAdvise: enum_advises,
};
fn mock() -> (IDataObject, Rc<Probe>) {
    let probe = Rc::new(Probe {
        calls: RefCell::new(vec![]),
        status: Cell::new(S_OK),
        callback: RefCell::new(None),
        medium: RefCell::new(None),
        formats: unsafe { SHCreateStdEnumFmtEtc(&[format()]).unwrap() },
        advises: unsafe { CreateDataAdviseHolder().unwrap().EnumAdvise().unwrap() },
        drops: Arc::new(AtomicUsize::new(0)),
        get_thread: Cell::new(0),
    });
    let ptr = Box::into_raw(Box::new(RawInner {
        vtable: &INNER_VTABLE,
        refs: Cell::new(1),
        probe: probe.clone(),
    }));
    (unsafe { IDataObject::from_raw(ptr.cast()) }, probe)
}
fn wrapped() -> (DragDataObject, Rc<Probe>) {
    let (inner, probe) = mock();
    (DragDataObject::new(inner).unwrap(), probe)
}
#[windows::core::implement(IAdviseSink, Agile = false)]
struct Sink;
impl IAdviseSink_Impl for Sink_Impl {
    fn OnDataChange(&self, _: *const FORMATETC, _: *const STGMEDIUM) {}
    fn OnViewChange(&self, _: u32, _: i32) {}
    fn OnRename(&self, _: Ref<'_, IMoniker>) {}
    fn OnSave(&self) {}
    fn OnClose(&self) {}
}
fn invalid(result: windows::core::Result<()>) {
    assert_eq!(result.unwrap_err().code(), E_UNEXPECTED);
}

#[test]
fn identity_is_outer_and_apartment_bound() {
    let _sta = Sta::new();
    let (inner, probe) = mock();
    let inner_id = inner.cast::<IUnknown>().unwrap();
    let drag = DragDataObject::new(inner).unwrap();
    let data_id = drag.data.cast::<IUnknown>().unwrap();
    let async_id = drag.capability.cast::<IUnknown>().unwrap();
    assert_eq!(data_id, async_id);
    assert_ne!(data_id, inner_id);
    assert_eq!(drag.capability.cast::<IDataObject>().unwrap(), drag.data);
    // QI by IID also detects accidental defaults without enabling extra feature flags.
    for iid in [
        GUID::from_u128(0x94ea2b94_e9cc_49e0_c0ff_ee64ca8f5b90),
        GUID::from_u128(0x00000003_0000_0000_c000_000000000046),
        IStream::IID,
    ] {
        let mut out = null_mut();
        let hr = unsafe { (data_id.vtable().QueryInterface)(data_id.as_raw(), &iid, &mut out) };
        assert_eq!(hr, E_NOINTERFACE);
        assert!(out.is_null());
    }
    assert!(!drag.finish_modal().unwrap());
    drop((drag, data_id, async_id, inner_id));
    assert_eq!(probe.drops.load(Ordering::SeqCst), 1);
}

#[test]
fn state_contract_and_invalid_duplicates_are_stable() {
    let _sta = Sta::new();
    let (drag, probe) = wrapped();
    let a = &drag.capability;
    unsafe {
        assert!(a.GetAsyncMode().unwrap().as_bool());
        assert!(!a.InOperation().unwrap().as_bool());
        a.SetAsyncMode(true).unwrap(); // Must not accumulate self-references.
        invalid(a.EndOperation(S_OK, None::<&IBindCtx>, 1));
        a.StartOperation(None::<&IBindCtx>).unwrap();
        invalid(a.StartOperation(None::<&IBindCtx>));
        invalid(a.SetAsyncMode(false));
        a.SetAsyncMode(true).unwrap();
        assert!(a.InOperation().unwrap().as_bool());
        assert!(drag.finish_modal().unwrap());
        a.EndOperation(E_NOTIMPL, None::<&IBindCtx>, 0).unwrap();
        assert!(!a.InOperation().unwrap().as_bool());
        assert!(a.GetAsyncMode().unwrap().as_bool());
        invalid(a.EndOperation(S_OK, None::<&IBindCtx>, 1));
        invalid(a.StartOperation(None::<&IBindCtx>));
        invalid(a.SetAsyncMode(true));
    }
    drop(drag);
    assert_eq!(probe.drops.load(Ordering::SeqCst), 1);
}

#[test]
fn sync_cancel_and_disabled_states_release_without_end() {
    let _sta = Sta::new();
    let (drag, probe) = wrapped();
    assert!(!drag.finish_modal().unwrap());
    unsafe {
        assert!(!drag.capability.GetAsyncMode().unwrap().as_bool());
        drag.capability.SetAsyncMode(false).unwrap();
        invalid(drag.capability.StartOperation(None::<&IBindCtx>));
        invalid(drag.capability.EndOperation(S_OK, None::<&IBindCtx>, 0));
    }
    drop(drag);
    assert_eq!(probe.drops.load(Ordering::SeqCst), 1);
}

#[test]
fn end_before_modal_return_preserves_finished_mode() {
    let _sta = Sta::new();
    let (drag, probe) = wrapped();
    unsafe {
        drag.capability.StartOperation(None::<&IBindCtx>).unwrap();
        drag.capability
            .EndOperation(S_FALSE, None::<&IBindCtx>, 0)
            .unwrap();
    }
    assert!(!drag.finish_modal().unwrap());
    assert!(unsafe { drag.capability.GetAsyncMode().unwrap() }.as_bool());
    drop(drag);
    assert_eq!(probe.drops.load(Ordering::SeqCst), 1);
}

#[test]
fn running_self_hold_outlives_modal_locals_and_other_drags() {
    let _sta = Sta::new();
    let (a, pa) = wrapped();
    let (b, pb) = wrapped();
    assert_ne!(a.id, b.id);
    unsafe {
        a.capability.StartOperation(None::<&IBindCtx>).unwrap();
        b.capability.StartOperation(None::<&IBindCtx>).unwrap();
    }
    // There is deliberately no drop HRESULT/effect input: even a failed modal return
    // must leave Running alive. Keep only a borrowed callback address across local drop.
    let callback = a.capability.as_raw();
    assert!(a.finish_modal().unwrap());
    drop(a);
    assert_eq!(pa.drops.load(Ordering::SeqCst), 0);
    assert!(b.finish_modal().unwrap());
    assert_eq!(pb.drops.load(Ordering::SeqCst), 0);
    assert_eq!(pa.drops.load(Ordering::SeqCst), 0);
    let callback = unsafe { IDataObjectAsyncCapability::from_raw_borrowed(&callback) }
        .unwrap()
        .clone();
    unsafe {
        callback.EndOperation(S_OK, None::<&IBindCtx>, 1).unwrap();
    }
    // A real target owns a reference while invoking EndOperation. The self-hold is
    // gone afterward, but this caller's reference still keeps the receiver alive.
    assert_eq!(pa.drops.load(Ordering::SeqCst), 0);
    drop(callback);
    assert_eq!(pa.drops.load(Ordering::SeqCst), 1);
    assert!(unsafe { b.capability.InOperation().unwrap() }.as_bool());
    assert_eq!(pb.drops.load(Ordering::SeqCst), 0);
    unsafe {
        b.capability
            .EndOperation(S_OK, None::<&IBindCtx>, 1)
            .unwrap();
    }
    drop(b);
    assert_eq!(pb.drops.load(Ordering::SeqCst), 1);
}

#[test]
fn all_nine_forwards_preserve_arguments_interfaces_and_raw_status() {
    let _sta = Sta::new();
    let (drag, p) = wrapped();
    let f = format();
    let mut m = STGMEDIUM::default();
    let mut out = FORMATETC::default();
    for status in [S_FALSE, DATA_S_SAMEFORMATETC, E_NOTIMPL] {
        p.status.set(status);
        unsafe {
            assert_eq!(drag.data.QueryGetData(&f), status);
            assert_eq!(drag.data.GetCanonicalFormatEtc(&f, &mut out), status);
        }
    }
    assert_eq!(out.lindex, 71);
    p.status.set(S_FALSE);
    unsafe {
        let vt = drag.data.vtable();
        let raw = drag.data.as_raw();
        assert_eq!((vt.GetData)(raw, &f, &mut m), S_OK);
        assert_eq!((vt.GetDataHere)(raw, &f, &mut m), S_OK);
        assert_eq!((vt.SetData)(raw, &f, &m, false.into()), S_OK);
        let mut formats = null_mut();
        assert_eq!((vt.EnumFormatEtc)(raw, 8, &mut formats), S_OK);
        let formats = IEnumFORMATETC::from_raw(formats);
        assert_eq!(formats, p.formats);
        let sink: IAdviseSink = Sink.into();
        let mut connection = 0;
        assert_eq!(
            (vt.DAdvise)(raw, &f, 0x42, sink.as_raw(), &mut connection),
            S_OK
        );
        assert_eq!(connection, 73);
        assert_eq!((vt.DUnadvise)(raw, connection), S_OK);
        let mut advises = null_mut();
        assert_eq!((vt.EnumDAdvise)(raw, &mut advises), S_OK);
        assert_eq!(IEnumSTATDATA::from_raw(advises), p.advises);
        let calls = p.calls.borrow();
        assert_eq!(
            &calls[0..6],
            &[
                Call("query", vec![&f as *const _ as usize]),
                Call(
                    "canonical",
                    vec![&f as *const _ as usize, &mut out as *mut _ as usize]
                ),
                Call("query", vec![&f as *const _ as usize]),
                Call(
                    "canonical",
                    vec![&f as *const _ as usize, &mut out as *mut _ as usize]
                ),
                Call("query", vec![&f as *const _ as usize]),
                Call(
                    "canonical",
                    vec![&f as *const _ as usize, &mut out as *mut _ as usize]
                ),
            ]
        );
        assert_eq!(
            &calls[6..],
            &[
                Call("get", vec![&f as *const _ as usize]),
                Call(
                    "here",
                    vec![&f as *const _ as usize, &mut m as *mut _ as usize]
                ),
                Call(
                    "set",
                    vec![&f as *const _ as usize, &m as *const _ as usize, 0]
                ),
                Call("formats", vec![8]),
                Call(
                    "advise",
                    vec![&f as *const _ as usize, 0x42, sink.as_raw() as usize]
                ),
                Call("unadvise", vec![73]),
                Call("advises", vec![]),
            ]
        );
    }
    p.status.set(E_NOTIMPL);
    unsafe {
        assert_eq!(drag.data.GetData(&f).err().unwrap().code(), E_NOTIMPL);
        assert_eq!(
            drag.data.GetDataHere(&f, &mut m).unwrap_err().code(),
            E_NOTIMPL
        );
        assert_eq!(
            drag.data.SetData(&f, &m, true).unwrap_err().code(),
            E_NOTIMPL
        );
        assert_eq!(drag.data.EnumFormatEtc(1).unwrap_err().code(), E_NOTIMPL);
        assert_eq!(
            drag.data
                .DAdvise(&f, 0, None::<&IAdviseSink>)
                .unwrap_err()
                .code(),
            E_NOTIMPL
        );
        assert_eq!(drag.data.DUnadvise(73).unwrap_err().code(), E_NOTIMPL);
        assert_eq!(drag.data.EnumDAdvise().unwrap_err().code(), E_NOTIMPL);
    }
    drag.finish_modal().unwrap();
}

#[test]
fn inner_getdata_can_reenter_async_state_and_end_operation() {
    let _sta = Sta::new();
    let (drag, p) = wrapped();
    let seen = Rc::new(Cell::new(false));
    unsafe {
        drag.capability.StartOperation(None::<&IBindCtx>).unwrap();
    }
    let a = drag.capability.clone();
    let result = seen.clone();
    *p.callback.borrow_mut() = Some(Box::new(move || unsafe {
        result.set(a.InOperation().unwrap().as_bool());
        a.EndOperation(S_OK, None::<&IBindCtx>, 1).unwrap();
    }));
    let mut medium = unsafe { drag.data.GetData(&format()).unwrap() };
    unsafe {
        ReleaseStgMedium(&mut medium);
    }
    assert!(seen.get());
    assert!(!unsafe { drag.capability.InOperation().unwrap() }.as_bool());
    drop(drag);
    assert_eq!(p.drops.load(Ordering::SeqCst), 1);
}

fn owned_medium() -> (STGMEDIUM, Rc<Probe>, usize) {
    let (owner, probe) = mock();
    let owner = owner.cast::<IUnknown>().unwrap();
    let address = owner.as_raw() as usize;
    (
        STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0::default(),
            pUnkForRelease: ManuallyDrop::new(Some(owner)),
        },
        probe,
        address,
    )
}

#[test]
fn medium_ownership_and_frelease_belong_to_inner_and_caller() {
    let _sta = Sta::new();
    let (drag, p) = wrapped();
    let (medium, released, address) = owned_medium();
    *p.medium.borrow_mut() = Some(medium);
    let mut medium = unsafe { drag.data.GetData(&format()).unwrap() };
    assert_eq!(
        medium.pUnkForRelease.as_ref().unwrap().as_raw() as usize,
        address
    );
    assert_eq!(released.drops.load(Ordering::SeqCst), 0);
    unsafe {
        ReleaseStgMedium(&mut medium);
    }
    assert_eq!(released.drops.load(Ordering::SeqCst), 1);

    for (status, take, expected) in [(S_OK, false, 0), (S_OK, true, 1), (E_NOTIMPL, true, 0)] {
        p.status.set(status);
        let (mut medium, released, address) = owned_medium();
        // Keep the COM object alive while inspecting the unchanged input after transfer.
        let observer = medium.pUnkForRelease.as_ref().unwrap().clone();
        let original_tymed = medium.tymed;
        let original_hglobal = unsafe { medium.u.hGlobal };
        let result = unsafe { drag.data.SetData(&format(), &medium, take) };
        assert_eq!(result.is_ok(), status.is_ok());
        assert_eq!(medium.tymed, original_tymed);
        assert_eq!(unsafe { medium.u.hGlobal }, original_hglobal);
        assert_eq!(
            medium.pUnkForRelease.as_ref().unwrap().as_raw() as usize,
            address
        );
        assert_eq!(unsafe { inner(observer.as_raw()) }.refs.get(), 2 - expected);
        assert_eq!(released.drops.load(Ordering::SeqCst), 0);
        let call = p.calls.borrow().last().unwrap().0;
        assert_eq!(call, "set");
        assert_eq!(p.calls.borrow().last().unwrap().1[2], usize::from(take));
        if expected == 0 {
            unsafe {
                ReleaseStgMedium(&mut medium);
            }
        }
        assert_eq!(unsafe { inner(observer.as_raw()) }.refs.get(), 1);
        drop(observer);
        assert_eq!(released.drops.load(Ordering::SeqCst), 1);
    }
    drag.finish_modal().unwrap();
}

#[test]
fn shell_snapshot_survives_source_vec_pidls_and_item_array() {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    let _sta = Sta::new();
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("sub")).unwrap();
    let expected = [
        temp.path().join("日本語.txt"),
        temp.path().join("sub/other.txt"),
    ];
    for path in &expected {
        std::fs::write(path, b"disposable drag test").unwrap();
    }
    let drag = {
        let paths = expected.to_vec();
        // The production factory frees PIDLs and drops the local IShellItemArray.
        let (inner, failed) = crate::file_drag::shell_data_object_for_paths(&paths).unwrap();
        assert_eq!(failed, 0);
        DragDataObject::new(inner).unwrap()
    };
    let shell_idlist =
        unsafe { RegisterClipboardFormatW(windows::core::w!("Shell IDList Array")) } as u16;
    assert_ne!(shell_idlist, 0);
    let mut formats = vec![];
    let enumerator = unsafe { drag.data.EnumFormatEtc(DATADIR_GET.0 as u32).unwrap() };
    loop {
        let mut f = FORMATETC::default();
        let mut fetched = 0;
        unsafe {
            enumerator
                .Next(std::slice::from_mut(&mut f), Some(&mut fetched))
                .ok()
                .unwrap();
        }
        if fetched == 0 {
            break;
        }
        formats.push(f.cfFormat);
        if !f.ptd.is_null() {
            unsafe {
                CoTaskMemFree(Some(f.ptd.cast()));
            }
        }
    }
    assert!(formats.contains(&CF_HDROP.0));
    assert!(formats.contains(&shell_idlist));
    let mut medium = unsafe { drag.data.GetData(&format()).unwrap() };
    let hdrop = HDROP(unsafe { medium.u.hGlobal }.0);
    assert_eq!(unsafe { DragQueryFileW(hdrop, u32::MAX, None) }, 2);
    for (index, expected) in expected.iter().enumerate() {
        let len = unsafe { DragQueryFileW(hdrop, index as u32, None) };
        let mut wide = vec![0; len as usize + 1];
        assert_eq!(
            unsafe { DragQueryFileW(hdrop, index as u32, Some(&mut wide)) },
            len
        );
        let actual = std::path::PathBuf::from(String::from_utf16(&wide[..len as usize]).unwrap());
        assert!(crate::path_key::eq_keep_drive(&actual, expected));
    }
    unsafe {
        ReleaseStgMedium(&mut medium);
    }
    let idlist_format = FORMATETC {
        cfFormat: shell_idlist,
        ..format()
    };
    let mut medium = unsafe { drag.data.GetData(&idlist_format).unwrap() };
    let global = unsafe { medium.u.hGlobal };
    assert!(unsafe { GlobalSize(global) } >= 4 * std::mem::size_of::<u32>());
    let ptr = unsafe { GlobalLock(global) };
    assert!(!ptr.is_null());
    assert_eq!(unsafe { ptr.cast::<u32>().read_unaligned() }, 2);
    let _ = unsafe { GlobalUnlock(global) };
    unsafe {
        ReleaseStgMedium(&mut medium);
    }
    drag.finish_modal().unwrap();
}

// These ole32 entry points otherwise need the windows-rs Com_Marshal feature.
// They are test-only; no production dependency/feature changes are needed.
#[link(name = "ole32")]
unsafe extern "system" {
    fn CoMarshalInterThreadInterfaceInStream(
        iid: *const GUID,
        unknown: *mut c_void,
        stream: *mut *mut c_void,
    ) -> HRESULT;
    fn CoGetInterfaceAndReleaseStream(
        stream: *mut c_void,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> HRESULT;
}

fn pump_until(mut done: impl FnMut() -> bool, duration: std::time::Duration) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
        PeekMessageW, QS_ALLINPUT,
    };
    let deadline = std::time::Instant::now() + duration;
    while !done() {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        unsafe {
            MsgWaitForMultipleObjectsEx(None, 10, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&message);
            }
        }
    }
    true
}

#[test]
fn standard_marshaling_dispatches_getdata_and_end_to_owner_sta() {
    use std::time::Duration;
    use windows::Win32::System::Com::{
        COINIT_MULTITHREADED, CoCancelCall, CoDisableCallCancellation, CoDisconnectObject,
        CoEnableCallCancellation,
    };
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc};
    let _sta = Sta::new();
    let owner_thread = unsafe { GetCurrentThreadId() };
    let (drag, p) = wrapped();
    let global = unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, 16).unwrap() };
    *p.medium.borrow_mut() = Some(STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 { hGlobal: global },
        pUnkForRelease: ManuallyDrop::new(None),
    });
    unsafe {
        drag.capability.StartOperation(None::<&IBindCtx>).unwrap();
    }
    let callback_thread = Rc::new(Cell::new(0));
    let observed = callback_thread.clone();
    let a = drag.capability.clone();
    *p.callback.borrow_mut() = Some(Box::new(move || {
        observed.set(unsafe { GetCurrentThreadId() });
        assert!(unsafe { a.InOperation().unwrap() }.as_bool());
    }));
    let mut stream = null_mut();
    unsafe {
        CoMarshalInterThreadInterfaceInStream(&IDataObject::IID, drag.data.as_raw(), &mut stream)
            .ok()
            .unwrap();
    }
    // Only the marshaled stream is transferred. Neither source interface nor Rc crosses threads.
    let stream = stream as usize;
    let (client_tx, client_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || -> Result<()> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        struct ComCleanup;
        impl Drop for ComCleanup {
            fn drop(&mut self) {
                unsafe {
                    let _ = CoDisableCallCancellation(None);
                    CoUninitialize();
                }
            }
        }
        let _cleanup = ComCleanup;
        client_tx.send(unsafe { GetCurrentThreadId() }).unwrap();
        unsafe {
            CoEnableCallCancellation(None)?;
            let mut raw = null_mut();
            CoGetInterfaceAndReleaseStream(stream as *mut c_void, &IDataObject::IID, &mut raw)
                .ok()?;
            let data = IDataObject::from_raw(raw);
            let mut medium = data.GetData(&format())?;
            ReleaseStgMedium(&mut medium);
            let capability: IDataObjectAsyncCapability = data.cast()?;
            capability.EndOperation(S_OK, None::<&IBindCtx>, 1)?;
        }
        Ok(())
    });
    let completed = pump_until(|| worker.is_finished(), Duration::from_secs(10));
    if !completed {
        // Cancel outstanding RPC and sever the marshaled connection before releasing the
        // fixture. Cleanup is bounded too; nextest owns the failing test process lifetime.
        if let Ok(thread) = client_rx.try_recv() {
            unsafe {
                let _ = CoCancelCall(thread, 0);
            }
        }
        unsafe {
            let _ = CoDisconnectObject(&drag.data, None);
        }
        pump_until(|| worker.is_finished(), Duration::from_secs(2));
        if unsafe { drag.capability.InOperation().unwrap() }.as_bool() {
            unsafe {
                drag.capability
                    .EndOperation(E_NOTIMPL, None::<&IBindCtx>, 0)
                    .unwrap();
            }
        }
    }
    assert!(
        completed,
        "standard COM marshaling did not complete before deadline"
    );
    worker.join().unwrap().unwrap();
    assert_ne!(client_rx.recv().unwrap(), owner_thread);
    assert_eq!(p.get_thread.get(), owner_thread);
    assert_eq!(callback_thread.get(), owner_thread);
    assert!(!unsafe { drag.capability.InOperation().unwrap() }.as_bool());
    drop(drag);
    assert_eq!(p.drops.load(Ordering::SeqCst), 1);
}
