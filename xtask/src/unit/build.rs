use super::*;
use std::ffi::OsStr;
use std::fs::{self, FileTimes, OpenOptions};
use std::time::{Duration, SystemTime};

/// Writes `contents` to `path` under `root`, creating its directories.
fn write(root: &Path, path: &str, contents: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Writes the dep-info cargo leaves beside `name`'s debug wasm, listing `files` the way cargo
/// does: absolute, joined to `root` as given, with spaces escaped.
fn write_dep_info(root: &Path, name: &str, files: &[&str]) -> PathBuf {
    let debug = root.join("target/wasm32-wasip1/debug");
    let escape = |path: &Path| path.to_str().unwrap().replace(' ', "\\ ");
    let prerequisites: String = files
        .iter()
        .map(|file| format!(" {}", escape(&root.join(file))))
        .collect();
    let dep_info = debug.join(name).with_extension("d");
    fs::create_dir_all(&debug).unwrap();
    fs::write(
        &dep_info,
        format!(
            "{}:{prerequisites}\n",
            escape(&dep_info.with_extension("wasm"))
        ),
    )
    .unwrap();
    dep_info
}

/// A checkout with every `PLUGIN_BUILD_FILES` entry in it.
fn checkout() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for file in PLUGIN_BUILD_FILES {
        write(root.path(), file, file);
    }
    root
}

/// The canonical checkout and what the debug plugin build of it read, or `None` before there is
/// one. `cargo x test` builds the debug plugins first.
fn built_plugin_inputs() -> Option<(PathBuf, BTreeSet<PathBuf>)> {
    let dep_infos = plugin_dep_infos("dev");
    if let Some(missing) = dep_infos.iter().find(|dep_info| !dep_info.is_file()) {
        eprintln!(
            "no debug plugin build to check: {} is missing",
            missing.display()
        );
        return None;
    }
    let root = crate::project_root().canonicalize().unwrap();
    // `None` here would degrade every build to running cargo: a dep-info format this does not
    // parse, or a listed file that is gone.
    let inputs = plugin_input_files(&root, &dep_infos).expect(
        "the debug dep-infos should list only files in this checkout; if plugin sources moved \
         since the last build, run `cargo x build --plugins-only`",
    );
    Some((root, inputs))
}

/// `PLUGINS` is the one list a new crate may need, so it must match the workspace: `-p` takes the
/// package name, and cargo names the wasm and its dep-info after the binary.
#[test]
fn plugins_are_every_binary_under_default_plugins() {
    let sh = Shell::new().unwrap();
    let metadata = crate::metadata::get_cargo_metadata(&sh).unwrap();
    let plugins_dir = crate::project_root()
        .canonicalize()
        .unwrap()
        .join("default-plugins");
    let mut binaries = BTreeSet::new();
    for package in metadata["packages"].as_array().unwrap() {
        let dir = Path::new(package["manifest_path"].as_str().unwrap())
            .parent()
            .unwrap()
            .canonicalize()
            .unwrap();
        if !dir.starts_with(&plugins_dir) {
            continue;
        }
        let name = package["name"].as_str().unwrap();
        for target in package["targets"].as_array().unwrap() {
            if target["kind"]
                .as_array()
                .unwrap()
                .iter()
                .any(|kind| kind == "bin")
            {
                assert_eq!(target["name"], name, "{name} names its binary differently");
                binaries.insert(name);
            }
        }
    }
    let plugins: BTreeSet<_> = PLUGINS.iter().copied().collect();
    assert_eq!(plugins.len(), PLUGINS.len(), "PLUGINS lists a plugin twice");
    assert_eq!(
        binaries, plugins,
        "every plugin crate needs an entry in PLUGINS, and a library crate needs none"
    );
}

