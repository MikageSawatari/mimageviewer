//! 診断情報の zip 書き出し。
//!
//! 環境設定 → 開発者タブの「ログを zip にする」ボタンから呼ばれる。logs
//! ディレクトリのログ群 + システム情報を 1 つの zip にまとめてデスクトップへ
//! 保存し、ユーザーがサポートへ添付しやすくするのが目的。コマンドライン引数や
//! `%APPDATA%` の手探りを利用者にさせないための入口。

use std::io::Write;
use std::path::PathBuf;

/// 診断 zip をデスクトップに書き出す。成功時は作成した zip のフルパスを返す。
///
/// best-effort: 失敗してもアプリ状態は変えず、エラー文字列を返すだけ。
pub fn export_diagnostics_zip() -> Result<PathBuf, String> {
    let logs_dir = crate::data_dir::logs_dir();
    // デスクトップが取得できない環境では logs ディレクトリ自体に出す
    // (どこにも書けないよりはマシ)。
    let out_dir = desktop_dir().unwrap_or_else(|| logs_dir.clone());
    let zip_path = out_dir.join(format!("mImageViewer_diag_{}.zip", local_timestamp()));

    let included = export_diagnostics_zip_from(&logs_dir, &zip_path, || {
        // This event is the end witness for the perf log inside the archive. It must be queued
        // before either logger is flushed, and both flushes must finish before any log file is
        // opened for ZIP collection.
        crate::perf::event(
            "diagnostics",
            "export_requested",
            None,
            0,
            &[("pid", serde_json::Value::from(std::process::id()))],
        );
        crate::perf::flush();
        crate::logger::flush();
    })?;

    crate::logger::log(format!(
        "diagnostics: exported {included} files -> {}",
        zip_path.display()
    ));
    Ok(zip_path)
}

/// Build a diagnostics archive after `prepare_logs` has made every buffered log write visible.
/// Keeping this boundary injectable lets the regression test exercise a real sub-64KiB
/// `BufWriter` without mutating the process-global logger singletons.
fn export_diagnostics_zip_from(
    logs_dir: &std::path::Path,
    zip_path: &std::path::Path,
    prepare_logs: impl FnOnce(),
) -> Result<usize, String> {
    prepare_logs();

    let file = std::fs::File::create(&zip_path)
        .map_err(|e| format!("zip ファイルを作成できません ({}): {e}", zip_path.display()))?;
    let mut zw = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let opts: zip::write::FileOptions<'static, ()> = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);

    // 先頭に環境情報を入れておく (バージョン・OS・GPU の特定に使う)。
    zw.start_file("system_info.txt", opts)
        .map_err(|e| format!("zip 書き込み失敗: {e}"))?;
    zw.write_all(system_info_text().as_bytes())
        .map_err(|e| format!("zip 書き込み失敗: {e}"))?;

    // logs ディレクトリの中身を入れる。rotate 済みの perf 世代
    // (perf_events.1.jsonl 〜) は巨大なので除外し、現行 perf_events.jsonl だけ含める。
    let mut included = 0usize;
    if let Ok(entries) = std::fs::read_dir(&logs_dir) {
        for entry in entries.flatten() {
            // デスクトップが取得できず out_dir == logs_dir の fallback 経路では、
            // たった今作った zip 自身が列挙に出てくる。自分を中に取り込む / 書き込み中の
            // 中途半端なバイト列を読むのを避けるため除外する。
            if entry.path() == zip_path {
                continue;
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if !ft.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_rotated_perf_generation(&name) {
                continue;
            }
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            if zw.start_file(format!("logs/{name}"), opts).is_err() {
                continue;
            }
            if zw.write_all(&bytes).is_ok() {
                included += 1;
            }
        }
    }

    zw.finish()
        .map_err(|e| format!("zip のクローズに失敗: {e}"))?;
    Ok(included)
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
    use super::{export_diagnostics_zip_from, is_rotated_perf_generation};
    use std::io::{Read as _, Write as _};

    #[test]
    fn rotated_perf_generations_are_detected() {
        assert!(is_rotated_perf_generation("perf_events.1.jsonl"));
        assert!(is_rotated_perf_generation("perf_events.42.jsonl"));
    }

    #[test]
    fn current_perf_log_and_others_are_not_rotated_generations() {
        // 現行 perf ログは含める対象なので false。
        assert!(!is_rotated_perf_generation("perf_events.jsonl"));
        assert!(!is_rotated_perf_generation("mimageviewer.log"));
        assert!(!is_rotated_perf_generation("panic.log"));
        assert!(!is_rotated_perf_generation("perf_events.abc.jsonl"));
        assert!(!is_rotated_perf_generation("perf_events..jsonl"));
    }

    #[test]
    fn export_flushes_sub_64k_perf_tail_and_includes_end_witness() {
        let tmp = tempfile::tempdir().unwrap();
        let logs_dir = tmp.path().join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        let perf_path = logs_dir.join("perf_events.jsonl");
        let perf_file = std::fs::File::create(&perf_path).unwrap();
        let mut perf_writer = std::io::BufWriter::with_capacity(64 * 1024, perf_file);
        let buffered_line = r#"{"cat":"grid","kind":"buffered_before_export","seq":41}"#;
        writeln!(perf_writer, "{buffered_line}").unwrap();
        assert_eq!(
            std::fs::metadata(&perf_path).unwrap().len(),
            0,
            "the fixture must still be below the BufWriter flush threshold"
        );

        let zip_path = tmp.path().join("diagnostics.zip");
        let included = export_diagnostics_zip_from(&logs_dir, &zip_path, || {
            writeln!(
                perf_writer,
                r#"{{"cat":"diagnostics","kind":"export_requested","seq":42}}"#
            )
            .unwrap();
            perf_writer.flush().unwrap();
        })
        .unwrap();
        assert_eq!(included, 1);

        let zip_file = std::fs::File::open(zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(zip_file).unwrap();
        let mut perf_log = String::new();
        archive
            .by_name("logs/perf_events.jsonl")
            .unwrap()
            .read_to_string(&mut perf_log)
            .unwrap();
        assert!(perf_log.contains("buffered_before_export"));
        assert!(perf_log.contains(r#""kind":"export_requested""#));
    }
}
