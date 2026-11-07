/** Agenda-backed issue development with local Git integration and durable evidence. */
import { z } from "npm:zod@4.6.7";
import {
  createDriveMethod,
  DriverSchema,
  HandoffSchema,
} from "./_lib/issue_driver.ts";
import {
  agentMethods,
  AgentTaskSchema,
  assertNoPendingAgent,
  RecoverySchema,
} from "./_lib/issue_agents.ts";
import {
  ActionSchemas,
  completeAgenda,
  createState,
  type LifecycleState,
  type MergeFailure,
  readAgenda,
  recordMergeFailure,
  refreshIssue,
  StateSchema,
  transition,
} from "./_lib/lifecycle.ts";
import {
  type Context,
  EventSchema,
  gitOperation,
  GlobalArgsSchema,
  inspect,
  InspectionSchema,
  loadState,
  mergeLogName,
  normalizeFailure,
  normalizeReceipt,
  saveState,
  withIssue,
} from "./_lib/runtime.ts";
import { runVerification } from "./_lib/verification.ts";

// Swamp delivers globals alongside method arguments. Strip them here; the
// pure transitions still validate their own inputs with strict schemas.
const Empty = z.object({}).strip();
const StartArgs = z.object({ workerWorktree: z.string().trim().min(1) })
  .strip();
const ClaimSchema = z.object({ worktree: z.string().min(1) });
const IntegrateArgs = z.object({ skipCiRequest: z.string().trim().default("") })
  .strip();

function mutableState(context: Context) {
  return assertNoPendingAgent(context);
}

async function currentIssue(state: LifecycleState, allowCompleted = false) {
  const issue = await readAgenda(
    state.issue.workspace,
    state.issue.itemId,
    allowCompleted,
  );
  if (issue.digest !== state.issue.digest) {
    throw new Error(
      "Agenda description changed; refresh the issue and review its revised plan",
    );
  }
  return issue;
}

async function reviewedHead(
  context: Context,
  state: LifecycleState,
  commit: string,
) {
  if (!state.implementation) throw new Error("Start implementation first");
  const repo = await inspect(context, state.implementation.worktree);
  if (
    !repo.clean || repo.branch !== state.implementation.branch ||
    repo.head !== commit
  ) {
    throw new Error(
      "Review and authorization must reference the clean implementation worktree's current commit",
    );
  }
}

function recordedTransition<K extends keyof typeof ActionSchemas>(
  action: K,
  description: string,
) {
  return {
    kind: "update" as const,
    description,
    arguments: ActionSchemas[action].strip(),
    execute: (args: z.infer<(typeof ActionSchemas)[K]>, context: Context) =>
      withIssue(context, async () => {
        const state = await mutableState(context);
        await currentIssue(state);
        if (action === "review_code" || action === "ready") {
          const { commit } = z.object({ commit: z.string() }).parse(args);
          await reviewedHead(context, state, commit);
        }
        return saveState(context, action, transition(state, action, args));
      }),
  };
}

