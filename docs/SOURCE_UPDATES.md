# Update a session from local main

The Sessions palette offers **Update Yinz from local main…** when the project
registry contains a project named `yinz` whose Git origin identifies
`github.com/zacharyhamm/yinz`. HTTPS and SSH origin spellings are accepted.
The ordinary **Update Yinz…** action continues to select the latest stable release.

Source updates use the committed local `main` on the **server host**. That branch
must have a clean checkout, which may be a separate Git worktree. The updater
rechecks the registry and origin, pins the commit, and extracts an isolated
snapshot. It never fetches, pulls, changes branches, or includes working changes.
Both executable and builtin plugins come from that snapshot.

Every participant must run Linux x86_64 or aarch64. Attached clients must advertise
source-update support; upgrade older clients first. The pinned source must also
contain source-update support. Unsupported participants or staged executables
are refused before installation publication.

Install these prerequisites on the server before invoking the action:

- The stable Rust toolchain, Cargo, and `rustup` targets for each participating CPU.
- The usual Yinz native build tools, including a host C/C++ compiler, Make, CMake,
  and pkg-config, for dependency build scripts.
- `cargo-zigbuild`, Zig, `protoc`, Python 3.12 or newer, `file`, and `readelf`.
- Git and sufficient disk space for an isolated source snapshot, builds, archives,
  and the existing updater's installation backups.

For example, a mixed x86_64/aarch64 session needs both
`x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` Rust targets.
The updater reports missing prerequisites; it does not install them.
`cargo x source-build <target>` is also available to build and package the current
checkout directly. It writes `dist/yinz-<target>.tar.gz` and never installs it.
The palette clears native linker flags before compiling the build driver. A direct
`cargo x` invocation still follows the checkout’s Cargo linker configuration; on
a host without mold, use `CARGO_ENCODED_RUSTFLAGS='' cargo x source-build <target>`.

Compilation appears in a server-host command pane. Successful builds close that
pane before migration. Failed builds retain it and its log. **Cancel Yinz update**
can stop compilation and its children before transfer; manually closing the build
pane also stops the operation. No client compiles the source or downloads its
artifacts from GitHub.

The coordinator builds each required target once and transfers bounded chunks
through the existing authenticated update connections. Receivers validate offsets,
length, archive and executable SHA-256 digests, architecture, and protocol
capabilities. Equal package versions still perform source replacement. Readiness
checks compare the running executable's digest with the pinned artifact.

Installation, terminal adoption, client re-exec, rollback, and explicit client
recovery use the ordinary in-place updater. Source updates retain terminal process
identities and per-client state. An unsupported newly attached client or target
stops preparation before publication. Other structural changes are checked again
at quiescence by the existing coordinator.

Use **Yinz update status** for operation progress. Journals and build records live
under `<data-dir>/updates/<operation>/`; the source build log is
`source/build.log`. Retrying an interrupted operation retains its original mode,
commit, and artifacts. An uncertain pane launch is not automatically repeated.
Keep journals and artifacts while recovery is pending. After successful commitment,
the server removes its operation-owned source checkout and staged archives while
retaining the build record and log.

Source updates currently support Linux only. Cross-compilation and artifact-header
validation do not establish execution on a different CPU; physical aarch64 and
mixed-architecture validation must be reported separately from local x86_64 tests.
