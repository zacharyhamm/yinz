# Plan review standards

Inspect source and requirements, not just the plan's prose. Review correctness,
scope and unnecessary complexity. For revisions, compare the previous plan,
findings, planner responses, controller dispositions and current plan. Verify
claimed fixes and look for new defects while preserving supported tradeoffs.

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
finding with a supported `resolution`, or omit it once verified fixed. Neither
history nor stale review evidence can approve a new plan.

The controller owns adjudication. Workers supply evidence and disagreements;
they do not invoke `adjudicate-plan` or approve implementation. The controller may
resolve, reject or downgrade a finding only after inspecting its cited code and
explaining its judgment. Accepting an unresolved high/critical risk is user
direction, not a resolution, and must remain blocked. Keep the original review
and record dispositions separately against its exact fingerprint.
