//! VST3 プラグインスキャナ。
//!
//! Windows の標準的な VST3 配置パスを再帰的に列挙する:
//! - `%COMMONPROGRAMFILES%\VST3\` (= C:\Program Files\Common Files\VST3\)
//! - `%LOCALAPPDATA%\Programs\Common\VST3\` (ユーザー単位インストール)
//!
//! VST3 は **bundle 形式 (= ディレクトリ)** が標準で、`.vst3` という拡張子の
//! ディレクトリの中の `Contents/x86_64-win/<plugin>.vst3` が実体 (DLL)。
//! ただし古いプラグインは単一 DLL のこともあるので、両対応する。

use std::path::{Path, PathBuf};

use rayon::prelude::*;

#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
    /// .vst3 ディレクトリ or DLL の絶対パス。VST3 SDK はこのパスを直接受け入れる。
    pub path: PathBuf,
    /// UI 表示用: ファイル名 (拡張子なし)
    pub display_name: String,
    /// bridge probe で取得した audio input bus 数。probe 失敗 / 未実行なら None。
    pub audio_input_buses: Option<u32>,
    /// bridge probe で取得した audio output bus 数。probe 失敗 / 未実行なら None。
    pub audio_output_buses: Option<u32>,
    pub event_input_buses: Option<u32>,
    pub event_output_buses: Option<u32>,
    pub audio_input_channels: Option<u32>,
    pub audio_output_channels: Option<u32>,
    /// bridge probe の最終判定。true なら mIV の audio input -> output 処理に使える。
    pub usable_audio_effect: Option<bool>,
    /// probe 失敗時の診断文字列。失敗時は一覧に出すが追加不可にする。
    pub probe_error: Option<String>,
}

impl DiscoveredPlugin {
    pub fn hidden_by_default(&self) -> bool {
        self.usable_audio_effect == Some(false)
    }

    pub fn has_probe_error(&self) -> bool {
        self.probe_error.is_some()
    }

    pub fn hidden_reason(&self) -> Option<&'static str> {
        if self.hidden_by_default() {
            Some("音声入力なし")
        } else {
            None
        }
    }
}

/// 既定の VST3 検索ルートを返す。
pub fn default_vst3_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(common) = std::env::var("CommonProgramFiles") {
        paths.push(PathBuf::from(common).join("VST3"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        paths.push(
            PathBuf::from(local)
                .join("Programs")
                .join("Common")
                .join("VST3"),
        );
    }
    paths
}

/// 指定したルート群以下から `.vst3` プラグインを再帰列挙する。
pub fn scan(roots: &[PathBuf]) -> Vec<DiscoveredPlugin> {
    let mut out = Vec::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
        scan_dir(root, &mut out, 0);
    }
    out.sort_by(|a, b| {
        a.display_name
            .to_ascii_lowercase()
            .cmp(&b.display_name.to_ascii_lowercase())
    });
    out.dedup_by(|a, b| a.path == b.path);
    out
}

fn scan_dir(dir: &Path, out: &mut Vec<DiscoveredPlugin>, depth: usize) {
    // 過剰な再帰を防ぐ (VST3 ベンダーのサブディレクトリは数階層で十分)
    if depth > 6 {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        // .vst3 拡張子は bundle (= ディレクトリ) でも単一 DLL でも有効
        if name.to_ascii_lowercase().ends_with(".vst3") {
            let display = name
                .trim_end_matches(".vst3")
                .trim_end_matches(".VST3")
                .to_string();
            out.push(DiscoveredPlugin {
                path: path.clone(),
                display_name: display,
                audio_input_buses: None,
                audio_output_buses: None,
                event_input_buses: None,
                event_output_buses: None,
                audio_input_channels: None,
                audio_output_channels: None,
                usable_audio_effect: None,
                probe_error: None,
            });
            // bundle の中に入ると DLL が見つかるが、それは load 時に SDK が解決する
            continue;
        }

        if file_type.is_dir() {
            scan_dir(&path, out, depth + 1);
        }
    }
}

#[cfg(windows)]
#[derive(Debug)]
struct ProbeResult {
    plugin_name: String,
    audio_input_buses: u32,
    audio_output_buses: u32,
    event_input_buses: u32,
    event_output_buses: u32,
    audio_input_channels: u32,
    audio_output_channels: u32,
    usable_audio_effect: bool,
}

/// `.vst3` を列挙したうえで、bridge subprocess で各 plugin の audio/event bus を probe する。
/// UI thread から直接呼ばず、環境設定の scan worker から呼ぶ想定。
#[cfg(windows)]
pub fn scan_with_audio_probe(roots: &[PathBuf]) -> Result<Vec<DiscoveredPlugin>, String> {
    scan_with_audio_probe_progress(roots, |_, _, _| {})
}

