import { z } from "npm:zod@4.6.7";
import { isAbsolute, join } from "node:path";
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
const BACKGROUND_FLAG = "background";
/**
 * Yinz reports Claude background work as `background: true`; older builds listed
 * task IDs in `background_tasks`. Fold the flag into the list so every background
 * check, including persisted state, sees either form.
 */
function foldBackground<
  T extends { background: boolean; background_tasks: string[] },
>(value: T): T {
  return value.background && value.background_tasks.length === 0
    ? { ...value, background_tasks: [BACKGROUND_FLAG] }
    : value;
}
/**
 * Whether reported background work is only the folded flag, without task IDs:
 * Yinz's background rests on Claude's session registry, or the build predates IDs.
 */
export function unidentifiedBackground(tasks: string[]) {
  return tasks.length === 1 && tasks[0] === BACKGROUND_FLAG;
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
export type AgentResponse = z.infer<typeof AgentResponseSchema>;

/** Which read or write produced the state's latest saved agent response. */
export const ObservedViaSchema = z.enum([
  "startup",
  "prompt",
  "submission",
  "idle",
  "launch",
  "launch_status",
]);
export type ObservedVia = z.infer<typeof ObservedViaSchema>;

/**
 * The latest failed cleanup attempt. `thrown` is the exact error text
 * cleanupAgent threw, so a controller can tell this refusal from a stale one.
 * It is not trimmed: CLI output in the text can end in whitespace, and the
 * controller compares it byte for byte.
 */
export const CleanupRefusalSchema = z.object({
  outcome: Text,
  message: z.string().optional(),
  thrown: z.string().min(1),
  retryable: z.boolean(),
  submissionId: Text.optional(),
  conversationId: Text.optional(),
  observedAt: z.iso.datetime(),
}).strict();
export type CleanupRefusal = z.infer<typeof CleanupRefusalSchema>;

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
  promptArtifact: z.object({
    submissionId: Text,
    promptDigest: z.string().regex(/^[a-f0-9]{64}$/),
    path: Text.refine(isAbsolute),
  }).strict().optional(),
  outcome: Text.optional(),
  message: z.string().optional(),
  turnId: Text.optional(),
  backgroundTasks: z.array(z.string()).default([]),
  rawResponse: z.string().optional(),
  lastError: Text.optional(),
  failureKind: z.enum(["timeout", "other"]).optional(),
  observedVia: ObservedViaSchema.optional(),
  cleanedUp: z.boolean().optional(),
  cleanupRefusal: CleanupRefusalSchema.optional(),
  continuation: z.object({
    id: z.uuid(),
    prompt: z.string().min(1),
    previousSubmissionId: Text,
  }).strict().optional(),
  createdAt: z.iso.datetime(),
  updatedAt: z.iso.datetime(),
}).strict();
export type AgentState = z.infer<typeof AgentStateSchema>;

/** Resources a rejection response retains, or why it is not a rejection. */
function rejectedResponseProblems(value: AgentResponse): string[] {
  const problems: string[] = [];
  if (value.outcome !== "rejected") {
    problems.push(`response outcome is "${value.outcome}", expected rejected`);
  }
  const retained = [
    value.spawned != null && "spawned pane",
    value.agents.length > 0 && "agent rows",
    value.submission_id != null && "submission",
    value.acknowledged && "acknowledgement",
    value.turn_id != null && "turn",
    value.background_tasks.length > 0 && "background tasks",
    value.project != null && "project",
    value.restarted != null && "restarted pane",
    value.launch_resources != null && "launch resources",
  ].filter((item): item is string => typeof item === "string");
  if (retained.length) {
    problems.push(`saved rejection retains resources: ${retained.join(", ")}`);
  }
  return problems;
}

function rejectedWithoutResources(value: AgentResponse): boolean {
  return rejectedResponseProblems(value).length === 0;
}

function quoted(value: unknown): string {
  return value === undefined ? "absent" : JSON.stringify(value);
}

/**
 * Why the saved state does not prove a resource-free launch rejection. Empty
 * exactly when the launch is definitively rejected: a matching version-two
 * receipt with closed, non-running resources, or a legacy resource-free
 * structured rejection.
 */
