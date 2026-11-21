import { closeoutPresentation } from "./issue_closeout.ts";
import { notifyHandoff } from "./issue_notifications.ts";
import {
  handoffMatchesState,
  handoffStateFields,
  nextStep,
} from "./handoff_next.ts";
/** Foreground orchestration up to a human decision, with durable pane recovery. */
import { z } from "npm:zod@4.6.7";
import {
  abortableSleep,
  acceptedEvidence,
  AgentInputSchema,
  AgentTaskSchema,
  childState,
  defaultDependencies,
  finishRecovery,
  implementationEvidence,
  type IssueAgentContext,
  type IssueAgentDependencies,
  persistedTask,
  prepareTask,
  RecoverySchema,
  resumeTask,
  runTask,
  taskBeforeDigest,
  taskNextDigest,
  withBefore,
} from "./issue_agents.ts";
import {
  ActionSchemas,
  activeFollowUps,
  activeMergeFailure,
  activeReviewFix,
  codeReviewFingerprint,
  deferredPlanFindings,
  describeMergeFailure,
  effectivePlanFindings,
  FindingSchema,
  implementationMergeGate,
  initialPlanning,
  type LifecycleState,
  LifecycleSummarySchema,
  pendingReviewFindings,
  planningGeneration,
  refreshImpact,
  refreshIssue,
  requireCurrentCodeReview,
  requireCurrentReviews,
  requireReviewAcceptance,
  reviewFingerprint,
  selectableFollowUp,
  stateDigest,
  StateSchema,
  transition,
  UserRequestSchema,
  verificationFailureItems,
} from "./lifecycle.ts";
import {
  type Context,
  type Handle,
  InspectionSchema,
  loadState,
  normalizeReceipt,
  saveState,
} from "./runtime.ts";
import {
  checkLogQuery,
  displayCommand,
  runVerification,
  verificationChecklist,
} from "./verification.ts";
import {
  type Grant,
  inspectWorkerFailure,
  RecoveryBudget,
  type WorkerVerdict,
} from "./worker_recovery.ts";
import {
  AgentResponseSchema,
  ControlIdentitySchema,
  reportedBackground,
  unidentifiedBackground,
} from "./yinz_agent.ts";

const Settings = AgentInputSchema.omit({ role: true, request: true });
const SettingsOverride = Settings.omit({
  startupTimeoutSeconds: true,
  timeoutSeconds: true,
}).partial().extend({
  startupTimeoutSeconds: Settings.shape.startupTimeoutSeconds.removeDefault()
    .optional(),
  timeoutSeconds: Settings.shape.timeoutSeconds.removeDefault().optional(),
});
export const DriveInputSchema = z.object({
  agent: SettingsOverride.optional(),
  control: ControlIdentitySchema.optional(),
  revisionApproval: z.object({
    handoffId: z.uuid(),
    request: UserRequestSchema,
  }).strict().optional(),
  // The user's standing request to repair every blocking review finding of
  // this plan and rollback without asking, or its withdrawal.
  autoRepair: z.union([
    z.object({ request: UserRequestSchema }).strict(),
    z.object({ withdraw: z.literal(true) }).strict(),
  ]).optional(),
})
  .strip();
/** Auto-repair rounds per authorization before drive asks the user again. */
export const AUTO_REPAIR_ROUNDS = 3;
const AutoRepairSchema = z.object({
  request: UserRequestSchema,
  createdAt: z.iso.datetime(),
  issueDigest: z.string(),
  planVersion: z.number().int().nonnegative(),
  rollbackId: z.uuid().optional(),
  rounds: z.number().int().nonnegative(),
}).strict();
/**
 * Consecutive ordinary wait timeouts, with accepted evidence and only pane
 * background work outstanding, before drive stops observing and hands off.
 */
export const BACKGROUND_STALL_TIMEOUTS = 2;
const BackgroundStallSchema = z.object({
  taskId: z.uuid(),
  role: AgentTaskSchema.shape.role,
  paneId: z.string().optional(),
  backgroundTasks: z.array(z.string()),
  commit: z.string().optional(),
  rounds: z.number().int().positive(),
  observedAt: z.iso.datetime(),
  // The child observation the stall rests on; any later observation supersedes it.
  child: z.object({
    phase: z.enum(["waiting", "background"]),
    outcome: z.string().optional(),
    updatedAt: z.iso.datetime(),
  }).strict(),
}).strict();
const AutomaticRevisionSchema = z.object({
  generation: z.string(),
  assignment: AgentTaskSchema,
  previousAssignmentId: z.uuid().optional(),
  recoveryId: z.uuid().optional(),
}).strict();
export const DriverSchema = z.object({
  agent: Settings.optional(),
  assignmentId: z.uuid().optional(),
  implementation: AgentTaskSchema.optional(),
  automaticRevision: AutomaticRevisionSchema.optional(),
  revisionAuthorization: z.object({
    handoffId: z.uuid(),
    request: UserRequestSchema,
    createdAt: z.iso.datetime(),
    assignment: AgentTaskSchema,
    previousAssignmentId: z.uuid().optional(),
    recoveryId: z.uuid().optional(),
  }).strict().optional(),
  // Accepted evidence whose pane kept reporting background work; repeated drive
  // calls re-present this blocker instead of observing the same task again.
  backgroundStall: BackgroundStallSchema.optional(),
  autoRepair: AutoRepairSchema.optional(),
}).strict();
/** The worker a handoff reports, without the task's prompt or evidence. */
const AssignmentRefSchema = z.object({
  id: z.uuid(),
  role: AgentTaskSchema.shape.role,
  behaviorName: z.string(),
}).strict();
/** The completed implementation a handoff reports. */
const ImplementationRefSchema = z.object({
  id: z.uuid(),
  commit: z.string().regex(/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/),
  summary: z.string().min(1),
}).strict();
/**
 * A human decision or blocker. It names the exact lifecycle state by canonical
 * digest and carries only what its readers use; the presentation holds the
 * rendered evidence. Handoffs saved before compaction embed `state`,
 * `assignment` and `implementation` instead and still parse.
 */
export const HandoffSchema = z.object({
  id: z.uuid(),
  status: z.enum(["current", "invalidated"]),
  decision: z.enum([
    "authorize_plan_revision",
    "approve_plan",
    "prepare_worktree",
    "authorize_merge",
    "address_review_findings",
    // Legacy: saved before repair planning was retired; drive no longer issues it.
    "approve_review_fixes",
    "integrate",
    "summarize",
    "authorize_closeout",
    "resume_closeout",
    "complete",
    "blocked",
  ]),
  reason: z.string().min(1),
  createdAt: z.iso.datetime(),
  stateDigest: z.string().regex(/^[0-9a-f]{64}$/).optional(),
  lifecycle: LifecycleSummarySchema.optional(),
  assignmentRef: AssignmentRefSchema.optional(),
  implementationRef: ImplementationRefSchema.optional(),
  // Legacy embedded records.
  state: StateSchema.optional(),
  assignment: AgentTaskSchema.optional(),
  implementation: AgentTaskSchema.optional(),
  recoveryId: z.uuid().optional(),
  proposedPlanVersion: z.number().int().positive().optional(),
  planVersion: z.number().int().positive().optional(),
  commit: z.string().optional(),
  reviewFingerprint: z.string().optional(),
  codeReviewFingerprint: z.string().optional(),
  // Set when drive stopped because the Agenda item no longer matches the state.
  requirementsChange: z.object({
    previousDigest: z.string(),
    digest: z.string(),
    changes: z.array(z.string()),
    invalidates: z.array(z.string()),
  }).strict().optional(),
  presentation: z.string().min(1),
}).strict();
export type Handoff = z.infer<typeof HandoffSchema>;
type Task = z.infer<typeof AgentTaskSchema>;

/** The worker task a handoff names, in either record shape. */
export function handoffAssignmentId(handoff: Handoff): string | undefined {
  return handoff.assignmentRef?.id ?? handoff.assignment?.id;
}

