/** Assign lifecycle work to visible Yinz panes and import correlated structured results. */
import { z } from "npm:zod@4.6.7";
import {
  ActionSchemas,
  readAgenda,
  StateSchema,
  transition,
} from "./lifecycle.ts";
import {
  type Context,
  inspect,
  loadState,
  saveState,
  withIssue,
} from "./runtime.ts";
import {
  AgentConfigSchema,
  AgentResultSchema,
  AgentStateSchema,
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
const Input = z.object({
  role: Role,
  session: z.string().trim().min(1),
  kind: z.enum(["claude", "codex", "muse", "pi", "agy"]),
  model: z.string().trim().min(1).optional(),
  effort: z.string().trim().min(1).optional(),
  permissionMode: z.string().trim().min(1).optional(),
  startupTimeoutSeconds: z.number().int().positive().max(3600).default(120),
  timeoutSeconds: z.number().int().positive().max(86400).default(600),
}).strict();

export const AgentTaskSchema = z.object({
  id: z.uuid(),
  behaviorName: z.string(),
  role: Role,
  config: AgentConfigSchema,
  prompt: z.string().min(1),
  promptDigest: z.string().regex(/^[0-9a-f]{64}$/),
  before: StateSchema,
  next: StateSchema.optional(),
  createdAt: z.iso.datetime(),
  reviewCommit: z.string().regex(/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/).optional(),
  status: z.enum(["running", "applying", "completed", "discarded"]),
  discard: z.object({
    request: z.string().trim().min(1),
    confirmedStopped: z.literal(true),
  }).strict().optional(),
  message: z.string().optional(),
  lastError: z.string().optional(),
}).strict();
type Task = z.infer<typeof AgentTaskSchema>;
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
  realPath: (path: string) => Promise<string>;
  id: () => string;
  now: () => string;
}
const defaultDependencies: IssueAgentDependencies = {
  withIssue,
  readAgenda,
  inspect,
  realPath: (path) => Deno.realPath(path),
  id: () => crypto.randomUUID(),
  now: () => new Date().toISOString(),
};

const ImplementationResult = z.object({
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

export async function assertNoPendingAgent(context: Context) {
  const raw = await context.readResource("agent-task");
  if (raw !== null && raw !== undefined) {
    const task = AgentTaskSchema.parse(raw);
    if (!["completed", "discarded"].includes(task.status)) {
      throw new Error(
        `Pane task ${task.behaviorName} is unresolved; use resume-agent and inspect its pane before changing lifecycle state`,
      );
    }
  }
}

function saveTask(context: Context, task: Task) {
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
  commit?: string,
) {
  const instructions = {
    triage:
      "Investigate the issue against the source. Reproduce a bug when practical. Return its classification and an evidence-based summary, including acceptance criteria and uncertainties.",
    plan:
      "Develop or revise a concrete implementation plan from the issue and investigation. Include affected files, risks in step descriptions, and focused development checks. Full cargo x ci check is mandatory later in the merge gate unless the user explicitly waives it.",
    review_plan:
      "Adversarially review the current plan against the actual code. Return planVersion and every actionable finding. Do not approve the plan or invent resolutions. An empty findings array is appropriate only after a real review found none.",
    implement:
      "Implement the approved plan in this existing issue worktree. Run focused checks following AGENTS.md, investigate failures, and commit the intended changes on the existing feature branch. Return a summary and the full resulting commit hash. Do not rebase, merge, push, or run the full CI gate here.",
    review_code:
      "Review the committed diff against local main and the approved plan. Cover each plan step exactly once, justify deviations, and record code findings and actual preliminary checks. Do not invent passing checks. Return the supplied full commit hash and planVersion.",
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
    "Return exactly one JSON object matching this schema, without Markdown fences or extra prose:",
    JSON.stringify(z.toJSONSchema(RoleSchemas[role])),
    "Current lifecycle snapshot (issue requirements and recorded evidence):",
    JSON.stringify(state),
    ...(commit ? [`The commit being reviewed is ${commit}.`] : []),
  ].join("\n\n");
}

async function childState(context: IssueAgentContext, task: Task) {
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

async function applyResult(
  context: IssueAgentContext,
  task: Task,
  dependencies: IssueAgentDependencies,
) {
  const current = await loadState(context);
  if (["completed", "discarded"].includes(task.status)) {
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
    return {
      dataHandles: [await saveTask(context, { ...task, status: "completed" })],
    };
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
    next = transition(current, task.role, evidence);
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
    await saveTask(context, { ...applying, status: "completed" }),
  );
  return handles;
}

export function createAgentMethods(
  dependencies: IssueAgentDependencies = defaultDependencies,
) {
  return {
    run_agent: {
      kind: "action",
      description:
        "Run a lifecycle phase in a visible Yinz agent pane and import its completed structured result",
      // Swamp merges behavior globals into method arguments before parsing.
      arguments: Input.strip(),
      execute: (input: z.infer<typeof Input>, context: IssueAgentContext) =>
        dependencies.withIssue(context, async () => {
          const args = Input.parse(input);
          await assertNoPendingAgent(context);
          const state = await loadState(context);
          const fresh = await dependencies.readAgenda(
            state.issue.workspace,
            state.issue.itemId,
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
            state.implementation?.worktree ?? context.repoDir,
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
          const prompt = taskPrompt(args.role, promptState, cwd, commit);
          const { role: _role, ...options } = args;
          const config = AgentConfigSchema.parse({ ...options, cwd });
          const task: Task = {
            id: dependencies.id(),
            behaviorName: `issue-agent-${dependencies.id()}`,
            role: args.role,
            config,
            prompt,
            promptDigest: await promptDigest(prompt),
            before: state,
            createdAt: dependencies.now(),
            status: "running",
            ...(commit ? { reviewCommit: commit } : {}),
          };
          await saveTask(context, task);
          try {
            const outcome = await context.runModel({
              modelType: "@yinz/agent",
              name: task.behaviorName,
              method: "run",
              arguments: { ...config, cwd, prompt },
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
        }),
    },
    resume_agent: {
      kind: "action",
      description:
        "Observe the original submission or reconcile applied evidence; rejected launches require discard-agent before a new task",
      arguments: z.object({}),
      execute: (_args: Record<string, never>, context: IssueAgentContext) =>
        dependencies.withIssue(context, async () => {
          const task = AgentTaskSchema.parse(
            await context.readResource("agent-task"),
          );
          if (["completed", "discarded", "applying"].includes(task.status)) {
            return applyResult(context, task, dependencies);
          }
          const outcome = await context.runModel({
            modelType: "@yinz/agent",
            name: task.behaviorName,
            method: "recover",
            arguments: { ...task.config, prompt: task.prompt },
          });
          if (!outcome.ok) throw new Error(outcome.error.message);
          return applyResult(context, task, dependencies);
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
