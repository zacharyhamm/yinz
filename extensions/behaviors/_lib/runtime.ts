/** Local process and ownership boundary for the Yinz development lifecycle. */
import { join } from "node:path";
import { z } from "npm:zod@4.6.7";
import {
  type LifecycleState,
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
export const GitReceiptSchema = z.object({
  status: z.literal("merged"),
  attemptId: z.uuid(),
  issueKey: z.string(),
  issueDigest: z.string(),
  worktree: z.string(),
  branch: z.string(),
  originalCommit: Commit,
  baseCommit: Commit,
  rebasedCommit: Commit,
  mergedCommit: Commit,
  mergedAt: z.string(),
  ci: z.object({
    status: z.enum(["passed", "skipped"]),
    command: z.tuple([
      z.literal("cargo"),
      z.literal("x"),
      z.literal("ci"),
      z.literal("check"),
    ]),
    request: z.string().optional(),
  }),
});

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
  context: Context,
  request: Record<string, unknown>,
  logName?: string,
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
      ? context.createFileWriter("command-log", logName).writeStream(
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

export async function inspect(context: Context, worktree?: string) {
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
  const event = EventSchema.parse({
    action,
    at: new Date().toISOString(),
    state,
  });
  const eventHandle = await context.writeResource(
    "event",
    eventName,
    event,
  );
  const stateHandle = await context.writeResource("state", "state", state);
  context.logger.info("Issue {issue} completed {action}; phase {phase}", {
    issue: context.definition.name,
    action,
    phase: state.phase,
  });
  return { dataHandles: [eventHandle, stateHandle] };
}

export function normalizeReceipt(raw: unknown, state: LifecycleState) {
  const receipt = GitReceiptSchema.parse(raw);
  if (
    receipt.issueKey !== `${state.issue.workspace}/${state.issue.itemId}` ||
    receipt.issueDigest !== state.issue.digest
  ) {
    throw new Error(
      "Merge receipt belongs to a different issue or description",
    );
  }
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
