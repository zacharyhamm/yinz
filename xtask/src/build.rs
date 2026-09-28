//! Subcommands for building.
//!
//! Currently has the following functions:
//!
//! - [`build`]: Builds general cargo projects (i.e. zellij components) with `cargo build`
use crate::flags;
use anyhow::Context;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use xshell::{cmd, Shell};

/// Plugin build inputs that no dep-info lists: the lockfile, the workspace manifest with its
/// profiles and patches, the toolchain pin, and this recipe with `PLUGINS` and the flags
/// `build_plugins` passes.
const PLUGIN_BUILD_FILES: &[&str] = &[
    "Cargo.lock",
    "Cargo.toml",
    "rust-toolchain.toml",
    "xtask/src/build.rs",
];

/// Build the plugins and the zellij binary.
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

    // The zellij binary, and with it every library crate, for the native target.
    if !flags.plugins_only {
        println!();
        let msg = ">> Building 'zellij'";
        crate::status(msg);
        println!("{}", msg);

        cmd!(sh, "{cargo} build --profile {profile}")
            .args(&flags.args)
            .run()
            .context("failed to build 'zellij'")?;
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

/// The crates built for wasm32-wasip1: every package under `default-plugins` with a binary, by
/// package name, which is also its wasm file stem. Library crates, `hocus-proto` and `hocus-data`
/// among them, build as dependencies of a plugin or of the zellij binary, so a new library needs
/// no entry here.
pub const PLUGINS: &[&str] = &[
    "compact-bar",
    "status-bar",
    "strider",
    "tab-bar",
    "fixture-plugin-for-tests",
    "session-manager",
    "configuration",
    "plugin-manager",
    "about",
    "multiple-select",
    "layout-manager",
    "link",
    "file-finder",
    "quit-confirmation",
    "shared-test",
    "hocus",
    "hocus-todos",
    "hocus-agents",
    "hocus-new-agent",
    "hocus-agentd",
    "hocus-store",
];

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
/// cause rebuilds while source, dependency, toolchain and build-flag changes still do.
///
/// The files come from the dep-info of the last build, so content, not mtimes, decides, and
/// `None` - no dep-info to go by - means build.
fn hash_plugin_inputs(profile: &str) -> Option<String> {
    let root = crate::project_root();
    let mut hasher = hash_plugin_files(&root, &plugin_dep_infos(profile))?;
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
/// `zellij-plugin-api`, `hocus-proto` and `hocus-data` all are. Left at opt-level 0 with DWARF
/// they are 20-100× slower under the interpreter on exactly the per-event decode path (a
/// `--profile dev` install once cost hocus 74 s of keystroke latency), and a plugin test loads
/// the same wasm.
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
            // zellij-runtime, which the plugins build, bakes it in with `option_env!`, so cargo
            // rebuilds when it changes.
            | "PREFIX"
            | "RUSTC"
            | "RUSTC_BOOTSTRAP"
            | "RUSTC_WORKSPACE_WRAPPER"
            | "RUSTC_WRAPPER"
            | "RUSTFLAGS"
    ) || key.starts_with("CARGO_BUILD_")
        || key.starts_with("CARGO_PROFILE_")
        || key.starts_with("CARGO_TARGET_WASM32_WASIP1_")
}

/// The dep-info cargo writes beside each plugin's wasm. It lists every local file the build read:
/// the sources of the plugin and of each workspace crate it builds, and whatever `include_str!`,
/// `include_bytes!` or `include!` pulled in from anywhere in the checkout. It lists no manifests
/// and no registry files, which `Cargo.lock` pins.
fn plugin_dep_infos(profile: &str) -> Vec<PathBuf> {
    PLUGINS
        .iter()
        .map(|name| plugin_path(profile, name).with_extension("d"))
        .collect()
}

