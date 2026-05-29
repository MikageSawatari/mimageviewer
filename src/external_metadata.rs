//! 外部メタデータサイドカー (画像と同名の `.json` / `.txt`) の検出・抽出。
//! 設計は docs/sidecar-metadata-ingest.md。
//!
//! - **読み取り専用**。mIV はサイドカーを生成・更新しない。
//! - JSON は **リーフ値のみ** を連結して自由語検索テキストにする (キー名は含めない)。
//! - TXT は全文をそのまま検索テキストにする。
//! - mIV タグ機能 (`#xxx`, dc:subject) とは別系統。サイドカーからタグ抽出はしない。
//!
//! 既存の `src/sidecar.rs` (`mimageviewer.dat` バックアップ系) とは無関係な別モジュール。

use std::path::{Path, PathBuf};

/// サイドカー 1 ファイルの上限サイズ。これを超えるものはスキップする (暴走ファイル対策)。
const MAX_SIDECAR_BYTES: u64 = 2 * 1024 * 1024;
/// 抽出後の検索テキスト連結上限 (bigram 索引肥大・誤ヒット抑制)。
const MAX_TEXT_BYTES: usize = 256 * 1024;

/// 画像 `image_path` に対応するサイドカー候補を §4 の優先順位で列挙する。
/// `<full>.json` → `<full>.txt` → `<stem>.json` → `<stem>.txt`。
fn candidate_paths(image_path: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::with_capacity(4);
    let Some(file_name) = image_path.file_name().and_then(|n| n.to_str()) else {
        return out;
    };
    // <full>.json / <full>.txt (拡張子込みファイル名 + サイドカー拡張子)
    out.push(image_path.with_file_name(format!("{file_name}.json")));
    out.push(image_path.with_file_name(format!("{file_name}.txt")));
    // <stem>.json / <stem>.txt (拡張子を置換する流儀)。
    // stem == file_name (拡張子なしファイル) のときは重複するので足さない。
    if let Some(stem) = image_path.file_stem().and_then(|s| s.to_str()) {
        if stem != file_name {
            out.push(image_path.with_file_name(format!("{stem}.json")));
            out.push(image_path.with_file_name(format!("{stem}.txt")));
        }
    }
    out
}

/// 優先順位で最初に存在したサイドカーを `(path, metadata)` で返す。
/// 存在チェックのみ (ディレクトリ走査はしない)。最大 4 回の `metadata` syscall。
fn detect_with_meta(image_path: &Path) -> Option<(PathBuf, std::fs::Metadata)> {
    for cand in candidate_paths(image_path) {
        if let Ok(md) = std::fs::metadata(&cand) {
            if md.is_file() {
                return Some((cand, md));
            }
        }
    }
    None
}

/// 画像に対応するサイドカーのパス (優先順位で最初の 1 つ)。無ければ `None`。
pub fn detect_sidecar(image_path: &Path) -> Option<PathBuf> {
    detect_with_meta(image_path).map(|(p, _)| p)
}

/// 3-way diff 用のサイドカー署名 `(mtime_secs, size)`。
/// walker / supervisor がこれを画像の署名に織り込み、サイドカーの追加・編集・削除を
/// 「変化あり」として検出できるようにする (docs §14-3/§14-4)。サイドカー無しは `None`。
pub fn sidecar_signature(image_path: &Path) -> Option<(i64, i64)> {
    let (_, md) = detect_with_meta(image_path)?;
    Some((mtime_secs(&md), md.len() as i64))
}

/// 画像に対応するサイドカーから検索用テキストを取り出す。
/// JSON: リーフ値のみ連結 (キー名なし)。TXT: 全文。
/// サイズ超過・パース失敗・不正文字コードは `None` + ログ 1 行に倒す。
/// 戻り値は **未正規化** (呼び出し側が `normalize_for_match` を適用する)。
pub fn read_search_text(image_path: &Path) -> Option<String> {
    let (path, md) = detect_with_meta(image_path)?;
    if md.len() > MAX_SIDECAR_BYTES {
        crate::logger::log(format!(
            "external_metadata: sidecar too large ({} bytes), skipping: {}",
            md.len(),
            path.display()
        ));
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    if is_json_ext(&path) {
        match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(v) => {
                let mut out = String::new();
                collect_json_values(&v, &mut out);
                (!out.trim().is_empty()).then_some(out)
            }
            Err(e) => {
                crate::logger::log(format!(
                    "external_metadata: JSON parse failed ({}): {e}",
                    path.display()
                ));
                None
            }
        }
    } else {
        // TXT: 全文 (不正バイトは置換)。
        let text = String::from_utf8_lossy(&bytes);
        let capped = truncate_on_char_boundary(&text, MAX_TEXT_BYTES);
        (!capped.trim().is_empty()).then(|| capped.to_string())
    }
}