#[test]
fn dep_info_parsing_follows_cargo() {
    let paths = |paths: &[&str]| Some(paths.iter().map(PathBuf::from).collect::<Vec<_>>());
    assert_eq!(
        dep_info_dependencies("/t/a.wasm: /r/a\\ b.rs /r/c.ts\n"),
        paths(&["/r/a b.rs", "/r/c.ts"])
    );
    // rustc's own format: a rule per file, and comments for what it read from the environment.
    assert_eq!(
        dep_info_dependencies("/t/a.d: /r/a.rs\n\n/r/a.rs:\n\n# env-dep:PREFIX=/usr\n"),
        paths(&["/r/a.rs"])
    );
    assert_eq!(dep_info_dependencies("/t/a.wasm:\n"), paths(&[]));
    for text in [
        "",
        "# env-dep:PREFIX\n",
        // A continuation line: better to rebuild than to hash the first line alone.
        "/t/a.wasm: /r/a.rs \\\n  /r/b.rs\n",
    ] {
        assert_eq!(dep_info_dependencies(text), None, "{text:?}");
    }
}

#[test]
fn plugin_stamp_follows_the_dep_info_not_mtimes_or_unlisted_files() {
    let root = checkout();
    let root = root.path();
    for file in [
        "default-plugins/example/Cargo.toml",
        "default-plugins/example/src/main.rs",
        "default-plugins/example/README.md",
        "default-plugins/example/src/unit/main_tests.rs",
        // A stale plugin binary lying around is output, not input.
        "zellij-config/assets/plugins/tab-bar.wasm",
    ] {
        write(root, file, file);
    }
    let dep_infos = [write_dep_info(
        root,
        "example",
        &["default-plugins/example/src/main.rs"],
    )];
    let hash = || hash_plugin_files(root, &dep_infos).unwrap().finalize();

    let original = hash();
    OpenOptions::new()
        .write(true)
        .open(root.join("default-plugins/example/src/main.rs"))
        .unwrap()
        .set_times(FileTimes::new().set_modified(SystemTime::now() + Duration::from_secs(1)))
        .unwrap();
    assert_eq!(original, hash());

    for unlisted in [
        "default-plugins/example/README.md",
        "default-plugins/example/src/unit/main_tests.rs",
        "zellij-config/assets/plugins/tab-bar.wasm",
    ] {
        write(root, unlisted, "changed\n");
    }
    assert_eq!(original, hash());

    // The source; its package's manifest, and a build script cargo finds by name, which no
    // dep-info lists until it has run; and the build files.
    let mut previous = original;
    for input in [
        "default-plugins/example/src/main.rs",
        "default-plugins/example/Cargo.toml",
        "default-plugins/example/build.rs",
    ]
    .into_iter()
    .chain(PLUGIN_BUILD_FILES.iter().copied())
    {
        write(root, input, "changed\n");
        let changed = hash();
        assert_ne!(previous, changed, "{input}");
        previous = changed;
    }
}

/// Hocus compiles in the SDK typings, the example plugins and its bundled skill, none of them Rust
/// and some outside any crate that plugins build.
#[test]
fn an_include_str_input_invalidates_the_plugin_stamp() {
    let root = checkout();
    let root = root.path();
    let included = [
        "default-plugins/hocus/skills/project-setup/SKILL.md",
        "example-plugins/clock.ts",
        "example-plugins/with space.ts",
        "zellij-plugin-host/Cargo.toml",
        "zellij-plugin-host/src/deno/sdk.d.ts",
    ];
    for file in included.iter().chain(&[
        "default-plugins/hocus/Cargo.toml",
        "default-plugins/hocus/src/main.rs",
    ]) {
        write(root, file, file);
    }
    // How rustc records an `include_str!("../../../…")` in `default-plugins/hocus/src`.
    let from_src = |file: &str| format!("default-plugins/hocus/src/../../../{file}");
    let dep_infos = [write_dep_info(
        root,
        "hocus",
        &[
            "default-plugins/hocus/skills/project-setup/SKILL.md",
            "default-plugins/hocus/src/main.rs",
            &from_src("example-plugins/clock.ts"),
            &from_src("example-plugins/with space.ts"),
            &from_src("zellij-plugin-host/Cargo.toml"),
            &from_src("zellij-plugin-host/src/deno/sdk.d.ts"),
        ],
    )];

    let files = plugin_input_files(root, &dep_infos).unwrap();
    for file in included {
        assert!(files.contains(Path::new(file)), "{file}: {files:?}");
    }
    assert!(
        files
            .iter()
            .all(|file| !file.to_string_lossy().contains("..")),
        "{files:?}"
    );

    let mut previous = hash_plugin_files(root, &dep_infos).unwrap().finalize();
    for file in included {
        write(root, file, "changed\n");
        let changed = hash_plugin_files(root, &dep_infos).unwrap().finalize();
        assert_ne!(previous, changed, "{file}");
        previous = changed;
    }
}

