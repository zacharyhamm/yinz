// test comment line 1 - this is just harmless placeholder text for a test change
// test comment line 2 - this is just harmless placeholder text for a test change
// test comment line 3 - this is just harmless placeholder text for a test change
// test comment line 4 - this is just harmless placeholder text for a test change
// test comment line 5 - this is just harmless placeholder text for a test change
// test comment line 6 - this is just harmless placeholder text for a test change
// test comment line 7 - this is just harmless placeholder text for a test change
// test comment line 8 - this is just harmless placeholder text for a test change
// test comment line 9 - this is just harmless placeholder text for a test change
// test comment line 10 - this is just harmless placeholder text for a test change
//! See <https://github.com/matklad/cargo-xtask/>.
//!
//! This binary defines various auxiliary build commands, which are not expressible with just
//! `cargo`. Notably, it provides tests via `cargo test -p xtask` for code generation and `cargo
//! xtask install` for installation of rust-analyzer server and client.
//!
//! This binary is integrated into the `cargo` command line by using an alias in `.cargo/config`.

// test comment
mod authoring;
mod bench;
mod build;
mod ci;
mod firecracker_test;
mod flags;
mod format;
mod integration_test;
mod lint;
mod metadata;
mod pipelines;
mod postgres_test;
mod remote_test;
mod test;
mod test_run;
// Keep the build driver independent of the host runtime's dependency graph.
#[path = "../../zellij-runtime/src/owned_dir.rs"]
mod owned_dir;

use anyhow::Context;
use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Instant,
};
use xshell::Shell;

pub struct WorkspaceMember {
    crate_name: &'static str,
    build: bool,
}

fn workspace_members() -> &'static Vec<WorkspaceMember> {
    static WORKSPACE_MEMBERS: OnceLock<Vec<WorkspaceMember>> = OnceLock::new();
    WORKSPACE_MEMBERS.get_or_init(|| {
        vec![
            WorkspaceMember {
                crate_name: "default-plugins/compact-bar",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/status-bar",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/strider",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/tab-bar",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/fixture-plugin-for-tests",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/session-manager",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/configuration",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/plugin-manager",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/about",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/multiple-select",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/layout-manager",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/link",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/file-finder",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/quit-confirmation",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/shared-test",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/hocus",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/hocus-todos",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/hocus-agents",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/hocus-new-agent",
                build: true,
            },
            // Pure logic and wire types shared by the hocus plugins. A library,
            // so there is no wasm to build — but it still wants the plugin test
            // invocation, which selects on the path rather than on `build`.
            WorkspaceMember {
                crate_name: "default-plugins/hocus-proto",
                build: false,
            },
            // The store seam, schemas and project/todo/note registry that hocus-proto
            // re-exports and the plugin host links. A library, like hocus-proto.
            WorkspaceMember {
                crate_name: "default-plugins/hocus-data",
                build: false,
            },
            WorkspaceMember {
                crate_name: "default-plugins/hocus-agentd",
                build: true,
            },
            WorkspaceMember {
                crate_name: "default-plugins/hocus-store",
                build: true,
            },
            // The swamp-club Lab client. A library, like hocus-proto.
            WorkspaceMember {
                crate_name: "default-plugins/swamp-lab",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-types",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-runtime",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-plugin-location",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-downloader",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-key-codec",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-storage",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-project-config",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-config",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-deno-builtins",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-plugin-api",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-ipc",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-session",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-cli",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-tile-utils",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-tile",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-tile-markdown",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-client",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-routing",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-bus",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-server-os",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-panes",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-plugin-host",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-plugin-authoring",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-process",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-codex",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-firecracker-provider",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-swamp",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-terminal",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-screen",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-pty",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-server",
                build: false,
            },
            WorkspaceMember {
                crate_name: "zellij-test-support",
                build: false,
            },
            WorkspaceMember {
                crate_name: ".",
                build: true,
            },
        ]
    })
}

