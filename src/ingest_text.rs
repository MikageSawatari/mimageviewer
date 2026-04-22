//! メタ抽出 → ソース別テキスト構築 (docs/search-expansion-design.md §19.5 Ingest Worker)。
//!
//! 画像 1 ファイルに対して以下のソースを個別にビルドする:
//!   - ファイル名 (拡張子を含む) → `SourceKind::Filename`
//!   - EXIF (カメラ / レンズ / 撮影日時 / GPS など) → `SourceKind::Exif`
//!   - XMP (X/Twitter 由来の mXD メタ) → `SourceKind::XmpTweet`
//!   - PNG tEXt/iTXt (A1111 / ComfyUI AI プロンプト) → `SourceKind::PngPrompt`
//!   - PDFium document info (PDF のみ) → `SourceKind::PdfMeta`
//!
//! ## 設計方針
//!
//! - **既存の `build_searchable_*` / `read_exif` / `read_tweet_info` を再利用**。抽出ロジックは重複実装しない。
//! - **空セクションは空文字列**: メタが無いフォーマット (BMP 等) でも panic しないよう `Option` で受ける。
//! - **正規化は各フィールドで独立に適用**: `search_norm::normalize_for_match` を各 String に 1 回ずつ。
//!   Tantivy bigram ingest / SQLite post-filter / クエリ側で同じ正規化関数を共有するルールを保つ。
//!
//! ## スコープ
//!
//! 本モジュールは **通常ファイル (FS 上の画像)** と PDF メタを対象にする。
//! ZIP 内エントリ (v1.x) は §7.7 の ZIP 専用 ingest コンテキストで別途扱う。

use std::path::Path;

use crate::fts_index::SourceKind;
use crate::search_norm::normalize_for_match;

/// 1 ファイル分のソース別検索テキスト (§19.5 + タグ統合)。各フィールドは `normalize_for_match` 適用済み
/// (tags は元の表記を保ったスペース区切り `#` 込み文字列を格納)。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PerSourceText {
    pub name: String,
    pub exif: String,
    pub xmp_tweet: String,
    pub png_prompt: String,
    pub pdf_meta: String,
    /// XMP `dc:subject` 由来のタグ列 (スペース区切り、`#` 込み / 既存タグは `#` なし)。
    /// Tantivy 側は bigram tokenize、fts_meta 側は同文字列を保存。
    pub tags: String,
}

impl PerSourceText {
    pub fn get(&self, source: SourceKind) -> &str {
        match source {
            SourceKind::Filename => &self.name,
            SourceKind::Exif => &self.exif,
            SourceKind::XmpTweet => &self.xmp_tweet,
            SourceKind::PngPrompt => &self.png_prompt,
            SourceKind::PdfMeta => &self.pdf_meta,
            SourceKind::Tags => &self.tags,
        }
    }

    /// 全ソース結合 (旧 `all_text` 互換、特に post-filter の "すべて" 検索で使う)。
    /// 区切りはスペース 1 個。既に個別正規化済みなので再適用はしない。
    pub fn combined(&self) -> String {
        let mut out = String::with_capacity(
            self.name.len()
                + self.exif.len()
                + self.xmp_tweet.len()
                + self.png_prompt.len()
                + self.pdf_meta.len()
                + self.tags.len()
                + 6,
        );
        for s in [
            &self.name,
            &self.exif,
            &self.xmp_tweet,
            &self.png_prompt,
            &self.pdf_meta,
            &self.tags,
        ] {
            if !s.is_empty() {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(s);
            }
        }
        out
    }

    pub fn is_empty_all(&self) -> bool {
        self.name.is_empty()
            && self.exif.is_empty()
            && self.xmp_tweet.is_empty()
            && self.png_prompt.is_empty()
            && self.pdf_meta.is_empty()
            && self.tags.is_empty()
    }
}

/// 1 タグ要素を「空白区切り列に載せられる」安全な形に正規化する。
/// 空白 / 制御文字は `_` に置換。索引側 (`build_tags_column`) と UI 側 (タグ定義ダイアログ)
/// の両方で呼んで、保存表記と検索時表記を一致させる唯一の経路にする。
pub fn canonicalize_tag_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_whitespace() || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// XMP `dc:subject` 由来のタグ列を fts_meta / Tantivy 向けのスペース区切り文字列にする。
/// 各要素は `canonicalize_tag_name` を通す。
pub fn build_tags_column(tags: &[String]) -> String {
    let mut out = String::new();
    for t in tags {
        let cleaned = canonicalize_tag_name(t);
        if cleaned.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&cleaned);
    }
    out
}

