import { captureCloseoutOrigin, offerCloseout } from "./issue_closeout.ts";
/** Assign pane work and consume acknowledged Swamp evidence independently of chat. */
import { join } from "node:path";
import { z } from "npm:zod@4.6.7";
import { AgentKindSchema } from "./worker_selection.ts";
import {
  activeReviewFix,
  codeReviewFingerprint,
  initialPlanning,
  readAgenda,
  recordFeedback,
  refreshIssue,
  reopenAgenda,
  RollbackInputSchema,
  rollbackState,
  StateSchema,
  transition,
  UserRequestSchema,
} from "./lifecycle.ts";
import {
  assignmentCore,
  AssignmentSchema,
  canonical,
  evidenceDigest,
  EvidenceReceiptSchema,
  EvidenceTargetSchema,
  ImplementationResult,
  RoleSchema,
  submissionInstructions,
  SubmissionSchema,
  validateReceipt,
} from "./issue_evidence.ts";
export { ImplementationResult } from "./issue_evidence.ts";
import {
  type Context,
  gitOperation,
  inspect,
  invalidateHandoff,
  loadState,
  normalizeReceipt,
  saveState,
  submissionPreflight,
  withIssue,
} from "./runtime.ts";
import {
  AgentConfigSchema,
  AgentResultSchema,
  AgentStateSchema,
  assertContinuationEligible,
  ControlIdentitySchema,
  isRejectedLaunch,
  isTerminalFailure,
  promptDigest,
} from "./yinz_agent.ts";

const Role = RoleSchema;
export const AgentInputSchema = z.object({
  role: Role,
  request: UserRequestSchema.optional(),
  session: z.string().trim().min(1),
  // Optional while decoding saved settings and tasks from older lifecycles.
  control: ControlIdentitySchema.optional(),
  kind: AgentKindSchema,
  model: z.string().trim().min(1).optional(),
  effort: z.string().trim().min(1).optional(),
  permissionMode: z.string().trim().min(1).optional(),
  startupTimeoutSeconds: z.number().int().positive().max(3600).default(120),
  timeoutSeconds: z.number().int().positive().max(86400).default(600),
}).strict();
const Input = AgentInputSchema;

export const AgentTaskSchema = z.object({
  id: z.uuid(),
  behaviorName: z.string(),
  role: Role,
  config: AgentConfigSchema,
  prompt: z.string().min(1),
  promptDigest: z.string().regex(/^[0-9a-f]{64}$/),
  request: z.string().min(1).optional(),
  before: StateSchema,
  next: StateSchema.optional(),
  createdAt: z.iso.datetime(),
  reviewCommit: z.string().regex(/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/).optional(),
  status: z.enum([
    "running",
    "applying",
    "completed",
    "discarded",
    "superseded",
  ]),
  supersededBy: z.uuid().optional(),
  discard: z.object({
    request: UserRequestSchema,
    confirmedStopped: z.literal(true),
  }).strict().optional(),
  message: z.string().optional(),
  lastError: z.string().optional(),
  cleanup: z.enum(["pending", "done"]).optional(),
  submission: EvidenceTargetSchema.optional(),
  evidence: EvidenceReceiptSchema.optional(),
  continuation: z.object({
    id: z.uuid(),
    request: UserRequestSchema,
    prompt: z.string().min(1),
  }).strict().optional(),
}).strict();
type Task = z.infer<typeof AgentTaskSchema>;
const RestartInput = Input.extend({
  taskId: z.uuid(),
  request: UserRequestSchema,
});
export const RecoverySchema = z.object({
  id: z.uuid(),
  step: RollbackInputSchema.shape.step,
  request: RollbackInputSchema.shape.request,
  createdAt: z.iso.datetime(),
  status: z.enum(["pending", "completed", "superseded"]),
  supersededBy: z.uuid().optional(),
  before: StateSchema,
  next: StateSchema,
  reconciled: StateSchema.optional(),
  task: AgentTaskSchema.optional(),
  replacement: AgentTaskSchema.optional(),
  restart: RestartInput.optional(),
}).strict();
type Recovery = z.infer<typeof RecoverySchema>;
export type IssueAgentContext = Context & {
  runModel: (args: {
    definition?: string;
    modelType?: string;
    name?: string;
    method: string;
    arguments?: Record<string, unknown>;
  }) => Promise<{ ok: true } | { ok: false; error: { message: string } }>;
  dataRepository: {
    getContent: (
      type: string,
      name: string,
      resource: string,
    ) => Promise<Uint8Array | null>;
  };
};

export interface IssueAgentDependencies {
  captureCloseoutOrigin?: typeof captureCloseoutOrigin;
  offerCloseout?: typeof offerCloseout;
  withIssue: typeof withIssue;
  readAgenda: typeof readAgenda;
  inspect: typeof inspect;
  gitOperation: typeof gitOperation;
  reopenAgenda: typeof reopenAgenda;
  realPath: (path: string) => Promise<string>;
  id: () => string;
  now: () => string;
  submissionPreflight: typeof submissionPreflight;
}
export const defaultDependencies: IssueAgentDependencies = {
  captureCloseoutOrigin,
  offerCloseout,
  withIssue,
  readAgenda,
  inspect,
  gitOperation,
  reopenAgenda,
  realPath: (path) => Deno.realPath(path),
  id: () => crypto.randomUUID(),
  now: () => new Date().toISOString(),
  submissionPreflight,
};

async function savedRevisions(context: Context) {
  const reservation = z.object({
    assignment: AgentTaskSchema,
    previousAssignmentId: z.uuid().optional(),
  });
  return z.object({
    automaticRevision: reservation.optional(),
    revisionAuthorization: reservation.optional(),
  }).parse(await context.readResource("driver") ?? {});
}