/// The commands whose leftover arguments are forwarded verbatim to another tool, paired with the
/// flags xtask itself owns.
///
/// xflags matches every `-`-prefixed token before `--` against the command's own declaration, so
/// `cargo x test -p zellij-server` dies with ``unexpected flag: `-p` `` long before the forwarding
/// code in `test::test` sees it. `insert_forwarding_separator` puts the `--` in for the caller.
const FORWARDING_COMMANDS: &[(&str, &[&str])] = &[
    ("test", &["-h", "--help"]),
    ("integration-test", &["--serial", "-h", "--help"]),
];

/// Insert the `--` that xflags needs in front of the first argument xtask does not own.
///
/// Only the commands in `FORWARDING_COMMANDS` are touched: they take no positionals of their own,
/// so the first unowned token is unambiguously the start of the forwarded run. Everything else -
/// `install <destination>`, `ci cross <triple>`, and the commands that still document a `--` - is
/// passed through untouched, as is any argv that already contains a `--`.
///
/// The cost is that a mistyped xtask flag (`--no-wbe`) is now forwarded instead of rejected here.
/// The tool downstream rejects it, so the run still fails loudly.
fn insert_forwarding_separator(args: Vec<OsString>) -> Vec<OsString> {
    let subcommand_at = args
        .iter()
        .skip(1)
        .position(|arg| !arg.to_string_lossy().starts_with('-'))
        .map(|idx| idx + 1);
    let Some(subcommand_at) = subcommand_at else {
        return args;
    };
    let Some((_, owned)) = FORWARDING_COMMANDS
        .iter()
        .find(|(name, _)| args[subcommand_at] == **name)
    else {
        return args;
    };

    let rest = &args[subcommand_at + 1..];
    if rest.iter().any(|arg| arg == "--") {
        return args;
    }
    let Some(first_unowned) = rest
        .iter()
        .position(|arg| !owned.iter().any(|flag| arg == *flag))
    else {
        return args;
    };

    let mut args = args;
    args.insert(subcommand_at + 1 + first_unowned, OsString::from("--"));
    args
}

fn main() -> anyhow::Result<()> {
    let shell = &Shell::new()?;

    // `from_vec` - unlike `from_env` - expects argv without the program name, which the separator
    // pass still needs to see so its indices line up with the tests.
    let flags = flags::Xtask::from_vec(
        insert_forwarding_separator(env::args_os().collect::<Vec<_>>())
            .into_iter()
            .skip(1)
            .collect(),
    )?;
    let now = Instant::now();

    match flags.subcommand {
        flags::XtaskCmd::Deprecated(_flags) => deprecation_notice(),
        flags::XtaskCmd::Build(flags) => build::build(shell, flags),
        flags::XtaskCmd::Format(flags) => format::format(shell, flags),
        flags::XtaskCmd::Lint(flags) => lint::lint(shell, flags),
        flags::XtaskCmd::Test(flags) => test::test(shell, flags),
        flags::XtaskCmd::IntegrationTest(flags) => integration_test::integration_test(shell, flags),
        flags::XtaskCmd::RemoteTest(flags) => remote_test::main(shell, flags),
        flags::XtaskCmd::FirecrackerTest(flags) => firecracker_test::main(shell, flags),
        flags::XtaskCmd::PostgresTest(flags) => postgres_test::main(shell, flags),
        flags::XtaskCmd::Bench(flags) => bench::bench(shell, flags),
        flags::XtaskCmd::Proto(_flags) => build::proto(shell),
        // Pipelines
        // These are composite commands, made up of multiple "stages" defined above.
        flags::XtaskCmd::Make(flags) => pipelines::make(shell, flags),
        flags::XtaskCmd::Install(flags) => pipelines::install(shell, flags),
        flags::XtaskCmd::Run(flags) => pipelines::run(shell, flags),
        flags::XtaskCmd::Ci(flags) => ci::main(shell, flags),
    }?;

    let elapsed = now.elapsed().as_secs();
    status(&format!("xtask (done after {} s)", elapsed));
    eprintln!("\n\n>> Command took {} s", elapsed);
    Ok(())
}