/** Whether the current reviews can scope a findings decision. */
function reviewDecisionReady(state: LifecycleState) {
  try {
    requireCurrentCodeReview(state);
    return true;
  } catch {
    return false;
  }
}

function blockedFindings(findings: z.infer<typeof FindingSchema>[] = []) {
  return findings.some((finding) =>
    ["high", "critical"].includes(finding.severity) && !finding.resolution
  );
}

/** Every failed merge attempt, so a later pass cannot hide a flake. */
function mergeAttemptLines(state: LifecycleState, issueName: string) {
  const failures = state.mergeFailures ?? [];
  const recorded = new Set(failures.map((failure) => failure.attemptId));
  // A receipt can name failures from before this state recorded them.
  const earlier = (state.receipt?.previousAttempts ?? []).filter((attempt) =>
    !recorded.has(attempt.attemptId)
  );
  if (!failures.length && !earlier.length) return [];
  const lines = ["Merge attempts:"];
  for (const attempt of earlier) {
    lines.push(
      `- ${attempt.attemptId}: failed; ${attempt.summary}` +
        (attempt.log ? `; log: ${attempt.log}` : ""),
    );
  }
  for (const failure of failures) {
    lines.push(`- ${failure.attemptId} at ${failure.at}: ${failure.summary}`);
    if (failure.phases.length) {
      lines.push(
        `  phases: ${
          failure.phases.map((phase) => `${phase.name} ${phase.status}`).join(
            ", ",
          )
        }`,
      );
    }
    if (failure.failedTests.length) {
      lines.push(
        `  failed tests: ${failure.failedTests.join(", ")}` +
          (failure.failedTestsTruncated
            ? ` (+${failure.failedTestsTruncated} more in the log)`
            : ""),
      );
    }
    if (failure.log) lines.push(`  log: ${failure.log}`);
    lines.push(`  output: ${checkLogQuery(issueName, failure.logResource)}`);
  }
  if (state.receipt) {
    lines.push(
      `- ${state.receipt.attemptId}: merged ${state.receipt.mergedCommit}`,
    );
  }
  return lines;
}

