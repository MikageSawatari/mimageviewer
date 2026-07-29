//! Native video rendering types.
//!
//! Window ownership lives in `native_window_host`; this module exposes only the
//! GPU/DComp render core and value-type overlay commands/state.

pub(crate) mod overlay_draw;
mod render_core;

pub use render_core::*;
