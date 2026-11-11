import { z } from "npm:zod@4.6.7";
import { isAbsolute } from "node:path";
import { AgentKindSchema } from "./worker_selection.ts";

const Text = z.string().trim().min(1);
const Pane = z.string().regex(/^terminal_[0-9]+$/);
export const ControlIdentitySchema = z.object({
  session: Text,
  paneId: Pane,
}).strict();
export const AgentConfigSchema = z.object({
  session: Text,
  control: ControlIdentitySchema.optional(),
  stackWorkers: z.boolean().optional(),
  kind: AgentKindSchema,
  cwd: Text.refine(isAbsolute, "cwd must be an absolute path"),
  model: z.string().trim().optional(),
  effort: z.string().trim().optional(),
  permissionMode: z.string().trim().optional(),
  startupTimeoutSeconds: z.number().int().min(1).max(3600).default(120),
  timeoutSeconds: z.number().int().min(1).max(86400).default(600),
}).strict();
export type AgentConfig = z.infer<typeof AgentConfigSchema>;
// Swamp merges validated global arguments into method inputs at its execution boundary.
export const RunArgsSchema = z.object({
  prompt: z.string().min(1).refine((value) => value.trim().length > 0),
}).strip();
export const EmptySchema = z.object({}).strip();
export const ContinueArgsSchema = z.object({
  id: z.uuid(),
  prompt: z.string().trim().min(1),
}).strip();
/**
 * Yinz reports Claude background work as `background: true`; older builds listed
 * task IDs in `background_tasks`. Fold the flag into the list so every background
 * check, including persisted state, sees either form.
 */
function foldBackground<
  T extends { background: boolean; background_tasks: string[] },
>(value: T): T {
  return value.background && value.background_tasks.length === 0
    ? { ...value, background_tasks: ["background"] }
    : value;
}
const AgentStatusSchema = z.object({
  pane_id: Pane,
  session: Text,
  agent: z.object({
    kind: Text,
    session_id: z.string().nullable().optional(),
    state: z.string(),
  }),
  cwd: z.string().nullable().optional(),
  ready: z.boolean(),
  status_available: z.boolean(),
  prompt_supported: z.boolean(),
  not_ready_reason: z.string().nullable().optional(),
  background: z.boolean().default(false),
  background_tasks: z.array(z.string()).default([]),
}).transform(foldBackground);
export const LaunchReceiptSchema = z.object({
  version: z.literal(2),
  launch_id: Text,
  request_digest: z.string(),
  server_instance: Text,
  outcome: z.enum([
    "pending",
    "rejected",
    "started",
    "failed",
    "cancelled",
    "unknown",
    "conflict",
  ]),
  phase: Text,
  reason: z.string().nullable(),
  process: z.enum(["not_started", "starting", "running", "exited", "unknown"]),
  resources: z.object({ cleanup_pending: z.boolean() }).passthrough(),
  disposition: z.enum(["reserved", "available", "closed", "unknown"]),
}).passthrough();

export const RetainedSubmissionSchema = z.object({
  server_instance: Text,
  session: Text,
  pane_id: Pane,
  kind: Text,
  cwd: Text,
  conversation_id: Text,
  submission_id: Text,
  turn_id: Text.nullable(),
  prompt_id: Text,
  acknowledged: z.literal(true),
  prompt_delivery: z.literal("started"),
  outcome: z.literal("completed"),
  message: z.string().nullable(),
  answered: z.literal(true),
  quiescent: z.literal(true),
}).strict();

export const AgentResponseSchema = z.object({
  outcome: Text,
  launch: LaunchReceiptSchema.optional(),
  retained_submission: RetainedSubmissionSchema.optional(),
  output: z.string().nullable().optional(),
  server_instance: Text.optional(),
  agents: z.array(AgentStatusSchema).default([]),
  spawned: z.object({
    pane_id: Pane,
    tab_id: z.number().int().nullable().optional(),
    cwd: z.string().nullable().optional(),
  }).nullable().optional(),
  // Other launch operations can retain resources even without a spawned pane.
  project: z.unknown().optional(),
  restarted: z.unknown().optional(),
  launch_resources: z.unknown().optional(),
  message: z.string().nullable().optional(),
  submission_id: z.string().nullable().optional(),
  prompt_delivery: z.enum(["not_sent", "started"]).optional(),
  acknowledged: z.boolean().default(false),
  turn_id: z.string().nullable().optional(),
  background: z.boolean().default(false),
  background_tasks: z.array(z.string()).default([]),
}).transform(foldBackground);
type AgentResponse = z.infer<typeof AgentResponseSchema>;

