//! Bounded, read-only PIC/APIC extraction from the leading ID3v2 tag only.
//! No FFmpeg, playback input, tag editing, or appended-tag discovery.
use flate2::{Decompress, FlushDecompress, Status};
use std::io::{self, Read};

pub const TAG_LIMIT: usize = 32 * 1024 * 1024;
pub const PICTURE_LIMIT: usize = 16 * 1024 * 1024;
const CHUNK: usize = 64 * 1024;
const MAX_PICTURES: usize = 16;

/// Request-local bounds; tests/fuzz can only reduce the production envelope.
#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub tag_bytes: usize,
    pub picture_bytes: usize,
    pub picture_count: usize,
}
impl Limits {
    pub const PRODUCTION: Self = Self {
        tag_bytes: TAG_LIMIT,
        picture_bytes: PICTURE_LIMIT,
        picture_count: MAX_PICTURES,
    };
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Malformed,
    Limit,
    Canceled,
    Timeout,
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Clone, Copy, Default)]
struct Frame {
    start: usize,
    end: usize,
    version: u8,
    compressed: bool,
    declared: Option<usize>,
}

fn syncsafe(bytes: &[u8]) -> Result<usize, Error> {
    if bytes.len() != 4 || bytes.iter().any(|b| b & 0x80 != 0) {
        return Err(Error::Malformed);
    }
    Ok(bytes.iter().fold(0usize, |n, b| (n << 7) | usize::from(*b)))
}

fn be(bytes: &[u8]) -> usize {
    bytes.iter().fold(0usize, |n, b| (n << 8) | usize::from(*b))
}

fn range_end(start: usize, size: usize, end: usize) -> Result<usize, Error> {
    start
        .checked_add(size)
        .filter(|n| *n <= end)
        .ok_or(Error::Malformed)
}

fn unsync(data: &mut [u8], check: &mut impl FnMut() -> Result<(), Error>) -> Result<usize, Error> {
    let mut write = 0;
    let mut read = 0;
    let mut next_check = 0;
    while read < data.len() {
        if read >= next_check {
            check()?;
            next_check = read.saturating_add(CHUNK);
        }
        let b = data[read];
        data[write] = b;
        write += 1;
        read += 1;
        if b == 0xff && data.get(read) == Some(&0) {
            read += 1;
        }
    }
    check()?;
    Ok(write)
}

/// Calls `accept` in front-cover-first order, retaining at most one image.
/// `check` owns cancellation and the single request-wide deadline.
pub fn extract<T>(
    reader: &mut impl Read,
    file_size: u64,
    check: &mut impl FnMut() -> Result<(), Error>,
    accept: &mut impl FnMut(&[u8]) -> Result<Option<T>, Error>,
) -> Result<Option<T>, Error> {
    extract_with_limits(reader, file_size, check, accept, Limits::PRODUCTION)
}