/** Returns the current state once no assignment or reservation is unresolved. */
export async function assertNoPendingAgent(context: Context) {
  await assertNoPendingRecovery(context);
  const raw = await context.readResource("agent-task");
  const task = raw == null ? undefined : AgentTaskSchema.parse(raw);
  const driver = await savedRevisions(context);
  const state = await loadState(context);
  for (
    const saved of [driver.automaticRevision, driver.revisionAuthorization]
  ) {
    if (
      saved && saved.assignment.id !== task?.id &&
      saved.previousAssignmentId === task?.id &&
      JSON.stringify(saved.assignment.before) === JSON.stringify(state)
    ) {
      throw new Error(
        "Reserved pane assignment is unresolved; resume drive or explicitly roll back/restart",
      );
    }
  }
  if (
    task &&
    ((task.status === "completed" && task.cleanup === "pending") ||
      !["completed", "discarded", "superseded"].includes(task.status))
  ) {
    throw new Error(
      `Pane task ${task.behaviorName} is unresolved; use resume-agent, or an explicitly requested rollback/restart-agent`,
    );
  }
  return state;
}

async function assertNoPendingRecovery(context: Context) {
  const raw = await context.readResource("recovery");
  if (raw != null && RecoverySchema.parse(raw).status === "pending") {
    throw new Error(
      "Rollback is incomplete; use resume-rollback before other lifecycle operations",
    );
  }
}

async function saveTask(context: Context, task: Task) {
  await invalidateHandoff(context);
  return context.writeResource(
    "agent-task",
    "agent-task",
    AgentTaskSchema.parse(task),
  );
}

export function taskPrompt(
  role: z.infer<typeof Role>,
  state: z.infer<typeof StateSchema>,
  cwd: string,
  references: string,
  submission: string,
  commit?: string,
  request?: string,
) {
  const instructions = {
    triage:
      "Investigate the issue against the source. Reproduce a bug when practical. Return its classification and an evidence-based summary, including acceptance criteria and uncertainties.",
    plan:
      "Develop or revise a concrete implementation plan from the issue and investigation. Address existing review findings. Include affected files, risks in step descriptions, assumptions, and a testingStrategy explaining the test selection. Return checks: focused development checks selected per AGENTS.md (never unfiltered full suites), each with a short id, an argv array run verbatim without a shell in the issue worktree, and the plan step IDs it covers. The lifecycle runs every check at the reviewed commit and requires them to pass before merge authorization. Full cargo x ci check is mandatory later in the merge gate unless the user explicitly waives it.",
    review_plan:
      "Adversarially review the current plan against the actual code, including whether its checks are valid command argument arrays that execute directly without a shell, use supported flags, are focused and safe to run unattended, and cover the changed behavior and affected callers. Return planVersion and every actionable finding. Do not approve the plan or invent resolutions. An empty findings array is appropriate only after a real review found none.",
    implement:
      "Implement the approved plan in this existing issue worktree. Run focused checks following AGENTS.md, investigate failures, and commit the intended changes on the existing feature branch. Return a summary and the full resulting commit hash. Do not rebase, merge, push, or run the full CI gate here. If lastFailure records a failed merge gate, read its log first and fix the cause, selecting focused tests that reproduce it; treat a flake or timeout as a defect, not something a rerun resolves.",
    review_code:
      "Review the committed diff against local main, issue requirements and the approved plan. Independently assess whether the behavior is correct, including incorrect assumptions or defects in the approved approach. Trace affected callers and relevant state transitions, failure paths, cleanup, ownership, concurrency and platform behavior as applicable. Inspect tests for missing cases and assertions that could pass despite incorrect behavior. Prioritize actionable defects introduced or exposed by this change. For each finding identify the trigger, consequence and concrete source locations, explain severity and distinguish demonstrated behavior from assumptions. Avoid speculative concerns and unrelated cleanup. An empty findings array is valid after a real review. Cover each plan step exactly once and justify deviations. Record only preliminary checks actually performed. Preliminary checks are advisory; the lifecycle runs the approved plan checks itself. Do not invent passing checks. Return the supplied full commit hash and planVersion.",
    plan_review_fixes:
      "Plan focused repairs for activeReviewFix's selected findings in reviewDecisions. Read the stored technical and UX reviews, the user's request and existing approved plan. Do not edit source. Propose only the selected repairs, explain risks in steps, and include focused argv checks and a testingStrategy. Return the repair plan for user approval; this task authorizes no implementation.",
    implement_review_fixes:
      "Implement only the approved repair plan in the last reviewDecisions entry in this existing issue worktree. Respect ignored findings and the user's selected scope. Run focused checks, investigate failures, and commit changes on the existing feature branch. Return summary and full commit hash. Do not rebase, merge, push, or run full CI.",
    review_ux:
      "Review the supplied committed change specifically for its impact on the user experience. Use the issue requirements, actual implementation, affected callers, tests, help and documentation to identify affected users and tasks. Trace representative end-to-end workflows through discovery, normal use, repeat use, waiting, failure, cancellation and recovery as applicable. Independently assess whether the experience remains understandable, consistent, responsive and usable, even when the code follows the approved plan. Inspect relevant terminal UI, keyboard navigation and focus, rendering/readability, CLI commands and output, configuration/defaults, help/documentation, feedback, errors and lifecycle handoffs. Consider indirect responsiveness, compatibility surprises, extra interaction steps and loss of user work. Can users understand what happened, what is happening and what to do next? Keep implementation details out of user flows unless they support a meaningful user decision. Exercise representative interactions when practical; distinguish observed results from source-based conclusions and state material limitations. Report concrete actionable UX defects introduced or exposed by the change. Each finding needs the affected user/task, triggering interaction sequence, observed or source-derived behavior, user consequence, severity and concrete source or interaction evidence. Avoid personal aesthetic preferences, speculative concerns and unrelated redesigns. An empty findings array is valid after a real review. Always submit assessment with impact, a nonempty rationale, assessedPaths describing affected user tasks/workflows and source paths, and limitations. A no_material_impact result requires a source-based rationale; absence of UI-file changes is insufficient. Preliminary checks are advisory; record only checks actually performed. Return the supplied full commit hash and planVersion.",
  }[role];
  return [
    "You are working on one Yinz issue lifecycle task in a visible Yinz pane.",
    `Work only in ${cwd}. Follow its AGENTS.md and applicable repository instructions.`,
    "Do not invoke the lifecycle launcher. Use only the assigned Swamp evidence interface described below. Do not merge or contact remote Git services.",
    (role === "implement" || role === "implement_review_fixes")
      ? "Only this task may edit the issue worktree."
      : "This is investigation or review: do not edit source files, Git refs, or configuration.",
    "Finish this task in the foreground. Do not leave background subagents or jobs running when returning your answer.",
    instructions,
    ...(["plan", "review_plan", "plan_review_fixes"].includes(role)
      ? [
        `Read ${join(references, "planning.md")} and ${
          join(references, "review.md")
        } before working.`,
        "Preserve unaffected steps, accepted decisions and tradeoffs. Historical context is not current approval evidence. Reuse finding IDs for persistent concerns; reopening a disposed concern requires new source evidence and an explanation of why its prior disposition no longer holds.",
        role === "plan" && !initialPlanning(state)
          ? "This is a targeted revision. Include revision.summary, acceptedDecisions and a response (addressed, disputed with evidence, or unresolved) to each blocking finding."
          : "Check correctness, scope and unnecessary complexity. Every blocker needs an affected requirement, concrete failure mechanism and source evidence. Missing implementation details, preferences or speculative concerns alone are not high severity.",
        ...(role === "review_plan"
          ? [
            "Compare the previous plan, review findings, revision responses and current plan in planningContext. Verify fixes and inspect for new defects. Do not invent resolutions or discard prior accepted tradeoffs.",
          ]
          : []),
      ]
      : []),
    ...(["review_plan", "review_code", "review_ux"].includes(role)
      ? [
        `Read ${join(references, "review.md")} and ${
          join(references, "dimensions.md")
        } before reviewing.`,
        "Follow the shared evidence and severity standards. Leave resolution unset for a newly reported defect: a proposed fix is not a resolution. Use resolution only for a concern verified fixed or otherwise supported as resolved. Preserve finding IDs and supported dispositions; reopening a disposed concern requires new evidence and an explanation of why its disposition no longer holds. Historical evidence is not current approval. Follow AGENTS.md focused-test selection; do not run unfiltered full suites.",
        role === "review_plan"
          ? "Apply relevant Yinz review dimensions to the proposed behavior and verification coverage against source."
          : "Apply relevant Yinz review dimensions to the committed implementation and affected callers.",
        "When applicable, cite a dimension name in description and its authoritative source in evidence alongside concrete affected code or plan locations. Use the existing finding fields; do not add a category property. A dimension citation does not replace requirement, mechanism, source evidence, severity, finding identity, disposition or reopening rules. An inapplicable dimension creates no finding or additional testing obligation.",
      ]
      : []),
    ...(request
      ? ["Exact user instructions for this assignment:", request]
      : []),
    submission,
    "Current lifecycle snapshot (issue requirements and recorded evidence):",
    JSON.stringify(state),
    ...(commit ? [`The commit being reviewed is ${commit}.`] : []),
  ].join("\n\n");
}

