/** Local process and ownership boundary for the Yinz development lifecycle. */
import { join } from "node:path";
import { z } from "npm:zod@4.6.7";
import {
  type LifecycleState,
  type MergeFailure,
  MergeFailureSchema,
  ReceiptSchema,
  StateSchema,
} from "./lifecycle.ts";

export const GlobalArgsSchema = z.object({
  workspace: z.string().regex(/^[a-z0-9][a-z0-9_-]{0,63}$/),
  itemId: z.number().int().positive().safe(),
}).strict();

export type Handle = { name: string };
export interface Context {
  repoDir: string;
  modelId: string;
  definition: { name: string };
  globalArgs: z.infer<typeof GlobalArgsSchema>;
  signal: AbortSignal;
  logger: {
    info: (message: string, values: Record<string, unknown>) => void;
  };
  readResource: (name: string) => Promise<unknown>;
  writeResource: (
    spec: string,
    name: string,
    value: Record<string, unknown>,
  ) => Promise<Handle>;
  createFileWriter: (spec: string, name: string) => {
    writeStream: (stream: ReadableStream<Uint8Array>) => Promise<Handle>;
  };
}

/** Direct submission requires independent writers while the controller waits. */
export async function submissionPreflight(context: Context) {
  const result = await new Deno.Command("swamp", {
    args: [
      "datastore",
      "status",
      "--repo-dir",
      context.repoDir,
      "--json",
      "--no-telemetry",
    ],
    env: {
      SWAMP_NO_INBOX_NOTICE: "1",
      SWAMP_SERVE_URL: "",
      SWAMP_SERVER_URL: "",
    },
    stdin: "null",
    stdout: "piped",
    stderr: "piped",
    signal: context.signal,
  }).output();
  if (!result.success) {
    throw new Error(
      `Cannot preflight Swamp submission access: ${
        new TextDecoder().decode(result.stderr)
      }`,
    );
  }
  assertSubmissionBackend(JSON.parse(new TextDecoder().decode(result.stdout)));
}

export function assertSubmissionBackend(raw: unknown) {
  const backend = z.object({
    type: z.string(),
    healthy: z.boolean(),
    format: z.number().optional(),
  }).parse(raw);
  if (
    !backend.healthy || backend.type !== "filesystem" ||
    (backend.format !== undefined && backend.format !== 2)
  ) {
    throw new Error(
      "Typed worker submission requires a healthy local format-2 filesystem Swamp datastore; solo format-3 holds a writer lease across the waiting controller. No worker was launched",
    );
  }
}

export const EventSchema = z.object({
  action: z.string().min(1),
  at: z.iso.datetime(),
  state: StateSchema,
}).strict();

export const InspectionSchema = z.object({
  commonDir: z.string().min(1),
  worktree: z.string().min(1),
  branch: z.string(),
  head: z.string().regex(/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/),
  main: z.string().regex(/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/),
  clean: z.boolean(),
});

const Commit = z.string().regex(/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/);
const CiCommand = z.tuple([
  z.literal("cargo"),
  z.literal("x"),
  z.literal("ci"),
  z.literal("check"),
]);
// Merge-gate details from git_ops.py; receipts written before them omit them.
const GateDetails = {
  log: z.string().min(1).optional(),
  exitCode: z.number().int().nullable().optional(),
  phases: z.array(z.object({
    name: z.string().min(1),
    status: z.enum(["passed", "failed"]),
  })).optional(),
  failedTests: z.array(z.string().min(1)).optional(),
  failedTestsTruncated: z.number().int().positive().optional(),
  summary: z.string().optional(),
};
const GitPreviousAttemptSchema = z.object({
  attemptId: z.uuid(),
  failure: z.string().nullable().optional(),
  ci: z.object({ summary: z.string().optional(), log: z.string().optional() })
    .loose().optional(),
}).loose();
const GitAttemptFields = {
  attemptId: z.uuid(),
  issueKey: z.string(),
  issueDigest: z.string(),
  worktree: z.string(),
  branch: z.string(),
  originalCommit: Commit,
  previousAttempts: z.array(GitPreviousAttemptSchema).optional(),
};
export const GitReceiptSchema = z.object({
  ...GitAttemptFields,
  status: z.literal("merged"),
  baseCommit: Commit,
  rebasedCommit: Commit,
  mergedCommit: Commit,
  mergedAt: z.string(),
  ci: z.object({
    status: z.enum(["passed", "skipped"]),
    command: CiCommand,
    request: z.string().optional(),
    ...GateDetails,
  }),
});
export const GitFailedReceiptSchema = z.object({
  ...GitAttemptFields,
  status: z.literal("failed"),
  failure: z.string().min(1),
  timestamp: z.string(),
  ci: z.object({
    status: z.string().min(1),
    command: CiCommand,
    request: z.string().optional(),
    ...GateDetails,
  }),
});

