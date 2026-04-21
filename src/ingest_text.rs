//! メタ抽出 → `all_text_norm` 構築 (docs/search-expansion-design.md §9.1 Ingest Worker)。
//!
//! 画像 1 ファイルに対して
//!   - ファイル名 (拡張子を含む)
//!   - EXIF (カメラ / レンズ / 撮影日時 / GPS など)
//!   - XMP (X/Twitter 由来の mXD メタ)
//!   - PNG tEXt/iTXt (A1111 / ComfyUI AI プロンプト)
//! を抽出し、`search_norm::normalize_for_match` を掛けた単一の `all_text_norm` 文字列を作る。
//!
//! ## 設計方針
//!
//! - **既存の `build_searchable_*` と `read_exif` / `read_tweet_info` を再利用**。
//!   抽出ロジックは重複実装しない。
//! - **空セクションを落とす**: メタが無いフォーマット (BMP 等) でも panic しないよう
//!   `Option` で受ける。
//! - **区切り文字は半角スペース 1 個**: bigram インデックスでは区切り位置の前後にまたがる
//!   bigram は実害なし (AND ヒットの合体文字列でも同じ挙動)。
//! - **最終段で `normalize_for_match`**: 小文字化は連結後に 1 回だけ。
//!
//! ## スコープ
//!
//! 本モジュールは **通常ファイル (FS 上の画像)** を対象にする。
//! ZIP 内エントリは §7.7 の ZIP 専用 ingest コンテキストで別途扱う (v1 後半)。

use std::path::Path;

use crate::search_norm::normalize_for_match;

/// XMP `dc:subject` から抽出したタグをスペース区切りで連結した文字列を返す。
/// タグが無い / 読み取り失敗なら空文字列。
///
/// - `#` で始まるタグ: mIV で付与されたタグ扱い
/// - `#` で始まらないタグ: 他ソフトで付与されたタグ (そのまま保存、検索では非使用)
///
/// 返す値はそのまま fts_meta.db の `tags` 列に UPSERT される。
/// 大文字小文字は保持 (検索時に正規化)。
pub fn extract_tags_for_file(path: &Path) -> String {
    let tags = crate::xmp_reader::read_dc_subject(path);
    build_tags_column(&tags)
}

/// バイト列版 (ZIP 内画像など)。
pub fn extract_tags_from_bytes(bytes: &[u8]) -> String {
    let tags = crate::xmp_reader::read_dc_subject_from_bytes(bytes);
    build_tags_column(&tags)
}

fn build_tags_column(tags: &[String]) -> String {
    // タグ内に空白や制御文字が混じると DB カラムの分割が崩れるので
    // 空白類は全部 `_` に置換する。タグ名の仕様で禁止予定なので実運用で
    // ほぼ発生しない防御ロジック。
    let mut out = String::new();
    for t in tags {
        let cleaned: String = t
            .chars()
            .map(|c| if c.is_whitespace() || c.is_control() { '_' } else { c })
            .collect();
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

/// 1 ファイルから検索対象テキストを作る。
/// 抽出に失敗した部分はスキップし、ファイル名は必ず含める (空文字列は返さない)。
pub fn build_all_text_for_file(path: &Path) -> String {
    let mut buf = String::with_capacity(256);

    // 1. ファイル名
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        buf.push_str(name);
        buf.push(' ');
    }

    // 2. EXIF (rexif)。hidden_tags は検索用途では空 (全部取る)。
    if let Some(exif) = crate::exif_reader::read_exif(path, &[]) {
        append_exif(&mut buf, &exif);
    }

    // 3. XMP (mXD Twitter メタ)
    if let Some(xmp) = crate::xmp_reader::read_tweet_info(path) {
        append_xmp(&mut buf, &xmp);
    }

    // 4. PNG AI プロンプト (tEXt/iTXt/zTXt)
    let png_text = crate::png_metadata::build_searchable_from_path(path);
    if !png_text.is_empty() {
        buf.push_str(&png_text);
        buf.push(' ');
    }

    // 5. 最終段で正規化 (§5.2 の設計: ingest / query / post-filter で唯一の正規化関数)
    normalize_for_match(&buf)
}

/// バイト列 (ZIP 内エントリ用。v1.x で使用予定)。
///
/// EXIF / XMP / PNG のバイト版 API を組み合わせる。ファイル名は呼び出し側から渡す。
pub fn build_all_text_from_bytes(display_name: &str, bytes: &[u8]) -> String {
    let mut buf = String::with_capacity(256);
    buf.push_str(display_name);
    buf.push(' ');

    if let Some(exif) = crate::exif_reader::read_exif_from_bytes(bytes, &[]) {
        append_exif(&mut buf, &exif);
    }
    if let Some(xmp) = crate::xmp_reader::read_tweet_info_from_bytes(bytes) {
        append_xmp(&mut buf, &xmp);
    }
    let png_text = crate::png_metadata::build_searchable_from_bytes(bytes);
    if !png_text.is_empty() {
        buf.push_str(&png_text);
        buf.push(' ');
    }

    normalize_for_match(&buf)
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
    // 検索で当たると価値のある情報を抜き出して連結。
    // 全部 Option<String> なので空なら飛ばす。
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
    // PNG tEXt 経路の E2E 動作は既存の `png_metadata::tests` でカバー済み。
    // ここでは ingest_text の連結・正規化ロジックのみを検証する。
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn includes_filename() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("夕焼け_IMG_1234.jpg");
        fs::write(&path, b"not a real image").unwrap();
        let text = build_all_text_for_file(&path);
        assert!(text.contains("夕焼け_img_1234.jpg"), "text was: {text}");
    }

    #[test]
    fn is_lowercased() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("CAMERA_PHOTO.JPG");
        fs::write(&path, b"").unwrap();
        let text = build_all_text_for_file(&path);
        // 大文字は残らない
        assert!(!text.contains("CAMERA"));
        assert!(text.contains("camera_photo.jpg"));
    }

    #[test]
    fn non_image_file_returns_only_name() {
        // EXIF / XMP / PNG 抽出が失敗しても panic しない。ファイル名は必ず返る。
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("note.txt");
        fs::write(&path, b"plain text").unwrap();
        let text = build_all_text_for_file(&path);
        assert!(text.trim() == "note.txt");
    }

    #[test]
    fn empty_file_safe() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("empty.jpg");
        fs::write(&path, b"").unwrap();
        let text = build_all_text_for_file(&path);
        assert!(text.contains("empty.jpg"));
    }

    #[test]
    fn nonexistent_path_returns_only_name() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("ghost.jpg");
        // 存在しないファイルでも、file_name は Path から取れるので name は入る
        let text = build_all_text_for_file(&path);
        assert!(text.contains("ghost.jpg"));
    }

    #[test]
    fn build_from_bytes_works() {
        let text = build_all_text_from_bytes("photo.jpg", b"no real metadata");
        // ファイル名は必ず入る
        assert!(text.contains("photo.jpg"));
    }
}
