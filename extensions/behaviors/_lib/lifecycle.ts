import { z } from "npm:zod@4.6.7";

const Text = z.string().trim().min(1);
const Commit = z.string().regex(/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/);
const Workspace = z.string().regex(/^[a-z0-9][a-z0-9_-]{0,63}$/);

export const IssueSchema = z.object({
  workspace: Workspace,
  itemId: z.number().int().positive().safe(),
  text: z.string().min(1),
  note: z.string(),
  digest: z.string().regex(/^[0-9a-f]{64}$/),
}).strict();
export type Issue = z.infer<typeof IssueSchema>;

export const FindingSchema = z.object({
  id: Text,
  severity: z.enum(["low", "medium", "high", "critical"]),
  description: Text,
  resolution: Text.optional(),
}).strict();
const FindingsSchema = z.array(FindingSchema).refine(
  (findings) =>
    new Set(findings.map((finding) => finding.id)).size === findings.length,
  "Finding IDs must be unique",
);
const PlanContentSchema = z.object({
  summary: Text,
  steps: z.array(
    z.object({
      id: Text,
      description: Text,
      files: z.array(Text),
    }).strict(),
  ).min(1).refine(
    (steps) => new Set(steps.map((step) => step.id)).size === steps.length,
    "Plan step IDs must be unique",
  ),
  testingStrategy: Text,
  assumptions: z.array(Text).optional(),
}).strict();
export const PlanSchema = PlanContentSchema.extend({
  version: z.number().int().positive(),
});
export type Plan = z.infer<typeof PlanSchema>;
const PlanReviewSchema = z.object({
  planVersion: z.number().int().positive(),
  findings: FindingsSchema,
}).strict();
const ApprovalSchema = z.object({
  planVersion: z.number().int().positive(),
  request: Text,
}).strict();
const ImplementationSchema = z.object({
  worktree: Text,
  branch: Text,
  head: Commit.optional(),
});
const CodeReviewSchema = z.object({
  planVersion: z.number().int().positive(),
  commit: Commit,
  steps: z.array(
    z.object({
      id: Text,
      status: z.enum(["implemented", "deviated"]),
      justification: Text.optional(),
    }).strict(),
  ),
  findings: FindingsSchema,
  preliminaryChecks: z.array(
    z.object({
      command: Text,
      result: z.enum(["passed", "failed", "skipped"]),
      summary: Text,
    }).strict(),
  ).optional(),
}).strict();
export const AttemptSchema = z.object({
  id: z.uuid(),
  originalHead: Commit,
  skipCiRequest: Text.optional(),
  startedAt: z.iso.datetime(),
}).strict();
export const ReceiptSchema = z.object({
  attemptId: z.uuid(),
  originalCommit: Commit,
  rebasedCommit: Commit,
  baseCommit: Commit,
  mergedCommit: Commit,
  worktree: Text,
  branch: Text,
  ciStatus: z.enum(["passed", "skipped"]),
  skipCiRequest: Text.optional(),
  mergedAt: z.iso.datetime(),
}).strict();
export const StateSchema = z.object({
  issue: IssueSchema,
  phase: z.enum([
    "triaging",
    "classified",
    "planning",
    "approved",
    "implementing",
    "ready",
    "merged",
    "done",
  ]),
  planVersion: z.number().int().nonnegative(),
  // Fences evidence from an earlier attempt even when rollback restores the same phase.
  rollbackId: z.uuid().optional(),
  classification: z.object({
    classification: z.enum(["bug", "feature", "maintenance", "security"]),
    summary: Text,
  }).optional(),
  plan: PlanSchema.optional(),
  planReview: PlanReviewSchema.optional(),
  approval: ApprovalSchema.optional(),
  implementation: ImplementationSchema.optional(),
  codeReview: CodeReviewSchema.optional(),
  mergeAuthorization: z.object({
    commit: Commit,
    planVersion: z.number().int().positive(),
    request: Text,
  }).optional(),
  attempt: AttemptSchema.optional(),
  receipt: ReceiptSchema.optional(),
  outcome: z.object({ outcomeMet: z.literal(true), summary: Text }).optional(),
  lastFailure: Text.optional(),
});
export type LifecycleState = z.infer<typeof StateSchema>;

export const LifecycleStepSchema = z.enum([
  "triage",
  "plan",
  "review_plan",
  "approve",
  "implement",
  "review_code",
  "ready",
  "merge",
  "summarize",
]);
export const RollbackInputSchema = z.object({
  step: LifecycleStepSchema,
  request: Text,
}).strict();

