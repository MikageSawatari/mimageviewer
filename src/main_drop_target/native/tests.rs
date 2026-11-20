use super::*;
use std::cell::Cell;
use windows::Win32::Foundation::{DV_E_FORMATETC, E_NOTIMPL, S_OK};
use windows::Win32::System::Com::{
    IAdviseSink, IAgileObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA,
};
use windows::core::{BOOL, HRESULT, Interface};

#[derive(Default)]
struct Observed {
    queries: Cell<usize>,
    reads: Cell<usize>,
    writes: RefCell<Vec<(u16, i32)>>,
    visual: RefCell<Vec<(&'static str, u32)>>,
}

struct FakeVisuals {
    observed: Rc<Observed>,
    shared: SharedMainDrop,
}
impl FakeVisuals {
    fn record(&self, event: &'static str, effect: DROPEFFECT) {
        let _reentry = self.shared.borrow_mut();
        self.observed.visual.borrow_mut().push((event, effect.0));
    }
}
impl Visuals for FakeVisuals {
    fn enter(&self, _: HWND, _: &IDataObject, _: &POINT, effect: DROPEFFECT) {
        self.record("enter", effect);
    }
    fn over(&self, _: &POINT, effect: DROPEFFECT) {
        self.record("over", effect);
    }
    fn leave(&self) {
        self.record("leave", DROPEFFECT_NONE);
    }
    fn dropped(&self, _: &IDataObject, _: &POINT, effect: DROPEFFECT) {
        self.record("drop", effect);
    }
}

#[windows::core::implement(IDataObject, Agile = false)]
struct FakeDataObject {
    shared: SharedMainDrop,
    observed: Rc<Observed>,
    description_format: u16,
    fail_paths: bool,
}

fn blob(bytes: &[u8]) -> WinResult<STGMEDIUM> {
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }?;
    let medium = Medium(STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 { hGlobal: memory },
        pUnkForRelease: ManuallyDrop::new(None),
    });
    let ptr = unsafe { GlobalLock(memory) };
    if ptr.is_null() {
        return Err(windows::core::Error::from_win32());
    }
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast::<u8>(), bytes.len()) };
    let _ = unsafe { GlobalUnlock(memory) };
    let medium = ManuallyDrop::new(medium);
    Ok(unsafe { std::ptr::read(&medium.0) })
}

impl IDataObject_Impl for FakeDataObject_Impl {
    fn GetData(&self, format: *const FORMATETC) -> WinResult<STGMEDIUM> {
        let format = unsafe { &*format };
        let _reentry = self.shared.borrow_mut();
        if format.cfFormat == CF_HDROP.0 {
            self.observed.reads.set(self.observed.reads.get() + 1);
            if self.fail_paths {
                return Err(E_NOTIMPL.into());
            }
            // DROPFILES is a 20-byte header; use UTF-16 CF_HDROP with a double NUL.
            let mut bytes = vec![0; 20];
            bytes[..4].copy_from_slice(&20u32.to_le_bytes());
            bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
            for unit in r"C:\external\visible.txt".encode_utf16().chain([0, 0]) {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            blob(&bytes)
        } else {
            // DragWindow without an actual HWND: no window/message/UI side effects.
            blob(&0u32.to_le_bytes())
        }
    }
    fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> WinResult<()> {
        Err(E_NOTIMPL.into())
    }
    fn QueryGetData(&self, format: *const FORMATETC) -> HRESULT {
        let _reentry = self.shared.borrow_mut();
        self.observed.queries.set(self.observed.queries.get() + 1);
        if unsafe { (*format).cfFormat } == CF_HDROP.0 {
            S_OK
        } else {
            DV_E_FORMATETC
        }
    }
    fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, _: *mut FORMATETC) -> HRESULT {
        E_NOTIMPL
    }
    fn SetData(
        &self,
        format: *const FORMATETC,
        medium: *const STGMEDIUM,
        release: BOOL,
    ) -> WinResult<()> {
        let _reentry = self.shared.borrow_mut();
        assert!(!release.as_bool(), "target owns its FALSE medium");
        let format = unsafe { &*format };
        assert_eq!(
            format.cfFormat, self.description_format,
            "no performed/logical/preferred effects may be written"
        );
        let memory = unsafe { (*medium).u.hGlobal };
        let ptr = unsafe { GlobalLock(memory) };
        assert!(!ptr.is_null());
        let image_type = unsafe { ptr.cast::<i32>().read_unaligned() };
        let _ = unsafe { GlobalUnlock(memory) };
        self.observed
            .writes
            .borrow_mut()
            .push((format.cfFormat, image_type));
        Ok(())
    }
    fn EnumFormatEtc(&self, _: u32) -> WinResult<IEnumFORMATETC> {
        Err(E_NOTIMPL.into())
    }
    fn DAdvise(&self, _: *const FORMATETC, _: u32, _: Ref<'_, IAdviseSink>) -> WinResult<u32> {
        Err(E_NOTIMPL.into())
    }
    fn DUnadvise(&self, _: u32) -> WinResult<()> {
        Err(E_NOTIMPL.into())
    }
    fn EnumDAdvise(&self) -> WinResult<IEnumSTATDATA> {
        Err(E_NOTIMPL.into())
    }
}