export const AgentStateSchema = z.object({
  config: AgentConfigSchema,
  prompt: z.string().min(1),
  promptDigest: z.string().regex(/^[a-f0-9]{64}$/),
  phase: z.enum([
    "preflight",
    "launching",
    "launch_rejected",
    "spawned",
    "submitting",
    "waiting",
    "background",
    "completed",
    "completed_unusable",
  ]),
  launchVersion: z.literal(2).optional(),
  launchId: Text.optional(),
  launchRequestDigest: z.string().regex(/^[a-f0-9]{64}$/).optional(),
  launchServer: Text.optional(),
  launchReceipt: LaunchReceiptSchema.optional(),
  retainedSubmission: RetainedSubmissionSchema.optional(),
  paneId: Pane.optional(),
  tabId: z.number().int().optional(),
  conversationId: Text.optional(),
  submissionId: Text.optional(),
  outcome: Text.optional(),
  message: z.string().optional(),
  turnId: Text.optional(),
  backgroundTasks: z.array(z.string()).default([]),
  rawResponse: z.string().optional(),
  lastError: Text.optional(),
  cleanedUp: z.boolean().optional(),
  continuation: z.object({
    id: z.uuid(),
    prompt: z.string().min(1),
    previousSubmissionId: Text,
  }).strict().optional(),
  createdAt: z.iso.datetime(),
  updatedAt: z.iso.datetime(),
}).strict();
export type AgentState = z.infer<typeof AgentStateSchema>;

function rejectedWithoutResources(value: AgentResponse): boolean {
  return value.outcome === "rejected" && value.spawned == null &&
    value.agents.length === 0 && value.submission_id == null &&
    !value.acknowledged && value.turn_id == null &&
    value.background_tasks.length === 0 && value.project == null &&
    value.restarted == null && value.launch_resources == null;
}

export function isRejectedLaunch(state: AgentState): boolean {
  if (state.launchId && state.launchReceipt && !state.submissionId) {
    const receipt = state.launchReceipt;
    return receipt.launch_id === state.launchId &&
      receipt.server_instance === state.launchServer &&
      receipt.request_digest === state.launchRequestDigest &&
      ["rejected", "cancelled", "failed"].includes(receipt.outcome) &&
      ["not_started", "exited"].includes(receipt.process) &&
      !receipt.resources.cleanup_pending &&
      receipt.disposition === "closed";
  }
  if (
    state.phase !== "launch_rejected" || state.outcome !== "rejected" ||
    state.paneId !== undefined || state.tabId !== undefined ||
    state.conversationId !== undefined || state.submissionId !== undefined ||
    state.turnId !== undefined || state.backgroundTasks.length ||
    state.rawResponse === undefined
  ) return false;
  try {
    return rejectedWithoutResources(
      AgentResponseSchema.parse(JSON.parse(state.rawResponse)),
    );
  } catch {
    return false;
  }
}
const PreparationFailures = new Set([
  "failed",
  "target_changed",
  "target_closed",
  "status_unavailable",
  "interrupted",
  "timed_out",
]);

/** A correlated provider failure or explicit non-delivery proves terminality. */
export function isTerminalFailure(state: AgentState): boolean {
  if (
    !state.paneId || !state.conversationId || !state.submissionId ||
    !["waiting", "submitting"].includes(state.phase) || !state.rawResponse
  ) return false;
  try {
    const value = AgentResponseSchema.parse(JSON.parse(state.rawResponse));
    const { agent } = selectedAgent(state, value);
    if (value.submission_id !== state.submissionId) return false;
    if (value.prompt_delivery === "not_sent") {
      return PreparationFailures.has(value.outcome) && !value.acknowledged &&
        value.turn_id == null && state.turnId === undefined &&
        !value.background_tasks.length && !agent.background_tasks.length &&
        !state.backgroundTasks.length;
    }
    // A delivery error is also called `failed`; only an acknowledged provider
    // failure can use the existing terminal-turn recovery path.
    return value.outcome === "failed" && value.acknowledged;
  } catch {
    return false;
  }
}

export const AgentResultSchema = z.object({
  session: Text,
  kind: Text,
  cwd: Text,
  paneId: Pane,
  conversationId: Text,
  submissionId: Text,
  promptDigest: Text,
  outcome: z.literal("completed"),
  message: z.string().optional(),
  turnId: Text.optional(),
  completedAt: z.iso.datetime(),
}).strict();
export const AgentObservationSchema = z.object({
  paneId: Pane,
  outcome: Text,
  rawResponse: z.string(),
  observedAt: z.iso.datetime(),
}).strict();
export type ResourceHandle = { name: string };
export interface AgentContext {
  globalArgs: AgentConfig;
  signal?: AbortSignal;
  readResource: (name: string) => Promise<unknown>;
  writeResource: (
    spec: string,
    name: string,
    data: Record<string, unknown>,
  ) => Promise<ResourceHandle>;
}
export type CommandResult = { code: number; stdout: string; stderr: string };
export type YinzRunner = (
  args: string[],
  stdin?: string,
  signal?: AbortSignal,
) => Promise<CommandResult>;
export interface Dependencies {
  run: YinzRunner;
  realPath: (path: string) => Promise<string>;
  id: () => string;
  now: () => string;
}

