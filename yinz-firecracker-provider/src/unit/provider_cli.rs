use super::*;

#[test]
fn minimal_guest_configuration_resets_identity_and_generates_keys_on_boot() {
    let root = tempfile::tempdir().unwrap();
    for directory in ["usr/local/bin", "etc/ssh", "var/lib/dbus"] {
        fs::create_dir_all(root.path().join(directory)).unwrap();
    }
    let key = root.path().join("client-key");
    fs::write(key.with_extension("pub"), "ssh-ed25519 test").unwrap();
    fs::write(
        root.path().join("etc/ssh/ssh_host_ed25519_key"),
        "shared key",
    )
    .unwrap();
    fs::write(root.path().join("etc/machine-id"), "shared identity").unwrap();
    fs::write(
        root.path().join("var/lib/dbus/machine-id"),
        "shared identity",
    )
    .unwrap();

    configure_guest(root.path(), &key).unwrap();

    assert_eq!(
        fs::read_to_string(root.path().join("root/.ssh/authorized_keys")).unwrap(),
        "ssh-ed25519 test"
    );
    assert!(!root.path().join("etc/ssh/ssh_host_ed25519_key").exists());
    assert_eq!(fs::read(root.path().join("etc/machine-id")).unwrap(), b"");
    assert!(!root.path().join("var/lib/dbus/machine-id").exists());
    assert!(fs::read_to_string(
        root.path()
            .join("etc/systemd/system/ssh.service.d/keys.conf")
    )
    .unwrap()
    .contains("ssh-keygen -A"));
    assert_eq!(
        fs::read_link(
            root.path().join(
                "etc/systemd/system/multi-user.target.wants/yinz-firecracker-network.service"
            )
        )
        .unwrap(),
        Path::new("../yinz-firecracker-network.service")
    );
    assert!(fs::read_to_string(
        root.path()
            .join("etc/ssh/sshd_config.d/99-yinz-firecracker.conf")
    )
    .unwrap()
    .contains("PasswordAuthentication no"));
}

#[test]
fn help_and_invalid_arguments_never_start_setup_or_image_builds() {
    for args in [
        vec!["--help"],
        vec!["setup", "--help"],
        vec!["image", "build", "--help"],
        vec!["help", "image"],
        vec!["doctor", "-h"],
        vec!["network", "setup", "--help"],
        vec!["network", "teardown", "--help"],
    ] {
        let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert!(cli_help(&args).unwrap().contains("Usage:"));
        run_cli(&args).unwrap();
    }
    for (args, expected) in [
        (
            vec!["image", "build", "--disk-size", "64"],
            "unknown argument",
        ),
        (
            vec!["image", "build", "--name", "--project", "."],
            "--name requires a value",
        ),
        (vec!["setup", "--name"], "--name requires a value"),
        (
            vec!["setup", "--name", "one", "--name", "two"],
            "more than once",
        ),
        (vec!["doctor", "extra"], "unknown argument"),
        (
            vec!["network", "setup", "--usr", "someone"],
            "unknown argument",
        ),
    ] {
        let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let error = run_cli(&args).unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn image_input_errors_are_reported_before_build_prerequisites() {
    let project = tempfile::tempdir().unwrap();
    let args = ["--project", project.path().to_str().unwrap()].map(str::to_owned);
    let error = image_build_options(&args).err().unwrap().to_string();
    assert!(
        error.contains("Create it to install project tools"),
        "{error}"
    );
    for value in ["0", "8589934592", "18446744073709551615"] {
        let args = ["--disk-size-gib", value].map(str::to_owned);
        let error = image_build_options(&args).err().unwrap().to_string();
        assert!(error.contains("--disk-size-gib must"), "{error}");
        assert!(parse_setup_options(&args).is_err());
    }
    assert_eq!(disk_size_bytes(8).unwrap(), 8 * 1024 * 1024 * 1024);
}

#[test]
fn setup_builds_a_base_image_when_a_project_has_no_recipe() {
    let project = tempfile::tempdir().unwrap();
    let options = SetupOptions {
        project: project.path().to_owned(),
        image: "dev".to_owned(),
        yinz: None,
        disk_size_gib: 64,
        rebuild: true,
    };
    let mut built = false;
    setup_image_with(
        &options,
        project.path(),
        Path::new("/bin/yinz"),
        Path::new("/bin/yinz-firecracker"),
        |_| Ok(PathBuf::from("/images/dev.ext4")),
        |command, _| {
            built = true;
            let args = command
                .get_args()
                .map(|arg| arg.to_str().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(
                args,
                [
                    "image",
                    "build",
                    "--name",
                    "dev",
                    "--yinz",
                    "/bin/yinz",
                    "--disk-size-gib",
                    "64"
                ]
            );
            Ok(())
        },
    )
    .unwrap();
    assert!(built);
}
