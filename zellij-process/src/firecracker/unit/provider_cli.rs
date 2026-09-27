use super::*;

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
        zellij: None,
        disk_size_gib: 64,
        rebuild: true,
    };
    let mut built = false;
    setup_image_with(
        &options,
        project.path(),
        Path::new("/bin/zellij"),
        Path::new("/bin/zellij-firecracker"),
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
                    "--zellij",
                    "/bin/zellij",
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
