//! 単枚・一括の転記と縮小画像の検出。RGB ICC は共通、EXIF は profile ごとの扱いを保つ。
use super::*;
use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Write};

const XMP_IMAGE_NS: &[u8] = b"http://ns.adobe.com/xap/1.0/g/img/";
const XMP_STANDARD: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const XMP_EXTENDED: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
const XMP_KEY: &[u8] = b"XML:com.adobe.xmp";
const CHUNK_LIMIT: usize = 16 * 1024 * 1024;
const FILE_LIMIT: usize = 32 * 1024 * 1024;
const RAW_APP1_PREFIX_LIMIT: usize = 1024;

fn has_thumbnail(bytes: &[u8]) -> bool {
    bytes.windows(XMP_IMAGE_NS.len()).any(|s| s == XMP_IMAGE_NS)
}

fn rgb_icc(bytes: &[u8]) -> bool {
    bytes.get(16..20) == Some(b"RGB ")
}

pub(super) fn transfer(
    encoded: &[u8],
    src: &[u8],
    format: &SrcFormat,
    options: &SaveOptions,
) -> Result<(Vec<u8>, ThumbnailNoticeKinds), SaveError> {
    let batch = options.metadata_profile == MetadataProfile::Batch;
    let mut notice = ThumbnailNoticeKinds::default();
    let bytes = match format {
        SrcFormat::Jpeg => {
            let mut segments = extract_jpeg_app1_segments(src)?;
            segments.retain_mut(|seg| {
                if seg[4..].starts_with(b"Exif\0\0") {
                    let Some(tiff) = transfer_exif(&seg[10..], options, &mut notice) else {
                        return false;
                    };
                    seg[10..].copy_from_slice(&tiff);
                }
                if !batch && options.caller_applied_orientation {
                    reset_exif_orientation_in_app1(seg);
                }
                true
            });
            notice.xmp = jpeg_xmp_thumbnail(&segments);
            segments.extend(jpeg_rgb_icc_segments(src)?);
            splice_jpeg_app1_segments(encoded, &segments)?
        }
        SrcFormat::Png => {
            let chunks = extract_png_metadata_chunks(src)?;
            let mut total = 0;
            let mut retained = Vec::new();
            let mut raw_app1_xmp = Vec::new();
            for mut chunk in chunks {
                let kind = chunk[4..8].to_vec();
                match kind.as_slice() {
                    b"eXIf" => {
                        if !batch {
                            continue;
                        }
                        let Some((tiff, maker)) = clean_exif(&chunk[8..chunk.len() - 4], true)
                        else {
                            continue;
                        };
                        chunk = png_chunk(b"eXIf", &tiff);
                        notice.maker_note |= maker && options.thumbnail_notice_for_hidden_edits;
                    }
                    b"iCCP" => {
                        let data = &chunk[8..chunk.len() - 4];
                        let Some(end) = data.iter().position(|&b| b == 0) else {
                            continue;
                        };
                        if data.get(end + 1) != Some(&0) {
                            continue;
                        }
                        let mut header = [0; 20];
                        if flate2::read::ZlibDecoder::new(&data[end + 2..])
                            .read_exact(&mut header)
                            .is_err()
                            || !rgb_icc(&header)
                        {
                            continue;
                        }
                    }
                    _ => {
                        if !process_png_text::<CHUNK_LIMIT, FILE_LIMIT>(
                            &mut chunk,
                            &mut total,
                            options,
                            &mut notice,
                            &mut raw_app1_xmp,
                        )? {
                            continue;
                        }
                    }
                }
                retained.push(chunk);
            }
            notice.xmp |= xmp_payloads_thumbnail(raw_app1_xmp.iter().map(Vec::as_slice));
            splice_png_text_chunks(encoded, &retained)?
        }
        SrcFormat::Webp => {
            let (icc, exif, xmp) = extract_webp_metadata_chunks(src)?;
            let exif = exif.and_then(|payload| {
                let prefix = if payload.starts_with(b"Exif\0\0") {
                    6
                } else {
                    0
                };
                let tiff = transfer_exif(&payload[prefix..], options, &mut notice)?;
                let mut result = payload[..prefix].to_vec();
                result.extend(tiff);
                Some(result)
            });
            notice.xmp = xmp.as_deref().is_some_and(has_thumbnail);
            let icc = icc.filter(|p| rgb_icc(p));
            if icc.is_some() || exif.is_some() || xmp.is_some() {
                inject_webp_metadata(encoded, icc, exif, xmp)?
            } else {
                encoded.to_vec()
            }
        }
        SrcFormat::Other(_) => unreachable!(),
    };
    Ok((bytes, notice))
}

