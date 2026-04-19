//! XMP メタデータ読み取り (mXD との連携用)。
//!
//! mXD (mxdownloader) が X/Twitter から保存したメディアファイルには、XMP RDF/XML
//! パケットに `xtw:` カスタム名前空間 (`https://mXDownloader.app/ns/x-twitter/1.0/`)
//! で tweet URL / 投稿者 / 本文 / スレッド情報等が埋め込まれている。
//!
//! このモジュールは JPEG APP1 / PNG iTXt から XMP パケットを取り出し、`xtw:*`
//! と必要最小限の `dc:*` プロパティだけを拾って [`XmpTweetInfo`] を返す。
//! フル機能の汎用 XMP パーサーではない。詳細仕様は
//! `C:/home/mxdownloader/docs/miv-integration.md` 参照。

use quick_xml::events::Event;
use quick_xml::reader::NsReader;
use std::collections::HashMap;
use std::path::Path;

/// mXD が `xtw:` 名前空間で埋め込む X/Twitter 情報。
///
/// 全フィールド optional — mXD が書き忘れたり、将来追加されるフィールドを
/// 互換性を壊さず受け入れるため。UI は `tweet_id` の有無でセクション表示を判定。
#[derive(Clone, Debug, Default)]
pub struct XmpTweetInfo {
    pub tweet_id: Option<String>,
    pub tweet_url: Option<String>,
    pub author_screen_name: Option<String>,
    pub author_id: Option<String>,
    pub author_display_name: Option<String>,
    pub author_url: Option<String>,
    /// ISO-8601 w/ offset (例: "2026:04:16 04:09:58.0000000+00:00")。整形は UI 側。
    pub posted_at: Option<String>,
    pub discovered_at: Option<String>,
    /// "Likes" または "Bookmarks"
    pub source: Option<String>,
    pub conversation_id: Option<String>,
    pub thread_part: Option<u32>,
    pub media_index: Option<u32>,
    pub media_count: Option<u32>,
    pub quoted_by_tweet_id: Option<String>,
    pub quoted_by_url: Option<String>,
    pub quoted_by_screen_name: Option<String>,
    pub quoted_by_author_id: Option<String>,
    pub quoted_by_author_display_name: Option<String>,
    /// `dc:description` (本文)
    pub description: Option<String>,
    /// `dc:creator` (投稿者表記、通常は `"{DisplayName} (@{screen})"`)
    pub creator: Option<String>,
}

impl XmpTweetInfo {
    /// 少なくとも `tweet_id` があれば有効。
    pub fn is_populated(&self) -> bool {
        self.tweet_id.is_some()
    }
}

/// mXD の xtw 名前空間 URI。**case-sensitive で完全一致**。
const XTW_NAMESPACE: &[u8] = b"https://mXDownloader.app/ns/x-twitter/1.0/";
/// dc の名前空間 URI。
const DC_NAMESPACE: &[u8] = b"http://purl.org/dc/elements/1.1/";

// ---------------------------------------------------------------------------
// 公開 API
// ---------------------------------------------------------------------------

/// mXD が出力し得るコンテナ形式 — XMP が入っている可能性がある拡張子だけ許可。
/// BMP / RAW / AVIF 等は mXD の出力対象外 + 当モジュールで解釈できないので
/// 無駄なファイル読み出しを避けるため早期に弾く。
fn extension_might_have_xmp(path: &Path) -> bool {
    match path.extension().and_then(|s| s.to_str()).map(|s| s.to_ascii_lowercase()) {
        Some(ext) => matches!(
            ext.as_str(),
            "jpg" | "jpeg" | "jfif" | "png" | "tif" | "tiff" | "mp4" | "mov" | "m4v"
        ),
        None => false,
    }
}

