import { createHash } from "node:crypto";
import { z } from "npm:zod@4.6.7";

const Text = z.string().trim().min(1);
const Commit = z.string().regex(/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/);
export const UserRequestSchema = z.string().min(1).refine(
  (request) => !!request.trim(),
  "Too small: request must not be blank",
);
const Workspace = z.string().regex(/^[a-z0-9][a-z0-9_-]{0,63}$/);

export const IssueSchema = z.object({
  workspace: Workspace,
  itemId: z.number().int().positive().safe(),
  text: z.string().min(1),
  note: z.string(),
  digest: z.string().regex(/^[0-9a-f]{64}$/),
}).strict();
export type Issue = z.infer<typeof IssueSchema>;

export const FindingSchema = z.object({
  id: Text,
  severity: z.enum(["low", "medium", "high", "critical"]),
  description: Text,
  resolution: Text.optional(),
  requirement: Text.optional(),
  mechanism: Text.optional(),
  evidence: z.array(Text).optional(),
  reopening: z.object({ rationale: Text, evidence: z.array(Text).min(1) })
    .strict().optional(),
}).strict();
const Severities = FindingSchema.shape.severity.options;
const FindingsSchema = z.array(FindingSchema).refine(
  (findings) =>
    new Set(findings.map((finding) => finding.id)).size === findings.length,
  "Finding IDs must be unique",
);
export const RevisionSchema = z.object({
  summary: Text,
  responses: z.array(
    z.object({
      findingId: Text,
      status: z.enum(["addressed", "disputed", "unresolved"]),
      rationale: Text,
      evidence: z.array(Text).min(1),
    }).strict(),
  ).refine(
    (responses) =>
      new Set(responses.map((r) => r.findingId)).size === responses.length,
    "Respond to each finding only once",
  ),
  acceptedDecisions: z.array(Text),
}).strict();
const CheckId = z.string().regex(
  /^[a-z0-9][a-z0-9_-]{0,47}$/,
  "Check IDs use lowercase letters, digits, '-' and '_' (at most 48 characters)",
);
const Argument = z.string().refine(
  (value) => !value.includes("\0"),
  "Command arguments cannot contain NUL",
);
/** One deterministic development check. The lifecycle runs argv verbatim, without a shell. */
export const CheckSchema = z.object({
  id: CheckId,
  command: z.array(Argument).min(1).refine(
    (argv) => (argv[0] ?? "x").trim().length > 0,
    "The command's executable must be nonempty",
  ),
  covers: z.array(Text).min(1),
}).strict();
export type Check = z.infer<typeof CheckSchema>;
const PlanBaseSchema = z.object({
  summary: Text,
  steps: z.array(
    z.object({
      id: Text,
      description: Text,
      files: z.array(Text),
    }).strict(),
  ).min(1).refine(
    (steps) => new Set(steps.map((step) => step.id)).size === steps.length,
    "Plan step IDs must be unique",
  ),
  testingStrategy: Text,
  assumptions: z.array(Text).optional(),
  revision: RevisionSchema.optional(),
  // Optional only for plans recorded before structured checks existed. Never
  // empty: isLegacyPlan treats any array as verifiable.
  checks: z.array(CheckSchema).min(1).optional(),
}).strict();
const CheckedPlanSchema = PlanBaseSchema.extend({
  checks: z.array(CheckSchema).min(1),
});
function validateChecks(
  plan: z.infer<typeof CheckedPlanSchema>,
  context: z.RefinementCtx,
) {
  const ids = new Set(plan.checks.map((check) => check.id));
  if (ids.size !== plan.checks.length) {
    context.addIssue({
      code: "custom",
      path: ["checks"],
      message: "Check IDs must be unique",
    });
  }
  const steps = new Set(plan.steps.map((step) => step.id));
  for (const [index, check] of plan.checks.entries()) {
    const unknown = check.covers.filter((id) => !steps.has(id));
    if (unknown.length) {
      context.addIssue({
        code: "custom",
        path: ["checks", index, "covers"],
        message: `Check ${check.id} covers unknown plan steps: ${
          unknown.join(", ")
        }`,
      });
    }
  }
}
/** New plans must declare the checks the lifecycle runs before merge authorization. */
const PlanContentSchema = CheckedPlanSchema.superRefine(validateChecks);
/** Result contract for a plan revision dispatched with the current worker contract. */
export const RevisionPlanSchema = CheckedPlanSchema.extend({
  revision: RevisionSchema,
}).superRefine(validateChecks);
export const PlanSchema = PlanBaseSchema.extend({
  version: z.number().int().positive(),
});
export type Plan = z.infer<typeof PlanSchema>;

