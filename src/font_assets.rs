//! 注釈 (comic) 用フォントの列挙と名前解決 (Inc 4c)。
//!
//! テキスト注釈 / オノマトペで使えるフォントを 3 つの出所から集める:
//!   1. 編集用追加パック (`editing_addon::installed_fonts()`) — オノマトペ向けの
//!      OFL 装飾フォント。
//!   2. ユーザー追加フォント (`%APPDATA%/mimageviewer/user_fonts`) — 利用者が
//!      自分で置いた .ttf/.otf/.ttc。
//!   3. システムフォント (Windows レジストリ `…\Fonts`) — インストール済みフォント。
//!
//! ここでは **表示名 → ファイルパス** のマップを作るだけで、フォントの parse は
//! しない (起動を速く保つため)。実際の読み込みは App 側の `ensure_comic_fonts_for`
//! が参照されたフォントだけを遅延ロードする。
//!
//! 表示名 (= `key`) はそのまま `TextBlock::font_key` として永続化される。pack →
//! user → system の順で先勝ち登録するので、同名があれば pack 同梱が優先される
//! (オノマトペプリセットが期待する装飾フォントを確実に拾うため)。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// フォントの出所カテゴリ (ピッカーの絞り込み用)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontCategory {
    /// 編集用追加パック同梱 (オノマトペ向け装飾フォント)。
    Pack,
    /// ユーザーが `user_fonts/` に置いたフォント。
    User,
    /// Windows にインストール済みのシステムフォント。
    System,
}

impl FontCategory {
    /// ピッカーのタブ表示用ラベル。
    pub fn label(self) -> &'static str {
        match self {
            FontCategory::Pack => "追加パック",
            FontCategory::User => "ユーザー追加",
            FontCategory::System => "システム",
        }
    }
}

/// 列挙された 1 フォント。`key` は表示名かつ `TextBlock::font_key` の値。
#[derive(Debug, Clone)]
pub struct FontAsset {
    /// 表示名 (= font_key)。例: "OtomanopeeOne Regular" / "Yu Gothic Medium"。
    pub key: String,
    /// 出所カテゴリ。
    pub category: FontCategory,
}

/// ユーザー追加フォントの置き場 `%APPDATA%/mimageviewer/user_fonts`。
pub fn user_fonts_dir() -> PathBuf {
    crate::data_dir::get().join("user_fonts")
}

/// `.ttf` / `.otf` / `.ttc` かどうかを拡張子だけで判定する (per-entry の
/// `GetFileAttributes` syscall を避けるため `is_file()` は呼ばない)。
fn is_font_ext(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".ttf") || lower.ends_with(".otf") || lower.ends_with(".ttc")
}

/// ファイル名 (stem) から表示ラベルを作る。`_` / `-` を空白にして見やすくする。
/// 例: `OtomanopeeOne-Regular.ttf` → "OtomanopeeOne Regular"。
fn label_from_path(path: &Path) -> Option<String> {
    let stem = path.file_stem().and_then(|s| s.to_str())?;
    let label = stem.replace(['_', '-'], " ");
    let label = label.trim();
    if label.is_empty() {
        None
    } else {
        Some(label.to_string())
    }
}

/// レジストリのフォント値名から末尾の ` (TrueType)` 等の種別タグを落とし、
/// ` & ` で連結された別名は先頭だけ採用する。
pub fn clean_font_name(raw: &str) -> String {
    let without_tag = match raw.rfind('(') {
        Some(idx) => raw[..idx].trim_end(),
        None => raw.trim_end(),
    };
    let first = without_tag.split(" & ").next().unwrap_or(without_tag);
    first.trim().to_string()
}

/// フォント名の比較用キー (空白 / `-` / `_` / `.` を除去して小文字化)。
pub fn font_lookup_key(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '-' | '_' | '.'))
        .flat_map(char::to_lowercase)
        .collect()
}

/// `font_name` が `candidate` を (区切り無視・部分一致で) 含むか。オノマトペ
/// プリセットの `font_candidate`("Otomanopee One" 等) を実フォント名
/// ("OtomanopeeOne Regular" 等) に対応付けるのに使う。
pub fn font_name_matches_candidate(font_name: &str, candidate: &str) -> bool {
    let font_name = font_lookup_key(font_name);
    let candidate = font_lookup_key(candidate);
    !candidate.is_empty() && font_name.contains(&candidate)
}

/// 指定ディレクトリ直下の .ttf/.otf/.ttc を列挙する (pack / user 用)。
fn enumerate_dir_fonts(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !is_font_ext(&name) {
            continue;
        }
        let path = dir.join(name.as_ref());
        if let Some(label) = label_from_path(&path) {
            out.push((label, path));
        }
    }
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    out
}

