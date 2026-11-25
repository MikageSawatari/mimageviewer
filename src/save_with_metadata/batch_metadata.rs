//! 一括エクスポート専用の転記。元の情報は書き直さず、EXIF の向きと IFD1 だけ処理する。
use super::*;
use std::collections::{BTreeMap, HashSet};
use std::io::Read;

const XMP_IMAGE_NS: &[u8] = b"http://ns.adobe.com/xap/1.0/g/img/";
const XMP_STANDARD: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const XMP_EXTENDED: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
const XMP_KEY: &[u8] = b"XML:com.adobe.xmp";
const CHUNK_LIMIT: usize = 16 * 1024 * 1024;
const FILE_LIMIT: usize = 32 * 1024 * 1024;

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
    let mut notice = ThumbnailNoticeKinds::default();
    let bytes = match format {
        SrcFormat::Jpeg => {
            let mut segments = extract_jpeg_app1_segments(src)?;
            segments.retain_mut(|seg| {
                if seg[4..].starts_with(b"Exif\0\0") {
                    let Some((tiff, maker)) = clean_exif(&seg[10..]) else {
                        return false;
                    };
                    seg[10..].copy_from_slice(&tiff);
                    notice.maker_note |= maker && options.thumbnail_notice_for_hidden_edits;
                }
                true
            });
            notice.xmp = jpeg_xmp_thumbnail(&segments);
            segments.extend(jpeg_rgb_icc_segments(src)?);
            splice_jpeg_app1_segments(encoded, &segments)?
        }
        SrcFormat::Png => {
            let mut chunks = extract_png_metadata_chunks(src)?;
            let mut total = 0;
            chunks.retain_mut(|chunk| {
                let kind = chunk[4..8].to_vec();
                match kind.as_slice() {
                    b"eXIf" => {
                        let Some((tiff, maker)) = clean_exif(&chunk[8..chunk.len() - 4]) else {
                            return false;
                        };
                        *chunk = png_chunk(b"eXIf", &tiff);
                        notice.maker_note |= maker && options.thumbnail_notice_for_hidden_edits;
                    }
                    b"iCCP" => {
                        let data = &chunk[8..chunk.len() - 4];
                        let Some(end) = data.iter().position(|&b| b == 0) else {
                            return false;
                        };
                        if data.get(end + 1) != Some(&0) {
                            return false;
                        }
                        let mut header = [0; 20];
                        if flate2::read::ZlibDecoder::new(&data[end + 2..])
                            .read_exact(&mut header)
                            .is_err()
                            || !rgb_icc(&header)
                        {
                            return false;
                        }
                    }
                    _ if !notice.xmp => inspect_png_xmp(chunk, &mut total, &mut notice),
                    _ => {}
                }
                true
            });
            splice_png_text_chunks(encoded, &chunks)?
        }
        SrcFormat::Webp => {
            let (icc, exif, xmp) = extract_webp_metadata_chunks(src)?;
            let exif = exif.and_then(|payload| {
                let prefix = if payload.starts_with(b"Exif\0\0") {
                    6
                } else {
                    0
                };
                let (tiff, maker) = clean_exif(&payload[prefix..])?;
                notice.maker_note |= maker && options.thumbnail_notice_for_hidden_edits;
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
    let mut groups: BTreeMap<&[u8], Vec<(u32, &[u8])>> = BTreeMap::new();
    for seg in segments {
        let payload = &seg[4..];
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

fn inspect_png_xmp(chunk: &[u8], total: &mut usize, notice: &mut ThumbnailNoticeKinds) {
    let data = &chunk[8..chunk.len() - 4];
    let Some(key_end) = data.iter().position(|&b| b == 0) else {
        return;
    };
    if &data[..key_end] != XMP_KEY {
        return;
    }
    let remaining = FILE_LIMIT.saturating_sub(*total);
    if remaining == 0 {
        notice.xmp_uninspectable = true;
        return;
    }
    let body = &data[key_end + 1..];
    let decoded = (|| -> Option<Vec<u8>> {
        let (compressed, content) = match &chunk[4..8] {
            b"tEXt" => (false, body),
            b"zTXt" => {
                if body.first() != Some(&0) {
                    return None;
                }
                (true, &body[1..])
            }
            b"iTXt" => {
                if body.len() < 2 || body[0] > 1 || body[1] != 0 {
                    return None;
                }
                let mut content = &body[2..];
                for _ in 0..2 {
                    let end = content.iter().position(|&b| b == 0)?;
                    content = &content[end + 1..];
                }
                (body[0] == 1, content)
            }
            _ => return None,
        };
        let limit = CHUNK_LIMIT.min(remaining);
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
            if content.len() > limit {
                return None;
            }
            Some(content.to_vec())
        }
    })();
    match decoded {
        Some(body) => notice.xmp |= has_thumbnail(&body),
        None => notice.xmp_uninspectable = true,
    }
}

// TIFF の offset はすべて TIFF 本体から。無効な領域や循環を含む EXIF は丸ごと落とす。
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

fn clean_exif(data: &[u8]) -> Option<(Vec<u8>, bool)> {
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