/** Plans recorded before structured checks keep the reviewer-reported gate. */
export function isLegacyPlan(plan: Plan): boolean {
  return !plan.checks;
}
const PlanReviewSchema = z.object({
  planVersion: z.number().int().positive(),
  findings: FindingsSchema,
}).strict();
const DispositionSchema = z.object({
  findingId: Text,
  disposition: z.enum(["resolved", "rejected", "downgraded"]),
  rationale: Text,
  evidence: z.array(Text).min(1),
  severity: FindingSchema.shape.severity.optional(),
}).strict().refine(
  (d) => d.disposition === "downgraded" ? !!d.severity : !d.severity,
  "Only downgrades require a corrected severity",
);
export const AdjudicationSchema = z.object({
  planVersion: z.number().int().positive(),
  reviewFingerprint: z.string().regex(/^[0-9a-f]{64}$/),
  dispositions: z.array(DispositionSchema).min(1).refine(
    (ds) => new Set(ds.map((d) => d.findingId)).size === ds.length,
    "Disposition IDs must be unique",
  ),
}).strict();
const PlanningContextSchema = z.object({
  generation: z.number().int().nonnegative(),
  initialPlanVersion: z.number().int().nonnegative(),
  automaticRevisionConsumed: z.boolean(),
  userFeedback: z.array(
    z.object({
      request: z.string().min(1),
      planVersion: z.number().int().nonnegative(),
    }).strict(),
  ),
  history: z.array(
    z.object({
      generation: z.number().int().nonnegative(),
      plan: PlanSchema,
      review: PlanReviewSchema.optional(),
      reviewFingerprint: z.string().optional(),
      adjudications: z.array(AdjudicationSchema),
    }).strict(),
  ),
  adjudications: z.array(AdjudicationSchema),
  acceptedDecisions: z.array(Text),
}).strict();
const ApprovalSchema = z.object({
  planVersion: z.number().int().positive(),
  request: UserRequestSchema,
}).strict();
const ImplementationSchema = z.object({
  worktree: Text,
  branch: Text,
  head: Commit.optional(),
});
const CodeReviewSchema = z.object({
  planVersion: z.number().int().positive(),
  commit: Commit,
  steps: z.array(
    z.object({
      id: Text,
      status: z.enum(["implemented", "deviated"]),
      justification: Text.optional(),
    }).strict(),
  ),
  findings: FindingsSchema,
  preliminaryChecks: z.array(
    z.object({
      command: Text,
      result: z.enum(["passed", "failed", "skipped"]),
      summary: Text,
    }).strict(),
  ).optional(),
}).strict();
/** UX review deliberately has no technical per-step conformance field. */
export const UxReviewSchema = CodeReviewSchema.pick({
  planVersion: true,
  commit: true,
  findings: true,
  preliminaryChecks: true,
}).extend({
  assessment: z.object({
    impact: z.enum(["material", "no_material_impact"]),
    summary: Text,
    assessedPaths: z.array(Text).min(1),
    limitations: z.array(Text),
  }).strict(),
}).strict();
const FindingRefSchema = z.object({
  role: z.enum(["review_code", "review_ux"]),
  findingId: Text,
}).strict();
const FindingRefsSchema = z.array(FindingRefSchema).refine(
  (refs) =>
    new Set(refs.map((ref) => JSON.stringify(ref))).size === refs.length,
  "Finding references must be unique",
);
const ReviewDecisionSchema = z.object({
  id: z.uuid(),
  reviewFingerprint: z.string().regex(/^[0-9a-f]{64}$/),
  request: UserRequestSchema,
  repair: FindingRefsSchema,
  ignore: FindingRefsSchema,
}).strict();
const RepairPlanSchema = CheckedPlanSchema.omit({ revision: true }).superRefine(
  validateChecks,
);
const ReviewFixSchema = ReviewDecisionSchema.extend({
  planVersion: z.number().int().positive(),
  commit: Commit,
  // Preserve the source reviews across repairs and subsequent review rounds.
  codeReview: CodeReviewSchema,
  uxReview: UxReviewSchema,
  plan: RepairPlanSchema.optional(),
  approval: UserRequestSchema.optional(),
  result: z.object({ summary: Text, commit: Commit }).strict().optional(),
}).strict();
const CheckResultSchema = CheckSchema.extend({
  // Null when the executable could not be started.
  exitCode: z.number().int().nullable(),
  durationMs: z.number().int().nonnegative(),
  // Exactly one of: the stored output, or why the check was not run.
  log: z.string().min(1).optional(),
  skipped: Text.optional(),
  worktreeUnchanged: z.boolean(),
  // Set when the Git helper stopped the check at its time limit.
  timedOut: z.literal(true).optional(),
  status: z.enum(["passed", "failed"]),
}).strict().refine(
  (result) => (result.log === undefined) !== (result.skipped === undefined),
  "A check result has either a log or a skip reason",
).refine(
  (result) => !result.timedOut || result.exitCode === null,
  "A timed-out check has no exit code",
).refine(
  (result) => !result.skipped || result.status === "failed",
  "A skipped check cannot pass",
);
/** One lifecycle-run verification of the plan's checks at an exact commit. */
export const VerificationSchema = z.object({
  id: z.uuid(),
  planVersion: z.number().int().positive(),
  commit: Commit,
  branch: Text,
  startedAt: z.iso.datetime(),
  finishedAt: z.iso.datetime(),
  status: z.enum(["passed", "failed"]),
  checks: z.array(CheckResultSchema).min(1),
}).strict();
export type Verification = z.infer<typeof VerificationSchema>;

/**
 * The deterministic evidence a merge of the reviewed commit requires. `ready`,
 * `drive` and the handoff summary all read this, so they cannot disagree.
 */
export type MergeGate =
  | { kind: "legacy"; failedPreliminary: boolean }
  | { kind: "unverified" }
  | { kind: "failed"; verification: Verification }
  | { kind: "passed"; verification: Verification };

export function mergeGate(
  plan: Plan,
  review: Pick<
    z.infer<typeof CodeReviewSchema>,
    "commit" | "preliminaryChecks"
  >,
  verification: Verification | undefined,
  uxReview?: Pick<z.infer<typeof UxReviewSchema>, "preliminaryChecks">,
): MergeGate {
  if (isLegacyPlan(plan)) {
    return {
      kind: "legacy",
      failedPreliminary: [review, uxReview].some((r) =>
        r?.preliminaryChecks?.some((check) => check.result === "failed")
      ),
    };
  }
  if (
    !verification || verification.commit !== review.commit ||
    verification.planVersion !== plan.version
  ) {
    return { kind: "unverified" };
  }
  return { kind: verification.status, verification };
}

export function failedCheckIds(verification: Verification): string[] {
  return verification.checks
    .filter((check) => check.status === "failed")
    .map((check) => check.id);
}
export const AttemptSchema = z.object({
  id: z.uuid(),
  originalHead: Commit,
  skipCiRequest: Text.optional(),
  startedAt: z.iso.datetime(),
}).strict();
/** One failed merge attempt, kept as a fact across repair, rollback and later merges. */
export const MergeFailureSchema = z.object({
  attemptId: z.uuid(),
  at: z.iso.datetime(),
  originalCommit: Commit,
  summary: Text,
  failure: Text,
  ciStatus: Text,
  // The merge-gate log under the repository's lifecycle directory, when CI ran.
  log: Text.optional(),
  // The Swamp copy of the helper's output for the whole attempt.
  logResource: Text,
  exitCode: z.number().int().nullable().optional(),
  phases: z.array(
    z.object({ name: Text, status: z.enum(["passed", "failed"]) }).strict(),
  ),
  failedTests: z.array(Text),
  failedTestsTruncated: z.number().int().positive().optional(),
}).strict();
export type MergeFailure = z.infer<typeof MergeFailureSchema>;
export const PreviousAttemptSchema = z.object({
  attemptId: z.uuid(),
  summary: Text,
  log: Text.optional(),
}).strict();
export const ReceiptSchema = z.object({
  attemptId: z.uuid(),
  originalCommit: Commit,
  rebasedCommit: Commit,
  baseCommit: Commit,
  mergedCommit: Commit,
  worktree: Text,
  branch: Text,
  ciStatus: z.enum(["passed", "skipped"]),
  skipCiRequest: Text.optional(),
  mergedAt: z.iso.datetime(),
  previousAttempts: z.array(PreviousAttemptSchema).optional(),
}).strict();
export const CloseoutFenceSchema = z.object({
  server_instance: Text,
  pane_id: z.string().regex(/^terminal_[0-9]+$/),
  provider: Text,
  conversation: Text,
  input_revision: z.number().int().nonnegative().safe(),
  turn: Text,
}).strict();
export const CloseoutOriginSchema = z.object({
  session: Text,
  paneId: z.string().regex(/^terminal_[0-9]+$/),
  server_instance: Text,
  provider: Text,
  conversation: Text,
}).strict();
const FileIdentitySchema = z.object({ device: Text, inode: Text }).strict();
export const CloseoutTargetSchema = z.object({
  worktree: Text,
  branch: Text,
  head: Commit,
  gitDir: Text,
  branchIdentity: Text,
  directoryIdentity: FileIdentitySchema,
  registrationIdentity: FileIdentitySchema,
  linkIdentity: FileIdentitySchema,
  gitLink: z.string().min(1),
  owner: z.object({ issueKey: Text, worktree: Text, branch: Text }).strict(),
}).strict();
export const CloseoutGitResultSchema = z.object({
  cleanupId: z.uuid(),
  target: CloseoutTargetSchema,
  worktree: z.enum(["pending", "removing", "removed"]),
  branch: z.enum(["pending", "deleting", "removed"]),
}).strip();
export const CloseoutSchema = z.object({
  id: z.uuid(),
  offerId: z.uuid(),
  issueDigest: Text,
  attemptId: z.uuid(),
  rollbackId: z.uuid().optional(),
  origin: CloseoutOriginSchema,
  target: CloseoutTargetSchema,
  fence: CloseoutFenceSchema,
  decision: z.enum(["offered", "declined", "accepted", "invalidated"]),
  request: UserRequestSchema.optional(),
  authorizedRequest: UserRequestSchema.optional(),
  git: z.enum(["pending", "complete"]),
  gitResult: CloseoutGitResultSchema.optional(),
  pane: z.enum(["pending", "unknown", "closed", "absent", "expired"]),
  failure: z.object({ stage: z.enum(["git", "pane"]), message: Text }).strict()
    .optional(),
}).strict();
export const StateSchema = z.object({
  issue: IssueSchema,
  // Absent only in legacy states, which retain implementation/control selection.
  workerWorktree: Text.optional(),
  closeout: CloseoutSchema.optional(),
  closeoutHistory: z.array(CloseoutSchema).optional(),
  phase: z.enum([
    "triaging",
    "classified",
    "planning",
    "approved",
    "implementing",
    "ready",
    "merged",
    "done",
  ]),
  planVersion: z.number().int().nonnegative(),
  planningContext: PlanningContextSchema.optional(),
  // Fences evidence from an earlier attempt even when rollback restores the same phase.
  rollbackId: z.uuid().optional(),
  classification: z.object({
    classification: z.enum(["bug", "feature", "maintenance", "security"]),
    summary: Text,
  }).optional(),
  plan: PlanSchema.optional(),
  planReview: PlanReviewSchema.optional(),
  approval: ApprovalSchema.optional(),
  implementation: ImplementationSchema.optional(),
  codeReview: CodeReviewSchema.optional(),
  uxReview: UxReviewSchema.optional(),
  reviewDecisions: z.array(ReviewFixSchema).optional(),
  verification: VerificationSchema.optional(),
  mergeAuthorization: z.object({
    commit: Commit,
    planVersion: z.number().int().positive(),
    request: UserRequestSchema,
  }).optional(),
  attempt: AttemptSchema.optional(),
  receipt: ReceiptSchema.optional(),
  mergeFailures: z.array(MergeFailureSchema).optional(),
  outcome: z.object({ outcomeMet: z.literal(true), summary: Text }).optional(),
  lastFailure: Text.optional(),
});
export type LifecycleState = z.infer<typeof StateSchema>;