/** Rewind decisions, not source files or Git refs. Earlier evidence remains required. */
export function rollbackState(
  current: LifecycleState,
  step: z.infer<typeof LifecycleStepSchema>,
  id: string,
): LifecycleState {
  const state = StateSchema.parse(current);
  const index = LifecycleStepSchema.options.indexOf(
    LifecycleStepSchema.parse(step),
  );
  const required = [
    state.classification,
    state.plan,
    state.planReview,
    state.approval,
    state.implementation,
    state.codeReview,
    state.mergeAuthorization,
    state.receipt,
  ];
  for (let previous = 0; previous < index; previous++) {
    if (!required[previous]) {
      throw new Error(
        `Cannot restart ${step}: missing ${
          LifecycleStepSchema.options[previous]
        } evidence`,
      );
    }
  }
  state.rollbackId = z.uuid().parse(id);
  delete state.outcome;
  delete state.lastFailure;
  delete state.attempt;
  // A merge which already landed is a fact. Retrying merge/close-out reuses its receipt.
  if (index >= 7 && state.receipt) {
    state.phase = "merged";
    return state;
  }
  delete state.receipt;
  if (index <= 6) delete state.mergeAuthorization;
  if (index <= 5) {
    delete state.codeReview;
    if (state.implementation) delete state.implementation.head;
  }
  if (index <= 3) delete state.approval;
  if (index <= 2) delete state.planReview;
  if (index <= 1) delete state.plan;
  if (index === 0) delete state.classification;
  // Keep the worktree/branch binding, including dirty work, for subsequent attempts.
  const phases = [
    "triaging",
    "classified",
    "planning",
    "planning",
    "approved",
    "implementing",
    "implementing",
    "ready",
    "merged",
  ] as const;
  state.phase = step === "implement" && state.implementation
    ? "implementing"
    : phases[index];
  return StateSchema.parse(state);
}

/** The behavior adapter and pure transition layer share the same public input contract. */
export const ActionSchemas = {
  triage: StateSchema.shape.classification.unwrap().strict(),
  plan: PlanContentSchema,
  review_plan: PlanReviewSchema,
  approve: ApprovalSchema,
  implement: ImplementationSchema.omit({ head: true }).strict(),
  review_code: CodeReviewSchema,
  ready: z.object({ commit: Commit, mergeRequest: Text }).strict(),
  begin_merge: z.object({ attempt: AttemptSchema }).strict(),
  repair: z.object({ reason: Text }).strict(),
  merged: z.object({ receipt: ReceiptSchema }).strict(),
  done: StateSchema.shape.outcome.unwrap().strict(),
};
export type LifecycleAction = keyof typeof ActionSchemas;

export function createState(issue: Issue): LifecycleState {
  return StateSchema.parse({ issue, phase: "triaging", planVersion: 0 });
}

function requirePhase(
  state: LifecycleState,
  phases: LifecycleState["phase"][],
): void {
  if (!phases.includes(state.phase)) {
    throw new Error(
      `Operation requires ${phases.join(" or ")}; issue is ${state.phase}`,
    );
  }
}

function requirePlan(state: LifecycleState, version?: number): Plan {
  if (
    !state.plan || (version !== undefined && state.plan.version !== version)
  ) {
    throw new Error("Evidence must reference the current plan version");
  }
  return state.plan;
}

function requireResolved(findings: z.infer<typeof FindingsSchema>): void {
  const unresolved = findings.filter((finding) =>
    ["critical", "high"].includes(finding.severity) && !finding.resolution
  );
  if (unresolved.length) {
    throw new Error(
      `Resolve high/critical findings first: ${
        unresolved.map((f) => f.id).join(", ")
      }`,
    );
  }
}

function clearReview(state: LifecycleState): void {
  delete state.codeReview;
  delete state.mergeAuthorization;
  delete state.attempt;
  if (state.implementation) delete state.implementation.head;
}

