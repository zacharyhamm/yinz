# Bounded reviewer evaluation

These are actual read-only reviewer conversations, separate from issue 72's
technical/UX approvals. They evaluate three deliberately small scenarios, not
general review reliability. `issue_review_test.ts` deterministically establishes
the scenarios and assignment contract; its passing assertions do not prove a
reviewer detects defects. A byte-identity test pins the evaluated fixture sources;
changing them requires repeating and recording the bounded live evaluation.

## Identity and method

Evaluation date: 2026-11-09 UTC (2026-11-08 America/Chicago).
Fixture commit: `16fd75cea7e259036683393c15c6c20f077fc999`.
Production prompt renderer commit: `ab9cff3ab774708a857d2ac4cd8f89a392185ef5`,
`extensions/behaviors/_lib/issue_agents.ts`, Git blob
`218be03cfb0b84651cd8b9383c4a3b53fb235dc6`. The working renderer used during the
conversations is byte-identical to that committed blob.

Each fresh conversation used the production `taskPrompt` renderer with a
fixture-only requirements snapshot, one approved `command` plan step, plan
version 1 and the full fixture commit. Only the submission text was replaced:
return an object matching `ActionSchemas.review_code` or `review_ux`, without
Swamp or real lifecycle mutation. Reviewers inspected the scoped `git show`
source; they were forbidden to read the seeded tests, other fixture modules or
this outcome reference. Neither expected findings nor canned results were given
to reviewers. No evaluation results were imported into real lifecycle state.

Workers ran in visible Yinz panes in session `sincere-cowbell`, stacked for the
implementation pane, through `yinz agent spawn`, guarded `prompt` and original
submission `wait`. Provider: Codex; displayed model/settings: GPT-6.1-Sol, xhigh.
Model, effort and permission flags were omitted to preserve provider defaults.
Prompts bounded each review to eight minutes, read-only interactions and no
subagents/background jobs. Observations used 45-second wait slices, retaining
the original acknowledged submission rather than resending. All final turns
completed in the foreground; all six evaluation panes were closed after results
were inspected. Omitted permission flags do not establish permission isolation;
read-only scope was an explicit assignment instruction.

| Case and source under `extensions/behaviors/unit/fixtures/issue_review/` | Supplied requirement | Approved approach |
| --- | --- | --- |
| Technical: `technical.ts` | Run every requested check in order; permit merging only when all succeed. | Initialize success, execute each check, assign its result to success, record indices, return both. The existing assertion checks execution count. |
| UX: `ux.ts` | Cancellation stops row intake immediately; accepted rows drain before settlement. Assess operators managing exports. | Call cancellation and acknowledge it in output; keep accepted rows draining, eventually settle and provide short help. |
| Control: `control.ts` | Preserve public greeting output and caller behavior for every input string. | Extract `name.trim()` into a private helper; compare supplied before/after functions. |

Final prompt SHA-256 identities:

| Case | Prompt digest | Conversation | Submission |
| --- | --- | --- | --- |
| Technical | `ffe84479d48134943d240f75671b04f31b67ff0c4ce4334a7814a8dcb497f07e` | `01a1bdd4-afcf-7602-96c8-a1fae45adf1d` | `issue72-eval-technical-2` |
| UX | `defa837a14e0ba59e81e650a7345113eb98119456f45c1871d8b7a24940ecc25` | `01a1bdd5-4236-7200-894a-da30fca4e33f` | `issue72-eval-ux-2` |
| Control | `39d36141a77a513c930217618a0642f89f67a58b56d4924e1ae0bf3d3d9137be` | `01a1bdd6-0bf0-7051-8ab4-5e9d6078d85b` | `issue72-eval-control-2` |

## Observed results

Technical review returned one **high**, unresolved finding,
`technical-aggregate-result`. It explicitly reported the plan step implemented
without deviation while explaining that the plan itself violates the requirement.
At `technical.ts:6`, assigning each result erases earlier failures; `:9` returns
only the last outcome. The reviewer exercised eight representative Boolean
sequences with an independent callback-order trace: `[false, true]` and
`[true, false, true]` incorrectly returned success. All callbacks executed in
order. `existingRegression()` at `:13-14` still returned true because it checks
only execution count. The consequence follows the supplied caller contract:
false success permits a merge despite a failed check. No real merge or external
caller was exercised. Advisory checks included the scoped diff, a passing module
type check and the failing behavior probe (exit 1, accurately reported).

