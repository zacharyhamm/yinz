# Four bb features to adopt in Yinz

Date: 2026-10-26

Adopt repeatable worktree preparation, a durable agent prompt queue, a workspace
review pane, and provider usage visibility. These features improve the daily
cycle of starting work, assigning follow-ups, reviewing changes, and choosing
an available provider. They fit Yinz's terminal interface and external agent
model.

| Priority | Feature | Main benefit | Relative implementation effort |
| --- | --- | --- | --- |
| 1 | Worktree preparation before agent startup | Fresh checkouts become usable without repeated setup prompts | Medium |
| 2 | Durable agent prompt queue | Follow-ups survive disconnection and wait for safe delivery | Large |
| 3 | Workspace review pane | Review changes across agents without losing progress | Medium |
| 4 | Provider usage and quota overview | Choose providers with useful capacity and recognize stale measurements | Medium, with provider integration uncertainty |

Effort describes the proposed first releases, not calendar estimates. The queue
has the largest correctness burden. Review and usage plugins can ship
independently of it.

## Scope and existing capabilities

The comparison uses the local `~/bb` checkout at
[`31716896f46c4b8b475968be2306f8be4190a6e5`][bb-tree] and Yinz at
`2c0cafde215166ba3a2fd472728b8880cac0337e`. BB links below identify that inspected
revision. The review covered its runtime architecture, CLI and SDK surfaces,
provider bridges, environment lifecycle, queue persistence, Git review UI, and
plugin catalog. The four selections were traced into implementation and relevant
test code. Runtime behavior and performance remain unverified.

BB models work as threads attached to environments and execution hosts. Its
server owns product policy and its host daemon executes local operations.
Yinz already provides the corresponding essentials through sessions, panes,
agent control, and Steel plugins. Adopting BB's full thread runtime would add
substantial overlap. [BB architecture][bb-architecture]

Several apparent opportunities already exist in Yinz:

- [Agent control](../AGENT_CLI.md) includes worktree creation, conversation
  guards, submission acknowledgements, correlated outcomes, hidden agents,
  lifecycle subscriptions, and searchable conversation and tool archives.
- [Remote sessions](../REMOTE_SESSIONS.md) and SSH panes already support work
  on another machine.
- [Repository plugins](../GIT_PLUGINS.md), [plugin storage and asynchronous
  commands](../PLUGIN_SDK.md), and the Agents panel supply extension points.
  `GIT_PLUGINS.md` concerns loading plugins from Git repositories. It does not
  describe a workspace diff viewer.
- [Notes and todos were removed](../UPGRADING_PROJECT_TOOLS.md). Commit
  `62db4500a` explicitly describes moving that work to Agenda and a plugin.
  [Swamp integration was also removed](../../README.md). Neither should return
  incidentally through this adoption plan.

## 1 Worktree preparation before agent startup

**What to adopt.** BB runs `.bb-env-setup.sh` in a newly provisioned workspace
before starting the agent. It streams output, closes stdin, and fails startup
when setup fails or times out. Its worktree provider also supports
`.worktreeinclude` for selected local files. The useful principle is that a
workspace must be prepared before agent work begins.
[Worktree contract][bb-worktrees], [script runner][bb-setup]

**Why this improves Yinz.** Yinz creates or reuses the requested checkout and
then prepares the agent command. The shared
[`prepare_agent`](../../yinz-plugin-host/src/yinz_exports/agent_launch.rs)
path has no project setup phase. A new agent therefore receives a valid Git
checkout that can still lack dependencies or generated development files.
A project setup script would remove repetitive instructions such as
“install the dependencies before starting.” It would also make setup failures
visible before they consume an agent turn.

**Proposed first release.** Add an explicit preparation option to CLI launches,
the launch JSON, the New Agent dialog, and the plugin spawn spec. The proposed
CLI spelling is `--prepare`. It runs one conventional, tracked script,
`.yinz/worktree-setup.sh`, from the newly created worktree. These are proposed
interfaces, not existing commands.

1. Validate preparation support and the requested execution target before Git
   changes. Initially support local worktrees. Reject preparation for SSH or
   Bubblewrap launches until execution in those targets is implemented.
2. Record whether this operation actually created a worktree. The existing
   `reused_worktree: false` also covers other checkout modes, so it is not
   sufficient evidence. Never run setup merely because a pane starts in an
   existing directory.
