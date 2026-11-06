use super::*;

const XML: &[u8] = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description xmlns:xtw="https://mXDownloader.app/ns/x-twitter/1.0/"
xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:xmp="http://ns.adobe.com/xap/1.0/"
xmlns:GPano="http://ns.google.com/photos/1.0/panorama/"
xtw:TweetId="42" xmp:Rating="4" GPano:ProjectionType="equirectangular">
<dc:subject><rdf:Bag><rdf:li>test</rdf:li></rdf:Bag></dc:subject>
</rdf:Description></rdf:RDF></x:xmpmeta>"#;

fn jpeg_segment(out: &mut Vec<u8>, marker: u8, payload: &[u8]) {
    out.extend_from_slice(&[0xff, marker]);
    out.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
    out.extend_from_slice(payload);
}

fn jpeg_xmp(out: &mut Vec<u8>, xml: &[u8]) {
    let mut payload = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
    payload.extend_from_slice(xml);
    jpeg_segment(out, 0xe1, &payload);
}

fn jpeg_image(out: &mut Vec<u8>, payload: &[u8]) {
    out.extend_from_slice(&[0xff, 0xda, 0, 2]);
    out.extend_from_slice(payload);
    out.extend_from_slice(&[0xff, 0xd9]);
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out.extend_from_slice(&[0; 4]); // CRC is deliberately ignored by both readers.
}

fn png_text(xml: &[u8], itxt: bool, compressed: bool) -> Vec<u8> {
    let mut payload = b"XML:com.adobe.xmp\0".to_vec();
    if itxt {
        payload.extend_from_slice(&[u8::from(compressed), 0, 0, 0]);
    }
    payload.extend_from_slice(xml);
    payload
}

fn png(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, payload) in chunks {
        png_chunk(&mut out, kind, payload);
    }
    out
}

fn webp(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let mut out = b"RIFF\0\0\0\0WEBP".to_vec();
    for (kind, payload) in chunks {
        out.extend_from_slice(*kind);
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        if payload.len() & 1 != 0 {
            out.push(0);
        }
    }
    let size = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&size.to_le_bytes());
    out
}

/// Every fixture checks the common path entry, all public wrappers, and bytes.
/// Debug serialization compares every optional parsed field without changing
/// the production types just to make assertions convenient.
fn check_fixture(bytes: &[u8], ext: &str, expected: Option<&[u8]>, legacy_equal: bool) -> u64 {
    assert_eq!(extract_xmp_packet(bytes).as_deref(), expected);
    if legacy_equal {
        assert_eq!(legacy_extract_xmp_packet(bytes).as_deref(), expected);
    } else {
        assert_ne!(legacy_extract_xmp_packet(bytes).as_deref(), expected);
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("fixture.{ext}"));
    std::fs::write(&path, bytes).unwrap();
    let read = read_xmp_packet_from_path(&path).unwrap();
    assert_eq!(read.packet.as_deref(), expected);
    assert_eq!(
        format!("{:?}", read_tweet_info(&path)),
        format!("{:?}", read_tweet_info_from_bytes(bytes))
    );
    assert_eq!(
        format!("{:?}", read_xmp_bundle(&path)),
        format!("{:?}", read_xmp_bundle_from_bytes(bytes))
    );
    assert_eq!(
        format!("{:?}", read_panorama_info(&path)),
        format!("{:?}", read_panorama_info_from_bytes(bytes))
    );
    assert_eq!(read_xmp_rating(&path), read_xmp_rating_from_bytes(bytes));
    // Video dc:subject intentionally reads only the sidecar.
    if !crate::xmp_writer::is_video_for_sidecar(&path) {
        assert_eq!(
            try_read_dc_subject(&path).unwrap(),
            read_dc_subject_from_bytes(bytes)
        );
        assert_eq!(read_dc_subject(&path), read_dc_subject_from_bytes(bytes));
    }
    let counted = read_tweet_info_counted(&path);
    assert_eq!(counted.bytes_read, read.bytes_read);
    read.bytes_read
}