export function rejectedLaunchProblems(state: AgentState): string[] {
  if (state.launchId && state.launchReceipt && !state.submissionId) {
    const receipt = state.launchReceipt;
    const problems: string[] = [];
    if (receipt.launch_id !== state.launchId) {
      problems.push(
        `launch receipt id ${
          quoted(receipt.launch_id)
        } differs from the saved launch ${quoted(state.launchId)}`,
      );
    }
    if (receipt.server_instance !== state.launchServer) {
      problems.push(
        `launch receipt server ${
          quoted(receipt.server_instance)
        } differs from the saved server ${quoted(state.launchServer)}`,
      );
    }
    if (receipt.request_digest !== state.launchRequestDigest) {
      problems.push(
        "launch receipt request digest differs from the saved request",
      );
    }
    if (!["rejected", "cancelled", "failed"].includes(receipt.outcome)) {
      problems.push(
        `launch receipt outcome is ${
          quoted(receipt.outcome)
        }, expected rejected, cancelled or failed`,
      );
    }
    if (!["not_started", "exited"].includes(receipt.process)) {
      problems.push(
        `launch receipt process is ${
          quoted(receipt.process)
        }, expected not_started or exited`,
      );
    }
    if (receipt.resources.cleanup_pending) {
      problems.push("cleanup is still pending");
    }
    if (receipt.disposition !== "closed") {
      problems.push(
        `receipt disposition is ${
          quoted(receipt.disposition)
        }, expected closed`,
      );
    }
    return problems;
  }
  const problems: string[] = [];
  if (state.phase !== "launch_rejected") {
    const why = state.launchId && state.launchReceipt
      ? "a submission exists, so the version-two launch receipt does not apply,"
      : "no version-two launch receipt";
    problems.push(
      `${why} and phase is ${quoted(state.phase)}, not launch_rejected`,
    );
  }
  if (state.outcome !== "rejected") {
    problems.push(`outcome is ${quoted(state.outcome)}, expected rejected`);
  }
  const retained = [
    state.paneId !== undefined && "spawned pane",
    state.tabId !== undefined && "tab",
    state.conversationId !== undefined && "conversation",
    state.submissionId !== undefined && "submission",
    state.turnId !== undefined && "turn",
    state.backgroundTasks.length > 0 && "background tasks",
  ].filter((item): item is string => typeof item === "string");
  if (retained.length) {
    problems.push(`saved rejection retains resources: ${retained.join(", ")}`);
  }
  if (state.rawResponse === undefined) {
    problems.push("no saved rejection response");
    return problems;
  }
  try {
    problems.push(
      ...rejectedResponseProblems(
        AgentResponseSchema.parse(JSON.parse(state.rawResponse)),
      ),
    );
  } catch {
    problems.push("saved rejection response is not a valid agent response");
  }
  return problems;
}

export function isRejectedLaunch(state: AgentState): boolean {
  return rejectedLaunchProblems(state).length === 0;
}
const PreparationFailures = new Set([
  "failed",
  "target_changed",
  "target_closed",
  "status_unavailable",
  "interrupted",
  "timed_out",
]);

/**
 * Why the saved response does not prove a terminal failure. Empty exactly when
 * a correlated provider failure or explicit non-delivery proves terminality.
 */
export function terminalFailureProblems(state: AgentState): string[] {
  const problems: string[] = [];
  if (!state.paneId) problems.push("no pane id");
  if (!state.conversationId) problems.push("no conversation id");
  if (!state.submissionId) problems.push("no submission id");
  if (!["waiting", "submitting"].includes(state.phase)) {
    problems.push(
      `phase is ${quoted(state.phase)}, expected waiting or submitting`,
    );
  }
  if (!state.rawResponse) {
    problems.push("no saved submission response");
    return problems;
  }
  let value: AgentResponse;
  try {
    value = AgentResponseSchema.parse(JSON.parse(state.rawResponse));
  } catch {
    problems.push("latest response is not a valid agent response");
    return problems;
  }
  let agentBackground: string[] = [];
  try {
    agentBackground = selectedAgent(state, value).agent.background_tasks;
  } catch (error) {
    problems.push(
      `latest response: ${error instanceof Error ? error.message : error}`,
    );
  }
  if (value.submission_id !== state.submissionId) {
    problems.push(
      "latest response submission differs from the saved submission",
    );
  }
  if (value.prompt_delivery === "not_sent") {
    if (!PreparationFailures.has(value.outcome)) {
      problems.push(
        `structured not_sent outcome ${
          quoted(value.outcome)
        } is not a preparation failure`,
      );
    }
    if (value.acknowledged) problems.push("not_sent response is acknowledged");
    if (value.turn_id != null) problems.push("not_sent response has a turn id");
    if (state.turnId !== undefined) problems.push("saved state has a turn id");
    if (
      value.background_tasks.length || agentBackground.length ||
      state.backgroundTasks.length
    ) problems.push("background tasks are running");
    return problems;
  }
  // A delivery error is also called `failed`; only an acknowledged provider
  // failure can use the existing terminal-turn recovery path.
  if (value.outcome !== "failed" || !value.acknowledged) {
    problems.push(
      `latest response outcome ${quoted(value.outcome)}${
        value.outcome === "failed" ? " (unacknowledged)" : ""
      } is neither an acknowledged failed nor a structured not_sent preparation failure`,
    );
  }
  return problems;
}

