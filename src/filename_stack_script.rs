//! ファイル名スタックのユーザー定義分割ルール (Rhai)。
//! 設計: docs/filename-stack-scripting-plan.md
//!
//! フォルダ内のメディア (画像 + 動画) を「スタック」へ畳むためのグループキーを、
//! ユーザーが編集できる Rhai スクリプトで決める。スクリプトは純関数 (I/O なし):
//! メンバー列を受け取り、各ファイルのグループキー文字列を返すだけ。
//!
//! - 既定スクリプトは `assets/stack_rules.default.rhai` を `include_str!` で内蔵。
//! - ユーザーが上書きしたい場合は `<data_dir>/stack_rules.rhai` を置く
//!   (通常版/単体exe版 = `%APPDATA%\mimageviewer\`、ポータブル版 = exe 隣の `data\`)。
//! - 自動実行されるユーザー編集スクリプトなので、操作上限つきの deny-by-default
//!   サンドボックス (`io`/`os`/`eval` を一切公開しない) で安全に実行する。
//! - 失敗 (コンパイル/実行エラー/戻り値長不一致) は呼び出し側で組み込み既定ルールへ
//!   フォールバックする (本モジュールは `Result` を返すだけ)。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use rhai::{Dynamic, Engine, Scope};

use crate::filename_stack::StackMember;

/// 内蔵の既定スクリプト (カスケード分割ルール)。
pub const DEFAULT_SCRIPT: &str = include_str!("../assets/stack_rules.default.rhai");

/// スクリプト実行の結果。
pub struct GroupingResult {
    /// `media` と同じ長さのグループキー (同じキー = 同じスタック)。
    /// 未該当 (script が `()` を返した) ファイルは NUL 区切りの一意キーになる (= 単独)。
    pub keys: Vec<String>,
    /// 採用ルールの表示名 (script が `#{ rule, keys }` を返した場合)。トースト用。
    pub rule: Option<String>,
}

/// ユーザースクリプトのパス (`<data_dir>/stack_rules.rhai`)。
pub fn script_path() -> PathBuf {
    crate::data_dir::get().join("stack_rules.rhai")
}

/// 実際に使うスクリプトソースを返す。ユーザーファイルがあればそれ、無ければ内蔵既定。
pub fn active_script_source() -> String {
    match std::fs::read_to_string(script_path()) {
        Ok(s) if !s.trim().is_empty() => s,
        _ => DEFAULT_SCRIPT.to_string(),
    }
}

/// ユーザースクリプトが無ければ既定を書き出す。返り値はそのパス (編集用に開く前段)。
pub fn ensure_user_script_exists() -> std::io::Result<PathBuf> {
    let path = script_path();
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, DEFAULT_SCRIPT)?;
    }
    Ok(path)
}

/// ユーザースクリプトを内蔵既定で上書きする (「既定に戻す」)。
pub fn reset_user_script() -> std::io::Result<PathBuf> {
    let path = script_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, DEFAULT_SCRIPT)?;
    Ok(path)
}

// ── 正規表現ヘルパー (コンパイル済みパターンをキャッシュ) ────────────────────
// regex クレートは線形時間保証 (ReDoS 不可) なのでユーザー入力パターンでも安全。
// コンパイル失敗は None をキャッシュして以後 false / "" を返す (panic しない)。

static REGEX_CACHE: LazyLock<Mutex<HashMap<String, Option<Regex>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn get_regex(pattern: &str) -> Option<Regex> {
    let mut cache = REGEX_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = cache.get(pattern) {
        return entry.clone();
    }
    let compiled = Regex::new(pattern).ok();
    // 無制限肥大を防ぐ (ファイル名からパターンを動的生成するスクリプト対策、Codex P3)。
    // 静的パターンしか使わない通常スクリプトでは到達しない上限。
    if cache.len() >= 1024 {
        cache.clear();
    }
    cache.insert(pattern.to_string(), compiled.clone());
    compiled
}

// ── path → 表示フィールド ────────────────────────────────────────────────

