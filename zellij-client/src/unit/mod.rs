// Only reachable through `#[cfg(test)] mod unit;` in lib.rs, so the modules
// here do not repeat that gate.
#[cfg(feature = "web_server_capability")]
mod terminal_loop_tests;
