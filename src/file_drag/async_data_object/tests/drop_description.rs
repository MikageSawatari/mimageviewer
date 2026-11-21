//! §1.836: target descriptions belong to the outer drag object, including Explorer writes.
use super::*;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc};
use windows::Win32::UI::Shell::{
    CFSTR_DROPDESCRIPTION, DROPDESCRIPTION, DROPIMAGE_COPY, DROPIMAGE_INVALID, DROPIMAGE_MOVE,
    DROPIMAGETYPE,
};

struct OwnedMedium(STGMEDIUM);
impl Drop for OwnedMedium {
    fn drop(&mut self) {
        unsafe { ReleaseStgMedium(&mut self.0) };
    }
}

fn description(image: DROPIMAGETYPE, name: &str) -> DROPDESCRIPTION {
    let mut insert = [0u16; 260];
    for (slot, unit) in insert.iter_mut().zip(name.encode_utf16()) {
        *slot = unit;
    }
    DROPDESCRIPTION {
        r#type: image,
        szInsert: insert,
        ..Default::default()
    }
}

fn description_medium(value: DROPDESCRIPTION) -> STGMEDIUM {
    let global =
        unsafe { GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of::<DROPDESCRIPTION>()) }.unwrap();
    let ptr = unsafe { GlobalLock(global) };
    assert!(!ptr.is_null());
    unsafe { ptr.cast::<DROPDESCRIPTION>().write_unaligned(value) };
    let _ = unsafe { GlobalUnlock(global) };
    STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 { hGlobal: global },
        pUnkForRelease: ManuallyDrop::new(None),
    }
}

fn tracked_description_medium(value: DROPDESCRIPTION) -> (STGMEDIUM, Rc<Probe>) {
    let medium = description_medium(value);
    let global = unsafe { medium.u.hGlobal };
    let (owner, probe) = mock_owning_medium(Some(medium));
    let unknown = owner.cast::<IUnknown>().unwrap();
    drop(owner);
    (
        STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: global },
            pUnkForRelease: ManuallyDrop::new(Some(unknown)),
        },
        probe,
    )
}

fn read_description(medium: &STGMEDIUM) -> DROPDESCRIPTION {
    assert_eq!(medium.tymed, TYMED_HGLOBAL.0 as u32);
    let global = unsafe { medium.u.hGlobal };
    assert!(unsafe { GlobalSize(global) } >= std::mem::size_of::<DROPDESCRIPTION>());
    let ptr = unsafe { GlobalLock(global) };
    assert!(!ptr.is_null());
    let value = unsafe { ptr.cast::<DROPDESCRIPTION>().read_unaligned() };
    let _ = unsafe { GlobalUnlock(global) };
    value
}