fn name_of(p: &Path) -> &str {
    p.file_name().and_then(|s| s.to_str()).unwrap_or("")
}

fn stem_of(p: &Path) -> &str {
    p.file_stem().and_then(|s| s.to_str()).unwrap_or("")
}

fn ext_of(p: &Path) -> String {
    p.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default()
}

/// サンドボックス済みエンジンを作る。
fn build_engine() -> Engine {
    let mut engine = Engine::new();
    // 暴走 backstop (自動実行されるユーザースクリプト用)。UI スレッドで走るので
    // op 上限はやや低め (既定スクリプトの 10k ファイルは ~数十万 op で十分収まる)。
    engine.set_max_operations(10_000_000);
    engine.set_max_call_levels(64);
    engine.set_max_expr_depths(64, 64);
    engine.set_max_string_size(64 * 1024);
    engine.set_max_array_size(8_000_000);
    engine.set_max_map_size(8_000_000);
    engine.disable_symbol("eval");
    // import 経由のモジュール読み込み (既定 FileModuleResolver による任意 .rhai の
    // ファイル読み) を封じる。これを忘れると I/O 非公開のサンドボックスに穴が空く
    // (Codex P1)。import はパース時に拒否される。
    engine.disable_symbol("import");

    engine.register_fn("regex_is_match", |text: &str, pattern: &str| -> bool {
        get_regex(pattern)
            .map(|re| re.is_match(text))
            .unwrap_or(false)
    });
    engine.register_fn(
        "regex_capture",
        |text: &str, pattern: &str, group: i64| -> String {
            if group < 0 {
                return String::new();
            }
            get_regex(pattern)
                .and_then(|re| {
                    re.captures(text)
                        .and_then(|caps| caps.get(group as usize).map(|m| m.as_str().to_string()))
                })
                .unwrap_or_default()
        },
    );
    engine.register_fn(
        "regex_replace",
        |text: &str, pattern: &str, replacement: &str| -> String {
            get_regex(pattern)
                .map(|re| re.replace_all(text, replacement).into_owned())
                .unwrap_or_else(|| text.to_string())
        },
    );
    // 整数配列を値の昇順に並べたときの添字配列 (連番順 / 時刻順の処理用)。
    engine.register_fn("argsort_int", |arr: rhai::Array| -> rhai::Array {
        let vals: Vec<i64> = arr.iter().map(|d| d.as_int().unwrap_or(0)).collect();
        let mut idx: Vec<usize> = (0..vals.len()).collect();
        idx.sort_by_key(|&i| vals[i]); // stable
        idx.into_iter().map(|i| Dynamic::from(i as i64)).collect()
    });

    engine
}

