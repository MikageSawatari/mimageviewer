//! 診断ZIPの非同期書き出し。UIは要求と結果の投影だけを扱う。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

#[derive(Clone, Debug)]
pub(crate) struct SavedExport {
    pub path: PathBuf,
    pub evidence_incomplete: bool,
}

/// PreferencesStateへの投影。worker/結果の所有者はAppだけ。
#[derive(Clone, Debug, Default)]
pub(crate) enum ExportView {
    #[default]
    Idle,
    Running,
    Finished(Result<SavedExport, String>),
}

#[derive(Default)]
pub(crate) enum ExportOwner {
    #[default]
    Idle,
    Running {
        receiver: mpsc::Receiver<Result<SavedExport, String>>,
        retired: Arc<AtomicBool>,
    },
    Finished(Result<SavedExport, String>),
}

impl ExportOwner {
    pub fn view(&self) -> ExportView {
        match self {
            Self::Idle => ExportView::Idle,
            Self::Running { .. } => ExportView::Running,
            Self::Finished(result) => ExportView::Finished(result.clone()),
        }
    }

    pub fn submit(&mut self, ctx: &egui::Context) {
        self.submit_worker(ctx, export_diagnostics_zip);
    }

    fn submit_worker(
        &mut self,
        ctx: &egui::Context,
        work: impl FnOnce(&AtomicBool) -> Result<SavedExport, String> + Send + 'static,
    ) {
        if matches!(self, Self::Running { .. }) {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let retired = Arc::new(AtomicBool::new(false));
        let worker_retired = retired.clone();
        let ctx = ctx.clone();
        match std::thread::Builder::new()
            .name("diagnostics-export".into())
            .spawn(move || {
                let result = work(&worker_retired);
                if !worker_retired.load(Ordering::Acquire) {
                    let _ = sender.send(result);
                    ctx.request_repaint();
                }
            }) {
            Ok(_) => *self = Self::Running { receiver, retired },
            Err(error) => {
                *self = Self::Finished(Err(format!("書き出しworkerを開始できません: {error}")))
            }
        }
    }

    pub fn poll(&mut self) {
        if let Self::Running { receiver, .. } = self {
            match receiver.try_recv() {
                Ok(result) => *self = Self::Finished(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    *self = Self::Finished(Err("書き出しworkerが終了しました".into()));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    pub fn retire(&mut self) {
        if let Self::Running { retired, .. } = self {
            retired.store(true, Ordering::Release);
        }
        *self = Self::Idle;
    }
}

/// Worker専用。barrier失敗時は出力作成・ログ読取りに進まない。
fn export_diagnostics_zip(retired: &AtomicBool) -> Result<SavedExport, String> {
    let snapshot = crate::log_writer::diagnostics_snapshot().map_err(|error| {
        format!("ログの保存完了を確認できないため、書き出しに失敗しました: {error}")
    })?;
    let logs_dir = crate::data_dir::logs_dir();
    let out_dir = desktop_dir().unwrap_or_else(|| logs_dir.clone());
    static NEXT_EXPORT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let zip_path = out_dir.join(format!(
        "mImageViewer_diag_{}_{}_{}.zip",
        local_timestamp(),
        std::process::id(),
        NEXT_EXPORT.fetch_add(1, Ordering::Relaxed)
    ));
    export_diagnostics_zip_from(&logs_dir, &zip_path, snapshot, retired)
}

struct PartialArchive<'a> {
    path: &'a Path,
    complete: bool,
}
impl Drop for PartialArchive<'_> {
    fn drop(&mut self) {
        if !self.complete {
            let _ = std::fs::remove_file(self.path);
        }
    }
}

/// 必須ログはwriterが確定した同一fileのprefixだけを保存する。
fn export_diagnostics_zip_from(
    logs_dir: &Path,
    zip_path: &Path,
    snapshot: crate::log_writer::ExportSnapshot,
    retired: &AtomicBool,
) -> Result<SavedExport, String> {
    if retired.load(Ordering::Acquire) {
        return Err("書き出しを終了しました".into());
    }
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(zip_path)
        .map_err(|e| format!("zip ファイルを作成できません ({}): {e}", zip_path.display()))?;
    let mut partial = PartialArchive {
        path: zip_path,
        complete: false,
    };
    let mut zw = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let opts: zip::write::FileOptions<'static, ()> = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);
    let incomplete = snapshot.checkpoint.evidence_incomplete();
    let manifest = serde_json::json!({
        "schema": 1,
        "evidence_status": if incomplete { "incomplete" } else { "complete" },
        "incomplete_reasons": if incomplete { vec!["session_records_dropped"] } else { vec![] },
        "checkpoint": snapshot.checkpoint,
        "snapshots": {
            "normal": {"entry": "logs/mimageviewer.log", "path": snapshot.normal.path, "length": snapshot.normal.len},
            "perf": snapshot.perf.as_ref().map(|perf| serde_json::json!({"entry": "logs/perf_events.jsonl", "path": perf.path, "length": perf.len})),
        },
        "perf_witness_required": snapshot.perf.is_some(),
    });
    zw.start_file("delivery_manifest.json", opts)
        .map_err(|e| format!("zip 書き込み失敗: {e}"))?;
    serde_json::to_writer_pretty(&mut zw, &manifest)
        .map_err(|e| format!("manifest 書き込み失敗: {e}"))?;
    zw.start_file("system_info.txt", opts)
        .map_err(|e| format!("zip 書き込み失敗: {e}"))?;
    zw.write_all(system_info_text().as_bytes())
        .map_err(|e| format!("zip 書き込み失敗: {e}"))?;

    fn write_snapshot<W: Write + std::io::Seek>(
        zw: &mut zip::ZipWriter<W>,
        name: &str,
        snapshot: crate::log_writer::SnapshotFile,
        opts: zip::write::FileOptions<'static, ()>,
    ) -> Result<(), String> {
        zw.start_file(name, opts)
            .map_err(|e| format!("zip 書き込み失敗: {e}"))?;
        let copied = std::io::copy(&mut snapshot.file.take(snapshot.len), zw)
            .map_err(|e| format!("必須ログの読み取り・保存に失敗しました: {e}"))?;
        if copied != snapshot.len {
            return Err("必須ログの確定範囲を読み取れませんでした".into());
        }
        Ok(())
    }
    let normal_path = snapshot.normal.path.clone();
    let perf_path = snapshot.perf.as_ref().map(|perf| perf.path.clone());
    write_snapshot(&mut zw, "logs/mimageviewer.log", snapshot.normal, opts)?;
    if let Some(perf) = snapshot.perf {
        write_snapshot(&mut zw, "logs/perf_events.jsonl", perf, opts)?;
    }

    // 補助ログは従来のbest-effort収集。旧perf世代とstale active perfは代用しない。
    if let Ok(entries) = std::fs::read_dir(logs_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path == zip_path || path == normal_path || perf_path.as_ref() == Some(&path) {
                continue;
            }
            if !entry.file_type().is_ok_and(|ft| ft.is_file()) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "perf_events.jsonl"
                || name == "mimageviewer.log"
                || is_rotated_perf_generation(&name)
            {
                continue;
            }
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            zw.start_file(format!("logs/{name}"), opts)
                .map_err(|e| format!("zip 書き込み失敗: {e}"))?;
            zw.write_all(&bytes)
                .map_err(|e| format!("zip 書き込み失敗: {e}"))?;
        }
    }
    let mut output = zw
        .finish()
        .map_err(|e| format!("zip のクローズに失敗: {e}"))?;
    output
        .flush()
        .map_err(|e| format!("zip のflushに失敗: {e}"))?;
    drop(output);
    if retired.load(Ordering::Acquire) {
        return Err("書き出しを終了しました".into());
    }
    partial.complete = true;
    Ok(SavedExport {
        path: zip_path.to_path_buf(),
        evidence_incomplete: incomplete,
    })
}

/// `perf_events.<N>.jsonl` (N >= 1、rotate 済みの過去世代) かどうか。
/// 現行の `perf_events.jsonl` は false (= zip に含める)。
fn is_rotated_perf_generation(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("perf_events.") else {
        return false;
    };
    let Some(num) = rest.strip_suffix(".jsonl") else {
        return false;
    };
    !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit())
}

fn system_info_text() -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "mImageViewer version: {}\n",
        env!("CARGO_PKG_VERSION")
    ));
    s.push_str(&format!("exported: {}\n", local_timestamp()));
    s.push_str(&format!("os: {}\n", std::env::consts::OS));
    s.push_str(&format!("arch: {}\n", std::env::consts::ARCH));
    if let Some(vendor) = crate::gpu_info::query_primary_gpu_vendor() {
        s.push_str(&format!("gpu_vendor: {vendor:?}\n"));
    }
    if let Some(vram) = crate::gpu_info::query_vram_summary_mib() {
        s.push_str(&format!("gpu_vram_mib: {vram}\n"));
    }
    s
}

