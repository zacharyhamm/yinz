# Issue 70: prompt Claude permission Waiting

Implemented from approved plan v1 against base
`220767c44679dcab36413c0b576b10aafc4b0152`.

The source report, `/tmp/yinz-claude-idle-tracking-report.md` (2026-11-08),
describes Claude Code 2.1.319 / sonnet and installed Yinz 0.1.2-alpha.9:
registry waiting immediately, Yinz Waiting 5.6–6 seconds later, and a prompt
approved after approximately three seconds with no Waiting edge. Those are the
report's measurements, not new live measurements.

## Evidence and contract

The pinned Linux executable
`/home/zacharyhamm/.local/share/claude/versions/2.1.319` was read at the following
zero-based byte offsets. Both hashes were independently matched during this
implementation to the issue-69 provider source index:

| Entry | Offset / bytes | SHA256 |
| --- | --- | --- |
| S18 | 321043195 / 850 | 7516baadb09555ad0e3ad1509027a9e1a35e83568b3540de4096859c1c2506a2 |
| S19 | 321054529 / 1300 | cf6a184a58cfdbc9ac8d409df6299aa445079141ac61eee16699f3ddfeab6756 |

S18 distinguishes permission prompts from questions, generic dialogs and
worker/sandbox requests. S19 persists status and waitingFor through an asynchronous
React effect. Native release hooks may therefore precede the registry rewrite.
The adapter requires matching PID/incarnation, interactive kind and conversation,
a fresh statusUpdatedAt, and exactly `waitingFor=permission prompt` with
`status=waiting`. The guarded reader retains nonblocking regular-file reads and
the 64 KiB ceiling. Missing/unknown data never starts a human wait.

An independent local permission observer starts after acknowledged SessionStart,
UserPromptSubmit or PermissionRequest. It requires neither a transcript nor an
unfinished parent turn. Its separate process/conversation/route lock prevents a
per-request observer fleet. Polling is 100 ms; active heartbeats are 500 ms;
unchanged inactive observations are deduplicated. The generic session-wide signal
does not invent a worker/tool identity. Remote reporting retains native hooks.

The screen retains registry permission presentation separately from the parent,
workers and notification obligations. Source retirement, registry watermarks and
equal-timestamp release precedence prevent stale reentry. Native permission
reminders with usable registry evidence join this lifetime; a sampled release
suppresses a late reminder before state/control/plugin readers. Missing registry
evidence retains the independent native fallback wait. Raw hook fields remain
available to history readers.

## Review findings addressed

- **issue70-observer-loss-overlay:** bounded reporter failures retry while the
  provider/conversation is alive, rather than abandoning a published wait.
  Two seconds of unknown reads sends unavailable. A separate three-second screen
  lease, renewed by active observations, also releases the overlay if the observer
  dies or its route remains unavailable. Loss restores the current aggregate and
  preserves independent native question/worker waits; it establishes no completion.
  Read timestamps distinguish a fresh guarded observation of an unchanged dialog
  from an old heartbeat after loss. Explicit cancellation and provider releases
  retain registry timestamp fencing.
- **issue70-latency-check-isolation:** the new integration latency case reserves
  all nextest threads within the existing `-j 16` check. Other integration coverage
  keeps its ordinary scheduling. The root observer cases likewise reserve the
  unit scheduler, and the assigned root check uses `-j 1`.

## Focused verification

The approved commands select types hook/control tests, the complete Claude/AGY
observer module, screen agent-control/resume tests, native agent-report observer
tests, and the claude_steering/waiting_agent_bells integration targets. Formatting
and focused Clippy cover the changed code too. Full CI, unfiltered test suites,
Docker/SSH tests, rebase, merge, push and remote Git operations are outside this
worker assignment.

