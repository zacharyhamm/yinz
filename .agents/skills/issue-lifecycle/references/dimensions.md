# Yinz review dimensions

Read these dimensions for plan and code review alongside the
[planning standards](planning.md) and [review standards](review.md). The names
below are stable descriptive categories. Apply only dimensions relevant to the
change; an inapplicable dimension creates no finding or additional testing
obligation. Current [repository instructions](../../../../AGENTS.md) and
applicable local instructions govern conflicts, including the root focused-test
policy when a local guide suggests full suites.

For plan review, inspect the proposed behavior, affected callers and
verification coverage against source. For code review, inspect the committed
implementation, affected callers and actual verification evidence. A dimension
is a review lens, not evidence of a defect. Every actionable concern needs
concrete source evidence.

Findings may cite a dimension name in `description` and its authoritative source
in `evidence`, together with the affected code or plan locations and the failure
they establish. For example, a `test-selection` concern should explain which
affected caller the selected checks miss. Use the existing finding fields; do
not add a `category` property. A label does not replace the affected
`requirement`, failure `mechanism`, source `evidence` or severity justification.
Preserve finding IDs, disposition and reopening rules from the review standards.

## build-isolation

Applies when building, testing, installing or changing build tooling in a
worktree. Each worktree needs its own real `target/`. Never set
`CARGO_TARGET_DIR` or change `build.target-dir`; do not share targets through
symlinks, hardlink mutable build artifacts, or copy workspace binaries or build
metadata between worktrees. Build the worktree's own binary before testing it.

- **Plan review:** Do build and test commands preserve isolation and build any
  required binary from this worktree?
- **Code review:** Do tooling and verification evidence use this worktree's own
  artifacts, without target-directory overrides or shared mutable outputs?