/** Lazily upgrade live state only; saved snapshots and prompts remain untouched. */
export function planningContext(state: LifecycleState) {
  return state.planningContext ??= {
    generation: 0,
    initialPlanVersion: 0,
    automaticRevisionConsumed: state.planVersion > 1,
    userFeedback: [],
    history: [],
    adjudications: [],
    acceptedDecisions: [],
  };
}
/** Record a user's planning instruction once per plan version. */
export function recordFeedback(
  state: LifecycleState,
  request: string,
  planVersion: number,
): void {
  const { userFeedback } = planningContext(state);
  if (
    !userFeedback.some((feedback) =>
      feedback.request === request && feedback.planVersion === planVersion
    )
  ) userFeedback.push({ request, planVersion });
}
export function planningGeneration(state: LifecycleState): string {
  return `${state.issue.digest}:${state.planningContext?.generation ?? 0}`;
}
export function initialPlanning(state: LifecycleState): boolean {
  return state.planVersion === (state.planningContext?.initialPlanVersion ?? 0);
}
export function archivePlanning(state: LifecycleState): void {
  const context = planningContext(state);
  if (!state.plan) return;
  const entry = {
    generation: context.generation,
    plan: state.plan,
    ...(state.planReview
      ? {
        review: state.planReview,
        reviewFingerprint: reviewFingerprint(state),
      }
      : {}),
    adjudications: context.adjudications.filter((a) =>
      a.planVersion === state.plan!.version
    ),
  };
  const samePlan = (old: (typeof context.history)[number]) =>
    old.generation === entry.generation &&
    JSON.stringify(old.plan) === JSON.stringify(entry.plan);
  if (!entry.review && context.history.some(samePlan)) return;
  // Keep one entry per plan and review: a reviewed entry supersedes its
  // unreviewed one, and later adjudications extend it instead of duplicating it.
  const index = context.history.findIndex((old) =>
    samePlan(old) &&
    (!old.review || JSON.stringify(old.review) === JSON.stringify(entry.review))
  );
  if (index >= 0) context.history[index] = structuredClone(entry);
  else context.history.push(structuredClone(entry));
}
export function reviewFingerprint(state: LifecycleState): string {
  return createHash("sha256").update(JSON.stringify({
    generation: planningGeneration(state),
    plan: state.plan,
    review: state.planReview,
  })).digest("hex");
}
function applyDispositions(
  findings: z.infer<typeof FindingsSchema>,
  adjudications: z.infer<typeof AdjudicationSchema>[],
  fingerprint?: string,
) {
  const dispositions = adjudications.filter((a) =>
    a.reviewFingerprint === fingerprint
  ).flatMap((a) => a.dispositions);
  // Dispositions accumulate: a later downgrade never erases an earlier resolution.
  return findings.map((finding) =>
    dispositions.filter((d) => d.findingId === finding.id).reduce(
      (current, d) =>
        d.disposition === "downgraded"
          ? { ...current, severity: d.severity! }
          : { ...current, resolution: d.rationale },
      finding,
    )
  );
}
export function effectivePlanFindings(state: LifecycleState) {
  return applyDispositions(
    state.planReview?.findings ?? [],
    state.planningContext?.adjudications ?? [],
    reviewFingerprint(state),
  );
}
/** Latest review in this generation, including after a planning rollback. */
export function previousPlanFindings(state: LifecycleState) {
  if (state.planReview) return effectivePlanFindings(state);
  const prior = state.planningContext?.history.findLast((h) =>
    h.generation === state.planningContext?.generation && h.review
  );
  return prior
    ? applyDispositions(
      prior.review!.findings,
      prior.adjudications,
      prior.reviewFingerprint,
    )
    : [];
}
export function validatePlanReview(
  state: LifecycleState,
  review: z.infer<typeof PlanReviewSchema>,
) {
  const generation = state.planningContext?.generation ?? 0;
  const history = [
    ...(state.planningContext?.history ?? []).filter((h) =>
      h.generation === generation
    ),
    ...(state.planReview
      ? [{
        review: state.planReview,
        reviewFingerprint: reviewFingerprint(state),
        adjudications: state.planningContext?.adjudications ?? [],
      }]
      : []),
  ];
  for (const finding of review.findings) {
    if (
      ["high", "critical"].includes(finding.severity) && !finding.resolution &&
      (!finding.requirement || !finding.mechanism || !finding.evidence?.length)
    ) {
      throw new Error(
        `Blocking finding ${finding.id} needs a requirement, failure mechanism and source evidence`,
      );
    }
    let seen = false;
    let closed = false;
    let downgraded = false;
    let severity: (typeof Severities)[number] | undefined;
    const oldEvidence: string[] = [];
    for (const entry of history) {
      if (!entry.review) continue;
      const prior = applyDispositions(
        entry.review.findings,
        entry.adjudications,
        entry.reviewFingerprint,
      )
        .find((f) => f.id === finding.id);
      if (prior) {
        seen = true;
        closed = !!prior.resolution;
        severity = prior.severity;
        downgraded ||= prior.severity !==
          entry.review.findings.find((f) => f.id === finding.id)!.severity;
        oldEvidence.push(
          ...(prior.evidence ?? []),
          ...(prior.reopening?.evidence ?? []),
        );
      } else if (seen) closed = true;
      oldEvidence.push(...entry.adjudications.flatMap((a) =>
        a.dispositions
          .filter((d) => d.findingId === finding.id).flatMap((d) => d.evidence)
      ));
    }
    // Escalating an open finding is ordinary review; only exceeding a
    // controller downgrade reopens its disposition.
    const raised = downgraded && severity !== undefined &&
      Severities.indexOf(finding.severity) > Severities.indexOf(severity);
    if (
      (closed || raised) && !finding.resolution &&
      (!finding.reopening ||
        !finding.reopening.evidence.some((e) => !oldEvidence.includes(e)))
    ) {
      throw new Error(
        `Reopening ${finding.id} requires new evidence and a rationale addressing its prior disposition`,
      );
    }
  }
}