fn jpeg_xmp_thumbnail(segments: &[Vec<u8>]) -> bool {
    xmp_payloads_thumbnail(segments.iter().map(|seg| &seg[4..]))
}

// JPEG APP1 と PNG Raw APP1 は同じ見出し・断片の結合規則を使う。
fn xmp_payloads_thumbnail<'a>(payloads: impl IntoIterator<Item = &'a [u8]>) -> bool {
    let mut groups: BTreeMap<&[u8], Vec<(u32, &[u8])>> = BTreeMap::new();
    for payload in payloads {
        if let Some(body) = payload.strip_prefix(XMP_STANDARD) {
            if has_thumbnail(body) {
                return true;
            }
        } else if let Some(body) = payload.strip_prefix(XMP_EXTENDED) {
            if body.len() < 40 {
                if has_thumbnail(body) {
                    return true;
                }
            } else {
                let offset = u32::from_be_bytes(body[36..40].try_into().unwrap());
                groups
                    .entry(&body[..32])
                    .or_default()
                    .push((offset, &body[40..]));
            }
        }
    }
    groups.values_mut().any(|fragments| {
        fragments.sort_by_key(|(offset, _)| *offset);
        // 実在する本体だけ。見出しの全体長で確保しない。
        let joined: Vec<u8> = fragments
            .iter()
            .flat_map(|(_, data)| data.iter().copied())
            .collect();
        has_thumbnail(&joined)
    })
}

fn jpeg_rgb_icc_segments(src: &[u8]) -> Result<Vec<Vec<u8>>, SaveError> {
    // APP1 抽出と同じ検証を APP2 に適用する。元のバイト列は変更しない。
    let segments = extract_jpeg_segments(src, 0xE2)?;
    let mut icc: Vec<_> = segments
        .into_iter()
        .filter(|s| s[4..].starts_with(b"ICC_PROFILE\0") && s.len() >= 18)
        .collect();
    icc.sort_by_key(|s| s[16]);
    let Some(first) = icc.first() else {
        return Ok(Vec::new());
    };
    let count = first[17] as usize;
    if count == 0
        || icc.len() != count
        || icc
            .iter()
            .enumerate()
            .any(|(i, s)| s[16] as usize != i + 1 || s[17] as usize != count)
    {
        return Ok(Vec::new());
    }
    let header: Vec<u8> = icc
        .iter()
        .flat_map(|s| s[18..].iter().copied())
        .take(20)
        .collect();
    Ok(if rgb_icc(&header) { icc } else { Vec::new() })
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut chunk = Vec::with_capacity(data.len() + 12);
    chunk.extend_from_slice(&(data.len() as u32).to_be_bytes());
    chunk.extend_from_slice(kind);
    chunk.extend_from_slice(data);
    let crc = png_crc32_compute(&chunk[4..]);
    chunk.extend_from_slice(&crc.to_be_bytes());
    chunk
}

// 読めない Single の EXIF は従来どおり残す。Batch だけが落とす。
fn transfer_exif(
    data: &[u8],
    options: &SaveOptions,
    notice: &mut ThumbnailNoticeKinds,
) -> Option<Vec<u8>> {
    let batch = options.metadata_profile == MetadataProfile::Batch;
    if let Some((tiff, maker)) = clean_exif(data, batch) {
        notice.maker_note |= maker && options.thumbnail_notice_for_hidden_edits;
        Some(tiff)
    } else if batch {
        None
    } else {
        notice.exif_uninspectable = true;
        Some(data.to_vec())
    }
}

