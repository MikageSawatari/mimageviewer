//! Opt-in investigation only. The child is this library's test executable, never mIV.
use super::*;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::POINTL;
use windows::Win32::System::Com::{STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET};
use windows::Win32::System::Ole::{DROPEFFECT_COPY, IDropTarget, OleInitialize, OleUninitialize};
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_LBUTTON};
use windows::Win32::UI::Shell::{
    BHID_SFViewObject, IShellItem, SHCreateItemFromParsingName, SHCreateMemStream,
};
use windows::core::PCWSTR;

const CHILD_TEST: &str =
    "file_drag::async_data_object::tests::shell_drop_probe::shell_drop_probe_child";
const ROOT_ENV: &str = "MIV_SHELL_DROP_PROBE_ROOT";
const MODE_ENV: &str = "MIV_SHELL_DROP_PROBE_MODE";

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoMarshalInterface(
        stream: *mut c_void,
        iid: *const GUID,
        object: *mut c_void,
        context: u32,
        reserved: *mut c_void,
        flags: u32,
    ) -> HRESULT;
    fn CoUnmarshalInterface(
        stream: *mut c_void,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> HRESULT;
    fn CoReleaseMarshalData(stream: *mut c_void) -> HRESULT;
}

struct Ole;
impl Ole {
    fn new() -> Self {
        unsafe { OleInitialize(None).unwrap() };
        Self
    }
}
impl Drop for Ole {
    fn drop(&mut self) {
        unsafe { OleUninitialize() };
    }
}

fn fixture(root: &Path) -> Vec<PathBuf> {
    let src = root.join("source");
    std::fs::create_dir(&src).unwrap();
    std::fs::create_dir(root.join("destination")).unwrap();
    (0..8)
        .map(|i| {
            let path = src.join(format!("probe-{i:02}.txt"));
            std::fs::write(&path, [b'A' + i; 128]).unwrap();
            path
        })
        .collect()
}

fn copied(root: &Path) -> bool {
    (0..8).all(|i| {
        let name = format!("probe-{i:02}.txt");
        let source = std::fs::read(root.join("source").join(&name));
        let destination = std::fs::read(root.join("destination").join(name));
        matches!((source, destination), (Ok(a), Ok(b)) if a == b)
    })
}

fn make_drag(paths: &[PathBuf]) -> DragDataObject {
    let (data, failed) = crate::file_drag::shell_data_object_for_paths(paths).unwrap();
    assert_eq!(failed, 0);
    DragDataObject::new(data).unwrap()
}

fn refcount(data: &IDataObject) -> u32 {
    let unknown: IUnknown = data.cast().unwrap();
    unsafe {
        (unknown.vtable().AddRef)(unknown.as_raw());
        // Subtract the temporary QI reference; AddRef/Release are balanced.
        (unknown.vtable().Release)(unknown.as_raw()) - 1
    }
}

fn events(stage: &str) {
    eprintln!("probe {stage}: lifecycle={:?}", take_lifecycle_events());
}

fn finish_and_observe(drag: DragDataObject, root: &Path) {
    let id = drag.id;
    let running = unsafe { drag.capability.InOperation().unwrap() }.as_bool();
    eprintln!(
        "probe observation id={id} copies_equal={} running={running} source_refs={} thread={}",
        copied(root),
        refcount(&drag.data),
        unsafe { GetCurrentThreadId() }
    );
    let before = take_lifecycle_events();
    eprintln!("probe before source locals dropped (no cleanup End): lifecycle={before:?}");
    drag.finish_modal().unwrap();
    drop(drag);
    let released = LIFECYCLE_EVENTS
        .lock()
        .unwrap()
        .iter()
        .filter(|(event, drag_id, _)| *event == "released" && *drag_id == id)
        .count();
    events("after source locals dropped (no cleanup End)");
    assert!(
        before
            .iter()
            .any(|(event, drag_id, _)| *event == "start" && *drag_id == id),
        "the fixture must exercise an asynchronous operation"
    );
    assert!(
        !running,
        "operation must finish without a synthetic cleanup End"
    );
    assert_eq!(
        before
            .iter()
            .filter(|(event, drag_id, _)| *event == "end" && *drag_id == id)
            .count(),
        1,
        "completion must run exactly once"
    );
    assert_eq!(
        released, 1,
        "object must release exactly once without a cleanup End"
    );
}

