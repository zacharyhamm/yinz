//! Persistence backends, independent of configuration loading.

#![cfg(not(target_family = "wasm"))]

pub mod data_access;
pub mod plugin_data_store;
