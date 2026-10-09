//! Album-art-only metadata inspection. No recursive EXIF, owned tag objects, or codecs.
use crate::audio_album_art::Error;

pub(super) const DECODE_BUDGET: u64 = 160 * 1024 * 1024;
const EXIF_LIMIT: usize = 64 * 1024;

/// Walk every marker, including between scans. Stuffed bytes and restart markers
/// stay in entropy data. The caller cannot miss a later SOF/DNL changing dimensions.
fn jpeg_segments(
    bytes: &[u8],
    check: &mut impl FnMut() -> Result<(), Error>,
    mut visit: impl FnMut(
        u8,
        usize,
        usize,
        &[u8],
        &mut dyn FnMut() -> Result<(), Error>,
    ) -> Result<(), Error>,
) -> Result<(), Error> {
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return Err(Error::Malformed);
    }
    let mut pos: usize = 2;
    let mut entropy = false;
    let mut next_check = 0;
    loop {
        if pos >= next_check {
            check()?;
            next_check = pos.saturating_add(64 * 1024);
        }
        if entropy {
            while bytes.get(pos).is_some_and(|b| *b != 0xff) {
                pos += 1;
                if pos >= next_check {
                    check()?;
                    next_check = pos.saturating_add(64 * 1024);
                }
            }
        }
        let start = pos;
        if bytes.get(pos) != Some(&0xff) {
            return Err(Error::Malformed);
        }
        while bytes.get(pos) == Some(&0xff) {
            pos += 1;
            if pos >= next_check {
                check()?;
                next_check = pos.saturating_add(64 * 1024);
            }
        }
        let marker = *bytes.get(pos).ok_or(Error::Malformed)?;
        pos += 1;
        if entropy && (marker == 0 || (0xd0..=0xd7).contains(&marker)) {
            continue;
        }
        entropy = false;
        if marker == 0xd9 {
            check()?;
            return Ok(());
        }
        if marker == 0 || marker == 0xd8 || (0xd0..=0xd7).contains(&marker) {
            return Err(Error::Malformed);
        }
        if marker == 1 {
            continue;
        }
        let len_bytes = bytes
            .get(pos..pos.checked_add(2).ok_or(Error::Malformed)?)
            .ok_or(Error::Malformed)?;
        let len = usize::from(u16::from_be_bytes([len_bytes[0], len_bytes[1]]));
        if len < 2 {
            return Err(Error::Malformed);
        }
        let end = pos
            .checked_add(len)
            .filter(|end| *end <= bytes.len())
            .ok_or(Error::Malformed)?;
        visit(marker, start, end, &bytes[pos + 2..end], check)?;
        pos = end;
        entropy = marker == 0xda;
    }
}

