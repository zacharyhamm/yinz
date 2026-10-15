# Agent spawn tab selection

Status: proposal only. No implementation is included. Source inspection: 2026-10-04.

## Recommended contract

Add explicit placement to `yinz agent spawn`. Start with stable tab IDs and optional names for newly created tabs. Keep automatic placement when neither option is supplied.

```sh
yinz agent spawn --kind claude --project work --branch review --tab 7 --json
yinz agent spawn --kind claude --cwd . --branch review --new-tab --json
yinz agent spawn --kind claude --cwd . --branch review --new-tab review --json
```

| Input | Meaning |
| --- | --- |
| No placement option | Preserve current automatic placement. |
| `--tab ID` | Place the pane in this existing stable tab ID in the selected Yinz session. |
| `--new-tab [NAME]` | Always create a tab, optionally named, and place the pane there. |
| Both options | Reject before checkout or process creation. |

Use `--tab` with a checked nonnegative integer, including zero. Do not interpret it as a tab-bar position. Reject names in the first release with an actionable error. Names are mutable and can be duplicated. This avoids numeric-name ambiguity and a second lookup mode. A new tab may have the same name as another tab: it is creation, not “focus or create.” Reject empty, whitespace-only, or control-character names. Absence of a name uses the normal default name. Treat a supplied name as user-named so automatic naming does not replace it.

Support the same choices in the flat `--file` launch object:

```json
{"kind":"claude","cwd":"/work/repo","branch":"review","tab":7}
```

```json
{"kind":"claude","cwd":"/work/repo","branch":"review","new_tab":true}
```

```json
{"kind":"claude","cwd":"/work/repo","branch":"review","new_tab":"review"}
```

Absent or null means unspecified. `new_tab: false` means no new-tab request. Reject other types, invalid IDs, invalid names, and two active placement choices. Normalize these boundary values into one typed placement choice: automatic, existing ID, or new tab with optional name. Keep this placement on `AgentRequest::Spawn`, separate from command/Git planning in `AgentSpawnSpec`.

CLI placement overrides the entire JSON placement choice. For example, `--tab 7` replaces a file's `new_tab` choice. Reject two CLI placement flags through Clap. Validate the merged request again on the server, including direct IPC callers. Other file precedence remains unchanged. Do not introduce global rejection of unrelated unknown JSON fields as part of this feature.

## What the current code does

Paths below are relative to the repository root. Links work from this document.

1. [`yinz-cli/src/args.rs`](../../yinz-cli/src/args.rs), `Agent::Spawn`, defines the current flags and promises keyboard-focus preservation, including overflow into another tab. There is no placement option. [`src/agent.rs`](../../src/agent.rs), `Agent::Spawn`, reads a JSON object, overlays non-null CLI values, rejects `prompt` and `reuse_prompt`, deserializes `AgentSpawnSpec`, and separately extracts project, cwd, and SSH host. It submits `AgentRequest::Spawn` with a 120-second CLI timeout.
2. [`yinz-server/src/client_router.rs`](../../yinz-server/src/client_router.rs) routes spawn to `yinz_plugin_host::control_agent`, using the last active client when available. [`yinz-plugin-host/src/agent_control.rs`](../../yinz-plugin-host/src/agent_control.rs), `prepare_automation_agent`, resolves registered projects, validates cwd, applies project worktree/image defaults, and checks SSH constraints. `--project` does not select a tab here.
3. [`yinz-runtime/src/agent/launch.rs`](../../yinz-runtime/src/agent/launch.rs), `plan`, is a pure command and Git-checkout planner. It does **not** choose a screen tab. [`yinz-plugin-host/src/yinz_exports.rs`](../../yinz-plugin-host/src/yinz_exports.rs), `prepare_agent` and `spawn_agent_for_control`, prepare resources, then send `PtyInstruction::SpawnTerminal` with tiled placement and `ClientIdNoFocus`. The PTY path is in [`yinz-pty/src/pty.rs`](../../yinz-pty/src/pty.rs).
4. [`yinz-screen/src/screen/panes.rs`](../../yinz-screen/src/screen/panes.rs), `place_new_pane`, invokes [`agent_pane_target`](../../yinz-screen/src/screen/agent_placement.rs). This screen-thread policy estimates cell area per tiled pane. It can select a roomier existing tab or create one when the source is crowded. It excludes pending/fullscreen alternatives. Explicit `TabIndex` and `TabIndexNoFocus` targets already bypass this heuristic.
5. [`yinz-screen/src/screen/warp_ops.rs`](../../yinz-screen/src/screen/warp_ops.rs), `new_tab_for_adopted_pane`, creates a pending tab and sends its default layout through plugin → PTY → screen. It suppresses the adopted command's duplicate layout run. The helper is also used by warp operations, so changes must preserve those callers. Pending tab work is buffered in [`yinz-tab/src/tab/mod.rs`](../../yinz-tab/src/tab/mod.rs).
6. Completion uses [`yinz-bus/src/route.rs`](../../yinz-bus/src/route.rs), `NotificationEnd` and `ActionCompletionResult`. It already has `affected_pane_id`, `affected_tab_id`, and an error. `place_new_pane` sets the pane ID, but does not currently set the actual tab ID. `spawn_agent_for_control` returns actual checkout resources with that pane ID. Registration failure can still return `spawned`; failures before a known pane use `launch_resources`.

