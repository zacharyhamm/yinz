//! KDL session metadata encoding and decoding.
use kdl::{KdlDocument, KdlEntry, KdlNode};
use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};
use zellij_types::data::{
    ClientId, LayoutInfo, LayoutMetadata, PaneId, PaneInfo, PaneManifest, SessionInfo, TabInfo,
};
pub mod pane_info;
pub mod pane_manifest;
pub mod session_info;
pub mod tab_info;

#[cfg(test)]
mod tests;