export const LifecycleStepSchema = z.enum([
  "triage",
  "plan",
  "review_plan",
  "approve",
  "implement",
  "review_code",
  "review_ux",
  "ready",
  "merge",
  "summarize",
]);
export const RollbackInputSchema = z.object({
  step: LifecycleStepSchema,
  request: UserRequestSchema,
}).strict();

/** Rewind decisions, not source files or Git refs. Earlier evidence remains required. */
export function rollbackState(
  current: LifecycleState,
  step: z.infer<typeof LifecycleStepSchema>,
  id: string,
): LifecycleState {
  const state = StateSchema.parse(current);
  // Revoke this generation's consent, retaining the operation UUID, authorization
  // history and even uncertain effects for guarded recovery after re-completion.
  if (state.closeout) state.closeout.decision = "invalidated";
  const index = LifecycleStepSchema.options.indexOf(
    LifecycleStepSchema.parse(step),
  );
  const required = [
    state.classification,
    state.plan,
    state.planReview,
    state.approval,
    state.implementation,
    state.codeReview,
    state.uxReview,
    state.mergeAuthorization,
    state.receipt,
  ];
  if (index >= 8 && state.receipt) {
    archivePlanning(state);
    state.rollbackId = z.uuid().parse(id);
    delete state.outcome;
    delete state.lastFailure;
    delete state.attempt;
    state.phase = "merged";
    return state;
  }
  for (let previous = 0; previous < index; previous++) {
    if (!required[previous]) {
      throw new Error(
        `Cannot restart ${step}: missing ${
          LifecycleStepSchema.options[previous]
        } evidence`,
      );
    }
  }
  if (
    step === "review_ux" &&
    (state.codeReview?.planVersion !== state.plan?.version ||
      state.approval?.planVersion !== state.plan?.version ||
      state.implementation?.head !== state.codeReview?.commit)
  ) {
    throw new Error(
      "Restart review_code first: technical evidence no longer matches the approved implementation",
    );
  }
  if (index >= 7) requireCurrentReviews(state, state.implementation?.head);
  archivePlanning(state);
  state.rollbackId = z.uuid().parse(id);
  delete state.outcome;
  delete state.lastFailure;
  delete state.attempt;
  delete state.receipt;
  if (index <= 7) delete state.mergeAuthorization;
  if (index <= 6) {
    delete state.uxReview;
    delete state.verification;
  }
  if (index <= 5) {
    delete state.codeReview;
    delete state.verification;
    if (state.implementation) delete state.implementation.head;
  }
  if (index <= 3) delete state.approval;
  if (index <= 2) delete state.planReview;
  if (index <= 1) delete state.plan;
  if (index === 0) delete state.classification;
  // Keep the worktree/branch binding, including dirty work, for subsequent attempts.
  const phases = [
    "triaging",
    "classified",
    "planning",
    "planning",
    "approved",
    "implementing",
    "implementing",
    "implementing",
    "ready",
    "merged",
  ] as const;
  state.phase = step === "implement" && state.implementation
    ? "implementing"
    : phases[index];
  return StateSchema.parse(state);
}

/** The behavior adapter and pure transition layer share the same public input contract. */
export const ActionSchemas = {
  triage: StateSchema.shape.classification.unwrap().strict(),
  plan: PlanContentSchema,
  review_plan: PlanReviewSchema,
  adjudicate_plan: AdjudicationSchema,
  approve: ApprovalSchema,
  implement: ImplementationSchema.omit({ head: true }).strict(),
  review_code: CodeReviewSchema,
  review_ux: UxReviewSchema,
  decide_review: ReviewDecisionSchema,
  plan_review_fixes: RepairPlanSchema,
  approve_review_fixes: z.object({
    id: z.uuid(),
    handoffId: z.uuid(),
    request: UserRequestSchema,
  }).strict(),
  implement_review_fixes: z.object({ summary: Text, commit: Commit }).strict(),
  verify: z.object({ verification: VerificationSchema }).strict(),
  ready: z.object({ commit: Commit, mergeRequest: Text }).strict(),
  begin_merge: z.object({ attempt: AttemptSchema }).strict(),
  repair: z.object({ reason: Text }).strict(),
  merged: z.object({ receipt: ReceiptSchema }).strict(),
  done: StateSchema.shape.outcome.unwrap().strict(),
};
export type LifecycleAction = keyof typeof ActionSchemas;

export function createState(
  issue: Issue,
  workerWorktree?: string,
): LifecycleState {
  return StateSchema.parse({
    issue,
    workerWorktree,
    phase: "triaging",
    planVersion: 0,
  });
}

function requirePhase(
  state: LifecycleState,
  phases: LifecycleState["phase"][],
): void {
  if (!phases.includes(state.phase)) {
    throw new Error(
      `Operation requires ${phases.join(" or ")}; issue is ${state.phase}`,
    );
  }
}

function requirePlan(state: LifecycleState, version?: number): Plan {
  if (
    !state.plan || (version !== undefined && state.plan.version !== version)
  ) {
    throw new Error("Evidence must reference the current plan version");
  }
  return state.plan;
}

function requireResolved(findings: z.infer<typeof FindingsSchema>): void {
  const unresolved = findings.filter((finding) =>
    ["critical", "high"].includes(finding.severity) && !finding.resolution
  );
  if (unresolved.length) {
    throw new Error(
      `Resolve high/critical findings first: ${
        unresolved.map((f) => f.id).join(", ")
      }`,
    );
  }
}