### Existing actions and project behavior

- `action new-tab --no-focus --name N` maps to `Action::NewTab` in [`yinz-cli/src/actions.rs`](../../yinz-cli/src/actions.rs). [`yinz-routing/src/lib.rs`](../../yinz-routing/src/lib.rs) sends `ScreenInstruction::NewTab` with focus disabled. [`yinz-screen/src/screen/tab_layout.rs`](../../yinz-screen/src/screen/tab_layout.rs) supplies tab completion metadata when the layout lands. Reuse the underlying lifecycle rather than invoking this CLI from the spawn handler.
- `pane-move-to-tab` is a named service operation, not a direct spawn placement. [`yinz-cli/src/operations.rs`](../../yinz-cli/src/operations.rs) sends an `agent_action` pipe. [`default-plugins/agent-service/src/operations.rs`](../../default-plugins/agent-service/src/operations.rs) validates pane/tab presence and calls `break_panes_to_tab_with_id(..., false)`. The workaround has separate creation, spawn, and move outcomes and depends on the service route.
- `project-assign` writes `agent_assignment` rows keyed with host, Yinz session, and tab ID in that same daemon file. It publishes tab-project metadata. [`yinz-plugin-host/src/agent_control.rs`](../../yinz-plugin-host/src/agent_control.rs), `filter_project`, gives explicit tab assignment precedence over checkout discovery for `agent list --project`.
- `arrange-panes-by-project` routes through [`yinz-plugin-host/src/lib.rs`](../../yinz-plugin-host/src/lib.rs) and [`default-plugins/project-store/src/main.rs`](../../default-plugins/project-store/src/main.rs). [`default-plugins/project-store/src/arrangement.rs`](../../default-plugins/project-store/src/arrangement.rs), `classify`, uses checkout locations, repository probes, and the project registry. It does not use `tab_projects` to classify groups. [`yinz-screen/src/screen/arrange_projects.rs`](../../yinz-screen/src/screen/arrange_projects.rs) selects/creates destinations and moves panes. This is a separate explicit operation, not the spawn crowding policy.

## Placement, projects, and focus

Placement and checkout selection remain independent. Allow either placement option with `--project`. Do not infer a project or cwd from the chosen tab, change an existing assignment, or automatically assign a new tab. Preserve all current project/cwd/SSH restrictions.

Explicit placement wins over the crowding heuristic, even when another tab has more room. If insertion cannot succeed, return failure rather than choose another tab. Leave ordinary tiled-pane behavior, such as fullscreen handling, consistent with existing explicit tab insertion. Test it and document any visible effect.

Allow an explicit project to differ from the destination's assigned project. Explain the consequence in help and the skill: the checkout comes from `--project`, but project-filtered listings can classify the pane by its tab assignment. This is consistent with moving the pane using today's workaround and avoids a new assignment-dependent launch restriction. If a conflict warning is added, it must use authoritative scoped data and must not turn service availability into a launch requirement.

A later `arrange-panes-by-project` or pane move may move the pane again. Spawn placement is an initial destination, not a persistent pin. A later `project-assign` may change its effective project classification. Neither behavior changes in this proposal.

All modes preserve keyboard focus for every attached client. Use no-focus routing and create tabs without attaching clients to them. Do not focus and then restore: that can overwrite intervening user input or focus changes. Do not add a focus override in the first release. Existing explicit focus actions remain available.

## Launch lifecycle and failures

Use the screen thread as the authority for tab existence, creation, placement, and conditional cleanup. Keep checkout preparation and process launch off that thread.

