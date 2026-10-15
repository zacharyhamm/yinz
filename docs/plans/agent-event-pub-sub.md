# Agent event subscriptions

Status: proposed implementation plan. No implementation changes are included.

## Recommendation

Add `yinz agent watch` over the existing session IPC connection. Publish normalized events from the screen thread, where agent state and submission correlation already live. Keep a bounded, in-memory replay log for each running session. Reuse the existing client writer and NDJSON conventions. Do not add a broker, database event table, or polling loop.

The stream describes observations that Yinz accepted and applied. It cannot guarantee that every provider action produced a hook. It must distinguish a confirmed turn outcome from state changes and observation failures.

## Current behavior and reusable code

Paths below are relative to the repository root. Function names identify the relevant code without depending on changing line numbers.

| Area | Verified behavior and source |
| --- | --- |
| CLI | `yinz-cli/src/args.rs` defines `Agent::{List,Prompt,Wait}`. Execution is in `src/agent.rs`, rather than entirely in `yinz-cli`. `run_inner` resolves one session. `exchange` opens IPC, sends `AgentControl`, receives one response, and sends `ClientExited`. |
| Submission identity | `src/agent.rs::submission_id` creates a caller-visible UUID before IPC. `yinz-types/src/agent/control.rs` defines `Prompt`, state-based `Wait`, and submission-based `WaitTurn`. `--conversation` guards the provider conversation, not the Yinz session. |
| Correlation | `yinz-types/src/agent/control.rs::PromptObservation` verifies prompt fingerprints, provider prompt/turn IDs, conversation identity, and provider-specific confirmations. A successful PTY write does not mean acceptance. Claude notification turns cannot complete an unrelated caller submission. |
| Screen ownership | `yinz-screen/src/screen/agent_control.rs` owns pending controls, lifecycle, background task tracking, and submission results. `SUBMISSION_HISTORY_LIMIT` is 32 per retained conversation. Lifecycle invalidation removes that history. `detach_agent_controls` detaches clients but retains prompt observation. |
| State | `yinz-types/src/agent.rs::AgentInfo` appears in `yinz-types/src/data.rs::PaneInfo.agent`. States are Busy, Waiting, Idle, and Exited. `is_waiting_on_a_person()` combines Waiting with `wait_needs_a_person`. `session_id` means provider conversation ID. `run_id` belongs to plugin history and is optional. |
| Hook installation | `src/agent_hooks.rs` installs provider integrations. Generated local reporters call `yinz action agent-report --hook-json`. Remote reporters use the pane broker. This differs from session configuration hooks. |
| Report ingestion | `src/agent_report.rs`, `yinz-cli/src/actions.rs`, and `yinz-runtime/src/agent/report.rs` construct and validate reports. `yinz-server/src/client_router.rs::accept_agent_report` acknowledges successful screen enqueue, not completed state application. |
| Report application | `yinz-screen/src/screen/thread_main.rs::apply_queued_agent_report` rejects stale evidence, normalizes confirmations, applies state, calls `agent_control_event`, then broadcasts `Event::AgentEvent` through `PluginInstruction::Update`. Pre-creation reports are buffered and replayed before newer queued instructions. |
| State mutation | `apply_pane_agent_report` in that same file preserves `run_id`, invalidates changed lifecycles, and reports changed pane state. Repeated identical records do not cause another manifest update. Spawn, rerun, closure, and direct plugin writes also affect agent state. |
| Existing event stream | `yinz events --event-type agent_event` already works at the code-contract level. `yinz-types/src/data.rs::CLI_STREAMABLE` includes AgentEvent. `yinz-client/src/cli_client.rs::start_events_client` streams flushed NDJSON. `yinz-plugin-host/src/plugin_bridge.rs` registers CLI subscribers, requests initial state, and forwards events. It coalesces consecutive state reports. It has no replay cursor or correlated submission envelope. |
| Raw payload | `yinz-types/src/agent/hooks.rs::AgentEvent` carries hook name, pane, kind, provider session, optional state, prompt, last message, and bounded raw JSON. It is a hook notification, not a normalized turn-result contract. |
| Permissions | `yinz-plugin-host/src/plugin_bridge.rs` requires `ReadApplicationState` for plugin AgentEvent subscriptions. CLI subscriptions use session socket access. `yinz-server/src/lib.rs` sets socket permissions. |
| Writes to clients | `yinz-server-os/src/os_input_output.rs::CoalescingSendQueue` has a bounded ordered queue and separate client writer threads. Only eligible pane render snapshots are coalesced. Overflow disconnects a client. Ordinary control messages have a count bound, but the existing byte accounting covers renders. |
| Project selection | `yinz-plugin-host/src/agent_control.rs::filter_project` uses the project registry, host/session-owned assignments, replacement project IDs, repository probes, and location-aware cwd matching. It is more than a path prefix test. |
| Other watches | `yinz-storage/src/plugin_data_store/watch.rs` watches query results in one process, suppresses unchanged results, and owns watches by plugin instance. `yinz-plugin-host/src/watch_filesystem.rs` watches filesystem changes. Neither is an ordered agent lifecycle log. |
| Pipes | `yinz-plugin-host/src/pipes.rs::PendingPipes` tracks plugin callbacks, holds, and request completion. It supplies useful cleanup examples, but a permanent pipe would tie subscriptions to plugin lifetime without solving replay. |

