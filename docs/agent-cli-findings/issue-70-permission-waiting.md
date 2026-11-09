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

## Merge-gate repair: split rejection and complete search query

The controller's merge attempt `6dcbe016-f1e5-4b94-805e-e27ab25754ea`
failed at clean HEAD `7f2519385ec7a649bb01db5742c4303b24d36af4`.
Read its durable `ci-check.log` before edits: the workspace phases passed, but
the integration snapshots captured a normal narrow pane title instead of the
split rejection and `SEARCHING: NEEDL` instead of the full query. Neither
snapshot needed an update.

The split test's PTY `done` output and chrome predicate did not establish that
the asynchronous rejection had rendered. The test now requires the second-row
`CA…` title, the narrow rendering of `CAN'T SPLIT!`, before taking the snapshot.
An initial focused run with only that stronger final predicate exposed a second
synchronization gap: 40 tests passed but this test timed out after 46.448 seconds.
It started the split after the prompt alone, while plugins were still loading;
the temporary error could expire before the required chrome was ready. The
test now waits for the prompt and narrow chrome before sending the split, then
waits for the rejection content. This fixes the observed timeout's cause rather
than accepting a passing retry. Width and snapshot assertions remain intact.

The search test already scrolled to line05 for a prefix of `NEEDLE`, so its
mode, match and scroll checks could accept incomplete input. Its snapshot wait
now also requires `SEARCHING: NEEDLE`. Both repairs use existing observable
content waits, without sleeps or production behavior changes.

Focused validation on Linux (2026-11-09), after both synchronization repairs:

- `cargo x integration-test --test panes --test search -j 16`: passed,
  41 tests across two binaries, zero skipped; 9.168 seconds in nextest.
- `cargo x format --check`: passed.
- `git diff --check`: passed.

No competing Cargo/nextest/rustc processes were present before integration runs.
Socket binding used escalation on the first attempt; xtask built this worktree's
own binary and retained its real, isolated target directory. Existing snapshots,
permission tracking, fallback release, quiet-pane rendering and dependency
resolution are unchanged. These are focused fake-OS integration checks, not a
full CI gate, live Claude, remote SSH or macOS measurement. No background jobs
or subagents were started by this repair task.

## Supplemental repair R5–R7: passive observations and interrupted recovery

Selected finding: `issue70-permission-observation-blocks-recovery`, UX review
at `7c584ac8888f905f827f5b55699aadc3c257abd5`, decision
`205119bc-79ef-4456-b99f-54b4fd0714c1` (approved repair R5–R7).
Only this recovery finding is repaired here; the prior native fallback ownership,
observer retry/lease, freshness and scheduler decisions remain intact.

The queued report path normalized permission observations correctly but then
advanced `Lifecycle.last_report` for every event. An accepted Inactive or
Unavailable observation after Esc therefore invalidated both interrupted
Recover and ClearDraft's input-after-hook proof. During clearing it also changed
the timestamp captured by the operation, despite no provider activity.

`agent_control_event` now exempts only matching ClaudePermissionStatus event/hook
identities carrying an accepted, normalized Inactive or Unavailable payload.
The timestamp remains exactly unchanged. Native hooks (including ones enriched
with an inactive registry sample), worker hooks and accepted Active permission
evidence still advance it; a later passive sample cannot erase that fence.
Report validation, watermark/presentation updates, control observations and
plugin delivery continue normally. Recovery authorization, quiet timing,
input/composer guards, schema and handoff representation are unchanged. The
user guide clarifies this boundary alongside the existing inspection guidance.

Five new portable queued-report regressions compose acknowledged submission,
terminal/CLI Esc, accepted passive evidence and recovery/clearing. They cover
Inactive, Unavailable and an older Active sample normalized to a retained
release; unchanged timestamp, input consumption/revision, parent, confirmation
and submission ownership; passive plugin events; early/plain recovery refusal;
quiet recovery to Idle/ready with Interrupt and PaneUpdate publication and an
Interrupted submission whose task completion remains unverified. Empty and
nonempty composer cases deliver more passive reports during marker verification
and stable-empty proof. Negative controls retain native/worker/Active fences,
interrupt clearing on actual activity and reject recovery of an unacted-on
native fallback or worker question, preserving the native latch.

Before the production repair, the corrected five-case selection failed four
assertions on the timestamp changing and passed the actual-activity clearing
control. Initial fixture development also exposed an un-abandoned submission
and a queued NeedsInput reply; the fixture now waits on the acknowledged
submission, explicitly abandons it with Esc, drains the cancellation reply and
checks the actual recovery rejection. These were fixture issues, not passing
reruns used to dismiss a runtime flake. After the repair and fixture corrections,
all five new cases passed without sleeps or new synchronization timeouts.

Focused development validation on Linux (2026-11-09):

- `cargo x test -p yinz-screen -E 'test(claude_permission_registry_recovery)'`:
  5 passed, 1722 skipped; doctests skipped by filter.
- `cargo x test -p yinz-screen -E 'test(/^screen::screen_tests::agent_control::/) | test(/^screen::screen_tests::agent_resume::/) | test(/^screen::screen_tests::agent_tracking_handoff::/)'`:
  350 passed, 1377 skipped; doctests skipped by filter.
- `cargo x integration-test --test claude_steering --test waiting_agent_bells -E 'test(claude_permission_latency) | test(waiting)' -j 16`:
  2 passed, 6 skipped; 20.178 seconds in nextest (consumer 12.095s,
  waiting UI 8.083s).
- `cargo x format --check` and `git diff --check`: passed.

These are development results before the repair commit; exact-commit repetitions
of the approved checks are recorded in the submitted task evidence. A competing
worktree build was observed during prelaunch inspection; it finished before the
integration check, whose immediate prelaunch check found no Cargo/nextest/rustc
processes. The existing all-slot latency reservation is unchanged. Socket checks
used escalation on their first attempts and all builds used this worktree's own
real target directory and production binary.

The new recovery composition uses portable screen tests with a fake OS; the
unchanged Linux consumer fixture uses actual reporter/observer and local IPC
with a test-owned provider/registry and fake whole-app OS. No authenticated live
Claude, end-to-end recovery, remote SSH or macOS result is claimed. No full
suite, full CI, rebase, merge, remote Git operation or background agent was used.