function present(
  handoff: Omit<Handoff, "presentation">,
  state: LifecycleState,
  issueName: string,
) {
  const lines = [
    `Issue ${state.issue.workspace}/${state.issue.itemId}: ${state.issue.text}`,
    `Handoff ${handoff.id}: ${handoff.decision}`,
    handoff.reason,
    ...(handoff.planVersion ? [`Plan version: ${handoff.planVersion}`] : []),
    ...(handoff.commit ? [`Commit: ${handoff.commit}`] : []),
    `Requirements: ${state.issue.note}`,
    ...(handoff.requirementsChange
      ? [
        "Requirements changes:",
        ...handoff.requirementsChange.changes.map((change) => `  ${change}`),
        ...(handoff.requirementsChange.invalidates.length
          ? [
            "Refresh would invalidate:",
            ...handoff.requirementsChange.invalidates.map((item) =>
              `- ${item}`
            ),
          ]
          : []),
      ]
      : []),
    ...(state.workerAgent
      ? [
        `Saved worker choice: ${state.workerAgent.kind}; model: ${
          state.workerAgent.model ?? "provider default"
        }; effort: ${state.workerAgent.effort ?? "provider default"}`,
      ]
      : ["Saved worker choice: none; explicit agent settings required."]),
  ];
  if (handoff.decision === "authorize_plan_revision") {
    lines.push(
      `Proposed next plan version: ${handoff.proposedPlanVersion}`,
      "Revision authorization does not approve implementation or merge.",
      ...(state.planReview && state.plan
        ? [
          "Alternatively, inspect cited code and adjudicate supported findings. Accepted unresolved high/critical risks still require user direction.",
          "The user may instead decide this plan is sufficient: approving with --ignore defers the named findings, unresolved, to implementation review. Only the user's explicit direction can do this; pending workers, stale handoffs and invalid evidence still block.",
        ]
        : []),
    );
  }
  const deferred = deferredPlanFindings(state);
  if (deferred) {
    lines.push(
      `Plan findings the user deferred at approval of plan ${
        state.approval!.planVersion
      } (unresolved; implementation reviews revisit them):`,
      ...deferred.findings.map((f) =>
        `- review_plan/${f.id} (${f.severity}): ${f.description}`
      ),
      `Deferral request: ${state.approval!.request}`,
    );
  }
  if (handoff.decision === "address_review_findings") {
    lines.push(
      "Which findings would you like addressed? You may ignore any findings or sign off on merge with findings outstanding.",
      "Selected findings go straight to a repair worker; the next review checks only the repair diff against them.",
      ...(state.uxReview
        ? []
        : ["UX review runs once the code review's findings are settled."]),
    );
  }
  if (handoff.decision === "approve_review_fixes") {
    lines.push(
      "Legacy repair-plan approval: approving lets the saved plan guide the repair worker. Running drive instead dispatches the repair worker without it.",
    );
  }
  const followUps = activeFollowUps(state);
  if (
    followUps.length &&
    ["address_review_findings", "authorize_merge", "summarize"].includes(
      handoff.decision,
    )
  ) {
    lines.push(
      `Follow-ups (${
        followUps.map((f) => f.detail.severity).filter((v, i, a) =>
          a.indexOf(v) === i
        ).join("/")
      } severity, not repaired${
        handoff.decision === "summarize"
          ? "; summarize files each as a separate Agenda item"
          : "; select one of the current reviews with --repair to fix it now"
      }):`,
      ...followUps.map((f) =>
        `- ${f.finding.role}/${f.finding.findingId}: ${f.detail.description}${
          f.agendaItem?.itemId ? ` (Agenda item ${f.agendaItem.itemId})` : ""
        }${
          handoff.decision === "summarize" || selectableFollowUp(state, f)
            ? ""
            : ` (from an earlier review at ${f.commit}; not selectable)`
        }`
      ),
    );
  }
  for (const decision of state.reviewDecisions ?? []) {
    lines.push(
      `Review decision ${decision.id}: ${decision.request}`,
      `Decision scope: ${decision.reviewFingerprint} at ${decision.commit}; historical decisions do not authorize current review evidence.`,
      `Selected repairs: ${
        decision.repair.map((f) => `${f.role}/${f.findingId}`).join(", ") ||
        "None"
      }`,
      `Ignored (remain unresolved): ${
        decision.ignore.map((f) => `${f.role}/${f.findingId}`).join(", ") ||
        "None"
      }`,
    );
    if (decision.plan) {
      lines.push(
        `Repair plan: ${decision.plan.summary}`,
        ...decision.plan.steps.map((step) =>
          `- ${step.id}: ${step.description} (${step.files.join(", ")})`
        ),
        ...(decision.plan.findingMappings ?? []).map((m) =>
          `- Repair mapping ${m.finding.role}/${m.finding.findingId} (${m.finding.reviewFingerprint}): steps ${
            m.steps.join(", ")
          }; checks ${m.checks.join(", ")}`
        ),
        ...(decision.plan.scopeChanges ?? []).map((change) =>
          `Proposed scope change: ${change}`
        ),
        ...(decision.plan.proposedTradeoffs ?? []).map((tradeoff) =>
          `Worker-proposed tradeoff: ${tradeoff}`
        ),
        `Repair tests: ${decision.plan.testingStrategy}`,
        ...decision.plan.checks.map((check) =>
          `- ${check.id}: ${displayCommand(check.command)}`
        ),
        ...(decision.plan.assumptions ?? []).map((a) => `Assumption: ${a}`),
      );
    }
    if (decision.result) {
      lines.push(
        `Repairs committed at ${decision.result.commit}: ${decision.result.summary}`,
        ...(decision.result.responses ?? []).map((r) =>
          `- Repair response ${r.finding.role}/${r.finding.findingId}: ${r.status}. ${r.rationale} Evidence: ${
            r.evidence.join("; ")
          }. Implementation claims require reviewer verification.`
        ),
      );
    }
  }
  if (state.planningContext) {
    const context = state.planningContext;
    if (context.userFeedback.length) {
      lines.push(
        "Recorded user feedback:",
        ...context.userFeedback.map((f) =>
          `- At plan ${f.planVersion}: ${f.request}`
        ),
      );
    }
    if (context.acceptedDecisions.length) {
      lines.push(
        "Worker-proposed accepted planning decisions (retain their recorded approval scope):",
        ...context.acceptedDecisions.map((decision) => `- ${decision}`),
      );
    }
    if (context.history.length) {
      lines.push("Planning history (not current approval evidence):");
    }
    for (const entry of context.history) {
      lines.push(
        `- Generation ${entry.generation}, plan ${entry.plan.version}: ${entry.plan.summary}`,
      );
      for (const finding of entry.review?.findings ?? []) {
        lines.push(
          `  ${finding.id} (${finding.severity}): ${finding.description}${
            finding.resolution ? ` Resolved: ${finding.resolution}` : ""
          }`,
        );
      }
      if (entry.plan.revision) {
        lines.push(`  Revision: ${entry.plan.revision.summary}`);
      }
    }
    for (const adjudication of context.adjudications) {
      for (const disposition of adjudication.dispositions) {
        lines.push(
          `Plan ${adjudication.planVersion} disposition, ${disposition.findingId}: ${disposition.disposition}${
            disposition.severity ? ` to ${disposition.severity}` : ""
          }. ${disposition.rationale} Evidence: ${
            disposition.evidence.join("; ")
          }`,
        );
      }
    }
  }
  if (state.plan?.revision) {
    lines.push(
      `Revision changes: ${state.plan.revision.summary}`,
      "Finding responses and disagreements:",
    );
    for (const response of state.plan.revision.responses) {
      lines.push(
        `- ${response.findingId}: ${response.status}. ${response.rationale} Evidence: ${
          response.evidence.join("; ")
        }`,
      );
    }
  }
  if (state.planReview) {
    lines.push(
      "Effective current plan findings:",
      ...effectivePlanFindings(state).map((f) =>
        `- ${f.id}: ${f.severity}${
          f.resolution
            ? `, resolved: ${f.resolution}`
            : deferred?.findings.some((d) => d.id === f.id)
            ? ", unresolved (deferred by the user to implementation review)"
            : ", unresolved"
        }`
      ),
    );
  }
  if (state.classification) {
    lines.push(`Investigation: ${state.classification.summary}`);
  }
  if (state.plan) {
    lines.push(
      `Proposed behavior: ${state.plan.summary}`,
      "Implementation steps:",
    );
    for (const step of state.plan.steps) {
      lines.push(
        `- ${step.id}: ${step.description} (${step.files.join(", ")})`,
      );
    }
    lines.push(`Tests: ${state.plan.testingStrategy}`);
    if (state.plan.checks) {
      lines.push(
        "Checks (run verbatim in the issue worktree before merge authorization):",
        ...state.plan.checks.map((check) =>
          `- ${check.id}: ${displayCommand(check.command)} (covers ${
            check.covers.join(", ")
          })`
        ),
      );
    } else {
      lines.push("Checks: none; this legacy plan predates structured checks.");
    }
    lines.push("Assumptions:");
    lines.push(
      ...(state.plan.assumptions?.length
        ? state.plan.assumptions.map((value) => `- ${value}`)
        : [
          "- No assumptions recorded; assess the investigation and plan risks.",
        ]),
    );
  }
  for (
    const [label, review] of [
      ["Plan review", state.planReview],
      ["Code review", state.codeReview],
      ["UX review", state.uxReview],
    ] as const
  ) {
    if (!review) continue;
    lines.push(`${label} findings:`);
    lines.push(
      ...(review.findings.length
        ? review.findings.map((finding) =>
          `- ${finding.severity} ${finding.id}: ${finding.description}${
            finding.origin
              ? ` ${
                finding.reopening ? "Reopened concern" : "Historical continuity"
              }: ${finding.origin.role}/${finding.origin.findingId} from ${finding.origin.reviewFingerprint}.`
              : ""
          }${finding.resolution ? ` Resolution: ${finding.resolution}` : ""}${
            finding.requirement ? ` Requirement: ${finding.requirement}` : ""
          }${finding.mechanism ? ` Failure: ${finding.mechanism}` : ""}${
            finding.evidence?.length
              ? ` Evidence: ${finding.evidence.join("; ")}`
              : ""
          }`
        )
        : ["- No findings."]),
    );
  }
  if (handoff.implementationRef) {
    lines.push(`Delivered change: ${handoff.implementationRef.summary}`);
  }
  if (state.uxReview) {
    const assessment = state.uxReview.assessment;
    lines.push(
      `UX assessment: ${assessment.impact}. ${assessment.summary}`,
      "Assessed user tasks, workflows and paths:",
      ...assessment.assessedPaths.map((path) => `- ${path}`),
      `UX limitations: ${
        assessment.limitations.join("; ") || "None recorded."
      }`,
      "UX reviewer-reported checks (advisory):",
      ...(state.uxReview.preliminaryChecks?.length
        ? state.uxReview.preliminaryChecks.map((check) =>
          `- ${check.command}: ${check.result}. ${check.summary}`
        )
        : ["- No preliminary checks recorded."]),
    );
  }
  if (state.codeReview) {
    lines.push("Delivered change coverage:");
    for (const step of state.codeReview.steps) {
      lines.push(
        `- ${step.id}: ${step.status}${
          step.justification ? ` (${step.justification})` : ""
        }`,
      );
    }
    lines.push("Verification results:");
    const gate = state.plan &&
      implementationMergeGate(state);
    if (gate?.kind === "legacy") {
      lines.push(
        `- No deterministic verification: plan version ${
          state.plan!.version
        } predates structured checks.`,
      );
    } else if (gate?.kind === "passed" || gate?.kind === "failed") {
      lines.push(...verificationChecklist(issueName, gate.verification));
    } else {
      lines.push("- The plan's checks have not run for this commit.");
    }
    lines.push(
      gate?.kind === "legacy"
        ? "Reviewer-reported checks:"
        : "Reviewer-reported checks (advisory):",
    );
    lines.push(
      ...(state.codeReview.preliminaryChecks?.length
        ? state.codeReview.preliminaryChecks.map((check) =>
          `- ${check.command}: ${check.result}. ${check.summary}`
        )
        : ["- No preliminary checks recorded."]),
    );
    lines.push(
      "The full CI gate runs during authorized integration unless the user explicitly waives it.",
    );
  }
  lines.push(...mergeAttemptLines(state, issueName));
  if (state.mergeAuthorization) {
    const authorization = state.mergeAuthorization;
    lines.push(
      `Merge authorization for ${authorization.commit} (plan version ${authorization.planVersion}): ${authorization.request}`,
    );
  }
  if (handoff.assignmentRef) {
    lines.push(
      `Worker: ${handoff.assignmentRef.role}, task ${handoff.assignmentRef.id}, behavior ${handoff.assignmentRef.behaviorName}`,
    );
  }
  lines.push(
    "The controlling agent must collect and present this result before ending its turn.",
    nextStep(handoff, state),
  );
  return lines.join("\n");
}

/** Facts about a correlated ordinary timeout, for logs and the background-stall bound. */
async function timeoutDiagnosis(context: IssueAgentContext, task: Task) {
  const child = await childState(context, task);
  const { backgroundTasks, agentState } = reportedBackground(child);
  // The same validation import applies: accepted, for this assignment and digest.
  const receipt = await acceptedEvidence(context, task).catch(() => undefined);
  return {
    child,
    phase: child.phase,
    paneId: child.paneId,
    agentState,
    backgroundTasks,
    receipt,
    // A submitted prompt is required; a spawned pane has no turn to finish.
    backgroundOnly: !!receipt &&
      (child.phase === "background" ||
        (child.phase === "waiting" && backgroundTasks.length > 0)),
  };
}

