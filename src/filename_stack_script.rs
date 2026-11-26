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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use regex::Regex;
use rhai::{
    AST, Dynamic, Engine, EvalAltResult, FuncRegistration, ImmutableString, NativeCallContext,
    Position, Scope,
};

use crate::filename_stack::StackMember;

mod writer;
pub(crate) use writer::rename_script_temporary;
pub use writer::{
    ScriptDocument, ScriptWriteOutcome, enqueue_open_user_script, enqueue_reset_user_script,
    read_script_document, write_script,
};

/// 内蔵の既定スクリプト (カスケード分割ルール)。
pub const DEFAULT_SCRIPT: &str = include_str!("../assets/stack_rules.default.rhai");

/// ユーザースクリプトとして読み込む上限バイト数。通常の分類スクリプトは数 KB なので、
/// これを超えるファイルは「壊れている / 別物が置かれた」とみなし内蔵既定へフォールバックする
/// (= 巨大ファイルや低速メディアで `read_to_string` がワーカー起動前に長時間ブロックするのを防ぐ)。
pub(crate) const MAX_SCRIPT_BYTES: u64 = 1024 * 1024; // 1 MiB

/// スクリプト実行の結果。
pub struct GroupingResult {
    /// `media` と同じ長さのグループキー (同じキー = 同じスタック)。
    /// 未該当 (script が `()` を返した) ファイルは NUL 区切りの一意キーになる (= 単独)。
    pub keys: Vec<String>,
    /// 採用ルールの表示名 (script が `#{ rule, keys }` を返した場合)。トースト用。
    pub rule: Option<String>,
}

/// コンパイル済みスタックスクリプト。
///
/// サブ展開ビューでは親フォルダごとに同じ `group(files)` を呼ぶため、フォルダごとに
/// コンパイルし直さず AST を使い回す。
pub struct CompiledStackScript {
    engine: Engine,
    ast: AST,
}

impl CompiledStackScript {
    pub fn compile(source: &str, cancel: Arc<AtomicBool>) -> Result<Self, String> {
        let mut engine = build_engine();
        // 数万 op ごとにキャンセルを確認する (per-op で atomic load すると interpreter が重く
        // なるので下位ビットでマスクして間引く)。`Some(..)` を返すと Rhai が ErrorTerminated で
        // 即座に評価を打ち切る。
        {
            let cancel = Arc::clone(&cancel);
            engine.on_progress(move |ops| {
                if ops & 0xFFFF == 0 && cancel.load(Ordering::Relaxed) {
                    Some(Dynamic::UNIT) // 中断
                } else {
                    None
                }
            });
        }
        let ast = engine
            .compile(source)
            .map_err(|e| format!("スクリプトのコンパイルに失敗: {e}"))?;
        Ok(Self { engine, ast })
    }

    pub fn group_keys(&self, media: &[StackMember]) -> Result<GroupingResult, String> {
        run_group_function(&self.engine, &self.ast, media)
    }
}

/// ユーザースクリプトのパス (`<data_dir>/stack_rules.rhai`)。
pub fn script_path() -> PathBuf {
    crate::data_dir::get().join("stack_rules.rhai")
}