/** All unlanded advancement paths require the same pair of current reviews. */
export function requireCurrentReviews(
  state: LifecycleState,
  commit?: string,
): void {
  const plan = requirePlan(state);
  const technical = state.codeReview;
  const ux = state.uxReview;
  if (
    !technical || technical.planVersion !== plan.version ||
    state.approval?.planVersion !== plan.version ||
    state.implementation?.head !== technical.commit ||
    (commit !== undefined && commit !== technical.commit)
  ) {
    throw new Error(
      "Current code review must match the reviewed commit and approved plan",
    );
  }
  if (
    !ux || ux.planVersion !== plan.version || ux.commit !== technical.commit
  ) {
    throw new Error(
      "Current UX review must match the reviewed commit and approved plan; rollback to review_ux and drive, or repair an unlanded active attempt first",
    );
  }
}

/** Decisions are scoped to the exact pair of reviews, requirements and rollback. */
export function codeReviewFingerprint(state: LifecycleState): string {
  requireCurrentReviews(state);
  return createHash("sha256").update(JSON.stringify({
    digest: state.issue.digest,
    rollbackId: state.rollbackId,
    plan: state.plan,
    technical: state.codeReview,
    ux: state.uxReview,
  })).digest("hex");
}
export function outstandingReviewFindings(state: LifecycleState) {
  return (["review_code", "review_ux"] as const).flatMap((role) =>
    (role === "review_code" ? state.codeReview : state.uxReview)?.findings
      .filter((finding) => !finding.resolution)
      .map((finding) => ({ role, findingId: finding.id, finding })) ?? []
  );
}
export function activeReviewFix(state: LifecycleState) {
  const decision = state.reviewDecisions?.at(-1);
  return decision?.repair.length && !decision.result && state.codeReview &&
      state.uxReview &&
      decision.reviewFingerprint === codeReviewFingerprint(state)
    ? decision
    : undefined;
}
export function requireReviewAcceptance(state: LifecycleState) {
  requireCurrentReviews(state);
  const fingerprint = codeReviewFingerprint(state);
  const ignored = (state.reviewDecisions ?? []).filter((d) =>
    d.reviewFingerprint === fingerprint
  ).flatMap((d) => d.ignore);
  if (
    activeReviewFix(state) ||
    outstandingReviewFindings(state).some((ref) =>
      !ignored.some((item) =>
        item.role === ref.role && item.findingId === ref.findingId
      )
    )
  ) {
    throw new Error(
      "Present review findings and obtain the user's decision before continuing",
    );
  }
}
/** Original checks plus every approved repair's focused checks, with unique log IDs. */
export function verificationPlan(state: LifecycleState): Plan {
  const plan = requirePlan(state);
  const repairs = (state.reviewDecisions ?? []).filter((d) =>
    d.planVersion === plan.version && d.result && d.approval && d.plan
  );
  if (!repairs.length) return plan;
  return {
    ...plan,
    checks: [
      ...(plan.checks ?? []),
      ...repairs.flatMap((d) =>
        d.plan!.checks.map((check, index) => ({
          ...check,
          id: `${d.id}-${index}`,
        }))
      ),
    ],
  };
}

/** Repair checks augment the legacy reviewer gate rather than discarding failures. */
export function implementationMergeGate(state: LifecycleState): MergeGate {
  if (!state.codeReview) {
    throw new Error("Record a code review before verifying");
  }
  const original = mergeGate(
    requirePlan(state),
    state.codeReview,
    state.verification,
    state.uxReview,
  );
  if (original.kind === "legacy" && original.failedPreliminary) return original;
  return mergeGate(
    verificationPlan(state),
    state.codeReview,
    state.verification,
    state.uxReview,
  );
}

/** The recorded failure of the active merge attempt, if it has one. */
export function activeMergeFailure(
  state: LifecycleState,
): MergeFailure | undefined {
  const id = state.attempt?.id;
  return id
    ? state.mergeFailures?.find((failure) => failure.attemptId === id)
    : undefined;
}

export function describeMergeFailure(failure: MergeFailure): string {
  return `Merge attempt ${failure.attemptId} failed: ${failure.summary}` +
    (failure.log ? `; log: ${failure.log}` : "");
}

/**
 * Record the active attempt's failed receipt once. A retry of the same failed
 * attempt keeps the original evidence instead of its "create a new attempt" error.
 */
export function recordMergeFailure(
  current: LifecycleState,
  failure: MergeFailure,
): LifecycleState {
  const state = StateSchema.parse(current);
  const parsed = MergeFailureSchema.parse(failure);
  if (
    !state.attempt || parsed.attemptId !== state.attempt.id ||
    parsed.originalCommit !== state.attempt.originalHead
  ) {
    throw new Error("Merge failure must belong to the active attempt");
  }
  if (activeMergeFailure(state)) return state;
  state.mergeFailures = [...(state.mergeFailures ?? []), parsed];
  state.lastFailure = describeMergeFailure(parsed);
  return state;
}

function clearReview(state: LifecycleState): void {
  delete state.codeReview;
  delete state.uxReview;
  delete state.verification;
  delete state.mergeAuthorization;
  delete state.attempt;
  if (state.implementation) delete state.implementation.head;
}

/** A changed Agenda description invalidates every decision based on its old contents. */
export function refreshIssue(
  current: LifecycleState,
  issue: Issue,
): LifecycleState {
  const state = StateSchema.parse(current);
  const next = IssueSchema.parse(issue);
  if (
    next.workspace !== state.issue.workspace ||
    next.itemId !== state.issue.itemId
  ) {
    throw new Error("Cannot replace an issue with a different Agenda identity");
  }
  if (next.digest === state.issue.digest) return state;
  requirePhase(state, [
    "triaging",
    "classified",
    "planning",
    "approved",
    "implementing",
    "ready",
  ]);
  if (state.attempt) {
    throw new Error(
      "Repair the active merge attempt before refreshing its issue",
    );
  }
  archivePlanning(state);
  const context = planningContext(state);
  context.generation += 1;
  context.initialPlanVersion = state.planVersion;
  context.automaticRevisionConsumed = false;
  if (state.closeout) state.closeout.decision = "invalidated";
  state.issue = next;
  state.phase = "triaging";
  delete state.classification;
  delete state.plan;
  delete state.planReview;
  delete state.approval;
  delete state.lastFailure;
  clearReview(state);
  return state;
}

