//! Subcommands for building.
//!
//! Currently has the following functions:
//!
//! - [`build`]: Builds general cargo projects (i.e. zellij components) with `cargo build`
use crate::{flags, WorkspaceMember};
use anyhow::Context;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use xshell::{cmd, Shell};

const PLUGIN_INPUT_DIRS: &[&str] = &[
    "default-plugins",
    "zellij-tile/src",
    "zellij-tile-utils/src",
    "zellij-utils/src",
    "zellij-utils/assets",
];

const PLUGIN_INPUT_FILES: &[&str] = &[
    "Cargo.lock",
    "Cargo.toml",
    "rust-toolchain.toml",
    "xtask/src/build.rs",
    "xtask/src/main.rs",
    "zellij-tile/Cargo.toml",
    "zellij-tile-utils/Cargo.toml",
    "zellij-utils/Cargo.toml",
];

/// Build members of the zellij workspace.
///
/// Build behavior is controlled by the [`flags`](flags::Build). Calls some variation of `cargo
/// build` under the hood.
pub fn build(sh: &Shell, flags: flags::Build) -> anyhow::Result<()> {
    let _pd = sh.push_dir(crate::project_root());

    let cargo = crate::cargo()?;
    if flags.no_plugins && flags.plugins_only {
        eprintln!("Cannot use both '--no-plugins' and '--plugins-only'");
        std::process::exit(1);
    }

    // zellij-utils requires protobuf definition files to be present. Usually these are
    // auto-generated with `build.rs`-files, but this is currently broken for us.
    // See [this PR][1] for details.
    //
    // [1]: https://github.com/zellij-org/zellij/pull/2711#issuecomment-1695015818
    run_proto_codegen(sh, false);

    if flags.release && !flags.no_plugins {
        ensure_release_plugins(sh)?;
    } else if !flags.no_plugins
        && (plugins_force()
            || plugin_outputs_missing(plugin_names().into_iter().map(debug_plugin_path))
            || plugin_sources_changed())
    {
        // Build all plugins in a single invocation so Cargo can unify transitive dependency
        // features across all of them and compile shared crates (e.g. zellij-utils) only once.
        let plugin_members: Vec<&WorkspaceMember> = crate::workspace_members()
            .iter()
            .filter(|m| m.build && m.crate_name.contains("plugins"))
            .collect();

        if !plugin_members.is_empty() {
            eprintln!();
            let msg = ">> Building plugins";
            crate::status(msg);
            eprintln!("{}", msg);

            let mut base_cmd = cmd!(sh, "{cargo} build --target wasm32-wasip1");
            for member in &plugin_members {
                base_cmd = base_cmd.args(["-p", plugin_name_of(member)?]);
            }
            base_cmd.run().context("failed to build plugins")?;
            write_plugin_stamp(sh);
        }
    } else if !flags.no_plugins {
        let msg = ">> Plugins unchanged since last build, skipping (set ZELLIJ_FORCE_PLUGINS=1 to rebuild)";
        crate::status(msg);
        eprintln!("{}", msg);
    }

    // Build non-plugin crates (native target).
    if !flags.plugins_only {
        for WorkspaceMember { crate_name, .. } in crate::workspace_members()
            .iter()
            .filter(|member| member.build && !member.crate_name.contains("plugins"))
        {
            let err_context = || format!("failed to build '{crate_name}'");

            let _pd = sh.push_dir(Path::new(crate_name));
            println!();
            let msg = format!(">> Building '{crate_name}'");
            crate::status(&msg);
            println!("{}", msg);

            let mut base_cmd = cmd!(sh, "{cargo} build");
            if flags.release {
                base_cmd = base_cmd.arg("--release");
            }
            base_cmd = base_cmd.args(&flags.args);
            base_cmd.run().with_context(err_context)?;
        }
    }

    Ok(())
}

fn plugins_force() -> bool {
    std::env::var_os("ZELLIJ_FORCE_PLUGINS").is_some()
}

/// The plugin crates `cargo x build -p` builds, by wasm file stem.
pub fn plugin_names() -> Vec<&'static str> {
    crate::workspace_members()
        .iter()
        .filter(|m| m.build && m.crate_name.contains("plugins"))
        .filter_map(|m| plugin_name_of(m).ok())
        .collect()
}

/// Plugins that exist for the test suites and must not be installed for users.
pub const TEST_ONLY_PLUGINS: &[&str] = &["fixture-plugin-for-tests", "shared-test"];

