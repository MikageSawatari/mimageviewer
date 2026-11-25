use super::*;
use std::io::Write;

fn put16(bytes: &mut [u8], pos: usize, n: u16, little: bool) {
    bytes[pos..pos + 2].copy_from_slice(&if little {
        n.to_le_bytes()
    } else {
        n.to_be_bytes()
    });
}
fn put32(bytes: &mut [u8], pos: usize, n: u32, little: bool) {
    bytes[pos..pos + 4].copy_from_slice(&if little {
        n.to_le_bytes()
    } else {
        n.to_be_bytes()
    });
}
fn entry(bytes: &mut [u8], pos: usize, tag: u16, kind: u16, count: u32, value: u32, little: bool) {
    put16(bytes, pos, tag, little);
    put16(bytes, pos + 2, kind, little);
    put32(bytes, pos + 4, count, little);
    if kind == 3 && count == 1 {
        put16(bytes, pos + 8, value as u16, little);
    } else {
        put32(bytes, pos + 8, value, little);
    }
}

fn tiff(little: bool) -> Vec<u8> {
    let mut bytes = vec![0; 94];
    bytes[..2].copy_from_slice(if little { b"II" } else { b"MM" });
    put16(&mut bytes, 2, 42, little);
    put32(&mut bytes, 4, 8, little);
    put16(&mut bytes, 8, 2, little);
    entry(&mut bytes, 10, 0x0112, 3, 1, 6, little);
    entry(&mut bytes, 22, 0x8769, 4, 1, 38, little);
    put32(&mut bytes, 34, 56, little);
    put16(&mut bytes, 38, 1, little);
    entry(&mut bytes, 40, 0x927C, 7, 4, 0xAABBCCDD, little);
    put16(&mut bytes, 56, 2, little);
    entry(&mut bytes, 58, 0x0201, 4, 1, 86, little);
    entry(&mut bytes, 70, 0x0202, 4, 1, 8, little);
    bytes[86..].copy_from_slice(b"OLDIMAGE");
    bytes
}
fn app(marker: u8, payload: &[u8]) -> Vec<u8> {
    let mut result = vec![0xFF, marker];
    result.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    result.extend_from_slice(payload);
    result
}
fn compressed(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}
fn text(kind: &[u8; 4], bytes: &[u8], compressed_itxt: bool) -> Vec<u8> {
    let mut data = XMP_KEY.to_vec();
    data.push(0);
    match kind {
        b"tEXt" => data.extend_from_slice(bytes),
        b"zTXt" => {
            data.push(0);
            data.extend(compressed(bytes));
        }
        b"iTXt" => {
            data.extend_from_slice(&[u8::from(compressed_itxt), 0, 0, 0]);
            if compressed_itxt {
                data.extend(compressed(bytes));
            } else {
                data.extend_from_slice(bytes);
            }
        }
        _ => unreachable!(),
    }
    png_chunk(kind, &data)
}
fn options(hidden: bool) -> SaveOptions {
    SaveOptions {
        metadata_profile: MetadataProfile::Batch,
        thumbnail_notice_for_hidden_edits: hidden,
        ..Default::default()
    }
}
fn rgba() -> Vec<u8> {
    [100, 0, 0, 16, 12, 34, 56, 255].repeat(4)
}
fn encode(format: SrcFormat, src: Option<&[u8]>, opts: &SaveOptions) -> EncodedImage {
    encode_rgba_with_metadata(&rgba(), 4, 2, None, src, format, opts).unwrap()
}
fn bare(format: SrcFormat) -> Vec<u8> {
    encode(
        format,
        None,
        &SaveOptions {
            include_metadata: false,
            ..Default::default()
        },
    )
    .bytes
}
fn source(format: SrcFormat, exif: &[u8], xmp: &[u8], icc: &[u8]) -> Vec<u8> {
    let raw = bare(format.clone());
    match format {
        SrcFormat::Jpeg => {
            let mut payload = b"Exif\0\0".to_vec();
            payload.extend_from_slice(exif);
            let mut xp = XMP_STANDARD.to_vec();
            xp.extend_from_slice(xmp);
            let split = icc.len() / 2;
            let mut first = b"ICC_PROFILE\0\x01\x02".to_vec();
            first.extend_from_slice(&icc[..split]);
            let mut second = b"ICC_PROFILE\0\x02\x02".to_vec();
            second.extend_from_slice(&icc[split..]);
            splice_jpeg_app1_segments(
                &raw,
                &[
                    app(0xE1, &payload),
                    app(0xE1, &xp),
                    app(0xE2, &second),
                    app(0xE2, &first),
                ],
            )
            .unwrap()
        }
        SrcFormat::Png => {
            let mut profile = b"profile\0\0".to_vec();
            profile.extend(compressed(icc));
            splice_png_text_chunks(
                &raw,
                &[
                    png_chunk(b"eXIf", exif),
                    png_chunk(b"iCCP", &profile),
                    text(b"iTXt", xmp, false),
                ],
            )
            .unwrap()
        }
        SrcFormat::Webp => inject_webp_metadata(
            &raw,
            Some(icc.to_vec()),
            Some(exif.to_vec()),
            Some(xmp.to_vec()),
        )
        .unwrap(),
        _ => unreachable!(),
    }
}
fn profile(space: &[u8; 4]) -> Vec<u8> {
    let mut p = vec![0; 128];
    p[16..20].copy_from_slice(space);
    p
}
fn output_exif(format: &SrcFormat, bytes: &[u8]) -> Option<Vec<u8>> {
    match format {
        SrcFormat::Jpeg => extract_jpeg_app1_segments(bytes)
            .unwrap()
            .into_iter()
            .find_map(|s| s[4..].strip_prefix(b"Exif\0\0").map(|b| b.to_vec())),
        SrcFormat::Png => extract_png_metadata_chunks(bytes)
            .unwrap()
            .into_iter()
            .find(|c| &c[4..8] == b"eXIf")
            .map(|c| c[8..c.len() - 4].to_vec()),
        SrcFormat::Webp => extract_webp_metadata_chunks(bytes)
            .unwrap()
            .1
            .map(|p| p.strip_prefix(b"Exif\0\0").unwrap_or(&p).to_vec()),
        _ => unreachable!(),
    }
}

