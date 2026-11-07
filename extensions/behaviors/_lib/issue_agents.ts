/** Assign lifecycle work to visible Yinz panes and import correlated structured results. */
import { join } from "node:path";
import { z } from "npm:zod@4.6.7";
import {
  ActionSchemas,
  initialPlanning,
  previousPlanFindings,
  readAgenda,
  recordFeedback,
  refreshIssue,
  reopenAgenda,
  RevisionPlanSchema,
  RollbackInputSchema,
  rollbackState,
  StateSchema,
  transition,
  UserRequestSchema,
  validatePlanReview,
} from "./lifecycle.ts";
import {
  type Context,
  gitOperation,
  inspect,
  invalidateHandoff,
  loadState,
  normalizeReceipt,
  saveState,
  withIssue,
} from "./runtime.ts";
import {
  AgentConfigSchema,
  AgentResultSchema,
  AgentStateSchema,
  ControlIdentitySchema,
  isRejectedLaunch,
  isTerminalFailure,
  promptDigest,
} from "./yinz_agent.ts";

const Role = z.enum([
  "triage",
  "plan",
  "review_plan",
  "implement",
  "review_code",
]);
export const AgentInputSchema = z.object({
  role: Role,
  request: UserRequestSchema.optional(),
  session: z.string().trim().min(1),
  // Optional while decoding saved settings and tasks from older lifecycles.
  control: ControlIdentitySchema.optional(),
  kind: z.enum(["claude", "codex", "muse", "pi", "agy"]),
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
  contractVersion: z.literal(2).optional(),
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
  withIssue: typeof withIssue;
  readAgenda: typeof readAgenda;
  inspect: typeof inspect;
  gitOperation: typeof gitOperation;
  reopenAgenda: typeof reopenAgenda;
  realPath: (path: string) => Promise<string>;
  id: () => string;
  now: () => string;
}
export const defaultDependencies: IssueAgentDependencies = {
  withIssue,
  readAgenda,
  inspect,
  gitOperation,
  reopenAgenda,
  realPath: (path) => Deno.realPath(path),
  id: () => crypto.randomUUID(),
  now: () => new Date().toISOString(),
};

export const ImplementationResult = z.object({
  summary: z.string().trim().min(1),
  commit: z.string().regex(/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/),
}).strict();
const RoleSchemas = {
  triage: ActionSchemas.triage,
  plan: ActionSchemas.plan,
  review_plan: ActionSchemas.review_plan,
  implement: ImplementationResult,
  review_code: ActionSchemas.review_code,
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

function taskPrompt(
  role: z.infer<typeof Role>,
  state: z.infer<typeof StateSchema>,
  cwd: string,
  references: string,
  commit?: string,
  request?: string,
) {
  const instructions = {
    triage:
      "Investigate the issue against the source. Reproduce a bug when practical. Return its classification and an evidence-based summary, including acceptance criteria and uncertainties.",
    plan:
      "Develop or revise a concrete implementation plan from the issue and investigation. Address existing review findings. Include affected files, risks in step descriptions, assumptions, and a testingStrategy explaining the test selection. Return checks: focused development checks selected per AGENTS.md (never unfiltered full suites), each with a short id, an argv array run verbatim without a shell in the issue worktree, and the plan step IDs it covers. The lifecycle runs every check at the reviewed commit and requires them to pass before merge authorization. Full cargo x ci check is mandatory later in the merge gate unless the user explicitly waives it.",
    review_plan:
      "Adversarially review the current plan against the actual code, including whether its checks are correct argv, focused, safe to run unattended, and cover the changed behavior and affected callers. Return planVersion and every actionable finding. Do not approve the plan or invent resolutions. An empty findings array is appropriate only after a real review found none.",
    implement:
      "Implement the approved plan in this existing issue worktree. Run focused checks following AGENTS.md, investigate failures, and commit the intended changes on the existing feature branch. Return a summary and the full resulting commit hash. Do not rebase, merge, push, or run the full CI gate here.",
    review_code:
      "Review the committed diff against local main and the approved plan. Cover each plan step exactly once, justify deviations, and record code findings and any preliminary checks you actually ran. Preliminary checks are advisory; the lifecycle runs the plan's checks itself. Do not invent passing checks. Return the supplied full commit hash and planVersion.",
  }[role];
  return [
    "You are working on one Yinz issue lifecycle task in a visible Yinz pane.",
    `Work only in ${cwd}. Follow its AGENTS.md and applicable repository instructions.`,
    "Do not invoke the lifecycle launcher, Swamp lifecycle methods, or Agenda mutations; the controlling workflow owns those operations. Do not merge or contact remote Git services.",
    role === "implement"
      ? "Only this task may edit the issue worktree."
      : "This is investigation or review: do not edit source files, Git refs, or configuration.",
    "Finish this task in the foreground. Do not leave background subagents or jobs running when returning your answer.",
    instructions,
    ...(["plan", "review_plan"].includes(role)
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
    ...(request
      ? ["Exact user instructions for this assignment:", request]
      : []),
    "Return exactly one JSON object matching this schema, without Markdown fences or extra prose:",
    JSON.stringify(
      z.toJSONSchema(
        role === "plan" && !initialPlanning(state)
          ? RevisionPlanSchema
          : RoleSchemas[role],
      ),
    ),
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
    result.message !== child.message?.trim() ||
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

async function applyResult(
  context: IssueAgentContext,
  task: Task,
  dependencies: IssueAgentDependencies,
) {
  const current = await loadState(context);
  if (task.status === "completed") return cleanupTask(context, task);
  if (["discarded", "superseded"].includes(task.status)) {
    return { dataHandles: [] };
  }
  const result = await completedResult(context, task);
  // A transition which changed state proves that saveState reached its state
  // write. Reconcile it before reading requirements that may have changed since.
  // Implementation reports leave state unchanged, so retain freshness checks and
  // retry their event write before completing instead of losing the evidence.
  if (
    task.next && JSON.stringify(current) === JSON.stringify(task.next) &&
    JSON.stringify(task.before) !== JSON.stringify(task.next)
  ) {
    return completeTask(context, task);
  }
  const fresh = await dependencies.readAgenda(
    current.issue.workspace,
    current.issue.itemId,
  );
  if (fresh.digest !== current.issue.digest) {
    throw new Error(
      "Agenda requirements changed while the pane task ran; its result is stale",
    );
  }
  if (JSON.stringify(current) !== JSON.stringify(task.before)) {
    throw new Error(
      "Lifecycle changed while the pane task ran; inspect its result before recording new evidence",
    );
  }
  let response: unknown;
  try {
    response = JSON.parse(result.message);
  } catch (cause) {
    throw new Error(
      `Pane task ${task.behaviorName} did not return the required JSON evidence`,
      { cause },
    );
  }
  const evidence = RoleSchemas[task.role].parse(response);
  if (task.contractVersion === 2) {
    if (task.role === "plan" && !initialPlanning(task.before)) {
      const { revision } = RevisionPlanSchema.parse(response);
      const prior = previousPlanFindings(task.before);
      for (
        const finding of prior.filter((f) =>
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
    if (task.role === "review_plan") {
      validatePlanReview(
        current,
        evidence as z.infer<typeof ActionSchemas.review_plan>,
      );
    }
  }
  let next = current;
  if (task.role === "implement" || task.role === "review_code") {
    const commit = z.object({ commit: z.string() }).parse(evidence).commit;
    if (!current.implementation) {
      throw new Error("Pane implementation has no recorded issue worktree");
    }
    const repo = await dependencies.inspect(
      context,
      current.implementation.worktree,
    );
    if (
      !repo.clean || repo.branch !== current.implementation!.branch ||
      repo.head !== commit
    ) {
      throw new Error(
        "Pane result does not match the clean issue branch's current commit",
      );
    }
    if (task.role === "review_code" && commit !== task.reviewCommit) {
      throw new Error(
        "Pane reviewed a different commit than the dispatched task",
      );
    }
  }
  if (task.role !== "implement") {
    const source = structuredClone(current);
    if (task.request) {
      recordFeedback(source, task.request, source.planVersion);
    }
    next = transition(source, task.role, evidence);
  }
  const applying: Task = {
    ...task,
    status: "applying",
    next,
    message: result.message,
  };
  await saveTask(context, applying);
  const handles = await saveState(
    context,
    `agent_${task.role}_${task.id}`,
    next,
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
  if (args.role === "implement" || args.role === "review_code") {
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
      (args.role === "review_code" && !repo.clean)
    ) {
      throw new Error(
        "Commit implementation changes before requesting code review",
      );
    }
    if (args.role === "review_code") commit = repo.head;
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
  const prompt = taskPrompt(
    args.role,
    promptState,
    cwd,
    references,
    commit,
    args.request,
  );
  const { role: _role, request: _request, ...options } = args;
  const config = AgentConfigSchema.parse({
    ...options,
    cwd,
    stackWorkers: true,
  });
  const task: Task = {
    id: dependencies.id(),
    behaviorName: `issue-agent-${dependencies.id()}`,
    role: args.role,
    contractVersion: 2,
    ...(args.request ? { request: args.request } : {}),
    config,
    prompt,
    promptDigest: await promptDigest(prompt),
    before: state,
    createdAt: dependencies.now(),
    status: "running",
    ...(commit ? { reviewCommit: commit } : {}),
  };
  return task;
}

export async function runTask(
  context: IssueAgentContext,
  task: Task,
  dependencies: IssueAgentDependencies,
  recover = false,
) {
  await saveTask(context, task);
  try {
    if (!task.config.control) {
      const child = recover ? await childState(context, task) : undefined;
      if (!child || child.phase === "preflight") {
        throw new Error(
          "Legacy unlaunched lifecycle worker requires a restart with control identity",
        );
      }
    }
    const outcome = await context.runModel({
      modelType: "@yinz/agent",
      name: task.behaviorName,
      method: recover ? "recover" : "run",
      arguments: { ...task.config, prompt: task.prompt },
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
  let next = rollbackState(source, input.step, id);
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

export async function resumeTask(
  context: IssueAgentContext,
  dependencies: IssueAgentDependencies,
) {
  const task = AgentTaskSchema.parse(await context.readResource("agent-task"));
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
              { step: args.role, request: args.request },
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
        "Run a lifecycle phase in a visible Yinz agent pane and import its completed structured result",
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
        "Discard completed unusable evidence, a confirmed stopped terminal or unsent failure, or a launch rejected without resources, with an operator reason and confirmation",
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
          return {
            dataHandles: [
              await saveTask(context, {
                ...task,
                status: "discarded",
                discard,
              }),
            ],
          };
        }),
    },
  };
}

export const agentMethods = createAgentMethods();
