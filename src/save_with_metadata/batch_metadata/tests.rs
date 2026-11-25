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
        batch_metadata: true,
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
fn exif_stripped_thumbnail_ranges_are_erased_and_malformed_offsets_rejected() {
    let mut bytes = tiff(true);
    put16(&mut bytes, 58, 0x0111, true);
    put16(&mut bytes, 70, 0x0117, true);
    assert_eq!(&clean_exif(&bytes).unwrap().0[86..], &[0; 8]);
    // A thumbnail must not overwrite a retained MakerNote or any IFD structure.
    put32(&mut bytes, 66, 48, true);
    assert!(clean_exif(&bytes).is_none());
    let mut cyclic = tiff(true);
    put32(&mut cyclic, 52, 8, true);
    assert!(clean_exif(&cyclic).is_none());
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
fn single_page_default_metadata_bytes_match_pre_unification_encoders() {
    assert!(!SaveOptions::default().batch_metadata);
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
            let legacy = match format {
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
            let wrapped =
                encode_image_with_metadata_report(&img, None, Some(&src), format.clone(), &opts)
                    .unwrap();
            assert_eq!(wrapped.bytes, legacy, "{format:?}, lossless={lossless}");
            assert!(wrapped.thumbnail_notice.is_empty());
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
        let opts = SaveOptions {
            include_metadata: false,
            ..options(true)
        };
        let result = encode(format.clone(), Some(&src), &opts);
        assert!(result.thumbnail_notice.is_empty());
        assert_eq!(result.bytes, bare(format));
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
