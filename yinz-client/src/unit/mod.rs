// Only reachable through `#[cfg(test)] mod unit;` in lib.rs, so the modules
// here do not repeat that gate.
mod cli_client_tests;
mod client_outcome_tests;
mod router_tests;
mod socket_dir_tests;
mod splash_loop_tests;
mod teardown_tests;
