//! What the Postgres backend does without a server: everything up to the socket.

use super::*;
use crate::data_access::PluginDataBackend;
use crate::plugin_data_store::{redact_url, PostgresPluginDataBackend};

/// A URL nobody is listening on, so that a test which does reach the socket fails fast.
const UNREACHABLE: &str = "postgres://nobody:secret@127.0.0.1:1/nothing";

const PASSWORD_URLS: &[(&str, &str)] = &[
    (UNREACHABLE, "postgres://nobody@127.0.0.1:1/nothing"),
    (
        "postgres://nobody@127.0.0.1:1/nothing?password=secret",
        "postgres://nobody@127.0.0.1:1/nothing",
    ),
    (
        "postgres://nobody:secret@127.0.0.1:1/nothing?password=secret&application_name=test",
        "postgres://nobody@127.0.0.1:1/nothing?application_name=test",
    ),
    (
        "postgres://nobody@127.0.0.1:1/nothing?password=secret&connect_timeout=1&password=secret2",
        "postgres://nobody@127.0.0.1:1/nothing?connect_timeout=1",
    ),
    (
        "postgres://nobody:s%65cret@127.0.0.1:1/nothing?%70assword=s%65cret%26value&application_name=a%26b+c%20d&pass%77ord=secret2",
        "postgres://nobody@127.0.0.1:1/nothing?application_name=a%26b+c%20d",
    ),
];

/// The sync `postgres` client drives a runtime of its own with `block_on`, which panics inside a
/// tokio worker. Every real caller is a plugin's pinned executor thread, but the failure mode
/// for a future caller that is not should be an error, not a crashed server.
#[test]
fn a_call_from_inside_a_tokio_runtime_is_an_error_rather_than_a_panic() {
    let backend = PostgresPluginDataBackend::new(UNREACHABLE, "zellij_plugin_data");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let err = runtime
        .block_on(async { backend.catalog(None) })
        .unwrap_err();
    assert_err!(&err, DataError::Storage(_));
    assert!(
        err.to_string().contains("runtime"),
        "the error should say what went wrong: {err}"
    );
}

#[test]
fn an_unreachable_database_is_reported_on_first_use_without_its_password() {
    for &(url, redacted) in PASSWORD_URLS {
        let backend = PostgresPluginDataBackend::new(url, "zellij_plugin_data");
        let err = backend.catalog(None).unwrap_err();
        assert_err!(&err, DataError::Storage(_));
        let message = err.to_string();
        assert!(message.contains("failed to connect"), "{message}");
        assert!(!message.contains("secret"), "{message}");
        assert!(!message.contains("s%65cret"), "{message}");
        assert!(message.contains(redacted), "{message}");
        // and the next call tries again rather than reporting a stale failure
        assert_err!(backend.catalog(None).unwrap_err(), DataError::Storage(_));
    }
}

#[test]
fn a_url_that_does_not_parse_is_reported_without_reaching_the_network() {
    let backend = PostgresPluginDataBackend::new(
        "postgres://x@y/z?password=secret&nonsense=1",
        "zellij_plugin_data",
    );
    let err = backend.catalog(None).unwrap_err();
    assert!(err.to_string().contains("does not parse"), "{err}");
    assert!(!err.to_string().contains("secret"), "{err}");
}

#[test]
fn a_bad_schema_name_is_refused_before_connecting() {
    let backend = PostgresPluginDataBackend::new(UNREACHABLE, "Public");
    let err = backend.catalog(None).unwrap_err();
    assert!(err.to_string().contains("schema name"), "{err}");
}

#[test]
fn the_debug_form_and_the_log_form_hide_the_password() {
    for &(url, redacted) in PASSWORD_URLS {
        let backend = PostgresPluginDataBackend::new(url, "zellij_plugin_data");
        let debug = format!("{:?}", backend);
        assert!(debug.contains("nobody@127.0.0.1:1"), "{debug}");
        assert!(!debug.contains("secret"), "{debug}");
        assert!(!debug.contains("s%65cret"), "{debug}");
        assert!(debug.contains("connected: false"), "{debug}");
        assert_eq!(redact_url(url), redacted);
        assert!(debug.contains(redacted), "{debug}");
    }
    assert_eq!(
        redact_url("host=127.0.0.1 password=secret"),
        "<postgres connection string>"
    );
}
