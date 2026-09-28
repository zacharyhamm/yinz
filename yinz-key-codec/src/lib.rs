//! Terminal key protocols: the kitty and legacy key encodings with their byte-level decode
//! helpers, and the input parser vendored from termwiz.

#[cfg(not(target_family = "wasm"))]
pub mod keys;
pub mod vendored;
