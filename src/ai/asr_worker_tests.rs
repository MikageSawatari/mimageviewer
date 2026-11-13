use super::super::vlm_worker::tests::fake_worker;
use super::*;

fn new_client(mode: &str) -> AsrWorkerClient {
    let args: Vec<_> = ["--asr-serve", "--fake-mode", mode]
        .into_iter()
        .map(OsString::from)
        .collect();
    let mut client = AsrWorkerClient::start_with(fake_worker(), &args).unwrap();
    client.hello().unwrap();
    client
}
fn load_request() -> LoadRequest {
    LoadRequest {
        model_path: "model.bin".into(),
        device_luid: "00000000:00000001".into(),
    }
}
fn loaded(mode: &str) -> (AsrInput, AsrOutput) {
    let mut client = new_client(mode);
    client.load(&load_request()).unwrap();
    client.split().unwrap()
}

#[test]
fn asr_split_receives_windows_and_reuses_model_for_multiple_inputs() {
    let (mut input, mut output) = loaded("ok");
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || {
            for id in [41, 42] {
                input.begin(id).unwrap();
                input.write_pcm(&[13; 3200]).unwrap();
                input.end().unwrap();
            }
            // writer の Drop も kill するので結果の終了まで所有を返しておく。
            input
        });
        for id in [41, 42] {
            assert_eq!(output.recv().unwrap(), AsrEvent::Begin { request_id: id });
            assert_eq!(
                output.recv().unwrap(),
                AsrEvent::Progress {
                    request_id: id,
                    processed_ms: 100
                }
            );
            let AsrEvent::Window(window) = output.recv().unwrap() else {
                panic!("missing window")
            };
            assert_eq!(window.segments[0].text, "日本語");
            assert_eq!(window.join_ms, 100);
            assert_eq!(
                output.recv().unwrap(),
                AsrEvent::End {
                    request_id: id,
                    duration_ms: 100,
                    utf8_discarded_bytes: 0,
                    utf8_discarded_fragments: 0
                }
            );
        }
        let _input = writer.join().unwrap();
    });
}

#[test]
fn asr_invalid_utf8_and_backwards_timestamps_retire_the_process() {
    for mode in ["invalid-utf8", "backwards"] {
        let (mut input, mut output) = loaded(mode);
        input.begin(7).unwrap();
        assert!(matches!(output.recv().unwrap(), AsrEvent::Begin { .. }));
        input.write_pcm(&[0; 3200]).unwrap();
        input.end().unwrap();
        if mode == "backwards" {
            assert!(matches!(output.recv().unwrap(), AsrEvent::Progress { .. }));
        }
        let error = output.recv().unwrap_err();
        if mode == "invalid-utf8" {
            assert!(matches!(
                error,
                AsrError::Transport(WorkerError::Protocol(_))
            ));
        } else {
            assert!(matches!(error, AsrError::Protocol(_)));
        }
        assert!(input.kill_handle().is_killed());
    }
}

#[test]
fn asr_begin_response_must_match_the_input_request() {
    let (mut input, mut output) = loaded("wrong-begin-id");
    input.begin(123).unwrap();
    assert!(matches!(output.recv(), Err(AsrError::Protocol(_))));
    assert!(input.kill_handle().is_killed());
}

#[test]
fn asr_load_keeps_the_selected_luid_when_enumeration_order_changes() {
    let mut client = new_client("load-reordered-gpu");
    let result = client.load(&load_request()).unwrap();
    assert_eq!(result.device.physical_index, 7);
    assert_eq!(result.device.luid, load_request().device_luid);
    let mut client = new_client("wrong-loaded-luid");
    assert!(matches!(
        client.load(&load_request()),
        Err(AsrError::Protocol(_))
    ));
    assert!(client.kill_handle().is_killed());
}

#[test]
fn asr_early_end_and_discarding_results_does_not_block_the_next_input() {
    let (mut input, mut output) = loaded("ok");
    for id in [1, 2] {
        input.begin(id).unwrap();
        input.write_pcm(&[0; 3200]).unwrap();
        // 抽出が途中で失敗した呼出し元も、ここでendを送り窓を保存せず捨てられる。
        input.end().unwrap();
        assert!(matches!(output.recv().unwrap(), AsrEvent::Begin { .. }));
        assert!(matches!(output.recv().unwrap(), AsrEvent::Progress { .. }));
        assert!(matches!(output.recv().unwrap(), AsrEvent::Window(_)));
        assert!(matches!(output.recv().unwrap(),AsrEvent::End{request_id,..} if request_id==id));
        assert!(!input.kill_handle().is_killed());
    }
}

