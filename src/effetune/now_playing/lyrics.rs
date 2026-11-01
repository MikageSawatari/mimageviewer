//! Raw lyric candidates. Parsing and selection belong to Mixwright.
use std::{
    io::Read,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

pub(crate) const UTF16_LIMIT: usize = 262_144;
const FILE_LIMIT: u64 = 1024 * 1024;

pub(crate) fn prefix(text: &str) -> &str {
    let mut units = 0;
    for (index, ch) in text.char_indices() {
        if ch == '\0' || units + ch.len_utf16() > UTF16_LIMIT {
            return &text[..index];
        }
        units += ch.len_utf16();
    }
    text
}

fn decode(bytes: &[u8]) -> String {
    let text = if let Some((encoding, skip)) = encoding_rs::Encoding::for_bom(bytes) {
        encoding.decode_without_bom_handling(&bytes[skip..]).0
    } else if let Ok(text) = std::str::from_utf8(bytes) {
        std::borrow::Cow::Borrowed(text)
    } else {
        encoding_rs::SHIFT_JIS.decode_without_bom_handling(bytes).0
    };
    prefix(&text).to_owned()
}

pub(super) fn read_sidecar(path: &Path, cancel: &AtomicBool) -> Result<Option<String>, String> {
    let started = std::time::Instant::now();
    let result = (|| {
        let Some(parent) = path.parent() else {
            return Ok(None);
        };
        let Some(stem) = path.file_stem() else {
            return Ok(None);
        };
        for entry in std::fs::read_dir(parent).map_err(|e| e.to_string())? {
            if cancel.load(Ordering::Acquire) {
                return Ok(None);
            }
            let entry = entry.map_err(|e| e.to_string())?;
            let candidate = entry.path();
            if !candidate
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("lrc"))
                || !candidate.file_stem().is_some_and(|other| {
                    other.to_string_lossy().to_lowercase() == stem.to_string_lossy().to_lowercase()
                })
                || !entry.file_type().map_err(|e| e.to_string())?.is_file()
            {
                continue;
            }
            let file = std::fs::File::open(&candidate).map_err(|e| e.to_string())?;
            if file.metadata().map_err(|e| e.to_string())?.len() > FILE_LIMIT {
                return Err("LRC exceeds 1 MiB".into());
            }
            let mut bytes = Vec::new();
            file.take(FILE_LIMIT + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() as u64 > FILE_LIMIT {
                return Err("LRC exceeds 1 MiB".into());
            }
            return Ok(Some(decode(&bytes)).filter(|text| !text.is_empty()));
        }
        Ok(None)
    })();
    if crate::perf::is_enabled() {
        crate::perf::event(
            "effetune",
            "lyrics_sidecar",
            None,
            0,
            &[
                (
                    "ms",
                    started.elapsed().as_secs_f64().mul_add(1000.0, 0.0).into(),
                ),
                ("ok", result.is_ok().into()),
            ],
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lyrics_encodings_case_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("Song.flac");
        let lrc = dir.path().join("sONG.LRC");
        let text = "[00:01.00]歌詞";
        let sjis = encoding_rs::SHIFT_JIS.encode(text).0.into_owned();
        let mut le = vec![0xff, 0xfe];
        let mut be = vec![0xfe, 0xff];
        for unit in text.encode_utf16() {
            le.extend(unit.to_le_bytes());
            be.extend(unit.to_be_bytes());
        }
        for bytes in [
            text.as_bytes().to_vec(),
            [b"\xef\xbb\xbf".as_slice(), text.as_bytes()].concat(),
            le,
            be,
            sjis,
        ] {
            std::fs::write(&lrc, bytes).unwrap();
            assert_eq!(
                read_sidecar(&media, &AtomicBool::new(false))
                    .unwrap()
                    .as_deref(),
                Some(text)
            );
        }
        std::fs::write(&lrc, vec![b'x'; FILE_LIMIT as usize]).unwrap();
        assert_eq!(
            read_sidecar(&media, &AtomicBool::new(false))
                .unwrap()
                .unwrap()
                .len(),
            UTF16_LIMIT
        );
        std::fs::write(&lrc, vec![b'x'; FILE_LIMIT as usize + 1]).unwrap();
        assert!(read_sidecar(&media, &AtomicBool::new(false)).is_err());
        assert_eq!(
            prefix(&("x".repeat(UTF16_LIMIT - 1) + "😀")),
            "x".repeat(UTF16_LIMIT - 1)
        );
        assert_eq!(prefix("first\0rest"), "first");
    }
}