#[test]
fn jpeg_partial_read_late_app1_and_extended_xmp_match_legacy() {
    let mut bytes = vec![0xff, 0xd8];
    for _ in 0..12 {
        jpeg_segment(&mut bytes, 0xe1, &vec![0; 60_000]);
    }
    let extension = b"http://ns.adobe.com/xmp/extension/\0GUID-part-1";
    jpeg_segment(&mut bytes, 0xe1, extension);
    jpeg_xmp(&mut bytes, XML);
    jpeg_segment(&mut bytes, 0xe1, extension);
    jpeg_image(&mut bytes, &[]);
    let count = check_fixture(&bytes, "jpg", Some(XML), true);
    assert!(count < 2_000, "{count}");
}

#[test]
fn jpeg_partial_read_no_xmp_skips_eight_mb_image() {
    let mut bytes = vec![0xff, 0xd8];
    let mut exif = [0; 64];
    exif[..6].copy_from_slice(b"Exif\0\0");
    jpeg_segment(&mut bytes, 0xe1, &exif);
    jpeg_image(&mut bytes, &vec![0; 8 * 1024 * 1024]);
    let count = check_fixture(&bytes, "jpeg", None, true);
    assert_eq!(count, 12 + 4 + 29 + 2 + 70);
}

#[test]
fn jpeg_partial_read_excludes_fallback_after_sos() {
    let mut bytes = vec![0xff, 0xd8];
    jpeg_image(&mut bytes, XML);
    check_fixture(&bytes, "jfif", None, false);
}

#[test]
fn jpeg_partial_read_preserves_malformed_and_standalone_markers() {
    for bytes in [
        vec![0xff, 0xd8],
        vec![0xff, 0xd8, 0xff, 0xe1, 0, 100, 1, 2],
        vec![0xff, 0xd8, 0xff, 0xe1, 0, 1],
        vec![0xff, 0xd8, 0, 0, 0, 0],
    ] {
        check_fixture(&bytes, "jpg", None, true);
    }
    let mut bytes = vec![0xff, 0xd8, 0xff, 0xd8, 0xff, 0xd9];
    for marker in 0xd0..=0xd7 {
        bytes.extend_from_slice(&[0xff, marker]);
    }
    jpeg_xmp(&mut bytes, XML);
    check_fixture(&bytes, "jpg", Some(XML), true);
}

#[test]
fn png_partial_read_itxt_before_and_after_large_idat() {
    let itxt = png_text(XML, true, false);
    let image = vec![0; 8 * 1024 * 1024];
    for chunks in [
        vec![
            (b"iTXt", itxt.as_slice()),
            (b"IDAT", image.as_slice()),
            (b"IEND", &[]),
        ],
        vec![
            (b"IDAT", image.as_slice()),
            (b"iTXt", itxt.as_slice()),
            (b"IEND", &[]),
        ],
    ] {
        let bytes = png(&chunks);
        assert!(check_fixture(&bytes, "png", Some(XML), true) < 2_000);
    }
}

#[test]
fn png_partial_read_text_xmp_before_and_after_idat() {
    let text = png_text(XML, false, false);
    let image = vec![0; 8 * 1024 * 1024];
    let bytes = png(&[(b"tEXt", &text), (b"IDAT", &image), (b"IEND", &[])]);
    check_fixture(&bytes, "png", Some(XML), true);
    let bytes = png(&[(b"IDAT", &image), (b"tEXt", &text), (b"IEND", &[])]);
    assert!(check_fixture(&bytes, "png", Some(XML), false) < 2_000);
    // Also preserve the old fallback's post-IDAT tEXt inside its 512KB window.
    let bytes = png(&[(b"IDAT", &[0; 8]), (b"tEXt", &text), (b"IEND", &[])]);
    check_fixture(&bytes, "png", Some(XML), true);
}

#[test]
fn png_partial_read_itxt_has_priority_over_first_text() {
    let first = png_text(b"first text", false, false);
    let second = png_text(b"second text", false, false);
    let itxt = png_text(XML, true, false);
    for chunks in [
        vec![(b"tEXt", first.as_slice()), (b"iTXt", itxt.as_slice())],
        vec![(b"iTXt", itxt.as_slice()), (b"tEXt", first.as_slice())],
    ] {
        check_fixture(&png(&chunks), "png", Some(XML), true);
    }
    check_fixture(
        &png(&[(b"tEXt", &first), (b"tEXt", &second)]),
        "png",
        Some(b"first text"),
        false,
    );
}