## Public event model

Add separate normalized types under `yinz-types/src/agent/`, for example `events.rs`. Do not rename or overload the existing raw `AgentEvent`.

Each event has this envelope:

| Field | Meaning |
| --- | --- |
| `schema_version` | Initially 1. Additive fields are compatible. Consumers ignore unknown optional fields. |
| `event` | Stable snake_case event type. |
| `host_id`, `session` | Owning Yinz server host and session name. Derive these from server context. |
| `epoch` | Random server-lifetime identity. A restart creates a new epoch, even with the same session name. |
| `sequence` | Strictly increasing session-wide sequence within the epoch. |
| `cursor` | Opaque encoding of version, owner, epoch, and sequence, plus filter identity for filtered subscriptions. |
| `pane_id`, `pane_generation`, `pane_sequence` | String pane ID, process-incarnation identity, and strictly increasing per-pane event sequence. |
| `kind` | Provider kind when known. |
| `conversation_id` | Provider conversation ID, copied from `AgentInfo.session_id` when applicable. Nullable. |
| `submission_id` | Yinz submission identity only when correlated. Never invent one for a human-entered turn. |
| `turn_id`, `run_id` | Provider turn ID and plugin history run ID. Both nullable and independent. |
| `observed_at` | Server observation time for diagnostics. Ordering comes from sequence numbers, not clocks. |
| `payload` | Typed event-specific data, with bounded strings and collections. |

Expose sequences as decimal strings in JSON so JavaScript consumers do not lose 64-bit integer precision. Protobuf uses `uint64`.

Use these event types:

- `agent_state_changed`: previous/current state and relevant metadata, source, readiness, status availability, `waiting_on`, and `needs_input`. Metadata-only changes must be observable. `needs_input` is a derived property, not a fifth AgentState.
- `submission_acknowledged`: emit only after matching provider evidence confirms acceptance. Include correlation fields and `acknowledged: true`.
- `turn_completed`, `turn_failed`, `turn_interrupted`: emit confirmed semantic transitions once. Include outcome, correlation status, and a bounded reason. Never convert a request timeout, lost connection, or unverified Stop into `turn_failed`.
- `submission_observation_changed`: represent uncertainty, status unavailable, target changed/closed, and interrupted observation without claiming the provider turn ended. Distinguish these from verified interruption.
- `pane_spawned`, `pane_closed`, `pane_replaced`: agent-related pane lifecycle, including registration/adoption origins. Replacement contains previous and next incarnation identities. A close contains the last known agent identity.
- `conversation_replaced`: previous/new provider conversation IDs and reason. Initial binding from an unknown conversation is metadata establishment, not replacement. Preserve the existing provisional Muse and `/clear`/`/new` behavior.
- `background_tasks_changed`: added/removed task IDs and current outstanding IDs. Initially this describes the Claude tasks tracked by `track_agent_turn`, not arbitrary Yinz jobs. Removal means Yinz observed the notification turn finish, not an independently verified process exit.