/** The Swamp file that holds the helper's output for one merge attempt. */
export function mergeLogName(attemptId: string) {
  return `merge-log-${attemptId}`;
}

function assertIssue(
  receipt: { issueKey: string; issueDigest: string },
  state: LifecycleState,
) {
  if (
    receipt.issueKey !== `${state.issue.workspace}/${state.issue.itemId}` ||
    receipt.issueDigest !== state.issue.digest
  ) {
    throw new Error(
      "Merge receipt belongs to a different issue or description",
    );
  }
}

async function sha256(text: string): Promise<string> {
  const hash = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(text),
  );
  return Array.from(
    new Uint8Array(hash),
    (byte) => byte.toString(16).padStart(2, "0"),
  ).join("");
}

export async function issueName(
  workspace: string,
  itemId: number,
): Promise<string> {
  const name = `issue-${workspace}-${itemId}`;
  return name.length <= 64
    ? name
    : `issue-${(await sha256(workspace)).slice(0, 24)}-${itemId}`;
}

/** Keep the Python supervisor alive until it has stopped and reaped its commands. */
export async function gitOperation(
  context: Pick<Context, "repoDir" | "signal" | "createFileWriter">,
  request: Record<string, unknown>,
  logName?: string,
  logSpec = "command-log",
): Promise<unknown> {
  context.signal.throwIfAborted();
  const child = new Deno.Command("python3", {
    args: [join(context.repoDir, "scripts", "issue_lifecycle", "git_ops.py")],
    cwd: context.repoDir,
    stdin: "piped",
    stdout: "piped",
    stderr: "piped",
  }).spawn();
  const abort = () => {
    try {
      child.kill("SIGTERM");
    } catch (error) {
      if (
        !(error instanceof Deno.errors.NotFound) &&
        !(error instanceof TypeError &&
          error.message.includes("already terminated"))
      ) throw error;
    }
  };
  context.signal.addEventListener("abort", abort, { once: true });
  if (context.signal.aborted) abort();
  // Install rejection handlers immediately. A failed datastore log writer must
  // stop the supervisor, rather than leave it mutating Git after we return.
  const output = new Response(child.stdout).text();
  const errors = Promise.resolve().then<string | Handle>(() =>
    logName
      ? context.createFileWriter(logSpec, logName).writeStream(
        child.stderr,
      )
      : new Response(child.stderr).text()
  );
  const statusPromise = child.status;
  const settled = Promise.allSettled([statusPromise, output, errors]);
  try {
    const input = child.stdin.getWriter();
    await input.write(
      new TextEncoder().encode(
        JSON.stringify({ repoPath: context.repoDir, ...request }),
      ),
    );
    await input.close();
    const [status, text, diagnostics] = await Promise.all([
      statusPromise,
      output,
      errors,
    ]);
    let result: unknown;
    try {
      result = JSON.parse(text);
    } catch (cause) {
      throw new Error(
        `Git helper returned invalid JSON (exit ${status.code})`,
        { cause },
      );
    }
    if (!status.success) {
      const failure = z.object({ error: z.string() }).safeParse(result);
      throw new Error(
        failure.success
          ? failure.data.error
          : `Git helper exited ${status.code}: ${
            typeof diagnostics === "string" ? diagnostics : logName
          }`,
      );
    }
    context.signal.throwIfAborted();
    return result;
  } catch (error) {
    abort();
    if (!child.stderr.locked) await child.stderr.cancel().catch(() => {});
    await settled;
    throw error;
  } finally {
    context.signal.removeEventListener("abort", abort);
  }
}

