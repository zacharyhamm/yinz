//! Shared fixtures for the client's tests.
//!
//! Every test module used to carry its own `ClientOsApi` stand-in, and they
//! were mostly the same no-op methods with one or two behaviours bolted on.
//! `FakeClientOsApi` covers all of those cases: it starts inert and each
//! builder method turns on the one capability a given test needs.

pub mod fake_os_api;

pub use fake_os_api::FakeClientOsApi;