1. Parse and validate placement before side effects. For an existing tab, ask the screen to resolve the ID before Git preparation. Missing tabs return `rejected`; never fall back to the active tab or create a replacement.
2. Associate explicit placement with an in-flight launch identity. A tab close must invalidate its launches. Carry this identity through PTY creation and final placement. Release it on completion, cancellation, or session teardown.
3. Prepare checkout resources through the current guarded path. For a requested new tab, create it after successful checkout preparation and immediately before process dispatch. Return its ID to the coordinator. Reuse the adopted-pane/default-layout lifecycle and add optional naming without changing existing callers' behavior.
4. Insert the pane into the selected tab without focus. Revalidate the launch identity at insertion and at pending-layout replay. A queued insertion is not yet a successful placement. Complete the launch only after the pane belongs to a live tab, or after a terminal failure.
5. Set completion's actual tab ID from screen state. Track a tab created by this launch separately. Propagate these fields through both success and partial-failure responses.

**ID reuse is a real race.** [`yinz-screen/src/screen/tabs.rs`](../../yinz-screen/src/screen/tabs.rs), `get_new_tab_id`, returns the largest live ID plus one, or zero. IDs survive reordering but can be reused after a highest-ID tab closes. Checking only `tabs.contains_key(id)` after checkout boot can target a replacement tab. Use close-invalidated launch identities, including pending layout work and cleanup. Do not claim IDs are globally unique or durable across server restarts. A session-wide monotonic allocator is an alternative, but requires an audit of restore/warp paths and is not assumed here.

If the destination closes during launch, fail without rerouting. If a process already exists, close the unplaced pane through the existing [`forget_unplaced_pane`](../../yinz-screen/src/screen/mod.rs) path. That helper cleans screen state and requests PTY closure. `place_new_pane` currently logs/drops a missing target without setting a completion error in that arm. The implementation must propagate an explicit failure and must not report a successful `spawned.tab_id` for that case. Report any allocated pane identity as partial resources, without promising that the pane remains live.

For new-tab failure, attempt cleanup only on a definitive failure and only on the screen thread. Delete a launch-owned empty tab after its pending layout work has been cancelled/drained and no pane can arrive later. Never delete a pre-existing tab or an ID that now refers to another tab. If the default layout created plugin/terminal panes, the user changed the tab, another launch used it, or ownership cannot be proved, retain it and report it. This conservative rule can leave an orphan tab with layout content; document that limit rather than close potentially useful panes. Do not add automatic Git branch/worktree deletion.

On timeout or a lost reply, outcome is unknown. Do not close tabs or repeat spawn blindly. Preserve known resources and use the existing inspect-before-retry guidance. A late placement can still complete. This feature does not provide durable launch deduplication or rollback of executed agent commands.

## Response contract

Extend [`SpawnedAgent` and `AgentLaunchResources`](../../yinz-types/src/agent.rs) with optional fields and serde defaults for compatibility:

- `spawned.tab_id`: the tab that actually accepted the pane, observed at successful placement. New successful direct CLI launches must supply it, including automatic placement.
- `spawned.created_tab_id`: a tab created by this launch, if any, including automatic overflow. Absent for a pre-existing destination.
- `launch_resources.tab_id`: a resolved/requested destination when known on a failure before successful placement. Document that this does not prove a live pane exists there.
- `launch_resources.created_tab_id`: a created tab that remains or whose cleanup outcome is unknown. Omit after confirmed cleanup. Describe confirmed cleanup or uncertainty in `message`.

Preserve `pane_id`, actual `cwd`, actual `branch`, `requested_branch`, `reused`, and `occupied_by`. On a partial failure with a known pane, use `spawned` for known pane/checkout data and retain tab cleanup details if needed. Do not unconditionally erase tab resources merely because a pane ID was allocated. `outcome` remains authoritative.

The success tab ID is an observation at placement, not a promise against later moves or closure. Callers that need its current location can inspect status. Text output should also show the actual tab and whether the launch created it.

## Implementation boundaries and transport

