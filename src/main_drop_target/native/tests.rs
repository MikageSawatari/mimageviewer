use super::*;
use std::cell::Cell;
use windows::Win32::Foundation::{DV_E_FORMATETC, E_NOTIMPL, S_OK};
use windows::Win32::System::Com::{
    IAdviseSink, IAgileObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA,
};
use windows::core::{BOOL, HRESULT, Interface};

fn tree_hit(path: PathBuf) -> Hit {
    Hit::Folder {
        path,
        place: DropPlace::Tree,
    }
}

#[test]
fn hover_860_own_outer_description_true_round_trip_and_replace() {
    let temp = tempfile::tempdir().unwrap();
    let paths = [
        temp.path().join("first.txt"),
        temp.path().join("日本語.txt"),
    ];
    for path in &paths {
        std::fs::write(path, b"disposable drop description test").unwrap();
    }
    let move_dir = temp.path().join("dst2");
    let copy_dir = temp.path().join("コピー先");
    for dir in [&move_dir, &copy_dir] {
        std::fs::create_dir(dir).unwrap();
    }

    crate::file_drag::with_shell_drag_data_object(&paths, |data| {
        // SHDoDragDrop and IDropTargetHelper receive this outer object, not its inner
        // Shell IDataObject. Exercise the real target writer and read the same object.
        let target_data = OleData(data.clone());
        let cf = unsafe { RegisterClipboardFormatW(CFSTR_DROPDESCRIPTION) } as u16;
        assert_ne!(cf, 0);
        for (description, image, message, insert) in [
            (
                Description::Transfer(ShellTransferOperation::Move, move_dir.clone()),
                DROPIMAGE_MOVE,
                "%1 へ移動",
                "dst2",
            ),
            (
                Description::Transfer(ShellTransferOperation::Copy, copy_dir.clone()),
                DROPIMAGE_COPY,
                "%1 へコピー",
                "コピー先",
            ),
            (Description::Invalid, DROPIMAGE_INVALID, "", ""),
            (
                Description::Transfer(ShellTransferOperation::Move, copy_dir.clone()),
                DROPIMAGE_MOVE,
                "%1 へ移動",
                "コピー先",
            ),
        ] {
            let result = target_data.set_description(&description);
            assert!(
                result.is_ok(),
                "production SetData(DropDescription) must accept {description:?}: {result:?}"
            );
            let medium = Medium(
                unsafe { data.GetData(&format(cf)) }.unwrap_or_else(|error| {
                    panic!("GetData(DropDescription) after {description:?}: {error}")
                }),
            );
            assert_eq!(medium.0.tymed, TYMED_HGLOBAL.0 as u32);
            let global = unsafe { medium.0.u.hGlobal };
            assert!(unsafe { GlobalSize(global) } >= std::mem::size_of::<DROPDESCRIPTION>());
            let ptr = unsafe { GlobalLock(global) };
            assert!(!ptr.is_null());
            let actual = unsafe { ptr.cast::<DROPDESCRIPTION>().read_unaligned() };
            let _ = unsafe { GlobalUnlock(global) };
            // DROPDESCRIPTION is packed: copy fields by value, never borrow them.
            let (actual_type, actual_message, actual_insert) =
                (actual.r#type, actual.szMessage, actual.szInsert);
            assert_eq!(actual_type, image, "{description:?}");
            // Compare every UTF-16 unit, including NUL termination and cleared tails.
            // Invalid must clear the old message/insert; subsequent writes replace it.
            for (actual, expected) in [(actual_message, message), (actual_insert, insert)] {
                let mut wide = [0u16; 260];
                let units: Vec<u16> = expected.encode_utf16().collect();
                wide[..units.len()].copy_from_slice(&units);
                assert_eq!(actual, wide, "{description:?}: expected {expected:?}");
            }
        }
    });
}

#[test]
fn main_drop_nodrop_shell_rejected_places_and_dragleave_clear_transfer_image() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source-folder");
    let destination = temp.path().join("dst2");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(source.join("disposable.txt"), b"test").unwrap();
    crate::file_drag::with_shell_drag_data_object(std::slice::from_ref(&source), |data| {
        let mut fixture = fixture(
            Some(InternalDrag {
                paths: vec![source.clone()],
                excluded_virtual: false,
                failed_paths: 0,
            }),
            false,
        );
        fixture.data = data.clone();
        *fixture.hit.borrow_mut() = Some(tree_hit(destination.clone()));
        let image = || {
            let cf = unsafe { RegisterClipboardFormatW(CFSTR_DROPDESCRIPTION) } as u16;
            let medium = Medium(unsafe { data.GetData(&format(cf)).unwrap() });
            let global = unsafe { medium.0.u.hGlobal };
            let ptr = unsafe { GlobalLock(global) };
            assert!(!ptr.is_null());
            let value = unsafe { ptr.cast::<DROPDESCRIPTION>().read_unaligned() };
            let _ = unsafe { GlobalUnlock(global) };
            value.r#type
        };
        let mut output = DROPEFFECT_COPY;
        unsafe {
            fixture
                .target
                .DragEnter(data, MODIFIERKEYS_FLAGS(0), point(), &mut output)
        }
        .unwrap();
        assert_eq!(output, DROPEFFECT_COPY);
        assert_eq!(image(), DROPIMAGE_MOVE);
        for (hit, keys, expected_effect, expected_image) in [
            (
                Some(tree_hit(destination.clone())),
                CONTROL,
                DROPEFFECT_COPY,
                DROPIMAGE_COPY,
            ),
            // Internal grid blank / same list: explicitly request the no-drop image.
            (
                Some(Hit::ExistingDropArea),
                0,
                DROPEFFECT_NONE,
                DROPIMAGE_NONE,
            ),
            // Dropping the folder onto itself or into its current parent is rejected.
            (
                Some(tree_hit(source.clone())),
                0,
                DROPEFFECT_NONE,
                DROPIMAGE_NONE,
            ),
            (
                Some(tree_hit(temp.path().to_path_buf())),
                0,
                DROPEFFECT_NONE,
                DROPIMAGE_NONE,
            ),
            (Some(Hit::TreeNonTarget), 0, DROPEFFECT_NONE, DROPIMAGE_NONE),
            (None, 0, DROPEFFECT_NONE, DROPIMAGE_NONE),
            (
                Some(tree_hit(destination.clone())),
                SHIFT,
                DROPEFFECT_COPY,
                DROPIMAGE_MOVE,
            ),
        ] {
            *fixture.hit.borrow_mut() = hit;
            unsafe {
                fixture
                    .target
                    .DragOver(MODIFIERKEYS_FLAGS(keys), point(), &mut output)
            }
            .unwrap();
            assert_eq!(output, expected_effect);
            assert_eq!(image(), expected_image);
        }
        unsafe { fixture.target.DragLeave() }.unwrap();
        assert_eq!(image(), DROPIMAGE_INVALID);
        assert!(fixture.shared.borrow().drops.is_empty());
        assert_eq!(fixture.observed.visual.borrow().last(), Some(&("leave", 0)));
    });
}

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
    has_paths: bool,
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
        if self.has_paths && unsafe { (*format).cfFormat } == CF_HDROP.0 {
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
        assert!(
            release.as_bool(),
            "target transfers its TRUE medium on success"
        );
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
        drop(_reentry);
        // Match IDataObject::SetData: consume only on success, once, after reading.
        let mut owned = unsafe { medium.read() };
        unsafe { ReleaseStgMedium(&mut owned) };
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
    ctx: egui::Context,
}

