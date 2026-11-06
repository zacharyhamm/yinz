# Issue 26: checkout execution boundary proof

Status: **blocked at approved plan v3, step `prove-portable-execution-boundary`**.
This is a capability experiment and a blocking design result, not an implemented
checkout policy. The issue's execution-scoping acceptance criterion is unmet.
The remaining plan steps depend on a viable boundary and have not been integrated.
No lifecycle evidence format, launcher behavior, provider support, or ordinary
pane behavior changes in this commit.

## Reproduce the foreground experiment

From `extensions/`:

```sh
deno test --allow-read --allow-write --allow-run --allow-env \
  behaviors/unit/checkout_boundary_probe_test.ts -- --checkout-boundary-probe
```

The probe is opt-in. Ordinary Deno test runs ignore it, so they do not acquire a
Linux, Landlock, C compiler, or kernel-header requirement. Explicit execution on
another platform fails with an explanation; an unavailable Linux ABI fails rather
than reporting successful coverage. Dependencies for this experiment are Linux
with enabled Landlock ABI 3 or newer, Linux Landlock headers, Deno, `cc`, Git,
Python 3, readable Linux procfs, and `/usr/bin:/bin` system tools. It does not use
Bubblewrap.

The Deno harness compiles the small
[C executor](../../extensions/behaviors/unit/fixtures/checkout_boundary_probe.c)
into a disposable directory, creates a primary repository and linked worktrees,
and awaits every subprocess. A Linux
[Python supervisor](../../extensions/behaviors/unit/fixtures/checkout_probe_supervisor.py)
sets `PR_SET_CHILD_SUBREAPER` before launching each command and owns its 30-second
deadline. It waits for the whole descendant tree, including orphaned or detached
children. On deadline or interruption it sends `SIGKILL` to unreaped direct
children, repeats for descendants adopted as their parents die, and reaps all of
them before returning. Timeout exits 124; command startup failure exits 125;
ordinary command exit statuses are preserved. Termination and collection still
depend on kernel scheduling and killable processes; the deadline is not a hard
real-time guarantee against uninterruptible kernel waits.

The Deno harness removes the fixture in `finally`; no providers, Yinz servers,
lifecycle methods, Agenda operations, or remote Git services are invoked. The
supervisor regressions deliberately fork disposable children and verify their
collection before returning. Worktree build output is local to that disposable
worktree; no Cargo build directory is changed. The supervisor is test cleanup,
not a production enforcement backend.

The executor changes to the assigned directory or exits 125, opens the rule roots,
sets `PR_SET_NO_NEW_PRIVS`, installs a Landlock mutation ruleset, closes its rule
descriptors, and calls `execvp`. Its allowlist is the assigned tree, private
scratch, write/truncate access to `/dev/null`, and an optional extra writable tree
used to test Git metadata access. `TMPDIR` points at scratch. Reads, executable
lookup, IPC, networking, and unhandled metadata operations remain unrestricted.
This is deliberately a partial candidate, never a production sandbox.

## Observed results

On 2026-11-06, Linux x86_64 kernel `7.3.1-1-cachyos`, Landlock ABI 11,
Deno 2.9.8, GCC 16.2.1, and Git 2.56.0: **1 test, 14 steps passed**.
A passing counterexample step means the candidate failed a required boundary.
These are real local subprocess checks, not provider execution checks.

Code review found that the original `AbortSignal.timeout` killed only the immediate
child: a descendant retaining stdout/stderr delayed completion beyond the deadline.
The supervisor repair adds six focused steps: retained pipes with a live parent,
an exited parent, a detached double-forked grandchild, successful child collection
with preserved command status, supervisor interruption, and an unavailable
executable. Timeout fixtures ignore `SIGTERM` and sleep for 20 seconds; a one-second
deadline must return within ten seconds with every reported PID absent from procfs.
The supervisor and original boundary proof now pass **2 tests, 20 steps**.
The first repair run failed because Deno protects `/proc` even with `--allow-read`;
the collection assertion now uses the existing Python dependency under the same
documented permissions instead of adding `--allow-all`.