/// Without a dep-info from this checkout there is nothing to say the wasm is current.
#[test]
fn plugin_stamp_needs_dep_info_from_this_checkout() {
    let root = checkout();
    let root = root.path();
    write(
        root,
        "default-plugins/example/src/main.rs",
        "fn main() {}\n",
    );
    let built = write_dep_info(root, "example", &["default-plugins/example/src/main.rs"]);
    assert!(hash_plugin_files(root, std::slice::from_ref(&built)).is_some());

    let no_rule = root.join("target/wasm32-wasip1/debug/no-rule.d");
    fs::write(&no_rule, "# env-dep:PREFIX\n").unwrap();
    // A `target/` another worktree built into.
    let elsewhere = tempfile::tempdir().unwrap();
    write(elsewhere.path(), "src/main.rs", "fn main() {}\n");
    let elsewhere_source = elsewhere.path().join("src/main.rs");
    for dep_info in [
        root.join("target/wasm32-wasip1/debug/never-built.d"),
        no_rule,
        write_dep_info(root, "deleted", &["default-plugins/example/src/gone.rs"]),
        write_dep_info(root, "shared", &[elsewhere_source.to_str().unwrap()]),
    ] {
        let dep_infos = [built.clone(), dep_info];
        assert!(
            plugin_input_files(root, &dep_infos).is_none(),
            "{dep_infos:?}"
        );
        assert!(
            hash_plugin_files(root, &dep_infos).is_none(),
            "{dep_infos:?}"
        );
    }
}

/// Every workspace crate a plugin compiles against must feed the rebuild stamp, or an edit to it
/// leaves stale wasm behind. This reads the real dep-info, so it checks what the stamp hashes.
#[test]
fn plugin_stamp_covers_every_workspace_crate_the_plugins_build() {
    let Some((root, inputs)) = built_plugin_inputs() else {
        return;
    };
    // A build file that is not there would leave the stamp unwritten, and every build running
    // cargo.
    for file in PLUGIN_BUILD_FILES {
        assert!(root.join(file).is_file(), "{file}");
    }
    let sh = Shell::new().unwrap();
    let metadata = crate::metadata::get_cargo_metadata(&sh).unwrap();
    let packages = metadata["packages"].as_array().unwrap();
    let directory_of = |manifest: &str| {
        Path::new(manifest)
            .parent()
            .unwrap()
            .canonicalize()
            .unwrap()
            .strip_prefix(&root)
            .unwrap()
            .to_path_buf()
    };
    let mut pending: Vec<PathBuf> = PLUGINS
        .iter()
        .map(|name| {
            let package = packages.iter().find(|p| p["name"] == *name).unwrap();
            directory_of(package["manifest_path"].as_str().unwrap())
        })
        .collect();
    let mut seen = BTreeSet::new();
    while let Some(dir) = pending.pop() {
        if !seen.insert(dir.clone()) {
            continue;
        }
        let package = packages
            .iter()
            .find(|package| directory_of(package["manifest_path"].as_str().unwrap()) == dir)
            .unwrap();
        for dependency in package["dependencies"].as_array().unwrap() {
            // Plugins build only for wasm32-wasip1, so neither dev nor native-only edges reach a
            // plugin's compile.
            if dependency["kind"] == "dev"
                || dependency["target"] == r#"cfg(not(target_family = "wasm"))"#
            {
                continue;
            }
            if let Some(path) = dependency["path"].as_str() {
                pending.push(directory_of(&format!("{path}/Cargo.toml")));
            }
        }
    }
    for dir in seen {
        let manifest = dir.join("Cargo.toml");
        assert!(
            inputs.contains(&manifest)
                && inputs
                    .iter()
                    .any(|file| file.starts_with(&dir) && *file != manifest),
            "the plugin stamp does not cover {}; if plugin dependencies changed since the last \
             build, run `cargo x build --plugins-only`",
            dir.display()
        );
    }
}

