import { captureCloseoutOrigin, offerCloseout } from "./issue_closeout.ts";
import { yinzCommand, type YinzRunner } from "./yinz_agent.ts";
/** Assign pane work and consume acknowledged Swamp evidence independently of chat. */
import { join } from "node:path";
import { z } from "npm:zod@4.6.7";
import { CHECK_POLICY_SUMMARY } from "./check_policy.ts";
import {
  assignmentFileContents,
  type AssignmentFiles,
  AssignmentFilesSchema,
  assignmentManifest,
  baselineView,
  publishAssignmentFiles,
} from "./assignment_files.ts";
import { AgentKindSchema } from "./worker_selection.ts";
import {
  activeReviewFix,
  codeReviewFingerprint,
  deferredPlanFindings,
  describeMergeFailure,
  implementationReviewFingerprint,
  initialPlanning,
  type LifecycleState,
  planningGeneration,
  priorImplementationFindings,
  readAgenda,
  recordFeedback,
  refreshIssue,
  reopenAgenda,
  reviewScope,
  RollbackInputSchema,
  rollbackState,
  stateDigest,
  StateSchema,
  transition,
  UserRequestSchema,
} from "./lifecycle.ts";
import {
  assignmentCore,
  AssignmentSchema,
  canonical,
  evidenceDigest,
  type EvidenceReceipt,
  EvidenceReceiptSchema,
  EvidenceTargetSchema,
  hasContinuityContract,
  ImplementationResult,
  reviewPacket,
  RoleSchema,
  shellQuote,
  submissionInstructions,
  SubmissionSchema,
  SWAMP_LOCAL,
  validateReceipt,
} from "./issue_evidence.ts";
export { ImplementationResult } from "./issue_evidence.ts";
import {
  type Context,
  gitOperation,
  inspect,
  invalidateHandoff,
  loadState,
  normalizeReceipt,
  saveState,
  submissionPreflight,
  withIssue,
} from "./runtime.ts";
import {
  loadVerificationSettings,
  type VerificationSettings,
} from "./verification.ts";
import {
  AgentConfigSchema,
  AgentObservationSchema,
  AgentResultSchema,
  AgentStateSchema,
  assertContinuationEligible,
  completedWorkerProblems,
  ControlIdentitySchema,
  promptDigest,
  rejectedLaunchProblems,
  reportedBackground,
  terminalFailureProblems,
  unidentifiedBackground,
  WORKER_NOT_STOPPED,
} from "./yinz_agent.ts";

import {
  baselineIdentity,
  ContinuityContext,
  GitBaseline,
  RoundRecord,
} from "./review_continuity.ts";
import {
  baselineBytes,
  compactBaseline,
  type DiffStore,
  swampDiffStore,
} from "./review_diffs.ts";

const Role = RoleSchema;
export const AgentInputSchema = z.object({
  role: Role,
  request: UserRequestSchema.optional(),
  session: z.string().trim().min(1),
  // Optional while decoding saved settings and tasks from older lifecycles.
  control: ControlIdentitySchema.optional(),
  kind: AgentKindSchema,
  model: z.string().trim().min(1).optional(),
  effort: z.string().trim().min(1).optional(),
  permissionMode: z.string().trim().min(1).optional(),
  startupTimeoutSeconds: z.number().int().positive().max(3600).default(120),
  timeoutSeconds: z.number().int().positive().max(86400).default(600),
}).strict();
const Input = AgentInputSchema;

const DiscardBasisSchema = z.enum([
  "terminal_failure",
  "rejected_launch",
  "completed_unusable",
]);
const StopEvidenceSchema = z.enum([
  "confirmed_failure",
  "launch_receipt",
  "rejected_response",
  "fresh_idle",
  "attested",
]);
/**
 * Caller input for discard-agent. `confirmedStopped` attests only that a
 * completed worker has stopped, and counts only when the behavior's own
 * confirm_stopped observation is impossible; caller-supplied
 * basis or stop evidence is stripped and never persisted.
 */
const DiscardArgs = z.object({
  request: UserRequestSchema,
  confirmedStopped: z.literal(true).optional(),
}).strip();

const Digest = z.string().regex(/^[0-9a-f]{64}$/);
/**
 * An accepted receipt as a task keeps it: the resulting state is identified by
 * digest. Receipts saved before snapshots moved out of tasks embed `next`.
 */
export const TaskEvidenceSchema = EvidenceReceiptSchema.extend({
  next: StateSchema.optional(),
  nextDigest: Digest.optional(),
}).refine(
  (receipt) =>
    (receipt.next === undefined) !== (receipt.nextDigest === undefined),
  "A task receipt identifies its resulting state exactly once",
);
export const AgentTaskSchema = z.object({
  id: z.uuid(),
  behaviorName: z.string(),
  role: Role,
  config: AgentConfigSchema,
  prompt: z.string().min(1),
  promptDigest: z.string().regex(/^[0-9a-f]{64}$/),
  request: z.string().min(1).optional(),
  // 3 adds private assignment files; tasks saved as 1 or 2 replay unchanged.
  contractVersion: z.literal([2, 3]).optional(),
  assignmentFiles: AssignmentFilesSchema.optional(),
  continuity: ContinuityContext.optional(),
  round: RoundRecord.optional(),
  // The lifecycle state the task was dispatched against. New tasks keep only
  // its canonical digest; the state itself is the `snapshot-<digest>` resource.
  // Tasks saved before snapshots moved out keep `before` (and `next`) inline.
  before: StateSchema.optional(),
  beforeDigest: Digest.optional(),
  next: StateSchema.optional(),
  // The imported state, by digest; set once the accepted receipt is applying.
  nextDigest: Digest.optional(),
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
  // basis and stopEvidence are optional so records saved before they existed
  // still parse; the behavior derives both and never accepts them as input.
  discard: z.object({
    request: UserRequestSchema,
    confirmedStopped: z.literal(true).optional(),
    basis: DiscardBasisSchema.optional(),
    stopEvidence: StopEvidenceSchema.optional(),
  }).strict().optional(),
  // Operator-authorized import of accepted evidence whose pane observation never
  // completed. It supersedes that observation; the pane is neither prompted nor closed.
  acceptance: z.object({
    request: UserRequestSchema,
    reason: z.string().min(1),
    acceptedAt: z.iso.datetime(),
    childPhase: z.string().min(1),
    backgroundTasks: z.array(z.string()),
  }).strict().optional(),
  message: z.string().optional(),
  lastError: z.string().optional(),
  cleanup: z.enum(["pending", "done"]).optional(),
  submission: EvidenceTargetSchema.optional(),
  evidence: TaskEvidenceSchema.optional(),
  continuation: z.object({
    id: z.uuid(),
    request: UserRequestSchema,
    prompt: z.string().min(1),
  }).strict().optional(),
}).strict().refine(
  (task) => task.before !== undefined || task.beforeDigest !== undefined,
  "A task identifies the lifecycle state it was dispatched against",
);
type Task = z.infer<typeof AgentTaskSchema>;
type State = LifecycleState;
/** A task with its dispatch snapshot loaded. */
export type HydratedTask = Task & { before: State };

/** The saved lifecycle snapshot a task names by digest. */
export function snapshotName(digest: string): string {
  return `snapshot-${digest}`;
}
/** Saves a dispatch snapshot once under its canonical digest. */
export async function saveSnapshot(
  context: Pick<Context, "writeResource">,
  state: State,
): Promise<string> {
  const parsed = StateSchema.parse(state);
  const digest = stateDigest(parsed);
  await context.writeResource("snapshot", snapshotName(digest), parsed);
  return digest;
}
export async function loadSnapshot(
  context: Pick<Context, "readResource">,
  digest: string,
): Promise<State> {
  const raw = await context.readResource(snapshotName(digest));
  if (raw == null) {
    throw new Error(
      `Lifecycle snapshot ${digest} is missing; inspect the task before recovery`,
    );
  }
  const state = StateSchema.parse(raw);
  if (stateDigest(state) !== digest) {
    throw new Error(`Lifecycle snapshot ${digest} does not match its digest`);
  }
  return state;
}
/** The digest of the state a task was dispatched against. */
export function taskBeforeDigest(task: Task): string {
  return task.beforeDigest ?? stateDigest(task.before!);
}
/** The digest of the state a task imported, once known. */
export function taskNextDigest(task: Task): string | undefined {
  return task.nextDigest ?? (task.next ? stateDigest(task.next) : undefined);
}
/** The task with its dispatch snapshot, loaded by digest when not inline. */
export async function withBefore(
  context: Pick<Context, "readResource">,
  task: Task,
): Promise<HydratedTask> {
  if (task.before) return task as HydratedTask;
  return { ...task, before: await loadSnapshot(context, task.beforeDigest!) };
}
/** A receipt without its embedded resulting state. */
export function taskReceipt(receipt: z.infer<typeof TaskEvidenceSchema>) {
  if (receipt.next === undefined) return receipt;
  const { next, ...rest } = receipt;
  return { ...rest, nextDigest: stateDigest(next) };
}
/**
 * What a task persists anywhere (agent-task, driver, recovery, handoff): a new
 * task names its snapshots by digest; a legacy task keeps its inline states.
 */
