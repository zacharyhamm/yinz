//! Composite pipelines for the build system.
//!
//! Defines multiple "pipelines" that run specific individual steps in sequence.
use crate::flags;
use crate::{build, format, test};
use anyhow::Context;
use std::path::{Path, PathBuf};
use xshell::{cmd, Shell};

/// Perform a default build.
///
/// Runs the following steps in sequence:
///
/// - format
/// - build
/// - test
pub fn make(sh: &Shell, flags: flags::Make) -> anyhow::Result<()> {
    let err_context = || format!("failed to run pipeline 'make' with args {flags:?}");

    if flags.clean {
        crate::cargo()
            .and_then(|cargo| cmd!(sh, "{cargo} clean").run().map_err(anyhow::Error::new))
            .with_context(err_context)?;
    }

    format::format(sh, flags::Format { check: false })
        .and_then(|_| {
            build::build(
                sh,
                flags::Build {
                    release: flags.release,
                    profile: None,
                    no_plugins: false,
                    plugins_only: false,
                    args: vec![],
                },
            )
        })
        .and_then(|_| test::test(sh, flags::Test { args: vec![] }))
        .with_context(err_context)
}

/// Generate a runnable executable and install its builtin plugins.
///
/// Runs the following steps in sequence:
///
/// - [`build`](build::build) (the chosen profile, plugins only)
/// - [`build`](build::build) (the chosen profile, without plugins)
/// - Copy the executable to [target file](flags::Install::destination)
/// - Copy the builtin plugin wasm to the directory the binary will look in
///
/// The profile is `release` unless `--profile` names another one; everything the install
/// touches - the binary, the firecracker provider, the builtin wasm - comes from that profile's
/// target directory.
pub fn install(sh: &Shell, flags: flags::Install) -> anyhow::Result<()> {
    let err_context = || format!("failed to run pipeline 'install' with args {flags:?}");

    let profile = flags.profile.as_deref().unwrap_or("release");
    let profile_dir = build::profile_dir(profile);

    // Build and optimize plugins
    build::build(
        sh,
        flags::Build {
            release: false,
            profile: Some(profile.to_string()),
            no_plugins: false,
            plugins_only: true,
            args: vec![],
        },
    )
    .and_then(|_| {
        // Build the main executable
        build::build(
            sh,
            flags::Build {
                release: false,
                profile: Some(profile.to_string()),
                no_plugins: true,
                plugins_only: false,
                args: flags.args.clone(),
            },
        )
    })
    .with_context(err_context)?;

    // Copy binary to destination
    let destination = if flags.destination.is_absolute() {
        flags.destination.clone()
    } else {
        std::env::current_dir()
            .context("Can't determine current working directory")?
            .join(&flags.destination)
    };
    // `sh.copy_file` accepts a directory destination, keep supporting that
    let destination = if destination.is_dir() {
        destination.join("zellij")
    } else {
        destination
    };
    sh.change_dir(crate::project_root());
    let binary = crate::target_dir().join(profile_dir).join("zellij");
    // Copy to a temp file and rename, rather than overwriting in place: on macOS an in-place
    // overwrite keeps the inode, so the kernel serves its cached (now stale) code signature and
    // SIGKILLs the binary on exec. A rename gives it a fresh inode.
    let tmp = destination.with_extension("xtask-install-tmp");
    sh.copy_file(&binary, &tmp).with_context(err_context)?;
    std::fs::rename(&tmp, &destination).with_context(err_context)?;

    #[cfg(target_os = "linux")]
    {
        // The provider is an implicit bin of the root crate, so the build just run produced it -
        // but cargo links the two bins in whatever order it finishes them (the smaller provider
        // usually first), so no comparison of what is on disk can say whether the provider there
        // is this build's or a leftover. Ask cargo instead: building it by name is a no-op after
        // the root build and current by construction. Only a `--bin` filter the user forwarded
        // can legitimately leave it out, and then we honour that rather than building a bin they
        // excluded.
        if provider_excluded_by_args(&flags.args) {
            println!(
                "Skipping zellij-firecracker: not part of this build \
                 (build it with 'cargo x build --profile {profile}')"
            );
        } else {
            println!();
            let msg = ">> Building 'zellij-firecracker'";
            crate::status(msg);
            println!("{}", msg);
            let cargo = crate::cargo().with_context(err_context)?;
            match cmd!(
                sh,
                "{cargo} build --profile {profile} --bin zellij-firecracker"
            )
            .args(&flags.args)
            .run()
            {
                Ok(()) => {
                    let provider = crate::target_dir()
                        .join(profile_dir)
                        .join("zellij-firecracker");
                    let provider_destination = destination.with_file_name("zellij-firecracker");
                    let provider_tmp = provider_destination.with_extension("xtask-install-tmp");
                    sh.copy_file(&provider, &provider_tmp)
                        .with_context(err_context)?;
                    std::fs::rename(provider_tmp, &provider_destination)
                        .with_context(err_context)?;
                },
                // The provider is an optional companion: a zellij that built and installed fine
                // should not be held back by it.
                Err(err) => println!("Skipping zellij-firecracker: {err}"),
            }
        }
    }

    if !flags.no_builtins {
        install_builtins(sh, &binary, profile, flags.builtins_dir.as_deref())
            .with_context(err_context)?;
    }

    Ok(())
}