function backgroundStallReason(stall: z.infer<typeof BackgroundStallSchema>) {
  // A flag without IDs rests on Claude's session registry or an older Yinz build.
  const work = stall.backgroundTasks.length === 0
    ? "background work (no task IDs reported)"
    : unidentifiedBackground(stall.backgroundTasks)
    ? "background work without task IDs (Claude's session registry or an older Yinz build)"
    : `background work [${stall.backgroundTasks.join(", ")}]`;
  return [
    `Pane task ${stall.taskId} (${stall.role}) has an accepted Swamp evidence submission${
      stall.commit ? ` for commit ${stall.commit}` : ""
    }, but pane ${
      stall.paneId ?? "unknown"
    } still reports ${work} after ${stall.rounds} consecutive ordinary wait timeouts.`,
    // Yinz cannot tell a running foreground turn from a finished one that left
    // background work, so say so unless the child observed that turn's answer.
    ...(stall.child.phase === "background" ? [] : [
      "Yinz has not confirmed that the foreground turn ended; check that the pane is idle at its prompt before authorizing.",
    ]),
    "drive will not wait on this task again or launch another worker.",
    `With the user's explicit authorization, import the accepted evidence without waiting on, prompting or closing the pane: accept-submission {taskId: "${stall.taskId}", request: "<the user's actual authorization>"}.`,
    "To observe the pane again instead, run resume-agent.",
  ].join(" ");
}

async function workerBlocker(
  context: IssueAgentContext,
  task: Task,
  error: unknown,
  verdict?: WorkerVerdict,
  grant?: Grant,
) {
  const child = await childState(context, task).catch(() => undefined);
  let observed: string | undefined;
  if (child?.rawResponse) {
    try {
      const response = AgentResponseSchema.parse(
        JSON.parse(child.rawResponse),
      );
      observed = `Latest response: outcome ${response.outcome}, server ${
        response.server_instance ?? "unspecified"
      }, turn ${response.turn_id ?? "unspecified"}, submission ${
        response.submission_id ?? "unspecified"
      }; expected server ${child.launchServer ?? "unknown"}, turn ${
        child.turnId ?? "unknown"
      }.`;
    } catch { /* The original error retains malformed-response details. */ }
  }
  let recovery: string | undefined;
  if (verdict?.kind === "human") {
    recovery = `Automatic recovery not applicable: ${verdict.reason}.`;
  } else if (verdict?.kind === "retry" && grant && !grant.allowed) {
    recovery =
      `Automatic ${verdict.class} recovery exhausted after ${grant.attempt} attempt(s) over ${
        Math.round(grant.elapsedMs / 1000)
      }s (${
        grant.exhausted === "deadline"
          ? "8 h drive deadline"
          : `limit ${grant.limit}`
      }). Rerunning drive starts a new bounded budget.`;
  }
  return new Error([
    error instanceof Error ? error.message : String(error),
    ...(observed ? [observed] : []),
    `Original task ${task.id}, child ${task.behaviorName}, session ${task.config.session}, pane ${
      child?.paneId ?? "unknown"
    }, conversation ${child?.conversationId ?? "unknown"}, submission ${
      child?.submissionId ?? "unknown"
    }.`,
    ...(recovery ? [recovery] : []),
    task.status === "completed" && task.cleanup === "pending"
      ? "Evidence is already imported; only guarded pane cleanup remains. Resolve the named guard, then drive; do not discard or restart."
      : "Completion is not reconciled. Inspect the saved child state and original submission with resume-agent; do not resend the prompt or infer termination from a closed pane or elapsed time.",
  ].join("\n"));
}

/**
 * Runs `first`, then re-observes the original task through resumeTask while
 * a correlated failure is retryable within the invocation's budget. A retry
 * never resends a prompt or spawns after a launch intent; every other
 * outcome blocks immediately with the reason.
 */
async function settle(
  context: IssueAgentContext,
  dependencies: IssueAgentDependencies,
  budget: RecoveryBudget,
  handles: Handle[],
  expected: Task,
  first: () => Promise<{ dataHandles: Handle[] }>,
  onTimeout: (task: Task) => Promise<{ dataHandles: Handle[] } | undefined>,
) {
  let attempt = first;
  while (true) {
    try {
      handles.push(...(await attempt()).dataHandles);
      return;
    } catch (error) {
      context.signal.throwIfAborted();
      const raw = await context.readResource("agent-task").catch(() => null);
      const parsed = AgentTaskSchema.safeParse(raw);
      // runTask can fail before saving its task; never judge the previous one.
      const current = parsed.success && parsed.data.id === expected.id
        ? parsed.data
        : undefined;
      const task = current ?? expected;
      const verdict: WorkerVerdict = current
        ? await inspectWorkerFailure(context, current, error).catch((
          failure,
        ) => ({
          kind: "human" as const,
          reason: `recovery classification failed: ${
            failure instanceof Error ? failure.message : failure
          }`,
        }))
        : {
          kind: "human",
          reason:
            `the saved assignment is not the expected task ${expected.id}`,
        };
      const grant = verdict.kind === "retry"
        ? budget.take(task.id, verdict.class)
        : undefined;
      if (verdict.kind !== "retry" || !grant?.allowed) {
        throw await workerBlocker(context, task, error, verdict, grant);
      }
      if (verdict.class === "long_wait") {
        const stalled = await onTimeout(task);
        if (stalled) return stalled;
      }
      context.logger.info(
        "Automatic recovery of original task {task} ({class} attempt {attempt}/{limit}, {elapsed}s elapsed): {reason}; waiting {delay} ms; no prompt is resent and no pane is spawned",
        {
          task: task.id,
          class: verdict.class,
          attempt: grant.attempt,
          limit: grant.limit,
          elapsed: Math.round(grant.elapsedMs / 1000),
          reason: verdict.reason,
          delay: grant.delayMs,
        },
      );
      if (grant.delayMs) {
        await (dependencies.sleep ?? abortableSleep)(
          grant.delayMs,
          context.signal,
        );
      }
      attempt = () => resumeTask(context, dependencies);
    }
  }
}

export const ImplementArgsSchema = z.object({
  worktree: z.string().trim().min(1).optional(),
  branch: z.string().trim().min(1).optional(),
});

/**
 * Binds implementation to the recorded worker worktree, defaulting its branch
 * to the recorded implementation branch or the branch checked out there.
 * Legacy creation, and a worktree that closeout may have removed, require an
 * explicit worktree and branch. The caller owns the issue lock.
 */
export async function adoptWorkerWorktree(
  context: Context,
  state: LifecycleState,
  args: z.infer<typeof ImplementArgsSchema>,
  deps: Pick<IssueAgentDependencies, "realPath" | "inspect" | "gitOperation">,
) {
  const recorded = Boolean(state.workerWorktree || state.implementation);
  if (state.closeout?.authorizedRequest && (!args.worktree || !args.branch)) {
    throw new Error(
      "Closeout may have removed the recorded worktree; deliberately select a new worktree and supply both worktree and branch to implement",
    );
  }
  let worktree = args.worktree ?? state.workerWorktree ??
    state.implementation?.worktree;
  if (!recorded && (!worktree || !args.branch)) {
    throw new Error(
      "Legacy issue without a recorded worker worktree: supply both worktree and branch to implement",
    );
  }
  if (!worktree) throw new Error("Implementation requires a worktree");
  // Only adoptions canonicalize; legacy creation passes the path through.
  if (recorded) worktree = await deps.realPath(worktree);
  let branch = args.branch ?? state.implementation?.branch;
  if (!branch) {
    const repo = await deps.inspect(context, worktree);
    if (!repo.branch || repo.branch === "main") {
      throw new Error(
        `Recorded worker worktree ${worktree} is ${
          repo.branch ? "on main" : "at a detached HEAD"
        }; check out the issue's feature branch there, or run implement with an explicit worktree and branch, then drive`,
      );
    }
    branch = repo.branch;
  }
  const implementation = { worktree, branch };
  // Validate the phase, approval and closeout before creating or claiming Git state.
  transition(state, "implement", implementation);
  const repo = InspectionSchema.parse(
    await deps.gitOperation(context, {
      action: "prepare",
      ...implementation,
      issueKey: `${state.issue.workspace}/${state.issue.itemId}`,
    }),
  );
  return saveState(
    context,
    "implement",
    transition(state, "implement", {
      worktree: repo.worktree,
      branch: repo.branch,
    }),
  );
}