export function persistedTask(task: Task): Task {
  const { before, next, ...rest } = task;
  return AgentTaskSchema.parse({
    ...rest,
    ...(task.beforeDigest ? {} : { before }),
    ...(next === undefined || task.nextDigest ? {} : { next }),
  });
}
/** The issue and plan version a task was dispatched for. */
async function taskBasis(context: Pick<Context, "readResource">, task: Task) {
  if (task.before) {
    return {
      issueDigest: task.before.issue.digest,
      planVersion: task.before.planVersion,
    };
  }
  if (task.continuity) {
    return {
      issueDigest: task.continuity.issueDigest,
      planVersion: task.continuity.planVersion,
    };
  }
  const before = await loadSnapshot(context, task.beforeDigest!);
  return { issueDigest: before.issue.digest, planVersion: before.planVersion };
}
const RestartInput = Input.extend({
  taskId: z.uuid(),
  request: UserRequestSchema,
});
export const AcceptSubmissionInputSchema = z.object({
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
  notificationRunner: YinzRunner;
  captureCloseoutOrigin?: typeof captureCloseoutOrigin;
  offerCloseout?: typeof offerCloseout;
  withIssue: typeof withIssue;
  readAgenda: typeof readAgenda;
  inspect: typeof inspect;
  gitOperation: typeof gitOperation;
  reopenAgenda: typeof reopenAgenda;
  realPath: (path: string) => Promise<string>;
  id: () => string;
  now: () => string;
  submissionPreflight: typeof submissionPreflight;
  /** The control checkout's baseline checks and rebase-probe switch. */
  verificationSettings: (context: Context) => Promise<VerificationSettings>;
  /** Publishes a contract-3 task's assignment files before any prompt is sent. */
  publishAssignment: (task: HydratedTask, diffs: DiffStore) => Promise<void>;
  /** Content-addressed review diff bodies; defaults to the issue's Swamp files. */
  diffStore?: (context: IssueAgentContext) => DiffStore;
  /** Abortable delay between bounded automatic recovery attempts. */
  sleep?: (ms: number, signal: AbortSignal) => Promise<void>;
  /** Monotonic milliseconds for recovery budgets. */
  monotonicNow?: () => number;
}

/** An abortable delay that rejects with the signal's reason. */
export function abortableSleep(ms: number, signal: AbortSignal) {
  return new Promise<void>((resolve, reject) => {
    if (signal.aborted) return reject(signal.reason);
    const abort = () => {
      clearTimeout(timer);
      reject(signal.reason);
    };
    const timer = setTimeout(() => {
      signal.removeEventListener("abort", abort);
      resolve();
    }, ms);
    signal.addEventListener("abort", abort, { once: true });
  });
}
export const defaultDependencies: IssueAgentDependencies = {
  notificationRunner: yinzCommand,
  captureCloseoutOrigin,
  offerCloseout,
  withIssue,
  readAgenda,
  inspect,
  gitOperation,
  reopenAgenda,
  realPath: (path) => Deno.realPath(path),
  id: () => crypto.randomUUID(),
  now: () => new Date().toISOString(),
  submissionPreflight,
  verificationSettings: (context) =>
    loadVerificationSettings(context, gitOperation),
  publishAssignment: (task, diffs) => {
    if (!task.assignmentFiles || !task.submission) {
      return Promise.reject(
        new Error("Task has no assignment files or submission target"),
      );
    }
    return publishAssignmentFiles({
      ...task,
      assignmentFiles: task.assignmentFiles,
      submission: task.submission,
    }, diffs);
  },
  diffStore: swampDiffStore,
  sleep: abortableSleep,
  monotonicNow: () => performance.now(),
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
      taskBeforeDigest(saved.assignment) === stateDigest(state)
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
      "Rollback is incomplete; run drive or resume-rollback to finish its recorded intent before other lifecycle operations",
    );
  }
}

function diffs(
  context: IssueAgentContext,
  dependencies: IssueAgentDependencies,
) {
  return (dependencies.diffStore ?? swampDiffStore)(context);
}

async function saveTask(context: Context, task: Task) {
  await invalidateHandoff(context);
  const saved = await context.writeResource(
    "agent-task",
    "agent-task",
    persistedTask(task),
  );
  await appendReservedRound(context, task);
  await recordRoundOutcomes(context, task);
  return saved;
}

/** What the reviewer covers: the whole feature, or only a repair diff. */
function reviewScopeInstructions(
  continuity: z.infer<typeof ContinuityContext>,
  files: AssignmentFiles | undefined,
) {
  const featureDiff = files
    ? files.files.featureDiff?.path ?? join(files.directory, "feature.diff")
    : undefined;
  const delta = files
    ? files.files.delta?.path ?? join(files.directory, "delta.diff")
    : undefined;
  const encoding =
    "Decode featureDiff/delta from base64 when their encoding field says base64; otherwise they are UTF-8.";
  const persistence =
    "keep persistent IDs and origin identities. For a persists outcome, also include the same finding with its origin in findings and leave resolution unset; label an unchanged ignored concern as historical continuity, not a new defect. Keep ignored findings visibly unresolved. Reopening a verified-fixed/disposed concern requires new concrete source or behavioral evidence and rationale addressing the earlier decision.";
  const scope = continuity.scope;
  if (scope?.kind === "delta") {
    return `This is a delta re-review after a repair pass, not a new audit. Review the repair diff from ${scope.base} (the last reviewed commit of your role) to the reviewed HEAD${
      delta ? `, decoded in ${delta}` : ""
    }. It changes: ${scope.paths.join(", ")}. ${
      files ? "" : `${encoding} `
    }Verify each prior finding listed in continuity.scope.findings: return priorFindings with an outcome (persists, verified_fixed or reopened), rationale and evidence for each; ${persistence} Report a new finding only for a regression this repair diff introduced; its affectedPaths must include a path the repair changed. Do not re-audit unchanged code or raise unrelated or pre-existing concerns. ${
      featureDiff
        ? `The complete feature diff in ${featureDiff} is context only.`
        : "The complete feature diff is context only."
    } Findings include affectedPaths and relevant approved checkIds.`;
  }
  return `Review the complete feature diff, current requirements, approved assumptions, affected callers, tests and failure paths independently. Endpoint delta prioritizes investigation; it never bounds scope. ${
    files
      ? `The complete feature diff and endpoint delta are decoded in ${featureDiff} and ${delta} (when present).`
      : encoding
  } Rebased/divergent history is not a descendant repair. Return priorFindings with an outcome (persists, verified_fixed or reopened), rationale and evidence for every prior unresolved finding of your role; ${persistence} Prior acceptance does not authorize a changed review. New defects anywhere in the full feature scope remain reportable. Findings include affectedPaths and relevant approved checkIds.`;
}

