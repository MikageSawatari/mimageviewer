//! XMP `dc:subject` (タグ) の書き込み (docs/tag-feature.md §5)。
//!
//! # 対応形式
//!
//! - **JPEG** (APP1 `http://ns.adobe.com/xap/1.0/\0`)
//!   Extended XMP は **バイト列のまま保持** し、Standard XMP のみ書き換える。
//!   `xmpNote:HasExtendedXMP` は消さない (消すと Extended XMP が孤児化して
//!   mXDownloader が埋め込んだツイート本文が失われる)。
//! - **PNG** (iTXt `XML:com.adobe.xmp` チャンク、CRC 再計算付き)
//! - **WebP** (RIFF `XMP ` チャンク。単純 WebP は VP8X 拡張コンテナに昇格)
//!
//! # 書き込み戦略 (最小差分編集)
//!
//! 1. 既存 XMP パケットをファイルから抽出 (無ければ最小パケット合成)
//! 2. XMP 内の `<dc:subject>` 要素範囲を quick-xml でバイト位置特定
//!    - 見つかれば新 Bag で丸ごと置換
//!    - 無ければ `<rdf:Description>` の閉じタグ直前に element 形式で挿入
//!    - `rdf:Description` 自己閉じタグは開始+終了形式に展開してから挿入
//! 3. `xmp:MetadataDate` を ISO-8601 現在時刻で更新 (Lightroom 同期のため、
//!    XMP Spec Part 1 §8.4)
//! 4. 形式別にファイルへ再埋め込み
//! 5. 一時ファイル → rename でアトミック置換

use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use quick_xml::events::Event;
use quick_xml::reader::NsReader;

use crate::xmp_reader;
use crate::xmp_reader::find_subsequence;

/// タグ / レーティングの XMP 書き込み全体にまたがる排他ロック。
/// `apply_tag_op` (tag_write_worker スレッド) と `apply_rating` (rating_write_worker
/// スレッド) が同じファイルに対して read-modify-write を並走させると、後勝ちで
/// 相手の編集 (dc:subject / xmp:Rating) を上書き消去してしまうため、ここで直列化する。
/// 1 ファイル I/O は msec オーダーで、ユーザ操作頻度では競合コストは無視できる。
static XMP_WRITE_LOCK: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// エラー型
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum WriteError {
    /// 対応していないファイル形式 (拡張子判定 or マジックバイト判定で弾かれた)
    UnsupportedFormat,
    /// ファイル I/O エラー
    Io(std::io::Error),
    /// XMP パース / シリアライズエラー
    Xmp(String),
    /// Standard XMP 書き込み後サイズが 64KB を超えた。
    /// Standard → Extended 再分割は未実装。
    StandardXmpTooLarge { required: usize },
    /// 読み取り専用ファイル
    ReadOnly,
    /// 想定外のコンテナ構造 (破損ファイル等)
    MalformedContainer(String),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::UnsupportedFormat => write!(f, "対応していないファイル形式です"),
            WriteError::Io(e) => write!(f, "I/O エラー: {e}"),
            WriteError::Xmp(s) => write!(f, "XMP 処理エラー: {s}"),
            WriteError::StandardXmpTooLarge { required } => write!(
                f,
                "XMP パケットが 64KB を超えるため書き込めません ({required} bytes、v1.0 制限)"
            ),
            WriteError::ReadOnly => write!(f, "ファイルが読み取り専用です"),
            WriteError::MalformedContainer(s) => write!(f, "ファイル構造エラー: {s}"),
        }
    }
}

impl std::error::Error for WriteError {}

impl From<std::io::Error> for WriteError {
    fn from(e: std::io::Error) -> Self {
        WriteError::Io(e)
    }
}

// ---------------------------------------------------------------------------
// 対応形式判定
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Jpeg,
    Png,
    WebP,
}

fn detect_format(path: &Path) -> Option<Format> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())?;
    match ext.as_str() {
        "jpg" | "jpeg" | "jfif" => Some(Format::Jpeg),
        "png" => Some(Format::Png),
        "webp" => Some(Format::WebP),
        _ => None,
    }
}

/// パスの拡張子がタグ書き込み対応形式か判定。
/// 現状 JPEG / PNG / WebP のみ。UI の grayout 判定と worker の処理判定の
/// 単一ソースにする。
pub fn is_writable_format(path: &Path) -> bool {
    detect_format(path).is_some()
}

// ---------------------------------------------------------------------------
// 公開 API
// ---------------------------------------------------------------------------

/// タグ書き込み操作。
#[derive(Debug, Clone)]
pub enum TagOp {
    /// `#name` 形式で 1 要素追加 (既に存在すれば何もしない)。
    Add(String),
    /// `#name` 形式の要素を削除 (存在しなければ何もしない)。
    Remove(String),
    /// `#` で始まる全要素を削除 (他タグは保持)。
    ClearMiv,
}

/// 指定ファイルに対してタグ操作を適用する。アトミック書き込み + 既存メタ保持。
///
/// 成功時は `Ok(new_tags)` で編集後のタグ列 (スペース区切り `#原神 #風景 既存`) を返す。
pub fn apply_tag_op(path: &Path, op: &TagOp) -> Result<String, WriteError> {
    let _lock = XMP_WRITE_LOCK.lock().unwrap();
    let format = detect_format(path).ok_or(WriteError::UnsupportedFormat)?;

    // 読み取り専用チェック (早期失敗)
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.permissions().readonly() {
            return Err(WriteError::ReadOnly);
        }
    }

    let bytes = std::fs::read(path)?;

    let (new_bytes, new_tags) = match format {
        Format::Jpeg => apply_tag_op_jpeg(&bytes, op)?,
        Format::Png => apply_tag_op_png(&bytes, op)?,
        Format::WebP => apply_tag_op_webp(&bytes, op)?,
    };

    write_atomically(path, &new_bytes)?;
    Ok(new_tags)
}

/// 指定ファイルの `xmp:Rating` を設定する。`rating` が `None` / `Some(0)` なら削除。
/// 対応形式は JPEG / PNG / WebP。読み取り専用は `ReadOnly` エラー。
///
/// タグ操作と同じ read-modify-write 経路だが、更新する XMP プロパティが
/// `xmp:Rating` である点だけが違う。dc:subject (タグ) は素通しで保持される。
pub fn apply_rating(path: &Path, rating: Option<u8>) -> Result<(), WriteError> {
    let _lock = XMP_WRITE_LOCK.lock().unwrap();
    let format = detect_format(path).ok_or(WriteError::UnsupportedFormat)?;
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.permissions().readonly() {
            return Err(WriteError::ReadOnly);
        }
    }
    let bytes = std::fs::read(path)?;
    let new_bytes = match format {
        Format::Jpeg => apply_rating_op_jpeg(&bytes, rating)?,
        Format::Png => apply_rating_op_png(&bytes, rating)?,
        Format::WebP => apply_rating_op_webp(&bytes, rating)?,
    };
    write_atomically(path, &new_bytes)?;
    Ok(())
}

/// 同一ディレクトリに tmp ファイルを作って rename でアトミックに置換。
fn write_atomically(target: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    let dir = target.parent().unwrap_or(Path::new("."));
    let tmp_name = format!(".miv-tag-{}.tmp", uuid::Uuid::new_v4());
    let tmp = dir.join(tmp_name);
    // スコープで File を閉じてから rename する (Windows で open 中は rename 不可)
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all().ok();
    }
    match std::fs::rename(&tmp, target) {
        Ok(_) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(WriteError::Io(e))
        }
    }
}

// ---------------------------------------------------------------------------
// XMP パケット編集 (形式非依存)
// ---------------------------------------------------------------------------

/// 最小 XMP パケット (dc:subject が空の状態)。新規ファイルへの初回書き込みで使う。
///
/// x:xmpmeta の属性 `xmptk` は書き込みツール名 (ExifTool が自身を示すのと同じ)。
/// `rdf:about=''` は XMP 仕様上の慣例で、パケットの主語 (画像そのもの) を表す。
const MINIMAL_XMP_TEMPLATE: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="mimageviewer">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
      xmlns:dc="http://purl.org/dc/elements/1.1/"
      xmlns:xmp="http://ns.adobe.com/xap/1.0/">
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

/// 既存 XMP パケット (バイト列) に対してタグ操作を適用し、新 XMP パケットを返す。
/// パケットが空文字列なら最小テンプレートから開始する。
/// 戻り値は (new_xmp_bytes, new_tags_space_separated)。
pub fn edit_xmp_packet(xmp: &[u8], op: &TagOp) -> Result<(Vec<u8>, String), WriteError> {
    let original = if xmp.is_empty() {
        MINIMAL_XMP_TEMPLATE.as_bytes().to_vec()
    } else {
        xmp.to_vec()
    };

    // 1. 既存 dc:subject のタグ一覧を読む (裸の XMP パケットなのでマジックバイト判定なし)
    let mut current = xmp_reader::parse_dc_subject(&original);

    // 2. 操作を適用
    let changed = apply_op_to_list(&mut current, op);

    // 3. XMP パケットの dc:subject 領域を差し替え
    let updated = replace_or_insert_dc_subject(&original, &current)?;

    // 4. MetadataDate を更新 (changed でも未 changed でも、タグ関連のクリアが
    //    空配列→空配列のような no-op なら更新しない)
    let with_date = if changed {
        update_metadata_date(&updated)?
    } else {
        updated
    };

    let tag_str = crate::ingest_text::build_tags_column(&current);
    Ok((with_date, tag_str))
}