/** A changed Agenda description invalidates every decision based on its old contents. */
export function refreshIssue(
  current: LifecycleState,
  issue: Issue,
): LifecycleState {
  const state = StateSchema.parse(current);
  const next = IssueSchema.parse(issue);
  if (
    next.workspace !== state.issue.workspace ||
    next.itemId !== state.issue.itemId
  ) {
    throw new Error("Cannot replace an issue with a different Agenda identity");
  }
  if (next.digest === state.issue.digest) return state;
  requirePhase(state, [
    "triaging",
    "classified",
    "planning",
    "approved",
    "implementing",
    "ready",
  ]);
  if (state.attempt) {
    throw new Error(
      "Repair the active merge attempt before refreshing its issue",
    );
  }
  state.issue = next;
  state.phase = "triaging";
  delete state.classification;
  delete state.plan;
  delete state.planReview;
  delete state.approval;
  delete state.lastFailure;
  clearReview(state);
  return state;
}

/** Pure transitions: callers persist the returned value after validating external effects. */
export function transition(
  current: LifecycleState,
  action: string,
  args: unknown,
): LifecycleState {
  const state = StateSchema.parse(current);
  if (state.attempt && !["merged", "repair"].includes(action)) {
    throw new Error("Finish or repair the active merge attempt first");
  }
  switch (action) {
    case "triage": {
      requirePhase(state, ["triaging", "classified"]);
      state.classification = ActionSchemas.triage.parse(args);
      state.phase = "classified";
      break;
    }
    case "plan": {
      requirePhase(state, [
        "classified",
        "planning",
        "approved",
        "implementing",
        "ready",
      ]);
      const content = ActionSchemas.plan.parse(args);
      state.planVersion += 1;
      state.plan = { ...content, version: state.planVersion };
      state.phase = "planning";
      delete state.planReview;
      delete state.approval;
      delete state.lastFailure;
      clearReview(state);
      break;
    }
    case "review_plan": {
      requirePhase(state, ["planning"]);
      const review = ActionSchemas.review_plan.parse(args);
      requirePlan(state, review.planVersion);
      state.planReview = review;
      break;
    }
    case "approve": {
      requirePhase(state, ["planning"]);
      const approval = ActionSchemas.approve.parse(args);
      requirePlan(state, approval.planVersion);
      if (
        !state.planReview ||
        state.planReview.planVersion !== approval.planVersion
      ) {
        throw new Error("The current plan needs a review before approval");
      }
      requireResolved(state.planReview.findings);
      state.approval = approval;
      state.phase = "approved";
      break;
    }
    case "implement": {
      requirePhase(state, ["approved", "implementing"]);
      const implementation = ActionSchemas.implement.parse(args);
      const plan = requirePlan(state);
      if (state.approval?.planVersion !== plan.version) {
        throw new Error("Approve the current plan first");
      }
      if (
        state.implementation &&
        (state.implementation.worktree !== implementation.worktree ||
          state.implementation.branch !== implementation.branch)
      ) {
        throw new Error(
          "Implementation must remain in its existing worktree and branch",
        );
      }
      state.implementation = implementation;
      state.phase = "implementing";
      clearReview(state);
      break;
    }
    case "review_code": {
      requirePhase(state, ["implementing", "ready"]);
      const review = ActionSchemas.review_code.parse(args);
      const plan = requirePlan(state, review.planVersion);
      if (
        !state.implementation || state.approval?.planVersion !== plan.version
      ) {
        throw new Error(
          "Code review requires implementation of the approved plan",
        );
      }
      const ids = new Set(review.steps.map((step) => step.id));
      if (
        ids.size !== review.steps.length || ids.size !== plan.steps.length ||
        plan.steps.some((step) => !ids.has(step.id))
      ) {
        throw new Error("Code review must cover every plan step exactly once");
      }
      if (
        review.steps.some((step) =>
          step.status === "deviated" && !step.justification
        )
      ) {
        throw new Error("Each plan deviation needs a justification");
      }
      clearReview(state);
      state.codeReview = review;
      state.implementation.head = review.commit;
      state.phase = "implementing";
      break;
    }
    case "ready": {
      requirePhase(state, ["implementing"]);
      const ready = ActionSchemas.ready.parse(args);
      const plan = requirePlan(state);
      if (
        !state.codeReview || state.codeReview.planVersion !== plan.version ||
        state.codeReview.commit !== ready.commit ||
        state.implementation?.head !== ready.commit ||
        state.approval?.planVersion !== plan.version
      ) {
        throw new Error(
          "Merge authorization must match the reviewed commit and approved plan",
        );
      }
      requireResolved(state.codeReview.findings);
      if (
        state.codeReview.preliminaryChecks?.some((check) =>
          check.result === "failed"
        )
      ) {
        throw new Error(
          "Resolve failed preliminary checks before requesting a merge",
        );
      }
      state.mergeAuthorization = {
        commit: ready.commit,
        planVersion: plan.version,
        request: ready.mergeRequest,
      };
      state.phase = "ready";
      break;
    }
    case "begin_merge": {
      requirePhase(state, ["ready"]);
      const { attempt } = ActionSchemas.begin_merge.parse(args);
      if (
        !state.mergeAuthorization ||
        attempt.originalHead !== state.mergeAuthorization.commit
      ) {
        throw new Error("Merge attempt does not match the authorized commit");
      }
      state.attempt = attempt;
      delete state.lastFailure;
      break;
    }
    case "repair": {
      requirePhase(state, ["implementing", "ready"]);
      const { reason } = ActionSchemas.repair.parse(args);
      clearReview(state);
      state.phase = "implementing";
      state.lastFailure = reason;
      break;
    }
    case "merged": {
      requirePhase(state, ["ready"]);
      const { receipt } = ActionSchemas.merged.parse(args);
      if (
        !state.attempt || receipt.attemptId !== state.attempt.id ||
        receipt.originalCommit !== state.attempt.originalHead ||
        receipt.originalCommit !== state.mergeAuthorization?.commit ||
        receipt.worktree !== state.implementation?.worktree ||
        receipt.branch !== state.implementation?.branch
      ) {
        throw new Error(
          "Merge receipt must match the active authorized attempt and implementation",
        );
      }
      if (receipt.mergedCommit !== receipt.rebasedCommit) {
        throw new Error("Merged commit must be the verified rebased commit");
      }
      if (
        receipt.ciStatus === "skipped" &&
        (!state.attempt.skipCiRequest ||
          receipt.skipCiRequest !== state.attempt.skipCiRequest)
      ) {
        throw new Error(
          "Skipped CI requires the explicit request recorded on this attempt",
        );
      }
      if (receipt.ciStatus === "passed" && receipt.skipCiRequest) {
        throw new Error(
          "Passed CI receipts cannot also claim a CI skip request",
        );
      }
      state.receipt = receipt;
      state.phase = "merged";
      delete state.attempt;
      delete state.lastFailure;
      break;
    }
    case "done": {
      requirePhase(state, ["merged"]);
      if (!state.receipt) {
        throw new Error("Completion requires a verified local merge receipt");
      }
      state.outcome = ActionSchemas.done.parse(args);
      state.phase = "done";
      break;
    }
    default:
      throw new Error(`Unknown lifecycle action: ${action}`);
  }
  return StateSchema.parse(state);
}

