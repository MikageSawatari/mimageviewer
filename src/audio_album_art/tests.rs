use super::*;
use std::io::{Cursor, Write};

pub(crate) fn ss(n: usize) -> [u8; 4] {
    [
        (n >> 21 & 127) as u8,
        (n >> 14 & 127) as u8,
        (n >> 7 & 127) as u8,
        (n & 127) as u8,
    ]
}
pub(crate) fn tag(body: &[u8], v: u8, flags: u8) -> Vec<u8> {
    let mut out = b"ID3".to_vec();
    out.extend([v, 0, flags]);
    out.extend(ss(body.len()));
    out.extend(body);
    if v == 4 && flags & 0x10 != 0 {
        let mut tail = out[..10].to_vec();
        tail[..3].copy_from_slice(b"3DI");
        out.extend(tail);
    }
    out
}
pub(crate) fn apic(data: &[u8], kind: u8) -> Vec<u8> {
    let mut b = b"\0image/png\0".to_vec();
    b.push(kind);
    b.push(0);
    b.extend(data);
    b
}
pub(crate) fn frame(body: &[u8], v: u8, flags: u16) -> Vec<u8> {
    let mut b = if v == 2 {
        b"PIC".to_vec()
    } else {
        b"APIC".to_vec()
    };
    if v == 2 {
        b.extend((body.len() as u32).to_be_bytes()[1..].iter());
    } else {
        b.extend(if v == 4 {
            ss(body.len())
        } else {
            (body.len() as u32).to_be_bytes()
        });
        b.extend(flags.to_be_bytes());
    }
    b.extend(body);
    b
}
pub(crate) fn compressed(body: &[u8], v: u8, declared: usize) -> Vec<u8> {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(body).unwrap();
    let mut b = if v == 4 {
        ss(declared).to_vec()
    } else {
        (declared as u32).to_be_bytes().to_vec()
    };
    b.extend(z.finish().unwrap());
    frame(&b, v, if v == 4 { 9 } else { 0x80 })
}
pub(crate) fn run(data: &[u8]) -> Result<Option<Vec<u8>>, Error> {
    extract(
        &mut Cursor::new(data),
        data.len() as u64,
        &mut || Ok(()),
        &mut |b| Ok(Some(b.to_vec())),
    )
}
fn escaped(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, b) in data.iter().enumerate() {
        out.push(*b);
        if *b == 0xff && data.get(i + 1).is_none_or(|n| *n == 0 || *n >= 0xe0) {
            out.push(0);
        }
    }
    out
}

