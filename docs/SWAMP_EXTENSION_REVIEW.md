# Local Swamp extension review

Package: `@hammz/zellij`, version `2026.09.06.1`. Local use only.

The user acknowledged this review before extension execution and smoke tests on
2026-09-06. The acceptance findings below are now resolved.

Mechanical review:

- PASS — resource writes parse their declared schemas; claims, questions, operations, review, test, assessment and Git evidence use distinct resource specifications.
- PASS — missing review findings are explicitly invalid and unresolved. Missing test evidence is shown as skipped and requires informed merge approval.
- PASS — durable claim instances use operation keys; question instances use question keys within a work item. Other outputs are versioned snapshots.
- PASS — optional operation fields have corresponding host paths. The view labels its 100-line workflow-log window; it does not claim to contain the complete command log.

Universal dimensions:

- Credentials and secrets: PASS — no new credential storage or credential copies. Agent credential handling stays in the existing host provider.
- Logging quality: PASS — method execution and outcome logs, supervised pane identities, structured evidence and failure reports are available.
- Error handling: PASS — invalid evidence refuses integration. Dispatch claims and recorded answers deliberately precede mutation, as required by the ownership contract.
- Testing completeness: PASS — extension tests cover literal arguments, typed evidence, Git safeguards, all question kinds, answer recovery and reports. The real-server example covers unattended suspension, an interactive view, explicit repair approval, stale targets, cancellation and restart. Real Firecracker tests cover provisioning, sync and conflicts; deterministic provider failures have unit coverage.
- Idempotency and resilience: PASS — conflicting dispatch keys fail; uncertain dispatches cannot repeat automatically. Answer recovery uses the recorded submission and command claim.
- API contracts: PASS — local CLI arguments are literal arrays and bridge results have typed schemas. No HTTP integration is introduced.
- Resource management: PASS — kernel checkout locks close in finally blocks. Managed agent and command exits retain resources. Cleanup requires approval, exact source revision, clean files, target ancestry and live VM inspection.
- Published surface hygiene: PASS — local package, no publish, push, fetch, or PR steps. Test repositories use placeholder identities.

Model dimensions:

- Schema strictness: PASS — explicit Zod object schemas, no passthrough.
- Lifetime and GC: PASS — ownership claims and questions have infinite lifetime; resource versions have explicit retention.
- CRUD completeness: not applicable — delivery and Git operation adapters extend existing host and official Git implementations.
- Preflight checks: PASS — checkout identity, clean state, conflicts, evidence, ownership and approval checks run in the mutating method; final verification and integration share the target lock.
- Instance names: PASS — separate work items and review/gate operations have separate model names; compatible snapshots share a versioned name.
- Data access: PASS — methods read saved resources with readResource; workflow connections use data.latest CEL. Reports read the exact saved handle versions and include persisted resources from earlier invocations of a suspended run.
- Version upgrades: not applicable — initial local package; existing inspect/action/pipe methods are retained.

Workflow acceptance findings:

- RESOLVED — changing the target after review invalidates the evidence, returns to preparation and requires fresh approval. The smoke test proves the first approval does not integrate stale work.
- RESOLVED — reports include findings, revisions, gate outcomes, decisions and resource identities across suspension/resume. The interactive terminal test renders a pending question, closes and reopens its view, and preserves the pending run. Command output is verified against the supervisor and process, including after a pane number changes; bounded scrollback is explicitly marked.
- RESOLVED — the disposable example completes integration and approved retention, then separately verifies failed-test choices and explicit repair, recorded-answer recovery, duplicate refusal, dispatcher reload, cancellation and server crash/adoption. A verified adopted agent accepts a new prompt and reports its process exit without launching another exit workflow. The Firecracker example exercises typed guest spawn, explicit completion, host sync and refused conflicting sync.

Validation commands:

```sh
deno fmt --check extensions
deno lint extensions
deno test --allow-all extensions
python3 extensions/tests/smoke.py
cargo x ci check
cargo x firecracker-test --test
swamp extension push extensions/manifest.yaml --dry-run --json
```

The full gate covers 7,690 tests and 368 integration tests. The Firecracker suite has
four real-VM scenarios, including the project workflow bridge. No protobuf schema
changed; the optional supervisor ownership field remains backward compatible with
existing JSON frames. Local extension bundling is a dry run; nothing is published.

All smoke mutations and approvals use disposable repositories. No merge or cleanup of
the development checkout is authorized by this review.

## Repeatable local validation update

Fresh launches use flow version 3 and one manual action menu. Earlier suspended decision flows require a fresh launch.
Review covers committed, staged, unstaged, and untracked changes. Repairs receive complete saved findings or failure reports.
Every helper receives its startup prompt and must exit before the next decision.
All five role specifications survive continuations without inheriting omitted model or effort fields.

Integration now requires current passing tests and a valid review without warning or error findings, including for direct model calls.
Exact revision approval, local fast-forward integration, VM checks, checkout locks, and guarded non-force removal remain required.
No remote Git operations were added.

Step cancellation retains partial output, checks owned process identities, interrupts the process group, and escalates after five seconds.
Unknown termination blocks new execution. Git inspection preserves completed mutations and paused rebases without implying rollback.
The existing extension, bridge, SDK, and real-server smoke tests cover these paths.