/**
 * Background work the saved child state and its latest response report for the
 * recorded pane. A timed-out wait carries Yinz's background flag and task IDs
 * only on the agent row, never at the top level saved in `backgroundTasks`.
 */
export function reportedBackground(state: AgentState) {
  const ids = new Set(state.backgroundTasks);
  let agentState: string | undefined;
  if (state.rawResponse) {
    let parsed;
    try {
      parsed = AgentResponseSchema.safeParse(JSON.parse(state.rawResponse));
    } catch {
      parsed = undefined;
    }
    if (parsed?.success) {
      for (const id of parsed.data.background_tasks) ids.add(id);
      const agent = parsed.data.agents.find((agent) =>
        agent.pane_id === state.paneId
      );
      agentState = agent?.agent.state;
      for (const id of agent?.background_tasks ?? []) ids.add(id);
    }
  }
  return { backgroundTasks: [...ids], agentState };
}

/** A correlated provider failure or explicit non-delivery proves terminality. */
export function isTerminalFailure(state: AgentState): boolean {
  return terminalFailureProblems(state).length === 0;
}

/**
 * Why the durable state is not a completed, quiescent worker with a retained
 * pane, conversation and submission. It carries no fresh observation.
 */
export function completedWorkerProblems(state: AgentState): string[] {
  const problems: string[] = [];
  if (!["completed", "completed_unusable"].includes(state.phase)) {
    problems.push(
      `phase is ${
        quoted(state.phase)
      }, expected completed or completed_unusable`,
    );
  }
  if (state.outcome !== "completed") {
    problems.push(`outcome is ${quoted(state.outcome)}, expected completed`);
  }
  if (state.backgroundTasks.length) {
    problems.push(
      `background tasks are running: ${state.backgroundTasks.join(", ")}`,
    );
  }
  if (!state.paneId) problems.push("no pane id");
  if (!state.conversationId) problems.push("no conversation id");
  if (!state.submissionId) problems.push("no submission id");
  return problems;
}

type AgentRow = AgentResponse["agents"][number];

/**
 * Why a fresh status row is not idle and drained. A leftover
 * `unconsumed_input` marker is not a problem.
 */
