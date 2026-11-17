use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::Duration,
};

/// A whole HTTP/1.1 reply read as bytes, because PNG bodies are not UTF-8.
pub(super) struct Reply {
    pub(super) status: u16,
    pub(super) headers: Vec<(String, String)>,
    pub(super) body: Vec<u8>,
}
impl Reply {
    pub(super) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
    pub(super) fn text(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap()
    }
    #[track_caller]
    pub(super) fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("{e}: {self}"))
    }
}
impl std::fmt::Display for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {:?} ", self.status, self.headers)?;
        f.write_str(&String::from_utf8_lossy(&self.body))
    }
}
pub(super) fn exchange(addr: SocketAddr, head: &str, body: &str) -> Reply {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(25)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(stream, "{head}Connection: close\r\n\r\n{body}").unwrap();
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).unwrap();
    let split = reply
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("complete reply head");
    let head = std::str::from_utf8(&reply[..split]).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap()["HTTP/1.1 ".len()..][..3]
        .parse()
        .unwrap();
    let headers = lines
        .map(|line| {
            let (key, value) = line.split_once(':').unwrap();
            (key.trim().to_owned(), value.trim().to_owned())
        })
        .collect();
    Reply {
        status,
        headers,
        body: reply[split + 4..].to_vec(),
    }
}
/// A browser navigation or subresource load: no bearer, no JSON content type.
pub(super) fn fetch(addr: SocketAddr, method: &str, path: &str, extra: &str) -> Reply {
    exchange(
        addr,
        &format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n{extra}"),
        "",
    )
}
/// A same-origin `fetch` from the frontend's API client.
pub(super) fn call(
    addr: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> Reply {
    let authorization = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    exchange(
        addr,
        &format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\n\
             Content-Type: application/json\r\n{authorization}Content-Length: {}\r\n",
            body.len()
        ),
        body,
    )
}
#[track_caller]
pub(super) fn assert_page(reply: &Reply) {
    assert_eq!(reply.status, 200, "{reply}");
    assert_eq!(
        reply.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    let policy = reply.header("content-security-policy").expect("page CSP");
    assert!(policy.contains("script-src 'self'"), "{policy}");
    assert!(!policy.contains("unsafe-inline"), "{policy}");
    assert!(!policy.contains("unsafe-eval"), "{policy}");
}
/// The bootstrap JSON carries only the public whitelist.
#[track_caller]
pub(super) fn assert_bootstrap(page: &str) {
    let start = page
        .find(r#"<script type="application/json" id="yinz-config">"#)
        .expect("bootstrap JSON script")
        + r#"<script type="application/json" id="yinz-config">"#.len();
    let end = start + page[start..].find("</script>").unwrap();
    let config: serde_json::Value = serde_json::from_str(&page[start..end]).unwrap();
    let mut keys: Vec<_> = config.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["poll", "quick_keys", "timeouts", "title"]);
    assert_eq!(config["title"], "yinz");
}
/// Follow the frontend's static ES module graph from its entry points, fetching each module
/// once. Every specifier must be local and relative: no bare names, import maps or CDNs.
pub(super) fn fetch_module_graph(addr: SocketAddr, entries: &[&str]) -> Vec<String> {
    let specifier = regex::Regex::new(r#"(?:\bfrom|\bimport)\s*\(?\s*["']([^"']+)["']"#).unwrap();
    let origin = url::Url::parse("http://localhost/").unwrap();
    let mut pending: Vec<url::Url> = entries.iter().map(|e| origin.join(e).unwrap()).collect();
    let mut seen = std::collections::BTreeSet::new();
    while let Some(module) = pending.pop() {
        if !seen.insert(module.path().to_owned()) {
            continue;
        }
        let reply = fetch(addr, "GET", module.path(), "");
        assert_eq!(reply.status, 200, "{module}: {reply}");
        assert_eq!(
            reply.header("content-type"),
            Some("text/javascript; charset=utf-8"),
            "{module}"
        );
        for import in specifier.captures_iter(reply.text()) {
            let import = &import[1];
            assert!(
                import.starts_with("./") || import.starts_with("../"),
                "{module} imports non-relative {import:?}"
            );
            let resolved = module.join(import).unwrap();
            assert_eq!(resolved.origin(), origin.origin(), "{import}");
            assert!(resolved.path().starts_with("/static/"), "{resolved}");
            pending.push(resolved);
        }
    }
    seen.into_iter().collect()
}

#[track_caller]
pub(super) fn assert_builtin_site(addr: SocketAddr) {
    let app = fetch(addr, "GET", "/", "");
    assert_page(&app);
    assert!(app.text().contains(r#"<div id="app"></div>"#));
    assert_bootstrap(app.text());
    let login = fetch(addr, "GET", "/login", "");
    assert_page(&login);
    assert!(login.text().contains(r#"id="login-form""#));
    assert_bootstrap(login.text());
    assert!(fetch_module_graph(addr, &["/static/app.js", "/static/login.js"]).len() > 5);
    let css = fetch(addr, "GET", "/static/app.css", "");
    assert_eq!(css.status, 200);
    assert_eq!(css.header("content-type"), Some("text/css; charset=utf-8"));
    let manifest = fetch(addr, "GET", "/static/manifest.webmanifest", "");
    assert_eq!(manifest.status, 200);
    assert_eq!(
        manifest.header("content-type"),
        Some("application/manifest+json")
    );
    let icons = manifest.json()["icons"].as_array().unwrap().clone();
    assert!(!icons.is_empty());
    for icon in icons {
        let reply = fetch(addr, "GET", icon["src"].as_str().unwrap(), "");
        assert_eq!(reply.status, 200);
        assert_eq!(reply.header("content-type"), icon["type"].as_str());
    }
}
