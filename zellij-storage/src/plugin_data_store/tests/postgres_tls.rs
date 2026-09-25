use super::*;
use crate::data_access::PluginDataBackend;

fn ca_bundle() -> String {
    std::fs::read_to_string(
        std::env::var_os("ZELLIJ_PLUGIN_DATA_TEST_CA")
            .expect("run cargo x postgres-test --test to export the fixture's CA"),
    )
    .unwrap()
}

fn tls_url(guard: &Guard, mode: Option<&str>, ca: Option<&std::path::Path>) -> String {
    let mut url = url::Url::parse(&guard.backend_url()).unwrap();
    url.set_host(Some("localhost")).unwrap();
    let retained: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| key != "sslmode")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.query_pairs_mut().clear().extend_pairs(retained);
    if let Some(mode) = mode {
        url.query_pairs_mut().append_pair("sslmode", mode);
    }
    if let Some(ca) = ca {
        // Postgres uses percent decoding, not form decoding ('+' means a literal '+').
        let path = percent_encoding::utf8_percent_encode(
            ca.to_str().unwrap(),
            percent_encoding::NON_ALPHANUMERIC,
        );
        return format!("{url}&sslrootcert={path}");
    }
    url.to_string()
}

fn tls_pid(admin: &mut ::postgres::Client, guard: &Guard) -> i32 {
    let row = admin
        .query_one(
            "SELECT a.pid, s.ssl FROM pg_stat_activity a JOIN pg_stat_ssl s USING (pid) \
             WHERE a.application_name = $1",
            &[&guard.schema],
        )
        .unwrap();
    assert!(
        row.get::<_, bool>(1),
        "the store connection must be encrypted"
    );
    row.get(0)
}

#[test]
fn direct_tls_connects_with_postgresql_alpn() {
    let guard = Guard::fresh();
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, ca_bundle()).unwrap();
    let url = format!(
        "{}&sslnegotiation=direct",
        tls_url(&guard, Some("require"), Some(&ca))
    );
    let backend = PostgresPluginDataBackend::new(&url, &guard.schema);
    backend.catalog(None).unwrap();
    let mut admin = ::postgres::Client::connect(&test_url(), NoTls).unwrap();
    tls_pid(&mut admin, &guard);
}

#[test]
fn tls_reads_writes_and_reconnects_with_a_reloaded_ca_bundle() {
    let guard = Guard::fresh();
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("ca + &%?.pem");
    let bundle = ca_bundle();
    assert_eq!(bundle.matches("-----BEGIN CERTIFICATE-----").count(), 2);
    std::fs::write(&ca, &bundle).unwrap();
    let backend =
        PostgresPluginDataBackend::new(&tls_url(&guard, Some("require"), Some(&ca)), &guard.schema);
    backend
        .define_entity(NS, OWNER, &agent_def(), false)
        .unwrap();
    backend
        .apply(
            NS,
            &[Mutation::put("agent").set("summary", "encrypted").build()],
        )
        .unwrap();
    assert_eq!(
        texts(
            &backend.fetch(NS, &FetchRequest::entity("agent")).unwrap(),
            "summary"
        ),
        ["encrypted"]
    );
    let mut admin = ::postgres::Client::connect(&test_url(), NoTls).unwrap();
    let first_pid = tls_pid(&mut admin, &guard);
    admin
        .query_one("SELECT pg_terminate_backend($1)", &[&first_pid])
        .unwrap();
    // A dead socket can fail the first call; the next call must open a fresh TLS connection.
    let _ = backend.catalog(None);
    backend.catalog(None).unwrap();
    let second_pid = tls_pid(&mut admin, &guard);
    assert_ne!(first_pid, second_pid);

    std::fs::write(&ca, "broken replacement CA").unwrap();
    admin
        .query_one("SELECT pg_terminate_backend($1)", &[&second_pid])
        .unwrap();
    let _ = backend.catalog(None);
    let error = backend.catalog(None).unwrap_err();
    assert!(error.to_string().contains("sslrootcert"), "{error}");
    std::fs::write(&ca, bundle).unwrap();
    backend.catalog(None).unwrap();
    assert_ne!(second_pid, tls_pid(&mut admin, &guard));
    backend
        .apply(
            NS,
            &[Mutation::put("agent")
                .set("summary", "after reconnect")
                .build()],
        )
        .unwrap();
    assert_eq!(
        backend.count(NS, &FetchRequest::entity("agent")).unwrap(),
        2
    );
}

#[test]
fn certificate_and_hostname_failures_never_fall_back_to_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, ca_bundle()).unwrap();
    let mut admin = ::postgres::Client::connect(&test_url(), NoTls).unwrap();
    for mode in [None, Some("prefer"), Some("require")] {
        let guard = Guard::fresh();
        let valid = tls_url(&guard, mode, Some(&ca));
        // Prove the fixture and trust work for each mode, including the implicit default.
        PostgresPluginDataBackend::new(&valid, &guard.schema)
            .catalog(None)
            .unwrap();
        for url in [
            tls_url(&guard, mode, None), // server CA is not in system trust
            valid.replace("@localhost:", "@127.0.0.1:"), // CA trusted, wrong hostname
        ] {
            // A query password exercises redaction without breaking authentication if a
            // regression lets the driver fall back to plaintext.
            let url = format!("{url}&password=zellij");
            let backend = PostgresPluginDataBackend::new(&url, &guard.schema);
            let error = backend.catalog(None).unwrap_err();
            assert!(matches!(error, DataError::Storage(_)));
            let message = error.to_string();
            assert!(
                message.contains("error performing TLS handshake"),
                "{message}"
            );
            assert!(!message.contains(":zellij@"), "{message}");
            assert!(!message.contains("password="), "{message}");
            assert!(!format!("{backend:?}").contains("password="));
        }
    }
    let guard = Guard::fresh();
    let backend = PostgresPluginDataBackend::new(
        &tls_url(
            &guard,
            Some("disable"),
            Some(&dir.path().join("missing.pem")),
        ),
        &guard.schema,
    );
    backend.catalog(None).unwrap();
    let row = admin.query_one(
        "SELECT s.ssl FROM pg_stat_activity a JOIN pg_stat_ssl s USING (pid) WHERE a.application_name = $1",
        &[&guard.schema],
    ).unwrap();
    assert!(!row.get::<_, bool>(0), "disable must use plaintext");
}

#[test]
fn a_valid_certificate_does_not_hide_malformed_bundle_contents() {
    let guard = Guard::fresh();
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("ca.pem");
    let bundle = ca_bundle();
    for pem in [
        format!("{bundle}garbage"),
        format!("garbage{bundle}"),
        format!("{bundle}-----BEGIN CERTIFICATE-----\nbroken\n-----END CERTIFICATE-----"),
    ] {
        std::fs::write(&ca, pem).unwrap();
        let backend = PostgresPluginDataBackend::new(
            &tls_url(&guard, Some("require"), Some(&ca)),
            &guard.schema,
        );
        let error = backend.catalog(None).unwrap_err();
        assert!(error.to_string().contains("sslrootcert"), "{error}");
    }
}
