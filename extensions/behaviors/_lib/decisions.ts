/** Pure resolution of handoff-bound human decisions into strict transition inputs. */
import { z } from "npm:zod@4.6.7";
import {
  ActionSchemas,
  activeReviewFix,
  AdjudicationSchema,
  blockingReviewFindings,
  codeReviewFingerprint,
  type LifecycleState,
  reviewFingerprint,
  UserRequestSchema,
} from "./lifecycle.ts";
import type { Handoff } from "./issue_driver.ts";

const HandoffId = z.uuid();
const Fingerprint = z.string().regex(/^[0-9a-f]{64}$/);
const Commit = z.string().regex(/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/);
const Refs = ActionSchemas.decide_review.shape.repair;

/**
 * Method inputs. Each takes the exact handoff id, or the legacy explicit
 * values that bind as they always have. `.strip()` because Swamp merges
 * globals into method arguments.
 */
export const DecisionInputSchemas = {
  approve: z.object({
    handoffId: HandoffId.optional(),
    planVersion: z.number().int().positive().optional(), // legacy
    request: UserRequestSchema,
  }).strip(),
  adjudicate_plan: z.object({
    handoffId: HandoffId.optional(),
    planVersion: z.number().int().positive().optional(), // legacy
    reviewFingerprint: Fingerprint.optional(), // legacy
    dispositions: AdjudicationSchema.shape.dispositions,
  }).strip(),
  decide_review: z.object({
    handoffId: HandoffId.optional(),
    id: z.uuid().optional(), // legacy alias of handoffId
    reviewFingerprint: Fingerprint.optional(), // legacy
    request: UserRequestSchema,
    repair: Refs.default([]),
    ignore: Refs.default([]),
    ignoreRemaining: z.boolean().default(false),
  }).strip(),
  approve_review_fixes: z.object({
    handoffId: HandoffId,
    id: z.uuid().optional(), // legacy; derived from the active repair
    request: UserRequestSchema,
  }).strip(),
  ready: z.object({
    handoffId: HandoffId.optional(),
    commit: Commit.optional(), // legacy
    mergeRequest: z.string().trim().min(1),
  }).strip(),
} as const;
export type DecisionAction = keyof typeof DecisionInputSchemas;

export function isDecisionAction(action: string): action is DecisionAction {
  return Object.hasOwn(DecisionInputSchemas, action);
}

/** The single currency check for every handoff-bound decision. */
export function assertCurrentHandoff(
  handoff: Handoff | undefined,
  state: LifecycleState,
  handoffId: string,
  decisions: readonly Handoff["decision"][],
): Handoff {
  if (
    !handoff || handoff.id !== handoffId || handoff.status !== "current" ||
    !decisions.includes(handoff.decision) ||
    JSON.stringify(handoff.state) !== JSON.stringify(state)
  ) {
    throw new Error(
      `Decision handoff is stale: ${handoffId} is not the current ${
        decisions.join(" or ")
      } handoff for this lifecycle state; run drive and present its new handoff`,
    );
  }
  return handoff;
}

/** A value derived from the current handoff or state; any explicit value must equal it. */
function derived<T>(field: string, given: T | undefined, value: T | undefined) {
  if (value === undefined) {
    throw new Error(`The current handoff records no ${field}`);
  }
  if (given !== undefined && given !== value) {
    throw new Error(
      `${field} ${String(given)} does not match the current ${
        String(value)
      }; the decision is stale`,
    );
  }
  return value;
}

/**
 * Returns arguments for the strict ActionSchemas[action]. `readHandoff` parses
 * lazily, so legacy inputs never depend on the handoff resource.
 */
