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

/// 3-way diff 用のサイドカー署名。walker / supervisor がこれを画像の署名に織り込み、
/// サイドカーの追加・編集・削除・**優先順位の切替** を「変化あり」として検出する
/// (docs §14-3/§14-4)。サイドカー無しは `None`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidecarSig {
    /// 選択されたサイドカーの mtime (秒)。差分用 mtime に `max` で織り込む。
    pub mtime: i64,
    /// サイドカーの識別子: **ファイル名 + size** のハッシュ (常に 1 以上の有界値)。
    /// 差分用 size に加算することで、size 変化だけでなく
    /// 「`a.jpg.json` 消失 → 同 mtime/size の `a.json` に切替」のような
    /// **優先順位プローブの結果が変わったケース** も検出する (Codex P3)。
    pub fingerprint: i64,
}

/// 画像に対応するサイドカーの 3-way diff 署名。無ければ `None`。
pub fn sidecar_signature(image_path: &Path) -> Option<SidecarSig> {
    let (path, md) = detect_with_meta(image_path)?;
    let size = md.len() as i64;
    // ファイル名 + size を安定ハッシュ。DefaultHasher は固定鍵 SipHash なので run/プロセスを
    // またいで決定的 (Rust バージョン更新で値が変わっても、最悪 1 度の再 ingest で済む)。
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        name.hash(&mut h);
    }
    size.hash(&mut h);
    // i64 に収め、加算しても画像 size と合わせて i64 を溢れさせない有界値 (約 30bit) + 1。
    let fingerprint = (h.finish() % 1_000_000_007) as i64 + 1;
    Some(SidecarSig {
        mtime: mtime_secs(&md),
        fingerprint,
    })
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
    let raw = std::fs::read(&path).ok()?;
    let bytes = strip_utf8_bom(&raw);
    if is_json_ext(&path) {
        match serde_json::from_slice::<serde_json::Value>(bytes) {
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
        let text = String::from_utf8_lossy(bytes);
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
    let raw = std::fs::read(&path).ok()?;
    let bytes = strip_utf8_bom(&raw);
    if is_json_ext(&path) {
        serde_json::from_slice::<serde_json::Value>(bytes)
            .ok()
            .map(SidecarDisplay::Json)
    } else {
        let text = String::from_utf8_lossy(bytes);
        let capped = truncate_on_char_boundary(&text, MAX_TEXT_BYTES).to_string();
        (!capped.trim().is_empty()).then_some(SidecarDisplay::Text(capped))
    }
}

/// manifest relative page 用。sidecar candidate も画像と同じ trust root の下で開き、
/// containment を確認した同一ハンドルからだけ読む。
#[allow(dead_code)] // lib target does not compile the App metadata consumer
pub(crate) fn read_for_display_verified(
    provenance: &crate::book_bookmarks::RelativePageProvenance,
) -> Option<SidecarDisplay> {
    for path in candidate_paths(&provenance.candidate_path()) {
        let Some(candidate) = provenance.for_candidate(&path) else {
            continue;
        };
        let Ok(opened) = candidate.open_verified() else {
            continue;
        };
        let Ok(metadata) = opened.metadata() else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > MAX_SIDECAR_BYTES {
            continue;
        }
        let Ok(raw) = opened.read_to_end() else {
            continue;
        };
        let bytes = strip_utf8_bom(&raw);
        let display = if is_json_ext(&path) {
            serde_json::from_slice::<serde_json::Value>(bytes)
                .ok()
                .map(SidecarDisplay::Json)
        } else {
            let text = String::from_utf8_lossy(bytes);
            let capped = truncate_on_char_boundary(&text, MAX_TEXT_BYTES).to_string();
            (!capped.trim().is_empty()).then_some(SidecarDisplay::Text(capped))
        };
        if display.is_some() {
            return display;
        }
    }
    None
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

/// 先頭の UTF-8 BOM (`EF BB BF`) を取り除く。JSON は RFC 8259 上 BOM を付けない規定だが、
/// 一部の Windows ツールが付与するため、付いていればパース前に剥がす
/// (docs/archive/search-metadata/sidecar-encoding-utf8.md §3.2 / TC4)。BOM を残すと serde_json が先頭で
/// パース失敗し、TXT では先頭に U+FEFF が残って表示・検索が崩れる。
fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

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
        let sig = sidecar_signature(&img).unwrap();
        assert!(
            sig.fingerprint >= 1,
            "present sidecar fingerprint must be >= 1"
        );
    }

    #[test]
    fn fingerprint_differs_for_same_size_priority_switch() {
        // Codex P3: `a.jpg.json` (優先1) 消失 → 同 size の `a.json` (優先3) に切替わったとき、
        // mtime/size が偶然一致しても fingerprint がファイル名を含むので署名が変わる。
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "p.jpg", b"img");
        // 同じ 7 バイト・別内容の 2 つのサイドカー (full 形式 / stem 形式)
        write(tmp.path(), "p.jpg.json", br#"{"x":1}"#);
        let sig_full = sidecar_signature(&img).unwrap();
        std::fs::remove_file(tmp.path().join("p.jpg.json")).unwrap();
        write(tmp.path(), "p.json", br#"{"y":2}"#);
        let sig_stem = sidecar_signature(&img).unwrap();
        assert_ne!(
            sig_full.fingerprint, sig_stem.fingerprint,
            "ファイル名が違えば size が同じでも fingerprint は変わるべき"
        );
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

    // ── エンコーディング回帰 (docs/archive/search-metadata/sidecar-encoding-utf8.md の TC1〜TC6) ──────
    // サイドカー JSON/TXT は常に UTF-8 として読む。CP932/ANSI で誤読すると CJK が
    // mojibake (縺ｮ…) になる。生 UTF-8 / \u エスケープ / BOM / 4byte / 不正バイトを網羅。

    /// TC1: 生 UTF-8・BOM 無し JSON。値が正しく UTF-8 デコードされる (CP932 誤読しない)。
    #[test]
    fn tc1_raw_utf8_json_values_decode() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "tc1.jpg", b"img");
        write(
            tmp.path(),
            "tc1.jpg.json",
            r#"{ "title": "原神 テスト", "user": { "name": "作者名テスト" },
                 "tags": ["風景", "オリジナル"] }"#
                .as_bytes(),
        );
        let text = read_search_text(&img).unwrap();
        assert!(text.contains("原神 テスト"), "title mojibake: {text}");
        assert!(text.contains("作者名テスト"), "name mojibake: {text}");
        assert!(text.contains("風景"), "tag mojibake: {text}");
        // 表示経路でも正しい Unicode
        match read_for_display(&img).unwrap() {
            SidecarDisplay::Json(v) => {
                assert_eq!(v["title"], serde_json::json!("原神 テスト"));
                assert_eq!(v["user"]["name"], serde_json::json!("作者名テスト"));
            }
            other => panic!("expected Json, got {other:?}"),
        }
    }

    /// TC2: 中国語。生の多バイト UTF-8 をそのまま復元する。
    #[test]
    fn tc2_chinese_json_values_decode() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "tc2.jpg", b"img");
        write(
            tmp.path(),
            "tc2.jpg.json",
            r#"{ "title": "简体字显示测试",
                 "user": { "name": "中文用户名测试" }, "tags": ["风景"] }"#
                .as_bytes(),
        );
        let text = read_search_text(&img).unwrap();
        assert!(text.contains("简体字显示测试"), "title mojibake: {text}");
        assert!(text.contains("中文用户名测试"), "name mojibake: {text}");
    }

    /// TC3: `\uXXXX` エスケープ (ascii 出力ツール対策)。ファイルが純 ASCII でも復元される。
    #[test]
    fn tc3_unicode_escape_decodes() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "tc3.jpg", b"img");
        // ファイルは純 ASCII バイト列 (マルチバイト文字を一切含まない)。
        // 原神 = 原神, 風景 = 風景。JSON パーサの \u 解釈だけで
        // 復元されるので、読み取りバイトエンコーディングに依存せず正しい値になる。
        // 二重バックスラッシュ = ファイル上は 原神 / 風景 という
        // ASCII エスケープ列 (原神 / 風景)。serde_json が \u を解釈して復元する。
        let json = "{ \"title\": \"\\u539f\\u795e\", \"tags\": [\"\\u98a8\\u666f\"] }";
        write(tmp.path(), "tc3.jpg.json", json.as_bytes());
        let text = read_search_text(&img).unwrap();
        assert!(text.contains("原神"), "escape not decoded: {text}");
        assert!(text.contains("風景"), "escape not decoded: {text}");
    }

    /// TC4: UTF-8 BOM 付き JSON。BOM を無視してパースし、キー名に U+FEFF が混ざらない。
    #[test]
    fn tc4_utf8_bom_json_is_parsed() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "tc4.jpg", b"img");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(r#"{ "title": "テスト" }"#.as_bytes());
        write(tmp.path(), "tc4.jpg.json", &bytes);
        // 検索: BOM があっても値が取れる
        let text = read_search_text(&img).expect("BOM JSON must still parse");
        assert!(text.contains("テスト"), "BOM broke search parse: {text}");
        // 表示: キー名が "\u{feff}title" にならない
        match read_for_display(&img).expect("BOM JSON must still parse for display") {
            SidecarDisplay::Json(serde_json::Value::Object(m)) => {
                assert!(
                    m.contains_key("title"),
                    "expected key 'title', keys={:?}",
                    m.keys().collect::<Vec<_>>()
                );
                assert!(!m.contains_key("\u{feff}title"), "BOM leaked into key name");
            }
            other => panic!("expected Json object, got {other:?}"),
        }
    }

    /// TC4-txt: UTF-8 BOM 付き TXT。先頭に U+FEFF が残らない。
    #[test]
    fn tc4_utf8_bom_txt_strips_bom() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "tc4b.jpg", b"img");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("風景\nオリジナル\n".as_bytes());
        write(tmp.path(), "tc4b.jpg.txt", &bytes);
        let text = read_search_text(&img).unwrap();
        assert!(!text.starts_with('\u{feff}'), "BOM not stripped: {text:?}");
        assert!(text.contains("風景"));
    }

    /// TC5: 4 バイト UTF-8 (絵文字・サロゲートペア) もそのまま復元する。
    #[test]
    fn tc5_four_byte_utf8_emoji() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "tc5.jpg", b"img");
        write(
            tmp.path(),
            "tc5.jpg.json",
            r#"{ "title": "🎨art" }"#.as_bytes(),
        );
        let text = read_search_text(&img).unwrap();
        assert!(text.contains("🎨art"), "4-byte utf8 mojibake: {text}");
    }

    /// TC6: 不正バイト混入 JSON。クラッシュせず None に倒れる (堅牢性)。
    #[test]
    fn tc6_invalid_bytes_json_no_crash() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "tc6.jpg", b"img");
        let mut bytes = r#"{ "title": "テス"#.as_bytes().to_vec();
        bytes.push(0xFF);
        bytes.push(0xFE);
        bytes.extend_from_slice(r#"ト" }"#.as_bytes());
        write(tmp.path(), "tc6.jpg.json", &bytes);
        // パース失敗 → None。panic しないことが要件。
        let _ = read_search_text(&img);
        let _ = read_for_display(&img);
    }

    /// TC6-txt: 不正バイト混入 TXT。lossy で読めてクラッシュせず、正常部分は残る。
    #[test]
    fn tc6_invalid_bytes_txt_lossy() {
        let tmp = TempDir::new().unwrap();
        let img = write(tmp.path(), "tc6b.jpg", b"img");
        let mut bytes = "風景".as_bytes().to_vec();
        bytes.push(0xFF);
        write(tmp.path(), "tc6b.jpg.txt", &bytes);
        let text = read_search_text(&img).unwrap();
        assert!(text.contains("風景"), "valid prefix lost: {text}");
    }
}