Use separate control records: `hello`, `snapshot_begin`, `snapshot_item`, `snapshot_end`, `ready`, `checkpoint`, and `gap`. Control records do not consume pane event sequences. Include the latest pane sequence and generation in snapshot items. All JSON records are one line.

Order events by screen-thread application, not provider timestamps. A report can yield several events. Stage them through state mutation and correlation, then publish them in a documented order: identity/lifecycle, state, acknowledgement/outcome, background tasks. Preserve old identities for invalidated submissions. Publish only after successful application, and before the next screen instruction. Do not sequence from the report enqueue acknowledgement.

Per-pane counters continue across conversation and process replacement for the same pane ID within one epoch. `pane_generation` changes on process replacement, including rerun. Keep bounded tombstones while events remain replayable. A reused numeric ID after tombstone eviction gets a fresh generation. Consumers identify panes with the full owner/epoch/pane/generation tuple.

## CLI and practical consumption

Proposed interface:

```sh
yinz --session work agent watch --json
yinz --session work agent watch --pane terminal_7 --kind codex --json
yinz --session work agent watch --project yinz --event-type turn_completed --json
yinz --session work agent watch --cursor "$CURSOR" --json
```

`watch` is the only new command initially. An alias `subscribe` adds no capability and can wait. `--json` selects stable NDJSON. Without it, print concise human-readable records. Diagnostics go to stderr. Flush every record.

Allow repeated `--pane`, `--kind`, and `--event-type` values. Values within one filter are ORed. Different filters are ANDed. `--project` takes one registry name. Validate unknown event types, projects, malformed cursors, and excessive filter sizes before registration. Omitted filters select all agent-related events in the chosen session.

Fresh subscriptions receive an atomic current-state snapshot and then `ready`. Events that follow have sequence greater than the snapshot boundary. Do not implement this as `agent list` followed by a separate subscribe request.

Use `--cursor` for resume. Do not add a bare `--since-sequence` initially: sequence alone is ambiguous across sessions and restarts. A later shorthand must also require epoch and owner. Reject changed filters on a saved cursor. Start a fresh snapshot when changing filters.

A controlling agent should run the stream as a managed background command:

1. Start watch and consume through `ready`.
2. Choose a submission ID and submit with the existing guarded `agent prompt --submission ID --conversation ID --json` command.
3. Match events by host, session, pane generation, conversation, and submission. Allow a documented conversation-reset transition.
4. React to `needs_input`, confirmed outcomes, target loss, or background task changes. Never treat general Idle as proof that this submission succeeded.
5. Save a cursor after processing its record. Deduplicate replay by owner/epoch/sequence. Side effects require the controller's own idempotency guard.
6. On `gap`, rebuild from the supplied snapshot. Use `agent wait --submission` to retrieve retained results where available. Unknown history is uncertainty, not permission to resend a prompt.

A Claude Code Monitor, if available in the installed version, can own this same long-lived command and notify its controller on matching records. No specific Monitor API is assumed or verified here. A normal background command is sufficient. Reading streamed stdout or awaiting monitor notifications removes Yinz polling, but an LLM still needs its execution environment to wake it.

The CLI stays connected until cancellation, server shutdown, or transport failure. Return nonzero on unexpected disconnect so a supervisor can resume with a saved cursor. Do not silently reconnect across a gap. On Ctrl-C, normal exit, or broken stdout, close IPC and send `ClientExited` when possible. EOF and forced termination must also trigger server cleanup. Closing a subscription never cancels a prompt, pane, or background task. A closed pane does not end a session-wide watch.

## Delivery, replay, and backpressure

Keep one screen-owned `VecDeque` of normalized events per session. Allocate sequence numbers even with no subscribers. Bound retained count and encoded bytes. Initial limits can be constants, proposed at 4,096 events and 8 MiB, with a 16 KiB maximum event record. Validate and truncate descriptive fields explicitly. Do not silently truncate identifiers needed for correlation.

Guarantees apply to events Yinz publishes: ordered delivery on a live connection and replay while retained. They do not cover missing provider hooks or durable delivery after restart. Across reconnects, delivery is at least once when the cursor remains available. Exactly-once consumer side effects are out of scope.