/// インストール済み Windows フォントをレジストリ (HKLM + HKCU の `…\Fonts`) から
/// 列挙する。値名は "Yu Gothic Medium (TrueType)" 形式、データは
/// `C:\Windows\Fonts\` 相対のファイル名 (HKCU のユーザーフォントは絶対パス)。
#[cfg(windows)]
fn enumerate_system_fonts() -> Vec<(String, PathBuf)> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    const FONTS_SUBKEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts";
    let windows_fonts = PathBuf::from(r"C:\Windows\Fonts");

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<(String, PathBuf)> = Vec::new();

    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        let Ok(key) = RegKey::predef(hive).open_subkey(FONTS_SUBKEY) else {
            continue;
        };
        for value in key.enum_values().flatten() {
            let (raw_name, _) = value;
            let data: String = match key.get_value(&raw_name) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let name = clean_font_name(&raw_name);
            if name.is_empty() {
                continue;
            }
            let path = {
                let p = PathBuf::from(&data);
                if p.is_absolute() {
                    p
                } else {
                    windows_fonts.join(&data)
                }
            };
            if !is_font_ext(&path.to_string_lossy()) {
                continue;
            }
            let dedup_key = name.to_lowercase();
            if seen.insert(dedup_key) {
                out.push((name, path));
            }
        }
    }
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    out
}

#[cfg(not(windows))]
fn enumerate_system_fonts() -> Vec<(String, PathBuf)> {
    Vec::new()
}

/// 注釈用フォントを全出所から列挙し、(ピッカー一覧, 表示名→パス) を返す。
///
/// 先勝ち登録: pack → user → system。pack 同梱が同名のシステムフォントより優先
/// される (オノマトペプリセットの期待フォントを確実に拾うため)。一覧はカテゴリ
/// 内では表示名の辞書順。
pub fn enumerate_comic_fonts() -> (Vec<FontAsset>, HashMap<String, PathBuf>) {
    let mut paths: HashMap<String, PathBuf> = HashMap::new();
    let mut assets: Vec<FontAsset> = Vec::new();

    let mut push = |label: String, path: PathBuf, category: FontCategory| {
        if paths.contains_key(&label) {
            return;
        }
        paths.insert(label.clone(), path);
        assets.push(FontAsset {
            key: label,
            category,
        });
    };

    // 1. 追加パック同梱フォント。
    for path in crate::editing_addon::installed_fonts() {
        if let Some(label) = label_from_path(&path) {
            push(label, path, FontCategory::Pack);
        }
    }
    // 2. ユーザー追加フォント。
    for (label, path) in enumerate_dir_fonts(&user_fonts_dir()) {
        push(label, path, FontCategory::User);
    }
    // 3. システムフォント。
    for (label, path) in enumerate_system_fonts() {
        push(label, path, FontCategory::System);
    }

    (assets, paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_key_ignores_separators_and_case() {
        assert_eq!(font_lookup_key("Otomanopee One"), "otomanopeeone");
        assert_eq!(
            font_lookup_key("OtomanopeeOne-Regular"),
            "otomanopeeoneregular"
        );
        assert_eq!(font_lookup_key("M PLUS 1"), "mplus1");
    }

    #[test]
    fn candidate_matches_filename_label() {
        // pack ファイル名由来ラベル ↔ プリセットの font_candidate。
        assert!(font_name_matches_candidate(
            "OtomanopeeOne Regular",
            "Otomanopee One"
        ));
        assert!(font_name_matches_candidate("MPLUS1 wght", "M PLUS 1"));
        assert!(font_name_matches_candidate(
            "KaiseiTokumin ExtraBold",
            "Kaisei Tokumin ExtraBold"
        ));
        // 別フォントは一致しない。
        assert!(!font_name_matches_candidate("Yu Gothic", "Dela Gothic One"));
    }

    #[test]
    fn clean_name_strips_type_tag_and_alias() {
        assert_eq!(
            clean_font_name("Yu Gothic Medium (TrueType)"),
            "Yu Gothic Medium"
        );
        assert_eq!(
            clean_font_name("MS Gothic & MS PGothic & MS UI Gothic (TrueType)"),
            "MS Gothic"
        );
        assert_eq!(clean_font_name("Arial"), "Arial");
    }

    #[test]
    fn label_from_path_replaces_separators() {
        assert_eq!(
            label_from_path(Path::new("fonts/OtomanopeeOne-Regular.ttf")).as_deref(),
            Some("OtomanopeeOne Regular")
        );
        assert_eq!(
            label_from_path(Path::new("C:/x/Dela_Gothic_One.otf")).as_deref(),
            Some("Dela Gothic One")
        );
    }

    #[test]
    fn font_ext_detection() {
        assert!(is_font_ext("a.ttf"));
        assert!(is_font_ext("A.OTF"));
        assert!(is_font_ext("x.ttc"));
        assert!(!is_font_ext("a.txt"));
        assert!(!is_font_ext("noext"));
    }
}