export async function yinzCommand(
  args: string[],
  stdin?: string,
  signal?: AbortSignal,
): Promise<CommandResult> {
  signal?.throwIfAborted();
  const process = new Deno.Command("yinz", {
    args,
    stdin: stdin === undefined ? "null" : "piped",
    stdout: "piped",
    stderr: "piped",
  }).spawn();
  // Capture rejection immediately while stdin is being written, then always reap the CLI.
  const output = process.output().then(
    (value) => ({ value }),
    (error: unknown) => ({ error }),
  );
  let forceKill: ReturnType<typeof setTimeout> | undefined;
  const stop = () => {
    try {
      process.kill("SIGTERM");
    } catch { /* The CLI may have already exited. */ }
    if (forceKill === undefined) {
      forceKill = setTimeout(() => {
        try {
          process.kill("SIGKILL");
        } catch { /* The CLI may have already exited. */ }
      }, 1000);
    }
  };
  signal?.addEventListener("abort", stop, { once: true });
  try {
    if (signal?.aborted) stop();
    if (stdin !== undefined) {
      const writer = process.stdin.getWriter();
      try {
        await writer.write(new TextEncoder().encode(stdin));
        await writer.close();
      } finally {
        writer.releaseLock();
      }
    }
    const settled = await output;
    if (signal?.aborted) {
      throw new Error(
        "Yinz CLI observation was cancelled; the pane and task may still be running",
      );
    }
    if ("error" in settled) throw settled.error;
    return {
      code: settled.value.code,
      stdout: new TextDecoder().decode(settled.value.stdout),
      stderr: new TextDecoder().decode(settled.value.stderr),
    };
  } catch (error) {
    stop();
    await output;
    throw error;
  } finally {
    signal?.removeEventListener("abort", stop);
    if (forceKill !== undefined) clearTimeout(forceKill);
  }
}
export const defaultDependencies: Dependencies = {
  run: yinzCommand,
  realPath: (path) => Deno.realPath(path),
  id: () => crypto.randomUUID(),
  now: () => new Date().toISOString(),
};

export async function promptDigest(prompt: string): Promise<string> {
  const hash = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(prompt),
  );
  return Array.from(
    new Uint8Array(hash),
    (byte) => byte.toString(16).padStart(2, "0"),
  ).join("");
}

export function hooksReady(output: string): boolean {
  const completeWiring = [
    ...output.matchAll(/\b([0-9]+) of ([0-9]+) events wired/g),
  ].some((match) => Number(match[1]) > 0 && match[1] === match[2]);
  return /^script:\s+.+ \(current\)$/m.test(output) && completeWiring &&
    !/disableAllHooks is on|allowManagedHooksOnly is on|schema_version is missing|\bmissing:/
      .test(output);
}

async function save(
  context: AgentContext,
  state: AgentState,
  dependencies: Dependencies,
): Promise<ResourceHandle> {
  state.updatedAt = dependencies.now();
  return await context.writeResource(
    "state",
    "state",
    AgentStateSchema.parse(state),
  );
}

async function load(
  context: AgentContext,
  dependencies: Dependencies,
): Promise<AgentState> {
  const value = await context.readResource("state");
  if (value == null) throw new Error("No agent task exists; call run first");
  const state = AgentStateSchema.parse(value);
  const config = AgentConfigSchema.parse(context.globalArgs);
  config.cwd = await dependencies.realPath(config.cwd);
  if (JSON.stringify(config) !== JSON.stringify(state.config)) {
    throw new Error(
      "Agent configuration changed; resume with the task's recorded configuration",
    );
  }
  return state;
}

function commandArgs(state: AgentState, operation: string[]): string[] {
  return ["--session", state.config.session, "agent", ...operation, "--json"];
}

async function response(
  context: AgentContext,
  state: AgentState,
  args: string[],
  dependencies: Dependencies,
  stdin?: string,
): Promise<AgentResponse> {
  const result = await dependencies.run(args, stdin, context.signal);
  state.rawResponse = result.stdout || result.stderr;
  let parsed: AgentResponse;
  try {
    parsed = AgentResponseSchema.parse(JSON.parse(result.stdout));
  } catch {
    state.outcome = "transport_error";
    throw new Error(
      `Yinz returned no valid agent response (exit ${result.code}): ${
        result.stderr || result.stdout
      }`,
    );
  }
  state.outcome = parsed.outcome;
  if (parsed.message != null) state.message = parsed.message;
  // Preserve acknowledged turn identity only after matching the original live target.
  if (
    parsed.submission_id === state.submissionId && parsed.turn_id &&
    state.turnId === undefined && parsed.acknowledged &&
    parsed.outcome !== "completed" && parsed.agents.length === 1 &&
    parsed.agents[0].pane_id === state.paneId &&
    parsed.agents[0].session === state.config.session &&
    parsed.agents[0].agent.kind === state.config.kind &&
    parsed.agents[0].agent.session_id === state.conversationId
  ) {
    state.turnId = parsed.turn_id;
  }
  state.backgroundTasks = parsed.background_tasks;
  if (
    state.phase === "launching" && !state.launchId &&
    rejectedWithoutResources(parsed)
  ) {
    // Persist the classification together with its response, before reporting failure.
    state.phase = "launch_rejected";
  }
  await save(context, state, dependencies);
  // Structured failure outcomes remain available even when the CLI exits nonzero.
  if (
    result.code !== 0 &&
    ["ok", "completed", "submitted"].includes(parsed.outcome)
  ) {
    throw new Error(
      `Yinz exited ${result.code} despite outcome ${parsed.outcome}`,
    );
  }
  return parsed;
}

