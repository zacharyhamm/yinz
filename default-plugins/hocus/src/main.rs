//! The wasm entry point. Everything else lives in the library half, which the
//! render benchmark also imports — see `src/lib.rs`.

use hocus::State;
use yinz_tile::prelude::*;

register_plugin!(State);