fn target_drop(data: &IDataObject, root: &Path) {
    let wide: Vec<u16> = root
        .join("destination")
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect();
    let item: IShellItem =
        unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None::<&IBindCtx>).unwrap() };
    let target: IDropTarget = unsafe {
        item.BindToHandler(None::<&IBindCtx>, &BHID_SFViewObject)
            .unwrap()
    };
    let start = Instant::now();
    let point = POINTL::default();
    let mut effect = DROPEFFECT_COPY;
    // Raw vtable calls retain successful HRESULTs such as S_FALSE in the evidence.
    unsafe {
        let enter = (target.vtable().DragEnter)(
            target.as_raw(),
            data.as_raw(),
            MK_CONTROL | MK_LBUTTON,
            point,
            &mut effect,
        );
        eprintln!(
            "probe DragEnter hr={enter:?} effect={} elapsed={:?}",
            effect.0,
            start.elapsed()
        );
        enter.ok().unwrap();
        effect = DROPEFFECT_COPY;
        let over = (target.vtable().DragOver)(
            target.as_raw(),
            MK_CONTROL | MK_LBUTTON,
            point,
            &mut effect,
        );
        eprintln!(
            "probe DragOver hr={over:?} effect={} elapsed={:?}",
            effect.0,
            start.elapsed()
        );
        over.ok().unwrap();
        effect = DROPEFFECT_COPY;
        let result = (target.vtable().Drop)(
            target.as_raw(),
            data.as_raw(),
            MK_CONTROL | MK_LBUTTON,
            point,
            &mut effect,
        );
        eprintln!(
            "probe Drop hr={result:?} effect={} duration={:?}",
            effect.0,
            start.elapsed()
        );
        result.ok().unwrap();
        let capability: IDataObjectAsyncCapability = data.cast().unwrap();
        eprintln!(
            "probe immediately after Drop: async={} in_operation={} thread={}",
            capability.GetAsyncMode().unwrap().as_bool(),
            capability.InOperation().unwrap().as_bool(),
            GetCurrentThreadId()
        );
    }
    drop((target, item));
    eprintln!(
        "probe target interfaces dropped; copies_equal={}",
        copied(root)
    );
    let completed = pump_until(|| copied(root), Duration::from_secs(5));
    eprintln!(
        "probe byte comparison completed={completed} at {:?}; pumping grace",
        start.elapsed()
    );
    // Completion is byte equality, not an elapsed-time inference. Pump another two
    // seconds after that observation to allow terminal callbacks/proxy releases.
    pump_until(|| false, Duration::from_secs(2));
    eprintln!(
        "probe grace ended at {:?}; copies_equal={}",
        start.elapsed(),
        copied(root)
    );
    assert!(
        completed,
        "Shell did not copy all disposable files before deadline"
    );
}

fn spawn_child(root: &Path, mode: &str) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            CHILD_TEST,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ROOT_ENV, root)
        .env(MODE_ENV, mode)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW; no desktop/input interaction.
        .spawn()
        .unwrap()
}

fn wait_child(child: &mut Child) -> bool {
    let finished = pump_until(
        || child.try_wait().unwrap().is_some(),
        Duration::from_secs(12),
    );
    if !finished {
        eprintln!("probe cleanup: test child exceeded 12 seconds; terminating child only");
        let _ = child.kill();
    }
    // try_wait above has already reaped a normally finished child.
    let status = child.wait().unwrap();
    finished && status.success()
}

#[test]
#[ignore = "opt-in headless real Shell target investigation; disposable files only"]
fn shell_drop_probe_same_process() {
    let _ole = Ole::new();
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    // Even a synchronous Shell Drop is bounded by an external test-process watchdog.
    let mut child = spawn_child(root.path(), "same-process");
    assert!(
        wait_child(&mut child),
        "same-process Shell probe child failed/timed out"
    );
    assert!(copied(root.path()));
}

#[test]
#[ignore = "opt-in cross-process real Shell target investigation; disposable files only"]
fn shell_drop_probe_cross_process() {
    run_cross_process("cross-process");
}

#[test]
#[ignore = "opt-in cross-process COM delivery control; no Shell Drop or application UI"]
fn shell_drop_probe_cross_process_control() {
    run_cross_process("control");
}