function selectedAgent(state: AgentState, value: AgentResponse) {
  const matches = value.agents.filter((agent) =>
    agent.pane_id === state.paneId
  );
  if (matches.length !== 1) {
    throw new Error("Yinz did not identify exactly the recorded agent pane");
  }
  const agent = matches[0];
  if (
    agent.session !== state.config.session ||
    agent.agent.kind !== state.config.kind
  ) {
    throw new Error("Agent pane has a different session or provider");
  }
  const conversation = agent.agent.session_id || undefined;
  if (state.conversationId && conversation !== state.conversationId) {
    throw new Error(
      "Agent conversation changed; do not submit or retrieve against a replacement",
    );
  }
  return { agent, conversation };
}

async function finish(
  context: AgentContext,
  state: AgentState,
  dependencies: Dependencies,
): Promise<ResourceHandle[]> {
  if (
    !state.paneId || !state.conversationId || !state.submissionId
  ) {
    throw new Error(
      "Completion lacks a pane, bound conversation or submission",
    );
  }
  const result = AgentResultSchema.parse({
    session: state.config.session,
    kind: state.config.kind,
    cwd: state.config.cwd,
    paneId: state.paneId,
    conversationId: state.conversationId,
    submissionId: state.submissionId,
    promptDigest: state.promptDigest,
    outcome: "completed",
    message: state.message,
    ...(state.turnId ? { turnId: state.turnId } : {}),
    completedAt: dependencies.now(),
  });
  const resultHandle = await context.writeResource("result", "result", result);
  state.phase = "completed";
  state.outcome = "completed";
  delete state.lastError;
  return [
    resultHandle,
    await save(context, state, dependencies),
  ];
}

async function observeBackground(
  context: AgentContext,
  state: AgentState,
  dependencies: Dependencies,
): Promise<ResourceHandle[]> {
  if (!state.conversationId || !state.paneId) {
    throw new Error(
      "Background observation requires the original conversation",
    );
  }
  const value = await response(
    context,
    state,
    commandArgs(state, [
      "wait",
      state.paneId,
      "--until",
      "idle",
      "--conversation",
      state.conversationId,
      "--timeout",
      `${state.config.timeoutSeconds}s`,
    ]),
    dependencies,
  );
  if (value.outcome !== "ok" && value.outcome !== "completed") {
    throw new Error(`Background work has not completed: ${value.outcome}`);
  }
  const { agent } = selectedAgent(state, value);
  if (
    agent.agent.state.toLowerCase() !== "idle" ||
    agent.background_tasks.length || value.background_tasks.length
  ) {
    throw new Error("Background work has no confirmed final idle answer");
  }
  state.message = value.message?.trim() || undefined;
  return await finish(context, state, dependencies);
}

function validateRetained(
  state: AgentState,
  proof: z.infer<typeof RetainedSubmissionSchema>,
) {
  RetainedSubmissionSchema.parse(proof);
  if (
    state.phase === "background" || state.backgroundTasks.length > 0 ||
    !state.launchServer || proof.server_instance !== state.launchServer ||
    proof.session !== state.config.session ||
    proof.kind !== state.config.kind ||
    proof.cwd !== state.config.cwd || proof.pane_id !== state.paneId ||
    proof.conversation_id !== state.conversationId ||
    proof.submission_id !== state.submissionId ||
    (state.turnId !== undefined && proof.turn_id !== state.turnId)
  ) {
    throw new Error(
      "Retained completion does not match the original launch and submission",
    );
  }
}

async function acceptSubmission(
  context: AgentContext,
  state: AgentState,
  value: AgentResponse,
  dependencies: Dependencies,
): Promise<ResourceHandle[]> {
  if (
    value.submission_id != null && value.submission_id !== state.submissionId
  ) {
    throw new Error("Yinz response does not match the saved submission ID");
  }
  if (value.outcome !== "completed") {
    throw new Error(
      `Agent task has not completed: ${value.outcome}${
        value.message ? ": " + value.message : ""
      }`,
    );
  }
  if (!value.submission_id) {
    throw new Error("Completed response lacks submission identity");
  }
  if (state.turnId && value.turn_id !== state.turnId) {
    throw new Error("Yinz response does not match the saved turn ID");
  }
  if (value.retained_submission) {
    validateRetained(state, value.retained_submission);
    if (
      value.background_tasks.length || value.agents.length ||
      value.turn_id !== value.retained_submission.turn_id ||
      !value.acknowledged || value.prompt_delivery !== "started" ||
      value.server_instance !== value.retained_submission.server_instance
    ) {
      throw new Error("Retained completion has conflicting execution facts");
    }
    state.retainedSubmission = value.retained_submission;
    if (value.turn_id) state.turnId = value.turn_id;
    state.message = value.retained_submission.message?.trim() || undefined;
    await save(context, state, dependencies);
    return await finish(context, state, dependencies);
  }
  const { agent, conversation } = selectedAgent(state, value);
  if (value.turn_id) state.turnId = value.turn_id;
  if (!conversation) {
    throw new Error("Completed task has no bound provider conversation");
  }
  state.conversationId = conversation;
  if (value.background_tasks.length || agent.background_tasks.length) {
    state.phase = "background";
    state.backgroundTasks = [
      ...new Set([...value.background_tasks, ...agent.background_tasks]),
    ];
    await save(context, state, dependencies);
    return await observeBackground(context, state, dependencies);
  }
  state.message = value.message?.trim() || undefined;
  return await finish(context, state, dependencies);
}