pub(crate) fn extract_with_limits<T>(
    reader: &mut impl Read,
    file_size: u64,
    check: &mut impl FnMut() -> Result<(), Error>,
    accept: &mut impl FnMut(&[u8]) -> Result<Option<T>, Error>,
    limits: Limits,
) -> Result<Option<T>, Error> {
    check()?;
    if limits.tag_bytes > TAG_LIMIT
        || limits.picture_bytes > PICTURE_LIMIT
        || limits.picture_count > MAX_PICTURES
    {
        return Err(Error::Limit);
    }
    let mut header = [0; 10];
    if file_size < 3 {
        return Ok(None);
    }
    reader.read_exact(&mut header[..3])?;
    check()?;
    if &header[..3] != b"ID3" {
        return Ok(None);
    }
    if file_size < 10 {
        return Err(Error::Malformed);
    }
    reader.read_exact(&mut header[3..])?;
    check()?;
    let version = header[3];
    let allowed = match version {
        2 => 0xc0,
        3 => 0xe0,
        4 => 0xf0,
        _ => return Ok(None),
    };
    if header[4] == 0xff || header[5] & !allowed != 0 {
        return Err(Error::Malformed);
    }
    let size = syncsafe(&header[6..10])?;
    let footer = version == 4 && header[5] & 0x10 != 0;
    let physical = size
        .checked_add(10 + if footer { 10 } else { 0 })
        .ok_or(Error::Limit)?;
    if physical > limits.tag_bytes {
        return Err(Error::Limit);
    }
    if physical as u64 > file_size {
        return Err(Error::Malformed);
    }
    if version == 2 && header[5] & 0x40 != 0 {
        return Ok(None);
    }
    let mut raw = Vec::new();
    raw.try_reserve_exact(size).map_err(|_| Error::Limit)?;
    raw.resize(size, 0);
    for chunk in raw.chunks_mut(CHUNK) {
        check()?;
        reader.read_exact(chunk)?;
        check()?;
    }
    if footer {
        let mut tail = [0; 10];
        check()?;
        reader.read_exact(&mut tail)?;
        if &tail[..3] != b"3DI" || tail[3..] != header[3..] {
            return Err(Error::Malformed);
        }
    }
    let mut end = raw.len();
    if version < 4 && header[5] & 0x80 != 0 {
        end = unsync(&mut raw, check)?;
    }
    let mut offset = 0;
    if version > 2 && header[5] & 0x40 != 0 {
        if end < 4 {
            return Err(Error::Malformed);
        }
        offset = if version == 3 {
            range_end(4, be(&raw[..4]), end)?
        } else {
            syncsafe(&raw[..4])?
        };
        if offset > end || offset < if version == 3 { 10 } else { 6 } {
            return Err(Error::Malformed);
        }
        // Extended header fields are not art metadata; validate their structural sizes.
        if version == 3 {
            let flags = u16::from_be_bytes([raw[4], raw[5]]);
            if flags & !0x8000 != 0 || offset != if flags & 0x8000 != 0 { 14 } else { 10 } {
                return Err(Error::Malformed);
            }
            if be(&raw[6..10]) > end - offset {
                return Err(Error::Malformed);
            }
        } else {
            if raw[4] != 1 || raw[5] & !0x70 != 0 {
                return Err(Error::Malformed);
            }
            let mut pos = 6;
            for (flag, len) in [(0x40, 0), (0x20, 5), (0x10, 1)] {
                if raw[5] & flag != 0 {
                    if raw.get(pos) != Some(&len) {
                        return Err(Error::Malformed);
                    }
                    pos = range_end(pos, usize::from(len) + 1, offset)?;
                }
            }
            if pos != offset {
                return Err(Error::Malformed);
            }
        }
    }
    let mut frames = [Frame::default(); MAX_PICTURES];
    let mut count = 0;
    let mut stored = 0;
    let hlen = if version == 2 { 6 } else { 10 };
    let idlen = if version == 2 { 3 } else { 4 };
    while offset < end {
        check()?;
        if raw[offset] == 0 {
            for chunk in raw[offset..end].chunks(CHUNK) {
                check()?;
                if chunk.iter().any(|b| *b != 0) {
                    return Err(Error::Malformed);
                }
            }
            break;
        }
        let start = range_end(offset, hlen, end)?;
        let id = &raw[offset..offset + idlen];
        if !id
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            return Err(Error::Malformed);
        }
        let art = id
            == if version == 2 {
                b"PIC".as_slice()
            } else {
                b"APIC".as_slice()
            };
        let len = if version == 4 {
            syncsafe(&raw[offset + 4..offset + 8])?
        } else {
            be(&raw[offset + idlen..offset + hlen - if version == 3 { 2 } else { 0 }])
        };
        if len == 0 {
            return Err(Error::Malformed);
        }
        let frame_end = range_end(start, len, end)?;
        if art {
            count += 1;
            if count > limits.picture_count {
                return Err(Error::Limit);
            }
            let flags = if version == 2 {
                0
            } else {
                u16::from_be_bytes([raw[offset + 8], raw[offset + 9]])
            };
            let known = match version {
                2 => 0,
                3 => 0xe0e0,
                _ => 0x704f,
            };
            let encrypted = flags & if version == 3 { 0x40 } else { 0x04 } != 0;
            if flags & !known == 0 && !encrypted {
                let mut logical_end = frame_end;
                if version == 4 && (header[5] & 0x80 != 0 || flags & 0x02 != 0) {
                    logical_end = start + unsync(&mut raw[start..frame_end], check)?;
                }
                let mut body = start;
                let compressed = flags & if version == 3 { 0x80 } else { 0x08 } != 0;
                let mut declared = None;
                let fields = (|| -> Result<(), Error> {
                    if version == 3 && compressed {
                        let next = range_end(body, 4, logical_end)?;
                        declared = Some(be(&raw[body..next]));
                        body = next;
                    }
                    if flags & if version == 3 { 0x20 } else { 0x40 } != 0 {
                        body = range_end(body, 1, logical_end)?;
                    }
                    if version == 4 && flags & 1 != 0 {
                        let next = range_end(body, 4, logical_end)?;
                        declared = Some(syncsafe(&raw[body..next])?);
                        body = next;
                    }
                    if compressed && declared.is_none() {
                        return Err(Error::Malformed);
                    }
                    Ok(())
                })();
                if fields.is_ok() && declared.is_none_or(|n| n <= limits.tag_bytes) {
                    frames[stored] = Frame {
                        start: body,
                        end: logical_end,
                        version,
                        compressed,
                        declared,
                    };
                    stored += 1;
                }
            }
        }
        offset = frame_end;
    }
    let mut kinds = [None; MAX_PICTURES];
    for (idx, frame) in frames[..stored].iter().enumerate() {
        match inspect(&raw[frame.start..frame.end], *frame, check, limits) {
            Ok(info) => kinds[idx] = Some(info.0),
            Err(Error::Malformed | Error::Limit) => {}
            Err(e) => return Err(e),
        }
    }
    for front in [true, false] {
        for (idx, frame) in frames[..stored].iter().enumerate() {
            if !kinds[idx].is_some_and(|kind| (kind == 3) == front) {
                continue;
            }
            check()?;
            match picture(&raw[frame.start..frame.end], *frame, check, accept, limits) {
                Ok(Some(value)) => {
                    check()?;
                    return Ok(Some(value));
                }
                Ok(None) | Err(Error::Malformed | Error::Limit) => {}
                Err(e) => return Err(e),
            }
        }
    }
    check()?;
    Ok(None)
}