fn run_cross_process(mode: &str) {
    let _ole = Ole::new();
    clear_lifecycle_events();
    let owner_thread = unsafe { GetCurrentThreadId() };
    let root = tempfile::tempdir().unwrap();
    let paths = fixture(root.path());
    let drag = make_drag(&paths);
    let stream = unsafe { SHCreateMemStream(None).unwrap() };
    unsafe {
        // MSHCTX_LOCAL=0, MSHLFLAGS_TABLESTRONG=1: the parent explicitly releases
        // this marshal-table hold after the child drops its unmarshaled proxy.
        CoMarshalInterface(
            stream.as_raw(),
            &IDataObject::IID,
            drag.data.as_raw(),
            0,
            null_mut(),
            1,
        )
        .ok()
        .unwrap();
        let mut stat = STATSTG::default();
        stream.Stat(&mut stat, STATFLAG_NONAME).unwrap();
        assert!(stat.cbSize < 1024 * 1024);
        stream.Seek(0, STREAM_SEEK_SET, None).unwrap();
        let mut packet = vec![0u8; stat.cbSize as usize];
        let mut read = 0;
        stream
            .Read(
                packet.as_mut_ptr().cast(),
                packet.len() as u32,
                Some(&mut read),
            )
            .ok()
            .unwrap();
        assert_eq!(read as usize, packet.len());
        std::fs::write(root.path().join("marshal.bin"), packet).unwrap();
    }
    let mut child = spawn_child(root.path(), mode);
    let success = wait_child(&mut child);
    eprintln!(
        "probe child exited; source_refs_before_marshal_release={}",
        refcount(&drag.data)
    );
    unsafe {
        stream.Seek(0, STREAM_SEEK_SET, None).unwrap();
        CoReleaseMarshalData(stream.as_raw()).ok().unwrap();
    }
    drop(stream);
    pump_until(|| false, Duration::from_millis(250));
    if mode == "control" && success {
        let observed = LIFECYCLE_EVENTS.lock().unwrap().clone();
        for event in ["start", "get_data", "end_entered", "end"] {
            assert!(
                observed.contains(&(event, drag.id, owner_thread)),
                "missing owner-STA event {event}: {observed:?}"
            );
        }
        assert!(!unsafe { drag.capability.InOperation().unwrap() }.as_bool());
        assert_eq!(refcount(&drag.data), 2, "only the two source locals remain");
    }
    finish_and_observe(drag, root.path());
    assert!(success, "cross-process Shell probe child failed/timed out");
    if mode != "control" {
        assert!(copied(root.path()));
    }
}

#[test]
#[ignore = "private child entry; invoked only by the parent investigation tests"]
fn shell_drop_probe_child() {
    let Some(root) = std::env::var_os(ROOT_ENV) else {
        eprintln!("probe child skipped: no parent fixture supplied");
        return;
    };
    let root = PathBuf::from(root);
    let _ole = Ole::new();
    let mode = std::env::var(MODE_ENV).unwrap();
    if mode == "same-process" {
        clear_lifecycle_events();
        let paths: Vec<_> = (0..8)
            .map(|i| root.join("source").join(format!("probe-{i:02}.txt")))
            .collect();
        let drag = make_drag(&paths);
        target_drop(&drag.data, &root);
        eprintln!(
            "probe finish_modal returned running={}",
            drag.finish_modal().unwrap()
        );
        finish_and_observe(drag, &root);
    } else {
        let packet = std::fs::read(root.join("marshal.bin")).unwrap();
        let stream = unsafe { SHCreateMemStream(Some(&packet)).unwrap() };
        let mut raw = null_mut();
        unsafe {
            CoUnmarshalInterface(stream.as_raw(), &IDataObject::IID, &mut raw)
                .ok()
                .unwrap();
        }
        let data = unsafe { IDataObject::from_raw(raw) };
        eprintln!("probe child unmarshaled source; thread={}", unsafe {
            GetCurrentThreadId()
        });
        if mode == "control" {
            // Explicit synthetic client calls isolate COM transport from Shell Drop.
            // They are never evidence that the real Shell target sent EndOperation.
            eprintln!("probe control: synthetic Start/GetData/End; no target Drop");
            let capability: IDataObjectAsyncCapability = data.cast().unwrap();
            unsafe {
                assert!(capability.GetAsyncMode().unwrap().as_bool());
                capability.StartOperation(None::<&IBindCtx>).unwrap();
                assert!(capability.InOperation().unwrap().as_bool());
                let mut medium = data.GetData(&format()).unwrap();
                assert_eq!(medium.tymed, TYMED_HGLOBAL.0 as u32);
                assert_eq!(DragQueryFileW(HDROP(medium.u.hGlobal.0), u32::MAX, None), 8);
                ReleaseStgMedium(&mut medium);
                capability.EndOperation(S_OK, None::<&IBindCtx>, 1).unwrap();
                assert!(!capability.InOperation().unwrap().as_bool());
            }
        } else {
            assert_eq!(mode, "cross-process");
            target_drop(&data, &root);
        }
        drop((data, stream));
        eprintln!("probe child source proxy dropped");
    }
}