/** Pure transitions: callers persist the returned value after validating external effects. */
export function transition(
  current: LifecycleState,
  action: string,
  args: unknown,
): LifecycleState {
  const state = StateSchema.parse(current);
  if (state.attempt && !["merged", "repair"].includes(action)) {
    throw new Error("Finish or repair the active merge attempt first");
  }
  switch (action) {
    case "triage": {
      requirePhase(state, ["triaging", "classified"]);
      state.classification = ActionSchemas.triage.parse(args);
      state.phase = "classified";
      break;
    }
    case "plan": {
      requirePhase(state, [
        "classified",
        "planning",
        "approved",
        "implementing",
        "ready",
      ]);
      const content = ActionSchemas.plan.parse(args);
      archivePlanning(state);
      const context = planningContext(state);
      if (!initialPlanning(state)) context.automaticRevisionConsumed = true;
      if (content.revision) {
        context.acceptedDecisions = [
          ...new Set([
            ...context.acceptedDecisions,
            ...content.revision.acceptedDecisions,
          ]),
        ];
      }
      state.planVersion += 1;
      state.plan = { ...content, version: state.planVersion };
      state.phase = "planning";
      delete state.planReview;
      delete state.approval;
      delete state.lastFailure;
      clearReview(state);
      break;
    }
    case "review_plan": {
      requirePhase(state, ["planning"]);
      const review = ActionSchemas.review_plan.parse(args);
      requirePlan(state, review.planVersion);
      archivePlanning(state);
      state.planReview = review;
      break;
    }
    case "adjudicate_plan": {
      requirePhase(state, ["planning"]);
      const adjudication = AdjudicationSchema.parse(args);
      requirePlan(state, adjudication.planVersion);
      if (
        !state.planReview ||
        adjudication.reviewFingerprint !== reviewFingerprint(state)
      ) {
        throw new Error(
          "Adjudication must reference the current review fingerprint; stale evidence",
        );
      }
      if (
        state.planningContext?.adjudications.some((previous) =>
          JSON.stringify(previous) === JSON.stringify(adjudication)
        )
      ) break;
      const findings = effectivePlanFindings(state);
      for (const disposition of adjudication.dispositions) {
        const finding = findings.find((f) => f.id === disposition.findingId);
        if (!finding) {
          throw new Error(`Unknown finding ${disposition.findingId}`);
        }
        if (
          disposition.disposition === "downgraded" &&
          Severities.indexOf(disposition.severity!) >=
            Severities.indexOf(finding.severity)
        ) {
          throw new Error("A downgrade must lower the current severity");
        }
      }
      planningContext(state).adjudications.push(adjudication);
      break;
    }
    case "approve": {
      requirePhase(state, ["planning"]);
      const approval = ActionSchemas.approve.parse(args);
      requirePlan(state, approval.planVersion);
      if (
        !state.planReview ||
        state.planReview.planVersion !== approval.planVersion
      ) {
        throw new Error("The current plan needs a review before approval");
      }
      requireResolved(effectivePlanFindings(state));
      state.approval = approval;
      state.phase = "approved";
      break;
    }
    case "implement": {
      requirePhase(state, ["approved", "implementing"]);
      const implementation = ActionSchemas.implement.parse(args);
      if (
        state.closeout?.authorizedRequest &&
        state.closeout.target.worktree === implementation.worktree
      ) {
        throw new Error(
          "Closeout may have removed this worktree; deliberately select a new valid worktree before implementation",
        );
      }
      if (
        state.closeout?.authorizedRequest &&
        state.closeout.target.worktree !== implementation.worktree
      ) {
        state.workerWorktree = implementation.worktree;
        delete state.implementation;
      }
      const plan = requirePlan(state);
      if (state.approval?.planVersion !== plan.version) {
        throw new Error("Approve the current plan first");
      }
      if (
        state.workerWorktree && state.workerWorktree !== implementation.worktree
      ) {
        throw new Error(
          "Implementation must adopt the recorded worker worktree",
        );
      }
      if (
        state.implementation &&
        (state.implementation.worktree !== implementation.worktree ||
          state.implementation.branch !== implementation.branch)
      ) {
        throw new Error(
          "Implementation must remain in its existing worktree and branch",
        );
      }
      state.implementation = implementation;
      state.phase = "implementing";
      clearReview(state);
      break;
    }
    case "review_code": {
      requirePhase(state, ["implementing", "ready"]);
      const review = ActionSchemas.review_code.parse(args);
      const plan = requirePlan(state, review.planVersion);
      if (
        !state.implementation || state.approval?.planVersion !== plan.version
      ) {
        throw new Error(
          "Code review requires implementation of the approved plan",
        );
      }
      const ids = new Set(review.steps.map((step) => step.id));
      if (
        ids.size !== review.steps.length || ids.size !== plan.steps.length ||
        plan.steps.some((step) => !ids.has(step.id))
      ) {
        throw new Error("Code review must cover every plan step exactly once");
      }
      if (
        review.steps.some((step) =>
          step.status === "deviated" && !step.justification
        )
      ) {
        throw new Error("Each plan deviation needs a justification");
      }
      clearReview(state);
      state.codeReview = review;
      state.implementation.head = review.commit;
      state.phase = "implementing";
      break;
    }
    case "review_ux": {
      requirePhase(state, ["implementing", "ready"]);
      const review = ActionSchemas.review_ux.parse(args);
      const plan = requirePlan(state, review.planVersion);
      if (
        !state.codeReview || state.codeReview.planVersion !== plan.version ||
        state.codeReview.commit !== review.commit ||
        state.implementation?.head !== review.commit ||
        state.approval?.planVersion !== plan.version
      ) {
        throw new Error(
          "UX review requires current technical review of the same approved implementation commit",
        );
      }
      state.uxReview = review;
      delete state.verification;
      delete state.mergeAuthorization;
      delete state.attempt;
      state.phase = "implementing";
      break;
    }
    case "decide_review": {
      requirePhase(state, ["implementing", "ready"]);
      if (state.attempt) {
        throw new Error(
          "Repair or roll back the active merge attempt before deciding findings",
        );
      }
      requireCurrentReviews(state);
      const decision = ActionSchemas.decide_review.parse(args);
      if (decision.reviewFingerprint !== codeReviewFingerprint(state)) {
        throw new Error("Review decision is stale");
      }
      const existing = state.reviewDecisions?.find((d) => d.id === decision.id);
      if (existing) {
        if (
          JSON.stringify(ReviewDecisionSchema.strip().parse(existing)) !==
            JSON.stringify(decision)
        ) throw new Error("Review decision ID already used");
        break;
      }
      if (activeReviewFix(state)) {
        throw new Error(
          "Finish or roll back the selected review repairs first",
        );
      }
      const refs = [...decision.repair, ...decision.ignore];
      if (
        !refs.length ||
        new Set(refs.map((ref) => JSON.stringify(ref))).size !== refs.length
      ) throw new Error("Select distinct review findings to repair or ignore");
      const findings = outstandingReviewFindings(state);
      if (
        refs.some((ref) =>
          !findings.some((f) =>
            f.role === ref.role && f.findingId === ref.findingId
          )
        )
      ) throw new Error("Unknown or resolved review finding");
      state.reviewDecisions = [...(state.reviewDecisions ?? []), {
        ...decision,
        planVersion: state.plan!.version,
        commit: state.codeReview!.commit,
        codeReview: state.codeReview!,
        uxReview: state.uxReview!,
      }];
      delete state.mergeAuthorization;
      state.phase = "implementing";
      break;
    }
    case "plan_review_fixes": {
      requirePhase(state, ["implementing"]);
      const fix = activeReviewFix(state);
      if (!fix || fix.plan) {
        throw new Error("Select review findings before planning repairs");
      }
      fix.plan = ActionSchemas.plan_review_fixes.parse(args);
      break;
    }
    case "approve_review_fixes": {
      requirePhase(state, ["implementing"]);
      const approval = ActionSchemas.approve_review_fixes.parse(args);
      const fix = activeReviewFix(state);
      if (!fix?.plan || fix.id !== approval.id) {
        throw new Error("Approval must match the current review repair plan");
      }
      if (fix.approval && fix.approval !== approval.request) {
        throw new Error("Repair plan already approved");
      }
      fix.approval = approval.request;
      break;
    }
    case "implement_review_fixes": {
      requirePhase(state, ["implementing"]);
      const fix = activeReviewFix(state);
      if (!fix?.plan || !fix.approval) {
        throw new Error("Approve the review repair plan before implementation");
      }
      fix.result = ActionSchemas.implement_review_fixes.parse(args);
      clearReview(state);
      state.implementation!.head = fix.result.commit;
      break;
    }
    case "verify": {
      requirePhase(state, ["implementing"]);
      const { verification } = ActionSchemas.verify.parse(args);
      requirePlan(state, verification.planVersion);
      const plan = verificationPlan(state);
      requireReviewAcceptance(state);
      requireCurrentReviews(state, verification.commit);
      if (!plan.checks) {
        throw new Error(
          "This legacy plan has no structured checks to verify; revise the plan to add checks",
        );
      }
      if (
        !state.codeReview || state.codeReview.planVersion !== plan.version ||
        state.approval?.planVersion !== plan.version || !state.implementation
      ) {
        throw new Error(
          "Verification requires a code review of the approved plan's implementation",
        );
      }
      if (
        verification.commit !== state.codeReview.commit ||
        state.implementation.head !== verification.commit ||
        verification.branch !== state.implementation.branch
      ) {
        throw new Error(
          "Verification must run at the reviewed commit on the issue branch",
        );
      }
      if (
        verification.checks.length !== plan.checks.length ||
        verification.checks.some((result, index) => {
          const check = plan.checks![index];
          return result.id !== check.id ||
            JSON.stringify(result.command) !== JSON.stringify(check.command) ||
            JSON.stringify(result.covers) !== JSON.stringify(check.covers);
        })
      ) {
        throw new Error(
          "Verification must run every plan check exactly as approved, in order",
        );
      }
      const consistent =
        verification.checks.every((result) =>
          (result.status === "passed") ===
            (result.exitCode === 0 && result.worktreeUnchanged)
        ) &&
        (verification.status === "passed") ===
          verification.checks.every((result) => result.status === "passed");
      if (!consistent) {
        throw new Error(
          "Verification status must follow its recorded exit codes and worktree checks",
        );
      }
      state.verification = verification;
      break;
    }
    case "ready": {
      requirePhase(state, ["implementing"]);
      const ready = ActionSchemas.ready.parse(args);
      const plan = requirePlan(state);
      if (
        !state.codeReview || state.codeReview.planVersion !== plan.version ||
        state.codeReview.commit !== ready.commit ||
        state.implementation?.head !== ready.commit ||
        state.approval?.planVersion !== plan.version
      ) {
        throw new Error(
          "Merge authorization must match the reviewed commit and approved plan",
        );
      }
      requireCurrentReviews(state, ready.commit);
      if (activeReviewFix(state)) {
        throw new Error(
          "Finish or roll back selected review repairs before merge",
        );
      }
      const outstanding = outstandingReviewFindings(state);
      if (outstanding.length) {
        state.reviewDecisions = [...(state.reviewDecisions ?? []), {
          id: crypto.randomUUID(),
          reviewFingerprint: codeReviewFingerprint(state),
          request: ready.mergeRequest,
          repair: [],
          ignore: outstanding.map(({ role, findingId }) => ({
            role,
            findingId,
          })),
          planVersion: plan.version,
          commit: ready.commit,
          codeReview: state.codeReview!,
          uxReview: state.uxReview!,
        }];
      }
      const gate = implementationMergeGate(state);
      if (gate.kind === "legacy" && gate.failedPreliminary) {
        throw new Error(
          "Resolve failed preliminary checks before requesting a merge",
        );
      }
      if (gate.kind === "unverified") {
        throw new Error(
          "Run verify for the reviewed commit before requesting a merge",
        );
      }
      if (gate.kind === "failed") {
        throw new Error(
          `Verification failed for ${
            failedCheckIds(gate.verification).join(", ")
          }; repair and verify a new reviewed commit, or rerun verify`,
        );
      }
      state.mergeAuthorization = {
        commit: ready.commit,
        planVersion: plan.version,
        request: ready.mergeRequest,
      };
      state.phase = "ready";
      break;
    }
    case "begin_merge": {
      requirePhase(state, ["ready"]);
      const { attempt } = ActionSchemas.begin_merge.parse(args);
      if (
        !state.mergeAuthorization ||
        attempt.originalHead !== state.mergeAuthorization.commit
      ) {
        throw new Error("Merge attempt does not match the authorized commit");
      }
      requireCurrentReviews(state, attempt.originalHead);
      requireReviewAcceptance(state);
      state.attempt = attempt;
      delete state.lastFailure;
      break;
    }
    case "repair": {
      requirePhase(state, ["implementing", "ready"]);
      const { reason } = ActionSchemas.repair.parse(args);
      const failure = activeMergeFailure(state);
      clearReview(state);
      state.phase = "implementing";
      // The implement agent sees lastFailure; give it the gate's evidence too.
      state.lastFailure = failure
        ? `${reason}\n\n${describeMergeFailure(failure)}`
        : reason;
      break;
    }
    case "merged": {
      requirePhase(state, ["ready"]);
      const { receipt } = ActionSchemas.merged.parse(args);
      if (
        !state.attempt || receipt.attemptId !== state.attempt.id ||
        receipt.originalCommit !== state.attempt.originalHead ||
        receipt.originalCommit !== state.mergeAuthorization?.commit ||
        receipt.worktree !== state.implementation?.worktree ||
        receipt.branch !== state.implementation?.branch
      ) {
        throw new Error(
          "Merge receipt must match the active authorized attempt and implementation",
        );
      }
      if (receipt.mergedCommit !== receipt.rebasedCommit) {
        throw new Error("Merged commit must be the verified rebased commit");
      }
      if (
        receipt.ciStatus === "skipped" &&
        (!state.attempt.skipCiRequest ||
          receipt.skipCiRequest !== state.attempt.skipCiRequest)
      ) {
        throw new Error(
          "Skipped CI requires the explicit request recorded on this attempt",
        );
      }
      if (receipt.ciStatus === "passed" && receipt.skipCiRequest) {
        throw new Error(
          "Passed CI receipts cannot also claim a CI skip request",
        );
      }
      state.receipt = receipt;
      state.phase = "merged";
      delete state.attempt;
      delete state.lastFailure;
      break;
    }
    case "done": {
      requirePhase(state, ["merged"]);
      if (!state.receipt) {
        throw new Error("Completion requires a verified local merge receipt");
      }
      state.outcome = ActionSchemas.done.parse(args);
      state.phase = "done";
      break;
    }
    default:
      throw new Error(`Unknown lifecycle action: ${action}`);
  }
  return StateSchema.parse(state);
}