/// The prerequisites of a cargo dep-info file: one Makefile rule per line, with spaces in paths
/// escaped as `\ `. `None` when there is no rule, or a line that is neither a rule nor a comment,
/// so a format this does not know rebuilds rather than hashing half the inputs.
fn dep_info_dependencies(text: &str) -> Option<Vec<PathBuf>> {
    let mut dependencies = vec![];
    let mut rules = 0;
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (_, prerequisites) = line
            .split_once(": ")
            .or_else(|| Some((line.strip_suffix(':')?, "")))?;
        rules += 1;
        let mut path = String::new();
        let mut chars = prerequisites.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' if chars.peek() == Some(&' ') => {
                    path.push(' ');
                    chars.next();
                },
                ' ' => {
                    if !path.is_empty() {
                        dependencies.push(PathBuf::from(std::mem::take(&mut path)));
                    }
                },
                c => path.push(c),
            }
        }
        if !path.is_empty() {
            dependencies.push(PathBuf::from(path));
        }
    }
    (rules > 0).then_some(dependencies)
}

/// Every file the plugin build reads, relative to the canonical checkout: what `dep_infos` list,
/// the manifest and any `build.rs` of each listed file's package, and `PLUGIN_BUILD_FILES`.
///
/// `None`, which rebuilds, when a dep-info is missing or unreadable, or lists a file that is gone
/// or lies outside the checkout. A missing one means the plugins were never built here; a file
/// outside means the wasm came from another checkout sharing this `target/`, whose sources this
/// one cannot vouch for.
fn plugin_input_files(root: &Path, dep_infos: &[PathBuf]) -> Option<BTreeSet<PathBuf>> {
    let root = root.canonicalize().ok()?;
    let mut listed = BTreeSet::new();
    for dep_info in dep_infos {
        listed.extend(dep_info_dependencies(
            &std::fs::read_to_string(dep_info).ok()?,
        )?);
    }

    let mut files: BTreeSet<PathBuf> = PLUGIN_BUILD_FILES.iter().map(PathBuf::from).collect();
    for path in listed {
        // Cargo writes absolute paths unless `build.dep-info-basedir` is set; a relative one is
        // taken against the checkout. Canonicalizing folds rustc's `src/../../..` and the macOS
        // `/tmp` alias into one identity per file.
        let file = root.join(path).canonicalize().ok()?;
        let relative = file.strip_prefix(&root).ok()?.to_path_buf();
        // Its package's manifest, and the build script cargo finds there without the manifest
        // naming one. The walk ends at the workspace manifest for a file outside any package,
        // such as `example-plugins/`.
        if let Some(package) = relative
            .ancestors()
            .skip(1)
            .find(|dir| root.join(dir).join("Cargo.toml").is_file())
        {
            files.insert(package.join("Cargo.toml"));
            let build_script = package.join("build.rs");
            if root.join(&build_script).is_file() {
                files.insert(build_script);
            }
        }
        files.insert(relative);
    }
    Some(files)
}

fn hash_plugin_files(root: &Path, dep_infos: &[PathBuf]) -> Option<Sha256> {
    let mut hasher = Sha256::new();
    for relative in plugin_input_files(root, dep_infos)? {
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(std::fs::read(root.join(relative)).ok()?);
        hasher.update([0]);
    }
    Some(hasher)
}

/// Make sure every plugin's wasm is present and fresh under
/// `target/wasm32-wasip1/<profile dir>`, building only when the inputs changed since the last
/// build of that profile (or `ZELLIJ_FORCE_PLUGINS` is set).
pub fn ensure_plugins(sh: &Shell, profile: &str) -> anyhow::Result<()> {
    let missing = PLUGINS
        .iter()
        .any(|name| !plugin_path(profile, name).is_file());
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

    build_plugins(sh, profile)
}

/// Build all plugins in a single invocation so Cargo can unify transitive dependency features
/// across all of them and compile shared crates (e.g. zellij-plugin-api) only once.
fn build_plugins(sh: &Shell, profile: &str) -> anyhow::Result<()> {
    let cargo = crate::cargo()?;
    let mut base_cmd = cmd!(
        sh,
        "{cargo} build --target wasm32-wasip1 --profile {profile}"
    );
    for name in PLUGINS {
        base_cmd = base_cmd.args(["-p", name]);
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
#[path = "unit/build.rs"]
mod tests;