fn tiff_orientation(
    data: &[u8],
    check: &mut (impl FnMut() -> Result<(), Error> + ?Sized),
) -> Result<Option<u16>, Error> {
    // Only IFD0 inline SHORT/count=1 is relevant. Never follow Exif/GPS/next-IFD offsets.
    if data.len() < 8 || data.len() > EXIF_LIMIT {
        return Ok(None);
    }
    let little = match &data[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Ok(None),
    };
    let u16_at = |pos: usize| -> Option<u16> {
        let b: [u8; 2] = data.get(pos..pos.checked_add(2)?)?.try_into().ok()?;
        Some(if little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32_at = |pos: usize| -> Option<u32> {
        let b: [u8; 4] = data.get(pos..pos.checked_add(4)?)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    if u16_at(2) != Some(42) {
        return Ok(None);
    }
    let Some(ifd) = u32_at(4).map(|n| n as usize).filter(|n| *n >= 8) else {
        return Ok(None);
    };
    let Some(count) = u16_at(ifd).map(usize::from) else {
        return Ok(None);
    };
    let Some(start) = ifd.checked_add(2) else {
        return Ok(None);
    };
    if start
        .checked_add(count * 12)
        .is_none_or(|end| end > data.len())
    {
        return Ok(None);
    }
    for i in 0..count {
        if i % 64 == 0 {
            check()?;
        }
        let pos = start + i * 12;
        if u16_at(pos) == Some(0x112) && u16_at(pos + 2) == Some(3) && u32_at(pos + 4) == Some(1) {
            return Ok(u16_at(pos + 8).filter(|value| (1..=8).contains(value)));
        }
    }
    Ok(None)
}

pub(super) fn orientation(
    bytes: &[u8],
    check: &mut impl FnMut() -> Result<(), Error>,
) -> Result<u16, Error> {
    check()?;
    let mut value = None;
    if bytes.starts_with(&[0xff, 0xd8]) {
        jpeg_segments(bytes, check, |marker, _, _, data, check| {
            if value.is_none() && marker == 0xe1 && data.starts_with(b"Exif\0\0") {
                value = tiff_orientation(&data[6..], check)?;
            }
            Ok(())
        })?;
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let mut pos: usize = 8;
        while pos < bytes.len() {
            check()?;
            let header = bytes
                .get(pos..pos.checked_add(8).ok_or(Error::Malformed)?)
                .ok_or(Error::Malformed)?;
            let len = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
            let start = pos + 8;
            let end = start
                .checked_add(len)
                .filter(|end| end.checked_add(4).is_some_and(|end| end <= bytes.len()))
                .ok_or(Error::Malformed)?;
            if &header[4..] == b"eXIf" {
                value = tiff_orientation(&bytes[start..end], check)?;
                break;
            }
            if &header[4..] == b"IEND" {
                break;
            }
            pos = end + 4;
        }
    }
    check()?;
    Ok(value.unwrap_or(1))
}

#[derive(Clone, Copy, Debug)]
pub(super) struct JpegBudget {
    pub dims: (u32, u32),
    pub required: u64,
}

/// Conservative bound for both fixed-version native and fallback decoders.
/// Assume full-image coefficients even for sequential JPEG and for any scan layout.
/// Metadata is stripped before either decoder, so neither expands EXIF/XMP/ICC.
pub(super) fn jpeg_budget(
    bytes: &[u8],
    budget: u64,
    check: &mut impl FnMut() -> Result<(), Error>,
) -> Result<JpegBudget, Error> {
    let mut frame = None;
    jpeg_segments(bytes, check, |marker, _, _, data, _| {
        if marker == 0xdc || marker == 0xde {
            return Err(Error::Malformed);
        }
        if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
            if frame.is_some() || ![0xc0, 0xc1, 0xc2, 0xc3].contains(&marker) || data.len() < 6 {
                return Err(Error::Malformed);
            }
            let h = u64::from(u16::from_be_bytes([data[1], data[2]]));
            let w = u64::from(u16::from_be_bytes([data[3], data[4]]));
            let components = usize::from(data[5]);
            if w == 0
                || h == 0
                || !(1..=4).contains(&components)
                || data.len() != 6 + 3 * components
                || ![8, 12, 16].contains(&data[0])
            {
                return Err(Error::Malformed);
            }
            let mut samples = 0u64;
            let mut ids = [0; 4];
            for (i, c) in data[6..].chunks_exact(3).enumerate() {
                if ids[..i].contains(&c[0]) {
                    return Err(Error::Malformed);
                }
                ids[i] = c[0];
                let (hs, vs) = (c[1] >> 4, c[1] & 15);
                if !(1..=4).contains(&hs) || !(1..=4).contains(&vs) {
                    return Err(Error::Malformed);
                }
                samples += u64::from(hs) * u64::from(vs);
            }
            if w * h > 40_000_000 {
                return Err(Error::Limit);
            }
            // i16 coefficient planes (worst noninterleaved layout), output RGBA,
            // generously bounded row/upsampling scratch, input copies, fixed tables.
            let padded_w = w.div_ceil(8) * 8;
            let padded_h = h.div_ceil(8) * 8;
            let required = padded_w * padded_h * samples * 2
                + w * h * 4
                + padded_w * samples * 512
                + bytes.len() as u64 * 4
                + 4 * 1024 * 1024;
            if required > budget {
                return Err(Error::Limit);
            }
            frame = Some(JpegBudget {
                dims: (w as u32, h as u32),
                required,
            });
        }
        Ok(())
    })?;
    frame.ok_or(Error::Malformed)
}

/// Retain JFIF/Adobe color hints, coding/scan data, and entropy verbatim. Drop
/// APP1..APP13, APP15 and COM so downstream general codecs never parse metadata.
pub(super) fn jpeg_without_metadata(
    bytes: &[u8],
    check: &mut impl FnMut() -> Result<(), Error>,
) -> Result<Vec<u8>, Error> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(bytes.len())
        .map_err(|_| Error::Limit)?;
    let mut start = 0;
    jpeg_segments(bytes, check, |marker, begin, end, _, _| {
        if (0xe1..=0xed).contains(&marker) || marker == 0xef || marker == 0xfe {
            result.extend_from_slice(&bytes[start..begin]);
            start = end;
        }
        Ok(())
    })?;
    result.extend_from_slice(&bytes[start..]);
    Ok(result)
}

#[cfg(test)]
#[path = "metadata_tests.rs"]
pub(crate) mod tests;
