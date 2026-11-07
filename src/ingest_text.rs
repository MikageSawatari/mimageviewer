//! メタ抽出 → ソース別テキスト構築 (docs/archive/search-metadata/search-expansion-design.md §19.5 Ingest Worker)。
//!
//! 画像 / 動画 1 ファイルに対して以下のソースを個別にビルドする:
//!   - ファイル名 (拡張子を含む) → `SourceKind::Filename`
//!   - EXIF (カメラ / レンズ / 撮影日時 / GPS など) → `SourceKind::Exif`
//!   - XMP (X/Twitter 由来の mXD メタ) → `SourceKind::XmpTweet`
//!   - PNG tEXt/iTXt (A1111 / ComfyUI AI プロンプト) → `SourceKind::PngPrompt`
//!   - PDFium document info (PDF のみ) → `SourceKind::PdfMeta`
//!   - FFmpeg container metadata (動画のみ) → `SourceKind::VideoMeta`
//!   - 外部メタデータサイドカー (JSON/TXT) → `SourceKind::Sidecar`
//!   - ローカル AI 説明文 → `SourceKind::AiCaption` (IngestSession が DB を参照)
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
//! 本モジュールは **通常ファイル (FS 上の画像 / 動画)** と PDF メタを対象にする。
//! ZIP 内エントリ (v1.x) は §7.7 の ZIP 専用 ingest コンテキストで別途扱う。

use std::path::Path;

use crate::fts_index::SourceKind;
use crate::search_norm::normalize_for_match;

/// 1 ファイル分のソース別検索テキスト (§19.5 + タグ統合)。各フィールドは `normalize_for_match` 適用済み。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PerSourceText {
    pub name: String,
    pub exif: String,
    pub xmp_tweet: String,
    pub png_prompt: String,
    pub pdf_meta: String,
    pub video_meta: String,
    /// 旧 XMP `dc:subject` 由来のタグ列。通常 ingest では populate せず、既存データ移行時だけ参照する。
    pub tags: String,
    /// 外部メタデータサイドカー (画像と同名の JSON/TXT) の値テキスト (`normalize_for_match` 済み)。
    /// JSON はリーフ値のみ / TXT は全文。mIV タグ (`tags`) とは別系統の読み取り専用
    /// フリーテキスト (docs/sidecar-metadata-ingest.md)。
    pub sidecar: String,
    /// Local AI caption, visible text and keywords (only successful rows).
    pub ai_caption: String,
}