export function taskPrompt(
  role: z.infer<typeof Role>,
  state: z.infer<typeof StateSchema>,
  cwd: string,
  references: string,
  submission: string,
  commit?: string,
  request?: string,
  continuity?: z.infer<typeof ContinuityContext>,
  extras: {
    logAccess?: { issueName: string; controlDir: string };
    // Contract 3: only the continuity block changes, to reference the files.
    files?: AssignmentFiles;
  } = {},
) {
  const files = extras.files;
  const implementing = role === "implement" ||
    role === "implement_review_fixes";
  const logAccess = extras.logAccess && implementing &&
      /log: (?:check|baseline)-log-/.test(state.lastFailure ?? "")
    ? [
      `Read each log named in lastFailure (read-only; this is not the evidence interface) with: ${SWAMP_LOCAL} data query ${
        shellQuote(
          `modelName == ${
            JSON.stringify(extras.logAccess.issueName)
          } && name == "<log>"`,
        )
      } --select content --repo-dir ${shellQuote(extras.logAccess.controlDir)}`,
    ]
    : [];
  const instructions = {
    triage:
      "Investigate the issue against the source. Reproduce a bug when practical. Return its classification and an evidence-based summary, including acceptance criteria and uncertainties.",
    plan:
      "Develop or revise a concrete implementation plan from the issue and investigation. Address existing review findings. Include affected files, risks in step descriptions, assumptions, and a testingStrategy explaining the test selection. Return checks: focused development checks selected per AGENTS.md (never unfiltered full suites), each with a short id, an argv array run verbatim without a shell in the issue worktree, and the plan step IDs it covers. The lifecycle runs every check at the reviewed commit and requires them to pass before merge authorization. Full cargo x ci check is mandatory later in the merge gate unless the user explicitly waives it.",
    review_plan:
      "Adversarially review the current plan against the actual code, including whether its checks are valid command argument arrays that execute directly without a shell, use supported flags, are focused and safe to run unattended, and cover the changed behavior and affected callers. Return planVersion and every actionable finding. Do not approve the plan or invent resolutions. An empty findings array is appropriate only after a real review found none.",
    implement:
      "Implement the approved plan in this existing issue worktree. Run focused checks following AGENTS.md, investigate failures, and commit the intended changes on the existing feature branch. Return a summary and the full resulting commit hash. Do not merge into main, push, or run the full CI gate here. Do not rebase, except when lastFailure records conflicts with local main, or a failed baseline check that local main (refs/heads/main) already fixes: then rebase this branch onto local main and resolve each conflict deliberately, never discarding either side's intent to make it apply. Baseline checks run at this un-rebased branch, so before fixing a baseline failure in code this branch did not change, check whether it comes from the branch's older base; prefer that rebase over re-fixing main's lines here. If lastFailure records a failed merge gate, read its log first and fix the cause, selecting focused tests that reproduce it; treat a flake or timeout as a defect, not something a rerun resolves.",
    review_code:
      "Review the committed diff against local main, issue requirements and the approved plan. Independently assess whether the behavior is correct, including incorrect assumptions or defects in the approved approach. Trace affected callers and relevant state transitions, failure paths, cleanup, ownership, concurrency and platform behavior as applicable. Inspect tests for missing cases and assertions that could pass despite incorrect behavior. Prioritize actionable defects introduced or exposed by this change. For each finding identify the trigger, consequence and concrete source locations, explain severity and distinguish demonstrated behavior from assumptions. Avoid speculative concerns and unrelated cleanup. An empty findings array is valid after a real review. Cover each plan step exactly once and justify deviations. Record only preliminary checks actually performed. Preliminary checks are advisory; the lifecycle runs the approved plan checks itself. Do not invent passing checks. Return the supplied full commit hash and planVersion.",
    plan_review_fixes:
      "Plan focused repairs for activeReviewFix's selected findings in reviewDecisions. Read the stored technical and UX reviews, the user's request and existing approved plan. Do not edit source. Propose only the selected repairs, explain risks in steps, and include focused argv checks and a testingStrategy. Return the repair plan for user approval; this task authorizes no implementation.",
    implement_review_fixes:
      "Repair the selected findings of the last reviewDecisions entry in this existing issue worktree, working directly from those findings, their evidence and the user's decision request; there is no separate repair plan or approval. Change only what the selected findings require, respect ignored findings and the user's selected scope, and preserve the approved plan's behavior. A legacy repair plan in that entry, if present, is guidance only. When the approved plan's checks do not cover a repaired behavior, return checks: focused development checks selected per AGENTS.md (never unfiltered full suites), each with a new short id that no plan check uses, an argv array run verbatim without a shell in the issue worktree, and covers naming the repaired findings or plan steps. The lifecycle runs them after the plan's checks before merge authorization. Run focused checks, investigate failures, and commit changes on the existing feature branch. Return summary, full commit hash and a response for each selected finding; report a finding that needs broader work as unresolved instead of expanding scope. Do not rebase, merge, push, or run full CI.",
    review_ux:
      "Review the supplied committed change specifically for its impact on the user experience. Use the issue requirements, actual implementation, affected callers, tests, help and documentation to identify affected users and tasks. Trace representative end-to-end workflows through discovery, normal use, repeat use, waiting, failure, cancellation and recovery as applicable. Independently assess whether the experience remains understandable, consistent, responsive and usable, even when the code follows the approved plan. Inspect relevant terminal UI, keyboard navigation and focus, rendering/readability, CLI commands and output, configuration/defaults, help/documentation, feedback, errors and lifecycle handoffs. Consider indirect responsiveness, compatibility surprises, extra interaction steps and loss of user work. Can users understand what happened, what is happening and what to do next? Keep implementation details out of user flows unless they support a meaningful user decision. Exercise representative interactions when practical; distinguish observed results from source-based conclusions and state material limitations. Report concrete actionable UX defects introduced or exposed by the change. Each finding needs the affected user/task, triggering interaction sequence, observed or source-derived behavior, user consequence, severity and concrete source or interaction evidence. Avoid personal aesthetic preferences, speculative concerns and unrelated redesigns. An empty findings array is valid after a real review. Always submit assessment with impact, a nonempty rationale, assessedPaths describing affected user tasks/workflows and source paths, and limitations. A no_material_impact result requires a source-based rationale; absence of UI-file changes is insufficient. Preliminary checks are advisory; record only checks actually performed. Return the supplied full commit hash and planVersion.",
  }[role];
  return [
    "You are working on one Yinz issue lifecycle task in a visible Yinz pane.",
    `Work only in ${cwd}. Follow its AGENTS.md and applicable repository instructions.`,
    "Do not invoke the lifecycle launcher. Use only the assigned Swamp evidence interface described below. Do not merge or contact remote Git services.",
    implementing
      ? "Only this task may edit the issue worktree."
      : "This is investigation or review: do not edit source files, Git refs, or configuration.",
    "Run every command and check in the foreground. Do not use run_in_background, Monitor, background subagents or any other background job. If a check exceeds the tool timeout, split it into smaller checks rather than backgrounding it.",
    continuity && role === "review_code"
      ? continuity.scope?.kind === "delta"
        ? instructions.replace(
          "Review the committed diff against local main, issue requirements and the approved plan.",
          "Review the committed repair diff against the prior findings, issue requirements and the approved plan.",
        ).replace(
          "Cover each plan step exactly once and justify deviations.",
          "Cover each plan step exactly once; the evidence template carries each step's status from the prior review, so change one only when the repair diff changes it, and justify deviations.",
        )
        : instructions.replace(
          "against local main",
          "from the captured merge base to reviewed HEAD",
        )
      : instructions,
    ...logAccess,
    ...(continuity
      ? [
        `${
          files
            ? "Assignment contract version 3. Assignment files are read-only and bound by the manifest below."
            : "Assignment contract version 2."
        } The bound packet below contains authoritative findings and decisions; do not truncate or reconstruct history. Worker-proposed tradeoffs are proposals; controller dispositions and exact user authorizations have their recorded scope only.`,
        "Preserve unaffected decisions and identify newly necessary scope changes. Repair implementers return responses (addressed, disputed or unresolved), rationale, affectedPaths, source/check evidence and checkIds for every selected finding. Implementation claims never close findings. Broader work requires a revised plan and approval through the controller.",
        reviewScopeInstructions(continuity, files),
        "Structured advisoryEvidence names producingTask, original full commit, requirementDigest, argv/cwd, result, durable outputRef, relevantFiles/tests, platform/toolchain and prerequisites. Reused evidence must retain original attribution and explain applicability at the current commit. Changed commits require a documented comparison of implementation, callers, tests, fixtures, dependencies, build configuration, requirements and environment; patch identity alone is insufficient. Missing objects/logs, dirty state, unknown dependencies or failed/flaky execution cannot support passing reuse. Free-text preliminaryChecks remain available; reused evidence is never newly executed. Advisory evidence cannot replace approved deterministic verification or rebased merge CI.",
        ...(files
          ? [
            `Bound review continuity (diff bodies in ${
              files.files.baseline?.path ??
                join(files.directory, "baseline.json")
            }): ${
              JSON.stringify({
                ...continuity,
                baseline: continuity.baseline
                  ? baselineView(continuity.baseline)
                  : undefined,
              })
            }`,
            `Authoritative repair/review packet: read ${files.files.packet.path} completely (sha256 ${files.files.packet.sha256}, equal to continuity.packetDigest). sourceRef paths beginning "assignment.before" resolve inside ${files.files.snapshot.path}.`,
            ...([
                "review_code",
                "review_ux",
                "plan_review_fixes",
                "implement_review_fixes",
              ].includes(role)
              ? [
                `Start from ${files.files.template.path}; it already lists every plan step, prior-finding obligation and selected finding identity you must cover exactly once.`,
              ]
              : []),
          ]
          : [
            `Bound review continuity: ${JSON.stringify(continuity)}`,
            `Authoritative repair/review packet (complete saved snapshot references): ${
              JSON.stringify(reviewPacket(state))
            }`,
          ]),
      ]
      : []),
    ...(["plan", "review_plan", "plan_review_fixes"].includes(role)
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
        CHECK_POLICY_SUMMARY,
      ]
      : []),
    ...(role === "implement_review_fixes" ? [CHECK_POLICY_SUMMARY] : []),
    ...deferredPlanFindingInstructions(
      state,
      role,
      continuity?.scope?.kind === "delta",
    ),
    ...(["review_plan", "review_code", "review_ux"].includes(role)
      ? [
        `Read ${join(references, "review.md")} and ${
          join(references, "dimensions.md")
        } before reviewing.`,
        "Follow the shared evidence and severity standards. Leave resolution unset for a newly reported defect: a proposed fix is not a resolution. Use resolution only for a concern verified fixed or otherwise supported as resolved. Preserve finding IDs and supported dispositions; reopening a disposed concern requires new evidence and an explanation of why its disposition no longer holds. Historical evidence is not current approval. Follow AGENTS.md focused-test selection; do not run unfiltered full suites.",
        role === "review_plan"
          ? "Apply relevant Yinz review dimensions to the proposed behavior and verification coverage against source."
          : "Apply relevant Yinz review dimensions to the committed implementation and affected callers.",
        "When applicable, cite a dimension name in description and its authoritative source in evidence alongside concrete affected code or plan locations. Use the existing finding fields; do not add a category property. A dimension citation does not replace requirement, mechanism, source evidence, severity, finding identity, disposition or reopening rules. An inapplicable dimension creates no finding or additional testing obligation.",
      ]
      : []),
    ...(request
      ? ["Exact user instructions for this assignment:", request]
      : []),
    submission,
    "Current lifecycle snapshot (issue requirements and recorded evidence):",
    continuity
      ? JSON.stringify({
        phase: state.phase,
        planVersion: state.planVersion,
        rollbackId: state.rollbackId,
        classification: state.classification,
        implementation: state.implementation,
        sourceRef: "assignment.before",
        note: files
          ? `Complete requirements and review obligations are in the packet. The full original snapshot is in ${files.files.snapshot.path} and remains in this task's authorized submission record; its historical diffs are inline or named by featureDiffRef/deltaRef.`
          : "Complete requirements and review obligations are in the packet. The full original snapshot, including referenced historical diffs, remains in this task's authorized submission record.",
      })
      : JSON.stringify(state),
    ...(commit ? [`The commit being reviewed is ${commit}.`] : []),
  ].join("\n\n");
}