UX review returned **material** impact and one **medium**, unresolved finding,
`ux-cancellation-premature-completion`. At `ux.ts:5-6`, cancellation enters
draining; `:16-18` immediately returns `Export cancelled`; help at `:21` omits
the waiting phase. Direct Deno interactions showed that initial and repeated
cancellation returned the same completion message with two rows still pending.
One tick left one row draining; the second settled at zero. Cancellation after
settlement and cancellation of a second job behaved correctly. The operator
cannot distinguish accepted cancellation from completed work through command
feedback. This is a demonstrated usability defect despite the tested API
contract working. The reviewer proposed clearer waiting/completion feedback and
help in its description, leaving `resolution` unset. Advisory checks included a
passing type check and interaction assertions; an initial unsupported data-URL
check failed with TS2307 and was honestly reported as a tooling invocation error.

Control review returned **no_material_impact**, explicit assessed paths and
limitations, and **zero findings**. It traced `control.ts:2-9`: both versions
trim exactly once, interpolate identical punctuation/spacing and synchronously
return a string; the private helper adds no state or interaction. A foreground
Deno harness compared explicit expected greetings for 45 inputs over 100 passes
(9,000 calls), including whitespace, Unicode, control characters, empty and long
names. The scoped source type check passed. An initial stdin check invocation
failed because Deno interpreted `-` as a filename; the corrected invocation
passed. Representative inputs support the assessment; direct source equivalence
supports the broader conclusion.

All three final objects parsed against their production role schemas and matched
the full fixture commit and plan version. Technical and UX findings left
`resolution` unset. Canonical result SHA-256 values (JSON keys sorted, compact
separators, UTF-8 with unescaped Unicode):

- Technical: `2e31278ec1bbae43e5692ea851fcf6f27b502eb7f94681d83a7a30006bcd8717`
- UX: `adad56a41215b63b74373e5376259eede60628572ec349b5d8b954be2fda8c8c`
- Control: `cb9042c0c58999af12906df0eb375833d2a5e2151d7d90ebe4b797fdab2fa178`

## Investigation and adjustments

The first round used fixture commit
`dbd549f1273fc4eb6daa321ccd5478f20b77ed66` and the initial strengthened prompt.
Technical review detected the masked failure; UX review detected misleading
completion feedback; the control returned no findings with an explicit no-impact
assessment. However, both defect reviews put proposed repairs in `resolution`.
Since lifecycle gates treat that field as resolved evidence, this was unacceptable
finding output despite detecting the defects. The shared prompt and review
standards now explicitly require leaving resolution unset for a newly reported
defect and distinguish a proposed repair from verified resolution. A prompt test
preserves this rule. Fresh conversations produced the final results above.

The first UX reviewer also demonstrated an unintended fixture defect: repeating
cancellation after settlement reset the job into draining with an empty queue,
and ticks drove the count negative forever. Its medium finding was supported,
not an aesthetic or invented blocker. The fixture was corrected to make
cancellation idempotent, committed separately, and its deterministic test now
covers repeat cancellation after settlement. Fresh UX review reproduced only the
intended feedback problem while confirming the corrected repeated-use behavior.
Initial conversations were `01a1bdcf-804d-7600-8f11-593efa391b30` (technical),
`01a1bdd0-0165-7af1-87d8-0f413ac4a802` (UX) and
`01a1bdd0-8fef-74f3-908b-4f9c264a281f` (control), each with the corresponding
`issue72-eval-<case>-1` submission identity. They were closed before fresh runs.

## Limits and repetition

This is one provider/settings combination and three small modules. It establishes
detection and a supported control assessment for these cases, not sensitivity,
false-positive rates or broad review quality. Interactions called exported
functions rather than a full terminal CLI. No real rendering, keyboard/focus,
row-intake, process scheduling, failure/recovery UI, external callers or actual
merge was exercised. Source-derived consequences are distinguished above from
observed interactions. Failed advisory invocations are not passed lifecycle
checks; fixture Cargo/Yinz stubs in installed smoke tests are not Rust CI.

To repeat, pin these fixture sources, construct the requirements/approved plan
above in a fixture-only snapshot, and render the appropriate production role
prompt. Replace only the submission section with its evaluation-only production
schema contract, restrict source inspection to the selected module, and launch
fresh bounded read-only visible conversations through the Yinz agents skill.
Retain correlated submissions, inspect actual findings and interactions, record
limitations and close every worker. Do not supply this outcomes reference or
seeded test assertions to reviewers. Investigate a missed defect, unsupported
control blocker or invalid finding semantics before declaring evaluation complete.