/// Whether forwarded `cargo build` args restrict the build to bins other than the provider.
///
/// A `--bin` filter is the only way an `install` can legitimately not want a `zellij-firecracker`
/// beside its `zellij`: a forwarded `--target` moves the `zellij` binary too, and copying that
/// fails before this question is asked. The profile is not forwarded here - `install --profile`
/// picks it, and the provider follows it.
#[cfg(target_os = "linux")]
fn provider_excluded_by_args(args: &[std::ffi::OsString]) -> bool {
    let mut filtered = false;
    let mut names = Vec::new();
    let mut expecting_name = false;
    for arg in args {
        let arg = arg.to_string_lossy();
        if expecting_name {
            expecting_name = false;
            names.push(arg.into_owned());
        } else if arg == "--bin" {
            filtered = true;
            expecting_name = true;
        } else if let Some(name) = arg.strip_prefix("--bin=") {
            filtered = true;
            names.push(name.to_owned());
        }
    }
    filtered && !names.iter().any(|name| name == "zellij-firecracker")
}

/// Where the binary at `binary` looks for its installed builtins: `(user, system)`.
///
/// Asked of the binary rather than recomputed here, because every input is the binary's -
/// `PREFIX` is baked in at compile time, the XDG answer is platform-specific, and the
/// `builtins/<version>` shape is zellij-config' to define. xtask cannot depend on zellij-config
/// without compiling the whole tree to run anything.
fn builtin_dirs_of(sh: &Shell, binary: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let output = cmd!(sh, "{binary} setup --print-builtin-plugin-dir")
        .read()
        .context("failed to ask the built zellij where its builtin plugins go")?;
    let mut user = None;
    let mut system = None;
    for line in output.lines() {
        if let Some(path) = line.strip_prefix("user=") {
            user = Some(PathBuf::from(path));
        } else if let Some(path) = line.strip_prefix("system=") {
            system = Some(PathBuf::from(path));
        }
    }
    match (user, system) {
        (Some(user), Some(system)) => Ok((user, system)),
        _ => anyhow::bail!(
            "unexpected output from `zellij setup --print-builtin-plugin-dir`:\n{output}"
        ),
    }
}