#[test]
fn batch_exif_normalizes_both_endians_in_all_three_formats() {
    for little in [true, false] {
        for format in [SrcFormat::Jpeg, SrcFormat::Png, SrcFormat::Webp] {
            let src = source(
                format.clone(),
                &tiff(little),
                XMP_IMAGE_NS,
                &profile(b"RGB "),
            );
            let result = encode(format.clone(), Some(&src), &options(true));
            let out = output_exif(&format, &result.bytes).unwrap();
            let t = Tiff { data: &out, little };
            assert_eq!(t.u16(18), Some(1));
            assert_eq!(t.u32(34), Some(0));
            assert_eq!(&out[86..], &[0; 8]);
            assert_eq!(&out[48..52], &tiff(little)[48..52]); // MakerNote unchanged
            assert!(result.thumbnail_notice.xmp && result.thumbnail_notice.maker_note);
            match format {
                SrcFormat::Jpeg => {
                    let segments = extract_jpeg_app1_segments(&result.bytes).unwrap();
                    assert!(
                        segments
                            .iter()
                            .any(|s| s[4..].strip_prefix(XMP_STANDARD) == Some(XMP_IMAGE_NS))
                    );
                    assert_eq!(jpeg_rgb_icc_segments(&result.bytes).unwrap().len(), 2);
                    assert_eq!(
                        extract_jpeg_segments(&result.bytes, 0xE2)
                            .unwrap()
                            .iter()
                            .map(|s| s[16])
                            .collect::<Vec<_>>(),
                        vec![1, 2]
                    );
                }
                SrcFormat::Png => assert!(
                    extract_png_metadata_chunks(&result.bytes)
                        .unwrap()
                        .contains(&text(b"iTXt", XMP_IMAGE_NS, false))
                ),
                SrcFormat::Webp => assert_eq!(
                    extract_webp_metadata_chunks(&result.bytes)
                        .unwrap()
                        .2
                        .as_deref(),
                    Some(XMP_IMAGE_NS)
                ),
                _ => unreachable!(),
            }
        }
    }
}

#[test]
fn batch_drops_invalid_exif_without_failing_export() {
    for invalid in [b"not TIFF".to_vec(), tiff(true)[..42].to_vec(), {
        let mut b = tiff(true);
        put32(&mut b, 78, u32::MAX, true);
        b
    }] {
        for format in [SrcFormat::Jpeg, SrcFormat::Png, SrcFormat::Webp] {
            let src = source(format.clone(), &invalid, b"no thumbnail", &profile(b"RGB "));
            let result = encode(format.clone(), Some(&src), &options(true));
            assert!(output_exif(&format, &result.bytes).is_none());
            assert!(result.thumbnail_notice.is_empty());
        }
    }
}

#[test]
fn batch_icc_accepts_rgb_and_drops_gray_cmyk_in_all_formats() {
    for color in [b"RGB ", b"GRAY", b"CMYK"] {
        for format in [SrcFormat::Jpeg, SrcFormat::Png, SrcFormat::Webp] {
            let src = source(format.clone(), &tiff(true), b"none", &profile(color));
            let result = encode(format.clone(), Some(&src), &options(false));
            let has_icc = match format {
                SrcFormat::Jpeg => !extract_jpeg_segments(&result.bytes, 0xE2)
                    .unwrap()
                    .is_empty(),
                SrcFormat::Png => extract_png_metadata_chunks(&result.bytes)
                    .unwrap()
                    .iter()
                    .any(|c| &c[4..8] == b"iCCP"),
                SrcFormat::Webp => extract_webp_metadata_chunks(&result.bytes)
                    .unwrap()
                    .0
                    .is_some(),
                _ => unreachable!(),
            };
            assert_eq!(has_icc, color == b"RGB ");
        }
    }
}

#[test]
fn maker_note_notice_requires_hidden_edits_and_preserved_metadata() {
    for format in [SrcFormat::Jpeg, SrcFormat::Png, SrcFormat::Webp] {
        let src = source(
            format.clone(),
            &tiff(true),
            b"no thumbnail",
            &profile(b"RGB "),
        );
        assert!(
            encode(format.clone(), Some(&src), &options(false))
                .thumbnail_notice
                .is_empty()
        );
        assert!(
            encode(format.clone(), Some(&src), &options(true))
                .thumbnail_notice
                .maker_note
        );
        let off = SaveOptions {
            include_metadata: false,
            ..options(true)
        };
        let result = encode(format.clone(), Some(&src), &off);
        assert!(result.thumbnail_notice.is_empty());
        assert!(output_exif(&format, &result.bytes).is_none());
        assert!(
            encode(format, None, &options(true))
                .thumbnail_notice
                .is_empty()
        );
    }
}