#[test]
fn png_partial_read_compressed_and_malformed_itxt_terminate_scan() {
    let valid = png_text(XML, true, false);
    let compressed = png_text(b"compressed data", true, true);
    let mut short = b"XML:com.adobe.xmp\0".to_vec();
    for rest in [&[][..], &[0][..], &[0, 0][..], &[0, 0, 0][..]] {
        short.truncate(18);
        short.extend_from_slice(rest);
        // Later structurally valid XMP is past the fallback scan window.
        let bytes = png(&[
            (b"iTXt", &short),
            (b"IDAT", &vec![0; 600_000]),
            (b"iTXt", &valid),
        ]);
        check_fixture(&bytes, "png", None, true);
    }
    let bytes = png(&[
        (b"iTXt", &compressed),
        (b"IDAT", &vec![0; 600_000]),
        (b"iTXt", &valid),
    ]);
    check_fixture(&bytes, "png", None, true);
    // Legacy fallback can still find uncompressed XML in a flagged packet.
    let flagged_xml = png_text(XML, true, true);
    check_fixture(&png(&[(b"iTXt", &flagged_xml)]), "png", Some(XML), true);
    let bytes = png(&[
        (b"IDAT", &vec![0; 600_000]),
        (b"tEXt", &png_text(b"ignored", false, false)),
        (b"iTXt", &compressed),
        (b"iTXt", &valid),
    ]);
    check_fixture(&bytes, "png", None, true);
}

#[test]
fn png_partial_read_preserves_truncation_and_missing_iend() {
    let itxt = png_text(XML, true, false);
    check_fixture(&png(&[(b"iTXt", &itxt)]), "png", Some(XML), true);
    let mut bytes = png(&[(b"iTXt", &itxt)]);
    bytes.truncate(bytes.len() - 1); // Incomplete CRC: structural failure, fallback still works.
    check_fixture(&bytes, "png", Some(XML), true);
    let mut bytes = png(&[]);
    bytes.extend_from_slice(&u32::MAX.to_be_bytes());
    bytes.extend_from_slice(b"tEXtshort");
    check_fixture(&bytes, "png", None, true);
}

#[test]
fn png_partial_read_large_non_xmp_text_reads_only_keywords() {
    let mut junk = b"not-xmp\0".to_vec();
    junk.resize(4 * 1024 * 1024, 0);
    // Put IDAT first so fallback excludes both large text payloads.
    let bytes = png(&[
        (b"IDAT", &[0; 16]),
        (b"tEXt", &junk),
        (b"iTXt", &junk),
        (b"IEND", &[]),
    ]);
    assert_eq!(
        check_fixture(&bytes, "png", None, true),
        12 + 4 * 8 + 2 * 80 + 8
    );
}

#[test]
fn png_partial_read_excludes_image_fallback_but_keeps_metadata_fallback() {
    check_fixture(&png(&[(b"IDAT", XML), (b"IEND", &[])]), "png", None, false);
    check_fixture(
        &png(&[(b"junk", XML), (b"IDAT", &[0; 8])]),
        "png",
        Some(XML),
        true,
    );
}

#[test]
fn webp_partial_read_late_xmp_and_odd_padding_match_legacy() {
    let image = vec![0; 8 * 1024 * 1024];
    let bytes = webp(&[(b"VP8 ", &image), (b"junk", b"odd"), (b"XMP ", XML)]);
    assert!(check_fixture(&bytes, "webp", Some(XML), true) < 2_000);
    // A trailing XMP chunk is accepted even if the RIFF size was not updated.
    let mut bytes = bytes;
    bytes[4..8].copy_from_slice(&4u32.to_le_bytes());
    check_fixture(&bytes, "webp", Some(XML), true);
}

#[test]
fn webp_partial_read_preserves_truncation() {
    let mut bytes = webp(&[(b"junk", b"abc")]);
    bytes.extend_from_slice(b"XMP \xff\xff\xff\xffshort");
    check_fixture(&bytes, "webp", None, true);
    check_fixture(&webp(&[]), "webp", None, true);
}

