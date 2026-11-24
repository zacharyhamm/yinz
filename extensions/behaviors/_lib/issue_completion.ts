/** Completion after a confirmed local merge. The caller holds the issue lock. */
import { z } from "npm:zod@4.6.7";
import {
  ActionSchemas,
  activeFollowUps,
  completeAgenda,
  fileFollowUpItem,
  followUpMarker,
  type LifecycleState,
  transition,
} from "./lifecycle.ts";
import {
  type Context,
  gitOperation,
  type Handle,
  normalizeReceipt,
  saveState,
} from "./runtime.ts";

export type Outcome = z.infer<typeof ActionSchemas.done>;

export interface CompletionDependencies {
  gitOperation: typeof gitOperation;
  completeAgenda: typeof completeAgenda;
  fileFollowUpItem: typeof fileFollowUpItem;
}
export const completionDependencies: CompletionDependencies = {
  gitOperation,
  completeAgenda,
  fileFollowUpItem,
};

/**
 * Record the outcome, file each follow-up as its own Agenda item exactly once
 * (retrying by marker), and complete the Agenda issue. Idempotent once done.
 */
export async function completeIssue(
  context: Context,
  state: LifecycleState,
  outcome: Outcome,
  deps: CompletionDependencies = completionDependencies,
): Promise<{ state: LifecycleState; dataHandles: Handle[] }> {
  if (!state.receipt) {
    throw new Error("Completion requires a local merge receipt");
  }
  await deps.gitOperation(context, {
    action: "receipt",
    attemptId: state.receipt.attemptId,
  }).then((raw) => normalizeReceipt(raw, state));
  if (state.phase === "done") return { state, dataHandles: [] };
  // Validate the outcome before any Agenda write.
  transition(state, "done", outcome);
  const filed: Handle[] = [];
  for (const followUp of activeFollowUps(state)) {
    if (followUp.agendaItem?.status === "created") continue;
    const marker = followUp.agendaItem?.marker ?? followUpMarker(followUp);
    if (!followUp.agendaItem) {
      // Record intent first: a retry searches for the marker before adding.
      followUp.agendaItem = { marker, status: "creating" };
      filed.push(
        ...(await saveState(context, "file_follow_up", state)).dataHandles,
      );
    }
    const itemId = await deps.fileFollowUpItem(state.issue, followUp, marker);
    followUp.agendaItem = { marker, status: "created", itemId };
    filed.push(
      ...(await saveState(context, "file_follow_up", state)).dataHandles,
    );
  }
  const next = transition(state, "done", outcome);
  await deps.completeAgenda(state.issue);
  const saved = await saveState(context, "done", next);
  return { state: next, dataHandles: [...filed, ...saved.dataHandles] };
}

/**
 * The outcome drive records once a merge lands. By then the approved plan was
 * reviewed against these requirements (drive refuses to dispatch when Agenda
 * changed), code and UX reviews were settled, verification passed and the
 * merge gate ran. The summary cites that evidence.
 */
export function automaticOutcome(state: LifecycleState): Outcome {
  const receipt = state.receipt;
  if (!receipt) throw new Error("Completion requires a local merge receipt");
  const parts = [
    `Merged ${receipt.mergedCommit} into local main at ${receipt.mergedAt}` +
    (receipt.ciStatus === "passed"
      ? " after the full CI gate passed."
      : ` with the CI gate waived: ${receipt.skipCiRequest ?? "waived"}.`),
  ];
  if (state.plan) {
    const summary = state.plan.summary.trim();
    parts.push(
      `Plan version ${state.plan.version}: ${summary}${
        /[.!?]$/.test(summary) ? "" : "."
      }`,
    );
  }
  const deviated =
    state.codeReview?.steps.filter((step) => step.status === "deviated") ?? [];
  if (state.codeReview) {
    parts.push(
      deviated.length
        ? `Code review accepted deviations: ${
          deviated.map((step) =>
            `${step.id}${step.justification ? ` (${step.justification})` : ""}`
          ).join("; ")
        }.`
        : `Code review found every plan step implemented.`,
    );
  }
  const followUps = activeFollowUps(state).length;
  if (followUps) {
    parts.push(`${followUps} follow-up finding(s) filed as Agenda items.`);
  }
  parts.push("Recorded automatically after the merge landed.");
  return { outcomeMet: true, summary: parts.join(" ") };
}