#[test]
fn v23_picture() {
    assert_eq!(
        run(&tag(&frame(&apic(b"png", 3), 3, 0), 3, 0)).unwrap(),
        Some(b"png".to_vec())
    );
}
#[test]
fn v24_picture() {
    assert_eq!(
        run(&tag(&frame(&apic(b"png", 3), 4, 0), 4, 0)).unwrap(),
        Some(b"png".to_vec())
    );
}
#[test]
fn v22_picture() {
    assert_eq!(
        run(&tag(&frame(b"\0PNG\x03\0png", 2, 0), 2, 0)).unwrap(),
        Some(b"png".to_vec())
    );
}
#[test]
fn front_before_back_stable() {
    let mut body = frame(&apic(b"back", 4), 4, 0);
    body.extend(frame(&apic(b"front-a", 3), 4, 0));
    body.extend(frame(&apic(b"front-b", 3), 4, 0));
    assert_eq!(run(&tag(&body, 4, 0)).unwrap(), Some(b"front-a".to_vec()));
}
#[test]
fn invalid_front_uses_next_front() {
    let mut body = frame(&apic(b"bad", 3), 3, 0);
    body.extend(frame(&apic(b"good", 3), 3, 0));
    let file = tag(&body, 3, 0);
    let result = extract(
        &mut Cursor::new(&file),
        file.len() as u64,
        &mut || Ok(()),
        &mut |b| Ok((b != b"bad").then(|| b.to_vec())),
    )
    .unwrap();
    assert_eq!(result, Some(b"good".to_vec()));
}
#[test]
fn no_tag() {
    assert!(run(b"plain audio").unwrap().is_none());
}
#[test]
fn appended_only_ignored() {
    let mut file = b"audio bytes".to_vec();
    file.extend(tag(&frame(&apic(b"new", 3), 4, 0), 4, 0x10));
    file.extend([0; 128]);
    assert!(run(&file).unwrap().is_none());
}
#[test]
fn subsequent_tags_ignored_even_if_update_or_deleted() {
    for flags in [0, 0x40] {
        let mut file = tag(&frame(&apic(b"old", 3), 4, 0), 4, 0);
        let mut extra = if flags != 0 {
            vec![0, 0, 0, 7, 1, 0x40, 0]
        } else {
            Vec::new()
        };
        extra.extend(frame(&apic(b"new", 3), 4, 0));
        file.extend(tag(&extra, 4, flags));
        assert_eq!(run(&file).unwrap(), Some(b"old".to_vec()));
    }
    let mut file = tag(&frame(&apic(b"old", 3), 4, 0), 4, 0);
    file.extend(tag(&[], 4, 0));
    assert_eq!(run(&file).unwrap(), Some(b"old".to_vec()));
}
#[test]
fn own_footer_validated() {
    assert_eq!(
        run(&tag(&frame(&apic(b"png", 3), 4, 0), 4, 0x10)).unwrap(),
        Some(b"png".to_vec())
    );
    let mut file = tag(&frame(&apic(b"png", 3), 4, 0), 4, 0x10);
    let n = file.len();
    file[n - 10] = b'X';
    assert!(matches!(run(&file), Err(Error::Malformed)));
}
#[test]
fn extended_headers() {
    for v in [3, 4] {
        let mut body = if v == 3 {
            vec![0, 0, 0, 6, 0, 0, 0, 0, 0, 0]
        } else {
            vec![0, 0, 0, 6, 1, 0]
        };
        body.extend(frame(&apic(b"png", 3), v, 0));
        assert_eq!(run(&tag(&body, v, 0x40)).unwrap(), Some(b"png".to_vec()));
    }
}
#[test]
fn compressed_v23_and_v24() {
    for v in [3, 4] {
        let a = apic(b"png", 3);
        assert_eq!(
            run(&tag(&compressed(&a, v, a.len()), v, 0)).unwrap(),
            Some(b"png".to_vec())
        );
    }
}
#[test]
fn lying_dli_smaller_and_larger() {
    for diff in [-1isize, 1] {
        let a = apic(b"png", 3);
        assert!(
            run(&tag(
                &compressed(&a, 4, (a.len() as isize + diff) as usize),
                4,
                0
            ))
            .unwrap()
            .is_none()
        );
    }
}
#[test]
fn compressed_missing_dli() {
    let a = frame(b"junk", 4, 8);
    assert!(run(&tag(&a, 4, 0)).unwrap().is_none());
}
#[test]
fn compressed_trailing_junk() {
    let a = apic(b"png", 3);
    let mut f = compressed(&a, 4, a.len());
    f.push(0);
    let n = ss(f.len() - 10);
    f[4..8].copy_from_slice(&n);
    assert!(run(&tag(&f, 4, 0)).unwrap().is_none());
}
#[test]
fn unsync_v23_tag() {
    let a = apic(&[0xff, 0xe0, 0xff, 0, 0xff], 3);
    let body = escaped(&frame(&a, 3, 0));
    assert_eq!(
        run(&tag(&body, 3, 0x80)).unwrap(),
        Some(vec![0xff, 0xe0, 0xff, 0, 0xff])
    );
}
#[test]
fn unsync_v24_once() {
    let a = apic(&[0xff, 0, 0xff, 0xe0], 3);
    let body = frame(&escaped(&a), 4, 2);
    assert_eq!(
        run(&tag(&body, 4, 0x80)).unwrap(),
        Some(vec![0xff, 0, 0xff, 0xe0])
    );
}
#[test]
fn compressed_unsync_v24() {
    let a = apic(&[0xff, 0xff, 0], 3);
    let f = compressed(&a, 4, a.len());
    let body = frame(&escaped(&f[10..]), 4, 11);
    assert_eq!(
        run(&tag(&body, 4, 0x80)).unwrap(),
        Some(vec![0xff, 0xff, 0])
    );
}
#[test]
fn group_fields() {
    for v in [3, 4] {
        let mut a = vec![42];
        a.extend(apic(b"png", 3));
        assert_eq!(
            run(&tag(&frame(&a, v, if v == 3 { 0x20 } else { 0x40 }), v, 0)).unwrap(),
            Some(b"png".to_vec())
        );
    }
}
#[test]
fn utf16_description() {
    let mut a = b"\x01image/png\0\x03\xff\xfeA\0\0\0".to_vec();
    a.extend(b"png");
    assert_eq!(
        run(&tag(&frame(&a, 3, 0), 3, 0)).unwrap(),
        Some(b"png".to_vec())
    );
}
#[test]
fn url_picture_not_opened() {
    assert!(
        run(&tag(&frame(b"\0-->\0\x03\0http://invalid", 4, 0), 4, 0))
            .unwrap()
            .is_none()
    );
}
#[test]
fn sixteen_and_seventeen_pictures() {
    for count in [16, 17] {
        let body = frame(&apic(b"png", 3), 4, 0).repeat(count);
        let result = run(&tag(&body, 4, 0));
        if count == 16 {
            assert!(result.unwrap().is_some());
        } else {
            assert!(matches!(result, Err(Error::Limit)));
        }
    }
}
#[test]
fn encrypted_and_unknown_skip() {
    for flags in [4, 0x8000] {
        assert!(
            run(&tag(&frame(&apic(b"png", 3), 4, flags), 4, 0))
                .unwrap()
                .is_none()
        );
    }
}
#[test]
fn cancellation_during_read() {
    let file = tag(&frame(&apic(&[1; 70000], 3), 4, 0), 4, 0);
    let mut checks = 0;
    let r = extract(
        &mut Cursor::new(&file),
        file.len() as u64,
        &mut || {
            checks += 1;
            if checks == 5 {
                Err(Error::Canceled)
            } else {
                Ok(())
            }
        },
        &mut |_| Ok(Some(())),
    );
    assert!(matches!(r, Err(Error::Canceled)));
}
#[test]
fn timeout_during_inflate() {
    let a = apic(&[1; 200000], 3);
    let file = tag(&compressed(&a, 4, a.len()), 4, 0);
    let mut checks = 0;
    let r = extract(
        &mut Cursor::new(&file),
        file.len() as u64,
        &mut || {
            checks += 1;
            if checks > 8 {
                Err(Error::Timeout)
            } else {
                Ok(())
            }
        },
        &mut |_| Ok(Some(())),
    );
    assert!(matches!(r, Err(Error::Timeout)));
}
#[test]
fn malformed_boundary_and_padding() {
    for mut file in [
        tag(b"APIC", 4, 0),
        tag(&frame(&apic(b"png", 3), 4, 0), 4, 0),
    ] {
        file.pop();
        assert!(matches!(run(&file), Err(Error::Malformed)));
    }
    let mut b = frame(&apic(b"png", 3), 4, 0);
    b.extend([0, 0, 1]);
    assert!(matches!(run(&tag(&b, 4, 0)), Err(Error::Malformed)));
}
#[test]
fn unknown_frame_does_not_inflate() {
    let mut f = frame(b"nonsense", 4, 9);
    f[..4].copy_from_slice(b"TIT2");
    assert!(run(&tag(&f, 4, 0)).unwrap().is_none());
}
#[test]
fn header_mutation_fuzz() {
    let file = tag(&frame(&apic(b"png", 3), 4, 0), 4, 0);
    for i in 0..16000 {
        let mut changed = file.clone();
        let index = i % changed.len();
        changed[index] = (i / changed.len()) as u8;
        let mut calls = 0;
        let _ = extract(
            &mut Cursor::new(&changed),
            changed.len() as u64,
            &mut || {
                calls += 1;
                if calls > 10000 {
                    Err(Error::Timeout)
                } else {
                    Ok(())
                }
            },
            &mut |_| Ok(Some(())),
        );
    }
}
#[test]
fn inflate_mutation_fuzz() {
    let a = apic(b"png", 3);
    let file = tag(&compressed(&a, 4, a.len()), 4, 0);
    for i in 0..16000 {
        let mut changed = file.clone();
        let index = 10 + i % (changed.len() - 10);
        changed[index] = (i / (changed.len() - 10)) as u8;
        let _ = extract(
            &mut Cursor::new(&changed),
            changed.len() as u64,
            &mut || Ok(()),
            &mut |_| Ok(Some(())),
        );
    }
}