// 本文の開始位置を保持し、iTXt の言語・訳語と圧縮フラグを再利用する。
fn png_text_layout(chunk: &[u8], key_end: usize) -> Option<(usize, bool)> {
    let data = &chunk[8..chunk.len() - 4];
    let start = key_end + 1;
    match &chunk[4..8] {
        b"tEXt" => Some((start, false)),
        b"zTXt" if data.get(start) == Some(&0) => Some((start + 1, true)),
        b"iTXt" => {
            let flag = *data.get(start)?;
            if flag > 1 || data.get(start + 1) != Some(&0) {
                return None;
            }
            let mut pos = start + 2;
            for _ in 0..2 {
                pos += data.get(pos..)?.iter().position(|&b| b == 0)? + 1;
            }
            Some((pos, flag == 1))
        }
        _ => None,
    }
}

fn decode_png_text<const CHUNK: usize, const FILE: usize>(
    content: &[u8],
    compressed: bool,
    total: &mut usize,
) -> Option<Vec<u8>> {
    let limit = CHUNK.min(FILE.saturating_sub(*total));
    if limit == 0 {
        return None;
    }
    if compressed {
        let mut decoded = Vec::new();
        let result = flate2::read::ZlibDecoder::new(content)
            .take((limit + 1) as u64)
            .read_to_end(&mut decoded);
        *total += decoded.len().min(limit);
        if result.is_err() || decoded.len() > limit {
            return None;
        }
        Some(decoded)
    } else {
        *total += content.len().min(limit);
        (content.len() <= limit).then(|| content.to_vec())
    }
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// 長さの行と16進の開始位置。長さの妥当性は厳密な読み取り側で調べる。
fn raw_profile_layout(body: &[u8]) -> Option<(&[u8], usize)> {
    let body_start = if body.starts_with(b"\r\n") {
        2
    } else if body.starts_with(b"\n") {
        1
    } else {
        return None;
    };
    let type_end = body[body_start..].iter().position(|&b| b == b'\n')? + body_start;
    if body[body_start..type_end]
        .iter()
        .all(u8::is_ascii_whitespace)
    {
        return None;
    }
    let length_start = type_end + 1;
    let length_end = body[length_start..].iter().position(|&b| b == b'\n')? + length_start;
    Some((&body[length_start..length_end], length_end + 1))
}

// ImageMagick: 改行・種類名・改行・10進長・改行・空白区切りの16進。
// 宣言より後ろのデータも含めて、桁数が厳密に一致することを確認する。
fn raw_profile(body: &[u8]) -> Option<(Vec<u8>, Vec<usize>)> {
    let (length_line, hex_start) = raw_profile_layout(body)?;
    let length_text = std::str::from_utf8(length_line).ok()?.trim();
    if length_text.is_empty() || !length_text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let length: usize = length_text.parse().ok()?;
    let digits = length.checked_mul(2)?;
    let positions: Vec<usize> = (hex_start..body.len())
        .filter(|&i| !body[i].is_ascii_whitespace())
        .collect();
    if positions.len() != digits {
        return None;
    }
    let bytes = positions
        .chunks_exact(2)
        .map(|p| Some((hex_digit(body[p[0]])? << 4) | hex_digit(body[p[1]])?))
        .collect::<Option<Vec<_>>>()?;
    Some((bytes, positions))
}

// 厳密な解析に失敗した APP1 の分類専用。長さの宣言は信用せず、
// XMP の見出しに必要な先頭の数字だけ戻す。本体の検査・転記には使わない。
fn raw_profile_has_xmp_header(body: &[u8]) -> bool {
    let Some((_, hex_start)) = raw_profile_layout(body) else {
        return false;
    };
    let mut digits = body[hex_start..]
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace());
    let header_len = XMP_STANDARD.len().max(XMP_EXTENDED.len());
    let mut prefix = Vec::with_capacity(header_len);
    for _ in 0..header_len {
        let Some(high) = digits.next().and_then(hex_digit) else {
            return false;
        };
        let Some(low) = digits.next().and_then(hex_digit) else {
            return false;
        };
        prefix.push((high << 4) | low);
        if prefix.as_slice() == XMP_STANDARD || prefix.as_slice() == XMP_EXTENDED {
            return true;
        }
        if !XMP_STANDARD.starts_with(&prefix) && !XMP_EXTENDED.starts_with(&prefix) {
            return false;
        }
    }
    false
}