Source: [Build and test](../../../../AGENTS.md#build-and-test).

## test-selection

Applies when choosing checks for changed behavior. Select tests by changed crate
and affected callers, including downstream behavior for shared libraries, plugin
APIs, rendering, dependencies, tooling and changes across crates. In-process
whole-app tests use a fake OS backend: `yinz-server-os/` changes need relevant
tests from that crate, the root package and the integration suite's sessions
that spawn the real binary. Remote-spawning changes also need relevant remote
tests; run the remote suite's `--build` before `--test`.

Outside a merge, full suites require an explicit user request. A branch handoff,
PR or cross-crate change does not authorize them. The later merge gate requires
rebasing onto current local `main` and `cargo x ci check` unless the user
explicitly waives that check for the merge and the request is recorded. Use
`cargo x`, put xtask flags first, and select nextest tests with its supported
filters and flags. Use 16 integration workers, or 4 on smaller or busy machines;
check for competing runs first. Selected socket-binding tests need sandbox
escalation on the first attempt.

- **Plan review:** Do focused checks exercise the changed behavior and affected
  callers through the appropriate real or fake backend? Are commands executable
  as written and within the authorized development scope?
- **Code review:** Do the selected checks and recorded results cover the
  delivered change, including relevant root, integration and remote behavior?
  Investigate failures rather than treating a passing rerun as a fix.

Sources: [Build and test](../../../../AGENTS.md#build-and-test),
[Arguments and test selection](../../../../AGENTS.md#arguments-and-test-selection)
and [Failures and test timing](../../../../AGENTS.md#failures-and-test-timing).

## test-synchronization

Applies to asynchronous tests, rendering and executable stubs. Use
`yinz-test-support`'s `Watched<T>::wait_until(what, timeout, predicate)` instead
of sleeps. Wait for render content, not `snapshots.last()` or a render count;
see
[`wait_for_settled_render`](../../../../yinz-screen/src/unit/screen_tests/mod.rs).
A render count is valid only for a controlled batch with a fixed count and a
comment explaining why scheduling cannot change it. Use
`yinz_test_support::write_warm_script` for executable stubs: first execution can
take seconds on macOS. Allow ample time unless timing is the behavior under
test. Do not convert existing sleeps in bulk.

- **Plan review:** Are waits tied to observable state or content, with warm
  stubs and appropriate timeouts? Does any count-based assertion satisfy the
  fixed-batch exception?
- **Code review:** Can scheduling or first-execution delays make the tests pass
  too early or time out? Record, investigate and fix every flake or timeout in
  the current branch and worktree; a passing diagnostic rerun does not resolve
  it.

Source:
[Failures and test timing](../../../../AGENTS.md#failures-and-test-timing).

## platform-portability

Applies to child supervision, Unix sockets, filesystem paths and
platform-sensitive tests. On macOS, poll `waitpid` with `WNOHANG` for continued
children; blocking waits and `SIGCHLD` do not report them. A full Unix socket
backlog returns `ECONNREFUSED` on BSD and `EAGAIN` on Linux. Retry for
`IPC_CONNECT_TIMEOUT` before treating a BSD socket as stale. Canonicalize paths
before comparing them because macOS aliases `/tmp` and `/var`. Use
`/usr/bin/true` on macOS and Linux.

- **Plan review:** Does the proposed mechanism handle these platform
  differences, with focused checks for the affected behavior?
- **Code review:** Do waiters, retry deadlines, path comparisons and test
  commands preserve the documented behavior on both macOS and Linux?

Source: [Platform constraints](../../../../AGENTS.md#platform-constraints).

## plugin-data-ownership

Applies to shared plugin-data queries, writes and migrations. Servers sharing a
data directory share one SQLite database. Scope server rows by both `host_id`
and `yinz_session_name` using `agent_proto::agents::rows::Owner`; carry
`PluginIds.host_id` from startup. Do not call `get_plugin_ids()` in library code
because it reads real stdin in native tests.

An empty host marks legacy data. Preserve legacy runs and import markers when
adding host-specific claims (`store::import_recorded`); delete legacy latches
and summaries, and leave legacy jobs alone. Use `store::host_or_legacy` only for
readers that accept legacy rows (`rows::owned_or_unclaimed`, or
`run_history::Filter::pred` with a session). `rows::owned_by` and jobs require
strict host equality.

- **Plan review:** Are ownership and legacy-data handling explicit for each
  affected reader and writer, including multiple hosts and sessions?
- **Code review:** Can a query, claim or cleanup cross a host/session boundary
  or incorrectly consume or remove legacy data?

Source: [Shared plugin data](../../../../AGENTS.md#shared-plugin-data).

## plugin-data-revisions

Applies to plugin-data read-modify-write `Put`/`Update` operations. Set
`expect_rev` to the revision the value's author read, carry `rev` through
selected columns and edits, and do not re-fetch the revision just before
writing. Conflicts return `DataError::Conflict` and roll back the batch. Setting
a flag alone does not need a guard. Watches can observe other local servers
after the 250 ms database poll plus query and delivery time, and updates may
coalesce to the latest result.

- **Plan review:** Does the edit flow retain the author's read revision and
  handle conflicts, with a justified flag-only exception where applicable?
- **Code review:** Is every affected read-modify-write guarded by that original
  revision? Do watch checks allow delivery time and coalescing instead of
  assuming one immediate event per write?

Source: [Shared plugin data](../../../../AGENTS.md#shared-plugin-data).

## generated-artifacts

Applies to protobuf and snapshot changes. After changing protobufs under
`yinz-plugin-api/assets/` or `yinz-ipc/assets/`, regenerate with
`cargo x proto`. Review `.snap` diffs and commit updates for intentional
behavior changes.

- **Plan review:** Does the plan include required generation and snapshot review
  for the proposed behavior?
- **Code review:** Are generated files current, and do reviewed snapshot changes
  express the intended behavior rather than hide a regression?

Source: [Conventions](../../../../AGENTS.md#conventions).

## lint-expectations

Applies to Rust lint fixes, suppressions and lint tooling. Fix warnings or use
`#[expect(lint, reason = "…")]` on the affected item. `cargo x lint` rejects
file-level `allow`/`expect`, including through `cfg_attr`, unless
`FILE_LEVEL_ALLOWS` in `xtask/src/lint.rs` records a reason. Workspace-wide
exceptions and their reasons belong in `[workspace.lints.clippy]` in
`Cargo.toml`.

- **Plan review:** Does any proposed exception have a concrete reason and the
  narrowest appropriate scope under the existing exception mechanisms?
- **Code review:** Do suppressions retain useful lint coverage and follow the
  item-level or documented exception conventions?

Source: [Conventions](../../../../AGENTS.md#conventions).

## local-area-instructions

Applies to the Steel runtime or SDK and Kitty graphics conformance. Read the
[Steel instructions](../../../../yinz-plugin-host/src/steel/CLAUDE.md) before
reviewing runtime or SDK changes. Check relevant host authority, confinement,
generation/lifetime, event-shape and worker/test contracts, and SDK
documentation. Read the
[Kitty graphics instructions](../../../../yinz-terminal-graphics/src/kitty_graphics/CLAUDE.md)
for graphics conformance and the pinned judgment calls in the
[module documentation](../../../../yinz-terminal-graphics/src/kitty_graphics/mod.rs)
before changing covered behavior. Conformance tests follow the protocol spec and
quote the sentence tested; inspect the affected protocol, grid/host-emission and
whole-app callers. The root focused-test policy still governs test selection.

- **Plan review:** Has the plan consulted applicable local instructions and
  preserved their contracts and deliberate conformance decisions?
- **Code review:** Does the implementation respect those contracts, with checks
  at the affected layers and documentation updated when their guarantees change?

Source:
[Plugins, hooks, and skills](../../../../AGENTS.md#plugins-hooks-and-skills) and
the linked subsystem instructions.

## documentation-and-skills

Applies when user-facing behavior, commands, configuration, developer workflows
or plugin contracts change. Inspect affected [docs/](../../../../docs/) and
[repository skills](../../), including their reference files, examples and
command instructions. Steel changes may affect the
[plugin guide](../../../../docs/PLUGINS.md),
[SDK guide](../../../../docs/PLUGIN_SDK.md) and
[testing guide](../../../../docs/PLUGIN_TESTING.md). Lifecycle changes may
affect the [operating guide](../../../../docs/ISSUE_LIFECYCLE.md) and
[lifecycle skill](../SKILL.md).

- **Plan review:** Does the plan identify which docs, skills and SDK guides need
  updates, or explain why existing guidance remains accurate?
- **Code review:** Do affected instructions, links and examples describe the
  delivered behavior and commands? Cite a concrete mismatch for a finding rather
  than requiring unrelated documentation work.

Sources:
[Plugins, hooks, and skills](../../../../AGENTS.md#plugins-hooks-and-skills),
[Steel instructions](../../../../yinz-plugin-host/src/steel/CLAUDE.md) and the
linked guides.