/// パスから読み取って [`XmpTweetInfo`] を返す。
/// XMP パケットが無い / `xtw:*` プロパティが無い場合は None。
pub fn read_tweet_info(path: &Path) -> Option<XmpTweetInfo> {
    if !extension_might_have_xmp(path) {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    read_tweet_info_from_bytes(&bytes)
}

/// バイト列版 (ZIP 内画像などで使用)。拡張子で事前フィルタできないので、
/// マジックバイトで JPEG / PNG / ISO BMFF 系かどうかを判別してから parse に進む。
pub fn read_tweet_info_from_bytes(bytes: &[u8]) -> Option<XmpTweetInfo> {
    if !has_xmp_capable_magic(bytes) {
        return None;
    }
    let xmp = extract_xmp_packet(bytes)?;
    let info = parse_xmp(&xmp)?;
    if info.is_populated() { Some(info) } else { None }
}

/// XMP が入り得るコンテナのマジックバイトか判定。
fn has_xmp_capable_magic(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        return true; // JPEG
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return true; // PNG
    }
    // ISO BMFF (MP4/MOV/HEIC): 4バイト長 + "ftyp"
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return true;
    }
    // TIFF: "II*\0" (LE) or "MM\0*" (BE)
    if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// XMP パケット抽出 (JPEG APP1 / PNG iTXt / fallback)
// ---------------------------------------------------------------------------

/// ファイル形式を自動判別して XMP パケット (x:xmpmeta を含む XML バイト列) を返す。
fn extract_xmp_packet(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        // JPEG
        if let Some(xmp) = extract_xmp_from_jpeg(bytes) {
            return Some(xmp);
        }
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        // PNG
        if let Some(xmp) = extract_xmp_from_png(bytes) {
            return Some(xmp);
        }
    }
    // フォールバック: ファイル先頭 256KB から <x:xmpmeta ... </x:xmpmeta> を切り出す。
    // MP4 / TIFF 等の未対応形式にも対応するため。
    extract_xmp_fallback(bytes)
}

/// JPEG APP1 セグメントから Adobe XMP パケットを探す。
/// Standard XMP は "http://ns.adobe.com/xap/1.0/\0" プレフィクス付き。
/// ExtendedXMP (>64KB) はここでは扱わない — mXD の出力には収まる想定。
fn extract_xmp_from_jpeg(bytes: &[u8]) -> Option<Vec<u8>> {
    const XMP_ID: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
    let mut pos = 2; // SOI をスキップ
    while pos + 4 <= bytes.len() {
        if bytes[pos] != 0xFF {
            return None;
        }
        let marker = bytes[pos + 1];
        pos += 2;
        // スタンドアロンマーカー (長さなし)
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        // SOS に来たら以降は画像データなのでメタデータ探索終了
        if marker == 0xDA {
            return None;
        }
        if pos + 2 > bytes.len() {
            return None;
        }
        let seg_len = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        if seg_len < 2 || pos + seg_len > bytes.len() {
            return None;
        }
        let payload = &bytes[pos + 2..pos + seg_len];
        if marker == 0xE1 && payload.starts_with(XMP_ID) {
            return Some(payload[XMP_ID.len()..].to_vec());
        }
        pos += seg_len;
    }
    None
}

/// PNG の iTXt チャンクから Adobe XMP を探す。keyword は `XML:com.adobe.xmp`。
fn extract_xmp_from_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut pos = 8; // PNG シグネチャをスキップ
    while pos + 8 <= bytes.len() {
        let length = u32::from_be_bytes([
            bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3],
        ]) as usize;
        let chunk_type = &bytes[pos + 4..pos + 8];
        let data_start = pos + 8;
        let data_end = data_start.checked_add(length)?;
        if data_end + 4 > bytes.len() {
            return None;
        }
        if chunk_type == b"iTXt" {
            let chunk = &bytes[data_start..data_end];
            if let Some(kw_end) = chunk.iter().position(|&b| b == 0) {
                let keyword = &chunk[..kw_end];
                if keyword == b"XML:com.adobe.xmp" {
                    // compression flag (1) + method (1) + lang\0 + translated_kw\0 + text
                    let rest = chunk.get(kw_end + 1..)?;
                    if rest.len() < 2 {
                        return None;
                    }
                    let compression_flag = rest[0];
                    let after = &rest[2..];
                    let lang_end = after.iter().position(|&b| b == 0)?;
                    let after_lang = &after[lang_end + 1..];
                    let trans_end = after_lang.iter().position(|&b| b == 0)?;
                    let text = &after_lang[trans_end + 1..];
                    if compression_flag == 0 {
                        return Some(text.to_vec());
                    }
                    // 圧縮 iTXt (通常の XMP では非圧縮が推奨) は未対応。
                    return None;
                }
            }
        } else if chunk_type == b"IEND" {
            return None;
        }
        pos = data_end + 4;
    }
    None
}

