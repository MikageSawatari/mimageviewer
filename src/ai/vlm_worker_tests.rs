use super::*;
use std::sync::OnceLock;
use std::time::Instant;

#[test]
fn worker_resolution_precedence_covers_debug_release_and_both_flavors_without_io() {
    use WorkerExecutableSource::{Bundled, Embedded, Override};
    let path = "synthetic-override.exe";
    for (debug, portable, supplied, expected) in [
        (false, false, None, Embedded),
        (false, false, Some(path), Embedded),
        (false, true, None, Bundled),
        (false, true, Some(path), Bundled),
        (true, false, None, Embedded),
        (true, false, Some(path), Override(path.into())),
        (true, true, None, Bundled),
        (true, true, Some(path), Override(path.into())),
    ] {
        assert_eq!(
            worker_executable_source(debug, portable, supplied.map(OsString::from)),
            expected
        );
    }
}

fn fake_worker() -> &'static Path {
    use sha2::{Digest as _, Sha256};

    static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let source = manifest.join("tests/support/vlm_fake_worker.rs");
        // include_bytes also makes Cargo rebuild the tests when the peer changes.
        let hash = Sha256::digest(include_bytes!("../../tests/support/vlm_fake_worker.rs"));
        let cache_root = manifest.join("target/vlm-fake-worker");
        let directory = cache_root.join(format!("{hash:x}"));
        let executable = directory.join("vlm-fake-worker.exe");
        if executable.is_file() {
            return executable;
        }
        std::fs::create_dir_all(&cache_root).unwrap();
        let staging = tempfile::Builder::new()
            .prefix(".build-")
            .tempdir_in(&cache_root)
            .unwrap();
        let output = std::process::Command::new("rustc")
            .env("TEMP", staging.path())
            .env("TMP", staging.path())
            .arg("--edition=2024")
            .arg("--crate-name=vlm_fake_worker")
            .arg(source)
            .arg("-o")
            .arg(staging.path().join("vlm-fake-worker.exe"))
            .output()
            .expect("rustc must be available for the standalone fake worker");
        assert!(
            output.status.success(),
            "fake worker compile failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Publish the exe and PDB together. Concurrent losers reuse the
        // winner and drop their staging directory, including its PDB.
        if let Err(error) = std::fs::rename(staging.path(), &directory) {
            assert!(executable.is_file(), "fake worker publish failed: {error}");
        }
        executable
    })
}

fn budgets() -> Timeouts {
    Timeouts {
        hello: Duration::from_secs(5),
        load: Duration::from_secs(5),
        first_caption: Duration::from_secs(5),
        caption: Duration::from_secs(5),
    }
}

fn args(mode: &str) -> Vec<OsString> {
    ["--serve", "--fake-mode", mode]
        .into_iter()
        .map(OsString::from)
        .collect()
}

fn fake(mode: &str) -> VlmWorkerClient {
    VlmWorkerClient::spawn_with(fake_worker(), &args(mode), budgets()).unwrap()
}

fn load_request() -> LoadRequest {
    LoadRequest {
        model_path: PathBuf::from("synthetic model.gguf"),
        mmproj_path: PathBuf::from("synthetic mmproj.gguf"),
        device_luid: "00000000:00000001".into(),
        cpu_threads: 2,
    }
}

fn loaded(mode: &str) -> VlmWorkerClient {
    let mut client = fake(mode);
    client.load(&load_request()).unwrap();
    client
}

fn caption(client: &mut VlmWorkerClient) -> Result<CaptionOutcome, VlmError> {
    client.caption(41, 2, 1, vec![13, 10, 0, 26, 255, 128])
}

fn assert_poisoned(client: &mut VlmWorkerClient) {
    assert!(!client.is_usable());
    assert!(
        caption(client).is_err(),
        "a failed peer must never be reused"
    );
    assert_worker_terminated(client);
}

fn assert_worker_terminated(client: &VlmWorkerClient) {
    use std::os::windows::io::AsRawHandle as _;
    use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::WaitForSingleObject;
    assert_eq!(
        unsafe {
            WaitForSingleObject(HANDLE(client._child.process_handle().as_raw_handle()), 3000)
        },
        WAIT_OBJECT_0,
        "terminal failure must actually terminate the worker process"
    );
}

