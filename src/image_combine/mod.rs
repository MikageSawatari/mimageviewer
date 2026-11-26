//! Pure image-combine layout and worker-side rasterization (§1.904, stage A).
// Stage B removes this allowance when the dialog and worker become callers.
#![cfg_attr(not(test), allow(dead_code))]

pub mod label;
pub mod layout;
pub mod options;
pub mod render;

pub use label::{LabelLayout, PlacedLabel, label_layout, resolve_label};
pub use layout::{CardLayout, Layout, LayoutError, Rect, layout};
pub use options::*;
pub use render::{RenderError, create_canvas, draw_card, draw_label, render_all};

#[cfg(test)]
mod tests;
