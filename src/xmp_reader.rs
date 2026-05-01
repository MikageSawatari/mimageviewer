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
const XMP_NAMESPACE: &[u8] = b"http://ns.adobe.com/xap/1.0/";

// ---------------------------------------------------------------------------
// 公開 API
// ---------------------------------------------------------------------------

/// 拡張子を小文字 ASCII で取り出す。拡張子なし / 非 UTF-8 なら None。
fn lowercase_ext(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
}

/// mXD が出力し得るコンテナ形式 — XMP が入っている可能性がある拡張子だけ許可。
/// BMP / RAW / AVIF 等は mXD の出力対象外 + 当モジュールで解釈できないので
/// 無駄なファイル読み出しを避けるため早期に弾く。
fn extension_might_have_xmp(path: &Path) -> bool {
    matches!(
        lowercase_ext(path).as_deref(),
        Some("jpg" | "jpeg" | "jfif" | "png" | "tif" | "tiff" | "mp4" | "mov" | "m4v")
    )
}

/// パスから読み取って [`XmpTweetInfo`] を返す。
/// XMP パケットが無い / `xtw:*` プロパティが無い場合は None。
///
/// **フォーマット別の読み込み戦略**:
/// - JPEG / PNG (通常 ≤50MB): ファイル全体を読む。専用パーサーが
///   APP1 / iTXt をコンテナ末尾まで走査するので、optipng や jpegtran 等で
///   XMP セグメントが先頭から離れた位置に置かれていても拾える。
/// - MP4 / MOV / M4V / TIFF (数百MB ありうる): 先頭 [`FALLBACK_SCAN_LIMIT`]
///   バイトだけ読む。mXD / ExifTool が書く XMP は uuid アトム / IFD0 の
///   先頭付近に置かれるので 512KB あれば実用上十分。これにより UI 同期スレッド
///   での丸読みハングを防ぐ。
pub fn read_tweet_info(path: &Path) -> Option<XmpTweetInfo> {
    if !extension_might_have_xmp(path) {
        return None;
    }
    let small_image = matches!(
        lowercase_ext(path).as_deref(),
        Some("jpg" | "jpeg" | "jfif" | "png")
    );
    if small_image {
        let bytes = std::fs::read(path).ok()?;
        return read_tweet_info_from_bytes(&bytes);
    }
    // 大容量コンテナ系: 先頭 FALLBACK_SCAN_LIMIT のみ
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut buf = Vec::with_capacity(FALLBACK_SCAN_LIMIT.min(64 * 1024));
    f.take(FALLBACK_SCAN_LIMIT as u64)
        .read_to_end(&mut buf)
        .ok()?;
    read_tweet_info_from_bytes(&buf)
}

/// バイト列版 (ZIP 内画像などで使用)。拡張子で事前フィルタできないので、
/// マジックバイトで JPEG / PNG / ISO BMFF 系かどうかを判別してから parse に進む。
pub fn read_tweet_info_from_bytes(bytes: &[u8]) -> Option<XmpTweetInfo> {
    if !has_xmp_capable_magic(bytes) {
        return None;
    }
    let xmp = extract_xmp_packet(bytes)?;
    let info = parse_xmp(&xmp)?;
    if info.is_populated() {
        Some(info)
    } else {
        None
    }
}