#[test]
fn tag_physical_limit_exact_and_plus_one() {
    let exact = tag(&vec![0; TAG_LIMIT - 10], 4, 0);
    assert!(run(&exact).unwrap().is_none());
    let mut header = b"ID3\x04\0\0".to_vec();
    header.extend(ss(TAG_LIMIT - 9));
    assert!(matches!(
        extract(
            &mut Cursor::new(&header),
            (TAG_LIMIT + 1) as u64,
            &mut || Ok(()),
            &mut |_| Ok(Some(()))
        ),
        Err(Error::Limit)
    ));
}
#[test]
fn uncompressed_picture_limit() {
    for n in [PICTURE_LIMIT, PICTURE_LIMIT + 1] {
        let file = tag(&frame(&apic(&vec![1; n], 3), 4, 0), 4, 0);
        let out = extract(
            &mut Cursor::new(&file),
            file.len() as u64,
            &mut || Ok(()),
            &mut |b| Ok(Some(b.len())),
        )
        .unwrap();
        assert_eq!(out, (n == PICTURE_LIMIT).then_some(n));
    }
}
#[test]
fn long_description_small_picture() {
    let mut body = b"\0image/png\0\x03".to_vec();
    body.extend(vec![b'd'; 300000]);
    body.push(0);
    body.extend(b"png");
    for v in [3, 4] {
        let file = tag(&compressed(&body, v, body.len()), v, 0);
        assert_eq!(run(&file).unwrap(), Some(b"png".to_vec()));
    }
}
#[test]
fn compressed_corruption_truncation_and_members() {
    let body = apic(b"png", 3);
    let f = compressed(&body, 4, body.len());
    let mut cases = Vec::new();
    let mut crc = f[10..].to_vec();
    *crc.last_mut().unwrap() ^= 1;
    cases.push(crc);
    cases.push(f[10..f.len() - 1].to_vec());
    let mut members = f[10..].to_vec();
    members.extend(&f[14..]);
    cases.push(members);
    let mut dictionary = f[10..].to_vec();
    dictionary[5] = 0x20;
    cases.push(dictionary);
    for bytes in cases {
        assert!(run(&tag(&frame(&bytes, 4, 9), 4, 0)).unwrap().is_none());
    }
}
#[test]
fn utf8_and_utf16be_descriptions() {
    for body in [
        b"\x03image/png\0\x03utf8\0png".to_vec(),
        b"\x02image/png\0\x03\0A\0\0png".to_vec(),
    ] {
        assert_eq!(
            run(&tag(&frame(&body, 4, 0), 4, 0)).unwrap(),
            Some(b"png".to_vec())
        );
    }
}
#[test]
fn unsync_cancellation_cannot_skip_chunk_boundary() {
    let mut bytes = vec![0xff, 0];
    bytes.extend(vec![1; CHUNK * 3]);
    let mut checks = 0;
    let result = unsync(&mut bytes, &mut || {
        checks += 1;
        if checks == 2 {
            Err(Error::Canceled)
        } else {
            Ok(())
        }
    });
    assert!(matches!(result, Err(Error::Canceled)));
    assert_eq!(checks, 2);
}
#[test]
fn prefix_cancellation_and_one_deadline_across_candidates() {
    let body = apic(&vec![1; CHUNK * 4], 3);
    let file = tag(&compressed(&body, 4, body.len()), 4, 0);
    let mut calls = 0;
    let out = extract(
        &mut Cursor::new(&file),
        file.len() as u64,
        &mut || {
            calls += 1;
            if calls == 8 {
                Err(Error::Canceled)
            } else {
                Ok(())
            }
        },
        &mut |_| Ok(Some(())),
    );
    assert!(matches!(out, Err(Error::Canceled)));
    let file = tag(&frame(&apic(b"png", 3), 4, 0).repeat(16), 4, 0);
    let expired = std::cell::Cell::new(false);
    let out = extract(
        &mut Cursor::new(&file),
        file.len() as u64,
        &mut || {
            if expired.get() {
                Err(Error::Timeout)
            } else {
                Ok(())
            }
        },
        &mut |_| {
            expired.set(true);
            Ok(None::<()>)
        },
    );
    assert!(matches!(out, Err(Error::Timeout)));
}