async function submit(
  context: AgentContext,
  state: AgentState,
  dependencies: Dependencies,
): Promise<ResourceHandle[]> {
  if (!state.paneId) throw new Error("No recorded pane exists");
  const value = await response(
    context,
    state,
    commandArgs(state, [
      "wait",
      state.paneId,
      "--until",
      "idle",
      "--timeout",
      `${state.config.startupTimeoutSeconds}s`,
      ...(state.conversationId ? ["--conversation", state.conversationId] : []),
    ]),
    dependencies,
  );
  if (value.outcome !== "ok") {
    throw new Error(`Agent startup is not ready: ${value.outcome}`);
  }
  const { agent, conversation } = selectedAgent(state, value);
  if (
    !agent.status_available || !agent.prompt_supported ||
    (!agent.ready && agent.not_ready_reason !== "unconsumed_input")
  ) {
    throw new Error(
      "Agent lacks reporting or guarded-prompt readiness; inspect its visible pane",
    );
  }
  if (!conversation && state.config.kind !== "muse") {
    throw new Error("Agent has no provider conversation to guard");
  }
  if (
    agent.cwd && await dependencies.realPath(agent.cwd) !== state.config.cwd
  ) throw new Error("Agent is reporting a different working directory");
  state.conversationId = conversation;
  state.submissionId = dependencies.id();
  state.phase = "submitting";
  // The saved ID survives a lost CLI response; retries retrieve it instead of sending again.
  await save(context, state, dependencies);
  const completed = await response(
    context,
    state,
    commandArgs(state, [
      "prompt",
      state.paneId,
      "--file",
      "-",
      "--submission",
      state.submissionId,
      "--wait",
      "--timeout",
      `${state.config.timeoutSeconds}s`,
      ...(conversation ? ["--conversation", conversation] : []),
    ]),
    dependencies,
    state.continuation?.prompt ?? state.prompt,
  );
  state.phase = "waiting";
  await save(context, state, dependencies);
  return await acceptSubmission(context, state, completed, dependencies);
}

async function launch(
  context: AgentContext,
  state: AgentState,
  dependencies: Dependencies,
): Promise<ResourceHandle[]> {
  if (state.config.stackWorkers && !state.config.control) {
    throw new Error("Worker stacks require a control pane");
  }
  if (["codex", "muse", "agy"].includes(state.config.kind)) {
    const hooks = await dependencies.run(
      [
        "--session",
        state.config.session,
        "hooks",
        "status",
        "--agent",
        state.config.kind,
        ...(state.config.kind === "agy"
          ? ["--project-dir", state.config.cwd]
          : []),
      ],
      undefined,
      context.signal,
    );
    state.rawResponse = hooks.stdout || hooks.stderr;
    if (hooks.code !== 0 || !hooksReady(hooks.stdout)) {
      state.outcome = "hooks_unavailable";
      throw new Error(
        "Current, fully wired Yinz hooks are required; inspect hooks status and install them explicitly",
      );
    }
  }
  const args = [
    "spawn",
    "--kind",
    state.config.kind,
    "--cwd",
    state.config.cwd,
    "--allow-outside-worktree",
  ];
  if (state.config.model) args.push("--model", state.config.model);
  if (state.config.effort) args.push("--effort", state.config.effort);
  if (state.config.permissionMode) {
    args.push("--permission-mode", state.config.permissionMode);
  }
  if (state.config.control) {
    const control = state.config.control;
    if (control.session !== state.config.session) {
      throw new Error(
        "Control pane belongs to a different session than the worker",
      );
    }
    const result = await dependencies.run(
      ["--session", control.session, "action", "list-panes", "--json"],
      undefined,
      context.signal,
    );
    if (result.code !== 0) {
      throw new Error(
        `Cannot resolve control pane: ${result.stderr || result.stdout}`,
      );
    }
    const panes = z.array(z.object({
      id: z.number().int().nonnegative(),
      is_plugin: z.boolean(),
      tab_id: z.number().int().nonnegative(),
    })).parse(JSON.parse(result.stdout));
    const matches = panes.filter((pane) =>
      !pane.is_plugin && `terminal_${pane.id}` === control.paneId
    );
    if (matches.length !== 1) {
      throw new Error(
        `Cannot resolve control pane ${control.paneId} in session ${control.session}`,
      );
    }
    if (state.config.stackWorkers) {
      args.push("--stack-for", control.paneId);
    } else {
      args.push("--tab", String(matches[0].tab_id));
    }
  }
  // Read-only negotiation must succeed before recording the launch intent. A distinct wire
  // request on the CLI then prevents an older server silently dropping idempotency fields.
  const supportResult = await dependencies.run(
    commandArgs(state, ["launch-support"]),
    undefined,
    context.signal,
  );
  const support = AgentResponseSchema.parse(JSON.parse(supportResult.stdout));
  if (
    supportResult.code !== 0 || support.outcome !== "ok" ||
    support.output !== "agent-launch-transaction-v2" || !support.server_instance
  ) {
    throw new Error(
      "agent-launch-transaction-v2 is required; launch was not sent",
    );
  }
  state.launchVersion = 2;
  state.launchServer = support.server_instance;
  state.launchId = `${support.server_instance}.${dependencies.id()}`;
  args.push("--launch-id", state.launchId);
  state.phase = "launching";
  await save(context, state, dependencies);
  const value = await response(
    context,
    state,
    commandArgs(state, args),
    dependencies,
  );
  return await acceptLaunch(context, state, value, dependencies);
}

