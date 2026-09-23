---
name: sync-zellij-upstream
description: Sync local upstream-main with upstream Zellij, review new upstream/main changes against this fork's live default branch, recommend which changes to adapt, record rejected changes, and adapt approved changes in the current feature worktree. Use when asked to sync, review, assess, backport, or adapt changes from zellij-org/zellij upstream main into this fork.
---

# Sync Zellij Upstream

Treat `upstream-main` as a local mirror of `upstream/main`. Use the fork's live default
branch only as a read-only comparison baseline. Record decisions and adapt approved changes
on the feature branch in the worktree where the user invoked this skill. Never merge
upstream wholesale into the fork.

## Guardrails

- Preserve all unrelated worktree and index changes.
- Never reset, rebase, force-update, or push.
- Move `upstream-main` only by fast-forward.
- Do not edit fork files or commit decisions before the user approves specific candidates.
- Do not edit, commit, or move branches on `main`, the live default branch, or in another
  worktree unless the user explicitly names that destination. Candidate approval does not
  authorize a destination change, branch switch, or merge.
- If the invocation branch is `main`, the live default branch, `upstream-main`, or detached,
  ask the user for the desired feature branch before any fork edits or commits. Do not
  create or switch branches silently. Continue the read-only review while awaiting a target.
- Do not adapt a rejected change again unless a later change needs it and the
  user explicitly reopens the decision.
- Stop and explain any remote, branch, or worktree state that makes these rules
  impossible to follow.

## Discover the repository state

1. Record the invocation worktree path, branch (or detached state), tip SHA, and worktree
   and index status. This is the adaptation destination unless the user explicitly chooses
   another. Record any chosen replacement destination with the same details.
2. Confirm that `origin` is the fork and `upstream` is `zellij-org/zellij`.
3. Read the live default branch from `git ls-remote --symref origin HEAD`.
   Do not trust the local `origin/HEAD`, because it can be stale.
4. Fetch `origin` and `upstream/main`.
5. Record these SHAs before changing a branch:
   - local `upstream-main`
   - fetched `upstream/main`
   - local and remote tips of the live default branch
6. Read `docs/upstream-sync-decisions.md` on both the comparison baseline and adaptation
   branch if present. Honor skips recorded on either branch.

Use the local default branch as the comparison baseline when it is equal to or
ahead of its remote branch. Use the fetched remote branch when the local branch
is only behind. Stop and ask the user which tip is authoritative when the two
tips diverge. Selecting a baseline never authorizes moving the default or adaptation
branch. If the adaptation tip differs from the baseline, inspect those differences
and assess each candidate against the adaptation tip too.

## Fast-forward upstream-main

Verify that the old `upstream-main` is an ancestor of `upstream/main`. If it is not, stop.

If `upstream-main` is not checked out in any worktree, update it to `upstream/main` with
a fast-forward ref update. If it is checked out, use `git merge --ff-only upstream/main`
only when that worktree and its index are clean. If that is another worktree, obtain the
user's explicit authorization for that destination first. Do not push `upstream-main`
to `origin`.

If the old and new `upstream-main` SHAs are equal, tell the user that upstream has no new
changes and stop.

## Analyze new upstream changes

Inspect every commit and diff in `old-upstream-main..upstream-main`. Group commits only when they
implement one logical change. For each group:

1. State what upstream changed and why.
2. Find the corresponding paths, symbols, tests, and behavior on the fork tip.
3. Determine whether the fork already has equivalent behavior.
4. Assess the benefit, adaptation cost, regression risk, and prerequisites.
5. Recommend `adapt` or `skip`.

Do not recommend a change only because it exists upstream. Prefer the fork's
existing architecture and behavior when they already solve the problem.

Match upstream SHAs against the decision ledger. Do not propose a recorded skip
again. If a new change depends on a skipped change, label it `reconsideration`,
explain the dependency, and request permission to reopen the old decision.

## Report and request a decision

Write the report in Simplified Technical English (ASD-STE100):

- Use short, active sentences.
- Put one idea or instruction in each sentence.
- Use common words and define necessary technical terms.
- Avoid contractions, idioms, vague pronouns, and long noun groups.
- Keep identifiers and Git terms exact.

Give the analyzed SHA range and the exact comparison baseline branch and SHA. State
the adaptation worktree path, branch, and tip SHA separately. For each candidate, give
its upstream SHAs, effect, fork comparison, risks, prerequisites, and recommendation.
Then ask the user which candidates to adapt or skip. If the adaptation destination is
unresolved, also ask for the desired feature branch. Do not edit the fork before the
user answers.

## Verify the adaptation destination

After the user decides, verify that the recorded worktree still has the recorded
adaptation branch and tip. Require a clean worktree and index before the first write
in this batch. Stop on unexpected changes instead of choosing another worktree.
Do not fast-forward, merge, or otherwise move a fork branch to the comparison baseline.
Use the recorded destination for every ledger edit, commit, adaptation, and check.
Track the batch's own ledger commit as an expected tip change.

## Record skipped changes

After the user rejects a candidate, append one entry to
`docs/upstream-sync-decisions.md` on the recorded adaptation branch:

```markdown
## YYYY-MM-DD — <short change name>

- Upstream commits: `<sha>`
- Upstream range: `<old-upstream-main>..<new-upstream-main>`
- Fork baseline: `<default-branch>` at `<sha>`
- Decision: Skip
- Reason: <specific reason>
- Reconsider only if: <dependency or condition, or "The user reopens this decision.">
```

Create the file with `# Upstream Sync Decisions` when the first skip occurs.
Keep prior entries unchanged. Make one local commit for all skip decisions in
the review batch with the message `docs: record skipped upstream changes`.
Commit only the ledger file in the recorded adaptation worktree, before code adaptations.
Do not push the commit. Do not switch to the default branch or create a temporary
worktree to record decisions.

## Adapt approved changes

For each approved candidate:

1. Trace the affected fork flow and all callers before editing.
2. Implement the smallest equivalent behavior in the fork's architecture.
3. Reuse fork helpers, types, and tests.
4. Do not merge upstream or blindly cherry-pick a commit across divergent code.
5. Run the smallest focused checks that cover the changed behavior.
6. Report the result, remaining risk, and test commands.

Apply adaptations in the recorded adaptation worktree and branch. Leave adapted code
uncommitted for review. Do not push it or merge it into the default branch.
In the final report, name the comparison baseline and the actual worktree and branch
that contain the ledger commit and uncommitted adaptations.
