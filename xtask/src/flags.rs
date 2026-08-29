//! CLI flags for `cargo xtask`
use std::ffi::OsString;
use std::path::PathBuf;

xflags::xflags! {
    src "./src/flags.rs"

    /// Custom build commands for zellij
    cmd xtask {
        /// Deprecation warning. Compatibility to transition from `cargo make`.
        cmd deprecated {
            repeated _args: OsString
        }

        /// Tasks for the CI
        cmd ci {
            /// end-to-end tests
            cmd e2e {
                /// Build E2E binary of zellij
                optional --build
                /// Run the E2E tests
                optional --test
                /// Additional arguments for `--test`
                repeated args: OsString
            }

            /// Perform cross-compiled release builds
            cmd cross {
                /// Target-triple to compile the application for
                required triple: OsString
            }

            /// The pre-handoff gate: format --check, lint, test
            cmd check {
            }

            /// Native release build (plugins + binary, no cross-compilation)
            cmd build-release {
            }
        }

        cmd proto {}

        /// Publish zellij and all the sub-crates
        cmd publish {
            /// Perform a dry-run (don't push/publish anything)
            optional --dry-run
            /// Publish but don't push a commit to git (only works with '--cargo-registry')
            optional --no-push
            /// Push commit to custom git remote
            optional --git-remote remote: OsString
            /// Publish crates to custom registry
            optional --cargo-registry registry: OsString
        }

        /// Sequentially call: format, build, test
        cmd make {
            /// Build in release mode without debug symbols
            optional -r, --release
            /// Clean project before building
            optional -c, --clean
        }

        /// Generate a runnable `zellij` executable with plugins bundled
        cmd install {
            required destination: PathBuf
            /// Extra arguments appended to the native `cargo build` invocation
            /// (e.g. `--no-default-features`, `--features ...`, `--offline`, `--locked`, `-j N`).
            /// Not applied to the wasm plugin build.
            repeated args: OsString
        }

        /// Run debug version of zellij
        cmd run {
            /// Take plugins from asset folder, skip building plugins.
            optional --quick-run
            /// Take plugins from here, skip building plugins. Passed to zellij verbatim
            optional --data-dir path: PathBuf
            /// Arguments to pass after `cargo run --`
            repeated args: OsString
        }

        /// Run `cargo fmt` on all crates
        cmd format {
            /// Run `cargo fmt` in check mode
            optional --check
        }

        /// Run `cargo clippy` on all crates, denying warnings
        cmd lint {
            /// Apply clippy's automatic suggestions instead of only reporting them
            optional --fix
            /// Arguments to pass after `cargo clippy --`
            repeated args: OsString
        }

        /// Run application tests
        cmd test {
            /// Arguments to pass after `cargo test --`
            repeated args: OsString
        }

        /// Run the in-process whole-app integration tests
        cmd integration-test {
            /// Run the tests one at a time instead of in parallel
            optional --serial
            /// Arguments to pass to the test runner
            repeated args: OsString
        }

        /// Run the remote-pane tests, against a real sshd in a container
        cmd remote-test {
            /// Build the musl binary and bring the containers up
            optional --build
            /// Run the tests
            optional --test
            /// Take the containers down and remove the generated keypair
            optional --clean
            /// Additional arguments for `--test`
            repeated args: OsString
        }

        /// Run the render throughput benchmarks
        cmd bench {
            /// Arguments to pass to criterion (e.g. `--save-baseline main`, `--sample-size 20`,
            /// or a bare filter like `frame/plain_text`)
            repeated args: OsString
        }

        /// Build the application and all plugins
        cmd build {
            /// Build in release mode without debug symbols
            optional -r, --release
            /// Build only the plugins
            optional -p, --plugins-only
            /// Build everything except the plugins
            optional --no-plugins
            /// Extra arguments appended to the native `cargo build` invocation
            /// (e.g. `--no-default-features`, `--features ...`, `--offline`, `--locked`, `-j N`).
            /// Not applied to the wasm plugin build.
            repeated args: OsString
        }
    }
}
// generated start
// The following code is generated by `xflags` macro.
// Run `env UPDATE_XFLAGS=1 cargo build` to regenerate.
#[derive(Debug)]
pub struct Xtask {
    pub subcommand: XtaskCmd,
}

#[derive(Debug)]
pub enum XtaskCmd {
    Deprecated(Deprecated),
    Ci(Ci),
    Proto(Proto),
    Publish(Publish),
    Make(Make),
    Install(Install),
    Run(Run),
    Format(Format),
    Lint(Lint),
    Test(Test),
    IntegrationTest(IntegrationTest),
    RemoteTest(RemoteTest),
    Bench(Bench),
    Build(Build),
}

#[derive(Debug)]
pub struct Deprecated {
    pub _args: Vec<OsString>,
}

#[derive(Debug)]
pub struct Ci {
    pub subcommand: CiCmd,
}

#[derive(Debug)]
pub enum CiCmd {
    E2e(E2e),
    Cross(Cross),
    Check(Check),
    BuildRelease(BuildRelease),
}

#[derive(Debug)]
pub struct E2e {
    pub args: Vec<OsString>,

    pub build: bool,
    pub test: bool,
}

#[derive(Debug)]
pub struct Cross {
    pub triple: OsString,
}

#[derive(Debug)]
pub struct Check;

#[derive(Debug)]
pub struct BuildRelease;

#[derive(Debug)]
pub struct Proto;

#[derive(Debug)]
pub struct Publish {
    pub dry_run: bool,
    pub no_push: bool,
    pub git_remote: Option<OsString>,
    pub cargo_registry: Option<OsString>,
}

#[derive(Debug)]
pub struct Make {
    pub release: bool,
    pub clean: bool,
}

#[derive(Debug)]
pub struct Install {
    pub destination: PathBuf,
    pub args: Vec<OsString>,
}

#[derive(Debug)]
pub struct Run {
    pub args: Vec<OsString>,

    pub quick_run: bool,
    pub data_dir: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Format {
    pub check: bool,
}

#[derive(Debug)]
pub struct Lint {
    pub args: Vec<OsString>,

    pub fix: bool,
}

#[derive(Debug)]
pub struct Test {
    pub args: Vec<OsString>,
}

#[derive(Debug)]
pub struct IntegrationTest {
    pub args: Vec<OsString>,

    pub serial: bool,
}

#[derive(Debug)]
pub struct RemoteTest {
    pub args: Vec<OsString>,

    pub build: bool,
    pub test: bool,
    pub clean: bool,
}

#[derive(Debug)]
pub struct Bench {
    pub args: Vec<OsString>,
}

#[derive(Debug)]
pub struct Build {
    pub args: Vec<OsString>,

    pub release: bool,
    pub plugins_only: bool,
    pub no_plugins: bool,
}

impl Xtask {
    #[allow(dead_code)]
    pub fn from_env_or_exit() -> Self {
        Self::from_env_or_exit_()
    }

    #[allow(dead_code)]
    pub fn from_env() -> xflags::Result<Self> {
        Self::from_env_()
    }

    #[allow(dead_code)]
    pub fn from_vec(args: Vec<std::ffi::OsString>) -> xflags::Result<Self> {
        Self::from_vec_(args)
    }
}
// generated end