/// The plugins' `env!` and `option_env!` reads are build inputs too, and no dep-info lists them.
#[test]
fn plugin_stamp_hashes_what_the_plugins_read_from_the_environment() {
    let Some((root, inputs)) = built_plugin_inputs() else {
        return;
    };
    for file in inputs
        .iter()
        .filter(|file| file.extension() == Some(OsStr::new("rs")))
    {
        let text = fs::read_to_string(root.join(file)).unwrap();
        for (at, _) in text.match_indices("env!(") {
            let Some((name, _)) = text[at + "env!(".len()..]
                .trim_start()
                .strip_prefix('"')
                .and_then(|rest| rest.split_once('"'))
            else {
                continue;
            };
            // What cargo itself sets from the manifest and the checkout, both already hashed.
            let from_cargo = name.starts_with("CARGO_PKG_")
                || matches!(
                    name,
                    "CARGO_MANIFEST_DIR"
                        | "CARGO_MANIFEST_PATH"
                        | "CARGO_CRATE_NAME"
                        | "CARGO_BIN_NAME"
                        | "CARGO_PRIMARY_PACKAGE"
                        | "OUT_DIR"
                );
            assert!(
                from_cargo || plugin_build_environment_variable(OsStr::new(name)),
                "{} reads {name} at compile time, but the plugin stamp does not hash it",
                file.display()
            );
        }
    }
}

#[test]
fn profile_dirs_follow_cargo() {
    // Cargo builds the two built-in profiles into directories that are not named after the
    // profile; every other profile gets a directory of its own name.
    assert_eq!(profile_dir("dev"), "debug");
    assert_eq!(profile_dir("test"), "debug");
    assert_eq!(profile_dir("release"), "release");
    assert_eq!(profile_dir("bench"), "release");
    assert_eq!(profile_dir("release-fast"), "release-fast");
}

#[test]
fn each_profile_gets_its_own_plugin_stamp() {
    // The two built-in profiles keep the stamp names they have always had, so an existing
    // tree does not rebuild its plugins once after this flag was added.
    assert!(plugin_stamp_path("dev").ends_with(".xtask-plugins-stamp"));
    assert!(plugin_stamp_path("release").ends_with(".xtask-plugins-release-stamp"));
    assert!(plugin_stamp_path("release-fast").ends_with(".xtask-plugins-release-fast-stamp"));
}

#[test]
fn profile_flag_wins_over_release() {
    let build = |release, profile: Option<&str>| flags::Build {
        release,
        profile: profile.map(str::to_string),
        plugins_only: false,
        no_plugins: false,
        args: vec![],
    };
    assert_eq!(profile_of(&build(false, None)), "dev");
    assert_eq!(profile_of(&build(true, None)), "release");
    assert_eq!(
        profile_of(&build(false, Some("release-fast"))),
        "release-fast"
    );
}

