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

Submit findings through the assigned Swamp evidence behavior and inspect the
accepted receipt before ending the task. Final chat formatting is presentation.
Correct submission errors using the same investigation; do not repeat the review.
Workers may submit only their own assignment's evidence and inspect its receipt.


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

Both reviews must be current. Present their findings together and let the user
choose which to repair or ignore, regardless of severity. Ignored findings remain
unresolved, with user decisions recorded separately against the exact reviews.
Users may explicitly authorize merge with outstanding findings. Selected repairs
need a worker's repair plan and separate user approval before implementation;
repairs require fresh technical and UX reviews and verification. No reviewer or
controller may infer permission to fix or ignore a finding. Checks actually performed by either
reviewer remain advisory for structured plans; the lifecycle runs approved checks.
Follow AGENTS.md's focused-test policy; review never authorizes full suites.
Plan checks are valid command argument arrays executed directly without a shell,
using supported flags and covering changed behavior and affected callers.
See the bounded [reviewer evaluation](review-evaluation.md) for measured outcomes
and limits; fixture reviews are not approval of a real implementation.

## Durable implementation review continuity (assignment contract 2)

New assignments carry a versioned `continuity` binding and a complete deterministic
review packet. Read the saved packet, source reviews and referenced failure logs.
The captured `baseline.main`, `mergeBase` and `head` are full object IDs. Review
`featureDiff` across the complete merge-base-to-HEAD scope; `delta` is optional
endpoint assistance. A moved local main never changes the technical/UX pair's
baseline. Divergent/rebased history is not a descendant repair. Missing previous
objects explicitly disable delta and reuse assistance; missing required current
objects block dispatch. Binary diffs are included without silent truncation.

Reference each prior finding by `{issue, role, reviewFingerprint, findingId}`.
Keep IDs and origins for persistent concerns; identical IDs in the two review
roles remain separate obligations. Cross-role links cite the other role and
never discharge its review. New findings include `affectedPaths`, source or
behavioral `evidence`, and relevant approved `checkIds` (an empty check list may
indicate a coverage gap).

Return `priorFindings` outcomes for every prior unresolved finding of your role:
`persists`, `verified_fixed`, or `reopened`, each with rationale and evidence.
For `persists`, also carry the same finding and its origin in `findings`, leaving
`resolution` unset. Describe an unchanged ignored concern as historical
continuity, not a newly discovered defect. Ignored concerns remain unresolved and
need fresh user decisions against the exact current review pair; a previous
acceptance never renews itself. Omission does not establish closure. A repair
worker's `addressed` claim does not establish closure either. Reopening a
verified-fixed concern requires concrete new evidence and a rationale addressing
the prior decision, plus an explicit `reopened` outcome. Structural validation
checks completeness and references; reviewers remain responsible for evidence
quality and recognizing the same underlying concern under changed wording.

Independently inspect current requirements, approved assumptions, affected callers,
tests and failure paths across the complete feature change, even when the delta
is small. Incorrect approved designs, incomplete selected repairs and newly
introduced defects outside the selected repairs remain reportable. Historical
archives inform review and never satisfy a current approval or review gate.

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