#[test]
fn webp_exif_optional_prefix_is_preserved_while_normalizing() {
    let mut exif = b"Exif\0\0".to_vec();
    exif.extend(tiff(true));
    let src = inject_webp_metadata(&bare(SrcFormat::Webp), None, Some(exif), None).unwrap();
    let out = encode(SrcFormat::Webp, Some(&src), &options(false));
    let payload = extract_webp_metadata_chunks(&out.bytes).unwrap().1.unwrap();
    assert!(payload.starts_with(b"Exif\0\0"));
    assert_eq!(&payload[6 + 86..], &[0; 8]);
}

#[test]
fn png_detects_every_xmp_text_variant_even_after_other_text() {
    for (kind, compression) in [
        (b"tEXt", false),
        (b"zTXt", false),
        (b"iTXt", false),
        (b"iTXt", true),
    ] {
        let target = text(kind, XMP_IMAGE_NS, compression);
        let before = vec![
            png_chunk(b"tEXt", b"prompt\0other text"),
            text(b"iTXt", b"ordinary XMP", false),
            target.clone(),
        ];
        let src = splice_png_text_chunks(&bare(SrcFormat::Png), &before).unwrap();
        let result = encode(SrcFormat::Png, Some(&src), &options(false));
        assert!(result.thumbnail_notice.xmp);
        assert!(!result.thumbnail_notice.xmp_uninspectable);
        assert_eq!(extract_png_text_chunks_raw(&result.bytes).unwrap(), before);
    }
}

#[test]
fn png_unreadable_and_oversize_xmp_warn_without_changing_chunks() {
    let mut invalid = XMP_KEY.to_vec();
    invalid.extend_from_slice(b"\0\0broken zlib");
    for target in [
        png_chunk(b"zTXt", &invalid),
        text(b"zTXt", &vec![b'x'; CHUNK_LIMIT + 1], false),
    ] {
        let src = splice_png_text_chunks(&bare(SrcFormat::Png), &[target.clone()]).unwrap();
        let result = encode(SrcFormat::Png, Some(&src), &options(false));
        assert!(result.thumbnail_notice.xmp_uninspectable);
        assert_eq!(
            extract_png_text_chunks_raw(&result.bytes).unwrap(),
            vec![target]
        );
    }
}

#[test]
fn png_xmp_total_budget_marks_later_chunks_uninspectable() {
    let chunk = text(b"zTXt", &vec![b'x'; CHUNK_LIMIT], false);
    let src = splice_png_text_chunks(
        &bare(SrcFormat::Png),
        &[chunk.clone(), chunk, text(b"iTXt", XMP_IMAGE_NS, true)],
    )
    .unwrap();
    let result = encode(SrcFormat::Png, Some(&src), &options(false));
    assert!(result.thumbnail_notice.xmp_uninspectable);
    assert!(!result.thumbnail_notice.xmp); // third chunk was not inspected
}

fn extended(guid: &[u8; 32], offset: u32, body: &[u8]) -> Vec<u8> {
    let mut p = XMP_EXTENDED.to_vec();
    p.extend_from_slice(guid);
    p.extend_from_slice(&u32::MAX.to_be_bytes()); // never allocate this claimed total
    p.extend_from_slice(&offset.to_be_bytes());
    p.extend_from_slice(body);
    app(0xE1, &p)
}

#[test]
fn jpeg_extended_xmp_groups_guids_and_sorts_present_fragments() {
    let split = 15;
    let a = [b'A'; 32];
    let b = [b'B'; 32];
    let segments = vec![
        extended(&a, 200, &XMP_IMAGE_NS[split..]),
        extended(&b, 100, b"other GUID"),
        extended(&a, 0, &XMP_IMAGE_NS[..split]),
    ];
    let src = splice_jpeg_app1_segments(&bare(SrcFormat::Jpeg), &segments).unwrap();
    let result = encode(SrcFormat::Jpeg, Some(&src), &options(false));
    assert!(result.thumbnail_notice.xmp);
    assert_eq!(extract_jpeg_app1_segments(&result.bytes).unwrap(), segments);
    let separate_guids = vec![
        extended(&a, 0, &XMP_IMAGE_NS[..split]),
        extended(&b, 0, &XMP_IMAGE_NS[split..]),
    ];
    assert!(!jpeg_xmp_thumbnail(&separate_guids));
}

#[test]
fn jpeg_extended_xmp_short_header_checks_available_body() {
    let mut payload = XMP_EXTENDED.to_vec();
    payload.extend_from_slice(XMP_IMAGE_NS);
    assert!(jpeg_xmp_thumbnail(&[app(0xE1, &payload)]));
    assert!(!jpeg_xmp_thumbnail(&[app(0xE1, XMP_STANDARD)]));
}