#[cfg(windows)]
pub fn scan_with_audio_probe_progress<F>(
    roots: &[PathBuf],
    progress: F,
) -> Result<Vec<DiscoveredPlugin>, String>
where
    F: Fn(usize, usize, &Path) + Sync,
{
    let mut plugins = scan(roots);
    let total = plugins.len();
    progress(0, total, Path::new(""));
    if plugins.is_empty() {
        return Ok(plugins);
    }
    let exe = super::extract::ensure_bridge_extracted()?.clone();
    let threads = std::thread::available_parallelism()
        .map(|n| n.get().clamp(1, 4))
        .unwrap_or(2);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|idx| format!("vst3-probe-{idx}"))
        .build()
        .map_err(|e| format!("vst3 probe thread pool: {e}"))?;

    let completed = std::sync::atomic::AtomicUsize::new(0);
    pool.install(|| {
        plugins.par_iter_mut().for_each(|plugin| {
            match probe_plugin_with_bridge(&exe, &plugin.path) {
                Ok(probe) => {
                    if !probe.plugin_name.is_empty() {
                        plugin.display_name = probe.plugin_name;
                    }
                    plugin.audio_input_buses = Some(probe.audio_input_buses);
                    plugin.audio_output_buses = Some(probe.audio_output_buses);
                    plugin.event_input_buses = Some(probe.event_input_buses);
                    plugin.event_output_buses = Some(probe.event_output_buses);
                    plugin.audio_input_channels = Some(probe.audio_input_channels);
                    plugin.audio_output_channels = Some(probe.audio_output_channels);
                    plugin.usable_audio_effect = Some(probe.usable_audio_effect);
                    plugin.probe_error = None;
                }
                Err(err) => {
                    plugin.probe_error = Some(err);
                }
            }
            let done = completed.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1;
            progress(done, total, &plugin.path);
        });
    });

    plugins.sort_by(|a, b| {
        a.display_name
            .to_ascii_lowercase()
            .cmp(&b.display_name.to_ascii_lowercase())
    });
    Ok(plugins)
}

#[cfg(windows)]
fn probe_plugin_with_bridge(exe: &Path, plugin_path: &Path) -> Result<ProbeResult, String> {
    use super::bridge::{Bridge, Cmd, Event, PROTOCOL_VERSION};

    let bridge = Bridge::spawn(exe, |line| {
        crate::logger::log(format!("[vst3-probe] {line}"));
    })
    .map_err(|e| format!("bridge spawn: {e}"))?;
    bridge
        .send(&Cmd::Hello {
            version: PROTOCOL_VERSION,
        })
        .map_err(|e| format!("hello send: {e}"))?;
    match bridge.recv_timeout(std::time::Duration::from_secs(3)) {
        Ok(Event::Ready { version }) => {
            // T09 (v0.9.0): protocol mismatch を握りつぶさない
            if version != PROTOCOL_VERSION {
                return Err(format!(
                    "VST3 bridge protocol version mismatch (bridge reported {version}, mIV expects {PROTOCOL_VERSION})"
                ));
            }
        }
        Ok(Event::Error { detail }) => {
            return Err(format!("VST3 bridge handshake error: {detail}"));
        }
        Ok(other) => return Err(format!("unexpected ready event: {other:?}")),
        Err(e) => return Err(format!("ready recv: {e}")),
    }

    bridge
        .send(&Cmd::Probe {
            plugin_path: plugin_path.to_string_lossy().replace('\\', "/"),
        })
        .map_err(|e| format!("probe send: {e}"))?;
    let result = match bridge.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(Event::Probed {
            plugin_name,
            audio_input_buses,
            audio_output_buses,
            event_input_buses,
            event_output_buses,
            audio_input_channels,
            audio_output_channels,
            usable_audio_effect,
        }) => Ok(ProbeResult {
            plugin_name,
            audio_input_buses,
            audio_output_buses,
            event_input_buses,
            event_output_buses,
            audio_input_channels,
            audio_output_channels,
            usable_audio_effect,
        }),
        Ok(Event::Error { detail }) => Err(detail),
        Ok(other) => Err(format!("unexpected probe event: {other:?}")),
        Err(e) => Err(format!("probe recv: {e}")),
    };
    // Probe can time out while the child is stuck inside plugin code. In that
    // case graceful shutdown would block on child.wait(), so let Drop terminate
    // the isolated bridge process instead.
    drop(bridge);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_lists_vst3_without_guessing_capability() {
        let temp = tempfile::tempdir().unwrap();
        let plugin_dir = temp.path().join("Eq.vst3");
        let resources = plugin_dir.join("Contents");
        std::fs::create_dir_all(&resources).unwrap();
        std::fs::write(
            resources.join("moduleinfo.json"),
            r#"{
                "Classes": [{
                    "Category": "Audio Module Class",
                    "Sub Categories": ["Fx", "EQ"]
                }]
            }"#,
        )
        .unwrap();

        let found = scan(&[temp.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].display_name, "Eq");
        assert_eq!(found[0].usable_audio_effect, None);
        assert!(!found[0].hidden_by_default());
    }
}