/// 既存 XMP パケット (バイト列) に対して xmp:Rating を設定/削除し、新パケットを返す。
/// `rating == None` or `Some(0)`: `xmp:Rating` を削除 (属性形式 / 要素形式の両方)。
/// `Some(1..=5)`: `xmp:Rating="N"` 属性として設定 (Lightroom / Adobe 標準形)。
/// タグ (dc:subject) は触らず、他の既存プロパティもすべて保持。
pub fn edit_xmp_packet_rating(xmp: &[u8], rating: Option<u8>) -> Result<Vec<u8>, WriteError> {
    let original = if xmp.is_empty() {
        MINIMAL_XMP_TEMPLATE.as_bytes().to_vec()
    } else {
        xmp.to_vec()
    };
    let normalized = rating.and_then(|r| if r == 0 { None } else { Some(r.min(5)) });
    // 1. 既存値と比較して no-op なら早期 return (MetadataDate 更新も避ける)
    let current = xmp_reader::parse_xmp_rating(&original);
    if current == normalized {
        return Ok(original);
    }
    // 2. xmp:Rating を設定 / 削除
    let updated = write_xmp_rating(&original, normalized)?;
    // 3. MetadataDate 更新
    update_metadata_date(&updated)
}

/// XMP パケット内の xmp:Rating を属性 / 要素形式のどちらにもある場合はすべて削除してから、
/// `Some(n)` なら rdf:Description に属性として追加する。
fn write_xmp_rating(xmp: &[u8], rating: Option<u8>) -> Result<Vec<u8>, WriteError> {
    // Step 1: 要素形式 `<xmp:Rating>...</xmp:Rating>` を削除
    let mut bytes = strip_rating_element(xmp);
    // Step 2: 属性形式 `xmp:Rating="..."` を rdf:Description から削除
    bytes = strip_rating_attribute(&bytes);
    // Step 3: Some なら rdf:Description 開始タグに属性として追加
    if let Some(n) = rating {
        bytes = insert_rating_attribute(&bytes, n)?;
    }
    Ok(bytes)
}

/// `<xmp:Rating>N</xmp:Rating>` を丸ごと削除する (要素形式対応)。
/// プレフィックスが `xmp:` 以外になっているケースは稀なので拾わない。
fn strip_rating_element(xmp: &[u8]) -> Vec<u8> {
    let needle_start = b"<xmp:Rating";
    let needle_end = b"</xmp:Rating>";
    let mut out = xmp.to_vec();
    loop {
        let Some(s) = find_subsequence(&out, needle_start) else {
            break;
        };
        // 属性形式のみ (`<xmp:Rating="4"/>` のような self-closing は element ではない)
        // を誤って消さないために、対応する `</xmp:Rating>` があるときのみ削除。
        if let Some(e_rel) = find_subsequence(&out[s..], needle_end) {
            let end = s + e_rel + needle_end.len();
            out.drain(s..end);
        } else {
            break;
        }
    }
    out
}

/// rdf:Description 開始タグ上の `xmp:Rating="..."` 属性 (および xmlns 以外の prefix 違い)
/// を削除する。rdf:Description が複数ある場合はすべてに対して処理する。
fn strip_rating_attribute(xmp: &[u8]) -> Vec<u8> {
    let mut out = xmp.to_vec();
    let tag_start = b"<rdf:Description";
    let mut search_from = 0;
    while let Some(rel) = find_subsequence(&out[search_from..], tag_start) {
        let open = search_from + rel;
        let Some(close_rel) = out[open..].iter().position(|&b| b == b'>') else {
            break;
        };
        let close = open + close_rel;
        // rdf:Description 開始タグの属性範囲 [open+tag_start.len()..close]
        let (new_attrs, removed) = remove_rating_attr(&out[open..close]);
        if removed {
            out.splice(open..close, new_attrs);
            // out.len が変わる可能性があるので search_from は open から再開
            search_from = open;
        } else {
            search_from = close;
        }
    }
    out
}

/// rdf:Description タグ中 (`<rdf:Description ...` の `<` から `>` の手前まで) の
/// `xmp:Rating="..."` / `xmp:Rating='...'` を削除した新バイト列を返す。
/// 変更があれば `removed = true`。純粋にバイト操作で処理するので、rdf:about 属性等に
/// 非 ASCII (UTF-8 の multibyte 文字) が入っていても破壊しない。
fn remove_rating_attr(tag_slice: &[u8]) -> (Vec<u8>, bool) {
    let needle = b"xmp:Rating";
    let mut out: Vec<u8> = Vec::with_capacity(tag_slice.len());
    let mut i = 0;
    let mut removed = false;
    while i < tag_slice.len() {
        if tag_slice[i..].starts_with(needle) {
            let after = i + needle.len();
            let ws_len = tag_slice[after..]
                .iter()
                .take_while(|&&c| c == b' ' || c == b'\t')
                .count();
            if after + ws_len < tag_slice.len() && tag_slice[after + ws_len] == b'=' {
                let quote_start = after + ws_len + 1;
                let q_ws = tag_slice[quote_start..]
                    .iter()
                    .take_while(|&&c| c == b' ' || c == b'\t')
                    .count();
                if quote_start + q_ws < tag_slice.len() {
                    let quote = tag_slice[quote_start + q_ws];
                    if quote == b'"' || quote == b'\'' {
                        let value_start = quote_start + q_ws + 1;
                        if let Some(end_rel) =
                            tag_slice[value_start..].iter().position(|&c| c == quote)
                        {
                            let end = value_start + end_rel + 1;
                            // 削除する属性の前にある空白を 1 つ吸収して、タグ開始との
                            // 間に余計な空白を残さない。
                            while let Some(&b' ' | &b'\t') = out.last() {
                                out.pop();
                            }
                            out.push(b' ');
                            i = end;
                            removed = true;
                            continue;
                        }
                    }
                }
            }
        }
        out.push(tag_slice[i]);
        i += 1;
    }
    (out, removed)
}

/// 最初に見つかった `<rdf:Description` 開始タグの `>` 直前に `xmp:Rating="N"` を挿入する。
fn insert_rating_attribute(xmp: &[u8], rating: u8) -> Result<Vec<u8>, WriteError> {
    let Some(open) = find_subsequence(xmp, b"<rdf:Description") else {
        return Err(WriteError::Xmp(
            "XMP パケットに <rdf:Description> が見つからない".to_string(),
        ));
    };
    // `<rdf:Description` から次の `>` または `/>` を探す。
    let Some(close_rel) = xmp[open..].iter().position(|&b| b == b'>') else {
        return Err(WriteError::Xmp("rdf:Description の閉じが不明".into()));
    };
    let mut close = open + close_rel;
    // self-closing `/>` なら `/` の位置を挿入点とする。
    if close > 0 && xmp[close - 1] == b'/' {
        close -= 1;
    }
    let insertion = format!(" xmp:Rating=\"{}\"", rating.min(5));
    let mut out = Vec::with_capacity(xmp.len() + insertion.len());
    out.extend_from_slice(&xmp[..close]);
    out.extend_from_slice(insertion.as_bytes());
    out.extend_from_slice(&xmp[close..]);
    Ok(out)
}

/// list に対して op を適用。変更があれば true を返す。
fn apply_op_to_list(list: &mut Vec<String>, op: &TagOp) -> bool {
    match op {
        TagOp::Add(name) => {
            if list.iter().any(|t| t == name) {
                false
            } else {
                list.push(name.clone());
                true
            }
        }
        TagOp::Remove(name) => {
            let before = list.len();
            list.retain(|t| t != name);
            list.len() != before
        }
        TagOp::ClearMiv => {
            let before = list.len();
            list.retain(|t| !t.starts_with('#'));
            list.len() != before
        }
    }
}

// ---------------------------------------------------------------------------
// dc:subject の差し替え / 挿入
// ---------------------------------------------------------------------------

const DC_NS: &[u8] = b"http://purl.org/dc/elements/1.1/";

/// 既存 XMP パケット内の `<dc:subject>...</dc:subject>` をバイト位置で探し、
/// 新しい Bag 形式の要素で丸ごと置換する。存在しなければ `<rdf:Description>` の
/// 閉じタグ直前に挿入する。
fn replace_or_insert_dc_subject(
    xmp: &[u8],
    tags: &[String],
) -> Result<Vec<u8>, WriteError> {
    let new_element = build_dc_subject_element(tags);

    if let Some((start, end)) = find_dc_subject_range(xmp)? {
        // 既存を置換
        let mut out = Vec::with_capacity(xmp.len() + new_element.len());
        out.extend_from_slice(&xmp[..start]);
        out.extend_from_slice(new_element.as_bytes());
        out.extend_from_slice(&xmp[end..]);
        return Ok(out);
    }

    // dc:subject 不在: rdf:Description の閉じタグ直前に挿入
    if let Some(insert_pos) = find_rdf_description_close_pos(xmp)? {
        let mut out = Vec::with_capacity(xmp.len() + new_element.len() + 4);
        out.extend_from_slice(&xmp[..insert_pos]);
        out.extend_from_slice(b"\n    ");
        out.extend_from_slice(new_element.as_bytes());
        out.extend_from_slice(b"\n  ");
        out.extend_from_slice(&xmp[insert_pos..]);
        return Ok(out);
    }

    // rdf:Description 自己閉じタグ `<rdf:Description ... />` を展開
    if let Some((self_close_start, self_close_end)) = find_rdf_description_self_close(xmp)? {
        // `<rdf:Description xxx />` → `<rdf:Description xxx>\n  <dc:subject>...</dc:subject>\n</rdf:Description>`
        // self_close_start は `/` の位置、self_close_end は `>` の次の位置 (排他)
        let mut out = Vec::with_capacity(xmp.len() + new_element.len() + 40);
        out.extend_from_slice(&xmp[..self_close_start]);
        out.extend_from_slice(b">\n    ");
        out.extend_from_slice(new_element.as_bytes());
        out.extend_from_slice(b"\n  </rdf:Description>");
        out.extend_from_slice(&xmp[self_close_end..]);
        return Ok(out);
    }

    Err(WriteError::Xmp(
        "XMP パケットに <rdf:Description> が見つからない".to_string(),
    ))
}