#[test]
fn byte_rgb_roundtrip_preserves_cr_lf_nul_ctrl_z_and_high_bytes() {
    let mut client = VlmWorkerClient::spawn_at(fake_worker()).unwrap();
    let load = client.load(&load_request()).unwrap();
    assert_eq!(load.model_load_s, 0.125);
    match caption(&mut client).unwrap() {
        CaptionOutcome::Ok(result) => {
            assert_eq!(result.caption, "0d0a001aff80");
            assert_eq!(result.text, "");
            assert_eq!(result.keywords, ["binary"]);
            assert_eq!(result.generated_tokens, 7);
            assert_eq!(result.wall_s, 0.01);
        }
        _ => panic!("valid binary request did not receive a caption"),
    }
    assert!(client.is_usable());
    assert!(matches!(
        client.caption(u64::MAX, 1, 1, vec![0, 13, 26]),
        Ok(CaptionOutcome::Ok(_))
    ));
    client.shutdown();
    assert!(!client.is_usable());
}

#[test]
fn missing_system_vulkan_loader_skips_the_worker_probe() {
    assert!(matches!(
        detect_gpu_with(
            || false,
            || panic!("worker must not start without the system loader")
        ),
        Ok(GpuDetection::VulkanLoaderUnavailable)
    ));
}

#[test]
fn system_vulkan_loader_path_is_absolute_instead_of_a_reusable_basename() {
    let encoded = system_vulkan_loader_path().expect("Windows system directory must be available");
    assert_eq!(encoded.last(), Some(&0));
    let decoded = String::from_utf16(&encoded[..encoded.len() - 1]).unwrap();
    let path = Path::new(&decoded);
    assert!(path.is_absolute());
    assert_eq!(path.file_name().unwrap(), "vulkan-1.dll");
    assert!(path.parent().unwrap().is_absolute());
}

#[test]
fn present_system_vulkan_loader_preserves_worker_start_failure() {
    let error = VlmError::Io("injected worker start failure".into());
    assert_eq!(
        detect_gpu_with(|| true, || Err(error.clone())).unwrap_err(),
        error
    );
    assert!(matches!(
        detect_gpu_with(|| true, || Ok(GpuDecision {
            device: None, vram_total_bytes: MIN_VRAM_BYTES, vulkan_available: true,
        })),
        Ok(GpuDetection::Device(gpu)) if gpu.is_eligible()
    ));
}

#[test]
fn incompatible_hello_version_is_rejected() {
    assert!(matches!(
        VlmWorkerClient::spawn_with(fake_worker(), &args("hello-version"), budgets()),
        Err(VlmError::Protocol(_))
    ));
}

#[test]
fn incompatible_or_missing_prompt_version_is_rejected_and_worker_is_retired() {
    for mode in ["hello-prompt", "hello-missing-prompt"] {
        let mut client =
            VlmWorkerClient::start_with(fake_worker(), &args(mode), budgets()).unwrap();
        assert!(matches!(client.hello(), Err(VlmError::Protocol(_))));
        assert_poisoned(&mut client);
    }
}

#[test]
fn gpu_eligibility_uses_vulkan_and_exact_seven_point_five_gib_boundary() {
    for bytes in [MIN_VRAM_BYTES - 1, MIN_VRAM_BYTES, MIN_VRAM_BYTES + 1] {
        for vulkan in [false, true] {
            let gpu = GpuDecision {
                device: None,
                vram_total_bytes: bytes,
                vulkan_available: vulkan,
            };
            assert_eq!(gpu.is_eligible(), vulkan && bytes >= 8_053_063_680);
        }
    }
}

#[test]
fn hello_deadline_is_bounded() {
    let mut timeouts = budgets();
    timeouts.hello = Duration::from_millis(150);
    let executable = fake_worker();
    let start = Instant::now();
    assert!(matches!(
        VlmWorkerClient::spawn_with(executable, &args("stall-hello"), timeouts),
        Err(VlmError::Timeout)
    ));
    assert!(start.elapsed() < Duration::from_secs(10));
}

