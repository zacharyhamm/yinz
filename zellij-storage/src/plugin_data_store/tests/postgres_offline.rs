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
    (
        "postgres://nobody:s%65cret@127.0.0.1:1/nothing?%70assword=secret&sslmode=disable&sslrootcert=/missing.pem",
        "postgres://nobody@127.0.0.1:1/nothing?sslmode=disable&sslrootcert=/missing.pem",
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

/// The peer accepts TCP and reads startup, but never sends an authentication response.
#[test]
fn authentication_has_a_deadline_and_drops_the_socket() {
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted, socket) = mpsc::sync_channel(1);
    let (finished, result) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let backend = PostgresPluginDataBackend::new(
            &format!("postgres://nobody:secret@{address}/nothing?sslmode=disable"),
            "zellij_plugin_data",
        );
        let started = Instant::now();
        let error = backend.catalog(None).unwrap_err();
        finished
            .send((error, started.elapsed(), format!("{backend:?}")))
            .unwrap();
    });
    std::thread::spawn(move || accepted.send(listener.accept().unwrap().0).unwrap());
    let mut socket = socket.recv_timeout(Duration::from_secs(10)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let (error, elapsed, debug) = result.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        error.to_string().contains("timed out after five seconds"),
        "{error}"
    );
    assert!(!error.to_string().contains("secret"));
    assert!(
        elapsed >= Duration::from_secs(5) && elapsed < Duration::from_secs(10),
        "{elapsed:?}"
    );
    assert!(debug.contains("connected: false"), "{debug}");
    // EOF proves the timed-out connection was disposed, not merely forgotten in a live task.
    socket.read_to_end(&mut Vec::new()).unwrap();
}

fn tls_peer(
    mode: &str,
    peer: impl FnOnce(&mut std::net::TcpStream),
) -> (DataError, std::time::Duration) {
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let url = format!("postgres://nobody:secret@{address}/nothing?sslmode={mode}");
    let (finished, result) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let backend = PostgresPluginDataBackend::new(&url, "zellij_plugin_data");
        let started = Instant::now();
        let error = backend.catalog(None).unwrap_err();
        finished.send((error, started.elapsed())).unwrap();
    });
    let (accepted, socket) = mpsc::sync_channel(1);
    std::thread::spawn(move || accepted.send(listener.accept().unwrap().0).unwrap());
    let mut socket = socket.recv_timeout(Duration::from_secs(10)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut request = [0; 8];
    socket.read_exact(&mut request).unwrap();
    assert_eq!(request, [0, 0, 0, 8, 4, 210, 22, 47], "SSLRequest");
    peer(&mut socket);
    let (error, elapsed) = result.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(matches!(error, DataError::Storage(_)));
    assert!(!error.to_string().contains("secret"), "{error}");
    (error, elapsed)
}

#[test]
fn require_refuses_a_server_that_declines_tls() {
    use std::io::{Read, Write};
    let (error, _) = tls_peer("require", |socket| {
        socket.write_all(b"N").unwrap();
        assert_eq!(socket.read(&mut [0]).unwrap(), 0, "no plaintext startup");
    });
    assert!(
        error.to_string().contains("server does not support TLS"),
        "{error}"
    );
}

#[test]
fn prefer_permits_plaintext_when_the_server_declines_tls() {
    use std::io::{Read, Write};
    let (error, _) = tls_peer("prefer", |socket| {
        socket.write_all(b"N").unwrap();
        let mut length = [0; 4];
        socket.read_exact(&mut length).unwrap();
        let length = u32::from_be_bytes(length) as usize;
        assert!((8..1024).contains(&length));
        let mut startup = vec![0; length - 4];
        socket.read_exact(&mut startup).unwrap();
        assert_eq!(&startup[..4], &[0, 3, 0, 0], "plaintext protocol 3 startup");
        let response = b"SERROR\0C28000\0Mplaintext startup accepted\0\0";
        socket.write_all(b"E").unwrap();
        socket
            .write_all(&(response.len() as u32 + 4).to_be_bytes())
            .unwrap();
        socket.write_all(response).unwrap();
    });
    assert!(
        error.to_string().contains("plaintext startup accepted"),
        "{error}"
    );
}

#[test]
fn tls_negotiation_has_a_deadline_and_drops_the_socket() {
    use std::io::{Read, Write};
    use std::time::Duration;
    let (error, elapsed) = tls_peer("require", |socket| {
        socket.write_all(b"S").unwrap();
        let mut hello = [0];
        socket.read_exact(&mut hello).unwrap();
        assert_eq!(hello[0], 22, "TLS handshake record");
        socket.read_to_end(&mut Vec::new()).unwrap();
    });
    assert!(
        error.to_string().contains("timed out after five seconds"),
        "{error}"
    );
    assert!(
        elapsed >= Duration::from_secs(5) && elapsed < Duration::from_secs(10),
        "{elapsed:?}"
    );
}