#[test]
fn png_raw_app1_extended_xmp_joins_same_guid_fragments_and_preserves_chunks() {
    let a = [b'A'; 32];
    let b = [b'B'; 32];
    let split = 15;
    let first = extended(&a, 0, &XMP_IMAGE_NS[..split]);
    let second = extended(&a, 200, &XMP_IMAGE_NS[split..]);
    let other = extended(&b, 100, b"another GUID");
    let different_guid = extended(&b, 0, &XMP_IMAGE_NS[split..]);
    let mut short = XMP_EXTENDED.to_vec();
    short.extend_from_slice(XMP_IMAGE_NS);
    let short = app(0xE1, &short);
    for opts in [single_options(false), options(false)] {
        for (fragments, expected) in [
            (vec![&second, &other, &first], true),
            (vec![&first, &different_guid], false),
            (vec![&short], true),
        ] {
            let chunks: Vec<_> = fragments
                .iter()
                .enumerate()
                .map(|(i, segment)| {
                    let payload = &segment[4..];
                    let kind = [b"tEXt", b"zTXt", b"iTXt"][i % 3];
                    raw_text(
                        kind,
                        b"Raw profile type APP1",
                        &raw_body(payload, payload.len()),
                        true,
                    )
                })
                .collect();
            let src = splice_png_text_chunks(&bare(SrcFormat::Png), &chunks).unwrap();
            let result = encode(SrcFormat::Png, Some(&src), &opts);
            assert_eq!(result.thumbnail_notice.xmp, expected);
            assert!(!result.thumbnail_notice.xmp_uninspectable);
            assert!(!result.thumbnail_notice.exif_uninspectable);
            assert_eq!(extract_png_text_chunks_raw(&result.bytes).unwrap(), chunks);
        }
    }
}

#[test]
fn png_raw_app1_unreadable_xmp_keeps_chunks_and_warns_in_both_profiles() {
    for opts in [single_options(false), options(false)] {
        for header in [XMP_STANDARD, XMP_EXTENDED] {
            let payload = [header, XMP_IMAGE_NS].concat();
            let short_length = raw_body(&payload, 1);
            let long_length = raw_body(&payload, payload.len() + 1);
            let mut bad_tail = raw_body(&payload, payload.len());
            bad_tail.extend_from_slice(b"zz");
            let mut odd_tail = raw_body(&payload, payload.len());
            odd_tail.push(b'f');
            // An invalid decimal declaration must not hide a recognizable XMP header.
            let invalid_length = raw_body(&payload, payload.len());
            let length_line = format!("{:8}", payload.len());
            let invalid_length = String::from_utf8(invalid_length)
                .unwrap()
                .replacen(length_line.as_str(), "invalid length", 1)
                .into_bytes();
            for body in [
                short_length,
                long_length,
                bad_tail,
                odd_tail,
                invalid_length,
            ] {
                for (kind, compression) in [
                    (b"tEXt", false),
                    (b"zTXt", true),
                    (b"iTXt", false),
                    (b"iTXt", true),
                ] {
                    let chunk = raw_text(kind, b"rAw PrOfIlE tYpE aPp1", &body, compression);
                    let src =
                        splice_png_text_chunks(&bare(SrcFormat::Png), &[chunk.clone()]).unwrap();
                    let result = encode(SrcFormat::Png, Some(&src), &opts);
                    assert_eq!(
                        result.thumbnail_notice,
                        ThumbnailNoticeKinds {
                            xmp_uninspectable: true,
                            ..Default::default()
                        }
                    );
                    assert_eq!(
                        extract_png_text_chunks_raw(&result.bytes).unwrap(),
                        vec![chunk]
                    );
                }
            }
        }
    }
}

#[test]
fn exif_stripped_thumbnail_ranges_are_erased_and_malformed_offsets_rejected() {
    let mut bytes = tiff(true);
    put16(&mut bytes, 58, 0x0111, true);
    put16(&mut bytes, 70, 0x0117, true);
    assert_eq!(&clean_exif(&bytes, true).unwrap().0[86..], &[0; 8]);
    // A thumbnail must not overwrite a retained MakerNote or any IFD structure.
    put32(&mut bytes, 66, 48, true);
    assert!(clean_exif(&bytes, true).is_none());
    let mut cyclic = tiff(true);
    put32(&mut cyclic, 52, 8, true);
    assert!(clean_exif(&cyclic, true).is_none());
}