| Area | Planned change |
| --- | --- |
| `yinz-cli/src/args.rs`, `src/agent.rs` | Flags, optional name parsing, file normalization, conflict validation, text output/help. |
| `yinz-types/src/agent/control.rs`, `yinz-types/src/agent.rs` | Optional typed request placement and additive response/resource metadata. |
| `yinz-plugin-host/src/agent_control.rs`, `yinz_exports.rs` | Placement coordination around existing preparation, failure/resource propagation, registration handling. |
| `yinz-bus/src/screen.rs`, `pty.rs`, `route.rs` | Minimal request/reply and in-flight placement data. Reuse `affected_tab_id`; add created-tab metadata where needed. |
| `yinz-screen/src/screen/{panes,agent_placement,warp_ops,tabs,tab_layout}.rs` | Resolve, create, invalidate on close, place, report, and conditionally clean up. Audit close paths outside these files too. |
| `yinz-pty/src/pty.rs`, `yinz-tab/src/tab/mod.rs` | Carry placement identity and completion through process creation and pending tab replay, if required. |
| Tests and `assets/skills/yinz-agents/SKILL.md` | Contract coverage, launch guidance, help fixtures, and recovery examples. |

Keep the pure shared launch planner unchanged unless reusable syntax validation belongs there. The first release adds placement to the direct guarded `AgentRequest::Spawn`, not to every plugin launch surface. Existing Agent/modal/plugin callers retain their placement behavior.

The direct agent-control IPC already uses JSON within protobuf envelopes: see [`yinz-ipc/src/ipc/protobuf_conversion.rs`](../../yinz-ipc/src/ipc/protobuf_conversion.rs), `AgentControlJson`. Request/response JSON changes need round-trip tests but no new IPC protobuf fields. Update all Rust constructors of the changed request/response types.

The plugin API is different: [`yinz-plugin-api/src/plugin_command.proto`](../../yinz-plugin-api/src/plugin_command.proto) has explicit `AgentSpawnSpec` and `SpawnAgentResponse` messages, converted in [`plugin_command.rs`](../../yinz-plugin-api/src/plugin_command.rs). For the proposed CLI-only request, no request proto change is needed. Plugin responses can leave the new optional Rust fields absent in this release. If tab selection or tab response parity is extended to Scheme callers, add numbered optional proto fields, update converters, SDK/schema declarations and shims, and run `cargo x proto`. Never hand-edit generated assets. Read [`yinz-plugin-host/src/steel/CLAUDE.md`](../../yinz-plugin-host/src/steel/CLAUDE.md) before SDK changes.

Tabs belong to the receiving local Yinz server regardless of where the command runs. Apply the same placement to local and SSH panes. Preserve SSH's existing absolute cwd and explicit worktree-guard requirements, and its incompatibility with local project/branch options. Long SSH startup makes close races particularly important. No new SSH protocol field is expected: placement remains server-side.

[`CLAUDE.md`](../../CLAUDE.md) is a symlink to [`AGENTS.md`](../../AGENTS.md). Scope any assignment reads by both `host_id` and `yinz_session_name`, using existing owner helpers and server startup identity. Do not look up a tab by ID across sessions or use hostnames as host IDs. This feature needs no assignment writes. If later work adds them, retain author-read `expect_rev`, legacy-row rules, and the cross-server watch limitation. Do not call `get_plugin_ids()` from library code.

## Tests and validation

Use existing adjacent unit directories and whole-app integration suites. Add focused tests for the contract rather than a second launch harness.

1. Extend [`yinz-cli/src/unit/agent_cli_tests.rs`](../../yinz-cli/src/unit/agent_cli_tests.rs) and [`src/unit/agent.rs`](../../src/unit/agent.rs): all flag/file forms, zero/invalid/overflow IDs, optional name parsing, CLI/file precedence, both-choice rejection, and unchanged prompt rejection. Verify failures occur before Git/tab/PTY side effects.
2. Extend [`yinz-ipc/src/ipc/tests/roundtrip_tests.rs`](../../yinz-ipc/src/ipc/tests/roundtrip_tests.rs): request choices, old messages without placement, and new success/partial-failure fields. Test server validation independently of Clap.
3. Extend screen placement tests in [`yinz-screen/src/unit/screen_tests/agent_placement.rs`](../../yinz-screen/src/unit/screen_tests/agent_placement.rs): explicit destination bypasses crowding, reordered IDs, new tab naming, automatic created-tab reporting, pending layout completion, missing/closed targets, close-and-recreate with the same numeric ID, and concurrent launches.
4. Extend [`yinz-integration-tests/tests/agent_pane_placement.rs`](../../yinz-integration-tests/tests/agent_pane_placement.rs): assert the returned tab contains the pane and keyboard input still reaches the original pane. Cover current/other/new tabs, multiple attached clients, no attached client where supported, and a launch without Agent services. Keep automatic-placement cases unchanged.
5. Extend [`yinz-plugin-host/src/unit/agent_control.rs`](../../yinz-plugin-host/src/unit/agent_control.rs) and [`yinz-integration-tests/tests/agent_tab_projects.rs`](../../yinz-integration-tests/tests/agent_tab_projects.rs): project and tab selection remain independent, assignments control filtering, later arrangement may move panes, and identical tab IDs on other hosts/sessions do not interfere.
6. Inject failures before preparation, after checkout, after tab creation, during delayed PTY startup, and during pending layout replay. Assert error outcomes, retained checkout details, empty-tab cleanup, preservation of populated tabs, unknown-outcome behavior, and no false success after target closure. Exercise local/SSH command targets in shared-path tests.

