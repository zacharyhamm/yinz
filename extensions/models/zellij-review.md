# Local Zellij extension review

The original inspection-only review below is retained for history. The complete
workflow extension review is in
[SWAMP_EXTENSION_REVIEW.md](../../docs/SWAMP_EXTENSION_REVIEW.md).

Reviewed: 2026-09-06. Scope: `zellij.ts`, `zellij_test.ts`, and
`zellij-inspect`. No blocking issues found. User acknowledged the review on
2026-09-06. Unit test passed (1/1). The live inspection workflow succeeded,
including its CEL assertion, and saved 13 panes and 2 tabs from session
`unique-mouse`. Action and pipe behavior has mock coverage; live mutation and
plugin reply checks were not run.

| Dimension                   | Finding                                                                                                                                                              |
| --------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Schema/write conformance    | PASS: state writes session, panes, tabs; result writes session, stdout, stderr, exitCode. Each matches its declared schema.                                          |
| Truncation honesty          | PASS: no pagination or result cap. Pane and tab fields are an explicit subset; other CLI fields are omitted.                                                         |
| Instance names              | PASS: state, action, and pipe have distinct names; action and pipe share the same result schema.                                                                     |
| Schema field coverage       | PASS: all fields are populated from the configured session or CLI output; optional pane fields are populated when returned by Zellij.                                |
| Credentials and secrets     | PASS: no credentials required. Arguments are not logged. CLI output is stored as model data, so avoid sending secrets through this model.                            |
| Logging                     | PASS: command entry and completion are logged without payloads.                                                                                                      |
| Error handling              | PASS: nonzero exit status, cancellation, invalid JSON, and invalid snapshot schemas fail before resource writes.                                                     |
| Testing completeness        | PASS: literal argv, pipe options and reply, snapshot schema, nonzero exit, invalid snapshot, and cancellation before spawn. Unit test and live inspection passed.    |
| Idempotency and resilience  | PASS: inspection is read-only; action and pipe methods make one attempt. Mutating actions are not idempotent; a timeout does not undo an action already delivered.   |
| CLI contract                | PASS: installed CLI help and live pane/tab metadata checked. Pipe payload uses the argument separator. CLI success does not establish plugin-level business success. |
| Resource management         | PASS: stdin is closed; subprocess output is collected with cancellation and a configurable timeout. Intended for finite CLI output, not event streams.               |
| Schema strictness           | PASS: named Zod fields allow CEL validation; no passthrough schema.                                                                                                  |
| Lifetime and GC             | PASS: both resources retain ten versions with a seven-day lifetime.                                                                                                  |
| Pre-flight checks           | PASS: action and pipe check that the named session responds. This is not a destructive-target approval: verify pane/tab IDs before destructive actions.              |
| Data access                 | PASS: workflow assertion reads the saved state using data.latest; no duplicate fetch. The two snapshot reads are not atomic.                                         |
| Published surface           | Not applicable: local extension, no publication requested.                                                                                                           |
| CRUD lifecycle and upgrades | Not applicable: CLI adapter and initial version.                                                                                                                     |
| HTTP contracts              | Not applicable: no HTTP API.                                                                                                                                         |

Completed checks:

```sh
~/.swamp/deno/deno test extensions/models/zellij_test.ts
swamp workflow run zellij-inspect
swamp model output get zellij-session --json
```