/// 右パネル表示用のサイドカー内容。JSON は構造を保ったまま (key も表示する) ツリー描画し、
/// TXT はテキストとして表示する (docs §11)。検索用 (`read_search_text`、値のみ) とは別。
#[derive(Clone, Debug)]
pub enum SidecarDisplay {
    Json(serde_json::Value),
    Text(String),
}

/// 画像に対応するサイドカーを右パネル表示用に読む。JSON はパースして構造を保持、
/// TXT はテキスト。サイズ超過・パース失敗・不正は `None`。
pub fn read_for_display(image_path: &Path) -> Option<SidecarDisplay> {
    let (path, md) = detect_with_meta(image_path)?;
    if md.len() > MAX_SIDECAR_BYTES {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    if is_json_ext(&path) {
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .map(SidecarDisplay::Json)
    } else {
        let text = String::from_utf8_lossy(&bytes);
        let capped = truncate_on_char_boundary(&text, MAX_TEXT_BYTES).to_string();
        (!capped.trim().is_empty()).then_some(SidecarDisplay::Text(capped))
    }
}

/// 監視 (notify) でサイドカー (`*.json` / `*.txt`) の変更イベントが届いたとき、
/// 再 ingest すべき兄弟画像を逆引きする (docs §14-2)。
/// - `<full>` 形式 (`foo.jpg.json`): 拡張子を剥がすと既存画像 `foo.jpg` → それを返す。
/// - `<stem>` 形式 (`foo.json`): 同ディレクトリの `foo.<imgext>` 画像を列挙して返す。
/// 対応画像が無い (孤立サイドカー) なら空 Vec。
pub fn images_for_sidecar(sidecar_path: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if !is_json_ext(sidecar_path) && !is_txt_ext(sidecar_path) {
        return out;
    }
    let Some(parent) = sidecar_path.parent() else {
        return out;
    };
    // 拡張子を 1 つ剥がした base ("foo.jpg.json" -> "foo.jpg", "foo.json" -> "foo")
    let Some(base) = sidecar_path.file_stem().and_then(|s| s.to_str()) else {
        return out;
    };

    // <full> 形式: base がそのまま既存画像ファイルか
    let full_candidate = parent.join(base);
    if is_image_file(&full_candidate) {
        out.push(full_candidate);
    }

    // <full> で解決できなければ <stem> 形式として、同 stem の画像を列挙する
    if out.is_empty() {
        if let Ok(rd) = std::fs::read_dir(parent) {
            for entry in rd.flatten() {
                match entry.file_type() {
                    Ok(ft) if ft.is_file() => {}
                    _ => continue,
                }
                let p = entry.path();
                if !is_image_file(&p) {
                    continue;
                }
                if p.file_stem().and_then(|s| s.to_str()) == Some(base) {
                    out.push(p);
                }
            }
        }
    }
    out
}

// -----------------------------------------------------------------------
// 内部ヘルパ
// -----------------------------------------------------------------------

fn mtime_secs(md: &std::fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn ext_eq(path: &Path, want: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case(want))
        .unwrap_or(false)
}

fn is_json_ext(path: &Path) -> bool {
    ext_eq(path, "json")
}

fn is_txt_ext(path: &Path) -> bool {
    ext_eq(path, "txt")
}

fn is_image_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    crate::folder_tree::is_recognized_image_ext(&ext)
}

/// JSON 値を再帰走査し、リーフ値 (String / Number / Bool) のみをスペース区切りで連結する。
/// オブジェクトのキー名は **含めない**。`MAX_TEXT_BYTES` で打ち切る。
fn collect_json_values(v: &serde_json::Value, out: &mut String) {
    if out.len() >= MAX_TEXT_BYTES {
        return;
    }
    match v {
        serde_json::Value::Null => {}
        serde_json::Value::Bool(b) => push_token(out, if *b { "true" } else { "false" }),
        serde_json::Value::Number(n) => push_token(out, &n.to_string()),
        serde_json::Value::String(s) => push_token(out, s),
        serde_json::Value::Array(a) => {
            for e in a {
                collect_json_values(e, out);
            }
        }
        serde_json::Value::Object(m) => {
            for (_k, val) in m {
                // キー名は索引に含めない (値のみ)
                collect_json_values(val, out);
            }
        }
    }
}

fn push_token(out: &mut String, tok: &str) {
    if tok.is_empty() || out.len() >= MAX_TEXT_BYTES {
        return;
    }
    if !out.is_empty() {
        out.push(' ');
    }
    let remaining = MAX_TEXT_BYTES.saturating_sub(out.len());
    if tok.len() <= remaining {
        out.push_str(tok);
    } else {
        out.push_str(truncate_on_char_boundary(tok, remaining).as_ref());
    }
}