/** Why drive stopped on a changed Agenda item instead of refreshing it. */
function requirementsChangedReason(
  state: LifecycleState,
  impact: ReturnType<typeof refreshImpact>,
  authorizationHandoffId?: string,
): string {
  if (authorizationHandoffId) {
    return `Agenda requirements changed during ${state.phase}; automatic refresh would discard the user's revision authorization for handoff ${authorizationHandoffId}, and refresh refuses while that authorization is reserved. Present the changes; with the user's direction run rollback to triage (it records the authorization's request as planning feedback and supersedes the reservation) then drive to re-plan, or restore the previous Agenda text to use the authorization.`;
  }
  if (["merged", "done"].includes(state.phase)) {
    return "Agenda requirements changed after merge; record the difference in the summary/closeout instead of refreshing.";
  }
  if (!impact.refreshable) {
    return "Agenda requirements changed during an active merge attempt. Repair the active merge attempt before refreshing its issue.";
  }
  const when = ["approved", "implementing", "ready"].includes(state.phase)
    ? "after plan approval"
    : `during ${state.phase}`;
  return `Agenda requirements changed ${when}. Refresh returns to triage and invalidates: ${
    impact.invalidates.join("; ") || "no recorded authorizations"
  }. Present the changes and the invalidated approvals; with the user's direction run refresh then drive, or restore the previous Agenda text.`;
}

/**
 * The completed implementation report that still describes the current plan,
 * rollback, requirements and worktree, if any. An implementation report imports
 * the state it was dispatched against, so its dispatch snapshot is that state.
 */
async function currentImplementation(
  context: IssueAgentContext,
  candidate: Task,
  state: LifecycleState,
): Promise<Task | undefined> {
  if (
    candidate.status !== "completed" ||
    candidate.evidence?.payload.role !== "implement"
  ) return undefined;
  const next = candidate.next ??
    (taskNextDigest(candidate) === taskBeforeDigest(candidate)
      ? await withBefore(context, candidate).then(
        (task) => task.before,
        () => undefined,
      )
      : undefined);
  if (
    !next || next.plan?.version !== state.plan?.version ||
    next.rollbackId !== state.rollbackId ||
    next.issue.digest !== state.issue.digest ||
    next.implementation?.worktree !== state.implementation?.worktree ||
    (state.codeReview &&
      implementationEvidence(candidate).commit !== state.codeReview.commit)
  ) return undefined;
  return candidate;
}