/// 最後の手段: ファイル先頭部分から `<x:xmpmeta` の開始と対応する終端を探す。
/// MP4 の `uuid` アトム (Adobe XMP の UUID) や TIFF tag 700 等を専用にパースする
/// 代わりに、バイト列中の XML サブストリングを切り出す。
///
/// 全域走査すると 100MB 超の動画でミリ秒〜秒オーダーの時間を食うので、
/// 先頭 512KB に制限する。mXD / ExifTool が書く XMP は常にコンテナ先頭部に
/// 配置されるので実用上十分。
const FALLBACK_SCAN_LIMIT: usize = 512 * 1024;
fn extract_xmp_fallback(bytes: &[u8]) -> Option<Vec<u8>> {
    let scan = &bytes[..bytes.len().min(FALLBACK_SCAN_LIMIT)];
    let start = find_subsequence(scan, b"<x:xmpmeta")?;
    let end_needle = b"</x:xmpmeta>";
    let rel_end = find_subsequence(&scan[start..], end_needle)?;
    Some(scan[start..start + rel_end + end_needle.len()].to_vec())
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

// ---------------------------------------------------------------------------
// RDF/XML → XmpTweetInfo
// ---------------------------------------------------------------------------

fn parse_xmp(xml: &[u8]) -> Option<XmpTweetInfo> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut info = XmpTweetInfo::default();
    let mut buf = Vec::new();

    // `rdf:Description` 要素のアトリビュートに `xtw:*` / `dc:*` が直接載る
    // ショートハンドもあるので、開始タグのアトリビュートも走査する。
    // 本文要素にネストする記法 (mXD の現行出力) も同時にサポート。
    //
    // パース戦略:
    // 1. 開始タグを見る: 名前空間 URI を取って (xtw/dc) 判定
    //    - `xtw:*` なら、属性 `rdf:resource` があればその値、無ければ中身のテキストを読む
    //    - `dc:description` / `dc:creator` は `rdf:Alt`/`rdf:Seq` > `rdf:li` の内部テキストを読む
    // 2. `rdf:Description` のアトリビュート記法: 属性を直接読む

    // 現在キャプチャ中の「リーフ文字列を溜める先」
    enum Capture {
        Xtw(String),        // xtw:* のローカル名
        DcDescLi,           // dc:description の中の rdf:li
        DcCreatorLi,        // dc:creator の中の rdf:li
    }
    let mut capture: Option<Capture> = None;
    // dc:description / dc:creator 内部にいるか
    let mut in_dc_description = false;
    let mut in_dc_creator = false;
    // 複数の rdf:li がある場合、最初に取れたものを採用する
    let mut xtw_fields: HashMap<String, String> = HashMap::new();

    // イベント 1 件をリーダー借用なしの形に正規化したもの。
    // リーダーの &mut 借用を貸し出しループ内に持ち込まないため、open/close/text を
    // このオーナー型に詰め替えてから処理する。
    enum Ev {
        Open {
            is_empty: bool,
            local: Vec<u8>,
            ns: Option<Vec<u8>>,
            /// 属性ベース記法 (rdf:Description xtw:Foo="...") で拾った xtw の local→value
            xtw_attrs: Vec<(String, String)>,
            /// rdf:resource 属性 (URI プロパティのショートハンド)
            resource: Option<String>,
        },
        Text(String),
        Close(Vec<u8>),
        Eof,
        Other,
    }
    const RDF_NS: &[u8] = b"http://www.w3.org/1999/02/22-rdf-syntax-ns#";

    let decoder = reader.decoder(); // Copy なのでループ前に確保

    loop {
        let event = {
            let (resolved_ns, ev) = match reader.read_resolved_event_into(&mut buf) {
                Ok(r) => r,
                Err(_) => break,
            };
            let is_start = matches!(ev, Event::Start(_));
            let is_empty = matches!(ev, Event::Empty(_));
            if is_start || is_empty {
                let e = match &ev {
                    Event::Start(e) => e,
                    Event::Empty(e) => e,
                    _ => unreachable!(),
                };
                let local: Vec<u8> = e.local_name().as_ref().to_vec();
                let ns: Option<Vec<u8>> = match &resolved_ns {
                    quick_xml::name::ResolveResult::Bound(ns) => Some(ns.as_ref().to_vec()),
                    _ => None,
                };
                // 属性を走査。resolver は &self 借用で OK。
                let resolver = reader.resolver();
                let mut xtw_attrs: Vec<(String, String)> = Vec::new();
                let mut resource: Option<String> = None;
                for attr in e.attributes().flatten() {
                    let (attr_ns, attr_local) = resolver.resolve_attribute(attr.key);
                    let ns_is = |uri: &[u8]| {
                        matches!(&attr_ns, quick_xml::name::ResolveResult::Bound(ns) if ns.as_ref() == uri)
                    };
                    if ns_is(XTW_NAMESPACE) {
                        let k = String::from_utf8_lossy(attr_local.as_ref()).into_owned();
                        if let Ok(v) = attr.decode_and_unescape_value(decoder) {
                            xtw_attrs.push((k, v.into_owned()));
                        }
                    } else if ns_is(RDF_NS) && attr_local.as_ref() == b"resource" {
                        if let Ok(v) = attr.decode_and_unescape_value(decoder) {
                            resource = Some(v.into_owned());
                        }
                    }
                }
                Ev::Open { is_empty, local, ns, xtw_attrs, resource }
            } else {
                match ev {
                    Event::Text(t) => Ev::Text(
                        t.decode().map(|s| s.into_owned()).unwrap_or_default(),
                    ),
                    Event::End(e) => Ev::Close(e.local_name().as_ref().to_vec()),
                    Event::Eof => Ev::Eof,
                    _ => Ev::Other,
                }
            }
        };

        match event {
            Ev::Open { is_empty, local, ns, xtw_attrs, resource } => {
                for (k, v) in xtw_attrs {
                    xtw_fields.entry(k).or_insert(v);
                }
                let ns_ref = ns.as_deref();
                if ns_ref == Some(XTW_NAMESPACE) {
                    let key = String::from_utf8_lossy(&local).into_owned();
                    if let Some(v) = resource {
                        xtw_fields.entry(key).or_insert(v);
                    } else if !is_empty {
                        capture = Some(Capture::Xtw(key));
                    }
                } else if ns_ref == Some(DC_NAMESPACE) {
                    match local.as_slice() {
                        b"description" => in_dc_description = true,
                        b"creator" => in_dc_creator = true,
                        _ => {}
                    }
                } else if local.as_slice() == b"li" && !is_empty {
                    if in_dc_description && info.description.is_none() {
                        capture = Some(Capture::DcDescLi);
                    } else if in_dc_creator && info.creator.is_none() {
                        capture = Some(Capture::DcCreatorLi);
                    }
                }
            }
            Ev::Text(s) => {
                if let Some(cap) = capture.take() {
                    let trimmed = s.trim().to_string();
                    match cap {
                        Capture::Xtw(name) => {
                            xtw_fields.entry(name).or_insert(trimmed);
                        }
                        Capture::DcDescLi => info.description = Some(trimmed),
                        Capture::DcCreatorLi => info.creator = Some(trimmed),
                    }
                }
            }
            Ev::Close(local) => {
                if local.as_slice() == b"description" {
                    in_dc_description = false;
                } else if local.as_slice() == b"creator" {
                    in_dc_creator = false;
                }
                // 空要素の xtw:* (テキスト無し) はここで破棄
                if matches!(capture, Some(Capture::Xtw(_))) {
                    capture = None;
                }
            }
            Ev::Eof => break,
            Ev::Other => {}
        }
        buf.clear();
    }

    // Map → struct
    let take = |m: &mut HashMap<String, String>, k: &str| m.remove(k);
    let mut m = xtw_fields;
    info.tweet_id = take(&mut m, "TweetId");
    info.tweet_url = take(&mut m, "TweetUrl");
    info.author_screen_name = take(&mut m, "AuthorScreenName");
    info.author_id = take(&mut m, "AuthorId");
    info.author_display_name = take(&mut m, "AuthorDisplayName");
    info.author_url = take(&mut m, "AuthorUrl");
    info.posted_at = take(&mut m, "PostedAt");
    info.discovered_at = take(&mut m, "DiscoveredAt");
    info.source = take(&mut m, "Source");
    info.conversation_id = take(&mut m, "ConversationId");
    info.thread_part = take(&mut m, "ThreadPart").and_then(|s| s.parse().ok());
    info.media_index = take(&mut m, "MediaIndex").and_then(|s| s.parse().ok());
    info.media_count = take(&mut m, "MediaCount").and_then(|s| s.parse().ok());
    info.quoted_by_tweet_id = take(&mut m, "QuotedByTweetId");
    info.quoted_by_url = take(&mut m, "QuotedByUrl");
    info.quoted_by_screen_name = take(&mut m, "QuotedByScreenName");
    info.quoted_by_author_id = take(&mut m, "QuotedByAuthorId");
    info.quoted_by_author_display_name = take(&mut m, "QuotedByAuthorDisplayName");

    if info.is_populated() { Some(info) } else { None }
}