fn assert_description(actual: DROPDESCRIPTION, expected: DROPDESCRIPTION) {
    // DROPDESCRIPTION is packed: never take a reference to one of its fields.
    let actual = (actual.r#type, actual.szMessage, actual.szInsert);
    let expected = (expected.r#type, expected.szMessage, expected.szInsert);
    assert_eq!(actual, expected);
}

#[test]
fn drag_data_object_drop_description_before_set_delegates_get_query_and_enum() {
    let _sta = Sta::new();
    let (drag, p) = wrapped();
    let f = effect_format(CFSTR_DROPDESCRIPTION);
    for status in [S_OK, S_FALSE, E_NOTIMPL] {
        p.status.set(status);
        assert_eq!(unsafe { drag.data.QueryGetData(&f) }, status);
        let result = unsafe { drag.data.GetData(&f) };
        if status.is_ok() {
            drop(OwnedMedium(result.unwrap()));
        } else {
            assert_eq!(result.err().unwrap().code(), status);
        }
    }
    p.status.set(S_OK);
    assert_eq!(
        unsafe { drag.data.EnumFormatEtc(DATADIR_GET.0 as u32).unwrap() },
        p.formats
    );
    assert_eq!(
        p.calls
            .borrow()
            .iter()
            .filter(|call| call.0 == "get")
            .count(),
        3
    );
    assert_eq!(
        p.calls
            .borrow()
            .iter()
            .filter(|call| call.0 == "query")
            .count(),
        3
    );
    drag.finish_modal().unwrap();
}

#[test]
fn drag_data_object_drop_description_release_true_copies_and_releases_once() {
    let _sta = Sta::new();
    let (drag, p) = wrapped();
    p.status.set(E_NOTIMPL); // An external target must not depend on inner SetData support.
    let f = effect_format(CFSTR_DROPDESCRIPTION);
    let expected = description(DROPIMAGE_MOVE, "Explorer destination");
    let (medium, owner) = tracked_description_medium(expected);
    unsafe { drag.data.SetData(&f, &medium, true).unwrap() };
    assert_eq!(owner.drops.load(Ordering::SeqCst), 1);
    assert!(p.calls.borrow().is_empty());
    let output = OwnedMedium(unsafe { drag.data.GetData(&f).unwrap() });
    assert_description(read_description(&output.0), expected);
    drop(output);
    drag.finish_modal().unwrap();
    drop(drag);
    assert_eq!(owner.drops.load(Ordering::SeqCst), 1);
    assert_eq!(p.drops.load(Ordering::SeqCst), 1);
}

#[test]
fn drag_data_object_drop_description_release_false_fresh_get_and_replace() {
    let _sta = Sta::new();
    let (drag, p) = wrapped();
    p.status.set(E_NOTIMPL);
    let f = effect_format(CFSTR_DROPDESCRIPTION);
    let first = description(DROPIMAGE_MOVE, "first");
    let (mut medium, owner) = tracked_description_medium(first);
    unsafe { drag.data.SetData(&f, &medium, false).unwrap() };
    assert_eq!(owner.drops.load(Ordering::SeqCst), 0);
    assert_description(read_description(&medium), first);
    unsafe { ReleaseStgMedium(&mut medium) };
    assert_eq!(owner.drops.load(Ordering::SeqCst), 1);

    let a = OwnedMedium(unsafe { drag.data.GetData(&f).unwrap() });
    let b = OwnedMedium(unsafe { drag.data.GetData(&f).unwrap() });
    assert_ne!(unsafe { a.0.u.hGlobal }, unsafe { b.0.u.hGlobal });
    assert_eq!(unsafe { drag.data.QueryGetData(&f) }, S_OK);
    for next in [
        description(DROPIMAGE_COPY, "second"),
        description(DROPIMAGE_INVALID, ""),
    ] {
        let input = OwnedMedium(description_medium(next));
        unsafe { drag.data.SetData(&f, &input.0, false).unwrap() };
        let output = OwnedMedium(unsafe { drag.data.GetData(&f).unwrap() });
        assert_description(read_description(&output.0), next);
    }
    // Existing results retain their own snapshots even after replacement.
    assert_description(read_description(&a.0), first);
    assert_description(read_description(&b.0), first);
    assert!(p.calls.borrow().is_empty());
    drag.finish_modal().unwrap();
    drop(drag);
    assert_eq!(owner.drops.load(Ordering::SeqCst), 1);
}

#[test]
fn drag_data_object_drop_description_preserves_performed_completion_and_ownership() {
    let _sta = Sta::new();
    for status in [S_OK, E_NOTIMPL] {
        for take in [false, true] {
            let (drag, p) = wrapped();
            unsafe { drag.capability.StartOperation(None::<&IBindCtx>).unwrap() };
            let f = effect_format(CFSTR_DROPDESCRIPTION);
            let value = description(DROPIMAGE_COPY, "outbound");
            let input = OwnedMedium(description_medium(value));
            unsafe { drag.data.SetData(&f, &input.0, false).unwrap() };
            assert!(unsafe { drag.capability.InOperation().unwrap() }.as_bool());
            assert!(p.calls.borrow().is_empty());
            p.status.set(status);
            let performed = effect_format(CFSTR_PERFORMEDDROPEFFECT);
            let (mut medium, owner) = effect_medium(1);
            let result = unsafe { drag.data.SetData(&performed, &medium, take) };
            assert_eq!(
                result.err().map(|e| e.code()),
                status.is_err().then_some(status)
            );
            assert!(!unsafe { drag.capability.InOperation().unwrap() }.as_bool());
            assert_eq!(p.calls.borrow().len(), 1);
            assert_eq!(p.calls.borrow()[0].0, "set");
            let transferred = status.is_ok() && take;
            assert_eq!(owner.drops.load(Ordering::SeqCst), usize::from(transferred));
            if !transferred {
                unsafe { ReleaseStgMedium(&mut medium) };
            }
            assert_eq!(owner.drops.load(Ordering::SeqCst), 1);
            let output = OwnedMedium(unsafe { drag.data.GetData(&f).unwrap() });
            assert_description(read_description(&output.0), value);
            drag.finish_modal().unwrap();
            drop(drag);
            assert_eq!(p.drops.load(Ordering::SeqCst), 1);
        }
    }
}

#[test]
fn drag_data_object_drop_description_invalid_input_retains_value_and_caller_ownership() {
    let _sta = Sta::new();
    let (drag, p) = wrapped();
    let f = effect_format(CFSTR_DROPDESCRIPTION);
    let expected = description(DROPIMAGE_MOVE, "retained");
    let initial = OwnedMedium(description_medium(expected));
    unsafe { drag.data.SetData(&f, &initial.0, false).unwrap() };
    let (mut input, owner) = tracked_description_medium(description(DROPIMAGE_COPY, "bad"));
    for (bad, error) in [
        (FORMATETC { dwAspect: 0, ..f }, DV_E_DVASPECT),
        (FORMATETC { lindex: 0, ..f }, DV_E_LINDEX),
        (FORMATETC { tymed: 0, ..f }, DV_E_TYMED),
    ] {
        assert_eq!(
            unsafe { drag.data.SetData(&bad, &input, true) }
                .unwrap_err()
                .code(),
            error
        );
        assert_eq!(owner.drops.load(Ordering::SeqCst), 0);
        assert_eq!(unsafe { drag.data.QueryGetData(&bad) }, error);
        assert_eq!(
            unsafe { drag.data.GetData(&bad) }.err().unwrap().code(),
            error
        );
    }
    input.tymed = 0;
    assert_eq!(
        unsafe { drag.data.SetData(&f, &input, true) }
            .unwrap_err()
            .code(),
        DV_E_TYMED
    );
    input.tymed = TYMED_HGLOBAL.0 as u32;
    assert_eq!(owner.drops.load(Ordering::SeqCst), 0);
    unsafe { ReleaseStgMedium(&mut input) };
    assert_eq!(owner.drops.load(Ordering::SeqCst), 1);
    let (mut short, short_owner) = effect_medium(0);
    assert_eq!(
        unsafe { drag.data.SetData(&f, &short, true) }
            .unwrap_err()
            .code(),
        DV_E_STGMEDIUM
    );
    assert_eq!(short_owner.drops.load(Ordering::SeqCst), 0);
    unsafe { ReleaseStgMedium(&mut short) };
    assert_eq!(short_owner.drops.load(Ordering::SeqCst), 1);
    let empty = STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        ..Default::default()
    };
    assert_eq!(
        unsafe { drag.data.SetData(&f, &empty, true) }
            .unwrap_err()
            .code(),
        DV_E_STGMEDIUM
    );
    assert_eq!(
        unsafe { drag.data.SetData(&f, std::ptr::null(), true) }
            .unwrap_err()
            .code(),
        E_INVALIDARG
    );
    let output = OwnedMedium(unsafe { drag.data.GetData(&f).unwrap() });
    assert_description(read_description(&output.0), expected);
    assert!(p.calls.borrow().is_empty());
    drag.finish_modal().unwrap();
}

#[test]
fn drag_data_object_drop_description_is_scoped_to_its_drag() {
    let _sta = Sta::new();
    let (a, pa) = wrapped();
    let (b, pb) = wrapped();
    pb.status.set(E_NOTIMPL);
    let f = effect_format(CFSTR_DROPDESCRIPTION);
    let input = OwnedMedium(description_medium(description(DROPIMAGE_COPY, "only a")));
    unsafe { a.data.SetData(&f, &input.0, false).unwrap() };
    assert_eq!(unsafe { b.data.QueryGetData(&f) }, E_NOTIMPL);
    assert_eq!(
        unsafe { b.data.GetData(&f) }.err().unwrap().code(),
        E_NOTIMPL
    );
    assert!(pa.calls.borrow().is_empty());
    assert_eq!(pb.calls.borrow().len(), 2);
    a.finish_modal().unwrap();
    b.finish_modal().unwrap();
}