#[test]
fn webp_partial_read_excludes_all_image_chunk_fallbacks() {
    for kind in [b"VP8 ", b"VP8L", b"ALPH", b"ANMF"] {
        check_fixture(&webp(&[(kind, XML)]), "webp", None, false);
    }
    check_fixture(
        &webp(&[(b"junk", XML), (b"VP8 ", &[0; 8])]),
        "webp",
        Some(XML),
        true,
    );
}

#[test]
fn fallback_partial_read_tiff_and_mp4_keep_512kb_limit() {
    for (ext, magic) in [
        ("tiff", b"II*\0".as_slice()),
        ("tif", b"MM\0*".as_slice()),
        ("mp4", b"\0\0\0\x18ftypisom".as_slice()),
    ] {
        for offset in [128, 700_000] {
            let mut bytes = vec![0; offset];
            bytes[..magic.len()].copy_from_slice(magic);
            bytes.extend_from_slice(XML);
            bytes.resize(8 * 1024 * 1024, 0);
            let expected = (offset == 128).then_some(XML);
            assert_eq!(
                check_fixture(&bytes, ext, expected, true),
                12 + FALLBACK_SCAN_LIMIT as u64
            );
        }
    }
}

#[test]
fn path_partial_read_uses_magic_even_with_tiff_or_video_extension() {
    let mut jpeg = vec![0xff, 0xd8];
    for _ in 0..12 {
        jpeg_segment(&mut jpeg, 0xe2, &vec![0; 60_000]);
    }
    jpeg_xmp(&mut jpeg, XML);
    let itxt = png_text(XML, true, false);
    let png = png(&[(b"IDAT", &vec![0; 700_000]), (b"iTXt", &itxt)]);
    let webp = webp(&[(b"VP8 ", &vec![0; 700_000]), (b"XMP ", XML)]);
    for bytes in [&jpeg, &png, &webp] {
        for ext in ["tif", "mp4"] {
            check_fixture(bytes, ext, Some(XML), true);
            assert!(legacy_extract_xmp_packet(&bytes[..FALLBACK_SCAN_LIMIT]).is_none());
        }
    }
}

#[test]
fn path_partial_read_propagates_dc_subject_io_errors() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.jpg");
    assert_eq!(
        try_read_dc_subject(&missing).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    assert!(read_dc_subject(&missing).is_empty());
    assert!(read_tweet_info(&missing).is_none());
    assert!(read_xmp_rating(&missing).is_none());
    assert!(read_panorama_info(&missing).is_none());
    assert!(read_xmp_bundle(&missing).tweet.is_none());
    assert!(read_xmp_packet_from_path(&missing).is_err());
    let invalid = dir.path().join("unknown.jpg");
    std::fs::write(&invalid, XML).unwrap();
    assert!(try_read_dc_subject(&invalid).unwrap().is_empty());
    assert_eq!(read_xmp_packet_from_path(&invalid).unwrap().bytes_read, 12);
}

#[test]
fn path_partial_read_keeps_absent_properties_empty() {
    for xml in [
        b"".as_slice(),
        b"<x:xmpmeta xmlns:x='adobe:ns:meta/'/>".as_slice(),
    ] {
        let mut bytes = vec![0xff, 0xd8];
        jpeg_xmp(&mut bytes, xml);
        check_fixture(&bytes, "jpg", Some(xml), true);
    }
}

#[test]
fn raw_extractor_keeps_unknown_magic_fallback_while_public_apis_reject_it() {
    let mut bytes = vec![0; 128];
    bytes.extend_from_slice(XML);
    assert_eq!(
        extract_xmp_packet(&bytes),
        legacy_extract_xmp_packet(&bytes)
    );
    assert_eq!(extract_xmp_packet(&bytes).as_deref(), Some(XML));
    assert!(read_tweet_info_from_bytes(&bytes).is_none());
    assert!(read_xmp_bundle_from_bytes(&bytes).tweet.is_none());
    assert!(read_xmp_bundle_from_bytes(&bytes).panorama.is_none());
    assert!(read_panorama_info_from_bytes(&bytes).is_none());
    assert!(read_xmp_rating_from_bytes(&bytes).is_none());
    assert!(read_dc_subject_from_bytes(&bytes).is_empty());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("unknown.jpg");
    std::fs::write(&path, &bytes).unwrap();
    assert!(read_tweet_info(&path).is_none());
    assert!(read_xmp_bundle(&path).tweet.is_none());
    assert!(read_xmp_bundle(&path).panorama.is_none());
    assert!(read_panorama_info(&path).is_none());
    assert!(read_xmp_rating(&path).is_none());
    assert!(try_read_dc_subject(&path).unwrap().is_empty());
    assert_eq!(read_xmp_packet_from_path(&path).unwrap().bytes_read, 12);
}

