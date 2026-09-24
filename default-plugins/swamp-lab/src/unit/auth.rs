use super::*;

fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

const FILE: &str = r#"{
  "serverUrl": "https://swamp-club.com",
  "apiKey": "swamp_filekey0123456789",
  "apiKeyId": "key-1",
  "username": "keeper",
  "collectives": ["swamp"],
  "apiKeyFingerprint": "swamp_envkey"
}"#;

#[test]
fn the_environment_key_wins_over_the_file() {
    let creds =
        Credentials::resolve(&env(&[(API_KEY_ENV, " swamp_envkey999 ")]), Some(FILE)).unwrap();
    assert_eq!(creds.api_key.as_deref(), Some("swamp_envkey999"));
    assert_eq!(creds.server_url, DEFAULT_SERVER_URL);
    // The file cached an identity for a key with the same first 12 characters.
    assert_eq!(creds.username.as_deref(), Some("keeper"));
}

#[test]
fn a_cached_identity_for_another_key_or_server_is_not_borrowed() {
    let other_key =
        Credentials::resolve(&env(&[(API_KEY_ENV, "swamp_different")]), Some(FILE)).unwrap();
    assert_eq!(other_key.username, None);
    let other_server = Credentials::resolve(
        &env(&[
            (API_KEY_ENV, "swamp_envkey999"),
            (SERVER_URL_ENV, "http://localhost:8000"),
        ]),
        Some(FILE),
    )
    .unwrap();
    assert_eq!(other_server.server_url, "http://localhost:8000");
    assert_eq!(other_server.username, None);
}

#[test]
fn the_file_is_used_without_an_environment_key() {
    let creds = Credentials::resolve(&env(&[]), Some(FILE)).unwrap();
    assert_eq!(creds.api_key.as_deref(), Some("swamp_filekey0123456789"));
    assert_eq!(creds.username.as_deref(), Some("keeper"));
    assert!(!creds.is_anonymous());
}

#[test]
fn the_file_names_its_own_server_as_the_cli_reads_it() {
    // SWAMP_CLUB_URL only accompanies SWAMP_API_KEY in the CLI; a logged-in
    // file keeps the server it was logged in to.
    let creds = Credentials::resolve(
        &env(&[(SERVER_URL_ENV, "http://localhost:8000")]),
        Some(FILE),
    )
    .unwrap();
    assert_eq!(creds.server_url, "https://swamp-club.com");
}

#[test]
fn the_retired_domain_reads_as_its_successor() {
    let legacy = r#"{"serverUrl":"https://swamp.club","apiKey":"swamp_x","username":"u"}"#;
    let creds = Credentials::resolve(&env(&[]), Some(legacy)).unwrap();
    assert_eq!(creds.server_url, DEFAULT_SERVER_URL);
    let custom = r#"{"serverUrl":"https://lab.example.test/","apiKey":"swamp_x"}"#;
    let creds = Credentials::resolve(&env(&[]), Some(custom)).unwrap();
    assert_eq!(creds.server_url, "https://lab.example.test");
}

#[test]
fn nobody_logged_in_is_anonymous_but_may_still_pick_a_server() {
    let creds = Credentials::resolve(&env(&[]), None).unwrap();
    assert_eq!(creds, Credentials::anonymous());
    let creds =
        Credentials::resolve(&env(&[(SERVER_URL_ENV, "http://127.0.0.1:9/")]), Some("  ")).unwrap();
    assert!(creds.is_anonymous());
    assert_eq!(creds.server_url, "http://127.0.0.1:9");
    let keyless =
        Credentials::resolve(&env(&[]), Some(r#"{"serverUrl":"https://swamp-club.com"}"#)).unwrap();
    assert!(keyless.is_anonymous());
}

#[test]
fn a_broken_file_or_url_is_an_error_not_a_logout() {
    assert!(Credentials::resolve(&env(&[]), Some("{not json")).is_err());
    assert!(Credentials::resolve(&env(&[(SERVER_URL_ENV, "ftp://x")]), None).is_err());
    assert!(Credentials::resolve(&env(&[(SERVER_URL_ENV, "swamp-club.com")]), None).is_err());
}

#[test]
fn debug_output_never_shows_the_key() {
    let creds = Credentials::resolve(&env(&[]), Some(FILE)).unwrap();
    let printed = format!("{creds:?}");
    assert!(!printed.contains("filekey"), "{printed}");
    assert!(printed.contains("<redacted>"));
}

#[test]
fn issue_urls_point_at_the_web_page() {
    assert_eq!(
        Credentials::anonymous().issue_url(42),
        "https://swamp-club.com/lab/42"
    );
}

#[test]
fn reading_the_file_through_a_shell() {
    assert_eq!(auth_json_from_output(Some(0), b"", b""), Ok(None));
    assert_eq!(
        auth_json_from_output(Some(0), b" {\"apiKey\":\"k\"}\n", b""),
        Ok(Some("{\"apiKey\":\"k\"}".into()))
    );
    let denied =
        auth_json_from_output(Some(1), b"", b"cat: auth.json: Permission denied\n").unwrap_err();
    assert!(denied.contains("Permission denied"), "{denied}");
    assert!(auth_json_from_output(None, b"", b"").is_err());
}

#[test]
fn the_shell_script_follows_the_cli_config_dir_order() {
    let run = |vars: &[(&str, &str)]| {
        let dir = std::env::temp_dir().join(format!(
            "swamp-lab-auth-{}-{}",
            std::process::id(),
            vars.iter()
                .map(|(_, sub)| *sub)
                .collect::<Vec<_>>()
                .join("-")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for (sub, text) in [
            ("swamphome/config", "home"),
            ("xdg/swamp", "xdg"),
            ("home/.config/swamp", "plain"),
        ] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
            std::fs::write(dir.join(sub).join("auth.json"), text).unwrap();
        }
        let argv = auth_json_argv();
        let mut command = std::process::Command::new(&argv[0]);
        command
            .args(&argv[1..])
            .env_remove("SWAMP_HOME")
            .env_remove("XDG_CONFIG_HOME")
            .env("HOME", dir.join("home"));
        for (key, sub) in vars {
            command.env(key, dir.join(sub));
        }
        let out = command.output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        auth_json_from_output(out.status.code(), &out.stdout, &out.stderr).unwrap()
    };
    assert_eq!(run(&[]).as_deref(), Some("plain"));
    assert_eq!(run(&[("XDG_CONFIG_HOME", "xdg")]).as_deref(), Some("xdg"));
    assert_eq!(
        run(&[("XDG_CONFIG_HOME", "xdg"), ("SWAMP_HOME", "swamphome")]).as_deref(),
        Some("home")
    );
    // SWAMP_HOME wins even when it holds no file: logged out, not a fallback.
    assert_eq!(
        run(&[("SWAMP_HOME", "missing"), ("XDG_CONFIG_HOME", "xdg")]).as_deref(),
        None
    );
}