async function acceptLaunch(
  context: AgentContext,
  state: AgentState,
  value: AgentResponse,
  dependencies: Dependencies,
): Promise<ResourceHandle[]> {
  const receipt = value.launch;
  if (
    !receipt || receipt.version !== 2 || receipt.launch_id !== state.launchId ||
    receipt.server_instance !== state.launchServer ||
    (state.launchRequestDigest &&
      receipt.request_digest !== state.launchRequestDigest)
  ) {
    throw new Error(
      `Launch receipt does not match the saved intent; observe launch-status ${state.launchId}`,
    );
  }
  // An unavailable history lookup has no digest and cannot establish any execution facts.
  if (!/^[a-f0-9]{64}$/.test(receipt.request_digest)) {
    throw new Error(
      `Launch history is unknown; observe launch-status ${state.launchId}; do not spawn again`,
    );
  }
  state.launchRequestDigest ??= receipt.request_digest;
  state.launchReceipt = receipt;
  await save(context, state, dependencies);
  if (isRejectedLaunch(state)) {
    state.phase = "launch_rejected";
    await save(context, state, dependencies);
    throw new Error(
      `Agent launch ${receipt.outcome}: ${receipt.reason}; process ${receipt.process}; use discard-agent deliberately for this original task`,
    );
  }
  if (
    receipt.outcome !== "started" || receipt.process !== "running" ||
    receipt.disposition !== "available" || receipt.resources.cleanup_pending ||
    value.outcome !== "ok" || !value.spawned?.cwd
  ) {
    throw new Error(
      `Launch ${receipt.outcome} (${receipt.phase}, ${receipt.reason}); observe launch-status ${state.launchId}; do not spawn again`,
    );
  }
  state.paneId = value.spawned.pane_id;
  if (value.spawned.tab_id != null) state.tabId = value.spawned.tab_id;
  if (await dependencies.realPath(value.spawned.cwd) !== state.config.cwd) {
    throw new Error(
      "Spawned agent directory differs from the requested worktree",
    );
  }
  state.phase = "spawned";
  await save(context, state, dependencies);
  return await submit(context, state, dependencies);
}

async function guarded(
  context: AgentContext,
  state: AgentState,
  dependencies: Dependencies,
  operation: () => Promise<ResourceHandle[]>,
): Promise<{ dataHandles: ResourceHandle[] }> {
  try {
    return { dataHandles: await operation() };
  } catch (error) {
    state.lastError = error instanceof Error ? error.message : String(error);
    await save(context, state, dependencies);
    throw error;
  }
}

export async function runAgent(
  args: z.infer<typeof RunArgsSchema>,
  context: AgentContext,
  dependencies = defaultDependencies,
) {
  const prompt = RunArgsSchema.parse(args).prompt;
  const previous = await context.readResource("state");
  if (previous != null) {
    const state = AgentStateSchema.parse(previous);
    throw new Error(
      await promptDigest(prompt) === state.promptDigest
        ? "Agent task already exists; use resume to retrieve its submission"
        : "Agent task already exists with another prompt; use a new behavior identity",
    );
  }
  const config = AgentConfigSchema.parse(context.globalArgs);
  config.cwd = await dependencies.realPath(config.cwd);
  const timestamp = dependencies.now();
  const state = AgentStateSchema.parse({
    config,
    prompt,
    promptDigest: await promptDigest(prompt),
    phase: "preflight",
    backgroundTasks: [],
    createdAt: timestamp,
    updatedAt: timestamp,
  });
  await save(context, state, dependencies);
  return await guarded(
    context,
    state,
    dependencies,
    () => launch(context, state, dependencies),
  );
}

/** Invoked as an action so Swamp holds the child behavior's execution lock. */
export async function recoverAgent(
  args: z.infer<typeof RunArgsSchema>,
  context: AgentContext,
  dependencies = defaultDependencies,
) {
  const prompt = RunArgsSchema.parse(args).prompt;
  if (await context.readResource("state") == null) {
    return await runAgent({ prompt }, context, dependencies);
  }
  const state = await load(context, dependencies);
  if (
    state.prompt !== prompt || state.promptDigest !== await promptDigest(prompt)
  ) {
    throw new Error("Recovery prompt does not match the original task");
  }
  return await resumeAgent(context, dependencies);
}