/// XMP が入り得るコンテナのマジックバイトか判定。
pub(crate) fn has_xmp_capable_magic(bytes: &[u8]) -> bool {
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
pub(crate) fn extract_xmp_packet(bytes: &[u8]) -> Option<Vec<u8>> {
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
        let length =
            u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
                as usize;
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

/// バイト列中で needle の最初の出現位置を返す。needle が空 or hay より長ければ None。
pub(crate) fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
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
        Xtw(String), // xtw:* のローカル名
        DcDescLi,    // dc:description の中の rdf:li
        DcCreatorLi, // dc:creator の中の rdf:li
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
                    let ns_is = |uri: &[u8]| matches!(&attr_ns, quick_xml::name::ResolveResult::Bound(ns) if ns.as_ref() == uri);
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
                Ev::Open {
                    is_empty,
                    local,
                    ns,
                    xtw_attrs,
                    resource,
                }
            } else {
                match ev {
                    Event::Text(t) => {
                        Ev::Text(t.decode().map(|s| s.into_owned()).unwrap_or_default())
                    }
                    Event::End(e) => Ev::Close(e.local_name().as_ref().to_vec()),
                    Event::Eof => Ev::Eof,
                    _ => Ev::Other,
                }
            }
        };

        match event {
            Ev::Open {
                is_empty,
                local,
                ns,
                xtw_attrs,
                resource,
            } => {
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

    if info.is_populated() {
        Some(info)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// dc:subject 読み取り (タグ機能 — docs/tag-feature.md §6.4)
// ---------------------------------------------------------------------------

/// ファイルの XMP `dc:subject` Bag 要素を読み取る。
/// `#` で始まるもの・つかないもの問わず全て返す。呼び出し側が必要に応じて
/// `#` 接頭辞でフィルタする。
pub fn read_dc_subject(path: &Path) -> Vec<String> {
    if !extension_might_have_xmp(path) {
        return Vec::new();
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    read_dc_subject_from_bytes(&bytes)
}

/// バイト列版。
pub fn read_dc_subject_from_bytes(bytes: &[u8]) -> Vec<String> {
    if !has_xmp_capable_magic(bytes) {
        return Vec::new();
    }
    let Some(xmp) = extract_xmp_packet(bytes) else {
        return Vec::new();
    };
    parse_dc_subject(&xmp)
}

/// 生の XMP RDF/XML バイト列から `<dc:subject><rdf:Bag><rdf:li>値</rdf:li>...</rdf:Bag></dc:subject>`
/// の要素を抽出する (コンテナ非依存版、xmp_writer がパケット直接編集で使う)。
/// 名前空間で厳密判定 (URI = purl.org/dc/elements/1.1/)。
pub(crate) fn parse_dc_subject(xml: &[u8]) -> Vec<String> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut tags: Vec<String> = Vec::new();
    // dc:subject 要素の内部にいるか
    let mut depth_in_dc_subject: i32 = 0;
    // 現在 rdf:li の内部にいてテキストを収集中か
    let mut in_li = false;
    let mut current_value = String::new();

    loop {
        let ev = match reader.read_resolved_event_into(&mut buf) {
            Ok((ns, e)) => (ns, e),
            Err(_) => break,
        };
        match ev {
            (ns, Event::Start(e)) => {
                let local = e.local_name().as_ref().to_vec();
                let is_dc_subject = local == b"subject"
                    && matches!(&ns, quick_xml::name::ResolveResult::Bound(b) if b.as_ref() == DC_NAMESPACE);
                if is_dc_subject {
                    depth_in_dc_subject += 1;
                } else if depth_in_dc_subject > 0 && local == b"li" {
                    in_li = true;
                    current_value.clear();
                }
            }
            (_, Event::Text(t)) => {
                if in_li {
                    if let Ok(s) = t.decode() {
                        current_value.push_str(s.as_ref());
                    }
                }
            }
            (_, Event::End(e)) => {
                let local = e.local_name().as_ref().to_vec();
                if local == b"li" && in_li {
                    let v = current_value.trim().to_string();
                    if !v.is_empty() {
                        tags.push(v);
                    }
                    in_li = false;
                    current_value.clear();
                } else if local == b"subject" && depth_in_dc_subject > 0 {
                    depth_in_dc_subject -= 1;
                }
            }
            (_, Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }
    tags
}

// ---------------------------------------------------------------------------
// xmp:Rating 読み取り (レーティング機能 — XMP spec: xmp:Rating 0..5)
// ---------------------------------------------------------------------------

/// ファイルの XMP `xmp:Rating` を読み取る。0 / 未設定で `None` を返す。
/// 1..=5 の範囲外は clamp。
pub fn read_xmp_rating(path: &Path) -> Option<u8> {
    if !extension_might_have_xmp(path) {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    read_xmp_rating_from_bytes(&bytes)
}

/// バイト列版。
pub fn read_xmp_rating_from_bytes(bytes: &[u8]) -> Option<u8> {
    if !has_xmp_capable_magic(bytes) {
        return None;
    }
    let xmp = extract_xmp_packet(bytes)?;
    parse_xmp_rating(&xmp)
}

/// 生の XMP RDF/XML から `xmp:Rating` を取る。
/// 属性形式 (`<rdf:Description xmp:Rating="4"/>`) と要素形式
/// (`<xmp:Rating>4</xmp:Rating>`) の両方を拾う (Lightroom は属性、古い書き出しは要素)。
pub(crate) fn parse_xmp_rating(xml: &[u8]) -> Option<u8> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut in_rating = false;
    let mut current_value = String::new();
    let mut found: Option<u8> = None;

    loop {
        let ev = match reader.read_resolved_event_into(&mut buf) {
            Ok((ns, e)) => (ns, e),
            Err(_) => break,
        };
        match ev {
            (ns, Event::Start(e)) => {
                scan_attributes_for_rating(&e, &mut found);
                let local = e.local_name().as_ref().to_vec();
                let is_rating_elem = local == b"Rating"
                    && matches!(&ns, quick_xml::name::ResolveResult::Bound(b) if b.as_ref() == XMP_NAMESPACE);
                if is_rating_elem {
                    in_rating = true;
                    current_value.clear();
                }
            }
            (_ns, Event::Empty(e)) => {
                // self-closing: 属性形式のみチェック (`<rdf:Description xmp:Rating="4"/>`)
                scan_attributes_for_rating(&e, &mut found);
            }
            (_, Event::Text(t)) => {
                if in_rating {
                    if let Ok(s) = t.decode() {
                        current_value.push_str(s.as_ref());
                    }
                }
            }
            (_, Event::End(e)) => {
                let local = e.local_name().as_ref().to_vec();
                if local == b"Rating" && in_rating {
                    if let Some(n) = parse_rating_value(current_value.trim()) {
                        found = Some(n);
                    }
                    in_rating = false;
                    current_value.clear();
                }
            }
            (_, Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }
    found
}

fn scan_attributes_for_rating(e: &quick_xml::events::BytesStart<'_>, found: &mut Option<u8>) {
    for attr in e.attributes().flatten() {
        let key = attr.key.as_ref();
        // prefix:local を分解。rdf:Description 上の xmp:Rating= 属性を拾う。
        let prefix_is_xmp = key
            .split(|&c| c == b':')
            .next()
            .map(|p| p == b"xmp")
            .unwrap_or(false);
        let local = key.rsplit(|&c| c == b':').next().unwrap_or(key);
        if local == b"Rating" && (prefix_is_xmp || !key.contains(&b':')) {
            if let Ok(v) = attr.unescape_value() {
                if let Some(n) = parse_rating_value(v.as_ref()) {
                    *found = Some(n);
                }
            }
        }
    }
}

fn parse_rating_value(s: &str) -> Option<u8> {
    // Lightroom は "-1" (rejected) も使うが mIV では 0 扱い。
    // 小数 ("3.5") は floor で解釈する: 3.5 → 3, 0.9 → 0, 5.9 → 5。
    let v: f32 = s.trim().parse().ok()?;
    if v < 1.0 {
        Some(0)
    } else {
        Some((v.floor() as i32).clamp(1, 5) as u8)
    }
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
        assert_eq!(
            info.tweet_url.as_deref(),
            Some("https://x.com/a/status/111")
        );
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

    /// `<x:xmpmeta>...</x:xmpmeta>` を含む有効な APP1 ペイロードを 1 つ持つ
    /// 最小 JPEG をでっち上げる。`junk_app1_size` バイトのダミー APP1 を
    /// XMP の **前に** 挟むことで、ファイル先頭からの距離を増やせる。
    fn build_jpeg_with_xmp_at_offset(junk_app1_size: usize) -> Vec<u8> {
        let xmp_id = b"http://ns.adobe.com/xap/1.0/\0";
        let xmp_payload: Vec<u8> = xmp_id
            .iter()
            .chain(SAMPLE_XMP_STR.as_bytes())
            .copied()
            .collect();

        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(&[0xFF, 0xD8]); // SOI

        // ダミー APP1 (Exif 風) を 64KB 単位で繰り返し挿入。APP* は length が 16bit (max 65535)。
        let mut remaining = junk_app1_size;
        while remaining > 0 {
            let chunk = remaining.min(65533);
            out.extend_from_slice(&[0xFF, 0xE1]); // APP1
            let seg_len = (chunk + 2) as u16;
            out.extend_from_slice(&seg_len.to_be_bytes());
            // length 値は length 自体の 2 バイト含む。中身は適当な 0 埋め
            out.extend(std::iter::repeat_n(0u8, chunk));
            remaining -= chunk;
        }

        // 本物の XMP APP1
        out.extend_from_slice(&[0xFF, 0xE1]);
        let seg_len = (xmp_payload.len() + 2) as u16;
        out.extend_from_slice(&seg_len.to_be_bytes());
        out.extend_from_slice(&xmp_payload);

        // SOS + EOI (画像本体は無くても extract_xmp_from_jpeg は SOS で打ち切るのでここまでで十分)
        out.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0xFF, 0xD9]);
        out
    }

    /// fallback-only コンテナの代用として、ftyp マジックを持つ最小 MP4 風バイト列を作る。
    fn build_mp4_with_xmp_at_offset(offset: usize) -> Vec<u8> {
        let offset = offset.max(12);
        let mut out = vec![0u8; offset];
        // ISO BMFF magic: 4-byte box size + "ftyp"
        out[4..8].copy_from_slice(b"ftyp");
        out.extend_from_slice(SAMPLE_XMP_STR.as_bytes());
        out
    }

    /// XMP が先頭から 512KB 超の位置にあっても、JPEG は全読みするので拾える。
    #[test]
    fn jpeg_with_xmp_past_512kb_is_found() {
        // FALLBACK_SCAN_LIMIT = 512KB。それを超える junk を XMP の前に置く。
        let bytes = build_jpeg_with_xmp_at_offset(700 * 1024);
        assert!(bytes.len() > FALLBACK_SCAN_LIMIT);
        let info = read_tweet_info_from_bytes(&bytes).expect("XMP should be parsed");
        assert_eq!(info.tweet_id.as_deref(), Some("2044629346967773284"));
    }

    /// `read_tweet_info(path)` のファイルベース経路でも、JPEG は全読みパスを
    /// 通るので 512KB 超の位置にある XMP を拾えること (Codex Finding 2 のリグレッション)。
    #[test]
    fn read_tweet_info_path_reads_full_jpeg() {
        let bytes = build_jpeg_with_xmp_at_offset(700 * 1024);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("big.jpg");
        std::fs::write(&path, &bytes).expect("write tempfile");
        let info = read_tweet_info(&path).expect("XMP should be parsed via path");
        assert_eq!(info.tweet_id.as_deref(), Some("2044629346967773284"));
    }

    /// MP4/MOV/M4V/TIFF など fallback-only の大容量コンテナは、パス経由では
    /// 先頭 FALLBACK_SCAN_LIMIT だけを読む。先頭付近の XMP は拾える。
    #[test]
    fn read_tweet_info_path_finds_mp4_xmp_near_start() {
        let bytes = build_mp4_with_xmp_at_offset(128);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("early.mp4");
        std::fs::write(&path, &bytes).expect("write tempfile");
        let info = read_tweet_info(&path).expect("XMP should be parsed via bounded path");
        assert_eq!(info.tweet_id.as_deref(), Some("2044629346967773284"));
    }

    /// MP4/MOV/M4V/TIFF など fallback-only の大容量コンテナでは全読みしない。
    /// 512KB より後ろの XMP を拾わないことを回帰テストにして、bounded read を保つ。
    #[test]
    fn read_tweet_info_path_bounds_mp4_fallback_scan() {
        let bytes = build_mp4_with_xmp_at_offset(700 * 1024);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("late.mp4");
        std::fs::write(&path, &bytes).expect("write tempfile");
        assert!(read_tweet_info(&path).is_none());
    }

    // ---- dc:subject 読み取り ----

    #[test]
    fn parse_dc_subject_standard_bag() {
        let xml = r#"<x:xmpmeta xmlns:x='adobe:ns:meta/'>
          <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
            <rdf:Description rdf:about=''
              xmlns:dc='http://purl.org/dc/elements/1.1/'>
              <dc:subject>
                <rdf:Bag>
                  <rdf:li>#原神</rdf:li>
                  <rdf:li>#風景</rdf:li>
                  <rdf:li>既存タグ</rdf:li>
                </rdf:Bag>
              </dc:subject>
            </rdf:Description>
          </rdf:RDF>
        </x:xmpmeta>"#;
        let tags = parse_dc_subject(xml.as_bytes());
        assert_eq!(tags, vec!["#原神", "#風景", "既存タグ"]);
    }

    #[test]
    fn parse_dc_subject_empty_when_absent() {
        let tags = parse_dc_subject(SAMPLE_XMP_STR.as_bytes());
        // SAMPLE_XMP_STR は dc:subject を持たない (dc:description と dc:creator のみ)
        assert!(tags.is_empty());
    }

    #[test]
    fn parse_dc_subject_ignores_empty_li() {
        let xml = br#"<x:xmpmeta xmlns:x='adobe:ns:meta/'>
          <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
            <rdf:Description rdf:about=''
              xmlns:dc='http://purl.org/dc/elements/1.1/'>
              <dc:subject>
                <rdf:Bag>
                  <rdf:li></rdf:li>
                  <rdf:li>valid</rdf:li>
                  <rdf:li>   </rdf:li>
                </rdf:Bag>
              </dc:subject>
            </rdf:Description>
          </rdf:RDF>
        </x:xmpmeta>"#;
        let tags = parse_dc_subject(xml);
        assert_eq!(tags, vec!["valid"]);
    }

    #[test]
    fn parse_dc_subject_tolerates_undeclared_xmp_prefix_after_subject() {
        // mxd が書き出す XMP は最初の rdf:Description に xmlns:dc しか載せない。
        // そこに我々が `<xmp:MetadataDate>` を追記すると、xmlns:xmp が未宣言のまま
        // 未定義プレフィックスを使う不正 XML になる。このケースでも dc:subject が
        // 先に出現していれば tag を取り出せることを保証する。
        let xmp = r#"<?xpacket begin='' id='W5M0MpCehiHzreSzNTczkc9d'?>
<x:xmpmeta xmlns:x='adobe:ns:meta/'>
<rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
 <rdf:Description rdf:about=''
  xmlns:dc='http://purl.org/dc/elements/1.1/'>
    <dc:subject>
      <rdf:Bag>
        <rdf:li>#ドール</rdf:li>
      </rdf:Bag>
    </dc:subject>
    <xmp:MetadataDate>2026-04-22T04:04:58+00:00</xmp:MetadataDate>
    </rdf:Description>
</rdf:RDF>
</x:xmpmeta>
<?xpacket end='w'?>"#;
        let tags = parse_dc_subject(xmp.as_bytes());
        assert_eq!(
            tags,
            vec!["#ドール"],
            "xmlns:xmp 未宣言でも dc:subject は読める"
        );
    }

    #[test]
    fn read_dc_subject_from_jpeg_roundtrip() {
        let xml = r#"<?xml version='1.0'?>
<x:xmpmeta xmlns:x='adobe:ns:meta/'>
  <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
    <rdf:Description rdf:about=''
      xmlns:dc='http://purl.org/dc/elements/1.1/'>
      <dc:subject>
        <rdf:Bag>
          <rdf:li>#nature</rdf:li>
          <rdf:li>#landscape</rdf:li>
        </rdf:Bag>
      </dc:subject>
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>"#;
        // JPEG APP1 セグメントを手動で構築
        let xmp_id = b"http://ns.adobe.com/xap/1.0/\0";
        let payload: Vec<u8> = xmp_id.iter().chain(xml.as_bytes()).copied().collect();
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(&[0xFF, 0xD8]);
        out.extend_from_slice(&[0xFF, 0xE1]);
        let seg_len = (payload.len() + 2) as u16;
        out.extend_from_slice(&seg_len.to_be_bytes());
        out.extend_from_slice(&payload);
        out.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0xFF, 0xD9]);

        let tags = read_dc_subject_from_bytes(&out);
        assert_eq!(tags, vec!["#nature", "#landscape"]);
    }

    // ---- xmp:Rating 読み取り ----

    #[test]
    fn parse_rating_attribute_form() {
        // Lightroom が書く属性形式
        let xml = r#"<x:xmpmeta xmlns:x='adobe:ns:meta/'>
          <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
            <rdf:Description rdf:about=''
              xmlns:xmp='http://ns.adobe.com/xap/1.0/'
              xmp:Rating='4'/>
          </rdf:RDF>
        </x:xmpmeta>"#;
        assert_eq!(parse_xmp_rating(xml.as_bytes()), Some(4));
    }

    #[test]
    fn parse_rating_element_form() {
        let xml = r#"<x:xmpmeta xmlns:x='adobe:ns:meta/'>
          <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
            <rdf:Description rdf:about=''
              xmlns:xmp='http://ns.adobe.com/xap/1.0/'>
              <xmp:Rating>5</xmp:Rating>
            </rdf:Description>
          </rdf:RDF>
        </x:xmpmeta>"#;
        assert_eq!(parse_xmp_rating(xml.as_bytes()), Some(5));
    }

    #[test]
    fn parse_rating_absent() {
        assert_eq!(parse_xmp_rating(SAMPLE_XMP_STR.as_bytes()), None);
    }

    #[test]
    fn parse_rating_negative_rejected_to_zero() {
        // Lightroom の "rejected" (-1) は mIV では 0 扱い
        let xml = r#"<x:xmpmeta xmlns:x='adobe:ns:meta/'>
          <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
            <rdf:Description rdf:about=''
              xmlns:xmp='http://ns.adobe.com/xap/1.0/'
              xmp:Rating='-1'/>
          </rdf:RDF>
        </x:xmpmeta>"#;
        assert_eq!(parse_xmp_rating(xml.as_bytes()), Some(0));
    }
}