## Paired continuity evaluation, 2026-11-11

A second bounded evaluation compared original and context-rich production review
prompts on four independent function scopes in
`unit/fixtures/issue_review/continuity.ts`. Two fresh Codex conversations ran in
visible Yinz panes through guarded prompts and correlated submission waits.
Displayed settings were GPT-6.1-Sol high; model, effort and permission flags were
omitted. The suite used an isolated temporary Git repository, read-only source
scope, foreground probes, an eight-minute bound, no subagents and no lifecycle
state/approval operations. Both panes were closed afterward. One conversation
handled all four cases per condition, so the four results are not independent
samples.

Exact prompts, seeded context snapshots, final objects, correction instructions,
conversation/submission identities and SHA-256 identities are preserved in
`docs/evaluations/lifecycle-continuity/reviewer-results/`. The fixture commit was
`f7906c3a4800b243e3b564a466c766295cbe06a5` in the temporary evaluation repository;
its source SHA-256 is
`cc6b8aa552321dec7a0135f685666c48ec8e7527b0ae603b52bd3c3eaaa86b6d`.
Source identity is checked against the repository fixture; reproductions should
record their own Git identities. No result was imported into a real lifecycle.

| Case | Actual original result | Actual context-rich result |
| --- | --- | --- |
| Incorrect approved design: `approvedDesign` overwrites accumulated success | High unresolved defect; explicitly identified the approved algorithm as wrong | Same defect, independently identified despite plan compliance |
| Incomplete repair: `repairedChecks` uses short-circuit conjunction | High unresolved skipped-callback defect | Retained `run-all` origin and explicit `persists` outcome |
| Regression beyond selected aggregation repair: greeting now lowercases names | High unresolved greeting-preservation defect; aggregation verified | New greeting defect; prior `mask` concern explicitly verified fixed |
| Unchanged trimming tradeoff: private helper extraction | No findings; source equivalence and representative comparisons | No new defect or unsupported reopening; original `whitespace` concern retained as historical unresolved continuity after validation correction |

Both conditions detected all three seeded defects and assessed all four case
contracts. Original probes covered all 63 Boolean sequences of lengths zero
through five: the wrong approved algorithm produced 26 aggregation mismatches,
and the incomplete repair produced 52 callback-execution mismatches despite
correct aggregate Booleans. Context probes covered all 31 sequences through
length four: 11 aggregation mismatches and 22 skipped-callback traces. Both found
correct aggregation/execution in `completeChecks`, then demonstrated changed
public greetings outside that repair. Original greeting comparisons used 15
representative inputs; context used ten. Both supported unchanged extraction with
identical source operations. These are actual foreground reviewer investigations,
not results generated by contract tests. Runtime sample sizes differ, so execution
counts/time are not a controlled performance comparison.

No unsupported reopening occurred. Context retained three explicit prior-finding
outcomes; original had no prior context. The unchanged tradeoff was seeded in
worker-proposed planning decisions, not a real user authorization; reviewers did
not infer renewed approval from it. The production packet separately labels
actual fingerprint-bound user decisions. No historical execution reuse was
supplied, no advisory executions avoided were measured, and failed advisory
commands (including an absent fixture AGENTS.md) remain honestly recorded.

There were two material contract lessons. The evaluation wrapper initially omitted
the concrete role schema; both conversations reformatted their existing findings
using the exact production schema without repeating investigation. Context then
claimed `persists` for the unchanged whitespace concern but omitted its current
visible finding. Structural validation rejected that output; the original worker
carried the same low-severity concern, origin and evidence without new probes or a
new defect claim. Production prompts and guidance now explicitly require both
`priorFindings` and the visible unresolved entry for `persists`. Final objects
parse against production schemas and context objects pass prior-finding
validation. These corrections are part of the observed results, not hidden
successful first attempts.

This provides bounded defect-detection and evidence-use observations, not a
measured efficiency improvement. Histories were seeded, scope excluded real
callers/processes/UI/concurrency and full lifecycle gates, and the final prompt's
compaction of repeated historical diffs to exact saved-snapshot references was
covered by contract tests rather than another fresh live round. Further pilot
issues must assess real user dispositions, requirements changes, recovery and
gate failures. The frozen pre-change sample, read-only report and next-20-issues
pilot protocol are documented in `docs/ISSUE_LIFECYCLE.md`. Unknown quality and
post-completion outcomes stay unknown; small samples and any regressions must be
reported before claiming a benefit.