/**
 * Plan findings the user deferred when approving the plan stay unresolved; the
 * implementer accounts for them and full implementation reviews revisit them.
 * A delta re-review reports only regressions of its repair diff, so the full
 * review that preceded it has already revisited them.
 */
function deferredPlanFindingInstructions(
  state: LifecycleState,
  role: string,
  delta: boolean,
) {
  const deferred = deferredPlanFindings(state);
  if (
    !deferred ||
    !["implement", "review_code", "review_ux"].includes(role) ||
    (delta && role !== "implement")
  ) return [];
  return [
    `The user approved plan version ${
      state.approval!.planVersion
    } despite these unresolved plan review findings, deferring them to implementation review (planApproval.deferredFindings). They are not resolved and were not waived for implementation: ${
      JSON.stringify(deferred.findings)
    }`,
    role === "implement"
      ? "Implement the approved plan; account for each deferred concern where the plan allows, and state in your summary how the committed change bears on it. Do not expand scope beyond the approved plan to address one."
      : "Revisit each deferred plan finding against the committed implementation. Report one that still applies to the delivered behavior as an ordinary finding with current source evidence and a severity justified by the implementation; a deferred concern the implementation settles needs no finding.",
  ];
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
    // The child already names the blocked cleanup and Yinz's reason.
    throw new Error(
      result.error.message.startsWith("Worker cleanup")
        ? result.error.message
        : `Worker cleanup blocked: ${result.error.message}`,
    );
  }
  return {
    dataHandles: [await saveTask(context, { ...task, cleanup: "done" })],
  };
}

async function completeTask(context: IssueAgentContext, task: Task) {
  const completed: Task = {
    ...task,
    status: "completed",
    // An operator-accepted submission never proved its pane quiescent; leave the
    // pane for the operator rather than closing it through guarded cleanup.
    cleanup: task.config.stackWorkers && !task.acceptance
      ? "pending"
      : undefined,
  };
  const saved = await saveTask(context, completed);
  const cleaned = await cleanupTask(context, completed);
  return {
    dataHandles: cleaned.dataHandles.length ? cleaned.dataHandles : [saved],
  };
}

async function taskAssignment(context: IssueAgentContext, task: Task) {
  if (!task.submission) {
    throw new Error(
      "Legacy task requires a typed submission supplement; use continue-agent on its original worker",
    );
  }
  const core = assignmentCore(
    context,
    await withBefore(context, task),
    task.submission.controlDir,
  );
  return AssignmentSchema.parse({
    ...core,
    digest: task.submission.assignmentDigest,
    promptDigest: task.promptDigest,
  });
}

async function evidenceOperation(
  context: IssueAgentContext,
  task: Task,
  method: string,
  arguments_: Record<string, unknown>,
) {
  if (!task.submission) return;
  if (
    method === "supersede" &&
    !await context.dataRepository.getContent(
      "@yinz/issue-evidence",
      task.submission.behaviorName,
      "submission",
    )
  ) {
    // Dispatch cannot reach a worker before open commits. A controller killed
    // before that write can supersede its saved task without creating a slot.
    return;
  }
  const outcome = await context.runModel({
    modelType: "@yinz/issue-evidence",
    name: task.submission.behaviorName,
    method,
    arguments: arguments_,
  });
  if (!outcome.ok) {
    throw new Error(
      `Swamp evidence ${method} blocked: ${outcome.error.message}`,
    );
  }
}

/** The validated accepted receipt for this exact assignment, or an error naming why not. */
export async function acceptedEvidence(context: IssueAgentContext, task: Task) {
  const assignment = await taskAssignment(context, task);
  const bytes = await context.dataRepository.getContent(
    "@yinz/issue-evidence",
    task.submission!.behaviorName,
    "submission",
  );
  const saved = bytes
    ? SubmissionSchema.parse(JSON.parse(new TextDecoder().decode(bytes)))
    : undefined;
  if (!saved || saved.status === "open") {
    throw new Error(
      "submission_required: provider turn ended without accepted Swamp evidence; use continue-agent to finish submission in the original conversation",
    );
  }
  if (
    saved.status !== "accepted" ||
    canonical(saved.assignment) !== canonical(assignment)
  ) {
    throw new Error(
      "Swamp evidence belongs to a superseded or different assignment",
    );
  }
  return validateReceipt(assignment, saved.receipt);
}

/** The commit an accepted receipt binds for freshness checks, by role. */
function evidenceCommit(task: Task, receipt: EvidenceReceipt) {
  const payload = receipt.payload;
  if (payload.role === "plan_review_fixes") return task.reviewCommit;
  return payload.role === "implement_review_fixes" ||
      payload.role === "implement" || payload.role === "review_code" ||
      payload.role === "review_ux"
    ? payload.evidence.commit
    : undefined;
}

/** Typed implementation evidence, including in cached driver handoffs. */
export function implementationEvidence(task: Task) {
  if (
    task.role !== "implement" || task.evidence?.payload.role !== "implement"
  ) {
    throw new Error(
      "Legacy implementation needs a typed submission supplement; use continue-agent on its original worker before reviewing code",
    );
  }
  if (
    !task.submission ||
    task.evidence.assignmentDigest !== task.submission.assignmentDigest ||
    task.evidence.taskId !== task.id ||
    task.evidence.payloadDigest !== evidenceDigest(task.evidence.payload)
  ) {
    throw new Error("Implementation receipt does not match its task");
  }
  return ImplementationResult.parse(task.evidence.payload.evidence);
}

async function validateResultFreshness(
  context: IssueAgentContext,
  task: Task,
  current: z.infer<typeof StateSchema>,
  dependencies: IssueAgentDependencies,
  commit?: string,
) {
  const fresh = await dependencies.readAgenda(
    current.issue.workspace,
    current.issue.itemId,
  );
  if (fresh.digest !== current.issue.digest) {
    throw new Error(
      "Agenda requirements changed while the pane task ran; its result is stale",
    );
  }
  if (stateDigest(current) !== taskBeforeDigest(task)) {
    throw new Error(
      "Lifecycle changed while the pane task ran; inspect its result before recording new evidence",
    );
  }
  if (
    task.role === "implement" || task.role === "review_code" ||
    task.role === "review_ux" || task.role === "plan_review_fixes" ||
    task.role === "implement_review_fixes"
  ) {
    if (
      !commit ||
      (!["implement", "implement_review_fixes"].includes(task.role) &&
        commit !== task.reviewCommit)
    ) {
      throw new Error(
        "Pane result has no verifiable evidence commit; use continue-agent for a typed submission supplement",
      );
    }
    if (!current.implementation) {
      throw new Error("Pane implementation has no recorded issue worktree");
    }
    const repo = await dependencies.inspect(
      context,
      current.implementation.worktree,
    );
    if (
      !repo.clean || repo.branch !== current.implementation.branch ||
      repo.head !== commit
    ) {
      throw new Error(
        "Pane result does not match the clean issue branch's current commit",
      );
    }
  }
}