export async function digestIssue(
  issue: Pick<Issue, "workspace" | "itemId" | "text" | "note">,
): Promise<string> {
  // Stable lexicographic keys; excludes mutable classifications and completion state.
  const value = JSON.stringify({
    id: issue.itemId,
    note: issue.note,
    text: issue.text,
    workspace: issue.workspace,
  });
  const hash = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(value),
  );
  return Array.from(
    new Uint8Array(hash),
    (byte) => byte.toString(16).padStart(2, "0"),
  ).join("");
}

export type AgendaCommand = (args: string[]) => Promise<unknown>;

export async function agendaCommand(args: string[]): Promise<unknown> {
  const result = await new Deno.Command("agenda", {
    args,
    stdout: "piped",
    stderr: "piped",
  }).output();
  const decode = new TextDecoder();
  if (!result.success) {
    throw new Error(
      `Agenda exited ${result.code}: ${decode.decode(result.stderr).trim()}`,
    );
  }
  try {
    return JSON.parse(decode.decode(result.stdout));
  } catch (cause) {
    throw new Error("Agenda returned invalid JSON", { cause });
  }
}

const AssignmentSchema = z.object({
  category: z.number().int(),
  item: z.number().int(),
  source: z.string(),
  value: z.union([
    z.literal("standard"),
    z.object({ text: z.string() }),
    z.object({ number: z.number() }),
    z.object({
      date: z.object({
        temporal: z.object({ kind: z.enum(["floating", "zoned", "instant"]) })
          .passthrough(),
        recurrence: z.object({}).passthrough().nullable(),
      }),
    }),
  ]),
});
const AgendaItemSchema = z.object({
  id: z.number().int().positive().safe(),
  kind: z.enum(["task", "note"]),
  text: z.string().min(1),
  note: z.string(),
  completed: z.boolean(),
  blocked: z.boolean(),
  discarded_at: z.number().int().nullable(),
  assignments: z.array(AssignmentSchema),
});
const AgendaEnvelopeSchema = z.object({
  schema_version: z.literal(0),
  item: AgendaItemSchema,
});

