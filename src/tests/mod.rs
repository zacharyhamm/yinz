pub mod cli;
/// The dockerized end-to-end suite. Behind a feature because it needs `ssh2`, which builds
/// libssh2 and OpenSSL - a heavy price for five `#[ignore]`d tests that only `cargo x ci e2e`
/// ever runs.
#[cfg(feature = "e2e")]
pub mod e2e;