export function resolveDecisionArgs(
  action: DecisionAction,
  raw: unknown,
  state: LifecycleState,
  readHandoff: () => Handoff | undefined,
): unknown {
  switch (action) {
    case "approve": {
      const a = DecisionInputSchemas.approve.parse(raw);
      if (!a.handoffId) {
        if (a.planVersion === undefined) {
          throw new Error(
            "approve requires handoffId (or the legacy planVersion)",
          );
        }
        return { planVersion: a.planVersion, request: a.request };
      }
      const h = assertCurrentHandoff(readHandoff(), state, a.handoffId, [
        "approve_plan",
      ]);
      const planVersion = derived("planVersion", a.planVersion, h.planVersion);
      if (h.reviewFingerprint !== reviewFingerprint(state)) {
        throw new Error("Plan review changed; the approval handoff is stale");
      }
      return { planVersion, request: a.request };
    }
    case "adjudicate_plan": {
      const a = DecisionInputSchemas.adjudicate_plan.parse(raw);
      if (!a.handoffId) {
        if (a.planVersion === undefined || a.reviewFingerprint === undefined) {
          throw new Error(
            "adjudicate-plan requires handoffId (or the legacy planVersion and reviewFingerprint)",
          );
        }
        return {
          planVersion: a.planVersion,
          reviewFingerprint: a.reviewFingerprint,
          dispositions: a.dispositions,
        };
      }
      const h = assertCurrentHandoff(readHandoff(), state, a.handoffId, [
        "authorize_plan_revision",
        "approve_plan",
      ]);
      return {
        planVersion: derived("planVersion", a.planVersion, h.planVersion),
        reviewFingerprint: derived(
          "reviewFingerprint",
          a.reviewFingerprint,
          h.reviewFingerprint,
        ),
        dispositions: a.dispositions,
      };
    }
    case "decide_review": {
      const a = DecisionInputSchemas.decide_review.parse(raw);
      if (a.handoffId && a.id && a.handoffId !== a.id) {
        throw new Error(
          `Review decision id ${a.id} differs from handoffId ${a.handoffId}; the decision id is the handoff id`,
        );
      }
      const handoffId = a.handoffId ?? a.id;
      if (!handoffId) {
        throw new Error("decide-review requires handoffId (or the legacy id)");
      }
      const h = assertCurrentHandoff(readHandoff(), state, handoffId, [
        "address_review_findings",
      ]);
      if (h.codeReviewFingerprint !== codeReviewFingerprint(state)) {
        throw new Error("Review findings changed");
      }
      const fingerprint = derived(
        "reviewFingerprint",
        a.reviewFingerprint,
        h.codeReviewFingerprint,
      );
      const ignore = [...a.ignore];
      if (a.ignoreRemaining) {
        const chosen = [...a.repair, ...a.ignore];
        // Follow-ups stay deferred; only findings that block need a choice.
        for (const { role, findingId } of blockingReviewFindings(state)) {
          if (
            !chosen.some((ref) =>
              ref.role === role && ref.findingId === findingId
            )
          ) ignore.push({ role, findingId });
        }
      }
      return {
        id: handoffId,
        reviewFingerprint: fingerprint,
        request: a.request,
        repair: a.repair,
        ignore,
      };
    }
    case "approve_review_fixes": {
      const a = DecisionInputSchemas.approve_review_fixes.parse(raw);
      const h = assertCurrentHandoff(readHandoff(), state, a.handoffId, [
        "approve_review_fixes",
      ]);
      if (h.codeReviewFingerprint !== codeReviewFingerprint(state)) {
        throw new Error("Review findings changed");
      }
      const fix = activeReviewFix(state);
      if (!fix) {
        throw new Error("Approval must match the current review repair plan");
      }
      return {
        id: derived("id", a.id, fix.id),
        handoffId: a.handoffId,
        request: a.request,
      };
    }
    case "ready": {
      const a = DecisionInputSchemas.ready.parse(raw);
      if (!a.handoffId) {
        if (a.commit === undefined) {
          throw new Error("ready requires handoffId (or the legacy commit)");
        }
        return { commit: a.commit, mergeRequest: a.mergeRequest };
      }
      const h = assertCurrentHandoff(readHandoff(), state, a.handoffId, [
        "authorize_merge",
      ]);
      return {
        commit: derived("commit", a.commit, h.commit),
        mergeRequest: a.mergeRequest,
      };
    }
  }
}
