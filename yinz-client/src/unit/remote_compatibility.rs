use super::*;

#[test]
fn wire_contracts_not_release_labels_determine_compatibility() {
    for release in ["0.1.1", "0.1.2", "2.0.0", "development-build"] {
        check_versions(
            release,
            CLIENT_SERVER_CONTRACT_VERSION,
            REMOTE_BOOTSTRAP_VERSION,
        )
        .unwrap();
        assert!(check_versions(
            release,
            CLIENT_SERVER_CONTRACT_VERSION + 1,
            REMOTE_BOOTSTRAP_VERSION
        )
        .is_err());
        assert!(check_versions(
            release,
            CLIENT_SERVER_CONTRACT_VERSION,
            REMOTE_BOOTSTRAP_VERSION + 1
        )
        .is_err());
    }
}

#[test]
fn legacy_identity_is_bounded_and_cannot_contain_shell_arguments() {
    assert_eq!(legacy_build_identity(b"yinz 0.1.1\n").unwrap(), "0.1.1");
    for invalid in [
        b"banner\nyinz 0.1.1".as_slice(),
        b"yinz ",
        b"yinz 0.1.1 --create",
        b"yinz 0.1.1\0",
    ] {
        assert!(legacy_build_identity(invalid).is_err());
    }
    assert!(legacy_build_identity(&[b'x'; 257]).is_err());
}

fn legacy_peer(mode: &str) -> (tempfile::TempDir, EnvironmentVariables) {
    let directory = tempfile::tempdir().unwrap();
    // SSH executes its last argument through the remote shell. Use a real shell
    // here so quoting and the fallback's command line are both exercised.
    yinz_test_support::write_warm_script(
        &directory.path().join("ssh"),
        "#!/bin/sh\nfor command do :; done\nexec /bin/sh -c \"$command\"\n",
    );
    yinz_test_support::write_warm_script(
        &directory.path().join("yinz"),
        r#"#!/bin/sh
[ -n "$YINZ_COMPAT_LOG" ] || exit 0
printf '%s\n' "$*" >> "$YINZ_COMPAT_LOG"
if [ "$1" = '--version' ]; then
    echo 'yinz legacy-fixture'
    exit 0
fi
if [ "$YINZ_COMPAT_MODE" = 'ordinary-error' ]; then
    echo 'session failed to initialize' >&2
    exit 1
fi
shift
while [ "$#" -gt 0 ]; do
    case "$1" in
        --client-version) version="$2"; shift 2 ;;
        --contract-version) contract="$2"; shift 2 ;;
        --bootstrap-version) bootstrap="$2"; shift 2 ;;
        *) exit 2 ;;
    esac
done
if [ "$YINZ_COMPAT_MODE" != 'modern' ] && [ "$version" != 'legacy-fixture' ]; then
    echo 'Yinz compatibility mismatch: exact release required' >&2
    exit 1
fi
if [ "$YINZ_COMPAT_MODE" = 'incompatible' ] || [ "$contract" != "$YINZ_COMPAT_CONTRACT" ] || [ "$bootstrap" != "$YINZ_COMPAT_BOOTSTRAP" ]; then
    echo 'Yinz compatibility mismatch: incompatible wire contracts' >&2
    exit 1
fi
printf '{"yinz_version":"legacy-fixture","contract_version":%s,"bootstrap_version":%s,"sessions":[]}\n' "$contract" "$bootstrap"
"#,
    );
    let env = EnvironmentVariables::from_data(
        [
            ("PATH".into(), directory.path().to_str().unwrap().into()),
            (
                "YINZ_COMPAT_LOG".into(),
                directory.path().join("calls").to_str().unwrap().into(),
            ),
            ("YINZ_COMPAT_MODE".into(), mode.into()),
            (
                "YINZ_COMPAT_CONTRACT".into(),
                CLIENT_SERVER_CONTRACT_VERSION.to_string(),
            ),
            (
                "YINZ_COMPAT_BOOTSTRAP".into(),
                REMOTE_BOOTSTRAP_VERSION.to_string(),
            ),
        ]
        .into(),
    );
    (directory, env)
}

#[test]
fn legacy_bootstrap_negotiates_without_relaxing_wire_contracts() {
    for (mode, success, calls) in [
        ("modern", true, 1),
        ("legacy", true, 3),
        ("incompatible", false, 3),
        ("ordinary-error", false, 1),
    ] {
        let (directory, env) = legacy_peer(mode);
        let result = negotiate_legacy_version("fixture-host", &env, &AtomicBool::new(false));
        assert_eq!(result.is_ok(), success, "{mode}: {result:?}");
        if mode == "modern" {
            assert_eq!(result.unwrap(), VERSION);
        } else if mode == "legacy" {
            assert_eq!(result.unwrap(), "legacy-fixture");
        }
        let log = std::fs::read_to_string(directory.path().join("calls")).unwrap();
        assert_eq!(log.lines().count(), calls, "{mode}: {log}");
        for line in log
            .lines()
            .filter(|line| line.starts_with("remote-sessions"))
        {
            assert!(
                line.contains(&format!(
                    "--contract-version {CLIENT_SERVER_CONTRACT_VERSION}"
                )),
                "{line}"
            );
            assert!(
                line.contains(&format!("--bootstrap-version {REMOTE_BOOTSTRAP_VERSION}")),
                "{line}"
            );
        }
    }
}

#[test]
fn cancelled_negotiation_does_not_start_ssh() {
    let (directory, env) = legacy_peer("legacy");
    assert!(negotiate_legacy_version("fixture-host", &env, &AtomicBool::new(true)).is_err());
    assert!(!directory.path().join("calls").exists());
}