fn build_dc_subject_element(tags: &[String]) -> String {
    if tags.is_empty() {
        // 空でも要素自体は置いておく (他ツールが dc:subject の存在を期待するため)
        return r#"<dc:subject><rdf:Bag/></dc:subject>"#.to_string();
    }
    let mut out = String::from("<dc:subject>\n      <rdf:Bag>\n");
    for t in tags {
        let escaped = quick_xml::escape::escape(t);
        out.push_str(&format!("        <rdf:li>{escaped}</rdf:li>\n"));
    }
    out.push_str("      </rdf:Bag>\n    </dc:subject>");
    out
}

/// `<dc:subject>` 開始タグの直前位置と `</dc:subject>` の直後位置を返す。
/// NsReader で名前空間を正しく判定してから、quick-xml の buffer_position で
/// バイトオフセットを復元する。
fn find_dc_subject_range(xmp: &[u8]) -> Result<Option<(usize, usize)>, WriteError> {
    // quick-xml は "tag 終端のオフセット" は返してくれるが、開始タグの
    // 開始オフセットは生バイト列から `<dc:subject` サブストリングで探す方が確実。
    //
    // 戦略:
    //   1. NsReader で dc:subject 要素の存在を確認 (名前空間判定)
    //   2. 確認できたら、バイト列中の `<dc:subject` と `</dc:subject>` を直接検索
    //   3. 名前空間プレフィックスが異なるケース (`<dc2:subject>` 等) は
    //      実運用で極稀なので諦める (NsReader で検出されたら警告ログに留める)
    let mut reader = NsReader::from_reader(xmp);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut found = false;
    loop {
        match reader.read_resolved_event_into(&mut buf) {
            Ok((ns, Event::Start(e))) => {
                if is_dc_subject(&ns, e.local_name().as_ref()) {
                    found = true;
                    break;
                }
            }
            Ok((ns, Event::Empty(e))) => {
                if is_dc_subject(&ns, e.local_name().as_ref()) {
                    found = true;
                    break;
                }
            }
            Ok((_, Event::Eof)) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        buf.clear();
    }
    if !found {
        return Ok(None);
    }

    // バイト列から `<dc:subject` を探す (最初の出現)。
    // プレフィックスが `dc` でない場合は名前空間判定を諦める (前述のとおり稀)。
    let open_needle = b"<dc:subject";
    let close_needle = b"</dc:subject>";
    let Some(open_pos) = find_subsequence(xmp, open_needle) else {
        return Ok(None); // プレフィックス違いなのでスキップ
    };
    // `<dc:subject/>` (自己閉じ) の場合
    let from = open_pos + open_needle.len();
    // 自己閉じかどうか判定: 次の `>` までに `/` があるか
    let end_of_open = xmp[from..]
        .iter()
        .position(|&b| b == b'>')
        .map(|p| p + from)
        .ok_or_else(|| WriteError::MalformedContainer("dc:subject 開始タグ未閉".into()))?;
    let between = &xmp[from..end_of_open];
    let is_self_closing = between.last() == Some(&b'/');
    if is_self_closing {
        return Ok(Some((open_pos, end_of_open + 1)));
    }
    // 閉じタグを探す (nest は dc:subject 内に dc:subject が無い前提で浅く)
    let close_pos = find_subsequence(&xmp[end_of_open..], close_needle)
        .map(|p| p + end_of_open)
        .ok_or_else(|| {
            WriteError::MalformedContainer("dc:subject 閉じタグが見つからない".into())
        })?;
    Ok(Some((open_pos, close_pos + close_needle.len())))
}

fn is_dc_subject(ns: &quick_xml::name::ResolveResult<'_>, local: &[u8]) -> bool {
    if local != b"subject" {
        return false;
    }
    matches!(ns, quick_xml::name::ResolveResult::Bound(n) if n.as_ref() == DC_NS)
}


/// `<rdf:Description>` の閉じタグ `</rdf:Description>` の開始位置を返す。
/// 最初に出現するものを採用 (XMP は通常 1 つだけ)。
fn find_rdf_description_close_pos(xmp: &[u8]) -> Result<Option<usize>, WriteError> {
    Ok(find_subsequence(xmp, b"</rdf:Description>"))
}

/// `<rdf:Description ... />` の自己閉じパターンを探す。
/// 戻り値は (`/` の位置, `>` の次の位置)。
fn find_rdf_description_self_close(xmp: &[u8]) -> Result<Option<(usize, usize)>, WriteError> {
    // 単純実装: `<rdf:Description` で始まり、次の `>` までに `/` があり、
    // その `/` の直後が `>` のもの。属性の最後が `/>` の形。
    let open = b"<rdf:Description";
    let Some(start) = find_subsequence(xmp, open) else {
        return Ok(None);
    };
    let from = start + open.len();
    let end = xmp[from..]
        .iter()
        .position(|&b| b == b'>')
        .map(|p| p + from)
        .ok_or_else(|| {
            WriteError::MalformedContainer("rdf:Description 開始タグ未閉".into())
        })?;
    // `>` の直前が `/` なら self-close
    if end > 0 && xmp[end - 1] == b'/' {
        return Ok(Some((end - 1, end + 1)));
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// xmp:MetadataDate の更新
// ---------------------------------------------------------------------------

/// XMP Spec Part 1 §8.4 準拠: 編集時に xmp:MetadataDate を現在時刻に更新する。
/// 存在しなければ rdf:Description の閉じタグ直前に追加。
///
/// 既存 element の検出は `<xmp:MetadataDate` (閉じ `>` を含めない) で前方一致する。
/// 我々が挿入する形式は `<xmp:MetadataDate xmlns:xmp="...">` で属性付きなので、`<xmp:MetadataDate>`
/// 完全一致だと既存をヒットさせられず毎回追記してしまう (mxd ファイルで `MetadataDate` が
/// 雪だるま式に増える regression を生む) — ここを開きタグ内の任意属性に耐えるよう緩める。
///
/// 過去の bug で複数の `<xmp:MetadataDate>` が累積したファイルに対しては、最初の 1 つを
/// 更新したあと残りを削除して 1 個だけにする (XMP Spec 上 1 つの schema property は 1 値が原則)。
fn update_metadata_date(xmp: &[u8]) -> Result<Vec<u8>, WriteError> {
    let now = current_iso8601_utc();
    let needle_open_prefix = b"<xmp:MetadataDate";
    let needle_end = b"</xmp:MetadataDate>";

    if let Some(open_start) = find_subsequence(xmp, needle_open_prefix) {
        // 開きタグの `>` を見つけてコンテンツ開始位置を決定する
        let after_prefix = open_start + needle_open_prefix.len();
        let Some(rel_close) = xmp[after_prefix..].iter().position(|&b| b == b'>') else {
            return Err(WriteError::MalformedContainer(
                "xmp:MetadataDate 開きタグ未閉".into(),
            ));
        };
        let content_start = after_prefix + rel_close + 1;
        // 自己閉じ `<xmp:MetadataDate ... />` の場合は内容置換ではなく element 全体を
        // タイムスタンプ付きの通常 element に差し替える (実運用ではほぼ無いが防御)。
        let is_self_closing = rel_close > 0 && xmp[after_prefix + rel_close - 1] == b'/';
        if is_self_closing {
            let replacement = format!(
                r#"<xmp:MetadataDate xmlns:xmp="http://ns.adobe.com/xap/1.0/">{now}</xmp:MetadataDate>"#
            );
            let mut out = Vec::with_capacity(xmp.len() + replacement.len());
            out.extend_from_slice(&xmp[..open_start]);
            out.extend_from_slice(replacement.as_bytes());
            out.extend_from_slice(&xmp[content_start..]);
            return Ok(out);
        }
        let Some(e) = find_subsequence(&xmp[content_start..], needle_end) else {
            return Err(WriteError::MalformedContainer(
                "xmp:MetadataDate 閉じタグ未発見".into(),
            ));
        };
        let content_end = content_start + e;
        let elem_end = content_end + needle_end.len();
        let mut out = Vec::with_capacity(xmp.len() + now.len());
        out.extend_from_slice(&xmp[..content_start]);
        out.extend_from_slice(now.as_bytes());
        // 末尾以降に残っている重複 MetadataDate 要素 (過去 bug の累積) を削除する。
        // 開始タグ単位で走査 → 閉じタグまでをまるごとスキップ。隣接する空白行も巻き添えで
        // 落として体裁を保つ。
        let tail = &xmp[content_end..];
        out.extend_from_slice(&tail[..needle_end.len()]); // </xmp:MetadataDate>
        let after_first_elem = &tail[needle_end.len()..];
        let cleaned_tail = strip_trailing_metadata_dates(after_first_elem);
        out.extend_from_slice(&cleaned_tail);
        let _ = elem_end; // (used implicitly via lengths above)
        return Ok(out);
    }

    // 追加 (rdf:Description 閉じタグ直前)。xmlns:xmp が document のどこにも宣言されて
    // いない XMP (ExifTool が最小構成で書いたファイル等 — mxd 経由で出てくる) に
    // prefix 付き要素を挿入すると XML が不正になる。要素側で xmlns:xmp を宣言して
    // しまえば、既存の Description 属性がどうであれ常に valid な XML になる。
    if let Some(close_pos) = find_subsequence(xmp, b"</rdf:Description>") {
        let insert = format!(
            r#"<xmp:MetadataDate xmlns:xmp="http://ns.adobe.com/xap/1.0/">{now}</xmp:MetadataDate>
    "#
        );
        let mut out = Vec::with_capacity(xmp.len() + insert.len() + 4);
        out.extend_from_slice(&xmp[..close_pos]);
        out.extend_from_slice(b"  ");
        out.extend_from_slice(insert.as_bytes());
        out.extend_from_slice(&xmp[close_pos..]);
        return Ok(out);
    }

    // rdf:Description すら無いパケットは想定外 (直前の replace_or_insert_dc_subject で
    // エラーを返しているはず)
    Ok(xmp.to_vec())
}

/// `bytes` の先頭から続く `<xmp:MetadataDate ...>...</xmp:MetadataDate>` (前後に空白
/// /改行を含む) を全部削ってから残りを返す。連続して並んでいる重複 MetadataDate を
/// 1 つだけ残す目的で `update_metadata_date` から呼ばれる。
///
/// 「先頭から続く」のは、最初の MetadataDate 要素を残した直後から走査するため。
/// 異種の要素 (例: `<xtw:...>`) に当たったら走査を打ち切る — 関係ない位置にある
/// MetadataDate (別 rdf:Description 内など) は触らない。
fn strip_trailing_metadata_dates(bytes: &[u8]) -> Vec<u8> {
    let needle_open = b"<xmp:MetadataDate";
    let needle_close = b"</xmp:MetadataDate>";
    let mut pos = 0;
    loop {
        // 先頭の空白/改行をスキップ
        let ws_end = bytes[pos..]
            .iter()
            .position(|&b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
            .map(|p| pos + p)
            .unwrap_or(bytes.len());
        if !bytes[ws_end..].starts_with(needle_open) {
            // 直後が MetadataDate 開始タグでなければここで止める。
            // (空白/改行は元の体裁を保つため残す)
            break;
        }
        // 開きタグの閉じ `>` を探す
        let after_prefix = ws_end + needle_open.len();
        let Some(rel) = bytes[after_prefix..].iter().position(|&b| b == b'>') else {
            break;
        };
        let after_open = after_prefix + rel + 1;
        // self-close なら element はそこまで
        let elem_end = if rel > 0 && bytes[after_prefix + rel - 1] == b'/' {
            after_open
        } else {
            // </xmp:MetadataDate> を探す
            let Some(rel_close) = find_subsequence(&bytes[after_open..], needle_close) else {
                break;
            };
            after_open + rel_close + needle_close.len()
        };
        pos = elem_end;
    }
    let mut out = Vec::with_capacity(bytes.len() - pos);
    out.extend_from_slice(&bytes[pos..]);
    out
}

fn current_iso8601_utc() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;
    let second = rem % 60;
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}+00:00"
    )
}

/// Howard Hinnant's date algorithm: days since UNIX epoch → civil (y, m, d).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { (mp + 3) as u32 } else { (mp - 9) as u32 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

// ---------------------------------------------------------------------------
// JPEG 書き込み
// ---------------------------------------------------------------------------

const JPEG_XMP_ID: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const JPEG_EXT_XMP_ID: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
const JPEG_STANDARD_XMP_MAX: usize = 65503; // 65535 - 2 (seg_len) - 30 (signature)

fn apply_tag_op_jpeg(bytes: &[u8], op: &TagOp) -> Result<(Vec<u8>, String), WriteError> {
    // 1. APP1 群を (Standard XMP, Extended XMP(複数), それ以外) に分類
    let parsed = parse_jpeg_segments(bytes)?;

    // 2. Standard XMP を抽出 (無ければ空)
    let std_xmp = parsed
        .standard_xmp_payload
        .as_deref()
        .unwrap_or(&[] as &[u8]);

    // 3. XMP パケット編集
    let (new_xmp, new_tags) = edit_xmp_packet(std_xmp, op)?;

    if new_xmp.len() > JPEG_STANDARD_XMP_MAX {
        return Err(WriteError::StandardXmpTooLarge {
            required: new_xmp.len(),
        });
    }

    // 4. 新 JPEG を組み立て
    let out = rebuild_jpeg(bytes, &parsed, &new_xmp)?;
    Ok((out, new_tags))
}

/// `apply_tag_op_jpeg` と同じ構造だが、XMP パケット編集を rating 用に差し替えた版。
fn apply_rating_op_jpeg(bytes: &[u8], rating: Option<u8>) -> Result<Vec<u8>, WriteError> {
    let parsed = parse_jpeg_segments(bytes)?;
    let std_xmp = parsed
        .standard_xmp_payload
        .as_deref()
        .unwrap_or(&[] as &[u8]);
    let new_xmp = edit_xmp_packet_rating(std_xmp, rating)?;
    if new_xmp.len() > JPEG_STANDARD_XMP_MAX {
        return Err(WriteError::StandardXmpTooLarge {
            required: new_xmp.len(),
        });
    }
    rebuild_jpeg(bytes, &parsed, &new_xmp)
}

/// JPEG セグメント解析結果。
struct JpegParsed {
    /// SOI 直後から最初の非 APP セグメント (通常 SOF) 手前までの各セグメント
    /// を (marker, full_segment_bytes_including_marker_and_length) で列挙。
    /// Standard XMP と Extended XMP は除外してここに入れる。
    header_segments: Vec<Vec<u8>>,
    /// 元の Standard XMP payload (signature 除く)
    standard_xmp_payload: Option<Vec<u8>>,
    /// Extended XMP APP1 チャンクの full segment (marker + length + signature + data)。
    /// 複数ある場合は順序を保持。書き戻し時にそのまま挿入する。
    extended_xmp_segments: Vec<Vec<u8>>,
    /// ヘッダ後の残り (SOS から EOI まで、エントロピー符号化データを含む)
    tail: Vec<u8>,
}

fn parse_jpeg_segments(bytes: &[u8]) -> Result<JpegParsed, WriteError> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return Err(WriteError::MalformedContainer("JPEG SOI なし".into()));
    }
    let mut header_segments: Vec<Vec<u8>> = Vec::new();
    let mut standard_xmp_payload: Option<Vec<u8>> = None;
    let mut extended_xmp_segments: Vec<Vec<u8>> = Vec::new();

    let mut pos = 2; // SOI の直後
    while pos + 4 <= bytes.len() {
        if bytes[pos] != 0xFF {
            return Err(WriteError::MalformedContainer(format!(
                "marker expected at 0x{:X}",
                pos
            )));
        }
        let marker = bytes[pos + 1];
        // スタンドアロンマーカー
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            pos += 2;
            continue;
        }
        // SOS (0xDA) 以降はエントロピー符号化データ。ここで打ち切って残りは tail。
        if marker == 0xDA {
            let tail = bytes[pos..].to_vec();
            return Ok(JpegParsed {
                header_segments,
                standard_xmp_payload,
                extended_xmp_segments,
                tail,
            });
        }
        if pos + 4 > bytes.len() {
            return Err(WriteError::MalformedContainer("truncated marker".into()));
        }
        let seg_len = u16::from_be_bytes([bytes[pos + 2], bytes[pos + 3]]) as usize;
        if seg_len < 2 {
            return Err(WriteError::MalformedContainer("invalid seg_len".into()));
        }
        let seg_end = pos + 2 + seg_len;
        if seg_end > bytes.len() {
            return Err(WriteError::MalformedContainer(
                "segment extends beyond file".into(),
            ));
        }
        let payload = &bytes[pos + 4..seg_end];
        // APP1 XMP 判定
        if marker == 0xE1 && payload.starts_with(JPEG_XMP_ID) {
            let xmp_body = payload[JPEG_XMP_ID.len()..].to_vec();
            standard_xmp_payload = Some(xmp_body);
        } else if marker == 0xE1 && payload.starts_with(JPEG_EXT_XMP_ID) {
            let full = bytes[pos..seg_end].to_vec();
            extended_xmp_segments.push(full);
        } else {
            // EXIF/ICC/APP0/APP2 など、そのまま保持
            let full = bytes[pos..seg_end].to_vec();
            header_segments.push(full);
        }
        pos = seg_end;
    }
    // SOS 無し → 異常ファイル
    Err(WriteError::MalformedContainer("SOS marker not found".into()))
}