/// `build_tags_column` の逆: スペース区切りのタグ列 (fts_meta.tags_norm 形式) を
/// `Vec<String>` に戻す。prewarm_grid_tags / tag_write_worker で UI キャッシュに
/// 載せる時に使う。空白要素は捨てる。
pub fn parse_tags_column(s: &str) -> Vec<String> {
    s.split_whitespace().map(|t| t.to_string()).collect()
}

/// 1 ファイル分のソース別検索テキストをディスクから構築する。
/// 抽出に失敗した部分は空文字列。ファイル名は必ず含める。
pub fn build_per_source_for_file(path: &Path) -> PerSourceText {
    let mut out = PerSourceText::default();

    // 1. ファイル名
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        out.name = normalize_for_match(name);
    }

    // 2. EXIF
    if let Some(exif) = crate::exif_reader::read_exif(path, &[]) {
        let mut buf = String::with_capacity(128);
        append_exif(&mut buf, &exif);
        if !buf.trim().is_empty() {
            out.exif = normalize_for_match(&buf);
        }
    }

    // 3. XMP (mXD Twitter メタ)
    if let Some(xmp) = crate::xmp_reader::read_tweet_info(path) {
        let mut buf = String::with_capacity(128);
        append_xmp(&mut buf, &xmp);
        if !buf.trim().is_empty() {
            out.xmp_tweet = normalize_for_match(&buf);
        }
    }

    // 4. PNG AI プロンプト (tEXt/iTXt/zTXt)
    let png_text = crate::png_metadata::build_searchable_from_path(path);
    if !png_text.is_empty() {
        out.png_prompt = normalize_for_match(&png_text);
    }

    // 5. XMP dc:subject タグ (tag 機能)
    let dc_tags = crate::xmp_reader::read_dc_subject(path);
    out.tags = build_tags_column(&dc_tags);

    out
}

/// バイト列から構築 (ZIP 内エントリ用。v1.x で使用予定)。
pub fn build_per_source_from_bytes(display_name: &str, bytes: &[u8]) -> PerSourceText {
    let mut out = PerSourceText::default();
    out.name = normalize_for_match(display_name);

    if let Some(exif) = crate::exif_reader::read_exif_from_bytes(bytes, &[]) {
        let mut buf = String::with_capacity(128);
        append_exif(&mut buf, &exif);
        if !buf.trim().is_empty() {
            out.exif = normalize_for_match(&buf);
        }
    }
    if let Some(xmp) = crate::xmp_reader::read_tweet_info_from_bytes(bytes) {
        let mut buf = String::with_capacity(128);
        append_xmp(&mut buf, &xmp);
        if !buf.trim().is_empty() {
            out.xmp_tweet = normalize_for_match(&buf);
        }
    }
    let png_text = crate::png_metadata::build_searchable_from_bytes(bytes);
    if !png_text.is_empty() {
        out.png_prompt = normalize_for_match(&png_text);
    }
    let dc_tags = crate::xmp_reader::read_dc_subject_from_bytes(bytes);
    out.tags = build_tags_column(&dc_tags);

    out
}

/// PDF 用の構築。`name` は呼び出し側がパスから取り、`info_text` は PDFium document info の
/// 既正規化前テキスト (Title / Author / Subject / Keywords 連結) を渡す。
pub fn build_per_source_for_pdf(display_name: &str, info_text: &str) -> PerSourceText {
    PerSourceText {
        name: normalize_for_match(display_name),
        exif: String::new(),
        xmp_tweet: String::new(),
        png_prompt: String::new(),
        pdf_meta: if info_text.is_empty() {
            String::new()
        } else {
            normalize_for_match(info_text)
        },
        tags: String::new(),
    }
}

/// ファイル名のみ (ZIP など、メタを展開しないフォールバック) の最小版。
pub fn build_per_source_name_only(display_name: &str) -> PerSourceText {
    PerSourceText {
        name: normalize_for_match(display_name),
        ..PerSourceText::default()
    }
}

fn append_exif(out: &mut String, info: &crate::exif_reader::ExifInfo) {
    for (_group, tags) in &info.sections {
        for (_name, value) in tags {
            // タグ名自体は検索キーワードとしての価値が薄い (ユーザが "ExposureTime" と
            // 打つことは稀)。値だけを入れる。カメラ名・レンズ名・撮影地名など
            // 人間が検索しそうなのは value 側に入っている。
            if !value.is_empty() {
                out.push_str(value);
                out.push(' ');
            }
        }
    }
}