Registration, snapshot boundary capture, and replay-to-live transition happen on the screen thread. For a valid cursor, send retained matching events strictly after its sequence, then `ready` at the replay high-water mark. New live delivery follows that mark without a registration race.

For an expired cursor or a different epoch on the same owner, send an explicit `gap` with reason, requested cursor, retained range, and new epoch. Follow it with a fresh snapshot and `ready`. Wrong-owner or future-sequence cursors are errors. A restart loses the replay log and cannot reconstruct all transitions from PaneInfo or archives.

Filtering naturally creates sequence gaps. Such gaps do not imply loss. Send bounded periodic checkpoints for filtered-out progress so an idle matching stream can advance its cursor. A checkpoint only covers events already scanned and all preceding matching records queued before it. Never advertise an event as processed by the consumer.

Reuse `CoalescingSendQueue` for ordered IPC records. Do not coalesce lifecycle events. Overflow disconnects only the slow subscriber. A final lag reason is best effort because a full socket cannot guarantee another message. Reconnect and replay use the last processed cursor.

Bound replay batches and schedule continuation instructions instead of filling a client's entire queue in one screen turn. A replaying subscriber retains a next-sequence pointer into the shared log, not an unbounded private queue. If the log overtakes it, report a gap or disconnect. Verify bounded memory across the entire path, including bus queues and snapshot output. Add event byte accounting or enforce record/count limits that provide a calculable byte ceiling. Bound subscriptions per connection and total server subscribers.

## Server implementation outline

1. **Contracts:** Add normalized event/filter/cursor types in `yinz-types`. Keep existing raw hook events and AgentResponse behavior compatible.
2. **IPC:** Add `SubscribeAgentEvents` and a structured server stream record to `yinz-ipc/src/ipc.rs`. One subscription belongs to one connection. Extend `yinz-ipc/src/client_server_contract/client_to_server.proto`, `server_to_client.proto`, and shared definitions as needed. Update `yinz-ipc/src/ipc/protobuf_conversion.rs` and exhaustive message matches. Allocate new field numbers. Run `cargo x proto` through `xtask/src/build.rs`, and inspect generated changes. No plugin protobuf change is necessary for the CLI-only first version.
3. **Routing:** In `yinz-server/src/client_router.rs`, validate and route subscriptions to screen instructions defined in `yinz-bus/src/screen.rs`. Reuse client IDs and OS client writers. Reject requests before session readiness rather than silently dropping them. Ensure this connection does not become an interactive UI client or change focus/session lifetime.
4. **Sequencing:** Store replay state and subscriber metadata in `yinz-screen/src/screen/mod.rs`, with a small adjacent implementation module. Send stream records through the existing OS enqueue API available to Screen. Avoid an extra plugin-thread hop or new dispatcher thread. The server owns socket cleanup through the current router/client-removal paths.
5. **Emission:** Extend `apply_queued_agent_report`, `apply_pane_agent_report`, `agent_control_event`, `record_submission`, and lifecycle invalidation with semantic emission points. Do not emit once per waiter reply from `finish_agent_control`: several clients can observe the same outcome, and retries must not duplicate completion events. Emit when the retained semantic fact changes, independent of attached waiters.
6. **Other mutation paths:** Audit all callers of those functions, `SetPaneAgent`, `SetPaneAgentRun`, `fire_agent_start_for_new_pane`, `cleanup_closed_pane`, rerun/replacement, adoption/warp, and process exit. Preserve the last identity before removal. The relevant paths are in `yinz-screen/src/screen/{mod,thread_main}.rs`, with producer instructions in `yinz-bus/src/{screen,pty}.rs`. Check `yinz-pty` and `yinz-tab` producers before adding hooks there. Prefer one emission at the screen commit point.
7. **Unsubmitted turns:** Extend lifecycle tracking only as needed to publish verified human/provider-originated turns without a CLI waiter. Reuse the provider correlation rules. Leave `submission_id` null. If a provider cannot verify an outcome, publish state/uncertainty rather than manufacturing completion.
8. **CLI:** Add arguments in `yinz-cli/src/args.rs` and a streaming branch in `src/agent.rs`. Reuse session resolution, permission diagnostics, broken-pipe handling, and the NDJSON pattern from `yinz-client/src/cli_client.rs`. Keep one-shot `exchange` unchanged for existing requests.
9. **Cleanup:** Remove subscriptions on every disconnect path, alongside existing screen client cleanup. Test EOF and queue-overflow cleanup, not only explicit unsubscribe. No unsubscribe request is needed when the connection owns exactly one subscription.