async function applyResult(
  context: IssueAgentContext,
  task: Task,
  dependencies: IssueAgentDependencies,
) {
  if (task.discard) return finishDiscard(context, task);
  const current = await loadState(context);
  if (task.status === "completed") return cleanupTask(context, task);
  if (["discarded", "superseded"].includes(task.status)) {
    return { dataHandles: [] };
  }
  // Reconcile applied legacy projections without reading chat. A pending
  // projection still needs current requirements and verifiable commit evidence.
  if (
    !task.submission && task.status === "applying" && task.next
  ) {
    if (
      taskNextDigest(task) !== taskBeforeDigest(task) &&
      stateDigest(current) === taskNextDigest(task)
    ) return completeTask(context, task);
    if (stateDigest(current) !== taskBeforeDigest(task)) {
      throw new Error(
        "Lifecycle changed during legacy evidence reconciliation",
      );
    }
    await validateResultFreshness(
      context,
      task,
      current,
      dependencies,
      task.role === "review_code"
        ? task.next.codeReview?.commit
        : task.role === "review_ux"
        ? task.next.uxReview?.commit
        : undefined,
    );
    await saveState(
      context,
      `agent_${task.role}_${task.id}`,
      task.next,
      `event-agent-${task.id}`,
    );
    return completeTask(context, task);
  }
  const receipt = await acceptedEvidence(context, task);
  // Operator acceptance replaces only the pane-completion proof. Every receipt,
  // requirement and Git freshness check below still applies.
  const message = task.acceptance
    ? undefined
    : (await completedResult(context, task)).message;
  // An applying task carries its exact accepted receipt. State equality alone
  // cannot prove import for implementation reports, whose next equals before.
  const eventName = `event-agent-${task.id}`;
  const event = await context.readResource(eventName);
  const projection = event == null
    ? undefined
    : z.object({ action: z.string(), state: StateSchema }).parse(event);
  const projected = projection?.action === `agent_${task.role}_${task.id}` &&
    canonical(projection.state) === canonical(receipt.next) &&
    canonical(current) === canonical(receipt.next);
  if (
    task.status === "applying" && task.evidence &&
    canonical(taskReceipt(task.evidence)) === canonical(taskReceipt(receipt)) &&
    projected
  ) {
    return completeTask(context, task);
  }
  await validateResultFreshness(
    context,
    task,
    current,
    dependencies,
    evidenceCommit(task, receipt),
  );
  // The receipt's state is saved as the lifecycle state; the task names it.
  const { next: _legacyNext, ...pending } = task;
  const applying: Task = {
    ...pending,
    status: "applying",
    nextDigest: stateDigest(receipt.next),
    evidence: taskReceipt(receipt),
    ...(message === undefined ? {} : { message }),
  };
  await saveTask(context, applying);
  const handles = await saveState(
    context,
    `agent_${task.role}_${task.id}`,
    receipt.next,
    eventName,
  );
  handles.dataHandles.push(
    ...(await completeTask(context, applying)).dataHandles,
  );
  return handles;
}

/**
 * A new assignment for the current state. The returned task carries `before`
 * in memory for dispatch; it persists only the snapshot's digest.
 */
export async function prepareTask(
  context: IssueAgentContext,
  args: z.infer<typeof Input>,
  state: z.infer<typeof StateSchema>,
  dependencies: IssueAgentDependencies,
  allowCompleted = false,
): Promise<HydratedTask> {
  if (
    state.closeout?.authorizedRequest &&
    (state.implementation?.worktree ?? state.workerWorktree) ===
      state.closeout.target.worktree
  ) {
    throw new Error(
      "Closeout may have removed the worker worktree; deliberately select a new valid worktree with implement before launching more work",
    );
  }
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
    review_ux: ["implementing", "ready"],
    plan_review_fixes: ["implementing"],
    implement_review_fixes: ["implementing"],
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
  if (
    args.role === "implement" || args.role === "review_code" ||
    args.role === "review_ux" || args.role === "plan_review_fixes" ||
    args.role === "implement_review_fixes"
  ) {
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
      (!["implement", "implement_review_fixes"].includes(args.role) &&
        !repo.clean)
    ) {
      throw new Error(
        "Commit implementation changes before requesting code review",
      );
    }
    if (args.role !== "implement") commit = repo.head;
    if (args.role === "plan_review_fixes") {
      throw new Error(
        "Repair planning is retired: run drive, which dispatches implement_review_fixes for the selected findings",
      );
    }
    if (args.role === "implement_review_fixes" && !activeReviewFix(state)) {
      throw new Error(
        "Review repairs require a current selection of findings to repair",
      );
    }
    if (args.role === "implement" && activeReviewFix(state)) {
      throw new Error(
        "Use the review repair worker for selected findings",
      );
    }
    if (
      args.role === "review_ux" &&
      (!state.codeReview || state.codeReview.commit !== commit ||
        state.implementation.head !== commit ||
        state.codeReview.planVersion !== state.plan.version)
    ) {
      throw new Error(
        "UX dispatch requires current technical review of the same clean implementation commit",
      );
    }
  }
  // The dispatched commit belongs to task evidence, not prior code review.
  const promptState = structuredClone(state);
  // The current endpoint is in continuity.baseline; retain the saved decision snapshot.
  // Issue worktrees may predate the shared standards; read the control checkout's.
  const references = join(
    state.implementation ? await dependencies.realPath(context.repoDir) : cwd,
    ".agents",
    "skills",
    "issue-lifecycle",
    "references",
  );
  await dependencies.submissionPreflight(context);
  const { role: _role, request: _request, ...options } = args;
  const config = AgentConfigSchema.parse({
    ...options,
    cwd,
    stackWorkers: true,
  });
  const identity = {
    id: dependencies.id(),
    behaviorName: `issue-agent-${dependencies.id()}`,
    role: args.role,
    ...(args.request ? { request: args.request } : {}),
    config,
    before: state,
    beforeDigest: await saveSnapshot(context, state),
    ...(commit ? { reviewCommit: commit } : {}),
    contractVersion: 3 as const,
    createdAt: dependencies.now(),
    round: undefined as z.infer<typeof RoundRecord> | undefined,
    continuity: undefined as z.infer<typeof ContinuityContext> | undefined,
  };
  let baseline: z.infer<typeof GitBaseline> | undefined;
  let assignmentDiffs: Awaited<ReturnType<typeof baselineBytes>> | undefined;
  if (
    state.implementation &&
    [
      "implement",
      "review_code",
      "review_ux",
      "plan_review_fixes",
      "implement_review_fixes",
    ].includes(args.role)
  ) {
    const role = args.role === "review_ux" ? "review_ux" : "review_code";
    const currentReview = role === "review_ux"
      ? state.uxReview
      : state.codeReview;
    const priorReview = currentReview ??
      state.reviewHistory?.findLast((h) =>
        h.role === role && h.issueDigest === state.issue.digest
      )?.review;
    const repo = await dependencies.inspect(context, cwd);
    baseline = GitBaseline.parse(
      await dependencies.gitOperation(context, {
        action: "review-context",
        worktree: cwd,
        head: commit ?? repo.head,
        // The helper compares commit identities; diff bodies stay out of line.
        ...(priorReview
          ? {
            previous: baselineIdentity(
              priorReview.baseline ?? { head: priorReview.commit },
            ),
          }
          : {}),
        ...(args.role === "review_ux" && state.codeReview?.baseline
          ? { baseline: baselineIdentity(state.codeReview.baseline) }
          : {}),
      }, `review-baseline-${identity.id}`),
    );
    if (args.role === "review_ux" && !state.codeReview?.baseline) {
      baseline.limitations.push(
        "Legacy technical review has no captured baseline. Equivalence cannot be established; UX reviews the complete currently captured feature scope without retroactive technical reinvestigation.",
      );
    }
    if (baseline.head !== (commit ?? repo.head)) {
      throw new Error("Captured review baseline differs from dispatch HEAD");
    }
    // Store each diff body once; the task, its reviews and state reference it.
    assignmentDiffs = await baselineBytes(undefined, baseline);
    baseline = await compactBaseline(diffs(context, dependencies), baseline);
  }
  identity.continuity = ContinuityContext.parse({
    version: 1,
    assignmentId: identity.id,
    issueDigest: state.issue.digest,
    generation: planningGeneration(state),
    rollbackId: state.rollbackId,
    planVersion: state.planVersion,
    packetDigest: evidenceDigest(reviewPacket(state)),
    reviewFingerprints: (["review_code", "review_ux"] as const).flatMap((
      role,
    ) =>
      (role === "review_code" ? state.codeReview : state.uxReview)
        ? [implementationReviewFingerprint(state, role)]
        : []
    ),
    baseline,
    ...(baseline && (args.role === "review_code" || args.role === "review_ux")
      ? { scope: reviewScope(state, args.role, baseline) }
      : {}),
  });
  identity.round = await createRound(context, identity);
  await dependencies.captureCloseoutOrigin?.(context, args.control);
  const controlDir = await dependencies.realPath(context.repoDir);
  const submission = EvidenceTargetSchema.parse({
    protocol: 1,
    behaviorName: `issue-evidence-${identity.id}`,
    controlDir,
    assignmentDigest: evidenceDigest(
      assignmentCore(context, identity, controlDir),
    ),
  });
  // Private, outside every worktree; the prompt binds the manifest's sha256.
  const commonDir = await dependencies.realPath(
    (await dependencies.inspect(context)).commonDir,
  );
  const assignmentFiles = assignmentManifest(
    join(commonDir, "yinz-lifecycle", "assignments", identity.id),
    assignmentFileContents({ ...identity, submission }, assignmentDiffs),
  );
  const prompt = taskPrompt(
    args.role,
    promptState,
    cwd,
    references,
    submissionInstructions(identity, submission, assignmentFiles),
    commit,
    args.request,
    identity.continuity,
    {
      logAccess: { issueName: context.definition.name, controlDir },
      files: assignmentFiles,
    },
  );
  const task: HydratedTask = {
    ...identity,
    submission,
    assignmentFiles,
    prompt,
    promptDigest: await promptDigest(prompt),
    status: "running",
  };
  return task;
}