fn wasm_target_path(profile: &str, plugin_name: &str) -> PathBuf {
    crate::target_dir()
        .join("wasm32-wasip1")
        .join(profile)
        .join(plugin_name)
        .with_extension("wasm")
}

/// Where `cargo build --target wasm32-wasip1 --release` leaves a plugin.
pub fn release_plugin_path(plugin_name: &str) -> PathBuf {
    wasm_target_path("release", plugin_name)
}

/// Where the debug plugin build leaves a plugin - what a debug zellij looks for first.
pub fn debug_plugin_path(plugin_name: &str) -> PathBuf {
    wasm_target_path("debug", plugin_name)
}

fn plugin_outputs_missing(paths: impl IntoIterator<Item = PathBuf>) -> bool {
    paths.into_iter().any(|path| !path.is_file())
}

/// Hashes the inputs which can change plugin wasm, so checkout and worktree mtime changes do not
/// cause rebuilds while dependency, toolchain and build-flag changes still do.
fn hash_plugin_inputs() -> Option<String> {
    let root = crate::project_root();
    let mut hasher = hash_plugin_files(&root)?;
    hash_cargo_configs(&root, cargo_home(&root).as_deref(), &mut hasher)?;

    let cargo = crate::cargo().ok()?;
    for program in [cargo, PathBuf::from("rustc")] {
        let output = Command::new(program).arg("-vV").output().ok()?;
        if !output.status.success() {
            return None;
        }
        hasher.update(output.stdout);
    }

    let mut environment: Vec<_> = std::env::vars_os()
        .filter(|(key, _)| plugin_build_environment_variable(key))
        .collect();
    environment.sort();
    for (key, value) in environment {
        hasher.update(key.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(value.to_string_lossy().as_bytes());
        hasher.update([0]);
    }

    Some(
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

fn cargo_home(root: &Path) -> Option<PathBuf> {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))?;
    Some(if cargo_home.is_absolute() {
        cargo_home
    } else {
        root.join(cargo_home)
    })
}

fn hash_cargo_configs(root: &Path, cargo_home: Option<&Path>, hasher: &mut Sha256) -> Option<()> {
    for path in cargo_config_files(root, cargo_home) {
        let identity = path.strip_prefix(root).unwrap_or(&path);
        hasher.update(identity.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(std::fs::read(path).ok()?);
        hasher.update([0]);
    }
    Some(())
}

fn cargo_config_files(root: &Path, cargo_home: Option<&Path>) -> Vec<PathBuf> {
    let mut config_dirs: Vec<_> = root
        .ancestors()
        .map(|ancestor| ancestor.join(".cargo"))
        .collect();
    config_dirs.extend(cargo_home.map(Path::to_path_buf));

    let mut configs = vec![];
    for dir in config_dirs {
        let legacy = dir.join("config");
        let config = if legacy.is_file() {
            Some(legacy)
        } else {
            let toml = dir.join("config.toml");
            toml.is_file().then_some(toml)
        };
        if let Some(config) = config {
            if !configs.contains(&config) {
                configs.push(config);
            }
        }
    }
    configs
}

fn plugin_build_environment_variable(key: &std::ffi::OsStr) -> bool {
    let key = key.to_string_lossy();
    matches!(
        key.as_ref(),
        "CARGO"
            | "CARGO_ENCODED_RUSTFLAGS"
            | "RUSTC"
            | "RUSTC_BOOTSTRAP"
            | "RUSTC_WORKSPACE_WRAPPER"
            | "RUSTC_WRAPPER"
            | "RUSTFLAGS"
    ) || key.starts_with("CARGO_BUILD_")
        || key.starts_with("CARGO_PROFILE_")
        || key.starts_with("CARGO_TARGET_WASM32_WASIP1_")
}

fn hash_plugin_files(root: &Path) -> Option<Sha256> {
    let mut files = vec![];
    for dir in PLUGIN_INPUT_DIRS {
        collect_files(&root.join(dir), &mut files);
    }
    files.extend(PLUGIN_INPUT_FILES.iter().map(|path| root.join(path)));
    if files.is_empty() {
        return None;
    }
    files.sort();
    files.dedup();

    let mut hasher = Sha256::new();
    for path in files {
        let relative = path.strip_prefix(root).ok()?;
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(std::fs::read(path).ok()?);
        hasher.update([0]);
    }
    Some(hasher)
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            let name = path.file_name().and_then(|name| name.to_str());
            if matches!(name, Some("benches" | "target" | "tests" | "unit")) {
                continue;
            }
            collect_files(&path, out);
        } else {
            let name = path.file_name().and_then(|name| name.to_str());
            // `.wasm` is build output, never an input: a stale, gitignored plugin left in
            // `zellij-utils/assets/plugins` by an older checkout must not perturb the hash.
            if !matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("md" | "snap" | "wasm")
            ) && !matches!(name, Some(".gitignore" | "test.rs"))
                && !name
                    .is_some_and(|name| name.ends_with("_test.rs") || name.ends_with("_tests.rs"))
            {
                out.push(path);
            }
        }
    }
}

