import { closeoutPresentation } from "./issue_closeout.ts";
/** Foreground orchestration up to a human decision, with durable pane recovery. */
import { z } from "npm:zod@4.6.7";
import {
  AgentInputSchema,
  AgentTaskSchema,
  childState,
  defaultDependencies,
  implementationEvidence,
  type IssueAgentContext,
  type IssueAgentDependencies,
  prepareTask,
  RecoverySchema,
  resumeTask,
  runTask,
} from "./issue_agents.ts";
import {
  activeMergeFailure,
  activeReviewFix,
  codeReviewFingerprint,
  describeMergeFailure,
  effectivePlanFindings,
  FindingSchema,
  implementationMergeGate,
  initialPlanning,
  type LifecycleState,
  planningGeneration,
  requireCurrentReviews,
  requireReviewAcceptance,
  reviewFingerprint,
  StateSchema,
  transition,
  UserRequestSchema,
} from "./lifecycle.ts";
import {
  type Handle,
  loadState,
  normalizeReceipt,
  saveState,
} from "./runtime.ts";
import {
  checkLogQuery,
  checkOutcome,
  displayCommand,
  runVerification,
  verificationChecklist,
} from "./verification.ts";
import { AgentResponseSchema, ControlIdentitySchema } from "./yinz_agent.ts";

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
})
  .strip();
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
}).strict();
export const HandoffSchema = z.object({
  id: z.uuid(),
  status: z.enum(["current", "invalidated"]),
  decision: z.enum([
    "authorize_plan_revision",
    "approve_plan",
    "prepare_worktree",
    "authorize_merge",
    "address_review_findings",
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
  state: StateSchema,
  assignment: AgentTaskSchema.optional(),
  implementation: AgentTaskSchema.optional(),
  recoveryId: z.uuid().optional(),
  proposedPlanVersion: z.number().int().positive().optional(),
  planVersion: z.number().int().positive().optional(),
  commit: z.string().optional(),
  reviewFingerprint: z.string().optional(),
  codeReviewFingerprint: z.string().optional(),
  presentation: z.string().min(1),
}).strict();
type Handoff = z.infer<typeof HandoffSchema>;
type Task = z.infer<typeof AgentTaskSchema>;

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
  issueName: string,
) {
  const state = handoff.state;
  const lines = [
    `Issue ${state.issue.workspace}/${state.issue.itemId}: ${state.issue.text}`,
    `Handoff ${handoff.id}: ${handoff.decision}`,
    handoff.reason,
    ...(handoff.planVersion ? [`Plan version: ${handoff.planVersion}`] : []),
    ...(handoff.commit ? [`Commit: ${handoff.commit}`] : []),
    `Requirements: ${state.issue.note}`,
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
      `To authorize this one revision, call drive with revisionApproval: {handoffId: "${handoff.id}", request: "<the user's actual authorization>"}.`,
      "Revision authorization does not approve implementation or merge.",
      ...(state.planReview && state.plan
        ? [
          `Alternatively, inspect cited code and record supported dispositions with adjudicate-plan using planVersion ${state.plan.version} and reviewFingerprint ${
            reviewFingerprint(state)
          }. Accepted unresolved high/critical risks still require user direction.`,
        ]
        : []),
    );
  }
  if (handoff.decision === "address_review_findings") {
    lines.push(
      "Which findings would you like addressed? You may ignore any findings or sign off on merge with findings outstanding.",
      `Record your selection with decide-review: {id: "${handoff.id}", reviewFingerprint: "${handoff.codeReviewFingerprint}", request: "<actual user direction>", repair: [{role: "review_code", findingId: "..."}], ignore: []}. Planning is authorized only for selected repairs; implementation needs separate plan approval.`,
    );
  }
  if (handoff.decision === "approve_review_fixes") {
    lines.push(
      "Review the proposed repairs and checks, then approve-review-fixes with the repair id, this handoffId, and the actual user request. Approval authorizes only this repair plan.",
    );
  }
  for (const decision of state.reviewDecisions ?? []) {
    lines.push(
      `Review decision ${decision.id}: ${decision.request}`,
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
        "Accepted planning decisions:",
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
          f.resolution ? `, resolved: ${f.resolution}` : ", unresolved"
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
            finding.resolution ? ` Resolution: ${finding.resolution}` : ""
          }${
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
  if (handoff.implementation?.evidence) {
    lines.push(
      `Delivered change: ${
        implementationEvidence(handoff.implementation)
          .summary
      }`,
    );
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
  if (handoff.assignment) {
    lines.push(
      `Worker: ${handoff.assignment.role}, task ${handoff.assignment.id}, behavior ${handoff.assignment.behaviorName}`,
    );
  }
  lines.push(
    "The controlling agent must collect and present this result before ending its turn.",
  );
  return lines.join("\n");
}

/** Continue only a correlated, acknowledged ordinary timeout, never a dialog or uncertain delivery. */
async function ordinaryTimeout(
  context: IssueAgentContext,
  task: Task,
  error: unknown,
) {
  const child = await childState(context, task);
  // Match the current persisted failure, not a particular human-readable error.
  // A transport error after an older timeout must never reuse that timeout.
  if (
    !["spawned", "waiting", "background"].includes(child.phase) ||
    child.failureKind !== "timeout" ||
    child.outcome !== "timed_out" || !child.paneId || !child.rawResponse ||
    !(error instanceof Error) ||
    (child.lastError !== error.message &&
      `Error: ${child.lastError}` !== error.message) ||
    (child.phase !== "spawned" &&
      (!child.conversationId || !child.submissionId))
  ) return false;
  const parsed = AgentResponseSchema.safeParse(JSON.parse(child.rawResponse));
  if (!parsed.success) return false;
  const response = parsed.data;
  const matches = response.agents.filter((agent) =>
    agent.pane_id === child.paneId
  );
  if (matches.length !== 1) return false;
  const agent = matches[0];
  return response.outcome === "timed_out" &&
    (!child.launchServer || !response.server_instance ||
      response.server_instance === child.launchServer) &&
    (!child.turnId || !response.turn_id || response.turn_id === child.turnId) &&
    (!agent.cwd || agent.cwd === task.config.cwd) &&
    response.prompt_delivery !== "not_sent" &&
    (child.phase === "spawned" || response.acknowledged || !!response.turn_id ||
      (child.phase === "background" && !!child.turnId)) &&
    (child.phase === "spawned" ||
      response.submission_id === child.submissionId ||
      (child.phase === "background" && response.submission_id == null)) &&
    agent.session === task.config.session &&
    agent.agent.kind === task.config.kind &&
    (child.conversationId == null ||
      agent.agent.session_id === child.conversationId) &&
    agent.agent.state.toLowerCase() === "busy" &&
    (agent.not_ready_reason == null ||
      ["not_idle", "unresolved_turn"].includes(agent.not_ready_reason));
}

async function workerBlocker(
  context: IssueAgentContext,
  task: Task,
  error: unknown,
) {
  const child = await childState(context, task).catch(() => undefined);
  let observed: string | undefined;
  if (child?.rawResponse) {
    try {
      const response = AgentResponseSchema.parse(JSON.parse(child.rawResponse));
      observed = `Latest response: outcome ${response.outcome}, server ${
        response.server_instance ?? "unspecified"
      }, turn ${response.turn_id ?? "unspecified"}, submission ${
        response.submission_id ?? "unspecified"
      }; expected server ${child.launchServer ?? "unknown"}, turn ${
        child.turnId ?? "unknown"
      }.`;
    } catch { /* The original error retains malformed-response details. */ }
  }
  return new Error([
    error instanceof Error ? error.message : String(error),
    ...(observed ? [observed] : []),
    `Original task ${task.id}, child ${task.behaviorName}, session ${task.config.session}, pane ${
      child?.paneId ?? "unknown"
    }, conversation ${child?.conversationId ?? "unknown"}, submission ${
      child?.submissionId ?? "unknown"
    }.`,
    "Completion is not reconciled. Inspect the saved child state and original submission with resume-agent; do not resend the prompt or infer termination from a closed pane or elapsed time.",
  ].join("\n"));
}

/** The caller owns the issue lock. Helpers here deliberately do not acquire it again. */
async function drive(
  input: z.infer<typeof DriveInputSchema>,
  context: IssueAgentContext,
  dependencies: IssueAgentDependencies,
) {
  const handles: Handle[] = [];
  const rawDriver = await context.readResource("driver");
  let driver = DriverSchema.parse(rawDriver ?? {});

  async function task() {
    const raw = await context.readResource("agent-task");
    return raw == null ? undefined : AgentTaskSchema.parse(raw);
  }
  async function handoff(decision: Handoff["decision"], reason: string) {
    const state = await loadState(context);
    const assignment = await task();
    const rawRecovery = await context.readResource("recovery");
    const recovery = rawRecovery == null
      ? undefined
      : RecoverySchema.parse(rawRecovery);
    const raw = await context.readResource("handoff");
    const cached = raw == null ? undefined : HandoffSchema.parse(raw);
    const candidate = driver.implementation;
    const implementation = candidate?.status === "completed" &&
        candidate.evidence?.payload.role === "implement" &&
        candidate.next?.plan?.version === state.plan?.version &&
        candidate.next?.rollbackId === state.rollbackId &&
        candidate.next?.issue.digest === state.issue.digest &&
        candidate.next?.implementation?.worktree ===
          state.implementation?.worktree &&
        (!state.codeReview ||
          implementationEvidence(candidate).commit ===
            state.codeReview.commit)
      ? candidate
      : undefined;
    let result: Handoff;
    if (
      decision !== "blocked" && cached?.status === "current" &&
      cached.decision === decision && cached.reason === reason &&
      JSON.stringify(cached.state) === JSON.stringify(state) &&
      cached.assignment?.id === assignment?.id &&
      cached.recoveryId === recovery?.id &&
      JSON.stringify(cached.implementation) === JSON.stringify(implementation)
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
        state,
        ...(decision !== "blocked" && state.codeReview && state.uxReview
          ? { codeReviewFingerprint: codeReviewFingerprint(state) }
          : {}),
        ...(state.planReview
          ? { reviewFingerprint: reviewFingerprint(state) }
          : {}),
        ...(assignment ? { assignment } : {}),
        ...(implementation ? { implementation } : {}),
        ...(recovery ? { recoveryId: recovery.id } : {}),
        ...(decision === "authorize_plan_revision"
          ? { proposedPlanVersion: state.planVersion + 1 }
          : {}),
        ...(state.plan ? { planVersion: state.plan.version } : {}),
        ...(state.codeReview ? { commit: state.codeReview.commit } : {}),
      };
      result = HandoffSchema.parse({
        ...value,
        presentation: present(value, context.definition.name),
      });
    }
    // Publish the handle even when cached so this invocation returns a concrete result.
    handles.push(await context.writeResource("handoff", "handoff", result));
    context.logger.info("{handoff}", { handoff: result.presentation });
    // Swamp requires unique output names even when a foreground invocation has
    // written several versions of state, driver and agent-task along the way.
    return {
      dataHandles: [
        ...new Map(handles.map((handle) => [handle.name, handle])).values(),
      ],
    };
  }

  try {
    const rawRecovery = await context.readResource("recovery");
    const recovery = rawRecovery == null
      ? undefined
      : RecoverySchema.parse(rawRecovery);
    if (recovery?.status === "pending") {
      throw new Error(
        "Rollback is incomplete; use resume-rollback to finish its recorded intent, then drive",
      );
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
      const sameState = (value: unknown) =>
        JSON.stringify(value) === JSON.stringify(state);
      const retry = authorization?.handoffId === approval.handoffId &&
        authorization.request === approval.request &&
        authorization.recoveryId === recovery?.id &&
        ((currentTask?.id === authorization.previousAssignmentId &&
          sameState(authorization.assignment.before)) ||
          (currentTask?.id === authorization.assignment.id &&
            ["running", "applying", "completed"].includes(currentTask.status) &&
            (sameState(currentTask.before) || sameState(currentTask.next))));
      if (!retry) {
        const raw = await context.readResource("handoff");
        const proposal = raw == null ? undefined : HandoffSchema.parse(raw);
        if (
          proposal?.id !== approval.handoffId ||
          proposal.status !== "current" ||
          proposal.decision !== "authorize_plan_revision" ||
          !sameState(proposal.state) ||
          proposal.assignment?.id !== currentTask?.id ||
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
          assignment,
          ...(currentTask ? { previousAssignmentId: currentTask.id } : {}),
          ...(recovery ? { recoveryId: recovery.id } : {}),
        };
      }
    }
    // Persist the authorization and exact assignment together before dispatch.
    handles.push(await context.writeResource("driver", "driver", driver));

    while (true) {
      context.signal.throwIfAborted();
      const assignment = await task();
      if (
        assignment &&
        ((assignment.status === "completed" &&
          assignment.cleanup === "pending") ||
          ["running", "applying"].includes(assignment.status))
      ) {
        driver = { ...driver, assignmentId: assignment.id };
        handles.push(await context.writeResource("driver", "driver", driver));
        try {
          handles.push(
            ...(await resumeTask(context, dependencies)).dataHandles,
          );
        } catch (error) {
          context.signal.throwIfAborted();
          if (
            !await ordinaryTimeout(context, assignment, error).catch(() =>
              false
            )
          ) throw await workerBlocker(context, assignment, error);
          context.logger.info(
            "Continuing original pane task {task} after an ordinary wait timeout",
            { task: assignment.id },
          );
        }
        continue;
      }

      const state = await loadState(context);
      const fresh = await dependencies.readAgenda(
        state.issue.workspace,
        state.issue.itemId,
        state.phase === "done",
      );
      if (fresh.digest !== state.issue.digest) {
        throw new Error(
          "Agenda requirements changed; refresh before continuing or presenting an approval handoff",
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
        return await handoff(
          "prepare_worktree",
          state.workerWorktree
            ? `Adopt the recorded worker worktree ${state.workerWorktree} with implement, then drive the approved plan`
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
          if (!state.uxReview) {
            role = "review_ux";
          } else {
            requireCurrentReviews(
              state,
              atReview ? repo.head : state.codeReview.commit,
            );
            const fix = activeReviewFix(state);
            if (fix) {
              if (!fix.plan) role = "plan_review_fixes";
              else if (!fix.approval) {
                return await handoff(
                  "approve_review_fixes",
                  "Approve the focused repair plan before an implementation worker edits code",
                );
              } else role = "implement_review_fixes";
            } else {
              try {
                requireReviewAcceptance(state);
              } catch {
                return await handoff(
                  "address_review_findings",
                  "Present technical and UX findings and ask which to repair or ignore before continuing",
                );
              }
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
                const failures = verification.checks
                  .filter((check) => check.status === "failed")
                  .map((check) =>
                    `${check.id} (${
                      check.skipped ??
                        `${checkOutcome(check)}; log: ${
                          checkLogQuery(context.definition.name, check.log!)
                        }`
                    })`
                  );
                return await handoff(
                  "blocked",
                  `Verification ${verification.id} failed at ${verification.commit}: ${
                    failures.join("; ")
                  }. Repair and obtain review of a new commit, or rerun verify explicitly for an environment failure${
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
            JSON.stringify(implementation.next) === JSON.stringify(state)
          ) {
            const evidence = implementationEvidence(implementation);
            if (!repo.clean || repo.head !== evidence.commit) {
              throw new Error(
                "Implementation evidence is stale for the current worktree commit",
              );
            }
            driver = { ...driver, implementation };
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
          JSON.stringify(reservation.assignment.before) ===
            JSON.stringify(state);
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
            assignment: next,
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
      try {
        handles.push(
          ...(await runTask(context, next, dependencies)).dataHandles,
        );
      } catch (error) {
        context.signal.throwIfAborted();
        if (!await ordinaryTimeout(context, next, error).catch(() => false)) {
          throw await workerBlocker(context, next, error);
        }
        context.logger.info(
          "Continuing original pane task {task} after an ordinary wait timeout",
          { task: next.id },
        );
      }
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
