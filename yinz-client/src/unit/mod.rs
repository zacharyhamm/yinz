// Only reachable through `#[cfg(test)] mod unit;` in lib.rs, so the modules
// here do not repeat that gate.
mod cli_client_tests;
mod frame_write_tests;
mod render_budget_tests;
mod router_tests;
mod socket_dir_tests;
mod teardown_tests;
