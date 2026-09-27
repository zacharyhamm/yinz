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
    "zellij-tile-markdown/src",
    "zellij-tile-utils/src",
    "zellij-config/src",
    "zellij-config/assets",
    "zellij-plugin-api/src",
    "zellij-plugin-api/assets",
    "zellij-ipc/src",
    "zellij-ipc/assets",
    "zellij-storage/src",
    "zellij-types/src",
    "zellij-runtime/src",
    "zellij-plugin-location/src",
];

const PLUGIN_INPUT_FILES: &[&str] = &[
    "Cargo.lock",
    "Cargo.toml",
    "rust-toolchain.toml",
    "xtask/src/build.rs",
    "xtask/src/main.rs",
    "zellij-tile/Cargo.toml",
    "zellij-tile-markdown/Cargo.toml",
    "zellij-tile-utils/Cargo.toml",
    "zellij-config/Cargo.toml",
    "zellij-plugin-api/Cargo.toml",
    "zellij-ipc/Cargo.toml",
    "zellij-storage/Cargo.toml",
    "zellij-types/Cargo.toml",
    "zellij-runtime/Cargo.toml",
    "zellij-plugin-location/Cargo.toml",
];

/// Build members of the zellij workspace.
///
/// Build behavior is controlled by the [`flags`](flags::Build). Calls some variation of `cargo
/// build` under the hood.
pub fn build(sh: &Shell, flags: flags::Build) -> anyhow::Result<()> {
    let _pd = sh.push_dir(crate::project_root());

    crate::authoring::prepare(sh)?;
    let cargo = crate::cargo()?;
    if flags.no_plugins && flags.plugins_only {
        eprintln!("Cannot use both '--no-plugins' and '--plugins-only'");
        std::process::exit(1);
    }
    if flags.release && flags.profile.is_some() {
        eprintln!("Cannot use both '--release' and '--profile'");
        std::process::exit(1);
    }
    let profile = profile_of(&flags);

    // zellij-plugin-api requires protobuf definition files to be present. Usually these are
    // auto-generated with `build.rs`-files, but this is currently broken for us.
    // See [this PR][1] for details.
    //
    // [1]: https://github.com/zellij-org/zellij/pull/2711#issuecomment-1695015818
    run_proto_codegen(sh, false);

    if !flags.no_plugins {
        ensure_plugins(sh, profile)?;
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

            let mut base_cmd = cmd!(sh, "{cargo} build --profile {profile}");
            base_cmd = base_cmd.args(&flags.args);
            base_cmd.run().with_context(err_context)?;
        }
    }

    Ok(())
}

fn plugins_force() -> bool {
    std::env::var_os("ZELLIJ_FORCE_PLUGINS").is_some()
}

/// The cargo profile a [`flags::Build`] asks for: an explicit `--profile` wins, otherwise
/// `--release` picks `release` and a plain build picks `dev`.
pub fn profile_of(flags: &flags::Build) -> &str {
    match flags.profile.as_deref() {
        Some(profile) => profile,
        None if flags.release => "release",
        None => "dev",
    }
}

