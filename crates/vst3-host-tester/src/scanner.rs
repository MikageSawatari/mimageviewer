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

#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
    /// .vst3 ディレクトリ or DLL の絶対パス。VST3 SDK はこのパスを直接受け入れる。
    pub path: PathBuf,
    /// UI 表示用: ファイル名 (拡張子なし)
    pub display_name: String,
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
            });
            // bundle の中に入ると DLL が見つかるが、それは load 時に SDK が解決する
            continue;
        }

        if file_type.is_dir() {
            scan_dir(&path, out, depth + 1);
        }
    }
}