Project filtering requires deliberate integration. Extract the current `filter_project` membership logic into a reusable resolver in `yinz-plugin-host/src/agent_control.rs`. Keep database and repository probes outside the screen event loop. Feed versioned membership updates to screen, keyed by pane generation and identity revision. Discard stale results.

Initial project subscriptions must buffer/replay from their registration boundary while membership resolves. If retention expires, report a gap. Subsequent spawn, assignment, cwd, and repository changes refresh membership through the same resolver. Retain old membership for close/replacement events and match transition events against either old or new membership. Commit membership changes in screen order. State the resulting observation boundary explicitly: asynchronous repository evidence becomes effective when applied, not retroactively when the filesystem changed. Unknown membership must be reported, not silently interpreted as non-membership.

Project registry writes from another server do not wake local database watches. Initially require resubscription to refresh externally changed assignments, and document that limit. Live cross-server project registry updates require a separate invalidation solution. Do not use repeated `agent list` queries to simulate the event stream.

## Ownership, security, and session scope

Follow `CLAUDE.md` shared-data rules. Every identity and saved cursor includes both `host_id` and Yinz session. Use `agent_proto::agents::rows::Owner` or the existing project-data owner helpers for persisted assignment queries. Carry server host identity from startup. Never call `get_plugin_ids()` from library code.

There are no database writes in the proposed replay design. If project integration later adds writes, preserve author-read `expect_rev`. Never claim legacy empty-host records as live events. Preserve the documented legacy run/import behavior. Shared SQLite storage does not make live events multi-session or cross-host.

Version one watches one explicitly resolved session. For several sessions, launch one watch per session and maintain separate cursors. There is no global order across them. Remote panes report through their owning Yinz session, whose host/session identify the event. A pane's SSH target is separate metadata. A pane moved to another session produces source removal and destination adoption. `run_id` can help link them, but the stream does not promise an atomic cross-session move.

Use existing socket permissions as the CLI trust boundary. Filters are selection, not authorization. A watch connection must not gain prompt, spawn, or plugin permissions merely by subscribing. Do not expose prompt text, raw hook JSON, transcripts, or full responses in the normalized stream. Bound and JSON-escape descriptive text. An optional content stream would require a separate explicit design.

`src/agent.rs::allow_sandboxed_yinz` currently appends `-c sandbox_workspace_write.network_access=true` for Codex launches and rejects other kinds. The source explains that Linux sandbox socket connections need network access. The watch uses the same socket and denial messages from `yinz-cli/src/sessions.rs`. `--allow-yinz` is a launch-time sandbox setting, not a narrowly scoped read-only Yinz capability. Do not bypass denied connections or change sandbox configuration from watch. Runtime behavior with the installed Codex version remains to be tested.

Keep existing plugin AgentEvent permissions unchanged. If normalized events become plugin-facing later, add an explicit permission mapping and regenerate plugin protobufs with `cargo x proto`. Read `yinz-plugin-host/src/steel/CLAUDE.md` before changing Steel runtime or SDK contracts.

## Validation and incremental rollout

Use adjacent `unit/` modules for new unit tests. Existing anchors include `yinz-screen/src/unit/screen_tests/agent_control.rs`, `yinz-cli/src/unit/agent_cli_tests.rs`, `src/unit/agent.rs`, `yinz-server/src/unit/client_router_tests.rs`, and `yinz-ipc/src/ipc/tests/roundtrip_tests.rs`.

