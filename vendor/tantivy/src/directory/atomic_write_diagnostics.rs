//! Diagnostic origin only; this state never controls error handling or retries.
use std::cell::Cell;

thread_local! {
    static ORIGIN: Cell<&'static str> = const { Cell::new("foreground") };
}

pub(crate) fn origin() -> &'static str {
    ORIGIN.with(Cell::get)
}

pub(crate) struct OriginGuard(&'static str);

pub(crate) fn merge_origin() -> OriginGuard {
    OriginGuard(ORIGIN.with(|origin| origin.replace("merge")))
}

impl Drop for OriginGuard {
    fn drop(&mut self) {
        ORIGIN.with(|origin| origin.set(self.0));
    }
}