3. Run the script outside the screen thread, with an explicit interpreter,
   target cwd, closed stdin, bounded retained output, and a deadline. Show
   progress in the launch UI and expose the same operation status to CLI and
   SDK callers. Start the agent only after a confirmed successful exit.
4. On failure or cancellation, keep the checkout, branch, log, and operation
   identity. Offer an explicit retry against that same checkout. Persist the
   setup outcome so restart cannot silently bypass an unfinished setup or
   rerun an operation whose result is unknown.

Script execution requires the explicit preparation choice and ordinary command
permissions. Reading project configuration must remain free of execution.
Do not run repository scripts automatically just because they exist.
Cancellation must stop and verify the owned process group. Cancelling an SDK
deferred result alone does not stop a host command, as the
[SDK contract](../PLUGIN_SDK.md#permissions-and-asynchronous-work) explains.

Keep the first release to setup. Defer `.worktreeinclude`, teardown hooks, and
a general environment-provider framework. Setup must respect Yinz's isolated
build rule: do not share or copy mutable `target/` artifacts between worktrees.
Preserve the existing clean-and-merged checks in
[`agent-exit.scm`](../../yinz-steel-builtins/assets/plugins/agent-exit.scm).
BB's automatic, forceful worktree removal is unsuitable for this adoption.

**Acceptance checks.** A marker-producing setup runs before the agent, exactly
once for a successful preparation. Existing worktrees remain untouched. A
nonzero exit, timeout, cancellation, or restart during setup prevents agent
startup and leaves inspectable resources. Concurrent launches cannot adopt an
unfinished preparation. BB's [script tests][bb-setup-tests] provide useful
cases for output streaming, stdin EOF, timeouts, and cancellation.

## 2 Durable agent prompt queue

**What to adopt.** BB persists queued messages and shows why each is waiting:
busy thread, provisioning, interaction, offline host, schedule, or plugin hold.
Messages can be inspected, edited, cancelled, and dispatched through the same
service used by the UI and CLI. Database claims coordinate dispatch.
[Messaging contract][bb-messaging], [queue storage][bb-queue-db],
[dispatch service][bb-queue-dispatch]

**Why this improves Yinz.** A user should be able to assign “review the changes
after this task” and disconnect. Today,
[`AgentRequest::Prompt`](../../yinz-types/src/agent/control.rs) submits to a live
agent, while the [screen controller](../../yinz-screen/src/screen/agent_control.rs)
rejects busy agents and retains only 32 recent submissions per conversation in
memory. Archives preserve history, but they do not provide a durable list of
future prompts. Controllers must currently own that list and its recovery.

**Proposed first release.** Add FIFO follow-ups for already identified, managed
agent conversations. Expose add, list, edit, cancel, pause, and resume through
`agent queue`, the plugin API, and a small queue view in the Agents panel.
Require explicit target identity. Queue acceptance means “saved,” not
“delivered” or “completed.” Defer scheduled sending, recurring work, automatic
provider retries, and steering active turns.

1. Store prompt text and order, target identity, a submission ID, dispatch
   state, and observed outcome in the existing plugin database. Scope server
   ownership by both `host_id` and `yinz_session_name`. Use the revision read
   by the editor or dispatcher for guarded writes and claims.
2. Use existing lifecycle observations to wake a single dispatcher for the
   owning session. Recheck readiness in the host controller before delivery.
   Keep database work outside the screen thread and use the guarded host
   control path, not raw paste helpers.
3. Bind entries to the provider, conversation, pane generation, and server
   epoch. Pane numbers alone are reusable. A changed conversation, pane
   replacement, or server restart pauses affected entries for explicit
   reconciliation. Resuming requires an explicit new binding when identity
   changed, rather than silently refreshing the saved guards.
4. Preserve the input the user types after enqueueing. Current
   [`agent prompt` replaces idle drafts](../AGENT_CLI.md#automatic-draft-replacement),
   and `preserve_draft` is ignored. Add an atomic conditional dispatch mode
   that checks identity and input revision and refuses unverified or nonempty
   input. Disable automatic delivery for providers or layouts that cannot
   supply this evidence. Keep existing explicit-prompt behavior. A separate
   status check followed by today's prompt call would still race with typing.
5. Persist a dispatch attempt before sending. Record acknowledgement and the
   correlated turn outcome separately. Advance after confirmed completion
   and readiness. Pause on failed, interrupted, or uncertain outcomes.
   Unknown delivery must never trigger automatic resubmission.

The existing [event stream](../AGENT_CLI.md#startup-replay-and-loss) has bounded,
in-memory replay. A gap requires reconciliation. Submission IDs are also
bounded and are not durable idempotency keys. Retain uncertain queue entries
for inspection when acknowledgement cannot be recovered. This design offers
durable intent with conservative recovery, not an exactly-once promise.
An explicit pause or user interruption must survive dispatcher restart.

**Acceptance checks.** Two dispatchers cannot claim one entry. Editing or
cancelling competes safely with dispatch. Human typing, an approval dialog,
pane reuse, and conversation replacement prevent unintended delivery. Inject
crashes before sending, after sending, and after acknowledgement. Never lose
the prompt or automatically repeat an uncertain attempt. Verify host/session
isolation and user pause behavior. BB's [queue recovery and edit tests][bb-queue-tests]
are useful examples, but its stale-claim recovery cannot simply be copied into
Yinz's terminal input protocol.

## 3 Workspace review pane with content-aware review marks

**What to adopt.** BB's Git plugin supplies status, changed files, and patches
through a common UI/CLI service. Its diff viewer records a file as Viewed for
a particular content revision. When that revision changes, the file becomes
unreviewed. Large patches load on demand, and refresh work is coalesced.
[Git service][bb-git], [revision calculation][bb-git-reads],
[Viewed UI behavior][bb-viewed-ui], [patch limits][bb-diff-limits]

**Why this improves Yinz.** The Agents panel exposes activity and history.
The [Git probe](../../default-plugins/project-data/src/git_probe.rs) identifies
repositories, worktrees, branches, and remotes. Neither is an integrated review
of the current changes. A review pane would let a user inspect one agent's
work, switch to another, and return knowing which files changed after review.
That is more useful than a plain diff launcher when several agents are active.

**Proposed first release.** Build an optional Steel review plugin with a file
list, unified patch view, a Viewed action, and “open in editor.” Start with
uncommitted changes and all changes against a selected merge base. Identify
the actual checkout on the pane's execution host. Initially support checkouts
local to the session server, including a remotely attached Yinz session.
Report unsupported SSH pane targets explicitly rather than examining a local
path with the same name.

1. Reuse project resolution, Git probes, command execution, and terminal UI
   widgets. Expose the same status/files/patch/viewed operations to scripts
   through the CLI and plugin API. Keep the Git implementation outside the
   terminal renderer.
2. Resolve and display the selected base ref and commit. Handle missing
   bases, detached HEAD, and repositories without an initial commit explicitly.
   Run Git with argv arguments and bounded output. Load the selected patch
   lazily, with clear binary, missing-file, and truncation states.
3. Store Viewed marks by host, canonical checkout identity, comparison
   target, path, and content revision. Include both sides and file modes in
   the revision. Mark the revision actually displayed, not a newly fetched
   version the user has not read. A content or base change invalidates the
   mark. An unavailable revision cannot establish that a file is reviewed.
4. Refresh when the pane opens or the user requests it. Add coalesced refresh
   on relevant changes while visible, with shared reads per checkout.
   Retain keyboard navigation, focus, and scroll position across refreshes.
   Show file status and review state with text as well as color.

Keep staging, committing, merging, GitHub mutations, and a full editor outside
the first release. Existing tools already perform those operations. The new
value is persistent review progress beside the agent, with a consistent
machine-readable view.

**Acceptance checks.** Review marks survive reopening and become stale after
edits or base changes. Identical relative paths in separate worktrees never
share marks. Cover rename, deletion, binary, symlink, untracked, and unborn
repository cases. Diff reads must not mutate the real Git index or checkout.
A large repository must not block pane input or require every patch to load.
BB's [workspace diff tests][bb-diff-tests] and
[refresh tests][bb-git-refresh-tests] cover useful edge cases.

## 4 Provider usage and quota overview

**Implementation update (2026-10-26).** The first slice is in the existing
Agents panel: local Codex and Claude account quotas, reset times, visible-panel
refresh, and stale/error states. Codex uses its app-server API; Claude uses its
local OAuth credentials. See [panel behavior](../AGENT_CLI.md#provider-quotas-in-the-agents-panel)
and the [plugin request contract](../PLUGIN_PROTOCOL.md). Remote account
collection, a separate usage plugin, and a CLI usage command remain outside
this implementation.

**What to adopt.** BB separates cheap resource discovery from collecting usage
for a selected account. Measurements include observation time, usage windows,
reset times, and explicit authentication or collection errors. Refresh failure
retains the last measurement. Known provider account IDs support deduplication
without summing the same quota across panes or machines.
[Usage contract][bb-usage-contract], [aggregation and cache][bb-usage-service]

**Why this improves Yinz.** Yinz's
[`AgentInfo`](../../yinz-types/src/agent.rs) and
[`AgentProvider`](../../yinz-types/src/agent/provider.rs) describe activity and
launch capabilities. They do not provide an account usage overview. When users
run several providers, a compact usage view would help them choose where to
start the next task and explain why capacity is unavailable. Account quotas
must remain distinct from an individual conversation's context usage or cost.

**Proposed first release.** Add an optional Steel usage plugin, accessible from
the command palette and New Agent dialog, plus a JSON read command. Start with
local Codex and Claude sources where their installed versions provide a
validated collection path. Show provider/account, usage windows, reset time,
last successful observation, and refresh status. Display unknown or unsupported
data explicitly.

1. Define a small measurement contract and keep provider collection behind
   optional adapters. UI reads use a cache. Explicit refresh and visible-panel
   polling share an in-flight request and back off on failure.
2. Keep credentials on the execution host. Expose normalized measurements,
   never tokens or credential files, to plugins and shared storage. Do not
   infer a remote pane's account from the local machine's login.
3. Deduplicate only when a provider supplies a trustworthy account identity.
   Unknown identities stay separate. Retain raw percentages for inspection,
   even if a visual bar caps its width. No limit reported does not mean zero
   use, and a throttled measurement request does not prove exhausted quota.
4. Refresh asynchronously without changing agent readiness, blocking launch,
   selecting a different account, or retrying work. Keep account pooling and
   automatic model routing outside this feature.

The integration risk is concrete: BB's inspected
[Codex collector][bb-codex-usage] reads local authentication and calls a
ChatGPT backend usage endpoint. Its [Claude collector][bb-claude-usage] reads
local credentials and calls an OAuth usage endpoint. These implementations do
not establish a stable public API contract. Begin with a compatibility spike
against installed providers, prefer supported native interfaces when available,
and keep any credential-based fallback explicit and replaceable. Do not ship
a dashboard that guesses when a source is unsupported.

**Acceptance checks.** Test valid, malformed, expired, unsupported, offline,
throttled, and stale responses. Failed refreshes retain the original observation
time. Multiple panes on one account do not multiply usage. Concurrent readers
share collection, and hidden views stop polling. Test adapters with captured,
sanitized fixtures and verify supported versions in a live smoke test.
BB's [usage tests][bb-usage-tests] demonstrate selected-source collection and
preservation of prior measurements.

## Implementation sequence and shared requirements

Start worktree preparation in the shared launch path, then ship the review
plugin. Develop the queue's persistence and conditional-dispatch contract
before adding its UI. Run the usage compatibility spike early, but keep its
release independent of the other three features.

| Feature | Main Yinz integration points |
| --- | --- |
| Preparation | [`agent_launch.rs`](../../yinz-plugin-host/src/yinz_exports/agent_launch.rs), [`AgentSpawnSpec`](../../yinz-types/src/agent.rs), [`new-agent.scm`](../../yinz-steel-builtins/assets/plugins/new-agent.scm), process supervision and launch results |
| Queue | [`src/agent.rs`](../../src/agent.rs), [`CLI arguments`](../../yinz-cli/src/args.rs), [`agent control types`](../../yinz-types/src/agent/control.rs), [`screen controller`](../../yinz-screen/src/screen/agent_control.rs), [`event stream`](../../yinz-screen/src/screen/agent_events.rs), [`plugin storage`](../../yinz-storage/src/plugin_data_store/mod.rs), Agents panel |
| Review | Existing project/Git probes, optional Steel plugin, [`asynchronous commands and UI`](../PLUGIN_SDK.md), plugin data for Viewed records |
| Usage | Provider-specific collectors, optional Steel plugin, existing command palette and New Agent UI, a normalized read interface |

Every user operation needs a CLI and plugin surface using the same underlying
implementation. New wire operations must negotiate support with older servers.
Update help, `docs/AGENT_CLI.md`, `docs/PLUGIN_SDK.md`, portable skills, and
intentional snapshots with the relevant release. Read the
[Steel instructions](../../yinz-plugin-host/src/steel/CLAUDE.md) before SDK
changes. Regenerate protobufs with `cargo x proto` when their sources change.

Use the existing SQLite store and guarded revisions. Server-owned rows require
strict host/session ownership. Persistent feature data must not disappear when
a panel closes or a plugin reloads. Do not reuse retired chat schedules, notes,
or todo tables as execution state.

For implementation, add focused tests for the failure cases above, then run the
required full unit and integration suites for shared API, plugin, and screen
changes. Build this worktree's own binary and run `cargo x ci check` before
branch handoff. Use 16 integration workers, socket-suite sandbox escalation,
and condition waits. Keep a real, isolated `target/` and never set
`CARGO_TARGET_DIR`. Run the Docker remote suite when remote spawning changes.

## Features considered but not selected

| BB capability | Reason to leave it outside these four |
| --- | --- |
| Shared memory and task boards | BB's bounded memory catalog and versioned writes are useful, but Yinz deliberately moved notes/todos out. Revisit through the external integration rather than introducing another knowledge store. [Memory implementation][bb-memory] |
| Automations and durable workflows | Valuable for unattended work, but they add scheduling and execution ownership shortly after Swamp's removal. Establish durable prompt delivery first. [Automations][bb-automations] |
| Agent concurrency limits | BB can hold new turns before dispatch. Yinz also hosts agents launched and controlled directly in terminals, so a turn limit would cover only managed submissions. Revisit after the queue has a clear admission boundary. [Concurrency plugin][bb-concurrency] |
| GitHub issue and PR dashboards | Useful, but provider-specific and dependent on authenticated network reads. The local review pane benefits every repository first. [GitHub plugin][bb-github] |
| Web/mobile clients and cloud environments | Much larger deployment, authentication, and UI scope. Yinz already has SSH session access. |
| Another transcript viewer or replacement chat runtime | Yinz already has searchable archives and external provider UIs. The chosen features add capabilities around those interfaces. |

[bb-tree]: https://github.com/get-bb/bb/tree/31716896f46c4b8b475968be2306f8be4190a6e5
[bb-architecture]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/docs/system-overview.md
[bb-worktrees]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/docs/worktrees.md
[bb-setup]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/apps/host-daemon/src/environment-lifecycle-script.ts
[bb-setup-tests]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/apps/host-daemon/src/environment-lifecycle-script.test.ts
[bb-messaging]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/packages/templates/src/templates/bb-guide-thread-messaging.md
[bb-queue-db]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/packages/db/src/data/queued-thread-messages.ts
[bb-queue-dispatch]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/apps/server/src/services/threads/queued-messages.ts
[bb-queue-tests]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/apps/server/test/threads/requested-queue-drain.test.ts
[bb-git]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/plugins/git/README.md
[bb-git-reads]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/plugins/git/host/workspace-reads.ts
[bb-viewed-ui]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/apps/app/src/components/secondary-panel/git-diff/DiffFilesPanel.tsx
[bb-diff-limits]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/plugins/git/host/diff-tiering.ts
[bb-diff-tests]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/plugins/git/host/workspace-diff.test.ts
[bb-git-refresh-tests]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/plugins/git/server/invalidation.test.ts
[bb-usage-contract]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/plugins/provider-usage/usage-source-contract.ts
[bb-usage-service]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/plugins/provider-usage/server.ts
[bb-codex-usage]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/plugins/provider-codex/src/bridge/provider-maintenance.ts
[bb-claude-usage]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/plugins/provider-claude-code/src/bridge/provider-maintenance.ts
[bb-usage-tests]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/plugins/provider-usage/server.test.ts
[bb-memory]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/plugins/memory/server.ts
[bb-automations]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/plugins/automations/PLUGIN_OVERVIEW.md
[bb-concurrency]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/plugins/concurrency-limit/server.ts
[bb-github]: https://github.com/get-bb/bb/blob/31716896f46c4b8b475968be2306f8be4190a6e5/plugins/github/PLUGIN_OVERVIEW.md