Use `Watched::wait_until` and existing render-content predicates. Do not use sleeps, render counts, or `snapshots.last()` as synchronization. Use `yinz_test_support::write_warm_script` for executable stubs. Give remote startup sufficient bounded time.

Run targeted crate/behavior checks while developing. Because this touches screen/PTY/tab plumbing and multiple crates, run full `cargo x test` and `cargo x integration-test -j 16`. Before branch handoff, run `cargo x ci check`, which includes the required format, lint, unit, and integration phases. Build the worktree's own native binary first. Keep a real isolated `target/`, do not set `CARGO_TARGET_DIR`, and use sandbox escalation on the first socket-binding suite attempt.


## Small rollout steps

1. Fix placement completion reporting and error propagation first. Add actual tab metadata for existing automatic launches and regression coverage. Preserve behavior otherwise.
2. Add stable-ID placement end to end, including early validation, close invalidation, file parity, and no-focus tests. Do not ship a flag that can silently land elsewhere.
3. Add named/unnamed new-tab placement using the existing layout path, with created-tab reporting and conservative cleanup. Include timeout and pending-layout race coverage before enabling it.
4. Update help and the repository's [`yinz-agents` skill](../../assets/skills/yinz-agents/SKILL.md). Explain IDs versus positions, session selection, project classification, no-focus behavior, actual returned IDs, and failed-spawn reconciliation including tabs. Update installed skill copies through the normal distribution mechanism, not an unrelated home-directory edit.
5. Run the full gate and review intentional help/snapshot changes. Treat name lookup and plugin placement parity as follow-up choices, not prerequisites for the CLI feature.

## Alternatives, open questions, and limits

- **Keep the workaround:** no new launch API, but extra service-dependent operations, interim placement, multiple failure windows, and no unified resource result. Keep it as a recovery technique, not the recommended implementation.
- **Spawn then move internally:** reuses the move operation, but permits automatic overflow tabs and a running pane in the wrong destination before the move. Reject this approach for explicit initial placement.
- **Accept `--tab ID|NAME` immediately:** convenient, but numeric names need escaping or a separate option. If added later, prefer `--tab-name NAME`: exact case-sensitive matching in the selected session, reject zero or multiple matches, return candidate IDs on ambiguity, then pin the resolved tab identity. Never choose the first match or re-resolve by name after closure.
- **Assign the new tab to `--project`:** changes durable project metadata and listing semantics. Defer unless users explicitly want that behavior. The proposed initial implementation permits mixed-project tabs and documents the consequence.
- **Focus override:** defer. Adding one later requires explicit multi-client targeting semantics rather than guessing from the last active client.
- **Cleanup of layout-created panes:** the initial empty-tab-only rule is intentionally conservative. More aggressive cleanup needs proven launch ownership and user-activity tracking. Default layouts may make this limitation common.
- **Pending layouts and completion ownership:** the existing `NotificationEnd` sends on drop, and cloning does not clone its sender. Audit each transfer and buffered insertion during implementation. Do not signal success early or drop a reply while waiting for a layout.
- **Old running servers:** serde defaults support old requests on a new server, but an old server may ignore new request fields. Do not advertise cross-version placement guarantees. Check existing version negotiation before implementation and require a matching/restarted server if unsupported options could be ignored.
- **Not verified by this planning task:** no code was changed or executed to test launch behavior, no CI/test suites were run, and no live SSH sessions were exercised. The complete close/cancel/layout-failure state machine, installed skill distribution, and mixed-version rejection path still need implementation-time tracing. Source-level findings above do not establish runtime race coverage.