struct Fixture {
    shared: SharedMainDrop,
    target: IDropTarget,
    data: IDataObject,
    hit: Rc<RefCell<Option<Hit>>>,
    observed: Rc<Observed>,
}

fn fixture(internal: Option<InternalDrag>, fail_paths: bool) -> Fixture {
    let shared = Rc::new(RefCell::new(MainDropShared {
        admission: Ok(()),
        internal,
        ..Default::default()
    }));
    let observed = Rc::new(Observed::default());
    let hit = Rc::new(RefCell::new(Some(Hit::TreeRow(r"C:\dst".into()))));
    let locator = hit.clone();
    let target = MainDropTarget {
        hwnd: HWND::default(),
        ctx: egui::Context::default(),
        shared: shared.clone(),
        locate: Box::new(move |_| locator.borrow().clone()),
        helper: Rc::new(FakeVisuals {
            shared: shared.clone(),
            observed: observed.clone(),
        }),
        session: RefCell::new(None),
    }
    .into();
    let data = FakeDataObject {
        shared: shared.clone(),
        observed: observed.clone(),
        description_format: unsafe { RegisterClipboardFormatW(CFSTR_DROPDESCRIPTION) } as u16,
        fail_paths,
    }
    .into();
    Fixture {
        shared,
        target,
        data,
        hit,
        observed,
    }
}

fn point() -> POINTL {
    POINTL { x: 100, y: 50 }
}

#[test]
fn main_drop_com_external_shift_drop_returns_copy_and_only_writes_drop_description() {
    let fixture = fixture(None, false);
    assert!(fixture.target.cast::<IAgileObject>().is_err());
    let mut effect = DROPEFFECT_COPY;
    unsafe {
        fixture.target.DragEnter(
            &fixture.data,
            MODIFIERKEYS_FLAGS(SHIFT),
            point(),
            &mut effect,
        )
    }
    .unwrap();
    assert_eq!(effect, DROPEFFECT_COPY);
    unsafe {
        fixture
            .target
            .DragOver(MODIFIERKEYS_FLAGS(SHIFT), point(), &mut effect)
    }
    .unwrap();
    unsafe {
        fixture.target.Drop(
            &fixture.data,
            MODIFIERKEYS_FLAGS(SHIFT),
            point(),
            &mut effect,
        )
    }
    .unwrap();
    assert_eq!(effect, DROPEFFECT_COPY);
    let mut shared = fixture.shared.borrow_mut();
    let DropRequest::TreeTransfer(request) = shared.drops.pop().unwrap() else {
        panic!("tree transfer expected")
    };
    assert_eq!(request.operation, ShellTransferOperation::Copy);
    assert!(!request.default_move_needs_volume_check);
    assert_eq!(
        request.sources,
        vec![PathBuf::from(r"C:\external\visible.txt")]
    );
    assert_eq!(fixture.observed.reads.get(), 1);
    assert_eq!(
        fixture
            .observed
            .writes
            .borrow()
            .iter()
            .map(|(_, ty)| *ty)
            .collect::<Vec<_>>(),
        vec![DROPIMAGE_COPY.0, DROPIMAGE_INVALID.0]
    );
    assert_eq!(
        fixture.observed.visual.borrow().as_slice(),
        &[
            ("enter", DROPEFFECT_COPY.0),
            ("over", DROPEFFECT_COPY.0),
            ("drop", DROPEFFECT_COPY.0)
        ]
    );
}