#[test]
fn plugin_hash_tracks_effective_external_cargo_config() {
    let base = std::env::temp_dir().join(format!(
        "zellij-xtask-cargo-config-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let root = base.join("workspace");
    let cargo_home = base.join("cargo-home");
    fs::create_dir_all(root.join(".cargo")).unwrap();
    fs::create_dir_all(base.join(".cargo")).unwrap();
    fs::create_dir_all(&cargo_home).unwrap();
    fs::write(
        root.join(".cargo/config"),
        "[build]\nrustflags = ['legacy']\n",
    )
    .unwrap();
    fs::write(
        root.join(".cargo/config.toml"),
        "[build]\nrustflags = ['ignored']\n",
    )
    .unwrap();
    fs::write(base.join(".cargo/config.toml"), "[build]\njobs = 1\n").unwrap();
    fs::write(cargo_home.join("config.toml"), "[net]\noffline = true\n").unwrap();

    assert_eq!(
        cargo_config_files(&root, Some(&cargo_home)),
        vec![
            root.join(".cargo/config"),
            base.join(".cargo/config.toml"),
            cargo_home.join("config.toml"),
        ]
    );

    let hash = |root: &Path, cargo_home: &Path| {
        let mut hasher = Sha256::new();
        hash_cargo_configs(root, Some(cargo_home), &mut hasher).unwrap();
        hasher.finalize()
    };
    let original = hash(&root, &cargo_home);
    fs::write(
        root.join(".cargo/config.toml"),
        "[build]\nrustflags = ['still ignored']\n",
    )
    .unwrap();
    assert_eq!(original, hash(&root, &cargo_home));

    fs::write(
        cargo_home.join("config.toml"),
        "[target.wasm32-wasip1]\nrustflags = ['home']\n",
    )
    .unwrap();
    assert_ne!(original, hash(&root, &cargo_home));
    fs::write(cargo_home.join("config.toml"), "[net]\noffline = true\n").unwrap();

    fs::write(
        base.join(".cargo/config.toml"),
        "[target.wasm32-wasip1]\nrustflags = ['changed']\n",
    )
    .unwrap();
    assert_ne!(original, hash(&root, &cargo_home));

    fs::remove_dir_all(base).unwrap();
}

#[test]
fn plugin_hash_ignores_runtime_rust_environment() {
    for key in [
        "RUSTFLAGS",
        "RUSTC",
        "RUSTC_BOOTSTRAP",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_RUSTC",
        "CARGO_PROFILE_RELEASE_LTO",
        "CARGO_TARGET_WASM32_WASIP1_LINKER",
        "PREFIX",
    ] {
        assert!(plugin_build_environment_variable(OsStr::new(key)), "{key}");
    }
    for key in ["RUST_BACKTRACE", "RUST_LOG"] {
        assert!(!plugin_build_environment_variable(OsStr::new(key)), "{key}");
    }
}

/// `dev` and `test` share `target/wasm32-wasip1/debug` and its stamp, so they must get the
/// same wasm settings; a profile with a tree of its own gets none.
#[test]
fn debug_dir_profiles_share_plugin_overrides() {
    let nobody_set = |_: &str| false;
    let dev = plugin_profile_overrides_given("dev", nobody_set);
    assert!(dev.iter().any(|(key, _)| *key == DEV_OPT_LEVEL), "{dev:?}");
    assert_eq!(plugin_profile_overrides_given("test", nobody_set), dev);
    for profile in ["release", "bench", "release-fast"] {
        assert!(
            plugin_profile_overrides_given(profile, nobody_set).is_empty(),
            "{profile}"
        );
    }
}

/// A profile setting the user exported wins, and is still what the stamp hashes.
#[test]
fn a_profile_setting_the_user_exported_is_left_to_them() {
    let overrides = plugin_profile_overrides_given("dev", |key| key == DEV_OPT_LEVEL);
    assert!(
        overrides.iter().all(|(key, _)| *key != DEV_OPT_LEVEL),
        "{overrides:?}"
    );
    assert!(!overrides.is_empty(), "the others still apply");
    for (key, _) in plugin_profile_overrides_given("dev", |_| false) {
        assert!(plugin_build_environment_variable(OsStr::new(key)), "{key}");
    }
}

/// The stamp is what decides whether the wasm is rebuilt: `dev` and `test` write the same
/// one, so they must agree on it, and it must move with the settings a build applies.
#[test]
fn dev_and_test_agree_on_the_plugin_stamp() {
    let Some(dev) = hash_plugin_inputs("dev") else {
        return;
    };
    assert_eq!(hash_plugin_inputs("test").as_deref(), Some(dev.as_str()));
    if !plugin_profile_overrides("dev").is_empty() {
        assert_ne!(hash_plugin_inputs("release").as_deref(), Some(dev.as_str()));
    }
}