Development results: types 87 passed; process 41 passed; screen 343 passed; native
reporter/observer 5 passed; integration targets 8 passed. The real-process consumer fixture passes its independent
three-second parent Write and post-confirmation background-worker Bash dialogs,
requiring CLI status/watch and Scheme PaneUpdate/AgentEvent Waiting within one
second on the warmed path. It observes CLI needs_input, ordered releases, one
Waiting edge per dialog, delayed reminders, immediate automatic allow/deny and an
800 ms automatic evaluation with no Waiting. The fixture owns and reaps its
provider, observers, reporters, watch and prompt processes. The strict process
consumer fixtures run on Linux; portable reader/reducer/screen tests exercise the
shared behavior. Existing waiting-bell integration coverage is selected alongside
the new case.

Failures investigated during development:
- The initial process run lacked socket permission for two existing AGY HTTP
  fixtures (PermissionDenied). The same selected command with local binding
  enabled passes.
- Replacing all legacy reconciliation exposed two deterministic regressions:
  adopted notification completion and the selected worker wait detail. Hook-only
  reconciliation is preserved; registry-owned presentation uses its own aggregate.
  The complete selected screen coverage then passes.
- The new integration fixture initially expected the wrong CLI watch envelope/
  ready event, included server-owned Scheme children in orphan cleanup, and treated
  the CLI's intentional nonzero needs_input exit as a command failure. The fixture
  now decodes payload/ready, excludes the server's existing children from reaping,
  and asserts the needs_input JSON outcome. Its isolated regression passes after
  these corrections. A passing rerun was not used to dismiss any timeout.

## Limits

No fresh authenticated live Claude session was launched. The real-process tests
use a test-owned provider and registry with Yinz's actual reporter/observer binary
and IPC/screen/plugin delivery; they do not establish every Claude version,
remote latency or the report's separate F4/F5 post-approval readiness defects.
Registry presentation remains optional input evidence, not completion or
composer-readiness proof. No public CLI flags, permission policies, protobufs,
portable skills, production Scheme or SDK interfaces were changed.

## Supplemental repair: issue70-native-fallback-release-latch

The approved repair decision `ca48c6d8-dc70-469f-8f0f-d4b068613191`
selects the same finding from technical and UX review of
`00656f2b7900f40f000d5d21bc03af9e342ecb7d`.
Registry-only staging retained the native Waiting parent but removed its
`PermissionWait`, preventing a subsequent forwarded Enter from releasing it.
The new screen composition regression failed at the missing native latch before
the production repair; this was the expected reproduction, not a timing flake.

Registry presentation now stages aggregate activity without replacing the parent
or its native release ownership. Ordinary report/adoption staging still clears
its latch as before; identity changes and exit still invalidate retired state.
The preserved latch keeps its original conversation and input revision. Enter
releases native fallback only; an active registry overlay or worker question
continues to require its own release. This restores the fallback contract in
AGENT_CLI.md without changing commands, permission policy, schemas or remote
behavior.

New screen coverage composes native fallback Notification with Unavailable,
Inactive, repeated Active samples and registry-owned reminders, then forwards
Enter through terminal and CLI input. It checks the original latch/revision,
Busy versus confirmed-parent Idle, cleared native detail/person flags,
PaneUpdate/status release, independent registry/worker waits and source
replacement. Existing placement, input, cancellation and handoff coverage is
included in the selected screen check.

The existing isolated Linux consumer fixture now adds two fallback phases after
its latency and automatic-policy controls. Each acknowledges a fresh inactive
registry sample, removes the file, establishes native fallback Waiting, and waits
for the observer's real two-second read-loss report. One phase also restores a
newer inactive record before Enter. Per-phase watch/event barriers and Scheme
notification/status counters ensure earlier rounds cannot satisfy assertions.
Real `agent send-keys` forwards Enter and CLI status/watch plus Scheme PaneUpdate
release without any post-tool, denial or completion hook. Enter does not require
or fabricate a provider AgentEvent. Registry loss may normalize to the retained
Inactive watermark, so the barrier checks the advanced read timestamp.

Repair development results (2026-11-09), separate from the historical results
above:

- The new single screen regression failed before the fix and passed after it.
- `cargo x test -p yinz-screen -E 'test(/^screen::screen_tests::agent_control::/) | test(/^screen::screen_tests::agent_resume::/) | test(/^screen::screen_tests::agent_tracking_handoff::/)'`:
  345 passed, 1376 skipped; doctests skipped by filter.
