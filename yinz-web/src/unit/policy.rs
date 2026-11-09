use super::*;
fn settings() -> WebSettings {
    WebSettings::from_config(&Default::default(), std::path::Path::new("/tmp")).unwrap()
}
fn headers(host: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("host", host.parse().unwrap());
    h
}
#[test]
fn strict_authorities_and_origin_ports() {
    let settings = settings();
    let peer = "127.0.0.1:1234".parse().unwrap();
    for host in [
        "localhost",
        "127.0.0.1:7419",
        "[::1]:7419",
        "box.tail.ts.net",
    ] {
        assert!(
            check(&Method::GET, &headers(host), peer, &settings).is_ok(),
            "{host}"
        );
    }
    for host in [
        "localhost:",
        "localhost:no",
        "localhost:65536",
        "a@localhost",
        "localhost,evil",
        "::1",
        "[bad]",
        "localhost/evil",
    ] {
        assert!(
            check(&Method::GET, &headers(host), peer, &settings).is_err(),
            "{host}"
        );
    }
    assert_eq!(
        check(&Method::GET, &headers("evil.example"), peer, &settings),
        Err(StatusCode::MISDIRECTED_REQUEST)
    );
    let mut h = headers("localhost:7419");
    h.insert(
        "content-type",
        "application/json; charset=utf-8".parse().unwrap(),
    );
    for (origin, allowed) in [
        ("http://localhost:7419", true),
        ("http://localhost", false),
        ("http://localhost:7419/", false),
        ("null", false),
        ("http://user@localhost:7419", false),
        ("http://localhost:7419?x", false),
        ("http://localhost:7419,http://evil", false),
    ] {
        h.insert("origin", origin.parse().unwrap());
        assert_eq!(
            check(&Method::POST, &h, peer, &settings).is_ok(),
            allowed,
            "{origin}"
        );
    }
    h.insert("origin", "https://proxy.ts.net".parse().unwrap());
    h.insert("x-forwarded-host", "proxy.ts.net".parse().unwrap());
    assert!(check(&Method::POST, &h, peer, &settings).is_ok());
    assert!(check(&Method::POST, &h, "192.0.2.1:1".parse().unwrap(), &settings).is_err());
    h.append("host", "localhost".parse().unwrap());
    assert_eq!(
        check(&Method::GET, &h, peer, &settings),
        Err(StatusCode::BAD_REQUEST)
    );
}
#[test]
fn identity_funnel_and_method_checks() {
    let mut settings = settings();
    settings.tailscale_users = vec!["me@example.com".into()];
    let mut h = headers("localhost");
    let peer = "127.0.0.1:1".parse().unwrap();
    assert_eq!(
        check(&Method::GET, &h, peer, &settings),
        Err(StatusCode::FORBIDDEN)
    );
    h.insert("tailscale-user-login", "me@example.com".parse().unwrap());
    assert!(check(&Method::GET, &h, peer, &settings).is_ok());
    assert!(check(&Method::GET, &h, "192.0.2.1:1".parse().unwrap(), &settings).is_err());
    h.insert("tailscale-funnel-request", "".parse().unwrap());
    assert_eq!(
        check(&Method::GET, &h, peer, &settings),
        Err(StatusCode::FORBIDDEN)
    );
    assert_eq!(
        check(&Method::OPTIONS, &h, peer, &settings),
        Err(StatusCode::METHOD_NOT_ALLOWED)
    );
}

#[test]
fn limiter_identity_is_single_bounded_loopback_only_and_never_xff() {
    let local = "127.0.0.1:1234".parse().unwrap();
    let remote = "192.0.2.1:1234".parse().unwrap();
    let mut h = http::HeaderMap::new();
    h.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());
    assert_eq!(client_key(&h, local), "peer:127.0.0.1");
    h.insert("tailscale-user-login", "alice@example.com".parse().unwrap());
    assert_eq!(client_key(&h, local), "user:alice@example.com");
    assert_eq!(client_key(&h, remote), "peer:192.0.2.1");
    h.append("tailscale-user-login", "bob@example.com".parse().unwrap());
    assert_eq!(client_key(&h, local), "peer:127.0.0.1");
    h.remove("tailscale-user-login");
    h.insert("tailscale-user-login", "a".repeat(257).parse().unwrap());
    assert_eq!(client_key(&h, local), "peer:127.0.0.1");
}
