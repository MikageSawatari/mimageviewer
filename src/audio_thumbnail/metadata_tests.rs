use super::*;
fn segment(marker: u8, data: &[u8]) -> Vec<u8> {
    let mut out = vec![0xff, marker];
    out.extend(((data.len() + 2) as u16).to_be_bytes());
    out.extend(data);
    out
}
pub(crate) fn jpeg_header(w: u16, h: u16, samples: &[u8], progressive: bool) -> Vec<u8> {
    let mut sof = vec![8];
    sof.extend(h.to_be_bytes());
    sof.extend(w.to_be_bytes());
    sof.push(samples.len() as u8);
    for (i, sample) in samples.iter().enumerate() {
        sof.extend([i as u8 + 1, *sample, 0]);
    }
    let mut out = vec![0xff, 0xd8];
    out.extend(segment(if progressive { 0xc2 } else { 0xc0 }, &sof));
    out.extend(segment(0xda, &[1, 1, 0, 0, 63, 0]));
    out.extend([0xff, 0xd9]);
    out
}
pub(crate) fn offset_bomb(orientation: u16, little: bool) -> Vec<u8> {
    let count: u16 = 5001;
    let mut data = if little {
        b"II".to_vec()
    } else {
        b"MM".to_vec()
    };
    let word = |n: u16| {
        if little {
            n.to_le_bytes()
        } else {
            n.to_be_bytes()
        }
    };
    let dword = |n: u32| {
        if little {
            n.to_le_bytes()
        } else {
            n.to_be_bytes()
        }
    };
    data.extend(word(42));
    data.extend(dword(8));
    data.extend(word(count));
    for _ in 0..count - 1 {
        data.extend(word(0x8769));
        data.extend(word(4));
        data.extend(dword(1));
        data.extend(dword(8));
    }
    data.extend(word(0x112));
    data.extend(word(3));
    data.extend(dword(1));
    data.extend(word(orientation));
    data.extend([0, 0]);
    data.extend(dword(8));
    let mut exif = b"Exif\0\0".to_vec();
    exif.extend(data);
    let mut jpeg = vec![0xff, 0xd8];
    jpeg.extend(segment(0xe1, &exif));
    jpeg.extend([0xff, 0xd9]);
    jpeg
}
#[test]
fn orientation_only_ignores_5000_repeated_offsets_and_next_ifd_cycle() {
    for little in [false, true] {
        for value in 1..=8 {
            assert_eq!(
                orientation(&offset_bomb(value, little), &mut || Ok(())).unwrap(),
                value
            );
        }
    }
}
#[test]
fn orientation_cancel_and_deadline_are_checked_while_walking_entries() {
    let bytes = offset_bomb(6, true);
    let mut calls = 0;
    assert!(matches!(
        orientation(&bytes, &mut || {
            calls += 1;
            if calls == 5 {
                Err(Error::Canceled)
            } else {
                Ok(())
            }
        }),
        Err(Error::Canceled)
    ));
    assert!(matches!(
        orientation(&bytes, &mut || Err(Error::Timeout)),
        Err(Error::Timeout)
    ));
}
#[test]
fn corrupt_orientation_does_not_follow_any_offset() {
    let mut b = offset_bomb(9, true);
    assert_eq!(orientation(&b, &mut || Ok(())).unwrap(), 1);
    // TIFF IFD0 pointer is out of range; no recursive parse or allocation.
    b[16..20].fill(255);
    assert_eq!(orientation(&b, &mut || Ok(())).unwrap(), 1);
    for end in 0..24 {
        let _ = orientation(&b[..end], &mut || Ok(()));
    }
}
#[test]
fn png_exif_reads_only_bounded_ifd0_orientation() {
    let jpeg = offset_bomb(8, false);
    let exif = &jpeg[12..jpeg.len() - 2];
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend((exif.len() as u32).to_be_bytes());
    png.extend(b"eXIf");
    png.extend(exif);
    png.extend([0; 4]);
    assert_eq!(orientation(&png, &mut || Ok(())).unwrap(), 8);
}
#[test]
fn progressive_40mp_444_and_sequential_multiscan_are_rejected_before_decode() {
    for progressive in [false, true] {
        let b = jpeg_header(8000, 5000, &[0x11; 3], progressive);
        assert!(matches!(
            jpeg_budget(&b, DECODE_BUDGET, &mut || Ok(())),
            Err(Error::Limit)
        ));
    }
}
#[test]
fn jpeg_limit_boundary_sampling_and_checked_dimensions() {
    for samples in [&[0x11][..], &[0x11; 3], &[0x22, 0x11, 0x11], &[0x11; 4]] {
        let b = jpeg_header(320, 200, samples, true);
        let proof = jpeg_budget(&b, u64::MAX, &mut || Ok(())).unwrap();
        assert!(jpeg_budget(&b, proof.required, &mut || Ok(())).is_ok());
        assert!(matches!(
            jpeg_budget(&b, proof.required - 1, &mut || Ok(())),
            Err(Error::Limit)
        ));
        assert_eq!(proof.dims, (320, 200));
    }
    for (w, h, s) in [
        (0, 1, 0x11),
        (1, 0, 0x11),
        (1, 1, 0x00),
        (1, 1, 0x51),
        (65535, 65535, 0x11),
    ] {
        assert!(
            jpeg_budget(&jpeg_header(w, h, &[s], false), DECODE_BUDGET, &mut || Ok(
                ()
            ))
            .is_err()
        );
    }
}
#[test]
fn later_frame_and_dynamic_height_cannot_bypass_jpeg_budget() {
    let mut b = jpeg_header(1, 1, &[0x11; 3], false);
    b.truncate(b.len() - 2);
    let second = jpeg_header(8000, 5000, &[0x11; 3], true);
    b.extend(&second[2..]);
    assert!(matches!(
        jpeg_budget(&b, DECODE_BUDGET, &mut || Ok(())),
        Err(Error::Malformed)
    ));
    let mut b = jpeg_header(1, 1, &[0x11; 3], false);
    b.truncate(b.len() - 2);
    b.extend(segment(0xdc, &[1, 1]));
    b.extend([0xff, 0xd9]);
    assert!(matches!(
        jpeg_budget(&b, DECODE_BUDGET, &mut || Ok(())),
        Err(Error::Malformed)
    ));
}
#[test]
fn metadata_is_removed_before_native_or_fallback_decoder() {
    let mut b = jpeg_header(20, 10, &[0x11; 3], false);
    let mut app = segment(0xe1, b"Exif\0\0malicious metadata");
    app.extend(segment(0xe2, b"ICC_PROFILE\0payload"));
    b.splice(2..2, app);
    let original = jpeg_budget(&b, DECODE_BUDGET, &mut || Ok(())).unwrap();
    let clean = jpeg_without_metadata(&b, &mut || Ok(())).unwrap();
    assert_eq!(clean, jpeg_header(20, 10, &[0x11; 3], false));
    assert_eq!(
        jpeg_budget(&clean, DECODE_BUDGET, &mut || Ok(()))
            .unwrap()
            .dims,
        original.dims
    );
}
#[test]
fn jpeg_entropy_stuffing_and_cancellation_are_bounded() {
    let mut b = jpeg_header(20, 10, &[0x11; 3], true);
    b.truncate(b.len() - 2);
    b.extend([0xff, 0, 0xff, 0xd0, 2]);
    b.extend([0xff, 0xd9]);
    assert!(jpeg_budget(&b, DECODE_BUDGET, &mut || Ok(())).is_ok());
    assert!(matches!(
        jpeg_budget(&b, DECODE_BUDGET, &mut || Err(Error::Canceled)),
        Err(Error::Canceled)
    ));
}
