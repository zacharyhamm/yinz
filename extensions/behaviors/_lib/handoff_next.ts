/**
 * The exact launcher command that follows a handoff. Pure, and it imports only
 * lifecycle.ts, so closeout can use it without an import cycle.
 */
import {
  activeFollowUps,
  type LifecycleState,
  lifecycleSummary,
  pendingReviewFindings,
  selectableFollowUp,
  stateDigest,
} from "./lifecycle.ts";

/**
 * What a handoff persists of its lifecycle state: the state's canonical digest,
 * which fences decisions, and the few fields its readers display.
 */
export function handoffStateFields(state: LifecycleState) {
  return {
    stateDigest: stateDigest(state),
    lifecycle: lifecycleSummary(state),
  };
}

/**
 * Whether a handoff was written for exactly this lifecycle state. Handoffs saved
 * before compaction embed the whole state instead of its digest.
 */
export function handoffMatchesState(
  handoff: { state?: unknown; stateDigest?: string },
  state: LifecycleState,
): boolean {
  if (handoff.stateDigest !== undefined) {
    return handoff.stateDigest === stateDigest(state);
  }
  return handoff.state !== undefined &&
    JSON.stringify(handoff.state) === JSON.stringify(state);
}

export function nextStep(h: {
  id: string;
  decision: string;
  planVersion?: number;
  commit?: string;
}, state: LifecycleState): string {
  const run = `scripts/issue-lifecycle --item ${state.issue.itemId}`;
  const worktreeInput = `--input-file <{"worktree": "...", "branch": "..."}>`;
  switch (h.decision) {
    case "approve_plan":
      return `Next: ${run} approve --handoff ${h.id} --request '<the explicit user approval of plan version ${h.planVersion} and its checks>' (records the approval; drive then continues to the next handoff)`;
    case "authorize_plan_revision":
      return `Next: ${run} drive --handoff ${h.id} --request '<the user authorization of exactly one targeted revision>'; or, needing no user input, ${run} adjudicate-plan --handoff ${h.id} --input-file <{"dispositions": [...]}> for evidence-supported dispositions (accepted unresolved high/critical risks still require user direction)`;
    case "prepare_worktree":
      if (state.closeout?.authorizedRequest) {
        return `Next: after the user deliberately selects a new worktree, ${run} implement ${worktreeInput}, then ${run} drive`;
      }
      if (state.workerWorktree) {
        return `Next: ${run} implement --input-file <{"worktree": "${state.workerWorktree}", "branch": "<its feature branch>"}>, then ${run} drive`;
      }
      return `Next: ${run} implement ${worktreeInput} (legacy issue without a recorded worker worktree), then ${run} drive`;
    case "address_review_findings": {
      // Findings the user already ignored, directly or carried, need no choice.
      const findings = pendingReviewFindings(state).map((f) =>
        `${f.role}/${f.findingId}`
      );
      // Only follow-ups in the current reviews are valid --repair selections.
      const followUps = activeFollowUps(state).filter((f) =>
        selectableFollowUp(state, f)
      ).map((f) => `${f.finding.role}/${f.finding.findingId}`);
      return `Next: ${run} decide-review --handoff ${h.id} --request '<the user direction>' with --repair <role/id> and --ignore <role/id> for each finding the user chose (--ignore-remaining when the user ignores all others); selected repairs go straight to a repair worker. Findings: ${
        findings.join(", ") || "none"
      }${
        followUps.length
          ? `. Follow-ups (deferred unless selected): ${followUps.join(", ")}`
          : ""
      }. To stop asking for this plan, ${run} drive --auto-repair --request '<the standing user instruction to repair every blocking finding>'`;
    }
    case "approve_review_fixes":
      return `Next: ${run} drive (dispatches the repair worker; legacy plan approval is no longer required), or ${run} approve-review-fixes --handoff ${h.id} --request '<the user approval of this repair plan>' to keep the saved plan as guidance`;
    case "authorize_merge":
      return `Next: ${run} ready --handoff ${h.id} --request '<the explicit user merge authorization for ${h.commit}>' (merge then rebases, runs the full CI gate and fast-forwards main; add --skip-ci-request '<the explicit user waiver>' only if the user waived CI for this merge)`;
    case "integrate":
      return state.attempt
        ? `Next: inspect merge attempt ${state.attempt.id} (an existing attempt is never re-run automatically), then ${run} merge`
        : `Next: ${run} merge, in the background or with a timeout of several hours (only ready and drive run merge automatically, unless --no-merge; other chained commands stop at this handoff; an existing attempt is never re-run automatically)`;
    case "summarize":
      return `Next: compare the receipt with the requirements, then ${run} summarize --input-file <{"outcomeMet": true, "summary": "..."}>; if the outcome is unmet, keep the Agenda item open and explain the remaining work`;
    case "authorize_closeout":
      return `Next: ${run} closeout --handoff ${h.id} --decision accept|decline --request '<the explicit user closeout decision>'`;
    case "resume_closeout":
      return `Next: from a surviving pane, ${run} resume-closeout`;
    case "complete":
      return "Next: None; present the completed result";
    case "blocked":
    default:
      return `Next: run ${run} status and follow taskRecovery, then the matching recovery section`;
  }
}
