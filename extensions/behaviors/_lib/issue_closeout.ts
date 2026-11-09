/** Optional post-completion effects. The caller holds the existing issue lock. */
import { z } from "npm:zod@4.6.7";
import {
  CloseoutFenceSchema,
  CloseoutGitResultSchema,
  CloseoutOriginSchema,
  CloseoutTargetSchema,
  type LifecycleState,
  requireCompletedAgenda,
  StateSchema,
} from "./lifecycle.ts";
import {
  type Context,
  gitOperation,
  GitOperationError,
  loadState,
  normalizeReceipt,
  saveState,
} from "./runtime.ts";
import { yinzCommand, type YinzRunner } from "./yinz_agent.ts";

export const CloseoutArgsSchema = z.object({
  handoffId: z.uuid().optional(),
  decision: z.enum(["accept", "decline"]).optional(),
  request: z.string().trim().min(1).optional(),
  bind: CloseoutOriginSchema.optional(),
}).strip();
export interface CloseoutDependencies {
  git: typeof gitOperation;
  run: YinzRunner;
  completed: typeof requireCompletedAgenda;
  id: () => string;
}
export const closeoutDependencies: CloseoutDependencies = {
  git: gitOperation,
  run: yinzCommand,
  completed: requireCompletedAgenda,
  id: () => crypto.randomUUID(),
};
const ReplySchema = z.object({
  outcome: z.string(),
  server_instance: z.string().optional(),
  output: z.string().nullable().optional(),
  message: z.string().nullable().optional(),
  closeout_target: CloseoutFenceSchema.optional(),
}).loose();
type Origin = z.infer<typeof CloseoutOriginSchema>;
async function command(
  context: Context,
  origin: { session: string },
  args: string[],
  deps: CloseoutDependencies,
) {
  const result = await deps.run(
    ["--session", origin.session, "agent", ...args, "--json"],
    undefined,
    context.signal,
  );
  // Nonzero structured target-not-found is still an observation. A lost CLI reply is not.
  try {
    return ReplySchema.parse(JSON.parse(result.stdout));
  } catch (cause) {
    throw new Error(
      `Cannot observe controller: ${result.stderr || result.stdout}`,
      { cause },
    );
  }
}
async function snapshot(
  context: Context,
  origin: { session: string; paneId: string },
  deps: CloseoutDependencies,
) {
  const support = await command(context, origin, ["closeout-support"], deps);
  if (
    support.outcome !== "ok" ||
    support.output !== "agent-controller-closeout-v1" ||
    !support.server_instance
  ) {
    throw new Error(
      "Server lacks guarded controller closeout support; retain resources or upgrade and bind the exact original controller",
    );
  }
  return command(context, origin, ["closeout-snapshot", origin.paneId], deps);
}
/** First successful control binding is frozen independently of future placement settings. */
export async function captureCloseoutOrigin(
  context: Context,
  control: { session: string; paneId: string },
  deps = closeoutDependencies,
) {
  if (await context.readResource("controller-origin") != null) return;
  let record: Record<string, unknown>;
  try {
    const observed = await snapshot(context, control, deps);
    const fence = CloseoutFenceSchema.parse(observed.closeout_target);
    if (observed.outcome !== "ok" || fence.pane_id !== control.paneId) {
      throw new Error("Original controller unavailable");
    }
    record = {
      origin: {
        ...control,
        server_instance: fence.server_instance,
        provider: fence.provider,
        conversation: fence.conversation,
      },
    };
  } catch (error) {
    record = { unavailable: String(error), control };
  }
  await context.writeResource("controller-origin", "controller-origin", record);
}
function binding(state: LifecycleState) {
  if (state.phase !== "done" || !state.outcome?.outcomeMet || !state.receipt) {
    throw new Error(
      "Closeout requires a confirmed merge, recorded outcome and completed Agenda task",
    );
  }
  return {
    issueKey: `${state.issue.workspace}/${state.issue.itemId}`,
    issueDigest: state.issue.digest,
    attemptId: state.receipt.attemptId,
  };
}
function sameOrigin(
  origin: Origin,
  fence: z.infer<typeof CloseoutFenceSchema>,
) {
  return origin.paneId === fence.pane_id &&
    origin.server_instance === fence.server_instance &&
    origin.provider === fence.provider &&
    origin.conversation === fence.conversation;
}
async function validate(
  context: Context,
  state: LifecycleState,
  deps: CloseoutDependencies,
) {
  const request = binding(state);
  await deps.completed(state.issue);
  normalizeReceipt(
    await deps.git(context, {
      action: "receipt",
      attemptId: request.attemptId,
    }),
    state,
  );
  return request;
}
/** Check all visible panes, including non-agent shells. Unknown cwd/identity blocks deletion. */
async function otherUsers(
  context: Context,
  state: LifecycleState,
  origin: Origin,
  deps: CloseoutDependencies,
) {
  const taskSession = z.object({
    config: z.object({ session: z.string().min(1) }).loose(),
  }).loose();
  const driver = z.object({
    agent: z.object({ session: z.string().min(1) }).loose().optional(),
    implementation: taskSession.optional(),
    automaticRevision: z.object({ assignment: taskSession }).loose().optional(),
    revisionAuthorization: z.object({ assignment: taskSession }).loose()
      .optional(),
  }).loose().parse(await context.readResource("driver") ?? {});
  const task = taskSession.nullable().parse(
    await context.readResource("agent-task"),
  );
  const recovery = z.object({
    task: taskSession.optional(),
    replacement: taskSession.optional(),
  }).loose().parse(await context.readResource("recovery") ?? {});
  const sessions = new Set([
    origin.session,
    driver.agent?.session,
    task?.config.session,
    driver.implementation?.config.session,
    driver.automaticRevision?.assignment.config.session,
    driver.revisionAuthorization?.assignment.config.session,
    recovery.task?.config.session,
    recovery.replacement?.config.session,
  ].filter((session): session is string => session !== undefined));
  const target = state.closeout!.target.worktree;
  let canonical = target;
  try {
    canonical = await Deno.realPath(target);
  } catch (error) {
    if (!(error instanceof Deno.errors.NotFound)) throw error;
  }
  for (const session of sessions) {
    const result = await deps.run(
      ["--session", session, "action", "list-panes", "--json"],
      undefined,
      context.signal,
    );
    if (result.code !== 0) {
      throw new Error(
        `Cannot inspect session ${session} users: ${result.stderr}`,
      );
    }
    const panes = z.array(
      z.object({
        id: z.number().int(),
        is_plugin: z.boolean(),
        pane_cwd: z.string().nullable().optional(),
      }).loose(),
    ).parse(JSON.parse(result.stdout));
    for (const pane of panes) {
      if (
        pane.is_plugin ||
        (session === origin.session && `terminal_${pane.id}` === origin.paneId)
      ) continue;
      if (!pane.pane_cwd) {
        throw new Error(
          `Pane ${session}/terminal_${pane.id} has unknown working directory; inspect or close it before closeout`,
        );
      }
      const cwd = await Deno.realPath(pane.pane_cwd);
      if (cwd === canonical || cwd.startsWith(`${canonical}/`)) {
        throw new Error(
          `Pane ${session}/terminal_${pane.id} still uses ${target}; retain resources until it leaves`,
        );
      }
    }
  }
}
export async function offerCloseout(
  context: Context,
  state: LifecycleState,
  deps = closeoutDependencies,
  bind?: Origin,
) {
  await validate(context, state, deps);
  if (state.closeout && state.closeout.decision !== "invalidated") {
    return { state, dataHandles: [] };
  }
  const raw = await context.readResource("controller-origin");
  const saved = z.object({ origin: CloseoutOriginSchema.optional() }).loose()
    .parse(raw ?? {});
  const remembered = saved.origin ?? state.closeout?.origin;
  const origin = bind ?? remembered;
  if (!origin) {
    throw new Error(
      "Original controller identity unavailable. Retain resources, or call closeout with bind containing the exact session, paneId, server_instance, provider and conversation to create an offer before consenting",
    );
  }
  if (remembered && JSON.stringify(remembered) !== JSON.stringify(origin)) {
    throw new Error("Binding cannot replace the original controller");
  }
  const observed = await snapshot(context, origin, deps);
  const fence = CloseoutFenceSchema.parse(observed.closeout_target);
  if (observed.outcome !== "ok" || !sameOrigin(origin, fence)) {
    throw new Error(
      "Original controller incarnation or conversation changed; retain resources",
    );
  }
  const target = CloseoutTargetSchema.parse(
    await deps.git(context, { action: "closeout-preview", ...binding(state) }),
  );
  if (state.closeout) (state.closeoutHistory ??= []).push(state.closeout);
  state.closeout = {
    id: deps.id(),
    offerId: deps.id(),
    issueDigest: state.issue.digest,
    attemptId: state.receipt!.attemptId,
    ...(state.rollbackId ? { rollbackId: state.rollbackId } : {}),
    origin,
    target,
    fence,
    decision: "offered",
    git: "pending",
    pane: "pending",
  };
  const savedState = await saveState(context, "offer_closeout", state);
  const handoff = await publishCloseoutOffer(context, state);
  return { state, dataHandles: [...savedState.dataHandles, handoff] };
}
function assertCurrent(state: LifecycleState) {
  const closeout = state.closeout;
  if (
    !closeout || closeout.decision === "invalidated" ||
    closeout.issueDigest !== state.issue.digest ||
    closeout.attemptId !== state.receipt?.attemptId ||
    closeout.rollbackId !== state.rollbackId
  ) {
    throw new Error(
      "Closeout offer no longer matches the issue and merge identity",
    );
  }
  return closeout;
}
export function closeoutPresentation(state: LifecycleState) {
  const c = state.closeout!;
  return `Offer ${c.offerId}: delete ${c.target.worktree} including its build outputs, delete local branch ${c.target.branch}, then close the original controller ${c.origin.session}/${c.origin.paneId}. Accepting ends that conversation. Decline retains all resources and the completed outcome. Call closeout with this handoffId, decision accept/decline and the actual user request. Git: ${c.git}${
    c.gitResult
      ? ` (worktree ${c.gitResult.worktree}, branch ${c.gitResult.branch})`
      : ""
  }; pane: ${c.pane}.${
    c.failure
      ? ` ${c.failure.stage} failed: ${c.failure.message}. Use resume-closeout from another pane.`
      : ""
  }`;
}
function publishCloseoutOffer(context: Context, state: LifecycleState) {
  const c = state.closeout!;
  return context.writeResource("handoff", "handoff", {
    id: c.offerId,
    status: "current",
    decision: "authorize_closeout",
    reason: closeoutPresentation(state),
    createdAt: new Date().toISOString(),
    state: StateSchema.parse(state),
    presentation: closeoutPresentation(state),
  });
}
export async function closeout(
  context: Context,
  args: z.infer<typeof CloseoutArgsSchema>,
  deps = closeoutDependencies,
) {
  const state = await loadState(context);
  await validate(context, state, deps);
  if (!state.closeout || state.closeout.decision === "invalidated") {
    if (args.decision) {
      throw new Error(
        "Create and present an exact closeout offer before recording consent",
      );
    }
    const offered = await offerCloseout(context, state, deps, args.bind);
    return { dataHandles: offered.dataHandles };
  }
  const c = assertCurrent(state);
  if (args.bind) {
    throw new Error("An existing offer freezes the original controller");
  }
  if (args.decision) {
    if (!args.request || args.handoffId !== c.offerId) {
      throw new Error(
        "Closeout consent requires the current exact handoffId and actual user request",
      );
    }
    if (
      c.decision === "accepted" && c.git === "complete" &&
      ["closed", "absent"].includes(c.pane)
    ) return { dataHandles: [] };
    if (c.decision === "declined") {
      if (args.decision !== "decline") {
        throw new Error(
          "Closeout was declined; retention is terminal for this offer",
        );
      }
      return { dataHandles: [] };
    }
    if (args.decision === "decline") {
      if (c.decision !== "offered") {
        throw new Error("Accepted closeout must reconcile its saved progress");
      }
      c.decision = "declined";
      c.request = args.request;
      return saveState(context, "decline_closeout", state);
    }
    const handoff = z.object({
      id: z.string(),
      status: z.string(),
      state: z.unknown(),
    }).loose().parse(await context.readResource("handoff"));
    if (
      handoff.id !== c.offerId || handoff.status !== "current" ||
      JSON.stringify(handoff.state) !== JSON.stringify(state)
    ) throw new Error("Closeout handoff is stale");
    const observed = await snapshot(context, c.origin, deps);
    if (observed.outcome !== "ok") {
      throw new Error(
        `Original controller changed or unavailable; retain the pane: ${
          observed.message ?? observed.outcome
        }`,
      );
    }
    const fresh = CloseoutFenceSchema.parse(observed.closeout_target);
    if (observed.outcome !== "ok" || !sameOrigin(c.origin, fresh)) {
      throw new Error("Original controller changed; retain the pane");
    }
    // New explicit consent may renew an expired turn fence, never change pane/conversation.
    if (c.git !== "complete") await otherUsers(context, state, c.origin, deps);
    c.fence = fresh;
    c.request = args.request;
    c.authorizedRequest = args.request;
    c.decision = "accepted";
    c.pane = "pending";
    delete c.failure;
    await saveState(context, "accept_closeout", state);
  }
  if (c.decision === "offered") {
    return { dataHandles: [await publishCloseoutOffer(context, state)] };
  }
  if (c.decision !== "accepted") return { dataHandles: [] };
  if (["closed", "absent"].includes(c.pane)) return { dataHandles: [] };
  if (c.pane === "expired") {
    return { dataHandles: [await publishCloseoutOffer(context, state)] };
  }
  if (c.git !== "complete") {
    try {
      await otherUsers(context, state, c.origin, deps);
      const result = CloseoutGitResultSchema.parse(
        await deps.git(context, {
          action: "closeout-cleanup",
          ...binding(state),
          cleanupId: c.id,
          target: c.target,
        }),
      );
      if (
        result.cleanupId !== c.id ||
        JSON.stringify(result.target) !== JSON.stringify(c.target)
      ) throw new Error("Git cleanup returned another target");
      if (result.worktree !== "removed" || result.branch !== "removed") {
        throw new Error("Git cleanup returned unfinished progress");
      }
      c.gitResult = result;
      c.git = "complete";
      delete c.failure;
      await saveState(context, "closeout_git_complete", state);
    } catch (error) {
      if (error instanceof GitOperationError) {
        const progress = z.object({ closeout: CloseoutGitResultSchema })
          .safeParse(error.result);
        if (
          progress.success && progress.data.closeout.cleanupId === c.id &&
          JSON.stringify(progress.data.closeout.target) ===
            JSON.stringify(c.target)
        ) c.gitResult = progress.data.closeout;
      }
      c.failure = { stage: "git", message: String(error) };
      await saveState(context, "closeout_git_failed", state);
      throw new Error(
        `Git closeout failed; pane retained. Use resume-closeout: ${error}`,
        { cause: error },
      );
    }
  }
  let observed: z.infer<typeof ReplySchema>;
  try {
    observed = await snapshot(context, c.origin, deps);
  } catch (error) {
    c.failure = {
      stage: "pane",
      message:
        `Cannot observe the original controller; retain its unconfirmed result and use resume-closeout: ${error}`,
    };
    return saveState(context, "closeout_pane_observation_failed", state);
  }
  if (observed.server_instance !== c.fence.server_instance) {
    c.pane = "expired";
  } else if (observed.outcome === "target_not_found" && c.pane === "unknown") {
    c.pane = "absent";
    return saveState(context, "closeout_pane_absent", state);
  } else if (
    observed.outcome !== "ok" ||
    JSON.stringify(observed.closeout_target) !== JSON.stringify(c.fence)
  ) {
    c.pane = "expired";
  }
  if (c.pane === "expired") {
    c.failure = {
      stage: "pane",
      message:
        "Original pane fence expired; obtain fresh explicit consent for the same controller",
    };
    c.offerId = deps.id();
    await saveState(context, "closeout_pane_expired", state);
    return { dataHandles: [await publishCloseoutOffer(context, state)] };
  }
  // Last durable write before self-close. The process may never observe its reply.
  c.pane = "unknown";
  delete c.failure;
  await saveState(context, "closeout_pane_intent", state);
  try {
    const result = await command(context, c.origin, [
      "closeout-close",
      "--fence",
      JSON.stringify(c.fence),
    ], deps);
    if (
      result.outcome !== "ok" ||
      result.server_instance !== c.fence.server_instance
    ) throw new Error(result.message ?? result.outcome);
    c.pane = "closed";
    return await saveState(context, "closeout_pane_closed", state);
  } catch (error) {
    c.failure = {
      stage: "pane",
      message:
        `Closure unconfirmed: ${error}. Recover with resume-closeout from another pane`,
    };
    return saveState(context, "closeout_pane_unknown", state);
  }
}
