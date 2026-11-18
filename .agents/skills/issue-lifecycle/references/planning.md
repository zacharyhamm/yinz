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
  The evidence template already lists one entry per blocking finding. Use
  `status` of `addressed`, `disputed`, or `unresolved`, and provide `rationale`
  and nonempty `evidence` citing source, requirements or checked behavior.
- `acceptedDecisions`: the decisions and tradeoffs being retained.

A disputed finding needs counterevidence; disagreement alone is insufficient.
An unresolved finding stays visible. Do not fabricate a fix, silently broaden
requirements, or treat the revision request as implementation approval. Historical
reviews help reasoning but do not constitute current review or approval evidence.
Start from the evidence template in the assignment directory named by your
prompt; keep its pre-filled identities exactly. Submit with the assignment's
`submit_evidence.py` command, which fills `taskId`, `assignmentDigest` and `role`
from the bound target and exits 0 only after printing the accepted receipt. Use
`--status` after an interrupted run. Older assignments without assignment files
use the exact Swamp evidence behavior, schema and commands in their saved prompt.
Final chat text is presentation and is never lifecycle evidence. Correct rejected
submissions in the same worker using the investigation already performed. For an
older assignment, the controller supplies a submission-only continuation; do not
restart the investigation or treat its old final-message contract as authority.

## Check policy

Plan checks are enforced when plan evidence (`plan`, legacy `plan_review_fixes`,
or the optional `checks` of an `implement_review_fixes` result) is
submitted; a violation is rejected and listed in full, and the worker corrects
the submission in place. Persisted plans are not re-validated. Each check's
`command` is an argv array run directly in the issue worktree without a shell:

- Put every argument in its own element. Shell operators (`&&`, `||`, `|`, `;`,
  `&`, redirections), `sh`/`bash -c` wrappers, `$VARIABLES` and `~` paths are
  rejected because nothing is expanded.
- Set environment variables only through a leading `env NAME=VALUE`. Never set
  `CARGO_TARGET_DIR`, `--target-dir`, or a target-dir or build-dir config. A
  `CARGO_TARGET_DIR=`, `CARGO_BUILD_TARGET_DIR=` or `CARGO_BUILD_BUILD_DIR=`
  assignment is rejected wherever it appears in argv, including behind any
  runner or in an `env -S` string, which is split with GNU env's quoting and
  `\_` separators. Any element containing such an assignment is rejected, even
  a `grep` pattern; search for `CARGO_TARGET_DIR` without the `=`. Use only
  env's `-i`, `-0`, `-v`, `-u`, `-C`, `-S` and `-a` options (and their long
  forms); other env options and `--env0-from`, whose entries cannot be
  inspected, are rejected.
- `cargo x test`, `cargo test` and `cargo nextest run` need a package (`-p`) or a
  test selection (`-E` filterset, a test name, or a named `--test`, `--bench`,
  `--bin` or `--example` target); `--workspace` (or a package glob such as
  `-p '*'` or `-p 'yinz-*'`) does not count, and neither does an empty name,
  `-E 'all()'`, `-E 'not none()'` or a `--skip` pattern.
  The values of nextest options such as `--no-tests`, `--build-jobs` or
  `--config-file` are not test names. `cargo x test` and
  `cargo x integration-test` pass everything after `--` to nextest as nextest
  options, so `cargo x test -- --run-ignored all` is unfiltered and
  `cargo x integration-test -- -E 'test(x)' -j 16` is bounded. With an explicit
  `--`, put every nextest option after it (`cargo x test -- -p a -- --exact
  name`): xtask rejects flags it does not own before the `--`.
- `cargo x integration-test` needs a selection and `-j N` (or `--serial`).
- `cargo x format` needs `--check`; `cargo x lint` must not use `--fix`;
  `cargo x build` is allowed. `cargo x ci` is the merge gate the lifecycle runs
  at merge. `cargo x run` (and the `cargo q` alias) launches the interactive app,
  and a check must terminate. Other `cargo x` subcommands are not development
  checks.
- The xtask rules also apply to `cargo run` of the xtask package, however it is
  named (`-p xtask`, `-p=xtask`, `xtask@0.1.0` or another package spec,
  `--bin xtask`, `-m`/`--manifest-path xtask/Cargo.toml`, or a working directory
  inside `xtask/`), to the xtask binary itself (`target/debug/xtask`), and to
  the repository's cargo aliases in `.cargo/config.toml` (`xtask`, `x`, `q`,
  `make`); the nextest rules apply to the `cargo-nextest` binary. Checks must not define cargo aliases
  (`--config alias.…`, `CARGO_ALIAS_…=`) or pass `--config` files.
- Use nextest's `--no-capture` and `-j N`, never libtest's `--nocapture` or
  `--test-threads`.
- Wrappers do not exempt a command: the cargo rules apply behind `env`
  (including `-C`, `-S` and `-a`), `timeout`, `nice`, `ionice`, `time`,
  `stdbuf`, `nohup`, and to every `cargo` that appears later in the argv.

The policy is mechanical. Planners still choose checks that cover the changed
behavior and affected callers.

## Review repair continuity

Contract-2 and contract-3 repair assignments include the exact selected findings and their full
origin identities, original evidence, ignored concerns, scoped planning decisions
and controller/user authorizations, approved checks, previous repair responses,
failure log references and captured Git endpoints. Use this packet as the source
of repair obligations; preserve unaffected decisions. Worker-proposed tradeoffs
are distinct from controller adjudications and user approval.

Selection authorizes the repair worker directly; there is no repair plan or
approval step. A repair implementation returns exactly one `responses` entry for
each selected finding (pre-filled in the template): `addressed`, `disputed` or
`unresolved`, rationale, affected repository paths, source/check evidence and
relevant check IDs. When the approved plan's checks do not cover a repaired
behavior, return `checks`: focused argv checks under the
[check policy](#check-policy), with IDs no plan check uses and `covers` naming
the repaired findings or plan steps. Verification runs them after the plan's
checks. Disputes and partial repairs remain visible. `scopeChanges` are rejected:
report a finding that needs broader work as `unresolved`, which needs a plan
revision. Implementation claims never close findings. Delta-scoped independent
reviews and deterministic verification still follow every repair.

Repair plans (`findingMappings`, `scopeChanges`, `proposedTradeoffs`) exist only
on issues saved before repair planning was retired. A saved plan is guidance for
the repair worker, and its checks join verification only if the user approved it.
