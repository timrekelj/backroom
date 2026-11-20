//! Raw and semantic design tokens. Platform-neutral data; no windowing or fs.

pub mod accent;
pub mod background;
pub mod color;
pub mod elevation;
pub mod icon;
pub mod motion;
pub mod palette;
pub mod radius;
pub mod space;
pub mod spec;
pub mod theme;
pub mod type_;

#[cfg(test)]
mod tests;