// Stream prefix fields without allocating strings, including a long description.
struct Prefix {
    version: u8,
    stage: u8,
    encoding: u8,
    mime: [u8; 3],
    mime_len: usize,
    kind: u8,
    desc: usize,
    previous: u8,
    bom: [u8; 2],
    length: usize,
}
impl Prefix {
    fn new(version: u8) -> Self {
        Self {
            version,
            stage: 0,
            encoding: 0,
            mime: [0; 3],
            mime_len: 0,
            kind: 0,
            desc: 0,
            previous: 0,
            bom: [0; 2],
            length: 0,
        }
    }
    fn feed(&mut self, b: u8) -> Result<bool, Error> {
        self.length += 1;
        match self.stage {
            0 => {
                if b > if self.version == 4 { 3 } else { 1 } {
                    return Err(Error::Malformed);
                }
                self.encoding = b;
                self.stage = 1;
            }
            1 => {
                if self.version > 2 && b == 0 || self.version == 2 && self.mime_len == 3 {
                    if self.mime_len == 0 || self.mime_len == 3 && &self.mime == b"-->" {
                        return Err(Error::Malformed);
                    }
                    self.stage = 2;
                    if self.version == 2 {
                        self.kind = b;
                        self.stage = 3;
                    }
                } else {
                    if self.mime_len < 3 {
                        self.mime[self.mime_len] = b;
                    }
                    self.mime_len += 1;
                }
            }
            2 => {
                self.kind = b;
                self.stage = 3;
            }
            3 => {
                if self.desc < 2 {
                    self.bom[self.desc] = b;
                }
                self.desc += 1;
                let done = if self.encoding == 0 || self.encoding == 3 {
                    b == 0
                } else {
                    self.desc % 2 == 0 && b == 0 && self.previous == 0
                };
                self.previous = b;
                if done {
                    if self.encoding == 1
                        && self.desc > 2
                        && self.bom != [0xff, 0xfe]
                        && self.bom != [0xfe, 0xff]
                    {
                        return Err(Error::Malformed);
                    }
                    return Ok(true);
                }
            }
            _ => unreachable!(),
        }
        Ok(false)
    }
}