fn install_builtins(
    sh: &Shell,
    binary: &Path,
    profile: &str,
    explicit_dir: Option<&Path>,
) -> anyhow::Result<()> {
    let (dir, prune) = match explicit_dir {
        Some(dir) => (dir.to_path_buf(), false),
        None => {
            let (user, system) = builtin_dirs_of(sh, binary)?;
            // The same variable `consts::system_default_data_dir` bakes into the binary, so an
            // install and the binary it produced agree on where "system" is.
            let system_install = std::env::var_os("PREFIX").is_some();
            (if system_install { system } else { user }, true)
        },
    };

    // The versioned dir is shared by every installed zellij of this version, so wasm built for
    // another profile replaces what they all load - and an unoptimized plugin is noticeably
    // slower. Say so before writing, while the user can still stop and use --no-builtins or
    // --builtins-dir instead.
    if profile != "release" {
        eprintln!(
            ">> Note: installing '{profile}' plugin wasm, which is unoptimized and replaces the \
             release wasm every installed zellij of this version loads. Re-run \
             'cargo x install <dest>' with no --profile to put the release wasm back, or pass \
             --no-builtins / --builtins-dir to leave {} alone.",
            dir.display()
        );
    }

    // Said before anything is written: under `sudo`, HOME is root's, and the user should see
    // that this is about to land in root's data dir rather than theirs.
    let msg = format!(">> Installing builtin plugins to {}", dir.display());
    crate::status(&msg);
    eprintln!("{}", msg);
    std::fs::create_dir_all(&dir).with_context(|| {
        format!(
            "failed to create {}. For a system-wide install set PREFIX and run as root \
             (`sudo -E`); to put the plugins elsewhere pass --builtins-dir",
            dir.display()
        )
    })?;

    let names: Vec<&str> = build::plugin_names()
        .into_iter()
        .filter(|name| !build::TEST_ONLY_PLUGINS.contains(name))
        .collect();
    for name in &names {
        install_one_builtin(&dir, profile, name)?;
    }

    if prune {
        prune_builtins(&dir, &names)?;
    }
    Ok(())
}

/// Copy one plugin in atomically: write beside the destination and rename over it, so a
/// server that opens the file mid-copy sees either the old plugin or the new one, never a
/// half-written file. Identical bytes are left alone.
fn install_one_builtin(dir: &Path, profile: &str, name: &str) -> anyhow::Result<()> {
    let from = build::plugin_path(profile, name);
    let to = dir.join(name).with_extension("wasm");
    let bytes = std::fs::read(&from).with_context(|| {
        format!(
            "no '{profile}' build of plugin '{name}' at {}",
            from.display()
        )
    })?;
    if std::fs::read(&to).ok().as_deref() == Some(bytes.as_slice()) {
        return Ok(());
    }
    let tmp = dir.join(format!(".{name}.wasm.tmp"));
    std::fs::write(&tmp, &bytes)
        .and_then(|_| std::fs::rename(&tmp, &to))
        .with_context(|| format!("failed to install plugin '{name}' to {}", to.display()))
}

/// Remove wasm in the versioned dir that no plugin crate produces any more - a plugin deleted
/// from the workspace must not keep loading - and point out sibling version dirs, which are
/// left alone because another installed zellij may still be that version.
fn prune_builtins(dir: &Path, installed: &[&str]) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("failed to read {}", dir.display()))?
        .flatten()
    {
        let path = entry.path();
        let is_wasm = path.extension().is_some_and(|ext| ext == "wasm");
        let stem = path.file_stem().and_then(|stem| stem.to_str());
        if is_wasm && stem.is_some_and(|stem| !installed.contains(&stem)) {
            eprintln!(">> Pruning {}", path.display());
            std::fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        }
    }

    let Some(versions_dir) = dir.parent() else {
        return Ok(());
    };
    let mut stale: Vec<String> = std::fs::read_dir(versions_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir() && entry.path() != dir)
        .map(|entry| entry.path().display().to_string())
        .collect();
    if !stale.is_empty() {
        stale.sort();
        eprintln!(">> Builtins of other zellij versions are still installed; delete them once no zellij of that version remains:");
        for path in stale {
            eprintln!("     {path}");
        }
    }
    Ok(())
}