impl PerSourceText {
    pub fn get(&self, source: SourceKind) -> &str {
        match source {
            SourceKind::Filename => &self.name,
            SourceKind::Exif => &self.exif,
            SourceKind::XmpTweet => &self.xmp_tweet,
            SourceKind::PngPrompt => &self.png_prompt,
            SourceKind::PdfMeta => &self.pdf_meta,
            SourceKind::VideoMeta => &self.video_meta,
            SourceKind::Tags => &self.tags,
            SourceKind::Sidecar => &self.sidecar,
            SourceKind::AiCaption => &self.ai_caption,
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
                + self.video_meta.len()
                + self.tags.len()
                + self.sidecar.len()
                + self.ai_caption.len()
                + 9,
        );
        for s in [
            &self.name,
            &self.exif,
            &self.xmp_tweet,
            &self.png_prompt,
            &self.pdf_meta,
            &self.video_meta,
            &self.tags,
            &self.sidecar,
            &self.ai_caption,
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
            && self.video_meta.is_empty()
            && self.tags.is_empty()
            && self.sidecar.is_empty()
            && self.ai_caption.is_empty()
    }

    /// Tantivy の STORED 原文へ投入する UTF-8 byte 数。
    ///
    /// `combined()` は検索互換用の一時文字列なので含めず、実際に個別保存する
    /// 9 source field の総量だけを数える。
    pub(crate) fn stored_text_byte_sizes(&self) -> (usize, usize) {
        let png_prompt_bytes = self.png_prompt.len();
        let total_bytes = self.name.len()
            + self.exif.len()
            + self.xmp_tweet.len()
            + png_prompt_bytes
            + self.pdf_meta.len()
            + self.video_meta.len()
            + self.tags.len()
            + self.sidecar.len()
            + self.ai_caption.len();
        (total_bytes, png_prompt_bytes)
    }
}

/// The same normalized successful caption text is used by ingest and Ctrl+F.
/// Stale signatures deliberately remain searchable; only status controls inclusion.
pub fn caption_search_text(row: &crate::ai_caption_db::CaptionRow) -> String {
    if row.status != crate::ai_caption_db::CaptionStatus::Ok {
        return String::new();
    }
    let mut parts = vec![row.caption.as_str(), row.visible_text.as_str()];
    parts.extend(row.keywords.iter().map(String::as_str));
    parts.retain(|part| !part.is_empty());
    normalize_for_match(&parts.join(" "))
}

/// 旧 XMP タグ列を「空白区切り列に載せられる」安全な形に正規化する。
/// 通常 ingest では使わず、既存 STORED tags など移行専用の互換 helper として残す。
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

/// XMP `dc:subject` 由来の旧タグ列をスペース区切り文字列にする。
/// 各要素は `canonicalize_tag_name` を通す。通常 ingest はこのフィールドを populate しない。
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

/// `build_tags_column` の逆: スペース区切りの旧タグ列を `Vec<String>` に戻す。
/// 空白要素は捨てる。
pub fn parse_tags_column(s: &str) -> Vec<String> {
    s.split_whitespace().map(|t| t.to_string()).collect()
}

/// ファイル名だけを検索対象にする種類向けのソース別テキストを構築する。
///
/// 音声ファイルは ID3 等の埋め込みタグを今回の索引スコープに含めないため、ファイルを
/// 読まずにこの helper を使う。正規化規則は他の ingest 経路と同じ。
pub fn build_per_source_for_filename(path: &Path) -> PerSourceText {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(normalize_for_match)
        .unwrap_or_default();
    PerSourceText {
        name,
        ..Default::default()
    }
}

/// 1 ファイル分のソース別検索テキストをディスクから構築する。
/// 抽出に失敗した部分は空文字列。ファイル名は必ず含める。
pub fn build_per_source_for_file(path: &Path) -> PerSourceText {
    build_per_source_for_file_counted(path).0
}

/// 同じ抽出と、メタデータ読み取り API が返す実読込バイト数の合計。
/// XMP / AI / EXIF / 先頭 2MB / 外部サイドカーを含む。seek で飛ばした範囲、
/// ファイル属性、説明文 DB、FFmpeg の動画メタデータ I/O は含まない。
pub fn build_per_source_for_file_counted(path: &Path) -> (PerSourceText, u64) {
    let mut out = build_per_source_for_filename(path);
    let mut bytes_read = 0;

    // 2. JPEG / PNG は Ctrl+F pass 2 と同じ path 版で見出しと必要なメタだけ読む。
    //    TIFF / MP4 等は既存の先頭 2MB を bytes 版で共有する経路を維持する。
    let is_video_sidecar = crate::xmp_writer::is_video_for_sidecar(path);
    let mut ai_from_exif_user_comment = false;

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let (xmp, ai_text, ai_origin) =
        if matches!(ext.as_deref(), Some("jpg" | "jpeg" | "jfif" | "png")) {
            let xmp = crate::xmp_reader::read_tweet_info_counted(path);
            let ai = crate::png_metadata::build_searchable_from_path_counted(path);
            bytes_read += xmp.bytes_read + ai.bytes_read;
            (xmp.info, ai.text, ai.origin)
        } else {
            let (bytes, read) = read_metadata_bytes(path);
            bytes_read += read;
            if let Some(bytes) = bytes {
                let xmp = crate::xmp_reader::read_tweet_info_from_bytes(&bytes);
                let (text, origin) =
                    crate::png_metadata::build_searchable_from_bytes_with_origin(&bytes);
                (xmp, text, origin)
            } else {
                (None, String::new(), None)
            }
        };
    // 2a. XMP (mXD Twitter メタ)
    if let Some(xmp) = xmp {
        let mut buf = String::with_capacity(128);
        append_xmp(&mut buf, &xmp);
        if !buf.trim().is_empty() {
            out.xmp_tweet = normalize_for_match(&buf);
        }
    }
    // 2b. AI プロンプト (PNG tEXt/iTXt/zTXt / EXIF UserComment)
    if !ai_text.is_empty() {
        out.png_prompt = normalize_for_match(&ai_text);
    }
    if ai_origin.is_some_and(|o| o.suppresses_exif_user_comment()) {
        ai_from_exif_user_comment = true;
    }

    // 3. EXIF (JPEG は SOS 手前まで。TIFF は従来どおり全体読み)
    //    UserComment が AI メタとして認識された場合は png_prompt 側に正規化済みで載るため、
    //    EXIF 側には再掲しない (Negative prompt の二重取り込み防止)。
    let exif = crate::exif_reader::read_exif_counted(path, &[]);
    bytes_read += exif.bytes_read;
    if let Some(exif) = exif.info {
        let mut buf = String::with_capacity(128);
        append_exif(&mut buf, &exif, ai_from_exif_user_comment);
        if !buf.trim().is_empty() {
            out.exif = normalize_for_match(&buf);
        }
    }

    // 4. 動画ファイルは同名 `.xmp` サイドカーではなくコンテナメタだけを検索対象にする。
    if is_video_sidecar {
        out.video_meta = build_video_metadata_text(path);
    }

    // 5. 外部メタデータサイドカー (画像のみ。動画は既存 .xmp 経路を維持する)。
    //    JSON はリーフ値のみ / TXT は全文を sidecar_text へ。タグ抽出はしない
    //    (mIV タグ系統とは別系統。docs/sidecar-metadata-ingest.md)。
    if !is_video_sidecar {
        let sidecar = crate::external_metadata::read_search_text_counted(path);
        bytes_read += sidecar.bytes_read;
        if let Some(text) = sidecar.text {
            out.sidecar = normalize_for_match(&text);
        }
    }

    (out, bytes_read)
}

/// 動画コンテナの埋め込みメタデータを検索用テキストにする。
///
/// 再生時の `VideoInfo` と同じ意味合いの代表値 (title / artist / URL /
/// description / chapter title) を拾う。失敗時は空文字列に倒し、ファイル名や
/// XMP サイドカータグの検索は継続できるようにする。
pub fn build_video_metadata_text(path: &Path) -> String {
    if !crate::xmp_writer::is_video_for_sidecar(path) {
        return String::new();
    }
    let Some(raw) = read_video_metadata_raw(path) else {
        return String::new();
    };
    normalize_for_match(&raw)
}

fn read_video_metadata_raw(path: &Path) -> Option<String> {
    use ffmpeg_the_third as ffmpeg;

    ffmpeg::init().ok()?;
    // T52 (Codex P2 / 2026-05-16): network mount や stall ファイルに引きずられて
    // ingester worker thread が永久ブロックするのを防ぐため、open に 10 秒 deadline
    // の interrupt callback を付ける。動画プレイヤー経路 (avio_progress.rs) と同じ
    // パターン (= deadline 超過で callback が `true` を返し avformat_open_input が
    // abort される)。10s は最初の packet 読み取りまで含む。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let input =
        ffmpeg::format::input_with_interrupt(path, move || std::time::Instant::now() >= deadline)
            .ok()?;
    let mut parts: Vec<String> = Vec::new();
    push_metadata_value(&mut parts, &input, &["title", "TITLE"]);
    push_metadata_value(&mut parts, &input, &["artist", "ARTIST", "author"]);
    push_metadata_value(
        &mut parts,
        &input,
        &[
            "purl",
            "PURL",
            "url",
            "URL",
            "webpage_url",
            "WEBPAGE_URL",
            "source_url",
            "SOURCE_URL",
            "original_url",
            "ORIGINAL_URL",
        ],
    );
    push_metadata_value(
        &mut parts,
        &input,
        &["description", "DESCRIPTION", "comment", "COMMENT"],
    );

    for chapter in input.chapters() {
        let md = chapter.metadata();
        if let Some(title) = md
            .get("title")
            .or_else(|| md.get("TITLE"))
            .filter(|s| !s.trim().is_empty())
        {
            parts.push(title.to_string());
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn push_metadata_value(
    out: &mut Vec<String>,
    input: &ffmpeg_the_third::format::context::Input,
    keys: &[&str],
) {
    let dict = input.metadata();
    for k in keys {
        if let Some(v) = dict.get(k).filter(|s| !s.trim().is_empty()) {
            out.push(v.to_string());
            return;
        }
    }
    for (key, value) in dict.iter() {
        if keys
            .iter()
            .any(|candidate| key.eq_ignore_ascii_case(candidate))
            && !value.trim().is_empty()
        {
            out.push(value.to_string());
            return;
        }
    }
}

/// TIFF / MP4 / MOV / M4V の先頭 2MB を bytes 版メタ抽出へ渡す。
///
/// JPEG / PNG は path 版の部分読みを使うためここでは読まない。
/// WebP 等の他拡張子も従来どおり読まない。
///
/// 戻り値の `Vec<u8>` は同一ファイルに対して XMP / PNG パーサーで共有される。
///
/// ## TIFF/MP4 の上限について (Codex P3 指摘対応)
///
/// 旧タグ検索経路では TIFF/MP4 でもフルファイル読みが起き得たため、大容量コンテナは
/// ここで上限を設けて通常操作を巻き込まないようにする。
///
/// 入力上限 2MB は既存契約として保つ。現行の共有 XMP bytes reader は fallback を
/// 512KB に制限するため、2MB 全域の XMP を抽出できるという保証ではない。
fn read_metadata_bytes(path: &Path) -> (Option<Vec<u8>>, u64) {
    const METADATA_SCAN_LIMIT: u64 = 2 * 1024 * 1024;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        // 大容量コンテナ: 先頭 2MB だけ読む (上記コメント参照)
        Some("tif" | "tiff" | "mp4" | "mov" | "m4v") => {
            use std::io::Read;
            let Ok(f) = std::fs::File::open(path) else {
                return (None, 0);
            };
            let mut buf = Vec::with_capacity(64 * 1024);
            let result = f.take(METADATA_SCAN_LIMIT).read_to_end(&mut buf);
            let bytes_read = buf.len() as u64;
            (result.ok().map(|_| buf), bytes_read)
        }
        // BMP / GIF / WebP / HEIC / RAW: XMP/PNG チャンクを持たない想定なので読まない
        _ => (None, 0),
    }
}

/// バイト列から構築 (ZIP 内エントリ用。v1.x で使用予定)。
pub fn build_per_source_from_bytes(display_name: &str, bytes: &[u8]) -> PerSourceText {
    let mut out = PerSourceText::default();
    out.name = normalize_for_match(display_name);

    // PNG メタ (tEXt/iTXt/zTXt) は inflate + 解析が重いので **1 回だけ**実行し、EXIF
    // UserComment 抑制判定 (origin) と png_prompt の両方で共有する。旧実装は exif 経路で
    // `_with_origin`、png_text 経路で `build_searchable_from_bytes` を別々に呼び、ZIP 内
    // 画像のチャンクを 2 回 inflate + 解析していた (ZIP ingest ホットパスの perf。path 版
    // `build_per_source_for_file` と同じく統合)。
    let (png_text, ai_origin) = crate::png_metadata::build_searchable_from_bytes_with_origin(bytes);

    if let Some(exif) = crate::exif_reader::read_exif_from_bytes(bytes, &[]) {
        let mut buf = String::with_capacity(128);
        append_exif(
            &mut buf,
            &exif,
            ai_origin.is_some_and(|o| o.suppresses_exif_user_comment()),
        );
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
    if !png_text.is_empty() {
        out.png_prompt = normalize_for_match(&png_text);
    }
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
        video_meta: String::new(),
        tags: String::new(),
        sidecar: String::new(),
        ai_caption: String::new(),
    }
}

fn append_exif(out: &mut String, info: &crate::exif_reader::ExifInfo, skip_user_comment: bool) {
    for (_group, tags) in &info.sections {
        for (name, value) in tags {
            if skip_user_comment && name == "UserComment" {
                continue;
            }
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

// Frozen pre-partial-read policy; keep independent of the production reader.
#[cfg(test)]
fn read_metadata_bytes_legacy(path: &Path) -> Option<Vec<u8>> {
    const METADATA_SCAN_LIMIT: u64 = 2 * 1024 * 1024;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)?;
    match ext.as_str() {
        // 小画像: 全読み (末尾に XMP セグメントが置かれるケースを拾うため)
        "jpg" | "jpeg" | "jfif" | "png" => std::fs::read(path).ok(),
        // 大容量コンテナ: 先頭 2MB だけ読む (上記コメント参照)
        "tif" | "tiff" | "mp4" | "mov" | "m4v" => {
            use std::io::Read;
            let f = std::fs::File::open(path).ok()?;
            let mut buf = Vec::with_capacity(64 * 1024);
            f.take(METADATA_SCAN_LIMIT).read_to_end(&mut buf).ok()?;
            Some(buf)
        }
        // BMP / GIF / WebP / HEIC / RAW: XMP/PNG チャンクを持たない想定なので読まない
        _ => None,
    }
}

#[cfg(test)]
fn build_per_source_for_file_legacy(path: &Path) -> PerSourceText {
    let mut out = build_per_source_for_filename(path);

    // 2. XMP / PNG / EXIF UserComment AI メタは同じファイル実体を
    //    複数回読む path-based 版を使うと
    //    AI 生成 PNG (10-30MB) で 3x 倍のディスク読み取りになり、インデクサが
    //    350MB/秒級でディスクを占有してしまう (Codex perf 報告)。
    //    ここで一度だけ読んで bytes 版に渡すことで I/O を 3 分の 1 に減らす。
    let is_video_sidecar = crate::xmp_writer::is_video_for_sidecar(path);
    let mut ai_from_exif_user_comment = false;

    if let Some(bytes) = read_metadata_bytes_legacy(path) {
        // 2a. XMP (mXD Twitter メタ)
        if let Some(xmp) = crate::xmp_reader::read_tweet_info_from_bytes(&bytes) {
            let mut buf = String::with_capacity(128);
            append_xmp(&mut buf, &xmp);
            if !buf.trim().is_empty() {
                out.xmp_tweet = normalize_for_match(&buf);
            }
        }
        // 2b. AI プロンプト (PNG tEXt/iTXt/zTXt / EXIF UserComment)
        let (ai_text, ai_origin) =
            crate::png_metadata::build_searchable_from_bytes_with_origin(&bytes);
        if !ai_text.is_empty() {
            out.png_prompt = normalize_for_match(&ai_text);
        }
        if ai_origin.is_some_and(|o| o.suppresses_exif_user_comment()) {
            ai_from_exif_user_comment = true;
        }
    }

    // 3. EXIF (rexif が自前で開く。内部パーサーが IFD を読むだけなので全読みしない想定)
    //    UserComment が AI メタとして認識された場合は png_prompt 側に正規化済みで載るため、
    //    EXIF 側には再掲しない (Negative prompt の二重取り込み防止)。
    if let Some(exif) = crate::exif_reader::read_exif(path, &[]) {
        let mut buf = String::with_capacity(128);
        append_exif(&mut buf, &exif, ai_from_exif_user_comment);
        if !buf.trim().is_empty() {
            out.exif = normalize_for_match(&buf);
        }
    }

    // 4. 動画ファイルは同名 `.xmp` サイドカーではなくコンテナメタだけを検索対象にする。
    if is_video_sidecar {
        out.video_meta = build_video_metadata_text(path);
    }

    // 5. 外部メタデータサイドカー (画像のみ。動画は既存 .xmp 経路を維持する)。
    //    JSON はリーフ値のみ / TXT は全文を sidecar_text へ。タグ抽出はしない
    //    (mIV タグ系統とは別系統。docs/sidecar-metadata-ingest.md)。
    if !is_video_sidecar {
        if let Some(text) = crate::external_metadata::read_search_text(path) {
            out.sidecar = normalize_for_match(&text);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn fixture(name: &str) -> Vec<u8> {
        fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/ai_metadata")
                .join(name),
        )
        .unwrap()
    }

    fn tiff_metadata(comment: Option<&str>) -> Vec<u8> {
        // Little-endian TIFF: IFD0 Make + ExifIFD, then UserComment (ASCII preamble).
        let mut tiff = b"II\x2a\0\x08\0\0\0".to_vec();
        tiff.extend_from_slice(&2_u16.to_le_bytes());
        for (tag, kind, count, offset) in [(0x010f_u16, 2_u16, 6_u32, 56_u32), (0x8769, 4, 1, 38)] {
            tiff.extend_from_slice(&tag.to_le_bytes());
            tiff.extend_from_slice(&kind.to_le_bytes());
            tiff.extend_from_slice(&count.to_le_bytes());
            tiff.extend_from_slice(&offset.to_le_bytes());
        }
        tiff.extend_from_slice(&0_u32.to_le_bytes());
        let user = [
            b"ASCII\0\0\0".as_slice(),
            comment.unwrap_or("ordinary comment").as_bytes(),
        ]
        .concat();
        tiff.extend_from_slice(&1_u16.to_le_bytes());
        tiff.extend_from_slice(&0x9286_u16.to_le_bytes());
        tiff.extend_from_slice(&7_u16.to_le_bytes());
        tiff.extend_from_slice(&(user.len() as u32).to_le_bytes());
        tiff.extend_from_slice(&62_u32.to_le_bytes());
        tiff.extend_from_slice(&0_u32.to_le_bytes());
        tiff.extend_from_slice(b"Canon\0");
        tiff.extend_from_slice(&user);
        tiff
    }

    fn jpeg_metadata(comment: Option<&str>, xmp: Option<&[u8]>, image_size: usize) -> Vec<u8> {
        fn app1(out: &mut Vec<u8>, data: &[u8]) {
            out.extend_from_slice(&[0xff, 0xe1]);
            out.extend_from_slice(&u16::try_from(data.len() + 2).unwrap().to_be_bytes());
            out.extend_from_slice(data);
        }
        let tiff = tiff_metadata(comment);
        let mut out = vec![0xff, 0xd8];
        app1(&mut out, &[b"Exif\0\0".as_slice(), &tiff].concat());
        if let Some(xmp) = xmp {
            app1(
                &mut out,
                &[b"http://ns.adobe.com/xap/1.0/\0".as_slice(), xmp].concat(),
            );
        }
        out.extend_from_slice(&[0xff, 0xda, 0, 2]);
        out.resize(out.len() + image_size, 0x55);
        out.extend_from_slice(&[0xff, 0xd9]);
        out
    }

    const XMP: &[u8] = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:xtw="https://mXDownloader.app/ns/x-twitter/1.0/" xtw:TweetId="123" xtw:AuthorScreenName="PartialReader"/></rdf:RDF></x:xmpmeta>"#;

    fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&[0; 4]); // Metadata readers do not validate CRC or decode IDAT.
    }

    fn large_png(ai: bool, xmp: bool) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        png_chunk(&mut out, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]);
        if ai {
            png_chunk(&mut out, b"tEXt", b"parameters\0positive lighthouse\nNegative prompt: negative-secret\nSteps: 20, Sampler: Euler, CFG scale: 7, Seed: 42");
        }
        png_chunk(&mut out, b"IDAT", &vec![0x55; 8 * 1024 * 1024]);
        if xmp {
            png_chunk(
                &mut out,
                b"iTXt",
                &[b"XML:com.adobe.xmp\0\0\0\0\0".as_slice(), XMP].concat(),
            );
        }
        png_chunk(&mut out, b"IEND", &[]);
        out
    }

    fn assert_legacy_matches(path: &Path, bytes: &[u8]) -> PerSourceText {
        fs::write(path, bytes).unwrap();
        let (actual, _) = build_per_source_for_file_counted(path);
        assert_eq!(
            actual,
            build_per_source_for_file_legacy(path),
            "{}",
            path.display()
        );
        assert_eq!(actual, build_per_source_for_file(path));
        actual
    }

    #[test]
    fn partial_ingest_matches_legacy_for_jpeg_png_fixtures() {
        fn visit(root: &Path, tmp: &Path, checked: &mut usize) {
            for entry in fs::read_dir(root).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(&path, tmp, checked);
                } else if matches!(
                    path.extension().and_then(|x| x.to_str()),
                    Some("jpg" | "jpeg" | "jfif" | "png")
                ) {
                    let target = tmp.join(path.file_name().unwrap());
                    assert_legacy_matches(&target, &fs::read(&path).unwrap());
                    *checked += 1;
                }
            }
        }
        let tmp = TempDir::new().unwrap();
        let mut checked = 0;
        for root in ["ai_metadata", "sd_parsers_upstream"] {
            visit(
                &Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures")
                    .join(root),
                tmp.path(),
                &mut checked,
            );
        }
        assert!(
            checked > 40,
            "must compare real fixtures, including text/iTXt/zTXt and bad images"
        );
    }

