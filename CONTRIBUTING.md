# Contributing to Zellij

Thanks for considering contributing to Zellij!

**First**: if you're unsure of anything, feel free to ask on our [Discord
server](https://discord.gg/MHV3n76PDq), or on [Matrix](https://matrix.to/#/#zellij_general:matrix.org). We're a friendly and welcoming bunch!

# Code of Conduct

Before contributing please read our [Code of Conduct](CODE_OF_CONDUCT.md) which
all contributors are expected to adhere to.

## Status of Code Contributions (PRs)
At the moment, the Zellij maintainers are very much overloaded implementing our [Roadmap](https://zellij.dev/roadmap) - and so while we very much welcome and appreciate the community's willingness to contribute - we are only able to accept code contributions for larger projects as they appear in said Roadmap.

For those willing to take up such large projects, please check with the maintainers first (eg. by asking on the general chat in one of our chat platforms) to make sure there is both willingness and availability on our sides.

If you're still eager to contribute minor fixes, please note that we might take a long while to get to them.

## Building

To build Zellij, we're using cargo xtask. This is a standalone package shipped
inside the repository, so you don't have to install additional dependencies.

To build zellij, you'll need [`protoc`](https://github.com/protocolbuffers/protobuf#protobuf-compiler-installation) installed. This is used to compile the .proto files into Rust assets. These protocol buffers are used for communication between Zellij and its plugins across the wasm boundary.

Here are some of the commands currently supported by the build system:

```sh
# Format code, build, then run tests
cargo xtask
# You can also perform these actions individually
cargo xtask format
cargo xtask build
cargo xtask test
# Run Zellij (optionally with additional arguments)
cargo xtask run
cargo xtask run -- -l strider
# Install Zellij to some directory
cargo xtask install /path/of/zellij/binary
# Publish the zellij and zellij-tile crates
cargo xtask publish
```

You can see a list of all commands (with supported arguments) with `cargo xtask
--help`. For convenience, `xtask` may be shortened to `x`: `cargo x build` etc.

To run `test`, you will need the package `pkg-config` and a version of `openssl`.

### How builtin plugins are found

Builtin plugins (`status-bar`, `tab-bar`, ...) are `.wasm` files loaded from disk on
demand - nothing is embedded in the binary. At runtime they are looked up in this order:

1. the `builtin_plugins_dir` config option, if set
2. the configured plugin directory (`<data dir>/plugins`)
3. `$PREFIX/share/zellij/plugins`
4. `<repository>/target/wasm32-wasip1/debug` (debug builds only)
5. `<repository>/zellij-utils/assets/plugins`

`zellij setup --check` prints the first three. The last two are baked in at compile time
from `CARGO_MANIFEST_DIR`, so a build run out of a checkout - `cargo run`, `cargo test`,
`cargo x run` - finds the plugins with no configuration at all. They do not exist on an
installed system and are simply skipped there.

### The cargo target directory

`.cargo/config.toml` pins `build.target-dir` to `target` so that lookup step 4 above
resolves for debug builds. Pointing cargo elsewhere (a global `target-dir` in
`~/.cargo/config.toml`, or the `CARGO_TARGET_DIR` environment variable) is no longer fatal
- the lookup falls through to the committed assets in step 5 - but you will silently get
the last plugins built by `cargo x build --release` instead of your debug ones. To share
compilation artifacts between projects, symlink the folder instead:

```sh
ln -s /path/to/shared/target /path/to/zellij/target
```

## Packaging Zellij for a distribution

Pre-built plugin copies live in `zellij-utils/assets/plugins` so that `cargo install
zellij` works without further tooling. Distributions that forbid pre-built binaries in
source packages can strip that folder and build the plugins from source instead:

```sh
# 1. build the plugins from source (repeat for every plugin, or use `cargo x build --release`)
cargo build --release --target wasm32-wasip1 \
  -p status-bar -p tab-bar -p compact-bar -p strider -p session-manager \
  -p configuration -p plugin-manager -p about -p share -p multiple-select \
  -p layout-manager -p link -p mobile

# 2. install the resulting artifacts from <target-dir>/wasm32-wasip1/release/*.wasm
#    into $PREFIX/share/zellij/plugins/

# 3. build zellij
PREFIX=/usr cargo build --release --bin zellij
```

No feature flags are involved: the binary never carries plugins, so the plugins must be
installed into one of the lookup locations above.

`protoc` is required to regenerate the protobuf definitions in `zellij-utils/assets/prost*`
via `cargo x proto`. The `web_server_capability` and `vendored_curl` features are optional
and may be dropped with `--no-default-features`.

## Running the end-to-end tests
Zellij includes some end-to-end tests which test the whole application as a black-box from the outside.
These tests work by running a docker container which contains the Zellij binary, connecting to it via ssh, sending some commands and comparing the output received against predefined snapshots.

<details>
<summary>macOS prerequisites (expand here):</summary>

The build uses `cargo-zigbuild` to cross-compile a native musl binary (arm64 on Apple Silicon, amd64 on Intel) so the Docker container runs without emulation on any Mac.

1. `cargo install cargo-zigbuild`
2. `brew install zig`
</details>

<details>
<summary>Linux prerequisites (expand here):</summary>

The build cross-compiles for `x86_64-unknown-linux-musl`, which needs the musl linker
(this is what CI installs too). Without it `cargo xtask ci e2e --build` fails with
`failed to find tool "x86_64-linux-musl-gcc"`.

1. `sudo apt-get install -y --no-install-recommends musl-tools`
2. `rustup target add x86_64-unknown-linux-musl`

Native (non-wasm, non-musl) Linux builds link with [`mold`](https://github.com/rui314/mold)
(configured in `.cargo/config.toml`), so it must be installed too:

3. `sudo apt-get install -y mold`
</details>


To run these tests locally, you'll need to have either `docker` or `podman` and also `docker compose` installed.
Once you do, in the repository root:

1. `docker compose up -d` will start up the docker container
2. `cargo xtask ci e2e --build` will build the Zellij binary in the target folder, which is shared with the container
3. `cargo xtask ci e2e --test` will run the tests

To re-run the tests after you've changed something in the code base, be sure to repeat steps 2 and 3.

## Running the remote-pane tests

A pane opened with `--remote-host` runs its process on another machine, over ssh. The
parts of that which need another machine — bootstrapping a broker there, forwarding a
unix socket to it, surviving a dropped link — can only be tested against a real one, so
they live in their own suite rather than with the unit tests.

`docker-compose.remote-panes.yml` provides that machine: an sshd with
`AllowStreamLocalForwarding` on and the musl build of your working tree on its `PATH`,
reached through a proxy the tests can switch off mid-pane to simulate an outage. It needs
`docker` and `docker compose`, and the same musl prerequisites as the e2e suite above.

From the repository root:

1. `cargo x remote-test --build` — builds the musl binary, generates a throwaway ssh
   keypair and config, and brings the containers up
2. `cargo x remote-test --test` — runs the tests
3. `cargo x remote-test --clean` — takes the containers down and removes the generated
   credentials

Repeat step 1 after changing anything the far machine runs; it is a no-op for the
container when nothing has changed, but the binary it mounts is rebuilt.

The tests are `#[ignore]`d, so `cargo x test` and `cargo x integration-test` never run
them and never need docker.

Two things about the setup are worth knowing before you change it:

- **The ssh config reaches the code under test through a `PATH` shim.** Zellij runs a bare
  `ssh <host>`, and OpenSSH resolves `~/.ssh/config` through `getpwuid()` rather than
  `$HOME` — so overriding `HOME` does nothing, and a test may not write to your real
  config. `--build` writes an `ssh` that execs the real one with `-F <test config>` and
  the suite puts it first on `PATH`. Nothing else about the invocation is touched.
- **The tests share one far machine and set process-wide environment**, so they must run
  single-threaded. `cargo x remote-test --test` passes `--test-threads 1`.

## Debugging / Troubleshooting while developing
Zellij uses the excellent [`log`](https://crates.io/crates/log) crate to handle its internal logging. The output of these logs will go to `/$temp_dir/zellij-<UID>/zellij-log/zellij.log` which `$temp_dir` refers to [std::env::temp_dir()](https://doc.rust-lang.org/std/env/fn.temp_dir.html). On most of operating systems it points to `/tmp`, but there are exceptions, such as `/var/folders/dr/xxxxxxxxxxxxxx/T/` for Mac.

Example:
```rust
let my_variable = some_function();
log::info!("my variable is: {:?}", my_variable);
```

Note that the output is truncated at 100KB. This can be adjusted for the purposes of debugging through the `LOG_MAX_BYTES` constant, at the time of writing here: https://github.com/zellij-org/zellij/blob/main/zellij-utils/src/logging.rs#L24

When running Zellij with the `--debug` flag, Zellij will dump a copy of all bytes received over the pty for each pane in: `/$temp_dir/zellij-<UID>/zellij-log/zellij-<pane_id>.log`. These might be useful when troubleshooting terminal issues.

## Testing plugins
Zellij allows the use of the singlepass [Winch](https://crates.io/crates/wasmtime-winch) compiler for wasmtime. This can enable great gains in compilation time of plugins at the cost of slower execution and less supported architectures.

To enable the singlepass compiler, use the `singlepass` flag. E.g.:
```sh
cargo xtask run --singlepass
```

## Toolchain Versions and MSRV

Development aims to track the current stable Rust toolchain version, although
with a slight delay. The reason behind this is that users running `cargo
install --locked zellij` will use whatever toolchain version they have
installed locally and we cannot influence this (except for terminating
compilation on a "mismatch" from our expectation). By using current toolchain
versions we hope to ensure that bugs are caught before users experience them.
It hopefully also ensures that (at least for a certain time after a release has
been made) the binary obtained by installation from source doesn't deviate
(much at least) from the pre-built binaries attached as release assets. The
delay in toolchain updates is due to a certain amount of manual testing that is
performed afterward.

At this point in time, there is no MSRV policy. As our resources are limited,
we try to focus on making the code work with whatever development toolchain is
currently mentioned in `rust-toolchain.toml`. While it may still be possible to
compile Zellij with older Rust versions, we cannot offer support in such
situations.

For questions and suggestions regarding the currently used Rust toolchain
version, please mention @har7an in your issue or pull request.


## Looking for something to work on?

If you are new contributor to `Zellij` going through
[beginners][good-first-issue] should be a good start or you can join our public
[Discord server][discord-invite-link], we would be happy to help finding
something interesting to work on and guide through.

[discord-invite-link]: https://discord.gg/feHDHahHCz
[good-first-issue]: https://github.com/zellij-org/zellij/labels/good%20first%20issue


## Tips for Code Contributions

### Prefer returning `Result`s instead of `unwrap`ing

- Add `use zellij_utils::errors::prelude::*;` to the file
- Make the function return `Result<T>`, with an appropriate `T` (Use `()` if there's nothing to return)
- Append `.context()` to any `Result` you get with a sensible error description (see [the docs][error-docs-context])
- Generate ad-hoc errors with `anyhow!(<SOME MESSAGE>)`
- *Further reading*: [See here][error-docs-result]

### Logging errors

- When there's a `Result` type around, use `.non_fatal()` on that instead of `log::error!`
- When there's a `Err` type around, use `Err::<(), _>(err).non_fatal()`
- Also attach context before logging!
- *Further reading*: [See here][error-docs-logging]

### Adding Concrete Errors, Handling Specific Errors

- Add a new variant to `zellij_utils::errors::ZellijError`, if needed
- Use `anyhow::Error::downcast_ref::<ZellijError>()` to recover underlying errors
- *Further reading*: [See here][error-docs-zellijerror]

[error-docs-context]: https://github.com/zellij-org/zellij/blob/main/docs/ERROR_HANDLING.md#attaching-context
[error-docs-result]: https://github.com/zellij-org/zellij/blob/main/docs/ERROR_HANDLING.md#converting-a-function-to-return-a-result-type
[error-docs-logging]: https://github.com/zellij-org/zellij/blob/main/docs/ERROR_HANDLING.md#logging-errors
[error-docs-zellijerror]: https://github.com/zellij-org/zellij/blob/main/docs/ERROR_HANDLING.md#adding-concrete-errors-handling-specific-errors


## Filing Issues

Bugs and enhancement suggestions are tracked as GitHub issues.

### Lacking API for plugin in Zellij?

If you have a plugin idea, but Zellij still doesn't have API required to make
the plugin consider opening [an issue][plugin-issue] and describing your
requirements.

[plugin-issue]:
https://github.com/zellij-org/zellij/issues/new?assignees=&labels=plugin%20system

### How Do I Submit A (Good) Bug Report?

After you've determined which repository your bug is related to and that the
issue is still present in the latest version of the master branch, create an
issue on that repository and provide the following information:

- Use a **clear and descriptive title** for the issue to identify the problem.
- Explain which **behavior you expected** to see instead and why.
- Describe the exact **steps to reproduce the problem** in as many details as
  necessary.
- When providing code samples, please use [code blocks][code-blocks].

### How Do I Submit A (Good) Enhancement Suggestion?

Instructions are similar to those for bug reports. Please provide the following
information:

- Use a **clear and descriptive title** for the issue to identify the
  suggestion.
- Provide a **description of the suggested enhancement** in as many details as
  necessary.
- When providing code samples, please use [code blocks][code-blocks].

[code-blocks]:
https://help.github.com/articles/creating-and-highlighting-code-blocks/

## Submitting Pull Requests

Instructions are similar to those for bug reports. Please provide the following
information:

- If this is not a trivial fix, consider **creating an issue to discuss first**
  and **later link to it from the PR**.
- Use a **clear and descriptive title** for the pull request.
    - Follow [Conventional Commit
      specification](https://www.conventionalcommits.org/en/v1.0.0/) where
      sufficiently large or impactful change is made.
- Provide a **description of the changes** in as many details as necessary.

Before submitting your pull request, also make sure that the following
conditions are met:

- Your new code **adheres to the code style** through running `cargo fmt`.
- Your new code **passes all existing and new tests** through running `cargo
  test`.