/** Reservation journal is independent of lifecycle gates and completion recovery. */
async function createRound(
  context: Context,
  task: Pick<
    HydratedTask,
    "id" | "role" | "before" | "createdAt" | "request" | "continuity"
  >,
) {
  const raw = await context.readResource("rounds");
  const journal = z.object({ records: z.array(RoundRecord) }).strict().parse(
    raw ?? { records: [] },
  );
  const existing = journal.records.find((r) => r.assignmentId === task.id);
  if (existing) return existing;
  const previous = journal.records.at(-1);
  const savedTask = AgentTaskSchema.safeParse(
    await context.readResource("agent-task"),
  );
  const predecessor = previous?.assignmentId ??
    (savedTask.success && savedTask.data.id !== task.id
      ? savedTask.data.id
      : undefined);
  const causes: z.infer<typeof RoundRecord>["launchCauses"] = [];
  const state = task.before;
  if (!predecessor && task.role === "triage" && state.phase === "triaging") {
    causes.push({
      kind: "initial_work",
      explanation: "First recorded assignment under the new contract",
      evidence: [`assignment:${task.id}`],
      source: "controller",
    });
  }
  if (previous && previous.issueDigest !== state.issue.digest) {
    causes.push({
      kind: "changed_requirements",
      explanation: "Requirements digest changed",
      evidence: [previous.issueDigest, state.issue.digest],
      source: "controller",
    });
  }
  if (state.lastFailure) {
    // repair clears the verification and may prefix the recorded evidence.
    const gateFailure = state.verification?.status === "failed" ||
      state.mergeFailures?.some((f) =>
        state.lastFailure!.includes(describeMergeFailure(f))
      ) ||
      /(?:^|\n)Verification [0-9a-f-]{36} failed at [0-9a-f]{40,64}/.test(
        state.lastFailure,
      );
    causes.push({
      kind: gateFailure ? "verification_merge_failure" : "unknown",
      explanation: state.lastFailure,
      evidence: [
        ...(state.mergeFailures ?? []).map((f) => f.logResource),
        ...(state.verification
          ? [`verification:${state.verification.id}`]
          : []),
        `assignment:${task.id}:before.lastFailure`,
      ],
      source: "controller",
    });
  }
  const recovery = await context.readResource("recovery");
  if (
    state.rollbackId && recovery && typeof recovery === "object" &&
    "id" in recovery && recovery.id === state.rollbackId
  ) {
    causes.push({
      kind: "recovery",
      explanation: "Assignment follows recorded rollback/restart",
      evidence: [`recovery:${state.rollbackId}`],
      source: "recovery",
    });
  }
  const fix = activeReviewFix(state);
  if (fix) {
    causes.push({
      kind: "newly_discovered_defect",
      explanation:
        "User selected review findings for repair; introduction is unattributed",
      evidence: [`review-decision:${fix.id}`, fix.reviewFingerprint],
      source: "user",
    });
  }
  if (!causes.length) {
    causes.push({
      kind: "unknown",
      explanation: task.request ??
        "Ordinary phase dispatch; no further cause established",
      evidence: [`assignment:${task.id}`],
      source: task.request ? "user" : "controller",
    });
  }
  return RoundRecord.parse({
    assignmentId: task.id,
    predecessor,
    role: task.role,
    at: task.createdAt,
    issueDigest: state.issue.digest,
    generation: task.continuity!.generation,
    launchCauses: causes,
    corrections: [],
  });
}

/** The original round is saved atomically inside a task/reserved driver assignment.
 * This journal projection reconciles after interrupted publication, without phantom
 * rounds for assignments whose reservation never persisted. */
async function appendReservedRound(context: Context, task: Task) {
  if (!hasContinuityContract(task.contractVersion) || !task.round) return;
  const raw = await context.readResource("rounds");
  const journal = z.object({ records: z.array(RoundRecord) }).strict().parse(
    raw ?? { records: [] },
  );
  if (journal.records.some((r) => r.assignmentId === task.id)) return;
  journal.records.push(task.round);
  await context.writeResource("rounds", "rounds", journal);
}

async function recordRoundOutcomes(context: Context, task: Task) {
  const payload = task.evidence?.payload;
  if (
    !hasContinuityContract(task.contractVersion) || !payload ||
    (payload.role !== "review_code" && payload.role !== "review_ux")
  ) return;
  const raw = await context.readResource("rounds");
  if (!raw) return;
  const journal = z.object({ records: z.array(RoundRecord) }).strict().parse(
    raw,
  );
  const record = journal.records.find((r) => r.assignmentId === task.id);
  if (!record) return;
  let changed = false;
  // Loaded only when a correction is not yet recorded.
  let snapshot: State | undefined;
  const before = async () =>
    snapshot ??= (await withBefore(context, task)).before;
  for (const outcome of payload.evidence.priorFindings ?? []) {
    if (outcome.outcome !== "persists") continue;
    const source = `review:${task.id}:${evidenceDigest(outcome)}`;
    if (record.corrections.some((c) => c.source === source)) continue;
    const fix = (await before()).reviewDecisions?.findLast((d) =>
      d.result &&
      d.repair.some((r) =>
        r.role === outcome.finding.role &&
        r.findingId === outcome.finding.findingId
      )
    );
    if (!fix) continue;
    record.corrections.push({
      at: task.evidence!.acceptedAt,
      source,
      cause: {
        kind: "incomplete_repair",
        explanation: outcome.rationale,
        evidence: [`review-decision:${fix.id}`, ...outcome.evidence],
        source: "worker",
      },
    });
    changed = true;
  }
  let prior: ReturnType<typeof priorImplementationFindings> | undefined;
  for (const finding of payload.evidence.findings) {
    if (finding.resolution) continue;
    const source = `review:${task.id}:new:${evidenceDigest(finding)}`;
    if (record.corrections.some((c) => c.source === source)) continue;
    prior ??= priorImplementationFindings(await before());
    if (
      prior.some((p) =>
        p.ref.role === payload.role && p.finding.id === finding.id
      )
    ) continue;
    record.corrections.push({
      at: task.evidence!.acceptedAt,
      source,
      cause: {
        kind: "newly_discovered_defect",
        explanation: finding.description,
        evidence: [`assignment:${task.id}`, ...(finding.evidence ?? [])],
        source: "worker",
      },
    });
    changed = true;
  }
  if (changed) await context.writeResource("rounds", "rounds", journal);
}

