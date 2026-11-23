use super::*;
use std::time::Duration;

const FIRST: &str = "fn group(files) { [\"first\"] }\n";
const SECOND: &str = "fn group(files) { [\"second\"] }\n\n";

fn recv<T>(receiver: &mpsc::Receiver<T>) -> T {
    receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("スクリプトの仕事が 10 秒以内に完了すること")
}

fn enqueue_write(
    writer: &ScriptWriter,
    source: &str,
    expected: &str,
) -> mpsc::Receiver<Result<ScriptWriteOutcome, String>> {
    validate_source(source).unwrap();
    let (reply, receiver) = mpsc::channel();
    writer
        .enqueue(ScriptJob::Write {
            source: source.into(),
            expected_sha256: expected.into(),
            reply,
        })
        .unwrap();
    receiver
}

#[test]
fn script_document_missing_empty_and_raw_text_have_exact_hashes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("stack_rules.rhai");
    let missing = read_document_at(&path).unwrap();
    assert!(missing.builtin);
    assert_eq!(missing.source, DEFAULT_SCRIPT);
    assert_eq!(missing.sha256, sha256(DEFAULT_SCRIPT));
    for text in ["", " \r\n", "// あいう\r\nfn group(files) { [] }\r\n"] {
        fs::write(&path, text).unwrap();
        let document = read_document_at(&path).unwrap();
        assert!(!document.builtin);
        assert_eq!(document.source, text);
        assert_eq!(document.sha256, sha256(text));
    }
}

#[test]
fn script_document_rejects_overflow_invalid_utf8_and_read_errors() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("stack_rules.rhai");
    fs::write(&path, vec![b' '; MAX_SCRIPT_BYTES as usize + 1]).unwrap();
    assert!(read_document_at(&path).unwrap_err().contains("1 MiB"));
    fs::write(&path, [0xff]).unwrap();
    assert!(read_document_at(&path).unwrap_err().contains("UTF-8"));
    assert!(read_document_at(directory.path()).is_err());
}

#[test]
fn script_write_rejects_overflow_and_noncompile_before_enqueue() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("stack_rules.rhai");
    fs::write(&path, FIRST).unwrap();
    assert!(write_script(" ".repeat(MAX_SCRIPT_BYTES as usize + 1), sha256(FIRST)).is_err());
    assert!(write_script("fn group(files) {".into(), sha256(FIRST)).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), FIRST);
}

#[test]
fn script_fifo_ai_reset_open_and_stale_write_follow_enqueue_order() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("stack_rules.rhai");
    let worker_path = path.clone();
    let (release, blocked) = mpsc::channel();
    let (opened, observations) = mpsc::channel();
    let writer = ScriptWriter::start(
        move || {
            recv(&blocked);
            worker_path
        },
        move |path| {
            opened.send(fs::read_to_string(path).unwrap()).unwrap();
            Ok(())
        },
    )
    .unwrap();
    // worker はまだ動けない。この間に全件 enqueue できるので UI は完了を待たない。
    let written = enqueue_write(&writer, FIRST, &sha256(DEFAULT_SCRIPT));
    writer.enqueue(ScriptJob::Reset).unwrap();
    writer.enqueue(ScriptJob::Open).unwrap();
    let stale = enqueue_write(&writer, SECOND, &sha256(FIRST));
    assert!(!path.exists());
    release.send(()).unwrap();
    let outcome = recv(&written).unwrap();
    assert_eq!(outcome.sha256, sha256(FIRST));
    assert_eq!(outcome.before_lines, DEFAULT_SCRIPT.lines().count());
    assert_eq!(outcome.after_lines, FIRST.lines().count());
    assert_eq!(recv(&observations), DEFAULT_SCRIPT);
    assert!(recv(&stale).unwrap_err().contains("読み直して"));
    assert_eq!(fs::read_to_string(&path).unwrap(), DEFAULT_SCRIPT);
}

#[test]
fn script_fifo_open_creates_default_but_preserves_existing_empty_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested").join("stack_rules.rhai");
    let worker_path = path.clone();
    let (opened, observations) = mpsc::channel();
    let writer = ScriptWriter::start(
        move || worker_path,
        move |path| {
            opened.send(fs::read_to_string(path).unwrap()).unwrap();
            Ok(())
        },
    )
    .unwrap();
    writer.enqueue(ScriptJob::Open).unwrap();
    assert_eq!(recv(&observations), DEFAULT_SCRIPT);
    writer
        .write_validated(String::new(), sha256(DEFAULT_SCRIPT))
        .unwrap();
    writer.enqueue(ScriptJob::Open).unwrap();
    assert_eq!(recv(&observations), "");
    assert_eq!(fs::read_to_string(path).unwrap(), "");
}

#[test]
fn script_atomic_replace_keeps_old_open_handle_and_cleans_temporary_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("stack_rules.rhai");
    fs::write(&path, FIRST).unwrap();
    let mut previous_handle = File::open(&path).unwrap();
    replace_script(&path, SECOND).unwrap();
    let mut old_source = String::new();
    previous_handle.read_to_string(&mut old_source).unwrap();
    assert_eq!(old_source, FIRST);
    assert_eq!(fs::read_to_string(&path).unwrap(), SECOND);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    // 置き換え先がディレクトリの場合も、その中の既存データを消さない。
    let occupied = directory.path().join("occupied");
    fs::create_dir(&occupied).unwrap();
    fs::write(occupied.join("keep"), FIRST).unwrap();
    assert!(replace_script(&occupied, SECOND).is_err());
    assert_eq!(fs::read_to_string(occupied.join("keep")).unwrap(), FIRST);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn script_new_text_is_used_by_next_grouping_without_mutating_compiled_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("stack_rules.rhai");
    let worker_path = path.clone();
    let writer = ScriptWriter::start(move || worker_path, |_| Ok(())).unwrap();
    let member = crate::filename_stack::StackMember {
        path: "example.jpg".into(),
        mtime: 0,
        size: None,
        sort_meta: crate::settings::ListingSortMetadata::new(0, None),
        is_video: false,
    };
    writer
        .write_validated(FIRST.into(), sha256(DEFAULT_SCRIPT))
        .unwrap();
    let old = CompiledStackScript::compile(
        &read_document_at(&path).unwrap().source,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    writer
        .write_validated(SECOND.into(), sha256(FIRST))
        .unwrap();
    let current = CompiledStackScript::compile(
        &read_document_at(&path).unwrap().source,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert_eq!(
        old.group_keys(std::slice::from_ref(&member)).unwrap().keys,
        ["first"]
    );
    assert_eq!(current.group_keys(&[member]).unwrap().keys, ["second"]);
}

#[test]
fn script_write_acceptance_only_compiles_sleep_pad_and_top_level_code() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("stack_rules.rhai");
    let worker_path = path.clone();
    let writer = ScriptWriter::start(move || worker_path, |_| Ok(())).unwrap();
    let mut expected = sha256(DEFAULT_SCRIPT);
    // 保存時には試し実行しない。実行時の sleep/pad の拒否は §1.863 の既存テストが検証。
    for source in [
        "sleep(1000000); fn group(files) { [] }",
        "fn group(files) { sleep(0); [] }",
        "fn group(files) { let s = \"a\"; s.pad(8, \"\"); [] }",
        "fn group(files) { let a = []; a.pad(9223372036854775807, [0]); [] }",
    ] {
        let result = recv(&enqueue_write(&writer, source, &expected)).unwrap();
        expected = result.sha256;
        assert_eq!(fs::read_to_string(&path).unwrap(), source);
    }
}