- `cargo x integration-test --test claude_steering --test waiting_agent_bells -E 'test(claude_permission_latency) | test(waiting)' -j 16`:
  2 passed, 6 skipped. The consumer case including the new fallback phases took
  11.667 seconds; bell rendering took 7.023 seconds. No competing Cargo/nextest
  run was present before starting; the existing all-slot latency reservation stays
  unchanged.

The three approved repair checks (including `cargo x format --check`) are run
again at the final repair commit before evidence submission. These are focused
checks, not a full CI run. The consumer test uses the actual worktree reporter
and observer with a test-owned provider/registry and fake whole-app OS backend
on Linux; no fresh authenticated Claude, macOS or remote SSH measurement is
claimed. All fixture processes/readers are stopped and reaped before return.

## Merge-gate repair: asynchronous disconnect and quiet-pane focus

The controller's merge attempt `8f965717-bd6a-4009-ae65-52dc2cdb1922`
failed at rebased HEAD `a3af2b45e370609fa5f2e1a04cfe7800c8d61e5f`.
The durable `ci-check.log` was read before edits; its two failures were
investigated in this same issue worktree. Passing diagnostic reruns were not
treated as resolutions.

The legacy-client refusal test observed `Exit` and immediately required the
client to be absent from inventory. `DetachSession` sends Exit before
`remove_client_and_flush_forwards` removes the interactive client, while the
inventory endpoint reads shared state from another router. Receiving Exit is
therefore not a cleanup fence. The test now polls inventory through a scoped
worker and `Watched::wait_until_described`, preserving the unsupported-message
assertions and reporting the observed inventory on timeout. No server protocol
or disconnect semantics changed. The initial 24 selected real-client tests
passed before this test repair, consistent with a scheduling-dependent assertion;
that pass alone did not address the race.

The OSC 7 timeout had a production cause: `focus_pane_by_request` changed focus
and published session metadata without scheduling a frame. For a focus change
within the same tab, neither tab switching nor scroll-mode synchronization
necessarily requests rendering. Unrelated plugin or PTY activity could mask the
missing request; quiet panes could retain the old cursor/frame and omit the
focus-dependent OSC 7 report indefinitely. The focus path now schedules a render
after its state change, retaining the already-focused no-op and deferred-layout
behavior.

The new synchronous screen regression services only render jobs requested by
focus itself, without plugin/PTY activity or an explicit rescue render. It
requires a post-request frame with the upper pane's cursor/content and its actual
OSC 7 output. During test development, an incorrect expected cursor row and
incomplete cumulative ANSI replay were corrected; this fixture has no tab-bar
row. With those corrections, removing the production fix reproduced the missing
post-request frame deterministically. Restoring the render request resolves it.

Focused repair validation on Linux (2026-11-09):

- `cargo x test -p yinz --bin yinz -E 'test(session_update_refuses_legacy_client_without_sending_unknown_messages) | test(publication_skips_disconnected_shared_client_but_acknowledges_connected_client)'`:
  2 passed, 233 skipped; doctests skipped by filter.
- `cargo x test -p yinz-screen -E 'test(/^screen::screen_tests::osc7::/) | test(/^screen::screen_tests::cli_actions::/)'`:
  64 passed, 1658 skipped; doctests skipped by filter.
- `cargo x integration-test --test osc7_forwarding --test claude_steering --test waiting_agent_bells -j 16`:
  13 passed, including the failed OSC 7 case and existing Claude permission
  latency, fallback and consumer checks. No competing Cargo/nextest run was
  present; the existing latency scheduler reservation remains intact.
- `cargo x lint`: passed, including deny-warnings Clippy and builtin plugin checks.
- `cargo x format --check` and `git diff --check`: passed.

Socket-binding checks used escalation on their first attempts and built this
worktree's own binary with its real target directory. These checks do not claim a
full merge gate, live Claude, remote SSH or macOS measurement. The inherited
permission implementation and additive dependency resolution remain intact.
