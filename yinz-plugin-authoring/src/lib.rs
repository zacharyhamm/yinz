//! Offline TypeScript plugin authoring: checking, writing and formatting plugin sources, as
//! `yinz plugins`, the plugin host's authoring commands and Agent's native tests do it.
//!
//! Nothing here parses or evaluates a plugin. Every parse, analysis, type check and strict
//! authoring run happens in a `--plugin-compilation-worker` subprocess of the server binary, so
//! this crate is the protocol, a supervised client for it and the file work around it, and
//! links neither V8 nor SWC. The worker's entry point is in `yinz-plugin-host`.
pub mod analysis;
pub mod git;
pub mod limits;
mod logs;
pub use logs::*;
mod plugin_authoring;
pub mod worker;

pub use plugin_authoring::*;