| Experiment | Result |
| --- | --- |
| Assigned edits, compiling and running a binary under its own `target/`, private scratch writes | Allowed |
| Canonical symlink alias and ordinary assigned subdirectory | Allowed |
| Shell-internal `cd` to another checkout | Allowed; child `pwd` reports the other checkout |
| File creation there after `cd`, absolute-path write, Python child write, symlink escape | Denied with `EACCES` |
| Truncating another checkout's existing source/build output, removing its source | Denied |
| `git -C`, explicit `--git-dir`/`--work-tree`, and `GIT_DIR`/`GIT_WORK_TREE`/`GIT_INDEX_FILE` selecting foreign metadata | Mutations denied when common metadata is read-only |
| Byte comparison of foreign worktree files, build output, and all common Git files after denied operations | Unchanged, including index, HEAD, refs and reflogs |
| Assigned `git add` and `git commit` with common metadata read-only | Also denied: legitimate implementation is blocked |
| Assigned commit with common metadata writable | Allowed |
| Foreign branch update and foreign index write with that same writable common metadata | Also allowed: boundary violated |
| Control checkout grant containing `.worktrees/contained` | Foreign edit, index write and commit allowed: boundary violated |
| Existing nested repository under the assigned tree | Foreign edit and index write allowed: boundary violated |
| Nested repository created and edited by descendants after restriction | Creation, edit and index write allowed: boundary violated |
| Assigned directory removed between preflight and execution | Executor exits 125 before command execution; no cwd fallback |

The first run exposed fixture permission omissions: GCC attempted scratch files
in shared `/tmp`, and Git could not open `/dev/null` for writing. The executor now
sets a private `TMPDIR` and grants only the required `/dev/null` rights. The corrected
run verifies successful builds and Git operations alongside the denials. Those
initial failures were fixture defects, not evidence of checkout containment.

## Why the first deliverable blocks integration