export async function childState(context: IssueAgentContext, task: Task) {
  const bytes = await context.dataRepository.getContent(
    "@yinz/agent",
    task.behaviorName,
    "state",
  );
  if (!bytes) {
    throw new Error(
      "Pane task has no durable agent state; inspect its launch before proceeding",
    );
  }
  const child = AgentStateSchema.parse(
    JSON.parse(new TextDecoder().decode(bytes)),
  );
  if (
    JSON.stringify(child.config) !== JSON.stringify(task.config) ||
    child.prompt !== task.prompt || child.promptDigest !== task.promptDigest ||
    await promptDigest(child.prompt) !== task.promptDigest
  ) {
    throw new Error(
      "Pane task state does not match the dispatched configuration and prompt",
    );
  }
  return child;
}

async function completedResult(context: IssueAgentContext, task: Task) {
  const child = await childState(context, task);
  if (
    child.phase !== "completed" || child.outcome !== "completed" ||
    child.backgroundTasks.length
  ) {
    throw new Error(
      "Pane task has not durably completed all foreground and background work",
    );
  }
  const bytes = await context.dataRepository.getContent(
    "@yinz/agent",
    task.behaviorName,
    "result",
  );
  if (!bytes) throw new Error("Pane task has no confirmed completed result");
  const result = AgentResultSchema.parse(
    JSON.parse(new TextDecoder().decode(bytes)),
  );
  if (
    result.session !== task.config.session ||
    result.kind !== task.config.kind ||
    result.cwd !== task.config.cwd ||
    result.promptDigest !== task.promptDigest ||
    result.paneId !== child.paneId ||
    result.conversationId !== child.conversationId ||
    result.submissionId !== child.submissionId ||
    result.turnId !== child.turnId
  ) {
    throw new Error(
      "Pane result does not match its original task and correlated submission",
    );
  }
  return result;
}

async function cleanupTask(context: IssueAgentContext, task: Task) {
  if (task.status !== "completed" || task.cleanup !== "pending") {
    return { dataHandles: [] };
  }
  const result = await context.runModel({
    definition: task.behaviorName,
    method: "cleanup",
  });
  if (!result.ok) {
    throw new Error(`Worker cleanup blocked: ${result.error.message}`);
  }
  return {
    dataHandles: [await saveTask(context, { ...task, cleanup: "done" })],
  };
}

async function completeTask(context: IssueAgentContext, task: Task) {
  const completed: Task = {
    ...task,
    status: "completed",
    cleanup: task.config.stackWorkers ? "pending" : undefined,
  };
  const saved = await saveTask(context, completed);
  const cleaned = await cleanupTask(context, completed);
  return {
    dataHandles: cleaned.dataHandles.length ? cleaned.dataHandles : [saved],
  };
}

function taskAssignment(context: IssueAgentContext, task: Task) {
  if (!task.submission) {
    throw new Error(
      "Legacy task requires a typed submission supplement; use continue-agent on its original worker",
    );
  }
  const core = assignmentCore(context, task, task.submission.controlDir);
  return AssignmentSchema.parse({
    ...core,
    digest: task.submission.assignmentDigest,
    promptDigest: task.promptDigest,
  });
}

async function evidenceOperation(
  context: IssueAgentContext,
  task: Task,
  method: string,
  arguments_: Record<string, unknown>,
) {
  if (!task.submission) return;
  if (
    method === "supersede" &&
    !await context.dataRepository.getContent(
      "@yinz/issue-evidence",
      task.submission.behaviorName,
      "submission",
    )
  ) {
    // Dispatch cannot reach a worker before open commits. A controller killed
    // before that write can supersede its saved task without creating a slot.
    return;
  }
  const outcome = await context.runModel({
    modelType: "@yinz/issue-evidence",
    name: task.submission.behaviorName,
    method,
    arguments: arguments_,
  });
  if (!outcome.ok) {
    throw new Error(
      `Swamp evidence ${method} blocked: ${outcome.error.message}`,
    );
  }
}