fn rebuild_jpeg(
    _original: &[u8],
    parsed: &JpegParsed,
    new_xmp: &[u8],
) -> Result<Vec<u8>, WriteError> {
    let mut out: Vec<u8> = Vec::with_capacity(_original.len() + new_xmp.len());
    // SOI
    out.extend_from_slice(&[0xFF, 0xD8]);

    // Adobe 推奨順: APP0 → APP1(Standard XMP) → APP1(Extended XMP 群) → その他
    //
    // ただし既存ファイルの APP0 (JFIF) や APP1 (EXIF) の順序を尊重するため、
    // header_segments のうち APP0/APP1 Exif はそのまま先に積み、XMP を追加する。
    // 簡略化のため、header_segments を順番通り保持 + XMP を末尾 (SOF 手前) に追加。
    // SOF は header_segments に含まれる。
    //
    // 実用上はヘッダセグメント群の末尾に XMP を差し込めば、多くのリーダが正しく読む。
    // ExifTool も実質これに近い挙動。

    // header_segments を列挙: SOF 系 (C0/C2 等) の直前に XMP を入れたい。
    // 判定: marker が 0xC0..=0xCF で 0xC4/0xC8/0xCC 以外なら SOF (frame header)。
    // SOF 前に XMP を配置することで APP セグメント群の末尾に挿入する形になる。
    let mut xmp_written = false;
    for seg in &parsed.header_segments {
        if seg.len() >= 2 && seg[0] == 0xFF {
            let m = seg[1];
            let is_sof = (0xC0..=0xCF).contains(&m) && m != 0xC4 && m != 0xC8 && m != 0xCC;
            if is_sof && !xmp_written {
                write_standard_xmp_app1(&mut out, new_xmp)?;
                for ext in &parsed.extended_xmp_segments {
                    out.extend_from_slice(ext);
                }
                xmp_written = true;
            }
        }
        out.extend_from_slice(seg);
    }
    // SOF が見つからなかった (極稀) 場合は SOS 直前に入れる
    if !xmp_written {
        write_standard_xmp_app1(&mut out, new_xmp)?;
        for ext in &parsed.extended_xmp_segments {
            out.extend_from_slice(ext);
        }
    }
    out.extend_from_slice(&parsed.tail);
    Ok(out)
}