/// `media` を `source` (Rhai スクリプト) の `group(files)` でグループ分けする。
///
/// 戻り値の `keys` は `media` と同じ長さ。失敗時は `Err` (呼び出し側で組み込み
/// 既定ルールへフォールバックする)。
pub fn group_keys(media: &[StackMember], source: &str) -> Result<GroupingResult, String> {
    let engine = build_engine();
    let ast = engine
        .compile(source)
        .map_err(|e| format!("スクリプトのコンパイルに失敗: {e}"))?;

    let files: rhai::Array = media
        .iter()
        .map(|m| {
            let mut map = rhai::Map::new();
            map.insert("name".into(), Dynamic::from(name_of(&m.path).to_string()));
            map.insert("stem".into(), Dynamic::from(stem_of(&m.path).to_string()));
            map.insert("ext".into(), Dynamic::from(ext_of(&m.path)));
            map.insert("mtime".into(), Dynamic::from(m.mtime));
            map.insert("size".into(), Dynamic::from(m.size));
            map.insert("is_video".into(), Dynamic::from(m.is_video));
            Dynamic::from(map)
        })
        .collect();

    let mut scope = Scope::new();
    let result: Dynamic = engine
        .call_fn(&mut scope, &ast, "group", (files,))
        .map_err(|e| format!("スクリプトの実行に失敗: {e}"))?;

    // 戻り値は keys 配列、または #{ rule, keys } マップ。
    let (keys_dyn, rule): (Dynamic, Option<String>) = if result.is::<rhai::Map>() {
        let mut map = result.cast::<rhai::Map>();
        let rule = map.remove("rule").and_then(|d| d.into_string().ok());
        let keys = map
            .remove("keys")
            .ok_or_else(|| "スクリプトの戻り値 map に keys がありません".to_string())?;
        (keys, rule)
    } else {
        (result, None)
    };

    let arr = keys_dyn
        .try_cast::<rhai::Array>()
        .ok_or_else(|| "スクリプトの戻り値 (keys) が配列ではありません".to_string())?;
    if arr.len() != media.len() {
        return Err(format!(
            "スクリプトの戻り値の長さ {} がファイル数 {} と一致しません",
            arr.len(),
            media.len()
        ));
    }

    // キーは「文字列」または `()` (= 単独) のみ受け付ける。数値は文字列化して許容するが、
    // 配列 / マップ / bool 等が来たら誤グループ化を防ぐためエラー → 組み込み既定へフォールバック
    // させる (Codex P2)。
    let keys: Vec<String> = arr
        .into_iter()
        .enumerate()
        .map(|(i, d)| {
            if d.is_unit() {
                // 未該当 (()) は一意キー = 単独スタック。NUL 区切りで実キーと衝突しない。
                Ok(format!("\u{0}solo\u{0}{}", media[i].path.display()))
            } else if d.is_string() {
                Ok(d.into_string().unwrap_or_default())
            } else if d.is_int() {
                Ok(d.as_int().unwrap_or(0).to_string())
            } else {
                Err(format!(
                    "スクリプトのキーが文字列でも数値でもありません (index {i})"
                ))
            }
        })
        .collect::<Result<Vec<String>, String>>()?;

    Ok(GroupingResult { keys, rule })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn m(name: &str, mtime: i64, is_video: bool) -> StackMember {
        StackMember {
            path: PathBuf::from(format!(r"C:\dl\{name}")),
            mtime,
            size: 0,
            is_video,
        }
    }

    fn run(media: &[StackMember]) -> GroupingResult {
        group_keys(media, DEFAULT_SCRIPT).expect("default script runs")
    }

    #[test]
    fn default_script_compiles_and_runs_empty() {
        let r = group_keys(&[], DEFAULT_SCRIPT).expect("empty ok");
        assert!(r.keys.is_empty());
    }

    #[test]
    fn trailing_number_groups_by_prefix() {
        let media = vec![
            m("111_p0.jpg", 0, false),
            m("111_p1.jpg", 0, false),
            m("222_p0.jpg", 0, false),
            m("222_p1.jpg", 0, false),
        ];
        let r = run(&media);
        assert_eq!(r.rule.as_deref(), Some("末尾連番"));
        assert_eq!(r.keys[0], r.keys[1]);
        assert_eq!(r.keys[2], r.keys[3]);
        assert_ne!(r.keys[0], r.keys[2]);
    }

    #[test]
    fn underscore_001_groups() {
        let media = vec![
            m("scan_001.jpg", 0, false),
            m("scan_002.jpg", 0, false),
            m("other_001.jpg", 0, false),
            m("other_002.jpg", 0, false),
        ];
        let r = run(&media);
        assert_eq!(r.rule.as_deref(), Some("末尾連番"));
        assert_eq!(r.keys[0], r.keys[1]);
        assert_ne!(r.keys[0], r.keys[2]);
    }

    #[test]
    fn camera_serials_fall_through_to_burst() {
        // 全部 IMG_xxxx (末尾連番だが distinct=1 で退化 → 不採用) → 連写(2クラスタ)へ。
        let media = vec![
            m("IMG_1001.jpg", 100, false),
            m("IMG_1002.jpg", 101, false),
            m("IMG_1003.jpg", 200, false),
            m("IMG_1004.jpg", 201, false),
        ];
        let r = run(&media);
        assert_eq!(r.rule.as_deref(), Some("更新時刻"));
        assert_eq!(r.keys[0], r.keys[1]);
        assert_eq!(r.keys[2], r.keys[3]);
        assert_ne!(r.keys[0], r.keys[2]);
    }

    #[test]
    fn leading_serial_splits_on_gap() {
        let media = vec![
            m("0001_a.jpg", 0, false),
            m("0002_b.jpg", 0, false),
            m("0050_c.jpg", 0, false),
            m("0051_d.jpg", 0, false),
        ];
        let r = run(&media);
        assert_eq!(r.rule.as_deref(), Some("先頭連番"));
        assert_eq!(r.keys[0], r.keys[1]);
        assert_eq!(r.keys[2], r.keys[3]);
        assert_ne!(r.keys[0], r.keys[2]);
    }

    #[test]
    fn mxd_suffix_fixed_groups_single_tweet() {
        let media = vec![
            m(
                "20260429_1100_0003_1234567890_p01_m01_@artist.jpg",
                0,
                false,
            ),
            m(
                "20260429_1100_0003_1234567890_p01_m02_@artist.jpg",
                0,
                false,
            ),
            m(
                "20260429_1100_0003_1234567890_p01_m03_@artist.jpg",
                0,
                false,
            ),
        ];
        let r = run(&media);
        assert_eq!(r.rule.as_deref(), Some("命名パターン"));
        assert_eq!(r.keys[0], r.keys[1]);
        assert_eq!(r.keys[1], r.keys[2]);
    }

    #[test]
    fn bulk_copy_same_mtime_stays_singletons() {
        let media = vec![
            m("alpha.jpg", 500, false),
            m("beta.jpg", 500, false),
            m("gamma.jpg", 500, false),
        ];
        let r = run(&media);
        assert_eq!(r.rule, None);
        assert_ne!(r.keys[0], r.keys[1]);
        assert_ne!(r.keys[1], r.keys[2]);
    }

    #[test]
    fn regex_helper_capture_works() {
        let src = r#"fn group(files) { files.map(|f| regex_capture(f.stem, "^(.+)_\\d+$", 1)) }"#;
        let media = vec![m("a_1.jpg", 0, false), m("a_2.jpg", 0, false)];
        let r = group_keys(&media, src).expect("runs");
        assert_eq!(r.keys[0], "a");
        assert_eq!(r.keys[1], "a");
    }

    #[test]
    fn length_mismatch_is_error() {
        let src = r#"fn group(files) { [] }"#;
        let media = vec![m("a.jpg", 0, false)];
        assert!(group_keys(&media, src).is_err());
    }

    #[test]
    fn import_is_blocked() {
        // import を無効化しているので、import を含むスクリプトはコンパイルできず Err。
        let src = r#"import "foo" as bar; fn group(files) { files.map(|f| f.name) }"#;
        let media = vec![m("a.jpg", 0, false)];
        assert!(group_keys(&media, src).is_err());
    }

    #[test]
    fn eval_is_blocked() {
        let src = r#"fn group(files) { eval("1"); files.map(|f| f.name) }"#;
        let media = vec![m("a.jpg", 0, false)];
        assert!(group_keys(&media, src).is_err());
    }

    #[test]
    fn non_string_non_int_key_is_rejected() {
        // 配列をキーに返すと誤グループ化を防ぐため Err (→ 呼び出し側で既定へフォールバック)。
        let src = r#"fn group(files) { files.map(|f| [1, 2]) }"#;
        let media = vec![m("a.jpg", 0, false), m("b.jpg", 0, false)];
        assert!(group_keys(&media, src).is_err());
    }

    #[test]
    fn int_key_is_accepted_as_string() {
        let src = r#"fn group(files) { files.map(|f| 7) }"#;
        let media = vec![m("a.jpg", 0, false), m("b.jpg", 0, false)];
        let r = group_keys(&media, src).expect("int keys ok");
        assert_eq!(r.keys[0], "7");
        assert_eq!(r.keys[1], "7");
    }
}
