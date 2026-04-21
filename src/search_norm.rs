//! 検索テキストの正規化関数 (docs/search-expansion-design.md §5.2)。
//!
//! **設計上の重要制約**:
//!   インデックス作成時 (ingest の `all_text_norm` 生成)、
//!   クエリパース時 (search_query)、
//!   post-filter 時 (matches の text 側) の **3 箇所で同じ関数を使う**。
//!   ズレると偽陰性が出る。
//!
//! v1 では `to_lowercase()` のみ。NFKC (全角/半角正規化) は v2 で検討。
//! NFKC 導入時は `index_version` を bump して再インデックス必須。
//!
//! パス正規化 (drive letter 保持 + `/` 区切り) は [`crate::search_index_db::normalize_path`] を利用。

/// インデックスとクエリの両方で使う、検索マッチ用のテキスト正規化。
/// v1: 小文字化のみ (現行 `search_query.rs` の `to_lowercase()` と整合)。
pub fn normalize_for_match(s: &str) -> String {
    s.to_lowercase()
}

/// ZIP 内エントリの fts_meta 上のキー表現 `<zip_path>!<entry>`。
/// path_key 正規化済みの `zip_path` と元エントリパス (slash 統一・lowercase 済み) を受け取る。
pub fn zip_entry_key(normalized_zip_path: &str, entry_name: &str) -> String {
    let entry_norm = entry_name.to_lowercase().replace('\\', "/");
    format!("{}!{}", normalized_zip_path, entry_norm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_lowers_ascii() {
        assert_eq!(normalize_for_match("ABCdef"), "abcdef");
    }

    #[test]
    fn normalize_preserves_cjk() {
        // CJK 漢字・ひらがな・カタカナは to_lowercase で変化しない
        assert_eq!(normalize_for_match("夕焼け"), "夕焼け");
        assert_eq!(normalize_for_match("カメラ"), "カメラ");
    }

    #[test]
    fn normalize_fullwidth_ascii_lowercases() {
        // to_lowercase は全角英字も小文字化する (Unicode case folding の動作)。
        // 実質的に fullwidth ⇄ halfwidth の混合は "同一 lowercase variant にはならない" ので、
        // 全角を含む検索は v2 で NFKC を入れるまで完全一致優先となる。
        assert_eq!(normalize_for_match("ＡＢＣ"), "ａｂｃ");
        // 半角小文字と全角小文字は別物
        assert_ne!(normalize_for_match("abc"), normalize_for_match("ＡＢＣ"));
    }

    #[test]
    fn normalize_is_idempotent() {
        let s = "Mixed カメラ 123";
        let once = normalize_for_match(s);
        let twice = normalize_for_match(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn zip_entry_key_combines() {
        assert_eq!(
            zip_entry_key("c:/photos/archive.zip", "folder/img.jpg"),
            "c:/photos/archive.zip!folder/img.jpg"
        );
    }

    #[test]
    fn zip_entry_key_lowers_entry() {
        assert_eq!(
            zip_entry_key("c:/a.zip", "SubDir\\Img.JPG"),
            "c:/a.zip!subdir/img.jpg"
        );
    }
}