/// Make sure every plugin's release wasm is present and fresh under
/// `target/wasm32-wasip1/release`, building only when the inputs changed since the last
/// release build (or `ZELLIJ_FORCE_PLUGINS` is set).
pub fn ensure_release_plugins(sh: &Shell) -> anyhow::Result<()> {
    let plugin_members: Vec<&WorkspaceMember> = crate::workspace_members()
        .iter()
        .filter(|m| m.build && m.crate_name.contains("plugins"))
        .collect();
    if plugin_members.is_empty() {
        return Ok(());
    }

    let missing = plugin_members
        .iter()
        .any(|member| match plugin_name_of(member) {
            Ok(name) => !release_plugin_path(name).is_file(),
            Err(_) => true,
        });
    let stale = plugins_force() || missing || release_plugin_sources_changed();

    if !stale {
        let msg = ">> Release plugins up to date, skipping plugin build";
        crate::status(msg);
        eprintln!("{}", msg);
        return Ok(());
    }

    let msg = ">> Building plugins (release)";
    crate::status(msg);
    eprintln!("{}", msg);

    build_plugins_release(sh, &plugin_members)
}

fn build_plugins_release(sh: &Shell, plugin_members: &[&WorkspaceMember]) -> anyhow::Result<()> {
    let cargo = crate::cargo()?;
    let mut base_cmd = cmd!(sh, "{cargo} build --target wasm32-wasip1 --release");
    for member in plugin_members {
        let plugin_name = plugin_name_of(member)?;
        base_cmd = base_cmd.args(["-p", plugin_name]);
    }
    base_cmd.run().context("failed to build release plugins")?;
    write_release_plugin_stamp(sh);
    Ok(())
}

fn plugin_name_of(member: &WorkspaceMember) -> anyhow::Result<&'static str> {
    Ok(member
        .crate_name
        .rsplit_once('/')
        .context("Cannot determine plugin name from crate path")?
        .1)
}

fn plugin_stamp_path() -> PathBuf {
    crate::target_dir().join(".xtask-plugins-stamp")
}

fn release_plugin_stamp_path() -> PathBuf {
    crate::target_dir().join(".xtask-plugins-release-stamp")
}

fn write_plugin_stamp(sh: &Shell) {
    if let Some(hash) = hash_plugin_inputs() {
        let _ = sh.write_file(plugin_stamp_path(), hash);
    }
}

fn write_release_plugin_stamp(sh: &Shell) {
    if let Some(hash) = hash_plugin_inputs() {
        let _ = sh.write_file(release_plugin_stamp_path(), hash);
    }
}

fn plugin_sources_changed() -> bool {
    sources_changed_since(&plugin_stamp_path())
}

fn release_plugin_sources_changed() -> bool {
    sources_changed_since(&release_plugin_stamp_path())
}

fn sources_changed_since(stamp_path: &Path) -> bool {
    let stamp_hash = match std::fs::read_to_string(stamp_path) {
        Ok(contents) => contents,
        Err(_) => return true,
    };
    match hash_plugin_inputs() {
        Some(current_hash) => stamp_hash != current_hash,
        None => true,
    }
}

pub fn proto(sh: &Shell) -> anyhow::Result<()> {
    let msg = ">> Generating protobuffer code";
    crate::status(msg);
    println!("{}", msg);

    run_proto_codegen(sh, true);
    Ok(())
}