async function acceptedEvidence(context: IssueAgentContext, task: Task) {
  const assignment = taskAssignment(context, task);
  const bytes = await context.dataRepository.getContent(
    "@yinz/issue-evidence",
    task.submission!.behaviorName,
    "submission",
  );
  const saved = bytes
    ? SubmissionSchema.parse(JSON.parse(new TextDecoder().decode(bytes)))
    : undefined;
  if (!saved || saved.status === "open") {
    throw new Error(
      "submission_required: provider turn ended without accepted Swamp evidence; use continue-agent to finish submission in the original conversation",
    );
  }
  if (
    saved.status !== "accepted" ||
    canonical(saved.assignment) !== canonical(assignment)
  ) {
    throw new Error(
      "Swamp evidence belongs to a superseded or different assignment",
    );
  }
  return validateReceipt(assignment, saved.receipt);
}

/** Typed implementation evidence, including in cached driver handoffs. */
export function implementationEvidence(task: Task) {
  if (
    task.role !== "implement" || task.evidence?.payload.role !== "implement"
  ) {
    throw new Error(
      "Legacy implementation needs a typed submission supplement; use continue-agent on its original worker before reviewing code",
    );
  }
  if (
    !task.submission ||
    task.evidence.assignmentDigest !== task.submission.assignmentDigest ||
    task.evidence.taskId !== task.id ||
    task.evidence.payloadDigest !== evidenceDigest(task.evidence.payload)
  ) {
    throw new Error("Implementation receipt does not match its task");
  }
  return ImplementationResult.parse(task.evidence.payload.evidence);
}

async function validateResultFreshness(
  context: IssueAgentContext,
  task: Task,
  current: z.infer<typeof StateSchema>,
  dependencies: IssueAgentDependencies,
  commit?: string,
) {
  const fresh = await dependencies.readAgenda(
    current.issue.workspace,
    current.issue.itemId,
  );
  if (fresh.digest !== current.issue.digest) {
    throw new Error(
      "Agenda requirements changed while the pane task ran; its result is stale",
    );
  }
  if (canonical(current) !== canonical(task.before)) {
    throw new Error(
      "Lifecycle changed while the pane task ran; inspect its result before recording new evidence",
    );
  }
  if (
    task.role === "implement" || task.role === "review_code" ||
    task.role === "review_ux" || task.role === "plan_review_fixes" ||
    task.role === "implement_review_fixes"
  ) {
    if (
      !commit ||
      (!["implement", "implement_review_fixes"].includes(task.role) &&
        commit !== task.reviewCommit)
    ) {
      throw new Error(
        "Pane result has no verifiable evidence commit; use continue-agent for a typed submission supplement",
      );
    }
    if (!current.implementation) {
      throw new Error("Pane implementation has no recorded issue worktree");
    }
    const repo = await dependencies.inspect(
      context,
      current.implementation.worktree,
    );
    if (
      !repo.clean || repo.branch !== current.implementation.branch ||
      repo.head !== commit
    ) {
      throw new Error(
        "Pane result does not match the clean issue branch's current commit",
      );
    }
  }
}

async function applyResult(
  context: IssueAgentContext,
  task: Task,
  dependencies: IssueAgentDependencies,
) {
  if (task.discard) return finishDiscard(context, task);
  const current = await loadState(context);
  if (task.status === "completed") return cleanupTask(context, task);
  if (["discarded", "superseded"].includes(task.status)) {
    return { dataHandles: [] };
  }
  // Reconcile applied legacy projections without reading chat. A pending
  // projection still needs current requirements and verifiable commit evidence.
  if (
    !task.submission && task.status === "applying" && task.next
  ) {
    if (
      canonical(task.next) !== canonical(task.before) &&
      canonical(current) === canonical(task.next)
    ) return completeTask(context, task);
    if (canonical(current) !== canonical(task.before)) {
      throw new Error(
        "Lifecycle changed during legacy evidence reconciliation",
      );
    }
    await validateResultFreshness(
      context,
      task,
      current,
      dependencies,
      task.role === "review_code"
        ? task.next.codeReview?.commit
        : task.role === "review_ux"
        ? task.next.uxReview?.commit
        : undefined,
    );
    await saveState(
      context,
      `agent_${task.role}_${task.id}`,
      task.next,
      `event-agent-${task.id}`,
    );
    return completeTask(context, task);
  }
  const receipt = await acceptedEvidence(context, task);
  const result = await completedResult(context, task);
  // An applying task carries its exact accepted receipt. State equality alone
  // cannot prove import for implementation reports, whose next equals before.
  const eventName = `event-agent-${task.id}`;
  const event = await context.readResource(eventName);
  const projection = event == null
    ? undefined
    : z.object({ action: z.string(), state: StateSchema }).parse(event);
  const projected = projection?.action === `agent_${task.role}_${task.id}` &&
    canonical(projection.state) === canonical(receipt.next) &&
    canonical(current) === canonical(receipt.next);
  if (
    task.status === "applying" && task.evidence &&
    canonical(task.evidence) === canonical(receipt) && projected
  ) {
    return completeTask(context, task);
  }
  await validateResultFreshness(
    context,
    task,
    current,
    dependencies,
    receipt.payload.role === "plan_review_fixes"
      ? task.reviewCommit
      : receipt.payload.role === "implement_review_fixes" ||
          receipt.payload.role === "implement" ||
          (receipt.payload.role === "review_code" ||
            receipt.payload.role === "review_ux")
      ? receipt.payload.evidence.commit
      : undefined,
  );
  const applying: Task = {
    ...task,
    status: "applying",
    next: receipt.next,
    evidence: receipt,
    message: result.message,
  };
  await saveTask(context, applying);
  const handles = await saveState(
    context,
    `agent_${task.role}_${task.id}`,
    receipt.next,
    eventName,
  );
  handles.dataHandles.push(
    ...(await completeTask(context, applying)).dataHandles,
  );
  return handles;
}