export function idleProblems(agent: AgentRow, value: AgentResponse): string[] {
  const problems: string[] = [];
  if (!agent.status_available) problems.push("status unavailable");
  if (agent.agent.state.toLowerCase() !== "idle") {
    problems.push(`agent state is ${quoted(agent.agent.state)}, expected idle`);
  }
  const background = [...agent.background_tasks, ...value.background_tasks];
  if (background.length) {
    problems.push(`background tasks are running: ${background.join(", ")}`);
  }
  if (!agent.prompt_supported) problems.push("prompt unsupported");
  if (
    agent.not_ready_reason != null &&
    agent.not_ready_reason !== "unconsumed_input"
  ) {
    problems.push(`not ready: ${agent.not_ready_reason}`);
  } else if (!agent.ready && agent.not_ready_reason !== "unconsumed_input") {
    problems.push("not ready: no reason reported");
  }
  return problems;
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
  // The default runner spawns local workers. Remote runners must reject artifact
  // delivery or provide a publisher whose returned path those workers can read.
  publishPromptArtifact: typeof publishPromptArtifact;
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
  publishPromptArtifact,
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

const MAX_PROMPT_BYTES = 1024 * 1024;

/** Publish exact bytes without ever exposing a partial file or replacing history. */
export async function publishPromptArtifact(
  cwd: string,
  submissionId: string,
  prompt: string,
  digest: string,
): Promise<string> {
  if (await promptDigest(prompt) !== digest) {
    throw new Error("Prompt artifact digest does not match the saved prompt");
  }
  // cwd is the canonical worker worktree, never a shared target or Swamp store.
  const directory = join(cwd, ".yinz-agent-prompts");
  try {
    await Deno.mkdir(directory, { mode: 0o700 });
  } catch (error) {
    if (!(error instanceof Deno.errors.AlreadyExists)) throw error;
  }
  const privateDirectory = await Deno.lstat(directory);
  if (
    !privateDirectory.isDirectory || privateDirectory.uid === null ||
    privateDirectory.mode === null ||
    (privateDirectory.mode & 0o777) !== 0o700
  ) {
    throw new Error(
      "Prompt artifact directory must be private and owned locally",
    );
  }
  const path = join(
    directory,
    `${await promptDigest(submissionId)}-${digest}.txt`,
  );
  const bytes = new TextEncoder().encode(prompt);
  async function verify() {
    const info = await Deno.lstat(path);
    if (
      !info.isFile || info.uid !== privateDirectory.uid || info.mode === null ||
      (info.mode & 0o777) !== 0o400
    ) {
      throw new Error(
        "Prompt artifact must be a private read-only regular file",
      );
    }
    const existing = await Deno.readFile(path);
    if (
      existing.length !== bytes.length ||
      !existing.every((byte, index) => byte === bytes[index])
    ) {
      throw new Error(
        "Prompt artifact collision or tampering; refusing delivery",
      );
    }
  }
  const temporary = join(directory, `.publishing-${crypto.randomUUID()}`);
  const file = await Deno.open(temporary, {
    write: true,
    createNew: true,
    mode: 0o600,
  });
  try {
    try {
      // A newly created file is owned by this process's effective user. Compare
      // that owner before writing any prompt bytes (no extra sys permission).
      if ((await file.stat()).uid !== privateDirectory.uid) {
        throw new Error("Prompt artifact directory must be owned locally");
      }
      let offset = 0;
      while (offset < bytes.length) {
        offset += await file.write(bytes.subarray(offset));
      }
      await Deno.chmod(temporary, 0o400);
      await file.sync();
    } finally {
      file.close();
    }
    try {
      // Hard-link publication is atomic and fails if the final name exists.
      await Deno.link(temporary, path);
    } catch (error) {
      if (!(error instanceof Deno.errors.AlreadyExists)) throw error;
    }
    const publishedDirectory = await Deno.open(directory, { read: true });
    try {
      await publishedDirectory.sync();
    } finally {
      publishedDirectory.close();
    }
    await verify();
    return path;
  } finally {
    await Deno.remove(temporary);
  }
}

async function deliveryPrompt(
  state: AgentState,
  submissionId: string,
  dependencies: Dependencies,
): Promise<string> {
  const prompt = state.continuation?.prompt ?? state.prompt;
  if (new TextEncoder().encode(prompt).length <= MAX_PROMPT_BYTES) {
    return prompt;
  }
  const digest = await promptDigest(prompt);
  if (!state.continuation && digest !== state.promptDigest) {
    throw new Error("Saved prompt does not match its original digest");
  }
  const path = await dependencies.publishPromptArtifact(
    state.config.cwd,
    submissionId,
    prompt,
    digest,
  );
  state.promptArtifact = { submissionId, promptDigest: digest, path };
  return `Read the complete assignment from the local UTF-8 file ${
    JSON.stringify(path)
  }. Before following it, verify the SHA-256 of its exact bytes is ${digest}. If the file is unavailable or the digest differs, stop and report the failure. Read all bytes without truncation, then follow the complete assignment, preserving all requirements and evidence correlation. This is submission ${
    JSON.stringify(submissionId)
  }; the file is its exact saved prompt. Do not edit the artifact.`;
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
  observedVia: ObservedVia,
  stdin?: string,
): Promise<AgentResponse> {
  const result = await dependencies.run(args, stdin, context.signal);
  state.rawResponse = result.stdout || result.stderr;
  // Diagnostic only: legacy records lack it, so no recovery decision depends on it.
  state.observedVia = observedVia;
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
  delete state.failureKind;
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
    "idle",
  );
  if (value.outcome !== "ok" && value.outcome !== "completed") {
    throw new ObservationError(
      `Background work has not completed: ${value.outcome}`,
      value.outcome,
    );
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
    throw new ObservationError(
      `Agent task has not completed: ${value.outcome}${
        value.message ? ": " + value.message : ""
      }`,
      value.outcome,
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
    "startup",
  );
  if (value.outcome !== "ok") {
    throw new ObservationError(
      `Agent startup is not ready: ${value.outcome}`,
      value.outcome,
    );
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
  const submissionId = dependencies.id();
  // Publication must finish before the new submission intent is observable. A
  // failure leaves this task spawned and safe to retry; no prompt has been sent.
  const prompt = await deliveryPrompt(state, submissionId, dependencies);
  state.submissionId = submissionId;
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
    "prompt",
    prompt,
  );
  state.phase = "waiting";
  await save(context, state, dependencies);
  if (inputInterruptedObservation(state, completed)) {
    // Pane input ends only the prompt's own observation; the accepted turn can
    // continue. The submission wait ignores input and returns the provider's
    // actual outcome, including a real interruption, without resending.
    return await acceptSubmission(
      context,
      state,
      await waitForSubmission(context, state, dependencies),
      dependencies,
    );
  }
  return await acceptSubmission(context, state, completed, dependencies);
}

/** An acknowledged prompt whose observation, not necessarily its turn, ended. */
function inputInterruptedObservation(
  state: AgentState,
  value: AgentResponse,
): boolean {
  return value.outcome === "interrupted" && value.acknowledged &&
    value.prompt_delivery === "started" &&
    value.submission_id === state.submissionId &&
    (state.turnId === undefined || value.turn_id === state.turnId);
}

async function waitForSubmission(
  context: AgentContext,
  state: AgentState,
  dependencies: Dependencies,
): Promise<AgentResponse> {
  if (!state.paneId || !state.submissionId) {
    throw new Error(
      "Submission wait requires the recorded pane and submission",
    );
  }
  return await response(
    context,
    state,
    commandArgs(state, [
      "wait",
      state.paneId,
      "--submission",
      state.submissionId,
      "--timeout",
      `${state.config.timeoutSeconds}s`,
      ...(state.conversationId ? ["--conversation", state.conversationId] : []),
    ]),
    dependencies,
    "submission",
  );
}

/**
 * A read-only launch preflight lost its transport (CLI failure or unparsable
 * output) before any launch intent was saved or spawn was sent.
 */
export class PreflightTransportError extends Error {}

function preflightTransport(
  state: AgentState,
  what: string,
  detail: string,
): PreflightTransportError {
  state.outcome = "preflight_transport";
  return new PreflightTransportError(
    `Launch preflight transport failure (${what}): ${detail}; launch was not sent`,
  );
}

function preflightJson(
  state: AgentState,
  what: string,
  result: CommandResult,
): unknown {
  try {
    return JSON.parse(result.stdout);
  } catch (error) {
    throw preflightTransport(
      state,
      what,
      `invalid JSON (exit ${result.code}): ${
        error instanceof Error ? error.message : String(error)
      }${
        result.stderr || result.stdout
          ? `: ${result.stderr || result.stdout}`
          : ""
      }`,
    );
  }
}

async function launch(
  context: AgentContext,
  state: AgentState,
  dependencies: Dependencies,
): Promise<ResourceHandle[]> {
  // A retried preflight starts fresh; a previous attempt's outcome must not
  // describe this attempt's failure.
  delete state.outcome;
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
      throw preflightTransport(
        state,
        "list-panes",
        `Cannot resolve control pane: ${result.stderr || result.stdout}`,
      );
    }
    const panes = z.array(z.object({
      id: z.number().int().nonnegative(),
      is_plugin: z.boolean(),
      tab_id: z.number().int().nonnegative(),
    })).parse(preflightJson(state, "list-panes", result));
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
  const support = AgentResponseSchema.parse(
    preflightJson(state, "launch-support", supportResult),
  );
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
  // Invariant: the launch intent (launchId, phase `launching`) is saved before
  // spawn is sent. A state still in `preflight` without launchId therefore
  // proves no spawn was sent, so retrying a preflight transport failure is a
  // first launch, never a respawn. Keep this order if launch() is reordered.
  await save(context, state, dependencies);
  const value = await response(
    context,
    state,
    commandArgs(state, args),
    dependencies,
    "launch",
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

class ObservationError extends Error {
  constructor(message: string, readonly outcome: string) {
    super(message);
  }
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
    state.failureKind =
      error instanceof ObservationError && error.outcome === "timed_out"
        ? "timeout"
        : "other";
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
      "submission",
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
    const problems = idleProblems(agent, value);
    if (problems.length) {
      throw new Error(
        `Cannot discard an uncertain or active pane task: ${
          problems.join("; ")
        }; require fresh idle status with no background work or pending clearing/submission writes; wait for writes to drain and inspect the composer before reuse`,
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
      const value = await waitForSubmission(context, state, dependencies);
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
        "launch_status",
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
  const problems = completedWorkerProblems(state);
  if (state.cleanedUp) problems.push("the worker was already cleaned up");
  if (problems.length) {
    throw new Error(
      `Submission continuation requires the original completed, retained conversation with no background work; inspect the pane, do not launch a replacement (${
        problems.join("; ")
      })`,
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
    delete state.promptArtifact;
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

/**
 * confirm_stopped's refusal prefix when it did observe the worker and found it
 * not stopped. An operator attestation never overrides that observation.
 */
export const WORKER_NOT_STOPPED = "Completed worker is not stopped";

/**
 * Freshly observe that a completed worker stopped: its pane is listed in its
 * session and idle in the same provider and conversation with no background
 * work or pending writes. A pane missing from the session's listing is not
 * evidence of a stop: warp-pane and warp-tab move a pane, running process and
 * all, to another session, and a pty-host supervisor can outlive its server
 * while a new server reuses the session name. That case, like a failed
 * listing, is refused as unobservable. It writes only the `observation`
 * resource; it never changes `state`, so a refusal leaves no lastError or
 * failureKind behind.
 */
export async function confirmStoppedAgent(
  context: AgentContext,
  dependencies = defaultDependencies,
) {
  const state = await load(context, dependencies);
  const problems = completedWorkerProblems(state);
  if (problems.length) {
    throw new Error(
      `Cannot confirm a stopped worker: ${problems.join("; ")}`,
    );
  }
  const session = state.config.session;
  const listing = await dependencies.run(
    ["--session", session, "action", "list-panes", "--json"],
    undefined,
    context.signal,
  );
  const unobservable =
    "the behavior cannot observe whether the completed worker stopped";
  if (listing.code !== 0) {
    throw new Error(`Cannot list panes in session ${session}; ${unobservable}`);
  }
  let panes: { id: number; is_plugin: boolean }[];
  try {
    panes = z.array(z.object({ id: z.number(), is_plugin: z.boolean() }))
      .parse(JSON.parse(listing.stdout));
  } catch {
    throw new Error(
      `Pane listing for session ${session} is malformed; ${unobservable}`,
    );
  }
  if (!panes.some((p) => !p.is_plugin && `terminal_${p.id}` === state.paneId)) {
    throw new Error(
      `Pane ${state.paneId} is not listed in session ${session}; it may have moved to another session or outlived its server, so ${unobservable}`,
    );
  }
  // statusAgent rejects a failed observation or a changed session, provider
  // or conversation before the idle check below.
  const observation = await statusAgent(context, dependencies);
  const raw = AgentObservationSchema.parse(
    await context.readResource("observation"),
  );
  const value = AgentResponseSchema.parse(JSON.parse(raw.rawResponse));
  const { agent } = selectedAgent(state, value);
  const idle = idleProblems(agent, value);
  if (idle.length) {
    throw new Error(`${WORKER_NOT_STOPPED}: ${idle.join("; ")}`);
  }
  return observation;
}

/**
 * Fragments of Rust `CloseCompleted` refusals (yinz-screen agent_control/close_completed.rs)
 * that concern identity, correlation or pane topology. Retrying cannot change
 * them, so they always need a human. unit/yinz_agent_test.ts pins them against
 * the Rust source.
 */
export const CLOSE_COMPLETED_CORRELATION_REFUSALS = [
  "close-completed requires the worker's conversation",
  "is not retained for this worker",
  "is not the worker's latest submission",
  "has no acknowledged completed result in this conversation",
  "worker cleanup would affect other panes",
  "worker pane could not be closed",
] as const;

/** Exact `CloseCompleted` guards for work that finishes by itself. */
export const CLOSE_COMPLETED_TRANSIENT_REFUSALS = [
  "worker is Busy, not idle",
  "worker background work is still running",
  "worker has an unresolved accepted turn",
  "a Yinz prompt write to the worker is still in flight",
  "a Yinz control operation on the worker is still in flight",
] as const;

const TransientNotReady = new Set([
  "pending_submission",
  "clearing_draft",
  "unresolved_turn",
  "not_idle",
]);

/**
 * Whether a close-completed refusal is a transient guard on the same worker,
 * conversation and submission. Status, dialog, exit, identity and correlation
 * refusals are never transient. Rust checks the conversation and idleness
 * before correlation, so a correlation failure hidden behind `Busy` appears on
 * a later attempt and is then not retryable; close-completed stays fully
 * guarded on every attempt.
 */
export function transientCleanupRefusal(
  state: AgentState,
  value: AgentResponse,
): boolean {
  if (value.outcome !== "rejected") return false;
  const message = value.message ?? "";
  if (CLOSE_COMPLETED_CORRELATION_REFUSALS.some((f) => message.includes(f))) {
    return false;
  }
  if (/^worker is (Waiting|Exited), not idle$/.test(message)) return false;
  const rows = value.agents.filter((row) => row.pane_id === state.paneId);
  if (rows.length !== 1) return false;
  const [row] = rows;
  if (
    row.session !== state.config.session ||
    row.agent.kind !== state.config.kind ||
    (row.agent.session_id || undefined) !== state.conversationId
  ) return false;
  const agentState = row.agent.state.toLowerCase();
  if (agentState === "waiting" || agentState === "exited") return false;
  return (CLOSE_COMPLETED_TRANSIENT_REFUSALS as readonly string[]).includes(
    message,
  ) || agentState === "busy" || row.background_tasks.length > 0 ||
    value.background_tasks.length > 0 ||
    TransientNotReady.has(row.not_ready_reason ?? "");
}

/** Called only after the lifecycle has durably accepted the result. */
export async function cleanupAgent(
  context: AgentContext,
  dependencies = defaultDependencies,
) {
  const state = await load(context, dependencies);
  if (state.cleanedUp) return { dataHandles: [] };
  // Every refusal is saved before it is thrown, with the exact thrown text, so
  // a controller can match a fresh refusal to the error it received.
  const refuse = async (
    error: Error,
    outcome: string,
    retryable: boolean,
    message?: string,
  ): Promise<Error> => {
    state.cleanupRefusal = CleanupRefusalSchema.parse({
      outcome,
      ...(message != null ? { message } : {}),
      thrown: error.message,
      retryable,
      ...(state.submissionId ? { submissionId: state.submissionId } : {}),
      ...(state.conversationId ? { conversationId: state.conversationId } : {}),
      observedAt: dependencies.now(),
    });
    await save(context, state, dependencies);
    return error;
  };
  if (
    state.phase !== "completed" || !state.paneId || !state.conversationId ||
    !state.submissionId ||
    state.backgroundTasks.length
  ) {
    throw await refuse(
      new Error("Worker is not ready for cleanup"),
      "not_ready",
      false,
    );
  }
  const listing = await dependencies.run(
    ["--session", state.config.session, "action", "list-panes", "--json"],
    undefined,
    context.signal,
  );
  if (listing.code !== 0) {
    throw await refuse(
      new Error("Cannot inspect worker before cleanup"),
      "list_panes_failed",
      true,
    );
  }
  let panes: { id: number; is_plugin: boolean }[];
  try {
    panes = z.array(z.object({ id: z.number(), is_plugin: z.boolean() }))
      .parse(JSON.parse(listing.stdout));
  } catch (error) {
    throw await refuse(
      error instanceof Error ? error : new Error(String(error)),
      "list_panes_failed",
      true,
    );
  }
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
    let value: AgentResponse;
    try {
      value = AgentResponseSchema.parse(JSON.parse(result.stdout));
    } catch {
      throw await refuse(
        new Error(
          `Worker cleanup returned no valid agent response (exit ${result.code}): ${
            result.stderr || result.stdout
          }`,
        ),
        "transport_error",
        true,
      );
    }
    if (result.code !== 0 || value.outcome !== "ok") {
      throw await refuse(
        new Error(
          `Worker cleanup blocked: ${value.outcome}${
            value.message ? `: ${value.message}` : ""
          }`,
        ),
        value.outcome,
        transientCleanupRefusal(state, value),
        value.message ?? undefined,
      );
    }
  }
  delete state.cleanupRefusal;
  state.cleanedUp = true;
  return { dataHandles: [await save(context, state, dependencies)] };
}