1. **Contracts and sequencing:** Test event and cursor round trips, nullable correlation IDs, rejected filters, large sequences, atomic snapshot boundaries, and ordering. Add stream registration/cleanup without changing existing wait behavior.
2. **Semantic events:** Test Busy/Waiting/Idle, needs-input suppression, stale and duplicate hooks, provisional conversation binding, confirmed/failed/interrupted turns, uncertain observation, multiple waiters, and disconnect-before-completion. Test Claude background notifications separately from the original submission. Compare stream outcomes with existing wait outcomes.
3. **Lifecycle and filters:** Test spawn, registration, exit, close, rerun, replacement, run-ID preservation, adopted panes, conversation reset, and numeric pane reuse. Test project assignment/repository rules, membership races, old-membership close delivery, host/session isolation, and unsupported provider uncertainty.
4. **Delivery hardening:** Test retained and expired cursors, epoch changes, future cursors, changed filters, filtered checkpoints, replay-to-live races, oversized payloads, and slow readers. Use the real OS writer for queue limits and disconnection. Confirm healthy subscribers and screen input continue while another consumer stops reading.
5. **Whole-app CLI:** Extend `yinz-integration-tests/tests/agent_agent_hooks.rs` or an adjacent agent subscription suite. Start a watch, await `ready`, inject known reports, and verify exact correlated output. Test broken stdout, Ctrl-C/EOF, forced subscriber exit, session shutdown/restart, and cleanup without cancelling work. Use a real binary test for the client writer path.

Use `yinz_test_support::Watched<T>::wait_until(what, timeout, predicate)` and event/ready barriers. Do not add sleeps or assertions based on arbitrary render counts. Use `write_warm_script` for executable stubs. Use generous deadlines unless timeout is the behavior under test.

Implement in small reviewable steps matching the sequence above. Keep the public command experimental until replay, cleanup, and semantic tests pass. Add project filtering after the core stream works, but complete it before claiming this goal is delivered. Leave existing list/wait/prompt and raw `yinz events` supported throughout.

For each implementation step, run affected crate tests through `cargo x`. Before branch handoff, run `cargo x ci check`. Shared IPC/types, screen, and plugin-host changes require full `cargo x test` and `cargo x integration-test -j 16`. The CI command includes those phases. Use sandbox escalation on the first socket-binding suite attempt. Build this worktree's plugins/binary with its own real `target/`. Never set `CARGO_TARGET_DIR` or share artifacts. If server-OS code changes, include `cargo x test -p yinz-server-os -p yinz`. Remote suites are needed if those spawning/transport paths change.

This planning-only change requires document/path checks, not builds or runtime suites.

## Alternatives, risks, and open questions

- **Wrap `yinz events`:** Useful today for raw hooks and pane snapshots. It cannot supply atomic correlated results, replay, or reliable transition history. A wrapper would duplicate screen correlation and inherit coalesced state reports.
- **Poll list or repeat wait:** Existing commands remain useful for reconciliation. Polling misses intermediate transitions and repeated wait requires per-submission orchestration.
- **Plugin pipes or database watches:** Reuse ownership lessons, not their transport. They couple delivery to plugins/query results and do not establish the required ordering or replay semantics.
- **Durable event database or external broker:** Defer until restart-surviving history is required. This adds retention, migrations, ownership, and database load. The first version explicitly reports a restart gap.
- **Extend generic `yinz events` with every guarantee:** Possible later, but it broadens this change to unrelated event classes and plugin permissions. Keep the normalized agent stream separate initially.
- **Provider evidence:** Completion coverage varies. Do not promise confirmed outcomes for providers that only report coarse state. Verify every supported provider's failure/interruption evidence during implementation.
- **Project membership:** Async probes and external database changes are the largest filtering risk. The proposed application-order boundary and resubscription limit need user-facing documentation and tests.
- **Budget defaults:** Measure realistic event rates and encoded sizes before fixing retention/subscriber limits. A bounded implementation is required from the first stream commit.
- **Compatibility:** Decide how an older server rejects an unknown subscription request promptly. A bounded handshake timeout with a clear unsupported-version error is the fallback. Do not leave the CLI waiting forever.
- **Exactly-once publication:** Existing waiter completion paths are not a general event ledger. Review all outcome transitions before choosing emission locations, especially recovery, interruption, and multiple observers.
- **Unverified in this review:** No live server, hook installation, Monitor integration, sandbox session, or end-to-end stream was executed. The code paths above were inspected statically. Full adoption/warp and process-exit coverage, protocol-version negotiation, and dynamic project invalidation still require implementation-time tracing. No claim is made that current raw events provide these proposed guarantees.