#[test]
fn partial_read_matches_legacy_at_every_truncation_boundary() {
    let mut jpeg = vec![0xff, 0xd8];
    jpeg_segment(&mut jpeg, 0xe1, b"Exif\0\0");
    jpeg_xmp(&mut jpeg, XML);
    jpeg_image(&mut jpeg, &[0; 8]);
    let itxt = png_text(XML, true, false);
    let png = png(&[(b"iTXt", &itxt), (b"IDAT", &[0; 8]), (b"IEND", &[])]);
    let webp = webp(&[(b"junk", b"odd"), (b"XMP ", XML), (b"VP8 ", &[0; 8])]);
    for (bytes, ext) in [(&jpeg, "jpg"), (&png, "png"), (&webp, "webp")] {
        for len in 0..=bytes.len() {
            let bytes = &bytes[..len];
            let expected = legacy_extract_xmp_packet(bytes);
            assert_eq!(extract_xmp_packet(bytes), expected, "{ext} length={len}");
        }
    }
}

/// Fail the read itself if the walker touches a skipped payload. This catches
/// accidental buffering/read-ahead as well as incorrect byte accounting.
struct AuditedReader<'a> {
    inner: Cursor<&'a [u8]>,
    forbidden: std::ops::Range<u64>,
    bytes_read: u64,
}

impl Read for AuditedReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let start = self.inner.position();
        let end = (start + buf.len() as u64).min(self.inner.get_ref().len() as u64);
        assert!(end <= self.forbidden.start || start >= self.forbidden.end);
        let n = self.inner.read(buf)?;
        self.bytes_read += n as u64;
        Ok(n)
    }
}

impl Seek for AuditedReader<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

#[test]
fn partial_read_never_reads_skipped_image_or_non_xmp_text_payloads() {
    let image = vec![0; 8 * 1024 * 1024];
    let itxt = png_text(XML, true, false);
    let mut jpeg = vec![0xff, 0xd8];
    jpeg_segment(&mut jpeg, 0xe1, &[0; 64]);
    jpeg_image(&mut jpeg, &image);
    let png_bytes = png(&[(b"IDAT", &image), (b"iTXt", &itxt)]);
    let webp_bytes = webp(&[(b"VP8 ", &image), (b"XMP ", XML)]);
    for (bytes, forbidden, expected) in [
        (&jpeg, 72..jpeg.len() as u64, None),
        (&png_bytes, 16..16 + image.len() as u64, Some(XML)),
        (&webp_bytes, 20..20 + image.len() as u64, Some(XML)),
    ] {
        let audited = AuditedReader {
            inner: Cursor::new(bytes.as_slice()),
            forbidden,
            bytes_read: 0,
        };
        let mut reader = XmpPacketReader {
            inner: audited,
            len: bytes.len() as u64,
            bytes_read: 0,
        };
        assert_eq!(reader.extract(true).unwrap().as_deref(), expected);
        assert_eq!(reader.bytes_read, reader.inner.bytes_read);
    }
    let mut junk = b"not-xmp\0".to_vec();
    junk.resize(1024 * 1024, 0);
    for kind in [b"tEXt", b"iTXt"] {
        let bytes = png(&[(b"IDAT", &[0; 16]), (kind, &junk)]);
        let audited = AuditedReader {
            inner: Cursor::new(bytes.as_slice()),
            forbidden: 44 + 80..44 + junk.len() as u64,
            bytes_read: 0,
        };
        let mut reader = XmpPacketReader {
            inner: audited,
            len: bytes.len() as u64,
            bytes_read: 0,
        };
        assert!(reader.extract(true).unwrap().is_none());
        assert_eq!(reader.bytes_read, reader.inner.bytes_read);
    }
}