/// The directory under `target/` cargo builds a profile into. Cargo names the two built-in
/// profiles' directories differently from the profiles themselves; every other profile gets a
/// directory of its own name.
pub fn profile_dir(profile: &str) -> &str {
    match profile {
        "dev" | "test" => "debug",
        "release" | "bench" => "release",
        other => other,
    }
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
pub const TEST_ONLY_PLUGINS: &[&str] = &[
    "fixture-plugin-for-tests",
    "shared-test",
    "status-bar",
    "tab-bar",
];

/// Where a plugin build for `profile` leaves a plugin.
pub fn plugin_path(profile: &str, plugin_name: &str) -> PathBuf {
    wasm_target_path(profile_dir(profile), plugin_name)
}

fn wasm_target_path(profile_dir: &str, plugin_name: &str) -> PathBuf {
    crate::target_dir()
        .join("wasm32-wasip1")
        .join(profile_dir)
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

/// Hashes the inputs which can change plugin wasm, so checkout and worktree mtime changes do not
/// cause rebuilds while dependency, toolchain and build-flag changes still do.
fn hash_plugin_inputs(profile: &str) -> Option<String> {
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
    // What `build_plugins` sets on the wasm build itself, which the process environment
    // above cannot see: a change to these must rebuild the plugins too.
    for (key, value) in plugin_profile_overrides(profile) {
        hasher.update(key.as_bytes());
        hasher.update([0]);
        hasher.update(value.as_bytes());
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

/// The dev profile's opt-level, which `plugin_profile_overrides` raises for the wasm build.
pub(crate) const DEV_OPT_LEVEL: &str = "CARGO_PROFILE_DEV_OPT_LEVEL";

/// Profile settings the wasm plugin build gets on top of the workspace `Cargo.toml`.
///
/// `[profile.dev.package."*"] opt-level = 3` keeps `wasmi` and the host's dependencies fast in
/// a dev build, but it deliberately excludes workspace members - and the plugins, `zellij-tile`,
/// `zellij-plugin-api` and `hocus-proto` all are. Left at opt-level 0 with DWARF they are
/// 20-100× slower under the interpreter on exactly the per-event decode path (a `--profile dev`
/// install once cost hocus 74 s of keystroke latency), and a plugin test loads the same wasm.
/// Setting these on the plugin build alone reaches only `target/wasm32-wasip1/debug`: the host
/// build is untouched, and the `package."*"` override still wins for dependencies. Overflow
/// checks and debug assertions stay on, so dev semantics do not change.
///
/// Keyed on the output directory rather than the profile name: `test` inherits `dev` and
/// builds into the same `debug` tree under the same stamp, so anything but the same settings
/// would leave the two rebuilding each other's wasm. A setting the user exported is theirs -
/// to step through a plugin with DWARF, say - and is left alone; the stamp already hashes it
/// as a `CARGO_PROFILE_` variable.
pub(crate) fn plugin_profile_overrides(profile: &str) -> Vec<(&'static str, &'static str)> {
    plugin_profile_overrides_given(profile, |key| std::env::var_os(key).is_some())
}

fn plugin_profile_overrides_given(
    profile: &str,
    user_set: impl Fn(&str) -> bool,
) -> Vec<(&'static str, &'static str)> {
    if profile_dir(profile) != "debug" {
        return Vec::new();
    }
    [(DEV_OPT_LEVEL, "2"), ("CARGO_PROFILE_DEV_DEBUG", "false")]
        .into_iter()
        .filter(|(key, _)| !user_set(key))
        .collect()
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
            // `zellij-config/assets/plugins` by an older checkout must not perturb the hash.
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

/// Make sure every plugin's wasm is present and fresh under
/// `target/wasm32-wasip1/<profile dir>`, building only when the inputs changed since the last
/// build of that profile (or `ZELLIJ_FORCE_PLUGINS` is set).
pub fn ensure_plugins(sh: &Shell, profile: &str) -> anyhow::Result<()> {
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
            Ok(name) => !plugin_path(profile, name).is_file(),
            Err(_) => true,
        });
    let stale = plugins_force() || missing || plugin_sources_changed(profile);

    if !stale {
        let msg = format!(
            ">> Plugins ({profile}) unchanged since last build, skipping \
             (set ZELLIJ_FORCE_PLUGINS=1 to rebuild)"
        );
        crate::status(&msg);
        eprintln!("{}", msg);
        return Ok(());
    }

    eprintln!();
    let msg = format!(">> Building plugins ({profile})");
    crate::status(&msg);
    eprintln!("{}", msg);

    build_plugins(sh, profile, &plugin_members)
}

/// Build all plugins in a single invocation so Cargo can unify transitive dependency features
/// across all of them and compile shared crates (e.g. zellij-plugin-api) only once.
fn build_plugins(
    sh: &Shell,
    profile: &str,
    plugin_members: &[&WorkspaceMember],
) -> anyhow::Result<()> {
    let cargo = crate::cargo()?;
    let mut base_cmd = cmd!(
        sh,
        "{cargo} build --target wasm32-wasip1 --profile {profile}"
    );
    for member in plugin_members {
        let plugin_name = plugin_name_of(member)?;
        base_cmd = base_cmd.args(["-p", plugin_name]);
    }
    for (key, value) in plugin_profile_overrides(profile) {
        base_cmd = base_cmd.env(key, value);
    }
    base_cmd
        .run()
        .with_context(|| format!("failed to build plugins ({profile})"))?;
    write_plugin_stamp(sh, profile);
    Ok(())
}

fn plugin_name_of(member: &WorkspaceMember) -> anyhow::Result<&'static str> {
    Ok(member
        .crate_name
        .rsplit_once('/')
        .context("Cannot determine plugin name from crate path")?
        .1)
}

/// One freshness stamp per profile directory. The two built-in profiles keep the names they
/// have always had, so an existing tree does not rebuild its plugins once.
fn plugin_stamp_path(profile: &str) -> PathBuf {
    let name = match profile_dir(profile) {
        "debug" => ".xtask-plugins-stamp".to_string(),
        "release" => ".xtask-plugins-release-stamp".to_string(),
        other => format!(".xtask-plugins-{other}-stamp"),
    };
    crate::target_dir().join(name)
}

fn write_plugin_stamp(sh: &Shell, profile: &str) {
    if let Some(hash) = hash_plugin_inputs(profile) {
        let _ = sh.write_file(plugin_stamp_path(profile), hash);
    }
}

fn plugin_sources_changed(profile: &str) -> bool {
    sources_changed_since(&plugin_stamp_path(profile), profile)
}

fn sources_changed_since(stamp_path: &Path, profile: &str) -> bool {
    let stamp_hash = match std::fs::read_to_string(stamp_path) {
        Ok(contents) => contents,
        Err(_) => return true,
    };
    match hash_plugin_inputs(profile) {
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
    let _pd = sh.push_dir(crate::project_root());

    let specs: &[(&str, &str, &str)] = &[
        (
            "zellij-plugin-api/assets",
            "zellij-plugin-api/src",
            "generated_plugin_api.rs",
        ),
        (
            "zellij-ipc/assets/prost_ipc",
            "zellij-ipc/src/client_server_contract",
            "generated_client_server_api.rs",
        ),
        (
            "zellij-ipc/assets/prost_nested_session",
            "zellij-ipc/src/nested_session_contract",
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

    /// Every workspace crate a plugin compiles against must feed the rebuild stamp, or an edit to
    /// it leaves stale wasm behind. Plugin crates themselves are covered by `default-plugins`.
    #[test]
    fn plugin_inputs_cover_every_workspace_crate_the_plugins_build() {
        let sh = Shell::new().unwrap();
        let metadata = crate::metadata::get_cargo_metadata(&sh).unwrap();
        let root = crate::project_root();
        let packages = metadata["packages"].as_array().unwrap();
        let directory_of = |package: &serde_json::Value| {
            Path::new(package["manifest_path"].as_str().unwrap())
                .parent()
                .unwrap()
                .strip_prefix(&root)
                .unwrap()
                .to_path_buf()
        };
        let mut pending: Vec<PathBuf> = packages
            .iter()
            .map(directory_of)
            .filter(|dir| dir.starts_with("default-plugins"))
            .collect();
        let mut seen = std::collections::BTreeSet::new();
        while let Some(dir) = pending.pop() {
            if !seen.insert(dir.clone()) {
                continue;
            }
            let package = packages
                .iter()
                .find(|package| directory_of(package) == dir)
                .unwrap();
            for dependency in package["dependencies"].as_array().unwrap() {
                // Plugins build only for wasm32-wasip1, so neither dev nor native-only edges
                // reach a plugin's compile.
                if dependency["kind"] == "dev"
                    || dependency["target"] == r#"cfg(not(target_family = "wasm"))"#
                {
                    continue;
                }
                if let Some(path) = dependency["path"].as_str() {
                    pending.push(Path::new(path).strip_prefix(&root).unwrap().to_path_buf());
                }
            }
        }
        for dir in seen
            .iter()
            .filter(|dir| !dir.starts_with("default-plugins"))
        {
            let dir = dir.to_str().unwrap();
            assert!(
                PLUGIN_INPUT_DIRS.contains(&format!("{dir}/src").as_str()),
                "PLUGIN_INPUT_DIRS is missing {dir}/src"
            );
            assert!(
                PLUGIN_INPUT_FILES.contains(&format!("{dir}/Cargo.toml").as_str()),
                "PLUGIN_INPUT_FILES is missing {dir}/Cargo.toml"
            );
            if root.join(dir).join("assets").is_dir() {
                assert!(
                    PLUGIN_INPUT_DIRS.contains(&format!("{dir}/assets").as_str()),
                    "PLUGIN_INPUT_DIRS is missing {dir}/assets"
                );
            }
        }
    }

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
        let stray_wasm = root.join("zellij-config/assets/plugins/tab-bar.wasm");
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
}