export async function inspect(
  context: Pick<Context, "repoDir" | "signal" | "createFileWriter">,
  worktree?: string,
) {
  return InspectionSchema.parse(
    await gitOperation(context, {
      action: "inspect",
      ...(worktree ? { worktree } : {}),
    }),
  );
}

const OwnerSchema = z.object({
  control: z.string(),
  modelId: z.string(),
  issueKey: z.string(),
  agendaData: z.string(),
}).strict();

async function lockIssue(lock: Deno.FsFile, signal: AbortSignal) {
  while (true) {
    signal.throwIfAborted();
    if (await lock.tryLock(true)) return;
    await new Promise<void>((resolve, reject) => {
      const abort = () => {
        clearTimeout(timer);
        reject(signal.reason);
      };
      const timer = setTimeout(() => {
        signal.removeEventListener("abort", abort);
        resolve();
      }, 100);
      signal.addEventListener("abort", abort, { once: true });
      if (signal.aborted) abort();
    });
  }
}

/** Bind one Agenda issue to one control checkout and hold a nonexpiring issue lock. */
export async function withIssue<T>(
  context: Context,
  work: () => Promise<T>,
): Promise<T> {
  const args = GlobalArgsSchema.parse(context.globalArgs);
  await gitOperation(context, {
    action: "agenda-workspace",
    expectedWorkspace: args.workspace,
  });
  const expectedName = await issueName(args.workspace, args.itemId);
  if (context.definition.name !== expectedName) {
    throw new Error(`Use behavior ${expectedName} for this Agenda issue`);
  }
  const repo = await inspect(context);
  const key = `${args.workspace}/${args.itemId}`;
  const directory = join(repo.commonDir, "yinz-lifecycle", "issues");
  await Deno.mkdir(directory, { recursive: true });
  const stem = join(directory, await sha256(key));
  const lock = await Deno.open(`${stem}.lock`, { create: true, write: true });
  try {
    await lockIssue(lock, context.signal);
    context.signal.throwIfAborted();
    const home = Deno.env.get("HOME");
    const xdg = Deno.env.get("XDG_DATA_HOME");
    const agendaData = xdg?.startsWith("/")
      ? xdg
      : home?.startsWith("/")
      ? join(home, ".local", "share")
      : undefined;
    if (!agendaData) {
      throw new Error("Agenda requires an absolute HOME or XDG_DATA_HOME");
    }
    const owner = {
      control: await Deno.realPath(context.repoDir),
      modelId: context.modelId,
      issueKey: key,
      agendaData: await Deno.realPath(agendaData),
    };
    let existing: z.infer<typeof OwnerSchema> | undefined;
    try {
      existing = OwnerSchema.parse(
        JSON.parse(await Deno.readTextFile(`${stem}.json`)),
      );
    } catch (error) {
      if (!(error instanceof Deno.errors.NotFound)) throw error;
    }
    if (
      existing &&
      Object.keys(owner).some((key) =>
        owner[key as keyof typeof owner] !==
          existing![key as keyof typeof owner]
      )
    ) {
      throw new Error(
        `This issue already belongs to control checkout ${existing.control}, behavior ${existing.modelId}; resume there with its original Agenda data directory`,
      );
    }
    if (!existing) {
      const temporary = `${stem}.${crypto.randomUUID()}.tmp`;
      await Deno.writeTextFile(temporary, JSON.stringify(owner));
      await Deno.rename(temporary, `${stem}.json`);
    }
    return await work();
  } finally {
    lock.close();
  }
}

export async function loadState(context: Context): Promise<LifecycleState> {
  const state = StateSchema.parse(await context.readResource("state"));
  if (
    state.issue.workspace !== context.globalArgs.workspace ||
    state.issue.itemId !== context.globalArgs.itemId
  ) {
    throw new Error("Behavior identity does not match its stored Agenda issue");
  }
  return state;
}