/** The caller owns the issue lock. Helpers here deliberately do not acquire it again. */
async function drive(
  input: z.infer<typeof DriveInputSchema>,
  context: IssueAgentContext,
  dependencies: IssueAgentDependencies,
) {
  const handles: Handle[] = [];
  // Automatic recovery allowances last for this invocation only.
  const budget = new RecoveryBudget(
    dependencies.monotonicNow ?? (() => performance.now()),
  );
  const rawDriver = await context.readResource("driver");
  let driver = DriverSchema.parse(rawDriver ?? {});

  async function task() {
    const raw = await context.readResource("agent-task");
    return raw == null ? undefined : AgentTaskSchema.parse(raw);
  }
  /**
   * The repair decision drive records under the user's standing auto-repair
   * request, or why the user must decide instead.
   */
  function autoRepairDecision(state: LifecycleState): {
    decision?: z.input<typeof ActionSchemas.decide_review>;
    reason?: string;
  } {
    const auto = driver.autoRepair;
    if (!auto) return {};
    if (
      auto.issueDigest !== state.issue.digest ||
      auto.planVersion !== state.planVersion ||
      auto.rollbackId !== state.rollbackId
    ) {
      return {
        reason:
          "The auto-repair request covered earlier requirements, plan or rollback; renew it with drive --auto-repair",
      };
    }
    if (auto.rounds >= AUTO_REPAIR_ROUNDS) {
      return {
        reason:
          `Auto-repair stopped after ${AUTO_REPAIR_ROUNDS} rounds; renew it with drive --auto-repair to continue`,
      };
    }
    const pending = pendingReviewFindings(state);
    const reopened = pending.filter((f) => f.finding.reopening);
    if (reopened.length) {
      return {
        reason: `Auto-repair needs your direction on reopened findings: ${
          reopened.map((f) => `${f.role}/${f.findingId}`).join(", ")
        }`,
      };
    }
    return {
      decision: {
        id: dependencies.id(),
        reviewFingerprint: codeReviewFingerprint(state),
        request: auto.request,
        repair: pending.map(({ role, findingId }) => ({ role, findingId })),
        ignore: [],
        source: "auto_repair",
      },
    };
  }
  async function handoff(
    decision: Handoff["decision"],
    reason: string,
    extra: Pick<Handoff, "requirementsChange"> = {},
    reusable = decision !== "blocked",
  ) {
    const state = await loadState(context);
    const assignment = await task();
    const rawRecovery = await context.readResource("recovery");
    const recovery = rawRecovery == null
      ? undefined
      : RecoverySchema.parse(rawRecovery);
    const raw = await context.readResource("handoff");
    const cached = raw == null ? undefined : HandoffSchema.parse(raw);
    const implementation = driver.implementation &&
      await currentImplementation(context, driver.implementation, state);
    const delivered = implementation && implementationEvidence(implementation);
    const implementationRef = implementation && delivered
      ? {
        id: implementation.id,
        commit: delivered.commit,
        summary: delivered.summary,
      }
      : undefined;
    let result: Handoff;
    if (
      reusable && cached?.status === "current" &&
      cached.decision === decision && cached.reason === reason &&
      handoffMatchesState(cached, state) && !cached.state &&
      handoffAssignmentId(cached) === assignment?.id &&
      cached.recoveryId === recovery?.id &&
      JSON.stringify(cached.implementationRef) ===
        JSON.stringify(implementationRef)
    ) {
      result = cached;
    } else {
      const value = {
        id: ["authorize_closeout", "resume_closeout"].includes(decision) &&
            state.closeout
          ? state.closeout.offerId
          : dependencies.id(),
        status: "current" as const,
        decision,
        reason,
        createdAt: dependencies.now(),
        ...handoffStateFields(state),
        ...(decision !== "blocked" && state.codeReview &&
            reviewDecisionReady(state)
          ? { codeReviewFingerprint: codeReviewFingerprint(state) }
          : {}),
        ...(state.planReview
          ? { reviewFingerprint: reviewFingerprint(state) }
          : {}),
        ...(assignment
          ? {
            assignmentRef: {
              id: assignment.id,
              role: assignment.role,
              behaviorName: assignment.behaviorName,
            },
          }
          : {}),
        ...(implementationRef ? { implementationRef } : {}),
        ...(recovery ? { recoveryId: recovery.id } : {}),
        ...(decision === "authorize_plan_revision"
          ? { proposedPlanVersion: state.planVersion + 1 }
          : {}),
        ...(state.plan ? { planVersion: state.plan.version } : {}),
        ...(state.codeReview ? { commit: state.codeReview.commit } : {}),
        ...extra,
      };
      result = HandoffSchema.parse({
        ...value,
        presentation: present(value, state, context.definition.name),
      });
    }
    // Publish the handle even when cached so this invocation returns a concrete result.
    handles.push(await context.writeResource("handoff", "handoff", result));
    context.logger.info("{handoff}", { handoff: result.presentation });
    await notifyHandoff(
      context,
      result,
      input.control ?? input.agent?.control ?? driver.agent?.control ??
        assignment?.config.control,
      dependencies.notificationRunner,
    );
    // Swamp requires unique output names even when a foreground invocation has
    // written several versions of state, driver and agent-task along the way.
    return {
      dataHandles: [
        ...new Map(handles.map((handle) => [handle.name, handle])).values(),
      ],
    };
  }

  // Ordinary timeouts observed by this invocation for the current task.
  let timeouts:
    | { taskId: string; rounds: number; stalled: number }
    | undefined;
  /** Log why observation continues; stop after a bounded background-only stall. */
  async function ordinaryTimeoutHandoff(assignment: Task) {
    const progress = timeouts?.taskId === assignment.id
      ? timeouts
      : { taskId: assignment.id, rounds: 0, stalled: 0 };
    const diagnosis = await timeoutDiagnosis(context, assignment);
    progress.rounds++;
    progress.stalled = diagnosis.backgroundOnly ? progress.stalled + 1 : 0;
    timeouts = progress;
    const stalled = progress.stalled >= BACKGROUND_STALL_TIMEOUTS;
    context.logger.info(
      `${
        stalled ? "Stopped observing" : "Continuing original"
      } pane task {task} after ordinary wait timeout {rounds}: phase {phase}, agent state {agentState}, background tasks {backgroundTasks}, submission accepted {submissionAccepted}`,
      {
        task: assignment.id,
        role: assignment.role,
        rounds: progress.rounds,
        stalledRounds: progress.stalled,
        phase: diagnosis.phase,
        agentState: diagnosis.agentState ?? "unknown",
        backgroundTasks: diagnosis.backgroundTasks,
        submissionAccepted: !!diagnosis.receipt,
      },
    );
    if (!stalled) return undefined;
    const evidence = diagnosis.receipt!.payload;
    const stall = {
      taskId: assignment.id,
      role: assignment.role,
      ...(diagnosis.paneId ? { paneId: diagnosis.paneId } : {}),
      backgroundTasks: diagnosis.backgroundTasks,
      ...(evidence.role === "implement" ||
          evidence.role === "implement_review_fixes"
        ? { commit: evidence.evidence.commit }
        : {}),
      rounds: progress.stalled,
      observedAt: dependencies.now(),
      child: {
        phase: diagnosis.phase as "waiting" | "background",
        ...(diagnosis.child.outcome
          ? { outcome: diagnosis.child.outcome }
          : {}),
        updatedAt: diagnosis.child.updatedAt,
      },
    };
    driver = { ...driver, backgroundStall: stall };
    handles.push(await context.writeResource("driver", "driver", driver));
    return await handoff("blocked", backgroundStallReason(stall), {}, true);
  }

  try {
    const rawRecovery = await context.readResource("recovery");
    let recovery = rawRecovery == null
      ? undefined
      : RecoverySchema.parse(rawRecovery);
    if (recovery?.status === "pending") {
      // The rollback's own recorded request is the authority. Finish it before
      // reading the task and state, so the driver follows the replacement and
      // any revision approval is checked against the post-recovery handoff.
      // finishRecovery takes no lock; resume_rollback.execute would nest it.
      context.logger.info(
        "Completing saved rollback {id} to {step} (recorded request: {request}); no new authorization is inferred",
        { id: recovery.id, step: recovery.step, request: recovery.request },
      );
      try {
        handles.push(
          ...(await finishRecovery(context, recovery, dependencies))
            .dataHandles,
        );
      } catch (error) {
        throw new Error(
          `Rollback is incomplete (recovery ${recovery.id} to ${recovery.step}): ${
            error instanceof Error ? error.message : error
          }. Rerunning drive retries the saved intent; a new explicit rollback may supersede it.`,
        );
      }
      recovery = RecoverySchema.parse(await context.readResource("recovery"));
    }
    const currentTask = await task();
    const state = await loadState(context);
    if (input.agent) {
      // A complete explicit provider selection keeps its existing replacement
      // semantics. Session/launch-only inputs inherit the issue or driver choice.
      const inherited = input.agent.kind
        ? {}
        : driver.agent ?? (currentTask
          ? Settings.strip().parse(currentTask.config)
          : state.workerAgent);
      const session = input.agent.session ?? driver.agent?.session ??
        input.agent.control?.session ?? input.control?.session ??
        currentTask?.config.session;
      if (!session) {
        throw new Error(
          "Worker session is required; run drive from a controlling Yinz pane or supply agent.session and control identity",
        );
      }
      const requestedAgent = Settings.parse({
        ...inherited,
        ...input.agent,
        session,
      });
      if (currentTask && ["running", "applying"].includes(currentTask.status)) {
        const {
          cwd: _cwd,
          control: _control,
          stackWorkers: _stackWorkers,
          ...original
        } = currentTask.config;
        const { control: _inputControl, ...requested } = requestedAgent;
        if (
          JSON.stringify(Settings.parse(original)) !==
            JSON.stringify(requested)
        ) {
          throw new Error(
            "Unresolved work must resume with its original session/provider settings; use an explicitly requested restart to replace it",
          );
        }
      }
      const control = requestedAgent.control ?? driver.agent?.control ??
        currentTask?.config.control;
      driver.agent = {
        ...requestedAgent,
        ...(control ? { control } : {}),
      };
      if (currentTask) driver.assignmentId = currentTask.id;
    } else if (
      currentTask &&
      (!driver.agent || currentTask.id !== driver.assignmentId) &&
      // A saved revision may not have reached the agent-task write yet. Keep
      // its selected settings instead of adopting the preceding review's config.
      !(driver.assignmentId === driver.revisionAuthorization?.assignment.id &&
        currentTask.id ===
          driver.revisionAuthorization?.previousAssignmentId) &&
      !(driver.assignmentId === driver.automaticRevision?.assignment.id &&
        currentTask.id === driver.automaticRevision?.previousAssignmentId)
    ) {
      const { cwd: _cwd, stackWorkers: _stackWorkers, ...original } =
        currentTask.config;
      driver.agent = Settings.parse(original);
      driver.assignmentId = currentTask.id;
    }
    if (!driver.agent && state.workerAgent) {
      const session = input.control?.session;
      if (!session) {
        throw new Error(
          "Worker session is required; run drive from a controlling Yinz pane or supply agent.session and control identity",
        );
      }
      driver.agent = Settings.parse({ ...state.workerAgent, session });
    }
    // Placement applies only to future launches; never rewrite an active task.
    if (driver.agent && input.control) {
      driver.agent = { ...driver.agent, control: input.control };
    }
    if (input.revisionApproval) {
      const approval = input.revisionApproval;
      const state = await loadState(context);
      const fresh = await dependencies.readAgenda(
        state.issue.workspace,
        state.issue.itemId,
      );
      if (fresh.digest !== state.issue.digest) {
        throw new Error(
          "Agenda requirements changed; refresh before authorizing a revision",
        );
      }
      const authorization = driver.revisionAuthorization;
      const current = stateDigest(state);
      const retry = authorization?.handoffId === approval.handoffId &&
        authorization.request === approval.request &&
        authorization.recoveryId === recovery?.id &&
        ((currentTask?.id === authorization.previousAssignmentId &&
          taskBeforeDigest(authorization.assignment) === current) ||
          (currentTask?.id === authorization.assignment.id &&
            ["running", "applying", "completed"].includes(currentTask.status) &&
            (taskBeforeDigest(currentTask) === current ||
              taskNextDigest(currentTask) === current)));
      if (!retry) {
        const raw = await context.readResource("handoff");
        const proposal = raw == null ? undefined : HandoffSchema.parse(raw);
        if (
          proposal?.id !== approval.handoffId ||
          proposal.status !== "current" ||
          proposal.decision !== "authorize_plan_revision" ||
          !handoffMatchesState(proposal, state) ||
          handoffAssignmentId(proposal) !== currentTask?.id ||
          proposal.recoveryId !== recovery?.id ||
          proposal.proposedPlanVersion !== state.planVersion + 1
        ) {
          throw new Error(
            "Revision approval requires the current authorize_plan_revision handoff; this approval is stale or has the wrong handoff type",
          );
        }
        if (!driver.agent) {
          throw new Error(
            "Select agent settings before authorizing a revision",
          );
        }
        const assignment = await prepareTask(
          context,
          { ...driver.agent, role: "plan", request: approval.request },
          state,
          dependencies,
        );
        driver.revisionAuthorization = {
          ...approval,
          createdAt: dependencies.now(),
          assignment: persistedTask(assignment),
          ...(currentTask ? { previousAssignmentId: currentTask.id } : {}),
          ...(recovery ? { recoveryId: recovery.id } : {}),
        };
      }
    }
    if (input.autoRepair && "withdraw" in input.autoRepair) {
      const { autoRepair: _withdrawn, ...rest } = driver;
      driver = rest;
    } else if (input.autoRepair) {
      const state = await loadState(context);
      driver = {
        ...driver,
        autoRepair: {
          request: input.autoRepair.request,
          createdAt: dependencies.now(),
          issueDigest: state.issue.digest,
          planVersion: state.planVersion,
          ...(state.rollbackId ? { rollbackId: state.rollbackId } : {}),
          rounds: 0,
        },
      };
    }
    // Persist the authorization and exact assignment together before dispatch.
    handles.push(await context.writeResource("driver", "driver", driver));

    while (true) {
      context.signal.throwIfAborted();
      const assignment = await task();
      const stall = driver.backgroundStall;
      if (stall) {
        // Re-present the bounded stall only while nothing has observed the child
        // since; a later resume-agent outcome supersedes it.
        const child = stall.taskId === assignment?.id &&
            assignment.status === "running" && !assignment.acceptance
          ? await childState(context, assignment).catch(() => undefined)
          : undefined;
        if (
          assignment && child && child.phase === stall.child.phase &&
          child.outcome === stall.child.outcome &&
          child.updatedAt === stall.child.updatedAt &&
          await acceptedEvidence(context, assignment).then(
            () => true,
            () => false,
          )
        ) {
          return await handoff(
            "blocked",
            backgroundStallReason(stall),
            {},
            true,
          );
        }
        const { backgroundStall: _superseded, ...rest } = driver;
        driver = rest;
        handles.push(await context.writeResource("driver", "driver", driver));
      }
      if (
        assignment &&
        ((assignment.status === "completed" &&
          assignment.cleanup === "pending") ||
          ["running", "applying"].includes(assignment.status))
      ) {
        driver = { ...driver, assignmentId: assignment.id };
        handles.push(await context.writeResource("driver", "driver", driver));
        const stalled = await settle(
          context,
          dependencies,
          budget,
          handles,
          assignment,
          () => resumeTask(context, dependencies),
          ordinaryTimeoutHandoff,
        );
        if (stalled) return stalled;
        continue;
      }

      const state = await loadState(context);
      const fresh = await dependencies.readAgenda(
        state.issue.workspace,
        state.issue.itemId,
        state.phase === "done",
      );
      if (fresh.digest !== state.issue.digest) {
        const impact = refreshImpact(state, fresh);
        // Only a human revision authorization reserved for this exact state
        // can be voided before approval. Any running agent-task has already
        // been resolved above, and refresh makes automatic reservations
        // inapplicable because their `before` no longer matches.
        const authorization = driver.revisionAuthorization;
        const unspent = !!authorization &&
          taskBeforeDigest(authorization.assignment) === stateDigest(state);
        if (impact.automatic && !unspent) {
          const refreshed = refreshIssue(state, fresh);
          context.logger.info(
            "Agenda requirements changed during {phase} before any approval; refreshing to planning generation {generation}: {changes}",
            {
              phase: state.phase,
              generation: refreshed.planningContext!.generation,
              changes: impact.changes.join("; "),
            },
          );
          // refreshIssue is deterministic, so a retry after a partial write
          // reuses this event; the generation keeps an A->B->A->B history distinct.
          handles.push(
            ...(await saveState(
              context,
              "refresh",
              refreshed,
              `event-refresh-${state.issue.digest.slice(0, 16)}-${
                fresh.digest.slice(0, 16)
              }-g${refreshed.planningContext!.generation}`,
            )).dataHandles,
          );
          continue;
        }
        return await handoff(
          "blocked",
          requirementsChangedReason(
            state,
            impact,
            unspent ? authorization!.handoffId : undefined,
          ),
          {
            requirementsChange: {
              previousDigest: state.issue.digest,
              digest: fresh.digest,
              changes: impact.changes,
              invalidates: impact.invalidates,
            },
          },
        );
      }
      let role: z.infer<typeof AgentInputSchema>["role"];
      if (state.phase === "triaging") role = "triage";
      else if (state.phase === "classified") role = "plan";
      else if (state.phase === "planning") {
        if (!state.plan) throw new Error("Planning has no persisted plan");
        if (
          !state.planReview ||
          state.planReview.planVersion !== state.plan.version
        ) role = "review_plan";
        else if (blockedFindings(effectivePlanFindings(state))) {
          role = "plan";
        } else {
          return await handoff(
            "approve_plan",
            "Explicit approval of this reviewed plan is required before implementation",
          );
        }
      } else if (state.phase === "approved") {
        // The recorded worker worktree, or the implementation a rollback kept,
        // determines the binding; the plan approval is already recorded.
        if (
          (state.workerWorktree || state.implementation) &&
          !state.closeout?.authorizedRequest
        ) {
          const adopted = await adoptWorkerWorktree(
            context,
            state,
            {},
            dependencies,
          );
          handles.push(...adopted.dataHandles);
          const implementation = (await loadState(context)).implementation!;
          context.logger.info(
            "Adopted worker worktree {worktree} on branch {branch} for the approved plan",
            implementation,
          );
          continue;
        }
        return await handoff(
          "prepare_worktree",
          state.closeout?.authorizedRequest
            ? "Closeout may have removed the recorded worktree; deliberately select a new worktree with implement, then drive"
            : "Prepare or adopt the issue worktree with implement, then drive the approved plan",
        );
      } else if (state.phase === "implementing") {
        if (
          !state.plan || state.approval?.planVersion !== state.plan.version ||
          !state.implementation
        ) {
          throw new Error(
            "Implementation requires explicit approval of the current plan and a prepared worktree",
          );
        }
        const repo = await dependencies.inspect(
          context,
          state.implementation.worktree,
        );
        if (repo.branch !== state.implementation.branch) {
          throw new Error("Issue worktree branch changed");
        }
        if (state.codeReview) {
          const atReview = repo.clean && repo.head === state.codeReview.commit;
          const gate = implementationMergeGate(state);
          // A check that dirtied or moved the worktree makes the review look
          // stale. Its recorded failure is the actionable explanation.
          if (
            (!atReview && gate.kind !== "failed") ||
            state.codeReview.planVersion !== state.plan.version
          ) {
            throw new Error(
              "Code review evidence is stale for the current worktree commit",
            );
          }
          requireCurrentCodeReview(
            state,
            atReview ? repo.head : state.codeReview.commit,
          );
          // Selected repairs go straight to a worker: legacy repair plans,
          // approved or not, are guidance only.
          if (activeReviewFix(state)) {
            role = "implement_review_fixes";
          } else if (pendingReviewFindings(state).length) {
            const auto = autoRepairDecision(state);
            if (auto.decision) {
              // Validate before spending a round of the authorization.
              const decided = transition(state, "decide_review", auto.decision);
              driver = {
                ...driver,
                autoRepair: {
                  ...driver.autoRepair!,
                  rounds: driver.autoRepair!.rounds + 1,
                },
              };
              handles.push(
                await context.writeResource("driver", "driver", driver),
              );
              handles.push(
                ...(await saveState(context, "decide_review", decided))
                  .dataHandles,
              );
              continue;
            }
            return await handoff(
              "address_review_findings",
              (state.uxReview
                ? "Present technical and UX findings and ask which to repair or ignore before continuing"
                : "Present the code review's findings and ask which to repair or ignore; UX review runs after code findings are settled") +
                (auto.reason ? `. ${auto.reason}` : ""),
            );
          } else if (!state.uxReview) {
            role = "review_ux";
          } else {
            requireCurrentReviews(
              state,
              atReview ? repo.head : state.codeReview.commit,
            );
            requireReviewAcceptance(state);
            if (gate.kind === "legacy" && gate.failedPreliminary) {
              return await handoff(
                "blocked",
                "Resolve failed verification, then obtain fresh review evidence",
              );
            }
            if (gate.kind === "unverified") {
              // Deterministic and authorization-free. Run inline: this method
              // already holds the issue lock that a nested method would need.
              handles.push(
                ...(await runVerification(context, state, dependencies))
                  .dataHandles,
              );
              continue;
            }
            if (gate.kind === "failed") {
              const { verification } = gate;
              const failures = verificationFailureItems(
                verification,
                (log) => checkLogQuery(context.definition.name, log),
              );
              const staleBase = (verification.baseline ?? []).some((check) =>
                  check.status === "failed"
                )
                ? ". Baseline checks ran at the un-rebased reviewed commit: a baseline failure in code this branch did not change can come from its older base, and the repaired worker rebases onto local main when main already fixes it"
                : "";
              return await handoff(
                "blocked",
                `Verification ${verification.id} failed at ${verification.commit}: ${
                  failures.join("; ")
                }. Run repair (no input needed: the recorded failures become lastFailure for the implementation worker; add a reason only for context the logs lack), or rerun verify explicitly for an environment failure at this same commit${staleBase}${
                  atReview
                    ? ""
                    : ". The worktree is no longer clean at the reviewed commit: obtain review of its new commit, or restore the reviewed commit before rerunning verify"
                }`,
              );
            }
            return await handoff(
              "authorize_merge",
              "Explicit merge authorization for this reviewed commit is required; plan approval does not authorize merging",
            );
          }
        } else {
          // A discarded review launch does not erase the completed implementation.
          const implementation = assignment?.role === "implement"
            ? assignment
            : driver.implementation;
          const repaired = state.reviewDecisions?.at(-1)?.result;
          if (
            repaired && repo.clean && repo.head === repaired.commit &&
            state.implementation.head === repaired.commit
          ) {
            role = "review_code";
          } else if (
            recovery?.status === "completed" &&
            recovery.id === state.rollbackId &&
            recovery.step === "review_code"
          ) {
            role = "review_code";
          } else if (
            implementation?.status === "completed" &&
            taskNextDigest(implementation) === stateDigest(state)
          ) {
            const evidence = implementationEvidence(implementation);
            if (!repo.clean || repo.head !== evidence.commit) {
              throw new Error(
                "Implementation evidence is stale for the current worktree commit",
              );
            }
            driver = {
              ...driver,
              implementation: persistedTask(implementation),
            };
            handles.push(
              await context.writeResource("driver", "driver", driver),
            );
            role = "review_code";
          } else role = "implement";
        }
      } else if (state.phase === "ready") {
        if (state.attempt) {
          const raw = await dependencies.gitOperation(context, {
            action: "receipt",
            attemptId: state.attempt.id,
          });
          if (z.object({ status: z.string() }).parse(raw).status === "merged") {
            handles.push(
              ...(await saveState(
                context,
                "reconcile_merge",
                transition(state, "merged", {
                  receipt: normalizeReceipt(raw, state),
                }),
              )).dataHandles,
            );
            continue;
          }
        }
        requireCurrentReviews(state, state.mergeAuthorization?.commit);
        try {
          requireReviewAcceptance(state);
        } catch {
          if (state.attempt) {
            throw new Error(
              "Outstanding review findings need user direction; roll back to ready before deciding findings for this unlanded attempt",
            );
          }
          return await handoff(
            "address_review_findings",
            "Present outstanding technical and UX findings before proceeding with this legacy merge authorization",
          );
        }
        const failure = activeMergeFailure(state);
        if (failure) {
          return await handoff(
            "blocked",
            `${
              describeMergeFailure(failure)
            }. Read the log and run repair with the cause; treat a flake or timeout as a defect, not a reason to retry`,
          );
        }
        return await handoff(
          "integrate",
          "Merge authorization is recorded; run merge with the required CI gate (a waiver requires its own explicit user request)",
        );
      } else if (state.phase === "merged") {
        return await handoff(
          "summarize",
          "Compare the merge receipt with the requirements and record the delivered outcome before Agenda close-out",
        );
      } else {
        if (
          state.closeout?.decision === "declined" ||
          (state.closeout?.git === "complete" &&
            ["closed", "absent"].includes(state.closeout.pane))
        ) {
          return await handoff(
            "complete",
            closeoutPresentation(state),
          );
        }
        let offered = state;
        try {
          if (dependencies.offerCloseout) {
            const result = await dependencies.offerCloseout(context, state);
            offered = result.state;
            handles.push(...result.dataHandles);
          }
        } catch (error) {
          if (state.closeout?.authorizedRequest) throw error;
          return await handoff(
            "complete",
            `The issue is merged and its outcome is recorded. Optional closeout is unavailable; remaining resources and recorded cleanup progress are retained: ${error}`,
          );
        }
        if (
          offered.closeout?.git === "complete" &&
          ["closed", "absent"].includes(offered.closeout.pane)
        ) {
          return await handoff("complete", closeoutPresentation(offered));
        }
        return await handoff(
          offered.closeout?.decision === "accepted"
            ? "resume_closeout"
            : "authorize_closeout",
          offered.closeout
            ? closeoutPresentation(offered)
            : "Original controller identity unavailable; call closeout with an exact explicit binding to create an offer, or retain resources",
        );
      }

      let next: Task;
      if (role === "plan" && !initialPlanning(state)) {
        const authorization = driver.revisionAuthorization;
        const automatic = driver.automaticRevision;
        const applicable = (
          reservation: typeof automatic | typeof authorization,
        ) =>
          reservation && reservation.previousAssignmentId === assignment?.id &&
          reservation.recoveryId === recovery?.id &&
          taskBeforeDigest(reservation.assignment) === stateDigest(state);
        if (applicable(authorization)) next = authorization!.assignment;
        else if (applicable(automatic)) next = automatic!.assignment;
        else if (
          state.planReview && blockedFindings(effectivePlanFindings(state)) &&
          !(state.planningContext?.automaticRevisionConsumed ??
            state.planVersion > 1) &&
          automatic?.generation !== planningGeneration(state)
        ) {
          if (!driver.agent) {
            throw new Error("Select agent settings before revising");
          }
          next = await prepareTask(
            context,
            { ...driver.agent, role },
            state,
            dependencies,
          );
          // Reserve the generation's allowance with the exact assignment in one write.
          // This survives failures before agent-task persistence and is never cleared by rollback.
          driver.automaticRevision = {
            generation: planningGeneration(state),
            assignment: persistedTask(next),
            ...(assignment ? { previousAssignmentId: assignment.id } : {}),
            ...(recovery ? { recoveryId: recovery.id } : {}),
          };
        } else {
          return await handoff(
            "authorize_plan_revision",
            "The automatic revision is unavailable; adjudicate supported findings or explicitly authorize one targeted revision and review",
          );
        }
      } else {
        if (!driver.agent) {
          throw new Error(
            "Select a running session and provider with drive {agent: {session, kind, ...}} before dispatching workers",
          );
        }
        next = await prepareTask(
          context,
          { ...driver.agent, role },
          state,
          dependencies,
        );
      }
      driver = { ...driver, assignmentId: next.id };
      handles.push(await context.writeResource("driver", "driver", driver));
      const stalled = await settle(
        context,
        dependencies,
        budget,
        handles,
        next,
        () => runTask(context, next, dependencies),
        ordinaryTimeoutHandoff,
      );
      if (stalled) return stalled;
    }
  } catch (error) {
    context.signal.throwIfAborted();
    return await handoff(
      "blocked",
      error instanceof Error ? error.message : String(error),
    );
  }
}

export function createDriveMethod(
  dependencies: IssueAgentDependencies = defaultDependencies,
) {
  return {
    kind: "action" as const,
    description:
      "Advance persisted pane work in the foreground until a human decision or blocker, without granting authorization",
    arguments: DriveInputSchema,
    execute: (
      input: z.infer<typeof DriveInputSchema>,
      context: IssueAgentContext,
    ) =>
      dependencies.withIssue(
        context,
        () => drive(DriveInputSchema.parse(input), context, dependencies),
      ),
  };
}
