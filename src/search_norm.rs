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

/// ZIP 内エントリの fts_meta 上のキー表現 `<zip_path>\x1F<entry>`。
/// path_key 正規化済みの `zip_path` と元エントリパス (slash 統一・lowercase 済み) を受け取る。
///
/// セパレータは ASCII Unit Separator (U+001F)。Windows / POSIX のいずれでも通常ファイル名
/// 文字として許容されない (Windows は制御文字 0x00–0x1F をすべて禁止、POSIX でも
/// ユーザーは普通使わない) ため、`<zip>SEP<entry>` と通常パス `c:/a/book.zip!cover.jpg`
/// が衝突するあいまいさを構造的に排除できる (Codex P2 対応)。旧実装は `!` 区切りで、
/// ファイル名に `!` を含む Eagle 生成ファイル等と曖昧だった。INDEX_VERSION bump で
/// 旧データは自動再構築される。
pub const ZIP_ENTRY_SEP: char = '\u{1F}';

pub fn zip_entry_key(normalized_zip_path: &str, entry_name: &str) -> String {
    let entry_norm = entry_name.to_lowercase().replace('\\', "/");
    format!("{normalized_zip_path}{ZIP_ENTRY_SEP}{entry_norm}")
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
            format!("c:/photos/archive.zip{ZIP_ENTRY_SEP}folder/img.jpg")
        );
    }

    #[test]
    fn zip_entry_key_lowers_entry() {
        assert_eq!(
            zip_entry_key("c:/a.zip", "SubDir\\Img.JPG"),
            format!("c:/a.zip{ZIP_ENTRY_SEP}subdir/img.jpg")
        );
    }

    /// 新 separator (U+001F) は Windows / POSIX の通常ファイル名に出現し得ないので、
    /// 通常パス `c:/a/book.zip!cover.jpg` (ファイル名に `!` 含む) と衝突しない。
    #[test]
    fn zip_entry_key_is_not_ambiguous_with_bang_filename() {
        let zip_key = zip_entry_key("c:/a/book.zip", "cover.jpg");
        let bang_filename = "c:/a/book.zip!cover.jpg";
        assert_ne!(zip_key, bang_filename);
        assert!(zip_key.contains(ZIP_ENTRY_SEP));
        assert!(!bang_filename.contains(ZIP_ENTRY_SEP));
    }
}