fn write_standard_xmp_app1(out: &mut Vec<u8>, xmp: &[u8]) -> Result<(), WriteError> {
    let payload_len = JPEG_XMP_ID.len() + xmp.len();
    if payload_len + 2 > 65535 {
        return Err(WriteError::StandardXmpTooLarge {
            required: payload_len,
        });
    }
    out.extend_from_slice(&[0xFF, 0xE1]);
    let seg_len = (payload_len + 2) as u16;
    out.extend_from_slice(&seg_len.to_be_bytes());
    out.extend_from_slice(JPEG_XMP_ID);
    out.extend_from_slice(xmp);
    Ok(())
}

// ---------------------------------------------------------------------------
// PNG 書き込み
// ---------------------------------------------------------------------------

const PNG_SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
const PNG_XMP_KEYWORD: &[u8] = b"XML:com.adobe.xmp";

/// `apply_tag_op_png` と同じ iTXt 探索 + 再構築を行うが、XMP 編集だけ rating 用。
fn apply_rating_op_png(bytes: &[u8], rating: Option<u8>) -> Result<Vec<u8>, WriteError> {
    let (existing_xmp_range, old_xmp) = find_png_xmp_itxt(bytes)?;
    let new_xmp = edit_xmp_packet_rating(&old_xmp, rating)?;
    let new_chunk = build_png_itxt_xmp(&new_xmp);
    let out = if let Some((start, end)) = existing_xmp_range {
        let mut out = Vec::with_capacity(bytes.len() + new_chunk.len());
        out.extend_from_slice(&bytes[..start]);
        out.extend_from_slice(&new_chunk);
        out.extend_from_slice(&bytes[end..]);
        out
    } else {
        let after_ihdr = find_first_chunk_end(bytes, b"IHDR")
            .ok_or_else(|| WriteError::MalformedContainer("PNG IHDR not found".into()))?;
        let mut out = Vec::with_capacity(bytes.len() + new_chunk.len());
        out.extend_from_slice(&bytes[..after_ihdr]);
        out.extend_from_slice(&new_chunk);
        out.extend_from_slice(&bytes[after_ihdr..]);
        out
    };
    Ok(out)
}

/// PNG iTXt (XML:com.adobe.xmp) チャンクを走査して、存在すれば chunk 範囲と
/// デコード済み XMP テキストを返す。チャンク圧縮は未対応。
fn find_png_xmp_itxt(bytes: &[u8]) -> Result<(Option<(usize, usize)>, Vec<u8>), WriteError> {
    if !bytes.starts_with(PNG_SIG) {
        return Err(WriteError::MalformedContainer("PNG signature なし".into()));
    }
    let mut pos = PNG_SIG.len();
    while pos + 8 <= bytes.len() {
        let length =
            u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
                as usize;
        let chunk_type = &bytes[pos + 4..pos + 8];
        let data_start = pos + 8;
        let data_end = data_start
            .checked_add(length)
            .ok_or_else(|| WriteError::MalformedContainer("PNG chunk length overflow".into()))?;
        let crc_end = data_end + 4;
        if crc_end > bytes.len() {
            return Err(WriteError::MalformedContainer(
                "PNG chunk extends beyond file".into(),
            ));
        }
        if chunk_type == b"iTXt" {
            let chunk = &bytes[data_start..data_end];
            if let Some(kw_end) = chunk.iter().position(|&b| b == 0) {
                let kw = &chunk[..kw_end];
                if kw == PNG_XMP_KEYWORD && chunk.len() >= kw_end + 3 {
                    let compression_flag = chunk[kw_end + 1];
                    let rest = &chunk[kw_end + 3..];
                    if let Some(lang_end) = rest.iter().position(|&b| b == 0) {
                        let after_lang = &rest[lang_end + 1..];
                        if let Some(trans_end) = after_lang.iter().position(|&b| b == 0) {
                            let text = &after_lang[trans_end + 1..];
                            if compression_flag == 0 {
                                return Ok((Some((pos, crc_end)), text.to_vec()));
                            }
                        }
                    }
                }
            }
        }
        if chunk_type == b"IEND" {
            break;
        }
        pos = crc_end;
    }
    Ok((None, Vec::new()))
}

fn apply_tag_op_png(bytes: &[u8], op: &TagOp) -> Result<(Vec<u8>, String), WriteError> {
    let (existing_xmp_range, old_xmp) = find_png_xmp_itxt(bytes)?;
    let (new_xmp, new_tags) = edit_xmp_packet(&old_xmp, op)?;
    let new_chunk = build_png_itxt_xmp(&new_xmp);
    let out = if let Some((start, end)) = existing_xmp_range {
        let mut out = Vec::with_capacity(bytes.len() + new_chunk.len());
        out.extend_from_slice(&bytes[..start]);
        out.extend_from_slice(&new_chunk);
        out.extend_from_slice(&bytes[end..]);
        out
    } else {
        let after_ihdr = find_first_chunk_end(bytes, b"IHDR")
            .ok_or_else(|| WriteError::MalformedContainer("PNG IHDR not found".into()))?;
        let mut out = Vec::with_capacity(bytes.len() + new_chunk.len());
        out.extend_from_slice(&bytes[..after_ihdr]);
        out.extend_from_slice(&new_chunk);
        out.extend_from_slice(&bytes[after_ihdr..]);
        out
    };
    Ok((out, new_tags))
}

fn find_first_chunk_end(bytes: &[u8], chunk_type: &[u8; 4]) -> Option<usize> {
    let mut pos = PNG_SIG.len();
    while pos + 8 <= bytes.len() {
        let length =
            u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
                as usize;
        let ty = &bytes[pos + 4..pos + 8];
        let crc_end = pos + 8 + length + 4;
        if ty == chunk_type {
            return Some(crc_end);
        }
        pos = crc_end;
    }
    None
}

/// PNG iTXt チャンク (keyword=XML:com.adobe.xmp, 非圧縮) を構築。
/// [length(4)] [type='iTXt'(4)] [keyword\0] [comp_flag=0] [comp_method=0]
///   [lang\0 (空)] [translated_keyword\0 (空)] [text] [crc(4)]
fn build_png_itxt_xmp(xmp_text: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(PNG_XMP_KEYWORD.len() + 5 + xmp_text.len());
    data.extend_from_slice(PNG_XMP_KEYWORD);
    data.push(0); // keyword terminator
    data.push(0); // compression_flag = 0 (uncompressed)
    data.push(0); // compression_method = 0 (default for uncompressed = ignored)
    data.push(0); // language_tag\0 (空)
    data.push(0); // translated_keyword\0 (空)
    data.extend_from_slice(xmp_text);

    let mut out = Vec::with_capacity(data.len() + 12);
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(b"iTXt");
    out.extend_from_slice(&data);
    // CRC は type + data 上で計算
    let crc_input: Vec<u8> = b"iTXt".iter().chain(data.iter()).copied().collect();
    let crc = png_crc32(&crc_input);
    out.extend_from_slice(&crc.to_be_bytes());
    out
}

/// PNG 仕様の CRC-32 (多項式 0xEDB88320、初期値 0xFFFFFFFF、出力反転あり)。
fn png_crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB88320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        t
    });
    let mut crc: u32 = 0xFFFFFFFF;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFFFFFF
}

