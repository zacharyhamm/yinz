# Planning standards

Read the issue requirements, investigation, exact user assignment, and recorded
planning context before planning. Inspect relevant source. State the intended
behavior, affected files, concrete steps, risks, assumptions and focused checks.
Keep scope proportional to the requirement; avoid unnecessary abstractions,
configuration, migrations and unrelated improvements.

Use the applicable [Yinz review dimensions](dimensions.md) to check proposed
behavior, affected callers and verification coverage against repository rules.

For a revision, compare the previous plan with its review and applicable
controller dispositions. Preserve unaffected steps, step IDs and accepted
tradeoffs. Change only what the findings or new user instructions justify.
Return the whole updated plan plus `revision` containing:

- `summary`: what changed and why, including scope changes.
- `responses`: one response to each blocking finding, retaining its `findingId`.
  Use `status` of `addressed`, `disputed`, or `unresolved`, and provide `rationale`
  and nonempty `evidence` citing source, requirements or checked behavior.
- `acceptedDecisions`: the decisions and tradeoffs being retained.

A disputed finding needs counterevidence; disagreement alone is insufficient.
An unresolved finding stays visible. Do not fabricate a fix, silently broaden
requirements, or treat the revision request as implementation approval. Historical
reviews help reasoning but do not constitute current review or approval evidence.
Submit the role evidence through the exact Swamp evidence behavior and schema in
the saved assignment. Inspect its matching accepted receipt before finishing.
Final chat text is presentation and is never lifecycle evidence. Correct rejected
submissions in the same worker using the investigation already performed. For an
older assignment, the controller supplies a submission-only continuation; do not
restart the investigation or treat its old final-message contract as authority.

## Review repair continuity

Contract-2 repair assignments include the exact selected findings and their full
origin identities, original evidence, ignored concerns, scoped planning decisions
and controller/user authorizations, approved checks, previous repair responses,
failure log references and captured Git endpoints. Use this packet as the source
of repair obligations; preserve unaffected decisions. Worker-proposed tradeoffs
are distinct from controller adjudications and user approval.

A repair plan returns `findingMappings`, exactly one per selected finding, mapping
its full identity to known repair `steps` and `checks`. Checks must cover the
mapped steps. Unknown, duplicate or omitted findings/steps/checks are rejected.
Record newly necessary scope in `scopeChanges` and proposed tradeoffs separately
in `proposedTradeoffs`. Selection authorizes planning; the existing separate
repair-plan approval authorizes implementation of the resulting scope.

A repair implementation returns exactly one `responses` entry for each selected
finding: `addressed`, `disputed` or `unresolved`, rationale, affected repository
paths, source/check evidence and relevant approved check IDs. Disputes and partial
repairs remain visible. Newly discovered implementation scope changes require a
revised plan and approval; do not broaden implementation under the old approval.
Implementation claims never close findings. Both fresh independent reviews and
approved deterministic verification still follow every repair.
