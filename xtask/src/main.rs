//! See <https://github.com/matklad/cargo-xtask/>.
//!
//! This binary defines various auxiliary build commands, which are not expressible with just
//! `cargo`. Notably, it provides tests via `cargo test -p xtask` for code generation and `cargo
//! xtask install` for installation of rust-analyzer server and client.
//!
//! This binary is integrated into the `cargo` command line by using an alias in `.cargo/config`.

mod authoring;
mod bench;
mod build;
mod ci;
mod flags;
mod format;
mod integration_test;
mod lint;
mod metadata;
mod pipelines;
mod remote_test;
mod test;
mod test_run;
// Keep the build driver independent of the host runtime's dependency graph.
#[path = "../../yinz-runtime/src/owned_dir.rs"]
mod owned_dir;

use anyhow::Context;
use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    time::Instant,
};
use xshell::Shell;

/// The commands whose leftover arguments are forwarded verbatim to another tool, paired with the
/// flags xtask itself owns.
///
/// xflags matches every `-`-prefixed token before `--` against the command's own declaration, so
/// `cargo x test -p yinz-server` dies with ``unexpected flag: `-p` `` long before the forwarding
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
        " !!! cargo make has been deprecated by yinz !!!

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
#[path = "unit/main.rs"]
mod tests;
