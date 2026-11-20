import {
  closeout,
  CloseoutArgsSchema,
  offerCloseout,
} from "./_lib/issue_closeout.ts";
/** Agenda-backed issue development with local Git integration and durable evidence. */
import { z } from "npm:zod@4.6.7";
import {
  adoptWorkerWorktree,
  createDriveMethod,
  DriverSchema,
  HandoffSchema,
  ImplementArgsSchema,
} from "./_lib/issue_driver.ts";
import {
  agentMethods,
  AgentTaskSchema,
  assertNoPendingAgent,
  type IssueAgentContext,
  RecoverySchema,
} from "./_lib/issue_agents.ts";
import { compactIssue } from "./_lib/issue_compaction.ts";
import {
  type DecisionAction,
  DecisionInputSchemas,
  isDecisionAction,
  resolveDecisionArgs,
} from "./_lib/decisions.ts";
import { assertCheckPolicy } from "./_lib/check_policy.ts";
import {
  ActionSchemas,
  activeFollowUps,
  CheckSchema,
  completeAgenda,
  createState,
  fileFollowUpItem,
  followUpMarker,
  type LifecycleState,
  type MergeFailure,
  readAgenda,
  recordMergeFailure,
  refreshIssue,
  requireCurrentReviews,
  requireReviewAcceptance,
  StateSchema,
  transition,
} from "./_lib/lifecycle.ts";
import {
  type Context,
  EventSchema,
  gitOperation,
  GlobalArgsSchema,
  type Handle,
  inspect,
  loadState,
  mergeLogName,
  normalizeFailure,
  normalizeReceipt,
  saveState,
  withIssue,
} from "./_lib/runtime.ts";
import { RoundRecord } from "./_lib/review_continuity.ts";
import { handoffStateFields } from "./_lib/handoff_next.ts";
import {
  loadVerificationSettings,
  runVerification,
} from "./_lib/verification.ts";
import { WorkerSelectionSchema } from "./_lib/worker_selection.ts";

// Swamp delivers globals alongside method arguments. Strip them here; the
// pure transitions still validate their own inputs with strict schemas.
const Empty = z.object({}).strip();
const StartArgs = z.object({
  // An explicit origin, or "" when start creates or adopts the canonical one.
  workerWorktree: z.string().trim().default(""),
  newWorktree: z.boolean().default(false),
  // The workflow supplies {} when no default was selected.
  agent: z.union([
    WorkerSelectionSchema,
    z.object({}).strict().transform(() => undefined),
  ]).optional(),
})
  .strip();
// Checked only on a first start: a repeated start never consults its origin.
const StartOrigin = StartArgs.refine(
  (args) => args.newWorktree !== (args.workerWorktree !== ""),
  "Supply exactly one of workerWorktree or newWorktree",
);
const ClaimSchema = z.object({
  worktree: z.string().min(1),
  branch: z.string().optional(),
  created: z.boolean().optional(),
});
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

/** Decision actions take handoff-bound inputs; the rest take their strict transition input. */
type TransitionInput<K extends keyof typeof ActionSchemas> = K extends
  DecisionAction ? z.input<(typeof DecisionInputSchemas)[K]>
  : z.infer<(typeof ActionSchemas)[K]>;

function recordedTransition<K extends keyof typeof ActionSchemas>(
  action: K,
  description: string,
) {
  return {
    kind: "update" as const,
    description,
    arguments: isDecisionAction(action)
      ? DecisionInputSchemas[action]
      : ActionSchemas[action].strip(),
    execute: (args: TransitionInput<K>, context: Context) =>
      withIssue(context, async () => {
        const state = await mutableState(context);
        await currentIssue(state);
        let input: unknown = args;
        if (isDecisionAction(action)) {
          // Legacy explicit inputs never parse the handoff.
          const raw = await context.readResource("handoff");
          input = resolveDecisionArgs(
            action,
            args,
            state,
            () => raw == null ? undefined : HandoffSchema.parse(raw),
          );
        }
        if (
          action === "plan" || action === "plan_review_fixes" ||
          action === "implement_review_fixes"
        ) {
          // Submission-time policy for new plans; transition() replays persisted
          // plans without it. Malformed checks fall through to transition's error.
          const plan = z.object({ checks: z.array(CheckSchema) }).loose()
            .safeParse(input);
          // A repair result's checks are optional; plans always carry them.
          if (plan.success) assertCheckPolicy(plan.data.checks);
        }
        if (
          action === "review_code" || action === "review_ux" ||
          action === "ready" || action === "plan_review_fixes"
        ) {
          const commit = action === "plan_review_fixes"
            ? (state.codeReview?.commit ?? "")
            : z.object({ commit: z.string() }).parse(input).commit;
          await reviewedHead(context, state, commit);
        }
        if (action === "decide_review" || action === "approve_review_fixes") {
          await reviewedHead(context, state, state.codeReview!.commit);
        }
        return saveState(context, action, transition(state, action, input));
      }),
  };
}

