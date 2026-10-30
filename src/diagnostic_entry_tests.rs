//! Source connection guards supplement the small pure admission-policy tests.
//! They do not run the GUI or any native worker entrypoint.

use crate::previous_run_diagnostics::{GuiRunOutcome, test_support};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn compact(source: &str) -> String {
    source.split_whitespace().collect()
}

fn section<'a>(source: &'a str, start: &str, end: &str) -> Result<&'a str, &'static str> {
    source
        .split_once(start)
        .and_then(|(_, tail)| tail.split_once(end).map(|(section, _)| section))
        .ok_or("production entry section missing")
}

fn entry_contract(library: &str, main: &str) -> Result<(), &'static str> {
    let run = section(library, "pub fn run(session:", "fn run_trt_smoke_test()")?;
    let (before_admission, after_admission) = run
        .split_once("if gui_diagnostics_admitted(")
        .ok_or("normal GUI admission policy disconnected")?;
    for excluded_entry in [
        "maybe_handle_version_or_help",
        "pdf_loader::run_worker_process();",
        "ai::tensorrt_builder::run_worker_process();",
        "ai::trt_worker_runtime::run_infer_worker();",
        "run_trt_smoke_test();",
        "if !guard.is_first_instance()",
    ] {
        if !before_admission.contains(excluded_entry) {
            return Err("excluded entry moved after diagnostic admission");
        }
    }
    let admission = compact(
        after_admission
            .split_once("let gpu_args:")
            .ok_or("admission boundary missing")?
            .0,
    );
    for connection in [
        "session.instance.as_ref().is_some_and(|guard|guard.is_first_instance())",
        "play_test_config.is_some(),scripted_run,skip_single_instance,",
        "session.diagnostics.admit(&data_dir::get());",
    ] {
        if !admission.contains(connection) {
            return Err("first-instance/mode/diagnostic-owner admission disconnected");
        }
    }
    if !before_admission.contains("exit_gui_startup(0, session);") {
        return Err("early successful entry bypasses session termination");
    }
    if !after_admission.contains("exit_gui_startup(2, session);")
        || !after_admission.contains("exit_startup_with_notice(2, session,")
        || run.contains("std::process::exit(")
        || run.contains("exit_startup(")
        || run.contains("exit_with_logs(")
    {
        return Err("GUI early exit bypasses session termination");
    }
    let native_start = after_admission
        .find("eframe::run_native(")
        .ok_or("GUI call missing")?;
    if !after_admission[..native_start]
        .contains("let previous_run_report = session.diagnostics.report_handle();")
        || !after_admission[native_start..].contains("run_native_failure::finish_run_native(")
    {
        return Err("session disconnected from GUI entry/return");
    }
    let early = compact(section(
        library,
        "fn exit_gui_startup(",
        "/// Final boundary only:",
    )?);
    if !early.contains("exit_startup_with_notice(code,session,None)")
        || !early.contains("session.finish_until(code==0,deadline);")
    {
        return Err("real early-exit helper no longer finalizes session");
    }
    let finish = compact(section(
        library,
        "pub fn finish_gui_run(",
        "pub fn run(session:",
    )?);
    if !finish.contains("run_native_failure::finish_at_main(")
        || !finish.contains("session.finish_until(ok,deadline);")
    {
        return Err("main final boundary no longer finalizes session");
    }
    let main = compact(main);
    if !main.contains("letmutsession=mimageviewer::GuiRunSession::default();")
        || !main.contains("letreturned=mimageviewer::run(&mutsession);")
        || !main.contains("mimageviewer::finish_gui_run(returned,&mutsession)")
    {
        return Err("main no longer retains the same session across run and finish");
    }
    Ok(())
}

#[test]
fn production_modes_early_exits_and_main_retain_diagnostic_session_connection() {
    assert_eq!(
        entry_contract(include_str!("lib.rs"), include_str!("main.rs")),
        Ok(())
    );
}

#[test]
fn entry_connection_guard_rejects_removed_admission_and_exit_handoffs() {
    let library = include_str!("lib.rs");
    let main = include_str!("main.rs");
    for removed in [
        "session.diagnostics.admit(&data_dir::get());",
        "session.finish_until(code == 0, deadline);",
        "session.finish_until(ok, deadline);",
    ] {
        assert!(
            library.contains(removed),
            "mutation target disappeared: {removed}"
        );
        assert!(entry_contract(&library.replacen(removed, "", 1), main).is_err());
    }
    let moved_mode = library.replacen("pdf_loader::run_worker_process();", "", 1);
    assert!(entry_contract(&moved_mode, main).is_err());
    assert!(entry_contract(library, &main.replace("&mut session", "&mut other_session")).is_err());
}

/// The parent starts only this libtest case, never `run`, eframe, or a product
/// executable. Calling the real ! helper intentionally terminates the child.
#[test]
fn actual_gui_early_exit_child() {
    let Some(directory) = std::env::var_os("MIV_DIAGNOSTIC_ENTRY_CHILD_DIR") else {
        return;
    };
    let code: i32 = std::env::var("MIV_DIAGNOSTIC_ENTRY_CHILD_CODE")
        .unwrap()
        .parse()
        .unwrap();
    assert!(matches!(code, 0 | 2));
    let _lock = crate::data_dir::test_override_lock();
    crate::data_dir::set_test_override(Some(directory.clone().into()));
    let mut session = crate::GuiRunSession::default();
    test_support::admit(&mut session.diagnostics, Path::new(&directory));
    crate::exit_gui_startup(code, &mut session);
}

#[test]
fn real_gui_early_exit_entry_saves_success_and_error_before_process_exit() {
    for (code, expected) in [
        (0, GuiRunOutcome::ReturnedOk),
        (2, GuiRunOutcome::ReturnedError),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "diagnostic_entry_tests::actual_gui_early_exit_child",
                "--nocapture",
            ])
            .env("MIV_DIAGNOSTIC_ENTRY_CHILD_DIR", directory.path())
            .env("MIV_DIAGNOSTIC_ENTRY_CHILD_CODE", code.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("real GUI early-exit helper timed out: code={code}");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.code(), Some(code));
        assert_eq!(test_support::terminal(directory.path()), Some(expected));
    }
}
