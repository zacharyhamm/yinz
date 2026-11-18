# Shared review standards

Inspect source and requirements, not just the plan's prose. Review correctness,
scope and unnecessary complexity. For revisions, compare the previous plan,
findings, planner responses, controller dispositions and current plan. Verify
claimed fixes and look for new defects while preserving supported tradeoffs.

For plan, technical code and UX review, read the [Yinz review dimensions](dimensions.md) and
apply those relevant to the change. Findings may cite a dimension name in
`description` and its authoritative source in `evidence`, alongside concrete
affected code or plan locations. Use the existing finding fields, without a
`category` property. Dimension citations do not replace evidence, severity,
finding identity, disposition or reopening requirements.

A high or critical blocker must identify an affected `requirement`, a concrete
failure `mechanism`, and nonempty source `evidence` with paths and relevant code
locations. Explain the trigger and consequence. Missing implementation detail,
style preferences, speculative concerns and unrelated improvements do not alone
justify high severity. Use medium or low for nonblocking improvements. Remove
unsupported findings rather than escalating uncertainty into a blocker.

Reuse a finding's ID when the same concern persists; don't rename it to avoid its
history. Retain valid resolved or rejected decisions. Reopening a disposed or
previously fixed concern requires `reopening: {rationale, evidence}`: cite new
evidence and explain why the previous disposition no longer holds. A planner's
claim of a fix is not proof; inspect the relevant code or plan change. Retain a
finding with a supported `resolution`, or omit it once verified fixed. Leave
`resolution` unset for a newly reported defect. A proposed repair belongs in the
description and does not resolve the concern. Neither
history nor stale review evidence can approve a new plan.

The controller owns adjudication. Workers supply evidence and disagreements;
they do not invoke `adjudicate-plan` or approve implementation. The controller may
resolve, reject or downgrade a finding only after inspecting its cited code and
explaining its judgment. Accepting an unresolved high/critical risk is user
direction, not a resolution. Plan blockers remain blocked; implementation review
findings may be accepted for merge by the user. Keep the original review
and record dispositions separately against its exact fingerprint.

Start from the evidence template in the assignment directory and submit with the
assignment's `submit_evidence.py` command; it exits 0 only after printing the
accepted receipt (`--status` after an interrupted run). Older assignments without
assignment files use the Swamp evidence behavior and commands in their saved
prompt. Final chat formatting is presentation. Correct submission errors using
the same investigation; do not repeat the review. Workers may submit only their
own assignment's evidence and inspect its receipt.


## Implementation review ownership

Technical review independently assesses requirements and behavior, including
incorrect assumptions in an approved design. Compare the committed change with
local main, trace affected callers, state transitions, failure/cleanup paths,
ownership, concurrency and platforms as applicable. Inspect missing coverage and
assertions that can pass with incorrect behavior. Also account for each approved
plan step exactly once and justify deviations; conformance alone is insufficient.

The separate `review_ux` worker reviews the same full commit and plan version.
Identify affected users/tasks and trace discovery, normal/repeat use, waiting,
failure, cancellation and recovery. Inspect relevant terminal UI, keys/focus,
readability, CLI/output, defaults/configuration, help/docs and lifecycle handoffs.
Include indirect responsiveness, interaction burden, compatibility and loss of
work. Can users understand what happened, what is happening and what to do next?
Keep implementation details out of user flows unless they support a meaningful
decision. Exercise representative interactions when practical; distinguish
observations from source-derived conclusions and state material limitations.

UX findings identify the user/task, interaction trigger, behavior, consequence,
severity and concrete evidence using the shared finding fields. Aesthetic
preferences, speculative concerns and unrelated redesigns are not blockers.
An empty findings array is valid after a real review. Always record an assessment:
`impact` (`material` or `no_material_impact`), nonempty `summary` rationale,
nonempty `assessedPaths` describing user tasks/workflows and source paths, and
`limitations` (possibly empty). No-impact conclusions require tracing affected
paths; absence of UI-file changes is insufficient.