fn append_xmp(out: &mut String, info: &crate::xmp_reader::XmpTweetInfo) {
    // 検索で当たると価値のある情報を抜き出して連結。全部 Option<String> なので空なら飛ばす。
    let fields: [&Option<String>; 9] = [
        &info.tweet_id,
        &info.author_screen_name,
        &info.author_display_name,
        &info.posted_at,
        &info.description,
        &info.creator,
        &info.quoted_by_screen_name,
        &info.quoted_by_tweet_id,
        &info.source,
    ];
    for f in fields {
        if let Some(s) = f {
            if !s.is_empty() {
                out.push_str(s);
                out.push(' ');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn includes_filename() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("夕焼け_IMG_1234.jpg");
        fs::write(&path, b"not a real image").unwrap();
        let pst = build_per_source_for_file(&path);
        assert!(pst.name.contains("夕焼け_img_1234.jpg"), "name={}", pst.name);
        // 他フィールドは空
        assert!(pst.exif.is_empty());
        assert!(pst.xmp_tweet.is_empty());
        assert!(pst.png_prompt.is_empty());
    }

    #[test]
    fn name_is_lowercased() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("CAMERA_PHOTO.JPG");
        fs::write(&path, b"").unwrap();
        let pst = build_per_source_for_file(&path);
        assert!(!pst.name.contains("CAMERA"));
        assert!(pst.name.contains("camera_photo.jpg"));
    }

    #[test]
    fn non_image_file_returns_only_name() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("note.txt");
        fs::write(&path, b"plain text").unwrap();
        let pst = build_per_source_for_file(&path);
        assert_eq!(pst.name.trim(), "note.txt");
        assert!(pst.exif.is_empty());
        assert!(pst.xmp_tweet.is_empty());
        assert!(pst.png_prompt.is_empty());
    }

    #[test]
    fn empty_file_safe() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("empty.jpg");
        fs::write(&path, b"").unwrap();
        let pst = build_per_source_for_file(&path);
        assert!(pst.name.contains("empty.jpg"));
    }

    #[test]
    fn nonexistent_path_returns_only_name() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("ghost.jpg");
        let pst = build_per_source_for_file(&path);
        assert!(pst.name.contains("ghost.jpg"));
    }

    #[test]
    fn build_from_bytes_works() {
        let pst = build_per_source_from_bytes("photo.jpg", b"no real metadata");
        assert!(pst.name.contains("photo.jpg"));
    }

    #[test]
    fn pdf_helper_sets_pdf_meta_only() {
        let pst = build_per_source_for_pdf("book.pdf", "Title: Example / Author: Alice");
        assert!(pst.name.contains("book.pdf"));
        assert!(pst.pdf_meta.contains("example"));
        assert!(pst.pdf_meta.contains("alice"));
        assert!(pst.exif.is_empty());
        assert!(pst.xmp_tweet.is_empty());
        assert!(pst.png_prompt.is_empty());
    }

    #[test]
    fn combined_joins_non_empty_fields() {
        let pst = PerSourceText {
            name: "photo.jpg".into(),
            exif: "canon 5d".into(),
            xmp_tweet: "".into(),
            png_prompt: "prompt text".into(),
            pdf_meta: "".into(),
            tags: "#原神".into(),
        };
        let c = pst.combined();
        assert_eq!(c, "photo.jpg canon 5d prompt text #原神");
    }

    #[test]
    fn get_returns_correct_field() {
        let pst = PerSourceText {
            name: "n".into(),
            exif: "e".into(),
            xmp_tweet: "x".into(),
            png_prompt: "p".into(),
            pdf_meta: "m".into(),
            tags: "t".into(),
        };
        assert_eq!(pst.get(SourceKind::Filename), "n");
        assert_eq!(pst.get(SourceKind::Exif), "e");
        assert_eq!(pst.get(SourceKind::XmpTweet), "x");
        assert_eq!(pst.get(SourceKind::PngPrompt), "p");
        assert_eq!(pst.get(SourceKind::PdfMeta), "m");
        assert_eq!(pst.get(SourceKind::Tags), "t");
    }

    #[test]
    fn canonicalize_tag_name_replaces_whitespace_and_control() {
        // UI の tag_editor と索引の build_tags_column は同じ変換を通すことで、
        // タグピッカーが挿入した `#タグ名` が索引中の表記と完全一致する。
        assert_eq!(canonicalize_tag_name("foo bar"), "foo_bar");
        assert_eq!(canonicalize_tag_name("tab\there"), "tab_here");
        assert_eq!(canonicalize_tag_name("line\nbreak"), "line_break");
        assert_eq!(canonicalize_tag_name("plain"), "plain");
        assert_eq!(canonicalize_tag_name("日本語"), "日本語");
    }

    #[test]
    fn build_tags_column_uses_canonicalize() {
        let input = vec!["#原神".to_string(), "foo bar".to_string()];
        assert_eq!(build_tags_column(&input), "#原神 foo_bar");
    }
}