export async function prepareTask(
  context: IssueAgentContext,
  args: z.infer<typeof Input>,
  state: z.infer<typeof StateSchema>,
  dependencies: IssueAgentDependencies,
  allowCompleted = false,
): Promise<Task> {
  if (
    state.closeout?.authorizedRequest &&
    (state.implementation?.worktree ?? state.workerWorktree) ===
      state.closeout.target.worktree
  ) {
    throw new Error(
      "Closeout may have removed the worker worktree; deliberately select a new valid worktree with implement before launching more work",
    );
  }
  if (!args.control) {
    throw new Error(
      "Lifecycle workers require control identity (session and paneId) before launching",
    );
  }
  if (args.control.session !== args.session) {
    throw new Error(
      "Control pane belongs to a different session than the worker",
    );
  }
  const fresh = await dependencies.readAgenda(
    state.issue.workspace,
    state.issue.itemId,
    allowCompleted,
  );
  if (fresh.digest !== state.issue.digest) {
    throw new Error(
      "Refresh changed Agenda requirements before dispatching an agent",
    );
  }
  const phases = {
    triage: ["triaging", "classified"],
    plan: [
      "classified",
      "planning",
      "approved",
      "implementing",
      "ready",
    ],
    review_plan: ["planning"],
    implement: ["implementing"],
    review_code: ["implementing", "ready"],
    review_ux: ["implementing", "ready"],
    plan_review_fixes: ["implementing"],
    implement_review_fixes: ["implementing"],
  };
  if (!phases[args.role].includes(state.phase)) {
    throw new Error(`Cannot run ${args.role} in ${state.phase}`);
  }
  if (state.attempt) {
    throw new Error(
      "Finish or repair the active merge attempt before dispatching an agent",
    );
  }
  const cwd = await dependencies.realPath(
    state.workerWorktree ?? state.implementation?.worktree ?? context.repoDir,
  );
  let commit: string | undefined;
  if (
    args.role === "implement" || args.role === "review_code" ||
    args.role === "review_ux" || args.role === "plan_review_fixes" ||
    args.role === "implement_review_fixes"
  ) {
    if (
      !state.implementation || !state.plan ||
      state.approval?.planVersion !== state.plan.version
    ) {
      throw new Error(
        "Implementation and code review require the current approved plan and issue worktree",
      );
    }
    const repo = await dependencies.inspect(context, cwd);
    if (
      repo.branch !== state.implementation.branch ||
      (!["implement", "implement_review_fixes"].includes(args.role) &&
        !repo.clean)
    ) {
      throw new Error(
        "Commit implementation changes before requesting code review",
      );
    }
    if (args.role !== "implement") commit = repo.head;
    if (
      args.role === "plan_review_fixes" ||
      args.role === "implement_review_fixes"
    ) {
      const fix = activeReviewFix(state);
      if (
        !fix ||
        (args.role === "plan_review_fixes"
          ? !!fix.plan
          : !fix.plan || !fix.approval)
      ) {
        throw new Error(
          "Review repairs require current selection and explicit repair-plan approval before implementation",
        );
      }
      if (args.role === "plan_review_fixes" && repo.head !== fix.commit) {
        throw new Error("Review repair plan commit is stale");
      }
    }
    if (args.role === "implement" && activeReviewFix(state)) {
      throw new Error(
        "Use the approved review repair worker for selected findings",
      );
    }
    if (
      args.role === "review_ux" &&
      (!state.codeReview || state.codeReview.commit !== commit ||
        state.implementation.head !== commit ||
        state.codeReview.planVersion !== state.plan.version)
    ) {
      throw new Error(
        "UX dispatch requires current technical review of the same clean implementation commit",
      );
    }
  }
  // The dispatched commit belongs to task evidence, not prior code review.
  const promptState = structuredClone(state);
  if (commit && promptState.implementation) {
    promptState.implementation.head = commit;
  }
  // Issue worktrees may predate the shared standards; read the control checkout's.
  const references = join(
    state.implementation ? await dependencies.realPath(context.repoDir) : cwd,
    ".agents",
    "skills",
    "issue-lifecycle",
    "references",
  );
  await dependencies.submissionPreflight(context);
  const { role: _role, request: _request, ...options } = args;
  const config = AgentConfigSchema.parse({
    ...options,
    cwd,
    stackWorkers: true,
  });
  const identity = {
    id: dependencies.id(),
    behaviorName: `issue-agent-${dependencies.id()}`,
    role: args.role,
    ...(args.request ? { request: args.request } : {}),
    config,
    before: state,
    ...(commit ? { reviewCommit: commit } : {}),
  };
  await dependencies.captureCloseoutOrigin?.(context, args.control);
  const controlDir = await dependencies.realPath(context.repoDir);
  const submission = EvidenceTargetSchema.parse({
    protocol: 1,
    behaviorName: `issue-evidence-${identity.id}`,
    controlDir,
    assignmentDigest: evidenceDigest(
      assignmentCore(context, identity, controlDir),
    ),
  });
  const prompt = taskPrompt(
    args.role,
    promptState,
    cwd,
    references,
    submissionInstructions(identity, submission),
    commit,
    args.request,
  );
  const task: Task = {
    ...identity,
    submission,
    prompt,
    promptDigest: await promptDigest(prompt),
    createdAt: dependencies.now(),
    status: "running",
  };
  return task;
}

