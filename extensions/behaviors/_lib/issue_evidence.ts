/** The submission protocol shared by lifecycle controllers and Swamp workers. */
import { createHash } from "node:crypto";
import { z } from "npm:zod@4.6.7";
import {
  ActionSchemas,
  activeReviewFix,
  implementationReviewFingerprint,
  initialPlanning,
  planningGeneration,
  previousPlanFindings,
  priorImplementationFindings,
  recordFeedback,
  RepairResultSchema,
  RevisionPlanSchema,
  selectedRepairFindings,
  StateSchema,
  transition,
  validateImplementationReview,
  validatePlanReview,
  validateRepairMappings,
  validateRepairResponses,
} from "./lifecycle.ts";
import {
  ContinuityContext,
  RoundRecord,
  validateAdvisory,
} from "./review_continuity.ts";
import { AgentConfigSchema } from "./yinz_agent.ts";

const Digest = z.string().regex(/^[0-9a-f]{64}$/);
const Commit = z.string().regex(/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/);
export const RoleSchema = z.enum([
  "triage",
  "plan",
  "review_plan",
  "implement",
  "review_code",
  "review_ux",
  "plan_review_fixes",
  "implement_review_fixes",
]);
export const ImplementationResult = z.object({
  summary: z.string().trim().min(1),
  commit: Commit,
}).strict();
export const RoleSchemas = {
  triage: ActionSchemas.triage,
  plan: ActionSchemas.plan,
  review_plan: ActionSchemas.review_plan,
  implement: ImplementationResult,
  review_code: ActionSchemas.review_code,
  review_ux: ActionSchemas.review_ux,
  plan_review_fixes: ActionSchemas.plan_review_fixes,
  implement_review_fixes: RepairResultSchema,
};
export const RoleEvidenceSchema = z.discriminatedUnion("role", [
  z.object({ role: z.literal("triage"), evidence: RoleSchemas.triage })
    .strict(),
  z.object({ role: z.literal("plan"), evidence: RoleSchemas.plan }).strict(),
  z.object({
    role: z.literal("review_plan"),
    evidence: RoleSchemas.review_plan,
  })
    .strict(),
  z.object({ role: z.literal("implement"), evidence: RoleSchemas.implement })
    .strict(),
  z.object({
    role: z.literal("review_code"),
    evidence: RoleSchemas.review_code,
  })
    .strict(),
  z.object({ role: z.literal("review_ux"), evidence: RoleSchemas.review_ux })
    .strict(),
  z.object({
    role: z.literal("plan_review_fixes"),
    evidence: RoleSchemas.plan_review_fixes,
  }).strict(),
  z.object({
    role: z.literal("implement_review_fixes"),
    evidence: RoleSchemas.implement_review_fixes,
  }).strict(),
]);
export type RoleEvidence = z.infer<typeof RoleEvidenceSchema>;
const Envelope = { taskId: z.uuid(), assignmentDigest: Digest };
export const SubmitArgsSchema = z.discriminatedUnion("role", [
  RoleEvidenceSchema.options[0].extend(Envelope),
  RoleEvidenceSchema.options[1].extend(Envelope),
  RoleEvidenceSchema.options[2].extend(Envelope),
  RoleEvidenceSchema.options[3].extend(Envelope),
  RoleEvidenceSchema.options[4].extend(Envelope),
  RoleEvidenceSchema.options[5].extend(Envelope),
  RoleEvidenceSchema.options[6].extend(Envelope),
  RoleEvidenceSchema.options[7].extend(Envelope),
]);
export const EvidenceTargetSchema = z.object({
  protocol: z.literal(1),
  behaviorName: z.string().regex(/^issue-evidence-[0-9a-f-]{36}$/),
  controlDir: z.string().min(1),
  assignmentDigest: Digest,
}).strict();
const AssignmentCoreSchema = z.object({
  protocol: z.literal(1),
  controlDir: z.string().min(1),
  parentId: z.string().min(1),
  parentName: z.string().min(1),
  taskId: z.uuid(),
  agentBehavior: z.string().min(1),
  role: RoleSchema,
  config: AgentConfigSchema,
  before: StateSchema,
  generation: z.string().min(1),
  request: z.string().min(1).optional(),
  reviewCommit: Commit.optional(),
  contractVersion: z.literal(2).optional(),
  continuity: ContinuityContext.optional(),
  round: RoundRecord.optional(),
}).strict();
export const AssignmentSchema = AssignmentCoreSchema.extend({
  digest: Digest,
  promptDigest: Digest,
});
export type Assignment = z.infer<typeof AssignmentSchema>;
export const EvidenceReceiptSchema = z.object({
  taskId: z.uuid(),
  assignmentDigest: Digest,
  payloadDigest: Digest,
  acceptedAt: z.iso.datetime(),
  payload: RoleEvidenceSchema,
  next: StateSchema,
}).strict();
export type EvidenceReceipt = z.infer<typeof EvidenceReceiptSchema>;
export const SubmissionSchema = z.discriminatedUnion("status", [
  z.object({ status: z.literal("open"), assignment: AssignmentSchema })
    .strict(),
  z.object({
    status: z.literal("accepted"),
    assignment: AssignmentSchema,
    receipt: EvidenceReceiptSchema,
  }).strict(),
  z.object({
    status: z.literal("superseded"),
    assignment: AssignmentSchema,
    operationId: z.uuid(),
    receipt: EvidenceReceiptSchema.optional(),
  }).strict(),
]);