fn run_proto_codegen(sh: &Shell, force: bool) {
    let zellij_utils_basedir = crate::project_root().join("zellij-utils");
    let _pd = sh.push_dir(&zellij_utils_basedir);

    let specs: &[(&str, &str, &str)] = &[
        ("assets/prost", "src/plugin_api", "generated_plugin_api.rs"),
        (
            "assets/prost_ipc",
            "src/client_server_contract",
            "generated_client_server_api.rs",
        ),
        (
            "assets/prost_nested_session",
            "src/nested_session_contract",
            "generated_nested_session_api.rs",
        ),
    ];

    for (out_subdir, src_subdir, include_file) in specs {
        let out_dir = sh.current_dir().join(out_subdir);
        let src_dir = sh.current_dir().join(src_subdir);
        std::fs::create_dir_all(&out_dir).unwrap();

        let last_generated = out_dir
            .join(include_file)
            .metadata()
            .and_then(|m| m.modified());
        let mut proto_files = vec![];
        let mut needs_regeneration = force;

        for entry in std::fs::read_dir(&src_dir).unwrap() {
            let entry_path = entry.unwrap().path();
            if entry_path.is_file()
                && entry_path
                    .extension()
                    .map(|e| e == "proto")
                    .unwrap_or(false)
            {
                let modified = entry_path.metadata().and_then(|m| m.modified());
                needs_regeneration |= match (&last_generated, modified) {
                    (Ok(last_generated), Ok(modified)) => modified > *last_generated,
                    // Couldn't read some metadata, assume needs update
                    _ => true,
                };
                proto_files.push(entry_path.display().to_string());
            }
        }
        proto_files.sort();

        if needs_regeneration {
            let mut prost = prost_build::Config::new();
            prost.out_dir(&out_dir);
            prost.include_file(include_file);
            prost.compile_protos(&proto_files, &[src_dir]).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::fs::{self, FileTimes, OpenOptions};
    use std::time::{Duration, SystemTime};

    #[test]
    fn plugin_file_hash_tracks_build_inputs_not_mtimes_or_tests() {
        let root = std::env::temp_dir().join(format!(
            "zellij-xtask-plugin-inputs-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for dir in PLUGIN_INPUT_DIRS {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        for file in PLUGIN_INPUT_FILES {
            fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
            fs::write(root.join(file), file).unwrap();
        }

        let source = root.join("default-plugins/example/src/main.rs");
        let test = root.join("default-plugins/example/tests/plugin.rs");
        let adjacent_test = root.join("default-plugins/example/src/main_test.rs");
        let readme = root.join("default-plugins/example/README.md");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::create_dir_all(test.parent().unwrap()).unwrap();
        fs::write(&source, "fn main() {}\n").unwrap();
        fs::write(&test, "test\n").unwrap();
        fs::write(&adjacent_test, "test\n").unwrap();
        fs::write(&readme, "docs\n").unwrap();

        let original = hash_plugin_files(&root).unwrap().finalize();
        OpenOptions::new()
            .write(true)
            .open(&source)
            .unwrap()
            .set_times(FileTimes::new().set_modified(SystemTime::now() + Duration::from_secs(1)))
            .unwrap();
        assert_eq!(original, hash_plugin_files(&root).unwrap().finalize());

        fs::write(&test, "changed test\n").unwrap();
        fs::write(&adjacent_test, "changed test\n").unwrap();
        fs::write(&readme, "changed docs\n").unwrap();
        assert_eq!(original, hash_plugin_files(&root).unwrap().finalize());

        fs::write(&source, "fn main() { println!(\"changed\"); }\n").unwrap();
        let changed = hash_plugin_files(&root).unwrap().finalize();
        assert_ne!(original, changed);

        // A stale plugin binary lying around is output, not input.
        let stray_wasm = root.join("zellij-utils/assets/plugins/tab-bar.wasm");
        fs::create_dir_all(stray_wasm.parent().unwrap()).unwrap();
        fs::write(&stray_wasm, "wasm").unwrap();
        assert_eq!(changed, hash_plugin_files(&root).unwrap().finalize());

        fs::write(root.join("xtask/src/build.rs"), "changed build recipe\n").unwrap();
        let changed_recipe = hash_plugin_files(&root).unwrap().finalize();
        assert_ne!(changed, changed_recipe);

        fs::write(
            root.join("xtask/src/main.rs"),
            "changed plugin membership\n",
        )
        .unwrap();
        assert_ne!(changed_recipe, hash_plugin_files(&root).unwrap().finalize());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_plugin_output_is_not_fresh() {
        let root = tempfile::tempdir().unwrap();
        let present = root.path().join("present.wasm");
        fs::write(&present, "wasm").unwrap();

        assert!(!plugin_outputs_missing([present.clone()]));
        assert!(plugin_outputs_missing([
            present,
            root.path().join("missing.wasm"),
        ]));
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
        ] {
            assert!(plugin_build_environment_variable(OsStr::new(key)), "{key}");
        }
        for key in ["RUST_BACKTRACE", "RUST_LOG"] {
            assert!(!plugin_build_environment_variable(OsStr::new(key)), "{key}");
        }
    }
}
