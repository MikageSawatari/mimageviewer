//! Arbitrary-input targets shared by deterministic MSVC gate and regression tests.
use super::*;
use std::io::Cursor;

pub(crate) fn small_limits(seed: u64) -> Limits {
    Limits {
        tag_bytes: 10 + (seed as usize & 2047),
        picture_bytes: (seed.rotate_left(13) as usize & 511),
        picture_count: (seed.rotate_left(31) as usize % 17),
    }
}
pub(crate) fn header_frames(input: &[u8], seed: u64) {
    let limits = small_limits(seed);
    let mut checks = 0;
    let _ = extract_with_limits(
        &mut Cursor::new(input),
        input.len() as u64,
        &mut || {
            checks += 1;
            if checks > 4096 {
                Err(Error::Timeout)
            } else {
                Ok(())
            }
        },
        &mut |bytes| {
            assert!(bytes.len() <= limits.picture_bytes);
            Ok(Some(bytes.len()))
        },
        limits,
    );
    assert!(checks <= 4097);
}
pub(crate) fn capped_apic_inflate(input: &[u8], seed: u64) {
    if input.len() < 5 {
        return;
    }
    let limits = small_limits(seed);
    let declared = usize::from_be_bytes({
        let mut b = [0u8; std::mem::size_of::<usize>()];
        b[std::mem::size_of::<usize>() - 4..].copy_from_slice(&input[..4]);
        b
    });
    let frame = Frame {
        version: 2 + (input[4] % 3),
        compressed: true,
        declared: Some(declared),
        ..Frame::default()
    };
    let mut checks = 0;
    let _ = picture(
        &input[5..],
        frame,
        &mut || {
            checks += 1;
            if checks > 4096 {
                Err(Error::Timeout)
            } else {
                Ok(())
            }
        },
        &mut |bytes| {
            assert!(bytes.len() <= limits.picture_bytes);
            Ok(Some(bytes.len()))
        },
        limits,
    );
    assert!(checks <= 4097);
}
#[test]
fn injected_limits_enforce_tag_picture_and_candidate_boundaries() {
    use super::tests::*;
    let b = tag(&frame(&apic(b"12345", 3), 4, 0), 4, 0);
    let run = |lim| {
        extract_with_limits(
            &mut Cursor::new(&b),
            b.len() as u64,
            &mut || Ok(()),
            &mut |b| Ok(Some(b.len())),
            lim,
        )
    };
    let lim = Limits {
        tag_bytes: b.len(),
        picture_bytes: 5,
        picture_count: 1,
    };
    assert_eq!(run(lim).unwrap(), Some(5));
    assert!(matches!(
        run(Limits {
            tag_bytes: b.len() - 1,
            ..lim
        }),
        Err(Error::Limit)
    ));
    assert!(
        run(Limits {
            picture_bytes: 4,
            ..lim
        })
        .unwrap()
        .is_none()
    );
    assert!(matches!(
        run(Limits {
            picture_count: 0,
            ..lim
        }),
        Err(Error::Limit)
    ));
    assert!(matches!(
        run(Limits {
            tag_bytes: TAG_LIMIT + 1,
            ..lim
        }),
        Err(Error::Limit)
    ));
    let body = apic(b"12345", 3);
    let b = tag(&compressed(&body, 4, body.len()), 4, 0);
    assert!(
        extract_with_limits(
            &mut Cursor::new(&b),
            b.len() as u64,
            &mut || Ok(()),
            &mut |_| Ok(Some(())),
            Limits {
                tag_bytes: 1024,
                picture_bytes: 4,
                picture_count: 1
            }
        )
        .unwrap()
        .is_none()
    );
}
