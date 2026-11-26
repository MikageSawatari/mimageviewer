//! Image-combine values, pure layout, rasterization and session worker (§1.904).

pub mod label;
pub mod layout;
pub mod options;
pub mod render;
pub(crate) mod store;
pub(crate) mod worker;

pub use label::{LabelLayout, PlacedLabel, label_layout, resolve_label};
pub use layout::{CardLayout, Layout, LayoutError, Rect, layout};
pub use options::*;
pub use render::{RenderError, create_canvas, draw_card, draw_label, render_all};

#[cfg(test)]
mod tests;