export async function runTask(
  context: IssueAgentContext,
  task: Task,
  dependencies: IssueAgentDependencies,
  recover = false,
) {
  if (task.discard) return finishDiscard(context, task);
  await saveTask(context, task);
  try {
    if (task.submission) {
      await dependencies.submissionPreflight(context);
      await evidenceOperation(context, task, "open", {
        assignment: taskAssignment(context, task),
      });
    }
    if (!task.config.control) {
      const child = recover ? await childState(context, task) : undefined;
      if (!child || child.phase === "preflight") {
        throw new Error(
          "Legacy unlaunched lifecycle worker requires a restart with control identity",
        );
      }
    }
    // A controller can lose its response after the child saved completion. Read
    // durable state first; applyResult still validates the result and receipt.
    if (recover && task.submission) {
      const receipt = await context.dataRepository.getContent(
        "@yinz/issue-evidence",
        task.submission.behaviorName,
        "submission",
      );
      const accepted = receipt && SubmissionSchema.parse(
            JSON.parse(new TextDecoder().decode(receipt)),
          ).status === "accepted";
      const bytes = await context.dataRepository.getContent(
        "@yinz/agent",
        task.behaviorName,
        "state",
      );
      if (accepted && !bytes) {
        throw new Error(
          "Accepted evidence has no durable original worker state; inspect the original child and submission before recovery, do not launch or resend",
        );
      }
      if (accepted && bytes) {
        const child = await childState(context, task);
        if (child.phase === "completed") {
          return await applyResult(context, task, dependencies);
        }
      }
    }
    const continuing = recover && task.continuation;
    const outcome = await context.runModel({
      modelType: "@yinz/agent",
      name: task.behaviorName,
      method: continuing ? "continue" : recover ? "recover" : "run",
      arguments: continuing
        ? { id: continuing.id, prompt: continuing.prompt }
        : { ...task.config, prompt: task.prompt },
    });
    if (!outcome.ok) throw new Error(outcome.error.message);
    return await applyResult(context, task, dependencies);
  } catch (error) {
    const saved = AgentTaskSchema.parse(
      await context.readResource("agent-task"),
    );
    await saveTask(context, {
      ...saved,
      lastError: error instanceof Error ? error.message : String(error),
    });
    throw error;
  }
}

async function finishRecovery(
  context: IssueAgentContext,
  recovery: Recovery,
  dependencies: IssueAgentDependencies,
) {
  if (recovery.status !== "pending") return { dataHandles: [] };
  const current = await loadState(context);
  if (
    JSON.stringify(current) !== JSON.stringify(recovery.before) &&
    JSON.stringify(current) !== JSON.stringify(recovery.next)
  ) {
    throw new Error(
      "Lifecycle changed during rollback; inspect the saved recovery before continuing",
    );
  }
  const rawTask = await context.readResource("agent-task");
  const task = rawTask == null ? undefined : AgentTaskSchema.parse(rawTask);
  if (task?.id !== recovery.task?.id && task?.id !== recovery.replacement?.id) {
    throw new Error(
      "Pane assignment changed during rollback; refusing to supersede a different task",
    );
  }
  const handles = [];
  if (recovery.task) {
    await evidenceOperation(context, recovery.task, "supersede", {
      taskId: recovery.task.id,
      assignmentDigest: recovery.task.submission?.assignmentDigest,
      operationId: recovery.id,
    });
    // Archive under a stable name before replacing the current assignment. Retrying
    // a partial write does not depend on the old pane, submission or child behavior.
    handles.push(
      await context.writeResource(
        "agent-task",
        `agent-task-${recovery.task.id}`,
        {
          ...recovery.task,
          status: "superseded",
          cleanup: undefined,
          supersededBy: recovery.id,
        },
      ),
    );
  }
  await dependencies.reopenAgenda(recovery.next.issue);
  handles.push(
    ...(await saveState(
      context,
      `rollback_${recovery.step}`,
      recovery.next,
      `event-rollback-${recovery.id}`,
    )).dataHandles,
  );
  if (recovery.replacement) {
    // A pending recovery prevents all dispatch until this exact replacement is saved.
    handles.push(await saveTask(context, recovery.replacement));
  } else if (recovery.task) {
    handles.push(
      await saveTask(context, {
        ...recovery.task,
        status: "superseded",
        cleanup: undefined,
        supersededBy: recovery.id,
      }),
    );
  }
  const completed: Recovery = { ...recovery, status: "completed" };
  handles.push(
    await context.writeResource(
      "recovery",
      `recovery-${recovery.id}`,
      completed,
    ),
  );
  handles.push(await context.writeResource("recovery", "recovery", completed));
  return { dataHandles: handles };
}

