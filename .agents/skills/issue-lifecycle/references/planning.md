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
