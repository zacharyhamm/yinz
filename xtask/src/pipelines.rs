//! Composite pipelines for the build system.
//!
//! Defines multiple "pipelines" that run specific individual steps in sequence.
use crate::{build, format, test};
use crate::{flags, WorkspaceMember};
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
/// - [`build`](build::build) (release, plugins only)
/// - [`build`](build::build) (release, without plugins)
/// - Copy the executable to [target file](flags::Install::destination)
/// - Copy the builtin plugin wasm to the directory the binary will look in
pub fn install(sh: &Shell, flags: flags::Install) -> anyhow::Result<()> {
    let err_context = || format!("failed to run pipeline 'install' with args {flags:?}");

    // Build and optimize plugins
    build::build(
        sh,
        flags::Build {
            release: true,
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
                release: true,
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
    let binary = crate::target_dir().join("release").join("zellij");
    // Copy to a temp file and rename, rather than overwriting in place: on macOS an in-place
    // overwrite keeps the inode, so the kernel serves its cached (now stale) code signature and
    // SIGKILLs the binary on exec. A rename gives it a fresh inode.
    let tmp = destination.with_extension("xtask-install-tmp");
    sh.copy_file(&binary, &tmp).with_context(err_context)?;
    std::fs::rename(&tmp, &destination).with_context(err_context)?;

    #[cfg(target_os = "linux")]
    {
        let provider = crate::target_dir().join("release/zellij-firecracker");
        let provider_destination = destination.with_file_name("zellij-firecracker");
        let provider_tmp = provider_destination.with_extension("xtask-install-tmp");
        sh.copy_file(&provider, &provider_tmp)
            .with_context(err_context)?;
        std::fs::rename(provider_tmp, provider_destination).with_context(err_context)?;
    }

    if flags.no_builtins {
        return Ok(());
    }
    install_builtins(sh, &binary, flags.builtins_dir.as_deref()).with_context(err_context)
}

/// Where the binary at `binary` looks for its installed builtins: `(user, system)`.
///
/// Asked of the binary rather than recomputed here, because every input is the binary's -
/// `PREFIX` is baked in at compile time, the XDG answer is platform-specific, and the
/// `builtins/<version>` shape is zellij-utils' to define. xtask cannot depend on zellij-utils
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

fn install_builtins(sh: &Shell, binary: &Path, explicit_dir: Option<&Path>) -> anyhow::Result<()> {
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
        install_one_builtin(&dir, name)?;
    }

    if prune {
        prune_builtins(&dir, &names)?;
    }
    Ok(())
}

/// Copy one plugin in atomically: write beside the destination and rename over it, so a
/// server that opens the file mid-copy sees either the old plugin or the new one, never a
/// half-written file. Identical bytes are left alone.
fn install_one_builtin(dir: &Path, name: &str) -> anyhow::Result<()> {
    let from = build::release_plugin_path(name);
    let to = dir.join(name).with_extension("wasm");
    let bytes = std::fs::read(&from)
        .with_context(|| format!("no release build of plugin '{name}' at {}", from.display()))?;
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

/// Actions for the user to choose from to resolve publishing errors/conflicts.
enum UserAction {
    Retry,
    Abort,
    Ignore,
}

const RELEASE_CONFIG: &str = "example/default.kdl";

fn git_status(sh: &Shell) -> anyhow::Result<String> {
    cmd!(sh, "git status --porcelain --untracked-files=all")
        .read()
        .context("failed to inspect the release worktree")
}

fn ensure_clean_release_worktree(sh: &Shell) -> anyhow::Result<()> {
    let status = git_status(sh)?;
    if !status.is_empty() {
        anyhow::bail!("release requires a clean worktree:\n{status}");
    }
    Ok(())
}

fn stage_release_changes(sh: &Shell) -> anyhow::Result<bool> {
    let status = git_status(sh)?;
    let unexpected = status
        .lines()
        .filter(|entry| entry.get(3..) != Some(RELEASE_CONFIG))
        .collect::<Vec<_>>();
    if !unexpected.is_empty() {
        anyhow::bail!(
            "release generation changed unexpected files:\n{}",
            unexpected.join("\n")
        );
    }
    if status.is_empty() {
        return Ok(false);
    }

    cmd!(sh, "git add -- {RELEASE_CONFIG}")
        .run()
        .context("failed to stage the generated release config")?;
    Ok(
        !cmd!(sh, "git diff --cached --name-only -- {RELEASE_CONFIG}")
            .read()
            .context("failed to inspect staged release changes")?
            .is_empty(),
    )
}

/// Make a zellij release and publish all crates.
pub fn publish(sh: &Shell, flags: flags::Publish) -> anyhow::Result<()> {
    let err_context = "failed to publish zellij";

    // Process flags
    let dry_run = if flags.dry_run {
        Some("--dry-run")
    } else {
        None
    };
    let remote = flags.git_remote.unwrap_or("origin".into());
    let registry = if let Some(ref registry) = flags.cargo_registry {
        Some(format!(
            "--registry={}",
            registry
                .clone()
                .into_string()
                .map_err(|registry| anyhow::Error::msg(format!(
                    "failed to convert '{:?}' to valid registry name",
                    registry
                )))
                .context(err_context)?
        ))
    } else {
        None
    };
    let registry = registry.as_ref();
    if flags.no_push && flags.cargo_registry.is_none() {
        anyhow::bail!("flag '--no-push' can only be used with '--cargo-registry'");
    }

    let project_dir = crate::project_root();
    sh.change_dir(&project_dir);
    ensure_clean_release_worktree(sh).context(err_context)?;
    let cargo = crate::cargo().context(err_context)?;
    let manifest = sh
        .read_file(project_dir.join("Cargo.toml"))
        .context(err_context)?
        .parse::<toml::Table>()
        .context(err_context)?;
    // Version of the core crate
    let version = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package["version"].as_str())
        .context("failed to read package version from manifest")
        .context(err_context)?;

    let mut skip_build = false;
    if cmd!(sh, "git tag -l")
        .read()
        .context(err_context)?
        .contains(version)
    {
        println!();
        println!("Git tag 'v{version}' is already present.");
        println!("If this is a mistake, delete it with: git tag -d 'v{version}'");
        println!("Skip build phase and continue to publish? [y/n]");

        let stdin = std::io::stdin();
        loop {
            let mut buffer = String::new();
            stdin.read_line(&mut buffer).context(err_context)?;
            match buffer.trim_end() {
                "y" | "Y" => {
                    skip_build = true;
                    break;
                },
                "n" | "N" => {
                    skip_build = false;
                    break;
                },
                _ => {
                    println!(" --> Unknown input '{buffer}', ignoring...");
                    println!();
                    println!("Skip build phase and continue to publish? [y/n]");
                },
            }
        }
    }

    let mut release_commit_created = false;
    if !skip_build {
        // Clean project
        cmd!(sh, "{cargo} clean").run().context(err_context)?;

        // Build plugins
        build::build(
            sh,
            flags::Build {
                release: true,
                no_plugins: false,
                plugins_only: true,
                args: vec![],
            },
        )
        .context(err_context)?;

        // Update default config
        sh.copy_file(
            project_dir
                .join("zellij-utils")
                .join("assets")
                .join("config")
                .join("default.kdl"),
            project_dir.join("example").join("default.kdl"),
        )
        .context(err_context)?;

        // Commit only the generated config. The plugin build no longer touches the tree, so
        // there may be nothing to commit beyond the default config copy above.
        if stage_release_changes(sh).context(err_context)? {
            cmd!(sh, "git commit -em")
                .arg(format!("chore(release): v{}", version))
                .run()
                .context(err_context)?;
            release_commit_created = true;
        } else {
            println!("Nothing to commit for the release; tagging HEAD");
        }

        // Tag release
        cmd!(sh, "git tag --annotate --message")
            .arg(format!("Version {}", version))
            .arg(format!("v{}", version))
            .run()
            .context(err_context)?;
    }

    let closure = || -> anyhow::Result<()> {
        // Push commit and tag
        if flags.dry_run {
            println!("Skipping push due to dry-run");
        } else if flags.no_push {
            println!("Skipping push due to no-push");
        } else {
            let branch = cmd!(sh, "git rev-parse --abbrev-ref HEAD")
                .read()
                .context(err_context)?;
            cmd!(sh, "git push --atomic {remote} {branch} v{version}")
                .run()
                .context(err_context)?;
        }

        // Publish all the crates
        for WorkspaceMember { crate_name, .. } in crate::workspace_members().iter() {
            if crate_name.contains("plugin") || crate_name.contains("xtask") {
                continue;
            }

            let _pd = sh.push_dir(project_dir.join(crate_name));
            loop {
                let msg = format!(">> Publishing '{crate_name}'");
                crate::status(&msg);
                println!("{}", msg);

                let more_args = match *crate_name {
                    "." => Some("--no-default-features"),
                    _ => None,
                };

                if let Err(err) = cmd!(
                    sh,
                    "{cargo} publish --locked {registry...} {more_args...} {dry_run...}"
                )
                .run()
                .context(err_context)
                {
                    println!();
                    println!("Publishing crate '{crate_name}' failed with error:");
                    println!("{:?}", err);
                    println!();
                    println!("Please choose what to do: [r]etry/[a]bort/[i]gnore");

                    let stdin = std::io::stdin();
                    let action;

                    loop {
                        let mut buffer = String::new();
                        stdin.read_line(&mut buffer).context(err_context)?;
                        match buffer.trim_end() {
                            "r" | "R" => {
                                action = UserAction::Retry;
                                break;
                            },
                            "a" | "A" => {
                                action = UserAction::Abort;
                                break;
                            },
                            "i" | "I" => {
                                action = UserAction::Ignore;
                                break;
                            },
                            _ => {
                                println!(" --> Unknown input '{buffer}', ignoring...");
                                println!();
                                println!("Please choose what to do: [r]etry/[a]bort/[i]gnore");
                            },
                        }
                    }

                    match action {
                        UserAction::Retry => continue,
                        UserAction::Ignore => break,
                        UserAction::Abort => {
                            eprintln!("Aborting publish for crate '{crate_name}'");
                            return Err::<(), _>(err);
                        },
                    }
                } else {
                    // publish successful, continue to next crate
                    break;
                }
            }
        }

        println!();
        println!(" +-----------------------------------------------+");
        println!(" | PRAISE THE DEVS, WE HAVE A NEW ZELLIJ RELEASE |");
        println!(" +-----------------------------------------------+");
        Ok(())
    };

    // We run this in a closure so that a failure in any of the commands doesn't abort the whole
    // program. When dry-running we need to undo the release commit first!
    let result = closure();

    if flags.dry_run && release_commit_created {
        cmd!(sh, "git reset --hard HEAD~1")
            .run()
            .context(err_context)?;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_clean_release_worktree, prune_builtins, stage_release_changes, RELEASE_CONFIG,
    };
    use std::fs;
    use tempfile::TempDir;
    use xshell::{cmd, Shell};

    fn git_repo() -> (TempDir, Shell) {
        let root = tempfile::tempdir().unwrap();
        let sh = Shell::new().unwrap();
        sh.change_dir(root.path());
        cmd!(sh, "git init --quiet").run().unwrap();
        cmd!(sh, "git config user.email release-test@example.com")
            .run()
            .unwrap();
        cmd!(sh, "git config user.name ReleaseTest").run().unwrap();
        cmd!(sh, "git config commit.gpgsign false").run().unwrap();
        fs::create_dir_all(root.path().join("example")).unwrap();
        fs::write(root.path().join(RELEASE_CONFIG), "original").unwrap();
        fs::write(root.path().join("tracked.txt"), "original").unwrap();
        cmd!(sh, "git add -- .").run().unwrap();
        cmd!(sh, "git commit --quiet -m initial").run().unwrap();
        (root, sh)
    }

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

    #[test]
    fn release_git_preflight_rejects_every_dirty_state() {
        for (state, path) in [
            ("untracked", "untracked.txt"),
            ("unstaged", "tracked.txt"),
            ("staged", "tracked.txt"),
        ] {
            let (root, sh) = git_repo();
            match state {
                "untracked" => fs::write(root.path().join("untracked.txt"), "dirty").unwrap(),
                "unstaged" => fs::write(root.path().join("tracked.txt"), "dirty").unwrap(),
                "staged" => {
                    fs::write(root.path().join("tracked.txt"), "dirty").unwrap();
                    cmd!(sh, "git add -- tracked.txt").run().unwrap();
                },
                _ => unreachable!(),
            }

            let error = ensure_clean_release_worktree(&sh).unwrap_err().to_string();
            assert!(error.contains(path), "{state}: {error}");
        }
    }

    #[test]
    fn release_git_stages_only_the_generated_config() {
        let (root, sh) = git_repo();
        fs::write(root.path().join(RELEASE_CONFIG), "generated").unwrap();

        assert!(stage_release_changes(&sh).unwrap());
        assert_eq!(
            cmd!(sh, "git diff --cached --name-only").read().unwrap(),
            RELEASE_CONFIG
        );

        cmd!(sh, "git reset --quiet").run().unwrap();
        fs::write(root.path().join("tracked.txt"), "unexpected").unwrap();
        let error = stage_release_changes(&sh).unwrap_err().to_string();
        assert!(error.contains("tracked.txt"), "{error}");
        assert!(cmd!(sh, "git diff --cached --name-only")
            .read()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn release_git_noop_does_not_claim_a_commit() {
        let (_root, sh) = git_repo();
        assert!(!stage_release_changes(&sh).unwrap());
    }
}
