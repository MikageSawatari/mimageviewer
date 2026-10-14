//! Count only allocations on this test's thread, so test harness activity cannot affect it.
use music_core::pitch::{HARMNET_LONG_WINDOW_SAMPLES, HarmNetTracker};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct CountingAllocator;
thread_local! {
    static COUNT: Cell<Option<usize>> = const { Cell::new(None) };
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        COUNT.with(|count| {
            if let Some(n) = count.get() {
                count.set(Some(n + 1));
            }
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        COUNT.with(|count| {
            if let Some(n) = count.get() {
                count.set(Some(n + 1));
            }
        });
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[test]
fn frame_push_probabilities_and_reset_do_not_allocate() {
    let mut tracker = HarmNetTracker::new(48_000);
    let tone: Vec<f32> = (0..HARMNET_LONG_WINDOW_SAMPLES)
        .map(|i| (i as f32 * 0.0288).sin() * 0.1)
        .collect();
    COUNT.with(|count| count.set(Some(0)));
    for _ in 0..12 {
        tracker.push_frame(&tone);
        std::hint::black_box(tracker.probabilities(3));
    }
    tracker.reset();
    tracker.push_frame(&[]);
    let allocations = COUNT.with(|count| count.replace(None).unwrap());
    assert_eq!(allocations, 0, "hot-path heap allocation count");
}