/** Sort object keys only; array ordering belongs to the submitted evidence. */
export function canonical(value: unknown): string {
  return JSON.stringify(
    value,
    (_key, item) =>
      item !== null && typeof item === "object" && !Array.isArray(item)
        ? Object.fromEntries(
          Object.entries(item).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0),
        )
        : item,
  );
}
export function evidenceDigest(value: unknown): string {
  return createHash("sha256").update(canonical(value)).digest("hex");
}

export type AssignmentTask = {
  id: string;
  behaviorName: string;
  role: z.infer<typeof RoleSchema>;
  config: z.infer<typeof AgentConfigSchema>;
  before: z.infer<typeof StateSchema>;
  request?: string;
  reviewCommit?: string;
  contractVersion?: 2;
  continuity?: z.infer<typeof ContinuityContext>;
  round?: z.infer<typeof RoundRecord>;
};
export function assignmentCore(
  parent: { modelId: string; definition: { name: string } },
  task: AssignmentTask,
  controlDir: string,
) {
  return AssignmentCoreSchema.parse({
    protocol: 1,
    controlDir,
    parentId: parent.modelId,
    parentName: parent.definition.name,
    taskId: task.id,
    agentBehavior: task.behaviorName,
    role: task.role,
    config: task.config,
    before: task.before,
    generation: planningGeneration(task.before),
    request: task.request,
    reviewCommit: task.reviewCommit,
    contractVersion: task.contractVersion,
    continuity: task.continuity,
    round: task.round,
  });
}
/** Complete obligations; repeated historical diffs have exact saved-snapshot references. */
export function reviewPacket(state: z.infer<typeof StateSchema>) {
  const review = (
    value: NonNullable<typeof state.codeReview | typeof state.uxReview>,
    source: string,
  ) => ({
    ...value,
    sourceRef: `assignment.before.${source}`,
    ...(value.baseline
      ? {
        baseline: {
          ...value.baseline,
          featureDiff: {
            sourceRef: `assignment.before.${source}.baseline.featureDiff`,
          },
          ...(value.baseline.delta !== undefined
            ? {
              delta: {
                sourceRef: `assignment.before.${source}.baseline.delta`,
              },
            }
            : {}),
        },
      }
      : {}),
  });
  const decisions = (state.reviewDecisions ?? []).map((decision, index) => ({
    ...decision,
    sourceRef: `assignment.before.reviewDecisions[${index}]`,
    codeReview: review(
      decision.codeReview,
      `reviewDecisions[${index}].codeReview`,
    ),
    uxReview: review(decision.uxReview, `reviewDecisions[${index}].uxReview`),
  }));
  const fix = activeReviewFix(state);
  return {
    requirements: state.issue,
    currentPlan: state.plan,
    approvedPlan: state.approval?.planVersion === state.plan?.version
      ? state.plan
      : undefined,
    planReview: state.planReview,
    planApproval: state.approval,
    planningContext: state.planningContext,
    workerProposedAcceptedDecisions: state.planningContext?.acceptedDecisions ??
      [],
    controllerAdjudications: state.planningContext?.adjudications ?? [],
    userReviewAuthorizations: decisions,
    selectedFindings: selectedRepairFindings(state),
    priorFindings: priorImplementationFindings(state),
    currentReviews: {
      technical: state.codeReview
        ? review(state.codeReview, "codeReview")
        : undefined,
      ux: state.uxReview ? review(state.uxReview, "uxReview") : undefined,
    },
    reviewArchives: (state.reviewHistory ?? []).map((entry, index) => ({
      ...entry,
      review: review(entry.review, `reviewHistory[${index}].review`),
    })),
    approvedRepair: fix ? decisions.find((d) => d.id === fix.id) : undefined,
    verification: state.verification,
    mergeFailures: state.mergeFailures ?? [],
    lastFailure: state.lastFailure,
    limitations: [
      "Historical records are context, never current approval. Saved-snapshot sourceRef paths are retrievable from the assigned submission's content.assignment.before via its authorized data query. Historical diffs are referenced without truncation; the current baseline includes the complete feature diff. Failure log references retain complete output where summary lists are bounded.",
    ],
  };
}
export function validateAssignment(assignment: Assignment) {
  const { digest, promptDigest: _prompt, ...core } = assignment;
  if (assignment.contractVersion === 2) {
    const c = assignment.continuity;
    const fingerprints = (["review_code", "review_ux"] as const).flatMap((
      role,
    ) =>
      (role === "review_code"
          ? assignment.before.codeReview
          : assignment.before.uxReview)
        ? [implementationReviewFingerprint(assignment.before, role)]
        : []
    );
    if (
      !assignment.round ||
      assignment.round.assignmentId !== assignment.taskId ||
      assignment.round.role !== assignment.role ||
      assignment.round.issueDigest !== assignment.before.issue.digest ||
      assignment.round.generation !== assignment.generation ||
      !c || c.assignmentId !== assignment.taskId ||
      c.issueDigest !== assignment.before.issue.digest ||
      c.generation !== assignment.generation ||
      c.rollbackId !== assignment.before.rollbackId ||
      c.planVersion !== assignment.before.planVersion ||
      c.packetDigest !== evidenceDigest(reviewPacket(assignment.before)) ||
      canonical(c.reviewFingerprints) !== canonical(fingerprints) ||
      (assignment.reviewCommit &&
        c.baseline?.head !== assignment.reviewCommit) ||
      ([
        "implement",
        "review_code",
        "review_ux",
        "plan_review_fixes",
        "implement_review_fixes",
      ].includes(assignment.role) && !c.baseline)
    ) {
      throw new Error(
        "Review continuity does not match its assignment snapshot and baseline",
      );
    }
  } else if (assignment.continuity) {
    throw new Error("Legacy assignment cannot carry a new continuity contract");
  }
  if (
    evidenceDigest(AssignmentCoreSchema.parse(core)) !== digest
  ) throw new Error("Submission assignment digest does not match its binding");
}

