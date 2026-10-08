//! Dependency-only gate using the exact product reader. Never launches mIV.
#[path = "../../../src/audio_album_art.rs"]
mod audio_album_art;
fn main() {
    println!(
        "Run cargo test --manifest-path scripts/audio-art-rust-gate/Cargo.toml --offline -- --nocapture --test-threads=1"
    );
}

#[cfg(test)]
mod allocation {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    static TRACK: AtomicBool = AtomicBool::new(false);
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);
    static MAX: AtomicUsize = AtomicUsize::new(0);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    struct Counting;
    #[global_allocator]
    static ALLOC: Counting = Counting;
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let align = layout.align().max(std::mem::align_of::<usize>());
            let head = align.max(2 * std::mem::size_of::<usize>());
            let extended = Layout::from_size_align(layout.size() + head, align).unwrap();
            let ptr = unsafe { System.alloc(extended) };
            if ptr.is_null() {
                return ptr;
            }
            let tracked = TRACK.load(Ordering::Relaxed);
            unsafe {
                (ptr as *mut usize).write(layout.size());
                (ptr as *mut usize).add(1).write(usize::from(tracked));
            }
            if tracked {
                CALLS.fetch_add(1, Ordering::Relaxed);
                MAX.fetch_max(layout.size(), Ordering::Relaxed);
                let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
                PEAK.fetch_max(live, Ordering::Relaxed);
            }
            unsafe { ptr.add(head) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            let align = layout.align().max(std::mem::align_of::<usize>());
            let head = align.max(2 * std::mem::size_of::<usize>());
            let base = unsafe { ptr.sub(head) };
            let size = unsafe { (base as *const usize).read() };
            if unsafe { (base as *const usize).add(1).read() } != 0 {
                LIVE.fetch_sub(size, Ordering::Relaxed);
            }
            unsafe {
                System.dealloc(
                    base,
                    Layout::from_size_align(layout.size() + head, align).unwrap(),
                );
            }
        }
    }
    pub fn measured<T>(f: impl FnOnce() -> T) -> (T, usize, usize, usize) {
        assert_eq!(LIVE.load(Ordering::Relaxed), 0);
        PEAK.store(0, Ordering::Relaxed);
        MAX.store(0, Ordering::Relaxed);
        CALLS.store(0, Ordering::Relaxed);
        TRACK.store(true, Ordering::Relaxed);
        let result = f();
        TRACK.store(false, Ordering::Relaxed);
        let max = MAX.load(Ordering::Relaxed);
        let peak = PEAK.load(Ordering::Relaxed);
        let calls = CALLS.load(Ordering::Relaxed);
        println!("GATE allocation max_request={max} peak_live={peak} calls={calls}");
        (result, max, peak, calls)
    }
}

#[cfg(test)]
mod gate {
    use super::{
        allocation::measured,
        audio_album_art::{self as art, tests::*},
    };
    use std::io::Cursor;
    fn no_copy(file: &[u8]) -> Result<Option<usize>, art::Error> {
        art::extract(
            &mut Cursor::new(file),
            file.len() as u64,
            &mut || Ok(()),
            &mut |b| Ok(Some(b.len())),
        )
    }
    #[test]
    fn oversized_header_before_allocation() {
        let mut header = b"ID3\x04\0\0".to_vec();
        header.extend(ss(art::TAG_LIMIT + 1));
        let (r, max, _, calls) = measured(|| {
            art::extract(
                &mut Cursor::new(&header),
                u64::MAX,
                &mut || Ok(()),
                &mut |_| Ok(Some(())),
            )
        });
        assert!(matches!(r, Err(art::Error::Limit)));
        assert_eq!((max, calls), (0, 0));
    }
    #[test]
    fn old_ffmpeg_bomb_no_large_allocation() {
        let mut a = apic(b"png", 3);
        a.resize(24 * 1024 * 1024, 0);
        let file = tag(&compressed(&a, 4, a.len()), 4, 0);
        let (r, max, peak, _) = measured(|| no_copy(&file));
        assert!(r.unwrap().is_none());
        assert!(max < 1024 * 1024);
        assert!(peak < 2 * 1024 * 1024);
    }
    #[test]
    fn over_declared_before_inflater() {
        let file = tag(&compressed(&apic(b"png", 3), 4, art::TAG_LIMIT + 1), 4, 0);
        let (r, max, _, calls) = measured(|| no_copy(&file));
        assert!(r.unwrap().is_none());
        assert_eq!(calls, 1);
        assert_eq!(max, file.len() - 10);
    }
    #[test]
    fn candidate_count_before_inflater() {
        let a = apic(b"png", 3);
        let file = tag(&compressed(&a, 4, a.len()).repeat(17), 4, 0);
        let (r, max, _, calls) = measured(|| no_copy(&file));
        assert!(matches!(r, Err(art::Error::Limit)));
        assert_eq!(calls, 1);
        assert_eq!(max, file.len() - 10);
    }
    #[test]
    fn exact_picture_limit_and_plus_one() {
        for size in [art::PICTURE_LIMIT, art::PICTURE_LIMIT + 1] {
            let a = apic(&vec![0; size], 3);
            let file = tag(&compressed(&a, 4, a.len()), 4, 0);
            let (r, max, peak, _) = measured(|| no_copy(&file));
            if size == art::PICTURE_LIMIT {
                assert_eq!(r.unwrap(), Some(size));
                assert_eq!(max, size);
                assert!(peak < size + 1024 * 1024);
            } else {
                assert!(r.unwrap().is_none());
                assert!(max < 1024 * 1024);
            }
        }
    }
    #[test]
    fn canceled_before_allocation() {
        let file = tag(&frame(&apic(b"png", 3), 4, 0), 4, 0);
        let (r, max, _, calls) = measured(|| {
            art::extract(
                &mut Cursor::new(&file),
                file.len() as u64,
                &mut || Err(art::Error::Canceled),
                &mut |_| Ok(Some(())),
            )
        });
        assert!(matches!(r, Err(art::Error::Canceled)));
        assert_eq!((max, calls), (0, 0));
    }
    #[test]
    fn real_deadline_is_request_wide() {
        let file = tag(&frame(&apic(b"png", 3), 4, 0), 4, 0);
        let start = std::time::Instant::now();
        let limit = std::time::Duration::from_secs(10);
        let checks = std::sync::atomic::AtomicUsize::new(0);
        let r = art::extract(
            &mut Cursor::new(&file),
            file.len() as u64,
            &mut || {
                checks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if start.elapsed() >= limit {
                    Err(art::Error::Timeout)
                } else {
                    Ok(())
                }
            },
            &mut |_| {
                std::thread::sleep(limit);
                Ok(Some(()))
            },
        );
        assert!(matches!(r, Err(art::Error::Timeout)));
        assert!(checks.load(std::sync::atomic::Ordering::Relaxed) > 3);
    }
}
