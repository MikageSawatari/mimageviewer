//! Duplicate-image signature primitives.
//!
//! This module is deliberately independent of application state, UI, I/O, and
//! persistence. Callers supply decoded, EXIF-oriented pixels to [`proxy`].

pub mod proxy;

pub use proxy::{PROXY_VERSION, Proxy};
