pub mod cli;
/// The dockerized end-to-end suite. Behind a feature because it needs `ssh2`, which builds
/// libssh2 and OpenSSL - a heavy price for five `#[ignore]`d tests that only `cargo x ci e2e`
/// ever runs.
#[cfg(feature = "e2e")]
pub mod e2e;
/// Chunked portable media over real SSH forwarding, run by `cargo x remote-test`.
#[cfg(feature = "native-terminal")]
#[path = "../unit/native_media_ssh.rs"]
mod native_media_ssh;