    #[test]
    fn partial_ingest_matches_legacy_for_exif_xmp_and_mismatched_extensions() {
        let tmp = TempDir::new().unwrap();
        let ai = "positive lighthouse\nNegative prompt: negative-secret\nSteps: 20, Sampler: Euler, CFG scale: 7, Seed: 42";
        for ext in ["jpg", "jpeg", "jfif", "png"] {
            let path = tmp.path().join(format!("photo.{ext}"));
            let exif = assert_legacy_matches(&path, &jpeg_metadata(None, None, 16));
            assert!(exif.exif.contains("canon"));
            let text = assert_legacy_matches(&path, &jpeg_metadata(Some(ai), None, 16));
            assert!(text.png_prompt.contains("positive lighthouse"));
            assert!(!text.combined().contains("negative-secret"));
            let text = assert_legacy_matches(
                &path,
                &jpeg_metadata(Some(r#"{"unknown":"json-secret"}"#), None, 16),
            );
            assert!(text.png_prompt.is_empty());
            assert!(!text.exif.contains("json-secret"));
            let text = assert_legacy_matches(&path, &jpeg_metadata(None, Some(XMP), 16));
            assert!(text.xmp_tweet.contains("partialreader"));
            let text = assert_legacy_matches(&path, &jpeg_metadata(Some(ai), Some(XMP), 16));
            assert!(text.xmp_tweet.contains("123"));
            assert!(text.png_prompt.contains("positive lighthouse"));
            let text = assert_legacy_matches(&path, &fixture("enc_a1111_itxt.png"));
            assert!(!text.png_prompt.is_empty());
            assert_legacy_matches(&path, &[]);
            assert_legacy_matches(&path, &[0xff, 0xd8, 0xff]);
            assert_legacy_matches(&path, b"\x89PNG\r\n\x1a\n\0\0");
        }
        let mut xmp_only = vec![0xff, 0xd8];
        let data = [b"http://ns.adobe.com/xap/1.0/\0".as_slice(), XMP].concat();
        xmp_only.extend_from_slice(&[0xff, 0xe1]);
        xmp_only.extend_from_slice(&((data.len() + 2) as u16).to_be_bytes());
        xmp_only.extend_from_slice(&data);
        xmp_only.extend_from_slice(&[0xff, 0xda, 0, 2, 0xff, 0xd9]);
        let text = assert_legacy_matches(&tmp.path().join("xmp.jpg"), &xmp_only);
        assert!(!text.xmp_tweet.is_empty());
        assert!(text.exif.is_empty());
        let text = assert_legacy_matches(&tmp.path().join("xmp.png"), &large_png(false, true));
        assert!(text.xmp_tweet.contains("partialreader"));
    }

    #[test]
    fn partial_ingest_matches_legacy_for_tiff_magic_jpg_and_late_tiff_comment() {
        let tmp = TempDir::new().unwrap();
        let ai = "positive lighthouse\nNegative prompt: negative-secret\nSteps: 20, Sampler: Euler, CFG scale: 7, Seed: 42";
        let mut bytes = tiff_metadata(Some(ai));
        assert!(bytes.starts_with(b"II*\0"));
        let jpg = tmp.path().join("tiff-content.jpg");
        let text = assert_legacy_matches(&jpg, &bytes);
        assert!(text.exif.contains("canon"));
        assert!(text.png_prompt.contains("positive lighthouse"));
        assert!(!text.combined().contains("negative-secret"));

        // Move UserComment beyond the old TIFF prefix limit. Its IFD entry's
        // value-offset field is at 48, and its original payload starts at 62.
        let comment = bytes.split_off(62);
        let offset = 2 * 1024 * 1024 + 128;
        bytes[48..52].copy_from_slice(&(offset as u32).to_le_bytes());
        bytes.resize(offset, 0);
        bytes.extend_from_slice(&comment);
        let text = assert_legacy_matches(&jpg, &bytes);
        assert!(text.png_prompt.contains("positive lighthouse"));
        assert!(!text.combined().contains("negative-secret"));

        for ext in ["tif", "tiff"] {
            let path = tmp.path().join(format!("late-comment.{ext}"));
            let text = assert_legacy_matches(&path, &bytes);
            assert!(
                text.png_prompt.is_empty(),
                "AI bytes reader must retain the 2MB limit"
            );
            assert!(
                text.exif.contains("negative-secret"),
                "the independent full EXIF read remains unsuppressed"
            );
        }
    }

    #[test]
    fn legacy_metadata_reader_preserves_extension_gates_and_two_mb_limit() {
        let tmp = TempDir::new().unwrap();
        let bytes = vec![0x55; 2 * 1024 * 1024 + 128];
        for ext in [
            "jpg", "jpeg", "jfif", "png", "JPG", "tif", "tiff", "mp4", "mov", "m4v", "TIFF",
            "webp", "bmp", "gif", "heic", "raw", "txt", "",
        ] {
            let path = tmp.path().join(format!("image.{ext}"));
            fs::write(&path, &bytes).unwrap();
            let expected = match ext {
                "jpg" | "jpeg" | "jfif" | "png" | "JPG" => Some(bytes.as_slice()),
                "tif" | "tiff" | "mp4" | "mov" | "m4v" | "TIFF" => Some(&bytes[..2 * 1024 * 1024]),
                _ => None,
            };
            assert_eq!(
                read_metadata_bytes_legacy(&path).as_deref(),
                expected,
                "{ext}"
            );
        }
    }

    #[test]
    fn partial_ingest_large_images_skip_pixel_payload_and_count_sidecar() {
        let tmp = TempDir::new().unwrap();
        for (name, bytes) in [
            ("large.jpg", jpeg_metadata(None, None, 8 * 1024 * 1024)),
            (
                "plain.jpg",
                [
                    vec![0xff, 0xd8, 0xff, 0xda, 0, 2],
                    vec![0x55; 8 * 1024 * 1024],
                    vec![0xff, 0xd9],
                ]
                .concat(),
            ),
            ("large.png", large_png(true, false)),
            ("plain.png", large_png(false, false)),
        ] {
            let path = tmp.path().join(name);
            assert_legacy_matches(&path, &bytes);
            let (without, read) = build_per_source_for_file_counted(&path);
            assert!(
                read > 0 && read < 64 * 1024,
                "{name}: {read} of {}",
                bytes.len()
            );
            let sidecar = b"sidecar counted text";
            fs::write(tmp.path().join(format!("{name}.txt")), sidecar).unwrap();
            let (with, total) = build_per_source_for_file_counted(&path);
            assert_eq!(total, read + sidecar.len() as u64);
            assert!(without.sidecar.is_empty());
            assert_eq!(with.sidecar, "sidecar counted text");
            assert_eq!(with, build_per_source_for_file_legacy(&path));
        }
    }

    #[test]
    fn partial_ingest_keeps_two_mb_prefix_and_skips_other_formats() {
        let tmp = TempDir::new().unwrap();
        for ext in ["tif", "tiff", "mp4", "mov", "m4v"] {
            let path = tmp.path().join(format!("late.{ext}"));
            let mut bytes = if matches!(ext, "tif" | "tiff") {
                b"II*\0".to_vec()
            } else {
                b"\0\0\0\x18ftypisom".to_vec()
            };
            bytes.resize(768 * 1024, 0);
            bytes.extend_from_slice(XMP);
            bytes.resize(3 * 1024 * 1024, 0);
            fs::write(&path, bytes).unwrap();
            let (prefix, read) = read_metadata_bytes(&path);
            assert_eq!(read, 2 * 1024 * 1024);
            let prefix = prefix.unwrap();
            assert_eq!(&prefix[768 * 1024..768 * 1024 + XMP.len()], XMP);
            // The shared XMP bytes reader currently also bounds fallback to 512KB.
            // Keep the existing 2MB input contract without expanding that reader here.
            assert!(crate::xmp_reader::read_tweet_info_from_bytes(&prefix).is_none());
            // TIFF EXIF retains its independent full-read behavior. Avoid FFmpeg in this test.
            if matches!(ext, "tif" | "tiff") {
                assert_eq!(
                    build_per_source_for_file(&path),
                    build_per_source_for_file_legacy(&path)
                );
            }
        }
        for ext in ["webp", "bmp", "gif", "heic", "raw", "jpg", "png"] {
            assert_eq!(
                read_metadata_bytes(&tmp.path().join(format!("skip.{ext}"))),
                (None, 0)
            );
        }
    }

    #[test]
    fn partial_ingest_xmp_without_tweet_id_does_not_append_fields() {
        let tmp = TempDir::new().unwrap();
        let xmp = String::from_utf8(XMP.to_vec())
            .unwrap()
            .replace("xtw:TweetId=\"123\"", "");
        let path = tmp.path().join("unpopulated.jpg");
        let text = assert_legacy_matches(&path, &jpeg_metadata(None, Some(xmp.as_bytes()), 16));
        assert!(text.xmp_tweet.is_empty());
        assert!(
            crate::xmp_reader::read_tweet_info_counted(&path)
                .info
                .is_none()
        );
    }

    #[test]
    fn includes_filename() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("夕焼け_IMG_1234.jpg");
        fs::write(&path, b"not a real image").unwrap();
        let pst = build_per_source_for_file(&path);
        assert!(
            pst.name.contains("夕焼け_img_1234.jpg"),
            "name={}",
            pst.name
        );
        // 他フィールドは空
        assert!(pst.exif.is_empty());
        assert!(pst.xmp_tweet.is_empty());
        assert!(pst.png_prompt.is_empty());
        assert!(pst.video_meta.is_empty());
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
    fn filename_only_helper_does_not_populate_metadata_sources() {
        let pst = build_per_source_for_filename(Path::new("C:/Music/MixedCase.MP3"));
        assert_eq!(pst.name, "mixedcase.mp3");
        assert!(pst.exif.is_empty());
        assert!(pst.xmp_tweet.is_empty());
        assert!(pst.png_prompt.is_empty());
        assert!(pst.pdf_meta.is_empty());
        assert!(pst.video_meta.is_empty());
        assert!(pst.tags.is_empty());
        assert!(pst.sidecar.is_empty());
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
    fn append_exif_can_skip_ai_user_comment() {
        let info = crate::exif_reader::ExifInfo {
            sections: vec![(
                crate::exif_reader::TagGroup::Other,
                vec![
                    (
                        "UserComment".to_string(),
                        "positive\nNegative prompt: should-not-leak".to_string(),
                    ),
                    ("LensModel".to_string(), "safe lens".to_string()),
                ],
            )],
        };
        let mut buf = String::new();
        append_exif(&mut buf, &info, true);
        assert!(buf.contains("safe lens"));
        assert!(!buf.contains("should-not-leak"), "buf={buf}");
        assert!(!buf.contains("positive"), "buf={buf}");
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
            video_meta: "video title".into(),
            tags: "".into(),
            sidecar: "".into(),
            ai_caption: "caption text".into(),
        };
        let c = pst.combined();
        assert_eq!(c, "photo.jpg canon 5d prompt text video title caption text");
    }

    #[test]
    fn get_returns_correct_field() {
        let pst = PerSourceText {
            name: "n".into(),
            exif: "e".into(),
            xmp_tweet: "x".into(),
            png_prompt: "p".into(),
            pdf_meta: "m".into(),
            video_meta: "v".into(),
            tags: "t".into(),
            sidecar: "s".into(),
            ai_caption: "a".into(),
        };
        assert_eq!(pst.get(SourceKind::Filename), "n");
        assert_eq!(pst.get(SourceKind::Exif), "e");
        assert_eq!(pst.get(SourceKind::XmpTweet), "x");
        assert_eq!(pst.get(SourceKind::PngPrompt), "p");
        assert_eq!(pst.get(SourceKind::PdfMeta), "m");
        assert_eq!(pst.get(SourceKind::VideoMeta), "v");
        assert_eq!(pst.get(SourceKind::Tags), "t");
        assert_eq!(pst.get(SourceKind::Sidecar), "s");
        assert_eq!(pst.get(SourceKind::AiCaption), "a");
    }

    #[test]
    fn stored_text_byte_sizes_count_utf8_source_fields_without_join_separators() {
        let pst = PerSourceText {
            name: "a.jpg".into(),
            exif: "夕焼け".into(),
            png_prompt: "prompt".into(),
            ..Default::default()
        };
        assert_eq!(pst.stored_text_byte_sizes(), (5 + 9 + 6, 6));
    }

    #[test]
    fn caption_text_uses_successful_content_without_signature_filtering() {
        use crate::ai_caption_db::{CaptionRow, CaptionStatus, FileSignature};
        let mut row = CaptionRow {
            rotation: crate::rotation_db::Rotation::None,
            item_key: "c:/missing.jpg".into(),
            signature: FileSignature {
                size: 0,
                mtime_ms: 0,
            },
            model_id: String::new(),
            prompt_version: 1,
            lang: "ja".into(),
            caption: "BLUE 空".into(),
            visible_text: "Visible Letter".into(),
            keywords: vec!["KEYWORD".into()],
            cover_ref: String::new(),
            status: CaptionStatus::Ok,
            created_at: 0,
            rev: 1,
            indexed: false,
        };
        let expected = "blue 空 visible letter keyword";
        assert_eq!(caption_search_text(&row), expected);
        let text = PerSourceText {
            ai_caption: caption_search_text(&row),
            ..Default::default()
        };
        assert!(!text.is_empty_all());
        assert_eq!(text.combined(), expected);
        assert_eq!(text.stored_text_byte_sizes(), (expected.len(), 0));
        row.status = CaptionStatus::Failed;
        assert!(caption_search_text(&row).is_empty());
    }

    #[test]
    fn comfy_template_fixture_keeps_path_and_bytes_ingest_text_small_and_identical() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ai_metadata/comfyui_linked_template_with_parameters.png");
        let bytes = fs::read(&path).unwrap();
        let from_path = build_per_source_for_file(&path);
        let from_bytes =
            build_per_source_from_bytes(path.file_name().unwrap().to_str().unwrap(), &bytes);

        assert_eq!(from_path.png_prompt, from_bytes.png_prompt);
        assert!(from_path.png_prompt.contains("resolved parameter positive"));
        assert!(!from_path.png_prompt.contains("template-token"));
        assert!(!from_path.png_prompt.contains("parameter-negative-leak"));
        let (_, png_prompt_bytes) = from_path.stored_text_byte_sizes();
        assert!(
            png_prompt_bytes < 1024,
            "template must not inflate STORED prompt text: {png_prompt_bytes} bytes"
        );
    }

    #[test]
    fn reads_json_sidecar_values_into_sidecar_field() {
        let tmp = TempDir::new().unwrap();
        let img = tmp.path().join("0001.jpg");
        fs::write(&img, b"not a real image").unwrap();
        fs::write(
            tmp.path().join("0001.jpg.json"),
            br#"{ "artist": "Karon-T", "tags": ["1girl"], "source": "https://ex.invalid/X" }"#,
        )
        .unwrap();
        let pst = build_per_source_for_file(&img);
        // 値が正規化 (lowercase) されて sidecar に入る
        assert!(pst.sidecar.contains("karon-t"), "sidecar={}", pst.sidecar);
        assert!(pst.sidecar.contains("1girl"));
        // キー名は入らない
        assert!(!pst.sidecar.contains("artist"));
        assert!(!pst.sidecar.contains("source"));
        // タグ系統 (tags) には混ざらない
        assert!(pst.tags.is_empty());
    }

    #[test]
    fn canonicalize_tag_name_replaces_whitespace_and_control() {
        // 旧タグ列の移行用 canonicalize は空白/制御文字を `_` にする。
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
