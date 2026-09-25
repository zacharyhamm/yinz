use super::*;

#[test]
fn extracts_only_the_ca_path_and_preserves_driver_url_semantics() {
    let base = "postgresql://user:p%26ss@localhost:5432,localhost:5433/db";
    let params = "application_name=a%26b+c%20d&password=p%26ss&options=-c%20search_path%3Dpublic";
    let url = format!("{base}?{params}&ssl%72ootcert=%2Ftmp%2Fca+one%20%26%25%3F.pem");
    let (retained, path) = extract_rootcert(&url).unwrap();
    assert_eq!(retained, format!("{base}?{params}"));
    assert_eq!(path.as_deref(), Some("/tmp/ca+one &%?.pem"));
    let config: Config = retained.parse().unwrap();
    assert_eq!(config.get_application_name(), Some("a&b+c d"));
    assert_eq!(config.get_password(), Some(b"p&ss".as_slice()));
    assert_eq!(config.get_hosts().len(), 2);
    assert_eq!(config.get_ssl_mode(), SslMode::Prefer);
    for url in [
        base,
        "host=localhost password='secret?sslrootcert=foo' sslmode=disable",
    ] {
        assert_eq!(extract_rootcert(url).unwrap(), (url.to_owned(), None));
    }
    assert_eq!(
        extract_rootcert("postgres://host/db?sslrootcert=ca.pem").unwrap(),
        ("postgres://host/db".to_owned(), Some("ca.pem".to_owned()))
    );
}

#[test]
fn invalid_ca_parameters_are_errors_without_passwords() {
    for params in [
        "sslrootcert=",
        "sslrootcert",
        "sslrootcert=a&ssl%72ootcert=b",
        "sslrootcert=%FF",
    ] {
        let error = configuration(&format!("postgres://u:secret@host/db?{params}"))
            .err()
            .unwrap();
        assert!(matches!(error, DataError::Storage(_)));
        assert!(error.to_string().contains("sslrootcert"), "{error}");
        assert!(!error.to_string().contains("secret"), "{error}");
    }
}

#[test]
fn ca_file_errors_are_contextual_and_disable_skips_loading() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ca+with spaces &%.pem");
    let encoded = percent_encoding::utf8_percent_encode(
        path.to_str().unwrap(),
        percent_encoding::NON_ALPHANUMERIC,
    );
    let url = format!("postgres://u:secret@host/db?sslrootcert={encoded}");
    // Missing, empty, whitespace-only, junk, truncated, and malformed certificates.
    for contents in [
        None,
        Some(""),
        Some(" \n\t"),
        Some("not a certificate"),
        Some("-----BEGIN CERTIFICATE-----\ntruncated"),
        Some("-----BEGIN CERTIFICATE-----\ninvalid\n-----END CERTIFICATE-----"),
    ] {
        if let Some(contents) = contents {
            std::fs::write(&path, contents).unwrap();
        }
        let error = configuration(&url).err().unwrap();
        assert!(matches!(error, DataError::Storage(_)));
        let message = error.to_string();
        assert!(
            message.contains("failed to load Postgres sslrootcert"),
            "{message}"
        );
        assert!(message.contains(path.to_str().unwrap()), "{message}");
        assert!(!message.contains("secret"), "{message}");
        assert!(configuration(&format!("{url}&sslmode=disable")).is_ok());
    }
    assert!(configuration("host=localhost password=secret sslmode=disable").is_ok());
    for mode in ["verify-ca", "verify-full", "allow", "nonsense"] {
        assert!(configuration(&format!("{url}&sslmode={mode}")).is_err());
    }
}