#[test]
fn main_drop_com_internal_keeps_full_payload_and_rejects_grid_and_partial_source() {
    let paths = vec![
        PathBuf::from(r"C:\src\first"),
        PathBuf::from(r"C:\src\second"),
    ];
    let fixture = fixture(
        Some(InternalDrag {
            paths: paths.clone(),
            excluded_virtual: false,
            failed_paths: 0,
        }),
        false,
    );
    let mut effect = DROPEFFECT_COPY;
    unsafe {
        fixture
            .target
            .DragEnter(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut effect)
    }
    .unwrap();
    *fixture.hit.borrow_mut() = Some(Hit::ExistingDropArea);
    unsafe {
        fixture
            .target
            .DragOver(MODIFIERKEYS_FLAGS(0), point(), &mut effect)
    }
    .unwrap();
    assert_eq!(effect, DROPEFFECT_NONE);
    *fixture.hit.borrow_mut() = Some(Hit::TreeRow(r"C:\dst".into()));
    unsafe {
        fixture
            .target
            .Drop(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut effect)
    }
    .unwrap();
    assert_eq!(effect, DROPEFFECT_COPY);
    assert_eq!(fixture.observed.queries.get(), 0);
    assert_eq!(fixture.observed.reads.get(), 0);
    let request = fixture.shared.borrow_mut().drops.pop().unwrap();
    let DropRequest::TreeTransfer(request) = request else {
        panic!("tree request expected")
    };
    assert_eq!(request.sources, paths);
    assert_eq!(request.operation, ShellTransferOperation::Move);
    assert!(request.default_move_needs_volume_check);
    fixture
        .shared
        .borrow_mut()
        .internal
        .as_mut()
        .unwrap()
        .failed_paths = 1;
    effect = DROPEFFECT_COPY;
    unsafe {
        fixture
            .target
            .DragEnter(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut effect)
    }
    .unwrap();
    assert_eq!(effect, DROPEFFECT_NONE);
    unsafe {
        fixture
            .target
            .Drop(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut effect)
    }
    .unwrap();
    assert_eq!(effect, DROPEFFECT_NONE);
    assert!(fixture.shared.borrow().drops.is_empty());
}

#[test]
fn main_drop_com_leaving_row_and_dragleave_clear_description_once_and_keep_tree_blank_rejected() {
    let fixture = fixture(None, false);
    let mut effect = DROPEFFECT_COPY;
    unsafe {
        fixture
            .target
            .DragEnter(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut effect)
    }
    .unwrap();
    *fixture.hit.borrow_mut() = Some(Hit::TreeNonTarget);
    unsafe {
        fixture
            .target
            .DragOver(MODIFIERKEYS_FLAGS(0), point(), &mut effect)
    }
    .unwrap();
    assert_eq!(effect, DROPEFFECT_NONE);
    unsafe { fixture.target.DragLeave() }.unwrap();
    assert!(fixture.shared.borrow().drops.is_empty());
    assert_eq!(
        fixture
            .observed
            .writes
            .borrow()
            .iter()
            .map(|(_, ty)| *ty)
            .collect::<Vec<_>>(),
        vec![DROPIMAGE_COPY.0, DROPIMAGE_INVALID.0]
    );
    assert_eq!(fixture.observed.visual.borrow().last(), Some(&("leave", 0)));
}

#[test]
fn main_drop_com_failed_path_read_returns_none_and_queues_notification_without_transfer() {
    let fixture = fixture(None, true);
    let mut effect = DROPEFFECT_COPY;
    unsafe {
        fixture
            .target
            .DragEnter(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut effect)
    }
    .unwrap();
    unsafe {
        fixture
            .target
            .Drop(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut effect)
    }
    .unwrap();
    assert_eq!(effect, DROPEFFECT_NONE);
    assert!(matches!(
        fixture.shared.borrow().drops.as_slice(),
        [DropRequest::Failed(_)]
    ));
    assert_eq!(fixture.observed.visual.borrow().last(), Some(&("drop", 0)));
}