async function agendaRecord(
  workspace: string,
  itemId: number,
  command: AgendaCommand,
) {
  Workspace.parse(workspace);
  z.number().int().positive().safe().parse(itemId);
  const envelope = AgendaEnvelopeSchema.parse(
    await command([
      "--workspace",
      workspace,
      "--json",
      "item",
      "show",
      String(itemId),
    ]),
  );
  if (envelope.item.id !== itemId) {
    throw new Error("Agenda returned a different item ID");
  }
  return envelope.item;
}

function validateAgendaItem(
  item: z.infer<typeof AgendaItemSchema>,
  allowCompleted: boolean,
): void {
  if (item.kind !== "task") {
    throw new Error("Agenda issues must be tasks, not reference notes");
  }
  if (item.discarded_at !== null) throw new Error("Agenda issue is trashed");
  if (item.completed && !allowCompleted) {
    throw new Error("Agenda issue is already completed");
  }
  if (item.blocked) {
    throw new Error("Agenda issue has unfinished prerequisites");
  }
  if (
    item.assignments.some(({ value }) =>
      typeof value === "object" && "date" in value &&
      value.date.recurrence !== null
    )
  ) throw new Error("Recurring Agenda tasks cannot be development issues");
}

export async function readAgenda(
  workspace: string,
  itemId: number,
  allowCompleted = false,
  command: AgendaCommand = agendaCommand,
): Promise<Issue> {
  const item = await agendaRecord(workspace, itemId, command);
  validateAgendaItem(item, allowCompleted);
  const issue = { workspace, itemId, text: item.text, note: item.note };
  return IssueSchema.parse({ ...issue, digest: await digestIssue(issue) });
}

/** Closeout requires completed, unchanged requirements, not just a done snapshot. */
export async function requireCompletedAgenda(
  issue: Issue,
  command: AgendaCommand = agendaCommand,
) {
  const item = await agendaRecord(issue.workspace, issue.itemId, command);
  validateAgendaItem(item, true);
  if (
    !item.completed ||
    await digestIssue({ ...issue, text: item.text, note: item.note }) !==
      issue.digest
  ) {
    throw new Error(
      "Closeout requires the completed Agenda task with unchanged requirements",
    );
  }
}

/** Rollback reopens a completed task without changing its requirements. Safe to retry. */
export async function reopenAgenda(
  issue: Issue,
  command: AgendaCommand = agendaCommand,
): Promise<void> {
  const item = await agendaRecord(issue.workspace, issue.itemId, command);
  validateAgendaItem(item, true);
  const digest = await digestIssue({
    ...issue,
    text: item.text,
    note: item.note,
  });
  if (digest !== issue.digest) {
    throw new Error(
      "Agenda description changed during rollback; inspect the recovery record",
    );
  }
  if (item.completed) {
    await command([
      "--workspace",
      issue.workspace,
      "--json",
      "item",
      "reopen",
      String(issue.itemId),
    ]);
  }
  const after = await readAgenda(issue.workspace, issue.itemId, false, command);
  if (after.digest !== issue.digest) {
    throw new Error(
      "Agenda description changed during rollback; inspect the recovery record",
    );
  }
}

/** Complete only the captured issue; retries preserve its description and verify the result. */
export async function completeAgenda(
  issue: Issue,
  command: AgendaCommand = agendaCommand,
): Promise<Issue> {
  const expected = IssueSchema.parse(issue);
  const current = await readAgenda(
    expected.workspace,
    expected.itemId,
    true,
    command,
  );
  if (current.digest !== expected.digest) {
    throw new Error("Agenda description changed before completion");
  }
  const completion = z.object({
    schema_version: z.literal(0),
    items: z.array(AgendaItemSchema),
    completion: z.object({
      completed: z.number().int().nonnegative(),
      reopened: z.number().int().nonnegative(),
      created: z.literal(0),
      created_items: z.array(AgendaItemSchema).length(0),
    }),
  }).parse(
    await command([
      "--workspace",
      expected.workspace,
      "--json",
      "item",
      "done",
      String(expected.itemId),
    ]),
  );
  if (
    completion.items.length !== 1 ||
    completion.items[0].id !== expected.itemId ||
    !completion.items[0].completed
  ) throw new Error("Agenda did not confirm issue completion");
  const after = await agendaRecord(
    expected.workspace,
    expected.itemId,
    command,
  );
  validateAgendaItem(after, true);
  if (!after.completed) {
    throw new Error("Agenda issue is still pending after completion");
  }
  const result = {
    workspace: expected.workspace,
    itemId: expected.itemId,
    text: after.text,
    note: after.note,
  };
  const digest = await digestIssue(result);
  if (digest !== expected.digest) {
    throw new Error(
      "Agenda description changed during completion; inspect the item",
    );
  }
  return IssueSchema.parse({ ...result, digest });
}