export async function digestIssue(
  issue: Pick<Issue, "workspace" | "itemId" | "text" | "note">,
): Promise<string> {
  // Stable lexicographic keys; excludes mutable classifications and completion state.
  const value = JSON.stringify({
    id: issue.itemId,
    note: issue.note,
    text: issue.text,
    workspace: issue.workspace,
  });
  const hash = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(value),
  );
  return Array.from(
    new Uint8Array(hash),
    (byte) => byte.toString(16).padStart(2, "0"),
  ).join("");
}

export type AgendaCommand = (args: string[]) => Promise<unknown>;

export async function agendaCommand(args: string[]): Promise<unknown> {
  const result = await new Deno.Command("agenda", {
    args,
    stdout: "piped",
    stderr: "piped",
  }).output();
  const decode = new TextDecoder();
  if (!result.success) {
    throw new Error(
      `Agenda exited ${result.code}: ${decode.decode(result.stderr).trim()}`,
    );
  }
  try {
    return JSON.parse(decode.decode(result.stdout));
  } catch (cause) {
    throw new Error("Agenda returned invalid JSON", { cause });
  }
}

const AssignmentSchema = z.object({
  category: z.number().int(),
  item: z.number().int(),
  source: z.string(),
  value: z.union([
    z.literal("standard"),
    z.object({ text: z.string() }),
    z.object({ number: z.number() }),
    z.object({
      date: z.object({
        temporal: z.object({ kind: z.enum(["floating", "zoned", "instant"]) })
          .passthrough(),
        recurrence: z.object({}).passthrough().nullable(),
      }),
    }),
  ]),
});
const AgendaItemSchema = z.object({
  id: z.number().int().positive().safe(),
  kind: z.enum(["task", "note"]),
  text: z.string().min(1),
  note: z.string(),
  completed: z.boolean(),
  blocked: z.boolean(),
  discarded_at: z.number().int().nullable(),
  assignments: z.array(AssignmentSchema),
});
const AgendaEnvelopeSchema = z.object({
  schema_version: z.literal(0),
  item: AgendaItemSchema,
});