export async function saveState(
  context: Context,
  action: string,
  value: LifecycleState,
  eventName = `event-${crypto.randomUUID()}`,
): Promise<{ dataHandles: Handle[] }> {
  const state = StateSchema.parse(value);
  await invalidateHandoff(context);
  const event = EventSchema.parse({
    action,
    at: new Date().toISOString(),
    state,
  });
  const existing = await context.readResource(eventName);
  if (existing != null) {
    const saved = EventSchema.parse(existing);
    if (
      saved.action !== action ||
      JSON.stringify(saved.state) !== JSON.stringify(state)
    ) {
      throw new Error(
        "Stable lifecycle event identity conflicts with its saved transition",
      );
    }
  }
  const eventHandle = existing == null
    ? await context.writeResource("event", eventName, event)
    : undefined;
  const stateHandle = await context.writeResource("state", "state", state);
  context.logger.info("Issue {issue} completed {action}; phase {phase}", {
    issue: context.definition.name,
    action,
    phase: state.phase,
  });
  return {
    dataHandles: eventHandle ? [eventHandle, stateHandle] : [stateHandle],
  };
}

/** Old handoffs remain evidence, but cannot authorize a superseding attempt. */
export async function invalidateHandoff(context: Context) {
  const raw = await context.readResource("handoff");
  if (raw != null) {
    const handoff = z.object({ status: z.string() }).loose().parse(raw);
    if (handoff.status === "current") {
      await context.writeResource("handoff", "handoff", {
        ...handoff,
        status: "invalidated",
      });
    }
  }
}

export function normalizeReceipt(raw: unknown, state: LifecycleState) {
  const receipt = GitReceiptSchema.parse(raw);
  assertIssue(receipt, state);
  const previous = receipt.previousAttempts?.map((attempt) => ({
    attemptId: attempt.attemptId,
    summary: attempt.ci?.summary?.trim() || attempt.failure?.trim() ||
      "failed",
    ...(attempt.ci?.log ? { log: attempt.ci.log } : {}),
  }));
  const normalized = ReceiptSchema.parse({
    attemptId: receipt.attemptId,
    originalCommit: receipt.originalCommit,
    rebasedCommit: receipt.rebasedCommit,
    baseCommit: receipt.baseCommit,
    mergedCommit: receipt.mergedCommit,
    worktree: receipt.worktree,
    branch: receipt.branch,
    ciStatus: receipt.ci.status,
    ...(receipt.ci.request ? { skipCiRequest: receipt.ci.request } : {}),
    mergedAt: new Date(receipt.mergedAt).toISOString(),
    ...(previous?.length ? { previousAttempts: previous } : {}),
  });
  if (
    state.receipt &&
    JSON.stringify(normalized) !==
      JSON.stringify(ReceiptSchema.parse(state.receipt))
  ) {
    throw new Error("Git receipt no longer matches the recorded merge receipt");
  }
  return normalized;
}

/** Bind a failed Git receipt to the active attempt and keep its gate evidence. */
export function normalizeFailure(
  raw: unknown,
  state: LifecycleState,
): MergeFailure {
  const receipt = GitFailedReceiptSchema.parse(raw);
  assertIssue(receipt, state);
  if (
    !state.attempt || receipt.attemptId !== state.attempt.id ||
    receipt.originalCommit !== state.attempt.originalHead ||
    receipt.worktree !== state.implementation?.worktree ||
    receipt.branch !== state.implementation?.branch
  ) {
    throw new Error(
      "Failed merge receipt must match the active attempt and implementation",
    );
  }
  const { ci } = receipt;
  return MergeFailureSchema.parse({
    attemptId: receipt.attemptId,
    at: new Date(receipt.timestamp).toISOString(),
    originalCommit: receipt.originalCommit,
    summary: ci.summary?.trim() || receipt.failure,
    failure: receipt.failure,
    ciStatus: ci.status,
    ...(ci.log ? { log: ci.log } : {}),
    logResource: mergeLogName(receipt.attemptId),
    ...(ci.exitCode !== undefined ? { exitCode: ci.exitCode } : {}),
    phases: ci.phases ?? [],
    failedTests: ci.failedTests ?? [],
    ...(ci.failedTestsTruncated
      ? { failedTestsTruncated: ci.failedTestsTruncated }
      : {}),
  });
}