#[cfg(windows)]
fn local_timestamp() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let st = unsafe { GetLocalTime() };
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
    )
}

#[cfg(not(windows))]
fn local_timestamp() -> String {
    // 非 Windows は開発用ビルドのみ。epoch 秒でファイル名がユニークになれば十分。
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

#[cfg(windows)]
fn desktop_dir() -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    unsafe {
        // OneDrive 等でリダイレクトされたデスクトップも正しく解決するため、
        // USERPROFILE 連結ではなく Known Folder API を使う。
        let pwstr = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None).ok()?;
        let path = pwstr.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(pwstr.0 as *const core::ffi::c_void));
        path.filter(|p| p.is_dir())
    }
}

#[cfg(not(windows))]
fn desktop_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Desktop"))
        .filter(|p| p.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_entry(archive: &mut zip::ZipArchive<std::fs::File>, name: &str) -> String {
        let mut text = String::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }

    #[test]
    fn zip_real_delivery_preserves_tail_witness_checkpoint_and_snapshot_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let writer = crate::log_writer::test_support::writer(&logs, 32, 65536);
        let enqueue = writer.pause_dequeue();
        assert!(writer.submit_normal("normal_before_export"));
        assert!(
            writer.submit_perf(serde_json::json!({"cat":"grid", "kind":"buffered_before_export"}))
        );
        drop(enqueue);
        let snapshot = writer.snapshot().unwrap();
        let normal_len = snapshot.normal.len;
        let perf_len = snapshot.perf.as_ref().unwrap().len;
        let request_id = snapshot.checkpoint.request_id.clone();
        // Later appends are outside the sealed archive prefix.
        let enqueue = writer.pause_dequeue();
        assert!(writer.submit_normal("normal_after_export"));
        assert!(writer.submit_perf(serde_json::json!({"cat":"grid", "kind":"after_export"})));
        drop(enqueue);
        let after = writer.snapshot().unwrap();
        assert!(after.normal.len > normal_len);
        assert!(after.perf.as_ref().unwrap().len > perf_len);
        drop(after);
        std::fs::write(logs.join("perf_events.1.jsonl"), "old generation").unwrap();
        std::fs::write(logs.join("panic.log"), "independent panic evidence").unwrap();
        let path = tmp.path().join("diagnostics.zip");
        let saved =
            export_diagnostics_zip_from(&logs, &path, snapshot, &AtomicBool::new(false)).unwrap();
        assert!(!saved.evidence_incomplete);
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let normal = read_entry(&mut zip, "logs/mimageviewer.log");
        let perf = read_entry(&mut zip, "logs/perf_events.jsonl");
        assert_eq!(normal.len() as u64, normal_len);
        assert_eq!(perf.len() as u64, perf_len);
        assert!(normal.contains("normal_before_export"));
        assert!(!normal.contains("normal_after_export"));
        assert!(normal.contains(&request_id));
        assert!(perf.contains("buffered_before_export"));
        assert!(perf.contains("export_requested"));
        assert!(perf.contains(&request_id));
        assert!(perf.contains("drain_checkpoint"));
        assert!(!perf.contains("after_export"));
        assert!(zip.by_name("logs/perf_events.1.jsonl").is_err());
        assert!(read_entry(&mut zip, "logs/panic.log").contains("independent panic evidence"));
        let manifest: serde_json::Value =
            serde_json::from_str(&read_entry(&mut zip, "delivery_manifest.json")).unwrap();
        assert_eq!(manifest["evidence_status"], "complete");
        assert_eq!(manifest["checkpoint"]["request_id"], request_id);
        assert!(writer.shutdown());
    }

    #[test]
    fn zip_real_delivery_prior_drop_saves_warning_and_short_snapshot_fails_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let writer = crate::log_writer::test_support::writer(&logs, 32, 65536);
        writer.force_drop();
        let snapshot = writer.snapshot().unwrap();
        assert!(snapshot.checkpoint.evidence_incomplete());
        let path = tmp.path().join("incomplete.zip");
        let saved =
            export_diagnostics_zip_from(&logs, &path, snapshot, &AtomicBool::new(false)).unwrap();
        assert!(saved.evidence_incomplete);
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_str(&read_entry(&mut zip, "delivery_manifest.json")).unwrap();
        assert_eq!(manifest["evidence_status"], "incomplete");

        let mut snapshot = writer.snapshot().unwrap();
        snapshot.normal.len += 1_000_000;
        let failed_path = tmp.path().join("failed.zip");
        assert!(
            export_diagnostics_zip_from(&logs, &failed_path, snapshot, &AtomicBool::new(false))
                .is_err()
        );
        assert!(!failed_path.exists(), "worker removes partial archives");
        assert!(writer.shutdown());
    }

    #[test]
    fn rotated_perf_generations_are_detected() {
        assert!(is_rotated_perf_generation("perf_events.1.jsonl"));
        assert!(is_rotated_perf_generation("perf_events.42.jsonl"));
        for name in [
            "perf_events.jsonl",
            "mimageviewer.log",
            "panic.log",
            "perf_events.abc.jsonl",
            "perf_events..jsonl",
        ] {
            assert!(!is_rotated_perf_generation(name));
        }
    }

    #[test]
    fn export_owner_polls_without_preferences_and_retirement_discards_late_result() {
        let (sender, receiver) = mpsc::channel();
        let retired = Arc::new(AtomicBool::new(false));
        let mut owner = ExportOwner::Running {
            receiver,
            retired: retired.clone(),
        };
        owner.poll();
        assert!(matches!(owner.view(), ExportView::Running));
        sender
            .send(Ok(SavedExport {
                path: PathBuf::from("diagnostics.zip"),
                evidence_incomplete: true,
            }))
            .unwrap();
        owner.poll();
        assert!(matches!(
            owner.view(),
            ExportView::Finished(Ok(SavedExport {
                evidence_incomplete: true,
                ..
            }))
        ));

        let (sender, receiver) = mpsc::channel();
        let mut owner = ExportOwner::Running {
            receiver,
            retired: retired.clone(),
        };
        owner.retire();
        assert!(retired.load(Ordering::Acquire));
        assert!(sender.send(Err("late".into())).is_err());
        owner.poll();
        assert!(matches!(owner.view(), ExportView::Idle));
    }

    #[test]
    fn export_owner_rejects_duplicate_and_reports_worker_disconnect() {
        let (sender, receiver) = mpsc::channel();
        let mut owner = ExportOwner::Running {
            receiver,
            retired: Arc::new(AtomicBool::new(false)),
        };
        owner.submit_worker(&egui::Context::default(), |_| {
            panic!("duplicate must not run")
        });
        assert!(matches!(owner.view(), ExportView::Running));
        drop(sender);
        owner.poll();
        assert!(matches!(owner.view(), ExportView::Finished(Err(_))));
    }
}