async function agendaRecord(
  workspace: string,
  itemId: number,
  command: AgendaCommand,
) {
  Workspace.parse(workspace);
  z.number().int().positive().safe().parse(itemId);
  const envelope = AgendaEnvelopeSchema.parse(
    await command([
      "--workspace",
      workspace,
      "--json",
      "item",
      "show",
      String(itemId),
    ]),
  );
  if (envelope.item.id !== itemId) {
    throw new Error("Agenda returned a different item ID");
  }
  return envelope.item;
}

function validateAgendaItem(
  item: z.infer<typeof AgendaItemSchema>,
  allowCompleted: boolean,
): void {
  if (item.kind !== "task") {
    throw new Error("Agenda issues must be tasks, not reference notes");
  }
  if (item.discarded_at !== null) throw new Error("Agenda issue is trashed");
  if (item.completed && !allowCompleted) {
    throw new Error("Agenda issue is already completed");
  }
  if (item.blocked) {
    throw new Error("Agenda issue has unfinished prerequisites");
  }
  if (
    item.assignments.some(({ value }) =>
      typeof value === "object" && "date" in value &&
      value.date.recurrence !== null
    )
  ) throw new Error("Recurring Agenda tasks cannot be development issues");
}

export async function readAgenda(
  workspace: string,
  itemId: number,
  allowCompleted = false,
  command: AgendaCommand = agendaCommand,
): Promise<Issue> {
  const item = await agendaRecord(workspace, itemId, command);
  validateAgendaItem(item, allowCompleted);
  const issue = { workspace, itemId, text: item.text, note: item.note };
  return IssueSchema.parse({ ...issue, digest: await digestIssue(issue) });
}

/** Rollback reopens a completed task without changing its requirements. Safe to retry. */
export async function reopenAgenda(
  issue: Issue,
  command: AgendaCommand = agendaCommand,
): Promise<void> {
  const item = await agendaRecord(issue.workspace, issue.itemId, command);
  validateAgendaItem(item, true);
  const digest = await digestIssue({
    ...issue,
    text: item.text,
    note: item.note,
  });
  if (digest !== issue.digest) {
    throw new Error(
      "Agenda description changed during rollback; inspect the recovery record",
    );
  }
  if (item.completed) {
    await command([
      "--workspace",
      issue.workspace,
      "--json",
      "item",
      "reopen",
      String(issue.itemId),
    ]);
  }
  const after = await readAgenda(issue.workspace, issue.itemId, false, command);
  if (after.digest !== issue.digest) {
    throw new Error(
      "Agenda description changed during rollback; inspect the recovery record",
    );
  }
}

/** Complete only the captured issue; retries preserve its description and verify the result. */
export async function completeAgenda(
  issue: Issue,
  command: AgendaCommand = agendaCommand,
): Promise<Issue> {
  const expected = IssueSchema.parse(issue);
  const current = await readAgenda(
    expected.workspace,
    expected.itemId,
    true,
    command,
  );
  if (current.digest !== expected.digest) {
    throw new Error("Agenda description changed before completion");
  }
  const completion = z.object({
    schema_version: z.literal(0),
    items: z.array(AgendaItemSchema),
    completion: z.object({
      completed: z.number().int().nonnegative(),
      reopened: z.number().int().nonnegative(),
      created: z.literal(0),
      created_items: z.array(AgendaItemSchema).length(0),
    }),
  }).parse(
    await command([
      "--workspace",
      expected.workspace,
      "--json",
      "item",
      "done",
      String(expected.itemId),
    ]),
  );
  if (
    completion.items.length !== 1 ||
    completion.items[0].id !== expected.itemId ||
    !completion.items[0].completed
  ) throw new Error("Agenda did not confirm issue completion");
  const after = await agendaRecord(
    expected.workspace,
    expected.itemId,
    command,
  );
  validateAgendaItem(after, true);
  if (!after.completed) {
    throw new Error("Agenda issue is still pending after completion");
  }
  const result = {
    workspace: expected.workspace,
    itemId: expected.itemId,
    text: after.text,
    note: after.note,
  };
  const digest = await digestIssue(result);
  if (digest !== expected.digest) {
    throw new Error(
      "Agenda description changed during completion; inspect the item",
    );
  }
  return IssueSchema.parse({ ...result, digest });
}