/// 実際に使うスクリプトソースを返す。ユーザーファイルがあればそれ、無ければ内蔵既定。
///
/// **ワーカースレッドから呼ぶこと** (= ファイル I/O を UI スレッドに乗せない)。`MAX_SCRIPT_BYTES`
/// を超えるファイルは読まずに既定へフォールバックし、巨大 / 低速メディア上のファイルで
/// `read_to_string` が長時間ブロックするのを防ぐ。
pub fn active_script_source() -> String {
    let path = script_path();
    // 先に長さを見て上限超過なら読まない (TOCTOU は自分の設定ファイルなので実害なし)。
    if let Ok(meta) = std::fs::metadata(&path)
        && meta.len() > MAX_SCRIPT_BYTES
    {
        crate::logger::log(format!(
            "stack script too large ({} bytes > {} cap), using builtin default: {}",
            meta.len(),
            MAX_SCRIPT_BYTES,
            path.display()
        ));
        return DEFAULT_SCRIPT.to_string();
    }
    match std::fs::read_to_string(&path) {
        Ok(s) if !s.trim().is_empty() => s,
        _ => DEFAULT_SCRIPT.to_string(),
    }
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

const SLEEP_ERROR: &str = "スタックのスクリプトでは sleep は使えません (処理が終わらなくなるため)";
const EMPTY_PAD_ERROR: &str =
    "スタックのスクリプトでは、空の文字列を詰める pad は使えません (処理が終わらなくなるため)";
const SCRIPT_OUTPUT_LIMIT: usize = 20;
const SCRIPT_OUTPUT_OMITTED: &str = "stack script print/debug: 21 行目以降は省略しました";

/// Rhai 1.27 の文字列版 pad。進まない空文字のループだけを拒否し、サイズ検査も維持する。
fn stack_pad_string(
    ctx: NativeCallContext,
    string: &mut ImmutableString,
    len: rhai::INT,
    padding: &str,
) -> Result<(), Box<EvalAltResult>> {
    if len <= 0 {
        return Ok(());
    }
    let too_large = || {
        Box::new(EvalAltResult::ErrorDataTooLarge(
            "Length of string".to_string(),
            Position::NONE,
        ))
    };
    let len = usize::try_from(len).map_err(|_| too_large())?;
    let max_size = ctx.engine().max_string_size();
    if max_size > 0 && len > max_size {
        return Err(too_large());
    }
    let mut str_len = string.chars().count();
    if len <= str_len {
        return Ok(());
    }
    if padding.is_empty() {
        return Err(EvalAltResult::ErrorRuntime(EMPTY_PAD_ERROR.into(), Position::NONE).into());
    }
    let padding_len = padding.chars().count();
    let text = string.make_mut();
    while str_len < len {
        if str_len + padding_len <= len {
            text.push_str(padding);
            str_len += padding_len;
        } else {
            text.extend(padding.chars().take(len - str_len));
            str_len = len;
        }
    }
    if max_size > 0 && string.len() > max_size {
        return Err(too_large());
    }
    Ok(())
}

fn script_data_too_large(kind: &str) -> Box<EvalAltResult> {
    EvalAltResult::ErrorDataTooLarge(kind.to_string(), Position::NONE).into()
}

/// Rhai 1.27 の data_check.rs と同じ総量。blob は入れ子のときだけ 1 を加える。
fn stack_data_sizes(value: &Dynamic, top: bool) -> (usize, usize, usize) {
    if value.is_shared() {
        // Rhai と同じく最上位の Shared だけを開く。内部要素の Shared は言語側の不変条件違反。
        assert!(top, "入れ子のデータに Shared は含まれない");
        return stack_data_sizes(&value.read_lock::<Dynamic>().unwrap(), true);
    }
    if value.is_array() {
        return stack_array_sizes(&value.as_array_ref().unwrap());
    }
    if value.is_blob() {
        return (
            value
                .as_blob_ref()
                .unwrap()
                .len()
                .saturating_add(usize::from(!top)),
            0,
            0,
        );
    }
    if value.is_map() {
        let map = value.as_map_ref().unwrap();
        return map
            .values()
            .fold((0usize, map.len(), 0usize), |(a, m, s), value| {
                let (ax, mx, sx) = stack_data_sizes(value, false);
                (
                    a.saturating_add(ax),
                    m.saturating_add(mx),
                    s.saturating_add(sx),
                )
            });
    }
    if value.is_string() {
        return (0, 0, value.as_immutable_string_ref().unwrap().len());
    }
    (0, 0, 0)
}

fn stack_array_sizes(array: &rhai::Array) -> (usize, usize, usize) {
    array
        .iter()
        .fold((array.len(), 0usize, 0usize), |(a, m, s), value| {
            let (ax, mx, sx) = stack_data_sizes(value, false);
            (
                a.saturating_add(ax),
                m.saturating_add(mx),
                s.saturating_add(sx),
            )
        })
}

/// 長さを先に検査し、標準 pad の総量計算があふれて resize に進むのを防ぐ。
fn stack_pad_array(
    ctx: NativeCallContext,
    array: &mut rhai::Array,
    len: rhai::INT,
    item: Dynamic,
) -> Result<(), Box<EvalAltResult>> {
    let Ok(len) = usize::try_from(len) else {
        return Ok(());
    };
    if len <= array.len() {
        return Ok(());
    }
    let engine = ctx.engine();
    let max_array = engine.max_array_size();
    if max_array > 0 {
        if len > max_array {
            return Err(script_data_too_large("Size of array/BLOB"));
        }
        let pad = len - array.len();
        let (a, m, s) = stack_array_sizes(array);
        let (ax, mx, sx) = stack_data_sizes(&item, true);
        // 通常の上限内ではあふれないが、ホスト側で上限を変えても折り返さない。
        let a = a.saturating_add(pad).saturating_add(ax.saturating_mul(pad));
        let m = m.saturating_add(mx.saturating_mul(pad));
        let s = s.saturating_add(sx.saturating_mul(pad));
        // 複数の上限を超えたときのエラーの優先順も Rhai に合わせる。
        if engine.max_string_size() > 0 && s > engine.max_string_size() {
            return Err(script_data_too_large("Length of string"));
        }
        if a > max_array {
            return Err(script_data_too_large("Size of array/BLOB"));
        }
        if engine.max_map_size() > 0 && m > engine.max_map_size() {
            return Err(script_data_too_large("Size of object map"));
        }
    }
    array.resize(len, item);
    Ok(())
}

/// 出力先の配線はここに集約する。親フォルダごとの呼び出しでも同じ計算の枠を共有する。
fn install_script_output(engine: &mut Engine, sink: impl Fn(String) + Send + Sync + 'static) {
    let count = Arc::new(AtomicUsize::new(0));
    let sink = Arc::new(sink);
    let print_count = Arc::clone(&count);
    let print_sink = Arc::clone(&sink);
    engine.on_print(move |text| {
        emit_script_output(&print_count, &*print_sink, "stack script print: ", text);
    });
    engine.on_debug(move |text, source, position| {
        emit_script_output(
            &count,
            &*sink,
            &format!(
                "stack script debug: {} @ {position} > ",
                source.unwrap_or("<none>")
            ),
            text,
        );
    });
}

fn emit_script_output(count: &AtomicUsize, sink: &impl Fn(String), prefix: &str, text: &str) {
    // 複数行の print でも実際のログ行数で数える。空の print も 1 行分を使う。
    for line in text.split('\n') {
        match count.fetch_add(1, Ordering::Relaxed) {
            n if n < SCRIPT_OUTPUT_LIMIT => {
                sink(format!("{prefix}{}", line.trim_end_matches('\r')))
            }
            SCRIPT_OUTPUT_LIMIT => sink(SCRIPT_OUTPUT_OMITTED.to_string()),
            _ => break,
        }
    }
}

/// サンドボックス済みエンジンを作る。
fn build_engine() -> Engine {
    let mut engine = Engine::new();
    // global namespace は標準パッケージより優先される。同じ引数型を登録し、
    // 関数ポインタ経由も含めて native 内で止まる呼び出しを実行時エラーにする。
    FuncRegistration::new("sleep")
        .with_purity(true)
        .with_volatility(true)
        .register_into_engine(
            &mut engine,
            |_: rhai::INT| -> Result<(), Box<EvalAltResult>> {
                Err(EvalAltResult::ErrorRuntime(SLEEP_ERROR.into(), Position::NONE).into())
            },
        );
    FuncRegistration::new("sleep")
        .with_purity(true)
        .with_volatility(true)
        .register_into_engine(
            &mut engine,
            |_: rhai::FLOAT| -> Result<(), Box<EvalAltResult>> {
                Err(EvalAltResult::ErrorRuntime(SLEEP_ERROR.into(), Position::NONE).into())
            },
        );
    // register_fn の既定の pure では定数への変更が許されるため、標準と同じ非 pure にする。
    FuncRegistration::new("pad")
        .with_purity(false)
        .with_volatility(false)
        .register_into_engine(&mut engine, stack_pad_string);
    FuncRegistration::new("pad")
        .with_purity(false)
        .with_volatility(false)
        .register_into_engine(&mut engine, stack_pad_array);
    install_script_output(&mut engine, |line| crate::logger::log(line));
    // 暴走 backstop (自動実行されるユーザースクリプト用)。実測 (perf_10k) では native
    // ヘルパー化後の 100k camera で ~数百万 op なので、100M はその ~30 倍の余裕。無限ループ
    // 等は確実に止めつつ、10 万件の正当な処理は通す。スクリプトはワーカーで走るので UI は
    // ブロックされない (本数値は OOM / 暴走の最終防壁)。
    engine.set_max_operations(100_000_000);
    engine.set_max_call_levels(64);
    engine.set_max_expr_depths(64, 64);
    // 注意: Rhai の max_string_size は配列/マップ内の文字列バイト**総量**を再帰的に見る。
    // 10k〜100k ファイルだと files 配列 (名前×N) + keys 配列で数〜数十 MB になるため、個々の
    // 文字列ではなく総量を許容できる大きさにする (= 暴走した単一巨大文字列の OOM 防止のみを
    // 担う上限。ループ回数は max_operations で別途縛る)。
    engine.set_max_string_size(256 * 1024 * 1024);
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
    // 全要素が () でない (= 全ファイルがそのルールに該当する) か。Rust 側 O(n)。
    // ※ Rhai の for ループ + map で書くと要素ごとに interpreter を回り遅いので native 化。
    engine.register_fn("stack_all_matched", |keys: rhai::Array| -> bool {
        keys.iter().all(|d| !d.is_unit())
    });
    // 異なるキーの数 (() は無視)。Rust の HashSet で O(n)。
    // ※ Rhai のマップで実装すると、Rhai が挿入のたびにデータサイズを再計算するため
    //   O(n^2) になり 10 万件で 16 秒かかる退行になる。必ず native 側で数える。
    engine.register_fn("stack_distinct", |keys: rhai::Array| -> i64 {
        let mut set: std::collections::HashSet<String> = std::collections::HashSet::new();
        for d in &keys {
            if d.is_unit() {
                continue;
            }
            set.insert(d.to_string());
        }
        set.len() as i64
    });

    engine
}

/// `media` を `source` (Rhai スクリプト) の `group(files)` でグループ分けする。
///
/// 戻り値の `keys` は `media` と同じ長さ。失敗時は `Err` (呼び出し側で組み込み
/// 既定ルールへフォールバックする)。キャンセル不要な呼び出し (テスト等) 向けの薄いラッパ。
pub fn group_keys(media: &[StackMember], source: &str) -> Result<GroupingResult, String> {
    group_keys_cancellable(media, source, Arc::new(AtomicBool::new(false)))
}

/// `group_keys` のキャンセル可能版。ワーカースレッドから `cancel` (= 新ロード時に立つ
/// キャンセルトークン) を渡すと、Rhai インタプリタの `on_progress` フックが定期的に確認して
/// **計算途中でも中断**する (= フォルダ/トグル連打で旧ワーカーが 10 万件の処理を最後まで
/// 走らせ続けてスレッドが累積・CPU を食うのを防ぐ)。中断時は `Err` を返すが、呼び出し側は
/// cancel が立っているので結果を捨てる。
pub fn group_keys_cancellable(
    media: &[StackMember],
    source: &str,
    cancel: Arc<AtomicBool>,
) -> Result<GroupingResult, String> {
    let script = CompiledStackScript::compile(source, cancel)?;
    script.group_keys(media)
}

fn run_group_function(
    engine: &Engine,
    ast: &AST,
    media: &[StackMember],
) -> Result<GroupingResult, String> {
    // 呼び出し側 (spawn_stack_script_worker) が **画像のみ** を渡す。フォルダ/ZIP/PDF は
    // passthrough、動画は常に単独なので、スクリプトには画像しか来ない (= is_video は公開しない)。
    let files: rhai::Array = media
        .iter()
        .map(|m| {
            let mut map = rhai::Map::new();
            map.insert("name".into(), Dynamic::from(name_of(&m.path).to_string()));
            map.insert("stem".into(), Dynamic::from(stem_of(&m.path).to_string()));
            map.insert("ext".into(), Dynamic::from(ext_of(&m.path)));
            map.insert("mtime".into(), Dynamic::from(m.mtime));
            // 旧 script API は整数値。metadata 不明は従来 placeholder と同じ 0 を渡すが、
            // 一覧ソート側の Unknown/real-zero 判定にはこの値を逆輸入しない。
            map.insert("size".into(), Dynamic::from(m.size.unwrap_or(0)));
            Dynamic::from(map)
        })
        .collect();

    let mut scope = Scope::new();
    let result: Dynamic = engine
        .call_fn(&mut scope, ast, "group", (files,))
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
    use std::sync::mpsc;
    use std::time::Duration;

    fn within_timeout<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        rx.recv_timeout(Duration::from_secs(10))
            .expect("スタックスクリプトが 10 秒以内に戻ること")
    }

    fn assert_script_runtime_error(body: &str, expected: &str) {
        let source = format!("fn group(files) {{ {body}; [] }}");
        let error = within_timeout(move || group_keys(&[], &source).err())
            .expect("危ない標準関数は Err になること");
        assert!(error.contains("スクリプトの実行に失敗:"), "{error}");
        assert!(error.contains(expected), "{error}");
    }

    // 修正前と同じエンジンとキャンセルの間引き。native に入ったあとでは
    // 小さい操作上限もキャンセルも効かないことを、破壊的な確認でのみ使用する。
    fn legacy_engine(cancel: Arc<AtomicBool>) -> Engine {
        let mut engine = Engine::new();
        engine.set_max_operations(100_000_000);
        engine.set_max_call_levels(64);
        engine.set_max_expr_depths(64, 64);
        engine.set_max_string_size(256 * 1024 * 1024);
        engine.set_max_array_size(8_000_000);
        engine.set_max_map_size(8_000_000);
        engine.disable_symbol("eval");
        engine.disable_symbol("import");
        engine.on_progress(move |ops| {
            if ops & 0xFFFF == 0 && cancel.load(Ordering::Relaxed) {
                Some(Dynamic::UNIT)
            } else {
                None
            }
        });
        engine
    }

    fn assert_legacy_native_does_not_return(source: &'static str) {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut engine = legacy_engine(worker_cancel);
            // native 呼び出しまで届く小ささ。上限超過の検査自体は別の常設テストで確認する。
            engine.set_max_operations(64);
            engine.register_fn("enter_native", move || entered_tx.send(()).unwrap());
            let result = engine.run(source);
            let _ = done_tx.send(result.map_err(|e| e.to_string()));
        });
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert!(matches!(
            done_rx.recv_timeout(Duration::from_secs(3)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
    }

    #[test]
    #[ignore = "修正前の停止確認専用。sleep 中のスレッドを残すため単独のプロセスで 1 回だけ実行"]
    fn section1863_legacy_sleep_ignores_cancel_and_operation_limit() {
        // join できないスレッドを残す。テストプロセス終了時に OS が回収する。
        assert_legacy_native_does_not_return("enter_native(); sleep(3600);");
    }

    #[test]
    #[ignore = "修正前の停止確認専用。pad が CPU 1 コアを回し続けるため単独のプロセスで 1 回だけ実行"]
    fn section1863_legacy_empty_pad_ignores_cancel_and_operation_limit() {
        // 無限ループのスレッドを残すので通常のテストに混ぜない。
        assert_legacy_native_does_not_return(r#"enter_native(); "a".pad(5, "");"#);
    }

    #[test]
    fn section1863_legacy_helper_enforces_operation_limit_for_script_loops() {
        within_timeout(|| {
            let mut engine = legacy_engine(Arc::new(AtomicBool::new(false)));
            engine.set_max_operations(64);
            assert!(matches!(
                *engine.run("loop {}").unwrap_err(),
                EvalAltResult::ErrorTooManyOperations(_)
            ));
        });
    }

    #[test]
    fn section1863_legacy_script_loop_observes_cancel_and_terminates() {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut engine = legacy_engine(worker_cancel);
            engine.register_fn("enter_loop", move || entered_tx.send(()).unwrap());
            let result = engine.run("enter_loop(); loop {}");
            let terminated = matches!(result, Err(error) if matches!(*error, EvalAltResult::ErrorTerminated(_, _)));
            let _ = done_tx.send(terminated);
        });
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert!(done_rx.recv_timeout(Duration::from_secs(10)).unwrap());
    }

    #[test]
    fn section1863_string_pad_preserves_standard_const_rejection() {
        within_timeout(|| {
            // method 形は定数の変更として拒否される。関数形は定数の写しを渡すので
            // 標準でもエラーにならず、定数は変わらない。
            for (call, standard_rejects) in [
                (r#"s.pad(5, "x")"#, true),
                (r#"pad(s, 5, "x")"#, false),
                (r#"s.pad(5, "")"#, true),
                (r#"s.pad(1, "")"#, true),
            ] {
                let source = format!(r#"const s = "a"; {call}; s"#);
                let mut standard_scope = Scope::new();
                let mut stack_scope = Scope::new();
                let standard = Engine::new()
                    .eval_with_scope::<Dynamic>(&mut standard_scope, &source)
                    .map_err(|e| e.to_string());
                let actual = build_engine()
                    .eval_with_scope::<Dynamic>(&mut stack_scope, &source)
                    .map_err(|e| e.to_string());
                // 空文字でも定数変更の拒否が先に働くので、危険な標準 pad には入らない。
                assert_eq!(standard.is_err(), standard_rejects, "{source}");
                assert_eq!(
                    actual.map(|v| v.to_string()),
                    standard.map(|v| v.to_string()),
                    "{source}"
                );
                let standard_value = standard_scope.get_value::<ImmutableString>("s").unwrap();
                assert_eq!(standard_value.as_str(), "a");
                assert_eq!(
                    stack_scope.get_value::<ImmutableString>("s"),
                    Some(standard_value)
                );
            }
        });
    }

    #[test]
    fn section1863_array_pad_rejects_overflow_lengths_without_panicking() {
        for len in [6148914691236517206_i64, i64::MAX] {
            let source = format!("fn group(files) {{ let a = []; a.pad({len}, [(), ()]); [] }}");
            let error = within_timeout(move || group_keys(&[], &source).err())
                .expect("巨大な配列 pad は panic ではなく Err");
            assert!(error.contains("スクリプトの実行に失敗:"), "{error}");
            assert!(error.contains("Size of array/BLOB"), "{error}");
        }
        within_timeout(|| {
            let error = build_engine()
                .run("let a = []; a.pad(6148914691236517206, [(), ()]);")
                .unwrap_err();
            assert!(
                matches!(*error, EvalAltResult::ErrorDataTooLarge(ref kind, _) if kind == "Size of array/BLOB")
            );
        });
    }

    #[test]
    fn section1863_array_pad_matches_standard_results_and_aggregate_limits() {
        within_timeout(|| {
            for (source, limits, succeeds) in [
                ("let a = [1]; a.pad(3, 7); a", (3, 8, 8), true),
                ("let a = [1]; a.pad(4, 7); a", (3, 8, 8), false),
                ("let a = [1]; a.pad(-1, [1]); a", (3, 8, 8), true),
                ("let a = [1]; a.pad(0, [1]); a", (3, 8, 8), true),
                ("let a = [1]; a.pad(1, [1]); a", (3, 8, 8), true),
                ("let a = []; a.pad(2, [1, 2]); a", (6, 8, 8), true),
                ("let a = []; a.pad(2, [1, 2]); a", (5, 8, 8), false),
                (r#"let a = []; a.pad(2, "ab"); a"#, (2, 8, 4), true),
                (r#"let a = []; a.pad(2, "ab"); a"#, (2, 8, 3), false),
                (
                    r#"let a = []; a.pad(2, #{long_key: [1, "ab"]}); a"#,
                    (6, 2, 4),
                    true,
                ),
                (
                    r#"let a = []; a.pad(2, #{long_key: [1, "ab"]}); a"#,
                    (5, 2, 4),
                    false,
                ),
                (
                    r#"let a = []; a.pad(2, #{long_key: [1, "ab"]}); a"#,
                    (6, 1, 4),
                    false,
                ),
                (
                    r#"let a = []; a.pad(2, #{long_key: [1, "ab"]}); a"#,
                    (6, 2, 3),
                    false,
                ),
                (
                    r#"let a = []; a.pad(2, #{long_key: [1, "ab"]}); a"#,
                    (5, 1, 3),
                    false,
                ),
                (
                    r#"let a = [#{k: "a"}, [2]]; a.pad(3, ["bc"]); a"#,
                    (5, 1, 3),
                    true,
                ),
                (
                    r#"let a = [#{k: "a"}, [2]]; a.pad(3, ["bc"]); a"#,
                    (4, 1, 3),
                    false,
                ),
                (
                    r#"let a = [#{k: "a"}, [2]]; a.pad(3, ["bc"]); a"#,
                    (5, 1, 2),
                    false,
                ),
                ("let a = []; a.pad(2, blob(2)); a", (8, 8, 8), true),
                ("let a = []; a.pad(2, blob(2)); a", (7, 8, 8), false),
                ("let a = []; a.pad(2, blob(2)); a", (5, 8, 8), false),
                ("let a = [blob(2)]; a.pad(2, 0); a", (5, 8, 8), true),
                ("let a = [blob(2)]; a.pad(2, 0); a", (4, 8, 8), false),
                ("let a = []; a.pad(2, [blob(2)]); a", (10, 8, 8), true),
                ("let a = []; a.pad(2, [blob(2)]); a", (9, 8, 8), false),
                (
                    r#"let item = [1, "ab"]; let f = || item; let a = []; a.pad(2, item); a"#,
                    (6, 8, 4),
                    true,
                ),
                (
                    r#"let item = [1, "ab"]; let f = || item; let a = []; a.pad(2, item); a"#,
                    (5, 8, 4),
                    false,
                ),
                // 配列上限 0 のときの標準の分岐と、非 pure の定数拒否も比較する。
                ("let a = []; a.pad(3, [1]); a", (0, 8, 8), true),
                ("const a = [1]; a.pad(2, 0); a", (8, 8, 8), false),
            ] {
                let evaluate = |mut engine: Engine| {
                    engine.set_max_array_size(limits.0);
                    engine.set_max_map_size(limits.1);
                    engine.set_max_string_size(limits.2);
                    let mut scope = Scope::new();
                    let result = engine
                        .eval_with_scope::<Dynamic>(&mut scope, source)
                        .map(|value| format!("{value:?}"))
                        .map_err(|error| error.to_string());
                    // 事前検査と呼び出し後の検査で、変更済みかどうかも標準と一致させる。
                    let array = scope
                        .get_value::<rhai::Array>("a")
                        .map(|a| format!("{a:?}"));
                    (result, array)
                };
                let expected = evaluate(Engine::new());
                assert_eq!(
                    expected.0.is_ok(),
                    succeeds,
                    "標準の前提: {source}, {limits:?}: {expected:?}"
                );
                let actual = evaluate(build_engine());
                assert_eq!(actual, expected, "{source}, {limits:?}");
            }
        });
    }

    #[test]
    fn section1863_data_sizes_match_standard_for_top_shared_and_nested_blobs() {
        within_timeout(|| {
            let mut map = rhai::Map::new();
            map.insert("key_not_counted".into(), Dynamic::from_blob(vec![0, 1]));
            let value = Dynamic::from_array(vec![
                Dynamic::from_map(map),
                Dynamic::from_array(vec![Dynamic::from("ab")]),
            ])
            .into_shared();
            // 配列要素 2 + 入れ子 blob (1+2) + 入れ子配列の要素 1、map は 1、文字列は 2。
            let sizes = stack_data_sizes(&value, true);
            assert_eq!(sizes, (6, 1, 2));
            for (a, m, s, succeeds) in [(6, 1, 2, true), (5, 1, 2, false), (6, 1, 1, false)] {
                let mut engine = Engine::new();
                engine.set_max_array_size(a);
                engine.set_max_map_size(m);
                engine.set_max_string_size(s);
                assert_eq!(
                    engine.ensure_data_size_within_limits(&value).is_ok(),
                    succeeds
                );
            }
            let blob = Dynamic::from_blob(vec![0, 1]).into_shared();
            assert_eq!(stack_data_sizes(&blob, true), (2, 0, 0));
        });
    }

    #[test]
    fn section1863_global_overrides_reject_sleep_int_float_and_zero() {
        for body in ["sleep(1)", "sleep(0.5)", "sleep(0)"] {
            assert_script_runtime_error(body, SLEEP_ERROR);
        }
    }

    #[test]
    fn section1863_global_overrides_reject_empty_pad_method_and_function() {
        for body in [
            r#"let s = "a"; s.pad(5, "")"#,
            r#"let s = "a"; pad(s, 5, "")"#,
            r#""a".pad(5, "")"#,
        ] {
            assert_script_runtime_error(body, EMPTY_PAD_ERROR);
        }
    }

    #[test]
    fn section1863_global_overrides_reject_sleep_function_pointers_and_curry() {
        for body in [
            r#"Fn("sleep").call(1)"#,
            r#"call(Fn("sleep"), 0.5)"#,
            r#"let f = Fn("sleep").curry(1); f.call()"#,
            r#"let f = curry(Fn("sleep"), 0); call(f)"#,
        ] {
            assert_script_runtime_error(body, SLEEP_ERROR);
        }
    }

    #[test]
    fn section1863_global_overrides_reject_empty_pad_function_pointers() {
        for body in [
            r#"let s = "a"; Fn("pad").call(s, 5, "")"#,
            r#"let f = Fn("pad").curry("a", 5, ""); call(f)"#,
        ] {
            assert_script_runtime_error(body, EMPTY_PAD_ERROR);
        }
    }

    #[test]
    fn section1863_safe_string_and_array_pad_remain_compatible() {
        within_timeout(|| {
            for (body, expected) in [
                (r#"let s = "a"; s.pad(5, "ab"); s"#, "aabab"),
                (r#"let s = "a"; pad(s, 4, "ab"); s"#, "aaba"),
                (r#"let s = "a"; s.pad(5, '*'); s"#, "a****"),
                (r#"let s = "abc"; s.pad(2, ""); s"#, "abc"),
                (r#"let s = "abc"; s.pad(3, ""); s"#, "abc"),
                (r#"let s = "abc"; s.pad(0, ""); s"#, "abc"),
                (r#"let s = "あ"; s.pad(4, "いう"); s"#, "あいうい"),
                (r#"let a = ["a"]; a.pad(3, "b"); a[2]"#, "b"),
            ] {
                // 文を含む式はブロックで囲み、戻り値を 1 件のキーにする。
                let source = format!("fn group(files) {{ [{{ {body} }}] }}");
                let result = group_keys(&[m("a.jpg", 0, false)], &source).expect("互換の pad");
                assert_eq!(result.keys, vec![expected.to_string()], "{body}");
            }
        });
    }

    #[test]
    fn section1863_string_pad_preserves_character_and_byte_size_checks() {
        within_timeout(|| {
            let mut engine = build_engine();
            engine.set_max_string_size(4);
            for source in [
                r#"let s = "a"; s.pad(5, "b");"#,
                r#"let s = "a"; s.pad(3, "あ");"#,
            ] {
                assert!(matches!(
                    *engine.run(source).unwrap_err(),
                    EvalAltResult::ErrorDataTooLarge(_, _)
                ));
            }
        });
    }

    fn script_with_output(source: &str) -> (CompiledStackScript, Arc<Mutex<Vec<String>>>) {
        let mut script = CompiledStackScript::compile(source, Arc::new(AtomicBool::new(false)))
            .expect("コンパイル成功");
        let lines = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&lines);
        install_script_output(&mut script.engine, move |line| {
            captured.lock().unwrap().push(line)
        });
        (script, lines)
    }

    #[test]
    fn section1863_print_thirty_times_succeeds_and_logs_twenty_lines() {
        within_timeout(|| {
            let (script, lines) =
                script_with_output(r#"fn group(files) { for i in 0..30 { print(i); } [] }"#);
            script.group_keys(&[]).expect("出力を省略しても分類は成功");
            let lines = lines.lock().unwrap();
            assert_eq!(lines.len(), 21);
            for (i, line) in lines[..20].iter().enumerate() {
                assert_eq!(line, &format!("stack script print: {i}"));
            }
            assert_eq!(lines[20], SCRIPT_OUTPUT_OMITTED);
        });
    }

    #[test]
    fn section1863_print_debug_share_limit_across_group_calls_and_include_location() {
        within_timeout(|| {
            let (mut script, lines) = script_with_output(
                r#"fn group(files) { for i in 0..6 { print(i); debug(i); } [] }"#,
            );
            script.ast.set_source("stack-test");
            for _ in 0..3 {
                script.group_keys(&[]).expect("親フォルダごとの呼び出し");
            }
            let lines = lines.lock().unwrap();
            assert_eq!(lines.len(), 21);
            assert!(lines[1].starts_with("stack script debug: stack-test @ "));
            assert!(lines[1].contains(" > 0"));
            assert_eq!(lines[20], SCRIPT_OUTPUT_OMITTED);
        });
    }

    #[test]
    fn section1863_multiline_print_counts_log_lines_and_new_compile_resets_limit() {
        within_timeout(|| {
            for _ in 0..2 {
                let (script, lines) = script_with_output(
                    r#"fn group(files) { for i in 0..15 { print("a\nb"); } [] }"#,
                );
                script.group_keys(&[]).unwrap();
                let lines = lines.lock().unwrap();
                assert_eq!(lines.len(), 21);
                assert!(lines[..20].iter().all(|line| !line.contains('\n')));
                assert_eq!(lines[20], SCRIPT_OUTPUT_OMITTED);
            }
        });
    }

    fn m(name: &str, mtime: i64, is_video: bool) -> StackMember {
        StackMember {
            path: PathBuf::from(format!(r"C:\dl\{name}")),
            mtime,
            size: Some(0),
            sort_meta: crate::settings::ListingSortMetadata::new(mtime, Some(0)),
            is_video,
        }
    }

    fn run(media: &[StackMember]) -> GroupingResult {
        group_keys(media, DEFAULT_SCRIPT).expect("default script runs")
    }

    #[test]
    fn default_script_has_no_carriage_returns() {
        assert!(
            !DEFAULT_SCRIPT.contains('\r'),
            "DEFAULT_SCRIPT must use LF; check .gitattributes for assets/stack_rules.default.rhai text eol=lf"
        );
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
    fn suffix_fixed_pattern_groups_single_post() {
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

    #[test]
    fn cancel_flag_aborts_long_running_script() {
        // 既にキャンセル済みなら、重いループ (10 万件相当の処理) を最後まで走らせず
        // 途中で中断し Err を返す (= ワーカーが結果を捨てる前提)。
        let cancel = Arc::new(AtomicBool::new(true));
        // Rhai は `let mut` を持たず変数は既定で可変。重いループで on_progress を多数回踏ませる。
        let src = r#"
            fn group(files) {
                let x = 0;
                for i in 0..5000000 { x += 1; }
                files.map(|f| f.name)
            }
        "#;
        let media = vec![m("a.jpg", 0, false)];
        let r = group_keys_cancellable(&media, src, cancel);
        assert!(r.is_err(), "cancel 済みなら中断して Err を返すはず");
    }

    #[test]
    fn no_cancel_completes_normally() {
        // キャンセルされていなければ同じ重いループでも完走する (= on_progress が誤爆しない)。
        let cancel = Arc::new(AtomicBool::new(false));
        let src = r#"
            fn group(files) {
                let x = 0;
                for i in 0..1000 { x += 1; }
                files.map(|f| f.stem)
            }
        "#;
        let media = vec![m("a.jpg", 0, false), m("b.jpg", 0, false)];
        let r = group_keys_cancellable(&media, src, cancel).expect("完走するはず");
        assert_eq!(r.keys.len(), 2);
    }

    // 10k ファイルでの既定スクリプト実行時間を測る (perf 計測)。
    // 実態は release ビルドなので、必ず `--release` で走らせる:
    //   cargo test --release --lib filename_stack_script::tests::perf_10k -- --ignored --nocapture
    // (debug ビルドの Rhai インタプリタは桁違いに遅く、実アプリの目安にならない。)
    #[test]
    #[ignore = "perf 計測。`cargo test --release ... -- --ignored --nocapture` で実行"]
    fn perf_10k_default_script() {
        use std::time::Instant;

        // 代表的なフォルダ形状を size 件ぶん作る (採用ルールが異なる = 走るルール数が違う)。
        fn suffix_fixed(n: usize) -> Vec<StackMember> {
            // ルール1 (命名パターン) で即採用 — 1 走査
            (0..n)
                .map(|i| {
                    m(
                        &format!(
                            "20260429_1100_{:04}_{:010}_p01_m{:02}_@artist.jpg",
                            i / 4,
                            i / 4,
                            i % 4
                        ),
                        0,
                        false,
                    )
                })
                .collect()
        }
        fn pixiv(n: usize) -> Vec<StackMember> {
            // ルール2 (末尾連番) で採用 — rule1 全走査 + rule2 全走査
            (0..n)
                .map(|i| m(&format!("{:08}_p{}.jpg", i / 5, i % 5), 0, false))
                .collect()
        }
        fn camera(n: usize) -> Vec<StackMember> {
            // 連写 — rule1/2/3 を全走査して落ち rule4 で採用 (最重ケース)
            (0..n)
                .map(|i| {
                    m(
                        &format!("IMG_{:06}.jpg", 1000 + i),
                        100 * (i / 5) as i64 + (i % 5) as i64,
                        false,
                    )
                })
                .collect()
        }

        for &size in &[10_000usize, 100_000usize] {
            let scenarios: [(&str, Vec<StackMember>); 3] = [
                ("suffix_fixed (rule1 即採用)", suffix_fixed(size)),
                ("pixiv (rule2 採用)", pixiv(size)),
                ("camera (rule4=最重)", camera(size)),
            ];
            for (name, media) in &scenarios {
                // warmup (regex キャッシュを温める = アプリ稼働中の定常状態に合わせる)。
                let warm = match group_keys(media, DEFAULT_SCRIPT) {
                    Ok(r) => r,
                    Err(e) => {
                        println!("[perf] {size:>7} {name:<22} ERROR: {e}");
                        continue;
                    }
                };
                assert_eq!(warm.keys.len(), media.len());
                let mut best = f64::INFINITY;
                for _ in 0..3 {
                    let t = Instant::now();
                    let r = group_keys(media, DEFAULT_SCRIPT).expect("runs");
                    let ms = t.elapsed().as_secs_f64() * 1000.0;
                    std::hint::black_box(&r);
                    best = best.min(ms);
                }
                println!(
                    "[perf] {size:>7} {name:<22} best={best:7.1}ms rule={:?}",
                    warm.rule
                );
            }
        }
    }
}