// 本文検査の 16/32 MiB の枠とは別に、分類だけに最大 1 KiB を使う。
// 合計残量は増減させず、枠が尽きた後も XMP と分かるものは保持して警告する。
// 分類時だけ zlib の 2 バイトのヘッダを飛ばし、raw deflate として読む。
// Adler-32 の不一致で先頭まで失わないよう、末尾のチェックサムは検証しない。
// 本文検査は従来どおり ZlibDecoder を使い、この先頭は本文の検査や書き換えには使わない。
fn raw_app1_prefix_has_xmp_header(content: &[u8], compressed: bool) -> bool {
    if !compressed {
        return raw_profile_has_xmp_header(&content[..content.len().min(RAW_APP1_PREFIX_LIMIT)]);
    }
    let Some(deflate) = content.get(2..) else {
        return false;
    };
    let mut prefix = Vec::new();
    let _ = flate2::read::DeflateDecoder::new(deflate)
        .take(RAW_APP1_PREFIX_LIMIT as u64)
        .read_to_end(&mut prefix);
    raw_profile_has_xmp_header(&prefix)
}

fn keep_uninspectable_png_text(
    raw_exif: bool,
    options: &SaveOptions,
    notice: &mut ThumbnailNoticeKinds,
) -> bool {
    if raw_exif {
        if options.metadata_profile == MetadataProfile::Batch {
            return false;
        }
        notice.exif_uninspectable = true;
    } else {
        notice.xmp_uninspectable = true;
    }
    true
}

fn patch_raw_profile(body: &mut [u8], positions: &[usize], original: &[u8], cleaned: &[u8]) {
    for (i, (&old, &new)) in original.iter().zip(cleaned).enumerate() {
        if old == new {
            continue;
        }
        for (pos, nibble) in [
            (positions[i * 2], new >> 4),
            (positions[i * 2 + 1], new & 15),
        ] {
            if hex_digit(body[pos]) != Some(nibble) {
                body[pos] = if nibble < 10 {
                    b'0' + nibble
                } else if body[pos].is_ascii_uppercase() {
                    b'A' + nibble - 10
                } else {
                    b'a' + nibble - 10
                };
            }
        }
    }
}

