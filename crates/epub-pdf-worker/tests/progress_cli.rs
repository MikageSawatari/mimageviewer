use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn assert_final_result(output: &std::process::Output, status: &str, exit_code: i32) {
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    let events: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event"] == "result")
            .count(),
        1
    );
    let final_event = events.last().unwrap();
    assert_eq!(final_event["event"], "result");
    assert_eq!(final_event["status"], status);
    assert_eq!(final_event["exit_code"], exit_code);
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_mimageviewer-epub-pdf")
}

#[test]
fn argument_error_has_one_final_result() {
    let output = Command::new(binary())
        .args([
            "convert",
            "missing.epub",
            "out.pdf",
            "--progress-json",
            "--unknown",
        ])
        .output()
        .unwrap();
    assert_final_result(&output, "invalid", 3);
}

#[test]
fn missing_input_has_one_final_result_and_no_output() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "epub-pdf-progress-test-{}-{nonce}",
        std::process::id()
    ));
    let input = root.join("missing.epub");
    let out = root.join("out.pdf.part");
    let report = root.join("report.json");
    let output = Command::new(binary())
        .arg("convert")
        .arg(&input)
        .arg(&out)
        .arg("--work-dir")
        .arg(root.join("work"))
        .arg("--report")
        .arg(&report)
        .arg("--progress-json")
        .output()
        .unwrap();
    assert_final_result(&output, "invalid", 3);
    assert!(!out.exists());
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
}

#[test]
fn unsafe_user_data_path_is_invalid_and_has_final_result() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "epub-pdf-unsafe-path-test-{}-{nonce}",
        std::process::id()
    ));
    let out = root.join("out.pdf.part");
    let output = Command::new(binary())
        .arg("convert")
        .arg(root.join("missing.epub"))
        .arg(&out)
        .arg("--work-dir")
        .arg(root.join("work"))
        .arg("--user-data-dir")
        .arg(&root)
        .arg("--progress-json")
        .output()
        .unwrap();
    assert_final_result(&output, "invalid", 3);
    assert!(!out.exists());
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
}

#[cfg(debug_assertions)]
#[test]
fn panic_after_user_data_creation_cleans_folder_and_reports_once() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "epub-pdf-panic-test-{}-{nonce}",
        std::process::id()
    ));
    let user_data = root.join("user-data");
    let out = root.join("out.pdf.part");
    let output = Command::new(binary())
        .arg("convert")
        .arg(root.join("missing.epub"))
        .arg(&out)
        .arg("--work-dir")
        .arg(root.join("work"))
        .arg("--user-data-dir")
        .arg(&user_data)
        .arg("--progress-json")
        .env("MIV_EPUB_PDF_TEST_PANIC", "after_user_data")
        .output()
        .unwrap();
    assert_final_result(&output, "render_failed", 5);
    assert!(!user_data.exists());
    assert!(!out.exists());
    if root.exists() {
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(debug_assertions)]
#[test]
fn panic_keeps_preexisting_empty_user_data_folder() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "epub-pdf-existing-data-test-{}-{nonce}",
        std::process::id()
    ));
    let user_data = root.join("user-data");
    fs::create_dir_all(&user_data).unwrap();
    let output = Command::new(binary())
        .arg("convert")
        .arg(root.join("missing.epub"))
        .arg(root.join("out.pdf.part"))
        .arg("--work-dir")
        .arg(root.join("work"))
        .arg("--user-data-dir")
        .arg(&user_data)
        .arg("--progress-json")
        .env("MIV_EPUB_PDF_TEST_PANIC", "after_user_data")
        .output()
        .unwrap();
    assert_final_result(&output, "render_failed", 5);
    assert!(user_data.is_dir());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn zero_timeout_is_invalid_before_webview_creation() {
    let output = Command::new(binary())
        .args([
            "convert",
            "missing.epub",
            "out.pdf",
            "--work-dir",
            "work",
            "--timeout-secs",
            "0",
            "--progress-json",
        ])
        .output()
        .unwrap();
    assert_final_result(&output, "invalid", 3);
}