/** Local behavior; the issue's state and every decision are versioned Swamp data. */
export const behavior = {
  type: "@yinz/issue-lifecycle",
  version: "2026.11.07.2",
  globalArguments: GlobalArgsSchema,
  resources: {
    driver: {
      description:
        "Selected pane settings and the foreground driver's current assignment",
      schema: DriverSchema,
      lifetime: "infinite",
      garbageCollection: 1000,
    },
    handoff: {
      description:
        "Human decision or blocker with its exact lifecycle and worker evidence",
      schema: HandoffSchema,
      lifetime: "infinite",
      garbageCollection: 1000,
    },
    recovery: {
      description:
        "Requested rollback, previous state and assignment, and resumable replacement intent",
      schema: RecoverySchema,
      lifetime: "infinite",
      garbageCollection: 1000,
    },
    "agent-task": {
      description:
        "Current pane assignment and its imported lifecycle evidence",
      schema: AgentTaskSchema,
      lifetime: "infinite",
      garbageCollection: 1000,
    },
    state: {
      description: "Current issue lifecycle",
      schema: StateSchema,
      lifetime: "infinite",
      garbageCollection: 1000,
    },
    event: {
      description: "Immutable decision and state snapshot",
      schema: EventSchema,
      lifetime: "infinite",
      garbageCollection: 1,
    },
  },
  files: {
    "command-log": {
      description: "Git and CI output for a merge attempt",
      contentType: "text/plain",
      lifetime: "infinite",
      garbageCollection: 100,
      streaming: true,
    },
    "check-log": {
      description:
        "Output of one deterministic plan check in a verification run",
      contentType: "text/plain",
      lifetime: "infinite",
      garbageCollection: 1000,
      streaming: true,
    },
  },
  methods: {
    drive: createDriveMethod(),
    ...agentMethods,
    start: {
      kind: "create",
      description:
        "Capture a pending Agenda issue, or return its existing lifecycle unchanged",
      arguments: StartArgs,
      execute: (args: z.infer<typeof StartArgs>, context: Context) =>
        withIssue(context, async () => {
          const issueKey =
            `${context.globalArgs.workspace}/${context.globalArgs.itemId}`;
          const previous = await context.readResource("state");
          if (previous !== null && previous !== undefined) {
            const existing = await loadState(context);
            // A start that saved state but stopped before confirming left its
            // claim pending, and so reclaimable; confirm it now.
            if (existing.workerWorktree) {
              await gitOperation(context, {
                action: "confirm",
                worktree: existing.workerWorktree,
                issueKey,
              });
            }
            // Repeated start never replaces the origin; say which one workers keep.
            context.logger.info(
              "Resuming existing issue {issue}; workers keep {worktree}",
              {
                issue: context.definition.name,
                worktree: existing.workerWorktree ?? "the legacy selection",
              },
            );
            return { dataHandles: [] };
          }
          const issue = await readAgenda(
            context.globalArgs.workspace,
            context.globalArgs.itemId,
          );
          // Claim the origin before saving state, so a worktree serves one issue.
          // The claim stays reclaimable until the saved state confirms it.
          const origin = ClaimSchema.parse(
            await gitOperation(context, {
              action: "claim",
              worktree: StartArgs.parse(args).workerWorktree,
              issueKey,
            }),
          );
          const saved = await saveState(
            context,
            "start",
            createState(issue, origin.worktree),
          );
          await gitOperation(context, {
            action: "confirm",
            worktree: origin.worktree,
            issueKey,
          });
          return saved;
        }),
    },
    triage: recordedTransition(
      "triage",
      "Record classification and findings from the codebase",
    ),
    plan: recordedTransition(
      "plan",
      "Write a new plan version and invalidate old review and approval",
    ),
    review_plan: recordedTransition(
      "review_plan",
      "Record adversarial findings for the current plan",
    ),
    adjudicate_plan: recordedTransition(
      "adjudicate_plan",
      "Record evidence-based controller dispositions for the exact current plan review",
    ),
    approve: recordedTransition(
      "approve",
      "Record explicit authorization of the reviewed plan",
    ),
    implement: {
      kind: "update",
      description:
        "Adopt the recorded worker worktree for implementation (legacy issues may create one)",
      arguments: ActionSchemas.implement.strip(),
      execute: (
        args: z.infer<typeof ActionSchemas.implement>,
        context: Context,
      ) =>
        withIssue(context, async () => {
          const state = await mutableState(context);
          await currentIssue(state);
          // New lifecycles adopt their existing origin. Only legacy lifecycles
          // without an implementation may still create a worktree.
          const implementation = state.workerWorktree || state.implementation
            ? { ...args, worktree: await Deno.realPath(args.worktree) }
            : args;
          // Validate the phase and approval before creating a branch or worktree.
          transition(state, "implement", implementation);
          const repo = InspectionSchema.parse(
            await gitOperation(context, {
              action: "prepare",
              ...implementation,
              issueKey: `${state.issue.workspace}/${state.issue.itemId}`,
            }),
          );
          return saveState(
            context,
            "implement",
            transition(state, "implement", {
              worktree: repo.worktree,
              branch: repo.branch,
            }),
          );
        }),
    },
    review_code: recordedTransition(
      "review_code",
      "Record coverage of every plan step at a committed HEAD",
    ),
    verify: {
      kind: "action",
      description:
        "Run the approved plan's checks in the issue worktree at the reviewed commit, outside the merge lock",
      arguments: Empty,
      execute: (_args: Record<string, never>, context: Context) =>
        withIssue(context, async () => {
          const state = await mutableState(context);
          await currentIssue(state);
          if (!state.codeReview) {
            throw new Error("Record a code review before verifying");
          }
          await reviewedHead(context, state, state.codeReview.commit);
          return runVerification(context, state, {
            gitOperation,
            id: () => crypto.randomUUID(),
            now: () => new Date().toISOString(),
          });
        }),
    },
    ready: recordedTransition(
      "ready",
      "Authorize the reviewed commit for rebase, final CI and merge",
    ),
    refresh: {
      kind: "update",
      description:
        "Refresh changed Agenda requirements and invalidate decisions based on the old description",
      arguments: Empty,
      execute: (_args: Record<string, never>, context: Context) =>
        withIssue(context, async () => {
          const state = await mutableState(context);
          const issue = await readAgenda(
            state.issue.workspace,
            state.issue.itemId,
          );
          return saveState(context, "refresh", refreshIssue(state, issue));
        }),
    },
    repair: {
      kind: "update",
      description:
        "Return to implementation after inspecting any previous merge attempt",
      arguments: ActionSchemas.repair.strip(),
      execute: (args: z.infer<typeof ActionSchemas.repair>, context: Context) =>
        withIssue(context, async () => {
          const state = await mutableState(context);
          if (state.attempt) {
            const raw = await gitOperation(context, {
              action: "receipt",
              attemptId: state.attempt.id,
            });
            const { status } = z.object({ status: z.string() }).parse(raw);
            if (status === "merged") {
              return saveState(
                context,
                "reconcile_merge",
                transition(state, "merged", {
                  receipt: normalizeReceipt(raw, state),
                }),
              );
            }
          }
          return saveState(
            context,
            "repair",
            transition(state, "repair", args),
          );
        }),
    },
    integrate: {
      kind: "action",
      description:
        "Hold the repository merge lock across rebase, full CI (unless explicitly waived), and local fast-forward",
      arguments: IntegrateArgs,
      execute: (args: z.infer<typeof IntegrateArgs>, context: Context) =>
        withIssue(context, async () => {
          let state = await mutableState(context);
          if (state.phase === "merged" || state.phase === "done") {
            if (!state.receipt) throw new Error("Merged issue has no receipt");
            const raw = await gitOperation(context, {
              action: "receipt",
              attemptId: state.receipt.attemptId,
            });
            normalizeReceipt(raw, state);
            return { dataHandles: [] };
          }
          if (!state.attempt) {
            await currentIssue(state);
            const commit = state.mergeAuthorization?.commit;
            if (!commit) {
              throw new Error(
                "Authorize the reviewed commit with ready before merging",
              );
            }
            await reviewedHead(context, state, commit);
            state = transition(state, "begin_merge", {
              attempt: {
                id: crypto.randomUUID(),
                originalHead: commit,
                startedAt: new Date().toISOString(),
                ...(args.skipCiRequest
                  ? { skipCiRequest: args.skipCiRequest }
                  : {}),
              },
            });
            // Persist intent before invoking Git: recovery never guesses an attempt.
            await saveState(context, "begin_merge", state);
          } else if (
            args.skipCiRequest &&
            args.skipCiRequest !== state.attempt.skipCiRequest
          ) {
            throw new Error(
              "Cannot change the CI exception on an existing attempt; repair and authorize a new attempt",
            );
          }
          const attempt = state.attempt!;
          const implementation = state.implementation!;
          try {
            const raw = await gitOperation(context, {
              action: "integrate",
              worktree: implementation.worktree,
              expectedBranch: implementation.branch,
              expectedHead: attempt.originalHead,
              attemptId: attempt.id,
              issueKey: `${state.issue.workspace}/${state.issue.itemId}`,
              issueDigest: state.issue.digest,
              agendaWorkspace: state.issue.workspace,
              agendaItemId: state.issue.itemId,
              ...(attempt.skipCiRequest
                ? { skipCiRequest: attempt.skipCiRequest }
                : {}),
            }, mergeLogName(attempt.id));
            const receipt = normalizeReceipt(raw, state);
            return await saveState(
              context,
              "merged",
              transition(state, "merged", { receipt }),
            );
          } catch (error) {
            // Keep intent and failure evidence. repair checks the durable Git receipt
            // before clearing them, including a merge interrupted before Swamp saved it.
            const message = error instanceof Error
              ? error.message
              : String(error);
            const failure = await failedReceipt(context, state, attempt.id);
            await saveState(
              context,
              "merge_failed",
              failure
                ? recordMergeFailure(state, failure)
                : { ...state, lastFailure: message },
            );
            throw error;
          }
        }),
    },
    summarize: {
      kind: "update",
      description:
        "Check the delivered outcome and complete the original Agenda task after a confirmed merge",
      arguments: ActionSchemas.done.strip(),
      execute: (args: z.infer<typeof ActionSchemas.done>, context: Context) =>
        withIssue(context, async () => {
          const state = await mutableState(context);
          if (!state.receipt) {
            throw new Error("Completion requires a local merge receipt");
          }
          await gitOperation(context, {
            action: "receipt",
            attemptId: state.receipt.attemptId,
          }).then((raw) => normalizeReceipt(raw, state));
          const next = state.phase === "done"
            ? state
            : transition(state, "done", args);
          await completeAgenda(state.issue);
          return saveState(context, "done", next);
        }),
    },
  },
};

/**
 * The active attempt's failed Git receipt, or undefined when Git recorded no
 * failure (a lock timeout before the attempt began) or cannot be read now.
 * It reads without the merge lock, which the next queued merge may hold.
 */
async function failedReceipt(
  context: Context,
  state: LifecycleState,
  attemptId: string,
): Promise<MergeFailure | undefined> {
  try {
    const raw = await gitOperation(context, {
      action: "receipt",
      attemptId,
      failedOnly: true,
    });
    const { status } = z.object({ status: z.string() }).loose().parse(raw);
    return status === "failed" ? normalizeFailure(raw, state) : undefined;
  } catch (error) {
    context.logger.info(
      "Cannot read the failed merge receipt for {issue}: {error}",
      {
        issue: context.definition.name,
        error: error instanceof Error ? error.message : String(error),
      },
    );
    return undefined;
  }
}
