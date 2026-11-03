import { z } from "npm:zod@4.6.7";
import { isAbsolute } from "node:path";

const Text = z.string().trim().min(1);
const Pane = z.string().regex(/^terminal_[0-9]+$/);
export const AgentConfigSchema = z.object({
  session: Text,
  kind: z.enum(["claude", "codex", "muse", "pi", "agy"]),
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
  background_tasks: z.array(z.string()).default([]),
});
export const AgentResponseSchema = z.object({
  outcome: Text,
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
  acknowledged: z.boolean().default(false),
  turn_id: z.string().nullable().optional(),
  background_tasks: z.array(z.string()).default([]),
});
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
/** Only a correlated provider failure proves terminality; transport errors do not. */
export function isTerminalFailure(state: AgentState): boolean {
  if (
    !state.paneId || !state.conversationId || !state.submissionId ||
    !["waiting", "submitting"].includes(state.phase) || !state.rawResponse
  ) return false;
  try {
    const value = AgentResponseSchema.parse(JSON.parse(state.rawResponse));
    selectedAgent(state, value);
    return value.outcome === "failed" &&
      value.submission_id === state.submissionId;
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
  message: Text,
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
  if (parsed.turn_id) state.turnId = parsed.turn_id;
  state.backgroundTasks = parsed.background_tasks;
  if (state.phase === "launching" && rejectedWithoutResources(parsed)) {
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
    !state.paneId || !state.conversationId || !state.submissionId ||
    !state.message?.trim()
  ) {
    throw new Error(
      "Completion lacks a pane, bound conversation, submission or final message",
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
    agent.background_tasks.length || value.background_tasks.length ||
    !value.message?.trim()
  ) {
    throw new Error("Background work has no confirmed final idle answer");
  }
  return await finish(context, state, dependencies);
}

async function acceptSubmission(
  context: AgentContext,
  state: AgentState,
  value: AgentResponse,
  dependencies: Dependencies,
): Promise<ResourceHandle[]> {
  if (value.submission_id !== state.submissionId) {
    throw new Error("Yinz response does not match the saved submission ID");
  }
  if (value.outcome !== "completed") {
    throw new Error(`Agent task has not completed: ${value.outcome}`);
  }
  const { agent, conversation } = selectedAgent(state, value);
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
  if (!value.message?.trim()) {
    // Identity and both background lists have been verified above. Preserve
    // terminality even though there is no answer that can become evidence.
    state.phase = "completed_unusable";
    delete state.message;
    throw new Error(
      "Completed task did not return a final message; use discard-agent to discard this unusable completion",
    );
  }
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
    state.prompt,
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
  if (["codex", "muse", "agy"].includes(state.config.kind)) {
    const hooks = await dependencies.run(
      [
        "--session",
        state.config.session,
        "hooks",
        "status",
        "--agent",
        state.config.kind,
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
  state.phase = "launching";
  await save(context, state, dependencies);
  const value = await response(
    context,
    state,
    commandArgs(state, args),
    dependencies,
  );
  if (isRejectedLaunch(state)) {
    throw new Error(
      "Agent launch was rejected without resources; use discard-agent for this lifecycle task before dispatching a new task with corrected configuration",
    );
  }
  if (value.spawned) {
    state.paneId = value.spawned.pane_id;
    if (value.spawned.tab_id != null) state.tabId = value.spawned.tab_id;
    await save(context, state, dependencies);
  }
  if (value.outcome !== "ok" || !value.spawned?.cwd || !state.paneId) {
    throw new Error(
      `Agent launch needs inspection: ${value.outcome}; never retry an uncertain spawn`,
    );
  }
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
      throw new Error("No confirmed terminal submission failure");
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
      agent.background_tasks.length || value.background_tasks.length
    ) {
      throw new Error("Cannot discard an uncertain or active pane task");
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
      throw new Error(
        "Completed task did not return a final message; use discard-agent to discard this unusable completion",
      );
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
    if (state.phase === "spawned" && state.paneId) {
      return await submit(context, state, dependencies);
    }
    throw new Error(
      "Launch outcome is uncertain; inspect the Yinz session and retained resources, do not spawn again",
    );
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
