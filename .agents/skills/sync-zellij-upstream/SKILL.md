---
name: sync-zellij-upstream
description: Sync local upstream-main with upstream Zellij, review new upstream/main changes against this fork's live default branch, recommend which changes to adapt, record rejected changes, and adapt approved changes to the divergent fork. Use when asked to sync, review, assess, backport, or adapt changes from zellij-org/zellij upstream main into this fork.
---

# Sync Zellij Upstream

Treat `upstream-main` as a local mirror of `upstream/main`. Treat the fork's live default
branch as the adaptation target. Never merge upstream wholesale into the fork.

## Guardrails

- Preserve all unrelated worktree and index changes.
- Never reset, rebase, force-update, or push.
- Move `upstream-main` only by fast-forward.
- Do not change the fork branch before the user approves specific changes.
- Do not adapt a rejected change again unless a later change needs it and the
  user explicitly reopens the decision.
- Stop and explain any remote, branch, or worktree state that makes these rules
  impossible to follow.

## Discover the repository state

1. Confirm that `origin` is the fork and `upstream` is `zellij-org/zellij`.
2. Read the live default branch from `git ls-remote --symref origin HEAD`.
   Do not trust the local `origin/HEAD`, because it can be stale.
3. Fetch `origin` and `upstream/main`.
4. Record these SHAs before changing a branch:
   - local `upstream-main`
   - fetched `upstream/main`
   - local and remote tips of the live default branch
5. Read `docs/upstream-sync-decisions.md` if it exists.

Use the local default branch as the comparison target when it is equal to or
ahead of its remote branch. Use the fetched remote branch when the local branch
is only behind. Stop and ask the user which tip is authoritative when the two
tips diverge.

## Fast-forward upstream-main

Verify that the old `upstream-main` is an ancestor of `upstream/main`. If it is not, stop.

If `upstream-main` is not checked out in any worktree, update it to `upstream/main` with
a fast-forward ref update. If `upstream-main` is checked out, update it in that worktree
with `git merge --ff-only upstream/main` only when that worktree and its index
are clean. Do not push `upstream-main` to `origin`.

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

Give the analyzed SHA range and the exact fork target SHA. For each candidate,
give its upstream SHAs, effect, fork comparison, risks, prerequisites, and
recommendation. Then ask the user which candidates to adapt or skip. Do not
edit the fork before the user answers.

## Prepare the fork branch

After the user decides, make the local default branch point to the exact fork
target SHA from the report. Create the local branch at that SHA if it does not
exist. If the local branch is behind and is not checked out, update its ref only
after an ancestor check. If it is checked out, fast-forward it only when that
worktree and index are clean. Stop if it diverged, moved after the report, or
cannot be fast-forwarded safely.

## Record skipped changes

After the user rejects a candidate, append one entry to
`docs/upstream-sync-decisions.md` on the local default branch:

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
Commit only the ledger file. Do not push the commit.

Use a clean worktree for this commit. If the default branch is not checked out
in any worktree, use a temporary worktree. If it is already checked out in a
dirty worktree, stop instead of trying to check it out again or mixing the
ledger with unrelated work.

## Adapt approved changes

For each approved candidate:

1. Trace the affected fork flow and all callers before editing.
2. Implement the smallest equivalent behavior in the fork's architecture.
3. Reuse fork helpers, types, and tests.
4. Do not merge upstream or blindly cherry-pick a commit across divergent code.
5. Run the smallest focused checks that cover the changed behavior.
6. Report the result, remaining risk, and test commands.

Apply adaptations to a clean worktree for the local default branch. If the
branch is not checked out in any worktree, create a named worktree under
`.worktrees/` and tell the user its path. If its existing worktree is dirty,
stop. Leave adapted code uncommitted for review. Do not push it.