fn fixture(internal: Option<InternalDrag>, fail_paths: bool) -> Fixture {
    fixture_with_paths(internal, fail_paths, true)
}

fn fixture_with_paths(
    internal: Option<InternalDrag>,
    fail_paths: bool,
    has_paths: bool,
) -> Fixture {
    let shared = Rc::new(RefCell::new(MainDropShared {
        admission: Ok(()),
        internal,
        ..Default::default()
    }));
    let observed = Rc::new(Observed::default());
    let hit = Rc::new(RefCell::new(Some(tree_hit(r"C:\dst".into()))));
    let locator = hit.clone();
    let ctx = egui::Context::default();
    let target = MainDropTarget {
        hwnd: HWND::default(),
        ctx: ctx.clone(),
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
        has_paths,
    }
    .into();
    Fixture {
        shared,
        target,
        data,
        hit,
        observed,
        ctx,
    }
}

fn point() -> POINTL {
    POINTL { x: 100, y: 50 }
}

#[test]
fn main_drop_nodrop_rejected_hits_write_none_on_enter_over_and_drop() {
    let source = InternalDrag {
        paths: vec![r"C:\src\a".into()],
        excluded_virtual: false,
        failed_paths: 0,
    };
    for (internal, has_paths, hit) in [
        (Some(source.clone()), true, None),
        (Some(source.clone()), true, Some(Hit::TreeNonTarget)),
        (Some(source.clone()), true, Some(Hit::GridNonTarget)),
        (Some(source), true, Some(Hit::ExistingDropArea)),
        (None, false, None),
        (None, false, Some(Hit::TreeNonTarget)),
        (None, false, Some(Hit::GridNonTarget)),
        (None, false, Some(Hit::ExistingDropArea)),
        (None, false, Some(tree_hit(r"C:\dst".into()))),
        (None, true, Some(Hit::TreeNonTarget)),
        (None, true, Some(Hit::GridNonTarget)),
    ] {
        let fixture = fixture_with_paths(internal, false, has_paths);
        *fixture.hit.borrow_mut() = hit.clone();
        let mut output = DROPEFFECT_COPY;
        unsafe {
            fixture
                .target
                .DragEnter(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut output)
        }
        .unwrap();
        assert_eq!(output, DROPEFFECT_NONE, "{hit:?}");
        assert_eq!(
            fixture.observed.writes.borrow().last().unwrap().1,
            DROPIMAGE_NONE.0
        );
        unsafe {
            fixture
                .target
                .DragOver(MODIFIERKEYS_FLAGS(0), point(), &mut output)
        }
        .unwrap();
        assert_eq!(output, DROPEFFECT_NONE, "{hit:?}");
        assert_eq!(
            fixture.observed.writes.borrow().len(),
            1,
            "unchanged description"
        );
        unsafe {
            fixture
                .target
                .Drop(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut output)
        }
        .unwrap();
        assert_eq!(output, DROPEFFECT_NONE, "{hit:?}");
        assert!(fixture.shared.borrow().drops.is_empty());
        assert_eq!(fixture.observed.reads.get(), 0);
        assert_eq!(
            fixture
                .observed
                .writes
                .borrow()
                .iter()
                .map(|(_, image)| *image)
                .collect::<Vec<_>>(),
            vec![DROPIMAGE_NONE.0, DROPIMAGE_INVALID.0]
        ); // finish_session hands back the image.
        assert_eq!(
            fixture.observed.visual.borrow().as_slice(),
            &[("enter", 0), ("over", 0), ("drop", 0)]
        );
    }
}