Technical review comes first; UX review runs once code findings are settled, and
both must be current at merge. Present the current reviews' pending findings and
let the user choose which to repair or ignore. Medium or higher findings need
that decision; low findings are recorded as follow-ups and start no repair round
(the user may still select one). Ignored findings remain unresolved, with user
decisions recorded separately against the exact reviews. Users may explicitly
authorize merge with outstanding findings. Selected repairs go straight to a
repair worker; repairs require fresh, delta-scoped technical and UX reviews and
verification. No reviewer or controller may infer permission to fix or ignore a
finding; only the user's own standing `drive --auto-repair` instruction lets
drive select every blocking finding for repair. Checks actually performed by either
reviewer remain advisory for structured plans; the lifecycle runs approved checks.
Follow AGENTS.md's focused-test policy; review never authorizes full suites.
Plan checks are valid command argument arrays executed directly without a shell;
the [check policy](planning.md#check-policy) enforces argv form and focused suite
selection at submission, so reviewers judge coverage of changed behavior and
affected callers.
See the bounded [reviewer evaluation](review-evaluation.md) for measured outcomes
and limits; fixture reviews are not approval of a real implementation.

## Durable implementation review continuity (assignment contract 2)

New assignments carry a versioned `continuity` binding and a complete deterministic
review packet. Contract 3 delivers them as read-only files bound by the
assignment manifest: `packet.json`, `snapshot.json` (which packet `sourceRef`
paths resolve against), `baseline.json`, and the decoded `feature.diff` and
`delta.diff`. Contract-2 prompts inline the packet and base64 diffs. Read the
saved packet, source reviews and referenced failure logs.
The captured `baseline.main`, `mergeBase` and `head` are full object IDs. A full
review (`continuity.scope.kind` is `full` or absent) covers the feature diff
across the complete merge-base-to-HEAD scope; the delta is optional endpoint
assistance there. A delta re-review is bounded instead (see below). A moved local main never changes the technical/UX pair's
baseline. Divergent/rebased history is not a descendant repair. Missing previous
objects explicitly disable delta and reuse assistance; missing required current
objects block dispatch. Binary diffs are included without silent truncation.

Reference each prior finding by `{issue, role, reviewFingerprint, findingId}`.
Keep IDs and origins for persistent concerns; identical IDs in the two review
roles remain separate obligations. Cross-role links cite the other role and
never discharge its review. New findings include `affectedPaths`, source or
behavioral `evidence`, and relevant approved `checkIds` (an empty check list may
indicate a coverage gap).

Return `priorFindings` outcomes for every prior unresolved finding of your role
(the contract-3 evidence template pre-fills one entry per obligation):
`persists`, `verified_fixed`, or `reopened`, each with rationale and evidence.
For `persists`, also carry the same finding and its origin in `findings`, leaving
`resolution` unset. Describe an unchanged ignored concern as historical
continuity, not a newly discovered defect. Ignored concerns remain unresolved.
In a full review they need fresh user decisions against the exact current
reviews; in a delta re-review an unchanged `persists` report, with no
`reopening` and no higher severity, keeps the earlier ignore. Omission does not
establish closure. A repair
worker's `addressed` claim does not establish closure either. Reopening a
verified-fixed concern requires concrete new evidence and a rationale addressing
the prior decision, plus an explicit `reopened` outcome. Structural validation
checks completeness and references; reviewers remain responsible for evidence
quality and recognizing the same underlying concern under changed wording.

In a full review, independently inspect current requirements, approved
assumptions, affected callers, tests and failure paths across the complete
feature change, even when the delta is small. Incorrect approved designs,
incomplete selected repairs and newly introduced defects outside the selected
repairs remain reportable.

### Delta re-review

After a repair pass, the assignment's `continuity.scope` is
`{kind: "delta", base, paths, findings}`: `base` is the last commit your role
reviewed, `paths` the files the repair diff (`delta.diff`) changes, and
`findings` the prior findings you must account for. Verify each listed finding
with a `priorFindings` outcome. Report a new finding only for a regression the
repair diff introduced; its `affectedPaths` must include a changed path, or the
submission is rejected. Do not re-audit unchanged code or raise unrelated or
pre-existing concerns; the complete feature diff is context only. A delta code
review's template carries each plan step's status from the base review: change
one only if the repair changed it. The controller stamps the assigned `scope` on
the review; do not supply a different one. A rollback to `review_code` or
`review_ux` returns to a full review.

Historical archives inform review and never satisfy a current approval or review
gate.

Optional `advisoryEvidence` records original task/commit/requirements, argv/cwd,
result, durable output reference, relevant files/tests, platform/toolchain,
prerequisites, cleanliness and dependency scope. Reused evidence retains that
original provenance and names the current commit with an applicability rationale.
For a changed commit, document comparison of implementation, callers, tests,
fixtures, dependencies, build configuration, requirements and environment.
Patch identity alone is insufficient. Missing logs/objects, unknown dependencies,
dirty execution and failed/flaky results cannot support passing reuse. Free-text
`preliminaryChecks` remain available: never describe reused output as a fresh
execution. Advisory reuse never replaces approved verification or rebased merge CI.