// Raw EXIF は XMP 検出後も必ず処理する。上限は XML XMP と Raw profile で共有する。
// 製品の上限は呼び出し元で固定し、テストだけが小さい上限を指定する。
fn process_png_text<const CHUNK: usize, const FILE: usize>(
    chunk: &mut Vec<u8>,
    total: &mut usize,
    options: &SaveOptions,
    notice: &mut ThumbnailNoticeKinds,
    raw_app1_xmp: &mut Vec<Vec<u8>>,
) -> Result<bool, SaveError> {
    let data = &chunk[8..chunk.len() - 4];
    let Some(key_end) = data.iter().position(|&b| b == 0) else {
        return Ok(true);
    };
    let key = &data[..key_end];
    let raw_app1 = key.eq_ignore_ascii_case(b"Raw profile type APP1");
    let raw_exif = key.eq_ignore_ascii_case(b"Raw profile type exif") || raw_app1;
    let raw_xmp = key.eq_ignore_ascii_case(b"Raw profile type xmp");
    if !raw_exif && !raw_xmp && (key != XMP_KEY || notice.xmp) {
        return Ok(true);
    }
    let layout = png_text_layout(chunk, key_end);
    let decoded = layout.and_then(|(start, compressed)| {
        decode_png_text::<CHUNK, FILE>(&data[start..], compressed, total)
            .map(|body| (start, compressed, body))
    });
    let Some((start, compressed, mut body)) = decoded else {
        if raw_app1
            && layout.is_some_and(|(start, compressed)| {
                raw_app1_prefix_has_xmp_header(&data[start..], compressed)
            })
        {
            notice.xmp_uninspectable = true;
            return Ok(true);
        }
        return Ok(keep_uninspectable_png_text(raw_exif, options, notice));
    };
    if !raw_exif && !raw_xmp {
        notice.xmp |= has_thumbnail(&body);
        return Ok(true);
    }
    let (bytes, positions) = match raw_profile(&body) {
        Some(parsed) => parsed,
        None => {
            if raw_app1 && raw_profile_has_xmp_header(&body) {
                notice.xmp_uninspectable = true;
                return Ok(true);
            }
            return Ok(keep_uninspectable_png_text(raw_exif, options, notice));
        }
    };
    if raw_app1 && (bytes.starts_with(XMP_STANDARD) || bytes.starts_with(XMP_EXTENDED)) {
        notice.xmp |= xmp_payloads_thumbnail(std::iter::once(bytes.as_slice()));
        if bytes.starts_with(XMP_EXTENDED) {
            raw_app1_xmp.push(bytes);
        }
        return Ok(true);
    }
    if raw_xmp {
        notice.xmp |= has_thumbnail(&bytes);
        return Ok(true);
    }
    let prefix = usize::from(bytes.starts_with(b"Exif\0\0")) * 6;
    let Some(cleaned) = transfer_exif(&bytes[prefix..], options, notice) else {
        return Ok(false);
    };
    let mut payload = bytes[..prefix].to_vec();
    payload.extend(cleaned);
    if payload == bytes {
        return Ok(true);
    }
    patch_raw_profile(&mut body, &positions, &bytes, &payload);
    let mut replacement = data[..start].to_vec();
    if compressed {
        let encoded = (|| -> io::Result<Vec<u8>> {
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(&body)?;
            encoder.finish()
        })();
        replacement.extend(encoded.map_err(|err| {
            crate::logger::log(&format!("export raw profile compression: {err}"));
            SaveError::EncodingFailed(format!("PNG raw profile: {err}"))
        })?);
    } else {
        replacement.extend(body);
    }
    let kind: [u8; 4] = chunk[4..8].try_into().unwrap();
    *chunk = png_chunk(&kind, &replacement);
    Ok(true)
}