struct Inflater<'a> {
    input: &'a [u8],
    decoder: Decompress,
    declared: usize,
    done: bool,
}
impl<'a> Inflater<'a> {
    fn new(input: &'a [u8], declared: usize, limits: Limits) -> Result<Self, Error> {
        if declared > limits.tag_bytes {
            return Err(Error::Limit);
        }
        Ok(Self {
            input,
            decoder: Decompress::new(true),
            declared,
            done: false,
        })
    }
    fn read(
        &mut self,
        output: &mut [u8],
        check: &mut impl FnMut() -> Result<(), Error>,
    ) -> Result<usize, Error> {
        if self.done {
            return Ok(0);
        }
        check()?;
        let start_in = self.decoder.total_in();
        let start_out = self.decoder.total_out();
        let begin = start_in as usize;
        let end = (begin + CHUNK).min(self.input.len());
        let remaining = self.declared.saturating_sub(start_out as usize);
        let cap = output.len().min(CHUNK).min(remaining.saturating_add(1));
        let status = self
            .decoder
            .decompress(
                &self.input[begin..end],
                &mut output[..cap],
                FlushDecompress::None,
            )
            .map_err(|_| Error::Malformed)?;
        check()?;
        let produced = (self.decoder.total_out() - start_out) as usize;
        if self.decoder.total_out() > self.declared as u64 {
            return Err(Error::Malformed);
        }
        if status == Status::StreamEnd {
            if self.decoder.total_out() != self.declared as u64
                || self.decoder.total_in() != self.input.len() as u64
            {
                return Err(Error::Malformed);
            }
            self.done = true;
        } else if self.decoder.total_in() == start_in && produced == 0 {
            return Err(Error::Malformed);
        }
        Ok(produced)
    }
}

fn inspect(
    input: &[u8],
    frame: Frame,
    check: &mut impl FnMut() -> Result<(), Error>,
    limits: Limits,
) -> Result<(u8, usize), Error> {
    let mut prefix = Prefix::new(frame.version);
    if frame.compressed {
        let mut stream = Inflater::new(input, frame.declared.ok_or(Error::Malformed)?, limits)?;
        let mut scratch = [0; CHUNK];
        loop {
            let n = stream.read(&mut scratch, check)?;
            for b in &scratch[..n] {
                if prefix.feed(*b)? {
                    let len = frame
                        .declared
                        .unwrap()
                        .checked_sub(prefix.length)
                        .ok_or(Error::Malformed)?;
                    if len > limits.picture_bytes {
                        return Err(Error::Limit);
                    }
                    return Ok((prefix.kind, prefix.length));
                }
            }
            if stream.done {
                return Err(Error::Malformed);
            }
        }
    } else {
        if frame.declared.is_some_and(|n| n != input.len()) {
            return Err(Error::Malformed);
        }
        for (idx, b) in input.iter().enumerate() {
            if idx % CHUNK == 0 {
                check()?;
            }
            if prefix.feed(*b)? {
                if input.len() - prefix.length > limits.picture_bytes {
                    return Err(Error::Limit);
                }
                return Ok((prefix.kind, prefix.length));
            }
        }
        Err(Error::Malformed)
    }
}

fn picture<T>(
    input: &[u8],
    frame: Frame,
    check: &mut impl FnMut() -> Result<(), Error>,
    accept: &mut impl FnMut(&[u8]) -> Result<Option<T>, Error>,
    limits: Limits,
) -> Result<Option<T>, Error> {
    if !frame.compressed {
        let (_, prefix_len) = inspect(input, frame, check, limits)?;
        return accept(&input[prefix_len..]);
    }
    let mut prefix = Prefix::new(frame.version);
    let mut stream = Inflater::new(input, frame.declared.ok_or(Error::Malformed)?, limits)?;
    let mut scratch = [0; CHUNK];
    let mut pixels = None;
    let mut written = 0;
    loop {
        let n = stream.read(&mut scratch, check)?;
        let mut begin = 0;
        if pixels.is_none() {
            while begin < n {
                let done = prefix.feed(scratch[begin])?;
                begin += 1;
                if done {
                    let len = stream
                        .declared
                        .checked_sub(prefix.length)
                        .ok_or(Error::Malformed)?;
                    if len > limits.picture_bytes {
                        return Err(Error::Limit);
                    }
                    let mut image = Vec::new();
                    image.try_reserve_exact(len).map_err(|_| Error::Limit)?;
                    image.resize(len, 0);
                    pixels = Some(image);
                    break;
                }
            }
        }
        if let Some(image) = pixels.as_mut() {
            let next = range_end(written, n - begin, image.len())?;
            image[written..next].copy_from_slice(&scratch[begin..n]);
            written = next;
        }
        if stream.done {
            let image = pixels.ok_or(Error::Malformed)?;
            if written != image.len() {
                return Err(Error::Malformed);
            }
            check()?;
            return accept(&image);
        }
    }
}

#[cfg(test)]
#[path = "audio_album_art/tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "audio_album_art/fuzz.rs"]
pub(crate) mod fuzz;