// ---------------------------------------------------------------------------
// WebP 書き込み
// ---------------------------------------------------------------------------

/// `apply_tag_op_webp` と同じ RIFF 解析 + VP8X フラグ付与 + 再構築だが、XMP 編集は rating 用。
fn apply_rating_op_webp(bytes: &[u8], rating: Option<u8>) -> Result<Vec<u8>, WriteError> {
    let (mut chunks, existing_xmp) = parse_webp_chunks(bytes)?;
    let new_xmp = edit_xmp_packet_rating(existing_xmp.as_deref().unwrap_or(&[]), rating)?;
    ensure_vp8x_with_xmp_flag(&mut chunks)?;
    chunks.push(("XMP ".to_string(), new_xmp));
    Ok(rebuild_webp_riff(&chunks))
}

/// WebP RIFF を解析して (非 XMP チャンク列, 既存 XMP payload) を返す。
/// 新規 rating/tag 書き込みで共有する軽量パーサ。
fn parse_webp_chunks(
    bytes: &[u8],
) -> Result<(Vec<(String, Vec<u8>)>, Option<Vec<u8>>), WriteError> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Err(WriteError::MalformedContainer("WebP RIFF ヘッダ不正".into()));
    }
    let mut pos = 12;
    let mut chunks: Vec<(String, Vec<u8>)> = Vec::new();
    let mut existing_xmp: Option<Vec<u8>> = None;
    while pos + 8 <= bytes.len() {
        let fourcc = std::str::from_utf8(&bytes[pos..pos + 4])
            .map_err(|_| WriteError::MalformedContainer("WebP fourcc not utf8".into()))?
            .to_string();
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let data_start = pos + 8;
        let data_end = data_start + size;
        if data_end > bytes.len() {
            return Err(WriteError::MalformedContainer("WebP chunk extends beyond file".into()));
        }
        let data = bytes[data_start..data_end].to_vec();
        if fourcc == "XMP " {
            existing_xmp = Some(data);
        } else {
            chunks.push((fourcc, data));
        }
        pos = data_end + (size & 1);
    }
    Ok((chunks, existing_xmp))
}