#[test]
fn malformed_response_frames_poison_the_client() {
    for mode in [
        "malformed-json",
        "oversized-frame",
        "invalid-json-length",
        "unknown-type",
        "unknown-status",
        "mismatch-id",
        "unexpected-rgb",
    ] {
        let mut client = loaded(mode);
        assert!(
            matches!(caption(&mut client), Err(VlmError::Protocol(_))),
            "mode {mode} must be a protocol failure"
        );
        assert_poisoned(&mut client);
    }
}

#[test]
fn disconnected_and_truncated_response_poison_the_client() {
    for mode in ["disconnect", "truncated-frame"] {
        let mut client = loaded(mode);
        assert!(caption(&mut client).is_err(), "mode {mode}");
        assert_poisoned(&mut client);
    }
}

#[test]
fn image_failure_keeps_the_same_worker_usable() {
    let mut client = loaded("image-failure-once");
    match caption(&mut client).unwrap() {
        CaptionOutcome::ImageFailure { message } => {
            assert_eq!(message, "synthetic bad image");
        }
        _ => panic!("expected one image failure"),
    }
    assert!(client.is_usable());
    assert!(matches!(caption(&mut client), Ok(CaptionOutcome::Ok(_))));
}

#[test]
fn invalid_success_content_is_an_image_failure_and_next_image_can_run() {
    for mode in [
        "invalid-caption-once",
        "incomplete-content-once",
        "mismatched-content-once",
        "length-finish-once",
    ] {
        let mut client = loaded(mode);
        assert!(
            matches!(
                caption(&mut client),
                Ok(CaptionOutcome::ImageFailure { .. })
            ),
            "mode {mode} must fail content validation"
        );
        assert!(client.is_usable());
        assert!(matches!(caption(&mut client), Ok(CaptionOutcome::Ok(_))));
    }
}

#[test]
fn loaded_device_must_match_the_requested_adapter() {
    let mut client = fake("wrong-loaded-device");
    assert!(matches!(
        client.load(&load_request()),
        Err(VlmError::Protocol(_))
    ));
    assert_poisoned(&mut client);
}

#[test]
fn fatal_caption_is_terminal_without_retry() {
    let mut client = loaded("fatal");
    match caption(&mut client) {
        Err(VlmError::Fatal(message)) => assert_eq!(message, "synthetic fatal caption"),
        _ => panic!("expected a fatal caption result"),
    }
    assert_poisoned(&mut client);
}

#[test]
fn fatal_load_and_disconnected_load_are_terminal() {
    let mut client = fake("fatal-load");
    assert!(matches!(
        client.load(&load_request()),
        Err(VlmError::Fatal(_))
    ));
    assert_poisoned(&mut client);
    let mut client = fake("disconnect-load");
    assert!(client.load(&load_request()).is_err());
    assert_poisoned(&mut client);
}

#[test]
fn load_deadline_kills_the_peer() {
    let mut timeouts = budgets();
    timeouts.load = Duration::from_millis(150);
    let mut client =
        VlmWorkerClient::spawn_with(fake_worker(), &args("stall-load"), timeouts).unwrap();
    let start = Instant::now();
    assert!(matches!(
        client.load(&load_request()),
        Err(VlmError::Timeout)
    ));
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_poisoned(&mut client);
}

#[test]
fn caption_read_deadline_kills_the_peer() {
    let mut timeouts = budgets();
    timeouts.first_caption = Duration::from_millis(150);
    let mut client =
        VlmWorkerClient::spawn_with(fake_worker(), &args("stall-caption"), timeouts).unwrap();
    client.load(&load_request()).unwrap();
    let start = Instant::now();
    assert!(matches!(caption(&mut client), Err(VlmError::Timeout)));
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_poisoned(&mut client);
}