/** Refresh failure evidence and independently prove the original pane has stopped. */
export async function confirmFailureAgent(
  context: AgentContext,
  dependencies = defaultDependencies,
) {
  const state = await load(context, dependencies);
  if (
    !state.paneId || !state.conversationId || !state.submissionId ||
    !["waiting", "submitting"].includes(state.phase)
  ) {
    throw new Error("Cannot discard an uncertain or active pane task");
  }
  return await guarded(context, state, dependencies, async () => {
    await response(
      context,
      state,
      commandArgs(state, [
        "wait",
        state.paneId!,
        "--submission",
        state.submissionId!,
        "--conversation",
        state.conversationId!,
        "--timeout",
        `${state.config.timeoutSeconds}s`,
      ]),
      dependencies,
    );
    if (!isTerminalFailure(state)) {
      throw new Error(
        "No confirmed terminal submission failure: require the original correlated submission with prompt_delivery: not_sent and no acknowledgement or turn ID, or an acknowledged provider failure; legacy text and uncertain delivery cannot authorize discard",
      );
    }
    // Status writes a separate observation, preserving the correlated failure response.
    const observation = await statusAgent(context, dependencies);
    const raw = AgentObservationSchema.parse(
      await context.readResource("observation"),
    );
    const value = AgentResponseSchema.parse(JSON.parse(raw.rawResponse));
    const { agent } = selectedAgent(state, value);
    if (
      !agent.status_available || agent.agent.state.toLowerCase() !== "idle" ||
      agent.background_tasks.length || value.background_tasks.length ||
      !agent.prompt_supported ||
      (agent.not_ready_reason != null &&
        agent.not_ready_reason !== "unconsumed_input") ||
      (!agent.ready && agent.not_ready_reason !== "unconsumed_input")
    ) {
      throw new Error(
        "Cannot discard an uncertain or active pane task: require fresh idle status with no background work or pending clearing/submission writes; wait for writes to drain and inspect the composer before reuse",
      );
    }
    return observation.dataHandles;
  });
}

export async function resumeAgent(
  context: AgentContext,
  dependencies = defaultDependencies,
) {
  const state = await load(context, dependencies);
  return await guarded(context, state, dependencies, async () => {
    if (state.phase === "completed_unusable") {
      if (state.outcome !== "completed" || state.backgroundTasks.length) {
        throw new Error("Legacy provider completion is not quiescent");
      }
      delete state.message;
      return await finish(context, state, dependencies);
    }
    if (state.phase === "launch_rejected") {
      throw new Error(
        "Agent launch was rejected; use discard-agent for this lifecycle task before dispatching a new task with corrected configuration",
      );
    }
    if (state.phase === "completed") {
      const result = AgentResultSchema.parse(
        await context.readResource("result"),
      );
      if (
        result.session !== state.config.session ||
        result.kind !== state.config.kind || result.cwd !== state.config.cwd ||
        result.paneId !== state.paneId ||
        result.conversationId !== state.conversationId ||
        result.submissionId !== state.submissionId ||
        result.promptDigest !== state.promptDigest
      ) {
        throw new Error(
          "Stored completion does not match the original submission",
        );
      }
      return [];
    }
    if (state.retainedSubmission) {
      validateRetained(state, state.retainedSubmission);
      state.message = state.retainedSubmission.message?.trim() || undefined;
      return await finish(context, state, dependencies);
    }
    if (isRejectedLaunch(state)) {
      throw new Error(
        "Agent launch has definitive terminal process facts; use discard-agent deliberately for this original task",
      );
    }
    if (state.phase === "background") {
      return await observeBackground(context, state, dependencies);
    }
    if (state.submissionId && state.paneId) {
      const value = await response(
        context,
        state,
        commandArgs(state, [
          "wait",
          state.paneId,
          "--submission",
          state.submissionId,
          "--timeout",
          `${state.config.timeoutSeconds}s`,
          ...(state.conversationId
            ? ["--conversation", state.conversationId]
            : []),
        ]),
        dependencies,
      );
      return await acceptSubmission(context, state, value, dependencies);
    }
    if (state.phase === "preflight") {
      return await launch(context, state, dependencies);
    }
    if (
      ["launching", "launch_rejected"].includes(state.phase) && state.launchId
    ) {
      const value = await response(
        context,
        state,
        commandArgs(state, ["launch-status", state.launchId]),
        dependencies,
      );
      return await acceptLaunch(context, state, value, dependencies);
    }
    if (state.phase === "spawned" && state.paneId) {
      return await submit(context, state, dependencies);
    }
    throw new Error(
      "Launch outcome is uncertain; inspect the Yinz session and retained resources, do not spawn again",
    );
  });
}

/** Check before persisting a new continuation in either controller or provider. */
export function assertContinuationEligible(state: AgentState) {
  if (
    !["completed", "completed_unusable"].includes(state.phase) ||
    state.outcome !== "completed" || state.backgroundTasks.length ||
    !state.paneId || !state.conversationId || !state.submissionId ||
    state.cleanedUp
  ) {
    throw new Error(
      "Submission continuation requires the original completed, retained conversation with no background work; inspect the pane, do not launch a replacement",
    );
  }
}