// TIFF の offset はすべて TIFF 本体から。無効な領域や循環は読めないと返す。
struct Tiff<'a> {
    data: &'a [u8],
    little: bool,
}
impl Tiff<'_> {
    fn u16(&self, pos: usize) -> Option<u16> {
        let b = self.data.get(pos..pos.checked_add(2)?)?.try_into().ok()?;
        Some(if self.little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }
    fn u32(&self, pos: usize) -> Option<u32> {
        let b = self.data.get(pos..pos.checked_add(4)?)?.try_into().ok()?;
        Some(if self.little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }
    fn ifd(&self, offset: usize) -> Option<(Vec<usize>, usize)> {
        if offset < 8 {
            return None;
        }
        let count = self.u16(offset)? as usize;
        let end = offset.checked_add(2)?.checked_add(count.checked_mul(12)?)?;
        self.u32(end)?;
        Some(((0..count).map(|i| offset + 2 + i * 12).collect(), end))
    }
    fn values(&self, entry: usize) -> Option<(u16, usize, std::ops::Range<usize>)> {
        let kind = self.u16(entry + 2)?;
        let unit = match kind {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 => 8,
            _ => return None,
        };
        let count = self.u32(entry + 4)? as usize;
        let len = count.checked_mul(unit)?;
        let start = if len <= 4 {
            entry + 8
        } else {
            self.u32(entry + 8)? as usize
        };
        let end = start.checked_add(len)?;
        self.data.get(start..end)?;
        Some((kind, count, start..end))
    }
    fn integers(&self, entry: usize) -> Option<Vec<usize>> {
        let (kind, count, range) = self.values(entry)?;
        match kind {
            3 => (0..count)
                .map(|i| self.u16(range.start + i * 2).map(usize::from))
                .collect(),
            4 | 13 => (0..count)
                .map(|i| self.u32(range.start + i * 4).map(|n| n as usize))
                .collect(),
            _ => None,
        }
    }
}

fn clean_exif(data: &[u8], reset_orientation: bool) -> Option<(Vec<u8>, bool)> {
    let little = match data.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let t = Tiff { data, little };
    if t.u16(2)? != 42 {
        return None;
    }
    let ifd0 = t.u32(4)? as usize;
    let (_, ifd0_next) = t.ifd(ifd0)?;
    let ifd1 = t.u32(ifd0_next)? as usize;
    let mut pending = vec![(ifd0, false)];
    let mut visited = HashSet::new();
    let mut structural = vec![0..8];
    let mut retained_values = Vec::new();
    let mut orientation = Vec::new();
    let mut thumbnail_ranges = Vec::new();
    let mut maker = false;
    while let Some((offset, exif_ifd)) = pending.pop() {
        if !visited.insert(offset) {
            return None;
        }
        let (entries, next) = t.ifd(offset)?;
        structural.push(offset..next + 4);
        let mut jpeg_offset = None;
        let mut jpeg_length = None;
        let mut strips = None;
        let mut lengths = None;
        let mut tiles = None;
        let mut tile_lengths = None;
        for entry in entries {
            let tag = t.u16(entry)?;
            // Single は不正な Orientation もそのまま残し、読めない理由にしない。
            if tag == 0x0112 && !reset_orientation {
                continue;
            }
            let (kind, count, range) = t.values(entry)?;
            if range.len() > 4 {
                retained_values.push(range);
            }
            if tag == 0x0112 {
                if kind != 3 || count != 1 {
                    return None;
                }
                orientation.push(entry + 8);
            }
            if exif_ifd && tag == 0x927C && count > 0 {
                maker = true;
            }
            if matches!(tag, 0x8769 | 0x8825 | 0xA005 | 0x014A) {
                if !matches!(kind, 4 | 13) {
                    return None;
                }
                for child in t.integers(entry)? {
                    if child != 0 {
                        pending.push((child, tag == 0x8769));
                    }
                }
            }
            if offset == ifd1 {
                match tag {
                    0x0201 => jpeg_offset = Some(t.integers(entry)?),
                    0x0202 => jpeg_length = Some(t.integers(entry)?),
                    0x0111 => strips = Some(t.integers(entry)?),
                    0x0117 => lengths = Some(t.integers(entry)?),
                    0x0144 => tiles = Some(t.integers(entry)?),
                    0x0145 => tile_lengths = Some(t.integers(entry)?),
                    _ => {}
                }
            }
        }
        if offset == ifd1 {
            for (offsets, sizes) in [
                (jpeg_offset, jpeg_length),
                (strips, lengths),
                (tiles, tile_lengths),
            ] {
                match (offsets, sizes) {
                    (None, None) => {}
                    (Some(offsets), Some(sizes)) if offsets.len() == sizes.len() => {
                        for (start, len) in offsets.into_iter().zip(sizes) {
                            let end = start.checked_add(len)?;
                            data.get(start..end)?;
                            if len > 0 {
                                thumbnail_ranges.push(start..end);
                            }
                        }
                    }
                    _ => return None,
                }
            }
        }
        let next_offset = t.u32(next)? as usize;
        if next_offset != 0 {
            pending.push((next_offset, false));
        }
    }
    for (i, range) in structural.iter().enumerate() {
        if structural[i + 1..]
            .iter()
            .chain(&retained_values)
            .any(|r| range.start < r.end && r.start < range.end)
        {
            return None;
        }
    }
    for range in &thumbnail_ranges {
        if structural
            .iter()
            .chain(&retained_values)
            .any(|r| range.start < r.end && r.start < range.end)
        {
            return None;
        }
    }
    let mut out = data.to_vec();
    for pos in orientation {
        out[pos..pos + 4].fill(0);
        out[pos + usize::from(!little)] = 1;
    }
    out[ifd0_next..ifd0_next + 4].fill(0);
    for range in thumbnail_ranges {
        out[range].fill(0);
    }
    Some((out, maker))
}

#[cfg(test)]
mod tests;
