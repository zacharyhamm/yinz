//! Terminal emulation, graphics, and output serialization.
pub mod host_query;
pub mod output;
mod pane_id;
pub mod panes;
pub mod ui;
pub use pane_id::PaneId;
pub type ClientId = u16;