/** Explicitly validate before writing: Swamp resource schemas only warn. */
export function validateEvidence(assignment: Assignment, raw: unknown) {
  validateAssignment(assignment);
  const payload = RoleEvidenceSchema.parse(raw);
  if (payload.role !== assignment.role) {
    throw new Error("Evidence role does not match the assigned task");
  }
  const state = structuredClone(assignment.before);
  if (assignment.contractVersion === 2) {
    if (payload.role === "plan_review_fixes") {
      validateRepairMappings(state, payload.evidence, true);
    }
    if (payload.role === "implement_review_fixes") {
      validateRepairResponses(state, payload.evidence, true);
    }
    if (payload.role === "review_code" || payload.role === "review_ux") {
      validateImplementationReview(state, payload.role, payload.evidence, true);
      if (
        payload.evidence.baseline &&
        canonical(payload.evidence.baseline) !==
          canonical(assignment.continuity!.baseline)
      ) throw new Error("Reviewer baseline differs from assignment");
      // Controller provenance, never a claim of worker execution.
      payload.evidence.baseline = assignment.continuity!.baseline;
    }
    if ("advisoryEvidence" in payload.evidence) {
      for (const advisory of payload.evidence.advisoryEvidence ?? []) {
        validateAdvisory(
          advisory,
          state.issue.digest,
          "commit" in payload.evidence
            ? payload.evidence.commit
            : assignment.reviewCommit!,
        );
        if (!advisory.reuse && advisory.producingTask !== assignment.taskId) {
          throw new Error(
            "New advisory execution must name its producing assignment",
          );
        }
        if (advisory.reuse) {
          const sources = [
            state.codeReview,
            state.uxReview,
            ...(state.reviewHistory ?? []).map((h) => h.review),
            ...(state.reviewDecisions ?? []).flatMap((
              d,
            ) => [d.codeReview, d.uxReview, d.result]),
          ];
          const original = ({ reuse: _reuse, ...execution }: typeof advisory) =>
            execution;
          if (
            !sources.some((s) =>
              s?.advisoryEvidence?.some((e) =>
                canonical(original(e)) === canonical(original(advisory))
              )
            )
          ) {
            throw new Error(
              "Reused advisory execution must match retained original provenance",
            );
          }
        }
        if (
          advisory.reuse &&
          assignment.continuity?.baseline?.ancestry === "missing"
        ) {
          throw new Error(
            "Missing prior objects disable advisory reuse assistance",
          );
        }
      }
    }
  }
  if (payload.role === "plan" && !initialPlanning(state)) {
    const { revision } = RevisionPlanSchema.parse(payload.evidence);
    for (
      const finding of previousPlanFindings(state).filter((f) =>
        ["high", "critical"].includes(f.severity) && !f.resolution
      )
    ) {
      if (!revision.responses.some((r) => r.findingId === finding.id)) {
        throw new Error(
          `Revision must respond to blocking finding ${finding.id}`,
        );
      }
    }
  }
  if (payload.role === "review_plan") {
    validatePlanReview(state, payload.evidence);
  }
  if (
    (payload.role === "review_code" || payload.role === "review_ux") &&
    payload.evidence.commit !== assignment.reviewCommit
  ) {
    throw new Error(
      "Pane reviewed a different commit than the dispatched task",
    );
  }
  if (
    ["triage", "plan", "review_plan", "review_code", "review_ux"].includes(
      payload.role,
    ) && assignment.request
  ) {
    recordFeedback(state, assignment.request, state.planVersion);
  }
  const next = payload.role === "implement"
    ? state
    : transition(state, payload.role, payload.evidence);
  // Replay the contract that accepted the receipt. Legacy transitions did not
  // archive reviews, but any history already bound into the snapshot is evidence.
  if (assignment.contractVersion !== 2) {
    if (assignment.before.reviewHistory === undefined) {
      delete next.reviewHistory;
    } else {
      next.reviewHistory = structuredClone(assignment.before.reviewHistory);
    }
  }
  return { payload, next };
}