#[test]
fn asr_receives_discard_counts_and_rejects_wrong_counts_or_legacy_failure() {
    for mode in [
        "repair-counts",
        "wrong-total",
        "invalid-counts",
        "wrong-end-duration",
        "legacy-item-failure",
    ] {
        let (mut input, mut output) = loaded(mode);
        input.begin(1).unwrap();
        input.write_pcm(&[0; 3200]).unwrap();
        input.end().unwrap();
        assert!(matches!(output.recv().unwrap(), AsrEvent::Begin { .. }));
        if mode == "legacy-item-failure" {
            assert!(matches!(output.recv(), Err(AsrError::Protocol(_))));
            continue;
        }
        assert!(matches!(output.recv().unwrap(), AsrEvent::Progress { .. }));
        if mode == "invalid-counts" {
            assert!(matches!(output.recv(), Err(AsrError::Protocol(_))));
            continue;
        }
        let AsrEvent::Window(window) = output.recv().unwrap() else {
            panic!("window required");
        };
        if mode == "repair-counts" {
            assert_eq!(
                (window.utf8_discarded_bytes, window.utf8_discarded_fragments),
                (2, 1)
            );
            assert_eq!(
                output.recv().unwrap(),
                AsrEvent::End {
                    request_id: 1,
                    duration_ms: 100,
                    utf8_discarded_bytes: 2,
                    utf8_discarded_fragments: 1
                }
            );
        } else {
            assert!(matches!(output.recv(), Err(AsrError::Protocol(_))));
        }
    }
}

#[test]
fn asr_counter_overflow_and_missing_window_fields_are_rejected() {
    let mut count = DiscardCount::checked(u64::MAX, u64::MAX).unwrap();
    assert!(count.add(DiscardCount::checked(1, 1).unwrap()).is_err());
    for (bytes, fragments) in [(0, 1), (1, 0), (1, 2)] {
        assert!(DiscardCount::checked(bytes, fragments).is_err());
    }
    let (mut input, mut output) = loaded("ok");
    input.begin(1).unwrap();
    let frame = |header| Frame {
        header,
        bytes: vec![],
    };
    output
        .accept(frame(json!({"type":"begin","request_id":1,"status":"ok"})))
        .unwrap();
    assert!(
        output
            .accept(frame(json!({"type":"window","request_id":1,"start_ms":0,
        "read_end_ms":10,"join_ms":10,"final":true,"segments":[]})))
            .is_err()
    );
}

#[test]
fn asr_worker_exit_preserves_queued_fatal_or_framing_error() {
    for mode in ["fatal-exit", "invalid-utf8-exit"] {
        let (mut input, mut output) = loaded(mode);
        input.begin(123).unwrap();
        assert!(matches!(output.recv().unwrap(), AsrEvent::Begin { .. }));
        input.end().unwrap();
        // Exit/EOF が受信より先に完了する順序を明示的に作る。
        use std::os::windows::io::AsRawHandle as _;
        use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::WaitForSingleObject;
        assert_eq!(
            unsafe {
                WaitForSingleObject(
                    HANDLE(output.owner._child.process_handle().as_raw_handle()),
                    5000,
                )
            },
            WAIT_OBJECT_0
        );
        // reader の EOF 観測と main thread の競争に依存させない。
        // すでに終了した子の kill flag を明示し、旧 recv の早期拒否を必ず通す。
        input.kill_handle().kill().unwrap();
        let error = output.recv().unwrap_err();
        if mode == "fatal-exit" {
            assert_eq!(
                error,
                AsrError::Worker {
                    code: "inference".into(),
                    message: "synthetic failure".into()
                }
            );
        } else {
            assert!(matches!(
                error,
                AsrError::Transport(WorkerError::Protocol(_))
            ));
        }
        assert!(input.kill_handle().is_killed());
    }
}