fn project_root() -> PathBuf {
    Path::new(
        &env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| env!("CARGO_MANIFEST_DIR").to_owned()),
    )
    .ancestors()
    .nth(1)
    .unwrap()
    .to_path_buf()
}

pub fn target_dir() -> PathBuf {
    match env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => crate::project_root().join("target"),
    }
}

pub fn cargo() -> anyhow::Result<PathBuf> {
    std::env::var_os("CARGO")
        .map_or_else(|| which::which("cargo"), |exe| Ok(PathBuf::from(exe)))
        .context("Couldn't find 'cargo' executable")
}

// Set terminal title to 'msg'
pub fn status(msg: &str) {
    eprint!("\u{1b}]0;{}\u{07}", msg);
}

fn deprecation_notice() -> anyhow::Result<()> {
    Err(anyhow::anyhow!(
        " !!! cargo make has been deprecated by zellij !!!

Our build system is now `cargo xtask`. Don't worry, you won't have to install
anything!

- To get an overview of the new build tasks, run `cargo xtask --help`
- Quick compatibility table:

| cargo make task                 | cargo xtask equivalent        |
| ------------------------------- | ----------------------------- |
| make                            | xtask                         |
| make format                     | xtask format                  |
| make build                      | xtask build                   |
| make test                       | xtask test                    |
| make run                        | xtask run                     |
| make run -l strider             | xtask run -- -l strider       |
| make install /path/to/binary    | xtask install /path/to/binary |


In order to disable xtask during the transitioning period: Delete/comment the
`[alias]` section in `.cargo/config.toml` and use `cargo make` as before.
If you're unhappy with `xtask` and decide to disable it, please tell us why so
we can discuss this before making it final for the next release. Thank you!
"
    ))
}

#[cfg(test)]
mod tests {
    use super::insert_forwarding_separator;
    use std::ffi::OsString;

    #[test]
    fn publishing_is_not_a_command() {
        let help = super::flags::Xtask::from_vec(vec![OsString::from("--help")])
            .unwrap_err()
            .to_string();
        assert!(help.contains("install"), "{help}");
        assert!(!help.contains("publish"), "{help}");
        let error = super::flags::Xtask::from_vec(vec![OsString::from("publish")])
            .unwrap_err()
            .to_string();
        assert!(error.contains("unexpected argument"), "{error}");
    }

    fn split(args: &[&str]) -> Vec<String> {
        insert_forwarding_separator(args.iter().map(OsString::from).collect())
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_package_selector_is_forwarded() {
        assert_eq!(
            split(&["xtask", "test", "-p", "zellij-server"]),
            ["xtask", "test", "--", "-p", "zellij-server"]
        );
    }

    #[test]
    fn a_bare_filter_is_forwarded() {
        assert_eq!(
            split(&["xtask", "test", "some_test_name"]),
            ["xtask", "test", "--", "some_test_name"]
        );
    }

    #[test]
    fn an_explicit_separator_is_left_alone() {
        assert_eq!(
            split(&["xtask", "test", "--", "-p", "zellij-server"]),
            ["xtask", "test", "--", "-p", "zellij-server"]
        );
    }

    #[test]
    fn a_bare_command_is_left_alone() {
        assert_eq!(split(&["xtask", "test"]), ["xtask", "test"]);
    }

    #[test]
    fn integration_test_takes_flags_directly_too() {
        assert_eq!(
            split(&["xtask", "integration-test", "--serial", "-j", "4"]),
            ["xtask", "integration-test", "--serial", "--", "-j", "4"]
        );
    }

    #[test]
    fn other_commands_and_their_positionals_are_untouched() {
        assert_eq!(
            split(&["xtask", "build", "-p", "--release"]),
            ["xtask", "build", "-p", "--release"]
        );
        assert_eq!(
            split(&["xtask", "install", "/path/to/zellij"]),
            ["xtask", "install", "/path/to/zellij"]
        );
        assert_eq!(split(&["xtask", "ci", "check"]), ["xtask", "ci", "check"]);
    }
}
