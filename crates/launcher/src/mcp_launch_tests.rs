use super::*;
use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

struct FakeChild<F>(F);
impl<F: FnMut() -> io::Result<i32>> WaitForExit for FakeChild<F> {
    fn wait_for_exit(&mut self) -> io::Result<i32> {
        (self.0)()
    }
}

struct FakeMarker(Rc<Cell<bool>>);
impl Drop for FakeMarker {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

fn runtime(temp: &tempfile::TempDir) -> PathBuf {
    temp.path().join("runtime").join(crate::VERSION)
}

fn assert_locked(path: &Path, name: &str) {
    assert!(
        runtime_locks::exclusive(path, name).is_err(),
        "{name} must stay locked"
    );
}

#[test]
fn only_leading_mcp_dispatches_and_remaining_arguments_are_verbatim() {
    for args in [vec![], vec!["--help", "--mcp"], vec!["--", "--mcp"]] {
        let args = args.into_iter().map(OsString::from).collect::<Vec<_>>();
        assert!(arguments(&args).is_none());
    }
    let args = [
        "--mcp",
        "--help",
        "--version",
        "--data-dir",
        "日本語 path",
        "--",
        "--mcp",
    ]
    .into_iter()
    .map(OsString::from)
    .collect::<Vec<_>>();
    assert_eq!(arguments(&args), Some(&args[1..]));
    assert_eq!(arguments(&["--mcp".into()]), Some([].as_slice()));
}

#[test]
fn mcp_only_extraction_and_child_lifetimes_follow_ownership_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let path = runtime(&temp);
    let marker_alive = Rc::new(Cell::new(true));
    let args = ["--data-dir", "a b", "--version", "--"].map(OsString::from);
    let result = run(
        &args,
        || Ok(path.clone()),
        || Ok(FakeMarker(marker_alive.clone())),
        |cmd| {
            assert!(marker_alive.get());
            assert_locked(&path, runtime_locks::IN_USE);
            assert_locked(&path, runtime_locks::EXTRACTION);
            assert_eq!(cmd.get_program(), path.join(crate::MCP_ASSET.0));
            assert_eq!(
                cmd.get_args().collect::<Vec<_>>(),
                args.iter().map(OsString::as_os_str).collect::<Vec<_>>()
            );
            assert_eq!(
                std::fs::read_dir(&path).unwrap().count(),
                2,
                "only MCP and its hash sidecar are extracted"
            );
            assert_eq!(
                crate::sha256_file_hex(&path.join(crate::MCP_ASSET.0)).unwrap(),
                crate::MCP_ASSET.2
            );
            Ok(FakeChild(|| {
                assert!(marker_alive.get());
                assert_locked(&path, runtime_locks::IN_USE);
                drop(runtime_locks::exclusive(&path, runtime_locks::EXTRACTION).unwrap());
                Ok(73)
            }))
        },
        |line| panic!("unexpected stderr: {line}"),
    );
    assert_eq!(result.unwrap(), 73);
    assert!(!marker_alive.get());
    assert!(runtime_locks::exclusive(&path, runtime_locks::IN_USE).is_ok());
    assert!(runtime_locks::exclusive(&path, runtime_locks::EXTRACTION).is_ok());
}

#[test]
fn mcp_hash_is_checked_even_with_current_sidecar_and_equal_length_corruption() {
    let temp = tempfile::tempdir().unwrap();
    let path = runtime(&temp);
    let launch = || {
        run(
            &[],
            || Ok(path.clone()),
            || Ok(()),
            |_| Ok(FakeChild(|| Ok(0))),
            |_| {},
        )
    };
    launch().unwrap();
    let executable = path.join(crate::MCP_ASSET.0);
    let mut corrupt = std::fs::read(&executable).unwrap();
    corrupt[0] ^= 0xff;
    std::fs::write(&executable, &corrupt).unwrap();
    assert_eq!(
        std::fs::read_to_string(crate::sidecar_hash_path(&executable)).unwrap(),
        crate::MCP_ASSET.2
    );
    launch().unwrap();
    assert_eq!(std::fs::read(executable).unwrap(), crate::MCP_ASSET.1);
    assert!(crate::requires_content_hash_on_launch(
        "MIMAGEVIEWER-MCP.EXE"
    ));
    assert!(
        crate::ASSETS
            .iter()
            .any(|asset| asset.0 == crate::MCP_ASSET.0)
    );
}

#[test]
fn marker_failure_warns_once_and_does_not_change_child_result() {
    let temp = tempfile::tempdir().unwrap();
    let mut stderr = Vec::new();
    let result = run(
        &[],
        || Ok(runtime(&temp)),
        || Err::<(), _>("marker\r\nunavailable".into()),
        |_| Ok(FakeChild(|| Ok(12))),
        |line| stderr.push(line.to_owned()),
    );
    assert_eq!(complete(result, |line| stderr.push(line.to_owned())), 12);
    assert_eq!(stderr.len(), 1);
    assert!(!stderr[0].contains(['\r', '\n']));
    assert!(stderr[0].contains("marker"));
}

#[test]
fn extraction_spawn_wait_and_runtime_failures_use_one_stderr_line_and_release_owners() {
    for failure in ["runtime", "extract", "spawn", "wait"] {
        let temp = tempfile::tempdir().unwrap();
        let path = runtime(&temp);
        if failure == "extract" {
            std::fs::create_dir_all(path.join(crate::MCP_ASSET.0)).unwrap();
        }
        let alive = Rc::new(Cell::new(true));
        let result = run(
            &[],
            || {
                if failure == "runtime" {
                    Err("runtime\nfailed".into())
                } else {
                    Ok(path.clone())
                }
            },
            || Ok(FakeMarker(alive.clone())),
            |_| {
                if matches!(failure, "runtime" | "extract") {
                    panic!("spawn reached after preparation error");
                }
                assert_locked(&path, runtime_locks::EXTRACTION);
                if failure == "spawn" {
                    return Err(io::Error::other("spawn\nfailed"));
                }
                Ok(FakeChild(|| {
                    assert_locked(&path, runtime_locks::IN_USE);
                    assert!(runtime_locks::exclusive(&path, runtime_locks::EXTRACTION).is_ok());
                    Err(io::Error::other("wait\nfailed"))
                }))
            },
            |line| panic!("unexpected marker warning: {line}"),
        );
        let mut stderr = Vec::new();
        assert_eq!(complete(result, |line| stderr.push(line.to_owned())), 1);
        assert_eq!(stderr.len(), 1);
        assert!(!stderr[0].contains(['\r', '\n']));
        assert!(!alive.get());
        if failure != "runtime" {
            assert!(runtime_locks::exclusive(&path, runtime_locks::IN_USE).is_ok());
            assert!(runtime_locks::exclusive(&path, runtime_locks::EXTRACTION).is_ok());
        }
    }
}

#[test]
fn completion_preserves_all_child_exit_codes_without_output() {
    for code in [0, 1, 17, -1, i32::MIN] {
        assert_eq!(
            complete(Ok(code), |_| panic!("child result must not print")),
            code
        );
    }
}
