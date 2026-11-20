/**
 * Moves review diff bodies out of a lifecycle state without changing any
 * identity derived from it. Review fingerprints hash review JSON, which used
 * to embed the bodies; compaction records each fingerprint from the full form
 * in `fingerprintAliases` before replacing the bodies with references.
 */
import { canonical } from "./canonical.ts";
import {
  activeFollowUps,
  activeReviewFix,
  aliasedFingerprint,
  carriedIgnores,
  codeReviewDigest,
  implementationReviewDigest,
  implementationReviewFingerprint,
  type LifecycleState,
  pendingReviewFindings,
  priorImplementationFindings,
  StateSchema,
} from "./lifecycle.ts";
import { identityKey, isInlineBaseline } from "./review_continuity.ts";
import { compactBaseline, type DiffStore } from "./review_diffs.ts";

type Role = "review_code" | "review_ux";
type Review = NonNullable<
  LifecycleState["codeReview"] | LifecycleState["uxReview"]
>;
type Slot = {
  role: Role;
  get: (state: LifecycleState) => Review | undefined;
  set: (state: LifecycleState, review: Review) => void;
};

/** Every place a state keeps an implementation review, with its role. */
function reviewSlots(state: LifecycleState): Slot[] {
  const slots: Slot[] = [
    {
      role: "review_code",
      get: (s) => s.codeReview,
      set: (s, r) => s.codeReview = r as LifecycleState["codeReview"],
    },
    {
      role: "review_ux",
      get: (s) => s.uxReview,
      set: (s, r) => s.uxReview = r as LifecycleState["uxReview"],
    },
  ];
  (state.reviewHistory ?? []).forEach((entry, index) =>
    slots.push({
      role: entry.role,
      get: (s) => s.reviewHistory?.[index]?.review,
      set: (s, r) => s.reviewHistory![index].review = r,
    })
  );
  (state.reviewDecisions ?? []).forEach((_decision, index) => {
    slots.push({
      role: "review_code",
      get: (s) => s.reviewDecisions?.[index]?.codeReview,
      set: (s, r) =>
        s.reviewDecisions![index].codeReview = r as NonNullable<
          LifecycleState["codeReview"]
        >,
    });
    slots.push({
      role: "review_ux",
      get: (s) => s.reviewDecisions?.[index]?.uxReview,
      set: (s, r) =>
        s.reviewDecisions![index].uxReview = r as LifecycleState["uxReview"],
    });
  });
  return slots;
}

/** Whether any review in the state still embeds its diff bodies. */
export function hasInlineDiffs(state: LifecycleState): boolean {
  return reviewSlots(state).some((slot) => {
    const baseline = slot.get(state)?.baseline;
    return !!baseline && isInlineBaseline(baseline);
  });
}

function attempt<T>(read: () => T): T | string {
  try {
    return read();
  } catch (error) {
    return `unavailable: ${error instanceof Error ? error.message : error}`;
  }
}

/**
 * The identities and obligations compaction must preserve: review and decision
 * fingerprints, finding lineage, follow-ups, ignores and the active repair.
 */
export function continuityFacts(state: LifecycleState) {
  return {
    reviews: reviewSlots(state).map((slot) => {
      const review = slot.get(state);
      return review
        ? implementationReviewFingerprint(state, slot.role, review)
        : null;
    }),
    code: state.codeReview
      ? aliasedFingerprint(state, codeReviewDigest(state))
      : null,
    codeStage: state.codeReview
      ? aliasedFingerprint(
        state,
        codeReviewDigest({ ...state, uxReview: undefined }),
      )
      : null,
    prior: priorImplementationFindings(state).map((record) => [
      identityKey(record.ref),
      record.fixed,
      record.sourceReview,
      record.finding.severity,
    ]),
    followUps: activeFollowUps(state).map((f) => identityKey(f.finding)),
    pending: attempt(() =>
      pendingReviewFindings(state).map((f) => `${f.role}/${f.findingId}`)
    ),
    carried: attempt(() =>
      carriedIgnores(state).map((f) => `${f.role}/${f.findingId}`)
    ),
    activeFix: attempt(() => activeReviewFix(state)?.id ?? null),
  };
}

/**
 * The state with every review baseline's diff bodies stored in `store` and
 * referenced by digest. Idempotent: a state without inline bodies is returned
 * as parsed. Throws, storing nothing in the state, when any preserved identity
 * would change.
 */
export async function compactState(
  current: LifecycleState,
  store: DiffStore,
): Promise<{ state: LifecycleState; changed: boolean }> {
  const legacy = StateSchema.parse(current);
  if (!hasInlineDiffs(legacy)) return { state: legacy, changed: false };
  const draft = structuredClone(legacy);
  for (const slot of reviewSlots(draft)) {
    const review = slot.get(draft);
    if (!review?.baseline || !isInlineBaseline(review.baseline)) continue;
    slot.set(draft, {
      ...review,
      baseline: await compactBaseline(store, review.baseline),
    } as Review);
  }
  // Hash exactly what a later load sees: Zod emits keys in schema order.
  const compact = StateSchema.parse(draft);
  const aliases: Record<string, string> = {
    ...(legacy.fingerprintAliases ?? {}),
  };
  const alias = (compactDigest: string, fingerprint: string) => {
    if (compactDigest !== fingerprint) aliases[compactDigest] = fingerprint;
  };
  const legacySlots = reviewSlots(legacy);
  reviewSlots(compact).forEach((slot, index) => {
    const before = legacySlots[index].get(legacy);
    const after = slot.get(compact);
    if (!before || !after) return;
    alias(
      implementationReviewDigest(compact, slot.role, after),
      implementationReviewFingerprint(legacy, slot.role, before),
    );
  });
  if (compact.codeReview) {
    alias(
      codeReviewDigest(compact),
      aliasedFingerprint(legacy, codeReviewDigest(legacy)),
    );
    alias(
      codeReviewDigest({ ...compact, uxReview: undefined }),
      aliasedFingerprint(
        legacy,
        codeReviewDigest({ ...legacy, uxReview: undefined }),
      ),
    );
  }
  const state = StateSchema.parse({
    ...compact,
    ...(Object.keys(aliases).length ? { fingerprintAliases: aliases } : {}),
  });
  if (
    canonical(continuityFacts(state)) !== canonical(continuityFacts(legacy))
  ) {
    throw new Error(
      "Compaction would change review fingerprints, finding identities or decisions; the state was not compacted",
    );
  }
  return { state, changed: true };
}