#[test]
fn blocked_pipe_writer_does_not_block_the_calling_thread_past_deadline() {
    let mut timeouts = budgets();
    timeouts.first_caption = Duration::from_millis(150);
    let mut client =
        VlmWorkerClient::spawn_with(fake_worker(), &args("stall-read"), timeouts).unwrap();
    client.load(&load_request()).unwrap();
    let start = Instant::now();
    assert!(matches!(
        client.caption(42, 1024, 1024, vec![26; 1024 * 1024 * 3]),
        Err(VlmError::Timeout)
    ));
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_poisoned(&mut client);
}

#[test]
fn only_first_caption_uses_the_longer_initial_budget() {
    let mut timeouts = budgets();
    timeouts.first_caption = Duration::from_secs(3);
    timeouts.caption = Duration::from_millis(30);
    let mut client =
        VlmWorkerClient::spawn_with(fake_worker(), &args("delayed-captions"), timeouts).unwrap();
    client.load(&load_request()).unwrap();
    assert!(matches!(caption(&mut client), Ok(CaptionOutcome::Ok(_))));
    assert!(matches!(caption(&mut client), Err(VlmError::Timeout)));
    assert_poisoned(&mut client);
}

fn wait_for_receipt(path: &Path, command: &[u8]) {
    let start = Instant::now();
    while std::fs::read(path).ok().as_deref() != Some(command) {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "fake did not accept request"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn external_kill_unblocks_pending_hello_load_and_caption() {
    for mode in ["stall-hello", "stall-load", "stall-caption"] {
        let directory = tempfile::tempdir().unwrap();
        let receipt = directory.path().join("accepted");
        let mut arguments = args(mode);
        arguments.push("--notify".into());
        arguments.push(receipt.as_os_str().to_owned());
        let mut timeouts = budgets();
        timeouts.hello = Duration::from_secs(30);
        timeouts.load = Duration::from_secs(30);
        timeouts.first_caption = Duration::from_secs(30);
        let mut client = VlmWorkerClient::start_with(fake_worker(), &arguments, timeouts).unwrap();
        assert!(matches!(client.state, State::New));
        let kill = client.kill_handle();
        if mode != "stall-hello" {
            client.hello().unwrap();
        }
        if mode == "stall-caption" {
            client.load(&load_request()).unwrap();
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let operation = std::thread::spawn(move || {
            let failed = match mode {
                "stall-hello" => client.hello().is_err(),
                "stall-load" => client.load(&load_request()).is_err(),
                "stall-caption" => caption(&mut client).is_err(),
                _ => unreachable!(),
            };
            assert_worker_terminated(&client);
            tx.send((failed, client.is_usable())).unwrap();
        });
        wait_for_receipt(
            &receipt,
            match mode {
                "stall-hello" => b"hello",
                "stall-load" => b"load",
                "stall-caption" => b"caption",
                _ => unreachable!(),
            },
        );
        let start = Instant::now();
        kill.kill().unwrap();
        let (failed, usable) = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(failed);
        assert!(!usable);
        assert!(start.elapsed() < Duration::from_secs(3));
        operation.join().unwrap();
    }
}

#[test]
fn shutdown_never_waits_for_a_nonreading_peer() {
    let mut client = loaded("stall-read");
    let start = Instant::now();
    client.shutdown();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(!client.is_usable());
    assert_worker_terminated(&client);
}

#[test]
fn invalid_rgb_dimensions_and_length_retire_the_client() {
    for (width, height, length) in [
        (0, 1, 0),
        (1, 0, 0),
        (1025, 1, 3075),
        (1, 1025, 3075),
        (2, 1, 5),
        (2, 1, 7),
        (u32::MAX, u32::MAX, 0),
    ] {
        let mut client = loaded("ok");
        assert!(matches!(
            client.caption(1, width, height, vec![0; length]),
            Err(VlmError::Protocol(_))
        ));
        assert_poisoned(&mut client);
    }
}

#[test]
fn requests_outside_the_single_load_session_are_rejected() {
    let mut client = fake("ok");
    assert!(matches!(caption(&mut client), Err(VlmError::Unavailable)));
    client.load(&load_request()).unwrap();
    assert!(matches!(
        client.load(&load_request()),
        Err(VlmError::Unavailable)
    ));
    assert!(matches!(caption(&mut client), Ok(CaptionOutcome::Ok(_))));
}

#[test]
fn frame_limit_applies_to_json_and_rgb_together() {
    let header = json!({"type":"caption","request_id":9});
    let json_size = serde_json::to_vec(&header).unwrap().len();
    let bytes = vec![26; MAX_FRAME - 4 - json_size];
    let encoded = encode_frame(&header, &bytes).unwrap();
    assert_eq!(
        u32::from_le_bytes(encoded[..4].try_into().unwrap()) as usize,
        MAX_FRAME
    );
    let decoded = read_frame(&mut encoded.as_slice()).unwrap();
    assert_eq!(decoded.header, header);
    assert_eq!(decoded.bytes, bytes);
    let over_limit = vec![0; bytes.len() + 1];
    assert!(matches!(
        encode_frame(&header, &over_limit),
        Err(VlmError::Protocol(_))
    ));
}

#[test]
fn malformed_outer_inner_and_json_are_rejected_before_use() {
    for length in [0u32, 1, 3, MAX_FRAME as u32 + 1, u32::MAX] {
        assert!(matches!(
            read_frame(&mut length.to_le_bytes().as_slice()),
            Err(VlmError::Protocol(_))
        ));
    }
    for json in [
        b"null".as_slice(),
        b"[]",
        b"\"hello\"",
        b"{",
        b"{}\0",
        &[0xff],
    ] {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(4u32 + json.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
        bytes.extend_from_slice(json);
        assert!(matches!(
            read_frame(&mut bytes.as_slice()),
            Err(VlmError::Protocol(_))
        ));
    }
    for (outer, inner) in [(4u32, 0u32), (4, 1), (5, 2)] {
        let mut bytes = outer.to_le_bytes().to_vec();
        bytes.extend_from_slice(&inner.to_le_bytes());
        bytes.resize(outer as usize + 4, 0);
        assert!(matches!(
            read_frame(&mut bytes.as_slice()),
            Err(VlmError::Protocol(_))
        ));
    }
}

fn maximum_caption() -> CaptionResult {
    let content = json!({
        "caption": "画".repeat(400),
        "text": "𠮷".repeat(400),
        "keywords": vec!["語".repeat(32); 20],
    });
    serde_json::from_value(json!({
        "content": content.to_string(),
        "finish_reason": "stop",
        "caption": content["caption"],
        "text": content["text"],
        "keywords": content["keywords"],
        "validation": {
            "encoding_valid": true, "ended_on_eog": true, "json_complete": true,
            "field_types": true, "limits": true, "token_limit": true, "ok": true
        },
        "generated_tokens": 800,
        "wall_s": 0.0
    }))
    .unwrap()
}

fn sync_content(result: &mut CaptionResult) {
    result.content = json!({
        "caption":result.caption,
        "text":result.text,
        "keywords":result.keywords,
    })
    .to_string();
}

#[test]
fn caption_contract_accepts_exact_unicode_limits_and_rejects_each_excess() {
    assert!(
        maximum_caption().valid(),
        "limits count Unicode scalars rather than UTF-8 bytes"
    );
    let mut result = maximum_caption();
    result.caption.push('画');
    sync_content(&mut result);
    assert!(!result.valid());
    let mut result = maximum_caption();
    result.text.push('𠮷');
    sync_content(&mut result);
    assert!(!result.valid());
    let mut result = maximum_caption();
    result.keywords.push("extra".into());
    sync_content(&mut result);
    assert!(!result.valid());
    let mut result = maximum_caption();
    result.keywords[0].push('語');
    sync_content(&mut result);
    assert!(!result.valid());
    let mut result = maximum_caption();
    result.generated_tokens = 801;
    assert!(!result.valid());
    for wall in [-1.0, f64::NAN, f64::INFINITY] {
        let mut result = maximum_caption();
        result.wall_s = wall;
        assert!(!result.valid());
    }
    for field in [
        "encoding_valid",
        "ended_on_eog",
        "json_complete",
        "field_types",
        "limits",
        "token_limit",
        "ok",
    ] {
        let mut result = maximum_caption();
        match field {
            "encoding_valid" => result.validation.encoding_valid = false,
            "ended_on_eog" => result.validation.ended_on_eog = false,
            "json_complete" => result.validation.json_complete = false,
            "field_types" => result.validation.field_types = false,
            "limits" => result.validation.limits = false,
            "token_limit" => result.validation.token_limit = false,
            "ok" => result.validation.ok = false,
            _ => unreachable!(),
        }
        assert!(!result.valid(), "false {field} must reject the output");
    }
}

#[test]
fn caption_contract_rechecks_complete_original_json_and_finish_reason() {
    for content in [
        "{",
        "null",
        r#"{"caption":"other","text":"","keywords":[]}"#,
        r#"{"caption":10,"text":"","keywords":[]}"#,
        r#"{"caption":"","text":"","keywords":[1]}"#,
    ] {
        let mut result = maximum_caption();
        result.content = content.into();
        assert!(!result.valid());
    }
    let mut result = maximum_caption();
    result.content.push('\0');
    assert!(!result.valid());
    let mut result = maximum_caption();
    result.content.push_str("{}");
    assert!(!result.valid());
    let mut result = maximum_caption();
    result.finish_reason = "length".into();
    assert!(!result.valid());
}

fn adapter(name: &str, luid: &str, gib: u64) -> Value {
    json!({"name":name,"luid":luid,"dedicated_bytes":gib * 1024 * 1024 * 1024,"software":false})
}

#[test]
fn gpu_decision_does_not_substitute_a_smaller_vulkan_adapter() {
    let decision = select_gpu(&json!({
        "dxgi": [adapter("small", "small", 4), adapter("large", "large", 16)],
        "vulkan": [{"luid_valid":true,"luid":"small","mapping":"pci-id"}]
    }))
    .unwrap();
    assert_eq!(decision.device.unwrap().name, "large");
    assert_eq!(decision.vram_total_bytes, 16 * 1024 * 1024 * 1024);
    assert!(!decision.vulkan_available);
}

#[test]
fn gpu_decision_matches_luid_mapping_and_ignores_software_and_zero_memory() {
    let mut software = adapter("software", "software", 64);
    software["software"] = json!(true);
    let adapters = json!([
        software,
        adapter("integrated", "integrated", 0),
        adapter("chosen", "chosen", 8)
    ]);
    for mapping in ["pci-id", "unique-description"] {
        let decision = select_gpu(&json!({
            "dxgi": adapters,
            "vulkan": [{"luid_valid":true,"luid":"chosen","mapping":mapping}]
        }))
        .unwrap();
        assert_eq!(decision.device.unwrap().name, "chosen");
        assert!(decision.vulkan_available);
    }
    for vk in [
        json!({"luid_valid":false,"luid":"chosen","mapping":"pci-id"}),
        json!({"luid_valid":true,"luid":"other","mapping":"pci-id"}),
        json!({"luid_valid":true,"luid":"chosen","mapping":"ambiguous"}),
    ] {
        let decision = select_gpu(&json!({"dxgi":adapters,"vulkan":[vk]})).unwrap();
        assert!(!decision.vulkan_available);
    }
}

#[test]
fn gpu_ties_keep_first_adapter_and_empty_inventory_has_no_gpu() {
    let decision = select_gpu(&json!({
        "dxgi":[adapter("first", "first", 8),adapter("second", "second", 8)],
        "vulkan":[]
    }))
    .unwrap();
    assert_eq!(decision.device.unwrap().name, "first");
    let decision = select_gpu(&json!({"dxgi":[],"vulkan":[]})).unwrap();
    assert!(decision.device.is_none());
    assert_eq!(decision.vram_total_bytes, 0);
    assert!(!decision.vulkan_available);
    assert!(select_gpu(&json!({"dxgi":[]})).is_err());
    assert!(select_gpu(&json!({"dxgi":{},"vulkan":[]})).is_err());
}

#[test]
fn device_probe_runs_without_model_and_handles_deadline_exit_and_malformed_json() {
    let decision = detect_gpu_at(fake_worker(), Duration::from_secs(5)).unwrap();
    assert_eq!(decision.vram_total_bytes, 8 * 1024 * 1024 * 1024);
    assert!(decision.vulkan_available);
    let directory = tempfile::tempdir().unwrap();
    for (mode, expected) in [
        ("stall", "timeout"),
        ("nonzero", "fatal"),
        ("malformed", "protocol"),
    ] {
        let executable = directory.path().join(format!("vlm-probe-{mode}.exe"));
        std::fs::copy(fake_worker(), &executable).unwrap();
        let timeout = if mode == "stall" {
            Duration::from_millis(150)
        } else {
            Duration::from_secs(5)
        };
        let result = detect_gpu_at(&executable, timeout);
        let correct = match expected {
            "timeout" => matches!(result, Err(VlmError::Timeout)),
            "fatal" => matches!(result, Err(VlmError::Fatal(_))),
            "protocol" => matches!(result, Err(VlmError::Protocol(_))),
            _ => unreachable!(),
        };
        assert!(correct, "probe mode {mode} was misclassified");
    }
}

/// Opt-in only: synthetic tests above never open a model or an input image.
/// All three paths must be explicit. No image or generated content is printed
/// or copied, including on a failed comparison.
#[test]
fn real_model_serve_matches_v1_0_on_all_26_images_when_configured() {
    let configured = [
        "MIV_VLM_TEST_MODEL",
        "MIV_VLM_TEST_MMPROJ",
        "MIV_VLM_TEST_IMAGES",
    ]
    .map(std::env::var_os);
    let [Some(model), Some(mmproj), Some(images)] = configured else {
        return;
    };
    let executable = std::env::var_os("MIV_VLM_WORKER")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/vlm-worker/mimageviewer-vlm.exe")
        });
    let baseline = std::env::var_os("MIV_VLM_TEST_BASELINE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"G:\home\vlm-eval\runs\20261102-V1-0\forward.jsonl"));
    let baseline = std::fs::read_to_string(baseline).expect("baseline must be readable");
    let records: Vec<serde_json::Value> = baseline
        .lines()
        .map(|line| serde_json::from_str(line).expect("baseline JSON must be valid"))
        .collect();
    assert_eq!(
        records.len(),
        26,
        "baseline must contain exactly 26 records"
    );
    let root = PathBuf::from(images);
    let gpu = detect_gpu_at(&executable, Duration::from_secs(30))
        .expect("real worker must report its GPU inventory");
    assert!(gpu.vulkan_available, "selected adapter must support Vulkan");
    let device_luid = std::env::var("MIV_VLM_TEST_LUID")
        .unwrap_or_else(|_| gpu.device.expect("selected GPU required").luid);
    let mut client = VlmWorkerClient::spawn_at(&executable).expect("real worker must start");
    client
        .load(&LoadRequest {
            model_path: model.into(),
            mmproj_path: mmproj.into(),
            device_luid,
            cpu_threads: 24,
        })
        .expect("real model must load");
    for (index, expected) in records.iter().enumerate() {
        let name = expected["image"]
            .as_str()
            .expect("baseline image name required");
        let relative = Path::new(name);
        assert!(
            relative
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_))),
            "baseline image path must stay inside the supplied input directory"
        );
        let rgb = image::open(root.join(relative))
            .expect("input image must be readable")
            .to_rgb8();
        let response = client
            .caption(index as u64, rgb.width(), rgb.height(), rgb.into_raw())
            .expect("real caption request must complete");
        let CaptionOutcome::Ok(actual) = response else {
            panic!("real caption failed at image index {index}");
        };
        let equal = serde_json::Value::String(actual.caption) == expected["caption"]
            && serde_json::Value::String(actual.text) == expected["text"]
            && serde_json::json!(actual.keywords) == expected["keywords"];
        assert!(
            equal,
            "serve output differs from V1-0 at image index {index}"
        );
    }
    client.shutdown();
}