/** Local behavior; the issue's state and every decision are versioned Swamp data. */
export const behavior = {
  type: "@yinz/issue-lifecycle",
  version: "2026.11.20.1",
  globalArguments: GlobalArgsSchema,
  resources: {
    rounds: {
      description:
        "Append-only assignment reservations and evidence-backed cause corrections",
      schema: z.object({ records: z.array(RoundRecord) }).strict(),
      lifetime: "infinite",
      garbageCollection: 1000,
    },
    "controller-origin": {
      description:
        "First controller binding, separately frozen from worker placement",
      schema: z.object({
        origin: StateSchema.shape.closeout.unwrap().shape.origin.optional(),
        unavailable: z.string().optional(),
        control: z.unknown().optional(),
      }).strict(),
      lifetime: "infinite",
      garbageCollection: 1000,
    },
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
        "Current pane assignment and its acknowledged typed Swamp evidence",
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
    snapshot: {
      description:
        "Lifecycle state a pane task was dispatched against, named snapshot-<sha256> by its canonical digest",
      schema: StateSchema,
      lifetime: "infinite",
      garbageCollection: 1,
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
    "review-diff": {
      description:
        "A review diff body stored once, named review-diff-<sha256> by the digest of its decoded bytes",
      contentType: "application/octet-stream",
      lifetime: "infinite",
      garbageCollection: 1,
    },
  },
  methods: {
    drive: createDriveMethod(),
    compact: {
      kind: "update",
      description:
        "Move review diff bodies and lifecycle snapshots out of an issue's records, preserving every fingerprint; idempotent",
      arguments: Empty,
      execute: (_args: unknown, context: IssueAgentContext) =>
        withIssue(
          context,
          () =>
            compactIssue(context, {
              id: () => crypto.randomUUID(),
              now: () => new Date().toISOString(),
            }),
        ),
    },
    closeout: {
      kind: "update",
      description:
        "Offer or explicitly authorize completed issue cleanup; pane closes last",
      arguments: CloseoutArgsSchema,
      execute: (args: z.infer<typeof CloseoutArgsSchema>, context: Context) =>
        withIssue(context, async () => {
          await assertNoPendingAgent(context);
          return closeout(context, CloseoutArgsSchema.parse(args));
        }),
    },
    resume_closeout: {
      kind: "update",
      description:
        "Reconcile the saved exact cleanup from another pane without repeating merge or completion",
      arguments: Empty,
      execute: (_args: unknown, context: Context) =>
        withIssue(context, async () => {
          await assertNoPendingAgent(context);
          return closeout(context, {});
        }),
    },
    ...agentMethods,
    start: {
      kind: "create",
      description:
        "Capture a pending Agenda issue, or return its existing lifecycle unchanged",
      arguments: StartArgs,
      execute: (args: z.input<typeof StartArgs>, context: Context) =>
        withIssue(context, async () => {
          const issueKey =
            `${context.globalArgs.workspace}/${context.globalArgs.itemId}`;
          const previous = await context.readResource("state");
          if (previous !== null && previous !== undefined) {
            const existing = await loadState(context);
            // A start that saved state but stopped before confirming left its
            // claim pending, and so reclaimable; confirm it now.
            if (
              existing.workerWorktree && !existing.closeout?.authorizedRequest
            ) {
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
          // Parsed after Agenda, so a non-pending item never creates a worktree.
          const parsed = StartOrigin.parse(args);
          // Claim the origin before saving state, so a worktree serves one issue.
          // The claim stays reclaimable until the saved state confirms it.
          const origin = ClaimSchema.parse(
            await gitOperation(
              context,
              parsed.newWorktree
                ? {
                  action: "claim-new",
                  itemId: context.globalArgs.itemId,
                  issueKey,
                }
                : {
                  action: "claim",
                  worktree: parsed.workerWorktree,
                  issueKey,
                },
            ),
          );
          if (origin.created) {
            context.logger.info(
              "Created {worktree} on {branch} from local main",
              origin,
            );
          }
          const saved = await saveState(
            context,
            "start",
            createState(issue, origin.worktree, parsed.agent),
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
      arguments: ImplementArgsSchema.strip(),
      execute: (
        args: z.infer<typeof ImplementArgsSchema>,
        context: Context,
      ) =>
        withIssue(context, async () => {
          const state = await mutableState(context);
          await currentIssue(state);
          // New lifecycles adopt their existing origin. Only legacy lifecycles
          // without an implementation may still create a worktree.
          return adoptWorkerWorktree(
            context,
            state,
            ImplementArgsSchema.parse(args),
            { realPath: (path) => Deno.realPath(path), inspect, gitOperation },
          );
        }),
    },
    decide_review: recordedTransition(
      "decide_review",
      "Record the user's selected review repairs and ignored findings; selected repairs go straight to the repair worker",
    ),
    plan_review_fixes: recordedTransition(
      "plan_review_fixes",
      "Legacy: record a repair plan for an issue saved before repair planning was retired",
    ),
    approve_review_fixes: recordedTransition(
      "approve_review_fixes",
      "Legacy: approve a saved repair plan; drive repairs selected findings without it",
    ),
    review_code: recordedTransition(
      "review_code",
      "Record coverage of every plan step at a committed HEAD",
    ),
    review_ux: recordedTransition(
      "review_ux",
      "Record the separate UX assessment of the committed implementation",
    ),
    verify: {
      kind: "action",
      description:
        "Probe rebase conflicts with local main, then run the configured baseline checks and the approved plan's checks in the issue worktree at the reviewed commit, outside the merge lock",
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
            verificationSettings: (c) =>
              loadVerificationSettings(c, gitOperation),
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
        "Return to implementation after inspecting any previous merge attempt; a recorded failed verification or merge attempt becomes lastFailure",
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
          // A landed receipt remains a historical fact, even without new review fields.
          const existing = await gitOperation(context, {
            action: "receipt",
            attemptId: state.attempt!.id,
          });
          if (
            z.object({ status: z.string() }).parse(existing).status === "merged"
          ) {
            return saveState(
              context,
              "reconcile_merge",
              transition(state, "merged", {
                receipt: normalizeReceipt(existing, state),
              }),
            );
          }
          requireCurrentReviews(state, state.attempt!.originalHead);
          requireReviewAcceptance(state);
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
          if (state.phase === "done") return { dataHandles: [] };
          // Validate the outcome before any Agenda write.
          transition(state, "done", args);
          const filed: Handle[] = [];
          for (const followUp of activeFollowUps(state)) {
            if (followUp.agendaItem?.status === "created") continue;
            const marker = followUp.agendaItem?.marker ??
              followUpMarker(followUp);
            if (!followUp.agendaItem) {
              // Record intent first: a retry searches for the marker before adding.
              followUp.agendaItem = { marker, status: "creating" };
              filed.push(
                ...(await saveState(context, "file_follow_up", state))
                  .dataHandles,
              );
            }
            const itemId = await fileFollowUpItem(
              state.issue,
              followUp,
              marker,
            );
            followUp.agendaItem = { marker, status: "created", itemId };
            filed.push(
              ...(await saveState(context, "file_follow_up", state))
                .dataHandles,
            );
          }
          const next = transition(state, "done", args);
          await completeAgenda(state.issue);
          const saved = await saveState(context, "done", next);
          try {
            const offered = await offerCloseout(context, next);
            return {
              dataHandles: [
                ...new Map(
                  [...filed, ...saved.dataHandles, ...offered.dataHandles].map(
                    (handle) => [handle.name, handle],
                  ),
                ).values(),
              ],
            };
          } catch (error) {
            // Optional cleanup cannot undo successful Agenda/outcome completion.
            const reason =
              `The issue is merged and its outcome is recorded. Optional closeout is unavailable; remaining resources and recorded cleanup progress are retained: ${error}`;
            const handoff = await context.writeResource("handoff", "handoff", {
              id: crypto.randomUUID(),
              status: "current",
              decision: "complete",
              reason,
              createdAt: new Date().toISOString(),
              ...handoffStateFields(StateSchema.parse(next)),
              presentation: reason,
            });
            return { dataHandles: [...filed, ...saved.dataHandles, handoff] };
          }
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