async function beginRecovery(
  context: IssueAgentContext,
  input: z.infer<typeof RollbackInputSchema>,
  dependencies: IssueAgentDependencies,
  restart?: z.infer<typeof RestartInput>,
) {
  const rawRecovery = await context.readResource("recovery");
  const previous = rawRecovery == null
    ? undefined
    : RecoverySchema.parse(rawRecovery);
  const before = await loadState(context);
  const rawTask = await context.readResource("agent-task");
  const task = rawTask == null ? undefined : AgentTaskSchema.parse(rawTask);
  if (restart && restart.taskId !== task?.id) {
    throw new Error(
      "Restart taskId does not match the current assignment; inspect status before restarting",
    );
  }
  let source = before;
  // Receipt lookup takes the shared Git lock, so an in-flight merge settles before
  // rollback records a new attempt. Never erase a merge that landed before a crash.
  if (source.attempt || source.receipt) {
    const raw = await dependencies.gitOperation(context, {
      action: "receipt",
      attemptId: source.attempt?.id ?? source.receipt!.attemptId,
    });
    const { status } = z.object({ status: z.string() }).parse(raw);
    if (status === "merged") {
      const receipt = normalizeReceipt(raw, source);
      source = source.attempt
        ? transition(source, "merged", { receipt })
        : source;
    } else if (source.receipt) {
      throw new Error(
        "Recorded merge receipt is unavailable; inspect Git before rolling back",
      );
    } else if (!["missing", "failed", "verified"].includes(status)) {
      throw new Error(`Unknown merge receipt status: ${status}`);
    }
  }
  const fresh = await dependencies.readAgenda(
    before.issue.workspace,
    before.issue.itemId,
    true,
  );
  const id = dependencies.id();
  const repairRestart = restart &&
    ["plan_review_fixes", "implement_review_fixes"].includes(restart.role);
  const selected = repairRestart
    ? activeReviewFix(source) ??
      (task?.role === restart.role ? activeReviewFix(task.before) : undefined)
    : undefined;
  if (repairRestart && !selected) {
    throw new Error("No current review repair assignment to restart");
  }
  let next = repairRestart
    ? structuredClone(source)
    : rollbackState(source, input.step, id);
  if (repairRestart && selected) {
    // Import clears the reviews and advances the head before pane cleanup. An
    // explicit restart restores that scope while retaining the imported outcome
    // in history; it does not rewind the checkout or require another approval.
    next.codeReview = structuredClone(selected.codeReview);
    next.uxReview = structuredClone(selected.uxReview);
    next.implementation!.head = selected.commit;
    next.rollbackId = id;
    const replacement = {
      ...structuredClone(selected),
      id,
      reviewFingerprint: codeReviewFingerprint(next),
    };
    if (restart.role === "plan_review_fixes") {
      delete replacement.plan;
      delete replacement.approval;
    }
    next.reviewDecisions = [...(next.reviewDecisions ?? []), replacement];
    delete next.verification;
    delete next.mergeAuthorization;
  }
  if (fresh.digest !== before.issue.digest) {
    if (input.step !== "triage") {
      throw new Error(
        "Agenda requirements changed; roll back to triage to replace stale prerequisite evidence",
      );
    }
    next = refreshIssue(next, fresh);
  }
  // Preserve instructions from superseded work even if its result was never imported.
  const reservations = await savedRevisions(context);
  for (
    const previous of [
      task,
      reservations.automaticRevision?.assignment,
      reservations.revisionAuthorization?.assignment,
    ]
  ) {
    if (
      previous?.request &&
      previous.before.issue.digest === before.issue.digest
    ) {
      recordFeedback(next, previous.request, previous.before.planVersion);
    }
  }
  recordFeedback(next, input.request, next.planVersion);
  let replacement: Task | undefined;
  if (restart) {
    const { taskId: _taskId, ...options } = restart;
    replacement = await prepareTask(
      context,
      Input.parse(options),
      next,
      dependencies,
      true,
    );
  }
  const recovery = RecoverySchema.parse({
    id,
    ...input,
    createdAt: dependencies.now(),
    status: "pending",
    before,
    next,
    task,
    replacement,
    restart,
    ...(source !== before ? { reconciled: source } : {}),
  });
  // Save intent before reopening Agenda or superseding assignments. A failed write
  // is resumed through resume-rollback, never by fabricating successful evidence.
  await invalidateHandoff(context);
  if (previous?.status === "pending") {
    // A new explicit rollback can replace even an interrupted recovery (for
    // example, requirements changed while Agenda was unavailable).
    await context.writeResource("recovery", `recovery-${previous.id}`, {
      ...previous,
      status: "superseded",
      supersededBy: id,
    });
  }
  await context.writeResource("recovery", "recovery", recovery);
  return finishRecovery(context, recovery, dependencies);
}

/** Durable discard intent must be fenced before any provider work or import. */
async function finishDiscard(context: IssueAgentContext, task: Task) {
  if (["discarded", "superseded"].includes(task.status)) {
    return { dataHandles: [] };
  }
  if (!task.discard) throw new Error("No pending discard intent");
  const current = await loadState(context);
  if (
    task.status === "completed" || canonical(current) !== canonical(task.before)
  ) {
    throw new Error(
      "Pane evidence may already be applied; cannot finish pending discard",
    );
  }
  await evidenceOperation(context, task, "supersede", {
    taskId: task.id,
    assignmentDigest: task.submission?.assignmentDigest,
    operationId: task.id,
  });
  return {
    dataHandles: [await saveTask(context, { ...task, status: "discarded" })],
  };
}

export async function resumeTask(
  context: IssueAgentContext,
  dependencies: IssueAgentDependencies,
) {
  const task = AgentTaskSchema.parse(await context.readResource("agent-task"));
  if (task.discard) return finishDiscard(context, task);
  if (
    ["completed", "discarded", "superseded", "applying"].includes(task.status)
  ) {
    return applyResult(context, task, dependencies);
  }
  return runTask(context, task, dependencies, true);
}