/// `s` を最大 `max_bytes` バイトまで char 境界で切り詰める。
fn truncate_on_char_boundary(s: &str, max_bytes: usize) -> std::borrow::Cow<'_, str> {
    if s.len() <= max_bytes {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    std::borrow::Cow::Borrowed(&s[..end])
}

// -----------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn detect_prefers_full_json_over_others() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "0001.jpg", b"img");
        write(tmp.path(), "0001.jpg.json", b"{}");
        write(tmp.path(), "0001.jpg.txt", b"x");
        write(tmp.path(), "0001.json", b"{}");
        let sc = detect_sidecar(&img).unwrap();
        assert!(sc.ends_with("0001.jpg.json"), "got {}", sc.display());
    }

    #[test]
    fn detect_full_txt_before_stem_json() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "0001.jpg", b"img");
        write(tmp.path(), "0001.jpg.txt", b"x");
        write(tmp.path(), "0001.json", b"{}");
        let sc = detect_sidecar(&img).unwrap();
        assert!(sc.ends_with("0001.jpg.txt"), "got {}", sc.display());
    }

    #[test]
    fn detect_stem_form_when_no_full() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "0001.jpg", b"img");
        write(tmp.path(), "0001.json", b"{}");
        let sc = detect_sidecar(&img).unwrap();
        assert!(sc.ends_with("0001.json"), "got {}", sc.display());
    }

    #[test]
    fn detect_none_when_absent() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "0001.jpg", b"img");
        assert!(detect_sidecar(&img).is_none());
    }

    #[test]
    fn json_values_only_no_keys() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "a.jpg", b"img");
        write(
            tmp.path(),
            "a.jpg.json",
            br#"{ "id": 11457572, "rating": "g", "score": 0,
                 "tags": ["1girl", "blue_eyes"],
                 "artist": "karon-t",
                 "source": "https://example.invalid/img/143512783_p0.jpg",
                 "image_width": 1000 }"#,
        );
        let text = read_search_text(&img).unwrap();
        // 値が入る
        assert!(text.contains("1girl"), "text={text}");
        assert!(text.contains("blue_eyes"));
        assert!(text.contains("karon-t"));
        assert!(text.contains("143512783"));
        assert!(text.contains("1000"));
        // キー名は入らない
        assert!(!text.contains("rating"), "key name leaked: {text}");
        assert!(!text.contains("artist"), "key name leaked: {text}");
        assert!(!text.contains("source"), "key name leaked: {text}");
        assert!(!text.contains("image_width"));
    }

    #[test]
    fn txt_whole_text() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "b.png", b"img");
        write(tmp.path(), "b.png.txt", b"1girl\nblue_eyes\ntwintails\n");
        let text = read_search_text(&img).unwrap();
        assert!(text.contains("1girl"));
        assert!(text.contains("twintails"));
    }

    #[test]
    fn broken_json_returns_none() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "c.jpg", b"img");
        write(tmp.path(), "c.jpg.json", b"{ this is not valid json ");
        assert!(read_search_text(&img).is_none());
    }

    #[test]
    fn oversized_sidecar_skipped() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "d.jpg", b"img");
        let big = vec![b'x'; (MAX_SIDECAR_BYTES + 1) as usize];
        write(tmp.path(), "d.jpg.txt", &big);
        assert!(read_search_text(&img).is_none());
    }

    #[test]
    fn signature_changes_with_sidecar() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "e.jpg", b"img");
        assert!(sidecar_signature(&img).is_none());
        write(tmp.path(), "e.jpg.json", br#"{"a":1}"#);
        let (_mtime, size) = sidecar_signature(&img).unwrap();
        assert!(size > 0);
    }

    #[test]
    fn reverse_map_full_form() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "f.jpg", b"img");
        let sc = write(tmp.path(), "f.jpg.json", b"{}");
        let imgs = images_for_sidecar(&sc);
        assert_eq!(imgs.len(), 1);
        assert_eq!(imgs[0], img);
    }

    #[test]
    fn reverse_map_stem_form_multiple() {
        let tmp = TempDir::new().unwrap();
        let jpg = write(tmp.path(), "g.jpg", b"img");
        let png = write(tmp.path(), "g.png", b"img");
        let sc = write(tmp.path(), "g.json", b"{}");
        let mut imgs = images_for_sidecar(&sc);
        imgs.sort();
        let mut expected = vec![jpg, png];
        expected.sort();
        assert_eq!(imgs, expected);
    }

    #[test]
    fn reverse_map_orphan_sidecar_empty() {
        let tmp = TempDir::new().unwrap();
        let sc = write(tmp.path(), "orphan.json", b"{}");
        assert!(images_for_sidecar(&sc).is_empty());
    }
}