export async function runTask(
  context: IssueAgentContext,
  task: Task,
  dependencies: IssueAgentDependencies,
  recover = false,
) {
  if (task.discard) return finishDiscard(context, task);
  // Recorded operator acceptance never observes or prompts the pane again.
  if (task.acceptance) return applyResult(context, task, dependencies);
  await saveTask(context, task);
  try {
    if (task.submission) {
      await dependencies.submissionPreflight(context);
      await evidenceOperation(context, task, "open", {
        assignment: await taskAssignment(context, task),
      });
    }
    if (!task.config.control) {
      const child = recover ? await childState(context, task) : undefined;
      if (!child || child.phase === "preflight") {
        throw new Error(
          "Legacy unlaunched lifecycle worker requires a restart with control identity",
        );
      }
    }
    // A controller can lose its response after the child saved completion. Read
    // durable state first; applyResult still validates the result and receipt.
    if (recover && task.submission) {
      const receipt = await context.dataRepository.getContent(
        "@yinz/issue-evidence",
        task.submission.behaviorName,
        "submission",
      );
      const accepted = receipt && SubmissionSchema.parse(
            JSON.parse(new TextDecoder().decode(receipt)),
          ).status === "accepted";
      const bytes = await context.dataRepository.getContent(
        "@yinz/agent",
        task.behaviorName,
        "state",
      );
      if (accepted && !bytes) {
        throw new Error(
          "Accepted evidence has no durable original worker state; inspect the original child and submission before recovery, do not launch or resend",
        );
      }
      if (accepted && bytes) {
        const child = await childState(context, task);
        if (child.phase === "completed") {
          return await applyResult(context, task, dependencies);
        }
      }
    }
    // Contracts 1 and 2 never publish: their saved prompts carry everything.
    if (task.contractVersion === 3 && !task.assignmentFiles) {
      throw new Error(
        "Contract-3 task has no assignment files; inspect, then explicitly restart the worker",
      );
    }
    if (task.assignmentFiles) {
      await dependencies.publishAssignment(
        await withBefore(context, task),
        diffs(context, dependencies),
      );
    }
    const continuing = recover && task.continuation;
    const outcome = await context.runModel({
      modelType: "@yinz/agent",
      name: task.behaviorName,
      method: continuing ? "continue" : recover ? "recover" : "run",
      arguments: continuing
        ? { id: continuing.id, prompt: continuing.prompt }
        : { ...task.config, prompt: task.prompt },
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

/** Completes a pending rollback's saved intent. The caller owns the issue lock. */
export async function finishRecovery(
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
    await evidenceOperation(context, recovery.task, "supersede", {
      taskId: recovery.task.id,
      assignmentDigest: recovery.task.submission?.assignmentDigest,
      operationId: recovery.id,
    });
    // Archive under a stable name before replacing the current assignment. Retrying
    // a partial write does not depend on the old pane, submission or child behavior.
    handles.push(
      await context.writeResource(
        "agent-task",
        `agent-task-${recovery.task.id}`,
        persistedTask({
          ...recovery.task,
          status: "superseded",
          cleanup: undefined,
          supersededBy: recovery.id,
        }),
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
  const repairRestart = restart &&
    ["plan_review_fixes", "implement_review_fixes"].includes(restart.role);
  const selected = repairRestart
    ? activeReviewFix(source) ??
      (task?.role === restart.role
        ? activeReviewFix((await withBefore(context, task)).before)
        : undefined)
    : undefined;
  if (restart?.role === "plan_review_fixes") {
    throw new Error(
      "Repair planning is retired: discard the planner with discard-agent, then run drive to dispatch implement_review_fixes",
    );
  }
  if (repairRestart && !selected) {
    throw new Error("No current review repair assignment to restart");
  }
  let next = repairRestart
    ? structuredClone(source)
    : rollbackState(source, input.step, id);
  if (repairRestart && selected) {
    // Import clears the reviews and advances the head before pane cleanup. An
    // explicit restart restores that scope while retaining the imported outcome
    // in history; it does not rewind the checkout or require another approval.
    next.codeReview = structuredClone(selected.codeReview);
    if (selected.uxReview) next.uxReview = structuredClone(selected.uxReview);
    else delete next.uxReview;
    next.implementation!.head = selected.commit;
    next.rollbackId = id;
    const replacement = {
      ...structuredClone(selected),
      id,
      reviewFingerprint: codeReviewFingerprint(next),
      generation: planningGeneration(next),
      rollbackId: id,
    };
    next.reviewDecisions = [...(next.reviewDecisions ?? []), replacement];
    delete next.verification;
    delete next.mergeAuthorization;
  }
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
    if (!previous?.request) continue;
    const basis = await taskBasis(context, previous);
    if (basis.issueDigest === before.issue.digest) {
      recordFeedback(next, previous.request, basis.planVersion);
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
    ...(task ? { task: persistedTask(task) } : {}),
    ...(replacement ? { replacement: persistedTask(replacement) } : {}),
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

/** Durable discard intent must be fenced before any provider work or import. */
async function finishDiscard(context: IssueAgentContext, task: Task) {
  if (["discarded", "superseded"].includes(task.status)) {
    return { dataHandles: [] };
  }
  if (!task.discard) throw new Error("No pending discard intent");
  const current = await loadState(context);
  if (
    task.status === "completed" ||
    stateDigest(current) !== taskBeforeDigest(task)
  ) {
    throw new Error(
      "Pane evidence may already be applied; cannot finish pending discard",
    );
  }
  await evidenceOperation(context, task, "supersede", {
    taskId: task.id,
    assignmentDigest: task.submission?.assignmentDigest,
    operationId: task.id,
  });
  return {
    dataHandles: [await saveTask(context, { ...task, status: "discarded" })],
  };
}

export async function resumeTask(
  context: IssueAgentContext,
  dependencies: IssueAgentDependencies,
) {
  const task = AgentTaskSchema.parse(await context.readResource("agent-task"));
  if (task.discard) return finishDiscard(context, task);
  if (
    ["completed", "discarded", "superseded", "applying"].includes(task.status)
  ) {
    return applyResult(context, task, dependencies);
  }
  return runTask(context, task, dependencies, true);
}

/**
 * Finishes the recorded rollback and resumes its exact replacement, if any.
 * The caller owns the issue lock.
 */
export async function resumeRollback(
  context: IssueAgentContext,
  dependencies: IssueAgentDependencies,
) {
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
}

export function createAgentMethods(
  dependencies: IssueAgentDependencies = defaultDependencies,
) {
  return {
    continue_agent: {
      kind: "action",
      description:
        "Ask the original retained worker to submit its existing findings through Swamp, without another investigation",
      arguments: z.object({ taskId: z.uuid(), request: UserRequestSchema })
        .strip(),
      execute: (
        args: { taskId: string; request: string },
        context: IssueAgentContext,
      ) =>
        dependencies.withIssue(context, async () => {
          await assertNoPendingRecovery(context);
          let task = AgentTaskSchema.parse(
            await context.readResource("agent-task"),
          );
          if (
            task.id !== args.taskId ||
            ["discarded", "superseded"].includes(task.status)
          ) {
            throw new Error(
              "Continuation must target the current original assignment",
            );
          }
          if (task.discard) return finishDiscard(context, task);
          if (task.evidence) {
            return task.status === "completed"
              ? applyResult(context, task, dependencies)
              : runTask(context, task, dependencies, true);
          }
          const current = await loadState(context);
          if (stateDigest(current) !== taskBeforeDigest(task)) {
            throw new Error(
              "Lifecycle changed; cannot continue a stale submission assignment",
            );
          }
          const nextDigest = taskNextDigest(task);
          if (
            task.status === "applying" && nextDigest &&
            nextDigest !== taskBeforeDigest(task)
          ) {
            throw new Error(
              "Resume the existing typed projection before continuing",
            );
          }
          await dependencies.submissionPreflight(context);
          if (!task.submission) {
            const controlDir = await dependencies.realPath(context.repoDir);
            task = {
              ...task,
              status: "running",
              cleanup: undefined,
              submission: {
                protocol: 1,
                behaviorName: `issue-evidence-${task.id}`,
                controlDir,
                assignmentDigest: evidenceDigest(
                  assignmentCore(
                    context,
                    await withBefore(context, task),
                    controlDir,
                  ),
                ),
              },
            };
          }
          await saveTask(context, task);
          await evidenceOperation(context, task, "open", {
            assignment: await taskAssignment(context, task),
          });
          // A prior continuation may already have committed evidence despite a
          // lost CLI response. Never send another prompt before inspecting it.
          const bytes = await context.dataRepository.getContent(
            "@yinz/issue-evidence",
            task.submission!.behaviorName,
            "submission",
          );
          if (
            bytes &&
            SubmissionSchema.parse(JSON.parse(new TextDecoder().decode(bytes)))
                .status === "accepted"
          ) return runTask(context, task, dependencies, true);
          if (task.continuation?.request !== args.request) {
            assertContinuationEligible(await childState(context, task));
          }
          const continuation = task.continuation?.request === args.request
            ? task.continuation
            : {
              id: dependencies.id(),
              request: args.request,
              prompt: [
                "Finish submission of the findings already established in this conversation. Preserve this task and its investigation; do not repeat the review or edit source or Git refs.",
                args.request,
                submissionInstructions(
                  await withBefore(context, task),
                  task.submission!,
                  task.assignmentFiles,
                ),
              ].join("\n\n"),
            };
          task = { ...task, continuation };
          await saveTask(context, task);
          if (task.assignmentFiles) {
            await dependencies.publishAssignment(
              await withBefore(context, task),
              diffs(context, dependencies),
            );
          }
          const outcome = await context.runModel({
            definition: task.behaviorName,
            method: "continue",
            arguments: { id: continuation.id, prompt: continuation.prompt },
          });
          if (!outcome.ok) throw new Error(outcome.error.message);
          return applyResult(context, task, dependencies);
        }),
    },
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
        dependencies.withIssue(
          context,
          () => resumeRollback(context, dependencies),
        ),
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
              {
                step: args.role === "plan_review_fixes" ||
                    args.role === "implement_review_fixes"
                  ? "review_code"
                  : args.role,
                request: args.request,
              },
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
        "Run a lifecycle phase in a visible Yinz pane and import its accepted Swamp evidence after provider completion",
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
        "Explicitly discard unapplied terminal work, a resource-free launch rejection or a completed unusable worker, fencing further submissions with the operator's request; the behavior verifies eligibility, and confirmedStopped only attests a completed worker after the behavior's own observation proves impossible",
      arguments: DiscardArgs,
      execute: (
        input: z.infer<typeof DiscardArgs>,
        context: IssueAgentContext,
      ) =>
        dependencies.withIssue(context, async () => {
          await assertNoPendingRecovery(context);
          const discard = DiscardArgs.parse(input);
          const task = AgentTaskSchema.parse(
            await context.readResource("agent-task"),
          );
          if (task.status === "discarded") return { dataHandles: [] };
          if (task.discard) return finishDiscard(context, task);
          if (task.status === "completed") {
            throw new Error(
              "Pane evidence is already applied; revise the lifecycle instead",
            );
          }
          const who =
            `task ${task.id} (${task.role}, child ${task.behaviorName})`;
          const childOf = () =>
            childState(context, task).catch((error) => {
              throw new Error(
                `Cannot discard ${who}: ${
                  error instanceof Error ? error.message : error
                }`,
              );
            });
          let child = await childOf();
          let basis: z.infer<typeof DiscardBasisSchema> | undefined;
          let stopEvidence: z.infer<typeof StopEvidenceSchema> | undefined;
          let terminal = terminalFailureProblems(child);
          if (!terminal.length) {
            // confirm_failure re-waits the original submission and requires
            // fresh idle status; its refusal names the failed idle conditions.
            const outcome = await context.runModel({
              definition: task.behaviorName,
              method: "confirm_failure",
            });
            if (!outcome.ok) throw new Error(outcome.error.message);
            child = await childOf();
            terminal = terminalFailureProblems(child);
            if (!terminal.length) {
              basis = "terminal_failure";
              stopEvidence = "confirmed_failure";
            }
          }
          const rejected = basis ? [] : rejectedLaunchProblems(child);
          if (!basis && !rejected.length) {
            basis = "rejected_launch";
            // The same condition selects the receipt branch of the predicate.
            stopEvidence =
              child.launchId && child.launchReceipt && !child.submissionId
                ? "launch_receipt"
                : "rejected_response";
          }
          const completed = basis ? [] : completedWorkerProblems(child);
          if (!basis && !completed.length) {
            // The behavior always tries its own observation first.
            // confirmedStopped stands in only when that observation is
            // impossible, never when it found the worker running.
            const unobserved = (cause: string) => {
              if (discard.confirmedStopped) return "attested" as const;
              throw new Error(
                `Cannot discard ${who}: ${cause}. The behavior could not verify that the completed worker stopped. Confirm the stop yourself (look for the pane in every session, including warped panes and orphaned pty-host supervisors, and check that its process has exited); once it has stopped, rerun discard-agent with the same request and confirmedStopped: true.`,
              );
            };
            const outcome = await context.runModel({
              definition: task.behaviorName,
              method: "confirm_stopped",
            });
            if (!outcome.ok) {
              if (outcome.error.message.includes(WORKER_NOT_STOPPED)) {
                throw new Error(
                  `Cannot discard ${who}: ${outcome.error.message}. The behavior observed the completed worker running, and confirmedStopped cannot override an observation. Wait for the worker to stop, or inspect it and close its pane, then rerun discard-agent with the same request.`,
                );
              }
              stopEvidence = unobserved(outcome.error.message);
            } else {
              const bytes = await context.dataRepository.getContent(
                "@yinz/agent",
                task.behaviorName,
                "observation",
              );
              const observation = bytes
                ? AgentObservationSchema.parse(
                  JSON.parse(new TextDecoder().decode(bytes)),
                )
                : undefined;
              // Only a fresh idle status of the worker's own pane proves the
              // stop; anything else is treated as unobserved.
              stopEvidence = !observation
                ? unobserved("the stop observation was not recorded")
                : observation.outcome === "ok" &&
                    observation.paneId === child.paneId
                ? "fresh_idle"
                : unobserved(
                  `the stop observation is not a fresh idle status of ${child.paneId}`,
                );
            }
            basis = "completed_unusable";
          }
          if (!basis) {
            throw new Error([
              `Cannot discard an uncertain or active pane task: no discard basis applies to ${who}.`,
              `- Completed worker: ${completed.join("; ")}`,
              `- Terminal failure: ${terminal.join("; ")}`,
              `- Rejected launch: ${rejected.join("; ")}`,
              "Retrieve the original submission for structured not_sent evidence or an acknowledged provider failure, then inspect the pane; legacy text is insufficient. Malformed responses, transport failures and legacy uncertain launches cannot be discarded. drive keeps observing an ordinary timeout; an explicit rollback or restart-agent with the user's request supersedes this task.",
            ].join("\n"));
          }
          const current = await loadState(context);
          if (stateDigest(current) !== taskBeforeDigest(task)) {
            const now = current as Record<string, unknown>;
            const before = await withBefore(context, task).then(
              (hydrated) => hydrated.before as Record<string, unknown>,
              () => undefined,
            );
            // Digests are canonical, so a mismatch always names a changed field.
            const changed = before
              ? [
                ...new Set([...Object.keys(now), ...Object.keys(before)]),
              ].filter((key) => canonical(now[key]) !== canonical(before[key]))
              : ["(dispatch snapshot unavailable)"];
            throw new Error(
              `Pane evidence may already be applied: lifecycle fields changed since ${who} was dispatched (${
                changed.join(", ")
              }); resume the task before discarding it`,
            );
          }
          const pending: Task = {
            ...task,
            discard: {
              request: discard.request,
              // The caller's flag; stopEvidence records whether it was used.
              ...(discard.confirmedStopped
                ? { confirmedStopped: true as const }
                : {}),
              basis,
              stopEvidence,
            },
          };
          await saveTask(context, pending);
          return finishDiscard(context, pending);
        }),
    },
    accept_submission: {
      kind: "action",
      description:
        "Explicitly import a pane task's accepted Swamp evidence when its pane observation never completes, without waiting on, prompting or closing the pane",
      arguments: AcceptSubmissionInputSchema.strip(),
      execute: (
        input: z.infer<typeof AcceptSubmissionInputSchema>,
        context: IssueAgentContext,
      ) =>
        dependencies.withIssue(context, async () => {
          await assertNoPendingRecovery(context);
          const args = AcceptSubmissionInputSchema.parse(input);
          const raw = await context.readResource("agent-task");
          const task = raw == null ? undefined : AgentTaskSchema.parse(raw);
          if (!task || task.id !== args.taskId) {
            throw new Error(
              "accept-submission must name the current pane task's taskId; inspect status before accepting",
            );
          }
          if (task.acceptance) {
            if (task.acceptance.request !== args.request) {
              throw new Error(
                "This task's accepted submission was already recorded under a different request",
              );
            }
            // Finish an interrupted import of the recorded acceptance.
            return applyResult(context, task, dependencies);
          }
          if (task.discard || task.status !== "running") {
            throw new Error(
              `accept-submission requires an unresolved running pane task; task ${task.id} is ${
                task.discard ? "being discarded" : task.status
              }. Use resume-agent to reconcile an applying task`,
            );
          }
          // Validates the receipt's task ID and assignment digest for this task.
          const receipt = await acceptedEvidence(context, task);
          const child = await childState(context, task);
          const { backgroundTasks } = reportedBackground(child);
          if (child.phase === "completed" && !backgroundTasks.length) {
            throw new Error(
              "The pane task completed normally; use resume-agent or drive to import it",
            );
          }
          // Yinz cannot tell a running foreground turn from one that ended with
          // background work left. Accept only after drive recorded its bounded
          // stall for this task, the child observed the turn's answer with
          // background work, or the pane closed. As in drive, any later child
          // observation (such as resume-agent's) supersedes the stall.
          const driver = await context.readResource("driver") as
            | {
              backgroundStall?: {
                taskId?: string;
                child?: {
                  phase?: string;
                  outcome?: string;
                  updatedAt?: string;
                };
              };
            }
            | null
            | undefined;
          const stall = driver?.backgroundStall;
          const stalled = stall?.taskId === task.id &&
            stall.child?.phase === child.phase &&
            stall.child.outcome === child.outcome &&
            stall.child.updatedAt === child.updatedAt;
          if (
            !stalled && child.phase !== "background" &&
            child.outcome !== "target_closed"
          ) {
            throw new Error(
              `accept-submission requires drive's background stall handoff for task ${task.id}, a background phase or a closed pane; the pane is in phase ${child.phase}. Use drive or resume-agent to keep observing it`,
            );
          }
          // Check requirements, lifecycle snapshot and the clean worktree at the
          // evidence commit before recording intent, exactly as import will.
          await validateResultFreshness(
            context,
            task,
            await loadState(context),
            dependencies,
            evidenceCommit(task, receipt),
          );
          const background = !backgroundTasks.length
            ? "none reported"
            : unidentifiedBackground(backgroundTasks)
            ? "reported without task IDs"
            : backgroundTasks.join(", ");
          const accepted: Task = {
            ...task,
            acceptance: {
              request: args.request,
              reason:
                `Operator accepted the Swamp evidence submission while pane observation was unresolved (phase ${child.phase}; background work: ${background}). The pane observation is superseded without waiting, input or closure.`,
              acceptedAt: dependencies.now(),
              childPhase: child.phase,
              backgroundTasks,
            },
          };
          await saveTask(context, accepted);
          return applyResult(context, accepted, dependencies);
        }),
    },
  };
}

export const agentMethods = createAgentMethods();