fn rebuild_webp_riff(chunks: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(b"WEBP");
    for (fourcc, data) in chunks {
        body.extend_from_slice(fourcc.as_bytes());
        let sz = data.len() as u32;
        body.extend_from_slice(&sz.to_le_bytes());
        body.extend_from_slice(data);
        if data.len() & 1 == 1 {
            body.push(0);
        }
    }
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn apply_tag_op_webp(bytes: &[u8], op: &TagOp) -> Result<(Vec<u8>, String), WriteError> {
    let (mut chunks, existing_xmp) = parse_webp_chunks(bytes)?;
    let (new_xmp, new_tags) = edit_xmp_packet(existing_xmp.as_deref().unwrap_or(&[]), op)?;
    ensure_vp8x_with_xmp_flag(&mut chunks)?;
    chunks.push(("XMP ".to_string(), new_xmp));
    Ok((rebuild_webp_riff(&chunks), new_tags))
}

/// VP8X 拡張チャンクの有無を確認し、無ければ挿入し、あれば XMP フラグ (bit2) を立てる。
/// 拡張メタデータ (XMP / Exif) を持つ WebP には VP8X が必須。
fn ensure_vp8x_with_xmp_flag(chunks: &mut Vec<(String, Vec<u8>)>) -> Result<(), WriteError> {
    let has_vp8x = chunks.iter().any(|(fc, _)| fc == "VP8X");
    if has_vp8x {
        for (fc, data) in chunks.iter_mut() {
            if fc == "VP8X" && !data.is_empty() {
                data[0] |= 0b0000_0100;
            }
        }
    } else {
        let (w, h) = extract_webp_canvas_size(chunks).ok_or_else(|| {
            WriteError::MalformedContainer("WebP 画像サイズが取得できない".into())
        })?;
        let mut vp8x = vec![0u8; 10];
        vp8x[0] = 0b0000_0100; // bit2 = XMP metadata present
        let w1 = w - 1;
        let h1 = h - 1;
        vp8x[4] = (w1 & 0xFF) as u8;
        vp8x[5] = ((w1 >> 8) & 0xFF) as u8;
        vp8x[6] = ((w1 >> 16) & 0xFF) as u8;
        vp8x[7] = (h1 & 0xFF) as u8;
        vp8x[8] = ((h1 >> 8) & 0xFF) as u8;
        vp8x[9] = ((h1 >> 16) & 0xFF) as u8;
        chunks.insert(0, ("VP8X".to_string(), vp8x));
    }
    Ok(())
}

fn extract_webp_canvas_size(chunks: &[(String, Vec<u8>)]) -> Option<(u32, u32)> {
    for (fc, data) in chunks {
        if fc == "VP8 " && data.len() >= 10 {
            // VP8 key frame: 3 bytes frame tag, 3 bytes start code, 2 bytes width, 2 bytes height
            // width is lower 14 bits at offset 6
            let w = u16::from_le_bytes([data[6], data[7]]) as u32 & 0x3FFF;
            let h = u16::from_le_bytes([data[8], data[9]]) as u32 & 0x3FFF;
            return Some((w, h));
        }
        if fc == "VP8L" && data.len() >= 5 {
            // VP8L: signature 0x2F, then 14-bit width, 14-bit height packed LE
            if data[0] != 0x2F {
                continue;
            }
            let b1 = data[1] as u32;
            let b2 = data[2] as u32;
            let b3 = data[3] as u32;
            let b4 = data[4] as u32;
            let w = (b1 | ((b2 & 0x3F) << 8)) + 1;
            let h = ((b2 >> 6) | (b3 << 2) | ((b4 & 0x0F) << 10)) + 1;
            return Some((w, h));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_xmp_with_subject() -> Vec<u8> {
        r#"<?xml version="1.0"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
      xmlns:dc="http://purl.org/dc/elements/1.1/"
      xmlns:xmp="http://ns.adobe.com/xap/1.0/">
      <dc:subject>
        <rdf:Bag>
          <rdf:li>Existing</rdf:li>
        </rdf:Bag>
      </dc:subject>
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>"#
            .as_bytes()
            .to_vec()
    }

    #[test]
    fn add_tag_to_existing_bag() {
        let xmp = sample_xmp_with_subject();
        let (out, tags) = edit_xmp_packet(&xmp, &TagOp::Add("#原神".to_string())).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("<rdf:li>Existing</rdf:li>"), "{s}");
        assert!(s.contains("<rdf:li>#原神</rdf:li>"), "{s}");
        assert!(tags.contains("Existing"));
        assert!(tags.contains("#原神"));
    }

    #[test]
    fn add_duplicate_tag_is_noop() {
        let xmp = sample_xmp_with_subject();
        let (out, _) = edit_xmp_packet(&xmp, &TagOp::Add("Existing".to_string())).unwrap();
        let s = String::from_utf8(out).unwrap();
        // 1 個しかないはず
        assert_eq!(s.matches("<rdf:li>Existing</rdf:li>").count(), 1);
    }

    #[test]
    fn remove_tag() {
        let xmp = sample_xmp_with_subject();
        let (out, tags) =
            edit_xmp_packet(&xmp, &TagOp::Remove("Existing".to_string())).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(!s.contains("<rdf:li>Existing</rdf:li>"));
        assert!(!tags.contains("Existing"));
    }

    #[test]
    fn clear_miv_preserves_non_hash() {
        let xmp = r#"<?xml version="1.0"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
      xmlns:dc="http://purl.org/dc/elements/1.1/"
      xmlns:xmp="http://ns.adobe.com/xap/1.0/">
      <dc:subject>
        <rdf:Bag>
          <rdf:li>Photographer</rdf:li>
          <rdf:li>#原神</rdf:li>
          <rdf:li>#風景</rdf:li>
        </rdf:Bag>
      </dc:subject>
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>"#
            .as_bytes();
        let (out, tags) = edit_xmp_packet(xmp, &TagOp::ClearMiv).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("<rdf:li>Photographer</rdf:li>"));
        assert!(!s.contains("#原神"));
        assert!(!s.contains("#風景"));
        assert!(tags.contains("Photographer"));
        assert!(!tags.contains('#'));
    }

    #[test]
    fn metadata_date_cleans_up_accumulated_duplicates() {
        // 過去 bug の犠牲ファイル: <xmp:MetadataDate> が 6 個累積している。
        // 1 回 edit_xmp_packet を通したら 1 個だけに整理されること。
        let xmp = r#"<?xpacket begin='' id='W5M0MpCehiHzreSzNTczkc9d'?>
<x:xmpmeta xmlns:x='adobe:ns:meta/'>
<rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
 <rdf:Description rdf:about=''
  xmlns:dc='http://purl.org/dc/elements/1.1/'>
    <dc:subject><rdf:Bag><rdf:li>#x</rdf:li></rdf:Bag></dc:subject>
    <xmp:MetadataDate xmlns:xmp="http://ns.adobe.com/xap/1.0/">2026-04-22T05:00:16+00:00</xmp:MetadataDate>
      <xmp:MetadataDate xmlns:xmp="http://ns.adobe.com/xap/1.0/">2026-04-22T05:04:15+00:00</xmp:MetadataDate>
      <xmp:MetadataDate xmlns:xmp="http://ns.adobe.com/xap/1.0/">2026-04-22T05:04:15+00:00</xmp:MetadataDate>
      <xmp:MetadataDate xmlns:xmp="http://ns.adobe.com/xap/1.0/">2026-04-22T05:04:15+00:00</xmp:MetadataDate>
      <xmp:MetadataDate xmlns:xmp="http://ns.adobe.com/xap/1.0/">2026-04-22T05:04:15+00:00</xmp:MetadataDate>
      <xmp:MetadataDate xmlns:xmp="http://ns.adobe.com/xap/1.0/">2026-04-22T05:08:18+00:00</xmp:MetadataDate>
    </rdf:Description>
 <rdf:Description rdf:about=''
  xmlns:xtw='https://example/'>
    <xtw:Marker>preserved</xtw:Marker>
 </rdf:Description>
</rdf:RDF>
</x:xmpmeta>
<?xpacket end='w'?>"#;
        let (out, _) = edit_xmp_packet(xmp.as_bytes(), &TagOp::Add("#y".to_string())).unwrap();
        let s = String::from_utf8(out).unwrap();
        let count = s.matches("<xmp:MetadataDate").count();
        assert_eq!(count, 1, "重複 MetadataDate が 1 個に整理されること: {s}");
        // 隣接していない他要素 (xtw:Marker) は壊さない
        assert!(s.contains("<xtw:Marker>preserved</xtw:Marker>"));
    }

    #[test]
    fn metadata_date_replaces_existing_with_attributes_no_duplication() {
        // Codex/User regression: 我々が挿入する <xmp:MetadataDate xmlns:xmp="..."> を、
        // 次回の更新時に正しく「既存」と認識して置換すること (毎回追記すると mxd ファイルで
        // MetadataDate が雪だるま式に増える)。
        let xmp = r#"<?xpacket begin='' id='W5M0MpCehiHzreSzNTczkc9d'?>
<x:xmpmeta xmlns:x='adobe:ns:meta/'>
<rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
 <rdf:Description rdf:about=''
  xmlns:dc='http://purl.org/dc/elements/1.1/'>
    <dc:subject><rdf:Bag><rdf:li>#existing</rdf:li></rdf:Bag></dc:subject>
    <xmp:MetadataDate xmlns:xmp="http://ns.adobe.com/xap/1.0/">2026-01-01T00:00:00+00:00</xmp:MetadataDate>
 </rdf:Description>
</rdf:RDF>
</x:xmpmeta>
<?xpacket end='w'?>"#;
        // 1 回目: タグ追加 (既存 MetadataDate の差し替えのみ起きるはず)
        let (out, _) = edit_xmp_packet(xmp.as_bytes(), &TagOp::Add("#new".to_string())).unwrap();
        let s = String::from_utf8(out.clone()).unwrap();
        let count = s.matches("<xmp:MetadataDate").count();
        assert_eq!(
            count, 1,
            "1 回目の write 後、MetadataDate は 1 個に保たれること: {s}"
        );

        // 2 回目: タグ削除 (再度 MetadataDate を更新)
        let (out2, _) = edit_xmp_packet(&out, &TagOp::Remove("#new".to_string())).unwrap();
        let s2 = String::from_utf8(out2.clone()).unwrap();
        let count2 = s2.matches("<xmp:MetadataDate").count();
        assert_eq!(
            count2, 1,
            "2 回目の write 後も MetadataDate は 1 個 (累積しない): {s2}"
        );

        // 3 回目: 再度追加。トグル繰り返しで累積しないこと
        let (out3, _) = edit_xmp_packet(&out2, &TagOp::Add("#new".to_string())).unwrap();
        let s3 = String::from_utf8(out3).unwrap();
        let count3 = s3.matches("<xmp:MetadataDate").count();
        assert_eq!(count3, 1, "3 回目: {s3}");
    }

    #[test]
    fn metadata_date_declares_xmp_namespace_on_insertion() {
        // mxd は xmlns:xmp を rdf:Description に載せない XMP を書き出す。そこに我々が
        // <xmp:MetadataDate> を追記しても、要素側で xmlns:xmp を宣言するので
        // 結果の XML は quick-xml (NsReader) で正常に解釈できる。
        let xmp = r#"<?xpacket begin='' id='W5M0MpCehiHzreSzNTczkc9d'?>
<x:xmpmeta xmlns:x='adobe:ns:meta/'>
<rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
 <rdf:Description rdf:about=''
  xmlns:dc='http://purl.org/dc/elements/1.1/'>
  <dc:creator><rdf:Seq><rdf:li>X</rdf:li></rdf:Seq></dc:creator>
 </rdf:Description>
</rdf:RDF>
</x:xmpmeta>
<?xpacket end='w'?>"#;
        let (out, _) = edit_xmp_packet(xmp.as_bytes(), &TagOp::Add("#ドール".to_string())).unwrap();
        let s = String::from_utf8(out.clone()).unwrap();
        // xmp:MetadataDate 要素に xmlns:xmp が載っていることを確認
        assert!(
            s.contains(r#"<xmp:MetadataDate xmlns:xmp="http://ns.adobe.com/xap/1.0/">"#),
            "insertion must self-declare xmlns:xmp. got: {s}"
        );
        // 書き出し後の XMP が我々の reader で dc:subject を取り出せる
        let tags = crate::xmp_reader::read_dc_subject_from_bytes(&wrap_bytes_as_jpeg(&out));
        assert_eq!(tags, vec!["#ドール"]);
    }

    /// XMP パケットを最小 JPEG でくるむ (read_dc_subject_from_bytes はコンテナ必須)。
    fn wrap_bytes_as_jpeg(xmp: &[u8]) -> Vec<u8> {
        let xmp_id = b"http://ns.adobe.com/xap/1.0/\0";
        let payload: Vec<u8> = xmp_id.iter().chain(xmp.iter()).copied().collect();
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(&[0xFF, 0xD8]);
        out.extend_from_slice(&[0xFF, 0xE1]);
        let seg_len = (payload.len() + 2) as u16;
        out.extend_from_slice(&seg_len.to_be_bytes());
        out.extend_from_slice(&payload);
        out.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0xFF, 0xD9]);
        out
    }

    #[test]
    fn add_to_xmp_without_dc_subject() {
        let xmp = r#"<?xml version="1.0"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
      xmlns:dc="http://purl.org/dc/elements/1.1/"
      xmlns:xmp="http://ns.adobe.com/xap/1.0/">
      <dc:creator><rdf:Seq><rdf:li>Someone</rdf:li></rdf:Seq></dc:creator>
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>"#
            .as_bytes();
        let (out, _) = edit_xmp_packet(xmp, &TagOp::Add("#new".to_string())).unwrap();
        let s = String::from_utf8(out).unwrap();
        // dc:creator は保持、dc:subject が新規挿入される
        assert!(s.contains("<dc:creator>"));
        assert!(s.contains("<dc:subject>"));
        assert!(s.contains("<rdf:li>#new</rdf:li>"));
    }

    #[test]
    fn add_to_self_closing_description() {
        let xmp = r#"<?xml version="1.0"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
      xmlns:dc="http://purl.org/dc/elements/1.1/"
      xmlns:xmp="http://ns.adobe.com/xap/1.0/"/>
  </rdf:RDF>
</x:xmpmeta>"#
            .as_bytes();
        let (out, _) = edit_xmp_packet(xmp, &TagOp::Add("#new".to_string())).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("<dc:subject>"));
        assert!(s.contains("</rdf:Description>"));
        assert!(s.contains("<rdf:li>#new</rdf:li>"));
    }

    #[test]
    fn png_crc_matches_known_value() {
        // 既知値: "IEND" チャンクの CRC は 0xAE426082 (データ長 0)
        assert_eq!(png_crc32(b"IEND"), 0xAE426082);
    }

    #[test]
    fn minimal_webp_roundtrip_add_tag() {
        // 最小 VP8L WebP (1x1 緑) にタグ追加 → XMP チャンクが挿入される
        let webp = minimal_webp_vp8l();
        let (out, tags) = apply_tag_op_webp(&webp, &TagOp::Add("#web".to_string())).unwrap();
        assert!(out.starts_with(b"RIFF"));
        assert_eq!(&out[8..12], b"WEBP");
        assert!(find_subsequence(&out, b"XMP ").is_some(), "XMP chunk present");
        assert!(find_subsequence(&out, b"VP8X").is_some(), "VP8X added");
        assert!(find_subsequence(&out, b"<rdf:li>#web</rdf:li>").is_some());
        assert!(tags.contains("#web"));
        // RIFF サイズが実ファイルサイズ - 8 と一致
        let riff_size = u32::from_le_bytes([out[4], out[5], out[6], out[7]]) as usize;
        assert_eq!(riff_size, out.len() - 8);
    }

    /// VP8L で 1x1 の最小 WebP (VP8X なし、つまり simple WebP)。
    /// 書き込み時に VP8X に自動昇格する挙動を検証する。
    fn minimal_webp_vp8l() -> Vec<u8> {
        // VP8L header: 0x2F signature + 5 bytes size (width-1, height-1, alpha, version)
        // w=1 h=1 → w-1=0, h-1=0。flags = 0, version = 0
        let vp8l_data: Vec<u8> = vec![0x2F, 0x00, 0x00, 0x00, 0x00, 0x00]; // 最小ペイロード
        let chunk_size = vp8l_data.len() as u32;
        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(b"WEBP");
        body.extend_from_slice(b"VP8L");
        body.extend_from_slice(&chunk_size.to_le_bytes());
        body.extend_from_slice(&vp8l_data);
        // pad to even
        if vp8l_data.len() & 1 == 1 {
            body.push(0);
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn minimal_png_roundtrip_add_tag() {
        // 1x1 黒 PNG (最小構成) に対してタグ追加
        let png = minimal_png_1x1();
        let (out, tags) = apply_tag_op_png(&png, &TagOp::Add("#test".to_string())).unwrap();
        // 読み戻し確認: out の中に iTXt チャンクと dc:subject がある
        assert!(find_subsequence(&out, b"iTXt").is_some());
        assert!(find_subsequence(&out, b"<rdf:li>#test</rdf:li>").is_some());
        assert!(tags.contains("#test"));
        // PNG 署名が先頭に残っている
        assert!(out.starts_with(PNG_SIG));
        // IEND が末尾にある
        assert!(find_subsequence(&out, b"IEND").is_some());
    }

    #[test]
    fn minimal_jpeg_roundtrip_add_tag() {
        let jpg = minimal_jpeg();
        let (out, tags) = apply_tag_op_jpeg(&jpg, &TagOp::Add("#hello".to_string())).unwrap();
        assert!(out.starts_with(&[0xFF, 0xD8])); // SOI
        assert!(find_subsequence(&out, b"<rdf:li>#hello</rdf:li>").is_some());
        assert!(tags.contains("#hello"));
    }

    #[test]
    fn jpeg_preserves_extended_xmp_bytes() {
        // Extended XMP APP1 を 2 つ持つ JPEG を作り、タグ追加後も Extended APP1 が
        // バイト単位で保持されていることを確認する (mXDownloader ツイート画像の
        // 重要な回帰テスト)。
        let jpg = jpeg_with_extended_xmp();
        let before_ext = extract_extended_xmp_bytes(&jpg);
        assert!(
            !before_ext.is_empty(),
            "setup: Extended XMP が元ファイルに含まれる"
        );
        let (out, _) = apply_tag_op_jpeg(&jpg, &TagOp::Add("#kept".to_string())).unwrap();
        let after_ext = extract_extended_xmp_bytes(&out);
        assert_eq!(
            before_ext, after_ext,
            "Extended XMP バイト列は書き換え後もそのまま"
        );
    }

    // ---------- test helpers ----------

    /// 最小の 1x1 PNG (非圧縮 IHDR + 1 バイトの IDAT + IEND)。
    fn minimal_png_1x1() -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(PNG_SIG);
        // IHDR: width=1, height=1, bit_depth=8, color_type=0 (grayscale), compression=0, filter=0, interlace=0
        let ihdr_data: Vec<u8> = vec![0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0];
        push_png_chunk(&mut out, b"IHDR", &ihdr_data);
        // IDAT: minimal zlib empty stream (実際のデコーダは通らないが、パースには十分)
        let idat_data: Vec<u8> = vec![0x78, 0x9C, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
        push_png_chunk(&mut out, b"IDAT", &idat_data);
        // IEND (empty)
        push_png_chunk(&mut out, b"IEND", &[]);
        out
    }

    fn push_png_chunk(out: &mut Vec<u8>, ty: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(ty);
        out.extend_from_slice(data);
        let crc_input: Vec<u8> = ty.iter().chain(data.iter()).copied().collect();
        out.extend_from_slice(&png_crc32(&crc_input).to_be_bytes());
    }

    /// SOI + 最小 SOF0 + SOS + EOI (XMP なし) の最小 JPEG。
    fn minimal_jpeg() -> Vec<u8> {
        let mut out: Vec<u8> = vec![0xFF, 0xD8]; // SOI
        // SOF0 (0xC0): length=11, precision=8, height=1, width=1, 1 component {id=1, sampling=0x11, q_table=0}
        out.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0x11, 0x00]);
        // SOS (0xDA): length=8, 1 component, id=1, huff=0x00, Ss=0, Se=63, Ah/Al=0
        out.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]);
        // 画像データ (無くても形式上 OK)、EOI
        out.extend_from_slice(&[0xFF, 0xD9]);
        out
    }

    fn jpeg_with_extended_xmp() -> Vec<u8> {
        // SOI + APP1 Standard XMP (rdf:Description + xmpNote:HasExtendedXMP の再現) +
        // APP1 Extended XMP (ペイロード 100 バイト) + SOF0 + SOS + EOI
        let std_xmp_body = br#"<?xml version="1.0"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
      xmlns:dc="http://purl.org/dc/elements/1.1/"
      xmlns:xmp="http://ns.adobe.com/xap/1.0/"
      xmlns:xmpNote="http://ns.adobe.com/xmp/note/"
      xmpNote:HasExtendedXMP="AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA">
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>"#.as_slice();
        let mut out: Vec<u8> = vec![0xFF, 0xD8];

        // Standard XMP APP1
        let std_payload: Vec<u8> = JPEG_XMP_ID
            .iter()
            .chain(std_xmp_body.iter())
            .copied()
            .collect();
        push_jpeg_app1(&mut out, &std_payload);

        // Extended XMP APP1 (GUID + size + offset + data のダミー)
        let mut ext_payload: Vec<u8> = Vec::new();
        ext_payload.extend_from_slice(JPEG_EXT_XMP_ID);
        ext_payload.extend_from_slice(&[b'A'; 32]); // GUID (32 hex chars)
        ext_payload.extend_from_slice(&100u32.to_be_bytes()); // total size
        ext_payload.extend_from_slice(&0u32.to_be_bytes()); // offset
        ext_payload.extend_from_slice(&[0xEE; 100]); // data
        push_jpeg_app1(&mut out, &ext_payload);

        // SOF0 + SOS + EOI (minimal_jpeg と同じ)
        out.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0x11, 0x00]);
        out.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]);
        out.extend_from_slice(&[0xFF, 0xD9]);
        out
    }

    fn push_jpeg_app1(out: &mut Vec<u8>, payload: &[u8]) {
        out.extend_from_slice(&[0xFF, 0xE1]);
        let seg_len = (payload.len() + 2) as u16;
        out.extend_from_slice(&seg_len.to_be_bytes());
        out.extend_from_slice(payload);
    }

    /// JPEG から Extended XMP APP1 をすべて抽出して連結した bytes を返す。
    /// Standard XMP は含まない。
    fn extract_extended_xmp_bytes(bytes: &[u8]) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        let mut pos = 2;
        while pos + 4 <= bytes.len() {
            if bytes[pos] != 0xFF {
                break;
            }
            let marker = bytes[pos + 1];
            if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
                pos += 2;
                continue;
            }
            if marker == 0xDA {
                break;
            }
            let seg_len = u16::from_be_bytes([bytes[pos + 2], bytes[pos + 3]]) as usize;
            let seg_end = pos + 2 + seg_len;
            if seg_end > bytes.len() {
                break;
            }
            let payload = &bytes[pos + 4..seg_end];
            if marker == 0xE1 && payload.starts_with(JPEG_EXT_XMP_ID) {
                out.extend_from_slice(&bytes[pos..seg_end]);
            }
            pos = seg_end;
        }
        out
    }

    // ---- xmp:Rating 書き込み ----

    #[test]
    fn edit_rating_writes_attribute_on_empty_packet() {
        let (out, _) = edit_xmp_packet(&[], &TagOp::Add("#x".into())).expect("seed");
        let updated = edit_xmp_packet_rating(&out, Some(4)).expect("write rating");
        let s = std::str::from_utf8(&updated).unwrap();
        assert!(s.contains("xmp:Rating=\"4\""));
        assert_eq!(crate::xmp_reader::parse_xmp_rating(&updated), Some(4));
        // タグは保持
        assert_eq!(crate::xmp_reader::parse_dc_subject(&updated), vec!["#x"]);
    }

    #[test]
    fn edit_rating_overwrites_existing() {
        let packet = edit_xmp_packet_rating(&[], Some(3)).expect("initial");
        let updated = edit_xmp_packet_rating(&packet, Some(5)).expect("update");
        assert_eq!(crate::xmp_reader::parse_xmp_rating(&updated), Some(5));
        // 同じ属性が重複していない
        let count = std::str::from_utf8(&updated)
            .unwrap()
            .matches("xmp:Rating=")
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn edit_rating_none_removes_attribute() {
        let packet = edit_xmp_packet_rating(&[], Some(3)).expect("initial");
        let cleared = edit_xmp_packet_rating(&packet, None).expect("clear");
        assert_eq!(crate::xmp_reader::parse_xmp_rating(&cleared), None);
        assert!(!std::str::from_utf8(&cleared).unwrap().contains("xmp:Rating="));
    }

    #[test]
    fn edit_rating_zero_is_same_as_none() {
        let packet = edit_xmp_packet_rating(&[], Some(4)).expect("initial");
        let cleared = edit_xmp_packet_rating(&packet, Some(0)).expect("clear via 0");
        assert_eq!(crate::xmp_reader::parse_xmp_rating(&cleared), None);
    }

    #[test]
    fn edit_rating_noop_when_unchanged() {
        let packet = edit_xmp_packet_rating(&[], Some(3)).expect("initial");
        // 同じ値で再実行 → MetadataDate すら更新しない (早期 return で同一バイト列)
        let updated = edit_xmp_packet_rating(&packet, Some(3)).expect("noop");
        assert_eq!(packet, updated);
    }
}
