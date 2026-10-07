use super::*;

fn resolve(json: &str, legacy: &[(&str, &str)], file: &str) -> Result<Opener, String> {
    let mut config: BTreeMap<_, _> = legacy
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    config.insert("openers_json".into(), json.into());
    opener_for(&config, Path::new(file))
}

#[test]
fn precedence_and_explicit_editor_overrides() {
    let legacy = [("open_pdf", "legacy"), ("open_default", "fallback")];
    for (json, entries, file, expected) in [
        (
            r#"{"pdf":{"argv":["structured"]},"default":null}"#,
            &legacy[..],
            "/tmp/a.PDF",
            "structured",
        ),
        (
            r#"{"default":{"argv":["structured"]}}"#,
            &legacy[..],
            "/tmp/a.pdf",
            "legacy",
        ),
        (
            r#"{"default":{"argv":["structured"]}}"#,
            &legacy[..],
            "/tmp/no-extension",
            "structured",
        ),
        ("{}", &legacy[..], "/tmp/a.txt", "fallback"),
        ("", &legacy[..], "/tmp/a.pdf", "legacy"),
        ("  ", &legacy[..], "/tmp/a.pdf", "legacy"),
    ] {
        assert_eq!(
            resolve(json, entries, file).unwrap(),
            Opener::Command {
                path: expected.into(),
                args: vec![file.into()]
            }
        );
    }
    for (json, entries, file) in [
        (r#"{"pdf":null}"#, &legacy[..], "/tmp/a.pdf"),
        (r#"{"default":null}"#, &legacy[..], "/tmp/a.txt"),
        (
            r#"{"default":{"argv":["structured"]}}"#,
            &[("open_pdf", " ")][..],
            "/tmp/a.pdf",
        ),
        ("{}", &[][..], "/tmp/a"),
    ] {
        assert_eq!(resolve(json, entries, file).unwrap(), Opener::YinzEditor);
    }
}

#[test]
fn literal_arguments_and_single_pass_substitution() {
    let file = "/tmp/ space 'quote\",雪#?%2C{} .pdf";
    let json = r#"{"pdf":{"argv":["/opt/Image Tools/{}", "", "--title", "Selected 'image'", "before{}after{}"]}}"#;
    assert_eq!(
        resolve(json, &[], file).unwrap(),
        Opener::Command {
            path: "/opt/Image Tools/{}".into(),
            args: vec![
                "".into(),
                "--title".into(),
                "Selected 'image'".into(),
                format!("before{file}after{file}")
            ],
        }
    );
    assert_eq!(
        resolve(r#"{"pdf":{"argv":["viewer{}", ""]}}"#, &[], file).unwrap(),
        Opener::Command {
            path: "viewer{}".into(),
            args: vec!["".into(), file.into()],
        }
    );
}

#[test]
fn invalid_entries_abort_even_when_another_entry_matches() {
    for json in [
        "{",
        "null",
        "[]",
        r#"{"PDF":null}"#,
        r#"{"":null}"#,
        r#"{".pdf":null}"#,
        r#"{"pdf":false}"#,
        r#"{"pdf":[]}"#,
        r#"{"pdf":{}}"#,
        r#"{"pdf":{"argv":[]}}"#,
        r#"{"pdf":{"argv":[" "]}}"#,
        r#"{"pdf":{"argv":["viewer",null]}}"#,
        r#"{"pdf":{"argv":["viewer","\u0000"]}}"#,
        r#"{"pdf":{"argv":["viewer"],"plugin":"reader"}}"#,
        r#"{"pdf":{"plugin":"reader","config":{}}}"#,
        r#"{"pdf":{"plugin":""}}"#,
        r#"{"pdf":{"plugin":"reader","message":" "}}"#,
        r#"{"pdf":{"plugin":"reader","message":"\u0000"}}"#,
        r#"{"pdf":{"plugin":42}}"#,
        r#"{"pdf":{"plugin":"reader","message":null}}"#,
    ] {
        let error = resolve(json, &[("open_txt", "fallback")], "/tmp/a.txt").unwrap_err();
        assert!(error.starts_with("openers_json"), "{json}: {error}");
    }
}

#[test]
fn destinations_preserve_the_hosts_single_decode_contract() {
    let raw = "/tmp/a '雪,#?%2C.ts";
    let encoded = url::Url::from_file_path(raw).unwrap().to_string();
    assert_eq!(plugin_destination(raw).unwrap(), encoded);
    assert_eq!(plugin_destination(&encoded).unwrap(), encoded);
    assert_eq!(
        plugin_destination("/tmp/a%2C.ts").unwrap(),
        "file:///tmp/a%252C.ts"
    );
    assert_eq!(
        plugin_destination("file:/tmp/a%2C.ts").unwrap(),
        "file:/tmp/a%2C.ts"
    );
    for destination in ["reader", "yinz:reader", "file://localhost/tmp/a.ts"] {
        assert_eq!(plugin_destination(destination).unwrap(), destination);
    }
    for destination in [
        "./a.ts",
        "../a.ts",
        "a/main.ts",
        "file:a.ts",
        "https://example.com/a.ts",
        "file://remote/tmp/a.ts",
        "file:/tmp/a.ts?x",
        "file:/tmp/a.ts#x",
        "yinz:",
        "a\0",
    ] {
        assert!(plugin_destination(destination).is_err(), "{destination}");
    }
}

#[test]
fn duplicate_keys_and_updated_configuration_use_latest_value() {
    assert_eq!(
        resolve(r#"{"pdf":{"argv":["old"]},"pdf":null}"#, &[], "/tmp/a.pdf").unwrap(),
        Opener::YinzEditor
    );
    let mut config = BTreeMap::from([("openers_json".into(), r#"{"pdf":null}"#.into())]);
    assert_eq!(
        opener_for(&config, Path::new("/tmp/a.pdf")).unwrap(),
        Opener::YinzEditor
    );
    config.insert(
        "openers_json".into(),
        r#"{"pdf":{"plugin":"reader"}}"#.into(),
    );
    assert_eq!(
        opener_for(&config, Path::new("/tmp/a.pdf")).unwrap(),
        Opener::Plugin {
            destination: "reader".into(),
            message: "open".into()
        }
    );
}