/// Run zellij debug build.
pub fn run(sh: &Shell, flags: flags::Run) -> anyhow::Result<()> {
    let err_context =
        |flags: &flags::Run| format!("failed to run pipeline 'run' with args {:?}", flags);

    if flags.quick_run && flags.data_dir.is_some() {
        eprintln!("cannot use '--data-dir' and '--quick-run' at the same time!");
        std::process::exit(1);
    }

    let data_dir = if flags.quick_run {
        // Skip the stamp hash and the build: run against whatever debug plugins are already
        // under target/. Only a tree that has never built them gets a (debug, one-off) plugin
        // build - never the release build, which is the slowest thing xtask does.
        let missing: Vec<&str> = build::plugin_names()
            .into_iter()
            .filter(|name| !build::debug_plugin_path(name).is_file())
            .collect();
        if missing.is_empty() {
            let msg = ">> Quick run: using the debug plugins already under target/";
            crate::status(msg);
            eprintln!("{}", msg);
        } else {
            eprintln!(">> Quick run: no debug build yet of {}", missing.join(", "));
            build::build(
                sh,
                flags::Build {
                    release: false,
                    profile: None,
                    no_plugins: false,
                    plugins_only: true,
                    args: vec![],
                },
            )
            .with_context(|| err_context(&flags))?;
        }
        // A scratch data dir under target/, so a debug build of whatever branch this is never
        // opens the user's real plugin-data.db or session recordings. Its `plugins/` folder
        // stays empty, so the builtins resolve from the debug target dir.
        let scratch = crate::target_dir().join("dev-data");
        sh.create_dir(&scratch)
            .with_context(|| err_context(&flags))?;
        Some(scratch)
    } else if let Some(ref data_dir) = flags.data_dir {
        // The caller said where the plugins are; nothing to build.
        Some(sh.current_dir().join(data_dir))
    } else {
        build::build(
            sh,
            flags::Build {
                release: false,
                profile: None,
                no_plugins: false,
                plugins_only: true,
                args: vec![],
            },
        )
        .with_context(|| err_context(&flags))?;
        None
    };

    let cargo = crate::cargo().with_context(|| err_context(&flags))?;
    let mut cmd = cmd!(sh, "{cargo} run").args(["--package", "zellij", "--"]);
    if let Some(data_dir) = data_dir {
        cmd = cmd.args(["--data-dir", &format!("{}", data_dir.display())]);
    }
    cmd.args(&flags.args)
        .run()
        .map_err(anyhow::Error::new)
        .with_context(|| err_context(&flags))
}

#[cfg(test)]
mod tests {
    use super::prune_builtins;
    use std::fs;

    /// Only wasm nothing produces any more goes; sibling version folders are somebody else's.
    #[test]
    fn pruning_removes_only_orphaned_wasm_in_the_version_dir() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("0.45.0");
        let other = root.path().join("0.44.0");
        fs::create_dir_all(&current).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(current.join("tab-bar.wasm"), "keep").unwrap();
        fs::write(current.join("share.wasm"), "orphan").unwrap();
        fs::write(current.join("notes.txt"), "not wasm").unwrap();
        fs::write(other.join("share.wasm"), "other version").unwrap();

        prune_builtins(&current, &["tab-bar"]).unwrap();

        assert!(current.join("tab-bar.wasm").is_file());
        assert!(!current.join("share.wasm").exists());
        assert!(current.join("notes.txt").is_file());
        assert!(other.join("share.wasm").is_file());
    }

    #[cfg(target_os = "linux")]
    mod provider_args_tests {
        use std::ffi::OsString;

        fn excluded(args: &[&str]) -> bool {
            super::super::provider_excluded_by_args(
                &args.iter().map(OsString::from).collect::<Vec<_>>(),
            )
        }

        #[test]
        fn a_plain_install_builds_the_provider() {
            assert!(!excluded(&[]));
            assert!(!excluded(&["--locked", "--features", "foo"]));
        }

        #[test]
        fn a_bin_filter_without_the_provider_excludes_it() {
            assert!(excluded(&["--bin", "zellij"]));
            assert!(excluded(&["--bin=zellij"]));
        }

        #[test]
        fn a_bin_filter_naming_the_provider_does_not() {
            assert!(!excluded(&["--bin", "zellij-firecracker"]));
            assert!(!excluded(&["--bin=zellij-firecracker"]));
            assert!(!excluded(&[
                "--bin",
                "zellij",
                "--bin",
                "zellij-firecracker"
            ]));
            assert!(!excluded(&["--bin=zellij", "--bin=zellij-firecracker"]));
        }

        #[test]
        fn building_all_bins_is_not_a_filter() {
            assert!(!excluded(&["--bins"]));
        }
    }
}
