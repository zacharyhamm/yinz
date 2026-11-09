# Bounded reviewer evaluation

These are actual read-only reviewer conversations, separate from issue 72's
technical/UX approvals. They evaluate three deliberately small scenarios, not
general review reliability. `issue_review_test.ts` deterministically establishes
the scenarios and assignment contract; its passing assertions do not prove a
reviewer detects defects.

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