#[test]
fn rgba_png_written_file_retains_low_alpha_straight_color() {
    let out = encode_rgba_with_metadata(
        &[100, 0, 0, 16],
        1,
        1,
        None,
        None,
        SrcFormat::Png,
        &options(false),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("straight.png");
    write_bytes_create_new(&out.bytes, &path).unwrap();
    let reloaded = image::open(path).unwrap().to_rgba8();
    assert_eq!(reloaded.get_pixel(0, 0).0, [100, 0, 0, 16]);
}

#[test]
fn rgba_entry_rejects_inconsistent_and_zero_dimensions() {
    for (bytes, w, h) in [
        (&[0u8; 3][..], 1, 1),
        (&[][..], 0, 1),
        (&[][..], u32::MAX, u32::MAX),
    ] {
        assert!(matches!(
            encode_rgba_with_metadata(
                bytes,
                w,
                h,
                None,
                None,
                SrcFormat::Png,
                &SaveOptions::default()
            ),
            Err(SaveError::InvalidPixels(_))
        ));
    }
}

#[test]
fn single_page_metadata_bytes_match_legacy_except_ifd1() {
    assert_eq!(
        SaveOptions::default().metadata_profile,
        MetadataProfile::Single
    );
    assert!(!SaveOptions::default().thumbnail_notice_for_hidden_edits);
    let img = ColorImage::from_rgba_unmultiplied([4, 2], &rgba());
    let input = color_image_to_rgba(&img);
    for format in [SrcFormat::Jpeg, SrcFormat::Png, SrcFormat::Webp] {
        for lossless in [false, true] {
            let opts = SaveOptions {
                webp_lossless: lossless,
                ..Default::default()
            };
            let src = source(format.clone(), &tiff(true), XMP_IMAGE_NS, &profile(b"RGB "));
            // The old encoding/transfer sequence is the comparison oracle.
            let mut legacy = match format {
                SrcFormat::Jpeg => {
                    let rgb = flatten_rgba_to_rgb(&input, 4, opts.jpeg_matte);
                    let image = image::RgbImage::from_raw(4, 2, rgb).unwrap();
                    let bytes =
                        turbojpeg::compress_image(&image, 95, opts.jpeg_subsampling).unwrap();
                    let mut segments = extract_jpeg_app1_segments(&src).unwrap();
                    for seg in &mut segments {
                        reset_exif_orientation_in_app1(seg);
                    }
                    splice_jpeg_app1_segments(bytes.as_ref(), &segments).unwrap()
                }
                SrcFormat::Png => {
                    use image::ImageEncoder;
                    let mut bytes = Vec::new();
                    image::codecs::png::PngEncoder::new(&mut bytes)
                        .write_image(&input, 4, 2, image::ColorType::Rgba8.into())
                        .unwrap();
                    splice_png_text_chunks(&bytes, &extract_png_text_chunks_raw(&src).unwrap())
                        .unwrap()
                }
                SrcFormat::Webp => {
                    let encoder = webp::Encoder::from_rgba(&input, 4, 2);
                    let raw = if lossless {
                        encoder.encode_lossless()
                    } else {
                        encoder.encode(90.0)
                    };
                    let (icc, exif, xmp) = extract_webp_metadata_chunks(&src).unwrap();
                    inject_webp_metadata(raw.as_ref(), icc, exif, xmp).unwrap()
                }
                _ => unreachable!(),
            };
            // Only IFD1 and its image bytes differ from the released transfer.
            match format {
                SrcFormat::Jpeg => {
                    let mut segments = extract_jpeg_app1_segments(&legacy).unwrap();
                    for seg in &mut segments {
                        if seg[4..].starts_with(b"Exif\0\0") {
                            seg[10 + 34..10 + 38].fill(0);
                            seg[10 + 86..].fill(0);
                        }
                    }
                    legacy = splice_jpeg_app1_segments(&legacy, &segments).unwrap();
                }
                SrcFormat::Webp => {
                    let (icc, mut exif, xmp) = extract_webp_metadata_chunks(&legacy).unwrap();
                    let exif = exif.as_mut().unwrap();
                    exif[34..38].fill(0);
                    exif[86..].fill(0);
                    legacy = inject_webp_metadata(&legacy, icc, Some(exif.clone()), xmp).unwrap();
                }
                _ => {}
            }
            let wrapped =
                encode_image_with_metadata_report(&img, None, Some(&src), format.clone(), &opts)
                    .unwrap();
            assert_eq!(wrapped.bytes, legacy, "{format:?}, lossless={lossless}");
            assert!(wrapped.thumbnail_notice.xmp);
            assert!(!wrapped.thumbnail_notice.exif_uninspectable);
            let straight =
                encode_rgba_with_metadata(&input, 4, 2, None, Some(&src), format.clone(), &opts)
                    .unwrap();
            assert_eq!(straight.bytes, legacy);
        }
    }
}

#[test]
fn metadata_disabled_never_detects_preserved_thumbnail_or_maker_note() {
    for format in [SrcFormat::Jpeg, SrcFormat::Png, SrcFormat::Webp] {
        let src = source(format.clone(), &tiff(true), XMP_IMAGE_NS, &profile(b"RGB "));
        for base in [options(true), single_options(true)] {
            let opts = SaveOptions {
                include_metadata: false,
                ..base
            };
            let result = encode(format.clone(), Some(&src), &opts);
            assert!(result.thumbnail_notice.is_empty());
            assert_eq!(result.bytes, bare(format.clone()));
        }
    }
}

#[test]
fn exif_without_maker_note_does_not_warn_for_hidden_edits() {
    let mut original = tiff(true);
    put16(&mut original, 40, 0x9286, true); // EXIF UserComment, no MakerNote
    for format in [SrcFormat::Jpeg, SrcFormat::Png, SrcFormat::Webp] {
        let src = source(
            format.clone(),
            &original,
            b"ordinary XMP",
            &profile(b"RGB "),
        );
        let result = encode(format, Some(&src), &options(true));
        assert!(result.thumbnail_notice.is_empty());
    }
}

#[test]
fn png_bad_crc_extended_metadata_is_neither_copied_nor_inspected() {
    let mut exif = png_chunk(b"eXIf", &tiff(true));
    *exif.last_mut().unwrap() ^= 1;
    let mut xmp = text(b"iTXt", XMP_IMAGE_NS, true);
    *xmp.last_mut().unwrap() ^= 1;
    let src = splice_png_text_chunks(&bare(SrcFormat::Png), &[exif, xmp]).unwrap();
    let result = encode(SrcFormat::Png, Some(&src), &options(true));
    assert!(result.thumbnail_notice.is_empty());
    assert!(
        extract_png_metadata_chunks(&result.bytes)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn source_bytes_take_precedence_over_nonexistent_source_path() {
    let src = source(SrcFormat::Png, &tiff(true), XMP_IMAGE_NS, &profile(b"RGB "));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("does-not-exist.png");
    let result = encode_rgba_with_metadata(
        &rgba(),
        4,
        2,
        Some(&path),
        Some(&src),
        SrcFormat::Png,
        &options(false),
    )
    .unwrap();
    assert!(result.thumbnail_notice.xmp);
}

#[test]
fn jpeg_incomplete_or_duplicate_icc_segments_are_dropped() {
    let mut p = b"ICC_PROFILE\0\x01\x02".to_vec();
    p.extend(profile(b"RGB "));
    for chunks in [vec![app(0xE2, &p)], vec![app(0xE2, &p), app(0xE2, &p)]] {
        let src = splice_jpeg_app1_segments(&bare(SrcFormat::Jpeg), &chunks).unwrap();
        let result = encode(SrcFormat::Jpeg, Some(&src), &options(false));
        assert!(
            extract_jpeg_segments(&result.bytes, 0xE2)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn webp_encoder_reports_dimension_limit_failure_without_panicking() {
    let bytes = vec![0; 16384 * 4];
    let result = encode_rgba_with_metadata(
        &bytes,
        16384,
        1,
        None,
        None,
        SrcFormat::Webp,
        &SaveOptions::default(),
    );
    assert!(matches!(result, Err(SaveError::EncodingFailed(_))));
}

fn single_options(hidden: bool) -> SaveOptions {
    SaveOptions {
        thumbnail_notice_for_hidden_edits: hidden,
        ..Default::default()
    }
}

#[test]
fn single_thumbnail_cleanup_preserves_format_selection_orientation_and_notices() {
    for little in [true, false] {
        for format in [SrcFormat::Jpeg, SrcFormat::Png, SrcFormat::Webp] {
            for xmp in [XMP_IMAGE_NS, &b"ordinary XMP"[..]] {
                for hidden in [false, true] {
                    let src = source(format.clone(), &tiff(little), xmp, &profile(b"GRAY"));
                    let result = encode(format.clone(), Some(&src), &single_options(hidden));
                    assert_eq!(result.thumbnail_notice.xmp, xmp == XMP_IMAGE_NS);
                    assert!(!result.thumbnail_notice.exif_uninspectable);
                    assert_eq!(
                        result.thumbnail_notice.maker_note,
                        hidden && format != SrcFormat::Png
                    );
                    if format == SrcFormat::Png {
                        assert_eq!(
                            extract_png_metadata_chunks(&result.bytes).unwrap(),
                            extract_png_text_chunks_raw(&src).unwrap()
                        );
                        continue;
                    }
                    let out = output_exif(&format, &result.bytes).unwrap();
                    let t = Tiff { data: &out, little };
                    assert_eq!(
                        t.u16(18),
                        Some(if format == SrcFormat::Jpeg { 1 } else { 6 })
                    );
                    assert_eq!(t.u32(34), Some(0));
                    assert_eq!(&out[86..], &[0; 8]);
                    if format == SrcFormat::Webp {
                        assert_eq!(
                            extract_webp_metadata_chunks(&result.bytes).unwrap().0,
                            Some(profile(b"GRAY"))
                        );
                    } else {
                        assert!(
                            extract_jpeg_segments(&result.bytes, 0xE2)
                                .unwrap()
                                .is_empty()
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn single_jpeg_only_resets_ifd0_orientation_and_keeps_other_bytes() {
    for little in [true, false] {
        let mut original = tiff(little);
        entry(&mut original, 40, 0x0112, 3, 1, 8, little);
        let mut old = b"Exif\0\0".to_vec();
        old.extend(&original);
        let mut old = app(0xE1, &old);
        reset_exif_orientation_in_app1(&mut old);
        let src = source(SrcFormat::Jpeg, &original, b"ordinary", &profile(b"RGB "));
        for applied in [false, true] {
            let opts = SaveOptions {
                caller_applied_orientation: applied,
                ..single_options(false)
            };
            let result = encode(SrcFormat::Jpeg, Some(&src), &opts);
            let out = output_exif(&SrcFormat::Jpeg, &result.bytes).unwrap();
            let mut expected = if applied {
                old[10..].to_vec()
            } else {
                original.clone()
            };
            expected[34..38].fill(0);
            expected[86..].fill(0);
            assert_eq!(out, expected);
            assert_eq!(Tiff { data: &out, little }.u16(48), Some(8));
        }
    }
}

#[test]
fn single_clean_exif_does_not_reject_malformed_orientation() {
    for (kind, count, value) in [(4, 1, 6), (99, 100, u32::MAX), (3, 9, u32::MAX)] {
        let mut original = tiff(true);
        entry(&mut original, 10, 0x0112, kind, count, value, true);
        assert!(clean_exif(&original, true).is_none());
        let out = clean_exif(&original, false).unwrap().0;
        assert_eq!(&out[10..22], &original[10..22]);
        assert_eq!(&out[34..38], &[0; 4]);
        assert_eq!(&out[86..], &[0; 8]);
    }
}

#[test]
fn single_unreadable_exif_keeps_legacy_bytes_and_warns_batch_drops() {
    let mut cyclic = tiff(true);
    put32(&mut cyclic, 52, 8, true);
    for original in [b"not TIFF".to_vec(), cyclic] {
        for format in [SrcFormat::Jpeg, SrcFormat::Webp] {
            let src = source(format.clone(), &original, b"ordinary", &profile(b"RGB "));
            let result = encode(format.clone(), Some(&src), &single_options(true));
            let mut expected = original.clone();
            if format == SrcFormat::Jpeg {
                let mut payload = b"Exif\0\0".to_vec();
                payload.extend(&original);
                let mut seg = app(0xE1, &payload);
                reset_exif_orientation_in_app1(&mut seg);
                expected = seg[10..].to_vec();
            }
            assert_eq!(output_exif(&format, &result.bytes), Some(expected));
            assert!(result.thumbnail_notice.exif_uninspectable);
            assert!(!result.thumbnail_notice.maker_note);
            let batch = encode(format.clone(), Some(&src), &options(true));
            assert!(output_exif(&format, &batch.bytes).is_none());
            assert!(batch.thumbnail_notice.is_empty());
        }
    }
}

fn raw_body(bytes: &[u8], declared: usize) -> Vec<u8> {
    let mut body = format!("\nexif\n  {declared:8}\n").into_bytes();
    for (i, byte) in bytes.iter().enumerate() {
        let digits = if i % 2 == 0 {
            format!("{byte:02X}")
        } else {
            format!("{byte:02x}")
        };
        body.extend_from_slice(digits.as_bytes());
        body.extend_from_slice(if i % 7 == 6 { b"\r\n" } else { b" " });
    }
    body
}

fn raw_text(kind: &[u8; 4], key: &[u8], body: &[u8], compression: bool) -> Vec<u8> {
    let mut data = key.to_vec();
    data.push(0);
    let compressed = kind == b"zTXt" || (kind == b"iTXt" && compression);
    match kind {
        b"zTXt" => data.push(0),
        b"iTXt" => {
            data.extend_from_slice(&[u8::from(compression), 0]);
            data.extend_from_slice("ja\0元の訳語\0".as_bytes());
        }
        _ => {}
    }
    data.extend(if compressed {
        self::compressed(body)
    } else {
        body.to_vec()
    });
    png_chunk(kind, &data)
}

fn text_body(chunk: &[u8]) -> Vec<u8> {
    let data = &chunk[8..chunk.len() - 4];
    let key_end = data.iter().position(|&b| b == 0).unwrap();
    let (start, compressed) = png_text_layout(chunk, key_end).unwrap();
    decode_png_text(&data[start..], compressed, &mut 0).unwrap()
}

#[test]
fn png_raw_exif_patches_only_hex_and_preserves_text_layout_after_xmp_notice() {
    for opts in [single_options(true), options(true)] {
        for (kind, compression) in [
            (b"tEXt", false),
            (b"zTXt", true),
            (b"iTXt", false),
            (b"iTXt", true),
        ] {
            for key in [&b"Raw profile type exif"[..], &b"rAW pROFILE tYPE aPP1"[..]] {
                let mut payload = b"Exif\0\0".to_vec();
                payload.extend(tiff(true));
                let body = raw_body(&payload, payload.len());
                let raw = raw_text(kind, key, &body, compression);
                let src = splice_png_text_chunks(
                    &bare(SrcFormat::Png),
                    &[text(b"iTXt", XMP_IMAGE_NS, true), raw.clone()],
                )
                .unwrap();
                let result = encode(SrcFormat::Png, Some(&src), &opts);
                assert!(result.thumbnail_notice.xmp && result.thumbnail_notice.maker_note);
                assert!(!result.thumbnail_notice.exif_uninspectable);
                let chunks = extract_png_text_chunks_raw(&result.bytes).unwrap();
                assert_eq!(chunks.len(), 2); // also verifies the rewritten CRC
                let out = &chunks[1];
                let data = &raw[8..raw.len() - 4];
                let end = data.iter().position(|&b| b == 0).unwrap();
                let (start, _) = png_text_layout(&raw, end).unwrap();
                assert_eq!(&out[8..8 + start], &raw[8..8 + start]);
                let decoded = text_body(out);
                let (mut expected, positions) = raw_profile(&body).unwrap();
                let original = expected.clone();
                expected[6 + 34..6 + 38].fill(0);
                expected[6 + 86..].fill(0);
                if opts.metadata_profile == MetadataProfile::Batch {
                    expected[6 + 18] = 1;
                }
                let mut expected_body = body.clone();
                for (i, (&before, &after)) in original.iter().zip(&expected).enumerate() {
                    if before != after {
                        let digits = format!("{after:02x}").into_bytes();
                        expected_body[positions[i * 2]] = digits[0];
                        expected_body[positions[i * 2 + 1]] = digits[1];
                    }
                }
                assert_eq!(decoded, expected_body);
                assert_eq!(raw_profile(&decoded).unwrap().0, expected);
                for (i, (&before, &after)) in body.iter().zip(&decoded).enumerate() {
                    if before != after {
                        assert!(positions.contains(&i));
                    }
                }
            }
        }
    }
}

#[test]
fn png_raw_xmp_and_app1_xmp_detect_thumbnail_without_changing_bytes() {
    for opts in [single_options(false), options(false)] {
        for (key, prefix) in [
            (&b"Raw profile type xmp"[..], &b""[..]),
            (&b"Raw profile type APP1"[..], XMP_STANDARD),
        ] {
            for xmp in [XMP_IMAGE_NS, &b"ordinary"[..]] {
                let bytes = [prefix, xmp].concat();
                for (kind, compression) in [
                    (b"tEXt", false),
                    (b"zTXt", true),
                    (b"iTXt", false),
                    (b"iTXt", true),
                ] {
                    let raw = raw_text(kind, key, &raw_body(&bytes, bytes.len()), compression);
                    let src =
                        splice_png_text_chunks(&bare(SrcFormat::Png), &[raw.clone()]).unwrap();
                    let result = encode(SrcFormat::Png, Some(&src), &opts);
                    assert_eq!(result.thumbnail_notice.xmp, xmp == XMP_IMAGE_NS);
                    assert!(!result.thumbnail_notice.xmp_uninspectable);
                    assert_eq!(
                        extract_png_text_chunks_raw(&result.bytes).unwrap(),
                        vec![raw]
                    );
                }
            }
        }
    }
}

#[test]
fn png_unreadable_raw_profiles_follow_single_batch_retention_rules() {
    for opts in [single_options(false), options(false)] {
        for key in [
            &b"Raw profile type exif"[..],
            &b"Raw profile type APP1"[..],
            &b"Raw profile type xmp"[..],
        ] {
            let mut cyclic = tiff(true);
            put32(&mut cyclic, 52, 8, true);
            for (body, valid_hex) in [
                (b"\nexif\n4\nnot hex!".to_vec(), false),
                (raw_body(XMP_IMAGE_NS, 2), false),
                (raw_body(&tiff(true), 2), false),
                (raw_body(&tiff(true), tiff(true).len() + 1), false),
                (b"not an ImageMagick profile".to_vec(), false),
                (raw_body(b"not TIFF", 8), true),
                (raw_body(&cyclic, cyclic.len()), true),
            ] {
                for kind in [b"tEXt", b"zTXt", b"iTXt"] {
                    let raw = raw_text(kind, key, &body, true);
                    let src =
                        splice_png_text_chunks(&bare(SrcFormat::Png), &[raw.clone()]).unwrap();
                    let result = encode(SrcFormat::Png, Some(&src), &opts);
                    let xmp = key.ends_with(b"xmp");
                    let retained = xmp || opts.metadata_profile == MetadataProfile::Single;
                    // A well-formed hex raw XMP body need not contain XML or the namespace.
                    let valid_raw_xmp = xmp && valid_hex;
                    assert_eq!(
                        result.thumbnail_notice.xmp_uninspectable,
                        xmp && !valid_raw_xmp
                    );
                    assert_eq!(result.thumbnail_notice.exif_uninspectable, !xmp && retained);
                    assert_eq!(
                        extract_png_text_chunks_raw(&result.bytes).unwrap(),
                        if retained { vec![raw] } else { vec![] }
                    );
                }
            }
        }
    }
}

#[test]
fn png_raw_exif_shares_xmp_total_budget_and_chunk_limit() {
    let filler = text(b"zTXt", &vec![b'x'; CHUNK_LIMIT], false);
    let body = raw_body(&tiff(true), tiff(true).len());
    for opts in [single_options(false), options(false)] {
        for oversized in [false, true] {
            let content = if oversized {
                vec![b'x'; CHUNK_LIMIT + 1]
            } else {
                body.clone()
            };
            let raw = raw_text(b"zTXt", b"Raw profile type exif", &content, true);
            let mut chunks = if oversized {
                vec![]
            } else {
                vec![filler.clone(), filler.clone()]
            };
            chunks.push(raw.clone());
            let src = splice_png_text_chunks(&bare(SrcFormat::Png), &chunks).unwrap();
            let result = encode(SrcFormat::Png, Some(&src), &opts);
            let single = opts.metadata_profile == MetadataProfile::Single;
            assert_eq!(result.thumbnail_notice.exif_uninspectable, single);
            if !single {
                chunks.pop();
            }
            assert_eq!(extract_png_text_chunks_raw(&result.bytes).unwrap(), chunks);
        }
    }
}

#[test]
fn png_raw_exif_accepts_tiff_without_exif_prefix() {
    for opts in [single_options(true), options(true)] {
        let raw = raw_text(
            b"tEXt",
            b"Raw profile type exif",
            &raw_body(&tiff(true), 94),
            false,
        );
        let src = splice_png_text_chunks(&bare(SrcFormat::Png), &[raw]).unwrap();
        let result = encode(SrcFormat::Png, Some(&src), &opts);
        let chunks = extract_png_text_chunks_raw(&result.bytes).unwrap();
        let out = raw_profile(&text_body(&chunks[0])).unwrap().0;
        assert_eq!(&out[34..38], &[0; 4]);
        assert_eq!(&out[86..94], &[0; 8]);
        assert_eq!(
            out[18],
            if opts.metadata_profile == MetadataProfile::Single {
                6
            } else {
                1
            }
        );
        assert!(result.thumbnail_notice.maker_note);
    }
}

#[test]
fn png_unreadable_raw_text_compression_keeps_xmp_and_profiles_exif() {
    for opts in [single_options(false), options(false)] {
        for key in [&b"Raw profile type exif"[..], &b"Raw profile type xmp"[..]] {
            let mut invalid = key.to_vec();
            invalid.extend_from_slice(b"\0\0broken zlib");
            let raw = png_chunk(b"zTXt", &invalid);
            let src = splice_png_text_chunks(&bare(SrcFormat::Png), &[raw.clone()]).unwrap();
            let result = encode(SrcFormat::Png, Some(&src), &opts);
            let xmp = key.ends_with(b"xmp");
            let retained = xmp || opts.metadata_profile == MetadataProfile::Single;
            assert_eq!(result.thumbnail_notice.xmp_uninspectable, xmp);
            assert_eq!(result.thumbnail_notice.exif_uninspectable, !xmp && retained);
            assert_eq!(
                extract_png_text_chunks_raw(&result.bytes).unwrap(),
                if retained { vec![raw] } else { vec![] }
            );
        }
    }
}
