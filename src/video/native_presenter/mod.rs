//! Native video rendering types.
//!
//! Window ownership lives in `native_window_host`; this module exposes only the
//! GPU/DComp render core and value-type overlay commands/state.

pub(crate) mod overlay_draw;
mod render_core;

pub use overlay_draw::draw_native_video_touch_first_run_help_snapshot_fixture;
pub use render_core::*;
