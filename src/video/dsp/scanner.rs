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
    /// VST3 moduleinfo.json から Instrument 系と判定できたもの。
    /// moduleinfo が無い古いプラグインは false (= 表示) に倒す。
    pub is_instrument: bool,
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
            let is_instrument = detect_instrument_plugin(&path);
            out.push(DiscoveredPlugin {
                path: path.clone(),
                display_name: display,
                is_instrument,
            });
            // bundle の中に入ると DLL が見つかるが、それは load 時に SDK が解決する
            continue;
        }

        if file_type.is_dir() {
            scan_dir(&path, out, depth + 1);
        }
    }
}

fn detect_instrument_plugin(path: &Path) -> bool {
    if path.is_file() {
        return false;
    }
    let mut moduleinfo_files = Vec::new();
    collect_moduleinfo_files(path, &mut moduleinfo_files, 0);
    moduleinfo_files
        .into_iter()
        .any(|p| moduleinfo_declares_instrument(&p))
}

fn collect_moduleinfo_files(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 4 {
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
        if file_type.is_file() && name.eq_ignore_ascii_case("moduleinfo.json") {
            out.push(path);
        } else if file_type.is_dir() {
            collect_moduleinfo_files(&path, out, depth + 1);
        }
    }
}

fn moduleinfo_declares_instrument(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    json_declares_instrument(&json)
}

fn json_declares_instrument(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(classes) = value_for_any_key(map, &["Classes", "classes"]) {
                if let Some(arr) = classes.as_array() {
                    return arr.iter().any(class_declares_instrument);
                }
            }
            map.values().any(json_declares_instrument)
        }
        serde_json::Value::Array(arr) => arr.iter().any(json_declares_instrument),
        _ => false,
    }
}

fn class_declares_instrument(value: &serde_json::Value) -> bool {
    let Some(map) = value.as_object() else {
        return false;
    };
    let category = string_for_any_key(map, &["Category", "category"]);
    let sub_categories =
        strings_for_any_key(map, &["Sub Categories", "SubCategories", "subCategories"]);
    let is_audio_module = category
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case("Audio Module Class"))
        .unwrap_or(true);
    is_audio_module && sub_categories.iter().any(|s| contains_instrument_token(s))
}

fn value_for_any_key<'a>(
    map: &'a serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<&'a serde_json::Value> {
    keys.iter().find_map(|k| map.get(*k))
}

fn string_for_any_key(
    map: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<String> {
    value_for_any_key(map, keys)
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn strings_for_any_key(
    map: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Vec<String> {
    let Some(value) = value_for_any_key(map, keys) else {
        return Vec::new();
    };
    match value {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

fn contains_instrument_token(s: &str) -> bool {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|part| part.eq_ignore_ascii_case("instrument"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_marks_moduleinfo_instrument() {
        let temp = tempfile::tempdir().unwrap();
        let plugin_dir = temp.path().join("Synth.vst3");
        let resources = plugin_dir.join("Contents").join("Resources");
        std::fs::create_dir_all(&resources).unwrap();
        std::fs::write(
            resources.join("moduleinfo.json"),
            r#"{
                "Classes": [{
                    "Category": "Audio Module Class",
                    "Sub Categories": "Instrument|Synth"
                }]
            }"#,
        )
        .unwrap();

        let found = scan(&[temp.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert!(found[0].is_instrument);
    }

    #[test]
    fn scan_keeps_fx_visible_by_default() {
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
        assert!(!found[0].is_instrument);
    }
}