#[test]
fn main_drop_nodrop_accepted_external_list_keeps_default_and_copy_effect() {
    let fixture = fixture(None, false);
    *fixture.hit.borrow_mut() = Some(Hit::ExistingDropArea);
    let mut output = DROPEFFECT_COPY;
    unsafe {
        fixture
            .target
            .DragEnter(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut output)
    }
    .unwrap();
    assert_eq!(output, DROPEFFECT_COPY);
    assert_eq!(
        fixture.observed.writes.borrow().last().unwrap().1,
        DROPIMAGE_INVALID.0
    );
    unsafe {
        fixture
            .target
            .Drop(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut output)
    }
    .unwrap();
    assert_eq!(output, DROPEFFECT_COPY);
    assert!(
        matches!(fixture.shared.borrow().drops.as_slice(), [DropRequest::GridFiles(paths)]
        if paths == &[PathBuf::from(r"C:\external\visible.txt")])
    );
    assert_eq!(fixture.observed.reads.get(), 1);
    assert_eq!(fixture.observed.writes.borrow().len(), 1); // Grid and finish both use INVALID.
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
    let DropRequest::FolderTransfer(request) = shared.drops.pop().unwrap() else {
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
    *fixture.hit.borrow_mut() = Some(tree_hit(r"C:\dst".into()));
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
    let DropRequest::FolderTransfer(request) = request else {
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
fn main_drop_nodrop_leaving_row_and_dragleave_write_none_then_invalid() {
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
        vec![DROPIMAGE_COPY.0, DROPIMAGE_NONE.0, DROPIMAGE_INVALID.0]
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

fn observe_repaints(fixture: &Fixture) -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
    let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = requests.clone();
    fixture.ctx.set_request_repaint_callback(move |_| {
        observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    });
    requests
}

fn clear_repaints(fixture: &Fixture, requests: &std::sync::atomic::AtomicUsize) {
    // Clear egui's coalesced immediate requests before checking the next callback.
    for _ in 0..3 {
        fixture
            .ctx
            .run_pass(Default::default(), |_| {})
            .drop_without_applying_deltas();
    }
    requests.store(0, std::sync::atomic::Ordering::Relaxed);
}

#[test]
fn hover_860_external_callbacks_track_path_place_and_repaint_only_changes() {
    use std::sync::atomic::Ordering;
    let fixture = fixture(None, false);
    let requests = observe_repaints(&fixture);
    clear_repaints(&fixture, &requests);
    let mut output = DROPEFFECT_COPY;
    unsafe {
        fixture
            .target
            .DragEnter(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut output)
    }
    .unwrap();
    assert_eq!(
        fixture.shared.borrow().hover,
        Some(DropHover {
            path: r"C:\dst".into(),
            place: DropPlace::Tree
        })
    );
    assert!(requests.load(Ordering::Relaxed) > 0);
    clear_repaints(&fixture, &requests);
    output = DROPEFFECT_COPY;
    unsafe {
        fixture
            .target
            .DragEnter(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut output)
    }
    .unwrap();
    assert_eq!(
        requests.load(Ordering::Relaxed),
        0,
        "replacing the session on the same folder keeps the same hover"
    );

    for (hit, hover, repaint) in [
        (
            Some(tree_hit(r"C:\dst".into())),
            Some(DropHover {
                path: r"C:\dst".into(),
                place: DropPlace::Tree,
            }),
            false,
        ),
        (
            Some(Hit::Folder {
                path: r"C:\dst".into(),
                place: DropPlace::Grid,
            }),
            Some(DropHover {
                path: r"C:\dst".into(),
                place: DropPlace::Grid,
            }),
            true,
        ),
        (
            Some(Hit::Folder {
                path: r"C:\other".into(),
                place: DropPlace::Grid,
            }),
            Some(DropHover {
                path: r"C:\other".into(),
                place: DropPlace::Grid,
            }),
            true,
        ),
        (Some(Hit::GridNonTarget), None, true),
        (Some(Hit::GridNonTarget), None, false),
        (Some(Hit::ExistingDropArea), None, false),
        (Some(Hit::TreeNonTarget), None, false),
        (None, None, false),
        (
            Some(tree_hit(r"C:\dst".into())),
            Some(DropHover {
                path: r"C:\dst".into(),
                place: DropPlace::Tree,
            }),
            true,
        ),
    ] {
        *fixture.hit.borrow_mut() = hit;
        clear_repaints(&fixture, &requests);
        unsafe {
            fixture
                .target
                .DragOver(MODIFIERKEYS_FLAGS(0), point(), &mut output)
        }
        .unwrap();
        assert_eq!(fixture.shared.borrow().hover, hover);
        assert_eq!(requests.load(Ordering::Relaxed) > 0, repaint);
    }
    clear_repaints(&fixture, &requests);
    unsafe { fixture.target.DragLeave() }.unwrap();
    assert!(fixture.shared.borrow().hover.is_none());
    assert!(requests.load(Ordering::Relaxed) > 0);
    clear_repaints(&fixture, &requests);
    unsafe { fixture.target.DragLeave() }.unwrap();
    assert_eq!(requests.load(Ordering::Relaxed), 0);
    assert!(fixture.shared.borrow().drops.is_empty());
}

#[test]
fn hover_860_drop_clears_hover_and_preserves_request_wake() {
    use std::sync::atomic::Ordering;
    for hit in [
        Some(Hit::Folder {
            path: r"C:\dst".into(),
            place: DropPlace::Grid,
        }),
        Some(Hit::ExistingDropArea),
        Some(Hit::GridNonTarget),
    ] {
        let fixture = fixture(None, false);
        let requests = observe_repaints(&fixture);
        let mut output = DROPEFFECT_COPY;
        unsafe {
            fixture
                .target
                .DragEnter(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut output)
        }
        .unwrap();
        assert!(fixture.shared.borrow().hover.is_some());
        *fixture.hit.borrow_mut() = hit.clone();
        clear_repaints(&fixture, &requests);
        unsafe {
            fixture
                .target
                .Drop(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut output)
        }
        .unwrap();
        assert!(fixture.shared.borrow().hover.is_none());
        assert!(requests.load(Ordering::Relaxed) > 0);
        let shared = fixture.shared.borrow();
        match hit {
            Some(Hit::Folder { .. }) => assert!(matches!(
                shared.drops.as_slice(),
                [DropRequest::FolderTransfer(_)]
            )),
            Some(Hit::ExistingDropArea) => assert!(matches!(
                shared.drops.as_slice(),
                [DropRequest::GridFiles(_)]
            )),
            _ => assert!(shared.drops.is_empty()),
        }
    }
    // Blank never acquires hover; a real external request must still wake the App.
    let fixture = fixture(None, false);
    let requests = observe_repaints(&fixture);
    *fixture.hit.borrow_mut() = Some(Hit::ExistingDropArea);
    let mut output = DROPEFFECT_COPY;
    unsafe {
        fixture
            .target
            .DragEnter(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut output)
    }
    .unwrap();
    clear_repaints(&fixture, &requests);
    unsafe {
        fixture
            .target
            .Drop(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut output)
    }
    .unwrap();
    assert!(requests.load(Ordering::Relaxed) > 0);
}

#[test]
fn hover_860_internal_and_rejected_sources_never_acquire_hover() {
    use std::sync::atomic::Ordering;
    for (internal, has_paths, allows_copy, admitted) in [
        (
            Some(InternalDrag {
                paths: vec![r"C:\src\a".into()],
                excluded_virtual: false,
                failed_paths: 0,
            }),
            true,
            true,
            true,
        ),
        (None, false, true, true),
        (None, true, false, true),
        (None, true, true, false),
    ] {
        let fixture = fixture_with_paths(internal, false, has_paths);
        if !admitted {
            fixture.shared.borrow_mut().admission = Err("blocked".into());
        }
        let requests = observe_repaints(&fixture);
        for place in [DropPlace::Tree, DropPlace::Grid] {
            *fixture.hit.borrow_mut() = Some(Hit::Folder {
                path: r"C:\dst".into(),
                place,
            });
            clear_repaints(&fixture, &requests);
            let mut output = if allows_copy {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            };
            unsafe {
                fixture
                    .target
                    .DragEnter(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut output)
            }
            .unwrap();
            assert!(fixture.shared.borrow().hover.is_none());
            assert_eq!(requests.load(Ordering::Relaxed), 0);
            clear_repaints(&fixture, &requests);
            unsafe {
                fixture
                    .target
                    .DragOver(MODIFIERKEYS_FLAGS(0), point(), &mut output)
            }
            .unwrap();
            assert!(fixture.shared.borrow().hover.is_none());
            assert_eq!(requests.load(Ordering::Relaxed), 0);
        }
    }
}

#[test]
fn hover_860_rejection_and_missing_session_clear_previous_hover() {
    let fixture = fixture(None, false);
    let mut output = DROPEFFECT_COPY;
    unsafe {
        fixture
            .target
            .DragEnter(&fixture.data, MODIFIERKEYS_FLAGS(0), point(), &mut output)
    }
    .unwrap();
    assert!(fixture.shared.borrow().hover.is_some());
    fixture.shared.borrow_mut().admission = Err("blocked".into());
    unsafe {
        fixture
            .target
            .DragOver(MODIFIERKEYS_FLAGS(0), point(), &mut output)
    }
    .unwrap();
    assert!(fixture.shared.borrow().hover.is_none());
    unsafe { fixture.target.DragLeave() }.unwrap();
    fixture.shared.borrow_mut().hover = Some(DropHover {
        path: r"C:\stale".into(),
        place: DropPlace::Grid,
    });
    unsafe {
        fixture
            .target
            .DragOver(MODIFIERKEYS_FLAGS(0), point(), &mut output)
    }
    .unwrap();
    assert!(fixture.shared.borrow().hover.is_none());
}

/// A raw Shell object models the external source without a drag loop or any window.
#[test]
fn hover_860_raw_shell_description_true_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("probe.txt");
    std::fs::write(&path, b"probe").unwrap();
    // Reuse the established STA guard, then deliberately write a separate raw object.
    crate::file_drag::with_shell_drag_data_object(std::slice::from_ref(&path), |_| {
        let (data, failed) =
            crate::file_drag::shell_data_object_for_paths(std::slice::from_ref(&path)).unwrap();
        assert_eq!(failed, 0);
        let cf = unsafe { RegisterClipboardFormatW(CFSTR_DROPDESCRIPTION) } as u16;
        let writer = OleData(data.clone());
        for (description, image) in [
            (
                Description::Transfer(ShellTransferOperation::Copy, temp.path().to_path_buf()),
                DROPIMAGE_COPY,
            ),
            (
                Description::Transfer(ShellTransferOperation::Move, temp.path().to_path_buf()),
                DROPIMAGE_MOVE,
            ),
            (Description::None, DROPIMAGE_NONE),
            (Description::Invalid, DROPIMAGE_INVALID),
        ] {
            writer.set_description(&description).unwrap();
            let medium = Medium(unsafe { data.GetData(&format(cf)) }.unwrap());
            assert_eq!(medium.0.tymed, TYMED_HGLOBAL.0 as u32);
            let global = unsafe { medium.0.u.hGlobal };
            assert!(unsafe { GlobalSize(global) } >= std::mem::size_of::<DROPDESCRIPTION>());
            let ptr = unsafe { GlobalLock(global) };
            assert!(!ptr.is_null());
            let actual = unsafe { ptr.cast::<DROPDESCRIPTION>().read_unaligned() };
            let _ = unsafe { GlobalUnlock(global) };
            let actual_type = actual.r#type;
            assert_eq!(actual_type, image);
        }
    });
}