#[test]
fn asr_kill_releases_reader_and_blocked_pcm_writer() {
    for mode in ["stall-read", "stall-write"] {
        let (mut input, mut output) = loaded(mode);
        let kill = input.kill_handle();
        input.begin(1).unwrap();
        if mode == "stall-read" {
            assert!(matches!(output.recv().unwrap(), AsrEvent::Begin { .. }));
        }
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = if mode == "stall-read" {
                output.recv().map(|_| ())
            } else {
                let _output = output;
                let pcm = vec![0; MAX_PCM_BYTES];
                loop {
                    if let Err(e) = input.write_pcm(&pcm) {
                        break Err(e);
                    }
                }
            };
            done_tx.send(result).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            done_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        kill.kill().unwrap();
        assert!(
            done_rx
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .is_err()
        );
        thread.join().unwrap();
    }
}

#[test]
fn asr_gpu_selection_keeps_integrated_and_excludes_software_and_preserves_ties() {
    let adapter = |id: &str, memory, software| json!({"name":id,"luid":id,"dedicated_bytes":memory,"software":software});
    for (adapters, expected) in [
        (
            vec![
                adapter("integrated", 0, false),
                adapter("4090", 24_u64 << 30, false),
            ],
            Some("4090"),
        ),
        (vec![adapter("integrated", 0, false)], Some("integrated")),
        (vec![adapter("software", 64_u64 << 30, true)], None),
        (
            vec![
                adapter("first", 8_u64 << 30, false),
                adapter("second", 8_u64 << 30, false),
            ],
            Some("first"),
        ),
    ] {
        let vulkan: Vec<_> = adapters.iter().enumerate().map(|(i,a)| json!({"luid":a["luid"],"physical_index":i,"luid_valid":true,"mapping":"pci-id"})).collect();
        let result = select_gpu(&json!({"dxgi":adapters,"vulkan":vulkan})).unwrap();
        assert_eq!(result.device.as_ref().map(|a| a.luid.as_str()), expected);
        assert_eq!(result.is_eligible(), expected.is_some());
    }
}

#[test]
fn asr_framing_preserves_pcm_bytes_and_rejects_invalid_utf8() {
    let frame = encode_frame(
        &json!({"type":"pcm","request_id":1}),
        &[0, 13, 10, 26, 255, 128],
    )
    .unwrap();
    assert_eq!(
        read_frame(&mut frame.as_slice()).unwrap().bytes,
        [0, 13, 10, 26, 255, 128]
    );
    let raw = [7, 0, 0, 0, 3, 0, 0, 0, b'"', 255, b'"'];
    assert!(read_frame(&mut raw.as_slice()).is_err());
}

#[test]
fn asr_rejects_nonmonotonic_segments_and_broken_window_continuity() {
    for segments in [
        json!([{"start_ms":5,"end_ms":10,"text":"a"},{"start_ms":4,"end_ms":12,"text":"b"}]),
        json!([{"start_ms":1,"end_ms":10,"text":"a"},{"start_ms":2,"end_ms":9,"text":"b"}]),
        json!([{"start_ms":-1,"end_ms":10,"text":"a"}]),
    ] {
        let (mut input, mut output) = loaded("ok");
        input.begin(1).unwrap();
        let frame = |header| Frame {
            header,
            bytes: vec![],
        };
        output
            .accept(frame(json!({"type":"begin","request_id":1,"status":"ok"})))
            .unwrap();
        assert!(
            output
                .accept(frame(json!({"type":"window","request_id":1,"start_ms":0,
            "read_end_ms":100,"join_ms":50,"final":false,"utf8_discarded_bytes":0,"utf8_discarded_fragments":0,"segments":segments})))
                .is_err()
        );
    }
    let (mut input, mut output) = loaded("ok");
    input.begin(1).unwrap();
    let frame = |header| Frame {
        header,
        bytes: vec![],
    };
    output
        .accept(frame(json!({"type":"begin","request_id":1,"status":"ok"})))
        .unwrap();
    output
        .accept(frame(json!({"type":"window","request_id":1,"start_ms":0,
        "read_end_ms":100,"join_ms":50,"final":false,"utf8_discarded_bytes":0,"utf8_discarded_fragments":0,"segments":[]})))
        .unwrap();
    assert!(
        output
            .accept(frame(json!({"type":"window","request_id":1,"start_ms":49,
        "read_end_ms":120,"join_ms":120,"final":true,"utf8_discarded_bytes":0,"utf8_discarded_fragments":0,"segments":[]})))
            .is_err()
    );
}