Landlock restrictions extend to descendants, but a path-tree allow rule does not
understand Git checkout identity. It also does not mediate `chdir`, several
metadata operations, or access through previously opened files. Newer socket
restrictions have distinct ABI requirements. See the
[kernel's Landlock API and limitations](https://docs.kernel.org/userspace-api/landlock.html).
Our subprocess results demonstrate that inheritance alone is insufficient.

The assignment may be the control checkout, with implementation worktrees beneath
it. Granting the whole tree immediately admits those checkouts. Inventorying and
excluding current nested repositories would still leave repositories created
after launch. The proof does not establish that every possible Landlock-based
design fails; it establishes that this inherited tree-grant design cannot satisfy
the approved contract without additional synchronous mediation.

Linked worktrees use common Git metadata; Git documents their
[repository layout](https://git-scm.com/docs/gitrepository-layout). The probe shows
the concrete conflict: direct access sufficient for assigned commits also allows
another branch and index to change. Scrubbing launch-time Git variables does not
remove explicit Git options or variables supplied by a descendant.

An assignment-bound Git broker and host-control broker have **not** been proved.
A CLI wrapper would be bypassable by shell/interpreter file operations. A general
host shell endpoint would recreate unrestricted authority. A correct broker needs
validated operations, controlled Git configuration/helpers/hooks and environment,
revision guards and atomic ref updates, plus filesystem authority that cannot
reach foreign checkouts during its subprocesses. It must handle both primary and
linked assignments, nested repositories created at runtime, and index/object/ref
updates needed for a normal commit. No such backend is shipped here.

On macOS, a supported equivalent also remains unestablished. Apple's DTS explains
that custom sandbox profiles are not a supported third-party interface in
[its sandbox-exec guidance](https://developer.apple.com/forums/thread/661939).
This does not prove portable enforcement impossible. It prevents treating a
custom profile as a demonstrated supported backend. There is no macOS host in
this experiment. A VM or another execution service would require its own design,
credential/tool provisioning, broker, and provider compatibility proof; none is
silently added as a prerequisite.

## Backend and provider capability matrix

| Candidate backend | Linux without mandatory Bubblewrap | macOS | Readiness verdict |
| --- | --- | --- | --- |
| Inherited Landlock tree grants used in the experiment | Partial mutation restrictions measured; checkout/Git counterexamples reproduced | Linux API unavailable | Not ready |
| Existing Yinz project Bubblewrap environment | Optional Linux feature; broader authority than an assignment | Unsupported by this feature | Not an assignment backend |
| Trusted checkout/Git/control mediation | Not implemented or proved | Not implemented or proved | Blocking deliverable |
| Custom macOS sandbox profile | Not applicable | Unsupported interface; not tested | Cannot claim supported coverage |

The existing [Bubblewrap contract](../BUBBLEWRAP.md) explicitly grants writable
primary checkout access, including contained worktrees and `.git`, and exposes
full Yinz control through shared sockets. Read-only socket mounts still allow
host actions. Reusing that environment would not close the demonstrated holes.

| Lifecycle provider | Repository execution-path evidence | Linux capability check | macOS capability check |
| --- | --- | --- | --- |
| Claude | `yinz-runtime/src/agent/launch.rs` prepares CLI/hooks; `yinz-process/src/claude_turn.rs` observes turns | CLI found; no enforced tool/delegation/backend readiness test | Unavailable |
| Codex | `yinz-codex/src/lib.rs` normally connects to a shared daemon; a private backend exists for Bubblewrap and project overrides | CLI found; restricting just its UI cannot cover the existing daemon; no checkout-bound backend tested | Unavailable |
| Muse | `yinz-muse/src/lib.rs` launches the CLI and registers filtered-hook routing | CLI not found; external tools and delegation unverified | Unavailable |
| Pi | `yinz-runtime/src/agent/pi-extension.ts` reports `tool_execution_start` | CLI not found; this callback is observational, not a synchronous gate | Unavailable |
| AGY | `yinz-runtime/src/agent/launch.rs` prepares CLI/resume arguments and permission flags | CLI found; execution services and delegation unverified | Unavailable |

Finding a CLI on `PATH` is inventory only. No provider prompt or live provider
smoke test ran: there is no candidate ready for provider integration. Hooks,
conversation cwd, and process cwd cannot certify individual tool containment.
No platform/provider cell has full execution-policy coverage.

## Required operation contract and readiness protocol

There is **no implemented execution-policy identity or valid readiness
attestation**. The following is the minimum continuation contract, not a claimed
API or an assurance supplied by the experimental executor:

1. Before resources are created, negotiate a versioned policy and supported
   backend for the selected platform/provider. Bind a canonical assignment root,
   worktree Git directory, common Git directory, permitted branch operations,
   policy digest, task/provider identity and a fresh launch nonce.
2. Permit ordinary assigned edits, builds with that checkout's own `target/`,
   and commits through trusted Git mediation. Allow only enumerated runtime
   dependencies/credentials/caches and private scratch that do not expose another
   checkout or execution service. Protect Git administration data from direct
   writes. Preserve legitimate canonical aliases and ordinary subdirectories;
   reject separately identified nested repositories and linked worktrees.
3. Establish restrictions in the actual executor and every execution backend
   before prompt delivery. Close inherited authority-bearing descriptors and
   sockets. Validate effective environment, paths and Git identity again there.
   Every command/tool/delegation route needs equivalent inherited enforcement or
   synchronous trusted authorization. Observation callbacks are insufficient.
4. Expose only an assignment-scoped reporting/control capability. Return a
   readiness result correlated to the nonce, policy digest, assignment identity,
   provider conversation/backend process, actual restrictions and covered tool
   routes. Readiness must also check existing credentials, tools and hooks inside
   that executor; it must not install hooks or provision a private home implicitly.
5. Refuse readiness on missing capabilities, directories, changed identities or
   unsupported routes. Denied operations must fail synchronously with assignment,
   attempted target, operation and recovery context; preserve a sticky violation.
   Pending validation or missing readiness blocks success. Restart/resume must
   establish fresh readiness rather than replaying historical success.
6. Only then integrate the remaining approved policy, provenance, report-routing,
   completion and durable-history steps. Keep assigned root, observed location,
   observation origin/freshness and executor attestation distinct. No configured
   root or prompt-time cwd may be promoted into verified execution evidence.

To continue, establish and review a concrete backend that closes the demonstrated
Git, nested-checkout and host-service holes, demonstrate a supported macOS path,
then run the required Linux/macOS checks for all five providers. If those require
different product prerequisites or scope, return that concrete design decision to
the controller. Do not turn this proof into a Linux-only lifecycle requirement,
replace enforcement with directory validation, or report the issue as resolved.

## Focused verification

The proof is standalone and changes no Rust runtime or protobuf definitions; it
does not require building Yinz. Focused results for this commit:

- Explicit boundary experiment and supervisor regressions: 2 tests, 20 steps
  passed, including all 14 original boundary steps.
- Existing `yinz_agent_test.ts` and `issue_agents_test.ts`, plus the probe without
  its opt-in flag: 105 passed, 0 failed, 2 ignored (the opt-in probe tests).
- `deno check`, `deno lint`, and `deno fmt --check` on the probe: passed.
- Python supervisor syntax check: passed.
- `git diff --check`: passed.

No workspace test suite, integration suite, full CI gate, rebase, merge or push is
part of this worker task.