/** Continue the original conversation; persist intent before any guarded prompt. */
export async function continueAgent(
  input: z.infer<typeof ContinueArgsSchema>,
  context: AgentContext,
  dependencies = defaultDependencies,
) {
  const args = ContinueArgsSchema.parse(input);
  const state = await load(context, dependencies);
  if (state.continuation?.id === args.id) {
    if (state.continuation.prompt !== args.prompt) {
      throw new Error("Continuation identity has a different prompt");
    }
    return resumeAgent(context, dependencies);
  }
  // Each turn stores its predecessor before its intent is persisted. Walk that
  // chain so delayed retries cannot reuse an older identity or overwrite it.
  let previous = state;
  const visited = new Set<string>();
  while (previous.continuation) {
    const turn = previous.continuation;
    if (visited.has(turn.id)) {
      throw new Error(
        "Continuation history contains a cycle; inspect retained history",
      );
    }
    visited.add(turn.id);
    if (turn.id === args.id) {
      if (turn.prompt !== args.prompt) {
        throw new Error("Continuation identity has a different prompt");
      }
      throw new Error(
        "Continuation identity was already used by an older turn",
      );
    }
    const snapshot = await context.readResource(`turn-${turn.id}`);
    if (snapshot == null) {
      throw new Error(
        "Continuation history is missing a turn snapshot; inspect retained history",
      );
    }
    previous = AgentStateSchema.parse(snapshot);
  }
  if (await context.readResource(`turn-${args.id}`) != null) {
    throw new Error(
      "Continuation snapshot already exists; inspect retained history",
    );
  }
  assertContinuationEligible(state);
  return guarded(context, state, dependencies, async () => {
    await context.writeResource(
      "state",
      `turn-${args.id}`,
      structuredClone(state),
    );
    state.continuation = { ...args, previousSubmissionId: state.submissionId! };
    state.phase = "spawned";
    delete state.submissionId;
    delete state.message;
    delete state.turnId;
    delete state.retainedSubmission;
    delete state.outcome;
    await save(context, state, dependencies);
    return submit(context, state, dependencies);
  });
}

export async function statusAgent(
  context: AgentContext,
  dependencies = defaultDependencies,
) {
  const state = await load(context, dependencies);
  if (!state.paneId) return { dataHandles: [] };
  const result = await dependencies.run(
    commandArgs(state, [
      "status",
      state.paneId,
      ...(state.conversationId ? ["--conversation", state.conversationId] : []),
    ]),
    undefined,
    context.signal,
  );
  let value: AgentResponse;
  try {
    value = AgentResponseSchema.parse(JSON.parse(result.stdout));
  } catch {
    await context.writeResource("observation", "observation", {
      paneId: state.paneId,
      outcome: "transport_error",
      rawResponse: result.stdout || result.stderr,
      observedAt: dependencies.now(),
    });
    throw new Error(
      `Yinz status returned no valid response (exit ${result.code})`,
    );
  }
  const observation = await context.writeResource(
    "observation",
    "observation",
    {
      paneId: state.paneId,
      outcome: value.outcome,
      rawResponse: result.stdout,
      observedAt: dependencies.now(),
    },
  );
  if (result.code !== 0 || value.outcome !== "ok") {
    throw new Error(`Agent observation failed: ${value.outcome}`);
  }
  selectedAgent(state, value);
  return { dataHandles: [observation] };
}

/** Called only after the lifecycle has durably accepted the result. */
export async function cleanupAgent(
  context: AgentContext,
  dependencies = defaultDependencies,
) {
  const state = await load(context, dependencies);
  if (state.cleanedUp) return { dataHandles: [] };
  if (
    state.phase !== "completed" || !state.paneId || !state.conversationId ||
    !state.submissionId ||
    state.backgroundTasks.length
  ) throw new Error("Worker is not ready for cleanup");
  const listing = await dependencies.run(
    ["--session", state.config.session, "action", "list-panes", "--json"],
    undefined,
    context.signal,
  );
  if (listing.code !== 0) {
    throw new Error("Cannot inspect worker before cleanup");
  }
  const panes = z.array(z.object({ id: z.number(), is_plugin: z.boolean() }))
    .parse(JSON.parse(listing.stdout));
  if (panes.some((p) => !p.is_plugin && `terminal_${p.id}` === state.paneId)) {
    const result = await dependencies.run(
      commandArgs(state, [
        "close-completed",
        state.paneId,
        "--conversation",
        state.conversationId,
        "--submission",
        state.submissionId,
      ]),
      undefined,
      context.signal,
    );
    const value = AgentResponseSchema.parse(JSON.parse(result.stdout));
    if (result.code !== 0 || value.outcome !== "ok") {
      throw new Error(`Worker cleanup blocked: ${value.outcome}`);
    }
  }
  state.cleanedUp = true;
  return { dataHandles: [await save(context, state, dependencies)] };
}