export function createAgentMethods(
  dependencies: IssueAgentDependencies = defaultDependencies,
) {
  return {
    continue_agent: {
      kind: "action",
      description:
        "Ask the original retained worker to submit its existing findings through Swamp, without another investigation",
      arguments: z.object({ taskId: z.uuid(), request: UserRequestSchema })
        .strip(),
      execute: (
        args: { taskId: string; request: string },
        context: IssueAgentContext,
      ) =>
        dependencies.withIssue(context, async () => {
          await assertNoPendingRecovery(context);
          let task = AgentTaskSchema.parse(
            await context.readResource("agent-task"),
          );
          if (
            task.id !== args.taskId ||
            ["discarded", "superseded"].includes(task.status)
          ) {
            throw new Error(
              "Continuation must target the current original assignment",
            );
          }
          if (task.discard) return finishDiscard(context, task);
          if (task.evidence) {
            return task.status === "completed"
              ? applyResult(context, task, dependencies)
              : runTask(context, task, dependencies, true);
          }
          const current = await loadState(context);
          if (canonical(current) !== canonical(task.before)) {
            throw new Error(
              "Lifecycle changed; cannot continue a stale submission assignment",
            );
          }
          if (
            task.status === "applying" && task.next &&
            canonical(task.next) !== canonical(task.before)
          ) {
            throw new Error(
              "Resume the existing typed projection before continuing",
            );
          }
          await dependencies.submissionPreflight(context);
          if (!task.submission) {
            const controlDir = await dependencies.realPath(context.repoDir);
            task = {
              ...task,
              status: "running",
              cleanup: undefined,
              submission: {
                protocol: 1,
                behaviorName: `issue-evidence-${task.id}`,
                controlDir,
                assignmentDigest: evidenceDigest(
                  assignmentCore(context, task, controlDir),
                ),
              },
            };
          }
          await saveTask(context, task);
          await evidenceOperation(context, task, "open", {
            assignment: taskAssignment(context, task),
          });
          // A prior continuation may already have committed evidence despite a
          // lost CLI response. Never send another prompt before inspecting it.
          const bytes = await context.dataRepository.getContent(
            "@yinz/issue-evidence",
            task.submission!.behaviorName,
            "submission",
          );
          if (
            bytes &&
            SubmissionSchema.parse(JSON.parse(new TextDecoder().decode(bytes)))
                .status === "accepted"
          ) return runTask(context, task, dependencies, true);
          if (task.continuation?.request !== args.request) {
            assertContinuationEligible(await childState(context, task));
          }
          const continuation = task.continuation?.request === args.request
            ? task.continuation
            : {
              id: dependencies.id(),
              request: args.request,
              prompt: [
                "Finish submission of the findings already established in this conversation. Preserve this task and its investigation; do not repeat the review or edit source or Git refs.",
                args.request,
                submissionInstructions(task, task.submission!),
              ].join("\n\n"),
            };
          task = { ...task, continuation };
          await saveTask(context, task);
          const outcome = await context.runModel({
            definition: task.behaviorName,
            method: "continue",
            arguments: { id: continuation.id, prompt: continuation.prompt },
          });
          if (!outcome.ok) throw new Error(outcome.error.message);
          return applyResult(context, task, dependencies);
        }),
    },
    rollback: {
      kind: "action",
      description:
        "Explicitly roll back to any lifecycle step, superseding old work while preserving history and Git",
      arguments: RollbackInputSchema.strip(),
      execute: (
        input: z.infer<typeof RollbackInputSchema>,
        context: IssueAgentContext,
      ) =>
        dependencies.withIssue(
          context,
          () =>
            beginRecovery(
              context,
              RollbackInputSchema.parse(input),
              dependencies,
            ),
        ),
    },
    resume_rollback: {
      kind: "action",
      description:
        "Finish a partially recorded rollback and resume its exact replacement, if requested",
      arguments: z.object({}).strip(),
      execute: (_args: Record<string, never>, context: IssueAgentContext) =>
        dependencies.withIssue(context, async () => {
          const recovery = RecoverySchema.parse(
            await context.readResource("recovery"),
          );
          const result = await finishRecovery(context, recovery, dependencies);
          if (recovery.replacement) {
            const task = AgentTaskSchema.parse(
              await context.readResource("agent-task"),
            );
            if (task.id === recovery.replacement.id) {
              return resumeTask(context, dependencies);
            }
          }
          return result;
        }),
    },
    restart_agent: {
      kind: "action",
      description:
        "Explicitly roll back to a worker step and start a fresh assignment, independent of the old outcome",
      arguments: RestartInput.strip(),
      execute: (
        input: z.infer<typeof RestartInput>,
        context: IssueAgentContext,
      ) =>
        dependencies.withIssue(context, async () => {
          const args = RestartInput.parse(input);
          const raw = await context.readResource("recovery");
          const previous = raw == null ? undefined : RecoverySchema.parse(raw);
          if (
            previous?.restart?.taskId === args.taskId &&
            JSON.stringify(previous.restart) === JSON.stringify(args)
          ) {
            await finishRecovery(context, previous, dependencies);
            const task = AgentTaskSchema.parse(
              await context.readResource("agent-task"),
            );
            if (task.id !== previous.replacement?.id) {
              throw new Error(
                "Restart has already been superseded; inspect the current assignment",
              );
            }
          } else {
            await beginRecovery(
              context,
              {
                step: args.role === "plan_review_fixes" ||
                    args.role === "implement_review_fixes"
                  ? "review_code"
                  : args.role,
                request: args.request,
              },
              dependencies,
              args,
            );
          }
          return resumeTask(context, dependencies);
        }),
    },
    run_agent: {
      kind: "action",
      description:
        "Run a lifecycle phase in a visible Yinz pane and import its accepted Swamp evidence after provider completion",
      // Swamp merges behavior globals into method arguments before parsing.
      arguments: Input.strip(),
      execute: (input: z.infer<typeof Input>, context: IssueAgentContext) =>
        dependencies.withIssue(context, async () => {
          const args = Input.parse(input);
          const task = await prepareTask(
            context,
            args,
            await assertNoPendingAgent(context),
            dependencies,
          );
          return runTask(context, task, dependencies);
        }),
    },
    resume_agent: {
      kind: "action",
      description:
        "Observe the original submission or reconcile applied evidence; use explicit rollback/restart for a replacement",
      arguments: z.object({}),
      execute: (_args: Record<string, never>, context: IssueAgentContext) =>
        dependencies.withIssue(context, async () => {
          await assertNoPendingRecovery(context);
          return resumeTask(context, dependencies);
        }),
    },
    discard_agent: {
      kind: "update",
      description:
        "Explicitly discard unapplied terminal work or a resource-free launch rejection, fencing further submissions with the operator's reason and confirmation",
      arguments: AgentTaskSchema.shape.discard.unwrap().strip(),
      execute: (
        input: z.infer<ReturnType<typeof AgentTaskSchema.shape.discard.unwrap>>,
        context: IssueAgentContext,
      ) =>
        dependencies.withIssue(context, async () => {
          await assertNoPendingRecovery(context);
          const discard = AgentTaskSchema.shape.discard.unwrap().parse(input);
          const task = AgentTaskSchema.parse(
            await context.readResource("agent-task"),
          );
          if (task.status === "discarded") return { dataHandles: [] };
          if (task.discard) return finishDiscard(context, task);
          if (task.status === "completed") {
            throw new Error(
              "Pane evidence is already applied; revise the lifecycle instead",
            );
          }
          let child = await childState(context, task);
          let confirmedFailure = false;
          if (isTerminalFailure(child)) {
            const outcome = await context.runModel({
              definition: task.behaviorName,
              method: "confirm_failure",
            });
            if (!outcome.ok) throw new Error(outcome.error.message);
            child = await childState(context, task);
            confirmedFailure = isTerminalFailure(child);
          }
          if (
            !confirmedFailure && !isRejectedLaunch(child) &&
            (!["completed", "completed_unusable"].includes(child.phase) ||
              child.outcome !== "completed" ||
              child.backgroundTasks.length || !child.paneId ||
              !child.conversationId || !child.submissionId)
          ) {
            throw new Error(
              "Cannot discard an uncertain or active pane task; retrieve its original submission for structured not_sent evidence or an acknowledged provider failure, then inspect the pane; legacy text is insufficient",
            );
          }
          const current = await loadState(context);
          if (JSON.stringify(current) !== JSON.stringify(task.before)) {
            throw new Error(
              "Pane evidence may already be applied; resume the task before discarding it",
            );
          }
          const pending = { ...task, discard };
          await saveTask(context, pending);
          return finishDiscard(context, pending);
        }),
    },
  };
}

export const agentMethods = createAgentMethods();