export function validateReceipt(assignment: Assignment, raw: unknown) {
  validateAssignment(assignment);
  const receipt = EvidenceReceiptSchema.parse(raw);
  const validated = validateEvidence(assignment, receipt.payload);
  if (
    receipt.taskId !== assignment.taskId ||
    receipt.assignmentDigest !== assignment.digest ||
    receipt.payloadDigest !== evidenceDigest(validated.payload) ||
    canonical(receipt.next) !== canonical(validated.next)
  ) throw new Error("Accepted receipt does not match its original assignment");
  return receipt;
}

function quote(value: string) {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

export function submissionInstructions(
  task: AssignmentTask,
  target: z.infer<typeof EvidenceTargetSchema>,
) {
  const envelope = {
    taskId: task.id,
    assignmentDigest: target.assignmentDigest,
    role: task.role,
    evidence: "replace this value with the role evidence object",
  };
  const command =
    `env -u SWAMP_SERVE_URL -u SWAMP_SERVER_URL -u SWAMP_REPO_DIR -u SWAMP_WORKFLOWS_DIR swamp`;
  const repo = `--repo-dir ${quote(target.controlDir)}`;
  return [
    "Submit evidence through Swamp before ending this task. Final chat text is presentation and is never imported as lifecycle evidence.",
    `Swamp submission target:\n${
      JSON.stringify({ ...target, taskId: task.id, role: task.role })
    }`,
    "You may invoke only this task's evidence submit method and data query. Approval, adjudication, rollback, restart, worktree preparation, merge, CI waivers and Agenda mutations belong to the controller. Do not invoke other lifecycle operations.",
    "Write a temporary JSON file outside the source tree with this envelope and the evidence schema below; temporary submission files are permitted during review:",
    JSON.stringify(envelope),
    `Evidence schema: ${
      JSON.stringify(z.toJSONSchema(
        task.role === "plan" && !initialPlanning(task.before)
          ? RevisionPlanSchema
          : RoleSchemas[task.role],
      ))
    }`,
    `Submit (replace /tmp/submission.json with your file):\n${command} behavior run ${
      quote(target.behaviorName)
    } submit ${repo} --stdin --literal-inputs --json < /tmp/submission.json`,
    `Inspect acceptance, including after a lost acknowledgement:\n${command} data query ${
      quote(`behavior("${target.behaviorName}") && name == "submission"`)
    } ${repo} --select content --single --json`,
    "A successful submission records status accepted and a receipt with this task ID and assignment digest. Correct validation errors and resubmit while retaining your investigation. An identical replay returns the original receipt; changed evidence after acceptance is rejected. Query before retrying an interrupted acknowledgement. Never initialize another Swamp store.",
    "Finish only after inspecting the matching accepted receipt. Return readable prose; its formatting and contents do not carry lifecycle evidence.",
  ].join("\n\n");
}