// ---------------------------------------------------------------------------
// URL 検証 (未信頼メタデータなので必ずチェック)
// ---------------------------------------------------------------------------

/// Tweet URL として許容できる形式か。制御文字禁止・x.com / twitter.com 限定。
pub fn is_safe_tweet_url(url: &str) -> bool {
    if url.chars().any(|c| c.is_control()) {
        return false;
    }
    url.starts_with("https://x.com/") || url.starts_with("https://twitter.com/")
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_XMP_STR: &str = r#"<?xml version='1.0'?>
<x:xmpmeta xmlns:x='adobe:ns:meta/' x:xmptk='Image::ExifTool 13.19'>
  <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
    <rdf:Description rdf:about=''
      xmlns:dc='http://purl.org/dc/elements/1.1/'
      xmlns:xtw='https://mXDownloader.app/ns/x-twitter/1.0/'>
      <dc:creator>
        <rdf:Seq>
          <rdf:li>ルル (@Ruru_0750)</rdf:li>
        </rdf:Seq>
      </dc:creator>
      <dc:description>
        <rdf:Alt>
          <rdf:li xml:lang='x-default'>Tweet body text</rdf:li>
        </rdf:Alt>
      </dc:description>
      <xtw:AuthorDisplayName>ルル</xtw:AuthorDisplayName>
      <xtw:AuthorId>1927039467489001472</xtw:AuthorId>
      <xtw:AuthorScreenName>Ruru_0750</xtw:AuthorScreenName>
      <xtw:AuthorUrl>https://x.com/Ruru_0750</xtw:AuthorUrl>
      <xtw:ConversationId>2044629346967773284</xtw:ConversationId>
      <xtw:MediaCount>1</xtw:MediaCount>
      <xtw:MediaIndex>1</xtw:MediaIndex>
      <xtw:PostedAt>2026:04:16 04:09:58.0000000+00:00</xtw:PostedAt>
      <xtw:Source>Likes</xtw:Source>
      <xtw:ThreadPart>1</xtw:ThreadPart>
      <xtw:TweetId>2044629346967773284</xtw:TweetId>
      <xtw:TweetUrl>https://x.com/Ruru_0750/status/2044629346967773284</xtw:TweetUrl>
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>
"#;
    fn sample_xmp() -> &'static [u8] {
        SAMPLE_XMP_STR.as_bytes()
    }

    #[test]
    fn parses_element_style_xmp() {
        let info = parse_xmp(sample_xmp()).expect("should parse");
        assert_eq!(info.tweet_id.as_deref(), Some("2044629346967773284"));
        assert_eq!(
            info.tweet_url.as_deref(),
            Some("https://x.com/Ruru_0750/status/2044629346967773284")
        );
        assert_eq!(info.author_screen_name.as_deref(), Some("Ruru_0750"));
        assert_eq!(info.author_display_name.as_deref(), Some("ルル"));
        assert_eq!(info.source.as_deref(), Some("Likes"));
        assert_eq!(info.media_count, Some(1));
        assert_eq!(info.media_index, Some(1));
        assert_eq!(info.thread_part, Some(1));
        assert_eq!(info.description.as_deref(), Some("Tweet body text"));
        assert_eq!(info.creator.as_deref(), Some("ルル (@Ruru_0750)"));
    }

    #[test]
    fn parses_attribute_style_xmp() {
        // ExifTool の代替表現: プロパティを rdf:Description の属性に載せる
        let xml = br#"<x:xmpmeta xmlns:x='adobe:ns:meta/'>
          <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
            <rdf:Description rdf:about=''
              xmlns:xtw='https://mXDownloader.app/ns/x-twitter/1.0/'
              xtw:TweetId='111'
              xtw:TweetUrl='https://x.com/a/status/111'
              xtw:AuthorScreenName='a' />
          </rdf:RDF>
        </x:xmpmeta>"#;
        let info = parse_xmp(xml).expect("should parse");
        assert_eq!(info.tweet_id.as_deref(), Some("111"));
        assert_eq!(info.tweet_url.as_deref(), Some("https://x.com/a/status/111"));
        assert_eq!(info.author_screen_name.as_deref(), Some("a"));
    }

    #[test]
    fn no_xtw_returns_none() {
        let xml = br#"<x:xmpmeta xmlns:x='adobe:ns:meta/'>
          <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
            <rdf:Description rdf:about=''
              xmlns:dc='http://purl.org/dc/elements/1.1/'>
              <dc:creator><rdf:Seq><rdf:li>User</rdf:li></rdf:Seq></dc:creator>
            </rdf:Description>
          </rdf:RDF>
        </x:xmpmeta>"#;
        // xtw が無ければ tweet_id が無いので None 扱い
        let info = parse_xmp(xml);
        assert!(info.map(|i| i.is_populated()).unwrap_or(false) == false);
    }

    #[test]
    fn fallback_extracts_from_raw_bytes() {
        let mut prelude = vec![0u8; 128];
        prelude.extend_from_slice(sample_xmp());
        prelude.extend_from_slice(&[0u8; 32]);
        let xmp = extract_xmp_fallback(&prelude).expect("should find xmp");
        assert!(xmp.starts_with(b"<x:xmpmeta"));
        let info = parse_xmp(&xmp).expect("parseable");
        assert_eq!(info.tweet_id.as_deref(), Some("2044629346967773284"));
    }

    #[test]
    fn url_safety_check() {
        assert!(is_safe_tweet_url("https://x.com/foo/status/123"));
        assert!(is_safe_tweet_url("https://twitter.com/foo"));
        assert!(!is_safe_tweet_url("http://x.com/foo")); // http 拒否
        assert!(!is_safe_tweet_url("https://evil.com/foo"));
        assert!(!is_safe_tweet_url("https://x.com/foo\nmalicious"));
        assert!(!is_safe_tweet_url("javascript:alert(1)"));
    }

    #[test]
    fn read_from_empty_bytes_returns_none() {
        assert!(read_tweet_info_from_bytes(&[]).is_none());
    }

    #[test]
    fn extension_gate_filters_non_xmp_formats() {
        use std::path::Path;
        assert!(extension_might_have_xmp(Path::new("a.jpg")));
        assert!(extension_might_have_xmp(Path::new("A.JPEG")));
        assert!(extension_might_have_xmp(Path::new("b.png")));
        assert!(extension_might_have_xmp(Path::new("c.mp4")));
        assert!(extension_might_have_xmp(Path::new("d.tiff")));
        assert!(!extension_might_have_xmp(Path::new("e.bmp")));
        assert!(!extension_might_have_xmp(Path::new("f.heic")));
        assert!(!extension_might_have_xmp(Path::new("g.cr2")));
        assert!(!extension_might_have_xmp(Path::new("no-extension")));
    }

    #[test]
    fn magic_gate_rejects_unknown_binaries() {
        assert!(has_xmp_capable_magic(&[0xFF, 0xD8, 0xFF, 0xE0])); // JPEG
        assert!(has_xmp_capable_magic(b"\x89PNG\r\n\x1a\n\x00"));
        let mut mp4 = vec![0u8; 12];
        mp4[4..8].copy_from_slice(b"ftyp");
        assert!(has_xmp_capable_magic(&mp4));
        assert!(!has_xmp_capable_magic(b"GIF89a...")); // GIF は XMP 抽出器が無いので除外
        assert!(!has_xmp_capable_magic(b"BM")); // BMP
        assert!(!has_xmp_capable_magic(&[]));
    }
}
