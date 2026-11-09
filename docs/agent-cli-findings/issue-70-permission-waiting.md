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
