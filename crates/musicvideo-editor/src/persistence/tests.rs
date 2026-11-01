use super::*;
use std::time::Duration;

fn event(service: &Persistence) -> Event {
    service.rx.recv_timeout(Duration::from_secs(10)).unwrap()
}

fn recent(service: &Persistence) -> Vec<PathBuf> {
    match event(service) {
        Event::Recent(paths) => paths,
        Event::Notice(error) => panic!("{error}"),
    }
}

#[test]
fn p4c_recent_corrupt_and_unreadable_are_empty_and_logged() {
    for unreadable in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("musicvideo/recent-projects.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        if unreadable {
            fs::create_dir(&path).unwrap();
        } else {
            fs::write(&path, b"{broken").unwrap();
        }
        let (service, guard) = Persistence::start(Some(temp.path().into())).unwrap();
        assert!(recent(&service).is_empty());
        drop(guard);
        assert!(
            fs::read_to_string(temp.path().join("logs/musicvideo.log"))
                .unwrap()
                .contains("最近使ったプロジェクト")
        );
    }
}

#[test]
fn p4c_recent_limit_duplicates_absolute_and_remove_missing() {
    let temp = tempfile::tempdir().unwrap();
    let (service, guard) = Persistence::start(Some(temp.path().into())).unwrap();
    assert!(recent(&service).is_empty());
    for n in 0..12 {
        service
            .tx
            .send(Command::Add(temp.path().join(format!("{n}.json"))))
            .unwrap();
        let paths = recent(&service);
        assert_eq!(paths.len(), (n + 1).min(10));
        assert!(paths.iter().all(|p| p.is_absolute()));
    }
    let duplicate = temp.path().join("5.json");
    service.tx.send(Command::Add(duplicate.clone())).unwrap();
    let paths = recent(&service);
    assert_eq!(paths.len(), 10);
    assert_eq!(paths[0], duplicate);
    assert_eq!(paths.iter().filter(|p| **p == duplicate).count(), 1);
    assert!(!duplicate.exists());
    service.tx.send(Command::Remove(duplicate.clone())).unwrap();
    assert!(!recent(&service).contains(&duplicate));
    service
        .tx
        .send(Command::Add("relative.json".into()))
        .unwrap();
    let paths = recent(&service);
    assert_eq!(
        paths[0],
        crate::project_file::absolute_lexical(Path::new("relative.json")).unwrap()
    );
    drop(guard);
    assert_eq!(
        read_recent(&temp.path().join("musicvideo/recent-projects.json")).unwrap(),
        paths
    );
    assert_eq!(
        fs::read_dir(temp.path().join("musicvideo"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn p4c_recent_read_sanitizes_duplicates_limit_and_rejects_relative() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("recent.json");
    let entries = (0..15)
        .map(|n| temp.path().join(format!("{}.json", n / 2)))
        .collect::<Vec<_>>();
    fs::write(&path, serde_json::to_vec(&entries).unwrap()).unwrap();
    let loaded = read_recent(&path).unwrap();
    assert_eq!(loaded.len(), 8);
    let entries = (0..15)
        .map(|n| temp.path().join(format!("{n}.json")))
        .collect::<Vec<_>>();
    fs::write(&path, serde_json::to_vec(&entries).unwrap()).unwrap();
    assert_eq!(read_recent(&path).unwrap(), entries[..10]);
    fs::write(&path, br#"["relative.json"]"#).unwrap();
    assert!(read_recent(&path).is_err());
}

#[test]
fn p4c_recent_concurrent_atomic_writes_leave_one_complete_list() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("musicvideo/recent-projects.json");
    let lists = [
        vec![temp.path().join("a.json")],
        vec![temp.path().join("b.json")],
    ];
    let successes = std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for list in &lists {
            let path = &path;
            workers.push(scope.spawn(move || {
                let mut successes = 0;
                for _ in 0..20 {
                    // Native replacement can reject a simultaneous replacement on
                    // Windows. The production worker reports it; D10 adds no retry.
                    if write_recent(path, list).is_ok() {
                        successes += 1;
                    }
                }
                successes
            }));
        }
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .sum::<usize>()
    });
    assert!(successes > 0);
    let saved = read_recent(&path).unwrap();
    assert!(lists.contains(&saved));
    write_recent(&path, &lists[1]).unwrap();
    assert_eq!(read_recent(&path).unwrap(), lists[1]);
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn p4c_without_data_dir_worker_ignores_recent_and_creates_no_files() {
    let temp = tempfile::tempdir().unwrap();
    let (service, guard) = Persistence::start(None).unwrap();
    service
        .tx
        .send(Command::Add(temp.path().join("project.json")))
        .unwrap();
    service
        .tx
        .send(Command::Remove(temp.path().join("project.json")))
        .unwrap();
    service.tx.send(Command::Log("stderr only".into())).unwrap();
    service
        .tx
        .send(Command::Panic("panic stderr only".into()))
        .unwrap();
    drop(guard);
    assert!(service.rx.try_iter().next().is_none());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    let diagnostics = Diagnostics::open(None).unwrap();
    assert!(diagnostics.log.is_none());
    assert!(diagnostics.panic_path.is_none());
}

#[test]
fn p4c_logs_rotate_only_over_limit_append_and_preserve_miv_logs() {
    for size in [LOG_LIMIT, LOG_LIMIT + 1] {
        let temp = tempfile::tempdir().unwrap();
        let logs = temp.path().join("logs");
        fs::create_dir_all(&logs).unwrap();
        for path in ["mimageviewer.log", "panic.log", "musicvideo.log.1"] {
            fs::write(logs.join(path), b"keep").unwrap();
        }
        fs::write(temp.path().join("settings.db"), b"settings sentinel").unwrap();
        fs::write(temp.path().join("catalog.db"), b"cache sentinel").unwrap();
        let file = File::create(logs.join("musicvideo.log")).unwrap();
        file.set_len(size).unwrap();
        drop(file);
        let (service, guard) = Persistence::start(Some(temp.path().into())).unwrap();
        recent(&service);
        service.tx.send(Command::Log("normal line".into())).unwrap();
        service
            .tx
            .send(Command::Panic("panic line".into()))
            .unwrap();
        drop(guard);
        assert_eq!(fs::read(logs.join("mimageviewer.log")).unwrap(), b"keep");
        assert_eq!(fs::read(logs.join("panic.log")).unwrap(), b"keep");
        assert_eq!(
            fs::read(temp.path().join("settings.db")).unwrap(),
            b"settings sentinel"
        );
        assert_eq!(
            fs::read(temp.path().join("catalog.db")).unwrap(),
            b"cache sentinel"
        );
        let log = fs::read(logs.join("musicvideo.log")).unwrap();
        assert!(log.ends_with(b"normal line\n"));
        assert_eq!(
            log.len() as u64,
            if size > LOG_LIMIT { 12 } else { size + 12 }
        );
        assert_eq!(
            fs::read_to_string(logs.join("musicvideo-panic.log")).unwrap(),
            "panic line\n"
        );
        if size > LOG_LIMIT {
            assert_eq!(
                logs.join("musicvideo.log.1").metadata().unwrap().len(),
                size
            );
        } else {
            assert_eq!(fs::read(logs.join("musicvideo.log.1")).unwrap(), b"keep");
        }
    }
}

#[test]
fn p4c_panic_hook_outputs_only_editor_panic_log() {
    // A child test harness isolates the global hook from parallel parent tests.
    const CHILD: &str = "MIV_P4C_PANIC_TEST_DIR";
    if let Some(dir) = std::env::var_os(CHILD) {
        let (service, guard) = Persistence::start(Some(dir.into())).unwrap();
        recent(&service);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(panic_hook(service.tx.clone())));
        let result = std::panic::catch_unwind(|| panic!("p4c hook marker"));
        std::panic::set_hook(previous);
        assert!(result.is_err());
        drop(guard);
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "persistence::tests::p4c_panic_hook_outputs_only_editor_panic_log",
            "--nocapture",
        ])
        .env(CHILD, temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fs::read_to_string(temp.path().join("logs/musicvideo-panic.log"))
            .unwrap()
            .contains("p4c hook marker")
    );
    assert!(!temp.path().join("logs/panic.log").exists());
    assert!(!temp.path().join("logs/mimageviewer.log").exists());
}

#[test]
fn p4c_log_failure_notifies_without_recovery() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("logs"), b"blocked").unwrap();
    let (service, guard) = Persistence::start(Some(temp.path().into())).unwrap();
    assert!(
        matches!(event(&service), Event::Notice(message) if message.contains("ログを開けません"))
    );
    assert!(recent(&service).is_empty());
    drop(guard);
    assert_eq!(fs::read(temp.path().join("logs")).unwrap(), b"blocked");
}
