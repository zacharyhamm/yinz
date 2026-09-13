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

            /// The pre-handoff gate: format --check, lint, test, integration-test (16 workers)
            cmd check {
            }

            /// Native release build (plugins + binary, no cross-compilation)
            cmd build-release {
            }
        }

        cmd proto {}

        /// Sequentially call: format, build, test
        cmd make {
            /// Build in release mode without debug symbols
            optional -r, --release
            /// Clean project before building
            optional -c, --clean
        }

        /// Generate a runnable `zellij` executable and install its builtin plugins
        cmd install {
            required destination: PathBuf
            /// Cargo profile to build with (e.g. `dev` for a fast, unoptimized install).
            /// Defaults to `release`.
            optional --profile name: String
            /// Where to put the builtin plugin wasm. Defaults to what the built binary
            /// reports: `<data dir>/builtins/<version>`, or `$PREFIX/share/zellij/builtins/<version>`
            /// when PREFIX is set. Nothing is pruned under an explicit directory.
            optional --builtins-dir path: PathBuf
            /// Install the binary and authoring tools; skip Wasm builtins
            optional --no-builtins
            /// Extra arguments appended to the native `cargo build` invocation
            /// (e.g. `--no-default-features`, `--features ...`, `--offline`, `--locked`, `-j N`).
            /// Not applied to the wasm plugin build.
            repeated args: OsString
        }

        /// Run debug version of zellij
        cmd run {
            /// Skip the plugin build: run against the debug plugins already under target/,
            /// with a scratch data dir (target/dev-data) instead of the real one.
            optional --quick-run
            /// Take plugins from here, skip building plugins. Passed to zellij verbatim
            optional --data-dir path: PathBuf
            /// Arguments to pass after `cargo run --`
            repeated args: OsString
        }

        /// Format Rust crates and shipped TypeScript plugins
        cmd format {
            /// Run `cargo fmt` in check mode
            optional --check
        }

        /// Check Rust crates and shipped TypeScript plugins, denying warnings
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

        /// Build and run the ignored tests against a real Firecracker VM
        cmd firecracker-test {
            /// Build the guest image and provider
            optional --build
            /// Run the tests with a temporary sudo-managed host network
            optional --test
            /// Remove generated images and test state
            optional --clean
            /// Additional arguments for `--test`
            repeated args: OsString
        }

        /// Run the plugin-data store suite against a real Postgres in a container
        cmd postgres-test {
            /// Bring the container up
            optional --build
            /// Run the tests (after `--`: extra arguments for the test runner)
            optional --test
            /// Take the container down
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
            /// Cargo profile to build with (e.g. `dev`). Mutually exclusive with `--release`
            optional --profile name: String
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
    Make(Make),
    Install(Install),
    Run(Run),
    Format(Format),
    Lint(Lint),
    Test(Test),
    IntegrationTest(IntegrationTest),
    RemoteTest(RemoteTest),
    FirecrackerTest(FirecrackerTest),
    PostgresTest(PostgresTest),
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
pub struct Make {
    pub release: bool,
    pub clean: bool,
}

#[derive(Debug)]
pub struct Install {
    pub destination: PathBuf,
    pub args: Vec<OsString>,

    pub profile: Option<String>,
    pub builtins_dir: Option<PathBuf>,
    pub no_builtins: bool,
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
pub struct FirecrackerTest {
    pub args: Vec<OsString>,

    pub build: bool,
    pub test: bool,
    pub clean: bool,
}

#[derive(Debug)]
pub struct PostgresTest {
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
    pub profile: Option<String>,
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
